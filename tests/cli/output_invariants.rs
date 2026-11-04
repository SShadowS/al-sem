//! Output invariants (MCP output plan `docs/superpowers/plans/2026-11-04-mcp-output-fixes.md`,
//! item T): properties of what explain, prove, digest and query print, checked on the
//! cross-app fixture `tests/fixtures/mcp-output/` (see `output_fixture`). A golden pins
//! what the engine prints; these pin that it is GOOD, so a blank name or an off-by-one
//! line fails a test even when someone regenerates the goldens.
//!
//! Each invariant lands with the plan item that makes it true. Today:
//! - 1a: [`anchor_source_check_on_the_fixture`] (every `file:line` a human output prints
//!   opens to the line it describes) and [`every_document_states_line_base_1`].
//! - #368: [`evidence_anchors_open_to_what_they_describe`] (the same for every
//!   `sourceAnchor` of `analyze --with-evidence` and `policy check`).
//!
//! The CDO twin is `output_cdo` (the same checks on three real routines, under
//! `scripts/cdo-gate`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

use crate::output_fixture::{ISSUE_DOC, ISSUE_DOC_Q, WRITE_LOG, run_alsem, workspace};

/// `aldump --dump-dependency-source <ws> <out>`: each dependency's AL at
/// `<out>/<guid>/<virtual path>`, the file a `dep:<guid>:<path>` anchor names.
pub(crate) fn dump_dependency_source(ws: &Path, out: &Path) {
    let o = Command::new(env!("CARGO_BIN_EXE_aldump"))
        .arg("--dump-dependency-source")
        .arg(ws)
        .arg(out)
        .env("RUST_LOG", "off")
        .output()
        .expect("spawn aldump");
    assert!(
        o.status.success(),
        "aldump --dump-dependency-source: {}",
        String::from_utf8_lossy(&o.stderr)
    );
}

/// One `file:line` a human output printed, with the words the line at it must hold
/// (any one, case-insensitive). `tokens` empty: the output line names nothing the
/// source line must contain (e.g. a dependency boundary group, anchored at the call
/// that enters the boundary), so the anchor is counted as unverifiable.
#[derive(Debug)]
pub(crate) struct Anchor {
    pub file: String,
    pub line: usize,
    pub tokens: Vec<String>,
    pub printed: String,
}

/// The words a DB/effect type's operation is spelled with in AL.
fn type_verbs(ty: &str) -> Vec<&'static str> {
    match ty {
        "DB_INSERT" => vec!["insert"],
        "DB_MODIFY" => vec!["modify", "rename", "validate"],
        "DB_DELETE" => vec!["delete"],
        "COMMIT" => vec!["commit"],
        "UI_MESSAGE" => vec!["message"],
        "UI_CONFIRM" => vec!["confirm"],
        "UI_ERROR" | "ERROR_THROW" => vec!["error"],
        "BACKGROUND_TASK" => vec!["createtask", "startsession"],
        "HTTP" => vec!["get", "post", "put", "send", "delete"],
        _ => vec![],
    }
}

/// The last identifier of a dotted display (`Handler.Handle` -> `handle`,
/// `Codeunit.Run(<dynamic>)` -> `run`, `Out Base.OnRun` -> `run`: a trigger is
/// entered by a `Run`, `Insert`, ... call).
fn last_segment(display: &str) -> String {
    let head = display.split('(').next().unwrap_or(display);
    // `Obj.Routine` and the owner-qualified `Codeunit X::Routine` (plan 1d).
    let seg = head
        .rsplit(['.', ':'])
        .next()
        .unwrap_or(head)
        .trim()
        .to_lowercase();
    let qualified = display.contains('.') || display.contains("::");
    match seg.strip_prefix("on") {
        Some(rest) if !rest.is_empty() && qualified => rest.to_string(),
        _ => seg,
    }
}

fn between<'a>(s: &'a str, open: &str, close: &str) -> Option<&'a str> {
    let i = s.find(open)? + open.len();
    let j = s[i..].find(close)? + i;
    Some(&s[i..j])
}

/// Every `ws:`/`dep:` anchor in `text` (one human output), with its tokens.
pub(crate) fn human_anchors(text: &str) -> Vec<Anchor> {
    let re_anchor = |s: &str| -> Vec<(String, usize, usize)> {
        // (file, line, byte offset of the anchor) for every `ws:...:N` / `dep:...:N`.
        let mut out = Vec::new();
        let mut from = 0;
        while let Some(rel) = s[from..].find(['w', 'd']) {
            let at = from + rel;
            from = at + 1;
            let rest = &s[at..];
            if !(rest.starts_with("ws:") || rest.starts_with("dep:")) {
                continue;
            }
            if at > 0 && s.as_bytes()[at - 1].is_ascii_alphanumeric() {
                continue;
            }
            let end = rest
                .find(|c: char| c.is_whitespace() || c == ']' || c == '"' || c == ')')
                .unwrap_or(rest.len());
            let tok = &rest[..end];
            // `file:line` or `file:line:col` (the fingerprint form).
            let parts: Vec<&str> = tok.rsplitn(3, ':').collect();
            let (file, line) = match parts.as_slice() {
                [last, mid, file]
                    if mid.parse::<usize>().is_ok() && last.parse::<usize>().is_ok() =>
                {
                    (file.to_string(), mid.parse().unwrap())
                }
                [last, file] => match last.parse::<usize>() {
                    Ok(n) => (file.to_string(), n),
                    Err(_) => continue,
                },
                [last, mid, file] => match last.parse::<usize>() {
                    Ok(n) => (format!("{file}:{mid}"), n),
                    Err(_) => continue,
                },
                _ => continue,
            };
            out.push((file, line, at));
            from = at + end;
        }
        out
    };
    let mut anchors = Vec::new();
    for l in text.lines() {
        let t = l.trim();
        for (file, line, _) in re_anchor(t) {
            let tokens = line_tokens(t);
            anchors.push(Anchor {
                file,
                line,
                tokens,
                printed: t.to_string(),
            });
        }
    }
    anchors
}

/// What the source line behind the anchor on output line `t` must contain.
fn line_tokens(t: &str) -> Vec<String> {
    let mut v: Vec<String> = Vec::new();
    // digest human: `evidence: <anchor> "<excerpt>"`.
    if t.starts_with("evidence:") {
        if let Some(x) = between(t, "\"", "\"") {
            v.push(last_segment(x.rsplit(' ').next().unwrap_or(x)));
        }
        return v;
    }
    // query: `[Op] on T via ...` / `+1 Caller [Op] via ...`; touches (W2-3):
    // `[Op] <family> at <anchor> in <owner>`.
    if let Some(op) = between(t, "[", "]")
        && op.chars().all(|c| c.is_ascii_alphabetic())
        && op.chars().next().is_some_and(|c| c.is_ascii_uppercase())
        && !op.starts_with("DB_")
        && op != "COMMIT"
        && (t.contains(" via ")
            || t.starts_with('+')
            || [
                " write at ",
                " read at ",
                " lock at ",
                " validate at ",
                " unclassified at ",
            ]
            .iter()
            .any(|f| t.contains(&format!("[{op}]{f}"))))
    {
        v.push(op.to_lowercase());
        return v;
    }
    // W2-2, a crossing's site: `<Object>::<Routine>, entered at <site> ...`:
    // the call that enters the target (`Run` for an `OnRun`, `Insert` for an
    // `OnInsert` trigger, else the routine's name).
    if let Some((head, _)) = t.split_once(", entered at ")
        && let Some((_, routine)) = head.rsplit_once("::")
    {
        let r = routine.trim().to_lowercase();
        if let Some(rest) = r.strip_prefix("on").filter(|s| !s.is_empty()) {
            v.push(rest.to_string());
        }
        v.push(r);
        return v;
    }
    // W2-2, query effects: `<Table>: <ops> — first <Kind> at <anchor> in …`.
    if let Some((_, rest)) = t.split_once(" — first ")
        && let Some(kind) = rest.split(' ').next()
    {
        v.push(kind.to_lowercase());
        return v;
    }
    // W2-2, a workspace write's witness anchor: `<Kind> at <anchor>`; a commit
    // site: `COMMIT at <anchor>`.
    // (Not a kind family line, `UI at 2 places: …`, handled below.)
    if let Some((kind, rest)) = t.split_once(" at ")
        && !kind.contains(' ')
        && kind.chars().next().is_some_and(|c| c.is_ascii_uppercase())
        && !rest.contains(" place")
    {
        if kind == "COMMIT" || kind.contains('_') || kind == "HTTP" || kind == "FILE" {
            v.extend(type_verbs(kind).iter().map(|s| s.to_string()));
        } else {
            v.push(kind.to_lowercase());
        }
        return v;
    }
    // W2-2, a kind family line: `HTTP at 1 place: HTTP at <anchor> (...); …`.
    if [
        "UI at ",
        "HTTP at ",
        "file at ",
        "background tasks at ",
        "isolated storage at ",
        "telemetry at ",
        "errors at ",
    ]
    .iter()
    .any(|p| t.starts_with(p))
    {
        // Several places of different types on one line, and an implicit
        // error (`Get`, `FindFirst`, `TestField`) has no verb of its own:
        // the line's anchors are unverifiable here, as before W2-2.
        return v;
    }
    // W2-2, no path kept: `in <App> code (n tables): T (Kind in Owner at <anchor>), …`.
    if t.starts_with("in ") && t.contains(" code (") {
        for seg in t.split(" (").skip(2) {
            if let Some(k) = seg.split(' ').next() {
                v.push(k.to_lowercase());
            }
        }
        return v;
    }
    // explain rules: `[TestField ws:...]`, `[Error ws:...]`.
    for (k, w) in [("[TestField ", "testfield"), ("[Error ", "error")] {
        if t.contains(k) {
            v.push(w.to_string());
            return v;
        }
    }
    // explain card header: `<Object> <Name>::<Routine>  ws:...`.
    if !t.starts_with('-')
        && let Some((head, _)) = t.split_once("  ws:").or_else(|| t.split_once("  dep:"))
        && let Some((_, routine)) = head.rsplit_once("::")
    {
        v.push(routine.trim().to_lowercase());
        return v;
    }
    // explain writes: `writes <Table> [fields] (DB_X[, cond]) at ...`; a blank
    // table is a boundary group anchored at the call entering it: unverifiable.
    if let Some(rest) = t.strip_prefix("writes ") {
        if rest.starts_with(' ') || rest.starts_with('(') {
            return v;
        }
        if let Some(ty) = between(t, "(DB_", ")") {
            let ty = format!("DB_{}", ty.split(',').next().unwrap_or(ty));
            v.extend(type_verbs(&ty).iter().map(|s| s.to_string()));
        }
        return v;
    }
    // explain raises: `raises <Event> (cond) at ...`; blank: a group.
    if let Some(rest) = t.strip_prefix("raises ") {
        if !rest.starts_with(' ') && !rest.starts_with('(') {
            v.push(rest.split(' ').next().unwrap_or("").to_lowercase());
        }
        return v;
    }
    if let Some(rest) = t.strip_prefix("reached from page action ") {
        v.push(rest.split(' ').next().unwrap_or("").to_lowercase());
        return v;
    }
    if let Some(callee) = between(t, "(callee ", ", status") {
        v.push(last_segment(callee));
        return v;
    }
    // explain UI: `UI_MESSAGE at ...`; prove evidence: `[TYPE] direct — ...`.
    if let Some(ty) = t.split(' ').next()
        && ty.starts_with("UI_")
    {
        v.extend(type_verbs(ty).iter().map(|s| s.to_string()));
        return v;
    }
    if let Some(ty) = between(t, "[", "]")
        && t.contains("] direct")
    {
        v.extend(type_verbs(ty).iter().map(|s| s.to_string()));
        return v;
    }
    // query global list: `<Object>::<Routine> — ws:...`.
    if let Some((head, _)) = t.split_once(" — ")
        && let Some((_, routine)) = head.rsplit_once("::")
        && !t.contains(" via ")
    {
        v.push(routine.trim().to_lowercase());
        return v;
    }
    v
}

/// The text of a `ws:` file (relative to `ws`) or a `dep:<guid>:<path>` file (under
/// `dep_root`, as the dump writes it). `None`: the file is not there.
pub(crate) fn source_of(file: &str, ws: &Path, dep_root: &Path) -> Option<String> {
    let path: PathBuf = if let Some(rel) = file.strip_prefix("ws:") {
        ws.join(rel)
    } else if let Some(rest) = file.strip_prefix("dep:") {
        let (guid, rel) = rest.split_once(':')?;
        dep_root.join(guid).join(rel)
    } else {
        return None;
    };
    std::fs::read_to_string(path)
        .ok()
        .map(|s| s.replace('\r', ""))
}

/// The outcome of checking anchors: how many matched, which did not, how many had
/// no token (unverifiable) or no file (skipped).
#[derive(Default, Debug)]
pub(crate) struct Checked {
    pub matched: usize,
    /// Of `matched`, the `dep:` anchors.
    pub matched_dep: usize,
    pub unverifiable: usize,
    pub missing_file: usize,
    pub failures: Vec<String>,
}

/// Each anchor's 1-based line must hold one of its tokens.
pub(crate) fn check_anchors(anchors: &[Anchor], ws: &Path, dep_root: &Path) -> Checked {
    let mut c = Checked::default();
    let mut cache: BTreeMap<String, Option<String>> = BTreeMap::new();
    for a in anchors {
        if a.tokens.is_empty() || a.tokens.iter().all(String::is_empty) {
            c.unverifiable += 1;
            continue;
        }
        let text = cache
            .entry(a.file.clone())
            .or_insert_with(|| source_of(&a.file, ws, dep_root));
        let Some(text) = text else {
            c.missing_file += 1;
            continue;
        };
        let line = a
            .line
            .checked_sub(1)
            .and_then(|i| text.lines().nth(i))
            .unwrap_or("")
            .to_lowercase();
        if a.tokens.iter().any(|t| !t.is_empty() && line.contains(t)) {
            c.matched += 1;
            c.matched_dep += usize::from(a.file.starts_with("dep:"));
        } else {
            c.failures.push(format!(
                "{}:{} is {line:?}, wanted one of {:?} — printed as: {}",
                a.file, a.line, a.tokens, a.printed
            ));
        }
    }
    c
}

/// The human outputs the fixture invariants read: explain, prove, digest and query
/// for `IssueDoc` and `WriteLog`.
fn fixture_human_outputs() -> Vec<(String, String)> {
    let ws = workspace().to_string_lossy().into_owned();
    let runs: Vec<Vec<&str>> = vec![
        vec!["explain", &ws, "--routine", ISSUE_DOC],
        vec!["explain", &ws, "--routine", WRITE_LOG],
        vec!["prove", &ws, ISSUE_DOC, "may-commit"],
        vec!["prove", &ws, ISSUE_DOC, "writes-table:Out Doc"],
        vec!["prove", &ws, WRITE_LOG, "writes-table:Out Log"],
        vec!["digest", &ws, "--routine", ISSUE_DOC],
        vec!["digest", &ws, "--routine", WRITE_LOG, "--include-reads"],
        vec!["query", "effects", &ws, "--routine", ISSUE_DOC_Q],
        vec![
            "query",
            "touches",
            &ws,
            "--table",
            "Out Log",
            "--from",
            ISSUE_DOC_Q,
        ],
        vec!["query", "touches", &ws, "--table", "Out Doc"],
    ];
    runs.into_iter()
        .map(|mut a| {
            let label = a.join(" ").replace(&ws, "{ws}");
            a.extend(["--format", "human"]);
            let (out, err, code) = run_alsem(&a);
            assert_eq!(code, 0, "{label}: {err}");
            (label, out)
        })
        .collect()
}

/// Item T, "anchor source-check" (catches B8: 0-based lines printed as `file:line`):
/// every `file:line` in the fixture's human outputs opens to a line holding the
/// operation verb, excerpt, event, callee or routine the output line names. `ws:`
/// anchors resolve against the workspace, `dep:` ones against
/// `aldump --dump-dependency-source`. A boundary group's anchor (the call entering
/// the boundary) names nothing to look for and is counted, not checked.
///
/// Discrimination (2026-11-04): printing the 0-based row again in explain's
/// `anchor_human`, digest's and prove's `ev_line` and query's `origin_human`
/// (`shown(l)` -> `l`) fails this test: 34 of the 46 checked anchors open one line
/// early (the other 12 print a JSON anchor, already 1-based); restored, it passes.
#[test]
fn anchor_source_check_on_the_fixture() {
    let dump = tempfile::tempdir().unwrap();
    dump_dependency_source(&workspace(), dump.path());
    let mut all = Checked::default();
    let mut per_output = Vec::new();
    for (label, out) in fixture_human_outputs() {
        let anchors = human_anchors(&out);
        let c = check_anchors(&anchors, &workspace(), dump.path());
        per_output.push(format!(
            "{label}: {} anchors, {} matched, {} unverifiable",
            anchors.len(),
            c.matched,
            c.unverifiable
        ));
        all.matched += c.matched;
        all.matched_dep += c.matched_dep;
        all.unverifiable += c.unverifiable;
        all.missing_file += c.missing_file;
        all.failures
            .extend(c.failures.into_iter().map(|f| format!("[{label}] {f}")));
    }
    assert!(
        all.failures.is_empty(),
        "anchors that do not open to what they print:\n{}\n\n{}",
        all.failures.join("\n"),
        per_output.join("\n")
    );
    assert_eq!(
        all.missing_file, 0,
        "every anchor's file exists: {per_output:?}"
    );
    eprintln!("{}", per_output.join("\n"));
    // Precondition: the check looked at enough anchors, `dep:` ones included.
    assert!(
        all.matched >= 25 && all.matched_dep >= 1,
        "only {} anchors checked ({} in dependency code):\n{}",
        all.matched,
        all.matched_dep,
        per_output.join("\n")
    );
}

/// The `lineBase` of a document of `kind`: where each kind states it.
fn line_base_of(doc: &Value) -> &Value {
    if doc.get("schema").is_some() {
        &doc["lineBase"] // explain: top level
    } else {
        &doc["payload"]["lineBase"]
    }
}

/// The first `"line"` number anywhere under `v`, depth first.
fn first_line(v: &Value) -> Option<u64> {
    match v {
        Value::Object(m) => {
            if let Some(n) = m.get("line").and_then(Value::as_u64) {
                return Some(n);
            }
            m.values().find_map(first_line)
        }
        Value::Array(a) => a.iter().find_map(first_line),
        _ => None,
    }
}

/// Item T, "every JSON document states `lineBase`, and it equals the base the human
/// form uses" (catches E5): explain, prove, digest, query effects and query touches
/// each say `lineBase: 1`, and one anchor of each is printed with the same number in
/// the human form.
///
/// Discrimination (2026-11-04): removing the `lineBase` insert from prove's payload
/// fails this test at `prove`; restored, it passes.
#[test]
fn every_document_states_line_base_1() {
    let ws = workspace().to_string_lossy().into_owned();
    let runs: Vec<(&str, Vec<&str>)> = vec![
        ("explain", vec!["explain", &ws, "--routine", ISSUE_DOC]),
        (
            "prove",
            vec!["prove", &ws, ISSUE_DOC, "writes-table:Out Doc"],
        ),
        ("digest", vec!["digest", &ws, "--routine", ISSUE_DOC]),
        (
            "query effects",
            // `--all` (W2-2): the default human form prints the first row per
            // table only; every row's line with `--all`.
            vec!["query", "effects", &ws, "--routine", ISSUE_DOC_Q, "--all"],
        ),
        (
            "query touches",
            // `--all`: the default human form folds temporary-record touches
            // into one counted line (W2-3), so the JSON's first witness line is
            // printed only when every witness is listed.
            vec![
                "query",
                "touches",
                &ws,
                "--table",
                "Out Log",
                "--from",
                ISSUE_DOC_Q,
                "--all",
            ],
        ),
        (
            "fingerprint inventory",
            vec!["fingerprint", &ws, "--inventory-only"],
        ),
    ];
    let mut failures = Vec::new();
    for (label, argv) in runs {
        let mut a = argv.clone();
        if label != "fingerprint inventory" {
            a.extend(["--format", "json"]);
        }
        let (out, err, code) = run_alsem(&a);
        assert_eq!(code, 0, "{label}: {err}");
        let doc: Value = serde_json::from_str(&out).unwrap_or_else(|e| panic!("{label}: {e}"));
        if line_base_of(&doc) != 1 {
            failures.push(format!("{label}: lineBase is {}", line_base_of(&doc)));
        }
        if label == "fingerprint inventory" {
            continue;
        }
        // Spot check: the document's first line number appears in the human form
        // with its file.
        let Some(n) = first_line(&doc) else {
            failures.push(format!("{label}: no line in the document"));
            continue;
        };
        let mut h = argv.clone();
        h.extend(["--format", "human"]);
        let (human, _, _) = run_alsem(&h);
        if !human.contains(&format!(":{n}")) {
            failures.push(format!(
                "{label}: the JSON's line {n} is not in the human form"
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// What the source line behind one evidence `sourceAnchor` must hold, from its
/// `syntaxKind` and the step's note: a loop's keyword, a call's method (`Modify on T`
/// -> `modify`, `calls Helper` -> `helper`), a routine's `procedure`/`trigger`.
/// Empty for a kind with no word to look for (counted as unverifiable).
fn evidence_tokens(kind: &str, note: &str) -> Vec<String> {
    let word = |s: &str| -> String {
        s.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .find(|w| !w.is_empty())
            .unwrap_or("")
            .to_lowercase()
    };
    match kind {
        "for_statement" | "while_statement" | "repeat_statement" | "foreach_statement" => {
            vec![kind.trim_end_matches("_statement").to_string()]
        }
        // `calls Helper`, `accesses Customer.Name`, `forwards customer (...) to X`,
        // `passes Customer to EnrichCustomer, ...`, else the method first:
        // `Modify on Customer`, `Commit`.
        "call_expression" | "member_expression" | "identifier" => {
            let next = |rest: &str| last_segment(rest.split(' ').next().unwrap_or(""));
            if let Some(rest) = note
                .strip_prefix("calls ")
                .or_else(|| note.strip_prefix("accesses "))
                .or_else(|| note.strip_prefix("forwards "))
            {
                vec![next(rest)]
            } else if let Some((_, callee)) = note
                .strip_prefix("passes ")
                .and_then(|rest| rest.split_once(" to "))
            {
                vec![word(callee)]
            } else {
                vec![word(note)]
            }
        }
        k if k.contains("procedure") => vec!["procedure".to_string()],
        k if k.contains("trigger") => vec!["trigger".to_string()],
        _ => vec![],
    }
}

/// Every evidence `sourceAnchor` in an analyze `--with-evidence` or `policy check`
/// JSON document (`findings` under `payload` or at the top), and the policy finding's
/// own `primaryLocation` (the same shape), as anchors for [`check_anchors`].
fn evidence_anchors(label: &str, doc: &Value) -> Vec<Anchor> {
    let findings = doc["payload"]["findings"]
        .as_array()
        .or_else(|| doc["findings"].as_array())
        .unwrap_or_else(|| panic!("{label}: no findings array: {doc}"));
    let mut out = Vec::new();
    let mut push = |a: &Value, note: &str| {
        let line = a["range"]["startLine"].as_u64().expect("startLine") as usize;
        let kind = a["syntaxKind"].as_str().unwrap_or("");
        out.push(Anchor {
            file: a["sourceUnitId"].as_str().unwrap_or("").to_string(),
            line,
            tokens: evidence_tokens(kind, note),
            printed: format!("[{label}] {kind} at line {line}: {note}"),
        });
    };
    for f in findings {
        if f["primaryLocation"]["range"].is_object() {
            push(&f["primaryLocation"], "");
        }
        for step in f["evidencePath"].as_array().into_iter().flatten() {
            push(&step["sourceAnchor"], step["note"].as_str().unwrap_or(""));
        }
    }
    out
}

/// Item T for the analyze surface (#368): every `sourceAnchor` that
/// `alsem analyze --with-evidence` and `alsem policy check` print opens to the line
/// it describes (a loop step to its loop keyword, a call step to its method, a
/// routine step to its `procedure`/`trigger` line), and both documents state
/// `lineBase: 1`. Runs on the cross-app fixture (`dep:` anchors included, via
/// `--scope all`), five corpus workspaces (every loop kind, call, member and
/// identifier steps) and a policy workspace.
///
/// Discrimination (2026-11-04): printing the 0-based `startLine` again in
/// `format_json::evidence_path_to_value` (`shown(..)` -> the raw row) fails this test
/// (143 of the 146 analyze anchors open one line early; the other 3 are adjacent
/// field accesses whose line above names the same field); doing it in
/// `format_policy::anchor_to_jv` fails it at the policy run (3 of 3); each passes
/// restored.
#[test]
fn evidence_anchors_open_to_what_they_describe() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let corpus = |w: &str| manifest.join("tests/r0-corpus").join(w);
    let fixture_ws = workspace();
    let dump = tempfile::tempdir().unwrap();
    dump_dependency_source(&fixture_ws, dump.path());
    let runs: Vec<(PathBuf, Vec<&str>)> = vec![
        (
            fixture_ws.clone(),
            vec!["analyze", "--with-evidence", "--scope", "all"],
        ),
        (
            corpus("ws-d1-multi-caller"),
            vec!["analyze", "--with-evidence"],
        ),
        (
            corpus("ws-d8-commit-in-tx"),
            vec!["analyze", "--with-evidence"],
        ),
        // Every loop kind (for, while, repeat, foreach).
        (
            corpus("ws-statement-tree"),
            vec!["analyze", "--with-evidence"],
        ),
        // Member and identifier steps (field accesses, a forwarded record).
        (corpus("ws-d3"), vec!["analyze", "--with-evidence"]),
        // Over a hundred repeat-loop and call steps.
        (
            corpus("ws-d5-modifyall"),
            vec!["analyze", "--with-evidence"],
        ),
        (
            manifest.join("tests/fixtures/cli-c-policy/ws-policy-api-isolated-storage"),
            vec!["policy", "check"],
        ),
    ];
    let mut all = Checked::default();
    let mut per_run = Vec::new();
    let mut failures = Vec::new();
    for (ws, argv) in &runs {
        let ws_s = ws.to_string_lossy().into_owned();
        let label = format!("{} {}", argv.join(" "), ws.file_name().unwrap().display());
        let mut a = argv.clone();
        a.extend([ws_s.as_str(), "--format", "json"]);
        let (out, err, code) = run_alsem(&a);
        assert_eq!(code, 0, "{label}: {err}");
        let doc: Value = serde_json::from_str(&out).unwrap_or_else(|e| panic!("{label}: {e}"));
        let base = if doc.get("payload").is_some() {
            &doc["payload"]["lineBase"]
        } else {
            &doc["lineBase"]
        };
        if base != 1 {
            failures.push(format!("{label}: lineBase is {base}"));
        }
        let anchors = evidence_anchors(&label, &doc);
        let c = check_anchors(&anchors, ws, dump.path());
        per_run.push(format!(
            "{label}: {} anchors, {} matched ({} dep), {} unverifiable",
            anchors.len(),
            c.matched,
            c.matched_dep,
            c.unverifiable
        ));
        all.matched += c.matched;
        all.matched_dep += c.matched_dep;
        all.unverifiable += c.unverifiable;
        all.missing_file += c.missing_file;
        failures.extend(c.failures);
    }
    eprintln!("{}", per_run.join("\n"));
    assert!(
        failures.is_empty(),
        "evidence that does not open to what it describes:\n{}\n\n{}",
        failures.join("\n"),
        per_run.join("\n")
    );
    assert_eq!(
        all.missing_file, 0,
        "every anchor's file exists: {per_run:?}"
    );
    // Precondition: the check looked at enough anchors, `dep:` ones included.
    assert!(
        all.matched >= 100 && all.matched_dep >= 1,
        "only {} anchors checked ({} in dependency code):\n{}",
        all.matched,
        all.matched_dep,
        per_run.join("\n")
    );
}

/// Item T, the digest summary through the MCP server (plan 1b: D1, D2, E3): the
/// renderer's summary rides with the file, at most 8 KiB, and says what decides
/// whether to open it: per root the effects by type, the physical tables
/// written (the dependency group's whole set), temporary tables, commits,
/// boundaries entered and the unresolved call sites as counts. The serialized
/// `tools/call` result stays under 40,000 characters.
///
/// Discrimination: the summary's content is pinned by `digest_cli`'s unit tests
/// (`digest_summary_*`, each with its break); this test pins the plumbing:
/// dropping the `WriteAction::Summary` in `DigestJob::serialize` (back to
/// `JobOutput::text`) fails it at "a summary" (2026-11-04); restored, it passes.
#[test]
fn digest_summary_through_mcp_on_the_fixture() {
    let mut s = crate::mcp_server::Server::ready(&workspace(), None);
    let result = s.call(
        1,
        "alsem_digest",
        serde_json::json!({"routines": [ISSUE_DOC], "deterministic": true}),
    );
    let text = result.to_string();
    let env = crate::mcp_server::envelope(&result);
    assert!(env["stdoutFile"].is_object(), "a file: {env}");
    let summary = &env["summary"];
    assert!(summary.is_object(), "a summary: {env}");
    assert!(
        summary.to_string().len() <= al_sem::engine::batch::output::SUMMARY_CAP_BYTES,
        "{summary}"
    );
    assert!(text.len() <= 40_000, "{} characters", text.len());
    let r = &summary["routines"][0];
    assert_eq!(
        r["routine"], "Codeunit Out Issue Document::IssueDoc",
        "{summary}"
    );
    // W2-2 (design §3.1, T-W2-13): the summary is the rollup's one
    // `RollupTotals` serialization, here against numbers counted by hand from
    // the fixture's AL.
    let t = &r["summary"];
    // W2-1 (design §3.2): the three classes. `Out Doc`'s `Modify` is on
    // IssueDoc's `var` record parameter, so it is caller-dependent, not a
    // table the routine may persist itself.
    assert_eq!(
        r["names"]["tablesOwnedByApp"],
        serde_json::json!(["Out Log"]),
        "{summary}"
    );
    assert_eq!(t["callerDependentTables"], 1, "{summary}");
    // Base Doc Line, twelve ledgers, Base Register, Base Price: the group's
    // whole set, not the ten its `detail` names.
    assert_eq!(t["mayWriteTables"]["byDependencies"], 15, "{summary}");
    assert_eq!(t["mayWriteTables"]["count"], 16, "{summary}");
    assert_eq!(t["commits"]["answer"], "may", "{summary}");
    let g = &t["gaps"];
    assert_eq!(
        (g["workspace"].as_u64(), g["dependency"].as_u64()),
        (Some(2), Some(1)),
        "{summary}"
    );
    assert!(!summary.to_string().contains("callsiteAnchor"), "{summary}");
    assert!(s.finish().success());
}

/// Item T, "one copy" (plan 1c, P3/E1/E6): in what the client reads
/// (`structuredContent`: measured through Claude Code, see `server.rs`'s
/// `tool_result`) the stdout is there exactly once, as `stdoutText` for a
/// human answer and as the parsed `output` (no `stdoutText`) for a JSON one;
/// the first text block has no `stdoutText` placeholder; `prove` answers human
/// by default.
///
/// Discrimination (2026-11-04): putting the envelope with `stdoutText` back
/// under `output` in `tool_result` fails at "query effects json: one copy";
/// restoring the `"<the next content block>"` placeholder fails at "no
/// placeholder"; restored, it passes.
#[test]
fn one_copy_of_each_answer() {
    let mut s = crate::mcp_server::Server::ready(&workspace(), None);
    let calls = [
        (
            "explain human",
            "alsem_explain",
            serde_json::json!({"routines": [ISSUE_DOC]}),
        ),
        (
            "prove default",
            "alsem_prove",
            serde_json::json!({"routine": ISSUE_DOC, "question": "may-commit"}),
        ),
        (
            "query effects json",
            "alsem_query_effects",
            serde_json::json!({"routine": ISSUE_DOC_Q, "format": "json"}),
        ),
    ];
    let mut failures = Vec::new();
    for (i, (label, tool, args)) in calls.into_iter().enumerate() {
        let r = s.call(i as i64 + 1, tool, args);
        let content = r["content"].as_array().unwrap();
        let head: Value = serde_json::from_str(content[0]["text"].as_str().unwrap()).unwrap();
        if head.get("stdoutText").is_some() {
            failures.push(format!("{label}: no placeholder: {head}"));
        }
        let stdout = content[1]["text"].as_str().unwrap();
        let sc = &r["structuredContent"];
        let as_text = sc.get("stdoutText").and_then(Value::as_str) == Some(stdout);
        let as_output = sc
            .get("output")
            .is_some_and(|o| serde_json::from_str::<Value>(stdout).ok().as_ref() == Some(o));
        if as_text == as_output {
            failures.push(format!(
                "{label}: one copy (stdoutText {as_text}, output {as_output})"
            ));
        }
        if label == "prove default" && !stdout.starts_with("--- ") {
            failures.push(format!("{label}: human by default: {stdout}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    assert!(s.finish().success());
}

/// Every boundary-group failure in one digest or prove JSON document (plan 1d's
/// invariants, item T rows 1 and 3): the boundary is owner-qualified (`::`);
/// `resourceCount` equals the length of `resources[]`, which a resource-less
/// type (COMMIT, UI, ERROR, HTTP, FILE, BACKGROUND) does not state at all;
/// `unresolvedResources` appears only on a `DB_*` group.
pub(crate) fn group_failures(label: &str, effects: &[Value]) -> Vec<String> {
    let mut f = Vec::new();
    for e in effects {
        let d = &e["detail"];
        let Some(boundary) = d["boundary"].as_str() else {
            continue;
        };
        let ty = e["type"].as_str().unwrap_or("");
        if !boundary.contains("::") {
            f.push(format!(
                "{label}: {ty} boundary {boundary:?} names no owner"
            ));
        }
        let has_resources = ty.starts_with("DB_") || ty == "EVENT_PUBLISH";
        match (has_resources, d.get("resourceCount")) {
            (true, Some(c)) => {
                // Digest 3.x (W2-2, O2) lists an event group's events by
                // publisher; every other list is `resources[]`.
                let n = match e["eventsByPublisher"].as_array() {
                    Some(by) => by
                        .iter()
                        .map(|p| p["events"].as_array().map_or(0, Vec::len))
                        .sum(),
                    None => e["resources"].as_array().map_or(0, Vec::len),
                };
                if c.as_str() != Some(n.to_string().as_str()) {
                    f.push(format!(
                        "{label}: {ty} resourceCount {c} != resources[] {n}"
                    ));
                }
            }
            (true, None) => f.push(format!("{label}: {ty} group states no resourceCount")),
            (false, _) => {
                if d.get("resourceCount").is_some() || d.get("resources").is_some() {
                    f.push(format!("{label}: {ty} has no resources to state: {d}"));
                }
            }
        }
        if d.get("unresolvedResources").is_some() && !ty.starts_with("DB_") {
            f.push(format!("{label}: unresolvedResources on {ty}"));
        }
    }
    f
}

/// Item T, boundary groups (plan 1d; catches B2, D4, P2): in the fixture's digest
/// and in prove's `writes-table` evidence, every group names its boundary
/// owner-qualified, lists every resource (`Base Ledger 12`, the eleventh of a
/// twelve-table group, is in `resources[]`), and claims no unresolved resource
/// on a commit.
///
/// Discrimination (2026-11-04): restoring `<App>.<Routine>` in
/// `boundary_terminal` fails at "names no owner"; cutting `resources[]` at ten in
/// `effect_to_value` fails at "resourceCount 12 != resources[] 10"; counting
/// resource-less facts again fails at "unresolvedResources on COMMIT"; each
/// passes restored.
#[test]
fn boundary_groups_name_their_owner_and_every_resource() {
    let ws = workspace().to_string_lossy().into_owned();
    let mut failures = Vec::new();
    let (out, _, _) = run_alsem(&["digest", &ws, "--routine", ISSUE_DOC, "--format", "json"]);
    let doc: Value = serde_json::from_str(&out).unwrap();
    let effects = doc["payload"]["entries"][0]["effects"]
        .as_array()
        .unwrap()
        .clone();
    assert!(
        effects
            .iter()
            .filter(|e| e["detail"]["boundary"].is_string())
            .count()
            >= 5,
        "precondition: the fixture's groups: {doc}"
    );
    failures.extend(group_failures("digest", &effects));
    let twelve = effects
        .iter()
        .find(|e| e["detail"]["resourceCount"] == "12")
        .expect("precondition: the twelve-table group");
    if !twelve["resources"]
        .as_array()
        .unwrap()
        .iter()
        .any(|r| r["display"] == "Base Ledger 12")
    {
        failures.push(format!("Base Ledger 12 not listed: {twelve}"));
    }
    let (out, _, _) = run_alsem(&[
        "prove",
        &ws,
        ISSUE_DOC,
        "writes-table:Base Ledger 12",
        "--format",
        "json",
    ]);
    let p: Value = serde_json::from_str(&out).unwrap();
    failures.extend(group_failures(
        "prove",
        p["payload"]["result"]["evidence"].as_array().unwrap(),
    ));
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// Item T, "every `summary` <= 8 KB" and "a summary on every spill" (plan 1e,
/// E2): an explain, query effects, query touches or prove answer that goes to a
/// file (forced here with `maxInlineBytes: 0`) carries the renderer's summary
/// (each card's header line and list lengths; effects by operation and temp
/// state; the touches headlines; the answer and obligations), at most 8 KiB,
/// inside a result under 40,000 characters; the same answer inline carries none.
///
/// Discrimination (2026-11-04): dropping the summary in `ExplainJob::serialize`
/// (back to `JobOutput::text`) fails at "explain: a summary"; restored, it passes.
#[test]
fn every_spilled_answer_carries_a_summary() {
    let mut s = crate::mcp_server::Server::ready(&workspace(), None);
    let calls = [
        (
            "explain",
            "alsem_explain",
            serde_json::json!({"routines": [ISSUE_DOC]}),
            "/cards/0/card",
        ),
        (
            "query effects",
            "alsem_query_effects",
            serde_json::json!({"routine": ISSUE_DOC_Q}),
            "/byOp/Insert",
        ),
        (
            "query touches",
            "alsem_query_touches",
            serde_json::json!({"table": "Out Log", "from": ISSUE_DOC_Q}),
            "/down",
        ),
        (
            "prove",
            "alsem_prove",
            serde_json::json!({"routine": ISSUE_DOC, "question": "may-commit"}),
            "/obligations",
        ),
    ];
    let mut failures = Vec::new();
    for (i, (label, tool, args, probe)) in calls.into_iter().enumerate() {
        let mut spilled = args.clone();
        spilled["maxInlineBytes"] = 0.into();
        let result = s.call(2 * i as i64 + 1, tool, spilled);
        let text = result.to_string();
        let env = crate::mcp_server::envelope(&result);
        let summary = &env["summary"];
        if !env["stdoutFile"].is_object() || !summary.is_object() {
            failures.push(format!("{label}: a summary beside the file: {env}"));
            continue;
        }
        if summary.to_string().len() > al_sem::engine::batch::output::SUMMARY_CAP_BYTES
            || summary.pointer(probe).is_none()
            || text.len() > 40_000
        {
            failures.push(format!("{label}: summary {summary} ({} chars)", text.len()));
        }
        let inline = crate::mcp_server::envelope(&s.call(2 * i as i64 + 2, tool, args));
        if inline.get("summary").is_some() {
            failures.push(format!("{label}: no summary inline"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    assert!(s.finish().success());
}

/// Item T, "descriptions" (plan 1f; catches S1, E5, E7): every command tool's
/// description and the server instructions say the lines are 1-based and give
/// a size; each command tool states its dependency depth; `alsem_refresh` no
/// longer claims there is no built revision. (`alsem_status` reports no source
/// lines and is exempt from the line-base sentence.)
///
/// Discrimination (2026-11-04): restoring `alsem_refresh`'s old sentence fails
/// at "alsem_refresh: stale"; dropping "Lines are 1-based." from
/// `alsem_query_touches` fails at "alsem_query_touches: line base"; both pass
/// restored.
#[test]
fn descriptions_state_line_base_size_and_depth() {
    let mut failures = Vec::new();
    let has_size = |d: &str| d.contains(" KB") || d.contains("megabytes");
    for t in al_sem::mcp::tools::tool_list() {
        let name = t["name"].as_str().unwrap();
        let d = t["description"].as_str().unwrap();
        if matches!(name, "alsem_status" | "alsem_refresh") {
            if d.contains("no built revision") {
                failures.push(format!("{name}: stale"));
            }
            continue;
        }
        if !d.contains("1-based") {
            failures.push(format!("{name}: line base"));
        }
        if !has_size(d) {
            failures.push(format!("{name}: size"));
        }
        if !d.contains("Dependency code:") {
            failures.push(format!("{name}: dependency depth"));
        }
    }
    let i = al_sem::mcp::INSTRUCTIONS;
    if !i.contains("1-based") || !has_size(i) {
        failures.push(format!("INSTRUCTIONS: {i}"));
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// W2-2, T-W2-8 and T-W2-22 on the fixture: for caps 0, 1, 2, 3 and 7 every
/// own row's `paths` holds at most the effective cap (§5: N, at most 3) and
/// `pathsOmitted == pathsKept - len(paths)`; every hop's `to` and every
/// crossing target is a key of `payload.routines`; `--schema 2` with
/// `--detail`, `--max-paths` or `--compact` exits 2; `--compact` is one line.
///
/// Discrimination (2026-11-04): writing a hop's `to` as the stable id
/// instead of its routine key fails at "is no routine key"; collecting the
/// routines of every listed path rather than the shown ones fails at
/// "names exactly the document's routines" (cap 0); each restored, it
/// passes. Limit: the fixture keeps at most 3 paths per row, so the cap
/// clamp at 3 is checked by `pathCap` only, not by an over-long list.
#[test]
fn digest_path_caps_keys_and_flags_on_the_fixture() {
    let ws = workspace().to_string_lossy().into_owned();
    for (cap, arg) in [(0, "0"), (1, "1"), (2, "2"), (3, "3"), (3, "7")] {
        let (out, err, code) = run_alsem(&[
            "digest",
            &ws,
            "--routine",
            ISSUE_DOC,
            "--format",
            "json",
            "--max-paths",
            arg,
        ]);
        assert_eq!(code, 0, "{err}");
        let d: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(d["payload"]["pathCap"], cap, "--max-paths {arg}");
        let keys = d["payload"]["routines"].as_object().unwrap();
        let entry = &d["payload"]["entries"][0];
        for e in entry["effects"].as_array().unwrap() {
            let shown = e["paths"].as_array().map_or(0, Vec::len);
            assert!(
                shown <= cap,
                "--max-paths {arg}: {shown} paths over cap {cap}: {e}"
            );
            if let Some(kept) = e["pathsKept"].as_u64() {
                assert_eq!(
                    e["pathsOmitted"].as_u64().unwrap(),
                    kept - shown as u64,
                    "{e}"
                );
            }
            for p in e["paths"].as_array().into_iter().flatten() {
                for h in p["hops"].as_array().unwrap() {
                    let to = h["to"].as_str().unwrap();
                    assert!(keys.contains_key(to), "hop to {to} is no routine key");
                }
            }
        }
        for c in entry["crossings"].as_object().unwrap().values() {
            for t in c["targets"].as_array().unwrap() {
                assert!(
                    keys.contains_key(t.as_str().unwrap()),
                    "crossing target {t}"
                );
            }
        }
        // T-W2-8: the table holds exactly the routines the document names
        // (every `to` and target anywhere, and the root).
        fn tos(v: &Value, out: &mut std::collections::BTreeSet<String>) {
            match v {
                Value::Object(m) => {
                    for (k, x) in m {
                        if (k == "to" || k == "targets") && k != "routines" {
                            match x {
                                Value::String(s) => {
                                    out.insert(s.clone());
                                }
                                Value::Array(a) => out
                                    .extend(a.iter().filter_map(|t| t.as_str().map(String::from))),
                                _ => {}
                            }
                        } else {
                            tos(x, out);
                        }
                    }
                }
                Value::Array(a) => a.iter().for_each(|x| tos(x, out)),
                _ => {}
            }
        }
        let mut named = std::collections::BTreeSet::new();
        tos(&d["payload"]["entries"], &mut named);
        let root = entry["routine"]["stableId"].as_str().unwrap();
        named.extend(
            keys.iter()
                .filter(|(_, v)| v["stableId"] == root)
                .map(|(k, _)| k.clone()),
        );
        let table: std::collections::BTreeSet<String> = keys.keys().cloned().collect();
        assert_eq!(
            named, table,
            "--max-paths {arg}: the routine table names exactly the document's routines"
        );
    }
    for extra in [&["--detail"][..], &["--max-paths", "1"], &["--compact"]] {
        let mut args = vec!["digest", &ws, "--routine", ISSUE_DOC, "--schema", "2"];
        args.extend_from_slice(extra);
        let (_, err, code) = run_alsem(&args);
        assert_eq!(code, 2, "--schema 2 {extra:?}: {err}");
    }
    let (out, _, code) = run_alsem(&["digest", &ws, "--routine", ISSUE_DOC, "--compact"]);
    assert_eq!(code, 0);
    assert_eq!(out.trim_end().lines().count(), 1, "compact is one line");
    let (out, _, _) = run_alsem(&["digest", &ws, "--routine", ISSUE_DOC, "--schema", "2"]);
    let v2: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v2["schemaVersion"], "2.2.0");
}

/// W2-2, T-W2-11 on the fixture: query effects' summary buckets are
/// exclusive and sum to `effectCount`; every `firstHop` is a routine whose
/// own `query effects` holds the row's operation on the row's table with the
/// row's op and temp state (the complete contribution), or is `null` with
/// `firstHopUnavailable`; a direct row has neither. Expected by hand: 6 rows
/// = 2 may-persist writes + 1 caller-dependent + 1 temporary + 2 on a table
/// not resolved.
///
/// Discrimination (2026-11-04): taking the first sorted direct callee
/// without the membership check (`FirstHops::of`) fails at "holds the
/// contribution"; restored, it passes.
#[test]
fn query_effects_buckets_and_first_hops_on_the_fixture() {
    let ws = workspace().to_string_lossy().into_owned();
    // `None`: not selectable (a dependency routine is not a query selector).
    let effects_of = |sel: &str| -> Option<Value> {
        let (out, _, code) = run_alsem(&[
            "query",
            "effects",
            &ws,
            "--routine",
            sel,
            "--format",
            "json",
        ]);
        (code == 0).then(|| serde_json::from_str::<Value>(&out).unwrap()["payload"].clone())
    };
    let p = effects_of(ISSUE_DOC_Q).expect("IssueDoc is selectable");
    let s = &p["summary"];
    let total: u64 = [
        "writes",
        "callerDependent",
        "locks",
        "validate",
        "reads",
        "unclassified",
        "temporary",
        "tableNotResolved",
    ]
    .iter()
    .map(|k| s[k]["operations"].as_u64().unwrap())
    .sum();
    assert_eq!(total, p["effectCount"].as_u64().unwrap(), "{s}");
    assert_eq!(
        (
            s["writes"]["operations"].as_u64(),
            s["callerDependent"]["operations"].as_u64(),
            s["temporary"]["operations"].as_u64(),
            s["tableNotResolved"]["operations"].as_u64()
        ),
        (Some(2), Some(1), Some(1), Some(2)),
        "{s}"
    );
    let mut checked = 0;
    for e in p["effects"].as_array().unwrap() {
        if e["via"] == "direct" {
            assert!(e.get("firstHop").is_none(), "direct row with a hop: {e}");
            continue;
        }
        let Some(hop) = e["firstHop"].as_object() else {
            assert!(e["firstHopUnavailable"].is_string(), "{e}");
            continue;
        };
        let Some(callee) = effects_of(hop["stableId"].as_str().unwrap()) else {
            continue;
        };
        let holds = callee["effects"].as_array().unwrap().iter().any(|c| {
            c["operationId"] == e["operationId"]
                && c["tableId"] == e["tableId"]
                && c["op"] == e["op"]
                && c["tempState"] == e["tempState"]
        });
        assert!(holds, "the first hop holds the contribution: {e}");
        checked += 1;
    }
    assert!(
        checked >= 3,
        "precondition: inherited rows with a hop ({checked})"
    );
}

/// One table write as both documents state it: (type, class, tempState, the
/// sorted resource ids it names).
fn write_keys(rows: &[Value], explain: bool) -> Vec<(String, String, String, String)> {
    let mut v: Vec<(String, String, String, String)> = rows
        .iter()
        .filter(|e| {
            let ty = if explain { &e["op"] } else { &e["type"] };
            matches!(ty.as_str(), Some("DB_INSERT" | "DB_MODIFY" | "DB_DELETE"))
        })
        .map(|e| {
            let ty = if explain { &e["op"] } else { &e["type"] };
            let temp = if explain {
                e["tempState"].as_str()
            } else {
                e["detail"]["tempState"].as_str()
            };
            let mut ids: Vec<&str> = match e["resources"].as_array() {
                Some(rs) => rs.iter().filter_map(|r| r["id"].as_str()).collect(),
                None => vec![if explain {
                    e["tableId"].as_str().unwrap_or("")
                } else {
                    e["detail"]["resourceId"].as_str().unwrap_or("")
                }],
            };
            ids.sort_unstable();
            (
                ty.as_str().unwrap_or("").to_string(),
                e["class"].as_str().unwrap_or("").to_string(),
                temp.unwrap_or("").to_string(),
                ids.join(","),
            )
        })
        .collect();
    v.sort();
    v
}

/// W2-2, T-W2-16 leg 1 and T-W2-6 on the fixture: explain `/3` and digest
/// 3.0.0 give every table write the same class (equal records), digest's
/// default and `--detail` documents hold the same multiset of rows, digest's
/// summary and explain's are the one `RollupTotals`, and every table either
/// lists as a may-persist or caller-dependent write is one prove answers
/// `writes-table:T` with `yes` for. Expected by hand from the fixture's AL:
/// 16 may-persist tables and `Out Doc` caller-dependent.
///
/// Discrimination (2026-11-04): explain classifying its own rows without
/// their joined temp state (`classify(&[])`) fails at "explain and digest
/// classes"; filtering temporary rows out of digest's default document
/// (`entry_v3`) fails (at the class records first, which also lack them);
/// each restored, it passes.
#[test]
fn explain_digest_and_prove_agree_on_the_fixture() {
    let ws = workspace().to_string_lossy().into_owned();
    let json = |args: &[&str]| -> Value {
        let (out, err, code) = run_alsem(args);
        assert_eq!(code, 0, "{args:?}: {err}");
        serde_json::from_str(&out).unwrap_or_else(|e| panic!("{args:?}: {e}"))
    };
    let explain = json(&["explain", &ws, "--routine", ISSUE_DOC, "--format", "json"]);
    let digest = json(&["digest", &ws, "--routine", ISSUE_DOC, "--format", "json"]);
    let detail = json(&[
        "digest",
        &ws,
        "--routine",
        ISSUE_DOC,
        "--format",
        "json",
        "--detail",
    ]);
    let card = &explain["cards"][0];
    let entry = &digest["payload"]["entries"][0];
    // Leg 1: equal class records.
    let e_writes = write_keys(card["effects"]["writes"].as_array().unwrap(), true);
    let d_writes = write_keys(entry["effects"].as_array().unwrap(), false);
    assert_eq!(e_writes, d_writes, "explain and digest classes");
    // The one summary, against the hand count.
    let summary = &card["summary"];
    for k in [
        "mayWriteTables",
        "callerDependentTables",
        "commits",
        "events",
        "gaps",
        "temporaryOnlyTables",
        "entryPoints",
    ] {
        assert_eq!(summary[k], entry["summary"][k], "summary {k}");
    }
    assert_eq!(summary["mayWriteTables"]["count"], 16, "{summary}");
    assert_eq!(summary["callerDependentTables"], 1, "{summary}");
    // T-W2-6: default and --detail hold the same multiset of rows.
    let rows = |d: &Value| {
        let mut v: Vec<String> = d["payload"]["entries"][0]["effects"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| {
                format!(
                    "{}|{}|{}|{}|{}",
                    e["factId"],
                    e["type"],
                    e["detail"]["tempState"],
                    e["detail"]["resourceId"],
                    e["crossing"]
                )
            })
            .collect();
        v.sort();
        v
    };
    assert_eq!(rows(&digest), rows(&detail), "default vs --detail rows");
    // Every listed may-persist or caller-dependent table: prove says yes.
    let mut tables: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for w in card["effects"]["writes"].as_array().unwrap() {
        if w["class"] == "temporary" || w["literalUnreachable"] == true {
            continue;
        }
        match w["resources"].as_array() {
            Some(rs) => tables.extend(
                rs.iter()
                    .filter_map(|r| r["display"].as_str().map(String::from)),
            ),
            None => tables.extend(
                w["table"]
                    .as_str()
                    .filter(|t| !t.is_empty())
                    .map(String::from),
            ),
        }
    }
    assert_eq!(tables.len(), 17, "16 may-persist + Out Doc: {tables:?}");
    for t in &tables {
        let p = json(&[
            "prove",
            &ws,
            ISSUE_DOC,
            &format!("writes-table:{t}"),
            "--format",
            "json",
        ]);
        assert_eq!(
            p["payload"]["result"]["answer"], "yes",
            "prove writes-table:{t}"
        );
    }
}

/// The fixture root that writes `Out Item` with every operation kind the
/// field rule tells apart (`Out Field Ops::RunAll`, W2-4).
pub(crate) const FIELD_OPS: &str = "Out Field Ops::RunAll";

/// W2-4, T-W2-4 (design §4.1 rule 2d): a write states fields by its ORIGINAL
/// operation kind. `Insert` / `Modify`: the fields its owning routine assigns
/// on the table (assignments and `Validate` alike), named per owner and worded
/// "not operation-specific"; `ModifyAll`: its own field argument; `Rename`:
/// the primary key; `Delete` / `DeleteAll` / `LockTable`: nothing, even when
/// the routine assigns fields on that table (`Purge` sets `Bin.Code` before its
/// `DeleteAll`). `Validate` is no digest write type, so it never names a write
/// kind; its field is in its routine's assignments (`Create`'s `Status`).
/// Expected by hand from `OutFieldOps.Codeunit.al`. Two routines write `Out
/// Item` (`Create`, `CloseAll`, `Relabel`): one clause each.
///
/// Discrimination (2026-11-04): `write_fields` attaching the owner's assigned
/// fields to every operation kind (the old rule) fails at "Out Bin's DeleteAll
/// states no field"; restored, passes.
#[test]
fn fields_follow_the_original_operation_kind() {
    let ws = workspace().to_string_lossy().into_owned();
    let (human, err, code) = run_alsem(&["explain", &ws, "--routine", FIELD_OPS]);
    assert_eq!(code, 0, "{err}");
    let (out, err, code) = run_alsem(&["explain", &ws, "--routine", FIELD_OPS, "--format", "json"]);
    assert_eq!(code, 0, "{err}");
    let doc: Value = serde_json::from_str(&out).unwrap();
    let writes = doc["cards"][0]["effects"]["writes"].as_array().unwrap();
    let row = |table: &str, kind: &str| -> &Value {
        writes
            .iter()
            .find(|w| {
                w["table"] == table && w["opKinds"].as_array().unwrap().iter().any(|k| k == kind)
            })
            .unwrap_or_else(|| panic!("no {kind} row on {table}: {writes:?}"))
    };
    let empty = |v: &Value| match v {
        Value::Object(m) => m.is_empty(),
        Value::Array(a) => a.is_empty(),
        _ => false,
    };
    let bin = row("Out Bin", "DeleteAll");
    assert!(
        empty(&bin["fields"]) && empty(&bin["fieldOps"]) && empty(&bin["renames"]),
        "Out Bin's DeleteAll states no field: {bin}"
    );
    let insert = row("Out Item", "Insert");
    assert_eq!(
        insert["fields"],
        serde_json::json!({"Codeunit Out Field Ops::Create": ["No.", "Qty", "Status"]}),
        "{insert}"
    );
    let modify_all = row("Out Item", "ModifyAll");
    assert!(empty(&modify_all["fields"]), "{modify_all}");
    assert_eq!(
        modify_all["fieldOps"],
        serde_json::json!([{"op": "ModifyAll", "field": "Status", "in": "Codeunit Out Field Ops::CloseAll"}])
    );
    let rename = row("Out Item", "Rename");
    assert!(
        empty(&rename["fields"]) && empty(&rename["fieldOps"]),
        "{rename}"
    );
    assert_eq!(
        rename["renames"],
        serde_json::json!(["Codeunit Out Field Ops::Relabel"])
    );
    assert!(
        !writes.iter().any(|w| w["opKinds"]
            .as_array()
            .unwrap()
            .iter()
            .any(|k| k == "Validate")),
        "Validate is no write kind"
    );
    // The human block: Out Bin has no fields line; Out Item one clause per
    // owner, in the stated wording.
    let block = |table: &str| -> Vec<&str> {
        let lines: Vec<&str> = human.lines().collect();
        let start = lines
            .iter()
            .position(|l| l.trim_start().starts_with(&format!("{table}: ")))
            .unwrap_or_else(|| panic!("no {table} block:\n{human}"));
        lines[start + 1..]
            .iter()
            .take_while(|l| l.starts_with("      "))
            .copied()
            .collect()
    };
    assert!(
        !block("Out Bin").iter().any(|l| l.contains("field")),
        "Out Bin's DeleteAll states no field:\n{human}"
    );
    let item = block("Out Item").join("\n");
    for want in [
        "fields Create assigns on this table (not operation-specific): No., Qty, Status",
        "ModifyAll sets: Status (in Codeunit Out Field Ops::CloseAll)",
        "renames (primary key) in Codeunit Out Field Ops::Relabel",
    ] {
        assert!(item.contains(want), "{want}:\n{human}");
    }
    // The rule on every fixture card: no field on a delete-only write.
    for routine in [ISSUE_DOC, WRITE_LOG, FIELD_OPS] {
        let (out, _, _) = run_alsem(&["explain", &ws, "--routine", routine, "--format", "json"]);
        let doc: Value = serde_json::from_str(&out).unwrap();
        for w in doc["cards"][0]["effects"]["writes"].as_array().unwrap() {
            let Some(kinds) = w["opKinds"].as_array() else {
                continue;
            };
            if !kinds.is_empty()
                && kinds
                    .iter()
                    .all(|k| matches!(k.as_str(), Some("Delete" | "DeleteAll" | "LockTable")))
            {
                assert!(
                    empty(&w["fields"]) && empty(&w["fieldOps"]),
                    "{routine}: {w}"
                );
            }
        }
    }
}

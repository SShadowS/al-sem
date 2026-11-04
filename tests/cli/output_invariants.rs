//! Output invariants (MCP output plan `docs/superpowers/plans/2026-11-04-mcp-output-fixes.md`,
//! item T): properties of what explain, prove, digest and query print, checked on the
//! cross-app fixture `tests/fixtures/mcp-output/` (see `output_fixture`). A golden pins
//! what the engine prints; these pin that it is GOOD, so a blank name or an off-by-one
//! line fails a test even when someone regenerates the goldens.
//!
//! Each invariant lands with the plan item that makes it true. Today:
//! - 1a: [`anchor_source_check_on_the_fixture`] (every `file:line` a human output prints
//!   opens to the line it describes) and [`every_document_states_line_base_1`].
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
    let seg = head
        .rsplit('.')
        .next()
        .unwrap_or(head)
        .trim()
        .to_lowercase();
    match seg.strip_prefix("on") {
        Some(rest) if !rest.is_empty() && display.contains('.') => rest.to_string(),
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
    // query: `[Op] on T via ...` / `+1 Caller [Op] via ...`.
    if let Some(op) = between(t, "[", "]")
        && op.chars().all(|c| c.is_ascii_alphabetic())
        && op.chars().next().is_some_and(|c| c.is_ascii_uppercase())
        && !op.starts_with("DB_")
        && op != "COMMIT"
        && (t.contains(" via ") || t.starts_with('+'))
    {
        v.push(op.to_lowercase());
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
            vec!["query", "effects", &ws, "--routine", ISSUE_DOC_Q],
        ),
        (
            "query touches",
            vec![
                "query",
                "touches",
                &ws,
                "--table",
                "Out Log",
                "--from",
                ISSUE_DOC_Q,
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

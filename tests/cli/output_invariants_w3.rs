//! Output invariants of the MCP output plan's wave 3 (plan
//! `docs/superpowers/plans/2026-11-04-mcp-output-fixes.md`, items 3b and T): the tool
//! descriptions say only what the tools take, one selector grammar holds for every
//! tool, and item T's remaining rows (spec §8.3). They run on the cross-app fixture
//! `tests/fixtures/mcp-output/` in every `cargo test`; their CDO twins live in
//! `output_cdo`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde_json::{Value, json};

use crate::output_fixture::{ISSUE_DOC, ISSUE_DOC_Q, WRITE_LOG, run_alsem, workspace};
use crate::output_invariants::FIELD_OPS;

/// The properties every command tool takes, described once in the server
/// instructions, and `deterministic` (byte-stable output for tests).
const COMMON: &[&str] = &[
    "workspace",
    "freshness",
    "singleApp",
    "out",
    "maxInlineBytes",
    "deterministic",
];

/// The text between each pair of backticks.
fn backticked(s: &str) -> Vec<&str> {
    s.split('`').skip(1).step_by(2).collect()
}

/// `name` when `tok` is a camelCase identifier, or one followed by `: value`
/// (`detail: true`, `maxPaths: 0..3`): how a description names an input.
/// The second item says whether a value followed.
fn named(tok: &str) -> Option<(&str, bool)> {
    let (name, valued) = match tok.split_once(": ") {
        Some((n, _)) => (n, true),
        None => (tok, false),
    };
    let ident = name.chars().next().is_some_and(|c| c.is_ascii_lowercase())
        && name.chars().all(|c| c.is_ascii_alphanumeric());
    ident.then_some((name, valued))
}

/// MCP output plan 3b: every tool description, and the server instructions,
/// name only inputs the tool's schema has (`name: value` in backticks is an
/// input), and every input of a tool's schema is named in its description
/// (the common ones in the instructions, once). Each command tool's
/// description says what the default prints, how to ask for more, the line
/// base, a size, its dependency depth and a next step; the instructions say
/// which tool answers which question, the selector grammar and what the
/// answer words mean.
///
/// Discrimination (2026-11-05): renaming `maxPaths: 0..3` to `maxPath:
/// 0..3` in explain's description fails at "names `maxPath`"; restored,
/// passes.
#[test]
fn descriptions_name_exactly_the_inputs_the_schemas_take() {
    let mut failures = Vec::new();
    let mut every_input = BTreeSet::new();
    for t in al_sem::mcp::tools::tool_list() {
        let name = t["name"].as_str().unwrap();
        let d = t["description"].as_str().unwrap();
        let props: BTreeSet<&str> = t["inputSchema"]["properties"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        every_input.extend(props.iter().map(|p| p.to_string()));
        let mentioned: BTreeSet<&str> = backticked(d)
            .into_iter()
            .filter_map(named)
            .map(|(n, valued)| {
                if valued && !props.contains(n) {
                    failures.push(format!("{name}: names `{n}`, not in its schema"));
                }
                n
            })
            .collect();
        for p in &props {
            if !COMMON.contains(p) && !mentioned.contains(p) {
                failures.push(format!("{name}: input `{p}` is not named"));
            }
        }
        if matches!(name, "alsem_status" | "alsem_refresh") {
            continue;
        }
        for want in [
            "Default (`format: ",
            "Ask for more",
            "1-based",
            "Dependency code:",
            "Next:",
            " KB",
        ] {
            if !d.contains(want) {
                failures.push(format!("{name}: says nothing of {want:?}"));
            }
        }
    }
    let i = al_sem::mcp::INSTRUCTIONS;
    for (n, valued) in backticked(i).into_iter().filter_map(named) {
        if valued && !every_input.contains(n) {
            failures.push(format!("INSTRUCTIONS: names `{n}`, which no tool takes"));
        }
    }
    let mentioned: BTreeSet<&str> = backticked(i)
        .into_iter()
        .filter_map(named)
        .map(|(n, _)| n)
        .collect();
    for c in COMMON.iter().filter(|c| **c != "deterministic") {
        if !mentioned.contains(c) {
            failures.push(format!("INSTRUCTIONS: the common input `{c}` is not named"));
        }
    }
    for want in al_sem::mcp::tools::TOOL_NAMES.iter().copied().chain([
        "1-based",
        "`Owner::Routine`",
        "`Owner.Routine`",
        "`unknown`",
        "`coverage`",
        "`assumptions`",
        "Temporary records are never counted as writes",
        "summary",
    ]) {
        if !i.contains(want) {
            failures.push(format!("INSTRUCTIONS: says nothing of {want:?}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// MCP output plan 3b: one routine selector grammar for every tool. Each
/// spelling the instructions name (`Owner::Routine`, `Owner.Routine`, either
/// with the owner's type word or AL quotes, a unique bare name) selects the
/// same routine in explain, prove, digest, query effects and query touches
/// (`--from`): the answer is byte-identical to the one for the
/// `Codeunit Owner::Routine` spelling.
///
/// Discrimination (2026-11-05): dropping form 6 (`Object.Routine`) from
/// `selector_index::cascade` fails at "explain: `Out Issue Document.IssueDoc`";
/// dropping the `split_qualified` tier from query's
/// `resolve_routine_selector` fails at "query effects: `Out Issue
/// Document::IssueDoc`"; each restored, passes.
#[test]
fn one_selector_grammar_for_every_tool() {
    let ws = workspace().to_string_lossy().into_owned();
    let spellings = [
        "Codeunit Out Issue Document::IssueDoc",
        ISSUE_DOC,
        "Out Issue Document.IssueDoc",
        "Codeunit Out Issue Document.IssueDoc",
        "Codeunit \"Out Issue Document\"::IssueDoc",
        "\"Out Issue Document\".IssueDoc",
        "IssueDoc",
    ];
    type Argv = fn(&str, &str) -> Vec<String>;
    let tools: [(&str, Argv); 5] = [
        ("explain", |ws, s| {
            vec!["explain".into(), ws.into(), "--routine".into(), s.into()]
        }),
        ("prove", |ws, s| {
            vec![
                "prove".into(),
                ws.into(),
                s.into(),
                "may-commit".into(),
                "--format".into(),
                "human".into(),
            ]
        }),
        ("digest", |ws, s| {
            vec![
                "digest".into(),
                ws.into(),
                "--routine".into(),
                s.into(),
                "--format".into(),
                "human".into(),
            ]
        }),
        ("query effects", |ws, s| {
            vec![
                "query".into(),
                "effects".into(),
                ws.into(),
                "--routine".into(),
                s.into(),
            ]
        }),
        ("query touches", |ws, s| {
            vec![
                "query".into(),
                "touches".into(),
                ws.into(),
                "--table".into(),
                "Out Log".into(),
                "--from".into(),
                s.into(),
            ]
        }),
    ];
    let mut failures = Vec::new();
    for (tool, argv) in tools {
        let mut expected = None;
        for s in spellings {
            let a = argv(&ws, s);
            let a: Vec<&str> = a.iter().map(String::as_str).collect();
            let (out, err, code) = run_alsem(&a);
            if code != 0 {
                failures.push(format!("{tool}: `{s}` exits {code}: {err}"));
                continue;
            }
            match &expected {
                None => expected = Some(out),
                Some(e) if *e != out => {
                    failures.push(format!("{tool}: `{s}` answers otherwise:\n{out}"));
                }
                Some(_) => {}
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

// ---------------------------------------------------------------------------
// Item T (spec §8.3): the rows not pinned before wave 3.
// ---------------------------------------------------------------------------

/// The longest line any human output may print (T-W2-15).
pub(crate) const MAX_LINE_CHARS: usize = 2_000;

/// A human output's size budget: whole-output lines and bytes (T-W2-15).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Budget {
    pub lines: usize,
    pub bytes: usize,
}

/// Every way one human output breaks the plan's hygiene rows (rows 1, 2
/// and 6 of item T: B1, B5, B6, and T-W2-12's `%20`): a blank name where a
/// table, event or boundary belongs, a `(no anchor)` row, an escaped space,
/// and two identical lines (`path:` lines aside: two writes may share a
/// path, design §8.3 T-W2-2).
pub(crate) fn hygiene_failures(label: &str, text: &str) -> Vec<String> {
    let mut f = Vec::new();
    let mut seen: BTreeMap<&str, usize> = BTreeMap::new();
    for line in text.lines() {
        let t = line.trim();
        for verb in ["writes", "raises", "reads", "extension point"] {
            if t.contains(&format!("{verb}  ("))
                || t.starts_with(&format!("{verb} ("))
                || t.contains(&format!("{verb} :"))
            {
                f.push(format!("{label}: a blank name after `{verb}`: {t}"));
            }
        }
        if t.contains("(no anchor)") {
            f.push(format!("{label}: a row with no position or owner: {t}"));
        }
        if t.contains("%20") {
            f.push(format!("{label}: an escaped space: {t}"));
        }
        let is_path = ["path:", "prefix path:", "representative path"]
            .iter()
            .any(|p| t.starts_with(p));
        if !t.is_empty() && !is_path {
            *seen.entry(line).or_default() += 1;
        }
    }
    for (l, n) in seen {
        if n > 1 {
            f.push(format!("{label}: {n} identical lines: {}", l.trim()));
        }
    }
    f
}

/// Every way one human output exceeds its budget (T-W2-15): whole lines,
/// whole bytes, and any line over [`MAX_LINE_CHARS`] characters.
pub(crate) fn size_failures(label: &str, text: &str, b: Option<Budget>) -> Vec<String> {
    let mut f = Vec::new();
    if let Some(b) = b {
        let lines = text.lines().count();
        if lines > b.lines {
            f.push(format!("{label}: {lines} lines > {}", b.lines));
        }
        if text.len() > b.bytes {
            f.push(format!("{label}: {} bytes > {}", text.len(), b.bytes));
        }
    }
    for (i, l) in text.lines().enumerate() {
        let n = l.chars().count();
        if n > MAX_LINE_CHARS {
            f.push(format!("{label}: line {} has {n} characters", i + 1));
        }
    }
    f
}

/// The fixture's budget per command (the plan's targets: explain ≤ 150
/// lines, query effects ≤ 100, touches ≤ 20; every byte budget is the 40 KiB
/// inline cap or less).
fn fixture_budget(label: &str) -> Option<Budget> {
    let cap = 40 * 1024;
    if label.contains("--detail") || label.contains("--all") {
        return None;
    }
    Some(match label.split_whitespace().next().unwrap_or("") {
        "explain" | "digest" => Budget {
            lines: 150,
            bytes: cap,
        },
        "prove" => Budget {
            lines: 20,
            bytes: 8 * 1024,
        },
        _ if label.starts_with("query effects") => Budget {
            lines: 100,
            bytes: cap,
        },
        _ => Budget {
            lines: 20,
            bytes: 8 * 1024,
        },
    })
}

/// Item T rows 1, 2 and 6 (B1, B5, B6) and T-W2-15 on the fixture: no
/// human output prints a blank name, a `(no anchor)` row, an escaped space
/// or two identical lines, and each stays within its budget (lines AND
/// bytes, whole output; every line ≤ 2,000 characters). Formatting checks:
/// no discrimination proof (CLAUDE.md "Scope of the proof"); the CDO twin
/// is `output_cdo::cdo_human_outputs_are_clean_and_within_their_ratchets`.
#[test]
fn human_outputs_are_clean_and_within_budget() {
    let ws = workspace().to_string_lossy().into_owned();
    let mut outputs = crate::output_invariants::fixture_human_outputs();
    let extra: Vec<Vec<&str>> = vec![
        vec!["explain", &ws, "--routine", ISSUE_DOC, "--detail"],
        vec!["explain", &ws, "--routine", FIELD_OPS],
        vec![
            "digest",
            &ws,
            "--routine",
            ISSUE_DOC,
            "--format",
            "human",
            "--detail",
        ],
        vec!["query", "effects", &ws, "--routine", ISSUE_DOC_Q, "--all"],
        vec!["query", "effects", &ws, "--routine", "Out Field Ops.RunAll"],
    ];
    for a in extra {
        let label = a.join(" ").replace(&ws, "{ws}");
        let (out, err, code) = run_alsem(&a);
        assert_eq!(code, 0, "{label}: {err}");
        outputs.push((label, out));
    }
    let mut failures = Vec::new();
    for (label, text) in &outputs {
        failures.extend(hygiene_failures(label, text));
        failures.extend(size_failures(label, text, fixture_budget(label)));
    }
    assert!(
        outputs.len() >= 15,
        "precondition: {} outputs",
        outputs.len()
    );
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// The human label each non-DB effect type is counted under in explain's
/// card (`routine_rollup::FAMILIES`), or the card line that states it.
fn kind_line(ty: &str) -> Option<&'static str> {
    Some(match ty {
        t if t.starts_with("DB_") || t == "UI_ERROR" => return None,
        "COMMIT" => "commits: ",
        "EVENT_PUBLISH" => "events: ",
        t if t.starts_with("UI_") => "UI at ",
        "HTTP" => "HTTP at ",
        "FILE" => "file at ",
        "BACKGROUND_TASK" => "background tasks at ",
        "ISOLATED_STORAGE" => "isolated storage at ",
        "TELEMETRY" => "telemetry at ",
        "ERROR_THROW" | "ERROR_IMPLICIT" => "errors at ",
        _ => "other effect kinds at ",
    })
}

/// Item T row 8 (M2): every live effect type a digest entry reports (own
/// rows and the types beyond an entry point) is in explain's card: on its
/// family's line, which names the type unless the line is folded; a commit
/// on a `commits:` line that is not `none`, an event on a non-empty
/// `events:` line.
pub(crate) fn kind_failures(label: &str, effects: &[Value], card: &str) -> Vec<String> {
    let mut f = Vec::new();
    let types: BTreeSet<&str> = effects
        .iter()
        .filter(|e| e["status"] != "literal-dead")
        .filter_map(|e| e["type"].as_str())
        .collect();
    for ty in types {
        let Some(prefix) = kind_line(ty) else {
            continue;
        };
        let line = card.lines().map(str::trim).find(|l| l.starts_with(prefix));
        let ok = match (ty, line) {
            (_, None) => false,
            ("COMMIT", Some(l)) => !l.starts_with("commits: none"),
            ("EVENT_PUBLISH", Some(l)) => !l.starts_with("events: none"),
            (_, Some(l)) => l.contains(ty) || l.contains("… +"),
        };
        if !ok {
            f.push(format!("{label}: {ty} is not in the card (line {line:?})"));
        }
    }
    f
}

/// Item T row 8 on the fixture (M2: HTTP, background tasks and errors were
/// missing from the card): for IssueDoc, WriteLog and RunAll, every effect
/// type digest reports is on its line of explain's card. Precondition: the
/// fixture reaches HTTP, BACKGROUND_TASK, ERROR_IMPLICIT, COMMIT and
/// EVENT_PUBLISH.
///
/// Discrimination (2026-11-05): `routine_rollup::family_of` without its
/// `BACKGROUND_TASK` arm (the type falls to "other effect kinds") fails at
/// "BACKGROUND_TASK is not in the card"; restored, passes.
#[test]
fn every_effect_kind_digest_reports_is_in_the_card() {
    let ws = workspace().to_string_lossy().into_owned();
    let mut failures = Vec::new();
    let mut seen = BTreeSet::new();
    for routine in [ISSUE_DOC, WRITE_LOG, FIELD_OPS] {
        let (d, err, code) = run_alsem(&["digest", &ws, "--routine", routine]);
        assert_eq!(code, 0, "{err}");
        let d: Value = serde_json::from_str(&d).unwrap();
        let effects = d["payload"]["entries"][0]["effects"].as_array().unwrap();
        seen.extend(
            effects
                .iter()
                .filter_map(|e| e["type"].as_str().map(str::to_string)),
        );
        let (card, err, code) = run_alsem(&["explain", &ws, "--routine", routine]);
        assert_eq!(code, 0, "{err}");
        failures.extend(kind_failures(routine, effects, &card));
    }
    for want in [
        "HTTP",
        "BACKGROUND_TASK",
        "ERROR_IMPLICIT",
        "COMMIT",
        "EVENT_PUBLISH",
    ] {
        assert!(
            seen.contains(want),
            "precondition: the fixture reaches {want}"
        );
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// T-W2-17: every evidence row of a prove JSON document names its place
/// (`evidence.file` with a 1-based `line`) or says `sourceKind:
/// unavailable`.
pub(crate) fn evidence_failures(label: &str, doc: &Value) -> Vec<String> {
    let mut f = Vec::new();
    for (i, e) in doc["payload"]["result"]["evidence"]
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
    {
        let ev = &e["evidence"];
        let placed = ev["file"].is_string() && ev["line"].as_u64().is_some_and(|l| l >= 1);
        if !placed && ev["sourceKind"] != "unavailable" {
            f.push(format!("{label}: evidence {i} has no place: {e}"));
        }
    }
    f
}

/// T-W2-17 on the fixture (F8: a group row in prove once carried no
/// evidence at all): for IssueDoc and WriteLog, every evidence row of
/// may-commit, reaches-ui, throws-error and `writes-table:T` for every
/// table of both apps names its file and line or says `unavailable`.
/// Precondition: group rows (beyond an entry point) are among them.
///
/// Discrimination (2026-11-05): prove's `effect_to_value` writing `evidence:
/// null` on group rows fails at "has no place"; restored, passes.
#[test]
fn prove_evidence_names_its_place_or_says_unavailable() {
    let ws = workspace().to_string_lossy().into_owned();
    let mut questions: Vec<String> = ["may-commit", "reaches-ui", "throws-error"]
        .iter()
        .map(|q| q.to_string())
        .collect();
    questions.extend(
        crate::output_fixture::TABLES
            .iter()
            .map(|t| format!("writes-table:{t}")),
    );
    let (mut failures, mut rows, mut groups) = (Vec::new(), 0, 0);
    for routine in [ISSUE_DOC, WRITE_LOG] {
        for q in &questions {
            let (out, err, code) = run_alsem(&["prove", &ws, routine, q, "--format", "json"]);
            assert_eq!(code, 0, "{routine} {q}: {err}");
            let doc: Value = serde_json::from_str(&out).unwrap();
            let ev = doc["payload"]["result"]["evidence"].as_array();
            rows += ev.map_or(0, Vec::len);
            groups += ev
                .into_iter()
                .flatten()
                .filter(|e| e["crossing"].is_string())
                .count();
            failures.extend(evidence_failures(&format!("{routine} {q}"), &doc));
        }
    }
    assert!(
        rows >= 10 && groups >= 3,
        "precondition: {rows} rows, {groups} groups"
    );
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// T-W2-18 (F2: a boundary named another target): in a digest 3.x document
/// every crossing's targets are routines of `payload.routines`, its
/// `boundary` is its one target's qualified display or names every target,
/// and every row's `crossing` is one of the entry's crossings.
pub(crate) fn crossing_failures(label: &str, doc: &Value) -> Vec<String> {
    let mut f = Vec::new();
    let routines = &doc["payload"]["routines"];
    for entry in doc["payload"]["entries"].as_array().into_iter().flatten() {
        let crossings = entry["crossings"].as_object().cloned().unwrap_or_default();
        for (key, c) in &crossings {
            let boundary = c["boundary"].as_str().unwrap_or("");
            let targets: Vec<&str> = c["targets"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|t| routines[t.as_str().unwrap_or("")]["display"].as_str())
                .collect();
            let listed = c["targets"].as_array().map_or(0, Vec::len);
            if targets.len() != listed || listed == 0 {
                f.push(format!(
                    "{label}: {key}'s targets are not all routines: {c}"
                ));
            } else if listed == 1 && boundary != targets[0] {
                f.push(format!(
                    "{label}: {key} prints {boundary:?}, its target is {:?}",
                    targets[0]
                ));
            } else if listed > 1 && !targets.iter().all(|t| boundary.contains(t)) {
                f.push(format!(
                    "{label}: {key} prints {boundary:?}, not every target of {targets:?}"
                ));
            }
        }
        for e in entry["effects"].as_array().into_iter().flatten() {
            if let Some(k) = e["crossing"].as_str()
                && !crossings.contains_key(k)
            {
                f.push(format!("{label}: a row's crossing {k} is not listed"));
            }
        }
    }
    f
}

/// T-W2-18 on the fixture: IssueDoc's two crossings (`Base Poster::OnRun`,
/// `Base Calc::OnRun`, one routine name in two codeunits) print their own
/// target's qualified display.
///
/// Discrimination (2026-11-05): `digest::boundary_display` printing the bare
/// routine (`OnRun`) fails at "prints \"OnRun\""; restored, passes.
#[test]
fn crossings_print_their_targets() {
    let ws = workspace().to_string_lossy().into_owned();
    let (out, err, code) = run_alsem(&["digest", &ws, "--routine", ISSUE_DOC]);
    assert_eq!(code, 0, "{err}");
    let doc: Value = serde_json::from_str(&out).unwrap();
    let n = doc["payload"]["entries"][0]["crossings"]
        .as_object()
        .map_or(0, |m| m.len());
    assert_eq!(n, 2, "precondition: two crossings");
    let f = crossing_failures("IssueDoc", &doc);
    assert!(f.is_empty(), "{}", f.join("\n"));
}

/// T-W2-7 for explain `/3`: a table group lists every resource it counts
/// (`resourceCount == len(resources)`), an event group carries its count with
/// `resourcesListed: false` by default and every event with `--detail`, and a
/// resource-less group carries neither.
fn explain_group_failures(label: &str, doc: &Value, detail: bool) -> Vec<String> {
    let mut f = Vec::new();
    for card in doc["cards"].as_array().into_iter().flatten() {
        for (family, rows) in card["effects"].as_object().into_iter().flatten() {
            for r in rows.as_array().into_iter().flatten() {
                if !r["crossing"].is_string() {
                    continue;
                }
                let count = r["resourceCount"].as_u64();
                match family.as_str() {
                    "writes" => {
                        let n = r["resources"].as_array().map_or(0, Vec::len) as u64;
                        if count != Some(n) {
                            f.push(format!(
                                "{label}: a write group counts {count:?}, lists {n}"
                            ));
                        }
                    }
                    "events" => {
                        let listed = r["resourcesListed"].as_bool();
                        let n = r["events"].as_array().map(|v| v.len() as u64);
                        let ok = match detail {
                            false => listed == Some(false) && n.is_none() && count.is_some(),
                            true => listed == Some(true) && n == count,
                        };
                        if !ok {
                            f.push(format!("{label}: an event group: {r}"));
                        }
                    }
                    "crossings" => {}
                    _ => {
                        if r.get("resourceCount").is_some() || r.get("resources").is_some() {
                            f.push(format!("{label}: a {family} group states resources: {r}"));
                        }
                    }
                }
            }
        }
    }
    f
}

/// T-W2-7 on the fixture: IssueDoc's twelve-table group lists all twelve in
/// explain's JSON, its event group is a count by default and every event with
/// `--detail`, and its commit, error and HTTP-free groups state no
/// resources.
///
/// Discrimination (2026-11-05): explain's write-group `resources` cut at ten
/// (`.take(10)`) fails at "counts Some(12), lists 10"; restored, passes.
#[test]
fn explain_groups_list_what_they_count() {
    let ws = workspace().to_string_lossy().into_owned();
    let mut failures = Vec::new();
    for detail in [false, true] {
        let mut a = vec![
            "explain",
            ws.as_str(),
            "--routine",
            ISSUE_DOC,
            "--format",
            "json",
        ];
        if detail {
            a.push("--detail");
        }
        let (out, err, code) = run_alsem(&a);
        assert_eq!(code, 0, "{err}");
        let doc: Value = serde_json::from_str(&out).unwrap();
        let writes = doc["cards"][0]["effects"]["writes"].as_array().unwrap();
        assert!(
            writes.iter().any(|w| w["resourceCount"] == 12),
            "precondition: the twelve-table group"
        );
        failures.extend(explain_group_failures(
            &format!("detail {detail}"),
            &doc,
            detail,
        ));
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// T-W2-21: the fields the ConTinAI pipeline reads (`effects.py` from query
/// effects, `verify.py` from prove; spec §8.3) are present with their types,
/// on the fixture: query effects rows (`via` `direct` exactly when the
/// operation is the queried routine's own, `operationId`,
/// `origin.routine.{stableId, objectDisplay, display}`, `origin.anchor.{file,
/// line}` with a dependency anchor among them, `tableName`, `tableId`,
/// `isUnknownBucket`, `tempState`, `op`); prove (`payload.lineBase`,
/// `result.answer`, the first evidence's `evidence.{file, line}`,
/// `result.blockedBy` on an `unknown`, `result.assumptions` with
/// `--closed-world`, an answer with no evidence); digest
/// `entries[].routine.anchor`.
///
/// Discrimination (2026-11-05): `effect_query_cli::touch_json` writing the
/// key `via` as `viaKind` fails at "row 0: via"; restored, passes.
#[test]
fn consumer_fields_are_present() {
    let ws = workspace().to_string_lossy().into_owned();
    let json_of = |a: &[&str]| -> Value {
        let (out, err, code) = run_alsem(a);
        assert_eq!(code, 0, "{a:?}: {err}");
        serde_json::from_str(&out).unwrap()
    };
    let mut f = Vec::new();
    let q = json_of(&[
        "query",
        "effects",
        &ws,
        "--routine",
        ISSUE_DOC_Q,
        "--format",
        "json",
    ]);
    let me = q["payload"]["routine"]["routine"]["stableId"]
        .as_str()
        .unwrap_or("?")
        .to_string();
    let rows = q["payload"]["effects"].as_array().unwrap();
    let mut dep_anchor = false;
    for (i, r) in rows.iter().enumerate() {
        let s = |p: &str| r.pointer(p).and_then(Value::as_str).is_some();
        for p in [
            "/op",
            "/operationId",
            "/tempState",
            "/tableId",
            "/origin/routine/stableId",
            "/origin/routine/objectDisplay",
            "/origin/routine/display",
            "/origin/anchor/file",
        ] {
            if !s(p) {
                f.push(format!("query row {i}: {p}"));
            }
        }
        if !(r["tableName"].is_string() || r["tableName"].is_null())
            || !r["isUnknownBucket"].is_boolean()
        {
            f.push(format!("query row {i}: tableName / isUnknownBucket"));
        }
        if r.pointer("/origin/anchor/line")
            .and_then(Value::as_u64)
            .is_none_or(|l| l < 1)
        {
            f.push(format!("query row {i}: origin.anchor.line"));
        }
        let own = r["origin"]["routine"]["stableId"] == me.as_str();
        match r["via"].as_str() {
            Some("direct") if own => {}
            Some("inherited") if !own => {}
            other => f.push(format!("row {i}: via {other:?} (own operation: {own})")),
        }
        dep_anchor |= r["origin"]["anchor"]["file"]
            .as_str()
            .is_some_and(|a| a.starts_with(&format!("dep:{}:", crate::output_fixture::DEP_GUID)));
    }
    assert!(
        dep_anchor,
        "precondition: a dependency anchor among the rows"
    );
    let prove_of = |routine: &str, t: &str, extra: &[&str]| -> Value {
        let mut a = vec!["prove", ws.as_str(), routine, t, "--format", "json"];
        a.extend_from_slice(extra);
        json_of(&a)
    };
    let prove = |t: &str, extra: &[&str]| prove_of(ISSUE_DOC, t, extra);
    let yes = prove("writes-table:Out Log", &[]);
    if yes["payload"]["lineBase"] != 1 || yes["payload"]["result"]["answer"] != "yes" {
        f.push(format!("prove: lineBase / answer: {yes}"));
    }
    let ev = &yes["payload"]["result"]["evidence"][0]["evidence"];
    if !ev["file"].is_string() || !ev["line"].is_u64() {
        f.push(format!("prove: evidence[0].evidence.{{file, line}}: {ev}"));
    }
    let unknown = prove("writes-table:Out Item", &[]);
    let r = &unknown["payload"]["result"];
    if r["answer"] != "unknown"
        || !r["blockedBy"].is_array()
        || r["evidence"].as_array().is_some_and(|e| !e.is_empty())
    {
        f.push(format!(
            "prove unknown: answer, blockedBy, no evidence: {r}"
        ));
    }
    // An assumption-bearing answer: WriteLog never writes `Out Doc`, a `no`
    // under the loaded world (and, asked closed, under that one too).
    let closed = prove_of(WRITE_LOG, "writes-table:Out Doc", &["--closed-world"]);
    if closed["payload"]["result"]["answer"] != "no"
        || closed["payload"]["result"]["assumptions"]
            .as_array()
            .is_none_or(Vec::is_empty)
    {
        f.push(format!(
            "prove --closed-world: assumptions: {}",
            closed["payload"]["result"]
        ));
    }
    let d = json_of(&["digest", &ws, "--routine", ISSUE_DOC]);
    let a = &d["payload"]["entries"][0]["routine"]["anchor"];
    if !a["file"].is_string() || a["line"].as_u64().is_none_or(|l| l < 1) {
        f.push(format!("digest: entries[0].routine.anchor: {a}"));
    }
    assert!(f.is_empty(), "{}", f.join("\n"));
}

/// Write a single-app workspace under `dir` holding `src` as one file.
fn write_ws(dir: &Path, src: &str) {
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(
        dir.join("app.json"),
        r#"{"id": "77777777-0000-4000-8000-0000000000c3", "name": "Item T App", "publisher": "T", "version": "1.0.0.0"}"#,
    )
    .unwrap();
    std::fs::write(dir.join("src/T.al"), src).unwrap();
}

/// A routine reached from `actions` page actions (on two pages), and with
/// `rules` `Error` rules whose messages are `msg_len` characters of quotes
/// and backslashes: every character escapes to two in JSON.
fn item_t_source(actions: usize, rules: usize, msg_len: usize) -> String {
    let msg: String = "\"\\".repeat(msg_len / 2);
    let mut s = String::from(
        "table 50300 \"T Doc\"\n{\n    fields\n    {\n        field(1; \"No.\"; Integer) { }\n    }\n    keys\n    {\n        key(PK; \"No.\") { }\n    }\n}\n\ncodeunit 50300 \"T Mgt\"\n{\n    procedure Check(var Doc: Record \"T Doc\")\n    begin\n",
    );
    for i in 0..rules {
        s.push_str(&format!(
            "        if Doc.\"No.\" = {i} then\n            Error('{i} {msg}');\n"
        ));
    }
    s.push_str("        Doc.Modify();\n    end;\n}\n");
    for (page, range) in [(50300, 0..actions / 2), (50301, actions / 2..actions)] {
        s.push_str(&format!(
            "\npage {page} \"T Card {page}\"\n{{\n    PageType = Card;\n    SourceTable = \"T Doc\";\n\n    actions\n    {{\n        area(Processing)\n        {{\n"
        ));
        for a in range {
            s.push_str(&format!(
                "            action(A{a})\n            {{\n                Caption = 'Check {}';\n\n                trigger OnAction()\n                var\n                    M: Codeunit \"T Mgt\";\n                begin\n                    M.Check(Rec);\n                end;\n            }}\n",
                a % 3
            ));
        }
        s.push_str("        }\n    }\n}\n");
    }
    s
}

/// T-W2-14 (F16g): with more than 3 page actions the page-action part of
/// explain's `callers:` takes at most 3 lines (the `reached from N page
/// actions` count, `by caption`, `most actions`), and the grouped form
/// counts the same actions `--detail` lists one per line: the captions'
/// and the page objects' counts each sum to the actions, and the JSON
/// `reachedFrom` names the same action ids in both forms. Hand-stated
/// workspace: 7 actions on two pages, 3 captions.
///
/// Discrimination (2026-11-05): explain's `actions <= 3 || detail` grouping
/// raised to `actions <= 30` (no grouping) fails at "page-action lines";
/// restored, passes.
#[test]
fn page_actions_fold_to_three_lines() {
    let dir = tempfile::tempdir().unwrap();
    write_ws(dir.path(), &item_t_source(7, 1, 4));
    let ws = dir.path().to_string_lossy().into_owned();
    let human = |detail: bool| {
        let mut a = vec!["explain", ws.as_str(), "--routine", "T Mgt::Check"];
        if detail {
            a.push("--detail");
        }
        let (out, err, code) = run_alsem(&a);
        assert_eq!(code, 0, "{err}");
        out
    };
    let action_lines = |t: &str| -> Vec<String> {
        t.lines()
            .map(str::trim)
            .filter(|l| {
                l.starts_with("reached from page action")
                    || l.starts_with("by caption")
                    || l.starts_with("most actions")
            })
            .map(str::to_string)
            .collect()
    };
    let grouped = human(false);
    let listed = human(true);
    assert!(grouped.contains("reached from 7 page actions"), "{grouped}");
    let g = action_lines(&grouped);
    // The count rides on the `callers:` line; the rest is at most 2 lines.
    assert!(g.len() < 3, "page-action lines: {g:?}");
    assert_eq!(action_lines(&listed).len(), 7, "{listed}");
    let json = |detail: bool| -> Value {
        let mut a = vec![
            "explain",
            ws.as_str(),
            "--routine",
            "T Mgt::Check",
            "--format",
            "json",
        ];
        if detail {
            a.push("--detail");
        }
        let (out, err, code) = run_alsem(&a);
        assert_eq!(code, 0, "{err}");
        serde_json::from_str(&out).unwrap()
    };
    let ids = |v: &Value| -> BTreeSet<String> {
        v["cards"][0]["reachedFrom"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|r| format!("{}::{}", r["object"], r["action"]))
            .collect()
    };
    let (dj, lj) = (json(false), json(true));
    assert_eq!(ids(&dj), ids(&lj), "the same actions in both forms");
    assert_eq!(ids(&lj).len(), 7);
    let s = &dj["cards"][0]["reachedFromSummary"];
    let sum = |k: &str| -> u64 {
        s[k].as_array()
            .into_iter()
            .flatten()
            .filter_map(|x| x["count"].as_u64())
            .sum()
    };
    assert_eq!(
        (s["actions"].as_u64(), sum("byCaption"), sum("byObject")),
        (Some(7), 7, 7),
        "{s}"
    );
}

/// The two budgets of T-W2-23 for one `tools/call` result: what the client
/// reads (`structuredContent`, serialized, in UTF-16 units as a JavaScript
/// client counts) under its limit when the answer is inline, and the whole
/// serialized result under the wire budget.
pub(crate) fn budget_failures(label: &str, result: &Value) -> Vec<String> {
    use al_sem::mcp::tools::{MCP_CLIENT_BUDGET_CHARS, MCP_WIRE_BUDGET_CHARS};
    let mut f = Vec::new();
    let sc = result["structuredContent"]
        .to_string()
        .encode_utf16()
        .count();
    if sc > MCP_CLIENT_BUDGET_CHARS {
        f.push(format!("{label}: structuredContent of {sc} characters"));
    }
    let wire = result.to_string().encode_utf16().count();
    if wire > MCP_WIRE_BUDGET_CHARS {
        f.push(format!("{label}: a result of {wire} characters"));
    }
    f
}

/// T-W2-23 (two budgets; astra 10): every answer keeps `structuredContent`
/// (what Claude Code reads and size-checks) under 50,000 characters and the
/// whole result under its wire budget, however its stdout escapes. On the
/// fixture's answers (each tool, both formats), and on a hand-stated
/// workspace whose explain card (about 35 KB raw, under the 40 KiB byte cap)
/// is mostly quotes and backslashes, so it grows by half when escaped: it
/// goes to a file with its summary. A digest always goes to a file; a call
/// that sets its own `maxInlineBytes` gets that cap exactly (inline here).
///
/// Discrimination (2026-11-05): the pool's client-budget check disabled
/// (`if false && client_chars(..)`) fails at "escaped card:
/// structuredContent of 51970 characters"; restored, passes. Raising the
/// raw cap to 64 KiB does not fail it, by design: the character budget, not
/// the byte cap, is the guard now.
#[test]
fn every_answer_fits_the_client_and_the_wire() {
    use crate::mcp_server::{Server, envelope};
    let mut failures = Vec::new();
    let mut s = Server::ready(&workspace(), None);
    let calls: Vec<(&str, &str, Value)> = vec![
        ("explain", "alsem_explain", json!({"routines": [ISSUE_DOC]})),
        (
            "explain json",
            "alsem_explain",
            json!({"routines": [ISSUE_DOC], "format": "json", "detail": true}),
        ),
        (
            "prove",
            "alsem_prove",
            json!({"routine": ISSUE_DOC, "question": "may-commit"}),
        ),
        (
            "prove json",
            "alsem_prove",
            json!({"routine": ISSUE_DOC, "question": "writes-table:Base Ledger 12", "format": "json"}),
        ),
        (
            "query effects",
            "alsem_query_effects",
            json!({"routine": ISSUE_DOC_Q, "all": true}),
        ),
        (
            "query effects json",
            "alsem_query_effects",
            json!({"routine": ISSUE_DOC_Q, "format": "json"}),
        ),
        (
            "query touches",
            "alsem_query_touches",
            json!({"table": "Out Log", "all": true}),
        ),
        ("digest", "alsem_digest", json!({"routines": [ISSUE_DOC]})),
    ];
    for (i, (label, tool, args)) in calls.into_iter().enumerate() {
        let r = s.call(i as i64 + 1, tool, args);
        failures.extend(budget_failures(label, &r));
        if tool == "alsem_digest" && !envelope(&r)["stdoutFile"].is_object() {
            failures.push("digest: always a file".into());
        }
    }
    assert!(s.finish().success());

    let dir = tempfile::tempdir().unwrap();
    write_ws(dir.path(), &item_t_source(1, 27, 700));
    let mut s = Server::ready(dir.path(), None);
    let args = json!({"routines": ["T Mgt::Check"], "detail": true});
    let r = s.call(1, "alsem_explain", args.clone());
    let env = envelope(&r);
    let (stdout, _, _) = crate::mcp_server::cli_bytes(&env);
    let raw = stdout.len();
    let escaped = serde_json::to_string(std::str::from_utf8(&stdout).unwrap())
        .unwrap()
        .len();
    assert!(
        raw < al_sem::mcp::tools::MCP_DEFAULT_INLINE_BYTES as usize
            && escaped > al_sem::mcp::tools::MCP_CLIENT_BUDGET_CHARS,
        "precondition: under the byte cap raw ({raw}), over the client budget escaped ({escaped})"
    );
    failures.extend(budget_failures("escaped card", &r));
    if !env["stdoutFile"].is_object() || !env["summary"].is_object() {
        failures.push(format!("escaped card: a file with its summary: {env}"));
    }
    let mut own = args;
    own["maxInlineBytes"] = json!(64 * 1024);
    let env = envelope(&s.call(2, "alsem_explain", own));
    if !env["stdoutText"].is_string() {
        failures.push("an own maxInlineBytes is exact: inline".into());
    }
    assert!(s.finish().success());
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

// ---------------------------------------------------------------------------
// The final CDO output review (2026-11-05): items 2 and 3.
// ---------------------------------------------------------------------------

/// The indentation of a line.
fn indent(l: &str) -> usize {
    l.len() - l.trim_start().len()
}

/// Final CDO review, item 2 (design §3.4.2: path and anchor of one
/// witness; never invent a path, say why there is none): every table record
/// under `writes in workspace code` in an explain card or a digest entry
/// (human) has a `path:` line, or a `no path kept: <reason>` line. Returns
/// the failures and how many records it checked. A record is a line two
/// deeper than the section header (not the `table not resolved` line or a
/// `… +N` fold); its block is the deeper lines after it.
pub(crate) fn write_path_failures(label: &str, text: &str) -> (Vec<String>, usize) {
    let lines: Vec<&str> = text.lines().collect();
    let (mut failures, mut checked) = (Vec::new(), 0);
    let mut i = 0;
    while i < lines.len() {
        if !lines[i]
            .trim_start()
            .starts_with("writes in workspace code (")
        {
            i += 1;
            continue;
        }
        let h = indent(lines[i]);
        let mut j = i + 1;
        while j < lines.len() && indent(lines[j]) > h {
            let t = lines[j].trim_start();
            if indent(lines[j]) != h + 2
                || t.starts_with("table not resolved")
                || t.starts_with("… +")
            {
                j += 1;
                continue;
            }
            checked += 1;
            let mut k = j + 1;
            let mut ok = false;
            while k < lines.len() && indent(lines[k]) > h + 2 {
                let s = lines[k].trim_start();
                ok |= s.starts_with("path: ")
                    || s.starts_with("path (")
                    || s.starts_with("no path kept: ");
                k += 1;
            }
            if !ok {
                failures.push(format!("{label}: no path and no reason under {t}"));
            }
            j = k;
        }
        i = j;
    }
    (failures, checked)
}

/// The entry-point counts a human output states (final CDO review, item 3):
/// the scope line's `(N entry points at S sites)`, the `writes beyond
/// dependency entry points (… through K entry points at S sites)` header
/// and the `M entry points at S sites write only temporary records` line,
/// each `None` when absent.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct EntryPointLines {
    pub scope: Option<(usize, usize)>,
    pub writing: Option<(usize, usize)>,
    pub other: Option<(usize, usize)>,
}

/// `N entry point(s) at S site(s)` right after `marker` in `line`.
fn points_at(line: &str, marker: &str) -> Option<(usize, usize)> {
    let rest = &line[line.find(marker)? + marker.len()..];
    let num = |s: &str| -> Option<(usize, usize)> {
        let digits: String = s
            .chars()
            .take_while(|c| c.is_ascii_digit() || *c == ',')
            .filter(char::is_ascii_digit)
            .collect();
        let used = s
            .chars()
            .take_while(|c| c.is_ascii_digit() || *c == ',')
            .count();
        Some((digits.parse().ok()?, used))
    };
    let (n, used) = num(rest)?;
    let rest = rest[used..].strip_prefix(" entry point")?;
    let rest = rest
        .strip_prefix('s')
        .unwrap_or(rest)
        .strip_prefix(" at ")?;
    let (s, _) = num(rest)?;
    Some((n, s))
}

pub(crate) fn entry_point_lines(text: &str) -> EntryPointLines {
    let mut out = EntryPointLines::default();
    for l in text.lines() {
        let t = l.trim_start();
        if t.starts_with("scope: ") {
            out.scope = out.scope.or(points_at(t, "grouped per entry point ("));
        } else if t.starts_with("writes beyond dependency entry points (") {
            out.writing = out.writing.or(points_at(t, " through "));
        } else if t.contains(" write only temporary records or no table") {
            out.other = out.other.or(points_at(t, ""));
        }
    }
    out
}

/// The entry-point lines of a `--detail` card's `writes beyond dependency
/// entry points` section, split into those with may-persist writes and the
/// rest: each line's sites (`entered at a; b — …`), an anchorless one as
/// its whole line (so it still counts once).
pub(crate) fn detail_entry_points(text: &str) -> (Vec<Vec<String>>, Vec<Vec<String>>) {
    let (mut writing, mut other) = (Vec::new(), Vec::new());
    let mut section: Option<usize> = None;
    let mut in_other = false;
    for l in text.lines() {
        let t = l.trim_start();
        if t.starts_with("writes beyond dependency entry points (") {
            section = Some(indent(l));
            in_other = false;
            continue;
        }
        let Some(h) = section else { continue };
        if indent(l) <= h {
            section = None;
            continue;
        }
        if indent(l) != h + 2 {
            continue;
        }
        if t.contains(" write only temporary records or no table:") {
            in_other = true;
            continue;
        }
        let Some((_, rest)) = t.split_once(", entered at ") else {
            continue;
        };
        let sites = rest.split(" — ").next().unwrap_or("");
        let sites: Vec<String> = if sites.is_empty() {
            vec![t.to_string()]
        } else {
            sites.split("; ").map(str::to_string).collect()
        };
        if in_other {
            other.push(sites);
        } else {
            writing.push(sites);
        }
    }
    (writing, other)
}

/// Final CDO review, item 3 (design §3.3, §3.6: a total is a union by
/// identity): the scope line's entry points are the section's writing ones
/// plus the temporary-only ones (they partition), and its sites are the
/// union of both sets' sites, counted from the `--detail` card's own lines
/// (`detail`); each section's counts are its own lines'. `query` (query
/// effects' human answer of the same routine) states the same writing and
/// temporary-only counts as the card.
pub(crate) fn entry_point_failures(
    label: &str,
    card: &str,
    detail: &str,
    query: Option<&str>,
) -> Vec<String> {
    let mut f = Vec::new();
    let c = entry_point_lines(card);
    let (writing, other) = detail_entry_points(detail);
    let union = |sets: &[&Vec<Vec<String>>]| -> usize {
        sets.iter()
            .flat_map(|s| s.iter().flatten())
            .collect::<BTreeSet<_>>()
            .len()
    };
    let Some(scope) = c.scope else {
        f.push(format!("{label}: no entry-point count on the scope line"));
        return f;
    };
    let w = c.writing.unwrap_or((0, 0));
    let o = c.other.unwrap_or((0, 0));
    if scope.0 != w.0 + o.0 {
        f.push(format!(
            "{label}: scope says {} entry points, the sections {} + {}",
            scope.0, w.0, o.0
        ));
    }
    let checks = [
        (
            "scope",
            scope,
            writing.len() + other.len(),
            union(&[&writing, &other]),
        ),
        ("writes beyond", w, writing.len(), union(&[&writing])),
        ("temporary only", o, other.len(), union(&[&other])),
    ];
    for (what, (n, s), lines, sites) in checks {
        if (n, s) != (lines, sites) {
            f.push(format!(
                "{label}: {what} says {n} entry points at {s} sites; the --detail lines \
                 are {lines} at {sites} distinct sites"
            ));
        }
    }
    if let Some(q) = query {
        let q = entry_point_lines(q);
        if (q.writing, q.other) != (c.writing, c.other) {
            f.push(format!(
                "{label}: query effects says {:?} / {:?}, explain {:?} / {:?}",
                q.writing, q.other, c.writing, c.other
            ));
        }
    }
    f
}

/// Final CDO review, items 2 and 3 on the fixture: every workspace-code
/// write record of explain's and digest's cards has a path or a reason, and
/// the entry-point counts of the scope line and the sections agree (union
/// by identity, read off the `--detail` card) in explain, digest and query
/// effects. The CDO twins (`output_cdo::cdo_write_lines_have_a_path_or_a_reason`,
/// `cdo_entry_point_counts_agree`) carry the discrimination proofs: the
/// fixture has no row whose listed paths all cannot run and no entry point
/// that writes only temporary records, so neither break moves it.
#[test]
fn write_lines_have_a_path_and_entry_point_counts_agree() {
    let ws = workspace().to_string_lossy().into_owned();
    let run = |a: &[&str]| {
        let (out, err, code) = run_alsem(a);
        assert_eq!(code, 0, "{a:?}: {err}");
        out
    };
    let mut failures = Vec::new();
    let mut checked = 0;
    for root in [ISSUE_DOC, WRITE_LOG] {
        let card = run(&["explain", &ws, "--routine", root]);
        let detail = run(&["explain", &ws, "--routine", root, "--detail"]);
        let digest = run(&["digest", &ws, "--routine", root, "--format", "human"]);
        for (what, text) in [("explain", &card), ("digest", &digest)] {
            let (f, n) = write_path_failures(&format!("{root} {what}"), text);
            failures.extend(f);
            checked += n;
        }
        let q = root.replace("::", ".");
        let query = run(&["query", "effects", &ws, "--routine", &q]);
        failures.extend(entry_point_failures(root, &card, &detail, Some(&query)));
        failures.extend(entry_point_failures(
            &format!("{root} digest"),
            &digest,
            &detail,
            None,
        ));
    }
    assert!(
        checked >= 2,
        "precondition: write records checked ({checked})"
    );
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

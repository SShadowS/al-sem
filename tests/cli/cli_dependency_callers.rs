//! Callers through dependency code (design r2,
//! `docs/superpowers/specs/2026-11-06-dependency-callers-design.md`), driven
//! through the shipped binaries on `tests/fixtures/dependency-callers` (its
//! README states every case). Expectations are hand-stated from the fixture
//! text, never computed by production code.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/dependency-callers")
}

fn run(bin: &str, args: &[&str], ws: &Path) -> String {
    let out = Command::new(bin)
        .args(args.iter().take(1))
        .arg(ws)
        .args(args.iter().skip(1))
        .output()
        .expect("spawn");
    assert!(
        out.status.success(),
        "{bin} {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).expect("utf8")
}

/// `Object::Routine` of a node line, by its source names.
fn node_name(v: &Value) -> String {
    format!(
        "{}::{}",
        v["objectName"].as_str().unwrap_or("?"),
        v["memberName"].as_str().unwrap_or("?")
    )
}

/// The export's edges carrying `configuredDispatch`, as `(caller, callee)`.
#[test]
fn the_export_labels_the_catalogued_arms_and_nothing_else() {
    let text = run(
        env!("CARGO_BIN_EXE_alsem"),
        &["graph"],
        &fixture().join("ws"),
    );
    let mut names = std::collections::HashMap::new();
    let mut labelled = BTreeSet::new();
    let mut edges = Vec::new();
    for line in text.lines() {
        let v: Value = serde_json::from_str(line).unwrap();
        match v["t"].as_str() {
            Some("node") => {
                names.insert(
                    v["v"]["id"].as_str().unwrap().to_string(),
                    node_name(&v["v"]),
                );
            }
            Some("edge") => edges.push(v["v"].clone()),
            _ => {}
        }
    }
    for e in &edges {
        let conds: Vec<&str> = e["conditions"]
            .as_array()
            .map(|a| a.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        if conds.contains(&"configuredDispatch") {
            labelled.insert((
                names[e["caller"].as_str().unwrap()].clone(),
                names[e["callee"].as_str().unwrap()].clone(),
            ));
        }
    }
    let er = "Workflow Response Handling::ExecuteResponse".to_string();
    let expected: BTreeSet<(String, String)> = [
        "DoNothingCode",
        "DoNothing",
        "PostCode",
        "PostIt",
        "OnExecuteWorkflowResponse",
    ]
    .iter()
    .map(|r| (er.clone(), format!("Workflow Response Handling::{r}")))
    .collect();
    // The export prunes to paths between workspace routines: an arm that
    // leads to none (DoNothing, the label functions) may be pruned with its
    // node. Every labelled edge is an expected one, and PostIt's (on the
    // path to the workspace subscriber) is there.
    assert!(
        labelled.is_subset(&expected),
        "only ExecuteResponse's arm edges carry the condition: {labelled:?}"
    );
    assert!(
        labelled.contains(&(er.clone(), "Workflow Response Handling::PostIt".to_string())),
        "the arm on the path to the workspace is labelled: {labelled:?}"
    );
    assert!(
        !labelled.contains(&(
            er,
            "Workflow Response Handling::OnBeforeExecuteResponse".to_string()
        )),
        "the header's call is not labelled"
    );
}

#[test]
fn the_probe_lists_the_catalogued_case_and_the_catalog_status() {
    let text = run(
        env!("CARGO_BIN_EXE_aldump"),
        &["--configured-dispatch-candidates", "--min-arms", "2"],
        &fixture().join("ws"),
    );
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(
        lines[0],
        "catalog 1: Workflow Response Handling::ExecuteResponse matched (Base Application 1.0.0.0)"
    );
    assert!(
        lines.iter().any(|l| l
            .contains("Codeunit Workflow Response Handling::ExecuteResponse  arms 3  call-arms 3")
            && l.contains("scrutinee WorkflowResponse.\"Function Name\"")
            && l.ends_with("[catalogued]")),
        "{text}"
    );
}

/// `alsem query touches <ws> --table "DC Log" --from "DC Target.Target"
/// --direction up --callers --format json [extra]`, its `payload.up`.
fn touches_up(extra: &[&str]) -> Value {
    let mut args = vec![
        "query",
        "touches",
        "--table",
        "DC Log",
        "--from",
        "DC Target.Target",
        "--direction",
        "up",
        "--callers",
        "--format",
        "json",
    ];
    args.extend_from_slice(extra);
    let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .args(&args[..2])
        .arg(fixture().join("ws"))
        .args(&args[2..])
        .output()
        .expect("spawn alsem");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: Value = serde_json::from_slice(&out.stdout).expect("json");
    assert_eq!(v["schemaVersion"], "3.9.0");
    v["payload"]["up"].clone()
}

/// `objectDisplay::display` of a routine row.
fn routine_name(r: &Value) -> String {
    format!(
        "{}::{}",
        r["objectDisplay"].as_str().unwrap_or("?"),
        r["display"].as_str().unwrap_or("?")
    )
}

#[test]
fn query_touches_up_lists_the_callers_through_dependency_code() {
    let up = touches_up(&[]);
    // The solve graph's answer is unchanged: its own relation and callers.
    assert_eq!(up["relation"], "code-calls");
    let solve: BTreeSet<String> = up["callers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| routine_name(&c["routine"]))
        .collect();
    let td = &up["throughDependency"];
    assert_eq!(td["relation"], "code-calls-through-dependency");
    assert_eq!(td["engine"], "reverse-walk");
    assert_eq!(td["feasibility"], "notChecked");
    assert_eq!(td["limited"], Value::Null);
    assert_eq!(td["catalog"]["version"], "1");
    assert_eq!(td["catalog"]["entries"][0]["status"], "matched");
    let list = td["list"].as_array().unwrap();
    let rows: Vec<(String, String, u64)> = list
        .iter()
        .map(|c| {
            (
                routine_name(&c["routine"]),
                c["tier"].as_str().unwrap().to_string(),
                c["hops"].as_u64().unwrap(),
            )
        })
        .collect();
    // Hand-stated from the fixture (README): by tier, then hops, then name.
    assert_eq!(
        rows,
        vec![
            ("Codeunit DC Both::Mixed".into(), "code".into(), 5),
            ("Codeunit DC Caller::Start".into(), "code".into(), 5),
            ("Codeunit DC Outer::Outer".into(), "code".into(), 6),
            ("Codeunit DC Toucher::Touch".into(), "code".into(), 6),
            (
                "Codeunit DC Approver::Approve".into(),
                "configured".into(),
                5
            ),
        ]
    );
    assert_eq!(td["callers"], 4);
    assert_eq!(td["configured"], 1);
    // Disjoint from the solve graph's callers.
    for (name, ..) in &rows {
        assert!(!solve.contains(name), "{name} is not also a solve caller");
    }
    assert!(solve.contains("Codeunit DC Both::Direct"), "{solve:?}");
    // The witness ends at the root and names the event and the configured arm.
    let start = &list[1];
    // V3 §2.1/§2.2: the one display (AL quoting; a dependency object by
    // name with its number).
    assert_eq!(start["entersAt"], "Codeunit \"DC Sub\"::HandleAfterRun");
    let w = start["witness"].as_array().unwrap();
    assert_eq!(w[2]["kind"], "event");
    assert_eq!(w[2]["event"], "Codeunit \"DC Engine\" (52901)::OnAfterRun");
    assert_eq!(w.last().unwrap()["to"], "Codeunit \"DC Target\"::Target");
    assert_eq!(
        w[1]["at"]["file"], "dep:437dbf0e-84ff-417a-965d-ed2bb9650972:src/Engine.al",
        "a dependency hop's site names its app"
    );
    let approve = &list[4];
    assert_eq!(
        approve["configuredVia"],
        serde_json::json!([{"entry": "Workflow Response Handling::ExecuteResponse", "arm": "PostCode()"}])
    );
    assert_eq!(list[3]["witness"][0]["kind"], "recordTrigger");
}

/// `alsem explain <ws> --routine "DC Target.Target" [extra] --format json`,
/// its one card.
fn explain_card(extra: &[&str]) -> Value {
    let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .arg("explain")
        .arg(fixture().join("ws"))
        .args(["--routine", "DC Target.Target", "--format", "json"])
        .args(extra)
        .output()
        .expect("spawn alsem");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: Value = serde_json::from_slice(&out.stdout).expect("json");
    let cards = v["cards"].as_array().expect("cards");
    assert_eq!(cards.len(), 1);
    cards[0].clone()
}

#[test]
fn explain_counts_the_callers_through_dependency_code_only_when_asked() {
    // Without `--callers-context` the card builds no sited graph and says
    // nothing new.
    assert_eq!(explain_card(&[])["throughDependency"], Value::Null);
    let td = explain_card(&["--callers-context"])["throughDependency"].clone();
    assert_eq!(td["relation"], "code-calls-through-dependency");
    assert_eq!(td["callers"], 4);
    assert_eq!(td["configured"], 1);
    assert_eq!(td["catalog"]["entries"][0]["status"], "matched");
    let nearest: Vec<&str> = td["nearest"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["routine"].as_str().unwrap())
        .collect();
    assert_eq!(nearest.len(), 3, "{td}");
    assert!(
        nearest.contains(&"Codeunit \"DC Caller\"::Start"),
        "the nearest code callers: {nearest:?}"
    );
    assert_eq!(
        td["nearest"][0]["witness"]
            .as_array()
            .unwrap()
            .last()
            .unwrap()["to"],
        "Codeunit \"DC Target\"::Target"
    );
}

#[test]
fn query_touches_up_single_app_is_not_applicable() {
    let up = touches_up(&["--single-app"]);
    assert_eq!(
        up["throughDependency"],
        serde_json::json!({"relation": "code-calls-through-dependency", "notApplicable": "no dependency code loaded"})
    );
}

/// Output validation F5: a callers' context answer's human text names a
/// dependency location by the app's name (`dep:Base Application:…`), never
/// by its guid; the JSON keeps the guid. Hand-stated: `DC Both.Direct`'s path
/// to `Target` passes `DC Engine.Process` in `dep/src/Engine.al`, the
/// dependency whose app.json says id `437dbf0e-…` and name
/// "Base Application".
///
/// Discrimination (2026-11-06): removing the `readable_locations` call after
/// `callers_document` in `prove.rs` fails at "no dependency guid in the human
/// text"; restored, passes.
#[test]
fn callers_context_human_text_names_dependency_locations_by_app_name() {
    const GUID: &str = "437dbf0e-84ff-417a-965d-ed2bb9650972";
    let ws = fixture().join("ws");
    let human = run(
        env!("CARGO_BIN_EXE_alsem"),
        &[
            "prove",
            "DC Target.Target",
            "callers-have-pending-writes",
            "--format",
            "human",
        ],
        &ws,
    );
    assert!(
        human.contains("Codeunit \"DC Engine\" (52901)::Process (dep:"),
        "precondition: a witness path through dependency code: {human}"
    );
    assert!(
        !human.contains(&format!("dep:{GUID}:")),
        "no dependency guid in the human text: {human}"
    );
    assert!(
        human.contains(
            "Codeunit \"DC Engine\" (52901)::Process (dep:Base Application:src/Engine.al:23)"
        ),
        "{human}"
    );
    let json = run(
        env!("CARGO_BIN_EXE_alsem"),
        &["prove", "DC Target.Target", "callers-have-pending-writes"],
        &ws,
    );
    assert!(
        json.contains(&format!("dep:{GUID}:")),
        "the JSON keeps the guid: {json}"
    );
}

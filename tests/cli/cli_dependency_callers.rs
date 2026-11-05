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

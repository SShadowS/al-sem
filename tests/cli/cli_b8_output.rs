//! Engine limits B8 (`docs/engine-limits.md` §1), two small wrong outputs,
//! each pinned through the CLI that prints it:
//!
//! - (4) `query effects` blamed dependency depth (`locks: none observed (one
//!   dependency routine deep; …)` and `limits: reads, locks and validations
//!   one dependency routine deep`) for a cone that enters no dependency code.
//! - (5) an interface call no loaded object implements was counted `not
//!   placed` while explain listed it under `gaps in workspace code`: the
//!   snapshot keeps no call-site evidence for a call with no target, so the
//!   cone item had no anchor.
//!
//! Each fixture states its precondition literally (a tempdir workspace with
//! no dependency; an interface with no implementer).

use std::process::Command;

use serde_json::Value;

use crate::output_fixture::{ISSUE_DOC, workspace as mcp_output_workspace};

fn run(args: &[&str]) -> (String, String, i32) {
    let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .args(args)
        .env("RUST_LOG", "off")
        .output()
        .expect("spawn alsem");
    (
        String::from_utf8_lossy(&out.stdout).replace('\r', ""),
        String::from_utf8_lossy(&out.stderr).to_string(),
        out.status.code().unwrap_or(-1),
    )
}

fn workspace(guid: &str, source: &str) -> tempfile::TempDir {
    let t = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(t.path().join("src")).unwrap();
    std::fs::write(
        t.path().join("app.json"),
        format!(
            r#"{{"id":"{guid}","name":"B8","publisher":"t","version":"1.0.0.0","dependencies":[]}}"#
        ),
    )
    .unwrap();
    std::fs::write(t.path().join("src/b8.al"), source).unwrap();
    t
}

/// (4) No dependency code in the cone: no depth clause, no depth limit, and
/// the families are the whole cone. A cone that does enter dependency code
/// (the MCP-output fixture's `IssueDoc` runs `Base Poster`) keeps both.
#[test]
fn query_effects_states_dependency_depth_only_when_entered() {
    let t = workspace(
        "b8b8b8b8-0000-4000-8000-0000000000b4",
        r#"table 50150 "B8 Item"
{
    fields { field(1; "No."; Code[20]) { } }
}

codeunit 50150 "B8 Reader"
{
    procedure ReadOnly()
    var
        I: Record "B8 Item";
    begin
        if I.FindFirst() then;
    end;
}
"#,
    );
    let ws = t.path().to_string_lossy().into_owned();
    let (human, err, code) = run(&["query", "effects", &ws, "--routine", "B8 Reader::ReadOnly"]);
    assert_eq!(code, 0, "{err}");
    // Precondition: the cone enters no dependency code and holds no lock.
    assert!(human.contains("dependencies: 0 apps"), "{human}");
    let locks = human
        .lines()
        .find(|l| l.trim_start().starts_with("locks:"))
        .unwrap_or_else(|| panic!("a locks line: {human}"));
    assert_eq!(
        locks.trim(),
        "locks: none observed (searched: the whole cone)"
    );
    assert!(
        !human.contains("one dependency routine deep;"),
        "no depth clause: {human}"
    );
    assert!(human.contains("limits: none"), "no depth limit: {human}");

    let (json, err, code) = run(&[
        "query",
        "effects",
        &ws,
        "--routine",
        "B8 Reader::ReadOnly",
        "--format",
        "json",
    ]);
    assert_eq!(code, 0, "{err}");
    let v: Value = serde_json::from_str(&json).expect("json");
    let families = &v["payload"]["result"]["scope"]["families"];
    for f in ["reads", "locks", "validate", "writes"] {
        assert_eq!(families[f]["depth"], "wholeCone", "{f}: {families}");
        assert_eq!(families[f]["inventory"], "exhaustive", "{f}: {families}");
    }

    // The contrast: a cone that enters dependency code keeps the clause.
    let mcp = mcp_output_workspace().to_string_lossy().into_owned();
    let (human, err, code) = run(&["query", "effects", &mcp, "--routine", ISSUE_DOC]);
    assert_eq!(code, 0, "{err}");
    assert!(
        human.contains(
            "locks: none observed (one dependency routine deep; deeper dependency locks are not \
             inventoried)"
        ),
        "{human}"
    );
    assert!(
        human.contains("limits: reads, locks and validations one dependency routine deep"),
        "{human}"
    );
}

/// (5) An interface call with no implementer at all is a gap in workspace
/// code, in the count and in the list alike, at its own call site.
#[test]
fn an_interface_call_with_no_implementer_is_placed() {
    let t = workspace(
        "b8b8b8b8-0000-4000-8000-0000000000b5",
        r#"interface "B8 INoImpl"
{
    procedure Go();
}

codeunit 50151 "B8 Caller"
{
    procedure CallNoImpl(N: Interface "B8 INoImpl")
    begin
        N.Go();
    end;
}
"#,
    );
    let ws = t.path().to_string_lossy().into_owned();
    let (json, err, code) = run(&[
        "explain",
        &ws,
        "--routine",
        "B8 Caller::CallNoImpl",
        "--format",
        "json",
    ]);
    assert_eq!(code, 0, "{err}");
    let v: Value = serde_json::from_str(&json).expect("json");
    let card = &v["cards"][0];
    // Precondition: the one gap is the call with no target, at its line.
    let gaps = card["gaps"].as_array().expect("gaps");
    assert_eq!(gaps.len(), 1, "{gaps:?}");
    assert_eq!(gaps[0]["callee"], "N.Go");
    assert_eq!(gaps[0]["anchor"]["file"], "ws:src/b8.al");
    assert_eq!(gaps[0]["anchor"]["line"], 10);
    // The count agrees with the list: placed in workspace code.
    assert_eq!(card["gapsInDependencies"]["notPlaced"], 0, "{card}");
    assert_eq!(card["gapsInDependencies"]["total"], 0, "{card}");

    let (human, err, code) = run(&["explain", &ws, "--routine", "B8 Caller::CallNoImpl"]);
    assert_eq!(code, 0, "{err}");
    assert!(
        human.contains("gaps: 1 in workspace code, 0 in dependency code, 0 not placed"),
        "{human}"
    );
    assert!(human.contains("gaps in workspace code (1 gap):"), "{human}");
}

//! #160 step 11b T1i (spec `2026-10-22-issue-160-step11b-routine-rekeying-design.md`
//! §5.3, R1-08): the production witness for the policy path. A routine declared
//! in an `#if` / `#else` pair is two model rows with ONE id; `alsem policy
//! check` (the production entry, `gate/policy/pipeline.rs`) evaluates the rule
//! per row today, so its counters count the id twice and it emits two findings
//! that share one finding id. At stage B the policy evaluation keeps one copy of
//! each distinct item (class K): the counters count the id once, one finding,
//! one finding id. Break: evaluate per row. Stage B.

use std::process::Command;

use serde_json::Value;

const APP_JSON: &str = r#"{"id":"11b11b11-0000-0000-0000-0000000011b1","name":"PolicyCollision","publisher":"Test","version":"1.0.0.0","platform":"26.0.0.0","runtime":"13.0","dependencies":[]}"#;

/// `Work` in two equal arms, each inserting `PC T`: one id, one node.
const WORK_AL: &str = r#"table 50890 "PC T"
{
    fields { field(1; "No."; Code[20]) { } }
    keys { key(PK; "No.") { } }
}

codeunit 50890 "PC Work"
{
#if A
    procedure Work()
    var
        T: Record "PC T";
    begin
        T.Insert();
    end;
#else
    procedure Work()
    var
        T: Record "PC T";
    begin
        T.Insert();
    end;
#endif
}
"#;

/// One rule: a DIRECT insert in a routine named `Work`.
const POLICY: &str = r#"version: 1
defaults:
  onUnknown: fail-open
  requireCoverage: any
rules:
  - id: work-insert
    title: "Direct insert in Work"
    severity: high
    when:
      routine.name: "Work"
      capability.op: insert
    facts: direct
"#;

#[test]
fn policy_evaluates_a_collided_routine_once() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().join("ws");
    std::fs::create_dir_all(ws.join("src")).unwrap();
    std::fs::write(ws.join("app.json"), APP_JSON).unwrap();
    std::fs::write(ws.join("src/Work.al"), WORK_AL).unwrap();
    let policy = dir.path().join("policy.yaml");
    std::fs::write(&policy, POLICY).unwrap();

    let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .args(["policy", "check"])
        .arg(&ws)
        .arg("--policy")
        .arg(&policy)
        .args(["--single-app", "--format", "json", "--deterministic"])
        .output()
        .expect("spawn alsem");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let doc: Value = serde_json::from_str(&stdout).unwrap_or_else(|e| {
        panic!(
            "{e}: exit {:?}: stdout {stdout:?} stderr {}",
            out.status.code(),
            String::from_utf8_lossy(&out.stderr)
        )
    });
    let summary = doc["ruleSummaries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["ruleId"] == "work-insert")
        .unwrap_or_else(|| panic!("no summary for work-insert: {doc}"))
        .clone();
    let findings: Vec<&Value> = doc["findings"].as_array().unwrap().iter().collect();
    assert!(
        !findings.is_empty(),
        "precondition: the rule matches the collided routine: {doc}"
    );
    assert_eq!(
        (
            summary["routinesEvaluated"].as_u64(),
            summary["routinesMatched"].as_u64(),
            summary["findingsEmitted"].as_u64()
        ),
        (Some(1), Some(1), Some(1)),
        "the counters count the collided id once: {summary}"
    );
    assert_eq!(findings.len(), 1, "one finding: {doc}");
}

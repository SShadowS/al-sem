//! #160 step 11b T1i (spec `2026-10-22-issue-160-step11b-routine-rekeying-design.md`
//! §5.3, R1-08; the §3.2 "F6 additions" decision FP-03): the production witness for
//! the policy path. A routine declared in an `#if` / `#else` pair is two model rows
//! with ONE id. Policy evaluation is class P: `alsem policy check` (the production
//! entry, `gate/policy/pipeline.rs`) evaluates a collided id ONCE, through its LAST
//! row, so its counters count the id once and it emits at most one finding per rule
//! and id. Policy findings are keyed by routine id, and the rows' anchors differ, so a
//! per-row (class K) evaluation could not collapse natural arms. LIMIT: a row-level
//! predicate (`routine.accessModifier`, `routine.kind`, ...) sees only the LAST row;
//! the facts are the id's (unioned) summary either way. Stage B.

use std::process::Command;

use al_sem::program::model::program_calls::assemble_and_resolve_inline_program_default;
use serde_json::Value;

const APP_GUID: &str = "11b11b11-0000-0000-0000-0000000011b1";

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

/// `Work` in two arms that differ in a ROW-level field: arm A is public, arm B (the
/// last row) is `local`. Both insert `PC T`.
const ACCESS_AL: &str = r#"table 50890 "PC T"
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
    local procedure Work()
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

/// Two rules on the access modifier: one matches the LAST row (`local`), one the
/// first (`public`).
const ACCESS_POLICY: &str = r#"version: 1
defaults:
  onUnknown: fail-open
  requireCoverage: any
rules:
  - id: work-local
    title: "Direct insert in a local Work"
    severity: high
    when:
      routine.name: "Work"
      routine.accessModifier: local
      capability.op: insert
    facts: direct
  - id: work-public
    title: "Direct insert in a public Work"
    severity: high
    when:
      routine.name: "Work"
      routine.accessModifier: public
      capability.op: insert
    facts: direct
"#;

/// The precondition, from the same assembly the CLI runs: `Work` is two rows of
/// one id; returns the rows' access modifiers in row order.
fn collided_work(al: &str) -> Vec<Option<String>> {
    let ws = assemble_and_resolve_inline_program_default(
        &[("src/Work.al".to_string(), al.to_string())],
        APP_GUID,
    )
    .workspace;
    let rows: Vec<_> = ws.routines.iter().filter(|r| r.name == "Work").collect();
    assert_eq!(rows.len(), 2, "precondition: Work is two rows");
    assert_eq!(rows[0].id, rows[1].id, "precondition: one id");
    rows.iter().map(|r| r.access_modifier.clone()).collect()
}

/// `alsem policy check --format json` on a workspace holding `al`.
fn policy_check(al: &str, policy_yaml: &str) -> Value {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().join("ws");
    std::fs::create_dir_all(ws.join("src")).unwrap();
    std::fs::write(ws.join("app.json"), APP_JSON).unwrap();
    std::fs::write(ws.join("src/Work.al"), al).unwrap();
    let policy = dir.path().join("policy.yaml");
    std::fs::write(&policy, policy_yaml).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .args(["policy", "check"])
        .arg(&ws)
        .arg("--policy")
        .arg(&policy)
        .args(["--single-app", "--format", "json", "--deterministic"])
        .output()
        .expect("spawn alsem");
    let stdout = String::from_utf8_lossy(&out.stdout);
    serde_json::from_str(&stdout).unwrap_or_else(|e| {
        panic!(
            "{e}: exit {:?}: stdout {stdout:?} stderr {}",
            out.status.code(),
            String::from_utf8_lossy(&out.stderr)
        )
    })
}

fn summary(doc: &Value, rule: &str) -> Value {
    doc["ruleSummaries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["ruleId"] == rule)
        .unwrap_or_else(|| panic!("no summary for {rule}: {doc}"))
        .clone()
}

/// Equal arms: the counters count the collided id once, one finding. Break:
/// evaluate per row -> counted twice, two findings.
#[test]
fn policy_evaluates_a_collided_routine_once() {
    collided_work(WORK_AL);
    let doc = policy_check(WORK_AL, POLICY);
    let summary = summary(&doc, "work-insert");
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

/// Arms that differ in `routine.accessModifier` (arm A public, arm B, the last
/// row, local): the evaluation is the LAST row's, so `work-local` matches and
/// `work-public` does not. Break: evaluate through the FIRST row -> the other
/// way round.
#[test]
fn policy_evaluates_a_collided_routine_through_its_last_row() {
    let access = collided_work(ACCESS_AL);
    assert!(
        access[0].as_deref() != Some("local") && access[1].as_deref() == Some("local"),
        "precondition: arm A is public, arm B (the last row) local: {access:?}"
    );
    let doc = policy_check(ACCESS_AL, ACCESS_POLICY);
    let matched = |rule: &str| summary(&doc, rule)["routinesMatched"].as_u64();
    assert_eq!(
        (matched("work-local"), matched("work-public")),
        (Some(1), Some(0)),
        "the LAST row (local) is evaluated: {doc}"
    );
}

// ---------------------------------------------------------------------------
// FP-20 (#22): `query effects` anchors each operation of a two-arm routine at
// its own arm, through the production `OperationJoin` use.
// ---------------------------------------------------------------------------

/// `Work` in two arms (arm A inserts `QJ A`, arm B, the last row, modifies
/// `QJ B`), at different lines; `Go` calls `Work`.
const JOIN_AL: &str = r#"table 50891 "QJ A"
{
    fields { field(1; "No."; Code[20]) { } }
    keys { key(PK; "No.") { } }
}

table 50892 "QJ B"
{
    fields { field(1; "No."; Code[20]) { } }
    keys { key(PK; "No.") { } }
}

codeunit 50891 "QJ Work"
{
#if A
    procedure Work()
    var
        A: Record "QJ A";
    begin
        A.Insert();
    end;
#else
    procedure Work()
    var
        B: Record "QJ B";
    begin
        B.Modify();
    end;
#endif

    procedure Go()
    begin
        Work();
    end;
}
"#;

/// `alsem query effects` (the production pipeline, `run_query_effects_pipeline`)
/// on `Go`: each effect of `Work` has its origin at ITS OWN arm's operation.
///
/// Engine limits B2 (per-arm ids, owner decision 2026-11-09): this test used to
/// pin FP-20's class-P pick (both arms owned `op0`, and every `op0` origin was
/// arm B's line, the LAST owning row's, so arm A's insert cited arm B's line).
/// Production now numbers arm B's sites after arm A's, so arm A's insert is
/// `op0` and arm B's modify `op1`, and the pick never has two owners here. The
/// pick for rows that DO share an id is pinned by the unit test
/// `operation_join_reads_the_last_owning_row`, which states the shared ids by
/// assignment. Break: number every arm from 0 (the model builder's
/// `SiteIdBase`) -> arm A's insert cites arm B's line again.
#[test]
fn query_effects_origin_is_each_arms_own_operation() {
    let ws_model = assemble_and_resolve_inline_program_default(
        &[("src/Work.al".to_string(), JOIN_AL.to_string())],
        APP_GUID,
    )
    .workspace;
    let rows: Vec<_> = ws_model
        .routines
        .iter()
        .filter(|r| r.name == "Work")
        .collect();
    assert_eq!(rows.len(), 2, "precondition: Work is two rows");
    assert_eq!(rows[0].id, rows[1].id, "precondition: one id");
    let op = |r: &al_sem::program::model::workspace::ModelRoutine| {
        let o = r.record_operations.first().expect("precondition: an op");
        (
            o.id.rsplit('/').next().unwrap().to_string(),
            u64::from(o.source_anchor.start_line),
        )
    };
    let (a, b) = (op(rows[0]), op(rows[1]));
    assert_eq!(
        (a.0.as_str(), b.0.as_str()),
        ("op0", "op1"),
        "each arm has its own op id"
    );
    assert_ne!(a.1, b.1, "precondition: the arms' lines differ");

    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().join("ws");
    std::fs::create_dir_all(ws.join("src")).unwrap();
    std::fs::write(ws.join("app.json"), APP_JSON).unwrap();
    std::fs::write(ws.join("src/Work.al"), JOIN_AL).unwrap();
    let inputs = al_sem::program::inputs::WorkspaceInputs::read(&ws, &Default::default());
    let q = al_sem::engine::l4::effect_query_cli::run_query_effects_pipeline(
        &inputs,
        "QJ Work.Go",
        "t",
        true,
        true,
    )
    .expect("query effects");
    let doc: Value = serde_json::from_str(&q.json_text).expect("query json");
    let effects = doc["payload"]["effects"]
        .as_array()
        .unwrap_or_else(|| panic!("effects: {doc}"));
    // The model's rows are 0-based; the document's lines 1-based (`lineBase: 1`).
    for (suffix, line) in [(&a.0, a.1), (&b.0, b.1)] {
        let lines: Vec<u64> = effects
            .iter()
            .filter(|e| {
                e["operationId"]
                    .as_str()
                    .is_some_and(|o| o.ends_with(&format!("/{suffix}")))
            })
            .filter_map(|e| e["origin"]["anchor"]["line"].as_u64())
            .collect();
        assert!(
            !lines.is_empty(),
            "precondition: Go's effects include Work's {suffix}: {doc}"
        );
        assert!(
            lines.iter().all(|&l| l == line + 1),
            "every {suffix} origin is its own arm's row {line}, written 1-based: {lines:?}"
        );
    }
}

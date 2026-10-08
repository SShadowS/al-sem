//! Gap G-4 (docs/engine-gaps.md): d1's transitive "a loop in X reaches <op>"
//! framing reads as if the TERMINAL routine loops, when the loop is purely in an
//! ancestor. Such findings are GENUINELY REAL (the op runs once per ancestor
//! iteration — real SQL cost), so the fix is WORDING ONLY, never suppression:
//! when the terminal op's OWN routine has no loop around the op, the rootCause
//! must name the terminal routine and attribute the loop to the ancestor
//! explicitly.
//!
//! Firing-preservation guards (the heart of G-4's "do not over-reach"):
//!   - the pure-transitive finding STILL FIRES at the SAME severity (high for a
//!     write at loop depth 1) — only the text changes;
//!   - a direct in-loop op in its OWN routine keeps the original wording;
//!   - a transitive terminal op that sits inside the CALLEE's own loop keeps the
//!     original wording (the callee genuinely loops — nothing misleading).

use al_sem::engine::l5::detectors::registered_detectors;
use al_sem::engine::l5::finding::Finding;
use al_sem::engine::l5::registry::run_detectors;
use al_sem::program::model::program_calls::assemble_and_resolve_inline_program_default;

const APP_GUID: &str = "11111111-0000-0000-0000-0000000g4abc";

/// Run d1 in isolation over an inline workspace and return its emitted findings.
fn run_d1(files: &[(String, String)]) -> Vec<Finding> {
    let resolved = assemble_and_resolve_inline_program_default(files, APP_GUID);
    let d1: Vec<_> = registered_detectors()
        .into_iter()
        .filter(|d| d.name == "d1-db-op-in-loop")
        .collect();
    assert_eq!(d1.len(), 1, "d1 detector must be registered exactly once");
    run_detectors(&resolved, &d1).findings
}

fn al(name: &str, body: &str) -> (String, String) {
    (format!("src/{name}.al"), body.to_string())
}

fn root_causes(findings: &[Finding]) -> Vec<&str> {
    findings.iter().map(|f| f.root_cause.as_str()).collect()
}

/// Two distinct physical tables: the loop cursor's table and the op's table, so
/// the rootCause (`"<Op> on <table>"`) unambiguously identifies the terminal op.
const TABLES: &str = r#"
table 50701 "G4 Cust"
{
    fields { field(1; "No."; Code[20]) { } }
    keys { key(PK; "No.") { } }
}

table 50702 "G4 Log"
{
    fields { field(1; "No."; Code[20]) { } }
    keys { key(PK; "No.") { } }
}
"#;

// --- (a) pure-transitive: loop in ancestor X, single Insert in loop-less leaf Z

/// `LoopCaller` repeats over a cursor calling `CreateLogEntry`; the leaf does a
/// single `Insert` and has NO loop of its own. The finding must STILL FIRE at
/// `high` (write, depth 1) — real per-iteration cost — but the rootCause must
/// name the terminal routine and make explicit that the loop is in the ancestor.
#[test]
fn pure_transitive_wording_names_terminal_routine_and_still_fires() {
    let src = format!(
        r#"{TABLES}
codeunit 50701 "G4 Transitive"
{{
    procedure LoopCaller(var Cust: Record "G4 Cust")
    begin
        repeat
            CreateLogEntry();
        until Cust.Next() = 0;
    end;

    procedure CreateLogEntry()
    var
        Log: Record "G4 Log";
    begin
        Log.Insert();
    end;
}}
"#
    );
    let findings = run_d1(&[al("G4Transitive", &src)]);
    assert_eq!(
        findings.len(),
        1,
        "the transitive Insert must FIRE (G-4 is wording-only, never suppression). findings: {:#?}",
        root_causes(&findings)
    );
    let f = &findings[0];
    assert_eq!(
        f.severity, "high",
        "severity must be unchanged by the wording fix (write at loop depth 1 = high). rootCause: {}",
        f.root_cause
    );
    assert!(
        f.root_cause
            .contains("A loop in LoopCaller reaches Insert on G4 Log"),
        "the loop must still be attributed to the ancestor. rootCause: {}",
        f.root_cause
    );
    assert!(
        f.root_cause.contains("in CreateLogEntry"),
        "the rootCause must NAME the terminal routine so the text no longer reads \
         as if the op's own routine loops. rootCause: {}",
        f.root_cause
    );
    assert!(
        f.root_cause.contains("has no loop of its own"),
        "the rootCause must state the terminal routine itself does not loop. rootCause: {}",
        f.root_cause
    );
    assert!(
        f.root_cause.contains("once per iteration"),
        "the rootCause must state the per-ancestor-iteration cost (the finding is real). rootCause: {}",
        f.root_cause
    );
}

// --- (b) CONTROL: direct in-loop op keeps the original wording -------------

/// The op sits in a loop WITHIN its own routine — nothing misleading, so the
/// wording must be byte-identical to the original shape.
#[test]
fn direct_in_loop_op_keeps_original_wording() {
    let src = format!(
        r#"{TABLES}
codeunit 50702 "G4 Direct"
{{
    procedure DirectLoop(var Cust: Record "G4 Cust")
    var
        Log: Record "G4 Log";
    begin
        repeat
            Log.Insert();
        until Cust.Next() = 0;
    end;
}}
"#
    );
    let findings = run_d1(&[al("G4Direct", &src)]);
    assert_eq!(
        findings.len(),
        1,
        "the direct in-loop Insert must fire. findings: {:#?}",
        root_causes(&findings)
    );
    assert_eq!(
        findings[0].root_cause, "A loop in DirectLoop reaches Insert on G4 Log.",
        "a direct in-loop op must keep the ORIGINAL wording unchanged"
    );
    assert_eq!(findings[0].severity, "high");
}

// --- (c) CONTROL: callee that loops ITSELF keeps the original wording ------

/// `LoopCaller` loops calling `BatchInsert`, whose Insert sits inside the
/// CALLEE's OWN `for` loop. The terminal routine genuinely loops, so the
/// original wording is not misleading and must stay (no "has no loop" clause).
#[test]
fn transitive_terminal_inside_callee_own_loop_keeps_original_wording() {
    let src = format!(
        r#"{TABLES}
codeunit 50703 "G4 CalleeLoops"
{{
    procedure LoopCaller(var Cust: Record "G4 Cust")
    begin
        repeat
            BatchInsert();
        until Cust.Next() = 0;
    end;

    procedure BatchInsert()
    var
        Log: Record "G4 Log";
        i: Integer;
    begin
        for i := 1 to 3 do
            Log.Insert();
    end;
}}
"#
    );
    let findings = run_d1(&[al("G4CalleeLoops", &src)]);
    // Two findings: the transitive one rooted at LoopCaller's loop AND the
    // callee's own direct in-loop finding; merge_by_terminal folds them into one
    // (same terminal op).
    assert!(
        !findings.is_empty(),
        "the callee's in-loop Insert must fire. findings: {:#?}",
        root_causes(&findings)
    );
    for f in &findings {
        assert!(
            !f.root_cause.contains("has no loop of its own"),
            "a terminal op inside the CALLEE's own loop must keep the original \
             wording (the callee genuinely loops). rootCause: {}",
            f.root_cause
        );
    }
}

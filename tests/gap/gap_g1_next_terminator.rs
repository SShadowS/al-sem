//! Gap G-1 (docs/engine-gaps.md): d1 must NOT fire on the `until <var>.Next() = 0`
//! TERMINATOR of the very loop being iterated — that `Next()` IS the loop's own
//! cursor advancement (removing it breaks the loop), not an extra db op in the body.
//!
//! Structural signal: the L2 body walk marks a record op that sits inside the
//! `condition` field of its nearest enclosing `repeat_statement`
//! (`in_until_condition`). d1 skips a `Next` op carrying that proof in BOTH its
//! direct-op branch and `terminals_at` (the interprocedural walk).
//!
//! Suppression-direction safety (controls):
//!   - a REAL db op in the loop body still fires;
//!   - a mid-body `Next()` advancing a DIFFERENT cursor still fires;
//!   - the loop's own cursor-opening `FindSet` inside an OUTER loop still fires.

use al_sem::engine::l5::detectors::registered_detectors;
use al_sem::engine::l5::finding::Finding;
use al_sem::engine::l5::registry::run_detectors;
use al_sem::program::model::program_calls::assemble_and_resolve_inline_program_default;

const APP_GUID: &str = "11111111-0000-0000-0000-0000000g1abc";

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

/// The shared physical tables. Two DISTINCT tables so a finding's rootCause
/// (`"<Op> on <table>"`) unambiguously identifies which record var it hit.
const TABLES: &str = r#"
table 50601 "G1 Cust"
{
    fields { field(1; "No."; Code[20]) { } }
    keys { key(PK; "No.") { } }
}

table 50602 "G1 Other"
{
    fields { field(1; "No."; Code[20]) { } }
    keys { key(PK; "No.") { } }
}
"#;

// --- (a) the terminator Next() must NOT fire -------------------------------

/// `repeat <non-db work> until Cust.Next() = 0` over a PHYSICAL record (a var
/// param, so the pre-loop cursor-opener heuristic does NOT cover it): the
/// `Next()` is the loop's own terminator → NO d1 finding at all.
#[test]
fn terminator_next_is_suppressed() {
    let src = format!(
        r#"{TABLES}
codeunit 50601 "G1 Terminator"
{{
    procedure ProcessAll(var Cust: Record "G1 Cust")
    var
        Total: Integer;
    begin
        repeat
            Total += 1;
        until Cust.Next() = 0;
    end;
}}
"#
    );
    let findings = run_d1(&[al("G1Terminator", &src)]);
    assert!(
        findings.is_empty(),
        "the loop's own `until Cust.Next() = 0` terminator must NOT fire d1. findings: {:#?}",
        root_causes(&findings)
    );
}

// --- (b) CONTROL: a real db op in the body must STILL fire -----------------

/// Same loop shape, but the body does `Other.Modify()` — a genuine db op inside
/// the loop. d1 must STILL fire on the Modify (and still not on the terminator).
#[test]
fn real_db_op_in_body_still_fires() {
    let src = format!(
        r#"{TABLES}
codeunit 50602 "G1 BodyOp"
{{
    procedure ProcessAll(var Cust: Record "G1 Cust"; var Other: Record "G1 Other")
    begin
        repeat
            Other.Modify();
        until Cust.Next() = 0;
    end;
}}
"#
    );
    let findings = run_d1(&[al("G1BodyOp", &src)]);
    assert_eq!(
        findings.len(),
        1,
        "exactly the in-body Modify must fire. findings: {:#?}",
        root_causes(&findings)
    );
    assert!(
        findings[0].root_cause.contains("Modify on G1 Other"),
        "the surviving finding must be the in-body Modify. rootCause: {}",
        findings[0].root_cause
    );
    assert!(
        !findings[0].root_cause.contains("Next on"),
        "the terminator Next must stay suppressed. rootCause: {}",
        findings[0].root_cause
    );
}

// --- (c) CONTROL: a mid-body Next() on a DIFFERENT cursor must STILL fire --

/// The body advances a SECOND cursor (`Other.Next()`) per iteration of the
/// `Cust` loop — that is a real per-iteration retrieval (and not the loop's
/// terminator), so d1 must keep firing on it.
#[test]
fn mid_body_next_on_different_cursor_still_fires() {
    let src = format!(
        r#"{TABLES}
codeunit 50603 "G1 SecondCursor"
{{
    procedure ProcessAll(var Cust: Record "G1 Cust"; var Other: Record "G1 Other")
    begin
        repeat
            Other.Next();
        until Cust.Next() = 0;
    end;
}}
"#
    );
    let findings = run_d1(&[al("G1SecondCursor", &src)]);
    assert_eq!(
        findings.len(),
        1,
        "exactly the mid-body Other.Next() must fire. findings: {:#?}",
        root_causes(&findings)
    );
    assert!(
        findings[0].root_cause.contains("Next on G1 Other"),
        "the surviving finding must be the SECOND cursor's Next. rootCause: {}",
        findings[0].root_cause
    );
    assert!(
        !findings[0].root_cause.contains("Next on G1 Cust"),
        "the Cust loop's own terminator must stay suppressed. rootCause: {}",
        findings[0].root_cause
    );
}

// --- (d) opener inside an OUTER loop: FindSet fires, terminator doesn't ----

/// `for ... do begin if Cust.FindSet() then repeat ... until Cust.Next() = 0`:
/// the FindSet sits inside the outer loop (a REAL repeated db op → fires); the
/// `Next()` terminator must NOT (the pre-loop cursor-opener heuristic does not
/// cover this shape — the opener's loopStack is non-empty — so only the
/// structural terminator proof suppresses it).
#[test]
fn opener_inside_outer_loop_keeps_findset_suppresses_terminator() {
    let src = format!(
        r#"{TABLES}
codeunit 50604 "G1 NestedOpener"
{{
    procedure Outer()
    var
        Cust: Record "G1 Cust";
        i: Integer;
        Total: Integer;
    begin
        for i := 1 to 5 do begin
            if Cust.FindSet() then
                repeat
                    Total += 1;
                until Cust.Next() = 0;
        end;
    end;
}}
"#
    );
    let findings = run_d1(&[al("G1NestedOpener", &src)]);
    assert!(
        findings
            .iter()
            .any(|f| f.root_cause.contains("FindSet on G1 Cust")),
        "the in-loop FindSet is a real repeated db op and must still fire. findings: {:#?}",
        root_causes(&findings)
    );
    assert!(
        !findings.iter().any(|f| f.root_cause.contains("Next on")),
        "the `until Cust.Next() = 0` terminator must NOT fire even when the opener \
         sits inside an outer loop. findings: {:#?}",
        root_causes(&findings)
    );
}

// --- (e) transitive: a callee's own terminator must not fire from a caller loop

/// `Caller` loops calling `Helper`; `Helper` runs its own
/// `FindSet → repeat … until Cust.Next() = 0`. The interprocedural walk
/// (`terminals_at`) must surface the FindSet (a real db op reached per caller
/// iteration) but NOT the helper loop's own terminator Next.
#[test]
fn transitive_terminator_in_callee_is_suppressed() {
    let src = format!(
        r#"{TABLES}
codeunit 50605 "G1 Transitive"
{{
    procedure Helper(var Cust: Record "G1 Cust")
    var
        Total: Integer;
    begin
        if Cust.FindSet() then
            repeat
                Total += 1;
            until Cust.Next() = 0;
    end;

    procedure Caller()
    var
        Cust: Record "G1 Cust";
        i: Integer;
    begin
        for i := 1 to 5 do
            Helper(Cust);
    end;
}}
"#
    );
    let findings = run_d1(&[al("G1Transitive", &src)]);
    assert!(
        findings
            .iter()
            .any(|f| f.root_cause.contains("FindSet on G1 Cust")),
        "the callee's FindSet reached from the caller loop must still fire. findings: {:#?}",
        root_causes(&findings)
    );
    assert!(
        !findings.iter().any(|f| f.root_cause.contains("Next on")),
        "the callee loop's own terminator Next must NOT fire transitively. findings: {:#?}",
        root_causes(&findings)
    );
}

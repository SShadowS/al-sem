//! Gap G-7 (docs/engine-gaps.md): `d1` fires on a routine whose ONLY caller is
//! commented out — the routine is dead (d14 correctly flags it), so the perf
//! finding is moot. The fix is DOWN-CONFIDENCE ONLY, never suppression: when
//! EVERY path root routine of a d1 finding is provably dead by d14's EXACT
//! criteria (no inbound edge from the entry-point closure, `local`/app-scoped
//! `internal` access, not a Test object, not a property-expression host, not
//! itself a reachable root), the finding KEEPS FIRING at the SAME severity but
//! its confidence drops one notch (likely → possible) and the rootCause gains
//! an explanatory note.
//!
//! Firing-preservation guards (the heart of G-7's "do not compound d14's FPs"):
//!   - the dead-rooted finding STILL FIRES at the SAME severity — only the
//!     confidence level and the note change;
//!   - a LIVE caller restores full confidence (control);
//!   - a PUBLIC routine with no callers is NOT down-confidenced (it is a
//!     reachable root — the open world may call it);
//!   - a merged finding with one LIVE and one dead loop root keeps full
//!     confidence (any live path keeps the finding fully actionable).

use al_sem::engine::l5::detectors::registered_detectors;
use al_sem::engine::l5::finding::Finding;
use al_sem::engine::l5::registry::run_detectors;
use al_sem::program::model::program_calls::assemble_and_resolve_inline_program_default;

const APP_GUID: &str = "11111111-0000-0000-0000-0000000g7abc";

/// The G-7 note fragment appended to a down-confidenced rootCause.
const UNREACHABLE_NOTE: &str = "appears unreachable from any entry point";

/// Run a single detector in isolation over an inline workspace.
fn run_one(detector: &str, files: &[(String, String)]) -> Vec<Finding> {
    let resolved = assemble_and_resolve_inline_program_default(files, APP_GUID);
    let selected: Vec<_> = registered_detectors()
        .into_iter()
        .filter(|d| d.name == detector)
        .collect();
    assert_eq!(
        selected.len(),
        1,
        "{detector} must be registered exactly once"
    );
    run_detectors(&resolved, &selected).findings
}

fn al(name: &str, body: &str) -> (String, String) {
    (format!("src/{name}.al"), body.to_string())
}

fn root_causes(findings: &[Finding]) -> Vec<&str> {
    findings.iter().map(|f| f.root_cause.as_str()).collect()
}

const TABLES: &str = r#"
table 50711 "G7 Cust"
{
    fields { field(1; "No."; Code[20]) { } }
    keys { key(PK; "No.") { } }
}

table 50712 "G7 Log"
{
    fields { field(1; "No."; Code[20]) { } }
    keys { key(PK; "No.") { } }
}
"#;

// --- (a) the gap: dead local routine → finding fires at LOWERED confidence --

/// `DeadWorker` is `local` and its only caller is commented out — d14 flags it
/// dead. The in-loop Insert finding must STILL FIRE at the same severity, but
/// at `possible` confidence with the unreachable note.
#[test]
fn dead_routine_finding_still_fires_at_lowered_confidence() {
    let src = format!(
        r#"{TABLES}
codeunit 50711 "G7 Dead"
{{
    procedure Entry()
    begin
        // DeadWorker();
    end;

    local procedure DeadWorker()
    var
        Cust: Record "G7 Cust";
        Log: Record "G7 Log";
    begin
        if Cust.FindSet() then
            repeat
                Log.Insert();
            until Cust.Next() = 0;
    end;
}}
"#
    );
    let files = [al("G7Dead", &src)];

    // Sanity: the fixture IS dead per d14 (validates the same-criteria premise).
    let d14 = run_one("d14-dead-routine", &files);
    assert_eq!(
        d14.len(),
        1,
        "d14 must flag DeadWorker dead (fixture premise). findings: {:#?}",
        root_causes(&d14)
    );
    assert!(
        d14[0].root_cause.contains("DeadWorker"),
        "d14 must name DeadWorker. rootCause: {}",
        d14[0].root_cause
    );

    let findings = run_one("d1-db-op-in-loop", &files);
    assert_eq!(
        findings.len(),
        1,
        "the in-loop Insert must STILL FIRE (G-7 is down-confidence only, never \
         suppression). findings: {:#?}",
        root_causes(&findings)
    );
    let f = &findings[0];
    assert_eq!(
        f.severity, "high",
        "severity must be UNCHANGED (write at loop depth 1 = high). rootCause: {}",
        f.root_cause
    );
    assert_eq!(
        f.confidence.level, "possible",
        "confidence must drop one notch (likely -> possible) when every path root \
         is provably dead. rootCause: {}",
        f.root_cause
    );
    assert!(
        f.root_cause.contains(UNREACHABLE_NOTE),
        "the rootCause must carry the unreachable note. rootCause: {}",
        f.root_cause
    );
}

// --- (b) CONTROL: live caller → full confidence, no note --------------------

/// Identical workspace except the caller is LIVE: `DeadWorker` is reachable
/// from the public `Entry`, so the finding keeps full confidence and no note.
#[test]
fn live_caller_keeps_full_confidence() {
    let src = format!(
        r#"{TABLES}
codeunit 50712 "G7 Live"
{{
    procedure Entry()
    begin
        DeadWorker();
    end;

    local procedure DeadWorker()
    var
        Cust: Record "G7 Cust";
        Log: Record "G7 Log";
    begin
        if Cust.FindSet() then
            repeat
                Log.Insert();
            until Cust.Next() = 0;
    end;
}}
"#
    );
    let files = [al("G7Live", &src)];

    // Sanity: nothing is dead here.
    let d14 = run_one("d14-dead-routine", &files);
    assert!(
        d14.is_empty(),
        "no routine is dead in the control. d14 findings: {:#?}",
        root_causes(&d14)
    );

    let findings = run_one("d1-db-op-in-loop", &files);
    assert_eq!(
        findings.len(),
        1,
        "the in-loop Insert must fire. findings: {:#?}",
        root_causes(&findings)
    );
    let f = &findings[0];
    assert_eq!(f.severity, "high");
    assert_eq!(
        f.confidence.level, "likely",
        "a live caller keeps FULL confidence. rootCause: {}",
        f.root_cause
    );
    assert!(
        !f.root_cause.contains(UNREACHABLE_NOTE),
        "no unreachable note for a live routine. rootCause: {}",
        f.root_cause
    );
}

// --- (c) CONTROL: public routine with no callers is NOT down-confidenced ----

/// A PUBLIC procedure with no in-app callers is a reachable ROOT (the open
/// world — another app, a page action — may call it), so d14 does not flag it
/// and d1 must keep full confidence. This is the "only on a STRONG unreachable
/// signal" guard.
#[test]
fn public_routine_without_callers_keeps_full_confidence() {
    let src = format!(
        r#"{TABLES}
codeunit 50713 "G7 Public"
{{
    procedure PublicWorker()
    var
        Cust: Record "G7 Cust";
        Log: Record "G7 Log";
    begin
        if Cust.FindSet() then
            repeat
                Log.Insert();
            until Cust.Next() = 0;
    end;
}}
"#
    );
    let files = [al("G7Public", &src)];

    let d14 = run_one("d14-dead-routine", &files);
    assert!(
        d14.is_empty(),
        "a public routine is never d14-dead. d14 findings: {:#?}",
        root_causes(&d14)
    );

    let findings = run_one("d1-db-op-in-loop", &files);
    assert_eq!(findings.len(), 1);
    assert_eq!(
        findings[0].confidence.level, "likely",
        "a public no-caller routine must keep FULL confidence (open-world). rootCause: {}",
        findings[0].root_cause
    );
    assert!(!findings[0].root_cause.contains(UNREACHABLE_NOTE));
}

// --- (d) CONTROL: merged finding with one LIVE loop root keeps confidence ---

/// Two loops reach the SAME terminal Insert: one in a live public routine, one
/// in a dead local routine. `merge_by_terminal` folds them into ONE finding —
/// since at least one path root is live, the finding must keep full confidence.
#[test]
fn mixed_live_and_dead_loop_roots_keep_full_confidence() {
    let src = format!(
        r#"{TABLES}
codeunit 50714 "G7 Mixed"
{{
    procedure LiveLoop(var Cust: Record "G7 Cust")
    begin
        repeat
            InsertLog();
        until Cust.Next() = 0;
    end;

    local procedure DeadLoop(var Cust: Record "G7 Cust")
    begin
        repeat
            InsertLog();
        until Cust.Next() = 0;
    end;

    local procedure InsertLog()
    var
        Log: Record "G7 Log";
    begin
        Log.Insert();
    end;
}}
"#
    );
    let files = [al("G7Mixed", &src)];

    // Sanity: DeadLoop is dead, InsertLog is live (reached from LiveLoop).
    let d14 = run_one("d14-dead-routine", &files);
    assert_eq!(
        d14.len(),
        1,
        "exactly DeadLoop must be d14-dead. findings: {:#?}",
        root_causes(&d14)
    );
    assert!(d14[0].root_cause.contains("DeadLoop"));

    let findings = run_one("d1-db-op-in-loop", &files);
    assert_eq!(
        findings.len(),
        1,
        "both loop paths share the terminal Insert and must merge into one \
         finding. findings: {:#?}",
        root_causes(&findings)
    );
    let f = &findings[0];
    assert_eq!(
        f.confidence.level, "likely",
        "one LIVE path root keeps FULL confidence (down-confidence requires \
         EVERY path root dead). rootCause: {}",
        f.root_cause
    );
    assert!(
        !f.root_cause.contains(UNREACHABLE_NOTE),
        "no unreachable note when a live path exists. rootCause: {}",
        f.root_cause
    );
}

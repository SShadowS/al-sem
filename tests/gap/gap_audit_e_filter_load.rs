//! Detector-audit class E (docs/detector-audit.md): net-effect filter / load
//! correctness for d41 + d42. Drives the REAL detectors over inline AL
//! workspaces (mirrors `tests/gap_g15_d3_d42_writes.rs`).
//!
//! d41 (transitive-filter-loss) Gap-T: the "caller filtered before the call"
//!   check must use the NET filter state at the callsite — a `SetRange` the
//!   caller itself wiped with its OWN `Reset` before the call leaves no filter
//!   to be lost across the helper. The naive first-prior-`SetRange` (ignoring an
//!   intervening `Reset`) fires when no filter was active.
//!
//! d42 (cross-call-wrong-setloadfields) Gap-Y: a FlowField/FlowFilter the callee
//!   reads must NOT count toward the required-load set — those are not physical
//!   columns, `SetLoadFields` ignores them, so they never force a wider narrow.
//!
//! Suppression signals are exact + structural; control cases confirm the
//! detectors still fire when the signal is absent.

use al_sem::engine::l5::detectors::registered_detectors;
use al_sem::engine::l5::finding::Finding;
use al_sem::engine::l5::registry::run_detectors;
use al_sem::program::model::program_calls::assemble_and_resolve_inline_program_default;

const APP_GUID: &str = "11111111-0000-0000-0000-0000000e5abc";

fn run_detector(name: &str, files: &[(String, String)]) -> Vec<Finding> {
    let resolved = assemble_and_resolve_inline_program_default(files, APP_GUID);
    let detectors: Vec<_> = registered_detectors()
        .into_iter()
        .filter(|d| d.name == name)
        .collect();
    assert_eq!(detectors.len(), 1, "{name} must be registered exactly once");
    run_detectors(&resolved, &detectors).findings
}

fn run_d41(files: &[(String, String)]) -> Vec<Finding> {
    run_detector("d41-transitive-filter-loss", files)
}

fn run_d42(files: &[(String, String)]) -> Vec<Finding> {
    run_detector("d42-cross-call-wrong-setloadfields", files)
}

fn al(name: &str, body: &str) -> (String, String) {
    (format!("src/{name}.al"), body.to_string())
}

// ===========================================================================
// d41 — net-effect filter state at the callsite (Gap-T)
// ===========================================================================

const TABLE_D41: &str = r#"
table 50180 "E41 Item"
{
    fields
    {
        field(1; "No."; Code[20]) { }
        field(2; Description; Text[100]) { }
    }
    keys { key(PK; "No.") { } }
}
"#;

/// Caller `SetRange`s, then `Reset`s the record ITSELF, then forwards to a
/// helper that also Resets; the post-call `FindSet` operates on a set the caller
/// already unfiltered. No filter is lost ACROSS the helper → no d41.
#[test]
fn d41_caller_reset_before_call_is_suppressed() {
    let cu_src = r#"
codeunit 50180 "E41 Reset"
{
    procedure Caller()
    var Item: Record "E41 Item";
    begin
        Item.SetRange("No.", '1000');
        Item.Reset();
        ClearIt(Item);
        if Item.FindSet() then;
    end;

    procedure ClearIt(var R: Record "E41 Item")
    begin
        R.Reset();
    end;
}
"#;
    let findings = run_d41(&[al("E41Item", TABLE_D41), al("E41Reset", cu_src)]);
    assert!(
        findings.is_empty(),
        "a SetRange the caller wiped with its OWN Reset before the call is not \
         lost across the helper — no d41. findings: {findings:#?}"
    );
}

/// Control: SAME shape WITHOUT the caller's Reset — the SetRange is still active
/// at the callsite, the helper's Reset silently loses it → d41 fires.
#[test]
fn d41_active_filter_lost_across_helper_still_fires() {
    let cu_src = r#"
codeunit 50181 "E41 Live"
{
    procedure Caller()
    var Item: Record "E41 Item";
    begin
        Item.SetRange("No.", '1000');
        ClearIt(Item);
        if Item.FindSet() then;
    end;

    procedure ClearIt(var R: Record "E41 Item")
    begin
        R.Reset();
    end;
}
"#;
    let findings = run_d41(&[al("E41Item", TABLE_D41), al("E41Live", cu_src)]);
    assert_eq!(
        findings.len(),
        1,
        "an ACTIVE SetRange lost across a helper that Resets is a real d41. \
         findings: {findings:#?}"
    );
}

// ===========================================================================
// d42 — FlowField fields excluded from the required-load set (Gap-Y)
// ===========================================================================

const TABLE_D42: &str = r#"
table 50182 "E42 Item"
{
    fields
    {
        field(1; "No."; Code[20]) { }
        field(2; Description; Text[100]) { }
        field(3; Amount; Decimal) { }
        field(4; Balance; Decimal) { FieldClass = FlowField; CalcFormula = Count("E42 Item"); }
    }
    keys { key(PK; "No.") { } }
}
"#;

/// Caller narrows the load to `Amount`, then forwards to a helper that reads the
/// FlowField `Balance`. A FlowField is never part of the SQL load (it is
/// materialised by CalcFields), so it cannot be "missing from the load" → no
/// extra round-trip → no d42.
#[test]
fn d42_callee_reads_flowfield_is_suppressed() {
    let cu_src = r#"
codeunit 50182 "E42 Flow"
{
    procedure Caller(): Decimal
    var Item: Record "E42 Item";
    begin
        Item.SetLoadFields(Amount);
        if Item.FindFirst() then
            exit(NeedsBalance(Item));
    end;

    local procedure NeedsBalance(var R: Record "E42 Item"): Decimal
    begin
        exit(R.Balance);
    end;
}
"#;
    let findings = run_d42(&[al("E42Item", TABLE_D42), al("E42Flow", cu_src)]);
    assert!(
        findings.is_empty(),
        "a FlowField the callee reads is not a physical load field — \
         SetLoadFields ignores it, no extra round-trip, no d42. findings: {findings:#?}"
    );
}

/// Control: callee reads a NORMAL field (`Description`) the caller did not load
/// → a real missing physical column → d42 fires.
#[test]
fn d42_callee_reads_unloaded_normal_field_still_fires() {
    let cu_src = r#"
codeunit 50183 "E42 Norm"
{
    procedure Caller(): Text
    var Item: Record "E42 Item";
    begin
        Item.SetLoadFields(Amount);
        if Item.FindFirst() then
            exit(NeedsDesc(Item));
    end;

    local procedure NeedsDesc(var R: Record "E42 Item"): Text[100]
    begin
        exit(R.Description);
    end;
}
"#;
    let findings = run_d42(&[al("E42Item", TABLE_D42), al("E42Norm", cu_src)]);
    assert_eq!(
        findings.len(),
        1,
        "a NORMAL field the callee reads but the caller did not load is a real \
         extra round-trip — d42 fires. findings: {findings:#?}"
    );
}

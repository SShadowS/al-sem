//! Gap G-3 (docs/engine-gaps.md): d33 blind to filters set via helper
//! procedures (interprocedural).
//!
//! `d33-unfiltered-bulk-write` must NOT fire on a `DeleteAll` / `ModifyAll`
//! when the receiver was passed `var` into a helper EARLIER in the routine and
//! that helper applies a narrowing filter (`SetRange` / `SetFilter`) to the
//! by-var parameter (one-hop callee summary over the resolved call graph —
//! same machinery as G-10's load-wrapper gate).
//!
//! Suppression-direction guardrails (controls below): a bulk write with NO
//! prior filter call still fires; a callee that does NOT filter the var-arg
//! still fires; a filter call AFTER the bulk write still fires; a by-VALUE
//! callee filter still fires; a `Reset` between the helper call and the bulk
//! write still fires.
//!
//! Drives the REAL detector over inline AL workspaces (mirrors
//! `tests/gap_g10_load_wrappers.rs`).

use al_sem::engine::l5::detectors::registered_detectors;
use al_sem::engine::l5::finding::Finding;
use al_sem::engine::l5::registry::run_detectors;
use al_sem::program::model::program_calls::assemble_and_resolve_inline_program_default;

const APP_GUID: &str = "11111111-0000-0000-0000-0000000g3abc";

/// Run d33 over an inline workspace and return all emitted findings.
fn run_g3_detector(files: &[(String, String)]) -> Vec<Finding> {
    let resolved = assemble_and_resolve_inline_program_default(files, APP_GUID);
    let detectors: Vec<_> = registered_detectors()
        .into_iter()
        .filter(|d| d.name == "d33-unfiltered-bulk-write")
        .collect();
    assert_eq!(detectors.len(), 1, "d33 must be registered exactly once");
    run_detectors(&resolved, &detectors).findings
}

fn al(name: &str, body: &str) -> (String, String) {
    (format!("src/{name}.al"), body.to_string())
}

const TABLE_SRC: &str = r#"
table 50160 "G3 Template Line"
{
    fields
    {
        field(1; "Template Code"; Code[20]) { }
        field(2; "Line No."; Integer) { }
    }
    keys { key(PK; "Template Code", "Line No.") { } }
}
"#;

// --- Suppression: helper filter before the bulk write ---------------------------

/// The receiver is passed `var` into local helpers that `SetRange` /
/// `SetFilter` it before the `DeleteAll` / `ModifyAll` — the receiver IS
/// filtered. NO d33.
#[test]
fn helper_filter_before_bulk_write_suppresses_d33() {
    let cu_src = r#"
codeunit 50160 "G3 HelperFilter"
{
    procedure DeleteTemplateLines()
    var Line: Record "G3 Template Line";
    begin
        SetTemplateFilter(Line);
        Line.DeleteAll();
    end;

    procedure ClearTemplateLines()
    var Line: Record "G3 Template Line";
    begin
        SetMergeFieldFilter(Line);
        Line.ModifyAll("Line No.", 0);
    end;

    local procedure SetTemplateFilter(var R: Record "G3 Template Line")
    begin
        R.SetRange("Template Code", 'T-001');
    end;

    local procedure SetMergeFieldFilter(var R: Record "G3 Template Line")
    begin
        R.SetFilter("Line No.", '>%1', 0);
    end;
}
"#;
    let findings = run_g3_detector(&[
        al("G3TemplateLine", TABLE_SRC),
        al("G3HelperFilter", cu_src),
    ]);
    assert!(
        findings.is_empty(),
        "a by-var helper that filters the receiver before the bulk write means the \
         receiver IS filtered — d33 must not fire. findings: {findings:#?}"
    );
}

// --- CONTROL: no prior filter call still fires -----------------------------------

/// `DeleteAll` with NO prior filter of any kind — d33 must STILL fire
/// (suppression-direction guard).
#[test]
fn control_no_filter_still_fires() {
    let cu_src = r#"
codeunit 50161 "G3 NoFilter"
{
    procedure DeleteBlind()
    var Line: Record "G3 Template Line";
    begin
        Line.DeleteAll();
    end;
}
"#;
    let findings = run_g3_detector(&[al("G3TemplateLine", TABLE_SRC), al("G3NoFilter", cu_src)]);
    assert!(
        findings
            .iter()
            .any(|f| f.detector == "d33-unfiltered-bulk-write"
                && f.root_cause.contains("DeleteBlind")),
        "d33 must still fire on DeleteAll with no prior filter. findings: {findings:#?}"
    );
}

// --- CONTROL: callee that does NOT filter the var-arg still fires ----------------

/// The helper takes the record `var` but never sets a filter (it only reads
/// it) — the caller's `DeleteAll` is still unfiltered. d33 must STILL fire.
#[test]
fn control_non_filtering_callee_still_fires() {
    let cu_src = r#"
codeunit 50162 "G3 NonFiltering"
{
    procedure DeleteAfterReadOnlyHelper()
    var Line: Record "G3 Template Line";
    begin
        InspectLines(Line);
        Line.DeleteAll();
    end;

    local procedure InspectLines(var R: Record "G3 Template Line")
    begin
        if R.FindFirst() then;
    end;
}
"#;
    let findings = run_g3_detector(&[
        al("G3TemplateLine", TABLE_SRC),
        al("G3NonFiltering", cu_src),
    ]);
    assert!(
        findings
            .iter()
            .any(|f| f.detector == "d33-unfiltered-bulk-write"
                && f.root_cause.contains("DeleteAfterReadOnlyHelper")),
        "d33 must still fire when the callee never filters the var-arg. \
         findings: {findings:#?}"
    );
}

// --- CONTROL: filter call AFTER the bulk write still fires -----------------------

/// The filter helper runs AFTER the `DeleteAll` — it proves nothing about the
/// bulk write. d33 must STILL fire.
#[test]
fn control_filter_call_after_bulk_write_still_fires() {
    let cu_src = r#"
codeunit 50163 "G3 LateFilter"
{
    procedure DeleteThenFilter()
    var Line: Record "G3 Template Line";
    begin
        Line.DeleteAll();
        SetTemplateFilter(Line);
    end;

    local procedure SetTemplateFilter(var R: Record "G3 Template Line")
    begin
        R.SetRange("Template Code", 'T-001');
    end;
}
"#;
    let findings = run_g3_detector(&[al("G3TemplateLine", TABLE_SRC), al("G3LateFilter", cu_src)]);
    assert!(
        findings
            .iter()
            .any(|f| f.detector == "d33-unfiltered-bulk-write"
                && f.root_cause.contains("DeleteThenFilter")),
        "d33 must still fire when the filter helper runs AFTER the DeleteAll. \
         findings: {findings:#?}"
    );
}

// --- CONTROL: by-VALUE callee filter does not count -------------------------------

/// The helper filters a BY-VALUE copy — the caller's record stays unfiltered.
/// d33 must STILL fire.
#[test]
fn control_by_value_callee_filter_still_fires() {
    let cu_src = r#"
codeunit 50164 "G3 ByValue"
{
    procedure DeleteAfterCopyFilter()
    var Line: Record "G3 Template Line";
    begin
        FilterCopy(Line);
        Line.DeleteAll();
    end;

    local procedure FilterCopy(R: Record "G3 Template Line")
    begin
        R.SetRange("Template Code", 'T-001');
    end;
}
"#;
    let findings = run_g3_detector(&[al("G3TemplateLine", TABLE_SRC), al("G3ByValue", cu_src)]);
    assert!(
        findings
            .iter()
            .any(|f| f.detector == "d33-unfiltered-bulk-write"
                && f.root_cause.contains("DeleteAfterCopyFilter")),
        "d33 must still fire when the callee filters only a by-value copy. \
         findings: {findings:#?}"
    );
}

// --- CONTROL: Reset between the helper call and the bulk write still fires --------

/// A `Reset` on the receiver AFTER the filter helper wipes its filters — the
/// `DeleteAll` is unfiltered again. d33 must STILL fire.
#[test]
fn control_reset_after_helper_filter_still_fires() {
    let cu_src = r#"
codeunit 50165 "G3 ResetAfter"
{
    procedure DeleteAfterReset()
    var Line: Record "G3 Template Line";
    begin
        SetTemplateFilter(Line);
        Line.Reset();
        Line.DeleteAll();
    end;

    local procedure SetTemplateFilter(var R: Record "G3 Template Line")
    begin
        R.SetRange("Template Code", 'T-001');
    end;
}
"#;
    let findings = run_g3_detector(&[al("G3TemplateLine", TABLE_SRC), al("G3ResetAfter", cu_src)]);
    assert!(
        findings
            .iter()
            .any(|f| f.detector == "d33-unfiltered-bulk-write"
                && f.root_cause.contains("DeleteAfterReset")),
        "d33 must still fire when a Reset wipes the helper-applied filter before \
         the DeleteAll. findings: {findings:#?}"
    );
}

// --- CONTROL: callee whose NET effect is unfiltered (filter then Reset) -----------

/// The helper sets a filter but then `Reset`s the by-var parameter — its net
/// effect leaves the receiver unfiltered. d33 must STILL fire.
#[test]
fn control_callee_filter_then_reset_still_fires() {
    let cu_src = r#"
codeunit 50166 "G3 CalleeReset"
{
    procedure DeleteAfterSelfClearingHelper()
    var Line: Record "G3 Template Line";
    begin
        FilterThenReset(Line);
        Line.DeleteAll();
    end;

    local procedure FilterThenReset(var R: Record "G3 Template Line")
    begin
        R.SetRange("Template Code", 'T-001');
        R.Reset();
    end;
}
"#;
    let findings = run_g3_detector(&[al("G3TemplateLine", TABLE_SRC), al("G3CalleeReset", cu_src)]);
    assert!(
        findings
            .iter()
            .any(|f| f.detector == "d33-unfiltered-bulk-write"
                && f.root_cause.contains("DeleteAfterSelfClearingHelper")),
        "d33 must still fire when the callee's net effect is unfiltered \
         (filter then Reset). findings: {findings:#?}"
    );
}

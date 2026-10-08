//! Gap G-10 (docs/engine-gaps.md): record-loading wrappers not recognized as
//! loads.
//!
//! `d11-modify-without-get` / `d21-read-without-load` must NOT fire when the
//! record WAS loaded — just not via a literal `Get`/`Find` record op:
//!
//! - Tier 1: the platform built-in `GetBySystemId(...)` (a complete row fetch)
//!   is not in the L2 record-op map, so it surfaces as a member CALL SITE, not
//!   a record operation — d11/d21 now recognize it as a load.
//! - Tier 2: the record was passed `var` into a callee earlier in the routine,
//!   and that callee performs a recognized load op (`Get`/`Find*`/…) on the
//!   by-var parameter (one-hop callee summary over the resolved call graph).
//!
//! Suppression-direction guardrails (controls below): a `Modify`/`TestField`
//! with NO prior load still fires; a callee that does NOT load the var-arg
//! still fires; a callee that loads a BY-VALUE copy still fires.
//!
//! Drives the REAL detectors over inline AL workspaces (mirrors
//! `tests/gap_g9_trigger_rec.rs`).

use al_sem::engine::l5::detectors::registered_detectors;
use al_sem::engine::l5::finding::Finding;
use al_sem::engine::l5::registry::run_detectors;
use al_sem::program::model::program_calls::assemble_and_resolve_inline_program_default;

const APP_GUID: &str = "11111111-0000-0000-0000-000000g10abc";

/// Run d11 + d21 over an inline workspace and return all emitted findings.
fn run_g10_detectors(files: &[(String, String)]) -> Vec<Finding> {
    let resolved = assemble_and_resolve_inline_program_default(files, APP_GUID);
    let wanted = ["d11-modify-without-get", "d21-read-without-load"];
    let detectors: Vec<_> = registered_detectors()
        .into_iter()
        .filter(|d| wanted.contains(&d.name.as_str()))
        .collect();
    assert_eq!(
        detectors.len(),
        2,
        "d11/d21 must each be registered exactly once"
    );
    run_detectors(&resolved, &detectors).findings
}

fn al(name: &str, body: &str) -> (String, String) {
    (format!("src/{name}.al"), body.to_string())
}

const TABLE_SRC: &str = r#"
table 50150 "G10 Item"
{
    fields
    {
        field(1; "No."; Code[20]) { }
        field(2; Description; Text[100]) { }
    }
    keys { key(PK; "No.") { } }
}
"#;

// --- Tier 1: GetBySystemId is a load -------------------------------------------

/// `Item.GetBySystemId(Id)` performs a complete row fetch — the subsequent
/// `Modify` / `TestField` operate on a loaded record. NO d11/d21.
#[test]
fn get_by_system_id_suppresses_d11_and_d21() {
    let cu_src = r#"
codeunit 50150 "G10 Tier1"
{
    procedure ModifyAfterGetBySystemId(Id: Guid)
    var Item: Record "G10 Item";
    begin
        Item.GetBySystemId(Id);
        Item.Modify();
    end;

    procedure ReadAfterGetBySystemId(Id: Guid)
    var Item: Record "G10 Item";
    begin
        Item.GetBySystemId(Id);
        Item.TestField(Description);
    end;
}
"#;
    let findings = run_g10_detectors(&[al("G10Item", TABLE_SRC), al("G10Tier1", cu_src)]);
    assert!(
        findings.is_empty(),
        "GetBySystemId loads the record — d11/d21 must not fire after it. \
         findings: {findings:#?}"
    );
}

// --- Tier 1 CONTROL: no load at all still fires ---------------------------------

/// `Modify` / `TestField` with NO prior load of any kind — d11 and d21 must
/// both STILL fire (suppression-direction guard).
#[test]
fn control_no_load_still_fires() {
    let cu_src = r#"
codeunit 50151 "G10 NoLoad"
{
    procedure MutateBlind()
    var Item: Record "G10 Item";
    begin
        Item.Modify();
    end;

    procedure ReadBlind()
    var Item: Record "G10 Item";
    begin
        Item.TestField("No.");
    end;
}
"#;
    let findings = run_g10_detectors(&[al("G10Item", TABLE_SRC), al("G10NoLoad", cu_src)]);
    assert!(
        findings
            .iter()
            .any(|f| f.detector == "d11-modify-without-get"
                && f.root_cause.contains("MutateBlind")),
        "d11 must still fire on Modify with no prior load. findings: {findings:#?}"
    );
    assert!(
        findings
            .iter()
            .any(|f| f.detector == "d21-read-without-load" && f.root_cause.contains("ReadBlind")),
        "d21 must still fire on TestField with no prior load. findings: {findings:#?}"
    );
}

// --- Tier 1 CONTROL: GetBySystemId AFTER the op does not count ------------------

/// The load must be STRICTLY BEFORE the mutating/reading op — a
/// `GetBySystemId` after the `Modify` proves nothing.
#[test]
fn control_get_by_system_id_after_op_still_fires() {
    let cu_src = r#"
codeunit 50152 "G10 LateLoad"
{
    procedure ModifyThenLoad(Id: Guid)
    var Item: Record "G10 Item";
    begin
        Item.Modify();
        Item.GetBySystemId(Id);
    end;
}
"#;
    let findings = run_g10_detectors(&[al("G10Item", TABLE_SRC), al("G10LateLoad", cu_src)]);
    assert!(
        findings
            .iter()
            .any(|f| f.detector == "d11-modify-without-get"
                && f.root_cause.contains("ModifyThenLoad")),
        "d11 must still fire when GetBySystemId comes AFTER the Modify. \
         findings: {findings:#?}"
    );
}

// --- Tier 1 CONTROL: GetBySystemId on a DIFFERENT record does not count ---------

/// A `GetBySystemId` on another record variable must not satisfy the load
/// precondition for this one.
#[test]
fn control_get_by_system_id_on_other_record_still_fires() {
    let cu_src = r#"
codeunit 50153 "G10 OtherRec"
{
    procedure ModifyWrongRec(Id: Guid)
    var
        Item: Record "G10 Item";
        Other: Record "G10 Item";
    begin
        Other.GetBySystemId(Id);
        Item.Modify();
    end;
}
"#;
    let findings = run_g10_detectors(&[al("G10Item", TABLE_SRC), al("G10OtherRec", cu_src)]);
    assert!(
        findings
            .iter()
            .any(|f| f.detector == "d11-modify-without-get"
                && f.root_cause.contains("ModifyWrongRec")),
        "d11 must still fire when GetBySystemId loaded a DIFFERENT record. \
         findings: {findings:#?}"
    );
}

// --- Tier 2: by-var callee that loads the record --------------------------------

/// The record is passed `var` into a local helper that `Get`s / `FindFirst`s
/// it — after the call the record is loaded. NO d11/d21.
#[test]
fn by_var_loading_callee_suppresses_d11_and_d21() {
    let cu_src = r#"
codeunit 50154 "G10 Tier2"
{
    procedure ModifyAfterHelperLoad()
    var Item: Record "G10 Item";
    begin
        LoadIt(Item);
        Item.Modify();
    end;

    procedure ReadAfterHelperLoad()
    var Item: Record "G10 Item";
    begin
        FindIt(Item);
        Item.TestField(Description);
    end;

    local procedure LoadIt(var R: Record "G10 Item")
    begin
        R.Get('A');
    end;

    local procedure FindIt(var R: Record "G10 Item")
    begin
        R.SetRange("No.", 'A');
        R.FindFirst();
    end;
}
"#;
    let findings = run_g10_detectors(&[al("G10Item", TABLE_SRC), al("G10Tier2", cu_src)]);
    let primary: Vec<_> = findings
        .iter()
        .filter(|f| {
            f.root_cause.contains("ModifyAfterHelperLoad")
                || f.root_cause.contains("ReadAfterHelperLoad")
        })
        .collect();
    assert!(
        primary.is_empty(),
        "a by-var callee that loads the record satisfies the load precondition — \
         d11/d21 must not fire in the caller. findings: {findings:#?}"
    );
}

// --- Tier 2 CONTROL: callee that does NOT load the var-arg still fires ----------

/// The helper takes the record `var` but only sets a filter — it never loads
/// the record, so the caller's `Modify` is still blind. d11 must STILL fire.
#[test]
fn control_non_loading_callee_still_fires() {
    let cu_src = r#"
codeunit 50155 "G10 NonLoading"
{
    procedure ModifyAfterFilterOnly()
    var Item: Record "G10 Item";
    begin
        FilterIt(Item);
        Item.Modify();
    end;

    local procedure FilterIt(var R: Record "G10 Item")
    begin
        R.SetRange("No.", 'A');
    end;
}
"#;
    let findings = run_g10_detectors(&[al("G10Item", TABLE_SRC), al("G10NonLoading", cu_src)]);
    assert!(
        findings
            .iter()
            .any(|f| f.detector == "d11-modify-without-get"
                && f.root_cause.contains("ModifyAfterFilterOnly")),
        "d11 must still fire when the callee only filters (never loads) the var-arg. \
         findings: {findings:#?}"
    );
}

// --- Tier 2 CONTROL: by-VALUE callee load does not count ------------------------

/// The helper loads a BY-VALUE copy — the caller's record stays unloaded.
/// d11 must STILL fire.
#[test]
fn control_by_value_callee_load_still_fires() {
    let cu_src = r#"
codeunit 50156 "G10 ByValue"
{
    procedure ModifyAfterCopyLoad()
    var Item: Record "G10 Item";
    begin
        LoadCopy(Item);
        Item.Modify();
    end;

    local procedure LoadCopy(R: Record "G10 Item")
    begin
        R.Get('A');
    end;
}
"#;
    let findings = run_g10_detectors(&[al("G10Item", TABLE_SRC), al("G10ByValue", cu_src)]);
    assert!(
        findings
            .iter()
            .any(|f| f.detector == "d11-modify-without-get"
                && f.root_cause.contains("ModifyAfterCopyLoad")),
        "d11 must still fire when the callee loads only a by-value copy. \
         findings: {findings:#?}"
    );
}

// --- Tier 2 CONTROL: unresolved callee does not count ---------------------------

/// A call to a procedure the resolver cannot resolve (no such routine in the
/// workspace) proves nothing — d11 must STILL fire.
#[test]
fn control_unresolved_callee_still_fires() {
    let cu_src = r#"
codeunit 50157 "G10 Unresolved"
{
    procedure ModifyAfterUnknownCall()
    var Item: Record "G10 Item";
    begin
        SomeUnknownHelper(Item);
        Item.Modify();
    end;
}
"#;
    let findings = run_g10_detectors(&[al("G10Item", TABLE_SRC), al("G10Unresolved", cu_src)]);
    assert!(
        findings
            .iter()
            .any(|f| f.detector == "d11-modify-without-get"
                && f.root_cause.contains("ModifyAfterUnknownCall")),
        "d11 must still fire when the callee cannot be resolved. findings: {findings:#?}"
    );
}

//! Detector-audit d29 FP-1 (docs/detector-audit.md): a subscriber that mutates
//! the inbound record with `RunTrigger=false` (`Modify(false)` / `Delete(false)`
//! / `ModifyAll(..., false)`) does NOT re-fire the publisher event — the platform
//! skips the modify/delete triggers — so there is no recursive-event loop and no
//! d29. Only an exact `false` literal suppresses; `Modify()` / `Modify(true)` /
//! a non-literal arg keep firing (suppression-direction safe).
//!
//! Drives the REAL detector over inline AL workspaces (mirrors
//! `tests/gap_audit_d2_guards.rs`).

use al_sem::engine::l5::detectors::registered_detectors;
use al_sem::engine::l5::finding::Finding;
use al_sem::engine::l5::registry::run_detectors;
use al_sem::program::model::program_calls::assemble_and_resolve_inline_program_default;

const APP_GUID: &str = "11111111-0000-0000-0000-00000d29abcd";

fn run_d29(files: &[(String, String)]) -> Vec<Finding> {
    let resolved = assemble_and_resolve_inline_program_default(files, APP_GUID);
    let detectors: Vec<_> = registered_detectors()
        .into_iter()
        .filter(|d| d.name == "d29-subscriber-modify-on-event-record")
        .collect();
    assert_eq!(detectors.len(), 1);
    run_detectors(&resolved, &detectors).findings
}

fn al(name: &str, body: &str) -> (String, String) {
    (format!("src/{name}.al"), body.to_string())
}

const TABLE_SRC: &str = r#"
table 50190 "D29 Item"
{
    fields
    {
        field(1; "No."; Code[20]) { }
        field(2; Description; Text[100]) { }
    }
    keys { key(PK; "No.") { } }
}
"#;

fn sub(name: &str, body: &str) -> (String, String) {
    al(
        name,
        &format!(
            r#"
codeunit 50190 "{name}"
{{
    [EventSubscriber(ObjectType::Table, Database::"D29 Item", 'OnAfterModifyEvent', '', false, false)]
    local procedure OnAfterModifyItem(var Rec: Record "D29 Item"; var xRec: Record "D29 Item"; RunTrigger: Boolean)
    begin
        {body}
    end;
}}
"#
        ),
    )
}

/// Control: bare `Modify()` (RunTrigger defaults to TRUE) re-fires the event → d29.
#[test]
fn modify_default_run_trigger_still_fires() {
    let findings = run_d29(&[
        al("D29Item", TABLE_SRC),
        sub("D29 Default", "Rec.Modify();"),
    ]);
    assert_eq!(
        findings.len(),
        1,
        "bare Modify() runs triggers → recursive event → d29 fires. findings: {findings:#?}"
    );
}

/// Control: explicit `Modify(true)` runs triggers → d29 fires.
#[test]
fn modify_run_trigger_true_still_fires() {
    let findings = run_d29(&[
        al("D29Item", TABLE_SRC),
        sub("D29 True", "Rec.Modify(true);"),
    ]);
    assert_eq!(
        findings.len(),
        1,
        "Modify(true) runs triggers → d29 fires. findings: {findings:#?}"
    );
}

/// FP-1: `Modify(false)` suppresses trigger re-firing → no recursion → no d29.
#[test]
fn modify_run_trigger_false_is_suppressed() {
    let findings = run_d29(&[
        al("D29Item", TABLE_SRC),
        sub("D29 False", "Rec.Modify(false);"),
    ]);
    assert!(
        findings.is_empty(),
        "Modify(false) skips the modify trigger → the event is not re-raised → \
         no d29. findings: {findings:#?}"
    );
}

/// FP-1: `Delete(false)` likewise suppressed.
#[test]
fn delete_run_trigger_false_is_suppressed() {
    let findings = run_d29(&[
        al("D29Item", TABLE_SRC),
        sub("D29 DelFalse", "Rec.Delete(false);"),
    ]);
    assert!(
        findings.is_empty(),
        "Delete(false) skips the delete trigger → no recursive event → no d29. \
         findings: {findings:#?}"
    );
}

/// Control: `Delete()` (default RunTrigger) still fires.
#[test]
fn delete_default_run_trigger_still_fires() {
    let findings = run_d29(&[al("D29Item", TABLE_SRC), sub("D29 Del", "Rec.Delete();")]);
    assert_eq!(
        findings.len(),
        1,
        "bare Delete() runs triggers → d29 fires. findings: {findings:#?}"
    );
}

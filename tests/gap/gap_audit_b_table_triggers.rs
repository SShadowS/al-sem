//! Detector-audit class B (docs/detector-audit.md): table-LEVEL triggers
//! `OnInsert` / `OnModify` / `OnDelete` / `OnRename` run with the implicit
//! `Rec` already loaded by the AL platform, and the platform AUTO-PERSISTS
//! (writes / renames / deletes) `Rec` after the trigger returns. So:
//!
//! - `d21-read-without-load` must NOT fire on `Rec.TestField(...)` in those
//!   triggers (Rec IS loaded),
//! - `d37-validate-without-persist` must NOT fire on `Rec.Validate(...)` in
//!   them (the platform persists Rec after the trigger; for `OnDelete` the
//!   record is being deleted, so "validate without persist" is moot),
//! - `d39-record-left-dirty-across-chain` must NOT fire when such a trigger
//!   forwards `Rec` by-var to a helper that exits dirty (same auto-persist).
//!
//! Suppression-direction controls: the SAME ops in a non-trigger procedure,
//! or on a NON-Rec record inside the trigger, must still fire.
//!
//! Drives the REAL detectors over inline AL workspaces (mirrors
//! `tests/gap_g14_onlookup_triggers.rs`).

use al_sem::engine::l5::detectors::registered_detectors;
use al_sem::engine::l5::finding::Finding;
use al_sem::engine::l5::registry::run_detectors;
use al_sem::program::model::program_calls::assemble_and_resolve_inline_program_default;

const APP_GUID: &str = "11111111-0000-0000-0000-000000audb01";

/// Run d21 + d37 + d39 over an inline workspace and return all emitted findings.
fn run_class_b_detectors(files: &[(String, String)]) -> Vec<Finding> {
    let resolved = assemble_and_resolve_inline_program_default(files, APP_GUID);
    let wanted = [
        "d21-read-without-load",
        "d37-validate-without-persist",
        "d39-record-left-dirty-across-chain",
    ];
    let detectors: Vec<_> = registered_detectors()
        .into_iter()
        .filter(|d| wanted.contains(&d.name.as_str()))
        .collect();
    assert_eq!(
        detectors.len(),
        3,
        "d21/d37/d39 must each be registered exactly once"
    );
    run_detectors(&resolved, &detectors).findings
}

fn al(name: &str, body: &str) -> (String, String) {
    (format!("src/{name}.al"), body.to_string())
}

// --- (a) Rec ops inside table-level triggers → no d21/d37/d39 ------------------

/// A Table whose table-level triggers read (`TestField`), `Validate`, and
/// forward `Rec` by-var to a dirty helper — all on the platform-loaded,
/// auto-persisted `Rec`. NO d21/d37/d39 finding may be emitted.
#[test]
fn table_level_trigger_rec_ops_are_suppressed() {
    let table_src = r#"
table 50160 "AuditB Item"
{
    fields
    {
        field(1; "No."; Code[20]) { }
        field(2; Description; Text[100]) { }
        field(3; "Unit Price"; Decimal) { }
    }
    keys { key(PK; "No.") { } }

    trigger OnInsert()
    begin
        Rec.Validate("Unit Price", 10);
    end;

    trigger OnModify()
    begin
        Rec.TestField("No.");
        MakeDirty(Rec);
    end;

    trigger OnDelete()
    begin
        Rec.TestField(Description);
        Rec.Validate(Description, 'deleting');
    end;

    trigger OnRename()
    begin
        Rec.Validate(Description, 'renamed');
    end;

    local procedure MakeDirty(var Item: Record "AuditB Item")
    begin
        Item.Validate("Unit Price", 99);
    end;
}
"#;
    let findings = run_class_b_detectors(&[al("AuditBItem", table_src)]);
    assert!(
        findings.is_empty(),
        "table-level OnInsert/OnModify/OnDelete/OnRename ops on the platform-loaded, \
         auto-persisted Rec must not fire d21/d37/d39. findings: {findings:#?}"
    );
}

// --- (b) CONTROL: non-trigger procedure keeps firing ---------------------------

/// A plain codeunit procedure doing the SAME ops on a local record — NOT a
/// trigger → d21/d37/d39 must all STILL fire (suppression-direction guard).
#[test]
fn control_non_trigger_procedure_still_fires() {
    let table_src = r#"
table 50161 "AuditB Plain"
{
    fields
    {
        field(1; "No."; Code[20]) { }
        field(2; Description; Text[100]) { }
    }
    keys { key(PK; "No.") { } }
}
"#;
    let cu_src = r#"
codeunit 50161 "AuditB Control"
{
    procedure ReadBlind()
    var Item: Record "AuditB Plain";
    begin
        Item.TestField("No.");
    end;

    procedure ValidateNoPersist()
    var Item: Record "AuditB Plain";
    begin
        Item.Get('X');
        Item.Validate(Description, 'y');
    end;

    procedure ForwardDirty()
    var Item: Record "AuditB Plain";
    begin
        Item.Get('X');
        MakeDirty(Item);
    end;

    local procedure MakeDirty(var Item: Record "AuditB Plain")
    begin
        Item.Validate(Description, 'z');
    end;
}
"#;
    let findings =
        run_class_b_detectors(&[al("AuditBPlain", table_src), al("AuditBControl", cu_src)]);
    assert!(
        findings
            .iter()
            .any(|f| f.detector == "d21-read-without-load" && f.root_cause.contains("ReadBlind")),
        "d21 must still fire on a non-trigger TestField without load. findings: {findings:#?}"
    );
    assert!(
        findings
            .iter()
            .any(|f| f.detector == "d37-validate-without-persist"
                && f.root_cause.contains("ValidateNoPersist")),
        "d37 must still fire on a non-trigger Validate without persist. findings: {findings:#?}"
    );
    assert!(
        findings
            .iter()
            .any(|f| f.detector == "d39-record-left-dirty-across-chain"
                && f.root_cause.contains("ForwardDirty")),
        "d39 must still fire on a non-trigger caller leaving the record dirty. \
         findings: {findings:#?}"
    );
}

// --- (c) CONTROL: NON-Rec record inside a table-level trigger keeps firing -----

/// Inside `OnModify`, the SAME ops on a LOCAL record variable (not Rec) are
/// still real problems — the suppression must stay receiver-exact.
#[test]
fn control_trigger_op_on_non_rec_record_still_fires() {
    let table_src = r#"
table 50162 "AuditB Other"
{
    fields
    {
        field(1; "No."; Code[20]) { }
        field(2; Description; Text[100]) { }
    }
    keys { key(PK; "No.") { } }

    trigger OnModify()
    var Other: Record "AuditB Other";
    begin
        Other.TestField("No.");
    end;
}
"#;
    let findings = run_class_b_detectors(&[al("AuditBOther", table_src)]);
    assert!(
        findings
            .iter()
            .any(|f| f.detector == "d21-read-without-load" && f.root_cause.contains("Other")),
        "d21 must still fire on a non-Rec record inside OnModify. findings: {findings:#?}"
    );
}

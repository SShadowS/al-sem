//! G-13 — d10 (self-modifying-loop) and d39 (record-left-dirty-across-chain)
//! were never temp-gated: they fired on `Known(true)` TEMPORARY records.
//!
//!  - d10: an in-memory cursor self-modify is safe — cursor corruption only
//!    applies to physical SQL cursors. A `Delete`/`Modify` of the iterating
//!    record inside its own loop must NOT fire when the record is temporary.
//!  - d39: discarding in-memory state has no SQL consequence — a temporary
//!    record left Validate-dirty across a helper chain must NOT fire.
//!
//! Suppression-direction guard: the gate ONLY skips on `temp_state` Known(true).
//! PHYSICAL (Known(false)) and Unknown records keep firing — the CONTROL tests
//! prove the detectors still fire on physical records.
//!
//! Drives the REAL detectors in-process over inline AL workspaces, exactly like
//! `tests/temp_state_d1_path.rs`.

use al_sem::engine::l5::detectors::registered_detectors;
use al_sem::engine::l5::finding::Finding;
use al_sem::engine::l5::registry::run_detectors;
use al_sem::program::model::program_calls::assemble_and_resolve_inline_program_default;

const APP_GUID: &str = "11111111-0000-0000-0000-0000000g13ab";

/// Run a single detector in isolation over an inline workspace.
fn run_detector(detector_name: &str, files: &[(String, String)]) -> Vec<Finding> {
    let resolved = assemble_and_resolve_inline_program_default(files, APP_GUID);
    let selected: Vec<_> = registered_detectors()
        .into_iter()
        .filter(|d| d.name == detector_name)
        .collect();
    assert_eq!(
        selected.len(),
        1,
        "{detector_name} must be registered exactly once"
    );
    run_detectors(&resolved, &selected).findings
}

fn al(name: &str, body: &str) -> (String, String) {
    (format!("src/{name}.al"), body.to_string())
}

// --- d10: self-modifying loop ------------------------------------------------

/// `repeat ... until Buf.Next() = 0` over a TEMPORARY record doing
/// `Buf.Delete()` on the iterating record → an in-memory cursor self-modify,
/// no SQL cursor to corrupt → NO d10 finding.
#[test]
fn d10_temp_iterating_record_suppressed() {
    let src = r#"
table 50131 "G13 Line"
{
    fields { field(1; "No."; Code[20]) { } field(2; Name; Text[100]) { } }
    keys { key(PK; "No.") { } }
}

codeunit 50131 "G13 D10 Temp"
{
    procedure PruneTemp()
    var
        Buf: Record "G13 Line" temporary;
    begin
        if Buf.FindSet() then
            repeat
                Buf.Delete();
            until Buf.Next() = 0;
    end;
}
"#;
    let findings = run_detector("d10-self-modifying-loop", &[al("G13D10Temp", src)]);
    assert!(
        findings.is_empty(),
        "d10 must NOT fire on a Known(true) TEMPORARY iterating record. findings: {:#?}",
        findings
            .iter()
            .map(|f| (&f.id, &f.root_cause))
            .collect::<Vec<_>>()
    );
}

/// CONTROL: the IDENTICAL loop over a PHYSICAL record → d10 STILL fires
/// (suppression-direction guard).
#[test]
fn d10_physical_iterating_record_still_fires() {
    let src = r#"
table 50132 "G13 Phys Line"
{
    fields { field(1; "No."; Code[20]) { } field(2; Name; Text[100]) { } }
    keys { key(PK; "No.") { } }
}

codeunit 50132 "G13 D10 Phys"
{
    procedure PrunePhysical()
    var
        Line: Record "G13 Phys Line";
    begin
        if Line.FindSet() then
            repeat
                Line.Delete();
            until Line.Next() = 0;
    end;
}
"#;
    let findings = run_detector("d10-self-modifying-loop", &[al("G13D10Phys", src)]);
    assert_eq!(
        findings.len(),
        1,
        "d10 must STILL fire on a PHYSICAL iterating record. findings: {:#?}",
        findings
            .iter()
            .map(|f| (&f.id, &f.root_cause))
            .collect::<Vec<_>>()
    );
}

// --- d39: record left dirty across helper chain --------------------------------

/// Caller forwards a TEMPORARY record by-var to a helper that Validates and
/// exits dirty; caller never persists → discarding in-memory state has no SQL
/// consequence → NO d39 finding.
#[test]
fn d39_temp_source_record_suppressed() {
    let src = r#"
table 50133 "G13 Cust"
{
    fields { field(1; "No."; Code[20]) { } field(2; Name; Text[100]) { } }
    keys { key(PK; "No.") { } }
}

codeunit 50133 "G13 D39 Temp"
{
    procedure ApplyName(var Cust: Record "G13 Cust")
    begin
        Cust.Validate(Name, 'X');
    end;

    procedure RunTemp()
    var
        TempCust: Record "G13 Cust" temporary;
    begin
        ApplyName(TempCust);
    end;
}
"#;
    let findings = run_detector(
        "d39-record-left-dirty-across-chain",
        &[al("G13D39Temp", src)],
    );
    assert!(
        findings.is_empty(),
        "d39 must NOT fire when the forwarded record is Known(true) TEMPORARY. findings: {:#?}",
        findings
            .iter()
            .map(|f| (&f.id, &f.root_cause))
            .collect::<Vec<_>>()
    );
}

/// CONTROL: the IDENTICAL chain with a PHYSICAL record → d39 STILL fires
/// (suppression-direction guard).
#[test]
fn d39_physical_source_record_still_fires() {
    let src = r#"
table 50134 "G13 Phys Cust"
{
    fields { field(1; "No."; Code[20]) { } field(2; Name; Text[100]) { } }
    keys { key(PK; "No.") { } }
}

codeunit 50134 "G13 D39 Phys"
{
    procedure ApplyName(var Cust: Record "G13 Phys Cust")
    begin
        Cust.Validate(Name, 'X');
    end;

    procedure RunPhysical()
    var
        Cust: Record "G13 Phys Cust";
    begin
        ApplyName(Cust);
    end;
}
"#;
    let findings = run_detector(
        "d39-record-left-dirty-across-chain",
        &[al("G13D39Phys", src)],
    );
    assert_eq!(
        findings.len(),
        1,
        "d39 must STILL fire when the forwarded record is PHYSICAL. findings: {:#?}",
        findings
            .iter()
            .map(|f| (&f.id, &f.root_cause))
            .collect::<Vec<_>>()
    );
}

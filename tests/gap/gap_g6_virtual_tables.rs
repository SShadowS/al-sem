//! Gap G-6 (docs/engine-gaps.md): BC system/virtual tables flagged as DB ops.
//!
//! `d1-db-op-in-loop` (and d4-repeated-lookup-in-loop) must NOT fire on reads of
//! BC VIRTUAL/system tables (`AllObjWithCaption`, `Field`, `Integer`, …) — these
//! have no physical SQL backing (they read the platform's in-memory metadata
//! store), so an in-loop read of one is never a SQL round-trip. The engine
//! previously marked them "type not loaded" and fired conservatively.
//!
//! Suppression signal (exact, structural): the op's type did NOT resolve to a
//! workspace table AND the receiving record variable's DECLARED type name is on
//! the `VIRTUAL_SYSTEM_TABLES` allowlist (exact name, case-insensitive).
//! Everything else keeps firing (control cases below): a loaded physical table,
//! and a NOT-loaded table whose name is not on the allowlist (the conservative
//! "type not loaded" behavior is preserved for normal tables).
//!
//! Drives the REAL detectors over inline AL workspaces (mirrors
//! `tests/gap_g9_trigger_rec.rs`).

use al_sem::engine::l5::detectors::registered_detectors;
use al_sem::engine::l5::finding::Finding;
use al_sem::engine::l5::registry::run_detectors;
use al_sem::program::model::program_calls::assemble_and_resolve_inline_program_default;

const APP_GUID: &str = "11111111-0000-0000-0000-0000000g6abc";

/// Run d1 + d4 over an inline workspace and return all emitted findings.
fn run_g6_detectors(files: &[(String, String)]) -> Vec<Finding> {
    let resolved = assemble_and_resolve_inline_program_default(files, APP_GUID);
    let wanted = ["d1-db-op-in-loop", "d4-repeated-lookup-in-loop"];
    let detectors: Vec<_> = registered_detectors()
        .into_iter()
        .filter(|d| wanted.contains(&d.name.as_str()))
        .collect();
    assert_eq!(
        detectors.len(),
        2,
        "d1/d4 must each be registered exactly once"
    );
    run_detectors(&resolved, &detectors).findings
}

fn al(name: &str, body: &str) -> (String, String) {
    (format!("src/{name}.al"), body.to_string())
}

/// A NORMAL physical table defined in the workspace (control).
const CUSTOMER_TABLE_SRC: &str = r#"
table 50160 "G6 Customer"
{
    fields
    {
        field(1; "No."; Code[20]) { }
        field(2; Name; Text[100]) { }
    }
    keys { key(PK; "No.") { } }
}
"#;

// --- (a) virtual-table reads in a loop → no d1 ---------------------------------

/// A `repeat` loop reading `AllObjWithCaption.Get(...)` and `Field.FindSet()` —
/// both are BC virtual tables with no SQL backing → NO d1 finding may be emitted.
#[test]
fn d1_skips_virtual_table_reads_in_loop() {
    let cu_src = r#"
codeunit 50160 "G6 Virtual Reader"
{
    procedure ScanMetadata()
    var
        AllObjWithCaption: Record AllObjWithCaption;
        FieldRec: Record "Field";
        I: Integer;
    begin
        I := 1;
        repeat
            AllObjWithCaption.Get(I);
            if FieldRec.FindSet() then;
            I := I + 1;
        until I > 100;
    end;
}
"#;
    let findings = run_g6_detectors(&[al("G6VirtualReader", cu_src)]);
    let d1: Vec<_> = findings
        .iter()
        .filter(|f| f.detector == "d1-db-op-in-loop")
        .collect();
    assert!(
        d1.is_empty(),
        "in-loop reads of BC virtual tables (AllObjWithCaption, Field) must not \
         fire d1 — they have no SQL backing. findings: {d1:#?}"
    );
}

/// The TRANSITIVE shape: a loop calls a helper whose only db op is a virtual-table
/// read. The interprocedural walk (`terminals_at`) must filter the virtual op too.
#[test]
fn d1_skips_virtual_table_reads_reached_through_in_loop_call() {
    let cu_src = r#"
codeunit 50161 "G6 Transitive Virtual"
{
    procedure Outer()
    var
        I: Integer;
    begin
        I := 1;
        repeat
            ReadMeta();
            I := I + 1;
        until I > 10;
    end;

    local procedure ReadMeta()
    var
        Obj: Record AllObjWithCaption;
    begin
        Obj.FindFirst();
    end;
}
"#;
    let findings = run_g6_detectors(&[al("G6TransitiveVirtual", cu_src)]);
    let d1: Vec<_> = findings
        .iter()
        .filter(|f| f.detector == "d1-db-op-in-loop")
        .collect();
    assert!(
        d1.is_empty(),
        "a virtual-table read reached through an in-loop call chain must not \
         fire d1. findings: {d1:#?}"
    );
}

// --- (b) CONTROL: the same loop shape on normal tables keeps firing -------------

/// The SAME loop shape reading a NORMAL physical table that IS loaded in the
/// workspace → d1 must STILL fire (suppression-direction guard).
#[test]
fn control_d1_fires_on_loaded_physical_table_in_loop() {
    let cu_src = r#"
codeunit 50162 "G6 Physical Reader"
{
    procedure ScanCustomers()
    var
        Cust: Record "G6 Customer";
        I: Integer;
    begin
        I := 1;
        repeat
            Cust.Get('X');
            I := I + 1;
        until I > 100;
    end;
}
"#;
    let findings = run_g6_detectors(&[
        al("G6Customer", CUSTOMER_TABLE_SRC),
        al("G6PhysicalReader", cu_src),
    ]);
    assert!(
        findings
            .iter()
            .any(|f| f.detector == "d1-db-op-in-loop" && f.root_cause.contains("ScanCustomers")),
        "d1 must still fire on an in-loop Get on a loaded physical table. \
         findings: {findings:#?}"
    );
}

/// A NOT-loaded table whose name is NOT on the virtual allowlist → the
/// conservative "type not loaded" behavior is preserved: d1 must STILL fire.
#[test]
fn control_d1_fires_on_unloaded_non_virtual_table_in_loop() {
    let cu_src = r#"
codeunit 50163 "G6 Unloaded Reader"
{
    procedure ScanVendors()
    var
        Vend: Record "Some Vendor";
        I: Integer;
    begin
        I := 1;
        repeat
            Vend.Get('X');
            I := I + 1;
        until I > 100;
    end;
}
"#;
    let findings = run_g6_detectors(&[al("G6UnloadedReader", cu_src)]);
    assert!(
        findings
            .iter()
            .any(|f| f.detector == "d1-db-op-in-loop" && f.root_cause.contains("ScanVendors")),
        "d1 must still fire (conservative) on an in-loop Get on a NOT-loaded table \
         that is not on the virtual allowlist. findings: {findings:#?}"
    );
}

// --- (c) d4: same gate, suppression + control ------------------------------------

/// Two identical literal-key `Get`s on a virtual table inside a loop → NO d4
/// (in-memory metadata lookup, nothing to hoist for SQL cost).
#[test]
fn d4_skips_repeated_virtual_table_lookup_in_loop() {
    let cu_src = r#"
codeunit 50164 "G6 Repeated Virtual"
{
    procedure RepeatVirtual()
    var
        Obj: Record AllObjWithCaption;
        I: Integer;
    begin
        I := 1;
        repeat
            Obj.Get('X');
            Obj.Get('X');
            I := I + 1;
        until I > 10;
    end;
}
"#;
    let findings = run_g6_detectors(&[al("G6RepeatedVirtual", cu_src)]);
    let d4: Vec<_> = findings
        .iter()
        .filter(|f| f.detector == "d4-repeated-lookup-in-loop")
        .collect();
    assert!(
        d4.is_empty(),
        "repeated lookups on a BC virtual table must not fire d4. findings: {d4:#?}"
    );
}

/// CONTROL: the same repeated-lookup shape on a NOT-loaded NORMAL table → d4
/// must STILL fire.
#[test]
fn control_d4_fires_on_repeated_normal_table_lookup_in_loop() {
    let cu_src = r#"
codeunit 50165 "G6 Repeated Physical"
{
    procedure RepeatPhysical()
    var
        Vend: Record "Some Vendor";
        I: Integer;
    begin
        I := 1;
        repeat
            Vend.Get('X');
            Vend.Get('X');
            I := I + 1;
        until I > 10;
    end;
}
"#;
    let findings = run_g6_detectors(&[al("G6RepeatedPhysical", cu_src)]);
    assert!(
        findings
            .iter()
            .any(|f| f.detector == "d4-repeated-lookup-in-loop"
                && f.root_cause.contains("RepeatPhysical")),
        "d4 must still fire on repeated identical lookups on a normal table. \
         findings: {findings:#?}"
    );
}

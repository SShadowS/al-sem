//! Gap G-2 (docs/engine-gaps.md): runtime-implied tempness not inferred.
//!
//! Records that are temporary *by runtime contract* but not by structural
//! declaration stayed `Unknown` → detectors fired (false positives). Two shapes:
//!
//! 1. **Self-guarding temp table** (Part 1): a table whose OnInsert/OnModify/
//!    OnDelete/OnRename trigger contains a top-level
//!    `if not Rec.IsTemporary[()] then Error(...)` guard — every instance is
//!    provably temporary (the trigger errors at runtime otherwise). The table is
//!    marked `is_temporary` by contract; the existing table-level override pass
//!    then forces `Known(true)` on every op on that table.
//!
//! 2. **Entry-guard temp routine** (Part 2): a routine whose FIRST executable
//!    statement is `if not <X>.IsTemporary[()] then Error(...)` where `<X>` is a
//!    record var/param — within that routine `<X>` is provably temporary →
//!    `Known(true)` on `<X>`'s ops.
//!
//! SOUNDNESS / suppression direction: only the EXACT negated-IsTemporary-guards-
//! Error shape upgrades to `Known(true)`. Any deviation — guard not the first
//! statement (Part 2), no Error then-branch, non-negated condition — leaves the
//! state untouched → detectors keep firing (controls below).
//!
//! Asserts both the L3 resolution (like tests/temp_state_tabletype.rs) and the
//! real detector pipeline (d1 / d33, like tests/gap_g6_virtual_tables.rs).

use al_sem::engine::l5::detectors::registered_detectors;
use al_sem::engine::l5::finding::Finding;
use al_sem::engine::l5::registry::run_detectors;
use al_sem::program::model::program_calls::assemble_and_resolve_inline_program_default;

const APP_GUID: &str = "11111111-0000-0000-0000-0000000g2abc";

/// Run the named detectors over an inline workspace and return all findings.
fn run_gap_detectors(files: &[(String, String)], wanted: &[&str]) -> Vec<Finding> {
    let resolved = assemble_and_resolve_inline_program_default(files, APP_GUID);
    let detectors: Vec<_> = registered_detectors()
        .into_iter()
        .filter(|d| wanted.contains(&d.name.as_str()))
        .collect();
    assert_eq!(
        detectors.len(),
        wanted.len(),
        "each wanted detector must be registered exactly once"
    );
    run_detectors(&resolved, &detectors).findings
}

fn al(name: &str, body: &str) -> (String, String) {
    (format!("src/{name}.al"), body.to_string())
}

// =============================================================================
// Part 1 — self-guarding temp table (table contract)
// =============================================================================

/// A table whose OnInsert errors unless the instance is temporary — the
/// `CDO File` shape. Every instance is temp by contract.
const GUARDED_TABLE_SRC: &str = r#"
table 50990 "G2 Guarded Buffer"
{
    fields { field(1; Id; Integer) { } }

    trigger OnInsert()
    begin
        if not Rec.IsTemporary() then
            Error('must be temp');
    end;
}
"#;

/// Same shape WITHOUT the parens on IsTemporary (property-style call), guarded
/// in OnDelete instead of OnInsert.
const GUARDED_TABLE_NO_PARENS_SRC: &str = r#"
table 50994 "G2 Guarded NoParens"
{
    fields { field(1; Id; Integer) { } }

    trigger OnDelete()
    begin
        if not Rec.IsTemporary then
            Error('must be temp');
    end;
}
"#;

/// CONTROL: a normal table with NO guard.
const PLAIN_TABLE_SRC: &str = r#"
table 50991 "G2 Plain Buffer"
{
    fields { field(1; Id; Integer) { } }
}
"#;

/// CONTROL: a table whose OnInsert has an IsTemporary if-statement that is NOT
/// the guard shape (non-negated condition, no Error in the negated sense).
const NON_GUARD_TABLE_SRC: &str = r#"
table 50995 "G2 NonGuard Buffer"
{
    fields { field(1; Id; Integer) { } }

    trigger OnInsert()
    begin
        if Rec.IsTemporary() then
            Error('must NOT be temp');
    end;
}
"#;

/// A codeunit doing a db-op-in-loop on a plain (no `temporary` keyword) local
/// record var of the given table.
fn loop_insert_codeunit(number: u32, table: &str) -> String {
    format!(
        r#"
codeunit {number} "G2 Probe {number}"
{{
    procedure LoopInsert()
    var
        Buf: Record "{table}";
        I: Integer;
    begin
        I := 0;
        repeat
            Buf.Id := I;
            Buf.Insert();
            I := I + 1;
        until I > 10;
    end;
}}
"#
    )
}

#[test]
fn guarded_table_is_temporary_by_contract_and_ops_resolve_known_true() {
    let files = [
        al("G2GuardedBuffer", GUARDED_TABLE_SRC),
        al("G2GuardedNoParens", GUARDED_TABLE_NO_PARENS_SRC),
        al("G2Probe", &loop_insert_codeunit(50992, "G2 Guarded Buffer")),
    ];
    let resolved = assemble_and_resolve_inline_program_default(&files, APP_GUID);

    // The guard marks the table temporary by contract (both call shapes).
    let table = resolved
        .table_by_name("G2 Guarded Buffer")
        .expect("guarded table must be indexed");
    assert!(
        table.is_temporary(),
        "an OnInsert `if not Rec.IsTemporary() then Error(...)` guard must mark \
         the table is_temporary (runtime contract — every instance is temp)",
    );
    let no_parens = resolved
        .table_by_name("G2 Guarded NoParens")
        .expect("no-parens guarded table must be indexed");
    assert!(
        no_parens.is_temporary(),
        "the paren-less `if not Rec.IsTemporary then Error(...)` guard (OnDelete) \
         must also mark the table is_temporary",
    );

    // The existing table-level override then upgrades ops on plain vars.
    let routine = resolved
        .routine_by_name("LoopInsert")
        .expect("LoopInsert must be resolved");
    assert_eq!(
        routine.first_record_op_temp_known("Buf"),
        Some(true),
        "`Buf.Insert()` on a self-guarding temp table must resolve Known(true)",
    );
    assert_eq!(
        routine.record_var_temp_known("Buf"),
        Some(true),
        "the record var of a self-guarding temp table must report Known(true)",
    );
}

#[test]
fn d1_suppressed_on_guarded_table_loop_insert() {
    let files = [
        al("G2GuardedBuffer", GUARDED_TABLE_SRC),
        al("G2Probe", &loop_insert_codeunit(50992, "G2 Guarded Buffer")),
    ];
    // d1 downgrades temp ops (TempVerdict::Temporary) to "info" — the guarded
    // table's in-loop Insert must NOT fire at the physical-table severity.
    let findings = run_gap_detectors(&files, &["d1-db-op-in-loop"]);
    let non_info: Vec<_> = findings.iter().filter(|f| f.severity != "info").collect();
    assert!(
        non_info.is_empty(),
        "d1 must downgrade an in-loop Insert into a self-guarding temp table to \
         info (runtime contract proves tempness); got: {:?}",
        non_info
            .iter()
            .map(|f| (&f.severity, &f.title))
            .collect::<Vec<_>>()
    );
}

#[test]
fn control_plain_table_stays_physical_and_d1_fires() {
    let files = [
        al("G2PlainBuffer", PLAIN_TABLE_SRC),
        al("G2Probe", &loop_insert_codeunit(50992, "G2 Plain Buffer")),
    ];
    let resolved = assemble_and_resolve_inline_program_default(&files, APP_GUID);
    let table = resolved
        .table_by_name("G2 Plain Buffer")
        .expect("plain table must be indexed");
    assert!(
        !table.is_temporary(),
        "a table without the guard must stay is_temporary == false",
    );
    let routine = resolved
        .routine_by_name("LoopInsert")
        .expect("LoopInsert must be resolved");
    assert_ne!(
        routine.first_record_op_temp_known("Buf"),
        Some(true),
        "ops on a plain table must NOT be upgraded to Known(true)",
    );

    let findings = run_gap_detectors(&files, &["d1-db-op-in-loop"]);
    assert!(
        findings.iter().any(|f| f.severity != "info"),
        "d1 must keep firing (above info) on an in-loop Insert into a normal \
         physical table",
    );
}

#[test]
fn control_non_negated_istemporary_trigger_is_not_a_guard() {
    let files = [
        al("G2NonGuardBuffer", NON_GUARD_TABLE_SRC),
        al(
            "G2Probe",
            &loop_insert_codeunit(50992, "G2 NonGuard Buffer"),
        ),
    ];
    let resolved = assemble_and_resolve_inline_program_default(&files, APP_GUID);
    let table = resolved
        .table_by_name("G2 NonGuard Buffer")
        .expect("non-guard table must be indexed");
    assert!(
        !table.is_temporary(),
        "a NON-negated `if Rec.IsTemporary() then Error(...)` (errors when temp!) \
         must NOT mark the table temporary — only the exact negated guard shape",
    );

    let findings = run_gap_detectors(&files, &["d1-db-op-in-loop"]);
    assert!(
        findings.iter().any(|f| f.severity != "info"),
        "d1 must keep firing (above info) — the non-negated shape proves the \
         OPPOSITE contract",
    );
}

// =============================================================================
// Part 2 — entry-guard temp routine
// =============================================================================

/// Routines over a plain physical table: one with the entry guard (provably
/// temp), one without (control), one with the guard NOT first (control), one
/// with a non-Error then-branch (control).
const ROUTINES_SRC: &str = r#"
table 50991 "G2 Plain Buffer"
{
    fields { field(1; Id; Integer) { } }
}

codeunit 50993 "G2 Routines"
{
    procedure GuardedDelete(var Buf: Record "G2 Plain Buffer")
    begin
        if not Buf.IsTemporary() then
            Error('temp only');
        Buf.DeleteAll();
    end;

    procedure UnguardedDelete(var Buf: Record "G2 Plain Buffer")
    begin
        Buf.DeleteAll();
    end;

    procedure LateGuardDelete(var Buf: Record "G2 Plain Buffer")
    var
        I: Integer;
    begin
        I := 0;
        if not Buf.IsTemporary() then
            Error('temp only');
        Buf.DeleteAll();
    end;

    procedure ExitGuardDelete(var Buf: Record "G2 Plain Buffer")
    begin
        if not Buf.IsTemporary() then
            exit;
        Buf.DeleteAll();
    end;
}
"#;

#[test]
fn entry_guarded_routine_param_ops_resolve_known_true() {
    let files = [al("G2Routines", ROUTINES_SRC)];
    let resolved = assemble_and_resolve_inline_program_default(&files, APP_GUID);

    let guarded = resolved
        .routine_by_name("GuardedDelete")
        .expect("GuardedDelete must be resolved");
    assert_eq!(
        guarded.first_record_op_temp_known("Buf"),
        Some(true),
        "`Buf.DeleteAll()` after the entry guard `if not Buf.IsTemporary() then \
         Error(...)` must resolve Known(true) — the guard proves tempness",
    );
    assert_eq!(
        guarded.record_var_temp_known("Buf"),
        Some(true),
        "the guarded by-var param record var must report Known(true)",
    );
}

#[test]
fn control_unguarded_routine_param_stays_unproven() {
    let files = [al("G2Routines", ROUTINES_SRC)];
    let resolved = assemble_and_resolve_inline_program_default(&files, APP_GUID);

    let unguarded = resolved
        .routine_by_name("UnguardedDelete")
        .expect("UnguardedDelete must be resolved");
    assert_ne!(
        unguarded.first_record_op_temp_known("Buf"),
        Some(true),
        "without the entry guard a by-var param's op must NOT resolve Known(true)",
    );
}

#[test]
fn control_guard_not_first_statement_is_not_proven() {
    let files = [al("G2Routines", ROUTINES_SRC)];
    let resolved = assemble_and_resolve_inline_program_default(&files, APP_GUID);

    let late = resolved
        .routine_by_name("LateGuardDelete")
        .expect("LateGuardDelete must be resolved");
    assert_ne!(
        late.first_record_op_temp_known("Buf"),
        Some(true),
        "a guard that is NOT the routine's first executable statement must NOT \
         upgrade — something ran before it (conservative)",
    );
    assert_ne!(
        late.record_var_temp_known("Buf"),
        Some(true),
        "the late-guard record var must stay unproven",
    );
}

#[test]
fn control_non_error_then_branch_is_not_proven() {
    let files = [al("G2Routines", ROUTINES_SRC)];
    let resolved = assemble_and_resolve_inline_program_default(&files, APP_GUID);

    let exit_guard = resolved
        .routine_by_name("ExitGuardDelete")
        .expect("ExitGuardDelete must be resolved");
    assert_ne!(
        exit_guard.first_record_op_temp_known("Buf"),
        Some(true),
        "`if not Buf.IsTemporary() then exit;` does NOT prove tempness (the body \
         still runs for temp records only — but exit is not the Error contract \
         shape; conservative: leave unproven)",
    );
}

/// Detector-level Part 2: d33 on a GLOBAL record var (d33 skips by-var params
/// structurally, so the global is the shape that actually fires). The entry
/// guard must suppress it; the unguarded twin codeunit keeps firing.
#[test]
fn d33_suppressed_on_entry_guarded_global_but_fires_unguarded() {
    let guarded_src = r#"
codeunit 50996 "G2 Guarded Global"
{
    var
        Buf: Record "G2 Plain Buffer";

    procedure ClearAll()
    begin
        if not Buf.IsTemporary() then
            Error('temp only');
        Buf.DeleteAll();
    end;
}
"#;
    let unguarded_src = r#"
codeunit 50997 "G2 Unguarded Global"
{
    var
        Buf: Record "G2 Plain Buffer";

    procedure ClearAll()
    begin
        Buf.DeleteAll();
    end;
}
"#;
    let files = [
        al("G2PlainBuffer", PLAIN_TABLE_SRC),
        al("G2GuardedGlobal", guarded_src),
        al("G2UnguardedGlobal", unguarded_src),
    ];
    let findings = run_gap_detectors(&files, &["d33-unfiltered-bulk-write"]);
    let in_guarded: Vec<_> = findings
        .iter()
        .filter(|f| {
            f.primary_location
                .source_unit_id
                .contains("G2GuardedGlobal")
        })
        .collect();
    assert!(
        in_guarded.is_empty(),
        "no d33 finding may point at the entry-guarded codeunit; got: {:?}",
        in_guarded.iter().map(|f| &f.root_cause).collect::<Vec<_>>()
    );
    let in_unguarded: Vec<_> = findings
        .iter()
        .filter(|f| {
            f.primary_location
                .source_unit_id
                .contains("G2UnguardedGlobal")
        })
        .collect();
    assert_eq!(
        in_unguarded.len(),
        1,
        "the UNGUARDED codeunit's unfiltered global DeleteAll must keep firing; \
         all findings: {:?}",
        findings.iter().map(|f| &f.root_cause).collect::<Vec<_>>()
    );
}

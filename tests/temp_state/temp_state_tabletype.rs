//! Task 4 (temp-state) — native `TableType = Temporary` capture + the
//! TABLE-LEVEL OVERRIDE PRECEDENCE RULE.
//!
//! # The one precedence rule everywhere
//!
//! A table declared `TableType = Temporary` is temp REGARDLESS of how a record
//! var of that table is declared (keyword / no-keyword / by-value / by-var) and
//! REGARDLESS of any `ParameterDependent(i)` stamped at L2. After an op's
//! `table_id` is fully resolved, a temp-table op (and its matching record var)
//! is force-upgraded to `Known(true)`.
//!
//! This is purely ADDITIVE toward `Known(true)` — it never downgrades a
//! `Known(true)` to false, and never forces `Known(false)`. The only signal is
//! the structural `TableType` property (Part A), so the upgrade is sound.

use al_sem::program::model::program_calls::assemble_and_resolve_inline_program_default;

const APP_GUID: &str = "2a000000-0000-0000-0000-0000000002aa";

/// (a) A `table 50800 "ThatTable" { TableType = Temporary; ... }` plus a codeunit
/// with a LOCAL `Rec: Record "ThatTable";` (NO `temporary` keyword) doing
/// `Rec.DeleteAll();`. The op must resolve `Known(true)` — the table-level
/// override beats the ABSENT keyword — and `L3Table.is_temporary == true`.
#[test]
fn tabletype_temporary_table_is_marked_and_local_var_op_upgraded() {
    let source = r#"
table 50800 "ThatTable"
{
    TableType = Temporary;
    fields { field(1; Id; Integer) { } }
}

codeunit 50801 "Probe"
{
    procedure P()
    var
        Rec: Record "ThatTable";
    begin
        Rec.DeleteAll();
    end;
}
"#;

    let resolved = assemble_and_resolve_inline_program_default(
        &[("src/main.al".to_string(), source.to_string())],
        APP_GUID,
    );

    // Part A: the table is structurally temporary.
    let table = resolved
        .table_by_name("ThatTable")
        .expect("ThatTable must be indexed");
    assert!(
        table.is_temporary(),
        "TableType = Temporary must set L3Table.is_temporary == true",
    );

    // Part B: the op on a NO-keyword local var of that table is force-upgraded.
    let routine = resolved
        .routine_by_name("P")
        .expect("P routine must be resolved");
    assert_eq!(
        routine.first_record_op_temp_known("Rec"),
        Some(true),
        "`Rec.DeleteAll()` on a TableType=Temporary table must resolve Known(true) \
         (table-level override beats the absent `temporary` keyword)",
    );
    // The record VARIABLE is upgraded too, for consistency.
    assert_eq!(
        routine.record_var_temp_known("Rec"),
        Some(true),
        "the record var of a temp table must also report Known(true)",
    );
}

/// (b) A by-var PARAM `procedure P(var Rec: Record "ThatTable")` (NO keyword) +
/// `Rec.DeleteAll();`. At L2 a by-var param with no keyword is stamped
/// `ParameterDependent(i)`; the table-level override supersedes it → `Known(true)`
/// at L3 (RV-8). A NON-temp table's plain var must stay `Known(false)` (control —
/// no false upgrade).
#[test]
fn tabletype_temporary_by_var_param_upgraded_nontemp_control_unchanged() {
    let source = r#"
table 50800 "ThatTable"
{
    TableType = Temporary;
    fields { field(1; Id; Integer) { } }
}

table 50802 "PhysTable"
{
    fields { field(1; Id; Integer) { } }
}

codeunit 50801 "Probe"
{
    procedure ByVarTemp(var Rec: Record "ThatTable")
    begin
        Rec.DeleteAll();
    end;

    procedure PlainPhys()
    var
        Phys: Record "PhysTable";
    begin
        Phys.DeleteAll();
    end;
}
"#;

    let resolved = assemble_and_resolve_inline_program_default(
        &[("src/main.al".to_string(), source.to_string())],
        APP_GUID,
    );

    // By-var param of a temp table → Known(true) (override beats the L2 PD(i)).
    let by_var = resolved
        .routine_by_name("ByVarTemp")
        .expect("ByVarTemp routine must be resolved");
    assert_eq!(
        by_var.first_record_op_temp_known("Rec"),
        Some(true),
        "a by-var PARAM of a TableType=Temporary table must resolve Known(true) at L3 \
         — the table-level override supersedes the PD(i) stamped at L2 (RV-8)",
    );
    assert_eq!(
        by_var.record_var_temp_known("Rec"),
        Some(true),
        "the by-var param record var of a temp table must also report Known(true)",
    );

    // Control: a plain physical-table var stays Known(false) — no false upgrade.
    let plain = resolved
        .routine_by_name("PlainPhys")
        .expect("PlainPhys routine must be resolved");
    assert_eq!(
        plain.first_record_op_temp_known("Phys"),
        Some(false),
        "a plain physical-table var must NOT be upgraded — the override only ever \
         adds Known(true) on temp tables, never downgrades or false-upgrades",
    );
    let phys = resolved
        .table_by_name("PhysTable")
        .expect("PhysTable must be indexed");
    assert!(
        !phys.is_temporary(),
        "a table with no TableType=Temporary must keep is_temporary == false",
    );
}

//! Task 3 (temp-state) — object-global record-var PROMOTION + SHADOWING.
//!
//! # What is being fixed (the CDO false-critical class)
//!
//! A codeunit MEMBER (object-global) record var such as
//! `Files: Record "CDO File" temporary;` was never seen by the L2 body walk
//! (which only extracts a routine's params + locals). So a member-var op like
//! `Files.DeleteAll()` carried `tempState = Unknown`, firing a false critical and
//! causing d1 to stamp "(temp state uncertain)".
//!
//! Task 3 promotes object-global RECORD vars into EACH routine's
//! `record_variables` (re-keyed per routine, keeping `scope: Some("global")` and
//! the captured `Known(true/false)` temp signal), then the L3 record-type pass
//! re-derives `op.temp_state` from the matched (now complete) record var.
//!
//! # Shadowing (load-bearing for soundness)
//!
//! A routine's OWN param/local of the same name SHADOWS the global (innermost
//! wins). The promotion only adds globals whose name is NOT already an own record
//! var, keeping `record_variables` NAME-UNIQUE so the record-type pass-1
//! last-wins index resolves each name to the single (innermost) declaration.

use al_sem::program::model::program_calls::assemble_and_resolve_inline_program_default;

const APP_GUID: &str = "2a000000-0000-0000-0000-0000000002aa";

/// (a) Object-global `Buf: Record "Bar" temporary;`, a procedure with NO local
/// `Buf` doing `Buf.DeleteAll();`. After L3 resolution the routine's
/// `record_variables` must contain a promoted `buf` with `scope == Some("global")`
/// and Known(true), AND the op on `buf` must resolve `temp_state` Known(true).
#[test]
fn global_temp_record_promoted_into_routine_and_op_resolves_known_true() {
    let source = r#"
table 50900 Bar
{
    fields { field(1; "No."; Code[20]) { } }
}

codeunit 50902 "CdoProbe"
{
    var
        Buf: Record "Bar" temporary;

    procedure Clear()
    begin
        Buf.DeleteAll();
    end;
}
"#;

    let resolved = assemble_and_resolve_inline_program_default(
        &[("src/main.al".to_string(), source.to_string())],
        APP_GUID,
    );

    let routine = resolved
        .routine_by_name("Clear")
        .expect("Clear routine must be resolved");

    // The object-global record var must have been promoted into the routine.
    assert_eq!(
        routine.record_var_scope("Buf").as_deref(),
        Some("global"),
        "object-global `Buf: Record \"Bar\" temporary` must be promoted into the \
         routine's record_variables with scope == global",
    );
    assert_eq!(
        routine.record_var_temp_known("Buf"),
        Some(true),
        "the promoted global `Buf` must keep its captured Known(true) temp_state",
    );

    // The member-var OP must resolve its temp_state from the promoted global —
    // the CDO false-critical fix (was Unknown before promotion).
    assert_eq!(
        routine.first_record_op_temp_known("Buf"),
        Some(true),
        "`Buf.DeleteAll()` must resolve temp_state Known(true) from the promoted \
         object-global temporary record var (the CDO false-critical root-cause fix)",
    );
}

/// (b) Shadowing: a SECOND procedure declares a LOCAL `Buf: Record "Baz";`
/// (physical, different table) and does `Buf.DeleteAll();`. THAT routine's `buf`
/// op must resolve Known(false) (the local shadows the temp global) and the table
/// must be Baz's.
#[test]
fn local_record_shadows_global_temp_and_op_resolves_known_false() {
    let source = r#"
table 50900 Bar
{
    fields { field(1; "No."; Code[20]) { } }
}

table 50901 Baz
{
    fields { field(1; "No."; Code[20]) { } }
}

codeunit 50902 "CdoProbe"
{
    var
        Buf: Record "Bar" temporary;

    procedure ClearGlobal()
    begin
        Buf.DeleteAll();
    end;

    procedure ClearLocal()
    var
        Buf: Record "Baz";
    begin
        Buf.DeleteAll();
    end;
}
"#;

    let resolved = assemble_and_resolve_inline_program_default(
        &[("src/main.al".to_string(), source.to_string())],
        APP_GUID,
    );

    // The global-using routine still resolves the temp global → Known(true).
    let global_routine = resolved
        .routine_by_name("ClearGlobal")
        .expect("ClearGlobal routine must be resolved");
    assert_eq!(
        global_routine.first_record_op_temp_known("Buf"),
        Some(true),
        "ClearGlobal has no own `Buf`, so it resolves the promoted temp global Known(true)",
    );

    // The local-declaring routine: its OWN physical `Buf: Record Baz` shadows the
    // temp global → Known(false), and the table is Baz's.
    let local_routine = resolved
        .routine_by_name("ClearLocal")
        .expect("ClearLocal routine must be resolved");

    // The own LOCAL record var carries scope == None (the L2 projection only
    // tags promoted globals as "global"); crucially it must NOT be "global",
    // proving the temp global was shadowed out and not promoted into this routine.
    assert_ne!(
        local_routine.record_var_scope("Buf").as_deref(),
        Some("global"),
        "ClearLocal's own LOCAL `Buf` must win over the promoted global (innermost wins); \
         the shadowed temp global must NOT be promoted into this routine",
    );
    assert_eq!(
        local_routine.first_record_op_temp_known("Buf"),
        Some(false),
        "`Buf.DeleteAll()` in ClearLocal must resolve Known(false) — the LOCAL physical \
         `Buf: Record Baz` shadows the object-global temporary `Buf`",
    );

    let ops = local_routine.record_ops();
    let buf_op = ops
        .iter()
        .find(|(_, var, _)| var.eq_ignore_ascii_case("buf"))
        .expect("DeleteAll op on Buf must be present in ClearLocal");
    assert_eq!(
        buf_op.2,
        Some(format!("{APP_GUID}:Table:50901")),
        "ClearLocal's `Buf` op must resolve to Baz (50901), the LOCAL declaration's table",
    );
}

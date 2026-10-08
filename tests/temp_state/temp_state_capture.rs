//! Task 2 (ts2) — unit-test `extract_object_global_record_vars`.
//!
//! # What is tested
//!
//! `extract_object_global_record_vars` scans an object-level `var_section` for
//! record-typed variable declarations and captures the `temporary_keyword` flag.
//! Two fixture variables in a codeunit:
//!
//! - `Buf: Record "Bar" temporary;` → `temp_state` Kind=known, value=Some(true),
//!   `scope` = Some("global")
//! - `Phys: Record "Bar";`          → `temp_state` Kind=known, value=Some(false),
//!   `scope` = Some("global")
//!
//! The test invokes the function DIRECTLY on a tree-sitter-parsed object node so
//! it is independent of the L3 wiring (Task 3).

use al_sem::program::body::features::PRecordVariable;
use al_sem::program::body::ir_walk::ir_object_global_record_vars;

const OBJECT_ID: &str = "test-app-guid/codeunit/50001";

const SOURCE: &str = r#"
codeunit 50001 "GlobalRecordProbe"
{
    var
        Buf: Record "Bar" temporary;
        Phys: Record "Bar";
        Counter: Integer;

    procedure DoWork()
    begin
    end;
}
"#;

/// Project the object-global record variables of the FIRST object in `source`
/// from the owned IR (the IR analogue of the retired tree-sitter extractor).
fn global_record_vars(source: &str) -> Vec<PRecordVariable> {
    let file = al_syntax::parse(source);
    let o = file.objects.first().expect("one object in source");
    ir_object_global_record_vars(o, OBJECT_ID)
}

#[test]
fn buf_is_temporary_phys_is_not() {
    let vars = global_record_vars(SOURCE);

    assert_eq!(
        vars.len(),
        2,
        "expected exactly 2 record variables (Counter: Integer is skipped); got {:?}",
        vars.iter().map(|v| &v.name).collect::<Vec<_>>()
    );

    // Find by name (case-insensitive; the function lowercases names).
    let buf = vars
        .iter()
        .find(|v| v.name.eq_ignore_ascii_case("buf"))
        .expect("Buf record variable present");

    let phys = vars
        .iter()
        .find(|v| v.name.eq_ignore_ascii_case("phys"))
        .expect("Phys record variable present");

    // --- Buf: Record "Bar" temporary ---
    assert_eq!(
        buf.temp_state.kind, "known",
        "Buf temp_state.kind must be 'known'"
    );
    assert_eq!(
        buf.temp_state.value,
        Some(true),
        "Buf temp_state.value must be Some(true)"
    );
    assert_eq!(
        buf.temp_state.parameter_index, None,
        "Buf temp_state.parameter_index must be None"
    );
    assert_eq!(
        buf.scope,
        Some("global".to_string()),
        "Buf scope must be Some(\"global\")"
    );
    assert!(!buf.is_parameter, "Buf is_parameter must be false");
    assert_eq!(
        buf.parameter_index, None,
        "Buf parameter_index must be None"
    );
    assert_eq!(
        buf.table_name.as_deref(),
        Some("Bar"),
        "Buf table_name must be Some(\"Bar\")"
    );

    // --- Phys: Record "Bar" ---
    assert_eq!(
        phys.temp_state.kind, "known",
        "Phys temp_state.kind must be 'known'"
    );
    assert_eq!(
        phys.temp_state.value,
        Some(false),
        "Phys temp_state.value must be Some(false)"
    );
    assert_eq!(
        phys.scope,
        Some("global".to_string()),
        "Phys scope must be Some(\"global\")"
    );

    // --- id format: {object_id}/grv/{lc_name} ---
    assert_eq!(
        buf.id,
        format!("{}/grv/buf", OBJECT_ID),
        "Buf id must use /grv/ prefix"
    );
    assert_eq!(
        phys.id,
        format!("{}/grv/phys", OBJECT_ID),
        "Phys id must use /grv/ prefix"
    );
}

/// Non-record globals (e.g. Integer) must be silently skipped.
#[test]
fn non_record_globals_are_skipped() {
    let vars = global_record_vars(SOURCE);

    let has_counter = vars.iter().any(|v| v.name.eq_ignore_ascii_case("counter"));
    assert!(
        !has_counter,
        "Counter: Integer must NOT appear in the record-variable list"
    );
}

/// Quoted variable names have their quotes stripped; table names from quoted
/// identifiers are also stripped.
#[test]
fn quoted_name_strips_correctly() {
    let source = r#"
codeunit 50002 "QuotedProbe"
{
    var
        "My Buf": Record "Some Table" temporary;
}
"#;
    let vars = global_record_vars(source);

    assert_eq!(vars.len(), 1, "one quoted-name record variable");
    let v = &vars[0];
    assert_eq!(
        v.name, "My Buf",
        "quoted variable name must be stripped of double quotes"
    );
    assert_eq!(
        v.table_name.as_deref(),
        Some("Some Table"),
        "quoted table name must be stripped"
    );
    assert_eq!(
        v.temp_state.value,
        Some(true),
        "temporary keyword captured for quoted-name var"
    );
}

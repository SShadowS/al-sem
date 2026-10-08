//! RV-5 prerequisite — first-wins shadowing for the `variable_decl_by_name`
//! lookup in `resolve_routine_record_types` (pass 2b).
//!
//! # What is being tested
//!
//! `resolve_routine_record_types` (pass 2b) builds a `variable_decl_by_name`
//! HashMap from `routine.variables` (ordered: params → locals → globals).
//! With an unconditional `.insert()` (last-wins), the GLOBAL `Foo: Record Bar`
//! inserted AFTER the LOCAL `Foo: Record Baz` would overwrite it — a silent
//! suppression vector once Task 3 promotes globals into routines.
//!
//! The fix changes the insertion to `.entry(k).or_insert(v)` (first-wins),
//! making the innermost (local) declaration win on a name collision.
//!
//! # Test strategy
//!
//! Two tests:
//!
//! 1. `pass_2b_first_wins_on_name_collision` — a LOW-LEVEL UNIT TEST that
//!    directly constructs an `L3Routine` with a manually-injected collision in
//!    `variables` (local "Baz" first, global "Bar" second, `record_variables`
//!    empty so passes 1/2a do NOT resolve the op — only pass 2b fires). This
//!    test FAILS before the fix (last-wins picks "Bar") and PASSES after
//!    (first-wins picks "Baz").
//!
//! 2. `integration_local_shadows_global_end_to_end` — an end-to-end test
//!    through `assemble_and_resolve_inline_program_default`. This passes both before AND after
//!    the fix because `extract_variables` already deduplicates the global
//!    variable when a local has the same name. It serves as a regression guard
//!    confirming the full-pipeline behavior is correct.

use al_sem::engine::l2::features::PAnchor;
use al_sem::engine::l3::l3_workspace::{
    L3RecordOperation, L3Routine, L3Table, L3Variable, RoutineVariables,
};
use al_sem::engine::l3::record_types::resolve_routine_record_types;
use al_sem::engine::l3::symbol_table::SymbolTable;
use al_sem::program::model::program_calls::assemble_and_resolve_inline_program_default;

const APP_GUID: &str = "2a000000-0000-0000-0000-0000000002aa";

/// The INTERNAL table id format used by the L3 engine: `{appGuid}/table/{n}`.
/// This is what `op.table_id` carries after resolution (NOT the StableTableId
/// projected for test/dump surfaces, which uses `:Table:` separators).
fn internal_table_id(number: i64) -> String {
    format!("{APP_GUID}/table/{number}")
}

/// The STABLE table id format projected for test/golden surfaces.
fn stable_table(number: i64) -> String {
    format!("{APP_GUID}:Table:{number}")
}

/// Helper: a dummy PAnchor (all zeroes) for routine/op construction.
fn dummy_anchor() -> PAnchor {
    PAnchor {
        source_unit_id: "ws:src/main.al".to_string(),
        start_line: 0,
        start_column: 0,
        end_line: 0,
        end_column: 0,
        syntax_kind: "procedure".to_string(),
    }
}

/// Build a minimal L3Table in-workspace (app_guid/table/{number}).
fn make_table(name: &str, number: i64) -> L3Table {
    L3Table {
        id: format!("{APP_GUID}/table/{number}"),
        app_guid: APP_GUID.to_string(),
        table_number: number,
        name: name.to_string(),
        fields: Vec::new(),
        keys: Vec::new(),
        is_temporary: false,
        is_extension_stub: false,
    }
}

/// Build a bare-minimum L3Routine suitable for directly exercising pass 2b.
/// `record_variables` is intentionally empty so that passes 1 and 2a do NOT
/// set the op's tableId — only pass 2b is in play.
fn make_routine_for_pass2b(variables: Vec<L3Variable>) -> L3Routine {
    // One record op on "Foo" with no tableId pre-set.
    let op = L3RecordOperation {
        id: "op0".to_string(),
        op: "FindSet".to_string(),
        record_variable_name: "Foo".to_string(),
        record_variable_id: None,
        table_id: None,
        temp_state: None,
        field_arguments: None,
        source_anchor: dummy_anchor(),
        loop_stack: Vec::new(),
        field_argument_infos: None,
        in_until_condition: false,
        run_trigger: None,
    };

    L3Routine {
        id: "r0/test-routine".to_string(),
        stable_routine_id: "test-stable-id".to_string(),
        object_id: "test-obj".to_string(),
        object_type: "Codeunit".to_string(),
        name: "DoWork".to_string(),
        kind: "procedure".to_string(),
        attributes_parsed: Vec::new(),
        app_guid: APP_GUID.to_string(),
        object_number: 50902,
        normalized_signature_hash: "test-hash".to_string(),
        body_available: true,
        parse_incomplete: false,
        record_variables: Vec::new(), // intentionally empty — passes 1/2a do nothing
        record_operations: vec![op],
        field_accesses: Vec::new(),
        // `from_own` keeps the injected flat list VERBATIM (including the
        // duplicate name), which is the whole point here: real assembly can never
        // emit a shadowed global, so this stays a synthetic, defensive guard on
        // pass 2b's map-building rather than on how the list was produced.
        variables: RoutineVariables::from_own(variables),
        parameters: Vec::new(),
        access_modifier: None,
        return_type: None,
        call_sites: Vec::new(),
        operation_sites: Vec::new(),
        statement_tree: None,
        loops: Vec::new(),
        source_anchor: dummy_anchor(),
        identifier_references: Vec::new(),
        unreachable_statements: Vec::new(),
        has_branching: false,
        var_assignments: Vec::new(),
        condition_references: Vec::new(),
        enclosing_member: None,
        originating_object: None,
        enclosing_member_range: None,
        entry_temp_guard_receiver: None,
    }
}

// ============================================================================
// Test 1 — LOW-LEVEL unit test: directly exercises pass 2b with a collision
//          in `variables`. FAILS before the fix (last-wins picks Bar/50900),
//          PASSES after (first-wins picks Baz/50901).
// ============================================================================

#[test]
fn pass_2b_first_wins_on_name_collision() {
    // Two tables in the symbol table.
    let bar_table = make_table("Bar", 50900);
    let baz_table = make_table("Baz", 50901);
    // `SymbolTable` borrows its slices, so the array must outlive the table.
    let tables = [bar_table, baz_table];
    let symbols = SymbolTable::build(&[], &tables);

    // `variables` carries the COLLISION: local "Baz" first, then global "Bar".
    // (params → locals → globals order; Task 3 will add globals AFTER locals.)
    let variables = vec![
        L3Variable {
            name: "foo".to_string(),
            declared_type: "Record Baz".to_string(), // LOCAL — should win
            is_parameter: false,
            parameter_index: None,
            initializer: None,
            scope: Some("local".to_string()),
        },
        L3Variable {
            name: "foo".to_string(),
            declared_type: "Record Bar".to_string(), // GLOBAL — must NOT win
            is_parameter: false,
            parameter_index: None,
            initializer: None,
            scope: Some("global".to_string()),
        },
    ];

    let mut routine = make_routine_for_pass2b(variables);

    // Run the full three-pass resolution (no object context → no implicit Rec).
    resolve_routine_record_types(&mut routine, None, &symbols);

    let op_table_id = routine.record_operations[0].table_id.as_deref();

    // pass 2b stores the INTERNAL table id (appGuid/table/N), not the stable id.
    assert_eq!(
        op_table_id,
        Some(internal_table_id(50901).as_str()),
        "FIRST-wins: the LOCAL `Foo: Record Baz` (first in variables) must resolve to \
         Baz (internal id .../table/50901), NOT the subsequent global `Foo: Record Bar` \
         (50900). With last-wins (`insert`) the global clobbers the local; \
         fix: use `.entry(k).or_insert(v)` so the first entry wins.",
    );

    assert_ne!(
        op_table_id,
        Some(internal_table_id(50900).as_str()),
        "the GLOBAL `Foo: Record Bar` (50900) must NOT win when a LOCAL `Foo` \
         appears earlier in the variables list",
    );
}

// ============================================================================
// Test 2 — END-TO-END integration test: verifies that the full pipeline
//          (assemble_and_resolve_inline_program_default) correctly resolves the LOCAL var.
//          This passes both before AND after the fix because `extract_variables`
//          already deduplicates globals when a same-named local exists.
//          Kept as a regression guard for the full-pipeline shape.
// ============================================================================

#[test]
fn integration_local_shadows_global_end_to_end() {
    let source = r#"
table 50900 Bar
{
    fields { field(1; "No."; Code[20]) { } }
}

table 50901 Baz
{
    fields { field(1; "No."; Code[20]) { } }
}

codeunit 50902 "ShadowProbe"
{
    var
        Foo: Record Bar;

    procedure DoWork()
    var
        Foo: Record Baz;
    begin
        Foo.FindSet();
    end;
}
"#;

    let resolved = assemble_and_resolve_inline_program_default(
        &[("src/main.al".to_string(), source.to_string())],
        APP_GUID,
    );

    let routine = resolved
        .routine_by_name("DoWork")
        .expect("DoWork routine must be resolved");

    let ops = routine.record_ops();
    let foo_op = ops
        .iter()
        .find(|(_, var, _)| var.eq_ignore_ascii_case("foo"))
        .expect("FindSet op on Foo must be present");

    assert_eq!(
        foo_op.2,
        Some(stable_table(50901)),
        "end-to-end: the LOCAL `Foo: Record Baz` (50901) must shadow the GLOBAL \
         `Foo: Record Bar` (50900) — innermost declaration wins",
    );
}

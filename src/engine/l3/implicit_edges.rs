//! Implicit-trigger edges (R2b Task 3) — brought to parity with the fresh
//! resolver's `implicit_trigger_route_applicable`
//! (`src/program/resolve/applicability.rs:159-227`).
//!
//! For each trigger-invoking record op whose record variable resolves to a table
//! IN indexed source, emit an edge to that table's trigger routine. Tables
//! al-sem cannot see produce no edge (reflected in coverage, not invented here).
//!
//! Two applicability preconditions mirror the fresh oracle exactly (Wave-2b Task
//! 1 — see `docs/superpowers/specs/2026-07-18-wave2-measurements.md` §2/§4a):
//!
//! 1. **RunTrigger gate** (`applicability.rs:166-168`): only an explicit `false`
//!    run-trigger arg suppresses the edge. The oracle maps an absent/unrecoverable
//!    arg to `RunTrigger::Guarded` (NOT `False`) — `Guarded`/`True` both keep the
//!    edge — so we gate on `op.run_trigger == Some(false)` and treat `None`
//!    (absent) exactly as the oracle's `Guarded`: edge kept. NOTE the L2 walk only
//!    captures `run_trigger` for `Modify`/`Delete`/`DeleteAll`/`ModifyAll`
//!    (`ir_walk.rs`); `Insert`/`Validate` never carry it (always `None` → kept),
//!    so this gate bites only on `Modify(false)`/`Delete(false)` in real source.
//! 2. **Field-specific OnValidate targeting** (`applicability.rs:184-195`): a
//!    `Validate(field)` edges to that field's OWN `OnValidate`, never an arbitrary
//!    per-table one. `Insert`/`Modify`/`Delete` still edge to the table's
//!    object-level trigger (`enclosing_member == None`), unchanged.
//!
//! `Validate` → "resolved"; `Insert`/`Modify`/`Delete` → "maybe" (the Resolution
//! taxonomy is unchanged — a follow-up question, not this task's surface).

use super::call_resolver::CallEdge;
use super::l3_workspace::{L3RecordOperation, L3Routine, L3Workspace};
use super::symbol_table::SymbolTable;
use super::taxonomy::{DispatchKind, Resolution};
use crate::engine::l2::node_util::strip_quotes;
use al_syntax::IdentifierFoldExt;

/// Normalize a captured record-op field argument (raw source text — quoted
/// identifiers keep their quotes, e.g. `"E-Mail"`) into the same logical form
/// `L3Routine.enclosing_member` is stored in (RE-3/RE-4 in `l3_workspace.rs`:
/// outer quotes stripped, inner `""` collapsed to `"`), case-folded for matching.
/// Applying the SAME transform to both sides is what makes field matching work;
/// getting it wrong silently never matches.
fn normalize_field_name(raw: &str) -> String {
    strip_quotes(raw).replace("\"\"", "\"").fold_identifier()
}

/// (trigger routine name, edge Resolution).
fn trigger_mapping(op: &str) -> Option<(&'static str, Resolution)> {
    match op {
        "Validate" => Some(("OnValidate", Resolution::Resolved)),
        "Insert" => Some(("OnInsert", Resolution::Maybe)),
        "Modify" => Some(("OnModify", Resolution::Maybe)),
        "Delete" => Some(("OnDelete", Resolution::Maybe)),
        // Rename takes no RunTrigger: measured on BC 28 (#9), it always fires
        // OnRename and never OnModify.
        "Rename" => Some(("OnRename", Resolution::Resolved)),
        _ => None,
    }
}

/// Build implicit-trigger CallEdges over the workspace. The op's operation id
/// doubles as the callsite ref (`callsiteId == operationId == op.id`).
pub fn build_implicit_trigger_edges(
    workspace: &L3Workspace,
    symbols: &SymbolTable,
) -> Vec<CallEdge> {
    let mut edges: Vec<CallEdge> = Vec::new();
    for routine in &workspace.routines {
        for op in &routine.record_operations {
            edges.extend(implicit_trigger_edge_for_op(routine, op, symbols));
        }
    }
    edges
}

/// The implicit-trigger edge of ONE record op, if any. The per-op body of
/// [`build_implicit_trigger_edges`], shared with the program adapter
/// (`program_calls`), which since engine-switch S3.5 reads it only for its
/// agreement counters (`adapter_trigger_edges_beyond_l3`/`_l3_only`).
pub(crate) fn implicit_trigger_edge_for_op(
    routine: &L3Routine,
    op: &L3RecordOperation,
    symbols: &SymbolTable,
) -> Option<CallEdge> {
    let (trigger_name, resolution) = trigger_mapping(&op.op)?;
    // Precondition 1 — RunTrigger gate (applicability.rs:166-168): only an
    // explicit `false` suppresses; `None` (absent → oracle `Guarded`) and
    // `Some(true)` keep the edge.
    if op.run_trigger == Some(false) {
        return None;
    }
    // Table not resolved → cannot find its trigger.
    let table = symbols.table_by_id(op.table_id.as_ref()?)?;
    // Tables are objects too — look up by type + number.
    let table_object = symbols.object_by_type_number("Table", table.table_number)?;
    // Precondition 2 — field-specific OnValidate (applicability.rs:184-195):
    // a Validate targets the validated field's OWN OnValidate; every other
    // op targets the table's object-level trigger (enclosing_member None).
    let trigger = if op.op == "Validate" {
        // Validate with no captured field → no edge (oracle: ctx.field required).
        let field_lc = validate_field_lc(op)?;
        symbols.trigger_in_object(&table_object.id, trigger_name, Some(&field_lc))
    } else {
        symbols.trigger_in_object(&table_object.id, trigger_name, None)
    }?;
    Some(CallEdge {
        from: routine.id.clone(),
        to: Some(trigger.id.clone()),
        callsite_id: op.id.clone(),
        operation_id: op.id.clone(),
        dispatch_kind: DispatchKind::ImplicitTrigger,
        resolution,
        candidates: None,
        external_type_ref: None,
        receiver_type: None,
        dispatch_meta: None,
        unknown_method_name: None,
        receiver_shape: None,
    })
}

/// The validated field of a `Validate` op, normalized and case-folded (the
/// form a field trigger's `enclosing_member` matches against).
pub(crate) fn validate_field_lc(op: &L3RecordOperation) -> Option<String> {
    op.field_arguments
        .as_ref()
        .and_then(|fa| fa.first())
        .map(|f| normalize_field_name(f))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::l2::features::PAnchor;
    use crate::engine::l3::l3_workspace::{
        L3Object, L3RecordOperation, L3Routine, L3Table, L3Workspace, RoutineVariables,
    };

    const APP: &str = "app";
    const TABLE_OBJ_ID: &str = "app/table/50000";
    const TABLE_ID: &str = "app/table/50000"; // L3Table.id == ${appGuid}/table/${tableNumber}
    const TABLE_NO: i64 = 50000;

    fn anchor() -> PAnchor {
        PAnchor {
            source_unit_id: "ws:test.al".to_string(),
            start_line: 0,
            start_column: 0,
            end_line: 0,
            end_column: 0,
            syntax_kind: "procedure".to_string(),
        }
    }

    fn table_object() -> L3Object {
        L3Object {
            id: TABLE_OBJ_ID.to_string(),
            app_guid: APP.to_string(),
            object_type: "Table".to_string(),
            object_number: TABLE_NO,
            name: "Cust".to_string(),
            source_table_name: None,
            extends_target_name: None,
            implements_interfaces: Some(Vec::new()),
            object_subtype: None,
            page_type: None,
            inherent_commit_behavior: None,
            source_table_temporary: None,
            page_controls: Vec::new(),
            single_instance: None,
            editable: None,
            insert_allowed: None,
            modify_allowed: None,
            delete_allowed: None,
            source_anchor: None,
        }
    }

    fn table_entry() -> L3Table {
        L3Table {
            id: TABLE_ID.to_string(),
            app_guid: APP.to_string(),
            table_number: TABLE_NO,
            name: "Cust".to_string(),
            fields: Vec::new(),
            keys: Vec::new(),
            is_temporary: false,
            is_extension_stub: false,
        }
    }

    fn bare_routine(id: &str, object_id: &str, name: &str) -> L3Routine {
        L3Routine {
            id: id.to_string(),
            stable_routine_id: String::new(),
            object_id: object_id.to_string(),
            object_type: "Table".to_string(),
            name: name.to_string(),
            kind: "procedure".to_string(),
            attributes_parsed: Vec::new(),
            app_guid: APP.to_string(),
            object_number: TABLE_NO,
            normalized_signature_hash: String::new(),
            body_available: true,
            parse_incomplete: false,
            record_variables: Vec::new(),
            record_operations: Vec::new(),
            field_accesses: Vec::new(),
            variables: RoutineVariables::default(),
            parameters: Vec::new(),
            access_modifier: None,
            return_type: None,
            call_sites: Vec::new(),
            operation_sites: Vec::new(),
            statement_tree: None,
            loops: Vec::new(),
            source_anchor: anchor(),
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

    /// A trigger routine on the table object. `enclosing_member = None` marks an
    /// object-level trigger (OnInsert/OnModify/OnDelete, or a — degenerate —
    /// object-level OnValidate); `Some(field)` marks a field's OWN trigger.
    fn trigger_routine(id: &str, name: &str, enclosing_member: Option<&str>) -> L3Routine {
        let mut r = bare_routine(id, TABLE_OBJ_ID, name);
        r.kind = "trigger".to_string();
        r.enclosing_member = enclosing_member.map(str::to_string);
        r
    }

    /// A caller routine (in a codeunit) carrying the record operations the builder
    /// iterates over.
    fn caller_routine(id: &str, ops: Vec<L3RecordOperation>) -> L3Routine {
        let mut r = bare_routine(id, "app/codeunit/50100", "DoWork");
        r.object_type = "Codeunit".to_string();
        r.record_operations = ops;
        r
    }

    fn record_op(
        id: &str,
        op: &str,
        field_arguments: Option<Vec<String>>,
        run_trigger: Option<bool>,
    ) -> L3RecordOperation {
        L3RecordOperation {
            id: id.to_string(),
            op: op.to_string(),
            record_variable_name: "Rec".to_string(),
            record_variable_id: None,
            table_id: Some(TABLE_ID.to_string()),
            temp_state: None,
            field_arguments,
            source_anchor: anchor(),
            loop_stack: Vec::new(),
            field_argument_infos: None,
            in_until_condition: false,
            run_trigger,
        }
    }

    /// Assemble `(workspace, triggers)`: `callers` are the workspace routines the
    /// builder iterates; `triggers` are the routines the symbol table indexes (the
    /// lookup surface), kept OUTSIDE the workspace so the two populations stay
    /// distinct.
    ///
    /// A borrowing [`SymbolTable`] cannot be returned alongside the data it
    /// borrows (that is a self-referential struct), so the caller finishes with
    /// `let symbols = SymbolTable::build(&ws.objects, &ws.tables, &triggers);` —
    /// the same three slices this helper used to pass.
    fn setup(triggers: Vec<L3Routine>, callers: Vec<L3Routine>) -> (L3Workspace, Vec<L3Routine>) {
        let workspace = L3Workspace {
            objects: vec![table_object()],
            tables: vec![table_entry()],
            routines: callers,
        };
        (workspace, triggers)
    }

    // 1. A Validate must target the validated field's OWN OnValidate trigger, never
    //    an arbitrary per-table one. The table declares two field OnValidate
    //    triggers (A and B); the old name-only `routine_in_object` collapses both
    //    onto one key (last-wins → B here), so validating A wrongly resolved to B.
    #[test]
    fn validate_targets_field_specific_trigger() {
        let ta = trigger_routine("app/table/50000::onvalidate::A", "OnValidate", Some("A"));
        let tb = trigger_routine("app/table/50000::onvalidate::B", "OnValidate", Some("B"));
        let caller = caller_routine(
            "app/codeunit/50100/r0",
            vec![record_op(
                "app/codeunit/50100/r0/op0",
                "Validate",
                Some(vec!["A".to_string()]),
                None,
            )],
        );
        let (ws, triggers) = setup(vec![ta, tb], vec![caller]);
        let symbols = SymbolTable::build(&ws.objects, &ws.tables, &triggers);
        let edges = build_implicit_trigger_edges(&ws, &symbols);
        assert_eq!(edges.len(), 1, "exactly one validate edge");
        assert_eq!(
            edges[0].to.as_deref(),
            Some("app/table/50000::onvalidate::A"),
            "must target field A's OWN OnValidate, not the last-wins collision winner",
        );
    }

    // 2. Validating a field that has NO OnValidate trigger emits no edge (the old
    //    builder would still edge to the collapsed per-table OnValidate).
    #[test]
    fn validate_without_matching_field_trigger_emits_no_edge() {
        let ta = trigger_routine("app/table/50000::onvalidate::A", "OnValidate", Some("A"));
        let caller = caller_routine(
            "c/r0",
            vec![record_op(
                "c/r0/op0",
                "Validate",
                Some(vec!["C".to_string()]),
                None,
            )],
        );
        let (ws, triggers) = setup(vec![ta], vec![caller]);
        let symbols = SymbolTable::build(&ws.objects, &ws.tables, &triggers);
        let edges = build_implicit_trigger_edges(&ws, &symbols);
        assert!(
            edges.is_empty(),
            "no edge when the validated field has no OnValidate trigger",
        );
    }

    // 3. RunTrigger gate: `Some(false)` suppresses (mirror
    //    `implicit_trigger_route_applicable`'s `RunTrigger::False` short-circuit);
    //    an ABSENT arg (`None` → fresh `RunTrigger::Guarded`) does NOT suppress.
    #[test]
    fn insert_run_trigger_false_emits_no_edge() {
        let oninsert = trigger_routine("app/table/50000::oninsert", "OnInsert", None);
        let caller_false = caller_routine(
            "cf/r0",
            vec![record_op("cf/r0/op0", "Insert", None, Some(false))],
        );
        let caller_absent =
            caller_routine("ca/r0", vec![record_op("ca/r0/op0", "Insert", None, None)]);
        let (ws, triggers) = setup(vec![oninsert], vec![caller_false, caller_absent]);
        let symbols = SymbolTable::build(&ws.objects, &ws.tables, &triggers);
        let edges = build_implicit_trigger_edges(&ws, &symbols);
        assert_eq!(
            edges.len(),
            1,
            "Some(false) suppressed the edge; None (absent → Guarded) kept it",
        );
        assert_eq!(
            edges[0].from, "ca/r0",
            "the surviving edge is the absent-arg Insert"
        );
        assert_eq!(edges[0].to.as_deref(), Some("app/table/50000::oninsert"));
    }

    // 4. Regression guard: a RunTrigger-true Insert still edges to the table's
    //    object-level OnInsert (enclosing_member None) — unchanged behavior.
    #[test]
    fn insert_object_level_trigger_still_edges() {
        let oninsert = trigger_routine("app/table/50000::oninsert", "OnInsert", None);
        let caller = caller_routine(
            "c/r0",
            vec![record_op("c/r0/op0", "Insert", None, Some(true))],
        );
        let (ws, triggers) = setup(vec![oninsert], vec![caller]);
        let symbols = SymbolTable::build(&ws.objects, &ws.tables, &triggers);
        let edges = build_implicit_trigger_edges(&ws, &symbols);
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].to.as_deref(), Some("app/table/50000::oninsert"));
    }

    // 5. A Validate must never target an object-level OnValidate (enclosing_member
    //    None) — only a field-specific one qualifies (mirror the oracle's
    //    `(Some(field), Some(enc))` requirement).
    #[test]
    fn validate_never_targets_object_level_onvalidate() {
        let obj_level = trigger_routine("app/table/50000::onvalidate::obj", "OnValidate", None);
        let caller = caller_routine(
            "c/r0",
            vec![record_op(
                "c/r0/op0",
                "Validate",
                Some(vec!["B".to_string()]),
                None,
            )],
        );
        let (ws, triggers) = setup(vec![obj_level], vec![caller]);
        let symbols = SymbolTable::build(&ws.objects, &ws.tables, &triggers);
        let edges = build_implicit_trigger_edges(&ws, &symbols);
        assert!(
            edges.is_empty(),
            "an object-level OnValidate is never a Validate target",
        );
    }

    // 6. Quoted-field normalization guard. `L3Routine.enclosing_member` stores the
    //    UNESCAPED logical name (`E-Mail`, no outer quotes); the L2 walk captures the
    //    Validate field argument as RAW source text (`"E-Mail"`, WITH quotes). The
    //    edge only lands if `normalize_field_name` strips the quotes on the arg side
    //    so both fold-match — this exercises that path (all prior tests used bare
    //    identifiers that skip it). A distractor field proves it stays specific.
    #[test]
    fn validate_targets_quoted_field_specific_trigger() {
        let email = trigger_routine(
            "app/table/50000::onvalidate::email",
            "OnValidate",
            Some("E-Mail"),
        );
        let phone = trigger_routine(
            "app/table/50000::onvalidate::phone",
            "OnValidate",
            Some("Phone No."),
        );
        let caller = caller_routine(
            "c/r0",
            vec![record_op(
                "c/r0/op0",
                "Validate",
                // Raw arg text as the L2 walk stores it for a quoted identifier.
                Some(vec!["\"E-Mail\"".to_string()]),
                None,
            )],
        );
        let (ws, triggers) = setup(vec![email, phone], vec![caller]);
        let symbols = SymbolTable::build(&ws.objects, &ws.tables, &triggers);
        let edges = build_implicit_trigger_edges(&ws, &symbols);
        assert_eq!(edges.len(), 1, "exactly one validate edge");
        assert_eq!(
            edges[0].to.as_deref(),
            Some("app/table/50000::onvalidate::email"),
            "quoted \"E-Mail\" arg must strip-and-fold to match the E-Mail trigger",
        );
    }

    // 6b. Inner-`\"\"`-escape guard: a field named with an embedded quote,
    //     `field(...; \"A\"\"B\"; ...)`, stores enclosing_member as the unescaped
    //     `A\"B`; the raw arg is `\"A\"\"B\"`. `normalize_field_name` must strip the
    //     outer quotes AND collapse the inner `\"\"`→`\"` for the two to fold-match.
    #[test]
    fn validate_matches_inner_escaped_quote_field() {
        let weird = trigger_routine(
            "app/table/50000::onvalidate::weird",
            "OnValidate",
            Some("A\"B"),
        );
        let caller = caller_routine(
            "c/r0",
            vec![record_op(
                "c/r0/op0",
                "Validate",
                Some(vec!["\"A\"\"B\"".to_string()]),
                None,
            )],
        );
        let (ws, triggers) = setup(vec![weird], vec![caller]);
        let symbols = SymbolTable::build(&ws.objects, &ws.tables, &triggers);
        let edges = build_implicit_trigger_edges(&ws, &symbols);
        assert_eq!(edges.len(), 1);
        assert_eq!(
            edges[0].to.as_deref(),
            Some("app/table/50000::onvalidate::weird"),
            "inner \"\" must collapse so the raw arg matches the stored logical name",
        );
    }
}

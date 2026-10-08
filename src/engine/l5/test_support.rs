//! Synthetic-input constructors shared by the L5 native-oracle tests. NOT a
//! golden fixture — these build minimal `ModelRoutine` / `CombinedGraph` /
//! `CapabilityFact` / `FullRoutineSummary` values directly so each oracle
//! exercises the query functions on hand-built inputs (mirroring al-sem's
//! probe-style soundness oracles, not a byte-diff).
//!
//! `#[cfg(test)]`-only — never compiled into the shipping engine.

#![cfg(test)]

use std::collections::HashMap;

use crate::engine::l4::capability_cone::{CapabilityFact, CoverageRecord};
use crate::engine::l4::combined_graph::{CombinedEdge, CombinedGraph};
use crate::engine::l4::cone_derived::{ConeDerivedBuilder, ConeDerivedStore};
use crate::engine::l5::detector_context::DetectorContext;
use crate::engine::l5::full_summary::FullRoutineSummary;
use crate::program::body::features::{
    PAnchor, PCallArgumentBinding, PCallSite, PCallee, PLoop, POperationSite, PTempState,
};
use crate::program::model::workspace::{ModelRecordOperation, ModelRoutine, RoutineVariables};

/// ⟨C1⟩ The derived cone substrate for a set of hand-built summaries — each
/// routine's literal `reachable()` sequence folded into a row. Fixture summaries
/// carry their `capability_facts_inherited` as INPUT (there is no cone walk and
/// so no key-dedup), so folding the flat reachable list is exactly right here —
/// unlike the production path, which must fold key-deduped representatives.
/// Every hand-built `DetectorContext` uses this so its `cone_derived` field can
/// never silently disagree with its `summaries`.
pub fn cone_store_of(summaries: &HashMap<String, FullRoutineSummary>) -> ConeDerivedStore {
    let mut b = ConeDerivedBuilder::default();
    for (id, s) in summaries {
        // ⟨fix M3⟩ Fold keyed by `s.routine_id`, not the map key `id` — every
        // consumer (`ConeDerivedStore::row` and friends) looks the row up by
        // `summary.routine_id`, so a fixture whose map key diverges from its
        // own `routine_id` must still fold under the id callers will query.
        debug_assert_eq!(
            id, &s.routine_id,
            "cone_store_of: fixture map key {id:?} must equal summary.routine_id {:?} — \
             every derived-store consumer looks the row up by routine_id, not the map key",
            s.routine_id
        );
        // ⟨C1 Task 3⟩ `reachable_iter()` is gone (R6 — it would have silently
        // yielded a direct-only view once the analyze path stopped materializing
        // the inherited Vec). Fixture summaries always own their inherited facts,
        // so the chain is spelled out here; `inherited_raw()` panics loudly if a
        // fixture ever forgets to supply them.
        b.fold_routine(
            &s.routine_id,
            s.capability_facts_direct.iter().chain(s.inherited_raw()),
        );
    }
    b.finish()
}

/// A throwaway anchor (positions are irrelevant to the L5 query substrate).
pub fn dummy_anchor() -> PAnchor {
    PAnchor {
        source_unit_id: "ws:test".to_string(),
        start_line: 0,
        start_column: 0,
        end_line: 0,
        end_column: 0,
        syntax_kind: "test".to_string(),
    }
}

/// A resolved combined edge `from → to` with a callsite id.
pub fn edge(from: &str, to: &str, callsite_id: &str) -> CombinedEdge {
    CombinedEdge {
        from: from.to_string(),
        to: to.to_string(),
        kind: "direct".to_string(),
        callsite_id: Some(callsite_id.to_string()),
        operation_id: None,
        event_id: None,
        subscriber_app_id: None,
        resolution: "resolved".to_string(),
    }
}

/// Build a `CombinedGraph` from a node list + flat edge list. `nodes` is sorted;
/// `edges_by_from` is grouped (each per-from list in input order — the L5
/// reverse-graph builder is robust to per-from ordering). `edges_from_order`
/// records the first-appearance of each `from` key in the `edges` slice
/// (matching al-sem JS Map insertion order).
pub fn graph_from_edges(nodes: &[&str], edges: &[CombinedEdge]) -> CombinedGraph {
    let mut node_vec: Vec<String> = nodes.iter().map(|n| n.to_string()).collect();
    node_vec.sort();
    let mut edges_by_from: HashMap<String, Vec<CombinedEdge>> = HashMap::new();
    let mut edges_from_order: Vec<String> = Vec::new();
    for e in edges {
        if !edges_by_from.contains_key(&e.from) {
            edges_from_order.push(e.from.clone());
        }
        edges_by_from
            .entry(e.from.clone())
            .or_default()
            .push(e.clone());
    }
    CombinedGraph {
        nodes: node_vec,
        edges_by_from,
        edges_from_order,
        uncertainty_edges: Vec::new(),
        typed_edges: Vec::new(),
    }
}

/// A minimal `ModelRoutine` with the given internal id + kind. All other fields are
/// empty / defaulted — only the L5-substrate-relevant ones (id, kind,
/// operation_sites, call_sites) are exercised by the oracles.
pub fn routine(id: &str, kind: &str) -> ModelRoutine {
    ModelRoutine {
        id: id.to_string(),
        stable_routine_id: format!("stable::{id}"),
        object_id: "app/Codeunit/1".to_string(),
        object_type: "Codeunit".to_string(),
        name: id.to_string(),
        kind: kind.to_string(),
        attributes_parsed: Vec::new(),
        app_guid: "app".to_string(),
        object_number: 1,
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

/// A routine with `kind` carrying `commit` operation sites for each given op id.
pub fn op_commit_routine(id: &str, kind: &str, commit_op_ids: &[&str]) -> ModelRoutine {
    let mut r = routine(id, kind);
    for op_id in commit_op_ids {
        r.operation_sites.push(POperationSite {
            id: op_id.to_string(),
            kind: "commit".to_string(),
            loop_stack: Vec::new(),
            source_anchor: dummy_anchor(),
            under_asserterror: None,
            control_context: None,
            order: None,
        });
    }
    r
}

/// An object-run call site (e.g. a `Codeunit.Run`) with the given object kind +
/// `objectRunReturnUsed` flag.
pub fn object_run_call_site(id: &str, object_kind: &str, return_used: Option<bool>) -> PCallSite {
    PCallSite {
        id: id.to_string(),
        operation_id: format!("{id}/op"),
        callee_text: format!("{object_kind}.Run"),
        callee: PCallee::ObjectRun {
            object_kind: object_kind.to_string(),
            target_type: object_kind.to_string(),
            target_ref: Some("50100".to_string()),
            target_is_name: false,
        },
        argument_texts: Vec::new(),
        argument_infos: Vec::new(),
        argument_bindings: Vec::new(),
        loop_stack: Vec::new(),
        source_anchor: dummy_anchor(),
        result_consumed: None,
        object_run_return_used: return_used,
        under_asserterror: None,
        control_context: None,
        order: None,
        in_statement_position: false,
        receiver_temp_state: None,
    }
}

/// A capability fact with the given op / resource kind / optional resource id.
/// Other fields are defaulted (they do not affect any L5 query helper).
pub fn fact(
    op: &'static str,
    resource_kind: &'static str,
    resource_id: Option<&str>,
) -> CapabilityFact {
    CapabilityFact {
        subject: "r".to_string(),
        op,
        resource_kind,
        resource_id: resource_id.map(|s| s.to_string()),
        resource_arg_source: None,
        confidence: "static",
        provenance: "direct",
        via: "self",
        witness_operation_id: None,
        witness_callsite_id: None,
        extra: None,
    }
}

/// A coverage record whose `inherited_status` is the given value.
pub fn coverage(inherited_status: &str) -> CoverageRecord {
    CoverageRecord {
        subject: "r".to_string(),
        direct_status: inherited_status.to_string(),
        inherited_status: inherited_status.to_string(),
        reasons: Vec::new(),
        unknown_targets: Vec::new(),
    }
}

/// A `FullRoutineSummary` from direct + inherited facts + optional coverage.
/// ⟨C1 Task 3⟩ Fixture summaries are always MATERIALIZED (`Some(inherited)`) —
/// they carry their inherited facts as INPUT rather than as a cone output, so
/// `inherited_raw()` is always legal on them. A test that wants the derived-only
/// shape builds it with `FullRoutineSummary::new(.., None, ..)` directly.
pub fn summary(
    routine_id: &str,
    direct: Vec<CapabilityFact>,
    inherited: Vec<CapabilityFact>,
    cov: Option<CoverageRecord>,
) -> FullRoutineSummary {
    FullRoutineSummary::new(routine_id.to_string(), direct, Some(inherited), cov)
}

// ---------------------------------------------------------------------------
// d1-reachability fixture constructors — shared by `d1_graph`'s and
// `d1_reach`'s test modules (hoisted here per Task 1's review: a second
// duplication of `d1_graph`'s local ctors moves to `test_support`).
// ---------------------------------------------------------------------------

/// A minimal `PLoop` with the given id (type `"for"`).
pub fn loop_def(id: &str) -> PLoop {
    PLoop {
        id: id.to_string(),
        loop_type: "for".to_string(),
        source_anchor: dummy_anchor(),
        exhausting_advance: None,
    }
}

/// An in-loop bare-call call site: `<callee_name>(...)` inside `loop_stack`.
pub fn call_site(id: &str, callee_name: &str, loop_stack: Vec<String>) -> PCallSite {
    PCallSite {
        id: id.to_string(),
        operation_id: format!("{id}/op"),
        callee_text: callee_name.to_string(),
        callee: PCallee::Bare {
            name: callee_name.to_string(),
        },
        argument_texts: Vec::new(),
        argument_infos: Vec::new(),
        argument_bindings: Vec::new(),
        loop_stack,
        source_anchor: dummy_anchor(),
        result_consumed: None,
        object_run_return_used: None,
        under_asserterror: None,
        control_context: None,
        order: None,
        in_statement_position: false,
        receiver_temp_state: None,
    }
}

/// A `known`/`parameter-dependent` `PTempState`.
pub fn ts_known(value: bool) -> PTempState {
    PTempState {
        kind: "known".to_string(),
        value: Some(value),
        parameter_index: None,
    }
}

pub fn ts_pd(idx: u32) -> PTempState {
    PTempState {
        kind: "parameter-dependent".to_string(),
        value: None,
        parameter_index: Some(idx),
    }
}

/// One argument binding of callee param `parameter_index` to `source_temp_state`.
pub fn arg_binding(
    parameter_index: u32,
    source_temp_state: Option<PTempState>,
) -> PCallArgumentBinding {
    PCallArgumentBinding {
        parameter_index,
        source_kind: "variable".to_string(),
        source_variable_name: Some("arg".to_string()),
        source_record_variable_id: None,
        source_parameter_index: None,
        caller_source_parameter_is_var: None,
        source_temp_state,
        argument_anchor: dummy_anchor(),
    }
}

/// A resolved combined edge `from -> to` with an explicit `kind` (unlike
/// [`edge`], which hardcodes `"direct"`).
pub fn edge_kind(from: &str, to: &str, callsite_id: &str, kind: &str) -> CombinedEdge {
    CombinedEdge {
        from: from.to_string(),
        to: to.to_string(),
        kind: kind.to_string(),
        callsite_id: Some(callsite_id.to_string()),
        operation_id: None,
        event_id: None,
        subscriber_app_id: None,
        resolution: "resolved".to_string(),
    }
}

/// A minimal `ModelRecordOperation` (`temp_state` defaults to `None`; callers
/// mutate `.temp_state` for the PD/Known cases).
#[allow(clippy::too_many_arguments)]
pub fn record_op(
    id: &str,
    op: &str,
    record_variable_name: &str,
    table_id: Option<&str>,
    loop_stack: Vec<String>,
    in_until_condition: bool,
) -> ModelRecordOperation {
    ModelRecordOperation {
        id: id.to_string(),
        op: op.to_string(),
        record_variable_name: record_variable_name.to_string(),
        record_variable_id: None,
        table_id: table_id.map(|s| s.to_string()),
        temp_state: None,
        field_arguments: None,
        source_anchor: dummy_anchor(),
        loop_stack,
        field_argument_infos: None,
        in_until_condition,
        run_trigger: None,
    }
}

/// A minimal `DetectorContext` sufficient for `build_d1_graph` / `search_loops`:
/// `routine_by_id` / `graph.edges_by_from` / `summaries` are populated from the
/// args, and `call_site_by_id` is built from every routine's own call sites (so
/// `D1Edge.loop_depth` derives non-zero when an edge's callsite carries a
/// `loop_stack`). Everything else is empty / default.
pub fn minimal_ctx<'a>(
    routines: &'a [ModelRoutine],
    graph_edges: HashMap<String, Vec<CombinedEdge>>,
    summaries: HashMap<String, FullRoutineSummary>,
) -> DetectorContext<'a> {
    let routine_by_id: HashMap<&'a str, &'a ModelRoutine> =
        routines.iter().map(|r| (r.id.as_str(), r)).collect();
    let call_site_by_id: HashMap<&'a str, &'a PCallSite> = routines
        .iter()
        .flat_map(|r| r.call_sites.iter().map(|cs| (cs.id.as_str(), cs)))
        .collect();
    let graph = crate::engine::l4::combined_graph::CombinedGraph {
        nodes: vec![],
        edges_by_from: graph_edges,
        edges_from_order: vec![],
        uncertainty_edges: vec![],
        typed_edges: vec![],
    };
    DetectorContext {
        graph,
        event_graph: crate::program::model::events::EventGraph {
            events: vec![],
            edges: vec![],
        },
        routine_by_id,
        objects_by_id: HashMap::new(),
        table_by_id: HashMap::new(),
        reverse_call_graph: std::collections::BTreeMap::new(),
        entry_points: std::collections::BTreeSet::new(),
        transaction_spans: vec![],
        resolved_call_edge_by_callsite: HashMap::new(),
        uncertainty_edges_by_from: HashMap::new(),
        uncertainties_by_node: HashMap::new(),
        uncertainties: Default::default(),
        call_site_by_id,
        // ⟨C1⟩ Fold BEFORE the move so the derived substrate always mirrors this
        // context's own summaries.
        cone_derived: cone_store_of(&summaries),
        summaries,
        event_flow_indexes: crate::engine::l5::event_flow::EventFlowIndexes::default(),
        parameter_roles_by_routine: HashMap::new(),
        upgraded_bindings_by_callsite: HashMap::new(),
        reachable_roots: std::collections::BTreeSet::new(),
        internal_reachable_externally: false,
        dep_routine_ids: std::collections::BTreeSet::new(),
        declared_dependencies: Vec::new(),
        app_versions: HashMap::new(),
        friends: HashMap::new(),
        root_classifications_by_routine: HashMap::new(),
        ordering_facts: std::sync::OnceLock::new(),
        ordering_source: None,
        closed_world_temp_params: Default::default(),
        summarize_diagnostics: Vec::new(),
        db_effect_bundle: None,
        reverse_effect_index: None,
        fingerprint_index: crate::engine::l5::fingerprint::FingerprintIndex::build(routines, &[]),
        cross_extension_subscribers: std::collections::BTreeMap::new(),
    }
}

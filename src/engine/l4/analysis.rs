//! #160 step 2 (spec §4.4.1): the L4 analysis as one value. [`build_l4`] is
//! the SCC and the v2 solve that the detector context and query's two bundle
//! builders (digest calls one of them) each ran for themselves.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use serde_json::json;

use crate::engine::l4::combined_graph::CombinedGraph;
use crate::engine::l4::effect_store::SummaryBundle;
use crate::engine::l4::scc::{SccInputGraph, SccResult, tarjan_scc};
use crate::engine::l4::summary::RoutineSummary;
use crate::engine::l4::summary_runner::{
    FieldIndex, NON_CONVERGENCE_TAIL, SummarizeDiagnostic, compute_summaries_v2_bundle_with_leaves,
    summaries_census,
};
use crate::engine::l5::detector_context::CrossAppInputs;
use crate::engine::perf_trace as pt;
use crate::program::model::workspace::{Model, ModelEntities};

/// What outlives `det` in digest and prove: the SCC, the compact db-effect
/// store and the solver's diagnostics. The field index is a solver input only,
/// so it is a local of [`build_l4`].
pub struct L4Analysis {
    /// The ONE SCC owner: computed by [`build_l4`] (counter `build.graph_scc`),
    /// or the cross-app base's `combined_scc`, shared by pointer.
    pub scc: Arc<SccResult>,
    pub bundle: SummaryBundle,
    pub summarize_diagnostics: Vec<SummarizeDiagnostic>,
    /// The non-convergence line(s) the solver prints (§4.4.7).
    pub warnings: Vec<String>,
}

/// The solver's per-routine core summaries, UNHARVESTED (their `db_effects`
/// are empty; the rows live in the bundle). The caller harvests or drops them.
pub struct CoreSummaries(pub HashMap<String, RoutineSummary>);

/// What a caller asks [`build_l4`] for. `extra_diagnostics` is appended to
/// `summarize_diagnostics` AFTER `warnings` are rendered, so an injected row is
/// never printed; it is empty in every production caller.
pub struct L4Request {
    pub scc: Option<Arc<SccResult>>,
    pub extra_diagnostics: Vec<SummarizeDiagnostic>,
}

/// The SCC (unless `req.scc` hands one in) and the v2 solve over `graph`, with
/// the dependency leaves and fixed roles from `cross`.
// The span names keep the detector context's (`context.*`) on purpose, so
// traces stay comparable; query and digest report them too (and the summaries
// census) once they call this (Tasks 25-27).
pub(crate) fn build_l4(
    model: &Model,
    graph: &CombinedGraph,
    req: L4Request,
    cross: Option<&CrossAppInputs<'_>>,
) -> (L4Analysis, CoreSummaries) {
    let computed_scc = req.scc.is_none();
    let scc = req.scc.unwrap_or_else(|| {
        let _s = pt::span("context", "context.core_scc_tarjan");
        Arc::new(graph_scc(graph))
    });
    emit_scc_stats(graph, &scc);

    let ws = &model.workspace;
    let no_leaves: HashMap<String, RoutineSummary> = HashMap::new();
    let no_fixed_roles: HashSet<String> = HashSet::new();
    let leaf_summaries = cross.map_or(&no_leaves, |c| c.leaf_summaries);
    let fixed_roles = cross.map_or(&no_fixed_roles, |c| c.fixed_roles);
    let summaries_span = pt::span("context", "context.compute_summaries");
    let field_index = solver_field_index(ws);
    let (bundle, core_summaries, diagnostics) = compute_summaries_v2_bundle_with_leaves(
        &ws.routines,
        graph,
        &scc,
        &model.calls.upgraded_bindings,
        &field_index,
        leaf_summaries,
        fixed_roles,
    );
    drop(field_index);
    drop(summaries_span);
    summaries_census::report();
    // The LEAN entry point leaves the solved summaries' `db_effects` empty (the
    // rows live in the bundle); a non-empty row means a materializing entry
    // point crept back (⟨Task B1⟩, commit a0cd348). A cross-app fixed leaf is
    // handed in WITH its own direct rows.
    debug_assert!(
        core_summaries
            .iter()
            .all(|(id, s)| s.db_effects.is_empty() || leaf_summaries.contains_key(id)),
        "build_l4's core summaries must carry EMPTY db_effects"
    );

    let warnings = render_warnings(&diagnostics);
    let mut summarize_diagnostics = diagnostics;
    summarize_diagnostics.extend(req.extra_diagnostics);
    let l4 = L4Analysis {
        scc,
        bundle,
        summarize_diagnostics,
        warnings,
    };
    if crate::engine::census::enabled() {
        crate::census_hook::mark("census.l4");
        census_l4(&l4, computed_scc);
    }
    (l4, CoreSummaries(core_summaries))
}

/// The line each non-convergence diagnostic stands for, as the solver prints
/// it; a diagnostic without [`NON_CONVERGENCE_TAIL`] renders nothing.
pub fn render_warnings(diagnostics: &[SummarizeDiagnostic]) -> Vec<String> {
    diagnostics
        .iter()
        .filter_map(|d| {
            d.message
                .strip_suffix(NON_CONVERGENCE_TAIL)
                .map(|text| format!("{}: {}: {text}", d.severity, d.stage))
        })
        .collect()
}

/// Tarjan over `graph.edges_by_from` (counter `build.graph_scc`).
pub(crate) fn graph_scc(graph: &CombinedGraph) -> SccResult {
    let mut scc_adjacency: HashMap<String, Vec<String>> = HashMap::new();
    for (from, list) in &graph.edges_by_from {
        scc_adjacency.insert(from.clone(), list.iter().map(|e| e.to.clone()).collect());
    }
    pt::counter_delta("build.graph_scc", 1);
    tarjan_scc(&SccInputGraph {
        nodes: &graph.nodes,
        edges_by_from: &scc_adjacency,
    })
}

/// The solver's field-resolution index (keyed (tableId, lowercased field
/// name)), timed by the summaries census. `parameterRoles` need it.
pub(crate) fn solver_field_index(ws: &ModelEntities) -> FieldIndex {
    let t = summaries_census::start();
    let mut field_index: FieldIndex = HashMap::new();
    for table in &ws.tables {
        for field in &table.fields {
            field_index
                .entry((table.id.clone(), field.name.to_lowercase()))
                .or_insert_with(|| field.id.clone());
        }
    }
    summaries_census::add_since(&summaries_census::FIELD_INDEX_NANOS, t);
    summaries_census::add(
        &summaries_census::FIELD_INDEX_ENTRIES,
        field_index.len() as u64,
    );
    field_index
}

/// STRUCTS one-shot: SCC population stats + the largest SCC's intra-edge
/// anatomy (which edge KIND fuses the biggest component, member outdegree).
/// Reimplements the historical `SCCSTATS`/`SCCANATOMY` eprintln probe
/// (`git show c8836e7^:src/engine/l5/detector_context.rs`) as one lazy JSON
/// payload, built only when tracing is enabled.
pub(crate) fn emit_scc_stats(graph: &CombinedGraph, scc: &SccResult) {
    pt::instant_lazy("l4", "scc_stats", || {
        let max_scc = scc.sccs.iter().map(|s| s.members.len()).max().unwrap_or(0);
        let recursive_sccs = scc.sccs.iter().filter(|s| s.recursive).count();
        let recursive_members: usize = scc
            .sccs
            .iter()
            .filter(|s| s.recursive)
            .map(|s| s.members.len())
            .sum();
        let largest_scc = scc.sccs.iter().max_by_key(|s| s.members.len()).map(|big| {
            let member_set: HashSet<&str> = big.members.iter().map(|s| s.as_str()).collect();
            let mut kind_counts: std::collections::BTreeMap<&str, usize> =
                std::collections::BTreeMap::new();
            let mut intra_edges = 0usize;
            let mut max_intra_outdegree = 0usize;
            for m in &big.members {
                let mut out_here = 0usize;
                if let Some(edges) = graph.edges_by_from.get(m) {
                    for e in edges {
                        if member_set.contains(e.to.as_str()) {
                            intra_edges += 1;
                            out_here += 1;
                            *kind_counts.entry(e.kind.as_str()).or_insert(0) += 1;
                        }
                    }
                }
                max_intra_outdegree = max_intra_outdegree.max(out_here);
            }
            json!({
                "members": big.members.len(),
                "intra_edges": intra_edges,
                "max_intra_outdegree": max_intra_outdegree,
                "kinds": kind_counts,
            })
        });
        json!({
            "nodes": graph.nodes.len(),
            "sccs": scc.sccs.len(),
            "recursive_sccs": recursive_sccs,
            "recursive_members": recursive_members,
            "max_scc": max_scc,
            "largest_scc": largest_scc,
        })
    });
}

/// #160 §4.1.5: the `l4.*` buckets, shallow. `l4.scc` only when [`build_l4`]
/// computed the SCC: one handed in is the cross-app base's, counted by
/// `base.combined_scc` (a shared allocation once).
fn census_l4(l4: &L4Analysis, computed_scc: bool) {
    use crate::engine::census::{Kind, Tally};
    let emit = |name: &str, t: &mut Tally| t.emit(name, Kind::Shallow);
    emit("l4.bundle", &mut l4.bundle.census_tally());
    if computed_scc {
        emit("l4.scc", &mut crate::engine::census::scc(&l4.scc));
    }
    emit(
        "l4.summarize_diagnostics",
        Tally::default().buf::<SummarizeDiagnostic>(l4.summarize_diagnostics.len()),
    );
    emit(
        "l4.warnings",
        Tally::default()
            .buf::<String>(l4.warnings.len())
            .strs(&l4.warnings),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::l4::combined_graph::build_combined_graph;

    /// TRANSIENT (spec §5): `build_l4` against the two builders it replaces.
    /// Deleted in Task 27 with `build_db_effect_bundle` and
    /// `cross_app_db_effect_bundle`.
    #[test]
    fn bundle_equals_the_old_builders() {
        // Single-app: ws-d8-commit-in-tx.
        let ws = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/r0-corpus/ws-d8-commit-in-tx");
        let inputs = crate::program::inputs::InputSet::read(&ws, &Default::default());
        let model = crate::program::model::program_calls::assemble_and_resolve_workspace_program(
            &inputs, "r0", false,
        )
        .expect("ws-d8 resolves");
        let old = crate::engine::l4::effect_query_cli::build_db_effect_bundle(&model).2;
        let graph = build_combined_graph(&model.workspace, &model.calls, &model.events.graph);
        let (new, _) = build_l4(
            &model,
            &graph,
            L4Request {
                scc: None,
                extra_diagnostics: vec![],
            },
            None,
        );
        assert!(
            !old.rows_for_test().is_empty(),
            "precondition: single-app rows"
        );
        assert_eq!(
            new.bundle.rows_for_test(),
            old.rows_for_test(),
            "single-app"
        );

        // Cross-app: the #70 fixture (a dependency publisher solved through
        // its subscriber's dispatch, a dependency leaf).
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().join("ws");
        crate::engine::l4::capability_cone::issue70_publisher_roles_tests::write_fixture(&ws);
        let inputs = crate::program::inputs::InputSet::read(&ws, &Default::default());
        let base = crate::engine::l4::capability_cone::build_cross_app_base(&inputs, "r0")
            .expect("cross-app base");
        assert!(
            !base.fixed_roles.is_empty() && !base.leaf_summaries.is_empty(),
            "precondition: the fixture has a fixed-roles publisher and a leaf"
        );
        let old = crate::engine::l4::effect_query_cli::cross_app_db_effect_bundle(&base);
        let (new, core) = build_l4(
            &base.resolved,
            &base.graph,
            L4Request {
                scc: Some(Arc::clone(&base.combined_scc)),
                extra_diagnostics: vec![],
            },
            Some(&base.cross_app_inputs()),
        );
        assert!(Arc::ptr_eq(&new.scc, &base.combined_scc), "SCC by pointer");
        assert!(
            !old.rows_for_test().is_empty(),
            "precondition: cross-app rows"
        );
        assert_eq!(new.bundle.rows_for_test(), old.rows_for_test(), "cross-app");
        // Rows cannot see `cross` (fixed roles change ROLES only): the
        // dependency publisher's path-sensitive role fields stay Unknown only
        // when `build_l4` passes `cross`'s fixed roles (as issue70 (ii)).
        use crate::engine::l4::capability_cone::issue70_publisher_roles_tests::{five, routine_id};
        use crate::engine::l4::effect_lattice::EffectPresence;
        use crate::engine::l4::summary::FieldList;
        let publisher = routine_id(&base, "OnDepRec");
        let roles = &core
            .0
            .get(&publisher)
            .expect("OnDepRec solved")
            .parameter_roles;
        let [role] = roles.as_slice() else {
            panic!("OnDepRec: want one role, got {roles:?}");
        };
        assert_eq!(
            five(role),
            (
                EffectPresence::Unknown,
                FieldList::Unknown,
                EffectPresence::Unknown,
                EffectPresence::Unknown,
                FieldList::Unknown,
            ),
            "OnDepRec's role must stay Unknown (cross's fixed roles)"
        );
    }

    #[test]
    fn warning_text_is_rendered_once() {
        let d = SummarizeDiagnostic::non_convergence(&["a", "b"]);
        assert_eq!(
            render_warnings(&[d]),
            ["warning: summarize: Summary fixed-point did not converge for SCC [a, b]"]
        );
        let other = SummarizeDiagnostic {
            severity: "warning".into(),
            stage: "summarize".into(),
            message: "Summary fixed-point did not converge for SCC [a, b]".into(),
        };
        assert_eq!(render_warnings(&[other]), Vec::<String>::new());
    }
}

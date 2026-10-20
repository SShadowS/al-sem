//! #160 step 2 (spec §4.4.1): the L4 analysis as one value. [`build_l4`] is
//! the one SCC and v2 solve that the detector context, digest, prove and
//! query all take.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use serde_json::json;

use crate::engine::l4::combined_graph::CombinedGraph;
use crate::engine::l4::effect_store::SummaryBundle;
use crate::engine::l4::exec_relation::ExecRelation;
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
/// store and the solver's diagnostics. The field index is a solver input only:
/// a local of [`build_l4`] single-app, the cross-app base's otherwise.
pub struct L4Analysis {
    /// The relation's SCC ([`ExecRelation::scc`], computed in
    /// [`ExecRelation::new`]), shared by pointer: [`build_l4`] runs no Tarjan.
    pub scc: Arc<SccResult>,
    pub bundle: SummaryBundle,
    pub summarize_diagnostics: Vec<SummarizeDiagnostic>,
    /// The non-convergence line(s) [`build_l4`] prints (§4.4.7).
    pub warnings: Vec<String>,
}

/// The solver's per-routine core summaries, UNHARVESTED (their `db_effects`
/// are empty; the rows live in the bundle). The caller harvests or drops them.
pub struct CoreSummaries(pub HashMap<String, RoutineSummary>);

/// What a caller asks [`build_l4`] for. `extra_diagnostics` is appended to
/// `summarize_diagnostics` AFTER `warnings` are rendered, so an injected row is
/// never printed; it is empty in every production caller.
pub struct L4Request {
    pub extra_diagnostics: Vec<SummarizeDiagnostic>,
}

/// The v2 solve over the relation's Primary payload, on the relation's SCC,
/// with the dependency leaves and fixed roles from `cross`. `cross`'s leaf
/// summaries change no output on any fixture (a leaf is a dependency routine
/// with no out-edge, so the solver recomputes its direct rows); `fixed_roles`
/// is pinned by `dependency_publisher_roles_stay_unknown` (ii) and
/// `issue107_dependency_publisher_presence_composes_subscribers`.
// The span names keep the detector context's (`context.*`) on purpose, so
// traces stay comparable; query and digest report them too, and the
// summaries census.
pub(crate) fn build_l4(
    model: &Model,
    rel: &ExecRelation,
    req: L4Request,
    cross: Option<&CrossAppInputs<'_>>,
) -> (L4Analysis, CoreSummaries) {
    let graph: &CombinedGraph = rel.primary();
    let scc = Arc::clone(&rel.scc);
    emit_scc_stats(graph, &scc);

    let ws = &model.workspace;
    let no_leaves: HashMap<String, RoutineSummary> = HashMap::new();
    let no_fixed_roles: HashSet<String> = HashSet::new();
    let leaf_summaries = cross.map_or(&no_leaves, |c| c.leaf_summaries);
    let fixed_roles = cross.map_or(&no_fixed_roles, |c| c.fixed_roles);
    let summaries_span = pt::span("context", "context.compute_summaries");
    // Cross-app the base built (and census-counted) the field index over the
    // same model; single-app it is a local of this solve.
    let own_field_index = cross.is_none().then(|| solver_field_index(ws));
    let field_index = own_field_index
        .as_ref()
        .or(cross.map(|c| c.field_index))
        .expect("a field index: the solve's own, or the cross-app base's");
    let (bundle, core_summaries, diagnostics) = compute_summaries_v2_bundle_with_leaves(
        &ws.routines,
        graph,
        &scc,
        &model.calls.upgraded_bindings,
        field_index,
        leaf_summaries,
        fixed_roles,
    );
    drop(own_field_index);
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
        census_l4(&l4);
    }
    // The ONE print site of a solve's non-convergence line (§4.4.7); the
    // injected `extra_diagnostics` were never rendered, so never print.
    for w in &l4.warnings {
        eprintln!("{w}");
    }
    (l4, CoreSummaries(core_summaries))
}

/// The line each non-convergence diagnostic stands for, as [`build_l4`]
/// prints it; a diagnostic without [`NON_CONVERGENCE_TAIL`] renders nothing.
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

/// Tarjan over `graph.edges_by_from` (counter `build.graph_scc`): the one
/// copy, also behind the cross-app base's SCC. The snapshot's coverage fold no
/// longer solves, so it no longer calls this (#160 step 3).
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
/// name)). `parameterRoles` need it.
pub(crate) fn field_index(ws: &ModelEntities) -> FieldIndex {
    let mut field_index: FieldIndex = HashMap::new();
    for table in &ws.tables {
        for field in &table.fields {
            field_index
                .entry((table.id.clone(), field.name.to_lowercase()))
                .or_insert_with(|| field.id.clone());
        }
    }
    field_index
}

/// [`field_index`], timed and counted by the summaries census: the index a
/// [`build_l4`] solve reads (single-app its own, cross-app the base's).
pub(crate) fn solver_field_index(ws: &ModelEntities) -> FieldIndex {
    let t = summaries_census::start();
    let field_index = field_index(ws);
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
fn emit_scc_stats(graph: &CombinedGraph, scc: &SccResult) {
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

/// #160 §4.1.5: the `l4.*` buckets, shallow. No `l4.scc`: the SCC is the
/// relation's, counted once by `rel.scc` (#160 step 4).
fn census_l4(l4: &L4Analysis) {
    use crate::engine::census::{Kind, Tally};
    let emit = |name: &str, t: &mut Tally| t.emit(name, Kind::Shallow);
    emit("l4.bundle", &mut l4.bundle.census_tally());
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

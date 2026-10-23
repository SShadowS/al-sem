//! #160 step 4 (epic §5.5): [`ExecRelation`], the one execution relation a
//! view builds: the combined graph, its SCC, and the dependency half's
//! uncertainty edges, from one call split.

use std::sync::Arc;

use super::combined_graph::{PrimaryGraph, TypedEdge, UncertaintyEdge, assemble};
use super::scc::SccResult;
use crate::engine::perf_trace as pt;
use crate::program::model::calls::CallEdge;
use crate::program::model::events::EventGraph;
use crate::program::model::workspace::{Model, ModelEntities};

/// One view's execution relation (#160 step 4, epic §5.5). The two transfer
/// rules are the two accessors; spec §4.3's field table says which consumer
/// reads which.
///
/// The carrier is split (#160 step 11b §4.4, amendments M1, M2): the Primary
/// payload is a [`PrimaryGraph`] (today's field names, no typed edges) and the
/// Cone payload a `Vec<TypedEdge>`, moved out of the assembled graph in
/// [`ExecRelation::new`].
///
/// The transfer rules are two accessors, not an `enum Transfer` value (M2):
/// each consumer's rule is fixed. The fields are private, so every reader
/// names its rule: [`ExecRelation::cone`] or [`ExecRelation::primary`] (plus
/// [`ExecRelation::scc`] and [`ExecRelation::dependency_uncertainty`]). The
/// type guarantee holds in both directions: the cone takes `&[TypedEdge]`, so
/// it cannot read the Primary payload, and `primary()` has no typed edges.
pub struct ExecRelation {
    /// Primary payload: `edges_by_from` / `edges_from_order` /
    /// `uncertainty_edges` / `nodes`.
    primary: Arc<PrimaryGraph>,
    /// Cone payload: the primary half's typed edges (event dispatch included),
    /// then the dependency half's, in that order (Decision 43). Shared by
    /// pointer with the detector artifacts (d43 reads it).
    typed: Arc<Vec<TypedEdge>>,
    /// Tarjan over the Primary payload, computed in [`ExecRelation::new`].
    scc: Arc<SccResult>,
    /// The dependency half's uncertainty edges, in the order the half-build
    /// sorts them. Read by the coverage fold through
    /// [`ExecRelation::uncertainty_all`] (#160 step 5) and by the census.
    dependency_uncertainty: Vec<UncertaintyEdge>,
    /// The calls dependency routines make (the `dependency` argument of
    /// [`ExecRelation::new`], moved in), in emission order. Read only by the
    /// snapshot tree's call-site ledger (#160 step 6 D6, Decision 18).
    dependency_calls: Vec<CallEdge>,
}

impl ExecRelation {
    /// The single-app relation over `model`: every call is a workspace
    /// caller's, so the dependency half is empty. Prints its `rel.*` census
    /// buckets here, where the single-app context graph's `context.graph`
    /// bucket printed before step 4.
    pub fn single_app(model: &Model) -> ExecRelation {
        let rel = ExecRelation::new(
            &model.workspace,
            &model.calls.edges,
            Vec::new(),
            &model.events.graph,
        );
        rel.emit_census();
        rel
    }

    /// Build the relation from one call split (spec §4.2). `primary` holds the
    /// calls of WORKSPACE callers, `dependency` the calls dependency routines
    /// make (empty single-app). The Primary payload is assembled over
    /// `primary` plus every event dispatch in `events`; the dependency half is
    /// assembled over an empty event graph (its dispatch is already in the
    /// primary half), and only its typed edges (appended to the Cone payload)
    /// and its uncertainty edges (kept apart) survive. Counts one
    /// `build.combined_graph`, however many halves it assembles. `dependency`
    /// is kept (moved in) for [`ExecRelation::dependency_calls`].
    pub fn new(
        ws: &ModelEntities,
        primary: &[CallEdge],
        dependency: Vec<CallEdge>,
        events: &EventGraph,
    ) -> ExecRelation {
        pt::counter_delta("build.combined_graph", 1);
        let _s = pt::span("rel", "rel.build");
        let graph = assemble(ws, primary, events);
        let (primary, mut typed) = (graph.primary, graph.typed_edges);
        let mut dependency_uncertainty = Vec::new();
        if !dependency.is_empty() {
            let d = assemble(ws, &dependency, &EventGraph::default());
            typed.extend(d.typed_edges);
            dependency_uncertainty = d.primary.uncertainty_edges;
        }
        // `graph_scc` reads `nodes` and `edges_by_from` only.
        let scc = super::analysis::graph_scc(&primary);
        ExecRelation {
            primary: Arc::new(primary),
            typed: Arc::new(typed),
            scc: Arc::new(scc),
            dependency_uncertainty,
            dependency_calls: dependency,
        }
    }

    /// The `rel.graph` / `rel.scc` / `rel.dependency_uncertainty` census
    /// buckets, shallow, when the census is on. Not printed by `new`: the
    /// owner prints them at the phase its old buckets printed (cross-app the
    /// base, after `census.base`; single-app [`ExecRelation::single_app`]).
    pub fn emit_census(&self) {
        if !crate::engine::census::enabled() {
            return;
        }
        use crate::engine::census::{Kind, Tally, combined_graph, scc as scc_tally};
        combined_graph(&self.primary, &self.typed).emit("rel.graph", Kind::Shallow);
        scc_tally(&self.scc).emit("rel.scc", Kind::Shallow);
        Tally::default()
            .buf::<UncertaintyEdge>(self.dependency_uncertainty.len())
            .emit("rel.dependency_uncertainty", Kind::Shallow);
        Tally::default()
            .buf::<CallEdge>(self.dependency_calls.len())
            .emit("rel.dependency_calls", Kind::Shallow);
    }

    /// The calls dependency routines make (empty single-app): the snapshot
    /// tree's call-site ledger reads them after the model's own calls (D6).
    pub fn dependency_calls(&self) -> &[CallEdge] {
        &self.dependency_calls
    }

    /// `Transfer::Cone`: every typed edge, every caller.
    pub fn cone(&self) -> &[TypedEdge] {
        &self.typed
    }

    /// The Cone payload, shared by pointer (`DetectorArtifacts::cone`).
    pub fn cone_shared(&self) -> &Arc<Vec<TypedEdge>> {
        &self.typed
    }

    /// `Transfer::Primary`: primary callers' call edges plus event dispatch
    /// from any publisher.
    pub fn primary(&self) -> &Arc<PrimaryGraph> {
        &self.primary
    }

    /// The Primary payload's SCC, shared by pointer with every `build_l4`.
    pub fn scc(&self) -> &Arc<SccResult> {
        &self.scc
    }

    /// The dependency half's uncertainty edges. The coverage fold reads them
    /// through [`ExecRelation::uncertainty_all`].
    pub fn dependency_uncertainty(&self) -> &[UncertaintyEdge] {
        &self.dependency_uncertainty
    }

    /// The uncertainty edges of every caller the Cone carries: the Primary half,
    /// then the dependency half. Read by the coverage fold only; the solve and
    /// the context's indexes read `primary().uncertainty_edges`.
    pub fn uncertainty_all(&self) -> impl Iterator<Item = &UncertaintyEdge> {
        self.primary
            .uncertainty_edges
            .iter()
            .chain(&self.dependency_uncertainty)
    }
}

/// Spec §5.1: the relation equals today's cross-app base recipe field by
/// field. The oracle ([`tests::legacy_relation`], over `build_combined_graph`)
/// and the relation share one assembler, so this guards the COMPOSITION (the
/// split, the order of the halves, which events each half sees, the halves
/// kept apart), not the assembler itself. The independent-code comparison,
/// with the assembler copied, ran on the corpus and on CDO before the
/// assembler was shared (the issue ledger records it).
#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::l4::combined_graph::{CombinedGraph, build_combined_graph};
    use crate::program::model::calls::ResolvedCalls;
    use crate::program::model::program_calls::{
        assemble_and_resolve_cross_app_program, assemble_and_resolve_workspace_program,
    };
    use crate::program::model::workspace::{MODEL_INSTANCE_ID_DEFAULT, Model};
    use std::collections::BTreeSet;
    use std::path::{Path, PathBuf};

    /// Today's cross-app base recipe (`capability_cone.rs`
    /// `build_cross_app_base`: the cut, the primary graph, its SCC, the
    /// dependency half-build appended to the typed edges), returning the
    /// dependency half's uncertainty edges it drops. With `dep_ids` empty it is
    /// the single-app context graph (`build_combined_graph` over every call)
    /// plus an empty half.
    fn legacy_relation(
        ws: &ModelEntities,
        all_calls: &ResolvedCalls,
        events: &EventGraph,
        dep_ids: &BTreeSet<String>,
    ) -> (CombinedGraph, SccResult, Vec<UncertaintyEdge>) {
        let calls = ResolvedCalls {
            edges: all_calls
                .edges
                .iter()
                .filter(|e| !dep_ids.contains(&e.from))
                .cloned()
                .collect(),
            upgraded_bindings: all_calls.upgraded_bindings.clone(),
            diagnostics: all_calls.diagnostics.clone(),
            external_targets: all_calls.external_targets.clone(),
        };
        let mut graph = build_combined_graph(ws, &calls, events);
        let scc = crate::engine::l4::analysis::graph_scc(&graph);
        let dep_calls = ResolvedCalls {
            edges: all_calls
                .edges
                .iter()
                .filter(|e| dep_ids.contains(&e.from))
                .cloned()
                .collect(),
            upgraded_bindings: all_calls.upgraded_bindings.clone(),
            diagnostics: Vec::new(),
            external_targets: all_calls.external_targets.clone(),
        };
        let no_events = EventGraph {
            events: Vec::new(),
            edges: Vec::new(),
        };
        let half = build_combined_graph(ws, &dep_calls, &no_events);
        graph.typed_edges.extend(half.typed_edges);
        (graph, scc, half.primary.uncertainty_edges)
    }

    /// `field: …` naming the first index where `new` and `legacy` differ.
    fn diff_vec<T: PartialEq + std::fmt::Debug>(
        field: &str,
        new: &[T],
        legacy: &[T],
    ) -> Option<String> {
        if new == legacy {
            return None;
        }
        let i = new
            .iter()
            .zip(legacy)
            .position(|(a, b)| a != b)
            .unwrap_or(new.len().min(legacy.len()));
        Some(format!(
            "{field}: len new {} legacy {}; first difference at {i}: new {:?} legacy {:?}",
            new.len(),
            legacy.len(),
            new.get(i),
            legacy.get(i)
        ))
    }

    #[derive(Default)]
    struct Stats {
        cases: usize,
        dep_half_with_dispatch: usize,
        dep_uncertainty: usize,
    }

    /// One case: the relation over the split of `calls` by `dep_ids` against
    /// [`legacy_relation`], each field compared separately.
    fn check_case(
        where_: &str,
        ws: &ModelEntities,
        calls: &ResolvedCalls,
        events: &EventGraph,
        dep_ids: &BTreeSet<String>,
        stats: &mut Stats,
        failures: &mut Vec<String>,
    ) {
        let (lg, lscc, lunc) = legacy_relation(ws, calls, events, dep_ids);
        let (dependency, primary): (Vec<CallEdge>, Vec<CallEdge>) = calls
            .edges
            .iter()
            .cloned()
            .partition(|e| dep_ids.contains(&e.from));
        // The preconditions count the ORACLE's output, never the relation's,
        // so a break in `new` shows as a field diff below.
        stats.cases += 1;
        let oracle_dispatches = lg
            .edges_by_from
            .values()
            .flatten()
            .any(|e| e.kind == "event-dispatch");
        if !dependency.is_empty() && oracle_dispatches {
            stats.dep_half_with_dispatch += 1;
        }
        if !lunc.is_empty() {
            stats.dep_uncertainty += 1;
        }
        let rel = ExecRelation::new(ws, &primary, dependency.clone(), events);
        let g = &*rel.primary;
        let mut diffs: Vec<String> = Vec::new();
        diffs.extend(diff_vec("nodes", &g.nodes, &lg.nodes));
        let keys: BTreeSet<&String> = g
            .edges_by_from
            .keys()
            .chain(lg.edges_by_from.keys())
            .collect();
        for k in keys {
            let empty = Vec::new();
            if let Some(d) = diff_vec(
                &format!("edges_by_from[{k}]"),
                g.edges_by_from.get(k).unwrap_or(&empty),
                lg.edges_by_from.get(k).unwrap_or(&empty),
            ) {
                diffs.push(d);
                break;
            }
        }
        if g.edges_by_from.len() != lg.edges_by_from.len() {
            diffs.push(format!(
                "edges_by_from: {} keys new, {} legacy",
                g.edges_by_from.len(),
                lg.edges_by_from.len()
            ));
        }
        diffs.extend(diff_vec(
            "edges_from_order",
            &g.edges_from_order,
            &lg.edges_from_order,
        ));
        diffs.extend(diff_vec(
            "uncertainty_edges",
            &g.uncertainty_edges,
            &lg.uncertainty_edges,
        ));
        diffs.extend(diff_vec("typed_edges", &rel.typed, &lg.typed_edges));
        if *rel.scc != lscc {
            diffs.push(format!(
                "scc: {} sccs new, {} legacy",
                rel.scc.sccs.len(),
                lscc.sccs.len()
            ));
        }
        diffs.extend(diff_vec(
            "dependency_uncertainty",
            &rel.dependency_uncertainty,
            &lunc,
        ));
        failures.extend(diffs.into_iter().map(|d| format!("{where_}: {d}")));
    }

    fn workspace_model(dir: &Path) -> Option<Model> {
        assemble_and_resolve_workspace_program(
            &crate::program::inputs::InputSet::read(dir, &Default::default()),
            MODEL_INSTANCE_ID_DEFAULT,
            false,
        )
    }

    /// Both arms on one workspace: single-app with no cut (the single-app
    /// path never cuts, even when the model carries dependency rows), and
    /// cross-app with the base's split (routines whose `app_guid` differs from
    /// the primary app's). A dir that builds no model fails.
    fn check_dir(dir: &Path, where_: &str, stats: &mut Stats, failures: &mut Vec<String>) {
        match workspace_model(dir) {
            Some(m) => check_case(
                where_,
                &m.workspace,
                &m.calls,
                &m.events.graph,
                &BTreeSet::new(),
                stats,
                failures,
            ),
            None => failures.push(format!("{where_}: no single-app model built")),
        }
        let inputs = crate::program::inputs::InputSet::read(dir, &Default::default());
        let Ok(x) =
            assemble_and_resolve_cross_app_program(&inputs, MODEL_INSTANCE_ID_DEFAULT, false)
        else {
            failures.push(format!("{where_}: no cross-app model built"));
            return;
        };
        if x.resolved.primary_app.is_none() {
            failures.push(format!("{where_}: cross-app model has no primary app"));
            return;
        }
        let ws = &x.resolved.workspace;
        let dep_ids: BTreeSet<String> = ws
            .routines
            .iter()
            .filter(|r| x.resolved.routine_role(r) == crate::program::model::role::Role::Dependency)
            .map(|r| r.id.clone())
            .collect();
        check_case(
            &format!("{where_} (cross-app)"),
            ws,
            &x.resolved.calls,
            &x.resolved.events.graph,
            &dep_ids,
            stats,
            failures,
        );
    }

    /// Sorted dirs under `root`, at any depth, holding an `app.json`.
    fn app_dirs(root: &Path) -> Vec<PathBuf> {
        let mut dirs: Vec<PathBuf> = walkdir::WalkDir::new(root)
            .into_iter()
            .map(|e| e.unwrap().into_path())
            .filter(|p| p.join("app.json").is_file())
            .collect();
        dirs.sort();
        dirs
    }

    /// Spec §5.1 on every app dir under step 3's five roots plus the §5.2 and
    /// §5.4 inline fixtures, both arms each.
    #[test]
    fn relation_equals_the_legacy_base_per_field() {
        let tests = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
        let mut stats = Stats::default();
        let mut failures = Vec::new();
        // The count each root holds today, an intentional tripwire: a dir that
        // stops being visited fails here (and the case count below). A NEW
        // corpus dir fails it too: bump the count in `FIXTURE_ROOTS`.
        use crate::engine::l4::capability_cone::callsite_local_scan_tests::FIXTURE_ROOTS;
        for (root, want) in FIXTURE_ROOTS {
            let dirs = app_dirs(&tests.join(root));
            assert_eq!(dirs.len(), want, "{root}: app dirs");
            for dir in &dirs {
                check_dir(dir, &dir.display().to_string(), &mut stats, &mut failures);
            }
        }
        let tmp = tempfile::tempdir().unwrap();
        let inline = tmp.path().join("ws");
        crate::engine::l5::test_support::write_fixture_step4(&inline);
        let before = stats.dep_half_with_dispatch;
        check_dir(&inline, "step4 inline", &mut stats, &mut failures);
        assert!(
            stats.dep_half_with_dispatch > before,
            "precondition: the step4 inline fixture has a dependency half and event dispatch"
        );
        // The §5.4 witness too: no corpus dir has dependency uncertainty, so
        // without it a break that mixes the halves' uncertainty would pass.
        let inline_unc = tmp.path().join("ws-unc");
        crate::engine::l5::test_support::write_fixture_dep_uncertainty(&inline_unc);
        let before = stats.dep_uncertainty;
        check_dir(
            &inline_unc,
            "step4 dependency uncertainty",
            &mut stats,
            &mut failures,
        );
        assert!(
            stats.dep_uncertainty > before,
            "precondition: the dependency-uncertainty fixture keeps dependency uncertainty"
        );
        println!(
            "exec_relation parity: {} cases, {} with a dependency half and event dispatch, {} with dependency uncertainty",
            stats.cases, stats.dep_half_with_dispatch, stats.dep_uncertainty
        );
        let dirs: usize = FIXTURE_ROOTS.iter().map(|(_, n)| n).sum();
        assert_eq!(stats.cases, 2 * (dirs + 2), "cases run");
        assert!(failures.is_empty(), "\n{}", failures.join("\n"));
    }

    /// Spec §5.1 (S2-1): `ws-txn-d47-event-neg-isolated` keeps an isolated
    /// publisher WITH a subscriber. Fails, never skips. The `isolated`
    /// discrimination (`DispatchMeta.isolated` forced to `false` fails the
    /// parity test's `edges_by_from` on this dir) was run while the assembler
    /// was a copy; on the final tree the oracle shares the assembler, so it
    /// cannot be repeated here. This test keeps the fixture shape alive for
    /// the goldens that still see it.
    #[test]
    fn isolated_dispatch_precondition() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/r0-corpus/ws-txn-d47-event-neg-isolated");
        let m = workspace_model(&dir).expect("a single-app model");
        let g = &m.events.graph;
        let isolated_with_subscriber = g
            .events
            .iter()
            .filter(|e| e.isolated == Some(true))
            .any(|e| g.edges.iter().any(|ee| ee.event_id == e.id));
        assert!(
            isolated_with_subscriber,
            "no isolated event with a subscriber in {}",
            dir.display()
        );
    }

    /// Spec §5.1, the CDO half (skips without `CDO_WS`, panics under
    /// `ENFORCE_CDO_WS=1`): both models of the pinned CDO workspace.
    #[test]
    fn relation_equals_the_legacy_base_on_cdo() {
        let Some(ws) = crate::cdo::cdo_ws_or_enforce() else {
            return;
        };
        let start = std::time::Instant::now();
        let mut stats = Stats::default();
        let mut failures = Vec::new();
        check_dir(&ws, "CDO", &mut stats, &mut failures);
        println!(
            "exec_relation parity on CDO: {} cases, {} with a dependency half and event dispatch, {} with dependency uncertainty, {:.1?}",
            stats.cases,
            stats.dep_half_with_dispatch,
            stats.dep_uncertainty,
            start.elapsed()
        );
        assert_eq!(stats.cases, 2, "both CDO models ran");
        assert!(failures.is_empty(), "\n{}", failures.join("\n"));
    }
}

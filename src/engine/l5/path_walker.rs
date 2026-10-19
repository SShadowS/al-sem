//! `walkEvidence` — bounded depth-first evidence walk. Port of al-sem
//! `src/engine/path-walker.ts`.
//!
//! Returns one [`WalkResult`] per branch that reached a terminal (`Complete`) or
//! stopped (`CycleCut` / `DepthCut` / `NodeBudgetCut` / `DeadEnd`). Pure; cycle
//! detection is per-path; bounds cap routine-path depth and total nodes visited.
//!
//! NO CONSUMER until a later wave (R4-D+). It is validated by native oracles only:
//! bounds cuts (depth + node-budget), cycle-cut, dead-end, terminal completion.
//! The [`WalkPolicy`] is supplied by the (future) detector; here it is a trait the
//! oracles implement with synthetic graphs.
//!
//! ## Uncertainty accumulation
//! Each [`WalkResult`] carries the deduplicated union of all per-node uncertainties
//! collected along the path. The per-node source (`uncertainties_by_node` parameter
//! to [`walk_evidence`]) is a caller-supplied map from routine id to the merged set
//! of uncertainties for that node; wiring it from `routine.summary.uncertainties ∪
//! uncertaintyEdgesByFrom` is deferred to the R4-D call site, exactly as
//! `entry_points::find_reachable_roots` takes `access_modifiers` as an explicit
//! input. Nodes absent from the map contribute no uncertainties.

use crate::engine::l4::combined_graph::CombinedEdge;
use crate::engine::l4::summary::{Uncertainty, dedupe_uncertainties};
#[cfg(test)]
use crate::engine::l5::detector_context::OwnedUncertainties;
use crate::engine::l5::detector_context::UncertaintyView;
use crate::engine::l5::finding::EvidenceStep;

/// A real op site the walk can terminate at. Policies may carry richer data in
/// `local_loop_depth`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Terminal {
    pub routine_id: String,
    /// Loop nesting depth of the op site within its OWN routine.
    pub local_loop_depth: i64,
    /// Optional detector-supplied op identity carried alongside the terminal —
    /// the Rust analogue of al-sem's `D1Terminal extends Terminal { op }`. The
    /// generic substrate ignores it; a policy's `build_terminal_step` reads it to
    /// recover the exact terminating op (its anchor / note / operationId). `None`
    /// for policies that don't carry an op (the synthetic oracles).
    pub op_id: Option<String>,
}

/// Why a walk branch stopped. Detectors emit findings only from `Complete`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WalkStop {
    Complete,
    CycleCut,
    DepthCut,
    NodeBudgetCut,
    DeadEnd,
}

/// One branch result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WalkResult {
    pub path: Vec<EvidenceStep>,
    pub effective_loop_depth: i64,
    /// Deduplicated union of all per-node uncertainties collected along this
    /// path, sorted by `uncertainty_key`. Mirrors al-sem's `WalkResult.uncertainties`.
    pub uncertainties: Vec<Uncertainty>,
    pub stop: WalkStop,
}

/// Hot-tier (`Detail::Hot`) aggregate counters for a run of [`walk_evidence`]
/// calls — a plain-`u64`, `LocalCounters`-style struct threaded as
/// `Option<&mut WalkTraceStats>`. The caller tests `perf_trace::enabled(Hot)`
/// ONCE (outside every walk) and passes `Some(&mut stats)`; `None` = zero
/// overhead (a single moved-`Option` check per walk / per `visit`, no
/// `enabled()` call and no allocation inside the walk). See the Hot-loop rule in
/// the tracing spec.
///
/// The result-by-stop-kind counts (`complete` .. `dead_end`) are the population
/// the d1 memo RETAINS — d1 stores every canonical walk's whole result vec and
/// only ever consumes the `Complete` ones — so
/// `unused_cut = (all - complete)` and `all = complete + cycle_cut + depth_cut +
/// node_budget_cut + dead_end` make the decisive
/// `unused_cut_results / all_retained_results` ratio computable from the trace
/// alone.
#[derive(Debug, Default, Clone)]
pub struct WalkTraceStats {
    /// Total `visit` invocations across all walks (nodes touched, incl. revisits
    /// of distinct paths — the walk's own budget metric, summed per walk).
    pub nodes_visited: u64,
    /// Total outgoing edges enumerated across all `visit`s (fan-out cost).
    pub edges_examined: u64,
    pub complete: u64,
    pub cycle_cut: u64,
    pub depth_cut: u64,
    pub node_budget_cut: u64,
    pub dead_end: u64,
    /// Number of [`walk_evidence`] invocations counted.
    pub walks: u64,
    /// Walks whose `nodes_visited` reached the node budget (`max_nodes`).
    pub walks_hit_node_bound: u64,
}

/// The mutable context threaded through one walk branch.
#[derive(Debug, Clone)]
pub struct PathCtx {
    pub routine_path: Vec<String>,
    pub inherited_loop_depth: i64,
    pub steps: Vec<EvidenceStep>,
    /// Accumulated deduplicated uncertainties inherited from the walk root up to
    /// (and including) the current node. Mirrors al-sem's `PathCtx.uncertainties`.
    pub uncertainties: Vec<Uncertainty>,
}

/// Depth + node-visit bounds.
#[derive(Debug, Clone, Copy)]
pub struct WalkBounds {
    /// Max routine-path length.
    pub max_depth: usize,
    /// Max nodes visited across the whole walk.
    pub max_nodes: usize,
}

impl Default for WalkBounds {
    /// al-sem's default bounds: maxDepth 20, maxNodes 500.
    fn default() -> Self {
        WalkBounds {
            max_depth: 20,
            max_nodes: 500,
        }
    }
}

/// Detector-supplied policy: which edges to follow, what counts as a terminal,
/// how to build steps. (The al-sem `WalkPolicy` interface.)
pub trait WalkPolicy {
    fn terminals_at(&self, node: &str, ctx: &PathCtx) -> Vec<Terminal>;
    fn expand(&self, node: &str, ctx: &PathCtx) -> Vec<CombinedEdge>;
    fn build_hop_step(&self, edge: &CombinedEdge, ctx: &PathCtx) -> EvidenceStep;
    fn build_terminal_step(&self, terminal: &Terminal, ctx: &PathCtx) -> EvidenceStep;
    /// Loop-depth contributed by traversing this edge (al-sem `loopDepthOfEdge`,
    /// the callsite's `loopStack.length`). The policy owns the callsite lookup.
    fn loop_depth_of_edge(&self, edge: &CombinedEdge) -> i64;
}

/// Options: an initial loop depth + initial steps the detector wants prepended.
#[derive(Debug, Clone, Default)]
pub struct WalkOpts {
    pub initial_loop_depth: i64,
    pub initial_steps: Vec<EvidenceStep>,
}

/// Bounded depth-first evidence walk. See module docs.
///
/// `uncertainties_by_node` maps each routine id to its merged per-node
/// uncertainty set (`routine.summary.uncertainties ∪ uncertaintyEdgesByFrom`).
/// Wiring this map from the real model/graph is deferred to the R4-D call site;
/// pass an empty map when uncertainties are not under test.
///
/// The value is an `Arc<[Uncertainty]>` because
/// [`DetectorContext::uncertainties_by_node`](crate::engine::l5::detector_context::DetectorContext::uncertainties_by_node)
/// hash-conses equal sets across nodes; the walk reads them through
/// [`UncertaintyView`], which resolves a node's interned set on demand. The
/// sharing is invisible here and cannot change what the walk accumulates.
pub fn walk_evidence<P: WalkPolicy>(
    start: &str,
    policy: &P,
    bounds: WalkBounds,
    opts: WalkOpts,
    uncertainties: UncertaintyView<'_>,
    mut stats: Option<&mut WalkTraceStats>,
) -> Vec<WalkResult> {
    let mut results: Vec<WalkResult> = Vec::new();
    let mut nodes_visited: usize = 0;

    visit(
        start,
        PathCtx {
            routine_path: vec![start.to_string()],
            inherited_loop_depth: opts.initial_loop_depth,
            steps: opts.initial_steps,
            uncertainties: Vec::new(),
        },
        policy,
        bounds,
        &mut nodes_visited,
        &mut results,
        uncertainties,
        stats.as_deref_mut(),
    );

    // Hot-tier aggregate tally (`Detail::Hot`). `stats` is `Some` only when the
    // caller opted in (tested `enabled(Hot)` ONCE, outside the walk). Node,
    // walk, and stop-kind totals are derived HERE from the finished walk (no
    // per-node threading needed); `edges_examined` is the one inline count
    // `visit` accumulates. `None` callers skip the whole block.
    if let Some(s) = stats {
        s.walks += 1;
        s.nodes_visited += nodes_visited as u64;
        if nodes_visited >= bounds.max_nodes {
            s.walks_hit_node_bound += 1;
        }
        for r in &results {
            match r.stop {
                WalkStop::Complete => s.complete += 1,
                WalkStop::CycleCut => s.cycle_cut += 1,
                WalkStop::DepthCut => s.depth_cut += 1,
                WalkStop::NodeBudgetCut => s.node_budget_cut += 1,
                WalkStop::DeadEnd => s.dead_end += 1,
            }
        }
    }

    results
}

#[allow(clippy::too_many_arguments)]
fn visit<P: WalkPolicy>(
    node: &str,
    ctx: PathCtx,
    policy: &P,
    bounds: WalkBounds,
    nodes_visited: &mut usize,
    results: &mut Vec<WalkResult>,
    uncertainties: UncertaintyView<'_>,
    mut stats: Option<&mut WalkTraceStats>,
) {
    *nodes_visited += 1;

    // Build ctx_here: deduplicate (inherited ++ this node's own uncertainties).
    // Mirrors al-sem path-walker.ts line 117-120.
    let combined: Vec<Uncertainty> = ctx
        .uncertainties
        .iter()
        .cloned()
        .chain(uncertainties.values_of(node).cloned())
        .collect();
    let ctx_here = PathCtx {
        routine_path: ctx.routine_path.clone(),
        inherited_loop_depth: ctx.inherited_loop_depth,
        steps: ctx.steps.clone(),
        uncertainties: dedupe_uncertainties(combined),
    };

    let terminals = policy.terminals_at(node, &ctx_here);
    for t in &terminals {
        let mut path = ctx_here.steps.clone();
        path.push(policy.build_terminal_step(t, &ctx_here));
        results.push(WalkResult {
            path,
            effective_loop_depth: ctx_here.inherited_loop_depth + t.local_loop_depth,
            uncertainties: ctx_here.uncertainties.clone(),
            stop: WalkStop::Complete,
        });
    }

    let edges = policy.expand(node, &ctx_here);
    if let Some(s) = stats.as_deref_mut() {
        s.edges_examined += edges.len() as u64;
    }
    if edges.is_empty() && terminals.is_empty() {
        results.push(WalkResult {
            path: ctx_here.steps,
            effective_loop_depth: ctx_here.inherited_loop_depth,
            uncertainties: ctx_here.uncertainties,
            stop: WalkStop::DeadEnd,
        });
        return;
    }

    for edge in &edges {
        if *nodes_visited >= bounds.max_nodes {
            results.push(WalkResult {
                path: ctx_here.steps.clone(),
                effective_loop_depth: ctx_here.inherited_loop_depth,
                uncertainties: ctx_here.uncertainties.clone(),
                stop: WalkStop::NodeBudgetCut,
            });
            continue;
        }
        if ctx_here.routine_path.iter().any(|r| r == &edge.to) {
            results.push(WalkResult {
                path: ctx_here.steps.clone(),
                effective_loop_depth: ctx_here.inherited_loop_depth,
                uncertainties: ctx_here.uncertainties.clone(),
                stop: WalkStop::CycleCut,
            });
            continue;
        }
        if ctx_here.routine_path.len() >= bounds.max_depth {
            results.push(WalkResult {
                path: ctx_here.steps.clone(),
                effective_loop_depth: ctx_here.inherited_loop_depth,
                uncertainties: ctx_here.uncertainties.clone(),
                stop: WalkStop::DepthCut,
            });
            continue;
        }
        let mut child_path = ctx_here.routine_path.clone();
        child_path.push(edge.to.clone());
        let mut child_steps = ctx_here.steps.clone();
        child_steps.push(policy.build_hop_step(edge, &ctx_here));
        let child_ctx = PathCtx {
            routine_path: child_path,
            inherited_loop_depth: ctx_here.inherited_loop_depth + policy.loop_depth_of_edge(edge),
            steps: child_steps,
            uncertainties: ctx_here.uncertainties.clone(),
        };
        visit(
            &edge.to,
            child_ctx,
            policy,
            bounds,
            nodes_visited,
            results,
            uncertainties,
            stats.as_deref_mut(),
        );
    }
}

// ===========================================================================
// Native oracles — ground-truth-free invariants on synthetic graphs.
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal source anchor for synthetic evidence steps.
    fn anchor(node: &str) -> crate::engine::l5::finding::SourceAnchor {
        crate::engine::l5::finding::SourceAnchor {
            source_unit_id: "ws:x.al".to_string(),
            start_line: 0,
            start_column: 0,
            end_line: 0,
            end_column: 0,
            enclosing_routine_id: node.to_string(),
            syntax_kind: "x".to_string(),
            normalized_text_hash: None,
            leading_context_hash: None,
            trailing_context_hash: None,
        }
    }

    fn step(node: &str, note: &str) -> EvidenceStep {
        EvidenceStep {
            routine_id: node.to_string(),
            operation_id: None,
            callsite_id: None,
            loop_id: None,
            source_anchor: anchor(node),
            note: note.to_string(),
        }
    }

    fn edge(from: &str, to: &str) -> CombinedEdge {
        CombinedEdge {
            from: from.to_string(),
            to: to.to_string(),
            kind: "direct".to_string(),
            callsite_id: Some(format!("{from}/cs0")),
            operation_id: None,
            event_id: None,
            subscriber_app_id: None,
            dispatch: None,
            resolution: "static".to_string(),
        }
    }

    /// A policy over a static adjacency map. Each node terminates iff it is in
    /// `terminal_nodes`; each edge contributes `edge_loop_depth`.
    struct MapPolicy {
        adjacency: std::collections::HashMap<String, Vec<String>>,
        terminal_nodes: std::collections::HashSet<String>,
        edge_loop_depth: i64,
    }

    impl WalkPolicy for MapPolicy {
        fn terminals_at(&self, node: &str, _ctx: &PathCtx) -> Vec<Terminal> {
            if self.terminal_nodes.contains(node) {
                vec![Terminal {
                    routine_id: node.to_string(),
                    local_loop_depth: 0,
                    op_id: None,
                }]
            } else {
                Vec::new()
            }
        }
        fn expand(&self, node: &str, _ctx: &PathCtx) -> Vec<CombinedEdge> {
            self.adjacency
                .get(node)
                .map(|tos| tos.iter().map(|to| edge(node, to)).collect())
                .unwrap_or_default()
        }
        fn build_hop_step(&self, e: &CombinedEdge, _ctx: &PathCtx) -> EvidenceStep {
            step(&e.to, "hop")
        }
        fn build_terminal_step(&self, t: &Terminal, _ctx: &PathCtx) -> EvidenceStep {
            step(&t.routine_id, "terminal")
        }
        fn loop_depth_of_edge(&self, _e: &CombinedEdge) -> i64 {
            self.edge_loop_depth
        }
    }

    fn map_policy(edges: &[(&str, &str)], terminals: &[&str], edge_loop_depth: i64) -> MapPolicy {
        let mut adjacency: std::collections::HashMap<String, Vec<String>> =
            std::collections::HashMap::new();
        for (from, to) in edges {
            adjacency
                .entry(from.to_string())
                .or_default()
                .push(to.to_string());
        }
        MapPolicy {
            adjacency,
            terminal_nodes: terminals.iter().map(|s| s.to_string()).collect(),
            edge_loop_depth,
        }
    }

    fn no_uncertainties() -> OwnedUncertainties {
        OwnedUncertainties::default()
    }

    fn make_uncertainty(kind: &str, callsite_id: Option<&str>) -> Uncertainty {
        Uncertainty {
            kind: kind.to_string(),
            callsite_id: callsite_id.map(str::to_string),
            operation_id: None,
            routine_id: None,
            interface_name: None,
        }
    }

    #[test]
    fn terminal_completion_and_effective_loop_depth() {
        // a→b→c, c is terminal, each edge adds loop depth 1; initial depth 2.
        let p = map_policy(&[("a", "b"), ("b", "c")], &["c"], 1);
        let results = walk_evidence(
            "a",
            &p,
            WalkBounds::default(),
            WalkOpts {
                initial_loop_depth: 2,
                initial_steps: vec![],
            },
            no_uncertainties().view(),
            None,
        );
        let complete: Vec<&WalkResult> = results
            .iter()
            .filter(|r| r.stop == WalkStop::Complete)
            .collect();
        assert_eq!(complete.len(), 1);
        // 2 initial + 1 (a→b) + 1 (b→c) + 0 local = 4.
        assert_eq!(complete[0].effective_loop_depth, 4);
        // path = hop(b), hop(c), terminal(c) = 3 steps.
        assert_eq!(complete[0].path.len(), 3);
    }

    #[test]
    fn dead_end_when_no_edges_and_no_terminal() {
        let p = map_policy(&[], &[], 0);
        let results = walk_evidence(
            "a",
            &p,
            WalkBounds::default(),
            WalkOpts::default(),
            no_uncertainties().view(),
            None,
        );
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].stop, WalkStop::DeadEnd);
    }

    #[test]
    fn cycle_cut_breaks_the_loop() {
        // a→b→a (cycle), nothing terminal.
        let p = map_policy(&[("a", "b"), ("b", "a")], &[], 0);
        let results = walk_evidence(
            "a",
            &p,
            WalkBounds::default(),
            WalkOpts::default(),
            no_uncertainties().view(),
            None,
        );
        assert!(results.iter().any(|r| r.stop == WalkStop::CycleCut));
        // No result should revisit `a` (per-path cycle detection).
        assert!(results.iter().all(|r| r.stop != WalkStop::Complete));
    }

    #[test]
    fn depth_cut_at_max_depth() {
        // Long chain a→b→c→…; max_depth 2 means after routine_path len reaches 2,
        // the next expansion is cut.
        let p = map_policy(&[("a", "b"), ("b", "c"), ("c", "d")], &[], 0);
        let results = walk_evidence(
            "a",
            &p,
            WalkBounds {
                max_depth: 2,
                max_nodes: 500,
            },
            WalkOpts::default(),
            no_uncertainties().view(),
            None,
        );
        assert!(results.iter().any(|r| r.stop == WalkStop::DepthCut));
    }

    #[test]
    fn node_budget_cut_caps_total_visits() {
        // Wide fan-out so the node budget bites before depth/cycle.
        let p = map_policy(&[("a", "b"), ("a", "c"), ("a", "d"), ("a", "e")], &[], 0);
        let results = walk_evidence(
            "a",
            &p,
            WalkBounds {
                max_depth: 20,
                max_nodes: 2,
            },
            WalkOpts::default(),
            no_uncertainties().view(),
            None,
        );
        assert!(results.iter().any(|r| r.stop == WalkStop::NodeBudgetCut));
    }

    // -----------------------------------------------------------------------
    // Uncertainty accumulation oracles (FIX 1).
    // -----------------------------------------------------------------------

    /// (a) A downstream node's Complete result carries the UNION of upstream + its own.
    /// (b) Duplicates (same key) are deduplicated, keeping first.
    /// (c) The set is sorted by uncertainty_key.
    /// (d) Terminal AND a CycleCut result both carry the accumulated set.
    #[test]
    fn uncertainties_accumulated_on_complete_and_cut() {
        // Graph: a→b→c, c is terminal AND loops back to a (cycle on b→a branch too).
        // Node a has uncertainty ua1 (cs "a_cs1") and ua2 (cs "a_cs2").
        // Node b has ub1 (cs "b_cs1") and a duplicate of ua1 (same key → deduped, keep first).
        // Node c is the terminal.
        let p = map_policy(&[("a", "b"), ("b", "c"), ("b", "a")], &["c"], 0);

        let ua1 = make_uncertainty("unresolved-dispatch", Some("a_cs1"));
        let ua2 = make_uncertainty("unresolved-dispatch", Some("a_cs2"));
        let ub1 = make_uncertainty("dynamic-call", Some("b_cs1"));
        // duplicate of ua1 by key
        let ua1_dup = make_uncertainty("unresolved-dispatch", Some("a_cs1"));

        let mut ubn = OwnedUncertainties::default();
        ubn.insert("a", vec![ua1.clone(), ua2.clone()]);
        ubn.insert("b", vec![ub1.clone(), ua1_dup]);
        // node c contributes nothing

        let results = walk_evidence(
            "a",
            &p,
            WalkBounds::default(),
            WalkOpts::default(),
            ubn.view(),
            None,
        );

        // Find the Complete result (a→b→c).
        let complete: Vec<&WalkResult> = results
            .iter()
            .filter(|r| r.stop == WalkStop::Complete)
            .collect();
        assert_eq!(complete.len(), 1, "expected exactly one Complete branch");
        let c_ucs = &complete[0].uncertainties;

        // (a) Union: ua1, ua2, ub1 (ua1_dup is deduped).
        assert_eq!(
            c_ucs.len(),
            3,
            "union should have 3 entries (ua1, ua2, ub1)"
        );

        // (b) Duplicate ua1 must NOT appear twice — key "unresolved-dispatch|a_cs1" once.
        let ua1_key = crate::engine::l4::summary::uncertainty_key(&ua1);
        assert_eq!(
            c_ucs
                .iter()
                .filter(|u| crate::engine::l4::summary::uncertainty_key(u) == ua1_key)
                .count(),
            1
        );

        // (c) Sorted by key: "dynamic-call|b_cs1" < "unresolved-dispatch|a_cs1" < "unresolved-dispatch|a_cs2".
        let keys: Vec<String> = c_ucs
            .iter()
            .map(crate::engine::l4::summary::uncertainty_key)
            .collect();
        let mut sorted_keys = keys.clone();
        sorted_keys.sort();
        assert_eq!(keys, sorted_keys, "uncertainties must be sorted by key");

        // (d) The CycleCut result (b→a) also carries the accumulated set at b.
        let cycle_cuts: Vec<&WalkResult> = results
            .iter()
            .filter(|r| r.stop == WalkStop::CycleCut)
            .collect();
        assert!(
            !cycle_cuts.is_empty(),
            "expected at least one CycleCut result"
        );
        for cc in cycle_cuts {
            // At b, uncertainties = deduped(a's + b's) = ua1, ua2, ub1.
            assert_eq!(
                cc.uncertainties.len(),
                3,
                "CycleCut result must carry the accumulated uncertainty set"
            );
        }
    }

    /// DeadEnd result also carries accumulated uncertainties.
    #[test]
    fn uncertainties_on_dead_end() {
        let p = map_policy(&[], &[], 0);
        let u = make_uncertainty("unresolved-dispatch", Some("x_cs1"));
        let mut ubn = OwnedUncertainties::default();
        ubn.insert("a", vec![u.clone()]);

        let results = walk_evidence(
            "a",
            &p,
            WalkBounds::default(),
            WalkOpts::default(),
            ubn.view(),
            None,
        );
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].stop, WalkStop::DeadEnd);
        assert_eq!(results[0].uncertainties.len(), 1);
        assert_eq!(
            crate::engine::l4::summary::uncertainty_key(&results[0].uncertainties[0]),
            crate::engine::l4::summary::uncertainty_key(&u)
        );
    }
}

//! `d1_witness` — Task C3 of the d1 cohort redesign
//! (`.superpowers/sdd/task-c3-brief.md`,
//! `docs/superpowers/plans/2026-07-21-d1-cohort-redesign.md`), made O(M) by
//! Task C8 (`docs/superpowers/plans/2026-07-21-d1-cohort-redesign.md`'s perf
//! polish): ONE bounded representative witness per `(terminal, ContextKey)`,
//! replacing the per-`(loop, terminal)` FULL witness
//! [`d1_dataflow::build_transitive_witness`] built for every winning lane (the
//! ~28k-hop predecessor-chain walk that drove the Base App 8020 ~8h run before
//! Task C1's sink cutover — see that module's doc). Task C1's
//! [`crate::engine::l5::d1_cohort::TerminalSink`] already collapses 3.2M
//! `(loop, terminal)` aggregates down to ~34,861 `(terminal, ContextKey)`
//! cohorts; this module makes the ONE witness each cohort still owes CHEAP,
//! instead of building one PER WINNING LANE.
//!
//! ## Design (Task C8: O(M), no full-chain walk at all)
//!
//! The winner's SEED is read directly from [`BatchSolver::reach_origin`] /
//! [`BatchSolver::value_origin`] (Task C2) — an O(1) lookup, never a walk to
//! FIND it. `total_hops` is read directly from [`BatchSolver::reach_hops`] /
//! [`BatchSolver::value_hops`] — the authoritative first-arrival hop count, no
//! recompute. Task C3's original design still walked the FULL predecessor chain
//! (`collect_reach_chain_b`/`collect_value_chain_b`, bounded by the cohort
//! count — ~34,861 — rather than the 3.2M `(loop, terminal)` pairs the old
//! per-winner build walked, but still O(total_hops) PER COHORT) to materialize
//! a first-K/last-M slice. For the ~28k-hop chains real BC code produces, that
//! per-cohort walk was itself the dominant remaining cost. Task C8 drops it
//! entirely: [`representative_witness`] now walks ONLY the last `m_last` hops
//! BACKWARD from the terminal (`collect_reach_chain_b_bounded`/
//! `collect_value_chain_b_bounded`) — O(m_last), independent of chain depth —
//! and never materializes a first-K prefix at all (`first_steps` is always just
//! `[loop_step, call_step]`, read O(1) off the seed). `omitted_hops` is
//! `total_hops.saturating_sub(m_last)` — computed from the O(1) authoritative
//! hop count, not from anything the walk discovers. The full uncertainty-vector
//! union and the TRUE (unclamped) effective-depth recompute
//! [`d1_dataflow::build_transitive_witness`] builds are DROPPED — the cohort's
//! own `ContextKey` already carries the exact `depth_bucket`/`unc` (Task C1),
//! so this witness owes only a REPRESENTATIVE realizing path, not a second
//! derivation of those two fields.
#![allow(dead_code)]

use std::sync::Arc;

use crate::engine::l5::d1_dataflow::{
    BatchSolver, collect_reach_chain_b_bounded, collect_value_chain_b_bounded,
};
use crate::engine::l5::d1_graph::{D1Graph, D1Seed, NodeIx};
use crate::engine::l5::d1_reach::{call_step_ev, loop_step_ev};
use crate::engine::l5::detector_context::DetectorContext;
use crate::engine::l5::detectors::d1::{hop_step, terminal_step};
use crate::engine::l5::finding::EvidenceStep;
use crate::program::model::workspace::{ModelRecordOperation, ModelRoutine};

/// A bounded representative witness for one `(terminal, ContextKey)` cohort:
/// `[loop_step, call_step]` (ALWAYS exactly these two — Task C8 dropped the
/// first-K prefix), an `omitted_hops` count for the (possibly empty) unwalked
/// middle, up to `m_last` hop steps nearest the terminal, and the terminal step
/// itself.
///
/// `total_hops` is the AUTHORITATIVE first-arrival hop count
/// (`BatchSolver::reach_hops`/`value_hops`) — independent of how many hop
/// steps were actually materialized into `last_steps`.
///
/// Derives `Debug`/`Clone`/`PartialEq`/`Eq` (Task C4) so it can be embedded in
/// [`crate::engine::l5::finding::D1CohortContext`], which follows `finding.rs`'s
/// internal-type convention of deriving that same set.
///
/// ## Why the steps are `Arc<EvidenceStep>`
/// A witness is the largest thing a cohort retains, and cohorts repeat their
/// steps heavily: on Base App 8020 the 36,228 retained cohort witnesses hold
/// **172,915 steps over 40,325 distinct values** — a **4.29x** sharing factor.
/// Every cohort of one terminal repeats that terminal's step; every cohort
/// seeded from one loop repeats its loop and call steps; every cohort crossing
/// one graph edge repeats that hop step. As owned `Vec<EvidenceStep>` that was
/// 95.5 MiB in 1,097,185 allocations (18.3% of everything d1 retains). Sharing
/// one allocation per DISTINCT step — via [`StepInterner`], applied where the
/// findings take their copy — costs an 8-byte slot per step instead of 280 bytes
/// plus ~6 string allocations.
///
/// `PartialEq`/`Eq` on `Arc<T>` compare the POINTEES, so witness equality is
/// unchanged by the handles, and `Clone` became a refcount bump.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WitnessSummary {
    pub total_hops: u32,
    /// `[loop_step, call_step]` — ALWAYS exactly these two steps (Task C8: no
    /// hop steps live here any more, since materializing them would require
    /// walking forward from the seed, an O(total_hops) walk).
    pub first_steps: Vec<Arc<EvidenceStep>>,
    /// Hops skipped between the call step and `last_steps`' window.
    /// `total_hops.saturating_sub(m_last)` — `0` when the whole chain fits
    /// within `last_steps` (`total_hops <= m_last`, the shallow case).
    pub omitted_hops: u32,
    /// Up to `m_last` hop steps immediately preceding the terminal, in
    /// seed->terminal order. Holds the WHOLE chain in the shallow case
    /// (`total_hops <= m_last`).
    pub last_steps: Vec<Arc<EvidenceStep>>,
    pub terminal_step: Arc<EvidenceStep>,
}

/// Hash-cons for [`EvidenceStep`]: hand back ONE `Arc` per distinct step value.
///
/// Scoped to a single d1 assembly pass and dropped with it, so the only thing it
/// leaves behind is the sharing itself. Applied at the point the findings take
/// their copy of a cohort's witness (`assemble_cohort_findings`), which is where
/// the retention actually happens — the sink's own copies are transient and are
/// freed with the run.
///
/// Correctness rests on `EvidenceStep`'s derived `Eq`/`Hash` agreeing, which they
/// do by construction (both derived over the same fields). Two steps that intern
/// to the same `Arc` were byte-equal, so nothing downstream can distinguish them:
/// `PartialEq` on `Arc` compares pointees and every reader derefs.
#[derive(Default)]
pub(crate) struct StepInterner {
    seen: std::collections::HashSet<Arc<EvidenceStep>>,
}

impl StepInterner {
    /// The shared handle for `s` — an existing one when an equal step has been
    /// seen, otherwise a fresh allocation this interner now owns a reference to.
    pub(crate) fn intern(&mut self, s: &EvidenceStep) -> Arc<EvidenceStep> {
        if let Some(existing) = self.seen.get(s) {
            return Arc::clone(existing);
        }
        let fresh: Arc<EvidenceStep> = Arc::new(s.clone());
        self.seen.insert(Arc::clone(&fresh));
        fresh
    }

    /// `w` with every step replaced by its shared handle. Value-identical to `w`
    /// (`WitnessSummary`'s `PartialEq` compares pointees), representation-shared.
    pub(crate) fn intern_witness(&mut self, w: &WitnessSummary) -> WitnessSummary {
        WitnessSummary {
            total_hops: w.total_hops,
            first_steps: w.first_steps.iter().map(|s| self.intern(s)).collect(),
            omitted_hops: w.omitted_hops,
            last_steps: w.last_steps.iter().map(|s| self.intern(s)).collect(),
            terminal_step: self.intern(&w.terminal_step),
        }
    }

    /// The number of DISTINCT steps interned so far (the census's
    /// "distinct whole steps"). Test/diagnostic only.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.seen.len()
    }
}

/// Build the representative witness for a DIRECT in-loop db op (old branch (a)):
/// `[loop_step]` + the terminal `op_step`, zero hops. This is the cohort-report
/// analogue of the old direct witness `vec![loop_step, op_step]`
/// (`d1_dataflow::emit_lane_aggregates`'s `BestSource::Direct` arm) — flattening
/// [`WitnessSummary`] (`first_steps ++ last_steps ++ terminal_step`) reproduces
/// that exact two-step path.
pub(crate) fn direct_witness(
    routine: &ModelRoutine,
    loop_info: &crate::program::body::features::PLoop,
    op: &ModelRecordOperation,
    ctx: &DetectorContext,
) -> WitnessSummary {
    let loop_step = loop_step_ev(routine, loop_info);
    let op_step = terminal_step(
        &ctx.routine_by_id,
        &ctx.table_by_id,
        routine.id.as_str(),
        Some(op.id.as_str()),
    );
    WitnessSummary {
        total_hops: 0,
        first_steps: vec![Arc::new(loop_step)],
        omitted_hops: 0,
        last_steps: vec![],
        terminal_step: Arc::new(op_step),
    }
}

/// Flatten a [`WitnessSummary`] into a single `Vec<EvidenceStep>` —
/// `first_steps ++ last_steps ++ [terminal_step]`, dropping the (possibly empty)
/// omitted middle. For a witness that fit within `m_last` hops (`omitted_hops
/// == 0`, the shallow case — every fixture path, and any DO path ≤ `m_last`
/// hops) this reproduces the OLD full `evidence_path` BYTE-FOR-BYTE; for a
/// deeper path it is the bounded representative (`[loop_step, call_step]` +
/// suffix + terminal), the approved compression.
/// Returns OWNED steps (the shared handles are materialised out) because every
/// consumer wants a plain path it can read or project; the result is a
/// short-lived derivation, not something the run retains.
pub(crate) fn flatten_witness(w: &WitnessSummary) -> Vec<EvidenceStep> {
    let mut out = Vec::with_capacity(w.first_steps.len() + w.last_steps.len() + 1);
    out.extend(w.first_steps.iter().map(|s| (**s).clone()));
    out.extend(w.last_steps.iter().map(|s| (**s).clone()));
    out.push((*w.terminal_step).clone());
    out
}

/// Render one graph hop `(from_node, edge_k)` into its [`EvidenceStep`] — the
/// same per-hop [`hop_step`] call `d1_dataflow::build_transitive_witness` makes.
fn render_hop(
    graph: &D1Graph,
    ctx: &DetectorContext,
    from_node: NodeIx,
    edge_k: usize,
) -> EvidenceStep {
    let edge = &graph.edges[from_node as usize][edge_k];
    let from_id = graph.node_ids[from_node as usize];
    let to_id = graph.node_ids[edge.to as usize];
    hop_step(
        &ctx.routine_by_id,
        from_id,
        to_id,
        edge.kind,
        edge.callsite_id,
    )
}

/// Build ONE bounded representative witness for a winning (lane, fact) pair —
/// O(m_last), Task C8: no walk over the full predecessor chain, ever.
///
/// `fact_ix`/`is_value_fact` select the winning reach or value fact (indexed
/// into `BatchSolver::reach_facts`/`value_facts`, the same way
/// `d1_dataflow::BestSource::Reach`/`Value` do); `terminal_node`/
/// `terminal_owner`/`terminal_op` are the terminal's own identity (the caller
/// already has these when it has a fact to build a witness for — mirrors
/// `build_transitive_witness`'s parameters).
///
/// Algorithm: the seed is read via [`BatchSolver::reach_origin`]/
/// [`BatchSolver::value_origin`] (Task C2) — O(1), NOT a walk to find it.
/// `total_hops` is read via [`BatchSolver::reach_hops`]/[`BatchSolver::value_hops`]
/// — O(1), the authoritative hop count. The hop STEPS are collected by a
/// BACKWARD walk from the terminal, capped at `m_last` hops
/// (`collect_reach_chain_b_bounded`/`collect_value_chain_b_bounded`) — O(m_last)
/// regardless of chain depth: a deep chain (`total_hops > m_last`) never sees
/// its middle or its seed-adjacent hops at all. `first_steps` is always exactly
/// `[loop_step, call_step]` (no hop steps — materializing a seed-adjacent
/// prefix would require walking FORWARD from the seed, which is exactly the
/// O(total_hops) cost this rewrite removes). `omitted_hops =
/// total_hops.saturating_sub(m_last)`, computed from the O(1) `total_hops`, not
/// from anything the bounded walk discovers. When the chain is short
/// (`total_hops <= m_last`) the bounded walk reaches the seed and `last_steps`
/// ends up holding the WHOLE chain, `omitted_hops == 0` — the walk cost is
/// still bounded by `m_last`, so this shallow case is cheap too.
/// `debug_assert`s cross-check the walk's own seed (when it reaches one)
/// against the O(1) origin lookup, and that a capped walk collects EXACTLY
/// `m_last` hops — the invariants a subtly wrong hop-vs-seed check ordering
/// could otherwise violate right at the `total_hops == m_last` boundary.
#[allow(clippy::too_many_arguments)]
pub(crate) fn representative_witness<'a>(
    solver: &BatchSolver,
    graph: &D1Graph<'a>,
    ctx: &DetectorContext,
    seeds: &[D1Seed<'a>],
    lane: usize,
    fact_ix: usize,
    is_value_fact: bool,
    terminal_node: NodeIx,
    terminal_owner: &'a ModelRoutine,
    terminal_op: &'a ModelRecordOperation,
    m_last: usize,
) -> WitnessSummary {
    // The seed: O(1) via Task C2's origin array — never a chain walk to find it.
    let origin_seed_index = (if is_value_fact {
        solver.value_origin[fact_ix][lane]
    } else {
        solver.reach_origin[fact_ix][lane]
    }) as usize;
    debug_assert_ne!(
        origin_seed_index,
        u32::MAX as usize,
        "a present (fact, lane) always carries a resolved origin seed"
    );

    // total_hops: authoritative, straight off the fixpoint's own first-arrival
    // record — never recomputed.
    let total_hops = if is_value_fact {
        solver.value_hops[fact_ix][lane]
    } else {
        solver.reach_hops[fact_ix][lane]
    };

    // Bounded BACKWARD walk from the terminal: at most `m_last` hops, O(m_last)
    // regardless of chain depth. `walked_seed_index` is `Some` only when the
    // walk reached the seed (chain length <= m_last); it is never used to
    // discover the chain length — `total_hops` (above) is authoritative.
    let (mut hops_terminal_to_seed, walked_seed_index) = if is_value_fact {
        collect_value_chain_b_bounded(
            &solver.value_pred,
            &solver.reach_pred,
            lane,
            fact_ix,
            m_last,
        )
    } else {
        collect_reach_chain_b_bounded(&solver.reach_pred, lane, fact_ix, m_last)
    };
    if let Some(ws) = walked_seed_index {
        debug_assert_eq!(
            ws, origin_seed_index,
            "origin_seed (Task C2) must equal the chain-walk seed when the bounded \
             walk reaches it — the correctness invariant this witness's O(1) seed \
             lookup relies on"
        );
        debug_assert_eq!(
            hops_terminal_to_seed.len() as u32,
            total_hops,
            "a bounded walk that reached the seed must have collected exactly \
             total_hops hops (the chain's true length)"
        );
    } else {
        debug_assert_eq!(
            hops_terminal_to_seed.len(),
            m_last,
            "a capped walk (seed not reached within m_last hops) always collects \
             exactly m_last hops"
        );
        debug_assert!(
            total_hops as usize > m_last,
            "a capped walk (seed not reached within m_last hops) implies \
             total_hops > m_last"
        );
    }
    hops_terminal_to_seed.reverse(); // now seed -> terminal order, for display

    let seed = &seeds[origin_seed_index];

    // Path validity: the chain's terminal-most hop must land on terminal_node
    // (a zero-hop witness's seed entry must BE the terminal node instead).
    match hops_terminal_to_seed.last() {
        Some(&(from_node, edge_k)) => {
            debug_assert_eq!(
                graph.edges[from_node as usize][edge_k].to, terminal_node,
                "the witness's terminal-most hop must land on the terminal node"
            );
        }
        None => {
            debug_assert_eq!(
                seed.entry, terminal_node,
                "a zero-hop witness's seed entry must BE the terminal node"
            );
        }
    }

    let omitted_hops = total_hops.saturating_sub(m_last as u32);

    // ALWAYS exactly [loop_step, call_step] — Task C8 dropped the first-K
    // prefix, which required a forward-from-seed walk to materialize.
    let first_steps: Vec<Arc<EvidenceStep>> = vec![
        Arc::new(loop_step_ev(seed.loop_routine, seed.loop_info)),
        Arc::new(call_step_ev(seed, graph, ctx)),
    ];

    let last_steps: Vec<Arc<EvidenceStep>> = hops_terminal_to_seed
        .iter()
        .map(|&(from_node, edge_k)| Arc::new(render_hop(graph, ctx, from_node, edge_k)))
        .collect();

    let terminal_ev = terminal_step(
        &ctx.routine_by_id,
        &ctx.table_by_id,
        terminal_owner.id.as_str(),
        Some(terminal_op.id.as_str()),
    );

    WitnessSummary {
        total_hops,
        first_steps,
        omitted_hops,
        last_steps,
        terminal_step: Arc::new(terminal_ev),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::engine::l4::combined_graph::CombinedEdge;
    use crate::engine::l5::closed_world_temp::ClosedWorldTempParams;
    use crate::engine::l5::d1_dataflow::{GroupSpec, condense, run_batch_fixpoint_for_test};
    use crate::engine::l5::d1_graph::build_d1_graph;
    use crate::engine::l5::d1_liveness::compute_liveness;
    use crate::engine::l5::full_summary::FullRoutineSummary;
    use crate::engine::l5::test_support::{
        arg_binding, call_site, coverage, edge_kind, fact, loop_def, minimal_ctx, record_op,
        routine, summary, ts_known, ts_pd,
    };
    use crate::program::model::workspace::{ModelEntities, ModelRoutine};

    type Fixture = (
        Vec<ModelRoutine>,
        HashMap<String, Vec<CombinedEdge>>,
        HashMap<String, FullRoutineSummary>,
    );

    fn db_summary(id: &str, table: &str) -> FullRoutineSummary {
        summary(
            id,
            vec![fact("read", "table", Some(table))],
            vec![],
            Some(coverage("complete")),
        )
    }

    /// A throwaway `ModelEntities` wrapping a CLONE of `routines` — `build_d1_graph`
    /// needs an owned `&ModelEntities` sharing `ctx`'s own lifetime; mirrors
    /// `d1_dataflow::tests::ws`.
    fn ws(routines: &[ModelRoutine]) -> ModelEntities {
        ModelEntities {
            objects: vec![],
            tables: vec![],
            routines: routines.to_vec(),
        }
    }

    /// A chain of `depth` intermediate routines between the seed entry `A` and
    /// the terminal `T`: `R --(loop, seed)--> A -> C1 -> C2 -> ... -> C{depth}
    /// -> T`. Every hop is a plain direct call (no argument bindings), so `T`'s
    /// op (no `temp_state`, i.e. non-PD/constant) is read via a REACH fact —
    /// `depth + 1` hops from `A` to `T`.
    fn deep_reach_chain_fixture(depth: usize) -> Fixture {
        let mut r = routine("R", "procedure");
        r.loops = vec![loop_def("R/loop0")];
        r.call_sites = vec![call_site("R/cs0", "A", vec!["R/loop0".to_string()])];

        let mut names: Vec<String> = vec!["A".to_string()];
        for i in 1..=depth {
            names.push(format!("C{i}"));
        }
        names.push("T".to_string());

        let mut routines = vec![r];
        let mut graph_edges: HashMap<String, Vec<CombinedEdge>> = HashMap::new();
        graph_edges.insert(
            "R".to_string(),
            vec![edge_kind("R", "A", "R/cs0", "direct")],
        );
        let mut summaries: HashMap<String, FullRoutineSummary> = HashMap::new();

        for i in 0..names.len() {
            let name = &names[i];
            summaries.insert(name.clone(), db_summary(name, &format!("t/{name}")));
            if i + 1 < names.len() {
                let next = &names[i + 1];
                let cs_id = format!("{name}/cs0");
                let mut rt = routine(name, "procedure");
                rt.call_sites = vec![call_site(&cs_id, next, vec![])];
                routines.push(rt);
                graph_edges.insert(name.clone(), vec![edge_kind(name, next, &cs_id, "direct")]);
            } else {
                // T: the terminal, a plain Modify with no temp_state (reads REACH).
                let mut t = routine(name, "procedure");
                t.record_operations = vec![record_op(
                    &format!("{name}/op0"),
                    "Modify",
                    "Rec",
                    Some(&format!("t/{name}")),
                    vec![],
                    false,
                )];
                routines.push(t);
            }
        }
        (routines, graph_edges, summaries)
    }

    /// `R --(loop, seed)--> A -> B -> C -> D -> H` (a VALUE-fact chain): `B`'s
    /// call to `C` binds `C`'s param 0 to a KNOWN literal (`ts_known`, a Const
    /// transfer — this is where the value chain switches onto the REACH chain,
    /// `ValuePredB::HopFromReach`, since a Const value has no caller-value
    /// parent); `C`'s and `D`'s calls forward their own param 0 (`ts_pd(0)`, a
    /// Copy transfer) onward, and `H` reads `ts_pd(0)` directly on its own op.
    /// 4 hops (A->B->C->D->H) from the seed entry `A` to the terminal `H`, with
    /// the HopFromReach transition mid-chain (at B's call to C).
    fn deep_value_chain_fixture() -> Fixture {
        let mut r = routine("R", "procedure");
        r.loops = vec![loop_def("R/loop0")];
        r.call_sites = vec![call_site("R/cs0", "A", vec!["R/loop0".to_string()])];

        let mut a = routine("A", "procedure");
        a.call_sites = vec![call_site("A/csB", "B", vec![])];

        let mut b = routine("B", "procedure");
        let mut b_cs = call_site("B/csC", "C", vec![]);
        b_cs.argument_bindings = vec![arg_binding(0, Some(ts_known(true)))];
        b.call_sites = vec![b_cs];

        let mut c = routine("C", "procedure");
        let mut c_cs = call_site("C/csD", "D", vec![]);
        c_cs.argument_bindings = vec![arg_binding(0, Some(ts_pd(0)))];
        c.call_sites = vec![c_cs];

        let mut d = routine("D", "procedure");
        let mut d_cs = call_site("D/csH", "H", vec![]);
        d_cs.argument_bindings = vec![arg_binding(0, Some(ts_pd(0)))];
        d.call_sites = vec![d_cs];

        let mut h = routine("H", "procedure");
        let mut op0 = record_op("H/op0", "Modify", "Rec", Some("t/H"), vec![], false);
        op0.temp_state = Some(ts_pd(0));
        h.record_operations = vec![op0];

        let routines = vec![r, a, b, c, d, h];

        let mut graph_edges: HashMap<String, Vec<CombinedEdge>> = HashMap::new();
        graph_edges.insert(
            "R".to_string(),
            vec![edge_kind("R", "A", "R/cs0", "direct")],
        );
        graph_edges.insert(
            "A".to_string(),
            vec![edge_kind("A", "B", "A/csB", "direct")],
        );
        graph_edges.insert(
            "B".to_string(),
            vec![edge_kind("B", "C", "B/csC", "direct")],
        );
        graph_edges.insert(
            "C".to_string(),
            vec![edge_kind("C", "D", "C/csD", "direct")],
        );
        graph_edges.insert(
            "D".to_string(),
            vec![edge_kind("D", "H", "D/csH", "direct")],
        );
        let summaries: HashMap<String, FullRoutineSummary> = ["A", "B", "C", "D", "H"]
            .iter()
            .map(|id| (id.to_string(), db_summary(id, &format!("t/{id}"))))
            .collect();
        (routines, graph_edges, summaries)
    }

    /// A real graph edge `from --from.callsite--> to_routine_id` exists —
    /// mirrors `d1_dataflow::tests::assert_witness_valid`'s contiguity check.
    fn assert_edge(ctx: &DetectorContext, from: &EvidenceStep, to_routine_id: &str) {
        assert!(
            from.callsite_id.is_some(),
            "an intermediate/call step must carry a callsite"
        );
        assert!(from.loop_id.is_none(), "non-loop intermediate step");
        let cs = from.callsite_id.as_deref();
        let is_real_edge = ctx
            .graph
            .edges_by_from
            .get(&from.routine_id)
            .is_some_and(|edges| {
                edges
                    .iter()
                    .any(|e| e.callsite_id.as_deref() == cs && e.to == to_routine_id)
            });
        assert!(
            is_real_edge,
            "step {} --{:?}--> {} must be a real graph edge (contiguity)",
            from.routine_id, cs, to_routine_id
        );
    }

    /// A `WitnessSummary` is a valid realizing path PREFIX (`first_steps`
    /// contiguous, `last_steps` contiguous where present, no gap-crossing
    /// contiguity claimed across an `omitted_hops` middle): first step is the
    /// loop step, second is the call step, the terminal step matches, and
    /// every consecutive pair within each contiguous run crosses a real edge.
    fn assert_witness_valid(
        w: &WitnessSummary,
        ctx: &DetectorContext,
        loop_routine_id: &str,
        loop_id: &str,
        terminal_owner_id: &str,
        terminal_op_id: &str,
    ) {
        assert!(
            w.first_steps.len() >= 2,
            "first_steps always carries [loop_step, call_step]"
        );
        assert_eq!(
            w.first_steps[0].routine_id, loop_routine_id,
            "first step must be in the loop routine"
        );
        assert_eq!(
            w.first_steps[0].loop_id.as_deref(),
            Some(loop_id),
            "first step must be the loop step"
        );
        assert!(
            w.first_steps[1].loop_id.is_none(),
            "second step (call step) is not a loop step"
        );
        assert!(
            w.first_steps[1].callsite_id.is_some(),
            "the call step carries a callsite"
        );

        assert_eq!(
            w.terminal_step.routine_id, terminal_owner_id,
            "terminal step must be owned by the terminal routine"
        );
        assert_eq!(
            w.terminal_step.operation_id.as_deref(),
            Some(terminal_op_id),
            "terminal step must be the terminal op"
        );
        assert!(
            w.terminal_step.callsite_id.is_none(),
            "the terminal step carries no callsite"
        );

        for pair in w.first_steps[1..].windows(2) {
            assert_edge(ctx, &pair[0], &pair[1].routine_id);
        }
        if w.last_steps.is_empty() {
            assert_edge(
                ctx,
                w.first_steps.last().unwrap(),
                &w.terminal_step.routine_id,
            );
        } else {
            for pair in w.last_steps.windows(2) {
                assert_edge(ctx, &pair[0], &pair[1].routine_id);
            }
            assert_edge(
                ctx,
                w.last_steps.last().unwrap(),
                &w.terminal_step.routine_id,
            );
        }
    }

    /// `StepInterner`'s own contract, independent of d1: equal steps collapse to
    /// ONE allocation, unequal steps do not, and `intern_witness` preserves the
    /// witness VALUE while sharing its steps. The d1-level use is pinned by
    /// `d1::assembly_tests::cohort_witness_steps_are_shared`.
    #[test]
    fn step_interner_shares_equal_steps_only() {
        fn s(routine: &str, note: &str) -> EvidenceStep {
            EvidenceStep {
                routine_id: routine.to_string(),
                operation_id: None,
                callsite_id: None,
                loop_id: None,
                source_anchor: crate::engine::l5::finding::SourceAnchor {
                    source_unit_id: "ws:t.al".to_string(),
                    start_line: 1,
                    start_column: 0,
                    end_line: 1,
                    end_column: 9,
                    enclosing_routine_id: routine.to_string(),
                    syntax_kind: "call".to_string(),
                    normalized_text_hash: None,
                    leading_context_hash: None,
                    trailing_context_hash: None,
                },
                note: note.to_string(),
            }
        }

        let mut i = StepInterner::default();
        let a = i.intern(&s("R", "calls A"));
        let b = i.intern(&s("R", "calls A"));
        let c = i.intern(&s("R", "calls B"));
        assert!(Arc::ptr_eq(&a, &b), "equal steps share one allocation");
        assert!(!Arc::ptr_eq(&a, &c), "unequal steps must NOT be collapsed");
        assert_eq!(&*c.note, "calls B", "…and keep their own value");
        assert_eq!(i.len(), 2, "two distinct steps interned, not three");

        // A witness interns to an EQUAL witness whose steps are the shared ones.
        let w = WitnessSummary {
            total_hops: 1,
            first_steps: vec![Arc::new(s("R", "calls A")), Arc::new(s("R", "calls B"))],
            omitted_hops: 0,
            last_steps: vec![],
            terminal_step: Arc::new(s("T", "Modify")),
        };
        let shared = i.intern_witness(&w);
        assert_eq!(shared, w, "interning is value-preserving");
        assert!(Arc::ptr_eq(&shared.first_steps[0], &a));
        assert!(Arc::ptr_eq(&shared.first_steps[1], &c));
        assert_eq!(i.len(), 3, "only the terminal step was new");
    }

    // === Deep fixture: hops > M — last-M + omitted, first_steps is just the
    // 2-element prefix (Task C8: no first-K any more) ========================
    // (Setup is inlined per test, not factored into a shared helper: `graph`/
    // `seeds` borrow BOTH `ctx` and `workspace`, so a helper returning them
    // would have to return a self-referential struct — mirrors
    // `d1_dataflow::tests::origin_propagation_matches_chain_walk`'s own inline
    // setup for the same reason.)
    #[test]
    fn deep_reach_witness_last_m_omitted() {
        let (routines, graph_edges, summaries) = deep_reach_chain_fixture(9);
        let ctx = minimal_ctx(&routines, graph_edges, summaries);
        let workspace = ws(&routines);
        let mut memo = HashMap::new();
        let (graph, seeds) = build_d1_graph(&ctx, &workspace, &mut memo);
        assert_eq!(seeds.len(), 1, "fixture must have exactly one seed");
        let cw = ClosedWorldTempParams::new();
        let liveness = compute_liveness(&graph, &ctx, &cw);
        let scc = condense(&graph);
        let group = GroupSpec {
            loop_routine: seeds[0].loop_routine,
            loop_id: seeds[0].loop_id,
            loop_info: seeds[0].loop_info,
            seed_indices: vec![0],
            direct_indices: Vec::new(),
        };
        let solver = run_batch_fixpoint_for_test(
            &graph,
            &liveness,
            &scc,
            &seeds,
            &ctx,
            &cw,
            std::slice::from_ref(&group),
        );

        let t_node = graph.node_ix["T"];
        let reach_facts = &solver.reach_at[t_node as usize];
        assert_eq!(reach_facts.len(), 1, "one reach fact at T");
        let fact_ix = reach_facts[0];

        let t_routine = ctx.routine_by_id["T"];
        let t_op = &t_routine.record_operations[0];

        const M: usize = 2;
        let w = representative_witness(
            &solver, &graph, &ctx, &seeds, 0, fact_ix, false, t_node, t_routine, t_op, M,
        );

        assert_eq!(w.total_hops, 10, "A -> C1..C9 -> T is 10 hops");
        assert_eq!(
            w.first_steps.len(),
            2,
            "first_steps is always just [loop_step, call_step] (Task C8)"
        );
        assert_eq!(w.last_steps.len(), M);
        assert_eq!(w.last_steps[0].routine_id, "C8");
        assert_eq!(w.last_steps[1].routine_id, "C9");
        assert_eq!(w.omitted_hops, w.total_hops - M as u32);
        assert_eq!(w.omitted_hops, 8);

        assert_witness_valid(&w, &ctx, "R", "R/loop0", "T", "T/op0");
    }

    // === Shallow fixture: hops < M — whole chain in last_steps, omitted = 0 =
    #[test]
    fn shallow_reach_witness_whole_chain_no_omission() {
        let (routines, graph_edges, summaries) = deep_reach_chain_fixture(1);
        let ctx = minimal_ctx(&routines, graph_edges, summaries);
        let workspace = ws(&routines);
        let mut memo = HashMap::new();
        let (graph, seeds) = build_d1_graph(&ctx, &workspace, &mut memo);
        assert_eq!(seeds.len(), 1, "fixture must have exactly one seed");
        let cw = ClosedWorldTempParams::new();
        let liveness = compute_liveness(&graph, &ctx, &cw);
        let scc = condense(&graph);
        let group = GroupSpec {
            loop_routine: seeds[0].loop_routine,
            loop_id: seeds[0].loop_id,
            loop_info: seeds[0].loop_info,
            seed_indices: vec![0],
            direct_indices: Vec::new(),
        };
        let solver = run_batch_fixpoint_for_test(
            &graph,
            &liveness,
            &scc,
            &seeds,
            &ctx,
            &cw,
            std::slice::from_ref(&group),
        );

        let t_node = graph.node_ix["T"];
        let reach_facts = &solver.reach_at[t_node as usize];
        assert_eq!(reach_facts.len(), 1, "one reach fact at T");
        let fact_ix = reach_facts[0];

        let t_routine = ctx.routine_by_id["T"];
        let t_op = &t_routine.record_operations[0];

        const M: usize = 5;
        let w = representative_witness(
            &solver, &graph, &ctx, &seeds, 0, fact_ix, false, t_node, t_routine, t_op, M,
        );

        assert_eq!(w.total_hops, 2, "A -> C1 -> T is 2 hops");
        assert_eq!(
            w.first_steps.len(),
            2,
            "first_steps is always just [loop_step, call_step] (Task C8)"
        );
        // total_hops (2) < m_last (5): the whole chain fits in last_steps.
        assert_eq!(w.last_steps.len(), 2);
        assert_eq!(w.last_steps[0].routine_id, "A");
        assert_eq!(w.last_steps[1].routine_id, "C1");
        assert_eq!(w.omitted_hops, 0);

        assert_witness_valid(&w, &ctx, "R", "R/loop0", "T", "T/op0");
    }

    // === Boundary fixture: hops == M exactly — the bounded walk must still
    // reach the seed (not off-by-one into a spurious `omitted_hops`) =========
    #[test]
    fn reach_witness_total_hops_equals_m_last_no_omission() {
        let (routines, graph_edges, summaries) = deep_reach_chain_fixture(2);
        let ctx = minimal_ctx(&routines, graph_edges, summaries);
        let workspace = ws(&routines);
        let mut memo = HashMap::new();
        let (graph, seeds) = build_d1_graph(&ctx, &workspace, &mut memo);
        assert_eq!(seeds.len(), 1, "fixture must have exactly one seed");
        let cw = ClosedWorldTempParams::new();
        let liveness = compute_liveness(&graph, &ctx, &cw);
        let scc = condense(&graph);
        let group = GroupSpec {
            loop_routine: seeds[0].loop_routine,
            loop_id: seeds[0].loop_id,
            loop_info: seeds[0].loop_info,
            seed_indices: vec![0],
            direct_indices: Vec::new(),
        };
        let solver = run_batch_fixpoint_for_test(
            &graph,
            &liveness,
            &scc,
            &seeds,
            &ctx,
            &cw,
            std::slice::from_ref(&group),
        );

        let t_node = graph.node_ix["T"];
        let reach_facts = &solver.reach_at[t_node as usize];
        assert_eq!(reach_facts.len(), 1, "one reach fact at T");
        let fact_ix = reach_facts[0];

        let t_routine = ctx.routine_by_id["T"];
        let t_op = &t_routine.record_operations[0];

        const M: usize = 3;
        let w = representative_witness(
            &solver, &graph, &ctx, &seeds, 0, fact_ix, false, t_node, t_routine, t_op, M,
        );

        assert_eq!(w.total_hops, 3, "A -> C1 -> C2 -> T is 3 hops");
        // total_hops == m_last EXACTLY: the bounded walk must reach the seed
        // right at the boundary, not report a spurious capped/omitted result.
        assert_eq!(w.last_steps.len(), 3);
        assert_eq!(w.last_steps[0].routine_id, "A");
        assert_eq!(w.last_steps[1].routine_id, "C1");
        assert_eq!(w.last_steps[2].routine_id, "C2");
        assert_eq!(w.omitted_hops, 0);

        assert_witness_valid(&w, &ctx, "R", "R/loop0", "T", "T/op0");
    }

    // === Value-fact terminal via a HopFromReach transition, straddling the
    // last-M window (the bounded walk must cross value->reach mid-window) ====
    #[test]
    fn deep_value_witness_crosses_hop_from_reach() {
        let (routines, graph_edges, summaries) = deep_value_chain_fixture();
        let ctx = minimal_ctx(&routines, graph_edges, summaries);
        let workspace = ws(&routines);
        let mut memo = HashMap::new();
        let (graph, seeds) = build_d1_graph(&ctx, &workspace, &mut memo);
        assert_eq!(seeds.len(), 1, "fixture must have exactly one seed");
        let cw = ClosedWorldTempParams::new();
        let liveness = compute_liveness(&graph, &ctx, &cw);
        let scc = condense(&graph);
        let group = GroupSpec {
            loop_routine: seeds[0].loop_routine,
            loop_id: seeds[0].loop_id,
            loop_info: seeds[0].loop_info,
            seed_indices: vec![0],
            direct_indices: Vec::new(),
        };
        let solver = run_batch_fixpoint_for_test(
            &graph,
            &liveness,
            &scc,
            &seeds,
            &ctx,
            &cw,
            std::slice::from_ref(&group),
        );

        let h_node = graph.node_ix["H"];
        let value_facts = &solver.value_at[h_node as usize];
        assert_eq!(value_facts.len(), 1, "one value fact at H (slot 0)");
        let fact_ix = value_facts[0];

        let h_routine = ctx.routine_by_id["H"];
        let h_op = &h_routine.record_operations[0];

        // M=3 puts the HopFromReach-crossing edge (B->C, hop index 1 from the
        // seed) INSIDE the last-3-nearest-terminal window: last_steps must hold
        // B->C, C->D, D->H, and the bounded walk must cross from the value chain
        // onto the reach chain mid-window to collect it.
        const M: usize = 3;
        let w = representative_witness(
            &solver, &graph, &ctx, &seeds, 0, fact_ix, true, h_node, h_routine, h_op, M,
        );

        assert_eq!(w.total_hops, 4, "A -> B -> C -> D -> H is 4 hops");
        assert_eq!(
            w.first_steps.len(),
            2,
            "first_steps is always just [loop_step, call_step] (Task C8)"
        );
        assert_eq!(w.last_steps.len(), M);
        assert_eq!(
            w.last_steps[0].routine_id, "B",
            "the HopFromReach-crossing edge (B->C) lands inside the last-M window"
        );
        assert_eq!(w.last_steps[1].routine_id, "C");
        assert_eq!(w.last_steps[2].routine_id, "D");
        assert_eq!(w.omitted_hops, 1, "the A->B hop is the omitted middle");

        assert_witness_valid(&w, &ctx, "R", "R/loop0", "H", "H/op0");
    }
}

//! `d1_dataflow` — Task D2 of the d1 dataflow-solver redesign
//! (`.superpowers/sdd/task-d2-brief.md`,
//! `docs/superpowers/plans/2026-07-20-d1-dataflow-solver.md`): the single-group
//! FACT solver (`solve_group`), the correctness spine of the rewrite. It
//! reproduces `d1_reach::process_group`'s six load-bearing components —
//! coverage, `reachable_verdicts`, severity, verdict, `depth_bucket`, and
//! `unc` — per (loop, terminal-op), while replacing the joint-`TempVec`
//! product-state BFS with independent per-parameter facts.
//!
//! **`solve_group`/`solve_batch` (and their private helpers below) are
//! `#[cfg(test)]`-only** — Task D3 widened the single-group fact model to the
//! batched, SCC-shared [`BatchSolver`] (`score_batch_to_sink`, wired into
//! production via `d1_reach::search_loops_cohorts`), which supersedes both.
//! `process_group`/`solve_group`/`solve_batch` are retained solely as the
//! differential ORACLE chain the `tests` module below checks
//! [`BatchSolver`]/`score_batch_to_sink` against on the six load-bearing
//! components; component 7 (the witness) is allowed to differ.
//!
//! ## The fact model (single group = a 1-bit "mask"; D3 widens to u64/64 lanes)
//!
//! Because d1's temp propagation is UNARY (D1's `d1_liveness` proves it: every
//! callee param is a function of ≤1 caller param, every terminal reads ≤1
//! param), the joint `TempVec` (3^params multiplicity) is unnecessary. We track
//! two independent fact families over the filtered [`D1Graph`]:
//!
//! ```text
//! reach[node][depth 0..2][unc 0..1]                            present/absent
//! value[node][live-param-slot][Temp|Physical|Unknown][d][unc]  present/absent
//! ```
//!
//! `reach` is the transitive closure of (depth, unc) states — the SAME node set
//! and (depth, unc) reachability `process_group`'s label set realizes (its label
//! dedup is FINER — on the full `TempVec` — but the (node, depth, unc) *marginal*
//! is identical, and a terminal reads only ONE param, so the value-fact marginal
//! at the read slot equals `process_group`'s projected label set). `value`
//! carries, per live param, the resolved temp class along some realizing path.
//!
//! ## Propagation (level-synchronous → first arrival = shortest = min hops)
//!
//! Seed each seed-entry: `reach[entry][min(2, seed_depth)][entry_unc]` and, for
//! each live entry param, ONE value fact from
//! `cross_hop(root_state(loop_routine), seed callsite, …)` (identical to the
//! `process_group` seed label's own `TempVec`, projected to the live params). A
//! FIFO delta worklist then propagates to fixpoint — facts only ever GAIN
//! presence, so cycles terminate. Edge `n→m`: `d2 = min(2, d + loop_depth)`,
//! `u2 = u || node_unc[m]`. `reach` propagates directly; a callee live param's
//! value applies D1's compiled [`ParamTransfer`] — `Const(pt)` sets that class
//! (driven by the caller's REACH fact, since a constant ignores caller values),
//! `Copy{slot}` forwards the caller's value fact at that slot. FIFO order makes a
//! fact's first arrival its minimum hop count.
//!
//! ## Terminal scoring + winner selection (mirrors `process_group` exactly)
//!
//! A constant terminal (op `temp_state` non-PD) and a PROVEN PD terminal read
//! REACH (the class is caller-independent — `resolve_terminal` returns `Temp`
//! for a proven PD *without* reading any value; D1's `terminal_reads` over-reports
//! proven params, so the proven re-check here mirrors `resolve_terminal` EXACTLY);
//! a non-proven PD terminal reads the VALUE fact at its `terminal_reads` slot. Each
//! present fact yields one candidate — verdict via [`flowfield_verdict`], scoring
//! depth `min(2, fact.depth + local_depth)`, severity via `severity_for`, `hops`
//! = the fact's first-arrival hops. Direct ops enter the accumulator FIRST (as
//! today). The winner is the max under `d1_reach::selection_rank` (the identical
//! rule); its witness is materialized from the fact's first-arrival predecessor
//! chain (prepend `[loop_step, call_step]`, hop steps, terminal step; sum the
//! TRUE unclamped depth; union node uncertainties along the ONE witness).
//!
//! Only the witness / entry-callsite / reported true-depth / uncertainty vector /
//! first-discovery tie (component 7) MAY differ from `process_group` — a change
//! in components 1-6 is a BUG (proven absent by the `tests` differential below).

#[cfg(test)]
use std::collections::BTreeMap;
use std::collections::{HashMap, HashSet, VecDeque};

use crate::program::body::features::PLoop;
use crate::program::model::workspace::{ModelRecordOperation, ModelRoutine};
// Owned-value uncertainties survive only on the `#[cfg(test)]` oracle path
// (`build_transitive_witness`); the production cohort path interns them into the
// sink's `UncertaintyTable` and carries `UncertaintyId`s.
#[cfg(test)]
use crate::engine::l4::summary::{Uncertainty, dedupe_uncertainties};
use crate::engine::l5::closed_world_temp::ClosedWorldTempParams;
use crate::engine::l5::d1_cohort::{
    CohortRep, ContextKey, GroupIx, PathUncertaintyCache, TerminalSink, UncertaintyId,
};
#[cfg(test)]
use crate::engine::l5::d1_graph::D1Terminal;
use crate::engine::l5::d1_graph::{D1Graph, D1Seed, NodeIx, edge_kind_binding_ok};
use crate::engine::l5::d1_liveness::{Liveness, ParamTransfer};
use crate::engine::l5::d1_reach::{
    DirectOp, flowfield_verdict, node_has_uncertainty, selection_rank,
};
#[cfg(test)]
use crate::engine::l5::d1_reach::{LoopTerminalAgg, call_step_ev, loop_step_ev};
use crate::engine::l5::d1_temp::{
    ParamTemp, TempVec, cross_hop, lookup, resolve_terminal, root_state,
};
use crate::engine::l5::d1_witness::{direct_witness, representative_witness};
use crate::engine::l5::detector_context::{DetectorContext, UncertaintySetId};
use crate::engine::l5::detectors::d1::{TempVerdict, is_setup_singleton_get, severity_for};
#[cfg(test)]
use crate::engine::l5::detectors::d1::{hop_step, terminal_step};
#[cfg(test)]
use crate::engine::l5::finding::EvidenceStep;
use crate::engine::l5::path_merge::sev_rank;
use std::sync::Arc;

/// A reach fact's first-arrival predecessor (the seed it descends from, or the
/// parent reach fact + the crossed edge). `solve_group`-only (oracle) — the
/// batch solver's analogue is [`ReachPredB`].
#[cfg(test)]
#[derive(Clone, Copy)]
enum ReachPred {
    Seed {
        seed_index: usize,
    },
    Hop {
        pred: usize,
        from_node: NodeIx,
        edge_k: usize,
    },
}

/// One `reach[node][depth][unc]` presence fact + its min-hop provenance.
/// `solve_group`-only (oracle).
#[cfg(test)]
struct ReachFact {
    node: NodeIx,
    depth: i64,
    unc: bool,
    hops: u32,
    pred: ReachPred,
}

/// A value fact's first-arrival predecessor. A `Const` transfer is born at a hop
/// from the caller's REACH fact (the class has no caller-value parent);
/// `Copy`/seed chain through value facts. `solve_group`-only (oracle) — the
/// batch solver's analogue is [`ValuePredB`].
#[cfg(test)]
#[derive(Clone, Copy)]
enum ValuePred {
    Seed {
        seed_index: usize,
    },
    HopFromValue {
        pred: usize,
        from_node: NodeIx,
        edge_k: usize,
    },
    HopFromReach {
        pred: usize,
        from_node: NodeIx,
        edge_k: usize,
    },
}

/// One `value[node][slot][class][depth][unc]` presence fact + provenance.
/// `solve_group`-only (oracle).
#[cfg(test)]
struct ValueFact {
    node: NodeIx,
    slot: u16,
    class: ParamTemp,
    depth: i64,
    unc: bool,
    hops: u32,
    pred: ValuePred,
}

/// A worklist item — a newly-created fact whose outgoing propagation is pending.
/// `solve_group`-only (oracle).
#[cfg(test)]
enum WorkItem {
    Reach(usize),
    Value(usize),
}

/// A candidate's provenance — the witness source. A direct op carries its own
/// step inputs; a transitive candidate points back into a fact arena.
/// `solve_group`-only (oracle).
#[cfg(test)]
enum CandSource<'a> {
    Direct {
        routine: &'a ModelRoutine,
        loop_info: &'a PLoop,
        op: &'a ModelRecordOperation,
    },
    TransReach {
        reach_fact: usize,
    },
    TransValue {
        value_fact: usize,
    },
}

/// One scored candidate for a (loop, terminal-op) bucket. `solve_group`-only
/// (oracle) — the batch solver's analogue is [`BestRef`]/[`DirectCand`].
#[cfg(test)]
struct Candidate<'a> {
    verdict: TempVerdict,
    severity: &'static str,
    unc: bool,
    hops: u32,
    depth_bucket: i64,
    discovery: usize,
    source: CandSource<'a>,
    terminal_op: &'a ModelRecordOperation,
    terminal_owner: &'a ModelRoutine,
    terminal_local_depth: i64,
}

/// The per-group fact solver's mutable state — the fact arenas, their dedup
/// indices, per-node fact lists (for scoring), and the delta worklist.
/// `solve_group`-only (oracle) — the batch solver's analogue is
/// [`BatchSolver`].
#[cfg(test)]
struct Solver {
    reach_facts: Vec<ReachFact>,
    value_facts: Vec<ValueFact>,
    reach_index: HashMap<(NodeIx, i64, bool), usize>,
    value_index: HashMap<(NodeIx, u16, ParamTemp, i64, bool), usize>,
    reach_at: Vec<Vec<usize>>,
    value_at: Vec<Vec<usize>>,
    queue: VecDeque<WorkItem>,
}

#[cfg(test)]
impl Solver {
    fn new(n_nodes: usize) -> Self {
        Solver {
            reach_facts: Vec::new(),
            value_facts: Vec::new(),
            reach_index: HashMap::new(),
            value_index: HashMap::new(),
            reach_at: vec![Vec::new(); n_nodes],
            value_at: vec![Vec::new(); n_nodes],
            queue: VecDeque::new(),
        }
    }

    /// Create `reach[node][depth][unc]` iff absent (first arrival wins the
    /// predecessor + min-hops), enqueueing it for propagation.
    fn ensure_reach(&mut self, node: NodeIx, depth: i64, unc: bool, hops: u32, pred: ReachPred) {
        let key = (node, depth, unc);
        if self.reach_index.contains_key(&key) {
            return;
        }
        let idx = self.reach_facts.len();
        self.reach_facts.push(ReachFact {
            node,
            depth,
            unc,
            hops,
            pred,
        });
        self.reach_index.insert(key, idx);
        self.reach_at[node as usize].push(idx);
        self.queue.push_back(WorkItem::Reach(idx));
    }

    /// Create `value[node][slot][class][depth][unc]` iff absent.
    #[allow(clippy::too_many_arguments)]
    fn ensure_value(
        &mut self,
        node: NodeIx,
        slot: u16,
        class: ParamTemp,
        depth: i64,
        unc: bool,
        hops: u32,
        pred: ValuePred,
    ) {
        let key = (node, slot, class, depth, unc);
        if self.value_index.contains_key(&key) {
            return;
        }
        let idx = self.value_facts.len();
        self.value_facts.push(ValueFact {
            node,
            slot,
            class,
            depth,
            unc,
            hops,
            pred,
        });
        self.value_index.insert(key, idx);
        self.value_at[node as usize].push(idx);
        self.queue.push_back(WorkItem::Value(idx));
    }
}

/// Walk a reach fact's first-arrival predecessor chain to its seed, collecting
/// the `(from_node, edge_k)` hops in TERMINAL->SEED order. `solve_group`-only
/// (oracle) — the batch solver's analogue is [`collect_reach_chain_b`].
#[cfg(test)]
fn collect_reach_chain(reach_facts: &[ReachFact], start: usize) -> (Vec<(NodeIx, usize)>, usize) {
    let mut hops: Vec<(NodeIx, usize)> = Vec::new();
    let mut cur = start;
    loop {
        match reach_facts[cur].pred {
            ReachPred::Seed { seed_index } => return (hops, seed_index),
            ReachPred::Hop {
                pred,
                from_node,
                edge_k,
            } => {
                hops.push((from_node, edge_k));
                cur = pred;
            }
        }
    }
}

/// Walk a value fact's first-arrival predecessor chain. A `HopFromReach` (the
/// hop that BORN this class via a `Const` transfer) switches the walk onto the
/// caller's reach chain for the remainder. `solve_group`-only (oracle) — the
/// batch solver's analogue is [`collect_value_chain_b`].
#[cfg(test)]
fn collect_value_chain(
    value_facts: &[ValueFact],
    reach_facts: &[ReachFact],
    start: usize,
) -> (Vec<(NodeIx, usize)>, usize) {
    let mut hops: Vec<(NodeIx, usize)> = Vec::new();
    let mut cur = start;
    loop {
        match value_facts[cur].pred {
            ValuePred::Seed { seed_index } => return (hops, seed_index),
            ValuePred::HopFromValue {
                pred,
                from_node,
                edge_k,
            } => {
                hops.push((from_node, edge_k));
                cur = pred;
            }
            ValuePred::HopFromReach {
                pred,
                from_node,
                edge_k,
            } => {
                hops.push((from_node, edge_k));
                let (mut rest, seed_index) = collect_reach_chain(reach_facts, pred);
                hops.append(&mut rest);
                return (hops, seed_index);
            }
        }
    }
}

/// Materialize a transitive winner's witness from its first-arrival hop chain
/// (`hops` in terminal->seed order): `[loop_step, call_step]` + one `hop_step`
/// per edge (seed->terminal order) + the terminal step; the union of node
/// uncertainties along THIS path; the entry callsite; and the TRUE (unclamped)
/// effective depth `seed_depth + Σ edge.loop_depth + local_depth`.
/// `solve_group`/`emit_lane_aggregates`-only (oracle) — the cohort path's
/// analogue is `representative_witness` (`d1_witness.rs`).
#[cfg(test)]
#[allow(clippy::too_many_arguments)]
fn build_transitive_witness<'a>(
    hops: &[(NodeIx, usize)],
    seed_index: usize,
    terminal_node: NodeIx,
    terminal_owner: &'a ModelRoutine,
    terminal_op: &'a ModelRecordOperation,
    terminal_local_depth: i64,
    graph: &D1Graph<'a>,
    ctx: &DetectorContext,
    seeds: &[D1Seed<'a>],
) -> (Vec<EvidenceStep>, Vec<Uncertainty>, Option<&'a str>, i64) {
    let seed = &seeds[seed_index];
    let mut witness: Vec<EvidenceStep> = Vec::with_capacity(hops.len() + 3);
    witness.push(loop_step_ev(seed.loop_routine, seed.loop_info));
    witness.push(call_step_ev(seed, graph, ctx));

    // hops in seed->terminal order (reverse of the terminal->seed walk).
    let mut sum_edges = 0i64;
    let mut path_nodes: Vec<NodeIx> = Vec::with_capacity(hops.len() + 1);
    for (from_node, edge_k) in hops.iter().rev() {
        let edge = &graph.edges[*from_node as usize][*edge_k];
        let from_id = graph.node_ids[*from_node as usize];
        let to_id = graph.node_ids[edge.to as usize];
        witness.push(hop_step(
            &ctx.routine_by_id,
            from_id,
            to_id,
            edge.kind,
            edge.callsite_id,
        ));
        sum_edges += edge.loop_depth;
        path_nodes.push(*from_node);
    }
    path_nodes.push(terminal_node);
    witness.push(terminal_step(
        &ctx.routine_by_id,
        &ctx.table_by_id,
        terminal_owner.id.as_str(),
        Some(terminal_op.id.as_str()),
    ));

    // Uncertainty union along the witness (seed -> terminal node order).
    let mut concat: Vec<Uncertainty> = Vec::new();
    let view = ctx.uncertainty_view();
    for &n in &path_nodes {
        let nid = graph.node_ids[n as usize];
        concat.extend(view.values_of(nid).cloned());
    }
    let uncertainties = dedupe_uncertainties(concat);
    let effective = seed.seed_depth + sum_edges + terminal_local_depth;
    (
        witness,
        uncertainties,
        Some(seed.callsite.id.as_str()),
        effective,
    )
}

/// Solve ONE loop group with the fact model; returns the SAME `Vec<LoopTerminalAgg>`
/// `process_group` returns (components 1-6 identical; witness may differ).
/// `#[cfg(test)]`-only: superseded in production by the batched [`BatchSolver`]
/// (`score_batch_to_sink`); retained as a differential oracle (see the module doc).
#[cfg(test)]
#[allow(clippy::too_many_arguments)]
pub(crate) fn solve_group<'a>(
    graph: &D1Graph<'a>,
    liveness: &Liveness,
    seeds: &[D1Seed<'a>],
    direct_ops: &[DirectOp<'a>],
    ctx: &'a DetectorContext,
    cw: &ClosedWorldTempParams,
    loop_routine: &'a ModelRoutine,
    loop_id: &'a str,
    loop_info: &'a PLoop,
    seed_indices: &[usize],
    direct_indices: &[usize],
) -> Vec<LoopTerminalAgg<'a>> {
    let n_nodes = graph.node_ids.len();
    let mut solver = Solver::new(n_nodes);

    // Per-node scalars used repeatedly during propagation.
    let unc_by_node: Vec<bool> = graph
        .node_ids
        .iter()
        .map(|id| node_has_uncertainty(ctx, id))
        .collect();
    // A node with no `routine_by_id` entry carries no composable bindings, so it
    // is a dead end — never expanded (mirrors `process_group`'s defensive skip).
    let expandable: Vec<bool> = graph
        .node_ids
        .iter()
        .map(|id| ctx.routine_by_id.contains_key(id))
        .collect();

    // Rule 1: seed the frontier (in seed order). The seed's entry `TempVec` is
    // computed EXACTLY as `process_group`'s seed label — `cross_hop` of the loop
    // routine's root state across the seed callsite — then projected to the
    // entry's live params (D1's `Need[entry]`).
    let root = root_state(loop_routine.id.as_str(), cw);
    for &si in seed_indices {
        let seed = &seeds[si];
        let entry = seed.entry;
        let entry_id = graph.node_ids[entry as usize];
        let binding_ok = edge_kind_binding_ok(seed.entry_edge_kind);
        let entry_temp = cross_hop(
            &root,
            seed.loop_routine,
            seed.callsite.id.as_str(),
            entry_id,
            binding_ok,
            cw,
        );
        let depth = seed.seed_depth.min(2);
        let unc = unc_by_node[entry as usize];
        solver.ensure_reach(entry, depth, unc, 0, ReachPred::Seed { seed_index: si });
        for (slot, &p) in liveness.need[entry as usize].iter().enumerate() {
            let class = lookup(&entry_temp, p);
            solver.ensure_value(
                entry,
                slot as u16,
                class,
                depth,
                unc,
                0,
                ValuePred::Seed { seed_index: si },
            );
        }
    }

    // Rules 2-3: FIFO delta propagation to fixpoint. Reach facts drive reach +
    // `Const` value propagation; value facts drive `Copy` value propagation.
    while let Some(item) = solver.queue.pop_front() {
        match item {
            WorkItem::Reach(ri) => {
                let (node, depth, unc, hops) = {
                    let f = &solver.reach_facts[ri];
                    (f.node, f.depth, f.unc, f.hops)
                };
                if !expandable[node as usize] {
                    continue;
                }
                for (k, edge) in graph.edges[node as usize].iter().enumerate() {
                    let m = edge.to;
                    let d2 = (depth + edge.loop_depth).min(2);
                    let u2 = unc || unc_by_node[m as usize];
                    solver.ensure_reach(
                        m,
                        d2,
                        u2,
                        hops + 1,
                        ReachPred::Hop {
                            pred: ri,
                            from_node: node,
                            edge_k: k,
                        },
                    );
                    for (callee_slot, transfer) in
                        liveness.edge_transfers[node as usize][k].iter().enumerate()
                    {
                        if let ParamTransfer::Const(pt) = transfer {
                            solver.ensure_value(
                                m,
                                callee_slot as u16,
                                *pt,
                                d2,
                                u2,
                                hops + 1,
                                ValuePred::HopFromReach {
                                    pred: ri,
                                    from_node: node,
                                    edge_k: k,
                                },
                            );
                        }
                    }
                }
            }
            WorkItem::Value(vi) => {
                let (node, slot, class, depth, unc, hops) = {
                    let f = &solver.value_facts[vi];
                    (f.node, f.slot, f.class, f.depth, f.unc, f.hops)
                };
                if !expandable[node as usize] {
                    continue;
                }
                for (k, edge) in graph.edges[node as usize].iter().enumerate() {
                    let m = edge.to;
                    let d2 = (depth + edge.loop_depth).min(2);
                    let u2 = unc || unc_by_node[m as usize];
                    for (callee_slot, transfer) in
                        liveness.edge_transfers[node as usize][k].iter().enumerate()
                    {
                        if let ParamTransfer::Copy { caller_slot } = transfer
                            && *caller_slot == slot
                        {
                            solver.ensure_value(
                                m,
                                callee_slot as u16,
                                class,
                                d2,
                                u2,
                                hops + 1,
                                ValuePred::HopFromValue {
                                    pred: vi,
                                    from_node: node,
                                    edge_k: k,
                                },
                            );
                        }
                    }
                }
            }
        }
    }

    // Rules 4/6: generate candidates. Direct ops first (branch (a) precedence),
    // then transitive candidates. `discovery` is a per-group monotonic counter.
    let mut buckets: BTreeMap<(&'a str, &'a str), Vec<Candidate<'a>>> = BTreeMap::new();
    let mut discovery = 0usize;

    for &di in direct_indices {
        let d = &direct_ops[di];
        let op = d.op;
        let owner = d.routine;
        let base_pt = resolve_terminal(op, &root, owner.id.as_str(), cw);
        let verdict = flowfield_verdict(base_pt, op, &ctx.table_by_id);
        let local_depth = op.loop_stack.len() as i64;
        let depth_bucket = local_depth.min(2);
        let is_singleton = is_setup_singleton_get(op, Some(owner), &ctx.table_by_id);
        let severity = severity_for(op, verdict, depth_bucket, is_singleton);
        buckets
            .entry((owner.id.as_str(), op.id.as_str()))
            .or_default()
            .push(Candidate {
                verdict,
                severity,
                unc: false,
                hops: 0,
                depth_bucket,
                discovery,
                source: CandSource::Direct {
                    routine: owner,
                    loop_info: d.loop_info,
                    op,
                },
                terminal_op: op,
                terminal_owner: owner,
                terminal_local_depth: local_depth,
            });
        discovery += 1;
    }

    // Transitive candidates: one per present fact reaching each terminal. A
    // constant terminal (non-PD op) and a PROVEN PD terminal read REACH facts
    // (caller-independent verdict); a non-proven PD terminal reads the VALUE
    // fact at its `terminal_reads` slot. The proven re-check mirrors
    // `resolve_terminal` EXACTLY (D1's `terminal_reads` over-reports proven).
    for node in 0..n_nodes as NodeIx {
        let terminals = &graph.terminals[node as usize];
        for (ti, t) in terminals.iter().enumerate() {
            let op = t.op;
            let owner = t.owner;
            let local_depth = t.local_depth;
            let is_singleton = is_setup_singleton_get(op, Some(owner), &ctx.table_by_id);

            // Resolve read mode: `Some(slot)` = value-based; `None` = reach-based.
            let value_slot: Option<u16> = match liveness.terminal_reads[node as usize][ti] {
                None => None,
                Some(slot) => {
                    let i = liveness.need[node as usize][slot as usize];
                    if cw.contains(&(owner.id.to_string(), i)) {
                        None // proven -> constant Temp verdict, read reach
                    } else {
                        Some(slot)
                    }
                }
            };

            match value_slot {
                None => {
                    // Reach-based: verdict is caller-independent (constant op, or
                    // a proven PD -> `resolve_terminal` returns Temp without a read).
                    let verdict = flowfield_verdict(
                        resolve_terminal(op, &TempVec::new(), owner.id.as_str(), cw),
                        op,
                        &ctx.table_by_id,
                    );
                    for &ri in &solver.reach_at[node as usize] {
                        let f = &solver.reach_facts[ri];
                        let depth_bucket = (f.depth + local_depth).min(2);
                        let severity = severity_for(op, verdict, depth_bucket, is_singleton);
                        buckets
                            .entry((owner.id.as_str(), op.id.as_str()))
                            .or_default()
                            .push(Candidate {
                                verdict,
                                severity,
                                unc: f.unc,
                                hops: f.hops,
                                depth_bucket,
                                discovery,
                                source: CandSource::TransReach { reach_fact: ri },
                                terminal_op: op,
                                terminal_owner: owner,
                                terminal_local_depth: local_depth,
                            });
                        discovery += 1;
                    }
                }
                Some(slot) => {
                    let i = liveness.need[node as usize][slot as usize];
                    for &vi in &solver.value_at[node as usize] {
                        let f = &solver.value_facts[vi];
                        if f.slot != slot {
                            continue;
                        }
                        let frame: TempVec = std::iter::once((i, f.class)).collect();
                        let verdict = flowfield_verdict(
                            resolve_terminal(op, &frame, owner.id.as_str(), cw),
                            op,
                            &ctx.table_by_id,
                        );
                        let depth_bucket = (f.depth + local_depth).min(2);
                        let severity = severity_for(op, verdict, depth_bucket, is_singleton);
                        buckets
                            .entry((owner.id.as_str(), op.id.as_str()))
                            .or_default()
                            .push(Candidate {
                                verdict,
                                severity,
                                unc: f.unc,
                                hops: f.hops,
                                depth_bucket,
                                discovery,
                                source: CandSource::TransValue { value_fact: vi },
                                terminal_op: op,
                                terminal_owner: owner,
                                terminal_local_depth: local_depth,
                            });
                        discovery += 1;
                    }
                }
            }
        }
    }

    // Rules 5/7: per bucket (already (owner id, op id)-sorted by the BTreeMap),
    // select the winner and materialize its witness.
    let mut out: Vec<LoopTerminalAgg<'a>> = Vec::new();
    for (_key, cands) in buckets {
        let mut reachable_verdicts: Vec<TempVerdict> = cands.iter().map(|c| c.verdict).collect();
        reachable_verdicts.sort();
        reachable_verdicts.dedup();

        let winner = cands
            .iter()
            .max_by_key(|c| {
                selection_rank(
                    c.severity,
                    c.verdict,
                    c.unc,
                    c.hops,
                    c.depth_bucket,
                    c.discovery,
                )
            })
            .expect("a bucket is never empty");

        let (witness, uncertainties, entry_callsite_id, effective_loop_depth) = match &winner.source
        {
            CandSource::Direct {
                routine,
                loop_info,
                op,
            } => {
                let loop_step = loop_step_ev(routine, loop_info);
                let op_step = terminal_step(
                    &ctx.routine_by_id,
                    &ctx.table_by_id,
                    routine.id.as_str(),
                    Some(op.id.as_str()),
                );
                (
                    vec![loop_step, op_step],
                    Vec::new(),
                    None,
                    winner.terminal_local_depth,
                )
            }
            CandSource::TransReach { reach_fact } => {
                let terminal_node = solver.reach_facts[*reach_fact].node;
                let (hops, seed_index) = collect_reach_chain(&solver.reach_facts, *reach_fact);
                build_transitive_witness(
                    &hops,
                    seed_index,
                    terminal_node,
                    winner.terminal_owner,
                    winner.terminal_op,
                    winner.terminal_local_depth,
                    graph,
                    ctx,
                    seeds,
                )
            }
            CandSource::TransValue { value_fact } => {
                let terminal_node = solver.value_facts[*value_fact].node;
                let (hops, seed_index) =
                    collect_value_chain(&solver.value_facts, &solver.reach_facts, *value_fact);
                build_transitive_witness(
                    &hops,
                    seed_index,
                    terminal_node,
                    winner.terminal_owner,
                    winner.terminal_op,
                    winner.terminal_local_depth,
                    graph,
                    ctx,
                    seeds,
                )
            }
        };

        out.push(LoopTerminalAgg {
            loop_routine,
            loop_id,
            loop_info,
            terminal: D1Terminal {
                op: winner.terminal_op,
                owner: winner.terminal_owner,
                local_depth: winner.terminal_local_depth,
            },
            entry_callsite_id,
            severity: winner.severity,
            verdict: winner.verdict,
            reachable_verdicts,
            depth_bucket: winner.depth_bucket,
            effective_loop_depth,
            witness,
            uncertainties,
        });
    }
    out
}

// ===========================================================================
// Task D3 — the BATCH driver: 64-lane group bitsets + call-SCC condensation
// scheduler. `solve_batch` solves up to 64 loop groups sharing ONE traversal of
// the (dense 797-member) call SCC: the D2 single-group fact model widened from a
// 1-bit "mask" to a `u64` group-mask (group i in the batch owns bit i). This is
// where the dataflow win materializes — the SCC is threaded once per BATCH, not
// once per group.
//
// ## Correctness (why the shared traversal is exact)
//
// Per gpt's design (memory note `d1-output-bound-falsified` §2026-07-20): d1's
// temp propagation is UNARY, so the GROUP-SETS realizing each per-parameter fact
// are union-only-monotone (bits are only ever OR-ed in, never cleared) even
// though temp VALUES are not. A standard least-fixpoint over the ORIGINAL call
// SCC therefore reproduces, PER LANE, the identical (node, depth, unc, slot,
// class) fact set — hence identical coverage / reachable_verdicts / severity /
// verdict / depth_bucket / unc — that `solve_group` (and thus `process_group`)
// produces for that group. Only the WITNESS / entry-callsite / equal-ranked tie
// (component 7) may differ, exactly as at D2.
//
// ## Scheduling + provenance
//
// Facts propagate FORWARD (caller -> callee). We process the call-graph SCC
// condensation in TOPOLOGICAL order (callers before callees); cross-SCC
// arrivals are buffered per downstream SCC and, within each SCC, drained by a
// min-hops-first (level-synchronous) delta worklist so a lane's FIRST arrival at
// a fact is its minimum hop count (= shortest witness). Provenance is per
// (fact, lane): the first-arrival predecessor (the seed it descends from, or the
// parent fact + crossed edge), stored in per-lane arrays and walked at scoring
// time to materialize that lane's winning witness. The whole batch arena is
// dropped after emitting its aggregates (the memory bound — serial batches, no
// concurrent arenas).

/// Batch width — how many loop groups share one condensation pass. Group `i` in
/// the batch owns bit `i` of the `u64` lane masks. (D6 may lower this — 32/16 —
/// if the dense-SCC batch arena exceeds the RSS headroom.)
pub(crate) const BATCH_WIDTH: usize = 64;

/// One loop group in a batch: the (loop routine, loop id) identity plus the seed
/// and direct-op indices (into the shared `seeds`/`direct_ops` slices) that
/// belong to it. `search_loops` assigns groups to lanes in the existing sorted
/// `(loop_routine_id, loop_id)` order.
pub(crate) struct GroupSpec<'a> {
    pub loop_routine: &'a ModelRoutine,
    pub loop_id: &'a str,
    pub loop_info: &'a PLoop,
    pub seed_indices: Vec<usize>,
    pub direct_indices: Vec<usize>,
}

/// The call-graph SCC condensation over the filtered [`D1Graph`]. Deterministic:
/// node iteration (0..n) and per-node edge order are both fixed, so Tarjan's
/// emission order — and hence every field here — is a pure function of the graph.
pub(crate) struct CallScc {
    /// `NodeIx` -> its SCC id (an index into `members`/`topo_order`, in
    /// topological numbering: a caller SCC has a lower id than its callees').
    pub scc_of: Vec<u32>,
    /// SCC id -> its member nodes, sorted ascending by `NodeIx`.
    pub members: Vec<Vec<NodeIx>>,
    /// SCC ids in TOPOLOGICAL order (callers before callees). Since ids are
    /// assigned in topological order, this is simply `0..members.len()`, but it
    /// is materialized so a caller iterates schedule order without assuming the
    /// numbering convention.
    pub topo_order: Vec<u32>,
}

/// One explicit Tarjan work-stack frame (node + next-child cursor) — no
/// recursion (AL call graphs can be deep). Mirrors `l4::scc::Frame`.
struct SccFrame {
    node: NodeIx,
    child: usize,
}

/// Tarjan's SCC over the filtered [`D1Graph`], iterative + deterministic.
///
/// This is a DENSE (`NodeIx`-indexed) port of the engine's existing iterative
/// Tarjan, `crate::engine::l4::scc::tarjan_scc` — same explicit-work-stack
/// shape, same reverse-topological emission — but that one operates on
/// `String`-keyed `SccInputGraph`s (it runs over al-sem's combined graph), which
/// would force a full String-adjacency rebuild + a re-map back to `NodeIx` on
/// every call; the `D1Graph` is already interned to dense `NodeIx`, so a direct
/// port keeps the index alignment the solver relies on. Tarjan emits SCCs in
/// REVERSE-topological order (callees before callers); we reverse that to number
/// SCCs (and `topo_order`) callers-first — the order forward fact propagation
/// wants.
pub(crate) fn condense(graph: &D1Graph) -> CallScc {
    let n = graph.node_ids.len();
    const UNVISITED: u32 = u32::MAX;
    let mut index = vec![UNVISITED; n];
    let mut lowlink = vec![0u32; n];
    let mut on_stack = vec![false; n];
    let mut tarjan_stack: Vec<NodeIx> = Vec::new();
    let mut next_index = 0u32;
    // Raw SCCs in Tarjan's natural REVERSE-topological emission order.
    let mut raw_sccs: Vec<Vec<NodeIx>> = Vec::new();

    for start in 0..n as NodeIx {
        if index[start as usize] != UNVISITED {
            continue;
        }
        let mut work: Vec<SccFrame> = vec![SccFrame {
            node: start,
            child: 0,
        }];
        while !work.is_empty() {
            let top = work.len() - 1;
            let node = work[top].node;
            let child = work[top].child;

            if child == 0 {
                index[node as usize] = next_index;
                lowlink[node as usize] = next_index;
                next_index += 1;
                tarjan_stack.push(node);
                on_stack[node as usize] = true;
            }

            let edges = &graph.edges[node as usize];
            if child < edges.len() {
                work[top].child += 1;
                let to = edges[child].to;
                if index[to as usize] == UNVISITED {
                    work.push(SccFrame { node: to, child: 0 });
                } else if on_stack[to as usize] {
                    let cur = lowlink[node as usize];
                    lowlink[node as usize] = cur.min(index[to as usize]);
                }
                continue;
            }

            // All children explored — settle this node.
            if lowlink[node as usize] == index[node as usize] {
                let mut members: Vec<NodeIx> = Vec::new();
                loop {
                    let w = tarjan_stack.pop().expect("Tarjan stack underflow");
                    on_stack[w as usize] = false;
                    members.push(w);
                    if w == node {
                        break;
                    }
                }
                raw_sccs.push(members);
            }
            work.pop();
            if let Some(parent) = work.last() {
                let pn = parent.node;
                let cur = lowlink[pn as usize];
                lowlink[pn as usize] = cur.min(lowlink[node as usize]);
            }
        }
    }

    // raw_sccs is in reverse-topological order (callees before callers); reverse
    // it so SCC id / topo_order run callers-first (forward propagation order).
    let mut scc_of = vec![u32::MAX; n];
    let mut members: Vec<Vec<NodeIx>> = Vec::with_capacity(raw_sccs.len());
    for raw in raw_sccs.iter().rev() {
        let scc_id = members.len() as u32;
        let mut sorted = raw.clone();
        sorted.sort_unstable();
        for &m in &sorted {
            scc_of[m as usize] = scc_id;
        }
        members.push(sorted);
    }
    let topo_order: Vec<u32> = (0..members.len() as u32).collect();
    CallScc {
        scc_of,
        members,
        topo_order,
    }
}

/// A reach fact's per-lane first-arrival predecessor (the seed it descends from,
/// or the parent reach fact + the crossed edge). `None` = the lane is not
/// present at this fact.
///
/// `pub(crate)`: Task C3's `d1_witness` (a sibling module) walks
/// `BatchSolver::reach_pred` directly to materialize a bounded witness's hop
/// steps.
#[derive(Clone, Copy)]
pub(crate) enum ReachPredB {
    None,
    Seed {
        seed_index: u32,
    },
    Hop {
        pred: u32,
        from_node: NodeIx,
        edge_k: u32,
    },
}

/// A value fact's per-lane first-arrival predecessor. `HopFromReach` is the hop
/// that BORN this class via a `Const` transfer (its parent is a reach fact);
/// `HopFromValue` / `Seed` chain through value facts (mirrors D2's `ValuePred`).
///
/// `pub(crate)`: see [`ReachPredB`]'s doc — `d1_witness` walks
/// `BatchSolver::value_pred` directly.
#[derive(Clone, Copy)]
pub(crate) enum ValuePredB {
    None,
    Seed {
        seed_index: u32,
    },
    HopFromValue {
        pred: u32,
        from_node: NodeIx,
        edge_k: u32,
    },
    HopFromReach {
        pred: u32,
        from_node: NodeIx,
        edge_k: u32,
    },
}

/// One `reach[node][depth][unc]` fact shared across the batch's ≤64 lanes: the
/// key + the lane presence `mask`. Per-lane provenance (hops + predecessor)
/// lives in the parallel `reach_hops`/`reach_pred` arrays (indexed by fact id).
struct ReachFactB {
    // Read back only by the `#[cfg(test)]` oracle (`solve_group`'s
    // `CandSource::TransReach` arm / `emit_lane_aggregates`) — the live cohort
    // path tracks the current node through its own SCC-traversal loop variable
    // instead of round-tripping it through the fact.
    #[cfg_attr(not(test), allow(dead_code))]
    node: NodeIx,
    depth: i64,
    unc: bool,
    mask: u64,
}

/// One `value[node][slot][class][depth][unc]` fact shared across the lanes.
struct ValueFactB {
    // Read back only by the `#[cfg(test)]` oracle — see `ReachFactB::node`'s doc.
    #[cfg_attr(not(test), allow(dead_code))]
    node: NodeIx,
    slot: u16,
    class: ParamTemp,
    depth: i64,
    unc: bool,
    mask: u64,
}

/// A pending arrival to a fact key: the newly-arriving lane `mask` at `hops`,
/// plus the predecessor to record for those lanes. Routed to the target SCC's
/// worklist (same SCC) or its pending buffer (a downstream SCC).
enum Proposal {
    Reach {
        node: NodeIx,
        depth: i64,
        unc: bool,
        mask: u64,
        hops: u32,
        pred: ReachPredB,
    },
    Value {
        node: NodeIx,
        slot: u16,
        class: ParamTemp,
        depth: i64,
        unc: bool,
        mask: u64,
        hops: u32,
        pred: ValuePredB,
    },
}

impl Proposal {
    fn node(&self) -> NodeIx {
        match self {
            Proposal::Reach { node, .. } | Proposal::Value { node, .. } => *node,
        }
    }
    fn hops(&self) -> u32 {
        match self {
            Proposal::Reach { hops, .. } | Proposal::Value { hops, .. } => *hops,
        }
    }
}

/// A hop-indexed FIFO bucket queue draining ONE SCC's proposals in
/// level-synchronous (min-hops-first) order. `buckets[h]` holds the proposals at
/// hop `h`; `cursor` is the lowest hop that may still hold items. Draining
/// lowest-hop-first, and within a hop in FIFO (push) order, reproduces the old
/// `BinaryHeap<HeapItem>`'s `(hops, seq)`-ascending pop order EXACTLY — `seq` was
/// just a monotonic push counter, so FIFO-within-hop IS `seq` order — so a lane's
/// first arrival at a fact is still its minimum hop count (the witness
/// shortest-path tiebreak, component 7). The win over the heap: O(1) push/pop
/// (no `log n` compare cascade) and no fat `HeapItem` wrapper allocation.
///
/// The invariant that makes this exact: within an SCC drain, processing a hop-`h`
/// item only ever routes same-SCC proposals at hop `h + 1` (`hops + 1`), never
/// back into hop `h` or below — so once `cursor` advances past a hop, nothing
/// re-fills it, and each hop bucket is complete before it is drained.
struct HopQueue {
    buckets: Vec<VecDeque<Proposal>>,
    cursor: usize,
    len: usize,
}

impl HopQueue {
    fn new() -> Self {
        HopQueue {
            buckets: Vec::new(),
            cursor: 0,
            len: 0,
        }
    }

    fn push(&mut self, prop: Proposal) {
        let hop = prop.hops() as usize;
        if hop >= self.buckets.len() {
            self.buckets.resize_with(hop + 1, VecDeque::new);
        }
        self.buckets[hop].push_back(prop);
        self.len += 1;
        // Defensive: pushes never target a below-cursor hop during a drain (the
        // invariant above), but keep the cursor valid if one ever did so no item
        // is stranded in an already-skipped bucket.
        self.cursor = self.cursor.min(hop);
    }

    /// Pop the lowest-hop, earliest-pushed (FIFO) proposal — identical order to
    /// the old heap's `(hops, seq)` pop.
    fn pop(&mut self) -> Option<Proposal> {
        while self.cursor < self.buckets.len() {
            if let Some(p) = self.buckets[self.cursor].pop_front() {
                self.len -= 1;
                return Some(p);
            }
            self.cursor += 1;
        }
        None
    }

    /// Live item count (for the Hot-tier worklist-size instrumentation).
    fn len(&self) -> usize {
        self.len
    }
}

/// The batch fact solver's shared arenas (across all lanes in the batch), their
/// dedup indices, per-node fact lists (for scoring), and per-lane provenance.
///
/// `pub(crate)` (struct + the `*_hops`/`*_pred`/`*_origin` provenance fields
/// only): Task C3's `d1_witness` (a sibling module) builds a bounded
/// representative witness directly off these arrays — `*_origin` for the O(1)
/// seed lookup, `*_hops` for the authoritative total, `*_pred` for the one
/// bounded chain walk that yields the first-K/last-M hop steps. The fact
/// arenas/indices stay private — `d1_witness` never needs them, only the
/// caller-supplied `terminal_node`/`terminal_owner`/`terminal_op`.
pub(crate) struct BatchSolver {
    reach_facts: Vec<ReachFactB>,
    value_facts: Vec<ValueFactB>,
    pub(crate) reach_hops: Vec<[u32; BATCH_WIDTH]>,
    pub(crate) reach_pred: Vec<[ReachPredB; BATCH_WIDTH]>,
    /// Task C2: `reach_origin[fact][lane]` = the seed index that FIRST reached
    /// this fact on this lane — set incrementally in [`Self::commit_reach`] by
    /// following the SAME predecessor just recorded in `reach_pred` (a `Seed`
    /// originates itself; a `Hop` copies its parent reach fact's origin[lane]).
    /// Lets a representative witness (Task C3) recover a fact's seed in O(1)
    /// instead of walking its full predecessor chain. `u32::MAX` = unset (a
    /// lane absent from this fact — mirrors `ReachPredB::None`'s sentinel role).
    pub(crate) reach_origin: Vec<[u32; BATCH_WIDTH]>,
    pub(crate) value_hops: Vec<[u32; BATCH_WIDTH]>,
    pub(crate) value_pred: Vec<[ValuePredB; BATCH_WIDTH]>,
    /// Task C2: `value_origin[fact][lane]`, mirroring `reach_origin` for value
    /// facts. `HopFromValue` copies the parent VALUE fact's origin[lane];
    /// `HopFromReach` copies the parent REACH fact's origin[lane] (`pred`
    /// indexes into `reach_origin`, not `value_origin`, for that variant — the
    /// value fact was born from a reach arrival, not a prior value fact).
    pub(crate) value_origin: Vec<[u32; BATCH_WIDTH]>,
    reach_index: HashMap<(NodeIx, i64, bool), usize>,
    value_index: HashMap<(NodeIx, u16, ParamTemp, i64, bool), usize>,
    /// `pub(crate)`: `d1_witness`'s tests locate a fixture's terminal-node fact
    /// (the same way the scoring loop's `entry.node` lookup does) to build a
    /// `representative_witness` call without needing a full `TerminalPlan`.
    pub(crate) reach_at: Vec<Vec<usize>>,
    pub(crate) value_at: Vec<Vec<usize>>,
}

impl BatchSolver {
    fn new(n_nodes: usize) -> Self {
        BatchSolver {
            reach_facts: Vec::new(),
            value_facts: Vec::new(),
            reach_hops: Vec::new(),
            reach_pred: Vec::new(),
            reach_origin: Vec::new(),
            value_hops: Vec::new(),
            value_pred: Vec::new(),
            value_origin: Vec::new(),
            reach_index: HashMap::new(),
            value_index: HashMap::new(),
            reach_at: vec![Vec::new(); n_nodes],
            value_at: vec![Vec::new(); n_nodes],
        }
    }

    /// Commit an incoming reach arrival: create the fact key if absent, then set
    /// the lanes that are NEWLY present (`mask & !fact.mask`) — first arrival
    /// wins, recording each new lane's `hops` + `pred`. Returns `(fact idx, new
    /// bits)`; `new bits == 0` means nothing to propagate.
    fn commit_reach(
        &mut self,
        node: NodeIx,
        depth: i64,
        unc: bool,
        mask: u64,
        hops: u32,
        pred: ReachPredB,
    ) -> (usize, u64) {
        let key = (node, depth, unc);
        let idx = match self.reach_index.get(&key) {
            Some(&i) => i,
            None => {
                let i = self.reach_facts.len();
                self.reach_facts.push(ReachFactB {
                    node,
                    depth,
                    unc,
                    mask: 0,
                });
                self.reach_hops.push([0u32; BATCH_WIDTH]);
                self.reach_pred.push([ReachPredB::None; BATCH_WIDTH]);
                self.reach_origin.push([u32::MAX; BATCH_WIDTH]);
                self.reach_at[node as usize].push(i);
                self.reach_index.insert(key, i);
                i
            }
        };
        let fact = &mut self.reach_facts[idx];
        let new_bits = mask & !fact.mask;
        if new_bits == 0 {
            return (idx, 0);
        }
        fact.mask |= new_bits;
        let hops_arr = &mut self.reach_hops[idx];
        let pred_arr = &mut self.reach_pred[idx];
        let mut m = new_bits;
        while m != 0 {
            let lane = m.trailing_zeros() as usize;
            hops_arr[lane] = hops;
            pred_arr[lane] = pred;
            // Task C2: origin follows the SAME predecessor just recorded above.
            // `Hop`'s `pred` fact was committed at a strictly lower hop count
            // (the HopQueue drains in nondecreasing-hops order), so its
            // origin[lane] is already set — copying it here needs no chain walk.
            self.reach_origin[idx][lane] = match pred {
                ReachPredB::Seed { seed_index } => seed_index,
                ReachPredB::Hop { pred: p, .. } => self.reach_origin[p as usize][lane],
                ReachPredB::None => unreachable!(
                    "commit_reach: a newly-committed lane always carries a real predecessor"
                ),
            };
            m &= m - 1;
        }
        (idx, new_bits)
    }

    /// Commit an incoming value arrival (see [`Self::commit_reach`]).
    #[allow(clippy::too_many_arguments)]
    fn commit_value(
        &mut self,
        node: NodeIx,
        slot: u16,
        class: ParamTemp,
        depth: i64,
        unc: bool,
        mask: u64,
        hops: u32,
        pred: ValuePredB,
    ) -> (usize, u64) {
        let key = (node, slot, class, depth, unc);
        let idx = match self.value_index.get(&key) {
            Some(&i) => i,
            None => {
                let i = self.value_facts.len();
                self.value_facts.push(ValueFactB {
                    node,
                    slot,
                    class,
                    depth,
                    unc,
                    mask: 0,
                });
                self.value_hops.push([0u32; BATCH_WIDTH]);
                self.value_pred.push([ValuePredB::None; BATCH_WIDTH]);
                self.value_origin.push([u32::MAX; BATCH_WIDTH]);
                self.value_at[node as usize].push(i);
                self.value_index.insert(key, i);
                i
            }
        };
        let fact = &mut self.value_facts[idx];
        let new_bits = mask & !fact.mask;
        if new_bits == 0 {
            return (idx, 0);
        }
        fact.mask |= new_bits;
        let hops_arr = &mut self.value_hops[idx];
        let pred_arr = &mut self.value_pred[idx];
        let mut m = new_bits;
        while m != 0 {
            let lane = m.trailing_zeros() as usize;
            hops_arr[lane] = hops;
            pred_arr[lane] = pred;
            // Task C2: `HopFromValue`'s parent is a VALUE fact (copy
            // value_origin); `HopFromReach`'s parent is a REACH fact — its
            // `pred` indexes reach_origin, not value_origin (see the struct
            // doc). Both parents were committed at a strictly lower hop count,
            // so their origin[lane] is already set.
            self.value_origin[idx][lane] = match pred {
                ValuePredB::Seed { seed_index } => seed_index,
                ValuePredB::HopFromValue { pred: p, .. } => self.value_origin[p as usize][lane],
                ValuePredB::HopFromReach { pred: p, .. } => self.reach_origin[p as usize][lane],
                ValuePredB::None => unreachable!(
                    "commit_value: a newly-committed lane always carries a real predecessor"
                ),
            };
            m &= m - 1;
        }
        (idx, new_bits)
    }

    /// The lanes of `mask` NOT already committed at reach fact `(node, depth,
    /// unc)` — the only bits worth PROPOSING. A not-yet-created fact has an
    /// all-zero mask, so every bit is new. This moves the first-arrival dedup
    /// BEFORE the push (was: only at [`Self::commit_reach`], after the proposal
    /// had already been pushed AND popped — the dense-SCC heap blowup). It stays
    /// output-identical because the target mask only GROWS between push and pop,
    /// so `commit_reach` re-filters the carried bits to the exact same set
    /// regardless of what was filtered here.
    fn reach_new_bits(&self, node: NodeIx, depth: i64, unc: bool, mask: u64) -> u64 {
        match self.reach_index.get(&(node, depth, unc)) {
            Some(&i) => mask & !self.reach_facts[i].mask,
            None => mask,
        }
    }

    /// The lanes of `mask` NOT already committed at value fact `(node, slot,
    /// class, depth, unc)` (see [`Self::reach_new_bits`]).
    fn value_new_bits(
        &self,
        node: NodeIx,
        slot: u16,
        class: ParamTemp,
        depth: i64,
        unc: bool,
        mask: u64,
    ) -> u64 {
        match self.value_index.get(&(node, slot, class, depth, unc)) {
            Some(&i) => mask & !self.value_facts[i].mask,
            None => mask,
        }
    }
}

/// Route a generated proposal to the current SCC's min-hops worklist (target in
/// the SAME SCC) or the target SCC's pending buffer (a downstream SCC — the call
/// SCC condensation is a DAG once condensed, so a proposal never targets an
/// already-settled upstream SCC).
fn route(
    prop: Proposal,
    current_scc: u32,
    scc_of: &[u32],
    queue: &mut HopQueue,
    pending: &mut [Vec<Proposal>],
) {
    let target = scc_of[prop.node() as usize];
    if target == current_scc {
        queue.push(prop);
    } else {
        // Topological invariant: a cross-SCC edge only ever targets a STRICTLY
        // downstream (not-yet-drained) SCC (`scc_of[caller] < scc_of[callee]`).
        // If a future `condense` regression broke the numbering, this would
        // otherwise SILENTLY drop the proposal into an already-drained upstream
        // pending buffer — fail loudly instead.
        debug_assert!(
            target > current_scc,
            "route: proposal targets SCC {target} <= current {current_scc} — \
             condense produced a non-topological order (upstream pending is drained)"
        );
        pending[target as usize].push(prop);
    }
}

/// Walk a reach fact's per-lane first-arrival predecessor chain to its seed,
/// collecting the `(from_node, edge_k)` hops in TERMINAL->SEED order (the batch
/// analogue of [`collect_reach_chain`], reading the per-lane predecessor).
///
/// `pub(crate)`: Task C3's `d1_witness` calls this to collect the bounded
/// witness's hop steps (the seed index it also returns is cross-checked
/// against `BatchSolver::reach_origin`'s O(1) value, not used to FIND it).
pub(crate) fn collect_reach_chain_b(
    reach_pred: &[[ReachPredB; BATCH_WIDTH]],
    lane: usize,
    start: usize,
) -> (Vec<(NodeIx, usize)>, usize) {
    let mut hops: Vec<(NodeIx, usize)> = Vec::new();
    let mut cur = start;
    loop {
        match reach_pred[cur][lane] {
            ReachPredB::Seed { seed_index } => return (hops, seed_index as usize),
            ReachPredB::Hop {
                pred,
                from_node,
                edge_k,
            } => {
                hops.push((from_node, edge_k as usize));
                cur = pred as usize;
            }
            ReachPredB::None => unreachable!("a present lane always has a reach predecessor"),
        }
    }
}

/// Walk a value fact's per-lane first-arrival predecessor chain (the batch
/// analogue of [`collect_value_chain`]); a `HopFromReach` switches the walk onto
/// the caller's reach chain for the remainder.
///
/// `pub(crate)`: see [`collect_reach_chain_b`]'s doc — `d1_witness` calls this
/// for the value-fact witness case.
pub(crate) fn collect_value_chain_b(
    value_pred: &[[ValuePredB; BATCH_WIDTH]],
    reach_pred: &[[ReachPredB; BATCH_WIDTH]],
    lane: usize,
    start: usize,
) -> (Vec<(NodeIx, usize)>, usize) {
    let mut hops: Vec<(NodeIx, usize)> = Vec::new();
    let mut cur = start;
    loop {
        match value_pred[cur][lane] {
            ValuePredB::Seed { seed_index } => return (hops, seed_index as usize),
            ValuePredB::HopFromValue {
                pred,
                from_node,
                edge_k,
            } => {
                hops.push((from_node, edge_k as usize));
                cur = pred as usize;
            }
            ValuePredB::HopFromReach {
                pred,
                from_node,
                edge_k,
            } => {
                hops.push((from_node, edge_k as usize));
                let (mut rest, seed_index) = collect_reach_chain_b(reach_pred, lane, pred as usize);
                hops.append(&mut rest);
                return (hops, seed_index);
            }
            ValuePredB::None => unreachable!("a present lane always has a value predecessor"),
        }
    }
}

/// Walk a reach fact's per-lane first-arrival predecessor chain, collecting AT
/// MOST `limit` `(from_node, edge_k)` hops nearest the TERMINAL (terminal->seed
/// order) — Task C8's BOUNDED analogue of [`collect_reach_chain_b`]. O(limit),
/// never O(total hops): the walk stops the instant it has collected `limit`
/// hops, without ever reaching the seed for a deep chain. The `Seed` arm is
/// checked FIRST, unconditionally (before the length check), so a chain whose
/// TRUE length is `<= limit` still walks to completion and reports its real
/// seed — the boundary case `total_hops == limit` correctly returns
/// `Some(seed_index)` with exactly `limit` hops collected, not `None`.
///
/// Returns `(hops, Some(seed_index))` when the walk reached the seed (the
/// chain's true length is `<= limit`) or `(hops, None)` when it was capped
/// (the chain is deeper than `limit` — the caller already knows the
/// authoritative `total_hops` via `BatchSolver::reach_hops`, so this walk never
/// needs to discover the chain's true length itself).
pub(crate) fn collect_reach_chain_b_bounded(
    reach_pred: &[[ReachPredB; BATCH_WIDTH]],
    lane: usize,
    start: usize,
    limit: usize,
) -> (Vec<(NodeIx, usize)>, Option<usize>) {
    let mut hops: Vec<(NodeIx, usize)> = Vec::with_capacity(limit);
    let mut cur = start;
    loop {
        match reach_pred[cur][lane] {
            ReachPredB::Seed { seed_index } => return (hops, Some(seed_index as usize)),
            ReachPredB::Hop {
                pred,
                from_node,
                edge_k,
            } => {
                if hops.len() >= limit {
                    return (hops, None);
                }
                hops.push((from_node, edge_k as usize));
                cur = pred as usize;
            }
            ReachPredB::None => unreachable!("a present lane always has a reach predecessor"),
        }
    }
}

/// Walk a value fact's per-lane first-arrival predecessor chain, collecting AT
/// MOST `limit` hops nearest the terminal (the bounded analogue of
/// [`collect_value_chain_b`], mirroring [`collect_reach_chain_b_bounded`]'s
/// Seed-first-unconditional check for the same boundary-exactness reason). A
/// `HopFromReach` mid-chain (the hop that BORN the value class via a `Const`
/// transfer) consumes one unit of the remaining budget, then hands the rest to
/// [`collect_reach_chain_b_bounded`] for the tail — so a last-M window that
/// happens to straddle the value/reach transition still walks it correctly, in
/// O(limit) total (never O(total hops)).
pub(crate) fn collect_value_chain_b_bounded(
    value_pred: &[[ValuePredB; BATCH_WIDTH]],
    reach_pred: &[[ReachPredB; BATCH_WIDTH]],
    lane: usize,
    start: usize,
    limit: usize,
) -> (Vec<(NodeIx, usize)>, Option<usize>) {
    let mut hops: Vec<(NodeIx, usize)> = Vec::with_capacity(limit);
    let mut cur = start;
    loop {
        match value_pred[cur][lane] {
            ValuePredB::Seed { seed_index } => return (hops, Some(seed_index as usize)),
            ValuePredB::HopFromValue {
                pred,
                from_node,
                edge_k,
            } => {
                if hops.len() >= limit {
                    return (hops, None);
                }
                hops.push((from_node, edge_k as usize));
                cur = pred as usize;
            }
            ValuePredB::HopFromReach {
                pred,
                from_node,
                edge_k,
            } => {
                if hops.len() >= limit {
                    return (hops, None);
                }
                hops.push((from_node, edge_k as usize));
                let remaining = limit - hops.len();
                let (mut rest, seed_index) =
                    collect_reach_chain_b_bounded(reach_pred, lane, pred as usize, remaining);
                hops.append(&mut rest);
                return (hops, seed_index);
            }
            ValuePredB::None => unreachable!("a present lane always has a value predecessor"),
        }
    }
}

/// The read-mode + precomputed verdict/severity tables for ONE terminal, all
/// batch-INDEPENDENT (computed ONCE per d1 run — see [`build_terminal_plan`]).
/// `Reach` = the terminal reads a caller-independent verdict (a constant op, or
/// a closed-world-proven PD whose class resolves to `Temp` without reading any
/// value); `Value` = the verdict depends on the resolved [`ParamTemp`] class of
/// the read slot. Mirrors `solve_batch`'s old per-batch `read_slots` +
/// `flowfield_verdict`/`severity_for` recomputation, hoisted out of the batch loop.
enum ReadPlan {
    Reach {
        verdict: TempVerdict,
        /// `severity_for(op, verdict, db, is_singleton)` for `db ∈ {0, 1, 2}`.
        sev_by_bucket: [&'static str; 3],
    },
    Value {
        slot: u16,
        /// Verdict per [`ParamTemp`] class (`Temp` = 0, `Physical` = 1,
        /// `Unknown` = 2 — the `class as usize` index).
        verdict_by_class: [TempVerdict; 3],
        /// `severity_for` per (class, depth-bucket) pair.
        sev_by_class_bucket: [[&'static str; 3]; 3],
    },
}

/// One graph terminal `(node, ti)` with its batch-independent read plan. Because
/// each `(owner, op)` terminal lives at exactly ONE node (the owner routine's
/// node), the entry's key `(owner.id, op.id)` is unique across the plan.
struct TermEntry<'a> {
    node: NodeIx,
    op: &'a ModelRecordOperation,
    owner: &'a ModelRoutine,
    local_depth: i64,
    read: ReadPlan,
}

/// The run-global terminal scoring plan — every graph terminal's read mode +
/// verdict/severity tables, precomputed ONCE (all batch-INDEPENDENT) and shared
/// across every batch. This hoists what `solve_batch`'s scoring loop used to
/// rebuild per batch: `terminal_nodes`, `read_slots`, `is_setup_singleton_get`,
/// the reach-case verdict, and the per-class value verdicts + severity tables.
/// Entries are in ascending `(node, ti)` order — the SAME order the old scoring
/// loop visited terminals, so the discovery (source-order) tie-break is exact.
pub(crate) struct TerminalPlan<'a> {
    entries: Vec<TermEntry<'a>>,
}

impl<'a> TerminalPlan<'a> {
    /// Number of terminal entries — the dense `TerminalSink` size.
    pub(crate) fn terminal_count(&self) -> usize {
        self.entries.len()
    }
}

/// Build the run-global [`TerminalPlan`]. Called ONCE (in `search_loops`) before
/// the batch loop and shared by every `solve_batch` call — never rebuilt per batch.
pub(crate) fn build_terminal_plan<'a>(
    graph: &D1Graph<'a>,
    liveness: &Liveness,
    ctx: &'a DetectorContext,
    cw: &ClosedWorldTempParams,
) -> TerminalPlan<'a> {
    const CLASSES: [ParamTemp; 3] = [ParamTemp::Temp, ParamTemp::Physical, ParamTemp::Unknown];
    let n_nodes = graph.node_ids.len();
    let mut entries: Vec<TermEntry<'a>> = Vec::new();
    for node in 0..n_nodes as NodeIx {
        for (ti, t) in graph.terminals[node as usize].iter().enumerate() {
            let op = t.op;
            let owner = t.owner;
            let local_depth = t.local_depth;
            let is_singleton = is_setup_singleton_get(op, Some(owner), &ctx.table_by_id);

            // Read mode + closed-world-proven re-check — mirrors the old
            // `read_slots` precompute EXACTLY (`Some(slot)` = value-based read,
            // `None` = reach-based: a constant op or a proven-PD terminal).
            let read_slot: Option<u16> = match liveness.terminal_reads[node as usize][ti] {
                None => None,
                Some(slot) => {
                    let i = liveness.need[node as usize][slot as usize];
                    if cw.contains(&(owner.id.to_string(), i)) {
                        None
                    } else {
                        Some(slot)
                    }
                }
            };

            let read = match read_slot {
                None => {
                    let verdict = flowfield_verdict(
                        resolve_terminal(op, &TempVec::new(), owner.id.as_str(), cw),
                        op,
                        &ctx.table_by_id,
                    );
                    let mut sev_by_bucket = [""; 3];
                    for (db, s) in sev_by_bucket.iter_mut().enumerate() {
                        *s = severity_for(op, verdict, db as i64, is_singleton);
                    }
                    ReadPlan::Reach {
                        verdict,
                        sev_by_bucket,
                    }
                }
                Some(slot) => {
                    let i = liveness.need[node as usize][slot as usize];
                    let mut verdict_by_class = [TempVerdict::Temporary; 3];
                    let mut sev_by_class_bucket = [[""; 3]; 3];
                    for (ci, &class) in CLASSES.iter().enumerate() {
                        let frame: TempVec = std::iter::once((i, class)).collect();
                        let verdict = flowfield_verdict(
                            resolve_terminal(op, &frame, owner.id.as_str(), cw),
                            op,
                            &ctx.table_by_id,
                        );
                        verdict_by_class[ci] = verdict;
                        for (db, s) in sev_by_class_bucket[ci].iter_mut().enumerate() {
                            *s = severity_for(op, verdict, db as i64, is_singleton);
                        }
                    }
                    ReadPlan::Value {
                        slot,
                        verdict_by_class,
                        sev_by_class_bucket,
                    }
                }
            };

            entries.push(TermEntry {
                node,
                op,
                owner,
                local_depth,
                read,
            });
        }
    }
    TerminalPlan { entries }
}

/// The winning candidate's source for one (lane, terminal-key) — enough to
/// materialize its witness. `Reach`/`Value` index the batch fact arenas; `Direct`
/// carries the in-loop op's own witness inputs (its loop + owner + op).
#[derive(Clone, Copy)]
enum BestSource<'a> {
    Direct {
        routine: &'a ModelRoutine,
        loop_info: &'a PLoop,
        op: &'a ModelRecordOperation,
        // Read back only by the `#[cfg(test)]` oracle (`emit_lane_aggregates`)
        // — `build_cohort_rep` (live) derives a Direct winner's witness from
        // `routine`/`loop_info`/`op` alone via `direct_witness`, ignoring this.
        #[cfg_attr(not(test), allow(dead_code))]
        local_depth: i64,
    },
    Reach {
        fact_ix: usize,
    },
    Value {
        fact_ix: usize,
    },
}

/// The running-best candidate for one lane at one terminal key. `rank` is the
/// FIRST FIVE components of [`selection_rank`] (severity, verdict-quality,
/// `unc == false`, `-hops`, `depth_bucket`). The sixth component (`-discovery`)
/// is NOT stored: candidates are folded in the exact old push order (direct ops
/// first, then facts in `reach_at`/`value_at` order) and [`update_best`] uses a
/// STRICT-greater compare, so the first-in-order wins every tie — reproducing
/// "lowest discovery wins" EXACTLY without a discovery counter.
#[derive(Clone, Copy)]
struct BestRef<'a> {
    rank: (i32, i32, i32, i64, i64),
    verdict: TempVerdict,
    severity: &'static str,
    depth_bucket: i64,
    source: BestSource<'a>,
}

/// One direct in-loop db-op candidate for a lane, precomputed in the direct
/// phase and folded into its terminal-key's running best BEFORE that key's
/// transitive facts (direct precedence == lowest discovery).
struct DirectCand<'a> {
    lane: usize,
    verdict: TempVerdict,
    severity: &'static str,
    depth_bucket: i64,
    rank: (i32, i32, i32, i64, i64),
    routine: &'a ModelRoutine,
    loop_info: &'a PLoop,
    op: &'a ModelRecordOperation,
    local_depth: i64,
}

/// The first FIVE [`selection_rank`] components (drops the `-discovery` tail —
/// see [`BestRef`]). Reuses `selection_rank` so the component formula is shared,
/// not duplicated.
#[inline]
fn rank5(
    severity: &str,
    verdict: TempVerdict,
    unc: bool,
    hops: u32,
    depth_bucket: i64,
) -> (i32, i32, i32, i64, i64) {
    let r = selection_rank(severity, verdict, unc, hops, depth_bucket, 0);
    (r.0, r.1, r.2, r.3, r.4)
}

/// STRICT-greater running-best update: replace `best[lane]` iff `cand.rank`
/// beats the stored rank. On a tie the incumbent (first-in-order = lower
/// discovery) is kept — the byte-identity guarantee.
#[inline]
fn update_best<'a>(best: &mut [Option<BestRef<'a>>], lane: usize, cand: BestRef<'a>) {
    match &best[lane] {
        Some(b) if cand.rank <= b.rank => {}
        _ => best[lane] = Some(cand),
    }
}

/// Fold a direct candidate into a terminal-key's running best + verdict masks.
fn fold_direct<'a>(c: &DirectCand<'a>, best: &mut [Option<BestRef<'a>>], vmask: &mut [u64; 4]) {
    vmask[c.verdict as usize] |= 1u64 << c.lane;
    update_best(
        best,
        c.lane,
        BestRef {
            rank: c.rank,
            verdict: c.verdict,
            severity: c.severity,
            depth_bucket: c.depth_bucket,
            source: BestSource::Direct {
                routine: c.routine,
                loop_info: c.loop_info,
                op: c.op,
                local_depth: c.local_depth,
            },
        },
    );
}

/// The distinct verdicts reaching `lane`, in [`TempVerdict`] declaration order
/// (== the sorted+deduped order the old `reachable_verdicts` Vec produced). Built
/// from the four per-verdict lane masks — replacing the old per-candidate verdict
/// collection + sort + dedup with four `u64` bit tests. `emit_lane_aggregates`-only
/// (oracle) — `sink_emit` computes the cohort path's equivalent inline.
#[cfg(test)]
fn reachable_from_masks(vmask: &[u64; 4], lane: usize) -> Vec<TempVerdict> {
    const ORDER: [TempVerdict; 4] = [
        TempVerdict::Temporary,
        TempVerdict::Physical,
        TempVerdict::Uncertain,
        TempVerdict::FlowFieldGated,
    ];
    let bit = 1u64 << lane;
    let mut out = Vec::new();
    for (i, v) in ORDER.iter().enumerate() {
        if vmask[i] & bit != 0 {
            out.push(*v);
        }
    }
    out
}

/// Emit one `LoopTerminalAgg` per present lane at one terminal key: for each
/// lane whose running-best is set, materialize the winner's witness (via the
/// existing per-lane predecessor walkers) and derive `reachable_verdicts` from
/// the verdict masks. `owner`/`op`/`local_depth` are the terminal's metadata
/// (used by the Reach/Value arms); a `Direct` winner carries its own.
/// `solve_batch`-only (oracle) — the cohort path's analogue is `sink_emit`.
#[cfg(test)]
#[allow(clippy::too_many_arguments)]
fn emit_lane_aggregates<'a>(
    best: &[Option<BestRef<'a>>],
    vmask: &[u64; 4],
    lanes: usize,
    batch: &[GroupSpec<'a>],
    owner: &'a ModelRoutine,
    op: &'a ModelRecordOperation,
    local_depth: i64,
    solver: &BatchSolver,
    graph: &D1Graph<'a>,
    ctx: &'a DetectorContext,
    seeds: &[D1Seed<'a>],
    out: &mut Vec<LoopTerminalAgg<'a>>,
) {
    for (lane, group) in batch.iter().enumerate().take(lanes) {
        let Some(b) = &best[lane] else {
            continue;
        };
        let reachable_verdicts = reachable_from_masks(vmask, lane);

        let (witness, uncertainties, entry_callsite_id, effective_loop_depth, t_owner, t_op, t_ld) =
            match b.source {
                BestSource::Direct {
                    routine,
                    loop_info,
                    op: dop,
                    local_depth: dld,
                } => {
                    let loop_step = loop_step_ev(routine, loop_info);
                    let op_step = terminal_step(
                        &ctx.routine_by_id,
                        &ctx.table_by_id,
                        routine.id.as_str(),
                        Some(dop.id.as_str()),
                    );
                    (
                        vec![loop_step, op_step],
                        Vec::new(),
                        None,
                        dld,
                        routine,
                        dop,
                        dld,
                    )
                }
                BestSource::Reach { fact_ix } => {
                    let terminal_node = solver.reach_facts[fact_ix].node;
                    let (hops, seed_index) =
                        collect_reach_chain_b(&solver.reach_pred, lane, fact_ix);
                    let (w, u, cs, eff) = build_transitive_witness(
                        &hops,
                        seed_index,
                        terminal_node,
                        owner,
                        op,
                        local_depth,
                        graph,
                        ctx,
                        seeds,
                    );
                    (w, u, cs, eff, owner, op, local_depth)
                }
                BestSource::Value { fact_ix } => {
                    let terminal_node = solver.value_facts[fact_ix].node;
                    let (hops, seed_index) = collect_value_chain_b(
                        &solver.value_pred,
                        &solver.reach_pred,
                        lane,
                        fact_ix,
                    );
                    let (w, u, cs, eff) = build_transitive_witness(
                        &hops,
                        seed_index,
                        terminal_node,
                        owner,
                        op,
                        local_depth,
                        graph,
                        ctx,
                        seeds,
                    );
                    (w, u, cs, eff, owner, op, local_depth)
                }
            };

        out.push(LoopTerminalAgg {
            loop_routine: group.loop_routine,
            loop_id: group.loop_id,
            loop_info: group.loop_info,
            terminal: D1Terminal {
                op: t_op,
                owner: t_owner,
                local_depth: t_ld,
            },
            entry_callsite_id,
            severity: b.severity,
            verdict: b.verdict,
            reachable_verdicts,
            depth_bucket: b.depth_bucket,
            effective_loop_depth,
            witness,
            uncertainties,
        });
    }
}

/// Solve a BATCH of up to [`BATCH_WIDTH`] loop groups sharing ONE call-SCC
/// condensation pass. Group `i` in `batch` owns bit `i` of the `u64` lane masks.
/// Returns one `LoopTerminalAgg` per (group, terminal-op) — components 1-6
/// identical to `solve_group` per group; witness (component 7) may differ.
///
/// `plan` is the run-global [`TerminalPlan`] (built ONCE per run, shared
/// across batches); the scoring phase reads its precomputed read-mode /
/// verdict / severity tables instead of rebuilding them per batch.
/// `#[cfg(test)]`-only: superseded in production by [`score_batch_to_sink`]
/// (the cohort-emitting sibling that reproduces this exact fixpoint); retained
/// as the `search_loops` differential oracle (see the module doc).
#[cfg(test)]
#[allow(clippy::too_many_arguments)]
pub(crate) fn solve_batch<'a>(
    graph: &D1Graph<'a>,
    liveness: &Liveness,
    scc: &CallScc,
    seeds: &[D1Seed<'a>],
    direct_ops: &[DirectOp<'a>],
    ctx: &'a DetectorContext,
    cw: &ClosedWorldTempParams,
    plan: &TerminalPlan<'a>,
    batch: &[GroupSpec<'a>],
) -> Vec<LoopTerminalAgg<'a>> {
    assert!(
        batch.len() <= BATCH_WIDTH,
        "a batch holds at most {BATCH_WIDTH} lanes"
    );
    let n_nodes = graph.node_ids.len();
    let mut solver = BatchSolver::new(n_nodes);

    // Hot-tier (`Detail::Hot`) attribution: the periodic in-fixpoint checkpoint +
    // per-batch summary confirm the worklist is BOUNDED (not the pre-fix
    // hundreds-of-millions accumulation). The gate is read ONCE here; every
    // increment/emission below is behind it, so the disabled path pays nothing.
    // `batch_seq` is a self-contained running batch ordinal (the "batch index"
    // label) — it needs no caller cooperation, so `solve_batch`'s signature stays
    // free of a trace-only param.
    let trace_hot = crate::engine::perf_trace::enabled(crate::engine::perf_trace::Detail::Hot);
    let mut pops: u64 = 0;
    let mut max_worklist: usize = 0;
    if trace_hot {
        crate::engine::perf_trace::counter_delta("d1.reach.batch_seq", 1);
    }

    // Per-node scalars used repeatedly during propagation (mirrors solve_group).
    let unc_by_node: Vec<bool> = graph
        .node_ids
        .iter()
        .map(|id| node_has_uncertainty(ctx, id))
        .collect();
    let expandable: Vec<bool> = graph
        .node_ids
        .iter()
        .map(|id| ctx.routine_by_id.contains_key(id))
        .collect();

    // Per-SCC pending arrivals from upstream SCCs (+ seeds). Drained when that
    // SCC is reached in topological order.
    let mut pending: Vec<Vec<Proposal>> = (0..scc.members.len()).map(|_| Vec::new()).collect();

    // Rule 1: seed every lane's frontier into its entry node's SCC pending
    // buffer. The seed entry `TempVec` is `cross_hop` of the group's loop-routine
    // root across the seed callsite (EXACTLY solve_group's seed label), projected
    // to the entry's live params.
    for (lane, group) in batch.iter().enumerate() {
        let bit = 1u64 << lane;
        let root = root_state(group.loop_routine.id.as_str(), cw);
        for &si in &group.seed_indices {
            let seed = &seeds[si];
            let entry = seed.entry;
            let entry_id = graph.node_ids[entry as usize];
            let binding_ok = edge_kind_binding_ok(seed.entry_edge_kind);
            let entry_temp = cross_hop(
                &root,
                seed.loop_routine,
                seed.callsite.id.as_str(),
                entry_id,
                binding_ok,
                cw,
            );
            let depth = seed.seed_depth.min(2);
            let unc = unc_by_node[entry as usize];
            let entry_scc = scc.scc_of[entry as usize] as usize;
            pending[entry_scc].push(Proposal::Reach {
                node: entry,
                depth,
                unc,
                mask: bit,
                hops: 0,
                pred: ReachPredB::Seed {
                    seed_index: si as u32,
                },
            });
            for (slot, &p) in liveness.need[entry as usize].iter().enumerate() {
                let class = lookup(&entry_temp, p);
                pending[entry_scc].push(Proposal::Value {
                    node: entry,
                    slot: slot as u16,
                    class,
                    depth,
                    unc,
                    mask: bit,
                    hops: 0,
                    pred: ValuePredB::Seed {
                        seed_index: si as u32,
                    },
                });
            }
        }
    }

    // Rules 2-3: process SCCs in topological order. Within each SCC, a min-hops
    // (level-synchronous) bucket-queue worklist drains to least-fixpoint — masks
    // only OR-in bits, so cycles terminate; min-hops-first pops make a lane's
    // first arrival its minimum hop count. Each generated proposal is filtered
    // through `*_new_bits` BEFORE it is enqueued: only the lanes NOT already
    // committed at the target fact are carried, and an all-redundant proposal is
    // never enqueued at all (the fix — see [`BatchSolver::reach_new_bits`]).
    for &scc_id in &scc.topo_order {
        let mut queue = HopQueue::new();
        for prop in std::mem::take(&mut pending[scc_id as usize]) {
            queue.push(prop);
        }
        if trace_hot && queue.len() > max_worklist {
            max_worklist = queue.len();
        }
        while let Some(prop) = queue.pop() {
            if trace_hot {
                pops += 1;
                let wl = queue.len();
                if wl > max_worklist {
                    max_worklist = wl;
                }
                if pops.is_multiple_of(100_000) {
                    let reach_facts = solver.reach_facts.len() as u64;
                    let value_facts = solver.value_facts.len() as u64;
                    let worklist = wl as u64;
                    crate::engine::perf_trace::instant_lazy("d1.reach", "batch_internal", || {
                        serde_json::json!({
                            "scc": scc_id,
                            "pops": pops,
                            "worklist": worklist,
                            "reach_facts": reach_facts,
                            "value_facts": value_facts,
                        })
                    });
                }
            }
            match prop {
                Proposal::Reach {
                    node,
                    depth,
                    unc,
                    mask,
                    hops,
                    pred,
                } => {
                    let (idx, new_bits) = solver.commit_reach(node, depth, unc, mask, hops, pred);
                    if new_bits == 0 || !expandable[node as usize] {
                        continue;
                    }
                    for (k, edge) in graph.edges[node as usize].iter().enumerate() {
                        let m = edge.to;
                        let d2 = (depth + edge.loop_depth).min(2);
                        let u2 = unc || unc_by_node[m as usize];
                        let rnb = solver.reach_new_bits(m, d2, u2, new_bits);
                        if rnb != 0 {
                            route(
                                Proposal::Reach {
                                    node: m,
                                    depth: d2,
                                    unc: u2,
                                    mask: rnb,
                                    hops: hops + 1,
                                    pred: ReachPredB::Hop {
                                        pred: idx as u32,
                                        from_node: node,
                                        edge_k: k as u32,
                                    },
                                },
                                scc_id,
                                &scc.scc_of,
                                &mut queue,
                                &mut pending,
                            );
                        }
                        for (callee_slot, transfer) in
                            liveness.edge_transfers[node as usize][k].iter().enumerate()
                        {
                            if let ParamTransfer::Const(pt) = transfer {
                                let vnb = solver.value_new_bits(
                                    m,
                                    callee_slot as u16,
                                    *pt,
                                    d2,
                                    u2,
                                    new_bits,
                                );
                                if vnb != 0 {
                                    route(
                                        Proposal::Value {
                                            node: m,
                                            slot: callee_slot as u16,
                                            class: *pt,
                                            depth: d2,
                                            unc: u2,
                                            mask: vnb,
                                            hops: hops + 1,
                                            pred: ValuePredB::HopFromReach {
                                                pred: idx as u32,
                                                from_node: node,
                                                edge_k: k as u32,
                                            },
                                        },
                                        scc_id,
                                        &scc.scc_of,
                                        &mut queue,
                                        &mut pending,
                                    );
                                }
                            }
                        }
                    }
                }
                Proposal::Value {
                    node,
                    slot,
                    class,
                    depth,
                    unc,
                    mask,
                    hops,
                    pred,
                } => {
                    let (idx, new_bits) =
                        solver.commit_value(node, slot, class, depth, unc, mask, hops, pred);
                    if new_bits == 0 || !expandable[node as usize] {
                        continue;
                    }
                    for (k, edge) in graph.edges[node as usize].iter().enumerate() {
                        let m = edge.to;
                        let d2 = (depth + edge.loop_depth).min(2);
                        let u2 = unc || unc_by_node[m as usize];
                        for (callee_slot, transfer) in
                            liveness.edge_transfers[node as usize][k].iter().enumerate()
                        {
                            if let ParamTransfer::Copy { caller_slot } = transfer
                                && *caller_slot == slot
                            {
                                let vnb = solver.value_new_bits(
                                    m,
                                    callee_slot as u16,
                                    class,
                                    d2,
                                    u2,
                                    new_bits,
                                );
                                if vnb != 0 {
                                    route(
                                        Proposal::Value {
                                            node: m,
                                            slot: callee_slot as u16,
                                            class,
                                            depth: d2,
                                            unc: u2,
                                            mask: vnb,
                                            hops: hops + 1,
                                            pred: ValuePredB::HopFromValue {
                                                pred: idx as u32,
                                                from_node: node,
                                                edge_k: k as u32,
                                            },
                                        },
                                        scc_id,
                                        &scc.scc_of,
                                        &mut queue,
                                        &mut pending,
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // Per-batch summary (Hot only): pops, peak worklist, and the final fact-arena
    // sizes — the bound-confirmation counter the fix exists to make observable.
    if trace_hot {
        let reach_facts = solver.reach_facts.len() as u64;
        let value_facts = solver.value_facts.len() as u64;
        let max_worklist = max_worklist as u64;
        let lanes = batch.len() as u64;
        let total_pops = pops;
        crate::engine::perf_trace::instant_lazy("d1.reach", "batch_summary", || {
            serde_json::json!({
                "lanes": lanes,
                "pops": total_pops,
                "max_worklist": max_worklist,
                "reach_facts": reach_facts,
                "value_facts": value_facts,
            })
        });
    }

    // Rules 4-7: score terminals + select winner + materialize witness PER LANE.
    // The old per-batch fan-out — a `BTreeMap<key, Vec<Candidate>>` per lane fed
    // ~100M fat `Candidate` pushes to pick a few thousand winners — is GONE.
    // Scoring is now TERMINAL-outer with a per-lane running-best array + four
    // verdict masks: each terminal-bearing node's facts are scanned once into a
    // fixed `[Option<BestRef>; 64]` (no allocation, no map lookup in the hot
    // loop), and only the winner per (lane, key) is materialized. All
    // batch-INDEPENDENT terminal interpretation (read mode, verdicts, severity
    // tables) was hoisted ONCE into `plan` (see `build_terminal_plan`).
    let _scoring_span = crate::engine::perf_trace::span("d1.reach", "scoring");
    let lanes = batch.len();

    // Direct ops first (branch (a) precedence). Grouped by terminal key so a
    // key's directs can be folded into its running best BEFORE its transitive
    // facts (the old direct-before-transitive discovery order). A direct op's key
    // = (loop_routine.id, op.id); its terminal owner IS the loop routine. Within
    // a lane the pushes stay in `direct_indices` order (the tie-break the old
    // per-lane discovery counter encoded).
    let mut direct_by_key: HashMap<(&'a str, &'a str), Vec<DirectCand<'a>>> = HashMap::new();
    for (lane, group) in batch.iter().enumerate() {
        let root = root_state(group.loop_routine.id.as_str(), cw);
        for &di in &group.direct_indices {
            let d = &direct_ops[di];
            let op = d.op;
            let owner = d.routine;
            let base_pt = resolve_terminal(op, &root, owner.id.as_str(), cw);
            let verdict = flowfield_verdict(base_pt, op, &ctx.table_by_id);
            let local_depth = op.loop_stack.len() as i64;
            let depth_bucket = local_depth.min(2);
            let is_singleton = is_setup_singleton_get(op, Some(owner), &ctx.table_by_id);
            let severity = severity_for(op, verdict, depth_bucket, is_singleton);
            direct_by_key
                .entry((owner.id.as_str(), op.id.as_str()))
                .or_default()
                .push(DirectCand {
                    lane,
                    verdict,
                    severity,
                    depth_bucket,
                    rank: rank5(severity, verdict, false, 0, depth_bucket),
                    routine: owner,
                    loop_info: d.loop_info,
                    op,
                    local_depth,
                });
        }
    }

    let mut out: Vec<LoopTerminalAgg<'a>> = Vec::new();
    // Terminal keys whose direct ops were already folded + emitted below, so the
    // direct-only pass doesn't re-emit them.
    let mut consumed_direct_keys: HashSet<(&'a str, &'a str)> = HashSet::new();

    // Transitive scoring, TERMINAL-outer. `plan.entries` is in ascending
    // (node, ti) order, so folding a key's directs first and then its facts in
    // `reach_at`/`value_at` order reproduces the OLD per-lane push order EXACTLY
    // — the strict-greater `update_best` then keeps the first-in-order (lowest
    // discovery) on every tie, so the winner (and thus the witness) is identical.
    for entry in &plan.entries {
        let node = entry.node;
        // A terminal node NOT reached by this batch can emit no TRANSITIVE
        // candidate; skip the fact scan. Its direct-only ops (if any) are emitted
        // by the direct-only pass below — byte-identical to the old code, whose
        // direct phase ran unconditionally and whose transitive phase fell through
        // the empty fact loops. This skips the ~99% of terminal-bearing nodes a
        // batch never reaches (the old post-fixpoint dominant cost).
        if solver.reach_at[node as usize].is_empty() && solver.value_at[node as usize].is_empty() {
            continue;
        }
        let key = (entry.owner.id.as_str(), entry.op.id.as_str());
        let mut best: [Option<BestRef<'a>>; BATCH_WIDTH] = [None; BATCH_WIDTH];
        let mut vmask = [0u64; 4];

        // Direct precedence: fold this key's direct ops before its facts.
        if let Some(cands) = direct_by_key.get(&key) {
            for c in cands {
                fold_direct(c, &mut best, &mut vmask);
            }
            consumed_direct_keys.insert(key);
        }

        match &entry.read {
            ReadPlan::Reach {
                verdict,
                sev_by_bucket,
            } => {
                let verdict = *verdict;
                for &ri in &solver.reach_at[node as usize] {
                    let f = &solver.reach_facts[ri];
                    let db = (f.depth + entry.local_depth).min(2);
                    let severity = sev_by_bucket[db as usize];
                    vmask[verdict as usize] |= f.mask;
                    // severity/verdict/unc/db are per-FACT constants; only `hops`
                    // varies per lane — hoist the rest out of the lane fan-out.
                    let sev_r = sev_rank(severity);
                    let q = verdict.quality();
                    let unc_pref = if f.unc { 0 } else { 1 };
                    let hops = &solver.reach_hops[ri];
                    let mut m = f.mask;
                    while m != 0 {
                        let lane = m.trailing_zeros() as usize;
                        m &= m - 1;
                        let rank = (sev_r, q, unc_pref, -(hops[lane] as i64), db);
                        // STRICT-greater keeps the first-in-order (lowest discovery)
                        // on ties; construct `BestRef` ONLY on a win.
                        if best[lane].is_none_or(|b| rank > b.rank) {
                            best[lane] = Some(BestRef {
                                rank,
                                verdict,
                                severity,
                                depth_bucket: db,
                                source: BestSource::Reach { fact_ix: ri },
                            });
                        }
                    }
                }
            }
            ReadPlan::Value {
                slot,
                verdict_by_class,
                sev_by_class_bucket,
            } => {
                let slot = *slot;
                for &vi in &solver.value_at[node as usize] {
                    let f = &solver.value_facts[vi];
                    if f.slot != slot {
                        continue;
                    }
                    let ci = f.class as usize;
                    let verdict = verdict_by_class[ci];
                    let db = (f.depth + entry.local_depth).min(2);
                    let severity = sev_by_class_bucket[ci][db as usize];
                    vmask[verdict as usize] |= f.mask;
                    let sev_r = sev_rank(severity);
                    let q = verdict.quality();
                    let unc_pref = if f.unc { 0 } else { 1 };
                    let hops = &solver.value_hops[vi];
                    let mut m = f.mask;
                    while m != 0 {
                        let lane = m.trailing_zeros() as usize;
                        m &= m - 1;
                        let rank = (sev_r, q, unc_pref, -(hops[lane] as i64), db);
                        if best[lane].is_none_or(|b| rank > b.rank) {
                            best[lane] = Some(BestRef {
                                rank,
                                verdict,
                                severity,
                                depth_bucket: db,
                                source: BestSource::Value { fact_ix: vi },
                            });
                        }
                    }
                }
            }
        }

        emit_lane_aggregates(
            &best,
            &vmask,
            lanes,
            batch,
            entry.owner,
            entry.op,
            entry.local_depth,
            &solver,
            graph,
            ctx,
            seeds,
            &mut out,
        );
    }

    // Direct-only keys: any terminal key with direct ops that was NOT scored above
    // (no graph terminal, or its owner node was unreached this batch). No
    // transitive facts exist for these, so the winner is always a Direct source —
    // identical to the old code's direct-only buckets. Key iteration order is
    // irrelevant: `search_loops`'s rule-8 sort canonicalizes the output.
    for (key, cands) in &direct_by_key {
        if consumed_direct_keys.contains(key) {
            continue;
        }
        let mut best: [Option<BestRef<'a>>; BATCH_WIDTH] = [None; BATCH_WIDTH];
        let mut vmask = [0u64; 4];
        for c in cands {
            fold_direct(c, &mut best, &mut vmask);
        }
        // Terminal metadata for the Reach/Value arms (never taken here — the
        // winner is always Direct, which carries its own witness inputs). Any
        // cand's owner/op/local_depth serve; they all share the key.
        let any = &cands[0];
        emit_lane_aggregates(
            &best,
            &vmask,
            lanes,
            batch,
            any.routine,
            any.op,
            any.local_depth,
            &solver,
            graph,
            ctx,
            seeds,
            &mut out,
        );
    }
    out
}

// ===========================================================================
// Task C1 — the terminal bitmap-COHORT emission (`score_batch_to_sink`).
//
// The cohort-redesign SINK path: the SAME fixpoint + SAME running-best scan as
// `solve_batch` above, but the winner per (lane, terminal) is EMITTED into a
// [`TerminalSink`] cohort (a `ContextKey` + the loop's bit) instead of a
// materialized `LoopTerminalAgg` witness. This is the speed-critical change —
// no per-(loop, terminal) witness build, no ~28k-hop predecessor walk.
// `detect_d1` is CUT OVER to this path (Tasks C5+C6, `d1_reach::search_loops_cohorts`);
// `solve_batch`/`emit_lane_aggregates` survive only as the `score_batch_to_sink_matches_old`
// differential's oracle below, which proves `decompress(sink)` equals the old
// aggregates on verdict / depth_bucket / unc / coverage / reachable_verdicts.
// ===========================================================================

/// The bounded-representative-witness slice bound (Task C8): the last `M` hop
/// steps nearest the terminal are kept, the rest summarized as `omitted_hops`.
/// Task C3 originally also kept a first-`K` prefix nearest the seed, but
/// materializing it required walking the chain FORWARD from the seed — an
/// O(total_hops) walk that dominated the ~220s cohort-build gap on the 8020
/// corpus (~28k-hop chains). Task C8 drops that prefix entirely: `first_steps`
/// is now always just `[loop_step, call_step]` (O(1) via `*_origin`), and
/// `last_steps` is collected by [`collect_reach_chain_b_bounded`]/
/// [`collect_value_chain_b_bounded`] — a BACKWARD walk from the terminal capped
/// at `M` hops, O(M) regardless of chain depth. `M = 4` covers every fixture
/// path (all ≤ 4 hops) whole (no omission), and bounds the DO/8020 witness size
/// for the deep chains that drove the blowup.
pub(crate) const WITNESS_M_LAST: usize = 4;

/// The uncertainty union along a representative predecessor chain, as INTERNED
/// [`UncertaintyId`]s — the SAME seed→terminal-node concat + dedupe that
/// [`build_transitive_witness`] computes with owned values, factored out so the
/// cohort path builds it WITHOUT materializing the full witness. `hops` is
/// TERMINAL→SEED order (as `collect_reach_chain_b`/`collect_value_chain_b`
/// return); the path nodes are the hops' `from_node`s (seed entry … pre-terminal,
/// in seed→terminal order) plus `terminal_node`. Preserving this exact order +
/// dedup makes the cohort finding's confidence BYTE-IDENTICAL to the old per-loop
/// winner's confidence (the cohort's first-seen representative IS the old
/// winner-selection's lowest-`(loop_routine_id, loop_id)` reaching loop — see
/// [`CohortRep`]).
///
/// The ONLY difference from the owned-value form is representation:
/// [`UncertaintyTable::intern`] is value-keyed, so the id sequence is a faithful
/// relabelling of the value sequence, and [`UncertaintyTable::dedupe`] reproduces
/// [`crate::engine::l4::summary::dedupe_uncertainties`]' key ordering and
/// last-write-wins collision rule (argued in its own doc). Nothing is sampled,
/// capped, or dropped.
/// The representative path's uncertainty union, interned into `table`.
///
/// Walks the path seed→terminal and unions each node's uncertainty set, deduped
/// last-write-wins by `uncertainty_key` — the order-sensitive rule
/// `dedupe_uncertainties` has always applied (two `interface-open-world` values
/// differing only in `interfaceName` share a key, and the LATER one wins).
///
/// **Two memo levels, both keyed on hash-consed set identity** (see
/// [`PathUncertaintyCache`]). Censused on 8020, this function was interning
/// 92,054,600 uncertainties per run — 4,250 per walk at ~105 ns each — because
/// every walk re-interned every element of every set it crossed and then deduped
/// the whole 4,250-element concatenation through a `BTreeMap<&str, _>`. The sets
/// are shared allocations, so both levels of that work were re-derivations.
///
/// **Why dropping an earlier duplicate set is EXACTLY equivalent.** Only the LAST
/// occurrence of each distinct set is kept before hashing the signature. Under
/// last-write-wins, a key's winner is the id at its last position in the
/// concatenation; an identical set contributes the identical `(key, id)` pairs, so
/// removing an EARLIER occurrence of it cannot change which occurrence of any key
/// is last. The signature therefore stays an ORDERED sequence — order still
/// decides the winner when two DIFFERENT sets share a key, which is precisely the
/// `interfaceName` case above.
fn path_uncertainty_ids(
    hops_terminal_to_seed: &[(NodeIx, usize)],
    terminal_node: NodeIx,
    graph: &D1Graph,
    ctx: &DetectorContext,
    cache: &mut PathUncertaintyCache,
) -> Arc<[UncertaintyId]> {
    // Seed→terminal node order = the hops' from_nodes reversed, then the terminal.
    let mut sets: Vec<UncertaintySetId> = Vec::new();
    for (from_node, _edge_k) in hops_terminal_to_seed.iter().rev() {
        let nid = graph.node_ids[*from_node as usize];
        if let Some(&sid) = ctx.uncertainties_by_node.get(nid) {
            sets.push(sid);
        }
    }
    let tid = graph.node_ids[terminal_node as usize];
    if let Some(&sid) = ctx.uncertainties_by_node.get(tid) {
        sets.push(sid);
    }
    scoring_census::add(&scoring_census::UNC_NODES_WITH_SET, sets.len() as u64);

    // Keep only the LAST occurrence of each distinct set (see the doc's
    // equivalence argument), preserving relative order.
    let mut sig: Vec<UncertaintySetId> = Vec::with_capacity(sets.len());
    for (i, &sid) in sets.iter().enumerate() {
        if sets[i + 1..].contains(&sid) {
            continue; // a later occurrence of this same set wins
        }
        sig.push(sid);
    }
    scoring_census::add(&scoring_census::UNC_DISTINCT_SETS_ON_PATH, sig.len() as u64);

    if let Some(hit) = cache.get_path(&sig) {
        scoring_census::add(&scoring_census::UNC_PATH_HITS, 1);
        scoring_census::add(&scoring_census::UNC_RESULT, hit.len() as u64);
        return hit;
    }

    // The elements are ALREADY interned — `ctx.uncertainties.elements(sid)` is the
    // substrate's own id slice, so there is nothing to intern here and no per-set
    // memo to keep. This is what the identity substrate bought: the 2,229,391
    // per-run interns this function used to perform are now zero.
    let mut concat: Vec<UncertaintyId> = Vec::new();
    for &sid in &sig {
        let ids = ctx.uncertainties.elements(sid);
        scoring_census::add(&scoring_census::UNC_ELEMS, ids.len() as u64);
        concat.extend_from_slice(ids);
    }
    let out: Arc<[UncertaintyId]> = ctx.uncertainties.dedupe(&concat).into();
    scoring_census::add(&scoring_census::UNC_RESULT, out.len() as u64);
    cache.put_path(sig, Arc::clone(&out));
    out
}

/// Build ONE representative [`CohortRep`] (bounded witness + path uncertainties)
/// for a lane's running-best winner — the closure `sink_emit` hands to
/// [`TerminalSink::insert`], invoked at most ONCE per `(terminal, ContextKey)`
/// cohort (first-seen). Reads the STILL-ALIVE per-batch `solver`: a `Direct`
/// winner needs no arena (its two-step witness is self-contained); a
/// `Reach`/`Value` winner walks its lane's predecessor chain (bounded per cohort,
/// not per `(loop, terminal)`) for the witness's hop steps + the uncertainty
/// union. `owner`/`op` are the terminal's identity (used only by the fact arms —
/// a `Direct` winner carries its own loop/op); `term_node` is the terminal's
/// graph node, `Some` on the terminal-outer scoring pass (where a fact winner is
/// possible) and `None` on the direct-only pass (whose winners are all `Direct`).
/// `table` is the owning sink's run-level [`UncertaintyTable`], which the
/// uncertainty union is interned into.
#[allow(clippy::too_many_arguments)]
fn build_cohort_rep<'a>(
    b: &BestRef<'a>,
    lane: usize,
    solver: &BatchSolver,
    graph: &D1Graph<'a>,
    ctx: &DetectorContext,
    seeds: &[D1Seed<'a>],
    owner: &'a ModelRoutine,
    op: &'a ModelRecordOperation,
    term_node: Option<NodeIx>,
    cache: &mut PathUncertaintyCache,
) -> CohortRep {
    match b.source {
        BestSource::Direct {
            routine,
            loop_info,
            op: dop,
            ..
        } => {
            scoring_census::add(&scoring_census::REPS_DIRECT, 1);
            CohortRep {
                witness: direct_witness(routine, loop_info, dop, ctx),
                uncertainties: Arc::from(Vec::new()),
            }
        }
        BestSource::Reach { fact_ix } => {
            let tn = term_node.expect("a Reach winner requires the terminal node");
            // Uncertainty union is EMPTY unless the winning path is uncertain
            // (`unc` == OR of node-has-uncertainty along the path; `unc == false`
            // ⇒ no path node contributes an uncertainty ⇒ `path_uncertainty_ids`
            // returns empty). So the O(chain) full-chain walk + union is skipped
            // for every CERTAIN cohort (the majority) — byte-identical, and the
            // dominant 8020 cost (3.2M→34,861 cohorts, but each still walked the
            // full ~28k-hop chain here regardless of `unc`).
            scoring_census::add(&scoring_census::REPS_REACH, 1);
            let uncertainties = if b.rank.2 == 0 {
                let t0 = std::time::Instant::now();
                let (hops, _seed) = collect_reach_chain_b(&solver.reach_pred, lane, fact_ix);
                let out = path_uncertainty_ids(&hops, tn, graph, ctx, cache);
                scoring_census::add(&scoring_census::UNC_WALKS, 1);
                scoring_census::add(&scoring_census::UNC_HOPS, hops.len() as u64);
                scoring_census::add(&scoring_census::UNC_NANOS, t0.elapsed().as_nanos() as u64);
                out
            } else {
                Arc::from(Vec::new())
            };
            let t_w = std::time::Instant::now();
            let witness = representative_witness(
                solver,
                graph,
                ctx,
                seeds,
                lane,
                fact_ix,
                false,
                tn,
                owner,
                op,
                WITNESS_M_LAST,
            );
            scoring_census::add(
                &scoring_census::WITNESS_NANOS,
                t_w.elapsed().as_nanos() as u64,
            );
            CohortRep {
                witness,
                uncertainties,
            }
        }
        BestSource::Value { fact_ix } => {
            let tn = term_node.expect("a Value winner requires the terminal node");
            // See the Reach arm: skip the O(chain) walk + union for certain paths.
            scoring_census::add(&scoring_census::REPS_VALUE, 1);
            let uncertainties = if b.rank.2 == 0 {
                let t0 = std::time::Instant::now();
                let (hops, _seed) =
                    collect_value_chain_b(&solver.value_pred, &solver.reach_pred, lane, fact_ix);
                let out = path_uncertainty_ids(&hops, tn, graph, ctx, cache);
                scoring_census::add(&scoring_census::UNC_WALKS, 1);
                scoring_census::add(&scoring_census::UNC_HOPS, hops.len() as u64);
                scoring_census::add(&scoring_census::UNC_NANOS, t0.elapsed().as_nanos() as u64);
                out
            } else {
                Arc::from(Vec::new())
            };
            let t_w = std::time::Instant::now();
            let witness = representative_witness(
                solver,
                graph,
                ctx,
                seeds,
                lane,
                fact_ix,
                true,
                tn,
                owner,
                op,
                WITNESS_M_LAST,
            );
            scoring_census::add(
                &scoring_census::WITNESS_NANOS,
                t_w.elapsed().as_nanos() as u64,
            );
            CohortRep {
                witness,
                uncertainties,
            }
        }
    }
}

/// Emit one cohort winner per PRESENT lane at one terminal key into `sink`:
/// intern the terminal, then for each lane whose running-best is set, set the
/// loop bit in the winner's [`ContextKey`] cohort + record each reaching verdict
/// for `reachable_verdicts`. Mirrors [`emit_lane_aggregates`]'s
/// present-lane-only iteration EXACTLY (a terminal with zero present lanes is
/// never interned — it produces no aggregate in the old path either). The
/// winner's `unc` bit is recovered from `rank.2` (the `unc == false` preference,
/// `0` iff the winning fact/path is uncertain — identical to the old
/// `!uncertainties.is_empty()`). The representative [`CohortRep`] (bounded
/// witness + confidence-driving uncertainties) is built LAZILY inside
/// [`TerminalSink::insert`] — only for the FIRST loop landing in each cohort —
/// off the still-alive `solver`.
#[allow(clippy::too_many_arguments)]
/// `ALSEM_D1_SCORING_CENSUS=1` — attribution inside the `d1.cohort / scoring`
/// span, which the trace shows as the single largest item in `detector.d1` but
/// cannot break down further (a span measures a region, not which line inside it).
///
/// The split that matters is three-way: the running-best FACT SCAN over every
/// terminal-bearing node, versus [`sink_emit`]'s per-cohort work, versus — inside
/// that — the two chain walks [`build_cohort_rep`] performs (the bounded
/// `representative_witness`, run for EVERY cohort, and the full-chain
/// `path_uncertainty_ids`, run only for UNCERTAIN ones). `docs/OUTSTANDING.md`'s
/// deferred d1 follow-up #1 assumes the uncertainty walk is the residual; this
/// census is what decides whether that is true before anything is rewritten.
///
/// Counters only, plus `Instant` pairs around regions entered at most once per
/// cohort — nothing here allocates, and with the env var unset it is a handful of
/// relaxed atomic loads. Serial by construction: `d1_dataflow` uses no rayon.
pub(crate) mod scoring_census {
    use std::sync::OnceLock;
    use std::sync::atomic::{AtomicU64, Ordering};

    pub(crate) static ENTRIES_SCANNED: AtomicU64 = AtomicU64::new(0);
    pub(crate) static ENTRIES_SKIPPED: AtomicU64 = AtomicU64::new(0);
    pub(crate) static FACTS_SCANNED: AtomicU64 = AtomicU64::new(0);
    pub(crate) static LANE_BITS_FOLDED: AtomicU64 = AtomicU64::new(0);
    pub(crate) static SCORING_NANOS: AtomicU64 = AtomicU64::new(0);
    pub(crate) static EMIT_CALLS: AtomicU64 = AtomicU64::new(0);
    pub(crate) static EMIT_NANOS: AtomicU64 = AtomicU64::new(0);
    pub(crate) static REPS_DIRECT: AtomicU64 = AtomicU64::new(0);
    pub(crate) static REPS_REACH: AtomicU64 = AtomicU64::new(0);
    pub(crate) static REPS_VALUE: AtomicU64 = AtomicU64::new(0);
    pub(crate) static WITNESS_NANOS: AtomicU64 = AtomicU64::new(0);
    pub(crate) static UNC_WALKS: AtomicU64 = AtomicU64::new(0);
    pub(crate) static UNC_HOPS: AtomicU64 = AtomicU64::new(0);
    pub(crate) static UNC_NANOS: AtomicU64 = AtomicU64::new(0);
    /// Elements CONCATENATED into a union before dedupe. This used to be
    /// `unc_elems_interned` and counted `UncertaintyTable::intern` calls; since
    /// the substrate hands d1 an already-interned slice, interning here is ZERO by
    /// construction and only the copy remains. Renamed rather than left reading as
    /// work that no longer happens.
    pub(crate) static UNC_ELEMS: AtomicU64 = AtomicU64::new(0);
    pub(crate) static UNC_RESULT: AtomicU64 = AtomicU64::new(0);
    pub(crate) static UNC_NODES_WITH_SET: AtomicU64 = AtomicU64::new(0);
    pub(crate) static UNC_DISTINCT_SETS_ON_PATH: AtomicU64 = AtomicU64::new(0);
    /// Walks served straight from the path memo — the complement of the walks
    /// that actually intern and dedupe.
    pub(crate) static UNC_PATH_HITS: AtomicU64 = AtomicU64::new(0);

    pub(crate) fn enabled() -> bool {
        static E: OnceLock<bool> = OnceLock::new();
        *E.get_or_init(|| std::env::var("ALSEM_D1_SCORING_CENSUS").as_deref() == Ok("1"))
    }

    pub(crate) fn add(c: &AtomicU64, v: u64) {
        c.fetch_add(v, Ordering::Relaxed);
    }

    fn get(c: &AtomicU64) -> u64 {
        c.load(Ordering::Relaxed)
    }

    /// Print the census. Called once, after the cohort run completes.
    pub(crate) fn report() {
        let scoring = get(&SCORING_NANOS);
        let emit = get(&EMIT_NANOS);
        let witness = get(&WITNESS_NANOS);
        let unc = get(&UNC_NANOS);
        let ms = |n: u64| n as f64 / 1e6;
        let pct = |n: u64| {
            if scoring == 0 {
                0.0
            } else {
                100.0 * n as f64 / scoring as f64
            }
        };
        eprintln!(
            "[d1-scoring-census] scoring={:.1}ms entries_scanned={} entries_skipped={} \
             facts_scanned={} lane_bits_folded={}",
            ms(scoring),
            get(&ENTRIES_SCANNED),
            get(&ENTRIES_SKIPPED),
            get(&FACTS_SCANNED),
            get(&LANE_BITS_FOLDED),
        );
        eprintln!(
            "[d1-scoring-census] emit={:.1}ms ({:.1}%) calls={} | scan_residual={:.1}ms ({:.1}%)",
            ms(emit),
            pct(emit),
            get(&EMIT_CALLS),
            ms(scoring.saturating_sub(emit)),
            pct(scoring.saturating_sub(emit)),
        );
        eprintln!(
            "[d1-scoring-census] reps direct={} reach={} value={} | witness={:.1}ms ({:.1}%) \
             unc_walks={} unc_hops={} unc={:.1}ms ({:.1}%)",
            get(&REPS_DIRECT),
            get(&REPS_REACH),
            get(&REPS_VALUE),
            ms(witness),
            pct(witness),
            get(&UNC_WALKS),
            get(&UNC_HOPS),
            ms(unc),
            pct(unc),
        );
        eprintln!(
            "[d1-scoring-census] unc_elems_concat={} unc_nodes_with_set={} unc_result_ids={}",
            get(&UNC_ELEMS),
            get(&UNC_NODES_WITH_SET),
            get(&UNC_RESULT),
        );
        eprintln!(
            "[d1-scoring-census] unc_distinct_sets_on_path={} unc_path_memo_hits={}",
            get(&UNC_DISTINCT_SETS_ON_PATH),
            get(&UNC_PATH_HITS),
        );
    }
}

/// Everything `sink_emit` needs that is fixed for the whole BATCH — the lane
/// window plus the still-alive per-batch arena the representative witness is
/// materialized from. Grouped so `sink_emit` stays inside clippy's argument
/// bound; every field is batch-invariant, which is exactly why grouping them is
/// safe (nothing here varies per terminal).
struct EmitBatch<'a, 'b> {
    batch_base: usize,
    lanes: usize,
    solver: &'b BatchSolver,
    graph: &'b D1Graph<'a>,
    ctx: &'b DetectorContext<'a>,
    seeds: &'b [D1Seed<'a>],
}

fn sink_emit<'a>(
    sink: &mut TerminalSink<'a>,
    owner: &'a ModelRoutine,
    op: &'a ModelRecordOperation,
    best: &[Option<BestRef<'a>>; BATCH_WIDTH],
    vmask: &[u64; 4],
    term_node: Option<NodeIx>,
    b: &EmitBatch<'a, '_>,
) {
    if !best.iter().take(b.lanes).any(|b| b.is_some()) {
        return;
    }
    let tix = sink.terminal_ix(owner, op);
    for (lane, slot) in best.iter().enumerate().take(b.lanes) {
        let Some(best_ref) = slot else {
            continue;
        };
        let group = (b.batch_base + lane) as GroupIx;
        let ck = ContextKey {
            severity: best_ref.severity,
            verdict: best_ref.verdict,
            depth_bucket: best_ref.depth_bucket,
            unc: best_ref.rank.2 == 0,
        };
        let reachable = [
            (vmask[0] >> lane) & 1 == 1,
            (vmask[1] >> lane) & 1 == 1,
            (vmask[2] >> lane) & 1 == 1,
            (vmask[3] >> lane) & 1 == 1,
        ];
        sink.insert(tix, group, ck, reachable, |cache| {
            build_cohort_rep(
                best_ref, lane, b.solver, b.graph, b.ctx, b.seeds, owner, op, term_node, cache,
            )
        });
    }
}

/// Solve a BATCH of up to [`BATCH_WIDTH`] loop groups (as [`solve_batch`]) but
/// emit the per-(lane, terminal) winners into `sink` as bitmap cohorts instead
/// of materializing `LoopTerminalAgg` witnesses. `batch_base` = `bi *
/// BATCH_WIDTH` (the batch index times the width): group `i` in `batch` is the
/// GLOBAL group index `batch_base + i` — the same dense group id
/// `search_loops`'s sorted `groups` vector assigns, so the sink's loop bits
/// index that vector.
///
/// The fixpoint (seed + SCC drain) and the terminal-outer running-best scan are
/// REPRODUCED verbatim from `solve_batch` — the redesign changes ONLY the
/// emission, so keeping the scan identical is the correctness spine. The
/// `score_batch_to_sink_matches_old` differential decompresses the sink and
/// asserts equality with `solve_batch`'s aggregates.
#[allow(clippy::too_many_arguments)]
pub(crate) fn score_batch_to_sink<'a>(
    graph: &D1Graph<'a>,
    liveness: &Liveness,
    scc: &CallScc,
    seeds: &[D1Seed<'a>],
    direct_ops: &[DirectOp<'a>],
    ctx: &'a DetectorContext,
    cw: &ClosedWorldTempParams,
    plan: &TerminalPlan<'a>,
    batch: &[GroupSpec<'a>],
    batch_base: usize,
    sink: &mut TerminalSink<'a>,
) {
    assert!(
        batch.len() <= BATCH_WIDTH,
        "a batch holds at most {BATCH_WIDTH} lanes"
    );
    let n_nodes = graph.node_ids.len();
    let mut solver = BatchSolver::new(n_nodes);

    let trace_hot = crate::engine::perf_trace::enabled(crate::engine::perf_trace::Detail::Hot);
    let mut pops: u64 = 0;
    let mut max_worklist: usize = 0;
    if trace_hot {
        crate::engine::perf_trace::counter_delta("d1.cohort.batch_seq", 1);
    }

    let unc_by_node: Vec<bool> = graph
        .node_ids
        .iter()
        .map(|id| node_has_uncertainty(ctx, id))
        .collect();
    let expandable: Vec<bool> = graph
        .node_ids
        .iter()
        .map(|id| ctx.routine_by_id.contains_key(id))
        .collect();

    let mut pending: Vec<Vec<Proposal>> = (0..scc.members.len()).map(|_| Vec::new()).collect();

    // Rule 1: seed every lane's frontier (IDENTICAL to `solve_batch`).
    for (lane, group) in batch.iter().enumerate() {
        let bit = 1u64 << lane;
        let root = root_state(group.loop_routine.id.as_str(), cw);
        for &si in &group.seed_indices {
            let seed = &seeds[si];
            let entry = seed.entry;
            let entry_id = graph.node_ids[entry as usize];
            let binding_ok = edge_kind_binding_ok(seed.entry_edge_kind);
            let entry_temp = cross_hop(
                &root,
                seed.loop_routine,
                seed.callsite.id.as_str(),
                entry_id,
                binding_ok,
                cw,
            );
            let depth = seed.seed_depth.min(2);
            let unc = unc_by_node[entry as usize];
            let entry_scc = scc.scc_of[entry as usize] as usize;
            pending[entry_scc].push(Proposal::Reach {
                node: entry,
                depth,
                unc,
                mask: bit,
                hops: 0,
                pred: ReachPredB::Seed {
                    seed_index: si as u32,
                },
            });
            for (slot, &p) in liveness.need[entry as usize].iter().enumerate() {
                let class = lookup(&entry_temp, p);
                pending[entry_scc].push(Proposal::Value {
                    node: entry,
                    slot: slot as u16,
                    class,
                    depth,
                    unc,
                    mask: bit,
                    hops: 0,
                    pred: ValuePredB::Seed {
                        seed_index: si as u32,
                    },
                });
            }
        }
    }

    // Rules 2-3: least-fixpoint over the SCC condensation (IDENTICAL).
    for &scc_id in &scc.topo_order {
        let mut queue = HopQueue::new();
        for prop in std::mem::take(&mut pending[scc_id as usize]) {
            queue.push(prop);
        }
        if trace_hot && queue.len() > max_worklist {
            max_worklist = queue.len();
        }
        while let Some(prop) = queue.pop() {
            if trace_hot {
                pops += 1;
                let wl = queue.len();
                if wl > max_worklist {
                    max_worklist = wl;
                }
            }
            match prop {
                Proposal::Reach {
                    node,
                    depth,
                    unc,
                    mask,
                    hops,
                    pred,
                } => {
                    let (idx, new_bits) = solver.commit_reach(node, depth, unc, mask, hops, pred);
                    if new_bits == 0 || !expandable[node as usize] {
                        continue;
                    }
                    for (k, edge) in graph.edges[node as usize].iter().enumerate() {
                        let m = edge.to;
                        let d2 = (depth + edge.loop_depth).min(2);
                        let u2 = unc || unc_by_node[m as usize];
                        let rnb = solver.reach_new_bits(m, d2, u2, new_bits);
                        if rnb != 0 {
                            route(
                                Proposal::Reach {
                                    node: m,
                                    depth: d2,
                                    unc: u2,
                                    mask: rnb,
                                    hops: hops + 1,
                                    pred: ReachPredB::Hop {
                                        pred: idx as u32,
                                        from_node: node,
                                        edge_k: k as u32,
                                    },
                                },
                                scc_id,
                                &scc.scc_of,
                                &mut queue,
                                &mut pending,
                            );
                        }
                        for (callee_slot, transfer) in
                            liveness.edge_transfers[node as usize][k].iter().enumerate()
                        {
                            if let ParamTransfer::Const(pt) = transfer {
                                let vnb = solver.value_new_bits(
                                    m,
                                    callee_slot as u16,
                                    *pt,
                                    d2,
                                    u2,
                                    new_bits,
                                );
                                if vnb != 0 {
                                    route(
                                        Proposal::Value {
                                            node: m,
                                            slot: callee_slot as u16,
                                            class: *pt,
                                            depth: d2,
                                            unc: u2,
                                            mask: vnb,
                                            hops: hops + 1,
                                            pred: ValuePredB::HopFromReach {
                                                pred: idx as u32,
                                                from_node: node,
                                                edge_k: k as u32,
                                            },
                                        },
                                        scc_id,
                                        &scc.scc_of,
                                        &mut queue,
                                        &mut pending,
                                    );
                                }
                            }
                        }
                    }
                }
                Proposal::Value {
                    node,
                    slot,
                    class,
                    depth,
                    unc,
                    mask,
                    hops,
                    pred,
                } => {
                    let (idx, new_bits) =
                        solver.commit_value(node, slot, class, depth, unc, mask, hops, pred);
                    if new_bits == 0 || !expandable[node as usize] {
                        continue;
                    }
                    for (k, edge) in graph.edges[node as usize].iter().enumerate() {
                        let m = edge.to;
                        let d2 = (depth + edge.loop_depth).min(2);
                        let u2 = unc || unc_by_node[m as usize];
                        for (callee_slot, transfer) in
                            liveness.edge_transfers[node as usize][k].iter().enumerate()
                        {
                            if let ParamTransfer::Copy { caller_slot } = transfer
                                && *caller_slot == slot
                            {
                                let vnb = solver.value_new_bits(
                                    m,
                                    callee_slot as u16,
                                    class,
                                    d2,
                                    u2,
                                    new_bits,
                                );
                                if vnb != 0 {
                                    route(
                                        Proposal::Value {
                                            node: m,
                                            slot: callee_slot as u16,
                                            class,
                                            depth: d2,
                                            unc: u2,
                                            mask: vnb,
                                            hops: hops + 1,
                                            pred: ValuePredB::HopFromValue {
                                                pred: idx as u32,
                                                from_node: node,
                                                edge_k: k as u32,
                                            },
                                        },
                                        scc_id,
                                        &scc.scc_of,
                                        &mut queue,
                                        &mut pending,
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    if trace_hot {
        let reach_facts = solver.reach_facts.len() as u64;
        let value_facts = solver.value_facts.len() as u64;
        let max_worklist = max_worklist as u64;
        let lanes = batch.len() as u64;
        let total_pops = pops;
        crate::engine::perf_trace::instant_lazy("d1.cohort", "batch_summary", || {
            serde_json::json!({
                "lanes": lanes,
                "pops": total_pops,
                "max_worklist": max_worklist,
                "reach_facts": reach_facts,
                "value_facts": value_facts,
            })
        });
    }

    // Rules 4-7: score terminals + select winner PER LANE, then EMIT the winner
    // as a cohort (no witness). The running-best scan is byte-for-byte the same
    // as `solve_batch`'s; only the terminal loop's tail (`sink_emit` vs
    // `emit_lane_aggregates`) differs.
    let _scoring_span = crate::engine::perf_trace::span("d1.cohort", "scoring");
    let t_scoring = std::time::Instant::now();
    let lanes = batch.len();
    let emit_batch = EmitBatch {
        batch_base,
        lanes,
        solver: &solver,
        graph,
        ctx,
        seeds,
    };

    let mut direct_by_key: HashMap<(&'a str, &'a str), Vec<DirectCand<'a>>> = HashMap::new();
    for (lane, group) in batch.iter().enumerate() {
        let root = root_state(group.loop_routine.id.as_str(), cw);
        for &di in &group.direct_indices {
            let d = &direct_ops[di];
            let op = d.op;
            let owner = d.routine;
            let base_pt = resolve_terminal(op, &root, owner.id.as_str(), cw);
            let verdict = flowfield_verdict(base_pt, op, &ctx.table_by_id);
            let local_depth = op.loop_stack.len() as i64;
            let depth_bucket = local_depth.min(2);
            let is_singleton = is_setup_singleton_get(op, Some(owner), &ctx.table_by_id);
            let severity = severity_for(op, verdict, depth_bucket, is_singleton);
            direct_by_key
                .entry((owner.id.as_str(), op.id.as_str()))
                .or_default()
                .push(DirectCand {
                    lane,
                    verdict,
                    severity,
                    depth_bucket,
                    rank: rank5(severity, verdict, false, 0, depth_bucket),
                    routine: owner,
                    loop_info: d.loop_info,
                    op,
                    local_depth,
                });
        }
    }

    let mut consumed_direct_keys: HashSet<(&'a str, &'a str)> = HashSet::new();

    for entry in &plan.entries {
        let node = entry.node;
        if solver.reach_at[node as usize].is_empty() && solver.value_at[node as usize].is_empty() {
            scoring_census::add(&scoring_census::ENTRIES_SKIPPED, 1);
            continue;
        }
        scoring_census::add(&scoring_census::ENTRIES_SCANNED, 1);
        scoring_census::add(
            &scoring_census::FACTS_SCANNED,
            (solver.reach_at[node as usize].len() + solver.value_at[node as usize].len()) as u64,
        );
        let key = (entry.owner.id.as_str(), entry.op.id.as_str());
        let mut best: [Option<BestRef<'a>>; BATCH_WIDTH] = [None; BATCH_WIDTH];
        let mut vmask = [0u64; 4];

        if let Some(cands) = direct_by_key.get(&key) {
            for c in cands {
                fold_direct(c, &mut best, &mut vmask);
            }
            consumed_direct_keys.insert(key);
        }

        match &entry.read {
            ReadPlan::Reach {
                verdict,
                sev_by_bucket,
            } => {
                let verdict = *verdict;
                for &ri in &solver.reach_at[node as usize] {
                    let f = &solver.reach_facts[ri];
                    let db = (f.depth + entry.local_depth).min(2);
                    let severity = sev_by_bucket[db as usize];
                    vmask[verdict as usize] |= f.mask;
                    let sev_r = sev_rank(severity);
                    let q = verdict.quality();
                    let unc_pref = if f.unc { 0 } else { 1 };
                    let hops = &solver.reach_hops[ri];
                    let mut m = f.mask;
                    while m != 0 {
                        let lane = m.trailing_zeros() as usize;
                        m &= m - 1;
                        scoring_census::add(&scoring_census::LANE_BITS_FOLDED, 1);
                        let rank = (sev_r, q, unc_pref, -(hops[lane] as i64), db);
                        if best[lane].is_none_or(|b| rank > b.rank) {
                            best[lane] = Some(BestRef {
                                rank,
                                verdict,
                                severity,
                                depth_bucket: db,
                                source: BestSource::Reach { fact_ix: ri },
                            });
                        }
                    }
                }
            }
            ReadPlan::Value {
                slot,
                verdict_by_class,
                sev_by_class_bucket,
            } => {
                let slot = *slot;
                for &vi in &solver.value_at[node as usize] {
                    let f = &solver.value_facts[vi];
                    if f.slot != slot {
                        continue;
                    }
                    let ci = f.class as usize;
                    let verdict = verdict_by_class[ci];
                    let db = (f.depth + entry.local_depth).min(2);
                    let severity = sev_by_class_bucket[ci][db as usize];
                    vmask[verdict as usize] |= f.mask;
                    let sev_r = sev_rank(severity);
                    let q = verdict.quality();
                    let unc_pref = if f.unc { 0 } else { 1 };
                    let hops = &solver.value_hops[vi];
                    let mut m = f.mask;
                    while m != 0 {
                        let lane = m.trailing_zeros() as usize;
                        m &= m - 1;
                        scoring_census::add(&scoring_census::LANE_BITS_FOLDED, 1);
                        let rank = (sev_r, q, unc_pref, -(hops[lane] as i64), db);
                        if best[lane].is_none_or(|b| rank > b.rank) {
                            best[lane] = Some(BestRef {
                                rank,
                                verdict,
                                severity,
                                depth_bucket: db,
                                source: BestSource::Value { fact_ix: vi },
                            });
                        }
                    }
                }
            }
        }

        let t_emit = std::time::Instant::now();
        sink_emit(
            sink,
            entry.owner,
            entry.op,
            &best,
            &vmask,
            Some(entry.node),
            &emit_batch,
        );
        scoring_census::add(&scoring_census::EMIT_CALLS, 1);
        scoring_census::add(
            &scoring_census::EMIT_NANOS,
            t_emit.elapsed().as_nanos() as u64,
        );
    }

    for (_key, cands) in &direct_by_key {
        if consumed_direct_keys.contains(_key) {
            continue;
        }
        let mut best: [Option<BestRef<'a>>; BATCH_WIDTH] = [None; BATCH_WIDTH];
        let mut vmask = [0u64; 4];
        for c in cands {
            fold_direct(c, &mut best, &mut vmask);
        }
        // A direct-only key's winner is always `Direct` (no facts), which carries
        // its own witness inputs; `owner`/`op` come from any cand (all share the
        // key) and `term_node` is `None` (never read on the Direct arm).
        let any = &cands[0];
        let t_emit = std::time::Instant::now();
        sink_emit(sink, any.routine, any.op, &best, &vmask, None, &emit_batch);
        scoring_census::add(&scoring_census::EMIT_CALLS, 1);
        scoring_census::add(
            &scoring_census::EMIT_NANOS,
            t_emit.elapsed().as_nanos() as u64,
        );
    }
    scoring_census::add(
        &scoring_census::SCORING_NANOS,
        t_scoring.elapsed().as_nanos() as u64,
    );
}

/// Test-only: drive the SAME fixpoint (Rules 1-3 — seed every lane's frontier,
/// then drain each SCC in topological order) that `solve_batch`/
/// `score_batch_to_sink` run internally, and return the populated
/// [`BatchSolver`] itself instead of a scored/emitted result. Task C2's
/// origin-propagation test needs to inspect the solver's per-lane provenance
/// (`reach_pred`/`reach_origin`/`value_pred`/`value_origin`) directly, which
/// neither production entry point exposes. Byte-for-byte the same commit/route
/// sequence as those two (minus their Hot-tier tracing, irrelevant here) — see
/// `score_batch_to_sink`'s own doc for why the fixpoint section is safe to
/// reproduce verbatim.
///
/// `pub(crate)`: Task C3's `d1_witness` test module (a sibling of this one)
/// reuses it the same way to populate a `BatchSolver` for
/// `representative_witness` fixtures, without re-deriving the fixpoint.
#[cfg(test)]
#[allow(clippy::too_many_arguments)]
pub(crate) fn run_batch_fixpoint_for_test<'a>(
    graph: &D1Graph<'a>,
    liveness: &Liveness,
    scc: &CallScc,
    seeds: &[D1Seed<'a>],
    ctx: &'a DetectorContext,
    cw: &ClosedWorldTempParams,
    batch: &[GroupSpec<'a>],
) -> BatchSolver {
    let n_nodes = graph.node_ids.len();
    let mut solver = BatchSolver::new(n_nodes);

    let unc_by_node: Vec<bool> = graph
        .node_ids
        .iter()
        .map(|id| node_has_uncertainty(ctx, id))
        .collect();
    let expandable: Vec<bool> = graph
        .node_ids
        .iter()
        .map(|id| ctx.routine_by_id.contains_key(id))
        .collect();

    let mut pending: Vec<Vec<Proposal>> = (0..scc.members.len()).map(|_| Vec::new()).collect();

    // Rule 1: seed every lane's frontier (identical to solve_batch/score_batch_to_sink).
    for (lane, group) in batch.iter().enumerate() {
        let bit = 1u64 << lane;
        let root = root_state(group.loop_routine.id.as_str(), cw);
        for &si in &group.seed_indices {
            let seed = &seeds[si];
            let entry = seed.entry;
            let entry_id = graph.node_ids[entry as usize];
            let binding_ok = edge_kind_binding_ok(seed.entry_edge_kind);
            let entry_temp = cross_hop(
                &root,
                seed.loop_routine,
                seed.callsite.id.as_str(),
                entry_id,
                binding_ok,
                cw,
            );
            let depth = seed.seed_depth.min(2);
            let unc = unc_by_node[entry as usize];
            let entry_scc = scc.scc_of[entry as usize] as usize;
            pending[entry_scc].push(Proposal::Reach {
                node: entry,
                depth,
                unc,
                mask: bit,
                hops: 0,
                pred: ReachPredB::Seed {
                    seed_index: si as u32,
                },
            });
            for (slot, &p) in liveness.need[entry as usize].iter().enumerate() {
                let class = lookup(&entry_temp, p);
                pending[entry_scc].push(Proposal::Value {
                    node: entry,
                    slot: slot as u16,
                    class,
                    depth,
                    unc,
                    mask: bit,
                    hops: 0,
                    pred: ValuePredB::Seed {
                        seed_index: si as u32,
                    },
                });
            }
        }
    }

    // Rules 2-3: least-fixpoint over the SCC condensation.
    for &scc_id in &scc.topo_order {
        let mut queue = HopQueue::new();
        for prop in std::mem::take(&mut pending[scc_id as usize]) {
            queue.push(prop);
        }
        while let Some(prop) = queue.pop() {
            match prop {
                Proposal::Reach {
                    node,
                    depth,
                    unc,
                    mask,
                    hops,
                    pred,
                } => {
                    let (idx, new_bits) = solver.commit_reach(node, depth, unc, mask, hops, pred);
                    if new_bits == 0 || !expandable[node as usize] {
                        continue;
                    }
                    for (k, edge) in graph.edges[node as usize].iter().enumerate() {
                        let m = edge.to;
                        let d2 = (depth + edge.loop_depth).min(2);
                        let u2 = unc || unc_by_node[m as usize];
                        let rnb = solver.reach_new_bits(m, d2, u2, new_bits);
                        if rnb != 0 {
                            route(
                                Proposal::Reach {
                                    node: m,
                                    depth: d2,
                                    unc: u2,
                                    mask: rnb,
                                    hops: hops + 1,
                                    pred: ReachPredB::Hop {
                                        pred: idx as u32,
                                        from_node: node,
                                        edge_k: k as u32,
                                    },
                                },
                                scc_id,
                                &scc.scc_of,
                                &mut queue,
                                &mut pending,
                            );
                        }
                        for (callee_slot, transfer) in
                            liveness.edge_transfers[node as usize][k].iter().enumerate()
                        {
                            if let ParamTransfer::Const(pt) = transfer {
                                let vnb = solver.value_new_bits(
                                    m,
                                    callee_slot as u16,
                                    *pt,
                                    d2,
                                    u2,
                                    new_bits,
                                );
                                if vnb != 0 {
                                    route(
                                        Proposal::Value {
                                            node: m,
                                            slot: callee_slot as u16,
                                            class: *pt,
                                            depth: d2,
                                            unc: u2,
                                            mask: vnb,
                                            hops: hops + 1,
                                            pred: ValuePredB::HopFromReach {
                                                pred: idx as u32,
                                                from_node: node,
                                                edge_k: k as u32,
                                            },
                                        },
                                        scc_id,
                                        &scc.scc_of,
                                        &mut queue,
                                        &mut pending,
                                    );
                                }
                            }
                        }
                    }
                }
                Proposal::Value {
                    node,
                    slot,
                    class,
                    depth,
                    unc,
                    mask,
                    hops,
                    pred,
                } => {
                    let (idx, new_bits) =
                        solver.commit_value(node, slot, class, depth, unc, mask, hops, pred);
                    if new_bits == 0 || !expandable[node as usize] {
                        continue;
                    }
                    for (k, edge) in graph.edges[node as usize].iter().enumerate() {
                        let m = edge.to;
                        let d2 = (depth + edge.loop_depth).min(2);
                        let u2 = unc || unc_by_node[m as usize];
                        for (callee_slot, transfer) in
                            liveness.edge_transfers[node as usize][k].iter().enumerate()
                        {
                            if let ParamTransfer::Copy { caller_slot } = transfer
                                && *caller_slot == slot
                            {
                                let vnb = solver.value_new_bits(
                                    m,
                                    callee_slot as u16,
                                    class,
                                    d2,
                                    u2,
                                    new_bits,
                                );
                                if vnb != 0 {
                                    route(
                                        Proposal::Value {
                                            node: m,
                                            slot: callee_slot as u16,
                                            class,
                                            depth: d2,
                                            unc: u2,
                                            mask: vnb,
                                            hops: hops + 1,
                                            pred: ValuePredB::HopFromValue {
                                                pred: idx as u32,
                                                from_node: node,
                                                edge_k: k as u32,
                                            },
                                        },
                                        scc_id,
                                        &scc.scc_of,
                                        &mut queue,
                                        &mut pending,
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    solver
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::l4::combined_graph::CombinedEdge;
    use crate::engine::l5::d1_graph::build_d1_graph;
    use crate::engine::l5::d1_liveness::compute_liveness;
    use crate::engine::l5::d1_reach::process_group;
    use crate::engine::l5::full_summary::FullRoutineSummary;
    use crate::engine::l5::test_support::{
        call_site, coverage, edge_kind, fact, loop_def, minimal_ctx, record_op, routine, summary,
    };
    use crate::program::model::workspace::ModelEntities;

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

    fn ws(routines: &[ModelRoutine]) -> ModelEntities {
        ModelEntities {
            objects: vec![],
            tables: vec![],
            routines: routines.to_vec(),
        }
    }

    /// Group `seeds`/`direct_ops` EXACTLY as `search_loops` does, run BOTH
    /// `process_group` (oracle) and `solve_group` per group, and assert the six
    /// load-bearing components agree per (loop, terminal-op). Also assert every
    /// `solve_group` aggregate carries a structurally-valid realizing witness.
    /// Panics if the fixture yields zero aggregates (non-vacuity).
    #[allow(clippy::type_complexity)]
    fn assert_agrees(
        graph: &D1Graph,
        seeds: &[D1Seed],
        direct_ops: &[DirectOp],
        ctx: &DetectorContext,
        cw: &ClosedWorldTempParams,
    ) {
        let liveness = compute_liveness(graph, ctx, cw);

        struct Group<'a> {
            loop_routine: &'a ModelRoutine,
            loop_info: &'a PLoop,
            seed_indices: Vec<usize>,
            direct_indices: Vec<usize>,
        }
        let mut groups: BTreeMap<(&str, &str), Group> = BTreeMap::new();
        for (i, seed) in seeds.iter().enumerate() {
            groups
                .entry((seed.loop_routine.id.as_str(), seed.loop_id))
                .or_insert_with(|| Group {
                    loop_routine: seed.loop_routine,
                    loop_info: seed.loop_info,
                    seed_indices: Vec::new(),
                    direct_indices: Vec::new(),
                })
                .seed_indices
                .push(i);
        }
        for (i, d) in direct_ops.iter().enumerate() {
            groups
                .entry((d.routine.id.as_str(), d.loop_id))
                .or_insert_with(|| Group {
                    loop_routine: d.routine,
                    loop_info: d.loop_info,
                    seed_indices: Vec::new(),
                    direct_indices: Vec::new(),
                })
                .direct_indices
                .push(i);
        }

        let mut total = 0usize;
        for (key, group) in &groups {
            let loop_id = key.1;
            let oracle = process_group(
                graph,
                seeds,
                direct_ops,
                ctx,
                cw,
                group.loop_routine,
                loop_id,
                group.loop_info,
                &group.seed_indices,
                &group.direct_indices,
            );
            let solved = solve_group(
                graph,
                &liveness,
                seeds,
                direct_ops,
                ctx,
                cw,
                group.loop_routine,
                loop_id,
                group.loop_info,
                &group.seed_indices,
                &group.direct_indices,
            );

            // Component 1 (coverage): the (owner, op) key sets are identical.
            let ok: std::collections::BTreeSet<(&str, &str)> = oracle
                .iter()
                .map(|a| (a.terminal.owner.id.as_str(), a.terminal.op.id.as_str()))
                .collect();
            let sk: std::collections::BTreeSet<(&str, &str)> = solved
                .iter()
                .map(|a| (a.terminal.owner.id.as_str(), a.terminal.op.id.as_str()))
                .collect();
            assert_eq!(
                ok, sk,
                "coverage (component 1) diverged for group {key:?}: oracle {ok:?} vs solved {sk:?}"
            );

            let oracle_by: HashMap<(&str, &str), &LoopTerminalAgg> = oracle
                .iter()
                .map(|a| ((a.terminal.owner.id.as_str(), a.terminal.op.id.as_str()), a))
                .collect();
            for s in &solved {
                let k = (s.terminal.owner.id.as_str(), s.terminal.op.id.as_str());
                let o = oracle_by[&k];
                assert_eq!(
                    s.reachable_verdicts, o.reachable_verdicts,
                    "reachable_verdicts (2) diverged for {key:?}/{k:?}"
                );
                assert_eq!(
                    s.severity, o.severity,
                    "severity (3) diverged for {key:?}/{k:?}"
                );
                assert_eq!(
                    s.verdict, o.verdict,
                    "verdict (4) diverged for {key:?}/{k:?}"
                );
                assert_eq!(
                    s.depth_bucket, o.depth_bucket,
                    "depth_bucket (5) diverged for {key:?}/{k:?}"
                );
                let s_unc = winner_unc(s);
                let o_unc = winner_unc(o);
                assert_eq!(s_unc, o_unc, "unc (6) diverged for {key:?}/{k:?}");

                assert_witness_valid(s, ctx);
            }
            total += solved.len();
        }
        assert!(total > 0, "fixture must produce at least one aggregate");
    }

    /// The winner's `unc` is not stored directly on `LoopTerminalAgg`; recover it
    /// from the materialized uncertainty union (empty iff the winning path
    /// crossed no uncertain node — the same bit `unc` tracks). Both oracle and
    /// solved use the same rule, so comparing the derived bit is faithful.
    ///
    /// This is EXACT for component 6, not a mere proxy: both `process_group` and
    /// `solve_group` derive `unc` as the OR of `node_has_uncertainty` over the
    /// winning path's nodes, and the materialized union is non-empty under
    /// exactly that same condition — so `!uncertainties.is_empty()` recovers the
    /// winner's `unc` bit identically on both sides. (The uncertainty VECTOR
    /// itself may still differ — that is component 7.)
    fn winner_unc(agg: &LoopTerminalAgg) -> bool {
        !agg.uncertainties.is_empty()
    }

    /// A `solve_group` aggregate's witness must be a valid realizing path: first
    /// step in the loop routine (a loop step), last step the terminal op, the
    /// intermediate steps a `[call, hop*]` chain, and — the brief's "hop count ==
    /// reported" + edge-contiguity requirements — every consecutive `(from, to)`
    /// pair of intermediate/terminal steps a REAL graph edge (`from`'s callsite
    /// resolving `from -> to` in `ctx.graph.edges_by_from`). Because the walk
    /// crosses exactly one real edge per step and lands precisely on the terminal
    /// owner, the witness's hop count is the true path length — no gap, no repeat,
    /// no phantom hop (the "reported" count).
    fn assert_witness_valid(agg: &LoopTerminalAgg, ctx: &DetectorContext) {
        let w = &agg.witness;
        assert!(!w.is_empty(), "witness must be non-empty");
        // First step: the loop step, in the loop routine, naming the loop.
        assert_eq!(
            w[0].routine_id, agg.loop_routine.id,
            "witness first step must be in the loop routine"
        );
        assert_eq!(
            w[0].loop_id.as_deref(),
            Some(agg.loop_info.id.as_str()),
            "witness first step must be the loop step"
        );
        // Last step: the terminal op.
        let last = w.last().unwrap();
        assert_eq!(
            last.operation_id.as_deref(),
            Some(agg.terminal.op.id.as_str()),
            "witness last step must be the terminal op"
        );
        assert_eq!(
            last.routine_id, agg.terminal.owner.id,
            "witness last step must be owned by the terminal routine"
        );
        assert!(
            last.callsite_id.is_none(),
            "the terminal step carries no callsite"
        );
        if w.len() == 2 {
            // Direct op: [loop_step, terminal_step], no entry callsite.
            assert_eq!(
                agg.entry_callsite_id, None,
                "a direct-op winner has no entry callsite"
            );
        } else {
            // Transitive: [loop, call, hop*, terminal]. The call + every hop
            // step carries a callsite; the entry callsite is recorded.
            assert!(w.len() >= 3, "a transitive witness has loop+call+terminal");
            assert!(
                agg.entry_callsite_id.is_some(),
                "a transitive winner records its entry callsite"
            );
            // Edge contiguity + hop count: each `(from, to)` in the [call, hop*,
            // terminal] tail is a real edge `from --from.callsite--> to`. `from`
            // ranges over the call + hop steps (all carry a callsite, none a loop
            // id); `to` ranges over the hops + terminal step.
            for pair in w[1..].windows(2) {
                let from = &pair[0];
                let to = &pair[1];
                assert!(
                    from.callsite_id.is_some(),
                    "the call + hop steps each carry a callsite"
                );
                assert!(from.loop_id.is_none(), "non-loop intermediate steps");
                let cs = from.callsite_id.as_deref();
                let is_real_edge =
                    ctx.graph
                        .edges_by_from
                        .get(&from.routine_id)
                        .is_some_and(|edges| {
                            edges
                                .iter()
                                .any(|e| e.callsite_id.as_deref() == cs && e.to == to.routine_id)
                        });
                assert!(
                    is_real_edge,
                    "witness step {} --{:?}--> {} must be a real graph edge (contiguity)",
                    from.routine_id, cs, to.routine_id
                );
            }
        }
    }

    // === Fixture 1: budget-buster fanout ================================
    #[test]
    fn agrees_on_budget_buster_fanout() {
        let mut r = routine("R", "procedure");
        r.loops = vec![loop_def("R/loop0")];
        r.call_sites = vec![call_site("R/cs0", "A", vec!["R/loop0".to_string()])];
        let a = routine("A", "procedure");

        let mut routines = vec![r, a];
        let mut a_edges: Vec<CombinedEdge> = Vec::new();
        let mut summaries: HashMap<String, FullRoutineSummary> = HashMap::new();
        summaries.insert("A".to_string(), db_summary("A", "t/A"));
        for i in 0..600 {
            let did = format!("D{i}");
            routines.push(routine(&did, "procedure"));
            a_edges.push(edge_kind("A", &did, &format!("A/cs{i}"), "direct"));
            summaries.insert(did.clone(), db_summary(&did, &format!("t/{did}")));
        }
        let mut t = routine("T", "procedure");
        t.record_operations = vec![record_op(
            "T/op0",
            "Modify",
            "Rec",
            Some("t/T"),
            vec![],
            false,
        )];
        routines.push(t);
        a_edges.push(edge_kind("A", "T", "A/csT", "direct"));
        summaries.insert("T".to_string(), db_summary("T", "t/T"));

        let mut graph_edges: HashMap<String, Vec<CombinedEdge>> = HashMap::new();
        graph_edges.insert(
            "R".to_string(),
            vec![edge_kind("R", "A", "R/cs0", "direct")],
        );
        graph_edges.insert("A".to_string(), a_edges);

        let ctx = minimal_ctx(&routines, graph_edges, summaries);
        let workspace = ws(&routines);
        let mut memo = HashMap::new();
        let (graph, seeds) = build_d1_graph(&ctx, &workspace, &mut memo);
        assert!(graph.node_ids.len() > 500);
        let cw = ClosedWorldTempParams::new();
        assert_agrees(&graph, &seeds, &[], &ctx, &cw);
    }

    // === Fixture 2: depth-2 route beats a shorter depth-1 route ==========
    #[test]
    fn agrees_on_depth2_beats_depth1() {
        let mut r = routine("R", "procedure");
        r.loops = vec![loop_def("R/loop0")];
        r.call_sites = vec![call_site("R/cs0", "A", vec!["R/loop0".to_string()])];
        let mut a = routine("A", "procedure");
        a.call_sites = vec![
            call_site("A/csT", "T", vec![]),
            call_site("A/csX", "X", vec!["A/loop0".to_string()]),
        ];
        let x = routine("X", "procedure");
        let y = routine("Y", "procedure");
        let mut t = routine("T", "procedure");
        t.record_operations = vec![record_op(
            "T/op0",
            "FindSet",
            "Rec",
            Some("t/T"),
            vec![],
            false,
        )];
        let routines = vec![r, a, x, y, t];

        let mut graph_edges: HashMap<String, Vec<CombinedEdge>> = HashMap::new();
        graph_edges.insert(
            "R".to_string(),
            vec![edge_kind("R", "A", "R/cs0", "direct")],
        );
        graph_edges.insert(
            "A".to_string(),
            vec![
                edge_kind("A", "T", "A/csT", "direct"),
                edge_kind("A", "X", "A/csX", "direct"),
            ],
        );
        graph_edges.insert(
            "X".to_string(),
            vec![edge_kind("X", "Y", "X/csY", "direct")],
        );
        graph_edges.insert(
            "Y".to_string(),
            vec![edge_kind("Y", "T", "Y/csT", "direct")],
        );

        let summaries: HashMap<String, FullRoutineSummary> = ["A", "X", "Y", "T"]
            .iter()
            .map(|id| (id.to_string(), db_summary(id, &format!("t/{id}"))))
            .collect();

        let ctx = minimal_ctx(&routines, graph_edges, summaries);
        let workspace = ws(&routines);
        let mut memo = HashMap::new();
        let (graph, seeds) = build_d1_graph(&ctx, &workspace, &mut memo);
        let cw = ClosedWorldTempParams::new();
        assert_agrees(&graph, &seeds, &[], &ctx, &cw);
    }

    // === Fixture 2b: a winner whose realizing path crosses an UNCERTAIN
    // node — the only fixture that drives component 6 (`unc`) to `true`, so the
    // fact solver's uncertainty propagation is genuinely differentiated (all the
    // other fixtures leave `uncertainties_by_node` empty → `unc` trivially
    // false). The deep A->X->Y->T route wins on severity; node X carries an
    // uncertainty, so BOTH engines must report the winner `unc == true`.
    #[test]
    fn agrees_on_uncertain_winner() {
        use crate::engine::l4::summary::Uncertainty;

        let mut r = routine("R", "procedure");
        r.loops = vec![loop_def("R/loop0")];
        r.call_sites = vec![call_site("R/cs0", "A", vec!["R/loop0".to_string()])];
        let mut a = routine("A", "procedure");
        a.call_sites = vec![
            call_site("A/csT", "T", vec![]),
            call_site("A/csX", "X", vec!["A/loop0".to_string()]),
        ];
        let x = routine("X", "procedure");
        let y = routine("Y", "procedure");
        let mut t = routine("T", "procedure");
        t.record_operations = vec![record_op(
            "T/op0",
            "FindSet",
            "Rec",
            Some("t/T"),
            vec![],
            false,
        )];
        let routines = vec![r, a, x, y, t];

        let mut graph_edges: HashMap<String, Vec<CombinedEdge>> = HashMap::new();
        graph_edges.insert(
            "R".to_string(),
            vec![edge_kind("R", "A", "R/cs0", "direct")],
        );
        graph_edges.insert(
            "A".to_string(),
            vec![
                edge_kind("A", "T", "A/csT", "direct"),
                edge_kind("A", "X", "A/csX", "direct"),
            ],
        );
        graph_edges.insert(
            "X".to_string(),
            vec![edge_kind("X", "Y", "X/csY", "direct")],
        );
        graph_edges.insert(
            "Y".to_string(),
            vec![edge_kind("Y", "T", "Y/csT", "direct")],
        );

        let summaries: HashMap<String, FullRoutineSummary> = ["A", "X", "Y", "T"]
            .iter()
            .map(|id| (id.to_string(), db_summary(id, &format!("t/{id}"))))
            .collect();

        let mut ctx = minimal_ctx(&routines, graph_edges, summaries);
        // Inject an uncertainty on node X — on the winning deep route only.
        ctx.set_uncertainties_for_test(
            "X",
            vec![Uncertainty {
                kind: "dynamic-dispatch".to_string(),
                callsite_id: None,
                operation_id: None,
                routine_id: Some("X".to_string()),
                interface_name: None,
            }],
        );

        let workspace = ws(&routines);
        let mut memo = HashMap::new();
        let (graph, seeds) = build_d1_graph(&ctx, &workspace, &mut memo);
        let cw = ClosedWorldTempParams::new();

        // Cross-check that this fixture actually reaches `unc == true` on the
        // winner (guards against a vacuous always-false component 6).
        let liveness = compute_liveness(&graph, &ctx, &cw);
        let seed_indices: Vec<usize> = (0..seeds.len()).collect();
        let solved = solve_group(
            &graph,
            &liveness,
            &seeds,
            &[],
            &ctx,
            &cw,
            seeds[0].loop_routine,
            seeds[0].loop_id,
            seeds[0].loop_info,
            &seed_indices,
            &[],
        );
        assert_eq!(solved.len(), 1);
        assert_eq!(solved[0].severity, "high", "the deep route wins");
        assert!(
            !solved[0].uncertainties.is_empty(),
            "the winning path crosses the uncertain node X — unc must be true"
        );

        assert_agrees(&graph, &seeds, &[], &ctx, &cw);
    }

    // === Fixture 3: physical route beats temp route (multi-seed) =========
    fn temp_vs_physical_fixture() -> Fixture {
        use crate::engine::l5::test_support::{arg_binding, ts_known, ts_pd};

        let mut r = routine("R", "procedure");
        r.loops = vec![loop_def("R/loop0")];
        let mut cs0 = call_site("R/cs0", "H", vec!["R/loop0".to_string()]);
        cs0.argument_bindings = vec![arg_binding(0, Some(ts_known(true)))];
        let mut cs1 = call_site("R/cs1", "H", vec!["R/loop0".to_string()]);
        cs1.argument_bindings = vec![arg_binding(0, Some(ts_known(false)))];
        r.call_sites = vec![cs0, cs1];

        let mut h = routine("H", "procedure");
        let mut op0 = record_op("H/op0", "Modify", "Rec", Some("t/H"), vec![], false);
        op0.temp_state = Some(ts_pd(0));
        h.record_operations = vec![op0];

        let routines = vec![r, h];
        let mut graph_edges: HashMap<String, Vec<CombinedEdge>> = HashMap::new();
        graph_edges.insert(
            "R".to_string(),
            vec![
                edge_kind("R", "H", "R/cs0", "direct"),
                edge_kind("R", "H", "R/cs1", "direct"),
            ],
        );
        let summaries: HashMap<String, FullRoutineSummary> =
            [("H".to_string(), db_summary("H", "t/H"))]
                .into_iter()
                .collect();
        (routines, graph_edges, summaries)
    }

    #[test]
    fn agrees_on_physical_beats_temp_multi_seed() {
        let (routines, graph_edges, summaries) = temp_vs_physical_fixture();
        let ctx = minimal_ctx(&routines, graph_edges, summaries);
        let workspace = ws(&routines);
        let mut memo = HashMap::new();
        let (graph, seeds) = build_d1_graph(&ctx, &workspace, &mut memo);
        assert_eq!(seeds.len(), 2);
        let cw = ClosedWorldTempParams::new();
        assert_agrees(&graph, &seeds, &[], &ctx, &cw);
    }

    // === Fixture 3b: the FlowFieldGated-vs-Physical DISCOVERY TIE =========
    // The one shape where a pure first-discovery tie decides component 4
    // (verdict): a `CalcFields` PD-terminal reachable by two in-loop seeds — one
    // passing a TEMP record (-> `FlowFieldGated`, since the FlowField gate BLOCKS
    // the info-downgrade — here via a missing `table_id`, `flowfield_gate_blocks_
    // downgrade` d1.rs:143-148), one PHYSICAL (-> `Physical`). Both share
    // verdict-quality rank 3 AND the SAME severity ("high", `CalcFields` is a
    // heavy-read op) at equal unc(false)/hops(0), so the winner is decided by
    // `-discovery` ALONE. `reachable_verdicts` (a SET) holds BOTH; only the single
    // WINNER verdict is at stake — a binding component-4 constraint. This asserts
    // `solve_group` and `process_group` pick the SAME winner verdict.
    fn flowfield_tie_fixture() -> Fixture {
        use crate::engine::l5::test_support::{arg_binding, ts_known, ts_pd};

        let mut r = routine("R", "procedure");
        r.loops = vec![loop_def("R/loop0")];
        let mut cs0 = call_site("R/cs0", "H", vec!["R/loop0".to_string()]);
        cs0.argument_bindings = vec![arg_binding(0, Some(ts_known(true)))]; // temp
        let mut cs1 = call_site("R/cs1", "H", vec!["R/loop0".to_string()]);
        cs1.argument_bindings = vec![arg_binding(0, Some(ts_known(false)))]; // physical
        r.call_sites = vec![cs0, cs1];

        let mut h = routine("H", "procedure");
        // CalcFields with NO table_id -> the FlowField gate blocks the temp
        // downgrade (unresolvable table is conservative), so a Temp-resolved read
        // is `FlowFieldGated`, not `Temporary`.
        let mut op0 = record_op("H/op0", "CalcFields", "Rec", None, vec![], false);
        op0.temp_state = Some(ts_pd(0)); // PD on param 0 — resolves per caller
        h.record_operations = vec![op0];

        let routines = vec![r, h];
        let mut graph_edges: HashMap<String, Vec<CombinedEdge>> = HashMap::new();
        graph_edges.insert(
            "R".to_string(),
            vec![
                edge_kind("R", "H", "R/cs0", "direct"),
                edge_kind("R", "H", "R/cs1", "direct"),
            ],
        );
        let summaries: HashMap<String, FullRoutineSummary> =
            [("H".to_string(), db_summary("H", "t/H"))]
                .into_iter()
                .collect();
        (routines, graph_edges, summaries)
    }

    #[test]
    fn agrees_on_flowfield_vs_physical_discovery_tie() {
        use crate::engine::l5::detectors::d1::TempVerdict;

        let (routines, graph_edges, summaries) = flowfield_tie_fixture();
        let ctx = minimal_ctx(&routines, graph_edges, summaries);
        let workspace = ws(&routines);
        let mut memo = HashMap::new();
        let (graph, seeds) = build_d1_graph(&ctx, &workspace, &mut memo);
        assert_eq!(seeds.len(), 2, "one temp seed + one physical seed into H");
        let cw = ClosedWorldTempParams::new();

        // Confirm the tie is real: BOTH verdicts reach the terminal at the same
        // severity, and the winner is genuinely FlowFieldGated (not Temporary,
        // not Physical) — else the fixture would not exercise the tie path.
        let liveness = compute_liveness(&graph, &ctx, &cw);
        let seed_indices: Vec<usize> = (0..seeds.len()).collect();
        let solved = solve_group(
            &graph,
            &liveness,
            &seeds,
            &[],
            &ctx,
            &cw,
            seeds[0].loop_routine,
            seeds[0].loop_id,
            seeds[0].loop_info,
            &seed_indices,
            &[],
        );
        assert_eq!(solved.len(), 1);
        assert_eq!(solved[0].severity, "high", "CalcFields heavy-read => high");
        assert_eq!(
            solved[0].reachable_verdicts,
            vec![TempVerdict::Physical, TempVerdict::FlowFieldGated],
            "both verdicts reach the terminal (the SET holds both)"
        );

        // The differential decides the tie: solve_group and process_group must
        // agree on the single WINNER verdict (component 4) despite the tie.
        assert_agrees(&graph, &seeds, &[], &ctx, &cw);
    }

    // === Fixture 4: cycle ================================================
    #[test]
    fn agrees_on_cycle() {
        let mut r = routine("R", "procedure");
        r.loops = vec![loop_def("R/loop0")];
        r.call_sites = vec![call_site("R/cs0", "A", vec!["R/loop0".to_string()])];
        let a = routine("A", "procedure");
        let mut b = routine("B", "procedure");
        b.record_operations = vec![record_op(
            "B/op0",
            "Modify",
            "Rec",
            Some("t/B"),
            vec![],
            false,
        )];
        let routines = vec![r, a, b];

        let mut graph_edges: HashMap<String, Vec<CombinedEdge>> = HashMap::new();
        graph_edges.insert(
            "R".to_string(),
            vec![edge_kind("R", "A", "R/cs0", "direct")],
        );
        graph_edges.insert(
            "A".to_string(),
            vec![edge_kind("A", "B", "A/cs0", "direct")],
        );
        graph_edges.insert(
            "B".to_string(),
            vec![edge_kind("B", "A", "B/cs0", "direct")],
        );

        let summaries: HashMap<String, FullRoutineSummary> = [
            ("A".to_string(), db_summary("A", "t/A")),
            ("B".to_string(), db_summary("B", "t/B")),
        ]
        .into_iter()
        .collect();

        let ctx = minimal_ctx(&routines, graph_edges, summaries);
        let workspace = ws(&routines);
        let mut memo = HashMap::new();
        let (graph, seeds) = build_d1_graph(&ctx, &workspace, &mut memo);
        let cw = ClosedWorldTempParams::new();
        assert_agrees(&graph, &seeds, &[], &ctx, &cw);
    }

    // === Fixture 5: direct + transitive adjudication =====================
    #[test]
    fn agrees_on_direct_and_transitive() {
        let mut r = routine("R", "procedure");
        r.loops = vec![loop_def("R/loop0")];
        r.call_sites = vec![call_site("R/cs0", "A", vec!["R/loop0".to_string()])];
        r.record_operations = vec![record_op(
            "R/T",
            "FindSet",
            "Rec",
            Some("t/R"),
            vec!["R/loop0".to_string()],
            false,
        )];
        let mut a = routine("A", "procedure");
        a.call_sites = vec![call_site("A/csR", "R", vec!["A/loop0".to_string()])];
        let routines = vec![r, a];

        let mut graph_edges: HashMap<String, Vec<CombinedEdge>> = HashMap::new();
        graph_edges.insert(
            "R".to_string(),
            vec![edge_kind("R", "A", "R/cs0", "direct")],
        );
        graph_edges.insert(
            "A".to_string(),
            vec![edge_kind("A", "R", "A/csR", "direct")],
        );

        let summaries: HashMap<String, FullRoutineSummary> = [
            ("A".to_string(), db_summary("A", "t/A")),
            ("R".to_string(), db_summary("R", "t/R")),
        ]
        .into_iter()
        .collect();

        let ctx = minimal_ctx(&routines, graph_edges, summaries);
        let workspace = ws(&routines);
        let mut memo = HashMap::new();
        let (graph, seeds) = build_d1_graph(&ctx, &workspace, &mut memo);
        let direct_ops = vec![DirectOp {
            routine: &routines[0],
            loop_id: routines[0].loops[0].id.as_str(),
            loop_info: &routines[0].loops[0],
            op: &routines[0].record_operations[0],
        }];
        let cw = ClosedWorldTempParams::new();
        assert_agrees(&graph, &seeds, &direct_ops, &ctx, &cw);
    }

    // === Fixture 6: multi-group (overlapping closure + a direct group) ===
    fn multi_group_fixture() -> Fixture {
        let mut r1 = routine("R1", "procedure");
        r1.loops = vec![loop_def("R1/loop0")];
        r1.call_sites = vec![call_site("R1/cs0", "A1", vec!["R1/loop0".to_string()])];
        let mut a1 = routine("A1", "procedure");
        a1.call_sites = vec![call_site("A1/csT", "T", vec![])];

        let mut r2 = routine("R2", "procedure");
        r2.loops = vec![loop_def("R2/loop0")];
        r2.call_sites = vec![call_site("R2/cs0", "A2", vec!["R2/loop0".to_string()])];
        let mut a2 = routine("A2", "procedure");
        a2.call_sites = vec![
            call_site("A2/csT", "T", vec![]),
            call_site("A2/csX", "X2", vec!["A2/loop0".to_string()]),
        ];
        let mut x2 = routine("X2", "procedure");
        x2.call_sites = vec![call_site("X2/csY", "Y2", vec![])];
        let mut y2 = routine("Y2", "procedure");
        y2.call_sites = vec![call_site("Y2/csT", "T", vec![])];

        let mut r3 = routine("R3", "procedure");
        r3.loops = vec![loop_def("R3/loop0")];
        r3.call_sites = vec![call_site("R3/cs0", "A3", vec!["R3/loop0".to_string()])];
        let mut a3 = routine("A3", "procedure");
        a3.call_sites = vec![call_site("A3/csT", "T", vec![])];

        let mut r4 = routine("R4", "procedure");
        r4.loops = vec![loop_def("R4/loop0")];
        r4.record_operations = vec![record_op(
            "R4/opD",
            "Modify",
            "Rec",
            Some("t/R4"),
            vec!["R4/loop0".to_string()],
            false,
        )];

        let mut t = routine("T", "procedure");
        t.record_operations = vec![record_op(
            "T/op0",
            "FindSet",
            "Rec",
            Some("t/T"),
            vec![],
            false,
        )];

        let routines = vec![r1, a1, r2, a2, x2, y2, r3, a3, r4, t];

        let mut graph_edges: HashMap<String, Vec<CombinedEdge>> = HashMap::new();
        graph_edges.insert(
            "R1".to_string(),
            vec![edge_kind("R1", "A1", "R1/cs0", "direct")],
        );
        graph_edges.insert(
            "A1".to_string(),
            vec![edge_kind("A1", "T", "A1/csT", "direct")],
        );
        graph_edges.insert(
            "R2".to_string(),
            vec![edge_kind("R2", "A2", "R2/cs0", "direct")],
        );
        graph_edges.insert(
            "A2".to_string(),
            vec![
                edge_kind("A2", "T", "A2/csT", "direct"),
                edge_kind("A2", "X2", "A2/csX", "direct"),
            ],
        );
        graph_edges.insert(
            "X2".to_string(),
            vec![edge_kind("X2", "Y2", "X2/csY", "direct")],
        );
        graph_edges.insert(
            "Y2".to_string(),
            vec![edge_kind("Y2", "T", "Y2/csT", "direct")],
        );
        graph_edges.insert(
            "R3".to_string(),
            vec![edge_kind("R3", "A3", "R3/cs0", "direct")],
        );
        graph_edges.insert(
            "A3".to_string(),
            vec![edge_kind("A3", "T", "A3/csT", "direct")],
        );

        let summaries: HashMap<String, FullRoutineSummary> = [
            ("A1", "t/A1"),
            ("A2", "t/A2"),
            ("X2", "t/X2"),
            ("Y2", "t/Y2"),
            ("A3", "t/A3"),
            ("T", "t/T"),
        ]
        .into_iter()
        .map(|(id, table)| (id.to_string(), db_summary(id, table)))
        .collect();

        (routines, graph_edges, summaries)
    }

    #[test]
    fn agrees_on_multi_group() {
        let (routines, graph_edges, summaries) = multi_group_fixture();
        let ctx = minimal_ctx(&routines, graph_edges, summaries);
        let workspace = ws(&routines);
        let mut memo = HashMap::new();
        let (graph, seeds) = build_d1_graph(&ctx, &workspace, &mut memo);
        assert_eq!(seeds.len(), 3);
        let r4_idx = routines.iter().position(|r| r.id == "R4").unwrap();
        let direct_ops = vec![DirectOp {
            routine: &routines[r4_idx],
            loop_id: routines[r4_idx].loops[0].id.as_str(),
            loop_info: &routines[r4_idx].loops[0],
            op: &routines[r4_idx].record_operations[0],
        }];
        let cw = ClosedWorldTempParams::new();
        assert_agrees(&graph, &seeds, &direct_ops, &ctx, &cw);
    }

    // === Task D3: condensation determinism + real-SCC identification ======
    #[test]
    fn condensation_deterministic() {
        use crate::engine::l5::d1_graph::D1Edge;
        // A<->B is a real 2-member cycle; C->D is a chain of two singletons.
        let node_ids = vec!["A", "B", "C", "D"];
        let mut node_ix = HashMap::new();
        for (i, id) in node_ids.iter().enumerate() {
            node_ix.insert(*id, i as u32);
        }
        let mk = |to: u32| D1Edge {
            to,
            kind: "direct",
            callsite_id: None,
            loop_depth: 0,
            binding_ok: true,
        };
        let graph = D1Graph {
            node_ids: node_ids.clone(),
            node_ix,
            edges: vec![
                vec![mk(1)], // A -> B
                vec![mk(0)], // B -> A (closes the cycle)
                vec![mk(3)], // C -> D
                vec![],      // D (leaf)
            ],
            terminals: vec![vec![], vec![], vec![], vec![]],
        };

        let s1 = condense(&graph);
        let s2 = condense(&graph);
        assert_eq!(s1.scc_of, s2.scc_of, "scc ids identical across runs");
        assert_eq!(
            s1.topo_order, s2.topo_order,
            "topo order identical across runs"
        );

        let a = graph.node_ix["A"];
        let b = graph.node_ix["B"];
        let c = graph.node_ix["C"];
        let d = graph.node_ix["D"];
        // The cycle is one SCC whose members are exactly {A, B}.
        assert_eq!(
            s1.scc_of[a as usize], s1.scc_of[b as usize],
            "A and B belong to the same (cyclic) SCC"
        );
        assert_eq!(
            s1.members[s1.scc_of[a as usize] as usize],
            vec![a.min(b), a.max(b)],
            "the cyclic SCC's members are exactly {{A, B}}"
        );
        // C and D are distinct singleton SCCs.
        assert_ne!(
            s1.scc_of[c as usize], s1.scc_of[d as usize],
            "C and D are separate singleton SCCs"
        );
        assert_eq!(s1.members[s1.scc_of[c as usize] as usize], vec![c]);
        assert_eq!(s1.members[s1.scc_of[d as usize] as usize], vec![d]);
        // Topological order: the caller C precedes its callee D.
        assert!(
            s1.scc_of[c as usize] < s1.scc_of[d as usize],
            "C (caller) precedes D (callee) in topological SCC order"
        );
        assert_eq!(
            s1.topo_order,
            (0..s1.members.len() as u32).collect::<Vec<_>>(),
            "topo_order is the identity permutation over topologically-numbered SCCs"
        );
    }

    // === Task D3: solve_batch == solve_group per lane, across >1 batch ====
    // A fixture with > BATCH_WIDTH independent loop groups (forcing >=2 batches),
    // all overlapping on a SHARED recursive SCC (the C<->D cycle) and a shared
    // terminal T. For every group, `solve_batch`'s per-lane aggregates must equal
    // `solve_group`'s on components 1-6, with a structurally-valid witness.
    #[allow(clippy::type_complexity)]
    fn many_groups_fixture(n: usize) -> Fixture {
        // Shared recursive SCC: C <-> D, with the terminal op on D.
        let c = routine("C", "procedure");
        let mut c_only = c;
        c_only.call_sites = vec![call_site("C/csD", "D", vec![])];
        let mut d = routine("D", "procedure");
        d.call_sites = vec![call_site("D/csC", "C", vec![])];
        d.record_operations = vec![record_op(
            "D/op0",
            "Modify",
            "Rec",
            Some("t/D"),
            vec![],
            false,
        )];
        // Shared plain terminal.
        let mut t = routine("T", "procedure");
        t.record_operations = vec![record_op(
            "T/op0",
            "FindSet",
            "Rec",
            Some("t/T"),
            vec![],
            false,
        )];

        let mut routines: Vec<ModelRoutine> = Vec::new();
        let mut graph_edges: HashMap<String, Vec<CombinedEdge>> = HashMap::new();
        let mut summaries: HashMap<String, FullRoutineSummary> = HashMap::new();

        graph_edges.insert(
            "C".to_string(),
            vec![edge_kind("C", "D", "C/csD", "direct")],
        );
        graph_edges.insert(
            "D".to_string(),
            vec![edge_kind("D", "C", "D/csC", "direct")],
        );
        summaries.insert("C".to_string(), db_summary("C", "t/C"));
        summaries.insert("D".to_string(), db_summary("D", "t/D"));
        summaries.insert("T".to_string(), db_summary("T", "t/T"));

        for i in 0..n {
            let rid = format!("R{i}");
            let aid = format!("A{i}");
            let mut r = routine(&rid, "procedure");
            r.loops = vec![loop_def(&format!("{rid}/loop0"))];
            r.call_sites = vec![call_site(
                &format!("{rid}/cs0"),
                &aid,
                vec![format!("{rid}/loop0")],
            )];
            let a = routine(&aid, "procedure");
            graph_edges.insert(
                rid.clone(),
                vec![edge_kind(&rid, &aid, &format!("{rid}/cs0"), "direct")],
            );
            graph_edges.insert(
                aid.clone(),
                vec![
                    edge_kind(&aid, "T", &format!("{aid}/csT"), "direct"),
                    edge_kind(&aid, "C", &format!("{aid}/csC"), "direct"),
                ],
            );
            summaries.insert(aid.clone(), db_summary(&aid, &format!("t/{aid}")));
            routines.push(r);
            routines.push(a);
        }
        routines.push(c_only);
        routines.push(d);
        routines.push(t);
        (routines, graph_edges, summaries)
    }

    /// Group `seeds`/`direct_ops` EXACTLY as `search_loops` does, chunk the sorted
    /// groups into `BATCH_WIDTH` lanes, and assert every lane of every batch's
    /// `solve_batch` output equals `solve_group`'s for that group on the six
    /// load-bearing components + a valid witness. Panics unless it exercises >1
    /// batch (the chunking boundary) and produces aggregates.
    fn assert_batch_agrees(
        graph: &D1Graph,
        seeds: &[D1Seed],
        direct_ops: &[DirectOp],
        ctx: &DetectorContext,
        cw: &ClosedWorldTempParams,
    ) {
        let liveness = compute_liveness(graph, ctx, cw);
        let scc = condense(graph);
        let plan = build_terminal_plan(graph, &liveness, ctx, cw);

        let mut groups: BTreeMap<(&str, &str), GroupSpec> = BTreeMap::new();
        for (i, seed) in seeds.iter().enumerate() {
            groups
                .entry((seed.loop_routine.id.as_str(), seed.loop_id))
                .or_insert_with(|| GroupSpec {
                    loop_routine: seed.loop_routine,
                    loop_id: seed.loop_id,
                    loop_info: seed.loop_info,
                    seed_indices: Vec::new(),
                    direct_indices: Vec::new(),
                })
                .seed_indices
                .push(i);
        }
        for (i, d) in direct_ops.iter().enumerate() {
            groups
                .entry((d.routine.id.as_str(), d.loop_id))
                .or_insert_with(|| GroupSpec {
                    loop_routine: d.routine,
                    loop_id: d.loop_id,
                    loop_info: d.loop_info,
                    seed_indices: Vec::new(),
                    direct_indices: Vec::new(),
                })
                .direct_indices
                .push(i);
        }
        let group_vec: Vec<GroupSpec> = groups.into_values().collect();
        assert!(
            group_vec.len() > BATCH_WIDTH,
            "fixture must exceed one batch ({} groups) to exercise chunking",
            group_vec.len()
        );

        let mut n_batches = 0usize;
        let mut total = 0usize;
        for chunk in group_vec.chunks(BATCH_WIDTH) {
            n_batches += 1;
            let batch_out = solve_batch(
                graph, &liveness, &scc, seeds, direct_ops, ctx, cw, &plan, chunk,
            );
            for group in chunk {
                let solo = solve_group(
                    graph,
                    &liveness,
                    seeds,
                    direct_ops,
                    ctx,
                    cw,
                    group.loop_routine,
                    group.loop_id,
                    group.loop_info,
                    &group.seed_indices,
                    &group.direct_indices,
                );
                let batch_g: Vec<&LoopTerminalAgg> = batch_out
                    .iter()
                    .filter(|a| {
                        a.loop_routine.id == group.loop_routine.id && a.loop_id == group.loop_id
                    })
                    .collect();

                let gk = (group.loop_routine.id.as_str(), group.loop_id);
                let solo_keys: std::collections::BTreeSet<(&str, &str)> = solo
                    .iter()
                    .map(|a| (a.terminal.owner.id.as_str(), a.terminal.op.id.as_str()))
                    .collect();
                let batch_keys: std::collections::BTreeSet<(&str, &str)> = batch_g
                    .iter()
                    .map(|a| (a.terminal.owner.id.as_str(), a.terminal.op.id.as_str()))
                    .collect();
                assert_eq!(
                    solo_keys, batch_keys,
                    "coverage (1) diverged for group {gk:?}"
                );

                let solo_by: HashMap<(&str, &str), &LoopTerminalAgg> = solo
                    .iter()
                    .map(|a| ((a.terminal.owner.id.as_str(), a.terminal.op.id.as_str()), a))
                    .collect();
                for b in &batch_g {
                    let k = (b.terminal.owner.id.as_str(), b.terminal.op.id.as_str());
                    let s = solo_by[&k];
                    assert_eq!(
                        b.reachable_verdicts, s.reachable_verdicts,
                        "reachable_verdicts (2) diverged for {gk:?}/{k:?}"
                    );
                    assert_eq!(
                        b.severity, s.severity,
                        "severity (3) diverged for {gk:?}/{k:?}"
                    );
                    assert_eq!(
                        b.verdict, s.verdict,
                        "verdict (4) diverged for {gk:?}/{k:?}"
                    );
                    assert_eq!(
                        b.depth_bucket, s.depth_bucket,
                        "depth_bucket (5) diverged for {gk:?}/{k:?}"
                    );
                    assert_eq!(
                        winner_unc(b),
                        winner_unc(s),
                        "unc (6) diverged for {gk:?}/{k:?}"
                    );
                    assert_witness_valid(b, ctx);
                }
                total += batch_g.len();
            }
        }
        assert!(n_batches >= 2, "the fixture must span more than one batch");
        assert!(total > 0, "fixture must produce at least one aggregate");
    }

    #[test]
    fn batch_equals_per_group() {
        // 80 independent loop groups -> two batches (64 + 16), all overlapping on
        // the shared recursive C<->D SCC and the shared terminal T.
        let (routines, graph_edges, summaries) = many_groups_fixture(80);
        let ctx = minimal_ctx(&routines, graph_edges, summaries);
        let workspace = ws(&routines);
        let mut memo = HashMap::new();
        let (graph, seeds) = build_d1_graph(&ctx, &workspace, &mut memo);
        assert_eq!(seeds.len(), 80, "one in-loop seed per group");
        let cw = ClosedWorldTempParams::new();
        assert_batch_agrees(&graph, &seeds, &[], &ctx, &cw);
    }

    // === Task D3 fix: the depth_bucket straddle (saturating severity) ======
    // A `LockTable` terminal — base severity "low", which `severity_for` does
    // NOT bump on depth>=2 (it SATURATES: only high/medium promote) — reached by
    // two EQUAL-hop paths whose summed loop_depth straddles the nested-loop
    // threshold: R->A->B->T carries edge loop_depth 1 (bucket 2), R->A->C->T
    // carries 0 (bucket 1). All of (severity, verdict, unc, hops) tie, so ONLY
    // `depth_bucket` distinguishes the two candidates — and the canonical
    // `selection_rank` must pick the HIGHER (nested-loop) bucket in
    // `process_group`, `solve_group` AND `solve_batch` deterministically, closing
    // the discovery-order divergence in the reported `depth_class`.
    fn straddle_fixture() -> Fixture {
        let mut r = routine("R", "procedure");
        r.loops = vec![loop_def("R/loop0")];
        r.call_sites = vec![call_site("R/cs0", "A", vec!["R/loop0".to_string()])];
        // A has NO loops (it never seeds); its callsites' loop_stacks give the
        // A->B edge loop_depth 1 and the A->C edge loop_depth 0 (via
        // call_site_by_id) — the Task-1 non-zero edge-loop_depth pattern.
        let mut a = routine("A", "procedure");
        a.call_sites = vec![
            call_site("A/csB", "B", vec!["A/loop0".to_string()]),
            call_site("A/csC", "C", vec![]),
        ];
        let b = routine("B", "procedure");
        let c = routine("C", "procedure");
        let mut t = routine("T", "procedure");
        t.record_operations = vec![record_op(
            "T/op0",
            "LockTable",
            "Rec",
            Some("t/T"),
            vec![],
            false,
        )];
        let routines = vec![r, a, b, c, t];

        let mut graph_edges: HashMap<String, Vec<CombinedEdge>> = HashMap::new();
        graph_edges.insert(
            "R".to_string(),
            vec![edge_kind("R", "A", "R/cs0", "direct")],
        );
        graph_edges.insert(
            "A".to_string(),
            vec![
                edge_kind("A", "B", "A/csB", "direct"),
                edge_kind("A", "C", "A/csC", "direct"),
            ],
        );
        graph_edges.insert(
            "B".to_string(),
            vec![edge_kind("B", "T", "B/csT", "direct")],
        );
        graph_edges.insert(
            "C".to_string(),
            vec![edge_kind("C", "T", "C/csT", "direct")],
        );
        let summaries: HashMap<String, FullRoutineSummary> = ["A", "B", "C", "T"]
            .iter()
            .map(|id| (id.to_string(), db_summary(id, &format!("t/{id}"))))
            .collect();
        (routines, graph_edges, summaries)
    }

    #[test]
    fn depth_bucket_straddle_prefers_nested_loop() {
        let (routines, graph_edges, summaries) = straddle_fixture();
        let ctx = minimal_ctx(&routines, graph_edges, summaries);
        let workspace = ws(&routines);
        let mut memo = HashMap::new();
        let (graph, seeds) = build_d1_graph(&ctx, &workspace, &mut memo);
        assert_eq!(seeds.len(), 1, "only R/cs0 -> A seeds");
        let cw = ClosedWorldTempParams::new();
        let liveness = compute_liveness(&graph, &ctx, &cw);
        let scc = condense(&graph);
        let plan = build_terminal_plan(&graph, &liveness, &ctx, &cw);
        let seed_indices: Vec<usize> = (0..seeds.len()).collect();

        let oracle = process_group(
            &graph,
            &seeds,
            &[],
            &ctx,
            &cw,
            seeds[0].loop_routine,
            seeds[0].loop_id,
            seeds[0].loop_info,
            &seed_indices,
            &[],
        );
        let solo = solve_group(
            &graph,
            &liveness,
            &seeds,
            &[],
            &ctx,
            &cw,
            seeds[0].loop_routine,
            seeds[0].loop_id,
            seeds[0].loop_info,
            &seed_indices,
            &[],
        );
        let group = GroupSpec {
            loop_routine: seeds[0].loop_routine,
            loop_id: seeds[0].loop_id,
            loop_info: seeds[0].loop_info,
            seed_indices: seed_indices.clone(),
            direct_indices: Vec::new(),
        };
        let batch = solve_batch(
            &graph,
            &liveness,
            &scc,
            &seeds,
            &[],
            &ctx,
            &cw,
            &plan,
            std::slice::from_ref(&group),
        );

        for (name, aggs) in [
            ("process_group", &oracle),
            ("solve_group", &solo),
            ("solve_batch", &batch),
        ] {
            assert_eq!(aggs.len(), 1, "{name}: one (loop, LockTable) aggregate");
            let agg = &aggs[0];
            assert_eq!(
                agg.terminal.op.id, "T/op0",
                "{name}: the LockTable terminal"
            );
            assert_eq!(
                agg.severity, "low",
                "{name}: LockTable base severity, saturated (no depth>=2 bump)"
            );
            assert_eq!(
                agg.depth_bucket, 2,
                "{name}: the HIGHER (nested-loop) bucket wins the straddle, deterministically"
            );
        }

        // The two dataflow engines agree with the oracle on all six components
        // (now including the canonical, discovery-independent depth_bucket).
        assert_agrees(&graph, &seeds, &[], &ctx, &cw);
    }

    // === Task C2: origin_seed propagation ===================================
    // A -> B -> C -> H (3 hops from the seed entry A to the terminal H) —
    // deliberately deeper than 2 hops. B's callsite to C binds C's param 0 to a
    // KNOWN temp LITERAL (`ts_known`, a Const transfer): this is where H's value
    // chain switches onto the REACH chain (`ValuePredB::HopFromReach`), since a
    // Const value has no caller-value parent. C's callsite to H then forwards
    // C's OWN param 0 (`ts_pd(0)`, a Copy transfer) to H, which reads it directly
    // (`ts_pd(0)` on H's own op) — so H ends up with BOTH a plain 3-hop REACH
    // fact and a VALUE fact whose predecessor chain crosses a HopFromReach
    // transition partway through. One fixture, both coverage requirements.
    fn deep_value_chain_fixture() -> Fixture {
        use crate::engine::l5::test_support::{arg_binding, ts_known, ts_pd};

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
        let mut c_cs = call_site("C/csH", "H", vec![]);
        c_cs.argument_bindings = vec![arg_binding(0, Some(ts_pd(0)))];
        c.call_sites = vec![c_cs];

        let mut h = routine("H", "procedure");
        let mut op0 = record_op("H/op0", "Modify", "Rec", Some("t/H"), vec![], false);
        op0.temp_state = Some(ts_pd(0));
        h.record_operations = vec![op0];

        let routines = vec![r, a, b, c, h];

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
            vec![edge_kind("C", "H", "C/csH", "direct")],
        );
        let summaries: HashMap<String, FullRoutineSummary> = ["A", "B", "C", "H"]
            .iter()
            .map(|id| (id.to_string(), db_summary(id, &format!("t/{id}"))))
            .collect();
        (routines, graph_edges, summaries)
    }

    /// Run the fixture's single lane through [`run_batch_fixpoint_for_test`] and
    /// assert, for EVERY reach fact and EVERY value fact the fixpoint populated
    /// (not just H's), that the incrementally-propagated `reach_origin`/
    /// `value_origin` equals the seed index [`collect_reach_chain_b`]/
    /// [`collect_value_chain_b`] finds by walking the full predecessor chain —
    /// the equivalence Task C2 exists to make cheap. Separately confirms the
    /// fixture actually exercises both required shapes: H's reach fact is >2
    /// hops from its seed, and H's value fact's chain crosses a `HopFromReach`
    /// transition (a value fact born from a reach arrival, not a prior value).
    #[test]
    fn origin_propagation_matches_chain_walk() {
        let (routines, graph_edges, summaries) = deep_value_chain_fixture();
        let ctx = minimal_ctx(&routines, graph_edges, summaries);
        let workspace = ws(&routines);
        let mut memo = HashMap::new();
        let (graph, seeds) = build_d1_graph(&ctx, &workspace, &mut memo);
        assert_eq!(seeds.len(), 1, "only R/cs0 -> A seeds");
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
        let lane = 0usize;

        // --- Deep reach chain (>2 hops) at the terminal node H -------------
        let h_node = graph.node_ix["H"];
        let h_reach_facts = &solver.reach_at[h_node as usize];
        assert_eq!(h_reach_facts.len(), 1, "one (depth, unc) reach fact at H");
        let h_reach_ix = h_reach_facts[0];
        let (h_reach_hops, h_reach_seed) =
            collect_reach_chain_b(&solver.reach_pred, lane, h_reach_ix);
        assert!(
            h_reach_hops.len() > 2,
            "H's reach chain must be deeper than 2 hops (A->B->C->H); got {}",
            h_reach_hops.len()
        );
        assert_eq!(
            solver.reach_origin[h_reach_ix][lane], h_reach_seed as u32,
            "incremental reach_origin at the deep terminal must equal the chain-walk seed"
        );

        // --- Value chain crossing a HopFromReach transition at H -----------
        let h_value_facts = &solver.value_at[h_node as usize];
        assert_eq!(h_value_facts.len(), 1, "one value fact at H (slot 0)");
        let h_value_ix = h_value_facts[0];
        let crosses_hop_from_reach = matches!(
            solver.value_pred[h_value_ix][lane],
            ValuePredB::HopFromValue { pred, .. }
                if matches!(solver.value_pred[pred as usize][lane], ValuePredB::HopFromReach { .. })
        );
        assert!(
            crosses_hop_from_reach,
            "H's value chain must cross a HopFromReach transition (born at B's Const edge to C)"
        );
        let (h_value_hops, h_value_seed) =
            collect_value_chain_b(&solver.value_pred, &solver.reach_pred, lane, h_value_ix);
        assert!(
            !h_value_hops.is_empty(),
            "the value chain must record at least one hop"
        );
        assert_eq!(
            solver.value_origin[h_value_ix][lane], h_value_seed as u32,
            "incremental value_origin across a HopFromReach transition must equal the chain-walk seed"
        );

        // --- Non-vacuous, comprehensive: every reach fact + every value fact -
        let mut reach_checked = 0usize;
        for (ix, fact) in solver.reach_facts.iter().enumerate() {
            let mut m = fact.mask;
            while m != 0 {
                let l = m.trailing_zeros() as usize;
                m &= m - 1;
                let (_, seed) = collect_reach_chain_b(&solver.reach_pred, l, ix);
                assert_eq!(
                    solver.reach_origin[ix][l], seed as u32,
                    "reach fact {ix} lane {l}: incremental origin != chain-walk origin"
                );
                reach_checked += 1;
            }
        }
        assert!(reach_checked > 0, "fixture must populate reach facts");

        let mut value_checked = 0usize;
        for (ix, fact) in solver.value_facts.iter().enumerate() {
            let mut m = fact.mask;
            while m != 0 {
                let l = m.trailing_zeros() as usize;
                m &= m - 1;
                let (_, seed) =
                    collect_value_chain_b(&solver.value_pred, &solver.reach_pred, l, ix);
                assert_eq!(
                    solver.value_origin[ix][l], seed as u32,
                    "value fact {ix} lane {l}: incremental origin != chain-walk origin"
                );
                value_checked += 1;
            }
        }
        assert!(value_checked > 0, "fixture must populate value facts");
    }

    // === Task C1: the terminal bitmap-COHORT differential (the spine) ======
    // For every fixture, run BOTH the old `solve_batch`/`emit_lane_aggregates`
    // path (Vec<LoopTerminalAgg>) AND the new `score_batch_to_sink` cohort path
    // over the SAME chunked batches. DECOMPRESS the sink — for each (loop=group,
    // terminal) recover its ctx (verdict, depth_bucket, unc) + reachable_verdicts
    // — and assert it EQUALS the old aggregate's on coverage + verdict +
    // depth_bucket + unc + reachable_verdicts. Witness is NOT compared
    // (representative, bounded). Non-vacuous (asserts > 0 pairs).

    /// The decompressed / old per-(loop, terminal) tuple compared by the
    /// differential: (winner verdict, depth_bucket, unc, reachable_verdicts).
    type CohortRow = (TempVerdict, i64, bool, Vec<TempVerdict>);

    fn assert_sink_matches_old(
        graph: &D1Graph,
        seeds: &[D1Seed],
        direct_ops: &[DirectOp],
        ctx: &DetectorContext,
        cw: &ClosedWorldTempParams,
    ) {
        use crate::engine::l5::d1_cohort::{
            TerminalSink, emit_finalize_census, emit_liveness_census, reachable_verdicts_of,
        };

        let liveness = compute_liveness(graph, ctx, cw);
        // Exercise the Hot-tier census helpers (no-ops with tracing off, but this
        // keeps them compiled + called on the C1 path).
        emit_liveness_census(&liveness, graph.node_ids.len());
        let scc = condense(graph);
        let plan = build_terminal_plan(graph, &liveness, ctx, cw);

        // Group EXACTLY as `search_loops` does (sorted BTreeMap order == the
        // global lane/group assignment).
        let mut groups: BTreeMap<(&str, &str), GroupSpec> = BTreeMap::new();
        for (i, seed) in seeds.iter().enumerate() {
            groups
                .entry((seed.loop_routine.id.as_str(), seed.loop_id))
                .or_insert_with(|| GroupSpec {
                    loop_routine: seed.loop_routine,
                    loop_id: seed.loop_id,
                    loop_info: seed.loop_info,
                    seed_indices: Vec::new(),
                    direct_indices: Vec::new(),
                })
                .seed_indices
                .push(i);
        }
        for (i, d) in direct_ops.iter().enumerate() {
            groups
                .entry((d.routine.id.as_str(), d.loop_id))
                .or_insert_with(|| GroupSpec {
                    loop_routine: d.routine,
                    loop_id: d.loop_id,
                    loop_info: d.loop_info,
                    seed_indices: Vec::new(),
                    direct_indices: Vec::new(),
                })
                .direct_indices
                .push(i);
        }
        let group_vec: Vec<GroupSpec> = groups.into_values().collect();

        // (loop_routine_id, loop_id) -> the GLOBAL group index (its position in
        // the sorted group vector == `batch_base + lane`).
        let mut group_ix: HashMap<(String, String), u32> = HashMap::new();
        for (i, g) in group_vec.iter().enumerate() {
            group_ix.insert((g.loop_routine.id.clone(), g.loop_id.to_string()), i as u32);
        }

        // OLD path: aggregate via `solve_batch` per chunk.
        let mut old_aggs: Vec<LoopTerminalAgg> = Vec::new();
        for chunk in group_vec.chunks(BATCH_WIDTH) {
            old_aggs.extend(solve_batch(
                graph, &liveness, &scc, seeds, direct_ops, ctx, cw, &plan, chunk,
            ));
        }

        // NEW path: emit into the cohort sink per chunk (batch_base = bi * WIDTH).
        let mut sink = TerminalSink::new(plan.entries.len(), group_vec.len());
        for (bi, chunk) in group_vec.chunks(BATCH_WIDTH).enumerate() {
            score_batch_to_sink(
                graph,
                &liveness,
                &scc,
                seeds,
                direct_ops,
                ctx,
                cw,
                &plan,
                chunk,
                bi * BATCH_WIDTH,
                &mut sink,
            );
        }
        let cohorts = sink.finalize();
        emit_finalize_census(&cohorts, &ctx.uncertainties);

        // DECOMPRESS the sink: (group, owner_id, op_id) -> the cohort tuple.
        let mut new_map: HashMap<(u32, String, String), CohortRow> = HashMap::new();
        for tc in &cohorts {
            let (owner, op) = (tc.key.0.id.as_str(), tc.key.1.id.as_str());
            for (ck, bm, _rep) in &tc.cohorts {
                for g in bm.iter() {
                    let reachable = reachable_verdicts_of(&tc.verdict_sets, g);
                    let prev = new_map.insert(
                        (g, owner.to_string(), op.to_string()),
                        (ck.verdict, ck.depth_bucket, ck.unc, reachable),
                    );
                    assert!(
                        prev.is_none(),
                        "sink decompress: loop {g} appears twice at terminal {owner}/{op} \
                         (disjointness — a loop must land in ONE ctx per terminal)"
                    );
                }
            }
        }

        // OLD tuples: one per (loop, terminal). `unc` is recovered the same way
        // the existing solve_group/solve_batch differentials do (component 6 ==
        // the winner path crossed an uncertain node == non-empty uncertainty
        // union).
        let mut old_map: HashMap<(u32, String, String), CohortRow> = HashMap::new();
        for a in &old_aggs {
            let g = *group_ix
                .get(&(a.loop_routine.id.clone(), a.loop_id.to_string()))
                .expect("every aggregate's loop is a known group");
            let owner = a.terminal.owner.id.clone();
            let op = a.terminal.op.id.clone();
            let unc = !a.uncertainties.is_empty();
            let prev = old_map.insert(
                (g, owner, op),
                (a.verdict, a.depth_bucket, unc, a.reachable_verdicts.clone()),
            );
            assert!(
                prev.is_none(),
                "old aggregates hold exactly one per (loop, terminal)"
            );
        }

        assert!(
            !old_map.is_empty(),
            "differential must be non-vacuous (> 0 (loop, terminal) pairs)"
        );

        // Coverage: the (loop, terminal) key sets are identical.
        let old_keys: std::collections::BTreeSet<_> = old_map.keys().cloned().collect();
        let new_keys: std::collections::BTreeSet<_> = new_map.keys().cloned().collect();
        assert_eq!(
            old_keys, new_keys,
            "coverage: sink (loop, terminal) pairs differ from the old aggregates"
        );

        // verdict / depth_bucket / unc / reachable_verdicts identical per pair.
        for (k, ov) in &old_map {
            assert_eq!(
                &new_map[k], ov,
                "decompressed cohort tuple diverged from the old aggregate at (loop, terminal) {k:?}"
            );
        }
    }

    #[test]
    fn sink_matches_old_multi_group() {
        let (routines, graph_edges, summaries) = multi_group_fixture();
        let ctx = minimal_ctx(&routines, graph_edges, summaries);
        let workspace = ws(&routines);
        let mut memo = HashMap::new();
        let (graph, seeds) = build_d1_graph(&ctx, &workspace, &mut memo);
        let r4_idx = routines.iter().position(|r| r.id == "R4").unwrap();
        let direct_ops = vec![DirectOp {
            routine: &routines[r4_idx],
            loop_id: routines[r4_idx].loops[0].id.as_str(),
            loop_info: &routines[r4_idx].loops[0],
            op: &routines[r4_idx].record_operations[0],
        }];
        let cw = ClosedWorldTempParams::new();
        assert_sink_matches_old(&graph, &seeds, &direct_ops, &ctx, &cw);
    }

    #[test]
    fn sink_matches_old_temp_vs_physical() {
        let (routines, graph_edges, summaries) = temp_vs_physical_fixture();
        let ctx = minimal_ctx(&routines, graph_edges, summaries);
        let workspace = ws(&routines);
        let mut memo = HashMap::new();
        let (graph, seeds) = build_d1_graph(&ctx, &workspace, &mut memo);
        let cw = ClosedWorldTempParams::new();
        assert_sink_matches_old(&graph, &seeds, &[], &ctx, &cw);
    }

    #[test]
    fn sink_matches_old_flowfield_tie() {
        let (routines, graph_edges, summaries) = flowfield_tie_fixture();
        let ctx = minimal_ctx(&routines, graph_edges, summaries);
        let workspace = ws(&routines);
        let mut memo = HashMap::new();
        let (graph, seeds) = build_d1_graph(&ctx, &workspace, &mut memo);
        let cw = ClosedWorldTempParams::new();
        assert_sink_matches_old(&graph, &seeds, &[], &ctx, &cw);
    }

    #[test]
    fn sink_matches_old_many_groups_multi_batch() {
        // 80 groups -> two chunks (64 + 16): exercises `batch_base` threading +
        // cross-batch terminal interning (shared terminal T reached in both).
        let (routines, graph_edges, summaries) = many_groups_fixture(80);
        let ctx = minimal_ctx(&routines, graph_edges, summaries);
        let workspace = ws(&routines);
        let mut memo = HashMap::new();
        let (graph, seeds) = build_d1_graph(&ctx, &workspace, &mut memo);
        assert_eq!(seeds.len(), 80);
        let cw = ClosedWorldTempParams::new();
        assert_sink_matches_old(&graph, &seeds, &[], &ctx, &cw);
    }

    #[test]
    fn sink_matches_old_depth_straddle() {
        let (routines, graph_edges, summaries) = straddle_fixture();
        let ctx = minimal_ctx(&routines, graph_edges, summaries);
        let workspace = ws(&routines);
        let mut memo = HashMap::new();
        let (graph, seeds) = build_d1_graph(&ctx, &workspace, &mut memo);
        let cw = ClosedWorldTempParams::new();
        assert_sink_matches_old(&graph, &seeds, &[], &ctx, &cw);
    }

    #[test]
    fn sink_matches_old_direct_and_transitive() {
        let mut r = routine("R", "procedure");
        r.loops = vec![loop_def("R/loop0")];
        r.call_sites = vec![call_site("R/cs0", "A", vec!["R/loop0".to_string()])];
        r.record_operations = vec![record_op(
            "R/T",
            "FindSet",
            "Rec",
            Some("t/R"),
            vec!["R/loop0".to_string()],
            false,
        )];
        let mut a = routine("A", "procedure");
        a.call_sites = vec![call_site("A/csR", "R", vec!["A/loop0".to_string()])];
        let routines = vec![r, a];

        let mut graph_edges: HashMap<String, Vec<CombinedEdge>> = HashMap::new();
        graph_edges.insert(
            "R".to_string(),
            vec![edge_kind("R", "A", "R/cs0", "direct")],
        );
        graph_edges.insert(
            "A".to_string(),
            vec![edge_kind("A", "R", "A/csR", "direct")],
        );
        let summaries: HashMap<String, FullRoutineSummary> = [
            ("A".to_string(), db_summary("A", "t/A")),
            ("R".to_string(), db_summary("R", "t/R")),
        ]
        .into_iter()
        .collect();

        let ctx = minimal_ctx(&routines, graph_edges, summaries);
        let workspace = ws(&routines);
        let mut memo = HashMap::new();
        let (graph, seeds) = build_d1_graph(&ctx, &workspace, &mut memo);
        let direct_ops = vec![DirectOp {
            routine: &routines[0],
            loop_id: routines[0].loops[0].id.as_str(),
            loop_info: &routines[0].loops[0],
            op: &routines[0].record_operations[0],
        }];
        let cw = ClosedWorldTempParams::new();
        assert_sink_matches_old(&graph, &seeds, &direct_ops, &ctx, &cw);
    }

    #[test]
    fn sink_matches_old_uncertain_winner() {
        use crate::engine::l4::summary::Uncertainty;

        // The deep A->X->Y->T route wins on severity; node X is uncertain, so the
        // winner's `unc` bit is TRUE — the one fixture that drives component 6 to
        // true, proving the cohort ctx `unc` matches the old `!uncertainties`.
        let mut r = routine("R", "procedure");
        r.loops = vec![loop_def("R/loop0")];
        r.call_sites = vec![call_site("R/cs0", "A", vec!["R/loop0".to_string()])];
        let mut a = routine("A", "procedure");
        a.call_sites = vec![
            call_site("A/csT", "T", vec![]),
            call_site("A/csX", "X", vec!["A/loop0".to_string()]),
        ];
        let x = routine("X", "procedure");
        let y = routine("Y", "procedure");
        let mut t = routine("T", "procedure");
        t.record_operations = vec![record_op(
            "T/op0",
            "FindSet",
            "Rec",
            Some("t/T"),
            vec![],
            false,
        )];
        let routines = vec![r, a, x, y, t];

        let mut graph_edges: HashMap<String, Vec<CombinedEdge>> = HashMap::new();
        graph_edges.insert(
            "R".to_string(),
            vec![edge_kind("R", "A", "R/cs0", "direct")],
        );
        graph_edges.insert(
            "A".to_string(),
            vec![
                edge_kind("A", "T", "A/csT", "direct"),
                edge_kind("A", "X", "A/csX", "direct"),
            ],
        );
        graph_edges.insert(
            "X".to_string(),
            vec![edge_kind("X", "Y", "X/csY", "direct")],
        );
        graph_edges.insert(
            "Y".to_string(),
            vec![edge_kind("Y", "T", "Y/csT", "direct")],
        );
        let summaries: HashMap<String, FullRoutineSummary> = ["A", "X", "Y", "T"]
            .iter()
            .map(|id| (id.to_string(), db_summary(id, &format!("t/{id}"))))
            .collect();

        let mut ctx = minimal_ctx(&routines, graph_edges, summaries);
        ctx.set_uncertainties_for_test(
            "X",
            vec![Uncertainty {
                kind: "dynamic-dispatch".to_string(),
                callsite_id: None,
                operation_id: None,
                routine_id: Some("X".to_string()),
                interface_name: None,
            }],
        );

        let workspace = ws(&routines);
        let mut memo = HashMap::new();
        let (graph, seeds) = build_d1_graph(&ctx, &workspace, &mut memo);
        let cw = ClosedWorldTempParams::new();
        assert_sink_matches_old(&graph, &seeds, &[], &ctx, &cw);
    }
}

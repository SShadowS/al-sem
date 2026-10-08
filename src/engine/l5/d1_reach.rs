//! `d1_reach` — Task 3 of the d1-reachability redesign
//! (`.superpowers/sdd/task-3-brief.md`): the algorithmic core. A single
//! multi-source label search per loop group over Task 1's compact
//! [`D1Graph`], threading Task 2's forward param-temp vector (`TempVec` via
//! `cross_hop`/`resolve_terminal`) instead of re-walking a `Vec<EvidenceStep>`
//! per terminal, aggregated per (loop, terminal-op) into one `LoopTerminalAgg`
//! with a selected winner witness.
//!
//! **Production entry point is [`search_loops_cohorts`]**, which shares this
//! module's grouping/liveness/SCC-condensation/terminal-plan setup but emits
//! per-terminal bitmap COHORTS (via `d1_dataflow::score_batch_to_sink`) instead
//! of one `LoopTerminalAgg` per (loop, terminal-op) — see its own doc. The
//! original per-loop label search documented below (`process_group` /
//! `search_loops`, Task 3's algorithm) is retained ONLY as the `#[cfg(test)]`
//! differential ORACLE that the cohort path and the `d1_dataflow` D2/D3
//! solvers are checked against; it is compiled into test builds alone and
//! never runs in a release binary.
//!
//! ## The search (locked algorithm — the brief's numbered list is the spec)
//!
//! 1. Group seeds by `(loop_routine.id, loop_id)`. Per group, ONE multi-source
//!    label search: the initial label per seed carries
//!    `cross_hop(root_state(loop_routine), seed callsite, entry, ...)`, depth
//!    `min(2, seed_depth)`, and `unc = !uncertainties_by_node[entry].is_empty()`.
//! 2. FIFO worklist (BFS by hop count). A label is the triple
//!    `(temp_vec, depth_bucket, unc)` + backpointer + hops + seed index; it is
//!    inserted only if that exact triple is NEW for the node (first discovery
//!    wins — FIFO order + CSR edge order makes first == shortest-then-lex, the
//!    deterministic witness tie-break). Cycle safety is label dedup ALONE — NO
//!    node budget, NO depth cap.
//! 3. Expansion: child `depth_bucket = min(2, depth + edge.loop_depth)`, child
//!    temp via [`cross_hop`] (using `edge.binding_ok`), child
//!    `unc = unc || !uncertainties_by_node[child].is_empty()`.
//! 4. Scoring per (loop, terminal-op): each label at the terminal's node yields
//!    a candidate — verdict = [`resolve_terminal`] + FlowField gate; the scoring
//!    depth is the bucket `min(2, label.depth + terminal.local_depth)`
//!    (`severity_for` only distinguishes `>= 2`, so the bucket is exact); the
//!    reported (unclamped) depth is `seed_depth + Σ edge.loop_depth +
//!    local_depth`, recomputed along the backpointer chain.
//! 5. Selection (external-review rule): max severity rank -> verdict quality
//!    (Physical == FlowFieldGated > Uncertain > Temporary) -> `unc == false`
//!    preferred -> fewest hops -> first-discovered (BFS order).
//! 6. Direct ops (old branch (a)) fold into the SAME aggregation: zero hops,
//!    verdict = `resolve_terminal(op, root_state(routine), ...)` + FlowField
//!    gate, depth = `op.loop_stack.len()`, witness `[loop_step, op_step]` (empty
//!    uncertainties, exactly as `d1.rs:1052-1073`). A loop can reach the same op
//!    directly AND transitively — the selection rule adjudicates.
//! 7. Witness materialization: `[loop_step, call_step]` (from the seed) + one
//!    [`hop_step`] per traversed edge + a [`terminal_step`]; the uncertainty
//!    union is `dedupe_uncertainties` over `uncertainties_by_node` of every node
//!    on the seed->terminal path (the same rule the walker applied).
//! 8. Output sorted by (loop routine id, loop id, terminal owner id, op id) —
//!    no traversal-order dependence.

use std::collections::{BTreeMap, HashMap};
#[cfg(test)]
use std::collections::{HashSet, VecDeque};

#[cfg(test)]
use crate::engine::l4::summary::{Uncertainty, dedupe_uncertainties};
use crate::engine::l5::closed_world_temp::ClosedWorldTempParams;
use crate::engine::l5::d1_cohort::{TerminalCohorts, TerminalSink, emit_finalize_census};
#[cfg(test)]
use crate::engine::l5::d1_dataflow::solve_batch;
use crate::engine::l5::d1_dataflow::{
    BATCH_WIDTH, GroupSpec, build_terminal_plan, condense, score_batch_to_sink,
};
#[cfg(test)]
use crate::engine::l5::d1_graph::{D1Edge, D1Terminal, NodeIx, edge_kind_binding_ok};
use crate::engine::l5::d1_graph::{D1Graph, D1Seed};
use crate::engine::l5::d1_liveness::compute_liveness;
use crate::engine::l5::d1_temp::ParamTemp;
#[cfg(test)]
use crate::engine::l5::d1_temp::{TempVec, cross_hop, resolve_terminal, root_state};
use crate::engine::l5::detector_context::DetectorContext;
use crate::engine::l5::detectors::anchor_of;
use crate::engine::l5::detectors::d1::{
    FLOWFIELD_GATED_OPS, TempVerdict, flowfield_gate_blocks_downgrade,
};
#[cfg(test)]
use crate::engine::l5::detectors::d1::{
    hop_step, is_setup_singleton_get, severity_for, terminal_step,
};
use crate::engine::l5::finding::{EvidenceStep, LoopCatalogEntry};
use crate::engine::l5::path_merge::sev_rank;
use crate::program::body::features::PLoop;
use crate::program::model::workspace::{ModelRecordOperation, ModelRoutine, ModelTable};

/// One (loop, terminal-op) aggregate: the old `process_group`/`solve_group`/
/// `solve_batch` oracle output shape. `search_loops_cohorts` (the production
/// path) never constructs one of these — it emits bitmap cohorts directly
/// (`TerminalCohorts`) — so this type is `#[cfg(test)]`-only.
#[cfg(test)]
pub(crate) struct LoopTerminalAgg<'a> {
    pub loop_routine: &'a ModelRoutine,
    pub loop_id: &'a str,
    pub loop_info: &'a PLoop,
    pub terminal: D1Terminal<'a>,
    /// The seed's entry callsite for a transitive winner; `None` for a direct op.
    pub entry_callsite_id: Option<&'a str>,
    /// Winning class per the selection rule (severity already realized):
    pub severity: &'static str,
    pub verdict: TempVerdict,
    /// Every distinct verdict reaching this terminal-op in this loop (sorted, deduped).
    pub reachable_verdicts: Vec<TempVerdict>,
    /// `min(2, total effective depth)` of the winner.
    pub depth_bucket: i64,
    /// The WINNER's actual (unclamped) depth (reported).
    pub effective_loop_depth: i64,
    /// loop step + hops + terminal step.
    pub witness: Vec<EvidenceStep>,
    /// Union along the WITNESS path.
    pub uncertainties: Vec<Uncertainty>,
}

/// A direct in-loop db op (old branch (a)) folded into the same aggregation.
pub(crate) struct DirectOp<'a> {
    pub routine: &'a ModelRoutine,
    pub loop_id: &'a str,
    pub loop_info: &'a PLoop,
    pub op: &'a ModelRecordOperation,
}

/// One BFS label: the identity triple `(temp_vec, depth_bucket, unc)` plus the
/// backpointer, hop count, and originating seed index. `'g` = the borrow of the
/// compact graph (the `D1Edge` backpointers live in `graph.edges`); `'a` = the
/// workspace/ctx lifetime. `process_group`-only (oracle).
#[cfg(test)]
struct LabelRec<'g, 'a> {
    node: NodeIx,
    temp_vec: TempVec,
    depth_bucket: i64,
    unc: bool,
    hops: u32,
    /// Index into the `seeds` slice this label descends from (inherited on expand).
    seed_index: usize,
    back: Back<'g, 'a>,
}

/// A label's predecessor: the seed itself, or the parent label + the edge crossed.
#[cfg(test)]
enum Back<'g, 'a> {
    Seed,
    Hop { pred: usize, edge: &'g D1Edge<'a> },
}

/// One scored candidate for a (loop, terminal-op) bucket. `discovery` is the
/// per-group generation index (direct ops first, then transitive in BFS label
/// order) — the deterministic last tie-break. `process_group`-only (oracle).
#[cfg(test)]
struct Candidate<'a> {
    verdict: TempVerdict,
    severity: &'static str,
    unc: bool,
    hops: u32,
    depth_bucket: i64,
    effective_loop_depth: i64,
    discovery: usize,
    kind: CandKind<'a>,
    terminal_op: &'a ModelRecordOperation,
    terminal_owner: &'a ModelRoutine,
    terminal_local_depth: i64,
    entry_callsite_id: Option<&'a str>,
}

/// A candidate's provenance — the witness source. A direct op carries its own
/// step inputs; a transitive candidate points back into the label arena.
/// `process_group`-only (oracle).
#[cfg(test)]
enum CandKind<'a> {
    Direct {
        routine: &'a ModelRoutine,
        loop_info: &'a PLoop,
        op: &'a ModelRecordOperation,
    },
    Transitive {
        label_idx: usize,
    },
}

/// `true` iff `node_id` has a non-empty per-node uncertainty set.
pub(crate) fn node_has_uncertainty(ctx: &DetectorContext, node_id: &str) -> bool {
    // A node with NO entry and a node carrying an EMPTY set are both `false`,
    // exactly as when this read a `HashMap<String, Arc<[Uncertainty]>>`. Pinned by
    // `absent_and_empty_are_both_no_uncertainty_and_nonempty_is_yes`; this feeds
    // `ContextKey.unc`, so changing it re-partitions d1's cohorts.
    ctx.uncertainty_view().has_any(node_id)
}

/// The terminal-op verdict: `resolve_terminal`'s [`ParamTemp`] mapped to a
/// [`TempVerdict`], with the RV-1 FlowField gate on the `Temp` case (a temp
/// `CalcFields`/`SetAutoCalcFields` whose FlowField gate BLOCKS the info
/// downgrade becomes the dedicated `FlowFieldGated`, not `Temporary`). Mirrors
/// `build_finding`'s verdict computation (`d1.rs:404-421`), forward-composed.
pub(crate) fn flowfield_verdict(
    pt: ParamTemp,
    op: &ModelRecordOperation,
    table_by_id: &HashMap<&str, &ModelTable>,
) -> TempVerdict {
    match pt {
        ParamTemp::Physical => TempVerdict::Physical,
        ParamTemp::Unknown => TempVerdict::Uncertain,
        ParamTemp::Temp => {
            if FLOWFIELD_GATED_OPS.contains(&op.op.as_str())
                && flowfield_gate_blocks_downgrade(op, table_by_id)
            {
                TempVerdict::FlowFieldGated
            } else {
                TempVerdict::Temporary
            }
        }
    }
}

/// The selection key (higher is better on each dimension, so the winner is the
/// candidate with the max key). Rule 5: severity rank -> verdict quality
/// ([`TempVerdict::quality`], the SAME rank Task 5's context ordering uses) ->
/// `unc == false` preferred -> fewest hops -> HIGHER `depth_bucket` ->
/// first-discovered. `discovery` is unique per candidate, so the key is a total
/// order (a single unique max). `process_group`-only (oracle) — `solve_group`/
/// `solve_batch`/`score_batch_to_sink` call [`selection_rank`] directly.
#[cfg(test)]
fn selection_key(c: &Candidate) -> (i32, i32, i32, i64, i64, i64) {
    selection_rank(
        c.severity,
        c.verdict,
        c.unc,
        c.hops,
        c.depth_bucket,
        c.discovery,
    )
}

/// The selection key (rule 5) from the RAW winner-selection fields, decoupled
/// from [`Candidate`] so the D2/D3 dataflow solvers
/// ([`crate::engine::l5::d1_dataflow`]) can rank their own fact-derived
/// candidates with the identical comparator instead of duplicating the tuple.
/// Higher is better on each dimension.
///
/// `depth_bucket` is a tiebreak AHEAD of `-discovery` (HIGHER bucket wins) so
/// the reported bucket — hence the golden-visible `depth_class`
/// (single-loop vs nested-loop, `d1.rs`) — is CANONICAL, not decided by an
/// engine-specific discovery order. This closes a real divergence vector:
/// `severity_for` SATURATES the depth>=2 bump (it promotes only high/medium,
/// leaving `low`/`info` unchanged — `d1.rs`), so a `low`-severity op (e.g.
/// `LockTable` -> db-lock) reached at EQUAL hops by two paths whose summed
/// loop_depth straddles the threshold (bucket 1 vs 2) ties on
/// `(severity, verdict, unc, hops)` — leaving `depth_bucket` the only thing that
/// distinguishes them. Preferring the higher bucket is conservative
/// (nested-loop is the worse finding, and it IS genuinely reachable at depth 2)
/// AND deterministic across `process_group` / `solve_group` / `solve_batch`,
/// whose discovery orders differ.
pub(crate) fn selection_rank(
    severity: &str,
    verdict: TempVerdict,
    unc: bool,
    hops: u32,
    depth_bucket: i64,
    discovery: usize,
) -> (i32, i32, i32, i64, i64, i64) {
    (
        sev_rank(severity),
        verdict.quality(),
        if unc { 0 } else { 1 },
        -(hops as i64),
        depth_bucket,
        -(discovery as i64),
    )
}

/// The unclamped (true) effective depth of a transitive candidate:
/// `seed_depth + Σ edge.loop_depth + local_depth`, recomputed along the
/// backpointer chain (rule 4's reported depth). `process_group`-only (oracle).
#[cfg(test)]
fn true_depth(labels: &[LabelRec], li: usize, local_depth: i64, seeds: &[D1Seed]) -> i64 {
    let mut sum_edges = 0i64;
    let mut cur = li;
    loop {
        match &labels[cur].back {
            Back::Seed => break,
            Back::Hop { pred, edge } => {
                sum_edges += edge.loop_depth;
                cur = *pred;
            }
        }
    }
    let seed_depth = seeds[labels[li].seed_index].seed_depth;
    seed_depth + sum_edges + local_depth
}

/// Insert a label iff its `(temp_vec, depth_bucket, unc)` triple is new for the
/// node — first discovery wins (rule 2). A revisited node with an already-seen
/// triple is not re-enqueued: this is the ONLY cycle-safety mechanism.
/// `process_group`-only (oracle).
#[cfg(test)]
#[allow(clippy::too_many_arguments)]
fn push_label<'g, 'a>(
    labels: &mut Vec<LabelRec<'g, 'a>>,
    queue: &mut VecDeque<usize>,
    seen: &mut HashMap<NodeIx, HashSet<(TempVec, i64, bool)>>,
    node: NodeIx,
    temp_vec: TempVec,
    depth_bucket: i64,
    unc: bool,
    hops: u32,
    seed_index: usize,
    back: Back<'g, 'a>,
) {
    let triple = (temp_vec.clone(), depth_bucket, unc);
    let set = seen.entry(node).or_default();
    if set.contains(&triple) {
        return;
    }
    set.insert(triple);
    let li = labels.len();
    labels.push(LabelRec {
        node,
        temp_vec,
        depth_bucket,
        unc,
        hops,
        seed_index,
        back,
    });
    queue.push_back(li);
}

/// A branch-(b) loop step (`d1.rs:1141-1148` / `d1.rs:1052-1059`).
pub(crate) fn loop_step_ev(routine: &ModelRoutine, loop_info: &PLoop) -> EvidenceStep {
    EvidenceStep {
        routine_id: routine.id.clone(),
        operation_id: None,
        callsite_id: None,
        loop_id: Some(loop_info.id.clone()),
        source_anchor: anchor_of(&loop_info.source_anchor, routine),
        note: format!("{} loop", loop_info.loop_type),
    }
}

/// The seed's call step (`d1.rs:1149-1161`): the in-loop call from the loop
/// routine into the seed's resolved callee entry.
pub(crate) fn call_step_ev<'a>(
    seed: &D1Seed<'a>,
    graph: &D1Graph<'a>,
    ctx: &DetectorContext,
) -> EvidenceStep {
    let entry_id = graph.node_ids[seed.entry as usize];
    let to_name = ctx
        .routine_by_id
        .get(entry_id)
        .map(|r| r.name.clone())
        .unwrap_or_else(|| entry_id.to_string());
    EvidenceStep {
        routine_id: seed.loop_routine.id.clone(),
        operation_id: None,
        callsite_id: Some(seed.callsite.id.clone()),
        loop_id: None,
        source_anchor: anchor_of(&seed.callsite.source_anchor, seed.loop_routine),
        note: format!("calls {to_name}"),
    }
}

/// Materialize the winning transitive candidate's witness + uncertainty union by
/// walking its backpointer chain to the seed (rule 7). `process_group`-only
/// (oracle) — the cohort/dataflow path's equivalent is `d1_dataflow`'s
/// `build_transitive_witness`/`representative_witness`.
#[cfg(test)]
fn materialize_transitive<'a>(
    labels: &[LabelRec<'_, 'a>],
    label_idx: usize,
    graph: &D1Graph<'a>,
    ctx: &DetectorContext,
    seeds: &[D1Seed<'a>],
    terminal_owner: &ModelRoutine,
    terminal_op: &ModelRecordOperation,
) -> (Vec<EvidenceStep>, Vec<Uncertainty>) {
    // Collect (from_node, edge) hops and the node ids on the path, terminal -> seed.
    let mut hops_rev: Vec<(NodeIx, &D1Edge)> = Vec::new();
    let mut nodes_rev: Vec<NodeIx> = Vec::new();
    let mut cur = label_idx;
    nodes_rev.push(labels[cur].node);
    loop {
        match &labels[cur].back {
            Back::Seed => break,
            Back::Hop { pred, edge } => {
                hops_rev.push((labels[*pred].node, edge));
                cur = *pred;
                nodes_rev.push(labels[cur].node);
            }
        }
    }

    let seed = &seeds[labels[label_idx].seed_index];
    let mut witness = Vec::with_capacity(hops_rev.len() + 3);
    witness.push(loop_step_ev(seed.loop_routine, seed.loop_info));
    witness.push(call_step_ev(seed, graph, ctx));
    // hop steps in seed -> terminal order (reverse of the terminal -> seed walk).
    for (from_node, edge) in hops_rev.iter().rev() {
        let from_id = graph.node_ids[*from_node as usize];
        let to_id = graph.node_ids[edge.to as usize];
        witness.push(hop_step(
            &ctx.routine_by_id,
            from_id,
            to_id,
            edge.kind,
            edge.callsite_id,
        ));
    }
    witness.push(terminal_step(
        &ctx.routine_by_id,
        &ctx.table_by_id,
        terminal_owner.id.as_str(),
        Some(terminal_op.id.as_str()),
    ));

    // Uncertainty union: concat uncertainties_by_node in seed -> terminal order,
    // then dedupe (== the walker's running per-node dedupe; see `path_walker`).
    let mut concat: Vec<Uncertainty> = Vec::new();
    let view = ctx.uncertainty_view();
    for &n in nodes_rev.iter().rev() {
        let nid = graph.node_ids[n as usize];
        concat.extend(view.values_of(nid).cloned());
    }
    (witness, dedupe_uncertainties(concat))
}

/// The whole multi-source search + aggregation + witness selection for ONE loop
/// group (rules 1-7). Returns one `LoopTerminalAgg` per (terminal owner, op).
///
/// NOT on the production path — `search_loops_cohorts` runs the batched
/// dataflow solver (`d1_dataflow::score_batch_to_sink`) instead. `process_group`
/// is retained as the `#[cfg(test)]` differential ORACLE the D2/D3/cohort tests
/// check `solve_group`/`solve_batch`/`score_batch_to_sink` against on the six
/// load-bearing components. Groups are INDEPENDENT: every argument is a
/// shared-immutable borrow (`graph`/`seeds`/`direct_ops`/`ctx`/`cw`) and the
/// only output is the return value, so per-group results never alias — the
/// property the fact solver's per-lane batching also relies on.
#[cfg(test)]
#[allow(clippy::too_many_arguments)]
pub(crate) fn process_group<'g, 'a>(
    graph: &'g D1Graph<'a>,
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
    let mut out: Vec<LoopTerminalAgg<'a>> = Vec::new();
    let mut labels: Vec<LabelRec<'g, 'a>> = Vec::new();
    let mut queue: VecDeque<usize> = VecDeque::new();
    let mut seen: HashMap<NodeIx, HashSet<(TempVec, i64, bool)>> = HashMap::new();

    // Rule 1: seed the frontier (in seed order).
    let root = root_state(loop_routine.id.as_str(), cw);
    for &si in seed_indices {
        let seed = &seeds[si];
        let entry_id = graph.node_ids[seed.entry as usize];
        let binding_ok = edge_kind_binding_ok(seed.entry_edge_kind);
        let temp = cross_hop(
            &root,
            seed.loop_routine,
            seed.callsite.id.as_str(),
            entry_id,
            binding_ok,
            cw,
        );
        let depth = seed.seed_depth.min(2);
        let unc = node_has_uncertainty(ctx, entry_id);
        push_label(
            &mut labels,
            &mut queue,
            &mut seen,
            seed.entry,
            temp,
            depth,
            unc,
            0,
            si,
            Back::Seed,
        );
    }

    // Rules 2-3: FIFO BFS expansion.
    while let Some(li) = queue.pop_front() {
        let node = labels[li].node;
        let from_id = graph.node_ids[node as usize];
        // Source-only: every closure node is a workspace routine. A non-source
        // node (no `routine_by_id` entry) carries no composable bindings, so it
        // cannot be expanded (defensive — never reached in the source-only pipeline).
        let Some(caller) = ctx.routine_by_id.get(from_id).copied() else {
            continue;
        };
        // Snapshot the parent's fields to release the immutable borrow before push.
        let parent_temp = labels[li].temp_vec.clone();
        let parent_depth = labels[li].depth_bucket;
        let parent_unc = labels[li].unc;
        let parent_hops = labels[li].hops;
        let seed_index = labels[li].seed_index;
        for edge in &graph.edges[node as usize] {
            let to_id = graph.node_ids[edge.to as usize];
            let child_depth = (parent_depth + edge.loop_depth).min(2);
            let child_temp = cross_hop(
                &parent_temp,
                caller,
                edge.callsite_id.unwrap_or(""),
                to_id,
                edge.binding_ok,
                cw,
            );
            let child_unc = parent_unc || node_has_uncertainty(ctx, to_id);
            push_label(
                &mut labels,
                &mut queue,
                &mut seen,
                edge.to,
                child_temp,
                child_depth,
                child_unc,
                parent_hops + 1,
                seed_index,
                Back::Hop { pred: li, edge },
            );
        }
    }

    // Rules 4/6: generate candidates. Direct ops first (branch (a) precedence),
    // then transitive candidates in BFS label discovery order. `discovery` is a
    // per-group monotonic counter — deterministic (no HashMap iteration).
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
                effective_loop_depth: local_depth,
                discovery,
                kind: CandKind::Direct {
                    routine: owner,
                    loop_info: d.loop_info,
                    op,
                },
                terminal_op: op,
                terminal_owner: owner,
                terminal_local_depth: local_depth,
                entry_callsite_id: None,
            });
        discovery += 1;
    }

    for (li, label) in labels.iter().enumerate() {
        let node = label.node;
        for t in &graph.terminals[node as usize] {
            let op = t.op;
            let owner = t.owner;
            let base_pt = resolve_terminal(op, &label.temp_vec, owner.id.as_str(), cw);
            let verdict = flowfield_verdict(base_pt, op, &ctx.table_by_id);
            let total_scoring = (label.depth_bucket + t.local_depth).min(2);
            let is_singleton = is_setup_singleton_get(op, Some(owner), &ctx.table_by_id);
            let severity = severity_for(op, verdict, total_scoring, is_singleton);
            let effective = true_depth(&labels, li, t.local_depth, seeds);
            let entry_cs = Some(seeds[label.seed_index].callsite.id.as_str());
            buckets
                .entry((owner.id.as_str(), op.id.as_str()))
                .or_default()
                .push(Candidate {
                    verdict,
                    severity,
                    unc: label.unc,
                    hops: label.hops,
                    depth_bucket: total_scoring,
                    effective_loop_depth: effective,
                    discovery,
                    kind: CandKind::Transitive { label_idx: li },
                    terminal_op: op,
                    terminal_owner: owner,
                    terminal_local_depth: t.local_depth,
                    entry_callsite_id: entry_cs,
                });
            discovery += 1;
        }
    }

    // Rules 5/7: per bucket, select the winner and materialize its witness.
    // BTreeMap iteration is already (owner id, op id) sorted.
    for (_key, cands) in buckets {
        let mut reachable_verdicts: Vec<TempVerdict> = cands.iter().map(|c| c.verdict).collect();
        reachable_verdicts.sort();
        reachable_verdicts.dedup();

        let winner = cands
            .iter()
            .max_by_key(|c| selection_key(c))
            .expect("a bucket is never empty");

        let (witness, uncertainties) = match &winner.kind {
            CandKind::Direct {
                routine,
                loop_info,
                op,
            } => {
                let loop_step = loop_step_ev(routine, loop_info);
                // For a direct op, `terminal_step(owner, op)` is byte-identical to
                // the old `op_step` (`d1.rs:1060-1067`); direct ops carry NO
                // uncertainties (the walker hardcoded `uncertainties: Vec::new()`).
                let op_step = terminal_step(
                    &ctx.routine_by_id,
                    &ctx.table_by_id,
                    routine.id.as_str(),
                    Some(op.id.as_str()),
                );
                (vec![loop_step, op_step], Vec::new())
            }
            CandKind::Transitive { label_idx } => materialize_transitive(
                &labels,
                *label_idx,
                graph,
                ctx,
                seeds,
                winner.terminal_owner,
                winner.terminal_op,
            ),
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
            entry_callsite_id: winner.entry_callsite_id,
            severity: winner.severity,
            verdict: winner.verdict,
            reachable_verdicts,
            depth_bucket: winner.depth_bucket,
            effective_loop_depth: winner.effective_loop_depth,
            witness,
            uncertainties,
        });
    }
    out
}

/// Run the reachability search over every loop group (rules 1-8) via the D3
/// batched dataflow solver (`solve_batch`), returning one `LoopTerminalAgg` per
/// (loop, terminal-op). `#[cfg(test)]`-only: the production entry point is
/// [`search_loops_cohorts`], which shares this function's grouping/batching
/// shape but emits bitmap cohorts instead of materializing every aggregate's
/// witness. `search_loops` survives solely as the differential oracle
/// `score_batch_to_sink_matches_old` (and friends) check
/// [`crate::engine::l5::d1_dataflow::score_batch_to_sink`] against.
///
/// Groups are keyed `(loop_routine_id, loop_id)` in a `BTreeMap`
/// (deterministic, sorted order — the SAME order `process_group`'s driver
/// used, and the order lanes are assigned within a batch), then chunked into
/// [`BATCH_WIDTH`]-lane batches and solved SERIALLY by `solve_batch` over one
/// shared call-SCC condensation + param-liveness. Components 1-6 per (loop,
/// terminal-op) are identical to `process_group`'s (`solve_batch` ==
/// `solve_group` == `process_group` on
/// coverage/reachable_verdicts/severity/verdict/depth_bucket/unc); only the
/// witness (component 7) may pick a different equal-ranked realizing path. The
/// one order-sensitive step, rule 8's total-order `out.sort_by`, is unchanged.
#[cfg(test)]
pub(crate) fn search_loops<'a>(
    graph: &D1Graph<'a>,
    seeds: &[D1Seed<'a>],
    direct_ops: &[DirectOp<'a>],
    ctx: &'a DetectorContext,
    cw: &ClosedWorldTempParams,
) -> Vec<LoopTerminalAgg<'a>> {
    // The loop-group universe = seed groups ∪ direct-op groups, keyed
    // (loop_routine_id, loop_id). BTreeMap => deterministic, sorted group order.
    let mut groups: BTreeMap<(&'a str, &'a str), GroupSpec<'a>> = BTreeMap::new();
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
    // Key-sorted group order (BTreeMap into_values) => deterministic lane
    // assignment; group i in a batch owns bit i.
    let groups: Vec<GroupSpec<'a>> = groups.into_values().collect();

    // Shared ONCE across every batch: the backward param-liveness fixpoint +
    // compiled transfers, and the call-graph SCC condensation.
    let g_lv = crate::engine::perf_trace::span("d1", "compute_liveness");
    let liveness = compute_liveness(graph, ctx, cw);
    drop(g_lv);
    // Hot-tier static-cost census (ΣNeed / static fact bounds) — zero cost off,
    // no effect on the emitted aggregates (goldens unaffected).
    crate::engine::l5::d1_cohort::emit_liveness_census(&liveness, graph.node_ids.len());
    let g_cd = crate::engine::perf_trace::span("d1", "condense");
    let scc = condense(graph);
    drop(g_cd);
    // The run-global terminal scoring plan (read modes + per-verdict/severity
    // tables), precomputed ONCE here and shared by every batch's scoring phase —
    // never rebuilt per batch (the old per-batch `terminal_nodes`/`read_slots`).
    let g_tp = crate::engine::perf_trace::span("d1", "terminal_plan");
    let plan = build_terminal_plan(graph, &liveness, ctx, cw);
    drop(g_tp);

    // Hot-tier (`Detail::Hot`) attribution: the batch census + kill-durable
    // per-batch cumulative timing. `d1.reach.batch_census` (once) carries the
    // group/batch/node/edge counts; `d1.reach.batches_done`/`cumulative_ms`
    // (last-value-wins C counters, re-emitted every batch) let a cap-killed run
    // still show batches/sec — distinguishing batch-count-bound from
    // per-batch-bound. Zero cost when Hot is off.
    let trace_hot = crate::engine::perf_trace::enabled(crate::engine::perf_trace::Detail::Hot);
    let n_batches = groups.len().div_ceil(BATCH_WIDTH);
    if trace_hot {
        let edges: u64 = graph.edges.iter().map(|e| e.len() as u64).sum();
        crate::engine::perf_trace::instant_lazy("d1.reach", "batch_census", || {
            serde_json::json!({
                "groups": groups.len(),
                "batches": n_batches,
                "nodes": graph.node_ids.len(),
                "edges": edges,
                "seeds": seeds.len(),
                "direct_ops": direct_ops.len(),
                "batch_width": BATCH_WIDTH,
            })
        });
    }

    // Serial 64-lane batches — each shares one condensation pass and drops its
    // arena before the next (the RSS bound; no rayon).
    let mut out: Vec<LoopTerminalAgg<'a>> = Vec::new();
    let mut cumulative_ms: u64 = 0;
    for (bi, batch) in groups.chunks(BATCH_WIDTH).enumerate() {
        let t0 = trace_hot.then(std::time::Instant::now);
        let n_before = out.len();
        out.extend(solve_batch(
            graph, &liveness, &scc, seeds, direct_ops, ctx, cw, &plan, batch,
        ));
        if let Some(t0) = t0 {
            cumulative_ms += t0.elapsed().as_millis() as u64;
            crate::engine::perf_trace::counter("d1.reach.batches_done", (bi + 1) as u64);
            crate::engine::perf_trace::counter("d1.reach.cumulative_ms", cumulative_ms);
            // OUTPUT-VOLUME signal: cumulative + per-batch (loop, terminal-op)
            // aggregate count. Tens of millions ⇒ the run is OUTPUT-BOUND (one
            // context per reaching loop at Base-App reachability density), not
            // compute-bound.
            crate::engine::perf_trace::counter("d1.reach.aggregates_so_far", out.len() as u64);
            crate::engine::perf_trace::counter(
                "d1.reach.batch_aggregates",
                (out.len() - n_before) as u64,
            );
        }
    }

    // Rule 8: deterministic output order, independent of traversal order.
    out.sort_by(|a, b| {
        a.loop_routine
            .id
            .cmp(&b.loop_routine.id)
            .then_with(|| a.loop_id.cmp(b.loop_id))
            .then_with(|| a.terminal.owner.id.cmp(&b.terminal.owner.id))
            .then_with(|| a.terminal.op.id.cmp(&b.terminal.op.id))
    });
    out
}

/// The PRODUCTION cohort-report output of one d1 run (the C6 cutover replacement
/// for `search_loops`'s `Vec<LoopTerminalAgg>`): the finalized per-terminal
/// bitmap cohorts + the run-level loop CATALOG (one [`LoopCatalogEntry`] per
/// loop-group, indexed by `loop_ix` == its GLOBAL group index == its position in
/// the sorted `groups` vector == the `GroupIx` bit the sink's cohort bitmaps set).
/// `detect_d1` consumes `terminals` to assemble one compressed `Finding` per
/// reached terminal, decompressing each cohort's `loop_set` back through
/// `catalog[loop_ix]`.
pub(crate) struct D1CohortRun<'a> {
    pub terminals: Vec<TerminalCohorts<'a>>,
    pub catalog: Vec<LoopCatalogEntry>,
}

/// Run the reachability search over every loop group, EMITTING per-terminal
/// bitmap COHORTS (the C6 cutover production path) instead of one
/// [`LoopTerminalAgg`] witness per `(loop, terminal-op)`. Same grouping /
/// liveness / SCC condensation / terminal plan as [`search_loops`]; the ONLY
/// change is the per-batch emission — [`score_batch_to_sink`] sets the winner's
/// loop bit in its `(terminal, ContextKey)` cohort (building ONE bounded
/// representative witness per cohort, lazily) rather than materializing a full
/// witness for every winning lane. The winner SELECTION (the running-best scan)
/// is byte-for-byte the same, so the per-`(loop, terminal)` verdict / depth_bucket
/// / unc / reachable_verdicts are identical to `search_loops`' aggregates — the
/// differential (`d1_dataflow::score_batch_to_sink_matches_old`) proves it.
pub(crate) fn search_loops_cohorts<'a>(
    graph: &D1Graph<'a>,
    seeds: &[D1Seed<'a>],
    direct_ops: &[DirectOp<'a>],
    ctx: &'a DetectorContext,
    cw: &ClosedWorldTempParams,
) -> D1CohortRun<'a> {
    // The loop-group universe — IDENTICAL construction/ordering to `search_loops`
    // (BTreeMap by (loop_routine_id, loop_id) => deterministic, sorted group order;
    // group i owns bit i / catalog index i).
    let mut groups: BTreeMap<(&'a str, &'a str), GroupSpec<'a>> = BTreeMap::new();
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
    let groups: Vec<GroupSpec<'a>> = groups.into_values().collect();

    // The run-level loop catalog: one entry per group, positional by GLOBAL group
    // index (== the `GroupIx` the sink's cohort bitmaps set). `entry_callsite_id`
    // is the group's FIRST seed callsite (representative; `None` for a direct-op-
    // only group), advisory metadata for rendering — never a correctness input.
    let catalog: Vec<LoopCatalogEntry> = groups
        .iter()
        .enumerate()
        .map(|(g, gs)| LoopCatalogEntry {
            loop_ix: g as u32,
            loop_routine_id: gs.loop_routine.id.clone(),
            loop_id: gs.loop_id.to_string(),
            anchor: anchor_of(&gs.loop_info.source_anchor, gs.loop_routine),
            entry_callsite_id: gs
                .seed_indices
                .first()
                .map(|&si| seeds[si].callsite.id.clone()),
        })
        .collect();

    let g_lv = crate::engine::perf_trace::span("d1", "compute_liveness");
    let liveness = compute_liveness(graph, ctx, cw);
    drop(g_lv);
    crate::engine::l5::d1_cohort::emit_liveness_census(&liveness, graph.node_ids.len());
    let g_cd = crate::engine::perf_trace::span("d1", "condense");
    let scc = condense(graph);
    drop(g_cd);
    let g_tp = crate::engine::perf_trace::span("d1", "terminal_plan");
    let plan = build_terminal_plan(graph, &liveness, ctx, cw);
    drop(g_tp);

    let trace_hot = crate::engine::perf_trace::enabled(crate::engine::perf_trace::Detail::Hot);
    let n_batches = groups.len().div_ceil(BATCH_WIDTH);
    if trace_hot {
        let edges: u64 = graph.edges.iter().map(|e| e.len() as u64).sum();
        crate::engine::perf_trace::instant_lazy("d1.cohort", "batch_census", || {
            serde_json::json!({
                "groups": groups.len(),
                "batches": n_batches,
                "nodes": graph.node_ids.len(),
                "edges": edges,
                "seeds": seeds.len(),
                "direct_ops": direct_ops.len(),
                "batch_width": BATCH_WIDTH,
            })
        });
    }

    // Serial 64-lane batches — each shares one condensation pass, drops its fact
    // arena before the next, and emits its winners into the RUN-GLOBAL sink.
    let mut sink = TerminalSink::new(plan.terminal_count(), groups.len());
    let mut cumulative_ms: u64 = 0;
    for (bi, batch) in groups.chunks(BATCH_WIDTH).enumerate() {
        let t0 = trace_hot.then(std::time::Instant::now);
        score_batch_to_sink(
            graph,
            &liveness,
            &scc,
            seeds,
            direct_ops,
            ctx,
            cw,
            &plan,
            batch,
            bi * BATCH_WIDTH,
            &mut sink,
        );
        if let Some(t0) = t0 {
            cumulative_ms += t0.elapsed().as_millis() as u64;
            crate::engine::perf_trace::counter("d1.cohort.batches_done", (bi + 1) as u64);
            crate::engine::perf_trace::counter("d1.cohort.cumulative_ms", cumulative_ms);
        }
    }

    let terminals = sink.finalize();
    emit_finalize_census(&terminals, &ctx.uncertainties);
    D1CohortRun { terminals, catalog }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::l4::combined_graph::CombinedEdge;
    use crate::engine::l5::d1_graph::build_d1_graph;
    use crate::engine::l5::full_summary::FullRoutineSummary;
    use crate::engine::l5::test_support::{
        call_site, coverage, edge_kind, fact, loop_def, minimal_ctx, record_op, routine, summary,
    };
    use crate::program::model::workspace::ModelEntities;

    /// A built fixture: the owned routines + the `edges_by_from` map + the
    /// per-routine summaries `minimal_ctx` / `build_d1_graph` consume.
    type Fixture = (
        Vec<ModelRoutine>,
        HashMap<String, Vec<CombinedEdge>>,
        HashMap<String, FullRoutineSummary>,
    );

    /// A summary with a single `read table` fact (touches_db == Yes -> included
    /// in the closure).
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

    // =======================================================================
    // Test 1 (defect D-A): the search must find a terminal the old walker's
    // 500-node budget starved. Star fan-out of 600 dead-end nodes plus one path
    // to a terminal placed AFTER them in edge order.
    // =======================================================================
    #[test]
    fn finds_terminal_missed_by_budget() {
        let mut r = routine("R", "procedure");
        r.loops = vec![loop_def("R/loop0")];
        r.call_sites = vec![call_site("R/cs0", "A", vec!["R/loop0".to_string()])];
        let a = routine("A", "procedure");

        let mut routines = vec![r, a];
        let mut a_edges: Vec<CombinedEdge> = Vec::new();
        let mut summaries: HashMap<String, FullRoutineSummary> = HashMap::new();
        summaries.insert("A".to_string(), db_summary("A", "t/A"));

        // 600 dead-end nodes (summary => real node, but NO record ops => no terminal).
        for i in 0..600 {
            let did = format!("D{i}");
            routines.push(routine(&did, "procedure"));
            a_edges.push(edge_kind("A", &did, &format!("A/cs{i}"), "direct"));
            summaries.insert(did.clone(), db_summary(&did, &format!("t/{did}")));
        }
        // The terminal, LAST in edge order (past the 500-node budget).
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

        assert!(
            graph.node_ids.len() > 500,
            "closure must exceed the old 500-node budget ({} nodes)",
            graph.node_ids.len()
        );

        let cw = ClosedWorldTempParams::new();
        let aggs = search_loops(&graph, &seeds, &[], &ctx, &cw);
        assert!(
            aggs.iter().any(|a| a.terminal.op.id == "T/op0"),
            "the budget-starved terminal must be found"
        );
    }

    // =======================================================================
    // Test 2: severity prefers a realizable depth-2 route over a shorter depth-1
    // route. Two routes to the same op: A->T (1 hop, bucket 1, medium) and
    // A->X->Y->T (3 hops, bucket 2, high). Winner = high + the 3-hop witness.
    // Exercises non-zero D1Edge.loop_depth via ctx.call_site_by_id (Task 1
    // review gap).
    // =======================================================================
    #[test]
    fn severity_prefers_realizable_depth2_over_shorter_depth1() {
        let mut r = routine("R", "procedure");
        r.loops = vec![loop_def("R/loop0")];
        r.call_sites = vec![call_site("R/cs0", "A", vec!["R/loop0".to_string()])];

        // A carries two OWN call sites so `call_site_by_id` gives the A->X edge a
        // loop_depth of 1 (A/csX loop_stack len 1) and A->T a depth of 0. A has NO
        // loops, so neither seeds a search.
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
        let aggs = search_loops(&graph, &seeds, &[], &ctx, &cw);

        assert_eq!(aggs.len(), 1, "one (loop, op) aggregate");
        let agg = &aggs[0];
        assert_eq!(agg.terminal.op.id, "T/op0");
        assert_eq!(agg.severity, "high", "the realizable depth-2 route wins");
        assert_eq!(agg.depth_bucket, 2);
        assert_eq!(
            agg.effective_loop_depth, 2,
            "seed 1 + A->X edge 1 + local 0"
        );
        // [loop, call, hop(A->X), hop(X->Y), hop(Y->T), terminal] = the 3-hop witness.
        assert_eq!(agg.witness.len(), 6, "the 3-hop route witness");
        assert_eq!(agg.entry_callsite_id, Some("R/cs0"));
        assert_eq!(agg.reachable_verdicts, vec![TempVerdict::Uncertain]);
    }

    // =======================================================================
    // Test 3: a physical route beats a temp route into the same PD-terminal op.
    // seed A (cs0) passes a temp var; seed B (cs1) passes a physical var. The
    // aggregate verdict must be Physical, witness through B, and
    // reachable_verdicts == [Temporary, Physical] (sorted).
    // =======================================================================
    fn temp_vs_physical_fixture() -> Fixture {
        use crate::engine::l5::test_support::{arg_binding, ts_known, ts_pd};

        let mut r = routine("R", "procedure");
        r.loops = vec![loop_def("R/loop0")];
        let mut cs0 = call_site("R/cs0", "H", vec!["R/loop0".to_string()]);
        cs0.argument_bindings = vec![arg_binding(0, Some(ts_known(true)))]; // temp
        let mut cs1 = call_site("R/cs1", "H", vec!["R/loop0".to_string()]);
        cs1.argument_bindings = vec![arg_binding(0, Some(ts_known(false)))]; // physical
        r.call_sites = vec![cs0, cs1];

        let mut h = routine("H", "procedure");
        let mut op0 = record_op("H/op0", "Modify", "Rec", Some("t/H"), vec![], false);
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
    fn physical_route_beats_temp_route_same_severity_inputs() {
        let (routines, graph_edges, summaries) = temp_vs_physical_fixture();
        let ctx = minimal_ctx(&routines, graph_edges, summaries);
        let workspace = ws(&routines);
        let mut memo = HashMap::new();
        let (graph, seeds) = build_d1_graph(&ctx, &workspace, &mut memo);
        assert_eq!(seeds.len(), 2, "two in-loop callsites into H");

        let cw = ClosedWorldTempParams::new();
        let aggs = search_loops(&graph, &seeds, &[], &ctx, &cw);
        assert_eq!(aggs.len(), 1);
        let agg = &aggs[0];
        assert_eq!(agg.terminal.op.id, "H/op0");
        assert_eq!(agg.verdict, TempVerdict::Physical, "physical route wins");
        assert_eq!(
            agg.severity, "high",
            "Physical Modify => op-based severity, not info"
        );
        assert_eq!(
            agg.entry_callsite_id,
            Some("R/cs1"),
            "witness through seed B"
        );
        assert_eq!(
            agg.reachable_verdicts,
            vec![TempVerdict::Temporary, TempVerdict::Physical],
            "both verdicts collected, sorted"
        );
        assert_eq!(agg.witness.len(), 3, "[loop, call(cs1), terminal]");
        assert_eq!(agg.witness[1].callsite_id.as_deref(), Some("R/cs1"));
    }

    // =======================================================================
    // Test 4: an A->B->A cycle with the terminal on B. The search terminates
    // (label dedup, no budget) and produces exactly one aggregate.
    // =======================================================================
    #[test]
    fn cycle_terminates_and_dedupes_labels() {
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
        ); // cycle

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
        let aggs = search_loops(&graph, &seeds, &[], &ctx, &cw);

        assert_eq!(aggs.len(), 1, "the cycle yields exactly one aggregate");
        assert_eq!(aggs[0].terminal.op.id, "B/op0");
    }

    // =======================================================================
    // Test 5: a loop reaches op T directly (depth 1, medium) and transitively at
    // bucket 2 (high). The transitively-realized severity wins the shared
    // per-(loop, op) aggregation.
    // =======================================================================
    #[test]
    fn direct_and_transitive_same_op_adjudicated() {
        // R's in-loop op T is BOTH a direct op AND a transitive terminal (the
        // search cycles back A->R and finds T on R).
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
        // A->R edge carries loop_depth 1 (pushes the transitive route to bucket 2);
        // A has no loops, so it does not seed.
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

        // The direct op (branch (a)) for T, folded into the same aggregation.
        let direct_ops = vec![DirectOp {
            routine: &routines[0],
            loop_id: routines[0].loops[0].id.as_str(),
            loop_info: &routines[0].loops[0],
            op: &routines[0].record_operations[0],
        }];

        let cw = ClosedWorldTempParams::new();
        let aggs = search_loops(&graph, &seeds, &direct_ops, &ctx, &cw);

        assert_eq!(aggs.len(), 1, "one aggregate for (R, R/T)");
        let agg = &aggs[0];
        assert_eq!(agg.terminal.op.id, "R/T");
        assert_eq!(agg.severity, "high", "the transitive bucket-2 route wins");
        assert_eq!(agg.depth_bucket, 2);
        assert_eq!(
            agg.effective_loop_depth, 3,
            "seed 1 + A->R edge 1 + local 1"
        );
        assert_eq!(
            agg.entry_callsite_id,
            Some("R/cs0"),
            "the winner is the transitive route, not the direct op"
        );
        assert_eq!(agg.witness.len(), 4, "[loop, call, hop(A->R), terminal]");
        assert_eq!(agg.reachable_verdicts, vec![TempVerdict::Uncertain]);
    }

    // =======================================================================
    // Test 6: determinism — two full builds produce field-for-field-equal output.
    // =======================================================================
    #[test]
    fn deterministic_across_runs() {
        let (routines, graph_edges, summaries) = temp_vs_physical_fixture();
        let workspace = ws(&routines);
        let cw = ClosedWorldTempParams::new();

        let ctx1 = minimal_ctx(&routines, graph_edges.clone(), summaries.clone());
        let mut memo1 = HashMap::new();
        let (graph1, seeds1) = build_d1_graph(&ctx1, &workspace, &mut memo1);
        let aggs1 = search_loops(&graph1, &seeds1, &[], &ctx1, &cw);

        let ctx2 = minimal_ctx(&routines, graph_edges, summaries);
        let mut memo2 = HashMap::new();
        let (graph2, seeds2) = build_d1_graph(&ctx2, &workspace, &mut memo2);
        let aggs2 = search_loops(&graph2, &seeds2, &[], &ctx2, &cw);

        assert_eq!(aggs1.len(), aggs2.len());
        for (x, y) in aggs1.iter().zip(aggs2.iter()) {
            assert_eq!(x.loop_routine.id, y.loop_routine.id);
            assert_eq!(x.loop_id, y.loop_id);
            assert_eq!(x.terminal.op.id, y.terminal.op.id);
            assert_eq!(x.terminal.owner.id, y.terminal.owner.id);
            assert_eq!(x.terminal.local_depth, y.terminal.local_depth);
            assert_eq!(x.entry_callsite_id, y.entry_callsite_id);
            assert_eq!(x.severity, y.severity);
            assert_eq!(x.verdict, y.verdict);
            assert_eq!(x.reachable_verdicts, y.reachable_verdicts);
            assert_eq!(x.depth_bucket, y.depth_bucket);
            assert_eq!(x.effective_loop_depth, y.effective_loop_depth);
            assert_eq!(x.witness, y.witness);
            assert_eq!(x.uncertainties, y.uncertainties);
        }
    }

    // =======================================================================
    // Task 7a: parallel/serial equivalence + determinism. Three INDEPENDENT
    // transitive groups (R1, R2, R3) all reach the SAME terminal (T/op0) —
    // the overlapping-closure shape the rayon `par_iter` refactor must not
    // scramble — plus one direct-op-only group (R4) with its own terminal.
    // Two invariants: (a) `search_loops` (the parallel entry point) is
    // field-identical across repeated calls; (b) it equals a serial
    // reference this test computes by calling `process_group` directly, per
    // group, in a DELIBERATELY reversed (non-canonical) order, then applying
    // rule 8's own sort — proving the final output does not depend on which
    // order the independent groups are processed in (the exact property
    // that licenses running them on rayon's pool instead of serially).
    //
    // TDD note: this test is written against the POST-refactor
    // `process_group` signature (returns `Vec<LoopTerminalAgg>`), so before
    // the refactor lands it fails to COMPILE against the pre-refactor
    // `&mut out`-taking signature — a meaningful RED for a call-order
    // refactor (there is no pre-refactor runtime behavior to trivially pass
    // against; the old code is already serial-only).
    // =======================================================================
    fn multi_group_fixture() -> Fixture {
        let mut r1 = routine("R1", "procedure");
        r1.loops = vec![loop_def("R1/loop0")];
        r1.call_sites = vec![call_site("R1/cs0", "A1", vec!["R1/loop0".to_string()])];
        let mut a1 = routine("A1", "procedure");
        a1.call_sites = vec![call_site("A1/csT", "T", vec![])];

        let mut r2 = routine("R2", "procedure");
        r2.loops = vec![loop_def("R2/loop0")];
        r2.call_sites = vec![call_site("R2/cs0", "A2", vec!["R2/loop0".to_string()])];
        // A2 carries two OWN call sites (mirrors Test 2): A2->T direct (decoy,
        // 1 hop) AND A2->X2->Y2->T (3 hops, bucket 2 via the A2/csX loop_stack
        // decoy) — the deeper realizable route must win WITHIN this group,
        // independent of any other group's search.
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

        // R4: a direct-op-only group (branch (a)) — no seed, own terminal.
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

        // The SHARED terminal, reached transitively by R1, R2 and R3.
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
    fn parallel_matches_serial_reference_and_is_deterministic() {
        let (routines, graph_edges, summaries) = multi_group_fixture();
        let ctx = minimal_ctx(&routines, graph_edges, summaries);
        let workspace = ws(&routines);
        let mut memo = HashMap::new();
        let (graph, seeds) = build_d1_graph(&ctx, &workspace, &mut memo);
        assert_eq!(
            seeds.len(),
            3,
            "one in-loop seed per transitive group (R1/R2/R3)"
        );

        let r4_idx = routines.iter().position(|r| r.id == "R4").unwrap();
        let direct_ops = vec![DirectOp {
            routine: &routines[r4_idx],
            loop_id: routines[r4_idx].loops[0].id.as_str(),
            loop_info: &routines[r4_idx].loops[0],
            op: &routines[r4_idx].record_operations[0],
        }];

        let cw = ClosedWorldTempParams::new();

        // (a) determinism: two full invocations of `search_loops` (the
        // parallel entry point) produce field-identical output.
        let aggs1 = search_loops(&graph, &seeds, &direct_ops, &ctx, &cw);
        let aggs2 = search_loops(&graph, &seeds, &direct_ops, &ctx, &cw);
        assert_eq!(aggs1.len(), aggs2.len());
        for (x, y) in aggs1.iter().zip(aggs2.iter()) {
            assert_eq!(x.loop_routine.id, y.loop_routine.id);
            assert_eq!(x.loop_id, y.loop_id);
            assert_eq!(x.terminal.op.id, y.terminal.op.id);
            assert_eq!(x.terminal.owner.id, y.terminal.owner.id);
            assert_eq!(x.entry_callsite_id, y.entry_callsite_id);
            assert_eq!(x.severity, y.severity);
            assert_eq!(x.verdict, y.verdict);
            assert_eq!(x.reachable_verdicts, y.reachable_verdicts);
            assert_eq!(x.depth_bucket, y.depth_bucket);
            assert_eq!(x.effective_loop_depth, y.effective_loop_depth);
            assert_eq!(x.witness, y.witness);
            assert_eq!(x.uncertainties, y.uncertainties);
        }

        assert_eq!(
            aggs1.len(),
            4,
            "3 transitive groups into T + 1 direct-op group"
        );
        let overlap_count = aggs1
            .iter()
            .filter(|a| a.terminal.owner.id == "T" && a.terminal.op.id == "T/op0")
            .count();
        assert_eq!(
            overlap_count, 3,
            "R1/R2/R3 all reach the SAME terminal T/op0 — the overlapping-closure shape"
        );

        // (b) equals a serial reference: group the seeds/direct-ops EXACTLY
        // like `search_loops` does, but call `process_group` directly, per
        // group, in a DELIBERATELY reversed order, then apply rule 8's own
        // sort.
        #[allow(clippy::type_complexity)]
        let mut ref_groups: Vec<(&ModelRoutine, &str, &PLoop, Vec<usize>, Vec<usize>)> = Vec::new();
        for (i, seed) in seeds.iter().enumerate() {
            let key = (seed.loop_routine.id.as_str(), seed.loop_id);
            if let Some(g) = ref_groups
                .iter_mut()
                .find(|g| (g.0.id.as_str(), g.1) == key)
            {
                g.3.push(i);
            } else {
                ref_groups.push((
                    seed.loop_routine,
                    seed.loop_id,
                    seed.loop_info,
                    vec![i],
                    Vec::new(),
                ));
            }
        }
        for (i, d) in direct_ops.iter().enumerate() {
            let key = (d.routine.id.as_str(), d.loop_id);
            if let Some(g) = ref_groups
                .iter_mut()
                .find(|g| (g.0.id.as_str(), g.1) == key)
            {
                g.4.push(i);
            } else {
                ref_groups.push((d.routine, d.loop_id, d.loop_info, Vec::new(), vec![i]));
            }
        }
        // Reversed relative to discovery order — proves the final sort makes
        // call order irrelevant (the property parallel execution relies on).
        ref_groups.reverse();

        let mut serial_out: Vec<LoopTerminalAgg> = Vec::new();
        for (loop_routine, loop_id, loop_info, seed_indices, direct_indices) in &ref_groups {
            serial_out.extend(process_group(
                &graph,
                &seeds,
                &direct_ops,
                &ctx,
                &cw,
                loop_routine,
                loop_id,
                loop_info,
                seed_indices,
                direct_indices,
            ));
        }
        serial_out.sort_by(|a, b| {
            a.loop_routine
                .id
                .cmp(&b.loop_routine.id)
                .then_with(|| a.loop_id.cmp(b.loop_id))
                .then_with(|| a.terminal.owner.id.cmp(&b.terminal.owner.id))
                .then_with(|| a.terminal.op.id.cmp(&b.terminal.op.id))
        });

        assert_eq!(serial_out.len(), aggs1.len());
        for (x, y) in serial_out.iter().zip(aggs1.iter()) {
            assert_eq!(x.loop_routine.id, y.loop_routine.id);
            assert_eq!(x.loop_id, y.loop_id);
            assert_eq!(x.terminal.op.id, y.terminal.op.id);
            assert_eq!(x.terminal.owner.id, y.terminal.owner.id);
            assert_eq!(x.entry_callsite_id, y.entry_callsite_id);
            assert_eq!(x.severity, y.severity);
            assert_eq!(x.verdict, y.verdict);
            assert_eq!(x.reachable_verdicts, y.reachable_verdicts);
            assert_eq!(x.depth_bucket, y.depth_bucket);
            assert_eq!(x.effective_loop_depth, y.effective_loop_depth);
            assert_eq!(x.witness, y.witness);
            assert_eq!(x.uncertainties, y.uncertainties);
        }
    }

    // =======================================================================
    // The selection comparator (rule 5) in isolation: max severity rank ->
    // verdict quality (Physical == FlowFieldGated > Uncertain > Temporary) ->
    // `unc == false` preferred -> fewest hops -> first-discovered. Each
    // assertion isolates ONE dimension (all lower dimensions held equal, and a
    // deliberately ADVERSE lower dimension proves the higher one dominates).
    // =======================================================================
    #[test]
    fn selection_order_comparator() {
        let owner = routine("OWN", "procedure");
        let op = record_op("OWN/op0", "Modify", "Rec", None, vec![], false);

        // Build a candidate varying exactly the 5 selection dimensions.
        fn cand<'a>(
            op: &'a ModelRecordOperation,
            owner: &'a ModelRoutine,
            severity: &'static str,
            verdict: TempVerdict,
            unc: bool,
            hops: u32,
            discovery: usize,
        ) -> Candidate<'a> {
            Candidate {
                verdict,
                severity,
                unc,
                hops,
                depth_bucket: 0,
                effective_loop_depth: 0,
                discovery,
                kind: CandKind::Transitive { label_idx: 0 },
                terminal_op: op,
                terminal_owner: owner,
                terminal_local_depth: 0,
                entry_callsite_id: None,
            }
        }
        // Return the discovery index of the winner among the candidates.
        let winner_of = |cands: &[Candidate]| -> usize {
            cands
                .iter()
                .max_by_key(|c| selection_key(c))
                .unwrap()
                .discovery
        };

        // (1) Severity dominates even with an adverse verdict/unc/hops/discovery.
        let c = [
            cand(&op, &owner, "high", TempVerdict::Physical, false, 0, 0),
            cand(&op, &owner, "critical", TempVerdict::Temporary, true, 9, 1),
        ];
        assert_eq!(winner_of(&c), 1, "critical outranks high");

        // (2) Verdict quality breaks a severity tie (Physical > Uncertain > Temporary).
        let c = [
            cand(&op, &owner, "high", TempVerdict::Uncertain, false, 0, 0),
            cand(&op, &owner, "high", TempVerdict::Physical, true, 9, 1),
        ];
        assert_eq!(
            winner_of(&c),
            1,
            "Physical outranks Uncertain at equal severity"
        );
        // Physical == FlowFieldGated (same quality rank) — then unc decides.
        let c = [
            cand(&op, &owner, "high", TempVerdict::FlowFieldGated, true, 0, 0),
            cand(&op, &owner, "high", TempVerdict::Physical, false, 0, 1),
        ];
        assert_eq!(
            winner_of(&c),
            1,
            "Physical/FlowFieldGated tie => unc==false wins"
        );

        // (3) unc == false preferred at equal severity + verdict.
        let c = [
            cand(&op, &owner, "high", TempVerdict::Physical, true, 0, 0),
            cand(&op, &owner, "high", TempVerdict::Physical, false, 9, 1),
        ];
        assert_eq!(winner_of(&c), 1, "unc==false beats unc==true");

        // (4) Fewest hops at equal severity + verdict + unc.
        let c = [
            cand(&op, &owner, "high", TempVerdict::Physical, false, 3, 0),
            cand(&op, &owner, "high", TempVerdict::Physical, false, 1, 1),
        ];
        assert_eq!(winner_of(&c), 1, "fewer hops wins");

        // (4b) HIGHER depth_bucket wins at equal severity+verdict+unc+hops, ahead
        // of -discovery — even with an ADVERSE (higher) discovery. This is the
        // canonicalization that keeps the reported bucket (-> depth_class)
        // deterministic across engines whose discovery orders differ (the
        // saturating-severity straddle: `low`/`info` ops don't get the depth>=2
        // bump, so bucket 1 vs 2 ties on every dimension above depth_bucket).
        let mut lo = cand(&op, &owner, "low", TempVerdict::Physical, false, 2, 0);
        lo.depth_bucket = 1;
        let mut hi = cand(&op, &owner, "low", TempVerdict::Physical, false, 2, 9);
        hi.depth_bucket = 2;
        let c = [lo, hi];
        assert_eq!(
            winner_of(&c),
            9,
            "higher depth_bucket wins ahead of -discovery (deterministic depth_class)"
        );

        // (5) First-discovered breaks a full tie (lowest discovery wins).
        let c = [
            cand(&op, &owner, "high", TempVerdict::Physical, false, 2, 5),
            cand(&op, &owner, "high", TempVerdict::Physical, false, 2, 3),
        ];
        assert_eq!(winner_of(&c), 3, "lowest discovery (first-discovered) wins");
    }

    // =======================================================================
    // Task D3: the FULL new `search_loops` (batch driver) equals a reference
    // that runs `process_group` per group + rule-8 sort, on components 1-6.
    // Run on the existing multi-group fixture (one batch) AND a >BATCH_WIDTH
    // fixture (forcing chunking) — proving the grouping / lane assignment /
    // chunking / final sort glue preserves the six load-bearing components. Only
    // the witness (component 7) may pick a different equal-ranked path.
    // =======================================================================

    /// The per-group `process_group` oracle: group EXACTLY as `search_loops`
    /// does, run `process_group` per group, apply rule 8's own sort.
    fn oracle_reference<'a>(
        graph: &D1Graph<'a>,
        seeds: &[D1Seed<'a>],
        direct_ops: &[DirectOp<'a>],
        ctx: &'a DetectorContext,
        cw: &ClosedWorldTempParams,
    ) -> Vec<LoopTerminalAgg<'a>> {
        let mut groups: BTreeMap<(&'a str, &'a str), GroupSpec<'a>> = BTreeMap::new();
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
        let mut out: Vec<LoopTerminalAgg<'a>> = Vec::new();
        for group in groups.into_values() {
            out.extend(process_group(
                graph,
                seeds,
                direct_ops,
                ctx,
                cw,
                group.loop_routine,
                group.loop_id,
                group.loop_info,
                &group.seed_indices,
                &group.direct_indices,
            ));
        }
        out.sort_by(|a, b| {
            a.loop_routine
                .id
                .cmp(&b.loop_routine.id)
                .then_with(|| a.loop_id.cmp(b.loop_id))
                .then_with(|| a.terminal.owner.id.cmp(&b.terminal.owner.id))
                .then_with(|| a.terminal.op.id.cmp(&b.terminal.op.id))
        });
        out
    }

    fn assert_search_loops_matches_oracle(
        graph: &D1Graph,
        seeds: &[D1Seed],
        direct_ops: &[DirectOp],
        ctx: &DetectorContext,
        cw: &ClosedWorldTempParams,
    ) {
        let got = search_loops(graph, seeds, direct_ops, ctx, cw);
        let want = oracle_reference(graph, seeds, direct_ops, ctx, cw);
        assert_eq!(
            got.len(),
            want.len(),
            "aggregate count (coverage, component 1) must match the oracle"
        );
        assert!(!got.is_empty(), "fixture must produce aggregates");
        for (g, r) in got.iter().zip(want.iter()) {
            // Component 1 (coverage) + rule-8 ordering: identical (loop, owner, op).
            assert_eq!(g.loop_routine.id, r.loop_routine.id, "loop routine (1)");
            assert_eq!(g.loop_id, r.loop_id, "loop id (1)");
            assert_eq!(
                g.terminal.owner.id, r.terminal.owner.id,
                "terminal owner (1)"
            );
            assert_eq!(g.terminal.op.id, r.terminal.op.id, "terminal op (1)");
            assert_eq!(g.reachable_verdicts, r.reachable_verdicts, "reachable (2)");
            assert_eq!(g.severity, r.severity, "severity (3)");
            assert_eq!(g.verdict, r.verdict, "verdict (4)");
            assert_eq!(g.depth_bucket, r.depth_bucket, "depth_bucket (5)");
            assert_eq!(
                g.uncertainties.is_empty(),
                r.uncertainties.is_empty(),
                "unc presence (6)"
            );
        }
    }

    /// n independent loop groups, all overlapping on a shared recursive SCC
    /// (C<->D, terminal on D) and a shared plain terminal T.
    fn many_groups_fixture(n: usize) -> Fixture {
        let mut c = routine("C", "procedure");
        c.call_sites = vec![call_site("C/csD", "D", vec![])];
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
        routines.push(c);
        routines.push(d);
        routines.push(t);
        (routines, graph_edges, summaries)
    }

    #[test]
    fn search_loops_matches_process_group() {
        let cw = ClosedWorldTempParams::new();

        // (a) The existing multi-group fixture (single batch): direct + transitive.
        {
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
            assert_search_loops_matches_oracle(&graph, &seeds, &direct_ops, &ctx, &cw);
        }

        // (b) A >BATCH_WIDTH fixture (80 groups -> chunked into >=2 batches),
        // overlapping on a shared recursive SCC — exercises the chunking glue.
        {
            let (routines, graph_edges, summaries) = many_groups_fixture(80);
            let ctx = minimal_ctx(&routines, graph_edges, summaries);
            let workspace = ws(&routines);
            let mut memo = HashMap::new();
            let (graph, seeds) = build_d1_graph(&ctx, &workspace, &mut memo);
            assert_eq!(seeds.len(), 80, "one in-loop seed per group");
            assert!(seeds.len() > BATCH_WIDTH, "must exceed one batch");
            assert_search_loops_matches_oracle(&graph, &seeds, &[], &ctx, &cw);
        }
    }
}

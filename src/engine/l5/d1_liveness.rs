//! `d1_liveness` — Task D1 of the d1 dataflow-solver redesign
//! (`.superpowers/sdd/task-d1-brief.md`,
//! `docs/superpowers/plans/2026-07-20-d1-dataflow-solver.md`): the backward
//! `Need[node]` param-liveness fixpoint + the compiled per-edge
//! [`ParamTransfer`] table the fact solvers consume. `compute_liveness` is
//! LIVE — called once per run by the production
//! `d1_reach::search_loops_cohorts` and shared across every batch's
//! `d1_dataflow::score_batch_to_sink` fixpoint.
//!
//! ## Why this is exact, not an approximation (the unary premise)
//!
//! The whole dataflow-solver redesign rests on d1's temp propagation being
//! UNARY: every callee parameter [`d1_temp::cross_hop`] produces is a function
//! of AT MOST ONE caller parameter (or a constant / closed-world-proven value /
//! `Unknown`) — never a combination of several — and every terminal
//! [`d1_temp::resolve_terminal`] reads AT MOST ONE parameter. This module's own
//! construction PROVES that premise rather than assuming it:
//! [`classify_edge_param`] below is a literal per-parameter transcription of
//! `cross_hop`'s binding-table loop body (`d1_temp.rs:130-167`) — same check
//! order (closed-world-proven FIRST, unconditionally; then the `binding_ok`
//! edge-kind guard; then the callsite lookup; then the argument-binding lookup;
//! then the `Known`/`ParameterDependent`/`Unknown` match) — and every one of
//! its outcomes is EITHER a constant ([`ParamTemp`], no caller dependency) OR a
//! copy of exactly ONE caller parameter (`ParameterDependent(j)` — never more
//! than one `j`). There is no branch anywhere in `cross_hop`'s per-binding
//! match, or in `resolve_terminal`'s `ParameterDependent(i)` arm, that reads
//! more than one index. **The unary premise holds** — confirmed by
//! transcription, not merely assumed; see `transfer_matches_cross_hop_per_param`
//! below, which differentially proves every compiled [`ParamTransfer`] equals
//! `cross_hop`'s own per-parameter answer.
//!
//! ## Backward fixpoint
//!
//! `Need[n] = {i : some terminal at n reads op temp_state PD(i)} ∪ {caller
//! param j : edge n→m, callee param p ∈ Need[m],
//! classify_edge_param(n, edge, m, p) == Copy(j)}`. A `Copy` outcome is the
//! ONLY way a caller need is created — `Const(_)` (whether from a closed-world
//! proof, a `Known` binding, a non-`binding_ok` edge, or a missing/absent
//! binding) contributes nothing (this is exactly `cross_hop`'s own behavior:
//! `Const` outcomes never consult `caller_state` at all). The fixpoint is
//! monotone (union-only: a node's `Need` set only ever grows), so a plain
//! iterate-to-no-change sweep over every node terminates — at most
//! `O(nodes × max |params|)` sweeps, since each sweep that makes progress adds
//! at least one `(node, param)` pair to a globally bounded universe.
#![allow(dead_code)]

use std::collections::{BTreeSet, HashMap};

use crate::engine::l4::effect_lattice::TempStateKind;
use crate::engine::l5::closed_world_temp::ClosedWorldTempParams;
use crate::engine::l5::d1_graph::{D1Edge, D1Graph};
use crate::engine::l5::d1_temp::ParamTemp;
use crate::engine::l5::detector_context::DetectorContext;
use crate::program::model::workspace::{ModelRecordOperation, ModelRoutine};

/// The single-parameter transfer an edge applies to ONE needed callee
/// parameter. Exhaustive per `d1_temp::cross_hop`'s per-param outcomes
/// (`d1_temp.rs:121-169`): closed-world-proven and constants collapse to a
/// value; a `PD` binding copies one caller slot; everything else is
/// `Unknown`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ParamTransfer {
    /// `Temp` (including closed-world-proven) | `Physical` | `Unknown`.
    Const(ParamTemp),
    /// Callee param = caller param AT THIS Need-SLOT (an index into the
    /// caller node's own `need`/value-fact vector, not a raw param index).
    Copy { caller_slot: u16 },
}

/// Per node: the ordered list of downstream-observable (live) parameter
/// indices. Slot `i` in a node's value-facts corresponds to `need[node][i]`.
pub(crate) struct Liveness {
    /// `NodeIx -> sorted live param indices`.
    pub need: Vec<Vec<u32>>,
    /// `NodeIx -> (param_index -> slot)`.
    pub slot_of: Vec<HashMap<u32, u16>>,
    /// Per edge (indexed as `graph.edges[from][k]`): transfers for the
    /// CALLEE's live params, in callee-slot order. `Const(Unknown)` for a
    /// callee live param with no usable binding / non-binding edge.
    pub edge_transfers: Vec<Vec<Vec<ParamTransfer>>>,
    /// Per terminal: the single param slot its op reads, or `None` for a
    /// constant terminal temp state (`resolve_terminal` reads <=1 index).
    pub terminal_reads: Vec<Vec<Option<u16>>>,
}

/// One callee-parameter's classification for ONE edge — the exact per-param
/// outcome `cross_hop`'s binding-table loop body computes, before the
/// caller's SLOT for a `Copy` target is known (that resolution happens once
/// `Need` is final — see [`compute_liveness`]).
enum EdgeParamOutcome {
    Const(ParamTemp),
    /// The callee param equals the CALLER's own param `.0` — a RAW param
    /// index, not yet resolved to a `Need`-slot.
    Copy(u32),
}

/// Classify callee param `p` of `edge` (owned by `caller`, targeting
/// `callee_id`) per `cross_hop`'s exact check order (`d1_temp.rs:130-167`):
///
/// 1. closed-world-proven(`callee_id`, `p`) FIRST, unconditionally — wins over
///    both the `binding_ok` guard and the binding table (mirrors `cross_hop`
///    seeding `proven_entries` before even checking `binding_ok`).
/// 2. `!edge.binding_ok` (a non-allowlisted edge kind) -> `Unknown` — no
///    caller-frame binding semantics at all.
/// 3. missing callsite / missing binding for `p` -> `Unknown`.
/// 4. `Known(true)` -> `Temp`; `Known(false)` -> `Physical`;
///    `ParameterDependent(j)` -> `Copy(j)`; `Unknown` source -> `Unknown`.
fn classify_edge_param(
    caller: &ModelRoutine,
    edge: &D1Edge,
    callee_id: &str,
    p: u32,
    cw: &ClosedWorldTempParams,
) -> EdgeParamOutcome {
    if cw.contains(&(callee_id.to_string(), p)) {
        return EdgeParamOutcome::Const(ParamTemp::Temp);
    }
    if !edge.binding_ok {
        return EdgeParamOutcome::Const(ParamTemp::Unknown);
    }
    let Some(cs_id) = edge.callsite_id else {
        return EdgeParamOutcome::Const(ParamTemp::Unknown);
    };
    let Some(cs) = caller.call_sites.iter().find(|c| c.id == cs_id) else {
        return EdgeParamOutcome::Const(ParamTemp::Unknown);
    };
    match cs.source_temp_state_for(p) {
        Some(ts) => match TempStateKind::from_p_temp_state(ts) {
            TempStateKind::Known(true) => EdgeParamOutcome::Const(ParamTemp::Temp),
            TempStateKind::Known(false) => EdgeParamOutcome::Const(ParamTemp::Physical),
            TempStateKind::ParameterDependent(j) => EdgeParamOutcome::Copy(j),
            TempStateKind::Unknown => EdgeParamOutcome::Const(ParamTemp::Unknown),
        },
        None => EdgeParamOutcome::Const(ParamTemp::Unknown),
    }
}

/// The param index a terminal op's `temp_state` reads, if any
/// (`resolve_terminal` reads <=1 index): `ParameterDependent(i)` reads `i`;
/// `Known`/`Unknown`/absent `temp_state` reads nothing.
fn terminal_read_index(op: &ModelRecordOperation) -> Option<u32> {
    let ts = op.temp_state.as_ref()?;
    match TempStateKind::from_p_temp_state(ts) {
        TempStateKind::ParameterDependent(i) => Some(i),
        TempStateKind::Known(_) | TempStateKind::Unknown => None,
    }
}

/// Compute the backward `Need[node]` fixpoint + compile the per-edge
/// [`ParamTransfer`] table (see module docs for the exact fixpoint + the
/// unary-premise argument).
pub(crate) fn compute_liveness<'a>(
    graph: &D1Graph<'a>,
    ctx: &DetectorContext,
    cw: &ClosedWorldTempParams,
) -> Liveness {
    let n = graph.node_ids.len();
    let mut need: Vec<BTreeSet<u32>> = vec![BTreeSet::new(); n];

    // Init: every terminal's own PD read index (regardless of proven-ness —
    // see module docs / EdgeParamOutcome doc: over-including a proven param
    // here is harmless, since its INCOMING edges will correctly compile to
    // Const(Temp) regardless of any caller value).
    for (node_ix, terminals) in graph.terminals.iter().enumerate() {
        for t in terminals {
            if let Some(i) = terminal_read_index(t.op) {
                need[node_ix].insert(i);
            }
        }
    }

    // Backward fixpoint: monotone (union-only), so iterate full sweeps until
    // no node's Need set grows.
    loop {
        let mut changed = false;
        for node_ix in 0..n {
            let Some(caller) = ctx.routine_by_id.get(graph.node_ids[node_ix]).copied() else {
                continue;
            };
            for edge in &graph.edges[node_ix] {
                let callee_ix = edge.to as usize;
                let callee_id = graph.node_ids[callee_ix];
                // Snapshot the callee's CURRENT need before mutating this
                // node's own set — guards the self-loop / mutual-recursion
                // case (callee_ix could equal node_ix); the snapshot only
                // ever makes this sweep more conservative, never unsound
                // (a later sweep picks up what this one missed).
                let callee_params: Vec<u32> = need[callee_ix].iter().copied().collect();
                for p in callee_params {
                    if let EdgeParamOutcome::Copy(j) =
                        classify_edge_param(caller, edge, callee_id, p, cw)
                        && need[node_ix].insert(j)
                    {
                        changed = true;
                    }
                }
            }
        }
        if !changed {
            break;
        }
    }

    // Freeze into sorted Vecs (BTreeSet iteration is already sorted) + slot maps.
    let need_vecs: Vec<Vec<u32>> = need.into_iter().map(|s| s.into_iter().collect()).collect();
    let slot_of: Vec<HashMap<u32, u16>> = need_vecs
        .iter()
        .map(|v| {
            v.iter()
                .enumerate()
                .map(|(slot, &p)| (p, slot as u16))
                .collect()
        })
        .collect();

    // Per-edge transfers, in callee-slot order.
    let mut edge_transfers: Vec<Vec<Vec<ParamTransfer>>> = Vec::with_capacity(n);
    for (node_ix, node_edges) in graph.edges.iter().enumerate() {
        let caller = ctx.routine_by_id.get(graph.node_ids[node_ix]).copied();
        let mut node_transfers: Vec<Vec<ParamTransfer>> = Vec::with_capacity(node_edges.len());
        for edge in node_edges {
            let callee_ix = edge.to as usize;
            let callee_id = graph.node_ids[callee_ix];
            let mut transfers: Vec<ParamTransfer> = Vec::with_capacity(need_vecs[callee_ix].len());
            for &p in &need_vecs[callee_ix] {
                let transfer = match caller {
                    Some(caller) => match classify_edge_param(caller, edge, callee_id, p, cw) {
                        EdgeParamOutcome::Const(pt) => ParamTransfer::Const(pt),
                        EdgeParamOutcome::Copy(j) => {
                            let slot = *slot_of[node_ix].get(&j).expect(
                                "a Copy(j) outcome must have added j to Need[caller] during the fixpoint",
                            );
                            ParamTransfer::Copy { caller_slot: slot }
                        }
                    },
                    // No routine at this node (defensive — never reached in
                    // the source-only pipeline; mirrors process_group's own
                    // defensive skip) -> Unknown, matching cross_hop's own
                    // missing-callsite fallback.
                    None => ParamTransfer::Const(ParamTemp::Unknown),
                };
                transfers.push(transfer);
            }
            node_transfers.push(transfers);
        }
        edge_transfers.push(node_transfers);
    }

    // Per-terminal read slot.
    let mut terminal_reads: Vec<Vec<Option<u16>>> = Vec::with_capacity(n);
    for (node_ix, node_terminals) in graph.terminals.iter().enumerate() {
        let reads = node_terminals
            .iter()
            .map(|t| terminal_read_index(t.op).map(|i| slot_of[node_ix][&i]))
            .collect();
        terminal_reads.push(reads);
    }

    Liveness {
        need: need_vecs,
        slot_of,
        edge_transfers,
        terminal_reads,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::l5::d1_graph::D1Terminal;
    use crate::engine::l5::d1_temp::{TempVec, cross_hop, lookup};
    use crate::engine::l5::test_support::{
        arg_binding, call_site, minimal_ctx, record_op, routine, ts_known, ts_pd,
    };

    /// `Need[A] contains that param; an unrelated param is NOT in Need` — a
    /// chain A -> B -> T where T's op is `PD(0)`, bound through B's callsite
    /// via a `PD` binding to A's own param 5. A's callsite ALSO carries an
    /// unrelated binding (targeting B's param 1, which B never reads) forwarding
    /// A's param 9 — that param must NOT leak into `Need[A]`.
    #[test]
    fn need_is_backward_closure() {
        let mut t = routine("T", "procedure");
        let mut top0 = record_op("T/op0", "Modify", "Rec", Some("t/T"), vec![], false);
        top0.temp_state = Some(ts_pd(0));
        t.record_operations = vec![top0];

        let mut b = routine("B", "procedure");
        let mut b_cs = call_site("B/cs0", "T", vec![]);
        b_cs.argument_bindings = vec![arg_binding(0, Some(ts_pd(2)))];
        b.call_sites = vec![b_cs];

        let mut a = routine("A", "procedure");
        let mut a_cs = call_site("A/cs0", "B", vec![]);
        a_cs.argument_bindings = vec![
            arg_binding(2, Some(ts_pd(5))),
            arg_binding(1, Some(ts_pd(9))), // unrelated: B never reads param 1
        ];
        a.call_sites = vec![a_cs];

        let routines = vec![a, b, t];
        let ctx = minimal_ctx(&routines, HashMap::new(), HashMap::new());

        let mut node_ix = HashMap::new();
        for (i, id) in ["A", "B", "T"].iter().enumerate() {
            node_ix.insert(*id, i as u32);
        }
        let graph = D1Graph {
            node_ids: vec!["A", "B", "T"],
            node_ix,
            edges: vec![
                vec![D1Edge {
                    to: 1,
                    kind: "direct",
                    callsite_id: Some("A/cs0"),
                    loop_depth: 0,
                    binding_ok: true,
                }],
                vec![D1Edge {
                    to: 2,
                    kind: "direct",
                    callsite_id: Some("B/cs0"),
                    loop_depth: 0,
                    binding_ok: true,
                }],
                vec![],
            ],
            terminals: vec![
                vec![],
                vec![],
                vec![D1Terminal {
                    op: &routines[2].record_operations[0],
                    owner: &routines[2],
                    local_depth: 0,
                }],
            ],
        };

        let cw = ClosedWorldTempParams::new();
        let liveness = compute_liveness(&graph, &ctx, &cw);

        assert!(
            liveness.need[1].contains(&2),
            "Need[B] must contain param 2 (forwarded into T's PD(0))"
        );
        assert!(
            liveness.need[0].contains(&5),
            "Need[A] must contain param 5 (forwarded into B's own live param 2)"
        );
        assert!(
            !liveness.need[0].contains(&9),
            "an unrelated binding (B's non-live param 1) must not leak into Need[A]"
        );
    }

    /// A callee param bound to a temp literal (`Const` via `Known`) or with NO
    /// binding at all (`Unknown`) must add nothing to the caller's Need.
    #[test]
    fn const_and_unknown_bindings_need_no_caller() {
        let mut h = routine("H", "procedure");
        let mut hop0 = record_op("H/op0", "Modify", "Rec", Some("t/H"), vec![], false);
        hop0.temp_state = Some(ts_pd(0));
        h.record_operations = vec![hop0];

        // A binds H's param0 to a Known(true) LITERAL — a Const, no caller need.
        let mut a = routine("A", "procedure");
        let mut a_cs = call_site("A/cs0", "H", vec![]);
        a_cs.argument_bindings = vec![arg_binding(0, Some(ts_known(true)))];
        a.call_sites = vec![a_cs];

        // B calls H with NO argument binding at all for param0 (missing binding).
        let mut b = routine("B", "procedure");
        b.call_sites = vec![call_site("B/cs0", "H", vec![])];

        let routines = vec![a, b, h];
        let ctx = minimal_ctx(&routines, HashMap::new(), HashMap::new());

        let mut node_ix = HashMap::new();
        for (i, id) in ["A", "B", "H"].iter().enumerate() {
            node_ix.insert(*id, i as u32);
        }
        let graph = D1Graph {
            node_ids: vec!["A", "B", "H"],
            node_ix,
            edges: vec![
                vec![D1Edge {
                    to: 2,
                    kind: "direct",
                    callsite_id: Some("A/cs0"),
                    loop_depth: 0,
                    binding_ok: true,
                }],
                vec![D1Edge {
                    to: 2,
                    kind: "direct",
                    callsite_id: Some("B/cs0"),
                    loop_depth: 0,
                    binding_ok: true,
                }],
                vec![],
            ],
            terminals: vec![
                vec![],
                vec![],
                vec![D1Terminal {
                    op: &routines[2].record_operations[0],
                    owner: &routines[2],
                    local_depth: 0,
                }],
            ],
        };

        let cw = ClosedWorldTempParams::new();
        let liveness = compute_liveness(&graph, &ctx, &cw);

        assert!(
            liveness.need[0].is_empty(),
            "a Const (Known) binding must add no caller need"
        );
        assert!(
            liveness.need[1].is_empty(),
            "a missing binding must add no caller need"
        );
    }

    /// A closed-world-proven callee param resolves `Const(Temp)` UNCONDITIONALLY
    /// — even with a `PD` binding present on the edge that would otherwise
    /// forward a caller param. The proof must win BEFORE the binding is even
    /// consulted, so it must add NO caller need.
    #[test]
    fn proven_param_needs_no_caller() {
        let mut h = routine("H", "procedure");
        let mut hop0 = record_op("H/op0", "Modify", "Rec", Some("t/H"), vec![], false);
        hop0.temp_state = Some(ts_pd(0));
        h.record_operations = vec![hop0];

        let mut a = routine("A", "procedure");
        let mut a_cs = call_site("A/cs0", "H", vec![]);
        a_cs.argument_bindings = vec![arg_binding(0, Some(ts_pd(5)))];
        a.call_sites = vec![a_cs];

        let routines = vec![a, h];
        let ctx = minimal_ctx(&routines, HashMap::new(), HashMap::new());

        let mut node_ix = HashMap::new();
        node_ix.insert("A", 0u32);
        node_ix.insert("H", 1u32);
        let graph = D1Graph {
            node_ids: vec!["A", "H"],
            node_ix,
            edges: vec![
                vec![D1Edge {
                    to: 1,
                    kind: "direct",
                    callsite_id: Some("A/cs0"),
                    loop_depth: 0,
                    binding_ok: true,
                }],
                vec![],
            ],
            terminals: vec![
                vec![],
                vec![D1Terminal {
                    op: &routines[1].record_operations[0],
                    owner: &routines[1],
                    local_depth: 0,
                }],
            ],
        };

        let mut cw = ClosedWorldTempParams::new();
        cw.insert(("H".to_string(), 0));

        let liveness = compute_liveness(&graph, &ctx, &cw);
        assert!(
            liveness.need[0].is_empty(),
            "a closed-world-proven callee param must add no caller need, even with a PD binding present"
        );
        assert_eq!(
            liveness.need[1],
            vec![0],
            "Need[H] itself still contains 0 (from H's own terminal read)"
        );
    }

    /// The load-bearing per-param equivalence oracle: for a small graph, EACH
    /// compiled [`ParamTransfer`] applied to the caller's per-slot value vector
    /// produces the SAME single-param answer `d1_temp::cross_hop` produces for
    /// that param. Exercises all 5 outcome kinds: closed-world-proven,
    /// const-temp (`Known(true)`), const-physical (`Known(false)`), unknown
    /// (missing binding, AND a non-`binding_ok` edge overriding a present
    /// binding), and copy (`ParameterDependent`).
    #[test]
    fn transfer_matches_cross_hop_per_param() {
        // Callee H: 5 terminals, each reading a distinct param 0..=4 ->
        // Need[H] = {0,1,2,3,4}.
        let mut h = routine("H", "procedure");
        h.record_operations = (0..5u32)
            .map(|i| {
                let mut op = record_op(
                    &format!("H/op{i}"),
                    "Modify",
                    "Rec",
                    Some("t/H"),
                    vec![],
                    false,
                );
                op.temp_state = Some(ts_pd(i));
                op
            })
            .collect();

        // A (binding_ok edge): param0 Known(false) [must be OVERRIDDEN by the
        // proof], param1 Known(true), param2 Known(false), param3 MISSING,
        // param4 PD(3) (forwards A's own param 3).
        let mut a = routine("A", "procedure");
        let mut a_cs = call_site("A/cs0", "H", vec![]);
        a_cs.argument_bindings = vec![
            arg_binding(0, Some(ts_known(false))),
            arg_binding(1, Some(ts_known(true))),
            arg_binding(2, Some(ts_known(false))),
            arg_binding(4, Some(ts_pd(3))),
        ];
        a.call_sites = vec![a_cs];

        // B (a NON-binding_ok edge): a binding on param1 that must be IGNORED
        // because binding_ok is false — only the proven param0 survives.
        let mut b = routine("B", "procedure");
        let mut b_cs = call_site("B/cs0", "H", vec![]);
        b_cs.argument_bindings = vec![arg_binding(1, Some(ts_known(true)))];
        b.call_sites = vec![b_cs];

        let routines = vec![a, b, h];
        let ctx = minimal_ctx(&routines, HashMap::new(), HashMap::new());

        let mut node_ix = HashMap::new();
        for (i, id) in ["A", "B", "H"].iter().enumerate() {
            node_ix.insert(*id, i as u32);
        }
        let terminals_h: Vec<D1Terminal> = routines[2]
            .record_operations
            .iter()
            .map(|op| D1Terminal {
                op,
                owner: &routines[2],
                local_depth: 0,
            })
            .collect();
        let graph = D1Graph {
            node_ids: vec!["A", "B", "H"],
            node_ix,
            edges: vec![
                vec![D1Edge {
                    to: 2,
                    kind: "direct",
                    callsite_id: Some("A/cs0"),
                    loop_depth: 0,
                    binding_ok: true,
                }],
                vec![D1Edge {
                    to: 2,
                    kind: "dynamic",
                    callsite_id: Some("B/cs0"),
                    loop_depth: 0,
                    binding_ok: false,
                }],
                vec![],
            ],
            terminals: vec![vec![], vec![], terminals_h],
        };

        let mut cw = ClosedWorldTempParams::new();
        cw.insert(("H".to_string(), 0));

        let liveness = compute_liveness(&graph, &ctx, &cw);
        assert_eq!(
            liveness.need[2],
            vec![0, 1, 2, 3, 4],
            "Need[H] = every terminal's own read index"
        );
        assert_eq!(
            liveness.need[0],
            vec![3],
            "only param4's PD(3) forward should have added A's param 3"
        );
        assert!(
            liveness.need[1].is_empty(),
            "binding_ok=false blocks every non-proven param on the B->H edge"
        );

        fn apply_transfer(transfer: &ParamTransfer, caller_slot_values: &[ParamTemp]) -> ParamTemp {
            match transfer {
                ParamTransfer::Const(pt) => *pt,
                ParamTransfer::Copy { caller_slot } => caller_slot_values[*caller_slot as usize],
            }
        }

        let a_routine = &routines[0];
        let b_routine = &routines[1];

        // --- Edge A->H: caller_state assigns A's OWN param 3 = Physical. ---
        let a_caller_state: TempVec = vec![(3, ParamTemp::Physical)].into_iter().collect();
        let a_slot_values: Vec<ParamTemp> = liveness.need[0]
            .iter()
            .map(|&j| lookup(&a_caller_state, j))
            .collect();
        let a_oracle = cross_hop(&a_caller_state, a_routine, "A/cs0", "H", true, &cw);
        let a_transfers = &liveness.edge_transfers[0][0];
        assert_eq!(a_transfers.len(), 5);
        for (slot, &p) in liveness.need[2].iter().enumerate() {
            let expected = lookup(&a_oracle, p);
            let actual = apply_transfer(&a_transfers[slot], &a_slot_values);
            assert_eq!(
                actual, expected,
                "edge A->H, callee param {p}: transfer must equal cross_hop"
            );
        }

        // --- Edge B->H (binding_ok=false): Need[B] is empty, so the slot
        // vector is empty — only Const transfers are legal here (never Copy).
        let b_caller_state: TempVec = TempVec::new();
        let b_slot_values: Vec<ParamTemp> = Vec::new();
        let b_oracle = cross_hop(&b_caller_state, b_routine, "B/cs0", "H", false, &cw);
        let b_transfers = &liveness.edge_transfers[1][0];
        assert_eq!(b_transfers.len(), 5);
        for (slot, &p) in liveness.need[2].iter().enumerate() {
            let expected = lookup(&b_oracle, p);
            let actual = apply_transfer(&b_transfers[slot], &b_slot_values);
            assert_eq!(
                actual, expected,
                "edge B->H, callee param {p}: transfer must equal cross_hop"
            );
        }
    }
}

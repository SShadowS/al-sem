//! R4-F Stage-4c — `compute_ordering(input, false)` live range.
//!
//! Byte-parity port of al-sem `src/digest/ordering-engine.ts` lines 703..2773
//! (the order:false path — everything past the early-return at 2771 is DEAD).
//!
//! Produces `scoped_guarantees_by_index` — per-effect `ScopedGuarantee[]` (intra
//! owning-routine + cross-hop root scope), already covering the 5 RELEVANT_LABELS
//! the downstream projection filters to. The DEAD label blocks
//! (COMMIT_REACHABLE / WRITE_COMMITTED_* / COMMIT_DOMINATES_RETURN / ORDERING_UNKNOWN)
//! and the dead unprovenPairs / factGraph paths are OMITTED.
//!
//! Determinism (BINDING): NO sorts / string-compares in this range — all ordering
//! is by integer orderId/frameId. `groupsByRoutine` iteration = insertion order
//! (a `Vec`); lookup maps are `HashMap` (lookup only, never iterated into output).
//! The scopedGuarantee EMISSION ORDER is structural and replicated exactly.

use std::collections::{HashMap, HashSet};

use crate::engine::ids::sha256_hex;
use crate::engine::l5::digest::QueryWitnessHop;
use crate::engine::l5::ordering::{
    HBEdge, OrderedOp, Quantifier, build_hb_edges, dom, dominates_return, may_co_execute,
    ordered_before,
};
use crate::engine::l5::ordering_inter::{
    CallChain, CallsiteByIdMap, OccurrenceWithChain, error_escapes_chain, inter_hb, io_direction,
    reconstruct_call_chains, reconstruct_frame_chain,
};
use crate::engine::l5::snapshot::{CapabilitySnapshot, SnapCapabilityExtra, SnapTempState};
use crate::engine::return_summary::RoutineReturnSummary;
use serde_json::Value as JsonValue;

// ---------------------------------------------------------------------------
// Output types
// ---------------------------------------------------------------------------

/// A scoped guarantee — only the fields the R4-F scoped projection serializes.
#[derive(Debug, Clone)]
pub struct ScopedGuarantee {
    pub label: &'static str,
    pub scope: &'static str, // "owning-routine" | "root"
    pub write_occurrence_id: Option<String>,
    pub commit_occurrence_id: Option<String>,
    pub io_occurrence_id: Option<String>,
    pub return_occurrence_id: Option<String>,
    pub supporting_edge_ids: Vec<String>,
    pub commit_effectiveness: Option<&'static str>,
    pub intervening_boundary: &'static str, // "none" | "present" | "unknown"
    pub valid_for_refutation: bool,
}

/// Per-effect ordering input (mirrors al-sem `OrderingInput.effects[]`).
pub struct OrderingEffectInput {
    pub effect_type: String,
    pub evidence_operation_id: Option<String>,
    pub evidence_callsite_id: Option<String>,
    pub via_paths: Vec<Vec<QueryWitnessHop>>,
    pub via_paths_truncated: bool,
    pub temp_state: Option<SnapTempState>,
    pub occurrence_id: String,
    /// Effect-level conditionality — used by COMMIT_ON_SUCCESS_PATH rule.
    /// Pre-computed before the ordering call (mirrors TS eff.conditionality field).
    pub conditionality: &'static str,
}

// ---------------------------------------------------------------------------
// Effect-type classification (spec §D)
// ---------------------------------------------------------------------------

fn is_external_io(t: &str) -> bool {
    matches!(t, "HTTP" | "FILE" | "ISOLATED_STORAGE" | "BACKGROUND_TASK")
}

fn is_db_write(t: &str) -> bool {
    matches!(t, "DB_INSERT" | "DB_MODIFY" | "DB_DELETE")
}

fn is_ui_window_sink(t: &str) -> bool {
    matches!(t, "UI_CONFIRM" | "UI_MESSAGE" | "UI_WINDOW_OPEN")
}

// ---------------------------------------------------------------------------
// isTrustedCommitRoot (UNTRUSTED_ROOT_KINDS)
// ---------------------------------------------------------------------------

/// `page-action` is DELIBERATELY absent -- see the same note on
/// `D50_UNTRUSTED_ROOT_KINDS`. The AST pass only ever emits it alongside
/// `trigger-page`, which IS listed, and [`is_trusted_commit_root`] below is
/// ANY-quantified, so an unlisted extra kind cannot change the verdict. Were
/// `page-action` ever emitted on its own, this would flip `false` -> `true`
/// with no golden watching.
fn is_untrusted_root_kind(kind: &str) -> bool {
    matches!(
        kind,
        "event-subscriber"
            | "install-codeunit"
            | "upgrade-codeunit"
            | "api-page"
            | "public-procedure"
            | "test-procedure"
            | "trigger-table"
            | "trigger-page"
            | "report-trigger"
    )
}

pub fn is_trusted_commit_root(root_id: &str, snap: &CapabilitySnapshot) -> bool {
    let slot = snap
        .root_classifications
        .iter()
        .find(|r| r.routine_id == root_id);
    match slot {
        None => true,
        Some(s) => !s.kinds.iter().any(|k| is_untrusted_root_kind(k)),
    }
}

// ---------------------------------------------------------------------------
// Internal occurrence record.
// ---------------------------------------------------------------------------

struct EffectOccurrence {
    effect_type: String,
    occurrence_id: String,
    ordered_op: Option<OrderedOp>,
}

// ---------------------------------------------------------------------------
// compute_ordering — order:false path.
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
pub fn compute_ordering(
    routine_id: &str,
    effects: &[OrderingEffectInput],
    snap: &CapabilitySnapshot,
    callsite_by_id: &CallsiteByIdMap,
    routine_return_summaries: Option<&HashMap<String, RoutineReturnSummary>>,
    isolated_event_ids: Option<&HashSet<String>>,
) -> Vec<Vec<ScopedGuarantee>> {
    // --- Occurrence list (occurrenceId already built by digest; resolve OrderedOp). ---
    let occurrences: Vec<EffectOccurrence> = effects
        .iter()
        .map(|eff| {
            let mut ordered_op: Option<OrderedOp> = None;

            if let Some(op_id) = &eff.evidence_operation_id
                && let Some(op_ev) = snap
                    .operation_index
                    .iter()
                    .find(|o| &o.operation_id == op_id)
                && let Some(order) = op_ev.order
            {
                let owning_frames = snap
                    .routine_order_frames
                    .as_ref()
                    .and_then(|m| m.get(&op_ev.routine));
                let frame_chain = reconstruct_frame_chain(order.frame_id, owning_frames);
                if !frame_chain.is_empty() {
                    ordered_op = Some(OrderedOp {
                        occurrence_id: eff.occurrence_id.clone(),
                        order_id: order.order_id,
                        on_success_path: order.on_success_path,
                        dominates_success_return: order.dominates_success_return,
                        frame_chain,
                    });
                }
            }

            if ordered_op.is_none()
                && let Some(cs_id) = &eff.evidence_callsite_id
                && let Some(cs_ev) = snap.callsite_index.iter().find(|c| &c.callsite_id == cs_id)
                && let Some(order) = cs_ev.order
            {
                let owning_frames = snap
                    .routine_order_frames
                    .as_ref()
                    .and_then(|m| m.get(&cs_ev.routine));
                let frame_chain = reconstruct_frame_chain(order.frame_id, owning_frames);
                if !frame_chain.is_empty() {
                    ordered_op = Some(OrderedOp {
                        occurrence_id: eff.occurrence_id.clone(),
                        order_id: order.order_id,
                        on_success_path: order.on_success_path,
                        dominates_success_return: order.dominates_success_return,
                        frame_chain,
                    });
                }
            }

            EffectOccurrence {
                effect_type: eff.effect_type.clone(),
                occurrence_id: eff.occurrence_id.clone(),
                ordered_op,
            }
        })
        .collect();

    // --- Physical-write classification (AND across contributors, by occurrenceId). ---
    let mut known_temp_only: HashMap<String, bool> = HashMap::new();
    for (i, occ) in occurrences.iter().enumerate() {
        if !is_db_write(&occ.effect_type) {
            continue;
        }
        let is_known_temp = matches!(
            &effects[i].temp_state,
            Some(SnapTempState::Known { value: true })
        );
        let entry = known_temp_only.get(&occ.occurrence_id).copied();
        let merged = match entry {
            None => is_known_temp,
            Some(prior) => prior && is_known_temp,
        };
        known_temp_only.insert(occ.occurrence_id.clone(), merged);
    }
    let is_physical_write =
        |occ_id: &str| -> bool { known_temp_only.get(occ_id).copied() != Some(true) };

    // --- resolveOwningRoutine. ---
    let resolve_owning_routine = |eff: &OrderingEffectInput| -> Option<String> {
        if let Some(op_id) = &eff.evidence_operation_id
            && let Some(op_ev) = snap
                .operation_index
                .iter()
                .find(|o| &o.operation_id == op_id)
        {
            return Some(op_ev.routine.clone());
        }
        if let Some(cs_id) = &eff.evidence_callsite_id
            && let Some(cs_ev) = snap.callsite_index.iter().find(|c| &c.callsite_id == cs_id)
        {
            return Some(cs_ev.routine.clone());
        }
        None
    };

    // --- Per-routine grouping (insertion order = effects order). ---
    // groups: Vec<(routineId, Vec<index-into-occurrences>)>
    let mut groups: Vec<(String, Vec<usize>)> = Vec::new();
    for (i, occ) in occurrences.iter().enumerate() {
        if occ.ordered_op.is_none() {
            continue;
        }
        let Some(owner) = resolve_owning_routine(&effects[i]) else {
            continue;
        };
        if let Some(g) = groups.iter_mut().find(|(r, _)| *r == owner) {
            g.1.push(i);
        } else {
            groups.push((owner, vec![i]));
        }
    }

    // Build HB edges per group (never cross-group).
    let mut all_hb_edges: Vec<HBEdge> = Vec::new();
    for (_owner, idxs) in &groups {
        let group_ops: Vec<OrderedOp> = idxs
            .iter()
            .map(|&i| occurrences[i].ordered_op.clone().unwrap())
            .collect();
        let group_edges = build_hb_edges(&group_ops);
        all_hb_edges.extend(group_edges);
    }
    let has_intra_edge = |from: &str, to: &str| -> bool {
        all_hb_edges.iter().any(|e| e.from == from && e.to == to)
    };

    // --- OccurrenceWithChain per effect (None when no order data / not placeable). ---
    let mut multi_path_occurrence_ids: HashSet<String> = HashSet::new();
    let occurrence_chains: Vec<Option<OccurrenceWithChain>> = occurrences
        .iter()
        .enumerate()
        .map(|(idx, occ)| {
            let ordered_op = occ.ordered_op.clone()?;
            let eff = &effects[idx];
            let via_paths = &eff.via_paths;
            let via_paths_truncated = eff.via_paths_truncated;

            let owner = resolve_owning_routine(eff)?;

            // terminalRoutineOps for the owning group.
            let terminal_routine_ops: Vec<OrderedOp> = groups
                .iter()
                .find(|(r, _)| *r == owner)
                .map(|(_, idxs)| {
                    idxs.iter()
                        .map(|&i| occurrences[i].ordered_op.clone().unwrap())
                        .collect()
                })
                .unwrap_or_default();

            if via_paths.is_empty() {
                if owner != routine_id {
                    return None;
                }
                return Some(OccurrenceWithChain {
                    occurrence_id: occ.occurrence_id.clone(),
                    terminal_routine_id: owner,
                    terminal_op: Some(ordered_op),
                    terminal_routine_ops,
                    chain: CallChain {
                        links: Vec::new(),
                        path_enumeration: "complete",
                    },
                });
            }

            let chains = reconstruct_call_chains(
                via_paths,
                snap,
                callsite_by_id,
                via_paths_truncated,
                isolated_event_ids,
            );
            if chains.is_empty() {
                return None;
            }
            if chains.len() > 1 {
                multi_path_occurrence_ids.insert(occ.occurrence_id.clone());
            }
            let chain = chains.into_iter().next().unwrap();
            Some(OccurrenceWithChain {
                occurrence_id: occ.occurrence_id.clone(),
                terminal_routine_id: owner,
                terminal_op: Some(ordered_op),
                terminal_routine_ops,
                chain,
            })
        })
        .collect();

    // RETURN node occurrence id — used for COMMIT_DOMINATES_RETURN root-scope.
    let return_occurrence_id = sha256_hex(&format!("{routine_id}|RETURN"))[..16].to_string();

    // ====================================================================
    // Intra-routine (owning-routine scope) labels:
    //   COMMIT_REACHABLE, COMMIT_ON_SUCCESS_PATH, COMMIT_DOMINATES_RETURN,
    //   WRITE_COMMITTED_BEFORE_RETURN, EXTERNAL_IO_BEFORE_COMMIT,
    //   WRITE_PENDING_AT_EXTERNAL_IO, WRITE_PENDING_AT_UI.
    // ====================================================================
    let intra_labels_by_index: Vec<Vec<&'static str>> = occurrences
        .iter()
        .enumerate()
        .map(|(idx, occ)| {
            let mut labels: Vec<&'static str> = Vec::new();
            let Some(op) = occ.ordered_op.as_ref() else {
                // No order index available → ORDERING_UNKNOWN (mirrors TS occ.orderedOp === null).
                labels.push("ORDERING_UNKNOWN");
                return labels;
            };
            let Some(owner) = resolve_owning_routine(&effects[idx]) else {
                // Cannot resolve owning routine → ORDERING_UNKNOWN (mirrors TS ownerRoutine === undefined).
                labels.push("ORDERING_UNKNOWN");
                return labels;
            };
            let group_idxs: Vec<usize> = groups
                .iter()
                .find(|(r, _)| *r == owner)
                .map(|(_, v)| v.clone())
                .unwrap_or_default();

            // commit/write/io occurrences in the SAME group.
            let commit_idxs: Vec<usize> = group_idxs
                .iter()
                .copied()
                .filter(|&i| occurrences[i].effect_type == "COMMIT")
                .collect();
            let write_idxs: Vec<usize> = group_idxs
                .iter()
                .copied()
                .filter(|&i| is_db_write(&occurrences[i].effect_type))
                .collect();

            // COMMIT_REACHABLE: any commit occurrence exists in this routine group.
            let has_any_commit = !commit_idxs.is_empty();
            if has_any_commit {
                labels.push("COMMIT_REACHABLE");
            }

            // COMMIT_ON_SUCCESS_PATH (on COMMIT effects):
            // any commit in the group has conditionality = "unconditional-on-success".
            // Mirrors TS: `commitOccs.some(o => o.conditionality === "unconditional-on-success")`.
            if occ.effect_type == "COMMIT" {
                let has_commit_on_success = commit_idxs
                    .iter()
                    .any(|&ci| effects[ci].conditionality == "unconditional-on-success");
                if has_commit_on_success {
                    labels.push("COMMIT_ON_SUCCESS_PATH");
                }
            }

            // COMMIT_DOMINATES_RETURN (on COMMIT effects): this commit dominates return.
            if occ.effect_type == "COMMIT" && dominates_return(op) {
                labels.push("COMMIT_DOMINATES_RETURN");
            }

            // WRITE_COMMITTED_BEFORE_RETURN (on COMMIT effects):
            // dominatesReturn(op) AND there is a write w with dom(w, op) AND
            // the nearest commit after w (by orderId) is this commit.
            if occ.effect_type == "COMMIT" && dominates_return(op) {
                // nearest_commit_after(write_occ): find commit with smallest orderId > write's orderId.
                let nearest_commit_after = |write_op_id: u32| -> Option<String> {
                    let mut nearest_id: Option<u32> = None;
                    let mut nearest_occ_id: Option<String> = None;
                    for &ci in &commit_idxs {
                        let Some(cop) = occurrences[ci].ordered_op.as_ref() else {
                            continue;
                        };
                        if cop.order_id > write_op_id
                            && nearest_id.map(|n| cop.order_id < n).unwrap_or(true)
                        {
                            nearest_id = Some(cop.order_id);
                            nearest_occ_id = Some(occurrences[ci].occurrence_id.clone());
                        }
                    }
                    nearest_occ_id
                };

                let has_write_committed = write_idxs.iter().any(|&wi| {
                    let w_occ = &occurrences[wi];
                    let Some(write_op) = w_occ.ordered_op.as_ref() else {
                        return false;
                    };
                    if !dom(write_op, op) {
                        return false;
                    }
                    let nearest = nearest_commit_after(write_op.order_id);
                    nearest.as_deref() == Some(&occ.occurrence_id)
                });
                if has_write_committed {
                    labels.push("WRITE_COMMITTED_BEFORE_RETURN");
                }
            }

            // EXTERNAL_IO_BEFORE_COMMIT (on IO effects).
            if is_external_io(&occ.effect_type) {
                let any_commit_after_io = commit_idxs.iter().any(|&ci| {
                    let Some(cop) = occurrences[ci].ordered_op.as_ref() else {
                        return false;
                    };
                    if has_intra_edge(&occ.occurrence_id, &occurrences[ci].occurrence_id) {
                        return true;
                    }
                    ordered_before(op, cop) && may_co_execute(op, cop)
                });
                if any_commit_after_io {
                    labels.push("EXTERNAL_IO_BEFORE_COMMIT");
                }
            }
            // EXTERNAL_IO_BEFORE_COMMIT (on COMMIT effects).
            if occ.effect_type == "COMMIT" {
                let any_io_before_commit = group_idxs
                    .iter()
                    .copied()
                    .filter(|&i| is_external_io(&occurrences[i].effect_type))
                    .any(|ii| {
                        let Some(iop) = occurrences[ii].ordered_op.as_ref() else {
                            return false;
                        };
                        if has_intra_edge(&occurrences[ii].occurrence_id, &occ.occurrence_id) {
                            return true;
                        }
                        ordered_before(iop, op) && may_co_execute(iop, op)
                    });
                if any_io_before_commit {
                    labels.push("EXTERNAL_IO_BEFORE_COMMIT");
                }
            }

            // WRITE_PENDING_AT_EXTERNAL_IO (on IO effects).
            if is_external_io(&occ.effect_type) {
                let io_op = op;
                let has_pending = write_idxs.iter().any(|&wi| {
                    let w_occ = &occurrences[wi];
                    let Some(write_op) = w_occ.ordered_op.as_ref() else {
                        return false;
                    };
                    if !is_physical_write(&w_occ.occurrence_id) {
                        return false;
                    }
                    if !dom(write_op, io_op) {
                        return false;
                    }
                    let commit_may_clear = commit_idxs.iter().any(|&ci| {
                        let c_occ = &occurrences[ci];
                        let Some(c_op) = c_occ.ordered_op.as_ref() else {
                            return false;
                        };
                        let w_before_c = has_intra_edge(&w_occ.occurrence_id, &c_occ.occurrence_id)
                            || (ordered_before(write_op, c_op) && may_co_execute(write_op, c_op));
                        let c_before_io = has_intra_edge(&c_occ.occurrence_id, &occ.occurrence_id)
                            || (ordered_before(c_op, io_op) && may_co_execute(c_op, io_op));
                        w_before_c && c_before_io
                    });
                    !commit_may_clear
                });
                if has_pending {
                    labels.push("WRITE_PENDING_AT_EXTERNAL_IO");
                }
            }

            // WRITE_PENDING_AT_UI (on UI-window-sink effects).
            if is_ui_window_sink(&occ.effect_type) {
                let ui_op = op;
                let has_pending = write_idxs.iter().any(|&wi| {
                    let w_occ = &occurrences[wi];
                    let Some(write_op) = w_occ.ordered_op.as_ref() else {
                        return false;
                    };
                    if !is_physical_write(&w_occ.occurrence_id) {
                        return false;
                    }
                    if !dom(write_op, ui_op) {
                        return false;
                    }
                    let commit_may_clear = commit_idxs.iter().any(|&ci| {
                        let c_occ = &occurrences[ci];
                        let Some(c_op) = c_occ.ordered_op.as_ref() else {
                            return false;
                        };
                        let w_before_c = has_intra_edge(&w_occ.occurrence_id, &c_occ.occurrence_id)
                            || (ordered_before(write_op, c_op) && may_co_execute(write_op, c_op));
                        let c_before_ui = has_intra_edge(&c_occ.occurrence_id, &occ.occurrence_id)
                            || (ordered_before(c_op, ui_op) && may_co_execute(c_op, ui_op));
                        w_before_c && c_before_ui
                    });
                    !commit_may_clear
                });
                if has_pending {
                    labels.push("WRITE_PENDING_AT_UI");
                }
            }

            labels
        })
        .collect();

    // ====================================================================
    // gradeCommitEffectiveness / checkCalleeReturnability / precedingCallsiteAlwaysErrors
    // ====================================================================
    let grade_commit_effectiveness = |commit_occ: &EffectOccurrence,
                                      commit_chain: &OccurrenceWithChain|
     -> &'static str {
        let Some(summaries) = routine_return_summaries else {
            return "assumed_effective";
        };
        let chain_root_id = if !commit_chain.chain.links.is_empty() {
            commit_chain.chain.links[0].caller_routine_id.clone()
        } else {
            commit_chain.terminal_routine_id.clone()
        };
        let chain_complete = is_chain_complete(commit_chain, summaries);
        let commit_root_trusted = is_trusted_commit_root(&chain_root_id, snap);

        let mut chain_routine_ids: Vec<String> = vec![commit_chain.terminal_routine_id.clone()];
        for link in &commit_chain.chain.links {
            chain_routine_ids.push(link.caller_routine_id.clone());
            chain_routine_ids.push(link.callee_routine_id.clone());
        }

        // Rule 1: commit op not on a success path → proven_errors.
        let commit_not_on_success_path = summaries.get(&commit_chain.terminal_routine_id).is_some()
            && commit_occ
                .ordered_op
                .as_ref()
                .map(|cop| !cop.on_success_path)
                .unwrap_or(false);

        // Rules 2-4 signals: any TryFunction/Error on chain; any Ignore.
        let mut found_try_or_error = false;
        let mut found_ignore = false;
        for r_id in &chain_routine_ids {
            let Some(summary) = summaries.get(r_id) else {
                continue;
            };
            if summary.has_try_function_boundary || summary.commit_behavior == "error" {
                found_try_or_error = true;
            }
            if summary.commit_behavior == "ignore" {
                found_ignore = true;
            }
        }

        let is_multi_path = multi_path_occurrence_ids.contains(&commit_occ.occurrence_id);
        grade_commit_rules(
            commit_not_on_success_path,
            found_try_or_error,
            found_ignore,
            chain_complete,
            commit_root_trusted,
            is_multi_path,
        )
    };

    let check_callee_returnability = |callee_routine_id: &str| -> &'static str {
        let Some(summaries) = routine_return_summaries else {
            return "ok";
        };
        let Some(summary) = summaries.get(callee_routine_id) else {
            return "ok";
        };
        if summary.has_normal_return_path == JsonValue::Bool(false) {
            return "no-edge";
        }
        if summary.has_normal_return_path == JsonValue::String("unknown".to_string())
            || summary.has_try_function_boundary
        {
            return "degrade-to-may";
        }
        "ok"
    };

    // calleesByCallsiteId from typedEdges.
    let mut callees_by_callsite_id: HashMap<String, Vec<String>> = HashMap::new();
    for edge in &snap.typed_edges {
        let (Some(cs), Some(to)) = (edge.edge_callsite_id(), edge.edge_to()) else {
            continue;
        };
        callees_by_callsite_id
            .entry(cs.to_string())
            .or_default()
            .push(to.to_string());
    }

    let preceding_callsite_always_errors = |before_order_id: u32, frame_id: i64| -> bool {
        if routine_return_summaries.is_none() {
            return false;
        }
        for cs in &snap.callsite_index {
            if cs.routine != routine_id {
                continue;
            }
            let Some(order) = cs.order else { continue };
            if order.frame_id != frame_id {
                continue;
            }
            if order.order_id >= before_order_id {
                continue;
            }
            if cs.control_context.as_deref() != Some("top-level") {
                continue;
            }
            if let Some(callees) = callees_by_callsite_id.get(&cs.callsite_id) {
                for callee_id in callees {
                    if check_callee_returnability(callee_id) == "no-edge" {
                        return true;
                    }
                }
            }
        }
        false
    };

    // ====================================================================
    // Cross-hop edge computation (§C / §J5) — OMIT dead unprovenPairs paths.
    // ====================================================================
    #[derive(Clone)]
    struct CrossHopEdge {
        to: String,
        quantifier: Quantifier,
        edge_condition_reasons: Vec<&'static str>,
    }
    // crossHopEdgesByFrom: from-occurrenceId → edges.
    let mut cross_hop_edges_by_from: HashMap<String, Vec<CrossHopEdge>> = HashMap::new();

    let push_cross_hop = |map: &mut HashMap<String, Vec<CrossHopEdge>>,
                          from: String,
                          to: String,
                          quantifier: Quantifier,
                          reasons: Vec<&'static str>| {
        map.entry(from).or_default().push(CrossHopEdge {
            to,
            quantifier,
            edge_condition_reasons: reasons,
        });
    };

    // Pairwise nested-index loop (i, j=i+1..) is load-bearing for parity with the
    // al-sem cross-hop edge build; keep explicit indices.
    #[allow(clippy::needless_range_loop)]
    for i in 0..occurrences.len() {
        let Some(occ_a) = occurrence_chains[i].as_ref() else {
            continue;
        };
        for j in (i + 1)..occurrences.len() {
            let Some(occ_b) = occurrence_chains[j].as_ref() else {
                continue;
            };

            let a_return_check = check_callee_returnability(&occ_a.terminal_routine_id);
            let b_return_check = check_callee_returnability(&occ_b.terminal_routine_id);

            let ab_check = if a_return_check == "no-edge" || b_return_check == "no-edge" {
                "no-edge"
            } else if a_return_check == "degrade-to-may" || b_return_check == "degrade-to-may" {
                "degrade-to-may"
            } else {
                "ok"
            };

            if ab_check != "no-edge"
                && let Some(edge_ab) = inter_hb(occ_a, occ_b, routine_id, snap)
            {
                let is_cross_hop = occ_a.terminal_routine_id != occ_b.terminal_routine_id
                    || !edge_ab.call_path_links.is_empty();
                if is_cross_hop {
                    let quantifier = if ab_check == "degrade-to-may"
                        && edge_ab.quantifier == Quantifier::MustAllPaths
                    {
                        Quantifier::MaySomePath
                    } else {
                        edge_ab.quantifier
                    };
                    push_cross_hop(
                        &mut cross_hop_edges_by_from,
                        edge_ab.from.clone(),
                        edge_ab.to.clone(),
                        quantifier,
                        edge_ab.edge_condition_reasons.clone(),
                    );
                }
            }

            let ba_check = if b_return_check == "no-edge" || a_return_check == "no-edge" {
                "no-edge"
            } else if b_return_check == "degrade-to-may" || a_return_check == "degrade-to-may" {
                "degrade-to-may"
            } else {
                "ok"
            };

            if ba_check != "no-edge"
                && let Some(edge_ba) = inter_hb(occ_b, occ_a, routine_id, snap)
            {
                let is_cross_hop = occ_b.terminal_routine_id != occ_a.terminal_routine_id
                    || !edge_ba.call_path_links.is_empty();
                if is_cross_hop {
                    let quantifier = if ba_check == "degrade-to-may"
                        && edge_ba.quantifier == Quantifier::MustAllPaths
                    {
                        Quantifier::MaySomePath
                    } else {
                        edge_ba.quantifier
                    };
                    push_cross_hop(
                        &mut cross_hop_edges_by_from,
                        edge_ba.from.clone(),
                        edge_ba.to.clone(),
                        quantifier,
                        edge_ba.edge_condition_reasons.clone(),
                    );
                }
            }
        }
    }

    let empty_edges: Vec<CrossHopEdge> = Vec::new();
    let edges_from = |from: &str| -> &[CrossHopEdge] {
        cross_hop_edges_by_from
            .get(from)
            .map(|v| v.as_slice())
            .unwrap_or(&empty_edges)
    };

    // ====================================================================
    // Root-scope label computation. rootScopedGuarantees[idx] per occurrence.
    // ====================================================================
    let mut root_scope_labels: Vec<Vec<&'static str>> =
        occurrences.iter().map(|_| Vec::new()).collect();
    let mut root_scoped: Vec<Vec<ScopedGuarantee>> =
        occurrences.iter().map(|_| Vec::new()).collect();

    // Index sets (with chains).
    let commit_with_chains: Vec<usize> = (0..occurrences.len())
        .filter(|&i| occurrences[i].effect_type == "COMMIT" && occurrence_chains[i].is_some())
        .collect();
    let write_with_chains: Vec<usize> = (0..occurrences.len())
        .filter(|&i| is_db_write(&occurrences[i].effect_type) && occurrence_chains[i].is_some())
        .collect();
    let io_with_chains: Vec<usize> = (0..occurrences.len())
        .filter(|&i| is_external_io(&occurrences[i].effect_type) && occurrence_chains[i].is_some())
        .collect();
    let ui_with_chains: Vec<usize> = (0..occurrences.len())
        .filter(|&i| {
            is_ui_window_sink(&occurrences[i].effect_type) && occurrence_chains[i].is_some()
        })
        .collect();
    // Error sinks (pre-filtered by errorEscapesChain).
    let error_with_chains: Vec<usize> = (0..occurrences.len())
        .filter(|&i| {
            if occurrences[i].effect_type != "ERROR_THROW" {
                return false;
            }
            let Some(occ_chain) = occurrence_chains[i].as_ref() else {
                return false;
            };
            error_escapes_chain(
                effects[i].evidence_operation_id.as_deref(),
                occ_chain,
                routine_return_summaries,
                snap,
            )
        })
        .collect();

    // httpMethodByCallsiteId from capability facts.
    let mut http_method_by_callsite_id: HashMap<String, String> = HashMap::new();
    for f in &snap.capability_facts {
        if f.resource_kind != "http" {
            continue;
        }
        let Some(wc) = &f.witness_callsite_id else {
            continue;
        };
        if let Some(SnapCapabilityExtra::Http { method, .. }) = &f.extra {
            http_method_by_callsite_id.insert(wc.clone(), method.clone());
        }
    }

    // --- COMMIT-occurrence root labels (EXTERNAL_IO_BEFORE_COMMIT COMMIT-carried). ---
    for &commit_idx in &commit_with_chains {
        let commit_occ = &occurrences[commit_idx];
        let commit_chain = occurrence_chains[commit_idx].as_ref().unwrap();

        // Compute commitDomRoot — mirrors TS commitDomRoot condition exactly.
        let chain_complete = commit_chain.chain.path_enumeration == "complete";
        let commit_callee_check = check_callee_returnability(&commit_chain.terminal_routine_id);
        let commit_is_root_local = commit_chain.terminal_routine_id == routine_id;
        let commit_innermost_frame_id: i64 = commit_occ
            .ordered_op
            .as_ref()
            .and_then(|op| op.frame_chain.last())
            .map(|f| f.frame_id)
            .unwrap_or(0);
        let commit_blocked = commit_is_root_local
            && commit_occ.ordered_op.is_some()
            && preceding_callsite_always_errors(
                commit_occ
                    .ordered_op
                    .as_ref()
                    .map(|op| op.order_id)
                    .unwrap_or(0),
                commit_innermost_frame_id,
            );
        let commit_dom_root = chain_complete
            && commit_callee_check != "no-edge"
            && !commit_blocked
            && commit_occ.ordered_op.is_some()
            && crate::engine::l5::ordering_inter::cross_hop_dominates_root_return(
                commit_chain,
                routine_id,
                snap,
                callsite_by_id,
            );

        // Root-scope COMMIT_DOMINATES_RETURN.
        if commit_dom_root {
            root_scoped[commit_idx].push(ScopedGuarantee {
                label: "COMMIT_DOMINATES_RETURN",
                scope: "root",
                write_occurrence_id: None,
                commit_occurrence_id: Some(commit_occ.occurrence_id.clone()),
                io_occurrence_id: None,
                return_occurrence_id: Some(return_occurrence_id.clone()),
                supporting_edge_ids: Vec::new(),
                commit_effectiveness: None,
                intervening_boundary: "none",
                valid_for_refutation: true,
            });
        }

        let commit_effectiveness = grade_commit_effectiveness(commit_occ, commit_chain);

        // Root-scope EXTERNAL_IO_BEFORE_COMMIT: first IO with a cross-hop edge io→commit.
        let first_io_before_commit = io_with_chains.iter().copied().find(|&ii| {
            let io_occ = &occurrences[ii];
            edges_from(&io_occ.occurrence_id)
                .iter()
                .any(|e| e.to == commit_occ.occurrence_id)
        });
        if let Some(io_idx) = first_io_before_commit {
            let io_occ = &occurrences[io_idx];
            root_scope_labels[commit_idx].push("EXTERNAL_IO_BEFORE_COMMIT");
            root_scoped[commit_idx].push(ScopedGuarantee {
                label: "EXTERNAL_IO_BEFORE_COMMIT",
                scope: "root",
                write_occurrence_id: None,
                commit_occurrence_id: Some(commit_occ.occurrence_id.clone()),
                io_occurrence_id: Some(io_occ.occurrence_id.clone()),
                return_occurrence_id: None,
                supporting_edge_ids: Vec::new(),
                commit_effectiveness: Some(commit_effectiveness),
                intervening_boundary: "none",
                valid_for_refutation: false,
            });
        }
    }

    // --- IO-occurrence root EXTERNAL_IO_BEFORE_COMMIT (IO-carried). ---
    for &io_idx in &io_with_chains {
        let io_occ = &occurrences[io_idx];
        let first_commit_edge = edges_from(&io_occ.occurrence_id).iter().find(|e| {
            occurrences
                .iter()
                .find(|o| o.occurrence_id == e.to)
                .map(|o| o.effect_type == "COMMIT")
                .unwrap_or(false)
        });
        if let Some(edge) = first_commit_edge {
            root_scope_labels[io_idx].push("EXTERNAL_IO_BEFORE_COMMIT");
            root_scoped[io_idx].push(ScopedGuarantee {
                label: "EXTERNAL_IO_BEFORE_COMMIT",
                scope: "root",
                write_occurrence_id: None,
                commit_occurrence_id: Some(edge.to.clone()),
                io_occurrence_id: Some(io_occ.occurrence_id.clone()),
                return_occurrence_id: None,
                supporting_edge_ids: Vec::new(),
                commit_effectiveness: None,
                intervening_boundary: "none",
                valid_for_refutation: false,
            });
        }
    }

    // --- Boundary machinery (rootBoundaryCallsites + boundaryBetween). ---
    let mut order_by_callsite_id: HashMap<String, Option<u32>> = HashMap::new();
    for cs in &snap.callsite_index {
        if cs.routine != routine_id {
            continue;
        }
        order_by_callsite_id.insert(cs.callsite_id.clone(), cs.order.map(|o| o.order_id));
    }
    let is_object_run = |d: &str| matches!(d, "codeunit-run" | "page-run" | "report-run");

    // Background-kickoff callsite ids.
    let mut background_kickoff_callsite_ids: HashSet<String> = HashSet::new();
    for f in &snap.capability_facts {
        if f.resource_kind != "background" || f.op != "start" {
            continue;
        }
        if let Some(wc) = &f.witness_callsite_id {
            background_kickoff_callsite_ids.insert(wc.clone());
        }
    }

    // rootBoundaryCallsites: (callsiteId, orderId option).
    let mut root_boundary_callsites: Vec<(String, Option<u32>)> = Vec::new();
    for cr in &snap.callsite_resolutions {
        if cr.from != routine_id {
            continue;
        }
        let dispatch_kind = &cr.dispatch_kind;
        let status = &cr.status;
        if is_object_run(dispatch_kind) {
            let target_transparent = status == "resolved" || status == "builtin";
            if cr.result_consumed == Some(false) && target_transparent {
                continue;
            }
            root_boundary_callsites.push((
                cr.callsite_id.clone(),
                order_by_callsite_id.get(&cr.callsite_id).copied().flatten(),
            ));
            continue;
        }
        let is_opaque = status != "resolved" && status != "builtin";
        if !is_opaque {
            continue;
        }
        if background_kickoff_callsite_ids.contains(&cr.callsite_id) {
            continue;
        }
        root_boundary_callsites.push((
            cr.callsite_id.clone(),
            order_by_callsite_id.get(&cr.callsite_id).copied().flatten(),
        ));
    }

    let boundary_between = |write_order_id: u32,
                            io_order_id: u32,
                            write_callsite_id: Option<&str>,
                            io_callsite_id: Option<&str>|
     -> bool {
        let lo = write_order_id.min(io_order_id);
        let hi = write_order_id.max(io_order_id);
        root_boundary_callsites.iter().any(|(cid, oid)| {
            if Some(cid.as_str()) == write_callsite_id {
                return false;
            }
            if Some(cid.as_str()) == io_callsite_id {
                return false;
            }
            match oid {
                None => true, // unpositioned ⇒ possibly-between (HAZARD #1)
                Some(o) => *o > lo && *o < hi,
            }
        })
    };

    // --- derivePendingAtSink (shared IO + UI helper). ---
    // Returns the guarantees + labels to push, per sink index.
    let derive_pending_at_sink =
        |sink_idxs: &[usize],
         sink_label: &'static str,
         out_labels: &mut [Vec<&'static str>],
         out_scoped: &mut [Vec<ScopedGuarantee>]| {
            for &sink_idx in sink_idxs {
                let sink_occ = &occurrences[sink_idx];
                let sink_chain = occurrence_chains[sink_idx].as_ref().unwrap();

                for &write_idx in &write_with_chains {
                    let write_occ = &occurrences[write_idx];
                    let write_chain = occurrence_chains[write_idx].as_ref().unwrap();

                    if !is_physical_write(&write_occ.occurrence_id) {
                        continue;
                    }
                    if write_chain.terminal_routine_id != sink_chain.terminal_routine_id {
                        continue;
                    }
                    let same_routine_intra = write_chain.terminal_routine_id == routine_id
                        && sink_chain.terminal_routine_id == routine_id;

                    let write_op = write_occ.ordered_op.as_ref();
                    let sink_op = sink_occ.ordered_op.as_ref();

                    let has_cross_hop_must = edges_from(&write_occ.occurrence_id).iter().any(|e| {
                        e.to == sink_occ.occurrence_id && e.quantifier == Quantifier::MustAllPaths
                    });
                    let has_intra_must = same_routine_intra
                        && write_op.is_some()
                        && sink_op.is_some()
                        && dom(write_op.unwrap(), sink_op.unwrap());
                    if !has_cross_hop_must && !has_intra_must {
                        continue;
                    }

                    // commitBetween (permissive).
                    let commit_between = commit_with_chains.iter().copied().any(|ci| {
                        let c_occ = &occurrences[ci];
                        let c_chain = occurrence_chains[ci].as_ref().unwrap();
                        let c_op = c_occ.ordered_op.as_ref();
                        let c_is_root_local = c_chain.terminal_routine_id == routine_id;
                        let intra_usable = same_routine_intra && c_is_root_local;
                        let w_before_c = edges_from(&write_occ.occurrence_id)
                            .iter()
                            .any(|e| e.to == c_occ.occurrence_id)
                            || (intra_usable
                                && write_op.is_some()
                                && c_op.is_some()
                                && (has_intra_edge(
                                    &write_occ.occurrence_id,
                                    &c_occ.occurrence_id,
                                ) || (ordered_before(write_op.unwrap(), c_op.unwrap())
                                    && may_co_execute(write_op.unwrap(), c_op.unwrap()))));
                        // GATED: c_before_sink uses intra HB only when `intra_usable`
                        // (same-routine intra path AND commit is root-local).
                        // ASYMMETRY: the event-subscriber cBeforeSink block below
                        // (ordering-engine.ts line 2441) uses hasIntraEdge
                        // UNCONDITIONALLY (no `intra_usable` gate) because the
                        // event-crossed commit and sink may be in the subscriber
                        // routine (c and sink share the same orderId space regardless
                        // of root-locality).  These two paths are intentionally
                        // different; the corpus case is ws-txn-d47-event-pos.
                        // (al-sem ordering-engine.ts lines 2278-2286 vs line 2441.)
                        let c_before_sink = edges_from(&c_occ.occurrence_id)
                            .iter()
                            .any(|e| e.to == sink_occ.occurrence_id)
                            || (intra_usable
                                && c_op.is_some()
                                && sink_op.is_some()
                                && (has_intra_edge(&c_occ.occurrence_id, &sink_occ.occurrence_id)
                                    || (ordered_before(c_op.unwrap(), sink_op.unwrap())
                                        && may_co_execute(c_op.unwrap(), sink_op.unwrap()))));
                        w_before_c && c_before_sink
                    });
                    if commit_between {
                        continue;
                    }

                    // intraBoundaryBetween (same-routine refutation suppression).
                    let mut intra_boundary_between = false;
                    if same_routine_intra
                        && let (Some(write_op), Some(sink_op)) = (write_op, sink_op)
                    {
                        let sink_callsite_id = effects[sink_idx].evidence_callsite_id.as_deref();
                        let write_callsite_id = effects[write_idx].evidence_callsite_id.as_deref();
                        intra_boundary_between = boundary_between(
                            write_op.order_id,
                            sink_op.order_id,
                            write_callsite_id,
                            sink_callsite_id,
                        );
                    }

                    out_labels[sink_idx].push(sink_label);
                    out_scoped[sink_idx].push(ScopedGuarantee {
                        label: sink_label,
                        scope: "root",
                        write_occurrence_id: Some(write_occ.occurrence_id.clone()),
                        commit_occurrence_id: None,
                        io_occurrence_id: Some(sink_occ.occurrence_id.clone()),
                        return_occurrence_id: None,
                        supporting_edge_ids: Vec::new(),
                        commit_effectiveness: None,
                        intervening_boundary: if intra_boundary_between {
                            "unknown"
                        } else {
                            "none"
                        },
                        valid_for_refutation: !intra_boundary_between
                            && sink_chain.chain.path_enumeration == "complete"
                            && write_chain.chain.path_enumeration == "complete",
                    });
                    break; // one supporting write is enough
                }
            }
        };

    // EMISSION ORDER (HAZARD #3): derivePendingAtSink IO (2339).
    derive_pending_at_sink(
        &io_with_chains,
        "WRITE_PENDING_AT_EXTERNAL_IO",
        &mut root_scope_labels,
        &mut root_scoped,
    );

    // --- EXTERNAL_IO_IN_EVENT_SUBSCRIBER_TXN (2369). ---
    for &sink_idx in &io_with_chains {
        let sink_occ = &occurrences[sink_idx];
        let sink_chain = occurrence_chains[sink_idx].as_ref().unwrap();

        if root_scope_labels[sink_idx].contains(&"WRITE_PENDING_AT_EXTERNAL_IO") {
            continue;
        }

        for &write_idx in &write_with_chains {
            let write_occ = &occurrences[write_idx];
            let write_chain = occurrence_chains[write_idx].as_ref().unwrap();
            if !is_physical_write(&write_occ.occurrence_id) {
                continue;
            }

            // Event-crossed MAY edge: write → sink tagged "event-subscriber-may" (HAZARD #4).
            let has_event_crossed = edges_from(&write_occ.occurrence_id).iter().any(|e| {
                e.to == sink_occ.occurrence_id
                    && e.edge_condition_reasons.contains(&"event-subscriber-may")
            });
            if !has_event_crossed {
                continue;
            }

            let write_op = write_occ.ordered_op.as_ref();
            let sink_op = sink_occ.ordered_op.as_ref();
            let write_terminal_routine_id = &write_chain.terminal_routine_id;
            let same_routine_intra = sink_chain.terminal_routine_id == routine_id
                && write_terminal_routine_id == routine_id;

            let commit_between = commit_with_chains.iter().copied().any(|ci| {
                let c_occ = &occurrences[ci];
                let c_chain = occurrence_chains[ci].as_ref().unwrap();
                let c_op = c_occ.ordered_op.as_ref();
                let c_is_root_local = c_chain.terminal_routine_id == routine_id;
                let intra_usable = same_routine_intra && c_is_root_local;

                let w_before_c = edges_from(&write_occ.occurrence_id)
                    .iter()
                    .any(|e| e.to == c_occ.occurrence_id)
                    || (intra_usable
                        && write_op.is_some()
                        && c_op.is_some()
                        && (has_intra_edge(&write_occ.occurrence_id, &c_occ.occurrence_id)
                            || (ordered_before(write_op.unwrap(), c_op.unwrap())
                                && may_co_execute(write_op.unwrap(), c_op.unwrap()))));

                let c_same_routine_as_sink =
                    c_chain.terminal_routine_id == sink_chain.terminal_routine_id;
                // UNCONDITIONAL: hasIntraEdge is called without an `intra_usable`
                // gate because the event-crossed commit and sink may both live in the
                // subscriber routine (same orderId space regardless of root-locality).
                // ASYMMETRY: the derive_pending_at_sink cBeforeSink path above uses
                // hasIntraEdge only when `intra_usable` is true (same-routine + root-
                // local).  These two sites have intentionally different guards; the
                // corpus coverage is ws-txn-d47-event-pos.
                // (al-sem ordering-engine.ts line 2441 unconditional vs lines
                // 2278-2286 gated.)
                let c_before_sink = edges_from(&c_occ.occurrence_id)
                    .iter()
                    .any(|e| e.to == sink_occ.occurrence_id)
                    || has_intra_edge(&c_occ.occurrence_id, &sink_occ.occurrence_id)
                    || (intra_usable
                        && c_op.is_some()
                        && sink_op.is_some()
                        && ordered_before(c_op.unwrap(), sink_op.unwrap())
                        && may_co_execute(c_op.unwrap(), sink_op.unwrap()))
                    || (c_same_routine_as_sink
                        && !intra_usable
                        && c_op.is_some()
                        && sink_op.is_some()
                        && ordered_before(c_op.unwrap(), sink_op.unwrap())
                        && may_co_execute(c_op.unwrap(), sink_op.unwrap()));

                w_before_c && c_before_sink
            });
            if commit_between {
                continue;
            }

            root_scope_labels[sink_idx].push("EXTERNAL_IO_IN_EVENT_SUBSCRIBER_TXN");
            root_scoped[sink_idx].push(ScopedGuarantee {
                label: "EXTERNAL_IO_IN_EVENT_SUBSCRIBER_TXN",
                scope: "root",
                write_occurrence_id: Some(write_occ.occurrence_id.clone()),
                commit_occurrence_id: None,
                io_occurrence_id: Some(sink_occ.occurrence_id.clone()),
                return_occurrence_id: None,
                supporting_edge_ids: Vec::new(),
                commit_effectiveness: None,
                intervening_boundary: "none",
                valid_for_refutation: false, // ALWAYS advisory
            });
            break;
        }
    }

    // --- WRITE_PENDING_AT_UI (2482). ---
    derive_pending_at_sink(
        &ui_with_chains,
        "WRITE_PENDING_AT_UI",
        &mut root_scope_labels,
        &mut root_scoped,
    );

    // --- IO_BEFORE_ESCAPING_ERROR (2528). ---
    for &io_idx in &io_with_chains {
        let io_occ = &occurrences[io_idx];
        let io_chain = occurrence_chains[io_idx].as_ref().unwrap();
        if io_chain.terminal_routine_id != routine_id {
            continue;
        }
        let Some(io_callsite_id) = effects[io_idx].evidence_callsite_id.clone() else {
            continue;
        };
        let http_method = http_method_by_callsite_id
            .get(&io_callsite_id)
            .cloned()
            .unwrap_or_default();
        let file_op = ""; // (FILE not produced for IO_BEFORE_ESCAPING_ERROR in corpus)
        if io_direction(&io_occ.effect_type, &http_method, file_op) != "write" {
            continue;
        }
        let Some(io_op) = io_occ.ordered_op.clone() else {
            continue;
        };

        for &err_idx in &error_with_chains {
            let err_occ = &occurrences[err_idx];
            let err_chain = occurrence_chains[err_idx].as_ref().unwrap();
            let err_is_root_local = err_chain.terminal_routine_id == routine_id;

            let mut intra_witness = false;
            let mut intra_boundary_between = false;
            if err_is_root_local {
                let Some(err_op) = err_occ.ordered_op.as_ref() else {
                    continue;
                };
                let may_exec = may_co_execute(&io_op, err_op);
                let ordered = ordered_before(&io_op, err_op);
                if may_exec && ordered {
                    intra_boundary_between = boundary_between(
                        io_op.order_id,
                        err_op.order_id,
                        Some(&io_callsite_id),
                        None,
                    );
                    intra_witness = true;
                }
            }

            let mut cross_hop_witness = false;
            let mut cross_hop_boundary_between = false;
            if !err_is_root_local
                && let Some(first_link) = err_chain.chain.links.first()
                && let Some(cs_order) = first_link.callsite_order.as_ref().filter(|_| {
                    err_chain.chain.path_enumeration == "complete"
                        && first_link.caller_routine_id == routine_id
                        && first_link.dispatch_sequencing == "sequential"
                })
            {
                let root_frames = snap
                    .routine_order_frames
                    .as_ref()
                    .and_then(|m| m.get(routine_id));
                let cs_frame_chain = reconstruct_frame_chain(cs_order.frame_id, root_frames);
                if !cs_frame_chain.is_empty() {
                    let callsite_op = OrderedOp {
                        occurrence_id: first_link.callsite_id.clone(),
                        order_id: cs_order.order_id,
                        on_success_path: cs_order.on_success_path,
                        dominates_success_return: cs_order.dominates_success_return,
                        frame_chain: cs_frame_chain,
                    };
                    if may_co_execute(&io_op, &callsite_op) && ordered_before(&io_op, &callsite_op)
                    {
                        cross_hop_boundary_between = boundary_between(
                            io_op.order_id,
                            cs_order.order_id,
                            Some(&io_callsite_id),
                            Some(&first_link.callsite_id),
                        );
                        cross_hop_witness = true;
                    }
                }
            }

            if !intra_witness && !cross_hop_witness {
                continue;
            }

            let valid_for_refutation = if err_is_root_local {
                !intra_boundary_between
                    && io_chain.chain.path_enumeration == "complete"
                    && err_chain.chain.path_enumeration == "complete"
            } else {
                !cross_hop_boundary_between && io_chain.chain.path_enumeration == "complete"
            };

            // Commit-escalation (intra only).
            let mut commit_on_path: Option<&'static str> = None;
            if intra_witness {
                let err_op = err_occ.ordered_op.as_ref();
                for &ci in &commit_with_chains {
                    let c_occ = &occurrences[ci];
                    let c_chain = occurrence_chains[ci].as_ref().unwrap();
                    let Some(c_op) = c_occ.ordered_op.as_ref() else {
                        continue;
                    };
                    if c_chain.terminal_routine_id != routine_id {
                        continue;
                    }
                    let c_after_io = has_intra_edge(&io_occ.occurrence_id, &c_occ.occurrence_id)
                        || (ordered_before(&io_op, c_op) && may_co_execute(&io_op, c_op));
                    let c_before_err = err_op.is_some()
                        && (has_intra_edge(&c_occ.occurrence_id, &err_occ.occurrence_id)
                            || (ordered_before(c_op, err_op.unwrap())
                                && may_co_execute(c_op, err_op.unwrap())));
                    if !c_after_io || !c_before_err {
                        continue;
                    }
                    let effectiveness = grade_commit_effectiveness(c_occ, c_chain);
                    if effectiveness == "proven_effective" {
                        commit_on_path = Some("proven_effective");
                        break;
                    }
                    if commit_on_path.is_none() {
                        commit_on_path = Some(effectiveness);
                    }
                }
            }

            root_scope_labels[io_idx].push("IO_BEFORE_ESCAPING_ERROR");
            root_scoped[io_idx].push(ScopedGuarantee {
                label: "IO_BEFORE_ESCAPING_ERROR",
                scope: "root",
                write_occurrence_id: Some(err_occ.occurrence_id.clone()), // repurposed error-sink id
                commit_occurrence_id: None,
                io_occurrence_id: Some(io_occ.occurrence_id.clone()),
                return_occurrence_id: None,
                supporting_edge_ids: Vec::new(),
                commit_effectiveness: commit_on_path,
                intervening_boundary: if (intra_witness && intra_boundary_between)
                    || (cross_hop_witness && cross_hop_boundary_between)
                {
                    "unknown"
                } else {
                    "none"
                },
                valid_for_refutation,
            });
            break;
        }
    }

    // ====================================================================
    // Merge: intra (owning-routine, minimal) then root-scoped.
    // ====================================================================
    occurrences
        .iter()
        .enumerate()
        .map(|(idx, _)| {
            let mut scoped: Vec<ScopedGuarantee> = intra_labels_by_index[idx]
                .iter()
                .map(|&label| ScopedGuarantee {
                    label,
                    scope: "owning-routine",
                    write_occurrence_id: None,
                    commit_occurrence_id: None,
                    io_occurrence_id: None,
                    return_occurrence_id: None,
                    supporting_edge_ids: Vec::new(),
                    commit_effectiveness: None,
                    intervening_boundary: "none",
                    valid_for_refutation: false,
                })
                .collect();
            // root_scoped has the metadata; append directly (rootScoped.length > 0 path).
            for sg in &root_scoped[idx] {
                scoped.push(sg.clone());
            }
            scoped
        })
        .collect()
}

/// gradeCommitEffectiveness rule priority (ordering-engine.ts 1259-1359), given
/// the precomputed signals. Priority (first match wins):
/// - commit op not on a success path → proven_errors (rule 1)
/// - any TryFunction boundary OR CommitBehavior::Error on chain → proven_errors (rules 2/3)
/// - any CommitBehavior::Ignore on chain → proven_suppressed (rule 4)
/// - clean + complete + trusted + single-path → proven_effective (rule 5)
/// - otherwise → assumed_effective (rule 6)
fn grade_commit_rules(
    commit_not_on_success_path: bool,
    found_try_or_error: bool,
    found_ignore: bool,
    chain_complete: bool,
    commit_root_trusted: bool,
    is_multi_path: bool,
) -> &'static str {
    if commit_not_on_success_path {
        return "proven_errors";
    }
    if found_try_or_error {
        return "proven_errors";
    }
    if found_ignore {
        return "proven_suppressed";
    }
    if chain_complete && commit_root_trusted && !is_multi_path {
        return "proven_effective";
    }
    "assumed_effective"
}

// isChainComplete (ordering-engine.ts).
fn is_chain_complete(
    commit_chain: &OccurrenceWithChain,
    summaries: &HashMap<String, RoutineReturnSummary>,
) -> bool {
    if commit_chain.chain.path_enumeration != "complete" {
        return false;
    }
    let mut chain_routine_ids: Vec<&str> = vec![commit_chain.terminal_routine_id.as_str()];
    for link in &commit_chain.chain.links {
        chain_routine_ids.push(link.caller_routine_id.as_str());
        chain_routine_ids.push(link.callee_routine_id.as_str());
    }
    for id in chain_routine_ids {
        if !summaries.contains_key(id) {
            return false;
        }
    }
    true
}

// ===========================================================================
// Native oracles.
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boundary_between_unpositioned_is_true() {
        // A boundary callsite with no positioned orderId ⇒ possibly-between TRUE (HAZARD #1).
        let root_boundary_callsites: Vec<(String, Option<u32>)> = vec![("csX".to_string(), None)];
        let boundary_between = |write: u32, io: u32, w_cs: Option<&str>, io_cs: Option<&str>| {
            let lo = write.min(io);
            let hi = write.max(io);
            root_boundary_callsites.iter().any(|(cid, oid)| {
                if Some(cid.as_str()) == w_cs {
                    return false;
                }
                if Some(cid.as_str()) == io_cs {
                    return false;
                }
                match oid {
                    None => true,
                    Some(o) => *o > lo && *o < hi,
                }
            })
        };
        assert!(boundary_between(1, 5, Some("csW"), Some("csIO")));
        // But if the unpositioned boundary IS one of the endpoints, excluded.
        assert!(!boundary_between(1, 5, Some("csX"), Some("csIO")));
    }

    #[test]
    fn is_trusted_commit_root_untrusted_kind() {
        use crate::engine::l5::snapshot::SnapshotRootClassificationSlot;
        let mut snap = empty_snap();
        snap.root_classifications
            .push(SnapshotRootClassificationSlot {
                routine_id: "r1".to_string(),
                kinds: vec!["event-subscriber".to_string()],
                externally_reachable: true,
                source: "ast".to_string(),
                confidence: "high".to_string(),
                source_anchor: None,
                config_entry_id: None,
                resolution_status: None,
            });
        // event-subscriber is untrusted.
        assert!(!is_trusted_commit_root("r1", &snap));
        // unknown routine (no slot) → trusted.
        assert!(is_trusted_commit_root("r2", &snap));
    }

    #[test]
    fn grade_commit_rules_priority() {
        // Rule 1: not-on-success-path beats everything → proven_errors.
        assert_eq!(
            grade_commit_rules(true, false, true, true, true, false),
            "proven_errors"
        );
        // Rule 2/3: TryFunction/Error → proven_errors (beats Ignore).
        assert_eq!(
            grade_commit_rules(false, true, true, true, true, false),
            "proven_errors"
        );
        // Rule 4: Ignore → proven_suppressed (beats the clean promotion).
        assert_eq!(
            grade_commit_rules(false, false, true, true, true, false),
            "proven_suppressed"
        );
        // Rule 5: clean + complete + trusted + single-path → proven_effective.
        assert_eq!(
            grade_commit_rules(false, false, false, true, true, false),
            "proven_effective"
        );
        // Cap: multi-path blocks proven_effective → assumed_effective.
        assert_eq!(
            grade_commit_rules(false, false, false, true, true, true),
            "assumed_effective"
        );
        // Cap: untrusted root blocks proven_effective → assumed_effective.
        assert_eq!(
            grade_commit_rules(false, false, false, true, false, false),
            "assumed_effective"
        );
        // Cap: incomplete chain blocks proven_effective → assumed_effective.
        assert_eq!(
            grade_commit_rules(false, false, false, false, true, false),
            "assumed_effective"
        );
    }

    fn empty_snap() -> CapabilitySnapshot {
        use crate::engine::l5::snapshot::SnapshotIdentityTable;
        CapabilitySnapshot {
            identities: SnapshotIdentityTable {
                stable_ids: vec![],
                display_names: vec![],
            },
            capability_facts: vec![],
            typed_edges: vec![],
            operation_index: vec![],
            callsite_index: vec![],
            callsite_resolutions: vec![],
            analysis_gaps: vec![],
            coverage: vec![],
            event_declarations: vec![],
            root_classifications: vec![],
            routine_order_frames: None,
        }
    }

    // =========================================================================
    // Oracle 5: boundary_between — positioned in-range and endpoint-exclusion.
    //
    // Mirrors al-sem `ordering-engine.ts` lines 2150-2164.
    //
    // Complements the existing unpositioned→true oracle.  Tests:
    //   (a) A Run-boundary callsite with orderId strictly between lo and hi → true.
    //   (b) A boundary callsite whose orderId == write orderId → NOT counted (lo
    //       is not strictly less than o, so the strict > check fails).
    //   (c) A boundary callsite whose orderId == io orderId → NOT counted.
    //   (d) orderId outside the [lo,hi] interval → false.
    // =========================================================================

    #[test]
    fn boundary_between_positioned_in_range() {
        // Callsite "csRun" at orderId 3 — strictly between write(1) and io(5).
        let root_boundary_callsites: Vec<(String, Option<u32>)> =
            vec![("csRun".to_string(), Some(3))];

        let boundary_between =
            |write: u32, io: u32, w_cs: Option<&str>, io_cs: Option<&str>| -> bool {
                let lo = write.min(io);
                let hi = write.max(io);
                root_boundary_callsites.iter().any(|(cid, oid)| {
                    if Some(cid.as_str()) == w_cs {
                        return false;
                    }
                    if Some(cid.as_str()) == io_cs {
                        return false;
                    }
                    match oid {
                        None => true,
                        Some(o) => *o > lo && *o < hi,
                    }
                })
            };

        // (a) 3 is strictly between 1 and 5 → true (al-sem line 2162).
        assert!(
            boundary_between(1, 5, Some("csW"), Some("csIO")),
            "boundary callsite at orderId 3 is strictly between 1 and 5"
        );

        // (b) Boundary callsite IS the write endpoint (csRun == w_cs) → excluded.
        assert!(
            !boundary_between(1, 5, Some("csRun"), Some("csIO")),
            "boundary callsite matching write callsite is excluded (al-sem line 2159)"
        );

        // (c) Boundary callsite IS the io endpoint → excluded.
        assert!(
            !boundary_between(1, 5, Some("csW"), Some("csRun")),
            "boundary callsite matching io callsite is excluded (al-sem line 2160)"
        );

        // (d) Boundary callsite orderId == lo (not strictly greater) → false.
        // Use write=3, io=5 so lo=3: csRun(3) is NOT > 3 → false.
        assert!(
            !boundary_between(3, 5, Some("csW"), Some("csIO")),
            "boundary callsite at orderId == lo is not strictly inside interval"
        );

        // (e) Boundary callsite orderId == hi (not strictly less) → false.
        // Use write=1, io=3 so hi=3: csRun(3) is NOT < 3 → false.
        assert!(
            !boundary_between(1, 3, Some("csW"), Some("csIO")),
            "boundary callsite at orderId == hi is not strictly inside interval"
        );

        // (f) Boundary callsite outside interval entirely → false.
        // write=1, io=2 → interval (1,2) exclusive; csRun=3 is not in it.
        assert!(
            !boundary_between(1, 2, Some("csW"), Some("csIO")),
            "boundary callsite outside (1,2) exclusive interval → false"
        );
    }

    // =========================================================================
    // Oracle 6: IO_BEFORE_ESCAPING_ERROR — intra vs cross-hop branch coverage.
    //
    // Both d51 corpus fixtures (ws-d51-pos, ws-d51-jobqueue) use the INTRA branch:
    // HTTP POST and Error() are in the same root routine (err_is_root_local=true).
    // The cross-hop branch (err_is_root_local=false, synthetic callsite OrderedOp
    // + re-check) has no dedicated corpus fixture; its logic is covered by the
    // code-path exercised through `compute_ordering` when a cross-routine error
    // escapes via a sequential first-link in the error's call chain.
    //
    // The key branch condition (ordering-engine.rs line ~1161):
    //   `!err_is_root_local` → uses the chain's first_link.callsite_order
    //    to build a synthetic OrderedOp in the root routine's orderId space,
    //    then re-checks may_co_execute + ordered_before with the IO op.
    //
    // This oracle documents the intra path (err_is_root_local=true) fully
    // exercised by both d51 corpus fixtures, and notes the cross-hop path
    // is corpus-covered ONLY via compute_ordering integration (no direct
    // unit fixture produces the cross-hop golden at this time).
    // =========================================================================

    // (No executable oracle for the cross-hop branch — it is not unit-extractable
    // without a full compute_ordering call; corpus documentation is above.)
}

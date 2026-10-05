//! R4-F Stage-4b — cross-hop / inter-routine HB substrate.
//!
//! Byte-parity port of al-sem:
//!   - `src/digest/call-chain.ts`         → CallChain / CallLink / reconstruct_call_chains
//!   - `src/digest/call-chain-helpers.ts` → control-placement / dispatch-sequencing / multiplicity
//!   - `src/digest/inter-dominance.ts`    → op_dominates_root_return (complete-graph dominance)
//!   - `src/digest/ordering-inter.ts`     → inter_hb / cross_hop_dominates_root_return / metadata
//!   - `src/digest/error-escapes-chain.ts`→ error_escapes_chain (B1-B14 barriers)
//!   - `src/transaction-integrity/io-direction.ts` → io_direction
//!
//! `interHBNoEdgeReason` / `unprovenPairs` are DEAD in the order:false path — OMITTED.

use std::collections::{HashMap, HashSet};

use crate::engine::l2::operation_order::{OperationOrder, ScopeFrame};
use crate::engine::l5::digest::QueryWitnessHop;
use crate::engine::l5::ordering::{
    OrderedOp, Quantifier, dom, dominates_return, may_co_execute, may_precede_success_return,
    ordered_before,
};
use crate::engine::l5::snapshot::{
    CapabilitySnapshot, SnapshotCallsiteEvidence, SnapshotCallsiteResolution,
};
use crate::engine::return_summary::RoutineReturnSummary;

// ---------------------------------------------------------------------------
// CallChain / CallLink (call-chain.ts)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct CallsiteOrder {
    pub order_id: u32,
    pub frame_id: i64,
    pub on_success_path: bool,
    pub dominates_success_return: bool,
}

#[derive(Debug, Clone)]
pub struct CallLink {
    pub caller_routine_id: String,
    pub callee_routine_id: String,
    pub callsite_id: String,
    pub callsite_order: Option<CallsiteOrder>,
    pub hop_kind: String,
    pub resolution_status: String,
    pub dispatch_sequencing: &'static str, // sequential|alternative|unordered-broadcast|barrier
    pub control_placement: &'static str,   // top-level|conditional|loop|non-fallthrough|unknown
    pub invocation_multiplicity: &'static str, // exactly_once_on_success|zero_or_one|zero_or_more|unknown
    pub event_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct CallChain {
    pub links: Vec<CallLink>,
    pub path_enumeration: &'static str, // complete|capped|truncated|unresolved
}

// ---------------------------------------------------------------------------
// Frame-chain reconstruction (root-first, innermost-last).
// ---------------------------------------------------------------------------

pub fn reconstruct_frame_chain(frame_id: i64, frames: Option<&[ScopeFrame]>) -> Vec<ScopeFrame> {
    let Some(frames) = frames else {
        return Vec::new();
    };
    if frames.is_empty() {
        return Vec::new();
    }
    let mut by_id: HashMap<i64, &ScopeFrame> = HashMap::new();
    for f in frames {
        by_id.insert(f.frame_id, f);
    }
    let mut chain: Vec<ScopeFrame> = Vec::new();
    let mut cur = by_id.get(&frame_id).copied();
    while let Some(c) = cur {
        chain.push(c.clone());
        if c.parent_frame_id == -1 {
            break;
        }
        cur = by_id.get(&c.parent_frame_id).copied();
    }
    chain.reverse();
    chain
}

// ---------------------------------------------------------------------------
// call-chain-helpers.ts
// ---------------------------------------------------------------------------

fn is_selector_kind(kind: &str) -> bool {
    kind == "if-then" || kind == "if-else" || kind == "case-branch"
}

pub fn derive_control_placement_from_frame_chain(frame_chain: &[ScopeFrame]) -> &'static str {
    if frame_chain.is_empty() {
        return "unknown";
    }
    let mut has_loop = false;
    let mut has_non_fallthrough = false;
    let mut has_selector = false;
    for f in frame_chain {
        if f.kind == "loop" {
            has_loop = true;
        } else if is_selector_kind(&f.kind) {
            has_selector = true;
            if f.branch_may_fall_through == Some(false) {
                has_non_fallthrough = true;
            }
        }
    }
    if has_loop {
        return "loop";
    }
    if has_non_fallthrough {
        return "non-fallthrough";
    }
    if has_selector {
        return "conditional";
    }
    "top-level"
}

pub fn derive_dispatch_sequencing_from_edge(
    hop_kind: &str,
    ledger_status: Option<&str>,
) -> &'static str {
    if hop_kind == "event-dispatch" || hop_kind == "implicit-trigger" {
        return "unordered-broadcast";
    }
    if hop_kind == "interface-dispatch" {
        return "alternative";
    }
    if hop_kind == "dependency-export" {
        return "barrier";
    }
    if hop_kind == "call" || hop_kind == "variable-typed-call" || hop_kind == "object-run" {
        return match ledger_status {
            Some("resolved") => "sequential",
            Some("polymorphic") => "alternative",
            Some("ambiguous")
            | Some("unfetched-dependency")
            | Some("external")
            | Some("dynamic-target")
            | Some("unresolved-receiver-type")
            | Some("unresolved-member")
            | Some("builtin") => "barrier",
            _ => "barrier",
        };
    }
    "barrier"
}

pub fn derive_invocation_multiplicity(
    control_placement: &str,
    dominates_success_return: bool,
) -> &'static str {
    match control_placement {
        "top-level" => {
            if dominates_success_return {
                "exactly_once_on_success"
            } else {
                "zero_or_one"
            }
        }
        "loop" => "zero_or_more",
        "conditional" | "non-fallthrough" => "zero_or_one",
        _ => "unknown",
    }
}

// ---------------------------------------------------------------------------
// reconstructCallChains (call-chain.ts)
// ---------------------------------------------------------------------------

/// `indexes.callsiteById` analog: a callsiteId → evidence lookup.
pub type CallsiteByIdMap<'a> = HashMap<&'a str, &'a SnapshotCallsiteEvidence>;

/// callsiteId → ledger row (`callsite_resolutions`; the LAST row wins).
pub type ResolutionByCallsiteId<'a> = HashMap<&'a str, &'a SnapshotCallsiteResolution>;

/// Build [`ResolutionByCallsiteId`] once per snapshot. `reconstruct_call_chains`
/// runs once per effect, and used to rebuild this map over every ledger row on
/// each call.
pub fn resolution_by_callsite_id(snap: &CapabilitySnapshot) -> ResolutionByCallsiteId<'_> {
    let mut m: ResolutionByCallsiteId = HashMap::new();
    for row in &snap.callsite_resolutions {
        m.insert(row.callsite_id.as_str(), row);
    }
    m
}

pub fn reconstruct_call_chains(
    via_paths: &[Vec<QueryWitnessHop>],
    snap: &CapabilitySnapshot,
    callsite_by_id: &CallsiteByIdMap,
    resolution_by_callsite_id: &ResolutionByCallsiteId,
    via_paths_capped: bool,
    isolated_event_ids: Option<&HashSet<String>>,
) -> Vec<CallChain> {
    via_paths
        .iter()
        .map(|hops| {
            let has_unresolved = hops.iter().any(|h| h.to_routine_id.is_none());

            let mut path_enumeration: &'static str = if has_unresolved {
                "unresolved"
            } else if via_paths_capped {
                "capped"
            } else {
                "complete"
            };

            // Rule 5 hardening (1b): event-boundary-erased demotion.
            let mut event_boundary_erased = false;
            for i in 0..hops.len() {
                let h = &hops[i];
                if h.kind != "event-dispatch" {
                    continue;
                }
                if h.callsite_id.is_some() {
                    continue;
                }
                let prev = if i > 0 { Some(&hops[i - 1]) } else { None };
                if prev.is_none() || prev.unwrap().callsite_id.is_none() {
                    event_boundary_erased = true;
                    break;
                }
            }
            if event_boundary_erased && path_enumeration == "complete" {
                path_enumeration = "capped";
            }

            let mut links: Vec<CallLink> = Vec::new();

            for hop_idx in 0..hops.len() {
                let hop = &hops[hop_idx];
                let Some(callsite_id) = hop.callsite_id.as_deref() else {
                    continue;
                };

                let caller_routine_id = hop.from_routine_id.clone();
                let callee_routine_id = hop.to_routine_id.clone().unwrap_or_default();

                let cs_evidence = callsite_by_id.get(callsite_id).copied();
                let callsite_order = cs_evidence.and_then(|c| c.order);

                let mut frame_chain: Vec<ScopeFrame> = Vec::new();
                if let Some(order) = callsite_order {
                    let frames = snap
                        .routine_order_frames
                        .as_ref()
                        .and_then(|m| m.get(&caller_routine_id));
                    frame_chain = reconstruct_frame_chain(order.frame_id, frames);
                }

                let ledger_row = resolution_by_callsite_id.get(callsite_id).copied();
                let ledger_status = ledger_row.map(|r| r.status.as_str());

                let mut dispatch_sequencing =
                    derive_dispatch_sequencing_from_edge(hop.kind, ledger_status);

                // Rule 5 §3: look-ahead — next hop event-dispatch.
                let mut broadcast_event_id: Option<String> = None;
                if let Some(next_hop) = hops.get(hop_idx + 1)
                    && next_hop.kind == "event-dispatch"
                {
                    match next_hop.event_id.as_deref() {
                        None => {
                            dispatch_sequencing = "barrier";
                        }
                        Some(eid) => {
                            if isolated_event_ids.map(|s| s.contains(eid)).unwrap_or(false) {
                                dispatch_sequencing = "barrier";
                            } else {
                                dispatch_sequencing = "unordered-broadcast";
                                broadcast_event_id = Some(eid.to_string());
                            }
                        }
                    }
                }

                // Rule 5 §0.5: isolated event-dispatch hop carrying a callsiteId (rare).
                if dispatch_sequencing == "unordered-broadcast"
                    && hop.kind == "event-dispatch"
                    && let (Some(set), Some(eid)) = (isolated_event_ids, hop.event_id.as_deref())
                    && set.contains(eid)
                {
                    dispatch_sequencing = "barrier";
                }

                // dependency-export barrier → sequential when callee has fresh dep frames.
                if dispatch_sequencing == "barrier" && hop.kind == "dependency-export" {
                    let callee_has_frames = ledger_status == Some("resolved")
                        && !callee_routine_id.is_empty()
                        && snap
                            .routine_order_frames
                            .as_ref()
                            .and_then(|m| m.get(&callee_routine_id))
                            .map(|f| !f.is_empty())
                            .unwrap_or(false);
                    if callee_has_frames {
                        dispatch_sequencing = "sequential";
                    }
                }

                let control_placement = if callsite_order.is_none() {
                    "unknown"
                } else {
                    derive_control_placement_from_frame_chain(&frame_chain)
                };

                let dominates_success_return = callsite_order
                    .map(|o| o.dominates_success_return)
                    .unwrap_or(false);
                let invocation_multiplicity =
                    derive_invocation_multiplicity(control_placement, dominates_success_return);

                let link = CallLink {
                    caller_routine_id,
                    callee_routine_id,
                    callsite_id: callsite_id.to_string(),
                    callsite_order: callsite_order.map(|o| CallsiteOrder {
                        order_id: o.order_id,
                        frame_id: o.frame_id,
                        on_success_path: o.on_success_path,
                        dominates_success_return: o.dominates_success_return,
                    }),
                    hop_kind: hop.kind.to_string(),
                    resolution_status: ledger_status.unwrap_or("unknown").to_string(),
                    dispatch_sequencing,
                    control_placement,
                    invocation_multiplicity,
                    event_id: broadcast_event_id,
                };

                links.push(link);
            }

            CallChain {
                links,
                path_enumeration,
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// OccurrenceWithChain (ordering-inter.ts)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct OccurrenceWithChain {
    pub occurrence_id: String,
    pub terminal_routine_id: String,
    pub terminal_op: Option<OrderedOp>,
    pub terminal_routine_ops: Vec<OrderedOp>,
    pub chain: CallChain,
}

// ---------------------------------------------------------------------------
// inter-dominance.ts
// ---------------------------------------------------------------------------

const MAX_CHAIN_DEPTH: usize = 64;

fn map_edge_kind_to_hop_kind(edge_kind: &str) -> &'static str {
    match edge_kind {
        "direct-call" => "call",
        "variable-typed-call" => "variable-typed-call",
        "interface-dispatch" => "interface-dispatch",
        "object-run-resolved" | "object-run-unresolved" => "object-run",
        "event-dispatch" => "event-dispatch",
        "implicit-trigger" => "implicit-trigger",
        "dependency-export" => "dependency-export",
        _ => "unknown",
    }
}

fn check_callsite_dominates_caller_return(
    callsite_id: &str,
    caller_routine_id: &str,
    snap: &CapabilitySnapshot,
    callsite_by_id: &CallsiteByIdMap,
) -> bool {
    let Some(cs_evidence) = callsite_by_id.get(callsite_id).copied() else {
        return false;
    };
    let Some(order) = cs_evidence.order else {
        return false;
    };
    if !order.dominates_success_return {
        return false;
    }
    let frames = snap
        .routine_order_frames
        .as_ref()
        .and_then(|m| m.get(caller_routine_id));
    let frame_chain = reconstruct_frame_chain(order.frame_id, frames);
    if derive_control_placement_from_frame_chain(&frame_chain) != "top-level" {
        return false;
    }
    // Exactly one outgoing typed edge for this callsite from this caller.
    let all_outgoing: Vec<&_> = snap
        .typed_edges
        .iter()
        .filter(|e| e.edge_from() == caller_routine_id && e.edge_callsite_id() == Some(callsite_id))
        .collect();
    if all_outgoing.len() != 1 {
        return false;
    }
    let single_edge = all_outgoing[0];
    let ledger_row = snap
        .callsite_resolutions
        .iter()
        .find(|r| r.callsite_id == callsite_id);
    let hop_kind = map_edge_kind_to_hop_kind(single_edge.edge_kind());
    let ledger_status = ledger_row.map(|r| r.status.as_str());
    if derive_dispatch_sequencing_from_edge(hop_kind, ledger_status) != "sequential" {
        return false;
    }
    if ledger_row
        .map(|r| r.open_world == Some(true))
        .unwrap_or(false)
    {
        return false;
    }
    true
}

#[allow(clippy::too_many_arguments)]
fn find_unique_dominating_chain(
    target_routine_id: &str,
    root_routine_id: &str,
    snap: &CapabilitySnapshot,
    callsite_by_id: &CallsiteByIdMap,
    visiting: &mut HashSet<String>,
    depth: usize,
) -> bool {
    if depth > MAX_CHAIN_DEPTH {
        return false;
    }
    if visiting.contains(target_routine_id) {
        return false;
    }
    if target_routine_id == root_routine_id {
        return true;
    }
    visiting.insert(target_routine_id.to_string());

    let incoming_edges: Vec<&_> = snap
        .typed_edges
        .iter()
        .filter(|e| e.edge_to() == Some(target_routine_id))
        .collect();

    let result = (|| {
        if incoming_edges.is_empty() {
            return false;
        }
        // Cycle guard.
        for edge in &incoming_edges {
            if visiting.contains(edge.edge_from()) {
                return false;
            }
        }
        let mut dominating_caller_found = false;
        for edge in &incoming_edges {
            let caller_routine_id = edge.edge_from();
            let Some(callsite_id) = edge.edge_callsite_id() else {
                continue;
            };
            if !check_callsite_dominates_caller_return(
                callsite_id,
                caller_routine_id,
                snap,
                callsite_by_id,
            ) {
                continue;
            }
            let mut visiting_copy = visiting.clone();
            let caller_dominates = find_unique_dominating_chain(
                caller_routine_id,
                root_routine_id,
                snap,
                callsite_by_id,
                &mut visiting_copy,
                depth + 1,
            );
            if caller_dominates {
                if dominating_caller_found {
                    return false;
                }
                dominating_caller_found = true;
            }
        }
        dominating_caller_found
    })();

    visiting.remove(target_routine_id);
    result
}

pub fn op_dominates_root_return(
    op_routine_id: &str,
    op_order: &OperationOrder,
    root_routine_id: &str,
    snap: &CapabilitySnapshot,
    callsite_by_id: &CallsiteByIdMap,
) -> bool {
    if !op_order.dominates_success_return {
        return false;
    }
    if op_routine_id == root_routine_id {
        return true;
    }
    let mut visiting = HashSet::new();
    find_unique_dominating_chain(
        op_routine_id,
        root_routine_id,
        snap,
        callsite_by_id,
        &mut visiting,
        0,
    )
}

// ---------------------------------------------------------------------------
// ordering-inter.ts — interHB + metadata + crossHopDominatesRootReturn
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct InterHBEdge {
    pub from: String,
    pub to: String,
    pub quantifier: Quantifier,
    pub coverage: &'static str,
    pub call_path_links: Vec<String>,
    pub edge_condition_kind: &'static str,
    pub edge_condition_reasons: Vec<&'static str>,
}

fn chain_common_prefix_length(a_links: &[CallLink], b_links: &[CallLink]) -> usize {
    let len = a_links.len().min(b_links.len());
    for i in 0..len {
        let a = &a_links[i];
        let b = &b_links[i];
        if a.caller_routine_id != b.caller_routine_id
            || a.callee_routine_id != b.callee_routine_id
            || a.callsite_id != b.callsite_id
        {
            return i;
        }
    }
    len
}

fn path_has_truncation_or_barrier(chain: &CallChain, link_count: usize) -> bool {
    if chain.path_enumeration != "complete" {
        return true;
    }
    for i in 0..link_count.min(chain.links.len()) {
        let link = &chain.links[i];
        if link.dispatch_sequencing != "sequential" {
            return true;
        }
        if link.control_placement == "unknown" {
            return true;
        }
        if link.callsite_order.is_none() {
            return true;
        }
    }
    false
}

fn edge_coverage(
    chain_a: &CallChain,
    a_link_depth: usize,
    chain_b: &CallChain,
    b_link_depth: usize,
) -> &'static str {
    if path_has_truncation_or_barrier(chain_a, a_link_depth)
        || path_has_truncation_or_barrier(chain_b, b_link_depth)
    {
        "partial"
    } else {
        "resolved"
    }
}

pub fn derive_intra_edge_condition(
    a_op: &OrderedOp,
    b_op: &OrderedOp,
) -> (&'static str, Vec<&'static str>) {
    let mut reasons: Vec<&'static str> = Vec::new();
    let mut kind: &'static str = "unconditional";
    let all_frames = a_op.frame_chain.iter().chain(b_op.frame_chain.iter());
    for f in all_frames {
        if f.kind == "loop" {
            if kind != "loop-dependent" {
                kind = "loop-dependent";
                reasons.push("loop-callsite");
            }
        } else if (f.kind == "if-then" || f.kind == "if-else" || f.kind == "case-branch")
            && kind == "unconditional"
        {
            kind = "conditional";
            reasons.push("conditional-callsite");
        }
    }
    (kind, reasons)
}

#[allow(clippy::too_many_arguments)]
fn derive_inter_edge_metadata(
    a: &OccurrenceWithChain,
    b: &OccurrenceWithChain,
    ka: Option<&CallLink>,
    kb: Option<&CallLink>,
    prefix_len: usize,
    coverage: &str,
    has_event_broadcast_on_path: bool,
) -> (&'static str, &'static str, Vec<&'static str>) {
    let mut condition_reasons: Vec<&'static str> = Vec::new();
    let mut condition_kind: &'static str = "unconditional";

    let mut all_links: Vec<&CallLink> = Vec::new();
    if let Some(k) = ka {
        all_links.push(k);
    }
    if let Some(k) = kb {
        all_links.push(k);
    }
    for i in (prefix_len + 1)..a.chain.links.len() {
        all_links.push(&a.chain.links[i]);
    }
    for i in (prefix_len + 1)..b.chain.links.len() {
        all_links.push(&b.chain.links[i]);
    }

    for link in &all_links {
        if link.control_placement == "loop" {
            if condition_kind != "loop-dependent" {
                condition_kind = "loop-dependent";
                condition_reasons.push("loop-callsite");
            }
        } else if link.control_placement == "conditional"
            || link.control_placement == "non-fallthrough"
        {
            if condition_kind == "unconditional" {
                condition_kind = "conditional";
                condition_reasons.push("conditional-callsite");
            }
        } else if link.control_placement == "unknown" && condition_kind == "unconditional" {
            condition_kind = "unknown";
        }
    }

    // Rule 5 §3: event-broadcast on path → push "event-subscriber-may".
    if has_event_broadcast_on_path {
        condition_reasons.push("event-subscriber-may");
    }

    let coverage_reason: &'static str = if coverage == "partial" {
        if a.chain.path_enumeration == "capped" || b.chain.path_enumeration == "capped" {
            "via-paths-capped"
        } else if a.chain.path_enumeration == "truncated" || b.chain.path_enumeration == "truncated"
        {
            "cone-truncated"
        } else if a.chain.path_enumeration == "unresolved"
            || b.chain.path_enumeration == "unresolved"
        {
            "unresolved-target"
        } else {
            "cone-truncated"
        }
    } else if condition_kind == "conditional" {
        "conditional-callsite"
    } else if condition_kind == "loop-dependent" {
        "loop-callsite"
    } else {
        "resolved-complete"
    };

    (coverage_reason, condition_kind, condition_reasons)
}

/// `orderIdInL` — returns (orderId, synthetic OrderedOp for ordering comparisons).
fn order_id_in_l(occ: &OccurrenceWithChain, prefix_len: usize) -> Option<(u32, OrderedOp)> {
    let chain_depth = occ.chain.links.len();
    if chain_depth == prefix_len {
        let op = occ.terminal_op.as_ref()?;
        return Some((op.order_id, op.clone()));
    }
    let link = occ.chain.links.get(prefix_len)?;
    let cs_order = link.callsite_order.as_ref()?;
    Some((
        cs_order.order_id,
        OrderedOp {
            occurrence_id: format!("{}@cs:{}", occ.occurrence_id, link.callsite_id),
            order_id: cs_order.order_id,
            on_success_path: cs_order.on_success_path,
            dominates_success_return: cs_order.dominates_success_return,
            frame_chain: Vec::new(),
        },
    ))
}

/// Inter-routine happens-before composition (§C). Returns an edge when `a ≺ b`
/// is provable, else None. The `coverage_reason` field of the edge is unused in
/// the order:false root labels — omitted from `InterHBEdge`.
pub fn inter_hb(
    a: &OccurrenceWithChain,
    b: &OccurrenceWithChain,
    _root_routine_id: &str,
    _snap: &CapabilitySnapshot,
) -> Option<InterHBEdge> {
    if a.occurrence_id == b.occurrence_id {
        return None;
    }

    let a_links = &a.chain.links;
    let b_links = &b.chain.links;
    let prefix_len = chain_common_prefix_length(a_links, b_links);

    // §C Case 1: both terminal in L.
    if prefix_len == a_links.len() && prefix_len == b_links.len() {
        if a.terminal_routine_id != b.terminal_routine_id {
            return None;
        }
        let (a_op, b_op) = match (&a.terminal_op, &b.terminal_op) {
            (Some(x), Some(y)) => (x, y),
            _ => return None,
        };
        if !ordered_before(a_op, b_op) {
            return None;
        }
        if !may_co_execute(a_op, b_op) {
            return None;
        }
        let quantifier = if dom(a_op, b_op) {
            Quantifier::MustAllPaths
        } else {
            Quantifier::MaySomePath
        };
        let cov = edge_coverage(&a.chain, prefix_len, &b.chain, prefix_len);
        let final_quantifier = if quantifier == Quantifier::MustAllPaths && cov == "partial" {
            Quantifier::MaySomePath
        } else {
            quantifier
        };
        let (cond_kind, cond_reasons) = derive_intra_edge_condition(a_op, b_op);
        return Some(InterHBEdge {
            from: a.occurrence_id.clone(),
            to: b.occurrence_id.clone(),
            quantifier: final_quantifier,
            coverage: cov,
            call_path_links: Vec::new(),
            edge_condition_kind: cond_kind,
            edge_condition_reasons: cond_reasons,
        });
    }

    let a_depth = a_links.len();
    let b_depth = b_links.len();
    let a_is_in_l = a_depth == prefix_len;
    let b_is_in_l = b_depth == prefix_len;

    let ka: Option<&CallLink> = if a_is_in_l {
        None
    } else {
        Some(&a_links[prefix_len])
    };
    let kb: Option<&CallLink> = if b_is_in_l {
        None
    } else {
        Some(&b_links[prefix_len])
    };

    // §C Case 4: barrier / alternative → no edge.
    if let Some(k) = ka {
        if k.dispatch_sequencing == "barrier" {
            return None;
        }
        if k.dispatch_sequencing == "alternative" {
            return None;
        }
    }
    if let Some(k) = kb {
        if k.dispatch_sequencing == "barrier" {
            return None;
        }
        if k.dispatch_sequencing == "alternative" {
            return None;
        }
    }

    // §C Rule 5 §3: unordered-broadcast handling.
    let ka_is_broadcast = ka.map(|k| k.dispatch_sequencing == "unordered-broadcast") == Some(true);
    let kb_is_broadcast = kb.map(|k| k.dispatch_sequencing == "unordered-broadcast") == Some(true);
    let mut has_event_broadcast_on_path = false;

    if ka_is_broadcast && kb_is_broadcast {
        if let (Some(ka_), Some(kb_)) = (ka, kb)
            && ka_.event_id.is_some()
            && ka_.event_id == kb_.event_id
        {
            return None;
        }
        has_event_broadcast_on_path = true;
    } else if ka_is_broadcast || kb_is_broadcast {
        has_event_broadcast_on_path = true;
    }

    let a_order_in_l = order_id_in_l(a, prefix_len)?;
    let b_order_in_l = order_id_in_l(b, prefix_len)?;

    if a_order_in_l.0 >= b_order_in_l.0 {
        return None;
    }
    if !a_order_in_l.1.on_success_path || !b_order_in_l.1.on_success_path {
        return None;
    }

    // Success-return restriction (§J2).
    let mut a_dominates_callee_return = false;
    if !a_is_in_l {
        let a_op = a.terminal_op.as_ref()?;
        a_dominates_callee_return = dominates_return(a_op);
        let a_may_precede = may_precede_success_return(a_op);
        if !a_may_precede {
            return None;
        }
    }
    if !b_is_in_l {
        let b_op = b.terminal_op.as_ref()?;
        if !may_precede_success_return(b_op) {
            return None;
        }
    }

    // Determine quantifier (§J1 / §J4).
    let mut is_must = true;
    if has_event_broadcast_on_path {
        is_must = false;
    }
    if a.chain.path_enumeration != "complete" || b.chain.path_enumeration != "complete" {
        is_must = false;
    }

    if !a_is_in_l {
        if let Some(k) = ka {
            if k.control_placement != "top-level"
                || k.invocation_multiplicity != "exactly_once_on_success"
                || k.callsite_order
                    .as_ref()
                    .map(|o| o.dominates_success_return)
                    != Some(true)
            {
                is_must = false;
            }
            if !a_dominates_callee_return {
                is_must = false;
            }
        }
    } else {
        match &a.terminal_op {
            None => is_must = false,
            Some(a_op) => {
                let a_has_conditional = a_op.frame_chain.iter().any(|f| {
                    f.kind == "if-then"
                        || f.kind == "if-else"
                        || f.kind == "case-branch"
                        || f.kind == "loop"
                });
                if a_has_conditional {
                    is_must = false;
                }
            }
        }
    }

    if !b_is_in_l
        && let Some(k) = kb
        && (k.control_placement != "top-level"
            || k.invocation_multiplicity != "exactly_once_on_success"
            || k.callsite_order
                .as_ref()
                .map(|o| o.dominates_success_return)
                != Some(true))
    {
        is_must = false;
    }

    // callPathLinks.
    let mut call_path_links: Vec<String> = Vec::new();
    if let Some(k) = ka {
        call_path_links.push(k.callsite_id.clone());
    }
    if let Some(k) = kb
        && Some(&k.callsite_id) != ka.map(|x| &x.callsite_id)
    {
        call_path_links.push(k.callsite_id.clone());
    }

    // Intermediate links on a's path.
    for link in a_links.iter().skip(prefix_len + 1) {
        if link.dispatch_sequencing != "sequential" {
            is_must = false;
            if link.dispatch_sequencing == "barrier" {
                return None;
            }
            if link.dispatch_sequencing == "alternative" {
                return None;
            }
            if link.dispatch_sequencing == "unordered-broadcast" {
                has_event_broadcast_on_path = true;
            }
        }
        if link.control_placement != "top-level"
            || link
                .callsite_order
                .as_ref()
                .map(|o| o.dominates_success_return)
                != Some(true)
        {
            is_must = false;
        }
    }
    // Intermediate links on b's path.
    for link in b_links.iter().skip(prefix_len + 1) {
        if link.dispatch_sequencing != "sequential" {
            is_must = false;
            if link.dispatch_sequencing == "barrier" {
                return None;
            }
            if link.dispatch_sequencing == "alternative" {
                return None;
            }
            if link.dispatch_sequencing == "unordered-broadcast" {
                has_event_broadcast_on_path = true;
            }
        }
        if link.control_placement != "top-level" {
            is_must = false;
        }
    }

    let cov = edge_coverage(&a.chain, a_depth, &b.chain, b_depth);

    let mut quantifier = if is_must {
        Quantifier::MustAllPaths
    } else {
        Quantifier::MaySomePath
    };
    if quantifier == Quantifier::MustAllPaths && cov == "partial" {
        quantifier = Quantifier::MaySomePath;
    }

    let (_coverage_reason, edge_condition_kind, edge_condition_reasons) =
        derive_inter_edge_metadata(a, b, ka, kb, prefix_len, cov, has_event_broadcast_on_path);

    Some(InterHBEdge {
        from: a.occurrence_id.clone(),
        to: b.occurrence_id.clone(),
        quantifier,
        coverage: cov,
        call_path_links,
        edge_condition_kind,
        edge_condition_reasons,
    })
}

/// crossHopDominatesRootReturn (ordering-inter.ts).
pub fn cross_hop_dominates_root_return(
    occ_a: &OccurrenceWithChain,
    root_routine_id: &str,
    snap: &CapabilitySnapshot,
    callsite_by_id: &CallsiteByIdMap,
) -> bool {
    let Some(op) = occ_a.terminal_op.as_ref() else {
        return false;
    };
    if occ_a.chain.path_enumeration != "complete" {
        return false;
    }
    if !op.dominates_success_return {
        return false;
    }
    for link in &occ_a.chain.links {
        if link.dispatch_sequencing != "sequential" {
            return false;
        }
        if link.control_placement != "top-level" {
            return false;
        }
        if link.invocation_multiplicity != "exactly_once_on_success" {
            return false;
        }
        if link
            .callsite_order
            .as_ref()
            .map(|o| o.dominates_success_return)
            != Some(true)
        {
            return false;
        }
    }
    let op_order = OperationOrder {
        order_id: op.order_id,
        frame_id: 0,
        on_success_path: op.on_success_path,
        dominates_success_return: op.dominates_success_return,
    };
    op_dominates_root_return(
        &occ_a.terminal_routine_id,
        &op_order,
        root_routine_id,
        snap,
        callsite_by_id,
    )
}

// ---------------------------------------------------------------------------
// error-escapes-chain.ts
// ---------------------------------------------------------------------------

/// `errorEscapesChain` — true ONLY when the terminal error is provably uncaught
/// through every hop (fail-closed). `evidence_operation_id` drives the B1 check.
pub fn error_escapes_chain(
    evidence_operation_id: Option<&str>,
    occ_chain: &OccurrenceWithChain,
    routine_return_summaries: Option<&HashMap<String, RoutineReturnSummary>>,
    snap: &CapabilitySnapshot,
) -> bool {
    // B12.
    if occ_chain.terminal_op.is_none() {
        return false;
    }
    // B1: underAsserterror on the terminal op.
    if let Some(op_id) = evidence_operation_id
        && let Some(op_ev) = snap
            .operation_index
            .iter()
            .find(|o| o.operation_id == op_id)
        && op_ev.under_asserterror == Some(true)
    {
        return false;
    }
    // B10 global floor.
    let Some(summaries) = routine_return_summaries else {
        return false;
    };
    // B11.
    if occ_chain.chain.path_enumeration != "complete" {
        return false;
    }
    // Collect routine ids.
    let mut routine_ids: HashSet<String> = HashSet::new();
    routine_ids.insert(occ_chain.terminal_routine_id.clone());
    for link in &occ_chain.chain.links {
        routine_ids.insert(link.caller_routine_id.clone());
        routine_ids.insert(link.callee_routine_id.clone());
    }
    for rid in &routine_ids {
        let Some(summary) = summaries.get(rid) else {
            return false; // B10 per-routine
        };
        if summary.has_try_function_boundary {
            return false; // B2/B3
        }
        if summary.has_error_behavior_collect {
            return false; // B4/B5
        }
    }
    // B13/B14 + B6/B7/B8/B9 on links.
    let mut cs_res_by_id: HashMap<&str, &SnapshotCallsiteResolution> = HashMap::new();
    for cr in &snap.callsite_resolutions {
        cs_res_by_id.insert(cr.callsite_id.as_str(), cr);
    }
    for link in &occ_chain.chain.links {
        if link.resolution_status != "resolved" {
            return false; // B9
        }
        if link.dispatch_sequencing == "unordered-broadcast" {
            return false; // B6
        }
        if link.dispatch_sequencing == "barrier" {
            return false; // B7
        }
        if link.dispatch_sequencing == "alternative" {
            return false; // B8
        }
        if let Some(cr) = cs_res_by_id.get(link.callsite_id.as_str()) {
            if cr.under_asserterror == Some(true) {
                return false; // B13
            }
            if cr.result_consumed == Some(true) {
                return false; // B14
            }
        }
    }
    true
}

// ---------------------------------------------------------------------------
// io-direction.ts
// ---------------------------------------------------------------------------

/// `ioDirection(type, detail)` → "read" | "write" | "unknown".
pub fn io_direction(effect_type: &str, method: &str, file_op: &str) -> &'static str {
    if effect_type == "HTTP" {
        let m = method.to_ascii_uppercase();
        if m == "POST" || m == "PUT" || m == "PATCH" || m == "DELETE" {
            return "write";
        }
        if m == "GET" || m == "HEAD" {
            return "read";
        }
        return "unknown";
    }
    if effect_type == "FILE" {
        if file_op == "write-blob" {
            return "write";
        }
        return "unknown";
    }
    "unknown"
}

// ===========================================================================
// Native oracles — 4b substrate.
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::l5::snapshot::SnapshotIdentityTable;
    use crate::engine::return_summary::RoutineReturnSummary;
    use serde_json::Value as JsonValue;

    fn empty_snap() -> CapabilitySnapshot {
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

    fn summary(try_fn: bool, collect: bool) -> RoutineReturnSummary {
        RoutineReturnSummary {
            has_normal_return_path: JsonValue::Bool(true),
            all_paths_error: JsonValue::Bool(false),
            has_try_function_boundary: try_fn,
            has_error_behavior_collect: collect,
            coverage: "resolved",
            commit_behavior: "normal",
        }
    }

    fn occ_with_chain_root(routine: &str) -> OccurrenceWithChain {
        OccurrenceWithChain {
            occurrence_id: "err1".to_string(),
            terminal_routine_id: routine.to_string(),
            terminal_op: Some(OrderedOp {
                occurrence_id: "err1".to_string(),
                order_id: 5,
                on_success_path: false,
                dominates_success_return: false,
                frame_chain: vec![],
            }),
            terminal_routine_ops: vec![],
            chain: CallChain {
                links: vec![],
                path_enumeration: "complete",
            },
        }
    }

    #[test]
    fn io_direction_http_methods() {
        assert_eq!(io_direction("HTTP", "POST", ""), "write");
        assert_eq!(io_direction("HTTP", "put", ""), "write");
        assert_eq!(io_direction("HTTP", "GET", ""), "read");
        assert_eq!(io_direction("HTTP", "OPTIONS", ""), "unknown");
        assert_eq!(io_direction("FILE", "", "write-blob"), "write");
        assert_eq!(io_direction("FILE", "", "open"), "unknown");
    }

    #[test]
    fn error_escapes_chain_tryfunction_barrier_suppresses() {
        // B2: terminal routine has a TryFunction boundary → error does NOT escape.
        let snap = empty_snap();
        let occ = occ_with_chain_root("rootR");
        let mut summaries: HashMap<String, RoutineReturnSummary> = HashMap::new();
        summaries.insert("rootR".to_string(), summary(true, false));
        assert!(!error_escapes_chain(
            Some("op1"),
            &occ,
            Some(&summaries),
            &snap
        ));

        // Clean summary (no barriers) → error escapes.
        let mut clean: HashMap<String, RoutineReturnSummary> = HashMap::new();
        clean.insert("rootR".to_string(), summary(false, false));
        assert!(error_escapes_chain(Some("op1"), &occ, Some(&clean), &snap));

        // B10 floor: missing summary → fail-closed (no escape).
        assert!(!error_escapes_chain(Some("op1"), &occ, None, &snap));
    }

    // -------------------------------------------------------------------------
    // Helpers shared across the new inter_hb / derive_* oracles.
    // -------------------------------------------------------------------------

    /// Build an `OrderedOp` with explicit frame chain for intra-level use.
    fn ordered_op(
        id: &str,
        order: u32,
        dom_ret: bool,
        frames: Vec<crate::engine::l2::operation_order::ScopeFrame>,
    ) -> crate::engine::l5::ordering::OrderedOp {
        crate::engine::l5::ordering::OrderedOp {
            occurrence_id: id.to_string(),
            order_id: order,
            on_success_path: true,
            dominates_success_return: dom_ret,
            frame_chain: frames,
        }
    }

    fn frame(
        id: i64,
        parent: i64,
        kind: &str,
        fall_through: Option<bool>,
    ) -> crate::engine::l2::operation_order::ScopeFrame {
        crate::engine::l2::operation_order::ScopeFrame {
            frame_id: id,
            parent_frame_id: parent,
            kind: kind.to_string(),
            branch_always_terminates: None,
            branch_may_fall_through: fall_through,
            branch_terminates_by: None,
        }
    }

    /// Build a minimal `CallLink` with the given dispatchSequencing/controlPlacement.
    #[allow(clippy::too_many_arguments)]
    fn call_link(
        caller: &str,
        callee: &str,
        cs_id: &str,
        order_id: u32,
        dom_ret: bool,
        dispatch: &'static str,
        placement: &'static str,
        multiplicity: &'static str,
        event_id: Option<String>,
    ) -> CallLink {
        CallLink {
            caller_routine_id: caller.to_string(),
            callee_routine_id: callee.to_string(),
            callsite_id: cs_id.to_string(),
            callsite_order: Some(CallsiteOrder {
                order_id,
                frame_id: 0,
                on_success_path: true,
                dominates_success_return: dom_ret,
            }),
            hop_kind: "call".to_string(),
            resolution_status: "resolved".to_string(),
            dispatch_sequencing: dispatch,
            control_placement: placement,
            invocation_multiplicity: multiplicity,
            event_id,
        }
    }

    /// Build an `OccurrenceWithChain` for a single-level (intra) occurrence.
    fn occ_intra(
        id: &str,
        routine: &str,
        op: crate::engine::l5::ordering::OrderedOp,
    ) -> OccurrenceWithChain {
        OccurrenceWithChain {
            occurrence_id: id.to_string(),
            terminal_routine_id: routine.to_string(),
            terminal_op: Some(op),
            terminal_routine_ops: vec![],
            chain: CallChain {
                links: vec![],
                path_enumeration: "complete",
            },
        }
    }

    /// Build an `OccurrenceWithChain` with a one-hop chain.
    fn occ_one_hop(
        id: &str,
        _root_routine: &str,
        callee_routine: &str,
        link: CallLink,
        op: crate::engine::l5::ordering::OrderedOp,
    ) -> OccurrenceWithChain {
        OccurrenceWithChain {
            occurrence_id: id.to_string(),
            terminal_routine_id: callee_routine.to_string(),
            terminal_op: Some(op),
            terminal_routine_ops: vec![],
            chain: CallChain {
                links: vec![link],
                path_enumeration: "complete",
            },
        }
    }

    // =========================================================================
    // Oracle 1: derive_dispatch_sequencing_from_edge — hopKind × ledgerStatus matrix.
    //
    // Mirrors al-sem `call-chain-helpers.ts` lines 64-104.
    // =========================================================================

    #[test]
    fn derive_dispatch_sequencing_matrix() {
        // event-dispatch / implicit-trigger → always unordered-broadcast (line 69-71).
        assert_eq!(
            derive_dispatch_sequencing_from_edge("event-dispatch", None),
            "unordered-broadcast"
        );
        assert_eq!(
            derive_dispatch_sequencing_from_edge("implicit-trigger", Some("resolved")),
            "unordered-broadcast"
        );

        // interface-dispatch → always alternative (line 74).
        assert_eq!(
            derive_dispatch_sequencing_from_edge("interface-dispatch", Some("resolved")),
            "alternative"
        );

        // dependency-export → always barrier (line 79).
        assert_eq!(
            derive_dispatch_sequencing_from_edge("dependency-export", Some("resolved")),
            "barrier"
        );

        // call + resolved → sequential (line 86).
        assert_eq!(
            derive_dispatch_sequencing_from_edge("call", Some("resolved")),
            "sequential"
        );

        // call + polymorphic → alternative (line 88).
        assert_eq!(
            derive_dispatch_sequencing_from_edge("call", Some("polymorphic")),
            "alternative"
        );

        // call + object-run-unresolved ledger status → barrier (line 90-99).
        assert_eq!(
            derive_dispatch_sequencing_from_edge("call", Some("unresolved-member")),
            "barrier"
        );
        assert_eq!(
            derive_dispatch_sequencing_from_edge("call", Some("ambiguous")),
            "barrier"
        );
        assert_eq!(
            derive_dispatch_sequencing_from_edge("call", Some("external")),
            "barrier"
        );
        assert_eq!(
            derive_dispatch_sequencing_from_edge("call", Some("builtin")),
            "barrier"
        );

        // variable-typed-call + resolved → sequential.
        assert_eq!(
            derive_dispatch_sequencing_from_edge("variable-typed-call", Some("resolved")),
            "sequential"
        );

        // object-run + resolved → sequential.
        assert_eq!(
            derive_dispatch_sequencing_from_edge("object-run", Some("resolved")),
            "sequential"
        );

        // unknown hopKind → barrier (line 102).
        assert_eq!(
            derive_dispatch_sequencing_from_edge("unknown-kind", Some("resolved")),
            "barrier"
        );

        // None ledger status on call → barrier (default arm, line 101).
        assert_eq!(
            derive_dispatch_sequencing_from_edge("call", None),
            "barrier"
        );
    }

    // =========================================================================
    // Oracle 2: derive_control_placement_from_frame_chain — precedence tier check.
    //
    // Mirrors al-sem `call-chain-helpers.ts` lines 30-52.
    // Priority: loop > non-fallthrough > conditional > top-level > unknown (empty).
    // =========================================================================

    #[test]
    fn derive_control_placement_precedence() {
        // Empty chain → "unknown" (line 31).
        assert_eq!(derive_control_placement_from_frame_chain(&[]), "unknown");

        // Root block only → "top-level" (line 51).
        assert_eq!(
            derive_control_placement_from_frame_chain(&[frame(0, -1, "root", None)]),
            "top-level"
        );

        // Selector present → "conditional" (line 50).
        assert_eq!(
            derive_control_placement_from_frame_chain(&[
                frame(0, -1, "root", None),
                frame(1, 0, "if-then", Some(true))
            ]),
            "conditional"
        );

        // Non-fallthrough selector → "non-fallthrough" beats conditional (line 48-49).
        assert_eq!(
            derive_control_placement_from_frame_chain(&[
                frame(0, -1, "root", None),
                frame(1, 0, "if-then", Some(false)) // branchMayFallThrough=false
            ]),
            "non-fallthrough"
        );

        // Loop frame present → "loop" beats everything (line 47).
        assert_eq!(
            derive_control_placement_from_frame_chain(&[
                frame(0, -1, "root", None),
                frame(1, 0, "loop", None)
            ]),
            "loop"
        );

        // Loop beats non-fallthrough.
        assert_eq!(
            derive_control_placement_from_frame_chain(&[
                frame(0, -1, "root", None),
                frame(1, 0, "if-then", Some(false)),
                frame(2, 1, "loop", None)
            ]),
            "loop"
        );

        // case-branch (selector kind) → "conditional".
        assert_eq!(
            derive_control_placement_from_frame_chain(&[
                frame(0, -1, "root", None),
                frame(1, 0, "case-branch", Some(true))
            ]),
            "conditional"
        );
    }

    // =========================================================================
    // Oracle 3: derive_invocation_multiplicity — controlPlacement × domRet.
    //
    // Mirrors al-sem `call-chain-helpers.ts` lines 114-129.
    // =========================================================================

    #[test]
    fn derive_invocation_multiplicity_cases() {
        // top-level + dominates → "exactly_once_on_success" (line 120).
        assert_eq!(
            derive_invocation_multiplicity("top-level", true),
            "exactly_once_on_success"
        );

        // top-level + NOT dominates → "zero_or_one" (line 122).
        assert_eq!(
            derive_invocation_multiplicity("top-level", false),
            "zero_or_one"
        );

        // loop → "zero_or_more" (line 124).
        assert_eq!(
            derive_invocation_multiplicity("loop", false),
            "zero_or_more"
        );
        assert_eq!(derive_invocation_multiplicity("loop", true), "zero_or_more");

        // conditional → "zero_or_one" (line 125).
        assert_eq!(
            derive_invocation_multiplicity("conditional", true),
            "zero_or_one"
        );

        // non-fallthrough → "zero_or_one" (line 126).
        assert_eq!(
            derive_invocation_multiplicity("non-fallthrough", true),
            "zero_or_one"
        );

        // unknown → "unknown" (line 127).
        assert_eq!(derive_invocation_multiplicity("unknown", true), "unknown");
    }

    // =========================================================================
    // Oracle 4a: inter_hb §C Case 1 — both terminal in L (same routine).
    //
    // Mirrors al-sem `ordering-inter.ts` lines 379-419.
    //
    // Sub-path 4a-i: dom(a,b) → MustAllPaths.
    // Sub-path 4a-ii: orderedBefore+mayCoExecute but NOT dom → MaySomePath.
    // Sub-path 4a-iii: different terminalRoutineId → None (fail-closed, line 387).
    // =========================================================================

    #[test]
    fn inter_hb_case1_same_routine_dom_gives_must() {
        // Both ops at root block, a before b, a has no enclosing frames → dom holds.
        // Expected: MustAllPaths (ordering-inter.ts line 397).
        let a_op = ordered_op("a", 1, true, vec![frame(0, -1, "root", None)]);
        let b_op = ordered_op("b", 2, true, vec![frame(0, -1, "root", None)]);
        let snap = empty_snap();

        let a = occ_intra("a", "routineA", a_op);
        let b = occ_intra("b", "routineA", b_op);

        let edge = inter_hb(&a, &b, "routineA", &snap).expect("should produce edge");
        assert_eq!(edge.quantifier, Quantifier::MustAllPaths);
        assert_eq!(edge.call_path_links.len(), 0);
    }

    #[test]
    fn inter_hb_case1_ordered_not_dom_gives_may() {
        // a inside an if-then, b at root. orderedBefore holds but dom(a,b) does NOT
        // (b is not dominated by a because a has extra frames beyond a's depth).
        // Expected: MaySomePath (ordering-inter.ts line 397 else branch).
        let a_op = ordered_op(
            "a",
            1,
            false,
            vec![
                frame(0, -1, "root", None),
                frame(1, 0, "if-then", Some(true)),
            ],
        );
        let b_op = ordered_op("b", 2, true, vec![frame(0, -1, "root", None)]);
        let snap = empty_snap();

        // Note: may_co_execute must hold — a is in if-then (fall-through=true) so not
        // mutually exclusive with b.
        let a = occ_intra("a", "routineX", a_op);
        let b = occ_intra("b", "routineX", b_op);

        let edge = inter_hb(&a, &b, "routineX", &snap).expect("should produce edge");
        assert_eq!(edge.quantifier, Quantifier::MaySomePath);
    }

    #[test]
    fn inter_hb_case1_different_terminal_routine_gives_none() {
        // Same chain depth (0 links each) but different terminalRoutineId.
        // Fail-closed guard (ordering-inter.ts line 387): must return None.
        let a_op = ordered_op("a", 1, true, vec![frame(0, -1, "root", None)]);
        let b_op = ordered_op("b", 2, true, vec![frame(0, -1, "root", None)]);
        let snap = empty_snap();

        let a = occ_intra("a", "routineA", a_op);
        let b = occ_intra("b", "routineB", b_op); // DIFFERENT terminal routine

        let edge = inter_hb(&a, &b, "routineA", &snap);
        assert!(
            edge.is_none(),
            "different terminal routines with empty chains must be fail-closed → None"
        );
    }

    // =========================================================================
    // Oracle 4b: inter_hb cross-hop divergent chains — quantifier degradation.
    //
    // Mirrors al-sem `ordering-inter.ts` lines 590-693.
    //
    // Sub-path 4b-i: top-level/sequential/exactly_once link → MustAllPaths.
    // Sub-path 4b-ii: conditional placement link → degrades to MaySomePath.
    // =========================================================================

    #[test]
    fn inter_hb_cross_hop_top_level_sequential_gives_must() {
        // a: root → callee1 via cs1 (top-level, sequential, exactly_once, dom).
        //    terminal op at order 1 in callee1 with no enclosing frames, dominates ret.
        // b: root → callee2 via cs2 (top-level, sequential, exactly_once, dom).
        //    terminal op at order 5 in callee2.
        // cs1.orderId=2, cs2.orderId=4 → cs1 strictly before cs2 in root.
        // Expect: MustAllPaths (all top-level, complete, no broadcast).
        let snap = empty_snap();

        let ka = call_link(
            "root",
            "callee1",
            "cs1",
            2,
            true,
            "sequential",
            "top-level",
            "exactly_once_on_success",
            None,
        );
        let kb = call_link(
            "root",
            "callee2",
            "cs2",
            4,
            true,
            "sequential",
            "top-level",
            "exactly_once_on_success",
            None,
        );

        let a_op = ordered_op("aOp", 1, true, vec![frame(0, -1, "root", None)]);
        let b_op = ordered_op("bOp", 5, true, vec![frame(0, -1, "root", None)]);

        let a = occ_one_hop("aOcc", "root", "callee1", ka, a_op);
        let b = occ_one_hop("bOcc", "root", "callee2", kb, b_op);

        let edge = inter_hb(&a, &b, "root", &snap).expect("should produce edge");
        assert_eq!(
            edge.quantifier,
            Quantifier::MustAllPaths,
            "top-level/sequential/exactly_once cross-hop → MustAllPaths"
        );
    }

    #[test]
    fn inter_hb_cross_hop_conditional_placement_degrades_to_may() {
        // Same as above but ka has controlPlacement="conditional".
        // Per ordering-inter.ts line 605: controlPlacement != "top-level" → isMust=false.
        // Expect: MaySomePath.
        let snap = empty_snap();

        let ka = call_link(
            "root",
            "callee1",
            "cs1",
            2,
            true,
            "sequential",
            "conditional", // NOT top-level → degrades
            "zero_or_one",
            None,
        );
        let kb = call_link(
            "root",
            "callee2",
            "cs2",
            4,
            true,
            "sequential",
            "top-level",
            "exactly_once_on_success",
            None,
        );

        let a_op = ordered_op("aOp", 1, true, vec![frame(0, -1, "root", None)]);
        let b_op = ordered_op("bOp", 5, true, vec![frame(0, -1, "root", None)]);

        let a = occ_one_hop("aOcc", "root", "callee1", ka, a_op);
        let b = occ_one_hop("bOcc", "root", "callee2", kb, b_op);

        let edge = inter_hb(&a, &b, "root", &snap).expect("should produce edge");
        assert_eq!(
            edge.quantifier,
            Quantifier::MaySomePath,
            "conditional placement on diverging link → degrades to MaySomePath"
        );
    }

    // =========================================================================
    // Oracle 4c: inter_hb — both-broadcast same eventId → None.
    //
    // Mirrors al-sem `ordering-inter.ts` lines 503-514.
    // Both ka and kb are unordered-broadcast, same eventId → cross-subscriber
    // pair from the SAME dispatch → no edge.
    // =========================================================================

    #[test]
    fn inter_hb_both_broadcast_same_event_gives_none() {
        let snap = empty_snap();

        let ka = call_link(
            "root",
            "sub1",
            "cs1",
            2,
            true,
            "unordered-broadcast",
            "top-level",
            "zero_or_one",
            Some("evt::OnPost".to_string()),
        );
        let kb = call_link(
            "root",
            "sub2",
            "cs2",
            4,
            true,
            "unordered-broadcast",
            "top-level",
            "zero_or_one",
            Some("evt::OnPost".to_string()), // SAME event id
        );

        let a_op = ordered_op("aOp", 1, true, vec![frame(0, -1, "root", None)]);
        let b_op = ordered_op("bOp", 5, true, vec![frame(0, -1, "root", None)]);

        let a = occ_one_hop("aOcc", "root", "sub1", ka, a_op);
        let b = occ_one_hop("bOcc", "root", "sub2", kb, b_op);

        let edge = inter_hb(&a, &b, "root", &snap);
        assert!(
            edge.is_none(),
            "both broadcast with same eventId (cross-subscriber) → no edge"
        );
    }

    // =========================================================================
    // Oracle 4d: inter_hb — one-side unordered-broadcast → edge present,
    //            edge_condition_reasons contains "event-subscriber-may", is_must=false.
    //
    // Mirrors al-sem `ordering-inter.ts` lines 516-521 (asymmetric broadcast).
    // One side is a publisher (sequential call → broadcast dispatch), other side
    // is a sequential callee. The directional ordering holds but isMust is forced
    // false (line 594-596) and "event-subscriber-may" is pushed (line 251-254).
    // =========================================================================

    #[test]
    fn inter_hb_one_side_broadcast_gives_may_with_event_subscriber_reason() {
        let snap = empty_snap();

        // ka is unordered-broadcast (e.g. publisher emitting event).
        let ka = call_link(
            "root",
            "subscriber1",
            "cs1",
            2,
            true,
            "unordered-broadcast",
            "top-level",
            "zero_or_one",
            Some("evt::OnCommit".to_string()),
        );
        // kb is sequential (normal callee after the event dispatch).
        let kb = call_link(
            "root",
            "callee2",
            "cs2",
            4,
            true,
            "sequential",
            "top-level",
            "exactly_once_on_success",
            None,
        );

        let a_op = ordered_op("aOp", 1, true, vec![frame(0, -1, "root", None)]);
        let b_op = ordered_op("bOp", 5, true, vec![frame(0, -1, "root", None)]);

        let a = occ_one_hop("aOcc", "root", "subscriber1", ka, a_op);
        let b = occ_one_hop("bOcc", "root", "callee2", kb, b_op);

        let edge =
            inter_hb(&a, &b, "root", &snap).expect("one-side broadcast should still produce edge");
        // Must be MAY (never MUST through a broadcast path).
        assert_eq!(
            edge.quantifier,
            Quantifier::MaySomePath,
            "asymmetric broadcast forces MaySomePath (ordering-inter.ts line 594)"
        );
        assert!(
            edge.edge_condition_reasons
                .contains(&"event-subscriber-may"),
            "edge_condition_reasons must contain 'event-subscriber-may' (ordering-inter.ts line 251-254)"
        );
    }

    // =========================================================================
    // Oracle 4e: inter_hb — barrier/alternative on divergent link → None.
    //
    // Mirrors al-sem `ordering-inter.ts` lines 471-487.
    // =========================================================================

    #[test]
    fn inter_hb_barrier_on_diverging_link_gives_none() {
        let snap = empty_snap();

        // ka is a barrier (e.g. object-run unresolved).
        let ka = call_link(
            "root",
            "callee1",
            "cs1",
            2,
            true,
            "barrier", // barrier → no edge (line 473)
            "top-level",
            "exactly_once_on_success",
            None,
        );
        let kb = call_link(
            "root",
            "callee2",
            "cs2",
            4,
            true,
            "sequential",
            "top-level",
            "exactly_once_on_success",
            None,
        );

        let a_op = ordered_op("aOp", 1, true, vec![frame(0, -1, "root", None)]);
        let b_op = ordered_op("bOp", 5, true, vec![frame(0, -1, "root", None)]);

        let a = occ_one_hop("aOcc", "root", "callee1", ka, a_op);
        let b = occ_one_hop("bOcc", "root", "callee2", kb, b_op);

        let edge = inter_hb(&a, &b, "root", &snap);
        assert!(edge.is_none(), "barrier on ka → no edge");
    }

    #[test]
    fn inter_hb_alternative_on_diverging_link_gives_none() {
        let snap = empty_snap();

        // kb is alternative (interface-dispatch).
        let ka = call_link(
            "root",
            "callee1",
            "cs1",
            2,
            true,
            "sequential",
            "top-level",
            "exactly_once_on_success",
            None,
        );
        let kb = call_link(
            "root",
            "callee2",
            "cs2",
            4,
            true,
            "alternative", // alternative → no edge (line 486)
            "top-level",
            "zero_or_one",
            None,
        );

        let a_op = ordered_op("aOp", 1, true, vec![frame(0, -1, "root", None)]);
        let b_op = ordered_op("bOp", 5, true, vec![frame(0, -1, "root", None)]);

        let a = occ_one_hop("aOcc", "root", "callee1", ka, a_op);
        let b = occ_one_hop("bOcc", "root", "callee2", kb, b_op);

        let edge = inter_hb(&a, &b, "root", &snap);
        assert!(edge.is_none(), "alternative on kb → no edge");
    }
}

//! R4-F Stage-4a — intra-routine happens-before (HB) ordering engine.
//!
//! Byte-parity port of al-sem `src/digest/ordering.ts`.
//!
//! Computes ordering predicates over a pair of `OrderedOp`s from the SAME
//! routine, using the persisted ScopeFrame chain from the snapshot.
//!
//! Cardinal rule: emit an order edge ONLY when order is provable on a resolved
//! path. Absence of an edge means ordering UNKNOWN — never assume.
//!
//! ALL ordering decisions compare integer `orderId` / `frameId` — no string
//! compares (determinism, BINDING).

use crate::program::body::operation_order::ScopeFrame;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// A single operation occurrence enriched with its execution-order data.
/// `frameChain` is root-first, innermost-last.
#[derive(Debug, Clone)]
pub struct OrderedOp {
    pub occurrence_id: String,
    pub order_id: u32,
    pub on_success_path: bool,
    /// Ancestor frame chain (root..innermost) from the snapshot.
    pub frame_chain: Vec<ScopeFrame>,
    /// Precomputed postdominance fact from `OperationOrder.dominatesSuccessReturn`.
    pub dominates_success_return: bool,
}

/// Edge quantifier for a happens-before edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Quantifier {
    MustAllPaths,
    MaySomePath,
}

/// A directed happens-before edge between two operation occurrences.
#[derive(Debug, Clone)]
pub struct HBEdge {
    pub from: String,
    pub to: String,
    pub quantifier: Quantifier,
    pub coverage: &'static str, // "resolved" | "partial"
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

fn is_selector_kind(kind: &str) -> bool {
    kind == "if-then" || kind == "if-else" || kind == "case-branch"
}

/// First index where the two chains diverge (frameIds differ); or the length of
/// the shorter chain if one is a prefix of the other.
fn common_prefix_length(a: &[ScopeFrame], b: &[ScopeFrame]) -> usize {
    let len = a.len().min(b.len());
    for i in 0..len {
        if a[i].frame_id != b[i].frame_id {
            return i;
        }
    }
    len
}

// ---------------------------------------------------------------------------
// Core predicates
// ---------------------------------------------------------------------------

/// `mayCoExecute(a, b)` — can a and b both execute on the same run?
pub fn may_co_execute(a: &OrderedOp, b: &OrderedOp) -> bool {
    let a_chain = &a.frame_chain;
    let b_chain = &b.frame_chain;

    if a.occurrence_id == b.occurrence_id {
        return false;
    }

    let prefix_len = common_prefix_length(a_chain, b_chain);

    // (i) Mutual exclusion: divergent siblings both selector kinds.
    if prefix_len < a_chain.len() && prefix_len < b_chain.len() {
        let a_frame = &a_chain[prefix_len];
        let b_frame = &b_chain[prefix_len];
        if is_selector_kind(&a_frame.kind) && is_selector_kind(&b_frame.kind) {
            return false;
        }
    }

    // (ii) Non-fallthrough exclusion.
    let b_ids: std::collections::HashSet<i64> = b_chain.iter().map(|f| f.frame_id).collect();
    for f in a_chain {
        if is_selector_kind(&f.kind)
            && f.branch_may_fall_through == Some(false)
            && !b_ids.contains(&f.frame_id)
        {
            return false;
        }
    }
    let a_ids: std::collections::HashSet<i64> = a_chain.iter().map(|f| f.frame_id).collect();
    for f in b_chain {
        if is_selector_kind(&f.kind)
            && f.branch_may_fall_through == Some(false)
            && !a_ids.contains(&f.frame_id)
        {
            return false;
        }
    }

    true
}

/// `orderedBefore(a, b)` — a sequentially ordered before b (not loop-carried).
pub fn ordered_before(a: &OrderedOp, b: &OrderedOp) -> bool {
    if a.order_id >= b.order_id {
        return false;
    }
    if a.occurrence_id == b.occurrence_id {
        return false;
    }
    // Loop-carried: shared `loop` frame → no intra-iteration edge.
    let a_ids: std::collections::HashSet<i64> = a.frame_chain.iter().map(|f| f.frame_id).collect();
    for f in &b.frame_chain {
        if f.kind == "loop" && a_ids.contains(&f.frame_id) {
            return false;
        }
    }
    true
}

/// `dom(a, b)` — does a dominate b?
pub fn dom(a: &OrderedOp, b: &OrderedOp) -> bool {
    if a.order_id >= b.order_id {
        return false;
    }
    if a.occurrence_id == b.occurrence_id {
        return false;
    }
    let a_chain = &a.frame_chain;
    let b_chain = &b.frame_chain;
    if a_chain.len() > b_chain.len() {
        return false;
    }
    for i in 0..a_chain.len() {
        if a_chain[i].frame_id != b_chain[i].frame_id {
            return false;
        }
    }
    for frame in b_chain.iter().skip(a_chain.len()) {
        let kind = &frame.kind;
        if is_selector_kind(kind) || kind == "loop" {
            return false;
        }
    }
    true
}

/// `dominatesReturn(a)` — uses the precomputed sound postdominance fact.
pub fn dominates_return(a: &OrderedOp) -> bool {
    a.dominates_success_return
}

/// `mayPrecedeSuccessReturn(a)` — can a execute before a successful return?
pub fn may_precede_success_return(a: &OrderedOp) -> bool {
    a.on_success_path
}

/// Compute the HB quantifier for an emitted edge.
pub fn edge_quantifier(a: &OrderedOp, b: &OrderedOp) -> Quantifier {
    if dom(a, b) {
        Quantifier::MustAllPaths
    } else {
        Quantifier::MaySomePath
    }
}

/// Build the full list of happens-before edges among a set of `OrderedOp`s.
pub fn build_hb_edges(ops: &[OrderedOp]) -> Vec<HBEdge> {
    let mut edges: Vec<HBEdge> = Vec::new();
    for i in 0..ops.len() {
        for j in (i + 1)..ops.len() {
            let a = &ops[i];
            let b = &ops[j];
            if ordered_before(a, b) && may_co_execute(a, b) {
                edges.push(HBEdge {
                    from: a.occurrence_id.clone(),
                    to: b.occurrence_id.clone(),
                    quantifier: edge_quantifier(a, b),
                    coverage: "resolved",
                });
            } else if ordered_before(b, a) && may_co_execute(b, a) {
                edges.push(HBEdge {
                    from: b.occurrence_id.clone(),
                    to: a.occurrence_id.clone(),
                    quantifier: edge_quantifier(b, a),
                    coverage: "resolved",
                });
            }
        }
    }
    edges
}

// ===========================================================================
// Native oracles — 4a HB predicates on hand-built OrderedOps.
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(id: i64, parent: i64, kind: &str, fall_through: Option<bool>) -> ScopeFrame {
        ScopeFrame {
            frame_id: id,
            parent_frame_id: parent,
            kind: kind.to_string(),
            branch_always_terminates: None,
            branch_may_fall_through: fall_through,
            branch_terminates_by: None,
        }
    }

    fn root_frame() -> ScopeFrame {
        frame(0, -1, "root", None)
    }

    fn op(id: &str, order: u32, frames: Vec<ScopeFrame>) -> OrderedOp {
        OrderedOp {
            occurrence_id: id.to_string(),
            order_id: order,
            on_success_path: true,
            frame_chain: frames,
            dominates_success_return: false,
        }
    }

    #[test]
    fn dom_top_level_sequential() {
        // Both at root block, a before b → dom holds.
        let a = op("a", 1, vec![root_frame()]);
        let b = op("b", 2, vec![root_frame()]);
        assert!(dom(&a, &b));
        assert_eq!(edge_quantifier(&a, &b), Quantifier::MustAllPaths);
    }

    #[test]
    fn dom_false_when_b_inside_conditional() {
        // a at root; b inside an if-then beyond a's depth → NOT dominated.
        let a = op("a", 1, vec![root_frame()]);
        let b = op(
            "b",
            2,
            vec![root_frame(), frame(1, 0, "if-then", Some(true))],
        );
        assert!(!dom(&a, &b));
        // But it IS ordered + co-executable → may edge.
        assert!(ordered_before(&a, &b));
        assert!(may_co_execute(&a, &b));
        assert_eq!(edge_quantifier(&a, &b), Quantifier::MaySomePath);
    }

    #[test]
    fn may_co_execute_false_selector_divergence() {
        // a in if-then branch, b in if-else branch of the SAME selector → mutually exclusive.
        let a = op(
            "a",
            2,
            vec![root_frame(), frame(1, 0, "if-then", Some(true))],
        );
        let b = op(
            "b",
            3,
            vec![root_frame(), frame(2, 0, "if-else", Some(true))],
        );
        assert!(!may_co_execute(&a, &b));
    }

    #[test]
    fn may_co_execute_false_non_fallthrough() {
        // a inside a non-fallthrough if-then (exits), b at ambient → cannot co-execute.
        let a = op(
            "a",
            2,
            vec![root_frame(), frame(1, 0, "if-then", Some(false))],
        );
        let b = op("b", 3, vec![root_frame()]);
        assert!(!may_co_execute(&a, &b));
    }

    #[test]
    fn ordered_before_false_loop_carried() {
        // a and b share a loop frame → no intra-iteration edge.
        let loop_frame = frame(1, 0, "loop", None);
        let a = op("a", 2, vec![root_frame(), loop_frame.clone()]);
        let b = op("b", 3, vec![root_frame(), loop_frame]);
        assert!(!ordered_before(&a, &b));
    }

    #[test]
    fn dom_false_loop_beyond_depth() {
        let a = op("a", 1, vec![root_frame()]);
        let b = op("b", 2, vec![root_frame(), frame(1, 0, "loop", None)]);
        assert!(!dom(&a, &b));
    }

    #[test]
    fn build_hb_edges_orders_pairs() {
        let a = op("a", 1, vec![root_frame()]);
        let b = op("b", 2, vec![root_frame()]);
        let edges = build_hb_edges(&[a, b]);
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].from, "a");
        assert_eq!(edges[0].to, "b");
        assert_eq!(edges[0].quantifier, Quantifier::MustAllPaths);
    }
}

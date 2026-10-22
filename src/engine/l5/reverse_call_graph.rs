//! Reverse call graph — faithful port of al-sem
//! `src/engine/reverse-call-graph.ts`.
//!
//! `build_reverse_call_graph(graph)` inverts `graph.edges_by_from` so each
//! routine knows who calls it: a map `calleeRoutineId → CombinedEdge[]` (the
//! edges where that routine is the `to`). `callers_of(reverse, id)` returns the
//! resolved callers of a routine (empty when none).
//!
//! The reverse map is keyed by internal RoutineId. We use a `BTreeMap` so any
//! iteration over the map (e.g. a debug dump) is deterministic. The per-key edge
//! list order does NOT necessarily equal al-sem's (al-sem iterates a JS-`Map`
//! `edges_by_from.values()` in insertion order; we iterate the sorted `nodes`
//! list — see `build_reverse_call_graph`). This difference is provably dead: no
//! L5 consumer treats the per-key list as ordered output — every routine id
//! derived from it is re-sorted, and span membership is a `BTreeSet`.

use std::collections::BTreeMap;

use crate::engine::l4::combined_graph::{CombinedEdge, PrimaryGraph};

/// Map of routineId → edges where that routine is the callee. Mirrors al-sem
/// `ReverseCallGraph = Map<RoutineId, CombinedEdge[]>`.
pub type ReverseCallGraph = BTreeMap<String, Vec<CombinedEdge>>;

/// Invert `graph.edges_by_from` so each routine knows who calls it.
///
/// al-sem iterates `edgesByFrom.values()` (a JS `Map`, insertion-ordered) then
/// each edge, pushing onto `reverse.get(e.to)`. The Rust forward graph stores
/// `edges_by_from` as a `HashMap`, so its `values()` order is nondeterministic —
/// but the reverse map only ever exposes per-key edge LISTS, never an ordered
/// walk of distinct callees as output, and every L5 consumer SORTS the routine
/// ids it derives (`entry_points`, `transaction_spans`). To make the per-key
/// list deterministic regardless of `HashMap` iteration order, we iterate the
/// SORTED node list and, for each, its (already edge-sort-keyed) edge list — so
/// the reverse list for any callee is a stable function of the inputs.
pub fn build_reverse_call_graph(graph: &PrimaryGraph) -> ReverseCallGraph {
    let mut reverse: ReverseCallGraph = BTreeMap::new();
    // Each id once (class K, #160 step 11b §4.6): a collided id is in `nodes`
    // once per row.
    for node in graph.distinct_nodes() {
        if let Some(edges) = graph.edges_by_from.get(node) {
            for e in edges {
                reverse.entry(e.to.clone()).or_default().push(e.clone());
            }
        }
    }
    reverse
}

/// Return the resolved callers of a routine; empty slice when none. Mirrors
/// al-sem `callersOf(reverse, routineId)`.
pub fn callers_of<'a>(reverse: &'a ReverseCallGraph, routine_id: &str) -> &'a [CombinedEdge] {
    reverse.get(routine_id).map(|v| v.as_slice()).unwrap_or(&[])
}

// ===========================================================================
// Native oracles — ground-truth-free invariants on synthetic inputs.
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::l5::test_support::{edge, graph_from_edges};
    use std::collections::HashSet;

    #[test]
    fn inverting_then_querying_recovers_forward_callers() {
        // a→b, a→c, c→b. Reverse: b has callers {a,c}; c has caller {a}; a none.
        let g = graph_from_edges(
            &["a", "b", "c"],
            &[
                edge("a", "b", "cs1"),
                edge("a", "c", "cs2"),
                edge("c", "b", "cs3"),
            ],
        );
        let rev = build_reverse_call_graph(&g);

        let callers_b: HashSet<&str> = callers_of(&rev, "b")
            .iter()
            .map(|e| e.from.as_str())
            .collect();
        assert_eq!(callers_b, ["a", "c"].into_iter().collect::<HashSet<_>>());

        let callers_c: HashSet<&str> = callers_of(&rev, "c")
            .iter()
            .map(|e| e.from.as_str())
            .collect();
        assert_eq!(callers_c, ["a"].into_iter().collect::<HashSet<_>>());

        // The exact forward edges are recovered (by callsite id), not just the from-set.
        let mut cs_into_b: Vec<&str> = callers_of(&rev, "b")
            .iter()
            .filter_map(|e| e.callsite_id.as_deref())
            .collect();
        cs_into_b.sort();
        assert_eq!(cs_into_b, vec!["cs1", "cs3"]);
    }

    #[test]
    fn unknown_id_yields_empty() {
        let g = graph_from_edges(&["a", "b"], &[edge("a", "b", "cs1")]);
        let rev = build_reverse_call_graph(&g);
        assert!(callers_of(&rev, "a").is_empty()); // a is never a callee
        assert!(callers_of(&rev, "nonexistent").is_empty());
    }

    #[test]
    fn empty_graph_yields_empty_reverse() {
        let g = graph_from_edges(&[], &[]);
        let rev = build_reverse_call_graph(&g);
        assert!(rev.is_empty());
        assert!(callers_of(&rev, "x").is_empty());
    }
}

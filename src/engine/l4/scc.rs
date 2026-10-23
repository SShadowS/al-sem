//! L4 Tarjan SCC (R3a-1 Task 2) — faithful port of al-sem's `tarjanScc`
//! (`src/engine/scc.ts`).
//!
//! ITERATIVE Tarjan (explicit work stack, NO recursion — AL call graphs can be
//! deep). Tarjan emits SCCs in REVERSE-TOPOLOGICAL order naturally (callees before
//! callers), which is exactly the bottom-up order the summary engine (R3a-2) wants.
//! Node iteration follows `nodes` (sorted) and edge iteration follows the pre-sorted
//! per-`from` edge lists, so the result is deterministic.
//!
//! `recursive` flag = SCC size > 1 OR the single member has a self-edge.
//!
//! Operates on INTERNAL routine ids (matching al-sem, which runs over the combined
//! graph's internal `RoutineId`s). The R3a-1 projection maps members to StableRoutineId
//! and re-sorts by the stable id; the internal-id member sort here is the same
//! determinism al-sem's `scc.ts:96` `[...members].sort()` provides.

use std::collections::{HashMap, HashSet};

/// The minimal graph shape `tarjan_scc` needs: sorted nodes + an adjacency map of
/// outgoing `to` targets per node. (Mirrors al-sem's `SccInputGraph`:
/// `{ nodes, edgesByFrom: Map<from, {to}[]> }`.) Edge lists are consumed in the
/// order given (the caller pre-sorts them), so iteration is deterministic.
pub struct SccInputGraph<'a> {
    /// Nodes in deterministic (sorted) order — drives the outer DFS roots.
    pub nodes: &'a [String],
    /// from-id → ordered list of `to` ids (the pre-sorted combined-edge targets).
    pub edges_by_from: &'a HashMap<String, Vec<String>>,
}

/// One strongly-connected component. `members` in internal-id sorted order;
/// `recursive` = size > 1 OR a self-edge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scc {
    pub members: Vec<String>,
    pub recursive: bool,
}

/// The SCC result: the reverse-topological SCC list + a member→index map.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SccResult {
    /// SCCs in reverse-topological order: callees before callers. ORDER IS PART OF
    /// THE SURFACE.
    pub sccs: Vec<Scc>,
    /// internal routine id → index into `sccs`.
    pub scc_id_by_routine: HashMap<String, usize>,
}

/// One frame of the explicit work stack: a node + its next-child cursor.
struct Frame {
    node: String,
    child_idx: usize,
}

/// Tarjan's SCC over the combined graph. Iterative (no recursion). Reverse-topo
/// output order; deterministic member sort; `recursive` flag = size > 1 OR self-loop.
///
/// Never panics on malformed input: missing adjacency entries degrade to "no
/// children" (an empty slice), and the explicit stack bounds memory by node count.
pub fn tarjan_scc(graph: &SccInputGraph) -> SccResult {
    let mut next_index: usize = 0;
    let mut index: HashMap<String, usize> = HashMap::new();
    let mut lowlink: HashMap<String, usize> = HashMap::new();
    let mut on_stack: HashSet<String> = HashSet::new();
    let mut stack: Vec<String> = Vec::new();
    let mut raw_sccs: Vec<Vec<String>> = Vec::new();

    let empty: Vec<String> = Vec::new();

    for start in graph.nodes {
        if index.contains_key(start) {
            continue;
        }
        let mut work: Vec<Frame> = vec![Frame {
            node: start.clone(),
            child_idx: 0,
        }];

        while !work.is_empty() {
            let top = work.len() - 1;
            let node = work[top].node.clone();
            let child_idx = work[top].child_idx;

            if child_idx == 0 {
                index.insert(node.clone(), next_index);
                lowlink.insert(node.clone(), next_index);
                next_index += 1;
                stack.push(node.clone());
                on_stack.insert(node.clone());
            }

            let children = graph.edges_by_from.get(&node).unwrap_or(&empty);
            if child_idx < children.len() {
                let to = children[child_idx].clone();
                work[top].child_idx += 1;
                if !index.contains_key(&to) {
                    work.push(Frame {
                        node: to,
                        child_idx: 0,
                    });
                } else if on_stack.contains(&to) {
                    let cur = *lowlink.get(&node).unwrap_or(&0);
                    let to_idx = *index.get(&to).unwrap_or(&0);
                    lowlink.insert(node.clone(), cur.min(to_idx));
                }
                continue;
            }

            // All children processed — settle this node.
            if lowlink.get(&node) == index.get(&node) {
                let mut members: Vec<String> = Vec::new();
                while let Some(w) = stack.pop() {
                    on_stack.remove(&w);
                    let is_root = w == node;
                    members.push(w);
                    if is_root {
                        break;
                    }
                }
                raw_sccs.push(members);
            }
            work.pop();
            if let Some(parent) = work.last() {
                let parent_node = parent.node.clone();
                let p_cur = *lowlink.get(&parent_node).unwrap_or(&0);
                let n_cur = *lowlink.get(&node).unwrap_or(&0);
                lowlink.insert(parent_node, p_cur.min(n_cur));
            }
        }
    }

    // raw_sccs is already in reverse-topological order (Tarjan property).
    let mut sccs: Vec<Scc> = Vec::new();
    let mut scc_id_by_routine: HashMap<String, usize> = HashMap::new();
    for members in raw_sccs {
        let mut sorted = members;
        sorted.sort();
        let mut recursive = sorted.len() > 1;
        if !recursive && let Some(only) = sorted.first() {
            recursive = graph
                .edges_by_from
                .get(only)
                .map(|tos| tos.iter().any(|t| t == only))
                .unwrap_or(false);
        }
        let scc_id = sccs.len();
        for m in &sorted {
            scc_id_by_routine.insert(m.clone(), scc_id);
        }
        sccs.push(Scc {
            members: sorted,
            recursive,
        });
    }

    SccResult {
        sccs,
        scc_id_by_routine,
    }
}

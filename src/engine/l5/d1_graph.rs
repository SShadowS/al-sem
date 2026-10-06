//! `d1_graph` — Task 1 of the d1-reachability redesign
//! (`.superpowers/sdd/task-1-brief.md`): extracts the COMPACT filtered call
//! graph + per-node terminal-op list + in-loop-call seed set that the
//! reachability search runs over. `build_d1_graph` is on the LIVE production
//! path — `detect_d1` calls it directly, then feeds its output to
//! `d1_reach::search_loops_cohorts` (the cohort-redesign entry point).
//!
//! Every filter here is a DIRECT port of an existing `d1.rs` rule — see the
//! per-function doc comment for its exact citation. `build_d1_graph` builds:
//!   - a [`D1Graph`]: dense node ids (interned to a [`NodeIx`]) + a filtered
//!     edge list per node (mirrors `D1Policy::expand`, d1.rs:657-675) + a
//!     filtered terminal-op list per node (mirrors `D1Policy::terminals_at`,
//!     d1.rs:632-655), restricted to the BFS closure reachable from every
//!     seed's entry node;
//!   - a `Vec<D1Seed>`: one seed per in-loop callsite surviving `detect_d1`
//!     branch (b)'s ladder (d1.rs:1094-1139), each carrying the interned
//!     [`NodeIx`] of its resolved callee entry.
//!
//! Node universe = BFS closure from the distinct seed-entry nodes over the
//! FILTERED edge set, insertion order = discovery order (deterministic: the
//! same inputs always produce the same `node_ids` order — see
//! `closure_is_reachable_only_and_deterministic` below).
//!
//! NOTE: wired into `detect_d1` (see above). The module-level
//! `allow(dead_code)` predates that wiring; it is retained unchanged here —
//! not audited in this pass (scoped to `d1_reach`/`d1_dataflow`/`finding`;
//! see their own doc headers for the cohort-redesign dead-code cleanup).
#![allow(dead_code)]

use std::collections::{HashMap, HashSet};

use crate::engine::l2::features::{PCallSite, PLoop};
use crate::engine::l3::l3_workspace::{L3RecordOperation, L3Routine, L3Table, L3Workspace};
use crate::engine::l4::cone_derived::ConeDerivedStore;
use crate::engine::l5::capability_query::{EffectPresence, touches_db_derived};
use crate::engine::l5::detector_context::DetectorContext;
use crate::engine::l5::detectors::d1::edge_target_matches_callsite_callee;
use crate::engine::l5::detectors::{is_terminator_next, op_targets_virtual_system_table};
use crate::engine::l5::full_summary::FullRoutineSummary;
use crate::engine::l5::op_classification::{classify_op, is_db_touching_class};

/// A dense index into [`D1Graph::node_ids`]/`edges`/`terminals` — one per
/// distinct routine id reachable in the closure.
pub(crate) type NodeIx = u32;

/// One filtered outgoing call edge in the compact graph. Mirrors the fields
/// `D1Policy::expand` / `build_hop_step` / `loop_depth_of_edge` read off a
/// `CombinedEdge` in the old path-walker traversal (d1.rs:657-767).
pub(crate) struct D1Edge<'a> {
    pub to: NodeIx,
    pub kind: &'a str,
    pub callsite_id: Option<&'a str>,
    /// `loop_depth_of_edge` semantics (`D1Policy::loop_depth_of_edge`,
    /// d1.rs:760-767): `call_site_by_id[cs].loop_stack.len()`, else 0.
    pub loop_depth: i64,
    /// kind ∈ {direct, method, implicit-trigger} — the temp-binding allowlist
    /// `resolve_temp_along_path_closed_world` gates a PD-chase hop on.
    pub binding_ok: bool,
}

/// One filtered db-touching terminal op in the compact graph. Mirrors
/// `D1Policy::terminals_at`'s per-op `Terminal` (d1.rs:632-655).
pub(crate) struct D1Terminal<'a> {
    pub op: &'a L3RecordOperation,
    pub owner: &'a L3Routine,
    /// `op.loop_stack.len()` — the op's own local loop depth.
    pub local_depth: i64,
}

/// The compact filtered graph: dense node ids + per-node filtered edges/terminals.
pub(crate) struct D1Graph<'a> {
    /// `NodeIx` -> routine internal id.
    pub node_ids: Vec<&'a str>,
    pub node_ix: HashMap<&'a str, NodeIx>,
    /// Filtered edges per node, in `edges_by_from`'s ORIGINAL order.
    pub edges: Vec<Vec<D1Edge<'a>>>,
    /// Filtered terminal ops per node, in `record_operations` order.
    pub terminals: Vec<Vec<D1Terminal<'a>>>,
}

/// One in-loop call-site seed surviving `detect_d1` branch (b)'s ladder
/// (d1.rs:1094-1139).
pub(crate) struct D1Seed<'a> {
    pub loop_routine: &'a L3Routine,
    /// Representative (innermost) loop id — `cs.loop_stack.last()`.
    pub loop_id: &'a str,
    pub loop_info: &'a PLoop,
    pub callsite: &'a PCallSite,
    /// The interned `NodeIx` of the seed's resolved callee entry.
    pub entry: NodeIx,
    pub entry_edge_kind: &'a str,
    /// `cs.loop_stack.len()`.
    pub seed_depth: i64,
}

/// The temp-binding edge-kind allowlist `resolve_temp_along_path_closed_world`
/// gates a PD-chase hop on (d1.rs's `edge_kind_by_callsite` consult, Component
/// 3 / RV-6): only `direct | method | implicit-trigger` hops carry usable
/// binding semantics.
pub(crate) fn edge_kind_binding_ok(kind: &str) -> bool {
    matches!(kind, "direct" | "method" | "implicit-trigger")
}

/// `touches_db_of`, memoized once per routine id in the CALLER-owned memo (so
/// Tasks 3/5 can share one memo across a `build_d1_graph` call and their own
/// subsequent probes). Mirrors `D1Policy::touches_db_memoized` (d1.rs:617-629).
///
/// ⟨C1 Task 2⟩ The presence half now reads the folded cone flag
/// ([`touches_db_derived`]) instead of scanning the routine's raw reachable
/// facts; the absence half is unchanged (`coverage.inherited_status`, read off
/// the same `summary`). ⟨fix N-C⟩ The memo is retained, but not because it
/// pays for itself: a memo HIT is one `HashMap<&str, _>` lookup, while the
/// direct alternative (`touches_db_derived` → [`ConeDerivedStore::row`],
/// `cone_derived.rs:285-287`) is one `HashMap<String, _>` lookup of the same
/// key bytes plus a `u8` mask test and a field read — the same asymptotics,
/// and a memo MISS additionally pays for the entry insert. At best a wash,
/// strictly negative on first touch. ⟨fix M1⟩ `detect_d1` does NOT read this
/// memo again after `build_d1_graph` returns — the prior wording claiming it
/// is "shared with `d1`'s own later probes" was false.
///
/// The memo's value is a function of `(store, summary)`, not of `summary`
/// alone, but nothing here binds it to the store it was filled from — never
/// reuse one `touches_db_memo` map across two `DetectorContext`s /
/// `ConeDerivedStore`s, or a stale entry would silently answer for the wrong
/// workspace's cone flags.
fn memoized_touches_db<'a>(
    store: &ConeDerivedStore,
    memo: &mut HashMap<&'a str, EffectPresence>,
    summary: &'a FullRoutineSummary,
) -> EffectPresence {
    *memo
        .entry(summary.routine_id.as_str())
        .or_insert_with(|| touches_db_derived(store, summary))
}

/// The filtered terminal-op list for `routine`'s own body. Mirrors
/// `D1Policy::terminals_at` (d1.rs:632-655): db-touching class only, minus the
/// G-1 terminator-`Next` exclusion, minus the G-6 virtual-system-table
/// exclusion. (The dependency-role `summary.dbEffects` fallback in al-sem's
/// `terminalsAt` is DEAD in the source-only pipeline — see d1.rs's module doc
/// — so it is not reproduced here either.)
fn terminals_of<'a>(
    routine: &'a L3Routine,
    table_by_id: &HashMap<&str, &L3Table>,
) -> Vec<D1Terminal<'a>> {
    routine
        .record_operations
        .iter()
        .filter(|op| is_db_touching_class(classify_op(&op.op)))
        .filter(|op| !is_terminator_next(op))
        .filter(|op| !op_targets_virtual_system_table(op, routine, table_by_id))
        .map(|op| D1Terminal {
            op,
            owner: routine,
            local_depth: op.loop_stack.len() as i64,
        })
        .collect()
}

/// Build the compact filtered d1 graph + the branch-(b) seed list.
///
/// Two passes:
///   1. Iterate every routine's in-loop call sites, applying the EXACT seed
///      ladder `detect_d1` branch (b) applies (d1.rs:1094-1139): the routine
///      gate (`body_available` && `!parse_incomplete`, d1.rs:984-990), a
///      resolvable G-18 edge (callsite-id match + target-name match via
///      `edge_target_matches_callsite_callee`), not `interface`/`dynamic`,
///      callee summary present and `touches_db != No`. Each survivor becomes a
///      [`D1Seed`] (its `entry` `NodeIx` is patched once every closure node is
///      interned below) and its resolved callee routine id seeds the BFS
///      frontier.
///   2. BFS-close the frontier over the FILTERED edge set (mirrors
///      `D1Policy::expand`, d1.rs:657-675: drop `event-dispatch`, drop targets
///      with no summary or `touches_db == No`), interning each newly
///      discovered node id to a dense `NodeIx` in discovery order, and filling
///      that node's edges/terminals as it is visited.
///
/// The BFS is implemented as a single growing `Vec` (`node_ids`) walked by an
/// index cursor: new targets are appended to the end as they are discovered,
/// so `node_ids[i]`'s edges/terminals are always computed at loop index `i` —
/// no separate queue is needed, and the node/edge/terminal vectors stay
/// trivially index-aligned.
pub(crate) fn build_d1_graph<'a>(
    ctx: &'a DetectorContext,
    ws: &'a L3Workspace,
    touches_db_memo: &mut HashMap<&'a str, EffectPresence>,
) -> (D1Graph<'a>, Vec<D1Seed<'a>>) {
    let mut seeds: Vec<D1Seed<'a>> = Vec::new();
    // The seed's resolved-callee entry id, index-aligned with `seeds` —
    // patched into `seeds[i].entry` once the closure's NodeIx assignment is
    // final (a seed is pushed before the closure is even built).
    let mut seed_entry_ids: Vec<&'a str> = Vec::new();
    // Distinct seed-entry routine ids, in first-discovery order — the BFS's
    // initial frontier.
    let mut frontier: Vec<&'a str> = Vec::new();
    let mut frontier_seen: HashSet<&'a str> = HashSet::new();

    for routine in &ws.routines {
        // detect_d1's routine gate (d1.rs:984-990): body available, not parse-incomplete.
        if !routine.body_available || routine.parse_incomplete {
            continue;
        }
        let loop_by_id: HashMap<&str, &'a PLoop> =
            routine.loops.iter().map(|l| (l.id.as_str(), l)).collect();
        for cs in &routine.call_sites {
            if cs.loop_stack.is_empty() {
                continue;
            }
            let Some(rep) = cs.loop_stack.last().map(|s| s.as_str()) else {
                continue;
            };
            let Some(loop_info) = loop_by_id.get(rep).copied() else {
                continue;
            };
            // G-18 (docs/engine-gaps.md): callsite-id match + resolved-target
            // callee-name match — see `edge_target_matches_callsite_callee`.
            // Taking the FIRST match is deliberate (engine-switch S3.6 audit): a
            // call site has several edges only for an interface dispatch, and
            // `interface` edges are skipped below whichever comes first.
            let edge = ctx.graph.edges_by_from.get(&routine.id).and_then(|edges| {
                edges.iter().find(|e| {
                    e.callsite_id.as_deref() == Some(cs.id.as_str())
                        && edge_target_matches_callsite_callee(e, cs, &ctx.routine_by_id)
                })
            });
            let Some(edge) = edge else {
                continue;
            };
            if edge.kind == "interface" || edge.kind == "dynamic" {
                continue;
            }
            let Some(sum) = ctx.summaries.get(&edge.to) else {
                continue;
            };
            if memoized_touches_db(&ctx.cone_derived, touches_db_memo, sum) == EffectPresence::No {
                continue;
            }
            let entry_id: &'a str = edge.to.as_str();
            if frontier_seen.insert(entry_id) {
                frontier.push(entry_id);
            }
            seeds.push(D1Seed {
                loop_routine: routine,
                loop_id: rep,
                loop_info,
                callsite: cs,
                entry: NodeIx::MAX, // patched below, once every node is interned
                entry_edge_kind: edge.kind.as_str(),
                seed_depth: cs.loop_stack.len() as i64,
            });
            seed_entry_ids.push(entry_id);
        }
    }

    // BFS closure via a growing frontier vector: `node_ids` doubles as the
    // traversal queue — index `i` is processed exactly once, appending newly
    // discovered targets to the end, so discovery order == node_ids order.
    let mut node_ids: Vec<&'a str> = Vec::new();
    let mut node_ix: HashMap<&'a str, NodeIx> = HashMap::new();
    for id in frontier {
        let ix = node_ids.len() as NodeIx;
        node_ix.insert(id, ix);
        node_ids.push(id);
    }

    let mut edges: Vec<Vec<D1Edge<'a>>> = Vec::new();
    let mut terminals: Vec<Vec<D1Terminal<'a>>> = Vec::new();

    let mut i = 0usize;
    while i < node_ids.len() {
        let id = node_ids[i];

        let mut node_edges: Vec<D1Edge<'a>> = Vec::new();
        if let Some(raw_edges) = ctx.graph.edges_by_from.get(id) {
            for e in raw_edges {
                // Mirrors `D1Policy::expand` exactly (d1.rs:657-675): event
                // fan-out is D2's job; drop targets with no summary or a
                // proven-`No` touches_db verdict.
                if e.kind == "event-dispatch" {
                    continue;
                }
                let Some(sum) = ctx.summaries.get(&e.to) else {
                    continue;
                };
                if memoized_touches_db(&ctx.cone_derived, touches_db_memo, sum)
                    == EffectPresence::No
                {
                    continue;
                }
                let to_id: &'a str = e.to.as_str();
                let to_ix = *node_ix.entry(to_id).or_insert_with(|| {
                    let ix = node_ids.len() as NodeIx;
                    node_ids.push(to_id);
                    ix
                });
                let loop_depth = e
                    .callsite_id
                    .as_deref()
                    .and_then(|cid| ctx.call_site_by_id.get(cid))
                    .map(|cs| cs.loop_stack.len() as i64)
                    .unwrap_or(0);
                node_edges.push(D1Edge {
                    to: to_ix,
                    kind: e.kind.as_str(),
                    callsite_id: e.callsite_id.as_deref(),
                    loop_depth,
                    binding_ok: edge_kind_binding_ok(&e.kind),
                });
            }
        }
        edges.push(node_edges);

        let node_terminals = match ctx.routine_by_id.get(id).copied() {
            Some(routine) => terminals_of(routine, &ctx.table_by_id),
            None => Vec::new(),
        };
        terminals.push(node_terminals);

        i += 1;
    }

    // Patch each seed's entry NodeIx now that every closure node is interned.
    // Every seed's entry_id was pushed into `frontier` above, so it is always
    // present in `node_ix` at this point.
    for (seed, entry_id) in seeds.iter_mut().zip(seed_entry_ids.iter()) {
        seed.entry = *node_ix
            .get(entry_id)
            .expect("every seed entry id was seeded into the BFS frontier");
    }

    (
        D1Graph {
            node_ids,
            node_ix,
            edges,
            terminals,
        },
        seeds,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::l4::combined_graph::CombinedEdge;
    use crate::engine::l5::test_support::{
        call_site, coverage, edge_kind, fact, loop_def, minimal_ctx, record_op, routine, summary,
    };

    // -----------------------------------------------------------------------
    // Test 1: edge filter drops event-dispatch and non-db targets.
    // -----------------------------------------------------------------------
    #[test]
    fn edge_filter_drops_event_dispatch_and_non_db_targets() {
        // L: a loop calling A in-loop (the seed).
        let mut l = routine("L", "procedure");
        l.loops = vec![loop_def("L/loop0")];
        l.call_sites = vec![call_site("L/cs0", "A", vec!["L/loop0".to_string()])];

        let a = routine("A", "procedure");
        let b = routine("B", "procedure");
        let c = routine("C", "procedure");
        let d = routine("D", "procedure");
        let routines = vec![l, a, b, c, d];

        let mut graph_edges: HashMap<String, Vec<CombinedEdge>> = HashMap::new();
        graph_edges.insert(
            "L".to_string(),
            vec![edge_kind("L", "A", "L/cs0", "direct")],
        );
        graph_edges.insert(
            "A".to_string(),
            vec![
                edge_kind("A", "B", "A/cs0", "direct"),
                edge_kind("A", "C", "A/cs1", "event-dispatch"),
                edge_kind("A", "D", "A/cs2", "direct"),
            ],
        );

        let summaries: HashMap<String, FullRoutineSummary> = [
            (
                "A".to_string(),
                summary(
                    "A",
                    vec![fact("read", "table", Some("t/A"))],
                    vec![],
                    Some(coverage("complete")),
                ),
            ),
            (
                "B".to_string(),
                summary(
                    "B",
                    vec![fact("read", "table", Some("t/B"))],
                    vec![],
                    Some(coverage("complete")),
                ),
            ),
            (
                "C".to_string(),
                summary(
                    "C",
                    vec![fact("read", "table", Some("t/C"))],
                    vec![],
                    Some(coverage("complete")),
                ),
            ),
            (
                "D".to_string(),
                summary("D", vec![], vec![], Some(coverage("complete"))),
            ),
        ]
        .into_iter()
        .collect();

        let ctx = minimal_ctx(&routines, graph_edges, summaries);
        let ws = L3Workspace {
            objects: vec![],
            tables: vec![],
            routines: routines.clone(),
        };
        let mut memo = HashMap::new();
        let (graph, seeds) = build_d1_graph(&ctx, &ws, &mut memo);

        assert_eq!(
            graph.node_ids,
            vec!["A", "B"],
            "closure must be {{A, B}} only"
        );
        let a_ix = graph.node_ix["A"];
        let b_ix = graph.node_ix["B"];
        let a_edges = &graph.edges[a_ix as usize];
        assert_eq!(
            a_edges.len(),
            1,
            "A's filtered edge list must have exactly 1 edge"
        );
        assert_eq!(a_edges[0].to, b_ix);
        assert_eq!(a_edges[0].kind, "direct");

        assert_eq!(seeds.len(), 1);
        assert_eq!(seeds[0].entry, a_ix);
    }

    // -----------------------------------------------------------------------
    // Test 2: terminals respect the G-1 (terminator-Next) and G-6
    // (virtual-system-table) exclusions.
    // -----------------------------------------------------------------------
    #[test]
    fn terminals_respect_g1_g6_filters() {
        let mut l = routine("L", "procedure");
        l.loops = vec![loop_def("L/loop0")];
        l.call_sites = vec![call_site("L/cs0", "B", vec!["L/loop0".to_string()])];

        let mut b = routine("B", "procedure");
        // A record variable "F" declared against the virtual/system table
        // "Field" — op_targets_virtual_system_table's G-6 exclusion signal.
        b.record_variables = vec![crate::engine::l3::l3_workspace::L3RecordVariable {
            id: "B/rv0".to_string(),
            name: "F".to_string(),
            table_name: Some("Field".to_string()),
            table_id: None,
            is_parameter: false,
            parameter_index: None,
            temp_state: crate::engine::l2::features::PTempState {
                kind: "unknown".to_string(),
                value: None,
                parameter_index: None,
            },
            scope: None,
        }];
        let op_get = record_op("B/op0", "Get", "Rec", None, vec![], false);
        let op_next_terminator = record_op("B/op1", "Next", "Rec", None, vec![], true);
        let op_get_virtual = record_op("B/op2", "Get", "F", None, vec![], false);
        b.record_operations = vec![op_get.clone(), op_next_terminator, op_get_virtual];

        let routines = vec![l, b];

        let mut graph_edges: HashMap<String, Vec<CombinedEdge>> = HashMap::new();
        graph_edges.insert(
            "L".to_string(),
            vec![edge_kind("L", "B", "L/cs0", "direct")],
        );

        let summaries: HashMap<String, FullRoutineSummary> = [(
            "B".to_string(),
            summary(
                "B",
                vec![fact("read", "table", Some("t/B"))],
                vec![],
                Some(coverage("complete")),
            ),
        )]
        .into_iter()
        .collect();

        let ctx = minimal_ctx(&routines, graph_edges, summaries);
        let ws = L3Workspace {
            objects: vec![],
            tables: vec![],
            routines: routines.clone(),
        };
        let mut memo = HashMap::new();
        let (graph, _seeds) = build_d1_graph(&ctx, &ws, &mut memo);

        let b_ix = graph.node_ix["B"];
        let terminals = &graph.terminals[b_ix as usize];
        assert_eq!(
            terminals.len(),
            1,
            "only the plain Get should survive the G-1/G-6 filters"
        );
        assert_eq!(terminals[0].op.id, op_get.id);
        assert_eq!(terminals[0].op.op, "Get");
    }

    // -----------------------------------------------------------------------
    // Test 3: seed ladder matches branch (b)'s exact skip rules.
    // -----------------------------------------------------------------------
    #[test]
    fn seed_ladder_matches_branch_b_skips() {
        let mut l = routine("L", "procedure");
        l.loops = vec![loop_def("L/loop0")];
        l.call_sites = vec![
            call_site("L/cs0", "A", vec!["L/loop0".to_string()]), // resolvable direct -> kept
            call_site("L/cs1", "I", vec!["L/loop0".to_string()]), // interface -> skipped
            call_site("L/cs2", "D", vec!["L/loop0".to_string()]), // touches_db == No -> skipped
        ];

        let a = routine("A", "procedure");
        let i = routine("I", "procedure");
        let d = routine("D", "procedure");
        let routines = vec![l, a, i, d];

        let mut graph_edges: HashMap<String, Vec<CombinedEdge>> = HashMap::new();
        graph_edges.insert(
            "L".to_string(),
            vec![
                edge_kind("L", "A", "L/cs0", "direct"),
                edge_kind("L", "I", "L/cs1", "interface"),
                edge_kind("L", "D", "L/cs2", "direct"),
            ],
        );

        let summaries: HashMap<String, FullRoutineSummary> = [
            (
                "A".to_string(),
                summary(
                    "A",
                    vec![fact("read", "table", Some("t/A"))],
                    vec![],
                    Some(coverage("complete")),
                ),
            ),
            (
                "D".to_string(),
                summary("D", vec![], vec![], Some(coverage("complete"))),
            ),
        ]
        .into_iter()
        .collect();

        let ctx = minimal_ctx(&routines, graph_edges, summaries);
        let ws = L3Workspace {
            objects: vec![],
            tables: vec![],
            routines: routines.clone(),
        };
        let mut memo = HashMap::new();
        let (graph, seeds) = build_d1_graph(&ctx, &ws, &mut memo);

        assert_eq!(seeds.len(), 1, "only the L/cs0 -> A callsite should seed");
        let a_ix = graph.node_ix["A"];
        assert_eq!(seeds[0].entry, a_ix);
        assert_eq!(seeds[0].entry_edge_kind, "direct");
        assert_eq!(seeds[0].seed_depth, 1);
        assert_eq!(seeds[0].loop_id, "L/loop0");
    }

    // -----------------------------------------------------------------------
    // Test 4: the closure is reachable-only and deterministic across builds.
    // -----------------------------------------------------------------------
    #[test]
    fn closure_is_reachable_only_and_deterministic() {
        let mut l = routine("L", "procedure");
        l.loops = vec![loop_def("L/loop0")];
        l.call_sites = vec![call_site("L/cs0", "A", vec!["L/loop0".to_string()])];

        let a = routine("A", "procedure");
        let b = routine("B", "procedure");
        let t = routine("T", "procedure");
        let x = routine("X", "procedure");
        let y = routine("Y", "procedure");
        let routines = vec![l, a, b, t, x, y];

        let mut graph_edges: HashMap<String, Vec<CombinedEdge>> = HashMap::new();
        graph_edges.insert(
            "L".to_string(),
            vec![edge_kind("L", "A", "L/cs0", "direct")],
        );
        graph_edges.insert(
            "A".to_string(),
            vec![edge_kind("A", "B", "A/cs0", "direct")],
        );
        graph_edges.insert(
            "B".to_string(),
            vec![edge_kind("B", "T", "B/cs0", "direct")],
        );
        // Unrelated chain — never seeded, must never appear in the closure.
        graph_edges.insert(
            "X".to_string(),
            vec![edge_kind("X", "Y", "X/cs0", "direct")],
        );

        let summaries: HashMap<String, FullRoutineSummary> = [
            ("A", "t/A"),
            ("B", "t/B"),
            ("T", "t/T"),
            ("X", "t/X"),
            ("Y", "t/Y"),
        ]
        .into_iter()
        .map(|(id, table)| {
            (
                id.to_string(),
                summary(
                    id,
                    vec![fact("read", "table", Some(table))],
                    vec![],
                    Some(coverage("complete")),
                ),
            )
        })
        .collect();

        let ws = L3Workspace {
            objects: vec![],
            tables: vec![],
            routines: routines.clone(),
        };

        let ctx1 = minimal_ctx(&routines, graph_edges.clone(), summaries.clone());
        let mut memo1 = HashMap::new();
        let (graph1, _seeds1) = build_d1_graph(&ctx1, &ws, &mut memo1);

        let ctx2 = minimal_ctx(&routines, graph_edges, summaries);
        let mut memo2 = HashMap::new();
        let (graph2, _seeds2) = build_d1_graph(&ctx2, &ws, &mut memo2);

        assert_eq!(graph1.node_ids, vec!["A", "B", "T"]);
        assert_eq!(
            graph1.node_ids, graph2.node_ids,
            "two builds over the same input must produce identical discovery order"
        );
        assert!(!graph1.node_ids.contains(&"X"));
        assert!(!graph1.node_ids.contains(&"Y"));
    }
}

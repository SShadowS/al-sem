//! R3a-1 EXIT-GATE — native L4-direct structural oracle for the combined graph +
//! Tarjan SCC.
//!
//! Ground-truth-free, STRUCTURAL oracles run NATIVELY against the Rust R3a-1
//! projection (`project_r3a1_combined_graph`) + the resolved model's event-graph /
//! record-type projections — NOT a transitive byte-match against the al-sem
//! goldens. The byte-parity differential (`r3a1_differential.rs`) is necessary but
//! not sufficient: if BOTH engines made the same structural mistake (a dangling
//! edge target, an out-of-order SCC, a mis-flagged `recursive`), a pure equality
//! diff would still pass. These oracles assert the combined-graph + SCC CONTRACT in
//! ABSOLUTE terms over the Rust output.
//!
//! ## The five invariants (plan Task 3 Step 2)
//!   1. every `CombinedEdge.to` is a REAL routine id in the model;
//!   2. the `event-dispatch` combined edges == the event graph's resolved
//!      publisher-routine → subscriber-routine pairs (by eventId);
//!   3. the `tarjanScc` output is a VALID REVERSE-topological order: for every
//!      combined edge `from→to`, `to`'s SCC index is AT-OR-BEFORE `from`'s SCC index
//!      (callees before callers), EXCEPT within a single recursive SCC (same index);
//!   4. `recursive` ⟺ (SCC size > 1 ∨ a self-loop edge on the lone member);
//!   5. the SCC partition COVERS every node EXACTLY once (no node missing, no node
//!      in two SCCs) — and every member is a real routine id.
//!
//! The corpus is the full SOURCE-ONLY `ws-*` set (the same goldens the differential
//! reads); the oracles run over EVERY fixture so the recursive / event-dispatch /
//! multi-member cases are all exercised (ws-event-cycle / ws-d7-event-cycle carry a
//! recursive multi-member SCC + event-dispatch edges).

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use al_sem::engine::l4::combined_graph::{PScc, R3a1Projection};
use al_sem::program::model::program_calls::assemble_and_resolve_workspace_with_program_calls;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn corpus_dir() -> PathBuf {
    repo_root().join("tests").join("r0-corpus")
}

fn goldens_dir() -> PathBuf {
    repo_root().join("tests").join("r3a1-goldens")
}

/// Every source-only fixture that has a committed R3a-1 golden (sorted).
fn discover_fixtures() -> Vec<String> {
    let dir = goldens_dir();
    let mut out = Vec::new();
    let entries = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("read R3a-1 goldens dir {}: {e}", dir.display()));
    for entry in entries {
        let name = entry
            .expect("dir entry")
            .file_name()
            .to_string_lossy()
            .to_string();
        if let Some(fx) = name.strip_suffix(".r3a1.golden.json") {
            out.push(fx.to_string());
        }
    }
    out.sort();
    out
}

/// The R3a-1 projection + the set of real routine ids (from the record-type
/// projection's `stableRoutineId`s) + the event-graph publisher/subscriber pairs.
struct OracleInput {
    proj: R3a1Projection,
    /// Every real StableRoutineId in the model.
    routine_ids: HashSet<String>,
    /// (eventId, publisherRoutineId) for every event whose publisher is a routine.
    event_publishers: HashMap<String, String>,
    /// (eventId, subscriberRoutineId) resolved subscriber pairs.
    event_subscribers: HashSet<(String, String)>,
}

/// Build the oracle input for ONE fixture from the RUST resolved model. Returns
/// None for a fail-closed/empty layout (those carry no golden, so they never reach
/// here — but stay total).
fn build(fixture: &str) -> Option<OracleInput> {
    let resolved = assemble_and_resolve_workspace_with_program_calls(&corpus_dir().join(fixture))?;
    let proj = resolved.project_r3a1_combined_graph();

    // Real routine ids: the record-type projection enumerates every routine in the
    // model keyed by StableRoutineId.
    let rt = resolved.project();
    let routine_ids: HashSet<String> = rt
        .routines
        .iter()
        .map(|r| r.stable_routine_id.clone())
        .collect();

    // Event-graph stable projection: publishers (eventId → publisherRoutineId) +
    // resolved subscriber pairs (eventId, subscriberRoutineId).
    let eg = resolved.project_event_graph();
    let mut event_publishers: HashMap<String, String> = HashMap::new();
    for ev in &eg.events {
        if let Some(pub_rid) = &ev.publisher_routine_id {
            event_publishers.insert(ev.id.clone(), pub_rid.clone());
        }
    }
    let event_subscribers: HashSet<(String, String)> = eg
        .edges
        .iter()
        .map(|e| (e.event_id.clone(), e.subscriber_routine_id.clone()))
        .collect();

    Some(OracleInput {
        proj,
        routine_ids,
        event_publishers,
        event_subscribers,
    })
}

/// SCC index map: StableRoutineId → its index into `sccs` (reverse-topo position).
fn scc_index_map(sccs: &[PScc]) -> HashMap<String, usize> {
    let mut m = HashMap::new();
    for (i, scc) in sccs.iter().enumerate() {
        for member in &scc.members {
            m.insert(member.clone(), i);
        }
    }
    m
}

// ============================================================================
// 1. Every CombinedEdge.to (and .from) is a REAL routine id in the model.
// ============================================================================

#[test]
fn every_combined_edge_target_is_a_real_routine_id() {
    let mut checked_edges = 0usize;
    for fixture in discover_fixtures() {
        let Some(input) = build(&fixture) else {
            continue;
        };
        for e in &input.proj.combined_edges {
            assert!(
                input.routine_ids.contains(&e.to),
                "[{fixture}] combined edge `to` {} is NOT a real routine id (kind={})",
                e.to,
                e.kind,
            );
            assert!(
                input.routine_ids.contains(&e.from),
                "[{fixture}] combined edge `from` {} is NOT a real routine id (kind={})",
                e.from,
                e.kind,
            );
            checked_edges += 1;
        }
    }
    assert!(
        checked_edges > 0,
        "no combined edges checked — corpus degenerate?"
    );
    eprintln!("R3a-1 oracle #1: {checked_edges} combined-edge endpoints are all real routine ids");
}

// ============================================================================
// 2. The event-dispatch combined edges == the event graph's resolved
//    publisher-routine → subscriber-routine pairs (by eventId).
// ============================================================================

#[test]
fn event_dispatch_edges_match_resolved_event_graph_pairs() {
    let mut checked = 0usize;
    for fixture in discover_fixtures() {
        let Some(input) = build(&fixture) else {
            continue;
        };
        for e in &input.proj.combined_edges {
            if e.kind != "event-dispatch" {
                continue;
            }
            let event_id = e
                .event_id
                .as_ref()
                .unwrap_or_else(|| panic!("[{fixture}] event-dispatch edge carries no eventId"));
            // The `from` must be the event's publisher routine.
            let publisher = input.event_publishers.get(event_id).unwrap_or_else(|| {
                panic!(
                    "[{fixture}] event-dispatch edge eventId {event_id} has no publisher routine in the event graph"
                )
            });
            assert_eq!(
                &e.from, publisher,
                "[{fixture}] event-dispatch edge `from` {} != event {event_id}'s publisher routine {publisher}",
                e.from,
            );
            // The (eventId, subscriber=`to`) pair must be a resolved subscriber edge.
            assert!(
                input
                    .event_subscribers
                    .contains(&(event_id.clone(), e.to.clone())),
                "[{fixture}] event-dispatch edge (event {event_id} → subscriber {}) is NOT a resolved \
                 subscriber pair in the event graph",
                e.to,
            );
            checked += 1;
        }
    }
    assert!(
        checked > 0,
        "no event-dispatch edges checked — the corpus must carry ≥1 (anti-degenerate)"
    );
    eprintln!(
        "R3a-1 oracle #2: {checked} event-dispatch edges all match resolved event-graph pairs"
    );
}

// ============================================================================
// 3. tarjanScc output is a VALID REVERSE-topological order: for every edge
//    from→to, scc[to] <= scc[from] (callee at-or-before caller), EXCEPT within a
//    single recursive SCC (scc[to] == scc[from]).
// ============================================================================

#[test]
fn scc_order_is_valid_reverse_topological() {
    let mut checked = 0usize;
    for fixture in discover_fixtures() {
        let Some(input) = build(&fixture) else {
            continue;
        };
        let idx = scc_index_map(&input.proj.sccs);
        // Every R3a-1 combined edge is routine→routine (both `from` and `to` set).
        for e in &input.proj.combined_edges {
            let (Some(&from_scc), Some(&to_scc)) = (idx.get(&e.from), idx.get(&e.to)) else {
                // Both endpoints are real routine ids (oracle #1), and every node is
                // in some SCC (oracle #5), so both are always present.
                panic!("[{fixture}] edge endpoint missing from the SCC partition: {e:?}");
            };
            // Reverse-topo: the callee's SCC must come AT-OR-BEFORE the caller's
            // (smaller-or-equal index). Equality means they're in the same SCC (a
            // cycle), which must therefore be recursive.
            assert!(
                to_scc <= from_scc,
                "[{fixture}] SCC order is NOT reverse-topological: edge {} → {} but \
                 scc[to]={to_scc} > scc[from]={from_scc} (callee after caller)",
                e.from,
                e.to,
            );
            if to_scc == from_scc {
                assert!(
                    input.proj.sccs[to_scc].recursive,
                    "[{fixture}] an intra-SCC edge {} → {} lands in a NON-recursive SCC (index {to_scc})",
                    e.from, e.to,
                );
            }
            checked += 1;
        }
    }
    assert!(checked > 0, "no edges checked for reverse-topo order");
    eprintln!("R3a-1 oracle #3: {checked} edges respect the reverse-topological SCC order");
}

// ============================================================================
// 4. recursive ⟺ (SCC size > 1 ∨ a self-loop edge on the lone member).
// ============================================================================

#[test]
fn recursive_flag_iff_multi_member_or_self_loop() {
    let mut recursive_seen = 0usize;
    let mut multi_member_seen = 0usize;
    for fixture in discover_fixtures() {
        let Some(input) = build(&fixture) else {
            continue;
        };

        // Self-loop set: a member id that has a combined edge to itself.
        let self_loops: HashSet<&str> = input
            .proj
            .combined_edges
            .iter()
            .filter(|e| e.from == e.to)
            .map(|e| e.from.as_str())
            .collect();

        for scc in &input.proj.sccs {
            let multi = scc.members.len() > 1;
            let self_loop = scc.members.len() == 1 && self_loops.contains(scc.members[0].as_str());
            let expect_recursive = multi || self_loop;
            assert_eq!(
                scc.recursive, expect_recursive,
                "[{fixture}] SCC {:?}: recursive={} but (size>1 || self-loop)={expect_recursive}",
                scc.members, scc.recursive,
            );
            if scc.recursive {
                recursive_seen += 1;
            }
            if multi {
                multi_member_seen += 1;
            }
        }
    }
    assert!(
        recursive_seen > 0,
        "no recursive SCC seen across the corpus (anti-degenerate: need ≥1)"
    );
    assert!(
        multi_member_seen > 0,
        "no multi-member SCC seen across the corpus (anti-degenerate: need ≥1)"
    );
    eprintln!(
        "R3a-1 oracle #4: recursive⟺(size>1∨self-loop) holds; {recursive_seen} recursive, \
         {multi_member_seen} multi-member SCC(s)"
    );
}

// ============================================================================
// 5. The SCC partition covers every node EXACTLY once (no node missing, no node
//    in two SCCs); every member is a real routine id.
// ============================================================================

#[test]
fn scc_partition_covers_every_node_exactly_once() {
    let mut total_members = 0usize;
    for fixture in discover_fixtures() {
        let Some(input) = build(&fixture) else {
            continue;
        };

        // No node appears in two SCCs.
        let mut seen: HashSet<&str> = HashSet::new();
        for scc in &input.proj.sccs {
            for m in &scc.members {
                assert!(
                    seen.insert(m.as_str()),
                    "[{fixture}] routine {m} appears in MORE THAN ONE SCC",
                );
                assert!(
                    input.routine_ids.contains(m),
                    "[{fixture}] SCC member {m} is NOT a real routine id",
                );
                total_members += 1;
            }
        }

        // Every real routine id is covered by exactly one SCC. (The combined-graph
        // node list is the sorted routine id set, so the partition = the routines.)
        for rid in &input.routine_ids {
            assert!(
                seen.contains(rid.as_str()),
                "[{fixture}] routine {rid} is MISSING from the SCC partition",
            );
        }
        assert_eq!(
            seen.len(),
            input.routine_ids.len(),
            "[{fixture}] SCC partition node count {} != routine count {}",
            seen.len(),
            input.routine_ids.len(),
        );
    }
    assert!(total_members > 0, "no SCC members checked");
    eprintln!(
        "R3a-1 oracle #5: SCC partition covers every node exactly once ({total_members} members)"
    );
}

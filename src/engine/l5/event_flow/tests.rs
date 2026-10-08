//! Native oracles for the event-flow substrate. Ground-truth-free: each builds a
//! synthetic `EventGraph` directly and asserts the index/query/fan-out/chain
//! output (correct + SORTED + unresolved-excluded), mirroring al-sem's probe-style
//! soundness oracles (not a byte-diff golden).

#![cfg(test)]

use std::collections::{BTreeSet, HashMap};

use super::*;
use crate::engine::l4::combined_graph::CombinedEdge;
use crate::engine::l5::test_support::{coverage, routine, summary};
use crate::program::model::events::{EventEdge, EventGraph, EventSymbol, Evidence};

fn ev(id: &str, publisher_routine: Option<&str>, name: &str, kind: &str) -> EventSymbol {
    EventSymbol {
        id: id.to_string(),
        publisher_object_id: "app/Codeunit/1".to_string(),
        publisher_routine_id: publisher_routine.map(|s| s.to_string()),
        publisher_stable_routine_id: publisher_routine.map(|s| format!("stable::{s}")),
        event_name: name.to_string(),
        event_kind: kind.to_string(),
        element_name: None,
        signature_hash: String::new(),
        parameters: Vec::new(),
        isolated: None,
        provenance: vec![Evidence {
            source: "tree-sitter".to_string(),
            note: None,
        }],
        publisher_ref: None,
    }
}

fn edge(event_id: &str, subscriber: &str, resolution: &str) -> EventEdge {
    EventEdge {
        event_id: event_id.to_string(),
        subscriber_routine_id: subscriber.to_string(),
        subscriber_stable_routine_id: format!("stable::{subscriber}"),
        subscriber_app_id: "app".to_string(),
        resolution: resolution.to_string(),
        provenance: vec![Evidence {
            source: "tree-sitter".to_string(),
            note: None,
        }],
    }
}

fn no_deps() -> BTreeSet<String> {
    BTreeSet::new()
}

// ---------------------------------------------------------------------------
// event_kind_of
// ---------------------------------------------------------------------------

#[test]
fn event_kind_of_passes_business_and_internal_else_integration() {
    assert_eq!(event_kind_of("business"), "business");
    assert_eq!(event_kind_of("internal"), "internal");
    assert_eq!(event_kind_of("integration"), "integration");
    assert_eq!(event_kind_of("trigger"), "integration");
    assert_eq!(event_kind_of("unknown"), "integration");
    assert_eq!(event_kind_of(""), "integration");
    assert_eq!(event_kind_of("anything-else"), "integration");
}

// ---------------------------------------------------------------------------
// build_event_flow_indexes
// ---------------------------------------------------------------------------

/// 1 publisher `P`, 2 events `E1`/`E2`; resolved edges (s2→E1, s1→E1) + an
/// unresolved edge (s3→E2). The unresolved edge must be EXCLUDED from
/// subscribers/publishers, and every list must be sorted.
fn synthetic_graph() -> EventGraph {
    EventGraph {
        events: vec![
            ev("E1", Some("P"), "OnFoo", "integration"),
            ev("E2", Some("P"), "OnBar", "business"),
        ],
        edges: vec![
            edge("E1", "s2", "resolved"),
            edge("E1", "s1", "resolved"),
            edge("E2", "s3", "maybe"), // unresolved → excluded
        ],
    }
}

#[test]
fn build_indexes_publisher_and_events_by_publisher() {
    let g = synthetic_graph();
    let rs = vec![routine("P", "event-publisher")];
    let ix = build_event_flow_indexes(&g, &rs, &no_deps());

    assert_eq!(ix.publisher_by_event.get("E1"), Some(&"P".to_string()));
    assert_eq!(ix.publisher_by_event.get("E2"), Some(&"P".to_string()));
    // events_by_publisher sorted: E1, E2.
    assert_eq!(
        ix.events_by_publisher.get("P"),
        Some(&vec!["E1".to_string(), "E2".to_string()])
    );
    assert_eq!(ix.event_name_by_event.get("E1"), Some(&"OnFoo".to_string()));
    assert_eq!(ix.event_name_by_event.get("E2"), Some(&"OnBar".to_string()));
}

#[test]
fn build_indexes_subscribers_sorted_and_unresolved_excluded() {
    let g = synthetic_graph();
    let rs = vec![routine("P", "event-publisher")];
    let ix = build_event_flow_indexes(&g, &rs, &no_deps());

    // E1 has resolved subs s1, s2 — SORTED (input was s2, s1).
    assert_eq!(
        ix.subscribers_by_event.get("E1"),
        Some(&vec!["s1".to_string(), "s2".to_string()])
    );
    // E2's only edge was unresolved → NOT present.
    assert_eq!(ix.subscribers_by_event.get("E2"), None);
}

#[test]
fn build_indexes_publishers_by_subscriber_unresolved_excluded() {
    let g = synthetic_graph();
    let rs = vec![routine("P", "event-publisher")];
    let ix = build_event_flow_indexes(&g, &rs, &no_deps());

    assert_eq!(
        ix.publishers_by_subscriber.get("s1"),
        Some(&vec!["P".to_string()])
    );
    assert_eq!(
        ix.publishers_by_subscriber.get("s2"),
        Some(&vec!["P".to_string()])
    );
    // s3's edge was unresolved → no publisher mapping.
    assert_eq!(ix.publishers_by_subscriber.get("s3"), None);
}

#[test]
fn primary_routines_excludes_dep_set() {
    let g = synthetic_graph();
    let rs = vec![
        routine("P", "event-publisher"),
        routine("s1", "event-subscriber"),
        routine("dep1", "procedure"),
    ];
    let mut deps = BTreeSet::new();
    deps.insert("dep1".to_string());
    let ix = build_event_flow_indexes(&g, &rs, &deps);

    assert!(ix.primary_routines.contains("P"));
    assert!(ix.primary_routines.contains("s1"));
    assert!(!ix.primary_routines.contains("dep1"));
}

// ---------------------------------------------------------------------------
// query helpers
// ---------------------------------------------------------------------------

#[test]
fn get_subscribers_of_publisher_unions_sorted() {
    // P publishes E1 {s1,s2} and E2 {s2,s4}; union sorted dedup = s1,s2,s4.
    let g = EventGraph {
        events: vec![
            ev("E1", Some("P"), "OnFoo", "integration"),
            ev("E2", Some("P"), "OnBar", "integration"),
        ],
        edges: vec![
            edge("E1", "s2", "resolved"),
            edge("E1", "s1", "resolved"),
            edge("E2", "s4", "resolved"),
            edge("E2", "s2", "resolved"),
        ],
    };
    let rs = vec![routine("P", "event-publisher")];
    let ix = build_event_flow_indexes(&g, &rs, &no_deps());
    assert_eq!(
        get_subscribers_of_publisher("P", &ix),
        vec!["s1".to_string(), "s2".to_string(), "s4".to_string()]
    );
    // Unknown publisher → empty.
    assert!(get_subscribers_of_publisher("nope", &ix).is_empty());
}

#[test]
fn get_publishers_for_subscriber_and_of_event() {
    let g = synthetic_graph();
    let rs = vec![routine("P", "event-publisher")];
    let ix = build_event_flow_indexes(&g, &rs, &no_deps());

    assert_eq!(
        get_publishers_for_subscriber("s1", &ix),
        vec!["P".to_string()]
    );
    assert!(get_publishers_for_subscriber("s3", &ix).is_empty());

    assert_eq!(
        get_subscribers_of_event("E1", &ix),
        vec!["s1".to_string(), "s2".to_string()]
    );
    assert!(get_subscribers_of_event("E2", &ix).is_empty());

    assert_eq!(get_publisher_of_event("E1", &ix), Some("P".to_string()));
    assert_eq!(get_publisher_of_event("missing", &ix), None);
}

// ---------------------------------------------------------------------------
// compute_fanout
// ---------------------------------------------------------------------------

#[test]
fn compute_fanout_counts_and_coverage_states() {
    let g = synthetic_graph();
    let rs = vec![routine("P", "event-publisher")];
    let ix = build_event_flow_indexes(&g, &rs, &no_deps());

    // Summaries: s1 complete, s2 partial → E1 capabilityComposition = partial.
    let mut summaries: HashMap<String, FullRoutineSummary> = HashMap::new();
    summaries.insert(
        "s1".to_string(),
        summary("s1", vec![], vec![], Some(coverage("complete"))),
    );
    summaries.insert(
        "s2".to_string(),
        summary("s2", vec![], vec![], Some(coverage("partial"))),
    );

    let fan = compute_fanout(&g, &ix, &summaries);
    assert_eq!(fan.len(), 2);

    // Sorted by (publisher, eventName, eventId): OnBar(E2) before OnFoo(E1).
    let e2 = &fan[0];
    assert_eq!(e2.event_id, "E2");
    assert_eq!(e2.event_name, "OnBar");
    assert_eq!(e2.event_kind, "business");
    // E2: only an unresolved edge → 0 resolved subscribers.
    assert_eq!(e2.direct_subscriber_count, 0);
    // 1 total edge, 1 unresolved → dispatch partial; 0 subs → capability unknown.
    assert_eq!(e2.coverage.dispatch_edges, "partial");
    assert_eq!(e2.coverage.subscriber_discovery, "partial");
    assert_eq!(e2.coverage.capability_composition, "unknown");

    let e1 = &fan[1];
    assert_eq!(e1.event_id, "E1");
    assert_eq!(e1.event_name, "OnFoo");
    assert_eq!(e1.event_kind, "integration");
    assert_eq!(e1.direct_subscriber_count, 2);
    // 2 edges all resolved → dispatch complete; one subscriber partial → partial.
    assert_eq!(e1.coverage.dispatch_edges, "complete");
    assert_eq!(e1.coverage.capability_composition, "partial");
}

#[test]
fn compute_fanout_missing_summary_is_unknown() {
    // E1 has resolved subs but NO summary for one → capabilityComposition unknown.
    let g = EventGraph {
        events: vec![ev("E1", Some("P"), "OnFoo", "integration")],
        edges: vec![edge("E1", "s1", "resolved")],
    };
    let rs = vec![routine("P", "event-publisher")];
    let ix = build_event_flow_indexes(&g, &rs, &no_deps());
    let summaries: HashMap<String, FullRoutineSummary> = HashMap::new(); // none
    let fan = compute_fanout(&g, &ix, &summaries);
    assert_eq!(fan[0].coverage.capability_composition, "unknown");

    // With a summary that HAS coverage=complete → complete.
    let mut summaries2: HashMap<String, FullRoutineSummary> = HashMap::new();
    summaries2.insert(
        "s1".to_string(),
        summary("s1", vec![], vec![], Some(coverage("complete"))),
    );
    let fan2 = compute_fanout(&g, &ix, &summaries2);
    assert_eq!(fan2[0].coverage.capability_composition, "complete");

    // Summary present but NO coverage record → undefined status → unknown.
    let mut summaries3: HashMap<String, FullRoutineSummary> = HashMap::new();
    summaries3.insert("s1".to_string(), summary("s1", vec![], vec![], None));
    let fan3 = compute_fanout(&g, &ix, &summaries3);
    assert_eq!(fan3[0].coverage.capability_composition, "unknown");
}

#[test]
fn compute_fanout_report_scope_and_summary() {
    let g = synthetic_graph();
    let rs = vec![routine("P", "event-publisher")];
    let ix = build_event_flow_indexes(&g, &rs, &no_deps());
    let summaries: HashMap<String, FullRoutineSummary> = HashMap::new();

    let rep = compute_fanout_report(&g, &ix, &summaries, Scope::All);
    assert_eq!(rep.total_events, 2);
    assert_eq!(rep.total_publishers, 1);
    // E2 has 0 resolved subs → zeroSubscriberEvents counts it.
    assert_eq!(rep.zero_subscriber_events, 1);
    assert_eq!(rep.hot_events, 0);
    // E2 dispatch partial → coverage_partial_events counts it.
    assert_eq!(rep.coverage_partial_events, 1);
}

// ---------------------------------------------------------------------------
// walk_event_chain
// ---------------------------------------------------------------------------

#[test]
fn walk_event_chain_two_hop() {
    // P --E1--> s1 ; s1 --E2--> s2. Chain: root(P) -> dispatch(E1) -> sub(s1)
    //                                      -> dispatch(E2) -> sub(s2).
    let g = EventGraph {
        events: vec![
            ev("E1", Some("P"), "OnFoo", "integration"),
            ev("E2", Some("s1"), "OnBar", "integration"),
        ],
        edges: vec![edge("E1", "s1", "resolved"), edge("E2", "s2", "resolved")],
    };
    let rs = vec![
        routine("P", "event-publisher"),
        routine("s1", "event-publisher"),
    ];
    let ix = build_event_flow_indexes(&g, &rs, &no_deps());
    let tree = walk_event_chain("P", &ix, &ChainWalkOptions::default());

    assert_eq!(tree.kind, "root");
    assert_eq!(tree.routine_id, Some("P".to_string()));
    assert_eq!(tree.children.len(), 1);

    let disp_e1 = &tree.children[0];
    assert_eq!(disp_e1.kind, "event-dispatch");
    assert_eq!(disp_e1.event_id, Some("E1".to_string()));
    assert_eq!(disp_e1.event_name, Some("OnFoo".to_string()));
    assert_eq!(disp_e1.children.len(), 1);

    let sub_s1 = &disp_e1.children[0];
    assert_eq!(sub_s1.kind, "subscriber");
    assert_eq!(sub_s1.routine_id, Some("s1".to_string()));
    assert_eq!(sub_s1.children.len(), 1);

    let disp_e2 = &sub_s1.children[0];
    assert_eq!(disp_e2.kind, "event-dispatch");
    assert_eq!(disp_e2.event_id, Some("E2".to_string()));
    assert_eq!(disp_e2.children.len(), 1);

    let sub_s2 = &disp_e2.children[0];
    assert_eq!(sub_s2.kind, "subscriber");
    assert_eq!(sub_s2.routine_id, Some("s2".to_string()));
    assert!(sub_s2.children.is_empty());
}

#[test]
fn walk_event_chain_cycle_terminates() {
    // P --E1--> Q ; Q --E2--> P. Walking from P must detect the cycle when E2's
    // subscriber P is already on the active path — no infinite loop.
    let g = EventGraph {
        events: vec![
            ev("E1", Some("P"), "OnFoo", "integration"),
            ev("E2", Some("Q"), "OnBar", "integration"),
        ],
        edges: vec![edge("E1", "Q", "resolved"), edge("E2", "P", "resolved")],
    };
    let rs = vec![
        routine("P", "event-publisher"),
        routine("Q", "event-publisher"),
    ];
    let ix = build_event_flow_indexes(&g, &rs, &no_deps());
    let tree = walk_event_chain("P", &ix, &ChainWalkOptions::default());

    // root(P) -> dispatch(E1) -> sub(Q) -> dispatch(E2) -> sub(P){cycleDetected}
    let q = &tree.children[0].children[0];
    assert_eq!(q.routine_id, Some("Q".to_string()));
    let p_again = &q.children[0].children[0];
    assert_eq!(p_again.routine_id, Some("P".to_string()));
    assert!(p_again.cycle_detected);
    assert!(p_again.children.is_empty());
}

#[test]
fn walk_event_chain_depth_bound_truncates() {
    // P --E1--> s1 ; s1 --E2--> s2. With max_depth = 1, the subscribers of E1 would
    // land at depth 1 (>= max_depth) → the dispatch node is depth_truncated and has
    // NO subscriber children.
    let g = EventGraph {
        events: vec![
            ev("E1", Some("P"), "OnFoo", "integration"),
            ev("E2", Some("s1"), "OnBar", "integration"),
        ],
        edges: vec![edge("E1", "s1", "resolved"), edge("E2", "s2", "resolved")],
    };
    let rs = vec![
        routine("P", "event-publisher"),
        routine("s1", "event-publisher"),
    ];
    let ix = build_event_flow_indexes(&g, &rs, &no_deps());
    let tree = walk_event_chain(
        "P",
        &ix,
        &ChainWalkOptions {
            max_depth: Some(1),
            max_nodes: None,
        },
    );
    let disp = &tree.children[0];
    assert_eq!(disp.event_id, Some("E1".to_string()));
    assert!(disp.depth_truncated);
    assert!(disp.children.is_empty());
}

#[test]
fn walk_event_chain_node_budget_bounds() {
    // max_nodes = 1: root consumes the only node; expand sees budget <= 0 and emits
    // nothing.
    let g = EventGraph {
        events: vec![ev("E1", Some("P"), "OnFoo", "integration")],
        edges: vec![edge("E1", "s1", "resolved")],
    };
    let rs = vec![routine("P", "event-publisher")];
    let ix = build_event_flow_indexes(&g, &rs, &no_deps());
    let tree = walk_event_chain(
        "P",
        &ix,
        &ChainWalkOptions {
            max_depth: None,
            max_nodes: Some(1),
        },
    );
    assert!(tree.children.is_empty());
}

#[test]
fn compute_chain_report_stats() {
    // P --E1--> Q ; Q --E2--> P (cycle). Stats: 1 root, max depth, 1 cycle.
    let g = EventGraph {
        events: vec![
            ev("E1", Some("P"), "OnFoo", "integration"),
            ev("E2", Some("Q"), "OnBar", "integration"),
        ],
        edges: vec![edge("E1", "Q", "resolved"), edge("E2", "P", "resolved")],
    };
    let rs = vec![
        routine("P", "event-publisher"),
        routine("Q", "event-publisher"),
    ];
    let ix = build_event_flow_indexes(&g, &rs, &no_deps());
    let rep = compute_chain_report(&ix, &ChainWalkOptions::default(), Scope::All);

    // Two publisher roots (P, Q) each produce a chain.
    assert_eq!(rep.total_roots, 2);
    assert_eq!(rep.roots_with_events, 2);
    // At least one cycle detected across the two roots.
    assert!(rep.cycles_detected >= 1);
}

// ---------------------------------------------------------------------------
// collect_relay_subscribers — direct oracles
// ---------------------------------------------------------------------------

/// Build a minimal `CombinedEdge` (direct call) from `from` to `to`.
fn call_edge(from: &str, to: &str) -> CombinedEdge {
    CombinedEdge {
        from: from.to_string(),
        to: to.to_string(),
        kind: "direct".to_string(),
        callsite_id: None,
        operation_id: None,
        event_id: None,
        subscriber_app_id: None,
        resolution: "resolved".to_string(),
    }
}

/// Build the `edges_by_from` map from a list of `(from, to)` pairs.
fn edges_map(pairs: &[(&str, &str)]) -> HashMap<String, Vec<CombinedEdge>> {
    let mut m: HashMap<String, Vec<CombinedEdge>> = HashMap::new();
    for &(f, t) in pairs {
        m.entry(f.to_string()).or_default().push(call_edge(f, t));
    }
    m
}

/// Oracle 1: relay cycle terminates.
///
/// Setup: publisher P publishes E1; subscriber Sub1 subscribes to E1.
/// Sub1's body calls relay publisher RelayP. RelayP publishes E2.
/// Sub2 subscribes to E2. Sub2's body calls back to P (cycle back to the root).
/// The `visited_pubs` guard must prevent infinite re-expansion of P.
/// Expected result: {Sub1 → depth 1, Sub2 → depth 2} — the cycle edge back to P
/// is silently dropped (P is already in `visited_pubs` from the start).
#[test]
fn collect_relay_subscribers_cycle_terminates() {
    let g = EventGraph {
        events: vec![
            ev("E1", Some("P"), "OnFoo", "integration"),
            ev("E2", Some("RelayP"), "OnBar", "integration"),
        ],
        edges: vec![
            edge("E1", "Sub1", "resolved"),
            edge("E2", "Sub2", "resolved"),
        ],
    };
    let rs = vec![
        routine("P", "event-publisher"),
        routine("RelayP", "event-publisher"),
        routine("Sub1", "event-subscriber"),
        routine("Sub2", "event-subscriber"),
    ];
    let ix = build_event_flow_indexes(&g, &rs, &no_deps());

    // Sub1 calls RelayP (relay hop); Sub2 calls P (cycle — P already visited).
    let edges = edges_map(&[("Sub1", "RelayP"), ("Sub2", "P")]);

    let result = collect_relay_subscribers("P", &ix, &edges, &RelayWalkOptions::default());

    // Sub1 is a direct subscriber of P at depth 1.
    assert_eq!(result.get("Sub1"), Some(&1), "Sub1 must be at depth 1");
    // Sub2 is reached via RelayP at depth 2.
    assert_eq!(result.get("Sub2"), Some(&2), "Sub2 must be at depth 2");
    // P itself must NOT appear as a subscriber (it is visited_pubs, not a result entry).
    assert!(
        !result.contains_key("P"),
        "cycle back to P must be suppressed"
    );
    // Exactly two subscribers discovered.
    assert_eq!(result.len(), 2, "exactly Sub1 and Sub2 in result");
}

/// Oracle 2: MAX_DEPTH=4 truncation — subscribers beyond depth 4 are absent.
///
/// Setup: a linear relay chain of depth 5.
///   P --E1--> S1 ; S1 calls R2 --E2--> S2 ; S2 calls R3 --E3--> S3 ;
///   S3 calls R4 --E4--> S4 ; S4 calls R5 --E5--> S5.
/// With the default max_depth=4: S1(1), S2(2), S3(3), S4(4) are collected;
/// S5 would land at depth 5 > 4 and must be absent.
#[test]
fn collect_relay_subscribers_max_depth_truncates() {
    let g = EventGraph {
        events: vec![
            ev("E1", Some("P"), "OnE1", "integration"),
            ev("E2", Some("R2"), "OnE2", "integration"),
            ev("E3", Some("R3"), "OnE3", "integration"),
            ev("E4", Some("R4"), "OnE4", "integration"),
            ev("E5", Some("R5"), "OnE5", "integration"),
        ],
        edges: vec![
            edge("E1", "S1", "resolved"),
            edge("E2", "S2", "resolved"),
            edge("E3", "S3", "resolved"),
            edge("E4", "S4", "resolved"),
            edge("E5", "S5", "resolved"),
        ],
    };
    let rs = vec![
        routine("P", "event-publisher"),
        routine("R2", "event-publisher"),
        routine("R3", "event-publisher"),
        routine("R4", "event-publisher"),
        routine("R5", "event-publisher"),
    ];
    let ix = build_event_flow_indexes(&g, &rs, &no_deps());

    // Relay chain: each subscriber Si calls the next relay publisher Ri+1.
    let edges = edges_map(&[("S1", "R2"), ("S2", "R3"), ("S3", "R4"), ("S4", "R5")]);

    let opts = RelayWalkOptions {
        max_depth: 4,
        max_nodes: 256,
    };
    let result = collect_relay_subscribers("P", &ix, &edges, &opts);

    // S1..S4 must be present at their respective depths.
    assert_eq!(result.get("S1"), Some(&1), "S1 at depth 1");
    assert_eq!(result.get("S2"), Some(&2), "S2 at depth 2");
    assert_eq!(result.get("S3"), Some(&3), "S3 at depth 3");
    assert_eq!(result.get("S4"), Some(&4), "S4 at depth 4");
    // S5 would be at depth 5 — must be absent (max_depth guard).
    assert!(
        !result.contains_key("S5"),
        "S5 beyond max_depth must be absent"
    );
}

/// Oracle 3: min-depth-on-shorter-path update.
///
/// A subscriber reachable via two paths — one longer, one shorter — must be
/// recorded at the SHORTER depth.
///
/// Setup:
///   P --E1--> DirectSub (depth 1, direct subscriber).
///   P --E1--> RelaySub1 (depth 1, also a direct subscriber).
///   RelaySub1 calls RelayP --E2--> DirectSub (depth 2, longer path).
///
/// DirectSub is reachable at depth 1 (directly) AND depth 2 (via relay).
/// The result must record depth 1, not 2.
#[test]
fn collect_relay_subscribers_records_min_depth() {
    let g = EventGraph {
        events: vec![
            ev("E1", Some("P"), "OnFoo", "integration"),
            ev("E2", Some("RelayP"), "OnBar", "integration"),
        ],
        edges: vec![
            // DirectSub is a direct (depth-1) subscriber of E1.
            edge("E1", "DirectSub", "resolved"),
            // RelaySub1 is also a direct subscriber; its body relays to RelayP.
            edge("E1", "RelaySub1", "resolved"),
            // DirectSub is ALSO a subscriber of E2 (depth-2 path via RelayP).
            edge("E2", "DirectSub", "resolved"),
        ],
    };
    let rs = vec![
        routine("P", "event-publisher"),
        routine("RelayP", "event-publisher"),
        routine("RelaySub1", "event-subscriber"),
        routine("DirectSub", "event-subscriber"),
    ];
    let ix = build_event_flow_indexes(&g, &rs, &no_deps());

    // RelaySub1 relays to RelayP.
    let edges = edges_map(&[("RelaySub1", "RelayP")]);

    let result = collect_relay_subscribers("P", &ix, &edges, &RelayWalkOptions::default());

    // DirectSub must be at depth 1 (the shorter path wins).
    assert_eq!(
        result.get("DirectSub"),
        Some(&1),
        "DirectSub must be recorded at depth 1 (min-depth), not 2"
    );
    // RelaySub1 is at depth 1 too.
    assert_eq!(result.get("RelaySub1"), Some(&1), "RelaySub1 at depth 1");
}

#[test]
fn compute_chain_report_primary_scope_filters() {
    // P primary, dep1 a dependency publisher with a dep-only subtree. scope=Primary
    // keeps only chains touching a primary routine.
    let g = EventGraph {
        events: vec![
            ev("E1", Some("P"), "OnFoo", "integration"),
            ev("E2", Some("dep1"), "OnBar", "integration"),
        ],
        edges: vec![
            edge("E1", "P_sub", "resolved"),
            edge("E2", "dep2", "resolved"),
        ],
    };
    let rs = vec![
        routine("P", "event-publisher"),
        routine("P_sub", "event-subscriber"),
        routine("dep1", "event-publisher"),
        routine("dep2", "event-subscriber"),
    ];
    let mut deps = BTreeSet::new();
    deps.insert("dep1".to_string());
    deps.insert("dep2".to_string());
    let ix = build_event_flow_indexes(&g, &rs, &deps);

    let all = compute_chain_report(&ix, &ChainWalkOptions::default(), Scope::All);
    assert_eq!(all.total_roots, 2);

    let primary = compute_chain_report(&ix, &ChainWalkOptions::default(), Scope::Primary);
    // dep1's chain touches no primary routine → dropped; P's chain kept.
    assert_eq!(primary.total_roots, 1);
    assert_eq!(primary.chains[0].routine_id, Some("P".to_string()));
}

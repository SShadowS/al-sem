//! `event_flow` — Rust port of al-sem `src/engine/event-flow.ts` (the shared
//! event-flow substrate the d43/d44/d45 detectors consume). NO detectors here —
//! this is the index + query + fan-out + chain-walk substrate only.
//!
//! What is ported (faithfully, 1:1 with al-sem):
//!   - `EventKind` + `event_kind_of` (business/internal pass-through, else
//!     integration default — "trigger"/"unknown"/"integration" all map to
//!     "integration").
//!   - `EventFlowIndexes` + `build_event_flow_indexes` — the six lookup tables.
//!   - `get_subscribers_of_publisher` / `get_publishers_for_subscriber` /
//!     `get_subscribers_of_event` / `get_publisher_of_event`.
//!   - `FanoutEntry` / `FanoutCoverage` + `compute_fanout` (the fan-out list with
//!     the tri-state coverage states) + `compute_fanout_report`.
//!   - `walk_event_chain` (the bounded transitive event-chain walk) +
//!     `compute_chain_report`.
//!   - `collect_relay_subscribers` (the bounded transitive subscriber walk d45 uses —
//!     bridges event-graph dispatches with call-graph relay edges).
//!   - `is_handled_re` — hand-rolled matcher for al-sem `IS_HANDLED_RE`.
//!   - `publisher_branch_facts` — the d43 branch-slice surface (reads the L3-forwarded
//!     `var_assignments` / `has_branching` + the publisher summary's direct facts).
//!     Ported with the d43/d44/d45 detector wave (previously deferred).
//!   - `build_cross_extension_subscribers` — per-event cross-app subscriber lookup
//!     keyed off the OBJECT-app map (`objects[].app_guid` via
//!     `EventSymbol.publisher_object_id`) vs `EventEdge.subscriber_app_id`. D43/D44/D45
//!     evidence-only (populates `Finding.cross_extension_subscribers`; no detector
//!     filtering). Ported with the detector wave (previously deferred).
//!
//! === Determinism ===
//! al-sem freezes every derived list with `[...set].sort(compareStrings)`
//! (byte-order, ASCII). The Rust port uses `BTreeMap`/`BTreeSet` keyed by `String`
//! (whose `Ord` is byte-order, matching `compareStrings`) so every value list and
//! every key iteration is already sorted into output. `event_name_by_event` and
//! `publisher_by_event` are `BTreeMap` too; `primary_routines` is a `BTreeSet`.
//! No `HashMap` iteration order reaches output.
//!
//! === walk_event_chain bounds (LOCKED, mirrored from al-sem) ===
//! Defaults: max_depth = 16, max_nodes = 1024. Truncation precedence:
//!   1. cycle wins when the next edge revisits a node already on the active path
//!   2. depth wins when expansion would exceed max_depth before evaluating children
//!   3. nodes wins globally once adding another node would exceed max_nodes
//!
//! `on_path` is the active-path set (added on enter, removed on exit) so sibling
//! subtrees do NOT see each other as cycles — only ancestors count. The root
//! consumes one node from the budget BEFORE the first expand, exactly as al-sem
//! decrements `nodeBudget` before calling `expand(root, 0)`.

use std::collections::{BTreeMap, BTreeSet};

use crate::engine::l4::capability_cone::CapabilityFact;
use crate::engine::l5::full_summary::FullRoutineSummary;
use crate::program::model::events::EventGraph;
use crate::program::model::workspace::{ModelObject, ModelRoutine};

// ---------------------------------------------------------------------------
// IS_HANDLED_RE — al-sem `/^is.?handled$/i`.
// ---------------------------------------------------------------------------

/// Faithful hand-rolled matcher for al-sem's `IS_HANDLED_RE = /^is.?handled$/i`.
///
/// The regex is anchored both ends, case-insensitive. `.?` matches ZERO OR ONE of
/// any character (Unicode code point) EXCEPT a newline (`'\n'`). So the accepted
/// strings are exactly:
///   - 9 chars: matches `"ishandled"` case-insensitively (`.?` matched empty), or
///   - 10 chars: `chars[0..2]` match `"is"` (ci), `chars[2] != '\n'`, `chars[3..10]`
///     match `"handled"` (ci).
///
/// Operates on `name.chars()` — NO whole-string `to_lowercase` — so multi-byte
/// middle characters (`"is😀handled"`, `"isßhandled"`) and lowercase-expanding chars
/// are handled faithfully.
pub fn is_handled_re(name: &str) -> bool {
    let chars: Vec<char> = name.chars().collect();
    let is_lit: [char; 2] = ['i', 's'];
    let handled_lit: [char; 7] = ['h', 'a', 'n', 'd', 'l', 'e', 'd'];

    match chars.len() {
        9 => {
            // ".?" matched empty: "ishandled" (ci).
            chars[0..2]
                .iter()
                .zip(is_lit.iter())
                .all(|(c, t)| c.eq_ignore_ascii_case(t))
                && chars[2..9]
                    .iter()
                    .zip(handled_lit.iter())
                    .all(|(c, t)| c.eq_ignore_ascii_case(t))
        }
        10 => {
            // ".?" matched one non-newline char.
            chars[0..2]
                .iter()
                .zip(is_lit.iter())
                .all(|(c, t)| c.eq_ignore_ascii_case(t))
                && chars[2] != '\n'
                && chars[3..10]
                    .iter()
                    .zip(handled_lit.iter())
                    .all(|(c, t)| c.eq_ignore_ascii_case(t))
        }
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// EventKind
// ---------------------------------------------------------------------------

/// Mirrors al-sem `EventKind`. Carried as a `&'static str` ("integration" |
/// "business" | "internal") so it serializes / compares identically.
pub type EventKind = &'static str;

/// `eventKindOf(k)` — business/internal pass through; everything else
/// ("trigger" / "unknown" / "integration") defaults to "integration".
pub fn event_kind_of(k: &str) -> EventKind {
    match k {
        "business" => "business",
        "internal" => "internal",
        _ => "integration",
    }
}

// ---------------------------------------------------------------------------
// EventFlowIndexes
// ---------------------------------------------------------------------------

/// Tri-state coverage for a single fan-out entry. Mirrors al-sem `FanoutCoverage`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FanoutCoverage {
    /// "complete" | "partial" | "unknown".
    pub dispatch_edges: &'static str,
    pub subscriber_discovery: &'static str,
    pub capability_composition: &'static str,
}

/// One fan-out row. Mirrors al-sem `FanoutEntry` (omits the subscriber id list —
/// only the count; re-query the index for the list).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FanoutEntry {
    pub publisher: String,
    pub event_id: String,
    pub event_name: String,
    pub event_kind: EventKind,
    pub direct_subscriber_count: usize,
    pub coverage: FanoutCoverage,
}

/// The six event-flow lookup tables. Every value list is SORTED (byte order); the
/// `BTreeMap`/`BTreeSet` keys iterate in byte order too — matching al-sem's
/// `sortedFreeze`.
#[derive(Debug, Clone, Default)]
pub struct EventFlowIndexes {
    /// EventId → publisher RoutineId (when present).
    pub publisher_by_event: BTreeMap<String, String>,
    /// Publisher RoutineId → sorted unique EventIds it publishes.
    pub events_by_publisher: BTreeMap<String, Vec<String>>,
    /// EventId → sorted unique subscriber RoutineIds.
    pub subscribers_by_event: BTreeMap<String, Vec<String>>,
    /// Subscriber RoutineId → sorted unique publisher RoutineIds.
    pub publishers_by_subscriber: BTreeMap<String, Vec<String>>,
    /// EventId → event name (node labels for `walk_event_chain`).
    pub event_name_by_event: BTreeMap<String, String>,
    /// RoutineIds whose analysis role is primary (NOT in the dep set).
    pub primary_routines: BTreeSet<String>,
}

/// Build the event-flow indexes from the L3 event graph + the routine set + the
/// dependency-routine set (source-only ⇒ empty dep set ⇒ every routine primary).
///
/// - `events_by_publisher` / `event_name_by_event` / `publisher_by_event` come
///   from `event_graph.events` (publisher routine + name + kind label).
/// - `subscribers_by_event` / `publishers_by_subscriber` come from
///   `event_graph.edges` where `resolution == "resolved"` ONLY (unresolved edges
///   are excluded — a maybe/unknown edge does NOT prove a subscriber/publisher
///   relationship).
/// - `primary_routines` = routines whose id is NOT in `dep_routine_ids`
///   (al-sem `roleOf(r) != "dependency"`).
pub fn build_event_flow_indexes(
    event_graph: &EventGraph,
    routines: &[ModelRoutine],
    dep_routine_ids: &BTreeSet<String>,
) -> EventFlowIndexes {
    let mut publisher_by_event: BTreeMap<String, String> = BTreeMap::new();
    let mut events_by_publisher_set: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut event_name_by_event: BTreeMap<String, String> = BTreeMap::new();

    let mut primary_routines: BTreeSet<String> = BTreeSet::new();
    for r in routines {
        if !dep_routine_ids.contains(&r.id) {
            primary_routines.insert(r.id.clone());
        }
    }

    for ev in &event_graph.events {
        event_name_by_event.insert(ev.id.clone(), ev.event_name.clone());
        if let Some(pub_routine) = &ev.publisher_routine_id {
            publisher_by_event.insert(ev.id.clone(), pub_routine.clone());
            events_by_publisher_set
                .entry(pub_routine.clone())
                .or_default()
                .insert(ev.id.clone());
        }
    }

    let mut subscribers_by_event_set: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut publishers_by_subscriber_set: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for e in &event_graph.edges {
        if e.resolution != "resolved" {
            continue;
        }
        subscribers_by_event_set
            .entry(e.event_id.clone())
            .or_default()
            .insert(e.subscriber_routine_id.clone());
        if let Some(pubr) = publisher_by_event.get(&e.event_id) {
            publishers_by_subscriber_set
                .entry(e.subscriber_routine_id.clone())
                .or_default()
                .insert(pubr.clone());
        }
    }

    // Freeze BTreeSet → sorted Vec (BTreeSet already iterates in byte order).
    let freeze = |m: BTreeMap<String, BTreeSet<String>>| -> BTreeMap<String, Vec<String>> {
        m.into_iter()
            .map(|(k, set)| (k, set.into_iter().collect()))
            .collect()
    };

    EventFlowIndexes {
        publisher_by_event,
        events_by_publisher: freeze(events_by_publisher_set),
        subscribers_by_event: freeze(subscribers_by_event_set),
        publishers_by_subscriber: freeze(publishers_by_subscriber_set),
        event_name_by_event,
        primary_routines,
    }
}

// ---------------------------------------------------------------------------
// Query helpers
// ---------------------------------------------------------------------------

/// All subscribers (sorted, deduped) reachable from a publisher routine across
/// every event it publishes. Mirrors al-sem `getSubscribersOfPublisher`.
pub fn get_subscribers_of_publisher(publisher: &str, ix: &EventFlowIndexes) -> Vec<String> {
    let mut out: BTreeSet<String> = BTreeSet::new();
    if let Some(events) = ix.events_by_publisher.get(publisher) {
        for ev in events {
            if let Some(subs) = ix.subscribers_by_event.get(ev) {
                for sub in subs {
                    out.insert(sub.clone());
                }
            }
        }
    }
    out.into_iter().collect()
}

/// The publishers (already sorted) a subscriber routine listens to. Mirrors
/// al-sem `getPublishersForSubscriber` (returns the frozen sorted list, or empty).
pub fn get_publishers_for_subscriber(subscriber: &str, ix: &EventFlowIndexes) -> Vec<String> {
    ix.publishers_by_subscriber
        .get(subscriber)
        .cloned()
        .unwrap_or_default()
}

/// The subscribers (already sorted) of a single event. Mirrors al-sem
/// `getSubscribersOfEvent`.
pub fn get_subscribers_of_event(event_id: &str, ix: &EventFlowIndexes) -> Vec<String> {
    ix.subscribers_by_event
        .get(event_id)
        .cloned()
        .unwrap_or_default()
}

/// The publisher routine of a single event, if any. Mirrors al-sem
/// `getPublisherOfEvent`.
pub fn get_publisher_of_event(event_id: &str, ix: &EventFlowIndexes) -> Option<String> {
    ix.publisher_by_event.get(event_id).cloned()
}

// ---------------------------------------------------------------------------
// compute_fanout
// ---------------------------------------------------------------------------

/// Build the fan-out list — one entry per published event with a publisher
/// routine. `summaries` supplies the per-subscriber coverage for the
/// `capability_composition` tri-state (al-sem reads `r.summary.coverage
/// .inheritedStatus`; the Rust pipeline carries summaries in a side map).
///
/// Coverage states (mirrored exactly):
///   - `dispatch_edges`: "complete" if there are NO edges OR every edge is
///     resolved; "partial" if any edge is unresolved.
///   - `subscriber_discovery`: mirrors `dispatch_edges` (al-sem comment: refined
///     when per-event coverage substrate lands).
///   - `capability_composition`: "unknown" if there are zero resolved subscribers;
///     else over the resolved subscribers — "unknown" if ANY has no summary
///     (sawMissing), else "partial" if ANY summary's inheritedStatus is "partial"
///     or "unknown", else "complete".
pub fn compute_fanout(
    event_graph: &EventGraph,
    ix: &EventFlowIndexes,
    summaries: &std::collections::HashMap<String, FullRoutineSummary>,
) -> Vec<FanoutEntry> {
    // Group ALL edges (resolved + unresolved) by eventId for dispatchEdges.
    // Unresolved-edge count per event drives dispatchEdges. al-sem also tallies
    // `allEdges.length` but only to special-case zero edges → "complete", which
    // `unresolved == 0` already covers (zero edges ⇒ zero unresolved).
    let mut unresolved_by_event: BTreeMap<String, usize> = BTreeMap::new();
    for e in &event_graph.edges {
        let entry = unresolved_by_event.entry(e.event_id.clone()).or_insert(0);
        if e.resolution != "resolved" {
            *entry += 1;
        }
    }

    let empty: Vec<String> = Vec::new();
    let mut out: Vec<FanoutEntry> = Vec::new();

    for ev in &event_graph.events {
        let Some(publisher) = &ev.publisher_routine_id else {
            continue;
        };

        let resolved_subs = ix.subscribers_by_event.get(&ev.id).unwrap_or(&empty);
        let unresolved_edges = unresolved_by_event.get(&ev.id).copied().unwrap_or(0);

        // al-sem: `allEdges.length === 0 ? "complete" : unresolvedEdges === 0 ?
        // "complete" : "partial"` — i.e. complete unless there is an unresolved
        // edge. Collapsed (clippy if_same_then_else) to the logically identical
        // single predicate; zero edges ⇒ zero unresolved ⇒ "complete".
        let dispatch_edges: &'static str = if unresolved_edges == 0 {
            "complete"
        } else {
            "partial"
        };

        // subscriberDiscovery mirrors dispatchEdges for now.
        let subscriber_discovery = dispatch_edges;

        // capabilityComposition: derive from the worst subscriber summary coverage.
        let capability_composition: &'static str = if resolved_subs.is_empty() {
            "unknown"
        } else {
            let mut saw_partial = false;
            let mut saw_missing = false;
            for sub in resolved_subs {
                match summaries.get(sub) {
                    None => saw_missing = true,
                    Some(sum) => match sum.coverage.as_ref().map(|c| c.inherited_status.as_str()) {
                        // al-sem reads `sum?.coverage?.inheritedStatus`; a summary
                        // present but WITHOUT a coverage record ⇒ status undefined ⇒
                        // sawMissing (matches `status === undefined`).
                        None => saw_missing = true,
                        Some("partial") | Some("unknown") => saw_partial = true,
                        Some(_) => {}
                    },
                }
            }
            if saw_missing {
                "unknown"
            } else if saw_partial {
                "partial"
            } else {
                "complete"
            }
        };

        out.push(FanoutEntry {
            publisher: publisher.clone(),
            event_id: ev.id.clone(),
            event_name: ev.event_name.clone(),
            event_kind: event_kind_of(&ev.event_kind),
            direct_subscriber_count: resolved_subs.len(),
            coverage: FanoutCoverage {
                dispatch_edges,
                subscriber_discovery,
                capability_composition,
            },
        });
    }

    out.sort_by(|a, b| {
        a.publisher
            .cmp(&b.publisher)
            .then_with(|| a.event_name.cmp(&b.event_name))
            .then_with(|| a.event_id.cmp(&b.event_id))
    });

    out
}

// ---------------------------------------------------------------------------
// walk_event_chain
// ---------------------------------------------------------------------------

/// Emitted kinds: "root", "event-dispatch", "subscriber". "publisher" is reserved
/// (al-sem comment) but never emitted.
pub type ChainNodeKind = &'static str;

/// One node in the event-chain tree. Mirrors al-sem `ChainNode`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChainNode {
    pub kind: ChainNodeKind,
    pub routine_id: Option<String>,
    pub event_id: Option<String>,
    pub event_name: Option<String>,
    pub children: Vec<ChainNode>,
    pub cycle_detected: bool,
    pub depth_truncated: bool,
}

/// Walk options. `None` ⇒ defaults (max_depth = 16, max_nodes = 1024).
#[derive(Debug, Clone, Default)]
pub struct ChainWalkOptions {
    pub max_depth: Option<usize>,
    pub max_nodes: Option<usize>,
}

const DEFAULT_MAX_DEPTH: usize = 16;
const DEFAULT_MAX_NODES: usize = 1024;

/// Walk the event-chain tree from a root publisher RoutineId. Follows ONLY
/// event-graph edges (publisher → subscriber via `events_by_publisher` +
/// `subscribers_by_event`); never call-graph relays. A pure function of `ix`.
///
/// Truncation precedence (LOCKED): cycle > depth > nodes. See module docs.
pub fn walk_event_chain(root: &str, ix: &EventFlowIndexes, opts: &ChainWalkOptions) -> ChainNode {
    let max_depth = opts.max_depth.unwrap_or(DEFAULT_MAX_DEPTH);
    let max_nodes = opts.max_nodes.unwrap_or(DEFAULT_MAX_NODES);

    struct Walker<'a> {
        ix: &'a EventFlowIndexes,
        max_depth: usize,
        node_budget: i64,
        on_path: BTreeSet<String>,
    }

    impl Walker<'_> {
        // `event_depth` is the depth at which the event-dispatch nodes emitted by
        // this call land. Root is conceptually at depth -1; the initial call passes
        // 0 so the root's immediate events land at depth 0.
        fn expand(&mut self, routine: &str, event_depth: usize) -> Vec<ChainNode> {
            if event_depth >= self.max_depth {
                return Vec::new();
            }
            if self.node_budget <= 0 {
                return Vec::new();
            }
            self.on_path.insert(routine.to_string());
            let mut out: Vec<ChainNode> = Vec::new();
            let events = self
                .ix
                .events_by_publisher
                .get(routine)
                .cloned()
                .unwrap_or_default();
            for ev in &events {
                if self.node_budget <= 0 {
                    break;
                }
                self.node_budget -= 1;

                let subs = self
                    .ix
                    .subscribers_by_event
                    .get(ev)
                    .cloned()
                    .unwrap_or_default();
                let mut sub_children: Vec<ChainNode> = Vec::new();
                let mut depth_trunc = false;

                if event_depth + 1 >= self.max_depth {
                    // Subscribers would land at depth >= max_depth — truncate entirely.
                    depth_trunc = true;
                } else {
                    for sub in &subs {
                        if self.node_budget <= 0 {
                            break;
                        }
                        self.node_budget -= 1;
                        // Cycle check wins over depth check (precedence 1 > 2).
                        if self.on_path.contains(sub) {
                            sub_children.push(ChainNode {
                                kind: "subscriber",
                                routine_id: Some(sub.clone()),
                                event_id: None,
                                event_name: None,
                                children: Vec::new(),
                                cycle_detected: true,
                                depth_truncated: false,
                            });
                            continue;
                        }
                        let grand = self.expand(sub, event_depth + 2);
                        sub_children.push(ChainNode {
                            kind: "subscriber",
                            routine_id: Some(sub.clone()),
                            event_id: None,
                            event_name: None,
                            children: grand,
                            cycle_detected: false,
                            depth_truncated: false,
                        });
                    }
                }

                out.push(ChainNode {
                    kind: "event-dispatch",
                    routine_id: None,
                    event_id: Some(ev.clone()),
                    event_name: self.ix.event_name_by_event.get(ev).cloned(),
                    children: sub_children,
                    cycle_detected: false,
                    depth_truncated: depth_trunc,
                });
            }
            self.on_path.remove(routine);
            out
        }
    }

    let mut walker = Walker {
        ix,
        max_depth,
        node_budget: max_nodes as i64,
        on_path: BTreeSet::new(),
    };
    walker.node_budget -= 1; // root counts as one node
    let children = walker.expand(root, 0);
    ChainNode {
        kind: "root",
        routine_id: Some(root.to_string()),
        event_id: None,
        event_name: None,
        children,
        cycle_detected: false,
        depth_truncated: false,
    }
}

// ---------------------------------------------------------------------------
// Report composition
// ---------------------------------------------------------------------------

/// Output scope. "primary" keeps only the workspace's own app(s); "all" keeps the
/// full merged model. Mirrors al-sem `Scope`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Primary,
    All,
}

/// Fan-out report — the entries plus the summary roll-up. Mirrors al-sem
/// `FanoutReport`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FanoutReport {
    pub entries: Vec<FanoutEntry>,
    pub total_publishers: usize,
    pub total_events: usize,
    pub zero_subscriber_events: usize,
    pub hot_events: usize,
    pub coverage_partial_events: usize,
}

/// Compute the fan-out report. `scope = Primary` keeps only entries where the
/// publisher is primary OR any subscriber of the event is primary (re-queried from
/// the index because `FanoutEntry` drops the subscriber id list).
pub fn compute_fanout_report(
    event_graph: &EventGraph,
    ix: &EventFlowIndexes,
    summaries: &std::collections::HashMap<String, FullRoutineSummary>,
    scope: Scope,
) -> FanoutReport {
    let all = compute_fanout(event_graph, ix, summaries);
    let empty: Vec<String> = Vec::new();
    let entries: Vec<FanoutEntry> = match scope {
        Scope::All => all,
        Scope::Primary => all
            .into_iter()
            .filter(|e| {
                ix.primary_routines.contains(&e.publisher)
                    || ix
                        .subscribers_by_event
                        .get(&e.event_id)
                        .unwrap_or(&empty)
                        .iter()
                        .any(|s| ix.primary_routines.contains(s))
            })
            .collect(),
    };

    let mut publishers: BTreeSet<&str> = BTreeSet::new();
    let mut zero = 0;
    let mut hot = 0;
    let mut partial = 0;
    for e in &entries {
        publishers.insert(e.publisher.as_str());
        if e.direct_subscriber_count == 0 {
            zero += 1;
        }
        if e.direct_subscriber_count > 5 {
            hot += 1;
        }
        if e.coverage.dispatch_edges == "partial" || e.coverage.capability_composition == "partial"
        {
            partial += 1;
        }
    }

    FanoutReport {
        total_publishers: publishers.len(),
        total_events: entries.len(),
        zero_subscriber_events: zero,
        hot_events: hot,
        coverage_partial_events: partial,
        entries,
    }
}

/// Chain report — the kept chains plus the summary roll-up. Mirrors al-sem
/// `ChainReport`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChainReport {
    pub chains: Vec<ChainNode>,
    pub total_roots: usize,
    pub roots_with_events: usize,
    pub max_chain_depth: usize,
    pub cycles_detected: usize,
    pub depth_truncated_nodes: usize,
}

struct ChainStatsAcc {
    max: usize,
    cycles: usize,
    depth_truncated: usize,
}

fn chain_stats(node: &ChainNode, depth: usize, acc: &mut ChainStatsAcc) {
    if depth > acc.max {
        acc.max = depth;
    }
    if node.cycle_detected {
        acc.cycles += 1;
    }
    if node.depth_truncated {
        acc.depth_truncated += 1;
    }
    for c in &node.children {
        chain_stats(c, depth + 1, acc);
    }
}

fn tree_touches_primary(node: &ChainNode, primary: &BTreeSet<String>) -> bool {
    if let Some(rid) = &node.routine_id
        && primary.contains(rid)
    {
        return true;
    }
    node.children
        .iter()
        .any(|c| tree_touches_primary(c, primary))
}

/// Compute the chain report over every publisher root (sorted by id). `scope =
/// Primary` keeps a tree only when a primary routine participates (root or any
/// descendant subscriber); stats accumulate over the KEPT trees.
pub fn compute_chain_report(
    ix: &EventFlowIndexes,
    opts: &ChainWalkOptions,
    scope: Scope,
) -> ChainReport {
    // events_by_publisher keys iterate in byte order (BTreeMap) — already sorted.
    let roots: Vec<String> = ix.events_by_publisher.keys().cloned().collect();
    let mut chains: Vec<ChainNode> = Vec::new();
    let mut acc = ChainStatsAcc {
        max: 0,
        cycles: 0,
        depth_truncated: 0,
    };
    for root in &roots {
        let tree = walk_event_chain(root, ix, opts);
        if scope == Scope::Primary && !tree_touches_primary(&tree, &ix.primary_routines) {
            continue;
        }
        chain_stats(&tree, 0, &mut acc);
        chains.push(tree);
    }
    ChainReport {
        // Roots are sourced from `events_by_publisher.keys()`, so every root has ≥1 event
        // by construction — `total_roots == roots_with_events` here (faithful to al-sem,
        // which keeps both fields for callers that build roots from a wider set).
        total_roots: chains.len(),
        roots_with_events: chains.len(),
        max_chain_depth: acc.max,
        cycles_detected: acc.cycles,
        depth_truncated_nodes: acc.depth_truncated,
        chains,
    }
}

// ---------------------------------------------------------------------------
// collect_relay_subscribers (port of al-sem engine/event-relay.ts)
// ---------------------------------------------------------------------------

/// Options for `collect_relay_subscribers`. Mirrors al-sem `RelayWalkOptions`.
#[derive(Debug, Clone)]
pub struct RelayWalkOptions {
    /// Max chain depth (depth 1 = direct subscriber of the root). Default 4.
    pub max_depth: usize,
    /// Max total subscribers discovered. Default 256.
    pub max_nodes: usize,
}

impl Default for RelayWalkOptions {
    fn default() -> Self {
        RelayWalkOptions {
            max_depth: 4,
            max_nodes: 256,
        }
    }
}

/// Walk the transitive subscriber chain from `publisher`, bridging event-graph
/// dispatches with call-graph relay edges. Returns `subscriber RoutineId → min chain
/// depth` (depth 1 = direct subscriber of the root). Faithful port of al-sem
/// `collectRelaySubscribers`.
///
/// Unlike `walk_event_chain` (event-graph only), this also follows call-graph edges
/// where a subscriber's BODY calls another event-publisher routine — a "relay" to the
/// next hop. `edges_by_from` is the combined-graph adjacency (`ctx.graph.edges_by_from`).
///
/// Deterministic: subscribers iterated in byte order at each hop; the call-graph relay
/// callees are iterated in the combined graph's existing edge order (already
/// edge-sort-key sorted at assembly).
pub fn collect_relay_subscribers(
    publisher: &str,
    ix: &EventFlowIndexes,
    edges_by_from: &std::collections::HashMap<
        String,
        Vec<crate::engine::l4::combined_graph::CombinedEdge>,
    >,
    opts: &RelayWalkOptions,
) -> BTreeMap<String, usize> {
    let max_depth = opts.max_depth;
    let max_nodes = opts.max_nodes;

    let mut result: BTreeMap<String, usize> = BTreeMap::new();
    // Queue of (publisher routine id, depth at which its direct subscribers land).
    let mut queue: std::collections::VecDeque<(String, usize)> = std::collections::VecDeque::new();
    queue.push_back((publisher.to_string(), 1));
    let mut visited_pubs: BTreeSet<String> = BTreeSet::new();
    visited_pubs.insert(publisher.to_string());

    while let Some((pub_id, depth)) = queue.pop_front() {
        if depth > max_depth {
            continue;
        }
        let events = ix
            .events_by_publisher
            .get(&pub_id)
            .cloned()
            .unwrap_or_default();
        for ev in &events {
            // subscribers already sorted (BTreeMap value list); al-sem re-sorts defensively.
            let subs = ix.subscribers_by_event.get(ev).cloned().unwrap_or_default();
            for sub in &subs {
                if result.len() >= max_nodes {
                    break;
                }
                // Record this subscriber at min depth.
                match result.get(sub) {
                    Some(&existing) if depth >= existing => {}
                    _ => {
                        result.insert(sub.clone(), depth);
                    }
                }
                if depth + 1 > max_depth {
                    continue;
                }
                // Follow call-graph edges from this subscriber to event-publisher callees.
                if let Some(call_edges) = edges_by_from.get(sub) {
                    for edge in call_edges {
                        let callee = &edge.to;
                        if !ix.events_by_publisher.contains_key(callee) {
                            continue;
                        }
                        if visited_pubs.contains(callee) {
                            continue;
                        }
                        visited_pubs.insert(callee.clone());
                        queue.push_back((callee.clone(), depth + 1));
                    }
                }
            }
        }
    }

    result
}

// ---------------------------------------------------------------------------
// build_cross_extension_subscribers
// ---------------------------------------------------------------------------

/// Per-event lookup of subscribers whose `subscriber_app_id` differs from the
/// PUBLISHER'S app guid. Returns a `BTreeMap<EventId, Vec<RoutineId>>` whose value
/// lists are sorted (byte order, matching al-sem `compareStrings`). Events with no
/// cross-app subscriber are ABSENT from the map. Used by D43/D44/D45 to populate
/// `Finding.cross_extension_subscribers` (evidence-only — never filters findings).
///
/// Faithful port of al-sem `buildCrossExtensionSubscribers`:
///   - `objectAppById` = `objects[].id → app_guid`.
///   - `eventPubApp[event] = objectAppById[event.publisher_object_id]` (the OBJECT
///     app, NOT an edge field).
///   - bucket each `edge` where `edge.subscriber_app_id != publisher_app`.
///
/// The publisher app is derived from the OBJECT-app map keyed by
/// `EventSymbol.publisher_object_id` — NOT from `EventEdge.subscriber_app_id`.
pub fn build_cross_extension_subscribers(
    event_graph: &EventGraph,
    objects: &[ModelObject],
) -> BTreeMap<String, Vec<String>> {
    // objectAppById: object id → app guid.
    let mut object_app_by_id: BTreeMap<&str, &str> = BTreeMap::new();
    for obj in objects {
        object_app_by_id.insert(obj.id.as_str(), obj.app_guid.as_str());
    }

    // eventPubApp: event id → publisher's app guid (via publisher_object_id).
    let mut event_pub_app: BTreeMap<&str, &str> = BTreeMap::new();
    for ev in &event_graph.events {
        if let Some(app) = object_app_by_id.get(ev.publisher_object_id.as_str()) {
            event_pub_app.insert(ev.id.as_str(), *app);
        }
    }

    // Bucket cross-extension subscriber routine ids per event (sorted, deduped).
    let mut buckets: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for edge in &event_graph.edges {
        let Some(pub_app) = event_pub_app.get(edge.event_id.as_str()) else {
            continue;
        };
        if edge.subscriber_app_id == *pub_app {
            continue;
        }
        buckets
            .entry(edge.event_id.clone())
            .or_default()
            .insert(edge.subscriber_routine_id.clone());
    }

    buckets
        .into_iter()
        .map(|(k, set)| (k, set.into_iter().collect()))
        .collect()
}

// ---------------------------------------------------------------------------
// publisher_branch_facts
// ---------------------------------------------------------------------------

/// Branch-slice confidence ladder. Mirrors al-sem `BranchSliceConfidence`.
pub type BranchSliceConfidence = &'static str;

/// The d43 branch-slice surface. Mirrors al-sem `PublisherBranchFacts`.
#[derive(Debug, Clone, PartialEq)]
pub struct PublisherBranchFacts {
    /// "exact" | "pattern" | "unknown".
    pub confidence: BranchSliceConfidence,
    /// Facts that fire when IsHandled is FALSE (the default branch). Conservative:
    /// ALL direct facts.
    pub default_branch_facts: Vec<CapabilityFact>,
    /// Facts that fire when IsHandled is TRUE (the guarded branch). Always empty
    /// (slicing deferred, mirroring al-sem).
    pub guarded_branch_facts: Vec<CapabilityFact>,
    /// Facts in neither branch (unconditional). Always empty.
    pub unguarded_facts: Vec<CapabilityFact>,
    /// Lowercased identifier name of the boolean guard parameter.
    pub guard_param_name: Option<String>,
}

/// `publisherBranchFacts(publisher, model)` — faithful port.
///
/// Returns `None` when the routine is absent OR has no IsHandled-shaped parameter.
/// The default-branch slice is conservative: ALL direct facts go to
/// `default_branch_facts`; `guarded`/`unguarded` are empty.
///
/// Confidence ladder:
///   - `exact`   — no branching at all (every direct fact is in the default branch).
///   - `pattern` — branching present AND IsHandled is assigned somewhere in the body.
///   - `unknown` — branching present but no IsHandled assignment detected.
///
/// `directFacts` = the publisher summary's `capability_facts_direct` (al-sem
/// `routine.summary?.capabilityFactsDirect ?? []`).
pub fn publisher_branch_facts(
    publisher: &str,
    routines: &[ModelRoutine],
    summaries: &std::collections::HashMap<String, FullRoutineSummary>,
) -> Option<PublisherBranchFacts> {
    let routine = routines.iter().find(|r| r.id == publisher)?;
    let guard_param = routine.parameters.iter().find(|p| is_handled_re(&p.name))?;
    let direct_facts: Vec<CapabilityFact> = summaries
        .get(publisher)
        .map(|s| s.capability_facts_direct.clone())
        .unwrap_or_default();
    let has_branching = routine.has_branching;
    let assigns_is_handled = routine
        .var_assignments
        .iter()
        .any(|a| is_handled_re(&a.lhs_name));
    let confidence: BranchSliceConfidence = if !has_branching {
        "exact"
    } else if assigns_is_handled {
        "pattern"
    } else {
        "unknown"
    };
    Some(PublisherBranchFacts {
        confidence,
        default_branch_facts: direct_facts,
        guarded_branch_facts: Vec::new(),
        unguarded_facts: Vec::new(),
        guard_param_name: Some(guard_param.name.to_lowercase()),
    })
}

#[cfg(test)]
mod tests;

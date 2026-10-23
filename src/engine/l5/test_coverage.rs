//! `digest --tests` (#254): which changed production routines some primary-app
//! `[Test]` procedure reaches over the resolved call graph, and why the rest are
//! not. Spec: `.agent/issue-254/ledger.md` (Design).
//!
//! Two layers:
//!   - [`CoverageGraph`] — the read-only input: test roots, test code, the
//!     followed edges (with an event bit), the per-routine incompleteness and
//!     dependency-boundary reasons, and the subscriber facts behind an
//!     `uncovered` reason. [`CoverageGraph::from_snapshot`] is the ONE place
//!     that knows the snapshot / model shape; when digest moves to another
//!     graph, only it changes.
//!   - [`compute_test_coverage`] — pure: a BFS per test root, ascending by stable
//!     id, over `(routine, crossedEvent)` states. The first root that reaches a
//!     routine is its witness; `viaEvent` is true when every shortest path from
//!     the witness crosses an `event-dispatch` edge.
//!
//! Every changed root counts (minus test code). One the digest query has no entry
//! for is `uncovered` with reason `not-in-graph`; the other reasons are
//! `declaration-only`, `platform-event-subscriber`, `dependency-event-subscriber`
//! and `no-path`.
//!
//! Determinism: the output lists are built from `BTreeMap`/`BTreeSet`s or sorted.
//! The `HashMap`s are lookups, or are iterated only into order-independent results
//! (a per-root BFS level map whose entries go into a first-root-wins map and into
//! sets).

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};

use serde::Serialize;

use crate::engine::l5::digest_cli::primary_guid_of;
use crate::engine::l5::snapshot::CapabilitySnapshot;
use al_syntax::IdentifierFoldExt;

use crate::program::attributes::{find_attribute, string_arg};
use crate::program::body::features::PCallee;
use crate::program::body::scope::unescape_al_identifier;
use crate::program::impact_graph::facts::{handler_names, test_page_target};
use crate::program::model::events::PublisherRef;
use crate::program::model::role::{Role, role_of_stable_id};
use crate::program::model::workspace::{Model, ModelRoutine};
use crate::program::resolve::receiver::unquote_identifier;

/// Why a subscriber is out of reach, beyond "no path" (spec §Why `uncovered`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct SubscriberFacts {
    /// It subscribes to a platform event: no publisher routine exists.
    pub(crate) platform: bool,
    /// It subscribes to a dependency event whose publisher is not a model
    /// routine (its app has none in the model, e.g. `--single-app`): no test
    /// can reach that publisher.
    pub(crate) outside_publisher: bool,
    /// The dependency publishers (model routines) of the events it subscribes to.
    pub(crate) dependency_publishers: BTreeSet<String>,
    /// It also subscribes to an event a primary-app routine publishes: being
    /// unreached is then a plain `no-path`, not a blind spot.
    pub(crate) primary_publisher: bool,
}

/// The coverage search's input, keyed by stable routine id.
#[derive(Debug, Clone, Default)]
pub struct CoverageGraph {
    /// Primary-app routines carrying `[Test]`.
    pub(crate) roots: BTreeSet<String>,
    /// The roots plus every routine of a primary-app `Subtype = Test` codeunit.
    pub(crate) test_code: BTreeSet<String>,
    /// from → `(to, is_event)`. Order and duplicates do not matter.
    pub(crate) edges: BTreeMap<String, Vec<(String, bool)>>,
    /// routine → its reasons of the "incomplete" bucket.
    pub(crate) incomplete: BTreeMap<String, BTreeSet<&'static str>>,
    /// routine → its dependency-boundary reasons.
    pub(crate) boundary: BTreeMap<String, BTreeSet<&'static str>>,
    /// subscriber → why it may be out of reach.
    pub(crate) subscriber_facts: BTreeMap<String, SubscriberFacts>,
}

/// What a changed root is to the search.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangedShape {
    /// A routine with a digest entry.
    Routine,
    /// An interface / ControlAddIn method declaration: no body, never a node.
    DeclarationOnly,
    /// A resolved root the digest query has no entry for (`root-not-in-snapshot`).
    NotInGraph,
}

/// One changed root of the digest.
#[derive(Debug, Clone)]
pub struct ChangedRoutine {
    pub stable_id: String,
    pub display: String,
    pub object_display: String,
    /// The entry's own `(file, line)` anchor, unchanged.
    pub anchor: Option<(String, u32)>,
    pub shape: ChangedShape,
}

/// The `reason` of an `uncovered` item; first match wins, in this order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum UncoveredReason {
    DeclarationOnly,
    NotInGraph,
    PlatformEventSubscriber,
    DependencyEventSubscriber,
    NoPath,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CoveredItem {
    pub stable_id: String,
    pub display: String,
    /// Exactly one id: the witness test.
    pub reached_from: Vec<String>,
    pub via_event: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Anchor {
    pub file: String,
    pub line: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UncoveredItem {
    pub stable_id: String,
    pub display: String,
    pub object_display: String,
    pub reason: UncoveredReason,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub anchor: Option<Anchor>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CoverageStatus {
    /// `"complete"` | `"partial"`.
    pub status: &'static str,
    /// Sorted, distinct.
    pub reasons: Vec<&'static str>,
    pub tests_with_unresolved_traversal: usize,
    pub tests_calling_dependency_code: usize,
}

/// The `payload.testCoverage` block.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TestCoverage {
    pub changed_routines: usize,
    pub covered_changed_routines: usize,
    /// `None` (JSON `null`, kept) when `changed_routines` is 0.
    pub ratio: Option<f64>,
    pub covered: Vec<CoveredItem>,
    pub uncovered: Vec<UncoveredItem>,
    pub test_procedures_considered: usize,
    pub excluded_test_code: usize,
    pub coverage: CoverageStatus,
}

impl TestCoverage {
    /// The block as JSON; `ratio` stays `null` when `None`.
    #[must_use]
    pub fn to_json_value(&self) -> serde_json::Value {
        serde_json::to_value(self).expect("TestCoverage serializes")
    }

    /// The human digest's one summary line (spec §Surface).
    #[must_use]
    pub fn human_line(&self) -> String {
        let status = match self.coverage.reasons.as_slice() {
            [] => "complete".to_string(),
            r => format!("partial: {}", r.join(", ")),
        };
        format!(
            "test coverage: {} of {} changed routines reached; {} [Test] procedures considered ({status})",
            self.covered_changed_routines, self.changed_routines, self.test_procedures_considered
        )
    }
}

/// Shortest levels from one root: `[without crossing an event, after crossing one]`.
type Levels<'g> = HashMap<&'g str, [Option<u32>; 2]>;

/// BFS over `(routine, crossedEvent)` states from `root`.
fn reach_from<'g>(root: &'g str, adj: &HashMap<&'g str, Vec<(&'g str, bool)>>) -> Levels<'g> {
    let mut levels: Levels<'g> = HashMap::new();
    let mut queue: VecDeque<(&str, bool, u32)> = VecDeque::new();
    levels.insert(root, [Some(0), None]);
    queue.push_back((root, false, 0));
    while let Some((n, crossed, d)) = queue.pop_front() {
        for &(m, is_event) in adj.get(n).map(Vec::as_slice).unwrap_or(&[]) {
            let c = crossed || is_event;
            let slot = &mut levels.entry(m).or_default()[usize::from(c)];
            if slot.is_none() {
                *slot = Some(d + 1);
                queue.push_back((m, c, d + 1));
            }
        }
    }
    levels
}

/// The coverage block for `changed` over `g` (spec §Search, §Output).
#[must_use]
pub fn compute_test_coverage(g: &CoverageGraph, changed: &[ChangedRoutine]) -> TestCoverage {
    // Sorted, deduplicated neighbour lists, whatever order `g` holds them in.
    let adj: HashMap<&str, Vec<(&str, bool)>> = g
        .edges
        .iter()
        .map(|(from, tos)| {
            let mut v: Vec<(&str, bool)> = tos.iter().map(|(t, e)| (t.as_str(), *e)).collect();
            v.sort_unstable();
            v.dedup();
            (from.as_str(), v)
        })
        .collect();

    // routine → (witness, viaEvent); roots ascend, so the first insert wins. Each
    // root's level map is iterated in hash order, which reaches only this
    // first-wins map (one root per pass) and the reason sets.
    let mut witness: HashMap<&str, (&str, bool)> = HashMap::new();
    let mut reasons: BTreeSet<&'static str> = BTreeSet::new();
    let (mut unresolved_tests, mut dependency_tests) = (0, 0);
    for root in &g.roots {
        let levels = reach_from(root, &adj);
        let (mut unresolved, mut dependency) = (false, false);
        for (&n, &[plain, crossed]) in &levels {
            let shortest = plain.into_iter().chain(crossed).min();
            let via_event = plain.is_none_or(|p| Some(p) > shortest);
            witness.entry(n).or_insert((root.as_str(), via_event));
            if let Some(r) = g.incomplete.get(n) {
                unresolved |= !r.is_empty();
                reasons.extend(r);
            }
            if let Some(r) = g.boundary.get(n) {
                dependency |= !r.is_empty();
                reasons.extend(r);
            }
        }
        unresolved_tests += usize::from(unresolved);
        dependency_tests += usize::from(dependency);
    }

    let mut distinct: BTreeMap<&str, &ChangedRoutine> = BTreeMap::new();
    for c in changed {
        distinct.entry(c.stable_id.as_str()).or_insert(c);
    }
    let is_test_code = |id: &str| g.roots.contains(id) || g.test_code.contains(id);
    let excluded_test_code = distinct.keys().filter(|id| is_test_code(id)).count();

    let mut covered = Vec::new();
    let mut uncovered = Vec::new();
    for (&id, c) in distinct.iter().filter(|(id, _)| !is_test_code(id)) {
        let hit = witness.get(id).filter(|_| c.shape == ChangedShape::Routine);
        if let Some(&(w, via_event)) = hit {
            covered.push(CoveredItem {
                stable_id: id.to_string(),
                display: c.display.clone(),
                reached_from: vec![w.to_string()],
                via_event,
            });
            continue;
        }
        // A blind spot only when no primary-published subscription could have
        // reached it.
        let facts = g.subscriber_facts.get(id).filter(|f| !f.primary_publisher);
        let reason = match c.shape {
            ChangedShape::DeclarationOnly => UncoveredReason::DeclarationOnly,
            ChangedShape::NotInGraph => UncoveredReason::NotInGraph,
            ChangedShape::Routine if facts.is_some_and(|f| f.platform) => {
                UncoveredReason::PlatformEventSubscriber
            }
            ChangedShape::Routine
                if facts.is_some_and(|f| {
                    f.outside_publisher
                        || (!f.dependency_publishers.is_empty()
                            && !f
                                .dependency_publishers
                                .iter()
                                .any(|p| witness.contains_key(p.as_str())))
                }) =>
            {
                UncoveredReason::DependencyEventSubscriber
            }
            ChangedShape::Routine => UncoveredReason::NoPath,
        };
        uncovered.push(UncoveredItem {
            stable_id: id.to_string(),
            display: c.display.clone(),
            object_display: c.object_display.clone(),
            reason,
            anchor: c.anchor.as_ref().map(|(file, line)| Anchor {
                file: file.clone(),
                line: *line,
            }),
        });
    }

    let changed_routines = covered.len() + uncovered.len();
    TestCoverage {
        changed_routines,
        covered_changed_routines: covered.len(),
        ratio: (changed_routines > 0).then(|| covered.len() as f64 / changed_routines as f64),
        covered,
        uncovered,
        test_procedures_considered: g.roots.len(),
        excluded_test_code,
        coverage: CoverageStatus {
            status: if reasons.is_empty() {
                "complete"
            } else {
                "partial"
            },
            reasons: reasons.into_iter().collect(),
            tests_with_unresolved_traversal: unresolved_tests,
            tests_calling_dependency_code: dependency_tests,
        },
    }
}

/// A callsite-ledger status's bucket (spec §Incomplete traversal); `None` when
/// the row is followed (its typed edges are the graph's edges).
fn ledger_bucket(status: &str) -> Option<(bool, &'static str)> {
    // (is_boundary, reason)
    Some(match status {
        "resolved" | "builtin" | "polymorphic" => return None,
        "ambiguous" => (false, "ambiguous"),
        "dynamic-target" => (false, "dynamic-target"),
        "unresolved-receiver-type" => (false, "unresolved-receiver-type"),
        "unresolved-member" => (false, "unresolved-member"),
        "external" => (true, "external"),
        "unfetched-dependency" => (true, "unfetched-dependency"),
        // A status this table does not know is not proof of a full traversal.
        _ => (false, "unknown-status"),
    })
}

/// The routine drives a `TestPage` / `TestRequestPage`, whose effects have no
/// edge: one of its own parameters or locals is one, or an object global is one
/// that its body names (a global alone would mark every sibling routine).
/// "Names" is `identifier_references` (plain identifiers only) OR the head of a
/// member call's receiver (`Card.OpenView()`, `"Card".OpenView()`,
/// `Action.OK().Invoke()`), which also covers quoted and keyword-lexed names.
/// A global used only outside a call (passed as an argument, say) is missed
/// here; a callee whose parameter is a `TestPage` is marked itself.
fn drives_test_page(r: &ModelRoutine) -> bool {
    let receivers: BTreeSet<String> = r
        .call_sites
        .iter()
        .filter_map(|cs| match &cs.callee {
            PCallee::Member { receiver, .. } => Some(receiver_head(receiver)),
            _ => None,
        })
        .collect();
    r.variables.iter().any(|v| {
        test_page_target(&v.declared_type).is_some()
            && (v.scope.as_deref() != Some("global")
                || r.identifier_references.binary_search(&v.name).is_ok()
                // The model stores a variable's name still escaped (`card ""a""`).
                || receivers.contains(&unescape_al_identifier(&v.name).fold_identifier()))
    })
}

/// The variable a receiver text starts with, folded: `"Tp Card".OK()` → `tp card`.
/// A quoted head ends at the first LONE `"` (`""` inside it is an escaped `"`)
/// and is decoded to its logical name: `"Card ""A""".OpenView()` → `card "a"`.
fn receiver_head(receiver: &str) -> String {
    let t = receiver.trim();
    let head = match t.strip_prefix('"') {
        Some(rest) => {
            let b = rest.as_bytes();
            let mut end = 0;
            while end < b.len() {
                if b[end] == b'"' {
                    if b.get(end + 1) != Some(&b'"') {
                        break;
                    }
                    end += 1;
                }
                end += 1;
            }
            unescape_al_identifier(&rest[..end])
        }
        None => t.split(['.', '(', '[']).next().unwrap_or("").to_string(),
    };
    head.trim().fold_identifier()
}

/// A routine name folded the way [`handler_names`] folds a handler name.
fn folded(name: &str) -> String {
    unquote_identifier(name).fold_identifier()
}

impl CoverageGraph {
    /// The one adapter from the digest's snapshot + model (spec §Test roots,
    /// §Changed routines, §Followed edges, §Incomplete traversal, §Why
    /// `uncovered`).
    #[must_use]
    pub fn from_snapshot(snap: &CapabilitySnapshot, resolved: &Model) -> Self {
        let mut g = CoverageGraph::default();
        let primary = primary_guid_of(resolved);
        let ws = &resolved.workspace;
        let objects: HashMap<&str, &crate::program::model::workspace::ModelObject> =
            ws.objects.iter().map(|o| (o.id.as_str(), o)).collect();
        fn flag(map: &mut BTreeMap<String, BTreeSet<&'static str>>, id: &str, why: &'static str) {
            map.entry(id.to_string()).or_default().insert(why);
        }

        let recorded: BTreeSet<&str> = snap.coverage.iter().map(|r| r.subject.as_str()).collect();
        let routines = || {
            ws.routines
                .iter()
                .filter(|r| !r.stable_routine_id.is_empty())
        };
        // (owning object, folded name) → ids: the `[HandlerFunctions]` lookup.
        let mut by_name: HashMap<(&str, String), Vec<&str>> = HashMap::new();
        for r in routines() {
            by_name
                .entry((r.object_id.as_str(), folded(&r.name)))
                .or_default()
                .push(&r.stable_routine_id);
        }
        for r in routines() {
            let id = r.stable_routine_id.as_str();
            let dependency = resolved.routine_role(r) == Role::Dependency;
            // The routine's OWN state: a record's reasons are inherited from the
            // whole cone, so they are not read here.
            if r.parse_incomplete {
                flag(&mut g.incomplete, id, "parse-incomplete");
            }
            if drives_test_page(r) {
                flag(&mut g.incomplete, id, "test-page");
            }
            if dependency {
                // Dependency code is a boundary (P6). Since #160 step 6 digest's
                // tree is the cone graph, so its dependency-internal calls are
                // edges too and the traversal follows them.
                flag(&mut g.boundary, id, "dependency-code");
                if !r.body_available {
                    flag(&mut g.boundary, id, "opaque-dependency");
                }
                continue;
            }
            if !r.body_available {
                flag(&mut g.incomplete, id, "body-unavailable");
            }
            if !recorded.contains(id) {
                flag(&mut g.incomplete, id, "no-coverage-record");
            }
            if find_attribute(&r.attributes_parsed, "Test").is_some() {
                g.roots.insert(id.to_string());
                g.test_code.insert(id.to_string());
                // The platform runs each named handler during the test.
                for name in r
                    .attributes_parsed
                    .iter()
                    .filter(|a| a.name.eq_ignore_ascii_case("HandlerFunctions"))
                    .filter_map(|a| string_arg(a, 0))
                    .flat_map(|list| handler_names(&list))
                {
                    for h in by_name
                        .get(&(r.object_id.as_str(), name))
                        .into_iter()
                        .flatten()
                    {
                        g.edges
                            .entry(id.to_string())
                            .or_default()
                            .push((h.to_string(), false));
                    }
                }
            }
            let test_codeunit = objects.get(r.object_id.as_str()).is_some_and(|o| {
                o.object_type.eq_ignore_ascii_case("codeunit")
                    && o.object_subtype
                        .as_deref()
                        .is_some_and(|s| s.eq_ignore_ascii_case("test"))
            });
            if test_codeunit {
                g.test_code.insert(id.to_string());
            }
        }

        for e in &snap.typed_edges {
            let from = e.edge_from();
            match e.edge_to() {
                Some(to) => {
                    g.edges
                        .entry(from.to_string())
                        .or_default()
                        .push((to.to_string(), e.edge_kind() == "event-dispatch"));
                    if role_of_stable_id(primary, to) == Role::Dependency {
                        flag(&mut g.boundary, to, "dependency-code");
                    }
                }
                None => flag(&mut g.incomplete, from, "object-run-unresolved"),
            }
        }

        for row in &snap.callsite_resolutions {
            if row.status == "polymorphic" {
                let open = row.candidates.as_ref().is_none_or(Vec::is_empty)
                    || row
                        .unresolved_candidates
                        .as_ref()
                        .is_some_and(|u| !u.is_empty())
                    || row
                        .candidates
                        .iter()
                        .flatten()
                        .any(|c| role_of_stable_id(primary, c) == Role::Dependency);
                if open {
                    flag(&mut g.incomplete, &row.from, "interface-open-world");
                }
            } else if let Some((is_boundary, reason)) = ledger_bucket(&row.status) {
                let map = if is_boundary {
                    &mut g.boundary
                } else {
                    &mut g.incomplete
                };
                flag(map, &row.from, reason);
            }
        }

        let events = &resolved.events.graph;
        let symbols: HashMap<&str, &crate::program::model::events::EventSymbol> =
            events.events.iter().map(|s| (s.id.as_str(), s)).collect();
        for edge in &events.edges {
            let Some(sym) = symbols.get(edge.event_id.as_str()) else {
                continue;
            };
            let facts = g
                .subscriber_facts
                .entry(edge.subscriber_stable_routine_id.clone())
                .or_default();
            match (&sym.publisher_ref, &sym.publisher_stable_routine_id) {
                (Some(PublisherRef::Platform { .. }), _) => facts.platform = true,
                // Set only when the publisher's app has no routine in the model
                // (`program_event_graph`'s `in_model` gate), so it is no node.
                (Some(PublisherRef::Dependency { .. }), _) => facts.outside_publisher = true,
                // A model publisher: the snapshot's event-dispatch edge joins it
                // to the subscriber.
                (None, Some(p)) if role_of_stable_id(primary, p) == Role::Dependency => {
                    facts.dependency_publishers.insert(p.clone());
                }
                (None, Some(_)) => facts.primary_publisher = true,
                (None, None) => {}
            }
        }
        g
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A hand-built graph: `roots`, and `(from, to, is_event)` edges.
    fn graph(roots: &[&str], edges: &[(&str, &str, bool)]) -> CoverageGraph {
        let mut g = CoverageGraph {
            roots: roots.iter().map(|r| r.to_string()).collect(),
            ..Default::default()
        };
        g.test_code = g.roots.clone();
        for (f, t, e) in edges {
            g.edges
                .entry(f.to_string())
                .or_default()
                .push((t.to_string(), *e));
        }
        g
    }

    fn ch(id: &str) -> ChangedRoutine {
        ChangedRoutine {
            stable_id: id.to_string(),
            display: format!("D{id}"),
            object_display: "Codeunit X".to_string(),
            anchor: None,
            shape: ChangedShape::Routine,
        }
    }

    fn changed(ids: &[&str]) -> Vec<ChangedRoutine> {
        ids.iter().map(|i| ch(i)).collect()
    }

    /// `(id, witness, viaEvent)` of every covered item.
    fn cov(tc: &TestCoverage) -> Vec<(&str, &str, bool)> {
        tc.covered
            .iter()
            .map(|c| {
                assert_eq!(c.reached_from.len(), 1, "{c:?}");
                (
                    c.stable_id.as_str(),
                    c.reached_from[0].as_str(),
                    c.via_event,
                )
            })
            .collect()
    }

    /// `(id, reason)` of every uncovered item.
    fn unc(tc: &TestCoverage) -> Vec<(&str, UncoveredReason)> {
        tc.uncovered
            .iter()
            .map(|u| (u.stable_id.as_str(), u.reason))
            .collect()
    }

    #[test]
    fn direct_call_is_covered() {
        let g = graph(&["t"], &[("t", "a", false)]);
        let tc = compute_test_coverage(&g, &changed(&["a"]));
        assert_eq!(cov(&tc), vec![("a", "t", false)]);
        assert!(tc.uncovered.is_empty());
        assert_eq!(tc.ratio, Some(1.0));
    }

    #[test]
    fn two_hop_is_covered() {
        let g = graph(&["t"], &[("t", "a", false), ("a", "b", false)]);
        let tc = compute_test_coverage(&g, &changed(&["b"]));
        assert_eq!(cov(&tc), vec![("b", "t", false)]);
    }

    #[test]
    fn subscriber_via_publisher_is_covered() {
        let g = graph(
            &["t"],
            &[
                ("t", "raiser", false),
                ("raiser", "pub", false),
                ("pub", "sub", true),
            ],
        );
        let tc = compute_test_coverage(&g, &changed(&["raiser", "sub"]));
        assert_eq!(cov(&tc), vec![("raiser", "t", false), ("sub", "t", true)]);
    }

    #[test]
    fn equal_length_event_free_path_wins() {
        // t -> a -(event)-> x and t -> b -> x: both two hops.
        let g = graph(
            &["t"],
            &[
                ("t", "a", false),
                ("a", "x", true),
                ("t", "b", false),
                ("b", "x", false),
            ],
        );
        let tc = compute_test_coverage(&g, &changed(&["x"]));
        assert_eq!(cov(&tc), vec![("x", "t", false)]);
    }

    #[test]
    fn shorter_event_path_sets_via_event() {
        // t -(event)-> x is one hop; t -> b -> x is two.
        let g = graph(
            &["t"],
            &[("t", "x", true), ("t", "b", false), ("b", "x", false)],
        );
        let tc = compute_test_coverage(&g, &changed(&["x"]));
        assert_eq!(cov(&tc), vec![("x", "t", true)]);
    }

    #[test]
    fn lowest_id_witness_wins_over_shorter_path() {
        let g = graph(
            &["t:a", "t:b"],
            &[
                ("t:a", "p", false),
                ("p", "q", false),
                ("q", "x", false),
                ("t:b", "x", false),
            ],
        );
        let tc = compute_test_coverage(&g, &changed(&["x"]));
        assert_eq!(cov(&tc), vec![("x", "t:a", false)]);
    }

    #[test]
    fn cycle_terminates() {
        let g = graph(
            &["t"],
            &[("t", "a", false), ("a", "t", false), ("a", "a", false)],
        );
        let tc = compute_test_coverage(&g, &changed(&["a", "a"]));
        assert_eq!(cov(&tc), vec![("a", "t", false)]);
        assert_eq!(tc.changed_routines, 1);
    }

    #[test]
    fn shuffled_adjacency_same_output() {
        let edges = [
            ("t1", "a", false),
            ("t1", "b", true),
            ("a", "x", true),
            ("b", "x", false),
            ("t2", "x", false),
            ("t2", "a", false),
            ("a", "y", false),
        ];
        let mut rev = edges;
        rev.reverse();
        let ids = ["x", "y", "a", "b", "z"];
        let one = compute_test_coverage(&graph(&["t1", "t2"], &edges), &changed(&ids));
        let two = compute_test_coverage(&graph(&["t2", "t1"], &rev), &changed(&ids));
        assert_eq!(one.to_json_value(), two.to_json_value());
    }

    #[test]
    fn unresolved_row_flips_status() {
        let mut g = graph(&["t"], &[("t", "a", false)]);
        let tc = compute_test_coverage(&g, &changed(&["a"]));
        assert_eq!(tc.coverage.status, "complete");
        assert!(tc.coverage.reasons.is_empty());
        assert_eq!(tc.coverage.tests_with_unresolved_traversal, 0);

        g.incomplete
            .entry("a".to_string())
            .or_default()
            .insert("dynamic-target");
        let tc = compute_test_coverage(&g, &changed(&["a"]));
        assert_eq!(tc.coverage.status, "partial");
        assert_eq!(tc.coverage.reasons, vec!["dynamic-target"]);
        assert_eq!(tc.coverage.tests_with_unresolved_traversal, 1);
        assert_eq!(tc.coverage.tests_calling_dependency_code, 0);
    }

    #[test]
    fn incomplete_on_unreached_routine_does_not_flip() {
        let mut g = graph(&["t"], &[("t", "a", false)]);
        g.incomplete
            .entry("z".to_string())
            .or_default()
            .insert("dynamic-target");
        let tc = compute_test_coverage(&g, &changed(&["a", "z"]));
        assert_eq!(tc.coverage.status, "complete");
        assert_eq!(tc.coverage.tests_with_unresolved_traversal, 0);
    }

    #[test]
    fn dependency_boundary_counted_apart() {
        let mut g = graph(&["t"], &[("t", "x", false)]);
        g.boundary
            .entry("x".to_string())
            .or_default()
            .insert("dependency-code");
        let tc = compute_test_coverage(&g, &changed(&["x"]));
        assert_eq!(tc.coverage.status, "partial");
        assert_eq!(tc.coverage.reasons, vec!["dependency-code"]);
        assert_eq!(tc.coverage.tests_calling_dependency_code, 1);
        assert_eq!(tc.coverage.tests_with_unresolved_traversal, 0);
    }

    #[test]
    fn test_code_excluded_from_denominator() {
        let mut g = graph(&["t"], &[("t", "helper", false), ("helper", "a", false)]);
        g.test_code.insert("helper".to_string());
        let tc = compute_test_coverage(&g, &changed(&["t", "helper", "a"]));
        assert_eq!(tc.changed_routines, 1);
        assert_eq!(tc.excluded_test_code, 2);
        assert_eq!(cov(&tc), vec![("a", "t", false)]);
    }

    #[test]
    fn test_attribute_outside_test_codeunit_is_root() {
        // `t` is a root but not in `test_code` (a `[Test]` in a normal codeunit).
        let mut g = graph(&["t"], &[("t", "a", false)]);
        g.test_code.clear();
        let tc = compute_test_coverage(&g, &changed(&["t", "a"]));
        assert_eq!(tc.excluded_test_code, 1);
        assert_eq!(tc.changed_routines, 1);
        assert_eq!(cov(&tc), vec![("a", "t", false)]);
        assert_eq!(tc.test_procedures_considered, 1);
    }

    #[test]
    fn declaration_only_stays_uncovered() {
        let g = graph(&["t"], &[("t", "a", false)]);
        let mut decl = ch("i");
        decl.shape = ChangedShape::DeclarationOnly;
        decl.anchor = Some(("src/I.al".to_string(), 3));
        let tc = compute_test_coverage(&g, &[decl, ch("a")]);
        assert_eq!(tc.changed_routines, 2);
        assert_eq!(unc(&tc), vec![("i", UncoveredReason::DeclarationOnly)]);
        assert_eq!(
            tc.uncovered[0].anchor,
            Some(Anchor {
                file: "src/I.al".to_string(),
                line: 3
            })
        );
        let json = tc.to_json_value();
        assert_eq!(json["uncovered"][0]["reason"], "declaration-only");
        assert_eq!(json["uncovered"][0]["anchor"]["line"], 3);
    }

    #[test]
    fn platform_and_dependency_subscriber_reasons() {
        let mut g = graph(&["t"], &[("t", "a", false), ("dep:pub", "sub2", true)]);
        g.subscriber_facts.insert(
            "sub1".to_string(),
            SubscriberFacts {
                platform: true,
                ..Default::default()
            },
        );
        // The cross-app shape: the dependency publisher is a model routine, joined
        // to the subscriber by the snapshot's event-dispatch edge.
        let dep = SubscriberFacts {
            dependency_publishers: ["dep:pub".to_string()].into(),
            ..Default::default()
        };
        g.subscriber_facts.insert("sub2".to_string(), dep);
        // The single-app shape: the publisher is no node at all.
        let outside = SubscriberFacts {
            outside_publisher: true,
            ..Default::default()
        };
        g.subscriber_facts.insert("sub3".to_string(), outside);
        let ids = changed(&["sub1", "sub2", "sub3"]);
        let tc = compute_test_coverage(&g, &ids);
        assert_eq!(
            unc(&tc),
            vec![
                ("sub1", UncoveredReason::PlatformEventSubscriber),
                ("sub2", UncoveredReason::DependencyEventSubscriber),
                ("sub3", UncoveredReason::DependencyEventSubscriber),
            ]
        );
        let json = tc.to_json_value();
        assert_eq!(json["uncovered"][0]["reason"], "platform-event-subscriber");
        assert_eq!(
            json["uncovered"][1]["reason"],
            "dependency-event-subscriber"
        );
        assert!(json["uncovered"][0].get("anchor").is_none());

        // A test reaches the dependency publisher: the subscriber is covered.
        g.edges
            .entry("a".to_string())
            .or_default()
            .push(("dep:pub".to_string(), false));
        let tc = compute_test_coverage(&g, &ids);
        assert_eq!(cov(&tc), vec![("sub2", "t", true)]);
        assert_eq!(
            unc(&tc),
            vec![
                ("sub1", UncoveredReason::PlatformEventSubscriber),
                ("sub3", UncoveredReason::DependencyEventSubscriber),
            ]
        );
    }

    /// R14: a subscriber that also subscribes to a primary-published event is an
    /// ordinary `no-path` when unreached, not a blind spot.
    #[test]
    fn primary_subscription_is_no_path() {
        let g = {
            let mut g = graph(&["t"], &[]);
            for (id, platform) in [("sub1", true), ("sub2", false)] {
                g.subscriber_facts.insert(
                    id.to_string(),
                    SubscriberFacts {
                        platform,
                        outside_publisher: !platform,
                        primary_publisher: true,
                        ..Default::default()
                    },
                );
            }
            g
        };
        let tc = compute_test_coverage(&g, &changed(&["sub1", "sub2"]));
        assert_eq!(
            unc(&tc),
            vec![
                ("sub1", UncoveredReason::NoPath),
                ("sub2", UncoveredReason::NoPath)
            ]
        );
    }

    /// R4: a changed root the digest query dropped still counts, as
    /// `not-in-graph`, even when a test would reach its id.
    #[test]
    fn not_in_graph_root_counts_uncovered() {
        let g = graph(&["t"], &[("t", "gone", false)]);
        let mut gone = ch("gone");
        gone.shape = ChangedShape::NotInGraph;
        let tc = compute_test_coverage(&g, &[gone]);
        assert_eq!(tc.changed_routines, 1);
        assert_eq!(unc(&tc), vec![("gone", UncoveredReason::NotInGraph)]);
        assert_eq!(tc.to_json_value()["uncovered"][0]["reason"], "not-in-graph");
    }

    #[test]
    fn ratio_null_when_no_production_changes() {
        let mut g = graph(&["t"], &[("t", "helper", false)]);
        g.test_code.insert("helper".to_string());
        let tc = compute_test_coverage(&g, &changed(&["t", "helper"]));
        assert_eq!(tc.changed_routines, 0);
        assert_eq!(tc.ratio, None);
        assert!(tc.covered.is_empty() && tc.uncovered.is_empty());
        let json = tc.to_json_value();
        assert!(json.as_object().unwrap().contains_key("ratio"), "{json}");
        assert!(json["ratio"].is_null(), "{json}");
    }

    #[test]
    fn partition_is_exact_disjoint_sorted() {
        let g = graph(
            &["t1", "t2"],
            &[("t1", "c", false), ("t2", "a", false), ("a", "e", true)],
        );
        let ids = ["e", "d", "c", "b", "a", "t1", "c"];
        let tc = compute_test_coverage(&g, &changed(&ids));
        let c: Vec<&str> = tc.covered.iter().map(|i| i.stable_id.as_str()).collect();
        let u: Vec<&str> = tc.uncovered.iter().map(|i| i.stable_id.as_str()).collect();
        assert_eq!(c, vec!["a", "c", "e"]);
        assert_eq!(u, vec!["b", "d"]);
        assert_eq!(tc.changed_routines, 5);
        assert_eq!(tc.covered_changed_routines, 3);
        assert_eq!(tc.ratio, Some(0.6));
    }

    #[test]
    fn duplicate_changed_ids_counted_once() {
        let g = graph(&["t"], &[("t", "a", false)]);
        let tc = compute_test_coverage(&g, &changed(&["a", "a", "b", "b"]));
        assert_eq!(tc.changed_routines, 2);
        assert_eq!(cov(&tc), vec![("a", "t", false)]);
        assert_eq!(unc(&tc), vec![("b", UncoveredReason::NoPath)]);
    }

    #[test]
    fn no_tests_all_uncovered() {
        let g = graph(&[], &[("a", "b", false)]);
        let tc = compute_test_coverage(&g, &changed(&["a", "b"]));
        assert_eq!(tc.test_procedures_considered, 0);
        assert_eq!(
            unc(&tc),
            vec![
                ("a", UncoveredReason::NoPath),
                ("b", UncoveredReason::NoPath)
            ]
        );
        assert_eq!(tc.ratio, Some(0.0));
        assert_eq!(tc.coverage.status, "complete");
    }

    /// The adapter over `tests/fixtures/test-selection-graph/` (the CLI tests'
    /// fixture): the main selection's exact sets and the traversal counters. Ids
    /// are hashes, so the expectation names routines.
    #[test]
    fn adapter_on_test_selection_graph() {
        let ws = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/test-selection-graph");
        let m = crate::program::model::program_calls::assemble_and_resolve_workspace_with_program_calls(&ws)
            .expect("model");
        let snap = crate::engine::l5::snapshot::compose_snapshot(&m);
        let g = CoverageGraph::from_snapshot(&snap, &m);
        assert_eq!(g.roots.len(), 11, "{:?}", g.roots);

        let by_name = |name: &str| {
            let rows: BTreeSet<&str> = m
                .workspace
                .routines
                .iter()
                .filter(|r| r.name == name)
                .map(|r| r.stable_routine_id.as_str())
                .collect();
            assert_eq!(rows.len(), 1, "`{name}`: {rows:?}");
            rows.into_iter().next().unwrap().to_string()
        };
        let name_of: HashMap<String, &str> = m
            .workspace
            .routines
            .iter()
            .map(|r| (r.stable_routine_id.clone(), r.name.as_str()))
            .collect();
        let main = [
            "TaxAmount",
            "PointsFor",
            "ApplyDecay",
            "CashRoundOnInvoiceTotal",
            "RoundToNickel",
            "AddBonusOnCalcPoints",
            "Ping",
            "ExportLine",
            "ReachedOnlyFromNowhere",
            "OnAfterInsertLedger",
            // Called only by `CardHandler`: reached through `[HandlerFunctions]`.
            "InModalHandler",
        ];
        let ids: Vec<ChangedRoutine> = main.iter().map(|n| ch(&by_name(n))).collect();
        let tc = compute_test_coverage(&g, &ids);

        let mut covered: Vec<(&str, &str, bool)> = tc
            .covered
            .iter()
            .map(|c| {
                (
                    name_of[&c.stable_id],
                    name_of[&c.reached_from[0]],
                    c.via_event,
                )
            })
            .collect();
        covered.sort();
        let lowest = |a: &str, b: &str| {
            let (a, b) = (by_name(a), by_name(b));
            name_of[&a.min(b)]
        };
        let mut want = vec![
            (
                "TaxAmount",
                lowest("GermanTax", "InvoiceTotalRounds"),
                false,
            ),
            (
                "PointsFor",
                lowest("BonusAboveThousand", "NightlyRecalcDecays"),
                false,
            ),
            ("ApplyDecay", "NightlyRecalcDecays", false),
            ("CashRoundOnInvoiceTotal", "InvoiceTotalRounds", true),
            ("RoundToNickel", "InvoiceTotalRounds", true),
            (
                "AddBonusOnCalcPoints",
                lowest("BonusAboveThousand", "NightlyRecalcDecays"),
                true,
            ),
            ("InModalHandler", "OpenCardModally", false),
        ];
        want.sort();
        assert_eq!(covered, want);

        let mut uncovered: Vec<(&str, UncoveredReason)> = tc
            .uncovered
            .iter()
            .map(|u| (name_of[&u.stable_id], u.reason))
            .collect();
        uncovered.sort_by_key(|u| u.0);
        assert_eq!(
            uncovered,
            vec![
                ("ExportLine", UncoveredReason::NoPath),
                (
                    "OnAfterInsertLedger",
                    UncoveredReason::PlatformEventSubscriber
                ),
                ("Ping", UncoveredReason::NoPath),
                ("ReachedOnlyFromNowhere", UncoveredReason::NoPath),
            ]
        );

        // `TaxRate.RatePct()` (Tax.al:31) is polymorphic over a closed set of three
        // workspace candidates: followed, so no `interface-open-world`.
        let c = &tc.coverage;
        assert_eq!(c.status, "partial");
        assert_eq!(
            c.reasons,
            vec![
                "dynamic-target",
                "object-run-unresolved",
                "test-page",
                "unfetched-dependency",
                "unresolved-receiver-type",
            ]
        );
        // CardShowsRecord, OpenCardModally (test-page), DynamicRun, VariantPing.
        assert_eq!(c.tests_with_unresolved_traversal, 4, "{c:?}");
        // OpenCardModally's `Page.RunModal` row.
        assert_eq!(c.tests_calling_dependency_code, 1, "{c:?}");
    }

    /// Matrix row 4, the precondition the CLI test `configured_test_kind_is_not_a_root`
    /// cannot see: the fixture's `roots.config.json` really classifies
    /// `Configured` as `test-procedure`, and the adapter still does not root it.
    #[test]
    fn configured_test_kind_is_not_a_root() {
        let ws = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/digest-test-coverage-configured-root");
        let m = crate::program::model::program_calls::assemble_and_resolve_workspace_with_program_calls(&ws)
            .expect("model");
        let r = m
            .workspace
            .routines
            .iter()
            .find(|r| r.name == "Configured")
            .expect("Configured");
        let configured = m
            .root_classifications
            .iter()
            .any(|c| c.routine_id == r.id && c.kinds.iter().any(|k| k == "test-procedure"));
        assert!(configured, "precondition: {:?}", m.root_classifications);
        let g =
            CoverageGraph::from_snapshot(&crate::engine::l5::snapshot::compose_snapshot(&m), &m);
        assert_eq!(g.roots.len(), 1, "{:?}", g.roots);
        assert!(!g.roots.contains(&r.stable_routine_id));
        assert!(!g.test_code.contains(&r.stable_routine_id));
    }

    // R12: each adapter branch through `from_snapshot`, on the
    // test-selection-graph model and snapshot with the branch's precondition
    // stated by hand (a field set, a record removed, a ledger row added).

    fn selection_graph() -> (Model, CapabilitySnapshot) {
        let ws = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/test-selection-graph");
        let m = crate::program::model::program_calls::assemble_and_resolve_workspace_with_program_calls(&ws)
            .expect("model");
        let snap = crate::engine::l5::snapshot::compose_snapshot(&m);
        (m, snap)
    }

    /// The one model row named `name`.
    fn row<'m>(m: &'m mut Model, name: &str) -> &'m mut ModelRoutine {
        let mut rows = m.workspace.routines.iter_mut().filter(|r| r.name == name);
        let r = rows.next().unwrap_or_else(|| panic!("no `{name}`"));
        assert!(rows.next().is_none(), "`{name}` is not unique");
        r
    }

    /// Makes `name` a dependency routine: its app is no longer the primary one.
    fn make_dependency(m: &mut Model, name: &str) -> String {
        let r = row(m, name);
        r.app_guid = "ffffffff-0000-0000-0000-00000000dead".to_string();
        r.stable_routine_id.clone()
    }

    fn reasons(map: &BTreeMap<String, BTreeSet<&'static str>>, id: &str) -> Vec<&'static str> {
        map.get(id)
            .map(|s| s.iter().copied().collect())
            .unwrap_or_default()
    }

    #[test]
    fn adapter_parse_incomplete_on_primary_and_dependency() {
        let (mut m, snap) = selection_graph();
        let ping = {
            let r = row(&mut m, "Ping");
            r.parse_incomplete = true;
            r.stable_routine_id.clone()
        };
        let export = make_dependency(&mut m, "ExportLine");
        row(&mut m, "ExportLine").parse_incomplete = true;
        let g = CoverageGraph::from_snapshot(&snap, &m);
        assert_eq!(reasons(&g.incomplete, &ping), vec!["parse-incomplete"]);
        assert_eq!(reasons(&g.incomplete, &export), vec!["parse-incomplete"]);
        assert_eq!(reasons(&g.boundary, &export), vec!["dependency-code"]);
    }

    #[test]
    fn adapter_body_unavailable_primary_is_incomplete() {
        let (mut m, snap) = selection_graph();
        let ping = {
            let r = row(&mut m, "Ping");
            r.body_available = false;
            r.stable_routine_id.clone()
        };
        let g = CoverageGraph::from_snapshot(&snap, &m);
        assert_eq!(reasons(&g.incomplete, &ping), vec!["body-unavailable"]);
        assert_eq!(reasons(&g.boundary, &ping), Vec::<&str>::new());
    }

    #[test]
    fn adapter_opaque_dependency_is_boundary() {
        let (mut m, snap) = selection_graph();
        let export = make_dependency(&mut m, "ExportLine");
        row(&mut m, "ExportLine").body_available = false;
        let g = CoverageGraph::from_snapshot(&snap, &m);
        assert_eq!(
            reasons(&g.boundary, &export),
            vec!["dependency-code", "opaque-dependency"]
        );
        assert_eq!(reasons(&g.incomplete, &export), Vec::<&str>::new());
    }

    #[test]
    fn adapter_dependency_code_and_no_dependency_root() {
        let (mut m, snap) = selection_graph();
        let german = make_dependency(&mut m, "GermanTax");
        let g = CoverageGraph::from_snapshot(&snap, &m);
        assert_eq!(reasons(&g.boundary, &german), vec!["dependency-code"]);
        // A dependency `[Test]` is never a root.
        assert_eq!(g.roots.len(), 10, "{:?}", g.roots);
        assert!(!g.roots.contains(&german));
    }

    #[test]
    fn adapter_no_coverage_record_primary_only() {
        let (mut m, mut snap) = selection_graph();
        let ping = row(&mut m, "Ping").stable_routine_id.clone();
        let export = make_dependency(&mut m, "ExportLine");
        let before = snap.coverage.len();
        snap.coverage
            .retain(|c| c.subject != ping && c.subject != export);
        assert_eq!(
            snap.coverage.len(),
            before - 2,
            "precondition: both had records"
        );
        let g = CoverageGraph::from_snapshot(&snap, &m);
        assert_eq!(reasons(&g.incomplete, &ping), vec!["no-coverage-record"]);
        assert_eq!(reasons(&g.incomplete, &export), Vec::<&str>::new());
    }

    /// The fixture's one polymorphic row (`TaxRate.RatePct()`), re-homed onto
    /// `from` with `edit` applied.
    fn polymorphic_row(
        snap: &CapabilitySnapshot,
        from: &str,
        edit: impl FnOnce(&mut crate::engine::l5::snapshot::SnapshotCallsiteResolution),
    ) -> crate::engine::l5::snapshot::SnapshotCallsiteResolution {
        let mut row = snap
            .callsite_resolutions
            .iter()
            .find(|r| r.status == "polymorphic")
            .expect("the fixture's polymorphic row")
            .clone();
        assert!(row.candidates.as_ref().is_some_and(|c| c.len() == 3));
        assert!(row.unresolved_candidates.is_none());
        row.from = from.to_string();
        edit(&mut row);
        row
    }

    #[test]
    fn adapter_interface_open_world_shapes() {
        let (mut m, mut snap) = selection_graph();
        let [a, b, c, d] = ["Ping", "ExportLine", "ApplyDecay", "RoundToNickel"]
            .map(|n| row(&mut m, n).stable_routine_id.clone());
        let rows = [
            polymorphic_row(&snap, &a, |r| r.candidates = None),
            polymorphic_row(&snap, &b, |r| {
                r.unresolved_candidates = Some(vec![
                    crate::engine::l5::snapshot::SnapshotUnresolvedCandidate {
                        object_id: "x".to_string(),
                        reason: "unresolved".to_string(),
                    },
                ])
            }),
            polymorphic_row(&snap, &c, |r| {
                r.candidates
                    .as_mut()
                    .unwrap()
                    .push("ffffffff-0000-0000-0000-00000000dead:Codeunit:1#00".to_string())
            }),
            // The closed shape: followed, no reason.
            polymorphic_row(&snap, &d, |_| {}),
        ];
        snap.callsite_resolutions.extend(rows);
        let g = CoverageGraph::from_snapshot(&snap, &m);
        for id in [&a, &b, &c] {
            assert_eq!(
                reasons(&g.incomplete, id),
                vec!["interface-open-world"],
                "{id}"
            );
        }
        assert_eq!(reasons(&g.incomplete, &d), Vec::<&str>::new());
    }

    #[test]
    fn adapter_ambiguous_row_is_incomplete() {
        let (mut m, mut snap) = selection_graph();
        let ping = row(&mut m, "Ping").stable_routine_id.clone();
        let amb = polymorphic_row(&snap, &ping, |r| r.status = "ambiguous".to_string());
        snap.callsite_resolutions.push(amb);
        let g = CoverageGraph::from_snapshot(&snap, &m);
        assert_eq!(reasons(&g.incomplete, &ping), vec!["ambiguous"]);
    }

    /// R14's adapter half: a subscriber to a workspace publisher carries
    /// `primary_publisher`; one to a platform event carries `platform` only.
    #[test]
    fn adapter_subscriber_facts_by_publisher() {
        let (mut m, snap) = selection_graph();
        let cash = row(&mut m, "CashRoundOnInvoiceTotal")
            .stable_routine_id
            .clone();
        let ledger = row(&mut m, "OnAfterInsertLedger").stable_routine_id.clone();
        let g = CoverageGraph::from_snapshot(&snap, &m);
        let f = &g.subscriber_facts[&cash];
        assert!(
            f.primary_publisher && !f.platform && !f.outside_publisher,
            "{f:?}"
        );
        let f = &g.subscriber_facts[&ledger];
        assert!(f.platform && !f.primary_publisher, "{f:?}");
    }

    /// R15: a handler row with no stable id gives no edge to `""`.
    #[test]
    fn adapter_handler_edges_skip_empty_ids() {
        let (mut m, snap) = selection_graph();
        let open = row(&mut m, "OpenCardModally").stable_routine_id.clone();
        let handler = row(&mut m, "CardHandler").stable_routine_id.clone();
        let g = CoverageGraph::from_snapshot(&snap, &m);
        assert!(g.edges[&open].contains(&(handler, false)), "precondition");
        row(&mut m, "CardHandler").stable_routine_id = String::new();
        let g = CoverageGraph::from_snapshot(&snap, &m);
        assert!(
            g.edges[&open].iter().all(|(to, _)| !to.is_empty()),
            "{:?}",
            g.edges[&open]
        );
    }

    /// The model of a one-file temp workspace holding `al`.
    fn temp_model(al: &str) -> Model {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("app.json"),
            r#"{"id":"3c4d5e6f-7a8b-4c9d-8e0f-1a2b3c4d5e6f","name":"Tp","publisher":"al-sem tests","version":"1.0.0.0","runtime":"17.0","idRanges":[{"from":51020,"to":51029}]}"#,
        )
        .unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/Tp.al"), al).unwrap();
        crate::program::model::program_calls::assemble_and_resolve_workspace_with_program_calls(
            dir.path(),
        )
        .expect("model")
    }

    /// R2: a global `TestPage` marks only the routines whose body names it.
    /// FR20: a global whose name needs quotes is never in `identifier_references`,
    /// so it marks every routine that has it in scope.
    #[test]
    fn adapter_global_test_page_needs_a_reference() {
        let m = temp_model(
            r#"page 51020 "Tp Card"
{
    PageType = Card;
}

codeunit 51021 "Tp Tests"
{
    Subtype = Test;

    var
        Card: TestPage "Tp Card";

    [Test]
    procedure UsesCard()
    begin
        Card.OpenView();
    end;

    [Test]
    procedure IgnoresCard()
    begin
    end;
}

codeunit 51022 "Tp Quoted Tests"
{
    Subtype = Test;

    var
        "Tp Card Page": TestPage "Tp Card";

    [Test]
    procedure UsesQuoted()
    begin
        "Tp Card Page".OpenView();
    end;

    [Test]
    procedure IgnoresQuoted()
    begin
    end;
}

codeunit 51024 "Tp Spelling Tests"
{
    Subtype = Test;

    var
        Card: TestPage "Tp Card";
        Action: TestPage "Tp Card";

    [Test]
    procedure UsesCardQuoted()
    begin
        "Card".OpenView();
    end;

    [Test]
    procedure UsesKeywordName()
    begin
        Action.OK().Invoke();
    end;

    [Test]
    procedure IgnoresBoth()
    begin
    end;
}

codeunit 51025 "Tp Escaped Tests"
{
    Subtype = Test;

    var
        "Card ""A""": TestPage "Tp Card";

    [Test]
    procedure UsesEscaped()
    begin
        "Card ""A""".OpenView();
    end;

    [Test]
    procedure IgnoresEscaped()
    begin
    end;
}
"#,
        );
        let quoted = m
            .workspace
            .routines
            .iter()
            .find(|r| r.name == "UsesQuoted")
            .unwrap();
        assert!(
            !quoted
                .identifier_references
                .iter()
                .any(|i| i.contains("tp card page")),
            "precondition: the quoted name is not recorded: {:?}",
            quoted.identifier_references
        );
        let id = |name: &str| {
            let r = m
                .workspace
                .routines
                .iter()
                .find(|r| r.name == name)
                .unwrap();
            let sees_global = r
                .variables
                .iter()
                .any(|v| test_page_target(&v.declared_type).is_some());
            assert!(
                sees_global,
                "precondition: `{name}` has the global in scope"
            );
            r.stable_routine_id.clone()
        };
        let (uses, ignores, quoted) = (id("UsesCard"), id("IgnoresCard"), id("UsesQuoted"));
        let g =
            CoverageGraph::from_snapshot(&crate::engine::l5::snapshot::compose_snapshot(&m), &m);
        assert!(
            reasons(&g.incomplete, &uses).contains(&"test-page"),
            "{:?}",
            g.incomplete
        );
        assert!(
            !reasons(&g.incomplete, &ignores).contains(&"test-page"),
            "{:?}",
            g.incomplete
        );
        assert!(
            reasons(&g.incomplete, &quoted).contains(&"test-page"),
            "{:?}",
            g.incomplete
        );
        // FR20: a plain name used quoted, and a name that lexes as a keyword
        // identifier, are not in `identifier_references` either; the call
        // receivers find them. The siblings that never name a global stay clean.
        for name in ["UsesCardQuoted", "UsesKeywordName"] {
            let r = m
                .workspace
                .routines
                .iter()
                .find(|r| r.name == name)
                .unwrap();
            assert!(
                !r.identifier_references
                    .iter()
                    .any(|i| i == "card" || i == "action"),
                "precondition: `{name}` records no plain reference: {:?}",
                r.identifier_references
            );
            assert!(
                reasons(&g.incomplete, &id(name)).contains(&"test-page"),
                "{name}: {:?}",
                g.incomplete
            );
        }
        // FR20: an escaped quoted name. Precondition: the model stores the global's
        // name still ESCAPED, so both it and the receiver are decoded once
        // (`unescape_al_identifier`) before they are compared.
        let escaped = m
            .workspace
            .routines
            .iter()
            .find(|r| r.name == "UsesEscaped")
            .unwrap();
        let names: Vec<&str> = escaped
            .variables
            .iter()
            .filter(|v| test_page_target(&v.declared_type).is_some())
            .map(|v| v.name.as_str())
            .collect();
        assert_eq!(names, vec!["card \"\"a\"\""], "precondition: stored name");
        assert!(
            reasons(&g.incomplete, &id("UsesEscaped")).contains(&"test-page"),
            "UsesEscaped: {:?}",
            g.incomplete
        );
        assert_eq!(receiver_head("\"Card \"\"A\"\"\".OpenView()"), "card \"a\"");
        for name in ["IgnoresQuoted", "IgnoresBoth", "IgnoresEscaped"] {
            assert!(
                !reasons(&g.incomplete, &id(name)).contains(&"test-page"),
                "{name}: {:?}",
                g.incomplete
            );
        }
    }

    /// FR19: the handler list comes from the decoded first string argument, so an
    /// apostrophe-quoted name inside a comment in the attribute is not read.
    #[test]
    fn adapter_handler_names_ignore_comments() {
        let m = temp_model(
            r#"codeunit 51023 "Tp Handler Tests"
{
    Subtype = Test;

    [Test]
    [HandlerFunctions(/* 'WrongHandler' */ 'RightHandler')]
    procedure OpensModal()
    begin
    end;

    [ConfirmHandler]
    procedure WrongHandler(Question: Text[1024]; var Reply: Boolean)
    begin
    end;

    [ConfirmHandler]
    procedure RightHandler(Question: Text[1024]; var Reply: Boolean)
    begin
    end;
}
"#,
        );
        let id = |name: &str| {
            m.workspace
                .routines
                .iter()
                .find(|r| r.name == name)
                .unwrap()
                .stable_routine_id
                .clone()
        };
        let g =
            CoverageGraph::from_snapshot(&crate::engine::l5::snapshot::compose_snapshot(&m), &m);
        assert_eq!(
            g.edges.get(&id("OpensModal")),
            Some(&vec![(id("RightHandler"), false)]),
            "{:?}",
            g.edges
        );
    }

    /// FR12: an interface call with known candidates AND an unresolved
    /// remainder still follows the candidates (covered, lowest-id witness) and
    /// makes the block partial with `interface-open-world`.
    #[test]
    fn open_interface_still_covers_its_known_candidates() {
        let (m, mut snap) = selection_graph();
        let row = snap
            .callsite_resolutions
            .iter_mut()
            .find(|r| r.status == "polymorphic")
            .expect("the fixture's polymorphic row");
        assert_eq!(
            row.candidates.as_ref().map(Vec::len),
            Some(3),
            "precondition"
        );
        row.unresolved_candidates = Some(vec![
            crate::engine::l5::snapshot::SnapshotUnresolvedCandidate {
                object_id: "x".to_string(),
                reason: "unresolved".to_string(),
            },
        ]);
        let candidates = row.candidates.clone().unwrap();
        let g = CoverageGraph::from_snapshot(&snap, &m);
        let changed: Vec<ChangedRoutine> = candidates.iter().map(|c| ch(c)).collect();
        let tc = compute_test_coverage(&g, &changed);

        let id_of = |name: &str| {
            m.workspace
                .routines
                .iter()
                .find(|r| r.name == name)
                .unwrap()
                .stable_routine_id
                .clone()
        };
        let witness = id_of("GermanTax").min(id_of("InvoiceTotalRounds"));
        let mut want: Vec<(&str, &str, bool)> = candidates
            .iter()
            .map(|c| (c.as_str(), witness.as_str(), false))
            .collect();
        want.sort();
        assert_eq!(cov(&tc), want);
        assert_eq!(tc.coverage.status, "partial");
        assert!(
            tc.coverage.reasons.contains(&"interface-open-world"),
            "{:?}",
            tc.coverage
        );
    }
}

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
//! Determinism: the output lists are built from `BTreeMap`/`BTreeSet`s or sorted;
//! the `HashMap`s below are lookups only.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};

use serde::Serialize;

use crate::engine::l5::digest_cli::primary_guid_of;
use crate::engine::l5::snapshot::CapabilitySnapshot;
use crate::program::attributes::{find_attribute, string_arg};
use crate::program::model::events::PublisherRef;
use crate::program::model::role::{Role, role_of_stable_id};
use crate::program::model::workspace::Model;

/// Why a subscriber is out of reach, beyond "no path" (spec §Why `uncovered`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct SubscriberFacts {
    /// It subscribes to a platform event: no publisher routine exists.
    pub(crate) platform: bool,
    /// The dependency publishers of the events it subscribes to.
    pub(crate) dependency_publishers: BTreeSet<String>,
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

/// One changed routine of the digest (one per digest entry).
#[derive(Debug, Clone)]
pub struct ChangedRoutine {
    pub stable_id: String,
    pub display: String,
    pub object_display: String,
    /// The entry's own `(file, line)` anchor, unchanged.
    pub anchor: Option<(String, u32)>,
    /// An interface / ControlAddIn method declaration: no body, never a node.
    pub declaration_only: bool,
}

/// The `reason` of an `uncovered` item; first match wins, in this order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum UncoveredReason {
    DeclarationOnly,
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

    // routine → (witness, viaEvent); roots ascend, so the first insert wins.
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
        let hit = witness.get(id).filter(|_| !c.declaration_only);
        if let Some(&(w, via_event)) = hit {
            covered.push(CoveredItem {
                stable_id: id.to_string(),
                display: c.display.clone(),
                reached_from: vec![w.to_string()],
                via_event,
            });
            continue;
        }
        let facts = g.subscriber_facts.get(id);
        let reason = if c.declaration_only {
            UncoveredReason::DeclarationOnly
        } else if facts.is_some_and(|f| f.platform) {
            UncoveredReason::PlatformEventSubscriber
        } else if facts.is_some_and(|f| {
            !f.dependency_publishers.is_empty()
                && !f
                    .dependency_publishers
                    .iter()
                    .any(|p| witness.contains_key(p.as_str()))
        }) {
            UncoveredReason::DependencyEventSubscriber
        } else {
            UncoveredReason::NoPath
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

/// `TestPage X` / `TestRequestPage X`: what such a variable drives has no edge.
fn is_test_page_type(declared: &str) -> bool {
    let head = declared.split_whitespace().next().unwrap_or("");
    head.eq_ignore_ascii_case("testpage") || head.eq_ignore_ascii_case("testrequestpage")
}

/// The handler names of a `[HandlerFunctions('A,B')]` attribute, lowercased.
fn handler_names(attr: &crate::program::attributes::AttributeInfo) -> Vec<String> {
    string_arg(attr, 0)
        .unwrap_or_default()
        .split(',')
        .map(|n| n.trim().trim_matches('"').to_lowercase())
        .filter(|n| !n.is_empty())
        .collect()
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
        for r in ws
            .routines
            .iter()
            .filter(|r| !r.stable_routine_id.is_empty())
        {
            let id = r.stable_routine_id.as_str();
            if resolved.routine_role(r) == Role::Dependency {
                // The snapshot holds no dependency-internal calls (P6).
                flag(&mut g.boundary, id, "dependency-code");
                continue;
            }
            if find_attribute(&r.attributes_parsed, "Test").is_some() {
                g.roots.insert(id.to_string());
                g.test_code.insert(id.to_string());
                // The platform runs each named handler during the test.
                let handlers: BTreeSet<String> = r
                    .attributes_parsed
                    .iter()
                    .filter(|a| a.name.eq_ignore_ascii_case("HandlerFunctions"))
                    .flat_map(handler_names)
                    .collect();
                for h in ws.routines.iter().filter(|h| {
                    h.object_id == r.object_id && handlers.contains(&h.name.to_lowercase())
                }) {
                    g.edges
                        .entry(id.to_string())
                        .or_default()
                        .push((h.stable_routine_id.clone(), false));
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
            if r.variables
                .iter()
                .any(|v| is_test_page_type(&v.declared_type))
            {
                flag(&mut g.incomplete, id, "test-page");
            }
            // The routine's OWN state: a record's reasons are inherited from the
            // whole cone, so they are not read here.
            if r.parse_incomplete {
                flag(&mut g.incomplete, id, "parse-incomplete");
            }
            if !r.body_available {
                flag(&mut g.boundary, id, "opaque-dependency");
            }
            if !recorded.contains(id) {
                flag(&mut g.incomplete, id, "no-coverage-record");
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
            let dependency_publisher = match (&sym.publisher_ref, &sym.publisher_stable_routine_id)
            {
                (Some(PublisherRef::Platform { .. }), _) => None,
                (Some(PublisherRef::Dependency { target, .. }), _) => Some(target.clone()),
                (None, Some(p)) if role_of_stable_id(primary, p) == Role::Dependency => {
                    Some(p.clone())
                }
                _ => continue,
            };
            let facts = g
                .subscriber_facts
                .entry(edge.subscriber_stable_routine_id.clone())
                .or_default();
            match dependency_publisher {
                None => facts.platform = true,
                Some(p) => {
                    facts.dependency_publishers.insert(p);
                }
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
            declaration_only: false,
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
        decl.declaration_only = true;
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
        let dep = SubscriberFacts {
            platform: false,
            dependency_publishers: ["dep:pub".to_string()].into(),
        };
        g.subscriber_facts.insert("sub2".to_string(), dep);
        let ids = changed(&["sub1", "sub2"]);
        let tc = compute_test_coverage(&g, &ids);
        assert_eq!(
            unc(&tc),
            vec![
                ("sub1", UncoveredReason::PlatformEventSubscriber),
                ("sub2", UncoveredReason::DependencyEventSubscriber),
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
            vec![("sub1", UncoveredReason::PlatformEventSubscriber)]
        );
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
}

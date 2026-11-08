//! **Selection over one test universe** (agent-tasks design §4.4-4.5): the
//! changed roots (StableRoutineIds, resolved in the production universe) are
//! looked up in the test universe's sited graph and walked backwards; the
//! tests the walk reaches are ranked, each root gets its status, and every
//! reason the ranked subset may miss a test is stated.
//!
//! The walk is R0's ([`walk_multi`] once over every mapped root, then
//! [`walk_one`] for a root no test was labelled with, since the multi-source
//! walk labels a state with its NEAREST root only). Nothing per root outlives
//! its walk but the summary this module prints.
//!
//! # Root status (§4.4)
//!
//! - `reached`: at least one test reaches the root.
//! - `no-known-path`: the root is in the graph, no test reaches it, and its
//!   cone is complete: no cap fired and no selection-relevant hidden channel
//!   is in it.
//! - `analysis-incomplete`: no test reaches it and a cap fired or a
//!   selection-relevant hidden channel is in its cone.
//! - `not-in-projection`: no node of the test universe has the stable id.
//! - `not-loaded`: the nodes with the id have no body in the test universe
//!   (symbol-only: the production app was not loaded from source).
//!
//! # Hidden channels (§4.5)
//!
//! A node of a cone with a way to run that no site shows. Every one is
//! **selection-relevant** (it can add test callers no edge shows), with one
//! class the design allows to be covered: a platform-raised table event
//! (`OnAfterModifyEvent` …), covered when every write of that table in the
//! program is an edge into the event. Since F1 (spec
//! `2026-11-06-db-event-subscribers.md`) every record operation on the table
//! raises it through a `tableEvent` edge, so the publisher is flagged only when
//! no such site raises it (a dynamic record op, `RecordRef`, reaches it through
//! the record hub only, which does not clear the flag); then the owner's rule
//! applies: one that cannot be shown covered counts as relevant. The class is printed, with that reason, so the
//! gap is visible rather than folded away.
//!
//! # Dependency-tier sameness (§4.5, Fable f6)
//!
//! A root's through-dependency tests each reach it via a first dependency
//! node on the path (from the root's side: the dependency code that calls
//! back into local code, e.g. a Base Application page). When every such
//! node's own cone, what it may execute (forward, resolved sites only),
//! covers more than half of the graph's nodes, those tests reach nearly
//! everything: `sameness` is set and the answer calls the set broad, saying
//! why (round-3 c07: the rule measures cones, not what the tests are, so it
//! never calls them "not specific to this change"). With more than
//! [`SAMENESS_ENTRIES`] such nodes the check is not made (`viaMeasured:
//! false`).
//!
//! # Every changed routine a test reaches (round-3 c21)
//!
//! A test's `reaches` (the multi-source walk's label) is the root of its
//! shortest path only. The first [`REACH_LIMIT`] roots each have a walk of
//! their own, so `reachesAll` lists every such root a test reaches, each with
//! its tier, hops and relation (`reachesComplete: true`); the roots past the
//! limit are named (`notExpanded`) and only their label counts
//! (`reachesComplete: false`). A root's `nearest` test is relation-aware: the
//! first code test in (tier, hops) order, else the first UI-navigation one
//! (`nearestRelation`).
//!
//! Output validation r3 T1, L3: a per-root walk carries the path's relation
//! class in its states, so a test reaches a root by `code` when ANY path is
//! code (with that path's tier and hops), by `UI navigation` only when every
//! path opens a page, and as a candidate (`candidate: "runtime-editable"`)
//! only when every UI path takes a page lifecycle into a record trigger the
//! page may block at runtime ([`SitedGraph::is_ui_candidate`]).
//!
//! Round-4 c28: a test's printed `path` is its shortest, which can open a
//! page while another path reaches a changed routine by code calls (the
//! test then counts as code, by its `reachesAll`). For such a test the
//! per-root walks keep their code path, and the row carries the best one
//! (`codePath`, `codePathTier`, `codePathHops`), so the answer shows a code
//! path under a code test. A `reachesAll` entry whose relation is code also
//! carries the root's best UI-navigation reach (`navigation`: tier, hops,
//! candidate), so the renderer can judge the reach on its tier pick (a code
//! path through a hub does not make a test a code test on a local path).

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use serde_json::{Value, json};

use crate::program::impact_graph::reverse::{
    ConeVisitor, HiddenChannel, NodeIx, SiteIx, Tier, Via, WalkOptions, WalkScratch, WalkState,
    walk_multi,
};
use crate::program::impact_graph::sited::{SitedGraph, flag};
use crate::program::node::ObjKey;

/// The select part's schema name and version.
pub const SCHEMA: &str = "al-sem.tests-select";
pub const VERSION: u32 = 1;

/// The most dependency entry nodes whose cones the sameness check measures
/// per root; with more, the check is not made (and says so).
const SAMENESS_ENTRIES: usize = 16;

/// Up to this many changed roots, each root's tests come from a walk of its
/// own (exact per root); the roots after the first [`REACH_LIMIT`] take
/// their tests from the multi-source walk's nearest-root labels (one walk; a
/// root's tests that are nearer another root are counted under that root),
/// and are named as not expanded (`notExpanded`, `reachesComplete: false`).
///
/// Output validation r3 L4: measured on CDO (2026-11-08, warm,
/// `release-fast`, the select phase alone via `ALSEM_SELECT_TIMING`, median
/// of 3 after one warm-up, roots from `CDOEMail`, `CDOEMailTemplateManagement`
/// and `CDOEMailHandler`): 32 roots 830 ms, 64 roots 568 ms, 128 roots
/// 1,083 ms on a shared, loaded machine; before the walks ran in parallel
/// ([`WALK_THREADS`]) and each dependency entry's cone was measured once per
/// call, 32 roots took 2,116 ms, 64 4,466 ms and 128 7,275 ms. 128 is the
/// largest measured size under 2 s.
pub const REACH_LIMIT: usize = 128;

/// The per-root walk's consumer bits ([`WalkState::ext`]): the path took a
/// UI-navigation step (a page the path opens), and a candidate one (a page
/// lifecycle into a record trigger the page may block, [`SitedGraph::is_ui_candidate`]).
mod ext {
    pub const NAV: u8 = 1;
    pub const CANDIDATE: u8 = 2;
}

/// A reach's relation class, best first: a code path, an established
/// UI-navigation path, a path whose only UI step is a candidate (output
/// validation r3 L3).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum Class {
    Code,
    Navigation,
    Candidate,
}

impl Class {
    fn of(ext: u8) -> Class {
        if ext & ext::CANDIDATE != 0 {
            Class::Candidate
        } else if ext & ext::NAV != 0 {
            Class::Navigation
        } else {
            Class::Code
        }
    }

    fn relation(self) -> &'static str {
        match self {
            Class::Code => crate::engine::vocab::RELATION_CODE,
            _ => crate::engine::vocab::RELATION_NAVIGATION,
        }
    }

    /// The `candidate` value of a reach in this class.
    fn candidate(self) -> Option<&'static str> {
        (self == Class::Candidate).then_some(RUNTIME_EDITABLE)
    }
}

/// The `candidate` value of a reach whose only UI step is a page a runtime
/// `Editable(…)` setter may make editable (output validation r3 L3).
pub const RUNTIME_EDITABLE: &str = "runtime-editable";

/// The walk's visitor: per state its parent and root label, per node its
/// best tier, every hidden node, every test's best state.
#[derive(Default)]
struct Sel {
    parent: HashMap<WalkState, WalkState>,
    best: HashMap<NodeIx, (Tier, u32)>,
    hidden: BTreeMap<NodeIx, BTreeSet<&'static str>>,
    universal: bool,
    /// Test node -> (tier, hops, state, root label).
    tests: HashMap<NodeIx, (Tier, u32, WalkState, u32)>,
    /// With [`Sel::classes`]: per (test node, relation class) its best
    /// (tier, hops, state).
    by_class: HashMap<(NodeIx, Class), (Tier, u32, WalkState)>,
    /// Whether the walk tracks the relation class in its states (the
    /// per-root walks: a test reaches a root by code when ANY path is code,
    /// output validation r3 T1).
    classes: bool,
    /// The nodes that are a page the path opens ([`super::render::is_navigation_hop`]).
    nav_nodes: std::sync::Arc<Vec<bool>>,
    limited: bool,
    /// A per-root status walk: it stops (through the walk's cancellation)
    /// once a test is reached, and a cap is then not a limit.
    stop_at_test: bool,
    found_test: std::rc::Rc<std::cell::Cell<bool>>,
    test_flagged: Vec<bool>,
}

impl ConeVisitor for Sel {
    fn reach(&mut self, root: u32, s: WalkState, hops: u32, _: Via, from: Option<WalkState>) {
        if let Some(f) = from {
            self.parent.insert(s, f);
        }
        let e = self.best.entry(s.node).or_insert((s.tier, hops));
        if (s.tier, hops) < *e {
            *e = (s.tier, hops);
        }
        if self.test_flagged[s.node as usize] {
            self.found_test.set(true);
            let better = self
                .tests
                .get(&s.node)
                .is_none_or(|b| (s.tier, hops) < (b.0, b.1));
            if better {
                self.tests.insert(s.node, (s.tier, hops, s, root));
            }
            if self.classes {
                let k = (s.node, Class::of(s.ext));
                if self
                    .by_class
                    .get(&k)
                    .is_none_or(|b| (s.tier, hops) < (b.0, b.1))
                {
                    self.by_class.insert(k, (s.tier, hops, s));
                }
            }
        }
    }
    fn hidden(&mut self, _: u32, n: NodeIx, ch: HiddenChannel) {
        match ch {
            HiddenChannel::Universal => self.universal = true,
            HiddenChannel::Flag(f) => {
                self.hidden.entry(n).or_default().insert(f);
            }
        }
    }
    /// A test cannot raise a control add-in's event: a `TestPage` hosts no
    /// add-in, and no AL statement raises one (round-1 c21). The page-run
    /// path into such a trigger (`via: clientEvent`) is a user's client's,
    /// so a test walk does not follow it; impact's walk does.
    ///
    /// With [`Sel::classes`], a step onto a page the path opens sets
    /// [`ext::NAV`], and a candidate page-lifecycle site sets
    /// [`ext::CANDIDATE`] too, so a test reached by a code path and by a
    /// UI-navigation path is two states (output validation r3 T1, L3).
    fn step(&mut self, g: &SitedGraph, from: WalkState, site: SiteIx) -> Option<u8> {
        let st = &g.sites()[site as usize];
        if st.via() == "clientEvent" {
            return None;
        }
        if !self.classes {
            return Some(from.ext);
        }
        let mut bits = from.ext;
        if self
            .nav_nodes
            .get(st.from as usize)
            .copied()
            .unwrap_or(false)
        {
            bits |= ext::NAV;
        }
        if g.is_ui_candidate(site) {
            bits |= ext::NAV | ext::CANDIDATE;
        }
        Some(bits)
    }
    fn limit(&mut self, l: crate::program::impact_graph::reverse::Limit) {
        let ours = self.stop_at_test
            && self.found_test.get()
            && l == crate::program::impact_graph::reverse::Limit::Cancelled;
        if !ours {
            self.limited = true;
        }
    }
}

impl Sel {
    fn new(g: &SitedGraph, stop_at_test: bool) -> Sel {
        Sel {
            stop_at_test,
            test_flagged: (0..g.node_count() as u32)
                .map(|n| g.flags(n) & flag::TEST != 0)
                .collect(),
            ..Sel::default()
        }
    }

    /// A per-root walk that tracks each test's relation class
    /// ([`Sel::classes`]).
    fn with_classes(g: &SitedGraph, nav_nodes: &std::sync::Arc<Vec<bool>>) -> Sel {
        Sel {
            classes: true,
            nav_nodes: std::sync::Arc::clone(nav_nodes),
            ..Sel::new(g, false)
        }
    }

    /// Whether the cone holds a selection-relevant hidden channel in its
    /// resolved part, or the universal hub.
    fn relevant_hidden(&self, g: &SitedGraph) -> bool {
        self.universal
            || self.hidden.iter().any(|(n, chs)| {
                self.best
                    .get(n)
                    .is_some_and(|b| b.0 <= Tier::ThroughDependency)
                    && chs.iter().any(|c| hidden_class(g, *n, c).1)
            })
    }

    /// The path of `s` back to its root: `s`'s node first.
    fn path(&self, s: WalkState) -> Vec<NodeIx> {
        let mut out = vec![s.node];
        let mut at = s;
        while let Some(p) = self.parent.get(&at) {
            out.push(p.node);
            at = *p;
            if out.len() > 256 {
                break;
            }
        }
        out
    }
}

/// A node's printed name: the one routine display (output validation V3,
/// §2.1, §2.2: `Codeunit "CDO Telemetry"::UpdateLastUpload`, a field
/// trigger with its field, an overloaded name with its parameter types, a
/// dependency object by name with its number), from the graph node's
/// declared names; the graph's key otherwise.
#[must_use]
pub fn display(g: &SitedGraph, n: NodeIx) -> String {
    if let Some(d) = crate::engine::l5::routine_rollup::graph_display_local(g, n) {
        return d;
    }
    match (g.local_name(n), g.routine_object(n)) {
        (Some(l), Some(o)) => format!(
            "{} {}::{}",
            crate::program::graphify_export::object_kind_str(o.kind),
            l.object,
            l.routine
        ),
        // An object node (its run or lifecycle): `Codeunit Name`.
        (Some(l), None) if l.routine.is_empty() => {
            let d = g.display(n);
            let kind = d.split(' ').next().unwrap_or("");
            format!("{kind} {}", l.object)
        }
        _ => g.display(n),
    }
}

/// A path hop's name (agent-tasks §12.1, output validation F9, V3 §2.1):
/// [`display`], which prints an overloaded name's parameter types, so two
/// overloads on one path never print alike.
fn path_name(g: &SitedGraph, n: NodeIx) -> String {
    display(g, n)
}

fn object_number(g: &SitedGraph, n: NodeIx) -> Option<i64> {
    match g.routine_object(n)?.key {
        ObjKey::Id(i) => Some(i),
        ObjKey::Name(_) => None,
    }
}

/// A hidden channel's class and whether it is selection-relevant (see the
/// module doc).
fn hidden_class(g: &SitedGraph, n: NodeIx, ch: &str) -> (&'static str, bool, &'static str) {
    match ch {
        "unraisedPublisher" if g.is_platform_publisher(n) => (
            "platform-table-event",
            true,
            "a platform-raised table event no record operation in the graph raises: its \
             table's writes are outside the loaded code, so it is not shown covered",
        ),
        "unraisedPublisher" => (
            "unraised-publisher",
            true,
            "a publisher no site raises: its raiser may be a test no edge shows",
        ),
        "unboundSubscriber" => (
            "unbound-subscriber",
            true,
            "a subscriber no event edge reaches: its publisher may be raised by a test",
        ),
        "unmodeledRecordTrigger" => (
            "unmodeled-record-trigger",
            true,
            "a record trigger no operation site is modelled for",
        ),
        _ => ("other", true, "a way to run no site shows"),
    }
}

fn tier_name(t: Tier) -> &'static str {
    match t {
        Tier::Local => "local",
        Tier::ThroughDependency => "throughDependency",
        Tier::OnlyOpen => "onlyOpen",
    }
}

/// The relation of a path (test first, then toward the root), by the
/// renderer's own rule ([`super::render::is_navigation_hop`]); a node's
/// answer is cached in `nav`.
fn path_relation(g: &SitedGraph, path: &[NodeIx], nav: &mut HashMap<NodeIx, bool>) -> &'static str {
    let opens = path.iter().skip(1).any(|&n| {
        *nav.entry(n)
            .or_insert_with(|| super::render::is_navigation_hop(&path_name(g, n)))
    });
    if opens {
        crate::engine::vocab::RELATION_NAVIGATION
    } else {
        crate::engine::vocab::RELATION_CODE
    }
}

/// The nodes that are a page a path opens, by [`path_relation`]'s rule: an
/// object node (a routine's display always names its routine) whose display
/// is a bare page.
fn nav_mask(g: &SitedGraph) -> Vec<bool> {
    (0..g.node_count() as u32)
        .map(|n| {
            g.routine_object(n).is_none() && super::render::is_navigation_hop(&path_name(g, n))
        })
        .collect()
}

/// Whether a path (test first, then toward the root) takes a candidate UI
/// step: two consecutive nodes joined only by candidate page-lifecycle sites
/// ([`SitedGraph::is_ui_candidate`], output validation r3 L3).
fn path_candidate(g: &SitedGraph, path: &[NodeIx]) -> bool {
    path.windows(2).any(|w| {
        let mut joining = g
            .sites_into(w[1])
            .iter()
            .filter(|&&s| g.sites()[s as usize].from == w[0])
            .peekable();
        joining.peek().is_some() && joining.all(|&s| g.is_ui_candidate(s))
    })
}

fn class_of(s: &WalkState) -> &'static str {
    if s.tier == Tier::OnlyOpen {
        "dynamic"
    } else if s.crossed_event {
        "event"
    } else {
        "direct"
    }
}

/// The most threads the per-root walks run on at once: each walk keeps its
/// states until its summary is taken, so this bounds the select part's
/// transient memory.
const WALK_THREADS: usize = 8;

/// The first dependency node of a path from the root's side (the
/// dependency code that calls back into local code, §4.5).
fn first_dependency_node(g: &SitedGraph, path: &[NodeIx]) -> Option<NodeIx> {
    path.iter()
        .rev()
        .find(|&&n| g.flags(n) & flag::LOCAL == 0)
        .copied()
}

/// One of a root's tests: (node, tier, hops, its best state).
type RootTest = (NodeIx, Tier, u32, WalkState);

/// A test's reach of a root: its relation class and (tier, hops) in it.
type ReachClass = (Class, Tier, u32);

/// What a root's own walk leaves once its states are dropped (output
/// validation r3 L4: the walks run in parallel, then the roots are
/// summarized in order).
struct RootWalk {
    /// The root's tests, (node, tier, hops, state), in (tier, hops, codeunit,
    /// name) order.
    mine: Vec<(NodeIx, Tier, u32, WalkState)>,
    /// Per test of `mine`: its relation class and (tier, hops) in that class
    /// (code when ANY path is code, r3 T1; a candidate only when every UI
    /// path takes a candidate step, L3).
    rel: Vec<(Class, Tier, u32)>,
    /// Per test of `mine` whose class is code: its best UI-navigation reach
    /// (an established one, else a candidate), when it has one (round-4
    /// c28: the answer's tier pick can leave the code path out).
    alt: Vec<Option<(Class, Tier, u32)>>,
    /// A cap fired, or a selection-relevant hidden channel is in the cone.
    incomplete: bool,
    /// Per through-dependency test of `mine`, in order: the first dependency
    /// node of its path.
    via: Vec<Option<NodeIx>>,
    /// Round-4 c28: per test of `wanted` (a test whose printed path, the
    /// multi-source walk's shortest one, opens a page) that reaches this
    /// root by a code path: that path's (tier, hops, nodes), so a test
    /// counted as code shows a code path, not only its UI one.
    code_paths: Vec<(NodeIx, Tier, u32, Vec<NodeIx>)>,
}

impl RootWalk {
    fn of(
        g: &SitedGraph,
        nodes: &[NodeIx],
        nav_nodes: &std::sync::Arc<Vec<bool>>,
        wanted: &HashSet<NodeIx>,
    ) -> RootWalk {
        let opts = WalkOptions {
            assume_no_dependents: true,
            ..Default::default()
        };
        let mut own = Sel::with_classes(g, nav_nodes);
        let mut scratch = WalkScratch::default();
        walk_multi(g, nodes, &opts, &mut scratch, &mut own);
        let key = |n: NodeIx| (object_number(g, n).unwrap_or(i64::MAX), display(g, n));
        let mut mine: Vec<(NodeIx, Tier, u32, WalkState)> = own
            .tests
            .iter()
            .map(|(&n, &(t, h, s, _))| (n, t, h, s))
            .collect();
        mine.sort_by_cached_key(|a| (a.1, a.2, key(a.0)));
        let mut nav: HashMap<NodeIx, bool> = HashMap::new();
        let rel = mine
            .iter()
            .map(|x| {
                [Class::Code, Class::Navigation, Class::Candidate]
                    .into_iter()
                    .find_map(|c| own.by_class.get(&(x.0, c)).map(|b| (c, b.0, b.1)))
                    .unwrap_or_else(|| {
                        let path = own.path(x.3);
                        let class = if path_relation(g, &path, &mut nav)
                            == crate::engine::vocab::RELATION_CODE
                        {
                            Class::Code
                        } else if path_candidate(g, &path) {
                            Class::Candidate
                        } else {
                            Class::Navigation
                        };
                        (class, x.1, x.2)
                    })
            })
            .collect();
        let alt = mine
            .iter()
            .zip(&rel)
            .map(|(x, r): (_, &ReachClass)| {
                (r.0 == Class::Code)
                    .then(|| {
                        [Class::Navigation, Class::Candidate]
                            .into_iter()
                            .find_map(|c| own.by_class.get(&(x.0, c)).map(|b| (c, b.0, b.1)))
                    })
                    .flatten()
            })
            .collect();
        let via = mine
            .iter()
            .filter(|x| x.1 == Tier::ThroughDependency)
            .map(|x| first_dependency_node(g, &own.path(x.3)))
            .collect();
        let code_paths = mine
            .iter()
            .filter(|x| wanted.contains(&x.0))
            .filter_map(|x| {
                own.by_class
                    .get(&(x.0, Class::Code))
                    .map(|b| (x.0, b.0, b.1, own.path(b.2)))
            })
            .collect();
        RootWalk {
            incomplete: own.limited || own.relevant_hidden(g),
            mine,
            rel,
            alt,
            via,
            code_paths,
        }
    }
}

/// What the select part is asked.
pub struct SelectArgs<'a> {
    /// The changed roots, StableRoutineIds, in the production part's order.
    pub roots: &'a [String],
    pub universe_key: &'a str,
    pub test_app_id: &'a str,
    pub production_app_id: &'a str,
    /// The production stamp of the bytes this universe consumed.
    pub production_stamp: Option<String>,
    /// The test app's declared dependencies `(id, name, publisher)`.
    pub declared: &'a [(String, String, String)],
    /// Production routines to probe apart (impact's nearest tested caller):
    /// whether this universe has each and whether a test reaches it. They
    /// never join the selection.
    pub probes: &'a [String],
}

/// One probe's answer: `not-in-projection`, `not-loaded`, `reached` (with
/// its nearest test) or `not-reached`.
fn probe(g: &SitedGraph, id: &str, opts: &WalkOptions<'_>) -> Value {
    let all = g.nodes_with_stable_id(id);
    let nodes: Vec<NodeIx> = all
        .iter()
        .copied()
        .filter(|&n| g.flags(n) & flag::SYMBOL_ONLY == 0)
        .collect();
    if nodes.is_empty() {
        let st = if all.is_empty() {
            "not-in-projection"
        } else {
            "not-loaded"
        };
        return json!({"stableId": id, "status": st});
    }
    let mut own = Sel::new(g, true);
    let found = std::rc::Rc::clone(&own.found_test);
    let stop = move || found.get();
    let o = WalkOptions {
        cancel: Some(&stop),
        ..*opts
    };
    let mut s2 = WalkScratch::default();
    walk_multi(g, &nodes, &o, &mut s2, &mut own);
    let best = own
        .tests
        .iter()
        .map(|(&n, &(t, h, _, _))| (t, h, n))
        .min_by_key(|&(t, h, n)| (t, h, display(g, n)));
    match best {
        Some((t, h, n)) => json!({"stableId": id, "status": "reached", "nearest": display(g, n),
                                  "nearestTier": tier_name(t), "nearestHops": h}),
        None => json!({"stableId": id, "status": "not-reached"}),
    }
}

/// The **direct callers** of routine node `r` in the sited graph, as
/// explain's `callers:` line counts them (agent-tasks R0-3): every routine
/// with a direct or interface call (`Sync`), an object run (`Run`) or a
/// record operation firing the trigger (`TriggerOf`) into `r`; and, when
/// `r` is a trigger a run of its object runs (`Lifecycle`: a codeunit's
/// `OnRun`, a page's or report's lifecycle triggers and the same triggers of
/// its extensions, [`crate::program::impact_graph::reverse::object_run_reaches`]),
/// every routine that runs that object (the model's calls
/// map `Codeunit.Run` straight to `OnRun`; the sited graph goes through the
/// object node). Only call sites the model has count (`via` call,
/// interface, run, implicitTrigger): a rule's edge (`declaredRunnable`, a
/// test's TestPage variable) is no call. Not callers: an event's publisher (a subscriber is
/// dispatched, not called), a background start (a `Background` site, F3: the
/// target runs in its own session, and the model's calls do not carry it;
/// the walks over the graph still reach the starter), a test's handler list, open sites, `r` itself,
/// hubs, and a page action's `OnAction` trigger (page actions are counted
/// apart, explain's F16j; a page's other triggers are callers).
#[must_use]
pub fn direct_callers(g: &SitedGraph, r: NodeIx) -> BTreeSet<NodeIx> {
    use crate::program::impact_graph::sited::Boundary;
    // A page action's `OnAction` trigger on a page or page extension
    // (explain's F16j: counted under page actions). A page's other triggers
    // (`OnOpenPage`, a field's `OnValidate`, …) are callers.
    let is_page_trigger = |n: NodeIx| {
        matches!(
            g.nodes.get(n as usize),
            Some(crate::program::impact_graph::project::Key::Routine(id))
                if matches!(
                    id.object.kind,
                    al_syntax::ir::ObjectKind::Page | al_syntax::ir::ObjectKind::PageExtension
                ) && id.enclosing_member_lc.is_some()
                    && id.name_lc.as_str() == "onaction"
        ) && g.local_name(n).is_some_and(|l| l.trigger)
    };
    let mut out = BTreeSet::new();
    let add = |n: NodeIx, out: &mut BTreeSet<NodeIx>| {
        if n != r
            && g.flags(n) & flag::HUB == 0
            && g.routine_object(n).is_some()
            && !is_page_trigger(n)
        {
            out.insert(n);
        }
    };
    // Whether a run of `r`'s object runs `r` (the resolver's own rule: a
    // codeunit's `OnRun`, a page's or report's lifecycle triggers, and those
    // of a page or report extension).
    let run_reaches = crate::program::impact_graph::reverse::object_run_reaches(g, r);
    for &s in g.sites_into(r) {
        let site = &g.sites()[s as usize];
        let from_routine = g.routine_object(site.from).is_some();
        // A call site the model has: not a rule's edge (`declaredRunnable`,
        // a TestPage variable; `hubExpansion`, `coverageMiss`, …).
        let called = |s: &crate::program::impact_graph::sited::Site| {
            matches!(s.via(), "call" | "interface" | "run" | "implicitTrigger")
        };
        match site.boundary {
            Boundary::Sync | Boundary::Run | Boundary::TriggerOf { .. }
                if from_routine && called(site) =>
            {
                add(site.from, &mut out);
            }
            // An object running its trigger: whoever runs the object.
            Boundary::Lifecycle if !from_routine && run_reaches => {
                for &t in g.sites_into(site.from) {
                    let run = &g.sites()[t as usize];
                    if run.boundary == Boundary::Run && called(run) {
                        add(run.from, &mut out);
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// The graph's forward adjacency over resolved sites (no open site), built
/// once per select call that needs it: `to[off[n]..off[n + 1]]`.
struct Forward {
    off: Vec<u32>,
    to: Vec<u32>,
}

impl Forward {
    fn of(g: &SitedGraph) -> Forward {
        let n = g.node_count();
        let mut off = vec![0u32; n + 1];
        let keep = |s: &crate::program::impact_graph::sited::Site| {
            s.boundary != crate::program::impact_graph::sited::Boundary::Open
        };
        for s in g.sites().iter().filter(|s| keep(s)) {
            off[s.from as usize + 1] += 1;
        }
        for i in 0..n {
            off[i + 1] += off[i];
        }
        let mut fill = off.clone();
        let mut to = vec![0u32; off[n] as usize];
        for s in g.sites().iter().filter(|s| keep(s)) {
            let slot = &mut fill[s.from as usize];
            to[*slot as usize] = s.to;
            *slot += 1;
        }
        Forward { off, to }
    }

    /// The distinct nodes `n` may execute (its forward cone, `n` included).
    fn cone(&self, n: NodeIx) -> usize {
        let mut seen: HashSet<NodeIx> = HashSet::from([n]);
        let mut queue = vec![n];
        while let Some(x) = queue.pop() {
            let (a, b) = (
                self.off[x as usize] as usize,
                self.off[x as usize + 1] as usize,
            );
            for &y in &self.to[a..b] {
                if seen.insert(y) {
                    queue.push(y);
                }
            }
        }
        seen.len()
    }
}

/// Select over `g` (see the module doc): the select part's JSON.
#[must_use]
pub fn select(g: &SitedGraph, args: &SelectArgs<'_>) -> Value {
    // Each root's nodes.
    let mapped: Vec<Vec<NodeIx>> = args
        .roots
        .iter()
        .map(|id| {
            g.nodes_with_stable_id(id)
                .into_iter()
                .filter(|&n| g.flags(n) & flag::SYMBOL_ONLY == 0)
                .collect()
        })
        .collect();
    select_mapped(g, args, &mapped)
}

/// [`select`] once each root's nodes are known (`mapped[i]` for
/// `args.roots[i]`; a hand-built graph's tests state them directly).
fn select_mapped(g: &SitedGraph, args: &SelectArgs<'_>, mapped: &[Vec<NodeIx>]) -> Value {
    let primary = g.primary_app();
    let tests_loaded = (0..g.node_count() as u32)
        .filter(|&n| g.flags(n) & flag::TEST != 0 && g.node_app(n) == Some(primary))
        .count();
    // The walk's roots (node -> root index).
    let mut walk_roots: Vec<NodeIx> = Vec::new();
    let mut label_root: Vec<usize> = Vec::new();
    for (i, nodes) in mapped.iter().enumerate() {
        for &n in nodes {
            walk_roots.push(n);
            label_root.push(i);
        }
    }
    let opts = WalkOptions {
        assume_no_dependents: true,
        ..Default::default()
    };
    let mut scratch = WalkScratch::default();
    let mut sel = Sel::new(g, false);
    let stats = walk_multi(g, &walk_roots, &opts, &mut scratch, &mut sel);
    // Ranked tests (tier, hops, codeunit number, name).
    let mut tests: Vec<(NodeIx, Tier, u32, WalkState, usize)> = sel
        .tests
        .iter()
        .map(|(&n, &(t, h, s, l))| (n, t, h, s, label_root[l as usize]))
        .collect();
    let key = |n: NodeIx| (object_number(g, n).unwrap_or(i64::MAX), display(g, n));
    tests.sort_by_cached_key(|a| (a.1, a.2, key(a.0)));
    // Per root: the tests labelled with it.
    let mut per_root: Vec<Vec<usize>> = vec![Vec::new(); args.roots.len()];
    for (j, t) in tests.iter().enumerate() {
        per_root[t.4].push(j);
    }
    // Hidden nodes in the resolved part of the cone.
    let resolved_hidden: Vec<(NodeIx, &'static str)> = sel
        .hidden
        .iter()
        .filter(|(n, _)| {
            sel.best
                .get(n)
                .is_some_and(|b| b.0 <= Tier::ThroughDependency)
        })
        .flat_map(|(n, chs)| chs.iter().map(move |c| (*n, *c)))
        .collect();
    let hidden: Vec<Value> = resolved_hidden
        .iter()
        .map(|&(n, ch)| {
            let (class, relevant, reason) = hidden_class(g, n, ch);
            json!({"node": display(g, n), "nodeId": g.node_id(n), "channel": ch,
                   "class": class, "selectionRelevant": relevant, "reason": reason})
        })
        .collect();
    // Roots.
    let mut roots_out = Vec::with_capacity(args.roots.len());
    let mut forward: Option<Forward> = None;
    let mut cone_size: HashMap<NodeIx, usize> = HashMap::new();
    let mut nav: HashMap<NodeIx, bool> = HashMap::new();
    let nav_nodes = std::sync::Arc::new(nav_mask(g));
    // Round-4 c28: the tests whose printed path (their shortest) opens a
    // page; the per-root walks keep a code path for each of them.
    let nav_printed: HashSet<NodeIx> = tests
        .iter()
        .filter(|t| {
            path_relation(g, &sel.path(t.3), &mut nav) != crate::engine::vocab::RELATION_CODE
        })
        .map(|t| t.0)
        .collect();
    // Output validation r3 L4: the first `REACH_LIMIT` roots each get a walk
    // of their own, run in parallel (each walk is independent and only its
    // summary outlives it); the others are not expanded (their tests come
    // from the labels) and are named.
    let mut walks: Vec<Option<RootWalk>> = {
        use rayon::prelude::*;
        let expanded = args.roots.len().min(REACH_LIMIT);
        let one = |i: usize| {
            (!mapped[i].is_empty()).then(|| RootWalk::of(g, &mapped[i], &nav_nodes, &nav_printed))
        };
        let threads = std::thread::available_parallelism()
            .map_or(1, |n| n.get())
            .min(WALK_THREADS);
        match rayon::ThreadPoolBuilder::new().num_threads(threads).build() {
            Ok(pool) if expanded > 1 && threads > 1 => {
                pool.install(|| (0..expanded).into_par_iter().map(one).collect())
            }
            _ => (0..expanded).map(one).collect(),
        }
    };
    let mut not_expanded: Vec<&String> = Vec::new();
    // Per test: every root it reaches, (root index, tier, hops, relation
    // class), from the per-root walks (exact: code when any path is code) or
    // the labels (the root of its shortest path only).
    // Round-4 c28: with a code reach, the best UI-navigation reach of the
    // same root too (a code path may lie outside the tier the answer picks).
    type Reach = (usize, Tier, u32, Class, Option<ReachClass>);
    let mut reach_all: HashMap<NodeIx, Vec<Reach>> = HashMap::new();
    // Round-4 c28: per test whose printed path opens a page, its best code
    // path to any changed routine (tier, hops, nodes; the first root wins a
    // tie).
    let mut code_path: HashMap<NodeIx, (Tier, u32, Vec<NodeIx>)> = HashMap::new();
    let label_class = |path: &[NodeIx], nav: &mut HashMap<NodeIx, bool>| -> Class {
        if path_relation(g, path, nav) == crate::engine::vocab::RELATION_CODE {
            Class::Code
        } else if path_candidate(g, path) {
            Class::Candidate
        } else {
            Class::Navigation
        }
    };
    for (i, id) in args.roots.iter().enumerate() {
        let any_node = !g.nodes_with_stable_id(id).is_empty();
        let mut r = json!({"stableId": id});
        if mapped[i].is_empty() {
            r["status"] = json!(if any_node {
                "not-loaded"
            } else {
                "not-in-projection"
            });
            roots_out.push(r);
            continue;
        }
        r["display"] = json!(display(g, mapped[i][0]));
        r["nodes"] = json!(mapped[i].len());
        // The root's own tests: from its own walk within the first
        // `REACH_LIMIT` roots (the multi-source walk labels a state with its
        // NEAREST root, so a root's tests nearer another root would be
        // counted there), else from the labels.
        let mut own_walk: Option<RootWalk> = walks.get_mut(i).and_then(Option::take);
        for (n, t, h, p) in own_walk
            .as_mut()
            .map(|w| std::mem::take(&mut w.code_paths))
            .unwrap_or_default()
        {
            if code_path.get(&n).is_none_or(|b| (t, h) < (b.0, b.1)) {
                code_path.insert(n, (t, h, p));
            }
        }
        if i >= REACH_LIMIT {
            not_expanded.push(id);
        }
        let (mine, rel, alt): (Vec<RootTest>, Vec<ReachClass>, Vec<Option<ReachClass>>) =
            match &own_walk {
                Some(w) => (w.mine.clone(), w.rel.clone(), w.alt.clone()),
                // Each label test's relation class, from its one path.
                None => {
                    let mine: Vec<RootTest> = per_root[i]
                        .iter()
                        .map(|&j| (tests[j].0, tests[j].1, tests[j].2, tests[j].3))
                        .collect();
                    let rel = mine
                        .iter()
                        .map(|x| (label_class(&sel.path(x.3), &mut nav), x.1, x.2))
                        .collect();
                    let alt = vec![None; mine.len()];
                    (mine, rel, alt)
                }
            };
        for ((x, r), a) in mine.iter().zip(&rel).zip(&alt) {
            reach_all
                .entry(x.0)
                .or_default()
                .push((i, r.1, r.2, r.0, *a));
        }
        let count = |t: Tier| mine.iter().filter(|x| x.1 == t).count();
        let status = if !mine.is_empty() {
            "reached"
        } else if let Some(own) = &own_walk {
            if own.incomplete {
                "analysis-incomplete"
            } else {
                "no-known-path"
            }
        } else {
            // A status walk of its own: the multi-source walk labels states
            // with the nearest root only.
            let mut own = Sel::new(g, true);
            let found = std::rc::Rc::clone(&own.found_test);
            let stop = move || found.get();
            let o = WalkOptions {
                cancel: Some(&stop),
                ..opts
            };
            let mut s2 = WalkScratch::default();
            walk_multi(g, &mapped[i], &o, &mut s2, &mut own);
            let relevant_hidden = own.relevant_hidden(g);
            if own.found_test.get() {
                "reached"
            } else if own.limited || stats.limited || relevant_hidden {
                "analysis-incomplete"
            } else {
                "no-known-path"
            }
        };
        r["status"] = json!(status);
        r["tests"] = json!({"local": count(Tier::Local),
                            "throughDependency": count(Tier::ThroughDependency),
                            "onlyOpen": count(Tier::OnlyOpen)});
        // Round-3 c21: the nearest test is the first code test (tier, hops,
        // codeunit, name), else the first UI-navigation one.
        // A candidate (L3) after an established UI-navigation test.
        if let Some((n, r_)) = mine
            .iter()
            .zip(&rel)
            .min_by_key(|(x, r_)| (r_.0, r_.1, r_.2, key(x.0)))
        {
            r["nearest"] = json!(display(g, n.0));
            r["nearestTier"] = json!(tier_name(r_.1));
            r["nearestHops"] = json!(r_.2);
            r["nearestRelation"] = json!(r_.0.relation());
            if let Some(c) = r_.0.candidate() {
                r["nearestCandidate"] = json!(c);
            }
        }
        // Dependency-tier sameness (§4.5).
        let thr: Vec<&(NodeIx, Tier, u32, WalkState)> = mine
            .iter()
            .filter(|x| x.1 == Tier::ThroughDependency)
            .collect();
        if !thr.is_empty() {
            let mut via: BTreeMap<NodeIx, usize> = BTreeMap::new();
            let firsts: Vec<Option<NodeIx>> = match &own_walk {
                Some(w) => w.via.clone(),
                None => thr
                    .iter()
                    .map(|t| first_dependency_node(g, &sel.path(t.3)))
                    .collect(),
            };
            for d in firsts.into_iter().flatten() {
                *via.entry(d).or_default() += 1;
            }
            let mut by: Vec<(NodeIx, usize)> = via.into_iter().collect();
            by.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
            let total = g.node_count();
            let measured = by.len() <= SAMENESS_ENTRIES;
            let mut entries = Vec::new();
            let mut all_big = measured && !by.is_empty();
            if measured {
                let fwd = forward.get_or_insert_with(|| Forward::of(g));
                for &(d, n) in &by {
                    // A dependency entry's cone is the same for every root
                    // (r3 L4: measured once per select call).
                    let c = *cone_size.entry(d).or_insert_with(|| fwd.cone(d));
                    let big = c * 2 > total;
                    all_big &= big;
                    entries.push(json!({"node": display(g, d), "tests": n, "coneNodes": c,
                                        "overHalfOfGraph": big}));
                }
            }
            r["throughDependency"] = json!({
                "tests": thr.len(),
                "via": entries,
                "viaMeasured": measured,
                "graphNodes": total,
                "sameness": all_big,
            });
        }
        roots_out.push(r);
    }
    // Unresolved calls made by test code, and the declared dependencies the
    // universe did not load.
    // (caller node, its site's (unit, line, column)): one per call site.
    type SiteKey = (NodeIx, Option<(u32, u32, u32)>);
    let mut unresolved: BTreeSet<SiteKey> = BTreeSet::new();
    for s in g.sites() {
        if s.via() == "unresolved"
            && g.node_app(s.from) == Some(primary)
            && g.flags(s.from) & flag::HUB == 0
        {
            unresolved.insert((
                s.from,
                s.anchor.map(|a| (a.unit, a.start_line, a.start_col)),
            ));
        }
    }
    let loaded: HashSet<String> = g
        .apps()
        .map(|(guid, _)| guid.to_ascii_lowercase())
        .collect();
    let missing: Vec<Value> = args
        .declared
        .iter()
        .filter(|(id, _, _)| !loaded.contains(&id.to_ascii_lowercase()))
        .map(|(id, name, publisher)| json!({"id": id, "name": name, "publisher": publisher}))
        .collect();
    let production_local = (0..g.node_count() as u32).any(|n| {
        g.node_app(n)
            .map(|a| {
                g.app_identity(a)
                    .0
                    .eq_ignore_ascii_case(args.production_app_id)
            })
            .unwrap_or(false)
            && g.flags(n) & flag::LOCAL != 0
    });
    let by_tier = |t: Tier| tests.iter().filter(|x| x.1 == t).count();
    let tests_out: Vec<Value> = tests
        .iter()
        .map(|&(n, t, h, s, root)| {
            let path = sel.path(s);
            let l = g.local_name(n);
            // Every root the test reaches, in the roots' order; the label
            // root is always one of them (a capped per-root walk may miss it).
            let mut all = reach_all.get(&n).cloned().unwrap_or_default();
            if !all.iter().any(|x| x.0 == root) {
                all.push((root, t, h, label_class(&path, &mut nav), None));
            }
            all.sort_by_key(|x| x.0);
            let reaches_all: Vec<Value> = all
                .iter()
                .map(|&(r, rt, rh, class, alt)| {
                    let mut v = json!({"stableId": args.roots[r], "tier": tier_name(rt),
                                       "hops": rh, "relation": class.relation()});
                    if let Some(c) = class.candidate() {
                        v["candidate"] = json!(c);
                    }
                    // Round-4 c28: the best UI-navigation reach beside a
                    // code one, for a tier pick the code path is outside of.
                    if let Some((ac, at, ah)) = alt {
                        v["navigation"] = json!({"tier": tier_name(at), "hops": ah});
                        if let Some(c) = ac.candidate() {
                            v["navigation"]["candidate"] = json!(c);
                        }
                    }
                    v
                })
                .collect();
            // The printed path's own candidate step (L3), for the human
            // text's `UI navigation (candidate)`.
            let path_is_candidate = path_relation(g, &path, &mut nav)
                != crate::engine::vocab::RELATION_CODE
                && path_candidate(g, &path);
            let mut row = json!({
                "stableId": g.stable_id(n),
                "display": display(g, n),
                "codeunitId": object_number(g, n),
                "codeunit": l.map(|l| l.object.to_string()),
                "procedure": l.map(|l| l.routine.to_string()),
                "file": l.map(|l| l.file.to_string()),
                "tier": tier_name(t),
                "class": class_of(&s),
                "hops": h,
                "reaches": args.roots[root],
                "reachesAll": reaches_all,
                "path": path.iter().map(|&p| path_name(g, p)).collect::<Vec<_>>(),
            });
            if path_is_candidate {
                row["pathCandidate"] = json!(RUNTIME_EDITABLE);
            }
            // Round-4 c28: the printed path opens a page but a code path
            // reaches a changed routine too (the test counts as code): that
            // code path, so the answer can show it.
            if let Some((ct, ch, cp)) = code_path.get(&n) {
                row["codePath"] = json!(cp.iter().map(|&p| path_name(g, p)).collect::<Vec<_>>());
                row["codePathTier"] = json!(tier_name(*ct));
                row["codePathHops"] = json!(ch);
            }
            row
        })
        .collect();
    json!({
        "schema": SCHEMA,
        "version": VERSION,
        "universe": args.universe_key,
        "testAppId": args.test_app_id,
        "productionSourceStamp": args.production_stamp,
        "productionTier": if production_local { "localApproximate" } else { "notLocalSource" },
        "testsLoaded": tests_loaded,
        "roots": roots_out,
        "tests": tests_out,
        "reachesComplete": not_expanded.is_empty(),
        // Output validation r3 L4: the per-root walk's limit and the roots
        // past it (their reach sets come from the labels: incomplete, never
        // "no modeled path").
        "reachLimit": REACH_LIMIT,
        "notExpanded": not_expanded,
        "byTier": {"local": by_tier(Tier::Local), "throughDependency": by_tier(Tier::ThroughDependency),
                   "onlyOpen": by_tier(Tier::OnlyOpen)},
        "universalHub": sel.universal,
        "hidden": hidden,
        "unresolvedTestCalls": unresolved.len(),
        "missingDependencies": missing,
        "limits": if stats.limited { vec!["the walk's state budget or cancellation stopped it"] } else { vec![] },
        "probes": args.probes.iter().map(|p| probe(g, p, &opts)).collect::<Vec<_>>(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::program::impact_graph::project::tests::{World, sited_of};
    use crate::program::impact_graph::{RoutineFacts, RunTarget};
    use crate::program::node_extract::{ObjectRef, PageControlKind, PageControlNode};
    use crate::snapshot::TrustTier;
    use al_syntax::ir::ObjectKind;

    /// A control add-in's event trigger is not reachable from a test (round-1
    /// c21): a test drives page `Ed` (a `TestPage` variable), whose `Body`
    /// add-in raises `EditorInitialized`, which calls `GetBody`; a `TestPage`
    /// hosts no add-in, so the selection walk from `GetBody` reaches no test.
    /// The page's action trigger, reached over the same page run, still is.
    /// Every fact is stated by hand.
    ///
    /// DISCRIMINATION (2026-11-06): removing `Sel::step` (the default follows
    /// every site) fails "no test reaches GetBody"; projecting the add-in
    /// trigger with `via: "lifecycle"` (the `usercontrol_names` check made
    /// false) fails the precondition.
    #[test]
    fn a_test_does_not_reach_a_control_add_in_event() {
        let mut w = World::default();
        let a = w.app("A");
        let mgt = w.obj(a, ObjectKind::Codeunit, 1, "Mgt", TrustTier::Workspace);
        let page = w.obj(a, ObjectKind::Page, 2, "Ed", TrustTier::Workspace);
        let tests = w.obj(a, ObjectKind::Codeunit, 3, "Tests", TrustTier::Workspace);
        w.object_mut(&page).page_controls = vec![PageControlNode {
            name_lc: "body".into(),
            kind: PageControlKind::UserControl,
            target: ObjectRef::Name {
                raw: "CDO.Editor".into(),
                normalized_lc: "cdo.editor".into(),
            },
        }];
        let get_body = w.rtn(&mgt, "GetBody");
        let send = w.rtn(&mgt, "Send");
        let init = w.rtn_with(&page, "EditorInitialized", |r| {
            r.is_trigger = true;
            r.id.enclosing_member_lc = Some("body".into());
        });
        let action = w.rtn_with(&page, "OnAction", |r| {
            r.is_trigger = true;
            r.id.enclosing_member_lc = Some("send".into());
        });
        w.call(&init, &get_body, 1);
        w.call(&action, &send, 2);
        let t = w.rtn(&tests, "TestIt");
        w.facts.routines.insert(
            t.clone(),
            RoutineFacts {
                test: true,
                test_pages: vec![RunTarget::Object(page.clone())],
                ..Default::default()
            },
        );
        let g = sited_of(&w, a, &[]);
        let ix = |r| g.routine_node(r).expect("a node");
        let into_init: Vec<&str> = g
            .sites_into(ix(&init))
            .iter()
            .map(|&s| g.sites()[s as usize].via())
            .collect();
        assert_eq!(
            into_init,
            vec!["clientEvent"],
            "precondition: the page run's site"
        );
        let tests_from = |root| {
            let mut sel = Sel::new(&g, false);
            walk_multi(
                &g,
                &[root],
                &WalkOptions::default(),
                &mut WalkScratch::default(),
                &mut sel,
            );
            sel.tests.keys().copied().collect::<Vec<_>>()
        };
        assert_eq!(tests_from(ix(&send)), vec![ix(&t)], "the action is reached");
        assert!(
            tests_from(ix(&get_body)).is_empty(),
            "no test reaches GetBody"
        );
    }

    /// Round-3 c21: a test lists EVERY changed routine it reaches, not only
    /// the root of its shortest path; and a changed routine's nearest test
    /// is a code test when one reaches it, even when a UI-navigation test is
    /// fewer hops away. Hand-stated program: `Both` calls `R1`, which calls
    /// `R2` (so `Both`'s shortest path ends at R1, and it reaches R2 too);
    /// `Nav` opens page `Ed` (a `TestPage`), whose action calls `Send`
    /// (3 hops, UI navigation); `Code` calls `H1 → H2 → H3 → Send` (4 hops,
    /// code).
    ///
    /// DISCRIMINATION (2026-11-07): (1) `reachesAll` built from the label
    /// only (the per-root `reach_all.entry(..).push(..)` loop removed) fails
    /// "Both reaches R2"; (2) the nearest taken as the first in (tier, hops)
    /// order, the relation class dropped from its key, fails "Send's nearest is
    /// the code test" (it names `Nav`). Each restored, the test passes.
    #[test]
    fn a_test_lists_every_changed_routine_and_nearest_prefers_code() {
        let mut w = World::default();
        let a = w.app("A");
        let mgt = w.obj(a, ObjectKind::Codeunit, 1, "Mgt", TrustTier::Workspace);
        let page = w.obj(a, ObjectKind::Page, 2, "Ed", TrustTier::Workspace);
        let tests = w.obj(a, ObjectKind::Codeunit, 3, "Tests", TrustTier::Workspace);
        let r1 = w.rtn(&mgt, "R1");
        let r2 = w.rtn(&mgt, "R2");
        let send = w.rtn(&mgt, "Send");
        let h1 = w.rtn(&mgt, "H1");
        let h2 = w.rtn(&mgt, "H2");
        let h3 = w.rtn(&mgt, "H3");
        let action = w.rtn_with(&page, "OnAction", |r| {
            r.is_trigger = true;
            r.id.enclosing_member_lc = Some("go".into());
        });
        w.call(&r1, &r2, 1);
        w.call(&action, &send, 2);
        w.call(&h1, &h2, 3);
        w.call(&h2, &h3, 4);
        w.call(&h3, &send, 5);
        let both = w.rtn(&tests, "Both");
        let nav = w.rtn(&tests, "Nav");
        let code = w.rtn(&tests, "Code");
        w.call(&both, &r1, 6);
        w.call(&code, &h1, 7);
        for (t, pages) in [
            (&both, vec![]),
            (&nav, vec![RunTarget::Object(page.clone())]),
            (&code, vec![]),
        ] {
            w.facts.routines.insert(
                t.clone(),
                RoutineFacts {
                    test: true,
                    test_pages: pages,
                    ..Default::default()
                },
            );
        }
        let g = sited_of(&w, a, &[]);
        let ix = |r: &crate::program::node::RoutineNodeId| g.routine_node(r).expect("a node");
        // The roots' nodes, stated by hand (a hand-built graph has no
        // stable ids); their ids are labels only.
        let roots: Vec<String> = ["r1", "r2", "send"].map(String::from).to_vec();
        let mapped = vec![vec![ix(&r1)], vec![ix(&r2)], vec![ix(&send)]];
        let v = select_mapped(
            &g,
            &SelectArgs {
                roots: &roots,
                universe_key: "u",
                test_app_id: "t",
                production_app_id: "A-guid",
                production_stamp: None,
                declared: &[],
                probes: &[],
            },
            &mapped,
        );
        assert_eq!(v["reachesComplete"], true, "{v:#}");
        let row = |r: &crate::program::node::RoutineNodeId| {
            v["tests"]
                .as_array()
                .unwrap()
                .iter()
                .find(|t| t["display"] == display(&g, ix(r)))
                .cloned()
                .unwrap_or_else(|| panic!("a row: {v:#}"))
        };
        // Precondition: the walk's label is R1 only (its shortest path).
        let b = row(&both);
        assert_eq!(b["reaches"], json!(roots[0]), "{b:#}");
        let reached: Vec<&str> = b["reachesAll"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["stableId"].as_str().unwrap())
            .collect();
        assert_eq!(reached, [&roots[0], &roots[1]], "Both reaches R2: {b:#}");
        assert_eq!(b["reachesAll"][1]["hops"], 2, "{b:#}");
        // Precondition: the navigation test is nearer Send than the code one.
        let hops_to_send = |t: &Value| t["reachesAll"][0]["hops"].as_u64().unwrap();
        let (n, c) = (row(&nav), row(&code));
        assert!(hops_to_send(&n) < hops_to_send(&c), "{n:#} {c:#}");
        assert_eq!(n["reachesAll"][0]["relation"], "UI navigation", "{n:#}");
        assert_eq!(c["reachesAll"][0]["relation"], "code", "{c:#}");
        let send_root = &v["roots"][2];
        assert_eq!(
            send_root["nearest"],
            json!(display(&g, ix(&code))),
            "Send's nearest is the code test: {send_root:#}"
        );
        assert_eq!(send_root["nearestRelation"], "code", "{send_root:#}");
    }

    /// Output validation r3 T1: a test reaches a changed routine by `code`
    /// when ANY of its paths is code, with that path's hops; `UI navigation`
    /// only when every path opens a page. Hand-stated program: `Both` opens
    /// page `Ed` (a `TestPage`), whose action calls `Send` (the shorter path,
    /// UI navigation), and calls `H1 → H2 → H3 → Send` (4 hops, code); `Nav`
    /// only opens the page.
    ///
    /// DISCRIMINATION (2026-11-08): the per-root walk made without classes
    /// (`Sel::with_classes` replaced by `Sel::new(g, false)`, so a test keeps
    /// its shortest path's relation) fails "Both reaches Send by code".
    #[test]
    fn a_test_reaches_by_code_when_any_path_is_code() {
        let mut w = World::default();
        let a = w.app("A");
        let mgt = w.obj(a, ObjectKind::Codeunit, 1, "Mgt", TrustTier::Workspace);
        let page = w.obj(a, ObjectKind::Page, 2, "Ed", TrustTier::Workspace);
        let tests = w.obj(a, ObjectKind::Codeunit, 3, "Tests", TrustTier::Workspace);
        let send = w.rtn(&mgt, "Send");
        let h1 = w.rtn(&mgt, "H1");
        let h2 = w.rtn(&mgt, "H2");
        let h3 = w.rtn(&mgt, "H3");
        let action = w.rtn_with(&page, "OnAction", |r| {
            r.is_trigger = true;
            r.id.enclosing_member_lc = Some("go".into());
        });
        w.call(&action, &send, 1);
        w.call(&h1, &h2, 2);
        w.call(&h2, &h3, 3);
        w.call(&h3, &send, 4);
        let both = w.rtn(&tests, "Both");
        let nav = w.rtn(&tests, "Nav");
        w.call(&both, &h1, 5);
        for t in [&both, &nav] {
            w.facts.routines.insert(
                t.clone(),
                RoutineFacts {
                    test: true,
                    test_pages: vec![RunTarget::Object(page.clone())],
                    ..Default::default()
                },
            );
        }
        let g = sited_of(&w, a, &[]);
        let ix = |r: &crate::program::node::RoutineNodeId| g.routine_node(r).expect("a node");
        let roots = vec!["send".to_string()];
        let v = select_mapped(
            &g,
            &SelectArgs {
                roots: &roots,
                universe_key: "u",
                test_app_id: "t",
                production_app_id: "A-guid",
                production_stamp: None,
                declared: &[],
                probes: &[],
            },
            &[vec![ix(&send)]],
        );
        let row = |r: &crate::program::node::RoutineNodeId| {
            v["tests"]
                .as_array()
                .unwrap()
                .iter()
                .find(|t| t["display"] == display(&g, ix(r)))
                .cloned()
                .unwrap_or_else(|| panic!("a row: {v:#}"))
        };
        let b = row(&both);
        // Precondition: the shortest path (the printed one) opens the page.
        assert!(
            b["path"].as_array().unwrap().iter().any(|h| h
                .as_str()
                .is_some_and(super::super::render::is_navigation_hop)),
            "{b:#}"
        );
        assert_eq!(
            b["reachesAll"][0]["relation"], "code",
            "Both reaches Send by code: {b:#}"
        );
        assert_eq!(b["reachesAll"][0]["hops"], 4, "the code path's hops: {b:#}");
        let n = row(&nav);
        assert_eq!(n["reachesAll"][0]["relation"], "UI navigation", "{n:#}");
        assert!(n["reachesAll"][0].get("candidate").is_none(), "{n:#}");
        assert_eq!(
            v["roots"][0]["nearest"],
            json!(display(&g, ix(&both))),
            "{v:#}"
        );
        assert_eq!(v["roots"][0]["nearestRelation"], "code", "{v:#}");
    }

    /// Round-4 c28/c21: a test whose SHORTEST path opens a page but which
    /// also reaches the changed routine by code calls is a code test in
    /// every place that classifies a test: the row's `relation`, the
    /// ranking, `byRelation` and the headline (schemas 1-3 and the human
    /// text); its row shows a code path. Hand-stated program: `Both` opens
    /// page `Ed` (a `TestPage`), whose action calls `Send` (3 hops, the
    /// printed path), and calls `H1 → H2 → H3 → H4 → Send` (5 hops, code);
    /// `Code` calls `G1 → G2 → G3 → Send` (4 hops, code only); `Nav` only
    /// opens the page.
    ///
    /// DISCRIMINATION (2026-11-08): (1) the render's `relation` made the
    /// printed path's relation again (`path_relation(t)` returned) fails
    /// "Both is a code test" (byRelation code 1, UI navigation 2); (2) the
    /// row's `codePath` not written (the `code_path.get(&n)` block removed)
    /// fails "Both carries its code path"; (3) the ranking by the row's own
    /// tier and hops (not the shown path's) fails "Code ranks before Both"
    /// (Both's printed path has 3 hops, Code's 4). Each restored, it passes.
    #[test]
    fn a_test_with_any_code_path_is_a_code_test_everywhere() {
        use super::super::render::{Part, RenderOpts, TierPick, render};
        let mut w = World::default();
        let a = w.app("A");
        let mgt = w.obj(a, ObjectKind::Codeunit, 1, "Mgt", TrustTier::Workspace);
        let page = w.obj(a, ObjectKind::Page, 2, "Ed", TrustTier::Workspace);
        let tests = w.obj(a, ObjectKind::Codeunit, 3, "Tests", TrustTier::Workspace);
        let nav_tests = w.obj(
            a,
            ObjectKind::Codeunit,
            4,
            "Nav Tests",
            TrustTier::Workspace,
        );
        let code_tests = w.obj(
            a,
            ObjectKind::Codeunit,
            5,
            "Code Tests",
            TrustTier::Workspace,
        );
        let send = w.rtn(&mgt, "Send");
        let hs: Vec<_> = (1..=4).map(|i| w.rtn(&mgt, &format!("H{i}"))).collect();
        let gs: Vec<_> = (1..=3).map(|i| w.rtn(&mgt, &format!("G{i}"))).collect();
        let action = w.rtn_with(&page, "OnAction", |r| {
            r.is_trigger = true;
            r.id.enclosing_member_lc = Some("go".into());
        });
        let mut line = 0;
        let mut call = |w: &mut World, f, t| {
            line += 1;
            w.call(f, t, line);
        };
        call(&mut w, &action, &send);
        for p in hs.windows(2) {
            call(&mut w, &p[0], &p[1]);
        }
        call(&mut w, &hs[3], &send);
        for p in gs.windows(2) {
            call(&mut w, &p[0], &p[1]);
        }
        call(&mut w, &gs[2], &send);
        let both = w.rtn(&tests, "Both");
        let nav = w.rtn(&nav_tests, "Nav");
        let code = w.rtn(&code_tests, "Code");
        call(&mut w, &both, &hs[0]);
        call(&mut w, &code, &gs[0]);
        for (t, pages) in [
            (&both, vec![RunTarget::Object(page.clone())]),
            (&nav, vec![RunTarget::Object(page.clone())]),
            (&code, vec![]),
        ] {
            w.facts.routines.insert(
                t.clone(),
                RoutineFacts {
                    test: true,
                    test_pages: pages,
                    ..Default::default()
                },
            );
        }
        let g = sited_of(&w, a, &[]);
        let ix = |r: &crate::program::node::RoutineNodeId| g.routine_node(r).expect("a node");
        let roots = vec!["send".to_string()];
        let v = select_mapped(
            &g,
            &SelectArgs {
                roots: &roots,
                universe_key: "u",
                test_app_id: "t",
                production_app_id: "A-guid",
                production_stamp: None,
                declared: &[],
                probes: &[],
            },
            &[vec![ix(&send)]],
        );
        let row = |r: &crate::program::node::RoutineNodeId| {
            v["tests"]
                .as_array()
                .unwrap()
                .iter()
                .find(|t| t["display"] == display(&g, ix(r)))
                .cloned()
                .unwrap_or_else(|| panic!("a row: {v:#}"))
        };
        let b = row(&both);
        // Preconditions: Both's printed path opens the page and is shorter
        // than Code's; its reach of Send is code, 5 hops.
        assert!(
            b["path"].as_array().unwrap().iter().any(|h| h
                .as_str()
                .is_some_and(super::super::render::is_navigation_hop)),
            "{b:#}"
        );
        assert!(b["hops"].as_u64() < row(&code)["hops"].as_u64(), "{b:#}");
        assert_eq!(b["reachesAll"][0]["relation"], "code", "{b:#}");
        assert_eq!(b["reachesAll"][0]["hops"], 5, "{b:#}");
        let code_path: Vec<String> = [&both, &hs[0], &hs[1], &hs[2], &hs[3], &send]
            .iter()
            .map(|r| display(&g, ix(r)))
            .collect();
        assert_eq!(
            b["codePath"],
            json!(code_path),
            "Both carries its code path: {b:#}"
        );
        assert_eq!(b["codePathHops"], 5, "{b:#}");
        assert!(row(&nav).get("codePath").is_none(), "Nav has none");
        assert!(row(&code).get("codePath").is_none(), "Code's path is code");

        let main = json!({
            "roots": [{"stableId": "send", "display": "Codeunit Mgt::Send"}],
            "scope": {"apps": [], "summary": "none", "complete": true},
            "workspaceTests": 0,
        });
        let part = || Part {
            app: json!({"name": "T", "root": "t"}),
            result: Ok(v.clone()),
        };
        let opts = |json: bool, schema: u32| RenderOpts {
            tier: TierPick::All,
            max_tests: None,
            json,
            surface: Default::default(),
            single_app: false,
            schema,
        };
        let names = |d: &Value| -> Vec<String> {
            d["runFirst"]
                .as_array()
                .unwrap()
                .iter()
                .map(|r| r["procedure"].as_str().unwrap().to_string())
                .collect()
        };
        for schema in [1, 2, 3] {
            let d: Value =
                serde_json::from_str(&render(&main, &[part()], opts(true, schema)).text).unwrap();
            assert_eq!(
                d["result"]["selection"]["byRelation"],
                json!({"code": 2, "UI navigation": 1}),
                "Both is a code test (schema {schema}): {d:#}"
            );
            assert_eq!(
                names(&d),
                ["Code", "Both", "Nav"],
                "Code ranks before Both (schema {schema}): {d:#}"
            );
            let rb = &d["runFirst"][1];
            assert_eq!(rb["relation"], "code", "{d:#}");
            assert_eq!(rb["pathRelation"], "UI navigation", "{d:#}");
            assert_eq!(rb["codePath"], json!(code_path), "{d:#}");
            assert!(rb.get("reachesByNavigationOnly").is_none(), "{d:#}");
            assert_eq!(d["runFirst"][2]["relation"], "UI navigation", "{d:#}");
            assert!(d["runFirst"][0].get("pathRelation").is_none(), "{d:#}");
        }
        let human = render(&main, &[part()], opts(false, 3)).text;
        assert!(
            human.contains("2 tests by code calls (codeunits 5|3;")
                && human.contains("1 more by UI navigation only in 1 codeunit"),
            "{human}"
        );
        assert!(
            human.contains("by relation: code 2, UI navigation 1"),
            "{human}"
        );
        assert!(
            human.contains("first [local, code] 5 hops (first: Both)"),
            "{human}"
        );
        assert!(
            human.contains(&format!("path: {}", code_path[1..].join(" → "))),
            "the code path is printed: {human}"
        );
    }

    /// Round-4 n02: a test whose only code path lies outside the answer's
    /// tier pick is not counted as reached "by code calls" there. Hand-stated
    /// program: `Both` opens page `Ed` (a `TestPage`), whose action calls
    /// `Send` (a local UI-navigation path), and calls `Dep::Run` in
    /// dependency app `D`, which calls `Send` (a via-dependency code path).
    /// Under `--tier local` it is a UI-navigation test with no code path
    /// shown; under `--tier dependency` a code test with that code path.
    ///
    /// DISCRIMINATION (2026-11-08): `on_tier` made a no-op (its body
    /// removed) fails "a via-dependency code path does not make a local code
    /// test" (byRelation code 1 under the local pick); the select part's
    /// `navigation` alternative not written (the `if let Some((ac, at, ah))
    /// = alt` block removed) fails the same assert. Each restored, it passes.
    #[test]
    fn a_code_path_outside_the_tier_pick_does_not_make_a_code_test() {
        use super::super::render::{Part, RenderOpts, TierPick, render};
        let mut w = World::default();
        let a = w.app("A");
        let d = w.app("D");
        let mgt = w.obj(a, ObjectKind::Codeunit, 1, "Mgt", TrustTier::Workspace);
        let page = w.obj(a, ObjectKind::Page, 2, "Ed", TrustTier::Workspace);
        let tests = w.obj(a, ObjectKind::Codeunit, 3, "Tests", TrustTier::Workspace);
        let dep = w.obj(d, ObjectKind::Codeunit, 9, "Dep", TrustTier::EmbeddedSource);
        let send = w.rtn(&mgt, "Send");
        let run = w.rtn(&dep, "Run");
        let action = w.rtn_with(&page, "OnAction", |r| {
            r.is_trigger = true;
            r.id.enclosing_member_lc = Some("go".into());
        });
        w.call(&action, &send, 1);
        w.call(&run, &send, 2);
        let both = w.rtn(&tests, "Both");
        w.call(&both, &run, 3);
        w.facts.routines.insert(
            both.clone(),
            RoutineFacts {
                test: true,
                test_pages: vec![RunTarget::Object(page.clone())],
                ..Default::default()
            },
        );
        let g = sited_of(&w, a, &[]);
        let ix = |r: &crate::program::node::RoutineNodeId| g.routine_node(r).expect("a node");
        let roots = vec!["send".to_string()];
        let v = select_mapped(
            &g,
            &SelectArgs {
                roots: &roots,
                universe_key: "u",
                test_app_id: "t",
                production_app_id: "A-guid",
                production_stamp: None,
                declared: &[],
                probes: &[],
            },
            &[vec![ix(&send)]],
        );
        let b = &v["tests"][0];
        // Preconditions: the printed path is local and opens the page; the
        // reach is code through the dependency, a local UI one beside it.
        assert_eq!(b["tier"], "local", "{v:#}");
        assert!(
            b["path"].as_array().unwrap().iter().any(|h| h
                .as_str()
                .is_some_and(super::super::render::is_navigation_hop)),
            "{b:#}"
        );
        assert_eq!(b["reachesAll"][0]["relation"], "code", "{b:#}");
        assert_eq!(b["reachesAll"][0]["tier"], "throughDependency", "{b:#}");
        assert_eq!(b["codePathTier"], "throughDependency", "{b:#}");
        let main = json!({
            "roots": [{"stableId": "send", "display": "Codeunit Mgt::Send"}],
            "scope": {"apps": [], "summary": "none", "complete": true},
            "workspaceTests": 0,
        });
        let answer = |tier: TierPick| -> Value {
            let part = Part {
                app: json!({"name": "T", "root": "t"}),
                result: Ok(v.clone()),
            };
            let opts = RenderOpts {
                tier,
                max_tests: None,
                json: true,
                surface: Default::default(),
                single_app: false,
                schema: 3,
            };
            serde_json::from_str(&render(&main, &[part], opts).text).unwrap()
        };
        let local = answer(TierPick::Local);
        assert_eq!(
            local["result"]["selection"]["byRelation"],
            json!({"code": 0, "UI navigation": 1}),
            "a via-dependency code path does not make a local code test: {local:#}"
        );
        let row = &local["runFirst"][0];
        assert_eq!(row["relation"], "UI navigation", "{local:#}");
        assert_eq!(row["reaches"][0]["relation"], "UI navigation", "{local:#}");
        assert!(row.get("codePath").is_none(), "{local:#}");
        let dep_pick = answer(TierPick::Dependency);
        assert_eq!(
            dep_pick["result"]["selection"]["byRelation"],
            json!({"code": 1, "UI navigation": 0}),
            "{dep_pick:#}"
        );
        let row = &dep_pick["runFirst"][0];
        assert_eq!(row["reaches"][0]["relation"], "code", "{dep_pick:#}");
        assert_eq!(row["codePathTier"], "via-dependency", "{dep_pick:#}");
    }

    /// The program of the reach-limit tests: `n` routines `R0..`, each
    /// called by one test `T` (so every root is one hop from it), and the
    /// roots' ids and nodes.
    fn many_roots(n: usize) -> (SitedGraph, Vec<String>, Vec<Vec<NodeIx>>) {
        let mut w = World::default();
        let a = w.app("A");
        let mgt = w.obj(a, ObjectKind::Codeunit, 1, "Mgt", TrustTier::Workspace);
        let tests = w.obj(a, ObjectKind::Codeunit, 3, "Tests", TrustTier::Workspace);
        let t = w.rtn(&tests, "T");
        let rs: Vec<_> = (0..n).map(|i| w.rtn(&mgt, &format!("R{i}"))).collect();
        for (i, r) in rs.iter().enumerate() {
            w.call(&t, r, i as u32 + 1);
        }
        w.facts.routines.insert(
            t.clone(),
            RoutineFacts {
                test: true,
                ..Default::default()
            },
        );
        let g = sited_of(&w, a, &[]);
        let mapped = rs
            .iter()
            .map(|r| vec![g.routine_node(r).expect("a node")])
            .collect();
        let ids = (0..n).map(|i| format!("r{i}")).collect();
        (g, ids, mapped)
    }

    fn select_many(n: usize) -> Value {
        let (g, roots, mapped) = many_roots(n);
        select_mapped(
            &g,
            &SelectArgs {
                roots: &roots,
                universe_key: "u",
                test_app_id: "t",
                production_app_id: "A-guid",
                production_stamp: None,
                declared: &[],
                probes: &[],
            },
            &mapped,
        )
    }

    /// Output validation r3 L4, at the limit: every changed routine has a
    /// walk of its own, so the one test's reach set lists all of them and
    /// nothing is named as not expanded.
    #[test]
    fn at_the_reach_limit_every_routine_is_expanded() {
        let v = select_many(REACH_LIMIT);
        assert_eq!(v["reachesComplete"], true, "{}", v["notExpanded"]);
        assert_eq!(v["notExpanded"], json!([]));
        assert_eq!(v["reachLimit"], REACH_LIMIT);
        assert_eq!(
            v["tests"][0]["reachesAll"].as_array().unwrap().len(),
            REACH_LIMIT
        );
    }

    /// Output validation r3 L4, at the limit plus one: the last routine is
    /// named as not expanded and the reach set is incomplete, yet its status
    /// is `reached` (a test reaches it), never "no modeled path".
    ///
    /// DISCRIMINATION (2026-11-08): (1) expanding every root again (`expand`
    /// set `true`) fails "the last routine is not expanded"; (2) reporting a
    /// not-expanded root with no label test as `no-known-path` (the status
    /// walk's `found_test` check dropped) fails "reached, not no modeled
    /// path".
    #[test]
    fn past_the_reach_limit_the_rest_are_named_not_expanded() {
        let v = select_many(REACH_LIMIT + 1);
        let last = format!("r{REACH_LIMIT}");
        assert_eq!(v["reachesComplete"], false, "{}", v["notExpanded"]);
        assert_eq!(
            v["notExpanded"],
            json!([last]),
            "the last routine is not expanded"
        );
        let root = &v["roots"][REACH_LIMIT];
        assert_eq!(root["stableId"], json!(last));
        // Precondition: the test's label is another root, so the last one has
        // no label test of its own.
        assert_ne!(v["tests"][0]["reaches"], json!(last), "{:#}", v["tests"][0]);
        assert_eq!(
            root["status"], "reached",
            "reached, not no modeled path: {root:#}"
        );
        assert_eq!(
            v["tests"][0]["reachesAll"].as_array().unwrap().len(),
            REACH_LIMIT
        );
    }
}

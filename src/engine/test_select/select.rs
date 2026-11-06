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
//! program is an edge into the event. The sited graph has no such edge (a
//! record write routes to the table's declared triggers only; the platform
//! publishers are reached through the record hub alone), so no table event
//! can be shown covered, and the owner's rule applies: one that cannot be
//! shown covered counts as relevant. The class is printed, with that reason,
//! so the gap is visible rather than folded away.
//!
//! # Dependency-tier sameness (§4.5, Fable f6)
//!
//! A root's through-dependency tests each reach it via a first dependency
//! node on the path (from the root's side: the dependency code that calls
//! back into local code, e.g. a Base Application page). When every such
//! node's own cone, what it may execute (forward, resolved sites only),
//! covers more than half of the graph's nodes, those tests reach nearly
//! everything: `sameness` is set and the answer says they are effectively the
//! suite's posting tests, not specific to this change. With more than
//! [`SAMENESS_ENTRIES`] such nodes the check is not made (`viaMeasured:
//! false`).

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use serde_json::{Value, json};

use crate::program::impact_graph::reverse::{
    ConeVisitor, HiddenChannel, NodeIx, Tier, Via, WalkOptions, WalkScratch, WalkState, walk_multi,
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
/// own (exact per root); with more, from the multi-source walk's nearest-root
/// labels (one walk; a root's tests that are nearer another root are counted
/// under that root).
const PER_ROOT_WALKS: usize = 32;

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

/// A node's printed name: the source's spelling for a local routine
/// (`Codeunit CDO Telemetry::UpdateLastUpload`), the graph's otherwise.
#[must_use]
pub fn display(g: &SitedGraph, n: NodeIx) -> String {
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

/// A path hop's name (agent-tasks §12.1, output validation F9): [`display`]
/// with the routine's overload handle, so two overloads on one path never
/// print alike.
fn path_name(g: &SitedGraph, n: NodeIx) -> String {
    let handle = g.local_name(n).map_or("", |l| &*l.handle);
    format!("{}{handle}", display(g, n))
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
            "a platform-raised table event: covered only when every write of the table is an \
             edge into it, and the graph has no such edge (writes route to declared triggers \
             only), so it is not shown covered",
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

fn class_of(s: &WalkState) -> &'static str {
    if s.tier == Tier::OnlyOpen {
        "dynamic"
    } else if s.crossed_event {
        "event"
    } else {
        "direct"
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
/// hubs, and a page's or page extension's triggers (page actions are counted
/// apart, explain's F16j).
#[must_use]
pub fn direct_callers(g: &SitedGraph, r: NodeIx) -> BTreeSet<NodeIx> {
    use crate::program::impact_graph::sited::Boundary;
    // A page's or page extension's trigger (explain's F16j: `object_type`
    // starting with `Page`).
    let is_page_trigger = |n: NodeIx| {
        g.routine_object(n).is_some_and(|o| {
            matches!(
                o.kind,
                al_syntax::ir::ObjectKind::Page | al_syntax::ir::ObjectKind::PageExtension
            )
        }) && g.local_name(n).is_some_and(|l| l.trigger)
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
    let primary = g.primary_app();
    let tests_loaded = (0..g.node_count() as u32)
        .filter(|&n| g.flags(n) & flag::TEST != 0 && g.node_app(n) == Some(primary))
        .count();
    // Each root's nodes, and the walk's roots (node -> root index).
    let mut mapped: Vec<Vec<NodeIx>> = Vec::with_capacity(args.roots.len());
    let mut walk_roots: Vec<NodeIx> = Vec::new();
    let mut label_root: Vec<usize> = Vec::new();
    for (i, id) in args.roots.iter().enumerate() {
        let nodes: Vec<NodeIx> = g
            .nodes_with_stable_id(id)
            .into_iter()
            .filter(|&n| g.flags(n) & flag::SYMBOL_ONLY == 0)
            .collect();
        for &n in &nodes {
            walk_roots.push(n);
            label_root.push(i);
        }
        mapped.push(nodes);
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
    tests.sort_by_key(|a| (a.1, a.2, key(a.0)));
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
        // The root's own tests: from its own walk when there are few roots
        // (the multi-source walk labels a state with its NEAREST root, so a
        // root's tests nearer another root would be counted there), else
        // from the labels.
        let few = args.roots.len() <= PER_ROOT_WALKS;
        let mut own_walk: Option<Sel> = None;
        let mine: Vec<(NodeIx, Tier, u32, WalkState)> = if few {
            let mut own = Sel::new(g, false);
            let mut s2 = WalkScratch::default();
            walk_multi(g, &mapped[i], &opts, &mut s2, &mut own);
            let mut v: Vec<(NodeIx, Tier, u32, WalkState)> = own
                .tests
                .iter()
                .map(|(&n, &(t, h, s, _))| (n, t, h, s))
                .collect();
            v.sort_by_key(|a| (a.1, a.2, key(a.0)));
            own_walk = Some(own);
            v
        } else {
            per_root[i]
                .iter()
                .map(|&j| (tests[j].0, tests[j].1, tests[j].2, tests[j].3))
                .collect()
        };
        let count = |t: Tier| mine.iter().filter(|x| x.1 == t).count();
        let status = if !mine.is_empty() {
            "reached"
        } else if let Some(own) = &own_walk {
            if own.limited || own.relevant_hidden(g) {
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
        if let Some(n) = mine.first() {
            r["nearest"] = json!(display(g, n.0));
            r["nearestTier"] = json!(tier_name(n.1));
            r["nearestHops"] = json!(n.2);
        }
        // Dependency-tier sameness (§4.5).
        let thr: Vec<&(NodeIx, Tier, u32, WalkState)> = mine
            .iter()
            .filter(|x| x.1 == Tier::ThroughDependency)
            .collect();
        if !thr.is_empty() {
            let paths = own_walk.as_ref().unwrap_or(&sel);
            let mut via: BTreeMap<NodeIx, usize> = BTreeMap::new();
            for t in &thr {
                let path = paths.path(t.3);
                // The first dependency node from the root's side.
                if let Some(&d) = path.iter().rev().find(|&&n| g.flags(n) & flag::LOCAL == 0) {
                    *via.entry(d).or_default() += 1;
                }
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
                    let c = fwd.cone(d);
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
            json!({
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
                "path": path.iter().map(|&p| path_name(g, p)).collect::<Vec<_>>(),
            })
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

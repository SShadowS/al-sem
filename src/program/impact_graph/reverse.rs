//! **Reverse walks** over the sited graph (agent-tasks design §3, milestone
//! R0): "who reaches this routine". The one up-walker that test selection
//! (C), "what context does R run in" (A) and impact (B1) share.
//!
//! # What a walk computes
//!
//! A breadth-first walk over the sited graph's reversed sites
//! ([`SitedGraph::sites_into`]), from one root ([`walk_one`]) or from many at
//! once ([`walk_multi`]). It keeps the shortest hop count of every **state**
//! `(node, tier, crossedEvent, ext)` ([`WalkState`]), not of every node, so a
//! node reached by a local path and by a shorter path through dependency code
//! has a state for each. Each state is handed to the [`ConeVisitor`] once,
//! when it is first reached, with the site it was reached by and the state it
//! was reached from; the walk itself keeps no parent, no path and nothing per
//! root (Fable f10: 699 roots × ~22,000 nodes must not be 15 million retained
//! entries). A visitor that wants a path keeps the parents it needs.
//!
//! - **Tier** ([`Tier`]): `Local` while every node of the path is in the
//!   workspace app or an app loaded from local source ([`flag::LOCAL`]),
//!   `ThroughDependency` once the path passes other code, `OnlyOpen` once it
//!   uses an `Open` site (a hub). A tier only gets worse along a path.
//! - **crossedEvent**: the path passed an event dispatch (publisher to
//!   subscriber).
//! - **ext**: the consumer's own bits, set by [`ConeVisitor::step`] per site
//!   (A: "a commit was passed", "inside a trap", "past an async boundary").
//!   The step is also where a consumer refines a `Run` or `TriggerOf` site by
//!   its anchor (checked or unchecked `Codeunit.Run`, a background start, a
//!   trigger's temporary state): the sited graph carries the boundary the
//!   program edges state ([`super::sited::Boundary`]), not those body facts.
//! - **Order**: level by level; a level is expanded in ascending (root label,
//!   state) order and each state's reversed sites in ascending site id, so
//!   the walk is deterministic and a state reached from several roots at the
//!   same hop count is labelled with the first root.
//! - **The universal hub** (`h:any`) stands for "a caller of every node". When
//!   an edge names it, the walk reaches it once, one hop from the roots, as
//!   an `OnlyOpen` state ([`Via::Universal`]), reports it once as the hidden
//!   channel [`HiddenChannel::Universal`], and walks its callers once from
//!   there. They are never dropped, and never walked per node.
//! - **Entries** ([`EntryKind`], §3.3) come from what the source declares
//!   (the model's root classifier over the IR, `root_classification::
//!   root_kind_mask`), never from the graph having no caller there: a page
//!   action reached by a lifecycle site is still a page-action entry. A
//!   page's action `RunObject` site makes the page an entry too. Graph
//!   sources (nodes with no reversed site) are reported apart
//!   ([`ConeVisitor::source`]) and are not entries by themselves.
//! - **External callers** (§3.5): a node of the workspace (or a local-source
//!   app) that apps outside the program may call ([`ExternalWhy`]): public
//!   access on an object that is not internal, `protected` (an extension in
//!   another app), or `internal` when the app names an `internalsVisibleTo`
//!   friend. A public procedure with internal callers is still external.
//!   [`WalkOptions::assume_no_dependents`] (CLI `--no-dependents`, MCP
//!   `assumeNoDependents`) turns these off; it is its own flag, not
//!   `closedWorld` (interface dispatch), and neither closes unknown platform
//!   scheduling.
//! - **Counts are of entries and sites**, never of paths.
//! - **Budget**: a walk stops at [`WalkOptions::state_budget`] states (default
//!   2,000,000) or when its cancellation says so (polled every 4,096 states),
//!   and says which ([`Limit`]); a consumer must then not answer `no`.
//!
//! # What it does not know (yet)
//!
//! The roots.config overlay kinds (`web-service-exposed`,
//! `job-queue-entrypoint`) are not in the source; a consumer with a model
//! passes them as [`WalkOptions::extra_entries`] ([`config_entries`]).
//! `background-start` comes from the graph (F3): the node a
//! [`Boundary::Background`] site enters (the codeunit object a static
//! `StartSession`/`TaskScheduler.CreateTask`/`CurrPage.EnqueueBackgroundTask`
//! starts) is that entry. The walk still goes on over the start site (R0
//! does not stop at a boundary; a consumer's [`ConeVisitor::step`] does). A
//! start whose codeunit is not a literal has no such site. A symbol-only
//! routine has no source, so no entry kinds and no stable id.

use std::collections::{HashMap, HashSet};

use super::sited::{Boundary, SitedGraph, flag};
use crate::engine::root_classification::{root_kind_bit, root_kinds_of_mask};

/// A node of the sited graph (its index).
pub type NodeIx = u32;
/// A site of the sited graph (its index).
pub type SiteIx = u32;

/// How a path reaches a state, best first ([`Ord`]: `min` is the best).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Tier {
    /// Every node of the path is local ([`flag::LOCAL`]).
    Local,
    /// The path passes dependency code.
    ThroughDependency,
    /// The path uses an `Open` site (a hub).
    OnlyOpen,
}

impl Tier {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Tier::Local => "local",
            Tier::ThroughDependency => "throughDependency",
            Tier::OnlyOpen => "onlyOpen",
        }
    }
}

/// One state of a walk (see the module doc).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct WalkState {
    pub node: NodeIx,
    pub tier: Tier,
    pub crossed_event: bool,
    /// The consumer's bits ([`ConeVisitor::step`]); 0 unless it sets them.
    pub ext: u8,
    /// The walk's own table-event bits ([`crate::program::table_event_guard`]):
    /// on a platform publisher reached backwards from a subscriber, what that
    /// subscriber exits on first thing ([`super::sited::Site::guard`]); 0
    /// everywhere else. A raising site that proves one of them (its record
    /// temporary, RunTrigger false; [`super::sited::Site::guard`]) is not
    /// crossed: it runs the subscriber's guard only. A publisher reached from
    /// two subscribers with different guards is two states, so a path never
    /// passes a raising site through a subscriber that site cannot run.
    pub guard: u8,
}

/// How a state was first reached.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Via {
    /// It is root number `.0` (an index into the walk's roots).
    Root(u32),
    /// Backwards over this site, from the state given beside it.
    Site(SiteIx),
    /// The universal hub, a caller of every node.
    Universal,
}

/// A way a node in the cone may run that no site shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum HiddenChannel {
    /// The universal hub: callers of every node.
    Universal,
    /// A node's `hiddenCallers` flag (`unraisedPublisher`,
    /// `unboundSubscriber`, `unmodeledRecordTrigger`).
    Flag(&'static str),
}

/// What a source declares as a way to start a run (§3.3), in print order:
/// "a user or a job will hit this" first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum EntryKind {
    PageAction,
    ApiPage,
    WebServiceExposed,
    JobQueueEntrypoint,
    BackgroundStart,
    ReportTrigger,
    TriggerPage,
    TriggerTable,
    EventSubscriber,
    InstallCodeunit,
    UpgradeCodeunit,
    OnrunCodeunit,
    TestProcedure,
}

impl EntryKind {
    /// Every kind, in print order.
    pub const ALL: [EntryKind; 13] = [
        EntryKind::PageAction,
        EntryKind::ApiPage,
        EntryKind::WebServiceExposed,
        EntryKind::JobQueueEntrypoint,
        EntryKind::BackgroundStart,
        EntryKind::ReportTrigger,
        EntryKind::TriggerPage,
        EntryKind::TriggerTable,
        EntryKind::EventSubscriber,
        EntryKind::InstallCodeunit,
        EntryKind::UpgradeCodeunit,
        EntryKind::OnrunCodeunit,
        EntryKind::TestProcedure,
    ];

    /// The kind's name: the root classifier's for the twelve it shares.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            EntryKind::PageAction => "page-action",
            EntryKind::ApiPage => "api-page",
            EntryKind::WebServiceExposed => "web-service-exposed",
            EntryKind::JobQueueEntrypoint => "job-queue-entrypoint",
            EntryKind::BackgroundStart => "background-start",
            EntryKind::ReportTrigger => "report-trigger",
            EntryKind::TriggerPage => "trigger-page",
            EntryKind::TriggerTable => "trigger-table",
            EntryKind::EventSubscriber => "event-subscriber",
            EntryKind::InstallCodeunit => "install-codeunit",
            EntryKind::UpgradeCodeunit => "upgrade-codeunit",
            EntryKind::OnrunCodeunit => "onrun-codeunit",
            EntryKind::TestProcedure => "test-procedure",
        }
    }

    /// The entry kinds of a root-kind mask, in print order. `public-procedure`
    /// is not an entry: it is an external caller ([`ExternalWhy`]).
    #[must_use]
    pub fn of_root_kinds(mask: u16) -> Vec<EntryKind> {
        let names: Vec<&str> = root_kinds_of_mask(mask).collect();
        EntryKind::ALL
            .into_iter()
            .filter(|k| names.contains(&k.as_str()))
            .collect()
    }
}

/// Why apps outside the program may call a node (§3.5).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ExternalWhy {
    /// Public access on an object that is not `Access = Internal`.
    Public,
    /// `protected`: an extension object in another app may call it.
    Protected,
    /// `internal` (or public on an internal object), and the app names an
    /// `internalsVisibleTo` friend.
    Friend,
}

impl ExternalWhy {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            ExternalWhy::Public => "public",
            ExternalWhy::Protected => "protected",
            ExternalWhy::Friend => "friend",
        }
    }
}

/// A cap that stopped a walk. A consumer must not answer `no` after one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Limit {
    /// The state budget was reached.
    StateBudget(usize),
    /// The walk's cancellation said stop.
    Cancelled,
}

/// What a walk reports (see the module doc). Every method has an empty
/// default; `root` is the index of the state's nearest root (0 for
/// [`walk_one`]).
pub trait ConeVisitor {
    /// A state, the first time it is reached (at its shortest hop count).
    /// `from` is the state it was reached from (`None` for a root and for
    /// the universal hub's state).
    fn reach(&mut self, root: u32, s: WalkState, hops: u32, via: Via, from: Option<WalkState>) {
        let _ = (root, s, hops, via, from);
    }
    /// A node in the cone with a way to run no site shows (once per node).
    fn hidden(&mut self, root: u32, n: NodeIx, ch: HiddenChannel) {
        let _ = (root, n, ch);
    }
    /// A state whose node has an entry kind (once per state; a page reached
    /// over one of its actions' `RunObject` sites is a `PageAction` entry).
    fn entry(&mut self, root: u32, n: NodeIx, kinds: &[EntryKind], s: WalkState, hops: u32) {
        let _ = (root, n, kinds, s, hops);
    }
    /// A state whose node apps outside the program may call (once per state).
    fn external(&mut self, root: u32, n: NodeIx, why: ExternalWhy, s: WalkState, hops: u32) {
        let _ = (root, n, why, s, hops);
    }
    /// A state whose node has no reversed site (a graph source).
    fn source(&mut self, root: u32, s: WalkState, hops: u32) {
        let _ = (root, s, hops);
    }
    /// A cap fired; the walk stops after this call.
    fn limit(&mut self, l: Limit) {
        let _ = l;
    }
    /// The consumer bits of the state reached backwards over `site` from
    /// `from`, or `None` to not follow that site. The default keeps the bits.
    /// A walk calls it for `from`'s reversed sites right after its
    /// [`Self::reach`] of `from`, so a visitor may read `from`'s hop count
    /// from that call.
    fn step(&mut self, g: &SitedGraph, from: WalkState, site: SiteIx) -> Option<u8> {
        let _ = (g, site);
        Some(from.ext)
    }
}

/// How a walk runs.
#[derive(Clone, Copy)]
pub struct WalkOptions<'a> {
    /// Follow `Open` sites and the universal hub (default `true`). `false`
    /// walks only resolved and over-approximated sites.
    pub follow_open: bool,
    /// No app outside the program depends on these apps: report no external
    /// callers.
    pub assume_no_dependents: bool,
    /// The most states a walk keeps.
    pub state_budget: usize,
    /// Polled every 4,096 states; `true` stops the walk ([`Limit::Cancelled`]).
    pub cancel: Option<&'a dyn Fn() -> bool>,
    /// Root kinds no source declares, per node (a root-kind mask): the
    /// roots.config overlay ([`config_entries`]), static background targets.
    pub extra_entries: Option<&'a HashMap<NodeIx, u16>>,
}

impl Default for WalkOptions<'_> {
    fn default() -> Self {
        WalkOptions {
            follow_open: true,
            assume_no_dependents: false,
            state_budget: DEFAULT_STATE_BUDGET,
            cancel: None,
            extra_entries: None,
        }
    }
}

/// The consumer-bit values ([`WalkState::ext`]) the dense visited set holds:
/// `ext` 0–7 (three bits) × 3 tiers × `crossedEvent` = 48 bits of a `u64`.
pub const DENSE_EXT: u8 = 8;

/// [`WalkOptions::state_budget`]'s default.
pub const DEFAULT_STATE_BUDGET: usize = 2_000_000;
const CANCEL_EVERY: usize = 4096;

/// What a walk did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WalkStats {
    pub states: usize,
    /// The highest hop count reached.
    pub max_hops: u32,
    pub limited: bool,
}

/// The walk's reusable scratch: the visited states and the level queues. One
/// scratch serves any number of walks over one graph; nothing in it outlives
/// a walk's meaning (it is cleared at the next walk's start).
#[derive(Default)]
pub struct WalkScratch {
    /// Per node, the visited states with `ext` below [`DENSE_EXT`] and no
    /// guard: bit `(ext * 3 + tier) * 2 + crossedEvent` (48 of 64 bits;
    /// round-2 r2 §2.1 Cost (a): impact's three bits and `dep_callers`' two
    /// stay here instead of the hash set).
    seen: Vec<u64>,
    /// Visited states with consumer bits of [`DENSE_EXT`] and above, or a
    /// table-event guard: the explicit hash fallback (callers' context sets
    /// its own bits there).
    seen_ext: HashSet<WalkState>,
    /// The nodes whose `seen` word is set (reset list).
    touched: Vec<NodeIx>,
    /// Tests only: every state goes to the hash fallback, so a test can
    /// compare the dense storage against it.
    #[cfg(test)]
    hash_only: bool,
    /// The nodes whose action-run entry was reported, per state.
    action_entries: HashSet<WalkState>,
    /// The nodes whose hidden channel was reported.
    hidden_done: HashSet<NodeIx>,
    level: Vec<(u32, WalkState, Option<WalkState>, Via)>,
    next: Vec<(u32, WalkState, Option<WalkState>, Via)>,
}

impl WalkScratch {
    /// The scratch's heap bytes (capacities): what a walk retains, the
    /// allocation measure of agent-tasks R0-6.
    #[must_use]
    pub fn bytes(&self) -> usize {
        use std::mem::size_of;
        let state = size_of::<WalkState>();
        let q = size_of::<(u32, WalkState, Option<WalkState>, Via)>();
        self.seen.capacity() * size_of::<u64>()
            + (self.seen_ext.capacity() + self.action_entries.capacity()) * (state + 8)
            + self.hidden_done.capacity() * (size_of::<NodeIx>() + 8)
            + self.touched.capacity() * size_of::<NodeIx>()
            + (self.level.capacity() + self.next.capacity()) * q
    }

    fn reset(&mut self, nodes: usize) {
        if self.seen.len() != nodes {
            self.seen = vec![0; nodes];
            self.touched.clear();
        } else {
            for n in self.touched.drain(..) {
                self.seen[n as usize] = 0;
            }
        }
        self.seen_ext.clear();
        self.action_entries.clear();
        self.hidden_done.clear();
        self.level.clear();
        self.next.clear();
    }

    /// A scratch that keeps every state in the hash fallback (tests only).
    #[cfg(test)]
    pub(crate) fn hash_only() -> WalkScratch {
        WalkScratch {
            hash_only: true,
            ..WalkScratch::default()
        }
    }

    /// Marks `s` visited; `false` when it already was.
    fn insert(&mut self, s: WalkState) -> bool {
        #[cfg(test)]
        if self.hash_only {
            return self.seen_ext.insert(s);
        }
        if s.ext >= DENSE_EXT || s.guard != 0 {
            return self.seen_ext.insert(s);
        }
        let bit = 1u64 << ((u32::from(s.ext) * 3 + s.tier as u32) * 2 + u32::from(s.crossed_event));
        let slot = &mut self.seen[s.node as usize];
        if *slot & bit != 0 {
            return false;
        }
        if *slot == 0 {
            self.touched.push(s.node);
        }
        *slot |= bit;
        true
    }
}

/// The tier of a path that reaches `node` over a site of confidence `open`
/// from a path of tier `tier`.
fn tier_at(g: &SitedGraph, node: NodeIx, tier: Tier, open: bool) -> Tier {
    let f = g.flags(node);
    let here = if open || f & flag::HUB != 0 {
        Tier::OnlyOpen
    } else if f & flag::LOCAL == 0 {
        Tier::ThroughDependency
    } else {
        Tier::Local
    };
    tier.max(here)
}

/// The tier of the state a walk reaches backwards over `site` from a state
/// of tier `tier`: the walk's own rule, for a visitor that needs the tier of
/// a state in [`ConeVisitor::step`] (before the walk builds it).
#[must_use]
pub fn tier_over(g: &SitedGraph, site: SiteIx, tier: Tier) -> Tier {
    let st = &g.sites()[site as usize];
    tier_at(g, st.from, tier, st.conf == super::project::Conf::Open)
}

/// One root's streaming walk: the visitor keeps what it needs; `scratch` is
/// reused across calls.
pub fn walk_one(
    g: &SitedGraph,
    root: NodeIx,
    opts: &WalkOptions<'_>,
    scratch: &mut WalkScratch,
    v: &mut dyn ConeVisitor,
) -> WalkStats {
    walk_multi(g, &[root], opts, scratch, v)
}

/// One breadth-first walk from every root at once; each state is labelled
/// with its nearest root (ties by root order), so per-root summaries cost
/// O(states), never O(states × roots).
pub fn walk_multi(
    g: &SitedGraph,
    roots: &[NodeIx],
    opts: &WalkOptions<'_>,
    scratch: &mut WalkScratch,
    v: &mut dyn ConeVisitor,
) -> WalkStats {
    scratch.reset(g.node_count());
    let mut stats = WalkStats::default();
    for (i, &r) in roots.iter().enumerate() {
        let s = WalkState {
            node: r,
            tier: tier_at(g, r, Tier::Local, false),
            crossed_event: false,
            ext: 0,
            guard: 0,
        };
        if scratch.insert(s) {
            scratch.level.push((i as u32, s, None, Via::Root(i as u32)));
        }
    }
    let universal = g
        .universal_hub()
        .filter(|&u| opts.follow_open && !g.sites_into(u).is_empty() && !roots.is_empty());
    let mut hops = 0u32;
    while !scratch.level.is_empty() {
        let mut level = std::mem::take(&mut scratch.level);
        level.sort_by_key(|(root, s, _, _)| (*root, *s));
        for &(root, s, from, via) in &level {
            if stats.states >= opts.state_budget {
                v.limit(Limit::StateBudget(opts.state_budget));
                stats.limited = true;
                scratch.level = level;
                return stats;
            }
            stats.states += 1;
            if stats.states.is_multiple_of(CANCEL_EVERY) && opts.cancel.is_some_and(|c| c()) {
                v.limit(Limit::Cancelled);
                stats.limited = true;
                scratch.level = level;
                return stats;
            }
            stats.max_hops = hops;
            report(g, root, s, hops, via, from, opts, scratch, v);
            for &site in g.sites_into(s.node) {
                let st = &g.sites()[site as usize];
                let open = st.conf == super::project::Conf::Open;
                if open && !opts.follow_open {
                    continue;
                }
                // Table-event dead dispatch: carry a subscriber's guard up to
                // its publisher; drop a raising site that proves it.
                let guard = match st.via() {
                    "eventFlow" => st.guard,
                    // A raising site (a record operation's `tableEvent`, a
                    // temporary-source page's lifecycle) that proves it.
                    _ if s.guard & st.guard != 0 => continue,
                    _ => 0,
                };
                let Some(ext) = v.step(g, s, site) else {
                    continue;
                };
                let n = WalkState {
                    node: st.from,
                    tier: tier_at(g, st.from, s.tier, open),
                    crossed_event: s.crossed_event || st.boundary == Boundary::EventDispatch,
                    ext,
                    guard,
                };
                if g.is_action_run(site) && scratch.action_entries.insert(n) {
                    v.entry(root, n.node, &[EntryKind::PageAction], n, hops + 1);
                }
                if scratch.insert(n) {
                    scratch.next.push((root, n, Some(s), Via::Site(site)));
                }
            }
        }
        if hops == 0
            && let Some(u) = universal
        {
            // The universal hub, once: one hop from every root.
            let s = WalkState {
                node: u,
                tier: Tier::OnlyOpen,
                crossed_event: false,
                ext: 0,
                guard: 0,
            };
            if scratch.insert(s) {
                scratch.next.push((0, s, None, Via::Universal));
            }
        }
        level.clear();
        scratch.level = std::mem::take(&mut scratch.next);
        scratch.next = level;
        hops += 1;
    }
    stats
}

/// The per-state reports of one state.
#[allow(clippy::too_many_arguments)]
fn report(
    g: &SitedGraph,
    root: u32,
    s: WalkState,
    hops: u32,
    via: Via,
    from: Option<WalkState>,
    opts: &WalkOptions<'_>,
    scratch: &mut WalkScratch,
    v: &mut dyn ConeVisitor,
) {
    v.reach(root, s, hops, via, from);
    let f = g.flags(s.node);
    if scratch.hidden_done.insert(s.node) {
        if f & flag::UNIVERSAL != 0 {
            v.hidden(root, s.node, HiddenChannel::Universal);
        }
        for ch in g.hidden_callers(s.node) {
            v.hidden(root, s.node, HiddenChannel::Flag(ch));
        }
    }
    let mask = g.root_kinds(s.node)
        | opts
            .extra_entries
            .and_then(|m| m.get(&s.node))
            .copied()
            .unwrap_or(0);
    let mut kinds = EntryKind::of_root_kinds(mask);
    // F3: the target of a static background start (a `Background` site) is
    // a `background-start` entry: a run starts there, in its own session.
    if g.is_background_target(s.node) {
        kinds.push(EntryKind::BackgroundStart);
        kinds.sort();
        kinds.dedup();
    }
    if !kinds.is_empty() {
        v.entry(root, s.node, &kinds, s, hops);
    }
    if !opts.assume_no_dependents {
        for (bit, why) in [
            (flag::EXT_PUBLIC, ExternalWhy::Public),
            (flag::EXT_PROTECTED, ExternalWhy::Protected),
            (flag::EXT_FRIEND, ExternalWhy::Friend),
        ] {
            if f & bit != 0 {
                v.external(root, s.node, why, s, hops);
            }
        }
    }
    if g.sites_into(s.node).is_empty() && f & flag::UNIVERSAL == 0 {
        v.source(root, s, hops);
    }
}

/// The roots.config overlay's kinds per node, from a model's root
/// classifications (the `config` and `ast+config` ones), joined on the
/// StableRoutineId. A graph-only revision has no model and passes none.
#[must_use]
pub fn config_entries(
    g: &SitedGraph,
    model: &crate::program::model::workspace::Model,
) -> HashMap<NodeIx, u16> {
    let stable = crate::engine::l4::summary::build_routine_stable_map(&model.workspace.routines);
    let mut out: HashMap<NodeIx, u16> = HashMap::new();
    for rc in &model.root_classifications {
        if rc.source == "ast" {
            continue;
        }
        let Some(id) = stable.get(&rc.routine_id).filter(|s| !s.is_empty()) else {
            continue;
        };
        let mask = rc
            .kinds
            .iter()
            .filter_map(|k| root_kind_bit(k))
            .fold(0u16, |m, b| m | b);
        for n in g.nodes_with_stable_id(id) {
            *out.entry(n).or_default() |= mask;
        }
    }
    out
}

/// Whether a run of routine node `r`'s object runs `r` with no user acting:
/// the resolver's own rule ([`crate::program::resolve::resolver::run_triggers`],
/// `dispatch_entry_trigger`). A codeunit's `OnRun`; a page's, report's or
/// XMLport's lifecycle triggers, and the same triggers of a page or report
/// EXTENSION (a run of the base object runs them too). Not: an action's
/// `OnAction`, a field's `OnValidate`/`OnLookup`/`OnDrillDown`, a page's
/// record triggers (`OnInsertRecord`, …), anything a page part runs.
#[must_use]
pub fn object_run_reaches(g: &SitedGraph, r: NodeIx) -> bool {
    let Some(super::project::Key::Routine(id)) = g.nodes.get(r as usize) else {
        return false;
    };
    run_reaches_trigger(
        id.object.kind,
        id.name_lc.as_str(),
        id.enclosing_member_lc.is_some(),
    )
}

/// [`object_run_reaches`] over a routine's parts: a trigger named `name_lc`
/// (folded) of an object of `kind`, declared in a member (`member`) or at
/// object level. The one rule both the sited graph and `alsem explain`'s
/// "reached from page actions" (a `RunObject` run) read.
#[must_use]
pub fn run_reaches_trigger(kind: al_syntax::ir::ObjectKind, name_lc: &str, member: bool) -> bool {
    use al_syntax::ir::ObjectKind;
    let kind = match kind {
        ObjectKind::PageExtension => ObjectKind::Page,
        ObjectKind::ReportExtension => ObjectKind::Report,
        k => k,
    };
    match crate::program::resolve::resolver::run_triggers(kind) {
        Some((object, members)) => {
            if member {
                members.contains(&name_lc)
            } else {
                object.contains(&name_lc)
            }
        }
        None => kind == ObjectKind::Codeunit && !member && name_lc == "onrun",
    }
}

/// Whether site `s` is an edge of the **code-call relation**: the calls the
/// program engine resolves for code that runs, the relation the model's calls
/// hold and the solve graph (`query touches` up, the detectors' caller
/// traversals) walks. It is the sited graph's may-execute relation without
/// what only a user or the platform starts:
///
/// - kept: a direct or interface call, an object run (`Codeunit.Run`,
///   `Page.RunModal`, a typed-variable run) and the trigger the run runs
///   ([`object_run_reaches`]), a record operation firing a table trigger or
///   raising its table's platform event (F1, `tableEvent`), an event
///   publisher dispatching to its subscribers;
/// - dropped: a page's lifecycle into a trigger a user starts (an action, a
///   field trigger, a part, a page it opens, its tables' triggers and platform
///   events), a rule's edge (`declaredRunnable`, a test's handler list or
///   runner), a background start (F3: a new session the model's calls do
///   not follow), every open site and hub.
///
/// On local code this is the solve graph's caller relation exactly (pinned by
/// `caller_agreement_tests`); the solve graph additionally stops at
/// dependency code (it holds workspace callers' calls only), which a
/// [`Tier::Local`] path never enters. R0-3's `select::direct_callers` counts
/// the same direct sites, minus event dispatch (a subscriber is dispatched,
/// not called).
#[must_use]
pub fn is_code_call_site(g: &SitedGraph, s: SiteIx) -> bool {
    let site = &g.sites()[s as usize];
    if site.conf == super::project::Conf::Open || g.flags(site.from) & flag::HUB != 0 {
        return false;
    }
    match site.boundary {
        Boundary::Sync | Boundary::Run | Boundary::TriggerOf { .. } => {
            matches!(
                site.via(),
                "call" | "interface" | "run" | "implicitTrigger" | "tableEvent"
            )
        }
        Boundary::EventDispatch => site.via() == "eventFlow",
        Boundary::Lifecycle => {
            g.routine_object(site.from).is_none() && object_run_reaches(g, site.to)
        }
        // A background start (F3): the started codeunit runs in its own
        // session, and the model's calls do not carry the start's edge.
        Boundary::Background => false,
        Boundary::TestHandler | Boundary::TestRunner | Boundary::Open => false,
    }
}

/// A visitor adapter that walks only the code-call relation
/// ([`is_code_call_site`]) and hands every report to the visitor it wraps.
pub struct CodeCalls<'v>(pub &'v mut dyn ConeVisitor);

impl ConeVisitor for CodeCalls<'_> {
    fn reach(&mut self, root: u32, s: WalkState, hops: u32, via: Via, from: Option<WalkState>) {
        self.0.reach(root, s, hops, via, from);
    }
    fn hidden(&mut self, root: u32, n: NodeIx, ch: HiddenChannel) {
        self.0.hidden(root, n, ch);
    }
    fn entry(&mut self, root: u32, n: NodeIx, kinds: &[EntryKind], s: WalkState, hops: u32) {
        self.0.entry(root, n, kinds, s, hops);
    }
    fn external(&mut self, root: u32, n: NodeIx, why: ExternalWhy, s: WalkState, hops: u32) {
        self.0.external(root, n, why, s, hops);
    }
    fn source(&mut self, root: u32, s: WalkState, hops: u32) {
        self.0.source(root, s, hops);
    }
    fn limit(&mut self, l: Limit) {
        self.0.limit(l);
    }
    fn step(&mut self, g: &SitedGraph, from: WalkState, site: SiteIx) -> Option<u8> {
        if is_code_call_site(g, site) {
            self.0.step(g, from, site)
        } else {
            None
        }
    }
}

/// A visitor that records every state's hops and first parent, for tests and
/// probes over small graphs (it is O(states), as a consumer's would be).
#[derive(Default)]
pub struct Recorder {
    pub states: Vec<(u32, WalkState, u32, Via, Option<WalkState>)>,
    pub hidden: Vec<(u32, NodeIx, HiddenChannel)>,
    pub entries: Vec<(u32, NodeIx, Vec<EntryKind>, WalkState, u32)>,
    pub external: Vec<(u32, NodeIx, ExternalWhy, WalkState, u32)>,
    pub sources: Vec<(u32, WalkState, u32)>,
    pub limits: Vec<Limit>,
}

impl ConeVisitor for Recorder {
    fn reach(&mut self, root: u32, s: WalkState, hops: u32, via: Via, from: Option<WalkState>) {
        self.states.push((root, s, hops, via, from));
    }
    fn hidden(&mut self, root: u32, n: NodeIx, ch: HiddenChannel) {
        self.hidden.push((root, n, ch));
    }
    fn entry(&mut self, root: u32, n: NodeIx, kinds: &[EntryKind], s: WalkState, hops: u32) {
        self.entries.push((root, n, kinds.to_vec(), s, hops));
    }
    fn external(&mut self, root: u32, n: NodeIx, why: ExternalWhy, s: WalkState, hops: u32) {
        self.external.push((root, n, why, s, hops));
    }
    fn source(&mut self, root: u32, s: WalkState, hops: u32) {
        self.sources.push((root, s, hops));
    }
    fn limit(&mut self, l: Limit) {
        self.limits.push(l);
    }
}

impl Recorder {
    /// The states of `node`, in reach order.
    #[must_use]
    pub fn of(&self, node: NodeIx) -> Vec<(WalkState, u32)> {
        self.states
            .iter()
            .filter(|x| x.1.node == node)
            .map(|x| (x.1, x.2))
            .collect()
    }
}

#[cfg(test)]
#[path = "reverse_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "caller_agreement_tests.rs"]
mod caller_agreement_tests;

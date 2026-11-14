//! **Who reaches each changed routine** (agent-tasks design §8.4 step 2): R0's
//! reverse walks over the production universe's sited graph.
//!
//! - Per root, [`walk_one`] (TLS-21: several roots at once on the one-shot
//!   CLI, each walk on a scratch from a stash of one per walk in flight) and
//!   a visitor that keeps COUNTS and the first few witnesses only
//!   ([`RootUp`]): its direct callers (explain's `callers:` definition,
//!   [`direct_callers`], with each caller's first call site), ancestors by
//!   tier, entries by kind (page actions by caption), event publishers that
//!   raise into the cone, external callability, hidden channels, the walk's
//!   limit, and its nearest ancestors with a stable id (what "nearest tested
//!   caller" maps into a test universe). Nothing per root outlives its walk
//!   but that summary (Fable f10: no cone per root is kept).
//! - Once over every root, [`walk_multi`]: the union (how many distinct
//!   entries the whole change is reachable from).
//!
//! **Relation and tier** (round-2 r2 §2.1, W2): the visitor's
//! [`ConeVisitor::step`] sets [`bit`]s on each state from the site it crosses
//! ([`site_bits`]: UI navigation, a rule edge, configured dispatch), so a
//! node's best state names its [`Population`]: established code, established
//! UI navigation, or a candidate (over configuration, over a rule edge, over
//! a hub). Ancestors and canonical entries
//! ([`crate::program::impact_graph::entries`]) are counted per population;
//! each population's witness is the state that put the node in it.
//!
//! The walk is R0's own; this module only aggregates.

use al_syntax::IdentifierFoldExt;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::Arc;

use serde_json::{Value, json};

use crate::engine::test_select::select::{direct_callers, display};
use crate::program::impact_graph::entries::EntryInventory;
use crate::program::impact_graph::reverse::{
    ConeVisitor, EntryKind, ExternalWhy, HiddenChannel, Limit, NodeIx, SiteIx, Tier, Via,
    WalkOptions, WalkScratch, WalkState, object_run_reaches, tier_over, walk_multi, walk_one,
};
use crate::program::impact_graph::sited::{Boundary, SitedGraph, flag};

/// The up part's schema.
pub const SCHEMA: &str = "al-sem.impact-up";

/// How many witnesses each list keeps (the rest are counted).
#[derive(Clone, Copy, Debug)]
pub struct Keep {
    pub per_list: usize,
    pub ancestors: usize,
}

impl Keep {
    #[must_use]
    pub fn new(detail: bool) -> Keep {
        Keep {
            per_list: if detail { 10 } else { 3 },
            ancestors: 32,
        }
    }
}

/// The entry kinds a user or a job starts (impact's ranking: these first).
#[must_use]
pub fn user_or_job(k: EntryKind) -> bool {
    matches!(
        k,
        EntryKind::PageAction
            | EntryKind::ApiPage
            | EntryKind::WebServiceExposed
            | EntryKind::JobQueueEntrypoint
            | EntryKind::BackgroundStart
            | EntryKind::ReportTrigger
            | EntryKind::TriggerPage
    )
}

/// Impact's consumer bits on a walk state ([`WalkState::ext`]; round-2 r2
/// §2.1 step 1). Bits are only set along a path, never cleared, so the
/// weakest step decides and a known navigation step stays known when the
/// path later crosses a hub. Three bits: every state fits R0's dense visited
/// set ([`crate::program::impact_graph::reverse::DENSE_EXT`]).
pub mod bit {
    /// The path takes a UI-navigation step: a page's lifecycle into a
    /// trigger a user starts (an action, a field trigger, a part, a page it
    /// opens, its tables' triggers), or an action's `RunObject`.
    pub const NAV: u8 = 1;
    /// The path takes a rule edge: a routine or object declaring a
    /// `TestPage`/`TestRequestPage` variable (`declaredRunnable`, which has
    /// no call site), or a page's lifecycle into a record trigger the page declares
    /// non-editable while code may make it editable at runtime
    /// ([`SitedGraph::is_ui_candidate`]). Its relation is not established.
    pub const RULE: u8 = 2;
    /// The path takes a site labelled `configuredDispatch`: it runs only
    /// when configuration data selects it.
    pub const CONFIGURED: u8 = 4;
}

/// The bits site `s` sets (round-2 r2 §2.1 step 1's table):
///
/// - code, no bit: a direct or interface call, an object run and the trigger
///   it runs, a record operation's trigger or table event, an event dispatch
///   (a deliberate superset of `is_code_call_site`, which takes `eventFlow`
///   only: running a publisher is code), an object node's lifecycle into a
///   trigger the run runs ([`object_run_reaches`]), a static background start
///   (the context axis labels it), a test runner or handler (a dispatching
///   builtin on a typed receiver is a `run` site since ENG-19: TLS-51 removed
///   the call-site `declaredRunnable` rule it had);
/// - [`bit::NAV`]: every other lifecycle site, and an action-run site;
/// - [`bit::RULE`]: `declaredRunnable` (a `TestPage` declaration, never a
///   call site), and (with [`bit::NAV`]) a candidate page-lifecycle site
///   ([`SitedGraph::is_ui_candidate`], [`crate::program::page_ui`]);
/// - an `Open` site sets nothing: the walk's tier says "over a hub";
/// - [`bit::CONFIGURED`], beside the row's own: a `configuredDispatch` site.
#[must_use]
pub fn site_bits(g: &SitedGraph, s: SiteIx) -> u8 {
    let st = &g.sites()[s as usize];
    let relation = match st.boundary {
        // A page's lifecycle into a record trigger the page may block
        // (`Editable = false`, while code may set its editability at
        // runtime): a UI step a rule allows but no fact establishes.
        Boundary::Lifecycle if g.is_ui_candidate(s) => bit::NAV | bit::RULE,
        Boundary::Lifecycle => {
            let code =
                !g.is_routine(st.from) && !g.is_action_run(s) && object_run_reaches(g, st.to);
            if code { 0 } else { bit::NAV }
        }
        // Every `declaredRunnable` site is a `TestPage`/`TestRequestPage`
        // declaration (no call site) since TLS-51.
        Boundary::Run if st.via() == "declaredRunnable" => {
            debug_assert!(st.anchor.is_none(), "a declaredRunnable call site");
            bit::RULE
        }
        Boundary::Sync
        | Boundary::Run
        | Boundary::Background
        | Boundary::EventDispatch
        | Boundary::TriggerOf { .. }
        | Boundary::TestHandler
        | Boundary::TestRunner
        | Boundary::Open => 0,
    };
    let configured = if g.is_configured_dispatch(s) {
        bit::CONFIGURED
    } else {
        0
    };
    relation | configured
}

/// The class of a walk state (round-2 r2 §2.1 step 2), in its total order,
/// best first: established code, established UI navigation, then the
/// candidates over configuration (code before UI navigation), over a rule
/// edge, over a hub. Configuration can be decided by reading data; a hub
/// cannot (r1 §8.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Population {
    /// Established: every step is code, the tier `local` or `via-dependency`.
    Code,
    /// Established: some step is UI navigation, none a rule edge or a hub.
    Navigation,
    /// A candidate over configuration whose steps are code.
    ConfiguredCode,
    /// A candidate over configuration with a UI-navigation step.
    ConfiguredNavigation,
    /// A candidate over a rule edge (relation not established).
    OverRule,
    /// A candidate over a hub (relation not established).
    OverHub,
}

impl Population {
    /// Every class, best first.
    pub const ALL: [Population; 6] = [
        Population::Code,
        Population::Navigation,
        Population::ConfiguredCode,
        Population::ConfiguredNavigation,
        Population::OverRule,
        Population::OverHub,
    ];

    /// The class of a state of `tier` with impact's bits `ext`. The tier
    /// comes from R0 as it is: a path over a hub is `via-hub` whatever its
    /// bits.
    #[must_use]
    pub fn of(tier: Tier, ext: u8) -> Population {
        if tier == Tier::OnlyOpen {
            Population::OverHub
        } else if ext & bit::RULE != 0 {
            Population::OverRule
        } else if ext & bit::CONFIGURED != 0 {
            if ext & bit::NAV != 0 {
                Population::ConfiguredNavigation
            } else {
                Population::ConfiguredCode
            }
        } else if ext & bit::NAV != 0 {
            Population::Navigation
        } else {
            Population::Code
        }
    }

    /// Established evidence: the tier is `local` or `via-dependency` and the
    /// relation is known.
    #[must_use]
    pub fn established(self) -> bool {
        matches!(self, Population::Code | Population::Navigation)
    }

    /// The class's JSON key.
    #[must_use]
    pub fn key(self) -> &'static str {
        match self {
            Population::Code => "code",
            Population::Navigation => "uiNavigation",
            Population::ConfiguredCode => "overConfigurationCode",
            Population::ConfiguredNavigation => "overConfigurationUiNavigation",
            Population::OverRule => "overRule",
            Population::OverHub => "overHub",
        }
    }

    /// The relation a witness of this class prints (§5's bracket form).
    #[must_use]
    pub fn relation(self) -> &'static str {
        match self {
            Population::Code | Population::ConfiguredCode => crate::engine::vocab::RELATION_CODE,
            Population::Navigation | Population::ConfiguredNavigation => {
                crate::engine::vocab::RELATION_NAVIGATION
            }
            Population::OverRule | Population::OverHub => "relation not established",
        }
    }
}

/// The tier a state shows (round-2 r2 §2.1 step 2): R0's `OnlyOpen` is
/// `via-hub`; else configured dispatch is `via-configuration`; else R0's
/// `ThroughDependency` is `via-dependency`; else `local`. Impact tracks no
/// app identity of its own.
#[must_use]
pub fn shown_tier(tier: Tier, ext: u8) -> &'static str {
    match tier {
        Tier::OnlyOpen => "via-hub",
        _ if ext & bit::CONFIGURED != 0 => crate::engine::vocab::VIA_CONFIGURATION,
        Tier::ThroughDependency => "via-dependency",
        Tier::Local => "local",
    }
}

/// [`shown_tier`]'s rank within a class: the better tier first.
fn tier_rank(tier: Tier, ext: u8) -> u8 {
    match shown_tier(tier, ext) {
        "local" => 0,
        "via-dependency" => 1,
        "via-hub" => 3,
        _ => 2,
    }
}

/// One state as a class witness: what it reached, how, and when it arrived.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Witness {
    tier: Tier,
    ext: u8,
    hops: u32,
    /// The node that shows it (an entry's node: a page for an action an
    /// action-run site stands for).
    node: NodeIx,
    /// Arrival order (ties keep it).
    at: usize,
}

impl Witness {
    fn population(&self) -> Population {
        Population::of(self.tier, self.ext)
    }

    /// The total order: class, then the better tier, then fewer hops.
    fn key(&self) -> (Population, u8, u32) {
        (self.population(), tier_rank(self.tier, self.ext), self.hops)
    }

    /// A path over a hub or a rule edge that also takes a UI-navigation
    /// step (r2 §2.1 step 2: "a path that also needs navigation says so").
    fn needs_navigation(&self) -> bool {
        self.ext & bit::NAV != 0
    }
}

/// The best class witness per id (a node, or a canonical entry).
#[derive(Default)]
struct Classed {
    best: HashMap<u32, Witness>,
    arrivals: usize,
}

impl Classed {
    /// A state of `(tier, ext)` reaching `id` at `hops` (shown by `node`):
    /// kept when it is better than the best so far (an equal one keeps the
    /// earlier arrival).
    fn add(&mut self, id: u32, node: NodeIx, tier: Tier, ext: u8, hops: u32) {
        self.arrivals += 1;
        let w = Witness {
            tier,
            ext,
            hops,
            node,
            at: self.arrivals,
        };
        let e = self.best.entry(id).or_insert(w);
        if w.key() < e.key() {
            *e = w;
        }
    }

    fn bytes(&self) -> usize {
        self.best.capacity() * (std::mem::size_of::<(u32, Witness)>() + 8)
    }

    /// The nearest witness of class `p`: the better tier, then fewer hops,
    /// then arrival.
    fn nearest(&self, p: Population) -> Option<(u32, Witness)> {
        self.best
            .iter()
            .filter(|(_, w)| w.population() == p)
            .min_by_key(|(_, w)| (w.key(), w.at))
            .map(|(&id, &w)| (id, w))
    }
}

/// A list's distinct nodes, each with its best witness state, and the first
/// `keep` of them for the answer ([`Firsts::first`]).
#[derive(Default)]
struct Firsts {
    /// Node -> its best `(only over open sites, hops, tier, arrival)`.
    best: HashMap<NodeIx, (bool, u32, Tier, usize)>,
    arrivals: usize,
}

impl Firsts {
    fn add(&mut self, n: NodeIx, hops: u32, tier: Tier) {
        let key = (tier == Tier::OnlyOpen, hops);
        self.arrivals += 1;
        let at = self.arrivals;
        let e = self.best.entry(n).or_insert((key.0, key.1, tier, at));
        if key < (e.0, e.1) {
            *e = (key.0, key.1, tier, at);
        }
    }

    /// The list's heap bytes (capacities), for the walk's cost report.
    fn bytes(&self) -> usize {
        self.best.capacity()
            * (std::mem::size_of::<NodeIx>() + std::mem::size_of::<(bool, u32, Tier, usize)>() + 8)
    }

    /// How many distinct nodes.
    fn count(&self) -> usize {
        self.best.len()
    }

    /// How many nodes only a path over an open site (a hub) reaches.
    fn only_open(&self) -> usize {
        self.best.values().filter(|b| b.0).count()
    }

    /// The witnesses to show: every node reached by a path with no open site
    /// first, nearest first, then the ones only an open site reaches (round-1
    /// c20: a `run:report` hub's 5-hop witness was listed beside the real
    /// 4-hop path as if it were one). Ties keep arrival (walk) order.
    fn first(&self, keep: usize) -> Vec<(u32, Tier, NodeIx)> {
        let mut v: Vec<(bool, u32, usize, Tier, NodeIx)> = self
            .best
            .iter()
            .map(|(&n, &(open, hops, tier, at))| (open, hops, at, tier, n))
            .collect();
        v.sort_unstable_by_key(|&(open, hops, at, _, _)| (open, hops, at));
        v.into_iter()
            .take(keep)
            .map(|(_, hops, _, tier, n)| (hops, tier, n))
            .collect()
    }
}

/// A node's best states: by tier alone (the legacy `ancestors` tiers), and
/// by class (round-2 r2 §2.1 step 2).
#[derive(Clone, Copy, Debug)]
struct NodeBest {
    /// The best `(tier, hops)`.
    tier: Tier,
    hops: u32,
    /// The best state in the class order; its witness is that state's.
    class: Witness,
}

/// What every walk of one answer shares: the program's canonical entries
/// and the changed routines' entries (left out on both sides of every
/// share).
struct Shared<'g> {
    inv: &'g EntryInventory,
    changed: HashSet<u32>,
}

/// One walk's visitor (one root, or the union).
struct Up<'g> {
    g: &'g SitedGraph,
    keep: Keep,
    shared: &'g Shared<'g>,
    /// Per node, its best tier (first reached at its shortest hops per tier)
    /// and its best class.
    best: HashMap<NodeIx, NodeBest>,
    /// The canonical entries reached (changed routines' left out), each with
    /// its best class witness.
    canon: Classed,
    /// The hop count of the state the walk last reached: the walk steps over
    /// a state's sites right after reaching it.
    cur_hops: u32,
    arrivals: usize,
    entries: BTreeMap<EntryKind, Firsts>,
    external: BTreeMap<&'static str, Firsts>,
    publishers: Firsts,
    /// Of [`Self::publishers`], those whose event raises a root directly
    /// (an event-dispatch site into a root).
    direct_publishers: Firsts,
    hidden: BTreeMap<&'static str, usize>,
    universal: bool,
    limit: Option<Limit>,
    /// The action-run site that made a page an entry (page node -> site).
    action_site: HashMap<NodeIx, SiteIx>,
    /// The nearest routine ancestors with a stable id, in reach order.
    ancestors: Vec<NodeIx>,
    root_nodes: BTreeSet<NodeIx>,
    /// The entry kinds of the walk's own roots (round-2 r2 §2.1, W1): a root
    /// is never listed as an entry it reaches, but a root that is an entry
    /// itself still ranks as one a user or a job starts.
    root_entry: BTreeSet<EntryKind>,
    states: usize,
    max_hops: u32,
}

impl<'g> Shared<'g> {
    /// The answer's shared state: `changed` holds every changed routine's
    /// nodes (each `#if` arm, symbol-only copies too).
    fn new(g: &'g SitedGraph, changed: &BTreeSet<NodeIx>) -> Shared<'g> {
        let inv = g.entry_inventory();
        Shared {
            inv,
            changed: changed.iter().filter_map(|&n| inv.of_node(n)).collect(),
        }
    }

    /// The program's entries a share counts: the inventory, the changed
    /// routines' left out.
    fn program(&self) -> usize {
        self.inv.len() - self.changed.len()
    }
}

impl<'g> Up<'g> {
    fn new(g: &'g SitedGraph, keep: Keep, roots: &[NodeIx], shared: &'g Shared<'g>) -> Up<'g> {
        Up {
            g,
            keep,
            shared,
            best: HashMap::new(),
            canon: Classed::default(),
            cur_hops: 0,
            arrivals: 0,
            entries: BTreeMap::new(),
            external: BTreeMap::new(),
            publishers: Firsts::default(),
            direct_publishers: Firsts::default(),
            hidden: BTreeMap::new(),
            universal: false,
            limit: None,
            action_site: HashMap::new(),
            ancestors: Vec::new(),
            root_nodes: roots.iter().copied().collect(),
            root_entry: BTreeSet::new(),
            states: 0,
            max_hops: 0,
        }
    }

    /// The visitor's heap bytes (capacities): what one walk's aggregation
    /// retains beside the walk's own scratch (round-2 r2 §2.1 Cost).
    fn bytes(&self) -> usize {
        use std::mem::size_of;
        self.best.capacity() * (size_of::<NodeIx>() + size_of::<NodeBest>() + 8)
            + self.canon.bytes()
            + self.entries.values().map(Firsts::bytes).sum::<usize>()
            + self.external.values().map(Firsts::bytes).sum::<usize>()
            + self.publishers.bytes()
            + self.direct_publishers.bytes()
            + self.action_site.capacity() * (size_of::<NodeIx>() + size_of::<SiteIx>() + 8)
            + self.ancestors.capacity() * size_of::<NodeIx>()
    }
}

impl ConeVisitor for Up<'_> {
    fn reach(&mut self, _: u32, s: WalkState, hops: u32, via: Via, _: Option<WalkState>) {
        self.states += 1;
        self.max_hops = self.max_hops.max(hops);
        self.cur_hops = hops;
        self.arrivals += 1;
        let w = Witness {
            tier: s.tier,
            ext: s.ext,
            hops,
            node: s.node,
            at: self.arrivals,
        };
        let e = self.best.entry(s.node).or_insert(NodeBest {
            tier: s.tier,
            hops,
            class: w,
        });
        if (s.tier, hops) < (e.tier, e.hops) {
            (e.tier, e.hops) = (s.tier, hops);
        }
        if w.key() < e.class.key() {
            e.class = w;
        }
        if let Via::Site(site) = via {
            let st = &self.g.sites()[site as usize];
            if st.boundary == Boundary::EventDispatch {
                self.publishers.add(st.from, hops, s.tier);
                // Round-2 n09: the events that raise a root itself (its
                // own subscriptions), apart from every publisher among the
                // ancestors.
                if self.root_nodes.contains(&st.to) {
                    self.direct_publishers.add(st.from, hops, s.tier);
                }
            }
        }
        if !self.root_nodes.contains(&s.node)
            && self.ancestors.len() < self.keep.ancestors
            && self.g.stable_id(s.node).is_some()
            && !self.ancestors.contains(&s.node)
        {
            self.ancestors.push(s.node);
        }
    }
    fn hidden(&mut self, _: u32, _: NodeIx, ch: HiddenChannel) {
        match ch {
            HiddenChannel::Universal => self.universal = true,
            HiddenChannel::Flag(f) => *self.hidden.entry(f).or_default() += 1,
        }
    }
    fn entry(&mut self, _: u32, n: NodeIx, kinds: &[EntryKind], s: WalkState, hops: u32) {
        // Round-2 r2 §2.1 (W1): the changed routine is not an entry it
        // reaches (c12 listed its own page action at 0 hops beside `local 0`).
        if self.root_nodes.contains(&n) {
            self.root_entry.extend(kinds.iter().copied());
            return;
        }
        for k in kinds {
            self.entries.entry(*k).or_default().add(n, hops, s.tier);
        }
        // Round-2 r2 §2.1 step 3: the node's canonical entry. A page R0
        // reports over an action's `RunObject` is no entry by itself (the
        // actions the site stands for are counted in `step`).
        if let Some(e) = self.shared.inv.of_node(n)
            && !self.shared.changed.contains(&e)
        {
            self.canon.add(e, n, s.tier, s.ext, hops);
        }
    }
    fn external(&mut self, _: u32, n: NodeIx, why: ExternalWhy, s: WalkState, hops: u32) {
        self.external
            .entry(why.as_str())
            .or_default()
            .add(n, hops, s.tier);
    }
    fn limit(&mut self, l: Limit) {
        self.limit = Some(l);
    }
    fn step(&mut self, g: &SitedGraph, from: WalkState, site: SiteIx) -> Option<u8> {
        let st = &g.sites()[site as usize];
        let ext = from.ext | site_bits(g, site);
        if g.is_action_run(site) {
            self.action_site.entry(st.from).or_insert(site);
            // Round-2 r2 §2.1 step 3: each action the site stands for is one
            // canonical page-action entry, reached by the state this step
            // makes (the walk reached `from` just before, at `cur_hops`).
            let tier = tier_over(g, site, from.tier);
            for &e in self.shared.inv.of_site(site) {
                if !self.shared.changed.contains(&e) {
                    self.canon.add(e, st.from, tier, ext, self.cur_hops + 1);
                }
            }
        }
        Some(ext)
    }
}

/// Page-action captions, read from the analyzed source (the files of the
/// shown entries only). Shared by the up walk's tasks (TLS-21): a file is
/// parsed outside the lock, and the first result inserted is kept. Two tasks
/// that miss the same file at once may both parse it; a value depends only on
/// its file, so either result is the same.
pub struct Captions {
    /// Workspace-relative path -> its text.
    texts: HashMap<String, Arc<str>>,
    /// Unit -> per object (lowercased name): its actions `(name lc, caption,
    /// first line, last line)` (0-based).
    parsed: std::sync::Mutex<HashMap<String, Arc<Vec<ObjectActions>>>>,
    #[cfg(test)]
    race: Option<CaptionRace>,
}

/// Tests: a forced race on `unit`, in two steps. A second task to ask for
/// `unit` waits, before it reads the cache, until the first task has missed
/// and is about to parse (`first_parsing`). The first task is then held
/// between its parse and its insert until the second has read the cache
/// (`lookups`). So the second read always falls inside the first task's
/// miss-to-insert window. A timeout in either step sets `timed_out`, so a
/// test can tell the race did not happen.
#[cfg(test)]
pub(crate) struct CaptionRace {
    pub(crate) unit: String,
    pub(crate) timeout: std::time::Duration,
    pub(crate) arrivals: std::sync::atomic::AtomicUsize,
    pub(crate) first_parsing: std::sync::atomic::AtomicBool,
    pub(crate) lookups: std::sync::atomic::AtomicUsize,
    pub(crate) timed_out: std::sync::atomic::AtomicBool,
}

#[cfg(test)]
impl CaptionRace {
    /// Wait until `done` holds or the timeout passes (then `timed_out`).
    fn wait(&self, done: impl Fn() -> bool) {
        let until = std::time::Instant::now() + self.timeout;
        while !done() {
            if std::time::Instant::now() >= until {
                self.timed_out
                    .store(true, std::sync::atomic::Ordering::SeqCst);
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }
}

/// One action: `(name lc, caption, first line, last line)` (0-based).
type ActionRow = (String, Option<String>, u32, u32);
/// One object's actions: `(object name lc, its actions)`.
type ObjectActions = (String, Vec<ActionRow>);

impl Captions {
    #[must_use]
    pub fn new(texts: HashMap<String, Arc<str>>) -> Captions {
        Captions {
            texts,
            parsed: std::sync::Mutex::default(),
            #[cfg(test)]
            race: None,
        }
    }

    /// Tests: the cache with a [`CaptionRace`] on `unit`.
    #[cfg(test)]
    pub(crate) fn with_race(mut self, unit: &str, timeout: std::time::Duration) -> Captions {
        self.race = Some(CaptionRace {
            unit: unit.to_string(),
            timeout,
            arrivals: std::sync::atomic::AtomicUsize::new(0),
            first_parsing: std::sync::atomic::AtomicBool::new(false),
            lookups: std::sync::atomic::AtomicUsize::new(0),
            timed_out: std::sync::atomic::AtomicBool::new(false),
        });
        self
    }

    /// Tests: the race this cache was given.
    #[cfg(test)]
    pub(crate) fn race(&self) -> Option<&CaptionRace> {
        self.race.as_ref()
    }

    /// Tests: how many files the cache holds.
    #[cfg(test)]
    pub(crate) fn cached(&self) -> usize {
        self.parsed.lock().unwrap_or_else(|p| p.into_inner()).len()
    }

    fn actions(&self, unit: &str) -> Arc<Vec<ObjectActions>> {
        // Units and local names spell a workspace file `ws:<rel>` or `<rel>`.
        let unit = unit.strip_prefix("ws:").unwrap_or(unit);
        let lock = || self.parsed.lock().unwrap_or_else(|p| p.into_inner());
        // Tests: a later task waits for the first one's miss before reading.
        #[cfg(test)]
        let race = self.race.as_ref().filter(|r| r.unit == unit);
        #[cfg(test)]
        if let Some(r) = race
            && r.arrivals.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 1
        {
            r.wait(|| r.first_parsing.load(std::sync::atomic::Ordering::SeqCst));
        }
        let hit = lock().get(unit).map(Arc::clone);
        // Tests: a lookup counts once the cache was read; the first one that
        // missed is held below.
        #[cfg(test)]
        let first_miss = race.and_then(|r| {
            let k = r.lookups.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            (hit.is_none() && k == 0).then_some(r)
        });
        if let Some(v) = hit {
            return v;
        }
        #[cfg(test)]
        if let Some(r) = first_miss {
            r.first_parsing
                .store(true, std::sync::atomic::Ordering::SeqCst);
        }
        // Parsed outside the lock: tasks needing other files do not wait.
        let v = Arc::new(
            self.texts
                .get(unit)
                .map(|t| {
                    let _s = crate::engine::perf_trace::span("impact", "impact.up.caption_parse");
                    // Parses, not files: two tasks may parse the same file.
                    crate::engine::perf_trace::counter_delta("impact.up.caption_parses", 1);
                    let f = al_syntax::parse(t);
                    f.objects
                        .iter()
                        .map(|o| {
                            (
                                o.name.fold_identifier(),
                                o.actions
                                    .iter()
                                    .map(|a| {
                                        (
                                            a.name.fold_identifier(),
                                            a.caption.first().cloned(),
                                            a.origin.start.row,
                                            a.origin.end.row,
                                        )
                                    })
                                    .collect(),
                            )
                        })
                        .collect()
                })
                .unwrap_or_default(),
        );
        #[cfg(test)]
        if let Some(r) = first_miss {
            r.wait(|| r.lookups.load(std::sync::atomic::Ordering::SeqCst) >= 2);
        }
        Arc::clone(lock().entry(unit.to_string()).or_insert(v))
    }

    /// The `(action name, caption)` of a page-action entry: an action's
    /// trigger node (its enclosing member names the action), or a page
    /// reached over an action's `RunObject` site (the site's line names it).
    fn of(
        &self,
        g: &SitedGraph,
        n: NodeIx,
        site: Option<SiteIx>,
    ) -> Option<(String, Option<String>)> {
        let l = g.local_name(n)?;
        let (object, file) = (l.object.fold_identifier(), l.file.to_string());
        let d = g.display(n);
        let parts: Vec<&str> = d.splitn(3, "::").collect();
        let member = (parts.len() == 3).then(|| parts[1].to_string());
        let line = site
            .and_then(|s| g.sites()[s as usize].anchor)
            .map(|a| (g.unit(a.unit).to_string(), a.start_line));
        let unit = line.as_ref().map_or(file, |(u, _)| u.clone());
        let objects = self.actions(&unit);
        let acts = objects
            .iter()
            .find(|(o, _)| *o == object)
            .map(|(_, a)| a.as_slice())
            .unwrap_or(&[]);
        let hit = match (&member, &line) {
            (Some(m), _) => acts.iter().find(|a| a.0 == *m),
            (None, Some((_, ln))) => acts.iter().find(|a| a.2 <= *ln && *ln <= a.3),
            _ => None,
        }?;
        Some((hit.0.clone(), hit.1.clone()))
    }
}

fn tier_str(t: Tier) -> &'static str {
    t.as_str()
}

/// A node as the answer names it.
fn node_json(g: &SitedGraph, n: NodeIx, hops: u32, tier: Tier) -> Value {
    let mut v = json!({"display": display(g, n), "hops": hops, "tier": tier_str(tier),
                       "pathTier": crate::engine::vocab::tier(tier)});
    if let Some(id) = g.stable_id(n) {
        v["stableId"] = json!(id);
    }
    if let Some(l) = g.local_name(n) {
        v["file"] = json!(l.file.to_string());
    }
    v
}

/// What the legacy `ancestors` tiers count: every relation together (the
/// classified counts are `byRelation`).
pub const POPULATION: &str =
    "legacy: every relation together (code, UI navigation, configuration, hub); see byRelation";

/// Counts per class and tier, in `byRelation`'s shape (round-2 r2 §2.1
/// step 5): `{code: {local, viaDependency}, uiNavigation: {local,
/// viaDependency}, candidates: {overConfiguration: {code, uiNavigation},
/// overRule, overHub}, total}`.
fn by_relation(ws: impl Iterator<Item = Witness>) -> Value {
    let mut n: BTreeMap<(Population, bool), usize> = BTreeMap::new();
    let mut total = 0usize;
    for w in ws {
        total += 1;
        *n.entry((w.population(), w.tier == Tier::Local))
            .or_default() += 1;
    }
    let get = |p: Population, local: bool| n.get(&(p, local)).copied().unwrap_or(0);
    let both = |p: Population| get(p, true) + get(p, false);
    json!({
        "code": {"local": get(Population::Code, true), "viaDependency": get(Population::Code, false)},
        "uiNavigation": {"local": get(Population::Navigation, true),
                         "viaDependency": get(Population::Navigation, false)},
        "candidates": {
            "overConfiguration": {"code": both(Population::ConfiguredCode),
                                  "uiNavigation": both(Population::ConfiguredNavigation)},
            "overRule": both(Population::OverRule),
            "overHub": both(Population::OverHub),
        },
        "total": total,
    })
}

/// A witness as the answer names it: the node, its hops, the tier it shows
/// and its relation (round-2 r2 §5's `[<tier>, <relation>]`), and whether a
/// candidate's path also takes a UI-navigation step.
fn witness_json(g: &SitedGraph, w: &Witness) -> Value {
    let p = w.population();
    let mut v = json!({"display": display(g, w.node), "hops": w.hops,
                       "tier": shown_tier(w.tier, w.ext), "relation": p.relation(),
                       "established": p.established()});
    if matches!(p, Population::OverRule | Population::OverHub) {
        v["needsNavigation"] = json!(w.needs_navigation());
    }
    if let Some(id) = g.stable_id(w.node) {
        v["stableId"] = json!(id);
    }
    v
}

/// A share of the program's canonical entries.
fn share_json(reached: usize, program: usize) -> Value {
    json!({"reached": reached, "program": program,
           "percent": (reached * 100).checked_div(program).map(|p| p.min(100))})
}

/// The classes a population key gathers: the five a population line prints
/// (`overConfiguration` holds both relations).
const POPULATIONS: [(&str, &[Population]); 5] = [
    ("code", &[Population::Code]),
    ("uiNavigation", &[Population::Navigation]),
    (
        "overConfiguration",
        &[Population::ConfiguredCode, Population::ConfiguredNavigation],
    ),
    ("overRule", &[Population::OverRule]),
    ("overHub", &[Population::OverHub]),
];

/// The classified part of a walk's summary (round-2 r2 §2.1 steps 2-5):
/// ancestors and canonical entries per population, the entries by kind and
/// context, the program's share, each population's nearest witness, and the
/// external callers of the code population.
fn classified(up: &Up<'_>, g: &SitedGraph) -> serde_json::Map<String, Value> {
    use crate::engine::vocab;
    let inv = up.shared.inv;
    let ancestors: Vec<Witness> = up
        .best
        .iter()
        .filter(|(n, _)| !up.root_nodes.contains(n) && g.flags(**n) & flag::HUB == 0)
        .map(|(_, b)| b.class)
        .collect();
    let entries: Vec<(u32, Witness)> = up.canon.best.iter().map(|(&e, &w)| (e, w)).collect();
    let program = up.shared.program();
    let mut by_kind = serde_json::Map::new();
    let mut by_context = serde_json::Map::new();
    let mut share = serde_json::Map::new();
    let mut nearest = serde_json::Map::new();
    let mut first_by_kind = serde_json::Map::new();
    for (key, classes) in POPULATIONS {
        let mine: Vec<&(u32, Witness)> = entries
            .iter()
            .filter(|(_, w)| classes.contains(&w.population()))
            .collect();
        let mut kinds: BTreeMap<EntryKind, usize> = BTreeMap::new();
        let mut contexts: BTreeMap<&str, usize> = BTreeMap::new();
        // Per kind, the nearest entry (class order, then arrival).
        let mut firsts: BTreeMap<EntryKind, (u32, Witness)> = BTreeMap::new();
        for &&(e, w) in &mine {
            let k = inv.kind(e);
            *kinds.entry(k).or_default() += 1;
            let ctx: BTreeSet<&str> = inv.kinds(e).filter_map(vocab::entry_context).collect();
            for c in ctx {
                *contexts.entry(c).or_default() += 1;
            }
            let f = firsts.entry(k).or_insert((e, w));
            if (w.key(), w.at) < (f.1.key(), f.1.at) {
                *f = (e, w);
            }
        }
        // Kinds by their §8.1 words, merged (`page trigger` holds the API
        // pages' triggers too), in print order.
        let mut words: Vec<(&str, usize)> = Vec::new();
        for (k, n) in &kinds {
            let w = vocab::entry_kind(*k);
            match words.iter_mut().find(|(x, _)| *x == w) {
                Some(x) => x.1 += n,
                None => words.push((w, *n)),
            }
        }
        by_kind.insert(
            key.into(),
            Value::Object(
                words
                    .iter()
                    .map(|(w, n)| ((*w).to_string(), json!(n)))
                    .collect(),
            ),
        );
        by_context.insert(
            key.into(),
            Value::Object(
                contexts
                    .iter()
                    .map(|(c, n)| ((*c).to_string(), json!(n)))
                    .collect(),
            ),
        );
        share.insert(key.into(), share_json(mine.len(), program));
        let mut printed: Vec<&str> = Vec::new();
        let mut list: Vec<Value> = Vec::new();
        for (k, (e, w)) in &firsts {
            let word = vocab::entry_kind(*k);
            if printed.contains(&word) {
                continue;
            }
            printed.push(word);
            let mut v = witness_json(g, w);
            v["kind"] = json!(word);
            if let Some(a) = inv.action(*e) {
                v["action"] = json!(a);
            }
            list.push(v);
        }
        first_by_kind.insert(key.into(), Value::Array(list));
        // The nearest witness: an entry of the population, else its nearest
        // ancestor.
        let entry = classes
            .iter()
            .filter_map(|p| up.canon.nearest(*p))
            .min_by_key(|(_, w)| (w.key(), w.at));
        let w = match entry {
            Some((e, w)) => {
                let mut v = witness_json(g, &w);
                v["entry"] = json!(vocab::entry_kind(inv.kind(e)));
                if let Some(a) = inv.action(e) {
                    v["action"] = json!(a);
                }
                Some(v)
            }
            None => ancestors
                .iter()
                .filter(|w| classes.contains(&w.population()))
                .min_by_key(|w| (w.key(), w.at))
                .map(|w| witness_json(g, w)),
        };
        nearest.insert(key.into(), w.unwrap_or(Value::Null));
    }
    let candidates = entries
        .iter()
        .filter(|(_, w)| !w.population().established())
        .count();
    share.insert("candidates".into(), share_json(candidates, program));
    share.insert("all".into(), share_json(entries.len(), program));
    let program_by_kind: Vec<(&str, usize)> = {
        let mut words: Vec<(&str, usize)> = Vec::new();
        // The same set as `programEntries`: the changed routines left out.
        for (k, n) in inv.by_kind(|e| !up.shared.changed.contains(&e)) {
            let w = vocab::entry_kind(k);
            match words.iter_mut().find(|(x, _)| *x == w) {
                Some(x) => x.1 += n,
                None => words.push((w, n)),
            }
        }
        words
    };
    // `external:` belongs to the code population: the external callers whose
    // best state is established code.
    let external_code: Vec<Value> = up
        .external
        .iter()
        .map(|(why, f)| {
            let n = f
                .best
                .keys()
                .filter(|n| {
                    up.best
                        .get(n)
                        .is_some_and(|b| b.class.population() == Population::Code)
                })
                .count();
            json!({"why": why, "count": n})
        })
        .filter(|v| v["count"] != json!(0))
        .collect();
    let mut m = serde_json::Map::new();
    m.insert("relationRecorded".into(), json!(true));
    m.insert(
        "byRelation".into(),
        json!({"ancestors": by_relation(ancestors.iter().copied()),
               "entries": by_relation(entries.iter().map(|(_, w)| *w))}),
    );
    m.insert("entriesByKind".into(), Value::Object(by_kind));
    m.insert("entriesByContext".into(), Value::Object(by_context));
    m.insert(
        "programEntriesByKind".into(),
        Value::Object(
            program_by_kind
                .iter()
                .map(|(w, n)| ((*w).to_string(), json!(n)))
                .collect(),
        ),
    );
    m.insert("programShare".into(), Value::Object(share));
    m.insert("nearest".into(), Value::Object(nearest));
    m.insert("firstByKind".into(), Value::Object(first_by_kind));
    m.insert("externalCode".into(), json!(external_code));
    // §R2.4 landing changes 1-2: the totals say when they include a page
    // counted once for its unnamed actions, or a background target with no
    // `OnRun` (counted apart).
    let noted = |bit: u8| {
        let reached = entries
            .iter()
            .filter(|(e, _)| inv.note(*e) & bit != 0)
            .count();
        let program = (0..inv.len() as u32)
            .filter(|e| inv.note(*e) & bit != 0 && !up.shared.changed.contains(e))
            .count();
        json!({"reached": reached, "program": program})
    };
    m.insert(
        "entryNotes".into(),
        json!({
            "pagesWithUnnamedActions": noted(crate::program::impact_graph::entries::NOTE_UNNAMED_ACTIONS),
            "backgroundTargetsWithoutOnRun":
                noted(crate::program::impact_graph::entries::NOTE_BACKGROUND_WITHOUT_ON_RUN),
            "unnamedActionRunSites": inv.unnamed_sites(),
        }),
    );
    m
}

/// What one walk found, as JSON.
fn summary(up: &Up<'_>, g: &SitedGraph, captions: &Captions) -> Value {
    let (mut local, mut dep, mut open) = (0usize, 0usize, 0usize);
    for (n, b) in &up.best {
        if up.root_nodes.contains(n) || g.flags(*n) & flag::HUB != 0 {
            continue;
        }
        match b.tier {
            Tier::Local => local += 1,
            Tier::ThroughDependency => dep += 1,
            Tier::OnlyOpen => open += 1,
        }
    }
    let entries: Vec<Value> = EntryKind::ALL
        .iter()
        .filter_map(|k| up.entries.get(k).map(|f| (k, f)))
        .map(|(k, f)| {
            let first: Vec<Value> = f
                .first(up.keep.per_list)
                .into_iter()
                .map(|(h, t, n)| {
                    let mut v = node_json(g, n, h, t);
                    if *k == EntryKind::PageAction
                        && let Some((action, caption)) =
                            captions.of(g, n, up.action_site.get(&n).copied())
                    {
                        v["action"] = json!(action);
                        v["caption"] = json!(caption);
                    }
                    v
                })
                .collect();
            json!({"kind": k.as_str(), "count": f.count(), "onlyOpen": f.only_open(),
                   "userOrJob": user_or_job(*k), "first": first})
        })
        .collect();
    let firsts = |f: &Firsts| -> Vec<Value> {
        f.first(up.keep.per_list)
            .into_iter()
            .map(|(h, t, n)| node_json(g, n, h, t))
            .collect()
    };
    let external: Vec<Value> = up
        .external
        .iter()
        .map(|(why, f)| {
            json!({"why": why, "count": f.count(), "onlyOpen": f.only_open(),
                   "first": firsts(f)})
        })
        .collect();
    // Output validation V2 (§3.2, F12): the hubs on the reach (each with a
    // site that calls it from inside the reach), and the program's entry
    // count, the denominator of the reach share that folds the lists.
    let mut hubs: BTreeSet<(String, String)> = BTreeSet::new();
    for n in up.best.keys() {
        let Some(kind) = crate::engine::l5::routine_rollup::hub_kind_of(g, *n) else {
            continue;
        };
        let at = g
            .sites_into(*n)
            .iter()
            .map(|&s| &g.sites()[s as usize])
            .filter(|s| up.best.contains_key(&s.from))
            .filter_map(|s| s.anchor.map(|a| (s.from, a)))
            .map(|(from, a)| {
                // `ws:` / `dep:<app name>:` as every human location reads.
                let unit = g.unit(a.unit);
                let file = match g.node_app(from) {
                    _ if unit.starts_with("ws:") || unit.starts_with("dep:") => unit.to_string(),
                    Some(app) if app == g.primary_app() => format!("ws:{unit}"),
                    Some(app) => format!("dep:{}:{unit}", g.app_identity(app).1),
                    None => unit.to_string(),
                };
                format!("{file}:{}", a.start_line + 1)
            })
            .min()
            .unwrap_or_default();
        hubs.insert((kind, at));
    }
    let entry_total: usize = up.entries.values().map(Firsts::count).sum();
    let mut out = json!({
        "rootEntryKinds": up.root_entry.iter().map(|k| k.as_str()).collect::<Vec<_>>(),
        "rootUserOrJob": up.root_entry.iter().any(|k| user_or_job(*k)),
        "hubs": hubs.iter().map(|(k, a)| json!({"kind": k, "at": a})).collect::<Vec<_>>(),
        // §R2.4 landing change 5: one program total, the canonical count
        // (`programShare.program`); the legacy root-kind mask count is gone.
        "programEntries": up.shared.program(),
        "entryBuckets": entry_total,
        "states": up.states,
        "maxHops": up.max_hops,
        // The legacy tiers mix every relation (r2 §2.1 step 5: they stay
        // until the next impact version; the classified counts are
        // `byRelation`). §R2.4 landing change 7: the label says so here
        // only.
        "ancestors": {"total": local + dep + open, "local": local, "throughDependency": dep,
                      "onlyOpen": open, "population": POPULATION},
        "entries": entries,
        "userOrJobEntries": up.entries.iter().filter(|(k, _)| user_or_job(**k)).map(|(_, f)| f.count()).sum::<usize>(),
        // `count`: publishers anywhere among the ancestors; `direct` (round-2
        // n09, additive): the events that raise a root itself.
        "eventPublishers": {"count": up.publishers.count(), "onlyOpen": up.publishers.only_open(),
                            "first": firsts(&up.publishers),
                            "direct": {"count": up.direct_publishers.count(),
                                       "first": firsts(&up.direct_publishers)}},
        "external": external,
        "hidden": {"universalHub": up.universal, "byChannel": up.hidden},
        "limited": up.limit.map(|l| match l {
            Limit::StateBudget(n) => format!("state budget {n}"),
            Limit::Cancelled => "cancelled".to_string(),
        }),
    });
    if let Value::Object(o) = &mut out {
        o.extend(classified(up, g));
    }
    out
}

/// The direct callers of `r`'s nodes (explain's definition), each with its
/// first call site into the root (`file:line`, 1-based), sorted by display.
fn callers_json(g: &SitedGraph, nodes: &[NodeIx], keep: usize) -> Value {
    let mut callers: BTreeSet<NodeIx> = BTreeSet::new();
    for &n in nodes {
        callers.extend(direct_callers(g, n));
    }
    let mut rows: Vec<(String, Value)> = callers
        .iter()
        .map(|&c| {
            // Its first site into a root node, or into the root's object.
            let site = nodes.iter().find_map(|&n| {
                g.sites_into(n)
                    .iter()
                    .map(|&s| &g.sites()[s as usize])
                    .find(|s| s.from == c && s.anchor.is_some())
                    .or_else(|| {
                        g.sites_into(n).iter().find_map(|&s| {
                            let o = g.sites()[s as usize].from;
                            g.sites_into(o)
                                .iter()
                                .map(|&t| &g.sites()[t as usize])
                                .find(|t| t.from == c && t.anchor.is_some())
                        })
                    })
            });
            let mut v = json!({"display": display(g, c)});
            if let Some(id) = g.stable_id(c) {
                v["stableId"] = json!(id);
            }
            if let Some(a) = site.and_then(|s| s.anchor) {
                v["file"] = json!(g.unit(a.unit));
                v["line"] = json!(a.start_line + 1);
            }
            (display(g, c), v)
        })
        .collect();
    rows.sort_by(|a, b| a.0.cmp(&b.0));
    let total = rows.len();
    let local = callers
        .iter()
        .filter(|&&c| g.flags(c) & flag::LOCAL != 0)
        .count();
    json!({
        "count": total,
        "local": local,
        "first": rows.into_iter().take(keep).map(|r| r.1).collect::<Vec<_>>(),
    })
}

/// What the up part is asked.
pub struct UpArgs<'a> {
    /// The changed roots (StableRoutineIds), in the main part's order.
    pub roots: &'a [String],
    pub assume_no_dependents: bool,
    pub keep: Keep,
    /// Shared by the tasks walking at once, so `Sync`.
    pub cancel: Option<&'a (dyn Fn() -> bool + Sync)>,
    /// How many roots walk at once (TLS-21), stated by the route.
    pub workers: crate::engine::l5::digest_cli::ReachWorkers,
}

/// One root's result (TLS-21): its row and what its walk cost.
struct RootOut {
    row: Value,
    states: usize,
    visitor_bytes: usize,
    walk_us: u64,
}

/// The scratches of the walks in flight (TLS-21): a walk takes one, or makes
/// one when none is free, and gives it back, so no more exist than walks in
/// flight. Each walk resets its scratch, so what one walk left never reaches
/// another's answer.
#[derive(Default)]
struct Scratches(std::sync::Mutex<Vec<WalkScratch>>);

impl Scratches {
    fn take(&self, _workers: &crate::engine::l5::digest_cli::ReachWorkers) -> WalkScratch {
        if let Some(s) = self.0.lock().unwrap_or_else(|p| p.into_inner()).pop() {
            return s;
        }
        #[cfg(test)]
        if let Some(p) = _workers.probe() {
            p.scratches_made
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
        WalkScratch::default()
    }

    fn put(&self, s: WalkScratch) {
        self.0.lock().unwrap_or_else(|p| p.into_inner()).push(s);
    }
}

/// A walk's options from the up part's arguments. Each task builds its own:
/// `WalkOptions` holds a plain `&dyn Fn` and is not `Sync`.
fn walk_options<'a>(args: &UpArgs<'a>) -> WalkOptions<'a> {
    WalkOptions {
        assume_no_dependents: args.assume_no_dependents,
        cancel: args.cancel.map(|c| c as &dyn Fn() -> bool),
        ..Default::default()
    }
}

/// One root's walk over its nodes (its `#if` arms walk together) and its row.
fn walk_root(
    g: &SitedGraph,
    args: &UpArgs<'_>,
    shared: &Shared<'_>,
    captions: &Captions,
    scratches: &Scratches,
    id: &str,
    nodes: &[NodeIx],
) -> RootOut {
    let opts = walk_options(args);
    let mut v = Up::new(g, args.keep, nodes, shared);
    let mut scratch = scratches.take(&args.workers);
    let t = std::time::Instant::now();
    if nodes.len() == 1 {
        walk_one(g, nodes[0], &opts, &mut scratch, &mut v);
    } else {
        walk_multi(g, nodes, &opts, &mut scratch, &mut v);
    }
    let walk_us = t.elapsed().as_micros() as u64;
    scratches.put(scratch);
    let mut s = summary(&v, g, captions);
    s["stableId"] = json!(id);
    s["status"] = json!("walked");
    s["display"] = json!(display(g, nodes[0]));
    s["nodes"] = json!(nodes.len());
    s["callers"] = callers_json(g, nodes, args.keep.per_list);
    s["nearestAncestors"] = json!(
        v.ancestors
            .iter()
            .filter_map(|&n| {
                g.stable_id(n).map(|id| {
                    json!({"stableId": id, "display": display(g, n),
                           "hops": v.best.get(&n).map(|b| b.hops)})
                })
            })
            .collect::<Vec<_>>()
    );
    RootOut {
        row: s,
        states: v.states,
        visitor_bytes: v.bytes(),
        walk_us,
    }
}

/// `/proc/self/status`'s `key` line in KiB (Linux).
fn status_kib(key: &str) -> Option<u64> {
    let s = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = s.lines().find(|l| l.starts_with(key))?;
    line[key.len()..]
        .trim_start_matches(':')
        .split_whitespace()
        .next()?
        .parse()
        .ok()
}

/// TLS-21, measurement only: the up walk's own memory window. Resets the
/// process's peak mark (`5` into `/proc/self/clear_refs`, Linux) and returns
/// the resident size at the start, in KiB. `None` (nothing reported) unless
/// tracing is on AND the route asked for it ([`ReachWorkers::up_window`]: a
/// one-shot run of a binary started with `AL_SEM_UP_WALK_WINDOW=1`, never a
/// resident revision, whose memory budget reads the same mark), or when the
/// reset fails. The reset also resets what `ru_maxrss` and the batch census
/// report, so such a run's process peak is not the command's.
///
/// [`ReachWorkers::up_window`]: crate::engine::l5::digest_cli::ReachWorkers::up_window
fn walk_window_start(workers: &crate::engine::l5::digest_cli::ReachWorkers) -> Option<u64> {
    if !workers.up_window()
        || !crate::engine::perf_trace::enabled(crate::engine::perf_trace::Detail::Stages)
    {
        return None;
    }
    std::fs::write("/proc/self/clear_refs", "5").ok()?;
    status_kib("VmRSS")
}

/// The up part's JSON over `g`. The walked roots run `args.workers` at a time
/// (TLS-21: the one-shot CLI 8 capped by the cores, a resident revision 1);
/// rows keep root order.
#[must_use]
pub fn up(g: &SitedGraph, args: &UpArgs<'_>, captions: &Captions) -> Value {
    let started = std::time::Instant::now();
    // Every changed routine's nodes (each `#if` arm, symbol-only copies
    // too): their entries are left out of both sides of every walk's share.
    let shared = Shared::new(
        g,
        &args
            .roots
            .iter()
            .flat_map(|id| g.nodes_with_stable_id(id))
            .collect(),
    );
    // After the entry inventory's first build: the window holds the walks.
    let window = walk_window_start(&args.workers);
    // Each root's nodes, in root order; empty: not in the graph.
    let roots: Vec<(&String, Vec<NodeIx>)> = args
        .roots
        .iter()
        .map(|id| {
            let nodes = g
                .nodes_with_stable_id(id)
                .into_iter()
                .filter(|&n| g.flags(n) & flag::SYMBOL_ONLY == 0)
                .collect();
            (id, nodes)
        })
        .collect();
    let mut all_nodes: Vec<NodeIx> = roots.iter().flat_map(|(_, n)| n.clone()).collect();
    let scratches = Scratches::default();
    #[cfg(test)]
    let first_walked = roots.iter().position(|(_, n)| !n.is_empty());
    #[cfg(test)]
    let walked = roots.iter().filter(|(_, n)| !n.is_empty()).count();
    let task = |(_i, (id, nodes)): (usize, &(&String, Vec<NodeIx>))| -> RootOut {
        if nodes.is_empty() {
            return RootOut {
                row: json!({"stableId": id, "status": "not-in-graph"}),
                states: 0,
                visitor_bytes: 0,
                walk_us: 0,
            };
        }
        #[cfg(test)]
        let _in = args.workers.probe().map(|p| {
            use std::sync::atomic::Ordering::SeqCst;
            p.up_tasks.fetch_add(1, SeqCst);
            p.pool_threads
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(rayon::current_num_threads());
            // In flight first, so a held first root counts while it waits.
            let in_flight = p.enter();
            let hold = *p.hold_first_root.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(timeout) = hold
                && Some(_i) == first_walked
            {
                let until = std::time::Instant::now() + timeout;
                while p.up_done.load(SeqCst) + 1 < walked {
                    if std::time::Instant::now() >= until {
                        p.first_root_timed_out.store(true, SeqCst);
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
            }
            in_flight
        });
        let out = walk_root(g, args, &shared, captions, &scratches, id, nodes);
        #[cfg(test)]
        if let Some(p) = args.workers.probe() {
            p.up_done.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
        out
    };
    let outs: Vec<RootOut> = {
        let _s = crate::engine::perf_trace::span("impact", "impact.up.roots");
        let n = args.workers.count();
        // Serial when asked, and from inside a rayon worker: there `install`
        // would let the blocked worker run other jobs while it waits (the
        // literal pass's reason, TLS-22); such a caller is parallel already.
        let in_rayon = n > 1 && rayon::current_thread_index().is_some();
        if in_rayon {
            crate::engine::perf_trace::counter_delta("impact.up.serial_in_rayon", 1);
        }
        if n <= 1 || in_rayon {
            roots.iter().enumerate().map(task).collect()
        } else {
            use rayon::prelude::*;
            // One root per task: per-root cost is uneven, work stealing
            // balances it; an indexed collect keeps root order.
            crate::engine::l5::digest_cli::reach_pool(n).install(|| {
                roots
                    .par_iter()
                    .enumerate()
                    .with_max_len(1)
                    .map(task)
                    .collect()
            })
        }
    };
    let mut states = 0usize;
    // The largest visitor of any one walk.
    let mut visitor_bytes = 0usize;
    let mut walks_us = 0u64;
    let mut roots_out = Vec::with_capacity(outs.len());
    for o in outs {
        states += o.states;
        visitor_bytes = visitor_bytes.max(o.visitor_bytes);
        walks_us += o.walk_us;
        roots_out.push(o.row);
    }
    all_nodes.sort_unstable();
    all_nodes.dedup();
    let mut u = Up::new(g, args.keep, &all_nodes, &shared);
    // One scratch for the union walk; the idle ones go first (their bytes
    // still count: the per-root pass held them).
    let mut idle = scratches.0.into_inner().unwrap_or_else(|p| p.into_inner());
    let mut scratch = idle.pop().unwrap_or_default();
    let idle_bytes: usize = idle.iter().map(WalkScratch::bytes).sum();
    drop(idle);
    let t = std::time::Instant::now();
    let ustats = {
        let _s = crate::engine::perf_trace::span("impact", "impact.up.union_walk");
        walk_multi(g, &all_nodes, &walk_options(args), &mut scratch, &mut u)
    };
    walks_us += t.elapsed().as_micros() as u64;
    // Every scratch's capacity: the scratch memory the pass held.
    let scratch_bytes = idle_bytes + scratch.bytes();
    drop(scratch);
    visitor_bytes = visitor_bytes.max(u.bytes());
    let union = {
        let _s = crate::engine::perf_trace::span("impact", "impact.up.union_summary");
        summary(&u, g, captions)
    };
    crate::engine::perf_trace::counter("impact.up.scratch_bytes", scratch_bytes as u64);
    if let Some(start) = window
        && let Some(peak) = status_kib("VmHWM")
    {
        crate::engine::perf_trace::counter("impact.up.rss_start_mb", start / 1024);
        crate::engine::perf_trace::counter("impact.up.peak_mb", peak / 1024);
    }
    json!({
        "schema": SCHEMA,
        "version": 1,
        "graph": {"nodes": g.node_count(), "sites": g.site_count()},
        // §R2.4 landing change 3: R0's `local` tier includes a dependency
        // loaded from a source folder; the scope line says so when the
        // program has one.
        "localSourceDependencies": (0..g.node_count() as u32).any(|n| {
            g.flags(n) & flag::LOCAL != 0
                && crate::program::impact_graph::dep_callers::is_dependency_node(g, n)
        }),
        "roots": roots_out,
        "union": union,
        "cost": {"perRootStates": states, "unionStates": ustats.states,
                 "walkMs": started.elapsed().as_millis() as u64,
                 // The walks alone (per root and the union), without the
                 // summaries and captions: the sum of each walk's own time,
                 // so more than the wall time when roots walk at once.
                 "walksUs": walks_us,
                 // Every scratch's capacity (one per walk in flight, TLS-21):
                 // varies with the worker count and between runs, a
                 // measurement like `walkMs`, not part of the answer.
                 "scratchBytes": scratch_bytes, "visitorBytes": visitor_bytes,
                 "inventoryBytes": shared.inv.bytes()},
    })
}

#[cfg(test)]
#[path = "up_par_tests.rs"]
pub(crate) mod up_par_tests;

#[cfg(test)]
mod tests {
    use super::*;

    /// A list's witnesses put every node a path with no open site reaches
    /// before the ones only an open site (a hub) reaches, nearest first; a
    /// node reached first over a hub and later by a real path is shown with
    /// the real path; arrival order breaks ties. States are fed in hop order,
    /// as the walk delivers them.
    ///
    /// DISCRIMINATION (2026-11-06): sorting `Firsts::first` by `(hops,
    /// arrival)` (the open flag dropped from the key) fails the first assert
    /// (the 2-hop hub witness comes first).
    #[test]
    fn real_paths_are_listed_before_open_site_witnesses() {
        let mut f = Firsts::default();
        f.add(10, 2, Tier::OnlyOpen);
        f.add(11, 2, Tier::OnlyOpen);
        f.add(12, 3, Tier::Local);
        f.add(13, 3, Tier::ThroughDependency);
        f.add(11, 4, Tier::ThroughDependency);
        f.add(12, 5, Tier::OnlyOpen);
        assert_eq!(
            f.first(10),
            vec![
                (3, Tier::Local, 12),
                (3, Tier::ThroughDependency, 13),
                (4, Tier::ThroughDependency, 11),
                (2, Tier::OnlyOpen, 10),
            ]
        );
        assert_eq!(f.count(), 4);
        assert_eq!(f.only_open(), 1, "only node 10 has no real path");
        assert_eq!(f.first(2).len(), 2);
    }

    use al_syntax::ir::ObjectKind;

    use crate::program::impact_graph::project::tests::{World, sited_of};
    use crate::program::impact_graph::{ObjectFacts, RoutineFacts, RunTarget};
    use crate::program::node::RoutineNodeId;
    use crate::program::node_extract::Access;
    use crate::snapshot::TrustTier;

    fn mask(kinds: &[&str]) -> u16 {
        kinds
            .iter()
            .map(|k| crate::engine::root_classification::root_kind_bit(k).expect("a kind"))
            .fold(0, |m, b| m | b)
    }

    /// A hand-built program whose root `R` (itself a page action) is reached
    /// by a page that an action's `RunObject` opens (`Opened`, an out-of-mask
    /// R0 entry), and by a codeunit a static background start enters (`Bg`,
    /// another). Declared entries (root kinds with an entry kind): `R`,
    /// `Opened.OnOpenPage`, `Bg.OnRun` and the unreached `Other.OnOpenPage`.
    /// Returns the graph and the nodes `(R, Opened.OnOpenPage, Bg.OnRun)`.
    pub(super) fn entry_world() -> (SitedGraph, NodeIx, NodeIx, NodeIx) {
        let mut w = World::default();
        let a = w.app("W");
        let cu = w.obj(a, ObjectKind::Codeunit, 1, "Cu", TrustTier::Workspace);
        let card = w.obj(a, ObjectKind::Page, 3, "Card", TrustTier::Workspace);
        let opened = w.obj(a, ObjectKind::Page, 6, "Opened", TrustTier::Workspace);
        let other = w.obj(a, ObjectKind::Page, 7, "Other", TrustTier::Workspace);
        let bg = w.obj(a, ObjectKind::Codeunit, 2, "Bg", TrustTier::Workspace);
        w.facts.objects.insert(
            card.clone(),
            ObjectFacts {
                opens: vec![RunTarget::Object(opened.clone())],
                action_runs: vec![RunTarget::Object(opened.clone())],
                ..Default::default()
            },
        );
        let r = w.rtn_with(&cu, "R", |x| x.access = Access::Local);
        let card_t = w.trigger(&card, "OnOpenPage");
        let open_t = w.trigger(&opened, "OnOpenPage");
        let other_t = w.trigger(&other, "OnOpenPage");
        let on_run = w.trigger(&bg, "OnRun");
        let starter = w.rtn_with(&cu, "Starter", |x| x.access = Access::Local);
        let sink = w.rtn_with(&cu, "Sink", |x| x.access = Access::Local);
        w.call(&open_t, &r, 1);
        w.call(&on_run, &r, 2);
        // Every trigger calls into the workspace, so pruning keeps it.
        w.call(&card_t, &sink, 3);
        w.call(&other_t, &sink, 4);
        w.background(
            &starter,
            vec![crate::program::resolve::edge::Route {
                target: crate::program::resolve::edge::RouteTarget::Routine(on_run.clone()),
                evidence: crate::program::resolve::edge::Evidence::Source,
                conditions: vec![],
                witness: crate::program::resolve::edge::Witness::None,
                receiver_tier: None,
            }],
            5,
        );
        let kinds = |w: &mut World, id: &RoutineNodeId, k: &[&str]| {
            w.facts.routines.insert(
                id.clone(),
                RoutineFacts {
                    root_kinds: mask(k),
                    ..Default::default()
                },
            );
        };
        kinds(&mut w, &r, &["trigger-page", "page-action"]);
        kinds(&mut w, &open_t, &["trigger-page"]);
        kinds(&mut w, &other_t, &["trigger-page"]);
        kinds(&mut w, &on_run, &["onrun-codeunit"]);
        let g = sited_of(&w, a, &[]);
        let n = |id: &RoutineNodeId| g.routine_node(id).expect("a node");
        let (r, open_t, on_run) = (n(&r), n(&open_t), n(&on_run));
        (g, r, open_t, on_run)
    }

    pub(super) fn walked(g: &SitedGraph, r: NodeIx) -> Value {
        let shared = Shared::new(g, &[r].into_iter().collect());
        let mut v = Up::new(g, Keep::new(false), &[r], &shared);
        walk_one(
            g,
            r,
            &WalkOptions::default(),
            &mut WalkScratch::default(),
            &mut v,
        );
        summary(&v, g, &Captions::new(HashMap::new()))
    }

    /// Round-2 r2 §2.1 steps 3-4 (W2, replacing W1's mask predicate): the
    /// share counts canonical entries on both sides, out-of-mask entries
    /// included (the page whose action's `RunObject` runs on the path, the
    /// codeunit object a background start enters), the changed routine left
    /// out of both. So `reached <= program`, and every reached entry is one
    /// the program counts.
    ///
    /// §R2.4 landing change 1: the background target object and the `OnRun`
    /// it starts are one entry; the page whose unnamed action runs on the
    /// path is noted. `programEntries` is the share's program total.
    ///
    /// DISCRIMINATION (2026-11-07): counting the changed routine's entry in
    /// the program (`Shared::program` returning `inv.len()`) fails "3 of 4"
    /// (3 of 5). DISCRIMINATION (2026-11-07, landing change 1): keying the
    /// background target object by its own node (the `on_run` lookup in
    /// `EntryInventory::of` skipped) fails "the background target and its
    /// OnRun are one entry".
    #[test]
    fn the_entry_share_counts_canonical_entries_on_both_sides() {
        let (g, r, open_t, on_run) = entry_world();
        let s = walked(&g, r);
        let kinds: Vec<&str> = s["entries"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|e| e["kind"].as_str())
            .collect();
        assert!(
            kinds.contains(&"background-start") && kinds.contains(&"page-action"),
            "precondition: R0 reports the out-of-mask entries: {kinds:?}"
        );
        let inv = g.entry_inventory();
        assert!(inv.of_node(open_t).is_some() && inv.of_node(on_run).is_some());
        assert!(inv.of_node(r).is_some(), "precondition: R itself is one");
        let bg = (0..g.node_count() as u32)
            .find(|&n| g.is_background_target(n))
            .expect("precondition: a background target");
        assert_eq!(
            inv.of_node(bg),
            inv.of_node(on_run),
            "the background target and its OnRun are one entry"
        );
        let d = &s["programShare"]["all"];
        let (reached, program) = (d["reached"].as_u64(), d["program"].as_u64());
        assert!(
            reached <= program,
            "reached {reached:?} <= program {program:?}"
        );
        // Reached: Opened.OnOpenPage, Bg (its object and OnRun, one entry)
        // and Card's action-run page; the program adds Other.OnOpenPage and
        // leaves R out.
        assert_eq!((reached, program), (Some(3), Some(4)), "{d:#}");
        assert_eq!(d["percent"], json!(75));
        assert_eq!(s["programEntries"], json!(4));
        // Card's RunObject names no action: one entry for the page, noted.
        assert_eq!(
            s["entryNotes"]["pagesWithUnnamedActions"],
            json!({"reached": 1, "program": 1})
        );
        assert_eq!(s["relationRecorded"], json!(true));
        assert_eq!(s["ancestors"]["population"], json!(POPULATION));
        assert!(
            s["population"].is_null(),
            "the label is on `ancestors` only"
        );
    }

    /// Round-2 r2 §2.1 (W1): the changed routine is not listed as an entry it
    /// reaches; a root that is an entry itself keeps its kinds apart
    /// (`rootEntryKinds`) and still ranks as reached from a user.
    ///
    /// DISCRIMINATION (2026-11-07): dropping the root check in `Up::entry`
    /// fails "the root is not its own page-action entry" (R is listed at
    /// 0 hops).
    #[test]
    fn the_root_is_not_its_own_entry() {
        let (g, r, _, _) = entry_world();
        let s = walked(&g, r);
        let page_action = s["entries"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|e| e["kind"] == "page-action")
            .expect("precondition: a page-action entry (the action-run page)");
        let display = display(&g, r);
        assert!(
            !page_action["first"]
                .as_array()
                .into_iter()
                .flatten()
                .any(|n| n["display"] == json!(display) || n["hops"] == json!(0)),
            "the root is not its own page-action entry: {page_action:#}"
        );
        assert_eq!(s["rootEntryKinds"], json!(["page-action", "trigger-page"]));
        assert_eq!(s["rootUserOrJob"], json!(true));
        // Nor is it one of its own canonical entries, in any population.
        let all = &s["programShare"]["all"];
        assert_eq!(all["reached"], json!(3), "{all:#}");
    }
}

#[cfg(test)]
#[path = "up_tests.rs"]
mod relation_tests;

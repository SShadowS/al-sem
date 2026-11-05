//! **Callers through dependency code** (design r2 §3.2,
//! `docs/superpowers/specs/2026-11-06-dependency-callers-design.md`): the
//! workspace routines that reach a root only by a code-call path that passes
//! dependency code, with a witness each.
//!
//! Not a new structure: R0's reverse walk over the sited graph
//! ([`super::reverse::walk_multi`]) wrapped in [`super::reverse::CodeCalls`],
//! run for one root, with a visitor that
//!
//! - sets its own consumer bits ([`bit`]): [`bit::DEP`] once the path passes a
//!   node of **dependency code** ([`is_dependency_node`]: its app is not the
//!   workspace app; the line `ExecRelation` draws, NOT R0's trust-tier
//!   `Tier::Local`, under which an app loaded from local source is local), and
//!   [`bit::CONFIGURED`] once it passes a site labelled `configuredDispatch`
//!   ([`super::configured_dispatch`]);
//! - records each state's parent and site, so a path is rebuilt from the
//!   caller's state back to the root: the witness comes from the same walk as
//!   the claim, and ends at the root, not at the first workspace routine the
//!   path enters.
//!
//! Each workspace routine (a routine node of the primary app with a stable
//! id; `#if` arms of one id are one routine) takes the best class of the
//! states that reached it: no `DEP` -> an **ordinary** code-call caller (the
//! solve graph's relation; not listed); `DEP` without `CONFIGURED` ->
//! [`DepTier::Code`]; only `DEP | CONFIGURED` -> [`DepTier::Configured`]. A
//! workspace path beats a code path beats a configured one; within a class
//! the shortest path is the witness. The path may cross between workspace
//! and dependency code any number of times, so callers compose transitively.
//! The first hop into dependency code is any code-call site: a call, an
//! object run, a record operation firing a dependency trigger, event dispatch.
//!
//! **Potential, not proven**: a witness shows graph reachability by code
//! calls, not a realizable execution, and the walk follows code-call sites
//! only (no open site, hub or user-started page trigger), so an empty answer
//! is not a whole-program absence. A walk that hit its budget says so
//! ([`Answer::limited`]).

use std::collections::{BTreeSet, HashMap};

use super::project::Key;
use super::reverse::{
    CodeCalls, ConeVisitor, Limit, NodeIx, SiteIx, Via, WalkOptions, WalkScratch, WalkState,
    walk_multi,
};
use super::sited::{Boundary, ConfiguredArm, SitedGraph};

/// The visitor's consumer bits ([`WalkState::ext`]).
pub mod bit {
    /// The path passed a node of dependency code.
    pub const DEP: u8 = 1;
    /// The path passed a `configuredDispatch` site.
    pub const CONFIGURED: u8 = 2;
}

/// Whether node `n` is dependency code: its app is not the workspace app.
/// One definition, shared with `ExecRelation`'s split (`Role::Dependency`: any
/// app other than the primary app); a hub has no app and is not.
#[must_use]
pub fn is_dependency_node(g: &SitedGraph, n: NodeIx) -> bool {
    g.node_app(n).is_some_and(|a| a != g.primary_app())
}

/// A listed caller's tier, best first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DepTier {
    /// Reached by a code-call path through dependency code.
    Code,
    /// Reached only by paths through a configured-dispatch site: a
    /// configuration-dependent potential caller, listed after the others.
    Configured,
}

impl DepTier {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            DepTier::Code => "code",
            DepTier::Configured => "configured",
        }
    }
}

/// What one witness hop is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HopKind {
    /// A direct call.
    Call,
    /// Interface dispatch (conditionally resolved).
    Interface,
    /// An object run, to the trigger the run runs (the object node and its
    /// lifecycle step are one hop).
    Run,
    /// A record operation firing a table trigger.
    RecordTrigger,
    /// A publisher dispatching to a subscriber ([`Hop::from`] is the event).
    Event,
}

impl HopKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            HopKind::Call => "call",
            HopKind::Interface => "interface",
            HopKind::Run => "run",
            HopKind::RecordTrigger => "recordTrigger",
            HopKind::Event => "event",
        }
    }
}

/// One hop of a witness, in execution order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hop {
    pub kind: HopKind,
    /// The routine the hop leaves (an event hop: the publisher).
    pub from: NodeIx,
    /// The routine it enters.
    pub to: NodeIx,
    /// The hop's first site (its anchor is where `from` makes the call).
    pub site: SiteIx,
    /// The catalog entry and arm, on a hop through a configured site.
    pub configured: Option<ConfiguredArm>,
}

/// One workspace routine that reaches the root through dependency code.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DepCaller {
    /// Its node (the first of its stable id's nodes the best state reached).
    pub node: NodeIx,
    pub stable_id: String,
    pub tier: DepTier,
    /// The witness passes an event dispatch.
    pub crossed_event: bool,
    /// The witness: from the caller to the root, every hop.
    pub hops: Vec<Hop>,
    /// The first workspace routine after the witness's last dependency hop.
    pub enters_at: Option<NodeIx>,
}

/// One root's answer.
#[derive(Debug, Default)]
pub struct Answer {
    /// Sorted by tier, then hop count, then stable id.
    pub callers: Vec<DepCaller>,
    /// The stable ids of the workspace routines reached without dependency
    /// code: the code-call relation on workspace code (agreement with the
    /// solve graph, design r2 invariant 1).
    pub ordinary: BTreeSet<String>,
    /// The walk stopped at a cap: the list is a lower bound.
    pub limited: Option<Limit>,
    /// States the walk visited.
    pub states: usize,
}

impl Answer {
    /// The number of listed callers in `tier`.
    #[must_use]
    pub fn count(&self, tier: DepTier) -> usize {
        self.callers.iter().filter(|c| c.tier == tier).count()
    }
}

/// The parent-recording visitor.
struct Visitor {
    /// The `configuredDispatch` bit of a site's conditions mask.
    configured_mask: u8,
    parent: HashMap<WalkState, (WalkState, SiteIx)>,
    reached: Vec<(WalkState, u32)>,
    limit: Option<Limit>,
}

impl ConeVisitor for Visitor {
    fn reach(&mut self, _: u32, s: WalkState, hops: u32, via: Via, from: Option<WalkState>) {
        if let (Via::Site(site), Some(f)) = (via, from) {
            self.parent.insert(s, (f, site));
        }
        self.reached.push((s, hops));
    }
    fn limit(&mut self, l: Limit) {
        self.limit = Some(l);
    }
    fn step(&mut self, g: &SitedGraph, from: WalkState, site: SiteIx) -> Option<u8> {
        let st = &g.sites()[site as usize];
        let mut ext = from.ext;
        if is_dependency_node(g, st.from) {
            ext |= bit::DEP;
        }
        if st.conditions & self.configured_mask != 0 {
            ext |= bit::CONFIGURED;
        }
        Some(ext)
    }
}

/// The class of a state: 0 ordinary, 1 code, 2 configured.
fn class(s: WalkState) -> u8 {
    match (s.ext & bit::DEP != 0, s.ext & bit::CONFIGURED != 0) {
        (false, _) => 0,
        (true, false) => 1,
        (true, true) => 2,
    }
}

/// Whether `n` is a workspace routine with a stable id.
fn workspace_routine(g: &SitedGraph, n: NodeIx) -> Option<String> {
    if !matches!(g.nodes.get(n as usize), Some(Key::Routine(_))) || is_dependency_node(g, n) {
        return None;
    }
    if g.node_app(n) != Some(g.primary_app()) {
        return None;
    }
    g.stable_id(n)
}

/// The callers through dependency code of the routine whose nodes are
/// `roots` (its stable id's nodes, [`SitedGraph::nodes_with_stable_id`]).
#[must_use]
pub fn callers_through_dependency(
    g: &SitedGraph,
    roots: &[NodeIx],
    opts: &WalkOptions<'_>,
) -> Answer {
    let mut v = Visitor {
        configured_mask: super::sited::condition_mask(
            [super::configured_dispatch::CONDITION].iter().copied(),
        ),
        parent: HashMap::new(),
        reached: Vec::new(),
        limit: None,
    };
    let stats = walk_multi(
        g,
        roots,
        opts,
        &mut WalkScratch::default(),
        &mut CodeCalls(&mut v),
    );
    let root_ids: BTreeSet<String> = roots.iter().filter_map(|&r| g.stable_id(r)).collect();
    // Per stable id: its best state (class, hops), first reached on ties.
    let mut best: HashMap<String, (u8, u32, WalkState)> = HashMap::new();
    for &(s, hops) in &v.reached {
        let Some(id) = workspace_routine(g, s.node) else {
            continue;
        };
        if root_ids.contains(&id) {
            continue;
        }
        let key = (class(s), hops);
        match best.get(&id) {
            Some(b) if (b.0, b.1) <= key => {}
            _ => {
                best.insert(id, (key.0, key.1, s));
            }
        }
    }
    let mut answer = Answer {
        limited: v.limit,
        states: stats.states,
        ..Answer::default()
    };
    for (id, (c, _, s)) in best {
        if c == 0 {
            answer.ordinary.insert(id);
            continue;
        }
        let hops = witness(g, &v.parent, s);
        let enters_at = hops
            .iter()
            .rposition(|h| is_dependency_node(g, h.from))
            .map(|i| hops[i].to);
        answer.callers.push(DepCaller {
            node: s.node,
            stable_id: id,
            tier: if c == 1 {
                DepTier::Code
            } else {
                DepTier::Configured
            },
            crossed_event: hops.iter().any(|h| h.kind == HopKind::Event),
            hops,
            enters_at,
        });
    }
    answer.callers.sort_by(|a, b| {
        (a.tier, a.hops.len(), &a.stable_id).cmp(&(b.tier, b.hops.len(), &b.stable_id))
    });
    answer
}

/// The hops from `s` back to the root, in execution order, an object node and
/// its lifecycle step merged into one `Run` hop.
fn witness(
    g: &SitedGraph,
    parent: &HashMap<WalkState, (WalkState, SiteIx)>,
    s: WalkState,
) -> Vec<Hop> {
    // The raw chain: (site, from node, to node), caller first.
    let mut chain: Vec<(SiteIx, NodeIx, NodeIx)> = Vec::new();
    let mut at = s;
    while let Some(&(p, site)) = parent.get(&at) {
        chain.push((site, at.node, p.node));
        at = p;
    }
    let mut hops = Vec::new();
    let mut i = 0;
    while i < chain.len() {
        let (site, from, mut to) = chain[i];
        let st = &g.sites()[site as usize];
        let mut configured = g.configured_arm(site).cloned();
        let mut kind = match st.boundary {
            Boundary::EventDispatch => HopKind::Event,
            Boundary::TriggerOf { .. } => HopKind::RecordTrigger,
            Boundary::Run | Boundary::Lifecycle => HopKind::Run,
            _ if st.via() == "interface" => HopKind::Interface,
            _ => HopKind::Call,
        };
        // A run (or a record trigger through an object node) enters the
        // object; its lifecycle step enters the trigger the run runs.
        if matches!(g.nodes.get(to as usize), Some(Key::Object(_)))
            && let Some(&(next, _, trigger)) = chain.get(i + 1)
        {
            configured = configured.or_else(|| g.configured_arm(next).cloned());
            to = trigger;
            if kind == HopKind::Call {
                kind = HopKind::Run;
            }
            i += 1;
        }
        hops.push(Hop {
            kind,
            from,
            to,
            site,
            configured,
        });
        i += 1;
    }
    hops
}

/// The block's `relation` string.
pub const RELATION: &str = "code-calls-through-dependency";

/// What the block follows, and what it leaves out (its `follows` string).
pub const FOLLOWS: &str = "code calls (calls, object runs and the triggers a run runs, record \
     triggers, event dispatch) that pass dependency code (an app other than the workspace's), any \
     number of times, ending at this routine; not open sites, hubs or what a user starts on a page";

/// A path longer than this is folded into a count in human output (design
/// r2 §6, Q4): presentation, never a cut.
pub const HUMAN_HOP_FOLD: usize = 12;

/// `stateBudget` / `cancelled`.
#[must_use]
pub fn limit_str(l: Limit) -> &'static str {
    match l {
        Limit::StateBudget(_) => "stateBudget",
        Limit::Cancelled => "cancelled",
    }
}

/// A hop's JSON: its kind, the routine it enters, the event (an event hop),
/// the site's anchor and the configured arm. `name` names a node.
#[must_use]
pub fn hop_json(g: &SitedGraph, h: &Hop, name: &dyn Fn(NodeIx) -> String) -> serde_json::Value {
    let mut m = serde_json::Map::new();
    m.insert("kind".into(), h.kind.as_str().into());
    if h.kind == HopKind::Event {
        m.insert("event".into(), name(h.from).into());
    }
    m.insert("to".into(), name(h.to).into());
    if let Some(a) = g.sites()[h.site as usize].anchor {
        // The query's location convention: `ws:<path>` in the workspace,
        // `dep:<app id>:<path>` in a dependency (human text names the app).
        let file = match g.node_app(h.from) {
            Some(app) if app != g.primary_app() => {
                format!(
                    "dep:{}:{}",
                    g.app_identity(app).0.to_ascii_lowercase(),
                    g.unit(a.unit)
                )
            }
            _ => format!("ws:{}", g.unit(a.unit)),
        };
        m.insert(
            "at".into(),
            serde_json::json!({ "file": file, "line": a.start_line + 1 }),
        );
    }
    if let Some(c) = &h.configured {
        m.insert(
            "configured".into(),
            serde_json::json!({ "entry": c.entry_name(), "arm": &*c.label }),
        );
    }
    serde_json::Value::Object(m)
}

/// A caller's JSON fields beside its `routine`: tier, hop count, whether the
/// witness passes an event, where it enters the workspace, the configured
/// hops and the witness.
#[must_use]
pub fn caller_json(
    g: &SitedGraph,
    c: &DepCaller,
    name: &dyn Fn(NodeIx) -> String,
) -> serde_json::Map<String, serde_json::Value> {
    let mut m = serde_json::Map::new();
    m.insert("tier".into(), c.tier.as_str().into());
    m.insert("hops".into(), c.hops.len().into());
    m.insert("crossedEvent".into(), c.crossed_event.into());
    m.insert(
        "entersAt".into(),
        c.enters_at
            .map(name)
            .map_or(serde_json::Value::Null, Into::into),
    );
    if c.tier == DepTier::Configured {
        m.insert(
            "configuredVia".into(),
            c.hops
                .iter()
                .filter_map(|h| h.configured.as_ref())
                .map(|a| serde_json::json!({ "entry": a.entry_name(), "arm": &*a.label }))
                .collect::<Vec<_>>()
                .into(),
        );
    }
    m.insert(
        "witness".into(),
        c.hops
            .iter()
            .map(|h| hop_json(g, h, name))
            .collect::<Vec<_>>()
            .into(),
    );
    m
}

/// The witness in one line: ` -> X -[event]-> Y -[run]-> Z …` (an event hop
/// leaves the publisher, the routine before it), folded to its first and
/// last 3 hops unless `all`.
#[must_use]
pub fn witness_human(c: &DepCaller, name: &dyn Fn(NodeIx) -> String, all: bool) -> String {
    let hop = |h: &Hop| {
        let arrow = match (&h.configured, h.kind) {
            (Some(a), _) => format!(" -[configured: {}]->", a.label),
            (None, HopKind::Event) => " -[event]->".to_string(),
            (None, HopKind::Run) => " -[run]->".to_string(),
            (None, HopKind::RecordTrigger) => " -[record trigger]->".to_string(),
            (None, HopKind::Interface) => " -[interface]->".to_string(),
            (None, HopKind::Call) => " ->".to_string(),
        };
        format!("{arrow} {}", name(h.to))
    };
    let n = c.hops.len();
    if all || n <= 6 {
        return c.hops.iter().map(hop).collect();
    }
    let mut s: String = c.hops[..3].iter().map(hop).collect();
    s.push_str(&format!(" … {} more …", n - 6));
    s.extend(c.hops[n - 3..].iter().map(hop));
    s
}

/// The configured callers' arms by frequency (first configured hop each),
/// most first: `(entry, arm, callers)`.
#[must_use]
pub fn arm_counts(callers: &[&DepCaller]) -> Vec<(&'static str, String, usize)> {
    let mut counts: HashMap<(&'static str, String), usize> = HashMap::new();
    for c in callers {
        if let Some(a) = c.hops.iter().find_map(|h| h.configured.as_ref()) {
            *counts
                .entry((a.entry_name(), a.label.to_string()))
                .or_default() += 1;
        }
    }
    let mut v: Vec<(&'static str, String, usize)> =
        counts.into_iter().map(|((e, a), n)| (e, a, n)).collect();
    v.sort_by(|a, b| b.2.cmp(&a.2).then_with(|| (a.0, &a.1).cmp(&(b.0, &b.1))));
    v
}

#[cfg(test)]
#[path = "dep_callers_tests.rs"]
mod tests;

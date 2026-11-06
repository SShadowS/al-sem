//! **Who reaches each changed routine** (agent-tasks design §8.4 step 2): R0's
//! reverse walks over the production universe's sited graph.
//!
//! - Per root, [`walk_one`] with one scratch reused across roots and a
//!   visitor that keeps COUNTS and the first few witnesses only
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
//! The walk is R0's own; this module only aggregates.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

use serde_json::{Value, json};

use crate::engine::test_select::select::{direct_callers, display};
use crate::program::impact_graph::reverse::{
    ConeVisitor, EntryKind, ExternalWhy, HiddenChannel, Limit, NodeIx, SiteIx, Tier, Via,
    WalkOptions, WalkScratch, WalkState, walk_multi, walk_one,
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

/// One walk's visitor (one root, or the union).
struct Up<'g> {
    g: &'g SitedGraph,
    keep: Keep,
    /// Per node, its best tier (first reached at its shortest hops per tier).
    best: HashMap<NodeIx, (Tier, u32)>,
    entries: BTreeMap<EntryKind, Firsts>,
    external: BTreeMap<&'static str, Firsts>,
    publishers: Firsts,
    hidden: BTreeMap<&'static str, usize>,
    universal: bool,
    limit: Option<Limit>,
    /// The action-run site that made a page an entry (page node -> site).
    action_site: HashMap<NodeIx, SiteIx>,
    /// The nearest routine ancestors with a stable id, in reach order.
    ancestors: Vec<NodeIx>,
    root_nodes: BTreeSet<NodeIx>,
    states: usize,
    max_hops: u32,
}

impl<'g> Up<'g> {
    fn new(g: &'g SitedGraph, keep: Keep, roots: &[NodeIx]) -> Up<'g> {
        Up {
            g,
            keep,
            best: HashMap::new(),
            entries: BTreeMap::new(),
            external: BTreeMap::new(),
            publishers: Firsts::default(),
            hidden: BTreeMap::new(),
            universal: false,
            limit: None,
            action_site: HashMap::new(),
            ancestors: Vec::new(),
            root_nodes: roots.iter().copied().collect(),
            states: 0,
            max_hops: 0,
        }
    }
}

impl ConeVisitor for Up<'_> {
    fn reach(&mut self, _: u32, s: WalkState, hops: u32, via: Via, _: Option<WalkState>) {
        self.states += 1;
        self.max_hops = self.max_hops.max(hops);
        let e = self.best.entry(s.node).or_insert((s.tier, hops));
        if (s.tier, hops) < *e {
            *e = (s.tier, hops);
        }
        if let Via::Site(site) = via {
            let st = &self.g.sites()[site as usize];
            if st.boundary == Boundary::EventDispatch {
                self.publishers.add(st.from, hops, s.tier);
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
        for k in kinds {
            self.entries.entry(*k).or_default().add(n, hops, s.tier);
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
        if g.is_action_run(site) {
            self.action_site
                .entry(g.sites()[site as usize].from)
                .or_insert(site);
        }
        Some(from.ext)
    }
}

/// Page-action captions, read from the analyzed source (the files of the
/// shown entries only, each parsed once).
pub struct Captions {
    /// Workspace-relative path -> its text.
    texts: HashMap<String, Arc<str>>,
    /// Unit -> per object (lowercased name): its actions `(name lc, caption,
    /// first line, last line)` (0-based).
    parsed: HashMap<String, Vec<ObjectActions>>,
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
            parsed: HashMap::new(),
        }
    }

    fn actions(&mut self, unit: &str) -> &[ObjectActions] {
        // Units and local names spell a workspace file `ws:<rel>` or `<rel>`.
        let unit = unit.strip_prefix("ws:").unwrap_or(unit);
        if !self.parsed.contains_key(unit) {
            let v = self
                .texts
                .get(unit)
                .map(|t| {
                    let f = al_syntax::parse(t);
                    f.objects
                        .iter()
                        .map(|o| {
                            (
                                o.name.to_ascii_lowercase(),
                                o.actions
                                    .iter()
                                    .map(|a| {
                                        (
                                            a.name.to_ascii_lowercase(),
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
                .unwrap_or_default();
            self.parsed.insert(unit.to_string(), v);
        }
        &self.parsed[unit]
    }

    /// The `(action name, caption)` of a page-action entry: an action's
    /// trigger node (its enclosing member names the action), or a page
    /// reached over an action's `RunObject` site (the site's line names it).
    fn of(
        &mut self,
        g: &SitedGraph,
        n: NodeIx,
        site: Option<SiteIx>,
    ) -> Option<(String, Option<String>)> {
        let l = g.local_name(n)?;
        let (object, file) = (l.object.to_ascii_lowercase(), l.file.to_string());
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
    let mut v = json!({"display": display(g, n), "hops": hops, "tier": tier_str(tier)});
    if let Some(id) = g.stable_id(n) {
        v["stableId"] = json!(id);
    }
    if let Some(l) = g.local_name(n) {
        v["file"] = json!(l.file.to_string());
    }
    v
}

/// What one walk found, as JSON.
fn summary(up: &Up<'_>, g: &SitedGraph, captions: &mut Captions) -> Value {
    let (mut local, mut dep, mut open) = (0usize, 0usize, 0usize);
    for (n, (t, _)) in &up.best {
        if up.root_nodes.contains(n) || g.flags(*n) & flag::HUB != 0 {
            continue;
        }
        match t {
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
    json!({
        "states": up.states,
        "maxHops": up.max_hops,
        "ancestors": {"total": local + dep + open, "local": local, "throughDependency": dep, "onlyOpen": open},
        "entries": entries,
        "userOrJobEntries": up.entries.iter().filter(|(k, _)| user_or_job(**k)).map(|(_, f)| f.count()).sum::<usize>(),
        "eventPublishers": {"count": up.publishers.count(), "onlyOpen": up.publishers.only_open(),
                            "first": firsts(&up.publishers)},
        "external": external,
        "hidden": {"universalHub": up.universal, "byChannel": up.hidden},
        "limited": up.limit.map(|l| match l {
            Limit::StateBudget(n) => format!("state budget {n}"),
            Limit::Cancelled => "cancelled".to_string(),
        }),
    })
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
    pub cancel: Option<&'a dyn Fn() -> bool>,
}

/// The up part's JSON over `g`.
#[must_use]
pub fn up(g: &SitedGraph, args: &UpArgs<'_>, captions: &mut Captions) -> Value {
    let opts = WalkOptions {
        assume_no_dependents: args.assume_no_dependents,
        cancel: args.cancel,
        ..Default::default()
    };
    let mut scratch = WalkScratch::default();
    let mut all_nodes: Vec<NodeIx> = Vec::new();
    let mut roots_out = Vec::with_capacity(args.roots.len());
    let started = std::time::Instant::now();
    let mut states = 0usize;
    for id in args.roots {
        let nodes: Vec<NodeIx> = g
            .nodes_with_stable_id(id)
            .into_iter()
            .filter(|&n| g.flags(n) & flag::SYMBOL_ONLY == 0)
            .collect();
        if nodes.is_empty() {
            roots_out.push(json!({"stableId": id, "status": "not-in-graph"}));
            continue;
        }
        all_nodes.extend(&nodes);
        let mut v = Up::new(g, args.keep, &nodes);
        // One root's nodes (its `#if` arms) walk together.
        if nodes.len() == 1 {
            walk_one(g, nodes[0], &opts, &mut scratch, &mut v);
        } else {
            walk_multi(g, &nodes, &opts, &mut scratch, &mut v);
        }
        states += v.states;
        let mut s = summary(&v, g, captions);
        s["stableId"] = json!(id);
        s["status"] = json!("walked");
        s["display"] = json!(display(g, nodes[0]));
        s["nodes"] = json!(nodes.len());
        s["callers"] = callers_json(g, &nodes, args.keep.per_list);
        s["nearestAncestors"] = json!(
            v.ancestors
                .iter()
                .filter_map(|&n| {
                    g.stable_id(n).map(|id| {
                        json!({"stableId": id, "display": display(g, n),
                                                               "hops": v.best.get(&n).map(|b| b.1)})
                    })
                })
                .collect::<Vec<_>>()
        );
        roots_out.push(s);
    }
    all_nodes.sort_unstable();
    all_nodes.dedup();
    let mut u = Up::new(g, args.keep, &all_nodes);
    let ustats = walk_multi(g, &all_nodes, &opts, &mut scratch, &mut u);
    let union = summary(&u, g, captions);
    json!({
        "schema": SCHEMA,
        "version": 1,
        "graph": {"nodes": g.node_count(), "sites": g.site_count()},
        "roots": roots_out,
        "union": union,
        "cost": {"perRootStates": states, "unionStates": ustats.states,
                 "walkMs": started.elapsed().as_millis() as u64,
                 "scratchBytes": scratch.bytes()},
    })
}

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
}

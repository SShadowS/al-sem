//! ENG-125 pass 1 (plan `docs/superpowers/plans/2026-11-17-eng125-multi-source-walk.md`,
//! Task 2): the count fields of the up pass for many roots from ONE [`multi_walk`], kept as
//! root bit masks per reached node and per canonical entry instead of one [`Up`] visitor per
//! root. Each root's fields equal what its own `Up` and [`summary`] give (`up_multi_tests`
//! compares every field, the oracle being the per-root visitor).
//!
//! The walk's events carry a root bit set, so an event costs mask operations, not one call
//! per root. The per-root work left is the end fold over each reached node's set bits
//! (`rows_p1`), and bounded lists (the nearest ancestors, the page-action sites).
//!
//! Pass 2 (the witness lists: `first`, `firstByKind`, `nearest`) is not here yet.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};

use super::{
    Keep, Limit, POPULATION, POPULATIONS, Population, Shared, by_relation_of, hub_locations,
    share_json, site_bits, user_or_job,
};
use crate::engine::test_select::select::display;
use crate::fxhash::FxHashMap;
use crate::program::impact_graph::multi_walk::{MultiSink, ones};
use crate::program::impact_graph::reverse::{
    EntryKind, ExternalWhy, HiddenChannel, NodeIx, SiteIx, Tier, Via, WalkOptions, WalkState,
    WalkStats, entry_kinds, external_whys, tier_over,
};
use crate::program::impact_graph::sited::{Boundary, SitedGraph, flag};

// The root-mask families of a reached node, `w` words each.
const REACHED: usize = 0;
/// `TIER + tier`: a state of that tier.
const TIER: usize = 1;
/// `CLASS + 2 * population + local`: a state of that class whose tier is (1) or is not (0)
/// `Local` (`by_relation`'s key, the class order's first parts).
const CLASS: usize = 4;
/// The page-action entry an action-run site makes: at all / over a path that is not only open.
const PA_ANY: usize = 16;
const PA_NON_OPEN: usize = 17;
/// An event-dispatch arrival makes the node a publisher: at all / not only open.
const PUB_ANY: usize = 18;
const PUB_NON_OPEN: usize = 19;
/// A publisher whose event raises one of the root's own nodes.
const DIRECT: usize = 20;
const FAMILIES: usize = 21;

// The families of a reached canonical entry.
const E_REACHED: usize = 0;
const E_CLASS: usize = 1;
const E_FAMILIES: usize = 13;

const NONE: u32 = u32::MAX;

fn set_all(dst: &mut [u64], src: &[u64]) {
    for (d, s) in dst.iter_mut().zip(src) {
        *d |= s;
    }
}

fn has(a: &[u64], k: usize) -> bool {
    a[k / 64] & (1 << (k % 64)) != 0
}

/// A sparse set of `families` root masks per id, `w` words each.
struct Masks {
    w: usize,
    families: usize,
    slot: Vec<u32>,
    ids: Vec<u32>,
    bits: Vec<u64>,
}

impl Masks {
    fn new(w: usize, families: usize, ids: usize) -> Masks {
        Masks {
            w,
            families,
            slot: vec![NONE; ids],
            ids: Vec::new(),
            bits: Vec::new(),
        }
    }

    /// `id`'s word offset (allocated on first use).
    fn of(&mut self, id: u32) -> usize {
        let s = &mut self.slot[id as usize];
        if *s == NONE {
            *s = self.ids.len() as u32;
            self.ids.push(id);
            self.bits
                .resize(self.bits.len() + self.families * self.w, 0);
        }
        *s as usize * self.families * self.w
    }

    fn fam(&mut self, id: u32, f: usize) -> &mut [u64] {
        let o = self.of(id) + f * self.w;
        &mut self.bits[o..o + self.w]
    }

    /// `id`'s family `f`, if `id` has masks.
    fn get(&self, id: u32, f: usize) -> Option<&[u64]> {
        let s = self.slot[id as usize];
        (s != NONE).then(|| {
            let o = (s as usize * self.families + f) * self.w;
            &self.bits[o..o + self.w]
        })
    }

    fn bytes(&self) -> usize {
        self.slot.capacity() * 4 + self.ids.capacity() * 4 + self.bits.capacity() * 8
    }
}

/// One root's nearest ancestors: each node with its best `(tier, hops)` so far.
type Ancestors = Vec<(NodeIx, Tier, u32)>;

/// Pass 1's sink and its masks (see the module doc).
pub(super) struct UpMulti<'g, 'a> {
    g: &'g SitedGraph,
    keep: Keep,
    shared: &'g Shared<'g>,
    opts: &'a WalkOptions<'a>,
    roots: &'a [Vec<NodeIx>],
    w: usize,
    nodes: Masks,
    entries: Masks,
    /// Per root node, the roots it belongs to (`Up::root_nodes`).
    own: FxHashMap<NodeIx, Vec<u64>>,
    root_entry: Vec<BTreeSet<EntryKind>>,
    /// Per page node, each action-run site that first made it an entry for some roots
    /// (`Up::action_site`'s first `or_insert`), in event order, with those roots.
    action_sites: FxHashMap<NodeIx, (Vec<u64>, Vec<(SiteIx, Vec<u64>)>)>,
    ancestors: Vec<Ancestors>,
    /// The roots whose ancestor list is not full yet.
    filling: Vec<u64>,
    /// Per node in some root's ancestor list, those roots.
    watch: FxHashMap<NodeIx, Vec<u64>>,
    limits: Vec<Option<Limit>>,
    scratch: Vec<u64>,
}

impl<'g, 'a> UpMulti<'g, 'a> {
    pub(super) fn new(
        g: &'g SitedGraph,
        keep: Keep,
        shared: &'g Shared<'g>,
        opts: &'a WalkOptions<'a>,
        roots: &'a [Vec<NodeIx>],
    ) -> Self {
        let w = roots.len().div_ceil(64).max(1);
        let mut own: FxHashMap<NodeIx, Vec<u64>> = FxHashMap::default();
        let mut filling = vec![0u64; w];
        for (k, lanes) in roots.iter().enumerate() {
            filling[k / 64] |= 1 << (k % 64);
            for &n in lanes {
                own.entry(n).or_insert_with(|| vec![0; w])[k / 64] |= 1 << (k % 64);
            }
        }
        UpMulti {
            g,
            keep,
            shared,
            opts,
            roots,
            w,
            nodes: Masks::new(w, FAMILIES, g.node_count()),
            entries: Masks::new(w, E_FAMILIES, shared.inv.len()),
            own,
            root_entry: vec![BTreeSet::new(); roots.len()],
            action_sites: FxHashMap::default(),
            ancestors: vec![Vec::new(); roots.len()],
            filling: if keep.ancestors == 0 {
                vec![0; w]
            } else {
                filling
            },
            watch: FxHashMap::default(),
            limits: vec![None; roots.len()],
            scratch: vec![0; w],
        }
    }

    /// The masks' heap bytes (capacities).
    pub(super) fn bytes(&self) -> usize {
        self.nodes.bytes() + self.entries.bytes()
    }

    /// `roots` without the roots `n` belongs to, into `self.scratch`.
    fn not_own(&mut self, n: NodeIx, roots: &[u64]) {
        self.scratch.copy_from_slice(roots);
        if let Some(o) = self.own.get(&n) {
            for (s, o) in self.scratch.iter_mut().zip(o) {
                *s &= !o;
            }
        }
    }

    /// The roots `n` belongs to, of `roots`.
    fn own_of(&self, n: NodeIx, roots: &[u64]) -> Option<Vec<u64>> {
        let o = self.own.get(&n)?;
        let v: Vec<u64> = roots.iter().zip(o).map(|(r, o)| r & o).collect();
        v.iter().any(|&x| x != 0).then_some(v)
    }

    /// A canonical entry reached by a state of `(tier, ext)`, for `roots`.
    fn canon(&mut self, e: u32, tier: Tier, ext: u8, roots: &[u64]) {
        let p = Population::of(tier, ext) as usize;
        let local = usize::from(tier == Tier::Local);
        set_all(self.entries.fam(e, E_REACHED), roots);
        set_all(self.entries.fam(e, E_CLASS + 2 * p + local), roots);
    }

    /// `Up::count_publisher` for `roots`.
    fn count_publisher(&mut self, site: SiteIx, tier: Tier, roots: &[u64]) {
        let st = &self.g.sites()[site as usize];
        if st.boundary != Boundary::EventDispatch {
            return;
        }
        let (from, to) = (st.from, st.to);
        set_all(self.nodes.fam(from, PUB_ANY), roots);
        if tier != Tier::OnlyOpen {
            set_all(self.nodes.fam(from, PUB_NON_OPEN), roots);
        }
        if let Some(direct) = self.own_of(to, roots) {
            set_all(self.nodes.fam(from, DIRECT), &direct);
        }
    }
}

impl MultiSink for UpMulti<'_, '_> {
    fn reach(
        &mut self,
        _: u32,
        s: WalkState,
        hops: u32,
        via: Via,
        _: Option<WalkState>,
        roots: &[u64],
    ) {
        let n = s.node;
        let p = Population::of(s.tier, s.ext) as usize;
        let local = usize::from(s.tier == Tier::Local);
        set_all(self.nodes.fam(n, REACHED), roots);
        set_all(self.nodes.fam(n, TIER + s.tier as usize), roots);
        set_all(self.nodes.fam(n, CLASS + 2 * p + local), roots);
        if let Via::Site(site) = via {
            self.count_publisher(site, s.tier, roots);
        }
        // The nearest ancestors: a list member's best (tier, hops); then new members.
        if let Some(watch) = self.watch.get(&n) {
            for k in ones(roots).filter(|&k| has(watch, k)) {
                if let Some(a) = self.ancestors[k].iter_mut().find(|a| a.0 == n)
                    && (s.tier, hops) < (a.1, a.2)
                {
                    (a.1, a.2) = (s.tier, hops);
                }
            }
        }
        if self.g.stable_id(n).is_some() {
            self.not_own(n, roots);
            for (x, f) in self.scratch.iter_mut().zip(&self.filling) {
                *x &= f;
            }
            let cand: Vec<usize> = ones(&self.scratch).collect();
            for k in cand {
                let list = &mut self.ancestors[k];
                if list.iter().any(|a| a.0 == n) {
                    continue;
                }
                list.push((n, s.tier, hops));
                let w = self.w;
                self.watch.entry(n).or_insert_with(|| vec![0; w])[k / 64] |= 1 << (k % 64);
                if list.len() >= self.keep.ancestors {
                    self.filling[k / 64] &= !(1 << (k % 64));
                }
            }
        }
    }
    fn rejoin(&mut self, _: u32, s: WalkState, _: u32, site: SiteIx, roots: &[u64]) {
        self.count_publisher(site, s.tier, roots);
    }
    fn hidden(&mut self, _: u32, _: NodeIx, _: HiddenChannel, _: &[u64]) {
        // Folded from the reached nodes' own channels at the end (`report` calls it once
        // per root and node, for every reached node with a channel).
    }
    fn entry(
        &mut self,
        _: u32,
        n: NodeIx,
        kinds: &[EntryKind],
        s: WalkState,
        _: u32,
        roots: &[u64],
    ) {
        // A node's own kinds, once per state: its kind counts fold from the node's masks at
        // the end; here the roots' own nodes and the canonical entry.
        if let Some(mine) = self.own_of(n, roots) {
            for k in ones(&mine) {
                self.root_entry[k].extend(kinds.iter().copied());
            }
        }
        if let Some(e) = self.shared.inv.of_node(n)
            && !self.shared.changed.contains(&e)
        {
            self.not_own(n, roots);
            let rest = std::mem::take(&mut self.scratch);
            if rest.iter().any(|&x| x != 0) {
                self.canon(e, s.tier, s.ext, &rest);
            }
            self.scratch = rest;
        }
    }
    fn action_entry(&mut self, _: u32, n: NodeIx, s: WalkState, _: u32, roots: &[u64]) {
        if let Some(mine) = self.own_of(n, roots) {
            for k in ones(&mine) {
                self.root_entry[k].insert(EntryKind::PageAction);
            }
        }
        self.not_own(n, roots);
        let rest = std::mem::take(&mut self.scratch);
        set_all(self.nodes.fam(n, PA_ANY), &rest);
        if s.tier != Tier::OnlyOpen {
            set_all(self.nodes.fam(n, PA_NON_OPEN), &rest);
        }
        self.scratch = rest;
    }
    fn external(&mut self, _: u32, _: NodeIx, _: ExternalWhy, _: WalkState, _: u32, _: &[u64]) {
        // Folded from the reached nodes' own flags at the end (once per state, every root).
    }
    fn limit(&mut self, root: usize, l: Limit) {
        self.limits[root] = Some(l);
    }
    fn step(&mut self, g: &SitedGraph, from: WalkState, site: SiteIx, roots: &[u64]) -> Option<u8> {
        let ext = from.ext | site_bits(g, site);
        if g.is_action_run(site) {
            let st = &g.sites()[site as usize];
            let w = self.w;
            let (seen, firsts) = self
                .action_sites
                .entry(st.from)
                .or_insert_with(|| (vec![0; w], Vec::new()));
            let fresh: Vec<u64> = roots.iter().zip(seen.iter()).map(|(r, s)| r & !s).collect();
            if fresh.iter().any(|&x| x != 0) {
                set_all(seen, &fresh);
                firsts.push((site, fresh));
            }
            let tier = tier_over(g, site, from.tier);
            for &e in self.shared.inv.of_site(site) {
                if !self.shared.changed.contains(&e) {
                    self.canon(e, tier, ext, roots);
                }
            }
        }
        Some(ext)
    }
}

/// One root's pass-1 fields, as [`summary`] prints them. `entries`, `external` and
/// `eventPublishers` hold their count parts only (their `first` lists are pass 2's), and
/// `nearest` / `firstByKind` are pass 2's.
pub(super) struct P1Row {
    pub(super) fields: serde_json::Map<String, Value>,
    /// The page-action site that made each page node an entry (`Up::action_site`).
    pub(super) action_site: BTreeMap<NodeIx, SiteIx>,
}

/// Per root, the counts the end fold gathers.
#[derive(Default, Clone)]
struct Acc {
    tiers: [usize; 3],
    by_rel: BTreeMap<(Population, bool), usize>,
    anc_total: usize,
    kinds: BTreeMap<EntryKind, (usize, usize)>,
    external: BTreeMap<&'static str, (usize, usize)>,
    external_code: BTreeMap<&'static str, usize>,
    hidden: BTreeMap<&'static str, usize>,
    universal: bool,
    hubs: BTreeSet<(String, String)>,
    publishers: (usize, usize),
    direct: usize,
}

/// The best class of a node (or entry) for root `k`: the first class with a state, and
/// whether one of its states is `Local`.
fn best_class(has_class: impl Fn(usize) -> bool) -> Option<(Population, bool)> {
    Population::ALL.iter().enumerate().find_map(|(p, &pop)| {
        let local = has_class(2 * p + 1);
        (local || has_class(2 * p)).then_some((pop, local))
    })
}

impl UpMulti<'_, '_> {
    /// Every root's pass-1 fields, from the walk's masks and each root's stats.
    #[allow(clippy::too_many_lines)]
    pub(super) fn rows_p1(&self, stats: &[WalkStats]) -> Vec<P1Row> {
        use crate::engine::vocab;
        let g = self.g;
        let inv = self.shared.inv;
        let w = self.w;
        let n_roots = self.roots.len();
        let mut acc = vec![Acc::default(); n_roots];
        // Nodes, reached-major: each set bit is one (root, node).
        for (i, &node) in self.nodes.ids.iter().enumerate() {
            let base = i * FAMILIES * w;
            let fam = |f: usize| &self.nodes.bits[base + f * w..base + (f + 1) * w];
            let reached = fam(REACHED);
            let f = g.flags(node);
            let hub = f & flag::HUB != 0;
            let own = self.own.get(&node);
            let kinds = entry_kinds(g, node, self.opts);
            let whys: Vec<ExternalWhy> = external_whys(f, self.opts).collect();
            let channels = g.hidden_callers(node);
            let hub_kind = crate::engine::l5::routine_rollup::hub_kind_of(g, node);
            let hub_sites: Vec<(NodeIx, String)> = if hub_kind.is_some() {
                let mut v: Vec<(NodeIx, String)> = hub_locations(g, node).collect();
                v.sort_by(|a, b| a.1.cmp(&b.1));
                v
            } else {
                Vec::new()
            };
            for k in ones(reached) {
                let a = &mut acc[k];
                let mine = own.is_some_and(|o| has(o, k));
                let class = best_class(|c| has(fam(CLASS + c), k));
                let non_open = has(fam(TIER), k) || has(fam(TIER + 1), k);
                if !mine && !hub {
                    let t = if has(fam(TIER), k) {
                        0
                    } else if has(fam(TIER + 1), k) {
                        1
                    } else {
                        2
                    };
                    a.tiers[t] += 1;
                    if let Some(c) = class {
                        *a.by_rel.entry(c).or_default() += 1;
                    }
                    a.anc_total += 1;
                }
                if !mine {
                    for &kind in &kinds {
                        if kind == EntryKind::PageAction {
                            continue;
                        }
                        let e = a.kinds.entry(kind).or_default();
                        e.0 += 1;
                        e.1 += usize::from(!non_open);
                    }
                }
                for &why in &whys {
                    let e = a.external.entry(why.as_str()).or_default();
                    e.0 += 1;
                    e.1 += usize::from(!non_open);
                    if class.is_some_and(|c| c.0 == Population::Code) {
                        *a.external_code.entry(why.as_str()).or_default() += 1;
                    }
                }
                if f & flag::UNIVERSAL != 0 {
                    a.universal = true;
                }
                for ch in channels {
                    *a.hidden.entry(ch).or_default() += 1;
                }
                if let Some(kind) = &hub_kind {
                    let at = hub_sites
                        .iter()
                        .find(|(from, _)| self.nodes.get(*from, REACHED).is_some_and(|r| has(r, k)))
                        .map(|(_, at)| at.clone())
                        .unwrap_or_default();
                    a.hubs.insert((kind.clone(), at));
                }
            }
            // Page actions: the node's own kind (its reached states) and the action-run
            // arrivals, one list per node.
            let static_pa = kinds.contains(&EntryKind::PageAction);
            for (wi, ((r, pa), (pn, (t0, t1)))) in reached
                .iter()
                .zip(fam(PA_ANY))
                .zip(
                    fam(PA_NON_OPEN)
                        .iter()
                        .zip(fam(TIER).iter().zip(fam(TIER + 1))),
                )
                .enumerate()
            {
                let own_w = own.map_or(0, |o| o[wi]);
                let any = if static_pa { (r & !own_w) | pa } else { *pa };
                let non_open = if static_pa {
                    ((t0 | t1) & !own_w) | pn
                } else {
                    *pn
                };
                let mut x = any;
                while x != 0 {
                    let k = wi * 64 + x.trailing_zeros() as usize;
                    x &= x - 1;
                    let e = acc[k].kinds.entry(EntryKind::PageAction).or_default();
                    e.0 += 1;
                    e.1 += usize::from(non_open & (1 << (k % 64)) == 0);
                }
            }
            for k in ones(fam(PUB_ANY)) {
                acc[k].publishers.0 += 1;
                acc[k].publishers.1 += usize::from(!has(fam(PUB_NON_OPEN), k));
            }
            for k in ones(fam(DIRECT)) {
                acc[k].direct += 1;
            }
        }
        // Canonical entries: each root's (entry, best class) list.
        let mut ents: Vec<Vec<(u32, Population, bool)>> = vec![Vec::new(); n_roots];
        for (i, &e) in self.entries.ids.iter().enumerate() {
            let base = i * E_FAMILIES * w;
            let fam = |f: usize| &self.entries.bits[base + f * w..base + (f + 1) * w];
            for k in ones(fam(E_REACHED)) {
                if let Some((p, local)) = best_class(|c| has(fam(E_CLASS + c), k)) {
                    ents[k].push((e, p, local));
                }
            }
        }
        let program = self.shared.program();
        let program_by_kind: serde_json::Map<String, Value> = inv
            .by_kind(|e| !self.shared.changed.contains(&e))
            .into_iter()
            .map(|((_, w), n)| (w.to_string(), json!(n)))
            .collect();
        let noted_program = |bit: u8| {
            (0..inv.len() as u32)
                .filter(|e| inv.note(*e) & bit != 0 && !self.shared.changed.contains(e))
                .count()
        };
        let notes = [
            crate::program::impact_graph::entries::NOTE_UNNAMED_ACTIONS,
            crate::program::impact_graph::entries::NOTE_BACKGROUND_WITHOUT_ON_RUN,
        ];
        let notes_program = notes.map(noted_program);
        (0..n_roots)
            .map(|k| {
                let a = &acc[k];
                let es = &ents[k];
                let mut m = serde_json::Map::new();
                let re = &self.root_entry[k];
                m.insert(
                    "rootEntryKinds".into(),
                    json!(re.iter().map(|k| k.as_str()).collect::<Vec<_>>()),
                );
                m.insert(
                    "rootEntryWord".into(),
                    json!((!re.is_empty()).then(|| vocab::entry_word(re.iter().copied()))),
                );
                m.insert(
                    "rootUserOrJob".into(),
                    json!(re.iter().any(|k| user_or_job(*k))),
                );
                m.insert(
                    "hubs".into(),
                    json!(
                        a.hubs
                            .iter()
                            .map(|(k, at)| json!({"kind": k, "at": at}))
                            .collect::<Vec<_>>()
                    ),
                );
                m.insert("programEntries".into(), json!(program));
                m.insert(
                    "entryBuckets".into(),
                    json!(a.kinds.values().map(|c| c.0).sum::<usize>()),
                );
                m.insert("states".into(), json!(stats[k].states));
                m.insert("maxHops".into(), json!(stats[k].max_hops));
                let [local, dep, open] = a.tiers;
                m.insert(
                    "ancestors".into(),
                    json!({"total": local + dep + open, "local": local, "throughDependency": dep,
                           "onlyOpen": open, "population": POPULATION}),
                );
                m.insert(
                    "entries".into(),
                    json!(
                        EntryKind::ALL
                            .iter()
                            .filter_map(|kind| a.kinds.get(kind).map(|c| (kind, c)))
                            .map(|(kind, c)| json!({"kind": kind.as_str(), "count": c.0,
                                                    "onlyOpen": c.1, "userOrJob": user_or_job(*kind)}))
                            .collect::<Vec<_>>()
                    ),
                );
                m.insert(
                    "userOrJobEntries".into(),
                    json!(
                        a.kinds
                            .iter()
                            .filter(|(kind, _)| user_or_job(**kind))
                            .map(|(_, c)| c.0)
                            .sum::<usize>()
                    ),
                );
                m.insert(
                    "eventPublishers".into(),
                    json!({"count": a.publishers.0, "onlyOpen": a.publishers.1,
                           "direct": {"count": a.direct}}),
                );
                m.insert(
                    "external".into(),
                    json!(
                        a.external
                            .iter()
                            .map(|(why, c)| json!({"why": why, "count": c.0, "onlyOpen": c.1}))
                            .collect::<Vec<_>>()
                    ),
                );
                m.insert(
                    "hidden".into(),
                    json!({"universalHub": a.universal, "byChannel": a.hidden}),
                );
                m.insert(
                    "limited".into(),
                    json!(self.limits[k].map(|l| match l {
                        Limit::StateBudget(n) => format!("state budget {n}"),
                        Limit::Cancelled => "cancelled".to_string(),
                    })),
                );
                // The classified part (`classified`'s counts).
                m.insert("relationRecorded".into(), json!(true));
                let mut ent_rel: BTreeMap<(Population, bool), usize> = BTreeMap::new();
                for &(_, p, local) in es {
                    *ent_rel.entry((p, local)).or_default() += 1;
                }
                m.insert(
                    "byRelation".into(),
                    json!({"ancestors": by_relation_of(&a.by_rel, a.anc_total),
                           "entries": by_relation_of(&ent_rel, es.len())}),
                );
                let (mut by_kind, mut by_context, mut share) = (
                    serde_json::Map::new(),
                    serde_json::Map::new(),
                    serde_json::Map::new(),
                );
                for (key, classes) in POPULATIONS {
                    let mut kinds: BTreeMap<(usize, &str), usize> = BTreeMap::new();
                    let mut contexts: BTreeMap<&str, usize> = BTreeMap::new();
                    let mut mine = 0usize;
                    for &(e, p, _) in es {
                        if !classes.contains(&p) {
                            continue;
                        }
                        mine += 1;
                        let word = inv.word(e);
                        *kinds.entry((vocab::entry_word_rank(word), word)).or_default() += 1;
                        let ctx: BTreeSet<&str> =
                            inv.kinds(e).filter_map(vocab::entry_context).collect();
                        for c in ctx {
                            *contexts.entry(c).or_default() += 1;
                        }
                    }
                    by_kind.insert(
                        key.into(),
                        Value::Object(
                            kinds
                                .iter()
                                .map(|(&(_, w), &n)| (w.to_string(), json!(n)))
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
                    share.insert(key.into(), share_json(mine, program));
                }
                let candidates = es.iter().filter(|(_, p, _)| !p.established()).count();
                share.insert("candidates".into(), share_json(candidates, program));
                share.insert("all".into(), share_json(es.len(), program));
                m.insert("entriesByKind".into(), Value::Object(by_kind));
                m.insert("entriesByContext".into(), Value::Object(by_context));
                m.insert(
                    "programEntriesByKind".into(),
                    Value::Object(program_by_kind.clone()),
                );
                m.insert("programShare".into(), Value::Object(share));
                m.insert(
                    "externalCode".into(),
                    json!(
                        a.external_code
                            .iter()
                            .filter(|(_, n)| **n != 0)
                            .map(|(why, n)| json!({"why": why, "count": n}))
                            .collect::<Vec<_>>()
                    ),
                );
                let reached_note =
                    |bit: u8| es.iter().filter(|(e, _, _)| inv.note(*e) & bit != 0).count();
                m.insert(
                    "entryNotes".into(),
                    json!({
                        "pagesWithUnnamedActions": {"reached": reached_note(notes[0]), "program": notes_program[0]},
                        "backgroundTargetsWithoutOnRun": {"reached": reached_note(notes[1]), "program": notes_program[1]},
                        "unnamedActionRunSites": inv.unnamed_sites(),
                    }),
                );
                m.insert(
                    "nearestAncestors".into(),
                    json!(
                        self.ancestors[k]
                            .iter()
                            .filter_map(|&(n, _, hops)| {
                                g.stable_id(n).map(|id| {
                                    json!({"stableId": id, "display": display(g, n), "hops": Some(hops)})
                                })
                            })
                            .collect::<Vec<_>>()
                    ),
                );
                let action_site = self
                    .action_sites
                    .iter()
                    .filter_map(|(&node, (_, firsts))| {
                        firsts
                            .iter()
                            .find(|(_, bits)| has(bits, k))
                            .map(|&(site, _)| (node, site))
                    })
                    .collect();
                P1Row {
                    fields: m,
                    action_site,
                }
            })
            .collect()
    }
}

#[cfg(test)]
#[path = "up_multi_tests.rs"]
pub(super) mod tests;

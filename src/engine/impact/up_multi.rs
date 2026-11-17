//! ENG-125 (plan `docs/superpowers/plans/2026-11-17-eng125-multi-source-walk.md`, Tasks 2-3):
//! every root's up row from TWO [`multi_walk`]s over all the roots at once, kept as root bit
//! masks, instead of one [`Up`] visitor per root. Each root's [`Row`] equals what its own `Up`
//! gives ([`super::row_of_up`]), so its printed row is the same bytes (`up_multi_tests`).
//!
//! - **Pass 1** ([`UpMulti`]): per reached node and per canonical entry, the roots that reached
//!   it, by tier and by class. The walk's events carry a root bit set, so an event costs mask
//!   operations, not one call per root. Its end fold ([`UpMulti::rows_p1`]) gives every count
//!   field, and the final category of each (root, node) that pass 2 needs.
//! - **Pass 2** ([`UpMultiP2`]): the same walk again, with pass 1's final masks known. It keeps
//!   the witness lists (`first`, `firstByKind`, `nearest`): per root, the best few items by
//!   `(hops, arrival)`, or the best one by `(class, tier rank, hops, arrival)`.
//!   - Arrival order is one counter over every add, as `Up` counts its own. A root's events
//!     come in its own walk's order (Task 1), so comparing two counters of one root compares
//!     their order in that root's walk.
//!   - A list is two partitions, as `Firsts::first` sorts it: arrivals not only open (any
//!     such arrival makes the node not only open), then only open, the latter for nodes whose
//!     final category (pass 1) is only open, and only while pass 1 says the first partition
//!     has fewer than `keep.per_list`.
//!   - A class witness is admitted only for an id whose final class (pass 1) is in the
//!     population, so a worse arrival of an id that belongs elsewhere is never counted.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use super::{
    GroupOut, Keep, Limit, ListOut, NOTES, POPULATIONS, Population, Row, Shared, Witness,
    hub_locations, site_bits,
};
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
/// A publisher whose event raises one of the root's own nodes: at all / not only open.
const DIRECT: usize = 20;
const DIRECT_NON_OPEN: usize = 21;
const FAMILIES: usize = 22;

// The families of a reached canonical entry.
const E_REACHED: usize = 0;
const E_CLASS: usize = 1;
const E_FAMILIES: usize = 13;

const NONE: u32 = u32::MAX;

/// The external reasons, in [`external_whys`]' order (a list index each).
const WHYS: [ExternalWhy; 3] = [
    ExternalWhy::Public,
    ExternalWhy::Protected,
    ExternalWhy::Friend,
];

/// Pass 2's lists: one per entry kind ([`EntryKind::ALL`]), per external why, then the
/// publishers and the direct publishers.
const N_KINDS: usize = EntryKind::ALL.len();
const L_PUB: usize = N_KINDS + WHYS.len();
const L_DIRECT: usize = L_PUB + 1;
const N_LISTS: usize = L_DIRECT + 1;

fn list_of_kind(k: EntryKind) -> usize {
    EntryKind::ALL.iter().position(|&x| x == k).expect("a kind")
}

fn list_of_why(why: ExternalWhy) -> usize {
    N_KINDS + WHYS.iter().position(|&x| x == why).expect("a why")
}

fn set_all(dst: &mut [u64], src: &[u64]) {
    for (d, s) in dst.iter_mut().zip(src) {
        *d |= s;
    }
}

fn has(a: &[u64], k: usize) -> bool {
    a[k / 64] & (1 << (k % 64)) != 0
}

fn any(a: &[u64]) -> bool {
    a.iter().any(|&x| x != 0)
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

    /// Word `wi` of `id`'s family `f` (0 when `id` has no masks).
    fn word(&self, id: u32, f: usize, wi: usize) -> u64 {
        let s = self.slot[id as usize];
        if s == NONE {
            0
        } else {
            self.bits[(s as usize * self.families + f) * self.w + wi]
        }
    }

    fn bytes(&self) -> usize {
        self.slot.capacity() * 4 + self.ids.capacity() * 4 + self.bits.capacity() * 8
    }
}

/// Word `wi` of the roots whose best class of an id is one of `classes`, from its
/// `class(c)` words (`c = 2 * population + local`).
fn final_class_word(classes: &[Population], class: impl Fn(usize) -> u64) -> u64 {
    let mut better = 0u64;
    let mut out = 0u64;
    for (p, pop) in Population::ALL.iter().enumerate() {
        let here = class(2 * p) | class(2 * p + 1);
        if classes.contains(pop) {
            out |= here & !better;
        }
        better |= here;
    }
    out
}

/// One root's nearest ancestors: each node with its best `(tier, hops)` so far.
type Ancestors = Vec<(NodeIx, Tier, u32)>;

/// Per page node: the roots it is an action-run entry of, and each site that first made it
/// one for some roots, with those roots.
type ActionSites = FxHashMap<NodeIx, (Vec<u64>, Vec<(SiteIx, Vec<u64>)>)>;

/// One root's per-population nearest entries: per kind word, the entry and its witness.
type FirstByKind = [BTreeMap<(usize, &'static str), (u32, Witness)>; 5];

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
    action_sites: ActionSites,
    ancestors: Vec<Ancestors>,
    /// The roots whose ancestor list is not full yet.
    filling: Vec<u64>,
    /// Per node in some root's ancestor list, those roots.
    watch: FxHashMap<NodeIx, Vec<u64>>,
    limits: Vec<Option<Limit>>,
    scratch: Vec<u64>,
    /// Per node, whether it has a stable id: 0 not asked yet, 1 yes, 2 no.
    stable: Vec<u8>,
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
            stable: vec![0; g.node_count()],
        }
    }

    /// The masks' heap bytes (capacities).
    pub(super) fn bytes(&self) -> usize {
        self.nodes.bytes() + self.entries.bytes()
    }

    /// Word `wi` of the roots `n` belongs to.
    fn own_word(&self, n: NodeIx, wi: usize) -> u64 {
        self.own.get(&n).map_or(0, |o| o[wi])
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
        any(&v).then_some(v)
    }

    /// A canonical entry reached by a state of `(tier, ext)`, for `roots`.
    fn canon(&mut self, e: u32, tier: Tier, ext: u8, roots: &[u64]) {
        let p = Population::of(tier, ext) as usize;
        let local = usize::from(tier == Tier::Local);
        set_all(self.entries.fam(e, E_REACHED), roots);
        set_all(self.entries.fam(e, E_CLASS + 2 * p + local), roots);
    }

    /// Whether `n` has a stable id (asked once per node).
    fn has_stable_id(&mut self, n: NodeIx) -> bool {
        let c = &mut self.stable[n as usize];
        if *c == 0 {
            *c = if self.g.stable_id(n).is_some() { 1 } else { 2 };
        }
        *c == 1
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
            if tier != Tier::OnlyOpen {
                set_all(self.nodes.fam(from, DIRECT_NON_OPEN), &direct);
            }
        }
    }

    /// Word `wi` of the roots for which node `n`'s list `l` is only open (a node in the list
    /// with no arrival that is not only open): pass 2's second partition. `static_pa`: `n`'s
    /// own kinds hold `page-action` (read only for the page-action list).
    fn open_only_word(&self, l: usize, n: NodeIx, wi: usize, static_pa: bool) -> u64 {
        let w = |f| self.nodes.word(n, f, wi);
        let non_open_tier = w(TIER) | w(TIER + 1);
        let not_own = !self.own_word(n, wi);
        match l {
            L_PUB => w(PUB_ANY) & !w(PUB_NON_OPEN),
            L_DIRECT => w(DIRECT) & !w(DIRECT_NON_OPEN),
            l if l >= N_KINDS => w(REACHED) & !non_open_tier,
            l if EntryKind::ALL[l] == EntryKind::PageAction => {
                let (any_pa, non_open) = if static_pa {
                    (
                        (w(REACHED) & not_own) | w(PA_ANY),
                        (non_open_tier & not_own) | w(PA_NON_OPEN),
                    )
                } else {
                    (w(PA_ANY), w(PA_NON_OPEN))
                };
                any_pa & !non_open
            }
            _ => w(REACHED) & not_own & !non_open_tier,
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
        // `Up::reach`'s order of tests, cheapest first: a root whose list still fills, not
        // its own node, then a stable id (`stable_id` builds a string; cached per node).
        self.not_own(n, roots);
        for (x, f) in self.scratch.iter_mut().zip(&self.filling) {
            *x &= f;
        }
        if any(&self.scratch) && self.has_stable_id(n) {
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
            if any(&rest) {
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
        // `Up::entry` as for any entry: the node's own canonical entry too (a page node has
        // none today; kept so the two paths agree by construction).
        if let Some(e) = self.shared.inv.of_node(n)
            && !self.shared.changed.contains(&e)
            && any(&rest)
        {
            self.canon(e, s.tier, s.ext, &rest);
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
            if any(&fresh) {
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

/// What pass 2 needs from pass 1's end fold, per root (`w` words each).
pub(super) struct Finals {
    /// Per list, the roots whose first partition (not only open) has fewer than
    /// `keep.per_list` nodes: the only-open partition can show.
    open_needed: Vec<Vec<u64>>,
    /// Per population, the roots with no entry of it: its nearest witness is an ancestor.
    no_entries: [Vec<u64>; 5],
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
    /// Every root's [`Row`] with pass 1's parts (every count; no witness list), and what
    /// pass 2 needs.
    #[allow(clippy::too_many_lines)]
    pub(super) fn rows_p1(&self, stats: &[WalkStats]) -> (Vec<Row>, Finals) {
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
            for wi in 0..w {
                let own_w = own.map_or(0, |o| o[wi]);
                let (r, pa, pn) = (reached[wi], fam(PA_ANY)[wi], fam(PA_NON_OPEN)[wi]);
                let non_open_tier = fam(TIER)[wi] | fam(TIER + 1)[wi];
                let any_pa = if static_pa { (r & !own_w) | pa } else { pa };
                let non_open = if static_pa {
                    (non_open_tier & !own_w) | pn
                } else {
                    pn
                };
                let mut x = any_pa;
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
        let mut finals = Finals {
            open_needed: vec![vec![0; w]; N_LISTS],
            no_entries: Default::default(),
        };
        for m in &mut finals.no_entries {
            *m = vec![0; w];
        }
        let per_list = self.keep.per_list;
        let rows = (0..n_roots)
            .map(|k| {
                let a = &acc[k];
                let es = &ents[k];
                let bit = |m: &mut Vec<u64>| m[k / 64] |= 1 << (k % 64);
                let list = |c: (usize, usize)| ListOut {
                    count: c.0,
                    only_open: c.1,
                    first: Vec::new(),
                };
                for (kind, c) in &a.kinds {
                    if c.0 - c.1 < per_list {
                        bit(&mut finals.open_needed[list_of_kind(*kind)]);
                    }
                }
                for why in WHYS {
                    let c = a.external.get(why.as_str()).copied().unwrap_or_default();
                    if c.0 - c.1 < per_list {
                        bit(&mut finals.open_needed[list_of_why(why)]);
                    }
                }
                if a.publishers.0 - a.publishers.1 < per_list {
                    bit(&mut finals.open_needed[L_PUB]);
                }
                // The direct list's only-open count is not printed; pass 2 fills both
                // partitions for it.
                bit(&mut finals.open_needed[L_DIRECT]);
                let mut groups: [GroupOut; 5] = Default::default();
                for (gi, (g_out, (_, classes))) in groups.iter_mut().zip(POPULATIONS).enumerate() {
                    for &(e, p, _) in es {
                        if !classes.contains(&p) {
                            continue;
                        }
                        g_out.mine += 1;
                        let word = inv.word(e);
                        *g_out
                            .kinds
                            .entry((vocab::entry_word_rank(word), word))
                            .or_default() += 1;
                        let ctx: BTreeSet<&str> =
                            inv.kinds(e).filter_map(vocab::entry_context).collect();
                        for c in ctx {
                            *g_out.contexts.entry(c).or_default() += 1;
                        }
                    }
                    if g_out.mine == 0 {
                        bit(&mut finals.no_entries[gi]);
                    }
                }
                let mut ent_rel: BTreeMap<(Population, bool), usize> = BTreeMap::new();
                for &(_, p, local) in es {
                    *ent_rel.entry((p, local)).or_default() += 1;
                }
                let action_site: HashMap<NodeIx, SiteIx> = self
                    .action_sites
                    .iter()
                    .filter_map(|(&node, (_, firsts))| {
                        firsts
                            .iter()
                            .find(|(_, bits)| has(bits, k))
                            .map(|&(site, _)| (node, site))
                    })
                    .collect();
                Row {
                    root_entry: self.root_entry[k].clone(),
                    hubs: a.hubs.clone(),
                    states: stats[k].states,
                    max_hops: stats[k].max_hops,
                    tiers: a.tiers,
                    entries: a.kinds.iter().map(|(&kind, &c)| (kind, list(c))).collect(),
                    action_site,
                    publishers: list(a.publishers),
                    direct: list((a.direct, 0)),
                    external: a.external.iter().map(|(&why, &c)| (why, list(c))).collect(),
                    hidden: a.hidden.clone(),
                    universal: a.universal,
                    limit: self.limits[k],
                    anc_rel: (a.by_rel.clone(), a.anc_total),
                    ent_rel: (ent_rel, es.len()),
                    groups,
                    candidates: es.iter().filter(|(_, p, _)| !p.established()).count(),
                    external_code: a.external_code.clone(),
                    notes: NOTES.map(|bit| {
                        es.iter()
                            .filter(|(e, _, _)| inv.note(*e) & bit != 0)
                            .count()
                    }),
                }
            })
            .collect();
        (rows, finals)
    }

    /// Each root's nearest ancestors with a stable id, and the hops of each one's best tier.
    pub(super) fn nearest_ancestors(&self, k: usize) -> &[(NodeIx, Tier, u32)] {
        &self.ancestors[k]
    }
}

/// One list item: a node with its best `(hops, arrival)` and that arrival's tier.
#[derive(Clone, Copy)]
struct Item {
    node: NodeIx,
    hops: u32,
    tier: Tier,
    at: usize,
}

/// The best `cap` distinct nodes of a partition by `(hops, arrival)`, each node keeping its
/// best (an equal later arrival does not replace it).
///
/// A node left out is never needed later: the worst kept key only gets better, and a node's
/// later arrivals are only worse than its earlier ones unless their hops are smaller, when
/// they compete again.
#[derive(Clone, Default)]
struct Top(Vec<Item>);

impl Top {
    fn offer(&mut self, cap: usize, it: Item) {
        if let Some(x) = self.0.iter_mut().find(|x| x.node == it.node) {
            if it.hops < x.hops {
                *x = it;
            }
            return;
        }
        if self.0.len() < cap {
            self.0.push(it);
            return;
        }
        let (wi, worst) = self
            .0
            .iter()
            .enumerate()
            .max_by_key(|(_, x)| (x.hops, x.at))
            .map(|(i, x)| (i, *x))
            .expect("cap > 0");
        if (it.hops, it.at) < (worst.hops, worst.at) {
            self.0[wi] = it;
        }
    }

    fn sorted(&self) -> Vec<Item> {
        let mut v = self.0.clone();
        v.sort_unstable_by_key(|x| (x.hops, x.at));
        v
    }
}

/// Pass 2's sink: the witness lists, with pass 1's final masks (see the module doc).
pub(super) struct UpMultiP2<'p, 'g, 'a> {
    p1: &'p UpMulti<'g, 'a>,
    finals: &'p Finals,
    /// The last add's arrival number (every add of every root counts once).
    at: usize,
    /// The hop count of the states reached last: `step` follows their reach.
    cur_hops: u32,
    /// Per root, per list, the two partitions: `tops[(k * N_LISTS + l) * 2 + open]`.
    tops: Vec<Top>,
    /// Per root, per population, per kind word: the nearest entry and its witness.
    first_by_kind: Vec<FirstByKind>,
    /// Per root, per population with no entry, its nearest ancestor's witness.
    ancestor: Vec<[Option<Witness>; 5]>,
    adm: Vec<u64>,
    /// Per population, its roots of an entry's arrival (`canon_add`).
    group_adm: Vec<u64>,
}

impl<'p, 'g, 'a> UpMultiP2<'p, 'g, 'a> {
    pub(super) fn new(p1: &'p UpMulti<'g, 'a>, finals: &'p Finals) -> Self {
        let n = p1.roots.len();
        UpMultiP2 {
            p1,
            finals,
            at: 0,
            cur_hops: 0,
            tops: vec![Top::default(); n * N_LISTS * 2],
            first_by_kind: (0..n).map(|_| Default::default()).collect(),
            ancestor: vec![[None; 5]; n],
            adm: vec![0; p1.w],
            group_adm: vec![0; POPULATIONS.len() * p1.w],
        }
    }

    /// An add of node `n` to list `l` at `(hops, tier)`, for `roots` (already without the
    /// roots a list leaves out).
    #[allow(clippy::needless_range_loop)] // the word index also reads masks by node id
    fn list_add(&mut self, l: usize, n: NodeIx, hops: u32, tier: Tier, roots: &[u64]) {
        self.at += 1;
        let it = Item {
            node: n,
            hops,
            tier,
            at: self.at,
        };
        let cap = self.p1.keep.per_list;
        if cap == 0 {
            return;
        }
        let open = tier == Tier::OnlyOpen;
        let static_pa = open
            && l == list_of_kind(EntryKind::PageAction)
            && entry_kinds(self.p1.g, n, self.p1.opts).contains(&EntryKind::PageAction);
        for wi in 0..self.p1.w {
            self.adm[wi] = if open {
                roots[wi]
                    & self.finals.open_needed[l][wi]
                    & self.p1.open_only_word(l, n, wi, static_pa)
            } else {
                roots[wi]
            };
        }
        let adm = std::mem::take(&mut self.adm);
        for k in ones(&adm) {
            self.tops[(k * N_LISTS + l) * 2 + usize::from(open)].offer(cap, it);
        }
        self.adm = adm;
    }

    /// A canonical entry `e` reached by a witness `w` (its `at` set here), for `roots`.
    #[allow(clippy::needless_range_loop)] // the word index also reads masks by entry id
    fn canon_add(&mut self, e: u32, mut w: Witness, roots: &[u64]) {
        self.at += 1;
        w.at = self.at;
        let word = self.p1.shared.inv.word(e);
        let key = (crate::engine::vocab::entry_word_rank(word), word);
        // Per word, the roots whose best class of `e` is each population (one pass over the
        // classes), then each population group's roots.
        let w_n = self.p1.w;
        self.group_adm.resize(POPULATIONS.len() * w_n, 0);
        for wi in 0..w_n {
            let r = roots[wi];
            if r == 0 {
                for gi in 0..POPULATIONS.len() {
                    self.group_adm[gi * w_n + wi] = 0;
                }
                continue;
            }
            let mut better = 0u64;
            let mut best = [0u64; 6];
            for (p, b) in best.iter_mut().enumerate() {
                let here = self.p1.entries.word(e, E_CLASS + 2 * p, wi)
                    | self.p1.entries.word(e, E_CLASS + 2 * p + 1, wi);
                *b = here & !better;
                better |= here;
            }
            for (gi, (_, classes)) in POPULATIONS.iter().enumerate() {
                let mut m = 0u64;
                for (p, pop) in Population::ALL.iter().enumerate() {
                    if classes.contains(pop) {
                        m |= best[p];
                    }
                }
                self.group_adm[gi * w_n + wi] = r & m;
            }
        }
        for gi in 0..POPULATIONS.len() {
            self.adm
                .copy_from_slice(&self.group_adm[gi * w_n..(gi + 1) * w_n]);
            let adm = std::mem::take(&mut self.adm);
            for k in ones(&adm) {
                let f = self.first_by_kind[k][gi].entry(key).or_insert((e, w));
                if (w.key(), w.at) < (f.1.key(), f.1.at) {
                    *f = (e, w);
                }
            }
            self.adm = adm;
        }
    }

    /// The publisher lists for an arrival over `site` at `(hops, tier)`.
    fn publisher(&mut self, site: SiteIx, hops: u32, tier: Tier, roots: &[u64]) {
        let st = &self.p1.g.sites()[site as usize];
        if st.boundary != Boundary::EventDispatch {
            return;
        }
        let (from, to) = (st.from, st.to);
        self.list_add(L_PUB, from, hops, tier, roots);
        if let Some(direct) = self.p1.own_of(to, roots) {
            self.list_add(L_DIRECT, from, hops, tier, &direct);
        }
    }

    /// Fills each root's witness lists into its pass-1 row.
    pub(super) fn fill(&self, rows: &mut [Row]) {
        let cap = self.p1.keep.per_list;
        for (k, row) in rows.iter_mut().enumerate() {
            let list = |l: usize| -> Vec<(u32, Tier, NodeIx)> {
                let base = (k * N_LISTS + l) * 2;
                self.tops[base]
                    .sorted()
                    .into_iter()
                    .chain(self.tops[base + 1].sorted())
                    .take(cap)
                    .map(|x| (x.hops, x.tier, x.node))
                    .collect()
            };
            for (kind, out) in &mut row.entries {
                out.first = list(list_of_kind(*kind));
            }
            for why in WHYS {
                if let Some(out) = row.external.get_mut(why.as_str()) {
                    out.first = list(list_of_why(why));
                }
            }
            row.publishers.first = list(L_PUB);
            row.direct.first = list(L_DIRECT);
            for (gi, g_out) in row.groups.iter_mut().enumerate() {
                g_out.first_by_kind = self.first_by_kind[k][gi].clone();
                g_out.nearest = if g_out.mine > 0 {
                    g_out
                        .first_by_kind
                        .values()
                        .min_by_key(|(_, w)| (w.key(), w.at))
                        .map(|&(e, w)| (Some(e), w))
                } else {
                    self.ancestor[k][gi].map(|w| (None, w))
                };
            }
        }
    }
}

impl MultiSink for UpMultiP2<'_, '_, '_> {
    #[allow(clippy::needless_range_loop)] // the word index also reads masks by node id
    fn reach(
        &mut self,
        _: u32,
        s: WalkState,
        hops: u32,
        via: Via,
        _: Option<WalkState>,
        roots: &[u64],
    ) {
        self.cur_hops = hops;
        // `Up::reach`'s order: the node's class witness, then the publisher.
        self.at += 1;
        let n = s.node;
        let p1 = self.p1;
        if p1.g.flags(n) & flag::HUB == 0 {
            let w = Witness {
                tier: s.tier,
                ext: s.ext,
                hops,
                node: n,
                at: self.at,
            };
            for (gi, (_, classes)) in POPULATIONS.iter().enumerate() {
                for wi in 0..p1.w {
                    self.adm[wi] = roots[wi]
                        & self.finals.no_entries[gi][wi]
                        & !p1.own_word(n, wi)
                        & final_class_word(classes, |c| p1.nodes.word(n, CLASS + c, wi));
                }
                let adm = std::mem::take(&mut self.adm);
                for k in ones(&adm) {
                    let best = &mut self.ancestor[k][gi];
                    if best.is_none_or(|b| (w.key(), w.at) < (b.key(), b.at)) {
                        *best = Some(w);
                    }
                }
                self.adm = adm;
            }
        }
        if let Via::Site(site) = via {
            self.publisher(site, hops, s.tier, roots);
        }
    }
    fn rejoin(&mut self, _: u32, s: WalkState, hops: u32, site: SiteIx, roots: &[u64]) {
        self.publisher(site, hops, s.tier, roots);
    }
    fn entry(
        &mut self,
        _: u32,
        n: NodeIx,
        kinds: &[EntryKind],
        s: WalkState,
        hops: u32,
        roots: &[u64],
    ) {
        // `Up::entry`'s order: each kind's list, then the canonical entry.
        let p1 = self.p1;
        let rest: Vec<u64> = (0..p1.w)
            .map(|wi| roots[wi] & !p1.own_word(n, wi))
            .collect();
        if !any(&rest) {
            return;
        }
        for &kind in kinds {
            self.list_add(list_of_kind(kind), n, hops, s.tier, &rest);
        }
        if let Some(e) = p1.shared.inv.of_node(n)
            && !p1.shared.changed.contains(&e)
        {
            let w = Witness {
                tier: s.tier,
                ext: s.ext,
                hops,
                node: n,
                at: 0,
            };
            self.canon_add(e, w, &rest);
        }
    }
    fn action_entry(&mut self, _: u32, n: NodeIx, s: WalkState, hops: u32, roots: &[u64]) {
        let p1 = self.p1;
        let rest: Vec<u64> = (0..p1.w)
            .map(|wi| roots[wi] & !p1.own_word(n, wi))
            .collect();
        if !any(&rest) {
            return;
        }
        self.list_add(list_of_kind(EntryKind::PageAction), n, hops, s.tier, &rest);
        // As pass 1: `Up::entry`'s canonical add for the node itself.
        if let Some(e) = p1.shared.inv.of_node(n)
            && !p1.shared.changed.contains(&e)
        {
            let w = Witness {
                tier: s.tier,
                ext: s.ext,
                hops,
                node: n,
                at: 0,
            };
            self.canon_add(e, w, &rest);
        }
    }
    fn external(
        &mut self,
        _: u32,
        n: NodeIx,
        why: ExternalWhy,
        s: WalkState,
        hops: u32,
        roots: &[u64],
    ) {
        self.list_add(list_of_why(why), n, hops, s.tier, roots);
    }
    fn step(&mut self, g: &SitedGraph, from: WalkState, site: SiteIx, roots: &[u64]) -> Option<u8> {
        let ext = from.ext | site_bits(g, site);
        if g.is_action_run(site) {
            let st = &g.sites()[site as usize];
            let tier = tier_over(g, site, from.tier);
            let (node, hops) = (st.from, self.cur_hops + 1);
            for &e in self.p1.shared.inv.of_site(site) {
                if !self.p1.shared.changed.contains(&e) {
                    let w = Witness {
                        tier,
                        ext,
                        hops,
                        node,
                        at: 0,
                    };
                    self.canon_add(e, w, roots);
                }
            }
        }
        Some(ext)
    }
}

#[cfg(test)]
#[path = "up_multi_tests.rs"]
pub(super) mod tests;

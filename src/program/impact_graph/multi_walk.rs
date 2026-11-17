//! ENG-125: one breadth-first walk for many logical roots at once (a multi-source,
//! bit-parallel walk; Then et al., VLDB 2014).
//!
//! Every logical root gets exactly the callbacks its own [`walk_multi`] gives a
//! [`ConeVisitor`], in the same order (`multi_walk_tests` compares the traces), but the graph
//! is traversed once per level for all of them. impact's up pass walked 699 roots of CDO one
//! by one: 81.5 M root states over 191,824 distinct ones (x425,
//! `docs/superpowers/notes/2026-11-17-eng125-shared-walk.md`).
//!
//! How it keeps each root's own walk:
//! - A state's visited set is a bit set over the logical roots (`seen`), and so are the
//!   action-run entries per state and the hidden channels per node. A walk's sets were per
//!   walk; here each root has its own bit.
//! - A level is a list of records `(lane, state, from, via, roots)`: the roots that first
//!   reached `state` over `via` from `from`. A lane is a position in a root's node list (a
//!   single walk's `root` argument). The level is sorted by (lane, state), the single walk's
//!   order, so every root keeps its own walk's order. A root is in at most one record per
//!   state (its `seen` bit), so records of one state are independent.
//! - The records of one (lane, state) are reported one after another; then the state's sites
//!   are stepped ONCE for the union of their roots. [`MultiSink::step`] must therefore depend
//!   on `(from, site)` only.
//! - Dropped arrivals wait with their roots and are delivered at the end of the next level,
//!   after the states they arrive at are reported (ENG-128's `rejoin` contract).
//! - The state budget is per root. A root past it gets [`MultiSink::limit`], and its bit
//!   leaves every later event, the waiting rejoins included.
//! - Cancellation is polled every [`CANCEL_EVERY`] records and stops every root that is not
//!   done. Where it cuts may differ from a single walk's cut.
//!
//! [`walk_multi`]: super::reverse::walk_multi
//! [`ConeVisitor`]: super::reverse::ConeVisitor

use super::reverse::{
    CANCEL_EVERY, EntryKind, ExternalWhy, HiddenChannel, Limit, NodeIx, SiteIx, Tier, Via,
    WalkOptions, WalkState, WalkStats, entry_kinds, external_whys, is_source, site_guard,
    state_over, tier_at,
};
use super::sited::{SitedGraph, flag};

/// What a [`multi_walk`] reports. Each method is [`super::reverse::ConeVisitor`]'s, with the
/// logical roots it holds for as a bit set (`roots`, bit k = `roots[k]` of the walk; a set is
/// never empty). `lane` is the single walk's `root` argument.
pub trait MultiSink {
    fn reach(
        &mut self,
        lane: u32,
        s: WalkState,
        hops: u32,
        via: Via,
        from: Option<WalkState>,
        roots: &[u64],
    ) {
        let _ = (lane, s, hops, via, from, roots);
    }
    fn rejoin(&mut self, lane: u32, s: WalkState, hops: u32, site: SiteIx, roots: &[u64]) {
        let _ = (lane, s, hops, site, roots);
    }
    fn hidden(&mut self, lane: u32, n: NodeIx, ch: HiddenChannel, roots: &[u64]) {
        let _ = (lane, n, ch, roots);
    }
    #[allow(clippy::too_many_arguments)]
    fn entry(
        &mut self,
        lane: u32,
        n: NodeIx,
        kinds: &[EntryKind],
        s: WalkState,
        hops: u32,
        roots: &[u64],
    ) {
        let _ = (lane, n, kinds, s, hops, roots);
    }
    #[allow(clippy::too_many_arguments)]
    fn external(
        &mut self,
        lane: u32,
        n: NodeIx,
        why: ExternalWhy,
        s: WalkState,
        hops: u32,
        roots: &[u64],
    ) {
        let _ = (lane, n, why, s, hops, roots);
    }
    fn source(&mut self, lane: u32, s: WalkState, hops: u32, roots: &[u64]) {
        let _ = (lane, s, hops, roots);
    }
    /// Logical root `root` stops here (it gets no later event).
    fn limit(&mut self, root: usize, l: Limit) {
        let _ = (root, l);
    }
    /// [`super::reverse::ConeVisitor::step`], called once for every root in `roots`: the
    /// answer must depend on `(from, site)` alone.
    fn step(&mut self, g: &SitedGraph, from: WalkState, site: SiteIx, roots: &[u64]) -> Option<u8> {
        let _ = (g, site, roots);
        Some(from.ext)
    }
}

/// What a [`multi_walk`] did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MultiStats {
    /// Per logical root, what its own walk would report.
    pub roots: Vec<WalkStats>,
    /// The distinct states the walk met (over every root).
    pub states: usize,
    /// The most records one level held (the next level's, as it was built).
    pub peak_records: usize,
    /// The walk's peak heap bytes (capacities), sampled at each level's end.
    pub peak_bytes: usize,
}

/// One level record: the roots at word offset `bits` of the level's arena reached `s` first
/// over `via` from `from`.
#[derive(Clone, Copy)]
struct Rec {
    lane: u32,
    s: WalkState,
    from: Option<WalkState>,
    via: Via,
    bits: usize,
}

fn via_key(v: Via) -> (u8, u32) {
    match v {
        Via::Root(i) => (0, i),
        Via::Site(s) => (1, s),
        Via::Universal => (2, 0),
    }
}

/// The logical roots in a bit set, in order.
pub fn ones(roots: &[u64]) -> impl Iterator<Item = usize> + '_ {
    roots.iter().enumerate().flat_map(|(i, &w)| {
        let mut w = w;
        std::iter::from_fn(move || {
            (w != 0).then(|| {
                let b = w.trailing_zeros() as usize;
                w &= w - 1;
                i * 64 + b
            })
        })
    })
}

fn any(a: &[u64]) -> bool {
    a.iter().any(|&x| x != 0)
}

fn set(a: &mut [u64], k: usize) {
    a[k / 64] |= 1 << (k % 64);
}

fn clear(a: &mut [u64], k: usize) {
    a[k / 64] &= !(1 << (k % 64));
}

fn has(a: &[u64], k: usize) -> bool {
    a[k / 64] & (1 << (k % 64)) != 0
}

/// The bit sets of the walk's states: per interned state, its `seen` set then its action-run
/// entry set (`w` words each).
/// Dense by node (a node has a few states: tiers, bits, guards), so finding a state is a
/// short scan of its node's list, not a hash.
struct States {
    w: usize,
    /// Per node, its last interned state's slot ([`NONE`]: none).
    head: Vec<u32>,
    /// Per slot, the state and the slot of its node's previous state.
    slots: Vec<(WalkState, u32)>,
    bits: Vec<u64>,
}

const NONE: u32 = u32::MAX;

impl States {
    fn new(w: usize, nodes: usize) -> States {
        States {
            w,
            head: vec![NONE; nodes],
            slots: Vec::new(),
            bits: Vec::new(),
        }
    }

    /// `s`'s word offset (its `seen` set; the action-run set follows it).
    fn of(&mut self, s: WalkState) -> usize {
        let head = &mut self.head[s.node as usize];
        let mut i = *head;
        while i != NONE {
            let (st, prev) = self.slots[i as usize];
            if st == s {
                return i as usize * 2 * self.w;
            }
            i = prev;
        }
        let slot = self.slots.len();
        self.slots.push((s, *head));
        *head = slot as u32;
        self.bits.resize((slot + 1) * 2 * self.w, 0);
        slot * 2 * self.w
    }

    fn len(&self) -> usize {
        self.slots.len()
    }

    fn bytes(&self) -> usize {
        self.bits.capacity() * 8
            + self.head.capacity() * 4
            + self.slots.capacity() * std::mem::size_of::<(WalkState, u32)>()
    }
}

/// A level being built: its records and their bit sets.
#[derive(Default)]
struct Level {
    recs: Vec<Rec>,
    bits: Vec<u64>,
}

impl Level {
    fn push(&mut self, lane: u32, s: WalkState, from: Option<WalkState>, via: Via, roots: &[u64]) {
        let bits = self.bits.len();
        self.bits.extend_from_slice(roots);
        self.recs.push(Rec {
            lane,
            s,
            from,
            via,
            bits,
        });
    }

    fn clear(&mut self) {
        self.recs.clear();
        self.bits.clear();
    }

    fn bytes(&self) -> usize {
        self.recs.capacity() * std::mem::size_of::<Rec>() + self.bits.capacity() * 8
    }
}

/// Dropped arrivals `(lane, state, hops, site)` and their roots.
#[derive(Default)]
struct Rejoins {
    recs: Vec<(u32, WalkState, u32, SiteIx, usize)>,
    bits: Vec<u64>,
}

impl Rejoins {
    fn push(&mut self, lane: u32, s: WalkState, hops: u32, site: SiteIx, roots: &[u64]) {
        let off = self.bits.len();
        self.bits.extend_from_slice(roots);
        self.recs.push((lane, s, hops, site, off));
    }

    /// Delivers every arrival to the roots still `alive`, then empties the list.
    fn deliver(&mut self, w: usize, alive: &[u64], tmp: &mut [u64], sink: &mut dyn MultiSink) {
        for &(lane, s, hops, site, off) in &self.recs {
            for (t, (b, a)) in tmp
                .iter_mut()
                .zip(self.bits[off..off + w].iter().zip(alive))
            {
                *t = b & a;
            }
            if any(tmp) {
                sink.rejoin(lane, s, hops, site, tmp);
            }
        }
        self.recs.clear();
        self.bits.clear();
    }

    fn bytes(&self) -> usize {
        self.recs.capacity() * std::mem::size_of::<(u32, WalkState, u32, SiteIx, usize)>()
            + self.bits.capacity() * 8
    }
}

/// Walks every logical root of `roots` (each a node list, a [`walk_multi`] of its own) at
/// once; see the module doc.
///
/// [`walk_multi`]: super::reverse::walk_multi
#[allow(clippy::too_many_lines)]
pub fn multi_walk(
    g: &SitedGraph,
    roots: &[Vec<NodeIx>],
    opts: &WalkOptions<'_>,
    sink: &mut dyn MultiSink,
) -> MultiStats {
    let w = roots.len().div_ceil(64).max(1);
    let mut out = MultiStats {
        roots: vec![WalkStats::default(); roots.len()],
        ..MultiStats::default()
    };
    let mut alive = vec![0u64; w];
    let mut non_empty = vec![0u64; w];
    for (k, lanes) in roots.iter().enumerate() {
        set(&mut alive, k);
        if !lanes.is_empty() {
            set(&mut non_empty, k);
        }
    }
    let mut states = States::new(w, g.node_count());
    // Per node, the word offset of its hidden-channel set ([`NONE`]: not reached yet).
    let mut hidden = vec![NONE; g.node_count()];
    let mut hidden_bits: Vec<u64> = Vec::new();
    let (mut level, mut next) = (Level::default(), Level::default());
    let (mut rejoin_new, mut rejoin_due) = (Rejoins::default(), Rejoins::default());
    let mut one = vec![0u64; w];
    for (k, lanes) in roots.iter().enumerate() {
        for (i, &r) in lanes.iter().enumerate() {
            let s = WalkState {
                node: r,
                tier: tier_at(g, r, Tier::Local, false),
                crossed_event: false,
                ext: 0,
                guard: 0,
            };
            let off = states.of(s);
            if !has(&states.bits[off..off + w], k) {
                set(&mut states.bits[off..off + w], k);
                one.fill(0);
                set(&mut one, k);
                level.push(i as u32, s, None, Via::Root(i as u32), &one);
            }
        }
    }
    let universal = g
        .universal_hub()
        .filter(|&u| opts.follow_open && !g.sites_into(u).is_empty());
    let (mut rec_roots, mut union, mut tmp) = (vec![0u64; w], vec![0u64; w], vec![0u64; w]);
    let (mut fresh, mut again) = (vec![0u64; w], vec![0u64; w]);
    let mut processed = 0usize;
    let mut hops = 0u32;
    while !level.recs.is_empty() {
        level
            .recs
            .sort_by_key(|r| (r.lane, r.s, r.from, via_key(r.via)));
        let mut i = 0;
        while i < level.recs.len() {
            let (lane, s) = (level.recs[i].lane, level.recs[i].s);
            let mut j = i + 1;
            while j < level.recs.len() && (level.recs[j].lane, level.recs[j].s) == (lane, s) {
                j += 1;
            }
            let f = g.flags(s.node);
            let kinds = entry_kinds(g, s.node, opts);
            union.fill(0);
            let mut r = i;
            while r < j {
                let rec = level.recs[r];
                rec_roots.copy_from_slice(&level.bits[rec.bits..rec.bits + w]);
                // Equal provenance (two roots sharing a node at one lane): one record.
                r += 1;
                while r < j
                    && level.recs[r].from == rec.from
                    && via_key(level.recs[r].via) == via_key(rec.via)
                {
                    let b = level.recs[r].bits;
                    for (x, y) in rec_roots.iter_mut().zip(&level.bits[b..b + w]) {
                        *x |= y;
                    }
                    r += 1;
                }
                for (x, a) in rec_roots.iter_mut().zip(&alive) {
                    *x &= a;
                }
                for wi in 0..w {
                    let mut x = rec_roots[wi];
                    while x != 0 {
                        let k = wi * 64 + x.trailing_zeros() as usize;
                        x &= x - 1;
                        let st = &mut out.roots[k];
                        if st.states >= opts.state_budget {
                            sink.limit(k, Limit::StateBudget(opts.state_budget));
                            st.limited = true;
                            clear(&mut alive, k);
                            clear(&mut rec_roots, k);
                        } else {
                            st.states += 1;
                            st.max_hops = hops;
                        }
                    }
                }
                if !any(&rec_roots) {
                    continue;
                }
                processed += 1;
                if processed.is_multiple_of(CANCEL_EVERY) && opts.cancel.is_some_and(|c| c()) {
                    for k in 0..roots.len() {
                        if has(&alive, k) {
                            sink.limit(k, Limit::Cancelled);
                            out.roots[k].limited = true;
                        }
                    }
                    out.states = states.len();
                    return out;
                }
                sink.reach(lane, s, hops, rec.via, rec.from, &rec_roots);
                let at = &mut hidden[s.node as usize];
                if *at == NONE {
                    *at = hidden_bits.len() as u32;
                    hidden_bits.resize(hidden_bits.len() + w, 0);
                }
                let off = *at as usize;
                for ((t, x), h) in tmp
                    .iter_mut()
                    .zip(&rec_roots)
                    .zip(&mut hidden_bits[off..off + w])
                {
                    *t = x & !*h;
                    *h |= x;
                }
                if any(&tmp) {
                    if f & flag::UNIVERSAL != 0 {
                        sink.hidden(lane, s.node, HiddenChannel::Universal, &tmp);
                    }
                    for ch in g.hidden_callers(s.node) {
                        sink.hidden(lane, s.node, HiddenChannel::Flag(ch), &tmp);
                    }
                }
                if !kinds.is_empty() {
                    sink.entry(lane, s.node, &kinds, s, hops, &rec_roots);
                }
                for why in external_whys(f, opts) {
                    sink.external(lane, s.node, why, s, hops, &rec_roots);
                }
                if is_source(g, s.node, f) {
                    sink.source(lane, s, hops, &rec_roots);
                }
                for (u, x) in union.iter_mut().zip(&rec_roots) {
                    *u |= x;
                }
            }
            i = j;
            if !any(&union) {
                continue;
            }
            for &site in g.sites_into(s.node) {
                let Some(guard) = site_guard(g, s, site, opts) else {
                    continue;
                };
                let Some(ext) = sink.step(g, s, site, &union) else {
                    continue;
                };
                let n = state_over(g, s, site, ext, guard);
                let off = states.of(n);
                if g.is_action_run(site) {
                    let done = &mut states.bits[off + w..off + 2 * w];
                    for ((t, u), d) in tmp.iter_mut().zip(&union).zip(done.iter_mut()) {
                        *t = u & !*d;
                        *d |= u;
                    }
                    if any(&tmp) {
                        sink.entry(lane, n.node, &[EntryKind::PageAction], n, hops + 1, &tmp);
                    }
                }
                let seen = &mut states.bits[off..off + w];
                for (((fr, ag), u), sn) in fresh
                    .iter_mut()
                    .zip(again.iter_mut())
                    .zip(&union)
                    .zip(seen.iter_mut())
                {
                    *fr = u & !*sn;
                    *ag = u & *sn;
                    *sn |= u;
                }
                if any(&fresh) {
                    next.push(lane, n, Some(s), Via::Site(site), &fresh);
                }
                if any(&again) {
                    rejoin_new.push(lane, n, hops + 1, site, &again);
                }
            }
        }
        if hops == 0
            && let Some(u) = universal
        {
            // The universal hub, once: one hop from every root with a node.
            let s = WalkState {
                node: u,
                tier: Tier::OnlyOpen,
                crossed_event: false,
                ext: 0,
                guard: 0,
            };
            let off = states.of(s);
            let seen = &mut states.bits[off..off + w];
            for (((t, a), ne), sn) in tmp
                .iter_mut()
                .zip(&alive)
                .zip(&non_empty)
                .zip(seen.iter_mut())
            {
                *t = a & ne & !*sn;
                *sn |= *t;
            }
            if any(&tmp) {
                next.push(0, s, None, Via::Universal, &tmp);
            }
        }
        // The previous level's dropped arrivals reach states reported by now; this level's
        // wait for the next.
        rejoin_due.deliver(w, &alive, &mut tmp, sink);
        std::mem::swap(&mut rejoin_due, &mut rejoin_new);
        out.peak_records = out.peak_records.max(next.recs.len());
        out.peak_bytes = out.peak_bytes.max(
            states.bytes()
                + hidden.capacity() * 4
                + hidden_bits.capacity() * 8
                + level.bytes()
                + next.bytes()
                + rejoin_new.bytes()
                + rejoin_due.bytes(),
        );
        level.clear();
        std::mem::swap(&mut level, &mut next);
        hops += 1;
    }
    rejoin_due.deliver(w, &alive, &mut tmp, sink);
    out.states = states.len();
    out
}

#[cfg(test)]
#[path = "multi_walk_tests.rs"]
pub(crate) mod tests;

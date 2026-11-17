//! ENG-125 Task 1: every logical root of a [`multi_walk`] gets the exact callback trace of its
//! own [`walk_multi`] (plan `docs/superpowers/plans/2026-11-17-eng125-multi-source-walk.md`,
//! §Event model). The comparison is the whole trace, every method with its arguments, in
//! order, plus each root's [`WalkStats`].

use std::path::PathBuf;

use al_syntax::ir::ObjectKind;

use super::*;
use crate::program::impact_graph::project::tests::{World, sited_of, unknown_route};
use crate::program::impact_graph::reverse::{ConeVisitor, WalkScratch, walk_multi};
use crate::program::node::RoutineNodeId;
use crate::program::resolve::edge::{DispatchShape, EdgeKind};
use crate::snapshot::TrustTier;

/// One callback, with its arguments (`Step` is the visitor's own `step` call).
#[derive(Clone, Debug, PartialEq)]
enum Ev {
    Reach(u32, WalkState, u32, Via, Option<WalkState>),
    Rejoin(u32, WalkState, u32, SiteIx),
    Hidden(u32, NodeIx, HiddenChannel),
    Entry(u32, NodeIx, Vec<EntryKind>, WalkState, u32),
    External(u32, NodeIx, ExternalWhy, WalkState, u32),
    Source(u32, WalkState, u32),
    Step(WalkState, SiteIx),
    Limit(Limit),
}

/// The consumer bits both recorders answer `step` with. `bits` false keeps `from`'s bits (the
/// default); true sets a bit per site and does not follow every seventh site, so the trace
/// covers bits that split states and a step that refuses. Pure in `(from, site)`.
fn answer(bits: bool, from: WalkState, site: SiteIx) -> Option<u8> {
    if !bits {
        return Some(from.ext);
    }
    if site % 7 == 6 {
        return None;
    }
    Some(from.ext | (1 << (site % 3)))
}

/// A single walk's trace.
struct One {
    ev: Vec<Ev>,
    bits: bool,
}

impl ConeVisitor for One {
    fn reach(&mut self, root: u32, s: WalkState, hops: u32, via: Via, from: Option<WalkState>) {
        self.ev.push(Ev::Reach(root, s, hops, via, from));
    }
    fn rejoin(&mut self, root: u32, s: WalkState, hops: u32, site: u32) {
        self.ev.push(Ev::Rejoin(root, s, hops, site));
    }
    fn wants_rejoin(&self) -> bool {
        true
    }
    fn hidden(&mut self, root: u32, n: NodeIx, ch: HiddenChannel) {
        self.ev.push(Ev::Hidden(root, n, ch));
    }
    fn entry(&mut self, root: u32, n: NodeIx, kinds: &[EntryKind], s: WalkState, hops: u32) {
        self.ev.push(Ev::Entry(root, n, kinds.to_vec(), s, hops));
    }
    fn external(&mut self, root: u32, n: NodeIx, why: ExternalWhy, s: WalkState, hops: u32) {
        self.ev.push(Ev::External(root, n, why, s, hops));
    }
    fn source(&mut self, root: u32, s: WalkState, hops: u32) {
        self.ev.push(Ev::Source(root, s, hops));
    }
    fn limit(&mut self, l: Limit) {
        self.ev.push(Ev::Limit(l));
    }
    fn step(&mut self, _: &SitedGraph, from: WalkState, site: SiteIx) -> Option<u8> {
        self.ev.push(Ev::Step(from, site));
        answer(self.bits, from, site)
    }
}

/// A multi walk's traces, one per logical root (an event goes to every root in its set).
struct Many {
    ev: Vec<Vec<Ev>>,
    bits: bool,
}

impl Many {
    fn push(&mut self, roots: &[u64], e: &Ev) {
        assert!(any(roots), "an event for no root: {e:?}");
        for (k, trace) in self.ev.iter_mut().enumerate() {
            if has(roots, k) {
                trace.push(e.clone());
            }
        }
        assert!(
            (self.ev.len()..roots.len() * 64).all(|k| !has(roots, k)),
            "a bit past the last root: {e:?}"
        );
    }
}

impl MultiSink for Many {
    fn reach(
        &mut self,
        lane: u32,
        s: WalkState,
        hops: u32,
        via: Via,
        from: Option<WalkState>,
        roots: &[u64],
    ) {
        self.push(roots, &Ev::Reach(lane, s, hops, via, from));
    }
    fn rejoin(&mut self, lane: u32, s: WalkState, hops: u32, site: SiteIx, roots: &[u64]) {
        self.push(roots, &Ev::Rejoin(lane, s, hops, site));
    }
    fn hidden(&mut self, lane: u32, n: NodeIx, ch: HiddenChannel, roots: &[u64]) {
        self.push(roots, &Ev::Hidden(lane, n, ch));
    }
    fn entry(
        &mut self,
        lane: u32,
        n: NodeIx,
        kinds: &[EntryKind],
        s: WalkState,
        hops: u32,
        roots: &[u64],
    ) {
        self.push(roots, &Ev::Entry(lane, n, kinds.to_vec(), s, hops));
    }
    fn external(
        &mut self,
        lane: u32,
        n: NodeIx,
        why: ExternalWhy,
        s: WalkState,
        hops: u32,
        roots: &[u64],
    ) {
        self.push(roots, &Ev::External(lane, n, why, s, hops));
    }
    fn action_entry(&mut self, lane: u32, n: NodeIx, s: WalkState, hops: u32, roots: &[u64]) {
        self.push(
            roots,
            &Ev::Entry(lane, n, vec![EntryKind::PageAction], s, hops),
        );
    }
    fn source(&mut self, lane: u32, s: WalkState, hops: u32, roots: &[u64]) {
        self.push(roots, &Ev::Source(lane, s, hops));
    }
    fn limit(&mut self, root: usize, l: Limit) {
        self.ev[root].push(Ev::Limit(l));
    }
    fn step(&mut self, _: &SitedGraph, from: WalkState, site: SiteIx, roots: &[u64]) -> Option<u8> {
        self.push(roots, &Ev::Step(from, site));
        answer(self.bits, from, site)
    }
}

/// What one comparison saw, for a test's preconditions.
#[derive(Default)]
pub(crate) struct Seen {
    pub(crate) events: usize,
    pub(crate) rejoins: usize,
    pub(crate) limited: usize,
    pub(crate) page_actions: usize,
    pub(crate) universal: usize,
    pub(crate) guarded: usize,
}

/// Asserts that every logical root of `roots` gets its own walk's trace and stats from one
/// multi walk, with both `step` answers.
pub(crate) fn assert_traces_equal(
    g: &SitedGraph,
    roots: &[Vec<NodeIx>],
    opts: &WalkOptions<'_>,
) -> Seen {
    let mut seen = Seen::default();
    for bits in [false, true] {
        let mut many = Many {
            ev: vec![Vec::new(); roots.len()],
            bits,
        };
        let stats = multi_walk(g, roots, opts, &mut many);
        assert_eq!(stats.roots.len(), roots.len());
        let mut scratch = WalkScratch::default();
        for (k, lanes) in roots.iter().enumerate() {
            let mut one = One {
                ev: Vec::new(),
                bits,
            };
            let st = walk_multi(g, lanes, opts, &mut scratch, &mut one);
            let (a, b) = (&one.ev, &many.ev[k]);
            if a != b {
                let at = a
                    .iter()
                    .zip(b)
                    .position(|(x, y)| x != y)
                    .unwrap_or(a.len().min(b.len()));
                panic!(
                    "root {k} {lanes:?} (bits {bits}): traces differ at event {at} of {}/{}\n  \
                     single: {:?}\n  multi:  {:?}",
                    a.len(),
                    b.len(),
                    &a[at.saturating_sub(2)..(at + 3).min(a.len())],
                    &b[at.saturating_sub(2)..(at + 3).min(b.len())],
                );
            }
            assert_eq!(
                stats.roots[k], st,
                "root {k} {lanes:?} (bits {bits}): stats"
            );
            seen.events += a.len();
            seen.limited += usize::from(st.limited);
            for e in a {
                match e {
                    Ev::Rejoin(..) => seen.rejoins += 1,
                    Ev::Entry(_, _, k, _, _) if k == &[EntryKind::PageAction] => {
                        seen.page_actions += 1;
                    }
                    Ev::Reach(_, s, _, via, _) => {
                        seen.universal += usize::from(*via == Via::Universal);
                        seen.guarded += usize::from(s.guard != 0);
                    }
                    _ => {}
                }
            }
        }
    }
    seen
}

/// The logical roots of a whole graph: every node alone, overlapping pairs (two lanes), a
/// root naming one node twice, an empty root, and every node at once (the union's shape).
pub(crate) fn every_shape(g: &SitedGraph) -> Vec<Vec<NodeIx>> {
    let n = g.node_count() as u32;
    let mut roots: Vec<Vec<NodeIx>> = (0..n).map(|x| vec![x]).collect();
    roots.extend((0..n.saturating_sub(1)).map(|x| vec![x + 1, x]));
    roots.push(vec![0, 0]);
    roots.push(Vec::new());
    roots.push((0..n).collect());
    // A small graph still spans two words; equal roots share their records.
    let mut x = 0;
    while roots.len() <= 64 {
        roots.push(vec![x % n.max(1)]);
        x += 1;
    }
    roots
}

fn fixture_graph(dir: &str) -> SitedGraph {
    let inputs = crate::program::inputs::WorkspaceInputs::read(
        &PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(dir),
        &crate::engine::resident::session::REVISION_READ,
    );
    let p = crate::program::resolved::ResolvedProgram::build(
        &inputs,
        crate::program::resolved::ProgramScope::CrossApp,
    )
    .expect("the fixture builds");
    crate::program::impact_graph::sited_from_program(&p, &|| false).expect("built")
}

/// Two real fixtures, every root shape, with and without open sites: the traces agree, over
/// more than 64 roots (several words), with rejoins, page-action entries and the universal hub
/// in them.
#[test]
fn multi_walk_traces_equal_per_root_walks() {
    let (mut rejoins, mut page_actions) = (0, 0);
    for dir in [
        "tests/fixtures/test-selection-graph",
        "tests/fixtures/test-universe/App",
        "tests/fixtures/test-universe/Test",
    ] {
        let g = fixture_graph(dir);
        let roots = every_shape(&g);
        assert!(
            roots.len() > 64,
            "{dir}: precondition, more than one word of roots"
        );
        let seen = assert_traces_equal(&g, &roots, &WalkOptions::default());
        (rejoins, page_actions) = (rejoins + seen.rejoins, page_actions + seen.page_actions);
        let closed = WalkOptions {
            follow_open: false,
            ..WalkOptions::default()
        };
        assert_traces_equal(&g, &roots, &closed);
    }
    assert!(rejoins > 0, "precondition: a rejoin in the traces");
    assert!(page_actions > 0, "precondition: a page-action entry");
}

/// The table-event guard, stated on the sites: `OnIns` raises `Sub` and `Plain` (its
/// `eventFlow` sites; `Sub`'s carries guard 1), and `Proving` and `Raising` call `OnIns` (the
/// first site proves guard 1). From `Sub`, the publisher's state carries the guard and
/// `Proving` is not crossed; from `Plain` it is. Root `[Sub, Plain]` holds both states of
/// the publisher, one per lane.
#[test]
fn multi_walk_traces_equal_with_a_table_event_guard() {
    let mut w = World::default();
    let a = w.app("W");
    let cu = w.obj(a, ObjectKind::Codeunit, 1, "Cu", TrustTier::Workspace);
    let on_ins = w.rtn(&cu, "OnIns");
    let (sub, plain) = (w.rtn(&cu, "Sub"), w.rtn(&cu, "Plain"));
    let (proving, raising) = (w.rtn(&cu, "Proving"), w.rtn(&cu, "Raising"));
    w.event(&on_ins, &[&sub, &plain]);
    w.call(&proving, &on_ins, 1);
    w.call(&raising, &on_ins, 2);
    let mut g = sited_of(&w, a, &[]);
    let (sub, plain, pubn, proving) = (
        node(&g, &sub),
        node(&g, &plain),
        node(&g, &on_ins),
        node(&g, &proving),
    );
    let (mut flows, mut proofs) = (0, 0);
    for st in g.sites_mut() {
        if st.from == pubn && st.to == sub && st.via() == "eventFlow" {
            st.guard = 1;
            flows += 1;
        } else if st.from == proving && st.to == pubn {
            st.guard = 1;
            proofs += 1;
        }
    }
    assert_eq!(
        (flows, proofs),
        (1, 1),
        "precondition: the guard is on both sites"
    );
    let roots = vec![vec![sub], vec![plain], vec![sub, plain], vec![plain, sub]];
    let seen = assert_traces_equal(&g, &roots, &WalkOptions::default());
    assert!(seen.guarded > 0, "precondition: a guarded state is reached");
}

/// The state budget per root at its boundaries: 0, one below and at each root's own count,
/// and one past the largest. Roots of different sizes hit it at different points of one walk.
#[test]
fn multi_walk_budget_boundaries() {
    let g = fixture_graph("tests/fixtures/test-selection-graph");
    let roots = every_shape(&g);
    // Each root's unlimited state count, in both step modes (the bits mode splits states).
    let sizes: Vec<usize> = [false, true]
        .into_iter()
        .flat_map(|bits| {
            let mut free = Many {
                ev: vec![Vec::new(); roots.len()],
                bits,
            };
            multi_walk(&g, &roots, &WalkOptions::default(), &mut free)
                .roots
                .into_iter()
                .map(|s| s.states)
        })
        .collect();
    let mut budgets: Vec<usize> = vec![0, sizes.iter().max().copied().unwrap_or(0) + 1];
    let mid = sizes
        .iter()
        .copied()
        .filter(|&n| n > 2)
        .min()
        .expect("a root of 3+ states");
    budgets.extend([mid - 1, mid, mid + 1]);
    for budget in budgets {
        let opts = WalkOptions {
            state_budget: budget,
            ..WalkOptions::default()
        };
        let seen = assert_traces_equal(&g, &roots, &opts);
        let want = sizes.iter().filter(|&&n| n > budget).count();
        assert!(
            seen.limited == want,
            "budget {budget}: {} limited, expected {want} over both step modes",
            seen.limited
        );
    }
}

fn node(g: &SitedGraph, r: &RoutineNodeId) -> NodeIx {
    g.routine_node(r).expect("the routine is a node")
}

/// Hand-built worlds for the review-focus shapes: two roots reaching one state from different
/// parents; a two-lane root with arms of unequal length; a publisher reached twice (ENG-128's
/// rejoin); the universal hub, from a root that also reaches it by a call.
#[test]
fn multi_walk_traces_equal_on_the_review_focus_worlds() {
    let mut w = World::default();
    let a = w.app("W");
    let cu = w.obj(a, ObjectKind::Codeunit, 1, "Cu", TrustTier::Workspace);
    let r = |w: &mut World, n: &str| w.rtn(&cu, n);
    let (x, y, p, q, top) = (
        r(&mut w, "X"),
        r(&mut w, "Y"),
        r(&mut w, "P"),
        r(&mut w, "Q"),
        r(&mut w, "Top"),
    );
    // X <- P <- Top and Y <- Q <- Top: roots X and Y reach Top from different parents.
    w.call(&p, &x, 1);
    w.call(&q, &y, 2);
    w.call(&top, &p, 3);
    w.call(&top, &q, 4);
    // A long arm: Y <- L1 <- L2 <- L3 <- Top2, and a short one: X <- Top2.
    let (l1, l2, l3, top2) = (
        r(&mut w, "L1"),
        r(&mut w, "L2"),
        r(&mut w, "L3"),
        r(&mut w, "Top2"),
    );
    w.call(&l1, &y, 5);
    w.call(&l2, &l1, 6);
    w.call(&l3, &l2, 7);
    w.call(&top2, &l3, 8);
    w.call(&top2, &x, 9);
    // A publisher two subscribers of X's cone raise (ENG-128).
    let (pub_e, sa, sb) = (r(&mut w, "OnE"), r(&mut w, "Sa"), r(&mut w, "Sb"));
    w.event(&pub_e, &[&sa, &sb]);
    w.call(&sa, &x, 10);
    w.call(&sb, &p, 11);
    // The universal hub, and a node that calls it.
    let u = r(&mut w, "U");
    w.edge(
        &u,
        EdgeKind::Call,
        DispatchShape::Exact,
        vec![unknown_route()],
        12,
    );
    w.call(&u, &x, 13);
    let g = sited_of(&w, a, &[]);
    let (x, y, p, top) = (node(&g, &x), node(&g, &y), node(&g, &p), node(&g, &top));
    let roots = vec![
        vec![x],
        vec![y],
        vec![x, y],
        vec![y, x],
        vec![p, x],
        vec![top],
        vec![x, x],
        vec![],
    ];
    let seen = assert_traces_equal(&g, &roots, &WalkOptions::default());
    assert!(seen.rejoins > 0, "precondition: a rejoin");
    assert!(seen.universal > 0, "precondition: the universal hub");
    for budget in 0..12 {
        let opts = WalkOptions {
            state_budget: budget,
            ..WalkOptions::default()
        };
        assert_traces_equal(&g, &roots, &opts);
    }
}

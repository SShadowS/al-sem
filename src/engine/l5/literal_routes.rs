//! Agent-tasks E2 (design `docs/superpowers/specs/2026-11-05-agent-tasks-design.md`
//! §11, review item a15): several distinct routes from one root to one site.
//!
//! [`LiteralReach::live_tree`] keeps ONE parent per `(routine, bindings)`
//! state, the first the search found, so two paths that reconverge on a state
//! leave one trace in it: walking that tree shows one route per state at
//! most. Routes are enumerated over the STATE-TRANSITION graph instead
//! ([`StateGraph`]): the same search, the same states in the same order, with
//! every transition it examined kept, live or ruled out by literal arguments.
//!
//! Definitions (each pinned by a fixture in `prove_slice_tests.rs`):
//!
//! - A **route** is a simple path in the state graph from the root's start
//!   state to a TERMINAL state: a state of the site's routine in which the
//!   site can run under the known values. No state repeats on a route, so
//!   recursion is followed only while it changes the known values: a routine
//!   may appear twice with different bindings, a `(routine, bindings)` state
//!   may not. A transition back to a state already on the route is counted
//!   (`repeats`), not followed.
//! - A route ends at its FIRST terminal state: a path that passed a terminal
//!   state earlier already ran the site there, and is that shorter route.
//! - Two routes are **distinct** when their sequences of (site, callee)
//!   differ. A second path with the same sequence (parallel edges of
//!   different kinds) is counted as a duplicate, not listed.
//! - **Order**: fewest hops first, then by the sequence of (site id, callee
//!   id), step by step. The search is best-first over partial routes, ranked
//!   by (hops so far + the fewest hops left to a terminal state, the sequence
//!   so far). The estimate is exact on the graph and never more than a simple
//!   completion needs, and a sequence sorts before its extensions, so complete
//!   routes come out in that order.
//! - **Caps**: the graph is the search's own (at most [`STATE_CAP`] states;
//!   past it no route is enumerated). The enumeration stops at the routes
//!   asked for (at most [`ROUTE_CAP`]) or after its edge budget (default
//!   [`ROUTE_EDGE_BUDGET`] candidate edges examined), whichever comes first,
//!   and checks its cancellation every [`CANCEL_EVERY_EDGES`] edges. The
//!   budget is per site.
//! - **Completion**: the enumeration is complete only when no partial route
//!   is left. Then the routes listed are EVERY route under these definitions.
//!   When a cap fires with partial routes left, more routes may exist, and
//!   [`RouteSearch::limit`] says which cap. Nothing here counts the routes
//!   that were not enumerated or the routes the literal arguments rule out:
//!   only the candidate edges examined and why each was rejected
//!   ([`RouteCounts`]). An edge into a state with no live way on to the site
//!   is rejected by literal arguments when its callee reaches the site's
//!   routine with guards ignored (every transition the search examines is
//!   either live or ruled out by them, so that is what blocked it), and as
//!   a plain dead end otherwise.

use super::{Bindings, EdgeSite, LiteralReach, LiveTree, STATE_CAP, State, Step, Stop};
use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, HashSet, VecDeque};

/// The most routes one site lists (`--routes`, design §11).
pub const ROUTE_CAP: usize = 10;

/// The candidate edges one site's enumeration examines at most.
pub const ROUTE_EDGE_BUDGET: usize = 50_000;

/// The enumeration checks its cancellation once per this many edges.
const CANCEL_EVERY_EDGES: usize = 1024;

/// A recorded transition the literal arguments rule out.
const DEAD: u32 = u32::MAX;

/// No state / no transition (the start of a route).
const NONE: u32 = u32::MAX;

fn index(i: usize) -> u32 {
    u32::try_from(i).expect("a state graph index fits u32")
}

/// The transitions a recording search examined
/// ([`LiteralReach::tree_recording`]): per state, in visit order, each
/// out-edge of its routine as (target state, or [`DEAD`]; the edge's position
/// in the routine's out-edges).
#[derive(Default)]
pub(crate) struct Recorder<'a> {
    idx: HashMap<State<'a>, u32>,
    offsets: Vec<u32>,
    trans: Vec<(u32, u32)>,
}

impl<'a> Recorder<'a> {
    pub(super) fn start(&mut self, s: State<'a>) {
        self.idx.insert(s, 0);
    }

    /// The next state in visit order opens its transitions.
    pub(super) fn open_state(&mut self) {
        self.offsets.push(index(self.trans.len()));
    }

    pub(super) fn dead(&mut self, edge: usize) {
        self.trans.push((DEAD, index(edge)));
    }

    pub(super) fn live_new(&mut self, edge: usize, key: State<'a>, at: usize) {
        self.idx.insert(key, index(at));
        self.trans.push((index(at), index(edge)));
    }

    pub(super) fn live_seen(&mut self, edge: usize, key: &State<'a>) {
        let at = *self.idx.get(key).expect("a seen state has an index");
        self.trans.push((at, index(edge)));
    }
}

/// One root's state-transition graph (module doc): the guarded search's tree
/// (the same as [`LiteralReach::live_tree`]'s) and every transition it
/// examined, with the live ones reversed for the distance to a site.
pub(crate) struct StateGraph<'a> {
    pub(crate) tree: LiveTree<'a>,
    /// Per state, its transitions' range in `trans` (one more entry: the end).
    offsets: Vec<u32>,
    trans: Vec<(u32, u32)>,
    /// Per state, its live predecessors' range in `preds`.
    pred_offsets: Vec<u32>,
    preds: Vec<u32>,
    /// The routine graph the root reaches with guards ignored, reversed:
    /// callee → its callers (with repeats). It tells a dead end the literal
    /// arguments made from one the graph has.
    callers: HashMap<&'a str, Vec<&'a str>>,
}

impl<'a> StateGraph<'a> {
    /// The routines that reach `owner` with guards ignored, `owner`
    /// included (within the root's reach).
    fn reaching(&self, owner: &'a str) -> HashSet<&'a str> {
        let mut seen = HashSet::from([owner]);
        let mut q = VecDeque::from([owner]);
        while let Some(v) = q.pop_front() {
            for &u in self.callers.get(v).into_iter().flatten() {
                if seen.insert(u) {
                    q.push_back(u);
                }
            }
        }
        seen
    }

    /// The states the search visited.
    pub(crate) fn states(&self) -> usize {
        self.tree.order.len()
    }

    /// The transitions it examined (live and ruled out).
    pub(crate) fn transitions(&self) -> usize {
        self.trans.len()
    }

    /// The bytes the graph holds beyond the tree (its transitions and their
    /// reverse), for the measurement.
    pub(crate) fn extra_bytes(&self) -> usize {
        let callers: usize = self.callers.values().map(|v| v.capacity() * 16 + 40).sum();
        (self.offsets.capacity() + self.pred_offsets.capacity() + self.preds.capacity()) * 4
            + self.trans.capacity() * 8
            + callers
    }

    fn out(&self, s: u32) -> std::ops::Range<usize> {
        self.offsets[s as usize] as usize..self.offsets[s as usize + 1] as usize
    }

    fn preds_of(&self, s: u32) -> &[u32] {
        &self.preds
            [self.pred_offsets[s as usize] as usize..self.pred_offsets[s as usize + 1] as usize]
    }
}

/// Why an enumeration stopped with partial routes left.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteLimit {
    /// It listed the routes asked for.
    RouteCap,
    /// It examined its edge budget.
    EdgeBudget,
    /// The request was cancelled.
    Cancelled,
}

impl RouteLimit {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            RouteLimit::RouteCap => "route-cap",
            RouteLimit::EdgeBudget => "edge-budget",
            RouteLimit::Cancelled => "cancelled",
        }
    }
}

/// What one site's enumeration examined (module doc). Every count is of
/// candidate EDGES examined from partial routes, an edge counted each time a
/// partial route reaches its source; none is a count of routes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RouteCounts {
    /// Candidate edges examined (the budget counts these).
    pub examined: usize,
    /// Rejected by literal arguments at the edge itself: they rule its site
    /// out.
    pub literal_dead: usize,
    /// Rejected by literal arguments further on: the edge enters a state
    /// from which no live way leads to the site, although its routine
    /// reaches the site's routine with guards ignored (so every way on is a
    /// site the literal arguments rule out, or the site itself is).
    pub literal_blocked: usize,
    /// Rejected: the edge goes back to a state already on the route.
    pub repeats: usize,
    /// Rejected: the edge's callee does not reach the site's routine even
    /// with guards ignored.
    pub no_way: usize,
    /// Complete paths with the sequence of a route already listed.
    pub duplicates: usize,
    /// Partial routes left when a cap fired (0 when complete).
    pub unexplored: usize,
}

/// One site's routes, shortest first, and what the enumeration examined.
#[derive(Debug, Clone, Default)]
pub(crate) struct RouteSearch<'a> {
    pub routes: Vec<Vec<Step<'a>>>,
    pub counts: RouteCounts,
    /// `None`: complete, every route is listed.
    pub limit: Option<RouteLimit>,
}

/// A partial route: its last state, the route it extends, the transition
/// that extends it and its hops.
#[derive(Clone, Copy)]
struct Node {
    state: u32,
    parent: u32,
    trans: u32,
    hops: u32,
}

/// The (site id, callee) sequence a route is ordered and told apart by.
type Key<'a> = Vec<(&'a str, &'a str)>;

fn site_id(site: Option<EdgeSite<'_>>) -> &str {
    match site {
        Some(EdgeSite::Guarded(s) | EdgeSite::Event(s)) => s,
        None => "",
    }
}

impl<'a> LiteralReach<'a> {
    /// The state-transition graph of `root` (module doc); `Err` when the
    /// search hit [`STATE_CAP`] or `cancel` said stop. Its tree is
    /// [`Self::live_tree`]'s.
    pub(crate) fn state_graph(
        &self,
        root: &'a str,
        cancel: &dyn Fn() -> bool,
    ) -> Result<StateGraph<'a>, Stop> {
        let mut rec = Recorder::default();
        let tree = self.tree_recording(
            (root, Bindings::default()),
            STATE_CAP,
            cancel,
            Some(&mut rec),
        )?;
        let Recorder {
            mut offsets, trans, ..
        } = rec;
        offsets.push(index(trans.len()));
        let n = tree.order.len();
        debug_assert_eq!(offsets.len(), n + 1);
        let mut pred_offsets = vec![0u32; n + 1];
        for &(t, _) in trans.iter().filter(|(t, _)| *t != DEAD) {
            pred_offsets[t as usize + 1] += 1;
        }
        for i in 0..n {
            pred_offsets[i + 1] += pred_offsets[i];
        }
        let mut fill = pred_offsets.clone();
        let mut preds = vec![0u32; pred_offsets[n] as usize];
        for s in 0..n {
            for &(t, _) in &trans[offsets[s] as usize..offsets[s + 1] as usize] {
                if t != DEAD {
                    preds[fill[t as usize] as usize] = index(s);
                    fill[t as usize] += 1;
                }
            }
        }
        // The routine graph the root reaches with guards ignored, reversed.
        let mut callers: HashMap<&'a str, Vec<&'a str>> = HashMap::new();
        let mut seen: HashSet<&'a str> = HashSet::from([root]);
        let mut q = VecDeque::from([root]);
        while let Some(f) = q.pop_front() {
            for &(to, _, _, _) in self.out.get(f).into_iter().flatten() {
                callers.entry(to).or_default().push(f);
                if seen.insert(to) {
                    q.push_back(to);
                }
            }
        }
        Ok(StateGraph {
            tree,
            offsets,
            trans,
            pred_offsets,
            preds,
            callers,
        })
    }

    /// Up to `want` distinct routes from `g`'s start to a state where
    /// operation `op` can run, shortest first, examining at most `budget`
    /// candidate edges (module doc).
    pub(crate) fn routes(
        &self,
        g: &StateGraph<'a>,
        op: &str,
        want: usize,
        budget: usize,
        cancel: &dyn Fn() -> bool,
    ) -> RouteSearch<'a> {
        let mut out = RouteSearch::default();
        let n = g.states();
        let Some(owner) = self.op_owner(op) else {
            return out;
        };
        if want == 0 || n == 0 {
            return out;
        }
        // The terminal states, and every state's fewest hops to one.
        let mut terminal = vec![false; n];
        let mut to_go = vec![u32::MAX; n];
        let mut q = VecDeque::new();
        for &i in g.tree.states_of(owner) {
            let (f, b) = &g.tree.order[i as usize];
            if !self.dead_at(f, op, b) {
                terminal[i as usize] = true;
                to_go[i as usize] = 0;
                q.push_back(i);
            }
        }
        while let Some(v) = q.pop_front() {
            for &u in g.preds_of(v) {
                if to_go[u as usize] == u32::MAX {
                    to_go[u as usize] = to_go[v as usize] + 1;
                    q.push_back(u);
                }
            }
        }
        if to_go[0] == u32::MAX {
            return out;
        }
        let reaching = g.reaching(owner);
        let mut arena = vec![Node {
            state: 0,
            parent: NONE,
            trans: NONE,
            hops: 0,
        }];
        let mut heap: BinaryHeap<Reverse<(u32, Key<'a>, u32)>> = BinaryHeap::new();
        heap.push(Reverse((to_go[0], Vec::new(), 0)));
        let mut listed: HashSet<Key<'a>> = HashSet::new();
        'search: while let Some(Reverse((_, key, ni))) = heap.pop() {
            let node = arena[ni as usize];
            if terminal[node.state as usize] {
                if listed.insert(key) {
                    out.routes.push(self.steps_of(g, &arena, ni));
                    if out.routes.len() >= want {
                        if !heap.is_empty() {
                            out.limit = Some(RouteLimit::RouteCap);
                            out.counts.unexplored = heap.len();
                        }
                        break;
                    }
                } else {
                    out.counts.duplicates += 1;
                }
                continue;
            }
            let routine = g.tree.order[node.state as usize].0;
            let edges = self.out.get(routine).map_or(&[][..], Vec::as_slice);
            for ti in g.out(node.state) {
                if out.counts.examined >= budget {
                    out.limit = Some(RouteLimit::EdgeBudget);
                    out.counts.unexplored = heap.len() + 1;
                    break 'search;
                }
                out.counts.examined += 1;
                // At the first edge, then once per CANCEL_EVERY_EDGES.
                if (out.counts.examined - 1).is_multiple_of(CANCEL_EVERY_EDGES) && cancel() {
                    out.limit = Some(RouteLimit::Cancelled);
                    out.counts.unexplored = heap.len() + 1;
                    break 'search;
                }
                let (t, j) = g.trans[ti];
                if t == DEAD {
                    out.counts.literal_dead += 1;
                    continue;
                }
                if to_go[t as usize] == u32::MAX {
                    if reaching.contains(g.tree.order[t as usize].0) {
                        out.counts.literal_blocked += 1;
                    } else {
                        out.counts.no_way += 1;
                    }
                    continue;
                }
                if on_route(&arena, ni, t) {
                    out.counts.repeats += 1;
                    continue;
                }
                let (to, site, _, _) = edges[j as usize];
                let mut k = key.clone();
                k.push((site_id(site), to));
                let hops = node.hops + 1;
                arena.push(Node {
                    state: t,
                    parent: ni,
                    trans: index(ti),
                    hops,
                });
                heap.push(Reverse((
                    hops + to_go[t as usize],
                    k,
                    index(arena.len() - 1),
                )));
            }
        }
        out
    }

    /// The steps of the route ending at partial route `ni`, start first.
    fn steps_of(&self, g: &StateGraph<'a>, arena: &[Node], mut ni: u32) -> Vec<Step<'a>> {
        let mut steps = Vec::new();
        while arena[ni as usize].parent != NONE {
            let node = arena[ni as usize];
            let from = g.tree.order[arena[node.parent as usize].state as usize].0;
            let (_, j) = g.trans[node.trans as usize];
            let (to, site, _, kind) = self.out[from][j as usize];
            steps.push(Step {
                from,
                to,
                site,
                kind,
            });
            ni = node.parent;
        }
        steps.reverse();
        steps
    }
}

/// Whether state `t` is on the route ending at partial route `ni`.
fn on_route(arena: &[Node], mut ni: u32, t: u32) -> bool {
    loop {
        let node = arena[ni as usize];
        if node.state == t {
            return true;
        }
        if node.parent == NONE {
            return false;
        }
        ni = node.parent;
    }
}

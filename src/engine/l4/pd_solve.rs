//! #69 D1 — the transitive producer solve.
//!
//! A PRODUCER is one raw direct table fact whose temp state is
//! `parameter-dependent` in its owner's frame. Producers never enter the
//! shared capability cone: a cone entry is copied unchanged into every
//! ancestor, and a PD temp state means something only in its owner's frame.
//! Instead this solve runs a product-state graph over the TYPED edges (pattern:
//! `db_effect_solver::solve_pd_reachability`). A state `(p, r, t, reqs)` says:
//! in routine `r`'s frame, producer `p`'s write has temp state `t` and happens
//! only under `reqs`. The seed is `(p, owner(p), PD(i0), site reqs of p)`; each
//! typed caller edge applies [`cross_edge`]. No state stops: a known or unknown
//! state keeps propagating, so a guard a caller introduces farther up still
//! applies. Every routine that reaches a producer therefore holds its OWN
//! states for it, and the cone's per-routine walks read them as candidates
//! (`ResolvedByRoutine`, the spec's D1.3 uses (a) and (b)).
//!
//! Termination: the visited set is the state tuple. `t` is drawn from the
//! producer's observed parameter indices and three constants, and `reqs` from
//! the observed literals, so the state space is finite even on cycles.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use super::capability_cone::{
    CapabilityExtra, CapabilityFact, TypedEdgeGraph, TypedOutEdge, fact_pd_index, site_reqs,
};
use super::param_guard::{FrameGuards, Req, across_edge, conjoin};
use super::summary::TempState;
use super::summary_runner::{
    event_param_temp_state, event_publisher_param, pd_temp_state_at_callsite,
    trigger_receiver_temp_state,
};
use crate::engine::census::Tally;
use crate::program::model::workspace::ModelRoutine;

/// Index into `ConeArena::states`.
pub type StateIx = usize;

/// One solve state `(p, r, t, reqs)`: in routine `routine`'s frame, producer
/// `producer`'s write has temp state `t` and happens only under `reqs`.
#[derive(Debug, Clone)]
pub struct ConeState {
    /// Index into `ConeArena::producers`.
    pub producer: usize,
    /// Internal routine id (the cone's id, NOT the stable id).
    pub routine: Arc<str>,
    pub t: TempState,
    pub reqs: Vec<Req>,
    /// True only for the seed `(p, owner(p), PD(i0), site reqs)`.
    pub seed: bool,
    /// Shortest link-path length to the seed (seed = 0).
    pub dist: usize,
    /// Range into `ConeArena::links`: this state's incoming links, deduplicated,
    /// in link rank (edge rank, which within one state is `sort_key` order,
    /// then callee state rank; [`finalize`]).
    pub links: std::ops::Range<usize>,
}

/// A link: "this state is reached from `callee_state` across `edge`"
/// (`edge` indexes `ConeArena::edges`; it is an out-edge of this state's
/// routine, and its `to` is `callee_state`'s routine).
#[derive(Debug, Clone)]
pub struct ConeLink {
    pub callee_state: StateIx,
    pub edge: usize,
}

#[derive(Debug, Clone)]
pub struct Producer {
    /// The owner's raw direct fact (PD temp state, `provenance == "direct"`).
    pub fact: Arc<CapabilityFact>,
    pub owner: Arc<str>,
    pub operation_id: String,
}

/// The whole solve, in cone ids, in canonical order (stage 1b, [`finalize`]):
/// producers, edges, states and links are each in rank order, a rank reads
/// only the solve's own (internal-id) identities, and every index is a
/// position in that order. The indices are storage, never identity: nothing
/// serializes them, and a projection (`snapshot::derive_pd_arena`) walks the
/// states in rank order.
#[derive(Debug, Clone, Default)]
pub struct ConeArena {
    pub producers: Vec<Producer>,
    pub states: Vec<ConeState>,
    pub links: Vec<ConeLink>,
    /// The distinct typed edges the links cross, ONE copy each (D1.4: a link
    /// is an index, not a clone of the edge and its five strings).
    pub edges: Vec<TypedOutEdge>,
    /// Internal routine id → its states (seed included), in state order.
    pub by_routine: HashMap<Arc<str>, Vec<StateIx>>,
}

impl ConeArena {
    /// The arena as text with every index replaced by the identity it points
    /// at: one line per producer, edge and state in storage order, then one
    /// line per routine of `by_routine`, routines sorted. A state is named by
    /// its solve key `(producer, routine, t, reqs)`, an edge by
    /// `from|sort_key`, so two arenas that differ only in numbering give the
    /// same lines up to their order. `aldump --snapshot-memory --pd-dump`
    /// writes it, so a change to the solve's order can be checked against the
    /// one before it as a pure reordering.
    pub fn logical_lines(&self) -> Vec<String> {
        // An edge's `from` is the routine of every state whose link crosses it.
        let mut edge_from: Vec<&str> = vec![""; self.edges.len()];
        for s in &self.states {
            for l in &self.links[s.links.clone()] {
                edge_from[l.edge] = &s.routine;
            }
        }
        let edge = |e: usize| format!("{}|{}", edge_from[e], self.edges[e].sort_key);
        let state = |ix: StateIx| {
            let s = &self.states[ix];
            let p = &self.producers[s.producer];
            format!(
                "{}|{} @{} t={:?} reqs={:?}",
                p.owner, p.operation_id, s.routine, s.t, s.reqs
            )
        };
        let mut out = Vec::with_capacity(
            self.producers.len() + self.edges.len() + self.states.len() + self.by_routine.len(),
        );
        for p in &self.producers {
            out.push(format!("producer {}|{}", p.owner, p.operation_id));
        }
        for e in 0..self.edges.len() {
            out.push(format!("edge {}", edge(e)));
        }
        for (ix, s) in self.states.iter().enumerate() {
            let links: Vec<String> = self.links[s.links.clone()]
                .iter()
                .map(|l| format!("{} <- {}", edge(l.edge), state(l.callee_state)))
                .collect();
            let first_hop = self.links[s.links.clone()]
                .iter()
                .find(|l| !s.seed && self.states[l.callee_state].dist + 1 == s.dist)
                .map_or_else(|| "-".to_string(), |l| edge(l.edge));
            out.push(format!(
                "state {} seed={} dist={} first_hop={} links=[{}]",
                state(ix),
                s.seed,
                s.dist,
                first_hop,
                links.join("; ")
            ));
        }
        let mut routines: Vec<&Arc<str>> = self.by_routine.keys().collect();
        routines.sort();
        for r in routines {
            let states: Vec<String> = self.by_routine[r].iter().map(|&s| state(s)).collect();
            out.push(format!("routine {r}: [{}]", states.join("; ")));
        }
        out
    }
}

/// D1.3: one non-seed state placed at its routine for uses (a) and (b). Its
/// guard requirements are its state's (`ConeState::reqs`).
#[derive(Debug, Clone)]
pub struct ResolvedEntry<'g> {
    /// The spec's D1.3 tuple names it, and it is the entry's only way back to
    /// its state's `reqs` (the tuple's `reqs` live there, not in a copy). No
    /// walk reads guards from an entry today: a guarded entry is folded as
    /// present, the sound direction.
    pub state: StateIx,
    /// The producer's raw fact with `temp_state = t` and
    /// `witness_operation_id` = the producer's op. One `Arc` per
    /// `(producer, t)`, shared by every state with that pair.
    pub rep: Arc<CapabilityFact>,
    pub dist: usize,
    /// The edge of this state's shortest link (an out-edge of its routine),
    /// borrowed from the solved graph.
    pub first_hop: &'g TypedOutEdge,
}

pub type ResolvedByRoutine<'g> = HashMap<Arc<str>, Vec<ResolvedEntry<'g>>>;

/// Publisher → subscriber: an event-dispatch edge whose both ends are known.
/// The subscriber's parameters are the publisher's, by name (engine-switch S8).
fn event_pair<'r>(
    edge: &TypedOutEdge,
    caller: Option<&'r ModelRoutine>,
    callee: Option<&'r ModelRoutine>,
) -> Option<(&'r ModelRoutine, &'r ModelRoutine)> {
    match (edge.kind.as_str(), caller, callee) {
        ("event-dispatch", Some(publisher), Some(subscriber)) => Some((publisher, subscriber)),
        _ => None,
    }
}

/// Where an edge binds the callee's parameters in the caller's frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BoundSite<'e> {
    /// A call site id: bindings keyed by the callee's parameter index.
    Call(&'e str),
    /// The record operation that fires an `implicit-trigger` edge
    /// (`TypedOutEdge::operation_id`): it binds only the trigger's `Rec`.
    Op(&'e str),
}

/// The bound site of an edge, the same allowlist as `substitute_pd_temp_state`
/// (`summary_runner.rs`: `direct | method | implicit-trigger`):
/// - `direct-call` / `variable-typed-call` (the typed twins of `direct` /
///   `method`) bind at their call site;
/// - `implicit-trigger` (#96 D3.3) binds at the record operation that fires the
///   trigger. A table trigger's implicit `Rec` is `PD(RECEIVER_PARAM_INDEX)`, and
///   the record it runs on is that op's receiver.
///
/// Keep the two in step.
fn bound_site<'e, 'r>(
    edge: &'e TypedOutEdge,
    caller: Option<&'r ModelRoutine>,
) -> Option<(BoundSite<'e>, &'r ModelRoutine)> {
    let caller = caller?;
    let site = match edge.kind.as_str() {
        "direct-call" | "variable-typed-call" => BoundSite::Call(edge.callsite.as_deref()?),
        "implicit-trigger" => BoundSite::Op(edge.operation_id.as_deref()?),
        _ => return None,
    };
    Some((site, caller))
}

/// The guard step shared by [`cross_edge`] and `capability_cone::substitute_entry`:
/// carry callee-frame `reqs` across `edge` into the caller's frame
/// ([`across_edge`] on call edges, the by-name re-anchor on event edges;
/// `implicit-trigger` edges, explicitly, and any other edge kind drop them),
/// then `conjoin` with `edge_reqs`, the
/// requirements under which the caller makes this call. `None`: a
/// contradiction, so the fact cannot happen through this edge.
pub(crate) fn carry_reqs(
    reqs: &[Req],
    edge: &TypedOutEdge,
    caller: Option<&ModelRoutine>,
    callee: Option<&ModelRoutine>,
    caller_frame: Option<&FrameGuards>,
    edge_reqs: &[Req],
) -> Option<Vec<Req>> {
    let mut out: Vec<Req> = Vec::new();
    if let Some((publisher, subscriber)) = event_pair(edge, caller, callee) {
        // A subscriber's guard re-anchors on the same-named publisher
        // parameter, when that one is a usable guard there.
        for &(j, v) in reqs {
            let mapped = event_publisher_param(publisher, subscriber, j)
                .map(|pp| pp.index)
                .filter(|i| caller_frame.is_some_and(|f| f.usable.contains(i)));
            if let Some(i) = mapped {
                out = conjoin(&out, &[(i, v)])?;
            }
        }
    } else if let Some((site, caller)) = bound_site(edge, caller) {
        match site {
            BoundSite::Call(cs) => {
                if let Some(cs) = caller.call_sites.iter().find(|c| c.id == cs) {
                    out = across_edge(reqs, cs, caller_frame)?;
                }
            }
            // A trigger has no argument list for a guard to re-anchor on, so
            // its guards are dropped (#96 non-goal, OUTSTANDING:123): fewer
            // requirements is the sound over-approximation.
            BoundSite::Op(_) => {}
        }
    }
    conjoin(&out, edge_reqs)
}

/// D1.2 transition over typed edge `caller → callee`. The temp map applies to
/// PD only: `pd_temp_state_at_callsite` on `direct-call` /
/// `variable-typed-call`, `trigger_receiver_temp_state` on `implicit-trigger`
/// (the firing op's own temp state, the rule L4 applies), `event_param_temp_state`
/// on `event-dispatch`, `Unknown` on any other kind. A known or unknown `t`
/// transfers unchanged; `reqs` go through [`carry_reqs`]. `None`: the
/// transition is contradicted.
pub(crate) fn cross_edge(
    t: &TempState,
    reqs: &[Req],
    edge: &TypedOutEdge,
    caller: Option<&ModelRoutine>,
    callee: Option<&ModelRoutine>,
    caller_frame: Option<&FrameGuards>,
    edge_reqs: &[Req],
) -> Option<(TempState, Vec<Req>)> {
    let t = match t {
        TempState::ParameterDependent(i) => {
            match (event_pair(edge, caller, callee), bound_site(edge, caller)) {
                (Some((publisher, subscriber)), _) => {
                    event_param_temp_state(publisher, subscriber, *i)
                }
                (None, Some((BoundSite::Call(cs), caller))) => {
                    pd_temp_state_at_callsite(caller, cs, *i)
                }
                (None, Some((BoundSite::Op(op), caller))) => {
                    trigger_receiver_temp_state(caller, op, *i)
                }
                _ => TempState::Unknown,
            }
        }
        other => other.clone(),
    };
    let reqs = carry_reqs(reqs, edge, caller, callee, caller_frame, edge_reqs)?;
    Some((t, reqs))
}

/// The producer's raw fact with its temp state set to `t`.
fn rep_for(fact: &CapabilityFact, t: &TempState) -> CapabilityFact {
    let mut rep = fact.clone();
    if let Some(CapabilityExtra::Table { temp_state, .. }) = &mut rep.extra {
        *temp_state = Some(t.to_p());
    }
    rep
}

/// Producer rank (stage 1b): `(owner internal id, operation id)`, the
/// producer's identity (`compose_inherited_cones` makes one producer per pair).
/// Operation ids compare by [`op_id_cmp`].
fn producer_cmp(a: &Producer, b: &Producer) -> std::cmp::Ordering {
    a.owner
        .cmp(&b.owner)
        .then_with(|| op_id_cmp(&a.operation_id, &b.operation_id))
}

/// Operation ids (`"{routine}/opN"`) in source order: the routine part as a
/// string, then the ordinal `N` as a number, then the whole id as a string
/// (so the order stays total for an id of any other shape). As strings,
/// `…/op10` sorts before `…/op8`; the solve's producers came in source order
/// before stage 1b (`direct_raw` order), so on CDO a string order changed the
/// winner of 21,424 equal-candidate ties among a routine's own entries
/// (`capability_cone::candidate_wins` keeps the first), for no reason but the
/// digit count. The ordinal keeps today's winner.
fn op_id_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    fn split(id: &str) -> Option<(&str, u64)> {
        let (routine, n) = id.rsplit_once("/op")?;
        Some((routine, n.parse().ok()?))
    }
    match (split(a), split(b)) {
        (Some(x), Some(y)) => x.cmp(&y).then_with(|| a.cmp(b)),
        _ => a.cmp(b),
    }
}

/// Edge rank (stage 1b): the whole `TypedOutEdge` identity in internal ids,
/// `from` included (an arena edge does not store it). `from` and `sort_key`
/// lead, so the links of one state (all out-edges of its routine) are in
/// `sort_key` order: the order the first hop has always been picked in, and
/// the tie-break [`super::capability_cone`]'s `candidate_wins` applies to a
/// first hop. The remaining fields make the rank total even where two
/// distinct edges would print one `sort_key`.
fn edge_cmp(a: (&str, &TypedOutEdge), b: (&str, &TypedOutEdge)) -> std::cmp::Ordering {
    let ((af, a), (bf, b)) = (a, b);
    af.cmp(bf)
        .then_with(|| a.sort_key.cmp(&b.sort_key))
        .then_with(|| a.kind.cmp(&b.kind))
        .then_with(|| a.to.cmp(&b.to))
        .then_with(|| a.callsite.cmp(&b.callsite))
        .then_with(|| a.event_id.cmp(&b.event_id))
        .then_with(|| a.operation_id.cmp(&b.operation_id))
}

/// The reps of one solve: one `Arc` per `(producer, t)`.
type Reps = HashMap<(usize, TempState), Arc<CapabilityFact>>;

/// Stage 1b: the canonical arena. The solve discovers states, edges and links
/// in an order that follows its inputs (producer order, the graph's hash-map
/// order, discovery); this pass puts all of them in RANK order, which reads
/// only the solve's own identities, and remaps every index:
/// - producers by [`producer_cmp`];
/// - edges by [`edge_cmp`], two edges with one identity becoming one;
/// - states by `(producer rank, dist, routine internal id, t, reqs)`, total
///   because `(producer, routine, t, reqs)` is the solve's state key and
///   `dist` is the shortest distance (a function of the key). Internal ids,
///   never stable ids: two routines with one stable id stay two states;
/// - each state's links by `(edge rank, callee state rank)`, deduplicated;
/// - `by_routine` and the resolved entries in state rank, each entry's first
///   hop its lowest-ranked link to a state at `dist - 1`.
///
/// The one place the order is visible downstream: of a routine's own entries
/// that tie under `capability_cone`'s `candidate_wins` (one fact key, one
/// `dist`, one first-hop `sort_key`), the cone keeps the first. Before stage
/// 1b that was the first discovered; now it is the lowest producer rank, then
/// the most precise `t` (`Known < ParameterDependent < Unknown`). Measured on
/// CDO (`bc3ccb18`): ranking the last tie by discovery instead of `(t, reqs)`
/// reproduces every snapshot section byte for byte, and `(t, reqs)` changes
/// 236 of 581,584 tie groups' winners, 224 to a state with the same rep (no
/// output moves) and 12 from `Unknown` to `Known(false)` of the same producer.
///
/// The indices of the result are positions in that order and nothing else:
/// no index from discovery survives. `first_hop` borrows the graph's edge; of
/// two graph edges with one identity it borrows either (equal in every field,
/// so nothing reads the difference). Every rank key is total, so the unstable
/// sorts below give one result whatever their input order.
///
/// `names` is the solve's interner: every state's `routine` is one of its
/// `Arc`s, so a routine's rank is looked up by pointer and the 2.8 M-state sort
/// on CDO compares integers, not 80-byte ids.
fn finalize<'g>(
    producers: Vec<Producer>,
    states: Vec<ConeState>,
    raw_links: Vec<Vec<ConeLink>>,
    edges: Vec<(&'g str, &'g TypedOutEdge)>,
    names: &HashSet<Arc<str>>,
) -> (ConeArena, ResolvedByRoutine<'g>, Reps) {
    use std::cmp::Ordering;
    /// `rank[old] = new` for a permutation `order` (`order[new] = old`).
    fn ranks(order: &[usize]) -> Vec<usize> {
        let mut rank = vec![0; order.len()];
        for (new, &old) in order.iter().enumerate() {
            rank[old] = new;
        }
        rank
    }

    // The two halves the plan (2026-11-03 L1) measures apart: the ranking
    // sorts, then the one build pass over the ranked states.
    let sort_span = crate::engine::perf_trace::span("cone", "pd.finalize.sort");
    let mut p_order: Vec<usize> = (0..producers.len()).collect();
    p_order.sort_unstable_by(|&a, &b| producer_cmp(&producers[a], &producers[b]));
    debug_assert!(
        p_order
            .windows(2)
            .all(|w| producer_cmp(&producers[w[0]], &producers[w[1]]) != Ordering::Equal),
        "two producers share an identity"
    );
    let p_rank = ranks(&p_order);

    let mut e_order: Vec<usize> = (0..edges.len()).collect();
    e_order.sort_unstable_by(|&a, &b| edge_cmp(edges[a], edges[b]));
    let mut e_rank = vec![0; edges.len()];
    let mut canon_edges: Vec<&'g TypedOutEdge> = Vec::with_capacity(edges.len());
    for (i, &old) in e_order.iter().enumerate() {
        if i == 0 || edge_cmp(edges[e_order[i - 1]], edges[old]) != Ordering::Equal {
            canon_edges.push(edges[old].1);
        }
        e_rank[old] = canon_edges.len() - 1;
    }

    // Routine rank: internal ids in string order, keyed by the interned `Arc`.
    let mut sorted_names: Vec<&Arc<str>> = names.iter().collect();
    sorted_names.sort_unstable();
    let r_rank: HashMap<*const u8, usize> = sorted_names
        .iter()
        .enumerate()
        .map(|(rank, a)| (a.as_ptr(), rank))
        .collect();
    drop(sorted_names);
    // `(producer rank, dist, routine rank)` per state, then `(t, reqs)`.
    let head: Vec<(usize, usize, usize)> = states
        .iter()
        .map(|s| {
            let r = *r_rank
                .get(&s.routine.as_ptr())
                .expect("a state's routine is interned");
            (p_rank[s.producer], s.dist, r)
        })
        .collect();
    drop(r_rank);
    let mut s_order: Vec<StateIx> = (0..states.len()).collect();
    s_order.sort_unstable_by(|&a, &b| {
        let (x, y) = (&states[a], &states[b]);
        head[a]
            .cmp(&head[b])
            .then_with(|| x.t.cmp(&y.t))
            .then_with(|| x.reqs.cmp(&y.reqs))
    });
    drop(head);
    let s_rank = ranks(&s_order);
    drop(sort_span);
    let _build_span = crate::engine::perf_trace::span("cone", "pd.finalize.build");

    let mut producers: Vec<Option<Producer>> = producers.into_iter().map(Some).collect();
    let mut states: Vec<Option<ConeState>> = states.into_iter().map(Some).collect();
    let mut raw_links = raw_links;
    let mut arena = ConeArena {
        producers: p_order
            .iter()
            .map(|&old| producers[old].take().expect("a permutation"))
            .collect(),
        states: Vec::with_capacity(states.len()),
        links: Vec::with_capacity(raw_links.iter().map(Vec::len).sum()),
        edges: canon_edges.iter().map(|&e| e.clone()).collect(),
        by_routine: HashMap::new(),
    };
    for &old in &s_order {
        let mut st = states[old].take().expect("a permutation");
        st.producer = p_rank[st.producer];
        let mut links: Vec<ConeLink> = std::mem::take(&mut raw_links[old])
            .into_iter()
            .map(|l| ConeLink {
                callee_state: s_rank[l.callee_state],
                edge: e_rank[l.edge],
            })
            .collect();
        links.sort_unstable_by_key(|l| (l.edge, l.callee_state));
        links.dedup_by_key(|l| (l.edge, l.callee_state));
        let start = arena.links.len();
        arena.links.extend(links);
        st.links = start..arena.links.len();
        arena
            .by_routine
            .entry(Arc::clone(&st.routine))
            .or_default()
            .push(arena.states.len());
        arena.states.push(st);
    }

    let mut reps: Reps = HashMap::new();
    let mut resolved: ResolvedByRoutine = HashMap::new();
    for (ix, s) in arena.states.iter().enumerate() {
        if s.seed {
            continue;
        }
        let first_hop = canon_edges[arena.links[s.links.clone()]
            .iter()
            .find(|l| arena.states[l.callee_state].dist + 1 == s.dist)
            .expect("a non-seed state has a shortest link")
            .edge];
        let rep = reps
            .entry((s.producer, s.t.clone()))
            .or_insert_with(|| Arc::new(rep_for(&arena.producers[s.producer].fact, &s.t)));
        resolved
            .entry(Arc::clone(&s.routine))
            .or_default()
            .push(ResolvedEntry {
                state: ix,
                rep: Arc::clone(rep),
                dist: s.dist,
                first_hop,
            });
    }
    (arena, resolved, reps)
}

/// The transitive product-state solve over the typed graph's INCOMING edges,
/// returned in canonical (rank) order ([`finalize`]): the result does not
/// depend on the order of `producers`, of the graph's edges or of the model's
/// routines. (Production composes through [`solve_seeded_fresh`].)
#[cfg(test)]
pub(crate) fn solve<'g>(
    producers: Vec<Producer>,
    g: &'g TypedEdgeGraph,
    routines_by_id: &HashMap<&str, &ModelRoutine>,
    frames: &HashMap<String, FrameGuards>,
) -> (ConeArena, ResolvedByRoutine<'g>) {
    let (arena, resolved, _) = solve_seeded(producers, g, routines_by_id, frames, None);
    (arena, resolved)
}

/// The previous revision's solve, for [`solve_seeded`] (context-server
/// design §6.10 "Producers", stage 5): its finalized arena, the per-routine
/// input digests it was solved over ([`super::capability_cone::pd_input_digests`])
/// and the current ones.
pub(crate) struct PdReuse<'a> {
    pub old: &'a ConeArena,
    pub old_digests: &'a HashMap<String, [u8; 32]>,
    pub new_digests: &'a HashMap<String, [u8; 32]>,
}

/// What [`solve_seeded`] reused.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct PdSolveReport {
    /// Producers of this solve.
    pub producers: usize,
    /// Producers whose states were copied from the previous arena.
    pub reused: usize,
    /// Routines whose solve inputs changed (Δ).
    pub delta_routines: usize,
    /// States copied from the previous arena.
    pub reused_states: usize,
}

/// For each of `producers`, the previous arena's index of the same producer
/// when its states may be copied, else `None` (re-solve). Design §6.10:
/// a producer is affected when its owner is reachable from Δ in the NEW
/// graph (Δ = the routines whose input digest changed, appeared or
/// vanished; a state of the producer can only be created, removed or
/// changed at a routine that reaches the owner), when it held a state at a
/// routine of Δ in the OLD arena (a removed edge or a changed row of a
/// routine the producer reached), or when its fact is new or changed. The
/// states never interact across producers, so every other producer's states,
/// links and distances are exactly the previous solve's.
fn reusable(
    producers: &[Producer],
    g: &TypedEdgeGraph,
    reuse: &PdReuse<'_>,
) -> (Vec<Option<usize>>, usize) {
    let mut delta: HashSet<&str> = HashSet::new();
    for (id, d) in reuse.new_digests {
        if reuse.old_digests.get(id) != Some(d) {
            delta.insert(id);
        }
    }
    for id in reuse.old_digests.keys() {
        if !reuse.new_digests.contains_key(id) {
            delta.insert(id);
        }
    }
    // Forward reachability from Δ in the new graph (callers reach callees).
    let mut reach: HashSet<&str> = delta.clone();
    let mut queue: Vec<&str> = delta.iter().copied().collect();
    while let Some(r) = queue.pop() {
        for e in g.outgoing.get(r).map_or(&[][..], Vec::as_slice) {
            if reach.insert(e.to.as_str()) {
                queue.push(e.to.as_str());
            }
        }
    }
    // Producers with a state at a routine of Δ in the old arena.
    let mut old_hit: HashSet<usize> = HashSet::new();
    for r in &delta {
        for &s in reuse.old.by_routine.get(*r).map_or(&[][..], Vec::as_slice) {
            old_hit.insert(reuse.old.states[s].producer);
        }
    }
    let old_ix: HashMap<(&str, &str), usize> = reuse
        .old
        .producers
        .iter()
        .enumerate()
        .map(|(i, p)| ((&*p.owner, p.operation_id.as_str()), i))
        .collect();
    let out = producers
        .iter()
        .map(|p| {
            if reach.contains(&*p.owner) {
                return None;
            }
            let o = *old_ix.get(&(&*p.owner, p.operation_id.as_str()))?;
            (!old_hit.contains(&o) && reuse.old.producers[o].fact == p.fact).then_some(o)
        })
        .collect();
    (out, delta.len())
}

/// [`solve`], copying from `reuse` the states of every producer
/// [`reusable`] leaves untouched and solving only the others; the finalized
/// arena equals a full solve's (the same states, links and distances, then
/// the same canonical order). (Tests; production reads the fresh set too:
/// [`solve_seeded_fresh`].)
#[cfg(test)]
pub(crate) fn solve_seeded<'g>(
    producers: Vec<Producer>,
    g: &'g TypedEdgeGraph,
    routines_by_id: &HashMap<&str, &ModelRoutine>,
    frames: &HashMap<String, FrameGuards>,
    reuse: Option<&PdReuse<'_>>,
) -> (ConeArena, ResolvedByRoutine<'g>, PdSolveReport) {
    let (arena, resolved, report, _) =
        solve_seeded_fresh(producers, g, routines_by_id, frames, reuse);
    (arena, resolved, report)
}

/// The identity `(owner, operation id)` of a producer.
pub(crate) type ProducerId = (Arc<str>, String);

/// Design §6.10 Δ_pd_output, for a fold's per-routine rows: the routines
/// whose own resolved entries may differ between the previous arena `old`
/// and `new`. An entry is what the derived fold and the raw emission read
/// of a non-seed state — its producer's fact with the state's temp state,
/// its distance and its first hop — so a routine's projection is the list,
/// in state order, of `(producer, fact, t, reqs, seed, dist, first-hop edge)`
/// over its states. Only the states of `fresh` producers (solved afresh in
/// `new`) and of `old` producers that `new` does not have can differ: every
/// other producer's states were copied whole. So a routine is in Δ when it
/// holds such a state in either arena and its projection over those
/// producers differs. The state order is producer-major by identity (stage
/// 1b), so equal projections over the affected producers, with the copied
/// producers' entries equal by construction, give equal entry lists.
pub(crate) fn pd_output_delta(
    old: &ConeArena,
    new: &ConeArena,
    fresh: &HashSet<ProducerId>,
) -> HashSet<Arc<str>> {
    let id_of =
        |p: &Producer| -> (Arc<str>, String) { (Arc::clone(&p.owner), p.operation_id.clone()) };
    let new_ids: HashSet<ProducerId> = new.producers.iter().map(id_of).collect();
    let affected_old: Vec<bool> = old
        .producers
        .iter()
        .map(|p| {
            let id = id_of(p);
            fresh.contains(&id) || !new_ids.contains(&id)
        })
        .collect();
    let affected_new: Vec<bool> = new
        .producers
        .iter()
        .map(|p| fresh.contains(&id_of(p)))
        .collect();
    type Proj<'a> = (
        &'a str,
        &'a str,
        &'a CapabilityFact,
        &'a TempState,
        &'a [Req],
        bool,
        usize,
        Option<&'a str>,
    );
    fn project<'a>(a: &'a ConeArena, affected: &[bool]) -> HashMap<&'a str, Vec<Proj<'a>>> {
        let mut out: HashMap<&'a str, Vec<Proj<'a>>> = HashMap::new();
        for s in a.states.iter().filter(|s| affected[s.producer]) {
            let p = &a.producers[s.producer];
            let first_hop = (!s.seed)
                .then(|| {
                    a.links[s.links.clone()]
                        .iter()
                        .find(|l| a.states[l.callee_state].dist + 1 == s.dist)
                        .map(|l| a.edges[l.edge].sort_key.as_str())
                })
                .flatten();
            out.entry(&*s.routine).or_default().push((
                &p.owner,
                &p.operation_id,
                &p.fact,
                &s.t,
                &s.reqs,
                s.seed,
                s.dist,
                first_hop,
            ));
        }
        out
    }
    let (po, pn) = (project(old, &affected_old), project(new, &affected_new));
    po.keys()
        .chain(pn.keys())
        .filter(|r| po.get(*r) != pn.get(*r))
        .map(|r| Arc::from(*r))
        .collect()
}

/// [`solve_seeded`], also returning the identities of the producers it
/// solved afresh (every producer when `reuse` is `None`): the cone store's
/// Δ_pd_output reads the states of exactly these, and of the previous
/// arena's producers that are gone (design §6.10).
pub(crate) fn solve_seeded_fresh<'g>(
    producers: Vec<Producer>,
    g: &'g TypedEdgeGraph,
    routines_by_id: &HashMap<&str, &ModelRoutine>,
    frames: &HashMap<String, FrameGuards>,
    reuse: Option<&PdReuse<'_>>,
) -> (
    ConeArena,
    ResolvedByRoutine<'g>,
    PdSolveReport,
    HashSet<ProducerId>,
) {
    crate::engine::perf_trace::counter_delta("build.pd_solve", 1);
    let mut report = PdSolveReport {
        producers: producers.len(),
        ..Default::default()
    };
    let mut arena = ConeArena {
        producers,
        ..Default::default()
    };
    // A workspace without producers pays nothing.
    if arena.producers.is_empty() {
        return (arena, HashMap::new(), report, HashSet::new());
    }
    let delta_span = crate::engine::perf_trace::span("cone", "pd.delta");
    let mut copy_from: Vec<Option<usize>> = vec![None; arena.producers.len()];
    if let Some(reuse) = reuse {
        let (c, d) = reusable(&arena.producers, g, reuse);
        copy_from = c;
        report.delta_routines = d;
    }
    drop(delta_span);
    let mut incoming: HashMap<&str, Vec<(&'g str, &'g TypedOutEdge)>> = HashMap::new();
    for (from, list) in &g.outgoing {
        for e in list {
            incoming
                .entry(e.to.as_str())
                .or_default()
                .push((from.as_str(), e));
        }
    }
    for list in incoming.values_mut() {
        list.sort_by(|a, b| a.1.sort_key.cmp(&b.1.sort_key).then(a.0.cmp(b.0)));
    }

    // key = (producer, routine, t, reqs): the visited set is the state tuple.
    type Key = (usize, Arc<str>, TempState, Vec<Req>);
    let mut ix_of: HashMap<Key, StateIx> = HashMap::new();
    let mut raw_links: Vec<Vec<ConeLink>> = Vec::new();
    // `ConeArena::edges` while solving: each distinct graph edge, by address,
    // with its `from` (the edge rank reads it, [`finalize`]).
    let mut edges: Vec<(&'g str, &'g TypedOutEdge)> = Vec::new();
    let mut edge_ix: HashMap<*const TypedOutEdge, usize> = HashMap::new();
    let mut names: HashSet<Arc<str>> = HashSet::new();
    let mut intern = |s: &str| -> Arc<str> {
        if let Some(a) = names.get(s) {
            return Arc::clone(a);
        }
        let a: Arc<str> = Arc::from(s);
        names.insert(Arc::clone(&a));
        a
    };
    // States are appended in BFS order, so `arena.states` is its own queue
    // (so `dist` is the shortest distance); [`finalize`] then renumbers.
    fn push(
        arena: &mut ConeArena,
        ix_of: &mut HashMap<Key, StateIx>,
        raw_links: &mut Vec<Vec<ConeLink>>,
        key: Key,
        seed: bool,
        dist: usize,
    ) -> StateIx {
        let ix = arena.states.len();
        let (producer, routine, t, reqs) = key.clone();
        arena.states.push(ConeState {
            producer,
            routine,
            t,
            reqs,
            seed,
            dist,
            links: 0..0,
        });
        raw_links.push(Vec::new());
        ix_of.insert(key, ix);
        ix
    }

    // Stage 5: the reused producers' states, links and edges, copied from the
    // previous arena (each producer's states are one contiguous range there:
    // the canonical order is producer-major). An edge is re-found in the
    // current graph by its full identity; its absence would mean the
    // producer's inputs moved without Δ seeing it, so that producer is solved
    // afresh instead (and counted as not reused).
    if let Some(reuse) = reuse.filter(|_| copy_from.iter().any(Option::is_some)) {
        let copy_span = crate::engine::perf_trace::span("cone", "pd.copy");
        let old = reuse.old;
        let mut start = vec![usize::MAX; old.producers.len()];
        let mut end = vec![0usize; old.producers.len()];
        for (ix, s) in old.states.iter().enumerate() {
            start[s.producer] = start[s.producer].min(ix);
            end[s.producer] = ix + 1;
        }
        type FromEdge<'g> = (&'g str, &'g TypedOutEdge);
        let mut by_from: HashMap<(&str, &str), Vec<FromEdge<'g>>> = HashMap::new();
        for (from, list) in &g.outgoing {
            for e in list {
                by_from
                    .entry((from.as_str(), e.sort_key.as_str()))
                    .or_default()
                    .push((from.as_str(), e));
            }
        }
        // Each old edge is re-found once (an arena edge has one `from`: its
        // identity includes it), then each link maps by index.
        const UNSEEN: usize = usize::MAX;
        const MISSING: usize = usize::MAX - 1;
        let mut new_edge: Vec<usize> = vec![UNSEEN; old.edges.len()];
        let mut map_edge = |from: &str, oe: usize| -> usize {
            if new_edge[oe] == UNSEEN {
                let e = &old.edges[oe];
                new_edge[oe] = by_from
                    .get(&(from, e.sort_key.as_str()))
                    .and_then(|v| {
                        v.iter()
                            .copied()
                            .find(|(f, c)| edge_cmp((f, c), (from, e)) == std::cmp::Ordering::Equal)
                    })
                    .map_or(MISSING, |(f, c)| {
                        *edge_ix.entry(std::ptr::from_ref(c)).or_insert_with(|| {
                            edges.push((f, c));
                            edges.len() - 1
                        })
                    });
            }
            new_edge[oe]
        };
        // The previous revision's routine names, by address, to this solve's.
        let mut names_of: HashMap<*const u8, Arc<str>> = HashMap::new();
        for (p, slot) in copy_from.iter_mut().enumerate() {
            let Some(o) = *slot else { continue };
            let (s0, s1) = (start[o], end[o]);
            debug_assert!(
                s0 < s1 && old.states[s0..s1].iter().all(|s| s.producer == o),
                "a producer's states are one range"
            );
            // First every link's edge in the current graph, then the copy.
            let complete = old.states[s0..s1].iter().all(|st| {
                old.links[st.links.clone()]
                    .iter()
                    .all(|l| map_edge(&st.routine, l.edge) != MISSING)
            });
            if !complete {
                debug_assert!(false, "a reused producer's edge is missing from the graph");
                *slot = None;
                continue;
            }
            let base = arena.states.len();
            for st in &old.states[s0..s1] {
                let links = old.links[st.links.clone()]
                    .iter()
                    .map(|l| ConeLink {
                        callee_state: base + (l.callee_state - s0),
                        edge: map_edge(&st.routine, l.edge),
                    })
                    .collect();
                let routine = names_of
                    .entry(st.routine.as_ptr())
                    .or_insert_with(|| intern(&st.routine));
                arena.states.push(ConeState {
                    producer: p,
                    routine: Arc::clone(routine),
                    t: st.t.clone(),
                    reqs: st.reqs.clone(),
                    seed: st.seed,
                    dist: st.dist,
                    links: 0..0,
                });
                raw_links.push(links);
            }
            report.reused += 1;
            report.reused_states += s1 - s0;
        }
        drop(copy_span);
    }
    let first_fresh = arena.states.len();
    let fresh: HashSet<ProducerId> = copy_from
        .iter()
        .zip(&arena.producers)
        .filter(|(from, _)| from.is_none())
        .map(|(_, p)| (Arc::clone(&p.owner), p.operation_id.clone()))
        .collect();

    for (p, from) in copy_from.iter().enumerate() {
        if from.is_some() {
            continue;
        }
        let prod = &arena.producers[p];
        let i0 = fact_pd_index(&prod.fact).expect("a producer is parameter-dependent");
        let reqs = frames
            .get(&*prod.owner)
            .and_then(|fg| site_reqs(fg, &prod.fact))
            .cloned()
            .unwrap_or_default();
        let key = (
            p,
            intern(&prod.owner),
            TempState::ParameterDependent(i0),
            reqs,
        );
        push(&mut arena, &mut ix_of, &mut raw_links, key, true, 0);
    }

    // Only the fresh states are expanded: a copied state's links are copied.
    let mut s: StateIx = first_fresh;
    while s < arena.states.len() {
        let st = &arena.states[s];
        let (p, c, t, reqs, dist) = (
            st.producer,
            Arc::clone(&st.routine),
            st.t.clone(),
            st.reqs.clone(),
            st.dist,
        );
        let callee = routines_by_id.get(&*c).copied();
        for &(r, e) in incoming.get(&*c).map_or(&[][..], Vec::as_slice) {
            let frame = frames.get(r);
            // The caller's own guard on the site it takes `e` at: a call site,
            // or the record op that fires an `implicit-trigger` edge (CR3).
            let edge_reqs = match (frame, e.site()) {
                (Some(fg), Some(site)) => fg.by_site.get(site).map_or(&[][..], Vec::as_slice),
                _ => &[],
            };
            let caller = routines_by_id.get(r).copied();
            let Some((t2, reqs2)) = cross_edge(&t, &reqs, e, caller, callee, frame, edge_reqs)
            else {
                continue;
            };
            let key = (p, intern(r), t2, reqs2);
            let ix = match ix_of.get(&key) {
                Some(&ix) => ix,
                None => push(&mut arena, &mut ix_of, &mut raw_links, key, false, dist + 1),
            };
            let edge = *edge_ix.entry(std::ptr::from_ref(e)).or_insert_with(|| {
                edges.push((r, e));
                edges.len() - 1
            });
            raw_links[ix].push(ConeLink {
                callee_state: s,
                edge,
            });
        }
        s += 1;
    }
    drop(ix_of);
    drop(edge_ix);

    let finalize_span = crate::engine::perf_trace::span("cone", "pd.finalize");
    let (mut arena, mut resolved, reps) =
        finalize(arena.producers, arena.states, raw_links, edges, &names);
    drop(finalize_span);
    // Growth slack is up to half of each pushed `Vec`; the solve is read-only
    // from here on.
    arena.states.shrink_to_fit();
    arena.links.shrink_to_fit();
    arena.by_routine.values_mut().for_each(Vec::shrink_to_fit);
    resolved.values_mut().for_each(Vec::shrink_to_fit);
    census(&arena, &resolved, &names, &reps);
    (arena, resolved, report, fresh)
}

/// D1.4: charge one solve to the `ALSEM_CONES_CENSUS` counters.
fn census(
    arena: &ConeArena,
    resolved: &ResolvedByRoutine,
    names: &HashSet<Arc<str>>,
    reps: &HashMap<(usize, TempState), Arc<CapabilityFact>>,
) {
    use crate::engine::l5::detector_context::cones_census as cc;
    if crate::engine::census::enabled() {
        // #160 §4.1.5: the two halves of §4.1.1, one bucket line each per solve.
        // Bytes and allocations come from ONE walk each, so they describe the
        // same allocations (final panel F2-2).
        use crate::engine::census::{Kind, bucket};
        crate::census_hook::mark("census.arena");
        let a = arena_tally(arena);
        bucket("arena.link_graph", a.bytes, a.allocs, Kind::Exact);
        let w = working_tally(resolved, names, reps);
        bucket("arena.working", w.bytes, w.allocs, Kind::Exact);
    }
    if !cc::enabled() {
        return;
    }
    cc::add(&cc::PD_PRODUCERS, arena.producers.len() as u64);
    cc::add(&cc::PD_STATES, arena.states.len() as u64);
    cc::add(&cc::PD_LINKS, arena.links.len() as u64);
    cc::add(
        &cc::PD_ENTRIES,
        resolved.values().map(Vec::len).sum::<usize>() as u64,
    );
    cc::add(&cc::PD_EDGES, arena.edges.len() as u64);
    cc::add_gated(&cc::PD_SOLVE_BYTES_ARENA, arena_tally(arena).bytes as u64);
    cc::add_gated(
        &cc::PD_SOLVE_BYTES_WORKING,
        working_tally(resolved, names, reps).bytes as u64,
    );
}

/// One heap block of `bytes` (nothing when empty).
fn block(t: &mut Tally, bytes: usize) {
    if bytes > 0 {
        t.bytes += bytes;
        t.allocs += 1;
    }
}

/// `v`'s buffer at capacity.
fn vec_block<T>(t: &mut Tally, v: &Vec<T>) {
    block(t, v.capacity() * std::mem::size_of::<T>());
}

/// `m`'s table at capacity: `capacity × (key + value + 1 control byte)`.
fn map_block<K, V>(t: &mut Tally, m: &HashMap<K, V>) {
    block(
        t,
        m.capacity() * (std::mem::size_of::<K>() + std::mem::size_of::<V>() + 1),
    );
}

/// An `Arc` allocation's strong + weak counts.
const ARC_HEADER: usize = 2 * std::mem::size_of::<usize>();

/// One `Arc<CapabilityFact>` allocation plus every allocation the fact owns.
fn arc_fact(t: &mut Tally, f: &CapabilityFact) {
    block(t, ARC_HEADER + std::mem::size_of::<CapabilityFact>());
    let h = super::cone_census::capability_fact_heap(f);
    t.bytes += h.bytes;
    t.allocs += h.allocs;
}

/// The arena half of one solve (spec §4.1.1 G-1): producers, states, links,
/// edges and `by_routine`, the part step 6 may retain. Bytes and allocations.
///
/// Counting rules (shared with [`working_tally`]): every `Vec` and hash table
/// at its CAPACITY (hash tables as `capacity × (key + value + 1 control
/// byte)`), every owned string at its capacity, plus `reqs` and fact heap (a
/// fact's strings by length, `cone_census`'s rule). A shared allocation counts
/// ONCE: each interned routine name, each rep, each edge in `ConeArena::edges`.
/// A `ResolvedEntry`'s `first_hop` borrows the graph, which the solve does not
/// own. Allocator per-allocation overhead is not modeled.
pub(crate) fn arena_tally(arena: &ConeArena) -> Tally {
    let mut t = Tally::default();
    vec_block(&mut t, &arena.producers);
    for p in &arena.producers {
        block(&mut t, ARC_HEADER + p.owner.len());
        block(&mut t, p.operation_id.capacity());
        arc_fact(&mut t, &p.fact);
    }
    vec_block(&mut t, &arena.states);
    for s in &arena.states {
        vec_block(&mut t, &s.reqs);
    }
    vec_block(&mut t, &arena.links);
    vec_block(&mut t, &arena.edges);
    for e in &arena.edges {
        for s in [Some(&e.to), Some(&e.kind), e.callsite.as_ref()]
            .into_iter()
            .chain([
                e.event_id.as_ref(),
                e.operation_id.as_ref(),
                Some(&e.sort_key),
            ])
            .flatten()
        {
            block(&mut t, s.capacity());
        }
    }
    map_block(&mut t, &arena.by_routine);
    for v in arena.by_routine.values() {
        vec_block(&mut t, v);
    }
    t
}

/// The working-set half of one solve (spec §4.1.1 G-1): the resolved own
/// entries, the interned routine names and the reps, the part step 6 retains
/// or re-solves. Same counting rules as [`arena_tally`].
pub(crate) fn working_tally(
    resolved: &ResolvedByRoutine,
    names: &HashSet<Arc<str>>,
    reps: &HashMap<(usize, TempState), Arc<CapabilityFact>>,
) -> Tally {
    let mut t = Tally::default();
    for n in names {
        block(&mut t, ARC_HEADER + n.len());
    }
    block(
        &mut t,
        names.capacity() * (std::mem::size_of::<Arc<str>>() + 1),
    );
    map_block(&mut t, resolved);
    for v in resolved.values() {
        vec_block(&mut t, v);
    }
    map_block(&mut t, reps);
    for r in reps.values() {
        arc_fact(&mut t, r);
    }
    t
}

/// Test hook (stage 1b, T-arena-canon on corpus apps): `compose_inherited_cones`
/// permutes its producers by [`test_shuffle::SEED`] before the solve. The
/// producers are the solve's only input whose order reaches it: the graph's
/// nodes are sorted and the incoming edges re-sorted before the walk, so the
/// model's routine and call order arrive canonical, and the edge numbering
/// follows the producer order. Process-global, because the cone can run on
/// rayon workers; a canonical solve makes it invisible to any other test.
#[cfg(test)]
pub(crate) mod test_shuffle {
    use std::sync::atomic::{AtomicU64, Ordering};

    /// 0: no permutation.
    pub(crate) static SEED: AtomicU64 = AtomicU64::new(0);
    /// Serializes the tests that set [`SEED`].
    pub(crate) static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// A deterministic permutation of `v` (Fisher–Yates over xorshift64).
    pub(crate) fn shuffled<T>(mut v: Vec<T>, seed: u64) -> Vec<T> {
        let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
        for i in (1..v.len()).rev() {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            v.swap(i, (x % (i as u64 + 1)) as usize);
        }
        v
    }

    pub(crate) fn apply<T>(v: Vec<T>) -> Vec<T> {
        match SEED.load(Ordering::SeqCst) {
            0 => v,
            seed => shuffled(v, seed),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeSet, HashMap};
    use std::sync::Arc;

    use super::*;
    use crate::engine::l4::capability_cone::{
        CapabilityExtra, CapabilityFact, TypedEdgeGraph, TypedOutEdge,
    };
    use crate::engine::l4::db_effect_solver::tests::{
        pd_arg_binding, pd_call_site, pd_record_op, pd_routine, pd_ts_known, pd_ts_pd,
    };
    use crate::engine::l4::param_guard::FrameGuards;
    use crate::engine::l4::summary::TempState;
    use crate::program::body::features::RECEIVER_PARAM_INDEX;
    use crate::program::model::workspace::{ModelParameter, ModelRoutine};

    fn edge(kind: &str, callsite: Option<&str>, to: &str) -> TypedOutEdge {
        TypedOutEdge::new(
            to.to_string(),
            kind.to_string(),
            callsite.map(str::to_string),
            None,
            None,
        )
    }

    fn param(index: u32, name: &str) -> ModelParameter {
        ModelParameter {
            index,
            name: name.to_string(),
            type_text: "Record Item".to_string(),
            is_var: true,
            is_record: true,
            table_name: Some("Item".to_string()),
        }
    }

    /// `caller` with one call site `cs` passing `arg` (its source temp state)
    /// for callee param 0.
    fn caller_passing(
        id: &str,
        cs: &str,
        arg: crate::program::body::features::PTempState,
    ) -> ModelRoutine {
        let mut r = pd_routine(id);
        r.call_sites.push(pd_call_site(
            cs,
            "Callee",
            vec![pd_arg_binding(0, Some(arg))],
        ));
        r
    }

    /// A raw direct insert on `owner`, parameter-dependent on param 0.
    fn producer(owner: &str) -> Producer {
        let fact = CapabilityFact {
            subject: owner.to_string(),
            op: "insert",
            resource_kind: "table",
            resource_id: Some("app/Table/1".to_string()),
            resource_arg_source: None,
            confidence: "static",
            provenance: "direct",
            via: "self",
            witness_operation_id: Some(format!("{owner}/op1")),
            witness_callsite_id: None,
            witness_first_hop_operation_id: None,
            witness_first_hop_event_id: None,
            extra: Some(CapabilityExtra::Table {
                record_variable_id: None,
                temp_state: Some(pd_ts_pd(0)),
                op_subtype: None,
            }),
        };
        Producer {
            operation_id: format!("{owner}/op1"),
            fact: Arc::new(fact),
            owner: Arc::from(owner),
        }
    }

    fn graph(edges: Vec<(&str, TypedOutEdge)>) -> TypedEdgeGraph {
        let mut outgoing: HashMap<String, Vec<TypedOutEdge>> = HashMap::new();
        let mut nodes: BTreeSet<String> = BTreeSet::new();
        for (from, e) in edges {
            nodes.insert(from.to_string());
            nodes.insert(e.to.clone());
            outgoing.entry(from.to_string()).or_default().push(e);
        }
        for list in outgoing.values_mut() {
            list.sort_by(|a, b| a.sort_key.cmp(&b.sort_key));
        }
        TypedEdgeGraph {
            nodes: nodes.into_iter().collect(),
            outgoing,
            unresolved_sources: BTreeSet::new(),
        }
    }

    /// Final panel F2-2: both buckets print `kind=exact`, so their allocation
    /// counts reach every allocation their bytes do: each producer's fact
    /// strings, each edge's strings, each rep's fact strings.
    #[test]
    fn tallies_count_every_owned_allocation() {
        let p = producer("A");
        let fact = Arc::clone(&p.fact);
        let arena = ConeArena {
            producers: vec![p],
            edges: vec![edge("direct-call", Some("cs1"), "B")],
            ..Default::default()
        };
        // producers buffer; owner Arc; operation_id; fact Arc; the fact's
        // subject, resource_id, witness_operation_id and temp-state kind;
        // edges buffer; the edge's to, kind, callsite and sort_key.
        assert_eq!(arena_tally(&arena).allocs, 13);
        let names: HashSet<Arc<str>> = HashSet::from([Arc::from("A")]);
        let reps = HashMap::from([((0, TempState::Unknown), fact)]);
        // one name; the names table; the reps table; the rep Arc and its
        // fact's four strings. `resolved` is empty and holds no table.
        assert_eq!(working_tally(&HashMap::new(), &names, &reps).allocs, 8);
    }

    #[test]
    fn cross_edge_pd_maps_only_pd() {
        let real = caller_passing("C", "cs1", pd_ts_known(false));
        let temp = caller_passing("C", "cs1", pd_ts_known(true));
        let e = edge("direct-call", Some("cs1"), "H");
        // A known state transfers unchanged, whatever the argument.
        assert_eq!(
            cross_edge(
                &TempState::Known(true),
                &[],
                &e,
                Some(&real),
                None,
                None,
                &[]
            ),
            Some((TempState::Known(true), vec![]))
        );
        // PD(0) takes the temp state of the argument the caller passes.
        assert_eq!(
            cross_edge(
                &TempState::ParameterDependent(0),
                &[],
                &e,
                Some(&temp),
                None,
                None,
                &[]
            ),
            Some((TempState::Known(true), vec![]))
        );
    }

    #[test]
    fn cross_edge_other_kind_is_unknown() {
        let temp = caller_passing("C", "cs1", pd_ts_known(true));
        let e = edge("object-run-resolved", Some("cs1"), "H");
        assert_eq!(
            cross_edge(
                &TempState::ParameterDependent(0),
                &[],
                &e,
                Some(&temp),
                None,
                None,
                &[]
            ),
            Some((TempState::Unknown, vec![]))
        );
    }

    /// #107 row 13 (Design 1): one parameter mapping, quotes ignored. A
    /// subscriber whose parameter name is `"Rec"` (quoted, as an ABI-derived
    /// name can be) re-anchors its guard on the publisher's `Rec`, exactly as
    /// `event_param_temp_state` already binds it. Hand-stated: publisher
    /// param 0 `Rec` is a usable guard parameter in the caller frame; the
    /// subscriber requires its param 0 to be true.
    #[test]
    fn carry_reqs_event_dispatch_ignores_quotes_in_parameter_names() {
        let mut publisher = pd_routine("Pub");
        publisher.parameters = vec![param(0, "Rec")];
        let mut subscriber = pd_routine("Sub");
        subscriber.parameters = vec![param(0, "\"Rec\"")];
        let e = edge("event-dispatch", None, "Sub");
        let frame = FrameGuards {
            usable: vec![0],
            by_site: HashMap::new(),
        };
        assert_eq!(
            carry_reqs(
                &[(0, true)],
                &e,
                Some(&publisher),
                Some(&subscriber),
                Some(&frame),
                &[]
            ),
            Some(vec![(0, true)]),
            "the quoted subscriber name must bind to the publisher's Rec"
        );
    }

    /// An event subscriber's parameters are the publisher's BY NAME, not by
    /// position.
    #[test]
    fn cross_edge_event_dispatch_maps_by_name() {
        let mut publisher = pd_routine("Pub");
        publisher.parameters = vec![param(0, "A"), param(1, "B")];
        for (i, ts) in [(0, pd_ts_known(false)), (1, pd_ts_pd(1))] {
            publisher.record_variables.push(
                crate::program::model::workspace::ModelRecordVariable {
                    id: format!("rv{i}"),
                    name: if i == 0 { "A" } else { "B" }.to_string(),
                    table_name: Some("Item".to_string()),
                    table_id: None,
                    is_parameter: true,
                    parameter_index: Some(i),
                    temp_state: ts,
                    scope: Some("parameter".to_string()),
                    is_receiver: false,
                    is_implicit_rec: false,
                },
            );
        }
        let mut subscriber = pd_routine("Sub");
        subscriber.parameters = vec![param(0, "B"), param(1, "A")];
        let e = edge("event-dispatch", None, "Sub");
        // Subscriber PD(0) is `B`, the publisher's param 1 (itself PD(1)).
        assert_eq!(
            cross_edge(
                &TempState::ParameterDependent(0),
                &[],
                &e,
                Some(&publisher),
                Some(&subscriber),
                None,
                &[]
            ),
            Some((TempState::ParameterDependent(1), vec![]))
        );
    }

    #[test]
    fn cross_edge_drops_on_contradiction() {
        let mut c = caller_passing("C", "cs1", pd_ts_known(false));
        c.call_sites[0].argument_texts = vec!["false".to_string()];
        let e = edge("direct-call", Some("cs1"), "H");
        assert_eq!(
            cross_edge(
                &TempState::Known(false),
                &[(0, true)],
                &e,
                Some(&c),
                None,
                None,
                &[]
            ),
            None
        );
    }

    /// An `implicit-trigger` edge from `C` to `T`, fired by `C`'s op `op`.
    fn trigger_edge(op: &str) -> TypedOutEdge {
        TypedOutEdge::new(
            "T".to_string(),
            "implicit-trigger".to_string(),
            None,
            None,
            Some(op.to_string()),
        )
    }

    /// `C` with one Validate op per `(op id, temp state)`.
    fn caller_with_ops(ops: &[(&str, crate::program::body::features::PTempState)]) -> ModelRoutine {
        let mut c = pd_routine("C");
        for (id, ts) in ops {
            c.record_operations
                .push(pd_record_op(id, "Validate", "app/Table/1", ts.clone()));
        }
        c
    }

    /// #96 D3.3: a trigger's `Rec` (PD(receiver)) binds to the FIRING op's own
    /// temp state in the caller's frame, per op; anything else is Unknown, and
    /// the caller's guards are dropped across the trigger edge.
    #[test]
    fn cross_edge_implicit_trigger_binds_the_firing_op() {
        let c = caller_with_ops(&[
            ("C/op1", pd_ts_known(true)),
            ("C/op2", pd_ts_known(false)),
            ("C/op3", pd_ts_pd(1)),
        ]);
        let rec = TempState::ParameterDependent(RECEIVER_PARAM_INDEX);
        let cross = |t: &TempState, reqs: &[Req], op: &str| {
            cross_edge(t, reqs, &trigger_edge(op), Some(&c), None, None, &[])
        };
        assert_eq!(
            cross(&rec, &[], "C/op1"),
            Some((TempState::Known(true), vec![]))
        );
        assert_eq!(
            cross(&rec, &[], "C/op2"),
            Some((TempState::Known(false), vec![]))
        );
        assert_eq!(
            cross(&rec, &[], "C/op3"),
            Some((TempState::ParameterDependent(1), vec![]))
        );
        assert_eq!(
            cross(&rec, &[], "C/op9"),
            Some((TempState::Unknown, vec![]))
        );
        assert_eq!(
            cross(&TempState::ParameterDependent(0), &[], "C/op1"),
            Some((TempState::Unknown, vec![]))
        );
        assert_eq!(
            cross(&TempState::Known(false), &[(0, true)], "C/op1"),
            Some((TempState::Known(false), vec![])),
            "a trigger edge drops the callee's guards"
        );
    }

    /// #96 D2: two ops of one caller that fire one trigger are two edges, so a
    /// state reached through both keeps a link per op (the link dedup is by
    /// sort key; without the op id in it the two would collapse).
    #[test]
    fn solve_keeps_a_link_per_trigger_op() {
        let c = caller_with_ops(&[("C/op1", pd_ts_known(false)), ("C/op2", pd_ts_known(false))]);
        let t = pd_routine("T");
        let g = graph(vec![
            ("C", trigger_edge("C/op2")),
            ("C", trigger_edge("C/op1")),
        ]);
        let by_id: HashMap<&str, &ModelRoutine> = [("C", &c), ("T", &t)].into();
        let mut p = producer("T");
        Arc::make_mut(&mut p.fact).extra = Some(CapabilityExtra::Table {
            record_variable_id: None,
            temp_state: Some(pd_ts_pd(RECEIVER_PARAM_INDEX)),
            op_subtype: None,
        });
        let (arena, resolved) = solve(vec![p], &g, &by_id, &HashMap::new());
        let at_c = &arena.by_routine["C"];
        assert_eq!(at_c.len(), 1, "both ops give one Known(false) state");
        let s = &arena.states[at_c[0]];
        assert_eq!(s.t, TempState::Known(false));
        let ops: Vec<&str> = arena.links[s.links.clone()]
            .iter()
            .map(|l| arena.edges[l.edge].operation_id.as_deref().unwrap())
            .collect();
        assert_eq!(ops, vec!["C/op1", "C/op2"]);
        assert_eq!(
            resolved["C"][0].first_hop.operation_id.as_deref(),
            Some("C/op1")
        );
    }

    /// A cycle that forwards the `var` parameter ends with ONE state per
    /// routine (the same tuple is never pushed twice). This pins
    /// the state-level dedup, not termination as such.
    #[test]
    fn solve_cycle_has_one_state_per_tuple() {
        let a = caller_passing("A", "a1", pd_ts_pd(0));
        let b = caller_passing("B", "b1", pd_ts_pd(0));
        let g = graph(vec![
            ("A", edge("direct-call", Some("a1"), "B")),
            ("B", edge("direct-call", Some("b1"), "A")),
        ]);
        let by_id: HashMap<&str, &ModelRoutine> = [("A", &a), ("B", &b)].into();
        let (arena, resolved) = solve(vec![producer("A")], &g, &by_id, &HashMap::new());
        for r in ["A", "B"] {
            let ts: Vec<&TempState> = arena.by_routine[r]
                .iter()
                .map(|&s| &arena.states[s].t)
                .collect();
            assert_eq!(ts, vec![&TempState::ParameterDependent(0)], "{r}");
        }
        assert!(arena.states[arena.by_routine["A"][0]].seed);
        assert_eq!(resolved["B"].len(), 1);
        assert!(!resolved.contains_key("A"), "the seed is not an entry");
    }

    /// One caller with two call sites into the producer's owner: ONE state,
    /// two links, in edge order.
    #[test]
    fn solve_keeps_all_links() {
        let mut c = caller_passing("C", "cs2", pd_ts_known(true));
        c.call_sites.push(pd_call_site(
            "cs1",
            "H",
            vec![pd_arg_binding(0, Some(pd_ts_known(true)))],
        ));
        let h = pd_routine("H");
        let g = graph(vec![
            ("C", edge("direct-call", Some("cs2"), "H")),
            ("C", edge("direct-call", Some("cs1"), "H")),
        ]);
        let by_id: HashMap<&str, &ModelRoutine> = [("C", &c), ("H", &h)].into();
        let frames: HashMap<String, FrameGuards> = HashMap::new();
        let (arena, resolved) = solve(vec![producer("H")], &g, &by_id, &frames);
        let at_c = &arena.by_routine["C"];
        assert_eq!(at_c.len(), 1);
        let s = &arena.states[at_c[0]];
        assert_eq!(s.t, TempState::Known(true));
        let sites: Vec<&str> = arena.links[s.links.clone()]
            .iter()
            .map(|l| arena.edges[l.edge].callsite.as_deref().unwrap())
            .collect();
        assert_eq!(sites, vec!["cs1", "cs2"]);
        let entry = &resolved["C"][0];
        assert_eq!(entry.dist, 1);
        assert_eq!(entry.first_hop.callsite.as_deref(), Some("cs1"));
    }

    /// D1.4 sharing: two producers reached across the SAME graph edge store
    /// that edge ONCE (`ConeArena::edges`), both links index it, and each
    /// entry's `first_hop` is the graph's own edge, not a copy.
    #[test]
    fn solve_stores_each_crossed_edge_once() {
        let c = caller_passing("C", "cs1", pd_ts_known(true));
        let h = pd_routine("H");
        let g = graph(vec![("C", edge("direct-call", Some("cs1"), "H"))]);
        let by_id: HashMap<&str, &ModelRoutine> = [("C", &c), ("H", &h)].into();
        let mut second = producer("H");
        second.operation_id = "H/op2".to_string();
        let (arena, resolved) = solve(vec![producer("H"), second], &g, &by_id, &HashMap::new());
        assert_eq!(arena.by_routine["C"].len(), 2, "one state per producer");
        assert_eq!(arena.edges.len(), 1, "the crossed edge is stored once");
        let links: Vec<usize> = arena.by_routine["C"]
            .iter()
            .flat_map(|&s| arena.links[arena.states[s].links.clone()].iter())
            .map(|l| l.edge)
            .collect();
        assert_eq!(links, vec![0, 0]);
        assert_eq!(arena.edges[0].sort_key, g.outgoing["C"][0].sort_key);
        assert_eq!(resolved["C"].len(), 2);
        for e in &resolved["C"] {
            assert!(std::ptr::eq(e.first_hop, &g.outgoing["C"][0]));
        }
    }

    /// `frames` entry for `id`: usable guard params and `site -> reqs`.
    fn frame(id: &str, usable: &[u32], sites: &[(&str, Vec<Req>)]) -> (String, FrameGuards) {
        (
            id.to_string(),
            FrameGuards {
                usable: usable.to_vec(),
                by_site: sites
                    .iter()
                    .map(|(s, r)| (s.to_string(), r.clone()))
                    .collect(),
            },
        )
    }

    /// States of `routine`, as `(t, reqs)`.
    fn states_at(arena: &ConeArena, routine: &str) -> Vec<(TempState, Vec<Req>)> {
        arena
            .by_routine
            .get(routine)
            .map_or(&[][..], Vec::as_slice)
            .iter()
            .map(|&s| (arena.states[s].t.clone(), arena.states[s].reqs.clone()))
            .collect()
    }

    /// Guard step: the seed carries the producer's own site requirements,
    /// read through the OWNER's frame.
    #[test]
    fn solve_seed_takes_site_reqs_from_the_owner_frame() {
        let c = caller_passing("C", "cs1", pd_ts_known(true));
        let h = pd_routine("H");
        let g = graph(vec![("C", edge("direct-call", Some("cs1"), "H"))]);
        let by_id: HashMap<&str, &ModelRoutine> = [("C", &c), ("H", &h)].into();
        let frames: HashMap<String, FrameGuards> =
            [frame("H", &[], &[("H/op1", vec![(1, true)])])].into();
        let (arena, _) = solve(vec![producer("H")], &g, &by_id, &frames);
        assert!(arena.states[0].seed);
        assert_eq!(arena.states[0].reqs, vec![(1, true)]);
    }

    /// Guard step: the caller's requirements for the call site (`edge_reqs`)
    /// are conjoined into the caller's state.
    #[test]
    fn solve_conjoins_the_call_site_guard_of_the_caller() {
        let c = caller_passing("C", "cs1", pd_ts_known(true));
        let h = pd_routine("H");
        let g = graph(vec![("C", edge("direct-call", Some("cs1"), "H"))]);
        let by_id: HashMap<&str, &ModelRoutine> = [("C", &c), ("H", &h)].into();
        let frames: HashMap<String, FrameGuards> =
            [frame("C", &[0], &[("cs1", vec![(0, true)])])].into();
        let (arena, _) = solve(vec![producer("H")], &g, &by_id, &frames);
        assert_eq!(
            states_at(&arena, "C"),
            vec![(TempState::Known(true), vec![(0, true)])]
        );
    }

    /// Guard step: a callee requirement is re-anchored on the caller's guard
    /// parameter, which needs the CALLER's frame (`usable`) in `cross_edge`.
    #[test]
    fn solve_passes_the_caller_frame_to_the_guard_step() {
        let mut c = caller_passing("C", "cs1", pd_ts_known(true));
        c.call_sites[0].argument_bindings[0].source_kind = "parameter".to_string();
        c.call_sites[0].argument_bindings[0].source_parameter_index = Some(2);
        let h = pd_routine("H");
        let g = graph(vec![("C", edge("direct-call", Some("cs1"), "H"))]);
        let by_id: HashMap<&str, &ModelRoutine> = [("C", &c), ("H", &h)].into();
        let frames: HashMap<String, FrameGuards> = [
            frame("H", &[], &[("H/op1", vec![(0, true)])]),
            frame("C", &[2], &[]),
        ]
        .into();
        let (arena, _) = solve(vec![producer("H")], &g, &by_id, &frames);
        let reqs: Vec<Vec<Req>> = states_at(&arena, "C").into_iter().map(|s| s.1).collect();
        assert_eq!(reqs, vec![vec![(2, true)]]);
    }

    /// Guard step: a transition an argument contradicts is dropped, not kept
    /// with the requirement forgotten.
    #[test]
    fn solve_drops_a_contradicted_transition() {
        let mut c = caller_passing("C", "cs1", pd_ts_known(true));
        c.call_sites[0].argument_texts = vec!["false".to_string()];
        let h = pd_routine("H");
        let g = graph(vec![("C", edge("direct-call", Some("cs1"), "H"))]);
        let by_id: HashMap<&str, &ModelRoutine> = [("C", &c), ("H", &h)].into();
        let frames: HashMap<String, FrameGuards> =
            [frame("H", &[], &[("H/op1", vec![(0, true)])])].into();
        let (arena, resolved) = solve(vec![producer("H")], &g, &by_id, &frames);
        assert!(states_at(&arena, "C").is_empty());
        assert!(!resolved.contains_key("C"));
    }

    /// No state stops (R1-F12): a known state keeps propagating, so a guard
    /// two hops up still applies to it.
    #[test]
    fn solve_known_state_still_picks_up_a_guard_higher_up() {
        let c = caller_passing("C", "cs1", pd_ts_known(true));
        let d = caller_passing("D", "d1", pd_ts_known(true));
        let h = pd_routine("H");
        let g = graph(vec![
            ("C", edge("direct-call", Some("cs1"), "H")),
            ("D", edge("direct-call", Some("d1"), "C")),
        ]);
        let by_id: HashMap<&str, &ModelRoutine> = [("C", &c), ("D", &d), ("H", &h)].into();
        let frames: HashMap<String, FrameGuards> =
            [frame("D", &[0], &[("d1", vec![(0, true)])])].into();
        let (arena, _) = solve(vec![producer("H")], &g, &by_id, &frames);
        assert_eq!(
            states_at(&arena, "D"),
            vec![(TempState::Known(true), vec![(0, true)])]
        );
    }

    /// #96 review CR3: the CALLER's guard on the record op that fires a trigger
    /// is conjoined into the caller's state, as for a call site. The op ids are
    /// not positional (`C/op1` is a `Commit`), so the op lookup is pinned too.
    #[test]
    fn solve_conjoins_the_caller_guard_on_the_firing_op() {
        let c = caller_with_ops(&[("C/op0", pd_ts_known(true)), ("C/op2", pd_ts_known(false))]);
        let t = pd_routine("T");
        let g = graph(vec![("C", trigger_edge("C/op2"))]);
        let by_id: HashMap<&str, &ModelRoutine> = [("C", &c), ("T", &t)].into();
        let mut p = producer("T");
        Arc::make_mut(&mut p.fact).extra = Some(CapabilityExtra::Table {
            record_variable_id: None,
            temp_state: Some(pd_ts_pd(RECEIVER_PARAM_INDEX)),
            op_subtype: None,
        });
        let frames: HashMap<String, FrameGuards> =
            [frame("C", &[0], &[("C/op2", vec![(0, false)])])].into();
        let (arena, _) = solve(vec![p], &g, &by_id, &frames);
        assert_eq!(
            states_at(&arena, "C"),
            vec![(TempState::Known(false), vec![(0, false)])]
        );
    }

    // ----- Stage 1b: the canonical arena (T-arena-canon) -----

    /// Every index of `arena` and `resolved` as text: two solves print the same
    /// only if their storage is the same, index for index, and their resolved
    /// entries (state, dist, first hop, rep) are the same in the same order.
    fn arena_bytes(arena: &ConeArena, resolved: &ResolvedByRoutine) -> String {
        let mut s = String::new();
        for p in &arena.producers {
            s += &format!("P {}|{} {:?}\n", p.owner, p.operation_id, p.fact);
        }
        for e in &arena.edges {
            s += &format!("E {e:?}\n");
        }
        for st in &arena.states {
            s += &format!("S {st:?}\n");
        }
        for l in &arena.links {
            s += &format!("L {l:?}\n");
        }
        let mut by: Vec<_> = arena.by_routine.iter().collect();
        by.sort();
        s += &format!("B {by:?}\n");
        let mut rs: Vec<_> = resolved.iter().collect();
        rs.sort_by(|a, b| a.0.cmp(b.0));
        for (r, es) in rs {
            for e in es {
                s += &format!(
                    "R {r} state={} dist={} hop={:?} rep={:?}\n",
                    e.state, e.dist, e.first_hop, e.rep
                );
            }
        }
        s
    }

    /// The canonical form's invariants, read off the arena alone: every rank
    /// strictly increasing, every reference pointing where its identity says,
    /// `by_routine` and the resolved entries exactly the states of each routine
    /// in rank order, each first hop the lowest-ranked shortest link.
    fn assert_canonical(arena: &ConeArena, resolved: &ResolvedByRoutine) {
        use std::cmp::Ordering::Less;
        for w in arena.producers.windows(2) {
            assert_eq!(producer_cmp(&w[0], &w[1]), Less, "producers out of rank");
        }
        // An edge's `from`: the routine of every state whose link crosses it.
        let mut from: Vec<Option<&str>> = vec![None; arena.edges.len()];
        for st in &arena.states {
            for l in &arena.links[st.links.clone()] {
                let f = from[l.edge].get_or_insert(&st.routine);
                assert_eq!(*f, &*st.routine, "an edge has one `from`");
                let callee = &arena.states[l.callee_state];
                assert_eq!(
                    arena.edges[l.edge].to, &*callee.routine,
                    "link edge → callee"
                );
                assert_eq!(callee.producer, st.producer, "a link stays in one producer");
            }
            let links = &arena.links[st.links.clone()];
            for w in links.windows(2) {
                assert!(
                    (w[0].edge, w[0].callee_state) < (w[1].edge, w[1].callee_state),
                    "links out of rank or duplicated"
                );
            }
        }
        for i in 1..arena.edges.len() {
            let (a, b) = (from[i - 1].unwrap(), from[i].unwrap());
            assert_eq!(
                edge_cmp((a, &arena.edges[i - 1]), (b, &arena.edges[i])),
                Less,
                "edges out of rank"
            );
        }
        for w in arena.states.windows(2) {
            let key = |s: &ConeState| {
                (
                    s.producer,
                    s.dist,
                    s.routine.clone(),
                    s.t.clone(),
                    s.reqs.clone(),
                )
            };
            assert!(key(&w[0]) < key(&w[1]), "states out of rank");
        }
        let mut by: HashMap<&str, Vec<StateIx>> = HashMap::new();
        for (ix, st) in arena.states.iter().enumerate() {
            by.entry(&st.routine).or_default().push(ix);
        }
        assert_eq!(by.len(), arena.by_routine.len());
        for (r, ixs) in &by {
            assert_eq!(&arena.by_routine[*r], ixs, "by_routine of {r}");
            let own: Vec<StateIx> = ixs
                .iter()
                .copied()
                .filter(|&s| !arena.states[s].seed)
                .collect();
            let got: Vec<StateIx> = resolved
                .get(*r)
                .map_or(&[][..], Vec::as_slice)
                .iter()
                .map(|e| e.state)
                .collect();
            assert_eq!(got, own, "resolved entries of {r}");
        }
        for es in resolved.values() {
            for e in es {
                let st = &arena.states[e.state];
                assert_eq!(e.dist, st.dist);
                let first = arena.links[st.links.clone()]
                    .iter()
                    .find(|l| arena.states[l.callee_state].dist + 1 == st.dist)
                    .unwrap();
                assert_eq!(
                    format!("{:?}", e.first_hop),
                    format!("{:?}", arena.edges[first.edge]),
                    "first hop"
                );
            }
        }
    }

    use super::test_shuffle::shuffled;

    /// T-arena-canon (a) on corpus apps (every fixture whose arena the stage-1b
    /// change reordered): with the cone's producers permuted
    /// ([`super::test_shuffle`]), the finalized arena's logical lines (storage
    /// order included) and the rendered digest (its witnesses and `via_paths`
    /// walk the arena) are the same bytes as unpermuted.
    #[test]
    fn corpus_arena_and_witnesses_are_independent_of_producer_order() {
        use super::test_shuffle::{LOCK, SEED};
        use std::sync::atomic::Ordering::SeqCst;
        let _guard = LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let corpus = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/r0-corpus");
        let mut producers_seen = 0usize;
        for ws in [
            "ws-d37",
            "ws-d40",
            "ws-d65",
            "ws-event-chains",
            "ws-event-multi-sub-overlap",
            "ws-explain",
            "ws-explain-calls",
            "ws-path-facts",
        ] {
            let model =
                crate::program::model::program_calls::assemble_and_resolve_workspace_with_program_calls(
                    &corpus.join(ws),
                )
                .expect("the corpus model builds");
            let render = || {
                let base = crate::engine::l4::capability_cone::build_r3a3_source_only_base(&model);
                let digest = crate::engine::l5::digest::compute_digest_effects(&model);
                (
                    base.pd_solution.producers.len(),
                    format!(
                        "{}\n{digest:?}",
                        base.pd_solution.logical_lines().join("\n")
                    ),
                )
            };
            SEED.store(0, SeqCst);
            let (n, want) = render();
            producers_seen += usize::from(n > 1);
            for seed in 1..=6 {
                SEED.store(seed, SeqCst);
                let got = render().1;
                SEED.store(0, SeqCst);
                assert!(
                    got == want,
                    "{ws}: seed {seed} changed the arena or the digest"
                );
            }
        }
        assert!(
            producers_seen >= 4,
            "the fixtures must carry several producers"
        );
    }

    /// A graph whose out-edge lists keep the given order (the solve must not
    /// rely on them being sorted).
    fn graph_as_given(edges: Vec<(&str, TypedOutEdge)>) -> TypedEdgeGraph {
        let mut outgoing: HashMap<String, Vec<TypedOutEdge>> = HashMap::new();
        let mut nodes: BTreeSet<String> = BTreeSet::new();
        for (from, e) in edges {
            nodes.insert(from.to_string());
            nodes.insert(e.to.clone());
            outgoing.entry(from.to_string()).or_default().push(e);
        }
        TypedEdgeGraph {
            nodes: nodes.into_iter().collect(),
            outgoing,
            unresolved_sources: BTreeSet::new(),
        }
    }

    /// `id` with one call site per `(site, callee param 0's argument)`.
    fn calling(
        id: &str,
        sites: &[(&str, crate::program::body::features::PTempState)],
    ) -> ModelRoutine {
        let mut r = pd_routine(id);
        for (cs, arg) in sites {
            r.call_sites.push(pd_call_site(
                cs,
                "Callee",
                vec![pd_arg_binding(0, Some(arg.clone()))],
            ));
        }
        r
    }

    /// T-arena-canon (a), hand-stated: four producers (three at one owner,
    /// whose ops `op1`, `op2`, `op10` order differently as strings), a cycle,
    /// a diamond, two states of one routine with different `t`, two links into
    /// one state across different edges, and one edge present twice in the
    /// graph. Whatever the order of the producers and of the graph's edges,
    /// the finalized arena and every resolved entry are the same bytes.
    #[test]
    fn solve_is_independent_of_input_order() {
        let routines = [
            calling(
                "C",
                &[
                    ("cs1", pd_ts_known(true)),
                    ("cs2", pd_ts_pd(0)),
                    ("cs3", pd_ts_pd(0)),
                ],
            ),
            calling("D", &[("d1", pd_ts_known(false)), ("d2", pd_ts_pd(0))]),
            calling("E", &[("e1", pd_ts_pd(0)), ("e2", pd_ts_known(true))]),
            calling("A", &[("a1", pd_ts_pd(0)), ("a2", pd_ts_pd(0))]),
            calling("B", &[("b1", pd_ts_pd(0))]),
            pd_routine("G"),
            pd_routine("H"),
        ];
        let by_id: HashMap<&str, &ModelRoutine> =
            routines.iter().map(|r| (r.id.as_str(), r)).collect();
        let edges = vec![
            ("C", edge("direct-call", Some("cs1"), "H")),
            ("C", edge("direct-call", Some("cs2"), "H")),
            // One graph edge twice: one arena edge, one link.
            ("C", edge("direct-call", Some("cs2"), "H")),
            ("C", edge("direct-call", Some("cs3"), "G")),
            ("D", edge("direct-call", Some("d1"), "C")),
            ("D", edge("direct-call", Some("d2"), "G")),
            ("E", edge("direct-call", Some("e1"), "D")),
            ("E", edge("direct-call", Some("e2"), "C")),
            ("A", edge("direct-call", Some("a1"), "B")),
            ("A", edge("direct-call", Some("a2"), "H")),
            ("B", edge("direct-call", Some("b1"), "A")),
        ];
        let producers = || {
            let mut out = vec![producer("H"), producer("G")];
            for op in ["H/op10", "H/op2"] {
                let mut p = producer("H");
                p.operation_id = op.to_string();
                Arc::make_mut(&mut p.fact).witness_operation_id = Some(op.to_string());
                out.push(p);
            }
            out
        };
        let frames: HashMap<String, FrameGuards> = HashMap::new();
        let g = graph_as_given(edges.clone());
        let (arena, resolved) = solve(producers(), &g, &by_id, &frames);
        assert_canonical(&arena, &resolved);
        let want = arena_bytes(&arena, &resolved);
        // The fixture exercises what it claims.
        let ops: Vec<&str> = arena
            .producers
            .iter()
            .map(|p| p.operation_id.as_str())
            .collect();
        assert_eq!(
            ops,
            ["G/op1", "H/op1", "H/op2", "H/op10"],
            "op ids rank by ordinal"
        );
        assert_eq!(arena.edges.len(), 10, "the doubled edge is one arena edge");
        assert!(states_at(&arena, "C").len() >= 2 && states_at(&arena, "D").len() >= 2);
        for seed in 1..=24 {
            let g = graph_as_given(shuffled(edges.clone(), seed));
            let (a, r) = solve(shuffled(producers(), seed + 100), &g, &by_id, &frames);
            assert_canonical(&a, &r);
            assert_eq!(arena_bytes(&a, &r), want, "seed {seed}");
        }
    }

    /// T-arena-canon (b), design r6 §6.10: two distinct INTERNAL routine ids
    /// sharing one STABLE id (the internal key carries `routine_kind`, the
    /// stable id does not), both reached from one producer with equal `dist`,
    /// `t` and `reqs`. Both states survive and rank by internal id, whatever
    /// the input order. The callsite ids are chosen so the solve DISCOVERS `X2`
    /// first (`a` sorts before `b` in the incoming order): only the internal-id
    /// rank puts `X1` first.
    #[test]
    fn solve_keeps_two_routines_that_share_a_stable_id() {
        let mut x1 = calling("app/X1", &[("b", pd_ts_known(true))]);
        let mut x2 = calling("app/X2", &[("a", pd_ts_known(true))]);
        x1.kind = "procedure".to_string();
        x2.kind = "trigger".to_string();
        x1.stable_routine_id = "app:Codeunit:1#shared".to_string();
        x2.stable_routine_id = x1.stable_routine_id.clone();
        let h = pd_routine("H");
        let by_id: HashMap<&str, &ModelRoutine> =
            [("app/X1", &x1), ("app/X2", &x2), ("H", &h)].into();
        let edges = vec![
            ("app/X1", edge("direct-call", Some("b"), "H")),
            ("app/X2", edge("direct-call", Some("a"), "H")),
        ];
        let frames: HashMap<String, FrameGuards> = HashMap::new();
        let mut want: Option<String> = None;
        for seed in 0..=8 {
            let g = graph_as_given(shuffled(edges.clone(), seed));
            let (arena, resolved) = solve(vec![producer("H")], &g, &by_id, &frames);
            assert_canonical(&arena, &resolved);
            let routines: Vec<&str> = arena.states.iter().map(|s| &*s.routine).collect();
            assert_eq!(routines, ["H", "app/X1", "app/X2"], "seed {seed}");
            let (s1, s2) = (&arena.states[1], &arena.states[2]);
            assert_eq!((s1.dist, &s1.t, &s1.reqs), (s2.dist, &s2.t, &s2.reqs));
            assert_eq!(resolved["app/X1"].len(), 1);
            assert_eq!(resolved["app/X2"].len(), 1);
            let bytes = arena_bytes(&arena, &resolved);
            assert_eq!(
                want.get_or_insert_with(|| bytes.clone()),
                &bytes,
                "seed {seed}"
            );
        }
    }

    #[test]
    fn op_id_cmp_orders_by_ordinal_and_stays_total() {
        use std::cmp::Ordering::*;
        assert_eq!(op_id_cmp("r/op8", "r/op10"), Less);
        assert_eq!(op_id_cmp("r/op10", "r/op10"), Equal);
        assert_eq!(op_id_cmp("q/op10", "r/op1"), Less, "routine first");
        assert_eq!(op_id_cmp("r/op01", "r/op1"), Less, "one ordinal, two ids");
        assert_eq!(
            op_id_cmp("r/opx", "r/op1"),
            Greater,
            "other shapes as strings"
        );
    }

    /// One revision's solve inputs for the reuse tests: rows, edges,
    /// producers.
    struct Rev {
        routines: Vec<ModelRoutine>,
        edges: Vec<(&'static str, TypedOutEdge)>,
        producers: Vec<Producer>,
    }

    /// A solve kept as the next one's seed: its arena and digests.
    type Kept = (ConeArena, HashMap<String, [u8; 32]>);

    impl Rev {
        /// The full solve's bytes, the reused solve's (seeded by `prev`), the
        /// reuse report and the reused solve kept.
        fn solve_both(&self, prev: Option<&Kept>) -> (String, String, PdSolveReport, Kept) {
            let by_id: HashMap<&str, &ModelRoutine> =
                self.routines.iter().map(|r| (r.id.as_str(), r)).collect();
            let frames: HashMap<String, FrameGuards> = HashMap::new();
            let g = graph_as_given(self.edges.clone());
            let digests =
                crate::engine::l4::capability_cone::pd_input_digests(&g, &self.routines, &frames);
            let (full, full_r) = solve(self.producers.clone(), &g, &by_id, &frames);
            let want = arena_bytes(&full, &full_r);
            let reuse = prev.map(|(old, old_digests)| PdReuse {
                old,
                old_digests,
                new_digests: &digests,
            });
            let (a, r, report) =
                solve_seeded(self.producers.clone(), &g, &by_id, &frames, reuse.as_ref());
            assert_canonical(&a, &r);
            let got = arena_bytes(&a, &r);
            drop(r);
            (want, got, report, (a, digests))
        }
    }

    /// Design §6.10 "Producers" (T-pd-out, T-arena-canon "incremental after
    /// insertion/deletion equals cold"): a chain A → C → P where P inserts
    /// into its parameter (a producer) and a second, unrelated producer Q
    /// called by B. Each edit below is solved with the previous revision's
    /// arena as the seed; the reused arena equals a full solve's, byte for
    /// byte, and exactly the producers the rule names are re-solved:
    /// - C's argument changes from a physical to a temporary record (C is Δ
    ///   and reaches P): P re-solved, Q copied, and A's state changes too
    ///   (the producer solve carries it, whatever the cone does);
    /// - a new caller X → C (X has no old state: only the NEW-graph
    ///   reachability catches it);
    /// - C stops calling P (C had a state of P: only the OLD arena catches
    ///   it; C no longer reaches P in the new graph);
    /// - no edit: everything copied.
    ///
    /// DISCRIMINATION (2026-11-02, break with Edit, run, revert, run):
    /// starting the reachability with an empty set instead of Δ fails "new
    /// caller: equals full" (X's state is missing); skipping the old-arena
    /// hits (`old_hit` left empty) fails the removed-edge step at the copy's
    /// debug assertion "a reused producer's edge is missing from the graph"
    /// (C's stale link crosses an edge the graph no longer has; a release
    /// build would fall back to solving that producer afresh, so the rule and
    /// the fallback both guard this shape).
    #[test]
    fn reused_producer_states_equal_a_full_solve() {
        fn pass(id: &str, cs: &str, temp: bool) -> ModelRoutine {
            // The argument: the caller's own parameter 0 (physical through
            // the chain), or a temporary record.
            let arg = if temp { pd_ts_known(true) } else { pd_ts_pd(0) };
            let mut r = pd_routine(id);
            r.call_sites
                .push(pd_call_site(cs, "P", vec![pd_arg_binding(0, Some(arg))]));
            r
        }
        let base = || Rev {
            routines: vec![
                pass("A", "a1", false),
                pass("B", "b1", false),
                pass("C", "c1", false),
                pd_routine("P"),
                pd_routine("Q"),
            ],
            edges: vec![
                ("A", edge("direct-call", Some("a1"), "C")),
                ("C", edge("direct-call", Some("c1"), "P")),
                ("B", edge("direct-call", Some("b1"), "Q")),
            ],
            producers: vec![producer("P"), producer("Q")],
        };
        let r0 = base();
        let (want, got, report, prev) = r0.solve_both(None);
        assert_eq!(got, want, "cold: equals full");
        assert_eq!(report.reused, 0);

        // No edit: everything copied.
        let (want, got, report, _) = base().solve_both(Some(&prev));
        assert_eq!(got, want, "no edit: equals full");
        assert_eq!((report.reused, report.delta_routines), (2, 0));

        // C passes a temporary record: P re-solved, Q copied.
        let mut r1 = base();
        r1.routines[2] = pass("C", "c1", true);
        let (want, got, report, prev1) = r1.solve_both(Some(&prev));
        assert_eq!(got, want, "temp argument: equals full");
        assert_eq!(report.reused, 1, "temp argument: Q copied, P re-solved");
        assert_ne!(
            want,
            base().solve_both(None).0,
            "precondition: the edit moves the arena"
        );
        let _ = prev1;

        // A new caller X of C: only the new graph sees it.
        let mut r2 = base();
        r2.routines.push(pass("X", "x1", false));
        r2.edges.push(("X", edge("direct-call", Some("x1"), "C")));
        let (want, got, report, _) = r2.solve_both(Some(&prev));
        assert_eq!(got, want, "new caller: equals full");
        assert_eq!(report.reused, 1, "new caller: Q copied");

        // C stops calling P: only the old arena sees it.
        let mut r3 = base();
        r3.routines[2] = pd_routine("C");
        r3.edges.retain(|(f, _)| *f != "C");
        let (want, got, report, _) = r3.solve_both(Some(&prev));
        assert_eq!(got, want, "removed edge: equals full");
        assert_eq!(report.reused, 1, "removed edge: Q copied");
    }
}

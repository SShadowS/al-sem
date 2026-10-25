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
    /// in edge order (`TypedOutEdge::sort_key`, then callee state).
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

/// The whole solve, in cone ids.
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

/// The transitive product-state solve over the typed graph's INCOMING edges.
/// Deterministic: producers are taken in the given order, incoming edges in
/// `(sort_key, from)` order, and states are numbered in BFS order.
pub(crate) fn solve<'g>(
    producers: Vec<Producer>,
    g: &'g TypedEdgeGraph,
    routines_by_id: &HashMap<&str, &ModelRoutine>,
    frames: &HashMap<String, FrameGuards>,
) -> (ConeArena, ResolvedByRoutine<'g>) {
    crate::engine::perf_trace::counter_delta("build.pd_solve", 1);
    let mut arena = ConeArena {
        producers,
        ..Default::default()
    };
    // A workspace without producers pays nothing.
    if arena.producers.is_empty() {
        return (arena, HashMap::new());
    }
    let mut incoming: HashMap<&str, Vec<(&str, &TypedOutEdge)>> = HashMap::new();
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
    // `ConeArena::edges` while solving: each distinct graph edge, by address.
    let mut edges: Vec<&'g TypedOutEdge> = Vec::new();
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
    // States are appended in BFS order, so `arena.states` is its own queue.
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

    for p in 0..arena.producers.len() {
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

    let mut s: StateIx = 0;
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
                edges.push(e);
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

    let key_of = |l: &ConeLink| edges[l.edge].sort_key.as_str();
    for (ix, mut links) in raw_links.into_iter().enumerate() {
        links.sort_by(|a, b| {
            key_of(a)
                .cmp(key_of(b))
                .then(a.callee_state.cmp(&b.callee_state))
        });
        links.dedup_by(|a, b| a.callee_state == b.callee_state && key_of(a) == key_of(b));
        let start = arena.links.len();
        arena.links.extend(links);
        arena.states[ix].links = start..arena.links.len();
        arena
            .by_routine
            .entry(Arc::clone(&arena.states[ix].routine))
            .or_default()
            .push(ix);
    }

    let mut reps: HashMap<(usize, TempState), Arc<CapabilityFact>> = HashMap::new();
    let mut resolved: ResolvedByRoutine = HashMap::new();
    for (ix, s) in arena.states.iter().enumerate() {
        if s.seed {
            continue;
        }
        let first_hop = edges[arena.links[s.links.clone()]
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
    arena.edges = edges.into_iter().cloned().collect();
    // Growth slack is up to half of each pushed `Vec`; the solve is read-only
    // from here on.
    arena.states.shrink_to_fit();
    arena.links.shrink_to_fit();
    arena.by_routine.values_mut().for_each(Vec::shrink_to_fit);
    resolved.values_mut().for_each(Vec::shrink_to_fit);
    census(&arena, &resolved, &names, &reps);
    (arena, resolved)
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
}

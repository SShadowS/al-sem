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
use super::summary_runner::{event_param_temp_state, pd_temp_state_at_callsite};
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
/// (`edge` is an out-edge of this state's routine, `edge.to` is
/// `callee_state`'s routine).
#[derive(Debug, Clone)]
pub struct ConeLink {
    pub callee_state: StateIx,
    pub edge: TypedOutEdge,
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
    /// Internal routine id → its states (seed included), in state order.
    pub by_routine: HashMap<Arc<str>, Vec<StateIx>>,
}

/// D1.3: one non-seed state placed at its routine for uses (a) and (b).
#[derive(Debug, Clone)]
pub struct ResolvedEntry {
    pub state: StateIx,
    /// The producer's raw fact with `temp_state = t` and
    /// `witness_operation_id` = the producer's op. One `Arc` per
    /// `(producer, t)`, shared by every state with that pair.
    pub rep: Arc<CapabilityFact>,
    pub dist: usize,
    /// The edge of this state's shortest link (an out-edge of its routine).
    pub first_hop: TypedOutEdge,
    pub reqs: Vec<Req>,
}

pub type ResolvedByRoutine = HashMap<Arc<str>, Vec<ResolvedEntry>>;

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

/// The call site of an argument-binding edge. Only `direct-call` /
/// `variable-typed-call` edges carry bindings keyed by the callee's parameter
/// index (the typed twins of the L4 solver's `direct` / `method`).
///
/// This allowlist is a CONSERVATIVE SUBSET of `substitute_pd_temp_state`'s
/// (`summary_runner.rs`: `direct | method | implicit-trigger`): the typed graph
/// carries no `implicit-trigger` edges (`combined_graph.rs` skips them). Keep the
/// two in step.
fn bound_site<'e, 'r>(
    edge: &'e TypedOutEdge,
    caller: Option<&'r ModelRoutine>,
) -> Option<(&'e str, &'r ModelRoutine)> {
    let binds = matches!(edge.kind.as_str(), "direct-call" | "variable-typed-call");
    match (edge.callsite.as_deref(), caller) {
        (Some(cs), Some(caller)) if binds => Some((cs, caller)),
        _ => None,
    }
}

/// The guard step shared by [`cross_edge`] and `capability_cone::substitute_entry`:
/// carry callee-frame `reqs` across `edge` into the caller's frame
/// ([`across_edge`] on call edges, the by-name re-anchor on event edges; any
/// other edge kind drops them), then `conjoin` with `edge_reqs`, the
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
            let mapped = subscriber
                .parameters
                .iter()
                .find(|p| p.index == j)
                .and_then(|sp| {
                    publisher
                        .parameters
                        .iter()
                        .find(|pp| pp.name.eq_ignore_ascii_case(&sp.name))
                })
                .map(|pp| pp.index)
                .filter(|i| caller_frame.is_some_and(|f| f.usable.contains(i)));
            if let Some(i) = mapped {
                out = conjoin(&out, &[(i, v)])?;
            }
        }
    } else if let Some((cs, caller)) = bound_site(edge, caller)
        && let Some(cs) = caller.call_sites.iter().find(|c| c.id == cs)
    {
        out = across_edge(reqs, cs, caller_frame)?;
    }
    conjoin(&out, edge_reqs)
}

/// D1.2 transition over typed edge `caller → callee`. The temp map applies to
/// PD only: `pd_temp_state_at_callsite` on `direct-call` /
/// `variable-typed-call`, `event_param_temp_state` on `event-dispatch`,
/// `Unknown` on any other kind. A known or unknown `t` transfers unchanged;
/// `reqs` go through [`carry_reqs`]. `None`: the transition is contradicted.
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
                (None, Some((cs, caller))) => pd_temp_state_at_callsite(caller, cs, *i),
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
pub(crate) fn solve(
    producers: Vec<Producer>,
    g: &TypedEdgeGraph,
    routines_by_id: &HashMap<&str, &ModelRoutine>,
    frames: &HashMap<String, FrameGuards>,
) -> (ConeArena, ResolvedByRoutine) {
    let mut arena = ConeArena {
        producers,
        ..Default::default()
    };
    // Review Focus 1: a workspace without producers pays nothing.
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
            let edge_reqs = match (frame, e.callsite.as_deref()) {
                (Some(fg), Some(cs)) => fg.by_site.get(cs).map_or(&[][..], Vec::as_slice),
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
            raw_links[ix].push(ConeLink {
                callee_state: s,
                edge: e.clone(),
            });
        }
        s += 1;
    }

    for (ix, mut links) in raw_links.into_iter().enumerate() {
        links.sort_by(|a, b| {
            a.edge
                .sort_key
                .cmp(&b.edge.sort_key)
                .then(a.callee_state.cmp(&b.callee_state))
        });
        links.dedup_by(|a, b| {
            a.callee_state == b.callee_state && a.edge.sort_key == b.edge.sort_key
        });
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
        let first_hop = arena.links[s.links.clone()]
            .iter()
            .find(|l| arena.states[l.callee_state].dist + 1 == s.dist)
            .expect("a non-seed state has a shortest link")
            .edge
            .clone();
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
                reqs: s.reqs.clone(),
            });
    }
    (arena, resolved)
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
        pd_arg_binding, pd_call_site, pd_routine, pd_ts_known, pd_ts_pd,
    };
    use crate::engine::l4::param_guard::FrameGuards;
    use crate::engine::l4::summary::TempState;
    use crate::program::model::workspace::{ModelParameter, ModelRoutine};

    fn edge(kind: &str, callsite: Option<&str>, to: &str) -> TypedOutEdge {
        TypedOutEdge::new(
            to.to_string(),
            kind.to_string(),
            callsite.map(str::to_string),
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

    /// Review Focus 2: an event subscriber's parameters are the publisher's BY
    /// NAME, not by position.
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

    /// Review Focus 3: a cycle that forwards the `var` parameter terminates;
    /// the visited set is the state tuple, never the routine.
    #[test]
    fn solve_terminates_on_cycle() {
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
            .map(|l| l.edge.callsite.as_deref().unwrap())
            .collect();
        assert_eq!(sites, vec!["cs1", "cs2"]);
        let entry = &resolved["C"][0];
        assert_eq!(entry.dist, 1);
        assert_eq!(entry.first_hop.callsite.as_deref(), Some("cs1"));
    }
}

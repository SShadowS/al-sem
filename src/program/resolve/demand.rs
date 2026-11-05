//! Which dependency routines a cross-app analysis needs (engine-switch S8.2; spec
//! S8, "demand policy").
//!
//! A workspace's findings can depend only on routines its own code can reach, and
//! on the code around the dependency events it subscribes to. Everything else in a
//! dependency (most of the Base Application) can neither produce a kept finding nor
//! change one, so the cross-app model leaves it out.
//!
//! - **Forward**: from every primary-app routine, every routine an edge reaches —
//!   calls, runs, implicit triggers, and the subscribers of an event a reached
//!   publisher raises (the program report's event-flow edges run publisher ->
//!   subscriber). Not a background start (F3): its codeunit runs in another
//!   session, which no detector follows.
//! - **Reverse**: for every dependency publisher a PRIMARY routine subscribes to,
//!   the publisher and each routine that calls it (the "raiser": d43 reads its
//!   IsHandled guard and the writes behind it); both then continue forward, so the
//!   publisher's other subscribers (d44) and the raiser's callees join.

use std::collections::{HashMap, HashSet};

use crate::program::graph::ProgramGraph;
use crate::program::node::{AppRef, RoutineNodeId};
use crate::program::resolve::edge::{EdgeKind, RouteTarget};
use crate::program::resolve::full::ClassifiedEdge;

/// The demanded routines: the primary app's, and the dependency routines the rules
/// above reach. `edges` must hold every body's edges (the workspace's report and
/// `ProgramContext::resolve_dependency_bodies`, `ProgramBody::all_edges`), event
/// flow included. They are walked once.
#[must_use]
pub fn cross_app_demand<'a>(
    primary: AppRef,
    graph: &ProgramGraph,
    edges: impl IntoIterator<Item = &'a ClassifiedEdge>,
) -> HashSet<RoutineNodeId> {
    let mut forward: HashMap<&RoutineNodeId, Vec<&RoutineNodeId>> = HashMap::new();
    let mut callers: HashMap<&RoutineNodeId, Vec<&RoutineNodeId>> = HashMap::new();
    let mut publishers_of: HashMap<&RoutineNodeId, Vec<&RoutineNodeId>> = HashMap::new();
    for ce in edges {
        // A background start's target runs in another session: no finding
        // of the starter's own execution reads it, and the model's calls
        // never carry it (F3). Not demanded through it.
        if ce.edge.kind == EdgeKind::BackgroundStart {
            continue;
        }
        let from = &ce.edge.from;
        for route in &ce.edge.routes {
            let RouteTarget::Routine(to) = &route.target else {
                continue;
            };
            forward.entry(from).or_default().push(to);
            if ce.edge.kind == EdgeKind::EventFlow {
                publishers_of.entry(to).or_default().push(from);
            } else {
                callers.entry(to).or_default().push(from);
            }
        }
    }

    let mut demand: HashSet<RoutineNodeId> = HashSet::new();
    let mut work: Vec<&RoutineNodeId> = Vec::new();
    for r in &graph.routines {
        if r.id.object.app == primary && demand.insert(r.id.clone()) {
            work.push(&r.id);
        }
    }
    let add = |id: &RoutineNodeId, demand: &mut HashSet<RoutineNodeId>| demand.insert(id.clone());
    // Reverse demand, once per primary subscriber.
    let mut reverse: Vec<&RoutineNodeId> = Vec::new();
    for sub in work.iter().copied() {
        for &publisher in publishers_of.get(sub).map(Vec::as_slice).unwrap_or(&[]) {
            if publisher.object.app == primary {
                continue;
            }
            reverse.push(publisher);
            reverse.extend(callers.get(publisher).map(Vec::as_slice).unwrap_or(&[]));
        }
    }
    for id in reverse {
        if add(id, &mut demand) {
            work.push(id);
        }
    }
    while let Some(r) = work.pop() {
        for &to in forward.get(r).map(Vec::as_slice).unwrap_or(&[]) {
            if add(to, &mut demand) {
                work.push(to);
            }
        }
    }
    demand
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::program::impact_graph::project::tests::World;
    use crate::program::node::ObjectKind;
    use crate::program::resolve::edge::{DispatchShape, Evidence, Route, Witness};
    use crate::snapshot::TrustTier;

    fn src(rid: &RoutineNodeId) -> Route {
        Route {
            target: RouteTarget::Routine(rid.clone()),
            evidence: Evidence::Source,
            conditions: vec![],
            witness: Witness::None,
            receiver_tier: None,
        }
    }

    /// F3: a dependency codeunit the workspace only starts in the background
    /// is not demanded (it runs in another session; the model's calls never
    /// carry the start); one it runs is.
    ///
    /// DISCRIMINATION (2026-11-05): removing the `BackgroundStart` skip in
    /// `cross_app_demand` fails "a started codeunit is not demanded".
    #[test]
    fn a_background_start_demands_nothing() {
        let mut w = World::default();
        let a = w.app("W");
        let d = w.app("D");
        w.deps.push((a, d));
        let cu = w.obj(a, ObjectKind::Codeunit, 1, "Cu", TrustTier::Workspace);
        let bg = w.obj(d, ObjectKind::Codeunit, 2, "Bg", TrustTier::EmbeddedSource);
        let run = w.obj(d, ObjectKind::Codeunit, 3, "Run", TrustTier::EmbeddedSource);
        let p = w.rtn(&cu, "P");
        let bg_run = w.trigger(&bg, "OnRun");
        let run_run = w.trigger(&run, "OnRun");
        w.background(&p, vec![src(&bg_run)], 1);
        w.edge(
            &p,
            EdgeKind::Run,
            DispatchShape::Multicast,
            vec![src(&run_run)],
            2,
        );
        let demand = w.with_inputs(a, |inp| {
            cross_app_demand(a, inp.graph, inp.edges.all_edges())
        });
        assert!(demand.contains(&run_run), "a run codeunit is demanded");
        assert!(
            !demand.contains(&bg_run),
            "a started codeunit is not demanded"
        );
    }
}

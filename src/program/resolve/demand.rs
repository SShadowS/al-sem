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
//!   subscriber).
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
/// `ProgramContext::resolve_dependency_bodies`), event flow included.
#[must_use]
pub fn cross_app_demand(
    primary: AppRef,
    graph: &ProgramGraph,
    edges: &[ClassifiedEdge],
) -> HashSet<RoutineNodeId> {
    let mut forward: HashMap<&RoutineNodeId, Vec<&RoutineNodeId>> = HashMap::new();
    let mut callers: HashMap<&RoutineNodeId, Vec<&RoutineNodeId>> = HashMap::new();
    let mut publishers_of: HashMap<&RoutineNodeId, Vec<&RoutineNodeId>> = HashMap::new();
    for ce in edges {
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

//! Closed-world temporary records through `local` events (engine-switch S8).
//!
//! An event subscriber's keyword-less `var Record` parameter is the record the
//! RAISER passed: its temp state is `ParameterDependent`, and every consumer
//! that reads the subscriber on its own (d44/d45 read subscribers' cones)
//! counts its writes as physical. When the publisher is a `local` procedure,
//! the language fixes who can raise it: only routines of the publisher's own
//! object. If every such raise is resolved and passes a `temporary` record for
//! that parameter, each subscriber's same-named parameter (AL binds subscriber
//! parameters by NAME) is temporary on every path. Continia Core's
//! `OnRequestAppFeatureInformation` is this shape: one raise, a
//! `TempAppFeature` buffer.
//!
//! The proof is a model fact, so it is written into the model: the
//! subscriber's parameter record variable, its record operations, and the call
//! bindings and receivers that read it become `Known(true)`. Every rule below
//! that cannot be checked fails the proof and leaves the parameter as it was
//! (the firing direction):
//!   1. the publisher is an `event-publisher` with `local` access, and no
//!      routine of its object is `parse_incomplete`;
//!   2. no call site in its object that could name it is unresolved;
//!   3. it is raised at least once, and every raise binds the parameter to a
//!      `Known(true)` source.

use std::collections::{HashMap, HashSet};

use super::workspace::{L3Resolved, L3Routine};
use crate::program::body::features::{PCallee, PTempState};

fn unq(s: &str) -> &str {
    s.trim().trim_matches('"')
}

fn is_pd(ts: &PTempState, j: u32) -> bool {
    ts.kind == "parameter-dependent" && ts.parameter_index == Some(j)
}

fn is_known_temp(ts: Option<&PTempState>) -> bool {
    crate::program::body::features::temp_state_suppresses(ts)
}

fn known_temp() -> PTempState {
    crate::program::body::scope::ts_known(true)
}

/// Prove and write the temporary subscriber parameters (see the module docs).
pub fn prove_event_param_temps(resolved: &mut L3Resolved) {
    let (calls, events) = (resolved.calls.clone(), resolved.events.clone());
    let rewrites: Vec<(usize, u32)> = {
        let routines = &resolved.workspace.routines;
        let index: HashMap<&str, usize> = routines
            .iter()
            .enumerate()
            .map(|(i, r)| (r.id.as_str(), i))
            .collect();
        let resolved_sites: HashSet<&str> = calls
            .edges
            .iter()
            .filter(|e| e.to.is_some())
            .map(|e| e.callsite_id.as_str())
            .collect();
        let mut raises: HashMap<&str, Vec<(&str, &str)>> = HashMap::new();
        for e in &calls.edges {
            if let Some(to) = &e.to {
                raises
                    .entry(to.as_str())
                    .or_default()
                    .push((e.from.as_str(), e.callsite_id.as_str()));
            }
        }
        let routine = |id: &str| index.get(id).map(|&i| &routines[i]);
        let complete = |p: &L3Routine| {
            routines
                .iter()
                .filter(|q| q.object_id == p.object_id)
                .all(|q| {
                    !q.parse_incomplete
                        && q.call_sites.iter().all(|cs| {
                            resolved_sites.contains(cs.id.as_str())
                                || match &cs.callee {
                                    PCallee::Bare { name } => {
                                        !unq(name).eq_ignore_ascii_case(unq(&p.name))
                                    }
                                    PCallee::Member { method, .. } => {
                                        !unq(method).eq_ignore_ascii_case(unq(&p.name))
                                    }
                                    PCallee::Unknown => false,
                                    _ => true,
                                }
                        })
                })
        };
        let mut out = Vec::new();
        for ev in &events.graph.events {
            let Some(p) = ev.publisher_routine_id.as_deref().and_then(routine) else {
                continue;
            };
            if p.kind != "event-publisher"
                || p.access_modifier.as_deref() != Some("local")
                || !complete(p)
            {
                continue;
            }
            let Some(sites) = raises.get(p.id.as_str()) else {
                continue;
            };
            for pv in p.record_variables.iter().filter(|rv| rv.is_parameter) {
                let Some(i) = pv.parameter_index else {
                    continue;
                };
                let all_temp = sites.iter().all(|(from, cs_id)| {
                    routine(from)
                        .and_then(|r| r.call_sites.iter().find(|c| c.id == *cs_id))
                        .is_some_and(|cs| is_known_temp(cs.source_temp_state_for(i)))
                });
                if !all_temp {
                    continue;
                }
                for edge in events.graph.edges.iter().filter(|e| e.event_id == ev.id) {
                    let Some(&si) = index.get(edge.subscriber_routine_id.as_str()) else {
                        continue;
                    };
                    let s = &routines[si];
                    let Some(sp) = s
                        .parameters
                        .iter()
                        .find(|sp| unq(&sp.name).eq_ignore_ascii_case(unq(&pv.name)))
                    else {
                        continue;
                    };
                    if s.record_variables.iter().any(|rv| {
                        rv.is_parameter
                            && rv.parameter_index == Some(sp.index)
                            && is_pd(&rv.temp_state, sp.index)
                    }) {
                        out.push((si, sp.index));
                    }
                }
            }
        }
        out
    };
    for (si, j) in rewrites {
        let s = &mut resolved.workspace.routines[si];
        for rv in &mut s.record_variables {
            if rv.is_parameter && rv.parameter_index == Some(j) {
                rv.temp_state = known_temp();
            }
        }
        for op in &mut s.record_operations {
            if op.temp_state.as_ref().is_some_and(|t| is_pd(t, j)) {
                op.temp_state = Some(known_temp());
            }
        }
        for cs in &mut s.call_sites {
            for b in &mut cs.argument_bindings {
                if b.source_temp_state.as_ref().is_some_and(|t| is_pd(t, j)) {
                    b.source_temp_state = Some(known_temp());
                }
            }
            if cs.receiver_temp_state.as_ref().is_some_and(|t| is_pd(t, j)) {
                cs.receiver_temp_state = Some(known_temp());
            }
        }
    }
}

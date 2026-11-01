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

use super::workspace::{Model, ModelRoutine};
use crate::program::body::features::{PCallee, PTempState};

fn unq(s: &str) -> &str {
    s.trim().trim_matches('"')
}

/// The ONE event-parameter name match (#107 Design 1): AL binds publisher and
/// subscriber parameters by NAME, ASCII case-insensitively, quotes and
/// surrounding whitespace ignored. Both directions of the mapping call this.
pub(crate) fn event_param_names_match(a: &str, b: &str) -> bool {
    unq(a).eq_ignore_ascii_case(unq(b))
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

/// Every row index of each object id, in row order: the `complete(p)` index
/// (context-server design §6.8 L8).
fn rows_by_object(routines: &[ModelRoutine]) -> HashMap<&str, Vec<usize>> {
    let mut by_object: HashMap<&str, Vec<usize>> = HashMap::new();
    for (i, r) in routines.iter().enumerate() {
        by_object.entry(r.object_id.as_str()).or_default().push(i);
    }
    by_object
}

/// Prove and write the temporary subscriber parameters (see the module docs).
pub fn prove_event_param_temps(resolved: &mut Model) {
    let (calls, events) = (resolved.calls.clone(), resolved.events.clone());
    let rewrites: Vec<(usize, u32)> = {
        let routines = &resolved.workspace.routines;
        // #17 (#160 step 11b §4.6): the publisher is read through its LAST row
        // (class P); each raise at its own row (`CallEdge.from_ix`); a proof
        // rewrites EVERY row of a subscriber id.
        let rows = super::projection::RowsOfId::build(routines);
        let index: HashMap<&str, usize> =
            super::projection::last_per_id_borrowed(&rows, (0..routines.len()).map(Some).collect());
        // Resolved per `(row, site)`: the rows of a collided id share site ids, so
        // one row's resolved site must not vouch for another row's.
        let resolved_sites: HashSet<(usize, &str)> = calls
            .edges
            .iter()
            .filter(|e| e.to.is_some())
            .map(|e| (e.from_ix.get(), e.callsite_id.as_str()))
            .collect();
        let mut raises: HashMap<&str, Vec<(usize, &str)>> = HashMap::new();
        for e in &calls.edges {
            if let Some(to) = &e.to {
                raises
                    .entry(to.as_str())
                    .or_default()
                    .push((e.from_ix.get(), e.callsite_id.as_str()));
            }
        }
        let routine = |id: &str| index.get(id).map(|&i| &routines[i]);
        // The rows of each object, in row order (context-server design §6.8 L8):
        // `complete(p)` reads only the rows of `p`'s object, so it scans those
        // instead of every row of the model per publisher. The predicate is
        // pure, so the visiting order cannot change the answer.
        let by_object = rows_by_object(routines);
        let complete = |p: &ModelRoutine| {
            by_object
                .get(p.object_id.as_str())
                .map_or(&[][..], Vec::as_slice)
                .iter()
                .map(|&qi| (qi, &routines[qi]))
                .all(|(qi, q)| {
                    !q.parse_incomplete
                        && q.call_sites.iter().all(|cs| {
                            resolved_sites.contains(&(qi, cs.id.as_str()))
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
                let all_temp = sites.iter().all(|&(from_ix, cs_id)| {
                    routines
                        .get(from_ix)
                        .and_then(|r| r.call_sites.iter().find(|c| c.id == cs_id))
                        .is_some_and(|cs| is_known_temp(cs.source_temp_state_for(i)))
                });
                if !all_temp {
                    continue;
                }
                for edge in events.graph.edges.iter().filter(|e| e.event_id == ev.id) {
                    for &six in rows.rows(edge.subscriber_routine_id.as_str()) {
                        let si = six.get();
                        let s = &routines[si];
                        let Some(sp) = s
                            .parameters
                            .iter()
                            .find(|sp| event_param_names_match(&sp.name, &pv.name))
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

#[cfg(test)]
mod tests {
    use crate::program::model::program_calls::assemble_and_resolve_inline_program;

    /// Rule 2 reads the publisher's OWN object only (`complete(p)` over the
    /// `object_id → rows` index). Hand-stated: another codeunit holds an
    /// unresolved bare call named like the publisher, `OnX(Rec)`; the
    /// publisher's object raises it once with a temporary record. The proof
    /// still holds. Break: index every row under one key (the scan then sees
    /// the other object's unresolved `OnX`) -> the subscriber stays unproven.
    #[test]
    fn complete_reads_only_the_publishers_object() {
        let src = r#"
table 50890 "EI T"
{
    fields { field(1; "No."; Code[20]) { } }
    keys { key(PK; "No.") { } }
}

codeunit 50890 "EI Pub"
{
    [IntegrationEvent(false, false)]
    local procedure OnX(var Rec: Record "EI T")
    begin
    end;

    procedure Raise()
    var
        Rec: Record "EI T" temporary;
    begin
        OnX(Rec);
    end;
}

codeunit 50891 "EI Other"
{
    procedure Unrelated()
    var
        Rec: Record "EI T";
    begin
        OnX(Rec);
    end;
}

codeunit 50892 "EI Sub"
{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::"EI Pub", 'OnX', '', false, false)]
    local procedure Handle(var Rec: Record "EI T")
    begin
        Rec.Insert();
    end;
}
"#;
        let m = assemble_and_resolve_inline_program(
            &[("Ei.al".to_string(), src.to_string())],
            "22222222-0000-0000-0000-0000000e1a01",
        );
        let routine = |name: &str| {
            m.workspace
                .routines
                .iter()
                .find(|r| r.name == name)
                .unwrap_or_else(|| panic!("routine {name}"))
        };
        let other = routine("Unrelated");
        assert!(
            !m.calls
                .edges
                .iter()
                .any(|e| e.from == other.id && e.to.is_some()),
            "precondition: the other object's OnX call is unresolved"
        );
        assert!(
            !m.events.graph.edges.is_empty(),
            "precondition: Handle subscribes to OnX"
        );
        let handle = routine("Handle");
        let params: Vec<_> = handle
            .record_variables
            .iter()
            .filter(|rv| rv.is_parameter)
            .collect();
        assert!(
            !params.is_empty(),
            "precondition: Handle has a record parameter"
        );
        assert!(
            params.iter().all(|rv| {
                crate::program::body::features::temp_state_suppresses(Some(&rv.temp_state))
            }),
            "another object's unresolved OnX does not block the proof: {params:?}"
        );
    }
}

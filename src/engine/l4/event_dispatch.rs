//! Dispatch bindings of an event publisher (#107 Design 2): which subscriber
//! parameter binds which record parameter of the publisher, and whether the
//! subscriber may not run. Built once per solve, read by the roles step.

use std::collections::HashMap;

use super::combined_graph::CombinedEdge;
use super::effect_lattice::EffectPresence;
use super::summary_runner::event_publisher_param;
use crate::program::model::event_param_temp::event_param_names_match;
use crate::program::model::workspace::ModelRoutine;

/// Bindings per (publisher, parameter) above this are an unseen dispatch.
pub(crate) const EXACT_DISPATCH_MAX: usize = 4;

/// One visible subscriber parameter `q` bound to a publisher parameter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ExactBinding {
    pub subscriber: String,
    pub q: u32,
    pub callee_is_var: bool,
    /// The subscriber may not run (a conditional subscription or an isolated
    /// event).
    pub optional: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DispatchKind {
    /// An opaque subscriber (non-resolved edge, unknown subscriber, or an
    /// overload collision), or more than [`EXACT_DISPATCH_MAX`] bindings.
    Unseen,
    Exact(Vec<ExactBinding>),
}

/// The dispatch of one record parameter of a publisher.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParamDispatch {
    pub param_index: u32,
    pub caller_is_var: bool,
    pub kind: DispatchKind,
}

/// A subscriber whose effects cannot be seen: the edge is not `resolved`, the
/// subscriber is unknown, or (step-6 Ruling, overloads share one event id) one
/// of its parameters other than `Sender` binds no parameter of THIS overload.
fn opaque_subscriber(
    publisher: &ModelRoutine,
    edge: &CombinedEdge,
    routines_by_id: &HashMap<String, &ModelRoutine>,
) -> bool {
    if edge.resolution != "resolved" {
        return true;
    }
    let Some(s) = routines_by_id.get(&edge.to) else {
        return true;
    };
    s.parameters.iter().any(|q| {
        !event_param_names_match(&q.name, "sender")
            && event_publisher_param(publisher, s, q.index).is_none()
    })
}

/// `routine`'s dispatch bindings, one entry per record parameter with at least
/// one binding, in parameter order. Empty when `routine` has no
/// `event-dispatch` edge or no body (a symbol-only publisher keeps its base
/// roles, Design 6).
pub(crate) fn dispatch_bindings(
    routine: &ModelRoutine,
    edges: &[CombinedEdge],
    routines_by_id: &HashMap<String, &ModelRoutine>,
) -> Vec<ParamDispatch> {
    if !routine.body_available {
        return Vec::new();
    }
    let dispatch: Vec<&CombinedEdge> = edges
        .iter()
        .filter(|e| e.kind == "event-dispatch")
        .collect();
    if dispatch.is_empty() {
        return Vec::new();
    }
    // Any opaque subscriber makes every record parameter unseen.
    let unseen = dispatch
        .iter()
        .any(|e| opaque_subscriber(routine, e, routines_by_id));
    let mut out = Vec::new();
    for p in routine.parameters.iter().filter(|p| p.is_record) {
        let kind = if unseen {
            DispatchKind::Unseen
        } else {
            let mut bindings: Vec<ExactBinding> = Vec::new();
            for e in &dispatch {
                let s = routines_by_id[&e.to];
                let optional = e
                    .dispatch
                    .as_ref()
                    .is_some_and(|d| d.conditional || d.isolated);
                for q in &s.parameters {
                    if event_publisher_param(routine, s, q.index).map(|pp| pp.index)
                        != Some(p.index)
                    {
                        continue;
                    }
                    // Two edges to one subscriber are one subscriber.
                    match bindings
                        .iter_mut()
                        .find(|b| b.subscriber == s.id && b.q == q.index)
                    {
                        Some(b) => b.optional |= optional,
                        None => bindings.push(ExactBinding {
                            subscriber: s.id.clone(),
                            q: q.index,
                            callee_is_var: q.is_var,
                            optional,
                        }),
                    }
                }
            }
            match bindings.len() {
                0 => continue,
                n if n > EXACT_DISPATCH_MAX => DispatchKind::Unseen,
                _ => DispatchKind::Exact(bindings),
            }
        };
        out.push(ParamDispatch {
            param_index: p.index,
            caller_is_var: p.is_var,
            kind,
        });
    }
    out
}

/// The published load trio of an OPTIONAL binding: a subscriber that may not
/// run neither gains nor destroys a load (`yes -> no`, else unchanged).
pub(crate) fn optional_load(v: EffectPresence) -> EffectPresence {
    match v {
        EffectPresence::Yes => EffectPresence::No,
        other => other,
    }
}

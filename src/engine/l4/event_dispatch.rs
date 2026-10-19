//! Dispatch bindings of an event publisher (#107 Design 2): which subscriber
//! parameter binds which record parameter of the publisher, and whether the
//! subscriber may not run. Built once per solve, read by the roles step.

use std::collections::{HashMap, HashSet};

use super::combined_graph::{CombinedEdge, CombinedGraph};
use super::effect_lattice::EffectPresence;
use super::summary_runner::{event_publisher_param, summaries_census};
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
    /// The subscriber may not run (a conditional subscription, an isolated
    /// event, or a shared overload event id). Always true when `path_opaque`.
    pub optional: bool,
    /// An isolated event (the subscriber is visible, but a failure rolls its
    /// database work back while its in-memory changes stay) or a shared
    /// overload event id (the binding may belong to another overload): the
    /// walker takes the opaque transfer (Design 4).
    pub path_opaque: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DispatchKind {
    /// An opaque subscriber (non-resolved edge, unknown subscriber, or an
    /// overload collision), or (`too_many`) more than [`EXACT_DISPATCH_MAX`]
    /// bindings. The two read alike; only the census tells them apart.
    Unseen {
        too_many: bool,
    },
    Exact(Vec<ExactBinding>),
}

/// The dispatch of one record parameter of a publisher.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParamDispatch {
    pub param_index: u32,
    pub caller_is_var: bool,
    pub kind: DispatchKind,
    /// The walker's runs ([`dispatch_sequences`]) of an exact dispatch with no
    /// isolated binding, built once per solve; empty otherwise (the walker
    /// takes the opaque transfer then).
    pub sequences: Vec<Vec<usize>>,
}

/// The event ids that more than one publisher routine raises: overloads share
/// one event id (step-6 Ruling), so each of their subscribers is attached to
/// every overload, though it selected only one.
pub(crate) fn shared_event_ids(graph: &CombinedGraph) -> HashSet<String> {
    let mut publishers: HashMap<&str, &str> = HashMap::new();
    let mut shared = HashSet::new();
    for e in graph.edges_by_from.values().flatten() {
        let Some(ev) = e.event_id.as_deref().filter(|_| e.kind == "event-dispatch") else {
            continue;
        };
        if *publishers.entry(ev).or_insert(&e.from) != e.from {
            shared.insert(ev.to_string());
        }
    }
    shared
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
/// roles, Design 6). A binding over a `shared_events` id is optional and
/// path-opaque: the engine does not keep which overload a subscriber selected,
/// so it may not run for this one, and its names may bind the wrong overload.
pub(crate) fn dispatch_bindings(
    routine: &ModelRoutine,
    edges: &[CombinedEdge],
    routines_by_id: &HashMap<String, &ModelRoutine>,
    shared_events: &HashSet<String>,
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
            DispatchKind::Unseen { too_many: false }
        } else {
            let mut bindings: Vec<ExactBinding> = Vec::new();
            for e in &dispatch {
                let s = routines_by_id[&e.to];
                let overload = e
                    .event_id
                    .as_ref()
                    .is_some_and(|ev| shared_events.contains(ev));
                let isolated = overload || e.dispatch.as_ref().is_some_and(|d| d.isolated);
                let optional = isolated || e.dispatch.as_ref().is_some_and(|d| d.conditional);
                for q in &s.parameters {
                    if event_publisher_param(routine, s, q.index).map(|pp| pp.index)
                        != Some(p.index)
                    {
                        continue;
                    }
                    // Two edges to one subscriber are one subscriber: it may be
                    // skipped only if every edge may skip it, and it is isolated
                    // if any edge is. An isolated subscriber may not run to
                    // completion, so it stays optional (`path_opaque =>
                    // optional`).
                    match bindings
                        .iter_mut()
                        .find(|b| b.subscriber == s.id && b.q == q.index)
                    {
                        Some(b) => {
                            b.path_opaque |= isolated;
                            b.optional = (b.optional && optional) || b.path_opaque;
                        }
                        None => bindings.push(ExactBinding {
                            subscriber: s.id.clone(),
                            q: q.index,
                            callee_is_var: q.is_var,
                            optional,
                            path_opaque: isolated,
                        }),
                    }
                }
            }
            let n = bindings.len();
            if n > 0 {
                summaries_census::bindings_per_param(n);
            }
            match n {
                0 => continue,
                n if n > EXACT_DISPATCH_MAX => DispatchKind::Unseen { too_many: true },
                _ => DispatchKind::Exact(bindings),
            }
        };
        let sequences = match &kind {
            DispatchKind::Exact(b) if !b.iter().any(|b| b.path_opaque) => dispatch_sequences(b),
            _ => Vec::new(),
        };
        out.push(ParamDispatch {
            param_index: p.index,
            caller_is_var: p.is_var,
            kind,
            sequences,
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

/// The runs of an exact dispatch (Design 4), as binding indices in run order:
/// for every subset of the optional bindings, every order of (unconditional
/// bindings ∪ subset). The empty sequence appears only when no binding always
/// runs. At most 65 sequences for [`EXACT_DISPATCH_MAX`] bindings.
pub(crate) fn dispatch_sequences(bindings: &[ExactBinding]) -> Vec<Vec<usize>> {
    let (optional, always): (Vec<usize>, Vec<usize>) =
        (0..bindings.len()).partition(|&i| bindings[i].optional);
    let mut out = Vec::new();
    for mask in 0..1u32 << optional.len() {
        let mut run = always.clone();
        run.extend(
            optional
                .iter()
                .enumerate()
                .filter(|(k, _)| mask >> k & 1 == 1)
                .map(|(_, &i)| i),
        );
        permute(&mut run, 0, &mut out);
    }
    out
}

/// Push every order of `v[k..]` (behind the fixed prefix `v[..k]`) to `out`.
fn permute(v: &mut [usize], k: usize, out: &mut Vec<Vec<usize>>) {
    if k == v.len() {
        out.push(v.to_vec());
        return;
    }
    for i in k..v.len() {
        v.swap(k, i);
        permute(v, k + 1, out);
        v.swap(k, i);
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    fn b(optional: bool) -> ExactBinding {
        ExactBinding {
            subscriber: "S".to_string(),
            q: 0,
            callee_is_var: true,
            optional,
            path_opaque: false,
        }
    }

    fn set(v: Vec<Vec<usize>>) -> HashSet<Vec<usize>> {
        let n = v.len();
        let s: HashSet<Vec<usize>> = v.into_iter().collect();
        assert_eq!(s.len(), n, "sequences must be distinct");
        s
    }

    /// Design 4: every subset of the optional bindings x every order of
    /// (unconditional ∪ subset); the empty sequence only without an
    /// unconditional binding.
    #[test]
    fn dispatch_sequences_cover_every_subset_and_order() {
        // 4 optional: 1 + 4 + 12 + 24 + 24.
        let s = set(dispatch_sequences(&[b(true), b(true), b(true), b(true)]));
        assert_eq!(s.len(), 65);
        // 4 unconditional: every order, each of length 4.
        let s = dispatch_sequences(&[b(false), b(false), b(false), b(false)]);
        assert!(s.iter().all(|q| q.len() == 4));
        assert_eq!(set(s).len(), 24);
        assert_eq!(dispatch_sequences(&[b(false)]), vec![vec![0]]);
        // Row 4b: no empty sequence when one binding always runs.
        assert_eq!(
            set(dispatch_sequences(&[b(false), b(true)])),
            HashSet::from([vec![0], vec![0, 1], vec![1, 0]])
        );
        assert_eq!(
            set(dispatch_sequences(&[b(true)])),
            HashSet::from([vec![], vec![0]])
        );
    }
}

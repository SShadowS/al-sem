//! #160 step 4 (epic §5.5): [`ExecRelation`], the one execution relation a
//! view builds: the combined graph, its SCC, and the dependency half's
//! uncertainty edges, from one call split.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use super::combined_graph::{
    CombinedEdge, CombinedGraph, DispatchMeta, TriggerFires, TypedEdge, Uncertainty,
    UncertaintyEdge, ValueSource, edge_sort_key,
};
use super::scc::SccResult;
use crate::engine::perf_trace as pt;
use crate::program::body::features::PCallee;
use crate::program::model::calls::CallEdge;
use crate::program::model::events::EventGraph;
use crate::program::model::taxonomy::{DispatchKind, Resolution};
use crate::program::model::workspace::ModelEntities;

/// One view's execution relation (#160 step 4, epic §5.5). The two transfer
/// rules are the two accessors; spec §4.3's field table says which consumer
/// reads which.
///
/// `CombinedGraph` stays the carrier (amendment M1): 32 source files read its
/// fields through a `graph.` value and 13 detectors read `edges_by_from` as a
/// map, so changing the container is step 11's job, when every key becomes a
/// `RoutineIx`.
///
/// The transfer rules are two accessors, not an `enum Transfer` value (M2):
/// each consumer's rule is fixed. The type guarantee is ONE-directional: the
/// cone takes `&[TypedEdge]`, so it cannot read the Primary payload. The
/// reverse is a naming convention until step 11 splits the container:
/// [`ExecRelation::primary`] returns the whole carrier, typed edges included
/// (d43 reads the Cone payload through `ctx.graph.typed_edges`).
pub struct ExecRelation {
    /// Primary payload in `edges_by_from` / `edges_from_order` /
    /// `uncertainty_edges` / `nodes`; Cone payload in `typed_edges` (primary
    /// half with event dispatch, then the dependency half, in that order).
    pub graph: Arc<CombinedGraph>,
    /// Tarjan over the Primary payload, computed in [`ExecRelation::new`].
    pub scc: Arc<SccResult>,
    /// The dependency half's uncertainty edges, in the order the half-build
    /// sorts them. No reader until step 5.
    pub dependency_uncertainty: Vec<UncertaintyEdge>,
}

impl ExecRelation {
    /// Build the relation from one call split (spec §4.2). `primary` holds the
    /// calls of WORKSPACE callers, `dependency` the calls dependency routines
    /// make (empty single-app). The Primary payload is assembled over
    /// `primary` plus every event dispatch in `events`; the dependency half is
    /// assembled over an empty event graph (its dispatch is already in the
    /// primary half), and only its typed edges (appended to the Cone payload)
    /// and its uncertainty edges (kept apart) survive. Counts one
    /// `build.combined_graph`, however many halves it assembles.
    pub fn new(
        ws: &ModelEntities,
        primary: &[CallEdge],
        dependency: &[CallEdge],
        events: &EventGraph,
    ) -> ExecRelation {
        pt::counter_delta("build.combined_graph", 1);
        let _s = pt::span("rel", "rel.build");
        let mut graph = assemble(ws, primary, events);
        let mut dependency_uncertainty = Vec::new();
        if !dependency.is_empty() {
            let d = assemble(ws, dependency, &EventGraph::default());
            graph.typed_edges.extend(d.typed_edges);
            dependency_uncertainty = d.uncertainty_edges;
        }
        // `graph_scc` reads `nodes` and `edges_by_from` only, so the typed-edge
        // extend above cannot move it.
        let scc = super::analysis::graph_scc(&graph);
        if crate::engine::census::enabled() {
            use crate::engine::census::{Kind, Tally, combined_graph, scc as scc_tally};
            combined_graph(&graph).emit("rel.graph", Kind::Shallow);
            scc_tally(&scc).emit("rel.scc", Kind::Shallow);
            Tally::default()
                .buf::<UncertaintyEdge>(dependency_uncertainty.len())
                .emit("rel.dependency_uncertainty", Kind::Shallow);
        }
        ExecRelation {
            graph: Arc::new(graph),
            scc: Arc::new(scc),
            dependency_uncertainty,
        }
    }

    /// `Transfer::Cone`: every typed edge, every caller.
    pub fn cone(&self) -> &[TypedEdge] {
        &self.graph.typed_edges
    }

    /// `Transfer::Primary`: primary callers' call edges plus event dispatch
    /// from any publisher.
    pub fn primary(&self) -> &Arc<CombinedGraph> {
        &self.graph
    }
}

// ---------------------------------------------------------------------------
// The assembler: a COPY of `combined_graph::build_combined_graph`'s body (minus
// its counter) and of `build_typed_edges` with the helpers they call, over
// `&[CallEdge]`. Commit B of step 4 replaces it with the one shared assembler;
// until then the parity oracle (`tests`) compares it with the untouched
// original, independent code.
// ---------------------------------------------------------------------------

/// `uncertaintySortKey(ue)` = `${from}|${u.kind}|${ref}` where ref is the
/// callsiteId, else operationId, else routineId.
fn uncertainty_sort_key(ue: &UncertaintyEdge) -> String {
    let u = &ue.uncertainty;
    let r = u
        .callsite_id
        .clone()
        .or_else(|| u.operation_id.clone())
        .or_else(|| u.routine_id.clone())
        .unwrap_or_default();
    format!("{}|{}|{}", ue.from, u.kind, r)
}

/// Enum-typed: CallEdge dispatch kinds that become resolved combined edges.
fn is_edge_kind_enum(kind: DispatchKind) -> bool {
    matches!(
        kind,
        DispatchKind::Direct
            | DispatchKind::Method
            | DispatchKind::CodeunitRun
            | DispatchKind::ReportRun
            | DispatchKind::PageRun
            | DispatchKind::Interface
            | DispatchKind::ImplicitTrigger
            | DispatchKind::Dynamic
    )
}

/// The object-run dispatch kinds that map to typed GraphEdge object-run variants.
fn is_object_run_kind(kind: &str) -> bool {
    matches!(kind, "codeunit-run" | "page-run" | "report-run")
}

/// Map a call-graph dispatchKind to the GraphEdge objectType field.
fn dispatch_kind_to_object_type(kind: &str) -> Option<&'static str> {
    match kind {
        "codeunit-run" => Some("Codeunit"),
        "page-run" => Some("Page"),
        "report-run" => Some("Report"),
        _ => None,
    }
}

/// The combined graph over `edges` and `event_graph` (the body of
/// `build_combined_graph`, uncounted).
fn assemble(
    workspace: &ModelEntities,
    call_edges: &[CallEdge],
    event_graph: &EventGraph,
) -> CombinedGraph {
    let mut edges: Vec<CombinedEdge> = Vec::new();
    let mut uncertainty_edges: Vec<UncertaintyEdge> = Vec::new();

    // Interface callsites that already received an "interface-open-world" uncertainty
    // (emit exactly once per callsite even with multiple "maybe" edges).
    let mut interface_open_world_emitted: HashSet<String> = HashSet::new();

    // --- call-derived edges + uncertainty records ---
    for ce in call_edges {
        // event-dispatch dispatchKind never appears in the model's calls; al-sem
        // skips it here regardless (event hops come from the event graph), so no
        // check is needed.
        if let Some(to) = &ce.to {
            if is_edge_kind_enum(ce.dispatch_kind) {
                edges.push(CombinedEdge {
                    from: ce.from.clone(),
                    to: to.clone(),
                    kind: ce.dispatch_kind.as_str().to_string(),
                    callsite_id: Some(ce.callsite_id.clone()),
                    operation_id: Some(ce.operation_id.clone()),
                    event_id: None,
                    subscriber_app_id: None,
                    resolution: ce.resolution.as_str().to_string(),
                    dispatch: None,
                });
            }
            // Resolved interface dispatch ("maybe"): open-world uncertainty, once.
            if ce.dispatch_kind == DispatchKind::Interface
                && !interface_open_world_emitted.contains(&ce.callsite_id)
            {
                interface_open_world_emitted.insert(ce.callsite_id.clone());
                let interface_name = ce
                    .dispatch_meta
                    .as_ref()
                    .map(|dm| dm.interface_name.clone())
                    .unwrap_or_default();
                uncertainty_edges.push(UncertaintyEdge {
                    from: ce.from.clone(),
                    uncertainty: Uncertainty {
                        kind: "interface-open-world".to_string(),
                        callsite_id: Some(ce.callsite_id.clone()),
                        operation_id: None,
                        routine_id: None,
                        interface_name: Some(interface_name),
                    },
                });
            }
            continue;
        }
        // to-less edge → typed uncertainty on the `from` routine.
        if ce.dispatch_kind == DispatchKind::Interface {
            // Zero-impl interface dispatch → "interface-open-world".
            if !interface_open_world_emitted.contains(&ce.callsite_id) {
                interface_open_world_emitted.insert(ce.callsite_id.clone());
                let interface_name = ce
                    .dispatch_meta
                    .as_ref()
                    .map(|dm| dm.interface_name.clone())
                    .unwrap_or_default();
                uncertainty_edges.push(UncertaintyEdge {
                    from: ce.from.clone(),
                    uncertainty: Uncertainty {
                        kind: "interface-open-world".to_string(),
                        callsite_id: Some(ce.callsite_id.clone()),
                        operation_id: None,
                        routine_id: None,
                        interface_name: Some(interface_name),
                    },
                });
            }
        } else if ce.dispatch_kind == DispatchKind::Dynamic {
            uncertainty_edges.push(UncertaintyEdge {
                from: ce.from.clone(),
                uncertainty: simple_uncertainty_op("dynamic-dispatch", &ce.operation_id),
            });
        } else if ce.resolution == Resolution::Ambiguous {
            uncertainty_edges.push(UncertaintyEdge {
                from: ce.from.clone(),
                uncertainty: simple_uncertainty_cs("ambiguous-overload", &ce.callsite_id),
            });
        } else if ce.resolution == Resolution::MemberNotFound {
            uncertainty_edges.push(UncertaintyEdge {
                from: ce.from.clone(),
                uncertainty: simple_uncertainty_cs("member-not-found", &ce.callsite_id),
            });
        } else if ce.resolution == Resolution::ExternalTarget {
            uncertainty_edges.push(UncertaintyEdge {
                from: ce.from.clone(),
                uncertainty: simple_uncertainty_cs("external-target", &ce.callsite_id),
            });
        } else if ce.resolution == Resolution::Builtin {
            // Recognized global builtin — known terminal, no uncertainty.
        } else {
            uncertainty_edges.push(UncertaintyEdge {
                from: ce.from.clone(),
                uncertainty: simple_uncertainty_cs("unresolved-call", &ce.callsite_id),
            });
        }
    }

    // --- event-dispatch edges: publisher routine → subscriber routine ---
    let mut subs_by_event: HashMap<String, Vec<&crate::program::model::events::EventEdge>> =
        HashMap::new();
    for ee in &event_graph.edges {
        subs_by_event
            .entry(ee.event_id.clone())
            .or_default()
            .push(ee);
    }
    for sym in &event_graph.events {
        let Some(publisher_routine_id) = &sym.publisher_routine_id else {
            continue;
        };
        if let Some(list) = subs_by_event.get(&sym.id) {
            for ee in list {
                if ee.mixed_readings {
                    super::summary_runner::summaries_census::mixed_readings();
                }
                edges.push(CombinedEdge {
                    from: publisher_routine_id.clone(),
                    to: ee.subscriber_routine_id.clone(),
                    kind: "event-dispatch".to_string(),
                    callsite_id: None,
                    operation_id: None,
                    event_id: Some(sym.id.clone()),
                    subscriber_app_id: Some(ee.subscriber_app_id.clone()),
                    resolution: ee.resolution.clone(),
                    dispatch: Some(DispatchMeta {
                        conditional: ee.conditional,
                        isolated: sym.isolated == Some(true),
                    }),
                });
            }
        }
    }

    // --- assemble: sorted nodes, sorted per-from edge lists, sorted uncertainties ---
    let mut nodes: Vec<String> = workspace.routines.iter().map(|r| r.id.clone()).collect();
    nodes.sort();

    let mut edges_by_from: HashMap<String, Vec<CombinedEdge>> = HashMap::new();
    // Track the first-appearance order of `from` keys (mirrors al-sem JS Map insertion
    // order: `for (const e of edges) edgesByFrom.set(e.from, ...)` where Set preserves
    // insertion order). Used by d17 to select the first sample in emission order.
    let mut edges_from_order: Vec<String> = Vec::new();
    for e in edges {
        if !edges_by_from.contains_key(&e.from) {
            edges_from_order.push(e.from.clone());
        }
        edges_by_from.entry(e.from.clone()).or_default().push(e);
    }
    for list in edges_by_from.values_mut() {
        list.sort_by_key(edge_sort_key);
    }

    uncertainty_edges.sort_by_key(uncertainty_sort_key);

    let typed_edges = build_typed_edges(workspace, call_edges, event_graph);

    CombinedGraph {
        nodes,
        edges_by_from,
        edges_from_order,
        uncertainty_edges,
        typed_edges,
    }
}

fn simple_uncertainty_cs(kind: &str, callsite_id: &str) -> Uncertainty {
    Uncertainty {
        kind: kind.to_string(),
        callsite_id: Some(callsite_id.to_string()),
        operation_id: None,
        routine_id: None,
        interface_name: None,
    }
}

fn simple_uncertainty_op(kind: &str, operation_id: &str) -> Uncertainty {
    Uncertainty {
        kind: kind.to_string(),
        callsite_id: None,
        operation_id: Some(operation_id.to_string()),
        routine_id: None,
        interface_name: None,
    }
}

/// Build a ValueSource for an object-run callsite's target id (al-sem
/// `objectRunTargetIdSource`).
fn object_run_target_id_source(callee: &PCallee) -> ValueSource {
    match callee {
        PCallee::ObjectRun {
            object_kind,
            target_ref,
            target_is_name,
            ..
        } => match target_ref {
            None => ValueSource::Expression,
            Some(target_ref) => {
                if *target_is_name {
                    ValueSource::Enum {
                        enum_name: object_kind.clone(),
                        member: Some(target_ref.clone()),
                    }
                } else {
                    ValueSource::Literal {
                        value: target_ref.clone(),
                    }
                }
            }
        },
        _ => ValueSource::Unknown,
    }
}

/// Derive the target ObjectId for an object-run callee when the targetRef is
/// statically known but the routine entry wasn't resolved (al-sem
/// `objectRunTargetObject`). Returns None when fully dynamic.
fn object_run_target_object(
    callee: &PCallee,
    objects_by_type_number: &HashMap<String, String>,
    objects_by_type_name: &HashMap<String, String>,
) -> Option<String> {
    let PCallee::ObjectRun {
        target_type,
        target_ref,
        target_is_name,
        ..
    } = callee
    else {
        return None;
    };
    let target_ref = target_ref.as_ref()?;
    if *target_is_name {
        objects_by_type_name
            .get(&format!(
                "{}/{}",
                target_type.to_lowercase(),
                target_ref.to_lowercase()
            ))
            .cloned()
    } else {
        objects_by_type_number
            .get(&format!("{}/{}", target_type.to_lowercase(), target_ref))
            .cloned()
    }
}

fn build_typed_edges(
    workspace: &ModelEntities,
    call_edges: &[CallEdge],
    event_graph: &EventGraph,
) -> Vec<TypedEdge> {
    // One pass over the routines:
    // - callsite id → &PCallSite (for callee details);
    // - routine id → objectId (for resolved object-run target);
    // - routine id → name (an `implicit-trigger` edge's `trigger_kind`).
    let mut call_site_by_id: HashMap<&str, &crate::program::body::features::PCallSite> =
        HashMap::new();
    let mut object_id_by_routine: HashMap<&str, &str> = HashMap::new();
    let mut routine_name_by_id: HashMap<&str, &str> = HashMap::new();
    for routine in &workspace.routines {
        for cs in &routine.call_sites {
            call_site_by_id.insert(cs.id.as_str(), cs);
        }
        object_id_by_routine.insert(routine.id.as_str(), routine.object_id.as_str());
        routine_name_by_id.insert(routine.id.as_str(), routine.name.as_str());
    }
    // object type+number / type+name → objectId (for unresolved object-run target).
    let mut objects_by_type_number: HashMap<String, String> = HashMap::new();
    let mut objects_by_type_name: HashMap<String, String> = HashMap::new();
    for obj in &workspace.objects {
        objects_by_type_number.insert(
            format!("{}/{}", obj.object_type.to_lowercase(), obj.object_number),
            obj.id.clone(),
        );
        objects_by_type_name.insert(
            format!(
                "{}/{}",
                obj.object_type.to_lowercase(),
                obj.name.to_lowercase()
            ),
            obj.id.clone(),
        );
    }

    // Group interface "maybe" resolved edges by callsiteId (for candidateCount +
    // interfaceName), mirroring al-sem `interfaceEdgesByCallsite`. Preserve order.
    let mut interface_order: Vec<String> = Vec::new();
    let mut interface_edges_by_callsite: HashMap<String, Vec<&CallEdge>> = HashMap::new();
    for ce in call_edges {
        if ce.dispatch_kind == DispatchKind::Interface
            && ce.resolution == Resolution::Maybe
            && ce.to.is_some()
        {
            if !interface_edges_by_callsite.contains_key(&ce.callsite_id) {
                interface_order.push(ce.callsite_id.clone());
            }
            interface_edges_by_callsite
                .entry(ce.callsite_id.clone())
                .or_default()
                .push(ce);
        }
    }

    let mut interface_typed_emitted: HashSet<String> = HashSet::new();
    let mut typed_edges: Vec<TypedEdge> = Vec::new();

    for ce in call_edges {
        // An implicit-trigger edge (#96) is keyed by its RECORD OPERATION
        // (`callsite_id` = `operation_id` = `"{routine}/opN"`), never a
        // `PCallSite` id, so it is built before the callsite lookup below, which
        // would drop it. `callsite_id` stays `None`: the op id is not a call site
        // (the snapshot's callsite-resolution pass reads `callsite_id`).
        if ce.dispatch_kind == DispatchKind::ImplicitTrigger {
            let Some(to) = &ce.to else { continue };
            typed_edges.push(TypedEdge {
                kind: "implicit-trigger".to_string(),
                from: ce.from.clone(),
                to: Some(to.clone()),
                callsite_id: None,
                operation_id: Some(ce.operation_id.clone()),
                event_id: None,
                receiver_type: None,
                interface_name: None,
                candidate_count: None,
                target_object: None,
                object_type: None,
                target_id_source: None,
                trigger_kind: routine_name_by_id
                    .get(to.as_str())
                    .map(|n| (*n).to_string()),
                // The adapter decides `fires` once, from the op (#96 D1). An
                // edge without it reads as `Maybe`, the conservative answer.
                fires: Some(ce.fires.unwrap_or(TriggerFires::Maybe)),
            });
            continue;
        }
        let Some(call_site) = call_site_by_id.get(ce.callsite_id.as_str()) else {
            continue; // opaque (dependency-only) callsite — skip
        };

        if let Some(to) = &ce.to {
            // Resolved edges
            if ce.dispatch_kind == DispatchKind::Direct {
                typed_edges.push(TypedEdge {
                    kind: "direct-call".to_string(),
                    from: ce.from.clone(),
                    to: Some(to.clone()),
                    callsite_id: Some(ce.callsite_id.clone()),
                    operation_id: None,
                    event_id: None,
                    receiver_type: None,
                    interface_name: None,
                    candidate_count: None,
                    target_object: None,
                    object_type: None,
                    target_id_source: None,
                    trigger_kind: None,
                    fires: None,
                });
            } else if ce.dispatch_kind == DispatchKind::Method
                && ce.resolution == Resolution::Resolved
            {
                typed_edges.push(TypedEdge {
                    kind: "variable-typed-call".to_string(),
                    from: ce.from.clone(),
                    to: Some(to.clone()),
                    callsite_id: Some(ce.callsite_id.clone()),
                    operation_id: None,
                    event_id: None,
                    receiver_type: Some(ce.receiver_type.clone().unwrap_or_default()),
                    interface_name: None,
                    candidate_count: None,
                    target_object: None,
                    object_type: None,
                    target_id_source: None,
                    trigger_kind: None,
                    fires: None,
                });
            } else if ce.dispatch_kind == DispatchKind::Interface
                && ce.resolution == Resolution::Maybe
            {
                // Interface dispatch: emit one edge per resolved candidate, once per
                // callsite (when first encountered), then skip.
                if interface_typed_emitted.contains(&ce.callsite_id) {
                    continue;
                }
                interface_typed_emitted.insert(ce.callsite_id.clone());
                let empty: Vec<&CallEdge> = Vec::new();
                let candidates = interface_edges_by_callsite
                    .get(&ce.callsite_id)
                    .unwrap_or(&empty);
                let candidate_count = candidates.len();
                let interface_name = candidates
                    .first()
                    .and_then(|c| c.dispatch_meta.as_ref())
                    .map(|dm| dm.interface_name.clone())
                    .unwrap_or_default();
                for candidate in candidates {
                    let Some(cand_to) = &candidate.to else {
                        continue;
                    };
                    if !call_site_by_id.contains_key(candidate.callsite_id.as_str()) {
                        continue;
                    }
                    typed_edges.push(TypedEdge {
                        kind: "interface-dispatch".to_string(),
                        from: candidate.from.clone(),
                        to: Some(cand_to.clone()),
                        callsite_id: Some(candidate.callsite_id.clone()),
                        operation_id: None,
                        event_id: None,
                        receiver_type: None,
                        interface_name: Some(interface_name.clone()),
                        candidate_count: Some(candidate_count),
                        target_object: None,
                        object_type: None,
                        target_id_source: None,
                        trigger_kind: None,
                        fires: None,
                    });
                }
            } else if is_object_run_kind(ce.dispatch_kind.as_str()) {
                let Some(object_type) = dispatch_kind_to_object_type(ce.dispatch_kind.as_str())
                else {
                    continue;
                };
                let Some(target_object) = object_id_by_routine.get(to.as_str()) else {
                    continue; // should not happen in a sound model
                };
                typed_edges.push(TypedEdge {
                    kind: "object-run-resolved".to_string(),
                    from: ce.from.clone(),
                    to: Some(to.clone()),
                    callsite_id: Some(ce.callsite_id.clone()),
                    operation_id: None,
                    event_id: None,
                    receiver_type: None,
                    interface_name: None,
                    candidate_count: None,
                    target_object: Some((*target_object).to_string()),
                    object_type: Some(object_type.to_string()),
                    target_id_source: None,
                    trigger_kind: None,
                    fires: None,
                });
            }
            // method unresolved / interface non-maybe etc. — skip
        } else {
            // Unresolved edges
            if is_object_run_kind(ce.dispatch_kind.as_str()) {
                let Some(object_type) = dispatch_kind_to_object_type(ce.dispatch_kind.as_str())
                else {
                    continue;
                };
                let target_id_source = object_run_target_id_source(&call_site.callee);
                let target_object = object_run_target_object(
                    &call_site.callee,
                    &objects_by_type_number,
                    &objects_by_type_name,
                );
                typed_edges.push(TypedEdge {
                    kind: "object-run-unresolved".to_string(),
                    from: ce.from.clone(),
                    to: None,
                    callsite_id: Some(ce.callsite_id.clone()),
                    operation_id: None,
                    event_id: None,
                    receiver_type: None,
                    interface_name: None,
                    candidate_count: None,
                    target_object,
                    object_type: Some(object_type.to_string()),
                    target_id_source: Some(target_id_source),
                    trigger_kind: None,
                    fires: None,
                });
            } else if ce.dispatch_kind == DispatchKind::Dynamic {
                // Dynamic dispatch — objectType from the callee when it's object-run.
                if let PCallee::ObjectRun { object_kind, .. } = &call_site.callee {
                    let target_id_source = object_run_target_id_source(&call_site.callee);
                    typed_edges.push(TypedEdge {
                        kind: "object-run-unresolved".to_string(),
                        from: ce.from.clone(),
                        to: None,
                        callsite_id: Some(ce.callsite_id.clone()),
                        operation_id: None,
                        event_id: None,
                        receiver_type: None,
                        interface_name: None,
                        candidate_count: None,
                        target_object: None,
                        object_type: Some(object_kind.clone()),
                        target_id_source: Some(target_id_source),
                        trigger_kind: None,
                        fires: None,
                    });
                }
                // dynamic member call (method dispatch) — not object-run; skip
            }
            // interface / unresolved bare call — skip
        }
    }

    // --- event-dispatch typed edges: bipartite publisher → subscriber ---
    let mut subs_by_event: HashMap<String, Vec<&crate::program::model::events::EventEdge>> =
        HashMap::new();
    for ee in &event_graph.edges {
        subs_by_event
            .entry(ee.event_id.clone())
            .or_default()
            .push(ee);
    }
    let routine_ids: HashSet<&str> = workspace.routines.iter().map(|r| r.id.as_str()).collect();
    for sym in &event_graph.events {
        let Some(publisher_routine_id) = &sym.publisher_routine_id else {
            continue;
        };
        if !routine_ids.contains(publisher_routine_id.as_str()) {
            continue;
        }
        if let Some(list) = subs_by_event.get(&sym.id) {
            for ee in list {
                if !routine_ids.contains(ee.subscriber_routine_id.as_str()) {
                    continue;
                }
                typed_edges.push(TypedEdge {
                    kind: "event-dispatch".to_string(),
                    from: publisher_routine_id.clone(),
                    to: Some(ee.subscriber_routine_id.clone()),
                    callsite_id: None,
                    operation_id: None,
                    event_id: Some(sym.id.clone()),
                    receiver_type: None,
                    interface_name: None,
                    candidate_count: None,
                    target_object: None,
                    object_type: None,
                    target_id_source: None,
                    trigger_kind: None,
                    fires: None,
                });
            }
        }
    }

    typed_edges
}

/// Spec §5.1: the relation equals today's cross-app base recipe field by
/// field. The oracle ([`tests::legacy_relation`]) runs over the UNTOUCHED
/// `build_combined_graph`; at this commit the relation runs over the copied
/// [`assemble`], so the two are independent code.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::l4::combined_graph::build_combined_graph;
    use crate::program::model::calls::ResolvedCalls;
    use crate::program::model::program_calls::{
        assemble_and_resolve_cross_app_program, assemble_and_resolve_workspace_program,
    };
    use crate::program::model::workspace::{MODEL_INSTANCE_ID_DEFAULT, Model};
    use std::collections::BTreeSet;
    use std::path::{Path, PathBuf};

    /// Today's cross-app base recipe (`capability_cone.rs`
    /// `build_cross_app_base`: the cut, the primary graph, its SCC, the
    /// dependency half-build appended to the typed edges), returning the
    /// dependency half's uncertainty edges it drops. With `dep_ids` empty it is
    /// the single-app context graph (`build_combined_graph` over every call)
    /// plus an empty half.
    fn legacy_relation(
        ws: &ModelEntities,
        all_calls: &ResolvedCalls,
        events: &EventGraph,
        dep_ids: &BTreeSet<String>,
    ) -> (CombinedGraph, SccResult, Vec<UncertaintyEdge>) {
        let calls = ResolvedCalls {
            edges: all_calls
                .edges
                .iter()
                .filter(|e| !dep_ids.contains(&e.from))
                .cloned()
                .collect(),
            upgraded_bindings: all_calls.upgraded_bindings.clone(),
            diagnostics: all_calls.diagnostics.clone(),
            external_targets: all_calls.external_targets.clone(),
        };
        let mut graph = build_combined_graph(ws, &calls, events);
        let scc = crate::engine::l4::analysis::graph_scc(&graph);
        let dep_calls = ResolvedCalls {
            edges: all_calls
                .edges
                .iter()
                .filter(|e| dep_ids.contains(&e.from))
                .cloned()
                .collect(),
            upgraded_bindings: all_calls.upgraded_bindings.clone(),
            diagnostics: Vec::new(),
            external_targets: all_calls.external_targets.clone(),
        };
        let no_events = EventGraph {
            events: Vec::new(),
            edges: Vec::new(),
        };
        let half = build_combined_graph(ws, &dep_calls, &no_events);
        graph.typed_edges.extend(half.typed_edges);
        (graph, scc, half.uncertainty_edges)
    }

    /// `field: …` naming the first index where `new` and `legacy` differ.
    fn diff_vec<T: PartialEq + std::fmt::Debug>(
        field: &str,
        new: &[T],
        legacy: &[T],
    ) -> Option<String> {
        if new == legacy {
            return None;
        }
        let i = new
            .iter()
            .zip(legacy)
            .position(|(a, b)| a != b)
            .unwrap_or(new.len().min(legacy.len()));
        Some(format!(
            "{field}: len new {} legacy {}; first difference at {i}: new {:?} legacy {:?}",
            new.len(),
            legacy.len(),
            new.get(i),
            legacy.get(i)
        ))
    }

    #[derive(Default)]
    struct Stats {
        cases: usize,
        dep_half_with_dispatch: usize,
        dep_uncertainty: usize,
    }

    /// One case: the relation over the split of `calls` by `dep_ids` against
    /// [`legacy_relation`], each field compared separately.
    fn check_case(
        where_: &str,
        ws: &ModelEntities,
        calls: &ResolvedCalls,
        events: &EventGraph,
        dep_ids: &BTreeSet<String>,
        stats: &mut Stats,
        failures: &mut Vec<String>,
    ) {
        let (lg, lscc, lunc) = legacy_relation(ws, calls, events, dep_ids);
        let (dependency, primary): (Vec<CallEdge>, Vec<CallEdge>) = calls
            .edges
            .iter()
            .cloned()
            .partition(|e| dep_ids.contains(&e.from));
        stats.cases += 1;
        if !dependency.is_empty() && !events.edges.is_empty() {
            stats.dep_half_with_dispatch += 1;
        }
        let rel = ExecRelation::new(ws, &primary, &dependency, events);
        if !rel.dependency_uncertainty.is_empty() {
            stats.dep_uncertainty += 1;
        }
        let g = &*rel.graph;
        let mut diffs: Vec<String> = Vec::new();
        diffs.extend(diff_vec("nodes", &g.nodes, &lg.nodes));
        let keys: BTreeSet<&String> = g
            .edges_by_from
            .keys()
            .chain(lg.edges_by_from.keys())
            .collect();
        for k in keys {
            let empty = Vec::new();
            if let Some(d) = diff_vec(
                &format!("edges_by_from[{k}]"),
                g.edges_by_from.get(k).unwrap_or(&empty),
                lg.edges_by_from.get(k).unwrap_or(&empty),
            ) {
                diffs.push(d);
                break;
            }
        }
        if g.edges_by_from.len() != lg.edges_by_from.len() {
            diffs.push(format!(
                "edges_by_from: {} keys new, {} legacy",
                g.edges_by_from.len(),
                lg.edges_by_from.len()
            ));
        }
        diffs.extend(diff_vec(
            "edges_from_order",
            &g.edges_from_order,
            &lg.edges_from_order,
        ));
        diffs.extend(diff_vec(
            "uncertainty_edges",
            &g.uncertainty_edges,
            &lg.uncertainty_edges,
        ));
        diffs.extend(diff_vec("typed_edges", &g.typed_edges, &lg.typed_edges));
        if *rel.scc != lscc {
            diffs.push(format!(
                "scc: {} sccs new, {} legacy",
                rel.scc.sccs.len(),
                lscc.sccs.len()
            ));
        }
        diffs.extend(diff_vec(
            "dependency_uncertainty",
            &rel.dependency_uncertainty,
            &lunc,
        ));
        failures.extend(diffs.into_iter().map(|d| format!("{where_}: {d}")));
    }

    fn workspace_model(dir: &Path) -> Option<Model> {
        assemble_and_resolve_workspace_program(
            &crate::program::inputs::InputSet::read(dir, &Default::default()),
            MODEL_INSTANCE_ID_DEFAULT,
            false,
        )
    }

    /// Both arms on one workspace: single-app with no cut (the single-app
    /// path never cuts, even when the model carries dependency rows), and
    /// cross-app with the base's split (routines whose `app_guid` differs from
    /// the primary app's). A dir that builds no model fails.
    fn check_dir(dir: &Path, where_: &str, stats: &mut Stats, failures: &mut Vec<String>) {
        match workspace_model(dir) {
            Some(m) => check_case(
                where_,
                &m.workspace,
                &m.calls,
                &m.events.graph,
                &BTreeSet::new(),
                stats,
                failures,
            ),
            None => failures.push(format!("{where_}: no single-app model built")),
        }
        let inputs = crate::program::inputs::InputSet::read(dir, &Default::default());
        let Ok(x) =
            assemble_and_resolve_cross_app_program(&inputs, MODEL_INSTANCE_ID_DEFAULT, false)
        else {
            failures.push(format!("{where_}: no cross-app model built"));
            return;
        };
        let Some(primary) = x.resolved.primary_app.as_ref() else {
            failures.push(format!("{where_}: cross-app model has no primary app"));
            return;
        };
        let primary = primary.app_guid.to_ascii_lowercase();
        let ws = &x.resolved.workspace;
        let dep_ids: BTreeSet<String> = ws
            .routines
            .iter()
            .filter(|r| r.app_guid.to_ascii_lowercase() != primary)
            .map(|r| r.id.clone())
            .collect();
        check_case(
            &format!("{where_} (cross-app)"),
            ws,
            &x.resolved.calls,
            &x.resolved.events.graph,
            &dep_ids,
            stats,
            failures,
        );
    }

    /// Sorted dirs under `root`, at any depth, holding an `app.json`.
    fn app_dirs(root: &Path) -> Vec<PathBuf> {
        let mut dirs: Vec<PathBuf> = walkdir::WalkDir::new(root)
            .into_iter()
            .map(|e| e.unwrap().into_path())
            .filter(|p| p.join("app.json").is_file())
            .collect();
        dirs.sort();
        dirs
    }

    /// Spec §5.1 on every app dir under step 3's five roots plus the §5.2 and
    /// §5.4 inline fixtures, both arms each.
    #[test]
    fn relation_equals_the_legacy_base_per_field() {
        let tests = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
        let mut stats = Stats::default();
        let mut failures = Vec::new();
        // The count each root holds today: a dir that stops being visited fails.
        for (root, want) in [
            ("r0-corpus", 217),
            ("fixtures", 29),
            ("r3a4-fixtures", 1),
            ("r3a5-fixtures", 1),
            ("r2-5b-fixtures", 1),
        ] {
            let dirs = app_dirs(&tests.join(root));
            assert_eq!(dirs.len(), want, "{root}: app dirs");
            for dir in &dirs {
                check_dir(dir, &dir.display().to_string(), &mut stats, &mut failures);
            }
        }
        let tmp = tempfile::tempdir().unwrap();
        let inline = tmp.path().join("ws");
        crate::engine::l5::test_support::write_fixture_step4(&inline);
        let before = stats.dep_half_with_dispatch;
        check_dir(&inline, "step4 inline", &mut stats, &mut failures);
        assert!(
            stats.dep_half_with_dispatch > before,
            "precondition: the step4 inline fixture has a dependency half and event dispatch"
        );
        // The §5.4 witness too: no corpus dir has dependency uncertainty, so
        // without it a break that mixes the halves' uncertainty would pass.
        let inline_unc = tmp.path().join("ws-unc");
        crate::engine::l5::test_support::write_fixture_dep_uncertainty(&inline_unc);
        let before = stats.dep_uncertainty;
        check_dir(
            &inline_unc,
            "step4 dependency uncertainty",
            &mut stats,
            &mut failures,
        );
        assert!(
            stats.dep_uncertainty > before,
            "precondition: the dependency-uncertainty fixture keeps dependency uncertainty"
        );
        println!(
            "exec_relation parity: {} cases, {} with a dependency half and event dispatch, {} with dependency uncertainty",
            stats.cases, stats.dep_half_with_dispatch, stats.dep_uncertainty
        );
        assert_eq!(stats.cases, 2 * (217 + 29 + 3 + 2), "cases run");
        assert!(failures.is_empty(), "\n{}", failures.join("\n"));
    }

    /// Spec §5.1 (S2-1): the dir the `isolated` discrimination runs on keeps
    /// an isolated publisher WITH a subscriber. Fails, never skips.
    #[test]
    fn isolated_dispatch_precondition() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/r0-corpus/ws-txn-d47-event-neg-isolated");
        let m = workspace_model(&dir).expect("a single-app model");
        let g = &m.events.graph;
        let isolated_with_subscriber = g
            .events
            .iter()
            .filter(|e| e.isolated == Some(true))
            .any(|e| g.edges.iter().any(|ee| ee.event_id == e.id));
        assert!(
            isolated_with_subscriber,
            "no isolated event with a subscriber in {}",
            dir.display()
        );
    }

    /// Spec §5.1, the CDO half (skips without `CDO_WS`, panics under
    /// `ENFORCE_CDO_WS=1`): both models of the pinned CDO workspace.
    #[test]
    fn relation_equals_the_legacy_base_on_cdo() {
        let Some(ws) = crate::cdo::cdo_ws_or_enforce() else {
            return;
        };
        let start = std::time::Instant::now();
        let mut stats = Stats::default();
        let mut failures = Vec::new();
        check_dir(&ws, "CDO", &mut stats, &mut failures);
        println!(
            "exec_relation parity on CDO: {} cases, {} with a dependency half and event dispatch, {} with dependency uncertainty, {:.1?}",
            stats.cases,
            stats.dep_half_with_dispatch,
            stats.dep_uncertainty,
            start.elapsed()
        );
        assert_eq!(stats.cases, 2, "both CDO models ran");
        assert!(failures.is_empty(), "\n{}", failures.join("\n"));
    }
}

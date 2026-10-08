//! L4 COMBINED GRAPH (R3a-1 Task 2) — faithful port of al-sem's
//! `buildCombinedGraph` (`src/engine/combined-graph.ts`) + the stable R3a-1
//! projection (`scripts/r3a1-projection.ts` `projectR3a1`).
//!
//! Two layers:
//!   1. `build_combined_graph` — the INTERNAL combined graph: resolved call edges
//!      → `CombinedEdge`s (kinds direct/method/codeunit-run/report-run/page-run/
//!      interface/implicit-trigger/dynamic), the bipartite event-dispatch edges
//!      (publisher→subscriber from the event graph), the to-less `UncertaintyEdge`s,
//!      and the typed `GraphEdge[]` (`typed_edges`). All ids INTERNAL.
//!   2. `project_r3a1` — projects the combined graph + the `tarjan_scc` result to
//!      the STABLE id form the R3a-1 vectors carry. SourceAnchors are DROPPED from
//!      typed edges (redundant by-reference copies already gated at R1a).
//!
//! The combined graph is built FROM the model's calls (incl. implicit-trigger
//! edges) + its event graph, both the program engine's. al-sem's
//! `model.callGraph` is the flat resolver edge list (event-dispatch dispatchKind
//! entries skipped here); the model's calls hold no event-dispatch CallEdges, so
//! event hops come SOLELY from the event graph — no double counting.

use std::collections::{HashMap, HashSet};

use super::scc::{Scc, SccInputGraph, SccResult, tarjan_scc};
use crate::engine::ids::to_stable_object_id;
use crate::engine::l2::features::PCallee;
use crate::engine::l3::call_resolver::{CallEdge, ResolvedCalls};
use crate::engine::l3::event_graph::{EventGraph, EventSymbol};
use crate::engine::l3::l3_workspace::{L3Resolved, L3Routine, L3Workspace};
use crate::engine::l3::taxonomy::{DispatchKind, Resolution};

// ---------------------------------------------------------------------------
// Internal combined-graph model (NOT the serde projection shape). Ids INTERNAL.
// ---------------------------------------------------------------------------

/// A resolved routine → routine combined edge (internal-id form).
#[derive(Debug, Clone, PartialEq, Eq, salsa::Update)]
pub struct CombinedEdge {
    pub from: String,
    pub to: String,
    /// direct | method | codeunit-run | report-run | page-run | interface |
    /// implicit-trigger | event-dispatch | dynamic
    pub kind: String,
    pub callsite_id: Option<String>,
    pub operation_id: Option<String>,
    pub event_id: Option<String>,
    pub subscriber_app_id: Option<String>,
    pub resolution: String,
}

/// An uncertainty attached to a routine whose call site had no resolved target.
#[derive(Debug, Clone, PartialEq, Eq, salsa::Update)]
pub struct UncertaintyEdge {
    pub from: String,
    pub uncertainty: Uncertainty,
}

/// The discriminated Uncertainty — kind + its single id reference (+ interfaceName
/// for interface-open-world). Only the R3a-1-reachable variants are modelled.
#[derive(Debug, Clone, PartialEq, Eq, salsa::Update)]
pub struct Uncertainty {
    pub kind: String,
    pub callsite_id: Option<String>,
    pub operation_id: Option<String>,
    pub routine_id: Option<String>,
    pub interface_name: Option<String>,
}

/// A typed `GraphEdge` (internal-id form). One flat struct over the discriminated
/// union — only the fields legal for `kind` are populated (mirrors the TS union).
#[derive(Debug, Clone, PartialEq, Eq, salsa::Update)]
pub struct TypedEdge {
    pub kind: String,
    pub from: String,
    pub to: Option<String>,
    pub callsite_id: Option<String>,
    pub operation_id: Option<String>,
    pub event_id: Option<String>,
    pub receiver_type: Option<String>,
    pub interface_name: Option<String>,
    pub candidate_count: Option<usize>,
    pub target_object: Option<String>,
    pub object_type: Option<String>,
    pub target_id_source: Option<ValueSource>,
}

/// A ValueSource (internal form — table-field tableId is INTERNAL until projected).
#[derive(Debug, Clone, PartialEq, Eq, salsa::Update)]
pub enum ValueSource {
    Literal {
        value: String,
    },
    Enum {
        enum_name: String,
        member: Option<String>,
    },
    TableField {
        table_id: String,
        field_name: String,
    },
    Expression,
    Unknown,
}

/// The internal combined graph: the assembled edges + uncertainty edges + the
/// typed edges + the sorted node list. `edges` flattened (already per-`from`
/// sorted via `edges_by_from` at assembly).
#[derive(Clone, PartialEq, Eq, salsa::Update)]
pub struct CombinedGraph {
    /// Sorted internal routine ids.
    pub nodes: Vec<String>,
    /// from-id → its edgeSortKey-sorted edge list.
    pub edges_by_from: HashMap<String, Vec<CombinedEdge>>,
    /// First-appearance order of `from` keys in the `edges` array (call-graph
    /// emission order, then event-dispatch). Mirrors al-sem JS `Map` insertion
    /// order so that d17's sample-selection iterates `edgesByFrom` in the SAME
    /// order: `for (const [, edges] of graph.edgesByFrom)`. Needed when ≥2
    /// primary callers reach the same dep — the first one in emission order wins.
    pub edges_from_order: Vec<String>,
    /// Sorted (uncertaintySortKey) uncertainty edges.
    pub uncertainty_edges: Vec<UncertaintyEdge>,
    /// Typed edges in emission order.
    pub typed_edges: Vec<TypedEdge>,
}

// ---------------------------------------------------------------------------
// Sort keys (al-sem `edgeSortKey` / `uncertaintySortKey`). Byte-order compare.
// ---------------------------------------------------------------------------

/// `edgeSortKey(e)` = `${kind}|${callsiteId ?? operationId ?? eventId ?? ""}|${to}`.
pub fn edge_sort_key(e: &CombinedEdge) -> String {
    let mid = e
        .callsite_id
        .clone()
        .or_else(|| e.operation_id.clone())
        .or_else(|| e.event_id.clone())
        .unwrap_or_default();
    format!("{}|{}|{}", e.kind, mid, e.to)
}

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

// ---------------------------------------------------------------------------
// Edge-kind / object-run helpers (al-sem `EDGE_KINDS` / `OBJECT_RUN_KINDS` etc.).
// ---------------------------------------------------------------------------

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

/// CallGraph dispatchKinds that become resolved routine→routine combined edges
/// (when `to` is set). Mirrors al-sem `EDGE_KINDS`.
// Currently unreferenced (al-sem is retired); kept as edge-classification domain
// logic for the L4 combined-graph design. Remove if a redesign supersedes it.
#[allow(dead_code)]
fn is_edge_kind(kind: &str) -> bool {
    matches!(
        kind,
        "direct"
            | "method"
            | "codeunit-run"
            | "report-run"
            | "page-run"
            | "interface"
            | "implicit-trigger"
            | "dynamic"
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

// ---------------------------------------------------------------------------
// build_combined_graph — the CombinedEdge / UncertaintyEdge / typed-edge build.
// ---------------------------------------------------------------------------

/// Build the combined graph over a resolved workspace + its call graph + event
/// graph. al-sem's `buildCombinedGraph` reads `model.callGraph` (the flat edge
/// list) + `model.eventGraph`; we pass the equivalent `resolved.edges` +
/// `event_graph` + `routines` (for the node list).
pub fn build_combined_graph(
    workspace: &L3Workspace,
    resolved: &ResolvedCalls,
    event_graph: &EventGraph,
) -> CombinedGraph {
    let mut edges: Vec<CombinedEdge> = Vec::new();
    let mut uncertainty_edges: Vec<UncertaintyEdge> = Vec::new();

    // Interface callsites that already received an "interface-open-world" uncertainty
    // (emit exactly once per callsite even with multiple "maybe" edges).
    let mut interface_open_world_emitted: HashSet<String> = HashSet::new();

    // --- call-derived edges + uncertainty records ---
    for ce in &resolved.edges {
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
    let mut subs_by_event: HashMap<String, Vec<&crate::engine::l3::event_graph::EventEdge>> =
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
                edges.push(CombinedEdge {
                    from: publisher_routine_id.clone(),
                    to: ee.subscriber_routine_id.clone(),
                    kind: "event-dispatch".to_string(),
                    callsite_id: None,
                    operation_id: None,
                    event_id: Some(sym.id.clone()),
                    subscriber_app_id: Some(ee.subscriber_app_id.clone()),
                    resolution: ee.resolution.clone(),
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

    let typed_edges = build_typed_edges(workspace, resolved, event_graph);

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

// ---------------------------------------------------------------------------
// build_typed_edges — the Phase 0b-β typed `GraphEdge[]`. Faithful port of
// al-sem's `buildTypedEdges`. SourceAnchors are NOT carried (dropped at R3a-1).
// ---------------------------------------------------------------------------

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
    workspace: &L3Workspace,
    resolved: &ResolvedCalls,
    event_graph: &EventGraph,
) -> Vec<TypedEdge> {
    // callsite id → &PCallSite (for callee details).
    let mut call_site_by_id: HashMap<&str, &crate::engine::l2::features::PCallSite> =
        HashMap::new();
    for routine in &workspace.routines {
        for cs in &routine.call_sites {
            call_site_by_id.insert(cs.id.as_str(), cs);
        }
    }
    // routine id → objectId (for resolved object-run target).
    let mut object_id_by_routine: HashMap<&str, &str> = HashMap::new();
    for routine in &workspace.routines {
        object_id_by_routine.insert(routine.id.as_str(), routine.object_id.as_str());
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
    for ce in &resolved.edges {
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

    for ce in &resolved.edges {
        if ce.dispatch_kind == DispatchKind::ImplicitTrigger {
            continue; // implicit-trigger edges not typed; event-dispatch never in the calls
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
                    });
                }
                // dynamic member call (method dispatch) — not object-run; skip
            }
            // interface / unresolved bare call — skip
        }
    }

    // --- event-dispatch typed edges: bipartite publisher → subscriber ---
    let mut subs_by_event: HashMap<String, Vec<&crate::engine::l3::event_graph::EventEdge>> =
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
                });
            }
        }
    }

    typed_edges
}

// ===========================================================================
// R3a-1 STABLE PROJECTION — mirrors scripts/r3a1-projection.ts `projectR3a1`.
// ===========================================================================

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct PCombinedEdge {
    pub from: String,
    pub to: String,
    pub kind: String,
    #[serde(rename = "callsiteId", skip_serializing_if = "Option::is_none")]
    pub callsite_id: Option<String>,
    #[serde(rename = "operationId", skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<String>,
    #[serde(rename = "eventId", skip_serializing_if = "Option::is_none")]
    pub event_id: Option<String>,
    #[serde(rename = "subscriberAppId", skip_serializing_if = "Option::is_none")]
    pub subscriber_app_id: Option<String>,
    pub resolution: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct PUncertainty {
    pub kind: String,
    #[serde(rename = "callsiteId", skip_serializing_if = "Option::is_none")]
    pub callsite_id: Option<String>,
    #[serde(rename = "operationId", skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<String>,
    #[serde(rename = "routineId", skip_serializing_if = "Option::is_none")]
    pub routine_id: Option<String>,
    #[serde(rename = "interfaceName", skip_serializing_if = "Option::is_none")]
    pub interface_name: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct PUncertaintyEdge {
    pub from: String,
    pub uncertainty: PUncertainty,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(tag = "kind")]
pub enum PValueSource {
    #[serde(rename = "literal")]
    Literal { value: String },
    #[serde(rename = "enum")]
    Enum {
        #[serde(rename = "enumName")]
        enum_name: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        member: Option<String>,
    },
    #[serde(rename = "table-field")]
    TableField {
        #[serde(rename = "tableId")]
        table_id: String,
        #[serde(rename = "fieldName")]
        field_name: String,
    },
    #[serde(rename = "expression")]
    Expression,
    #[serde(rename = "unknown")]
    Unknown,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct PTypedEdge {
    pub kind: String,
    pub from: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
    #[serde(rename = "callsiteId", skip_serializing_if = "Option::is_none")]
    pub callsite_id: Option<String>,
    #[serde(rename = "operationId", skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<String>,
    #[serde(rename = "eventId", skip_serializing_if = "Option::is_none")]
    pub event_id: Option<String>,
    #[serde(rename = "receiverType", skip_serializing_if = "Option::is_none")]
    pub receiver_type: Option<String>,
    #[serde(rename = "interfaceName", skip_serializing_if = "Option::is_none")]
    pub interface_name: Option<String>,
    #[serde(rename = "candidateCount", skip_serializing_if = "Option::is_none")]
    pub candidate_count: Option<usize>,
    #[serde(rename = "targetObject", skip_serializing_if = "Option::is_none")]
    pub target_object: Option<String>,
    #[serde(rename = "objectType", skip_serializing_if = "Option::is_none")]
    pub object_type: Option<String>,
    #[serde(rename = "targetIdSource", skip_serializing_if = "Option::is_none")]
    pub target_id_source: Option<PValueSource>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct PScc {
    pub members: Vec<String>,
    pub recursive: bool,
}

/// The full R3a-1 projection — the vector / golden document shape.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct R3a1Projection {
    #[serde(rename = "combinedEdges")]
    pub combined_edges: Vec<PCombinedEdge>,
    #[serde(rename = "uncertaintyEdges")]
    pub uncertainty_edges: Vec<PUncertaintyEdge>,
    #[serde(rename = "typedEdges")]
    pub typed_edges: Vec<PTypedEdge>,
    pub sccs: Vec<PScc>,
}

/// Byte-order string compare (al-sem `cmpStable`).
fn cmp_stable(a: &str, b: &str) -> std::cmp::Ordering {
    a.cmp(b)
}

/// Internal RoutineId → StableRoutineId; pass through if unmapped (so a divergence
/// is VISIBLE rather than silently dropped).
fn stable_routine_id(internal: &str, map: &HashMap<String, String>) -> String {
    map.get(internal)
        .cloned()
        .unwrap_or_else(|| internal.to_string())
}

/// Rewrite `${routineId}/<suffix>` (callsiteId `/csN`, operationId `/opN`) → stable
/// form. Internal RoutineId is exactly two `/`-parts, so the suffix is everything
/// after the SECOND `/` (== after the LAST `/`). Mirrors `stableSubId`.
fn stable_sub_id(internal_sub_id: &str, map: &HashMap<String, String>) -> String {
    match internal_sub_id.rsplit_once('/') {
        Some((prefix, suffix)) => match map.get(prefix) {
            Some(stable) => format!("{stable}/{suffix}"),
            None => internal_sub_id.to_string(),
        },
        None => internal_sub_id.to_string(),
    }
}

/// Project an internal EventId to StableEventId via the EventSymbol (eventName +
/// signatureHash). Mirrors `stableEventId` + `stable_event_id_from_symbol`: DUMB
/// `/`→`:` on the publisherObjectId, never parse the raw eventId.
fn stable_event_id(internal_event_id: &str, event_by_id: &HashMap<String, &EventSymbol>) -> String {
    match event_by_id.get(internal_event_id) {
        Some(evt) => format!(
            "{}::{}::{}",
            to_stable_object_id(&evt.publisher_object_id),
            evt.event_name,
            evt.signature_hash
        ),
        None => internal_event_id.to_string(),
    }
}

/// Internal TableId (`appGuid/table/N`) → StableTableId (`appGuid:Table:N`). Mirrors
/// al-sem `toStableTableId`. `"unknown"` passes through.
fn stable_table_id(internal: &str) -> String {
    if internal == "unknown" {
        return "unknown".to_string();
    }
    let parts: Vec<&str> = internal.split('/').collect();
    if parts.len() == 3 && parts[1] == "table" {
        format!("{}:Table:{}", parts[0], parts[2])
    } else {
        internal.to_string()
    }
}

fn project_value_source(vs: &ValueSource) -> PValueSource {
    match vs {
        ValueSource::Literal { value } => PValueSource::Literal {
            value: value.clone(),
        },
        ValueSource::Enum { enum_name, member } => PValueSource::Enum {
            enum_name: enum_name.clone(),
            member: member.clone(),
        },
        ValueSource::TableField {
            table_id,
            field_name,
        } => PValueSource::TableField {
            table_id: stable_table_id(table_id),
            field_name: field_name.clone(),
        },
        ValueSource::Expression => PValueSource::Expression,
        ValueSource::Unknown => PValueSource::Unknown,
    }
}

fn project_combined_edge(
    e: &CombinedEdge,
    map: &HashMap<String, String>,
    event_by_id: &HashMap<String, &EventSymbol>,
) -> PCombinedEdge {
    PCombinedEdge {
        from: stable_routine_id(&e.from, map),
        to: stable_routine_id(&e.to, map),
        kind: e.kind.clone(),
        callsite_id: e.callsite_id.as_ref().map(|c| stable_sub_id(c, map)),
        operation_id: e.operation_id.as_ref().map(|o| stable_sub_id(o, map)),
        event_id: e
            .event_id
            .as_ref()
            .map(|ev| stable_event_id(ev, event_by_id)),
        subscriber_app_id: e.subscriber_app_id.clone(),
        resolution: e.resolution.clone(),
    }
}

fn project_uncertainty(u: &Uncertainty, map: &HashMap<String, String>) -> PUncertainty {
    PUncertainty {
        kind: u.kind.clone(),
        callsite_id: u.callsite_id.as_ref().map(|c| stable_sub_id(c, map)),
        operation_id: u.operation_id.as_ref().map(|o| stable_sub_id(o, map)),
        routine_id: u.routine_id.as_ref().map(|r| stable_routine_id(r, map)),
        interface_name: u.interface_name.clone(),
    }
}

fn project_typed_edge(
    e: &TypedEdge,
    map: &HashMap<String, String>,
    event_by_id: &HashMap<String, &EventSymbol>,
) -> PTypedEdge {
    PTypedEdge {
        kind: e.kind.clone(),
        from: stable_routine_id(&e.from, map),
        to: e.to.as_ref().map(|t| stable_routine_id(t, map)),
        callsite_id: e.callsite_id.as_ref().map(|c| stable_sub_id(c, map)),
        operation_id: e.operation_id.as_ref().map(|o| stable_sub_id(o, map)),
        event_id: e
            .event_id
            .as_ref()
            .map(|ev| stable_event_id(ev, event_by_id)),
        receiver_type: e.receiver_type.clone(),
        interface_name: e.interface_name.clone(),
        candidate_count: e.candidate_count,
        target_object: e.target_object.as_ref().map(|o| to_stable_object_id(o)),
        object_type: e.object_type.clone(),
        target_id_source: e.target_id_source.as_ref().map(project_value_source),
    }
}

fn project_scc(scc: &Scc, map: &HashMap<String, String>) -> PScc {
    let mut members: Vec<String> = scc
        .members
        .iter()
        .map(|m| stable_routine_id(m, map))
        .collect();
    members.sort_by(|a, b| cmp_stable(a, b));
    PScc {
        members,
        recursive: scc.recursive,
    }
}

/// Project the combined graph + SCC result to the R3a-1 comparison surface. The
/// `routines` provide the internal→stable id map + event symbols.
pub fn project_r3a1(
    routines: &[L3Routine],
    event_graph: &EventGraph,
    graph: &CombinedGraph,
    scc: &SccResult,
) -> R3a1Projection {
    let map: HashMap<String, String> = routines
        .iter()
        .map(|r| (r.id.clone(), r.stable_routine_id.clone()))
        .collect();
    let event_by_id: HashMap<String, &EventSymbol> = event_graph
        .events
        .iter()
        .map(|e| (e.id.clone(), e))
        .collect();

    // Combined edges: al-sem's emission order — sorted nodes → per-from sorted edges.
    let mut combined_edges: Vec<PCombinedEdge> = Vec::new();
    for node in &graph.nodes {
        if let Some(list) = graph.edges_by_from.get(node) {
            for e in list {
                combined_edges.push(project_combined_edge(e, &map, &event_by_id));
            }
        }
    }

    let uncertainty_edges: Vec<PUncertaintyEdge> = graph
        .uncertainty_edges
        .iter()
        .map(|ue| PUncertaintyEdge {
            from: stable_routine_id(&ue.from, &map),
            uncertainty: project_uncertainty(&ue.uncertainty, &map),
        })
        .collect();

    let typed_edges: Vec<PTypedEdge> = graph
        .typed_edges
        .iter()
        .map(|e| project_typed_edge(e, &map, &event_by_id))
        .collect();

    let sccs: Vec<PScc> = scc.sccs.iter().map(|s| project_scc(s, &map)).collect();

    R3a1Projection {
        combined_edges,
        uncertainty_edges,
        typed_edges,
        sccs,
    }
}

// ---------------------------------------------------------------------------
// L3Resolved entry point — assemble combined graph + SCC + project (read-once).
// ---------------------------------------------------------------------------

impl L3Resolved {
    /// Build the combined graph + Tarjan SCC over the resolved SOURCE-ONLY workspace
    /// and project to the R3a-1 stable shape. Mirrors al-sem's
    /// `indexWorkspace → resolveModel → buildCombinedGraph → tarjanScc → projectR3a1`
    /// (READ-once, no dep hooks, no `computeSummaries`).
    pub fn project_r3a1_combined_graph(&self) -> R3a1Projection {
        let ws = &self.workspace;
        let graph = build_combined_graph(ws, &self.calls, &self.events.graph);

        // Tarjan over the combined graph's adjacency (internal ids, pre-sorted).
        let mut adjacency: HashMap<String, Vec<String>> = HashMap::new();
        for (from, list) in &graph.edges_by_from {
            adjacency.insert(from.clone(), list.iter().map(|e| e.to.clone()).collect());
        }
        let scc = tarjan_scc(&SccInputGraph {
            nodes: &graph.nodes,
            edges_by_from: &adjacency,
        });

        project_r3a1(&ws.routines, &self.events.graph, &graph, &scc)
    }
}

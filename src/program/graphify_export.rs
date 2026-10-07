//! Adapter: project the whole-program resolved call graph into a **graphify**
//! node-link extraction document (`{ nodes, edges, hyperedges }`).
//!
//! # Why this exists
//!
//! graphify (<https://github.com/safishamsi/graphify>) turns a corpus into a
//! NetworkX knowledge graph + Leiden communities + Obsidian vault + HTML/GraphML/
//! Neo4j + an MCP query surface. Its own code extractor is a generic tree-sitter
//! AST pass with a name-matching resolver — it has no AL parser and cannot resolve
//! AL dispatch (variable-type resolution, overloads, implicit `Rec`/`SourceTable`,
//! cross-app objects, event publisher→subscriber wiring).
//!
//! This module hands graphify the finished, engine-**resolved** graph instead:
//! one node per AL object + routine, one edge per resolved route, with the honest
//! obligation taxonomy bridged to graphify's `EXTRACTED`/`INFERRED`/`AMBIGUOUS`
//! confidence tiers **without laundering** (see [`bridge_confidence`]). The full
//! engine classification is preserved verbatim in the `obligation`/`evidence`/
//! `dispatch_shape` edge attributes.
//!
//! graphify ingests the emitted document via `graphify/build.py::build_from_json`.
//! The mapping is documented end-to-end in `U:\Git\graphify\adapter.md`.
//!
//! # Determinism
//!
//! Object/routine nodes are emitted in `ProgramGraph`'s already-sorted order;
//! synthetic/builtin/external target nodes are de-duplicated and emitted in sorted
//! id order; edges follow the resolver's deterministic `ClassifiedEdge` order. No
//! clock/random. The output is a stable projection — safe to golden and to
//! `build_merge` incrementally.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

use al_syntax::IdentifierFoldExt;
use al_syntax::ir::ObjectKind;
use serde::Serialize;

use crate::program::graph::ProgramGraph;
use crate::program::node::{AppRef, AppRegistry, ObjKey, ObjectNodeId, RoutineNodeId};
use crate::program::node_extract::{ObjectNode, RoutineNode};
use crate::program::resolve::edge::{
    AbiRoutineKey, BuiltinId, Condition, DispatchShape, Edge, EdgeKind, Evidence,
    ObligationOutcome, OpenWorldReason, RouteTarget, SetCompleteness, Witness, classify_obligation,
};
use crate::program::resolve::full::ClassifiedEdge;
use crate::snapshot::TrustTier;

// ---------------------------------------------------------------------------
// Output schema (graphify node-link extraction document)
// ---------------------------------------------------------------------------

/// One graphify node. `id`/`label`/`file_type` are graphify-native; every `al_*`
/// field is an engine-specific attribute that graphify passes through verbatim to
/// `graph.json` / Neo4j / GraphML (unknown keys are copied, never rejected).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct GNode {
    pub id: String,
    pub label: String,
    pub file_type: &'static str, // always "code" for AL nodes
    /// Always present (graphify treats it as required). Empty string for nodes
    /// with no workspace source — builtin/external/synthetic dynamic/unresolved.
    pub source_file: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub al_object_kind: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub al_routine_kind: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub al_kind: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub al_app: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub al_tier: Option<&'static str>,
}

/// One graphify edge. `source`/`target`/`relation`/`confidence`/`confidence_score`
/// are graphify-native; the rest are pass-through engine attributes that preserve
/// the full obligation taxonomy the 3-tier `confidence` collapses.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct GEdge {
    pub source: String,
    pub target: String,
    pub relation: &'static str,
    pub confidence: &'static str, // EXTRACTED | INFERRED | AMBIGUOUS
    pub confidence_score: f64,
    /// Always present (graphify treats it as required): the file the relationship
    /// was found in — the caller's call site, or the routine's own file for
    /// `contains`. Empty when unknown.
    pub source_file: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub obligation: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub evidence: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dispatch_shape: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub condition: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub open_world_reason: Option<&'static str>,
    /// For an `unknown`-obligation edge: the diagnostic [`UnknownReason`]
    /// (`compoundReceiver`, `catalogMiss`, `memberNotFound`, …) of its first
    /// unresolved route — the "why" behind the failure. `None` on all other edges.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unknown_reason: Option<&'static str>,
    /// Reason-split Task 2, ADDITIVE key (appended last — never reorders the
    /// fields above; BC-Brain consumes this export): the diagnostic
    /// [`crate::snapshot::TrustTier`] of the resolved receiver object for a
    /// `memberNotFound`-reason edge (see [`crate::program::resolve::edge::
    /// Route::receiver_tier`]'s doc). `None` on every other edge, INCLUDING
    /// other `unknown` reasons.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unknown_receiver_tier: Option<&'static str>,
    /// Task 3, ADDITIVE key (appended last — never reorders the fields
    /// above): the explicit may-fire signal for [`Condition::AmbiguousDispatch`]
    /// routes (BC-Brain contract — see [`may_fire_str`]'s doc). `Some(true)`
    /// on a route one of an `AmbiguousOverload` candidate set; `None`
    /// (omitted) on every other edge, so pre-Task-3 exports stay
    /// byte-identical. NEVER `Some(false)`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub may_fire: Option<bool>,
}

/// A graphify extraction document. Fed to `build_from_json` / `build_merge`.
#[derive(Debug, Clone, Serialize)]
pub struct GraphifyDocument {
    pub nodes: Vec<GNode>,
    pub edges: Vec<GEdge>,
    /// Group relationships over 3+ nodes. Empty in the current projection;
    /// reserved for event/subscriber and interface/implementer groupings.
    pub hyperedges: Vec<serde_json::Value>,
}

// ---------------------------------------------------------------------------
// Public entry points
// ---------------------------------------------------------------------------

/// Build the graphify document for a workspace: resolve the whole program, then
/// project the graph. Returns `None` when the snapshot build fails (fail-closed).
#[must_use]
pub fn export_workspace(workspace_root: &Path) -> Option<GraphifyDocument> {
    let (graph, edges, primary) =
        crate::program::resolve::full::resolve_full_program_for_export(workspace_root)?;
    Some(build_graphify_document(&graph, &edges, primary))
}

/// Pure projection: `(ProgramGraph, resolved edges)` → graphify document.
///
/// This is the mapping contract — unit-tested against in-memory graphs so the
/// schema is pinned independently of any workspace fixture.
#[must_use]
pub fn build_graphify_document(
    graph: &ProgramGraph,
    edges: &[ClassifiedEdge],
    _primary_app_ref: AppRef,
) -> GraphifyDocument {
    let obj_by_id: HashMap<&ObjectNodeId, &ObjectNode> =
        graph.objects.iter().map(|o| (&o.id, o)).collect();
    let rtn_by_id: HashMap<&RoutineNodeId, &RoutineNode> =
        graph.routines.iter().map(|r| (&r.id, r)).collect();

    // Best-effort source-file hints (the node tables carry no def location):
    //   caller  → the file its call site sits in (`site.span.unit`)
    //   callee  → the file its resolved Source route witnesses (`Witness::SourceSpan`)
    let loc_by_rtn = build_location_hints(edges);

    // Object source file ← any contained routine's known file (first wins).
    let mut obj_file: HashMap<ObjectNodeId, String> = HashMap::new();
    for rtn in &graph.routines {
        if let Some(f) = loc_by_rtn.get(&rtn.id) {
            obj_file
                .entry(rtn.id.object.clone())
                .or_insert_with(|| f.clone());
        }
    }

    let mut nodes: Vec<GNode> = Vec::new();
    let mut node_ids: HashSet<String> = HashSet::new();
    // Extra (synthetic / builtin / external / off-graph routine) targets, sorted.
    let mut extra_nodes: BTreeMap<String, GNode> = BTreeMap::new();

    // ── Object nodes ─────────────────────────────────────────────────────────
    for obj in &graph.objects {
        let id = object_id_str(&obj.id, &graph.apps);
        if node_ids.insert(id.clone()) {
            nodes.push(GNode {
                id,
                label: object_label(obj),
                file_type: "code",
                source_file: obj_file.get(&obj.id).cloned().unwrap_or_default(),
                al_object_kind: Some(object_kind_str(obj.id.kind)),
                al_routine_kind: None,
                al_kind: Some("object"),
                al_app: app_slug(&graph.apps, obj.id.app),
                al_tier: Some(tier_str(obj.tier)),
            });
        }
    }

    // ── Routine nodes + `contains` edges ─────────────────────────────────────
    let mut edges_out: Vec<GEdge> = Vec::new();
    for rtn in &graph.routines {
        let rid = routine_id_str(&rtn.id, &graph.apps);
        if node_ids.insert(rid.clone()) {
            nodes.push(GNode {
                id: rid.clone(),
                label: routine_label(&rtn.id, rtn.name.as_str(), &obj_by_id),
                file_type: "code",
                source_file: loc_by_rtn.get(&rtn.id).cloned().unwrap_or_default(),
                al_object_kind: Some(object_kind_str(rtn.id.object.kind)),
                al_routine_kind: Some(routine_kind_str(rtn)),
                al_kind: Some("routine"),
                al_app: app_slug(&graph.apps, rtn.id.object.app),
                al_tier: Some(tier_str(rtn.tier)),
            });
        }
        // Containment edge object → routine.
        edges_out.push(GEdge {
            source: object_id_str(&rtn.id.object, &graph.apps),
            target: rid,
            relation: "contains",
            confidence: "EXTRACTED",
            confidence_score: 1.0,
            source_file: loc_by_rtn.get(&rtn.id).cloned().unwrap_or_default(),
            obligation: None,
            evidence: None,
            dispatch_shape: None,
            condition: None,
            open_world_reason: None,
            unknown_reason: None,
            unknown_receiver_tier: None,
            may_fire: None,
        });
    }

    // ── Call / dispatch / event edges (the moat) ─────────────────────────────
    for ce in edges {
        project_edge(
            &ce.edge,
            graph,
            &obj_by_id,
            &rtn_by_id,
            &loc_by_rtn,
            &node_ids,
            &mut extra_nodes,
            &mut edges_out,
        );
    }

    // Group relationships over 3+ nodes (event neighbourhoods, interface families).
    let hyperedges = build_hyperedges(graph, edges, &obj_by_id, &rtn_by_id);

    // Append de-duplicated extra target nodes in sorted id order.
    nodes.extend(extra_nodes.into_values());

    GraphifyDocument {
        nodes,
        edges: edges_out,
        hyperedges,
    }
}

/// Build graphify hyperedges (`{ id, label, nodes:[…] }`, 3+ nodes each):
///
/// - **event groups** — one publisher event + all its (≥2) subscribers, the
///   non-pairwise integration unit ("what reacts when this fires");
/// - **interface families** — one interface + its (≥2) implementers, the
///   polymorphic-dispatch target set.
///
/// graphify renders each as a shaded region; both survive to `graph.json`.
fn build_hyperedges(
    graph: &ProgramGraph,
    edges: &[ClassifiedEdge],
    obj_by_id: &HashMap<&ObjectNodeId, &ObjectNode>,
    rtn_by_id: &HashMap<&RoutineNodeId, &RoutineNode>,
) -> Vec<serde_json::Value> {
    let mut hyper: Vec<serde_json::Value> = Vec::new();

    // ── Event groups: publisher + its ≥2 real subscribers ─────────────────────
    for ce in edges {
        if ce.edge.kind != EdgeKind::EventFlow {
            continue;
        }
        let subs: Vec<String> = ce
            .edge
            .routes
            .iter()
            .filter(|r| {
                !matches!(r.evidence, Evidence::Unknown(_)) && r.target != RouteTarget::Unresolved
            })
            .filter_map(|r| match &r.target {
                RouteTarget::Routine(nid) => Some(routine_id_str(nid, &graph.apps)),
                _ => None,
            })
            .collect();
        if subs.len() < 2 {
            continue; // a hyperedge needs 3+ nodes (publisher + ≥2 subscribers)
        }
        let pub_id = routine_id_str(&ce.edge.from, &graph.apps);
        let label = rtn_by_id
            .get(&ce.edge.from)
            .map(|r| routine_label(&ce.edge.from, r.name.as_str(), obj_by_id))
            .unwrap_or_else(|| routine_label(&ce.edge.from, &ce.edge.from.name_lc, obj_by_id));
        let mut nodes = Vec::with_capacity(subs.len() + 1);
        nodes.push(pub_id.clone());
        nodes.extend(subs);
        hyper.push(serde_json::json!({
            "id": format!("hev:{pub_id}"),
            "label": format!("event: {label}"),
            "kind": "event_group",
            "nodes": nodes,
        }));
    }

    // ── Interface families: interface + its ≥2 implementers ───────────────────
    let iface_by_name: HashMap<String, &ObjectNode> = graph
        .objects
        .iter()
        .filter(|o| o.id.kind == ObjectKind::Interface)
        .map(|o| (o.name.fold_identifier(), o))
        .collect();
    let mut impls: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for o in &graph.objects {
        for iface in &o.implements {
            impls
                .entry(iface.fold_identifier())
                .or_default()
                .push(object_id_str(&o.id, &graph.apps));
        }
    }
    for (iface_lc, impl_ids) in &impls {
        if impl_ids.len() < 2 {
            continue;
        }
        let Some(iface_obj) = iface_by_name.get(iface_lc) else {
            continue; // interface not in the graph — skip rather than dangle
        };
        let iface_id = object_id_str(&iface_obj.id, &graph.apps);
        let mut nodes = Vec::with_capacity(impl_ids.len() + 1);
        nodes.push(iface_id.clone());
        nodes.extend(impl_ids.iter().cloned());
        hyper.push(serde_json::json!({
            "id": format!("hif:{iface_id}"),
            "label": format!("interface: {} ({} impls)", iface_obj.name, impl_ids.len()),
            "kind": "interface_group",
            "nodes": nodes,
        }));
    }

    hyper
}

// ---------------------------------------------------------------------------
// Edge projection + the confidence bridge
// ---------------------------------------------------------------------------

/// Project one resolved [`Edge`] into 0+ graphify edges (one per real route, or a
/// single synthetic edge for honest-dynamic / honest-empty / unknown), ensuring
/// every referenced target node exists (graphify prunes dangling edges).
#[allow(clippy::too_many_arguments)]
fn project_edge(
    edge: &Edge,
    graph: &ProgramGraph,
    obj_by_id: &HashMap<&ObjectNodeId, &ObjectNode>,
    rtn_by_id: &HashMap<&RoutineNodeId, &RoutineNode>,
    loc_by_rtn: &HashMap<RoutineNodeId, String>,
    emitted_ids: &HashSet<String>,
    extra_nodes: &mut BTreeMap<String, GNode>,
    edges_out: &mut Vec<GEdge>,
) {
    let source = routine_id_str(&edge.from, &graph.apps);
    let site_file = edge.site.span.unit.clone();
    let outcome = classify_obligation(edge);
    let dispatch_shape = Some(dispatch_shape_str(edge.shape));

    match outcome {
        ObligationOutcome::Resolved | ObligationOutcome::ConditionalResolved => {
            let obligation = if outcome == ObligationOutcome::ConditionalResolved {
                "conditional_resolved"
            } else {
                "resolved"
            };
            // One graphify edge per REAL route (skip Unknown/Unresolved routes).
            for route in &edge.routes {
                if matches!(route.evidence, Evidence::Unknown(_))
                    || route.target == RouteTarget::Unresolved
                {
                    continue;
                }
                let target = ensure_target_node(
                    &route.target,
                    graph,
                    obj_by_id,
                    rtn_by_id,
                    loc_by_rtn,
                    emitted_ids,
                    extra_nodes,
                );
                edges_out.push(GEdge {
                    source: source.clone(),
                    target,
                    relation: relation_for(edge.kind, &route.target),
                    confidence: "EXTRACTED",
                    confidence_score: 1.0,
                    source_file: site_file.clone(),
                    obligation: Some(obligation),
                    evidence: Some(evidence_str(route.evidence)),
                    dispatch_shape,
                    condition: condition_str(&route.conditions),
                    open_world_reason: None,
                    unknown_reason: None,
                    unknown_receiver_tier: None,
                    may_fire: may_fire_str(&route.conditions),
                });
            }
        }
        // Task 3 (mechanics only — no producer constructs this outcome yet):
        // one graphify edge per candidate route, same per-route shape as the
        // `Resolved` arm above, but distinctly labelled `ambiguous_resolved`/
        // `ambiguous_overload` and carrying the explicit `may_fire` signal so
        // BC-Brain never reads these as dead code (round-2 closer #3 —
        // BINDING). INFERRED, not AMBIGUOUS: the candidate set is a proven,
        // closed enumeration (unlike `Unknown`'s true failure) — exactly one
        // of these WILL run, this engine just cannot name which.
        ObligationOutcome::AmbiguousResolved => {
            for route in &edge.routes {
                // Defense-in-depth (whole-branch review F3): `classify_
                // obligation`'s `is_ambiguous_resolved` prevalidation already
                // GUARANTEES every route reaching this arm carries
                // non-`Unknown` evidence and a non-`Unresolved` target (see
                // that function's doc) — this arm is therefore safe by
                // invariant to iterate unconditionally, unlike the `Resolved`
                // arm above (whose routes are NOT pre-filtered that way).
                // Mirror that arm's skip anyway: a future producer bug that
                // violates the invariant then degrades to a silently-dropped
                // edge rather than a bogus node/edge for an `Unresolved`
                // target, and the `debug_assert` makes the violation loud in
                // debug/test builds instead of only in release-mode output.
                if matches!(route.evidence, Evidence::Unknown(_))
                    || route.target == RouteTarget::Unresolved
                {
                    debug_assert!(
                        false,
                        "AmbiguousResolved route must be concrete per classify_obligation's \
                         is_ambiguous_resolved invariant; got {route:?}"
                    );
                    continue;
                }
                let target = ensure_target_node(
                    &route.target,
                    graph,
                    obj_by_id,
                    rtn_by_id,
                    loc_by_rtn,
                    emitted_ids,
                    extra_nodes,
                );
                edges_out.push(GEdge {
                    source: source.clone(),
                    target,
                    relation: relation_for(edge.kind, &route.target),
                    confidence: "INFERRED",
                    confidence_score: 0.5,
                    source_file: site_file.clone(),
                    obligation: Some("ambiguous_resolved"),
                    evidence: Some(evidence_str(route.evidence)),
                    dispatch_shape,
                    condition: condition_str(&route.conditions),
                    open_world_reason: None,
                    unknown_reason: None,
                    unknown_receiver_tier: None,
                    may_fire: may_fire_str(&route.conditions),
                });
            }
        }
        ObligationOutcome::HonestDynamic | ObligationOutcome::HonestEmpty => {
            // Provably dynamic / legal-empty fan-out: NOT a failure. INFERRED, to a
            // synthetic dynamic node so the honest uncertainty stays visible.
            let obligation = if outcome == ObligationOutcome::HonestDynamic {
                "honest_dynamic"
            } else {
                "honest_empty"
            };
            let target = synthetic_node(
                extra_nodes,
                emitted_ids,
                format!("al:dynamic:{}#{}", source, edge.site.callee_fingerprint),
                "«dynamic»".to_string(),
                "dynamic",
            );
            edges_out.push(GEdge {
                source,
                target,
                relation: relation_for(edge.kind, &RouteTarget::Unresolved),
                confidence: "INFERRED",
                confidence_score: 0.5,
                source_file: site_file,
                obligation: Some(obligation),
                evidence: None,
                dispatch_shape,
                condition: None,
                open_world_reason: open_world_reason_str(edge.completeness),
                unknown_reason: None,
                unknown_receiver_tier: None,
                may_fire: None,
            });
        }
        ObligationOutcome::Unknown => {
            // The one true failure — AMBIGUOUS, surfaces in graphify's review report.
            let target = synthetic_node(
                extra_nodes,
                emitted_ids,
                format!("al:unresolved:{}#{}", source, edge.site.callee_fingerprint),
                "«unresolved»".to_string(),
                "unresolved",
            );
            edges_out.push(GEdge {
                source,
                target,
                relation: relation_for(edge.kind, &RouteTarget::Unresolved),
                confidence: "AMBIGUOUS",
                confidence_score: 0.2,
                source_file: site_file,
                obligation: Some("unknown"),
                evidence: None,
                dispatch_shape,
                condition: None,
                open_world_reason: None,
                unknown_reason: edge.routes.iter().find_map(|r| match r.evidence {
                    Evidence::Unknown(reason) => Some(reason.as_str()),
                    _ => None,
                }),
                // Reason-split Task 2, additive: the same first `Unknown`
                // route's `receiver_tier`, rendered via the canonical
                // `TrustTier::as_str()` — `None` unless that route's reason
                // is `MemberNotFound` (see `Route::receiver_tier`'s doc).
                unknown_receiver_tier: edge
                    .routes
                    .iter()
                    .find_map(|r| match r.evidence {
                        Evidence::Unknown(_) => Some(r.receiver_tier),
                        _ => None,
                    })
                    .flatten()
                    .map(|t| t.as_str()),
                may_fire: None,
            });
        }
    }
}

/// Ensure a node exists for a route target and return its id. Source routines and
/// objects are already emitted in the main loops; builtin/external/off-graph
/// routine targets are added to `extra_nodes` on demand.
#[allow(clippy::too_many_arguments)]
fn ensure_target_node(
    target: &RouteTarget,
    graph: &ProgramGraph,
    obj_by_id: &HashMap<&ObjectNodeId, &ObjectNode>,
    rtn_by_id: &HashMap<&RoutineNodeId, &RoutineNode>,
    loc_by_rtn: &HashMap<RoutineNodeId, String>,
    emitted_ids: &HashSet<String>,
    extra_nodes: &mut BTreeMap<String, GNode>,
) -> String {
    match target {
        RouteTarget::Routine(nid) => {
            let id = routine_id_str(nid, &graph.apps);
            if !emitted_ids.contains(&id) && !extra_nodes.contains_key(&id) {
                // Off-graph routine target (rare) — synthesize a minimal node so the
                // edge never dangles. Prefer the real RoutineNode when present.
                let label = rtn_by_id
                    .get(nid)
                    .map(|r| routine_label(nid, r.name.as_str(), obj_by_id))
                    .unwrap_or_else(|| routine_label(nid, &nid.name_lc, obj_by_id));
                extra_nodes.insert(
                    id.clone(),
                    GNode {
                        id: id.clone(),
                        label,
                        file_type: "code",
                        source_file: loc_by_rtn.get(nid).cloned().unwrap_or_default(),
                        al_object_kind: Some(object_kind_str(nid.object.kind)),
                        al_routine_kind: Some("procedure"),
                        al_kind: Some("routine"),
                        al_app: app_slug(&graph.apps, nid.object.app),
                        al_tier: None,
                    },
                );
            }
            id
        }
        RouteTarget::Builtin(BuiltinId(bid)) => {
            let id = format!("al:builtin:{bid}");
            if !extra_nodes.contains_key(&id) {
                extra_nodes.insert(
                    id.clone(),
                    GNode {
                        id: id.clone(),
                        label: bid.clone(),
                        file_type: "code",
                        source_file: String::new(),
                        al_object_kind: None,
                        al_routine_kind: None,
                        al_kind: Some("builtin"),
                        al_app: None,
                        al_tier: None,
                    },
                );
            }
            id
        }
        RouteTarget::AbiSymbol { key } => {
            let id = abi_id_str(key, &graph.apps);
            if !extra_nodes.contains_key(&id) {
                extra_nodes.insert(
                    id.clone(),
                    GNode {
                        id: id.clone(),
                        label: abi_label(key),
                        file_type: "code",
                        source_file: String::new(),
                        al_object_kind: None,
                        al_routine_kind: None,
                        al_kind: Some("external"),
                        al_app: app_slug(&graph.apps, key.app),
                        al_tier: Some("symbol_only"),
                    },
                );
            }
            id
        }
        RouteTarget::Unresolved => {
            // Never reached: unresolved routes are handled at the edge level.
            "al:unresolved:orphan".to_string()
        }
    }
}

/// Insert a synthetic node (dynamic / unresolved) if absent and return its id.
fn synthetic_node(
    extra_nodes: &mut BTreeMap<String, GNode>,
    emitted_ids: &HashSet<String>,
    id: String,
    label: String,
    al_kind: &'static str,
) -> String {
    if !emitted_ids.contains(&id) && !extra_nodes.contains_key(&id) {
        extra_nodes.insert(
            id.clone(),
            GNode {
                id: id.clone(),
                label,
                file_type: "code",
                source_file: String::new(),
                al_object_kind: None,
                al_routine_kind: None,
                al_kind: Some(al_kind),
                al_app: None,
                al_tier: None,
            },
        );
    }
    id
}

// ---------------------------------------------------------------------------
// Location hints
// ---------------------------------------------------------------------------

/// Recover a best-effort source file per routine from the edge set (the node
/// tables have no def location). Callee witnesses (the real def file) win over
/// caller call-site files.
fn build_location_hints(edges: &[ClassifiedEdge]) -> HashMap<RoutineNodeId, String> {
    let mut map: HashMap<RoutineNodeId, String> = HashMap::new();
    // Pass 1: caller files (call-site unit). First write wins (deterministic order).
    for ce in edges {
        map.entry(ce.edge.from.clone())
            .or_insert_with(|| ce.edge.site.span.unit.clone());
    }
    // Pass 2: callee def files from Source-route witnesses (authoritative — overwrite).
    for ce in edges {
        for route in &ce.edge.routes {
            if let (RouteTarget::Routine(nid), Witness::SourceSpan { file, .. }) =
                (&route.target, &route.witness)
            {
                map.insert(nid.clone(), file.clone());
            }
        }
    }
    map
}

// ---------------------------------------------------------------------------
// Id / label helpers
// ---------------------------------------------------------------------------

/// Slugify an app name for use inside a stable node id (lowercase, alnum + dash).
fn sanitize_slug(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_dash = false;
    for ch in s.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash {
            out.push('-');
            prev_dash = true;
        }
    }
    out.trim_matches('-').to_string()
}

/// Resolve an `AppRef` to a stable slug. `None` if the ref is unknown (never for
/// graph-derived refs). NOTE: keyed on the app NAME, never the interned `AppRef`
/// integer (which is assignment-order dependent — see node.rs).
fn app_slug(apps: &AppRegistry, app: AppRef) -> Option<String> {
    apps.try_resolve(app).map(|id| sanitize_slug(&id.name))
}

fn obj_key_str(key: &ObjKey) -> String {
    match key {
        ObjKey::Id(n) => n.to_string(),
        ObjKey::Name(s) => sanitize_slug(s),
    }
}

/// `al:obj:{app}/{kind}/{key}` — stable object node id.
fn object_id_str(oid: &ObjectNodeId, apps: &AppRegistry) -> String {
    let app = app_slug(apps, oid.app).unwrap_or_else(|| format!("app{}", oid.app.0));
    format!(
        "al:obj:{app}/{}/{}",
        object_kind_str(oid.kind).to_ascii_lowercase(),
        obj_key_str(&oid.key)
    )
}

/// `al:rtn:{app}/{kind}/{key}#{enclosing?}{name}/{params}/{sig_fp}` — unique per
/// overload (params_count + sig_fp) and per member-trigger (enclosing member).
fn routine_id_str(nid: &RoutineNodeId, apps: &AppRegistry) -> String {
    let obj = object_id_str(&nid.object, apps);
    let member = nid
        .enclosing_member_lc
        .as_deref()
        .map(|m| format!("{m}."))
        .unwrap_or_default();
    format!(
        "al:rtn:{}#{}{}/{}/{}",
        obj.trim_start_matches("al:obj:"),
        member,
        sanitize_slug(&nid.name_lc),
        nid.params_count,
        nid.sig_fp
    )
}

fn object_label(obj: &ObjectNode) -> String {
    let kind = object_kind_str(obj.id.kind);
    match obj.declared_id {
        Some(n) => format!("{kind} {n} \"{}\"", obj.name),
        None => format!("{kind} \"{}\"", obj.name),
    }
}

fn routine_label(
    nid: &RoutineNodeId,
    name: &str,
    obj_by_id: &HashMap<&ObjectNodeId, &ObjectNode>,
) -> String {
    let obj_name = obj_by_id
        .get(&nid.object)
        .map(|o| o.name.clone())
        .unwrap_or_else(|| obj_key_str(&nid.object.key));
    format!("{obj_name}.{name}()")
}

fn abi_id_str(key: &AbiRoutineKey, apps: &AppRegistry) -> String {
    let app = app_slug(apps, key.app).unwrap_or_else(|| format!("app{}", key.app.0));
    format!(
        "al:abi:{app}/{}/{}#{}/{}",
        sanitize_slug(&key.object_type),
        key.object_number,
        sanitize_slug(&key.routine_name_lc),
        key.params_count
    )
}

fn abi_label(key: &AbiRoutineKey) -> String {
    format!(
        "«ext» {} {} \"{}\".{}()",
        key.object_type, key.object_number, key.object_name_lc, key.routine_name_lc
    )
}

// ---------------------------------------------------------------------------
// Enum → string helpers
// ---------------------------------------------------------------------------

fn object_kind_str(k: ObjectKind) -> &'static str {
    match k {
        ObjectKind::Codeunit => "Codeunit",
        ObjectKind::Table => "Table",
        ObjectKind::TableExtension => "TableExtension",
        ObjectKind::Page => "Page",
        ObjectKind::PageExtension => "PageExtension",
        ObjectKind::Report => "Report",
        ObjectKind::ReportExtension => "ReportExtension",
        ObjectKind::Query => "Query",
        ObjectKind::XmlPort => "XmlPort",
        ObjectKind::Enum => "Enum",
        ObjectKind::EnumExtension => "EnumExtension",
        ObjectKind::Interface => "Interface",
        ObjectKind::ControlAddIn => "ControlAddIn",
        ObjectKind::Entitlement => "Entitlement",
        ObjectKind::PermissionSet => "PermissionSet",
        ObjectKind::PermissionSetExtension => "PermissionSetExtension",
        ObjectKind::Profile => "Profile",
        ObjectKind::Other => "Other",
    }
}

fn routine_kind_str(r: &RoutineNode) -> &'static str {
    match r.publisher_kind {
        Some(crate::program::resolve::event::PublisherKind::Platform) => "platform_event",
        Some(_) => "event_publisher",
        None if !r.event_subscribers.is_empty() => "event_subscriber",
        None if r.is_trigger => "trigger",
        None => "procedure",
    }
}

fn tier_str(t: TrustTier) -> &'static str {
    // Delegates to the canonical mapping (resolve-reason-split Task 2) —
    // byte-identical strings to the pre-Task-2 hand-rolled match, so existing
    // `al_tier` output is unaffected.
    t.as_str()
}

fn relation_for(kind: EdgeKind, target: &RouteTarget) -> &'static str {
    match kind {
        EdgeKind::Run => "runs",
        EdgeKind::ImplicitTrigger => "triggers",
        EdgeKind::EventFlow => "raises_event",
        EdgeKind::Call => match target {
            RouteTarget::Builtin(_) => "calls_builtin",
            RouteTarget::AbiSymbol { .. } => "calls_external",
            _ => "calls",
        },
    }
}

fn evidence_str(e: Evidence) -> &'static str {
    match e {
        Evidence::Source => "source",
        Evidence::Abi => "abi",
        Evidence::Catalog => "catalog",
        Evidence::Opaque => "opaque",
        Evidence::Unknown(_) => "unknown",
    }
}

fn dispatch_shape_str(s: DispatchShape) -> &'static str {
    match s {
        DispatchShape::Exact => "exact",
        DispatchShape::Polymorphic => "polymorphic",
        DispatchShape::Multicast => "multicast",
        DispatchShape::DynamicOpen => "dynamic_open",
        DispatchShape::AmbiguousOverload => "ambiguous_overload",
    }
}

fn condition_str(conditions: &[Condition]) -> Option<&'static str> {
    if conditions.contains(&Condition::ManualBinding) {
        Some("manual_binding")
    } else if conditions.contains(&Condition::SkipOnMissingLicense) {
        Some("skip_on_missing_license")
    } else if conditions.contains(&Condition::SkipOnMissingPermission) {
        Some("skip_on_missing_permission")
    } else if conditions.contains(&Condition::RunTriggerGuarded) {
        Some("run_trigger_guarded")
    } else if conditions.contains(&Condition::AmbiguousDispatch) {
        Some("ambiguous_dispatch")
    } else {
        None
    }
}

/// `Condition::AmbiguousDispatch` MAY-FIRE projection (Task 3 round-2 closer
/// #3, BINDING): `graphify_export` must never let BC-Brain read
/// `fires_by_default:false`-shaped output as dead code for these routes —
/// exactly one candidate of an `AmbiguousOverload` set WILL run at runtime.
/// `Some(true)` for a route carrying the condition; `None` (skipped by
/// `#[serde(skip_serializing_if = "Option::is_none")]`) for every other
/// route, so every pre-Task-3 export stays byte-identical. ADDITIVE ONLY —
/// never emits `Some(false)` (there is no "may not fire" claim to make).
fn may_fire_str(conditions: &[Condition]) -> Option<bool> {
    if conditions.contains(&Condition::AmbiguousDispatch) {
        Some(true)
    } else {
        None
    }
}

fn open_world_reason_str(c: SetCompleteness) -> Option<&'static str> {
    match c {
        SetCompleteness::Complete => None,
        SetCompleteness::Partial { reason } => Some(match reason {
            OpenWorldReason::ReverseDependentImplementers => "reverse_dependent_implementers",
            OpenWorldReason::ReverseDependentSubscribers => "reverse_dependent_subscribers",
            OpenWorldReason::ReverseDependentExtensions => "reverse_dependent_extensions",
            OpenWorldReason::RuntimeTypeUnbounded => "runtime_type_unbounded",
        }),
    }
}

// ---------------------------------------------------------------------------
// Incremental: per-object fragments + content-hash manifest (P3)
// ---------------------------------------------------------------------------

/// The whole graphify document partitioned into per-object fragments plus a
/// change-detection manifest.
///
/// Whole-program resolution is cheap for AL (re-run on any edit), so the
/// incremental value is NOT skipping extraction — it is telling a downstream
/// consumer (Obsidian vault, embeddings) exactly which objects' output changed
/// so only those are re-processed. Each object owns one fragment (`{nodes,
/// edges, hyperedges}`: its object node + routines + `contains` + the edges/
/// hyperedges ORIGINATING from it); `manifest[obj]` is a stable content hash of
/// that fragment. Re-run, diff the manifest → the changed set. `shared` holds
/// the synthetic/builtin/external target nodes referenced across fragments (so
/// nothing dangles when graphify `build_merge`s them all).
#[derive(Debug, Clone, Serialize)]
pub struct FragmentSet {
    /// objectId → stable content hash of its fragment.
    pub manifest: BTreeMap<String, String>,
    /// objectId → its fragment.
    pub fragments: BTreeMap<String, GraphifyDocument>,
    /// Cross-fragment target nodes (builtin / external / dynamic / unresolved).
    pub shared: GraphifyDocument,
}

/// Build the per-object fragment set for a workspace (resolve, project, partition).
#[must_use]
pub fn export_workspace_fragments(workspace_root: &Path) -> Option<FragmentSet> {
    let (graph, edges, primary) =
        crate::program::resolve::full::resolve_full_program_for_export(workspace_root)?;
    Some(build_fragment_set(&graph, &edges, primary))
}

/// Partition a built document into per-object fragments + a content-hash manifest.
#[must_use]
pub fn build_fragment_set(
    graph: &ProgramGraph,
    edges: &[ClassifiedEdge],
    primary_app_ref: AppRef,
) -> FragmentSet {
    let doc = build_graphify_document(graph, edges, primary_app_ref);
    let empty = || GraphifyDocument {
        nodes: Vec::new(),
        edges: Vec::new(),
        hyperedges: Vec::new(),
    };
    let mut fragments: BTreeMap<String, GraphifyDocument> = BTreeMap::new();
    let mut shared = empty();

    for n in doc.nodes {
        match owning_object(&n.id) {
            Some(obj) => fragments.entry(obj).or_insert_with(empty).nodes.push(n),
            None => shared.nodes.push(n),
        }
    }
    for e in doc.edges {
        // Every edge originates at a real object-owned node (object for
        // `contains`, routine otherwise); it belongs to that object's fragment.
        match owning_object(&e.source) {
            Some(obj) => fragments.entry(obj).or_insert_with(empty).edges.push(e),
            None => shared.edges.push(e),
        }
    }
    for h in doc.hyperedges {
        // A hyperedge is anchored at its first node (publisher / interface).
        let anchor = h
            .get("nodes")
            .and_then(|n| n.get(0))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        match owning_object(anchor) {
            Some(obj) => fragments
                .entry(obj)
                .or_insert_with(empty)
                .hyperedges
                .push(h),
            None => shared.hyperedges.push(h),
        }
    }

    // Sort within each fragment for a stable serialization, then hash.
    let mut manifest: BTreeMap<String, String> = BTreeMap::new();
    for (obj, f) in &mut fragments {
        f.nodes.sort_by(|a, b| a.id.cmp(&b.id));
        f.edges.sort_by(|a, b| {
            (a.source.as_str(), a.target.as_str(), a.relation).cmp(&(
                b.source.as_str(),
                b.target.as_str(),
                b.relation,
            ))
        });
        let json = serde_json::to_string(f).unwrap_or_default();
        manifest.insert(obj.clone(), format!("{:016x}", fnv1a(json.as_bytes())));
    }
    shared.nodes.sort_by(|a, b| a.id.cmp(&b.id));

    FragmentSet {
        manifest,
        fragments,
        shared,
    }
}

/// The `al:obj:…` id owning a node/edge-source id, or `None` for a
/// cross-fragment target (builtin / external / dynamic / unresolved).
fn owning_object(id: &str) -> Option<String> {
    if id.starts_with("al:obj:") {
        return Some(id.to_string());
    }
    if let Some(rest) = id.strip_prefix("al:rtn:") {
        let obj = rest.split('#').next().unwrap_or(rest);
        return Some(format!("al:obj:{obj}"));
    }
    None
}

/// FNV-1a 64-bit — a small, dependency-free, run-stable content hash for the
/// manifest (deterministic across processes; only used for change detection).
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

// ---------------------------------------------------------------------------
// Tests — the mapping contract, pinned against in-memory graphs
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::program::graph::ObjectIndex;
    use crate::program::node::{AppRegistry, ObjKey, ObjectNodeId};
    use crate::program::node_extract::AbiParams;
    use crate::program::resolve::edge::{CanonicalSpan, Route, SiteId, SourcePos};
    use crate::program::topology::DependencyGraph;
    use crate::snapshot::AppId;

    fn app_id(name: &str) -> AppId {
        AppId {
            guid: String::new(),
            name: name.into(),
            publisher: "P".into(),
            version: "1.0.0.0".into(),
        }
    }

    fn obj(app: AppRef, kind: ObjectKind, id: i64, name: &str) -> ObjectNode {
        ObjectNode {
            id: ObjectNodeId {
                app,
                kind,
                key: ObjKey::Id(id),
            },
            name: name.to_string(),
            declared_id: Some(id),
            extends_target: None,
            implements: vec![],
            tier: TrustTier::Workspace,
            source_table: None,
            table_no: None,
            source_table_temporary: false,
            page_controls: vec![],
            fields: vec![],
            dataitems: vec![],
            query_columns: Vec::new(),
            protected_vars: Vec::new(),
            parse_incomplete: false,
        }
    }

    fn rtn(app: AppRef, obj_id: i64, name: &str, params: usize) -> RoutineNode {
        RoutineNode {
            id: RoutineNodeId {
                object: ObjectNodeId {
                    app,
                    kind: ObjectKind::Codeunit,
                    key: ObjKey::Id(obj_id),
                },
                name_lc: name.fold_identifier(),
                enclosing_member_lc: None,
                params_count: params,
                sig_fp: 0,
            },
            name: name.to_string(),
            is_trigger: false,
            access: crate::program::node_extract::Access::Public,
            tier: TrustTier::Workspace,
            event_subscribers: vec![],
            subscriber_instance_manual: false,
            publisher_kind: None,
            include_sender: None,
            abi_routine_kind: None,
            abi_event_kind: None,
            param_sig_key: String::new(),
            return_type: None,
            return_type_id: None,
            abi_overload_collapsed: false,
            source_overload_aliased: false,
            abi_params: AbiParams::Missing,
        }
    }

    fn rid(app: AppRef, obj_id: i64, name: &str, params: usize) -> RoutineNodeId {
        RoutineNodeId {
            object: ObjectNodeId {
                app,
                kind: ObjectKind::Codeunit,
                key: ObjKey::Id(obj_id),
            },
            name_lc: name.fold_identifier(),
            enclosing_member_lc: None,
            params_count: params,
            sig_fp: 0,
        }
    }

    fn site(caller: RoutineNodeId, fp: u64) -> SiteId {
        SiteId {
            caller,
            span: CanonicalSpan {
                unit: "src/Cu.al".into(),
                start: SourcePos { line: 3, col: 5 },
                end: SourcePos { line: 3, col: 20 },
            },
            callee_fingerprint: fp,
        }
    }

    /// Two-codeunit workspace: CU 50100 "Caller" has `Foo` calling CU 50101
    /// "Callee".`Bar` (a Source route). Assemble the graph + one ClassifiedEdge.
    fn fixture() -> (ProgramGraph, Vec<ClassifiedEdge>, AppRef) {
        let mut apps = AppRegistry::default();
        let a = apps.intern(&app_id("MyApp"));

        let mut objects = vec![
            obj(a, ObjectKind::Codeunit, 50100, "Caller"),
            obj(a, ObjectKind::Codeunit, 50101, "Callee"),
        ];
        objects.sort_by(|x, y| x.id.cmp(&y.id));

        let mut routines = vec![rtn(a, 50100, "Foo", 0), rtn(a, 50101, "Bar", 0)];
        routines.sort_by(|x, y| x.id.cmp(&y.id));

        let obj_index = ObjectIndex::build(&objects);
        let graph = ProgramGraph {
            apps,
            topology: DependencyGraph::default(),
            objects: objects.into(),
            routines: routines.into(),
            obj_index,
            friends: Default::default(),
            abi_ingest_errors: Default::default(),
            workspace_rows: Default::default(),
        };

        let caller = rid(a, 50100, "Foo", 0);
        let callee = rid(a, 50101, "Bar", 0);
        let edge = Edge {
            from: caller.clone(),
            site: site(caller.clone(), 42),
            kind: EdgeKind::Call,
            shape: DispatchShape::Exact,
            completeness: SetCompleteness::Complete,
            routes: vec![Route {
                target: RouteTarget::Routine(callee.clone()),
                evidence: Evidence::Source,
                conditions: vec![],
                witness: Witness::SourceSpan {
                    file: "src/Callee.al".into(),
                    span: (10, 20),
                },
                receiver_tier: None,
            }],
        };
        let ce = ClassifiedEdge {
            obligation_id: crate::program::resolve::full::ObligationId::CallSite {
                caller,
                span: edge.site.span.clone(),
                callee_fp: 42,
            },
            edge,
        };
        (graph, vec![ce], a)
    }

    #[test]
    fn objects_routines_and_contains_edges_emitted() {
        let (g, edges, primary) = fixture();
        let doc = build_graphify_document(&g, &edges, primary);

        // 2 objects + 2 routines = 4 base nodes.
        assert_eq!(doc.nodes.len(), 4, "2 objects + 2 routines");
        let ids: HashSet<&str> = doc.nodes.iter().map(|n| n.id.as_str()).collect();
        assert!(ids.contains("al:obj:myapp/codeunit/50100"));
        assert!(ids.contains("al:rtn:myapp/codeunit/50100#foo/0/0"));
        assert!(ids.contains("al:rtn:myapp/codeunit/50101#bar/0/0"));

        // Two `contains` edges (one per routine).
        let contains: Vec<&GEdge> = doc
            .edges
            .iter()
            .filter(|e| e.relation == "contains")
            .collect();
        assert_eq!(contains.len(), 2);
        assert!(contains.iter().all(|e| e.confidence == "EXTRACTED"));
    }

    #[test]
    fn source_call_becomes_extracted_calls_edge() {
        let (g, edges, primary) = fixture();
        let doc = build_graphify_document(&g, &edges, primary);

        let calls: Vec<&GEdge> = doc.edges.iter().filter(|e| e.relation == "calls").collect();
        assert_eq!(calls.len(), 1, "exactly one resolved call edge");
        let e = calls[0];
        assert_eq!(e.source, "al:rtn:myapp/codeunit/50100#foo/0/0");
        assert_eq!(e.target, "al:rtn:myapp/codeunit/50101#bar/0/0");
        assert_eq!(e.confidence, "EXTRACTED");
        assert_eq!(e.confidence_score, 1.0);
        assert_eq!(e.obligation, Some("resolved"));
        assert_eq!(e.evidence, Some("source"));
        assert_eq!(e.dispatch_shape, Some("exact"));
    }

    #[test]
    fn callee_source_file_recovered_from_witness() {
        let (g, edges, primary) = fixture();
        let doc = build_graphify_document(&g, &edges, primary);
        let callee = doc
            .nodes
            .iter()
            .find(|n| n.id == "al:rtn:myapp/codeunit/50101#bar/0/0")
            .unwrap();
        assert_eq!(callee.source_file, "src/Callee.al");
    }

    /// The confidence bridge must NOT launder: an Unknown obligation → AMBIGUOUS
    /// edge to a synthetic unresolved node (no dangling edge).
    #[test]
    fn unknown_obligation_bridges_to_ambiguous() {
        let (mut g, _edges, primary) = fixture();
        // Rebuild an edge with a single Unknown/Unresolved route.
        let caller = rid(primary, 50100, "Foo", 0);
        let edge = Edge {
            from: caller.clone(),
            site: site(caller.clone(), 7),
            kind: EdgeKind::Call,
            shape: DispatchShape::Exact,
            completeness: SetCompleteness::Complete,
            routes: vec![Route {
                target: RouteTarget::Unresolved,
                evidence: Evidence::Unknown(
                    crate::program::resolve::edge::UnknownReason::UnclassifiedCallee,
                ),
                conditions: vec![],
                witness: Witness::None,
                receiver_tier: None,
            }],
        };
        let ce = ClassifiedEdge {
            obligation_id: crate::program::resolve::full::ObligationId::CallSite {
                caller,
                span: edge.site.span.clone(),
                callee_fp: 7,
            },
            edge,
        };
        // Drop the routines so only the unknown edge is present.
        g.routines.clear();
        let doc = build_graphify_document(&g, &[ce], primary);

        let amb: Vec<&GEdge> = doc
            .edges
            .iter()
            .filter(|e| e.confidence == "AMBIGUOUS")
            .collect();
        assert_eq!(amb.len(), 1);
        assert_eq!(amb[0].obligation, Some("unknown"));
        // The raw per-route decline reason rides along for BC-Brain.
        assert_eq!(amb[0].unknown_reason, Some("unclassifiedCallee"));
        // Target node must exist (graphify prunes dangling edges).
        let tgt = &amb[0].target;
        assert!(
            doc.nodes.iter().any(|n| &n.id == tgt),
            "synthetic unresolved node must be emitted"
        );
    }

    #[test]
    fn document_serializes_to_networkx_fragment_shape() {
        let (g, edges, primary) = fixture();
        let doc = build_graphify_document(&g, &edges, primary);
        let v = serde_json::to_value(&doc).unwrap();
        assert!(v.get("nodes").is_some());
        assert!(v.get("edges").is_some());
        assert!(v.get("hyperedges").is_some());
        // A node carries id/label/file_type.
        let n0 = &v["nodes"][0];
        assert!(n0.get("id").is_some());
        assert!(n0.get("label").is_some());
        assert_eq!(n0["file_type"], "code");
    }

    #[test]
    fn event_with_multiple_subscribers_emits_hyperedge() {
        use crate::program::resolve::edge::OpenWorldReason;
        let mut apps = AppRegistry::default();
        let a = apps.intern(&app_id("MyApp"));
        let mut objects = vec![
            obj(a, ObjectKind::Codeunit, 50100, "Pub"),
            obj(a, ObjectKind::Codeunit, 50101, "SubA"),
            obj(a, ObjectKind::Codeunit, 50102, "SubB"),
        ];
        objects.sort_by(|x, y| x.id.cmp(&y.id));
        let mut routines = vec![
            rtn(a, 50100, "OnAfterPost", 0),
            rtn(a, 50101, "HandleA", 1),
            rtn(a, 50102, "HandleB", 1),
        ];
        routines.sort_by(|x, y| x.id.cmp(&y.id));
        let obj_index = ObjectIndex::build(&objects);
        let graph = ProgramGraph {
            apps,
            topology: DependencyGraph::default(),
            objects: objects.into(),
            routines: routines.into(),
            obj_index,
            friends: Default::default(),
            abi_ingest_errors: Default::default(),
            workspace_rows: Default::default(),
        };

        let pubr = rid(a, 50100, "OnAfterPost", 0);
        let mk_route = |nid: RoutineNodeId| Route {
            target: RouteTarget::Routine(nid),
            evidence: Evidence::Source,
            conditions: vec![],
            witness: Witness::SourceSpan {
                file: "f.al".into(),
                span: (0, 1),
            },
            receiver_tier: None,
        };
        let edge = Edge {
            from: pubr.clone(),
            site: site(pubr.clone(), 9),
            kind: EdgeKind::EventFlow,
            shape: DispatchShape::Multicast,
            completeness: SetCompleteness::Partial {
                reason: OpenWorldReason::ReverseDependentSubscribers,
            },
            routes: vec![
                mk_route(rid(a, 50101, "HandleA", 1)),
                mk_route(rid(a, 50102, "HandleB", 1)),
            ],
        };
        let ce = ClassifiedEdge {
            obligation_id: crate::program::resolve::full::ObligationId::Publisher(pubr),
            edge,
        };
        let doc = build_graphify_document(&graph, &[ce], a);

        let hev: Vec<&serde_json::Value> = doc
            .hyperedges
            .iter()
            .filter(|h| h["kind"] == "event_group")
            .collect();
        assert_eq!(hev.len(), 1, "one event with 2 subscribers → one hyperedge");
        assert_eq!(
            hev[0]["nodes"].as_array().unwrap().len(),
            3,
            "hyperedge groups publisher + 2 subscribers"
        );
    }

    #[test]
    fn fragments_partition_by_object_with_stable_manifest() {
        let (g, edges, primary) = fixture();
        let fs = build_fragment_set(&g, &edges, primary);

        // One fragment per object; the calls edge lives in the caller's fragment.
        assert_eq!(fs.fragments.len(), 2);
        assert!(fs.fragments.contains_key("al:obj:myapp/codeunit/50100"));
        assert!(fs.fragments.contains_key("al:obj:myapp/codeunit/50101"));
        assert_eq!(fs.manifest.len(), 2);
        let caller = &fs.fragments["al:obj:myapp/codeunit/50100"];
        assert!(
            caller.edges.iter().any(|e| e.relation == "calls"),
            "outgoing calls edge belongs to the caller object's fragment"
        );
        // The callee fragment has the contains edge but not the calls edge.
        let callee = &fs.fragments["al:obj:myapp/codeunit/50101"];
        assert!(callee.edges.iter().all(|e| e.relation != "calls"));

        // Manifest must be run-stable (prerequisite for change detection).
        let fs2 = build_fragment_set(&g, &edges, primary);
        assert_eq!(fs.manifest, fs2.manifest, "manifest must be deterministic");
    }

    // -----------------------------------------------------------------------
    // Task 3 (sigfp-and-ambiguous-reclassification plan): the DTO mapping for
    // `ObligationOutcome::AmbiguousResolved` — pinned NOW (round-2 closer #3,
    // BINDING) even though no producer constructs this outcome yet.
    // -----------------------------------------------------------------------

    /// One GEdge per candidate route; `dispatch_shape`/`condition`/`obligation`
    /// carry the new taxonomy strings, and EVERY `AmbiguousDispatch` route
    /// carries the explicit `may_fire: true` signal so BC-Brain cannot read
    /// `fires_by_default:false`-shaped output as dead code (exactly one
    /// candidate IS guaranteed to run). `confidence: "INFERRED"`, never
    /// `"AMBIGUOUS"` — that tier is reserved for `Unknown`'s TRUE failure
    /// (see `project_edge`'s `ObligationOutcome::Unknown` arm doc); a closed,
    /// proven candidate set is not a failure.
    #[test]
    fn ambiguous_resolved_projects_one_gedge_per_candidate_with_may_fire_signal() {
        let (mut g, _edges, primary) = fixture();
        let caller = rid(primary, 50100, "Foo", 0);
        let cand_a = rid(primary, 50101, "Bar", 0);
        let cand_b = rid(primary, 50101, "Bar", 1);
        // A second overload candidate under the same callee object (params_count
        // distinguishes it here; real overloads differ by sig_fp — irrelevant to
        // this projection-shape test).
        g.routines.push(rtn(primary, 50101, "Bar", 1));
        g.routines.sort_by(|x, y| x.id.cmp(&y.id));

        let mk_route = |nid: RoutineNodeId| Route {
            target: RouteTarget::Routine(nid),
            evidence: Evidence::Source,
            conditions: vec![Condition::AmbiguousDispatch],
            witness: Witness::SourceSpan {
                file: "src/Callee.al".into(),
                span: (10, 20),
            },
            receiver_tier: None,
        };
        let edge = Edge {
            from: caller.clone(),
            site: site(caller.clone(), 99),
            kind: EdgeKind::Call,
            shape: DispatchShape::AmbiguousOverload,
            completeness: SetCompleteness::Complete,
            routes: vec![mk_route(cand_a), mk_route(cand_b)],
        };
        let ce = ClassifiedEdge {
            obligation_id: crate::program::resolve::full::ObligationId::CallSite {
                caller,
                span: edge.site.span.clone(),
                callee_fp: 99,
            },
            edge,
        };
        let doc = build_graphify_document(&g, &[ce], primary);

        let amb: Vec<&GEdge> = doc
            .edges
            .iter()
            .filter(|e| e.obligation == Some("ambiguous_resolved"))
            .collect();
        assert_eq!(amb.len(), 2, "one GEdge per candidate route");
        for e in &amb {
            assert_eq!(e.dispatch_shape, Some("ambiguous_overload"));
            assert_eq!(e.condition, Some("ambiguous_dispatch"));
            assert_eq!(
                e.may_fire,
                Some(true),
                "AmbiguousDispatch routes MUST carry the explicit may-fire signal"
            );
            assert_eq!(
                e.confidence, "INFERRED",
                "a closed candidate set is not the AMBIGUOUS true-failure tier"
            );
            assert_eq!(e.evidence, Some("source"));
        }

        // Serde-level pin: `may_fire` actually SERIALIZES for these edges (not
        // just present as a Rust struct field).
        let v = serde_json::to_value(amb[0]).unwrap();
        assert_eq!(v["may_fire"], serde_json::json!(true));
    }

    /// Negative / inertness pin: a plain pre-Task-3 resolved edge (no
    /// `AmbiguousDispatch` route anywhere) must NEVER carry the `may_fire`
    /// key — proves the new field is additive-only and every existing export
    /// shape stays byte-identical (the field is skipped, not `false`).
    #[test]
    fn may_fire_absent_on_every_pre_task3_edge_shape() {
        let (g, edges, primary) = fixture();
        let doc = build_graphify_document(&g, &edges, primary);
        assert!(
            doc.edges.iter().all(|e| e.may_fire.is_none()),
            "may_fire must stay None on every edge shape Task 3 didn't touch"
        );
        // And it must not appear in the serialized JSON at all.
        for e in &doc.edges {
            let v = serde_json::to_value(e).unwrap();
            assert!(
                v.get("may_fire").is_none(),
                "may_fire key must be OMITTED (skip_serializing_if), not null: {v}"
            );
        }
    }
}

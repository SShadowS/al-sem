//! The detector event graph's model: event symbols (one per published event),
//! subscription edges, and how a publisher routine becomes a symbol.
//!
//! Moved out of `engine::l3::event_graph` in engine-switch S4.2a (spec
//! `docs/superpowers/specs/2026-10-06-engine-switch-design.md`, G7). The builders
//! that fill it live with their engines: L3's `build_event_graph` (still used by
//! every consumer but `alsem analyze` until S6) re-exports these types.

use super::workspace::{L3Parameter, L3Routine};
use crate::program::attributes::{AttributeInfo, bool_arg, find_attribute};

/// One evidence record. The R2c surface only ever carries `{source}` (+ an optional
/// `note` on synthesized symbols).
#[derive(Debug, Clone)]
pub struct Evidence {
    pub source: String,
    pub note: Option<String>,
}

impl Evidence {
    /// Provenance: this edge was derived from STRUCTURAL/SYNTACTIC analysis (the
    /// parsed AST), as opposed to symbol-table resolution or a heuristic. The
    /// serialized `source` string is `"tree-sitter"` — a STABLE provenance LABEL
    /// retained for golden compatibility, NOT a live dependency: since the Phase 5
    /// seal the syntactic evidence comes from the owned `al-syntax` IR, and the
    /// engine no longer links tree-sitter. (Renaming the serialized label would
    /// churn the event/policy goldens for no behavior change; the label's meaning —
    /// "syntactic provenance" — is unchanged.)
    pub(crate) fn tree_sitter() -> Evidence {
        Evidence {
            source: "tree-sitter".to_string(),
            note: None,
        }
    }
    pub(crate) fn with_note(note: &str) -> Evidence {
        Evidence {
            source: "tree-sitter".to_string(),
            note: Some(note.to_string()),
        }
    }
}

/// One EventSymbol (publisher or synthesized). Ids in INTERNAL form.
#[derive(Debug, Clone)]
pub struct EventSymbol {
    /// Internal event id (`${publisherObjectId}/event/${eventName_lc}`).
    pub id: String,
    /// Internal ObjectId (conforming) for real/maybe; the sentinel string for unknown.
    pub publisher_object_id: String,
    /// Internal RoutineId of the publisher — None for synthesized symbols.
    pub publisher_routine_id: Option<String>,
    /// StableRoutineId of the publisher (projection convenience) — None when synthesized.
    pub publisher_stable_routine_id: Option<String>,
    pub event_name: String,
    pub event_kind: String,
    pub element_name: Option<String>,
    pub signature_hash: String,
    pub parameters: Vec<L3Parameter>,
    pub isolated: Option<bool>,
    pub provenance: Vec<Evidence>,
    /// The publisher when it is not a model routine (engine-switch S4.2): then
    /// `publisher_routine_id` is `None` although subscriptions to the event are
    /// bound. `None` for a model publisher and for a synthesized symbol.
    pub publisher_ref: Option<PublisherRef>,
}

/// A bound event publisher that is not a routine of the detector model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PublisherRef {
    /// A dependency routine. `target` is [`super::model_routine_key`]; `body` is
    /// the registry's body state (`None` when the registry has no entry).
    Dependency {
        target: String,
        body: Option<crate::program::registry::BodyState>,
    },
    /// The platform raises the event itself (a table's `OnAfterInsertEvent`, a
    /// page's `OnOpenPageEvent`, …); no routine declares it. `target` names the
    /// program engine's synthetic publisher.
    Platform { target: String },
}

/// One EventEdge. Ids in INTERNAL form.
#[derive(Debug, Clone)]
pub struct EventEdge {
    pub event_id: String,
    pub subscriber_routine_id: String,
    /// StableRoutineId of the subscriber (projection convenience).
    pub subscriber_stable_routine_id: String,
    pub subscriber_app_id: String,
    pub resolution: String,
    pub provenance: Vec<Evidence>,
}

/// The internal event graph.
#[derive(Debug, Clone)]
pub struct EventGraph {
    pub events: Vec<EventSymbol>,
    pub edges: Vec<EventEdge>,
}

/// Determine the event kind from a publisher routine's structured attributes.
fn publisher_event_kind(attrs: &[AttributeInfo]) -> &'static str {
    if find_attribute(attrs, "IntegrationEvent").is_some() {
        "integration"
    } else if find_attribute(attrs, "BusinessEvent").is_some() {
        "business"
    } else {
        "unknown"
    }
}

/// Parse the `Isolated` boolean. `[IntegrationEvent(.,.,Isolated)]` (index 2) /
/// `[BusinessEvent(.,Isolated)]` (index 1). Returns Some(true) only when isolated;
/// None when absent / explicit-false; conservative Some(true) when present-but-
/// unparseable (Rule 5: prefer exclusion over a false weave).
fn parse_isolated(attrs: &[AttributeInfo]) -> Option<bool> {
    if let Some(int_attr) = find_attribute(attrs, "IntegrationEvent") {
        if let Some(v) = bool_arg(int_attr, 2) {
            // explicit false → omit (None); true → Some(true).
            return if v { Some(true) } else { None };
        }
        // arg present but not a boolean literal → conservative true; absent → None.
        return if int_attr.args.get(2).is_some() {
            Some(true)
        } else {
            None
        };
    }
    if let Some(biz_attr) = find_attribute(attrs, "BusinessEvent") {
        if let Some(v) = bool_arg(biz_attr, 1) {
            return if v { Some(true) } else { None };
        }
        return if biz_attr.args.get(1).is_some() {
            Some(true)
        } else {
            None
        };
    }
    None
}

/// `encodeEventId(publisherObjectId, eventName)` — lowercases the eventName.
pub fn encode_event_id(publisher_object_id: &str, event_name: &str) -> String {
    format!("{publisher_object_id}/event/{}", event_name.to_lowercase())
}

/// Build the EventSymbol for a real publisher routine.
pub fn build_event_symbol(routine: &L3Routine) -> EventSymbol {
    let isolated = parse_isolated(&routine.attributes_parsed);
    EventSymbol {
        id: encode_event_id(&routine.object_id, &routine.name),
        publisher_object_id: routine.object_id.clone(),
        publisher_routine_id: Some(routine.id.clone()),
        publisher_stable_routine_id: Some(routine.stable_routine_id.clone()),
        event_name: routine.name.clone(),
        event_kind: publisher_event_kind(&routine.attributes_parsed).to_string(),
        element_name: None,
        signature_hash: routine.normalized_signature_hash.clone(),
        parameters: routine.parameters.clone(),
        isolated: if isolated == Some(true) {
            Some(true)
        } else {
            None
        },
        provenance: vec![Evidence::tree_sitter()],
        publisher_ref: None,
    }
}

// ---------------------------------------------------------------------------
// The detector event graph from the program engine (engine-switch S4.2, G7).
// ---------------------------------------------------------------------------

/// How [`program_event_graph`] built its graph. `unmapped_subscribers` and
/// `bound_model_missing` lose or degrade a subscription and are expected to be 0.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EventCensus {
    /// Model routines of kind `event-subscriber`.
    pub model_subscribers: usize,
    /// Model subscribers with no physical row, so no program node: their
    /// subscriptions are not in the graph.
    pub unmapped_subscribers: usize,
    /// Model subscribers whose program node has no parsed subscription (an
    /// attribute neither engine can read).
    pub subscribers_without_subscription: usize,
    pub bound_model: usize,
    pub bound_dependency: usize,
    pub bound_platform: usize,
    /// Bound to a workspace publisher that has no model publisher symbol; the edge
    /// degrades to `maybe`.
    pub bound_model_missing: usize,
    pub ambiguous: usize,
    pub orphaned: usize,
    pub object_unresolved: usize,
}

impl EventCensus {
    /// One `name\tcount` row per counter.
    pub fn lines(&self) -> Vec<String> {
        [
            ("model_subscribers", self.model_subscribers),
            ("unmapped_subscribers", self.unmapped_subscribers),
            (
                "subscribers_without_subscription",
                self.subscribers_without_subscription,
            ),
            ("bound_model", self.bound_model),
            ("bound_dependency", self.bound_dependency),
            ("bound_platform", self.bound_platform),
            ("bound_model_missing", self.bound_model_missing),
            ("ambiguous", self.ambiguous),
            ("orphaned", self.orphaned),
            ("object_unresolved", self.object_unresolved),
        ]
        .iter()
        .map(|(k, v)| format!("{k}\t{v}"))
        .collect()
    }
}

/// The detector event graph built from the program engine, and how.
#[derive(Debug, Clone)]
pub struct ProgramEvents {
    pub graph: EventGraph,
    pub census: EventCensus,
}

/// Collect the set of internal EventIds whose publisher routine carries an
/// `Isolated` event attribute (`EventSymbol.isolated === true`). Used by the
/// R4-F ordering engine (Rule 5 §0.5) to promote isolated event-dispatch links
/// to barriers. EventId form matches the witness hop's `event_id`
/// (`${publisherObjectId}/event/${eventName_lc}`).
pub fn isolated_event_ids(routines: &[L3Routine]) -> std::collections::HashSet<String> {
    let mut ids: std::collections::HashSet<String> = std::collections::HashSet::new();
    for routine in routines {
        if routine.kind != "event-publisher" {
            continue;
        }
        let symbol = build_event_symbol(routine);
        if symbol.isolated == Some(true) {
            ids.insert(symbol.id);
        }
    }
    ids
}

/// Build the detector event graph from the program engine's subscription
/// inventory (`SubscriberIndex::subscriptions`) for the model `ws`.
///
/// - Symbols for the model's publisher routines come first, exactly as L3's
///   `build_event_graph` makes them.
/// - Edges: for each model subscriber routine (model order), one edge per
///   subscription of its program node (attribute order). Bound -> `resolved`;
///   ambiguous overload -> `ambiguous`; publisher object found but no publisher
///   routine -> `maybe`; publisher object not found -> `unknown`. Only `resolved`
///   is a proven subscriber; every consumer tests `!= "resolved"`.
/// - A platform event with an element filter (a field's `OnAfterValidateEvent`,
///   an action's `OnAfterActionEvent`) is one event per element: its id gains
///   `/{element}` and the symbol carries `element_name`.
/// - A publisher that is not a model routine (a dependency routine, or the
///   platform's own table/page event) gets one symbol with
///   `publisher_routine_id: None` and a [`PublisherRef`]. Consumers that key on the
///   publisher routine (combined graph, cones, fan-out, d43, d45) therefore do not
///   change; `subscribers_by_event` gains the workspace subscribers of such events.
pub fn program_event_graph(
    ctx: &crate::program::resolve::full::ProgramContext,
    ws: &super::workspace::L3Workspace,
) -> ProgramEvents {
    use crate::engine::ids::sha256_hex;
    use crate::program::node::RoutineNodeId;
    use crate::program::registry::TargetParams;
    use crate::program::resolve::event::PublisherKind;
    use crate::program::resolve::index::{SubscriberIndex, Subscription, SubscriptionOutcome};
    use std::collections::{HashMap, HashSet};

    let graph = ctx.graph();
    let primary = ctx.primary_app_ref;
    let registry = ctx.registry();
    let index = SubscriberIndex::build(graph);
    let mut subs_by_node: HashMap<&RoutineNodeId, Vec<&Subscription>> = HashMap::new();
    for s in index.subscriptions() {
        subs_by_node.entry(&s.subscriber).or_default().push(s);
    }
    let mut node_by_span: HashMap<super::census::Key, &RoutineNodeId> = HashMap::new();
    for row in &graph.workspace_rows.routines {
        node_by_span
            .entry((row.file.clone(), row.start, row.end))
            .or_insert(&row.node);
    }
    // A cross-app model (engine-switch S7.3) also holds dependency routines; their
    // declarations come from the dependency tier (no physical rows), spelled with
    // the model's `dep:<guid>:<path>` unit.
    let model_guids: HashSet<String> = ws
        .routines
        .iter()
        .map(|r| r.app_guid.to_ascii_lowercase())
        .collect();
    let in_model = |app: crate::program::node::AppRef| {
        model_guids.contains(&graph.apps.resolve(app).guid.to_ascii_lowercase())
    };
    let surface = ctx.decl_surface();
    for r in &graph.routines {
        if r.id.object.app == primary || !in_model(r.id.object.app) {
            continue;
        }
        if let Some((meta, path)) = surface.get_with_path(&r.id) {
            let guid = &graph.apps.resolve(r.id.object.app).guid;
            node_by_span
                .entry((
                    format!("dep:{guid}:{path}"),
                    crate::program::physical::Pos::of(meta.origin.start),
                    crate::program::physical::Pos::of(meta.origin.end),
                ))
                .or_insert(&r.id);
        }
    }
    let mut routine_by_id = HashMap::new();
    for r in &graph.routines {
        routine_by_id.entry(&r.id).or_insert(r);
    }

    let mut c = EventCensus::default();
    let mut events: Vec<EventSymbol> = Vec::new();
    let mut event_ix: HashMap<String, usize> = HashMap::new();
    let mut real: HashSet<String> = HashSet::new();
    for r in &ws.routines {
        if r.kind == "event-publisher" {
            let s = build_event_symbol(r);
            real.insert(s.id.clone());
            event_ix.insert(s.id.clone(), events.len());
            events.push(s);
        }
    }
    // A symbol for an event no bound publisher stands behind (L3's synthesized
    // `maybe`/`unknown` symbols).
    fn synth(
        events: &mut Vec<EventSymbol>,
        event_ix: &mut HashMap<String, usize>,
        id: &str,
        publisher_object_id: String,
        s: &Subscription,
        note: &str,
    ) {
        if event_ix.contains_key(id) {
            return;
        }
        event_ix.insert(id.to_string(), events.len());
        events.push(EventSymbol {
            id: id.to_string(),
            publisher_object_id,
            publisher_routine_id: None,
            publisher_stable_routine_id: None,
            event_name: s.event_name_lc.clone(),
            event_kind: "unknown".to_string(),
            element_name: s.element.clone(),
            signature_hash: sha256_hex(id),
            parameters: Vec::new(),
            isolated: None,
            provenance: vec![Evidence::with_note(note)],
            publisher_ref: None,
        });
    }

    let mut edges: Vec<EventEdge> = Vec::new();
    for r in &ws.routines {
        if r.kind != "event-subscriber" {
            continue;
        }
        c.model_subscribers += 1;
        let Some(node) = node_by_span.get(&super::census::anchor_key(&r.source_anchor)) else {
            c.unmapped_subscribers += 1;
            continue;
        };
        let subs = subs_by_node.get(node).map(Vec::as_slice).unwrap_or(&[]);
        if subs.is_empty() {
            c.subscribers_without_subscription += 1;
        }
        for s in subs {
            let (event_id, resolution) = match &s.outcome {
                SubscriptionOutcome::Bound(pid) => {
                    let node = routine_by_id.get(pid);
                    let platform =
                        node.is_some_and(|n| n.publisher_kind == Some(PublisherKind::Platform));
                    let object_id = super::model_object_id(graph, &pid.object);
                    let mut id = encode_event_id(&object_id, &s.event_name_lc);
                    // A platform field or action event fires per element:
                    // `OnAfterValidateEvent` for 'No.' never fires when 'Name'
                    // is validated. Each element is its own event, so two
                    // subscribers of different fields are not co-subscribers
                    // (d44 would otherwise pair them).
                    let element = if platform { s.element.clone() } else { None };
                    if let Some(e) = &element {
                        id = format!("{id}/{e}");
                    }
                    if (pid.object.app == primary || in_model(pid.object.app)) && !platform {
                        if real.contains(&id) {
                            c.bound_model += 1;
                            (id, "resolved")
                        } else {
                            c.bound_model_missing += 1;
                            synth(
                                &mut events,
                                &mut event_ix,
                                &id,
                                object_id,
                                s,
                                "publisher not indexed",
                            );
                            (id, "maybe")
                        }
                    } else {
                        if platform {
                            c.bound_platform += 1;
                        } else {
                            c.bound_dependency += 1;
                        }
                        if !event_ix.contains_key(&id) {
                            let target = super::model_routine_key(graph, pid);
                            let dep = registry.target(pid);
                            let parameters = match dep.as_ref().map(|t| &t.params) {
                                Some(TargetParams::Known(ps)) => ps
                                    .iter()
                                    .enumerate()
                                    .map(|(i, p)| {
                                        let ty = p.ty.clone().unwrap_or_default();
                                        L3Parameter {
                                            index: i as u32,
                                            name: p.name.clone(),
                                            is_var: p.by_ref,
                                            is_record: crate::program::body::ir_walk::is_record_type_str(&ty),
                                            table_name: crate::program::body::ir_walk::parse_record_table_name(&ty),
                                            type_text: ty,
                                        }
                                    })
                                    .collect(),
                                _ => Vec::new(),
                            };
                            let (publisher_ref, note) = if platform {
                                (PublisherRef::Platform { target }, "platform event")
                            } else {
                                (
                                    PublisherRef::Dependency {
                                        target,
                                        body: dep.as_ref().map(|t| t.body),
                                    },
                                    "dependency publisher",
                                )
                            };
                            let event_kind = match node.and_then(|n| n.publisher_kind) {
                                Some(PublisherKind::Integration) => "integration",
                                Some(PublisherKind::Business) => "business",
                                Some(PublisherKind::Internal) => "internal",
                                Some(PublisherKind::Platform) => "trigger",
                                None => "unknown",
                            };
                            event_ix.insert(id.clone(), events.len());
                            events.push(EventSymbol {
                                id: id.clone(),
                                publisher_object_id: object_id,
                                publisher_routine_id: None,
                                publisher_stable_routine_id: None,
                                event_name: node
                                    .map(|n| n.name.clone())
                                    .unwrap_or_else(|| s.event_name_lc.clone()),
                                event_kind: event_kind.to_string(),
                                element_name: element,
                                signature_hash: sha256_hex(&id),
                                parameters,
                                // Not read from dependency attributes yet.
                                isolated: None,
                                provenance: vec![Evidence {
                                    source: "program".to_string(),
                                    note: Some(note.to_string()),
                                }],
                                publisher_ref: Some(publisher_ref),
                            });
                        }
                        (id, "resolved")
                    }
                }
                SubscriptionOutcome::Ambiguous {
                    publisher_object, ..
                } => {
                    c.ambiguous += 1;
                    let object_id = super::model_object_id(graph, publisher_object);
                    let id = encode_event_id(&object_id, &s.event_name_lc);
                    synth(
                        &mut events,
                        &mut event_ix,
                        &id,
                        object_id,
                        s,
                        "publisher overload ambiguous",
                    );
                    (id, "ambiguous")
                }
                SubscriptionOutcome::Orphaned { publisher_object } => {
                    c.orphaned += 1;
                    let object_id = super::model_object_id(graph, publisher_object);
                    let id = encode_event_id(&object_id, &s.event_name_lc);
                    synth(
                        &mut events,
                        &mut event_ix,
                        &id,
                        object_id,
                        s,
                        "publisher not indexed",
                    );
                    (id, "maybe")
                }
                SubscriptionOutcome::ObjectUnresolved => {
                    c.object_unresolved += 1;
                    // L3's sentinel shape: `unknown/{type}/0:{name}`; the
                    // number stands in for the name when the attribute uses one.
                    let reference = match s.publisher_id {
                        Some(n) => n.to_string(),
                        None => s.publisher_name.clone(),
                    };
                    let sentinel = format!("unknown/{}/0:{reference}", s.publisher_object_type);
                    let id = encode_event_id(&sentinel, &s.event_name_lc);
                    synth(
                        &mut events,
                        &mut event_ix,
                        &id,
                        sentinel,
                        s,
                        "target object not in indexed source",
                    );
                    (id, "unknown")
                }
            };
            edges.push(EventEdge {
                event_id,
                subscriber_routine_id: r.id.clone(),
                subscriber_stable_routine_id: r.stable_routine_id.clone(),
                subscriber_app_id: r.app_guid.clone(),
                resolution: resolution.to_string(),
                provenance: vec![Evidence::tree_sitter()],
            });
        }
    }
    ProgramEvents {
        graph: EventGraph { events, edges },
        census: c,
    }
}

// ---------------------------------------------------------------------------
// Stable projection — the golden / vector comparison surface (`tests/r2c-goldens`).
// Mirrors scripts/r2c-l3eg-projection.ts EXACTLY. Moved from
// `engine::l3::event_graph` in engine-switch S9.5e: the r2c goldens now project
// the program engine's event graph (`program_event_graph`).
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct PEvidence {
    pub source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct PParameter {
    pub index: u32,
    pub name: String,
    #[serde(rename = "typeText")]
    pub type_text: String,
    #[serde(rename = "isVar")]
    pub is_var: bool,
    #[serde(rename = "isRecord")]
    pub is_record: bool,
    #[serde(rename = "tableName", skip_serializing_if = "Option::is_none")]
    pub table_name: Option<String>,
}

/// One projected EventSymbol (stable id form). Field ORDER mirrors al-sem's
/// `projectEventSymbol` key order so a byte-level golden compare aligns:
/// id, publisherObjectId, eventName, eventKind, signatureHash, parameters,
/// provenance, then the optionals publisherRoutineId / isolated / elementName.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct PEventSymbol {
    pub id: String,
    #[serde(rename = "publisherObjectId")]
    pub publisher_object_id: String,
    #[serde(rename = "eventName")]
    pub event_name: String,
    #[serde(rename = "eventKind")]
    pub event_kind: String,
    #[serde(rename = "signatureHash")]
    pub signature_hash: String,
    pub parameters: Vec<PParameter>,
    pub provenance: Vec<PEvidence>,
    #[serde(rename = "publisherRoutineId", skip_serializing_if = "Option::is_none")]
    pub publisher_routine_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub isolated: Option<bool>,
    #[serde(rename = "elementName", skip_serializing_if = "Option::is_none")]
    pub element_name: Option<String>,
}

/// One projected EventEdge (stable id form).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct PEventEdge {
    #[serde(rename = "eventId")]
    pub event_id: String,
    #[serde(rename = "subscriberRoutineId")]
    pub subscriber_routine_id: String,
    #[serde(rename = "subscriberAppId")]
    pub subscriber_app_id: String,
    pub resolution: String,
    pub provenance: Vec<PEvidence>,
}

/// The full event-graph projection — the golden / vector document shape.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct L3EventGraphProjection {
    pub events: Vec<PEventSymbol>,
    pub edges: Vec<PEventEdge>,
}

/// `toStableEventId(publisher, eventName, signatureHash)`.
fn to_stable_event_id(publisher: &str, event_name: &str, signature_hash: &str) -> String {
    format!("{publisher}::{event_name}::{signature_hash}")
}

/// Stable event id FROM an EventSymbol (DUMB `/`→`:` on publisherObjectId; NEVER
/// parse the raw eventId). For the sentinel `unknown/type/0:ref` the dumb replace
/// yields `unknown:type:0:ref` — an opaque deterministic comparison id.
fn stable_event_id_from_symbol(sym: &EventSymbol) -> String {
    to_stable_event_id(
        &crate::engine::ids::to_stable_object_id(&sym.publisher_object_id),
        &sym.event_name,
        &sym.signature_hash,
    )
}

fn project_parameter(p: &L3Parameter) -> PParameter {
    PParameter {
        index: p.index,
        name: p.name.clone(),
        type_text: p.type_text.clone(),
        is_var: p.is_var,
        is_record: p.is_record,
        table_name: p.table_name.clone(),
    }
}

fn project_evidence(e: &Evidence) -> PEvidence {
    PEvidence {
        source: e.source.clone(),
        note: e.note.clone(),
    }
}

fn project_event_symbol(sym: &EventSymbol) -> PEventSymbol {
    PEventSymbol {
        id: stable_event_id_from_symbol(sym),
        publisher_object_id: crate::engine::ids::to_stable_object_id(&sym.publisher_object_id),
        event_name: sym.event_name.clone(),
        event_kind: sym.event_kind.clone(),
        signature_hash: sym.signature_hash.clone(),
        parameters: sym.parameters.iter().map(project_parameter).collect(),
        provenance: sym.provenance.iter().map(project_evidence).collect(),
        publisher_routine_id: sym.publisher_stable_routine_id.clone(),
        isolated: if sym.isolated == Some(true) {
            Some(true)
        } else {
            None
        },
        element_name: sym.element_name.clone(),
    }
}

/// Project an internal `EventGraph` to the stable event-graph projection.
/// Events sorted by stable id (byte order, al-sem `cmpStable`); edges by (stable
/// eventId, subscriberRoutineId). The edge eventId is mapped THROUGH the
/// rawEventId→stableEventId map (LAST-wins on raw-id collision); a missing
/// mapping keeps the raw id so a divergence is VISIBLE.
pub fn project_event_graph(graph: &EventGraph) -> L3EventGraphProjection {
    // rawEventId → stableEventId (walk events[] in emitted order, LAST-wins).
    let mut raw_to_stable: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    for sym in &graph.events {
        raw_to_stable.insert(sym.id.clone(), stable_event_id_from_symbol(sym));
    }

    let mut events: Vec<PEventSymbol> = graph.events.iter().map(project_event_symbol).collect();
    events.sort_by(|a, b| a.id.cmp(&b.id));

    let mut edges: Vec<PEventEdge> = graph
        .edges
        .iter()
        .map(|edge| {
            let stable_event_id = raw_to_stable
                .get(&edge.event_id)
                .cloned()
                .unwrap_or_else(|| edge.event_id.clone());
            PEventEdge {
                event_id: stable_event_id,
                subscriber_routine_id: edge.subscriber_stable_routine_id.clone(),
                subscriber_app_id: edge.subscriber_app_id.clone(),
                resolution: edge.resolution.clone(),
                provenance: edge.provenance.iter().map(project_evidence).collect(),
            }
        })
        .collect();
    edges.sort_by(|a, b| {
        a.event_id
            .cmp(&b.event_id)
            .then_with(|| a.subscriber_routine_id.cmp(&b.subscriber_routine_id))
    });

    L3EventGraphProjection { events, edges }
}

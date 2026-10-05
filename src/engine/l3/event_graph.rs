//! L3 EVENT GRAPH (R2c Task 2) — Rust port of al-sem's FIXED `buildEventGraph`
//! (`src/resolve/event-graph.ts`, al-sem ≥9eb9c55 / summarySchema 32) + the stable
//! projection (`scripts/r2c-l3eg-projection.ts`).
//!
//! Two layers:
//!   1. `build_event_graph` — the INTERNAL event graph (EventSymbol[] + EventEdge[]),
//!      in al-sem's exact iteration order and with the FIXED open-world semantics.
//!   2. `project_event_graph` — projects the internal graph to the STABLE id form
//!      the R2c vectors / goldens carry (the differential comparison surface).
//!
//! === The FIXED semantics (Rev 2 MUST-FIX #1) ===
//! A subscriber is `resolved` IFF a REAL indexed event-publisher routine produces
//! its eventId — tracked in `real_publisher_event_ids`, NEVER in `event_by_id`
//! (which also holds synthesized "maybe"/"unknown" symbols). Consulting `event_by_id`
//! for resolution would falsely upgrade the 2nd+ subscriber to an unindexed event to
//! `resolved` (the 6th al-sem oracle bug). `event_by_id` drives ONLY dedup of
//! synthesized symbols.
//!
//! Three-case subscriber synthesis:
//!   - target found + real publisher  → `resolved` (no synthesis)
//!   - target found + NO real publisher → `maybe`   + synthesize (conforming objectId)
//!   - target NOT found                → `unknown`  + synthesize (sentinel pseudo-id)
//!
//! Synthesized `signatureHash = sha256_hex(RAW eventId)`. `encode_event_id`
//! lowercases the eventName. Vec output order = routine-iteration order (publishers
//! then subscribers); maps for LOOKUP only.

use std::collections::{HashMap, HashSet};

use super::l3_workspace::{L3Parameter, L3Resolved, L3Routine};
use super::symbol_table::SymbolTable;
use crate::engine::ids::{sha256_hex, to_stable_object_id};
use crate::program::attributes::{
    AttributeInfo, bool_arg, find_attribute, qualified_arg, string_arg,
};

// ---------------------------------------------------------------------------
// Internal event-graph model (NOT the serde projection shape).
// ---------------------------------------------------------------------------

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
    fn tree_sitter() -> Evidence {
        Evidence {
            source: "tree-sitter".to_string(),
            note: None,
        }
    }
    fn with_note(note: &str) -> Evidence {
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

// ---------------------------------------------------------------------------
// Attribute helpers (port of publisherEventKind / parseIsolated /
// parseSubscriberAttribute).
// ---------------------------------------------------------------------------

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

/// Parsed `[EventSubscriber(...)]` target parts.
struct SubscriberTarget {
    target_object_type: String,
    target_ref: String,
    event_name: String,
    element_name: String,
}

/// Read an `[EventSubscriber(ObjectType::X, X::"Y", 'EventName', 'ElementName', ...)]`
/// attribute's target parts, or None if absent / not parseable.
fn parse_subscriber_attribute(attrs: &[AttributeInfo]) -> Option<SubscriberTarget> {
    let attr = find_attribute(attrs, "EventSubscriber")?;
    let object_type_arg = qualified_arg(attr, 0);
    let target_ref_arg = qualified_arg(attr, 1);
    let event_name = string_arg(attr, 2);
    let element_name = string_arg(attr, 3);
    let (Some(object_type_arg), Some(target_ref_arg), Some(event_name)) =
        (object_type_arg, target_ref_arg, event_name)
    else {
        return None;
    };
    Some(SubscriberTarget {
        target_object_type: object_type_arg.member,
        target_ref: target_ref_arg.member,
        event_name,
        element_name: element_name.unwrap_or_default(),
    })
}

/// `encodeEventId(publisherObjectId, eventName)` — lowercases the eventName.
fn encode_event_id(publisher_object_id: &str, event_name: &str) -> String {
    format!("{publisher_object_id}/event/{}", event_name.to_lowercase())
}

/// `encodeObjectId(appGuid, objectType, objectNumber)`.
fn encode_object_id(app_guid: &str, object_type: &str, object_number: i64) -> String {
    format!("{app_guid}/{object_type}/{object_number}")
}

/// Build the EventSymbol for a real publisher routine.
fn build_event_symbol(routine: &L3Routine) -> EventSymbol {
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
    }
}

// ---------------------------------------------------------------------------
// build_event_graph — the FIXED open-world semantics.
// ---------------------------------------------------------------------------

/// Build the event graph: EventSymbols from publisher routines, EventEdges from
/// subscriber routines (open-world: every parseable subscriber → an edge, never a
/// silent gap). Iterates `routines` once for publishers, once for subscribers; Vecs
/// in iteration order, maps for lookup/dedup only.
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

pub fn build_event_graph(routines: &[L3Routine], symbols: &SymbolTable) -> EventGraph {
    let mut events: Vec<EventSymbol> = Vec::new();
    let mut event_by_id: HashMap<String, usize> = HashMap::new();
    // Event ids backed by a REAL indexed event-publisher routine — the ONLY set that
    // may drive a `resolved` decision.
    let mut real_publisher_event_ids: HashSet<String> = HashSet::new();

    // objectId → appGuid, so a subscriber routine maps back to its owning app.
    // (routine.app_guid already carries this; kept for parity with al-sem's lookup.)

    // --- publishers ---
    for routine in routines {
        if routine.kind != "event-publisher" {
            continue;
        }
        let symbol = build_event_symbol(routine);
        let id = symbol.id.clone();
        real_publisher_event_ids.insert(id.clone());
        event_by_id.insert(id, events.len());
        events.push(symbol);
    }

    // --- subscribers ---
    let mut edges: Vec<EventEdge> = Vec::new();
    for routine in routines {
        if routine.kind != "event-subscriber" {
            continue;
        }
        let Some(target) = parse_subscriber_attribute(&routine.attributes_parsed) else {
            continue;
        };

        // routine.app_guid is the subscriber's owning app guid.
        let subscriber_app_id = routine.app_guid.clone();

        // Resolve the target object by type + name (case-insensitive).
        let target_object =
            symbols.object_by_type_name(&target.target_object_type, &target.target_ref);

        let event_id: String;
        let resolution: &'static str;

        if let Some(target_object) = target_object {
            let id = encode_event_id(&target_object.id, &target.event_name);
            if real_publisher_event_ids.contains(&id) {
                resolution = "resolved";
            } else {
                resolution = "maybe";
                // Synthesize a "maybe" symbol; dedup against event_by_id so a 2nd+
                // subscriber to the same unindexed event reuses it (stays "maybe").
                if !event_by_id.contains_key(&id) {
                    let sig = sha256_hex(&id);
                    let symbol = EventSymbol {
                        id: id.clone(),
                        publisher_object_id: target_object.id.clone(),
                        publisher_routine_id: None,
                        publisher_stable_routine_id: None,
                        event_name: target.event_name.clone(),
                        event_kind: "unknown".to_string(),
                        element_name: if target.element_name.is_empty() {
                            None
                        } else {
                            Some(target.element_name.clone())
                        },
                        signature_hash: sig,
                        parameters: Vec::new(),
                        isolated: None,
                        provenance: vec![Evidence::with_note("publisher not indexed")],
                    };
                    event_by_id.insert(id.clone(), events.len());
                    events.push(symbol);
                }
            }
            event_id = id;
        } else {
            // Target not in indexed source — synthesize a pseudo object id. The
            // `${pseudoObjectId}:${targetRef}` is a NON-conforming sentinel string.
            let pseudo_object_id = encode_object_id("unknown", &target.target_object_type, 0);
            let sentinel = format!("{pseudo_object_id}:{}", target.target_ref);
            let id = encode_event_id(&sentinel, &target.event_name);
            resolution = "unknown";
            if !event_by_id.contains_key(&id) {
                let sig = sha256_hex(&id);
                let symbol = EventSymbol {
                    id: id.clone(),
                    publisher_object_id: sentinel,
                    publisher_routine_id: None,
                    publisher_stable_routine_id: None,
                    event_name: target.event_name.clone(),
                    event_kind: "unknown".to_string(),
                    element_name: if target.element_name.is_empty() {
                        None
                    } else {
                        Some(target.element_name.clone())
                    },
                    signature_hash: sig,
                    parameters: Vec::new(),
                    isolated: None,
                    provenance: vec![Evidence::with_note("target object not in indexed source")],
                };
                event_by_id.insert(id.clone(), events.len());
                events.push(symbol);
            }
            event_id = id;
        }

        edges.push(EventEdge {
            event_id,
            subscriber_routine_id: routine.id.clone(),
            subscriber_stable_routine_id: routine.stable_routine_id.clone(),
            subscriber_app_id,
            resolution: resolution.to_string(),
            provenance: vec![Evidence::tree_sitter()],
        });
    }

    EventGraph { events, edges }
}

// ---------------------------------------------------------------------------
// Stable projection — the golden / vector comparison surface.
// Mirrors scripts/r2c-l3eg-projection.ts EXACTLY.
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

/// The full L3 event-graph projection — the golden / vector document shape.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct L3EventGraphProjection {
    pub events: Vec<PEventSymbol>,
    pub edges: Vec<PEventEdge>,
}

/// Byte-order string compare (al-sem `cmpStable`).
fn cmp_stable(a: &str, b: &str) -> std::cmp::Ordering {
    a.cmp(b)
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
        &to_stable_object_id(&sym.publisher_object_id),
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
        publisher_object_id: to_stable_object_id(&sym.publisher_object_id),
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

/// Project an internal `EventGraph` to the stable L3 event-graph projection.
/// Events sorted by stable id; edges by (stable eventId, subscriberRoutineId). The
/// edge eventId is mapped THROUGH the rawEventId→stableEventId map (LAST-wins on
/// raw-id collision); a missing mapping keeps the raw id so a divergence is VISIBLE.
pub fn project_event_graph(graph: &EventGraph) -> L3EventGraphProjection {
    // rawEventId → stableEventId (walk events[] in emitted order, LAST-wins).
    let mut raw_to_stable: HashMap<String, String> = HashMap::new();
    for sym in &graph.events {
        raw_to_stable.insert(sym.id.clone(), stable_event_id_from_symbol(sym));
    }

    let mut events: Vec<PEventSymbol> = graph.events.iter().map(project_event_symbol).collect();
    events.sort_by(|a, b| cmp_stable(&a.id, &b.id));

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
        cmp_stable(&a.event_id, &b.event_id)
            .then_with(|| cmp_stable(&a.subscriber_routine_id, &b.subscriber_routine_id))
    });

    L3EventGraphProjection { events, edges }
}

impl L3Resolved {
    /// Build the workspace event graph and project it to the golden / vector stable
    /// shape. Builds the symbol table ONCE over the resolved workspace.
    pub fn project_event_graph(&self) -> L3EventGraphProjection {
        let ws = &self.workspace;
        let symbols = SymbolTable::build(&ws.objects, &ws.tables, &ws.routines);
        let graph = build_event_graph(&ws.routines, &symbols);
        project_event_graph(&graph)
    }
}

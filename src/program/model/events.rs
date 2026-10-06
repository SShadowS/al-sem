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
    }
}

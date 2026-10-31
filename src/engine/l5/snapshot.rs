//! R4-F Stage-2b — `CapabilitySnapshot` CONSUMED-CORE, byte-parity port of
//! al-sem's `composeSnapshot` (the ordering-facts subset digestQuery consumes).
//!
//! This module is LARGELY A RE-PROJECTION of the already-byte-parity R3a
//! substrate (`engine::l4::capability_cone::build_r3a3_source_only_base`): the
//! cone facts, the combined graph's typed edges, the event graph, the L2 op/
//! callsite features, coverage, and the Stage-0 root classifications. The
//! derivers RESHAPE + rewrite-to-stable + sort — they do NOT re-derive facts,
//! edges, or summaries.
//!
//! ## Consumed-core field set (al-sem `scripts/r4f-snapshot-projection.ts`)
//!
//! KEEP: identities, capabilityFacts, typedEdges (incl. edgeId), operationIndex,
//! callsiteIndex, callsiteResolutions, analysisGaps, coverage, eventDeclarations,
//! rootClassifications, routineOrderFrames. The DROP fields (schemaVersion,
//! alsemVersion, workspaceFingerprint, generatedAt, apps, contractFacts,
//! schemaFacts, permissionFacts, inputs, inputsMetadata) are dead — never built.
//!
//! ## Determinism (R4-F spec Rev 2)
//!
//! - M1: no `HashMap`/`HashSet` iteration reaches an emitted array / sort key /
//!   hash input. Every output `Vec` is explicitly `sort_by`'d; lookups go through
//!   `HashMap` but emission iterates a sorted `Vec` or the routine list.
//! - M8 (sort contract): every sort site in this module is ORDINAL (`str::cmp` /
//!   `BTreeMap` key order), and that is the contract. It once rested on a claim that
//!   ordinal equals al-sem's `String.localeCompare` for these keys; al-sem is retired
//!   and that parity is no longer a goal. The claim is also no longer true: a
//!   numberless object's key is `@` + a percent-encoded name (`%XX`,
//!   `engine::ids::object_key`), and `@` sorts AFTER a digit ordinally but before it
//!   under `locale_compare`, so `…:Codeunit:1` < `…:Codeunit:@a` here while the
//!   full snapshot's contracts (`snapshot_full.rs`, `locale_compare`) list `…:@a`
//!   first. Each output keeps its own order; nothing relies on the two agreeing
//!   (pinned by `cli_numberless_object_ids`'s T7). A key that holds a stable object
//!   or routine id is not single-case: a numberless object's segment carries `@` and
//!   uppercase `%XX`.
//! - M9: the edge-id present-only predicate is exact (`Option::is_some`; an empty
//!   string IS kept).

use std::collections::HashMap;

use serde::Serialize;

use crate::engine::ids::{object_id_app_guid, to_stable_object_id};
use crate::engine::l4::capability_cone::{
    CapabilityExtra, CapabilityFact, CoverageRecord, R3a3SourceBase, ValueSource,
    build_r3a3_source_only_base,
};
use crate::engine::l4::combined_graph::TypedEdge;
use crate::program::body::features::{PAnchor, PCallSite, PCallee, POperationSite};
use crate::program::body::operation_order::{OperationOrder, ScopeFrame, apply_operation_order};
use crate::program::model::calls::CallEdge;
use crate::program::model::events::{EventGraph, EventSymbol};
use crate::program::model::taxonomy::DispatchKind;
use crate::program::model::workspace::{Model, ModelRoutine};

// ===========================================================================
// ORDERED serde projections of ValueSource / CapabilityExtra.
//
// CRITICAL: serde_json's `preserve_order` is NOT active for the lib/test target
// (it is only enabled via a tree-sitter BUILD dependency, a separate feature
// graph). A `serde_json::Value::Object` therefore re-sorts keys alphabetically on
// serialize — which would scramble the per-kind field order the golden carries.
// So the snapshot uses `#[derive(Serialize)]` ordered types everywhere (fields
// serialize in DECLARATION order regardless of preserve_order). Internal ids are
// preserved verbatim (the snapshot rewrites ONLY `subject` + endpoints).
// ===========================================================================

/// Ordered `ValueSource` projection — internal table-field id kept verbatim.
/// al-sem field order per variant. Untagged-by-`kind` discriminant.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(tag = "kind")]
pub enum SnapValueSource {
    #[serde(rename = "literal")]
    Literal { value: String },
    #[serde(rename = "enum")]
    Enum {
        #[serde(rename = "enumName")]
        enum_name: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        member: Option<String>,
    },
    #[serde(rename = "constant-var")]
    ConstantVar {
        #[serde(rename = "varName")]
        var_name: String,
        initializer: Box<SnapValueSource>,
    },
    #[serde(rename = "parameter")]
    Parameter {
        index: u32,
        #[serde(rename = "varName")]
        var_name: String,
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

fn snap_value_source(vs: &ValueSource) -> SnapValueSource {
    match vs {
        ValueSource::Literal { value } => SnapValueSource::Literal {
            value: value.clone(),
        },
        ValueSource::Enum { enum_name, member } => SnapValueSource::Enum {
            enum_name: enum_name.clone(),
            member: member.clone(),
        },
        ValueSource::ConstantVar {
            var_name,
            initializer,
        } => SnapValueSource::ConstantVar {
            var_name: var_name.clone(),
            initializer: Box::new(snap_value_source(initializer)),
        },
        ValueSource::Parameter { index, var_name } => SnapValueSource::Parameter {
            index: *index,
            var_name: var_name.clone(),
        },
        ValueSource::TableField {
            table_id,
            field_name,
        } => SnapValueSource::TableField {
            table_id: table_id.clone(),
            field_name: field_name.clone(),
        },
        ValueSource::Expression => SnapValueSource::Expression,
        ValueSource::Unknown => SnapValueSource::Unknown,
    }
}

/// Ordered `CapabilityExtra` projection. al-sem `model/capability.ts` field order
/// per kind. Internal recordVariableId kept verbatim.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind")]
pub enum SnapCapabilityExtra {
    // Field order = al-sem model capability fact extraction object order:
    // kind, opSubtype, recordVariableId, tempState (verified against the golden;
    // NOT the TS interface declaration order, which lists recordVariableId first).
    #[serde(rename = "table")]
    Table {
        #[serde(rename = "opSubtype", skip_serializing_if = "Option::is_none")]
        op_subtype: Option<String>,
        #[serde(rename = "recordVariableId", skip_serializing_if = "Option::is_none")]
        record_variable_id: Option<String>,
        #[serde(rename = "tempState", skip_serializing_if = "Option::is_none")]
        temp_state: Option<SnapTempState>,
    },
    #[serde(rename = "dispatch")]
    Dispatch {
        #[serde(rename = "objectType")]
        object_type: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        modal: Option<bool>,
    },
    #[serde(rename = "event")]
    Event {
        #[serde(rename = "eventClass")]
        event_class: String,
        #[serde(rename = "includeSender", skip_serializing_if = "Option::is_none")]
        include_sender: Option<bool>,
    },
    #[serde(rename = "http")]
    Http {
        method: String,
        #[serde(rename = "bodyArgSource", skip_serializing_if = "Option::is_none")]
        body_arg_source: Option<SnapValueSource>,
    },
    #[serde(rename = "storage")]
    Storage {
        #[serde(rename = "keyArgSource", skip_serializing_if = "Option::is_none")]
        key_arg_source: Option<SnapValueSource>,
        #[serde(rename = "valueArgSource", skip_serializing_if = "Option::is_none")]
        value_arg_source: Option<SnapValueSource>,
        #[serde(skip_serializing_if = "Option::is_none")]
        scope: Option<String>,
    },
}

/// Ordered temp-state projection. al-sem: `{kind:"known",value}` |
/// `{kind:"parameter-dependent",parameterIndex}` | `{kind:"unknown"}`.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind")]
pub enum SnapTempState {
    #[serde(rename = "known")]
    Known { value: bool },
    #[serde(rename = "parameter-dependent")]
    ParameterDependent {
        #[serde(rename = "parameterIndex")]
        parameter_index: u32,
    },
    #[serde(rename = "unknown")]
    Unknown,
}

impl SnapTempState {
    /// Projection onto `l2::features::known_temp_suppresses` (#34): `Some(value)`
    /// when known, `None` for parameter-dependent or unknown.
    #[must_use]
    pub fn known_value(&self) -> Option<bool> {
        match self {
            SnapTempState::Known { value } => Some(*value),
            _ => None,
        }
    }
}

fn snap_temp_state(ts: &crate::program::body::features::PTempState) -> SnapTempState {
    match ts.kind.as_str() {
        "known" => SnapTempState::Known {
            value: ts.value.unwrap_or(false),
        },
        "parameter-dependent" => SnapTempState::ParameterDependent {
            parameter_index: ts.parameter_index.unwrap_or(0),
        },
        _ => SnapTempState::Unknown,
    }
}

fn snap_capability_extra(e: &CapabilityExtra) -> SnapCapabilityExtra {
    match e {
        CapabilityExtra::Table {
            record_variable_id,
            temp_state,
            op_subtype,
        } => SnapCapabilityExtra::Table {
            record_variable_id: record_variable_id.clone(),
            temp_state: temp_state.as_ref().map(snap_temp_state),
            op_subtype: op_subtype.clone(),
        },
        CapabilityExtra::Dispatch { object_type, modal } => SnapCapabilityExtra::Dispatch {
            object_type: object_type.clone(),
            modal: *modal,
        },
        CapabilityExtra::Event {
            event_class,
            include_sender,
        } => SnapCapabilityExtra::Event {
            event_class: event_class.clone(),
            include_sender: *include_sender,
        },
        CapabilityExtra::Http {
            method,
            body_arg_source,
        } => SnapCapabilityExtra::Http {
            method: method.clone(),
            body_arg_source: body_arg_source.as_ref().map(snap_value_source),
        },
        CapabilityExtra::Storage {
            key_arg_source,
            value_arg_source,
            scope,
        } => SnapCapabilityExtra::Storage {
            key_arg_source: key_arg_source.as_ref().map(snap_value_source),
            value_arg_source: value_arg_source.as_ref().map(snap_value_source),
            scope: scope.clone(),
        },
    }
}

// ===========================================================================
// SourceAnchor (snapshot form). al-sem `model/identity.ts` `SourceAnchor`:
//   { sourceUnitId, range:{startLine,startColumn,endLine,endColumn},
//     enclosingRoutineId, syntaxKind }. The optional hash fields are undefined
//   in Phase 1 (never serialized). enclosingRoutineId is the INTERNAL routine id
//   (modelInstanceId-pinned "r0/<hash>"), reconstructed from the owning routine.
// ===========================================================================

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SnapshotRange {
    #[serde(rename = "startLine")]
    pub start_line: u32,
    #[serde(rename = "startColumn")]
    pub start_column: u32,
    #[serde(rename = "endLine")]
    pub end_line: u32,
    #[serde(rename = "endColumn")]
    pub end_column: u32,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SnapshotSourceAnchor {
    #[serde(rename = "sourceUnitId")]
    pub source_unit_id: String,
    pub range: SnapshotRange,
    #[serde(rename = "enclosingRoutineId")]
    pub enclosing_routine_id: String,
    #[serde(rename = "syntaxKind")]
    pub syntax_kind: String,
}

/// Build a snapshot `SourceAnchor` from an L2 `PAnchor` (which drops
/// enclosingRoutineId) + the owning routine's INTERNAL id. The op/callsite/
/// routine anchor's enclosing routine IS the routine it lives in.
fn anchor_from_panchor(a: &PAnchor, enclosing_routine_id: &str) -> SnapshotSourceAnchor {
    SnapshotSourceAnchor {
        source_unit_id: a.source_unit_id.clone(),
        range: SnapshotRange {
            start_line: a.start_line,
            start_column: a.start_column,
            end_line: a.end_line,
            end_column: a.end_column,
        },
        enclosing_routine_id: enclosing_routine_id.to_string(),
        syntax_kind: a.syntax_kind.clone(),
    }
}

// ===========================================================================
// Consumed-core sub-types — serde field order MIRRORS the al-sem golden EXACTLY.
// ===========================================================================

/// Interning table — parallel stableIds[] / displayNames[], sorted by stableId.
#[derive(Debug, Clone, Default, Serialize)]
pub struct SnapshotIdentityTable {
    #[serde(rename = "stableIds")]
    pub stable_ids: Vec<String>,
    #[serde(rename = "displayNames")]
    pub display_names: Vec<String>,
}

/// One capability fact — the RAW model fact (internal ids preserved) with
/// `subject` rewritten to the StableRoutineId.
///
/// Field order is DYNAMIC per fact, matching al-sem's object-construction order
/// (`composeSnapshot` does `{...f, subject}`, preserving the source key order;
/// the source order differs per capability EXTRACTOR + per provenance):
///   - HEAD (always): subject, op, resourceKind, [resourceId], [resourceArgSource],
///     confidence, provenance, via.
///   - TAIL — depends on the WITNESS the extractor used + provenance:
///       * op-witness facts (table/commit/error — carry witnessOperationId):
///         `witnessOperationId, [extra]` then `[witnessCallsiteId]` (inherited).
///       * event facts (publish/subscribe — direct carries NO witness, extra last):
///         `[extra]` then `[witnessCallsiteId]` (inherited).
///       * callsite-witness facts (http/ui/dispatch — carry witnessCallsiteId):
///         `[witnessCallsiteId], [extra]`.
///
/// al-sem inherited = `{...rep, provenance, via, witnessCallsiteId}` — the new
/// `witnessCallsiteId` key is APPENDED LAST when the rep had no witnessCallsiteId.
/// A custom `Serialize` (below) emits the keys in this exact order.
#[derive(Debug, Clone)]
pub struct SnapshotCapabilityFact {
    pub subject: String,
    pub op: String,
    pub resource_kind: String,
    pub resource_id: Option<String>,
    pub resource_arg_source: Option<SnapValueSource>,
    pub confidence: String,
    pub provenance: String,
    pub via: String,
    pub witness_operation_id: Option<String>,
    pub witness_callsite_id: Option<String>,
    pub extra: Option<SnapCapabilityExtra>,
    /// #96 D5: the firing operation of an inherited fact whose first hop is an
    /// `implicit-trigger` edge (`CapabilityFact::witness_first_hop_operation_id`),
    /// `None` otherwise. Serialized last, only when present.
    pub witness_first_hop_operation_id: Option<String>,
    /// #98 D1: the INTERNAL event id of an inherited fact whose first hop is
    /// an `event-dispatch` edge (`CapabilityFact::witness_first_hop_event_id`),
    /// `None` otherwise. Serialized last, only when present.
    pub witness_first_hop_event_id: Option<String>,
}

impl SnapshotCapabilityFact {
    /// The snapshot twin of `l4::cone_derived::fact_is_known_temp` (#20): the
    /// same rule, `l2::features::known_temp_suppresses`, reached through this
    /// carrier's projection. True only for a table fact whose temp state is
    /// exactly known/true.
    #[must_use]
    pub fn is_known_temp(&self) -> bool {
        match &self.extra {
            Some(SnapCapabilityExtra::Table { temp_state, .. }) => {
                crate::program::body::features::known_temp_suppresses(
                    temp_state.as_ref().and_then(SnapTempState::known_value),
                )
            }
            _ => false,
        }
    }
}

impl Serialize for SnapshotCapabilityFact {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut map = serializer.serialize_map(None)?;
        // HEAD.
        map.serialize_entry("subject", &self.subject)?;
        map.serialize_entry("op", &self.op)?;
        map.serialize_entry("resourceKind", &self.resource_kind)?;
        if let Some(rid) = &self.resource_id {
            map.serialize_entry("resourceId", rid)?;
        }
        if let Some(ras) = &self.resource_arg_source {
            map.serialize_entry("resourceArgSource", ras)?;
        }
        map.serialize_entry("confidence", &self.confidence)?;
        map.serialize_entry("provenance", &self.provenance)?;
        map.serialize_entry("via", &self.via)?;

        // TAIL — per al-sem extractor construction order (see the struct doc).
        let is_event = self.resource_kind == "event";
        if let Some(wo) = &self.witness_operation_id {
            // op-witness family (table/commit/error): witnessOperationId, [extra],
            // then [witnessCallsiteId] (appended last when inherited added it).
            map.serialize_entry("witnessOperationId", wo)?;
            if let Some(extra) = &self.extra {
                map.serialize_entry("extra", extra)?;
            }
            if let Some(wc) = &self.witness_callsite_id {
                map.serialize_entry("witnessCallsiteId", wc)?;
            }
        } else if is_event {
            // event family: extra first (direct has no witness), then
            // witnessCallsiteId last (inherited only).
            if let Some(extra) = &self.extra {
                map.serialize_entry("extra", extra)?;
            }
            if let Some(wc) = &self.witness_callsite_id {
                map.serialize_entry("witnessCallsiteId", wc)?;
            }
        } else {
            // callsite-witness family (http/ui/dispatch): witnessCallsiteId, extra.
            if let Some(wc) = &self.witness_callsite_id {
                map.serialize_entry("witnessCallsiteId", wc)?;
            }
            if let Some(extra) = &self.extra {
                map.serialize_entry("extra", extra)?;
            }
        }
        if let Some(op) = &self.witness_first_hop_operation_id {
            map.serialize_entry("witnessFirstHopOperationId", op)?;
        }
        if let Some(ev) = &self.witness_first_hop_event_id {
            map.serialize_entry("witnessFirstHopEventId", ev)?;
        }
        map.end()
    }
}

/// A typed edge in snapshot form — the full `GraphEdge` discriminated union (per
/// kind, with anchors) + the deterministic `edgeId`. Each variant carries its own
/// per-kind field order (al-sem `model/graph-edge.ts` declaration order) with
/// `edgeId` last. Serialized as an ORDERED struct (NOT a `serde_json::Value`, so
/// the per-kind field order survives even without serde_json `preserve_order`).
#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum SnapshotGraphEdge {
    DirectCall {
        kind: &'static str,
        #[serde(rename = "callsiteId")]
        callsite_id: String,
        from: String,
        to: String,
        #[serde(rename = "sourceAnchor")]
        source_anchor: SnapshotSourceAnchor,
        #[serde(rename = "edgeId")]
        edge_id: String,
    },
    VariableTypedCall {
        kind: &'static str,
        #[serde(rename = "callsiteId")]
        callsite_id: String,
        from: String,
        to: String,
        #[serde(rename = "receiverType")]
        receiver_type: String,
        #[serde(rename = "sourceAnchor")]
        source_anchor: SnapshotSourceAnchor,
        #[serde(rename = "edgeId")]
        edge_id: String,
    },
    InterfaceDispatch {
        kind: &'static str,
        #[serde(rename = "callsiteId")]
        callsite_id: String,
        from: String,
        to: String,
        #[serde(rename = "interfaceName")]
        interface_name: String,
        #[serde(rename = "candidateCount")]
        candidate_count: usize,
        #[serde(rename = "sourceAnchor")]
        source_anchor: SnapshotSourceAnchor,
        #[serde(rename = "edgeId")]
        edge_id: String,
    },
    ObjectRunResolved {
        kind: &'static str,
        #[serde(rename = "callsiteId")]
        callsite_id: String,
        from: String,
        to: String,
        #[serde(rename = "targetObject")]
        target_object: String,
        #[serde(rename = "objectType")]
        object_type: String,
        #[serde(rename = "sourceAnchor")]
        source_anchor: SnapshotSourceAnchor,
        #[serde(rename = "edgeId")]
        edge_id: String,
    },
    ObjectRunUnresolved {
        kind: &'static str,
        #[serde(rename = "callsiteId")]
        callsite_id: String,
        from: String,
        #[serde(rename = "targetObject", skip_serializing_if = "Option::is_none")]
        target_object: Option<String>,
        #[serde(rename = "targetIdSource")]
        target_id_source: SnapValueSource,
        #[serde(rename = "objectType")]
        object_type: String,
        #[serde(rename = "sourceAnchor")]
        source_anchor: SnapshotSourceAnchor,
        #[serde(rename = "edgeId")]
        edge_id: String,
    },
    EventDispatch {
        kind: &'static str,
        from: String,
        to: String,
        #[serde(rename = "eventId")]
        event_id: String,
        #[serde(rename = "publishAnchor")]
        publish_anchor: SnapshotSourceAnchor,
        #[serde(rename = "subscriberAnchor")]
        subscriber_anchor: SnapshotSourceAnchor,
        #[serde(rename = "edgeId")]
        edge_id: String,
    },
    /// #96 D4: a record operation (`T.Validate`, `T.Insert(true)`, ...) firing a
    /// table or field trigger. It has no call site (`callsite_id()` is `None`);
    /// its identity is the firing operation. `anchor` is that operation's source
    /// anchor, `None` when the operation is not in a workspace routine's body
    /// (the edge is kept anyway, so the snapshot never lacks an edge the cone
    /// follows).
    ImplicitTrigger {
        kind: &'static str,
        from: String,
        to: String,
        #[serde(rename = "operationId")]
        operation_id: String,
        #[serde(rename = "triggerKind")]
        trigger_kind: String,
        fires: crate::engine::l4::combined_graph::TriggerFires,
        #[serde(rename = "sourceAnchor", skip_serializing_if = "Option::is_none")]
        anchor: Option<SnapshotSourceAnchor>,
        #[serde(rename = "edgeId")]
        edge_id: String,
    },
}

impl SnapshotGraphEdge {
    /// Sort key components (kind|from|to|edgeId) — al-sem `edgeKey`.
    fn sort_key(&self) -> String {
        match self {
            SnapshotGraphEdge::DirectCall {
                kind,
                from,
                to,
                edge_id,
                ..
            }
            | SnapshotGraphEdge::VariableTypedCall {
                kind,
                from,
                to,
                edge_id,
                ..
            }
            | SnapshotGraphEdge::InterfaceDispatch {
                kind,
                from,
                to,
                edge_id,
                ..
            }
            | SnapshotGraphEdge::ObjectRunResolved {
                kind,
                from,
                to,
                edge_id,
                ..
            }
            | SnapshotGraphEdge::EventDispatch {
                kind,
                from,
                to,
                edge_id,
                ..
            }
            | SnapshotGraphEdge::ImplicitTrigger {
                kind,
                from,
                to,
                edge_id,
                ..
            } => format!("{kind}|{from}|{to}|{edge_id}"),
            SnapshotGraphEdge::ObjectRunUnresolved {
                kind,
                from,
                edge_id,
                ..
            } => format!("{kind}|{from}||{edge_id}"),
        }
    }

    fn edge_id(&self) -> &str {
        match self {
            SnapshotGraphEdge::DirectCall { edge_id, .. }
            | SnapshotGraphEdge::VariableTypedCall { edge_id, .. }
            | SnapshotGraphEdge::InterfaceDispatch { edge_id, .. }
            | SnapshotGraphEdge::ObjectRunResolved { edge_id, .. }
            | SnapshotGraphEdge::ObjectRunUnresolved { edge_id, .. }
            | SnapshotGraphEdge::EventDispatch { edge_id, .. }
            | SnapshotGraphEdge::ImplicitTrigger { edge_id, .. } => edge_id,
        }
    }

    fn kind_str(&self) -> &str {
        match self {
            SnapshotGraphEdge::DirectCall { kind, .. }
            | SnapshotGraphEdge::VariableTypedCall { kind, .. }
            | SnapshotGraphEdge::InterfaceDispatch { kind, .. }
            | SnapshotGraphEdge::ObjectRunResolved { kind, .. }
            | SnapshotGraphEdge::ObjectRunUnresolved { kind, .. }
            | SnapshotGraphEdge::EventDispatch { kind, .. }
            | SnapshotGraphEdge::ImplicitTrigger { kind, .. } => kind,
        }
    }

    /// callsiteId of a call-family edge (for the resolution ledger lookup).
    /// An implicit-trigger edge has none (#96 M1): its identity is its
    /// operation ([`Self::edge_operation_id`]).
    fn callsite_id(&self) -> Option<&str> {
        match self {
            SnapshotGraphEdge::DirectCall { callsite_id, .. }
            | SnapshotGraphEdge::VariableTypedCall { callsite_id, .. }
            | SnapshotGraphEdge::InterfaceDispatch { callsite_id, .. }
            | SnapshotGraphEdge::ObjectRunResolved { callsite_id, .. }
            | SnapshotGraphEdge::ObjectRunUnresolved { callsite_id, .. } => Some(callsite_id),
            SnapshotGraphEdge::EventDispatch { .. } | SnapshotGraphEdge::ImplicitTrigger { .. } => {
                None
            }
        }
    }

    /// `to` endpoint (for the resolution-ledger interface candidate list).
    fn to_endpoint(&self) -> Option<&str> {
        match self {
            SnapshotGraphEdge::DirectCall { to, .. }
            | SnapshotGraphEdge::VariableTypedCall { to, .. }
            | SnapshotGraphEdge::InterfaceDispatch { to, .. }
            | SnapshotGraphEdge::ObjectRunResolved { to, .. }
            | SnapshotGraphEdge::EventDispatch { to, .. }
            | SnapshotGraphEdge::ImplicitTrigger { to, .. } => Some(to),
            SnapshotGraphEdge::ObjectRunUnresolved { .. } => None,
        }
    }

    /// `from` endpoint (for the dep third-pass resolution).
    fn source_routine(&self) -> &str {
        match self {
            SnapshotGraphEdge::DirectCall { from, .. }
            | SnapshotGraphEdge::VariableTypedCall { from, .. }
            | SnapshotGraphEdge::InterfaceDispatch { from, .. }
            | SnapshotGraphEdge::ObjectRunResolved { from, .. }
            | SnapshotGraphEdge::ObjectRunUnresolved { from, .. }
            | SnapshotGraphEdge::EventDispatch { from, .. }
            | SnapshotGraphEdge::ImplicitTrigger { from, .. } => from,
        }
    }

    /// The INTERNAL `eventId` of an event-dispatch edge: digest's first-hop
    /// selector for an event fact (#98 D2). `None` on every other kind.
    pub fn edge_event_id(&self) -> Option<&str> {
        match self {
            SnapshotGraphEdge::EventDispatch { event_id, .. } => Some(event_id),
            _ => None,
        }
    }

    /// The firing record operation of an implicit-trigger edge (#96 D2), the
    /// `PdEdgeKey`'s last part and digest's first-hop selector (#96 D5). `None`
    /// on every other kind.
    pub fn edge_operation_id(&self) -> Option<&str> {
        match self {
            SnapshotGraphEdge::ImplicitTrigger { operation_id, .. } => Some(operation_id),
            _ => None,
        }
    }

    /// Public `from` accessor (R4-F Stage-4 ordering: typedEdge `.from`).
    pub fn edge_from(&self) -> &str {
        self.source_routine()
    }

    /// Public `to` accessor (R4-F Stage-4 ordering: typedEdge `.to`; None for
    /// object-run-unresolved which has no `to` — al-sem reads `undefined`).
    pub fn edge_to(&self) -> Option<&str> {
        self.to_endpoint()
    }

    /// Public `callsiteId` accessor (None for event-dispatch).
    pub fn edge_callsite_id(&self) -> Option<&str> {
        self.callsite_id()
    }

    /// Public `kind` accessor (the `GraphEdgeKind` discriminant string).
    pub fn edge_kind(&self) -> &str {
        self.kind_str()
    }
}

/// Operation evidence — anchor metadata for op witnesses. al-sem
/// `OperationEvidence` field order: operationId, routine, sourceFile, startLine,
/// startColumn, endLine, endColumn, displayText, [controlContext], [order],
/// [underAsserterror].
#[derive(Debug, Clone, Serialize)]
pub struct SnapshotOperationEvidence {
    #[serde(rename = "operationId")]
    pub operation_id: String,
    pub routine: String,
    #[serde(rename = "sourceFile")]
    pub source_file: String,
    #[serde(rename = "startLine")]
    pub start_line: u32,
    #[serde(rename = "startColumn")]
    pub start_column: u32,
    #[serde(rename = "endLine")]
    pub end_line: u32,
    #[serde(rename = "endColumn")]
    pub end_column: u32,
    #[serde(rename = "displayText")]
    pub display_text: String,
    #[serde(rename = "controlContext", skip_serializing_if = "Option::is_none")]
    pub control_context: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub order: Option<OperationOrder>,
    #[serde(rename = "underAsserterror", skip_serializing_if = "Option::is_none")]
    pub under_asserterror: Option<bool>,
}

/// Callsite evidence — anchor metadata for call witnesses. al-sem
/// `CallsiteEvidence` field order: callsiteId, routine, sourceFile, startLine,
/// startColumn, endLine, endColumn, calleeDisplay, [controlContext], [order],
/// [underAsserterror].
#[derive(Debug, Clone, Serialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct SnapshotCallsiteEvidence {
    #[serde(rename = "callsiteId")]
    pub callsite_id: String,
    pub routine: String,
    #[serde(rename = "sourceFile")]
    pub source_file: String,
    #[serde(rename = "startLine")]
    pub start_line: u32,
    #[serde(rename = "startColumn")]
    pub start_column: u32,
    #[serde(rename = "endLine")]
    pub end_line: u32,
    #[serde(rename = "endColumn")]
    pub end_column: u32,
    #[serde(rename = "calleeDisplay")]
    pub callee_display: String,
    #[serde(rename = "controlContext", skip_serializing_if = "Option::is_none")]
    pub control_context: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub order: Option<OperationOrder>,
    #[serde(rename = "underAsserterror", skip_serializing_if = "Option::is_none")]
    pub under_asserterror: Option<bool>,
}

/// Per-callsite resolution ledger entry. al-sem `CallsiteResolution` field order:
/// callsiteId, from, calleeDisplay, dispatchKind, status, resolvedEdges,
/// [candidates], [openWorld], [unresolvedCandidates], [resultConsumed],
/// [underAsserterror].
#[derive(Debug, Clone, Serialize)]
pub struct SnapshotCallsiteResolution {
    #[serde(rename = "callsiteId")]
    pub callsite_id: String,
    pub from: String,
    #[serde(rename = "calleeDisplay")]
    pub callee_display: String,
    #[serde(rename = "dispatchKind")]
    pub dispatch_kind: String,
    pub status: String,
    #[serde(rename = "resolvedEdges")]
    pub resolved_edges: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub candidates: Option<Vec<String>>,
    #[serde(rename = "openWorld", skip_serializing_if = "Option::is_none")]
    pub open_world: Option<bool>,
    #[serde(
        rename = "unresolvedCandidates",
        skip_serializing_if = "Option::is_none"
    )]
    pub unresolved_candidates: Option<Vec<SnapshotUnresolvedCandidate>>,
    #[serde(rename = "resultConsumed", skip_serializing_if = "Option::is_none")]
    pub result_consumed: Option<bool>,
    #[serde(rename = "underAsserterror", skip_serializing_if = "Option::is_none")]
    pub under_asserterror: Option<bool>,
    /// G29: an interface site whose implementations the call stack's publishers
    /// select (`DispatchMeta::publisher_scoped`). Not serialized.
    #[serde(skip)]
    pub publisher_scoped: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct SnapshotUnresolvedCandidate {
    #[serde(rename = "objectId")]
    pub object_id: String,
    pub reason: String,
}

/// Non-callsite unknown. al-sem `AnalysisGap`: kind, subject, detail; #220
/// adds `role` (`"primary"` only for a primary `parse-incomplete`).
#[derive(Debug, Clone, Serialize)]
pub struct SnapshotAnalysisGap {
    pub kind: String,
    pub subject: String,
    pub detail: String,
    pub role: String,
}

/// Per-routine coverage record. al-sem `CoverageRecord`: subject, directStatus,
/// inheritedStatus, reasons, unknownTargets.
#[derive(Debug, Clone, Serialize)]
pub struct SnapshotCoverageRecord {
    pub subject: String,
    #[serde(rename = "directStatus")]
    pub direct_status: String,
    #[serde(rename = "inheritedStatus")]
    pub inherited_status: String,
    pub reasons: Vec<String>,
    #[serde(rename = "unknownTargets")]
    pub unknown_targets: Vec<String>,
}

/// Subscriber binding. al-sem `SubscriberBinding`: publisherObject, eventName.
#[derive(Debug, Clone, Serialize)]
pub struct SnapshotSubscriberBinding {
    #[serde(rename = "publisherObject")]
    pub publisher_object: String,
    #[serde(rename = "eventName")]
    pub event_name: String,
}

/// Bipartite publisher/subscriber declaration. al-sem `EventDeclaration`:
/// kind, routine, eventId, [binding], sourceAnchor.
#[derive(Debug, Clone, Serialize)]
pub struct SnapshotEventDeclaration {
    pub kind: String,
    pub routine: String,
    #[serde(rename = "eventId")]
    pub event_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub binding: Option<SnapshotSubscriberBinding>,
    #[serde(rename = "sourceAnchor")]
    pub source_anchor: SnapshotSourceAnchor,
}

/// Root classification slot. al-sem `RootClassificationSlot`: routineId, kinds,
/// externallyReachable, source, confidence, [sourceAnchor], [configEntryId],
/// [resolutionStatus].
#[derive(Debug, Clone, Serialize)]
pub struct SnapshotRootClassificationSlot {
    #[serde(rename = "routineId")]
    pub routine_id: String,
    pub kinds: Vec<String>,
    #[serde(rename = "externallyReachable")]
    pub externally_reachable: bool,
    pub source: String,
    pub confidence: String,
    #[serde(rename = "sourceAnchor", skip_serializing_if = "Option::is_none")]
    pub source_anchor: Option<SnapshotSourceAnchor>,
    #[serde(rename = "configEntryId", skip_serializing_if = "Option::is_none")]
    pub config_entry_id: Option<String>,
    #[serde(rename = "resolutionStatus", skip_serializing_if = "Option::is_none")]
    pub resolution_status: Option<String>,
}

/// The consumed-core CapabilitySnapshot. Top-level key order is FIXED (al-sem
/// `ConsumedCoreSnapshot`): identities, capabilityFacts, typedEdges,
/// operationIndex, callsiteIndex, callsiteResolutions, analysisGaps, coverage,
/// eventDeclarations, rootClassifications, [routineOrderFrames].
#[derive(Debug, Clone)]
pub struct CapabilitySnapshot {
    pub identities: SnapshotIdentityTable,
    pub capability_facts: Vec<SnapshotCapabilityFact>,
    pub typed_edges: Vec<SnapshotGraphEdge>,
    pub operation_index: Vec<SnapshotOperationEvidence>,
    pub callsite_index: Vec<SnapshotCallsiteEvidence>,
    pub callsite_resolutions: Vec<SnapshotCallsiteResolution>,
    pub analysis_gaps: Vec<SnapshotAnalysisGap>,
    pub coverage: Vec<SnapshotCoverageRecord>,
    pub event_declarations: Vec<SnapshotEventDeclaration>,
    pub root_classifications: Vec<SnapshotRootClassificationSlot>,
    /// Absent when no routine has scope frames (symbol-only). A sorted-key map
    /// (StableRoutineId → ScopeFrame[]); serialized in sorted-key order with
    /// declaration-ordered frame fields via `RoutineOrderFrames`.
    pub routine_order_frames: Option<RoutineOrderFrames>,
    /// #69 D2: the producer solve (`l4::pd_solve`) over `typed_edges`. Never
    /// serialized: the serialized snapshot moves only where fact values move.
    pub pd_solution: PdArena,
    /// #160 step 6 rule B (spec §14.3.5): set only on digest's tree over the
    /// cone graph ([`compose_tree`]); `None` on the cut. Never serialized.
    pub boundary: Option<BoundaryRule>,
}

/// Rule B's input (spec §14.3.5): the tree's dependency routines, by the role
/// rule (#220), each with its app's name. A witness ends at the first routine
/// listed here.
#[derive(Debug, Clone, Default)]
pub struct BoundaryRule {
    /// Stable routine id of every `Role::Dependency` routine → its app's name.
    pub dependency_app: HashMap<String, std::sync::Arc<str>>,
}

/// Index into `PdArena::states`.
pub type StateIx = usize;
/// Index into `CapabilitySnapshot::typed_edges` (after its sort and filter).
pub type EdgeIx = usize;
/// Stable routine id (the snapshot's routine identity).
pub type RoutineId = String;

/// #69 D2: the producer solve in snapshot ids. A state says: in `routine`'s
/// frame, the producer's write has temp state `temp_state`. Guards are not
/// carried: the solve already dropped every contradicted state, and a proof
/// reads only links. The producer's operation id is
/// `capability_facts[producer_fact].witness_operation_id`.
#[derive(Debug, Clone, Default)]
pub struct PdArena {
    pub states: Vec<StateNode>,
    /// Each state's incoming links, contiguous: `(callee state, edge)`, in edge
    /// order. The edge is an out-edge of the state's routine.
    pub links: Vec<(StateIx, EdgeIx)>,
    /// Stable routine id → its states (seed included), in state order.
    pub by_routine: HashMap<RoutineId, Vec<StateIx>>,
}

#[derive(Debug, Clone)]
pub struct StateNode {
    /// Index into `CapabilitySnapshot::capability_facts` of the producer's
    /// owner-direct fact (its `extra` carries the raw parameter-dependent state).
    pub producer_fact: usize,
    pub temp_state: crate::engine::l4::effect_lattice::TempStateKind,
    pub routine: RoutineId,
    /// True only for the seed (the producer's owner, its own frame).
    pub seed: bool,
    /// Range into `PdArena::links`.
    pub links: std::ops::Range<usize>,
}

/// Ordered routine-order-frames map. Entries are pre-sorted by StableRoutineId;
/// serialized as a JSON object preserving that order + declaration-ordered
/// `ScopeFrame` fields (serde_json `preserve_order` is OFF for this target, so a
/// `serde_json::Value` object would scramble field order — a custom `serialize_map`
/// emits entries in fed order).
#[derive(Debug, Clone)]
pub struct RoutineOrderFrames {
    entries: Vec<(String, Vec<ScopeFrame>)>,
}

impl RoutineOrderFrames {
    /// Look up the scope-frame table for a routine (`snap.routineOrderFrames?.[routine]`).
    pub fn get(&self, routine_id: &str) -> Option<&[ScopeFrame]> {
        self.entries
            .iter()
            .find(|(k, _)| k == routine_id)
            .map(|(_, v)| v.as_slice())
    }

    /// Keep the frame tables whose routine passes `keep` (#220 key classes).
    pub(crate) fn retain(&mut self, mut keep: impl FnMut(&str) -> bool) {
        self.entries.retain(|(k, _)| keep(k));
    }

    /// Hand-built frame tables for tests (#220 T9g).
    #[cfg(test)]
    pub(crate) fn from_entries(entries: Vec<(String, Vec<ScopeFrame>)>) -> Self {
        RoutineOrderFrames { entries }
    }
}

impl Serialize for RoutineOrderFrames {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut map = serializer.serialize_map(Some(self.entries.len()))?;
        for (k, frames) in &self.entries {
            map.serialize_entry(k, frames)?;
        }
        map.end()
    }
}

// ===========================================================================
// edge-id (al-sem `derive/edge-id.ts` `computeEdgeId`).
//
// parts = [kind, String(from)], then PUSH each present-and-not-undefined identity
// field IN THIS FIXED ORDER: to, callsiteId, eventId, operationId, targetObject,
// triggerKind, targetAppGuid, interfaceName. (Predicate: present AND not undefined —
// `Option::is_some`. receiverType/candidateCount EXCLUDED.) edgeId =
// sha256(parts.join("|")).hex[..16].
// ===========================================================================

/// The edge-id identity fields, in the FIXED push order. `None` = absent (NOT
/// pushed); `Some("")` IS pushed (empty string kept, M9).
struct EdgeIdentity<'a> {
    kind: &'a str,
    from: &'a str,
    to: Option<&'a str>,
    callsite_id: Option<&'a str>,
    event_id: Option<&'a str>,
    operation_id: Option<&'a str>,
    target_object: Option<&'a str>,
    trigger_kind: Option<&'a str>,
    target_app_guid: Option<&'a str>,
    interface_name: Option<&'a str>,
}

fn compute_edge_id(e: &EdgeIdentity) -> String {
    let mut parts: Vec<String> = vec![e.kind.to_string(), e.from.to_string()];
    // M9: present-only predicate — push iff Some (empty string kept).
    if let Some(v) = e.to {
        parts.push(v.to_string());
    }
    if let Some(v) = e.callsite_id {
        parts.push(v.to_string());
    }
    if let Some(v) = e.event_id {
        parts.push(v.to_string());
    }
    if let Some(v) = e.operation_id {
        parts.push(v.to_string());
    }
    if let Some(v) = e.target_object {
        parts.push(v.to_string());
    }
    if let Some(v) = e.trigger_kind {
        parts.push(v.to_string());
    }
    if let Some(v) = e.target_app_guid {
        parts.push(v.to_string());
    }
    if let Some(v) = e.interface_name {
        parts.push(v.to_string());
    }
    let joined = parts.join("|");
    let full = crate::engine::ids::sha256_hex(&joined);
    full[..16].to_string()
}

// ===========================================================================
// Stable-id helpers (mirror al-sem IdentityIndex).
// ===========================================================================

/// Internal RoutineId → StableRoutineId; pass through if unmapped.
pub(crate) fn stable_routine_id(internal: &str, map: &HashMap<String, String>) -> String {
    map.get(internal)
        .cloned()
        .unwrap_or_else(|| internal.to_string())
}

/// Internal TableId (`appGuid/table/N`) → StableTableId (`appGuid:Table:N`).
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

/// Internal EventId → StableEventId via the EventSymbol (publisherObject:: ::).
/// Mirrors `toStableEventId(stablePublisherObject, eventName, signatureHash)`.
fn stable_event_id_for(evt: &EventSymbol) -> String {
    format!(
        "{}::{}::{}",
        to_stable_object_id(&evt.publisher_object_id),
        evt.event_name,
        evt.signature_hash
    )
}

// ===========================================================================
// renderCallee (al-sem `derive/callsite-evidence.ts` `renderCallee`).
// ===========================================================================

fn render_callee(callee: &PCallee) -> String {
    match callee {
        PCallee::Bare { name } => name.clone(),
        PCallee::Member { receiver, method } => format!("{receiver}.{method}"),
        PCallee::ObjectRun { object_kind, .. } => format!("{object_kind}.Run"),
        PCallee::Unknown => "unknown".to_string(),
    }
}

/// `displayOfCallee` (al-sem `derive/callsite-resolutions.ts`). Object-run renders
/// the target ref. Used for the callsite-resolution ledger calleeDisplay and
/// d35's own unresolved sites (#265).
pub(crate) fn display_of_callee(callee: &PCallee) -> String {
    match callee {
        PCallee::Bare { name } => name.clone(),
        PCallee::Member { receiver, method } => format!("{receiver}.{method}"),
        PCallee::ObjectRun {
            object_kind,
            target_ref,
            ..
        } => {
            let r = target_ref.as_deref().unwrap_or("<dynamic>");
            format!("{object_kind}.Run({r})")
        }
        PCallee::Unknown => "<unknown>".to_string(),
    }
}

// ===========================================================================
// Lowercased attribute names for the operation-order TryFunction guard.
// ===========================================================================

fn routine_attr_names_lc(r: &ModelRoutine) -> Vec<String> {
    r.attributes_parsed
        .iter()
        .map(|a| a.name.to_lowercase())
        .collect()
}

/// Re-apply the L2 operation-order pass over a routine's features (the L3
/// assembly does NOT stamp `cs.order`/`op.order`/scopeFrames). Returns the
/// stamped call sites, operation sites, and the scope-frame table.
///
/// REUSES `apply_operation_order` (the exact L2 walker) — no re-derivation.
struct RoutineOrder {
    call_sites: Vec<PCallSite>,
    operation_sites: Vec<POperationSite>,
    scope_frames: Vec<ScopeFrame>,
}

fn compute_routine_order(r: &ModelRoutine) -> RoutineOrder {
    use crate::program::body::features::PFeatures;
    // Build a minimal PFeatures carrying ONLY what apply_operation_order reads:
    // statement_tree (the CFN skeleton) + call_sites + operation_sites. The walker
    // stamps `order` onto each site and sets scope_frames.
    let mut features = PFeatures {
        loops: Vec::new(),
        operation_sites: r.operation_sites.clone(),
        record_operations: Vec::new(),
        call_sites: r.call_sites.clone(),
        field_accesses: Vec::new(),
        record_variables: Vec::new(),
        nesting_depth: 0,
        has_branching: r.has_branching,
        unreachable_statements: Vec::new(),
        identifier_references: Vec::new(),
        variables: Vec::new(),
        var_assignments: Vec::new(),
        condition_references: Vec::new(),
        statement_tree: r.statement_tree.clone(),
        scope_frames: Vec::new(),
    };
    let attr_names_lc = routine_attr_names_lc(r);
    apply_operation_order(&mut features, &attr_names_lc);
    RoutineOrder {
        call_sites: features.call_sites,
        operation_sites: features.operation_sites,
        scope_frames: features.scope_frames,
    }
}

/// The rows of the snapshot's COLLIDED ids (#160 step 11b §4.6; spec §3.2
/// #20): which row owns each of their call sites (class P), and where each
/// row's scope frames sit in the id's ONE frame table. Every row's frames are
/// kept (the call-site evidence keeps every row's copy, class K, and each copy's
/// `order` names its own row's frames), in blocks ordered by the row's
/// declaration anchor, then row order, so the table does not depend on row
/// order; each row's frame ids are shifted past the blocks before it, and rows
/// with equal frame tables share one block. A
/// single-row id is in neither map: its own frames, unshifted.
struct CollidedRows<'a> {
    /// Call-site id -> its LAST OWNING row.
    site_owner: HashMap<&'a str, crate::program::model::ix::RoutineIx>,
    /// Row -> the shift its frame ids take in its id's table.
    frame_offset: HashMap<usize, i64>,
    /// Collided id -> its one frame table.
    frames: HashMap<&'a str, Vec<ScopeFrame>>,
}

impl<'a> CollidedRows<'a> {
    fn build(routines: &'a [ModelRoutine]) -> Self {
        use crate::program::model::projection::{RowsOfId, site_owners};
        let rows = RowsOfId::build(routines);
        let site_owner = site_owners(&rows, routines);
        let mut frame_offset = HashMap::new();
        let mut frames = HashMap::new();
        for (id, ixs) in rows.collided() {
            let mut per_row: Vec<(usize, Vec<ScopeFrame>)> = ixs
                .iter()
                .map(|ix| {
                    (
                        ix.get(),
                        compute_routine_order(&routines[ix.get()]).scope_frames,
                    )
                })
                .filter(|(_, f)| !f.is_empty())
                .collect();
            per_row.sort_by_key(|(ri, _)| {
                let a = &routines[*ri].source_anchor;
                (a.source_unit_id.clone(), a.start_line, a.start_column, *ri)
            });
            let (mut table, mut next) = (Vec::new(), 0);
            // Rows with EQUAL frame tables share one block (a frame table is pure
            // structure), so equal rows keep equal `order`s and their evidence
            // copies still collapse (class K).
            let mut blocks: Vec<(Vec<ScopeFrame>, i64)> = Vec::new();
            for (ri, fs) in per_row {
                if let Some((_, off)) = blocks.iter().find(|(b, _)| *b == fs) {
                    frame_offset.insert(ri, *off);
                    continue;
                }
                blocks.push((fs.clone(), next));
                let off = next;
                frame_offset.insert(ri, off);
                for mut f in fs {
                    f.frame_id += off;
                    if f.parent_frame_id >= 0 {
                        f.parent_frame_id += off;
                    }
                    next = next.max(f.frame_id + 1);
                    table.push(f);
                }
            }
            frames.insert(id, table);
        }
        CollidedRows {
            site_owner,
            frame_offset,
            frames,
        }
    }

    /// Row `ri`'s `order`, its frame id shifted into its id's table.
    fn order(&self, ri: usize, order: Option<OperationOrder>) -> Option<OperationOrder> {
        let off = self.frame_offset.get(&ri).copied().unwrap_or(0);
        order.map(|mut o| {
            if o.frame_id >= 0 {
                o.frame_id += off;
            }
            o
        })
    }

    /// Whether row `ri` is the LAST OWNING row of call site `cs` (always, for a
    /// single-row id's site).
    fn owns_site(&self, ri: usize, cs: &str) -> bool {
        self.site_owner.get(cs).is_none_or(|o| o.get() == ri)
    }
}

// ===========================================================================
// compose_snapshot — the orchestrator. Runs every consumed-core deriver over the
// source-only R3a base, then assembles in fixed key order.
// ===========================================================================

/// Compose the consumed-core `CapabilitySnapshot` for a resolved source-only
/// workspace. `resolved` carries the workspace routines + root classifications.
///
/// Over the cut graph (`build_r3a3_source_only_base`: workspace callers only).
/// Its readers since #160 step 9 (digest, prove, `fingerprint --routine` and
/// `--inventory-only` read [`compose_tree`]): the ordering cell, cross-app and
/// single-app, until the #161 follow-up moves it; fingerprint's full snapshot
/// (B0), its query forms with no `--routine`, and `diff`, single-app, where the
/// cut equals the cone graph; the r4f projections and tests. Rule B is off here
/// (`boundary: None`).
pub fn compose_snapshot(resolved: &Model) -> CapabilitySnapshot {
    let base = {
        let _s = crate::engine::perf_trace::span("snapshot", "snapshot.r3a3_base");
        build_r3a3_source_only_base(resolved)
    };
    compose_snapshot_from_base(resolved, &base, None)
}

/// The `callsite_resolutions` of [`compose_snapshot`], without the rest of the
/// snapshot. They read only the call-site ledger ([`Ledger`]: the routines,
/// the cut graph's typed edges, the calls), never the cone, so this builds the
/// same cut graph `build_r3a3_source_only_base` does and skips the cone.
///
/// The cone is what a whole snapshot costs. Over the cross-app CDO model (33k
/// routines) `compose_snapshot` peaked at 46 GB and took 60 s, almost all of it
/// the raw inherited facts of every routine (measured 2026-10-31, Linux, `cdo`
/// profile); a reader of the call-site rows alone should call this.
pub fn compose_callsite_resolutions(resolved: &Model) -> Vec<SnapshotCallsiteResolution> {
    let ws = &resolved.workspace;
    let graph = crate::engine::l4::combined_graph::build_combined_graph(
        ws,
        &resolved.calls,
        &resolved.events.graph,
    );
    let routine_to_stable = crate::engine::l4::capability_cone::routine_to_stable_of(ws);
    let ledger = Ledger {
        ws_routines: &ws.routines,
        typed_edges: &graph.typed_edges,
        calls: &resolved.calls.edges,
        dependency_calls: &[],
        routine_to_stable: &routine_to_stable,
        bodyless_dependency: None,
    };
    let collided = CollidedRows::build(&ws.routines);
    let (typed_edges, _) = derive_typed_edges(&ledger);
    derive_callsite_resolutions(&ledger, &typed_edges, &collided)
}

/// [`compose_snapshot`] over an already-built source-only base, for a caller
/// that reads the base too (`snapshot_full::compose_full_snapshot`): the base
/// runs the whole cone and the producer solve, so it is built once.
/// `source_less_apps`: D9's set when the caller holds a cross-app base, else
/// `None` (the old gap rule).
pub fn compose_snapshot_from_base(
    resolved: &Model,
    base: &R3a3SourceBase,
    source_less_apps: Option<&std::collections::HashSet<String>>,
) -> CapabilitySnapshot {
    crate::engine::perf_trace::counter_delta("build.snapshot", 1);
    let _s = crate::engine::perf_trace::span("snapshot", "snapshot.derive");

    let inp = TreeInputs {
        ws_routines: &base.ws_routines,
        typed_edges: &base.graph.typed_edges,
        calls: &base.calls.edges,
        dependency_calls: &[],
        event_graph: &base.event_graph,
        direct_full: &base.direct_full,
        coverage: &base.coverage,
        routine_to_stable: &base.routine_to_stable,
        bodyless_dependency: None,
        source_less_apps,
    };
    let collided = CollidedRows::build(&base.ws_routines);
    let identities = derive_identity_table(resolved);
    let capability_facts = derive_capability_facts(&inp, |id| base.raw.get(id).map(Vec::as_slice));
    let (typed_edges, skipped) = derive_typed_edges(&inp.ledger());
    let raw = |id: &str| base.raw.get(id).map(Vec::as_slice);
    let operation_index = derive_operation_evidence(&inp, raw, &collided);
    let callsite_index = derive_callsite_evidence(&inp, raw, &collided);
    let callsite_resolutions = derive_callsite_resolutions(&inp.ledger(), &typed_edges, &collided);
    let analysis_gaps = derive_analysis_gaps(resolved, &inp);
    let coverage = derive_coverage(&inp);
    let event_declarations = derive_event_declarations(&inp);
    let root_classifications = derive_root_classifications(resolved, &inp);
    let routine_order_frames = derive_routine_order_frames(&inp, &collided);
    let pd_solution = derive_pd_arena(
        &inp,
        &base.pd_solution,
        None,
        &typed_edges,
        &capability_facts,
        &skipped,
    );

    finish_snapshot(CapabilitySnapshot {
        identities,
        capability_facts,
        typed_edges,
        operation_index,
        callsite_index,
        callsite_resolutions,
        analysis_gaps,
        coverage,
        event_declarations,
        root_classifications,
        routine_order_frames,
        pd_solution,
        boundary: None,
    })
}

/// The parts every snapshot deriver reads, borrowed (#160 step 6 D4): from a
/// cut base ([`compose_snapshot_from_base`]) or from the view
/// ([`compose_tree`]). The raw inherited facts are passed apart, so the tree
/// can drain them.
struct TreeInputs<'a> {
    /// Every routine row of the model (the universe + per-routine features).
    ws_routines: &'a [ModelRoutine],
    /// The typed edges the tree is composed over.
    typed_edges: &'a [TypedEdge],
    /// The model's own resolved calls (the call-site ledger, first).
    calls: &'a [CallEdge],
    /// The calls dependency routines make (the ledger, after `calls`; D6).
    dependency_calls: &'a [CallEdge],
    event_graph: &'a EventGraph,
    /// Per-routine RAW direct capability facts (full, ordered).
    direct_full: &'a HashMap<String, Vec<CapabilityFact>>,
    /// Per-routine cone coverage: every routine that has a cone entry.
    coverage: &'a HashMap<String, CoverageRecord>,
    routine_to_stable: &'a HashMap<String, String>,
    /// D6: the internal ids of the body-less dependency rows. A resolved call
    /// whose every target is one is `unfetched-dependency`. `None` on the cut.
    bodyless_dependency: Option<&'a std::collections::HashSet<&'a str>>,
    /// #160 step 9 D9: the lowercase guids of the dependency apps with no
    /// source ([`source_less_apps`]). Only such an app's body-less rows give a
    /// `symbol-only-boundary` gap; a source app's give `body-unavailable`.
    /// `None` (a cut with no cross-app base) keeps the old rule: every
    /// body-less dependency row's app.
    source_less_apps: Option<&'a std::collections::HashSet<String>>,
}

/// The call-site ledger part of [`TreeInputs`]: everything the typed edges and
/// the call-site resolutions read, and nothing of the cone (no direct facts,
/// no coverage, no raw facts). Those two derivers take this type, so the
/// compiler holds [`compose_callsite_resolutions`] to the same rows
/// [`compose_snapshot`] emits without building the cone.
struct Ledger<'a> {
    ws_routines: &'a [ModelRoutine],
    typed_edges: &'a [TypedEdge],
    calls: &'a [CallEdge],
    dependency_calls: &'a [CallEdge],
    routine_to_stable: &'a HashMap<String, String>,
    bodyless_dependency: Option<&'a std::collections::HashSet<&'a str>>,
}

impl<'a> TreeInputs<'a> {
    fn ledger(&self) -> Ledger<'a> {
        Ledger {
            ws_routines: self.ws_routines,
            typed_edges: self.typed_edges,
            calls: self.calls,
            dependency_calls: self.dependency_calls,
            routine_to_stable: self.routine_to_stable,
            bodyless_dependency: self.bodyless_dependency,
        }
    }
}

/// The lowercase guids of the dependency apps that ship no source
/// (`DependencyApp::has_source == false`), for D9.
pub(crate) fn source_less_apps(
    apps: &[crate::program::model::workspace::DependencyApp],
) -> std::collections::HashSet<String> {
    apps.iter()
        .filter(|a| !a.has_source)
        .map(|a| a.guid.to_ascii_lowercase())
        .collect()
}

/// The dependency apps a tree names (#160 step 6 rule B; step 9 D9): `names`,
/// lowercase app guid → name of the loaded apps, and `source_less`,
/// [`source_less_apps`]. Both empty single-app.
#[derive(Default)]
pub(crate) struct TreeApps {
    pub names: HashMap<String, String>,
    pub source_less: std::collections::HashSet<String>,
}

/// A borrow-mode tree's one view of the view's shared raw map (#277 D5.3,
/// O20): every raw reader of [`compose_tree`] walks every workspace routine,
/// so each goes through [`SubjectRaw::get`], which hides any id outside the
/// tree's own subjects.
pub(crate) struct SubjectRaw<'a> {
    map: &'a HashMap<String, Vec<CapabilityFact>>,
    subjects: std::collections::BTreeSet<String>,
}

impl<'a> SubjectRaw<'a> {
    /// The subjects are `scope` ∩ the map's keys (O21): a scoped routine the
    /// composition never walked has no entry, and the scope alone would widen
    /// the pd arena's scope.
    pub(crate) fn new(
        map: &'a HashMap<String, Vec<CapabilityFact>>,
        scope: &std::collections::BTreeSet<String>,
    ) -> Self {
        let subjects = scope
            .iter()
            .filter(|id| map.contains_key(id.as_str()))
            .cloned()
            .collect();
        SubjectRaw { map, subjects }
    }

    /// `id`'s raw facts; `None` outside the subjects.
    pub(crate) fn get(&self, id: &str) -> Option<&'a [CapabilityFact]> {
        if self.subjects.contains(id) {
            self.map.get(id).map(Vec::as_slice)
        } else {
            None
        }
    }

    pub(crate) fn subjects(&self) -> &std::collections::BTreeSet<String> {
        &self.subjects
    }
}

/// How a tree reads the raw facts (#277 D5.3). `Drain`: the tree is the last
/// reader of every one of its subjects, and the map holds exactly their
/// entries (the subjects are its keys), moved in and dropped as they go.
/// `Borrow`: some subject has a later reader, so the shared map is lent
/// through [`SubjectRaw`].
pub(crate) enum TreeRaw<'a> {
    Drain(HashMap<String, Vec<CapabilityFact>>),
    Borrow(SubjectRaw<'a>),
}

/// The tree over the view's cone graph (#160 step 6 D4, D5, D6): `typed` =
/// `rel.cone()`, the ledger over `model.calls` then `dependency_calls`, the
/// direct facts and coverage from `caps`, the raw facts of the subjects
/// (`raw`, drained or borrowed, #277 D5.3), the pd arena (`pd`: lent to every
/// tree of a view but the last, owned by the last) scoped to the subjects'
/// state closure, and its own identity table (digest's root resolution builds
/// one too, but drops it before the composition so it never sits beside the
/// composition's peak; final panel F1-C4). Rule B is on (spec §14.3.5): every
/// dependency routine is named by its app (`apps.names`); D9's source-less
/// apps are `apps.source_less` (#160 step 9).
pub(crate) fn compose_tree(
    model: &Model,
    typed: &[TypedEdge],
    dependency_calls: &[CallEdge],
    caps: &crate::engine::l5::detector_context::CapabilityAnalysis,
    raw: TreeRaw<'_>,
    pd: std::borrow::Cow<'_, crate::engine::l4::pd_solve::ConeArena>,
    apps: &TreeApps,
) -> CapabilitySnapshot {
    crate::engine::perf_trace::counter_delta("build.snapshot", 1);
    let _s = crate::engine::perf_trace::span("snapshot", "snapshot.derive");
    let ws = &model.workspace;
    let boundary = boundary_rule(model, &apps.names);
    let bodyless: std::collections::HashSet<&str> = ws
        .routines
        .iter()
        .filter(|r| {
            !r.body_available
                && model.routine_role(r) == crate::program::model::role::Role::Dependency
                // A cataloged platform native has known behaviour (no unfetched body).
                && crate::program::model::platform_natives::pure_native(r).is_none()
        })
        .map(|r| r.id.as_str())
        .collect();
    let routine_to_stable = crate::engine::l4::capability_cone::routine_to_stable_of(ws);
    let cone_pd = pd;
    let inp = TreeInputs {
        ws_routines: &ws.routines,
        typed_edges: typed,
        calls: &model.calls.edges,
        dependency_calls,
        event_graph: &model.events.graph,
        direct_full: &caps.direct_full,
        coverage: &caps.coverage,
        routine_to_stable: &routine_to_stable,
        bodyless_dependency: Some(&bodyless),
        source_less_apps: Some(&apps.source_less),
    };
    let collided = CollidedRows::build(&ws.routines);
    let identities = derive_identity_table(model);
    // The two evidence indexes read the raw facts, so they run first; in
    // drain mode each subject's Vec is then moved in and dropped as it goes.
    let (subjects, operation_index, callsite_index, capability_facts) = match raw {
        TreeRaw::Drain(mut map) => {
            let subjects: std::collections::BTreeSet<String> = map.keys().cloned().collect();
            let get = |id: &str| map.get(id).map(Vec::as_slice);
            let operation_index = derive_operation_evidence(&inp, get, &collided);
            let callsite_index = derive_callsite_evidence(&inp, get, &collided);
            let capability_facts = derive_capability_facts(&inp, |id| map.remove(id));
            drop(map);
            (subjects, operation_index, callsite_index, capability_facts)
        }
        TreeRaw::Borrow(acc) => {
            let get = |id: &str| acc.get(id);
            let operation_index = derive_operation_evidence(&inp, get, &collided);
            let callsite_index = derive_callsite_evidence(&inp, get, &collided);
            let capability_facts = derive_capability_facts(&inp, get);
            (
                acc.subjects,
                operation_index,
                callsite_index,
                capability_facts,
            )
        }
    };
    let (typed_edges, skipped) = derive_typed_edges(&inp.ledger());
    let callsite_resolutions = derive_callsite_resolutions(&inp.ledger(), &typed_edges, &collided);
    let analysis_gaps = derive_analysis_gaps(model, &inp);
    let coverage = derive_coverage(&inp);
    let event_declarations = derive_event_declarations(&inp);
    let root_classifications = derive_root_classifications(model, &inp);
    let routine_order_frames = derive_routine_order_frames(&inp, &collided);
    let pd_solution = derive_pd_arena(
        &inp,
        &cone_pd,
        Some(&subjects),
        &typed_edges,
        &capability_facts,
        &skipped,
    );
    drop(cone_pd);

    finish_snapshot(CapabilitySnapshot {
        identities,
        capability_facts,
        typed_edges,
        operation_index,
        callsite_index,
        callsite_resolutions,
        analysis_gaps,
        coverage,
        event_declarations,
        root_classifications,
        routine_order_frames,
        pd_solution,
        boundary: Some(boundary),
    })
}

/// Rule B's dependency routines of `model` (THE role rule, #220), each named
/// by its app from `app_names` (lowercase guid → name).
pub(crate) fn boundary_rule(model: &Model, app_names: &HashMap<String, String>) -> BoundaryRule {
    use crate::program::model::role::Role;
    let mut names: HashMap<String, std::sync::Arc<str>> = HashMap::new();
    let mut dependency_app = HashMap::new();
    for r in &model.workspace.routines {
        if model.routine_role(r) != Role::Dependency {
            continue;
        }
        let guid = r.app_guid.to_ascii_lowercase();
        let name = names.entry(guid).or_insert_with_key(|g| {
            // A dependency row's app is one the build loaded, so it is named;
            // the guid stands in only if the loaded-apps list lacks it.
            std::sync::Arc::from(app_names.get(g).map_or(g.as_str(), String::as_str))
        });
        dependency_app.insert(r.stable_routine_id.clone(), std::sync::Arc::clone(name));
    }
    BoundaryRule { dependency_app }
}

/// The census of a composed snapshot, when it is on.
fn finish_snapshot(snapshot: CapabilitySnapshot) -> CapabilitySnapshot {
    if crate::engine::census::enabled() {
        crate::census_hook::mark("census.snapshot");
        census_snapshot(&snapshot);
    }
    snapshot
}

/// #160 §4.1.5: one `snapshot.<field>` bucket per field of
/// [`CapabilitySnapshot`]. `pd_solution` is counted by [`pd_arena_bytes`]
/// (exact, at capacity); the rest is shallow.
fn census_snapshot(s: &CapabilitySnapshot) {
    use crate::engine::census::{Kind, Tally};
    let emit = |name: &str, t: &mut Tally| t.emit(name, Kind::Shallow);
    emit(
        "snapshot.identities",
        Tally::default()
            .buf::<String>(s.identities.stable_ids.len())
            .strs(&s.identities.stable_ids)
            .buf::<String>(s.identities.display_names.len())
            .strs(&s.identities.display_names),
    );
    emit(
        "snapshot.capability_facts",
        Tally::default().buf::<SnapshotCapabilityFact>(s.capability_facts.len()),
    );
    emit(
        "snapshot.typed_edges",
        Tally::default().buf::<SnapshotGraphEdge>(s.typed_edges.len()),
    );
    emit(
        "snapshot.operation_index",
        Tally::default().buf::<SnapshotOperationEvidence>(s.operation_index.len()),
    );
    emit(
        "snapshot.callsite_index",
        Tally::default().buf::<SnapshotCallsiteEvidence>(s.callsite_index.len()),
    );
    emit(
        "snapshot.callsite_resolutions",
        Tally::default().buf::<SnapshotCallsiteResolution>(s.callsite_resolutions.len()),
    );
    emit(
        "snapshot.analysis_gaps",
        Tally::default().buf::<SnapshotAnalysisGap>(s.analysis_gaps.len()),
    );
    emit(
        "snapshot.coverage",
        Tally::default().buf::<SnapshotCoverageRecord>(s.coverage.len()),
    );
    emit(
        "snapshot.event_declarations",
        Tally::default().buf::<SnapshotEventDeclaration>(s.event_declarations.len()),
    );
    emit(
        "snapshot.root_classifications",
        Tally::default().buf::<SnapshotRootClassificationSlot>(s.root_classifications.len()),
    );
    let mut frames = Tally::default();
    if let Some(f) = &s.routine_order_frames {
        frames
            .buf::<(String, Vec<ScopeFrame>)>(f.entries.len())
            .strs(f.entries.iter().map(|(k, _)| k))
            .bufs(f.entries.iter().map(|(_, v)| v));
    }
    emit("snapshot.routine_order_frames", &mut frames);
    let a = &s.pd_solution;
    let mut allocs = Tally::default();
    allocs
        .buf::<StateNode>(a.states.capacity())
        .strs(a.states.iter().map(|n| &n.routine))
        .buf::<(StateIx, EdgeIx)>(a.links.capacity())
        .buf::<(RoutineId, Vec<StateIx>)>(a.by_routine.capacity())
        .strs(a.by_routine.keys())
        .bufs(a.by_routine.values());
    crate::engine::census::bucket(
        "snapshot.pd_solution",
        pd_arena_bytes(a) as usize,
        allocs.allocs,
        Kind::Exact,
    );
}

// ---------------------------------------------------------------------------
// deriveIdentityTable — objects + tables + routines → parallel sorted arrays.
//
// M8: sorted by stableId, ORDINAL (the contract). stableIds are
// `<guid>:<ObjectType>:<key>[#<lowerhex>]` or `<guid>:Table:<num>`, where `<key>`
// is the object number, or `@` + the percent-encoded (`%XX`) folded name for an
// object without one.
// ---------------------------------------------------------------------------

pub(crate) fn derive_identity_table(resolved: &Model) -> SnapshotIdentityTable {
    let ws = &resolved.workspace;
    // id → display name, deduped. BTreeMap keeps deterministic, ordinal key order
    // (M1, M8).
    let mut pairs: std::collections::BTreeMap<String, String> = std::collections::BTreeMap::new();

    for obj in &ws.objects {
        pairs.insert(to_stable_object_id(&obj.id), obj.name.clone());
    }
    // G40: a REAL table wins an id collision with a `tableextension` stub
    // (`ModelTable::is_extension_stub`): DO's `CDOReturnReceiptHeaderExt`
    // (tableextension 6175291) displayed as table 6175291 "CDO Cust.Template
    // Setup", so `prove … writes-table:CDO Cust.Template Setup` matched none
    // of its writes.
    for (id, tbl) in crate::program::model::workspace::table_by_id_preferring_real(&ws.tables) {
        pairs.insert(stable_table_id(id), tbl.name.clone());
    }
    for r in &ws.routines {
        pairs.insert(r.stable_routine_id.clone(), r.name.clone());
    }

    // BTreeMap iteration is ordinal-sorted by key (M8).
    let stable_ids: Vec<String> = pairs.keys().cloned().collect();
    let display_names: Vec<String> = pairs.values().cloned().collect();
    SnapshotIdentityTable {
        stable_ids,
        display_names,
    }
}

// ---------------------------------------------------------------------------
// deriveCapabilityFacts — flatten each routine's direct ∪ inherited cone facts,
// rewrite subject → StableRoutineId, sort by the documented key.
//
// The facts keep INTERNAL resourceId / witness ids / extra (al-sem composeSnapshot
// rewrites ONLY `subject`; it does NOT project the rest). Sort key (al-sem
// `factKey`): subject|op|resourceKind|resourceId|confidence|provenance|
// witnessCallsiteId|witnessOperationId.
//
// Ordinal (M8 contract).
// ---------------------------------------------------------------------------

fn snapshot_fact(f: &CapabilityFact, subject: &str) -> SnapshotCapabilityFact {
    SnapshotCapabilityFact {
        subject: subject.to_string(),
        op: f.op.to_string(),
        resource_kind: f.resource_kind.to_string(),
        resource_id: f.resource_id.clone(),
        resource_arg_source: f.resource_arg_source.as_ref().map(snap_value_source),
        confidence: f.confidence.to_string(),
        provenance: f.provenance.to_string(),
        via: f.via.to_string(),
        witness_operation_id: f.witness_operation_id.clone(),
        witness_callsite_id: f.witness_callsite_id.clone(),
        extra: f.extra.as_ref().map(snap_capability_extra),
        witness_first_hop_operation_id: f.witness_first_hop_operation_id.clone(),
        witness_first_hop_event_id: f.witness_first_hop_event_id.clone(),
    }
}

fn capability_fact_sort_key(f: &SnapshotCapabilityFact) -> String {
    [
        f.subject.clone(),
        f.op.clone(),
        f.resource_kind.clone(),
        f.resource_id.clone().unwrap_or_default(),
        f.confidence.clone(),
        f.provenance.clone(),
        f.witness_callsite_id.clone().unwrap_or_default(),
        f.witness_operation_id.clone().unwrap_or_default(),
    ]
    .join("|")
}

/// `take_raw(id)` hands each emitted id's raw inherited facts over, borrowed
/// (the cut base) or by move (the tree drains its subjects: each Vec drops
/// once its facts are emitted).
fn derive_capability_facts<R>(
    inp: &TreeInputs<'_>,
    mut take_raw: impl FnMut(&str) -> Option<R>,
) -> Vec<SnapshotCapabilityFact>
where
    R: std::borrow::Borrow<[CapabilityFact]> + Send,
{
    use rayon::prelude::*;
    // Per-routine emission is independent; indexed par_iter + sequential flatten
    // preserves the exact sequential emission order (ties in the sort key below are
    // resolved by the STABLE sort from this input order — do not reorder).
    // The facts are per id (`direct_full`, coverage, raw), so the rows of a
    // collided id would emit equal copies: each id is emitted once, at its first
    // row (class K, #160 step 11b §4.6).
    // Only routines with a cone entry contribute (mirrors composeSnapshot:
    // facts come from `r.summary`, present iff the cone ran for the routine).
    let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
    let first_rows: Vec<(&ModelRoutine, Option<R>)> = inp
        .ws_routines
        .iter()
        .filter(|r| seen.insert(r.id.as_str()) && inp.coverage.contains_key(&r.id))
        .map(|r| (r, take_raw(&r.id)))
        .collect();
    let per_routine: Vec<Vec<SnapshotCapabilityFact>> = first_rows
        .into_par_iter()
        .map(|(r, raw)| {
            let subject = stable_routine_id(&r.id, inp.routine_to_stable);
            let mut v: Vec<SnapshotCapabilityFact> = Vec::new();
            if let Some(direct) = inp.direct_full.get(&r.id) {
                for f in direct {
                    v.push(snapshot_fact(f, &subject));
                }
            }
            if let Some(raw) = raw {
                for f in raw.borrow() {
                    v.push(snapshot_fact(f, &subject));
                }
            }
            v
        })
        .collect();
    let mut all: Vec<SnapshotCapabilityFact> = per_routine.into_iter().flatten().collect();
    // sort_by_cached_key: identical order to sort_by_key (same key, both stable),
    // but the 8-field join("|") String is built ONCE per fact instead of per
    // comparison (~128k facts on CDO).
    all.sort_by_cached_key(capability_fact_sort_key);
    all
}

// ---------------------------------------------------------------------------
// deriveTypedEdges — rebuild model.typedEdges WITH anchors (the Rust TypedEdge
// drops anchors), rewrite from/to → StableRoutineId, stamp edgeId, sort by
// (kind|from|to|edgeId).
//
// REUSES the combined graph + the per-callsite anchors + routine source anchors
// — mirrors al-sem `engine/combined-graph.ts` typed-edge build. The edge-id is
// computed over the STABLE-rewritten from/to (and the present identity fields).
//
// The sort key is kind | stable-from | stable-to | edgeId; ordinal (M8 contract).
// ---------------------------------------------------------------------------

fn derive_typed_edges(inp: &Ledger<'_>) -> (Vec<SnapshotGraphEdge>, SkippedEdgeKeys) {
    // callsite id -> (anchor, owning-routine internal id) for call-family anchors.
    let mut callsite_anchor: HashMap<&str, (&PAnchor, &str)> = HashMap::new();
    for r in inp.ws_routines {
        for cs in &r.call_sites {
            callsite_anchor.insert(cs.id.as_str(), (&cs.source_anchor, r.id.as_str()));
        }
    }
    // routine id -> &PAnchor for event-dispatch publish/subscriber anchors.
    let mut routine_anchor: HashMap<&str, &PAnchor> = HashMap::new();
    for r in inp.ws_routines {
        routine_anchor.insert(r.id.as_str(), &r.source_anchor);
    }
    // record op id -> (anchor, owning-routine internal id) for implicit-trigger
    // anchors (#96 D4).
    let mut op_anchor: HashMap<&str, (&PAnchor, &str)> = HashMap::new();
    for r in inp.ws_routines {
        for ro in &r.record_operations {
            op_anchor.insert(ro.id.as_str(), (&ro.source_anchor, r.id.as_str()));
        }
    }

    let map = inp.routine_to_stable;

    let mut out: Vec<SnapshotGraphEdge> = Vec::new();
    let mut skipped = SkippedEdgeKeys::new();

    for e in inp.typed_edges {
        let from_stable = stable_routine_id(&e.from, map);
        let to_stable = e.to.as_ref().map(|t| stable_routine_id(t, map));

        // eventId stays INTERNAL: al-sem `deriveTypedEdges` does `{...e, edgeId}` —
        // it rewrites ONLY from/to, NEVER eventId. The model edge's eventId is the
        // internal EventId (`appGuid/Type/N/event/name`); the golden carries it
        // verbatim, and the edge-id is computed over that internal form.
        let event_id_internal: Option<String> = e.event_id.clone();
        // targetObject stays INTERNAL for the same reason (deriveTypedEdges does not
        // project it). (Object-run edges are not in the source-only corpus.)
        let target_object_internal: Option<String> = e.target_object.clone();

        // The edge-id over the present identity fields (M9 present-only predicate),
        // computed on the STABLE-rewritten endpoints / projected event+object ids.
        let edge_id = compute_edge_id(&EdgeIdentity {
            kind: &e.kind,
            from: &from_stable,
            to: to_stable.as_deref(),
            callsite_id: e.callsite_id.as_deref(),
            event_id: event_id_internal.as_deref(),
            operation_id: e.operation_id.as_deref(),
            target_object: target_object_internal.as_deref(),
            // Set on implicit-trigger edges only (#96 D1). dependency-export
            // targetAppGuid is not modelled on the Rust TypedEdge.
            trigger_kind: e.trigger_kind.as_deref(),
            target_app_guid: None,
            interface_name: e.interface_name.as_deref(),
        });

        // Resolve per-kind anchors (enclosingRoutineId = owning routine internal id).
        let cs_anchor = e.callsite_id.as_deref().and_then(|cid| {
            callsite_anchor
                .get(cid)
                .map(|(a, rid)| anchor_from_panchor(a, rid))
        });

        // Anchor lookups degrade to a skip (engine-never-throws): the combined-graph
        // builder only emits an edge whose callsite/endpoints are workspace routines,
        // so for the source-only corpus these are always present — but a future
        // dep-edge injection must not panic here.
        let edge = 'build: {
            Some(match e.kind.as_str() {
                "direct-call" => SnapshotGraphEdge::DirectCall {
                    kind: "direct-call",
                    callsite_id: e.callsite_id.clone().unwrap_or_default(),
                    from: from_stable.clone(),
                    to: to_stable.clone().unwrap_or_default(),
                    source_anchor: match cs_anchor {
                        Some(a) => a,
                        None => break 'build None,
                    },
                    edge_id: edge_id.clone(),
                },
                "variable-typed-call" => SnapshotGraphEdge::VariableTypedCall {
                    kind: "variable-typed-call",
                    callsite_id: e.callsite_id.clone().unwrap_or_default(),
                    from: from_stable.clone(),
                    to: to_stable.clone().unwrap_or_default(),
                    receiver_type: e.receiver_type.clone().unwrap_or_default(),
                    source_anchor: match cs_anchor {
                        Some(a) => a,
                        None => break 'build None,
                    },
                    edge_id: edge_id.clone(),
                },
                "interface-dispatch" => SnapshotGraphEdge::InterfaceDispatch {
                    kind: "interface-dispatch",
                    callsite_id: e.callsite_id.clone().unwrap_or_default(),
                    from: from_stable.clone(),
                    to: to_stable.clone().unwrap_or_default(),
                    interface_name: e.interface_name.clone().unwrap_or_default(),
                    candidate_count: e.candidate_count.unwrap_or(0),
                    source_anchor: match cs_anchor {
                        Some(a) => a,
                        None => break 'build None,
                    },
                    edge_id: edge_id.clone(),
                },
                "object-run-resolved" => SnapshotGraphEdge::ObjectRunResolved {
                    kind: "object-run-resolved",
                    callsite_id: e.callsite_id.clone().unwrap_or_default(),
                    from: from_stable.clone(),
                    to: to_stable.clone().unwrap_or_default(),
                    target_object: target_object_internal.clone().unwrap_or_default(),
                    object_type: e.object_type.clone().unwrap_or_default(),
                    source_anchor: match cs_anchor {
                        Some(a) => a,
                        None => break 'build None,
                    },
                    edge_id: edge_id.clone(),
                },
                "object-run-unresolved" => SnapshotGraphEdge::ObjectRunUnresolved {
                    kind: "object-run-unresolved",
                    callsite_id: e.callsite_id.clone().unwrap_or_default(),
                    from: from_stable.clone(),
                    target_object: target_object_internal.clone(),
                    target_id_source: e
                        .target_id_source
                        .as_ref()
                        .map(snap_combined_value_source)
                        .unwrap_or(SnapValueSource::Unknown),
                    object_type: e.object_type.clone().unwrap_or_default(),
                    source_anchor: match cs_anchor {
                        Some(a) => a,
                        None => break 'build None,
                    },
                    edge_id: edge_id.clone(),
                },
                "event-dispatch" => {
                    let Some(publish_anchor) = routine_anchor
                        .get(e.from.as_str())
                        .map(|a| anchor_from_panchor(a, &e.from))
                    else {
                        break 'build None;
                    };
                    let to_internal = e.to.clone().unwrap_or_default();
                    let Some(subscriber_anchor) = routine_anchor
                        .get(to_internal.as_str())
                        .map(|a| anchor_from_panchor(a, &to_internal))
                    else {
                        break 'build None;
                    };
                    SnapshotGraphEdge::EventDispatch {
                        kind: "event-dispatch",
                        from: from_stable.clone(),
                        to: to_stable.clone().unwrap_or_default(),
                        event_id: event_id_internal.clone().unwrap_or_default(),
                        publish_anchor,
                        subscriber_anchor,
                        edge_id: edge_id.clone(),
                    }
                }
                // #96 D4: never dropped for a missing anchor (M2): the cone follows
                // this edge, so the PD arena needs its twin. The anchor is the
                // firing op's, absent when the op is not in a workspace body.
                "implicit-trigger" => SnapshotGraphEdge::ImplicitTrigger {
                    kind: "implicit-trigger",
                    from: from_stable.clone(),
                    to: to_stable.clone().unwrap_or_default(),
                    operation_id: e.operation_id.clone().unwrap_or_default(),
                    trigger_kind: e.trigger_kind.clone().unwrap_or_default(),
                    fires: e
                        .fires
                        .unwrap_or(crate::engine::l4::combined_graph::TriggerFires::Maybe),
                    anchor: e
                        .operation_id
                        .as_deref()
                        .and_then(|op| op_anchor.get(op))
                        .map(|(a, rid)| anchor_from_panchor(a, rid)),
                    edge_id: edge_id.clone(),
                },
                // Unmodelled kind (dependency-export) - never built by the combined
                // graph; skip rather than emit a malformed edge.
                _ => break 'build None,
            })
        };
        match edge {
            Some(edge) => out.push(edge),
            // R6-4: the skipped row's pd key, for `derive_pd_arena`'s check.
            None => {
                let event = match e.kind.as_str() {
                    "event-dispatch" => e.event_id.clone().unwrap_or_default(),
                    _ => String::new(),
                };
                skipped.insert((
                    e.kind.clone(),
                    from_stable,
                    to_stable.unwrap_or_default(),
                    e.callsite_id.clone().unwrap_or_default(),
                    event,
                    e.operation_id.clone().unwrap_or_default(),
                ));
            }
        }
    }

    out.sort_by_key(|e| e.sort_key());
    (out, skipped)
}

/// The pd keys ([`PdEdgeKey`], owned) of the typed edges `derive_typed_edges`
/// skipped for want of an anchor or for an unmodelled kind (R6-4). An
/// `implicit-trigger` row is never skipped, so no key here has that kind.
type SkippedEdgeKeys = std::collections::HashSet<(String, String, String, String, String, String)>;

// ---------------------------------------------------------------------------
// derivePdArena (#69 D2) — the cone's producer solve rewritten into snapshot
// ids: routines → StableRoutineId, each link's cone edge → its index in the
// sorted `typed_edges`, each producer → its owner-direct fact's index.
// ---------------------------------------------------------------------------

/// Edge identity shared by a cone `TypedOutEdge` and a snapshot edge:
/// `(kind, stable from, stable to, callsite, event, operation)`. `operation` is
/// the firing record op of an `implicit-trigger` edge (#96 D2), `""` on every
/// other kind, so two ops of one caller that fire one trigger never collide.
type PdEdgeKey<'a> = (&'a str, &'a str, &'a str, &'a str, &'a str, &'a str);

/// `subjects` scopes the arena (#160 step 6 D5): only the states reachable from
/// a subject's states through the links (`callee_state`) are mapped, each once,
/// in state order. `None` maps every state (the cut base).
fn derive_pd_arena(
    inp: &TreeInputs<'_>,
    cone: &crate::engine::l4::pd_solve::ConeArena,
    subjects: Option<&std::collections::BTreeSet<String>>,
    typed_edges: &[SnapshotGraphEdge],
    capability_facts: &[SnapshotCapabilityFact],
    skipped: &SkippedEdgeKeys,
) -> PdArena {
    use crate::engine::l5::detector_context::cones_census;
    if cone.states.is_empty() {
        return PdArena::default();
    }
    let map = inp.routine_to_stable;
    // The kept states and their new indexes (identity when unscoped).
    let keep: Vec<bool> = match subjects {
        None => vec![true; cone.states.len()],
        Some(subjects) => {
            let mut keep = vec![false; cone.states.len()];
            let mut stack: Vec<usize> = subjects
                .iter()
                .filter_map(|s| cone.by_routine.get(s.as_str()))
                .flatten()
                .copied()
                .collect();
            while let Some(ix) = stack.pop() {
                if std::mem::replace(&mut keep[ix], true) {
                    continue;
                }
                stack.extend(
                    cone.links[cone.states[ix].links.clone()]
                        .iter()
                        .map(|l| l.callee_state),
                );
            }
            keep
        }
    };
    let new_ix: Vec<usize> = keep
        .iter()
        .scan(0usize, |n, &k| {
            let ix = *n;
            *n += usize::from(k);
            Some(ix)
        })
        .collect();
    // The key must be unique: a duplicate makes the index ambiguous and
    // silently points a link at the wrong edge (event-dispatch edges carry no
    // call site, so only `derive_typed_edges` keeps them apart).
    let mut edge_ix: HashMap<PdEdgeKey, EdgeIx> = HashMap::with_capacity(typed_edges.len());
    for (ix, e) in typed_edges.iter().enumerate() {
        let Some(to) = e.to_endpoint() else { continue };
        let key = (
            e.kind_str(),
            e.source_routine(),
            to,
            e.callsite_id().unwrap_or(""),
            e.edge_event_id().unwrap_or(""),
            e.edge_operation_id().unwrap_or(""),
        );
        let prev = edge_ix.insert(key, ix);
        debug_assert!(
            prev.is_none(),
            "pd arena: snapshot edges {prev:?} and {ix} share the key {key:?}"
        );
    }
    // (owner stable id, operation id) → the owner's direct fact.
    let mut direct_fact_ix: HashMap<(&str, &str), usize> = HashMap::new();
    for (ix, f) in capability_facts.iter().enumerate() {
        if let ("direct", Some(op)) = (f.provenance.as_str(), f.witness_operation_id.as_deref()) {
            direct_fact_ix.entry((f.subject.as_str(), op)).or_insert(ix);
        }
    }

    let mut out = PdArena::default();
    let mut unmapped_edges: std::collections::HashSet<usize> = std::collections::HashSet::new();
    for (ix, s) in cone.states.iter().enumerate() {
        if !keep[ix] {
            continue;
        }
        let ix = new_ix[ix];
        let producer = &cone.producers[s.producer];
        let routine = stable_routine_id(&s.routine, map);
        let owner = stable_routine_id(&producer.owner, map);
        let producer_fact = *direct_fact_ix
            .get(&(owner.as_str(), producer.operation_id.as_str()))
            .expect("a producer is its owner's direct fact");
        let start = out.links.len();
        for l in &cone.links[s.links.clone()] {
            let edge = &cone.edges[l.edge];
            let to = stable_routine_id(&edge.to, map);
            let event = match edge.kind.as_str() {
                "event-dispatch" => edge.event_id.as_deref().unwrap_or(""),
                _ => "",
            };
            let key = (
                edge.kind.as_str(),
                routine.as_str(),
                to.as_str(),
                edge.callsite.as_deref().unwrap_or(""),
                event,
                edge.operation_id.as_deref().unwrap_or(""),
            );
            match edge_ix.get(&key) {
                Some(&e) => out.links.push((new_ix[l.callee_state], e)),
                None => {
                    // R6-4: a link may miss only a row skipped for want of an
                    // anchor (or an unmodelled kind).
                    debug_assert!(
                        skipped.contains(&(
                            key.0.to_string(),
                            key.1.to_string(),
                            key.2.to_string(),
                            key.3.to_string(),
                            key.4.to_string(),
                            key.5.to_string(),
                        )),
                        "pd arena: cone edge {key:?} has no snapshot edge and was not skipped"
                    );
                    cones_census::add(&cones_census::PD_EDGE_UNMAPPED, 1);
                    unmapped_edges.insert(l.edge);
                }
            }
        }
        out.by_routine.entry(routine.clone()).or_default().push(ix);
        out.states.push(StateNode {
            producer_fact,
            temp_state: s.t.to_kind(),
            routine,
            seed: s.seed,
            links: start..out.links.len(),
        });
    }
    cones_census::add(
        &cones_census::PD_EDGE_UNMAPPED_KEYS,
        unmapped_edges.len() as u64,
    );
    if cones_census::enabled() {
        // `PD_SNAPSHOT_BYTES` alone: charging `PD_SOLVE_BYTES_ARENA` here
        // would count the retained arena twice (#160 spec §4.1.1).
        cones_census::add(&cones_census::PD_SNAPSHOT_BYTES, pd_arena_bytes(&out));
    }
    out
}

/// The bytes a `PdArena` retains, counted like `pd_solve::arena_tally`: every
/// `Vec` and hash table at its capacity, plus string heap.
fn pd_arena_bytes(a: &PdArena) -> u64 {
    use std::mem::size_of;
    let state_heap: usize = a.states.iter().map(|s| s.routine.capacity()).sum();
    let by_routine: usize = a
        .by_routine
        .iter()
        .map(|(k, v)| k.capacity() + v.capacity() * size_of::<StateIx>())
        .sum::<usize>()
        + a.by_routine.capacity() * (size_of::<RoutineId>() + size_of::<Vec<StateIx>>() + 1);
    (a.states.capacity() * size_of::<StateNode>()
        + state_heap
        + a.links.capacity() * size_of::<(StateIx, EdgeIx)>()
        + by_routine) as u64
}

/// Project a combined-graph internal `ValueSource` (object-run targetIdSource)
/// to the ordered snapshot ValueSource. The combined-graph ValueSource has NO
/// ConstantVar/Parameter variants (those are capability-only), so map the five
/// it carries; table-field id kept INTERNAL (snapshot does not project it).
fn snap_combined_value_source(
    vs: &crate::engine::l4::combined_graph::ValueSource,
) -> SnapValueSource {
    use crate::engine::l4::combined_graph::ValueSource as CV;
    match vs {
        CV::Literal { value } => SnapValueSource::Literal {
            value: value.clone(),
        },
        CV::Enum { enum_name, member } => SnapValueSource::Enum {
            enum_name: enum_name.clone(),
            member: member.clone(),
        },
        CV::TableField {
            table_id,
            field_name,
        } => SnapValueSource::TableField {
            table_id: table_id.clone(),
            field_name: field_name.clone(),
        },
        CV::Expression => SnapValueSource::Expression,
        CV::Unknown => SnapValueSource::Unknown,
    }
}

// ---------------------------------------------------------------------------
// deriveOperationEvidence — REFERENCED-ONLY: emit only ops that appear as a
// CapabilityFact witnessOperationId (direct ∪ inherited). recordOperations
// OVERWRITE operationSites for the same opId (richer displayText). Internal ids.
//
// Sort by operationId (al-sem); ordinal (M8 contract).
// ---------------------------------------------------------------------------

fn derive_operation_evidence<'r>(
    base: &TreeInputs<'_>,
    raw: impl Fn(&str) -> Option<&'r [CapabilityFact]>,
    collided: &CollidedRows<'_>,
) -> Vec<SnapshotOperationEvidence> {
    use std::collections::BTreeSet;

    // Referenced op ids = witnessOperationId of any direct ∪ inherited fact.
    let mut referenced: BTreeSet<String> = BTreeSet::new();
    for r in base.ws_routines {
        if !base.coverage.contains_key(&r.id) {
            continue;
        }
        if let Some(direct) = base.direct_full.get(&r.id) {
            for f in direct {
                if let Some(w) = &f.witness_operation_id {
                    referenced.insert(w.clone());
                }
            }
        }
        if let Some(inherited) = raw(&r.id) {
            for f in inherited {
                if let Some(w) = &f.witness_operation_id {
                    referenced.insert(w.clone());
                }
            }
        }
    }
    // #96 D4: the firing op of every implicit-trigger edge, so a trigger hop's
    // control context (digest conditionality) is in the index.
    for e in base.typed_edges {
        if let Some(op) = &e.operation_id {
            referenced.insert(op.clone());
        }
    }

    // byId keyed by operationId — operationSites first, recordOperations overwrite.
    let mut by_id: std::collections::BTreeMap<String, SnapshotOperationEvidence> =
        std::collections::BTreeMap::new();

    // A collided id's op is its LAST OWNING row's (the later row overwrites),
    // with that row's `order`, shifted into the id's one frame table.
    for (ri, r) in base.ws_routines.iter().enumerate() {
        let stable = stable_routine_id(&r.id, base.routine_to_stable);
        // Re-stamp op/callsite order (L3 assembly does not).
        let order = compute_routine_order(r);
        let op_order_by_id: HashMap<&str, &OperationOrder> = order
            .operation_sites
            .iter()
            .filter_map(|op| op.order.as_ref().map(|o| (op.id.as_str(), o)))
            .collect();

        // operationSites first.
        for op in &order.operation_sites {
            if !referenced.contains(&op.id) {
                continue;
            }
            let a = &op.source_anchor;
            let entry = SnapshotOperationEvidence {
                operation_id: op.id.clone(),
                routine: stable.clone(),
                source_file: a.source_unit_id.clone(),
                start_line: a.start_line,
                start_column: a.start_column,
                end_line: a.end_line,
                end_column: a.end_column,
                display_text: op.kind.clone(),
                control_context: op.control_context.clone(),
                order: collided.order(ri, op.order),
                under_asserterror: if op.under_asserterror == Some(true) {
                    Some(true)
                } else {
                    None
                },
            };
            by_id.insert(op.id.clone(), entry);
        }
        // recordOperations OVERWRITE the operationSites entry for the same id
        // (richer displayText `Var.Op`); preserves controlContext + order from the
        // matching operationSite.
        for ro in &r.record_operations {
            if !referenced.contains(&ro.id) {
                continue;
            }
            let matching = r.operation_sites.iter().find(|op| op.id == ro.id);
            let a = &ro.source_anchor;
            let display = format!(
                "{}.{}",
                if ro.record_variable_name.is_empty() {
                    "?".to_string()
                } else {
                    ro.record_variable_name.clone()
                },
                ro.op
            );
            let entry = SnapshotOperationEvidence {
                operation_id: ro.id.clone(),
                routine: stable.clone(),
                source_file: a.source_unit_id.clone(),
                start_line: a.start_line,
                start_column: a.start_column,
                end_line: a.end_line,
                end_column: a.end_column,
                display_text: display,
                control_context: matching.and_then(|op| op.control_context.clone()),
                order: collided.order(
                    ri,
                    matching.and_then(|_| op_order_by_id.get(ro.id.as_str()).map(|o| **o)),
                ),
                under_asserterror: None,
            };
            by_id.insert(ro.id.clone(), entry);
        }
    }

    // BTreeMap iteration = sorted by operationId, ordinal (M8 contract).
    by_id.into_values().collect()
}

// ---------------------------------------------------------------------------
// deriveCallsiteEvidence — REFERENCED-ONLY: emit callsites that appear as a
// CapabilityFact witnessCallsiteId (direct ∪ inherited) OR as a typed-edge
// callsiteId anchor. Internal ids. Sort by callsiteId.
//
// Ordinal (M8 contract).
// ---------------------------------------------------------------------------

fn derive_callsite_evidence<'r>(
    base: &TreeInputs<'_>,
    raw: impl Fn(&str) -> Option<&'r [CapabilityFact]>,
    collided: &CollidedRows<'_>,
) -> Vec<SnapshotCallsiteEvidence> {
    use std::collections::BTreeSet;

    let mut referenced: BTreeSet<String> = BTreeSet::new();
    for r in base.ws_routines {
        if !base.coverage.contains_key(&r.id) {
            continue;
        }
        if let Some(direct) = base.direct_full.get(&r.id) {
            for f in direct {
                if let Some(w) = &f.witness_callsite_id {
                    referenced.insert(w.clone());
                }
            }
        }
        if let Some(inherited) = raw(&r.id) {
            for f in inherited {
                if let Some(w) = &f.witness_callsite_id {
                    referenced.insert(w.clone());
                }
            }
        }
    }
    // Typed-edge callsite anchors.
    for e in base.typed_edges {
        if let Some(cid) = &e.callsite_id {
            referenced.insert(cid.clone());
        }
    }

    let mut out: Vec<SnapshotCallsiteEvidence> = Vec::new();
    for (ri, r) in base.ws_routines.iter().enumerate() {
        let stable = stable_routine_id(&r.id, base.routine_to_stable);
        let order = compute_routine_order(r);
        for cs in &order.call_sites {
            if !referenced.contains(&cs.id) {
                continue;
            }
            let a = &cs.source_anchor;
            out.push(SnapshotCallsiteEvidence {
                callsite_id: cs.id.clone(),
                routine: stable.clone(),
                source_file: a.source_unit_id.clone(),
                start_line: a.start_line,
                start_column: a.start_column,
                end_line: a.end_line,
                end_column: a.end_column,
                callee_display: render_callee(&cs.callee),
                control_context: cs.control_context.clone(),
                order: collided.order(ri, cs.order),
                under_asserterror: if cs.under_asserterror == Some(true) {
                    Some(true)
                } else {
                    None
                },
            });
        }
    }
    // A call-site id names one routine id, so only the rows of a collided id tie
    // here: their copies are ordered by full value and equal ones collapse (class
    // K, #160 step 11b §4.6).
    out.sort_by(|a, b| a.callsite_id.cmp(&b.callsite_id).then_with(|| a.cmp(b)));
    out.dedup();
    out
}

// ---------------------------------------------------------------------------
// deriveCallsiteResolutions — one row per syntactic callsite in model.callGraph.
// Groups by callsiteId; skips implicit-trigger edges. Status mapping per al-sem
// `callsite-resolutions.ts`. resolvedEdges = matching typed-edge edgeIds.
//
// Sort by (from|callsiteId); ordinal (M8 contract).
// ---------------------------------------------------------------------------

fn map_dispatch_kind(dk: &str) -> &'static str {
    match dk {
        "direct" => "direct",
        "method" | "interface" => "method",
        "codeunit-run" => "codeunit-run",
        "page-run" => "page-run",
        "report-run" => "report-run",
        "dynamic" => "unresolved",
        _ => "unresolved",
    }
}

fn derive_callsite_resolutions(
    base: &Ledger<'_>,
    typed_edges: &[SnapshotGraphEdge],
    collided: &CollidedRows<'_>,
) -> Vec<SnapshotCallsiteResolution> {
    use std::collections::BTreeMap;

    let map = base.routine_to_stable;

    // callsiteId → typed-edge edgeIds (for resolvedEdges).
    let mut edges_by_callsite: HashMap<String, Vec<String>> = HashMap::new();
    // callsiteId → interface-dispatch (kind == "interface-dispatch") edgeIds + `to`s.
    let mut iface_edges_by_callsite: HashMap<String, Vec<(String, Option<String>)>> =
        HashMap::new();
    for e in typed_edges {
        let Some(cid) = e.callsite_id() else {
            continue;
        };
        let eid = e.edge_id().to_string();
        edges_by_callsite
            .entry(cid.to_string())
            .or_default()
            .push(eid.clone());
        if e.kind_str() == "interface-dispatch" {
            let to = e.to_endpoint().map(|s| s.to_string());
            iface_edges_by_callsite
                .entry(cid.to_string())
                .or_default()
                .push((eid, to));
        }
    }

    // callsiteId → calleeDisplay / resultConsumed / underAsserterror from callSites.
    // A collided id's site reads every piece from its LAST OWNING row (#160 step
    // 11b §4.6, class P), as its representative edge below does.
    let mut callee_display: HashMap<String, String> = HashMap::new();
    let mut result_consumed: HashMap<String, bool> = HashMap::new();
    let mut under_asserterror: HashMap<String, bool> = HashMap::new();
    for (ri, r) in base.ws_routines.iter().enumerate() {
        for cs in &r.call_sites {
            if !collided.owns_site(ri, &cs.id) {
                continue;
            }
            callee_display.insert(cs.id.clone(), display_of_callee(&cs.callee));
            if let Some(rc) = cs.result_consumed {
                result_consumed.insert(cs.id.clone(), rc);
            }
            if cs.under_asserterror == Some(true) {
                under_asserterror.insert(cs.id.clone(), true);
            }
        }
    }

    let closed_sites = crate::program::model::calls::closed_interface_sites(
        base.calls.iter().chain(base.dependency_calls),
    );
    // Group callGraph edges by callsiteId (skip implicit-trigger). Preserve first.
    // BTreeMap for deterministic group iteration (M1). Every caller's calls:
    // the model's, then the dependency routines' (D6, Decision 18).
    let mut groups: BTreeMap<String, Vec<&crate::program::model::calls::CallEdge>> =
        BTreeMap::new();
    let mut group_order: Vec<String> = Vec::new();
    for ce in base.calls.iter().chain(base.dependency_calls) {
        if ce.dispatch_kind == DispatchKind::ImplicitTrigger {
            continue;
        }
        let key = ce.callsite_id.clone();
        if !groups.contains_key(&key) {
            group_order.push(key.clone());
        }
        groups.entry(key).or_default().push(ce);
    }

    let mut out: Vec<SnapshotCallsiteResolution> = Vec::new();
    let mut grouped_callsites: std::collections::BTreeSet<String> =
        std::collections::BTreeSet::new();

    for key in &group_order {
        grouped_callsites.insert(key.clone());
        let mut group_edges = &groups[key];
        // A collided site: its LAST OWNING row's edges, when that row has any.
        let owned: Vec<&crate::program::model::calls::CallEdge>;
        if let Some(&o) = collided.site_owner.get(key.as_str()) {
            owned = group_edges
                .iter()
                .copied()
                .filter(|e| e.from_ix == o)
                .collect();
            if !owned.is_empty() {
                group_edges = &owned;
            }
        }
        let representative = group_edges[0];
        let callsite_key = representative.callsite_id.clone();
        let from = stable_routine_id(&representative.from, map);
        let cdisplay = callee_display
            .get(&callsite_key)
            .cloned()
            .unwrap_or_default();
        let dispatch_kind = map_dispatch_kind(representative.dispatch_kind.as_str()).to_string();
        let rc = result_consumed.get(&callsite_key).copied();
        let ua = under_asserterror.get(&callsite_key).copied() == Some(true);

        // Interface dispatch group → polymorphic.
        let has_interface = group_edges
            .iter()
            .any(|e| e.dispatch_kind == DispatchKind::Interface);
        if has_interface {
            let empty: Vec<(String, Option<String>)> = Vec::new();
            let iface = iface_edges_by_callsite.get(&callsite_key).unwrap_or(&empty);
            let mut resolved_edges: Vec<String> =
                iface.iter().map(|(eid, _)| eid.clone()).collect();
            resolved_edges.sort();
            let mut candidates: Vec<String> =
                iface.iter().filter_map(|(_, to)| to.clone()).collect();
            candidates.sort();
            let unresolved = representative
                .dispatch_meta
                .as_ref()
                .map(|dm| dm.unresolved_impls.clone())
                .unwrap_or_default();
            // #264: closed in every row (the combined graph's rule too).
            let closed = closed_sites.contains(callsite_key.as_str());
            out.push(SnapshotCallsiteResolution {
                callsite_id: callsite_key.clone(),
                from,
                callee_display: cdisplay,
                dispatch_kind,
                status: "polymorphic".to_string(),
                resolved_edges,
                candidates: if candidates.is_empty() {
                    None
                } else {
                    Some(candidates)
                },
                open_world: Some(!closed),
                unresolved_candidates: if unresolved.is_empty() {
                    None
                } else {
                    Some(
                        unresolved
                            .into_iter()
                            .map(|(object_id, reason)| SnapshotUnresolvedCandidate {
                                object_id,
                                reason,
                            })
                            .collect(),
                    )
                },
                result_consumed: rc,
                under_asserterror: if ua { Some(true) } else { None },
                publisher_scoped: representative
                    .dispatch_meta
                    .as_ref()
                    .is_some_and(|dm| dm.publisher_scoped),
            });
            continue;
        }

        // D6: every target a body-less dependency row.
        let into_unfetched = base.bodyless_dependency.is_some_and(|b| {
            group_edges
                .iter()
                .all(|e| e.to.as_deref().is_some_and(|t| b.contains(t)))
        });
        out.push(classify_resolution(
            representative,
            &from,
            &cdisplay,
            &dispatch_kind,
            map,
            &edges_by_callsite,
            rc,
            ua,
            into_unfetched,
        ));
    }

    // Third pass: dep-injected callsiteIds appearing in typedEdges but not in
    // callGraph → "resolved". Source-only has no injected dep edges, but mirror the
    // pass for parity. (No-op for the corpus.) Iterate a SORTED key list (M1: no
    // HashMap iteration into output).
    let mut leftover_keys: Vec<&String> = edges_by_callsite.keys().collect();
    leftover_keys.sort();
    for callsite_key in leftover_keys {
        if grouped_callsites.contains(callsite_key) {
            continue;
        }
        let eids = &edges_by_callsite[callsite_key];
        // Find the `from` (already stable) of the first typed edge for this callsite.
        let from_internal = typed_edges.iter().find_map(|e| {
            if e.callsite_id() == Some(callsite_key.as_str()) {
                Some(e.source_routine().to_string())
            } else {
                None
            }
        });
        let Some(from_stable) = from_internal else {
            continue;
        };
        let mut resolved_edges = eids.clone();
        resolved_edges.sort();
        out.push(SnapshotCallsiteResolution {
            callsite_id: callsite_key.clone(),
            from: from_stable,
            callee_display: callee_display
                .get(callsite_key)
                .cloned()
                .unwrap_or_default(),
            dispatch_kind: "direct".to_string(),
            status: "resolved".to_string(),
            resolved_edges,
            candidates: None,
            open_world: None,
            unresolved_candidates: None,
            result_consumed: None,
            under_asserterror: None,
            publisher_scoped: false,
        });
    }

    out.sort_by(|a, b| {
        format!("{}|{}", a.from, a.callsite_id).cmp(&format!("{}|{}", b.from, b.callsite_id))
    });
    out
}

#[allow(clippy::too_many_arguments)]
fn classify_resolution(
    ce: &crate::program::model::calls::CallEdge,
    from: &str,
    callee_display: &str,
    dispatch_kind: &str,
    map: &HashMap<String, String>,
    edges_by_callsite: &HashMap<String, Vec<String>>,
    result_consumed: Option<bool>,
    under_asserterror: bool,
    into_unfetched: bool,
) -> SnapshotCallsiteResolution {
    let callsite_key = &ce.callsite_id;
    let ua = if under_asserterror { Some(true) } else { None };

    let base_row = |status: &str,
                    resolved_edges: Vec<String>,
                    candidates: Option<Vec<String>>|
     -> SnapshotCallsiteResolution {
        SnapshotCallsiteResolution {
            callsite_id: callsite_key.clone(),
            from: from.to_string(),
            callee_display: callee_display.to_string(),
            dispatch_kind: dispatch_kind.to_string(),
            status: status.to_string(),
            resolved_edges,
            candidates,
            open_world: None,
            unresolved_candidates: None,
            result_consumed,
            under_asserterror: ua,
            publisher_scoped: false,
        }
    };

    match ce.resolution.as_str() {
        "resolved" => {
            let mut edges = edges_by_callsite
                .get(callsite_key)
                .cloned()
                .unwrap_or_default();
            edges.sort();
            // D6: resolved only into body-less dependency rows (with its edges).
            let status = if into_unfetched {
                "unfetched-dependency"
            } else {
                "resolved"
            };
            base_row(status, edges, None)
        }
        "ambiguous" => {
            let mut cands: Vec<String> = ce
                .candidates
                .clone()
                .unwrap_or_default()
                .iter()
                .map(|id| stable_routine_id(id, map))
                .collect();
            cands.sort();
            base_row(
                "ambiguous",
                Vec::new(),
                if cands.is_empty() { None } else { Some(cands) },
            )
        }
        "member-not-found" => {
            let mut cands: Vec<String> = ce
                .candidates
                .clone()
                .unwrap_or_default()
                .iter()
                .map(|id| stable_routine_id(id, map))
                .collect();
            cands.sort();
            base_row(
                "unresolved-member",
                Vec::new(),
                if cands.is_empty() { None } else { Some(cands) },
            )
        }
        "external-target" => base_row("external", Vec::new(), None),
        "opaque" => base_row("unfetched-dependency", Vec::new(), None),
        "builtin" => base_row("builtin", Vec::new(), None),
        _ => {
            if ce.dispatch_kind == DispatchKind::Dynamic {
                base_row("dynamic-target", Vec::new(), None)
            } else {
                base_row("unresolved-receiver-type", Vec::new(), None)
            }
        }
    }
}

// ---------------------------------------------------------------------------
// deriveAnalysisGaps — non-callsite unknowns. Sort by (kind|subject), dedup
// adjacent. Every gap of the model, unfiltered (#220 I8: prove's gap count
// intersects it with its own cone; the cross-app serialization filters it by
// reach). #160 step 9 D9: an app-level `symbol-only-boundary` names only a
// source-less app when the caller passes `source_less_apps`.
//
// Sort by kind | subject; ordinal (M8 contract).
// A subject keeps the app guid's model casing (`encode_object_id` keeps it
// verbatim): `digest_cli::count_gaps_in_cone` (prove's gap count) matches an
// app subject as a case-sensitive `{guid}:` prefix of the cone's stable ids, so
// lowercasing it here would hide the gap of an upper-case manifest guid.
// ---------------------------------------------------------------------------

fn derive_analysis_gaps(resolved: &Model, base: &TreeInputs<'_>) -> Vec<SnapshotAnalysisGap> {
    use crate::program::model::role::Role;
    use std::collections::BTreeSet;

    let map = &base.routine_to_stable;
    let is_dependency_role = |r: &ModelRoutine| resolved.routine_role(r) == Role::Dependency;
    let dependency = || Role::Dependency.as_str().to_string();
    let mut out: Vec<SnapshotAnalysisGap> = Vec::new();

    // Source-only: no coverage.routinesParseIncomplete plumbed through the base, so
    // derive parse-incomplete from routine.parse_incomplete (the model's own flag).
    for r in base.ws_routines {
        if r.parse_incomplete {
            out.push(SnapshotAnalysisGap {
                kind: "parse-incomplete".to_string(),
                subject: stable_routine_id(&r.id, map),
                detail: "routine body had a parse error; structural extraction was partial"
                    .to_string(),
                role: resolved.routine_role(r).as_str().to_string(),
            });
        }
    }

    // Symbol-only apps: any app guid in a body-unavailable dependency routine.
    // A routine whose body a parse error lost is not a symbol: it has its
    // `parse-incomplete` gap above (final panel C1, as the loop below).
    // #160 step 9 D9: with a cross-app base's set, only an app with no source
    // (F21: "an opaque boundary is a source-less app"); a source app's
    // body-less row gets the `body-unavailable` gap below.
    let source_less = |guid: &str| {
        base.source_less_apps
            .is_none_or(|s| s.contains(&guid.to_ascii_lowercase()))
    };
    let mut opaque_apps: BTreeSet<String> = BTreeSet::new();
    for r in base.ws_routines {
        if !r.body_available
            && !r.parse_incomplete
            && is_dependency_role(r)
            && crate::program::model::platform_natives::pure_native(r).is_none()
            && let Some(guid) = object_id_app_guid(&r.object_id)
            && source_less(guid)
        {
            opaque_apps.insert(guid.to_string());
        }
    }
    for guid in &opaque_apps {
        out.push(SnapshotAnalysisGap {
            kind: "symbol-only-boundary".to_string(),
            subject: guid.clone(),
            detail: "dependency app has symbols only (no parseable source); its cones are opaque"
                .to_string(),
            role: dependency(),
        });
    }

    // Body-unavailable dependency routines not covered by an app-level gap.
    for r in base.ws_routines {
        if r.body_available || r.parse_incomplete {
            continue;
        }
        if !is_dependency_role(r)
            || crate::program::model::platform_natives::pure_native(r).is_some()
        {
            continue;
        }
        if object_id_app_guid(&r.object_id).is_some_and(|g| opaque_apps.contains(g)) {
            continue;
        }
        out.push(SnapshotAnalysisGap {
            kind: "body-unavailable".to_string(),
            subject: stable_routine_id(&r.id, map),
            detail: "routine has no parseable body".to_string(),
            role: dependency(),
        });
    }

    out.sort_by(|a, b| {
        format!("{}|{}", a.kind, a.subject).cmp(&format!("{}|{}", b.kind, b.subject))
    });
    // dedup adjacent (kind + subject).
    let mut deduped: Vec<SnapshotAnalysisGap> = Vec::new();
    for g in out {
        match deduped.last() {
            Some(prev) if prev.kind == g.kind && prev.subject == g.subject => {}
            _ => deduped.push(g),
        }
    }
    deduped
}

// ---------------------------------------------------------------------------
// deriveCoverage — per-routine cone coverage, subject already stable on the cone.
// Sort by subject; ordinal (M8 contract).
// ---------------------------------------------------------------------------

fn derive_coverage(base: &TreeInputs<'_>) -> Vec<SnapshotCoverageRecord> {
    let map = &base.routine_to_stable;
    let mut out: Vec<SnapshotCoverageRecord> = Vec::new();
    // The cone is per id, so the rows of a collided id would emit equal copies:
    // each id is emitted once (class K, #160 step 11b §4.6).
    let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for r in base.ws_routines {
        let Some(cov) = base.coverage.get(&r.id) else {
            continue;
        };
        if !seen.insert(r.id.as_str()) {
            continue;
        }
        let mut reasons = cov.reasons.clone();
        reasons.sort();
        let mut unknown_targets: Vec<String> = cov
            .unknown_targets
            .iter()
            .map(|t| stable_routine_id(t, map))
            .collect();
        unknown_targets.sort();
        out.push(SnapshotCoverageRecord {
            subject: stable_routine_id(&r.id, map),
            direct_status: cov.direct_status.clone(),
            inherited_status: cov.inherited_status.clone(),
            reasons,
            unknown_targets,
        });
    }
    out.sort_by(|a, b| a.subject.cmp(&b.subject));
    out
}

// ---------------------------------------------------------------------------
// deriveEventDeclarations — bipartite publisher/subscriber decls from the event
// graph. Sort by (kind|routine|eventId); ordinal (M8 contract).
// ---------------------------------------------------------------------------

fn derive_event_declarations(base: &TreeInputs<'_>) -> Vec<SnapshotEventDeclaration> {
    let map = &base.routine_to_stable;

    // routine id → internal id (for enclosingRoutineId) + source anchor.
    let mut routine_anchor: HashMap<&str, &PAnchor> = HashMap::new();
    for r in base.ws_routines {
        routine_anchor.insert(r.id.as_str(), &r.source_anchor);
    }

    let event_by_id: HashMap<&str, &EventSymbol> = base
        .event_graph
        .events
        .iter()
        .map(|e| (e.id.as_str(), e))
        .collect();

    let mut out: Vec<SnapshotEventDeclaration> = Vec::new();

    // Publisher entries — one per EventSymbol with a publisherRoutineId.
    for evt in &base.event_graph.events {
        let Some(pub_rid) = &evt.publisher_routine_id else {
            continue;
        };
        let Some(anchor) = routine_anchor.get(pub_rid.as_str()) else {
            continue;
        };
        let stable_routine = stable_routine_id(pub_rid, map);
        let event_id = stable_event_id_for(evt);
        out.push(SnapshotEventDeclaration {
            kind: "publisher".to_string(),
            routine: stable_routine,
            event_id,
            binding: None,
            source_anchor: anchor_from_panchor(anchor, pub_rid),
        });
    }

    // Subscriber entries — one per EventEdge.
    for edge in &base.event_graph.edges {
        let sub_rid = &edge.subscriber_routine_id;
        let Some(anchor) = routine_anchor.get(sub_rid.as_str()) else {
            continue;
        };
        let stable_routine = stable_routine_id(sub_rid, map);

        let evt = event_by_id.get(edge.event_id.as_str()).copied();
        let stable_publisher_object = match evt {
            Some(e) => to_stable_object_id(&e.publisher_object_id),
            None => edge.event_id.clone(),
        };
        let event_name = evt.map(|e| e.event_name.clone()).unwrap_or_default();
        let signature_hash = evt.map(|e| e.signature_hash.clone()).unwrap_or_default();
        let event_id = format!("{stable_publisher_object}::{event_name}::{signature_hash}");

        out.push(SnapshotEventDeclaration {
            kind: "subscriber".to_string(),
            routine: stable_routine,
            event_id,
            binding: Some(SnapshotSubscriberBinding {
                publisher_object: stable_publisher_object,
                event_name,
            }),
            source_anchor: anchor_from_panchor(anchor, sub_rid),
        });
    }

    out.sort_by(|a, b| {
        format!("{}|{}|{}", a.kind, a.routine, a.event_id)
            .cmp(&format!("{}|{}|{}", b.kind, b.routine, b.event_id))
    });
    out
}

// ---------------------------------------------------------------------------
// deriveRootClassifications — Stage-0 root classifications, rewrite routineId →
// stable, RECONSTRUCT sourceAnchor (the snapshot RootClassificationSlot carries
// it; the Stage-0 RootClassification dropped it). sourceAnchor = the routine's
// own declaration anchor (al-sem `classifyRoots`: `sourceAnchor:
// routine.sourceAnchor`), with enclosingRoutineId = the routine's INTERNAL id.
//
// Sort by routineId; ordinal (M8 contract).
// ---------------------------------------------------------------------------

fn derive_root_classifications(
    resolved: &Model,
    base: &TreeInputs<'_>,
) -> Vec<SnapshotRootClassificationSlot> {
    let map = &base.routine_to_stable;

    // internal routine id → (&PAnchor, internal id) for the sourceAnchor.
    let mut routine_by_id: HashMap<&str, &ModelRoutine> = HashMap::new();
    for r in base.ws_routines {
        routine_by_id.insert(r.id.as_str(), r);
    }

    let mut out: Vec<SnapshotRootClassificationSlot> = Vec::new();
    for c in &resolved.root_classifications {
        let Some(stable) = map.get(&c.routine_id) else {
            continue;
        };
        let source_anchor = routine_by_id
            .get(c.routine_id.as_str())
            .map(|r| anchor_from_panchor(&r.source_anchor, &r.id));
        out.push(SnapshotRootClassificationSlot {
            routine_id: stable.clone(),
            kinds: c.kinds.clone(),
            externally_reachable: c.externally_reachable,
            source: c.source.clone(),
            confidence: c.confidence.clone(),
            source_anchor,
            config_entry_id: c.config_entry_id.clone(),
            resolution_status: c.resolution_status.clone(),
        });
    }
    out.sort_by(|a, b| a.routine_id.cmp(&b.routine_id));
    out
}

// ---------------------------------------------------------------------------
// deriveRoutineOrderFrames — per-routine scope-frame table, keyed by
// StableRoutineId, only routines with >=1 frame. Absent when none.
//
// Emitted as a serde_json Map with keys in sorted order (M1: explicit sort, no
// HashMap iteration); ordinal (M8 contract).
// ---------------------------------------------------------------------------

fn derive_routine_order_frames(
    base: &TreeInputs<'_>,
    collided: &CollidedRows<'_>,
) -> Option<RoutineOrderFrames> {
    use std::collections::BTreeMap;
    // BTreeMap keyed by stable routine id → sorted key order (ordinal, M8
    // contract; M1: BTreeMap iteration is sorted, not hash-ordered).
    let mut entries: BTreeMap<String, Vec<ScopeFrame>> = BTreeMap::new();
    for r in base.ws_routines {
        // A collided id's one table holds every row's frames (`CollidedRows`).
        let frames = match collided.frames.get(r.id.as_str()) {
            Some(table) => table.clone(),
            None => compute_routine_order(r).scope_frames,
        };
        if frames.is_empty() {
            continue;
        }
        let stable = stable_routine_id(&r.id, base.routine_to_stable);
        entries.insert(stable, frames);
    }
    if entries.is_empty() {
        return None;
    }
    Some(RoutineOrderFrames {
        entries: entries.into_iter().collect(),
    })
}

// ===========================================================================
// R4-F STABLE PROJECTION — project_r4f_snapshot. Emits the top-level key order
// al-sem `R4FSnapshotProjection` uses: fixtureName, capabilityFactCount,
// typedEdgeCount, operationIndexCount, callsiteIndexCount, callsiteResolutionCount,
// analysisGapCount, coverageCount, eventDeclarationCount, rootClassificationCount,
// hasRoutineOrderFrames, snapshot{ identities, capabilityFacts, typedEdges,
// operationIndex, callsiteIndex, callsiteResolutions, analysisGaps, coverage,
// eventDeclarations, rootClassifications, [routineOrderFrames] }.
//
// Arrays are emitted VERBATIM as the derivers produced them (the deriver sort IS
// the parity surface; no re-sort here).
// ===========================================================================

/// The consumed-core `snapshot` object, in FIXED key order (serde derives field
/// order from declaration order — survives serde_json's `preserve_order` being OFF).
#[derive(Debug, Clone, Serialize)]
struct SnapshotEnvelope<'a> {
    identities: &'a SnapshotIdentityTable,
    #[serde(rename = "capabilityFacts")]
    capability_facts: &'a [SnapshotCapabilityFact],
    #[serde(rename = "typedEdges")]
    typed_edges: &'a [SnapshotGraphEdge],
    #[serde(rename = "operationIndex")]
    operation_index: &'a [SnapshotOperationEvidence],
    #[serde(rename = "callsiteIndex")]
    callsite_index: &'a [SnapshotCallsiteEvidence],
    #[serde(rename = "callsiteResolutions")]
    callsite_resolutions: &'a [SnapshotCallsiteResolution],
    #[serde(rename = "analysisGaps")]
    analysis_gaps: &'a [SnapshotAnalysisGap],
    coverage: &'a [SnapshotCoverageRecord],
    #[serde(rename = "eventDeclarations")]
    event_declarations: &'a [SnapshotEventDeclaration],
    #[serde(rename = "rootClassifications")]
    root_classifications: &'a [SnapshotRootClassificationSlot],
    #[serde(rename = "routineOrderFrames", skip_serializing_if = "Option::is_none")]
    routine_order_frames: &'a Option<RoutineOrderFrames>,
}

/// The full R4-F snapshot projection document, in FIXED key order.
#[derive(Debug, Clone, Serialize)]
struct R4FSnapshotProjection<'a> {
    #[serde(rename = "fixtureName")]
    fixture_name: &'a str,
    #[serde(rename = "capabilityFactCount")]
    capability_fact_count: usize,
    #[serde(rename = "typedEdgeCount")]
    typed_edge_count: usize,
    #[serde(rename = "operationIndexCount")]
    operation_index_count: usize,
    #[serde(rename = "callsiteIndexCount")]
    callsite_index_count: usize,
    #[serde(rename = "callsiteResolutionCount")]
    callsite_resolution_count: usize,
    #[serde(rename = "analysisGapCount")]
    analysis_gap_count: usize,
    #[serde(rename = "coverageCount")]
    coverage_count: usize,
    #[serde(rename = "eventDeclarationCount")]
    event_declaration_count: usize,
    #[serde(rename = "rootClassificationCount")]
    root_classification_count: usize,
    #[serde(rename = "hasRoutineOrderFrames")]
    has_routine_order_frames: bool,
    snapshot: SnapshotEnvelope<'a>,
}

/// Project a resolved source-only workspace to the R4-F snapshot differential
/// document, PRETTY-serialized with a trailing newline (the exact on-disk golden
/// form). Arrays are emitted VERBATIM as the derivers produced them (the deriver
/// sort IS the parity surface; no re-sort here).
///
/// Returns the serialized STRING (NOT a `serde_json::Value`): serde_json's
/// `preserve_order` is OFF for this target, so re-materializing through a
/// `Value` would alphabetize keys. Serializing the ordered struct directly emits
/// fields in declaration order. The differential test byte-compares this string.
pub fn project_r4f_snapshot(resolved: &Model, fixture_name: &str) -> String {
    let snap = compose_snapshot(resolved);

    let doc = R4FSnapshotProjection {
        fixture_name,
        capability_fact_count: snap.capability_facts.len(),
        typed_edge_count: snap.typed_edges.len(),
        operation_index_count: snap.operation_index.len(),
        callsite_index_count: snap.callsite_index.len(),
        callsite_resolution_count: snap.callsite_resolutions.len(),
        analysis_gap_count: snap.analysis_gaps.len(),
        coverage_count: snap.coverage.len(),
        event_declaration_count: snap.event_declarations.len(),
        root_classification_count: snap.root_classifications.len(),
        has_routine_order_frames: snap.routine_order_frames.is_some(),
        snapshot: SnapshotEnvelope {
            identities: &snap.identities,
            capability_facts: &snap.capability_facts,
            typed_edges: &snap.typed_edges,
            operation_index: &snap.operation_index,
            callsite_index: &snap.callsite_index,
            callsite_resolutions: &snap.callsite_resolutions,
            analysis_gaps: &snap.analysis_gaps,
            coverage: &snap.coverage,
            event_declarations: &snap.event_declarations,
            root_classifications: &snap.root_classifications,
            routine_order_frames: &snap.routine_order_frames,
        },
    };
    let mut s = serde_json::to_string_pretty(&doc).expect("serialize R4-F snapshot projection");
    s.push('\n');
    s
}

#[cfg(test)]
mod temp_rule_tests {
    use super::SnapTempState;
    use crate::program::body::features::known_temp_suppresses;

    /// #34: every SnapTempState variant reaches the one decision through its
    /// projection; only `Known { value: true }` suppresses.
    #[test]
    fn temp_rule_truth_table_snaptempstate() {
        let cases = [
            (SnapTempState::Known { value: true }, true),
            (SnapTempState::Known { value: false }, false),
            (
                SnapTempState::ParameterDependent { parameter_index: 0 },
                false,
            ),
            (SnapTempState::Unknown, false),
        ];
        for (state, want) in cases {
            assert_eq!(
                known_temp_suppresses(state.known_value()),
                want,
                "{state:?}"
            );
        }
    }
}

#[cfg(test)]
mod capability_fact_serialize_tests {
    //! Grounds the dynamic per-fact key order of `SnapshotCapabilityFact`'s custom
    //! `Serialize` for INHERITED facts of all three witness families — the corpus
    //! byte-verifies only inherited callsite-witness (crosshop HTTP), so these guard
    //! the op-witness + event tail order (witnessCallsiteId APPENDED after extra) that
    //! al-sem's `{...rep, ..., witnessCallsiteId}` spread produces. (R4-F Stage 2 review.)
    use super::*;

    fn idx(s: &str, key: &str) -> usize {
        s.find(&format!("\"{key}\""))
            .unwrap_or_else(|| panic!("key {key} absent in {s}"))
    }

    fn fact(resource_kind: &str, witness_op: Option<&str>) -> SnapshotCapabilityFact {
        SnapshotCapabilityFact {
            subject: "g:Codeunit:1#h".into(),
            op: "x".into(),
            resource_kind: resource_kind.into(),
            resource_id: None,
            resource_arg_source: None,
            confidence: "static".into(),
            provenance: "inherited".into(),
            via: "call".into(),
            witness_operation_id: witness_op.map(str::to_string),
            witness_callsite_id: Some("g/h/cs1".into()),
            extra: Some(SnapCapabilityExtra::Table {
                op_subtype: None,
                record_variable_id: None,
                temp_state: None,
            }),
            witness_first_hop_operation_id: None,
            witness_first_hop_event_id: None,
        }
    }

    #[test]
    fn head_order_provenance_before_via() {
        let s = serde_json::to_string(&fact("table", Some("g/h/op1"))).unwrap();
        assert!(idx(&s, "confidence") < idx(&s, "provenance"));
        assert!(idx(&s, "provenance") < idx(&s, "via"));
    }

    #[test]
    fn op_witness_inherited_tail_is_op_extra_callsite() {
        // op-witness (table/commit/error): witnessOperationId, extra, witnessCallsiteId(last).
        let s = serde_json::to_string(&fact("table", Some("g/h/op1"))).unwrap();
        assert!(idx(&s, "via") < idx(&s, "witnessOperationId"));
        assert!(idx(&s, "witnessOperationId") < idx(&s, "extra"));
        assert!(idx(&s, "extra") < idx(&s, "witnessCallsiteId"));
    }

    #[test]
    fn event_inherited_tail_is_extra_then_callsite() {
        // event family (no witnessOperationId): extra, then witnessCallsiteId(last).
        let s = serde_json::to_string(&fact("event", None)).unwrap();
        assert!(!s.contains("witnessOperationId"));
        assert!(idx(&s, "extra") < idx(&s, "witnessCallsiteId"));
    }

    #[test]
    fn callsite_witness_tail_is_callsite_then_extra() {
        // callsite-witness (http/ui/dispatch): witnessCallsiteId BEFORE extra (in-place).
        let s = serde_json::to_string(&fact("http", None)).unwrap();
        assert!(!s.contains("witnessOperationId"));
        assert!(idx(&s, "witnessCallsiteId") < idx(&s, "extra"));
    }
}

#[cfg(test)]
mod step6_tree_tests {
    use super::*;
    use crate::engine::l4::cone_derived::{ConeScope, WitnessScope};
    use crate::engine::l4::exec_relation::ExecRelation;
    use crate::engine::l5::detector_context::{compose_capabilities, det_cone_inputs};

    /// Every field of `s` as a comparable string; the pd arena's hash map in
    /// key order.
    fn fields(s: &CapabilitySnapshot) -> Vec<(&'static str, String)> {
        let by_routine: std::collections::BTreeMap<_, _> =
            s.pd_solution.by_routine.iter().collect();
        vec![
            ("identities", format!("{:?}", s.identities)),
            ("capability_facts", format!("{:?}", s.capability_facts)),
            ("typed_edges", format!("{:?}", s.typed_edges)),
            ("operation_index", format!("{:?}", s.operation_index)),
            ("callsite_index", format!("{:?}", s.callsite_index)),
            (
                "callsite_resolutions",
                format!("{:?}", s.callsite_resolutions),
            ),
            ("analysis_gaps", format!("{:?}", s.analysis_gaps)),
            ("coverage", format!("{:?}", s.coverage)),
            ("event_declarations", format!("{:?}", s.event_declarations)),
            (
                "root_classifications",
                format!("{:?}", s.root_classifications),
            ),
            (
                "routine_order_frames",
                format!("{:?}", s.routine_order_frames),
            ),
            ("pd.states", format!("{:?}", s.pd_solution.states)),
            ("pd.links", format!("{:?}", s.pd_solution.links)),
            ("pd.by_routine", format!("{by_routine:?}")),
        ]
    }

    /// #160 step 6 Task 1 (spec §4.4): on every `tests/r0-corpus/` app dir,
    /// single-app, `compose_tree` over the relation's cone with `{false, All}`
    /// equals today's cut snapshot (`compose_snapshot`) field by field.
    /// Preconditions: some dir has inherited facts and some has pd states, so
    /// the drain and the scoped arena are exercised.
    #[test]
    fn compose_tree_equals_the_cut_snapshot_single_app() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/r0-corpus");
        let mut dirs: Vec<std::path::PathBuf> = walkdir::WalkDir::new(&root)
            .into_iter()
            .map(|e| e.unwrap().into_path())
            .filter(|p| p.join("app.json").is_file())
            .collect();
        dirs.sort();
        assert!(!dirs.is_empty(), "precondition: r0 dirs under {root:?}");
        let (mut with_inherited, mut with_pd) = (0usize, 0usize);
        let mut failures: Vec<String> = Vec::new();
        for dir in &dirs {
            let where_ = dir.strip_prefix(&root).unwrap_or(dir).display().to_string();
            let inputs = crate::program::inputs::WorkspaceInputs::read(dir, &Default::default());
            let Some(m) =
                crate::program::model::program_calls::assemble_and_resolve_workspace_program(
                    &inputs,
                    crate::program::model::workspace::MODEL_INSTANCE_ID_DEFAULT,
                    false,
                    crate::program::model::workspace::AssemblyNeeds::BASE,
                )
            else {
                failures.push(format!("{where_}: no single-app model built"));
                continue;
            };
            let cut = compose_snapshot(&m);
            let rel = ExecRelation::single_app(&m);
            let (mut caps, raw) = compose_capabilities(
                &m,
                rel.cone(),
                det_cone_inputs(&m, &rel),
                ConeScope {
                    derived: false,
                    raw: WitnessScope::All,
                },
            );
            let pd = std::borrow::Cow::Owned(caps.pd.take().unwrap_or_default());
            let tree = compose_tree(
                &m,
                rel.cone(),
                &[],
                &caps,
                TreeRaw::Drain(raw),
                pd,
                &TreeApps::default(),
            );
            with_inherited += usize::from(
                cut.capability_facts
                    .iter()
                    .any(|f| f.provenance == "inherited"),
            );
            with_pd += usize::from(!cut.pd_solution.states.is_empty());
            for ((name, a), (_, b)) in fields(&cut).into_iter().zip(fields(&tree)) {
                if a != b {
                    failures.push(format!("{where_}: {name} differs"));
                }
            }
        }
        assert!(
            with_inherited > 0,
            "precondition: a dir with inherited facts"
        );
        assert!(with_pd > 0, "precondition: a dir with pd states");
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }

    /// `compose_callsite_resolutions` gives exactly the `callsite_resolutions`
    /// of `compose_snapshot`, on every `tests/r0-corpus/` app dir, single-app
    /// and cross-app (the CDO census in `program_resolve_harness` reads it on
    /// a cross-app model in place of the whole snapshot).
    #[test]
    fn callsite_resolutions_alone_equal_the_snapshot_rows() {
        use crate::program::model::program_calls::{
            assemble_and_resolve_cross_app_program, assemble_and_resolve_workspace_program,
        };
        use crate::program::model::workspace::{AssemblyNeeds, MODEL_INSTANCE_ID_DEFAULT};
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
        // `tests/fixtures` too: `test-selection-graph` is the one with an
        // interface closed over a non-extensible enum.
        let mut dirs: Vec<std::path::PathBuf> = ["r0-corpus", "fixtures"]
            .iter()
            .flat_map(|d| walkdir::WalkDir::new(root.join(d)).max_depth(3))
            .map(|e| e.unwrap().into_path())
            .filter(|p| p.join("app.json").is_file())
            .collect();
        dirs.sort();
        let (mut models, mut rows, mut closed) = (0usize, 0usize, 0usize);
        let mut failures: Vec<String> = Vec::new();
        for dir in &dirs {
            let where_ = dir.strip_prefix(&root).unwrap_or(dir).display().to_string();
            let inputs = crate::program::inputs::WorkspaceInputs::read(dir, &Default::default());
            let single = assemble_and_resolve_workspace_program(
                &inputs,
                MODEL_INSTANCE_ID_DEFAULT,
                false,
                AssemblyNeeds::BASE,
            );
            let cross = assemble_and_resolve_cross_app_program(
                &inputs,
                MODEL_INSTANCE_ID_DEFAULT,
                false,
                AssemblyNeeds::BASE,
            )
            .ok()
            .map(|r| r.resolved);
            for (mode, m) in [("single-app", single), ("cross-app", cross)] {
                let Some(m) = m else { continue };
                models += 1;
                let whole = compose_snapshot(&m).callsite_resolutions;
                let alone = compose_callsite_resolutions(&m);
                rows += whole.len();
                closed += whole
                    .iter()
                    .filter(|r| r.status == "polymorphic" && r.open_world == Some(false))
                    .count();
                if format!("{whole:?}") != format!("{alone:?}") {
                    failures.push(format!("{where_} ({mode}): the rows differ"));
                }
            }
        }
        assert!(models > 100, "precondition: {models} models composed");
        assert!(rows > 0, "precondition: some call-site rows");
        assert!(closed > 0, "precondition: a closed interface site");
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }

    /// T6 (R6-4, spec §4.5): a hand-stated cone edge `R → A` at `csX`, a call
    /// site absent from `R`'s `call_sites`, carrying a pd link (R's state
    /// reached from A's seed across it). `derive_typed_edges` keeps no row for
    /// it and records it skipped; the arena drops the link, counts
    /// `PD_EDGE_UNMAPPED` once, and the debug check passes (the key was
    /// skipped).
    #[test]
    fn t6_an_anchorless_cone_edge_has_no_row_and_its_link_is_dropped() {
        use crate::engine::l4::capability_cone::TypedOutEdge;
        use crate::engine::l4::pd_solve::{ConeArena, ConeLink, ConeState, Producer};
        use crate::engine::l4::summary::TempState;
        use crate::engine::l5::detector_context::cones_census;
        use crate::engine::l5::test_support::{fact, routine};
        use std::sync::atomic::Ordering;
        let ws_routines = vec![routine("R", "procedure"), routine("A", "procedure")];
        assert!(
            ws_routines.iter().all(|r| r.call_sites.is_empty()),
            "precondition: no routine has a call site"
        );
        let typed = vec![TypedEdge {
            kind: "direct-call".to_string(),
            from: "R".to_string(),
            to: Some("A".to_string()),
            callsite_id: Some("csX".to_string()),
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
        }];
        let routine_to_stable: HashMap<String, String> = [("R", "R#"), ("A", "A#")]
            .into_iter()
            .map(|(a, b)| (a.to_string(), b.to_string()))
            .collect();
        let event_graph = EventGraph::default();
        let (direct_full, coverage) = (HashMap::new(), HashMap::new());
        let inp = TreeInputs {
            ws_routines: &ws_routines,
            typed_edges: &typed,
            calls: &[],
            dependency_calls: &[],
            event_graph: &event_graph,
            direct_full: &direct_full,
            coverage: &coverage,
            routine_to_stable: &routine_to_stable,
            bodyless_dependency: None,
            source_less_apps: None,
        };
        let (rows, skipped) = derive_typed_edges(&inp.ledger());
        assert!(rows.is_empty(), "no typedEdges row: {rows:?}");
        assert_eq!(skipped.len(), 1, "the row is recorded skipped");

        let mut producer_fact = fact("insert", "table", Some("T"));
        producer_fact.subject = "A".to_string();
        producer_fact.witness_operation_id = Some("opA".to_string());
        let state = |routine: &str, seed: bool, links: std::ops::Range<usize>| ConeState {
            producer: 0,
            routine: std::sync::Arc::from(routine),
            t: TempState::Known(false),
            reqs: Vec::new(),
            seed,
            dist: usize::from(!seed),
            links,
        };
        let cone = ConeArena {
            producers: vec![Producer {
                fact: std::sync::Arc::new(producer_fact.clone()),
                owner: std::sync::Arc::from("A"),
                operation_id: "opA".to_string(),
            }],
            states: vec![state("A", true, 0..0), state("R", false, 0..1)],
            links: vec![ConeLink {
                callee_state: 0,
                edge: 0,
            }],
            edges: vec![TypedOutEdge::new(
                "A".to_string(),
                "direct-call".to_string(),
                Some("csX".to_string()),
                None,
                None,
            )],
            by_routine: [("A", 0), ("R", 1)]
                .into_iter()
                .map(|(r, s)| (std::sync::Arc::from(r), vec![s]))
                .collect(),
        };
        let facts = vec![snapshot_fact(&producer_fact, "A#")];
        let before = cones_census::PD_EDGE_UNMAPPED.load(Ordering::Relaxed);
        let arena = derive_pd_arena(&inp, &cone, None, &rows, &facts, &skipped);
        assert_eq!(
            cones_census::PD_EDGE_UNMAPPED.load(Ordering::Relaxed) - before,
            1,
            "the link is counted unmapped"
        );
        assert_eq!(arena.states.len(), 2, "both states are kept");
        assert!(arena.links.is_empty(), "the link is dropped");
    }

    /// #277 T25 (D5.3; O20, O21): three primary routines X, Y, Z; ONE raw map
    /// holding entries for X and Y (by assignment), none for Z; Z owns the pd
    /// arena's one state. (a) A borrow-mode tree scoped to {X} sees nothing of
    /// Y and equals a drain tree over a map holding X alone. (b) Scoped to
    /// {X, Z}, its subject set is still {X} (Z has no entry), and its output,
    /// `pd_solution` included, equals (a)'s. DISCRIMINATION: (a) the
    /// operation index read from the map directly → `op:y` appears; (b) the
    /// subject set from the scope alone → Z's state enters `pd_solution`.
    #[test]
    fn subject_raw_hides_other_subjects() {
        use crate::engine::l4::pd_solve::{ConeArena, ConeState, Producer};
        use crate::engine::l4::summary::TempState;
        use crate::engine::l5::detector_context::CapabilityAnalysis;
        use crate::engine::l5::test_support::{
            call_site, coverage, empty_resolved, fact, op_commit_routine,
        };
        use std::borrow::Cow;
        use std::collections::BTreeSet;
        use std::sync::Arc;
        let mut m = empty_resolved();
        for id in ["X", "Y", "Z"] {
            let mut r = op_commit_routine(id, "procedure", &[&format!("op:{}", id.to_lowercase())]);
            r.call_sites.push(call_site(
                &format!("cs:{}", id.to_lowercase()),
                "Foo",
                vec![],
            ));
            m.workspace.routines.push(r);
        }
        let inherited = |id: &str| {
            let mut f = fact("insert", "table", Some("T"));
            f.provenance = "inherited";
            f.via = "call";
            f.witness_operation_id = Some(format!("op:{id}"));
            f.witness_callsite_id = Some(format!("cs:{id}"));
            vec![f]
        };
        let map: HashMap<String, Vec<CapabilityFact>> =
            [("X", inherited("x")), ("Y", inherited("y"))]
                .into_iter()
                .map(|(k, v)| (k.to_string(), v))
                .collect();
        assert!(
            map.contains_key("X") && map.contains_key("Y") && !map.contains_key("Z"),
            "precondition: X and Y have entries, Z none"
        );
        let mut z_direct = fact("insert", "table", Some("T"));
        z_direct.witness_operation_id = Some("op:z".to_string());
        let caps = CapabilityAnalysis {
            direct_full: [("Z".to_string(), vec![z_direct.clone()])].into(),
            coverage: ["X", "Y", "Z"]
                .into_iter()
                .map(|id| (id.to_string(), coverage("complete")))
                .collect(),
            derived: Default::default(),
            pd: None,
        };
        let arena = ConeArena {
            producers: vec![Producer {
                fact: Arc::new(z_direct),
                owner: Arc::from("Z"),
                operation_id: "op:z".to_string(),
            }],
            states: vec![ConeState {
                producer: 0,
                routine: Arc::from("Z"),
                t: TempState::Known(false),
                reqs: Vec::new(),
                seed: true,
                dist: 0,
                links: 0..0,
            }],
            links: vec![],
            edges: vec![],
            by_routine: [(Arc::from("Z"), vec![0])].into(),
        };
        let ids = |xs: &[&str]| -> BTreeSet<String> { xs.iter().map(|x| x.to_string()).collect() };
        let tree = |raw: TreeRaw<'_>| {
            compose_tree(
                &m,
                &[],
                &[],
                &caps,
                raw,
                Cow::Borrowed(&arena),
                &TreeApps::default(),
            )
        };
        let has_y = |s: &CapabilitySnapshot| {
            s.capability_facts.iter().any(|f| f.subject == "stable::Y")
                || s.operation_index.iter().any(|o| o.operation_id == "op:y")
                || s.callsite_index.iter().any(|c| c.callsite_id == "cs:y")
        };
        assert!(
            has_y(&tree(TreeRaw::Drain(map.clone()))),
            "precondition: Y's entry is observable in a tree that reads it"
        );

        // (a)
        let acc = SubjectRaw::new(&map, &ids(&["X"]));
        assert!(acc.get("Y").is_none(), "Y is outside the subjects");
        assert!(acc.get("X").is_some());
        assert_eq!(acc.subjects(), &ids(&["X"]));
        let a = tree(TreeRaw::Borrow(acc));
        assert!(!has_y(&a), "nothing of Y's entry reaches the tree");
        let only_x: HashMap<String, Vec<CapabilityFact>> =
            [("X".to_string(), map["X"].clone())].into();
        let drain_x = tree(TreeRaw::Drain(only_x));
        assert_eq!(fields(&a), fields(&drain_x), "(a) equals a drain over X");

        // (b)
        let acc = SubjectRaw::new(&map, &ids(&["X", "Z"]));
        assert_eq!(acc.subjects(), &ids(&["X"]), "Z has no entry");
        let b = tree(TreeRaw::Borrow(acc));
        assert_eq!(fields(&b), fields(&a), "(b) equals (a), pd included");
        assert!(
            b.pd_solution.states.is_empty(),
            "Z's state is outside the arena's scope"
        );
    }
}

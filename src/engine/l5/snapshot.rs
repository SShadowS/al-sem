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
//! - M8 (localeCompare audit): al-sem sorts the derivers with `String.localeCompare`,
//!   which is NOT ordinal `str::cmp` for mixed-case ASCII. EVERY sort site here is
//!   ORDINAL-SAFE: the discriminating characters are single-case (lowercase guids +
//!   lowercase hex hashes, fixed lowercase op/kind labels, capitalized-but-uniform
//!   object-type segments `Codeunit`/`Table`/...). No sort key can differ at a
//!   mixed-case position, so ordinal `cmp` == localeCompare for the corpus. This
//!   matches the established R3a* sort sites (which already byte-match
//!   localeCompare-sorted goldens). The 5-fixture differential is the arbiter.
//! - M9: the edge-id present-only predicate is exact (`Option::is_some`; an empty
//!   string IS kept).

use std::collections::HashMap;

use serde::Serialize;

use crate::engine::ids::to_stable_object_id;
use crate::engine::l2::features::{PAnchor, PCallSite, PCallee, POperationSite};
use crate::engine::l2::operation_order::{OperationOrder, ScopeFrame, apply_operation_order};
use crate::engine::l3::event_graph::EventSymbol;
use crate::engine::l3::l3_workspace::{L3Resolved, L3Routine};
use crate::engine::l3::taxonomy::DispatchKind;
use crate::engine::l4::capability_cone::{
    CapabilityExtra, CapabilityFact, R3a3SourceBase, ValueSource, build_r3a3_source_only_base,
};

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

fn snap_temp_state(ts: &crate::engine::l2::features::PTempState) -> SnapTempState {
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
#[derive(Debug, Clone, Serialize)]
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
                crate::engine::l2::features::known_temp_suppresses(
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
            | SnapshotGraphEdge::EventDispatch { edge_id, .. } => edge_id,
        }
    }

    fn kind_str(&self) -> &str {
        match self {
            SnapshotGraphEdge::DirectCall { kind, .. }
            | SnapshotGraphEdge::VariableTypedCall { kind, .. }
            | SnapshotGraphEdge::InterfaceDispatch { kind, .. }
            | SnapshotGraphEdge::ObjectRunResolved { kind, .. }
            | SnapshotGraphEdge::ObjectRunUnresolved { kind, .. }
            | SnapshotGraphEdge::EventDispatch { kind, .. } => kind,
        }
    }

    /// callsiteId of a call-family edge (for the resolution ledger lookup).
    fn callsite_id(&self) -> Option<&str> {
        match self {
            SnapshotGraphEdge::DirectCall { callsite_id, .. }
            | SnapshotGraphEdge::VariableTypedCall { callsite_id, .. }
            | SnapshotGraphEdge::InterfaceDispatch { callsite_id, .. }
            | SnapshotGraphEdge::ObjectRunResolved { callsite_id, .. }
            | SnapshotGraphEdge::ObjectRunUnresolved { callsite_id, .. } => Some(callsite_id),
            SnapshotGraphEdge::EventDispatch { .. } => None,
        }
    }

    /// `to` endpoint (for the resolution-ledger interface candidate list).
    fn to_endpoint(&self) -> Option<&str> {
        match self {
            SnapshotGraphEdge::DirectCall { to, .. }
            | SnapshotGraphEdge::VariableTypedCall { to, .. }
            | SnapshotGraphEdge::InterfaceDispatch { to, .. }
            | SnapshotGraphEdge::ObjectRunResolved { to, .. }
            | SnapshotGraphEdge::EventDispatch { to, .. } => Some(to),
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
            | SnapshotGraphEdge::EventDispatch { from, .. } => from,
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
#[derive(Debug, Clone, Serialize)]
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
}

#[derive(Debug, Clone, Serialize)]
pub struct SnapshotUnresolvedCandidate {
    #[serde(rename = "objectId")]
    pub object_id: String,
    pub reason: String,
}

/// Non-callsite unknown. al-sem `AnalysisGap`: kind, subject, detail.
#[derive(Debug, Clone, Serialize)]
pub struct SnapshotAnalysisGap {
    pub kind: String,
    pub subject: String,
    pub detail: String,
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
fn stable_routine_id(internal: &str, map: &HashMap<String, String>) -> String {
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
/// the target ref. Used for the callsite-resolution ledger calleeDisplay.
fn display_of_callee(callee: &PCallee) -> String {
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

fn routine_attr_names_lc(r: &L3Routine) -> Vec<String> {
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

fn compute_routine_order(r: &L3Routine) -> RoutineOrder {
    use crate::engine::l2::features::PFeatures;
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

// ===========================================================================
// compose_snapshot — the orchestrator. Runs every consumed-core deriver over the
// source-only R3a base, then assembles in fixed key order.
// ===========================================================================

/// Compose the consumed-core `CapabilitySnapshot` for a resolved source-only
/// workspace. `resolved` carries the workspace routines + root classifications.
pub fn compose_snapshot(resolved: &L3Resolved) -> CapabilitySnapshot {
    use crate::engine::perf_trace as pt;
    let base = {
        let _s = pt::span("snapshot", "snapshot.r3a3_base");
        build_r3a3_source_only_base(resolved)
    };
    let _s = pt::span("snapshot", "snapshot.derive");

    let identities = derive_identity_table(resolved);
    let capability_facts = derive_capability_facts(&base);
    let typed_edges = derive_typed_edges(&base);
    let operation_index = derive_operation_evidence(&base);
    let callsite_index = derive_callsite_evidence(&base);
    let callsite_resolutions = derive_callsite_resolutions(&base, &typed_edges);
    let analysis_gaps = derive_analysis_gaps(&base);
    let coverage = derive_coverage(&base);
    let event_declarations = derive_event_declarations(&base);
    let root_classifications = derive_root_classifications(resolved, &base);
    let routine_order_frames = derive_routine_order_frames(&base);

    CapabilitySnapshot {
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
    }
}

// ---------------------------------------------------------------------------
// deriveIdentityTable — objects + tables + routines → parallel sorted arrays.
//
// M8: sorted by stableId. stableIds are `<guid>:<ObjectType>:<num>[#<lowerhex>]`
// or `<guid>:Table:<num>` — discriminating positions are single-case (lowercase
// guid/hex, uniform-case object-type segment), so ordinal cmp == localeCompare.
// ---------------------------------------------------------------------------

fn derive_identity_table(resolved: &L3Resolved) -> SnapshotIdentityTable {
    let ws = &resolved.workspace;
    // id → display name, deduped. BTreeMap keeps deterministic key order (M1) and
    // gives the sorted-by-id pairing al-sem produces (its `Map` + `.sort()`).
    let mut pairs: std::collections::BTreeMap<String, String> = std::collections::BTreeMap::new();

    for obj in &ws.objects {
        pairs.insert(to_stable_object_id(&obj.id), obj.name.clone());
    }
    for tbl in &ws.tables {
        pairs.insert(stable_table_id(&tbl.id), tbl.name.clone());
    }
    for r in &ws.routines {
        pairs.insert(r.stable_routine_id.clone(), r.name.clone());
    }

    // BTreeMap iteration is ordinal-sorted by key — matches al-sem's `.sort()`
    // (ordinal-safe per M8 above).
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
// M8: all key components are single-case (stable ids lowercase-guid/hex, op/kind
// fixed-lowercase labels, witness ids `r0/<lowerhex>/...`) → ordinal-safe.
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

fn derive_capability_facts(base: &R3a3SourceBase) -> Vec<SnapshotCapabilityFact> {
    use rayon::prelude::*;
    // Per-routine emission is independent; indexed par_iter + sequential flatten
    // preserves the exact sequential emission order (ties in the sort key below are
    // resolved by the STABLE sort from this input order — do not reorder).
    let per_routine: Vec<Vec<SnapshotCapabilityFact>> = base
        .ws_routines
        .par_iter()
        .map(|r| {
            // Only routines with a cone entry contribute (mirrors composeSnapshot:
            // facts come from `r.summary`, present iff the cone ran for the routine).
            if !base.cones.contains_key(&r.id) {
                return Vec::new();
            }
            let subject = stable_routine_id(&r.id, &base.routine_to_stable);
            let mut v: Vec<SnapshotCapabilityFact> = Vec::new();
            if let Some(direct) = base.direct_full.get(&r.id) {
                for f in direct {
                    v.push(snapshot_fact(f, &subject));
                }
            }
            if let Some(cone) = base.cones.get(&r.id) {
                for f in &cone.inherited {
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
// M8: the sort key is kind (fixed lowercase) | stable-from | stable-to | edgeId
// (lowercase hex) → all single-case → ordinal-safe.
// ---------------------------------------------------------------------------

fn derive_typed_edges(base: &R3a3SourceBase) -> Vec<SnapshotGraphEdge> {
    // callsite id -> (anchor, owning-routine internal id) for call-family anchors.
    let mut callsite_anchor: HashMap<&str, (&PAnchor, &str)> = HashMap::new();
    for r in &base.ws_routines {
        for cs in &r.call_sites {
            callsite_anchor.insert(cs.id.as_str(), (&cs.source_anchor, r.id.as_str()));
        }
    }
    // routine id -> &PAnchor for event-dispatch publish/subscriber anchors.
    let mut routine_anchor: HashMap<&str, &PAnchor> = HashMap::new();
    for r in &base.ws_routines {
        routine_anchor.insert(r.id.as_str(), &r.source_anchor);
    }

    let map = &base.routine_to_stable;

    let mut out: Vec<SnapshotGraphEdge> = Vec::new();

    for e in &base.graph.typed_edges {
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
            // implicit-trigger triggerKind / dependency-export targetAppGuid are not
            // modelled on the Rust TypedEdge (no such typed edge in the source-only
            // corpus; implicit-trigger is filtered from typedEdges anyway).
            trigger_kind: None,
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
        let edge = match e.kind.as_str() {
            "direct-call" => SnapshotGraphEdge::DirectCall {
                kind: "direct-call",
                callsite_id: e.callsite_id.clone().unwrap_or_default(),
                from: from_stable.clone(),
                to: to_stable.clone().unwrap_or_default(),
                source_anchor: match cs_anchor {
                    Some(a) => a,
                    None => continue,
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
                    None => continue,
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
                    None => continue,
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
                    None => continue,
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
                    None => continue,
                },
                edge_id: edge_id.clone(),
            },
            "event-dispatch" => {
                let Some(publish_anchor) = routine_anchor
                    .get(e.from.as_str())
                    .map(|a| anchor_from_panchor(a, &e.from))
                else {
                    continue;
                };
                let to_internal = e.to.clone().unwrap_or_default();
                let Some(subscriber_anchor) = routine_anchor
                    .get(to_internal.as_str())
                    .map(|a| anchor_from_panchor(a, &to_internal))
                else {
                    continue;
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
            // Unmodelled kind (implicit-trigger / dependency-export) - never reached
            // for the source-only corpus; skip rather than emit a malformed edge.
            _ => continue,
        };
        out.push(edge);
    }

    out.sort_by_key(|e| e.sort_key());
    out
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
// Sort by operationId (al-sem). M8: operationId = `r0/<lowerhex>/opN` → ordinal-safe.
// ---------------------------------------------------------------------------

fn derive_operation_evidence(base: &R3a3SourceBase) -> Vec<SnapshotOperationEvidence> {
    use std::collections::BTreeSet;

    // Referenced op ids = witnessOperationId of any direct ∪ inherited fact.
    let mut referenced: BTreeSet<String> = BTreeSet::new();
    for r in &base.ws_routines {
        if !base.cones.contains_key(&r.id) {
            continue;
        }
        if let Some(direct) = base.direct_full.get(&r.id) {
            for f in direct {
                if let Some(w) = &f.witness_operation_id {
                    referenced.insert(w.clone());
                }
            }
        }
        if let Some(cone) = base.cones.get(&r.id) {
            for f in &cone.inherited {
                if let Some(w) = &f.witness_operation_id {
                    referenced.insert(w.clone());
                }
            }
        }
    }

    // byId keyed by operationId — operationSites first, recordOperations overwrite.
    let mut by_id: std::collections::BTreeMap<String, SnapshotOperationEvidence> =
        std::collections::BTreeMap::new();

    for r in &base.ws_routines {
        let stable = stable_routine_id(&r.id, &base.routine_to_stable);
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
                order: op.order,
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
                order: matching.and_then(|_| op_order_by_id.get(ro.id.as_str()).map(|o| **o)),
                under_asserterror: None,
            };
            by_id.insert(ro.id.clone(), entry);
        }
    }

    // BTreeMap iteration = sorted-by-operationId (ordinal-safe, M8).
    by_id.into_values().collect()
}

// ---------------------------------------------------------------------------
// deriveCallsiteEvidence — REFERENCED-ONLY: emit callsites that appear as a
// CapabilityFact witnessCallsiteId (direct ∪ inherited) OR as a typed-edge
// callsiteId anchor. Internal ids. Sort by callsiteId.
//
// M8: callsiteId = `r0/<lowerhex>/csN` → ordinal-safe.
// ---------------------------------------------------------------------------

fn derive_callsite_evidence(base: &R3a3SourceBase) -> Vec<SnapshotCallsiteEvidence> {
    use std::collections::BTreeSet;

    let mut referenced: BTreeSet<String> = BTreeSet::new();
    for r in &base.ws_routines {
        if !base.cones.contains_key(&r.id) {
            continue;
        }
        if let Some(direct) = base.direct_full.get(&r.id) {
            for f in direct {
                if let Some(w) = &f.witness_callsite_id {
                    referenced.insert(w.clone());
                }
            }
        }
        if let Some(cone) = base.cones.get(&r.id) {
            for f in &cone.inherited {
                if let Some(w) = &f.witness_callsite_id {
                    referenced.insert(w.clone());
                }
            }
        }
    }
    // Typed-edge callsite anchors.
    for e in &base.graph.typed_edges {
        if let Some(cid) = &e.callsite_id {
            referenced.insert(cid.clone());
        }
    }

    let mut out: Vec<SnapshotCallsiteEvidence> = Vec::new();
    for r in &base.ws_routines {
        let stable = stable_routine_id(&r.id, &base.routine_to_stable);
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
                order: cs.order,
                under_asserterror: if cs.under_asserterror == Some(true) {
                    Some(true)
                } else {
                    None
                },
            });
        }
    }
    out.sort_by(|a, b| a.callsite_id.cmp(&b.callsite_id));
    out
}

// ---------------------------------------------------------------------------
// deriveCallsiteResolutions — one row per syntactic callsite in model.callGraph.
// Groups by callsiteId; skips implicit-trigger edges. Status mapping per al-sem
// `callsite-resolutions.ts`. resolvedEdges = matching typed-edge edgeIds.
//
// Sort by (from|callsiteId). M8: both stable/internal single-case → ordinal-safe.
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
    base: &R3a3SourceBase,
    typed_edges: &[SnapshotGraphEdge],
) -> Vec<SnapshotCallsiteResolution> {
    use std::collections::BTreeMap;

    let map = &base.routine_to_stable;

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
    let mut callee_display: HashMap<String, String> = HashMap::new();
    let mut result_consumed: HashMap<String, bool> = HashMap::new();
    let mut under_asserterror: HashMap<String, bool> = HashMap::new();
    for r in &base.ws_routines {
        for cs in &r.call_sites {
            callee_display.insert(cs.id.clone(), display_of_callee(&cs.callee));
            if let Some(rc) = cs.result_consumed {
                result_consumed.insert(cs.id.clone(), rc);
            }
            if cs.under_asserterror == Some(true) {
                under_asserterror.insert(cs.id.clone(), true);
            }
        }
    }

    // Group callGraph edges by callsiteId (skip implicit-trigger). Preserve first.
    // BTreeMap for deterministic group iteration (M1).
    let mut groups: BTreeMap<String, Vec<&crate::engine::l3::call_resolver::CallEdge>> =
        BTreeMap::new();
    let mut group_order: Vec<String> = Vec::new();
    for ce in &base.calls.edges {
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
        let group_edges = &groups[key];
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
                open_world: Some(true),
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
            });
            continue;
        }

        out.push(classify_resolution(
            representative,
            &from,
            &cdisplay,
            &dispatch_kind,
            map,
            &edges_by_callsite,
            rc,
            ua,
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
        });
    }

    out.sort_by(|a, b| {
        format!("{}|{}", a.from, a.callsite_id).cmp(&format!("{}|{}", b.from, b.callsite_id))
    });
    out
}

#[allow(clippy::too_many_arguments)]
fn classify_resolution(
    ce: &crate::engine::l3::call_resolver::CallEdge,
    from: &str,
    callee_display: &str,
    dispatch_kind: &str,
    map: &HashMap<String, String>,
    edges_by_callsite: &HashMap<String, Vec<String>>,
    result_consumed: Option<bool>,
    under_asserterror: bool,
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
        }
    };

    match ce.resolution.as_str() {
        "resolved" => {
            let mut edges = edges_by_callsite
                .get(callsite_key)
                .cloned()
                .unwrap_or_default();
            edges.sort();
            base_row("resolved", edges, None)
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
// adjacent. The source-only corpus has zero gaps (no parse-incomplete / no opaque
// deps), so this is empty for the 5 fixtures — implemented faithfully regardless.
//
// M8: kind (fixed lowercase) | subject (stable id / guid, lowercase) → ordinal-safe.
// ---------------------------------------------------------------------------

fn derive_analysis_gaps(base: &R3a3SourceBase) -> Vec<SnapshotAnalysisGap> {
    use std::collections::BTreeSet;

    let map = &base.routine_to_stable;
    let mut out: Vec<SnapshotAnalysisGap> = Vec::new();

    // Source-only: no coverage.routinesParseIncomplete plumbed through the base, so
    // derive parse-incomplete from routine.parse_incomplete (the model's own flag).
    for r in &base.ws_routines {
        if r.parse_incomplete {
            out.push(SnapshotAnalysisGap {
                kind: "parse-incomplete".to_string(),
                subject: stable_routine_id(&r.id, map),
                detail: "routine body had a parse error; structural extraction was partial"
                    .to_string(),
            });
        }
    }

    // Symbol-only apps: any app guid in a body-unavailable dependency routine.
    let mut opaque_apps: BTreeSet<String> = BTreeSet::new();
    for r in &base.ws_routines {
        if !r.body_available
            && is_dependency_role(r)
            && let Some(guid) = r.object_id.split('/').next()
            && !guid.is_empty()
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
        });
    }

    // Body-unavailable dependency routines not covered by an app-level gap.
    for r in &base.ws_routines {
        if r.body_available || r.parse_incomplete {
            continue;
        }
        if !is_dependency_role(r) {
            continue;
        }
        let guid = r.object_id.split('/').next().unwrap_or("");
        if opaque_apps.contains(guid) {
            continue;
        }
        out.push(SnapshotAnalysisGap {
            kind: "body-unavailable".to_string(),
            subject: stable_routine_id(&r.id, map),
            detail: "routine has no parseable body".to_string(),
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

/// Source-only workspaces have no dependency routines; this is always false here
/// but kept for faithfulness with al-sem's `roleOf(r) === "dependency"`.
fn is_dependency_role(_r: &L3Routine) -> bool {
    false
}

// ---------------------------------------------------------------------------
// deriveCoverage — per-routine cone coverage, subject already stable on the cone.
// Sort by subject. M8: stable routine id → ordinal-safe.
// ---------------------------------------------------------------------------

fn derive_coverage(base: &R3a3SourceBase) -> Vec<SnapshotCoverageRecord> {
    let map = &base.routine_to_stable;
    let mut out: Vec<SnapshotCoverageRecord> = Vec::new();
    for r in &base.ws_routines {
        let Some(cone) = base.cones.get(&r.id) else {
            continue;
        };
        let cov = &cone.coverage;
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
// graph. Sort by (kind|routine|eventId). M8: kind fixed lowercase, routine/eventId
// stable single-case → ordinal-safe.
// ---------------------------------------------------------------------------

fn derive_event_declarations(base: &R3a3SourceBase) -> Vec<SnapshotEventDeclaration> {
    let map = &base.routine_to_stable;

    // routine id → internal id (for enclosingRoutineId) + source anchor.
    let mut routine_anchor: HashMap<&str, &PAnchor> = HashMap::new();
    for r in &base.ws_routines {
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
// Sort by routineId. M8: stable routine id → ordinal-safe.
// ---------------------------------------------------------------------------

fn derive_root_classifications(
    resolved: &L3Resolved,
    base: &R3a3SourceBase,
) -> Vec<SnapshotRootClassificationSlot> {
    let map = &base.routine_to_stable;

    // internal routine id → (&PAnchor, internal id) for the sourceAnchor.
    let mut routine_by_id: HashMap<&str, &L3Routine> = HashMap::new();
    for r in &base.ws_routines {
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
// HashMap iteration). M8: stable routine id keys → ordinal-safe.
// ---------------------------------------------------------------------------

fn derive_routine_order_frames(base: &R3a3SourceBase) -> Option<RoutineOrderFrames> {
    use std::collections::BTreeMap;
    // BTreeMap keyed by stable routine id → sorted key order (M8: stable id ⇒
    // ordinal-safe; M1: BTreeMap iteration is sorted, not hash-ordered).
    let mut entries: BTreeMap<String, Vec<ScopeFrame>> = BTreeMap::new();
    for r in &base.ws_routines {
        let order = compute_routine_order(r);
        if order.scope_frames.is_empty() {
            continue;
        }
        let stable = stable_routine_id(&r.id, &base.routine_to_stable);
        entries.insert(stable, order.scope_frames);
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
pub fn project_r4f_snapshot(resolved: &L3Resolved, fixture_name: &str) -> String {
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
    use crate::engine::l2::features::known_temp_suppresses;

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

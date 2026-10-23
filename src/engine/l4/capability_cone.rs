//! L4 CAPABILITY CONE + COVERAGE (R3a-3) — faithful port of al-sem's
//! `src/engine/capability-cone.ts` (`composeInheritedCones` + the supporting
//! cone-build recurrences) and the cone/coverage wire-in in
//! `src/engine/summary-runner.ts` (~505-680) + the R3a-3 stable projection
//! (`scripts/r3a3-projection.ts` `projectR3a3`).
//!
//! Three layers:
//!   1. DIRECT facts — per-routine `capabilityFactsDirect`, built from the
//!      RESOLVED L3 routine (table family from `record_operations`, dispatch
//!      family from object-run call sites) + the L4 publisher-fact injection
//!      from the resolved event graph. This mirrors al-sem's L4 `extractCapabilities`
//!      (which reads `routine.features.recordOperations` with the L3-RESOLVED
//!      `tableId`) + the publisher-fact injection in summary-runner.ts.
//!   2. The CONE — `compose_inherited_cones`: a single fused bottom-up walk over
//!      the SCC condensation of `typedEdges`. Per SCC it builds a fact cone
//!      (members' direct facts at dist 0 + each successor cone shifted +1) and a
//!      coverage cone, emits per-routine inherited facts (SHORTEST-distance
//!      witness, `inheritedFactKey` dedup, equal-distance tie-breaker, canonical
//!      sort) + the coverage `CoverageRecord` roll-up, then refcount-frees
//!      downstream cones.
//!   3. The PROJECTION — `project_r3a3`: the stable-id comparison surface the
//!      R3a-3 vectors / goldens carry.
//!
//! All ids are INTERNAL until the projection. No `HashMap` iteration reaches the
//! output: the cone fact maps are `BTreeMap` (key-ordered) and every output Vec
//! is explicitly sorted. The walk terminates on cyclic/deep graphs because it
//! runs over the ACYCLIC SCC condensation; within a recursive SCC the BFS visits
//! each member at most once.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::Arc;

use super::combined_graph::{CombinedGraph, TypedEdge, UncertaintyEdge, build_combined_graph};
use super::cone_census;
use super::cone_derived::{
    ConeDerivedBuilder, ConeDerivedStore, ConeOutput, FoldedRow, fact_is_known_temp,
};
use super::scc::{Scc, SccInputGraph, SccResult, tarjan_scc};
use crate::engine::ids::to_stable_object_id;
use crate::program::body::capability::table_commit;
use crate::program::body::features::{PCallSite, PCallee, PExpressionInfo, POperationSite};
use crate::program::model::events::{EventGraph, EventSymbol};
use crate::program::model::role::Role;
use crate::program::model::workspace::{Model, ModelEntities, ModelRoutine};

// ===========================================================================
// Internal CapabilityFact (FULL form — internal ids). Mirrors al-sem
// `model/capability.ts` `CapabilityFact`, dropping `subject` (the projection
// key) and carrying the L3-resolved `resource_id`.
// ===========================================================================

/// A `ValueSource` (internal form — table-field tableId is INTERNAL until
/// projected). Mirrors al-sem `ValueSource`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum ValueSource {
    Literal {
        value: String,
    },
    Enum {
        enum_name: String,
        member: Option<String>,
    },
    ConstantVar {
        var_name: String,
        initializer: Box<ValueSource>,
    },
    Parameter {
        index: u32,
        var_name: String,
    },
    TableField {
        table_id: String,
        field_name: String,
    },
    Expression,
    Unknown,
}

/// Per-resourceKind extra semantics (internal form). Only the variants the
/// vector-exercised families produce are modelled with structure; the rest pass
/// through as opaque JSON for projection (so the full corpus in Task 3 can be
/// extended without reshaping the cone).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum CapabilityExtra {
    Table {
        record_variable_id: Option<String>,
        temp_state: Option<crate::program::body::features::PTempState>,
        op_subtype: Option<String>,
    },
    Dispatch {
        object_type: String,
        modal: Option<bool>,
    },
    Event {
        event_class: String,
        include_sender: Option<bool>,
    },
    /// HttpExtra (al-sem `model/capability.ts`). `method` + optional `bodyArgSource`.
    Http {
        method: String,
        body_arg_source: Option<ValueSource>,
    },
    /// StorageExtra (al-sem `model/capability.ts`). Optional key/value sources +
    /// scope (`User` | `Company` | `Module` | `unknown`).
    Storage {
        key_arg_source: Option<ValueSource>,
        value_arg_source: Option<ValueSource>,
        scope: Option<String>,
    },
}

/// One normalized direct/inherited capability fact (internal form). `subject` is
/// kept for `repKey` tie-break parity but excluded from the projection.
/// `Ord` (#160 step 11b §4.6): a collided id's direct facts are a full-value
/// union, ordered by this total order.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct CapabilityFact {
    pub subject: String,
    /// ⟨C1 Task 4⟩ `op` / `resource_kind` / `confidence` / `provenance` / `via`
    /// are CLOSED vocabularies — every producer in the engine hands one of a
    /// fixed set of literals (`table_commit::map_op`, `object_type_to_resource_kind`,
    /// `confidence_from_source`, `capability_via_for_edge_kind`, and the direct
    /// extractors' inline literals), so an owned `String` per field bought
    /// nothing but 5 heap allocations and 5 memcpys per fact — on every one of
    /// the millions of `CapabilityFact` clones the cone merge performs. As
    /// `&'static str` they are pointer copies into the binary's read-only data
    /// and the struct itself is 40 B smaller. The six genuinely-open fields
    /// (`subject`, `resource_id`, `resource_arg_source`, `witness_operation_id`,
    /// `witness_callsite_id`, `extra`) stay owned.
    pub op: &'static str,
    pub resource_kind: &'static str,
    /// Internal resourceId (TableId / EventId / ObjectId) — projected to stable.
    pub resource_id: Option<String>,
    pub resource_arg_source: Option<ValueSource>,
    pub confidence: &'static str,
    pub provenance: &'static str,
    pub via: &'static str,
    pub witness_operation_id: Option<String>,
    pub witness_callsite_id: Option<String>,
    /// An inherited fact whose first hop is an `implicit-trigger` edge: that
    /// edge's record operation id (#96 D5). `None` on direct facts and on every
    /// other first hop. `witness_callsite_id` keeps its meaning (a trigger edge
    /// has no call site).
    pub witness_first_hop_operation_id: Option<String>,
    /// An inherited fact whose first hop is an `event-dispatch` edge: that
    /// edge's INTERNAL event id (#98 D1). `None` on direct facts and on every
    /// other first hop (an event edge has no call site and no op).
    pub witness_first_hop_event_id: Option<String>,
    pub extra: Option<CapabilityExtra>,
}

/// One per-routine coverage record (internal form). Mirrors al-sem
/// `CoverageRecord`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoverageRecord {
    pub subject: String,
    pub direct_status: String,
    pub inherited_status: String,
    pub reasons: Vec<String>,
    pub unknown_targets: Vec<String>,
}

// ===========================================================================
// DIRECT facts — per-routine `capabilityFactsDirect`, built over the RESOLVED
// L3 routine + the publisher-fact injection. Mirrors al-sem's L4
// `extractCapabilities` read of the RESOLVED features + summary-runner.ts
// publisher injection.
// ===========================================================================

/// Map an object-run kind to the dispatch resourceKind (al-sem `dispatch.ts`).
fn object_type_to_resource_kind(object_type: &str) -> &'static str {
    match object_type {
        "Codeunit" => "codeunit",
        "Page" => "page",
        "Report" => "report",
        _ => "codeunit",
    }
}

/// Map an EventSymbol.eventKind to its CapabilityExtra event class. Faithful port
/// of al-sem `summary-runner.ts` `mapEventKindToClass`: business→Business,
/// internal→Internal, trigger→Trigger, EVERYTHING else (incl. "integration" /
/// "unknown") → Integration. The Rust event graph emits lowercase kinds
/// ("integration" / "business" / "unknown").
fn map_event_kind_to_class(event_kind: &str) -> &'static str {
    match event_kind {
        "business" => "Business",
        "internal" => "Internal",
        "trigger" => "Trigger",
        _ => "Integration",
    }
}

/// Map an HttpClient method name to its `HttpExtra.method` literal, or `None`
/// when the method isn't an HTTP verb (al-sem `http.ts` HTTP_METHOD_SET).
fn http_method(method: &str) -> Option<&'static str> {
    match method {
        "Send" => Some("Send"),
        "Get" => Some("Get"),
        "Post" => Some("Post"),
        "Put" => Some("Put"),
        "Delete" => Some("Delete"),
        "Patch" => Some("Patch"),
        _ => None,
    }
}

/// Map a lowercased IsolatedStorage method to its capability op (al-sem
/// `isolated-storage.ts` ISOLATED_STORAGE_OPS), or `None`.
fn isolated_storage_op(method_lc: &str) -> Option<&'static str> {
    match method_lc {
        "get" | "getencrypted" | "contains" => Some("store-read"),
        "set" | "setencrypted" => Some("store-write"),
        "delete" => Some("store-delete"),
        _ => None,
    }
}

/// Parse a DataScope enum text → `StorageExtra.scope` (al-sem `parseDataScope`).
fn parse_data_scope(text: &str) -> String {
    let lower = text.to_lowercase();
    if lower.contains("::company") {
        "Company".to_string()
    } else if lower.contains("::user") {
        "User".to_string()
    } else if lower.contains("::module") {
        "Module".to_string()
    } else {
        "unknown".to_string()
    }
}

use crate::program::body::capability::io::is_temp_blob_type;

/// True when a declared type names a Page or Report (al-sem
/// `ui-window-open.ts` `isPageOrReportType`).
fn is_page_or_report_type(declared_type: &str) -> bool {
    let lower = declared_type.to_lowercase();
    lower == "page"
        || lower == "report"
        || lower.starts_with("page ")
        || lower.starts_with("report ")
}

/// Confidence derivation from a `ValueSource` (al-sem `confidenceFromSource`).
fn confidence_from_source(vs: &ValueSource) -> &'static str {
    match vs {
        ValueSource::Literal { .. } | ValueSource::Enum { .. } => "static",
        ValueSource::ConstantVar { initializer, .. } => confidence_from_source(initializer),
        ValueSource::Parameter { .. } => "userDynamic",
        ValueSource::TableField { .. } => "configDynamic",
        ValueSource::Expression | ValueSource::Unknown => "unresolved",
    }
}

/// A minimal variable index for value-source classification: lowercased name →
/// (is_parameter, parameter_index, declared_type, table_id?). Built from the L3
/// routine's record vars + lexical variables.
struct VarInfo {
    is_parameter: bool,
    parameter_index: u32,
    /// Declared type — `receiverTypeOf` (http/file-blob/ui-window-open receiver
    /// classification) + the member-expression (table-field) record check read it.
    declared_type: String,
    /// Resolved internal TableId when this var is a record variable — feeds the
    /// member-expression (table-field) value-source branch (`classifyMemberExpression`).
    table_id: Option<String>,
    /// The L2-captured one-hop initializer as a `ValueSource` (al-sem
    /// `VariableSymbol.initializer`). Feeds `classifyIdentifier`'s constant-var
    /// resolution + the one-hop var-to-var alias chase.
    initializer: Option<ValueSource>,
}

/// Classify a call-argument `PExpressionInfo` into a `ValueSource`. A focused
/// port of al-sem `value-source.ts` covering the literal / enum / database-ref /
/// parameter / member-expression (table-field) forms the dispatch + background +
/// IO families need. Constant-var initializer chasing requires the L2 captured
/// initializer (dropped at L3); unresolved local identifiers degrade to
/// `Expression` (matching al-sem when no initializer is captured).
fn classify_value_source(
    info: Option<&PExpressionInfo>,
    variables: &HashMap<String, VarInfo>,
) -> ValueSource {
    let Some(info) = info else {
        return ValueSource::Unknown;
    };
    match info.kind.as_str() {
        "string_literal" => ValueSource::Literal {
            value: info
                .value
                .clone()
                .unwrap_or_else(|| strip_single_quotes(&info.text)),
        },
        "integer" | "decimal" | "boolean" => ValueSource::Literal {
            value: info
                .value
                .clone()
                .unwrap_or_else(|| info.text.trim().to_string()),
        },
        "qualified_enum_value" | "database_reference" => {
            let enum_name = info
                .qualifier
                .as_deref()
                .map(strip_double_quotes)
                .map(|s| s.to_string());
            // al-sem value-source.ts: `info.member ?? info.value ?? ""` — the `?? ""`
            // floor means `member` is ALWAYS a string (worst case ""), so it is always
            // serialized. Without the floor, a qualified_enum_value/database_reference
            // whose ExpressionInfo carries neither member nor value would omit the key
            // where al-sem emits "". (Corpus-invisible: corpus enum refs are well-formed.)
            let member = info
                .member
                .clone()
                .or_else(|| info.value.clone())
                .or(Some(String::new()));
            ValueSource::Enum {
                enum_name: enum_name.unwrap_or_default(),
                member,
            }
        }
        "identifier" | "quoted_identifier" => {
            let name = info
                .value
                .clone()
                .unwrap_or_else(|| info.text.clone())
                .to_lowercase();
            classify_identifier(&name, variables, 0)
        }
        "member_expression" => classify_member_expression(&info.text, variables),
        "unary_expression" => match &info.value {
            Some(v) => ValueSource::Literal { value: v.clone() },
            None => ValueSource::Expression,
        },
        _ => ValueSource::Expression,
    }
}

const MAX_CHASE_DEPTH: u32 = 3;

/// Resolve an identifier name to a `ValueSource`. Faithful port of al-sem
/// `value-source.ts` `classifyIdentifier`: a parameter → `parameter`; a local
/// with a resolved initializer → that initializer (one-hop var-to-var alias
/// chase, capped at `MAX_CHASE_DEPTH`); a local with no / opaque initializer →
/// `constant-var`. Unknown name → `expression`.
fn classify_identifier(
    name_lower: &str,
    variables: &HashMap<String, VarInfo>,
    depth: u32,
) -> ValueSource {
    let Some(sym) = variables.get(name_lower) else {
        return ValueSource::Expression;
    };
    if sym.is_parameter {
        return ValueSource::Parameter {
            index: sym.parameter_index,
            var_name: name_lower.to_string(),
        };
    }
    let init = sym.initializer.clone();
    match &init {
        None | Some(ValueSource::Unknown) | Some(ValueSource::Expression) => {
            // No initializer captured or it's already opaque — emit constant-var.
            ValueSource::ConstantVar {
                var_name: name_lower.to_string(),
                initializer: Box::new(init.unwrap_or(ValueSource::Unknown)),
            }
        }
        Some(init_vs) => {
            if depth >= MAX_CHASE_DEPTH {
                // Depth cap — keep the raw initializer, don't recurse.
                return ValueSource::ConstantVar {
                    var_name: name_lower.to_string(),
                    initializer: Box::new(init_vs.clone()),
                };
            }
            if let ValueSource::ConstantVar {
                var_name: inner, ..
            } = init_vs
            {
                let deeper = classify_identifier(inner, variables, depth + 1);
                if matches!(
                    deeper,
                    ValueSource::Literal { .. }
                        | ValueSource::Enum { .. }
                        | ValueSource::Parameter { .. }
                ) {
                    return deeper;
                }
                return ValueSource::ConstantVar {
                    var_name: name_lower.to_string(),
                    initializer: Box::new(deeper),
                };
            }
            // Initializer already a resolved kind (literal / enum / parameter / table-field).
            init_vs.clone()
        }
    }
}

/// Classify a `member_expression` text (`Receiver.Field`) as a `table-field`
/// ValueSource when the receiver resolves to a record-typed variable; else
/// `expression`. Faithful port of al-sem `value-source.ts`
/// `classifyMemberExpression`. The first `.` separates receiver from field.
fn classify_member_expression(text: &str, variables: &HashMap<String, VarInfo>) -> ValueSource {
    let Some(dot_idx) = text.find('.') else {
        return ValueSource::Expression;
    };
    let receiver_raw = text[..dot_idx].trim();
    let field_raw = text[dot_idx + 1..].trim();
    let receiver_lower = receiver_raw.to_lowercase();
    let Some(sym) = variables.get(&receiver_lower) else {
        return ValueSource::Expression;
    };
    let decl = sym.declared_type.to_lowercase();
    let is_record =
        decl.starts_with("record ") || decl == "record" || decl.starts_with("recordref");
    if !is_record {
        return ValueSource::Expression;
    }
    let field_name = strip_double_quotes(field_raw).to_string();
    let table_id = sym
        .table_id
        .clone()
        .unwrap_or_else(|| "unknown".to_string());
    ValueSource::TableField {
        table_id,
        field_name,
    }
}

/// Parse an L2-captured `VariableSymbol.initializer` JSON value into a
/// `ValueSource`. Mirrors al-sem's `ValueSource` JSON shape. Unknown / malformed
/// shapes degrade to `ValueSource::Unknown` (engine-never-throws).
fn value_source_from_json(v: &serde_json::Value) -> ValueSource {
    let Some(kind) = v.get("kind").and_then(|k| k.as_str()) else {
        return ValueSource::Unknown;
    };
    match kind {
        "literal" => ValueSource::Literal {
            value: v
                .get("value")
                .and_then(|x| x.as_str())
                .unwrap_or_default()
                .to_string(),
        },
        "enum" => ValueSource::Enum {
            enum_name: v
                .get("enumName")
                .and_then(|x| x.as_str())
                .unwrap_or_default()
                .to_string(),
            member: v
                .get("member")
                .and_then(|x| x.as_str())
                .map(|s| s.to_string()),
        },
        "constant-var" => ValueSource::ConstantVar {
            var_name: v
                .get("varName")
                .and_then(|x| x.as_str())
                .unwrap_or_default()
                .to_string(),
            initializer: Box::new(
                v.get("initializer")
                    .map(value_source_from_json)
                    .unwrap_or(ValueSource::Unknown),
            ),
        },
        "parameter" => ValueSource::Parameter {
            index: v.get("index").and_then(|x| x.as_u64()).unwrap_or(0) as u32,
            var_name: v
                .get("varName")
                .and_then(|x| x.as_str())
                .unwrap_or_default()
                .to_string(),
        },
        "table-field" => ValueSource::TableField {
            table_id: v
                .get("tableId")
                .and_then(|x| x.as_str())
                .unwrap_or("unknown")
                .to_string(),
            field_name: v
                .get("fieldName")
                .and_then(|x| x.as_str())
                .unwrap_or_default()
                .to_string(),
        },
        "expression" => ValueSource::Expression,
        _ => ValueSource::Unknown,
    }
}

fn strip_single_quotes(s: &str) -> String {
    let t = s.trim();
    let b = t.as_bytes();
    if b.len() >= 2 && b[0] == b'\'' && b[b.len() - 1] == b'\'' {
        t[1..t.len() - 1].to_string()
    } else {
        t.to_string()
    }
}

fn strip_double_quotes(s: &str) -> &str {
    let t = s.trim();
    let b = t.as_bytes();
    if b.len() >= 2 && b[0] == b'"' && b[b.len() - 1] == b'"' {
        &t[1..t.len() - 1]
    } else {
        t
    }
}

/// Build the per-routine direct capability facts in al-sem extractor order
/// (table → dispatch → … → events) for the families the source-only cone needs,
/// PLUS the L4 publisher-fact injection. Returns `(facts, status, reasons)`.
///
/// Opaque / parse-incomplete routines yield `([], "unknown", reasons)` mirroring
/// the summary-runner.ts:553-559 override.
pub(crate) fn direct_facts_for_routine(
    routine: &ModelRoutine,
    publisher_events: &[&EventSymbol],
) -> (Vec<CapabilityFact>, String, Vec<String>) {
    if !routine.body_available {
        return (
            Vec::new(),
            "unknown".to_string(),
            vec!["opaque-dependency".to_string()],
        );
    }
    if routine.parse_incomplete {
        return (
            Vec::new(),
            "unknown".to_string(),
            vec!["parse-incomplete".to_string()],
        );
    }

    // Variable index for value-source classification + receiverTypeOf. Built from
    // the L3 lexical variables ALONE (which already include the parameters as
    // VariableSymbols with isParameter=true), LAST-wins per lowercased name —
    // mirroring al-sem's orchestrator `for (v of features.variables)
    // variables.set(v.name, v)`. Record-var TableIds are folded in afterwards so
    // the member-expression (table-field) value-source branch can resolve.
    //
    // NOTE: al-sem reads `sym.tableId` off the VariableSymbol, which is NOT
    // populated for source-only routines (the VariableSymbol carries no resolved
    // tableId — only the RecordVariable does), so a `Receiver.Field` member-expr
    // resolves to `table-field` with `tableId: "unknown"`. We therefore DO NOT
    // fold the resolved record-var tableId here: doing so would over-resolve vs
    // al-sem (confirmed by the ws-policy-api-dynamic-dispatch golden, which carries
    // `tableId: "unknown"`).
    let mut variables: HashMap<String, VarInfo> = HashMap::new();
    for v in routine.variables.iter() {
        variables.insert(
            v.name.to_lowercase(),
            VarInfo {
                is_parameter: v.is_parameter,
                parameter_index: v.parameter_index.unwrap_or(0),
                declared_type: v.declared_type.clone(),
                table_id: None,
                initializer: v.initializer.as_ref().map(value_source_from_json),
            },
        );
    }

    // ── Unreachable exclusion (extractor.ts:100-142) ───────────────────────
    // Operation sites / call sites with controlContext === "unreachable" never
    // produce capability facts. recordOperations share IDs with operationSites,
    // so exclude record ops whose id is in the unreachable-op-id set.
    let mut unreachable_op_ids: BTreeSet<String> = BTreeSet::new();
    for op in &routine.operation_sites {
        if op.control_context.as_deref() == Some("unreachable") {
            unreachable_op_ids.insert(op.id.clone());
        }
    }
    let record_ops: Vec<&crate::program::model::workspace::ModelRecordOperation> = routine
        .record_operations
        .iter()
        .filter(|op| !unreachable_op_ids.contains(&op.id))
        .collect();
    let call_sites: Vec<&PCallSite> = routine
        .call_sites
        .iter()
        .filter(|cs| cs.control_context.as_deref() != Some("unreachable"))
        .collect();
    let operation_sites: Vec<&POperationSite> = routine
        .operation_sites
        .iter()
        .filter(|op| op.control_context.as_deref() != Some("unreachable"))
        .collect();

    let mut facts: Vec<CapabilityFact> = Vec::new();
    let reasons: Vec<String> = Vec::new();

    // `receiverTypeOf` — declared type of a named receiver, else "unknown"
    // (extractor.ts:147). Used by http / file-blob / ui-window-open families.
    let receiver_type_of = |name: &str| -> String {
        variables
            .get(&name.to_lowercase())
            .map(|v| v.declared_type.clone())
            .unwrap_or_else(|| "unknown".to_string())
    };

    // ── table family (al-sem table.ts) ─────────────────────────────────────
    for op in &record_ops {
        // The one table op map shared with L2 (#69 D5, #100).
        let Some(cap_op) = table_commit::map_op(&op.op) else {
            continue;
        };
        let resource_id = op.table_id.clone();
        let confidence = if resource_id.is_some() {
            "static"
        } else {
            "unresolved"
        };
        facts.push(CapabilityFact {
            subject: routine.id.clone(),
            op: cap_op,
            resource_kind: "table",
            resource_id,
            resource_arg_source: None,
            confidence,
            provenance: "direct",
            via: "self",
            witness_operation_id: Some(op.id.clone()),
            witness_callsite_id: None,
            witness_first_hop_operation_id: None,
            witness_first_hop_event_id: None,
            extra: Some(CapabilityExtra::Table {
                record_variable_id: op.record_variable_id.clone(),
                temp_state: op.temp_state.clone(),
                op_subtype: Some(op.op.clone()),
            }),
        });
    }

    // ── commit family (al-sem commit.ts) ───────────────────────────────────
    // One fact per operationSite with kind === "commit".
    for op in &operation_sites {
        if op.kind == "commit" {
            facts.push(CapabilityFact {
                subject: routine.id.clone(),
                op: "commit",
                resource_kind: "transaction",
                resource_id: None,
                resource_arg_source: None,
                confidence: "static",
                provenance: "direct",
                via: "self",
                witness_operation_id: Some(op.id.clone()),
                witness_callsite_id: None,
                witness_first_hop_operation_id: None,
                witness_first_hop_event_id: None,
                extra: None,
            });
        }
    }

    // ── dispatch family (al-sem dispatch.ts) ───────────────────────────────
    for cs in &call_sites {
        match &cs.callee {
            PCallee::ObjectRun { object_kind, .. } => {
                let object_type = object_kind.clone();
                let target = classify_value_source(cs.argument_infos.first(), &variables);
                let confidence = confidence_from_source(&target);
                facts.push(CapabilityFact {
                    subject: routine.id.clone(),
                    op: "execute",
                    resource_kind: object_type_to_resource_kind(&object_type),
                    resource_id: None,
                    resource_arg_source: Some(target),
                    confidence,
                    provenance: "direct",
                    via: "self",
                    witness_operation_id: None,
                    witness_callsite_id: Some(cs.id.clone()),
                    witness_first_hop_operation_id: None,
                    witness_first_hop_event_id: None,
                    extra: Some(CapabilityExtra::Dispatch {
                        object_type,
                        modal: None,
                    }),
                });
            }
            PCallee::Member { receiver, method } => {
                let key = format!("{}|{}", receiver.to_lowercase(), method.to_lowercase());
                let (object_type, modal): (&str, Option<bool>) = match key.as_str() {
                    "page|runmodal" => ("Page", Some(true)),
                    "report|execute" => ("Report", None),
                    _ => continue,
                };
                let target = classify_value_source(cs.argument_infos.first(), &variables);
                let confidence = confidence_from_source(&target);
                facts.push(CapabilityFact {
                    subject: routine.id.clone(),
                    op: "execute",
                    resource_kind: object_type_to_resource_kind(object_type),
                    resource_id: None,
                    resource_arg_source: Some(target),
                    confidence,
                    provenance: "direct",
                    via: "self",
                    witness_operation_id: None,
                    witness_callsite_id: Some(cs.id.clone()),
                    witness_first_hop_operation_id: None,
                    witness_first_hop_event_id: None,
                    extra: Some(CapabilityExtra::Dispatch {
                        object_type: object_type.to_string(),
                        modal,
                    }),
                });
            }
            _ => {}
        }
    }

    // ── http family (al-sem http.ts) ───────────────────────────────────────
    // HttpClient member calls. receiverTypeOf(receiver) === "HttpClient".
    for cs in &call_sites {
        let PCallee::Member { receiver, method } = &cs.callee else {
            continue;
        };
        if receiver_type_of(receiver) != "HttpClient" {
            continue;
        }
        let Some(http_method) = http_method(method) else {
            continue;
        };
        let is_send = http_method == "Send";
        // .Send(Request, Response): arg[0] is the body, no URL.
        // .Post/.Put/.Patch(Url, Request, Response): arg[0] URL, arg[1] body.
        let (url_info, body_info) = if is_send {
            (None, cs.argument_infos.first())
        } else {
            (cs.argument_infos.first(), cs.argument_infos.get(1))
        };
        let url_source = match url_info {
            Some(i) => classify_value_source(Some(i), &variables),
            None => ValueSource::Unknown,
        };
        let body_arg_source = body_info.map(|i| classify_value_source(Some(i), &variables));
        let confidence = confidence_from_source(&url_source);
        facts.push(CapabilityFact {
            subject: routine.id.clone(),
            op: "send",
            resource_kind: "http",
            resource_id: None,
            resource_arg_source: Some(url_source),
            confidence,
            provenance: "direct",
            via: "self",
            witness_operation_id: None,
            witness_callsite_id: Some(cs.id.clone()),
            witness_first_hop_operation_id: None,
            witness_first_hop_event_id: None,
            extra: Some(CapabilityExtra::Http {
                method: http_method.to_string(),
                body_arg_source,
            }),
        });
    }

    // ── telemetry family (al-sem telemetry.ts) ─────────────────────────────
    // Session.LogMessage(...) member OR bare LogMessage(...).
    for cs in &call_sites {
        let matches = match &cs.callee {
            PCallee::Member { receiver, method } => {
                receiver.to_lowercase() == "session" && method.to_lowercase() == "logmessage"
            }
            PCallee::Bare { name } => name.to_lowercase() == "logmessage",
            _ => false,
        };
        if !matches {
            continue;
        }
        let event_id_source = match cs.argument_infos.first() {
            Some(i) => classify_value_source(Some(i), &variables),
            None => ValueSource::Unknown,
        };
        let confidence = confidence_from_source(&event_id_source);
        facts.push(CapabilityFact {
            subject: routine.id.clone(),
            op: "log",
            resource_kind: "telemetry",
            resource_id: None,
            resource_arg_source: Some(event_id_source),
            confidence,
            provenance: "direct",
            via: "self",
            witness_operation_id: None,
            witness_callsite_id: Some(cs.id.clone()),
            witness_first_hop_operation_id: None,
            witness_first_hop_event_id: None,
            extra: None,
        });
    }

    // ── isolated-storage family (al-sem isolated-storage.ts) ───────────────
    for cs in &call_sites {
        let PCallee::Member { receiver, method } = &cs.callee else {
            continue;
        };
        if receiver.to_lowercase() != "isolatedstorage" {
            continue;
        }
        let Some(op) = isolated_storage_op(&method.to_lowercase()) else {
            continue;
        };
        let key_source = match cs.argument_infos.first() {
            Some(i) => classify_value_source(Some(i), &variables),
            None => ValueSource::Unknown,
        };
        let confidence = confidence_from_source(&key_source);
        // store-write: capture value arg (arg[1]) + scope (arg[2]).
        let (value_arg_source, scope) = if op == "store-write" {
            let value = cs
                .argument_infos
                .get(1)
                .map(|i| classify_value_source(Some(i), &variables));
            let scope = cs.argument_infos.get(2).map(|i| parse_data_scope(&i.text));
            (value, scope)
        } else {
            (None, None)
        };
        facts.push(CapabilityFact {
            subject: routine.id.clone(),
            op,
            resource_kind: "isolated-storage",
            resource_id: None,
            resource_arg_source: Some(key_source.clone()),
            confidence,
            provenance: "direct",
            via: "self",
            witness_operation_id: None,
            witness_callsite_id: Some(cs.id.clone()),
            witness_first_hop_operation_id: None,
            witness_first_hop_event_id: None,
            extra: Some(CapabilityExtra::Storage {
                key_arg_source: Some(key_source),
                value_arg_source,
                scope,
            }),
        });
    }

    // ── hyperlink family (al-sem hyperlink.ts) ─────────────────────────────
    // Bare Hyperlink(url) → op=open, resourceKind=ui.
    for cs in &call_sites {
        let PCallee::Bare { name } = &cs.callee else {
            continue;
        };
        if name.to_lowercase() != "hyperlink" {
            continue;
        }
        let url_source = match cs.argument_infos.first() {
            Some(i) => classify_value_source(Some(i), &variables),
            None => ValueSource::Unknown,
        };
        let confidence = confidence_from_source(&url_source);
        facts.push(CapabilityFact {
            subject: routine.id.clone(),
            op: "open",
            resource_kind: "ui",
            resource_id: None,
            resource_arg_source: Some(url_source),
            confidence,
            provenance: "direct",
            via: "self",
            witness_operation_id: None,
            witness_callsite_id: Some(cs.id.clone()),
            witness_first_hop_operation_id: None,
            witness_first_hop_event_id: None,
            extra: None,
        });
    }

    // ── file-blob family (al-sem file-blob.ts) ─────────────────────────────
    // Write-side File / TempBlob member methods. op=write-blob, resourceKind=file.
    for cs in &call_sites {
        let PCallee::Member { receiver, method } = &cs.callee else {
            continue;
        };
        let method_lc = method.to_lowercase();
        let receiver_type = receiver_type_of(receiver);
        let is_file = receiver_type == "File"
            && matches!(method_lc.as_str(), "create" | "writealltext" | "copy");
        let is_temp_blob = is_temp_blob_type(&receiver_type) && method_lc == "createoutstream";
        if !is_file && !is_temp_blob {
            continue;
        }
        let arg_source = match cs.argument_infos.first() {
            Some(i) => classify_value_source(Some(i), &variables),
            None => ValueSource::Unknown,
        };
        let confidence = confidence_from_source(&arg_source);
        facts.push(CapabilityFact {
            subject: routine.id.clone(),
            op: "write-blob",
            resource_kind: "file",
            resource_id: None,
            resource_arg_source: Some(arg_source),
            confidence,
            provenance: "direct",
            via: "self",
            witness_operation_id: None,
            witness_callsite_id: Some(cs.id.clone()),
            witness_first_hop_operation_id: None,
            witness_first_hop_event_id: None,
            extra: None,
        });
    }

    // ── background family (al-sem background.ts) ───────────────────────────
    // TaskScheduler.CreateTask(arg0) / Session.StartSession(_, arg1) / bare
    // StartSession(_, arg1). op=start, resourceKind=background.
    for cs in &call_sites {
        let codeunit_arg_idx: Option<usize> = match &cs.callee {
            PCallee::Member { receiver, method } => {
                let r = receiver.to_lowercase();
                let m = method.to_lowercase();
                if r == "taskscheduler" && m == "createtask" {
                    Some(0)
                } else if r == "session" && m == "startsession" {
                    Some(1)
                } else {
                    None
                }
            }
            PCallee::Bare { name } => {
                if name.to_lowercase() == "startsession" {
                    Some(1)
                } else {
                    None
                }
            }
            _ => None,
        };
        let Some(idx) = codeunit_arg_idx else {
            continue;
        };
        let codeunit_source = match cs.argument_infos.get(idx) {
            Some(i) => classify_value_source(Some(i), &variables),
            None => ValueSource::Unknown,
        };
        let confidence = confidence_from_source(&codeunit_source);
        facts.push(CapabilityFact {
            subject: routine.id.clone(),
            op: "start",
            resource_kind: "background",
            resource_id: None,
            resource_arg_source: Some(codeunit_source),
            confidence,
            provenance: "direct",
            via: "self",
            witness_operation_id: None,
            witness_callsite_id: Some(cs.id.clone()),
            witness_first_hop_operation_id: None,
            witness_first_hop_event_id: None,
            extra: None,
        });
    }

    // ── ui family (al-sem ui.ts) ───────────────────────────────────────────
    // Bare Confirm / Message / Error → ui-confirm / ui-message / ui-error.
    for cs in &call_sites {
        let PCallee::Bare { name } = &cs.callee else {
            continue;
        };
        let op = match name.to_lowercase().as_str() {
            "confirm" => "ui-confirm",
            "message" => "ui-message",
            "error" => "ui-error",
            _ => continue,
        };
        facts.push(CapabilityFact {
            subject: routine.id.clone(),
            op,
            resource_kind: "ui",
            resource_id: None,
            resource_arg_source: None,
            confidence: "static",
            provenance: "direct",
            via: "self",
            witness_operation_id: None,
            witness_callsite_id: Some(cs.id.clone()),
            witness_first_hop_operation_id: None,
            witness_first_hop_event_id: None,
            extra: None,
        });
    }

    // ── ui-window-open family (al-sem ui-window-open.ts) ───────────────────
    // Bare StrMenu; Page.Run / Report.Run (object-run); Page/Report.RunModal
    // (static keyword receiver OR a Page/Report-typed variable receiver).
    for cs in &call_sites {
        let matched = match &cs.callee {
            PCallee::Bare { name } => name.to_lowercase() == "strmenu",
            PCallee::ObjectRun { object_kind, .. } => {
                object_kind == "Page" || object_kind == "Report"
            }
            PCallee::Member { receiver, method } => {
                if method.to_lowercase() != "runmodal" {
                    false
                } else {
                    let r = receiver.to_lowercase();
                    if r == "page" || r == "report" {
                        true
                    } else {
                        let declared = receiver_type_of(&r);
                        declared != "unknown" && is_page_or_report_type(&declared)
                    }
                }
            }
            _ => false,
        };
        if !matched {
            continue;
        }
        facts.push(CapabilityFact {
            subject: routine.id.clone(),
            op: "ui-window-open",
            resource_kind: "ui",
            resource_id: None,
            resource_arg_source: None,
            confidence: "static",
            provenance: "direct",
            via: "self",
            witness_operation_id: None,
            witness_callsite_id: Some(cs.id.clone()),
            witness_first_hop_operation_id: None,
            witness_first_hop_event_id: None,
            extra: None,
        });
    }

    // ── events family — SUBSCRIBE side (al-sem events.ts) ──────────────────
    // A routine with an [EventSubscriber(...)] attribute emits one subscribe fact.
    if routine
        .attributes_parsed
        .iter()
        .any(|a| a.name.eq_ignore_ascii_case("EventSubscriber"))
    {
        facts.push(CapabilityFact {
            subject: routine.id.clone(),
            op: "subscribe",
            resource_kind: "event",
            resource_id: None,
            resource_arg_source: None,
            confidence: "static",
            provenance: "direct",
            via: "self",
            witness_operation_id: None,
            witness_callsite_id: None,
            witness_first_hop_operation_id: None,
            witness_first_hop_event_id: None,
            extra: Some(CapabilityExtra::Event {
                event_class: "Integration".to_string(),
                include_sender: None,
            }),
        });
    }

    // ── error family (al-sem error.ts) ─────────────────────────────────────
    // One fact per operationSite with kind === "error-call".
    for op in &operation_sites {
        if op.kind == "error-call" {
            facts.push(CapabilityFact {
                subject: routine.id.clone(),
                op: "error-throw",
                resource_kind: "error",
                resource_id: None,
                resource_arg_source: None,
                confidence: "static",
                provenance: "direct",
                via: "self",
                witness_operation_id: Some(op.id.clone()),
                witness_callsite_id: None,
                witness_first_hop_operation_id: None,
                witness_first_hop_event_id: None,
                extra: None,
            });
        }
    }

    // ── publisher-fact injection (summary-runner.ts:615-632) ───────────────
    // One direct `publish` fact per EventSymbol whose publisherRoutineId is this
    // routine. Appended AFTER the family facts (al-sem `facts = [...facts, f]`).
    for evt in publisher_events {
        facts.push(CapabilityFact {
            subject: routine.id.clone(),
            op: "publish",
            resource_kind: "event",
            resource_id: Some(evt.id.clone()),
            resource_arg_source: None,
            confidence: "static",
            provenance: "direct",
            via: "self",
            witness_operation_id: None,
            witness_callsite_id: None,
            witness_first_hop_operation_id: None,
            witness_first_hop_event_id: None,
            extra: Some(CapabilityExtra::Event {
                event_class: map_event_kind_to_class(&evt.event_kind).to_string(),
                include_sender: None,
            }),
        });
    }

    let status = if reasons.is_empty() {
        "complete"
    } else {
        "partial"
    };
    (facts, status.to_string(), reasons)
}

// ===========================================================================
// The CONE — ports capability-cone.ts. Internal ids throughout.
// ===========================================================================

/// A resolved outgoing typed edge (only edges with a `to`). Mirrors al-sem
/// `TypedOutEdge`.
#[derive(Debug, Clone)]
pub struct TypedOutEdge {
    pub(crate) to: String,
    pub(crate) kind: String,
    pub(crate) callsite: Option<String>,
    pub(crate) event_id: Option<String>,
    /// The record operation that fires an `implicit-trigger` edge (`"{routine}/opN"`,
    /// #96 D2); `None` on every other kind. It is part of the edge's identity:
    /// two ops of one caller that fire the same trigger are two edges, each
    /// binding the trigger's `Rec` to its own op's receiver (`pd_solve::bound_site`).
    pub(crate) operation_id: Option<String>,
    /// `edge_sort_key(self)`, computed ONCE at graph build.
    ///
    /// It is both the out-edge sort key and the first-hop tie-breaker in
    /// `inherited_facts_for_singleton`, which used to call `edge_sort_key` TWICE
    /// per comparison — each call a `format!` allocation — inside a loop that runs
    /// per cone entry per out-edge over 100,419 singleton routines. The value is a
    /// pure function of the edge's identity fields, none of which change after
    /// construction, so computing it here is the same string, once.
    pub(crate) sort_key: String,
}

impl TypedOutEdge {
    pub(crate) fn new(
        to: String,
        kind: String,
        callsite: Option<String>,
        event_id: Option<String>,
        operation_id: Option<String>,
    ) -> Self {
        let mut out = TypedOutEdge {
            to,
            kind,
            callsite,
            event_id,
            operation_id,
            sort_key: String::new(),
        };
        out.sort_key = edge_sort_key(&out);
        out
    }

    /// The caller-frame site this edge is made at: its call site, or, for an
    /// `implicit-trigger` edge, the record operation that fires it. A caller's
    /// guards (`FrameGuards::by_site`) are keyed by both kinds of id, so this is
    /// the key for the requirements under which the caller takes this edge
    /// (#96 review CR3).
    pub(crate) fn site(&self) -> Option<&str> {
        self.callsite.as_deref().or(self.operation_id.as_deref())
    }
}

/// The typed-edge graph: per-routine sorted out-edges + the unresolved sources.
/// Mirrors al-sem `TypedEdgeGraph`.
pub(crate) struct TypedEdgeGraph {
    pub(crate) nodes: Vec<String>,
    pub(crate) outgoing: HashMap<String, Vec<TypedOutEdge>>,
    pub(crate) unresolved_sources: BTreeSet<String>,
}

/// `edgeSortKey(e)` = `${kind}|${callsite ?? ""}|${eventId ?? ""}|${to}`, plus
/// `|${operationId}` when the edge has one (only `implicit-trigger` edges do,
/// #96 D2), so every other edge's key is unchanged. Op ids compare as strings
/// (`…/op10` sorts before `…/op2`).
fn edge_sort_key(e: &TypedOutEdge) -> String {
    let mut key = format!(
        "{}|{}|{}|{}",
        e.kind,
        e.callsite.as_deref().unwrap_or(""),
        e.event_id.as_deref().unwrap_or(""),
        e.to
    );
    if let Some(op) = &e.operation_id {
        key.push('|');
        key.push_str(op);
    }
    key
}

/// Map a typed-edge kind to the inherited `via`. Mirrors
/// `capabilityViaForEdgeKind`.
fn capability_via_for_edge_kind(kind: &str) -> &'static str {
    match kind {
        "direct-call" | "variable-typed-call" | "interface-dispatch" => "call",
        "object-run-resolved" | "object-run-unresolved" => "object-run",
        "event-dispatch" => "event-dispatch",
        "implicit-trigger" => "implicit-trigger",
        "dependency-export" => "dependency",
        _ => "call",
    }
}

/// The callsiteId carried by a typed out-edge (al-sem `callsiteIdForEdge`).
fn callsite_id_for_edge(e: &TypedEdge) -> Option<String> {
    match e.kind.as_str() {
        "direct-call"
        | "variable-typed-call"
        | "interface-dispatch"
        | "object-run-resolved"
        | "object-run-unresolved"
        | "dependency-export" => e.callsite_id.clone(),
        _ => None,
    }
}

/// Build the typed-edge graph from the typed edges (the Cone payload,
/// `Transfer::Cone`) + the node list. Mirrors al-sem `buildTypedEdgeGraph`.
fn build_typed_edge_graph(typed: &[TypedEdge], nodes: &[String]) -> TypedEdgeGraph {
    let mut outgoing: HashMap<String, Vec<TypedOutEdge>> = HashMap::new();
    let mut unresolved_sources: BTreeSet<String> = BTreeSet::new();

    for edge in typed {
        if edge.kind == "object-run-unresolved" {
            unresolved_sources.insert(edge.from.clone());
            continue;
        }
        let Some(to) = &edge.to else {
            continue;
        };
        outgoing
            .entry(edge.from.clone())
            .or_default()
            .push(TypedOutEdge::new(
                to.clone(),
                edge.kind.clone(),
                callsite_id_for_edge(edge),
                edge.event_id.clone(),
                edge.operation_id.clone(),
            ));
    }
    for list in outgoing.values_mut() {
        list.sort_by(|a, b| a.sort_key.cmp(&b.sort_key));
    }

    let mut sorted_nodes = nodes.to_vec();
    sorted_nodes.sort();

    TypedEdgeGraph {
        nodes: sorted_nodes,
        outgoing,
        unresolved_sources,
    }
}

/// Dedup key for inherited facts —
/// `op|resourceKind|resourceId|confidence|tempClass`.
///
/// ⟨issue 33⟩ The last component is the BINARY temp class
/// ([`fact_is_known_temp`]) — `kt` for a provably-temporary record operation,
/// `nt` for everything else. Without it, a known-temp and a non-known-temp fact
/// about the SAME table collide, one of the two is discarded at every reduction
/// site, and which one survives is decided by distance / `rep_key` / BFS arrival
/// — none of which is about evidence quality. On the pinned CDO baseline
/// (CROSS-APP scope) that hid a physical obligation behind a temp one at 1,755
/// keys: 292 of them WRITES across 222 routines, and **1,463 of them READS** —
/// the read set is the bigger mover, by roughly 5x. Source-only scope: 820
/// hidden, 89 writes and 731 reads.
///
/// Splitting the classes is what makes the fix ADDITIVE: no reduction site
/// changes its comparison, the widened key's candidate group is a SUBSET of the
/// old one, and the fact that used to win still wins within its own class. The
/// extra entry costs at most one per key (measured on CDO: +2.0% cone entries
/// cross-app, inside the run-to-run noise band on both wall time and peak RSS).
///
/// ORDERING SAFETY, and for the RIGHT reason. `ConeFacts` is a `BTreeMap` keyed
/// by this string, so appending a suffix flips iteration order if one base key
/// is a proper string PREFIX of another (`'k' < '|'`). This is NOT safe because
/// `confidence` is always `"static"` — that premise is false: `confidence_from_
/// source` yields `static` / `userDynamic` / `configDynamic` / `unresolved`, and
/// the r3a3 manifest records both `unresolved` and `configDynamic`. It is safe
/// because **no one of those four values is a prefix of another, and
/// `resource_id` is only ever a table id or an event id, neither of which can
/// contain `|`.** Add a `dynamic` confidence alongside `dynamicX`, or a rid that
/// can carry a `|`, and this breaks.
///
/// What this buys is CONSERVATIVE FOR THE PHYSICAL-WRITE AND PHYSICAL-READ SETS
/// — never "conservative for permissions". `writes_physical_tables_of` and
/// `physical_table_reads_of` can only grow, but the detectors built on them are
/// not monotone in the same direction: d44 needs non-empty `writers` AND a
/// non-empty `readers \ writers`, so both sets growing moves its findings in
/// both directions independently and can DELETE one; d43 can DOWNGRADE one. All
/// of it needs triage on a real workspace, the READ half included — and note
/// that `physical_table_reads_of` has exactly ONE consumer anywhere (d44), so
/// the larger half of what this unmasks is also the hardest half to observe.
fn inherited_fact_key(f: &CapabilityFact) -> String {
    format!(
        "{}|{}|{}|{}|{}",
        f.op,
        f.resource_kind,
        f.resource_id.as_deref().unwrap_or(""),
        f.confidence,
        if fact_is_known_temp(f) { "kt" } else { "nt" }
    )
}

/// Serialize a ValueSource to a stable JSON-ish string for `repKey`
/// (`JSON.stringify(resourceArgSource ?? null)` parity). Uses serde_json on a
/// canonical projection so the byte form matches al-sem's `JSON.stringify`.
fn value_source_json(vs: &Option<ValueSource>) -> String {
    match vs {
        None => "null".to_string(),
        Some(v) => serde_json::to_string(&value_source_to_json(v)).unwrap_or_default(),
    }
}

pub(crate) fn value_source_to_json(vs: &ValueSource) -> serde_json::Value {
    use serde_json::json;
    match vs {
        ValueSource::Literal { value } => json!({"kind":"literal","value":value}),
        ValueSource::Enum { enum_name, member } => match member {
            Some(m) => json!({"kind":"enum","enumName":enum_name,"member":m}),
            None => json!({"kind":"enum","enumName":enum_name}),
        },
        ValueSource::ConstantVar {
            var_name,
            initializer,
        } => {
            json!({"kind":"constant-var","varName":var_name,"initializer":value_source_to_json(initializer)})
        }
        ValueSource::Parameter { index, var_name } => {
            json!({"kind":"parameter","index":index,"varName":var_name})
        }
        ValueSource::TableField {
            table_id,
            field_name,
        } => json!({"kind":"table-field","tableId":table_id,"fieldName":field_name}),
        ValueSource::Expression => json!({"kind":"expression"}),
        ValueSource::Unknown => json!({"kind":"unknown"}),
    }
}

/// Serialize a CapabilityExtra to a stable JSON string for `repKey`
/// (`JSON.stringify(extra ?? null)` parity).
fn extra_json(extra: &Option<CapabilityExtra>) -> String {
    match extra {
        None => "null".to_string(),
        Some(e) => serde_json::to_string(&extra_to_json(e)).unwrap_or_default(),
    }
}

pub(crate) fn extra_to_json(e: &CapabilityExtra) -> serde_json::Value {
    use serde_json::json;
    match e {
        CapabilityExtra::Table {
            record_variable_id,
            temp_state,
            op_subtype,
        } => {
            let mut m = serde_json::Map::new();
            m.insert("kind".into(), json!("table"));
            if let Some(rv) = record_variable_id {
                m.insert("recordVariableId".into(), json!(rv));
            }
            if let Some(ts) = temp_state {
                m.insert("tempState".into(), temp_state_to_json(ts));
            }
            if let Some(os) = op_subtype {
                m.insert("opSubtype".into(), json!(os));
            }
            serde_json::Value::Object(m)
        }
        CapabilityExtra::Dispatch { object_type, modal } => {
            let mut m = serde_json::Map::new();
            m.insert("kind".into(), json!("dispatch"));
            m.insert("objectType".into(), json!(object_type));
            if let Some(md) = modal {
                m.insert("modal".into(), json!(md));
            }
            serde_json::Value::Object(m)
        }
        CapabilityExtra::Event {
            event_class,
            include_sender,
        } => {
            let mut m = serde_json::Map::new();
            m.insert("kind".into(), json!("event"));
            m.insert("eventClass".into(), json!(event_class));
            if let Some(is) = include_sender {
                m.insert("includeSender".into(), json!(is));
            }
            serde_json::Value::Object(m)
        }
        CapabilityExtra::Http {
            method,
            body_arg_source,
        } => {
            let mut m = serde_json::Map::new();
            m.insert("kind".into(), json!("http"));
            m.insert("method".into(), json!(method));
            if let Some(bs) = body_arg_source {
                m.insert("bodyArgSource".into(), value_source_to_json(bs));
            }
            serde_json::Value::Object(m)
        }
        CapabilityExtra::Storage {
            key_arg_source,
            value_arg_source,
            scope,
        } => {
            let mut m = serde_json::Map::new();
            m.insert("kind".into(), json!("storage"));
            if let Some(ks) = key_arg_source {
                m.insert("keyArgSource".into(), value_source_to_json(ks));
            }
            if let Some(vs) = value_arg_source {
                m.insert("valueArgSource".into(), value_source_to_json(vs));
            }
            if let Some(sc) = scope {
                m.insert("scope".into(), json!(sc));
            }
            serde_json::Value::Object(m)
        }
    }
}

fn temp_state_to_json(ts: &crate::program::body::features::PTempState) -> serde_json::Value {
    use serde_json::json;
    match ts.kind.as_str() {
        "known" => json!({"kind":"known","value": ts.value.unwrap_or(false)}),
        "parameter-dependent" => {
            json!({"kind":"parameter-dependent","parameterIndex": ts.parameter_index.unwrap_or(0)})
        }
        _ => json!({"kind":"unknown"}),
    }
}

/// Canonical representative key — deterministic, traversal-order-independent.
/// Mirrors `repKey` (the `§`-joined tuple).
fn rep_key(f: &CapabilityFact) -> String {
    [
        inherited_fact_key(f),
        f.subject.clone(),
        f.witness_operation_id.clone().unwrap_or_default(),
        f.witness_callsite_id.clone().unwrap_or_default(),
        value_source_json(&f.resource_arg_source),
        extra_json(&f.extra),
    ]
    .join("§")
}

/// One cone fact entry: the representative DIRECT fact + its min hop distance.
#[derive(Debug, Clone)]
struct ConeFactEntry {
    /// SHARED: a cone entry is copied into every predecessor's cone, and the rep
    /// itself never changes across those merges (only `dist` does), so a merge is
    /// a refcount bump rather than a deep clone of a `CapabilityFact`'s five-plus
    /// `String`s. 6,556,465 merges per BC Base App run went through here.
    rep: Arc<CapabilityFact>,
    /// `rep_key(rep)`, computed ONCE when the entry is minted.
    ///
    /// `merge_cone`'s tie-break used to call `rep_key` on BOTH sides of every
    /// collision — 1,243,002 collisions per 8020 run, so ~2.5 M calls, each one
    /// building a `String` by JSON-serializing `resource_arg_source` and `extra`.
    /// The rep is immutable across merges, so the key is too.
    rep_key: Arc<str>,
    dist: usize,
    /// `Some`: the fact happens only when this cone's routine receives the
    /// required boolean argument values ([`super::param_guard`]); the cone key
    /// carries them as a suffix, so a guarded and an unguarded copy of one fact
    /// never collapse. Only a non-recursive singleton's cone keeps it;
    /// everywhere else it is dropped (the fact becomes unconditional).
    ///
    /// A cone holds no parameter-dependent PRODUCER ([`is_producer`]): those
    /// are resolved per routine by [`super::pd_solve`] (#69 D1), so every
    /// entry's rep already has its key's temp class.
    guard: Option<Arc<EntryGuard>>,
}

/// A guarded cone entry's requirements and its key without the guard suffix.
#[derive(Debug)]
struct EntryGuard {
    reqs: Vec<super::param_guard::Req>,
    base_key: Arc<str>,
}

/// The cone key of a fact with base key `base` under `reqs` (`base` itself
/// when there is no requirement).
fn guarded_key(base: &Arc<str>, reqs: &[super::param_guard::Req]) -> Arc<str> {
    if reqs.is_empty() {
        return Arc::clone(base);
    }
    let mut k = String::with_capacity(base.len() + 4 * reqs.len());
    k.push_str(base);
    k.push_str("|g");
    for (i, v) in reqs {
        k.push_str(&format!("{i}{}", if *v { 't' } else { 'f' }));
    }
    Arc::from(k)
}

/// `(key, guard)` for a fact with base key `base` under `reqs`.
fn with_reqs(
    base: Arc<str>,
    reqs: Vec<super::param_guard::Req>,
) -> (Arc<str>, Option<Arc<EntryGuard>>) {
    if reqs.is_empty() {
        return (base, None);
    }
    let key = guarded_key(&base, &reqs);
    (
        key,
        Some(Arc::new(EntryGuard {
            reqs,
            base_key: base,
        })),
    )
}

/// The key a cone entry has without its guard suffix.
fn base_key_of<'k>(key: &'k Arc<str>, entry: &'k ConeFactEntry) -> &'k Arc<str> {
    entry.guard.as_ref().map_or(key, |g| &g.base_key)
}

/// Whether an entry is bound to its cone routine's frame (its guard is
/// resolved per call edge).
fn frame_bound(entry: &ConeFactEntry) -> bool {
    entry.guard.is_some()
}

/// Whether a raw direct fact is a PRODUCER (#69 D1.1): a table fact whose temp
/// state is `parameter-dependent` in its owner's frame. Producers never enter a
/// shared cone; [`super::pd_solve::solve`] resolves them in every routine that
/// reaches them.
///
/// The test is the temp state alone, like digest's `is_pd_snap`, so the cone
/// and the witness BFS can never disagree on what a producer is. Every table
/// fact carries its operation id (`direct_facts_for_routine`); the producer
/// collection in `compose_inherited_cones` `expect`s it.
fn is_producer(f: &CapabilityFact) -> bool {
    fact_pd_index(f).is_some()
}

/// A routine's raw direct facts deduplicated by [`inherited_fact_key`]
/// (canonical rep per key: min `rep_key`), producers excluded. The ONE builder
/// of the cone's `direct` map.
fn dedup_direct(facts: &[CapabilityFact]) -> BTreeMap<String, CapabilityFact> {
    let mut byk: BTreeMap<String, CapabilityFact> = BTreeMap::new();
    for f in facts.iter().filter(|f| !is_producer(f)) {
        let k = inherited_fact_key(f);
        match byk.get(&k) {
            Some(cur) if rep_key(f) >= rep_key(cur) => {}
            _ => {
                byk.insert(k, f.clone());
            }
        }
    }
    byk
}

/// The `parameter-dependent` index of a table fact, if it has one.
pub(crate) fn fact_pd_index(f: &CapabilityFact) -> Option<u32> {
    use super::summary::TempState;
    match &f.extra {
        Some(CapabilityExtra::Table {
            temp_state: Some(ts),
            ..
        }) => match TempState::from_p(ts) {
            TempState::ParameterDependent(i) => Some(i),
            _ => None,
        },
        _ => None,
    }
}

/// Carry one GUARDED cone entry of callee `edge.to` across `edge` into the
/// caller's frame ([`super::pd_solve::carry_reqs`], the guard step the
/// producer solve shares), then add `edge_reqs`, the requirements under which
/// the caller makes this call. `None`: an argument contradicts a requirement,
/// so the fact cannot happen through this edge. The rep and its temp state are
/// unchanged: a cone holds no producer.
fn substitute_entry(
    key: &Arc<str>,
    entry: &ConeFactEntry,
    edge: &TypedOutEdge,
    caller: Option<&ModelRoutine>,
    callee: Option<&ModelRoutine>,
    caller_frame: Option<&super::param_guard::FrameGuards>,
    edge_reqs: &[super::param_guard::Req],
) -> Option<(Arc<str>, ConeFactEntry)> {
    let reqs = entry.guard.as_ref().map_or(&[][..], |g| g.reqs.as_slice());
    let reqs = super::pd_solve::carry_reqs(reqs, edge, caller, callee, caller_frame, edge_reqs)?;
    let (key, guard) = with_reqs(Arc::clone(base_key_of(key, entry)), reqs);
    Some((
        key,
        ConeFactEntry {
            rep: Arc::clone(&entry.rep),
            rep_key: Arc::clone(&entry.rep_key),
            dist: entry.dist,
            guard,
        },
    ))
}

/// A fact cone: dedup key → entry (min dist, tie-broken by canonical rep).
///
/// The key is `Arc<str>`, not `String`, for the same reason [`ConeFactEntry`]'s
/// `rep`/`rep_key` are: a cone entry is copied into every predecessor's cone, and
/// its key is IMMUTABLE across those copies. `ALSEM_CONES_CENSUS=1` counted
/// **6,556,465 `merge_cone` calls per BC Base App 8020 run**, each of which used
/// to pass an owned `key.clone()` — a fresh heap copy of a string that already
/// existed in the successor cone being read. Only ~121,387 of those (one per
/// direct fact) genuinely mint a key; the other ~6.43 M are re-copies.
///
/// `Arc<str>` orders through `str`'s `Ord`, exactly as `String` does, so the
/// `BTreeMap`'s iteration order — which every downstream consumer depends on — is
/// unchanged by construction.
type ConeFacts = BTreeMap<Arc<str>, ConeFactEntry>;

/// Merge `entry` (already distance-shifted) into `dst` at `key`, keeping min
/// dist; tie-break by canonical rep. Mirrors `mergeCone`.
///
/// `key` is BORROWED: the lookup goes through `Arc<str>: Borrow<str>`, so a merge
/// that loses its tie-break — 1,243,002 of the calls collide, and a collision that
/// loses inserts nothing — touches no refcount at all, and a winning merge pays
/// one `Arc::clone` rather than a heap copy.
fn merge_cone(dst: &mut ConeFacts, key: &Arc<str>, entry: ConeFactEntry) {
    crate::engine::l5::detector_context::cones_census::add_gated(
        &crate::engine::l5::detector_context::cones_census::MERGE_CALLS,
        1,
    );
    match dst.get(&**key) {
        None => {
            dst.insert(Arc::clone(key), entry);
        }
        Some(existing) => {
            crate::engine::l5::detector_context::cones_census::add_gated(
                &crate::engine::l5::detector_context::cones_census::MERGE_TIEBREAKS,
                1,
            );
            // min dist wins; equal dist → smaller canonical rep wins (mergeCone).
            let wins = entry.dist < existing.dist
                || (entry.dist == existing.dist && entry.rep_key < existing.rep_key);
            if wins {
                dst.insert(Arc::clone(key), entry);
            }
        }
    }
}

/// Re-tag a representative direct fact as an inherited fact on `subject` via a
/// first-hop edge. Mirrors `retag`.
fn retag(rep: &CapabilityFact, subject: &str, edge: &TypedOutEdge) -> CapabilityFact {
    CapabilityFact {
        subject: subject.to_string(),
        provenance: "inherited",
        via: capability_via_for_edge_kind(&edge.kind),
        witness_callsite_id: edge.callsite.clone(),
        // Set for EVERY edge, never inherited from `rep` (#96 D5, N4): the op
        // id of a trigger first hop, `None` otherwise, so a stale op id cannot
        // leak through `..rep.clone()`.
        witness_first_hop_operation_id: edge.operation_id.clone(),
        // Same rule for the event id of an event-dispatch first hop (#98 D1).
        witness_first_hop_event_id: edge.event_id.clone(),
        ..rep.clone()
    }
}

/// Sort key for the final capabilityFactsInherited array. Mirrors
/// `inheritedOutputSortKey`.
fn inherited_output_sort_key(f: &CapabilityFact) -> String {
    // ⟨C1 Task 4⟩ Borrowed throughout — `join` copies into the one output
    // String either way, so the seven per-call clones this used to make were
    // pure waste (this runs once per fact per `sort_inherited`). Byte-identical:
    // `as_deref().unwrap_or_default()` is `""` for `None`, exactly what
    // `clone().unwrap_or_default()` produced.
    [
        f.op,
        f.resource_kind,
        f.resource_id.as_deref().unwrap_or_default(),
        f.confidence,
        f.via,
        f.witness_callsite_id.as_deref().unwrap_or_default(),
        f.witness_operation_id.as_deref().unwrap_or_default(),
    ]
    .join("|")
}

fn sort_inherited(mut facts: Vec<CapabilityFact>) -> Vec<CapabilityFact> {
    facts.sort_by_key(inherited_output_sort_key);
    facts
}

/// Per-routine direct facts grouped by dedup key (canonical rep per key).
type RoutineDirectFacts = HashMap<String, BTreeMap<String, CapabilityFact>>;

/// Minimal per-routine direct coverage the cone roll-up needs.
struct DirectCoverage {
    direct_status: String,
    reasons: Vec<String>,
}
type RoutineDirectCoverage = HashMap<String, DirectCoverage>;

/// A coverage cone (includes self). Mirrors `CoverageCone`.
#[derive(Debug, Clone)]
struct CoverageCone {
    complete: bool,
    reasons: Vec<String>,
    unknown_targets: Vec<String>,
}

/// The per-key witness rule of a routine's inherited output: min dist wins;
/// equal dist → smaller edgeSortKey of the first hop wins. Mirrors
/// inheritedFactsForSingleton. `cur`: the current winner's `(dist, first hop)`.
fn candidate_wins(dist: usize, edge: &TypedOutEdge, cur: Option<(usize, &TypedOutEdge)>) -> bool {
    match cur {
        None => true,
        Some((d, e)) => dist < d || (dist == d && edge.sort_key < e.sort_key),
    }
}

/// The closed-set memo of one composition (#160 step 7b §4.3): a callee
/// Cone-SCC's index → F(the closed entries of its fact cone), built at the
/// first read and freed with the cone.
type ClosedRowMemo = HashMap<usize, FoldedRow>;

/// Singleton non-recursive fast path. Mirrors `inheritedFactsForSingleton`.
///
/// ⟨C1⟩ `best` holds BORROWED representatives/edges (it used to clone both on
/// every win): the winners' fields are read identically whether borrowed or
/// cloned, and only the survivors are cloned — by `retag`, and only when `mode`
/// wants the raw Vec.
///
/// #160 step 7b: `best` is built only for the raw Vec. Under
/// [`ConeOutput::DerivedOnly`] there is no scan: the derived row comes from
/// `memo` rows, the open entries and the own entries.
#[allow(clippy::too_many_arguments)]
fn inherited_facts_for_singleton<'g>(
    subject: &str,
    g: &'g TypedEdgeGraph,
    scc_id_by_routine: &HashMap<String, usize>,
    cones: &'g HashMap<usize, ConeFacts>,
    caller: Option<&ModelRoutine>,
    caller_frame: Option<&super::param_guard::FrameGuards>,
    routines_by_id: &HashMap<&str, &ModelRoutine>,
    own: &'g [super::pd_solve::ResolvedEntry<'g>],
    mode: ConeOutput,
    derived: &mut ConeDerivedBuilder,
    memo: &mut ClosedRowMemo,
    stats: &mut ComposeStats,
) -> Vec<CapabilityFact> {
    struct Best<'a> {
        rep: &'a CapabilityFact,
        dist: usize,
        edge: &'a TypedOutEdge,
    }
    // ⟨C1 Task 4 hardening⟩ A root SCC's cone is never built
    // (`compose_inherited_cones`'s `root_scc` skip) — the only thing keeping
    // this walk from ever looking one up is that a non-recursive singleton
    // has no self-edge, so `edge.to` can never land back in `subject`'s own
    // SCC. Assert it: a silent `None` here (via the `let else` below) would
    // read as "no inherited facts from this hop" instead of the bug it is.
    let my_scc = scc_id_by_routine.get(subject).copied();
    let out_edges: &'g [TypedOutEdge] =
        g.outgoing.get(subject).map(|v| v.as_slice()).unwrap_or(&[]);
    // Frame-bound (guarded) entries are carried across their edge into this
    // routine's frame FIRST, per edge (`substitute_entry`); the scan below
    // skips them in the cone and reads these instead. A guarded entry an
    // argument contradicts is dropped here. Rare: only sites under a
    // boolean-parameter guard are guarded.
    let _subst_t = crate::engine::l5::detector_context::cones_census::start();
    let subst: Vec<Vec<(Arc<str>, ConeFactEntry)>> = out_edges
        .iter()
        .map(|edge| {
            let Some(ycone) = scc_id_by_routine.get(&edge.to).and_then(|yj| cones.get(yj)) else {
                return Vec::new();
            };
            ycone
                .iter()
                .filter(|(_, e)| frame_bound(e))
                .filter_map(|(k, e)| {
                    substitute_entry(
                        k,
                        e,
                        edge,
                        caller,
                        routines_by_id.get(edge.to.as_str()).copied(),
                        caller_frame,
                        &[],
                    )
                })
                .collect()
        })
        .collect();
    crate::engine::l5::detector_context::cones_census::add_since(
        &crate::engine::l5::detector_context::cones_census::SGL_SUBST_NANOS,
        _subst_t,
    );
    // #160 step 7b §4.3: the derived row is F(memo rows of the distinct callee
    // SCCs) ∪ F(substituted open entries) ∪ F(own entries); F is a function of
    // the key, so it equals the fold of `best` (spec §4.1).
    if mode.wants_derived() {
        // J(m), in out-edge order. Small (at most 156 on CDO): `contains`.
        let mut callee_sccs: Vec<usize> = Vec::new();
        for edge in out_edges {
            let Some(&yj) = scc_id_by_routine.get(&edge.to) else {
                continue;
            };
            debug_assert_ne!(
                Some(yj),
                my_scc,
                "inherited_facts_for_singleton({subject:?}): out-edge to {:?} maps back to \
                 the subject's own SCC",
                edge.to
            );
            if cones.contains_key(&yj) && !callee_sccs.contains(&yj) {
                callee_sccs.push(yj);
            }
        }
        for j in callee_sccs {
            if let std::collections::hash_map::Entry::Vacant(slot) = memo.entry(j) {
                let _t = crate::engine::l5::detector_context::cones_census::start();
                let mut entries = 0u64;
                let row = derived.fold_set(
                    cones[&j]
                        .values()
                        .filter(|e| !frame_bound(e))
                        .inspect(|_| entries += 1)
                        .map(|e| &*e.rep),
                );
                stats.closed_memo_builds += 1;
                stats.closed_memo_entries += entries;
                slot.insert(row);
                if crate::engine::l5::detector_context::cones_census::enabled() {
                    crate::engine::l5::detector_context::cones_census::max(
                        &crate::engine::l5::detector_context::cones_census::CLOSED_MEMO_LIVE_MAX,
                        memo.len() as u64,
                    );
                }
                crate::engine::l5::detector_context::cones_census::add_since(
                    &crate::engine::l5::detector_context::cones_census::SGL_MEMO_NANOS,
                    _t,
                );
            }
            let _t = crate::engine::l5::detector_context::cones_census::start();
            derived.fold_row(&memo[&j]);
            stats.closed_memo_reads += 1;
            crate::engine::l5::detector_context::cones_census::add_since(
                &crate::engine::l5::detector_context::cones_census::SGL_ROW_NANOS,
                _t,
            );
        }
        let _t = crate::engine::l5::detector_context::cones_census::start();
        for (_, e) in subst.iter().flatten() {
            derived.fold_fact(&e.rep);
        }
        crate::engine::l5::detector_context::cones_census::add_since(
            &crate::engine::l5::detector_context::cones_census::SGL_OPEN_NANOS,
            _t,
        );
        let _t = crate::engine::l5::detector_context::cones_census::start();
        for e in own {
            derived.fold_fact(&e.rep);
        }
        crate::engine::l5::detector_context::cones_census::add_since(
            &crate::engine::l5::detector_context::cones_census::SGL_OWN_FOLD_NANOS,
            _t,
        );
        if !mode.wants_raw() {
            return Vec::new();
        }
    }
    stats.singleton_scans += 1;
    // Keys BORROW from `cones` (and `subst`) instead of being cloned. The
    // census counted 10,522,793 `best` insertions per 8020 run against only
    // 136,952 out-edges — i.e. the walk is cone-ENTRY-bound, and every one of
    // those insertions was allocating a fresh `String` copy of a key that
    // already lives in `cones` for the whole walk.
    let own_keys: Vec<String> = own.iter().map(|e| inherited_fact_key(&e.rep)).collect();
    let mut best: BTreeMap<&str, Best<'_>> = BTreeMap::new();
    // Census populations: plain locals, folded into the atomics ONCE per call.
    let mut c_edges: u64 = 0;
    // Scanned cone entries (closed + open), the open ones among them, and own.
    let mut c_scanned: u64 = 0;
    let mut c_open: u64 = 0;
    let mut c_own: u64 = 0;
    let mut c_wins: u64 = 0;
    let mut c_cone_edges: u64 = 0;
    let _scan_t = crate::engine::l5::detector_context::cones_census::start();
    for (edge, edge_subst) in out_edges.iter().zip(&subst) {
        c_edges += 1;
        let Some(yj) = scc_id_by_routine.get(&edge.to) else {
            continue;
        };
        debug_assert_ne!(
            Some(*yj),
            my_scc,
            "inherited_facts_for_singleton({subject:?}): out-edge to {:?} maps back to \
             the subject's own SCC — a root SCC's cone is never built, so this would \
             silently look up a missing/foreign cone instead of panicking",
            edge.to
        );
        let Some(ycone) = cones.get(yj) else {
            continue;
        };
        c_cone_edges += 1;
        c_open += edge_subst.len() as u64;
        let entries = ycone
            .iter()
            .filter(|(_, e)| !frame_bound(e))
            .chain(edge_subst.iter().map(|(k, e)| (k, e)));
        for (key, entry) in entries {
            c_scanned += 1;
            // A guard does not matter at the subject: its own parameters are
            // unknown here, so a guarded fact is a possible one. One fact, one key.
            let key = base_key_of(key, entry);
            let cand_dist = entry.dist + 1;
            let wins = candidate_wins(cand_dist, edge, best.get(&**key).map(|c| (c.dist, c.edge)));
            if wins {
                c_wins += 1;
                best.insert(
                    &**key,
                    Best {
                        rep: &entry.rep,
                        dist: cand_dist,
                        edge,
                    },
                );
            }
        }
    }
    // #69 D1.3 use (a): the subject's own producer-solve entries compete under
    // the same rule. They are its COMPLETE parameter-dependent-origin output:
    // no cone carries a producer.
    crate::engine::l5::detector_context::cones_census::add_since(
        &crate::engine::l5::detector_context::cones_census::SGL_SCAN_CONE_NANOS,
        _scan_t,
    );
    let _own_t = crate::engine::l5::detector_context::cones_census::start();
    for (e, key) in own.iter().zip(&own_keys) {
        c_own += 1;
        if candidate_wins(
            e.dist,
            e.first_hop,
            best.get(key.as_str()).map(|c| (c.dist, c.edge)),
        ) {
            c_wins += 1;
            best.insert(
                key,
                Best {
                    rep: &e.rep,
                    dist: e.dist,
                    edge: e.first_hop,
                },
            );
        }
    }
    crate::engine::l5::detector_context::cones_census::add_since(
        &crate::engine::l5::detector_context::cones_census::SGL_SCAN_OWN_NANOS,
        _own_t,
    );
    let c_entries = c_scanned + c_own;
    crate::engine::l5::detector_context::cones_census::add(
        &crate::engine::l5::detector_context::cones_census::SGL_EDGES,
        c_edges,
    );
    crate::engine::l5::detector_context::cones_census::add_gated(
        &crate::engine::l5::detector_context::cones_census::SGL_CLOSED_ENTRIES,
        c_scanned - c_open,
    );
    crate::engine::l5::detector_context::cones_census::add_gated(
        &crate::engine::l5::detector_context::cones_census::SGL_OPEN_ENTRIES,
        c_open,
    );
    crate::engine::l5::detector_context::cones_census::add_gated(
        &crate::engine::l5::detector_context::cones_census::SGL_OWN_ENTRIES,
        c_own,
    );
    crate::engine::l5::detector_context::cones_census::add(
        &crate::engine::l5::detector_context::cones_census::SGL_WINS,
        c_wins,
    );
    match c_cone_edges {
        0 => crate::engine::l5::detector_context::cones_census::add(
            &crate::engine::l5::detector_context::cones_census::SGL_CALLS_0_CONES,
            1,
        ),
        1 => crate::engine::l5::detector_context::cones_census::add(
            &crate::engine::l5::detector_context::cones_census::SGL_CALLS_1_CONE,
            1,
        ),
        _ => {
            crate::engine::l5::detector_context::cones_census::add(
                &crate::engine::l5::detector_context::cones_census::SGL_CALLS_N_CONES,
                1,
            );
            crate::engine::l5::detector_context::cones_census::add(
                &crate::engine::l5::detector_context::cones_census::SGL_ENTRIES_N_CONES,
                c_entries,
            );
        }
    }
    crate::engine::l5::detector_context::cones_census::add(
        &crate::engine::l5::detector_context::cones_census::SGL_BESTS,
        best.len() as u64,
    );

    // Only the raw half reaches here (`DerivedOnly` returned above).
    let _t = crate::engine::l5::detector_context::cones_census::start();
    let out: Vec<CapabilityFact> = best
        .values()
        .map(|b| retag(b.rep, subject, b.edge))
        .collect();
    let sorted = sort_inherited(out);
    crate::engine::l5::detector_context::cones_census::add_since(
        &crate::engine::l5::detector_context::cones_census::SGL_RAW_NANOS,
        _t,
    );
    sorted
}

/// Set-correct path for routines in recursive SCCs. Mirrors
/// `inheritedFactsByBfs`.
///
/// #160 step 7a: this walk feeds the RAW half only (`RawOnly`, `Both`). The
/// derived row of a recursive member is folded from its Cone-SCC's set (the
/// SCC's fact cone) plus its own entries, never from these picks.
///
/// ⟨C1⟩ Note the asymmetry: a SIBLING member's facts come from the key-deduped
/// `direct` map (not its raw Vec), while a downstream SCC contributes its cone
/// entries. ⟨issue 33⟩ With the temp class in [`inherited_fact_key`] the fold
/// is dedup-invariant, so the oracle's fold of these picks and the set fold
/// agree (see `cone_derived`'s module docs). The walk returns its picks, one
/// `(representative, first hop)` per key in emission order; [`emit_picks`]
/// retags and sorts them into the raw output.
///
/// #69 D1.3 use (a): after the walk, the subject's own producer-solve entries
/// (`own`) compete per key with the walk's picks under [`candidate_wins`]
/// (a pick's dist is its hop count plus its cone entry's dist). A sibling's
/// producers are not in `direct`, so they are never emitted in the sibling's
/// frame; the subject reaches them through its own entries.
fn inherited_facts_by_bfs<'g>(
    subject: &str,
    g: &'g TypedEdgeGraph,
    direct: &'g RoutineDirectFacts,
    scc_id_by_routine: &HashMap<String, usize>,
    cones: &'g HashMap<usize, ConeFacts>,
    own: &'g [super::pd_solve::ResolvedEntry<'g>],
) -> Vec<(&'g CapabilityFact, &'g TypedOutEdge)> {
    let my_scc = scc_id_by_routine.get(subject).copied();
    // key -> index into `picked`; `picked` keeps emission order.
    let mut seen: BTreeMap<String, usize> = BTreeMap::new();
    let mut picked: Vec<(&'g CapabilityFact, usize, &'g TypedOutEdge)> = Vec::new();
    let mut visited: BTreeSet<String> = BTreeSet::new();
    visited.insert(subject.to_string());

    struct QI<'g> {
        id: String,
        first_hop: &'g TypedOutEdge,
        hops: usize,
    }
    let mut queue: std::collections::VecDeque<QI<'g>> = std::collections::VecDeque::new();
    let out_edges = |id: &str| -> &'g [TypedOutEdge] {
        g.outgoing.get(id).map(|v| v.as_slice()).unwrap_or(&[])
    };
    for edge in out_edges(subject) {
        if !visited.contains(&edge.to) {
            visited.insert(edge.to.clone());
            queue.push_back(QI {
                id: edge.to.clone(),
                first_hop: edge,
                hops: 1,
            });
        }
    }
    while let Some(item) = queue.pop_front() {
        let id = item.id;
        let first_hop = item.first_hop;
        if scc_id_by_routine.get(&id).copied() == my_scc {
            // sibling member: emit its own direct facts (attributed to firstHop).
            if let Some(byk) = direct.get(&id) {
                for (key, rep) in byk {
                    if !seen.contains_key(key) {
                        seen.insert(key.clone(), picked.len());
                        picked.push((rep, item.hops, first_hop));
                    }
                }
            }
            for edge in out_edges(&id) {
                if !visited.contains(&edge.to) {
                    visited.insert(edge.to.clone());
                    queue.push_back(QI {
                        id: edge.to.clone(),
                        first_hop,
                        hops: item.hops + 1,
                    });
                }
            }
        } else {
            // downstream entry: pull its whole cone, do NOT recurse.
            let their_scc = scc_id_by_routine.get(&id).copied();
            // ⟨C1 Task 4 hardening⟩ This branch is reached only when the `if`
            // above already found `their_scc != my_scc` — i.e. never for the
            // subject's own (root-eligible) SCC. Assert it explicitly, right
            // at the read, so a future change to the branch condition above
            // trips loudly here instead of silently returning an
            // empty/foreign cone for a root SCC that was never built.
            debug_assert_ne!(
                their_scc, my_scc,
                "inherited_facts_by_bfs({subject:?}): downstream branch reached with a \
                 same-SCC hop ({id:?}) — a root SCC's cone is never built, so pulling it \
                 here would silently drop inherited facts instead of panicking",
            );
            let ycone = their_scc.and_then(|yj| cones.get(&yj));
            if let Some(ycone) = ycone {
                for (key, entry) in ycone {
                    // No frame here (recursive subject): a guard is dropped,
                    // and the fact keeps its one base key.
                    let key = base_key_of(key, entry);
                    // `key` is the cone's `Arc<str>`; `seen` stays keyed by `String`
                    // because the sibling branch above feeds it `direct`'s own
                    // `String` keys. Since #160 step 7a this walk runs only for the
                    // raw half (RawOnly, Both) and the test oracle, never under
                    // `analyze`'s DerivedOnly, so the one owned copy per newly-seen
                    // key is not on any hot path.
                    if !seen.contains_key(&**key) {
                        seen.insert(key.to_string(), picked.len());
                        picked.push((&entry.rep, item.hops + entry.dist, first_hop));
                    }
                }
            }
        }
    }
    for e in own {
        let key = inherited_fact_key(&e.rep);
        match seen.get(&key) {
            Some(&i) => {
                let (_, d, hop) = picked[i];
                if candidate_wins(e.dist, e.first_hop, Some((d, hop))) {
                    picked[i] = (&e.rep, e.dist, e.first_hop);
                }
            }
            None => {
                seen.insert(key, picked.len());
                picked.push((&e.rep, e.dist, e.first_hop));
            }
        }
    }
    picked
        .into_iter()
        .map(|(rep, _, first_hop)| (rep, first_hop))
        .collect()
}

/// A recursive member's RAW inherited output from its BFS picks (retagged,
/// sorted). The derived row never reads the picks in production: it folds the
/// member's Cone-SCC set (#160 step 7a). Empty unless the mode wants raw.
fn emit_picks(
    subject: &str,
    picks: Vec<(&CapabilityFact, &TypedOutEdge)>,
    mode: ConeOutput,
) -> Vec<CapabilityFact> {
    if !mode.wants_raw() {
        return Vec::new();
    }
    sort_inherited(
        picks
            .into_iter()
            .map(|(rep, first_hop)| retag(rep, subject, first_hop))
            .collect(),
    )
}

/// Distinct successor SCCs per SCC (cross-SCC edges only). Mirrors
/// `buildSuccSccs`.
fn build_succ_sccs(
    g: &TypedEdgeGraph,
    sccs: &[Scc],
    scc_id_by_routine: &HashMap<String, usize>,
) -> Vec<BTreeSet<usize>> {
    let mut succ: Vec<BTreeSet<usize>> = sccs.iter().map(|_| BTreeSet::new()).collect();
    let empty: Vec<TypedOutEdge> = Vec::new();
    for (i, scc) in sccs.iter().enumerate() {
        for m in &scc.members {
            for e in g.outgoing.get(m).unwrap_or(&empty) {
                if let Some(yj) = scc_id_by_routine.get(&e.to)
                    && *yj != i
                {
                    succ[i].insert(*yj);
                }
            }
        }
    }
    succ
}

/// Build one SCC's fact cone (members at dist 0 + successor cones at dist+1).
/// Mirrors `factConeForScc`.
fn fact_cone_for_scc(
    members: &[String],
    succ_ids: &BTreeSet<usize>,
    fact_cones: &HashMap<usize, ConeFacts>,
    direct: &RoutineDirectFacts,
    anchor: Option<GuardAnchor<'_>>,
) -> ConeFacts {
    let mut cone: ConeFacts = BTreeMap::new();
    let frame = anchor.as_ref().and_then(|a| a.frame);
    for m in members {
        if let Some(byk) = direct.get(m) {
            // The ordinary raw facts and their keys, when this frame has
            // guarded sites: a key's guards are those of the raw facts deduped
            // under it. Producers are not in `direct`, so they are skipped here.
            let raw: Option<Vec<(&CapabilityFact, String)>> = match (&anchor, frame) {
                (Some(a), Some(fg)) if !fg.by_site.is_empty() => Some(
                    a.direct_raw
                        .iter()
                        .filter(|f| !is_producer(f))
                        .map(|f| (f, inherited_fact_key(f)))
                        .collect(),
                ),
                _ => None,
            };
            for (key, f) in byk {
                // One entry per distinct requirement list among the raw facts
                // under this key; any unguarded raw fact makes the key
                // unconditional (it dominates every guarded copy).
                let mut req_sets: Vec<Vec<super::param_guard::Req>> = Vec::new();
                if let (Some(fg), Some(raw)) = (frame, &raw) {
                    for (g, _) in raw.iter().filter(|(_, k)| k == key) {
                        match site_reqs(fg, g) {
                            Some(r) => {
                                if !req_sets.contains(r) {
                                    req_sets.push(r.clone());
                                }
                            }
                            None => {
                                req_sets.clear();
                                break;
                            }
                        }
                    }
                }
                if req_sets.is_empty() {
                    req_sets.push(Vec::new());
                }
                // The ONE place a cone key is minted: `direct`'s keys are
                // `String`s owned by the per-routine dedup map, so a dist-0 entry
                // allocates its `Arc<str>` here. That is once per direct fact
                // (121,387 per 8020 run), not once per merge — every merge below
                // and in every predecessor cone then shares this allocation.
                let base: Arc<str> = Arc::from(key.as_str());
                let rep = Arc::new(f.clone());
                let rep_key_s: Arc<str> = Arc::from(rep_key(f));
                for reqs in req_sets {
                    let (k, guard) = with_reqs(Arc::clone(&base), reqs);
                    merge_cone(
                        &mut cone,
                        &k,
                        ConeFactEntry {
                            rep: Arc::clone(&rep),
                            rep_key: Arc::clone(&rep_key_s),
                            dist: 0,
                            guard,
                        },
                    );
                }
            }
        }
    }
    // A frame-bound successor entry crosses into this cone per EDGE, through
    // `substitute_entry`; the per-SCC merge below then skips it. So does EVERY
    // entry behind an edge the member makes under a guard (it gains the
    // edge's requirements).
    let mut guarded_only: BTreeSet<usize> = BTreeSet::new();
    if let Some(a) = &anchor {
        let mut unguarded_to: BTreeSet<usize> = BTreeSet::new();
        for edge in a.out_edges {
            let Some(&yj) = a.scc_id_by_routine.get(&edge.to) else {
                continue;
            };
            let Some(yc) = fact_cones.get(&yj) else {
                continue;
            };
            let edge_reqs: &[super::param_guard::Req] = match (frame, edge.site()) {
                (Some(fg), Some(cs)) => fg.by_site.get(cs).map_or(&[], Vec::as_slice),
                _ => &[],
            };
            if edge_reqs.is_empty() {
                unguarded_to.insert(yj);
            } else {
                guarded_only.insert(yj);
            }
            for (key, entry) in yc
                .iter()
                .filter(|(_, e)| !edge_reqs.is_empty() || frame_bound(e))
            {
                if let Some((k, mut e)) = substitute_entry(
                    key,
                    entry,
                    edge,
                    a.caller,
                    a.routines_by_id.get(edge.to.as_str()).copied(),
                    frame,
                    edge_reqs,
                ) {
                    e.dist += 1;
                    merge_cone(&mut cone, &k, e);
                }
            }
        }
        guarded_only.retain(|y| !unguarded_to.contains(y));
    }
    for y in succ_ids {
        if guarded_only.contains(y) {
            continue; // every edge to it is guarded: carried per edge above
        }
        // ⟨C1 Task 4 hardening⟩ `y` is a successor of the SCC being built, and
        // `compose_inherited_cones` processes SCCs in an order where every
        // successor has already run (Tarjan emits callees before callers) —
        // successors are provably non-root, so their cone was built and has
        // not yet been refcount-freed. A miss here is always a bug (the
        // refcount-free fired before this, its only intended predecessor,
        // read the cone), not a legitimate "not built yet" case.
        debug_assert!(
            fact_cones.contains_key(y),
            "fact_cone_for_scc: successor SCC {y} has no fact cone — successors are \
             provably non-root, so this cone should have been built and not yet freed"
        );
        if let Some(yc) = fact_cones.get(y) {
            for (key, entry) in yc {
                if anchor.is_some() && frame_bound(entry) {
                    continue; // carried per edge above
                }
                merge_cone(
                    &mut cone,
                    base_key_of(key, entry),
                    ConeFactEntry {
                        rep: Arc::clone(&entry.rep),
                        rep_key: Arc::clone(&entry.rep_key),
                        dist: entry.dist + 1,
                        // Without an anchor (recursive / multi-member SCC) a
                        // successor's guard means nothing here.
                        guard: None,
                    },
                );
            }
        }
    }
    cone
}

/// The requirements under which raw direct fact `f`'s site runs, by its
/// witness operation or call site. `None`: unguarded.
pub(crate) fn site_reqs<'f>(
    fg: &'f super::param_guard::FrameGuards,
    f: &CapabilityFact,
) -> Option<&'f Vec<super::param_guard::Req>> {
    [&f.witness_operation_id, &f.witness_callsite_id]
        .into_iter()
        .flatten()
        .find_map(|id| fg.by_site.get(id))
}

/// Every routine's guard frame ([`super::param_guard`]), for the routines that
/// have a usable guard parameter, over the typed graph's call edges.
fn guard_frames(
    g: &TypedEdgeGraph,
    routines: &[ModelRoutine],
    routines_by_id: &HashMap<&str, &ModelRoutine>,
) -> HashMap<String, super::param_guard::FrameGuards> {
    let mut out = HashMap::new();
    for r in routines {
        let edges: &[TypedOutEdge] = g.outgoing.get(&r.id).map_or(&[], Vec::as_slice);
        let callees_at = |cs_id: &str| -> Vec<&str> {
            edges
                .iter()
                .filter(|e| e.callsite.as_deref() == Some(cs_id))
                .map(|e| e.to.as_str())
                .collect()
        };
        if let Some(f) = super::param_guard::frame_guards_over(r, callees_at, routines_by_id) {
            out.insert(r.id.clone(), f);
        }
    }
    out
}

/// What [`fact_cone_for_scc`] needs to keep guarded facts frame-bound: present
/// only for a NON-RECURSIVE SINGLETON SCC, whose cone has exactly one frame
/// (its member's parameters).
struct GuardAnchor<'a> {
    /// The member's RAW (un-deduped) direct facts.
    direct_raw: &'a [CapabilityFact],
    /// The member's out-edges.
    out_edges: &'a [TypedOutEdge],
    scc_id_by_routine: &'a HashMap<String, usize>,
    /// The member's L3 routine (its call sites carry the argument bindings).
    caller: Option<&'a ModelRoutine>,
    /// The member's guard frame, when it has a usable guard parameter.
    frame: Option<&'a super::param_guard::FrameGuards>,
    /// Every routine, for an edge's callee (an event subscriber's parameters).
    routines_by_id: &'a HashMap<&'a str, &'a ModelRoutine>,
}

/// Build one SCC's coverage cone (includes self). Mirrors `coverageConeForScc`.
fn coverage_cone_for_scc(
    members: &[String],
    succ_ids: &BTreeSet<usize>,
    cov_cones: &HashMap<usize, CoverageCone>,
    cov: &RoutineDirectCoverage,
    unresolved_sources: &BTreeSet<String>,
) -> CoverageCone {
    let mut complete = true;
    let mut reason_set: BTreeSet<String> = BTreeSet::new();
    let mut unknown_set: BTreeSet<String> = BTreeSet::new();
    for m in members {
        if unresolved_sources.contains(m) {
            complete = false;
            reason_set.insert("object-run-unresolved".to_string());
        }
        if let Some(c) = cov.get(m)
            && (c.direct_status == "partial" || c.direct_status == "unknown")
        {
            complete = false;
            unknown_set.insert(m.clone());
            for r in &c.reasons {
                reason_set.insert(r.clone());
            }
        }
    }
    for y in succ_ids {
        // ⟨C1 Task 4 hardening⟩ Same successor-non-root invariant as
        // `fact_cone_for_scc` above — a miss here is always a bug, never a
        // legitimate "not built yet".
        debug_assert!(
            cov_cones.contains_key(y),
            "coverage_cone_for_scc: successor SCC {y} has no coverage cone — successors \
             are provably non-root, so this cone should have been built and not yet freed"
        );
        if let Some(yc) = cov_cones.get(y) {
            if !yc.complete {
                complete = false;
            }
            for r in &yc.reasons {
                reason_set.insert(r.clone());
            }
            for t in &yc.unknown_targets {
                unknown_set.insert(t.clone());
            }
        }
    }
    CoverageCone {
        complete,
        reasons: reason_set.into_iter().collect(),
        unknown_targets: unknown_set.into_iter().collect(),
    }
}

/// Per-call counts of the cone's SCC shapes and of the recursive branch's
/// work (#160 step 7a §4.5). Plain integers, computed on every call; the
/// census (`cones_census`, `cone_scc:` line) sums them over the process.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct ComposeStats {
    pub(crate) recursive_sccs: u64,
    /// Every member Tarjan placed in a recursive SCC, emitted or not.
    pub(crate) recursive_members_all: u64,
    /// The recursive members in `routine_ids` (the population `BFS_CALLS` counts).
    pub(crate) recursive_members_emitted: u64,
    pub(crate) recursive_roots: u64,
    pub(crate) self_loops: u64,
    pub(crate) max_recursive_members: u64,
    /// Recursive SCCs by member count: 1, 2, 3-9, 10-99, >= 100.
    pub(crate) size_hist: [u64; 5],
    /// Calls of [`inherited_facts_by_bfs`] (the raw half only; 0 under
    /// `DerivedOnly`). Their time is the census `walk_total`;
    /// `bfs_walk` also holds the set builds and borrows.
    pub(crate) member_walks: u64,
    /// Own (producer-solve) entries of the emitted recursive members.
    pub(crate) own_entries_recursive: u64,
    /// Emitted recursive members with at least one own entry (§4.1 point 6).
    pub(crate) members_with_own_entry: u64,
    /// Recursive SCCs with at least one successor SCC.
    pub(crate) with_successor: u64,
    /// Recursive SCCs with a successor whose fact cone holds a frame-bound
    /// entry, which the set takes with its base key (§4.1 point 4).
    pub(crate) with_guarded_successor: u64,
    /// Sets taken from a non-root SCC's fact cone.
    pub(crate) scc_sets_borrowed: u64,
    /// Sets built for a root SCC (its fact cone is never built).
    pub(crate) scc_sets_built: u64,
    /// Σ set length over the emitted members that read a set: the entries the
    /// members' rows cover (each member reads the SCC's set row once).
    pub(crate) scc_set_entries_folded: u64,
    /// #160 step 7b §4.8: memo rows built (one per callee Cone-SCC read).
    pub(crate) closed_memo_builds: u64,
    /// `fold_row` calls on memo rows: one per (singleton, distinct callee SCC).
    pub(crate) closed_memo_reads: u64,
    /// Closed entries folded into memo rows.
    pub(crate) closed_memo_entries: u64,
    /// Folds of a recursive SCC's set: one per recursive SCC with a set.
    pub(crate) scc_set_folds: u64,
    /// Singleton calls that built `best` (the raw half only).
    pub(crate) singleton_scans: u64,
}

/// The per-routine cone result.
struct InheritedConeResult {
    inherited: Vec<CapabilityFact>,
    coverage: CoverageRecord,
}

/// #160 step 7b §4.7: the T7'/T8' oracle's streamed raw fold. Test-only, so
/// production builds carry no parameter and no per-routine branch for it.
/// While [`raw_fold_hook::fold`] runs, every routine `compose_inherited_cones`
/// emits is folded at once from its raw inherited Vec ∪ its raw direct facts
/// into the hook's builder, and its raw Vec is dropped, so no raw Vec of all
/// routines is ever held.
#[cfg(test)]
pub(crate) mod raw_fold_hook {
    use super::{CapabilityFact, ConeDerivedBuilder, ConeDerivedStore};
    use std::cell::RefCell;

    thread_local! {
        static ORACLE: RefCell<Option<ConeDerivedBuilder>> = const { RefCell::new(None) };
    }

    /// Run `compose` (a `RawOnly` composition) with the hook on; return its
    /// result and the oracle store.
    pub(crate) fn fold<T>(compose: impl FnOnce() -> T) -> (T, ConeDerivedStore) {
        ORACLE.with(|o| *o.borrow_mut() = Some(ConeDerivedBuilder::default()));
        let out = compose();
        let oracle = ORACLE
            .with(|o| o.borrow_mut().take())
            .expect("the hook's builder");
        (out, oracle.finish())
    }

    /// At emission: with the hook on, fold `raw ∪ direct` for `id` and keep
    /// an empty Vec; otherwise return `raw` unchanged.
    pub(super) fn offer(
        id: &str,
        raw: Vec<CapabilityFact>,
        direct: Option<&Vec<CapabilityFact>>,
    ) -> Vec<CapabilityFact> {
        ORACLE.with(|o| match o.borrow_mut().as_mut() {
            Some(b) => {
                b.fold_routine(id, raw.iter().chain(direct.into_iter().flatten()));
                Vec::new()
            }
            None => raw,
        })
    }
}

/// Compute capabilityFactsInherited + coverage for EVERY routine via a single
/// fused bottom-up SCC-cone pass. Mirrors `composeInheritedCones`. The engine
/// never throws: on internal inconsistency it still returns whatever it built.
///
/// ⟨C1⟩ `direct_raw` is the RAW, un-deduped per-routine direct-fact map (the
/// same Vec `FullRoutineSummary.capability_facts_direct` holds). It is the SELF
/// half of the derived fold: the reachable sequence scans every direct fact, so
/// the fold must too — unlike the inherited half, which folds key-deduped
/// representatives. `mode` decides which of the two outputs is produced.
#[allow(clippy::too_many_arguments)]
fn compose_inherited_cones(
    g: &TypedEdgeGraph,
    scc: &SccResult,
    direct: &RoutineDirectFacts,
    direct_raw: &HashMap<String, Vec<CapabilityFact>>,
    cov: &RoutineDirectCoverage,
    routine_ids: &BTreeSet<String>,
    routines_by_id: &HashMap<&str, &ModelRoutine>,
    frames: &HashMap<String, super::param_guard::FrameGuards>,
    mode: ConeOutput,
) -> (
    HashMap<String, InheritedConeResult>,
    ConeDerivedStore,
    super::pd_solve::ConeArena,
    ComposeStats,
) {
    let mut stats = ComposeStats::default();
    // #160 §4.1.1: the per-call untimed remainder. Entry snapshot only under
    // the census (one OnceLock read otherwise); the function has one exit.
    let rest_entry = crate::engine::l5::detector_context::cones_census::enabled().then(|| {
        (
            std::time::Instant::now(),
            crate::engine::l5::detector_context::cones_census::component_snapshot(),
        )
    });
    let mut out: HashMap<String, InheritedConeResult> = HashMap::new();
    let mut derived = ConeDerivedBuilder::default();

    // #69 D1: the producers (raw direct facts, in node order) leave the shared
    // cone and are resolved per routine by the transitive producer solve.
    // A producer's identity is `(owner, operation_id)`: `g.nodes` repeats a
    // routine id once per declaration (two `#if` arms, one id), and each repeat
    // reads the same `direct_raw` entry, so a repeat must not make a second
    // producer (the solve keys states by producer, so its states would be
    // twins).
    let mut seen: std::collections::HashSet<(&str, &str)> = std::collections::HashSet::new();
    let producers: Vec<super::pd_solve::Producer> = g
        .nodes
        .iter()
        .flat_map(|id| {
            direct_raw
                .get(id)
                .into_iter()
                .flatten()
                .filter(|f| is_producer(f))
                .map(move |f| {
                    let op = f
                        .witness_operation_id
                        .as_deref()
                        .expect("a producer has a witness operation id");
                    (id.as_str(), op, f)
                })
        })
        .filter(|&(id, op, _)| seen.insert((id, op)))
        .map(|(id, op, f)| super::pd_solve::Producer {
            fact: Arc::new(f.clone()),
            owner: Arc::from(id),
            operation_id: op.to_string(),
        })
        .collect();
    let _t_pd = crate::engine::l5::detector_context::cones_census::start();
    let (arena, resolved) = super::pd_solve::solve(producers, g, routines_by_id, frames);
    crate::engine::l5::detector_context::cones_census::add_since(
        &crate::engine::l5::detector_context::cones_census::PD_SOLVE_NANOS,
        _t_pd,
    );
    crate::engine::l5::detector_context::cones_census::add_gated(
        &crate::engine::l5::detector_context::cones_census::PD_SOLVE_CALLS,
        1,
    );
    let own_entries = |m: &str| resolved.get(m).map_or(&[][..], Vec::as_slice);

    let succ_sccs = build_succ_sccs(g, &scc.sccs, &scc.scc_id_by_routine);
    let mut remaining_uses: Vec<usize> = scc.sccs.iter().map(|_| 0usize).collect();
    for set in &succ_sccs {
        for y in set {
            remaining_uses[*y] += 1;
        }
    }

    let mut fact_cones: HashMap<usize, ConeFacts> = HashMap::new();
    let mut cov_cones: HashMap<usize, CoverageCone> = HashMap::new();
    // #160 step 7b §4.4: a memo row lives exactly as long as its cone.
    let mut memo: ClosedRowMemo = HashMap::new();

    let empty_succ: BTreeSet<usize> = BTreeSet::new();
    for i in 0..scc.sccs.len() {
        let scc_entry = &scc.sccs[i];
        let succ_ids = succ_sccs.get(i).unwrap_or(&empty_succ);

        // ⟨C1 Task 4⟩ An SCC's cone has two readers. (1) Its PREDECESSORS:
        // `fact_cone_for_scc` reads `fact_cones[y]` only for `y ∈ succ_sccs[j]`,
        // and both per-routine walks (`inherited_facts_for_singleton` /
        // `inherited_facts_by_bfs`) read a cone only for a routine whose SCC
        // differs from the subject's (`build_succ_sccs` excludes self, and the
        // BFS treats a same-SCC hop as a sibling and expands it instead of
        // pulling a cone; a non-recursive singleton has no self-edge by
        // `tarjan_scc`'s `recursive` rule). Since #160 step 7b this includes,
        // in every mode that wants derived rows, the singleton's memo-row build
        // (`memo[y]` = F(closed entries of `fact_cones[y]`)), whose row is freed
        // together with the cone below. (2) Since step 7a, a non-root RECURSIVE
        // SCC's own members, which fold `fact_cones[i]` as their Cone-SCC set
        // during `i`'s member loop. Both readers are done before `i`'s cone is
        // freed: the free below runs after the member loop, and only once every
        // predecessor has run.
        //
        // `remaining_uses[i]` is the number of SCCs holding `i` as a successor,
        // decremented once per predecessor as that predecessor finishes. Tarjan
        // emits callees before callers, so none of `i`'s predecessors has run
        // yet when `i` is reached: `remaining_uses[i] == 0` here means `i` is a
        // call-graph ROOT (nothing calls it) and no predecessor will ever read
        // its cone from the cached `fact_cones` map, so caching it is pure
        // waste. (A recursive root's set is built locally instead, for its own
        // members, and dropped after them: #160 step 7a §4.2.) On the 8020
        // corpus caching was 17,864 root SCCs holding 2,224,901 fact entries (~1,599 MB) alive for the
        // whole walk, because the refcount-free below fires only for a
        // successor and a root is nobody's successor. (Even if the emission
        // order were ever NOT reverse-topological, a zero here would still mean
        // every predecessor has already run and already read whatever this map
        // held, so skipping stays sound.)
        let root_scc = remaining_uses[i] == 0;
        if !root_scc {
            let _t_fc = crate::engine::l5::detector_context::cones_census::start();
            let anchor = match scc_entry.members.as_slice() {
                [m] if !scc_entry.recursive => Some(GuardAnchor {
                    direct_raw: direct_raw.get(m).map(|v| v.as_slice()).unwrap_or(&[]),
                    out_edges: g.outgoing.get(m).map(|v| v.as_slice()).unwrap_or(&[]),
                    scc_id_by_routine: &scc.scc_id_by_routine,
                    caller: routines_by_id.get(m.as_str()).copied(),
                    frame: frames.get(m),
                    routines_by_id,
                }),
                _ => None,
            };
            let fcone =
                fact_cone_for_scc(&scc_entry.members, succ_ids, &fact_cones, direct, anchor);
            fact_cones.insert(i, fcone);
            crate::engine::l5::detector_context::cones_census::add_since(
                &crate::engine::l5::detector_context::cones_census::FACTCONE_NANOS,
                _t_fc,
            );
            crate::engine::l5::detector_context::cones_census::add(
                &crate::engine::l5::detector_context::cones_census::FACTCONE_CALLS,
                1,
            );
        }
        // The coverage cone is ALWAYS computed — `ccone` is what this SCC's own
        // members' `CoverageRecord`s are built from below — but a root's copy is
        // never published for the same reason.
        let _t_cov = crate::engine::l5::detector_context::cones_census::start();
        let ccone = coverage_cone_for_scc(
            &scc_entry.members,
            succ_ids,
            &cov_cones,
            cov,
            &g.unresolved_sources,
        );
        crate::engine::l5::detector_context::cones_census::add_since(
            &crate::engine::l5::detector_context::cones_census::COVERAGE_NANOS,
            _t_cov,
        );
        if !root_scc {
            cov_cones.insert(i, ccone.clone());
        }

        crate::engine::l5::detector_context::cones_census::max(
            &crate::engine::l5::detector_context::cones_census::MAX_SCC_MEMBERS,
            scc_entry.members.len() as u64,
        );
        let recursive = scc_entry.recursive;
        // The shape counts; a singleton SCC pays no `routine_ids` lookup.
        let emitted = if recursive {
            let n = scc_entry.members.len() as u64;
            let emitted = scc_entry
                .members
                .iter()
                .filter(|m| routine_ids.contains(*m))
                .count() as u64;
            stats.recursive_sccs += 1;
            stats.recursive_members_all += n;
            stats.recursive_members_emitted += emitted;
            stats.recursive_roots += u64::from(root_scc);
            stats.self_loops += u64::from(n == 1);
            stats.max_recursive_members = stats.max_recursive_members.max(n);
            stats.size_hist[match n {
                1 => 0,
                2 => 1,
                3..=9 => 2,
                10..=99 => 3,
                _ => 4,
            }] += 1;
            stats.with_successor += u64::from(!succ_ids.is_empty());
            // A scan of every successor cone: only where it is read (the census,
            // and T7'/T8' in tests), not on every production compose.
            if cfg!(test) || crate::engine::l5::detector_context::cones_census::enabled() {
                stats.with_guarded_successor += u64::from(succ_ids.iter().any(|y| {
                    fact_cones
                        .get(y)
                        .is_some_and(|c| c.values().any(frame_bound))
                }));
            }
            emitted
        } else {
            0
        };
        // §4.2 step 1: ONE set per recursive SCC with an emitted member, read
        // by every member's derived fold. A non-root borrows its fact cone
        // (built above, freed after the member loop); a root, whose cone is
        // never built, gets the same build here, dropped after the loop.
        let use_set = recursive && emitted > 0 && mode.wants_derived();
        let _t_set = use_set
            .then(crate::engine::l5::detector_context::cones_census::start)
            .flatten();
        let root_set = (use_set && root_scc)
            .then(|| fact_cone_for_scc(&scc_entry.members, succ_ids, &fact_cones, direct, None));
        let scc_set: Option<&ConeFacts> = if !use_set {
            None
        } else if let Some(set) = &root_set {
            stats.scc_sets_built += 1;
            Some(set)
        } else {
            stats.scc_sets_borrowed += 1;
            Some(&fact_cones[&i])
        };
        crate::engine::l5::detector_context::cones_census::add_since(
            &crate::engine::l5::detector_context::cones_census::BFS_WALK_NANOS,
            _t_set,
        );
        // 7b §4.5: F(the SCC set), folded ONCE per SCC, after the set window
        // (timed as fold only); each member `fold_row`s it.
        let set_row = scc_set.map(|set| {
            let _t = crate::engine::l5::detector_context::cones_census::start();
            let row = derived.fold_set(set.values().map(|e| &*e.rep));
            stats.scc_set_folds += 1;
            crate::engine::l5::detector_context::cones_census::add_since(
                &crate::engine::l5::detector_context::cones_census::BFS_FOLD_NANOS,
                _t,
            );
            row
        });
        for m in &scc_entry.members {
            if !routine_ids.contains(m) {
                continue;
            }
            if mode.wants_derived() {
                let _t_der0 = crate::engine::l5::detector_context::cones_census::start();
                derived.begin_routine();
                crate::engine::l5::detector_context::cones_census::add_since(
                    &crate::engine::l5::detector_context::cones_census::DERIVED_NANOS,
                    _t_der0,
                );
            }
            let _t_walk = crate::engine::l5::detector_context::cones_census::start();
            let inherited = if recursive {
                stats.own_entries_recursive += own_entries(m).len() as u64;
                stats.members_with_own_entry += u64::from(!own_entries(m).is_empty());
                // The walk runs only for the raw half: never under `DerivedOnly`.
                let picks = if mode.wants_raw() {
                    stats.member_walks += 1;
                    let picks = inherited_facts_by_bfs(
                        m,
                        g,
                        direct,
                        &scc.scc_id_by_routine,
                        &fact_cones,
                        own_entries(m),
                    );
                    // One reading feeds the component and the `walk_total` report.
                    if let Some(t) = _t_walk {
                        let ns = t.elapsed().as_nanos() as u64;
                        crate::engine::l5::detector_context::cones_census::add(
                            &crate::engine::l5::detector_context::cones_census::BFS_WALK_NANOS,
                            ns,
                        );
                        crate::engine::l5::detector_context::cones_census::add(
                            &crate::engine::l5::detector_context::cones_census::WALK_TOTAL_NANOS,
                            ns,
                        );
                    }
                    crate::engine::l5::detector_context::cones_census::add_gated(
                        &crate::engine::l5::detector_context::cones_census::BFS_PICKS,
                        picks.len() as u64,
                    );
                    Some(picks)
                } else {
                    None
                };
                let _t_fold = crate::engine::l5::detector_context::cones_census::start();
                // §4.2 step 2: the SCC set's row, then the member's own entries.
                if let (Some(set), Some(row)) = (scc_set, &set_row) {
                    derived.fold_row(row);
                    for entry in own_entries(m) {
                        derived.fold_fact(&entry.rep);
                    }
                    stats.scc_set_entries_folded += set.len() as u64;
                }
                let out = match picks {
                    Some(picks) => emit_picks(m, picks, mode),
                    None => Vec::new(),
                };
                crate::engine::l5::detector_context::cones_census::add_since(
                    &crate::engine::l5::detector_context::cones_census::BFS_FOLD_NANOS,
                    _t_fold,
                );
                out
            } else {
                inherited_facts_for_singleton(
                    m,
                    g,
                    &scc.scc_id_by_routine,
                    &fact_cones,
                    routines_by_id.get(m.as_str()).copied(),
                    frames.get(m),
                    routines_by_id,
                    own_entries(m),
                    mode,
                    &mut derived,
                    &mut memo,
                    &mut stats,
                )
            };
            if recursive {
                crate::engine::l5::detector_context::cones_census::add(
                    &crate::engine::l5::detector_context::cones_census::BFS_CALLS,
                    1,
                );
            } else {
                crate::engine::l5::detector_context::cones_census::add_since(
                    &crate::engine::l5::detector_context::cones_census::SINGLETON_NANOS,
                    _t_walk,
                );
                crate::engine::l5::detector_context::cones_census::add(
                    &crate::engine::l5::detector_context::cones_census::SINGLETON_CALLS,
                    1,
                );
            }
            if mode.wants_derived() {
                // The SELF half — the routine's RAW direct facts, one fold each
                // (they are stored un-deduped and the reachable sequence scans
                // every one of them).
                let _t_der1 = crate::engine::l5::detector_context::cones_census::start();
                if let Some(own) = direct_raw.get(m) {
                    for f in own {
                        derived.fold_fact(f);
                    }
                }
                derived.finish_routine(m);
                crate::engine::l5::detector_context::cones_census::add_since(
                    &crate::engine::l5::detector_context::cones_census::DERIVED_NANOS,
                    _t_der1,
                );
            }
            #[cfg(test)]
            let inherited = raw_fold_hook::offer(m, inherited, direct_raw.get(m));
            let _t_emit = crate::engine::l5::detector_context::cones_census::start();
            let d_status = cov
                .get(m)
                .map(|c| c.direct_status.clone())
                .unwrap_or_else(|| "unknown".to_string());
            out.insert(
                m.clone(),
                InheritedConeResult {
                    inherited,
                    coverage: CoverageRecord {
                        subject: m.clone(),
                        direct_status: d_status,
                        inherited_status: if ccone.complete {
                            "complete".to_string()
                        } else {
                            "partial".to_string()
                        },
                        reasons: ccone.reasons.clone(),
                        unknown_targets: ccone.unknown_targets.clone(),
                    },
                },
            );
            crate::engine::l5::detector_context::cones_census::add_since(
                &crate::engine::l5::detector_context::cones_census::EMIT_NANOS,
                _t_emit,
            );
        }
        // 7b §4.5: the set row's destruction is recursive fold work, charged to
        // `BFS_FOLD_NANOS` (the B1 timer `bfs_fold`), not left untimed.
        if let Some(row) = set_row {
            let _t = crate::engine::l5::detector_context::cones_census::start();
            drop(row);
            crate::engine::l5::detector_context::cones_census::add_since(
                &crate::engine::l5::detector_context::cones_census::BFS_FOLD_NANOS,
                _t,
            );
        }

        // refcount-free downstream cones whose last predecessor (this SCC) is done.
        for y in succ_ids {
            if remaining_uses[*y] > 0 {
                remaining_uses[*y] -= 1;
            }
            if remaining_uses[*y] == 0 {
                fact_cones.remove(y);
                // 7b §4.4: a memo row's destruction is singleton work, charged
                // to `SINGLETON_NANOS` (the B1 timer `singleton`) and to its
                // `SGL_MEMO_NANOS` sub-timer, from one reading.
                let _t = crate::engine::l5::detector_context::cones_census::start();
                let dropped = memo.remove(y).is_some(); // the row drops at this `;`
                if dropped && let Some(t) = _t {
                    let ns = t.elapsed().as_nanos() as u64;
                    crate::engine::l5::detector_context::cones_census::add(
                        &crate::engine::l5::detector_context::cones_census::SINGLETON_NANOS,
                        ns,
                    );
                    crate::engine::l5::detector_context::cones_census::add(
                        &crate::engine::l5::detector_context::cones_census::SGL_MEMO_NANOS,
                        ns,
                    );
                }
                cov_cones.remove(y);
            }
        }
    }

    // ⟨C1 Task 4⟩ Both maps MUST be empty here. Every non-root SCC's cone is
    // refcount-freed by its last predecessor (each of its predecessors runs
    // exactly once and decrements exactly once, so the counter always reaches
    // 0), and a root's cone is never inserted at all (see `root_scc` above). A
    // non-empty map means a cone escaped both mechanisms — the 1,599 MB leak
    // this task closed, returning.
    debug_assert!(
        fact_cones.is_empty() && cov_cones.is_empty() && memo.is_empty(),
        "cone leak: {} fact cone(s) / {} coverage cone(s) / {} closed-set memo row(s) \
         survived the SCC walk",
        fact_cones.len(),
        cov_cones.len(),
        memo.len()
    );

    // ⟨C1 census⟩ `ALSEM_RETAINED_CENSUS=1` — report what's LEFT in `fact_cones`/
    // `cov_cones` at function exit. BEFORE Task 4 this measured the root-SCC
    // residual — an SCC with no caller (a true call-graph root: an entry point
    // / public API nothing else in the workspace calls) never had its refcount
    // driven to zero by the loop above, so its cone survived until these two
    // locals dropped at this function's return (17,864 SCCs / 1,598.87 MB on
    // the 8020 corpus). Task 4 stopped building a root's cone in the first
    // place, so this census now reports ZERO — kept as the cheap standing
    // check that it stays that way (the release-profile counterpart of the
    // `debug_assert!` above). It was never a peak sample either way: mid-walk,
    // before earlier SCCs were refcount-freed, the maps hold more.
    if cone_census::enabled() {
        let mut fc_entries: u64 = 0;
        let mut fc_bytes: u64 = 0;
        for cone in fact_cones.values() {
            for (key, entry) in cone {
                fc_entries += 1;
                fc_bytes += key.len() as u64
                    + std::mem::size_of::<ConeFactEntry>() as u64
                    + cone_census::capability_fact_heap_bytes(&entry.rep);
            }
        }
        let mut cc_reasons: u64 = 0;
        let mut cc_reasons_bytes: u64 = 0;
        let mut cc_unknown: u64 = 0;
        let mut cc_unknown_bytes: u64 = 0;
        for cone in cov_cones.values() {
            cc_reasons += cone.reasons.len() as u64;
            cc_reasons_bytes += cone.reasons.iter().map(|s| s.len() as u64).sum::<u64>();
            cc_unknown += cone.unknown_targets.len() as u64;
            cc_unknown_bytes += cone
                .unknown_targets
                .iter()
                .map(|s| s.len() as u64)
                .sum::<u64>();
        }
        eprintln!("[retained-census] --------------------------------------------------");
        eprintln!(
            "[retained-census] section: fact_cones/cov_cones residual at compose_inherited_cones \
             exit (root SCCs only, never refcount-freed — NOT a peak sample, NOT part of any \
             grand_total_bytes)"
        );
        eprintln!(
            "[retained-census]   fact_cones_residual_sccs={} fact_cones_residual_entries={fc_entries} fact_cones_residual_bytes={fc_bytes} ({:.2} MB)",
            fact_cones.len(),
            fc_bytes as f64 / 1_048_576.0
        );
        eprintln!(
            "[retained-census]   cov_cones_residual_sccs={} cov_cones_residual_reasons={cc_reasons} reasons_bytes={cc_reasons_bytes} cov_cones_residual_unknown_targets={cc_unknown} unknown_targets_bytes={cc_unknown_bytes} ({:.2} MB total)",
            cov_cones.len(),
            (cc_reasons_bytes + cc_unknown_bytes) as f64 / 1_048_576.0
        );
    }

    crate::engine::l5::detector_context::cones_census::add_compose_stats(&stats);
    if let Some((t0, before)) = rest_entry {
        let after = crate::engine::l5::detector_context::cones_census::component_snapshot();
        let comp: i64 = after
            .iter()
            .zip(before.iter())
            .map(|(a, b)| *a as i64 - *b as i64)
            .sum();
        crate::engine::l5::detector_context::cones_census::COMPOSE_REST_NANOS.fetch_add(
            t0.elapsed().as_nanos() as i64 - comp,
            std::sync::atomic::Ordering::Relaxed,
        );
    }
    (out, derived.finish(), arena, stats)
}

// ===========================================================================
// R3b WRAP SEAM — a public cone entry the Salsa `inherited_facts`/`coverage`
// queries call. WRAPS the full cone (typed-edge graph build + Tarjan SCC +
// `compose_inherited_cones`) over the typed edges (`&[TypedEdge]`, the Cone
// payload; #160 step 4) + per-routine direct facts +
// direct coverage. Byte-identical to the cone path inside `project_r3a3` /
// `project_r3a5_cross_app` (it IS that path, factored out). No re-port.
// ===========================================================================

/// One routine's cone result in PUBLIC (internal-id) form. Field-for-field the
/// private `InheritedConeResult`.
#[derive(Debug, Clone, PartialEq)]
pub struct ConeResultPub {
    pub inherited: Vec<CapabilityFact>,
    pub coverage: CoverageRecord,
}

/// What one cone composition produced: the per-routine RAW cone results and the
/// compact derived substrate. Which halves are populated is decided by the
/// [`ConeOutput`] mode — see its variants.
pub struct ConeOutcome {
    /// Per-routine inherited facts + coverage. Under
    /// [`ConeOutput::DerivedOnly`] every `inherited` Vec is empty (never built);
    /// `coverage` is always populated (it is not the memory problem).
    pub cones: HashMap<String, ConeResultPub>,
    /// The compact derived substrate. Empty under [`ConeOutput::RawOnly`].
    pub derived: ConeDerivedStore,
    /// The transitive producer solve (#69 D1), in cone (internal) ids.
    pub pd_solution: super::pd_solve::ConeArena,
    /// This call's SCC-shape and recursive-branch counts (#160 step 7a), for
    /// tests; production reads them through the census.
    #[cfg(test)]
    pub(crate) stats: ComposeStats,
}

/// Compute every routine's `capabilityFactsInherited` + `coverage` over the given
/// typed edges (incl. any injected intra-app dep edges) + the per-routine direct
/// facts + direct coverage. `typed` is the Cone payload (`Transfer::Cone`,
/// [`crate::engine::l4::exec_relation::ExecRelation::cone`]): the cone reads
/// nothing else of the graph, so it cannot read the Primary payload. The `nodes` list is
/// the routine universe; `direct_in` maps routineId → its direct facts (ordered),
/// `coverage_in` maps routineId → `(direct_status, reasons)`.
///
/// This is the EXACT cone substrate `project_r3a3` / `project_r3a5_cross_app`
/// build inline; factored out so the R3b Salsa layer wraps it without re-porting.
///
/// ⟨C1⟩ `mode` selects the output(s). `direct_in` doubles as the derived fold's
/// SELF half — it is the raw, un-deduped direct-fact Vec every caller also stores
/// on `FullRoutineSummary.capability_facts_direct` (verified at all three call
/// sites), which is exactly the self half of the reachable sequence.
pub fn compose_cone_over_graph(
    typed: &[TypedEdge],
    nodes: &[String],
    direct_in: &HashMap<String, Vec<CapabilityFact>>,
    coverage_in: &HashMap<String, (String, Vec<String>)>,
    routines: &[ModelRoutine],
    mode: ConeOutput,
) -> ConeOutcome {
    crate::engine::perf_trace::counter_delta("build.cone", 1);
    let _t_graph = crate::engine::l5::detector_context::cones_census::start();
    let g = build_typed_edge_graph(typed, nodes);
    let mut adjacency: HashMap<String, Vec<String>> = HashMap::new();
    for (from, list) in &g.outgoing {
        adjacency.insert(from.clone(), list.iter().map(|e| e.to.clone()).collect());
    }
    crate::engine::l5::detector_context::cones_census::add_since(
        &crate::engine::l5::detector_context::cones_census::WALK_GRAPH_NANOS,
        _t_graph,
    );
    let _t_scc = crate::engine::l5::detector_context::cones_census::start();
    crate::engine::perf_trace::counter_delta("build.graph_scc", 1);
    let scc = tarjan_scc(&SccInputGraph {
        nodes: &g.nodes,
        edges_by_from: &adjacency,
    });
    // ⟨C1 Task 4⟩ `adjacency` is a whole second copy of every edge target id
    // (`g.outgoing` still holds the originals) and is dead the moment Tarjan
    // returns. Rust would otherwise hold it for the entire cone walk below.
    drop(adjacency);

    // Per-routine dedup-keyed direct facts (canonical rep per key) + direct
    // coverage + the routine-id set — mirrors the assembly in project_r3a3.
    crate::engine::l5::detector_context::cones_census::add_since(
        &crate::engine::l5::detector_context::cones_census::WALK_SCC_NANOS,
        _t_scc,
    );
    let _t_dedup = crate::engine::l5::detector_context::cones_census::start();
    let mut direct: RoutineDirectFacts = HashMap::new();
    let mut cov: RoutineDirectCoverage = HashMap::new();
    let mut routine_ids: BTreeSet<String> = BTreeSet::new();
    for id in nodes {
        routine_ids.insert(id.clone());
        if let Some(facts) = direct_in.get(id) {
            let byk = dedup_direct(facts);
            if !byk.is_empty() {
                direct.insert(id.clone(), byk);
            }
        }
        if let Some((status, reasons)) = coverage_in.get(id) {
            cov.insert(
                id.clone(),
                DirectCoverage {
                    direct_status: status.clone(),
                    reasons: reasons.clone(),
                },
            );
        }
    }

    crate::engine::l5::detector_context::cones_census::add_since(
        &crate::engine::l5::detector_context::cones_census::WALK_DEDUP_NANOS,
        _t_dedup,
    );
    let _t_compose = crate::engine::l5::detector_context::cones_census::start();
    let routines_by_id: HashMap<&str, &ModelRoutine> =
        routines.iter().map(|r| (r.id.as_str(), r)).collect();
    let frames = guard_frames(&g, routines, &routines_by_id);
    let (cones, derived, pd_solution, _stats) = compose_inherited_cones(
        &g,
        &scc,
        &direct,
        direct_in,
        &cov,
        &routine_ids,
        &routines_by_id,
        &frames,
        mode,
    );
    crate::engine::l5::detector_context::cones_census::add_since(
        &crate::engine::l5::detector_context::cones_census::WALK_COMPOSE_NANOS,
        _t_compose,
    );

    // ⟨C1 census⟩ `direct` — the SCC walk's per-routine DEDUP-KEYED direct-fact
    // map (`inherited_fact_key` -> canonical rep) — is alive for the whole
    // `compose_inherited_cones` call above (borrowed throughout), a SECOND copy
    // of the direct-facts population alongside the caller's own raw map (which
    // becomes `capability_facts_direct` in `summaries`). Task 4 drops it at
    // last use, immediately below, instead of at this function's return.
    if cone_census::enabled() {
        let mut inner_entries: u64 = 0;
        let mut struct_bytes: u64 = 0;
        let mut heap_bytes: u64 = 0;
        let mut outer_key_bytes: u64 = 0;
        for (outer_key, byk) in &direct {
            outer_key_bytes += outer_key.len() as u64;
            for (inner_key, fact) in byk {
                inner_entries += 1;
                struct_bytes += std::mem::size_of::<CapabilityFact>() as u64;
                heap_bytes +=
                    inner_key.len() as u64 + cone_census::capability_fact_heap_bytes(fact);
            }
        }
        let total = struct_bytes + heap_bytes + outer_key_bytes;
        eprintln!("[retained-census] --------------------------------------------------");
        eprintln!(
            "[retained-census] section: `direct` dedup-keyed facts map (compose_cone_over_graph's \
             own SCC-walk input — alive for the whole cone walk, a SECOND copy alongside the \
             final capability_facts_direct; NOT part of any grand_total_bytes)"
        );
        eprintln!(
            "[retained-census]   outer_entries={} inner_entries={inner_entries} outer_key_heap_bytes={outer_key_bytes} struct_bytes={struct_bytes} heap_bytes={heap_bytes}",
            direct.len()
        );
        eprintln!(
            "[retained-census]   direct_dedup_total_bytes={total} ({:.2} MB)  # BTreeMap node overhead NOT modeled (only key+value payload bytes) — a further under-count",
            total as f64 / 1_048_576.0
        );
    }

    // ⟨C1 Task 4⟩ Last use of all three walk inputs was the call above (the
    // census read `direct` one more time). `direct` measured 63.70 MB on the
    // 8020 corpus and `routine_ids` is a full extra copy of every routine id;
    // freeing them here — rather than at this function's closing brace — keeps
    // them out of the peak that the `ConeOutcome` rebuild below adds to.
    drop(direct);
    drop(cov);
    drop(routine_ids);

    ConeOutcome {
        cones: cones
            .into_iter()
            .map(|(id, r)| {
                (
                    id,
                    ConeResultPub {
                        inherited: r.inherited,
                        coverage: r.coverage,
                    },
                )
            })
            .collect(),
        derived,
        pd_solution,
        #[cfg(test)]
        stats: _stats,
    }
}

// ===========================================================================
// R3a-3 STABLE PROJECTION — mirrors scripts/r3a3-projection.ts `projectR3a3`.
// ===========================================================================

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
    #[serde(rename = "constant-var")]
    ConstantVar {
        #[serde(rename = "varName")]
        var_name: String,
        initializer: Box<PValueSource>,
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

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct PCapabilityFact {
    pub op: String,
    #[serde(rename = "resourceKind")]
    pub resource_kind: String,
    pub confidence: String,
    pub provenance: String,
    pub via: String,
    #[serde(rename = "resourceId", skip_serializing_if = "Option::is_none")]
    pub resource_id: Option<String>,
    #[serde(rename = "resourceArgSource", skip_serializing_if = "Option::is_none")]
    pub resource_arg_source: Option<PValueSource>,
    #[serde(rename = "witnessOperationId", skip_serializing_if = "Option::is_none")]
    pub witness_operation_id: Option<String>,
    #[serde(rename = "witnessCallsiteId", skip_serializing_if = "Option::is_none")]
    pub witness_callsite_id: Option<String>,
    /// Only on trigger-first inherited facts (#96 D5).
    #[serde(
        rename = "witnessFirstHopOperationId",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub witness_first_hop_operation_id: Option<String>,
    /// Only on event-dispatch-first inherited facts (#98 D1), stable form.
    #[serde(
        rename = "witnessFirstHopEventId",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub witness_first_hop_event_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extra: Option<serde_json::Value>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct PCoverageRecord {
    pub subject: String,
    #[serde(rename = "directStatus")]
    pub direct_status: String,
    #[serde(rename = "inheritedStatus")]
    pub inherited_status: String,
    pub reasons: Vec<String>,
    #[serde(rename = "unknownTargets")]
    pub unknown_targets: Vec<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct PRoutineConeCoverage {
    #[serde(rename = "routineId")]
    pub routine_id: String,
    #[serde(rename = "capabilityFactsDirect")]
    pub capability_facts_direct: Vec<PCapabilityFact>,
    #[serde(rename = "capabilityFactsInherited")]
    pub capability_facts_inherited: Vec<PCapabilityFact>,
    pub coverage: PCoverageRecord,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct R3a3Projection {
    pub summaries: Vec<PRoutineConeCoverage>,
}

/// Internal RoutineId → StableRoutineId; pass through if unmapped.
pub(crate) fn stable_routine_id(internal: &str, map: &HashMap<String, String>) -> String {
    map.get(internal)
        .cloned()
        .unwrap_or_else(|| internal.to_string())
}

/// Rewrite `${routineId}/<suffix>` → stable form (the suffix is everything after
/// the SECOND `/`). Mirrors `stableSubId`.
fn stable_sub_id(internal_sub_id: &str, map: &HashMap<String, String>) -> String {
    let first = internal_sub_id.find('/');
    let second = first.and_then(|f| internal_sub_id[f + 1..].find('/').map(|s| f + 1 + s));
    match second {
        Some(sec) => {
            let routine_id = &internal_sub_id[..sec];
            let suffix = &internal_sub_id[sec..];
            match map.get(routine_id) {
                Some(stable) => format!("{stable}{suffix}"),
                None => internal_sub_id.to_string(),
            }
        }
        None => internal_sub_id.to_string(),
    }
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

/// Project an internal EventId to StableEventId via the EventSymbol. Mirrors
/// `stableEventId` (dumb `/`→`:` fallback when the symbol is absent).
fn stable_event_id(internal: &str, event_by_id: &HashMap<String, &EventSymbol>) -> String {
    match event_by_id.get(internal) {
        Some(evt) => format!(
            "{}::{}::{}",
            to_stable_object_id(&evt.publisher_object_id),
            evt.event_name,
            evt.signature_hash
        ),
        None => internal.replace('/', ":"),
    }
}

/// Project a CapabilityFact.resourceId to stable form (by resourceKind). Mirrors
/// `stableResourceId`.
fn stable_resource_id(
    internal: &str,
    resource_kind: &str,
    event_by_id: &HashMap<String, &EventSymbol>,
) -> String {
    match resource_kind {
        "table" | "transaction" => {
            if internal.contains("/table/") {
                stable_table_id(internal)
            } else {
                internal.replace('/', ":")
            }
        }
        "event" => stable_event_id(internal, event_by_id),
        "codeunit" | "page" | "report" => to_stable_object_id(internal),
        _ => internal.replace('/', ":"),
    }
}

/// Project an internal ValueSource → stable form (table-field tableId → stable).
fn project_value_source(vs: &ValueSource) -> PValueSource {
    match vs {
        ValueSource::Literal { value } => PValueSource::Literal {
            value: value.clone(),
        },
        ValueSource::Enum { enum_name, member } => PValueSource::Enum {
            enum_name: enum_name.clone(),
            member: member.clone(),
        },
        ValueSource::ConstantVar {
            var_name,
            initializer,
        } => PValueSource::ConstantVar {
            var_name: var_name.clone(),
            initializer: Box::new(project_value_source(initializer)),
        },
        ValueSource::Parameter { index, var_name } => PValueSource::Parameter {
            index: *index,
            var_name: var_name.clone(),
        },
        ValueSource::TableField {
            table_id,
            field_name,
        } => PValueSource::TableField {
            table_id: if table_id == "unknown" {
                "unknown".to_string()
            } else {
                stable_table_id(table_id)
            },
            field_name: field_name.clone(),
        },
        ValueSource::Expression => PValueSource::Expression,
        ValueSource::Unknown => PValueSource::Unknown,
    }
}

pub(crate) fn project_capability_fact(
    f: &CapabilityFact,
    map: &HashMap<String, String>,
    event_by_id: &HashMap<String, &EventSymbol>,
) -> PCapabilityFact {
    PCapabilityFact {
        op: f.op.to_string(),
        resource_kind: f.resource_kind.to_string(),
        confidence: f.confidence.to_string(),
        provenance: f.provenance.to_string(),
        via: f.via.to_string(),
        resource_id: f
            .resource_id
            .as_ref()
            .map(|r| stable_resource_id(r, f.resource_kind, event_by_id)),
        resource_arg_source: f.resource_arg_source.as_ref().map(project_value_source),
        witness_operation_id: f
            .witness_operation_id
            .as_ref()
            .map(|w| stable_sub_id(w, map)),
        witness_callsite_id: f
            .witness_callsite_id
            .as_ref()
            .map(|w| stable_sub_id(w, map)),
        witness_first_hop_operation_id: f
            .witness_first_hop_operation_id
            .as_ref()
            .map(|w| stable_sub_id(w, map)),
        witness_first_hop_event_id: f
            .witness_first_hop_event_id
            .as_ref()
            .map(|e| stable_event_id(e, event_by_id)),
        extra: f.extra.as_ref().map(extra_to_json),
    }
}

/// Sort key for projected capability facts (al-sem `capabilityFactSortKey`).
pub(crate) fn capability_fact_sort_key(f: &PCapabilityFact) -> String {
    [
        f.op.clone(),
        f.resource_kind.clone(),
        f.resource_id.clone().unwrap_or_default(),
        f.confidence.clone(),
        f.via.clone(),
        f.witness_callsite_id.clone().unwrap_or_default(),
        f.witness_operation_id.clone().unwrap_or_default(),
    ]
    .join("|")
}

/// Per-routine coverage reasons from call-site-local uncertainty (#160 steps 3
/// and 5): the kind of every uncertainty edge whose kind
/// [`is_callsite_local_kind`](super::db_effect_solver::is_callsite_local_kind)
/// selects, keyed by the edge's `from`. The reason text is the kind text.
///
/// Every coverage producer calls it once over its edges and folds each row with
/// [`fold_uncertainty_reasons`]: the detector context and the cross-app base
/// over `ExecRelation::uncertainty_all` (both halves), the r3a3 projection and
/// the snapshot base over their own graph's edges.
///
/// A routine's own edges are its whole contribution, because the L4 solve never
/// moves these kinds from a callee to a caller or between SCC members:
/// - a base summary never carries them:
///   `summary_runner::base_intraprocedural_summary` emits only `opaque-body` or
///   `parse-incomplete`;
/// - `db_effect_solver::solve_side_facts` calls `fold_shared` (the set an SCC
///   shares and its callers inherit) only under `if
///   !is_callsite_local_kind(&u.kind)`, both for a member's base uncertainties
///   and for its uncertainty edges; the kinds selected here go only to the
///   member's own list (`own`, kept per member in `own_by_member`), and each
///   member takes back only its own list (`own_by_member.remove(&m_ix)`).
///
/// A `from` with no coverage row may get a key; readers look up row ids only.
/// The map borrows from `edges`.
pub(crate) fn callsite_local_coverage_reasons<'e>(
    edges: impl IntoIterator<Item = &'e UncertaintyEdge>,
) -> HashMap<&'e str, BTreeSet<&'e str>> {
    let mut out: HashMap<&str, BTreeSet<&str>> = HashMap::new();
    for ue in edges {
        let kind = ue.uncertainty.kind.as_str();
        if super::db_effect_solver::is_callsite_local_kind(kind) {
            out.entry(ue.from.as_str()).or_default().insert(kind);
        }
    }
    out
}

/// Fold a routine's call-site-local coverage reasons (`extra`, from
/// [`callsite_local_coverage_reasons`]) into its direct coverage. A reason that
/// is new implies incomplete resolution, so `complete` drops to `partial` and
/// the reasons come back sorted; with nothing new, both come back unchanged.
pub(crate) fn fold_uncertainty_reasons(
    mut status: String,
    reasons: Vec<String>,
    extra: Option<&BTreeSet<&str>>,
) -> (String, Vec<String>) {
    let mut reason_set: BTreeSet<String> = reasons.iter().cloned().collect();
    let base_len = reason_set.len();
    for rr in extra.into_iter().flatten() {
        reason_set.insert((*rr).to_string());
    }
    if reason_set.len() > base_len {
        if status == "complete" {
            status = "partial".to_string();
        }
        (status, reason_set.into_iter().collect())
    } else {
        (status, reasons)
    }
}

// ===========================================================================
// R3a-5 — the FULL cross-app RoutineSummary projection (core + cone/coverage).
//
// Ports al-sem's `analyzeWorkspace` order over the cross-app corpus WITH the dep
// hooks (`scripts/r3a5-projection.ts` `projectR3a5FullSummary`):
//   indexWorkspace(+deps) → withDependencyArtifacts → resolveModel
//   → buildCombinedGraph → injectIntraAppCallEdges → computeSummaries → cone
//
// What's new vs R3a-3 (source-only):
//   - The MERGED model carries dep routines (EMPTY merged features, like al-sem's
//     `EMPTY_FEATURES`) plus their RETAINED summary (the dep's own `via:"direct"`
//     dbEffects) + RETAINED direct capability facts (the dep's own intrinsic
//     facts), recovered from each dep's embedded-source analysis (the R3a-4
//     producer path). Dep routines are LEAVES (the `_with_leaves` solver entry
//     point, today `compute_summaries_v2_bundle_with_leaves`).
//   - injectIntraAppCallEdges adds the dep intra-app `direct-call` edges to the
//     typed-edge graph so the cone propagates the dep's `capabilityFactsDirect`
//     through intra-dep chains AND to primary callers.
// ===========================================================================

/// One projected FULL RoutineSummary (R3a-2 core + R3a-3 cone/coverage + the
/// `isDepRoutine` flag). Field order + key names mirror al-sem
/// `PRoutineFullSummary` (`scripts/r3a5-projection.ts`).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct PRoutineFullSummary {
    #[serde(rename = "routineId")]
    pub routine_id: String,
    #[serde(rename = "isDepRoutine")]
    pub is_dep_routine: bool,
    #[serde(rename = "dbEffects")]
    pub db_effects: Vec<crate::engine::l4::summary::PDbEffect>,
    pub uncertainties: Vec<crate::engine::l4::summary::PUncertainty>,
    #[serde(rename = "parameterRoles")]
    pub parameter_roles: Vec<crate::engine::l4::summary::PRecordRoleSummary>,
    #[serde(rename = "inRecursiveCycle")]
    pub in_recursive_cycle: bool,
    #[serde(rename = "hasUnresolvedCalls")]
    pub has_unresolved_calls: bool,
    #[serde(rename = "capabilityFactsDirect")]
    pub capability_facts_direct: Vec<PCapabilityFact>,
    #[serde(rename = "capabilityFactsInherited")]
    pub capability_facts_inherited: Vec<PCapabilityFact>,
    pub coverage: PCoverageRecord,
}

/// The full R3a-5 cross-app projection — per-routine full summary + the
/// anti-degenerate matrix counts. Mirrors al-sem `R3a5FullSummaryProjection`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct R3a5FullSummaryProjection {
    #[serde(rename = "fixtureName")]
    pub fixture_name: String,
    pub summaries: Vec<PRoutineFullSummary>,
    #[serde(rename = "primaryRoutinesWithInheritedDepFacts")]
    pub primary_routines_with_inherited_dep_facts: usize,
    #[serde(rename = "primaryRoutinesWithDepDbEffects")]
    pub primary_routines_with_dep_db_effects: usize,
    #[serde(rename = "coveragesWithOpaqueAppsReason")]
    pub coverages_with_opaque_apps_reason: usize,
    #[serde(rename = "totalCrossAppInheritedFacts")]
    pub total_cross_app_inherited_facts: usize,
}

/// The FULLY-ASSEMBLED cross-app L4 BASE — every from-scratch intermediate the
/// R3a-5 projection (and the R3b Salsa wrap) consume, before the core/cone +
/// projection. Extracted so the R3b Salsa layer can build its fine-grained inputs
/// from EXACTLY the same base the from-scratch path uses (no divergent assembly).
pub struct R3a5CrossAppBase {
    /// The program build's dependency coverage and ledger (`alsem analyze`'s
    /// preflight, engine-switch S8.3).
    pub coverage: crate::program::resolve::full::FreshCoverage,
    /// `internalsVisibleTo`, lower-case guids: exposing app -> friend apps (d13).
    pub friends: HashMap<String, BTreeSet<String>>,
    /// The cross-app model itself (engine-switch S7.6), with its calls cut to the
    /// relation's primary half (workspace callers only). The detectors get it as their
    /// `resolved`, and the context reads its root classifications and ordering
    /// facts from it. [`Self::ws_routines`] and [`Self::objects`] are views onto
    /// it.
    pub resolved: crate::program::model::workspace::Model,
    /// The ids of the routines whose role is `Dependency`
    /// (`Model::routine_role`, #220): the string-keyed projection of the role
    /// rule, built once here.
    pub dep_routine_ids: BTreeSet<String>,
    /// The view's ONE execution relation (#160 step 4): the Primary payload
    /// (workspace callers' calls plus every event dispatch) drives the solve
    /// and the detector traversals, the Cone payload (its typed edges, the
    /// dependency code's own calls included) drives the cone, and its SCC is
    /// the one every cross-app `build_l4` shares by pointer.
    pub rel: std::sync::Arc<crate::engine::l4::exec_relation::ExecRelation>,
    /// The solver field index: the leaf summaries' and every cross-app
    /// `build_l4` solve's (through [`Self::cross_app_inputs`]), built once.
    pub field_index: crate::engine::l4::summary_runner::FieldIndex,
    /// Fixed-leaf (dep) RETAINED summaries: every source-bearing dependency
    /// routine with no out-edge in the relation's Primary payload.
    pub leaf_summaries: HashMap<String, crate::engine::l4::summary::RoutineSummary>,
    /// Source-bearing dependency routines WITH an out-edge (an event publisher's
    /// subscriber dispatch, #70): solved for their db-effect rows, but their
    /// parameter roles stay the leaf's: the five path-sensitive fields
    /// (`requiresLoadedAtEntry`, `requiredLoadedFieldsAtEntry`,
    /// `mutatesBeforeLoad`, `dirtyAtExit`, `currentLoadedFieldsAtExit`) unknown,
    /// the body-local ones (`persistsCurrentRecord`, `writesFields`,
    /// `validatesParam`, …) from the publisher's own body, exactly as for every
    /// dependency leaf, plus the presence facts their dispatch composes (#107
    /// Design 6). The path facts do not follow the dispatch edges.
    pub fixed_roles: HashSet<String>,
    /// Per-routine direct capability facts (full, ordered).
    pub direct_full: HashMap<String, Vec<CapabilityFact>>,
    /// Per-routine direct coverage `(status, reasons)`.
    pub direct_coverage: HashMap<String, (String, Vec<String>)>,
    /// The DECLARED workspace dependencies `{appGuid, name, minVersion}` (app.json
    /// `dependencies[]`). The d17 MinVersion side. ADDITIVE — only the cross-app L5
    /// detector context reads it; NOT serialized into the r3a5 gate.
    pub declared_dependencies: Vec<crate::program::model::workspace::DeclaredDependencyDecl>,
    /// Every dependency app the build loaded (the model's), `.app` path order:
    /// the cross-app snapshot's `apps[]` dependency rows (#220).
    pub dependency_apps: Vec<crate::program::model::workspace::DependencyApp>,
    /// Resolved dep `.app` versions keyed by appGuid (`model.apps[].version`). The d17
    /// resolved-version side. ADDITIVE — same gate-additivity note as above.
    pub resolved_app_versions: HashMap<String, String>,
    /// Which dependency apps the build loaded, and how (`alsem graph`'s
    /// `meta.apps` rows); `alsem digest` / `alsem query` state it (#66).
    pub loaded_apps: Vec<serde_json::Value>,
}

impl R3a5CrossAppBase {
    /// Every model routine: the workspace's, then each dependency's.
    pub fn ws_routines(&self) -> &Vec<ModelRoutine> {
        &self.resolved.workspace.routines
    }

    pub fn objects(&self) -> &Vec<crate::program::model::workspace::ModelObject> {
        &self.resolved.workspace.objects
    }

    /// What every cross-app detector context takes from this base; the ONE
    /// constructor, so its callers cannot drift.
    pub(crate) fn cross_app_inputs(
        &self,
    ) -> crate::engine::l5::detector_context::CrossAppInputs<'_> {
        crate::engine::l5::detector_context::CrossAppInputs {
            dep_routine_ids: &self.dep_routine_ids,
            leaf_summaries: &self.leaf_summaries,
            fixed_roles: &self.fixed_roles,
            declared_dependencies: &self.declared_dependencies,
            app_versions: &self.resolved_app_versions,
            friends: &self.friends,
            field_index: &self.field_index,
        }
    }
}

/// The cross-app L4 BASE (engine-switch S7.4): every intermediate the R3a-5
/// projection and the cross-app detectors read, from ONE program-backed model
/// (`assemble_and_resolve_cross_app_program`): the workspace and every dependency,
/// each body's calls and the event graph from the program engine. Returns `Err`
/// for a fail-closed workspace, naming why (#160 §4.4.5).
///
/// Before S7.4 there were two bases, both L3: a symbol-only one (r3a5) whose
/// source-bearing dependency routines were empty rows with `body_available`
/// flipped on, and a source-parsing one (r4); each also re-parsed every `.app`
/// twice more (`build_dep_artifact_l4`, `recover_dep_retained`) for facts the model
/// now carries itself. Kept contracts:
/// - a source-bearing dependency routine with no out-edge in the combined graph
///   is a FIXED solver leaf holding its own direct dbEffects
///   (`base_intraprocedural_summary`), so a workspace caller does not inherit
///   what the dependency routine's own callees write (those calls are cut from
///   the combined graph). The only out-edges a dependency routine keeps are an
///   event publisher's subscriber dispatches (#70): such a publisher is solved,
///   so the store follows its dispatch as the cone does, but it keeps the leaf's
///   parameter roles (`fixed_roles`: the five path-sensitive fields unknown, the
///   body-local fields from its own body, as for every dependency leaf), plus
///   the presence facts its dispatch composes (#107 Design 6; path facts stay
///   unknown);
/// - a symbol-only dependency routine is no leaf: its summary is the `opaque-body`
///   self-uncertainty;
/// - every routine's direct capability facts and coverage come from
///   `direct_facts_for_routine` (a bodyless routine: `opaque-dependency`).
///
/// - the relation's Primary payload (solver, SCC, detector traversals) takes the
///   calls of WORKSPACE callers only; a dependency's own calls reach the CONE alone, as
///   typed edges built by the same rules as the workspace's (engine-switch S8.6):
///   object runs and calls from one dependency into another included. S7 fed the
///   cone only the R3a-4 artifact's admitted own-app edges, because the whole
///   dependency world took CDO's r3a5 projection from 329 MB to 9.8 GB; since
///   S8.2 the model holds only the DEMANDED dependency code, and S8.6 measured
///   +1 s and no memory on CDO/DO.
pub(crate) fn build_cross_app_base(
    inputs: &std::sync::Arc<crate::program::inputs::InputSet>,
    model_instance_id: &str,
) -> Result<R3a5CrossAppBase, crate::program::model::program_calls::CrossAppFailure> {
    use crate::engine::l4::summary_runner::base_intraprocedural_summary;
    let mut x = crate::program::model::program_calls::assemble_and_resolve_cross_app_program(
        inputs,
        model_instance_id,
        false,
    )?;
    if x.resolved.primary_app.is_none() {
        return Err(
            crate::program::model::program_calls::CrossAppFailure::NoPrimaryApp {
                fresh: Box::new(x.coverage),
            },
        );
    }
    let ws: &ModelEntities = &x.resolved.workspace;
    let dep_routine_ids: BTreeSet<String> = ws
        .routines
        .iter()
        .filter(|r| x.resolved.routine_role(r) == Role::Dependency)
        .map(|r| r.id.clone())
        .collect();

    // The one call split (#160 step 4), in emission order: the workspace
    // callers' calls (the Primary payload, and the cut model's calls) and the
    // dependency code's own calls, cone-only (see the doc above): every
    // resolved call a dependency routine makes, typed by the same rules as the
    // workspace's (object runs, resolved methods, interface `Maybe`, calls into
    // another dependency). Event dispatch is in the Primary payload.
    //
    // The split MOVES the model's calls out (a clone only if another owner
    // shares them), so no `CallEdge` exists twice; `partition` keeps each
    // half in emission order.
    let crate::program::model::calls::ResolvedCalls {
        edges: all_edges,
        upgraded_bindings,
        diagnostics,
        external_targets,
    } = std::sync::Arc::try_unwrap(std::mem::take(&mut x.resolved.calls))
        .unwrap_or_else(|shared| (*shared).clone());
    let (dependency_calls, primary_calls): (
        Vec<crate::program::model::calls::CallEdge>,
        Vec<crate::program::model::calls::CallEdge>,
    ) = all_edges
        .into_iter()
        .partition(|e| dep_routine_ids.contains(&e.from));
    let ws: &ModelEntities = &x.resolved.workspace;
    let rel = crate::engine::l4::exec_relation::ExecRelation::new(
        ws,
        &primary_calls,
        &dependency_calls,
        &x.resolved.events.graph,
    );
    drop(dependency_calls);
    let calls = crate::program::model::calls::ResolvedCalls {
        edges: primary_calls,
        upgraded_bindings,
        diagnostics,
        external_targets,
    };

    let _s_index = crate::engine::perf_trace::span("crossapp", "crossapp.field_index");
    // The solve's index (summaries-census counted here, not in `build_l4`).
    let field_index = crate::engine::l4::analysis::solver_field_index(ws);
    drop(_s_index);

    let _s_facts = crate::engine::perf_trace::span("crossapp", "crossapp.base_facts");
    let event_graph = &x.resolved.events.graph;
    let mut publisher_events_by_routine: HashMap<String, Vec<&EventSymbol>> = HashMap::new();
    for evt in &event_graph.events {
        if let Some(pr) = &evt.publisher_routine_id {
            publisher_events_by_routine
                .entry(pr.clone())
                .or_default()
                .push(evt);
        }
    }
    // #160 step 11b §4.6: R2b, per row, then the projection: the routine row is
    // class P (LAST); a collided id's leaf summary, direct facts and coverage
    // are the full-value union (coverage: the least complete).
    let rows = crate::program::model::projection::RowsOfId::build(&ws.routines);
    let routines_by_id: HashMap<String, &ModelRoutine> =
        crate::program::model::projection::last_per_id(
            &rows,
            ws.routines.iter().map(Some).collect(),
        );
    let mut leaf_per_row = Vec::with_capacity(ws.routines.len());
    let mut fixed_roles: HashSet<String> = HashSet::new();
    let mut facts_per_row = Vec::with_capacity(ws.routines.len());
    let mut coverage_per_row = Vec::with_capacity(ws.routines.len());
    let empty_pub: Vec<&EventSymbol> = Vec::new();
    // #160 step 5: the one coverage fold, both uncertainty halves.
    let uncertainty_reasons = callsite_local_coverage_reasons(rel.uncertainty_all());
    for r in &ws.routines {
        let mut leaf = None;
        if dep_routine_ids.contains(&r.id) && r.body_available {
            let has_out_edge = rel
                .primary()
                .edges_by_from
                .get(&r.id)
                .is_some_and(|e| !e.is_empty());
            if has_out_edge {
                fixed_roles.insert(r.id.clone());
            } else {
                leaf = Some(base_intraprocedural_summary(
                    r,
                    &routines_by_id,
                    &field_index,
                ));
            }
        }
        leaf_per_row.push(leaf);
        let pubs = publisher_events_by_routine.get(&r.id).unwrap_or(&empty_pub);
        let (facts, status, reasons) = direct_facts_for_routine(r, pubs);
        let coverage =
            fold_uncertainty_reasons(status, reasons, uncertainty_reasons.get(r.id.as_str()));
        coverage_per_row.push(Some(coverage));
        facts_per_row.push(Some(facts));
    }
    let leaf_summaries: HashMap<String, crate::engine::l4::summary::RoutineSummary> =
        crate::program::model::projection::merge_per_id(
            &rows,
            leaf_per_row,
            super::summary_runner::union_summaries,
        );
    let direct_coverage = crate::program::model::projection::merge_per_id(
        &rows,
        coverage_per_row,
        crate::program::model::projection::least_complete,
    );
    let direct_full = crate::program::model::projection::merge_per_id(&rows, facts_per_row, |v| {
        crate::program::model::projection::union_by_value(v, |_| ())
    });
    drop(rows);

    let declared_dependencies = std::mem::take(&mut x.declared_dependencies);
    let dependency_apps = std::mem::take(&mut x.dependency_apps);
    let resolved_app_versions = dependency_apps
        .iter()
        .map(|a| (a.guid.clone(), a.version.clone()))
        .collect();
    let loaded_apps = std::mem::take(&mut x.loaded_apps);
    let mut resolved = x.resolved;
    resolved.calls = std::sync::Arc::new(calls);
    let base = R3a5CrossAppBase {
        coverage: x.coverage.clone(),
        friends: x.friends.clone(),
        resolved,
        dep_routine_ids,
        rel: std::sync::Arc::new(rel),
        field_index,
        leaf_summaries,
        fixed_roles,
        direct_full,
        direct_coverage,
        declared_dependencies,
        dependency_apps,
        resolved_app_versions,
        loaded_apps,
    };
    if crate::engine::census::enabled() {
        crate::census_hook::mark("census.base");
        census_cross_app_base(&base);
    }
    Ok(base)
}

/// #160 §4.1.5: one `base.<field>` bucket per field of [`R3a5CrossAppBase`],
/// shallow. `resolved` is the model, moved in with its calls replaced by the
/// cut set: its rows and events (`base.resolved.rows`, the part of the `model`
/// bucket that survives) and the replacement calls (`base.resolved.calls`) are
/// charged apart, so nothing the model still owns is missing from the base.
/// `rel` prints its own `rel.*` buckets here, after `census.base`, where
/// `base.graph` / `base.combined_scc` printed before #160 step 4.
fn census_cross_app_base(b: &R3a5CrossAppBase) {
    use crate::engine::census::{Kind, Tally};
    use crate::program::model::program_calls::{census_calls, census_model_rows};
    b.rel.emit_census();
    let emit = |name: &str, t: &mut Tally| t.emit(name, Kind::Shallow);
    emit(
        "base.coverage",
        Tally::default()
            .buf::<String>(b.coverage.opaque_apps.len())
            .strs(&b.coverage.opaque_apps)
            .buf::<crate::program::resolve::full::LedgerEntry>(b.coverage.ledger.len())
            .buf::<String>(b.coverage.unidentified_packages.len())
            .strs(&b.coverage.unidentified_packages),
    );
    emit(
        "base.friends",
        Tally::default()
            .buf::<(String, BTreeSet<String>)>(b.friends.len())
            .strs(b.friends.keys())
            .strs(b.friends.values().flatten()),
    );
    emit("base.resolved.rows", &mut census_model_rows(&b.resolved));
    emit("base.resolved.calls", &mut census_calls(&b.resolved.calls));
    emit(
        "base.dep_routine_ids",
        Tally::default()
            .buf::<String>(b.dep_routine_ids.len())
            .strs(&b.dep_routine_ids),
    );
    emit(
        "base.field_index",
        Tally::default()
            .buf::<((String, String), String)>(b.field_index.len())
            .strs(
                b.field_index
                    .iter()
                    .flat_map(|((t, f), id)| [t.as_str(), f.as_str(), id.as_str()]),
            ),
    );
    emit(
        "base.leaf_summaries",
        Tally::default()
            .buf::<(String, crate::engine::l4::summary::RoutineSummary)>(b.leaf_summaries.len())
            .strs(b.leaf_summaries.keys()),
    );
    emit(
        "base.fixed_roles",
        Tally::default()
            .buf::<String>(b.fixed_roles.len())
            .strs(&b.fixed_roles),
    );
    let mut direct = Tally::default();
    direct
        .buf::<(String, Vec<CapabilityFact>)>(b.direct_full.len())
        .strs(b.direct_full.keys());
    for facts in b.direct_full.values() {
        let (sb, hb) = cone_census::facts_bytes(facts);
        direct.bytes += (sb + hb) as usize;
        direct.allocs += usize::from(!facts.is_empty());
    }
    emit("base.direct_full", &mut direct);
    emit(
        "base.direct_coverage",
        Tally::default()
            .buf::<(String, (String, Vec<String>))>(b.direct_coverage.len())
            .strs(b.direct_coverage.keys())
            .strs(b.direct_coverage.values().map(|(s, _)| s))
            .bufs(b.direct_coverage.values().map(|(_, r)| r))
            .strs(b.direct_coverage.values().flat_map(|(_, r)| r)),
    );
    emit(
        "base.declared_dependencies",
        Tally::default().buf::<crate::program::model::workspace::DeclaredDependencyDecl>(
            b.declared_dependencies.len(),
        ),
    );
    emit(
        "base.dependency_apps",
        Tally::default()
            .buf::<crate::program::model::workspace::DependencyApp>(b.dependency_apps.len())
            .strs(
                b.dependency_apps
                    .iter()
                    .flat_map(|a| [&a.guid, &a.publisher, &a.name, &a.version]),
            ),
    );
    emit(
        "base.resolved_app_versions",
        Tally::default()
            .buf::<(String, String)>(b.resolved_app_versions.len())
            .strs(b.resolved_app_versions.keys())
            .strs(b.resolved_app_versions.values()),
    );
    emit(
        "base.loaded_apps",
        Tally::default().buf::<serde_json::Value>(b.loaded_apps.len()),
    );
}

/// Run the FULL cross-app L4 summary pipeline (merged model + dep hooks + cone)
/// and project every routine's full RoutineSummary in stable form. The R3a-5
/// EXIT-GATE differential surface. Engine-never-throws: a fail-closed / dep-less
/// workspace yields an empty projection.
pub fn project_r3a5_cross_app(
    workspace: &std::path::Path,
    model_instance_id: &str,
    fixture_name: &str,
) -> R3a5FullSummaryProjection {
    use crate::engine::l4::summary_runner::compute_summaries_v2_with_leaves_core;

    let empty = R3a5FullSummaryProjection {
        fixture_name: fixture_name.to_string(),
        summaries: Vec::new(),
        primary_routines_with_inherited_dep_facts: 0,
        primary_routines_with_dep_db_effects: 0,
        coverages_with_opaque_apps_reason: 0,
        total_cross_app_inherited_facts: 0,
    };

    let inputs = crate::program::inputs::InputSet::read(workspace, &Default::default());
    let Some(base) = build_cross_app_base(&inputs, model_instance_id).ok() else {
        return empty;
    };
    let ws_routines = base.ws_routines();
    let dep_routine_ids = &base.dep_routine_ids;
    let graph = base.rel.primary();
    let event_graph = &base.resolved.events.graph;
    let direct_full = &base.direct_full;
    // The cone's routine universe: the relation's node ids, every model
    // routine's id, sorted. The cone sorts its graph's nodes itself and reads
    // the list otherwise only into a set and keyed maps, so the order cannot
    // move its output (the r3a5 goldens pin it).
    let nodes = &graph.nodes;

    // From-scratch core (v2 db-effect solver, Phase-1-parity) + cone over the
    // assembled base. Trace + roles-cap diagnostics ignored here, so since #160
    // step 2 (§4.4.7) `aldump --r3a5-cross-app-summary` prints no
    // non-convergence line (only `build_l4` prints).
    let (core_summaries, _) = compute_summaries_v2_with_leaves_core(
        ws_routines,
        graph,
        base.rel.scc(),
        &base.resolved.calls.upgraded_bindings,
        &base.field_index,
        &base.leaf_summaries,
        &base.fixed_roles,
    );
    // ⟨C1 Task 3⟩ `RawOnly` — this projection path consumes the RAW cone (its
    // exact `sort_inherited` byte order IS the R3a-5 golden surface, Global
    // Constraint 7). It never reads `ConeOutcome::derived`, so folding one here
    // was build-then-drop; `RawOnly` skips the fold outright. The raw Vec and its
    // ordering are untouched.
    let cones = compose_cone_over_graph(
        base.rel.cone(),
        nodes,
        &base.direct_full,
        &base.direct_coverage,
        base.ws_routines(),
        ConeOutput::RawOnly,
    )
    .cones;

    project_r3a5_from_parts(
        ws_routines,
        dep_routine_ids,
        &core_summaries,
        &cones,
        direct_full,
        event_graph,
        fixture_name,
    )
}

/// The R3a-5 PROJECTION TAIL — project the (core JACOBI summaries + cone) over the
/// merged model into the stable full-RoutineSummary surface + the anti-degenerate
/// counts. A pure function of its parts, shared by the from-scratch
/// `project_r3a5_cross_app` AND the R3b Salsa wrap (which demands the
/// `core_summaries` and `cones` through the Salsa queries — proving the wrapped
/// projection is byte-identical to the from-scratch one).
#[allow(clippy::too_many_arguments)]
pub(crate) fn project_r3a5_from_parts(
    ws_routines: &[ModelRoutine],
    dep_routine_ids: &BTreeSet<String>,
    core_summaries: &HashMap<String, crate::engine::l4::summary::RoutineSummary>,
    cones: &HashMap<String, ConeResultPub>,
    direct_full: &HashMap<String, Vec<CapabilityFact>>,
    event_graph: &EventGraph,
    fixture_name: &str,
) -> R3a5FullSummaryProjection {
    use crate::engine::l4::summary::project_routine_summary_core_pub;

    let map: HashMap<String, String> = ws_routines
        .iter()
        .map(|r| (r.id.clone(), r.stable_routine_id.clone()))
        .collect();
    let event_by_id: HashMap<String, &EventSymbol> = event_graph
        .events
        .iter()
        .map(|e| (e.id.clone(), e))
        .collect();

    let mut summaries: Vec<PRoutineFullSummary> = Vec::new();
    for r in ws_routines {
        let is_dep = dep_routine_ids.contains(&r.id);

        // R3a-2 core (dbEffects / uncertainties / parameterRoles / cycle / unresolved).
        let core = core_summaries
            .get(&r.id)
            .map(|s| project_routine_summary_core_pub(s, &map));

        let (db_effects, uncertainties, parameter_roles, in_recursive_cycle, has_unresolved_calls) =
            match core {
                Some(c) => (
                    c.db_effects,
                    c.uncertainties,
                    c.parameter_roles,
                    c.in_recursive_cycle,
                    c.has_unresolved_calls,
                ),
                None => (Vec::new(), Vec::new(), Vec::new(), false, false),
            };

        // R3a-3 cone + coverage.
        let empty_facts: Vec<CapabilityFact> = Vec::new();
        let mut direct_facts: Vec<PCapabilityFact> = direct_full
            .get(&r.id)
            .unwrap_or(&empty_facts)
            .iter()
            .map(|f| project_capability_fact(f, &map, &event_by_id))
            .collect();
        direct_facts.sort_by_key(capability_fact_sort_key);

        let (inherited_facts, coverage) = match cones.get(&r.id) {
            Some(cone) => {
                let mut inh: Vec<PCapabilityFact> = cone
                    .inherited
                    .iter()
                    .map(|f| project_capability_fact(f, &map, &event_by_id))
                    .collect();
                inh.sort_by_key(capability_fact_sort_key);

                let mut reasons = cone.coverage.reasons.clone();
                reasons.sort();
                let mut unknown_targets: Vec<String> = cone
                    .coverage
                    .unknown_targets
                    .iter()
                    .map(|t| stable_routine_id(t, &map))
                    .collect();
                unknown_targets.sort();

                (
                    inh,
                    PCoverageRecord {
                        subject: stable_routine_id(&cone.coverage.subject, &map),
                        direct_status: cone.coverage.direct_status.clone(),
                        inherited_status: cone.coverage.inherited_status.clone(),
                        reasons,
                        unknown_targets,
                    },
                )
            }
            None => (
                Vec::new(),
                PCoverageRecord {
                    subject: stable_routine_id(&r.id, &map),
                    direct_status: "unknown".to_string(),
                    inherited_status: "unknown".to_string(),
                    reasons: Vec::new(),
                    unknown_targets: Vec::new(),
                },
            ),
        };

        summaries.push(PRoutineFullSummary {
            routine_id: stable_routine_id(&r.id, &map),
            is_dep_routine: is_dep,
            db_effects,
            uncertainties,
            parameter_roles,
            in_recursive_cycle,
            has_unresolved_calls,
            capability_facts_direct: direct_facts,
            capability_facts_inherited: inherited_facts,
            coverage,
        });
    }
    summaries.sort_by(|a, b| a.routine_id.cmp(&b.routine_id));

    // --- Anti-degenerate matrix counts (over the PRIMARY routines). ---
    let primary: Vec<&PRoutineFullSummary> =
        summaries.iter().filter(|s| !s.is_dep_routine).collect();
    let primary_routines_with_inherited_dep_facts = primary
        .iter()
        .filter(|s| !s.capability_facts_inherited.is_empty())
        .count();
    let primary_routines_with_dep_db_effects =
        primary.iter().filter(|s| !s.db_effects.is_empty()).count();
    let coverages_with_opaque_apps_reason = summaries
        .iter()
        .filter(|s| {
            s.coverage.reasons.iter().any(|r| {
                r == "opaque-dependency" || r == "external-target" || r == "missing-dep-package"
            })
        })
        .count();
    let total_cross_app_inherited_facts: usize = primary
        .iter()
        .map(|s| s.capability_facts_inherited.len())
        .sum();

    R3a5FullSummaryProjection {
        fixture_name: fixture_name.to_string(),
        summaries,
        primary_routines_with_inherited_dep_facts,
        primary_routines_with_dep_db_effects,
        coverages_with_opaque_apps_reason,
        total_cross_app_inherited_facts,
    }
}

// ===========================================================================
// project_r3a3 — the Model entry point.
// ===========================================================================

/// Run the full source-only pipeline (call-resolve → combined graph → typed-edge
/// graph + SCC → direct facts → cone) and project the post-computeSummaries
/// cone+coverage surface. READ-once; no dep hooks; no JACOBI summary core
/// (the cone reads ONLY the direct facts + the typed edges, exactly as al-sem's
/// post-fixed-point cone pass does over the source-only model).
pub fn project_r3a3(resolved: &Model) -> R3a3Projection {
    let ws: &ModelEntities = &resolved.workspace;
    let calls = &resolved.calls;
    let event_graph = &resolved.events.graph;
    let graph = build_combined_graph(ws, calls, event_graph);

    // Typed-edge graph (cone substrate) + Tarjan SCC over it.
    let nodes: Vec<String> = ws.routines.iter().map(|r| r.id.clone()).collect();
    let g = build_typed_edge_graph(&graph.typed_edges, &nodes);
    let mut adjacency: HashMap<String, Vec<String>> = HashMap::new();
    for (from, list) in &g.outgoing {
        adjacency.insert(from.clone(), list.iter().map(|e| e.to.clone()).collect());
    }
    crate::engine::perf_trace::counter_delta("build.graph_scc", 1);
    let scc = tarjan_scc(&SccInputGraph {
        nodes: &g.nodes,
        edges_by_from: &adjacency,
    });

    // Publisher events indexed by routine (for the publisher-fact injection).
    let mut publisher_events_by_routine: HashMap<String, Vec<&EventSymbol>> = HashMap::new();
    for evt in &event_graph.events {
        if let Some(pr) = &evt.publisher_routine_id {
            publisher_events_by_routine
                .entry(pr.clone())
                .or_default()
                .push(evt);
        }
    }

    // Per-routine UNCERTAINTY-DERIVED coverage reasons.
    // A scan of the combined graph's uncertainty edges for the four call-site-local
    // kinds (ambiguous-overload / member-not-found / external-target /
    // interface-open-world); no L4 solve (#160 step 3). A routine carrying any of
    // these has its directStatus downgraded complete→partial so the coverage cone
    // forwards the reason (+ adds the routine to unknownTargets). Source-only
    // `interfaceImplsKnowledgePartial` is false, so the
    // `interface-impls-unknown-in-deps` add-on never fires.
    let uncertainty_reasons = callsite_local_coverage_reasons(&graph.uncertainty_edges);

    // Per-routine direct facts (full) + direct coverage + the dedup-keyed map.
    // #160 step 11b §4.6: R2c, per row, then the projection: a collided id's
    // direct facts are the full-value union, its coverage the least complete.
    let mut routine_ids: BTreeSet<String> = BTreeSet::new();
    let empty_pub: Vec<&EventSymbol> = Vec::new();
    let mut facts_per_row = Vec::with_capacity(ws.routines.len());
    let mut cov_per_row = Vec::with_capacity(ws.routines.len());

    for r in &ws.routines {
        routine_ids.insert(r.id.clone());
        let pubs = publisher_events_by_routine.get(&r.id).unwrap_or(&empty_pub);
        let (facts, status, reasons) = direct_facts_for_routine(r, pubs);

        // Fold uncertainty-derived coverage reasons (identical to the snapshot base).
        let (status, final_reasons) =
            fold_uncertainty_reasons(status, reasons, uncertainty_reasons.get(r.id.as_str()));

        cov_per_row.push(Some((status, final_reasons)));
        facts_per_row.push(Some(facts));
    }
    let rows = crate::program::model::projection::RowsOfId::build(&ws.routines);
    let direct_full: HashMap<String, Vec<CapabilityFact>> =
        crate::program::model::projection::merge_per_id(&rows, facts_per_row, |v| {
            crate::program::model::projection::union_by_value(v, |_| ())
        });
    let cov: RoutineDirectCoverage = crate::program::model::projection::merge_per_id(
        &rows,
        cov_per_row,
        crate::program::model::projection::least_complete,
    )
    .into_iter()
    .map(|(id, (direct_status, reasons))| {
        (
            id,
            DirectCoverage {
                direct_status,
                reasons,
            },
        )
    })
    .collect();
    drop(rows);
    // Canonical rep per dedup key (min repKey wins) — the
    // composeInheritedCones direct-fact dedup, over the per-id facts.
    let direct: RoutineDirectFacts = direct_full
        .iter()
        .filter_map(|(id, facts)| {
            let byk = dedup_direct(facts);
            (!byk.is_empty()).then(|| (id.clone(), byk))
        })
        .collect();

    // ⟨C1 Task 3⟩ `RawOnly` — this projection consumes the RAW cone (its
    // `sort_inherited` byte order IS the R3a-3 golden surface, Global Constraint
    // 7) and discards the derived store. Folding one was build-then-drop.
    let routines_by_id: HashMap<&str, &ModelRoutine> =
        ws.routines.iter().map(|r| (r.id.as_str(), r)).collect();
    let frames = guard_frames(&g, &ws.routines, &routines_by_id);
    let (cones, _derived, _pd_solution, _stats) = compose_inherited_cones(
        &g,
        &scc,
        &direct,
        &direct_full,
        &cov,
        &routine_ids,
        &routines_by_id,
        &frames,
        ConeOutput::RawOnly,
    );

    // ── Project ───────────────────────────────────────────────────────────
    let map: HashMap<String, String> = ws
        .routines
        .iter()
        .map(|r| (r.id.clone(), r.stable_routine_id.clone()))
        .collect();
    let event_by_id: HashMap<String, &EventSymbol> = event_graph
        .events
        .iter()
        .map(|e| (e.id.clone(), e))
        .collect();

    let mut summaries: Vec<PRoutineConeCoverage> = Vec::new();
    for r in &ws.routines {
        let Some(cone) = cones.get(&r.id) else {
            continue;
        };

        let empty_facts: Vec<CapabilityFact> = Vec::new();
        let mut direct_facts: Vec<PCapabilityFact> = direct_full
            .get(&r.id)
            .unwrap_or(&empty_facts)
            .iter()
            .map(|f| project_capability_fact(f, &map, &event_by_id))
            .collect();
        direct_facts.sort_by_key(capability_fact_sort_key);

        let mut inherited_facts: Vec<PCapabilityFact> = cone
            .inherited
            .iter()
            .map(|f| project_capability_fact(f, &map, &event_by_id))
            .collect();
        inherited_facts.sort_by_key(capability_fact_sort_key);

        let mut reasons = cone.coverage.reasons.clone();
        reasons.sort();
        let mut unknown_targets: Vec<String> = cone
            .coverage
            .unknown_targets
            .iter()
            .map(|t| stable_routine_id(t, &map))
            .collect();
        unknown_targets.sort();

        summaries.push(PRoutineConeCoverage {
            routine_id: stable_routine_id(&r.id, &map),
            capability_facts_direct: direct_facts,
            capability_facts_inherited: inherited_facts,
            coverage: PCoverageRecord {
                subject: stable_routine_id(&cone.coverage.subject, &map),
                direct_status: cone.coverage.direct_status.clone(),
                inherited_status: cone.coverage.inherited_status.clone(),
                reasons,
                unknown_targets,
            },
        });
    }
    summaries.sort_by(|a, b| a.routine_id.cmp(&b.routine_id));

    R3a3Projection { summaries }
}

// ===========================================================================
// R4-F Stage-2b — SOURCE-ONLY cone/graph BASE for the CapabilitySnapshot.
//
// The snapshot's consumed-core derivers (capabilityFacts / typedEdges /
// operation+callsite evidence / callsiteResolutions / coverage / eventDeclarations)
// all RE-PROJECT the SAME source-only R3a-3 substrate the cone path assembles. This
// helper exposes that substrate's RAW (internal-id) parts so `engine::l5::snapshot`
// reshapes + rewrites-to-stable + sorts WITHOUT re-deriving any fact/edge/cone.
//
// Mirrors the inline assembly of `project_r3a3` (the model's calls and events
// → build_combined_graph → direct_facts_for_routine + uncertainty-folded coverage →
// compose_cone_over_graph), but returns the parts instead of projecting them.
// ===========================================================================

/// The RAW source-only L4 cone/graph base — every internal-id part the R4-F
/// snapshot derivers consume. Built by `build_r3a3_source_only_base`.
pub struct R3a3SourceBase {
    /// Workspace routines (clone) — the universe + per-routine features.
    pub ws_routines: Vec<ModelRoutine>,
    /// The combined graph (typed edges + call substrate).
    pub graph: CombinedGraph,
    /// The resolved call edges (for the callsite-resolution ledger).
    pub calls: crate::program::model::calls::ResolvedCalls,
    /// The event graph (publisher symbols + subscriber edges).
    pub event_graph: EventGraph,
    /// Per-routine RAW direct capability facts (full, ordered — NOT deduped).
    pub direct_full: HashMap<String, Vec<CapabilityFact>>,
    /// Per-routine cone result (inherited facts + coverage), internal-id form.
    pub cones: HashMap<String, ConeResultPub>,
    /// Internal RoutineId → StableRoutineId.
    pub routine_to_stable: HashMap<String, String>,
    /// The producer solve (#69 D1), internal ids; the snapshot maps it (D2).
    pub pd_solution: super::pd_solve::ConeArena,
}

/// Assemble the RAW source-only cone/graph base (mirrors `project_r3a3`'s inline
/// assembly, returning the parts). Fail-closed callers get an empty base via the
/// resolved workspace having zero routines.
///
/// The LEGACY CUT (#160 step 4, amendment M9): this base builds its own graph
/// over `resolved.calls`, which cross-app is the primary half only, without
/// the dependency typed edges the view's `ExecRelation` carries. The snapshot
/// stays on it until step 6 deletes it.
pub fn build_r3a3_source_only_base(resolved: &Model) -> R3a3SourceBase {
    use crate::engine::perf_trace as pt;
    // Not `r3a3.calls_graph`: that span also covered the input clones, now
    // `r3a3.clone_inputs` (a changed span gets a new name, spec §4.1.1).
    let _s0 = pt::span("r3a3", "r3a3.graph_build");
    let graph = build_combined_graph(&resolved.workspace, &resolved.calls, &resolved.events.graph);
    drop(_s0);
    build_r3a3_source_only_base_over(resolved, graph)
}

/// Internal routine id → stable routine id over every routine of `ws`: the
/// one builder (`R3a3SourceBase::routine_to_stable`, the snapshot closure).
pub fn routine_to_stable_of(ws: &ModelEntities) -> HashMap<String, String> {
    ws.routines
        .iter()
        .map(|r| (r.id.clone(), r.stable_routine_id.clone()))
        .collect()
}

/// [`build_r3a3_source_only_base`] with the graph taken as a parameter (#160
/// §4.1.6): calls and event graph still cloned from `resolved`, the direct facts,
/// the uncertainty fold (a scan of `graph`'s uncertainty edges, no L4 solve) and
/// the cone computed over `graph`. Only the full-graph
/// timings mode passes a graph other than the one the wrapper builds.
pub fn build_r3a3_source_only_base_over(resolved: &Model, graph: CombinedGraph) -> R3a3SourceBase {
    use crate::engine::perf_trace as pt;
    let ws: &ModelEntities = &resolved.workspace;
    let _s0 = pt::span("r3a3", "r3a3.clone_inputs");
    let calls = (*resolved.calls).clone();
    let event_graph: EventGraph = resolved.events.graph.clone();
    drop(_s0);
    // Not `r3a3.direct_facts`: that span also covered the coverage fold's L4
    // solve, now a scan (a changed span gets a new name, #160 step 3).
    let _s1 = pt::span("r3a3", "r3a3.direct_facts_scan");

    // Publisher events indexed by routine (for the publisher-fact injection).
    let mut publisher_events_by_routine: HashMap<String, Vec<&EventSymbol>> = HashMap::new();
    for evt in &event_graph.events {
        if let Some(pr) = &evt.publisher_routine_id {
            publisher_events_by_routine
                .entry(pr.clone())
                .or_default()
                .push(evt);
        }
    }

    // Per-routine UNCERTAINTY-DERIVED coverage reasons: a scan of `graph`'s
    // uncertainty edges, no L4 solve (#160 step 3).
    let uncertainty_reasons = callsite_local_coverage_reasons(&graph.uncertainty_edges);

    // Per-routine direct facts (full) + uncertainty-folded direct coverage.
    // #160 step 11b §4.6: R2c, per row, then the projection (full-value union;
    // coverage the least complete).
    let empty_pub: Vec<&EventSymbol> = Vec::new();
    let mut facts_per_row = Vec::with_capacity(ws.routines.len());
    let mut coverage_per_row = Vec::with_capacity(ws.routines.len());
    for r in &ws.routines {
        let pubs = publisher_events_by_routine.get(&r.id).unwrap_or(&empty_pub);
        let (facts, status, reasons) = direct_facts_for_routine(r, pubs);

        // Fold uncertainty-derived coverage reasons (identical to project_r3a3).
        let (status, final_reasons) =
            fold_uncertainty_reasons(status, reasons, uncertainty_reasons.get(r.id.as_str()));

        coverage_per_row.push(Some((status, final_reasons)));
        facts_per_row.push(Some(facts));
    }
    let rows = crate::program::model::projection::RowsOfId::build(&ws.routines);
    let direct_coverage: HashMap<String, (String, Vec<String>)> =
        crate::program::model::projection::merge_per_id(
            &rows,
            coverage_per_row,
            crate::program::model::projection::least_complete,
        );
    let direct_full: HashMap<String, Vec<CapabilityFact>> =
        crate::program::model::projection::merge_per_id(&rows, facts_per_row, |v| {
            crate::program::model::projection::union_by_value(v, |_| ())
        });
    drop(rows);

    drop(_s1);
    let _s2 = pt::span("r3a3", "r3a3.compose_cone");
    let nodes: Vec<String> = ws.routines.iter().map(|r| r.id.clone()).collect();
    // ⟨C1 Task 3⟩ `RawOnly` — the snapshot/digest/prove derivers read the RAW
    // cone off this base and never touch `ConeOutcome::derived` (see
    // `project_r3a5_cross_app`). Raw bytes and ordering unchanged.
    let ConeOutcome {
        cones, pd_solution, ..
    } = compose_cone_over_graph(
        &graph.typed_edges,
        &nodes,
        &direct_full,
        &direct_coverage,
        &ws.routines,
        ConeOutput::RawOnly,
    );

    let routine_to_stable = routine_to_stable_of(ws);

    R3a3SourceBase {
        ws_routines: ws.routines.clone(),
        graph,
        calls,
        event_graph,
        direct_full,
        cones,
        routine_to_stable,
        pd_solution,
    }
}

/// `aldump --issue160-full-graph-timings` (#160 §4.1.6): the snapshot's cost over
/// today's CUT graph and over the cross-app base's FULL graph, from one binary.
/// Measurement only; deleted when step 6 lands. The cone wall and the solve are
/// read off the cone census, so the caller runs it with `ALSEM_CONES_CENSUS=1`;
/// without it this returns `Err` rather than print zeros. `Ok(None)`: the
/// workspace has no cross-app base.
#[doc(hidden)]
pub fn issue160_full_graph_timings(
    workspace: &std::path::Path,
) -> Result<Option<String>, &'static str> {
    use crate::engine::l5::detector_context::cones_census as cc;
    use std::sync::atomic::Ordering;
    if !cc::enabled() {
        return Err("--issue160-full-graph-timings needs ALSEM_CONES_CENSUS=1");
    }
    let Some(base) = build_cross_app_base(
        &crate::program::inputs::InputSet::read(workspace, &Default::default()),
        crate::program::model::workspace::MODEL_INSTANCE_ID_DEFAULT,
    )
    .ok() else {
        return Ok(None);
    };
    let cone_nanos = || {
        [
            &cc::WALK_GRAPH_NANOS,
            &cc::WALK_SCC_NANOS,
            &cc::WALK_DEDUP_NANOS,
            &cc::WALK_COMPOSE_NANOS,
        ]
        .iter()
        .map(|c| c.load(Ordering::Relaxed))
        .sum::<u64>()
    };
    let ms = |n: u64| n as f64 / 1e6;
    let mut out = String::new();
    for side in ["cut", "full"] {
        let (cone0, pd0) = (cone_nanos(), cc::PD_SOLVE_NANOS.load(Ordering::Relaxed));
        let sb = if side == "cut" {
            build_r3a3_source_only_base(&base.resolved)
        } else {
            build_r3a3_source_only_base_over(
                &base.resolved,
                CombinedGraph {
                    primary: (**base.rel.primary()).clone(),
                    typed_edges: base.rel.cone().to_vec(),
                },
            )
        };
        let (cone1, pd1) = (cone_nanos(), cc::PD_SOLVE_NANOS.load(Ordering::Relaxed));
        let t = std::time::Instant::now();
        let snap = crate::engine::l5::snapshot::compose_snapshot_from_base(&base.resolved, &sb);
        let derive = t.elapsed().as_nanos() as u64;
        drop(snap);
        let inherited: usize = sb.cones.values().map(|c| c.inherited.len()).sum();
        // §4.1.6's raw population in bytes, by the census rule (each cone's
        // buffer plus every fact's owned heap).
        let inherited_bytes: usize = sb
            .cones
            .values()
            .map(|c| super::cone_census::facts_tally(&c.inherited).bytes)
            .sum();
        out.push_str(&format!(
            "{side} cone_wall_ms={:.3}\n{side} pd_solve_ms={:.3}\n{side} snapshot_derive_ms={:.3}\n{side} inherited_facts={inherited}\n{side} inherited_bytes={inherited_bytes}\n",
            ms(cone1 - cone0),
            ms(pd1 - pd0),
            ms(derive),
        ));
    }
    Ok(Some(out))
}

// ===========================================================================
// REAL anti-degenerate matrix counts (review note 1). An INDEPENDENT oracle:
// re-derives the typed-edge graph + per-routine direct-fact keys, then runs a
// clean multi-source BFS over the typed edges to compute, per routine + per
// inheritedFactKey, the GENUINE shortest call-distance witness and whether ≥2
// DISTINCT first-hop edges reach that key at the same shortest distance (an
// equal-distance tie). It does NOT reuse the production singleton/BFS retag
// path, so it cross-validates the cone rather than echoing it.
// ===========================================================================

/// Real BFS-derived matrix counts for one workspace.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct R3a3RealMatrix {
    /// Routines emitting ≥1 inherited capability fact (genuine, distance ≥ 1).
    pub routines_with_inherited_facts: usize,
    /// Inherited facts whose GENUINE shortest witness distance is ≥ 2 hops.
    pub facts_with_more_than_1_hop_witness: usize,
    /// Inherited facts reached at their shortest distance by ≥2 DISTINCT first-hop
    /// edges (the equal-distance tie-breaker genuinely fired).
    pub equal_distance_ties: usize,
}

/// Compute the REAL (BFS-derived) anti-degenerate counts over a resolved
/// source-only workspace. Independent of `project_r3a3`'s cone path.
pub fn compute_r3a3_real_matrix(resolved: &Model) -> R3a3RealMatrix {
    let ws: &ModelEntities = &resolved.workspace;
    let calls = &resolved.calls;
    let event_graph = &resolved.events.graph;
    let graph = build_combined_graph(ws, calls, event_graph);

    let nodes: Vec<String> = ws.routines.iter().map(|r| r.id.clone()).collect();
    let g = build_typed_edge_graph(&graph.typed_edges, &nodes);

    // Publisher events for the direct-fact key set.
    let mut publisher_events_by_routine: HashMap<String, Vec<&EventSymbol>> = HashMap::new();
    for evt in &event_graph.events {
        if let Some(pr) = &evt.publisher_routine_id {
            publisher_events_by_routine
                .entry(pr.clone())
                .or_default()
                .push(evt);
        }
    }

    // Per-routine SET of direct-fact dedup keys (inheritedFactKey).
    // #160 step 11b §4.6: R2c, per row, then the projection (a collided id's
    // key sets are unioned).
    let empty_pub: Vec<&EventSymbol> = Vec::new();
    let mut keys_per_row = Vec::with_capacity(ws.routines.len());
    for r in &ws.routines {
        let pubs = publisher_events_by_routine.get(&r.id).unwrap_or(&empty_pub);
        let (facts, _, _) = direct_facts_for_routine(r, pubs);
        let mut keys: BTreeSet<String> = BTreeSet::new();
        for f in &facts {
            keys.insert(inherited_fact_key(f));
        }
        keys_per_row.push((!keys.is_empty()).then_some(keys));
    }
    let direct_keys: HashMap<String, BTreeSet<String>> =
        crate::program::model::projection::merge_per_id(
            &crate::program::model::projection::RowsOfId::build(&ws.routines),
            keys_per_row,
            |sets| sets.into_iter().flatten().collect(),
        );

    let empty_edges: Vec<TypedOutEdge> = Vec::new();
    let mut m = R3a3RealMatrix::default();

    for r in &ws.routines {
        let subject = &r.id;
        // Multi-source BFS over the typed-edge graph from `subject`. For each
        // reachable node we record its BFS distance from subject. We then, per
        // inheritedFactKey, find the MIN distance at which it first becomes
        // available (a callee at BFS distance d contributes its direct keys at
        // call-distance d+... — but the inheritedFactKey witness distance is the
        // shortest path on which the key appears). We compute, per key, the set of
        // FIRST-HOP edges that reach it at the genuine minimum total distance.

        // node -> shortest BFS distance from subject (excluding subject itself).
        let mut node_dist: HashMap<String, usize> = HashMap::new();
        // (firstHopKey, node) used to attribute first-hop edges.
        let mut queue: std::collections::VecDeque<(String, usize)> =
            std::collections::VecDeque::new();
        let mut visited: BTreeSet<String> = BTreeSet::new();
        visited.insert(subject.clone());
        for e in g.outgoing.get(subject).unwrap_or(&empty_edges) {
            if visited.insert(e.to.clone()) {
                node_dist.insert(e.to.clone(), 0);
                queue.push_back((e.to.clone(), 0));
            }
        }
        while let Some((id, d)) = queue.pop_front() {
            for e in g.outgoing.get(&id).unwrap_or(&empty_edges) {
                if visited.insert(e.to.clone()) {
                    node_dist.insert(e.to.clone(), d + 1);
                    queue.push_back((e.to.clone(), d + 1));
                }
            }
        }

        // For each inheritedFactKey reachable through any callee, the witness
        // distance is `min over nodes carrying that key of (node_dist + 1)`. The
        // first-hop edges achieving that min are those whose target node lies on a
        // shortest path of that length. We approximate the first-hop SET by: for
        // each DIRECT successor edge `e`, run the key's reachable min-distance
        // *through that edge's subtree* — but the simplest sound tie test is: a
        // key is a tie iff ≥2 distinct first-hop edges each reach a key-carrying
        // node at the SAME minimal total distance.
        // Min witness distance per key:
        let mut key_min: BTreeMap<String, usize> = BTreeMap::new();
        for (node, nd) in &node_dist {
            if let Some(keys) = direct_keys.get(node) {
                for k in keys {
                    let cand = nd + 1;
                    key_min
                        .entry(k.clone())
                        .and_modify(|cur| {
                            if cand < *cur {
                                *cur = cand;
                            }
                        })
                        .or_insert(cand);
                }
            }
        }

        if key_min.is_empty() {
            continue;
        }
        m.routines_with_inherited_facts += 1;

        // Per first-hop edge: BFS within that edge's reachable subtree to find the
        // distance at which each key first appears VIA that first hop.
        // firstHopId -> (key -> minDistViaThisHop).
        let mut per_hop_key_dist: Vec<BTreeMap<String, usize>> = Vec::new();
        for fe in g.outgoing.get(subject).unwrap_or(&empty_edges) {
            let mut hop_node_dist: HashMap<String, usize> = HashMap::new();
            let mut hop_visited: BTreeSet<String> = BTreeSet::new();
            hop_visited.insert(subject.clone());
            let mut hq: std::collections::VecDeque<(String, usize)> =
                std::collections::VecDeque::new();
            if hop_visited.insert(fe.to.clone()) {
                hop_node_dist.insert(fe.to.clone(), 0);
                hq.push_back((fe.to.clone(), 0));
            }
            while let Some((id, d)) = hq.pop_front() {
                for e in g.outgoing.get(&id).unwrap_or(&empty_edges) {
                    if hop_visited.insert(e.to.clone()) {
                        hop_node_dist.insert(e.to.clone(), d + 1);
                        hq.push_back((e.to.clone(), d + 1));
                    }
                }
            }
            let mut hk: BTreeMap<String, usize> = BTreeMap::new();
            for (node, nd) in &hop_node_dist {
                if let Some(keys) = direct_keys.get(node) {
                    for k in keys {
                        let cand = nd + 1;
                        hk.entry(k.clone())
                            .and_modify(|cur| {
                                if cand < *cur {
                                    *cur = cand;
                                }
                            })
                            .or_insert(cand);
                    }
                }
            }
            per_hop_key_dist.push(hk);
        }

        for (k, &min_d) in &key_min {
            if min_d >= 2 {
                m.facts_with_more_than_1_hop_witness += 1;
            }
            // Tie: ≥2 distinct first-hop edges reach this key at the global min.
            let hop_count = per_hop_key_dist
                .iter()
                .filter(|hk| hk.get(k).copied() == Some(min_d))
                .count();
            if hop_count >= 2 {
                m.equal_distance_ties += 1;
            }
        }
    }

    m
}

/// #69 D1.3: the two cone walks read the subject's own producer-solve entries.
#[cfg(test)]
mod pd_uses_tests {
    use std::collections::{BTreeSet, HashMap};
    use std::sync::Arc;

    use super::*;
    use crate::engine::l4::cone_derived::{ConeDerivedBuilder, ConeOutput};
    use crate::engine::l4::db_effect_solver::tests::pd_ts_known;
    use crate::engine::l4::pd_solve::ResolvedEntry;

    /// An own entry for subject `C`: a physical (known/false) insert into
    /// `app/Table/1`, reached through `cs1` at distance 1. No cone and no graph
    /// edge supplies it, so only the `own` merge can put it in the output.
    fn own_entry() -> ResolvedEntry<'static> {
        let rep = CapabilityFact {
            subject: "H".to_string(),
            op: "insert",
            resource_kind: "table",
            resource_id: Some("app/Table/1".to_string()),
            resource_arg_source: None,
            confidence: "static",
            provenance: "direct",
            via: "self",
            witness_operation_id: Some("H/op1".to_string()),
            witness_callsite_id: None,
            witness_first_hop_operation_id: None,
            witness_first_hop_event_id: None,
            extra: Some(CapabilityExtra::Table {
                record_variable_id: None,
                temp_state: Some(pd_ts_known(false)),
                op_subtype: None,
            }),
        };
        ResolvedEntry {
            state: 1,
            rep: Arc::new(rep),
            dist: 1,
            first_hop: Box::leak(Box::new(TypedOutEdge::new(
                "H".to_string(),
                "direct-call".to_string(),
                Some("cs1".to_string()),
                None,
                None,
            ))),
        }
    }

    fn empty_graph() -> TypedEdgeGraph {
        TypedEdgeGraph {
            nodes: Vec::new(),
            outgoing: HashMap::new(),
            unresolved_sources: BTreeSet::new(),
        }
    }

    fn singleton(mode: ConeOutput, derived: &mut ConeDerivedBuilder) -> Vec<CapabilityFact> {
        let own = [own_entry()];
        let scc: HashMap<String, usize> = [("C".to_string(), 0)].into();
        inherited_facts_for_singleton(
            "C",
            &empty_graph(),
            &scc,
            &HashMap::new(),
            None,
            None,
            &HashMap::new(),
            &own,
            mode,
            derived,
            &mut HashMap::new(),
            &mut ComposeStats::default(),
        )
    }

    /// Use (a), singleton walk: the own entry is a candidate, and (b) the raw
    /// output retags it onto the subject through its first hop.
    #[test]
    fn singleton_walk_emits_own_entries_retagged() {
        let mut derived = ConeDerivedBuilder::default();
        let out = singleton(ConeOutput::RawOnly, &mut derived);
        assert_eq!(out.len(), 1);
        let f = &out[0];
        assert_eq!(f.subject, "C");
        assert_eq!(f.provenance, "inherited");
        assert_eq!(f.witness_callsite_id.as_deref(), Some("cs1"));
        assert_eq!(f.witness_operation_id.as_deref(), Some("H/op1"));
    }

    /// Use (b), singleton walk: the derived fold sees the own entry, so a
    /// physical write reaches the routine's derived row.
    #[test]
    fn singleton_walk_folds_own_entries() {
        let mut derived = ConeDerivedBuilder::default();
        derived.begin_routine();
        let out = singleton(ConeOutput::DerivedOnly, &mut derived);
        assert!(out.is_empty(), "DerivedOnly builds no raw Vec");
        derived.finish_routine("C");
        let store = derived.finish();
        assert_eq!(
            store.writes_physical_tables_of("C"),
            vec!["app/Table/1".to_string()]
        );
    }

    /// Use (a), recursive walk: the own entry is picked even though no member
    /// or downstream cone carries it.
    #[test]
    fn bfs_walk_picks_own_entries() {
        let own = [own_entry()];
        let scc: HashMap<String, usize> = [("C".to_string(), 0)].into();
        let g = empty_graph();
        let direct: RoutineDirectFacts = HashMap::new();
        let cones: HashMap<usize, ConeFacts> = HashMap::new();
        let picks = inherited_facts_by_bfs("C", &g, &direct, &scc, &cones, &own);
        assert_eq!(picks.len(), 1);
        assert_eq!(picks[0].0.witness_operation_id.as_deref(), Some("H/op1"));
        assert_eq!(picks[0].1.callsite.as_deref(), Some("cs1"));
    }

    /// #96 D5 (N4): `retag` sets `witness_first_hop_operation_id` from the
    /// edge for EVERY edge: a trigger edge's op id, and `None` on any other
    /// edge even when the representative carries a stale one.
    #[test]
    fn retag_sets_the_first_hop_op_from_the_edge() {
        let mut rep = (*own_entry().rep).clone();
        rep.witness_first_hop_operation_id = Some("stale/op7".to_string());
        let call = TypedOutEdge::new(
            "H".to_string(),
            "direct-call".to_string(),
            Some("cs1".to_string()),
            None,
            None,
        );
        assert_eq!(retag(&rep, "C", &call).witness_first_hop_operation_id, None);
        let trig = TypedOutEdge::new(
            "H".to_string(),
            "implicit-trigger".to_string(),
            None,
            None,
            Some("C/op2".to_string()),
        );
        let f = retag(&rep, "C", &trig);
        assert_eq!(f.witness_first_hop_operation_id.as_deref(), Some("C/op2"));
        assert_eq!(f.witness_callsite_id, None);
        assert_eq!(f.via, "implicit-trigger");
    }

    /// #98 D1 (A1): `retag` sets `witness_first_hop_event_id` from the edge
    /// for EVERY edge: an event-dispatch edge's event id, and `None` on any
    /// other edge even when the representative carries a stale one.
    #[test]
    fn retag_sets_the_first_hop_event_from_the_edge() {
        let mut rep = (*own_entry().rep).clone();
        rep.witness_first_hop_event_id = Some("stale/event/e".to_string());
        let call = TypedOutEdge::new(
            "H".to_string(),
            "direct-call".to_string(),
            Some("cs1".to_string()),
            None,
            None,
        );
        assert_eq!(retag(&rep, "C", &call).witness_first_hop_event_id, None);
        let ev = TypedOutEdge::new(
            "H".to_string(),
            "event-dispatch".to_string(),
            None,
            Some("app/Codeunit/1/event/onx".to_string()),
            None,
        );
        let f = retag(&rep, "C", &ev);
        assert_eq!(
            f.witness_first_hop_event_id.as_deref(),
            Some("app/Codeunit/1/event/onx")
        );
        assert_eq!(f.witness_callsite_id, None);
        assert_eq!(f.witness_first_hop_operation_id, None);
        assert_eq!(f.via, "event-dispatch");
    }

    /// #98 A4: the projection writes the first-hop event id in the stable
    /// event form, the same form the fact's own event `resourceId` uses.
    #[test]
    fn projection_writes_the_first_hop_event_in_stable_form() {
        let internal = "app/Codeunit/1/event/onx";
        let sym = EventSymbol {
            id: internal.to_string(),
            publisher_object_id: "app/Codeunit/1".to_string(),
            publisher_routine_id: None,
            publisher_stable_routine_id: None,
            event_name: "OnX".to_string(),
            event_kind: "integration".to_string(),
            element_name: None,
            signature_hash: "h1".to_string(),
            parameters: Vec::new(),
            isolated: None,
            provenance: Vec::new(),
            publisher_ref: None,
        };
        let event_by_id: HashMap<String, &EventSymbol> = [(internal.to_string(), &sym)].into();
        let mut f = (*own_entry().rep).clone();
        f.witness_first_hop_event_id = Some(internal.to_string());
        let p = project_capability_fact(&f, &HashMap::new(), &event_by_id);
        assert_eq!(
            p.witness_first_hop_event_id.as_deref(),
            Some("app:Codeunit:1::OnX::h1")
        );
        f.witness_first_hop_event_id = None;
        let p = project_capability_fact(&f, &HashMap::new(), &event_by_id);
        assert_eq!(p.witness_first_hop_event_id, None);
        assert!(
            !serde_json::to_string(&p)
                .unwrap()
                .contains("witnessFirstHopEventId")
        );
    }
}

/// #70 row 3c: a solved dependency publisher keeps UNKNOWN parameter roles.
#[cfg(test)]
mod issue70_publisher_roles_tests {
    use super::*;
    use crate::engine::l4::effect_lattice::EffectPresence;
    use crate::engine::l4::summary::FieldList;
    use std::io::Write;

    const WS_GUID: &str = "aaaa7071-0000-0000-0000-000000000070";
    const DEP_GUID: &str = "bbbb7071-0000-0000-0000-000000000070";

    const DEP_AL: &str = r#"codeunit 12 "Dep Rec Lib"
{
    [IntegrationEvent(false, false)]
    procedure OnDepRec(var Rec: Record Customer)
    begin
    end;
}

codeunit 13 "Dep Rec Sub"
{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::"Dep Rec Lib", 'OnDepRec', '', false, false)]
    local procedure HandleRec(var Rec: Record Customer)
    begin
        Rec.Modify();
    end;
}
"#;

    const WS_AL: &str = r#"codeunit 50180 "Ws Fwd"
{
    procedure Fwd(var Rec: Record Customer)
    var
        Lib: Codeunit "Dep Rec Lib";
    begin
        Lib.OnDepRec(Rec);
    end;
}
"#;

    /// The workspace (`Fwd` only) and its embedded-source dependency, written
    /// as `tests/common/symbol_app.rs` does (40-byte header + zip).
    fn write_fixture(ws: &std::path::Path) {
        std::fs::create_dir_all(ws.join("src")).unwrap();
        std::fs::write(
            ws.join("app.json"),
            format!(
                r#"{{"id":"{WS_GUID}","name":"Ws71","publisher":"probe","version":"1.0.0.0","runtime":"13.0","idRanges":[{{"from":50100,"to":50199}}],"dependencies":[{{"id":"{DEP_GUID}","name":"XRec","publisher":"probe","version":"1.0.0.0"}}]}}"#
            ),
        )
        .unwrap();
        std::fs::write(ws.join("src/Fwd.al"), WS_AL).unwrap();

        let manifest = format!(
            r#"<?xml version="1.0" encoding="utf-8"?><Package xmlns="http://schemas.microsoft.com/navx/2015/manifest"><App Id="{DEP_GUID}" Name="XRec" Publisher="probe" Version="1.0.0.0" Runtime="13.0" /></Package>"#
        );
        let rec_param = r#"{"Name":"Rec","IsVar":true,"TypeDefinition":{"Name":"Record","Subtype":{"Name":"Customer","Id":18}}}"#;
        let symbols = format!(
            r#"{{"RuntimeVersion":"13.0","Tables":[{{"Id":18,"Name":"Customer","Fields":[{{"Id":1,"Name":"No.","TypeDefinition":{{"Name":"Code"}}}}]}}],"Codeunits":[{{"Id":12,"Name":"Dep Rec Lib","Methods":[{{"Name":"OnDepRec","Parameters":[{rec_param}],"Attributes":[{{"Name":"IntegrationEvent","Arguments":[{{"Value":"False"}},{{"Value":"False"}}]}}]}}]}},{{"Id":13,"Name":"Dep Rec Sub","Methods":[]}}],"AppId":"{DEP_GUID}","Name":"XRec","Publisher":"probe","Version":"1.0.0.0"}}"#
        );
        let customer = "table 18 Customer\n{\n    fields\n    {\n        field(1; \"No.\"; Code[20])\n        {\n        }\n    }\n}\n";
        let mut bytes = std::io::Cursor::new(Vec::new());
        {
            let mut zip = zip::ZipWriter::new(&mut bytes);
            let opts = zip::write::SimpleFileOptions::default();
            for (name, text) in [
                ("NavxManifest.xml", manifest.as_str()),
                ("SymbolReference.json", symbols.as_str()),
                ("src/Customer.al", customer),
                ("src/Lib.al", DEP_AL),
            ] {
                zip.start_file(name, opts).unwrap();
                zip.write_all(text.as_bytes()).unwrap();
            }
            zip.finish().unwrap();
        }
        let pkgs = ws.join(".alpackages");
        std::fs::create_dir_all(&pkgs).unwrap();
        let mut out = std::fs::File::create(pkgs.join("probe_XRec_1.0.0.0.app")).unwrap();
        out.write_all(&[0u8; 40]).unwrap();
        out.write_all(bytes.get_ref()).unwrap();
    }

    fn routine_id(base: &R3a5CrossAppBase, name: &str) -> String {
        let ids: Vec<&ModelRoutine> = base
            .ws_routines()
            .iter()
            .filter(|r| r.name == name)
            .collect();
        assert_eq!(ids.len(), 1, "{name}: {}", ids.len());
        ids[0].id.clone()
    }

    /// The five path-sensitive role fields of a CORE role, as
    /// (requiresLoadedAtEntry, requiredLoadedFieldsAtEntry, mutatesBeforeLoad,
    /// dirtyAtExit, currentLoadedFieldsAtExit).
    fn five(
        r: &crate::engine::l4::summary::RecordRoleSummary,
    ) -> (
        EffectPresence,
        FieldList,
        EffectPresence,
        EffectPresence,
        FieldList,
    ) {
        (
            r.requires_loaded_at_entry,
            r.required_loaded_fields_at_entry.clone(),
            r.mutates_before_load,
            r.dirty_at_exit,
            r.current_loaded_fields_at_exit.clone(),
        )
    }

    /// The same five fields of a PROJECTED role (`project_r3a5_cross_app`).
    fn five_projected(r: &crate::engine::l4::summary::PRecordRoleSummary) -> [String; 5] {
        let v = serde_json::to_value(r).unwrap();
        [
            "requiresLoadedAtEntry",
            "requiredLoadedFieldsAtEntry",
            "mutatesBeforeLoad",
            "dirtyAtExit",
            "currentLoadedFieldsAtExit",
        ]
        .map(|k| v[k].as_str().unwrap_or("<not a string>").to_string())
    }

    /// Row 3c. A dependency publisher `OnDepRec(var Rec: Record Customer)` whose
    /// dependency subscriber `HandleRec` does `Rec.Modify()`, and a workspace
    /// forwarder `Fwd(var Rec)` whose body is ONLY `Lib.OnDepRec(Rec)`.
    /// Hand-stated expectations, checked through the production paths that
    /// expose each fact:
    ///   (i)   DB rows, through the store `build_l4` builds cross-app:
    ///         `OnDepRec` has its subscriber's `Modify` row;
    ///   (ii)  parameter roles, through the detector context analyze uses
    ///         (`build_detector_context_with` + the base's cross-app inputs,
    ///         `ctx.parameter_roles_by_routine`);
    ///   (iii) roles, through `project_r3a5_cross_app`.
    /// In (ii) and (iii): `OnDepRec`'s five path-sensitive role fields are all
    /// unknown; `Fwd`'s entry fields (`requiresLoadedAtEntry`,
    /// `requiredLoadedFieldsAtEntry`, `mutatesBeforeLoad`) are unknown, its exit
    /// fields keep the walker's behaviour (`dirtyAtExit: no`,
    /// `currentLoadedFieldsAtExit: full`).
    ///
    /// Mutations: (a) the solver writes back roles for `fixed_roles` members ->
    /// concrete roles -> fails; (b) an EMPTY `fixed_roles` only at the
    /// detector-context call -> (ii) fails while (iii) passes.
    ///
    /// (ii) takes its inputs from `base.cross_app_inputs()`, the one
    /// constructor every cross-app caller uses, so mutation (b) reaches it.
    ///
    /// (i) since #160 Task 27 (2026-10-19): `build_l4` called the cross-app
    /// way. Removing `edges_by_from` for the `fixed_roles` members inside
    /// `build_l4` fails (i) (`[]`); restored, it passes. Stated limit: passing
    /// `None` instead of the cross-app inputs to (i)'s `build_l4` stays GREEN.
    /// Fixed roles change roles only, and the one leaf (`HandleRec`, no
    /// out-edge) equals its solved direct rows, so the rows do not see it.
    #[test]
    fn dependency_publisher_roles_stay_unknown() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().join("ws");
        write_fixture(&ws);
        let inputs = crate::program::inputs::InputSet::read(&ws, &Default::default());
        let base = build_cross_app_base(&inputs, "r0").expect("cross-app base");
        let publisher = routine_id(&base, "OnDepRec");
        let fwd = routine_id(&base, "Fwd");
        assert!(
            base.dep_routine_ids.contains(&publisher),
            "precondition: OnDepRec is a dependency routine"
        );
        let mut failures: Vec<String> = Vec::new();

        // (i) DB rows through the store, solved the cross-app way (the base's
        // relation and cross-app inputs, so the leaves and fixed roles are in).
        let (l4, _) = crate::engine::l4::analysis::build_l4(
            &base.resolved,
            &base.rel,
            crate::engine::l4::analysis::L4Request {
                extra_diagnostics: vec![],
            },
            Some(&base.cross_app_inputs()),
        );
        let bundle = l4.bundle;
        let ops: Vec<String> = bundle
            .routine_ix(&publisher)
            .map(|ix| bundle.db_effects(ix).map(|e| e.op.to_string()).collect())
            .unwrap_or_default();
        if !ops.iter().any(|op| op.eq_ignore_ascii_case("modify")) {
            failures.push(format!(
                "(i) OnDepRec has no subscriber Modify row in the store: {ops:?}"
            ));
        }

        let all_unknown = (
            EffectPresence::Unknown,
            FieldList::Unknown,
            EffectPresence::Unknown,
            EffectPresence::Unknown,
            FieldList::Unknown,
        );
        let fwd_want = (
            EffectPresence::Unknown,
            FieldList::Unknown,
            EffectPresence::Unknown,
            EffectPresence::No,
            FieldList::Full,
        );

        // (ii) The detector context analyze uses.
        let owned = crate::engine::l5::detector_context::build_detector_context_with(
            &base.resolved,
            crate::engine::l5::registry::substrate::ALL,
            Some(&base),
        );
        let ctx = owned.ctx();
        for (name, id, want) in [
            ("OnDepRec", &publisher, &all_unknown),
            ("Fwd", &fwd, &fwd_want),
        ] {
            match ctx.parameter_roles_by_routine.get(id).map(|v| v.as_slice()) {
                Some([role]) => {
                    if &five(role) != want {
                        failures.push(format!("(ii) {name}: want {want:?}, got {:?}", five(role)));
                    }
                }
                other => failures.push(format!("(ii) {name}: want one role, got {other:?}")),
            }
        }

        // (iii) project_r3a5_cross_app.
        let proj = project_r3a5_cross_app(&ws, "r0", "issue70");
        let u = "unknown".to_string();
        for (name, want) in [
            (
                "OnDepRec",
                [u.clone(), u.clone(), u.clone(), u.clone(), u.clone()],
            ),
            (
                "Fwd",
                [
                    u.clone(),
                    u.clone(),
                    u.clone(),
                    "no".to_string(),
                    "full".to_string(),
                ],
            ),
        ] {
            let stable = base
                .ws_routines()
                .iter()
                .find(|r| r.name == name)
                .map(|r| r.stable_routine_id.clone())
                .unwrap();
            match proj.summaries.iter().find(|s| s.routine_id == stable) {
                Some(s) => match s.parameter_roles.as_slice() {
                    [role] => {
                        if five_projected(role) != want {
                            failures.push(format!(
                                "(iii) {name}: want {want:?}, got {:?}",
                                five_projected(role)
                            ));
                        }
                    }
                    other => failures.push(format!("(iii) {name}: want one role, got {other:?}")),
                },
                None => failures.push(format!("(iii) {name}: no projected summary")),
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }

    /// #160 Task 27 review I1: cross-app `alsem query` keeps the model, the
    /// graph and the `l4` analysis, and drops the rest of the base before the
    /// query and the render. The base holds the relation (`base.rel`), which
    /// owns the graph and the SCC, so a live base (or view cell) shows as a
    /// second owner of each (#160 step 4).
    #[test]
    fn cross_app_query_drops_the_base() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().join("ws");
        write_fixture(&ws);
        let inputs = crate::program::inputs::InputSet::read(&ws, &Default::default());
        let q = crate::engine::l4::effect_query_cli::QuerySubstrate::build(&inputs, "r0", false)
            .expect("cross-app query substrate");
        assert_eq!(q.shared_owners(), (1, 1), "(graph, scc) owners");
    }

    /// #160 Task 27 review #2: the cross-app shim solves over the base's
    /// relation, so it takes the relation's SCC instead of running a second
    /// Tarjan.
    #[test]
    fn cross_app_shim_reuses_the_base_scc() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().join("ws");
        write_fixture(&ws);
        let inputs = crate::program::inputs::InputSet::read(&ws, &Default::default());
        let base = build_cross_app_base(&inputs, "r0").expect("cross-app base");
        let owned = crate::engine::l5::detector_context::build_detector_context_with(
            &base.resolved,
            crate::engine::l5::registry::substrate::CORE_SUMMARIES,
            Some(&base),
        );
        let l4 = owned.l4().expect("CORE_SUMMARIES solves");
        assert!(
            std::sync::Arc::ptr_eq(&l4.scc, base.rel.scc()),
            "the shim computed its own SCC"
        );
    }

    /// #160 final panel P1-1: (ii) through the `View`, the path every command
    /// takes since step 2. A cross-app `View` prepared with a plan whose probe
    /// demands `CORE_SUMMARIES` keeps `OnDepRec`'s five path-sensitive role
    /// fields `unknown` in `view.det()`, as (ii) does through the shim.
    ///
    /// DISCRIMINATION: passing `None` instead of `target.cross_app_inputs()`
    /// to the det fill in `View::prepare` fails this, while the shim-path
    /// tests above stay green.
    #[test]
    fn dependency_publisher_roles_stay_unknown_through_the_view() {
        use crate::engine::l5::registry::{Detector, DetectorOutput, DetectorStats, substrate};
        use crate::engine::session::{Plan, View};
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().join("ws");
        write_fixture(&ws);
        let inputs = crate::program::inputs::InputSet::read(&ws, &Default::default());
        let base = build_cross_app_base(&inputs, "r0").expect("cross-app base");
        let publisher = routine_id(&base, "OnDepRec");
        let probe = Detector {
            name: "probe".to_string(),
            run: |_, _| {
                Ok(DetectorOutput::no_diag(
                    vec![],
                    DetectorStats::new("probe", 0, 0),
                ))
            },
            requires: substrate::CORE_SUMMARIES,
            rule_name: "Probe",
            short_description: "test probe",
        };
        let mut view = View::new(crate::engine::gate::run::AnalysisTarget::CrossApp(
            Box::new(base),
        ));
        view.prepare(&Plan::for_test(vec![vec![probe]], true, false, 0));
        let all_unknown = (
            EffectPresence::Unknown,
            FieldList::Unknown,
            EffectPresence::Unknown,
            EffectPresence::Unknown,
            FieldList::Unknown,
        );
        match view
            .det()
            .parameter_roles_by_routine
            .get(&publisher)
            .map(|v| v.as_slice())
        {
            Some([role]) => assert_eq!(five(role), all_unknown, "OnDepRec through the View"),
            other => panic!("OnDepRec: want one role, got {other:?}"),
        }
    }

    // -----------------------------------------------------------------------
    // #107: a dependency publisher's presence facts compose its subscribers.
    // -----------------------------------------------------------------------

    const DEP_AL_107: &str = r#"codeunit 12 "Dep Rec Lib"
{
    [IntegrationEvent(false, false)]
    procedure OnDepRec(var Rec: Record Customer)
    begin
    end;

    [IntegrationEvent(false, false)]
    procedure OnDepRec2(var Rec: Record Customer)
    begin
    end;
}

codeunit 13 "Dep Rec Sub"
{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::"Dep Rec Lib", 'OnDepRec', '', false, false)]
    local procedure HandleRec(var Rec: Record Customer)
    begin
        Rec.Modify();
    end;
}
"#;

    const WS_AL_107: &str = r#"codeunit 50180 "Ws Fwd"
{
    procedure Fwd(var Rec: Record Customer)
    var
        Lib: Codeunit "Dep Rec Lib";
    begin
        Lib.OnDepRec(Rec);
    end;

    procedure Fwd2(var Rec: Record Customer)
    var
        Lib: Codeunit "Dep Rec Lib";
    begin
        Lib.OnDepRec2(Rec);
    end;

    [EventSubscriber(ObjectType::Codeunit, Codeunit::"Dep Rec Lib", 'OnDepRec2', '', false, false)]
    local procedure HandleWs(var Rec: Record Customer)
    begin
        Rec.Modify();
    end;
}
"#;

    /// The `probes/gen/mkdep.py` fixture: `OnDepRec` (dependency subscriber
    /// `Modify`), `OnDepRec2` (WORKSPACE subscriber `HandleWs` doing `Modify`),
    /// workspace forwarders `Fwd`/`Fwd2`. `with_src = false` is the symbol-only
    /// dependency (`ws-dep-sym`): no embedded source, so both publishers are
    /// bodyless.
    fn write_fixture_107(ws: &std::path::Path, with_src: bool) {
        std::fs::create_dir_all(ws.join("src")).unwrap();
        std::fs::write(
            ws.join("app.json"),
            format!(
                r#"{{"id":"{WS_GUID}","name":"Ws107","publisher":"probe","version":"1.0.0.0","runtime":"13.0","idRanges":[{{"from":50100,"to":50199}}],"dependencies":[{{"id":"{DEP_GUID}","name":"XRec","publisher":"probe","version":"1.0.0.0"}}]}}"#
            ),
        )
        .unwrap();
        std::fs::write(ws.join("src/Fwd.al"), WS_AL_107).unwrap();

        let manifest = format!(
            r#"<?xml version="1.0" encoding="utf-8"?><Package xmlns="http://schemas.microsoft.com/navx/2015/manifest"><App Id="{DEP_GUID}" Name="XRec" Publisher="probe" Version="1.0.0.0" Runtime="13.0" /></Package>"#
        );
        let rec_param = r#"{"Name":"Rec","IsVar":true,"TypeDefinition":{"Name":"Record","Subtype":{"Name":"Customer","Id":18}}}"#;
        let ev =
            r#"[{"Name":"IntegrationEvent","Arguments":[{"Value":"False"},{"Value":"False"}]}]"#;
        let symbols = format!(
            r#"{{"RuntimeVersion":"13.0","Tables":[{{"Id":18,"Name":"Customer","Fields":[{{"Id":1,"Name":"No.","TypeDefinition":{{"Name":"Code"}}}}]}}],"Codeunits":[{{"Id":12,"Name":"Dep Rec Lib","Methods":[{{"Name":"OnDepRec","Parameters":[{rec_param}],"Attributes":{ev}}},{{"Name":"OnDepRec2","Parameters":[{rec_param}],"Attributes":{ev}}}]}},{{"Id":13,"Name":"Dep Rec Sub","Methods":[]}}],"AppId":"{DEP_GUID}","Name":"XRec","Publisher":"probe","Version":"1.0.0.0"}}"#
        );
        let customer = "table 18 Customer\n{\n    fields\n    {\n        field(1; \"No.\"; Code[20])\n        {\n        }\n    }\n}\n";
        let mut entries: Vec<(&str, &str)> = vec![
            ("NavxManifest.xml", manifest.as_str()),
            ("SymbolReference.json", symbols.as_str()),
        ];
        if with_src {
            entries.push(("src/Customer.al", customer));
            entries.push(("src/Lib.al", DEP_AL_107));
        }
        let mut bytes = std::io::Cursor::new(Vec::new());
        {
            let mut zip = zip::ZipWriter::new(&mut bytes);
            let opts = zip::write::SimpleFileOptions::default();
            for (name, text) in entries {
                zip.start_file(name, opts).unwrap();
                zip.write_all(text.as_bytes()).unwrap();
            }
            zip.finish().unwrap();
        }
        let pkgs = ws.join(".alpackages");
        std::fs::create_dir_all(&pkgs).unwrap();
        let mut out = std::fs::File::create(pkgs.join("probe_XRec_1.0.0.0.app")).unwrap();
        out.write_all(&[0u8; 40]).unwrap();
        out.write_all(bytes.get_ref()).unwrap();
    }

    /// One role field of an INTERNAL role, by its projected JSON key, in the
    /// projection's spelling (`yes`/`no`/`unknown`; a field list `unknown`/`full`).
    fn internal_fact(r: &crate::engine::l4::summary::RecordRoleSummary, key: &str) -> String {
        let field_list = |l: &FieldList| match l {
            FieldList::Unknown => "unknown".to_string(),
            FieldList::Full => "full".to_string(),
            FieldList::Known(v) => format!("{v:?}"),
        };
        let p = |e: EffectPresence| format!("{e:?}").to_lowercase();
        match key {
            "persistsCurrentRecord" => p(r.persists_current_record),
            "validatesParam" => p(r.validates_param),
            "copiesIntoParam" => p(r.copies_into_param),
            "resetsFiltersOnParam" => p(r.resets_filters_on_param),
            "loadsFromDbParam" => p(r.loads_from_db_param),
            "initialisesParam" => p(r.initialises_param),
            "mutatesParam" => p(r.mutates_param),
            "requiresLoadedAtEntry" => p(r.requires_loaded_at_entry),
            "requiredLoadedFieldsAtEntry" => field_list(&r.required_loaded_fields_at_entry),
            "mutatesBeforeLoad" => p(r.mutates_before_load),
            "dirtyAtExit" => p(r.dirty_at_exit),
            "currentLoadedFieldsAtExit" => field_list(&r.current_loaded_fields_at_exit),
            other => panic!("unknown key {other}"),
        }
    }

    /// One role field of a PROJECTED role, by its JSON key.
    fn projected_fact(r: &crate::engine::l4::summary::PRecordRoleSummary, key: &str) -> String {
        serde_json::to_value(r).unwrap()[key]
            .as_str()
            .unwrap_or("<not a string>")
            .to_string()
    }

    /// `(routine name, source, json key -> value)`.
    type RoleRow = (String, &'static str, Vec<(String, String)>);

    /// Each named routine's single role, through the detector context
    /// (`ctx.parameter_roles_by_routine`) and through `project_r3a5_cross_app`,
    /// as `(name, source, json key -> value)` for the keys asked.
    fn roles_both_paths(
        ws: &std::path::Path,
        base: &R3a5CrossAppBase,
        names: &[&str],
        keys: &[&str],
    ) -> Vec<RoleRow> {
        let owned = crate::engine::l5::detector_context::build_detector_context_with(
            &base.resolved,
            crate::engine::l5::registry::substrate::ALL,
            Some(base),
        );
        let ctx = owned.ctx();
        let proj = project_r3a5_cross_app(ws, "r0", "issue107");
        let mut out = Vec::new();
        for name in names {
            let id = routine_id(base, name);
            match ctx
                .parameter_roles_by_routine
                .get(&id)
                .map(|v| v.as_slice())
            {
                Some([role]) => {
                    out.push((
                        name.to_string(),
                        "ctx",
                        keys.iter()
                            .map(|k| (k.to_string(), internal_fact(role, k)))
                            .collect(),
                    ));
                }
                other => out.push((
                    name.to_string(),
                    "ctx",
                    vec![("<roles>".to_string(), format!("{other:?}"))],
                )),
            }
            let stable = base
                .ws_routines()
                .iter()
                .find(|r| &r.name == name)
                .map(|r| r.stable_routine_id.clone())
                .unwrap();
            match proj
                .summaries
                .iter()
                .find(|s| s.routine_id == stable)
                .map(|s| s.parameter_roles.as_slice())
            {
                Some([role]) => out.push((
                    name.to_string(),
                    "r3a5",
                    keys.iter()
                        .map(|k| (k.to_string(), projected_fact(role, k)))
                        .collect(),
                )),
                other => out.push((
                    name.to_string(),
                    "r3a5",
                    vec![("<roles>".to_string(), format!("{other:?}"))],
                )),
            }
        }
        out
    }

    /// #107 acceptance item 2 ("... and dependency publishers alike"), cross-app.
    /// `OnDepRec` (dependency subscriber `Modify`) and `OnDepRec2` (WORKSPACE
    /// subscriber `HandleWs` doing `Modify`) are source-bearing `fixed_roles`
    /// publishers: their presence facts compose the subscriber
    /// (`persistsCurrentRecord yes`, `mutatesParam yes`) while their five
    /// path-sensitive fields stay `unknown` (#70 row 3c unchanged); the
    /// forwarders `Fwd`/`Fwd2` inherit `persistsCurrentRecord yes` and keep
    /// `dirtyAtExit no` (Persisted is not dirty). Checked through both the
    /// detector context and `project_r3a5_cross_app`.
    ///
    /// Mutation to record at implementation (with #70's two): "skip the
    /// dispatch fold for `fixed_roles` members" -> this test fails.
    #[test]
    fn issue107_dependency_publisher_presence_composes_subscribers() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().join("ws");
        write_fixture_107(&ws, true);
        let inputs = crate::program::inputs::InputSet::read(&ws, &Default::default());
        let base = build_cross_app_base(&inputs, "r0").expect("cross-app base");
        for p in ["OnDepRec", "OnDepRec2"] {
            assert!(
                base.dep_routine_ids.contains(&routine_id(&base, p)),
                "precondition: {p} is a dependency routine"
            );
            assert!(
                base.fixed_roles.contains(&routine_id(&base, p)),
                "precondition: {p} is a fixed_roles publisher"
            );
        }
        let keys = [
            "persistsCurrentRecord",
            "mutatesParam",
            "requiresLoadedAtEntry",
            "requiredLoadedFieldsAtEntry",
            "mutatesBeforeLoad",
            "dirtyAtExit",
            "currentLoadedFieldsAtExit",
        ];
        let want_pub = [
            "yes", "yes", "unknown", "unknown", "unknown", "unknown", "unknown",
        ];
        let mut failures = Vec::new();
        for (name, src, got) in
            roles_both_paths(&ws, &base, &["OnDepRec", "OnDepRec2", "Fwd", "Fwd2"], &keys)
        {
            let is_pub = name.starts_with("On");
            for (i, (k, v)) in got.iter().enumerate() {
                let want = if is_pub {
                    want_pub.get(i).copied()
                } else {
                    match k.as_str() {
                        "persistsCurrentRecord" | "mutatesParam" => Some("yes"),
                        "dirtyAtExit" => Some("no"),
                        _ => None,
                    }
                };
                if k == "<roles>" {
                    failures.push(format!("({src}) {name}: want one role, got {v}"));
                } else if let Some(w) = want
                    && v != w
                {
                    failures.push(format!("({src}) {name}.{k}: want {w}, got {v}"));
                }
            }
        }
        assert!(failures.is_empty(), "\n{}", failures.join("\n"));
    }

    /// #107 row 8 (Design 6): a symbol-only (bodyless) dependency publisher is
    /// skipped by both composition steps, so its own role and its callers'
    /// roles stay what base gives (pinned from the base `8405f96d` output,
    /// probe b2): the publishers carry their all-`no` presence facts, and the
    /// forwarders see an opaque callee (presence `unknown`).
    #[test]
    fn issue107_symbol_only_dependency_publisher_roles_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().join("ws");
        write_fixture_107(&ws, false);
        let inputs = crate::program::inputs::InputSet::read(&ws, &Default::default());
        let base = build_cross_app_base(&inputs, "r0").expect("cross-app base");
        let keys = [
            "persistsCurrentRecord",
            "validatesParam",
            "copiesIntoParam",
            "resetsFiltersOnParam",
            "loadsFromDbParam",
            "initialisesParam",
            "mutatesParam",
        ];
        let mut failures = Vec::new();
        for (name, src, got) in
            roles_both_paths(&ws, &base, &["OnDepRec", "OnDepRec2", "Fwd", "Fwd2"], &keys)
        {
            let want = if name.starts_with("On") {
                "no"
            } else {
                "unknown"
            };
            for (k, v) in got {
                if v != want {
                    failures.push(format!("({src}) {name}.{k}: want {want}, got {v}"));
                }
            }
        }
        assert!(failures.is_empty(), "\n{}", failures.join("\n"));
    }
}

/// #160 step 4 (spec §5.2-§5.4): the epic row-4 behaviours the one
/// `ExecRelation` must keep, pinned on the base code before it exists.
#[cfg(test)]
mod step4_tests {
    use super::*;
    use crate::engine::gate::run::AnalysisTarget;
    use crate::engine::l5::detectors::registered_detectors;
    use crate::engine::l5::registry::Detector;
    use crate::engine::l5::test_support::{
        dependency_half, write_fixture_dep_uncertainty, write_fixture_step4,
    };
    use crate::engine::session::{View, analyze_plan, digest_plan};
    use std::path::Path;

    fn id_of(base: &R3a5CrossAppBase, name: &str) -> String {
        let ids: Vec<&ModelRoutine> = base
            .ws_routines()
            .iter()
            .filter(|r| r.name == name)
            .collect();
        assert_eq!(ids.len(), 1, "{name}: {}", ids.len());
        ids[0].id.clone()
    }

    fn detector(name: &str) -> Detector {
        registered_detectors()
            .into_iter()
            .find(|d| d.name == name)
            .unwrap_or_else(|| panic!("no detector {name}"))
    }

    fn base_of(ws: &Path) -> R3a5CrossAppBase {
        let inputs = crate::program::inputs::InputSet::read(ws, &Default::default());
        build_cross_app_base(&inputs, "r0").expect("cross-app base")
    }

    /// Spec §5.2: the Primary payload takes WORKSPACE callers only. A
    /// dependency caller (`Post`) of a dependency publisher (`OnDepPost`)
    /// stays out of the subscriber's reverse graph and its commit span, and a
    /// dependency `local` helper (`Touch`) with only a dependency caller is
    /// never proven closed-world temporary. The preconditions read immutable
    /// inputs only (model rows, the model's event graph, an independent
    /// [`dependency_half`]), so a break inside the relation cannot fail them.
    #[test]
    fn step4_dependency_publisher_primary_rule() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().join("ws");
        write_fixture_step4(&ws);
        let base = base_of(&ws);
        let [on_dep_post, post, call_touch, touch, handle_ws] =
            ["OnDepPost", "Post", "CallTouch", "Touch", "HandleWs"].map(|n| id_of(&base, n));
        for (name, id) in [
            ("OnDepPost", &on_dep_post),
            ("Post", &post),
            ("CallTouch", &call_touch),
            ("Touch", &touch),
        ] {
            assert!(
                base.dep_routine_ids.contains(id),
                "precondition: {name} is a dependency routine"
            );
        }
        let t = base.ws_routines().iter().find(|r| r.id == touch).unwrap();
        assert_eq!(t.kind, "procedure", "precondition: Touch's kind");
        assert_eq!(
            t.access_modifier.as_deref(),
            Some("local"),
            "precondition: Touch is local"
        );
        let r = t
            .record_variables
            .iter()
            .find(|v| v.is_parameter && v.name == "R")
            .expect("precondition: Touch has the record parameter R");
        assert_eq!(
            r.temp_state.kind, "parameter-dependent",
            "precondition: R's temp state"
        );
        let eg = &base.resolved.events.graph;
        let evt = eg
            .events
            .iter()
            .find(|e| e.publisher_routine_id.as_deref() == Some(on_dep_post.as_str()))
            .expect("precondition: an event published by OnDepPost");
        assert!(
            eg.edges
                .iter()
                .any(|e| e.event_id == evt.id && e.subscriber_routine_id == handle_ws),
            "precondition: OnDepPost's event reaches HandleWs"
        );
        let dep = dependency_half(&ws);
        for (from, to) in [(&post, &on_dep_post), (&call_touch, &touch)] {
            assert!(
                dep.iter()
                    .any(|e| &e.from == from && e.to.as_deref() == Some(to.as_str())),
                "precondition: the dependency half holds {from} -> {to}"
            );
        }

        let mut view = View::new(AnalysisTarget::CrossApp(Box::new(base)));
        view.prepare(&digest_plan(
            crate::engine::l5::detectors::default_detectors(),
        ));
        let det = view.det();
        let name = |id: &str| -> String {
            let m = view.model();
            m.workspace
                .routines
                .iter()
                .find(|r| r.id == id)
                .map_or_else(|| id.to_string(), |r| r.name.clone())
        };
        // All three are checked and reported together, so a break shows
        // which of them it moves.
        let mut failures: Vec<String> = Vec::new();
        // (1)
        let callers: Vec<(String, String)> = det
            .reverse_call_graph
            .get(&handle_ws)
            .map(|v| v.iter().map(|e| (e.kind.clone(), name(&e.from))).collect())
            .unwrap_or_default();
        let want = vec![("event-dispatch".to_string(), "OnDepPost".to_string())];
        if callers != want {
            failures.push(format!(
                "(1) HandleWs's reverse callers: want {want:?}, got {callers:?}"
            ));
        }
        // (2)
        let roots: Vec<Vec<String>> = det
            .transaction_spans
            .iter()
            .filter(|s| s.commit_routine_id == handle_ws)
            .map(|s| s.span_roots.iter().map(|r| name(r)).collect())
            .collect();
        if roots != vec![vec!["OnDepPost".to_string()]] {
            failures.push(format!(
                "(2) HandleWs's commit span roots: want [[OnDepPost]], got {roots:?}"
            ));
        }
        // (3)
        let proven: Vec<&(String, u32)> = det
            .closed_world_temp_params
            .iter()
            .filter(|(id, _)| id == &touch)
            .collect();
        if !proven.is_empty() {
            failures.push(format!("(3) Touch proven closed-world temp: {proven:?}"));
        }
        assert!(failures.is_empty(), "\n{}", failures.join("\n"));
    }

    /// Spec §5.3: when two workspace callers reach one dependency, d17's
    /// sample is the FIRST caller in call-emission order (`edges_from_order`).
    /// The precondition reads the call list handed to the graph build, not
    /// the graph, so reversing `edges_from_order` there leaves it true.
    #[test]
    fn step4_from_order_picks_the_first_caller() {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/r0-corpus/ws-d17-drift");
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().join("ws");
        std::fs::create_dir_all(ws.join("src")).unwrap();
        std::fs::create_dir_all(ws.join(".alpackages")).unwrap();
        std::fs::copy(src.join("app.json"), ws.join("app.json")).unwrap();
        for p in std::fs::read_dir(src.join(".alpackages")).unwrap() {
            let p = p.unwrap().path();
            std::fs::copy(&p, ws.join(".alpackages").join(p.file_name().unwrap())).unwrap();
        }
        for (file, id, name) in [("A.al", 50171, "A"), ("B.al", 50172, "B")] {
            std::fs::write(
                ws.join("src").join(file),
                format!(
                    "codeunit {id} {name}\n{{\n    procedure Do{name}()\n    begin\n        Codeunit.Run(Codeunit::D17DepNewer);\n    end;\n}}\n"
                ),
            )
            .unwrap();
        }
        let base = base_of(&ws);
        let (a, b) = (id_of(&base, "DoA"), id_of(&base, "DoB"));
        let first = |id: &str| base.resolved.calls.edges.iter().position(|e| e.from == id);
        let (fa, fb) = (first(&a), first(&b));
        assert!(
            matches!((fa, fb), (Some(x), Some(y)) if x < y),
            "precondition: A's first edge before B's: {fa:?} {fb:?}"
        );
        let a_object = base
            .ws_routines()
            .iter()
            .find(|r| r.id == a)
            .unwrap()
            .object_id
            .clone();

        let plan = analyze_plan(vec![detector("d17-min-version-drift")], true);
        let key = plan.keys().remove(0);
        let mut view = View::new(AnalysisTarget::CrossApp(Box::new(base)));
        view.prepare(&plan);
        let out = view.run(&key);
        let got: Vec<(&str, Vec<&str>, Vec<&str>)> = out
            .findings
            .iter()
            .map(|f| {
                (
                    f.id.as_str(),
                    f.evidence_path
                        .iter()
                        .map(|s| s.routine_id.as_str())
                        .collect(),
                    f.affected_objects.iter().map(|o| &**o).collect(),
                )
            })
            .collect();
        assert_eq!(
            got,
            vec![(
                "d17/dddddddd-0000-0000-0000-0000000000d17",
                vec![a.as_str()],
                vec![a_object.as_str()],
            )],
            "the d17 sample is A (B is {b})"
        );
    }

    /// Spec §5.4: a dependency routine's to-less call is kept, apart from
    /// the Primary payload. (1) `rel.dependency_uncertainty` holds exactly
    /// one edge from `DepCaller`, the `unresolved-call` its to-less edge
    /// maps to (observed on the base `c554b704`), at that edge's call site;
    /// (2) neither `rel.primary().uncertainty_edges` nor the detector
    /// context (a plan holding d1, so the context is built) carries it.
    #[test]
    fn step4_dependency_uncertainty_stays_out_of_primary() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().join("ws");
        write_fixture_dep_uncertainty(&ws);
        let base = base_of(&ws);
        let dep_caller = id_of(&base, "DepCaller");
        assert!(
            base.dep_routine_ids.contains(&dep_caller),
            "precondition: DepCaller is a dependency routine"
        );
        let dep = dependency_half(&ws);
        let to_less: Vec<&crate::program::model::calls::CallEdge> = dep
            .iter()
            .filter(|e| e.from == dep_caller && e.to.is_none())
            .collect();
        assert_eq!(
            to_less.len(),
            1,
            "precondition: the dependency half holds one to-less edge from DepCaller: {dep:?}"
        );
        // All checked and reported together.
        let mut failures: Vec<String> = Vec::new();
        // (1)
        let kept: Vec<(&str, &str, Option<&str>)> = base
            .rel
            .dependency_uncertainty()
            .iter()
            .map(|u| {
                (
                    u.from.as_str(),
                    u.uncertainty.kind.as_str(),
                    u.uncertainty.callsite_id.as_deref(),
                )
            })
            .collect();
        let want = vec![(
            dep_caller.as_str(),
            "unresolved-call",
            Some(to_less[0].callsite_id.as_str()),
        )];
        if kept != want {
            failures.push(format!(
                "(1) rel.dependency_uncertainty: want {want:?}, got {kept:?}"
            ));
        }
        // (2)
        let in_graph: Vec<&crate::engine::l4::combined_graph::UncertaintyEdge> = base
            .rel
            .primary()
            .uncertainty_edges
            .iter()
            .filter(|u| u.from == dep_caller)
            .collect();
        if !in_graph.is_empty() {
            failures.push(format!(
                "(2) rel.primary().uncertainty_edges from DepCaller: {in_graph:?}"
            ));
        }
        let mut view = View::new(AnalysisTarget::CrossApp(Box::new(base)));
        view.prepare(&analyze_plan(
            vec![detector(crate::engine::l5::detectors::d1::DETECTOR)],
            true,
        ));
        if let Some(u) = view.det().uncertainty_edges_by_from.get(&dep_caller) {
            failures.push(format!(
                "(2) context uncertainty_edges_by_from[DepCaller]: {u:?}"
            ));
        }
        assert!(failures.is_empty(), "\n{}", failures.join("\n"));
    }
}

/// #160 steps 3 and 5: the coverage reasons come from a scan of uncertainty
/// edges ([`callsite_local_coverage_reasons`]), with no L4 solve; the snapshot
/// base folds it into direct coverage and prints no roles-cap line. The other
/// producers' folds are pinned in `detector_context_step5_tests`.
#[cfg(test)]
mod callsite_local_scan_tests {
    use super::*;
    use crate::engine::l4::combined_graph::{PrimaryGraph, Uncertainty};
    use crate::engine::l4::summary_runner::with_roles_iteration_cap;
    use crate::program::model::program_calls::{
        assemble_and_resolve_inline_program_default, assemble_and_resolve_workspace_program,
    };
    use std::path::{Path, PathBuf};

    const APP: &str = "11111111-0000-0000-0000-000000160003";

    /// `table` + `src`, the shape of `summary_runner`'s private test `model`.
    pub(super) fn model(src: &str) -> Model {
        let table = "table 50100 Cust\n{\n    fields\n    {\n        field(1; \"No.\"; Code[20]) { }\n    }\n    keys { key(PK; \"No.\") { } }\n}\n";
        assemble_and_resolve_inline_program_default(
            &[
                ("src/Tab.al".to_string(), table.to_string()),
                ("src/Code.al".to_string(), src.to_string()),
            ],
            APP,
        )
    }

    /// [`model`] with one table per name instead of `Cust`, in the same text
    /// shape, ids 50101.. (#160 step 7a §5.4).
    pub(super) fn model_with_tables(names: &[&str], src: &str) -> Model {
        let mut files: Vec<(String, String)> = names
            .iter()
            .enumerate()
            .map(|(i, name)| {
                (
                    format!("src/Tab{}.al", i + 1),
                    format!(
                        "table {} {name}\n{{\n    fields\n    {{\n        field(1; \"No.\"; Code[20]) {{ }}\n    }}\n    keys {{ key(PK; \"No.\") {{ }} }}\n}}\n",
                        50101 + i
                    ),
                )
            })
            .collect();
        files.push(("src/Code.al".to_string(), src.to_string()));
        assemble_and_resolve_inline_program_default(&files, APP)
    }

    pub(super) fn workspace_model(dir: &Path) -> Option<Model> {
        assemble_and_resolve_workspace_program(
            &crate::program::inputs::InputSet::read(dir, &Default::default()),
            crate::program::model::workspace::MODEL_INSTANCE_ID_DEFAULT,
            false,
        )
    }

    /// The scan's borrowed map, owned, to compare with a literal map.
    fn owned(m: HashMap<&str, BTreeSet<&str>>) -> HashMap<String, BTreeSet<String>> {
        m.into_iter()
            .map(|(k, v)| (k.to_string(), v.into_iter().map(str::to_string).collect()))
            .collect()
    }

    /// A copy of `summary_runner`'s `CYC_SRC` (the SCC `{OnCyc, HandleCyc}`
    /// caps at one roles round) whose `HandleCyc` also makes an
    /// `external-target` call (the `ws-r2b-external` shape) and a
    /// `member-not-found` call. Neither adds a resolved edge.
    /// [`snapshot_base_prints_no_capped_line`] builds the base on it.
    const CAP_SRC: &str = r#"
codeunit 50103 "Cyc Other"
{
    procedure Known()
    begin
    end;
}

codeunit 50102 Cyc
{
    var
        Ext: Codeunit "External Dep Helper";
        Other: Codeunit "Cyc Other";

    [IntegrationEvent(false, false)]
    procedure OnCyc(var Rec: Record Cust; Depth: Integer)
    begin
    end;

    [EventSubscriber(ObjectType::Codeunit, Codeunit::Cyc, 'OnCyc', '', false, false)]
    local procedure HandleCyc(var Rec: Record Cust; Depth: Integer)
    begin
        if Depth > 0 then
            OnCyc(Rec, Depth - 1);
        Ext.RunIt();
        Other.Missing();
        Rec.Modify();
    end;
}
"#;

    /// Each of the four kinds in a non-recursive routine (`Plain`), a
    /// self-loop (`Loop`) and a two-member cycle (`Ping`/`Pong`), so the fold is
    /// checked in every SCC shape without depending on corpus luck.
    const CELLS_SRC: &str = r#"
interface "Cell IFace"
{
    procedure Touch();
}

codeunit 50110 "Cell Target"
{
    procedure P(N: Integer): Integer
    begin
        exit(1);
    end;

    procedure P(S: Text): Integer
    begin
        exit(0);
    end;
}

codeunit 50111 "Cell Calls"
{
    var
        Ext: Codeunit "External Dep Helper";
        Face: Interface "Cell IFace";

    procedure Plain(Depth: Integer; C: Char)
    var
        T: Codeunit "Cell Target";
    begin
        Ext.RunIt();
        T.Missing();
        T.P(C);
        Face.Touch();
    end;

    procedure Loop(Depth: Integer; C: Char)
    var
        T: Codeunit "Cell Target";
    begin
        if Depth > 0 then
            Loop(Depth - 1, C);
        Ext.RunIt();
        T.Missing();
        T.P(C);
        Face.Touch();
    end;

    procedure Ping(Depth: Integer; C: Char)
    var
        T: Codeunit "Cell Target";
    begin
        if Depth > 0 then
            Pong(Depth - 1, C);
        Ext.RunIt();
        T.Missing();
        T.P(C);
        Face.Touch();
    end;

    procedure Pong(Depth: Integer; C: Char)
    begin
        if Depth > 0 then
            Ping(Depth - 1, C);
    end;
}
"#;

    /// The four kinds the fold maps, written out (not read from the classifier).
    /// A deliberate hand-stated copy of `is_callsite_local_kind`'s list: the
    /// test's expectation must not come from the code under test.
    const KINDS: [&str; 4] = [
        "ambiguous-overload",
        "external-target",
        "interface-open-world",
        "member-not-found",
    ];

    /// The fixture roots under `tests/` that `scc_set_tests`'s fixture oracle
    /// (`derived_only_matches_raw_fold_on_fixtures`) visits, with the app dirs
    /// each holds today: a dir that stops being visited fails that oracle, and
    /// a new fixture changes this one list.
    pub(super) const FIXTURE_ROOTS: [(&str, usize); 5] = [
        ("r0-corpus", 217),
        ("fixtures", 29),
        ("r3a4-fixtures", 1),
        ("r3a5-fixtures", 1),
        ("r2-5b-fixtures", 1),
    ];

    /// Sorted dirs at most `max_depth` levels under `root` holding an `app.json`.
    pub(super) fn app_dirs(root: &Path, max_depth: usize) -> Vec<PathBuf> {
        let walk = walkdir::WalkDir::new(root).max_depth(max_depth);
        let mut dirs: Vec<PathBuf> = walk
            .into_iter()
            .map(|e| e.unwrap().into_path())
            .filter(|p| p.join("app.json").is_file())
            .collect();
        dirs.sort();
        dirs
    }

    /// A3.7: the scan against a hand-built graph. `A` carries all six kinds
    /// the builder emits (`member-not-found` at two call sites), `B` one
    /// `external-target`, and an id that is no workspace routine one
    /// `ambiguous-overload`. The expected map is written literally.
    #[test]
    fn callsite_local_scan_hand_stated() {
        let ue = |from: &str, kind: &str, cs: &str| UncertaintyEdge {
            from: from.to_string(),
            uncertainty: Uncertainty {
                kind: kind.to_string(),
                callsite_id: Some(cs.to_string()),
                operation_id: None,
                routine_id: None,
                interface_name: None,
            },
        };
        let graph = PrimaryGraph {
            nodes: vec!["A".to_string(), "B".to_string()],
            edges_by_from: HashMap::new(),
            edges_from_order: Vec::new(),
            uncertainty_edges: vec![
                ue("A", "interface-open-world", "a1"),
                ue("A", "dynamic-dispatch", "a2"),
                ue("A", "ambiguous-overload", "a3"),
                ue("A", "member-not-found", "a4"),
                ue("A", "member-not-found", "a5"),
                ue("A", "external-target", "a6"),
                ue("A", "unresolved-call", "a7"),
                ue("B", "external-target", "b1"),
                ue("Orphan", "ambiguous-overload", "o1"),
            ],
            collided_site_callee: HashMap::new(),
        };
        let set = |ks: &[&str]| {
            ks.iter()
                .map(|k| k.to_string())
                .collect::<BTreeSet<String>>()
        };
        let want: HashMap<String, BTreeSet<String>> = HashMap::from([
            (
                "A".to_string(),
                set(&[
                    "ambiguous-overload",
                    "external-target",
                    "interface-open-world",
                    "member-not-found",
                ]),
            ),
            ("B".to_string(), set(&["external-target"])),
            ("Orphan".to_string(), set(&["ambiguous-overload"])),
        ]);
        assert_eq!(
            owned(callsite_local_coverage_reasons(&graph.uncertainty_edges)),
            want
        );
    }

    /// The snapshot base USES the scan: built by production code on
    /// [`CELLS_SRC`], each routine that makes the four call-site-local calls
    /// has all four as direct coverage reasons and a `partial` direct status.
    #[test]
    fn snapshot_base_folds_the_scan_into_direct_coverage() {
        let m = model(CELLS_SRC);
        let base = build_r3a3_source_only_base(&m);
        let mut failures = Vec::new();
        for name in ["Plain", "Loop", "Ping"] {
            let ids: Vec<&str> = m
                .workspace
                .routines
                .iter()
                .filter(|r| r.name == name)
                .map(|r| r.id.as_str())
                .collect();
            assert_eq!(ids.len(), 1, "precondition: one routine {name}");
            let coverage = &base.cones[ids[0]].coverage;
            if coverage.direct_status != "partial" {
                failures.push(format!("{name}: direct status {}", coverage.direct_status));
            }
            for kind in KINDS {
                if !coverage.reasons.iter().any(|r| r == kind) {
                    failures.push(format!(
                        "{name}: no reason {kind} in {:?}",
                        coverage.reasons
                    ));
                }
            }
        }
        assert!(failures.is_empty(), "\n{}", failures.join("\n"));
    }

    /// A3.3 (ii): the snapshot base prints no non-convergence line. A child
    /// copy of this test binary builds the base on [`CAP_SRC`] under a forced
    /// cap with `--nocapture` (the pattern of `summary_runner`'s
    /// `build_l4_prints_the_capped_line_once`); its stderr must hold zero
    /// `warning: summarize:` lines.
    #[test]
    fn snapshot_base_prints_no_capped_line() {
        const CHILD: &str = "AL_SEM_SNAPSHOT_BASE_PRINT_CHILD";
        const NAME: &str = "engine::l4::capability_cone::callsite_local_scan_tests::snapshot_base_prints_no_capped_line";
        if std::env::var_os(CHILD).is_some() {
            let m = model(CAP_SRC);
            let base = with_roles_iteration_cap(1, || build_r3a3_source_only_base(&m));
            assert!(
                !base.ws_routines.is_empty(),
                "precondition: a base was built"
            );
            return;
        }
        // Precondition: under the same cap, the L4 solve on CAP_SRC does cap
        // (non-empty diagnostics, a `fixpoint-capped` member), so a base that
        // ran the solve and printed its warnings would print a line here.
        let m = model(CAP_SRC);
        let graph = build_combined_graph(&m.workspace, &m.calls, &m.events.graph);
        let scc = crate::engine::l4::analysis::graph_scc(&graph);
        let fields = crate::engine::l4::analysis::field_index(&m.workspace);
        let (summaries, diags) = with_roles_iteration_cap(1, || {
            crate::engine::l4::summary_runner::compute_summaries_v2(
                &m.workspace.routines,
                &graph,
                &scc,
                &m.calls.upgraded_bindings,
                &fields,
            )
        });
        assert!(!diags.is_empty(), "precondition: the cap fires");
        assert!(
            summaries
                .values()
                .any(|s| s.uncertainties.iter().any(|u| u.kind == "fixpoint-capped")),
            "precondition: a cycle member carries fixpoint-capped"
        );
        assert!(
            crate::engine::l4::analysis::render_warnings(&diags)
                .iter()
                .any(|l| l.starts_with("warning: summarize:")),
            "precondition: the cap renders a `warning: summarize:` line"
        );
        let out = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", NAME, "--nocapture", "--test-threads=1"])
            .env(CHILD, "1")
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            out.status.success() && stdout.contains("1 passed"),
            "the child ran the test\nstdout: {stdout}\nstderr: {stderr}"
        );
        let printed: Vec<&str> = stderr
            .lines()
            .filter(|l| l.starts_with("warning: summarize:"))
            .collect();
        assert_eq!(printed, Vec::<&str>::new(), "stderr: {stderr}");
    }
}

/// #160 step 7a: one inherited set per Cone-SCC. The tests of the spec's §5
/// (`docs/superpowers/specs/2026-10-20-issue-160-step7a-scc-membership-design.md`),
/// T1-T11 and the §5.1 row comparator. Each asserts its precondition before
/// its main assertions. Since step 7b's last commit the oracle is the fold of
/// a `RawOnly` composition (raw inherited Vec ∪ raw direct facts per
/// routine), the witness engine step 6 keeps (7b spec §4.7).
#[cfg(test)]
mod scc_set_tests {
    use super::callsite_local_scan_tests::{
        FIXTURE_ROOTS, app_dirs, model, model_with_tables, workspace_model,
    };
    use super::*;
    use crate::engine::l4::cone_derived::test_facts::{call_edge, fact, temp_fact};
    use crate::engine::l4::cone_derived::{MAY_COMMIT, TOUCHES_PHYSICAL_TABLE, TOUCHES_TABLE};
    use crate::engine::l4::exec_relation::ExecRelation;
    use crate::engine::l5::detector_context::det_cone_inputs;
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::path::Path;

    type Direct = HashMap<String, Vec<CapabilityFact>>;

    fn strs(xs: &[&str]) -> Vec<String> {
        xs.iter().map(|s| s.to_string()).collect()
    }

    // -- §5.1 the row comparator ---------------------------------------------

    /// The first field in which two stores' rows differ, compared by resolved
    /// content (never by `ResId`), or `None` when every row is equal.
    fn rows_diff(a: &ConeDerivedStore, b: &ConeDerivedStore) -> Option<String> {
        if a.row_ids() != b.row_ids() {
            return Some(format!("row ids: {:?} vs {:?}", a.row_ids(), b.row_ids()));
        }
        for id in a.row_ids() {
            let field = if a.flags_of(id) != b.flags_of(id) {
                "flags_of"
            } else if a.writes_tables_of(id) != b.writes_tables_of(id) {
                "writes_tables_of"
            } else if a.writes_physical_tables_of(id) != b.writes_physical_tables_of(id) {
                "writes_physical_tables_of"
            } else if a.physical_table_write_ops_of(id) != b.physical_table_write_ops_of(id) {
                "physical_table_write_ops_of"
            } else if a.physical_table_reads_of(id) != b.physical_table_reads_of(id) {
                "physical_table_reads_of"
            } else if a.publishes_events_of(id) != b.publishes_events_of(id) {
                "publishes_events_of"
            } else {
                continue;
            };
            return Some(format!("{id}: {field} differs"));
        }
        None
    }

    fn assert_rows_equal(label: &str, a: &ConeDerivedStore, b: &ConeDerivedStore) {
        if let Some(d) = rows_diff(a, b) {
            panic!("{label}: {d}");
        }
    }

    type Rows<'a> = Vec<(&'a str, Vec<CapabilityFact>)>;

    /// One store from hand-built rows, each `begin_routine` → fold → finish.
    fn store(rows: &[(&str, Vec<CapabilityFact>)]) -> ConeDerivedStore {
        let mut b = ConeDerivedBuilder::default();
        for (id, facts) in rows {
            b.begin_routine();
            for f in facts {
                b.fold_fact(f);
            }
            b.finish_routine(id);
        }
        b.finish()
    }

    /// §5.1: seven pairs, each differing in one thing; the comparator panics
    /// on each and names the field (or "row ids"). Equal stores pass.
    #[test]
    fn rows_comparator_sees_each_field() {
        let phys = |op: &'static str, t: &str| fact("X", op, "table", Some(t));
        let tmp = |op: &'static str, t: &str| temp_fact("X", op, t, "known", Some(true));
        let commit = fact("X", "commit", "transaction", None);
        let publish = |e: &str| fact("X", "publish", "event", Some(e));
        let pairs: Vec<(&str, Rows, Rows)> = vec![
            (
                "flags_of",
                vec![("X", vec![commit.clone()])],
                vec![("X", vec![])],
            ),
            (
                "writes_tables_of",
                vec![("X", vec![tmp("insert", "t/A"), tmp("insert", "t/B")])],
                vec![("X", vec![tmp("insert", "t/A")])],
            ),
            (
                "writes_physical_tables_of",
                vec![("X", vec![phys("insert", "t/A"), tmp("insert", "t/B")])],
                vec![("X", vec![tmp("insert", "t/A"), phys("insert", "t/B")])],
            ),
            (
                "physical_table_write_ops_of",
                vec![("X", vec![phys("insert", "t/A")])],
                vec![("X", vec![phys("modify", "t/A")])],
            ),
            (
                "physical_table_reads_of",
                vec![("X", vec![phys("read", "t/R")])],
                vec![("X", vec![phys("read", "t/S")])],
            ),
            (
                "publishes_events_of",
                vec![("X", vec![publish("e/1")])],
                vec![("X", vec![publish("e/2")])],
            ),
            (
                "row ids",
                vec![("X", vec![phys("insert", "t/A")])],
                vec![("X", vec![phys("insert", "t/A")]), ("Y", vec![])],
            ),
        ];
        for (field, a, b) in &pairs {
            let (sa, sb) = (store(a), store(b));
            // Positive control: each side equals its own rebuild.
            assert_rows_equal("control", &sa, &store(a));
            assert_rows_equal("control", &sb, &store(b));
            // A neutral label: the field must come from the diagnostic itself.
            let msg = catch_unwind(AssertUnwindSafe(|| assert_rows_equal("pair", &sa, &sb)))
                .err()
                .map(|p| {
                    p.downcast_ref::<String>()
                        .cloned()
                        .or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string()))
                        .unwrap_or_default()
                });
            match msg {
                Some(m) => assert!(
                    if *field == "row ids" {
                        m.starts_with("pair: row ids: ")
                    } else {
                        m == format!("pair: X: {field} differs")
                    },
                    "pair {field}: the comparator panicked naming something else: {m}"
                ),
                None => panic!("pair {field}: the comparator did not panic"),
            }
        }
    }

    // -- helpers over synthetic graphs ----------------------------------------

    fn compose(
        typed: &[TypedEdge],
        nodes: &[String],
        direct_in: &Direct,
        mode: ConeOutput,
    ) -> ConeOutcome {
        compose_cone_over_graph(typed, nodes, direct_in, &HashMap::new(), &[], mode)
    }

    /// The oracle store (7b §4.7): per routine, F(its `RawOnly` inherited Vec
    /// ∪ its raw direct facts).
    fn raw_fold(raw: &ConeOutcome, direct_in: &Direct) -> ConeDerivedStore {
        let mut b = ConeDerivedBuilder::default();
        for (id, r) in &raw.cones {
            b.fold_routine(
                id,
                r.inherited
                    .iter()
                    .chain(direct_in.get(id).into_iter().flatten()),
            );
        }
        b.finish()
    }

    /// The production path (`DerivedOnly`), the `RawOnly` witness engine, and
    /// the oracle store folded from the latter.
    fn derived_and_raw_fold(
        typed: &[TypedEdge],
        nodes: &[String],
        direct_in: &Direct,
    ) -> (ConeOutcome, ConeOutcome, ConeDerivedStore) {
        let new = compose(typed, nodes, direct_in, ConeOutput::DerivedOnly);
        let raw = compose(typed, nodes, direct_in, ConeOutput::RawOnly);
        let oracle = raw_fold(&raw, direct_in);
        (new, raw, oracle)
    }

    fn insert(subject: &str, t: &str) -> CapabilityFact {
        fact(subject, "insert", "table", Some(t))
    }

    const TABLE_FLAGS: u8 = TOUCHES_TABLE | TOUCHES_PHYSICAL_TABLE;

    /// T1's graph: A <-> B (one SCC), B -> C; A writes `t/A`, B publishes
    /// `e/B`, C writes `t/C`.
    fn t1_graph() -> (Vec<TypedEdge>, Vec<String>, Direct) {
        let typed = vec![
            call_edge("A", "B", "cs/AB"),
            call_edge("B", "A", "cs/BA"),
            call_edge("B", "C", "cs/BC"),
        ];
        let direct: Direct = [
            ("A".to_string(), vec![insert("A", "t/A")]),
            (
                "B".to_string(),
                vec![fact("B", "publish", "event", Some("e/B"))],
            ),
            ("C".to_string(), vec![insert("C", "t/C")]),
        ]
        .into();
        (typed, strs(&["A", "B", "C"]), direct)
    }

    // -- T1 -------------------------------------------------------------------

    /// §5.2 T1: hand-stated rows over A <-> B, B -> C.
    #[test]
    fn scc_set_rows_hand_stated() {
        let (typed, nodes, direct) = t1_graph();
        let (new, _raw, oracle) = derived_and_raw_fold(&typed, &nodes, &direct);
        assert_eq!(
            new.stats.recursive_sccs, 1,
            "precondition: one recursive SCC, {{A, B}}"
        );
        let d = &new.derived;
        for m in ["A", "B"] {
            assert_eq!(d.writes_tables_of(m), ["t/A", "t/C"], "{m}: writes");
            assert_eq!(
                d.writes_physical_tables_of(m),
                ["t/A", "t/C"],
                "{m}: physical writes"
            );
            assert_eq!(d.publishes_events_of(m), ["e/B"], "{m}: events");
            assert_eq!(d.flags_of(m), TABLE_FLAGS, "{m}: flags");
        }
        assert_eq!(d.writes_tables_of("C"), ["t/C"], "C: writes");
        assert_eq!(d.publishes_events_of("C"), Vec::<String>::new());
        assert_eq!(d.flags_of("C"), TABLE_FLAGS, "C: flags");
        assert_rows_equal("T1", d, &oracle);
    }

    // -- helpers over inline AL -----------------------------------------------

    /// Compose over the det cone's exact inputs ([`det_cone_inputs`]).
    fn compose_model(m: &Model, rel: &ExecRelation, mode: ConeOutput) -> ConeOutcome {
        let i = det_cone_inputs(m, rel);
        compose_cone_over_graph(
            rel.cone(),
            &i.nodes,
            &i.direct_full,
            &i.coverage_in,
            &m.workspace.routines,
            mode,
        )
    }

    /// [`derived_and_raw_fold`] over a model's det cone inputs.
    fn derived_and_raw_fold_model(
        m: &Model,
        rel: &ExecRelation,
    ) -> (ConeOutcome, ConeOutcome, ConeDerivedStore) {
        let new = compose_model(m, rel, ConeOutput::DerivedOnly);
        let raw = compose_model(m, rel, ConeOutput::RawOnly);
        let oracle = raw_fold(&raw, &det_cone_inputs(m, rel).direct_full);
        (new, raw, oracle)
    }

    /// The id of the one routine named `name`.
    fn routine_id(m: &Model, name: &str) -> String {
        let ids: Vec<&str> = m
            .workspace
            .routines
            .iter()
            .filter(|r| r.name == name)
            .map(|r| r.id.as_str())
            .collect();
        assert_eq!(ids.len(), 1, "fixture: one routine {name}, got {ids:?}");
        ids[0].to_string()
    }

    /// The resource id the model gives table `name` (a fixture input).
    fn table_id(m: &Model, name: &str) -> String {
        let ids: Vec<&str> = m
            .workspace
            .tables
            .iter()
            .filter(|t| t.name == name)
            .map(|t| t.id.as_str())
            .collect();
        assert_eq!(ids.len(), 1, "fixture: one table {name}, got {ids:?}");
        ids[0].to_string()
    }

    /// The producer solve's non-seed states at routine `id`.
    fn non_seed_states(pd: &super::super::pd_solve::ConeArena, id: &str) -> usize {
        pd.by_routine
            .get(id)
            .map_or(0, |v| v.iter().filter(|&&s| !pd.states[s].seed).count())
    }

    // -- T3, T3b --------------------------------------------------------------

    const T3_SRC: &str = r#"
codeunit 50100 C
{
    procedure W(Flag: Boolean; var R: Record Cust)
    begin
        if Flag then
            R.Insert();
    end;

    procedure Q(Flag: Boolean)
    var
        R: Record Cust;
    begin
        W(Flag, R);
        P();
    end;

    procedure P()
    begin
        Q(false);
    end;
}
"#;

    /// §5.4 T3: P and Q form one SCC; Q holds a physical state of W's
    /// producer under `[Q.Flag = true]`, and P's literal `false` contradicts
    /// it, so the members' own entries differ.
    #[test]
    fn scc_set_keeps_own_entries_per_member() {
        let m = model(T3_SRC);
        let rel = ExecRelation::single_app(&m);
        let (q, p, cust) = (
            routine_id(&m, "Q"),
            routine_id(&m, "P"),
            table_id(&m, "Cust"),
        );
        let (new, _raw, oracle) = derived_and_raw_fold_model(&m, &rel);
        assert!(
            non_seed_states(&new.pd_solution, &q) >= 1,
            "precondition: Q holds a state of W's producer"
        );
        assert_eq!(
            non_seed_states(&new.pd_solution, &p),
            0,
            "precondition: P's Q(false) contradicts W's guard"
        );
        assert_eq!(
            new.stats.own_entries_recursive, 1,
            "own entries of the recursive members (Q's state of W's producer)"
        );
        assert_eq!(new.derived.writes_physical_tables_of(&q), [cust], "Q's row");
        assert_eq!(
            new.derived.writes_physical_tables_of(&p),
            Vec::<String>::new(),
            "P's row"
        );
        assert_rows_equal("T3", &new.derived, &oracle);
    }

    // -- T4 -------------------------------------------------------------------

    const T4_SRC: &str = r#"
codeunit 50100 C
{
    var
        Rec2: Record Cust;

    procedure A()
    begin
        B();
    end;

    procedure B()
    begin
        G(true);
        A();
    end;

    procedure G(Flag: Boolean)
    begin
        if Flag then
            Rec2.Modify();
    end;

    procedure N()
    begin
        G(false);
    end;

    procedure M()
    begin
        G(true);
    end;
}
"#;

    /// §5.5 T4: the recursive SCC {A, B} reaches G's guarded write; its set
    /// merges G's frame-bound entry with its base key.
    #[test]
    fn scc_set_carries_guarded_successor_entries() {
        let m = model(T4_SRC);
        let rel = ExecRelation::single_app(&m);
        let cust = table_id(&m, "Cust");
        let [a, b, n, mm] = ["A", "B", "N", "M"].map(|r| routine_id(&m, r));
        let (new, _raw, oracle) = derived_and_raw_fold_model(&m, &rel);
        assert_eq!(
            new.derived.writes_tables_of(&n),
            Vec::<String>::new(),
            "precondition: N's G(false) contradicts the guard, so G's entry is guarded"
        );
        assert_eq!(
            new.derived.writes_tables_of(&mm),
            std::slice::from_ref(&cust),
            "precondition: M's G(true) satisfies it"
        );
        for (name, id) in [("A", &a), ("B", &b)] {
            assert_eq!(
                new.derived.writes_tables_of(id),
                std::slice::from_ref(&cust),
                "{name}: the T2 write"
            );
        }
        assert_rows_equal("T4", &new.derived, &oracle);
    }

    // -- T11 ------------------------------------------------------------------

    /// §5.5b T11: `nodes = [A, C]`, edges A -> B, B -> A, B -> C; B is in the
    /// SCC but not in `nodes`, so it gets no row.
    #[test]
    fn scc_set_skips_members_outside_nodes() {
        let typed = vec![
            call_edge("A", "B", "cs/AB"),
            call_edge("B", "A", "cs/BA"),
            call_edge("B", "C", "cs/BC"),
        ];
        let nodes = strs(&["A", "C"]);
        let direct: Direct = [
            ("B".to_string(), vec![insert("B", "t/B")]),
            ("C".to_string(), vec![insert("C", "t/C")]),
        ]
        .into();
        let (new, _raw, oracle) = derived_and_raw_fold(&typed, &nodes, &direct);
        assert_eq!(
            new.stats.recursive_members_all, 2,
            "precondition: Tarjan puts A and B in one SCC"
        );
        assert_eq!(
            new.stats.recursive_members_emitted, 1,
            "precondition: only A is in nodes"
        );
        assert_eq!(
            new.derived.row_ids(),
            ["A", "C"].into_iter().collect::<BTreeSet<&str>>()
        );
        assert_eq!(
            new.derived.writes_tables_of("A"),
            ["t/C"],
            "A reaches t/C through B"
        );
        assert_rows_equal("T11", &new.derived, &oracle);
    }

    // -- T10 ------------------------------------------------------------------

    /// §5.9 T10: one 3-member root SCC, one non-root self-loop, one
    /// non-recursive singleton.
    #[test]
    fn compose_stats_counts_scc_shapes() {
        let typed = vec![
            call_edge("X", "Y", "cs/XY"),
            call_edge("Y", "Z", "cs/YZ"),
            call_edge("Z", "X", "cs/ZX"),
            call_edge("Z", "L", "cs/ZL"),
            call_edge("L", "L", "cs/LL"),
            call_edge("L", "S", "cs/LS"),
        ];
        let nodes = strs(&["X", "Y", "Z", "L", "S"]);
        let out = compose(&typed, &nodes, &HashMap::new(), ConeOutput::DerivedOnly);
        let s = out.stats;
        assert_eq!(s.recursive_sccs, 2, "recursive_sccs");
        assert_eq!(s.recursive_members_all, 4, "recursive_members_all");
        assert_eq!(s.recursive_roots, 1, "recursive_roots");
        assert_eq!(s.self_loops, 1, "self_loops");
        assert_eq!(s.size_hist, [1, 0, 1, 0, 0], "size_hist");
        assert_eq!(s.recursive_members_emitted, 4, "recursive_members_emitted");
        assert_eq!(s.max_recursive_members, 3, "max_recursive_members");
        assert_eq!(
            s.member_walks, 0,
            "member_walks (DerivedOnly folds the sets)"
        );
        assert_eq!(
            (s.scc_sets_built, s.scc_sets_borrowed),
            (1, 1),
            "one set built (the root), one borrowed (the self-loop)"
        );
    }

    // -- T7', T8': the oracle over fixtures and CDO ----------------------------

    /// T7'/T8''s classes, summed over the compositions checked (§5.8), from the
    /// new path's [`ComposeStats`]: the cone's own counts, no second walk.
    #[derive(Default, Debug)]
    struct Classes {
        compositions: usize,
        routines: usize,
        self_loops: u64,
        multi_member: u64,
        roots: u64,
        non_roots: u64,
        with_successor: u64,
        with_guarded_successor: u64,
        members_with_own_entry: u64,
        /// #160 step 7b: memo builds summed (asserted `> 0` over T7''s corpus
        /// and on T8''s CDO cross-app universe, never per composition).
        closed_memo_builds: u64,
    }

    impl Classes {
        fn add(&mut self, s: &ComposeStats, routines: usize) {
            self.compositions += 1;
            self.routines += routines;
            self.self_loops += s.self_loops;
            self.multi_member += s.recursive_sccs - s.self_loops;
            self.roots += s.recursive_roots;
            self.non_roots += s.recursive_sccs - s.recursive_roots;
            self.with_successor += s.with_successor;
            self.with_guarded_successor += s.with_guarded_successor;
            self.members_with_own_entry += s.members_with_own_entry;
            self.closed_memo_builds += s.closed_memo_builds;
        }

        fn line(&self) -> String {
            format!(
                "self-loops {} multi-member {} roots {} non-roots {} with-successor {} with-guarded-successor {} members-with-own-entry {} closed-memo-builds {}",
                self.self_loops,
                self.multi_member,
                self.roots,
                self.non_roots,
                self.with_successor,
                self.with_guarded_successor,
                self.members_with_own_entry,
                self.closed_memo_builds
            )
        }
    }

    /// #160 step 7b §4.7 T7'/T8': one model, `DerivedOnly` on the production
    /// path against the fold of the streamed `RawOnly` Vec plus `direct_raw`,
    /// per routine; counts classes and returns the production path's stats.
    /// Survives step 6 (no `Both`, no second derived path).
    fn check_model(
        m: &Model,
        rel: &ExecRelation,
        where_: &str,
        classes: &mut Classes,
        failures: &mut Vec<String>,
    ) -> ComposeStats {
        let i = det_cone_inputs(m, rel);
        let new = compose_cone_over_graph(
            rel.cone(),
            &i.nodes,
            &i.direct_full,
            &i.coverage_in,
            &m.workspace.routines,
            ConeOutput::DerivedOnly,
        );
        if (new.stats.member_walks, new.stats.singleton_scans) != (0, 0) {
            failures.push(format!(
                "{where_}: DerivedOnly ran an old path (member_walks {}, singleton_scans {})",
                new.stats.member_walks, new.stats.singleton_scans
            ));
        }
        let (_raw, oracle) = raw_fold_hook::fold(|| {
            compose_cone_over_graph(
                rel.cone(),
                &i.nodes,
                &i.direct_full,
                &i.coverage_in,
                &m.workspace.routines,
                ConeOutput::RawOnly,
            )
        });
        if let Some(d) = rows_diff(&new.derived, &oracle) {
            failures.push(format!("{where_}: {d}"));
        }
        classes.add(&new.stats, i.nodes.len());
        new.stats
    }

    fn check_cross_app(
        dir: &Path,
        where_: &str,
        classes: &mut Classes,
        failures: &mut Vec<String>,
    ) -> Option<ComposeStats> {
        let inputs = crate::program::inputs::InputSet::read(dir, &Default::default());
        match build_cross_app_base(
            &inputs,
            crate::program::model::workspace::MODEL_INSTANCE_ID_DEFAULT,
        ) {
            Ok(base) => Some(check_model(
                &base.resolved,
                &base.rel,
                &format!("{where_} (cross-app)"),
                classes,
                failures,
            )),
            Err(_) => {
                failures.push(format!("{where_}: no cross-app base built"));
                None
            }
        }
    }

    /// 7b §4.7 T7' (was 7a §5.8 T7): the derived-row oracle
    /// ([`check_model`]) on every app dir under step 3's five roots,
    /// single-app and cross-app, over the det cone's exact inputs. Prints the
    /// class table; the classes non-zero on the first run are minimums.
    #[test]
    fn derived_only_matches_raw_fold_on_fixtures() {
        let label = "raw-fold oracle";
        let start = std::time::Instant::now();
        let tests = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
        let mut single = Classes::default();
        let mut cross = Classes::default();
        let mut failures = Vec::new();
        for (root, want) in FIXTURE_ROOTS {
            let dirs = app_dirs(&tests.join(root), usize::MAX);
            assert_eq!(dirs.len(), want, "{root}: app dirs");
            for dir in &dirs {
                let where_ = dir.display().to_string();
                match workspace_model(dir) {
                    Some(m) => {
                        let rel = ExecRelation::single_app(&m);
                        check_model(&m, &rel, &where_, &mut single, &mut failures);
                    }
                    None => failures.push(format!("{where_}: no single-app model built")),
                }
                check_cross_app(dir, &where_, &mut cross, &mut failures);
            }
        }
        println!(
            "{label} on fixtures, {:.1?}\nsingle-app: {} compositions, {} routines, {}\ncross-app: {} compositions, {} routines, {}",
            start.elapsed(),
            single.compositions,
            single.routines,
            single.line(),
            cross.compositions,
            cross.routines,
            cross.line()
        );
        assert!(failures.is_empty(), "\n{}", failures.join("\n"));
        let dirs: usize = FIXTURE_ROOTS.iter().map(|(_, n)| n).sum();
        assert_eq!(single.compositions, dirs, "single-app compositions");
        assert_eq!(cross.compositions, dirs, "cross-app compositions");
        // The first run's non-zero classes (2026-10-20: 4 multi-member roots in
        // each mode). Every other class is 0 over the fixtures, a stated LIMIT:
        // self-loops, non-roots, successors, guarded successors and own
        // entries are covered only by T2, T3/T3b, T4 and T8'.
        for (mode, c) in [("single-app", &single), ("cross-app", &cross)] {
            assert!(c.multi_member >= 1, "{mode}: no multi-member SCC");
            assert!(c.roots >= 1, "{mode}: no recursive root");
        }
        // #160 step 7b §4.7: summed, not per composition (`ws-d7-event-cycle`
        // holds only recursive participants and builds no memo row).
        assert!(
            single.closed_memo_builds + cross.closed_memo_builds > 0,
            "{label}: no memo row built over the whole corpus"
        );
    }

    /// 7b §4.7 T8' (was 7a §5.8 T8): the oracle ([`check_model`]) on CDO
    /// (skips without `CDO_WS`, panics under `ENFORCE_CDO_WS=1`), cross-app
    /// and single-app, one after the other.
    #[test]
    fn derived_only_matches_raw_fold_on_cdo() {
        let label = "raw-fold oracle";
        let Some(ws) = crate::cdo::cdo_ws_or_enforce() else {
            return;
        };
        let mut failures = Vec::new();
        let mut cross_memo_builds = 0;
        for mode in ["cross-app", "single-app"] {
            let start = std::time::Instant::now();
            let mut classes = Classes::default();
            let stats = if mode == "cross-app" {
                check_cross_app(&ws, "CDO", &mut classes, &mut failures)
            } else {
                let m = workspace_model(&ws).expect("the CDO workspace builds a single-app model");
                let rel = ExecRelation::single_app(&m);
                Some(check_model(&m, &rel, "CDO", &mut classes, &mut failures))
            };
            let stats = stats.unwrap_or_else(|| {
                panic!(
                    "CDO {mode}: no composition to check (no model built)\n{}",
                    failures.join("\n")
                )
            });
            println!(
                "{label} on CDO ({mode}): {} routines, {} recursive SCCs, {} members, max {}, {}, singleton_scans {}, {:.1?}",
                classes.routines,
                stats.recursive_sccs,
                stats.recursive_members_emitted,
                stats.max_recursive_members,
                classes.line(),
                stats.singleton_scans,
                start.elapsed()
            );
            if mode == "cross-app" {
                assert_eq!(
                    stats.recursive_members_emitted, 714,
                    "CDO cross-app recursive members emitted: step 0 counted 714 BFS calls; a \
                     different number means the input moved and the spec's numbers are stale"
                );
                // T8' is the only real-code oracle for these classes (T7''s
                // fixtures reach none of them), so a moved baseline must not drop them
                // silently. Roots are 0 on CDO, so the root-set build is carried
                // by the hand graphs (T1-T6, T10, T11 all have a recursive root;
                // T2's break targets it) and the census CLI test.
                for (class, n) in [
                    ("multi-member", classes.multi_member),
                    ("with-successor", classes.with_successor),
                    ("with-guarded-successor", classes.with_guarded_successor),
                    ("members-with-own-entry", classes.members_with_own_entry),
                ] {
                    assert!(n >= 1, "CDO cross-app: no {class} class");
                }
                cross_memo_builds = stats.closed_memo_builds;
            }
        }
        assert!(failures.is_empty(), "\n{}", failures.join("\n"));
        // #160 step 7b §4.7: the probe counted 25,498 builds cross-app (P5).
        // Single-app is not asserted: its count was not measured.
        assert!(
            cross_memo_builds > 0,
            "{label}: CDO cross-app built no memo row"
        );
    }

    /// T5's graph: a 479-member cycle M000 -> ... -> M478 -> M000, each member
    /// one distinct write `t/Mi`, plus the chain M000 -> C1 -> C2 -> C3, each
    /// writing `t/Ck`: 482 tables.
    fn t5_graph() -> (Vec<TypedEdge>, Vec<String>, Direct) {
        let m = |i: usize| format!("M{i:03}");
        let mut typed: Vec<TypedEdge> = (0..479)
            .map(|i| call_edge(&m(i), &m((i + 1) % 479), &format!("cs/{}", m(i))))
            .collect();
        typed.push(call_edge("M000", "C1", "cs/M000-C1"));
        typed.push(call_edge("C1", "C2", "cs/C1"));
        typed.push(call_edge("C2", "C3", "cs/C2"));
        let mut nodes: Vec<String> = (0..479).map(m).collect();
        nodes.extend(strs(&["C1", "C2", "C3"]));
        let direct: Direct = nodes
            .iter()
            .map(|n| (n.clone(), vec![insert(n, &format!("t/{n}"))]))
            .collect();
        (typed, nodes, direct)
    }

    /// T3b's source: M1 -> ... -> M8 -> M1; Mi calls `Wi(R)` on a local
    /// physical `Record Ti` and `Commit()`; `Wi` inserts into its parameter.
    fn t3b_src() -> String {
        let mut s = String::from("codeunit 50100 C\n{\n");
        for i in 1..=8 {
            let next = i % 8 + 1;
            s += &format!(
                "    procedure M{i}()\n    var\n        R: Record T{i};\n    begin\n        W{i}(R);\n        Commit();\n        M{next}();\n    end;\n\n    procedure W{i}(var R: Record T{i})\n    begin\n        R.Insert();\n    end;\n\n"
            );
        }
        s + "}\n"
    }

    /// One raw fact as `(op, kind, resource id, via, witness call site)`.
    fn raw_row(f: &CapabilityFact) -> (&str, &str, Option<&str>, &str, Option<&str>) {
        (
            f.op,
            f.resource_kind,
            f.resource_id.as_deref(),
            f.via,
            f.witness_callsite_id.as_deref(),
        )
    }

    // -- T2 -------------------------------------------------------------------

    /// §5.3 T2: a root two-member SCC {X, Y} (X -> S) and a non-root
    /// self-loop R (R -> R, R -> S2) called by the singleton U.
    #[test]
    fn scc_set_root_and_self_loop() {
        let typed = vec![
            call_edge("X", "Y", "cs/XY"),
            call_edge("Y", "X", "cs/YX"),
            call_edge("X", "S", "cs/XS"),
            call_edge("R", "R", "cs/RR"),
            call_edge("R", "S2", "cs/RS2"),
            call_edge("U", "R", "cs/UR"),
        ];
        let nodes = strs(&["X", "Y", "S", "R", "S2", "U"]);
        let direct: Direct = [
            ("S".to_string(), vec![insert("S", "t/S")]),
            ("S2".to_string(), vec![insert("S2", "t/S2")]),
        ]
        .into();
        let (new, _raw, oracle) = derived_and_raw_fold(&typed, &nodes, &direct);
        assert_eq!(
            new.stats.recursive_roots, 1,
            "precondition: one root ({{X, Y}})"
        );
        assert_eq!(new.stats.self_loops, 1, "precondition: one self-loop (R)");
        let d = &new.derived;
        for m in ["X", "Y", "S"] {
            assert_eq!(d.writes_tables_of(m), ["t/S"], "{m}: writes");
        }
        for m in ["R", "U", "S2"] {
            assert_eq!(d.writes_tables_of(m), ["t/S2"], "{m}: writes");
        }
        assert_rows_equal("T2", d, &oracle);
        assert_eq!(new.stats.scc_sets_built, 1, "the root's set is built once");
        assert_eq!(
            new.stats.scc_sets_borrowed, 1,
            "the self-loop's set is its fact cone"
        );
    }

    /// §5.4 T3b: the producer blow-up, callers inside the cycle.
    #[test]
    fn scc_set_own_entries_scale() {
        let names: Vec<String> = (1..=8).map(|i| format!("T{i}")).collect();
        let name_refs: Vec<&str> = names.iter().map(String::as_str).collect();
        let m = model_with_tables(&name_refs, &t3b_src());
        let rel = ExecRelation::single_app(&m);
        let mut tables: Vec<String> = names.iter().map(|n| table_id(&m, n)).collect();
        tables.sort();
        let members: Vec<String> = (1..=8).map(|i| routine_id(&m, &format!("M{i}"))).collect();
        let (new, _raw, oracle) = derived_and_raw_fold_model(&m, &rel);
        for (i, mi) in members.iter().enumerate() {
            assert_eq!(
                non_seed_states(&new.pd_solution, mi),
                8,
                "precondition: M{} holds one state per producer",
                i + 1
            );
        }
        assert_eq!(
            new.stats.own_entries_recursive, 64,
            "precondition: 8 members x 8 own entries"
        );
        assert_eq!(
            new.stats.scc_set_entries_folded, 8,
            "precondition: the set holds one entry (the commit), folded by 8 members"
        );
        for (i, mi) in members.iter().enumerate() {
            assert!(
                new.derived.flags_of(mi) & MAY_COMMIT != 0,
                "precondition: M{} may commit",
                i + 1
            );
        }
        for (i, mi) in members.iter().enumerate() {
            assert_eq!(
                new.derived.writes_physical_tables_of(mi),
                tables,
                "M{}'s row",
                i + 1
            );
        }
        assert_rows_equal("T3b", &new.derived, &oracle);
    }

    // -- T5, T6 ---------------------------------------------------------------

    /// §5.6 T5: a 479-member cycle with a successor chain of 3.
    #[test]
    fn scc_set_479_cycle_scales() {
        let (typed, nodes, direct) = t5_graph();
        let (new, raw, oracle) = derived_and_raw_fold(&typed, &nodes, &direct);
        let mut all: Vec<String> = nodes.iter().map(|n| format!("t/{n}")).collect();
        all.sort();
        assert_eq!(all.len(), 482);
        for n in nodes.iter().filter(|n| n.starts_with('M')) {
            assert_eq!(new.derived.writes_tables_of(n), all, "{n}: all 482 tables");
        }
        assert_rows_equal("T5", &new.derived, &oracle);
        assert_eq!(
            raw.stats.member_walks, 479,
            "the oracle's RawOnly composition walks every member"
        );
        assert_eq!(
            new.stats.member_walks, 0,
            "no per-member walk under DerivedOnly"
        );
        assert_eq!(
            new.stats.scc_sets_borrowed + new.stats.scc_sets_built,
            1,
            "one set for the one SCC"
        );
        assert_eq!(
            new.stats.scc_set_entries_folded,
            479 * 482,
            "one pass per member over one set"
        );
    }

    /// §5.6 T6: (a) a chain of 20 two-member SCCs, (b) a three-member SCC with
    /// 200 successor singletons; one set per SCC in each.
    #[test]
    fn scc_set_chain_and_fan_out() {
        // (a) P_k <-> Q_k, Q_k -> P_{k+1}; P_k writes t/k.
        let mut typed = Vec::new();
        let mut nodes = Vec::new();
        let mut direct: Direct = HashMap::new();
        for k in 0..20 {
            let (p, q) = (format!("P{k:02}"), format!("Q{k:02}"));
            typed.push(call_edge(&p, &q, &format!("cs/{p}")));
            typed.push(call_edge(&q, &p, &format!("cs/{q}")));
            if k < 19 {
                typed.push(call_edge(
                    &q,
                    &format!("P{:02}", k + 1),
                    &format!("cs/{q}-next"),
                ));
            }
            direct.insert(p.clone(), vec![insert(&p, &format!("t/{k}"))]);
            nodes.push(p);
            nodes.push(q);
        }
        let (new, _raw, oracle) = derived_and_raw_fold(&typed, &nodes, &direct);
        assert_rows_equal("T6 chain", &new.derived, &oracle);
        assert_eq!(
            new.stats.scc_sets_borrowed + new.stats.scc_sets_built,
            20,
            "chain: one set per SCC"
        );

        // (b) A -> B -> C -> A, A -> S_j for 200 singletons, each writing t/S_j.
        let mut typed = vec![
            call_edge("A", "B", "cs/AB"),
            call_edge("B", "C", "cs/BC"),
            call_edge("C", "A", "cs/CA"),
        ];
        let mut nodes = strs(&["A", "B", "C"]);
        let mut direct: Direct = HashMap::new();
        for j in 0..200 {
            let s = format!("S{j:03}");
            typed.push(call_edge("A", &s, &format!("cs/A-{s}")));
            direct.insert(s.clone(), vec![insert(&s, &format!("t/{s}"))]);
            nodes.push(s);
        }
        let (new, _raw, oracle) = derived_and_raw_fold(&typed, &nodes, &direct);
        assert_rows_equal("T6 fan-out", &new.derived, &oracle);
        assert_eq!(
            new.stats.scc_sets_borrowed + new.stats.scc_sets_built,
            1,
            "fan-out: one set for the one SCC"
        );
    }

    // -- T9 -------------------------------------------------------------------

    /// §5.7 T9: the production entry `compose_cone_over_graph` uses the new
    /// path (derived), and the raw half is unchanged (raw, recursive members).
    #[test]
    fn derived_only_runs_no_member_walk() {
        let run = |g: &(Vec<TypedEdge>, Vec<String>, Direct), mode| {
            compose_cone_over_graph(&g.0, &g.1, &g.2, &HashMap::new(), &[], mode)
        };
        let t1 = t1_graph();
        let t5 = t5_graph();

        // Raw: A's raw inherited Vec on T1, literal, under Both and RawOnly.
        let want = vec![
            ("insert", "table", Some("t/C"), "call", Some("cs/AB")),
            ("publish", "event", Some("e/B"), "call", Some("cs/AB")),
        ];
        for (label, g, walks) in [("T1", &t1, 2), ("T5", &t5, 479)] {
            let both = run(g, ConeOutput::Both);
            let raw = run(g, ConeOutput::RawOnly);
            if label == "T1" {
                for (mode, out) in [("Both", &both), ("RawOnly", &raw)] {
                    let got: Vec<_> = out.cones["A"].inherited.iter().map(raw_row).collect();
                    assert_eq!(got, want, "T1 {mode}: A's raw inherited Vec");
                }
            }
            for n in &g.1 {
                assert_eq!(
                    both.cones[n].inherited, raw.cones[n].inherited,
                    "{label} {n}: Both's raw Vec equals RawOnly's"
                );
            }
            assert_eq!(both.stats.member_walks, walks, "{label}: Both walks");
            assert_eq!(raw.stats.member_walks, walks, "{label}: RawOnly walks");
            let derived = run(g, ConeOutput::DerivedOnly);
            assert_rows_equal(label, &both.derived, &derived.derived);
        }

        // Derived: on T5 under DerivedOnly, no member walk runs.
        let out = run(&t5, ConeOutput::DerivedOnly);
        assert_eq!(
            out.stats.member_walks, 0,
            "compose_cone_over_graph runs a per-member walk under DerivedOnly"
        );
        assert_eq!(out.stats.recursive_members_emitted, 479);
    }

    /// Plan Review Focus 2: no set for a recursive SCC with no member in
    /// `nodes`. `nodes = [A, C]`, edges A -> X, X -> Y, Y -> X, X -> C. Tarjan
    /// starts only from `nodes`, so such an SCC is always reached from a
    /// node outside it and is never a root: the case is a non-root one.
    #[test]
    fn scc_set_none_for_an_scc_with_no_emitted_member() {
        let typed = vec![
            call_edge("A", "X", "cs/AX"),
            call_edge("X", "Y", "cs/XY"),
            call_edge("Y", "X", "cs/YX"),
            call_edge("X", "C", "cs/XC"),
        ];
        let nodes = strs(&["A", "C"]);
        let direct: Direct = [("C".to_string(), vec![insert("C", "t/C")])].into();
        let (new, _raw, oracle) = derived_and_raw_fold(&typed, &nodes, &direct);
        assert_eq!(
            (
                new.stats.recursive_sccs,
                new.stats.recursive_members_emitted
            ),
            (1, 0),
            "precondition: one recursive SCC, {{X, Y}}, with no member in nodes"
        );
        assert_eq!(
            new.stats.scc_sets_borrowed + new.stats.scc_sets_built,
            0,
            "no set for an SCC with no emitted member"
        );
        assert_eq!(
            new.derived.row_ids(),
            ["A", "C"].into_iter().collect::<BTreeSet<&str>>()
        );
        assert_eq!(new.derived.writes_tables_of("A"), ["t/C"], "A reaches t/C");
        assert_rows_equal("no emitted member", &new.derived, &oracle);
    }

    // -- #160 step 7b: closed-class sets, fold memo (spec §5) ------------------

    /// The production entry over a hand graph (no coverage, no routines).
    fn production(
        typed: &[TypedEdge],
        nodes: &[String],
        direct: &Direct,
        mode: ConeOutput,
    ) -> ConeOutcome {
        compose_cone_over_graph(typed, nodes, direct, &HashMap::new(), &[], mode)
    }

    /// 7b §5 T1: singletons A -> C, B -> C, C -> D; C writes `t/C`, D writes
    /// `t/D`. C's cone is read by A and B, D's by C: two memo rows, three reads.
    #[test]
    fn closed_memo_folds_a_shared_callee_once() {
        let typed = vec![
            call_edge("A", "C", "cs/AC"),
            call_edge("B", "C", "cs/BC"),
            call_edge("C", "D", "cs/CD"),
        ];
        let nodes = strs(&["A", "B", "C", "D"]);
        let direct: Direct = [
            ("C".to_string(), vec![insert("C", "t/C")]),
            ("D".to_string(), vec![insert("D", "t/D")]),
        ]
        .into();
        let out = production(&typed, &nodes, &direct, ConeOutput::DerivedOnly);
        assert_eq!(
            out.stats.recursive_sccs, 0,
            "precondition: every routine is a singleton"
        );
        let d = &out.derived;
        for m in ["A", "B", "C"] {
            assert_eq!(d.writes_tables_of(m), ["t/C", "t/D"], "{m}: writes");
            assert_eq!(d.flags_of(m), TABLE_FLAGS, "{m}: flags");
        }
        assert_eq!(d.writes_tables_of("D"), ["t/D"], "D: writes");
        assert_eq!(
            out.stats.closed_memo_builds, 2,
            "one memo row per callee cone read (C's once for A and B, D's for C)"
        );
        assert_eq!(
            out.stats.closed_memo_reads, 3,
            "one fold_row per (subject, callee SCC)"
        );
        assert_eq!(
            out.stats.closed_memo_entries, 3,
            "C's cone holds t/C and t/D, D's holds t/D"
        );
    }

    /// 7b §5 T2's source: G writes `Cust` under `[G.Flag = true]`; N calls
    /// `G(false)`, which contradicts the guard, M calls `G(true)`.
    const T2_7B_SRC: &str = r#"
codeunit 50100 C
{
    var
        Rec2: Record Cust;

    procedure G(Flag: Boolean)
    begin
        if Flag then
            Rec2.Modify();
    end;

    procedure N()
    begin
        G(false);
    end;

    procedure M()
    begin
        G(true);
    end;
}
"#;

    /// 7b §5 T2: G's only entry is frame-bound, so it must stay out of G's
    /// memo row and reach N and M through `substitute_entry` alone.
    #[test]
    fn closed_memo_excludes_frame_bound_entries() {
        let m = model(T2_7B_SRC);
        let rel = ExecRelation::single_app(&m);
        let cust = table_id(&m, "Cust");
        let [n, mm] = ["N", "M"].map(|r| routine_id(&m, r));
        let out = compose_model(&m, &rel, ConeOutput::DerivedOnly);
        assert_eq!(
            out.derived.writes_tables_of(&n),
            Vec::<String>::new(),
            "N: G(false) contradicts the guard"
        );
        assert_eq!(
            out.derived.writes_tables_of(&mm),
            std::slice::from_ref(&cust),
            "M: G(true) satisfies it"
        );
        // Pins the USE: N and M read G's cone through ONE memo row, so a memo
        // row that took the frame-bound entry would reach N.
        assert_eq!(
            out.stats.closed_memo_builds, 1,
            "G's cone: one memo row, shared by N and M"
        );
        assert_eq!(out.stats.closed_memo_reads, 2, "N and M each read G's row");
        assert_eq!(
            out.stats.closed_memo_entries, 0,
            "G's only entry is guarded, so its memo row folds no entry"
        );
    }

    /// 7b §5 T3: A calls C at two call sites; C's memo row is folded into A
    /// once.
    #[test]
    fn duplicate_edges_fold_one_row() {
        let typed = vec![call_edge("A", "C", "cs/AC1"), call_edge("A", "C", "cs/AC2")];
        let nodes = strs(&["A", "C"]);
        let direct: Direct = [("C".to_string(), vec![insert("C", "t/C")])].into();
        assert_eq!(
            build_typed_edge_graph(&typed, &nodes).outgoing["A"]
                .iter()
                .filter(|e| e.to == "C")
                .count(),
            2,
            "precondition: the cone's graph keeps both A -> C edges"
        );
        let out = production(&typed, &nodes, &direct, ConeOutput::DerivedOnly);
        assert_eq!(out.derived.writes_tables_of("A"), ["t/C"], "A: writes");
        assert_eq!(out.stats.closed_memo_builds, 1, "C's cone, built once");
        assert_eq!(
            out.stats.closed_memo_reads, 1,
            "A reads C's row once, not once per edge (the J(m) dedup)"
        );
    }

    /// 7b §5 T4 on 7a T3's source: P and Q form one recursive SCC; Q owns a
    /// state of W's producer, P does not. The SCC's set is folded once, and
    /// each member's own entries stay its own.
    #[test]
    fn scc_set_folds_once_per_scc() {
        let m = model(T3_SRC);
        let rel = ExecRelation::single_app(&m);
        let (q, p, cust) = (
            routine_id(&m, "Q"),
            routine_id(&m, "P"),
            table_id(&m, "Cust"),
        );
        let out = compose_model(&m, &rel, ConeOutput::DerivedOnly);
        assert!(
            non_seed_states(&out.pd_solution, &q) >= 1,
            "precondition: Q holds a state of W's producer"
        );
        assert_eq!(
            non_seed_states(&out.pd_solution, &p),
            0,
            "precondition: P's Q(false) contradicts W's guard"
        );
        assert_eq!(out.derived.writes_physical_tables_of(&q), [cust], "Q's row");
        assert_eq!(
            out.derived.writes_physical_tables_of(&p),
            Vec::<String>::new(),
            "P's row"
        );
        assert_eq!(
            out.stats.scc_set_folds, 1,
            "the SCC's set is folded once, not once per member"
        );
    }

    /// 7b §5 T5 (pins the USE): the production entry builds no `best` under
    /// `DerivedOnly`; under `Both` every singleton scans and the raw Vecs are
    /// today's. Graph A -> C -> D; C writes `t/C`, D writes `t/D`.
    #[test]
    fn derived_only_builds_no_best() {
        let typed = vec![call_edge("A", "C", "cs/AC"), call_edge("C", "D", "cs/CD")];
        let nodes = strs(&["A", "C", "D"]);
        let direct: Direct = [
            ("C".to_string(), vec![insert("C", "t/C")]),
            ("D".to_string(), vec![insert("D", "t/D")]),
        ]
        .into();
        let both = production(&typed, &nodes, &direct, ConeOutput::Both);
        let raw = |id: &str| -> Vec<_> { both.cones[id].inherited.iter().map(raw_row).collect() };
        assert_eq!(
            raw("A"),
            vec![
                ("insert", "table", Some("t/C"), "call", Some("cs/AC")),
                ("insert", "table", Some("t/D"), "call", Some("cs/AC")),
            ],
            "Both: A's raw Vec"
        );
        assert_eq!(
            raw("C"),
            vec![("insert", "table", Some("t/D"), "call", Some("cs/CD"))],
            "Both: C's raw Vec"
        );
        assert!(raw("D").is_empty(), "Both: D's raw Vec");
        assert_eq!(both.stats.singleton_scans, 3, "Both: every singleton scans");
        let derived = production(&typed, &nodes, &direct, ConeOutput::DerivedOnly);
        assert_eq!(
            derived.stats.singleton_scans, 0,
            "DerivedOnly built `best` for a singleton"
        );
    }
}

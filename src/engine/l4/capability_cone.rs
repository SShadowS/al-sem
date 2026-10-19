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

use super::combined_graph::{CombinedGraph, TypedEdge, build_combined_graph};
use super::cone_census;
use super::cone_derived::{ConeDerivedBuilder, ConeDerivedStore, ConeOutput, fact_is_known_temp};
use super::scc::{Scc, SccInputGraph, SccResult, tarjan_scc};
use crate::engine::ids::to_stable_object_id;
use crate::program::body::capability::table_commit;
use crate::program::body::features::{PCallSite, PCallee, PExpressionInfo, POperationSite};
use crate::program::model::events::{EventGraph, EventSymbol};
use crate::program::model::workspace::{Model, ModelEntities, ModelRoutine};

// ===========================================================================
// Internal CapabilityFact (FULL form — internal ids). Mirrors al-sem
// `model/capability.ts` `CapabilityFact`, dropping `subject` (the projection
// key) and carrying the L3-resolved `resource_id`.
// ===========================================================================

/// A `ValueSource` (internal form — table-field tableId is INTERNAL until
/// projected). Mirrors al-sem `ValueSource`.
#[derive(Debug, Clone, PartialEq, Eq, salsa::SalsaValue)]
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
#[derive(Debug, Clone, PartialEq, salsa::SalsaValue)]
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
#[derive(Debug, Clone, PartialEq, salsa::SalsaValue)]
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
    /// `witness_callsite_id`, `extra`) stay owned. `salsa::SalsaValue` still
    /// derives (salsa 0.28 replaced `Update` with `SalsaValue` + `PartialEq`):
    /// a `&'static str` — unlike the `&'db T` salsa's own doc warns about — can
    /// never dangle or change under a revision.
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
#[derive(Debug, Clone, PartialEq, Eq, salsa::SalsaValue)]
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

/// Build the typed-edge graph from the combined graph's typed edges + the node
/// list. Mirrors al-sem `buildTypedEdgeGraph`.
fn build_typed_edge_graph(graph: &CombinedGraph, nodes: &[String]) -> TypedEdgeGraph {
    let mut outgoing: HashMap<String, Vec<TypedOutEdge>> = HashMap::new();
    let mut unresolved_sources: BTreeSet<String> = BTreeSet::new();

    for edge in &graph.typed_edges {
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

/// Singleton non-recursive fast path. Mirrors `inheritedFactsForSingleton`.
///
/// ⟨C1⟩ `best` holds BORROWED representatives/edges (it used to clone both on
/// every win): the winners' fields are read identically whether borrowed or
/// cloned, and only the survivors are cloned — by `retag`, and only when `mode`
/// wants the raw Vec. Under [`ConeOutput::DerivedOnly`] neither the `retag`
/// clone nor `sort_inherited`'s allocation happens; the fold over the SAME
/// `best.values()` key-winners still runs.
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

    if mode.wants_derived() {
        let _t = crate::engine::l5::detector_context::cones_census::start();
        for b in best.values() {
            derived.fold_fact(b.rep);
        }
        crate::engine::l5::detector_context::cones_census::add_since(
            &crate::engine::l5::detector_context::cones_census::SGL_FOLD_NANOS,
            _t,
        );
    }
    if !mode.wants_raw() {
        return Vec::new();
    }
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
/// ⟨C1⟩ The `seen`-deduped representatives this walk visits are the derived
/// fold's inherited source. Note the asymmetry: a SIBLING member's facts come
/// from the key-deduped `direct` map (not its raw Vec), while a downstream SCC
/// contributes its cone entries. ⟨issue 33⟩ R3 used to *pin* that asymmetry
/// because the two inputs gave different answers; with the temp class in
/// [`inherited_fact_key`] the derived fold is dedup-invariant and they agree, so
/// this is now a cost choice rather than a correctness one (see
/// `cone_derived`'s module docs). The walk returns its picks, one
/// `(representative, first hop)` per key in emission order; [`emit_picks`]
/// folds and retags them.
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
                    // `String` keys. This walk runs 503 times per 8020 run, so the
                    // one owned copy per newly-seen key is not on any hot path.
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

/// A recursive member's inherited output from its BFS picks: the raw push and
/// the derived fold consume the SAME picks. Under [`ConeOutput::DerivedOnly`]
/// the `retag` clones and `sort_inherited` are skipped; the fold is unchanged.
fn emit_picks(
    subject: &str,
    picks: Vec<(&CapabilityFact, &TypedOutEdge)>,
    mode: ConeOutput,
    derived: &mut ConeDerivedBuilder,
) -> Vec<CapabilityFact> {
    if mode.wants_derived() {
        for (rep, _) in &picks {
            derived.fold_fact(rep);
        }
    }
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

/// The per-routine cone result.
struct InheritedConeResult {
    inherited: Vec<CapabilityFact>,
    coverage: CoverageRecord,
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
) {
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

    let empty_succ: BTreeSet<usize> = BTreeSet::new();
    for i in 0..scc.sccs.len() {
        let scc_entry = &scc.sccs[i];
        let succ_ids = succ_sccs.get(i).unwrap_or(&empty_succ);

        // ⟨C1 Task 4⟩ An SCC's cone is read by exactly ONE kind of consumer: a
        // PREDECESSOR of that SCC. `fact_cone_for_scc` reads `fact_cones[y]`
        // only for `y ∈ succ_sccs[j]`, and both per-routine walks
        // (`inherited_facts_for_singleton` / `inherited_facts_by_bfs`) read a
        // cone only for a routine whose SCC differs from the subject's — never
        // the subject's own (`build_succ_sccs` excludes self, and the BFS
        // treats a same-SCC hop as a sibling and expands it instead of pulling
        // a cone; a non-recursive singleton has no self-edge by `tarjan_scc`'s
        // `recursive` rule, so it cannot reach its own id either). So SCC `i`'s
        // cone is never read while `i` itself is being processed.
        //
        // `remaining_uses[i]` is the number of SCCs holding `i` as a successor,
        // decremented once per predecessor as that predecessor finishes. Tarjan
        // emits callees before callers, so none of `i`'s predecessors has run
        // yet when `i` is reached: `remaining_uses[i] == 0` here means `i` is a
        // call-graph ROOT (nothing calls it) and NOTHING will ever read its
        // cone. Building it is pure waste — on the 8020 corpus that was 17,864
        // root SCCs holding 2,224,901 fact entries (~1,599 MB) alive for the
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
                let picks = inherited_facts_by_bfs(
                    m,
                    g,
                    direct,
                    &scc.scc_id_by_routine,
                    &fact_cones,
                    own_entries(m),
                );
                crate::engine::l5::detector_context::cones_census::add_since(
                    &crate::engine::l5::detector_context::cones_census::BFS_WALK_NANOS,
                    _t_walk,
                );
                crate::engine::l5::detector_context::cones_census::add_gated(
                    &crate::engine::l5::detector_context::cones_census::BFS_PICKS,
                    picks.len() as u64,
                );
                let _t_fold = crate::engine::l5::detector_context::cones_census::start();
                let out = emit_picks(m, picks, mode, &mut derived);
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

        // refcount-free downstream cones whose last predecessor (this SCC) is done.
        for y in succ_ids {
            if remaining_uses[*y] > 0 {
                remaining_uses[*y] -= 1;
            }
            if remaining_uses[*y] == 0 {
                fact_cones.remove(y);
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
        fact_cones.is_empty() && cov_cones.is_empty(),
        "cone leak: {} fact cone(s) / {} coverage cone(s) survived the SCC walk",
        fact_cones.len(),
        cov_cones.len()
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
    (out, derived.finish(), arena)
}

// ===========================================================================
// R3b WRAP SEAM — a public cone entry the Salsa `inherited_facts`/`coverage`
// queries call. WRAPS the full cone (typed-edge graph build + Tarjan SCC +
// `compose_inherited_cones`) over a CombinedGraph + per-routine direct facts +
// direct coverage. Byte-identical to the cone path inside `project_r3a3` /
// `project_r3a5_cross_app` (it IS that path, factored out). No re-port.
// ===========================================================================

/// One routine's cone result in PUBLIC (internal-id) form. Field-for-field the
/// private `InheritedConeResult`.
#[derive(Debug, Clone, PartialEq, salsa::SalsaValue)]
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
}

/// Compute every routine's `capabilityFactsInherited` + `coverage` over the given
/// combined graph (typed edges already folded, incl. any injected intra-app dep
/// edges) + the per-routine direct facts + direct coverage. The `nodes` list is
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
    graph: &CombinedGraph,
    nodes: &[String],
    direct_in: &HashMap<String, Vec<CapabilityFact>>,
    coverage_in: &HashMap<String, (String, Vec<String>)>,
    routines: &[ModelRoutine],
    mode: ConeOutput,
) -> ConeOutcome {
    crate::engine::perf_trace::counter_delta("build.cone", 1);
    let _t_graph = crate::engine::l5::detector_context::cones_census::start();
    let g = build_typed_edge_graph(graph, nodes);
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
    let (cones, derived, pd_solution) = compose_inherited_cones(
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

/// Map a routine's L4 fixed-point `uncertainties` to coverage reasons, faithful
/// to summary-runner.ts:565-596. Only the four call-resolution uncertainty kinds
/// that imply incomplete call resolution map to reasons:
///   ambiguous-overload → "ambiguous-overload"
///   member-not-found   → "member-not-found"
///   external-target    → "external-target"
///   interface-open-world → "interface-open-world"
/// (Source-only: `interfaceImplsKnowledgePartial` is false, so the coarser
/// `interface-impls-unknown-in-deps` reason never co-fires.)
///
/// Runs the L4 JACOBI fixed point over the COMBINED-graph SCC (the same inputs
/// `project_r3a2` uses) — the cone reads only the typed-edge SCC, but the
/// uncertainties are a property of the call-resolution fixed point.
fn compute_uncertainty_coverage_reasons(
    ws: &ModelEntities,
    graph: &CombinedGraph,
    calls: &crate::program::model::calls::ResolvedCalls,
) -> HashMap<String, BTreeSet<String>> {
    use crate::engine::l4::summary_runner::{FieldIndex, compute_summaries_v2};

    // Tarjan SCC over the COMBINED graph (summary substrate — distinct from the
    // typed-edge SCC the cone walks).
    let mut adjacency: HashMap<String, Vec<String>> = HashMap::new();
    for (from, list) in &graph.edges_by_from {
        adjacency.insert(from.clone(), list.iter().map(|e| e.to.clone()).collect());
    }
    crate::engine::perf_trace::counter_delta("build.graph_scc", 1);
    let scc = tarjan_scc(&SccInputGraph {
        nodes: &graph.nodes,
        edges_by_from: &adjacency,
    });

    // Field-resolution index (parameterRoles need it; harmless here).
    let mut field_index: FieldIndex = HashMap::new();
    for table in &ws.tables {
        for field in &table.fields {
            field_index
                .entry((table.id.clone(), field.name.to_lowercase()))
                .or_insert_with(|| field.id.clone());
        }
    }

    // v2 db-effect solver (roles-cap diagnostics ignored here). Only
    // `uncertainties` are read below.
    let (summaries, _) = compute_summaries_v2(
        &ws.routines,
        graph,
        &scc,
        &calls.upgraded_bindings,
        &field_index,
    );

    let mut out: HashMap<String, BTreeSet<String>> = HashMap::new();
    for (rid, summary) in &summaries {
        let mut reasons: BTreeSet<String> = BTreeSet::new();
        for u in &summary.uncertainties {
            match u.kind.as_str() {
                "ambiguous-overload" => {
                    reasons.insert("ambiguous-overload".to_string());
                }
                "member-not-found" => {
                    reasons.insert("member-not-found".to_string());
                }
                "external-target" => {
                    reasons.insert("external-target".to_string());
                }
                "interface-open-world" => {
                    reasons.insert("interface-open-world".to_string());
                }
                _ => {}
            }
        }
        if !reasons.is_empty() {
            out.insert(rid.clone(), reasons);
        }
    }
    out
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
    /// The workspace app's guid, lowercase (engine-switch S7.4): the role of an
    /// object-anchored finding.
    pub primary_app_guid: String,
    /// The program build's dependency coverage and ledger (`alsem analyze`'s
    /// preflight, engine-switch S8.3).
    pub coverage: crate::program::resolve::full::FreshCoverage,
    /// `internalsVisibleTo`, lower-case guids: exposing app -> friend apps (d13).
    pub friends: HashMap<String, BTreeSet<String>>,
    /// The cross-app model itself (engine-switch S7.6), with its calls cut to the
    /// combined graph's (workspace callers only). The detectors get it as their
    /// `resolved`, and the context reads its root classifications and ordering
    /// facts from it. [`Self::ws_routines`] and [`Self::objects`] are views onto
    /// it.
    pub resolved: crate::program::model::workspace::Model,
    pub dep_routine_ids: BTreeSet<String>,
    /// The combined graph WITH the injected dep intra-app typed edges folded in
    /// (the cone substrate). The combined `edges_by_from` / `uncertainty_edges`
    /// drive the JACOBI; the `typed_edges` (incl. injected) drive the cone.
    pub graph: CombinedGraph,
    pub combined_scc: SccResult,
    pub field_index: crate::engine::l4::summary_runner::FieldIndex,
    pub upgraded_bindings: HashMap<String, Vec<crate::program::model::calls::UpgradedBinding>>,
    pub event_graph: EventGraph,
    /// Fixed-leaf (dep) RETAINED summaries: every source-bearing dependency
    /// routine with no out-edge in [`Self::graph`].
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
    /// The dependency-internal edges the cone follows (R3a-4 intra-app edges as
    /// `direct-call` typed edges); also folded into [`Self::graph`].
    pub injected_typed_edges: Vec<TypedEdge>,
    /// Per-routine direct capability facts (full, ordered).
    pub direct_full: HashMap<String, Vec<CapabilityFact>>,
    /// Per-routine direct coverage `(status, reasons)`.
    pub direct_coverage: HashMap<String, (String, Vec<String>)>,
    pub nodes: Vec<String>,
    /// The DECLARED workspace dependencies `{appGuid, name, minVersion}` (app.json
    /// `dependencies[]`). The d17 MinVersion side. ADDITIVE — only the cross-app L5
    /// detector context reads it; NOT serialized into the r3a5 gate.
    pub declared_dependencies: Vec<crate::program::model::workspace::DeclaredDependencyDecl>,
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
            injected_typed_edges: &self.injected_typed_edges,
            declared_dependencies: &self.declared_dependencies,
            app_versions: &self.resolved_app_versions,
            friends: &self.friends,
        }
    }
}

/// The cross-app L4 BASE (engine-switch S7.4): every intermediate the R3a-5
/// projection and the cross-app detectors read, from ONE program-backed model
/// (`assemble_and_resolve_cross_app_program`): the workspace and every dependency,
/// each body's calls and the event graph from the program engine. Returns `None`
/// for a fail-closed workspace.
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
/// - the combined graph (solver, SCC, detector traversals) takes the calls of
///   WORKSPACE callers only; a dependency's own calls reach the CONE alone, as
///   typed edges built by the same rules as the workspace's (engine-switch S8.6):
///   object runs and calls from one dependency into another included. S7 fed the
///   cone only the R3a-4 artifact's admitted own-app edges, because the whole
///   dependency world took CDO's r3a5 projection from 329 MB to 9.8 GB; since
///   S8.2 the model holds only the DEMANDED dependency code, and S8.6 measured
///   +1 s and no memory on CDO/DO.
pub(crate) fn build_cross_app_base(
    inputs: &std::sync::Arc<crate::program::inputs::InputSet>,
    model_instance_id: &str,
) -> Option<R3a5CrossAppBase> {
    use crate::engine::l4::summary_runner::{FieldIndex, base_intraprocedural_summary};
    let mut x = crate::program::model::program_calls::assemble_and_resolve_cross_app_program(
        inputs,
        model_instance_id,
        false,
    )?;
    let ws: &ModelEntities = &x.resolved.workspace;
    let primary = x
        .resolved
        .primary_app
        .as_ref()?
        .app_guid
        .to_ascii_lowercase();
    let dep_routine_ids: BTreeSet<String> = ws
        .routines
        .iter()
        .filter(|r| r.app_guid.to_ascii_lowercase() != primary)
        .map(|r| r.id.clone())
        .collect();

    let all_calls = &x.resolved.calls;
    let calls = crate::program::model::calls::ResolvedCalls {
        edges: all_calls
            .edges
            .iter()
            .filter(|e| !dep_routine_ids.contains(&e.from))
            .cloned()
            .collect(),
        upgraded_bindings: all_calls.upgraded_bindings.clone(),
        diagnostics: all_calls.diagnostics.clone(),
        external_targets: all_calls.external_targets.clone(),
    };
    let event_graph: EventGraph = x.resolved.events.graph.clone();
    let _s_graph = crate::engine::perf_trace::span("crossapp", "crossapp.base_graph");
    let mut graph = build_combined_graph(ws, &calls, &event_graph);

    let nodes: Vec<String> = ws.routines.iter().map(|r| r.id.clone()).collect();
    let mut adjacency: HashMap<String, Vec<String>> = HashMap::new();
    for (from, list) in &graph.edges_by_from {
        adjacency.insert(from.clone(), list.iter().map(|e| e.to.clone()).collect());
    }
    crate::engine::perf_trace::counter_delta("build.graph_scc", 1);
    let combined_scc = tarjan_scc(&SccInputGraph {
        nodes: &graph.nodes,
        edges_by_from: &adjacency,
    });
    let mut field_index: FieldIndex = HashMap::new();
    for table in &ws.tables {
        for field in &table.fields {
            field_index
                .entry((table.id.clone(), field.name.to_lowercase()))
                .or_insert_with(|| field.id.clone());
        }
    }

    drop(_s_graph);
    let _s_facts = crate::engine::perf_trace::span("crossapp", "crossapp.base_artifacts_facts");
    // The dependency code's own calls, cone-only (see the doc above): every
    // resolved call a dependency routine makes, typed by the same rules as the
    // workspace's (object runs, resolved methods, interface `Maybe`, calls into
    // another dependency). Event dispatch is already in `graph`.
    let dep_calls = crate::program::model::calls::ResolvedCalls {
        edges: all_calls
            .edges
            .iter()
            .filter(|e| dep_routine_ids.contains(&e.from))
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
    let injected_typed_edges: Vec<TypedEdge> =
        build_combined_graph(ws, &dep_calls, &no_events).typed_edges;
    graph
        .typed_edges
        .extend(injected_typed_edges.iter().cloned());

    let mut publisher_events_by_routine: HashMap<String, Vec<&EventSymbol>> = HashMap::new();
    for evt in &event_graph.events {
        if let Some(pr) = &evt.publisher_routine_id {
            publisher_events_by_routine
                .entry(pr.clone())
                .or_default()
                .push(evt);
        }
    }
    let routines_by_id: HashMap<String, &ModelRoutine> =
        ws.routines.iter().map(|r| (r.id.clone(), r)).collect();
    let mut leaf_summaries: HashMap<String, crate::engine::l4::summary::RoutineSummary> =
        HashMap::new();
    let mut fixed_roles: HashSet<String> = HashSet::new();
    let mut direct_full: HashMap<String, Vec<CapabilityFact>> = HashMap::new();
    let mut direct_coverage: HashMap<String, (String, Vec<String>)> = HashMap::new();
    let empty_pub: Vec<&EventSymbol> = Vec::new();
    for r in &ws.routines {
        if dep_routine_ids.contains(&r.id) && r.body_available {
            let has_out_edge = graph
                .edges_by_from
                .get(&r.id)
                .is_some_and(|e| !e.is_empty());
            if has_out_edge {
                fixed_roles.insert(r.id.clone());
            } else {
                leaf_summaries.insert(
                    r.id.clone(),
                    base_intraprocedural_summary(r, &routines_by_id, &field_index),
                );
            }
        }
        let pubs = publisher_events_by_routine.get(&r.id).unwrap_or(&empty_pub);
        let (facts, status, reasons) = direct_facts_for_routine(r, pubs);
        direct_coverage.insert(r.id.clone(), (status, reasons));
        direct_full.insert(r.id.clone(), facts);
    }

    let upgraded_bindings = calls.upgraded_bindings.clone();
    let declared_dependencies = std::mem::take(&mut x.declared_dependencies);
    let resolved_app_versions = x
        .dependency_apps
        .iter()
        .map(|a| (a.guid.clone(), a.version.clone()))
        .collect();
    let loaded_apps = std::mem::take(&mut x.loaded_apps);
    let mut resolved = x.resolved;
    resolved.calls = std::sync::Arc::new(calls);
    let base = R3a5CrossAppBase {
        primary_app_guid: primary.clone(),
        coverage: x.coverage.clone(),
        friends: x.friends.clone(),
        resolved,
        dep_routine_ids,
        graph,
        combined_scc,
        field_index,
        upgraded_bindings,
        event_graph: event_graph.clone(),
        leaf_summaries,
        fixed_roles,
        injected_typed_edges,
        direct_full,
        direct_coverage,
        nodes,
        declared_dependencies,
        resolved_app_versions,
        loaded_apps,
    };
    if crate::engine::census::enabled() {
        crate::census_hook::mark("census.base");
        census_cross_app_base(&base);
    }
    Some(base)
}

/// #160 §4.1.5: one `base.<field>` bucket per field of [`R3a5CrossAppBase`],
/// shallow. `resolved` is the model, moved in with its calls replaced by the
/// cut set: its rows and events (`base.resolved.rows`, the part of the `model`
/// bucket that survives) and the replacement calls (`base.resolved.calls`) are
/// charged apart, so nothing the model still owns is missing from the base.
fn census_cross_app_base(b: &R3a5CrossAppBase) {
    use crate::engine::census::{Kind, Tally, combined_graph, event_graph};
    use crate::program::model::calls::UpgradedBinding;
    use crate::program::model::program_calls::{census_calls, census_model_rows};
    let emit = |name: &str, t: &mut Tally| t.emit(name, Kind::Shallow);
    emit(
        "base.primary_app_guid",
        Tally::default().strs([&b.primary_app_guid]),
    );
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
    emit("base.graph", &mut combined_graph(&b.graph));
    emit(
        "base.combined_scc",
        Tally::default()
            .buf::<crate::engine::l4::scc::Scc>(b.combined_scc.sccs.len())
            .buf::<(String, usize)>(b.combined_scc.scc_id_by_routine.len())
            .strs(b.combined_scc.scc_id_by_routine.keys()),
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
        "base.upgraded_bindings",
        Tally::default()
            .buf::<(String, Vec<UpgradedBinding>)>(b.upgraded_bindings.len())
            .strs(b.upgraded_bindings.keys())
            .bufs(b.upgraded_bindings.values()),
    );
    emit("base.event_graph", &mut event_graph(&b.event_graph));
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
    emit(
        "base.injected_typed_edges",
        Tally::default().buf::<TypedEdge>(b.injected_typed_edges.len()),
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
        "base.nodes",
        Tally::default().buf::<String>(b.nodes.len()).strs(&b.nodes),
    );
    emit(
        "base.declared_dependencies",
        Tally::default().buf::<crate::program::model::workspace::DeclaredDependencyDecl>(
            b.declared_dependencies.len(),
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
    let Some(base) = build_cross_app_base(&inputs, model_instance_id) else {
        return empty;
    };
    let ws_routines = base.ws_routines();
    let dep_routine_ids = &base.dep_routine_ids;
    let graph = &base.graph;
    let event_graph = &base.event_graph;
    let direct_full = &base.direct_full;

    // From-scratch core (v2 db-effect solver, Phase-1-parity) + cone over the
    // assembled base. Trace + roles-cap diagnostics ignored here.
    let (core_summaries, _) = compute_summaries_v2_with_leaves_core(
        ws_routines,
        graph,
        &base.combined_scc,
        &base.upgraded_bindings,
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
        graph,
        &base.nodes,
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
    let g = build_typed_edge_graph(&graph, &nodes);
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

    // Per-routine UNCERTAINTY-DERIVED coverage reasons (summary-runner.ts:565-596).
    // Run the L4 fixed-point summary (over the COMBINED-graph SCC) to obtain each
    // routine's `uncertainties`, then map the four call-resolution uncertainty kinds
    // (ambiguous-overload / member-not-found / external-target / interface-open-world)
    // to coverage reasons. A routine carrying any of these has its directStatus
    // downgraded complete→partial so the coverage cone forwards the reason (+ adds
    // the routine to unknownTargets). Source-only `interfaceImplsKnowledgePartial`
    // is false, so the `interface-impls-unknown-in-deps` add-on never fires.
    let uncertainty_reasons = compute_uncertainty_coverage_reasons(ws, &graph, calls);

    // Per-routine direct facts (full) + direct coverage + the dedup-keyed map.
    let mut direct_full: HashMap<String, Vec<CapabilityFact>> = HashMap::new();
    let mut direct: RoutineDirectFacts = HashMap::new();
    let mut cov: RoutineDirectCoverage = HashMap::new();
    let mut routine_ids: BTreeSet<String> = BTreeSet::new();
    let empty_pub: Vec<&EventSymbol> = Vec::new();

    for r in &ws.routines {
        routine_ids.insert(r.id.clone());
        let pubs = publisher_events_by_routine.get(&r.id).unwrap_or(&empty_pub);
        let (facts, mut status, reasons) = direct_facts_for_routine(r, pubs);

        // Canonical rep per dedup key (min repKey wins) — the
        // composeInheritedCones direct-fact dedup.
        let byk = dedup_direct(&facts);
        if !byk.is_empty() {
            direct.insert(r.id.clone(), byk);
        }

        // Fold uncertainty-derived coverage reasons (summary-runner.ts:565-596).
        let mut reason_set: BTreeSet<String> = reasons.iter().cloned().collect();
        let base_len = reason_set.len();
        if let Some(extra_reasons) = uncertainty_reasons.get(&r.id) {
            for rr in extra_reasons {
                reason_set.insert(rr.clone());
            }
        }
        let final_reasons: Vec<String> = if reason_set.len() > base_len {
            // Uncertainty-derived reasons imply incomplete resolution → downgrade.
            if status == "complete" {
                status = "partial".to_string();
            }
            reason_set.into_iter().collect()
        } else {
            reasons.clone()
        };

        cov.insert(
            r.id.clone(),
            DirectCoverage {
                direct_status: status,
                reasons: final_reasons,
            },
        );
        direct_full.insert(r.id.clone(), facts);
    }

    // ⟨C1 Task 3⟩ `RawOnly` — this projection consumes the RAW cone (its
    // `sort_inherited` byte order IS the R3a-3 golden surface, Global Constraint
    // 7) and discards the derived store. Folding one was build-then-drop.
    let routines_by_id: HashMap<&str, &ModelRoutine> =
        ws.routines.iter().map(|r| (r.id.as_str(), r)).collect();
    let frames = guard_frames(&g, &ws.routines, &routines_by_id);
    let (cones, _derived, _pd_solution) = compose_inherited_cones(
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
pub fn build_r3a3_source_only_base(resolved: &Model) -> R3a3SourceBase {
    use crate::engine::perf_trace as pt;
    // Not `r3a3.calls_graph`: that span also covered the input clones, now
    // `r3a3.clone_inputs` (a changed span gets a new name, spec §4.1.1).
    let _s0 = pt::span("r3a3", "r3a3.graph_build");
    let graph = build_combined_graph(&resolved.workspace, &resolved.calls, &resolved.events.graph);
    drop(_s0);
    build_r3a3_source_only_base_over(resolved, graph)
}

/// [`build_r3a3_source_only_base`] with the graph taken as a parameter (#160
/// §4.1.6): calls and event graph still cloned from `resolved`, the direct facts,
/// the uncertainty fold and the cone computed over `graph`. Only the full-graph
/// timings mode passes a graph other than the one the wrapper builds.
pub fn build_r3a3_source_only_base_over(resolved: &Model, graph: CombinedGraph) -> R3a3SourceBase {
    use crate::engine::perf_trace as pt;
    let ws: &ModelEntities = &resolved.workspace;
    let _s0 = pt::span("r3a3", "r3a3.clone_inputs");
    let calls = (*resolved.calls).clone();
    let event_graph: EventGraph = resolved.events.graph.clone();
    drop(_s0);
    let _s1 = pt::span("r3a3", "r3a3.direct_facts");

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

    // Per-routine UNCERTAINTY-DERIVED coverage reasons (summary-runner.ts:565-596).
    let uncertainty_reasons = compute_uncertainty_coverage_reasons(ws, &graph, &calls);

    // Per-routine direct facts (full) + uncertainty-folded direct coverage.
    let mut direct_full: HashMap<String, Vec<CapabilityFact>> = HashMap::new();
    let mut direct_coverage: HashMap<String, (String, Vec<String>)> = HashMap::new();
    let empty_pub: Vec<&EventSymbol> = Vec::new();
    for r in &ws.routines {
        let pubs = publisher_events_by_routine.get(&r.id).unwrap_or(&empty_pub);
        let (facts, mut status, reasons) = direct_facts_for_routine(r, pubs);

        // Fold uncertainty-derived coverage reasons (identical to project_r3a3).
        let mut reason_set: BTreeSet<String> = reasons.iter().cloned().collect();
        let base_len = reason_set.len();
        if let Some(extra_reasons) = uncertainty_reasons.get(&r.id) {
            for rr in extra_reasons {
                reason_set.insert(rr.clone());
            }
        }
        let final_reasons: Vec<String> = if reason_set.len() > base_len {
            if status == "complete" {
                status = "partial".to_string();
            }
            reason_set.into_iter().collect()
        } else {
            reasons.clone()
        };

        direct_coverage.insert(r.id.clone(), (status, final_reasons));
        direct_full.insert(r.id.clone(), facts);
    }

    drop(_s1);
    let _s2 = pt::span("r3a3", "r3a3.compose_cone");
    let nodes: Vec<String> = ws.routines.iter().map(|r| r.id.clone()).collect();
    // ⟨C1 Task 3⟩ `RawOnly` — the snapshot/digest/prove derivers read the RAW
    // cone off this base and never touch `ConeOutcome::derived` (see
    // `project_r3a5_cross_app`). Raw bytes and ordering unchanged.
    let ConeOutcome {
        cones, pd_solution, ..
    } = compose_cone_over_graph(
        &graph,
        &nodes,
        &direct_full,
        &direct_coverage,
        &ws.routines,
        ConeOutput::RawOnly,
    );

    let routine_to_stable: HashMap<String, String> = ws
        .routines
        .iter()
        .map(|r| (r.id.clone(), r.stable_routine_id.clone()))
        .collect();

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
    ) else {
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
            build_r3a3_source_only_base_over(&base.resolved, base.graph.clone())
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
    let g = build_typed_edge_graph(&graph, &nodes);

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
    let empty_pub: Vec<&EventSymbol> = Vec::new();
    let mut direct_keys: HashMap<String, BTreeSet<String>> = HashMap::new();
    for r in &ws.routines {
        let pubs = publisher_events_by_routine.get(&r.id).unwrap_or(&empty_pub);
        let (facts, _, _) = direct_facts_for_routine(r, pubs);
        let mut keys: BTreeSet<String> = BTreeSet::new();
        for f in &facts {
            keys.insert(inherited_fact_key(f));
        }
        if !keys.is_empty() {
            direct_keys.insert(r.id.clone(), keys);
        }
    }

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
    ///   (i)   DB rows, through the store `cross_app_db_effect_bundle` builds:
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

        // (i) DB rows through the store.
        let bundle = crate::engine::l4::effect_query_cli::cross_app_db_effect_bundle(&base);
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
        let ctx = crate::engine::l5::detector_context::build_detector_context_with(
            &base.resolved,
            crate::engine::l5::registry::substrate::ALL,
            Some(base.cross_app_inputs()),
        );
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
        let ctx = crate::engine::l5::detector_context::build_detector_context_with(
            &base.resolved,
            crate::engine::l5::registry::substrate::ALL,
            Some(base.cross_app_inputs()),
        );
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
        let base = build_cross_app_base(&ws, "r0").expect("cross-app base");
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
        let base = build_cross_app_base(&ws, "r0").expect("cross-app base");
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

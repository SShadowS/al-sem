//! R1d — L2 direct capability extractors (Rust port of al-sem's 13 capability
//! family extractors + the `extractCapabilities` orchestrator).
//!
//! Source of truth (al-sem `master`):
//!   - `src/index/capability/extractor.ts` — orchestrator + unreachable filter +
//!     status/reasons roll-up + the L2 opaque override.
//!   - the 13 `src/index/capability/*.ts` family extractors.
//!   - `src/index/capability/value-source.ts` — `ValueSource` builder.
//!   - `src/model/capability.ts` / `src/model/coverage.ts` — the types.
//!
//! This is the L2-DIRECT surface ONLY: every fact is `provenance == "direct"`,
//! `via == "self"`. The STRIPPED projection (matching the committed
//! `l2cap-vectors.json` + the R1d golden) drops:
//!   - `subject` (redundant — nested under its routine),
//!   - `resourceId` (L3-resolved; deferred to R2) — STRUCTURALLY ABSENT here,
//!   - `tableId` on every nested `table-field` `ValueSource` (L3-resolved /
//!     `"unknown"` at L2) — STRUCTURALLY ABSENT here.
//!   - `op:"publish"` facts are NEVER produced (publish is L4-injected from the
//!     resolved eventGraph; `extractEvents` emits SUBSCRIBE only).
//!
//! Because the forbidden L3 fields are not declared on these serde types, they
//! can never serialize — the strip is structural, not a post-pass.

pub mod dispatch_background;
pub mod io;
pub mod table_commit;
pub mod ui_events_error;
pub mod value_source;

use super::features::{PFeatures, PRoutine, PVariableSymbol};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

// ===========================================================================
// Serde types — the STRIPPED projection (matches the vector/golden JSON shape).
// ===========================================================================

/// Where a value at a capability extraction site comes from (`ValueSource`,
/// `model/capability.ts`). `table-field` carries NO `tableId` (deep-stripped:
/// L3-resolved / `"unknown"` at L2). Internally tagged on `kind`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind")]
pub enum ValueSource {
    #[serde(rename = "literal")]
    Literal { value: String },
    #[serde(rename = "enum")]
    Enum {
        #[serde(rename = "enumName")]
        enum_name: String,
        member: String,
    },
    #[serde(rename = "constant-var")]
    ConstantVar {
        #[serde(rename = "varName")]
        var_name: String,
        initializer: Box<ValueSource>,
    },
    #[serde(rename = "parameter")]
    Parameter {
        index: u32,
        #[serde(rename = "varName")]
        var_name: String,
    },
    /// `tableId` STRIPPED — only `fieldName` survives at L2.
    #[serde(rename = "table-field")]
    TableField {
        #[serde(rename = "fieldName")]
        field_name: String,
    },
    #[serde(rename = "expression")]
    Expression,
    #[serde(rename = "unknown")]
    Unknown,
}

/// Per-resourceKind extra semantics (`CapabilityExtra`, `model/capability.ts`).
/// Internally tagged on `kind`. Nested arg-source fields are deep-stripped (no
/// `tableId`) by construction (they are `ValueSource`, which has none).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum CapabilityExtra {
    Table(TableExtra),
    Dispatch(DispatchExtra),
    Http(HttpExtra),
    Event(EventExtra),
    Storage(StorageExtra),
}

fn kind_table() -> &'static str {
    "table"
}
fn kind_dispatch() -> &'static str {
    "dispatch"
}
fn kind_http() -> &'static str {
    "http"
}
fn kind_event() -> &'static str {
    "event"
}
fn kind_storage() -> &'static str {
    "storage"
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TableExtra {
    /// Always "table".
    #[serde(skip_deserializing, default = "kind_table")]
    pub kind: &'static str,
    #[serde(rename = "recordVariableId", skip_serializing_if = "Option::is_none")]
    pub record_variable_id: Option<String>,
    #[serde(rename = "tempState", skip_serializing_if = "Option::is_none")]
    pub temp_state: Option<super::features::PTempState>,
    #[serde(rename = "opSubtype", skip_serializing_if = "Option::is_none")]
    pub op_subtype: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DispatchExtra {
    /// Always "dispatch".
    #[serde(skip_deserializing, default = "kind_dispatch")]
    pub kind: &'static str,
    #[serde(rename = "objectType")]
    pub object_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modal: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HttpExtra {
    /// Always "http".
    #[serde(skip_deserializing, default = "kind_http")]
    pub kind: &'static str,
    pub method: String,
    #[serde(rename = "bodyArgSource", skip_serializing_if = "Option::is_none")]
    pub body_arg_source: Option<ValueSource>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EventExtra {
    /// Always "event".
    #[serde(skip_deserializing, default = "kind_event")]
    pub kind: &'static str,
    #[serde(rename = "eventClass")]
    pub event_class: String,
    #[serde(rename = "includeSender", skip_serializing_if = "Option::is_none")]
    pub include_sender: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StorageExtra {
    /// Always "storage".
    #[serde(skip_deserializing, default = "kind_storage")]
    pub kind: &'static str,
    #[serde(rename = "keyArgSource", skip_serializing_if = "Option::is_none")]
    pub key_arg_source: Option<ValueSource>,
    #[serde(rename = "valueArgSource", skip_serializing_if = "Option::is_none")]
    pub value_arg_source: Option<ValueSource>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
}

/// A normalized direct capability fact — STRIPPED projection (no `subject`, no
/// `resourceId`). `provenance` is always "direct"; `via` always "self".
///
/// Field order matches the al-sem JSON projection key set; `skip_serializing_if`
/// mirrors the TS "only emit a key when defined" convention.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CapabilityFact {
    pub op: String,
    #[serde(rename = "resourceKind")]
    pub resource_kind: String,
    pub confidence: String,
    pub provenance: String,
    pub via: String,
    #[serde(rename = "resourceArgSource", skip_serializing_if = "Option::is_none")]
    pub resource_arg_source: Option<ValueSource>,
    #[serde(rename = "witnessOperationId", skip_serializing_if = "Option::is_none")]
    pub witness_operation_id: Option<String>,
    #[serde(rename = "witnessCallsiteId", skip_serializing_if = "Option::is_none")]
    pub witness_callsite_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extra: Option<CapabilityExtra>,
}

/// Coverage status lattice (`model/coverage.ts`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CoverageStatus {
    Complete,
    Partial,
    Unknown,
}

impl CoverageStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            CoverageStatus::Complete => "complete",
            CoverageStatus::Partial => "partial",
            CoverageStatus::Unknown => "unknown",
        }
    }
}

/// Coverage reason (`model/coverage.ts`). Only the variants the L2 extractors +
/// the opaque override can produce are modelled; serialized as the kebab string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CoverageReason {
    OpaqueDependency,
    ParseIncomplete,
    ExtractionFailed,
}

impl CoverageReason {
    pub fn as_str(self) -> &'static str {
        match self {
            CoverageReason::OpaqueDependency => "opaque-dependency",
            CoverageReason::ParseIncomplete => "parse-incomplete",
            CoverageReason::ExtractionFailed => "extraction-failed",
        }
    }
}

/// An index-stage diagnostic — emitted by the unreachable filter (`severity` =
/// "info", `stage` = "index"). Mirrors `model/finding.ts` `Diagnostic`, projected
/// to the 4 fields al-sem's capture captures.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CapabilityDiagnostic {
    pub severity: String,
    pub stage: String,
    pub message: String,
    #[serde(rename = "sourceRef")]
    pub source_ref: String,
}

/// The full return value of `extract_capabilities`.
pub struct CapabilityExtractionResult {
    pub facts: Vec<CapabilityFact>,
    pub status: CoverageStatus,
    pub reasons: Vec<CoverageReason>,
    pub diagnostics: Vec<CapabilityDiagnostic>,
}

// ===========================================================================
// The extraction context — the Rust equivalent of al-sem's `ExtractionContext`.
// ===========================================================================

/// Per-routine extraction context: the reachable (unreachable-filtered) features
/// view + a variable index (by lowercased name) + the `receiverTypeOf` lookup.
pub struct ExtractionContext<'a> {
    /// Unreachable-filtered features (the family extractors iterate these).
    pub features: &'a PFeatures,
    /// Variable index by lowercased name (built from `features.variables`).
    pub variables: HashMap<String, &'a PVariableSymbol>,
}

impl ExtractionContext<'_> {
    /// Resolve a member-call receiver name to its declared type, "unknown" when
    /// the variable isn't in the index. Mirrors `ctx.receiverTypeOf`.
    pub fn receiver_type_of(&self, receiver_name: &str) -> String {
        self.variables
            .get(&receiver_name.to_lowercase())
            .map(|v| v.declared_type.clone())
            .unwrap_or_else(|| "unknown".to_string())
    }
}

// ===========================================================================
// The orchestrator.
// ===========================================================================

/// Run all 13 family extractors over `routine`, applying the unreachable filter +
/// the L2 opaque override, and roll up status/reasons. Mirrors
/// `extractCapabilities` (`extractor.ts`) + the `summary-runner.ts:509-513`
/// opaque override.
///
/// NEVER produces an `op:"publish"` fact (publish is L4-injected).
pub fn extract_capabilities(routine: &PRoutine) -> CapabilityExtractionResult {
    // ── L2 opaque override (summary-runner.ts:509-513) ─────────────────────
    // !bodyAvailable → opaque-dependency; parseIncomplete → parse-incomplete.
    // Both clear facts + force status "unknown". parseIncomplete takes precedence
    // in al-sem's check order (`!bodyAvailable || parseIncomplete` then the reason
    // is chosen by which branch). The vector for parse-incomplete (body present,
    // has_error) expects reasons=["parse-incomplete"]; a no-body symbol-only
    // routine yields ["opaque-dependency"].
    if !routine.body_available {
        return CapabilityExtractionResult {
            facts: vec![],
            status: CoverageStatus::Unknown,
            reasons: vec![CoverageReason::OpaqueDependency],
            diagnostics: vec![],
        };
    }
    if routine.parse_incomplete {
        return CapabilityExtractionResult {
            facts: vec![],
            status: CoverageStatus::Unknown,
            reasons: vec![CoverageReason::ParseIncomplete],
            diagnostics: vec![],
        };
    }

    let mut diagnostics: Vec<CapabilityDiagnostic> = Vec::new();

    // ── Unreachable exclusion ──────────────────────────────────────────────
    // Partition operationSites / callSites into reachable + unreachable; record
    // ops share IDs with operation sites — exclude by the same id set. Each
    // excluded site emits an index-stage "info" diagnostic.
    let mut unreachable_op_ids: HashSet<String> = HashSet::new();

    let mut reachable_features = routine.features.clone();

    reachable_features.operation_sites.retain(|op| {
        if op.control_context.as_deref() == Some("unreachable") {
            unreachable_op_ids.insert(op.id.clone());
            diagnostics.push(CapabilityDiagnostic {
                severity: "info".to_string(),
                stage: "index".to_string(),
                message: format!("unreachable code: {} after terminating statement", op.kind),
                source_ref: format!(
                    "{}:{}:{}",
                    op.source_anchor.source_unit_id,
                    op.source_anchor.start_line + 1,
                    op.source_anchor.start_column + 1
                ),
            });
            return false;
        }
        true
    });

    reachable_features.call_sites.retain(|cs| {
        if cs.control_context.as_deref() == Some("unreachable") {
            diagnostics.push(CapabilityDiagnostic {
                severity: "info".to_string(),
                stage: "index".to_string(),
                message: "unreachable code: call after terminating statement".to_string(),
                source_ref: format!(
                    "{}:{}:{}",
                    cs.source_anchor.source_unit_id,
                    cs.source_anchor.start_line + 1,
                    cs.source_anchor.start_column + 1
                ),
            });
            return false;
        }
        true
    });

    reachable_features
        .record_operations
        .retain(|op| !unreachable_op_ids.contains(&op.id));

    // ── Build the dispatch context (variable index from the filtered view) ──
    let mut variables: HashMap<String, &PVariableSymbol> = HashMap::new();
    for v in &reachable_features.variables {
        variables.insert(v.name.to_lowercase(), v);
    }
    let ctx = ExtractionContext {
        features: &reachable_features,
        variables,
    };

    // ── Dispatch to families (same order as al-sem's extractor list) ───────
    let mut facts: Vec<CapabilityFact> = Vec::new();
    let mut reasons: Vec<CoverageReason> = Vec::new();

    let mut run = |out: (Vec<CapabilityFact>, Vec<CoverageReason>)| {
        facts.extend(out.0);
        reasons.extend(out.1);
    };

    run(table_commit::extract_table(&ctx, routine));
    run(table_commit::extract_commit(&ctx, routine));
    run(dispatch_background::extract_dispatch(&ctx, routine));
    run(io::extract_http(&ctx, routine));
    run(io::extract_telemetry(&ctx, routine));
    run(io::extract_isolated_storage(&ctx, routine));
    run(io::extract_hyperlink(&ctx, routine));
    run(io::extract_file_blob(&ctx, routine));
    run(dispatch_background::extract_background(&ctx, routine));
    run(ui_events_error::extract_ui(&ctx, routine));
    run(ui_events_error::extract_ui_window_open(&ctx, routine));
    run(ui_events_error::extract_events(&ctx, routine));
    run(ui_events_error::extract_error(&ctx, routine));

    // ── Status roll-up + reason dedupe/sort (matches the TS) ───────────────
    let status = if reasons.is_empty() {
        CoverageStatus::Complete
    } else {
        CoverageStatus::Partial
    };
    let mut deduped: Vec<CoverageReason> = reasons
        .into_iter()
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    deduped.sort();

    CapabilityExtractionResult {
        facts,
        status,
        reasons: deduped,
        diagnostics,
    }
}

/// Confidence derivation from a `ValueSource` (shared by the IO / dispatch /
/// background families). Mirrors the `confidenceFromSource` switch repeated in
/// each TS family extractor.
pub(crate) fn confidence_from_source(vs: &ValueSource) -> &'static str {
    match vs {
        ValueSource::Literal { .. } | ValueSource::Enum { .. } => "static",
        ValueSource::ConstantVar { initializer, .. } => confidence_from_source(initializer),
        ValueSource::Parameter { .. } => "userDynamic",
        ValueSource::TableField { .. } => "configDynamic",
        ValueSource::Expression | ValueSource::Unknown => "unresolved",
    }
}

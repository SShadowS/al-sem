//! R1a L2 feature projection — serde types matching al-sem's ALLOWLISTED L2
//! shape EXACTLY (`scripts/r1a-l2-projection.ts`).
//!
//! These types are the parity surface against the committed
//! `<fixture>.l2.golden.json` files and the `l2-vectors.json` family vectors.
//!
//! R1c: `order` (OperationOrder) on each op/callsite + `scopeFrames` on the
//! routine features ARE now declared + emitted (no longer forbidden).
//!
//! FORBIDDEN fields are STRUCTURALLY ABSENT — they are not declared here, so a
//! stray field can never serialize:
//!   - CapabilityFact (R1d)
//!   - tableId on RecordVariable/RecordOperation/VariableSymbol (L3 Phase-2)
//!   - resourceId (L3 Phase-2)
//!   - resolver-upgraded argumentBindings fields: calleeParameterIsVar,
//!     bindingResolution, sourceTableId (L3 call-resolver).
//!
//! DROPPED (mirrors the TS projection): CFN-node sourceAnchor (the skeleton is
//! shape + id refs only), and enclosingRoutineId on every anchor (it embeds the
//! modelInstanceId-dependent internal RoutineId).
//!
//! `skip_serializing_if = "Option::is_none"` mirrors the TS pattern of only
//! emitting a key when the value is defined — so the JSON shape matches the
//! golden's "optional keys absent" convention exactly.
//!
//! INTERNAL-ONLY fields (`#[serde(skip)]`) ARE in the model and feed L4/L5, but
//! never serialize, so `aldump --l2` and every L2 golden are a LOSSY view of the
//! model: a fact missing from a dump may still be modelled (#15). The complete
//! list (`serde_skip_list_is_complete` keeps it in sync):
//!   - `PCallSite.in_statement_position`
//!   - `PRecordOperation.in_until_condition`
//!   - `PRecordOperation.run_trigger`
//!   - `PLoop.exhausting_advance`
//!   - `PFieldAccess.access`
//!   - `PVarAssignment.rhs_identifier`
//!   - `PCFNNode.is_case_else`
//!   - `PCFNNode.source_range`

use super::operation_order::{OperationOrder, ScopeFrame};
use serde::{Deserialize, Serialize};

/// A projected source anchor. Columns are tree-sitter byte columns (which match
/// al-sem's web-tree-sitter columns byte-for-byte — see `node_util::Utf16Cols`);
/// `enclosingRoutineId` is dropped (it embeds the modelInstanceId-dependent id).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PAnchor {
    #[serde(rename = "sourceUnitId")]
    pub source_unit_id: String,
    #[serde(rename = "startLine")]
    pub start_line: u32,
    #[serde(rename = "startColumn")]
    pub start_column: u32,
    #[serde(rename = "endLine")]
    pub end_line: u32,
    #[serde(rename = "endColumn")]
    pub end_column: u32,
    #[serde(rename = "syntaxKind")]
    pub syntax_kind: String,
}

/// Structured expression classification (`model/expression.ts`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PExpressionInfo {
    pub kind: String,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub qualifier: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub member: Option<String>,
}

/// Temp-state of a record variable / op.
///
/// `{ kind: "known", value }` | `{ kind: "parameter-dependent", parameterIndex }`
/// | `{ kind: "unknown" }`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PTempState {
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<bool>,
    #[serde(rename = "parameterIndex", skip_serializing_if = "Option::is_none")]
    pub parameter_index: Option<u32>,
}

/// THE temp-record suppression decision (#34): only an exact known/true
/// suppresses. Callers PROJECT their representation to `Option<bool>` --
/// `Some(v)` when the temp state is KNOWN with value `v`, `None` for unknown,
/// parameter-dependent or absent -- and never re-derive the rule. A copy of the
/// rule elsewhere fails `temp_rule_lives_once` (`tests/l2_ir/temp_rule_lint.rs`).
#[must_use]
pub fn known_temp_suppresses(known_value: Option<bool>) -> bool {
    known_value == Some(true)
}

impl PTempState {
    /// Projection onto [`known_temp_suppresses`]: `Some(value)` when `kind` is
    /// `"known"`, `None` otherwise. A known state with a missing value reads as
    /// `Some(false)` -- the behaviour of every inline copy this replaced (they
    /// all required `value == Some(true)`).
    #[must_use]
    pub fn known_value(&self) -> Option<bool> {
        (self.kind == "known").then(|| self.value.unwrap_or(false))
    }
}

/// [`known_temp_suppresses`] over an optional [`PTempState`]; absent = not temp.
#[must_use]
pub fn temp_state_suppresses(ts: Option<&PTempState>) -> bool {
    known_temp_suppresses(ts.and_then(PTempState::known_value))
}

/// Structured Callee classification (matches `model/callee.ts`).
///
/// Untagged so the bare / member / object-run / unknown shapes serialize as
/// flat objects (the TS `Callee` union is structural, no discriminant key beyond
/// `kind`). Each variant carries its own field set.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind")]
pub enum PCallee {
    #[serde(rename = "bare")]
    Bare { name: String },
    #[serde(rename = "member")]
    Member { receiver: String, method: String },
    #[serde(rename = "object-run")]
    ObjectRun {
        #[serde(rename = "objectKind")]
        object_kind: String,
        #[serde(rename = "targetType")]
        target_type: String,
        #[serde(rename = "targetRef", skip_serializing_if = "Option::is_none")]
        target_ref: Option<String>,
        #[serde(rename = "targetIsName")]
        target_is_name: bool,
    },
    #[serde(rename = "unknown")]
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PCallArgumentBinding {
    #[serde(rename = "parameterIndex")]
    pub parameter_index: u32,
    #[serde(rename = "sourceKind")]
    pub source_kind: String,
    #[serde(rename = "sourceVariableName", skip_serializing_if = "Option::is_none")]
    pub source_variable_name: Option<String>,
    #[serde(
        rename = "sourceRecordVariableId",
        skip_serializing_if = "Option::is_none"
    )]
    pub source_record_variable_id: Option<String>,
    #[serde(
        rename = "sourceParameterIndex",
        skip_serializing_if = "Option::is_none"
    )]
    pub source_parameter_index: Option<u32>,
    #[serde(
        rename = "callerSourceParameterIsVar",
        skip_serializing_if = "Option::is_none"
    )]
    pub caller_source_parameter_is_var: Option<bool>,
    #[serde(rename = "sourceTempState", skip_serializing_if = "Option::is_none")]
    pub source_temp_state: Option<PTempState>,
    #[serde(rename = "argumentAnchor")]
    pub argument_anchor: PAnchor,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PCallSite {
    pub id: String,
    #[serde(rename = "operationId")]
    pub operation_id: String,
    #[serde(rename = "calleeText")]
    pub callee_text: String,
    pub callee: PCallee,
    #[serde(rename = "argumentTexts")]
    pub argument_texts: Vec<String>,
    #[serde(rename = "argumentInfos")]
    pub argument_infos: Vec<PExpressionInfo>,
    #[serde(rename = "argumentBindings")]
    pub argument_bindings: Vec<PCallArgumentBinding>,
    #[serde(rename = "loopStack")]
    pub loop_stack: Vec<String>,
    #[serde(rename = "sourceAnchor")]
    pub source_anchor: PAnchor,
    #[serde(rename = "resultConsumed", skip_serializing_if = "Option::is_none")]
    pub result_consumed: Option<bool>,
    #[serde(
        rename = "objectRunReturnUsed",
        skip_serializing_if = "Option::is_none"
    )]
    pub object_run_return_used: Option<bool>,
    #[serde(rename = "underAsserterror", skip_serializing_if = "Option::is_none")]
    pub under_asserterror: Option<bool>,
    /// R1b control-context lattice value (kebab-case string). ABSENT when the
    /// site has no entry (TryFunction / no body / unknown) — matching al-sem's
    /// "assign only when defined" convention.
    #[serde(rename = "controlContext", skip_serializing_if = "Option::is_none")]
    pub control_context: Option<String>,
    /// R1c operation-order index entry. ABSENT when the walk produced no entry
    /// (symbol-only / no-body / TryFunction) — matching al-sem's
    /// "assign only when defined" convention.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub order: Option<OperationOrder>,
    /// True iff this call is the OUTERMOST expression of a bare call statement
    /// (`StmtKind::Call`) — the result is discarded. Expression-position calls
    /// (if-conditions, assignment RHS, arguments) are `false`. Consumed by d53
    /// (ignored TryFunction result).
    ///
    /// INTERNAL-ONLY (`serde(skip)`): never serialized, so every feature-level
    /// golden stays byte-identical; deserialized goldens default it to `false`.
    /// Excluded from PartialEq for the same reason `PVarAssignment.rhs_identifier`
    /// is (baseline vectors deserialize the default and would compare unequal).
    #[serde(skip)]
    pub in_statement_position: bool,
}

/// MANUAL PartialEq: compares exactly the SERIALIZED L2 contract surface.
/// `in_statement_position` is EXCLUDED — derived, serde-skipped internal data
/// (an L5 input, not part of the L2 shape); baseline vectors deserialize it to
/// the default `false`.
impl PartialEq for PCallSite {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
            && self.operation_id == other.operation_id
            && self.callee_text == other.callee_text
            && self.callee == other.callee
            && self.argument_texts == other.argument_texts
            && self.argument_infos == other.argument_infos
            && self.argument_bindings == other.argument_bindings
            && self.loop_stack == other.loop_stack
            && self.source_anchor == other.source_anchor
            && self.result_consumed == other.result_consumed
            && self.object_run_return_used == other.object_run_return_used
            && self.under_asserterror == other.under_asserterror
            && self.control_context == other.control_context
            && self.order == other.order
    }
}
impl Eq for PCallSite {}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct POperationSite {
    pub id: String,
    pub kind: String,
    #[serde(rename = "loopStack")]
    pub loop_stack: Vec<String>,
    #[serde(rename = "sourceAnchor")]
    pub source_anchor: PAnchor,
    #[serde(rename = "underAsserterror", skip_serializing_if = "Option::is_none")]
    pub under_asserterror: Option<bool>,
    /// R1b control-context lattice value (kebab-case string). ABSENT when the
    /// site has no entry (TryFunction / no body / unknown).
    #[serde(rename = "controlContext", skip_serializing_if = "Option::is_none")]
    pub control_context: Option<String>,
    /// R1c operation-order index entry. ABSENT when the walk produced no entry.
    /// For `error-call` ops, populated by the emitter's source-range post-pass
    /// (it inherits the paired callsite's order verbatim).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub order: Option<OperationOrder>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PRecordOperation {
    pub id: String,
    pub op: String,
    #[serde(rename = "recordVariableName")]
    pub record_variable_name: String,
    #[serde(rename = "recordVariableId", skip_serializing_if = "Option::is_none")]
    pub record_variable_id: Option<String>,
    #[serde(rename = "tempState")]
    pub temp_state: PTempState,
    #[serde(rename = "fieldArguments", skip_serializing_if = "Option::is_none")]
    pub field_arguments: Option<Vec<String>>,
    #[serde(rename = "fieldArgumentInfos", skip_serializing_if = "Option::is_none")]
    pub field_argument_infos: Option<Vec<PExpressionInfo>>,
    #[serde(rename = "loopStack")]
    pub loop_stack: Vec<String>,
    #[serde(rename = "sourceAnchor")]
    pub source_anchor: PAnchor,
    /// G-1: `true` when this op sits inside the `until` CONDITION of its nearest
    /// enclosing `repeat` loop — i.e. it IS the loop's own terminator expression
    /// (e.g. the `Next()` in `repeat … until Rec.Next() = 0`), evaluated once per
    /// iteration as loop control, not a db op added to the body.
    ///
    /// INTERNAL-ONLY (`serde(skip)`): never serialized, so every feature-level
    /// golden stays byte-identical; deserialized goldens default it to `false`.
    /// Consumed by d1 to suppress the terminator `Next()` (suppression-direction
    /// safe: an exact structural proof, anything else keeps firing).
    #[serde(skip)]
    pub in_until_condition: bool,
    /// Detector-audit d29 FP-1: the literal `RunTrigger` argument of a mutating
    /// op — `Modify([RunTrigger])` / `Delete([RunTrigger])` /
    /// `DeleteAll([RunTrigger])` (arg 0) and `ModifyAll(Field, Value[, RunTrigger])`
    /// (arg 2). `Some(false)` ⇒ the canonical RunTrigger=false pattern that
    /// SUPPRESSES trigger re-firing (so no recursive-event loop); `Some(true)` ⇒
    /// triggers run; `None` ⇒ no literal arg captured / not a mutating op (the
    /// default — keeps the detector firing). Only `false` (an exact literal) is a
    /// suppression signal.
    ///
    /// INTERNAL-ONLY (`serde(skip)`, excluded from `PartialEq`): never serialized,
    /// so every feature-level golden stays byte-identical and baseline vectors
    /// deserialize it to `None`.
    #[serde(skip)]
    pub run_trigger: Option<bool>,
}

/// MANUAL PartialEq: compares exactly the SERIALIZED L2 contract surface.
/// `in_until_condition` is EXCLUDED — it is derived, serde-skipped internal data
/// (an L5 input, not part of the L2 shape), and baseline vectors deserialize it
/// to the default `false`; including it would make every freshly-walked op in an
/// `until` condition compare unequal to its vector counterpart.
impl PartialEq for PRecordOperation {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
            && self.op == other.op
            && self.record_variable_name == other.record_variable_name
            && self.record_variable_id == other.record_variable_id
            && self.temp_state == other.temp_state
            && self.field_arguments == other.field_arguments
            && self.field_argument_infos == other.field_argument_infos
            && self.loop_stack == other.loop_stack
            && self.source_anchor == other.source_anchor
    }
}

impl Eq for PRecordOperation {}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PLoop {
    pub id: String,
    #[serde(rename = "type")]
    pub loop_type: String,
    #[serde(rename = "sourceAnchor")]
    pub source_anchor: PAnchor,
    /// #49 / #50: for a `repeat` loop whose `until` provably exhausts a cursor,
    /// `(record variable lowercased, op id of that Next)`. Recognised shapes:
    /// `until R.Next(args) = 0` (or `0 = R.Next(args)`, parentheses allowed), and
    /// `until Done` where the loop assigns `Done` exactly once, as a top-level
    /// body statement `Done := R.Next(args) = 0`. Anything else -- a compound
    /// condition, `<> 0`, any other comparison -- is `None`: the loop is not
    /// shown to visit the whole set.
    ///
    /// INTERNAL-ONLY (`serde(skip)`): never serialized, so no feature-level
    /// golden moves; deserialized goldens default it to `None`.
    #[serde(skip)]
    pub exhausting_advance: Option<(String, String)>,
}

/// Excludes the serde-skipped `exhausting_advance`, like the other
/// INTERNAL-ONLY fields: a deserialized golden or vector always holds `None`, so
/// comparing it would fail every repeat loop against its own baseline.
impl PartialEq for PLoop {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
            && self.loop_type == other.loop_type
            && self.source_anchor == other.source_anchor
    }
}

impl Eq for PLoop {}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PRecordVariable {
    pub id: String,
    pub name: String,
    #[serde(rename = "tableName", skip_serializing_if = "Option::is_none")]
    pub table_name: Option<String>,
    #[serde(rename = "tempState")]
    pub temp_state: PTempState,
    #[serde(rename = "isParameter")]
    pub is_parameter: bool,
    #[serde(rename = "parameterIndex", skip_serializing_if = "Option::is_none")]
    pub parameter_index: Option<u32>,
    /// Variable scope: `"local"` | `"parameter"` | `"global"`. `None` when not
    /// yet populated (additive — skip_serializing_if keeps goldens stable).
    /// Populated by later tasks.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
}

/// Variable initializer — a one-hop `ValueSource` projection. Kept as raw JSON
/// because R1a does not constrain its internal shape beyond "matches the TS
/// `VariableSymbol.initializer` value verbatim".
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PVariableSymbol {
    pub name: String,
    #[serde(rename = "declaredType")]
    pub declared_type: String,
    pub scope: String,
    #[serde(rename = "isParameter")]
    pub is_parameter: bool,
    #[serde(rename = "parameterIndex", skip_serializing_if = "Option::is_none")]
    pub parameter_index: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub initializer: Option<serde_json::Value>,
    #[serde(rename = "sourceAnchor")]
    pub source_anchor: PAnchor,
}

/// How a field access uses the field (#14).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FieldAccessKind {
    /// Read in value position -- the default for anything not an assignment target.
    #[default]
    Read,
    /// The target of a plain `:=`: written, not read.
    Write,
    /// The target of a compound `+=` / `-=` / `*=` / `/=`: read, then written.
    ReadWrite,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PFieldAccess {
    #[serde(rename = "recordVariableName")]
    pub record_variable_name: String,
    #[serde(rename = "fieldName")]
    pub field_name: String,
    #[serde(rename = "sourceAnchor")]
    pub source_anchor: PAnchor,
    /// Read, write, or both (#14). Before this, an assignment target was
    /// recorded exactly like a read, so "nothing reads this field" could not be
    /// asked. `R.F := R.F + 1` gives TWO accesses (a Write and a Read).
    ///
    /// INTERNAL-ONLY (`serde(skip)`): never serialized, so no feature-level golden
    /// moves; deserialized goldens default it to `Read`.
    #[serde(skip)]
    pub access: FieldAccessKind,
}

/// Excludes the serde-skipped `access`, like the other INTERNAL-ONLY fields.
impl PartialEq for PFieldAccess {
    fn eq(&self, other: &Self) -> bool {
        self.record_variable_name == other.record_variable_name
            && self.field_name == other.field_name
            && self.source_anchor == other.source_anchor
    }
}

impl Eq for PFieldAccess {}

#[derive(Debug, Clone, Serialize, Deserialize, Eq)]
pub struct PVarAssignment {
    #[serde(rename = "lhsName")]
    pub lhs_name: String,
    #[serde(rename = "rhsLiteralValue", skip_serializing_if = "Option::is_none")]
    pub rhs_literal_value: Option<String>,
    #[serde(rename = "sourceAnchor")]
    pub source_anchor: PAnchor,
    /// G-16: the RHS bare-identifier name (lowercased), set ONLY when BOTH the
    /// assignment target AND the value are bare `identifier` nodes — i.e. a
    /// whole-variable copy like `Cust := Rec`, never a field write
    /// (`Rec.Field := x`) or an expression RHS. Consumed by d11/d21's
    /// record-assign-as-load gate (`RecB := RecA` loads `RecB` when `RecA` is
    /// provably loaded).
    ///
    /// INTERNAL-ONLY (`serde(skip)`): never serialized, so every feature-level
    /// golden stays byte-identical; deserialized goldens default it to `None`.
    #[serde(skip)]
    pub rhs_identifier: Option<String>,
}

/// MANUAL PartialEq: compares exactly the SERIALIZED L2 contract surface.
/// `rhs_identifier` is EXCLUDED — it is derived, serde-skipped internal data
/// (an L5 input, not part of the L2 shape), and baseline vectors deserialize
/// it to the default `None`; including it would make every freshly-walked
/// record-copy assignment compare unequal to its vector counterpart.
impl PartialEq for PVarAssignment {
    fn eq(&self, other: &Self) -> bool {
        self.lhs_name == other.lhs_name
            && self.rhs_literal_value == other.rhs_literal_value
            && self.source_anchor == other.source_anchor
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PConditionReference {
    pub identifier: String,
    #[serde(rename = "conditionKind")]
    pub condition_kind: String,
    #[serde(rename = "statementAnchor")]
    pub statement_anchor: PAnchor,
    #[serde(rename = "referenceAnchor")]
    pub reference_anchor: PAnchor,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PUnreachableStatement {
    pub id: String,
    #[serde(rename = "exitKind")]
    pub exit_kind: String,
    #[serde(rename = "exitAnchor")]
    pub exit_anchor: PAnchor,
    #[serde(rename = "unreachableAnchor")]
    pub unreachable_anchor: PAnchor,
}

/// A simple boolean-guard condition recognized on `if` nodes.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PConditionGuard {
    pub identifier: String,
    pub polarity: String,
}

/// Normalized CFN skeleton node — kind, child/else structure, op/callsite refs,
/// conditionGuard, ordered conditionLeaves. sourceAnchor DROPPED.
///
/// `PartialEq`/`Eq` are implemented MANUALLY (not derived) so the in-memory-only
/// `is_case_else` marker is EXCLUDED from equality — it never serializes, so a
/// freshly-built node (marker set) must still compare equal to a golden node
/// deserialized without it (marker defaulted false). Keeping it out of `==`
/// preserves L2 vector parity (the vectors compare via `PartialEq`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PCFNNode {
    pub kind: String,
    #[serde(rename = "operationId", skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<String>,
    #[serde(rename = "callsiteId", skip_serializing_if = "Option::is_none")]
    pub callsite_id: Option<String>,
    #[serde(rename = "conditionGuard", skip_serializing_if = "Option::is_none")]
    pub condition_guard: Option<PConditionGuard>,
    #[serde(rename = "conditionLeaves", skip_serializing_if = "Option::is_none")]
    pub condition_leaves: Option<Vec<PCFNNode>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub children: Option<Vec<PCFNNode>>,
    #[serde(rename = "elseChildren", skip_serializing_if = "Option::is_none")]
    pub else_children: Option<Vec<PCFNNode>>,
    /// In-memory ONLY (never serialized — `#[serde(skip)]`, so L2 golden parity is
    /// unchanged): marks a `case-branch` node that is the `else`/default branch of
    /// a `case`. al-sem distinguishes it via `sourceAnchor.syntaxKind ===
    /// "case_else_branch"`; the L2 projection drops sourceAnchor, so we carry this
    /// flag for the L4 branch-aware walker's "case without else joins pre" rule.
    /// EXCLUDED from `PartialEq` (see the manual impl below).
    #[serde(skip, default)]
    pub is_case_else: bool,
    /// In-memory ONLY (never serialized — `#[serde(skip)]`): the node's TRUE source
    /// range `(startLine, startColumn, endLine, endColumn)` in the SAME basis as
    /// `PAnchor` (0-based row, utf16 column). al-sem reads `node.sourceAnchor.range`
    /// to attribute field-accesses to the right block level in the branch-aware
    /// walker (`collectFieldAccessesInBlock`); the L2 projection drops sourceAnchor,
    /// so we carry just the range for L4. `None` when unavailable (synthetic nodes).
    /// EXCLUDED from `PartialEq`.
    #[serde(skip, default)]
    pub source_range: Option<(u32, u32, u32, u32)>,
}

impl PartialEq for PCFNNode {
    fn eq(&self, other: &Self) -> bool {
        // `is_case_else` is deliberately omitted — it is an in-memory-only marker.
        self.kind == other.kind
            && self.operation_id == other.operation_id
            && self.callsite_id == other.callsite_id
            && self.condition_guard == other.condition_guard
            && self.condition_leaves == other.condition_leaves
            && self.children == other.children
            && self.else_children == other.else_children
    }
}

impl Eq for PCFNNode {}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PFeatures {
    pub loops: Vec<PLoop>,
    #[serde(rename = "operationSites")]
    pub operation_sites: Vec<POperationSite>,
    #[serde(rename = "recordOperations")]
    pub record_operations: Vec<PRecordOperation>,
    #[serde(rename = "callSites")]
    pub call_sites: Vec<PCallSite>,
    #[serde(rename = "fieldAccesses")]
    pub field_accesses: Vec<PFieldAccess>,
    #[serde(rename = "recordVariables")]
    pub record_variables: Vec<PRecordVariable>,
    #[serde(rename = "nestingDepth")]
    pub nesting_depth: u32,
    #[serde(rename = "hasBranching")]
    pub has_branching: bool,
    #[serde(rename = "unreachableStatements")]
    pub unreachable_statements: Vec<PUnreachableStatement>,
    #[serde(rename = "identifierReferences")]
    pub identifier_references: Vec<String>,
    pub variables: Vec<PVariableSymbol>,
    #[serde(rename = "varAssignments")]
    pub var_assignments: Vec<PVarAssignment>,
    #[serde(rename = "conditionReferences")]
    pub condition_references: Vec<PConditionReference>,
    #[serde(rename = "statementTree", skip_serializing_if = "Option::is_none")]
    pub statement_tree: Option<PCFNNode>,
    /// R1c scope-frame table. OMITTED when empty (TryFunction / no body), but
    /// PRESENT (carrying the root "block" frame) when a body tree exists even with
    /// zero orders — mirrors al-sem `routine-indexer.ts:398`
    /// (`...(scopeFrames.length > 0 ? { scopeFrames } : {})`).
    #[serde(rename = "scopeFrames", default, skip_serializing_if = "Vec::is_empty")]
    pub scope_frames: Vec<ScopeFrame>,
}

/// A projected routine envelope (for the golden files; metadata prerequisite).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PRoutine {
    #[serde(rename = "stableRoutineId")]
    pub stable_routine_id: String,
    pub name: String,
    pub kind: String,
    pub attributes: Vec<String>,
    #[serde(rename = "attributesParsed")]
    pub attributes_parsed: Vec<serde_json::Value>,
    #[serde(rename = "accessModifier", skip_serializing_if = "Option::is_none")]
    pub access_modifier: Option<String>,
    #[serde(rename = "bodyAvailable")]
    pub body_available: bool,
    #[serde(rename = "parseIncomplete")]
    pub parse_incomplete: bool,
    pub features: PFeatures,
    // ── R1d: per-routine DIRECT capability surface ─────────────────────────
    // Siblings of `features` (NOT nested inside it), matching the al-sem golden.
    // ALWAYS present (even when empty / "complete") — the golden never omits a
    // capability key, so we do NOT `skip_serializing_if`. Default fns make the
    // golden round-trip via `Deserialize` even though the dump always emits them.
    #[serde(rename = "capabilityFactsDirect", default)]
    pub capability_facts_direct: Vec<super::capability::CapabilityFact>,
    #[serde(rename = "capabilityStatus", default = "default_capability_status")]
    pub capability_status: super::capability::CoverageStatus,
    #[serde(rename = "capabilityReasons", default)]
    pub capability_reasons: Vec<super::capability::CoverageReason>,
    #[serde(rename = "capabilityDiagnostics", default)]
    pub capability_diagnostics: Vec<super::capability::CapabilityDiagnostic>,
}

/// Default for `capability_status` on deserialize — never used in practice (the
/// golden always emits the key) but required so `PRoutine` derives `Deserialize`.
fn default_capability_status() -> super::capability::CoverageStatus {
    super::capability::CoverageStatus::Complete
}

/// A projected object envelope (for the golden files; metadata prerequisite).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PObject {
    #[serde(rename = "stableObjectId")]
    pub stable_object_id: String,
    pub name: String,
    #[serde(rename = "objectType")]
    pub object_type: String,
    #[serde(rename = "objectSubtype", skip_serializing_if = "Option::is_none")]
    pub object_subtype: Option<String>,
    #[serde(rename = "pageType", skip_serializing_if = "Option::is_none")]
    pub page_type: Option<String>,
    #[serde(rename = "sourceTableName", skip_serializing_if = "Option::is_none")]
    pub source_table_name: Option<String>,
    #[serde(
        rename = "inherentCommitBehavior",
        skip_serializing_if = "Option::is_none"
    )]
    pub inherent_commit_behavior: Option<String>,
}

/// Full L2 projection of a workspace (pre-resolve). Top-level routines sorted by
/// StableRoutineId, objects by StableObjectId.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct L2Projection {
    pub objects: Vec<PObject>,
    pub routines: Vec<PRoutine>,
}

#[cfg(test)]
mod tests {
    use super::{PTempState, known_temp_suppresses, temp_state_suppresses};

    fn ts(kind: &str, value: Option<bool>, param: Option<u32>) -> PTempState {
        PTempState {
            kind: kind.to_string(),
            value,
            parameter_index: param,
        }
    }

    /// #34: the one decision, and the PTempState projection onto it, over every
    /// shape the string-keyed carrier can hold. Only an exact known/true
    /// suppresses; a stray `value: true` on a non-known kind must not.
    #[test]
    fn temp_rule_truth_table_ptempstate() {
        assert!(known_temp_suppresses(Some(true)));
        assert!(!known_temp_suppresses(Some(false)));
        assert!(!known_temp_suppresses(None));

        let cases: [(Option<PTempState>, bool); 7] = [
            (None, false),                                           // absent
            (Some(ts("known", Some(true), None)), true),             // known/true
            (Some(ts("known", Some(false), None)), false),           // known/false
            (Some(ts("known", None, None)), false),                  // known, value missing
            (Some(ts("unknown", None, None)), false),                // unknown
            (Some(ts("unknown", Some(true), None)), false),          // unknown, stray true
            (Some(ts("parameter-dependent", None, Some(0))), false), // param-dependent
        ];
        for (state, want) in cases {
            assert_eq!(temp_state_suppresses(state.as_ref()), want, "{state:?}");
        }
        // The projection keeps known/missing distinguishable from "not known".
        assert_eq!(ts("known", None, None).known_value(), Some(false));
        assert_eq!(ts("unknown", Some(true), None).known_value(), None);
    }
    /// The module doc's INTERNAL-ONLY list must name exactly the fields this file
    /// marks `#[serde(skip)]` / `#[serde(skip, ...)]` (#15), so it cannot drift.
    #[test]
    fn serde_skip_list_is_complete() {
        let src = include_str!("features.rs");
        let documented: Vec<&str> = src
            .lines()
            .filter_map(|l| l.strip_prefix("//!   - `"))
            .filter_map(|l| l.strip_suffix('`'))
            .filter(|l| l.starts_with('P') && l.contains('.'))
            .collect();
        let mut actual = Vec::new();
        let (mut owner, mut pending) = ("", false);
        for line in src.lines().map(str::trim) {
            if let Some(rest) = line.strip_prefix("pub struct ") {
                owner = rest.split([' ', '{']).next().unwrap();
            } else if line.starts_with("#[serde(skip)") || line.starts_with("#[serde(skip,") {
                pending = true;
            } else if pending && let Some(rest) = line.strip_prefix("pub ") {
                actual.push(format!("{owner}.{}", rest.split(':').next().unwrap()));
                pending = false;
            }
        }
        assert_eq!(actual.len(), 8, "scanner found {actual:?}");
        assert_eq!(documented, actual);
    }
}

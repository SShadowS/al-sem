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
//!   - `PCallSite.result_tested`
//!   - `PCallSite.receiver_temp_state`
//!   - `PCallSite.rare`
//!   - `PCallSite.start_may_fail`
//!   - `PCallSite.checked_run`
//!   - `PCallSite.write_txn_query`
//!   - `PCallSite.try_call`
//!   - `PRecordOperation.in_until_condition`
//!   - `PRecordOperation.in_statement_position`
//!   - `PRecordOperation.run_trigger`
//!   - `PRecordOperation.receiver_is_record_ref`
//!   - `PRecordOperation.holder_arguments`
//!   - `PLoop.exhausting_advance`
//!   - `PRecordVariable.is_receiver`
//!   - `PVariableSymbol.fixed_local`
//!   - `PFieldAccess.access`
//!   - `PVarAssignment.rhs_identifier`
//!   - `PVarAssignment.rhs_member`
//!   - `PVarAssignment.lhs_is_member`
//!   - `PVarAssignment.lhs_text`
//!   - `PVarAssignment.rhs_text`
//!   - `PVarAssignment.ternary_leaf`
//!   - `PCFNNode.rare`
//!   - `PCFNNode.source_range`
//!   - `PCFNNode.flow`

use super::operation_order::{OperationOrder, ScopeFrame};
use al_syntax::IdentifierFoldExt;
use serde::{Deserialize, Serialize};

/// A projected source anchor. Columns are tree-sitter byte columns (which match
/// al-sem's web-tree-sitter columns byte-for-byte — see `node_util::Utf16Cols`);
/// `enclosingRoutineId` is dropped (it embeds the modelInstanceId-dependent id).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PAnchor {
    /// ENG-85: shared, not owned: every anchor of a file holds the same text
    /// (`SharedStr` compares, prints and serializes as its text).
    #[serde(rename = "sourceUnitId")]
    pub source_unit_id: crate::program::str_pool::SharedStr,
    #[serde(rename = "startLine")]
    pub start_line: u32,
    #[serde(rename = "startColumn")]
    pub start_column: u32,
    #[serde(rename = "endLine")]
    pub end_line: u32,
    #[serde(rename = "endColumn")]
    pub end_column: u32,
    /// ENG-85: one shared text per grammar kind ([`syntax_kind_of`]).
    #[serde(rename = "syntaxKind")]
    pub syntax_kind: crate::program::str_pool::SharedStr,
}

/// ENG-85: the file id for an anchor, shared with the previous anchor built on this thread
/// when the text is the same (a walk builds one file's anchors in a row), so a file's anchors
/// hold one allocation, not one each.
pub fn anchor_unit(unit: &str) -> crate::program::str_pool::SharedStr {
    use crate::program::str_pool::SharedStr;
    thread_local! {
        static LAST: std::cell::RefCell<Option<SharedStr>> = const { std::cell::RefCell::new(None) };
    }
    LAST.with(|last| {
        let mut last = last.borrow_mut();
        match last.as_ref() {
            Some(s) if s.as_str() == unit => s.clone(),
            _ => {
                let s = SharedStr::from(unit);
                *last = Some(s.clone());
                s
            }
        }
    })
}

/// ENG-85: a text from a closed vocabulary (an anchor's syntax kind, an expression's kind, an
/// argument's source kind), one allocation per text per thread. Only for closed sets: the
/// thread keeps every text it saw.
pub fn shared_vocab(kind: &str) -> crate::program::str_pool::SharedStr {
    thread_local! {
        static KINDS: SharedSet = SharedSet::default();
    }
    KINDS.with(|kinds| shared_in(kinds, SHARED_VOCAB_CAP, kind))
}

/// Texts [`shared_vocab`] keeps per thread: well above the grammar's kind names (~1,500)
/// plus the expression and argument kinds.
pub const SHARED_VOCAB_CAP: usize = 4096;

/// ENG-96: a record variable's table name or table id, one allocation per text per thread.
/// User text (a workspace's tables), so its own set and cap: it never crowds the closed
/// vocabularies out of [`shared_vocab`]'s (Fable, ENG-96).
pub fn shared_table_text(text: &str) -> crate::program::str_pool::SharedStr {
    thread_local! {
        static TABLES: SharedSet = SharedSet::default();
    }
    TABLES.with(|tables| shared_in(tables, SHARED_TABLE_TEXT_CAP, text))
}

/// ENG-108: a coverage record's status word, reason or unknown target, one allocation per text
/// per thread. Its own set and cap (user text: an unknown target is a routine id), so it never
/// crowds the other sets out. On CDO the three coverage stores repeat 3 reasons and 188 targets.
pub fn shared_coverage_text(text: &str) -> crate::program::str_pool::SharedStr {
    thread_local! {
        static COVERAGE: SharedSet = SharedSet::default();
    }
    COVERAGE.with(|texts| shared_in(texts, SHARED_COVERAGE_TEXT_CAP, text))
}

/// Texts [`shared_coverage_text`] keeps per thread; past it a text is still right, only no
/// longer shared.
pub const SHARED_COVERAGE_TEXT_CAP: usize = 16_384;

/// Texts [`shared_table_text`] keeps per thread: CDO with its dependencies has about 2,300
/// (1,149 table names, 1,120 table ids); a workspace past this still answers right, it only
/// stops sharing the further texts.
pub const SHARED_TABLE_TEXT_CAP: usize = 16_384;

type SharedSet = std::cell::RefCell<crate::fxhash::FxHashSet<crate::program::str_pool::SharedStr>>;

/// `text`'s shared copy in `set`, added while the set holds fewer than `cap` texts.
fn shared_in(set: &SharedSet, cap: usize, text: &str) -> crate::program::str_pool::SharedStr {
    let mut set = set.borrow_mut();
    if let Some(s) = set.get(text) {
        return s.clone();
    }
    let s = crate::program::str_pool::SharedStr::from(text);
    // Bounded however it is called: past the cap a text is no longer kept (it is still
    // right, just not shared), so a long-running engine thread never grows this set.
    if set.len() < cap {
        set.insert(s.clone());
    }
    s
}

impl PAnchor {
    /// 0-based containment: is `(line, column)` within `[start, end]` (inclusive)?
    /// The file is not compared; callers check `source_unit_id` themselves.
    pub fn contains(&self, line: u32, column: u32) -> bool {
        (self.start_line, self.start_column) <= (line, column)
            && (line, column) <= (self.end_line, self.end_column)
    }

    /// Total order on EXTENT (smaller = tighter container): spanned lines, then
    /// spanned columns. Picks the smallest of several anchors containing a point.
    pub fn extent_cmp(&self, other: &PAnchor) -> std::cmp::Ordering {
        let lines = |a: &PAnchor| a.end_line.saturating_sub(a.start_line);
        let cols = |a: &PAnchor| a.end_column.saturating_sub(a.start_column);
        lines(self)
            .cmp(&lines(other))
            .then_with(|| cols(self).cmp(&cols(other)))
    }
}

/// Structured expression classification (`model/expression.ts`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PExpressionInfo {
    /// ENG-85: a closed vocabulary ([`shared_vocab`]).
    pub kind: crate::program::str_pool::SharedStr,
    /// ENG-85: shared with `value` when they are the same text.
    pub text: crate::program::str_pool::SharedStr,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<crate::program::str_pool::SharedStr>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub qualifier: Option<crate::program::str_pool::SharedStr>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub member: Option<crate::program::str_pool::SharedStr>,
}

crate::open_vocab!(
    /// [`PCallSite::control_context`]'s words, the lattice values of
    /// `control_context::ControlContext::as_str` (ENG-96); any other word is kept as `Other`.
    pub ControlContextWord {
        TopLevel => "top-level",
        Conditional => "conditional",
        LoopBody => "loop-body",
        IsHandledGuarded => "is-handled-guarded",
        ErrorPath => "error-path",
        Unreachable => "unreachable",
    }
);

crate::open_vocab!(
    /// [`PTempState::kind`]'s words (ENG-96); any other word is kept as `Other`.
    pub TempKind {
        Known => "known",
        ParameterDependent => "parameter-dependent",
        Unknown => "unknown",
    }
);

/// Temp-state of a record variable / op.
///
/// `{ kind: "known", value }` | `{ kind: "parameter-dependent", parameterIndex }`
/// | `{ kind: "unknown" }`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct PTempState {
    /// ENG-96: a 4-byte word ([`TempKind`]), compared, ordered and serialized as its text.
    pub kind: TempKind,
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
    /// ENG-85: a closed vocabulary ([`shared_vocab`]).
    #[serde(rename = "sourceKind")]
    pub source_kind: crate::program::str_pool::SharedStr,
    #[serde(rename = "sourceVariableName", skip_serializing_if = "Option::is_none")]
    pub source_variable_name: Option<String>,
    /// ENG-85: shares the record variable's own id.
    #[serde(
        rename = "sourceRecordVariableId",
        skip_serializing_if = "Option::is_none"
    )]
    pub source_record_variable_id: Option<crate::program::str_pool::SharedStr>,
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
    /// ENG-96: a 4-byte word ([`ControlContextWord`]), serialized as its text.
    #[serde(rename = "controlContext", skip_serializing_if = "Option::is_none")]
    pub control_context: Option<ControlContextWord>,
    /// R1c operation-order index entry. ABSENT when the walk produced no entry
    /// (symbol-only / no-body / TryFunction) — matching al-sem's
    /// "assign only when defined" convention. ENG-96: boxed, as no model row
    /// carries one (CDO: 0 of 160,213); serialized as before.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub order: Option<Box<OperationOrder>>,
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
    /// True iff the call's value, looking through parentheses and `not`, is
    /// the WHOLE right-hand side of a plain assignment (`Ok := F()`) or the
    /// WHOLE condition of an `if`, `while` or `repeat … until`. These are the
    /// shapes Microsoft Learn names for a try method call whose errors are
    /// caught ("Handling errors using try methods": "`OK:=` statement or a
    /// conditional statement such as `if-then`"). Any other use (an
    /// argument, `exit(F())`, a part of `a and F()`) is `false`: Learn does
    /// not say it traps, and its argument example (`DoWork(DoInsert(Rec))`)
    /// points the other way. Read by the one trap rule
    /// (`engine::l5::traps::call_trap`, engine limits B7).
    ///
    /// INTERNAL-ONLY (`serde(skip)`, excluded from PartialEq), as
    /// `in_statement_position`; deserialized rows default it to `false` (no
    /// trap: the safe side).
    #[serde(skip)]
    pub result_tested: bool,
    /// The temp state of the RECORD this call is made on, when the callee may be
    /// a table method: the member receiver (`Buf.ClearBuffer()` -> `Buf`), or the
    /// caller's implicit `Rec` for a bare call. A table method's implicit `Rec` is
    /// `ParameterDependent(RECEIVER_PARAM_INDEX)`, and this is the value the PD
    /// substitution reads for that index ([`PCallSite::source_temp_state_for`]).
    /// `None` when the receiver is not a record variable the caller declares.
    ///
    /// INTERNAL-ONLY (`serde(skip)`, excluded from PartialEq), as
    /// `in_statement_position`.
    #[serde(skip)]
    pub receiver_temp_state: Option<PTempState>,
    /// The facts almost no call has ([`PCallSiteRare`]), boxed so the rest pay one pointer
    /// (ENG-96: a byte here is 0.7 MiB on CDO; ENG-175 shares this box). Read them through
    /// [`Self::implicit_receiver`] and [`Self::ternary_holders`].
    ///
    /// INTERNAL-ONLY (`serde(skip)`, excluded from PartialEq), as
    /// `in_statement_position`; deserialized rows default it to `None`.
    #[serde(skip)]
    pub rare: Option<Box<PCallSiteRare>>,
    /// True iff the resolver decided this call starts an XmlPort in a way that
    /// may fail (ENG-08, measured on BC 28): an IMPORT (an `XmlPort.Import`, or
    /// an `XmlPort.Run` whose `Direction` allows one, `dataset::import_export`),
    /// which fails on its input whatever `AutoUpdate`/`AutoReplace` say (a
    /// value that does not evaluate, malformed XML, a key the file holds
    /// twice); or an explicit import or export its `Direction` refuses
    /// (`dataset::direction_refused`: "designed for import only"). Keyword or
    /// variable form. Set by `program_calls` after resolution, never by the
    /// body walk; in statement position it raises an error
    /// (`ui_events_error::implicit_error_call`).
    ///
    /// INTERNAL-ONLY (`serde(skip)`, excluded from PartialEq), as
    /// `in_statement_position`; deserialized rows default it to `false`.
    #[serde(skip)]
    pub start_may_fail: bool,
    /// True iff the resolver decided this call is a checked `Codeunit.Run`
    /// (`traps::checked_run`: its result used; the keyword form, or a codeunit
    /// variable's `.Run()` the resolver decided starts the codeunit). Such a run
    /// commits its own changes
    /// when it ends without an error, and no `[CommitBehavior]` changes that
    /// (ENG-09, Learn, measured on BC 28), so the capability cone gives the
    /// site a commit of its own. Set by `program_calls` after resolution,
    /// never by the body walk.
    ///
    /// INTERNAL-ONLY (`serde(skip)`, excluded from PartialEq), as
    /// `in_statement_position`; deserialized rows default it to `false`.
    #[serde(skip)]
    pub checked_run: bool,
    /// True iff the resolver decided this call is the platform's
    /// `IsInWriteTransaction()` (bare or `Database.`), not a procedure of
    /// that name. ENG-22: `if IsInWriteTransaction() then Commit();` ends the
    /// open write transaction on both paths (measured on BC 28: the method
    /// is true exactly when a checked run would fail), so the pending-write
    /// walk reads that statement as a `Commit()`. Set by `program_calls`
    /// after resolution, never by the body walk.
    ///
    /// INTERNAL-ONLY (`serde(skip)`, excluded from PartialEq), as
    /// `in_statement_position`; deserialized rows default it to `false`.
    #[serde(skip)]
    pub write_txn_query: bool,
    /// True iff the resolver decided this call may start a `[TryFunction]`
    /// (any resolved callee, any `#if` arm declaring it) and its value is used
    /// (not [`Self::in_statement_position`]). ENG-10 (measured on BC 28 on
    /// premises, Windows and the Linux sidecar): with
    /// `DisableWriteInsideTryFunctions` on, a database write the call reaches
    /// raises an error the try call does not catch; every use of the value
    /// counts (`Ok :=`, a condition, `exit(..)`, an argument, `a and ..`), only
    /// a call statement is ordinary. Wider than [`Self::result_tested`] on
    /// purpose: catching needs a definite trap, the refusal a possible one.
    /// Set by `program_calls` after resolution, never by the body walk.
    ///
    /// INTERNAL-ONLY (`serde(skip)`, excluded from PartialEq), as
    /// `in_statement_position`; deserialized rows default it to `false`.
    #[serde(skip)]
    pub try_call: bool,
}

/// [`PCallSite::rare`]: the facts almost no call site has (CDO: `implicit_receiver` on 4 of
/// 160,213 call sites).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PCallSiteRare {
    /// For a BARE `SetAutoCalcFields` call, the record it is made on: the
    /// receiver text of the innermost implicit frame that
    /// is a record, `with X do`'s `X` or the object's implicit `Rec`, the same
    /// frame a bare record operation takes its `record_variable_name` from.
    /// AL looks a bare name up among the `with` record's members first, then
    /// outward, so a `with` on a non-record does not hide the outer `Rec`.
    /// `None` for every other call, for a `SetAutoCalcFields` that is one of
    /// the object's own routines, and where no record frame is in scope. Read
    /// by the FlowField rule for a bare `SetAutoCalcFields`
    /// (`program::model::flow_fields`, engine limits B5).
    pub implicit_receiver: Option<String>,
    /// ENG-175: each whole variable (as written) that is a leaf of a conditional expression
    /// (`c ? X : Y`, nested and parenthesized ones included) used as one of the call's arguments
    /// or as the receiver of a method that binds it (`recref_bind`). BC 28.4: the value is the
    /// chosen branch's handle, so the call may rebind or keep any of them.
    pub ternary_holders: Vec<String>,
}

/// The parameter index a table method's implicit `Rec` depends on: `Rec` is the
/// record the method is CALLED ON, so its temporariness is the caller's choice,
/// exactly like a keyword-less `var` record parameter (engine-switch S8, triage D:
/// `Buf.ClearBuffer()` on a `temporary` buffer was counted a physical
/// `DeleteAll`). Reserved: no real parameter list reaches it.
pub const RECEIVER_PARAM_INDEX: u32 = u32::MAX;

impl PCallSite {
    /// The caller-side temp state bound to callee parameter `i`: the argument
    /// binding's source state, or the receiver's for [`RECEIVER_PARAM_INDEX`].
    /// `None` when nothing is bound (every consumer reads that as Unknown).
    #[must_use]
    pub fn source_temp_state_for(&self, i: u32) -> Option<&PTempState> {
        if i == RECEIVER_PARAM_INDEX {
            return self.receiver_temp_state.as_ref();
        }
        self.argument_bindings
            .iter()
            .find(|b| b.parameter_index == i)
            .and_then(|b| b.source_temp_state.as_ref())
    }
}

/// MANUAL PartialEq: compares exactly the SERIALIZED L2 contract surface.
/// `in_statement_position` is EXCLUDED — derived, serde-skipped internal data
/// (an L5 input, not part of the L2 shape); baseline vectors deserialize it to
/// the default `false`.
/// How a RecordRef method binds the RecordRef it runs on: the one list (ENG-160), measured on
/// BC 28.4 (`docs/superpowers/notes/2026-11-20-eng160-recordref-table-moves.md` and ENG-145's
/// note). Through every alias of it, as `GetTable` (one handle). `Copy` moves neither the table
/// (another table is a runtime error) nor the temporary state; `GetBySystemId` looks only in its
/// own table; neither is here. Re-exported by `program::model::recref_alias`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecRefBind {
    /// `Open(No[, Temp])`: the table its first argument names.
    Open,
    /// `GetTable(Rec)`: its argument's table and temporary state.
    GetTable,
    /// `Get(RecordId)`: the RecordId's table when it finds the record (a different table is
    /// physical, the same table keeps its state), else nothing: any table, state unknown.
    Get,
    /// `Close`: no table.
    Close,
}

/// [`RecRefBind`] of a method name (any case).
#[must_use]
pub fn recref_bind(method: &str) -> Option<RecRefBind> {
    Some(match method.trim().fold_identifier().as_str() {
        "open" => RecRefBind::Open,
        "gettable" => RecRefBind::GetTable,
        "get" => RecRefBind::Get,
        "close" => RecRefBind::Close,
        _ => return None,
    })
}

impl PCallSite {
    /// [`PCallSiteRare::implicit_receiver`].
    #[must_use]
    pub fn implicit_receiver(&self) -> Option<&str> {
        self.rare.as_deref()?.implicit_receiver.as_deref()
    }

    /// [`PCallSiteRare::ternary_holders`], empty when there is none.
    #[must_use]
    pub fn ternary_holders(&self) -> &[String] {
        self.rare
            .as_deref()
            .map_or(&[], |r| r.ternary_holders.as_slice())
    }
}

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
    /// True iff the op is the outermost expression of a bare call statement, so
    /// its "ok" Boolean is discarded (`PCallSite.in_statement_position`'s twin).
    /// G20: an unguarded lookup (`Get`, `FindFirst`, …) or write (`Insert`,
    /// `Modify`, …) raises an error when it fails
    /// (`ui_events_error::implicit_error_record_op`). INTERNAL-ONLY like `in_until_condition`
    /// (`serde(skip)`, excluded from `PartialEq`).
    #[serde(skip)]
    pub in_statement_position: bool,
    /// Detector-audit d29 FP-1: the literal `RunTrigger` argument of a mutating
    /// op — `Insert([RunTrigger])` / `Modify([RunTrigger])` / `Delete([RunTrigger])` /
    /// `DeleteAll([RunTrigger])` (arg 0) and `ModifyAll(Field, Value[, RunTrigger])`
    /// (arg 2). `Some(false)` ⇒ the canonical RunTrigger=false pattern that
    /// SUPPRESSES trigger re-firing (so no recursive-event loop); `Some(true)` ⇒
    /// triggers run; `None` ⇒ no literal arg captured / not a mutating op (the
    /// default — keeps the detector firing). Only `false` (an exact literal) is a
    /// suppression signal. The typed `implicit-trigger` edge's `fires` also reads
    /// it (#96: `Some(true)` ⇒ the trigger always fires).
    ///
    /// INTERNAL-ONLY (`serde(skip)`, excluded from `PartialEq`): never serialized,
    /// so every serde-based feature golden stays byte-identical and baseline
    /// vectors deserialize it to `None`. The `Debug`-printed ir-l2 snapshot
    /// (`tests/ir-l2-goldens/l2_features.snapshot`) does show it.
    #[serde(skip)]
    pub run_trigger: Option<bool>,
    /// MCP output wave 3a (Q4): the receiver is a variable declared `RecordRef`
    /// in the routine's lexical scope (parameters and locals before the
    /// object's globals), so the operation's table is decided at run time.
    /// The body walk decides it where it captures the operation, from the
    /// declaration it already reads for receiver eligibility; an implicit
    /// receiver (`Rec`, a `with` frame) is never a RecordRef. Read by `alsem
    /// query` to say "RecordRef" only where the receiver really is one.
    ///
    /// INTERNAL-ONLY (`serde(skip)`, excluded from `PartialEq`), like
    /// `run_trigger`; the `Debug`-printed ir-l2 snapshot shows it.
    #[serde(skip)]
    pub receiver_is_record_ref: bool,
    /// ENG-173: the arguments (as written) that name a RecordRef or Variant variable in the
    /// routine's scope; `None` when there is none (boxed: almost every operation has none). A
    /// table procedure named like the operation may take the call (BC 28.4: it overloads the
    /// platform method) and keep or rebind them (`ModelRecordOperation::holder_arguments`).
    ///
    /// INTERNAL-ONLY (`serde(skip)`, excluded from `PartialEq`), like `run_trigger`.
    #[serde(skip)]
    pub holder_arguments: Option<Box<Vec<String>>>,
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

#[derive(Debug, Clone, Serialize, Deserialize, Eq)]
pub struct PRecordVariable {
    /// ENG-85: the variable's shared id ([`super::scope::RecordVariable::id`]).
    pub id: crate::program::str_pool::SharedStr,
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
    /// #299: this row is the implicit receiver `Rec` of a Table / TableExtension
    /// routine (the record the routine was called or fired on), set only where
    /// `ir_record_variables` seeds it. Not serialized: a deserialized row reads
    /// `false`, so every reader fails closed (the detectors keep reporting).
    #[serde(skip)]
    pub is_receiver: bool,
}

/// MANUAL PartialEq: compares exactly the SERIALIZED L2 contract surface.
/// `is_receiver` is EXCLUDED — serde-skipped internal data that a deserialized
/// baseline row reads as `false`; including it would make every freshly-walked
/// table receiver compare unequal to its vector counterpart (as `PVarAssignment`).
impl PartialEq for PRecordVariable {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
            && self.name == other.name
            && self.table_name == other.table_name
            && self.temp_state == other.temp_state
            && self.is_parameter == other.is_parameter
            && self.parameter_index == other.parameter_index
            && self.scope == other.scope
    }
}

/// Variable initializer — a one-hop `ValueSource` projection. Kept as raw JSON
/// because R1a does not constrain its internal shape beyond "matches the TS
/// `VariableSymbol.initializer` value verbatim".
#[derive(Debug, Clone, Serialize, Deserialize, Eq)]
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
    /// `Some` only for a LOCAL declared exactly once in its routine, with no
    /// parameter and no object global of the same name (G52). The IR keeps both
    /// arms of an `#if`, so only then is this declaration, its type and its value,
    /// the one every compile of the routine sees. `None` for everything else.
    ///
    /// INTERNAL-ONLY (`serde(skip)`, excluded from PartialEq), as
    /// `PCallSite.in_statement_position`.
    #[serde(skip)]
    pub fixed_local: Option<FixedLocal>,
}

impl PartialEq for PVariableSymbol {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
            && self.declared_type == other.declared_type
            && self.scope == other.scope
            && self.is_parameter == other.is_parameter
            && self.parameter_index == other.parameter_index
            && self.initializer == other.initializer
            && self.source_anchor == other.source_anchor
    }
}

/// What a [`PVariableSymbol::fixed_local`] knows beyond the declared type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FixedLocal {
    /// The text of a LOCKED label (`X: Label '…', Locked = true;`), the one kind
    /// of variable whose value is a constant no translation can change. `None`
    /// for every other declaration, an unlocked label included.
    pub constant: Option<String>,
}

/// The constant text a declaration gives its variable: a locked label's text,
/// else `None` (see [`FixedLocal::constant`]).
pub fn label_constant_of(label: Option<&al_syntax::ir::LabelValue>) -> Option<String> {
    label.filter(|l| l.locked).and_then(|l| l.text.clone())
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
    /// G34: `(X, M)` folded, set ONLY when the target is a bare identifier and
    /// the value is `X.M` or `X.M()` on a bare identifier `X` (`R :=
    /// Other.Duplicate()`, `Id := R.RecordId`). INTERNAL-ONLY like
    /// `rhs_identifier`, and excluded from `PartialEq` for the same reason.
    #[serde(skip)]
    pub rhs_member: Option<(String, String)>,
    /// ENG-27: the target is a member (`X.F := …`), so `lhs_name` is the
    /// member's name, not a variable's. INTERNAL-ONLY like `rhs_identifier`,
    /// and excluded from `PartialEq` for the same reason.
    #[serde(skip)]
    pub lhs_is_member: bool,
    /// ENG-176: the target's source text as written (`this.G`, `"My Ref"`, `Slot[1]`), for a
    /// reader to resolve with `holder_key::HolderScope::text`, which applies AL's scoping
    /// (`lhs_name` is folded and cannot tell a hidden global from the local hiding it).
    /// INTERNAL-ONLY like `rhs_identifier`, and excluded from `PartialEq`.
    #[serde(skip)]
    pub lhs_text: String,
    /// ENG-176: the value's source text as written, set whenever the value spells a whole
    /// variable (an identifier or `this.X`, through parentheses), whatever the target: resolved
    /// like `lhs_text`. INTERNAL-ONLY like `rhs_identifier`.
    #[serde(skip)]
    pub rhs_text: Option<String>,
    /// ENG-175: the row is one leaf of a conditional expression (`A := c ? X : Y` gives a row per
    /// leaf, all at the statement's anchor): it runs only when its branch is chosen, so a reader
    /// that needs an assignment to run for certain (d43's "must set") reads it as conditional.
    ///
    /// INTERNAL-ONLY (`serde(skip)`, excluded from PartialEq).
    #[serde(skip)]
    pub ternary_leaf: bool,
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
/// `rare` / `source_range` / `flow` markers are EXCLUDED from equality. They never serialize, so a freshly-built node (markers set) must
/// still compare equal to a golden node deserialized without them (markers
/// defaulted). Keeping them out of `==` preserves L2 vector parity (the vectors
/// compare via `PartialEq`).
///
/// `Debug` is written by hand (ENG-95): it prints the [`PCFNRare`] facts flat, in their
/// old place, so the `l2_features.snapshot` golden (a `Debug` print) still pins each fact.
#[derive(Clone, Serialize, Deserialize)]
pub struct PCFNNode {
    pub kind: String,
    #[serde(rename = "operationId", skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<String>,
    #[serde(rename = "callsiteId", skip_serializing_if = "Option::is_none")]
    pub callsite_id: Option<String>,
    /// Boxed (ENG-95): on 2.5 % of the nodes, 48 B inline otherwise.
    #[serde(rename = "conditionGuard", skip_serializing_if = "Option::is_none")]
    pub condition_guard: Option<Box<PConditionGuard>>,
    #[serde(rename = "conditionLeaves", skip_serializing_if = "Option::is_none")]
    pub condition_leaves: Option<Vec<PCFNNode>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub children: Option<Vec<PCFNNode>>,
    #[serde(rename = "elseChildren", skip_serializing_if = "Option::is_none")]
    pub else_children: Option<Vec<PCFNNode>>,
    /// In-memory ONLY (never serialized — `#[serde(skip)]`, ENG-95): the facts few nodes
    /// carry ([`PCFNRare`]: `is_case_else`, `path_hint`, `table_case`, the exit guards,
    /// `junction_guards`, `value_fact`), boxed so the rest pay 8 bytes, not 203. Read them
    /// with the accessors of the same names; set them with [`PCFNNode::rare_mut`].
    /// EXCLUDED from `PartialEq` (see the manual impl below).
    #[serde(skip, default)]
    pub rare: Option<Box<PCFNRare>>,
    /// In-memory ONLY (never serialized — `#[serde(skip)]`): the node's TRUE source
    /// range `(startLine, startColumn, endLine, endColumn)` in the SAME basis as
    /// `PAnchor` (0-based row, utf16 column). al-sem reads `node.sourceAnchor.range`
    /// to attribute field-accesses to the right block level in the branch-aware
    /// walker (`collectFieldAccessesInBlock`); the L2 projection drops sourceAnchor,
    /// so we carry just the range for L4. `None` when unavailable (synthetic nodes).
    /// EXCLUDED from `PartialEq`.
    #[serde(skip, default)]
    pub source_range: Option<(u32, u32, u32, u32)>,
    /// In-memory ONLY (r3 E2): what the within-routine value flow reads from
    /// the node ([`FlowFact`]). EXCLUDED from `PartialEq`.
    #[serde(skip, default)]
    pub flow: Option<Box<FlowFact>>,
}

/// ENG-95: a [`PCFNNode`]'s in-memory facts that few nodes carry (on CDO's dependency
/// trees each is on at most 3.6 % of the nodes), kept in one box. In-memory only, like
/// the node's other markers.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PCFNRare {
    /// Marks a `case-branch` node that is the `else`/default branch of a `case`.
    /// al-sem distinguishes it via `sourceAnchor.syntaxKind === "case_else_branch"`;
    /// the L2 projection drops sourceAnchor, so we carry this flag for the L4
    /// branch-aware walker's "case without else joins pre" rule.
    pub is_case_else: bool,
    /// #343: what a must-path walk needs that the kind does not say ([`PathHint`]).
    pub path_hint: Option<PathHint>,
    /// G33: a `case` on a RecordRef's table number, or one of its branches' tables
    /// ([`TableCase`]).
    pub table_case: Option<TableCase>,
    /// G36: on an `if` with no `else` whose condition is `not R.FieldExist(N)` and
    /// whose then-branch is only `exit`: `(R folded, N)`. Past it, `R` holds a table
    /// that has field `N`.
    pub field_exit_guard: Option<(String, i64)>,
    /// G34: on an `if` with no `else` whose condition is `R.Number <> Database::"T"`
    /// (either side, through parentheses) and whose then-branch is only `exit`:
    /// `(R folded, [T folded])`. Past it, `R` holds table `T`.
    pub table_exit_guard: Option<(String, Vec<String>)>,
    /// Round-3 m04: on an `if` whose condition is not one simple guard
    /// (`condition_guard` is `None`) but an `and` or an `or` chain (through
    /// parentheses) with simple guards among its operands ([`JunctionGuards`]).
    pub junction_guards: Option<JunctionGuards>,
    /// r3 E1: what the node says about an Integer, Option or Enum value
    /// ([`ValueFact`]).
    pub value_fact: Option<ValueFact>,
}

impl std::fmt::Debug for PCFNNode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PCFNNode")
            .field("kind", &self.kind)
            .field("operation_id", &self.operation_id)
            .field("callsite_id", &self.callsite_id)
            .field("condition_guard", &self.condition_guard)
            .field("condition_leaves", &self.condition_leaves)
            .field("children", &self.children)
            .field("else_children", &self.else_children)
            .field("is_case_else", &self.is_case_else())
            .field("source_range", &self.source_range)
            .field("path_hint", &self.path_hint())
            .field("table_case", &self.table_case())
            .field("field_exit_guard", &self.field_exit_guard())
            .field("table_exit_guard", &self.table_exit_guard())
            .field("junction_guards", &self.junction_guards())
            .field("value_fact", &self.value_fact())
            .field("flow", &self.flow)
            .finish()
    }
}

impl PCFNNode {
    /// The rare facts, created on first write (ENG-95).
    pub fn rare_mut(&mut self) -> &mut PCFNRare {
        self.rare.get_or_insert_default()
    }
    pub fn is_case_else(&self) -> bool {
        self.rare.as_ref().is_some_and(|r| r.is_case_else)
    }
    pub fn path_hint(&self) -> Option<PathHint> {
        self.rare.as_ref().and_then(|r| r.path_hint)
    }
    pub fn table_case(&self) -> Option<&TableCase> {
        self.rare.as_ref().and_then(|r| r.table_case.as_ref())
    }
    pub fn field_exit_guard(&self) -> Option<&(String, i64)> {
        self.rare.as_ref().and_then(|r| r.field_exit_guard.as_ref())
    }
    pub fn table_exit_guard(&self) -> Option<&(String, Vec<String>)> {
        self.rare.as_ref().and_then(|r| r.table_exit_guard.as_ref())
    }
    pub fn junction_guards(&self) -> Option<&JunctionGuards> {
        self.rare.as_ref().and_then(|r| r.junction_guards.as_ref())
    }
    pub fn value_fact(&self) -> Option<&ValueFact> {
        self.rare.as_ref().and_then(|r| r.value_fact.as_ref())
    }
}

/// r3 E2: an expression as the within-routine value flow
/// (`literal_reach`'s flow pass) reads it. Everything the pass does not model
/// is [`FlowExpr::Unknown`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FlowExpr {
    /// `true` / `false`.
    Bool(bool),
    /// An integer literal or a qualified label (resolved by the reader).
    Label(ValueLabel),
    /// A bare identifier (quoted or not), unquoted and folded. A call written
    /// without parentheses is never one.
    Var(String),
    Not(Box<FlowExpr>),
    And(Box<FlowExpr>, Box<FlowExpr>),
    Or(Box<FlowExpr>, Box<FlowExpr>),
    /// `A = B` (`equal`) or `A <> B`.
    Cmp {
        equal: bool,
        lhs: Box<FlowExpr>,
        rhs: Box<FlowExpr>,
    },
    /// `A in [B, …]`.
    In(Box<FlowExpr>, Vec<FlowExpr>),
    /// Round-4 literal routes: a `Variant` type test with no argument,
    /// `V.IsRecord` / `V.IsRecord()`, `V` a bare identifier: `V`'s name and
    /// the method, both unquoted and folded.
    TypeTest {
        var: String,
        method: String,
    },
    /// Any other expression: a call, a member, arithmetic, a range.
    Unknown,
}

/// r3 E2: what one statement-tree node says to the within-routine value flow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FlowFact {
    /// On an assignment (an `other` node): the target's bare name, folded
    /// (`None` for a member or an indexed target), the value, and whether
    /// the assignment is compound (`+=`).
    Assign {
        target: Option<String>,
        value: FlowExpr,
        compound: bool,
    },
    /// On an `if`, a `while` and a `case` (its scrutinee): the condition,
    /// and its source text (whitespace folded, at most [`FLOW_TEXT_MAX`]
    /// characters) for a witness to name it.
    Cond { expr: FlowExpr, text: Box<str> },
    /// On a `for`, a `foreach` and a `repeat`: the loop's condition as
    /// written, for a witness to name it as a guard of the sites in its body
    /// (`V := A to B`, `V in X`, the `until` condition; folded as
    /// [`FlowFact::Cond`]'s text). The flow reads no value from it.
    Loop { text: Box<str> },
    /// On a call leaf: its arguments, by position.
    Args(Vec<FlowExpr>),
    /// A `with` statement: inside it a bare name may be a field.
    With,
    /// A statement the lowerer did not recognise: it may write anything.
    Opaque,
}

/// Whether a routine is OPAQUE to every reader that resolves an argument, variable or condition
/// by NAME (the literal pruning's frames and bindings, the RecordRef flow, the Variant routes,
/// E2): the ONE test they all call (sol on ENG-30). Opaque when
/// - its statement tree holds a `with` ([`FlowFact::With`]): inside `with R do` an unqualified
///   name may be one of R's fields;
/// - or it has a body but no tree (an untrusted body, ENG-14): its call sites remain, but no
///   tree says whether a `with` encloses them (sol round 5).
///
/// A routine with no body at all (symbol-only code) has no names to misread and is not opaque.
/// shortcut: opaque as a whole; resolving with-scope names as alc does is ENG-143.
#[must_use]
pub fn names_opaque(tree: Option<&PCFNNode>, body_available: bool) -> bool {
    fn walk(n: &PCFNNode) -> bool {
        matches!(n.flow.as_deref(), Some(FlowFact::With))
            || n.condition_leaves
                .iter()
                .chain(&n.children)
                .chain(&n.else_children)
                .flatten()
                .any(walk)
    }
    match tree {
        Some(t) => walk(t),
        None => body_available,
    }
}

/// The longest condition text a [`FlowFact::Cond`] keeps.
pub const FLOW_TEXT_MAX: usize = 80;

/// r3 E1: a literal value as written: an integer literal or a qualified
/// label `Q::M`. Where a label points (an Option variable's member, an enum
/// value) depends on the routine's scope, so it is resolved by its reader
/// (`literal_reach`), not here.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ValueLabel {
    /// An integer literal, its sign included (`-1`).
    Int(i64),
    /// `Q::M`: the qualifier's segments, each unquoted and folded
    /// (`Enum::"Sales Document Type"` → `["enum", "sales document type"]`,
    /// `Rec."Document Type"` → `["rec", "document type"]`, `V` → `["v"]`),
    /// and the member, unquoted and folded.
    Qualified { path: Vec<String>, member: String },
    /// ENG-30: a text literal's content as written, case and spaces kept,
    /// a doubled `''` read as one quote (`'it''s'` → `it's`).
    Text(String),
}

impl ValueLabel {
    /// The label an expression's [`PExpressionInfo`] states: an integer
    /// literal (signed too), a qualified enum/option value or a text literal.
    /// `None` for any other expression (a variable, a call, a range,
    /// `FromInteger`).
    pub fn of_info(info: &PExpressionInfo) -> Option<ValueLabel> {
        match info.kind.as_str() {
            "integer" | "unary_expression" => {
                let v: String = info.value.as_deref()?.split_whitespace().collect();
                v.parse::<i64>().ok().map(ValueLabel::Int)
            }
            // From the raw text: `value` has every outer quote trimmed.
            "string_literal" => {
                let inner = info.text.trim().strip_prefix('\'')?.strip_suffix('\'')?;
                Some(ValueLabel::Text(inner.replace("''", "'")))
            }
            "qualified_enum_value" => {
                let path = split_label_path(info.qualifier.as_deref()?)?;
                let member = fold_label_segment(info.member.as_deref()?)?;
                Some(ValueLabel::Qualified { path, member })
            }
            _ => None,
        }
    }
}

/// One identifier of a label, unquoted (a doubled `""` inside quotes is one
/// `"`), trimmed and folded; `None` when empty.
fn fold_label_segment(s: &str) -> Option<String> {
    let t = s.trim();
    let inner = t
        .strip_prefix('"')
        .and_then(|x| x.strip_suffix('"'))
        .map(|x| x.replace("\"\"", "\""))
        .unwrap_or_else(|| t.to_string());
    let inner = inner.trim();
    (!inner.is_empty()).then(|| inner.fold_identifier())
}

/// A label qualifier's segments, split at `::` and `.` outside quotes
/// (`Enum::"A.B"` is two segments, `enum` and `a.b`). `None` when a segment
/// is empty.
fn split_label_path(text: &str) -> Option<Vec<String>> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_quotes = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                in_quotes = !in_quotes;
                cur.push(c);
            }
            '.' if !in_quotes => out.push(fold_label_segment(&std::mem::take(&mut cur))?),
            ':' if !in_quotes && chars.peek() == Some(&':') => {
                chars.next();
                out.push(fold_label_segment(&std::mem::take(&mut cur))?);
            }
            _ => cur.push(c),
        }
    }
    out.push(fold_label_segment(&cur)?);
    Some(out)
}

/// r3 E1: what one statement-tree node says about a value (see
/// `literal_reach`'s value frame, which decides whether the name is a usable
/// parameter and what each label means).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValueFact {
    /// On a `case` whose scrutinee is a bare identifier (through
    /// parentheses): its name, unquoted and folded.
    CaseOn(String),
    /// On a non-`else` `case-branch`: one entry per pattern, `None` for a
    /// pattern that is not a literal or a label (a range, an expression).
    Arm(Vec<Option<ValueLabel>>),
    /// On an `if` whose whole condition (through parentheses) is `N = L`,
    /// `N <> L` (either side) or `N in [L, …]`, `N` a bare identifier:
    /// its name, whether the test is `=` / `in` (`true`) or `<>`, and the
    /// labels (`None` for one that is not a literal or a label).
    Test {
        name: String,
        equal: bool,
        labels: Vec<Option<ValueLabel>>,
    },
    /// On a `for` / `foreach` whose loop variable is a bare identifier: its
    /// name, unquoted and folded (the loop writes it).
    LoopVar(String),
}

/// What an `and` / `or` condition says about the simple guards among its
/// operands (each in [`PConditionGuard`]'s form: `P`, `not P`, `P = false`).
/// The other operands are unknown, so they decide nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JunctionGuards {
    /// `A and B and …`: the THEN branch runs only when every listed guard
    /// holds. The ELSE branch is not constrained.
    All(Vec<PConditionGuard>),
    /// `A or B or …`: the ELSE branch (and what follows an `if … then exit;`)
    /// runs only when no listed guard holds. The THEN branch is not
    /// constrained.
    Any(Vec<PConditionGuard>),
}

/// G33: what a `case R.Number of Database::"T": …` says about the tables.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TableCase {
    /// On the `case`: `R`, folded (`R.Number`, `R.Number()`, parenthesized).
    On(String),
    /// On a `case-branch` whose every pattern is `Database::"T"`: the tables'
    /// logical names, folded. A branch with any other pattern has none.
    Tables(Vec<String>),
}

/// In-memory-only facts on a [`PCFNNode`] for path walks (#343,
/// `detectors::walk_paths`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathHint {
    /// On an `if` whose whole condition is a call `C(..)` or `not C(..)`
    /// (through parentheses): whether it is negated. The call is the `if`'s
    /// last top-level condition leaf.
    CondCall { negated: bool },
    /// On an `exit(true)` / `exit(false)`: the literal value.
    Exit(bool),
    /// On the node of an `asserterror` statement: an error inside is caught.
    AssertError,
    /// On a leaf harvested from one alternative of a ternary expression: it
    /// may or may not run.
    MayRun,
}

impl PartialEq for PCFNNode {
    fn eq(&self, other: &Self) -> bool {
        // The in-memory-only markers (`rare`, `source_range`, `flow`) are
        // deliberately omitted.
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
    use super::{
        PRecordVariable, PTempState, RECEIVER_PARAM_INDEX, known_temp_suppresses,
        temp_state_suppresses,
    };

    /// ENG-85: the anchor and vocabulary caches answer the text asked for, and share the
    /// allocation only when the text is the same: alternating files and kinds never hand one
    /// back for another.
    #[test]
    fn shared_text_caches_answer_the_text_asked_for() {
        use super::{anchor_unit, shared_vocab};
        use crate::program::str_pool::SharedStr;
        let a1 = anchor_unit("ws:A.al");
        let a2 = anchor_unit("ws:A.al");
        let b = anchor_unit("ws:B.al");
        let a3 = anchor_unit("ws:A.al");
        assert_eq!(
            (a1.as_str(), b.as_str(), a3.as_str()),
            ("ws:A.al", "ws:B.al", "ws:A.al")
        );
        assert!(
            SharedStr::ptr_eq(&a1, &a2),
            "consecutive equal file ids share"
        );
        let k1 = shared_vocab("identifier");
        let k2 = shared_vocab("call_expression");
        let k3 = shared_vocab("identifier");
        assert_eq!(
            (k1.as_str(), k2.as_str()),
            ("identifier", "call_expression")
        );
        assert!(
            SharedStr::ptr_eq(&k1, &k3),
            "a vocabulary text is one allocation"
        );
    }

    /// ENG-85: the per-thread vocabulary set is bounded (a resident engine's pool threads
    /// live across every refresh): past `SHARED_VOCAB_CAP` texts a new text is still answered
    /// right but no longer kept, while the texts kept before stay shared.
    #[test]
    fn the_shared_vocabulary_stops_growing_at_its_cap() {
        use super::{SHARED_VOCAB_CAP, shared_vocab};
        use crate::program::str_pool::SharedStr;
        std::thread::spawn(|| {
            for i in 0..SHARED_VOCAB_CAP {
                shared_vocab(&format!("k{i}"));
            }
            let first = (shared_vocab("k0"), shared_vocab("k0"));
            assert!(SharedStr::ptr_eq(&first.0, &first.1), "kept below the cap");
            let past = (shared_vocab("past"), shared_vocab("past"));
            assert_eq!(past.0.as_str(), "past");
            assert!(
                !SharedStr::ptr_eq(&past.0, &past.1),
                "not kept past the cap"
            );
        })
        .join()
        .expect("the vocabulary thread");
    }

    /// ENG-108: the coverage-text set is bounded the same way. Its texts are open (an unknown
    /// target is a routine id), so a workspace past `SHARED_COVERAGE_TEXT_CAP` distinct texts on
    /// one thread still gets every text answered right, only no longer shared, while the texts
    /// kept before stay shared. DISCRIMINATION: dropping the cap check keeps "past" shared.
    #[test]
    fn the_shared_coverage_texts_stop_growing_at_their_cap() {
        use super::{SHARED_COVERAGE_TEXT_CAP, shared_coverage_text};
        use crate::program::str_pool::SharedStr;
        std::thread::spawn(|| {
            for i in 0..SHARED_COVERAGE_TEXT_CAP {
                shared_coverage_text(&format!("app/Codeunit/{i}/proc"));
            }
            let first = (
                shared_coverage_text("app/Codeunit/0/proc"),
                shared_coverage_text("app/Codeunit/0/proc"),
            );
            assert!(SharedStr::ptr_eq(&first.0, &first.1), "kept below the cap");
            let past = (shared_coverage_text("past"), shared_coverage_text("past"));
            assert_eq!(past.0.as_str(), "past", "answered right past the cap");
            assert!(
                !SharedStr::ptr_eq(&past.0, &past.1),
                "not kept past the cap"
            );
        })
        .join()
        .expect("the coverage-text thread");
    }

    fn ts(kind: &str, value: Option<bool>, param: Option<u32>) -> PTempState {
        PTempState {
            kind: kind.into(),
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
    /// r3 E1: a label's qualifier splits at `::` and `.` outside quotes only,
    /// each segment unquoted and folded; a signed integer literal reads.
    #[test]
    fn value_label_shapes() {
        use super::{PExpressionInfo, ValueLabel};
        let info =
            |kind: &str, value: Option<&str>, q: Option<&str>, m: Option<&str>| PExpressionInfo {
                kind: kind.into(),
                text: "".into(),
                value: value.map(Into::into),
                qualifier: q.map(Into::into),
                member: m.map(Into::into),
            };
        let q = |path: &[&str], member: &str| ValueLabel::Qualified {
            path: path.iter().map(|s| s.to_string()).collect(),
            member: member.into(),
        };
        assert_eq!(
            ValueLabel::of_info(&info(
                "qualified_enum_value",
                None,
                Some("Enum::\"Sales.Doc Type\""),
                Some("\"Blanket Order\"")
            )),
            Some(q(&["enum", "sales.doc type"], "blanket order"))
        );
        assert_eq!(
            ValueLabel::of_info(&info(
                "qualified_enum_value",
                None,
                Some("SalesHeader.\"Document Type\""),
                Some("Order")
            )),
            Some(q(&["salesheader", "document type"], "order"))
        );
        assert_eq!(
            ValueLabel::of_info(&info("unary_expression", Some("- 1"), None, None)),
            Some(ValueLabel::Int(-1))
        );
        assert_eq!(
            ValueLabel::of_info(&info("identifier", Some("X"), None, None)),
            None
        );
        // ENG-30: a text literal from its raw text, quotes unescaped, case
        // and spaces kept (`value` would have lost the lone quote of `''''`).
        let text = |raw: &str| {
            ValueLabel::of_info(&PExpressionInfo {
                kind: "string_literal".into(),
                text: raw.into(),
                value: Some(raw.trim_matches('\'').into()),
                qualifier: None,
                member: None,
            })
        };
        assert_eq!(text("'it''s'"), Some(ValueLabel::Text("it's".into())));
        assert_eq!(text("''''"), Some(ValueLabel::Text("'".into())));
        assert_eq!(text("' Ab '"), Some(ValueLabel::Text(" Ab ".into())));
        assert_eq!(text("''"), Some(ValueLabel::Text(String::new())));
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
        assert_eq!(actual.len(), 26, "scanner found {actual:?}");
        assert_eq!(documented, actual);
    }

    /// #299: a table receiver row round-trips through JSON without the flag (it is
    /// serde-skipped, so it reads back `false`: fail closed) and still compares
    /// equal, because `PartialEq` covers only the serialized surface.
    #[test]
    fn receiver_flag_is_not_serialized_and_not_compared() {
        let row = PRecordVariable {
            id: "r/rv/rec".into(),
            name: "Rec".to_string(),
            table_name: None,
            temp_state: PTempState {
                kind: "parameter-dependent".into(),
                value: None,
                parameter_index: Some(RECEIVER_PARAM_INDEX),
            },
            is_parameter: false,
            parameter_index: None,
            scope: None,
            is_receiver: true,
        };
        let json = serde_json::to_string(&row).unwrap();
        assert!(!json.contains("eceiver"), "flag leaked into {json}");
        let back: PRecordVariable = serde_json::from_str(&json).unwrap();
        assert!(!back.is_receiver);
        assert_eq!(back, row);
    }
}

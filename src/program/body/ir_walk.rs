//! Owned-IR L2 walk — the Phase-2 cut foundation.
//!
//! Re-expresses the SPINE of `body_walk` over the owned al-syntax IR rather than
//! the tree-sitter CST: the op/cs DFS numbering (the parity-critical heart), loop
//! tracking, `has_branching`, and the normalized CFN (`statement_tree`) as a REAL
//! `PCFNNode`. The op/cs visit order and the CFN shape were proven at 100% vs the
//! real engine in `tests/ir_dual_run.rs`; this module promotes that proven logic
//! from trace-comparisons to real engine-type production, the first piece of
//! `project_routine_features_ir`. Rich fields (callee/bindings/temp_state) land in
//! subsequent increments, which thread the full `Ctx` scope inputs.

use super::features::{
    PAnchor, PCFNNode, PCallArgumentBinding, PCallSite, PCallee, PConditionGuard,
    PConditionReference, PExpressionInfo, PFieldAccess, PLoop, POperationSite, PRecordOperation,
    PTempState, PUnreachableStatement, PVarAssignment,
};
use super::node_util::Utf16Cols;
use super::record_op::{FIELD_ARGS_OPS, record_op_type};
use al_syntax::ir::{
    AlFile, BinaryOp, BlockId, BlockItem, ExprId, ExprKind, ObjectKind, Origin, RoutineDecl,
    StmtKind, UnaryOp, VarDecl,
};
use std::collections::{HashMap, HashSet};

/// Strip surrounding double-quotes from a quoted identifier's raw text.
fn strip_quotes(s: &str) -> String {
    if s.len() >= 2 && s.starts_with('"') && s.ends_with('"') {
        s[1..s.len() - 1].to_string()
    } else {
        s.to_string()
    }
}

/// Pre-backfill record-op temp state (`{ kind: "unknown" }`).
fn ts_unknown() -> PTempState {
    PTempState {
        kind: "unknown".to_string(),
        value: None,
        parameter_index: None,
    }
}

/// Strip ONE surrounding quote char (`"` or `'`), as legacy `strip_quote_chars`.
fn strip_quote_chars(s: &str) -> String {
    let s = s.trim();
    if s.len() >= 2
        && (s.starts_with('"') && s.ends_with('"') || s.starts_with('\'') && s.ends_with('\''))
    {
        s[1..s.len() - 1].to_string()
    } else {
        s.to_string()
    }
}

/// Object-run object kind for a `keyword_identifier` receiver (`Codeunit`/`Page`/`Report`).
fn object_run_kind(text: &str) -> Option<&'static str> {
    match text.trim().to_ascii_lowercase().as_str() {
        "codeunit" => Some("Codeunit"),
        "page" => Some("Page"),
        "report" => Some("Report"),
        _ => None,
    }
}

/// LHS base name of an assignment target (legacy `lhs_identifier_of`, lowercased):
/// a bare identifier's name, or a member_expression's member name; else None.
fn lhs_base_name(file: &AlFile, target: ExprId) -> Option<String> {
    match &file.ir.expr(target).kind {
        ExprKind::Identifier(x) | ExprKind::QuotedIdentifier(x) => Some(x.to_ascii_lowercase()),
        ExprKind::Member { member, .. } => Some(member.to_ascii_lowercase()),
        _ => None,
    }
}

/// The spine result of the IR walk: the op/cs id maps (keyed by the call ExprId,
/// formatted `{routine_id}/op{N}` / `/cs{N}` in DFS order — exactly legacy's
/// numbering), plus `has_branching`. Consumed by the CFN builder.
pub struct IrSpine {
    pub op_id_by_expr: HashMap<ExprId, String>,
    pub cs_id_by_expr: HashMap<ExprId, String>,
    pub has_branching: bool,
    pub loop_count: u32,
    pub loops: Vec<PLoop>,
    pub field_accesses: Vec<PFieldAccess>,
    pub var_assignments: Vec<PVarAssignment>,
    pub condition_references: Vec<PConditionReference>,
    pub identifier_references: Vec<String>,
    pub unreachable_statements: Vec<PUnreachableStatement>,
    /// record_operations BEFORE the temp_state/record_variable_id backfill (which
    /// `routine_features_partial` applies from `ir_record_variables`).
    pub record_operations: Vec<PRecordOperation>,
    /// The unified op0..opN list (record-op / lock / commit / error-call), in
    /// op-index order. `control_context`/`order` are None (post-pass fields).
    pub operation_sites: Vec<POperationSite>,
    /// call_sites with `argument_bindings` EMPTY (filled by the post-pass in
    /// `routine_features_partial` from `ir_record_variables` + the params).
    pub call_sites: Vec<PCallSite>,
    /// Per call-site, the argument ExprIds (parallel to `call_sites`) — the binding
    /// post-pass needs them to anchor + classify each argument.
    pub cs_arg_exprs: Vec<Vec<ExprId>>,
}

/// An implicit-receiver frame (table-method base `Rec` seed / `with` body). Carries
/// the receiver TEXT so an implicit bare record-op (`Modify()` inside `with Cust do`)
/// records the right `record_variable_name`. Mirrors `body_walk::ImplicitReceiverFrame`.
struct ImplicitFrame {
    is_record: bool,
    text: String,
}

/// Per-routine record-receiver scope, mirroring the harness `ir_op_trace` setup
/// (validated at 100%): `rvars` = record-OP receiver set (Rec/xRec convention +
/// record globals/params/locals); `frvars` = field-access record set (record_var_names
/// semantics — Rec only for table methods).
struct Scope {
    rvars: HashSet<String>,
    frvars: HashSet<String>,
    /// Lowercased names of the object's procedures/triggers. A bare implicit-receiver
    /// call whose method name collides with one of these is a CALL to that procedure,
    /// not a record op (legacy `object_procedure_names` collision check).
    object_procedure_names: HashSet<String>,
}

/// Record-OP / field-access RECEIVER eligibility (the `rvars`/`frvars` sets): a `X.`
/// receiver whose ops are captured as `PRecordOperation` / field accesses. This is
/// INCLUSIVE of `RecordRef` (legacy captures `RecRef.DeleteAll` as a record op) —
/// unlike [`is_record_type_str`], which is the stricter record-VARIABLE test (temp
/// state / table resolution) that excludes `RecordRef`.
fn is_record_receiver_ty(ty: &str) -> bool {
    ty.trim().to_ascii_lowercase().starts_with("record")
}

fn is_record_ty(v: &VarDecl) -> bool {
    v.ty.as_deref().map(is_record_receiver_ty).unwrap_or(false)
}

/// Does the object expose an implicit `Rec` (legacy `implicit_base_receiver` +
/// codeunit `TableNo`)? table/tableext methods + pageext always; a page only with a
/// resolved SourceTable; a codeunit only with a `TableNo` property.
fn has_implicit_rec(o: &al_syntax::ir::ObjectDecl, source_table_name: Option<&str>) -> bool {
    let codeunit_tableno = o.kind == ObjectKind::Codeunit
        && o.properties
            .iter()
            .any(|p| p.name == "tableno" && !p.value.trim().is_empty());
    matches!(
        o.kind,
        ObjectKind::Table | ObjectKind::TableExtension | ObjectKind::PageExtension
    ) || (o.kind == ObjectKind::Page && source_table_name.is_some())
        || codeunit_tableno
}

/// [`has_implicit_rec`] with the page's `SourceTable` read from `o` itself, the
/// way [`ir_object_metadata`] reads it. The program extractor's base implicit
/// frame (`program::resolve::extract`), so the two engines agree on which bare
/// calls are record operations.
pub(crate) fn object_has_implicit_rec(o: &al_syntax::ir::ObjectDecl) -> bool {
    let source_table = o
        .properties
        .iter()
        .find(|p| p.name == "sourcetable")
        .map(|p| p.value.as_str());
    has_implicit_rec(o, source_table)
}

/// Build the per-routine scope (record receiver sets) for `object`/`routine`. The
/// bool result is whether an implicit `Rec` is in scope (drives the base
/// implicit-receiver frame's is-record).
fn build_scope(
    file: &AlFile,
    object_idx: usize,
    routine: &RoutineDecl,
    source_table_name: Option<&str>,
) -> (Scope, bool) {
    let o = &file.objects[object_idx];
    let mut globals: HashSet<String> = o
        .globals
        .iter()
        .filter(|v| is_record_ty(v))
        .map(|v| v.name.to_ascii_lowercase())
        .collect();
    // Rec/xRec are record receivers by name convention regardless of object type.
    globals.insert("rec".to_string());
    globals.insert("xrec".to_string());
    let explicit_globals: HashSet<String> = o
        .globals
        .iter()
        .filter(|v| is_record_ty(v))
        .map(|v| v.name.to_ascii_lowercase())
        .collect();
    let implicit_rec = has_implicit_rec(o, source_table_name);

    let params_locals: Vec<String> = routine
        .params
        .iter()
        .filter(|p| p.ty.as_deref().map(is_record_receiver_ty).unwrap_or(false))
        .map(|p| p.name.to_ascii_lowercase())
        .chain(
            routine
                .locals
                .iter()
                .filter(|v| is_record_ty(v))
                .map(|v| v.name.to_ascii_lowercase()),
        )
        .collect();

    let mut rvars = globals;
    rvars.extend(params_locals.iter().cloned());
    let mut frvars = explicit_globals;
    if implicit_rec {
        frvars.insert("rec".to_string());
    }
    frvars.extend(params_locals);
    let object_procedure_names: HashSet<String> = o
        .routines
        .iter()
        .map(|r| r.name.to_ascii_lowercase())
        .collect();
    (
        Scope {
            rvars,
            frvars,
            object_procedure_names,
        },
        implicit_rec,
    )
}

/// Working state for the spine walk.
struct SpineCtx<'a> {
    file: &'a AlFile,
    /// Parens-less value reads the program resolver kept as calls
    /// (`program::resolve::full::ParenlessCalls`): walked as zero-argument calls.
    parenless_calls: &'a HashSet<ExprId>,
    scope: &'a Scope,
    routine_id: &'a str,
    source: &'a str,
    cols: &'a Utf16Cols<'a>,
    source_unit_id: &'a str,
    op_index: u32,
    cs_index: u32,
    loop_count: u32,
    has_branching: bool,
    implicit: Vec<ImplicitFrame>,
    /// Enclosing-loop sequence numbers (for record-op loop_stack snapshots).
    cur_loops: Vec<u32>,
    op_id_by_expr: HashMap<ExprId, String>,
    cs_id_by_expr: HashMap<ExprId, String>,
    loops: Vec<PLoop>,
    field_accesses: Vec<PFieldAccess>,
    var_assignments: Vec<PVarAssignment>,
    condition_references: Vec<PConditionReference>,
    identifier_ref_set: HashSet<String>,
    unreachable_statements: Vec<PUnreachableStatement>,
    unreachable_index: u32,
    record_operations: Vec<PRecordOperation>,
    operation_sites: Vec<POperationSite>,
    /// asserterror nesting depth (only error-call operation_sites carry under_asserterror).
    assert_depth: u32,
    /// True while walking a bare statement-position call (`StmtKind::Call`'s expr) —
    /// drives object-run result_consumed/return_used (a bare statement does not
    /// consume the result; an expression-position call does).
    in_stmt_position: bool,
    /// One-shot: the NEXT call walked is a chained-call RECEIVER (the object of a
    /// member call), so its bare function name is harvested as an identifier-ref
    /// (legacy `collect_identifiers_from` counts it — a non-root identifier in the
    /// receiver subtree). Top-level calls do not count their function.
    count_next_call_fn: bool,
    /// True while walking a `repeat`'s `until` expression — a record op here is the
    /// loop terminator (`PRecordOperation.in_until_condition`, an L5/d1 input that is
    /// serde-skipped so the L2 byte gate cannot see it). `until` is an expression, so
    /// no nested loop resets the nearest-enclosing-loop context.
    in_until: bool,
    call_sites: Vec<PCallSite>,
    cs_arg_exprs: Vec<Vec<ExprId>>,
    /// Every assignment whose target is a bare variable, `(name lowercased,
    /// value expr)`, in walk order. Read by `exhausting_advance` (#50) to see
    /// how often a `repeat` body assigns its `until` flag.
    bare_assigns: Vec<(String, ExprId)>,
    /// While walking an assignment's target: that target expression and how it
    /// is used (#14). Only a field access ON that exact expression is a write;
    /// one nested inside it (an index) is still a read.
    assign_target: Option<(ExprId, crate::program::body::features::FieldAccessKind)>,
}

impl<'a> SpineCtx<'a> {
    /// PAnchor for an IR node origin (utf16 columns via `Utf16Cols`, exactly as the
    /// legacy `Ctx::anchor`; `syntax_kind` = the raw grammar kind).
    fn anchor(&self, origin: &Origin) -> PAnchor {
        PAnchor {
            source_unit_id: self.source_unit_id.to_string(),
            start_line: origin.start.row,
            start_column: self
                .cols
                .col(origin.start.row as usize, origin.start.column as usize),
            end_line: origin.end.row,
            end_column: self
                .cols
                .col(origin.end.row as usize, origin.end.column as usize),
            syntax_kind: origin.kind_text.to_string(),
        }
    }

    /// Exact raw source text of an IR node (via its byte span).
    fn raw(&self, origin: &Origin) -> &'a str {
        &self.source[origin.byte.clone()]
    }

    /// Literal text of an rhs value (legacy `literal_text_of`): boolean → lc,
    /// integer → raw, string → quote-stripped lc; otherwise None.
    fn lit_text(&self, eid: ExprId) -> Option<String> {
        use al_syntax::ir::Literal::*;
        match &self.file.ir.expr(eid).kind {
            ExprKind::Literal(Bool(b)) => Some(b.to_string()),
            ExprKind::Literal(Int(s)) => Some(s.clone()),
            ExprKind::Literal(Text(s)) => {
                let t = s
                    .strip_prefix('\'')
                    .and_then(|x| x.strip_suffix('\''))
                    .unwrap_or(s);
                Some(t.to_ascii_lowercase())
            }
            _ => None,
        }
    }

    /// Classify a call's callee (legacy `callee_from_node`) + its `callee_text`
    /// (the raw function-expr source). Handles bare, member, and the object-run
    /// upgrade (`Codeunit.Run(Database::"X")`). The implicit-frame member upgrade is
    /// not modelled here (rare; bare calls inside a non-record `with`).
    fn classify_callee(&self, function: ExprId, args: &[ExprId]) -> (PCallee, String) {
        use ExprKind::*;
        let fe = self.file.ir.expr(function);
        let callee_text = self.raw(&fe.origin).to_string();
        let callee = match &fe.kind {
            Identifier(_) | QuotedIdentifier(_) => PCallee::Bare {
                name: strip_quote_chars(self.raw(&fe.origin)),
            },
            Member { object, member, .. } => {
                let obj = self.file.ir.expr(*object);
                if obj.origin.kind_text == "keyword_identifier"
                    && let Some(okind) = object_run_kind(self.raw(&obj.origin))
                    && member.eq_ignore_ascii_case("run")
                {
                    return (self.object_run_callee(okind, args), callee_text);
                }
                PCallee::Member {
                    receiver: self.raw(&obj.origin).to_string(),
                    method: strip_quote_chars(member),
                }
            }
            _ => PCallee::Unknown,
        };
        (callee, callee_text)
    }

    /// ObjectRun callee from the first argument (a `database_reference`), legacy
    /// `classify_object_run_first_arg`.
    fn object_run_callee(&self, object_kind: &str, args: &[ExprId]) -> PCallee {
        let mut target_ref = None;
        let mut target_is_name = false;
        if let Some(&first) = args.first()
            && let ExprKind::DatabaseReference(text) = &self.file.ir.expr(first).kind
            && let Some((_, tn)) = text.split_once("::")
        {
            let tn = tn.trim();
            if tn.starts_with('"') {
                target_ref = Some(strip_quote_chars(tn));
                target_is_name = true;
            } else if tn.parse::<i64>().is_ok() {
                target_ref = Some(tn.to_string());
                target_is_name = false;
            } else {
                target_ref = Some(tn.to_string());
                target_is_name = true;
            }
        }
        PCallee::ObjectRun {
            object_kind: object_kind.to_string(),
            target_type: object_kind.to_string(),
            target_ref,
            target_is_name,
        }
    }

    /// Field-argument capture for a record op (legacy: only for `FIELD_ARGS_OPS`,
    /// every arg's raw text + structured `PExpressionInfo`). `Some(vec![])` when the
    /// op is a FIELD_ARGS op with an (possibly empty) argument list.
    fn record_op_field_args(
        &self,
        op_type: &str,
        args: &[ExprId],
    ) -> (Option<Vec<String>>, Option<Vec<PExpressionInfo>>) {
        if !FIELD_ARGS_OPS.contains(&op_type) {
            return (None, None);
        }
        let mut texts = Vec::with_capacity(args.len());
        let mut infos = Vec::with_capacity(args.len());
        for &a in args {
            texts.push(self.raw(&self.file.ir.expr(a).origin).to_string());
            infos.push(self.ir_expression_info(a));
        }
        (Some(texts), Some(infos))
    }

    /// Structured expression classification (legacy `expression_info_from_node`).
    fn ir_expression_info(&self, eid: ExprId) -> PExpressionInfo {
        use ExprKind::*;
        let e = self.file.ir.expr(eid);
        let text = self.raw(&e.origin).to_string();
        let strip_q = |s: &str| s.trim_matches(|c| c == '"' || c == '\'').to_string();
        let mut value = None;
        let mut qualifier = None;
        let mut member = None;
        let kind = match &e.kind {
            Literal(al_syntax::ir::Literal::Text(_)) => {
                value = Some(strip_q(&text));
                "string_literal"
            }
            QuotedIdentifier(_) => {
                value = Some(strip_q(&text));
                "quoted_identifier"
            }
            Literal(al_syntax::ir::Literal::Int(_)) => {
                value = Some(text.clone());
                "integer"
            }
            Literal(al_syntax::ir::Literal::Decimal(_)) => {
                value = Some(text.clone());
                "decimal"
            }
            Literal(al_syntax::ir::Literal::Bool(_)) => {
                value = Some(text.clone());
                "boolean"
            }
            Identifier(_) if e.origin.kind_text == "identifier" => {
                value = Some(text.clone());
                "identifier"
            }
            QualifiedEnum {
                enum_type,
                value: v,
            } => {
                qualifier = Some(self.raw(&self.file.ir.expr(*enum_type).origin).to_string());
                let m = strip_q(v);
                member = Some(m.clone());
                value = Some(m);
                "qualified_enum_value"
            }
            DatabaseReference(t) => {
                if let Some((kw, tn)) = t.split_once("::") {
                    qualifier = Some(kw.trim().to_string());
                    let m = strip_q(tn.trim());
                    member = Some(m.clone());
                    value = Some(m);
                }
                "database_reference"
            }
            Unary { operand, .. } => {
                // signed numeric literal → the signed text, else None.
                if matches!(
                    &self.file.ir.expr(*operand).kind,
                    Literal(al_syntax::ir::Literal::Int(_))
                        | Literal(al_syntax::ir::Literal::Decimal(_))
                ) {
                    value = Some(text.clone());
                }
                "unary_expression"
            }
            Member { .. } => "member_expression",
            Call { .. } => "call_expression",
            Parenthesized(_) => "parenthesized_expression",
            _ => "other",
        };
        PExpressionInfo {
            kind: kind.to_string(),
            text,
            value,
            qualifier,
            member,
        }
    }

    /// Collect condition references (the `collect_idents` closure in legacy): a bare
    /// `identifier`, or a `member_expression`'s member identifier, anchored at the
    /// reference; recursing other expression shapes but NOT a member's object.
    fn collect_cond_idents(&mut self, eid: ExprId, kind: &str, stmt: &PAnchor) {
        use ExprKind::*;
        let e = self.file.ir.expr(eid);
        match &e.kind {
            Identifier(name) if e.origin.kind_text == "identifier" => {
                self.condition_references.push(PConditionReference {
                    identifier: name.to_ascii_lowercase(),
                    condition_kind: kind.to_string(),
                    statement_anchor: stmt.clone(),
                    reference_anchor: self.anchor(&e.origin),
                });
            }
            Member {
                member,
                member_origin,
                ..
            } => {
                if member_origin.kind_text == "identifier" {
                    self.condition_references.push(PConditionReference {
                        identifier: member.to_ascii_lowercase(),
                        condition_kind: kind.to_string(),
                        statement_anchor: stmt.clone(),
                        reference_anchor: self.anchor(member_origin),
                    });
                }
                // does NOT recurse into the object.
            }
            Call { function, args } => {
                let (function, args) = (*function, args.clone());
                self.collect_cond_idents(function, kind, stmt);
                for a in args {
                    self.collect_cond_idents(a, kind, stmt);
                }
            }
            Binary { lhs, rhs, .. } => {
                let (lhs, rhs) = (*lhs, *rhs);
                self.collect_cond_idents(lhs, kind, stmt);
                self.collect_cond_idents(rhs, kind, stmt);
            }
            Unary { operand, .. } => {
                let operand = *operand;
                self.collect_cond_idents(operand, kind, stmt);
            }
            Parenthesized(x) => {
                let x = *x;
                self.collect_cond_idents(x, kind, stmt);
            }
            Index { base, index } => {
                let (base, index) = (*base, *index);
                self.collect_cond_idents(base, kind, stmt);
                self.collect_cond_idents(index, kind, stmt);
            }
            QualifiedEnum { enum_type, .. } => {
                let enum_type = *enum_type;
                self.collect_cond_idents(enum_type, kind, stmt);
            }
            RangeExpr { start, end } => {
                let (start, end) = (*start, *end);
                self.collect_cond_idents(start, kind, stmt);
                self.collect_cond_idents(end, kind, stmt);
            }
            Ternary { .. } | TypeOp { .. } | List(_) => {
                for c in e.kind.children() {
                    self.collect_cond_idents(c, kind, stmt);
                }
            }
            _ => {}
        }
    }

    /// Record a loop at its node (legacy creates the PLoop at the loop node, id
    /// `{routine}/loop{N}` with N = discovery order = `loops.len()`).
    fn enter_loop(&mut self, loop_type: &str, origin: &Origin) {
        let n = self.loops.len() as u32;
        let id = format!("{}/loop{}", self.routine_id, n);
        let source_anchor = self.anchor(origin);
        self.loops.push(PLoop {
            id,
            loop_type: loop_type.to_string(),
            source_anchor,
            exhausting_advance: None,
        });
        self.loop_count += 1;
        self.cur_loops.push(n);
    }

    /// `e` with any parentheses removed.
    fn unparen(&self, mut e: ExprId) -> ExprId {
        while let ExprKind::Parenthesized(x) = &self.file.ir.expr(e).kind {
            e = *x;
        }
        e
    }

    /// `R.Next(args) = 0` or `0 = R.Next(args)` (parentheses anywhere) ->
    /// `(R lowercased, the Next call's expr)`. Any other shape -> `None`.
    fn next_eq_zero(&self, e: ExprId) -> Option<(String, ExprId)> {
        let ExprKind::Binary {
            op: al_syntax::ir::BinaryOp::Eq,
            lhs,
            rhs,
        } = &self.file.ir.expr(self.unparen(e)).kind
        else {
            return None;
        };
        let is_zero = |x: ExprId| {
            matches!(&self.file.ir.expr(self.unparen(x)).kind,
                ExprKind::Literal(al_syntax::ir::Literal::Int(v)) if v == "0")
        };
        let call = if is_zero(*rhs) {
            self.unparen(*lhs)
        } else if is_zero(*lhs) {
            self.unparen(*rhs)
        } else {
            return None;
        };
        let ExprKind::Call { function, .. } = &self.file.ir.expr(call).kind else {
            return None;
        };
        let ExprKind::Member { object, member, .. } = &self.file.ir.expr(*function).kind else {
            return None;
        };
        if !member.eq_ignore_ascii_case("next") {
            return None;
        }
        match &self.file.ir.expr(*object).kind {
            ExprKind::Identifier(v) | ExprKind::QuotedIdentifier(v) => {
                Some((v.to_ascii_lowercase(), call))
            }
            _ => None,
        }
    }

    /// The `PLoop::exhausting_advance` of a `repeat` loop (#49 / #50), computed
    /// after its body and `until` are walked (so the `Next` already has an op id).
    /// `assigns_before` is `bare_assigns.len()` when the body began.
    fn exhausting_advance(
        &self,
        body: BlockId,
        until: ExprId,
        assigns_before: usize,
    ) -> Option<(String, String)> {
        let (var, call) = match self.next_eq_zero(until) {
            Some(found) => found,
            None => {
                // #50: `until Done`, with `Done := R.Next(args) = 0` the ONLY
                // assignment to `Done` anywhere in the loop, and a top-level body
                // statement (not under an `if`, which would make it conditional).
                let ExprKind::Identifier(flag) = &self.file.ir.expr(self.unparen(until)).kind
                else {
                    return None;
                };
                let flag = flag.to_ascii_lowercase();
                let mut assigns = self.bare_assigns[assigns_before..]
                    .iter()
                    .filter(|(n, _)| *n == flag);
                let (Some((_, value)), None) = (assigns.next(), assigns.next()) else {
                    return None;
                };
                let top_level = self.file.ir.block(body).items.iter().any(|item| {
                    matches!(item, al_syntax::ir::BlockItem::Stmt(s)
                        if matches!(&self.file.ir.stmt(*s).kind,
                            StmtKind::Assignment { value: v, .. } if v == value))
                });
                if !top_level {
                    return None;
                }
                self.next_eq_zero(*value)?
            }
        };
        let op_id = self.op_id_by_expr.get(&call)?.clone();
        Some((var, op_id))
    }

    /// The enclosing-loop id stack (`{routine}/loop{N}`) for a record-op snapshot.
    fn loop_stack_ids(&self) -> Vec<String> {
        self.cur_loops
            .iter()
            .map(|n| format!("{}/loop{}", self.routine_id, n))
            .collect()
    }

    fn walk_block(&mut self, bid: BlockId) {
        // Block-scoped unreachable scan (legacy runs per code_block, pre-order):
        // the FIRST statement that is an unconditional exit makes its immediate
        // next sibling unreachable. Comments/keywords are not IR statements, so the
        // block's Stmt items already match legacy's filtered `block_statements`.
        let stmts: Vec<al_syntax::ir::StmtId> = self
            .file
            .ir
            .block(bid)
            .items
            .iter()
            .filter_map(|it| match it {
                BlockItem::Stmt(s) => Some(*s),
                BlockItem::Preproc(_) => None,
            })
            .collect();
        for i in 0..stmts.len().saturating_sub(1) {
            if let Some(exit_kind) = self.unconditional_exit_kind(stmts[i]) {
                let exit_anchor = self.anchor(&self.file.ir.stmt(stmts[i]).origin);
                let unreachable_anchor = self.anchor(&self.file.ir.stmt(stmts[i + 1]).origin);
                self.unreachable_statements.push(PUnreachableStatement {
                    id: format!("{}/u{}", self.routine_id, self.unreachable_index),
                    exit_kind: exit_kind.to_string(),
                    exit_anchor,
                    unreachable_anchor,
                });
                self.unreachable_index += 1;
                break;
            }
        }

        for item in &self.file.ir.block(bid).items {
            match item {
                BlockItem::Stmt(s) => self.walk_stmt(*s),
                BlockItem::Preproc(g) => {
                    for b in &g.branches {
                        self.walk_block(*b);
                    }
                }
            }
        }
    }

    /// Legacy `unconditional_exit_kind` over an IR statement: an `exit`/`break`, an
    /// `Error(...)` call, or `CurrReport.Quit(...)`. Conditional exits are
    /// `if`-statements, never classified here (structural, as in legacy).
    fn unconditional_exit_kind(&self, sid: al_syntax::ir::StmtId) -> Option<&'static str> {
        use ExprKind::*;
        match &self.file.ir.stmt(sid).kind {
            StmtKind::Exit(_) => Some("exit"),
            StmtKind::Break => Some("break"),
            StmtKind::Call(e) => {
                let Call { function, .. } = &self.file.ir.expr(*e).kind else {
                    return None;
                };
                match &self.file.ir.expr(*function).kind {
                    Identifier(m) if m.eq_ignore_ascii_case("error") => Some("error"),
                    Member { object, member, .. } => {
                        let obj_is = matches!(&self.file.ir.expr(*object).kind,
                            Identifier(o) | QuotedIdentifier(o) if o.eq_ignore_ascii_case("currreport"));
                        if obj_is && member.eq_ignore_ascii_case("quit") {
                            Some("currreport-quit")
                        } else {
                            None
                        }
                    }
                    _ => None,
                }
            }
            _ => None,
        }
    }

    fn walk_stmt(&mut self, sid: al_syntax::ir::StmtId) {
        use StmtKind::*;
        let st = self.file.ir.stmt(sid);
        match &st.kind {
            Assignment {
                target,
                value,
                compound,
            } => {
                if let ExprKind::Identifier(name) = &self.file.ir.expr(*target).kind {
                    self.bare_assigns.push((name.to_ascii_lowercase(), *value));
                }
                // PVarAssignment: lhs base name (identifier or member name), optional
                // literal rhs, anchored on the assignment statement.
                if let Some(lhs_name) = lhs_base_name(self.file, *target) {
                    let rhs_identifier = match (
                        &self.file.ir.expr(*target).kind,
                        &self.file.ir.expr(*value).kind,
                    ) {
                        (ExprKind::Identifier(_), ExprKind::Identifier(v)) => {
                            Some(v.to_ascii_lowercase())
                        }
                        _ => None,
                    };
                    self.var_assignments.push(PVarAssignment {
                        lhs_name,
                        rhs_literal_value: self.lit_text(*value),
                        source_anchor: self.anchor(&st.origin),
                        rhs_identifier,
                    });
                }
                use crate::program::body::features::FieldAccessKind;
                let kind = if *compound {
                    FieldAccessKind::ReadWrite
                } else {
                    FieldAccessKind::Write
                };
                self.assign_target = Some((*target, kind));
                self.walk_expr(*target);
                self.assign_target = None;
                self.walk_expr(*value);
            }
            Call(x) => {
                self.in_stmt_position = true;
                self.walk_expr(*x);
                self.in_stmt_position = false;
            }
            If {
                cond,
                then_block,
                else_block,
            } => {
                self.has_branching = true;
                let sa = self.anchor(&st.origin);
                self.collect_cond_idents(*cond, "if", &sa);
                self.walk_expr(*cond);
                self.walk_block(*then_block);
                if let Some(b) = else_block {
                    self.walk_block(*b);
                }
            }
            Case {
                scrutinee,
                branches,
                else_block,
            } => {
                self.has_branching = true;
                let sa = self.anchor(&st.origin);
                self.collect_cond_idents(*scrutinee, "case", &sa);
                self.walk_expr(*scrutinee);
                for br in branches {
                    for p in &br.patterns {
                        self.walk_expr(*p);
                    }
                    self.walk_block(br.body);
                }
                if let Some(b) = else_block {
                    self.walk_block(*b);
                }
            }
            While { cond, body } => {
                let sa = self.anchor(&st.origin);
                self.collect_cond_idents(*cond, "while", &sa);
                self.enter_loop("while", &st.origin);
                self.walk_expr(*cond);
                self.walk_block(*body);
                self.cur_loops.pop();
            }
            Repeat { body, until } => {
                let sa = self.anchor(&st.origin);
                self.collect_cond_idents(*until, "repeat-until", &sa);
                let loop_ix = self.loops.len();
                let assigns_before = self.bare_assigns.len();
                self.enter_loop("repeat", &st.origin);
                self.walk_block(*body);
                self.in_until = true;
                self.walk_expr(*until);
                self.in_until = false;
                self.loops[loop_ix].exhausting_advance =
                    self.exhausting_advance(*body, *until, assigns_before);
                self.cur_loops.pop();
            }
            For {
                var,
                from,
                to,
                body,
                ..
            } => {
                self.enter_loop("for", &st.origin);
                self.walk_expr(*var);
                self.walk_expr(*from);
                self.walk_expr(*to);
                self.walk_block(*body);
                self.cur_loops.pop();
            }
            Foreach {
                var,
                iterable,
                body,
            } => {
                self.enter_loop("foreach", &st.origin);
                self.walk_expr(*var);
                self.walk_expr(*iterable);
                self.walk_block(*body);
                self.cur_loops.pop();
            }
            With { receiver, body } => {
                self.walk_expr(*receiver);
                let is_record = match &self.file.ir.expr(*receiver).kind {
                    ExprKind::Identifier(x) | ExprKind::QuotedIdentifier(x) => {
                        self.scope.rvars.contains(&x.to_ascii_lowercase())
                    }
                    _ => false,
                };
                let text = self
                    .raw(&self.file.ir.expr(*receiver).origin)
                    .trim()
                    .to_string();
                self.implicit.push(ImplicitFrame { is_record, text });
                self.walk_block(*body);
                self.implicit.pop();
            }
            Try { body, catch_block } => {
                self.has_branching = true;
                self.walk_block(*body);
                if let Some(c) = catch_block {
                    self.walk_block(*c);
                }
            }
            AssertError(body) => {
                self.assert_depth += 1;
                self.walk_block(*body);
                self.assert_depth -= 1;
            }
            Exit(x) => {
                if let Some(x) = x {
                    self.walk_expr(*x);
                }
            }
            Block(b) => self.walk_block(*b),
            Break | Continue | Unknown => {}
        }
    }

    fn walk_expr(&mut self, eid: ExprId) {
        use ExprKind::*;
        let e = self.file.ir.expr(eid);
        // A parens-less value read the resolver proved a call is walked exactly as
        // the lowered statement form `X.M;`: a call whose function is the read.
        let call = match &e.kind {
            Call { function, args } => Some((*function, args.clone())),
            Identifier(_) | QuotedIdentifier(_) | Member { .. }
                if self.parenless_calls.contains(&eid) =>
            {
                Some((eid, Vec::new()))
            }
            _ => None,
        };
        if let Some((function, args)) = call {
            // A parenless call (`Rec.Find;`) was normalized from a member/identifier/
            // subscript, so its origin kind is NOT `call_expression`. Legacy harvests
            // identifier-refs from a call's function subtree ONLY for real (parens)
            // call_expressions; a parenless call's receiver is not counted.
            let is_parens_call = e.origin.kind_text == "call_expression";
            // Consume the one-shot chained-receiver flag: if set, this call is a
            // receiver and its bare function name is counted as an identifier-ref.
            let count_this_fn = self.count_next_call_fn;
            self.count_next_call_fn = false;
            let fe = self.file.ir.expr(function);
            if count_this_fn
                && let Identifier(n) = &fe.kind
                && fe.origin.kind_text == "identifier"
            {
                self.identifier_ref_set.insert(n.to_ascii_lowercase());
            }
            // Record-op classification + (op_type, receiver text) for the emit.
            let record_op: Option<(&'static str, String)> = match &fe.kind {
                Member { object, member, .. } => {
                    let recv_name = match &self.file.ir.expr(*object).kind {
                        Identifier(x) | QuotedIdentifier(x) => Some(x.to_ascii_lowercase()),
                        _ => None,
                    };
                    let is_rec = recv_name
                        .map(|r| self.scope.rvars.contains(&r))
                        .unwrap_or(false);
                    match (is_rec, record_op_type(&member.to_ascii_lowercase())) {
                        (true, Some(op)) => {
                            Some((op, self.raw(&self.file.ir.expr(*object).origin).to_string()))
                        }
                        _ => None,
                    }
                }
                Identifier(m) | QuotedIdentifier(m) => {
                    let m_lc = m.to_ascii_lowercase();
                    // A bare implicit-receiver call is a record op only if the method
                    // name does NOT collide with an object procedure (else it is a
                    // call to that procedure) — legacy object_procedure_names check.
                    let frame_is_rec = self.implicit.last().map(|f| f.is_record).unwrap_or(false)
                        && !self.scope.object_procedure_names.contains(&m_lc);
                    match (frame_is_rec, record_op_type(&m_lc)) {
                        (true, Some(op)) => {
                            let recv = self
                                .implicit
                                .last()
                                .map(|f| f.text.clone())
                                .unwrap_or_default();
                            Some((op, recv))
                        }
                        _ => None,
                    }
                }
                _ => None,
            };
            let is_record_op = record_op.is_some();
            let fname = match &fe.kind {
                Identifier(m) | QuotedIdentifier(m) => Some(m.to_ascii_lowercase()),
                _ => None,
            };
            let is_commit = fname.as_deref() == Some("commit");
            let is_error = fname.as_deref() == Some("error");
            if is_record_op || is_commit || is_error {
                let op_id = format!("{}/op{}", self.routine_id, self.op_index);
                // Error advances the op counter but is NOT mapped (legacy
                // op_id_by_node_id omits error — it renders as its cs "error" leaf).
                if !is_error {
                    self.op_id_by_expr.insert(eid, op_id.clone());
                }
                let anchor = self.anchor(&e.origin);
                let loop_stack = self.loop_stack_ids();
                // operation_sites: one entry per op (record-op/lock/commit/error-call),
                // in op-index order. Only error-call carries under_asserterror.
                let (kind, under) = if let Some((op_type, receiver)) = &record_op {
                    // PRecordOperation (pre-backfill) for record DB ops.
                    let (field_arguments, field_argument_infos) =
                        self.record_op_field_args(op_type, &args);
                    self.record_operations.push(PRecordOperation {
                        id: op_id.clone(),
                        op: op_type.to_string(),
                        record_variable_name: receiver.clone(),
                        record_variable_id: None,
                        temp_state: ts_unknown(),
                        field_arguments,
                        field_argument_infos,
                        loop_stack: loop_stack.clone(),
                        source_anchor: anchor.clone(),
                        in_until_condition: self.in_until,
                        // RunTrigger literal arg of a mutating op (Modify/Delete/
                        // DeleteAll → arg 0; ModifyAll → arg 2) — Some(bool) iff that
                        // arg is a boolean literal. L5/d29 input (serde-skipped).
                        run_trigger: {
                            let idx = match *op_type {
                                "Modify" | "Delete" | "DeleteAll" => Some(0),
                                "ModifyAll" => Some(2),
                                _ => None,
                            };
                            idx.and_then(|i| args.get(i)).and_then(|&a| {
                                match &self.file.ir.expr(a).kind {
                                    ExprKind::Literal(al_syntax::ir::Literal::Bool(b)) => Some(*b),
                                    _ => None,
                                }
                            })
                        },
                    });
                    let k = if *op_type == "LockTable" {
                        "lock"
                    } else {
                        "record-op"
                    };
                    (k, None)
                } else if is_commit {
                    ("commit", None)
                } else {
                    (
                        "error-call",
                        if self.assert_depth > 0 {
                            Some(true)
                        } else {
                            None
                        },
                    )
                };
                self.operation_sites.push(POperationSite {
                    id: op_id,
                    kind: kind.to_string(),
                    loop_stack,
                    source_anchor: anchor,
                    under_asserterror: under,
                    control_context: None,
                    order: None,
                });
                self.op_index += 1;
            }
            if !is_record_op && !is_commit {
                let cs_id = format!("{}/cs{}", self.routine_id, self.cs_index);
                self.cs_index += 1;
                self.cs_id_by_expr.insert(eid, cs_id.clone());
                // Full PCallSite (argument_bindings filled by the post-pass).
                let (callee, callee_text) = self.classify_callee(function, &args);
                let is_object_run = matches!(callee, PCallee::ObjectRun { .. });
                let argument_texts: Vec<String> = args
                    .iter()
                    .map(|&a| self.raw(&self.file.ir.expr(a).origin).to_string())
                    .collect();
                let argument_infos: Vec<PExpressionInfo> =
                    args.iter().map(|&a| self.ir_expression_info(a)).collect();
                let under = if self.assert_depth > 0 {
                    Some(true)
                } else {
                    None
                };
                // object-run result_consumed/return_used from the call's position: a
                // bare statement does not consume (result_consumed true only under
                // asserterror; return_used false); an expression-position call does.
                let (result_consumed, object_run_return_used) = if is_object_run {
                    if self.in_stmt_position {
                        (Some(self.assert_depth > 0), Some(false))
                    } else {
                        (Some(true), Some(true))
                    }
                } else {
                    (None, None)
                };
                self.call_sites.push(PCallSite {
                    id: cs_id,
                    operation_id: String::new(),
                    callee_text,
                    callee,
                    argument_texts,
                    argument_infos,
                    argument_bindings: Vec::new(),
                    loop_stack: self.loop_stack_ids(),
                    source_anchor: self.anchor(&e.origin),
                    result_consumed,
                    object_run_return_used,
                    under_asserterror: under,
                    control_context: None,
                    order: None,
                    in_statement_position: self.in_stmt_position,
                    receiver_temp_state: None,
                });
                self.cs_arg_exprs.push(args.clone());
            }
            // Recurse: a member-call RECEIVER is a value ref; a bare callee name is not.
            // Sub-expressions (receiver, args) are EXPRESSION position (consumed).
            self.in_stmt_position = false;
            match &self.file.ir.expr(function).kind {
                Member { object, .. } => {
                    let object = *object;
                    // Walk the receiver for a parens call (legacy harvests its
                    // identifier-refs), or when it is itself a chained call (legacy
                    // `chained_receiver_descent` visits the inner call regardless).
                    let object_is_call = matches!(self.file.ir.expr(object).kind, Call { .. });
                    if is_parens_call || object_is_call {
                        // Chained-call receiver → harvest its function name (legacy
                        // collect_identifiers_from counts the receiver subtree).
                        if object_is_call {
                            self.count_next_call_fn = true;
                        }
                        self.walk_expr(object);
                    }
                }
                // A PARENLESS bare-identifier call (`Modify;`) is a standalone
                // `identifier` node in legacy — its main visit counts it as a value
                // ref BEFORE the record-op routing. A parens `Foo()` callee is NOT
                // counted (the call_expression is not an identifier node).
                Identifier(n) if !is_parens_call && fe.origin.kind_text == "identifier" => {
                    self.identifier_ref_set.insert(n.to_ascii_lowercase());
                }
                Identifier(_) | QuotedIdentifier(_) => {}
                _ => self.walk_expr(function),
            }
            for a in args {
                self.walk_expr(a);
            }
            return;
        }
        match &e.kind {
            Member {
                object,
                member,
                member_origin,
            } => {
                // Value-position `X.Field` where X is a record var → field access,
                // anchored on the member_expression. Enum-scope refs go through the
                // QualifiedEnum arm (object recursed directly), never here.
                if let Identifier(x) | QuotedIdentifier(x) = &self.file.ir.expr(*object).kind
                    && self.scope.frvars.contains(&x.to_ascii_lowercase())
                {
                    let record_variable_name =
                        self.raw(&self.file.ir.expr(*object).origin).to_string();
                    let field_name = if member_origin.kind_text == "quoted_identifier" {
                        strip_quotes(member)
                    } else {
                        member.clone()
                    };
                    let source_anchor = self.anchor(&e.origin);
                    let access = match self.assign_target {
                        Some((t, kind)) if t == eid => kind,
                        _ => crate::program::body::features::FieldAccessKind::Read,
                    };
                    self.field_accesses.push(PFieldAccess {
                        record_variable_name,
                        field_name,
                        source_anchor,
                        access,
                    });
                }
                let object = *object;
                self.walk_expr(object);
            }
            QualifiedEnum { enum_type, .. } => {
                let enum_type = *enum_type;
                match &self.file.ir.expr(enum_type).kind {
                    Member { object, .. } => {
                        let object = *object;
                        self.walk_expr(object);
                    }
                    // A bare-identifier enum TYPE name (`DataScope::Company`) is NOT a
                    // value ref — legacy excludes the enum_type field identifier.
                    Identifier(_) | QuotedIdentifier(_) => {}
                    _ => self.walk_expr(enum_type),
                }
            }
            Binary { lhs, rhs, .. } => {
                let (lhs, rhs) = (*lhs, *rhs);
                self.walk_expr(lhs);
                self.walk_expr(rhs);
            }
            Unary { operand, .. } => {
                let operand = *operand;
                self.walk_expr(operand);
            }
            Parenthesized(x) => {
                let x = *x;
                self.walk_expr(x);
            }
            Index { base, index } => {
                let (base, index) = (*base, *index);
                self.walk_expr(base);
                self.walk_expr(index);
            }
            RangeExpr { start, end } => {
                let (start, end) = (*start, *end);
                self.walk_expr(start);
                self.walk_expr(end);
            }
            Ternary { .. } | TypeOp { .. } | List(_) => {
                for c in e.kind.children() {
                    self.walk_expr(c);
                }
            }
            // Value-reference identifier (lc, deduped) — legacy counts only plain
            // `identifier` nodes, NOT keyword_identifier/quoted_identifier.
            Identifier(name) => {
                if e.origin.kind_text == "identifier" {
                    self.identifier_ref_set.insert(name.to_ascii_lowercase());
                }
            }
            // `Keyword::Name` (database_reference): the object-type keyword is excluded,
            // but an UNQUOTED table_name identifier is a value ref.
            DatabaseReference(text) => {
                if let Some(last) = text.rsplit("::").next() {
                    let t = last.trim();
                    if !t.starts_with('"') {
                        self.identifier_ref_set.insert(t.to_ascii_lowercase());
                    }
                }
            }
            _ => {}
        }
    }
}

/// Walk the routine body, producing the op/cs id maps + has_branching + the
/// anchored `loops` and `field_accesses`.
#[allow(clippy::too_many_arguments)]
pub fn walk_spine(
    file: &AlFile,
    object_idx: usize,
    routine: &RoutineDecl,
    routine_id: &str,
    source: &str,
    cols: &Utf16Cols,
    source_unit_id: &str,
    source_table_name: Option<&str>,
    parenless_calls: &HashSet<ExprId>,
) -> IrSpine {
    let (scope, implicit_rec) = build_scope(file, object_idx, routine, source_table_name);
    let mut ctx = SpineCtx {
        file,
        parenless_calls,
        scope: &scope,
        routine_id,
        source,
        cols,
        source_unit_id,
        op_index: 0,
        cs_index: 0,
        loop_count: 0,
        has_branching: false,
        // Base implicit frame: an object with an implicit `Rec` (table/tableext/
        // pageext, page-with-SourceTable, codeunit-TableNo) seeds a record frame so a
        // bare record-op call resolves to `Rec`.
        implicit: vec![ImplicitFrame {
            is_record: implicit_rec,
            text: "Rec".to_string(),
        }],
        cur_loops: Vec::new(),
        op_id_by_expr: HashMap::new(),
        cs_id_by_expr: HashMap::new(),
        loops: Vec::new(),
        field_accesses: Vec::new(),
        var_assignments: Vec::new(),
        condition_references: Vec::new(),
        identifier_ref_set: HashSet::new(),
        unreachable_statements: Vec::new(),
        unreachable_index: 0,
        record_operations: Vec::new(),
        operation_sites: Vec::new(),
        assert_depth: 0,
        in_stmt_position: false,
        count_next_call_fn: false,
        in_until: false,
        call_sites: Vec::new(),
        cs_arg_exprs: Vec::new(),
        bare_assigns: Vec::new(),
        assign_target: None,
    };
    if let Some(b) = routine.body {
        ctx.walk_block(b);
    }
    // Two-phase numbering: each call site's operation_id = op{op_count + i}, where
    // op_count is the final op total and i the call-site index (legacy body_walk tail).
    let op_count = ctx.op_index;
    for (i, cs) in ctx.call_sites.iter_mut().enumerate() {
        cs.operation_id = format!("{}/op{}", routine_id, op_count + i as u32);
    }
    // Post-pass ordering to match legacy: var_assignments by source anchor,
    // condition_references by reference anchor, identifier_references sorted+deduped.
    let mut var_assignments = ctx.var_assignments;
    var_assignments.sort_by(|a, b| {
        (a.source_anchor.start_line, a.source_anchor.start_column)
            .cmp(&(b.source_anchor.start_line, b.source_anchor.start_column))
    });
    let mut condition_references = ctx.condition_references;
    condition_references.sort_by(|a, b| {
        (
            a.reference_anchor.start_line,
            a.reference_anchor.start_column,
        )
            .cmp(&(
                b.reference_anchor.start_line,
                b.reference_anchor.start_column,
            ))
    });
    let mut identifier_references: Vec<String> = ctx.identifier_ref_set.into_iter().collect();
    identifier_references.sort();
    IrSpine {
        op_id_by_expr: ctx.op_id_by_expr,
        cs_id_by_expr: ctx.cs_id_by_expr,
        has_branching: ctx.has_branching,
        loop_count: ctx.loop_count,
        loops: ctx.loops,
        field_accesses: ctx.field_accesses,
        var_assignments,
        condition_references,
        identifier_references,
        unreachable_statements: ctx.unreachable_statements,
        record_operations: ctx.record_operations,
        operation_sites: ctx.operation_sites,
        call_sites: ctx.call_sites,
        cs_arg_exprs: ctx.cs_arg_exprs,
    }
}

// ---- CFN (statement_tree) as a real PCFNNode ----

fn cfn_node(kind: &str) -> PCFNNode {
    PCFNNode {
        kind: kind.to_string(),
        operation_id: None,
        callsite_id: None,
        condition_guard: None,
        condition_leaves: None,
        children: None,
        else_children: None,
        is_case_else: false,
        source_range: None,
    }
}

/// Builds the real `PCFNNode` statement_tree from the IR + the op/cs id maps.
pub struct IrCfn<'a> {
    pub file: &'a AlFile,
    pub spine: &'a IrSpine,
    pub cols: &'a Utf16Cols<'a>,
}

impl<'a> IrCfn<'a> {
    fn ir(&self) -> &al_syntax::ir::Ir {
        &self.file.ir
    }

    /// The node's TRUE source range `(startLine, startColumn, endLine, endColumn)` in
    /// the PAnchor basis (0-based row, utf16 column) — the SAME basis the legacy
    /// `cfn.rs` `range_of` produces. The L4 branch-aware walker reads this (serde-skip
    /// `PCFNNode.source_range`) to attribute field accesses to the right block level;
    /// WITHOUT it, `node_range` reconstructs a too-narrow range from op/cs leaves only
    /// and drops statement-level field reads (the L3-swap r3a2 divergence).
    fn range_of(&self, origin: &Origin) -> Option<(u32, u32, u32, u32)> {
        Some((
            origin.start.row,
            self.cols
                .col(origin.start.row as usize, origin.start.column as usize),
            origin.end.row,
            self.cols
                .col(origin.end.row as usize, origin.end.column as usize),
        ))
    }

    fn block_items(&self, bid: BlockId) -> Vec<PCFNNode> {
        let mut out = Vec::new();
        for item in &self.file.ir.block(bid).items {
            match item {
                BlockItem::Stmt(s) => {
                    if let Some(c) = self.build_statement(*s) {
                        out.push(c);
                    }
                }
                BlockItem::Preproc(g) => {
                    for b in &g.branches {
                        out.extend(self.block_items(*b));
                    }
                }
            }
        }
        out
    }

    pub fn build_block(&self, bid: BlockId) -> PCFNNode {
        let mut n = cfn_node("block");
        n.source_range = self.range_of(&self.ir().block(bid).origin);
        n.children = Some(self.block_items(bid));
        n
    }

    fn is_error_fn(&self, function: ExprId) -> bool {
        use ExprKind::*;
        matches!(&self.ir().expr(function).kind, Identifier(m) | QuotedIdentifier(m) if m.eq_ignore_ascii_case("error"))
    }

    fn harvest_receiver(&self, function: ExprId, out: &mut Vec<PCFNNode>) {
        use ExprKind::*;
        if let Member { object, .. } = &self.ir().expr(function).kind
            && matches!(&self.ir().expr(*object).kind, Call { .. } | Member { .. })
        {
            self.harvest(*object, out);
        }
    }

    fn harvest(&self, eid: ExprId, out: &mut Vec<PCFNNode>) {
        use ExprKind::*;
        let e = self.ir().expr(eid);
        // A parens-less call: the walk gave the read itself a call-site id.
        if !matches!(e.kind, Call { .. })
            && let Some(cs) = self.spine.cs_id_by_expr.get(&eid)
        {
            self.harvest_receiver(eid, out);
            let mut leaf = cfn_node("call");
            leaf.callsite_id = Some(cs.clone());
            out.push(leaf);
            return;
        }
        match &e.kind {
            Call { function, args } => {
                if let Some(op) = self.spine.op_id_by_expr.get(&eid) {
                    let mut inner = Vec::new();
                    for a in args {
                        self.harvest(*a, &mut inner);
                    }
                    self.harvest_receiver(*function, out);
                    let mut leaf = cfn_node("op");
                    leaf.operation_id = Some(op.clone());
                    if !inner.is_empty() {
                        leaf.condition_leaves = Some(inner);
                    }
                    out.push(leaf);
                    return;
                }
                if let Some(cs) = self.spine.cs_id_by_expr.get(&eid) {
                    let mut inner = Vec::new();
                    for a in args {
                        self.harvest(*a, &mut inner);
                    }
                    self.harvest_receiver(*function, out);
                    let mut leaf = cfn_node(if self.is_error_fn(*function) {
                        "error"
                    } else {
                        "call"
                    });
                    leaf.callsite_id = Some(cs.clone());
                    if !inner.is_empty() {
                        leaf.condition_leaves = Some(inner);
                    }
                    out.push(leaf);
                    return;
                }
                self.harvest(*function, out);
                for a in args {
                    self.harvest(*a, out);
                }
            }
            Member { object, .. } => self.harvest(*object, out),
            Binary { lhs, rhs, .. } => {
                self.harvest(*lhs, out);
                self.harvest(*rhs, out);
            }
            Unary { operand, .. } => self.harvest(*operand, out),
            Parenthesized(x) => self.harvest(*x, out),
            Index { base, index } => {
                self.harvest(*base, out);
                self.harvest(*index, out);
            }
            QualifiedEnum { enum_type, .. } => self.harvest(*enum_type, out),
            RangeExpr { start, end } => {
                self.harvest(*start, out);
                self.harvest(*end, out);
            }
            Ternary { .. } | TypeOp { .. } | List(_) => {
                for c in e.kind.children() {
                    self.harvest(c, out);
                }
            }
            _ => {}
        }
    }

    fn build_stmt_call(&self, eid: ExprId) -> PCFNNode {
        use ExprKind::*;
        let e = self.ir().expr(eid);
        let Call { function, args } = &e.kind else {
            return cfn_node("other");
        };
        let mut pre = Vec::new();
        self.harvest_receiver(*function, &mut pre);
        for a in args {
            self.harvest(*a, &mut pre);
        }
        let mut leaf = if let Some(op) = self.spine.op_id_by_expr.get(&eid) {
            let mut l = cfn_node("op");
            l.operation_id = Some(op.clone());
            l
        } else if let Some(cs) = self.spine.cs_id_by_expr.get(&eid) {
            let mut l = cfn_node(if self.is_error_fn(*function) {
                "error"
            } else {
                "call"
            });
            l.callsite_id = Some(cs.clone());
            l
        } else {
            cfn_node("other")
        };
        if !pre.is_empty() {
            leaf.condition_leaves = Some(pre);
        }
        leaf
    }

    fn simple_guard(&self, cond: ExprId) -> Option<PConditionGuard> {
        use ExprKind::*;
        let e = self.ir().expr(cond);
        match &e.kind {
            Identifier(n) if e.origin.kind_text == "identifier" => Some(PConditionGuard {
                identifier: n.to_ascii_lowercase(),
                polarity: "positive".to_string(),
            }),
            Unary {
                op: UnaryOp::Not,
                operand,
            } => match &self.ir().expr(*operand).kind {
                Identifier(n) if self.ir().expr(*operand).origin.kind_text == "identifier" => {
                    Some(PConditionGuard {
                        identifier: n.to_ascii_lowercase(),
                        polarity: "negative".to_string(),
                    })
                }
                _ => None,
            },
            Binary {
                op: BinaryOp::Eq,
                lhs,
                rhs,
            } => {
                let id_side =
                    [*lhs, *rhs]
                        .into_iter()
                        .find_map(|s| match &self.ir().expr(s).kind {
                            Identifier(n) if self.ir().expr(s).origin.kind_text == "identifier" => {
                                Some(n.to_ascii_lowercase())
                            }
                            _ => None,
                        });
                let false_side = [*lhs, *rhs].into_iter().any(|s| {
                    matches!(
                        &self.ir().expr(s).kind,
                        Literal(al_syntax::ir::Literal::Bool(false))
                    )
                });
                match (id_side, false_side) {
                    (Some(id), true) => Some(PConditionGuard {
                        identifier: id,
                        polarity: "negative".to_string(),
                    }),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    fn harvest_vec(&self, eid: ExprId) -> Vec<PCFNNode> {
        let mut v = Vec::new();
        self.harvest(eid, &mut v);
        v
    }

    fn build_statement(&self, sid: al_syntax::ir::StmtId) -> Option<PCFNNode> {
        use StmtKind::*;
        let st = self.ir().stmt(sid);
        let some_if = |v: Vec<PCFNNode>| if v.is_empty() { None } else { Some(v) };
        let mut node = match &st.kind {
            Call(e) => self.build_stmt_call(*e),
            If {
                cond,
                then_block,
                else_block,
            } => {
                let mut n = cfn_node("if");
                n.children = Some(vec![self.build_block(*then_block)]);
                n.else_children = else_block.map(|b| vec![self.build_block(b)]);
                n.condition_leaves = some_if(self.harvest_vec(*cond));
                n.condition_guard = self.simple_guard(*cond);
                n
            }
            Case {
                scrutinee,
                branches,
                else_block,
            } => {
                let mut branch_cfns = Vec::new();
                for br in branches {
                    let mut b = cfn_node("case-branch");
                    b.source_range = self.range_of(&br.origin);
                    b.children = Some(vec![self.build_block(br.body)]);
                    branch_cfns.push(b);
                }
                if let Some(eb) = else_block {
                    let mut b = cfn_node("case-branch");
                    b.is_case_else = true;
                    b.source_range = self.range_of(&self.ir().block(*eb).origin);
                    b.children = Some(vec![self.build_block(*eb)]);
                    branch_cfns.push(b);
                }
                let mut n = cfn_node("case");
                n.children = Some(branch_cfns);
                n.condition_leaves = some_if(self.harvest_vec(*scrutinee));
                n
            }
            While { cond, body } => {
                let mut n = cfn_node("while");
                n.children = Some(vec![self.build_block(*body)]);
                n.condition_leaves = some_if(self.harvest_vec(*cond));
                n
            }
            For { from, to, body, .. } => {
                let mut n = cfn_node("for");
                n.children = Some(vec![self.build_block(*body)]);
                let mut leaves = self.harvest_vec(*from);
                leaves.extend(self.harvest_vec(*to));
                n.condition_leaves = some_if(leaves);
                n
            }
            Foreach { iterable, body, .. } => {
                let mut n = cfn_node("foreach");
                n.children = Some(vec![self.build_block(*body)]);
                n.condition_leaves = some_if(self.harvest_vec(*iterable));
                n
            }
            Repeat { body, until } => {
                let mut n = cfn_node("repeat");
                n.children = Some(self.block_items(*body));
                n.condition_leaves = some_if(self.harvest_vec(*until));
                n
            }
            Try { .. } => {
                let mut n = cfn_node("try");
                n.children = Some(vec![]);
                n
            }
            Exit(x) => {
                let mut n = cfn_node("exit");
                n.condition_leaves = x.map(|e| self.harvest_vec(e)).and_then(some_if);
                n
            }
            With { body, .. } | AssertError(body) => {
                let mut n = cfn_node("other");
                n.children = Some(vec![self.build_block(*body)]);
                n
            }
            Assignment { target, value, .. } => {
                let mut leaves = self.harvest_vec(*target);
                leaves.extend(self.harvest_vec(*value));
                let mut n = cfn_node("other");
                n.condition_leaves = some_if(leaves);
                n
            }
            Block(b) => self.build_block(*b),
            // `break`/`continue` get their OWN CFN kinds (not the inert `other`)
            // so the L4 dataflow walker (cfg_walker.rs) can join their at-break /
            // at-continue state into the enclosing loop's exit/head instead of
            // silently threading state through statements they actually skip.
            // Other flat-children consumers (control_context.rs, return_summary.rs,
            // dep_artifact_l4.rs, operation_order.rs, control_flow.rs) fall through
            // their generic default arm for any unmatched kind — verified they do
            // not special-case "other" vs an unrecognised kind, so this rename is
            // behavior-preserving for them.
            Break => cfn_node("break"),
            Continue => cfn_node("continue"),
            Unknown => cfn_node("other"),
        };
        // The L4 walker reads `source_range` (serde-skip) to attribute field accesses
        // to the right block level. Set it to the statement's range for any node that
        // did not already carry its own (build_block sets the block's origin).
        if node.source_range.is_none() {
            node.source_range = self.range_of(&st.origin);
        }
        Some(node)
    }
}

/// The slice of `PFeatures` the IR walk currently produces as REAL engine types.
/// Grows toward full `PFeatures` as the rich call_site/record_operation payloads
/// are threaded in subsequent increments.
pub struct IrPartialFeatures {
    pub statement_tree: Option<PCFNNode>,
    pub has_branching: bool,
    pub nesting_depth: u32,
    pub loops: Vec<PLoop>,
    pub field_accesses: Vec<PFieldAccess>,
    pub var_assignments: Vec<PVarAssignment>,
    pub condition_references: Vec<PConditionReference>,
    pub identifier_references: Vec<String>,
    pub unreachable_statements: Vec<PUnreachableStatement>,
    pub record_operations: Vec<PRecordOperation>,
    pub operation_sites: Vec<POperationSite>,
    pub call_sites: Vec<PCallSite>,
}

/// Build the validated slice of `PFeatures` from the owned IR for one routine.
/// `source_table_name` gates the page implicit `Rec` for `ir_record_variables`
/// (which supplies the record-op temp_state/record_variable_id backfill).
#[allow(clippy::too_many_arguments)]
pub fn routine_features_partial(
    file: &AlFile,
    object_idx: usize,
    routine: &RoutineDecl,
    routine_id: &str,
    source: &str,
    source_unit_id: &str,
    source_table_name: Option<&str>,
    parenless_calls: &HashSet<ExprId>,
) -> IrPartialFeatures {
    let cols = Utf16Cols::new(source);
    let spine = walk_spine(
        file,
        object_idx,
        routine,
        routine_id,
        source,
        &cols,
        source_unit_id,
        source_table_name,
        parenless_calls,
    );
    let statement_tree = routine.body.map(|b| {
        let cfn = IrCfn {
            file,
            spine: &spine,
            cols: &cols,
        };
        cfn.build_block(b)
    });
    let nesting_depth = super::compute_nesting_depth(&spine.loops);

    // recordOperation backfill: copy temp_state + record_variable_id from the
    // declaring RecordVariable (matched by lc name) — mirrors extract_body_features.
    let rec_vars = ir_record_variables(file, object_idx, routine, routine_id, source_table_name);
    let rv_by_lc: HashMap<String, &super::features::PRecordVariable> = rec_vars
        .iter()
        .map(|rv| (rv.name.to_ascii_lowercase(), rv))
        .collect();
    let mut record_operations = spine.record_operations;
    for op in &mut record_operations {
        if let Some(rv) = rv_by_lc.get(&op.record_variable_name.to_ascii_lowercase()) {
            if op.record_variable_id.is_none() {
                op.record_variable_id = Some(rv.id.clone());
            }
            op.temp_state = rv.temp_state.clone();
        }
    }

    // argument_bindings post-pass: each call-site arg → its source (parameter /
    // implicit-rec / local record / unknown / expression). Mirrors
    // extract_argument_bindings, using ir_record_variables + the IR params.
    let anchor = |o: &Origin| PAnchor {
        source_unit_id: source_unit_id.to_string(),
        start_line: o.start.row,
        start_column: cols.col(o.start.row as usize, o.start.column as usize),
        end_line: o.end.row,
        end_column: cols.col(o.end.row as usize, o.end.column as usize),
        syntax_kind: o.kind_text.to_string(),
    };
    // param lc-name → (index, is_var).
    let param_by_lc: HashMap<String, (u32, bool)> = routine
        .params
        .iter()
        .enumerate()
        .map(|(i, p)| (p.name.to_ascii_lowercase(), (i as u32, p.by_ref)))
        .collect();
    let mut call_sites = spine.call_sites;
    for (cs, arg_exprs) in call_sites.iter_mut().zip(spine.cs_arg_exprs.iter()) {
        // The record the call is made on (a table method's `Rec`): the member
        // receiver, or the implicit `Rec` for a bare call. Object globals are
        // filled at L3 (`workspace.rs`), as for argument bindings.
        let receiver_lc = match &cs.callee {
            super::features::PCallee::Member { receiver, .. } => {
                Some(receiver.trim().trim_matches('"').to_ascii_lowercase())
            }
            super::features::PCallee::Bare { .. } => Some("rec".to_string()),
            _ => None,
        };
        cs.receiver_temp_state = receiver_lc
            .and_then(|lc| rv_by_lc.get(&lc))
            .map(|rv| rv.temp_state.clone());
        cs.argument_bindings = arg_exprs
            .iter()
            .enumerate()
            .map(|(parameter_index, &a)| {
                let e = file.ir.expr(a);
                let argument_anchor = anchor(&e.origin);
                let parameter_index = parameter_index as u32;
                // Only bare-identifier args bind to a record/parameter symbol.
                let lc = match &e.kind {
                    ExprKind::Identifier(n) if e.origin.kind_text == "identifier" => {
                        n.to_ascii_lowercase()
                    }
                    _ => {
                        return PCallArgumentBinding {
                            parameter_index,
                            source_kind: "expression".to_string(),
                            source_variable_name: None,
                            source_record_variable_id: None,
                            source_parameter_index: None,
                            caller_source_parameter_is_var: None,
                            source_temp_state: None,
                            argument_anchor,
                        };
                    }
                };
                let param = param_by_lc.get(&lc);
                let rec_var = rv_by_lc.get(&lc);
                let is_implicit_rec = (lc == "rec" || lc == "xrec")
                    && rec_var.map(|rv| rv.table_name.is_none()).unwrap_or(true);
                let source_kind = if param.is_some() {
                    "parameter"
                } else if is_implicit_rec {
                    "implicit-rec"
                } else if rec_var.is_some() {
                    "local"
                } else {
                    "unknown"
                };
                let bound_rec_var = if is_implicit_rec { None } else { rec_var };
                let source_temp_state = match bound_rec_var {
                    Some(rv) => Some(rv.temp_state.clone()),
                    None if is_implicit_rec => rec_var.map(|rv| rv.temp_state.clone()),
                    None => None,
                };
                PCallArgumentBinding {
                    parameter_index,
                    source_kind: source_kind.to_string(),
                    source_variable_name: if source_kind == "unknown" {
                        None
                    } else {
                        Some(lc.clone())
                    },
                    source_record_variable_id: bound_rec_var.map(|rv| rv.id.clone()),
                    source_parameter_index: param.map(|(i, _)| *i),
                    caller_source_parameter_is_var: param.map(|(_, v)| *v),
                    source_temp_state,
                    argument_anchor,
                }
            })
            .collect();
    }

    IrPartialFeatures {
        statement_tree,
        has_branching: spine.has_branching,
        nesting_depth,
        loops: spine.loops,
        field_accesses: spine.field_accesses,
        var_assignments: spine.var_assignments,
        condition_references: spine.condition_references,
        identifier_references: spine.identifier_references,
        unreachable_statements: spine.unreachable_statements,
        record_operations,
        operation_sites: spine.operation_sites,
        call_sites,
    }
}

/// Assemble the FULL `PFeatures` for one routine from the owned IR — the complete
/// `project_routine_features_ir`. `order` / `control_context` / `scope_frames` are
/// left empty (post-pass fields that `extract_body_features` also leaves empty;
/// the L2 emitter applies operation_order/control_context on the `statement_tree`).
#[allow(clippy::too_many_arguments)]
pub fn project_routine_features_ir(
    file: &AlFile,
    object_idx: usize,
    routine: &RoutineDecl,
    routine_id: &str,
    source: &str,
    source_unit_id: &str,
    source_table_name: Option<&str>,
) -> super::features::PFeatures {
    project_routine_features_ir_with(
        file,
        object_idx,
        routine,
        routine_id,
        source,
        source_unit_id,
        source_table_name,
        &HashSet::new(),
    )
}

/// [`project_routine_features_ir`] with the parens-less value reads the program
/// resolver kept as calls (`program::resolve::full::ParenlessCalls`).
#[allow(clippy::too_many_arguments)]
pub fn project_routine_features_ir_with(
    file: &AlFile,
    object_idx: usize,
    routine: &RoutineDecl,
    routine_id: &str,
    source: &str,
    source_unit_id: &str,
    source_table_name: Option<&str>,
    parenless_calls: &HashSet<ExprId>,
) -> super::features::PFeatures {
    let p = routine_features_partial(
        file,
        object_idx,
        routine,
        routine_id,
        source,
        source_unit_id,
        source_table_name,
        parenless_calls,
    );
    let record_variables =
        ir_record_variables(file, object_idx, routine, routine_id, source_table_name);
    let variables = ir_variables(file, object_idx, routine, source, source_unit_id);
    super::features::PFeatures {
        loops: p.loops,
        operation_sites: p.operation_sites,
        record_operations: p.record_operations,
        call_sites: p.call_sites,
        field_accesses: p.field_accesses,
        record_variables,
        nesting_depth: p.nesting_depth,
        has_branching: p.has_branching,
        unreachable_statements: p.unreachable_statements,
        identifier_references: p.identifier_references,
        variables,
        var_assignments: p.var_assignments,
        condition_references: p.condition_references,
        statement_tree: p.statement_tree,
        scope_frames: Vec::new(),
    }
}

// ---- record_variables (params + locals + implicit Rec) ----

/// Table name from a `Record …` type string (`Record Customer` /
/// `Record "Sales Header"` / `Record Customer temporary`). None if not a record /
/// no subtype.
pub(crate) fn parse_record_table_name(ty: &str) -> Option<String> {
    let t = ty.trim();
    if !is_record_type_str(t) {
        return None;
    }
    let rest = t[6..].trim_start();
    if rest.is_empty() {
        return None;
    }
    if let Some(after) = rest.strip_prefix('"') {
        let end = after.find('"')?;
        Some(after[..end].to_string())
    } else {
        rest.split_whitespace().next().map(|w| w.to_string())
    }
}

/// True when `ty` is an AL `Record` variable type (`Record`, `Record Customer`,
/// `Record "Sales Header" temporary`). MUST NOT match the distinct built-in types
/// `RecordRef` / `RecordId` — legacy keys off the grammar's `record_type` node, which
/// those don't produce; a naive `starts_with("record")` wrongly classifies them as
/// record variables (then their record ops get a spurious Known(false) temp_state via
/// the backfill — the L3 RecordRef-flow divergence). The discriminator: after the
/// `record` prefix the next char must be whitespace or `"` (a table follows), or the
/// type is exactly `record`.
pub(crate) fn is_record_type_str(ty: &str) -> bool {
    let lc = ty.trim().to_ascii_lowercase();
    match lc.strip_prefix("record") {
        Some(rest) => rest.is_empty() || rest.starts_with(' ') || rest.starts_with('"'),
        None => false,
    }
}

/// Per-routine `record_variables` (`PRecordVariable`) from the owned IR: record
/// parameters, local record vars, and the implicit `Rec` of a table/page/pageext/
/// codeunit-TableNo method. Mirrors `scope::extract_record_variables` + the
/// implicit-Rec seeding in `project_routine_features`. NOT yet modelled (needs IR
/// extensions): a named return-value record, and report dataitem record vars.
pub fn ir_record_variables(
    file: &AlFile,
    object_idx: usize,
    routine: &RoutineDecl,
    routine_id: &str,
    source_table_name: Option<&str>,
) -> Vec<super::features::PRecordVariable> {
    use super::features::PRecordVariable;
    use super::scope::{ts_known, ts_param_dependent};
    let o = &file.objects[object_idx];
    let mut out: Vec<PRecordVariable> = Vec::new();

    // Record parameters (index = position among ALL params).
    for (i, p) in routine.params.iter().enumerate() {
        let Some(ty) = p.ty.as_deref() else { continue };
        if !is_record_type_str(ty) {
            continue;
        }
        let is_temp = ty.to_ascii_lowercase().contains("temporary");
        let temp_state = if is_temp {
            ts_known(true)
        } else if p.by_ref {
            ts_param_dependent(i as u32)
        } else {
            ts_known(false)
        };
        out.push(PRecordVariable {
            id: format!("{}/rv/{}", routine_id, p.name.to_lowercase()),
            name: p.name.clone(),
            table_name: parse_record_table_name(ty),
            temp_state,
            is_parameter: true,
            parameter_index: Some(i as u32),
            scope: None,
        });
    }

    // Local record declarations.
    for v in &routine.locals {
        let Some(ty) = v.ty.as_deref() else { continue };
        if !is_record_type_str(ty) {
            continue;
        }
        out.push(PRecordVariable {
            id: format!("{}/rv/{}", routine_id, v.name.to_lowercase()),
            name: v.name.clone(),
            table_name: parse_record_table_name(ty),
            temp_state: ts_known(v.temporary),
            is_parameter: false,
            parameter_index: None,
            scope: None,
        });
    }

    // Implicit `Rec` (skip when a declared `Rec` already exists). table/tableext
    // methods + pageext always; page only with a SourceTable; codeunit only with a
    // TableNo (whose value is the Rec's table_name). table/page/pageext leave
    // table_name None (L3 fills from the effective own table).
    let prop = |name: &str| {
        o.properties
            .iter()
            .find(|p| p.name == name)
            .map(|p| p.value.as_str())
    };
    let strip_q = |s: &str| s.trim().trim_matches('"').to_string();
    let codeunit_tableno = if o.kind == ObjectKind::Codeunit {
        prop("tableno").map(strip_q).filter(|s| !s.is_empty())
    } else {
        None
    };
    // A report DATAITEM trigger's implicit `Rec` is typed to its enclosing dataitem's
    // source table (set directly, like codeunit TableNo). The two are mutually
    // exclusive (a routine is in a codeunit OR a report); mirror legacy precedence
    // (dataitem first) for `direct_rec_table`.
    let direct_rec_table = routine.dataitem_source_table.clone().or(codeunit_tableno);
    // Page implicit Rec is gated on the RESOLVED source_table_name (as in
    // `implicit_base_receiver`), not merely the SourceTable property's presence —
    // mirroring `project_routine_features`'s `source_table_name` arg.
    let has_implicit_rec = matches!(
        o.kind,
        ObjectKind::Table | ObjectKind::TableExtension | ObjectKind::PageExtension
    ) || (o.kind == ObjectKind::Page && source_table_name.is_some())
        || direct_rec_table.is_some();
    if has_implicit_rec && !out.iter().any(|v| v.name.eq_ignore_ascii_case("Rec")) {
        // A table (extension) PROCEDURE runs on the record it is called on, so its
        // `Rec` is as temporary as the caller's receiver: parameter-dependent on
        // the reserved receiver index, substituted at each call site from
        // `PCallSite::receiver_temp_state`. Triggers keep `Known(false)`: the
        // platform invokes them, and no call site carries their receiver.
        let rec_temp_state = if matches!(o.kind, ObjectKind::Table | ObjectKind::TableExtension)
            && routine.kind == al_syntax::ir::RoutineKind::Procedure
        {
            ts_param_dependent(super::features::RECEIVER_PARAM_INDEX)
        } else {
            ts_known(false)
        };
        out.push(PRecordVariable {
            id: format!("{}/rv/rec", routine_id),
            name: "Rec".to_string(),
            table_name: direct_rec_table,
            temp_state: rec_temp_state,
            is_parameter: false,
            parameter_index: None,
            scope: None,
        });
    }

    // Report dataitem NAMES are in scope as record vars across ALL the report's
    // routines (a dataitem may be referenced by name as a record typed to its source
    // table). Seed each, skipping any name already declared (never shadow). Mirrors
    // `project_routine_features`'s report_dataitem_record_vars block.
    if matches!(
        o.kind,
        ObjectKind::Report | ObjectKind::ReportExtension | ObjectKind::XmlPort
    ) {
        for (di_name, di_table) in &o.dataitems {
            if out.iter().any(|v| v.name.eq_ignore_ascii_case(di_name)) {
                continue;
            }
            out.push(PRecordVariable {
                id: format!("{}/rv/dataitem/{}", routine_id, di_name),
                name: di_name.clone(),
                table_name: Some(di_table.clone()),
                temp_state: ts_known(false),
                is_parameter: false,
                parameter_index: None,
                scope: None,
            });
        }
    }

    out
}

/// Object-global RECORD variables (`PRecordVariable`, scope=global) from the IR's
/// object globals — mirrors `scope::extract_object_global_record_vars`. The L3 emitter
/// promotes these into each routine's record_variables (honoring shadowing). First-wins
/// by lowercased name across `#if`/`#else` branches (the IR carries both).
pub fn ir_object_global_record_vars(
    o: &al_syntax::ir::ObjectDecl,
    object_id: &str,
) -> Vec<super::features::PRecordVariable> {
    use super::features::PRecordVariable;
    use super::scope::ts_known;
    let mut out = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for v in &o.globals {
        let Some(ty) = v.ty.as_deref() else { continue };
        if !is_record_type_str(ty) {
            continue;
        }
        let name_lc = v.name.to_ascii_lowercase();
        if !seen.insert(name_lc.clone()) {
            continue;
        }
        out.push(PRecordVariable {
            id: format!("{}/grv/{}", object_id, name_lc),
            name: v.name.clone(),
            table_name: parse_record_table_name(ty),
            temp_state: ts_known(v.temporary),
            is_parameter: false,
            parameter_index: None,
            scope: Some("global".to_string()),
        });
    }
    out
}

// ---- entry temp-contract guard (G-2): `if not X.IsTemporary then Error(...)` ----

fn ir_unwrap_parens(file: &AlFile, eid: ExprId) -> ExprId {
    let mut e = eid;
    while let ExprKind::Parenthesized(inner) = &file.ir.expr(e).kind {
        e = *inner;
    }
    e
}

/// `<X>.IsTemporary` / `<X>.IsTemporary()` with `<X>` a BARE identifier / quoted id →
/// the receiver name (unquoted, lowercased). Mirrors `istemporary_receiver`.
fn ir_istemporary_receiver(file: &AlFile, eid: ExprId) -> Option<String> {
    let eid = ir_unwrap_parens(file, eid);
    let member_eid = match &file.ir.expr(eid).kind {
        // `IsTemporary()` takes no arguments.
        ExprKind::Call { function, args } if args.is_empty() => *function,
        ExprKind::Member { .. } => eid,
        _ => return None,
    };
    let ExprKind::Member { object, member, .. } = &file.ir.expr(member_eid).kind else {
        return None;
    };
    if !member
        .trim()
        .trim_matches('"')
        .eq_ignore_ascii_case("istemporary")
    {
        return None;
    }
    match &file.ir.expr(*object).kind {
        ExprKind::Identifier(n) | ExprKind::QuotedIdentifier(n) => {
            Some(n.trim().to_ascii_lowercase())
        }
        _ => None,
    }
}

/// The receiver of an `if not <X>.IsTemporary[()] then Error(...)` (or
/// `<X>.IsTemporary = false`) guard STATEMENT — `<X>` a bare identifier, the
/// then-branch a single PARENFUL `Error(...)` call. Mirrors `is_temporary_error_guard`.
fn ir_is_temporary_error_guard(file: &AlFile, sid: al_syntax::ir::StmtId) -> Option<String> {
    use al_syntax::ir::{BinaryOp, BlockItem, Literal, UnaryOp};
    let StmtKind::If {
        cond,
        then_block,
        else_block,
    } = &file.ir.stmt(sid).kind
    else {
        return None;
    };
    if else_block.is_some() {
        return None;
    }
    let cond_eid = ir_unwrap_parens(file, *cond);
    let receiver = match &file.ir.expr(cond_eid).kind {
        ExprKind::Unary {
            op: UnaryOp::Not,
            operand,
        } => ir_istemporary_receiver(file, *operand)?,
        ExprKind::Binary {
            op: BinaryOp::Eq,
            lhs,
            rhs,
        } => {
            let is_false = |e: ExprId| {
                matches!(
                    &file.ir.expr(ir_unwrap_parens(file, e)).kind,
                    ExprKind::Literal(Literal::Bool(false))
                )
            };
            if is_false(*rhs) {
                ir_istemporary_receiver(file, *lhs)?
            } else if is_false(*lhs) {
                ir_istemporary_receiver(file, *rhs)?
            } else {
                return None;
            }
        }
        _ => return None,
    };
    // then-branch: a single PARENFUL `Error(...)` call (call_expression, not a parenless
    // call statement — matching legacy `is_error_call`'s `kind == "call_expression"`).
    let stmts: Vec<_> = file
        .ir
        .block(*then_block)
        .items
        .iter()
        .filter_map(|it| match it {
            BlockItem::Stmt(s) => Some(*s),
            _ => None,
        })
        .collect();
    if stmts.len() != 1 {
        return None;
    }
    let StmtKind::Call(call_eid) = &file.ir.stmt(stmts[0]).kind else {
        return None;
    };
    let call = file.ir.expr(*call_eid);
    if call.origin.kind_text != "call_expression" {
        return None;
    }
    let ExprKind::Call { function, .. } = &call.kind else {
        return None;
    };
    match &file.ir.expr(*function).kind {
        ExprKind::Identifier(n) if n.eq_ignore_ascii_case("error") => Some(receiver),
        _ => None,
    }
}

/// IR port of `entry_temp_guard_receiver_of` (the receiver only — the caller verifies it
/// is a record var). The guard must be the FIRST body statement.
pub fn ir_entry_temp_guard_receiver(file: &AlFile, routine: &RoutineDecl) -> Option<String> {
    let body = routine.body?;
    let first = file.ir.block(body).items.iter().find_map(|it| match it {
        al_syntax::ir::BlockItem::Stmt(s) => Some(*s),
        _ => None,
    })?;
    ir_is_temporary_error_guard(file, first)
}

/// IR port of `table_has_temp_contract_guard` (G-2 Part 1): a table whose
/// OnInsert/OnModify/OnDelete/OnRename trigger has a top-level
/// `if not Rec.IsTemporary then Error(...)` guard is temporary by runtime contract.
pub fn ir_table_has_temp_contract_guard(file: &AlFile, o: &al_syntax::ir::ObjectDecl) -> bool {
    use al_syntax::ir::{BlockItem, RoutineKind};
    for r in &o.routines {
        if r.kind != RoutineKind::Trigger {
            continue;
        }
        if !matches!(
            r.name.to_ascii_lowercase().as_str(),
            "oninsert" | "onmodify" | "ondelete" | "onrename"
        ) {
            continue;
        }
        let Some(body) = r.body else { continue };
        for it in &file.ir.block(body).items {
            if let BlockItem::Stmt(s) = it
                && ir_is_temporary_error_guard(file, *s).as_deref() == Some("rec")
            {
                return true;
            }
        }
    }
    false
}

// ---- variables (PVariableSymbol: params + locals + object globals) ----

/// Classify a variable initializer's RHS expression into the legacy ValueSource
/// JSON (`classify_rhs`).
fn ir_classify_rhs(file: &AlFile, eid: ExprId, source: &str) -> serde_json::Value {
    use al_syntax::ir::Literal::*;
    use serde_json::json;
    let e = file.ir.expr(eid);
    let text = &source[e.origin.byte.clone()];
    match &e.kind {
        ExprKind::Literal(Text(_)) => {
            let v = text.trim();
            let v = v
                .strip_prefix('\'')
                .and_then(|x| x.strip_suffix('\''))
                .unwrap_or(v);
            json!({ "kind": "literal", "value": v })
        }
        ExprKind::Literal(Int(_)) | ExprKind::Literal(Decimal(_)) | ExprKind::Literal(Bool(_)) => {
            json!({ "kind": "literal", "value": text.trim() })
        }
        ExprKind::QualifiedEnum { enum_type, value } => {
            let en = &source[file.ir.expr(*enum_type).origin.byte.clone()];
            let en = en.trim().trim_matches('"');
            json!({ "kind": "enum", "enumName": en, "member": value })
        }
        ExprKind::Identifier(n) if e.origin.kind_text == "identifier" => {
            json!({ "kind": "constant-var", "varName": n.to_ascii_lowercase(), "initializer": { "kind": "unknown" } })
        }
        _ => json!({ "kind": "expression" }),
    }
}

/// First assignment `var := <rhs>` to `var_lc` (bare-identifier target) in DFS order;
/// returns the rhs ExprId.
fn ir_first_assignment_value(file: &AlFile, bid: BlockId, var_lc: &str) -> Option<ExprId> {
    for item in &file.ir.block(bid).items {
        match item {
            BlockItem::Stmt(s) => {
                if let Some(v) = ir_first_assignment_in_stmt(file, *s, var_lc) {
                    return Some(v);
                }
            }
            BlockItem::Preproc(g) => {
                for b in &g.branches {
                    if let Some(v) = ir_first_assignment_value(file, *b, var_lc) {
                        return Some(v);
                    }
                }
            }
        }
    }
    None
}

fn ir_first_assignment_in_stmt(
    file: &AlFile,
    sid: al_syntax::ir::StmtId,
    var_lc: &str,
) -> Option<ExprId> {
    use StmtKind::*;
    match &file.ir.stmt(sid).kind {
        Assignment {
            target,
            value,
            compound,
        } => {
            // A compound `X += v` does not DEFINE X's value -- `v` is a delta. It
            // was indistinguishable from `X := v` until the IR kept the operator
            // (#14).
            if !compound
                && matches!(&file.ir.expr(*target).kind,
                ExprKind::Identifier(x) if x.eq_ignore_ascii_case(var_lc))
            {
                return Some(*value);
            }
            None
        }
        If {
            then_block,
            else_block,
            ..
        } => ir_first_assignment_value(file, *then_block, var_lc)
            .or_else(|| else_block.and_then(|b| ir_first_assignment_value(file, b, var_lc))),
        While { body, .. }
        | Repeat { body, .. }
        | For { body, .. }
        | Foreach { body, .. }
        | With { body, .. }
        | AssertError(body)
        | Block(body) => ir_first_assignment_value(file, *body, var_lc),
        Case {
            branches,
            else_block,
            ..
        } => {
            for br in branches {
                if let Some(v) = ir_first_assignment_value(file, br.body, var_lc) {
                    return Some(v);
                }
            }
            else_block.and_then(|b| ir_first_assignment_value(file, b, var_lc))
        }
        Try { body, catch_block } => ir_first_assignment_value(file, *body, var_lc)
            .or_else(|| catch_block.and_then(|c| ir_first_assignment_value(file, c, var_lc))),
        _ => None,
    }
}

/// Per-routine `variables` (`PVariableSymbol`): parameters, local var declarations
/// (with first-assignment initializer), and object globals — first-name-wins
/// shadowing (params/locals shadow globals). Mirrors `scope::extract_variables`.
/// NOT yet IR-modelled (absent from this corpus): named return-value variable.
pub fn ir_variables(
    file: &AlFile,
    object_idx: usize,
    routine: &RoutineDecl,
    source: &str,
    source_unit_id: &str,
) -> Vec<super::features::PVariableSymbol> {
    use super::features::PVariableSymbol;
    use super::scope::canonicalize_type_text;
    let cols = Utf16Cols::new(source);
    let anchor = |origin: &Origin, kind: &str| PAnchor {
        source_unit_id: source_unit_id.to_string(),
        start_line: origin.start.row,
        start_column: cols.col(origin.start.row as usize, origin.start.column as usize),
        end_line: origin.end.row,
        end_column: cols.col(origin.end.row as usize, origin.end.column as usize),
        syntax_kind: kind.to_string(),
    };
    let mut out: Vec<PVariableSymbol> = Vec::new();

    // 1. Parameters (synthetic all-zero anchor).
    for (i, p) in routine.params.iter().enumerate() {
        out.push(PVariableSymbol {
            name: p.name.to_ascii_lowercase(),
            declared_type: canonicalize_type_text(p.ty.as_deref().unwrap_or("")),
            scope: "parameter".to_string(),
            is_parameter: true,
            parameter_index: Some(i as u32),
            initializer: None,
            source_anchor: PAnchor {
                source_unit_id: source_unit_id.to_string(),
                start_line: 0,
                start_column: 0,
                end_line: 0,
                end_column: 0,
                syntax_kind: "parameter".to_string(),
            },
        });
    }

    // 2. Locals (declared_type "unknown" when absent; initializer = first assignment).
    for v in &routine.locals {
        let lc = v.name.to_ascii_lowercase();
        if out.iter().any(|x| x.is_parameter && x.name == lc) {
            continue;
        }
        let declared_type = match v.ty.as_deref() {
            Some(t) if !t.is_empty() => canonicalize_type_text(t),
            _ => "unknown".to_string(),
        };
        let initializer = routine
            .body
            .and_then(|b| ir_first_assignment_value(file, b, &lc))
            .map(|val| ir_classify_rhs(file, val, source));
        out.push(PVariableSymbol {
            name: lc,
            declared_type,
            scope: "local".to_string(),
            is_parameter: false,
            parameter_index: None,
            initializer,
            source_anchor: anchor(&v.origin, "variable_declaration"),
        });
    }

    // 3. Object globals (first-name-wins shadowing). `ir_object_globals` has already
    //    deduped the object's own globals first-wins by lowercased name, so the only
    //    filter left here is the routine's own params/locals — which is exactly what
    //    seeding `emitted` from `out` did when this loop was inlined.
    let emitted: HashSet<String> = out.iter().map(|v| v.name.clone()).collect();
    out.extend(
        ir_object_globals(file, object_idx, &cols, source_unit_id)
            .into_iter()
            .filter(|g| !emitted.contains(&g.name)),
    );

    out
}

/// The owning object's GLOBAL variables (`PVariableSymbol`, scope=global), deduped
/// **first-wins by lowercased name** (the IR carries both `#if`/`#else` branches, so
/// one name can appear twice).
///
/// Every routine of the object sees the SAME list: [`ir_variables`] appends the subset
/// not shadowed by the routine's own params/locals. Split out of `ir_variables` so
/// there is ONE definition of "the object's globals" — the L3 emitter builds this list
/// once per object and shares it across the object's routines
/// ([`crate::program::model::workspace::RoutineVariables`]) instead of replicating a
/// copy into each. Sibling of [`ir_object_global_record_vars`].
pub fn ir_object_globals(
    file: &AlFile,
    object_idx: usize,
    cols: &Utf16Cols,
    source_unit_id: &str,
) -> Vec<super::features::PVariableSymbol> {
    use super::features::PVariableSymbol;
    use super::scope::canonicalize_type_text;
    let mut out: Vec<PVariableSymbol> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for g in &file.objects[object_idx].globals {
        let lc = g.name.to_ascii_lowercase();
        if !seen.insert(lc.clone()) {
            continue;
        }
        out.push(PVariableSymbol {
            name: lc,
            declared_type: canonicalize_type_text(g.ty.as_deref().unwrap_or("")),
            scope: "global".to_string(),
            is_parameter: false,
            parameter_index: None,
            initializer: None,
            source_anchor: PAnchor {
                source_unit_id: source_unit_id.to_string(),
                start_line: g.origin.start.row,
                start_column: cols.col(g.origin.start.row as usize, g.origin.start.column as usize),
                end_line: g.origin.end.row,
                end_column: cols.col(g.origin.end.row as usize, g.origin.end.column as usize),
                syntax_kind: "variable_declaration".to_string(),
            },
        });
    }
    out
}

// ---- routine envelope metadata (attributes / attributes_parsed) ----

/// Faithful port of `attr_arg_from_node` over an IR expr — the attribute-argument
/// JSON ({kind,text,[value],[qualifier],[member]}). attr_arg_kind maps unhandled
/// kinds (decimal/unary/call/…) to "unknown" with no value parts.
fn ir_attr_arg(file: &AlFile, eid: ExprId, source: &str) -> serde_json::Value {
    use al_syntax::ir::Literal as L;
    use serde_json::json;
    let e = file.ir.expr(eid);
    let text = source[e.origin.byte.clone()].to_string();
    let mut obj = serde_json::Map::new();
    let kind = match &e.kind {
        ExprKind::Literal(L::Bool(_)) => "boolean",
        ExprKind::Literal(L::Int(_)) => "integer",
        ExprKind::Literal(L::Text(_)) => "string_literal",
        ExprKind::Identifier(_) if e.origin.kind_text == "identifier" => "identifier",
        ExprKind::QuotedIdentifier(_) => "quoted_identifier",
        ExprKind::QualifiedEnum { .. } => "qualified_enum_value",
        ExprKind::DatabaseReference(_) => "database_reference",
        ExprKind::Member { .. } => "member_expression",
        _ => "unknown",
    };
    obj.insert("kind".into(), json!(kind));
    obj.insert("text".into(), json!(text));
    let strip = |s: &str| s.trim().trim_matches(|c| c == '"' || c == '\'').to_string();
    match kind {
        "boolean" | "integer" | "identifier" => {
            obj.insert("value".into(), json!(text));
        }
        "string_literal" | "quoted_identifier" => {
            obj.insert("value".into(), json!(strip(&text)));
        }
        "qualified_enum_value" => {
            if let ExprKind::QualifiedEnum { enum_type, value } = &e.kind {
                let q = source[file.ir.expr(*enum_type).origin.byte.clone()].to_string();
                let m = strip(value);
                obj.insert("value".into(), json!(m));
                obj.insert("qualifier".into(), json!(q));
                obj.insert("member".into(), json!(m));
            }
        }
        "database_reference" => {
            if let ExprKind::DatabaseReference(t) = &e.kind
                && let Some((kw, tn)) = t.split_once("::")
            {
                let m = strip(tn);
                obj.insert("value".into(), json!(m));
                obj.insert("qualifier".into(), json!(kw.trim()));
                obj.insert("member".into(), json!(m));
            }
        }
        _ => {}
    }
    serde_json::Value::Object(obj)
}

/// Routine `attributes` (raw text, document order) + `attributesParsed` JSON
/// ({name, args, raw}) from the IR — mirrors `collect_attributes`.
pub fn ir_attributes(
    routine: &RoutineDecl,
    file: &AlFile,
    source: &str,
) -> (Vec<String>, Vec<serde_json::Value>) {
    use serde_json::json;
    let mut raw = Vec::new();
    let mut parsed = Vec::new();
    for a in &routine.attributes_parsed {
        raw.push(a.raw.clone());
        let args: Vec<serde_json::Value> = a
            .args
            .iter()
            .map(|&arg| ir_attr_arg(file, arg, source))
            .collect();
        parsed.push(json!({ "name": a.name, "args": args, "raw": a.raw }));
    }
    (raw, parsed)
}

/// IR `ObjectKind` → the L2 object-type string, mirroring `scope::object_type_for`
/// EXACTLY (including its skips: PermissionSetExtension / Profile / Entitlement and
/// any `Other` construct return `None`, so the emitter omits them — same object set
/// the legacy tree-sitter walk produced). Validated 404/404 over the r0-corpus.
pub fn ir_object_type(kind: &al_syntax::ir::ObjectKind) -> Option<&'static str> {
    use al_syntax::ir::ObjectKind::*;
    Some(match kind {
        Codeunit => "Codeunit",
        Table => "Table",
        TableExtension => "TableExtension",
        Page => "Page",
        PageExtension => "PageExtension",
        Report => "Report",
        ReportExtension => "ReportExtension",
        Query => "Query",
        XmlPort => "XMLport",
        Enum => "Enum",
        EnumExtension => "EnumExtension",
        Interface => "Interface",
        ControlAddIn => "ControlAddIn",
        PermissionSet => "PermissionSet",
        PermissionSetExtension | Profile | Entitlement | Other => return None,
    })
}

// ---- object metadata (subtype / pageType / sourceTable / inherentCommit) ----

/// Object metadata from the IR's properties — mirrors `extract_object_metadata`.
/// Returns (object_subtype, page_type, source_table_name, inherent_commit_behavior).
pub fn ir_object_metadata(
    o: &al_syntax::ir::ObjectDecl,
    object_type: &str,
) -> (
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
) {
    let prop = |name: &str| {
        o.properties
            .iter()
            .find(|p| p.name == name)
            .map(|p| p.value.clone())
    };
    let mut object_subtype = None;
    let mut page_type = None;
    let mut source_table_name = None;
    let mut inherent_commit_behavior = None;
    if object_type == "Codeunit" {
        object_subtype = prop("subtype");
    }
    if object_type == "Page" || object_type == "PageExtension" {
        page_type = prop("pagetype");
        source_table_name = prop("sourcetable").map(|s| s.trim().trim_matches('"').to_string());
    }
    if (object_type == "Codeunit" || object_type == "Table" || object_type == "TableExtension")
        && let Some(icb_raw) = prop("inherentcommitbehavior")
    {
        let member = match icb_raw.rfind("::") {
            Some(sep) => icb_raw[sep + 2..].to_lowercase(),
            None => icb_raw.to_lowercase(),
        };
        inherent_commit_behavior = match member.as_str() {
            "ignore" => Some("ignore".to_string()),
            "error" => Some("error".to_string()),
            "allow" => Some("allow".to_string()),
            _ => None,
        };
    }
    (
        object_subtype,
        page_type,
        source_table_name,
        inherent_commit_behavior,
    )
}

/// Per-routine ParameterSymbols from the IR (index/name/type_text/is_var/is_record/
/// table_name) — mirrors scope::extract_parameters. Validated byte-exact (the
/// stable-routine-id signature hash depends on type_text + is_var).
pub fn ir_parameter_symbols(routine: &RoutineDecl) -> Vec<super::scope::ParameterSymbol> {
    routine
        .params
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let ty = p.ty.clone().unwrap_or_default();
            let is_record = is_record_type_str(&ty);
            let table_name = if is_record {
                parse_record_table_name(&ty)
            } else {
                None
            };
            super::scope::ParameterSymbol {
                index: i as u32,
                name: p.name.clone(),
                type_text: ty,
                is_var: p.by_ref,
                is_record,
                table_name,
            }
        })
        .collect()
}

/// The routine's enclosing-member discriminator: the UNESCAPED logical name of
/// the member wrapper (table field / page field / action / dataitem / …) that
/// declares this member trigger, `None` for procedures and object-level triggers
/// (`OnRun`/`OnOpenPage`).
///
/// THE single source for that string. It feeds BOTH `ModelRoutine.enclosing_member`
/// and the internal routine id's conditional 7th key part
/// ([`crate::engine::ids::CanonicalRoutineKey::enclosing_member`]) — those must be
/// the same string, and the raw IR value is only outer-quote-stripped, so the
/// unescape has to happen in exactly one place. Two call paths mint ids for the
/// same routine (`l2_workspace::build_proutine` and `l3_workspace::project_file`);
/// both go through here.
pub fn ir_enclosing_member(routine: &RoutineDecl) -> Option<String> {
    routine
        .enclosing_member
        .as_ref()
        .map(|(name, _origin)| super::scope::unescape_al_identifier(name))
}

/// Routine kind for id/classification (mirrors classify_kind): event-subscriber /
/// event-publisher from attributes, else trigger / procedure.
pub fn ir_routine_kind(routine: &RoutineDecl) -> &'static str {
    let has = |n: &str| routine.attributes.iter().any(|a| a == n);
    if has("eventsubscriber") {
        "event-subscriber"
    } else if has("integrationevent") || has("businessevent") {
        "event-publisher"
    } else if routine.kind == al_syntax::ir::RoutineKind::Trigger {
        "trigger"
    } else {
        "procedure"
    }
}

#[cfg(test)]
mod stmt_position_tests {
    use crate::program::body::l2_workspace::ir_features_for_named_routine;

    const SRC: &str = r#"
codeunit 50001 T
{
    procedure Caller()
    var
        Ok: Boolean;
    begin
        DoThing();
        if DoThing() then
            Ok := true;
        Ok := DoThing();
    end;

    procedure DoThing(): Boolean
    begin
        exit(true);
    end;
}
"#;

    #[test]
    fn statement_position_set_only_for_bare_call_statements() {
        let (features, _, _) =
            ir_features_for_named_routine(SRC, "Caller", "g", "m", "u").expect("routine");
        let sites: Vec<(&str, bool)> = features
            .call_sites
            .iter()
            .map(|cs| (cs.callee_text.as_str(), cs.in_statement_position))
            .collect();
        // Exactly one DoThing call is a bare statement; the if-condition and the
        // assignment RHS are expression-position (result consumed).
        let stmt_count = sites
            .iter()
            .filter(|(t, p)| t.contains("DoThing") && *p)
            .count();
        let expr_count = sites
            .iter()
            .filter(|(t, p)| t.contains("DoThing") && !*p)
            .count();
        assert_eq!(stmt_count, 1, "sites: {sites:?}");
        assert_eq!(expr_count, 2, "sites: {sites:?}");
    }

    /// #14: a field access says whether it reads, writes, or both. A pure write
    /// is no longer indistinguishable from a read.
    #[test]
    fn field_access_kind_read_write_readwrite() {
        use crate::program::body::features::FieldAccessKind::{Read, ReadWrite, Write};
        const SRC: &str = r#"
codeunit 50003 F
{
    procedure Caller()
    var
        R: Record Customer;
        D: Decimal;
        A: array[3] of Decimal;
    begin
        D := R.OldAmt;
        R.NewAmt := D;
        R.Cnt += 1;
        R.Total := R.Total + 1;
        A[R.Idx] := 0;
    end;
}
"#;
        let (features, _, _) =
            ir_features_for_named_routine(SRC, "Caller", "g", "m", "u").expect("routine");
        let got: Vec<(&str, _)> = features
            .field_accesses
            .iter()
            .map(|fa| (fa.field_name.as_str(), fa.access))
            .collect();
        assert_eq!(
            got,
            [
                ("OldAmt", Read),
                ("NewAmt", Write),
                ("Cnt", ReadWrite),
                ("Total", Write),
                ("Total", Read),
                ("Idx", Read), // an index INSIDE a target is still a read
            ]
        );
    }

    /// Which condition shapes `condition_references` records (#24, and the
    /// detector-substrate note in CLAUDE.md that cites this test). Parentheses are
    /// looked through; a QUOTED name -- bare or as a member -- is never recorded.
    #[test]
    fn condition_references_shapes() {
        const SRC: &str = r#"
codeunit 50002 C
{
    procedure Caller()
    var
        Rec: Record Customer;
        Plain: Boolean;
        Paren: Boolean;
        "Quoted Var": Boolean;
    begin
        if Plain then;
        if (Paren) then;
        if Rec.Name <> '' then;
        if "Quoted Var" then;
        case Rec."E-Mail" of
            '':
                ;
        end;
    end;
}
"#;
        let (features, _, _) =
            ir_features_for_named_routine(SRC, "Caller", "g", "m", "u").expect("routine");
        let got: Vec<(&str, &str)> = features
            .condition_references
            .iter()
            .map(|r| (r.identifier.as_str(), r.condition_kind.as_str()))
            .collect();
        assert_eq!(got, [("plain", "if"), ("paren", "if"), ("name", "if")]);
    }
}

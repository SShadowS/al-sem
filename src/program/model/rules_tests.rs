//! Intent-evidence step 3: the rules a model built with `ViewNeeds::rules`
//! holds (`tests/r0-corpus/ws-rules`), and that a model built without them
//! holds none and says so loudly.

use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::Arc;

use crate::engine::batch::job::{ViewKey, ViewNeeds};
use crate::engine::gate::run::{build_analysis_model, build_view_model};
use crate::program::body::rules::{PRule, Stops};
use crate::program::inputs::WorkspaceInputs;
use crate::program::model::program_calls::{
    assemble_and_resolve_cross_app_program, assemble_and_resolve_workspace_program,
};
use crate::program::model::workspace::{AssemblyNeeds, MODEL_INSTANCE_ID_DEFAULT, Model};

fn ws() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/r0-corpus/ws-rules")
}

fn inputs() -> Arc<WorkspaceInputs> {
    WorkspaceInputs::read(&ws(), &Default::default())
}

fn model(needs: AssemblyNeeds) -> Model {
    assemble_and_resolve_workspace_program(&inputs(), MODEL_INSTANCE_ID_DEFAULT, true, needs)
        .expect("ws-rules model")
}

/// One rule, every field but the id, one line each.
fn render(out: &mut String, r: &PRule) {
    let stops = match &r.stops {
        Stops::Always => "always".to_string(),
        Stops::MayNotStop { reason } => format!("may-not-stop: {reason}"),
    };
    writeln!(out, "  {:?} line {} {stops}", r.kind, r.anchor.start_line).unwrap();
    let when_text = if r.when_text.is_empty() {
        "(empty)"
    } else {
        r.when_text.as_str()
    };
    writeln!(out, "    when_text: {when_text}").unwrap();
    for a in &r.when {
        writeln!(out, "    atom: {a:?}").unwrap();
    }
    writeln!(out, "    message: {:?}", r.message).unwrap();
    writeln!(out, "    comment: {:?}", r.comment).unwrap();
}

/// Every routine with rules, in model order: `Object.Routine`, then its rules.
fn rules_text(m: &Model) -> String {
    let mut out = String::new();
    for r in &m.workspace.routines {
        if r.rules().is_empty() {
            continue;
        }
        let object = m
            .workspace
            .objects
            .iter()
            .find(|o| o.id == r.object_id)
            .map_or("?", |o| o.name.as_str());
        writeln!(out, "{object}.{}", r.name).unwrap();
        for rule in r.rules() {
            render(&mut out, rule);
        }
    }
    out
}

/// The rules of `ws-rules`, checked by hand against the AL source (lines are
/// 0-based). What each routine pins:
/// - `CheckOrder`: a label with `Comment` and two args; a `TextConst`; the
///   earlier-check `not C` rule (each later check carries the negated
///   conditions of the `Error`s before it, but not of a `TestField`); the
///   early exit `if PostingPreview then exit;` (`ParamBool` false);
///   `Error(StrSubstNo(GlobalLbl, ..))` (a global label, `Always` because the
///   `StrSubstNo` binds to the builtin); the trailing comment and the two-line
///   comment run above a check.
/// - `ConfirmDelete`: `ConfirmExit` with the question as message (the
///   `Confirm` default is not an argument), then a `ConfirmError` that
///   carries the first `Confirm`'s early exit.
/// - `CheckStatus`: a `case` arm (`FieldCompare` `=`) and the `else` arm
///   (`NoneOf` with every listed value).
/// - `CheckCustomer`: `RecordFound`; an `or` kept as one `Other`; the
///   `else` side negated; a negated `or` split by De Morgan; `case true of`.
/// - `CollectibleCheck`: an `ErrorInfo` message with its text from
///   `ErrorInfo.Create(Label)`, `MayNotStop`, so the next check does NOT get
///   `not (Order.Amount > 1000000)`.
/// - `ShadowedLabel`: a local and a global label of one name: `Dynamic`,
///   both candidates, the local first (scope order).
/// - `FailWith`: a Text parameter: `Dynamic`, no text.
/// - `CheckLimit`: an `Error` that binds to the object's own `procedure
///   Error`: `MayNotStop`, settled after resolution, and the next check does
///   not get its `not C`.
/// - `CheckVersioned`: a label declared in both `#if` arms: `Dynamic`, both
///   texts.
/// - `OnValidate`: a bare `TestField` / `FieldError` on a table's implicit
///   `Rec`, its table resolved to the table itself.
const EXPECTED: &str = r#"Rules Checks.CheckOrder
  Error line 14 always
    when_text: Order.Status <> Order.Status::Released
    atom: FieldCompare { record_var: "Order", table: Some("Rules Order"), field: "Status", op: Ne, value: "Order.Status::Released" }
    message: Some(RuleMessage { source: Label { var: "NotReleasedErr" }, text: Some("You cannot post %1 %2 because it is not released."), args: ["Order.\"Document Type\"", "Order.\"No.\""], label_comment: Some("%1 = document type, %2 = number"), candidates: [] })
    comment: Some("released only")
  Error line 16 always
    when_text: not (Order.Status <> Order.Status::Released) and Order.Amount <= 0
    atom: FieldCompare { record_var: "Order", table: Some("Rules Order"), field: "Status", op: Eq, value: "Order.Status::Released" }
    atom: FieldCompare { record_var: "Order", table: Some("Rules Order"), field: "Amount", op: Le, value: "0" }
    message: Some(RuleMessage { source: TextConst { var: "AmountTxt" }, text: Some("The amount must be positive."), args: [], label_comment: None, candidates: [] })
    comment: None
  TestField line 19 always
    when_text: not (Order.Status <> Order.Status::Released) and not (Order.Amount <= 0) and Order."Customer No." is empty
    atom: FieldCompare { record_var: "Order", table: Some("Rules Order"), field: "Status", op: Eq, value: "Order.Status::Released" }
    atom: FieldCompare { record_var: "Order", table: Some("Rules Order"), field: "Amount", op: Gt, value: "0" }
    atom: FieldEmpty { record_var: "Order", table: Some("Rules Order"), field: "Customer No.", negated: false }
    message: Some(RuleMessage { source: Platform, text: None, args: [], label_comment: None, candidates: [] })
    comment: Some("A customer is required.\nIt is checked before the blocked flag.")
  TestField line 20 always
    when_text: not (Order.Status <> Order.Status::Released) and not (Order.Amount <= 0) and Order.Blocked <> false
    atom: FieldCompare { record_var: "Order", table: Some("Rules Order"), field: "Status", op: Eq, value: "Order.Status::Released" }
    atom: FieldCompare { record_var: "Order", table: Some("Rules Order"), field: "Amount", op: Gt, value: "0" }
    atom: FieldCompare { record_var: "Order", table: Some("Rules Order"), field: "Blocked", op: Ne, value: "false" }
    message: Some(RuleMessage { source: Platform, text: None, args: [], label_comment: None, candidates: [] })
    comment: None
  Error line 24 always
    when_text: not (Order.Status <> Order.Status::Released) and not (Order.Amount <= 0) and not PostingPreview and Order."Posting Date" = 0D
    atom: FieldCompare { record_var: "Order", table: Some("Rules Order"), field: "Status", op: Eq, value: "Order.Status::Released" }
    atom: FieldCompare { record_var: "Order", table: Some("Rules Order"), field: "Amount", op: Gt, value: "0" }
    atom: ParamBool { index: 1, value: false }
    atom: FieldCompare { record_var: "Order", table: Some("Rules Order"), field: "Posting Date", op: Eq, value: "0D" }
    message: Some(RuleMessage { source: Label { var: "GlobalOnlyLbl" }, text: Some("Order %1 has no posting date."), args: ["Order.\"No.\""], label_comment: Some("%1 = order number"), candidates: [] })
    comment: None
Rules Checks.ConfirmDelete
  ConfirmExit line 33 always
    when_text: not Confirm(DeleteQst, false, Order."No.")
    atom: CallResult { callee_text: "Confirm(DeleteQst, false, Order.\"No.\")", negated: true }
    message: Some(RuleMessage { source: Label { var: "DeleteQst" }, text: Some("Delete order %1?"), args: ["Order.\"No.\""], label_comment: None, candidates: [] })
    comment: None
  ConfirmError line 36 always
    when_text: Confirm(DeleteQst, false, Order."No.") and GuiAllowed() and not Confirm('Really delete?')
    atom: CallResult { callee_text: "Confirm(DeleteQst, false, Order.\"No.\")", negated: false }
    atom: CallResult { callee_text: "GuiAllowed()", negated: false }
    atom: CallResult { callee_text: "Confirm('Really delete?')", negated: true }
    message: Some(RuleMessage { source: Label { var: "CancelledErr" }, text: Some("The deletion was cancelled."), args: [], label_comment: None, candidates: [] })
    comment: None
Rules Checks.CheckStatus
  TestField line 44 always
    when_text: Order.Status = Order.Status::Open and Order.Amount is empty
    atom: FieldCompare { record_var: "Order", table: Some("Rules Order"), field: "Status", op: Eq, value: "Order.Status::Open" }
    atom: FieldEmpty { record_var: "Order", table: Some("Rules Order"), field: "Amount", negated: false }
    message: Some(RuleMessage { source: Platform, text: None, args: [], label_comment: None, candidates: [] })
    comment: None
  Error line 48 always
    when_text: Order.Status not in [Order.Status::Open, Order.Status::Released, Order.Status::Pending]
    atom: NoneOf { scrutinee: "Order.Status", values: ["Order.Status::Open", "Order.Status::Released", "Order.Status::Pending"] }
    message: Some(RuleMessage { source: Literal, text: Some("Unknown status %1."), args: ["Order.Status"], label_comment: None, candidates: [] })
    comment: None
Rules Checks.CheckCustomer
  Error line 57 always
    when_text: not Customer.Get(CustomerNo)
    atom: RecordFound { record_var: "Customer", op: "Get", negated: true }
    message: Some(RuleMessage { source: Literal, text: Some("Customer %1 does not exist."), args: ["CustomerNo"], label_comment: None, candidates: [] })
    comment: None
  Error line 59 always
    when_text: Customer.Get(CustomerNo) and (Customer.Blocked or (Customer."Credit Limit" < Amount))
    atom: RecordFound { record_var: "Customer", op: "Get", negated: false }
    atom: Other { text: "(Customer.Blocked or (Customer.\"Credit Limit\" < Amount))", anchor: PAnchor { source_unit_id: "ws:src/checks.al", start_line: 58, start_column: 11, end_line: 58, end_column: 65, syntax_kind: "logical_expression" } }
    message: Some(RuleMessage { source: Literal, text: Some("Customer %1 cannot buy."), args: ["CustomerNo"], label_comment: None, candidates: [] })
    comment: None
  Error line 63 always
    when_text: Customer.Get(CustomerNo) and not Customer.Blocked and not (Customer."Credit Limit" < Amount) and not (Amount <= Customer."Credit Limit")
    atom: RecordFound { record_var: "Customer", op: "Get", negated: false }
    atom: Other { text: "not Customer.Blocked", anchor: PAnchor { source_unit_id: "ws:src/checks.al", start_line: 58, start_column: 11, end_line: 58, end_column: 27, syntax_kind: "member_expression" } }
    atom: FieldCompare { record_var: "Customer", table: Some("Rules Customer"), field: "Credit Limit", op: Ge, value: "Amount" }
    atom: Other { text: "not (Amount <= Customer.\"Credit Limit\")", anchor: PAnchor { source_unit_id: "ws:src/checks.al", start_line: 60, start_column: 11, end_line: 60, end_column: 44, syntax_kind: "comparison_expression" } }
    message: Some(RuleMessage { source: Literal, text: Some("Over the limit."), args: [], label_comment: None, candidates: [] })
    comment: None
  Error line 66 always
    when_text: Customer.Get(CustomerNo) and not Customer.Blocked and not (Customer."Credit Limit" < Amount) and Amount > 1000
    atom: RecordFound { record_var: "Customer", op: "Get", negated: false }
    atom: Other { text: "not Customer.Blocked", anchor: PAnchor { source_unit_id: "ws:src/checks.al", start_line: 58, start_column: 11, end_line: 58, end_column: 27, syntax_kind: "member_expression" } }
    atom: FieldCompare { record_var: "Customer", table: Some("Rules Customer"), field: "Credit Limit", op: Ge, value: "Amount" }
    atom: Other { text: "Amount > 1000", anchor: PAnchor { source_unit_id: "ws:src/checks.al", start_line: 65, start_column: 12, end_line: 65, end_column: 25, syntax_kind: "comparison_expression" } }
    message: Some(RuleMessage { source: Literal, text: Some("Too large."), args: [], label_comment: None, candidates: [] })
    comment: None
Rules Checks.CollectibleCheck
  Error line 77 may-not-stop: the message is an ErrorInfo: when the error is collectible, execution goes on
    when_text: Order.Amount > 1000000
    atom: FieldCompare { record_var: "Order", table: Some("Rules Order"), field: "Amount", op: Gt, value: "1000000" }
    message: Some(RuleMessage { source: ErrorInfo, text: Some("The amount is too large."), args: [], label_comment: None, candidates: [] })
    comment: None
  Error line 79 always
    when_text: Order.Amount < 0
    atom: FieldCompare { record_var: "Order", table: Some("Rules Order"), field: "Amount", op: Lt, value: "0" }
    message: Some(RuleMessage { source: Literal, text: Some("Negative amount."), args: [], label_comment: None, candidates: [] })
    comment: None
Rules Checks.ShadowedLabel
  Error line 86 always
    when_text: (empty)
    message: Some(RuleMessage { source: Dynamic, text: None, args: [], label_comment: None, candidates: ["Local shared text.", "Global shared text."] })
    comment: None
Rules Checks.FailWith
  Error line 91 always
    when_text: (empty)
    message: Some(RuleMessage { source: Dynamic, text: None, args: [], label_comment: None, candidates: [] })
    comment: None
Rules Own Error.CheckLimit
  Error line 5 may-not-stop: the bare Error may bind to a procedure named Error, which returns
    when_text: Order.Amount < 0
    atom: FieldCompare { record_var: "Order", table: Some("Rules Order"), field: "Amount", op: Lt, value: "0" }
    message: Some(RuleMessage { source: Literal, text: Some("Amount %1 is negative."), args: ["Order.Amount"], label_comment: None, candidates: [] })
    comment: None
  TestField line 6 always
    when_text: Order.Status is empty
    atom: FieldEmpty { record_var: "Order", table: Some("Rules Order"), field: "Status", negated: false }
    message: Some(RuleMessage { source: Platform, text: None, args: [], label_comment: None, candidates: [] })
    comment: None
Rules Directives.CheckVersioned
  Error line 29 always
    when_text: Order.Amount = 0
    atom: FieldEmpty { record_var: "Order", table: Some("Rules Order"), field: "Amount", negated: false }
    message: Some(RuleMessage { source: Dynamic, text: None, args: [], label_comment: None, candidates: ["The new text.", "The old text."] })
    comment: None
Rules Order.OnValidate
  TestField line 18 always
    when_text: Status <> Status::Open
    atom: FieldCompare { record_var: "Rec", table: Some("Rules Order"), field: "Status", op: Ne, value: "Status::Open" }
    message: Some(RuleMessage { source: Platform, text: None, args: [], label_comment: None, candidates: [] })
    comment: None
  FieldError line 20 always
    when_text: Amount < 0
    atom: FieldCompare { record_var: "Rec", table: Some("Rules Order"), field: "Amount", op: Lt, value: "0" }
    message: Some(RuleMessage { source: Label { var: "NegativeErr" }, text: Some("must not be negative"), args: [], label_comment: None, candidates: [] })
    comment: Some("amounts are never negative")
"#;

fn assert_text(got: &str, what: &str) {
    if got != EXPECTED {
        panic!("{what}: the ws-rules rules differ. Got:\n{got}");
    }
}

/// The single-app model built with rules holds exactly these rules.
#[test]
fn ws_rules_single_app_rules_are_exact() {
    assert_text(&rules_text(&model(AssemblyNeeds::RULES)), "single-app");
}

/// The cross-app path threads the need to the same pass (ws-rules has no
/// dependency, so the rules are the same).
#[test]
fn ws_rules_cross_app_rules_are_exact() {
    let x = assemble_and_resolve_cross_app_program(
        &inputs(),
        MODEL_INSTANCE_ID_DEFAULT,
        true,
        AssemblyNeeds::RULES,
    )
    .expect("cross-app model");
    assert_text(&rules_text(&x.resolved), "cross-app");
}

/// The one view builder hands a view's `rules` need to the assembly, in both
/// universes: pins the USE in `build_view_model`, not just the pass.
#[test]
fn a_view_that_declares_rules_builds_them() {
    let needs = ViewNeeds::MODEL.union(ViewNeeds {
        rules: true,
        ..ViewNeeds::MODEL
    });
    assert!(needs.rules, "union keeps the rules need");
    for single_app in [true, false] {
        let i = inputs();
        let key = ViewKey::new(&i.workspace, single_app, false);
        let view = build_view_model(&i, &key, needs, |_| {})
            .unwrap_or_else(|_| panic!("view (single_app {single_app})"))
            .expect("a model view");
        assert_text(
            &rules_text(view.target.resolved()),
            &format!("view, single_app {single_app}"),
        );
    }
}

/// A model built without the need has no rules on any row, and reading them
/// fails loudly instead of reading as "none".
#[test]
#[should_panic(expected = "rules not built: declare ViewNeeds::rules")]
fn reading_rules_not_built_panics() {
    let m = model(AssemblyNeeds::BASE);
    assert!(
        m.workspace
            .routines
            .iter()
            .all(|r| r.rules.is_none() && r.rule_drafts.is_none()),
        "no rules without the need"
    );
    let r = m
        .workspace
        .routines
        .iter()
        .find(|r| r.name == "CheckOrder")
        .expect("CheckOrder");
    let _ = r.rules();
}

/// A default `analyze` view (`ViewNeeds::ANALYZE`) builds no rules, in either
/// universe: a default run pays nothing for them.
#[test]
fn a_default_analyze_view_builds_no_rules() {
    let (analyze, model) = (ViewNeeds::ANALYZE, ViewNeeds::MODEL);
    assert!(!analyze.rules && !model.rules);
    for single_app in [true, false] {
        let a = build_analysis_model(&inputs(), single_app);
        let m = a.model.unwrap_or_else(|_| panic!("analyze model"));
        let rows = &m.resolved().workspace.routines;
        assert!(rows.iter().any(|r| r.name == "CheckOrder"), "precondition");
        assert!(
            rows.iter().all(|r| r.rules.is_none()),
            "single_app {single_app}: a default analyze built rules"
        );
    }
}

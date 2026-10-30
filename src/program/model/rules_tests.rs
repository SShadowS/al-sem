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

/// A default `analyze` view (`ViewNeeds::analyze_for` the default detectors)
/// builds no rules, in either universe: a default run pays nothing for them.
#[test]
fn a_default_analyze_view_builds_no_rules() {
    let (analyze, model) = (ViewNeeds::ANALYZE, ViewNeeds::MODEL);
    assert!(!analyze.rules && !model.rules);
    let defaults = crate::engine::l5::detectors::default_detectors();
    assert!(
        !ViewNeeds::analyze_for(&defaults).rules,
        "no default detector reads the rules"
    );
    for single_app in [true, false] {
        let a = build_analysis_model(&inputs(), single_app, &defaults);
        let m = a.model.unwrap_or_else(|_| panic!("analyze model"));
        let rows = &m.resolved().workspace.routines;
        assert!(rows.iter().any(|r| r.name == "CheckOrder"), "precondition");
        assert!(
            rows.iter().all(|r| r.rules.is_none()),
            "single_app {single_app}: a default analyze built rules"
        );
        assert!(
            rows.iter().all(|r| r.card_text.is_none()),
            "single_app {single_app}: a default analyze built card text"
        );
        assert!(
            m.resolved()
                .workspace
                .objects
                .iter()
                .all(|o| o.run_actions.is_none()),
            "single_app {single_app}: a default analyze built the RunObject actions"
        );
    }
}

/// An `analyze` that selects d65 (`substrate::RULES`) builds the rules, and
/// only them (no card text), in either universe (intent-evidence step 6).
#[test]
fn an_analyze_view_running_d65_builds_rules() {
    let d65: Vec<_> = crate::engine::l5::detectors::registered_detectors()
        .into_iter()
        .filter(|d| d.name == "d65-guard-bypass")
        .collect();
    assert_eq!(d65.len(), 1, "precondition: d65 registered");
    let needs = ViewNeeds::analyze_for(&d65);
    assert!(needs.rules && !needs.card_text && needs.analyze_extras);
    for single_app in [true, false] {
        let a = build_analysis_model(&inputs(), single_app, &d65);
        let m = a.model.unwrap_or_else(|_| panic!("analyze model"));
        assert_text(
            &rules_text(m.resolved()),
            &format!("analyze with d65, single_app {single_app}"),
        );
        assert!(
            m.resolved()
                .workspace
                .routines
                .iter()
                .all(|r| r.card_text.is_none()),
            "single_app {single_app}: d65 needs no card text"
        );
    }
}

/// `explain`'s view (`ViewNeeds::EXPLAIN`) builds the card text in both
/// universes, through the one view builder: the doc above `CheckOrder`, and
/// a routine with no doc reads `None` inside a BUILT text.
#[test]
fn an_explain_view_builds_card_text() {
    for single_app in [true, false] {
        let i = inputs();
        let key = ViewKey::new(&i.workspace, single_app, false);
        let view = build_view_model(&i, &key, ViewNeeds::EXPLAIN, |_| {})
            .unwrap_or_else(|_| panic!("view (single_app {single_app})"))
            .expect("a model view");
        let rows = &view.target.resolved().workspace.routines;
        let text = |name: &str| {
            rows.iter()
                .find(|r| r.name == name)
                .unwrap_or_else(|| panic!("{name}"))
                .card_text()
                .clone()
        };
        assert_eq!(
            text("CheckOrder").doc.as_deref(),
            Some("Checks an order before it is posted."),
            "single_app {single_app}"
        );
        assert_eq!(text("ConfirmDelete").doc, None, "single_app {single_app}");
        assert!(rows.iter().all(|r| r.card_text.is_some()));
        // The RunObject actions are built with the card text, for every
        // object (an object with none reads `Some(vec![])`).
        assert!(
            view.target
                .resolved()
                .workspace
                .objects
                .iter()
                .all(|o| o.run_actions.is_some())
        );
    }
}

/// A model built without the card-text need fails loudly on a read of an
/// object's RunObject actions.
#[test]
#[should_panic(expected = "run actions not built: declare ViewNeeds::card_text")]
fn reading_run_actions_not_built_panics() {
    let m = model(AssemblyNeeds::RULES);
    let _ = m.workspace.objects[0].run_actions();
}

/// A model built without the card-text need fails loudly on a read.
#[test]
#[should_panic(expected = "card text not built: declare ViewNeeds::card_text")]
fn reading_card_text_not_built_panics() {
    let m = model(AssemblyNeeds::RULES);
    let r = m
        .workspace
        .routines
        .iter()
        .find(|r| r.name == "CheckOrder")
        .expect("CheckOrder");
    assert!(r.rules.is_some(), "precondition: rules are built");
    let _ = r.card_text();
}

/// Step 6: `when_earlier` marks the `not C` an earlier always-stopping check
/// adds, and not the one an early `exit` adds (the exit returns to the caller,
/// so its `not C` is a real precondition; d65's key keeps it). On the path:
/// a `TestField` directly in a `then` is not passed in the `else`, an `Error`
/// is.
#[test]
fn when_earlier_and_on_path_shapes() {
    let src = "table 50995 \"WE T\" { fields { field(1; \"No.\"; Code[20]) { } \
               field(2; Status; Option) { OptionMembers = Open,Released; } \
               field(3; Amount; Decimal) { } field(4; Blocked; Boolean) { } } }\n\
               codeunit 50996 \"WE C\" {\n\
               procedure AfterCheck(var T: Record \"WE T\")\nbegin\n\
               if T.Amount < 0 then Error('neg');\nT.TestField(Status);\nT.Amount := 1;\nend;\n\
               procedure AfterExit(var T: Record \"WE T\")\nbegin\n\
               if T.Amount < 0 then exit;\nT.TestField(Status);\nT.Amount := 1;\nend;\n\
               procedure ElseShapes(var T: Record \"WE T\"; Q: Boolean)\nbegin\n\
               if Q then T.TestField(Status) else T.Amount := 1;\n\
               if T.Blocked then Error('blocked') else T.Amount := 2;\nend;\n}\n";
    let m = crate::program::model::program_calls::assemble_and_resolve_inline_program_with(
        &[("src/we.al".to_string(), src.to_string())],
        "99999999-0000-0000-0000-0000000ee652",
        MODEL_INSTANCE_ID_DEFAULT,
        AssemblyNeeds::RULES,
    );
    let routine = |name: &str| {
        m.workspace
            .routines
            .iter()
            .find(|r| r.name == name)
            .unwrap_or_else(|| panic!("{name}"))
    };
    let testfield = |name: &str| {
        routine(name)
            .rules()
            .iter()
            .find(|r| r.kind == crate::program::body::rules::RuleKind::TestField)
            .cloned()
            .unwrap_or_else(|| panic!("{name}: a TestField rule"))
    };
    let after_check = testfield("AfterCheck");
    assert_eq!(after_check.when.len(), 2, "{after_check:?}");
    assert_eq!(
        after_check.when_earlier,
        vec![true, false],
        "{after_check:?}"
    );
    let after_exit = testfield("AfterExit");
    assert_eq!(after_exit.when.len(), 2, "{after_exit:?}");
    assert_eq!(
        after_exit.when_earlier,
        vec![false, false],
        "{after_exit:?}"
    );
    // ElseShapes: the writes in the two `else` branches.
    let r = routine("ElseShapes");
    let writes: Vec<_> = r
        .field_accesses
        .iter()
        .filter(|fa| fa.field_name.eq_ignore_ascii_case("Amount"))
        .map(|fa| fa.source_anchor.clone())
        .collect();
    assert_eq!(writes.len(), 2, "precondition: two writes");
    let tf = testfield("ElseShapes");
    assert!(
        !tf.guards(&writes[0]),
        "a TestField is not evaluated in the else"
    );
    let err = r
        .rules()
        .iter()
        .find(|x| x.kind == crate::program::body::rules::RuleKind::Error)
        .expect("the Error rule");
    assert!(
        err.guards(&writes[1]),
        "an Error's condition is the branch's"
    );
}

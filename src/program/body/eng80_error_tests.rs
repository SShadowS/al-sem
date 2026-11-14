//! ENG-80 (#310 remainder part 2; spec `docs/superpowers/specs/2026-11-14-eng14-310-remainder-design.md`
//! §4.1, acceptance B1-B3, B5, B6, C1, C4): a bare `Error(...)` ends the path only when it raises on
//! every run, in every build: it binds the builtin (the resolver's verdict, rule 3), no callee or
//! argument varies by `#if` build (rule 1), and its argument selects the Text overload (rule 2). Any
//! other bare `Error` is a call that may raise (the code after it is live, no d20); one bound to a
//! user `procedure Error` is a plain call.
//!
//! Every case goes through production assembly, so the resolver's verdicts are real.

use crate::program::body::features::{PCFNNode, PCallSite};
use crate::program::model::program_calls::{
    assemble_and_resolve_inline_program_default, assemble_and_resolve_inline_program_with,
};
use crate::program::model::workspace::{AssemblyNeeds, Model, ModelRoutine};

const APP: &str = "e8000000-0000-4000-8000-000000000081";

const SOURCE: &str = r#"table 50380 "Eng80 T"
{
    fields { field(1; "No."; Code[20]) { } }

    procedure StrSubstNo(Msg: Text): Text
    begin
        exit(Msg);
    end;
}

codeunit 50380 "Eng80 Error"
{
    var
#if X
        GE: ErrorInfo;
#else
        GE: Text;
#endif
        GT: Text;
#if X
        GF: Text;
#else
        GF: ErrorInfo;
#endif

    // B1: the Text forms terminate.
    procedure TextLiteral() begin Error('x'); Next(); end;
    procedure TextParen() begin Error(('x')); Next(); end;
    procedure TextVar() var T: Text; begin Error(T); Next(); end;
    procedure TextLabel() var L: Label 'x'; begin Error(L); Next(); end;
    procedure TextCode() var C: Code[20]; begin Error(C); Next(); end;
    procedure TextConstVar() var TC: TextConst ENU = 'x'; begin Error(TC); Next(); end;
    procedure TextCodeParam(P: Code[20]) begin Error(P); Next(); end;
    procedure TextTwoArgs() var L: Label 'x %1'; begin Error(L, 1); Next(); end;
    procedure TextStrSubstNo() var L: Label 'x %1'; begin Error(StrSubstNo(L, 1)); Next(); end;
    procedure TextFormat() begin Error(Format(1)); Next(); end;
    procedure TextConcat() var T: Text; begin Error('a' + T); Next(); end;
    procedure TextQuoted() begin "Error"('x'); Next(); end;
    procedure TextGlobalPlain() begin Error(GT); Next(); end;

    // B2: ErrorInfo and unprovable forms do not terminate.
    procedure InfoVar() var EI: ErrorInfo; begin Error(EI); Next(); end;
    procedure InfoParen() var EI: ErrorInfo; begin Error((EI)); Next(); end;
    procedure InfoCall() begin Error(GetErrInfo()); Next(); end;
    procedure InfoParenless() begin Error(GetErrInfo); Next(); end;
    procedure InfoIndex() var Errs: array[2] of ErrorInfo; I: Integer; begin Error(Errs[I]); Next(); end;
    procedure InfoNamedReturn() EI: ErrorInfo begin Error(EI); Next(); end;
    procedure InfoField() var R: Record "Eng80 T"; begin Error(R."No."); Next(); end;

    // B2: an argument or callee that varies by build (taint).
    procedure P13()
    var
        ErrInfo: ErrorInfo;
    begin
        Error(
#if X
            ErrInfo
#else
            'txt'
#endif
        );
        Next();
    end;

    procedure S1Operand()
    var
        ErrInfo: ErrorInfo;
    begin
        Error((
#if X
            ErrInfo
#else
            'txt'
#endif
        ));
        Next();
    end;

    procedure S1OperandRev()
    var
        ErrInfo: ErrorInfo;
    begin
        Error((
#if X
            'txt'
#else
            ErrInfo
#endif
        ));
        Next();
    end;

    procedure S2Callee()
    begin
#if X
        Error
#else
        Work
#endif
        ('x');
        Next();
    end;

    procedure I1SplitCall(V: Integer)
    var
        Lbl: Label 'x %1';
    begin
#if X
        Error(Lbl,
#else
        Message(Lbl,
#endif
            V);
        Next();
    end;

    // B3: declarations that prove nothing.
    procedure GlobalInfoFirst() begin Error(GE); Next(); end;
    procedure GlobalTextFirst() begin Error(GF); Next(); end;

    procedure P15()
    var
#if X
        E: ErrorInfo;
#else
        E: Text;
#endif
    begin
        Error(E);
        Next();
    end;

    procedure SplitArmLocal()
#if X
    var
        E: ErrorInfo;
    begin
#else
    begin
#endif
        Error(GT);
        Next();
    end;

    procedure Undeclared() begin Error(Nope); Next(); end;

    local procedure GetErrInfo(): ErrorInfo var EI: ErrorInfo; begin exit(EI); end;
    local procedure Work(M: Text) begin end;
    local procedure Next() begin end;
}

// B1 control for B5: a `StrSubstNo` declared on the base table binds before the builtin.
tableextension 50381 "Eng80 TabExt" extends "Eng80 T"
{
    procedure ExtStrSubstNo()
    begin
        Error(StrSubstNo('x'));
        Next();
    end;

    local procedure Next() begin end;
}

// C1 / B6: an own `procedure Error` runs and returns.
codeunit 50382 "Eng80 Own Error"
{
    procedure OwnText() begin Error('x'); Next(); end;
    procedure OwnFormat() begin Error('x %1', 1); Next(); end;

    local procedure Error(Msg: Text) begin end;
    local procedure Error(Msg: Text; V: Integer) begin end;
    local procedure Next() begin end;
}

codeunit 50383 "Eng80 Own Bool Error"
{
    procedure IfError() begin if Error('x') then Work(); Next(); end;
    procedure AssignError() var B: Boolean; begin B := Error('x'); Work(); end;

    local procedure Error(Msg: Text): Boolean begin end;
    local procedure Work() begin end;
    local procedure Next() begin end;
}

// C1: a table-scope `procedure Error`, from a table extension (User) and from a page on the
// table (the resolver's builtin-precedence collision: Unknown).
table 50384 "Eng80 Err Table"
{
    fields { field(1; "No."; Code[20]) { } }

    procedure Error(Msg: Text) begin end;
}

tableextension 50385 "Eng80 Err TabExt" extends "Eng80 Err Table"
{
    procedure ExtCaller() begin Error('x'); Next(); end;

    local procedure Next() begin end;
}

page 50386 "Eng80 Err Page"
{
    SourceTable = "Eng80 Err Table";

    trigger OnOpenPage()
    begin
        Error('x');
        Next();
    end;

    local procedure Next() begin end;
}
"#;

fn model() -> Model {
    assemble_and_resolve_inline_program_default(
        &[("Eng80.al".to_string(), SOURCE.to_string())],
        APP,
    )
}

fn routine<'a>(m: &'a Model, name: &str) -> &'a ModelRoutine {
    let rows: Vec<_> = m
        .workspace
        .routines
        .iter()
        .filter(|r| r.name == name)
        .collect();
    assert_eq!(rows.len(), 1, "{name}: one row");
    rows[0]
}

fn site<'a>(m: &'a Model, name: &str, callee: &str) -> &'a PCallSite {
    let found: Vec<_> = routine(m, name)
        .call_sites
        .iter()
        .filter(|c| c.callee_text == callee)
        .collect();
    assert_eq!(found.len(), 1, "{name}: one call to {callee}");
    found[0]
}

/// Whether the call to `next` in `name` is dead (marked `unreachable`, with a d20 entry).
/// Fails when the mark and d20 disagree.
fn next_dead(m: &Model, name: &str, next: &str) -> bool {
    let r = routine(m, name);
    assert!(
        r.statement_tree.is_some(),
        "{name}: precondition: a trusted routine"
    );
    let cs = site(m, name, next);
    let marked = cs.control_context.as_deref() == Some("unreachable");
    let d20 = r
        .unreachable_statements
        .iter()
        .any(|u| u.unreachable_anchor.start_line == cs.source_anchor.start_line);
    assert_eq!(marked, d20, "{name}: the mark and d20 agree at {next}");
    marked
}

/// Whether `name`'s tree holds the synthetic may-raise leaf (an `error` node with no site).
fn has_may_raise(m: &Model, name: &str) -> bool {
    fn walk(n: &PCFNNode) -> bool {
        (n.kind == "error" && n.callsite_id.is_none())
            || [&n.children, &n.else_children, &n.condition_leaves]
                .into_iter()
                .flatten()
                .flatten()
                .any(walk)
    }
    routine(m, name).statement_tree.as_deref().is_some_and(walk)
}

fn all_paths_error(r: &ModelRoutine) -> bool {
    crate::engine::return_summary::compute_routine_return_summary(r).all_paths_error
        == serde_json::Value::Bool(true)
}

/// Every name in `names` whose `Next` call is not `want_dead`, checked together so one break
/// shows every row it moves.
fn mismatches(m: &Model, names: &[&str], want_dead: bool) -> Vec<String> {
    names
        .iter()
        .filter(|n| next_dead(m, n, "Next") != want_dead)
        .map(|n| format!("{n}: Next dead = {}", !want_dead))
        .collect()
}

#[test]
fn eng80_b1_text_forms_terminate() {
    let m = model();
    let names = [
        "TextLiteral",
        "TextParen",
        "TextVar",
        "TextLabel",
        "TextCode",
        "TextConstVar",
        "TextCodeParam",
        "TextTwoArgs",
        "TextStrSubstNo",
        "TextFormat",
        "TextConcat",
        "TextQuoted",
        "TextGlobalPlain",
    ];
    let bad = mismatches(&m, &names, true);
    assert!(bad.is_empty(), "\n{}", bad.join("\n"));
    for n in names {
        assert!(!has_may_raise(&m, n), "{n}: no may-raise leaf");
        assert!(all_paths_error(routine(&m, n)), "{n}: all paths error");
    }
}

#[test]
fn eng80_b2_unproven_forms_may_raise() {
    let m = model();
    let names = [
        "InfoVar",
        "InfoParen",
        "InfoCall",
        "InfoParenless",
        "InfoIndex",
        "InfoNamedReturn",
        "InfoField",
        "P13",
        "S1Operand",
        "S1OperandRev",
        "S2Callee",
        "I1SplitCall",
    ];
    let bad = mismatches(&m, &names, false);
    assert!(bad.is_empty(), "\n{}", bad.join("\n"));
    for n in names {
        assert!(has_may_raise(&m, n), "{n}: the may-raise leaf is kept");
        assert!(!all_paths_error(routine(&m, n)), "{n}: not all paths error");
    }
}

#[test]
fn eng80_b3_declarations_that_prove_nothing() {
    let m = model();
    let names = [
        "GlobalInfoFirst",
        "GlobalTextFirst",
        "P15",
        "SplitArmLocal",
        "Undeclared",
    ];
    let bad = mismatches(&m, &names, false);
    assert!(bad.is_empty(), "\n{}", bad.join("\n"));
}

/// B5: `Error(StrSubstNo(..))` terminates only when `StrSubstNo` binds the builtin; a base
/// table's own `StrSubstNo` binds first from its extension.
#[test]
fn eng80_b5_inner_call_binds_the_builtin() {
    let m = model();
    assert!(
        next_dead(&m, "TextStrSubstNo", "Next"),
        "control: the builtin"
    );
    assert!(
        !next_dead(&m, "ExtStrSubstNo", "Next"),
        "the base table's StrSubstNo"
    );
}

/// B6: a user `procedure Error(...): Boolean` in expression position is a plain call: the code
/// it guards is not pruned and no `error` leaf is built.
#[test]
fn eng80_b6_user_error_in_expression_position() {
    let m = model();
    for (name, after) in [("IfError", "Work"), ("AssignError", "Work")] {
        assert!(!next_dead(&m, name, after), "{name}: {after} live");
        assert!(!has_may_raise(&m, name), "{name}: no may-raise leaf");
        let mut errors = 0;
        fn count(n: &PCFNNode, k: &mut usize) {
            *k += usize::from(n.kind == "error");
            for c in [&n.children, &n.else_children, &n.condition_leaves]
                .into_iter()
                .flatten()
                .flatten()
            {
                count(c, k);
            }
        }
        count(
            routine(&m, name).statement_tree.as_deref().unwrap(),
            &mut errors,
        );
        assert_eq!(errors, 0, "{name}: no error leaf");
    }
}

/// C1: a bare `Error` bound to a user procedure (own, or the base table's from its extension)
/// is a plain call; one the resolver cannot settle (a page on the table: the builtin-precedence
/// collision) may raise.
#[test]
fn eng80_c1_user_and_unsettled_bindings() {
    let m = model();
    for name in ["OwnText", "OwnFormat", "ExtCaller"] {
        assert!(!next_dead(&m, name, "Next"), "{name}: Next live");
        assert!(
            !has_may_raise(&m, name),
            "{name}: User binding: a plain call"
        );
        assert!(!all_paths_error(routine(&m, name)), "{name}");
    }
    assert!(
        !next_dead(&m, "OnOpenPage", "Next"),
        "OnOpenPage: Next live"
    );
    assert!(
        has_may_raise(&m, "OnOpenPage"),
        "OnOpenPage: an unsettled binding may raise"
    );
    assert!(!all_paths_error(routine(&m, "OnOpenPage")));
}

/// C4: a Label global shadowed by a same-named local in a split-body arm (absent from the
/// routine's locals): the rule does not use the global's message. Control: no split, the
/// global's message.
#[test]
fn eng80_c4_rules_respect_incomplete_locals() {
    let src = r#"codeunit 50387 "Eng80 Rules"
{
    var
        Msg: Label 'global';

    procedure Split()
#if X
    var
        Msg: Label 'local';
    begin
#else
    begin
#endif
        Error(Msg);
    end;

    procedure Plain()
    begin
        Error(Msg);
    end;
}
"#;
    let m = assemble_and_resolve_inline_program_with(
        &[("Rules.al".to_string(), src.to_string())],
        "e8000000-0000-4000-8000-000000000082",
        AssemblyNeeds::RULES,
    );
    let text = |name: &str| {
        let rules = routine(&m, name).rules();
        assert_eq!(rules.len(), 1, "{name}: one rule");
        rules[0].message.as_ref().and_then(|msg| msg.text.clone())
    };
    assert_eq!(text("Plain").as_deref(), Some("global"), "control");
    assert_ne!(text("Split").as_deref(), Some("global"), "Split");
}

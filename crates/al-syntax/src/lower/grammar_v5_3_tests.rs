//! GRM-04: what grammar v5.3.0 (tree-sitter-al `9d155ee7`, issue #44) lets the
//! lowerer read, and the policy it reads each new shape with. Every shape here was
//! an ERROR on v5.2.0 (except the empty-group corrections in
//! `an_empty_group_before_a_body_no_longer_takes_the_body`), so each test also
//! states which arm or arms a consumer gets.
//!
//! DISCRIMINATIONS (2026-11-16), each red then green restored: see each test.

use crate::ir::{AlFile, ExprId, ExprKind, Literal, StmtKind};
use crate::parse;

/// A call's callee as text: `Foo` or `Rec.Member`'s member.
fn callee(af: &AlFile, e: ExprId) -> String {
    let ExprKind::Call { function, .. } = &af.ir.expr(e).kind else {
        return String::new();
    };
    match &af.ir.expr(*function).kind {
        ExprKind::Identifier(n) | ExprKind::QuotedIdentifier(n) => n.clone(),
        ExprKind::Member { member, .. } => member.clone(),
        _ => "?".to_string(),
    }
}

/// The calls in statement position, as (callee, its arguments).
fn statement_calls(af: &AlFile) -> Vec<(String, Vec<ExprId>)> {
    af.ir
        .iter_stmts()
        .filter_map(|s| match &s.kind {
            StmtKind::Call(e) => match &af.ir.expr(*e).kind {
                ExprKind::Call { args, .. } => Some((callee(af, *e), args.clone())),
                _ => None,
            },
            _ => None,
        })
        .collect()
}

/// `)` in every `#if` arm (`preproc_split_paren_close`): the call's last arguments
/// are the FIRST arm a build can take, so the arity is a real build's: `Foo(1, 2,
/// true)` (3, not the union's 3 + the `#else` arm's 0, and not one `Unknown`), and
/// `Validate`'s second argument is the first arm's `CalcBomQuantity(1)`. Both calls
/// are tainted (ENG-80), and a `SyntaxIssue` says the other arms are not modelled.
///
/// DISCRIMINATION: the `PreprocSplitParenClose` arm removed from `lower_arguments`
/// (the group lowers as one argument) -> `Validate`'s second argument is no call and
/// `Foo` has 3 arguments the last of which is not `true`.
#[test]
fn a_paren_split_reads_the_first_arm_as_the_last_arguments() {
    let af = parse(
        "codeunit 50100 G\n{\n    procedure Go(var Rec: Record \"G T\")\n    begin\n        Rec.Validate(\"Quantity per\",\n#if V19\n            Rec.CalcBomQuantity(1));\n#else\n            Rec.CalcQuantityFromBom(1));\n#endif\n        Foo(1, 2\n#if V21\n            , true);\n#else\n            );\n#endif\n    end;\n}\n",
    );
    let calls = statement_calls(&af);
    let validate = calls
        .iter()
        .find(|(c, _)| c == "Validate")
        .expect("the Validate call");
    assert_eq!(validate.1.len(), 2, "Validate's arity");
    assert_eq!(
        callee(&af, validate.1[1]),
        "CalcBomQuantity",
        "the first arm"
    );
    let foo = calls
        .iter()
        .find(|(c, _)| c == "Foo")
        .expect("the Foo call");
    assert_eq!(foo.1.len(), 3, "Foo's arity is the first arm's build");
    assert!(
        matches!(
            af.ir.expr(foo.1[2]).kind,
            ExprKind::Literal(Literal::Bool(true))
        ),
        "the first arm's `true`"
    );
    let tainted = af
        .ir
        .iter_stmts()
        .filter(|s| matches!(s.kind, StmtKind::Call(e) if af.ir.call_is_tainted(e)))
        .count();
    assert_eq!(tainted, 2, "both calls vary by build");
    assert_eq!(
        af.issues
            .iter()
            .filter(|i| i.message.contains("every `#if` arm of an argument list"))
            .count(),
        2
    );
}

/// `#if` around a procedure's access modifier (`preproc_conditional_modifier`): the
/// first arm the routine's build does not rule out gives the modifier, as for a
/// return clause; without it the procedure read as public in every build.
///
/// DISCRIMINATION: `access_modifier` reading `arm_field(Modifier)` only -> both are
/// `None`.
#[test]
fn a_modifier_group_gives_the_first_arms_modifier() {
    let af = parse(
        "codeunit 50101 G\n{\n#if V16\n    internal\n#endif\n    procedure A()\n    begin\n    end;\n\n#if V17\n    local\n#else\n    internal\n#endif\n    procedure B()\n    begin\n    end;\n}\n",
    );
    let modifier = |name: &str| {
        af.objects[0]
            .routines
            .iter()
            .find(|r| r.name == name)
            .and_then(|r| r.access_modifier.clone())
    };
    assert_eq!(modifier("A").as_deref(), Some("internal"));
    assert_eq!(modifier("B").as_deref(), Some("local"));
}

/// A part/usercontrol header chosen per `#if` arm (`preproc_split_part`): every
/// arm's control is collected, each with its own kind and target.
///
/// DISCRIMINATION: the `PreprocSplitPart` block removed from `collect_page_controls`
/// -> no control at all.
#[test]
fn a_split_part_collects_every_arms_control() {
    let af = parse(
        "page 50102 G\n{\n    layout\n    {\n        area(Content)\n        {\n#if V24\n            part(Lines; \"G Lines A\")\n#else\n            part(Lines; \"G Lines B\")\n#endif\n            {\n                ApplicationArea = All;\n            }\n#if V24\n            usercontrol(Logo; \"G Addin A\")\n#else\n            usercontrol(Logo; \"G Addin B\")\n#endif\n            {\n            }\n        }\n    }\n}\n",
    );
    let controls: Vec<(String, String, String)> = af.objects[0]
        .page_controls
        .iter()
        .map(|c| (c.name.clone(), c.kind.clone(), c.target.clone()))
        .collect();
    let s = |a: &str, b: &str, c: &str| (a.to_string(), b.to_string(), c.to_string());
    assert_eq!(
        controls,
        vec![
            s("Lines", "part", "G Lines A"),
            s("Lines", "part", "G Lines B"),
            s("Logo", "usercontrol", "G Addin A"),
            s("Logo", "usercontrol", "G Addin B"),
        ]
    );
}

/// A table field opened in several `#if` arms (`preproc_split_table_field_open` with
/// `#else`, or a nested group as its first arm): the FIRST arm's id, name and type,
/// every arm's properties (a nested group's arms' own included), and a `SyntaxIssue`
/// where the arms differ. The one-arm form (Microsoft's removed-field blocks) reads
/// as before, with no issue. The nested field's header is the nested group's first
/// arm because `.field()` follows inherited field entries (measured; a cursor walk
/// of the outer node sees only the `#else` arm's `id`, 12).
///
/// DISCRIMINATIONS: `field_bodies` not descending a nested group -> the nested
/// field's captions miss 'NB' and 'NN'; the issue's arm test reduced to `true` -> 3
/// issues, not 2.
#[test]
fn a_field_opened_in_several_arms_reads_the_first_arms_header() {
    let af = parse(
        "table 50103 G\n{\n    fields\n    {\n#if CLEAN25\n        field(10; \"Qty\"; Decimal)\n        {\n            Caption = 'Dec';\n#else\n        field(10; \"Qty\"; Integer)\n        {\n            Caption = 'Int';\n#endif\n            ToolTip = 'Qty';\n        }\n#if A\n#if B\n        field(11; \"Nested B\"; Integer)\n        {\n            Caption = 'NB';\n#else\n        field(11; \"Nested NotB\"; Integer)\n        {\n            Caption = 'NN';\n#endif\n#else\n        field(12; \"Other\"; Code[10])\n        {\n            Caption = 'O';\n#endif\n            ToolTip = 'N';\n        }\n#if not CLEAN26\n        field(20; \"Removed\"; Integer)\n        {\n#endif\n            Caption = 'R';\n        }\n    }\n}\n",
    );
    let fields: Vec<(i64, String, String)> = af.objects[0]
        .fields
        .iter()
        .map(|f| (f.number, f.name.clone(), f.data_type.clone()))
        .collect();
    let f = |n: i64, a: &str, b: &str| (n, a.to_string(), b.to_string());
    assert_eq!(
        fields,
        vec![
            f(10, "Qty", "Decimal"),
            f(11, "Nested B", "Integer"),
            f(20, "Removed", "Integer")
        ]
    );
    let qty = &af.objects[0].fields[0];
    assert_eq!(
        qty.caption,
        vec!["Dec".to_string(), "Int".to_string()],
        "every arm's"
    );
    assert_eq!(qty.tooltip, vec!["Qty".to_string()], "the shared body's");
    let mut nested = af.objects[0].fields[1].caption.clone();
    nested.sort();
    assert_eq!(
        nested,
        vec!["NB".to_string(), "NN".to_string(), "O".to_string()],
        "the nested group's arms' own bodies, and the `#else` arm's"
    );
    assert_eq!(
        af.issues
            .iter()
            .filter(|i| i.message.contains("opened in several `#if` arms"))
            .count(),
        2,
        "the two fields whose arms differ, not the one-arm form"
    );
}

/// v5.3.0's correction: an EMPTY `#if` group after `asserterror` or `then` no longer
/// takes the `body` / `then_branch` field; the statement after it does. So the call
/// lowers INSIDE the `asserterror` (trapped) and the `then` block, where v5.2.0
/// lowered it as the next statement.
///
/// DISCRIMINATION: the group given a statement (`#if D Pre(); #endif`), which takes
/// the body on v5.3.0 as the empty group did on v5.2.0 -> the `asserterror` block
/// holds no call (`[]`, not `["Foo"]`).
#[test]
fn an_empty_group_before_a_body_no_longer_takes_the_body() {
    let af = parse(
        "codeunit 50104 G\n{\n    procedure Go()\n    begin\n        asserterror\n#if D\n#endif\n            Foo();\n        if true then\n#if D\n#endif\n            Bar();\n    end;\n}\n",
    );
    let calls_in = |b: crate::ir::BlockId| -> Vec<String> {
        af.ir
            .block(b)
            .items
            .iter()
            .filter_map(|i| match i {
                crate::ir::BlockItem::Stmt(s) => match &af.ir.stmt(*s).kind {
                    StmtKind::Call(e) => Some(callee(&af, *e)),
                    _ => None,
                },
                crate::ir::BlockItem::Preproc(_) => None,
            })
            .collect()
    };
    let assert_block = af
        .ir
        .iter_stmts()
        .find_map(|s| match &s.kind {
            StmtKind::AssertError(b) => Some(*b),
            _ => None,
        })
        .expect("the asserterror");
    assert_eq!(calls_in(assert_block), vec!["Foo".to_string()]);
    let then_block = af
        .ir
        .iter_stmts()
        .find_map(|s| match &s.kind {
            StmtKind::If { then_block, .. } => Some(*then_block),
            _ => None,
        })
        .expect("the if");
    assert_eq!(calls_in(then_block), vec!["Bar".to_string()]);
}

//! Witnesses for the grammar-v4 changes that alter what a program MEANS
//! (TLS-41; `docs/OUTSTANDING.md`, "Fixture witnesses for the grammar-v4 semantic
//! fixes"): a dangling `else` binds to the inner `if`, and the operator
//! precedence repairs (`and`/`or`/`xor` bind tighter than comparison, unary minus
//! tighter than `*`, `..` only inside a list or a case label). No golden pinned
//! the IR shape of either, so a grammar regression on `main` would have passed
//! unseen. The end-to-end witness is the r0-corpus fixture
//! `ws-grammar-v4-shapes` (its routines are in `l2_features.snapshot`). That
//! digest moves for any change to the routines, source positions included, so
//! it shows THAT something moved; these tests say WHICH shape.
//!
//! Discrimination (2026-11-13): each source here was rewritten to the OTHER
//! meaning (the `else` moved to the outer `if` with `begin`/`end`; the operands
//! parenthesised the other way; the ranges written as plain values), and each
//! of the four tests failed on it with the other shape; restored, each passes. The `.._other_meaning` tests keep those rewrites as executable
//! contrasts, so the renderer is shown to tell the two meanings apart.

use crate::ir::{AlFile, BinaryOp, ExprId, ExprKind, Literal, StmtKind, UnaryOp};
use crate::parse;

/// The expression as an s-expression: `(and a b)`, `(neg a)`, `[1 (.. 1 5)]`.
/// Parentheses written in the source show as `(paren x)`.
fn sx(af: &AlFile, id: ExprId) -> String {
    let e = &af.ir.expr(id).kind;
    match e {
        ExprKind::Identifier(n) | ExprKind::QuotedIdentifier(n) => n.clone(),
        ExprKind::Literal(Literal::Int(t)) => t.clone(),
        ExprKind::Literal(Literal::Bool(b)) => b.to_string(),
        ExprKind::Unary { op, operand } => {
            let op = match op {
                UnaryOp::Not => "not",
                UnaryOp::Neg => "neg",
                UnaryOp::Plus => "pos",
            };
            format!("({op} {})", sx(af, *operand))
        }
        ExprKind::Binary { op, lhs, rhs } => {
            let op = match op {
                BinaryOp::Add => "+",
                BinaryOp::Sub => "-",
                BinaryOp::Mul => "*",
                BinaryOp::Div => "/",
                BinaryOp::IntDiv => "div",
                BinaryOp::Mod => "mod",
                BinaryOp::Eq => "=",
                BinaryOp::Ne => "<>",
                BinaryOp::Lt => "<",
                BinaryOp::Le => "<=",
                BinaryOp::Gt => ">",
                BinaryOp::Ge => ">=",
                BinaryOp::And => "and",
                BinaryOp::Or => "or",
                BinaryOp::Xor => "xor",
                BinaryOp::In => "in",
                BinaryOp::Other => "other",
            };
            format!("({op} {} {})", sx(af, *lhs), sx(af, *rhs))
        }
        ExprKind::Parenthesized(x) => format!("(paren {})", sx(af, *x)),
        ExprKind::RangeExpr { start, end } => {
            format!("(.. {} {})", sx(af, *start), sx(af, *end))
        }
        ExprKind::List(items) => {
            let items: Vec<String> = items.iter().map(|i| sx(af, *i)).collect();
            format!("[{}]", items.join(" "))
        }
        _ => "?".to_string(),
    }
}

/// A codeunit whose one procedure's body is `body`.
fn unit(body: &str) -> AlFile {
    parse(&format!(
        "codeunit 50000 G\n{{\n    procedure P(A: Boolean; B: Boolean; C: Boolean; X: Integer; Y: Integer; Z: Integer)\n    begin\n{body}\n    end;\n}}\n"
    ))
}

/// Every `if` in source order: (its condition, whether it has an `else`, the
/// condition of each `if` directly in its `then` block).
fn ifs(af: &AlFile) -> Vec<(String, bool, Vec<String>)> {
    let mut out = Vec::new();
    for s in af.ir.iter_stmts() {
        if let StmtKind::If {
            cond,
            then_block,
            else_block,
        } = &s.kind
        {
            let inner = af
                .ir
                .block(*then_block)
                .items
                .iter()
                .filter_map(|i| match i {
                    crate::ir::BlockItem::Stmt(id) => match &af.ir.stmt(*id).kind {
                        StmtKind::If { cond, .. } => Some(sx(af, *cond)),
                        _ => None,
                    },
                    crate::ir::BlockItem::Preproc(_) => None,
                })
                .collect();
            out.push((sx(af, *cond), else_block.is_some(), inner));
        }
    }
    out.sort();
    out
}

/// The condition of the procedure's first `if`.
fn first_cond(af: &AlFile) -> String {
    af.ir
        .iter_stmts()
        .find_map(|s| match &s.kind {
            StmtKind::If { cond, .. } => Some(sx(af, *cond)),
            _ => None,
        })
        .expect("an if")
}

/// The right side of the procedure's first assignment.
fn first_value(af: &AlFile) -> String {
    af.ir
        .iter_stmts()
        .find_map(|s| match &s.kind {
            StmtKind::Assignment { value, .. } => Some(sx(af, *value)),
            _ => None,
        })
        .expect("an assignment")
}

#[test]
fn dangling_else_binds_to_the_inner_if() {
    let af = unit(
        "        if A then\n            if B then\n                X := 1\n            else\n                X := 2;",
    );
    assert!(af.issues.is_empty(), "{:?}", af.issues);
    // The outer `if A` has no else; the inner `if B` has it.
    assert_eq!(
        ifs(&af),
        vec![
            ("A".to_string(), false, vec!["B".to_string()]),
            ("B".to_string(), true, vec![]),
        ]
    );
}

#[test]
fn dangling_else_other_meaning() {
    // The else on the outer `if`, written with begin/end: the renderer sees it.
    let af = unit(
        "        if A then begin\n            if B then\n                X := 1;\n        end else\n            X := 2;",
    );
    assert_eq!(
        ifs(&af),
        vec![
            ("A".to_string(), true, vec!["B".to_string()]),
            ("B".to_string(), false, vec![]),
        ]
    );
}

#[test]
fn logical_operators_bind_tighter_than_comparison() {
    // AL: `and` is a multiplying operator and `or`/`xor` adding ones, both above
    // the relational operators.
    for (src, want) in [
        ("X = Y and C", "(= X (and Y C))"),
        ("X = Y or C", "(= X (or Y C))"),
        ("X < Y xor C", "(< X (xor Y C))"),
        ("A or B and C", "(or A (and B C))"),
    ] {
        let af = unit(&format!("        if {src} then\n            X := 1;"));
        assert!(af.issues.is_empty(), "{src}: {:?}", af.issues);
        assert_eq!(first_cond(&af), want, "{src}");
    }
}

#[test]
fn logical_operators_other_meaning() {
    let af = unit("        if (X = Y) and C then\n            X := 1;");
    assert_eq!(first_cond(&af), "(and (paren (= X Y)) C)");
}

#[test]
fn unary_minus_binds_tighter_than_multiplication() {
    for (src, want) in [
        ("-X * Y", "(* (neg X) Y)"),
        ("X + Y * Z", "(+ X (* Y Z))"),
        ("not A and B", "(and (not A) B)"),
    ] {
        let af = unit(&format!("        X := {src};"));
        assert!(af.issues.is_empty(), "{src}: {:?}", af.issues);
        assert_eq!(first_value(&af), want, "{src}");
    }
}

#[test]
fn unary_minus_other_meaning() {
    let af = unit("        X := -(X * Y);");
    assert_eq!(first_value(&af), "(neg (paren (* X Y)))");
}

#[test]
fn range_is_a_list_element_and_a_case_label() {
    let af = unit(
        "        if X in [1, 3..5] then\n            X := 1;\n        case X of\n            1..5:\n                X := 2;\n        end;",
    );
    assert!(af.issues.is_empty(), "{:?}", af.issues);
    assert_eq!(first_cond(&af), "(in X [1 (.. 3 5)])");
    let labels: Vec<String> = af
        .ir
        .iter_stmts()
        .find_map(|s| match &s.kind {
            StmtKind::Case { branches, .. } => Some(
                branches
                    .iter()
                    .flat_map(|b| b.patterns.iter().map(|p| sx(&af, *p)))
                    .collect(),
            ),
            _ => None,
        })
        .expect("a case");
    assert_eq!(labels, ["(.. 1 5)"]);
}

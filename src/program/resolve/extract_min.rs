//! Minimal call-expression extraction — Phase 0 Task 3.
//!
//! Walks every routine body in an [`AlFile`] and emits one [`RawSite`] per
//! call expression. The result feeds the Phase-0 stub resolver and the
//! dual-run differential harness.
//!
//! # Signature note
//! The brief's draft signature was `extract_raw_sites(file, unit)`. We add
//! `src: &str` (the original AL source text) because `Origin` stores byte
//! ranges that must be sliced into `src` to recover callee text. The test
//! call is updated accordingly.

use al_syntax::IdentifierFoldExt;
use al_syntax::ir::{AlFile, BlockId, BlockItem, ExprId, ExprKind, StmtKind};

use crate::program::resolve::edge::{CanonicalSpan, SourcePos};

/// One call-expression site extracted from a routine body.
#[derive(Debug, Clone)]
pub struct RawSite {
    /// The enclosing routine's name, lowercased.
    pub caller_routine: String,
    /// Raw source text of the callee (function) expression — e.g. `"Foo"` or
    /// `"Rec.Insert"`. Does NOT include the argument list.
    pub callee_text: String,
    /// Source span of the whole call expression (callee + arg list).
    pub span: CanonicalSpan,
}

/// Convert a byte offset into a 0-based `(line, col)` source position by
/// counting newlines in the prefix `src[..byte]`.
fn byte_to_pos(src: &str, byte: usize) -> SourcePos {
    let byte = byte.min(src.len());
    let prefix = &src[..byte];
    let line = prefix.bytes().filter(|&b| b == b'\n').count() as u32;
    let col = match prefix.rfind('\n') {
        Some(nl) => (byte - nl - 1) as u32,
        None => byte as u32,
    };
    SourcePos { line, col }
}

/// Recursively collect every [`ExprKind::Call`] reachable from `eid`,
/// including calls nested inside arguments or chained receivers.
fn collect_calls(
    file: &AlFile,
    src: &str,
    eid: ExprId,
    unit: &str,
    caller: &str,
    out: &mut Vec<RawSite>,
) {
    let e = file.ir.expr(eid);
    match &e.kind {
        ExprKind::Call { function, args } => {
            // Copy out the ids so the borrow on `e` ends before we recurse.
            let fn_id = *function;
            let arg_ids = args.to_vec();

            // Emit one site for this call expression.
            let callee_text = src[file.ir.expr(fn_id).origin.byte.clone()].to_string();
            let span = CanonicalSpan {
                unit: unit.to_string(),
                start: byte_to_pos(src, e.origin.byte.start),
                end: byte_to_pos(src, e.origin.byte.end),
            };
            out.push(RawSite {
                caller_routine: caller.to_string(),
                callee_text,
                span,
            });

            // Recurse: function expression (catches chained calls), then args.
            collect_calls(file, src, fn_id, unit, caller, out);
            for a in arg_ids {
                collect_calls(file, src, a, unit, caller, out);
            }
        }
        ExprKind::Member { object, .. } => {
            let obj = *object;
            collect_calls(file, src, obj, unit, caller, out);
        }
        ExprKind::Binary { lhs, rhs, .. } => {
            let (l, r) = (*lhs, *rhs);
            collect_calls(file, src, l, unit, caller, out);
            collect_calls(file, src, r, unit, caller, out);
        }
        ExprKind::Unary { operand, .. } => {
            let op = *operand;
            collect_calls(file, src, op, unit, caller, out);
        }
        ExprKind::Parenthesized(x) => {
            let x = *x;
            collect_calls(file, src, x, unit, caller, out);
        }
        ExprKind::Index { base, index } => {
            let (b, i) = (*base, *index);
            collect_calls(file, src, b, unit, caller, out);
            collect_calls(file, src, i, unit, caller, out);
        }
        ExprKind::RangeExpr { start, end } => {
            let (s, e2) = (*start, *end);
            collect_calls(file, src, s, unit, caller, out);
            collect_calls(file, src, e2, unit, caller, out);
        }
        ExprKind::QualifiedEnum { enum_type, .. } => {
            let et = *enum_type;
            collect_calls(file, src, et, unit, caller, out);
        }
        ExprKind::Ternary { .. } | ExprKind::TypeOp { .. } | ExprKind::List(_) => {
            for c in file.ir.expr(eid).kind.children() {
                collect_calls(file, src, c, unit, caller, out);
            }
        }
        // Identifier / QuotedIdentifier / Literal / DatabaseReference /
        // Unknown: no nested calls.
        _ => {}
    }
}

fn walk_block(
    file: &AlFile,
    src: &str,
    bid: BlockId,
    unit: &str,
    caller: &str,
    out: &mut Vec<RawSite>,
) {
    for item in &file.ir.block(bid).items {
        match item {
            BlockItem::Stmt(sid) => {
                let st = file.ir.stmt(*sid);
                walk_stmt(file, src, &st.kind, unit, caller, out);
            }
            BlockItem::Preproc(g) => {
                for b in &g.branches {
                    walk_block(file, src, *b, unit, caller, out);
                }
            }
        }
    }
}

fn walk_stmt(
    file: &AlFile,
    src: &str,
    kind: &StmtKind,
    unit: &str,
    caller: &str,
    out: &mut Vec<RawSite>,
) {
    match kind {
        StmtKind::Assignment { target, value, .. } => {
            collect_calls(file, src, *target, unit, caller, out);
            collect_calls(file, src, *value, unit, caller, out);
        }
        StmtKind::Call(eid) => {
            collect_calls(file, src, *eid, unit, caller, out);
        }
        StmtKind::If {
            cond,
            then_block,
            else_block,
        } => {
            collect_calls(file, src, *cond, unit, caller, out);
            walk_block(file, src, *then_block, unit, caller, out);
            if let Some(b) = else_block {
                walk_block(file, src, *b, unit, caller, out);
            }
        }
        StmtKind::Case {
            scrutinee,
            branches,
            else_block,
        } => {
            collect_calls(file, src, *scrutinee, unit, caller, out);
            for br in branches {
                for &p in &br.patterns {
                    collect_calls(file, src, p, unit, caller, out);
                }
                walk_block(file, src, br.body, unit, caller, out);
            }
            if let Some(b) = else_block {
                walk_block(file, src, *b, unit, caller, out);
            }
        }
        StmtKind::While { cond, body } => {
            collect_calls(file, src, *cond, unit, caller, out);
            walk_block(file, src, *body, unit, caller, out);
        }
        StmtKind::Repeat { body, until } => {
            walk_block(file, src, *body, unit, caller, out);
            collect_calls(file, src, *until, unit, caller, out);
        }
        StmtKind::For {
            var,
            from,
            to,
            body,
            ..
        } => {
            collect_calls(file, src, *var, unit, caller, out);
            collect_calls(file, src, *from, unit, caller, out);
            collect_calls(file, src, *to, unit, caller, out);
            walk_block(file, src, *body, unit, caller, out);
        }
        StmtKind::Foreach {
            var,
            iterable,
            body,
        } => {
            collect_calls(file, src, *var, unit, caller, out);
            collect_calls(file, src, *iterable, unit, caller, out);
            walk_block(file, src, *body, unit, caller, out);
        }
        StmtKind::With { receiver, body } => {
            collect_calls(file, src, *receiver, unit, caller, out);
            walk_block(file, src, *body, unit, caller, out);
        }
        StmtKind::Try { body, catch_block } => {
            walk_block(file, src, *body, unit, caller, out);
            if let Some(c) = catch_block {
                walk_block(file, src, *c, unit, caller, out);
            }
        }
        StmtKind::AssertError(body) => {
            walk_block(file, src, *body, unit, caller, out);
        }
        StmtKind::Exit(x) => {
            if let Some(e) = x {
                collect_calls(file, src, *e, unit, caller, out);
            }
        }
        StmtKind::Block(b) => {
            walk_block(file, src, *b, unit, caller, out);
        }
        StmtKind::Break | StmtKind::Continue | StmtKind::Unknown => {}
    }
}

/// Walk every routine body in `file` and return one [`RawSite`] per call
/// expression (statement-position and expression-position alike).
///
/// `src` is the original AL source text; byte origins in the IR index into it
/// to recover callee text. `unit` names the file (e.g. `"C.al"`).
///
/// The result is sorted by `(caller_routine, span.start)` for determinism.
pub fn extract_raw_sites(file: &AlFile, src: &str, unit: &str) -> Vec<RawSite> {
    let mut out = Vec::new();
    for obj in &file.objects {
        for routine in &obj.routines {
            if let Some(body) = routine.body {
                let caller = routine.name.fold_identifier();
                walk_block(file, src, body, unit, &caller, &mut out);
            }
        }
    }
    out.sort_by(|a, b| {
        a.caller_routine
            .cmp(&b.caller_routine)
            .then_with(|| a.span.start.cmp(&b.span.start))
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_one_site_per_call_expression() {
        // Fixture exercises four call patterns:
        //  1. Top-level bare call:             Foo()
        //  2. Top-level call with literals:    Bar(1, 2)
        //  3. Nested-argument call:            Baz(Foo())  → emits BOTH Baz and the inner Foo
        //  4. Member (receiver) call:          Rec.Insert(true)
        let src = r#"
codeunit 50100 "C"
{
    procedure Run()
    var
        Rec: Record Customer;
    begin
        Foo();
        Bar(1, 2);
        Baz(Foo());
        Rec.Insert(true);
    end;
    procedure Foo() begin end;
    procedure Bar(a: Integer; b: Integer) begin end;
    procedure Baz(x: Integer) begin end;
}
"#;
        let file = al_syntax::parse(src);
        let sites = extract_raw_sites(&file, src, "C.al");
        let in_run: Vec<_> = sites.iter().filter(|s| s.caller_routine == "run").collect();

        // 5 call expressions total in Run():
        //   Foo(), Bar(1,2), Baz (outer), Foo (nested inside Baz arg), Rec.Insert
        assert_eq!(in_run.len(), 5, "sites: {sites:?}");

        // Bar() must be captured (guards that top-level calls with args are covered).
        assert!(
            in_run
                .iter()
                .any(|s| s.callee_text.to_ascii_lowercase().contains("bar")),
            "expected a 'Bar' site; sites: {sites:?}"
        );

        // Foo appears twice: once as top-level and once nested inside Baz(Foo()).
        // This guards that the walk recurses into call arguments.
        let foo_count = in_run
            .iter()
            .filter(|s| s.callee_text.to_ascii_lowercase().contains("foo"))
            .count();
        assert_eq!(
            foo_count, 2,
            "expected 2 'Foo' sites (top-level + nested arg); sites: {sites:?}"
        );

        // Member call Rec.Insert(true) must appear, guarding member-call coverage.
        assert!(
            in_run
                .iter()
                .any(|s| s.callee_text.to_ascii_lowercase().contains("insert")),
            "expected a 'Rec.Insert' site; sites: {sites:?}"
        );

        // Spans are non-degenerate and ordered by source position.
        assert!(in_run[0].span.start <= in_run[1].span.start);
    }
}

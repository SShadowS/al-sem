//! Parse entry point: AL source → owned [`AlFile`]. The tree-sitter `Tree` lives
//! only for the duration of lowering; everything the engine needs is copied into
//! the owned IR before it drops.

use crate::ir::AlFile;
use crate::lower;
use crate::raw::RawNode;

/// Parse + lower one AL source file.
pub fn parse(source: &str) -> AlFile {
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&crate::language::language())
        .expect("load AL grammar");
    let tree = parser
        .parse(source, None)
        .expect("tree-sitter parse returned None");
    lower::lower_file(RawNode::new(tree.root_node()), source)
}

#[cfg(test)]
mod tests {
    use super::parse;
    use crate::ir::ParseStatus;

    #[test]
    fn parses_minimal_codeunit() {
        let f = parse("codeunit 50000 Foo\n{\n    procedure Bar()\n    begin\n    end;\n}\n");
        assert_eq!(f.parse_status, ParseStatus::Clean);
    }

    #[test]
    fn flags_recovery_on_broken_source() {
        let f = parse("codeunit 50000 Foo\n{\n    procedure Bar(  @@@ \n");
        assert_eq!(f.parse_status, ParseStatus::Recovered);
    }

    #[test]
    fn lowers_outer_structure() {
        use crate::ir::{ObjectKind, RoutineKind};
        let src = "codeunit 50000 Foo\n{\n    var\n        G: Integer;\n    \
                   procedure Bar(var X: Integer; Y: Code[20]): Boolean\n    var\n        \
                   L: Text;\n    begin\n    end;\n\n    trigger OnRun()\n    begin\n    end;\n}\n";
        let f = parse(src);
        assert_eq!(f.parse_status, ParseStatus::Clean);
        assert_eq!(f.objects.len(), 1);
        let o = &f.objects[0];
        assert_eq!(o.kind, ObjectKind::Codeunit);
        assert_eq!(o.id, Some(50000));
        assert_eq!(o.name, "Foo");
        assert_eq!(o.globals.len(), 1, "object global G");
        assert_eq!(o.globals[0].name, "G");
        assert_eq!(o.routines.len(), 2, "Bar + OnRun");
        let bar = o.routines.iter().find(|r| r.name == "Bar").expect("Bar");
        assert_eq!(bar.kind, RoutineKind::Procedure);
        assert_eq!(bar.params.len(), 2);
        assert!(bar.params[0].by_ref, "var X");
        assert!(!bar.params[1].by_ref, "Y");
        assert_eq!(bar.return_type.as_deref(), Some("Boolean"));
        assert_eq!(bar.locals.len(), 1);
        assert_eq!(bar.locals[0].name, "L");
        let onrun = o
            .routines
            .iter()
            .find(|r| r.name == "OnRun")
            .expect("OnRun");
        assert_eq!(onrun.kind, RoutineKind::Trigger);
    }

    #[test]
    fn lowers_statement_body() {
        use crate::ir::{BlockItem, StmtKind};
        let src = "codeunit 1 A\n{\n    procedure P()\n    var\n        i: Integer;\n    begin\n        \
                   i := 1;\n        if i > 0 then\n            Message('x')\n        else\n            \
                   Clear(i);\n        while i < 10 do\n            i += 1;\n    end;\n}\n";
        let f = parse(src);
        assert_eq!(f.parse_status, ParseStatus::Clean);
        let r = &f.objects[0].routines[0];
        let body = r.body.expect("body");
        let blk = f.ir.block(body);
        assert_eq!(blk.items.len(), 3, "assignment, if, while");
        match blk.items[0] {
            BlockItem::Stmt(sid) => {
                assert!(matches!(f.ir.stmt(sid).kind, StmtKind::Assignment { .. }));
            }
            _ => panic!("expected stmt"),
        }
        match blk.items[1] {
            BlockItem::Stmt(sid) => {
                assert!(matches!(f.ir.stmt(sid).kind, StmtKind::If { .. }));
            }
            _ => panic!("expected if"),
        }
        let msgs: Vec<&String> = f.issues.iter().map(|i| &i.message).collect();
        assert!(
            f.issues.is_empty(),
            "unexpected unlowered nodes: {:?}",
            msgs
        );
    }

    // -------------------------------------------------------------------
    // T2.1 (stack-overflow hardening): the lowerer's depth budget must fail
    // closed on pathological nesting instead of overflowing the native
    // stack. Run on a thread sized to the LSP's real main-thread stack on
    // Windows (~1 MiB — see `src/snapshot/parse.rs`'s doc) so a regression
    // here reproduces the actual crash this hardens against, not just a
    // slow pass on the test harness's own (much larger) default stack.
    // -------------------------------------------------------------------

    const SMALL_STACK: usize = 1024 * 1024;

    #[test]
    fn deep_binary_expression_degrades_instead_of_overflowing_small_stack() {
        // A 50k-deep right-associative binary-expression chain (`1 + 1 + … +
        // 1`) recurses `lower_expr` once per `+` via `lower_opt_field`.
        let mut src = String::from(
            "codeunit 50000 Deep\n{\n    procedure P()\n    var\n        X: Integer;\n    \
             begin\n        X := 1",
        );
        for _ in 0..50_000 {
            src.push_str(" + 1");
        }
        src.push_str(";\n    end;\n}\n");

        let handle = std::thread::Builder::new()
            .stack_size(SMALL_STACK)
            .spawn(move || parse(&src))
            .expect("spawn small-stack worker");
        let f = handle.join().expect("lowering must not crash the thread");

        assert_eq!(f.objects.len(), 1);
        assert!(
            f.issues
                .iter()
                .any(|i| i.message.contains("lowering depth budget")),
            "expected a depth-budget SyntaxIssue, got: {:?}",
            f.issues.iter().map(|i| &i.message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn deep_parenthesized_expression_degrades_instead_of_overflowing_small_stack() {
        // `((((…1…))))` recurses `lower_expr` directly on itself (no
        // `lower_opt_field` indirection) — a distinct code path from the
        // binary-chain case above.
        let mut src = String::from(
            "codeunit 50001 DeepParen\n{\n    procedure P()\n    var\n        X: Integer;\n    \
             begin\n        X := ",
        );
        src.push_str(&"(".repeat(5_000));
        src.push('1');
        src.push_str(&")".repeat(5_000));
        src.push_str(";\n    end;\n}\n");

        let handle = std::thread::Builder::new()
            .stack_size(SMALL_STACK)
            .spawn(move || parse(&src))
            .expect("spawn small-stack worker");
        let f = handle.join().expect("lowering must not crash the thread");

        assert_eq!(f.objects.len(), 1);
        assert!(
            f.issues
                .iter()
                .any(|i| i.message.contains("lowering depth budget")),
            "expected a depth-budget SyntaxIssue, got: {:?}",
            f.issues.iter().map(|i| &i.message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn deep_nested_case_groups_degrade_instead_of_overflowing_small_stack() {
        // 300 nested `#if` case groups (`preproc_conditional_case` in
        // `preproc_conditional_case`) recurse `collect_case_branches_in` once per
        // level; past `MAX_LOWER_DEPTH` it must stop and say so.
        let mut src = String::from(
            "codeunit 50002 DeepCase\n{\n    procedure P(I: Integer)\n    begin\n        \
             case I of\n",
        );
        src.push_str(&"#if A\n".repeat(300));
        src.push_str("1: Foo();\n");
        src.push_str(&"#endif\n".repeat(300));
        src.push_str("        end;\n    end;\n}\n");

        let handle = std::thread::Builder::new()
            .stack_size(SMALL_STACK)
            .spawn(move || parse(&src))
            .expect("spawn small-stack worker");
        let f = handle.join().expect("lowering must not crash the thread");

        assert_eq!(f.objects.len(), 1);
        assert!(
            f.issues
                .iter()
                .any(|i| i.message.contains("case-group nesting exceeds")),
            "expected a case-group depth-budget SyntaxIssue, got: {:?}",
            f.issues.iter().map(|i| &i.message).collect::<Vec<_>>()
        );
    }
}

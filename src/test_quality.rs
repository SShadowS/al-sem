//! `--analyze --format json`'s `test_quality` block (#253): every `[Test]`
//! procedure's assert counts, summed per test codeunit and for the workspace.
//!
//! An assert is a call `X.M(..)` whose receiver `X` is declared (local, then
//! parameter, then object global; `this.X` reads globals only) with a type that
//! classifies as `Codeunit Assert` or `Codeunit "Library Assert"`, or an
//! `asserterror` statement. The own count walks the routine's IR body; the
//! reachable count adds the own counts of every routine the test reaches
//! through resolved same-app call edges (`edges_by_file`: Call, Run,
//! ImplicitTrigger; never event links), each routine once per test.

use std::collections::{HashMap, HashSet};

use al_syntax::IdentifierFoldExt;
use al_syntax::ir::{
    BlockId, BlockItem, ExprId, ExprKind, Ir, Literal, ObjectDecl, ObjectKind, RoutineDecl,
    StmtKind, UnaryOp,
};
use serde::Serialize;

use crate::lsp::snapshot::{LspSnapshot, LspTarget};
use crate::program::RoutineNodeId;
use crate::program::node_extract::ObjectRef;
use crate::program::resolve::edge::EdgeKind;
use crate::program::resolve::receiver::{ParsedType, classify_type_text};

/// `AreEqual` / `AreNotEqual` / `AreNearlyEqual` calls by the shape of the
/// expected (first) argument.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct AreEqualSplit {
    pub total: usize,
    pub literal_expected: usize,
    pub expression_expected: usize,
}

impl AreEqualSplit {
    fn add(&mut self, other: AreEqualSplit) {
        self.total += other.total;
        self.literal_expected += other.literal_expected;
        self.expression_expected += other.expression_expected;
    }
}

/// What one routine body holds by itself.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BodyCounts {
    pub assert_calls: usize,
    pub asserterrors: usize,
    pub are_equal: AreEqualSplit,
}

impl BodyCounts {
    /// Assert calls plus `asserterror` statements.
    pub fn own(&self) -> usize {
        self.assert_calls + self.asserterrors
    }
}

/// One `[Test]` procedure. The identity fields are the values of its
/// `metrics` row (they join on `(path, line)`).
#[derive(Debug, Clone, Serialize)]
pub struct TestProcedure {
    pub path: String,
    pub line: u32,
    pub object_type: String,
    pub object_name: String,
    pub object_id: Option<i64>,
    /// Internal object id `<appGuid>/<Type>/<key>`; `None` when the
    /// `routine_id`'s object part is (no app guid, nested app, no type).
    pub object_stable_id: Option<String>,
    pub procedure_name: String,
    pub routine_id: Option<String>,
    pub assert_count: usize,
    /// `None` without a call graph, never a guess. With a graph but no matching
    /// declaration it is the test's own count (`fill_reachable`).
    pub assert_count_reachable: Option<usize>,
    pub zero_assert: Option<bool>,
    pub has_asserterror: bool,
    pub are_equal: AreEqualSplit,
    /// The object's start line: groups the codeunit sums.
    #[serde(skip)]
    pub object_line: u32,
    /// The graph's declaration id, when the graph holds this routine.
    #[serde(skip)]
    pub node: Option<RoutineNodeId>,
}

/// Sums over test procedures. `assert_count_reachable` and
/// `zero_assert_procedures` are `None` as soon as one procedure's is.
#[derive(Debug, Clone, Serialize)]
pub struct TestSums {
    pub test_procedures: usize,
    pub assert_count: usize,
    pub assert_count_reachable: Option<usize>,
    pub zero_assert_procedures: Option<usize>,
    pub asserterror_procedures: usize,
    pub are_equal: AreEqualSplit,
}

impl Default for TestSums {
    fn default() -> Self {
        TestSums {
            test_procedures: 0,
            assert_count: 0,
            assert_count_reachable: Some(0),
            zero_assert_procedures: Some(0),
            asserterror_procedures: 0,
            are_equal: AreEqualSplit::default(),
        }
    }
}

impl TestSums {
    fn add(&mut self, p: &TestProcedure) {
        self.test_procedures += 1;
        self.assert_count += p.assert_count;
        self.assert_count_reachable = self
            .assert_count_reachable
            .zip(p.assert_count_reachable)
            .map(|(a, b)| a + b);
        self.zero_assert_procedures = self
            .zero_assert_procedures
            .zip(p.zero_assert)
            .map(|(n, zero)| n + usize::from(zero));
        self.asserterror_procedures += usize::from(p.has_asserterror);
        self.are_equal.add(p.are_equal);
    }
}

/// One object holding at least one `[Test]` procedure.
#[derive(Debug, Clone, Serialize)]
pub struct TestCodeunit {
    pub path: String,
    pub line: u32,
    pub object_type: String,
    pub object_name: String,
    pub object_id: Option<i64>,
    pub object_stable_id: Option<String>,
    #[serde(flatten)]
    pub sums: TestSums,
}

#[derive(Debug, Clone, Serialize)]
pub struct TestQuality {
    pub procedures: Vec<TestProcedure>,
    pub codeunits: Vec<TestCodeunit>,
    pub workspace: TestSums,
}

impl TestQuality {
    /// Sums `procedures` (already in `metrics` order) per object and overall.
    /// One object is `(path, object_type, object_id)`, so a file holding the
    /// same object in two `#if` arms gives one row; an object with no number
    /// falls back to its start line.
    pub fn new(procedures: Vec<TestProcedure>) -> Self {
        let mut codeunits: Vec<TestCodeunit> = Vec::new();
        let mut workspace = TestSums::default();
        for p in &procedures {
            workspace.add(p);
            let same = |c: &TestCodeunit| {
                c.path == p.path
                    && match p.object_id {
                        Some(id) => c.object_type == p.object_type && c.object_id == Some(id),
                        None => c.object_id.is_none() && c.line == p.object_line,
                    }
            };
            let at = codeunits.iter().rposition(same);
            if at.is_none() {
                codeunits.push(TestCodeunit {
                    path: p.path.clone(),
                    line: p.object_line,
                    object_type: p.object_type.clone(),
                    object_name: p.object_name.clone(),
                    object_id: p.object_id,
                    object_stable_id: p.object_stable_id.clone(),
                    sums: TestSums::default(),
                });
            }
            let i = at.unwrap_or(codeunits.len() - 1);
            codeunits[i].sums.add(p);
        }
        TestQuality {
            procedures,
            codeunits,
            workspace,
        }
    }
}

/// Whether `r` is a test procedure (`[Test]`; the lowerer lowercases names).
pub fn is_test(r: &RoutineDecl) -> bool {
    r.attributes.iter().any(|a| a == "test")
}

/// The own counts of `r`'s body; `None` when it has no body.
pub fn body_counts(ir: &Ir, obj: &ObjectDecl, r: &RoutineDecl) -> Option<BodyCounts> {
    let mut w = Walker {
        ir,
        obj,
        r,
        counts: BodyCounts::default(),
    };
    w.block(r.body?);
    Some(w.counts)
}

struct Walker<'a> {
    ir: &'a Ir,
    obj: &'a ObjectDecl,
    r: &'a RoutineDecl,
    counts: BodyCounts,
}

impl Walker<'_> {
    fn block(&mut self, b: BlockId) {
        for item in &self.ir.block(b).items {
            match item {
                BlockItem::Stmt(s) => self.stmt(*s),
                // Directives are not evaluated: every branch is source.
                BlockItem::Preproc(g) => {
                    for branch in &g.branches {
                        self.block(*branch);
                    }
                }
            }
        }
    }

    // No catch-all arm: a new statement kind must decide its slots here.
    fn stmt(&mut self, s: al_syntax::ir::StmtId) {
        match &self.ir.stmt(s).kind {
            StmtKind::Assignment { target, value, .. } => {
                self.expr(*target);
                self.expr(*value);
            }
            StmtKind::Call(e) => self.expr(*e),
            StmtKind::If {
                cond,
                then_block,
                else_block,
            } => {
                self.expr(*cond);
                self.block(*then_block);
                self.opt_block(*else_block);
            }
            StmtKind::Case {
                scrutinee,
                branches,
                else_block,
            } => {
                self.expr(*scrutinee);
                for br in branches {
                    for p in &br.patterns {
                        self.expr(*p);
                    }
                    self.block(br.body);
                }
                self.opt_block(*else_block);
            }
            StmtKind::While { cond, body } => {
                self.expr(*cond);
                self.block(*body);
            }
            StmtKind::Repeat { body, until } => {
                self.block(*body);
                self.expr(*until);
            }
            StmtKind::For {
                var,
                from,
                to,
                body,
                ..
            } => {
                self.expr(*var);
                self.expr(*from);
                self.expr(*to);
                self.block(*body);
            }
            StmtKind::Foreach {
                var,
                iterable,
                body,
            } => {
                self.expr(*var);
                self.expr(*iterable);
                self.block(*body);
            }
            StmtKind::With { receiver, body } => {
                self.expr(*receiver);
                self.block(*body);
            }
            StmtKind::Try { body, catch_block } => {
                self.block(*body);
                self.opt_block(*catch_block);
            }
            StmtKind::AssertError(b) => {
                self.counts.asserterrors += 1;
                self.block(*b);
            }
            StmtKind::Exit(e) => {
                if let Some(e) = e {
                    self.expr(*e);
                }
            }
            StmtKind::Block(b) => self.block(*b),
            StmtKind::Break | StmtKind::Continue | StmtKind::Unknown => {}
        }
    }

    fn opt_block(&mut self, b: Option<BlockId>) {
        if let Some(b) = b {
            self.block(b);
        }
    }

    fn expr(&mut self, e: ExprId) {
        let kind = &self.ir.expr(e).kind;
        if let ExprKind::Call { function, args } = kind
            && let ExprKind::Member { object, member, .. } = &self.ir.expr(*function).kind
            && self.is_assert_receiver(*object)
        {
            self.counts.assert_calls += 1;
            let method = unquote_member(member).fold_identifier();
            if matches!(
                method.as_str(),
                "areequal" | "arenotequal" | "arenearlyequal"
            ) {
                let split = &mut self.counts.are_equal;
                split.total += 1;
                if args.first().is_some_and(|a| literal_expected(self.ir, *a)) {
                    split.literal_expected += 1;
                } else {
                    split.expression_expected += 1;
                }
            }
        }
        for child in kind.children() {
            self.expr(child);
        }
    }

    /// A bare `X` (locals, then params, then globals) or `this.X` (globals)
    /// whose declared type is an assert codeunit.
    fn is_assert_receiver(&self, e: ExprId) -> bool {
        let ty = match &self.ir.expr(e).kind {
            ExprKind::Identifier(name) | ExprKind::QuotedIdentifier(name) => {
                // IR names are already unquoted (one outer pair removed).
                let hit = |n: &str| n.eq_fold_identifier(name);
                self.r
                    .locals
                    .iter()
                    .find(|v| hit(&v.name))
                    .map(|v| v.ty.as_deref())
                    .or_else(|| {
                        self.r
                            .params
                            .iter()
                            .find(|p| hit(&p.name))
                            .map(|p| p.ty.as_deref())
                    })
                    .or_else(|| self.global_type(name))
            }
            ExprKind::Member { object, member, .. }
                if matches!(&self.ir.expr(*object).kind,
                    ExprKind::Identifier(t) if t.eq_fold_identifier("this")) =>
            {
                self.global_type(unquote_member(member))
            }
            _ => None,
        };
        ty.flatten().is_some_and(is_assert_type)
    }

    /// `name` is an unquoted IR name.
    fn global_type(&self, name: &str) -> Option<Option<&str>> {
        self.obj
            .globals
            .iter()
            .find(|v| v.name.eq_fold_identifier(name))
            .map(|v| v.ty.as_deref())
    }
}

/// A `Member`'s name is raw source text: remove exactly one outer quote pair,
/// as the lowerer does for every other name.
fn unquote_member(m: &str) -> &str {
    m.strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .unwrap_or(m)
}

/// `Codeunit Assert` or `Codeunit "Library Assert"` (namespace and quotes
/// stripped). Numeric forms are not matched.
fn is_assert_type(ty: &str) -> bool {
    matches!(
        classify_type_text(ty),
        ParsedType::Object {
            kind: ObjectKind::Codeunit,
            object_ref: ObjectRef::Name { normalized_lc, .. },
        } if normalized_lc == "assert" || normalized_lc == "library assert"
    )
}

/// A compile-time constant expected value: a literal, a signed numeric
/// literal, an enum/option value or a `Database::X` reference.
fn literal_expected(ir: &Ir, mut e: ExprId) -> bool {
    while let ExprKind::Parenthesized(inner) = ir.expr(e).kind {
        e = inner;
    }
    match &ir.expr(e).kind {
        ExprKind::Literal(_) | ExprKind::QualifiedEnum { .. } | ExprKind::DatabaseReference(_) => {
            true
        }
        ExprKind::Unknown => is_unmodelled_literal(ir, e),
        ExprKind::Unary {
            op: UnaryOp::Neg | UnaryOp::Plus,
            operand,
        } => {
            matches!(
                ir.expr(*operand).kind,
                ExprKind::Literal(Literal::Int(_) | Literal::Decimal(_))
            ) || ir.expr(*operand).origin.kind_text == "biginteger_literal"
        }
        _ => false,
    }
}

/// The lowerer models no date, time, datetime or BigInteger literal
/// (`0D`, `0T`, `0DT`, `20261023D`, `123L`): they are `Unknown`, with the
/// grammar's kind kept in `origin.kind_text`.
fn is_unmodelled_literal(ir: &Ir, e: ExprId) -> bool {
    matches!(
        ir.expr(e).origin.kind_text,
        "date_literal" | "time_literal" | "datetime_literal" | "biginteger_literal"
    )
}

/// The resolved call graph the reachable count walks, per app root.
pub struct CallMap<'s> {
    /// (caller's app root, caller) -> resolved routine targets.
    calls: HashMap<(&'s str, &'s RoutineNodeId), Vec<&'s RoutineNodeId>>,
    /// (app root, id) -> own count summed over that root's declarations.
    own: HashMap<(&'s str, &'s RoutineNodeId), usize>,
}

impl<'s> CallMap<'s> {
    /// `root_of`: every workspace file's app root. `own_at`: per file, the
    /// own count of each routine with a body, by its start byte.
    pub fn new(
        snap: &'s LspSnapshot,
        root_of: &'s HashMap<String, String>,
        own_at: &HashMap<&str, HashMap<usize, usize>>,
    ) -> Self {
        let mut own: HashMap<(&str, &RoutineNodeId), usize> = HashMap::new();
        for (path, decls) in &snap.decls_by_file {
            let Some(root) = root_of.get(path) else {
                continue;
            };
            for d in decls.iter() {
                let n = own_at
                    .get(path.as_str())
                    .and_then(|m| m.get(&d.origin.byte.start))
                    .copied()
                    .unwrap_or(0);
                *own.entry((root.as_str(), &d.id)).or_default() += n;
            }
        }
        let mut calls: HashMap<(&str, &RoutineNodeId), Vec<&RoutineNodeId>> = HashMap::new();
        for (path, edges) in &snap.edges_by_file {
            let Some(root) = root_of.get(path) else {
                continue;
            };
            for e in edges.iter().filter(|e| {
                matches!(
                    e.kind,
                    EdgeKind::Call | EdgeKind::Run | EdgeKind::ImplicitTrigger
                )
            }) {
                let targets = calls.entry((root.as_str(), &e.from)).or_default();
                for t in e.targets.iter() {
                    if let LspTarget::Routine(id) = t {
                        targets.push(id);
                    }
                }
            }
        }
        CallMap { calls, own }
    }

    /// The test's own declaration count plus the own count of every other
    /// routine it reaches in its app root, each once.
    pub fn reachable(&self, root: &str, test: &RoutineNodeId, own: usize) -> usize {
        let mut visited: HashSet<&RoutineNodeId> = HashSet::from([test]);
        let mut stack = vec![test];
        let mut total = own;
        while let Some(id) = stack.pop() {
            for &t in self.calls.get(&(root, id)).into_iter().flatten() {
                // Same app root only: a target with no declaration there is skipped.
                let Some(&n) = self.own.get(&(root, t)) else {
                    continue;
                };
                if visited.insert(t) {
                    total += n;
                    stack.push(t);
                }
            }
        }
        total
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use al_syntax::ir::{Block, PreprocGroup};

    const UNIT: &str = r#"codeunit 50100 "U"
{
    var
        Assert: Codeunit Assert;

    procedure P()
    var
        Expected: Integer;
        C: Enum "Color";
    begin
        Assert.AreEqual(5, Expected, '');
        Assert.AreEqual(-5, Expected, '');
        Assert.AreEqual(+1.5, Expected, '');
        Assert.AreEqual('', Expected, '');
        Assert.AreEqual(true, Expected, '');
        Assert.AreEqual((7), Expected, '');
        Assert.AreEqual(Enum::"Color"::Red, C, '');
        Assert.AreEqual(Database::Customer, Expected, '');
        Assert.AreEqual(Expected, Expected, '');
        Assert.AreEqual(-Expected, Expected, '');
        Assert.AreEqual(Expected + 1, Expected, '');
    end;
}"#;

    /// The literal classifier, one call per shape (the `n`th assert call).
    #[test]
    fn literal_classifier_shapes() {
        let f = al_syntax::parse(UNIT);
        let r = &f.objects[0].routines[0];
        let mut firsts = Vec::new();
        for item in &f.ir.block(r.body.expect("body")).items {
            let BlockItem::Stmt(s) = item else {
                panic!("precondition: plain statements")
            };
            let StmtKind::Call(e) = &f.ir.stmt(*s).kind else {
                panic!("precondition: call statements")
            };
            let ExprKind::Call { args, .. } = &f.ir.expr(*e).kind else {
                panic!("precondition: a call")
            };
            firsts.push(literal_expected(&f.ir, args[0]));
        }
        // 5, -5, +1.5, '', true, (7), Enum, Database: literal; then three expressions.
        assert_eq!(
            firsts,
            [
                true, true, true, true, true, true, true, true, false, false, false
            ]
        );
        let counts = body_counts(&f.ir, &f.objects[0], r).expect("body");
        assert_eq!(
            counts.are_equal,
            AreEqualSplit {
                total: 11,
                literal_expected: 8,
                expression_expected: 3
            }
        );
    }

    /// U1: a `Preproc` block item (never emitted by the lowerer today) visits
    /// every branch.
    #[test]
    fn preproc_item_visits_every_branch() {
        let src = r#"codeunit 50100 "U"
{
    var
        Assert: Codeunit Assert;

    procedure P()
    begin
        Assert.IsTrue(true, 'a');
        Assert.IsTrue(true, 'b');
    end;
}"#;
        let mut f = al_syntax::parse(src);
        let body = f.objects[0].routines[0].body.expect("body");
        let origin = f.ir.block(body).origin.clone();
        let stmts: Vec<BlockItem> =
            f.ir.block(body)
                .items
                .iter()
                .map(|i| match i {
                    BlockItem::Stmt(s) => BlockItem::Stmt(*s),
                    BlockItem::Preproc(_) => panic!("precondition: plain statements"),
                })
                .collect();
        assert_eq!(stmts.len(), 2, "precondition: two assert statements");
        let mut branches = Vec::new();
        for item in stmts {
            branches.push(f.ir.add_block(Block {
                items: vec![item],
                origin: origin.clone(),
            }));
        }
        let group = f.ir.add_block(Block {
            items: vec![BlockItem::Preproc(PreprocGroup {
                branches,
                origin: origin.clone(),
            })],
            origin,
        });
        let obj = &f.objects[0];
        let mut w = Walker {
            ir: &f.ir,
            obj,
            r: &obj.routines[0],
            counts: BodyCounts::default(),
        };
        w.block(group);
        assert_eq!(w.counts.assert_calls, 2);
    }

    /// F2: the IR name of `"Assert"""` is `Assert""` (one outer quote pair
    /// removed by the lowerer). It is a different variable from `Assert`, so it
    /// must not hide the global assert codeunit.
    #[test]
    fn escaped_quote_name_does_not_hide_assert() {
        let src = "codeunit 50100 \"U\"\n{\n    var\n        Assert: Codeunit Assert;\n\n    procedure P()\n    var\n        \"Assert\"\"\": Integer;\n    begin\n        Assert.IsTrue(true, '');\n    end;\n}\n";
        let f = al_syntax::parse(src);
        let r = &f.objects[0].routines[0];
        assert!(!r.parse_incomplete, "precondition: clean parse");
        assert_eq!(r.locals[0].name, "Assert\"\"", "precondition: IR name");
        let counts = body_counts(&f.ir, &f.objects[0], r).expect("body");
        assert_eq!(counts.assert_calls, 1);
    }

    /// F1: date, time, datetime and BigInteger literals lower to `Unknown`
    /// with their grammar kind kept in `origin.kind_text`.
    #[test]
    fn unknown_literal_kinds_are_literal() {
        let src = "codeunit 50100 \"U\"\n{\n    var\n        Assert: Codeunit Assert;\n\n    procedure P()\n    var\n        X: Integer;\n    begin\n        Assert.AreEqual(0D, X, '');\n        Assert.AreEqual(0T, X, '');\n        Assert.AreEqual(0DT, X, '');\n        Assert.AreEqual(20261023D, X, '');\n        Assert.AreEqual(123L, X, '');\n        Assert.AreEqual(-123L, X, '');\n        Assert.AreEqual(X, X, '');\n    end;\n}\n";
        let f = al_syntax::parse(src);
        let r = &f.objects[0].routines[0];
        let mut firsts = Vec::new();
        for item in &f.ir.block(r.body.expect("body")).items {
            let BlockItem::Stmt(s) = item else {
                panic!("precondition: plain statements")
            };
            let StmtKind::Call(e) = &f.ir.stmt(*s).kind else {
                panic!("precondition: call statements")
            };
            let ExprKind::Call { args, .. } = &f.ir.expr(*e).kind else {
                panic!("precondition: a call")
            };
            firsts.push(literal_expected(&f.ir, args[0]));
        }
        assert_eq!(firsts, [true, true, true, true, true, true, false]);
    }
}

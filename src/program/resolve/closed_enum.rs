//! #264: an interface call whose receiver provably holds a value of a
//! non-extensible SOURCE enum dispatches over a closed set (spec §2–§4).
//!
//! [`closed_over`] is the one entry point (`full.rs`'s member arm). It proves,
//! for one call site, that every write to the routine-local interface variable
//! comes from closed enums ([`closed_enum_sources`]), and that every
//! implementation those enums map is the object of a real route on the edge
//! ([`enum_covered_by_routes`]). Every doubt fails OPEN: an empty result.

use al_syntax::IdentifierFoldExt;
use al_syntax::ir::{
    AlFile, BlockId, BlockItem, ExprId, ExprKind, Ir, ObjectDecl, ObjectKind, ParseStatus,
    RoutineDecl, StmtId, StmtKind,
};

use crate::program::graph::ProgramGraph;
use crate::program::node::ObjectNodeId;
use crate::program::node_extract::{ObjectNode, fold_ref_name};
use crate::program::resolve::edge::{Evidence, Route, RouteTarget};
use crate::program::resolve::index::{ObjectRefResolution, ResolveIndex};
use crate::program::resolve::receiver::{ParsedType, classify_type_text, parse_object_kind_type};

/// What [`closed_enum_sources`] reads about the call site.
pub(crate) struct ClosedEnumCx<'a> {
    pub file: &'a AlFile,
    pub routine: &'a RoutineDecl,
    pub obj: &'a ObjectDecl,
    pub from_object: &'a ObjectNode,
    pub graph: &'a ProgramGraph,
    pub index: &'a ResolveIndex,
}

/// The closed enums (sorted) the interface call on `receiver` dispatches over,
/// when its `routes` are exactly the reachable implementations; else empty.
pub(crate) fn closed_over(
    cx: &ClosedEnumCx,
    receiver: ExprId,
    interface_lc: &str,
    routes: &[Route],
) -> Vec<ObjectNodeId> {
    // Closed + empty would classify `Unknown` (`edge.rs`); a route that is not
    // real is an honest hole, never part of a closed set (spec §4).
    let real = |r: &Route| {
        !matches!(r.target, RouteTarget::Unresolved) && !matches!(r.evidence, Evidence::Unknown(_))
    };
    if routes.is_empty() || !routes.iter().all(real) {
        return Vec::new();
    }
    let Some(enums) = closed_enum_sources(cx, receiver, interface_lc) else {
        return Vec::new();
    };
    let covered = |id: &ObjectNodeId| {
        cx.graph
            .object(id)
            .is_some_and(|e| enum_covered_by_routes(cx.graph, cx.index, e, interface_lc, routes))
    };
    if enums.iter().all(covered) {
        enums
    } else {
        Vec::new()
    }
}

/// The closed enums (sorted, deduplicated) whose values are the ONLY values
/// the interface variable `receiver` can hold at the call (spec §3), or `None`:
/// 1. the file parsed cleanly and so did the routine;
/// 2. `receiver` (parentheses removed) names a routine LOCAL declared
///    `Interface I` in every `#if` arm, and no parameter, return name or object
///    global has that name;
/// 3. every occurrence of it in the body is a plain assignment target or the
///    object of a member call, there is at least one assignment, and the body
///    holds no `with` and nothing the lowerer could not model
///    ([`allowlisted_writes`]);
/// 4. every assigned value has a closed-enum identity implementing I
///    ([`enum_identity`]).
pub(crate) fn closed_enum_sources(
    cx: &ClosedEnumCx,
    receiver: ExprId,
    interface_lc: &str,
) -> Option<Vec<ObjectNodeId>> {
    if cx.file.parse_status != ParseStatus::Clean || cx.routine.parse_incomplete {
        return None;
    }
    let ir = &cx.file.ir;
    let v_lc = name_of(ir, receiver)?;
    if !is_interface_local(cx, &v_lc, interface_lc) {
        return None;
    }
    let writes = allowlisted_writes(ir, cx.routine.body?, &v_lc)?;
    let mut enums = writes
        .into_iter()
        .map(|value| enum_identity(cx, value, interface_lc))
        .collect::<Option<Vec<_>>>()?;
    enums.sort();
    enums.dedup();
    Some(enums)
}

/// The folded name of `e` when it is a (quoted) identifier, parentheses removed.
fn name_of(ir: &Ir, mut e: ExprId) -> Option<String> {
    loop {
        match &ir.expr(e).kind {
            ExprKind::Parenthesized(inner) => e = *inner,
            ExprKind::Identifier(n) | ExprKind::QuotedIdentifier(n) => {
                return Some(n.fold_identifier());
            }
            _ => return None,
        }
    }
}

/// Rule 2: `v_lc` is a local of the routine, every declaration of it (one per
/// `#if` arm) is `Interface <interface_lc>`, and nothing else in scope shares
/// the name.
fn is_interface_local(cx: &ClosedEnumCx, v_lc: &str, interface_lc: &str) -> bool {
    let r = cx.routine;
    let named = |n: &str| n.fold_identifier() == v_lc;
    let mut locals = r.locals.iter().filter(|l| named(&l.name)).peekable();
    let is_i = |ty: Option<&str>| matches!(ty.map(classify_type_text), Some(ParsedType::Interface { name }) if name == interface_lc);
    locals.peek().is_some()
        && locals.all(|l| is_i(l.ty.as_deref()))
        && !r.params.iter().any(|p| named(&p.name))
        && !r.return_name.as_deref().is_some_and(named)
        && !cx.obj.globals.iter().any(|g| named(&g.name))
}

/// Rule 3: the assigned VALUE of every write to `v_lc` in `body` (at least
/// one), or `None` when any occurrence is not admitted, the body holds a
/// `with`, or the lowerer left a statement or expression unmodelled.
fn allowlisted_writes(ir: &Ir, body: BlockId, v_lc: &str) -> Option<Vec<ExprId>> {
    let mut walk = Occurrences {
        ir,
        v_lc,
        writes: Vec::new(),
    };
    walk.block(body)?;
    (!walk.writes.is_empty()).then_some(walk.writes)
}

/// The rule-3 walk: every block (every `#if` branch), statement and expression.
/// Each method returns `None` as soon as the site cannot be closed.
struct Occurrences<'a> {
    ir: &'a Ir,
    v_lc: &'a str,
    writes: Vec<ExprId>,
}

impl Occurrences<'_> {
    fn is_v(&self, e: ExprId) -> bool {
        name_of(self.ir, e).is_some_and(|n| n == self.v_lc)
    }

    fn block(&mut self, b: BlockId) -> Option<()> {
        for item in &self.ir.block(b).items {
            match item {
                BlockItem::Stmt(s) => self.stmt(*s)?,
                BlockItem::Preproc(group) => {
                    for branch in &group.branches {
                        self.block(*branch)?;
                    }
                }
            }
        }
        Some(())
    }

    fn blocks(&mut self, blocks: impl IntoIterator<Item = BlockId>) -> Option<()> {
        blocks.into_iter().try_for_each(|b| self.block(b))
    }

    fn exprs(&mut self, exprs: impl IntoIterator<Item = ExprId>) -> Option<()> {
        exprs.into_iter().try_for_each(|e| self.expr(e))
    }

    fn stmt(&mut self, s: StmtId) -> Option<()> {
        match &self.ir.stmt(s).kind {
            StmtKind::Assignment {
                target,
                value,
                compound,
            } => {
                if self.is_v(*target) {
                    if *compound {
                        return None;
                    }
                    self.writes.push(*value);
                } else {
                    self.expr(*target)?;
                }
                self.expr(*value)
            }
            StmtKind::Call(e) => self.expr(*e),
            StmtKind::If {
                cond,
                then_block,
                else_block,
            } => {
                self.expr(*cond)?;
                self.blocks(std::iter::once(*then_block).chain(*else_block))
            }
            StmtKind::Case {
                scrutinee,
                branches,
                else_block,
            } => {
                self.expr(*scrutinee)?;
                for branch in branches {
                    self.exprs(branch.patterns.iter().copied())?;
                    self.block(branch.body)?;
                }
                self.blocks(*else_block)
            }
            StmtKind::While { cond, body } => {
                self.expr(*cond)?;
                self.block(*body)
            }
            StmtKind::Repeat { body, until } => {
                self.block(*body)?;
                self.expr(*until)
            }
            StmtKind::For {
                var,
                from,
                to,
                body,
                ..
            } => {
                self.exprs([*var, *from, *to])?;
                self.block(*body)
            }
            StmtKind::Foreach {
                var,
                iterable,
                body,
            } => {
                self.exprs([*var, *iterable])?;
                self.block(*body)
            }
            StmtKind::Try { body, catch_block } => {
                self.blocks(std::iter::once(*body).chain(*catch_block))
            }
            StmtKind::AssertError(b) | StmtKind::Block(b) => self.block(*b),
            StmtKind::Exit(value) => self.exprs(*value),
            StmtKind::Break | StmtKind::Continue => Some(()),
            // A `with` can rebind any name; an unmodelled statement can hide a write.
            StmtKind::With { .. } | StmtKind::Unknown => None,
        }
    }

    fn expr(&mut self, e: ExprId) -> Option<()> {
        if self.is_v(e) {
            // A bare (or parenthesized) occurrence: an argument, an operand, …
            return None;
        }
        match &self.ir.expr(e).kind {
            ExprKind::Unknown => None,
            ExprKind::Call { function, args }
                if matches!(&self.ir.expr(*function).kind,
                    ExprKind::Member { object, .. } if self.is_v(*object)) =>
            {
                self.exprs(args.iter().copied())
            }
            kind => self.exprs(kind.children()),
        }
    }
}

/// Rule 4 (Task 3: the variable arm): the closed enum `value` is a value of,
/// resolved where the variable's `Enum X` type is declared (the calling
/// object), or `None`. The enum must be `closed_enum` and implement I.
fn enum_identity(cx: &ClosedEnumCx, value: ExprId, interface_lc: &str) -> Option<ObjectNodeId> {
    let name = name_of(&cx.file.ir, value)?;
    let ty = declared_type(cx, &name)?;
    let (head, rest) = ty.trim().split_once(char::is_whitespace)?;
    if !head.eq_ignore_ascii_case("enum") {
        return None;
    }
    let ParsedType::Object { object_ref, .. } = parse_object_kind_type(ObjectKind::Enum, rest)
    else {
        return None;
    };
    let ObjectRefResolution::Unique(id) = cx.index.resolve_object_ref(
        cx.graph,
        cx.from_object.id.clone(),
        ObjectKind::Enum,
        &object_ref,
    ) else {
        return None;
    };
    let node = cx.graph.object(&id)?;
    let implements_i = node
        .implements
        .iter()
        .any(|i| fold_ref_name(i) == interface_lc);
    (node.closed_enum && implements_i).then_some(id)
}

/// The declared type text of the variable `name_lc` in scope at the routine:
/// its locals, parameters and named return value, else the object's globals.
/// `None` when it has none, or its declarations (`#if` arms) disagree.
fn declared_type<'a>(cx: &ClosedEnumCx<'a>, name_lc: &str) -> Option<&'a str> {
    let r = cx.routine;
    let named = |n: &str| n.fold_identifier() == name_lc;
    let routine_scope: Vec<Option<&str>> = r
        .locals
        .iter()
        .filter(|v| named(&v.name))
        .map(|v| v.ty.as_deref())
        .chain(
            r.params
                .iter()
                .filter(|p| named(&p.name))
                .map(|p| p.ty.as_deref()),
        )
        .chain(
            r.return_name
                .as_deref()
                .filter(|n| named(n))
                .map(|_| r.return_type.as_deref()),
        )
        .collect();
    let decls = if routine_scope.is_empty() {
        cx.obj
            .globals
            .iter()
            .filter(|g| named(&g.name))
            .map(|g| g.ty.as_deref())
            .collect()
    } else {
        routine_scope
    };
    let first = (*decls.first()?)?;
    decls.iter().all(|t| *t == Some(first)).then_some(first)
}

/// Spec §2: every implementation `enum_node` maps for `interface_lc` resolves,
/// from the enum's own app, to exactly one codeunit that is the object of one
/// of `routes`. Not vacuous (opus N3): the enum must be closed and hold at
/// least one entry for EVERY interface it implements.
pub(crate) fn enum_covered_by_routes(
    graph: &ProgramGraph,
    index: &ResolveIndex,
    enum_node: &ObjectNode,
    interface_lc: &str,
    routes: &[Route],
) -> bool {
    let has_entry = |iface: &str| {
        enum_node
            .enum_impls
            .iter()
            .any(|(i, _)| i.as_ref() == iface)
    };
    if !enum_node.closed_enum
        || !enum_node
            .implements
            .iter()
            .all(|i| has_entry(&fold_ref_name(i)))
        || !has_entry(interface_lc)
    {
        return false;
    }
    let reached: Vec<ObjectNodeId> = routes.iter().filter_map(Route::target_object).collect();
    enum_node
        .enum_impls
        .iter()
        .filter(|(i, _)| i.as_ref() == interface_lc)
        .all(|(_, codeunit)| {
            matches!(
                index.resolve_object_ref(graph, enum_node.id.clone(), ObjectKind::Codeunit, codeunit),
                ObjectRefResolution::Unique(id) if reached.contains(&id)
            )
        })
}

#[cfg(test)]
mod tests {
    use crate::program::resolve::edge::{DispatchShape, SetCompleteness};

    const APP_JSON: &str = r#"{"id":"26426426-0000-0000-0000-000000000003","name":"Closed264","publisher":"probe","version":"1.0.0.0","runtime":"14.0","idRanges":[{"from":50100,"to":50199}]}"#;

    const STRATEGY: &str = r#"interface "Demo Strategy"
{
    procedure NextKm(CurrentKm: Integer): Integer;
}
"#;

    const ITEM: &str = r#"table 50100 "Demo Item"
{
    fields
    {
        field(1; "No."; Code[20]) { }
    }
}
"#;

    /// A codeunit `name` implementing `Demo Strategy`.
    fn implementer(id: u32, name: &str) -> String {
        format!(
            r#"codeunit {id} "{name}" implements "Demo Strategy"
{{
    procedure NextKm(CurrentKm: Integer): Integer
    begin
        exit(CurrentKm + 1);
    end;
}}
"#
        )
    }

    /// Enum `name` implementing `Demo Strategy`, with `props` and `values`.
    fn kind(id: u32, name: &str, props: &str, values: &str) -> String {
        format!("enum {id} \"{name}\" implements \"Demo Strategy\"\n{{\n{props}\n{values}\n}}\n")
    }

    /// The closed `Demo Kind`: one value mapped to `Demo Impl`.
    fn demo_kind() -> String {
        kind(
            50102,
            "Demo Kind",
            "    Extensible = false;",
            r#"    value(0; Standard) { Implementation = "Demo Strategy" = "Demo Impl"; }"#,
        )
    }

    /// The caller codeunit: `Dispatch` with `vars` and `body`, plus `extra`
    /// members.
    fn caller(vars: &str, body: &str, extra: &str) -> String {
        format!(
            "codeunit 50103 \"Caller\"\n{{\n    procedure Dispatch()\n    var\n{vars}\n    begin\n{body}\n    end;\n{extra}\n}}\n"
        )
    }

    /// The common locals: `S` the interface, `K`/`K2` closed-enum values.
    const VARS: &str = "        S: Interface \"Demo Strategy\";\n        K: Enum \"Demo Kind\";\n        K2: Enum \"Demo Kind\";";

    /// The base workspace with `caller_text` as the caller; `(path, text)`
    /// entries of `more` replace or add files.
    fn workspace(caller_text: String, more: &[(&str, String)]) -> Vec<(String, String)> {
        let mut files: Vec<(String, String)> = vec![
            ("app.json".into(), APP_JSON.into()),
            ("src/Strategy.al".into(), STRATEGY.into()),
            ("src/Item.al".into(), ITEM.into()),
            ("src/Impl.al".into(), implementer(50101, "Demo Impl")),
            ("src/Kind.al".into(), demo_kind()),
            ("src/Caller.al".into(), caller_text),
        ];
        for (path, text) in more {
            match files.iter_mut().find(|(p, _)| p == path) {
                Some(f) => f.1 = text.clone(),
                None => files.push(((*path).into(), text.clone())),
            }
        }
        files
    }

    fn write(files: &[(String, String)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        for (rel, text) in files {
            let path = dir.path().join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        dir
    }

    /// The completeness of the ONE interface (`Polymorphic`) edge from
    /// `Caller.Dispatch`.
    fn completeness_in(root: &std::path::Path) -> SetCompleteness {
        let report = crate::program::resolve::full::resolve_full_program(root).expect("resolves");
        let hits: Vec<_> = report
            .edges
            .iter()
            .filter(|c| {
                c.edge.from.name_lc.as_ref() == "dispatch"
                    && c.edge.shape == DispatchShape::Polymorphic
            })
            .collect();
        assert_eq!(
            hits.len(),
            1,
            "precondition: one interface site, got {}",
            hits.len()
        );
        hits[0].edge.completeness
    }

    fn completeness(files: &[(String, String)]) -> SetCompleteness {
        completeness_in(write(files).path())
    }

    fn is_open(c: SetCompleteness) -> bool {
        matches!(c, SetCompleteness::Partial { .. })
    }

    /// The base shape: one write from a closed-enum local.
    #[test]
    fn i264_variable_write_from_closed_enum_is_complete() {
        let ws = workspace(
            caller(VARS, "        S := K;\n        S.NextKm(1);", ""),
            &[],
        );
        assert_eq!(completeness(&ws), SetCompleteness::Complete);
    }

    /// T28: writes on both branches of an `if`, each from a closed enum.
    #[test]
    fn i264_t28_conditional_writes_from_closed_variables_are_complete() {
        let vars = format!("{VARS}\n        C: Boolean;");
        let body = "        if C then\n            S := K\n        else\n            S := K2;\n        S.NextKm(1);";
        assert_eq!(
            completeness(&workspace(caller(&vars, body, ""), &[])),
            SetCompleteness::Complete
        );
    }

    /// T30: one write from a closed enum, one from an extensible one.
    #[test]
    fn i264_t30_closed_and_extensible_writes_stay_open() {
        let open_kind = kind(
            50104,
            "Open Kind",
            "    Extensible = true;",
            r#"    value(0; Standard) { Implementation = "Demo Strategy" = "Demo Impl"; }"#,
        );
        let vars = format!("{VARS}\n        KO: Enum \"Open Kind\";");
        let body = "        S := K;\n        S := KO;\n        S.NextKm(1);";
        let ws = workspace(caller(&vars, body, ""), &[("src/OpenKind.al", open_kind)]);
        assert!(is_open(completeness(&ws)));
    }

    /// Not implementing I: a closed enum over ANOTHER interface is never a
    /// source for a dispatch over I (Task 2 review, minor 2).
    #[test]
    fn i264_enum_not_implementing_the_interface_is_not_a_source() {
        let other = r#"interface "Other Strategy"
{
    procedure Other();
}

codeunit 50105 "Other Impl" implements "Other Strategy"
{
    procedure Other()
    begin
    end;
}

enum 50106 "Other Kind" implements "Other Strategy"
{
    Extensible = false;
    value(0; Standard) { Implementation = "Other Strategy" = "Other Impl"; }
}
"#;
        let vars = format!("{VARS}\n        KX: Enum \"Other Kind\";");
        let body = "        S := KX;\n        S.NextKm(1);";
        let ws = workspace(
            caller(&vars, body, ""),
            &[("src/Other.al", other.to_string())],
        );
        assert!(is_open(completeness(&ws)));
    }

    /// T8b: a parenthesized argument `Foo((S))` is an occurrence the allowlist
    /// does not admit (P2e: it cannot reassign `S`, but it is declined anyway).
    #[test]
    fn i264_t8b_parenthesized_argument_stays_open() {
        let body = "        S := K;\n        Foo((S));\n        S.NextKm(1);";
        let extra =
            "\n    local procedure Foo(X: Interface \"Demo Strategy\")\n    begin\n    end;";
        assert!(is_open(completeness(&workspace(
            caller(VARS, body, extra),
            &[]
        ))));
    }

    /// T23: `S` as a `foreach` variable.
    #[test]
    fn i264_t23_foreach_variable_stays_open() {
        let vars = format!("{VARS}\n        L: List of [Interface \"Demo Strategy\"];");
        let body = "        S := K;\n        foreach S in L do\n            S.NextKm(1);";
        let ws = workspace(caller(&vars, body, ""), &[]);
        // Two interface sites would break the one-site precondition: count them.
        let report = crate::program::resolve::full::resolve_full_program(write(&ws).path())
            .expect("resolves");
        let sites: Vec<_> = report
            .edges
            .iter()
            .filter(|c| {
                c.edge.from.name_lc.as_ref() == "dispatch"
                    && c.edge.shape == DispatchShape::Polymorphic
            })
            .collect();
        assert_eq!(sites.len(), 1, "precondition: one interface site");
        assert!(is_open(sites[0].edge.completeness));
    }

    /// T24: any `with` in the routine.
    #[test]
    fn i264_t24_any_with_stays_open() {
        let vars = format!("{VARS}\n        Rec: Record \"Demo Item\";");
        let body = "        S := K;\n        with Rec do begin\n        end;\n        S.NextKm(1);";
        assert!(is_open(completeness(&workspace(
            caller(&vars, body, ""),
            &[]
        ))));
    }

    /// T25: a second, QUOTED write of `S` from a non-enum value.
    #[test]
    fn i264_t25_quoted_write_stays_open() {
        let vars = format!("{VARS}\n        SomeCu: Codeunit \"Demo Impl\";");
        let body = "        S := K;\n        \"S\" := SomeCu;\n        S.NextKm(1);";
        assert!(is_open(completeness(&workspace(
            caller(&vars, body, ""),
            &[]
        ))));
    }

    /// T26: a clean file whose routine nests blocks past the lowerer's depth
    /// cap, so a statement lowers as `StmtKind::Unknown`.
    #[test]
    fn i264_t26_unknown_statement_stays_open() {
        let depth = 140;
        let nested = format!("{}{}", "begin ".repeat(depth), "end; ".repeat(depth));
        let body = format!("        S := K;\n        {nested}\n        S.NextKm(1);");
        let text = caller(VARS, &body, "");
        let parsed = al_syntax::parse(&text);
        assert_eq!(
            parsed.parse_status,
            al_syntax::ir::ParseStatus::Clean,
            "precondition"
        );
        assert!(
            parsed
                .ir
                .iter_stmts()
                .any(|s| matches!(s.kind, al_syntax::ir::StmtKind::Unknown)),
            "precondition: an Unknown statement"
        );
        assert!(is_open(completeness(&workspace(text, &[]))));
    }

    /// T18: a syntax error elsewhere in the caller's file.
    #[test]
    fn i264_t18_recovered_caller_file_stays_open() {
        let extra = "\n    procedure Broken()\n    begin\n        if then ;\n    end;";
        let text = caller(VARS, "        S := K;\n        S.NextKm(1);", extra);
        assert_eq!(
            al_syntax::parse(&text).parse_status,
            al_syntax::ir::ParseStatus::Recovered,
            "precondition"
        );
        assert!(is_open(completeness(&workspace(text, &[]))));
    }

    /// T11: a second value maps an implementer that lacks the method.
    #[test]
    fn i264_t11_implementer_without_the_method_stays_open() {
        let bare = "codeunit 50107 \"Bare Impl\" implements \"Demo Strategy\"\n{\n}\n".to_string();
        let kind_text = kind(
            50102,
            "Demo Kind",
            "    Extensible = false;",
            "    value(0; Standard) { Implementation = \"Demo Strategy\" = \"Demo Impl\"; }\n    value(1; Bare) { Implementation = \"Demo Strategy\" = \"Bare Impl\"; }",
        );
        let ws = workspace(
            caller(VARS, "        S := K;\n        S.NextKm(1);", ""),
            &[("src/Bare.al", bare), ("src/Kind.al", kind_text)],
        );
        assert!(is_open(completeness(&ws)));
    }

    /// Spec §4 "every route is real": the enum maps only `Demo Impl` (covered),
    /// but another implementer of I lacks the method, so the edge carries an
    /// `Unknown` route. T11's own shape is also caught by §2 coverage; this one
    /// is caught only by the real-route check.
    #[test]
    fn i264_unknown_route_on_the_edge_stays_open() {
        let bare = "codeunit 50107 \"Bare Impl\" implements \"Demo Strategy\"\n{\n}\n".to_string();
        let ws = workspace(
            caller(VARS, "        S := K;\n        S.NextKm(1);", ""),
            &[("src/Bare.al", bare)],
        );
        assert!(is_open(completeness(&ws)));
    }

    /// T20: a value maps a codeunit the program does not hold.
    #[test]
    fn i264_t20_unloaded_mapped_implementation_stays_open() {
        let kind_text = kind(
            50102,
            "Demo Kind",
            "    Extensible = false;",
            "    value(0; Standard) { Implementation = \"Demo Strategy\" = \"Demo Impl\"; }\n    value(1; Gone) { Implementation = \"Demo Strategy\" = \"Missing Impl\"; }",
        );
        let ws = workspace(
            caller(VARS, "        S := K;\n        S.NextKm(1);", ""),
            &[("src/Kind.al", kind_text)],
        );
        assert!(is_open(completeness(&ws)));
    }

    /// Task 2 review (namespace): the enum maps `Ns2."Demo Impl"`, and only a
    /// same-named `Demo Impl` in namespace `Ns1` is loaded. Name resolution
    /// would land on the wrong codeunit, so the site stays open.
    #[test]
    fn i264_namespace_qualified_implementation_of_another_namespace_stays_open() {
        let impl_ns1 = format!("namespace Ns1;\n\n{}", implementer(50101, "Demo Impl"));
        let kind_text = demo_kind().replace(r#"= "Demo Impl""#, r#"= Ns2."Demo Impl""#);
        assert!(
            kind_text.contains("Ns2."),
            "precondition: the reference is qualified"
        );
        let ws = workspace(
            caller(VARS, "        S := K;\n        S.NextKm(1);", ""),
            &[("src/Impl.al", impl_ns1), ("src/Kind.al", kind_text)],
        );
        assert!(is_open(completeness(&ws)));
    }

    /// T32 (route half): `DefaultImplementation` and `UnknownValueImplementation`
    /// are required routes; with both loaded the site closes, with the default
    /// codeunit missing it stays open.
    #[test]
    fn i264_t32_default_and_unknown_implementations_are_required_routes() {
        let kind_with = |default: &str| {
            kind(
                50102,
                "Demo Kind",
                &format!(
                    "    Extensible = false;\n    DefaultImplementation = \"Demo Strategy\" = \"{default}\";\n    UnknownValueImplementation = \"Demo Strategy\" = \"Demo Unknown\";"
                ),
                "    value(0; Standard) { Implementation = \"Demo Strategy\" = \"Demo Impl\"; }\n    value(1; Other) { }",
            )
        };
        let body = "        S := K;\n        S.NextKm(1);";
        let more = |default: &str| {
            vec![
                ("src/Default.al", implementer(50108, "Demo Default")),
                ("src/Unknown.al", implementer(50109, "Demo Unknown")),
                ("src/Kind.al", kind_with(default)),
            ]
        };
        let present = workspace(caller(VARS, body, ""), &more("Demo Default"));
        assert_eq!(completeness(&present), SetCompleteness::Complete);
        let missing = workspace(caller(VARS, body, ""), &more("Missing Default"));
        assert!(is_open(completeness(&missing)));
    }

    /// RF2: declaration, write and call spell the variable differently.
    #[test]
    fn i264_rf2_case_and_quote_variants_are_one_variable() {
        let vars = "        Strategy: Interface \"Demo Strategy\";\n        K: Enum \"Demo Kind\";\n        SomeCu: Codeunit \"Demo Impl\";";
        let closed = "        strategy := K;\n        \"Strategy\".NextKm(1);";
        let ws = workspace(caller(vars, closed, ""), &[]);
        assert_eq!(completeness(&ws), SetCompleteness::Complete);
        let open =
            "        Strategy := K;\n        \"strategy\" := SomeCu;\n        Strategy.NextKm(1);";
        assert!(is_open(completeness(&workspace(
            caller(vars, open, ""),
            &[]
        ))));
    }

    /// RF3: the call inside a condition inside a loop is an admitted occurrence.
    #[test]
    fn i264_rf3_member_call_in_condition_stays_closed() {
        let body = "        S := K;\n        repeat\n            if S.NextKm(1) > 0 then\n                exit;\n        until true;";
        let ws = workspace(caller(VARS, body, ""), &[]);
        assert_eq!(completeness(&ws), SetCompleteness::Complete);
    }

    /// T6b (locality): `S` is a PARAMETER written from a closed enum. A
    /// parameter is not routine-local (spec §3 rule 2), so the site stays open.
    #[test]
    fn i264_t6b_assigned_parameter_stays_open() {
        let text = "codeunit 50103 \"Caller\"\n{\n    procedure Dispatch(S: Interface \"Demo Strategy\")\n    var\n        K: Enum \"Demo Kind\";\n    begin\n        S := K;\n        S.NextKm(1);\n    end;\n}\n".to_string();
        assert!(is_open(completeness(&workspace(text, &[]))));
    }

    /// Locality: an object global of the same name as the local.
    #[test]
    fn i264_global_of_the_same_name_stays_open() {
        let text = caller(VARS, "        S := K;\n        S.NextKm(1);", "").replace(
            "{\n    procedure Dispatch()",
            "{\n    var\n        S: Interface \"Demo Strategy\";\n\n    procedure Dispatch()",
        );
        assert!(
            text.contains("    var\n        S:"),
            "precondition: the global exists"
        );
        assert!(is_open(completeness(&workspace(text, &[]))));
    }

    /// No write at all: the variable is only called.
    #[test]
    fn i264_unassigned_local_stays_open() {
        let ws = workspace(caller(VARS, "        S.NextKm(1);", ""), &[]);
        assert!(is_open(completeness(&ws)));
    }

    /// T19: a SOURCE enum mapping a SYMBOL-ONLY implementer from a dependency;
    /// the route is an `AbiSymbol` and its object counts as reached.
    #[test]
    fn i264_t19_symbol_only_implementer_of_a_source_enum_is_complete() {
        const DEP_GUID: &str = "26426426-0000-0000-0000-0000000000df";
        let app_json = format!(
            r#"{{"id":"26426426-0000-0000-0000-000000000004","name":"Ws264b","publisher":"probe","version":"1.0.0.0","runtime":"14.0","idRanges":[{{"from":50100,"to":50199}}],"dependencies":[{{"id":"{DEP_GUID}","name":"Dep264b","publisher":"probe","version":"1.0.0.0"}}]}}"#
        );
        let kind_text = r#"enum 50102 "Ws Kind" implements "Dep Strategy"
{
    Extensible = false;
    value(0; FromDep) { Implementation = "Dep Strategy" = "Dep Impl"; }
}
"#;
        let caller_text = caller(
            "        S: Interface \"Dep Strategy\";\n        K: Enum \"Ws Kind\";",
            "        S := K;\n        S.NextKm(1);",
            "",
        );
        let files: Vec<(String, String)> = vec![
            ("app.json".into(), app_json),
            ("src/Kind.al".into(), kind_text.into()),
            ("src/Caller.al".into(), caller_text),
        ];
        let dir = write(&files);
        let symbols = r#"{"RuntimeVersion":"14.0","Interfaces":[{"Name":"Dep Strategy","Methods":[{"Name":"NextKm","Parameters":[{"Name":"CurrentKm","TypeDefinition":{"Name":"Integer"}}],"ReturnTypeDefinition":{"Name":"Integer"}}]}],"Codeunits":[{"Id":70001,"Name":"Dep Impl","ImplementedInterfaces":["Dep Strategy"],"Methods":[{"Name":"NextKm","Parameters":[{"Name":"CurrentKm","TypeDefinition":{"Name":"Integer"}}],"ReturnTypeDefinition":{"Name":"Integer"}}]}]}"#;
        let manifest = format!(
            r#"<?xml version="1.0" encoding="utf-8"?><Package xmlns="http://schemas.microsoft.com/navx/2015/manifest"><App Id="{DEP_GUID}" Name="Dep264b" Publisher="probe" Version="1.0.0.0" Runtime="14.0" /></Package>"#
        );
        let app = crate::engine::deps::app_package_zip::test_apps::build_app(&[
            ("NavxManifest.xml", manifest.as_bytes()),
            ("SymbolReference.json", symbols.as_bytes()),
        ]);
        let pkgs = dir.path().join(".alpackages");
        std::fs::create_dir_all(&pkgs).unwrap();
        std::fs::write(pkgs.join("probe_Dep264b_1.0.0.0.app"), app).unwrap();

        let report =
            crate::program::resolve::full::resolve_full_program(dir.path()).expect("resolves");
        let site: Vec<_> = report
            .edges
            .iter()
            .filter(|c| {
                c.edge.from.name_lc.as_ref() == "dispatch"
                    && c.edge.shape == DispatchShape::Polymorphic
            })
            .collect();
        assert_eq!(site.len(), 1, "precondition: one interface site");
        assert!(
            site[0].edge.routes.iter().any(|r| matches!(
                r.target,
                crate::program::resolve::edge::RouteTarget::AbiSymbol { .. }
            )),
            "precondition: an AbiSymbol route, {:#?}",
            site[0].edge.routes
        );
        assert_eq!(site[0].edge.completeness, SetCompleteness::Complete);
    }

    /// T34 (opus N3): an enum whose `enum_impls` hold no entry for one of the
    /// interfaces it implements is not covered, even with every route present.
    /// Stated precondition: the node's fields are set by assignment.
    #[test]
    fn enum_covered_by_routes_requires_each_implemented_interface() {
        let ws = workspace(
            caller(VARS, "        S := K;\n        S.NextKm(1);", ""),
            &[],
        );
        let dir = write(&ws);
        let ctx = crate::program::resolve::full::build_context_res(dir.path()).expect("context");
        let graph = ctx.graph();
        let index = super::ResolveIndex::build(graph);
        let report = crate::program::resolve::full::resolve_full_program_with(&ctx);
        let routes = &report
            .edges
            .iter()
            .find(|c| c.edge.shape == DispatchShape::Polymorphic)
            .expect("the interface site")
            .edge
            .routes;
        let mut node = graph
            .objects
            .iter()
            .find(|o| o.name.as_ref() == "Demo Kind")
            .expect("the enum")
            .clone();
        assert!(node.closed_enum, "precondition: the real fact is closed");
        assert!(
            super::enum_covered_by_routes(graph, &index, &node, "demo strategy", routes),
            "precondition: covered as extracted"
        );
        node.implements.push("Other Strategy".into());
        assert!(!super::enum_covered_by_routes(
            graph,
            &index,
            &node,
            "demo strategy",
            routes
        ));
    }
}

//! The one table a local `RecordRef` can hold (round-2 fact check, call c02).
//!
//! `Codeunit "Upgrade - BaseApp"::UpgradeCustomerTemplates` opens a local
//! RecordRef on a literal table (`TemplateRecordRef.Open(Database::"Customer
//! Templ.")`, `UpgradeBaseApp.Codeunit.al:2089`), binds a record of the same
//! table (`GetTable(CustomerTempl)`, :2090) and calls `TemplateRecordRef.Modify()`
//! (:2100). The engine had no table for the RecordRef, so the operation was an
//! open record hub that "reached" every table's `OnModify`, and `prove` named
//! it as a hub into `Table "CAL Test Line"::OnModify`.
//!
//! A RecordRef holds the table its last `Open` or `GetTable` bound. When every
//! binding a LOCAL RecordRef gets anywhere in its routine names the same table,
//! and nothing else can rebind it, it holds that table at every operation that
//! can run (an operation before any binding fails at run time: no table). That
//! table's operations, triggers and table events are then the operation's, as
//! for a record variable of the table. [`local_recordref_bindings`] says when
//! that is provable:
//!
//! - the variable is a local of type `RecordRef` (a parameter or an object
//!   global may be bound by a caller or another routine);
//! - it appears only as the receiver of a member (`R.Open(…)`, `R.Modify()`,
//!   `R.Field(…)`, `R.Number`): never assigned, passed as an argument (a `var`
//!   parameter could re-open it), assigned to another RecordRef (one record
//!   reference), or used as a `with` receiver;
//! - each binding is `R.Open(<table literal>[, …])` (`Database::"T"`,
//!   `Database::T`, `Database::<number>`, or a positive integer) or
//!   `R.GetTable(<record variable>)`; `R.Copy(…)` and any other binding form,
//!   and an expression the IR does not model, decline.
//!
//! The callers resolve each binding to a table in their own terms (the
//! program graph in `resolve::full`, the model's symbols in `record_types`)
//! and use it only when all bindings resolve to the same table.

use al_syntax::IdentifierFoldExt;
use al_syntax::ir::{AlFile, BlockId, BlockItem, ExprId, ExprKind, Literal, RoutineDecl, StmtKind};

/// One way a RecordRef is bound to a table.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Binding {
    /// `Open(<literal>)`: the table's name (unquoted) or number.
    Table { reference: String, is_name: bool },
    /// `GetTable(<variable>)`: the record variable's name (unquoted), or
    /// `Rec`/`xRec`.
    GetTable(String),
}

struct Walk<'a> {
    file: &'a AlFile,
    var: String,
    bindings: Vec<Binding>,
    /// FieldRef mode ([`local_fieldref_sources`]): `var := R.Field(…)` is a
    /// source, collected here; every other use of `var` is as in RecordRef mode.
    field_sources: Option<Vec<FieldSource>>,
    ok: bool,
}

impl Walk<'_> {
    fn names_var(&self, e: ExprId) -> bool {
        match &self.file.ir.expr(e).kind {
            ExprKind::Identifier(n) | ExprKind::QuotedIdentifier(n) => {
                n.eq_fold_identifier(&self.var)
            }
            _ => false,
        }
    }

    fn block(&mut self, b: BlockId) {
        for item in &self.file.ir.block(b).items {
            if !self.ok {
                return;
            }
            match item {
                BlockItem::Stmt(s) => self.stmt(*s),
                BlockItem::Preproc(g) => {
                    for br in &g.branches {
                        self.block(*br);
                    }
                }
            }
        }
    }

    fn stmt(&mut self, s: al_syntax::ir::StmtId) {
        match &self.file.ir.stmt(s).kind {
            StmtKind::Assignment { target, value, .. } => {
                if self.field_sources.is_some() && self.names_var(*target) {
                    match field_call(self.file, *value) {
                        Some(src) => {
                            if let Some(s) = self.field_sources.as_mut() {
                                s.push(src);
                            }
                        }
                        None => self.ok = false,
                    }
                } else {
                    self.expr(*target);
                }
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
                if let Some(b) = else_block {
                    self.block(*b);
                }
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
                if let Some(b) = else_block {
                    self.block(*b);
                }
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
                if let Some(b) = catch_block {
                    self.block(*b);
                }
            }
            StmtKind::AssertError(b) | StmtKind::Block(b) => self.block(*b),
            StmtKind::Exit(e) => {
                if let Some(e) = e {
                    self.expr(*e);
                }
            }
            StmtKind::Break | StmtKind::Continue => {}
            // A statement the IR does not model may hold anything.
            StmtKind::Unknown => self.ok = false,
        }
    }

    fn expr(&mut self, e: ExprId) {
        if !self.ok {
            return;
        }
        let file = self.file;
        match &file.ir.expr(e).kind {
            ExprKind::Identifier(_) | ExprKind::QuotedIdentifier(_) => {
                // A use of the variable that is not a member receiver.
                if self.names_var(e) {
                    self.ok = false;
                }
            }
            ExprKind::Call { function, args } => {
                if let ExprKind::Member { object, member, .. } = &file.ir.expr(*function).kind
                    && self.names_var(*object)
                {
                    self.method(member, args);
                } else {
                    self.expr(*function);
                }
                for a in args {
                    self.expr(*a);
                }
            }
            ExprKind::Member { object, member, .. } => {
                if self.names_var(*object) {
                    // A parens-less member (`R.Number`, `R.Close`): a binding
                    // always takes an argument.
                    let m = member.trim().trim_matches('"').to_ascii_lowercase();
                    if self.field_sources.is_none()
                        && matches!(m.as_str(), "open" | "gettable" | "copy")
                    {
                        self.ok = false;
                    }
                } else {
                    self.expr(*object);
                }
            }
            ExprKind::Unknown => self.ok = false,
            k => {
                for c in k.children() {
                    self.expr(c);
                }
            }
        }
    }

    fn method(&mut self, member: &str, args: &[ExprId]) {
        if self.field_sources.is_some() {
            // A FieldRef's own methods (`Validate`, `Value`, …) never rebind it.
            return;
        }
        let m = member.trim().trim_matches('"').to_ascii_lowercase();
        match m.as_str() {
            "open" => match args.first().and_then(|a| table_literal(self.file, *a)) {
                Some(b) => self.bindings.push(b),
                None => self.ok = false,
            },
            "gettable" => {
                let name = args
                    .first()
                    .and_then(|a| match &self.file.ir.expr(*a).kind {
                        ExprKind::Identifier(n) | ExprKind::QuotedIdentifier(n) => Some(n.clone()),
                        _ => None,
                    });
                match name {
                    Some(n) if !n.eq_fold_identifier(&self.var) => {
                        self.bindings.push(Binding::GetTable(n));
                    }
                    _ => self.ok = false,
                }
            }
            // Another RecordRef's table, by reference or by copy.
            "copy" => self.ok = false,
            _ => {}
        }
    }
}

/// `Database::"T"`, `Database::T`, `Database::<n>` or a positive integer.
fn table_literal(file: &AlFile, e: ExprId) -> Option<Binding> {
    match &file.ir.expr(e).kind {
        ExprKind::DatabaseReference(text) => {
            let (kind, rest) = text.split_once("::")?;
            if !kind.trim().eq_ignore_ascii_case("database") {
                return None;
            }
            let rest = rest.trim();
            if rest.starts_with('"') {
                let name =
                    crate::program::body::node_util::strip_quotes(rest).replace("\"\"", "\"");
                Some(Binding::Table {
                    reference: name,
                    is_name: true,
                })
            } else if let Ok(n) = rest.parse::<i64>() {
                (n > 0).then(|| Binding::Table {
                    reference: n.to_string(),
                    is_name: false,
                })
            } else {
                Some(Binding::Table {
                    reference: rest.to_string(),
                    is_name: true,
                })
            }
        }
        ExprKind::Literal(Literal::Int(n)) => {
            let n: i64 = n.trim().parse().ok()?;
            (n > 0).then(|| Binding::Table {
                reference: n.to_string(),
                is_name: false,
            })
        }
        _ => None,
    }
}

/// The bindings of local RecordRef `var` (as written; quotes allowed) in
/// routine `r`, sorted and deduplicated, when they are provably the only ones
/// (see the module doc); `None` otherwise, and for no binding at all.
#[must_use]
pub fn local_recordref_bindings(file: &AlFile, r: &RoutineDecl, var: &str) -> Option<Vec<Binding>> {
    let name = crate::program::body::node_util::strip_quotes(var.trim()).replace("\"\"", "\"");
    if r.params.iter().any(|p| p.name.eq_fold_identifier(&name)) {
        return None;
    }
    let local = r.locals.iter().find(|v| v.name.eq_fold_identifier(&name))?;
    if !local
        .ty
        .as_deref()
        .is_some_and(|t| t.trim().eq_ignore_ascii_case("recordref"))
    {
        return None;
    }
    let mut w = Walk {
        file,
        var: name,
        bindings: Vec::new(),
        field_sources: None,
        ok: true,
    };
    w.block(r.body?);
    if !w.ok || w.bindings.is_empty() {
        return None;
    }
    w.bindings.sort();
    w.bindings.dedup();
    Some(w.bindings)
}

/// How the argument of a `RecRef.Field(…)` names its field.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum FieldArg {
    /// An integer literal: the field's number.
    Number(i64),
    /// A text literal (`'No.'`): the field's name (unquoted, as written).
    Name(String),
    /// `Rec.FieldNo(F)`: the number of field `F` of `Rec`'s table, which need
    /// not be the RecordRef's (`record` and `field` as written, unquoted).
    FieldNo { record: String, field: String },
    /// `FieldIndex(…)`, a computed number, or anything else.
    Unknown,
}

/// One `RecRef.Field(…)` a FieldRef can come from.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FieldSource {
    /// The RecordRef, as written (an identifier).
    pub recordref: String,
    pub field: FieldArg,
}

/// `R.Field(<arg>)` or `R.FieldIndex(<arg>)` with `R` an identifier.
fn field_call(file: &AlFile, e: ExprId) -> Option<FieldSource> {
    let ExprKind::Call { function, args } = &file.ir.expr(e).kind else {
        return None;
    };
    let ExprKind::Member { object, member, .. } = &file.ir.expr(*function).kind else {
        return None;
    };
    let recordref = match &file.ir.expr(*object).kind {
        ExprKind::Identifier(n) | ExprKind::QuotedIdentifier(n) => n.clone(),
        _ => return None,
    };
    let field = match member
        .trim()
        .trim_matches('"')
        .to_ascii_lowercase()
        .as_str()
    {
        "field" => args
            .first()
            .map_or(FieldArg::Unknown, |a| field_arg(file, *a)),
        "fieldindex" => FieldArg::Unknown,
        _ => return None,
    };
    Some(FieldSource { recordref, field })
}

/// [`FieldArg`] of one `Field(…)` argument.
fn field_arg(file: &AlFile, e: ExprId) -> FieldArg {
    match &file.ir.expr(e).kind {
        ExprKind::Literal(Literal::Int(n)) => n
            .trim()
            .parse::<i64>()
            .map_or(FieldArg::Unknown, FieldArg::Number),
        ExprKind::Literal(Literal::Text(raw)) => {
            al_syntax::ir::al_string_literal_text(raw).map_or(FieldArg::Unknown, FieldArg::Name)
        }
        // `Rec.FieldNo(F)` / `Rec.FieldNo("F")`: field `F` of `Rec`.
        ExprKind::Call { function, args } => {
            let ExprKind::Member { object, member, .. } = &file.ir.expr(*function).kind else {
                return FieldArg::Unknown;
            };
            if !member
                .trim()
                .trim_matches('"')
                .eq_ignore_ascii_case("fieldno")
            {
                return FieldArg::Unknown;
            }
            let name = |e: ExprId| match &file.ir.expr(e).kind {
                ExprKind::Identifier(n) | ExprKind::QuotedIdentifier(n) => Some(n.clone()),
                _ => None,
            };
            match (name(*object), args.as_slice()) {
                (Some(record), [a]) => match name(*a) {
                    Some(field) => FieldArg::FieldNo { record, field },
                    None => FieldArg::Unknown,
                },
                _ => FieldArg::Unknown,
            }
        }
        _ => FieldArg::Unknown,
    }
}

/// The `RecRef.Field(…)` calls a FieldRef receiver `recv` (an expression of
/// routine `r`) can be, when that is provable; `None` otherwise. A
/// `R.Field(…)` written in place is itself. A LOCAL `FieldRef` variable is
/// every `V := R.Field(…)` / `R.FieldIndex(…)` assigned to it, provided it
/// is used otherwise only as a member receiver (`V.Validate(…)`, `V.Value`):
/// passed as an argument (a `var` parameter could rebind it), assigned
/// anything else, a parameter or a global declines, as for a RecordRef
/// ([`local_recordref_bindings`], the module doc).
#[must_use]
pub fn fieldref_sources(file: &AlFile, r: &RoutineDecl, recv: ExprId) -> Option<Vec<FieldSource>> {
    if let Some(src) = field_call(file, recv) {
        return Some(vec![src]);
    }
    let name = match &file.ir.expr(recv).kind {
        ExprKind::Identifier(n) | ExprKind::QuotedIdentifier(n) => n.clone(),
        _ => return None,
    };
    local_fieldref_sources(file, r, &name)
}

/// [`fieldref_sources`] for a local FieldRef variable `var`, sorted and
/// deduplicated; `None` when not provable or never assigned.
#[must_use]
pub fn local_fieldref_sources(
    file: &AlFile,
    r: &RoutineDecl,
    var: &str,
) -> Option<Vec<FieldSource>> {
    let name = crate::program::body::node_util::strip_quotes(var.trim()).replace("\"\"", "\"");
    if r.params.iter().any(|p| p.name.eq_fold_identifier(&name)) {
        return None;
    }
    let local = r.locals.iter().find(|v| v.name.eq_fold_identifier(&name))?;
    if !local
        .ty
        .as_deref()
        .is_some_and(|t| t.trim().eq_ignore_ascii_case("fieldref"))
    {
        return None;
    }
    let mut w = Walk {
        file,
        var: name,
        bindings: Vec::new(),
        field_sources: Some(Vec::new()),
        ok: true,
    };
    w.block(r.body?);
    let mut sources = w.field_sources.take().unwrap_or_default();
    if !w.ok || sources.is_empty() {
        return None;
    }
    sources.sort();
    sources.dedup();
    Some(sources)
}

/// [`local_recordref_bindings`] for every local RecordRef of `r` that has
/// provable bindings: `(folded name, bindings)`, sorted by name.
#[must_use]
pub fn local_recordref_bindings_all(file: &AlFile, r: &RoutineDecl) -> Vec<(String, Vec<Binding>)> {
    let mut out: Vec<(String, Vec<Binding>)> = r
        .locals
        .iter()
        .filter(|v| {
            v.ty.as_deref()
                .is_some_and(|t| t.trim().eq_ignore_ascii_case("recordref"))
        })
        .filter_map(|v| {
            local_recordref_bindings(file, r, &v.name).map(|b| (v.name.fold_identifier(), b))
        })
        .collect();
    out.sort();
    out.dedup_by(|a, b| a.0 == b.0);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bindings(body: &str) -> Option<Vec<Binding>> {
        let src = format!(
            "codeunit 50100 X\n{{\n    procedure P(Param: RecordRef)\n    var\n        R: RecordRef;\n        Other: RecordRef;\n        Cust: Record Customer;\n    begin\n{body}\n    end;\n}}\n"
        );
        let f = al_syntax::parse(&src);
        let r = &f.objects[0].routines[0];
        local_recordref_bindings(&f, r, "R")
    }

    fn table(name: &str) -> Binding {
        Binding::Table {
            reference: name.to_string(),
            is_name: true,
        }
    }

    #[test]
    fn literal_open_and_gettable_bind() {
        assert_eq!(
            bindings("R.Open(Database::\"Customer Templ.\");\nR.Modify();\nR.Close();"),
            Some(vec![table("Customer Templ.")])
        );
        assert_eq!(
            bindings(
                "R.Open(Database::Customer);\nR.GetTable(Cust);\nif R.Number = 18 then R.Modify();"
            ),
            Some(vec![table("Customer"), Binding::GetTable("Cust".into())])
        );
        assert_eq!(
            bindings("R.Open(18);"),
            Some(vec![Binding::Table {
                reference: "18".into(),
                is_name: false
            }])
        );
        // A field read through the reference does not rebind it.
        assert_eq!(
            bindings("R.Open(Database::Customer);\nR.Field(1).Value := 'x';\nR.Insert();"),
            Some(vec![table("Customer")])
        );
    }

    #[test]
    fn anything_that_may_rebind_declines() {
        for body in [
            // A computed table.
            "R.Open(Param.Number);",
            // Passed on: a callee may re-open it.
            "R.Open(Database::Customer);\nDoIt(R);",
            // Assigned, or aliased.
            "R := Param;",
            "R.Open(Database::Customer);\nOther := R;\nOther.Open(Database::Vendor);",
            "R.Open(Database::Customer);\nR.Copy(Param);",
            // Never bound.
            "R.Modify();",
            // Not a table literal.
            "R.Open(Codeunit::\"X\");",
            "R.Open(0);",
        ] {
            assert_eq!(bindings(body), None, "{body}");
        }
        // Two tables: provable bindings, the caller sees they differ.
        assert_eq!(
            bindings("R.Open(Database::Customer);\nR.Close();\nR.Open(Database::Vendor);"),
            Some(vec![table("Customer"), table("Vendor")])
        );
    }

    #[test]
    fn only_a_local_recordref() {
        let src = "codeunit 50100 X\n{\n    var\n        G: RecordRef;\n    procedure P(Param: RecordRef)\n    var\n        C: Record Customer;\n    begin\n        Param.Open(Database::Customer);\n        G.Open(Database::Customer);\n    end;\n}\n";
        let f = al_syntax::parse(src);
        let r = &f.objects[0].routines[0];
        assert_eq!(local_recordref_bindings(&f, r, "Param"), None);
        assert_eq!(local_recordref_bindings(&f, r, "G"), None);
        assert_eq!(local_recordref_bindings(&f, r, "C"), None);
    }
}

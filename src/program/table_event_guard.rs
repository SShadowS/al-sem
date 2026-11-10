//! Table-event subscribers that cannot run in their raising context (round-2
//! fact check, calls c12, c20, n09, n10 of the 2026-11-06 output validation).
//!
//! A record operation raises its table's database trigger events
//! (`OnBefore/OnAfter{Insert,Modify,Delete,Rename}Event`; spec
//! `docs/superpowers/specs/2026-11-06-db-event-subscribers.md`). Many
//! subscribers begin by leaving at once in two situations the raising site
//! may already decide:
//!
//! - `if Rec.IsTemporary() then exit;` — the raising record is temporary;
//! - `if not RunTrigger then exit;` — the operation passed RunTrigger false.
//!
//! When the raising site proves the situation (its record is declared
//! temporary; its RunTrigger argument is `false` or left out), that subscriber
//! runs only its guard and exits: the dispatch is dead in that context. Paths
//! must not use it and effects must not count it. Where the site does not
//! decide (a record whose temp state the caller decides, a RunTrigger
//! expression), the dispatch stays.
//!
//! # The Business Central facts this relies on
//!
//! 1. **Table events are raised for temporary records.** Microsoft Learn is
//!    silent on it ("Event types", database trigger events; the
//!    `OnAfterInsertEvent` page; "Temporary tables"). The evidence is
//!    Microsoft's own code: 151 of the 515 database trigger event
//!    subscribers in the Base Application of the CDO baseline's `.alpackages`
//!    check `if Rec.IsTemporary[()] then` near their top (e.g.
//!    `WorkflowEventHandling.Codeunit.al:712-713`,
//!    `OfficeDocumentHandler.Codeunit.al:486-487`), which only makes sense if
//!    the event fires for a temporary record. The F1 spec (§2) records the
//!    same. So the edge to such a subscriber is real; it is its guard that
//!    stops it.
//! 2. **RunTrigger does not decide whether the events are raised.** Microsoft
//!    Learn ("Event types", order of event execution: trigger event before,
//!    table trigger, global table trigger, database operation, trigger event
//!    after) lists the events beside the table trigger, and hands the flag to
//!    the subscriber as its `RunTrigger` parameter. Subscribers branch on it
//!    (`if not RunTrigger then exit;`: 10 in the CDO baseline and its
//!    dependency source, e.g. `CDODataDeleteHandler.Codeunit.al:137-138`),
//!    which only makes sense if the event fires with `RunTrigger = false`.
//!    `Insert()`, `Modify()`, `Delete()`, `DeleteAll()`, `ModifyAll(F, V)`
//!    pass `false` (the argument's default; measured on BC 28 for the table
//!    trigger, `applicability::TriggerSiteRule`).
//! 3. **`DeleteAll` and `ModifyAll` raise them once per record.** Microsoft
//!    Learn ("Performance articles for developers", limit your event
//!    subscriptions): table events force `ModifyAll` and `DeleteAll`, which
//!    normally do one bulk SQL statement, into single-row operations; each row
//!    raises the events with the call's RunTrigger. `Record.DeleteAll`:
//!    "Setting `RunTrigger` to false only affects the `OnDelete` trigger."
//!
//! Nothing here was probed on a live BC: the kraken BC sidecar is reachable
//! only from server1, and this change was made on a developer laptop.
//!
//! # The guard shape
//!
//! [`subscriber_entry_exits`] reads the subscriber's LEADING statements while
//! each is `if <cond> then exit` (no `else`, the `then` branch only `exit`),
//! where `<cond>` is one atom or an `or` of atoms, each:
//!
//! - `Rec.IsTemporary` / `Rec.IsTemporary()` ([`EntryExits::TEMPORARY_REC`]);
//! - `not RunTrigger` / `RunTrigger = false` ([`EntryExits::NO_RUN_TRIGGER`]).
//!
//! `Rec` and `RunTrigger` must be the routine's own parameters (an event
//! subscriber's parameters bind to the event's by name). Any other atom ends
//! the reading: AL evaluates every operand of `or`, so a call beside the test
//! (`if Rec.IsTemporary or Mgt.AreSubscribersDisabled() then exit`) still
//! runs, and that subscriber is not dead. Every recognized statement has no
//! effect but its exit, so reading past one is sound.

use al_syntax::IdentifierFoldExt;
use al_syntax::ir::{
    AlFile, BinaryOp, BlockItem, ExprId, ExprKind, Literal, RoutineDecl, StmtKind, UnaryOp,
};

/// What a subscriber exits on before doing anything: a bit set.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct EntryExits(u8);

impl EntryExits {
    /// Exits when `Rec` is temporary.
    pub const TEMPORARY_REC: EntryExits = EntryExits(1);
    /// Exits when the operation passed `RunTrigger = false`.
    pub const NO_RUN_TRIGGER: EntryExits = EntryExits(2);
    pub const NONE: EntryExits = EntryExits(0);

    #[must_use]
    pub fn bits(self) -> u8 {
        self.0
    }

    #[must_use]
    pub fn from_bits(b: u8) -> EntryExits {
        EntryExits(b & 3)
    }

    #[must_use]
    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    #[must_use]
    pub fn contains(self, o: EntryExits) -> bool {
        self.0 & o.0 == o.0 && o.0 != 0
    }

    #[must_use]
    pub fn union(self, o: EntryExits) -> EntryExits {
        EntryExits(self.0 | o.0)
    }

    #[must_use]
    pub fn intersection(self, o: EntryExits) -> EntryExits {
        EntryExits(self.0 & o.0)
    }

    #[must_use]
    pub fn intersects(self, o: EntryExits) -> bool {
        self.0 & o.0 != 0
    }
}

/// What a raising site proves about the dispatch, in [`EntryExits`] terms:
/// the situations a subscriber exiting on them meets for sure. A subscriber
/// whose [`EntryExits`] intersects it runs only its guard from this site.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct RaiseContext {
    /// The raising record is temporary on every path through the site.
    pub record_temporary: bool,
    /// The operation passes `RunTrigger = false` (a literal, or left out).
    pub run_trigger_false: bool,
    /// The operation passes a literal `RunTrigger = true`. Not a reason to
    /// drop anything: with it, a `not RunTrigger` guard provably passes.
    pub run_trigger_true: bool,
}

impl RaiseContext {
    /// The situations as bits ([`EntryExits`] layout).
    #[must_use]
    pub fn as_exits(self) -> EntryExits {
        let mut b = 0;
        if self.record_temporary {
            b |= EntryExits::TEMPORARY_REC.0;
        }
        if self.run_trigger_false {
            b |= EntryExits::NO_RUN_TRIGGER.0;
        }
        EntryExits(b)
    }

    /// Whether a subscriber exiting on `exits` runs only its guard here.
    #[must_use]
    pub fn kills(self, exits: EntryExits) -> bool {
        self.as_exits().intersects(exits)
    }

    /// Whether a subscriber exiting on `exits`, not killed here
    /// ([`Self::kills`]), may still exit at its guard: the site does not
    /// decide the situation. `record_not_temporary`: the caller knows the
    /// record is persistent. Such a dispatch is kept, marked conditional
    /// (the subscription may be skipped), so a path through it says what it
    /// assumes.
    #[must_use]
    pub fn may_skip(self, exits: EntryExits, record_not_temporary: bool) -> bool {
        (exits.intersects(EntryExits::TEMPORARY_REC)
            && !self.record_temporary
            && !record_not_temporary)
            || (exits.intersects(EntryExits::NO_RUN_TRIGGER)
                && !self.run_trigger_false
                && !self.run_trigger_true)
    }

    #[must_use]
    pub fn is_empty(self) -> bool {
        !self.record_temporary && !self.run_trigger_false && !self.run_trigger_true
    }
}

fn unwrap_parens(file: &AlFile, mut e: ExprId) -> ExprId {
    while let ExprKind::Parenthesized(inner) = &file.ir.expr(e).kind {
        e = *inner;
    }
    e
}

fn is_ident(file: &AlFile, e: ExprId, name: &str) -> bool {
    match &file.ir.expr(unwrap_parens(file, e)).kind {
        ExprKind::Identifier(n) | ExprKind::QuotedIdentifier(n) => n.eq_fold_identifier(name),
        _ => false,
    }
}

/// `Rec.IsTemporary` / `Rec.IsTemporary()`.
fn is_rec_istemporary(file: &AlFile, e: ExprId) -> bool {
    let e = unwrap_parens(file, e);
    let member = match &file.ir.expr(e).kind {
        ExprKind::Call { function, args } if args.is_empty() => *function,
        ExprKind::Member { .. } => e,
        _ => return false,
    };
    let ExprKind::Member { object, member, .. } = &file.ir.expr(member).kind else {
        return false;
    };
    member
        .trim()
        .trim_matches('"')
        .eq_ignore_ascii_case("istemporary")
        && is_ident(file, *object, "rec")
}

/// One atom of a guard condition, or `None` for anything else.
fn atom(file: &AlFile, e: ExprId) -> Option<EntryExits> {
    let e = unwrap_parens(file, e);
    if is_rec_istemporary(file, e) {
        return Some(EntryExits::TEMPORARY_REC);
    }
    match &file.ir.expr(e).kind {
        ExprKind::Unary {
            op: UnaryOp::Not,
            operand,
        } if is_ident(file, *operand, "runtrigger") => Some(EntryExits::NO_RUN_TRIGGER),
        ExprKind::Binary {
            op: BinaryOp::Eq,
            lhs,
            rhs,
        } => {
            let is_false = |x: ExprId| {
                matches!(
                    &file.ir.expr(unwrap_parens(file, x)).kind,
                    ExprKind::Literal(Literal::Bool(false))
                )
            };
            ((is_ident(file, *lhs, "runtrigger") && is_false(*rhs))
                || (is_ident(file, *rhs, "runtrigger") && is_false(*lhs)))
            .then_some(EntryExits::NO_RUN_TRIGGER)
        }
        _ => None,
    }
}

/// The exits of an `or` of atoms; `None` when any operand is not an atom.
fn condition(file: &AlFile, e: ExprId) -> Option<EntryExits> {
    let e = unwrap_parens(file, e);
    if let ExprKind::Binary {
        op: BinaryOp::Or,
        lhs,
        rhs,
    } = &file.ir.expr(e).kind
    {
        return Some(condition(file, *lhs)?.union(condition(file, *rhs)?));
    }
    atom(file, e)
}

/// The exits of one `if <cond> then exit` statement, or `None`.
fn guard_statement(file: &AlFile, s: al_syntax::ir::StmtId) -> Option<EntryExits> {
    let StmtKind::If {
        cond,
        then_block,
        else_block: None,
    } = &file.ir.stmt(s).kind
    else {
        return None;
    };
    let items = &file.ir.block(*then_block).items;
    let [BlockItem::Stmt(only)] = items.as_slice() else {
        return None;
    };
    if !matches!(file.ir.stmt(*only).kind, StmtKind::Exit(None)) {
        return None;
    }
    condition(file, *cond)
}

/// What routine `r` exits on before anything else runs (see the module doc):
/// [`EntryExits::NONE`] for any other shape, and for a routine without the
/// `Rec` / `RunTrigger` parameter a recognized atom names.
#[must_use]
pub fn subscriber_entry_exits(file: &AlFile, r: &RoutineDecl) -> EntryExits {
    let Some(body) = r.body else {
        return EntryExits::NONE;
    };
    let has_param = |n: &str| r.params.iter().any(|p| p.name.eq_fold_identifier(n));
    let mut out = EntryExits::NONE;
    for item in &file.ir.block(body).items {
        let BlockItem::Stmt(s) = item else {
            break;
        };
        let Some(exits) = guard_statement(file, *s) else {
            break;
        };
        if exits.intersects(EntryExits::TEMPORARY_REC) && !has_param("rec") {
            break;
        }
        if exits.intersects(EntryExits::NO_RUN_TRIGGER) && !has_param("runtrigger") {
            break;
        }
        out = out.union(exits);
    }
    out
}

/// Whether the record a record operation's receiver names is declared
/// `temporary` in the routine's scope: a parameter whose type text ends in
/// `temporary` (a `var` parameter declared temporary accepts only a temporary
/// record, and a by-value one is a temporary copy), else a local, else an
/// object global declared temporary. Lookup order params, locals, globals:
/// the first declaration of the name decides. The implicit `Rec` of a page
/// declared `SourceTableTemporary = true` is temporary too
/// (`page_source_table_temporary`). Anything else is not known temporary.
#[must_use]
pub fn receiver_declared_temporary(
    receiver: &str,
    routine: &RoutineDecl,
    globals: &[al_syntax::ir::VarDecl],
    page_source_table_temporary: bool,
) -> bool {
    let name = crate::program::body::node_util::strip_quotes(receiver.trim()).replace("\"\"", "\"");
    if name.contains('.') || name.contains('(') {
        return false;
    }
    if let Some(p) = routine
        .params
        .iter()
        .find(|p| p.name.eq_fold_identifier(&name))
    {
        return p.ty.as_deref().is_some_and(ends_with_temporary);
    }
    if let Some(v) = routine
        .locals
        .iter()
        .find(|v| v.name.eq_fold_identifier(&name))
    {
        return v.temporary || v.ty.as_deref().is_some_and(ends_with_temporary);
    }
    if routine
        .return_name
        .as_deref()
        .is_some_and(|n| n.eq_fold_identifier(&name))
    {
        return false;
    }
    if let Some(v) = globals.iter().find(|v| v.name.eq_fold_identifier(&name)) {
        return v.temporary || v.ty.as_deref().is_some_and(ends_with_temporary);
    }
    page_source_table_temporary && name.eq_fold_identifier("rec")
}

/// Whether object `o` is a page declared `SourceTableTemporary = true`: its
/// implicit `Rec` (and every record a user works on in it) is a temporary
/// record of its source table. A page extension inherits its base page's
/// property, which this does not read (not known temporary).
#[must_use]
pub fn page_source_table_temporary(o: &al_syntax::ir::ObjectDecl) -> bool {
    // Every build must set it (engine limits B2): an `#if`-only
    // `SourceTableTemporary = true` leaves the default (`false`) possible, and
    // reading the page's `Rec` as temporary there drops its database writes.
    let entries = || {
        o.properties
            .iter()
            .filter(|p| p.name.eq_ignore_ascii_case("sourcetabletemporary"))
    };
    o.kind == al_syntax::ir::ObjectKind::Page
        && entries().any(|p| !p.conditional)
        && entries().all(|p| p.value.trim().eq_ignore_ascii_case("true"))
}

fn ends_with_temporary(ty: &str) -> bool {
    let t = ty.trim_end();
    t.len() > "temporary".len()
        && t.is_char_boundary(t.len() - "temporary".len())
        && t[t.len() - "temporary".len()..].eq_ignore_ascii_case("temporary")
        && t[..t.len() - "temporary".len()]
            .chars()
            .last()
            .is_some_and(char::is_whitespace)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exits_of(src: &str) -> EntryExits {
        let f = al_syntax::parse(src);
        let o = &f.objects[0];
        let r = o.routines.iter().find(|r| r.name == "Sub").expect("Sub");
        subscriber_entry_exits(&f, r)
    }

    fn sub(params: &str, body: &str) -> String {
        format!(
            "codeunit 50100 X\n{{\n    local procedure Sub({params})\n    begin\n{body}\n    end;\n}}\n"
        )
    }

    const P: &str = "var Rec: Record Customer; RunTrigger: Boolean";

    #[test]
    fn recognizes_the_guard_shapes() {
        for (body, want) in [
            ("if Rec.IsTemporary() then exit;", EntryExits::TEMPORARY_REC),
            ("if Rec.IsTemporary then exit;", EntryExits::TEMPORARY_REC),
            (
                "if (Rec.IsTemporary) then\n exit;",
                EntryExits::TEMPORARY_REC,
            ),
            ("if not RunTrigger then exit;", EntryExits::NO_RUN_TRIGGER),
            (
                "if RunTrigger = false then exit;",
                EntryExits::NO_RUN_TRIGGER,
            ),
            (
                "if not RunTrigger then exit;\nif Rec.IsTemporary() then exit;",
                EntryExits::TEMPORARY_REC.union(EntryExits::NO_RUN_TRIGGER),
            ),
            (
                "if Rec.IsTemporary or not RunTrigger then exit;",
                EntryExits::TEMPORARY_REC.union(EntryExits::NO_RUN_TRIGGER),
            ),
            (
                "if Rec.IsTemporary() then begin exit; end;",
                EntryExits::TEMPORARY_REC,
            ),
        ] {
            assert_eq!(exits_of(&sub(P, body)), want, "{body}");
        }
    }

    #[test]
    fn rejects_shapes_that_still_run_code() {
        for body in [
            // A call beside the test runs whatever the test says (no
            // short-circuit in AL).
            "if Rec.IsTemporary or Mgt.Disabled() then exit;",
            // Not the first statement.
            "Mgt.Log();\nif Rec.IsTemporary() then exit;",
            // An else branch, or more than an exit.
            "if Rec.IsTemporary() then exit else Mgt.Log();",
            "if Rec.IsTemporary() then begin Mgt.Log(); exit; end;",
            // The opposite test.
            "if not Rec.IsTemporary() then exit;",
            "if RunTrigger then exit;",
            // `and` is not a guard of either situation alone.
            "if Rec.IsTemporary and not RunTrigger then exit;",
            // Another record.
            "if Other.IsTemporary() then exit;",
        ] {
            assert_eq!(exits_of(&sub(P, body)), EntryExits::NONE, "{body}");
        }
    }

    #[test]
    fn needs_the_event_parameters() {
        // `Rec` / `RunTrigger` here are not the subscriber's parameters.
        assert_eq!(
            exits_of(&sub(
                "var Other: Record Customer",
                "if not RunTrigger then exit;"
            )),
            EntryExits::NONE
        );
        assert_eq!(
            exits_of(&sub(
                "RunTrigger: Boolean",
                "if Rec.IsTemporary() then exit;"
            )),
            EntryExits::NONE
        );
    }

    #[test]
    fn raise_context_kills_only_the_matching_guard() {
        let temp = RaiseContext {
            record_temporary: true,
            ..RaiseContext::default()
        };
        assert!(temp.kills(EntryExits::TEMPORARY_REC));
        assert!(!temp.kills(EntryExits::NO_RUN_TRIGGER));
        assert!(!temp.kills(EntryExits::NONE));
        assert!(!RaiseContext::default().kills(EntryExits::TEMPORARY_REC));
        // Undecided sites keep the dispatch, conditional.
        let open = RaiseContext::default();
        assert!(open.may_skip(EntryExits::TEMPORARY_REC, false));
        assert!(!open.may_skip(EntryExits::TEMPORARY_REC, true));
        assert!(open.may_skip(EntryExits::NO_RUN_TRIGGER, true));
        let rt = RaiseContext {
            run_trigger_true: true,
            ..RaiseContext::default()
        };
        assert!(!rt.may_skip(EntryExits::NO_RUN_TRIGGER, false));
        assert!(!rt.kills(EntryExits::NO_RUN_TRIGGER));
    }

    #[test]
    fn declared_temporary_lookup() {
        let src = "codeunit 50100 X\n{\n    var\n        G: Record Customer temporary;\n        H: Record Customer;\n    procedure R(var P: Record Customer temporary; Q: Record Customer)\n    var\n        L: Record Customer temporary;\n        G: Record Customer;\n    begin\n    end;\n}\n";
        let f = al_syntax::parse(src);
        let o = &f.objects[0];
        let r = &o.routines[0];
        let t = |n: &str| receiver_declared_temporary(n, r, &o.globals, false);
        assert!(t("P"));
        assert!(!t("Q"));
        assert!(t("L"));
        // The local `G` shadows the temporary global.
        assert!(!t("G"));
        assert!(!t("H"));
        assert!(!t("Rec"));
        assert!(receiver_declared_temporary("Rec", r, &o.globals, true));
        assert!(!t("P.Field"));
    }

    /// The model's table-event edges (`program_calls::table_events`): none
    /// where the site proves the guard, a conditional one where it does not
    /// decide, a plain one where the guard provably passes.
    #[test]
    fn model_edges_follow_the_raising_context() {
        let src = r#"
table 50700 "TG Rec"
{
    fields { field(1; Code; Code[20]) { } }
    keys { key(PK; Code) { } }
}
table 50701 "TG Buffer"
{
    TableType = Temporary;
    fields { field(1; Code; Code[20]) { } }
    keys { key(PK; Code) { } }
}
codeunit 50720 "TG Subscribers"
{
    [EventSubscriber(ObjectType::Table, Database::"TG Rec", 'OnAfterInsertEvent', '', false, false)]
    local procedure AfterInsertTemp(var Rec: Record "TG Rec"; RunTrigger: Boolean)
    begin
        if Rec.IsTemporary() then
            exit;
        Rec.Init();
    end;

    [EventSubscriber(ObjectType::Table, Database::"TG Rec", 'OnAfterDeleteEvent', '', false, false)]
    local procedure AfterDeleteRT(var Rec: Record "TG Rec"; RunTrigger: Boolean)
    begin
        if not RunTrigger then
            exit;
        Rec.Init();
    end;

    [EventSubscriber(ObjectType::Table, Database::"TG Buffer", 'OnAfterInsertEvent', '', false, false)]
    local procedure AfterInsertBuffer(var Rec: Record "TG Buffer"; RunTrigger: Boolean)
    begin
        if Rec.IsTemporary() then
            exit;
        Rec.Init();
    end;
}
codeunit 50730 "TG Callers"
{
    procedure InsertTemp()
    var
        R: Record "TG Rec" temporary;
    begin
        R.Insert();
    end;

    procedure InsertPersistent()
    var
        R: Record "TG Rec";
    begin
        R.Insert();
    end;

    procedure InsertCallersRecord(var R: Record "TG Rec")
    begin
        R.Insert();
    end;

    procedure InsertBuffer()
    var
        B: Record "TG Buffer";
    begin
        B.Insert();
    end;

    procedure DeleteDefault()
    var
        R: Record "TG Rec";
    begin
        R.Delete();
    end;

    procedure DeleteTrue()
    var
        R: Record "TG Rec";
    begin
        R.Delete(true);
    end;

    procedure DeleteComputed(Run: Boolean)
    var
        R: Record "TG Rec";
    begin
        R.Delete(Run);
    end;
}
"#;
        let m = crate::program::model::program_calls::assemble_and_resolve_inline_program_default(
            &[("src/a.al".to_string(), src.to_string())],
            "38383838-0000-4000-8000-000000000038",
        );
        let id = |name: &str| {
            let hits: Vec<_> = m
                .workspace
                .routines
                .iter()
                .filter(|r| r.name == name)
                .collect();
            assert_eq!(hits.len(), 1, "precondition: one routine {name}");
            hits[0].id.clone()
        };
        // Per (caller, subscriber): `None` no edge, `Some(conditional)`.
        let edge = |from: &str, to: &str| -> Option<bool> {
            let (f, t) = (id(from), id(to));
            let es: Vec<bool> = m
                .calls
                .edges
                .iter()
                .filter(|e| {
                    e.dispatch_kind == crate::program::model::taxonomy::DispatchKind::TableEvent
                        && e.from == f
                        && e.to.as_deref() == Some(t.as_str())
                })
                .map(|e| {
                    e.table_event
                        .as_ref()
                        .expect("table-event meta")
                        .conditional
                })
                .collect();
            assert!(es.len() <= 1, "{from} -> {to}: {es:?}");
            es.first().copied()
        };
        let mut wrong = Vec::new();
        for (from, to, want) in [
            ("InsertTemp", "AfterInsertTemp", None),
            ("InsertPersistent", "AfterInsertTemp", Some(false)),
            ("InsertCallersRecord", "AfterInsertTemp", Some(true)),
            // Temporary by its table (the model's temp state, not a keyword).
            ("InsertBuffer", "AfterInsertBuffer", None),
            ("DeleteDefault", "AfterDeleteRT", None),
            ("DeleteTrue", "AfterDeleteRT", Some(false)),
            ("DeleteComputed", "AfterDeleteRT", Some(true)),
        ] {
            let got = edge(from, to);
            if got != want {
                wrong.push(format!("{from} -> {to}: {got:?} (want {want:?})"));
            }
        }
        assert!(wrong.is_empty(), "{wrong:#?}");
    }
}

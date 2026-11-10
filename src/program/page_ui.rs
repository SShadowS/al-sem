//! **What a user can do to a page's records through the page** (round-3 fact
//! check n09 and m03): the page properties that decide whether a user's
//! insert, modify, rename, delete or field edit on a page can run its source
//! table's record triggers and table events.
//!
//! A page's lifecycle site into its source table's triggers
//! ([`crate::program::impact_graph::project`]) stands for a user working in
//! the page. Before this module the site was added for every trigger whatever
//! the page declared, so `Page "CDO E-Mail/Print Statement"` (`DeleteAllowed =
//! false`) was an established UI route to the Customer `OnAfterDeleteEvent`.
//!
//! The rules (Microsoft Learn, page properties; runtime 1.0):
//!
//! | Property, written literally `false` | Blocks through the page |
//! |---|---|
//! | `InsertAllowed` | insert |
//! | `ModifyAllowed` | modify, rename (renaming is editing a key field of an existing record) |
//! | `DeleteAllowed` | delete |
//! | `Editable` | insert, modify, rename. **Not delete**: Microsoft Learn says "Do not use `CurrPage.Editable` to prevent users from deleting entries"; a user deletes on a non-editable list page (Base Application's `Customer List` is `Editable = false`) |
//!
//! A field edit (`validate`) needs either a new record (insert) or an
//! existing one (modify): it is blocked when both are.
//!
//! - `InsertAllowed`, `ModifyAllowed` and `DeleteAllowed` apply to a Page or a
//!   request page only (not a page extension) and no AL method sets them at
//!   runtime (the `Page` data type's methods: `Editable` is the only one of the
//!   four), so a literal `false` is certain.
//! - `Editable` can be changed at runtime: `Page.Editable([Boolean])` "gets or
//!   sets the default editability of the page". A block that comes from
//!   `Editable = false` alone is therefore certain only when no code in the
//!   program may call `Editable(…)` with an argument on that page
//!   ([`EditableSetter`]); otherwise it is a CANDIDATE: the lifecycle site
//!   stays, and its relation is not established
//!   ([`crate::engine::impact::up::site_bits`]).
//! - A property with a `#if` arm that is not literal `false` (or any other
//!   value) blocks nothing. Neither does a property that only some builds
//!   write (every entry in a `#if` arm or under a whole-value `#if`, engine
//!   limits B2): a build without it has the default, `true`. Imprecise in one
//!   direction only: `#if X DeleteAllowed = false; #else DeleteAllowed =
//!   false; #endif` covers every build and still blocks nothing.
//! - `SourceTableTemporary = true` is not here: a user's work in such a page
//!   still runs the table's own triggers (they run on temporary records), and
//!   its table events come with the record temporary, which the existing
//!   table-event guard already carries (`Site::guard`, `TEMPORARY_REC`).
//!
//! The facts live on [`crate::program::node_extract::ObjectNode::page_ui`]
//! (source pages from their properties and bodies, symbol-only pages from the
//! symbol reference), and [`page_ui_blocks`] reads them from the program graph,
//! so the sited graph (impact, tests) and the model (`explain`'s potential UI
//! surfaces) read one answer.

use std::collections::{HashMap, HashSet};

use al_syntax::IdentifierFoldExt;
use al_syntax::ir::{AlFile, ExprKind, ObjectDecl, ObjectKind};
use serde::{Deserialize, Serialize};

use crate::program::graph::ProgramGraph;
use crate::program::node::ObjectNodeId;
use crate::program::node_extract::{ObjectNode, ObjectRef};

/// The record operations a page can block, as bits over
/// [`crate::program::impact_graph::sited::RECORD_OPS`] indexes (insert 0,
/// modify 1, delete 2, rename 3, validate 4).
pub mod op {
    pub const INSERT: u8 = 1 << 0;
    pub const MODIFY: u8 = 1 << 1;
    pub const DELETE: u8 = 1 << 2;
    pub const RENAME: u8 = 1 << 3;
    pub const VALIDATE: u8 = 1 << 4;

    /// The bit of the operation at `RECORD_OPS` index `i`.
    #[must_use]
    pub fn of_index(i: u8) -> u8 {
        1u8.checked_shl(u32::from(i)).unwrap_or(0)
    }
}

/// One code site that may change a page's editability at runtime: a call
/// `X.Editable(<argument>)` (the setter form; the bare getter `Editable()`
/// changes nothing). What `X` names, as far as the file's declarations say.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum EditableSetter {
    /// `CurrPage.Editable(…)` (or an unqualified `Editable(…)` in a page
    /// file that declares no procedure of that name): the declaring page, or
    /// for a page extension its base page.
    CurrPage,
    /// A variable or parameter declared `Page X` in the file.
    Page(ObjectRef),
    /// `CurrPage.<part>.Page.Editable(…)` (or `CurrPage.<part>.Editable(…)`):
    /// the page the part control shows; the control's name, folded.
    Part(String),
    /// A receiver the file's declarations do not type (a call result, a
    /// variable the file does not declare): any page.
    Unknown,
}

/// A page's declared UI permissions and the setters in its object's code
/// that may change its editability. `Default` (nothing blocked, no setter)
/// for every object that is not a page and every page that declares none.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageUiDecl {
    /// `Editable` written literally `false` (in every `#if` arm).
    pub editable_false: bool,
    /// `InsertAllowed` written literally `false`.
    pub insert_false: bool,
    /// `ModifyAllowed` written literally `false`.
    pub modify_false: bool,
    /// `DeleteAllowed` written literally `false`.
    pub delete_false: bool,
    /// The `Editable(…)` setter calls in this object's code (any object kind),
    /// deduplicated, sorted by first appearance.
    pub editable_setters: Vec<EditableSetter>,
}

impl PageUiDecl {
    /// The four properties of a source Page (a page extension cannot declare
    /// them), from its lowered properties: `true` only when the property is
    /// present, at least one entry is in every build (not conditional), and
    /// every value (one per `#if` arm) is literally `false`.
    #[must_use]
    pub fn from_properties(obj: &ObjectDecl) -> PageUiDecl {
        if obj.kind != ObjectKind::Page {
            return PageUiDecl::default();
        }
        // Engine limits B2: an entry only some builds have (`#if X DeleteAllowed
        // = false; #endif`) leaves the default (`true`) possible in the others,
        // so a block needs one entry every build has.
        let all_false = |name: &str| {
            let values: Vec<_> = obj
                .properties
                .iter()
                .filter(|p| p.name.eq_ignore_ascii_case(name))
                .collect();
            values.iter().any(|p| !p.conditional)
                && values
                    .iter()
                    .all(|p| p.value.trim().eq_ignore_ascii_case("false"))
        };
        PageUiDecl {
            editable_false: all_false("editable"),
            insert_false: all_false("insertallowed"),
            modify_false: all_false("modifyallowed"),
            delete_false: all_false("deleteallowed"),
            editable_setters: Vec::new(),
        }
    }

    /// A symbol-only page's properties as its symbol reference spells them
    /// (`Some(false)` = written false; a symbol carries the compiled value).
    #[must_use]
    pub fn from_symbols(
        editable: Option<bool>,
        insert_allowed: Option<bool>,
        modify_allowed: Option<bool>,
        delete_allowed: Option<bool>,
    ) -> PageUiDecl {
        PageUiDecl {
            editable_false: editable == Some(false),
            insert_false: insert_allowed == Some(false),
            modify_false: modify_allowed == Some(false),
            delete_false: delete_allowed == Some(false),
            editable_setters: Vec::new(),
        }
    }

    /// Merge another copy of the same object (`#if` arms the program keeps
    /// as one node): a property blocks only when every copy blocks, and the
    /// setters are the union.
    pub fn merge(&mut self, other: &PageUiDecl) {
        self.editable_false &= other.editable_false;
        self.insert_false &= other.insert_false;
        self.modify_false &= other.modify_false;
        self.delete_false &= other.delete_false;
        for s in &other.editable_setters {
            if !self.editable_setters.contains(s) {
                self.editable_setters.push(s.clone());
            }
        }
    }
}

/// The `Editable(<argument>)` setter calls in `file`'s code, one list per
/// object of `file` (index-aligned with `file.objects`): a call belongs to
/// the object whose source span holds it, and its receiver is typed from that
/// object's declarations. A call no object's span holds (not expected) is
/// given to every object: more candidates, never a false block.
#[must_use]
pub fn editable_setters(file: &AlFile) -> Vec<Vec<EditableSetter>> {
    let mut out: Vec<Vec<EditableSetter>> = vec![Vec::new(); file.objects.len()];
    for e in file.ir.iter_exprs() {
        let ExprKind::Call { function, args } = &e.kind else {
            continue;
        };
        if args.is_empty() {
            continue;
        }
        let bare = match &file.ir.expr(*function).kind {
            ExprKind::Identifier(n) if n.eq_fold_identifier("editable") => None,
            ExprKind::Member { object, member, .. }
                if crate::program::body::node_util::strip_quotes(member)
                    .eq_fold_identifier("editable") =>
            {
                Some(*object)
            }
            _ => continue,
        };
        let at = e.origin.byte.start;
        let owners: Vec<usize> = match file
            .objects
            .iter()
            .position(|o| o.origin.byte.contains(&at))
        {
            Some(i) => vec![i],
            None => (0..file.objects.len()).collect(),
        };
        for i in owners {
            let obj = &file.objects[i];
            let setter = match bare {
                // An unqualified `Editable(…)` in a page: `CurrPage`'s,
                // unless the object declares a procedure of that name.
                None => (matches!(obj.kind, ObjectKind::Page | ObjectKind::PageExtension)
                    && !obj
                        .routines
                        .iter()
                        .any(|r| r.name.eq_fold_identifier("editable")))
                .then_some(EditableSetter::CurrPage),
                Some(recv) => setter_of_receiver(file, obj, recv),
            };
            if let Some(s) = setter
                && !out[i].contains(&s)
            {
                out[i].push(s);
            }
        }
    }
    out
}

/// The name of an identifier expression, unquoted; `None` for any other shape.
fn ident_name(file: &AlFile, id: al_syntax::ir::ExprId) -> Option<String> {
    match &file.ir.expr(id).kind {
        ExprKind::Identifier(n) => Some(n.clone()),
        ExprKind::QuotedIdentifier(n) => {
            Some(crate::program::body::node_util::strip_quotes(n).replace("\"\"", "\""))
        }
        _ => None,
    }
}

/// Every declared type of `name` in object `o` (its globals, and the
/// parameters and locals of every routine of it: a name that means a page in
/// one routine and something else in another gives both).
fn declared_types<'f>(o: &'f ObjectDecl, name: &str) -> Vec<&'f str> {
    let mut tys: Vec<&str> = o
        .globals
        .iter()
        .filter(|v| v.name.eq_fold_identifier(name))
        .filter_map(|v| v.ty.as_deref())
        .collect();
    for r in &o.routines {
        tys.extend(
            r.params
                .iter()
                .filter(|p| p.name.eq_fold_identifier(name))
                .filter_map(|p| p.ty.as_deref()),
        );
        tys.extend(
            r.locals
                .iter()
                .filter(|v| v.name.eq_fold_identifier(name))
                .filter_map(|v| v.ty.as_deref()),
        );
    }
    tys
}

/// What the receiver `recv` of an `Editable(…)` setter in object `obj`
/// names; `None` when `obj` declares the receiver with types none of which is
/// a page (a codeunit's own `Editable` procedure, a `TestPage`, whose
/// `Editable()` is a getter): no page's editability changes.
fn setter_of_receiver(
    file: &AlFile,
    obj: &ObjectDecl,
    recv: al_syntax::ir::ExprId,
) -> Option<EditableSetter> {
    use crate::program::resolve::receiver::{ParsedType, classify_type_text};
    let ident = |id: al_syntax::ir::ExprId| ident_name(file, id);
    if let Some(name) = ident(recv) {
        if name.eq_fold_identifier("currpage") {
            return Some(EditableSetter::CurrPage);
        }
        let tys = declared_types(obj, &name);
        if tys.is_empty() {
            // Declared elsewhere (a base page's `protected var`): any page.
            return Some(EditableSetter::Unknown);
        }
        // The first Page-typed declaration names the page.
        for t in &tys {
            if let ParsedType::Object {
                kind: ObjectKind::Page,
                object_ref,
            } = classify_type_text(t)
            {
                return Some(EditableSetter::Page(object_ref));
            }
        }
        let page_like = tys.iter().any(|t| {
            let h = t.trim_start().to_ascii_lowercase();
            h.starts_with("page") && !h.starts_with("pageextension")
        });
        // A Page type the classifier did not read: any page.
        return page_like.then_some(EditableSetter::Unknown);
    }
    // `CurrPage.<part>.Page` (and `CurrPage.<part>`): the part's page.
    let part_of = |id: al_syntax::ir::ExprId| match &file.ir.expr(id).kind {
        ExprKind::Member { object, member, .. }
            if ident(*object).is_some_and(|r| r.eq_fold_identifier("currpage")) =>
        {
            Some(
                crate::program::body::node_util::strip_quotes(member)
                    .replace("\"\"", "\"")
                    .fold_identifier(),
            )
        }
        _ => None,
    };
    if let Some(part) = part_of(recv) {
        return Some(EditableSetter::Part(part));
    }
    if let ExprKind::Member { object, member, .. } = &file.ir.expr(recv).kind
        && crate::program::body::node_util::strip_quotes(member).eq_fold_identifier("page")
        && let Some(part) = part_of(*object)
    {
        return Some(EditableSetter::Part(part));
    }
    Some(EditableSetter::Unknown)
}

/// What a page blocks a user from doing to its source table's records
/// through the page: `certain` operations (no site), `candidate` ones (the
/// site stays, its relation not established), as [`op`] bits. An operation
/// is in at most one of the two.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PageUiBlocks {
    pub certain: u8,
    pub candidate: u8,
}

impl PageUiBlocks {
    /// The blocks a page's declaration gives, `editable_at_runtime` when some
    /// code may set its editability.
    #[must_use]
    pub fn of(d: &PageUiDecl, editable_at_runtime: bool) -> PageUiBlocks {
        let mut b = PageUiBlocks::default();
        let editable = |b: &mut PageUiBlocks, ops: u8| {
            if editable_at_runtime {
                b.candidate |= ops;
            } else {
                b.certain |= ops;
            }
        };
        if d.editable_false {
            editable(&mut b, op::INSERT | op::MODIFY | op::RENAME);
        }
        if d.insert_false {
            b.certain |= op::INSERT;
        }
        if d.modify_false {
            b.certain |= op::MODIFY | op::RENAME;
        }
        if d.delete_false {
            b.certain |= op::DELETE;
        }
        // A field edit needs a new record or an existing one.
        let blocked = |b: &PageUiBlocks, o: u8| (b.certain | b.candidate) & o != 0;
        if b.certain & op::INSERT != 0 && b.certain & op::MODIFY != 0 {
            b.certain |= op::VALIDATE;
        } else if blocked(&b, op::INSERT) && blocked(&b, op::MODIFY) {
            b.candidate |= op::VALIDATE;
        }
        b.candidate &= !b.certain;
        b
    }

    /// Whether operation bit `o` is blocked for certain.
    #[must_use]
    pub fn blocks(self, o: u8) -> bool {
        self.certain & o != 0
    }
}

/// The pages that may have their editability set at runtime, and whether
/// some setter names a page the graph cannot resolve (then every page may).
fn editable_at_runtime(graph: &ProgramGraph) -> (HashSet<ObjectNodeId>, bool) {
    let mut pages: HashSet<ObjectNodeId> = HashSet::new();
    let mut any = false;
    let resolve = |from: &ObjectNode, r: &ObjectRef| -> Option<ObjectNodeId> {
        match r {
            ObjectRef::Id(n) => graph
                .resolve_object_by_number(from.id.app, ObjectKind::Page, *n)
                .map(|o| o.id.clone()),
            ObjectRef::Name { raw, .. } => graph
                .resolve_object(from.id.app, ObjectKind::Page, raw)
                .map(|o| o.id.clone()),
        }
    };
    let base_page = |o: &ObjectNode| -> Option<ObjectNodeId> {
        match o.id.kind {
            ObjectKind::Page => Some(o.id.clone()),
            ObjectKind::PageExtension => graph
                .resolve_object(o.id.app, ObjectKind::Page, o.extends_target.as_deref()?)
                .map(|b| b.id.clone()),
            _ => None,
        }
    };
    for o in graph.objects.iter() {
        for s in &o.page_ui.editable_setters {
            let target = match s {
                EditableSetter::CurrPage => match base_page(o) {
                    Some(p) => Some(p),
                    // `CurrPage` outside a page: not a page's editability.
                    None if !matches!(o.id.kind, ObjectKind::Page | ObjectKind::PageExtension) => {
                        continue;
                    }
                    None => None,
                },
                EditableSetter::Page(r) => resolve(o, r),
                EditableSetter::Part(name) => {
                    // The part control, declared on this object (or, for a
                    // page extension, on its base page).
                    let on_base = base_page(o)
                        .and_then(|b| graph.objects.iter().find(|x| x.id == b))
                        .into_iter();
                    std::iter::once(o)
                        .chain(on_base)
                        .flat_map(|x| x.page_controls.iter().map(move |c| (x, c)))
                        .find(|(_, c)| c.name_lc.as_str() == name.as_str())
                        .and_then(|(x, c)| resolve(x, &c.target))
                }
                EditableSetter::Unknown => None,
            };
            match target {
                Some(p) => {
                    pages.insert(p);
                }
                None => any = true,
            }
        }
    }
    (pages, any)
}

/// Every page of `graph` that blocks something, with its blocks.
#[must_use]
pub fn page_ui_blocks(graph: &ProgramGraph) -> HashMap<ObjectNodeId, PageUiBlocks> {
    let declares =
        |d: &PageUiDecl| d.editable_false || d.insert_false || d.modify_false || d.delete_false;
    if !graph
        .objects
        .iter()
        .any(|o| o.id.kind == ObjectKind::Page && declares(&o.page_ui))
    {
        return HashMap::new();
    }
    let (runtime, any) = editable_at_runtime(graph);
    graph
        .objects
        .iter()
        .filter(|o| o.id.kind == ObjectKind::Page && declares(&o.page_ui))
        .map(|o| {
            let b = PageUiBlocks::of(&o.page_ui, any || runtime.contains(&o.id));
            (o.id.clone(), b)
        })
        .filter(|(_, b)| *b != PageUiBlocks::default())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decl(e: bool, i: bool, m: bool, d: bool) -> PageUiDecl {
        PageUiDecl {
            editable_false: e,
            insert_false: i,
            modify_false: m,
            delete_false: d,
            editable_setters: Vec::new(),
        }
    }

    #[test]
    fn each_property_blocks_its_operations() {
        assert_eq!(
            PageUiBlocks::of(&decl(false, true, false, false), false),
            PageUiBlocks {
                certain: op::INSERT,
                candidate: 0
            }
        );
        assert_eq!(
            PageUiBlocks::of(&decl(false, false, true, false), false).certain,
            op::MODIFY | op::RENAME
        );
        assert_eq!(
            PageUiBlocks::of(&decl(false, false, false, true), false).certain,
            op::DELETE
        );
        assert_eq!(
            PageUiBlocks::of(&decl(false, true, true, false), false).certain,
            op::INSERT | op::MODIFY | op::RENAME | op::VALIDATE
        );
    }

    /// The setter shapes, each stated in source: `CurrPage`, a `Page`
    /// variable, a part's page (with and without `.Page`), a codeunit's own
    /// `Editable` procedure (no page), a getter (no argument), and a receiver
    /// the file does not declare (any page).
    #[test]
    fn editable_setters_read_each_receiver_shape() {
        let src = r#"page 50100 "S Page"
{
    SourceTable = "S Rec";
    Editable = false;
    InsertAllowed = false;
    layout { area(Content) { part(Lines; "S Part") { } } }
    trigger OnOpenPage()
    var
        P: Page "S Other";
        C: Codeunit "S Cu";
    begin
        CurrPage.Editable(true);
        P.Editable(true);
        CurrPage.Lines.Page.Editable(false);
        CurrPage.Lines.Editable(false);
        C.Editable(true);
        if CurrPage.Editable() then;
        Undeclared.Editable(true);
    end;
}
"#;
        let f = al_syntax::parse(src);
        let got = editable_setters(&f);
        assert_eq!(
            got[0],
            vec![
                EditableSetter::CurrPage,
                EditableSetter::Page(ObjectRef::Name {
                    raw: "S Other".into(),
                    normalized_lc: "s other".into()
                }),
                EditableSetter::Part("lines".into()),
                EditableSetter::Unknown,
            ],
            "{got:?}"
        );
        let d = PageUiDecl::from_properties(&f.objects[0]);
        assert!(d.editable_false && d.insert_false && !d.modify_false && !d.delete_false);
    }

    /// One-app graph of `src` (the extraction the program build runs).
    fn graph_of(src: &str) -> ProgramGraph {
        use crate::program::graph::ObjectIndex;
        use crate::program::node::AppRegistry;
        use crate::program::node_extract::extract_nodes;
        use crate::snapshot::{AppId, TrustTier};
        let mut apps = AppRegistry::default();
        let app = apps.intern(&AppId {
            guid: String::new(),
            name: "PU".into(),
            publisher: "T".into(),
            version: "1.0.0.0".into(),
        });
        let (mut objects, mut routines) = (Vec::new(), Vec::new());
        extract_nodes(
            app,
            &al_syntax::parse(src),
            TrustTier::Workspace,
            &mut objects,
            &mut routines,
        );
        objects.sort_by(|a, b| a.id.cmp(&b.id));
        routines.sort_by(|a, b| a.id.cmp(&b.id));
        let obj_index = ObjectIndex::build(&objects);
        ProgramGraph {
            apps,
            objects: objects.into(),
            routines: routines.into(),
            obj_index,
            ..Default::default()
        }
    }

    fn blocks_of(g: &ProgramGraph, name: &str) -> PageUiBlocks {
        let id = &g
            .objects
            .iter()
            .find(|o| o.name.as_str() == name)
            .unwrap_or_else(|| panic!("no {name}"))
            .id;
        page_ui_blocks(g).get(id).copied().unwrap_or_default()
    }

    /// The setters reach the pages they name, a page extension's `CurrPage`
    /// its base page, and only those pages' `Editable = false` blocks become
    /// candidates; an undeclared receiver makes every page's a candidate.
    #[test]
    fn a_setter_makes_only_its_pages_editable_blocks_candidates() {
        let pages = r#"page 50850 "PU Read Only Page" { SourceTable = "PU Rec"; Editable = false; }
page 50855 "PU Read Only Set Page" { SourceTable = "PU Rec"; Editable = false; }
page 50858 "PU Read Only Ext Page" { SourceTable = "PU Rec"; Editable = false; InsertAllowed = false; }
pageextension 50859 "PU Read Only Ext" extends "PU Read Only Ext Page"
{
    trigger OnOpenPage()
    begin
        CurrPage.Editable(true);
    end;
}
codeunit 50865 "PU Run Read Only Set"
{
    trigger OnRun()
    var
        P: Page "PU Read Only Set Page";
    begin
        P.Editable(true);
        P.Run();
    end;
}
"#;
        let g = graph_of(pages);
        let edits = op::INSERT | op::MODIFY | op::RENAME | op::VALIDATE;
        assert_eq!(blocks_of(&g, "PU Read Only Page").certain, edits);
        assert_eq!(blocks_of(&g, "PU Read Only Set Page").candidate, edits);
        assert_eq!(blocks_of(&g, "PU Read Only Set Page").certain, 0);
        let ext = blocks_of(&g, "PU Read Only Ext Page");
        assert_eq!(
            (ext.certain, ext.candidate),
            (op::INSERT, op::MODIFY | op::RENAME | op::VALIDATE),
            "InsertAllowed stays certain beside a runtime Editable"
        );
        let g = graph_of(&format!(
            "{pages}codeunit 50870 \"PU Any\" {{ trigger OnRun() begin Undeclared.Editable(true); end; }}\n"
        ));
        assert_eq!(blocks_of(&g, "PU Read Only Page").candidate, edits);
    }

    /// A property blocks only when every `#if` arm writes it `false`.
    #[test]
    fn a_property_with_a_true_arm_blocks_nothing() {
        let src = "page 50101 \"S Arm\"\n{\n    SourceTable = \"S Rec\";\n#if CLEAN\n    DeleteAllowed = false;\n#else\n    DeleteAllowed = true;\n#endif\n    ModifyAllowed = false;\n}\n";
        let f = al_syntax::parse(src);
        let d = PageUiDecl::from_properties(&f.objects[0]);
        assert!(!d.delete_false, "{d:?}");
        assert!(d.modify_false, "{d:?}");
    }

    /// `Editable = false` does not block delete (Microsoft Learn), and a
    /// runtime setter makes its blocks candidates; `InsertAllowed = false`
    /// stays certain beside it.
    #[test]
    fn editable_blocks_edits_not_delete_and_a_setter_makes_them_candidates() {
        let b = PageUiBlocks::of(&decl(true, false, false, false), false);
        assert_eq!(
            b.certain,
            op::INSERT | op::MODIFY | op::RENAME | op::VALIDATE
        );
        assert!(!b.blocks(op::DELETE));
        let b = PageUiBlocks::of(&decl(true, true, false, false), true);
        assert_eq!(b.certain, op::INSERT);
        assert_eq!(b.candidate, op::MODIFY | op::RENAME | op::VALIDATE);
    }
}

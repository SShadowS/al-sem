//! The owned AL syntax IR — the public surface `al-syntax` exposes to the engine.
//!
//! Produced once by the lowerer from the tree-sitter CST; the engine consumes only
//! this (never `tree_sitter::Node`, raw kinds, or queries). It models AL at the
//! syntactic level the analyzer reasons about — NOT a resolver HIR (that stays in
//! the engine). Every node carries an [`Origin`] back to source for byte-identical
//! anchors and traceable findings.
//!
//! Storage is a push-only arena ([`Ir`]) of `Expr`/`Stmt`/`Block`; nodes reference
//! each other by `Copy` newtype ids. No node is ever removed and ids are never
//! reused, so an id is stable for the life of one `AlFile`.

mod decl;
mod expr;
mod stmt;

pub use decl::{
    ActionDecl, AttributeIr, DatasetItemDecl, DatasetItemKind, EnumValueDecl, FieldDecl,
    LabelValue, ObjectDecl, ObjectKind, ObjectProperty, PageControl, Param, REQUEST_PAGE_MEMBER,
    RoutineDecl, RoutineKind, RoutineRare, SHARED_TYPE_TEXT_CAP, SourceComment, TypeText, VarDecl,
    al_string_literal_text, shared_type_text,
};
pub use expr::{BinaryOp, Expr, ExprKind, Literal, TypeOp, UnaryOp};
pub use stmt::{Block, BlockItem, CaseBranch, PreprocGroup, Stmt, StmtKind};

use std::ops::Range;

/// A source position. Our own type — `tree_sitter::Point` never crosses the IR
/// boundary. `column` is a UTF-8 byte column within the line (matches the engine's
/// existing anchor column semantics).
#[derive(Copy, Clone, PartialEq, Eq, Debug, Hash, serde::Serialize, serde::Deserialize)]
pub struct Point {
    pub row: u32,
    pub column: u32,
}

/// Provenance of an IR node: where it came from in source, for anchors + findings.
///
/// `PartialEq`/`Eq` are structural over every field, so a test that round-trips an
/// `Origin` through a wire format can assert equality TOTALLY rather than
/// field-by-field — a hand-listed comparison silently stops covering any field
/// added later (`src/program/pack`'s codec tests rely on this).
///
/// ENG-86: 28 bytes, not 48 (one per expression, statement and block: about 3.9 M on CDO). The
/// kind is the grammar's symbol id, read back through the same table `RawNode::kind_str` uses,
/// and the byte range is two `u32`s; [`Origin::kind_text`] and [`Origin::byte`] give the old
/// values. `PartialEq` and `Debug` read through them, so both behave as before.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct Origin {
    kind_id: u16,
    byte_start: u32,
    byte_end: u32,
    pub start: Point,
    pub end: Point,
}

impl Origin {
    /// From a grammar kind id (what a parsed node carries).
    #[must_use]
    pub fn from_kind_id(kind_id: u16, byte: Range<usize>, start: Point, end: Point) -> Self {
        let narrow = |b: usize| u32::try_from(b).expect("a source file under 4 GiB");
        Origin {
            kind_id,
            byte_start: narrow(byte.start),
            byte_end: narrow(byte.end),
            start,
            end,
        }
    }

    /// From a grammar kind string (pack decode, tests). A name used by several ids (a named
    /// and an anonymous `procedure`, say) takes the named one; [`PartialEq`] compares names,
    /// so the choice is invisible. Panics on a string that is no kind of the grammar: an
    /// `Origin` names a real node kind.
    #[must_use]
    pub fn new(kind_text: &str, byte: Range<usize>, start: Point, end: Point) -> Self {
        let id = *kind_table()
            .ids
            .get(kind_text)
            .unwrap_or_else(|| panic!("`{kind_text}` is not a kind of the grammar"));
        Self::from_kind_id(id, byte, start, end)
    }

    /// The raw grammar kind string, fed verbatim to anchor `syntax_kind` (parity).
    #[must_use]
    pub fn kind_text(&self) -> &'static str {
        match kind_table().names.get(usize::from(self.kind_id)) {
            Some(name) => name,
            // Ids past the symbol table: tree-sitter's builtin ERROR (65535).
            None => crate::language::static_language()
                .node_kind_for_id(self.kind_id)
                .expect("an origin's kind id is a symbol of its own grammar"),
        }
    }

    /// The grammar kind id itself ([`PartialEq`] compares the names, which two ids can
    /// share; ENG-119's oracle compares the ids).
    #[must_use]
    pub(crate) fn kind_id(&self) -> u16 {
        self.kind_id
    }

    /// The node's byte range in its source.
    #[must_use]
    pub fn byte(&self) -> Range<usize> {
        self.byte_start as usize..self.byte_end as usize
    }
}

/// The grammar's kind names by id, and ids by name (named ids first), built once.
struct KindTable {
    names: Vec<&'static str>,
    ids: std::collections::HashMap<&'static str, u16>,
}

fn kind_table() -> &'static KindTable {
    static TABLE: std::sync::OnceLock<KindTable> = std::sync::OnceLock::new();
    TABLE.get_or_init(|| {
        let lang = crate::language::static_language();
        let count = u16::try_from(lang.node_kind_count()).expect("under 65535 grammar symbols");
        let names: Vec<&'static str> = (0..count)
            .map(|id| lang.node_kind_for_id(id).unwrap_or(""))
            .collect();
        let mut ids = std::collections::HashMap::new();
        for named in [true, false] {
            // Id 0 is tree-sitter's `end` symbol, which no node carries.
            for id in 1..count {
                if lang.node_kind_is_named(id) == named && !names[usize::from(id)].is_empty() {
                    ids.entry(names[usize::from(id)]).or_insert(id);
                }
            }
        }
        ids.insert("ERROR", u16::MAX);
        KindTable { names, ids }
    })
}

impl PartialEq for Origin {
    fn eq(&self, other: &Self) -> bool {
        // Destructured, so a field added later cannot be left out of the comparison.
        let Origin {
            kind_id,
            byte_start,
            byte_end,
            start,
            end,
        } = self;
        // Two ids can share a kind name (a named and an anonymous `procedure`), so ids that
        // differ compare by name.
        (*kind_id == other.kind_id || self.kind_text() == other.kind_text())
            && *byte_start == other.byte_start
            && *byte_end == other.byte_end
            && *start == other.start
            && *end == other.end
    }
}

impl Eq for Origin {}

impl std::fmt::Debug for Origin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Origin")
            .field("kind_text", &self.kind_text())
            .field("byte", &self.byte())
            .field("start", &self.start)
            .field("end", &self.end)
            .finish()
    }
}

macro_rules! id_type {
    ($name:ident) => {
        #[derive(Copy, Clone, PartialEq, Eq, Debug, Hash, serde::Serialize, serde::Deserialize)]
        pub struct $name(u32);
        impl $name {
            #[inline]
            pub fn index(self) -> usize {
                self.0 as usize
            }
        }
    };
}
id_type!(ExprId);
id_type!(StmtId);
id_type!(BlockId);
id_type!(MemberOriginId);

/// Preprocessor symbols a build condition decides: `(symbol, defined)`.
///
/// Code inside `#if X` / `#else` / `#elif` branches, and a routine that exists only
/// under such a branch, compiles only in builds where its conditions hold. The
/// lowerer records the symbols those conditions decide (a condition true; every
/// earlier arm's false; `A and B` true decides both, `A or B` false decides both).
/// A condition that decides no symbol is recorded whole, named by its parenthesized
/// text (`("(A or B)", true)`), so code under any condition never has an empty context
/// (empty means "every build"). Symbols are case-sensitive: alc 18.0.41.45789
/// with `CLEAN27` defined activates `#if CLEAN27` and not `#if clean27`.
pub type PreprocSymbols = Vec<(String, bool)>;

/// Whether two sets of decided symbols can never hold in one build: some symbol
/// is decided defined in one and undefined in the other.
pub fn preproc_contradicts(a: &[(String, bool)], b: &[(String, bool)]) -> bool {
    a.iter()
        .any(|(s, v)| b.iter().any(|(t, w)| s == t && v != w))
}

/// Move `v` into a new allocation of exactly its length, freeing the old one.
/// Not `Vec::shrink_to_fit`: an allocator may shrink in place and keep the
/// whole block (mimalloc, the `alsem` binary's, keeps any block the new size
/// fills at least half of — always the case after doubling growth).
pub fn exact_size<T>(v: &mut Vec<T>) {
    if v.capacity() > v.len() {
        let mut exact = Vec::with_capacity(v.len());
        exact.append(v);
        *v = exact;
    }
}

/// One `#if` wrapper around variable declarations ([`Ir::var_wrappers`]): inside a `var`
/// section, around whole sections, a split section tail, or a conditional name list.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct VarWrapper {
    pub byte: Range<usize>,
    /// Each arm's byte range (between its directive and the next). Empty for a wrapper no
    /// arm-by-arm reading applies to (a conditional name list): never covering.
    pub arms: Vec<Range<usize>>,
    /// Whether one arm is an `#else`, so the arms cover every build.
    pub has_else: bool,
}

impl VarWrapper {
    /// Whether every build declares a name here: an `#else` arm, and every arm holds one of
    /// the declarations starting at `starts`.
    pub fn covered_by(&self, starts: &[usize]) -> bool {
        self.has_else
            && !self.arms.is_empty()
            && self
                .arms
                .iter()
                .all(|a| starts.iter().any(|s| a.contains(s)))
    }
}

/// Push-only arena holding the `Expr`/`Stmt`/`Block` pools an `AlFile` references.
#[derive(Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Ir {
    exprs: Vec<Expr>,
    stmts: Vec<Stmt>,
    blocks: Vec<Block>,
    /// ENG-86: each `ExprKind::Member`'s member-name origin, out of the enum (one per
    /// member expression; inline it doubled every expression's size).
    member_origins: Vec<Origin>,
    /// Expression id ranges lowered under a non-empty build context, each with
    /// that whole context (enclosing branches and routine arm together). Ranges
    /// nest; [`Ir::preproc_context`] picks the innermost.
    preproc_ranges: Vec<(Range<u32>, PreprocSymbols)>,
    /// ENG-80 (spec §4.2 C): call expressions whose callee or arguments hold a
    /// build-varying `#if` node, or built from a split call statement. The lowerer
    /// reads one arm there, so the call's arity and argument kinds may differ in
    /// another build. Pushed in expression order (sorted).
    tainted_calls: Vec<u32>,
    /// ENG-80 (spec §4.2 C): routine bodies (by block id) whose routine's declared
    /// locals, or whose object's globals, the lowerer did not collect in full (a
    /// split body's arm `var` sections; a `var` `#if` nested deeper than one level).
    locals_incomplete: Vec<u32>,
    globals_incomplete: Vec<u32>,
    /// ENG-80 (sol review): the `#if` wrappers whose declarations a `var` section lists
    /// (every arm flattened), as [`VarWrapper`]s: a declaration inside one exists only in
    /// the builds of its arm.
    conditional_vars: Vec<VarWrapper>,
    /// ENG-80 (sol review): routine bodies (by block id) whose signature (parameters or
    /// return clause) varies by `#if`; the lowerer read one arm of it.
    signature_varies: Vec<u32>,
}

impl Ir {
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether call `e`'s callee or arguments vary by `#if` build (ENG-80): its
    /// arity and argument kinds prove nothing about which overload every build calls.
    pub fn call_is_tainted(&self, e: ExprId) -> bool {
        self.tainted_calls.binary_search(&e.0).is_ok()
    }

    pub(crate) fn mark_tainted(&mut self, e: ExprId) {
        if let Err(i) = self.tainted_calls.binary_search(&e.0) {
            self.tainted_calls.insert(i, e.0);
        }
    }

    /// Whether the routine with body `body` lists every local it declares in every
    /// build (`RoutineDecl::locals`). ENG-80: a Text proof reads the list only when so.
    pub fn locals_complete(&self, body: BlockId) -> bool {
        !self.locals_incomplete.contains(&body.0)
    }

    /// Whether the object of the routine with body `body` lists every global it
    /// declares in every build (`ObjectDecl::globals`).
    pub fn globals_complete(&self, body: BlockId) -> bool {
        !self.globals_incomplete.contains(&body.0)
    }

    pub(crate) fn mark_locals_incomplete(&mut self, body: BlockId) {
        self.locals_incomplete.push(body.0);
    }

    pub(crate) fn mark_globals_incomplete(&mut self, body: BlockId) {
        self.globals_incomplete.push(body.0);
    }

    /// Every `#if` wrapper that holds the variable declared at `origin` (it then exists only
    /// in the builds of its arms, ENG-80).
    pub fn var_wrappers<'a>(&'a self, origin: &'a Origin) -> impl Iterator<Item = &'a VarWrapper> {
        self.conditional_vars
            .iter()
            .filter(|w| w.byte.contains(&origin.byte().start))
    }

    /// [`Ir::var_wrappers`] as seen from the routine declared at `routine`: a wrapper whose
    /// same arm holds both the routine and the variable is left out, since the variable
    /// exists in every build that has the routine (the usual `#if not CLEANxx` around an
    /// obsolete procedure, Fable r6). A routine in another arm does not see the variable.
    pub fn var_wrappers_from<'a>(
        &'a self,
        origin: &'a Origin,
        routine: &'a Origin,
    ) -> impl Iterator<Item = &'a VarWrapper> {
        self.var_wrappers(origin).filter(move |w| {
            !w.arms
                .iter()
                .any(|a| a.contains(&origin.byte().start) && a.contains(&routine.byte().start))
        })
    }

    pub(crate) fn mark_conditional_vars(&mut self, wrappers: Vec<VarWrapper>) {
        self.conditional_vars.extend(wrappers);
    }

    /// Whether the signature (parameters, return clause) of the routine with body `body` is
    /// the same in every build. When not, the lowerer read one arm (ENG-80).
    pub fn signature_complete(&self, body: BlockId) -> bool {
        !self.signature_varies.contains(&body.0)
    }

    pub(crate) fn mark_signature_varies(&mut self, body: BlockId) {
        self.signature_varies.push(body.0);
    }

    /// Drop the arenas' spare capacity (a finished lowering adds nothing).
    pub fn shrink_to_fit(&mut self) {
        exact_size(&mut self.exprs);
        exact_size(&mut self.stmts);
        exact_size(&mut self.blocks);
        exact_size(&mut self.member_origins);
        exact_size(&mut self.preproc_ranges);
        exact_size(&mut self.tainted_calls);
    }

    pub fn add_expr(&mut self, e: Expr) -> ExprId {
        let id = ExprId(u32::try_from(self.exprs.len()).expect("expr arena overflow"));
        self.exprs.push(e);
        id
    }
    pub fn add_stmt(&mut self, s: Stmt) -> StmtId {
        let id = StmtId(u32::try_from(self.stmts.len()).expect("stmt arena overflow"));
        self.stmts.push(s);
        id
    }
    pub fn add_block(&mut self, b: Block) -> BlockId {
        let id = BlockId(u32::try_from(self.blocks.len()).expect("block arena overflow"));
        self.blocks.push(b);
        id
    }

    #[inline]
    pub fn expr(&self, id: ExprId) -> &Expr {
        &self.exprs[id.index()]
    }
    #[inline]
    pub fn stmt(&self, id: StmtId) -> &Stmt {
        &self.stmts[id.index()]
    }
    #[inline]
    pub fn block(&self, id: BlockId) -> &Block {
        &self.blocks[id.index()]
    }

    pub fn add_member_origin(&mut self, o: Origin) -> MemberOriginId {
        let id = MemberOriginId(
            u32::try_from(self.member_origins.len()).expect("member origin arena overflow"),
        );
        self.member_origins.push(o);
        id
    }

    /// The member-name origin of an `ExprKind::Member` (its `member_origin` id).
    #[inline]
    pub fn member_origin(&self, id: MemberOriginId) -> &Origin {
        &self.member_origins[id.index()]
    }

    pub fn iter_exprs(&self) -> impl Iterator<Item = &Expr> {
        self.exprs.iter()
    }

    pub fn iter_stmts(&self) -> impl Iterator<Item = &Stmt> {
        self.stmts.iter()
    }

    pub fn expr_count(&self) -> usize {
        self.exprs.len()
    }

    /// Record that the expressions added since `first` (an earlier
    /// [`Ir::expr_count`]) were lowered under build context `ctx`. Nothing is
    /// recorded for an empty context or an empty range.
    pub fn record_preproc_context(&mut self, first: usize, ctx: &[(String, bool)]) {
        let (start, end) = (first as u32, self.exprs.len() as u32);
        if !ctx.is_empty() && start < end {
            self.preproc_ranges.push((start..end, ctx.to_vec()));
        }
    }

    /// The build context expression `id` was lowered under (empty: none).
    pub fn preproc_context(&self, id: ExprId) -> &[(String, bool)] {
        self.preproc_ranges
            .iter()
            .filter(|(r, _)| r.contains(&id.0))
            .min_by_key(|(r, _)| r.len())
            .map_or(&[], |(_, ctx)| ctx.as_slice())
    }
    pub fn stmt_count(&self) -> usize {
        self.stmts.len()
    }
    pub fn block_count(&self) -> usize {
        self.blocks.len()
    }
}

/// Whether the parse was clean or hit tree-sitter error recovery.
#[derive(Copy, Clone, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub enum ParseStatus {
    /// No `ERROR`/`MISSING` nodes anywhere.
    Clean,
    /// Recovery nodes present — the IR is partial. The dependency IR cache (ENG-110) stores
    /// it like any other IR and replays it exactly: the status is part of the cached `AlFile`.
    Recovered,
}

/// A syntactic problem the lowerer recorded (recovery node, or an `Unknown` it
/// could not classify). Never silently dropped — surfaced as data.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SyntaxIssue {
    pub message: String,
    pub origin: Origin,
}

/// The lowered representation of one AL source file.
#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct AlFile {
    pub objects: Vec<ObjectDecl>,
    pub ir: Ir,
    pub issues: Vec<SyntaxIssue>,
    pub parse_status: ParseStatus,
    /// Every comment in the file, in source order, as written. Comments are
    /// grammar `extras`, so the parse tree hangs them on whatever node is open,
    /// not on the declaration they describe; position (line numbers) is what
    /// ties a comment to code. The `doc` runs on routines, fields and actions are
    /// read from this list, and a later pass reads a check's trailing comment
    /// (`own_line == false`) or the comment above it from here.
    pub comments: Vec<SourceComment>,
}

#[cfg(test)]
mod exact_size_tests {
    /// ENG-102, through `parse`: equal declared types share one text (dropping
    /// `shared_type_text` at a site gives two copies), a routine with no rare field
    /// carries no box (an always-boxed `RoutineRare` fails it) and one with a doc or a
    /// named return does, read back through the accessors.
    #[test]
    fn parsed_declarations_share_type_texts_and_box_only_rare_fields() {
        let file = crate::parse(
            "codeunit 50103 \"Sh\"\n{\n    var\n        G: Record Customer;\n\n    procedure Plain(P: Record Customer)\n    var\n        L: Record Customer;\n    begin\n    end;\n\n    /// Says hello.\n    procedure Named() Ret: Record Customer\n    begin\n    end;\n\n    // Only a doc.\n    procedure DocOnly()\n    begin\n    end;\n}\n",
        );
        let o = &file.objects[0];
        let (plain, named, doc_only) = (&o.routines[0], &o.routines[1], &o.routines[2]);
        // A doc alone boxes too (`set_doc` on a routine that has no box yet; sol, ENG-102).
        assert_eq!(doc_only.doc(), Some("Only a doc."));
        assert!(doc_only.return_name().is_none());
        let texts = [
            o.globals[0].ty.as_ref(),
            plain.params[0].ty.as_ref(),
            plain.locals[0].ty.as_ref(),
            named.return_type.as_ref(),
        ]
        .map(|t| t.expect("every declaration here has a type").clone());
        for t in &texts[1..] {
            assert!(
                std::sync::Arc::ptr_eq(&texts[0], t),
                "equal type texts share one allocation: {t}"
            );
        }
        assert!(plain.rare.is_none(), "no rare field: no box");
        assert_eq!(named.return_name(), Some("Ret"));
        assert_eq!(named.doc(), Some("Says hello."));
        assert!(named.preproc_context().is_empty() && named.dataitem_source_table().is_none());
    }

    /// ENG-94: a parsed file's declaration lists keep no spare capacity. Lowering pushes
    /// into them (a first push reserves four elements); `lower_file` trims every one, the
    /// nested ones included. Pinned through `parse`, so dropping the call fails this.
    #[test]
    fn parsed_declarations_keep_no_spare_capacity() {
        let file = crate::parse(
            "codeunit 50100 \"Cap\"\n{\n    var\n        G: Integer;\n\n    procedure One(P: Integer)\n    var\n        L: Text;\n    begin\n    end;\n\n    [EventSubscriber(ObjectType::Codeunit, 1, 'X', '', false, false)]\n    local procedure Two()\n    begin\n    end;\n}\n",
        );
        let o = &file.objects[0];
        let exact = |what: &str, len: usize, cap: usize| {
            assert_eq!(cap, len, "{what}: capacity {cap} for {len} elements");
        };
        exact("routines", o.routines.len(), o.routines.capacity());
        exact("globals", o.globals.len(), o.globals.capacity());
        for r in &o.routines {
            exact("params", r.params.len(), r.params.capacity());
            exact("locals", r.locals.len(), r.locals.capacity());
            exact("attributes", r.attributes.len(), r.attributes.capacity());
            exact(
                "attributes_parsed",
                r.attributes_parsed.len(),
                r.attributes_parsed.capacity(),
            );
        }
        assert_eq!(o.routines.len(), 2, "the fixture has two routines");
        assert_eq!(o.routines[0].locals.len(), 1, "and a local in the first");
        for a in &o.routines[1].attributes_parsed {
            exact("attribute args", a.args.len(), a.args.capacity());
        }

        // The object-level and nested lists (lowerer review): a table's fields, captions and
        // keys, a page's actions, a label global's translations.
        let file = crate::parse(
            "table 50101 \"Cap T\"\n{\n    fields\n    {\n        field(1; Code; Code[20]) { Caption = 'Code'; ToolTip = 'The code.'; }\n        field(2; Name; Text[50]) { Caption = 'Name'; }\n    }\n    keys\n    {\n        key(PK; Code) { Clustered = true; }\n        key(K2; Name, Code) { }\n    }\n    var\n        Msg: Label 'Hello %1', Comment = '%1 = name', Locked = true, Foo = 3;\n        Tc: TextConst DEU = 'Hallo', ENU = 'Hello', FRA = 'Salut';\n}\npage 50102 \"Cap P\"\n{\n    actions\n    {\n        area(Processing)\n        {\n            action(Go) { Caption = 'Go'; ToolTip = 'Go now.'; trigger OnAction() begin end; }\n        }\n    }\n}\n",
        );
        let t = &file.objects[0];
        exact("fields", t.fields.len(), t.fields.capacity());
        exact("keys", t.keys.len(), t.keys.capacity());
        for k in &t.keys {
            exact("key fields", k.len(), k.capacity());
        }
        for f in &t.fields {
            exact("field caption", f.caption.len(), f.caption.capacity());
            exact("field tooltip", f.tooltip.len(), f.tooltip.capacity());
        }
        for g in &t.globals {
            if let Some(l) = &g.label {
                exact("label other", l.other.len(), l.other.capacity());
                exact(
                    "label translations",
                    l.translations.len(),
                    l.translations.capacity(),
                );
            }
        }
        let p = &file.objects[1];
        exact("actions", p.actions.len(), p.actions.capacity());
        for a in &p.actions {
            exact("action caption", a.caption.len(), a.caption.capacity());
            exact("action tooltip", a.tooltip.len(), a.tooltip.capacity());
        }
        // sol: the label lists must be non-empty, or their checks prove nothing. They pin the
        // property, not `VarDecl::exact_size`: the lowerer clones each label per declared
        // name, and a clone is exact, so dropping that trim stays green (a stated limit).
        let labels: Vec<_> = t.globals.iter().filter_map(|g| g.label.as_ref()).collect();
        assert!(
            labels.iter().any(|l| !l.other.is_empty()),
            "a label with `other`"
        );
        assert!(
            labels.iter().any(|l| !l.translations.is_empty()),
            "a TextConst with translations"
        );
        assert_eq!(t.fields.len(), 2, "the table has two fields");
        assert_eq!(t.keys.len(), 2, "and two keys");
        assert!(!p.actions.is_empty(), "the page has an action");
    }
}

#[cfg(test)]
mod origin_tests {
    use super::*;

    /// ENG-86: the slim layout (28 bytes, was 48).
    #[test]
    fn origin_is_28_bytes() {
        assert_eq!(std::mem::size_of::<Origin>(), 28);
        // With `member_origin` out of the enum: was 128 (ExprKind 80 + Origin 48).
        assert_eq!(std::mem::size_of::<Expr>(), 72);
    }

    /// ENG-86: a name with a named and an anonymous id (`procedure`) gives equal origins
    /// either way, and every kind name of the grammar builds an origin that reads it back.
    #[test]
    fn kind_names_round_trip_and_shared_names_compare_equal() {
        let lang = crate::language::static_language();
        let (named, anon) = (
            lang.id_for_node_kind("procedure", true),
            lang.id_for_node_kind("procedure", false),
        );
        assert!(
            named != 0 && anon != 0 && named != anon,
            "two `procedure` ids"
        );
        let p = Point { row: 0, column: 0 };
        let a = Origin::from_kind_id(anon, 0..9, p, p);
        let n = Origin::new("procedure", 0..9, p, p);
        assert_eq!(n.kind_id, named);
        assert_eq!(a, n);
        assert_ne!(a, Origin::new("comment", 0..9, p, p));
        for id in 1..u16::try_from(lang.node_kind_count()).unwrap() {
            let Some(name) = lang.node_kind_for_id(id) else {
                continue;
            };
            if name.is_empty() {
                continue;
            }
            let o = Origin::from_kind_id(id, 0..1, p, p);
            assert_eq!(o.kind_text(), name);
            assert_eq!(Origin::new(name, 0..1, p, p).kind_text(), name);
        }
        assert_eq!(Origin::new("ERROR", 0..1, p, p).kind_text(), "ERROR");
    }

    /// ENG-86: a member expression's member-name origin, now in the side table, is the
    /// member node's own (kind and bytes), one per member expression.
    #[test]
    fn member_origin_is_the_member_node() {
        let src = "codeunit 50100 C\n{\n    procedure P()\n    begin\n        if Rec.\"No.\" = A.B then;\n    end;\n}\n";
        let file = crate::parse(src);
        let members: Vec<_> = file
            .ir
            .iter_exprs()
            .filter_map(|e| match &e.kind {
                ExprKind::Member {
                    member,
                    member_origin,
                    ..
                } => Some((member.clone(), file.ir.member_origin(*member_origin))),
                _ => None,
            })
            .collect();
        assert_eq!(members.len(), 2);
        for (member, o) in members {
            assert_eq!(&src[o.byte()], member);
            let want = if member.starts_with('"') {
                "quoted_identifier"
            } else {
                "identifier"
            };
            assert_eq!(o.kind_text(), want, "{member}");
        }
    }

    /// ENG-86: every origin a parse makes reads back the node's own kind string and byte
    /// range, and a kind string turns back into an equal origin.
    #[test]
    fn origin_reads_back_kind_and_bytes() {
        let src = "codeunit 50100 C\n{\n    procedure P()\n    begin\n        Message('x', Rec.\"No.\");\n    end;\n}\n";
        let file = crate::parse(src);
        let mut seen = 0;
        for e in file.ir.iter_exprs() {
            let o = &e.origin;
            let again = Origin::new(o.kind_text(), o.byte(), o.start, o.end);
            assert_eq!(&again, o);
            assert!(o.byte().end <= src.len() && o.byte().start <= o.byte().end);
            seen += 1;
        }
        assert!(seen >= 4, "only {seen} expressions");
        let call = file
            .ir
            .iter_exprs()
            .find(|e| matches!(e.kind, ExprKind::Call { .. }));
        let call = call.expect("a call");
        assert_eq!(call.origin.kind_text(), "call_expression");
        assert_eq!(&src[call.origin.byte()], "Message('x', Rec.\"No.\")");
    }
}

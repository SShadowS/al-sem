//! IR declarations: objects, routines, parameters, variables.
//!
//! Types are kept as raw strings here (e.g. `"Record Customer"`, `"Code[20]"`);
//! the engine parses/resolves them (that is resolver work, not syntax). `globals`
//! and `locals` include declarations from BOTH `#if`/`#else` branches.

use super::{BlockId, Origin};

pub struct ObjectDecl {
    pub kind: ObjectKind,
    /// Object number where the grammar provides one (codeunit/table/page/...).
    pub id: Option<i64>,
    pub name: String,
    pub routines: Vec<RoutineDecl>,
    pub globals: Vec<VarDecl>,
    /// The names (unquoted, as written) of the [`Self::globals`] declared in a
    /// `protected var` section, document order: an extension of this object can
    /// read them.
    pub protected_globals: Vec<String>,
    /// Object-level properties (`SourceTable`, `TableNo`, `PageType`, …) in source
    /// order. Needed by the engine to seed implicit-`Rec` table resolution and object
    /// classification; the value is the raw value text (trimmed).
    pub properties: Vec<ObjectProperty>,
    /// Report `dataitem(Name; "Source Table")` and XmlPort `tableelement(Name; Table)`
    /// declarations (name, source-table), both unquoted, document order —
    /// Report/ReportExtension/XmlPort only (empty otherwise). The name is in scope as a
    /// record var across ALL the object's routines, so
    /// the engine seeds each as a record variable in every routine. Distinct from a
    /// dataitem trigger's per-dataitem implicit `Rec` (see
    /// [`RoutineDecl::dataitem_source_table`]).
    pub dataitems: Vec<(String, String)>,
    /// The [`Self::dataitems`] names (as declared) whose own properties set
    /// `UseTemporary = true`: the record behind the name is temporary.
    pub temporary_dataitems: Vec<String>,
    /// Query `column(Name; "Source Field")` declarations with no `Method` property, as
    /// (column name, enclosing dataitem's source table, source field), all unquoted,
    /// document order — Query only (empty otherwise). Such a column has its source
    /// field's type (alc 18.0.41.45789: assigning an Enum-field column to a `Date`
    /// fails AL0122 naming the enum); a `Method` column's type is not modelled.
    pub query_columns: Vec<(String, String, String)>,
    /// The tables the platform reads or writes by itself when the object runs (B6):
    /// every report `dataitem` (Report / ReportExtension), XmlPort `tableelement` and
    /// query `dataitem`, nested ones included, `#if` arms included (every arm),
    /// document order. Empty for every other object kind.
    pub dataset_items: Vec<DatasetItemDecl>,
    /// The `extends <Target>` target name (unquoted) for an extension object
    /// (Table/Page/Report/Enum/PermissionSet extension), else `None`.
    pub extends_target: Option<String>,
    /// Interface names from an `implements` clause (unquoted, document order) — for
    /// Codeunit / Enum / Interface objects; empty otherwise.
    pub implements: Vec<String>,
    /// Page controls (`part` / `systempart` / `usercontrol` sections) in document
    /// order — Page / PageExtension only. Resolves `CurrPage.<control>…` member calls.
    pub page_controls: Vec<PageControl>,
    /// Table fields (Table / TableExtension only), document order. The engine assigns
    /// the internal/stable ids; the IR carries the raw extracted shape.
    pub fields: Vec<FieldDecl>,
    /// Table keys (Table / TableExtension only) — each the member field NAMES (unquoted,
    /// lowercased) in declaration order; the engine resolves them to field ids.
    pub keys: Vec<Vec<String>>,
    /// The raw value of every `RunObject` property anywhere in the object outside
    /// code (page and page-extension actions, report-extension and other members),
    /// one entry per `#if` arm, document order: `Page "Customer Card"`,
    /// `Codeunit 50100`, … A user or a `TestPage` action opens that object.
    pub run_objects: Vec<String>,
    /// The `value(<n>; <Name>)` declarations of an Enum or EnumExtension
    /// object, document order, values inside `#if` wrappers included (every
    /// arm). Empty for every other object kind.
    pub enum_values: Vec<EnumValueDecl>,
    /// Every page action declared anywhere in the object outside code, document
    /// order: `action`, `systemaction`, `fileuploadaction` and `customaction`
    /// declarations, nested in `area(…)` / `group(…)`, in page-extension `add*(…)`
    /// blocks, in `#if` arms (every arm) and in a report's request page. A
    /// page-extension `modify(X)` block is not a declaration and gives no entry.
    pub actions: Vec<ActionDecl>,
    pub origin: Origin,
}

/// What declares a [`DatasetItemDecl`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DatasetItemKind {
    /// A report or report-extension `dataitem(Name; Table)`.
    ReportDataitem,
    /// An XmlPort `tableelement(Name; Table)`.
    XmlPortTableElement,
    /// A query `dataitem(Name; Table)`.
    QueryDataitem,
}

/// One table of an object's dataset (see [`ObjectDecl::dataset_items`]). Each
/// property is `Some(value)` when every build gives it that value (the AL default
/// when it is not written) and `None` when `#if` arms give it different values or
/// the value is not `true`/`false`.
pub struct DatasetItemDecl {
    pub kind: DatasetItemKind,
    /// The item name, unquoted, as written.
    pub name: String,
    /// The source table, unquoted, as written (a name or a number).
    pub table: String,
    /// `UseTemporary` (report dataitem, XmlPort table element; default `false`).
    pub use_temporary: Option<bool>,
    /// XmlPort table element `AutoSave` (default `true`); `Some(false)` otherwise.
    pub auto_save: Option<bool>,
    /// XmlPort table element `AutoUpdate` (default `false`).
    pub auto_update: Option<bool>,
    /// XmlPort table element `AutoReplace` (default `false`).
    pub auto_replace: Option<bool>,
    pub origin: Origin,
}

/// One page action declaration (`action(Name) { … }` and its kin, see
/// [`ObjectDecl::actions`]): the user gesture, what it says to the user, and the
/// code it runs.
pub struct ActionDecl {
    /// The action name, unquoted, as written.
    pub name: String,
    /// The raw grammar keyword: `action`, `systemaction`, `fileuploadaction` or
    /// `customaction`.
    pub kind: &'static str,
    /// `Caption` text(s), one per textually present `#if` arm (see
    /// [`FieldDecl::caption`] for the text rule).
    pub caption: Vec<String>,
    /// `ToolTip` text(s), same rule.
    pub tooltip: Vec<String>,
    /// The raw `RunObject` value(s), one per `#if` arm, as in
    /// [`ObjectDecl::run_objects`]: `Page "Customer Card"`, `Codeunit 50100`.
    pub run_object: Vec<String>,
    /// Indexes into [`ObjectDecl::routines`] of the triggers declared in this
    /// action (its `OnAction`, one per `#if` arm when the trigger is in one).
    /// Linked by the routine's [`RoutineDecl::enclosing_member`] range.
    pub triggers: Vec<usize>,
    /// The comment run directly above the action (see [`RoutineDecl::doc`]).
    pub doc: Option<String>,
    pub origin: Origin,
}

/// One comment in the source file, as written (see [`super::AlFile::comments`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceComment {
    /// The comment text with its markers (`// x`, `/// x`, `/* x */`).
    pub text: String,
    /// `true` for a `/* … */` comment, `false` for a `//` line comment.
    pub block: bool,
    /// `true` when nothing but white space comes before the comment on its first
    /// line. A `//` comment after code on the same line (a trailing comment) is
    /// `false`.
    pub own_line: bool,
    pub origin: Origin,
}

/// One enum `value(<n>; <Name>) { … }` declaration.
pub struct EnumValueDecl {
    /// The value name, unquoted, as written.
    pub name: String,
    /// The raw `Implementation` property value (`"I" = "Impl A", "J" = "Impl B"`),
    /// trimmed, one entry per textually present `#if` arm around the property or
    /// its whole value; empty = none declared. An `#if` BETWEEN list elements
    /// (`"I" = "P" #if X , "J" = "Q" #endif`) is not split: that entry keeps the
    /// directive lines in its text, so a consumer must treat text holding `#` as
    /// unreadable (fail open), not parse past it.
    pub implementations: Vec<String>,
    pub origin: Origin,
}

/// A page control section (`part` / `systempart` / `usercontrol`).
pub struct PageControl {
    pub name: String,
    /// Raw kind: `"part"` / `"systempart"` / `"usercontrol"`.
    pub kind: String,
    pub target: String,
}

/// A table `field(<no>; <Name>; <Type>) { ... }` declaration. `data_type` is the raw
/// type text; `field_class` is `Normal` / `FlowField` / `FlowFilter` (from the
/// `FieldClass` property); `is_blob_like` flags Blob/Media/MediaSet.
pub struct FieldDecl {
    pub number: i64,
    pub name: String,
    pub data_type: String,
    pub field_class: String,
    pub is_blob_like: bool,
    /// The field's `ObsoleteState` value(s) as written (`Pending`, `Removed`, …),
    /// one per textually present `#if` arm; empty = the field declares none.
    /// More than one = the state differs by preprocessor branch: a consumer must
    /// not pick one (the lowerer's superset rule, see `collect_properties`).
    pub obsolete_state: Vec<String>,
    /// The field's `ObsoleteReason` text(s), quotes stripped, same arm rule.
    pub obsolete_reason: Vec<String>,
    /// The field's `Caption` text(s), one per textually present `#if` arm. Each is
    /// the property's first string literal, quotes removed and every doubled `''`
    /// read as one `'` (`Caption = 'It''s', Comment = 'c';` gives `It's`). An arm
    /// whose value does not start with a plain literal gives no entry.
    pub caption: Vec<String>,
    /// The field's `ToolTip` text(s), same rule.
    pub tooltip: Vec<String>,
    /// The field's `OptionCaption` text(s), same rule (the whole comma list as one
    /// string: `A,B`).
    pub option_caption: Vec<String>,
    /// The field's `CalcFormula` value text(s) as written (`sum("Cust. Ledger
    /// Entry".Amount where(...))`), one per textually present `#if` arm; empty
    /// = none declared. Read by the FlowField read rule (engine limits B5).
    pub calc_formula: Vec<String>,
    /// The comment run directly above the field (see [`RoutineDecl::doc`]).
    pub doc: Option<String>,
    /// The field declaration node's origin (for a field whose header is split
    /// across `#if` arms, the split node's, which starts at the `#if`).
    pub origin: Origin,
}

/// A single object-level `property` node (`name = value`). `name` is lowercased;
/// `value` is the raw value text (quotes preserved — the engine strips as needed).
pub struct ObjectProperty {
    pub name: String,
    pub value: String,
    pub origin: Origin,
    /// `true` when only some builds have this entry (engine limits B2; B6
    /// round 2 added the same flag for an XmlPort's `Direction`): the
    /// property sits in an `#if` arm, or its whole value is an `#if`
    /// (`TableNo = #if X Customer; #endif`). A build that takes another arm, or
    /// none, has another value or the property's default. No configuration is
    /// chosen, so a reader that decides a definite answer from the value must
    /// treat a conditional entry as "may": the default stays possible.
    pub conditional: bool,
}

/// A parsed routine attribute (`[EventSubscriber(ObjectType::Codeunit, …)]`).
/// `name` is the raw attribute name; `raw` the full `attribute_item` text; `args`
/// the lowered argument exprs (the engine projects each to kind/text/value/…).
pub struct AttributeIr {
    pub name: String,
    pub raw: String,
    pub args: Vec<super::ExprId>,
    /// The `attribute_item` node's origin (#71: the routine's declaration span starts
    /// at its first attribute).
    pub origin: super::Origin,
    /// `true` when some build in which THIS declaration of the routine exists
    /// does not have this attribute with these arguments (engine limits B2):
    /// it sits in an `#if` arm the declaration is not in (`#if CLEAN99
    /// [CommitBehavior(…)] #endif procedure P()`), it reached the routine past a
    /// `#if` wrapper by a path not every build takes, or its arguments hold an
    /// `#if` group. No configuration is chosen, so every such build stays
    /// possible: a reader that decides a definite answer from the attribute's
    /// presence must read a conditional one as "may"
    /// (`program::attributes::find_definite_attribute`). Imprecise in one
    /// direction only: an attribute whose arms together cover every build
    /// (`#if X [A] #else [A] #endif`) is still conditional.
    pub conditional: bool,
    /// `true` when the routine is declared more than once (two `#if` arms of a
    /// member list, of a whole object, or two files) and ANOTHER declaration
    /// does not carry the same attribute (engine limits B2, wave 2 final
    /// review). It says nothing about this declaration's own builds: in every
    /// build in which this declaration exists the attribute holds (unless
    /// [`Self::conditional`]). So a reader judging this declaration's own body
    /// reads it as definite, and a reader judging the routine from outside (a
    /// caller asking "is this a TryFunction") reads it as "may". Set here for
    /// the declarations of one object declaration; the model sets it across
    /// object declarations and files.
    pub another_declaration_lacks: bool,
}

#[derive(Copy, Clone, PartialEq, Eq, Debug, Hash, Ord, PartialOrd)]
pub enum ObjectKind {
    Codeunit,
    Table,
    TableExtension,
    Page,
    PageExtension,
    Report,
    ReportExtension,
    Query,
    XmlPort,
    Enum,
    EnumExtension,
    Interface,
    ControlAddIn,
    Entitlement,
    PermissionSet,
    PermissionSetExtension,
    Profile,
    /// Any other object construct, kind preserved via `Origin::kind_text`.
    Other,
}

impl ObjectKind {
    /// True for the four AL extension kinds that carry procedures and can
    /// therefore reach a base object's `protected` members: `TableExtension`,
    /// `PageExtension`, `ReportExtension`, `EnumExtension`. `PermissionSetExtension`
    /// is deliberately excluded — permission sets carry no procedures/`Access`
    /// modifiers, so kind-compatible `Protected` visibility never applies to it.
    /// Consumed by the resolve engine's `ResolveIndex::object_extends` identity
    /// check (`src/program/resolve/index.rs`).
    #[must_use]
    pub fn is_extension_kind(self) -> bool {
        self.extension_base_kind().is_some()
    }

    /// The base object kind this extension kind extends: `TableExtension` →
    /// `Table`, `PageExtension` → `Page`, `ReportExtension` → `Report`,
    /// `EnumExtension` → `Enum`. `None` for a non-extension kind (see
    /// [`Self::is_extension_kind`]).
    #[must_use]
    pub fn extension_base_kind(self) -> Option<ObjectKind> {
        match self {
            ObjectKind::TableExtension => Some(ObjectKind::Table),
            ObjectKind::PageExtension => Some(ObjectKind::Page),
            ObjectKind::ReportExtension => Some(ObjectKind::Report),
            ObjectKind::EnumExtension => Some(ObjectKind::Enum),
            _ => None,
        }
    }
}

pub struct RoutineDecl {
    pub kind: RoutineKind,
    pub name: String,
    /// Origin of the routine's NAME identifier node (not the whole routine). The LSP
    /// front-end uses this for a call-hierarchy item's `selection_range` (the range
    /// the editor highlights when you click the symbol) — e.g. an event publisher's
    /// procedure-name range. Falls back to the routine `origin` if the name is absent
    /// (a malformed/anonymous routine).
    pub name_origin: Origin,
    pub params: Vec<Param>,
    /// Return type text, if declared.
    pub return_type: Option<TypeText>,
    pub locals: Vec<VarDecl>,
    /// Attribute names (lowercased) from the `attribute_item` siblings preceding the
    /// routine — e.g. "eventsubscriber", "tryfunction", "integrationevent". Drives
    /// routine-kind classification + control-context guards.
    pub attributes: Vec<String>,
    /// Full parsed attributes (name + raw text + lowered argument exprs), in source
    /// order — for the L2 `attributes` / `attributesParsed` envelope. The engine
    /// renders each arg via its expression-info projection.
    pub attributes_parsed: Vec<AttributeIr>,
    /// Access modifier keyword (lowercased: "local"/"internal"/"protected"), or None
    /// for a public procedure / a trigger. Mirrors the `modifier` field.
    pub access_modifier: Option<String>,
    /// `true` when this routine's subtree contains a parse error (tree-sitter
    /// `has_error`). Mirrors the legacy `parseIncomplete` / drives the IR-vs-legacy
    /// feature-extraction choice (malformed routines use legacy ERROR-recovery).
    pub parse_incomplete: bool,
    /// For a MEMBER-trigger (a trigger nested in a named member: table/page field,
    /// page part, action, report dataitem, query element, …), the enclosing member's
    /// name (outer-quote-stripped — the engine applies `unescape_al_identifier`) and the
    /// member wrapper's `Origin` (for `enclosingMemberRange` + `originatingObject`).
    /// `None` for a procedure or an object-level trigger (OnRun / OnOpenPage). Mirrors
    /// the legacy `enclosing_member_of` (E1 — additive, never serialized into a golden).
    pub enclosing_member: Option<(String, Origin)>,
    /// `true` when this routine's `enclosing_member` is a report/report-extension
    /// DATASET `modify(<Dataitem>)` node (dataitem-receivers plan, Task 1) — i.e. the
    /// routine is nested inside `dataset { modify(X) { trigger .. } }` (directly, or
    /// under an `add*dataset_modification` wrapper), as opposed to a `modify()` inside
    /// `fields`/`layout`/`requestpage`/`views`. A `modify_modification` node carries its
    /// target in the grammar's `target` field, not `name` (see `RawModifyModification`),
    /// so the lowerer cannot itself resolve WHICH dataitem the target names (that needs
    /// the full own+base dataitem map — a resolve-time, cross-object concern). This flag
    /// is the ADDITIVE signal that lets the engine's resolve-time fallback (looking the
    /// name up via `enclosing_member`) apply ONLY to confirmed dataset `modify()`
    /// context — REQUESTPAGE ISOLATION: always `false` inside `requestpage` (the lowerer
    /// forces dataset-context off descending into it), and always `false` for every
    /// non-Report/ReportExtension object (they never contain a `dataset` section).
    /// Always `false` when `enclosing_member` is not itself a `modify_modification`.
    pub in_dataset_modify_context: bool,
    /// The fields few routines set, in one box (ENG-102: on CDO 3,705 of 121,107
    /// routines have a return name, 3,002 a dataitem table, 1,335 build conditions and
    /// 14,599 a doc; inline they cost 96 B on every routine). `None` when none is
    /// set. Read through [`Self::return_name`], [`Self::dataitem_source_table`],
    /// [`Self::preproc_context`] and [`Self::doc`].
    pub rare: Option<Box<RoutineRare>>,
    /// `None` for a forward/external declaration with no body.
    pub body: Option<BlockId>,
    pub origin: Origin,
}

/// The enclosing member of a request page's own triggers (ENG-20): a request page has no
/// name, and without a member its `OnOpenPage` had a report procedure `OnOpenPage()`'s id
/// (alc accepts the pair). Its controls' and actions' triggers keep their own member.
pub const REQUEST_PAGE_MEMBER: &str = "RequestPage";

/// [`RoutineDecl`]'s rarely set fields (see [`RoutineDecl::rare`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RoutineRare {
    /// The NAMED-return-value binding name (`procedure X() Ret: Record Y` — grammar
    /// `_procedure_named_return`'s `return_value` field), unquoted (outer-quote-
    /// stripped, mirrors `ident_text`); `None` for an anonymous `: Type` return (or no
    /// return at all). The grammar only ever sets this alongside `return_type` (the
    /// same production requires both), so `Some(_)` here implies `return_type` is also
    /// `Some`. Scoped to THIS routine only — the engine synthesizes a scoped value
    /// symbol from this name + `return_type` (own-routine scope, never leaking across
    /// routines) that participates in `receiver.rs`'s Step 2 bare-identifier lookup and
    /// `arg_dispatch.rs`'s caller-scope-exact arg typing via the shared
    /// `caller_scope_symbol` helper (T3, receiver-closure-and-arg-increments plan).
    pub return_name: Option<String>,
    /// For a trigger nested inside a report `dataitem(Name; "Source Table")`, the
    /// enclosing (innermost) dataitem's SOURCE TABLE name (unquoted) — the type of the
    /// dataitem trigger's implicit `Rec`. `None` for any non-dataitem routine. Mirrors
    /// the legacy `report_dataitem_source_table`.
    pub dataitem_source_table: Option<String>,
    /// The build conditions this routine exists under (see
    /// [`super::PreprocSymbols`]): the object-level `#if` branches around it and,
    /// for a header split across `#if` arms, its own arm. Empty for a routine
    /// every build compiles. Overload selection drops a candidate whose
    /// conditions contradict the call site's build context.
    pub preproc_context: super::PreprocSymbols,
    /// The run of `//` / `///` line comments directly above the declaration (its
    /// first attribute, when it has one), top to bottom, joined with `\n`. Each
    /// line loses its marker (`///`, else `//`) and one space after it; nothing
    /// else is changed. The run is the comment lines that each stand alone on
    /// their line, with no gap: a blank line or a code line ends it, a
    /// `#region` / `#endregion` line is skipped over. `/* … */` comments are not
    /// part of a run (they end it). `None` when there is no such run. The raw
    /// comments, markers included, stay in [`super::AlFile::comments`].
    pub doc: Option<String>,
    /// ENG-20: the routine is a request page's own trigger (its `enclosing_member` is
    /// [`REQUEST_PAGE_MEMBER`], which a dataitem or control could also be named). Set only
    /// by the lowerer, from the `requestpage` section itself, so no name can set it; the ids
    /// carry it as their own part (`engine::ids`, `RoutineNodeId::request_page`).
    pub request_page: bool,
}

impl RoutineRare {
    /// `None` when every field is unset (a routine then carries no box).
    #[must_use]
    pub fn boxed(self) -> Option<Box<Self>> {
        (self != Self::default()).then(|| Box::new(self))
    }
}

impl RoutineDecl {
    #[must_use]
    pub fn return_name(&self) -> Option<&str> {
        self.rare.as_deref()?.return_name.as_deref()
    }
    #[must_use]
    pub fn dataitem_source_table(&self) -> Option<&str> {
        self.rare.as_deref()?.dataitem_source_table.as_deref()
    }
    #[must_use]
    pub fn preproc_context(&self) -> &[(String, bool)] {
        self.rare
            .as_deref()
            .map_or(&[], |r| r.preproc_context.as_slice())
    }
    #[must_use]
    pub fn doc(&self) -> Option<&str> {
        self.rare.as_deref()?.doc.as_deref()
    }
    /// Whether this is a request page's own trigger ([`RoutineRare::request_page`]).
    #[must_use]
    pub fn is_request_page_trigger(&self) -> bool {
        self.rare.as_deref().is_some_and(|r| r.request_page)
    }
    /// Set the doc run (`lower_file`, after the routine is built); boxes only when set.
    pub fn set_doc(&mut self, doc: Option<String>) {
        let Some(doc) = doc else { return };
        self.rare.get_or_insert_with(Box::default).doc = Some(doc);
    }
}

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum RoutineKind {
    Procedure,
    Trigger,
}

/// A declared type's text (`Record Customer`, `Integer`), shared (ENG-102): on CDO 288,663
/// parameter, variable and return types hold 7,041 distinct texts, so the lowerer keeps one
/// copy per text per thread ([`shared_type_text`]).
pub type TypeText = std::sync::Arc<str>;

/// Texts [`shared_type_text`] keeps per thread; past it a text is still right, only no
/// longer shared, so a long-running thread's set stays bounded.
pub const SHARED_TYPE_TEXT_CAP: usize = 16_384;

/// `text`'s shared copy on this thread.
#[must_use]
pub fn shared_type_text(text: &str) -> TypeText {
    thread_local! {
        static TEXTS: std::cell::RefCell<std::collections::HashSet<TypeText>> =
            std::cell::RefCell::default();
    }
    TEXTS.with(|texts| {
        let mut texts = texts.borrow_mut();
        if let Some(t) = texts.get(text) {
            return t.clone();
        }
        let t = TypeText::from(text);
        if texts.len() < SHARED_TYPE_TEXT_CAP {
            texts.insert(t.clone());
        }
        t
    })
}

pub struct Param {
    pub name: String,
    /// `var` (by-reference) parameter.
    pub by_ref: bool,
    pub ty: Option<TypeText>,
    pub origin: Origin,
}

pub struct VarDecl {
    pub name: String,
    pub ty: Option<TypeText>,
    pub temporary: bool,
    pub origin: Origin,
    /// The text of a label declaration (`Name: Label 'text', Locked = true;`) or
    /// of a `TextConst` (`Name: TextConst ENU = 'text';`), `None` for every other
    /// declaration. See [`LabelValue`]. Boxed (ENG-102): 112 B inline on every
    /// declaration, set on 761 of 94,471 locals and 23,979 of 63,592 globals on CDO.
    pub label: Option<Box<LabelValue>>,
}

/// What a label declaration's value is: the literal text with its quotes removed
/// and every doubled `''` read as one `'`, whether it is locked, and its other
/// attributes. A label that is not locked can be translated, so its text at run
/// time may differ from this one; only a `locked` label's text is a constant
/// (G52). A `TextConst` is never locked.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LabelValue {
    /// The literal's text. `None` when the value is not a plain `'…'` string
    /// literal (a verbatim `@'…'` string), so nothing can claim to know it. For a
    /// `TextConst`: the `ENU` value, else the first value.
    pub text: Option<String>,
    /// True only when the declaration says `Locked = true` and nothing says
    /// `Locked = false`.
    pub locked: bool,
    /// `Comment = '…'`, the text a developer wrote for translators (it often
    /// says what each `%1` is). `None` when absent or not a plain literal.
    pub comment: Option<String>,
    /// `MaxLength = n`.
    pub max_length: Option<u32>,
    /// Every other `name = value` attribute, as written, in source order: a
    /// `Comment` or `MaxLength` whose value is not of the expected kind, and
    /// any name the grammar accepts but the fields above do not cover.
    pub other: Vec<(String, String)>,
    /// `TextConst` only: every `(language, text)` pair other than the one in
    /// [`Self::text`], in source order. A pair whose value is no plain literal
    /// keeps its raw text.
    pub translations: Vec<(String, String)>,
}

// ENG-94: the declarations keep no spare capacity. The lowerer push-builds them, and a
// first push reserves four elements (a routine with one local held 4 x 192 B), so a
// dependency tier kept by a resident session carried 49.6 MiB of unused capacity on CDO in
// these lists alone. `lower_file` calls `exact_size` on each object once it is built. Each
// method names every field (no `..`), so a field added later does not compile here until
// someone decides whether it owns a list to trim (lowerer review, ENG-94).
use super::exact_size;

impl ObjectDecl {
    /// Drop every list's spare capacity, this object's and its members' ([`exact_size`]).
    pub fn exact_size(&mut self) {
        let ObjectDecl {
            kind: _,
            id: _,
            name: _,
            routines,
            globals,
            protected_globals,
            properties,
            dataitems,
            temporary_dataitems,
            query_columns,
            dataset_items,
            extends_target: _,
            implements,
            page_controls,
            fields,
            keys,
            run_objects,
            enum_values,
            actions,
            origin: _,
        } = self;
        routines.iter_mut().for_each(RoutineDecl::exact_size);
        globals.iter_mut().for_each(VarDecl::exact_size);
        actions.iter_mut().for_each(ActionDecl::exact_size);
        fields.iter_mut().for_each(FieldDecl::exact_size);
        enum_values.iter_mut().for_each(EnumValueDecl::exact_size);
        keys.iter_mut().for_each(exact_size);
        exact_size(routines);
        exact_size(globals);
        exact_size(protected_globals);
        exact_size(properties);
        exact_size(dataitems);
        exact_size(temporary_dataitems);
        exact_size(query_columns);
        exact_size(dataset_items);
        exact_size(implements);
        exact_size(page_controls);
        exact_size(fields);
        exact_size(keys);
        exact_size(run_objects);
        exact_size(enum_values);
        exact_size(actions);
    }
}

impl RoutineDecl {
    /// Drop every list's spare capacity ([`exact_size`]).
    pub fn exact_size(&mut self) {
        let RoutineDecl {
            kind: _,
            name: _,
            name_origin: _,
            params,
            return_type: _,
            locals,
            attributes,
            attributes_parsed,
            access_modifier: _,
            parse_incomplete: _,
            enclosing_member: _,
            in_dataset_modify_context: _,
            rare,
            body: _,
            origin: _,
        } = self;
        locals.iter_mut().for_each(VarDecl::exact_size);
        attributes_parsed
            .iter_mut()
            .for_each(AttributeIr::exact_size);
        exact_size(params);
        exact_size(locals);
        exact_size(attributes);
        exact_size(attributes_parsed);
        if let Some(RoutineRare {
            return_name: _,
            dataitem_source_table: _,
            preproc_context,
            doc: _,
            request_page: _,
        }) = rare.as_deref_mut()
        {
            exact_size(preproc_context);
        }
    }
}

impl VarDecl {
    /// Drop its label's lists' spare capacity ([`exact_size`]).
    pub fn exact_size(&mut self) {
        let VarDecl {
            name: _,
            ty: _,
            temporary: _,
            origin: _,
            label,
        } = self;
        if let Some(LabelValue {
            text: _,
            locked: _,
            comment: _,
            max_length: _,
            other,
            translations,
        }) = label.as_deref_mut()
        {
            // Exact already today: the lowerer clones one label per declared name, and a
            // clone has no spare capacity (sol's test gap, ENG-94: a break here stays green).
            exact_size(other);
            exact_size(translations);
        }
    }
}

impl AttributeIr {
    /// Drop its argument list's spare capacity ([`exact_size`]).
    pub fn exact_size(&mut self) {
        let AttributeIr {
            name: _,
            raw: _,
            args,
            origin: _,
            conditional: _,
            another_declaration_lacks: _,
        } = self;
        exact_size(args);
    }
}

impl EnumValueDecl {
    /// Drop its implementations' spare capacity ([`exact_size`]).
    pub fn exact_size(&mut self) {
        let EnumValueDecl {
            name: _,
            implementations,
            origin: _,
        } = self;
        exact_size(implementations);
    }
}

impl ActionDecl {
    /// Drop every list's spare capacity ([`exact_size`]).
    pub fn exact_size(&mut self) {
        let ActionDecl {
            name: _,
            kind: _,
            caption,
            tooltip,
            run_object,
            triggers,
            doc: _,
            origin: _,
        } = self;
        exact_size(caption);
        exact_size(tooltip);
        exact_size(run_object);
        exact_size(triggers);
    }
}

impl FieldDecl {
    /// Drop every list's spare capacity ([`exact_size`]).
    pub fn exact_size(&mut self) {
        let FieldDecl {
            number: _,
            name: _,
            data_type: _,
            field_class: _,
            is_blob_like: _,
            obsolete_state,
            obsolete_reason,
            caption,
            tooltip,
            option_caption,
            calc_formula,
            doc: _,
            origin: _,
        } = self;
        exact_size(obsolete_state);
        exact_size(obsolete_reason);
        exact_size(caption);
        exact_size(tooltip);
        exact_size(option_caption);
        exact_size(calc_formula);
    }
}

/// The text of an AL string literal (`'it''s'` is `it's`), or `None` when `raw`
/// is not one plain, well-formed `'…'` literal.
pub fn al_string_literal_text(raw: &str) -> Option<String> {
    let inner = raw.strip_prefix('\'')?.strip_suffix('\'')?;
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c == '\'' {
            // Inside the literal a quote only appears doubled.
            if chars.next() != Some('\'') {
                return None;
            }
        }
        out.push(c);
    }
    Some(out)
}

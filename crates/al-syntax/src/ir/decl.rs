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
    /// Object-level properties (`SourceTable`, `TableNo`, `PageType`, …) in source
    /// order. Needed by the engine to seed implicit-`Rec` table resolution and object
    /// classification; the value is the raw value text (trimmed).
    pub properties: Vec<ObjectProperty>,
    /// Report `dataitem(Name; "Source Table")` declarations (name, source-table), both
    /// unquoted, document order — Report/ReportExtension only (empty otherwise). A
    /// dataitem name is in scope as a record var across ALL the report's routines, so
    /// the engine seeds each as a record variable in every routine. Distinct from a
    /// dataitem trigger's per-dataitem implicit `Rec` (see
    /// [`RoutineDecl::dataitem_source_table`]).
    pub report_dataitems: Vec<(String, String)>,
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
}

/// A single object-level `property` node (`name = value`). `name` is lowercased;
/// `value` is the raw value text (quotes preserved — the engine strips as needed).
pub struct ObjectProperty {
    pub name: String,
    pub value: String,
    pub origin: Origin,
}

/// A parsed routine attribute (`[EventSubscriber(ObjectType::Codeunit, …)]`).
/// `name` is the raw attribute name; `raw` the full `attribute_item` text; `args`
/// the lowered argument exprs (the engine projects each to kind/text/value/…).
pub struct AttributeIr {
    pub name: String,
    pub raw: String,
    pub args: Vec<super::ExprId>,
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
    /// Any other object construct, kind preserved via `Origin.kind_text`.
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
    pub return_type: Option<String>,
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
    /// For a trigger nested inside a report `dataitem(Name; "Source Table")`, the
    /// enclosing (innermost) dataitem's SOURCE TABLE name (unquoted) — the type of the
    /// dataitem trigger's implicit `Rec`. `None` for any non-dataitem routine. Mirrors
    /// the legacy `report_dataitem_source_table`.
    pub dataitem_source_table: Option<String>,
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
    /// `None` for a forward/external declaration with no body.
    pub body: Option<BlockId>,
    pub origin: Origin,
}

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum RoutineKind {
    Procedure,
    Trigger,
}

pub struct Param {
    pub name: String,
    /// `var` (by-reference) parameter.
    pub by_ref: bool,
    pub ty: Option<String>,
    pub origin: Origin,
}

pub struct VarDecl {
    pub name: String,
    pub ty: Option<String>,
    pub temporary: bool,
    pub origin: Origin,
}

//! Receiver-type lattice and Phase-A inference for member-call resolution.
//!
//! # Overview
//!
//! Member-call resolution (Phase B) dispatches on the STATIC TYPE of the receiver
//! expression. This module provides:
//! - [`ReceiverType`] — the lattice value Phase B dispatches on.
//! - [`FrameworkKind`] — the platform/framework data-type discriminant.
//! - [`ParsedType`] — intermediate result of [`classify_type_text`] (string→shape,
//!   no graph access).
//! - [`classify_type_text`] — pure string parse of a declared type string.
//! - [`infer_receiver_type`] — Phase A: infer the receiver type for a member call.
//!
//! # Phase A inference order
//!
//! Given a lowercased receiver name `receiver_lc`, inference proceeds:
//!
//! 0. **`CurrPage.<part>.Page` subpage-instance receivers** — a page control's
//!    (`part(<part>; <SubPage>)`) SUBPAGE INSTANCE, distinct from the CONTROL
//!    itself (`CurrPage.<part>` with no `.Page`, which addresses structural
//!    control methods like `.Update`/`.Visible` and is NOT resolved here).
//!    Only the exact `<part>.Page` shape (one control segment, one trailing
//!    `.Page` accessor) resolves, and only for a `Part` control whose target
//!    resolves unambiguously; a `SystemPart`/`UserControl`, a bare part, a
//!    deeper chain, or an unresolved/ambiguous target all fall through to
//!    `Unknown` (see [`infer_receiver_type`]'s Step 0).
//! 1. **(Retired — moved to 3c, T1.5 deep-review remediation.)** Platform
//!    singletons (`currpage`, `session`, `this`, …) used to be checked HERE,
//!    unconditionally, before any variable lookup ever ran — a real bug: a
//!    declared `var Session: Codeunit "Telemetry Wrapper"` was silently
//!    shadowed by the framework `Session` singleton (false `builtin` edge,
//!    or a false `Unknown` for `Session: Record Session`). Compiler-probed
//!    (see Step 3c below): none of these names are AL reserved words, so a
//!    declaration always wins — moved to Step 3c, after every
//!    higher-precedence declared-symbol lookup (2/2b/3a/3b) has missed. The
//!    numbering gap at 1 is deliberate — kept rather than renumbering every
//!    later step's pervasive in-file cross-references.
//! 2. **Variable lookup** — searches `routine.params` then `routine.locals` then
//!    `object_globals` by lowercased name → calls [`classify_type_text`] on the
//!    declared type → resolves Record table names and Object names against the graph.
//!    When the receiver name is `rec`/`xrec`, a variable with that name shadows
//!    the implicit-Rec step (a Codeunit routine may declare `var Rec: Record
//!    Customer`; the declared type is used in that case).
//! 3. **Implicit Rec / xRec** — two sub-cases, in order, both reached only on a
//!    Step 2 miss (a variable/param/global ALWAYS shadows the implicit Rec,
//!    whether by identity or by field — AL scoping; see Step 2's quote-parity
//!    fix, which is what makes this precedence correctly enforceable for a
//!    quoted name):
//!    - **3a. Bare quoted-field receiver** (record-field chains plan Task 4) —
//!      when the receiver is a QUOTED identifier and the enclosing object is a
//!      Table or TableExtension, looks the name up in the implicit-Rec table's
//!      visibility-scoped field surface (`ResolveIndex::field_in_table`) and
//!      types by the field's declared type. A same-named ROUTINE anywhere in
//!      that same visibility-scoped table surface
//!      (`ResolveIndex::table_scope_has_routine`) declines FIRST — AL's
//!      parens are optional on a zero-argument call, so a bare `Member` AST
//!      node is structurally ambiguous between a field access and a
//!      parens-less procedure call, and this step must never guess between
//!      them. Also gated on `WithState::NoWithProven` (mirrors
//!      [`crate::program::resolve::resolver::resolve_bare`]'s own Step 3
//!      implicit-Rec with-guard). Any other object kind, an unquoted
//!      receiver, a field-name miss, or an ambiguous/duplicate field all
//!      decline (fall through to 3b, never guessed) — quoted-only is
//!      deliberate undercoverage, an unquoted bare field reference is
//!      deferred to a future task.
//!    - **3b. `rec`/`xrec` identity** — resolves to the enclosing object's
//!      implicit record type (Table self-id, TableExtension base,
//!      Page/PageExtension via `SourceTable`, Codeunit via `TableNo` —
//!      topology-aware, fail-closed through `ResolveIndex::resolve_object_ref`,
//!      see [`infer_implicit_rec`] — or `Record{None}` for
//!      Report/ReportExtension, whose implicit Rec is per-dataitem scoped
//!      rather than object-level and is not yet modeled). A Codeunit with no
//!      `TableNo` declared at all (including `Subtype = Test`/`TestRunner`,
//!      which never declares one) has no implicit-Rec entity to type and
//!      returns `Unknown`; every other object kind not listed above
//!      (Report/ReportExtension aside) also returns `Unknown`.
//!
//! **Step 3c — platform singletons** (T1.5, deep-review remediation plan —
//! formerly Step 1, see the retirement note above) — hardcoded platform
//! names (`currpage`/`page`, `currreport`/`report`, `session`, `navapp`,
//! `database`, `isolatedstorage`, `taskscheduler`, `system`,
//! `companyproperty`, `sessioninformation`, `this`). Reached only on a Step
//! 2/2b/3a/3b miss, so a declared var/param/global, a report dataitem name,
//! an implicit-Rec table field, or the `rec`/`xrec` identity ALL shadow a
//! same-named singleton — matching both AL compiler semantics
//! (compiler-probed: none of these twelve names are reserved words; `this`
//! alone draws a soft `AL0848` "keyword since 14.0" warning but still
//! compiles and still shadows) and the L3 sibling
//! (`engine/l3/receiver_type.rs:283-318`), which checks every one of them,
//! `this` included, in the identical position.
//!
//! 4. **Static framework type name** — when the receiver name matches a framework
//!    type name (e.g. `XmlDocument`, `Text`, `File`, `Version`) and no variable was
//!    found, type it as `Framework(kind)` so Phase B dispatches the static method
//!    via the builtin catalog.
//! 5. **Compound call-result receiver (`Func().Method()`)** — beyond-1B.3b
//!    Task 3. Only engages when `receiver_expr` carries a structured
//!    `ExprKind::Call{function, args}` node whose `function` is a BARE
//!    identifier (never dotted/member — a `Obj.Method().X()` cross-object
//!    chain declines HERE, at Step 5, and falls through to Step 6's
//!    cross-object-chain arm instead, plan v2.1 Task 3). Fail-closed, in order:
//!    (a) a caller param/local/global named identically to `function` SHADOWS
//!    it in AL (`resolve_bare` cannot see variables) — any such shadow
//!    declines immediately; (b) otherwise `function` is typed by calling
//!    [`crate::program::resolve::resolver::resolve_bare`] as a TYPE QUERY
//!    (reusing its own-object/extension-base/implicit-Rec/builtin precedence,
//!    ambiguity declines, and with-guard) — anything other than exactly one
//!    route to a `RouteTarget::Routine` declines; (c) that routine's
//!    `graph.routines[..].return_type` must be `Some` and parse (via
//!    [`classify_type_text`]) to a non-`Primitive` shape — a `None` or scalar
//!    return declines; the parsed type is then resolved to a receiver exactly
//!    like Step 2's declared-variable path (via [`parsed_type_to_receiver`]),
//!    inheriting its fail-closed cross-app-ambiguous-object decline. Only
//!    engaged when the caller passes a `bare_ctx` (full end-to-end resolution
//!    via `resolve_full_program`); callers with no `DeclSurface`/`WithState` in
//!    scope (tests, `semantic_golden.rs`) pass `None` and this step is a no-op
//!    — resolution-neutral for them, exactly like `receiver_expr` itself.
//! 6. **Compound framework property/method + `this.<rest>` receiver**
//!    (beyond-1B.3b Task 4). Only engages when `receiver_expr` (Task 2) is
//!    populated — unlike Step 5, this step does NOT need `bare_ctx` (it never
//!    calls `resolve_bare`), so it also fires for callers that supply
//!    `receiver_expr` but not `bare_ctx`. Two independent AST-based sub-cases,
//!    both operating on the STRUCTURED `Expr` node (never `receiver_text`
//!    string-splitting): (a) `<Framework>.<Prop>` / `<Framework>.<Method(..)>`
//!    — the receiver is `ExprKind::Member{object, member}` (property form) or
//!    `ExprKind::Call{function: Member{object, member}, args}` (method-call
//!    form); `object` is recursively typed via the AST-native
//!    [`infer_receiver_type_for_expr`] helper, and if it resolves to
//!    `Framework(kind)`, the versioned [`framework_return_kind`] table maps
//!    `(kind, member_lc, is_method, arity)` to the returned kind — a table
//!    miss (wrong member, wrong form, wrong arity) declines. (b) `this.<rest>`
//!    — when `object` is the bare `this` identifier, `member` is resolved
//!    against a SELF-ONLY scope (`object_globals` only — never
//!    `routine.params`/`routine.locals`, per AL's documented `this.` semantics
//!    of addressing only "methods and globals within the same object"); a
//!    `this.<method>(..)` CALL form (dispatching a same-object procedure's
//!    return type) is deliberately NOT handled here — declines — since typing
//!    it correctly needs `resolve_bare`-style routine lookup, out of this
//!    step's scope. See [`infer_receiver_type_for_expr`] for the full
//!    recursion. (c) `<RecordRef|FieldRef|KeyRef>.<Method(..)>` (Task 4,
//!    chain-tables plan) — the SAME recursive base-typing as (a); if it
//!    resolves to `RecordRef`/`FieldRef`/`KeyRef`, the versioned
//!    [`recordref_family_return_kind`] table (a DISTINCT family from
//!    `framework_return_kind`, same fail-closed table-miss-declines
//!    contract) maps `(kind, member_lc, is_method, arity)` to the returned
//!    `*Ref` kind — e.g. `RecordRef.KeyIndex(1).FieldIndex(1)`.
//! 7. **Unknown** — no positive typing found.
//!
//! # Clean-room note
//!
//! This mirrors the logic of L3's `infer_receiver_type` in
//! `src/engine/l3/receiver_type.rs` but is written fresh over the IR
//! (`RoutineDecl`/`VarDecl`/`Param`) and `ProgramGraph`/`ResolveIndex`, carrying
//! `ObjectNodeId`s instead of L3 string IDs.

use al_syntax::IdentifierFoldExt;
use al_syntax::ir::{AlFile, ExprId, ExprKind, ObjectKind, RoutineDecl, VarDecl};

use crate::program::graph::ProgramGraph;
use crate::program::node::{ObjectNodeId, RoutineNodeId};
use crate::program::node_extract::{
    FieldNode, ObjectNode, ObjectRef, PageControlKind, PageControlNode, RoutineNode,
};
use crate::program::resolve::decl_surface::DeclSurface;
use crate::program::resolve::edge::RouteTarget;
use crate::program::resolve::extract::WithState;
use crate::program::resolve::framework_returns::{enum_chain_return_kind, framework_return_kind};
use crate::program::resolve::index::{ObjectRefResolution, ResolveIndex};
use crate::program::resolve::recordref_returns::{
    RecordRefFamilyKind, recordref_family_return_kind,
};
use crate::program::resolve::resolver::{
    implicit_rec_table_id, resolve_bare, resolve_member, routine_node_for_type_query,
};

// ---------------------------------------------------------------------------
// FrameworkKind
// ---------------------------------------------------------------------------

/// Discriminant for AL platform framework / value types whose methods are
/// dispatched purely via the builtin catalog in Phase B.
///
/// Explicit variants cover the high-volume framework types; `Other` is the
/// catch-all for less-common types, carrying the lowercased first token of the
/// declared type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrameworkKind {
    // JSON types
    JsonObject,
    JsonToken,
    JsonArray,
    JsonValue,
    // HTTP types
    HttpClient,
    HttpRequestMessage,
    HttpResponseMessage,
    HttpContent,
    HttpHeaders,
    // Stream types
    InStream,
    OutStream,
    // String / text types
    TextBuilder,
    Text,
    BigText,
    SecretText,
    // Collection types
    List,
    Dictionary,
    // XML types (all xml* tokens)
    Xml,
    // Date/time value types
    Date,
    DateTime,
    Time,
    Duration,
    // GUID
    Guid,
    // Media / blob
    Blob,
    Media,
    // Notification / error
    Notification,
    ErrorInfo,
    // Misc platform value types
    RecordId,
    ModuleInfo,
    DataTransfer,
    SessionSettings,
    FilterPageBuilder,
    File,
    FileUpload,
    NumberSequence,
    Version,
    // Dialog
    Dialog,
    // Page/Report singleton types (from receiver name, not declared type)
    PageInstance,
    ReportInstance,
    // Query instance (from a variable's DECLARED type — `V: Query "Name"`),
    // never a receiver name. Unlike Page/Report there is no `CurrQuery`
    // singleton; the `Query.SaveAsXml(...)` STATIC form is a separate surface
    // and is deliberately not modelled here (no measured population).
    QueryInstance,
    // Platform singletons (from receiver name)
    Session,
    NavApp,
    Database,
    IsolatedStorage,
    TaskScheduler,
    System,
    CompanyProperty,
    SessionInformation,
    // Enum VALUE-instance surface (Task 4, receiver-closure-and-arg-increments
    // plan — the SPLIT catalog closer): `AsInteger()`/`Names()`/`Ordinals()`,
    // callable on an enum VALUE (a declared `Enum "X"`-typed var/field, or an
    // enum-value-literal chain `X::Y`) — see `ReceiverType::EnumType`.
    // `FromInteger` moved OFF this surface to `EnumTypeStatic` below (MS Learn
    // `enum-data-type`: "Static methods: FromInteger(Integer)" vs "Instance
    // methods: AsInteger()/Names()/Ordinals()" — see `member_catalog.rs`'s
    // `ENUM_VALUE`/`ENUM_TYPE_STATIC` split for the full citation).
    Enum,
    /// Enum TYPE-static surface (Task 4): `FromInteger(Integer)`/`Names()`/
    /// `Ordinals()`, callable on the enum TYPE reference itself — an
    /// `Enum::"Type"` chain (`ExprKind::QualifiedEnum` whose `enum_type` is the
    /// literal `Enum` keyword) or a bare (quoted or not) enum-type-name
    /// receiver that passes the programmatic collision rule — see
    /// `ReceiverType::EnumTypeStatic`. `AsInteger` is deliberately NOT on this
    /// surface (round-2 closer, BINDING: "AsInteger is VALUE-surface... not
    /// TYPE-surface") — there is no specific value to convert via a bare type
    /// reference.
    EnumTypeStatic,
    /// Programmatic-construction catch-all for less-common types encountered at
    /// Phase-B dispatch time.  Carries the lowercased first token of the declared
    /// type string.
    ///
    /// **Never emitted by [`classify_type_text`]** — all recognized type names map
    /// to explicit variants.  This variant exists for callers (Phase B, tests) that
    /// construct a [`FrameworkKind`] programmatically for unlisted types.
    Other(String),
}

// ---------------------------------------------------------------------------
// ReceiverType
// ---------------------------------------------------------------------------

/// The static type of a member-call receiver — the lattice Phase B dispatches on.
///
/// Every variant maps 1:1 onto a Phase-B `match` arm. The lattice is fail-closed:
/// any receiver that Phase A cannot positively type becomes `Unknown`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReceiverType {
    /// A first-class AL object type (Codeunit / Page / Report / Query / XmlPort)
    /// identified by kind and lowercased name.  Phase B resolves the method among
    /// the object's declared procedures via `graph.resolve_object`.
    Object {
        kind: ObjectKind,
        name_lc: String,
        /// The resolved target's `ObjectNodeId`, when Phase A already proved
        /// it MECHANICALLY (Task 7's `CurrPage.<part>.Page` subpage-instance
        /// Step 0, via the fail-closed `ResolveIndex::resolve_object_ref`) —
        /// carried through so `resolve_member`'s `Object` arm can
        /// short-circuit on it directly instead of re-resolving `name_lc`
        /// against the graph a second time (which could in principle land on
        /// a different object than the one Step 0 actually verified unique).
        /// `None` for every other `Object` receiver (declared-variable /
        /// param / global lookup via [`classify_type_text`]), which still
        /// resolves by name in `resolve_member` as before.
        id: Option<ObjectNodeId>,
    },
    /// An `Interface IFoo` receiver — Phase B fans out to every implementer.
    Interface { name_lc: String },
    /// A declared `Enum "Color"`-typed VALUE (a var/field), OR a VERIFIED
    /// enum-value-literal chain (`X::Y` — `ExprKind::QualifiedEnum` whose
    /// `enum_type` is NOT the literal `Enum` keyword, but WAS confirmed to
    /// itself resolve to an Enum shape — see the `QualifiedEnum` arm of
    /// `infer_receiver_type_for_expr`, Task 4 review fix) — the
    /// VALUE-instance surface (`AsInteger`/`Names`/`Ordinals`; see
    /// `FrameworkKind::Enum`). `name_lc` is carried for parity with the
    /// declared-type case but is NOT consulted by dispatch (every arm
    /// matches `{ .. }`) — the VALUE-instance catalog applies uniformly
    /// regardless of which enum's value this is.
    EnumType { name_lc: String },
    /// The enum TYPE reference itself, TASK 4 (receiver-closure-and-arg-
    /// increments plan) — `Enum::"Type"` (an `ExprKind::QualifiedEnum` whose
    /// `enum_type` derefs to the literal `Enum` keyword identifier) or a bare
    /// (quoted or not) enum-type-name receiver that passed the programmatic
    /// collision rule (`infer_receiver_type`'s Step 4b). Dispatches via the
    /// TYPE-static catalog (`FrameworkKind::EnumTypeStatic`) — a DISTINCT
    /// surface from `EnumType` above: `FromInteger`/`Names`/`Ordinals`, never
    /// `AsInteger`. `name_lc` is the enum's declared name, lowercased — unlike
    /// `EnumType`, this one WAS existence-checked (`ObjectKind::Enum`
    /// resolved uniquely) before construction, since a raw quoted string here
    /// has no parser-level type guarantee the declared-var case enjoys.
    EnumTypeStatic { name_lc: String },
    /// A `ControlAddIn "Foo"` receiver — either a direct-var declaration
    /// (`var X: ControlAddIn "Foo"`) or a `CurrPage.<usercontrol>` reference
    /// (receiver-closure plan, Task 1). `name_lc` is the declared addin type
    /// name, lowercased, as written at the reference site (NOT necessarily
    /// the resolved object's canonical name — see
    /// [`resolve_control_addin_receiver`]'s doc on the real-world
    /// short-name-vs-fully-qualified mismatch this tolerates). `surface`
    /// carries what Phase A already proved about the addin's declaration,
    /// closed-if-known — Phase B (`resolve_member`) gates the actual member
    /// name+arity against it. This variant is ONLY ever constructed for a
    /// [`ControlAddInSurface::Declared`] or [`ControlAddInSurface::TruePlatform`]
    /// outcome — an ambiguous, out-of-closure, degraded, or genuinely-absent
    /// (and non-platform) addin type declines to `Unknown` directly in Phase A
    /// and never reaches here.
    ControlAddIn {
        name_lc: String,
        surface: ControlAddInSurface,
    },
    /// A `Record`-typed receiver.  `table` is the resolved `ObjectNodeId` of the
    /// table when it is in the workspace closure, else `None` (out-of-source table).
    ///
    /// A Record receiver is ALWAYS `Record`, even with `None` — Phase B's builtin
    /// catalog check is table-independent (SetRange / FindSet etc. are `builtin`
    /// regardless), and only a non-builtin method on a table-less Record yields
    /// the honest `Unknown` (decided in Phase B, not here).
    Record { table: Option<ObjectNodeId> },
    /// The enclosing object instance (`this.OwnMethod()`).  Phase B resolves the
    /// method among the caller object's own procedures.
    SelfObject,
    /// `RecordRef` receiver — catalog-only in Phase B.
    RecordRef,
    /// `FieldRef` receiver — catalog-only in Phase B.
    FieldRef,
    /// `KeyRef` receiver — catalog-only in Phase B.
    KeyRef,
    /// A platform/framework type (`Json*` / `Http*` / `InStream` / … ) — catalog
    /// lookup in Phase B.
    Framework(FrameworkKind),
    /// A primitive or unrecognized non-object, non-catalog type.  Phase B turns
    /// this into an honest `Unknown` edge.
    Primitive,
    /// A `Variant`-typed receiver — the held type is determined at runtime.
    /// NOT a resolution failure: genuinely `dynamic` per the honest taxonomy.
    Dynamic,
    /// Phase A could not positively type the receiver.
    Unknown,
}

/// What Phase A already proved about a `ControlAddIn` receiver's declaration —
/// the closed-if-known tri-state (receiver-closure plan, Task 1 round-2
/// closer). `Ambiguous`/`Degraded`/genuinely-absent-and-non-platform outcomes
/// are NOT represented here — they decline directly to `ReceiverType::Unknown`
/// in Phase A (see [`resolve_control_addin_receiver`]) and never construct a
/// [`ReceiverType::ControlAddIn`] at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlAddInSurface {
    /// The addin type is declared (source or ABI/SymbolOnly) and resolved to
    /// exactly ONE object, whose declaration parsed cleanly. `procedures` is
    /// its full declared procedure surface — `(name_lc, arity)` pairs, EVENTS
    /// EXCLUDED (never AL-callable; see
    /// `al_syntax::lower::collect_routines`'s `interface_procedure` handling,
    /// which never lowers an `event_declaration` as a `RoutineDecl` in the
    /// first place — there is nothing to filter out here, the exclusion is
    /// structural at the source). Phase B's gate: `method_lc` + `arity` must
    /// match ONE OF `procedures` OR the (currently EMPTY — see
    /// [`resolve_control_addin_receiver`]'s doc) platform base-member union;
    /// otherwise the call is a genuine `MemberNotFound`, never a guessed
    /// Catalog.
    Declared { procedures: Vec<(String, usize)> },
    /// No source/symbol declaration is reachable ANYWHERE for this addin name,
    /// but the name matches a known Microsoft-shipped platform addin CLASS
    /// (currently only `WebPageViewer` — see the const doc) whose JS-side
    /// method surface this engine cannot enumerate from here. Every method
    /// call is accepted unconditionally as a real platform invocation — the
    /// pre-Task-1 open policy, now scoped to just this small allowlist rather
    /// than every `ControlAddIn`-typed receiver.
    TruePlatform,
}

// ---------------------------------------------------------------------------
// ParsedType — intermediate result of classify_type_text
// ---------------------------------------------------------------------------

/// Result of the pure string→shape parse in [`classify_type_text`].
///
/// Names (table name, object name, interface name, enum name, controladdin
/// name) are preserved as
/// lowercased strings for subsequent graph-based resolution in
/// [`infer_receiver_type`].  No graph access is performed here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParsedType {
    /// `Record <TableName>` — the table reference, LOSSLESSLY shaped: a
    /// numeric AL object id (`Record 18`) is [`ObjectRef::Id`]; a name
    /// (quoted or not, `Record Customer` / `Record "Customer"`) is
    /// [`ObjectRef::Name`], stripped of quotes and a trailing ` temporary`
    /// modifier. Distinguishing the two shapes here (rather than collapsing
    /// both to a bare string) is the I1 Caller-A fix: `Record "18"` (a table
    /// literally NAMED "18") must never be silently coerced into numeric id
    /// 18 by a later stringly-typed re-parse.
    Record { table_ref: ObjectRef },
    /// `Codeunit X` / `Page X` / `Report X` / `Query X` / `XmlPort X` — object
    /// kind and the object reference, LOSSLESSLY shaped exactly like
    /// `Record`'s `table_ref` above (this is the I1 mirror for Caller-A's
    /// object-typed sibling): a numeric AL object id (`Codeunit 80`) is
    /// [`ObjectRef::Id`]; a name (quoted or not, `Codeunit "Sales-Post"` /
    /// `Codeunit MyMgt`) is [`ObjectRef::Name`]. Distinguishing the two shapes
    /// here — rather than collapsing both to a bare string — is required so
    /// `Codeunit 80` (numeric id 80) and `Codeunit "80"` (a codeunit literally
    /// NAMED "80") can never be conflated by a later stringly-typed re-parse.
    Object {
        kind: ObjectKind,
        object_ref: ObjectRef,
    },
    /// `Interface <Name>` — lowercased interface name.
    Interface { name: String },
    /// `Enum <Name>` — lowercased enum name.
    EnumType { name: String },
    /// `ControlAddIn <Name>` — lowercased addin type name, quotes stripped
    /// (Task 1). Distinct from `Framework` because a `ControlAddIn`'s member
    /// surface is gated on the SPECIFIC addin's declared procedures (closed-
    /// if-known), unlike every other `Framework` kind's uniform catalog.
    ControlAddIn { name: String },
    /// `RecordRef`
    RecordRef,
    /// `FieldRef`
    FieldRef,
    /// `KeyRef`
    KeyRef,
    /// A recognized platform/framework type.
    Framework(FrameworkKind),
    /// Primitive numeric/boolean type or an unrecognized keyword → Phase B unknown.
    Primitive,
    /// `Variant` — runtime-typed, genuinely dynamic dispatch.
    Dynamic,
}

// ---------------------------------------------------------------------------
// classify_type_text
// ---------------------------------------------------------------------------

/// Parse a declared type string into its [`ParsedType`] shape without any graph
/// access.
///
/// Logic mirrors L3's `classify_receiver` + `parse_object_type_ref` (clean-room):
/// - Strips a trailing `[N]` length suffix from the leading token (`Text[200]` →
///   `text`, `Code[20]` → `code`).
/// - Checks the first whitespace-delimited lowercased token against the full
///   catalog of keywords / framework types.
/// - Strips surrounding double-quotes from the name portion of compound types.
/// - `Record "Customer" temporary` → `Record { table_name: "customer" }`.
/// - `Variant` → `Dynamic`; unrecognized or primitive numeric types → `Primitive`.
pub fn classify_type_text(ty: &str) -> ParsedType {
    let trimmed = ty.trim();
    if trimmed.is_empty() {
        return ParsedType::Primitive;
    }

    // Split on the first whitespace character to get the leading token and the
    // remaining name portion (empty when the type has no name component).
    let (first_tok, rest) = match trimmed.find(char::is_whitespace) {
        Some(i) => (&trimmed[..i], trimmed[i + 1..].trim()),
        None => (trimmed, ""),
    };

    // Strip a trailing `[N]` length suffix so `Text[200]` normalises to `text`.
    let base = match first_tok.find('[') {
        Some(i) => &first_tok[..i],
        None => first_tok,
    };
    let lc = base.to_ascii_lowercase();

    match lc.as_str() {
        "record" => {
            // Parse the table reference: strip trailing " temporary", then
            // shape-classify — a numeric string is an `Id`, ANYTHING else
            // (including a QUOTED numeric string, since the quote characters
            // make it fail the `i64` parse before unquoting) is a `Name`.
            // Mirrors `node_extract::parse_object_ref_value`'s identical
            // numeric-vs-quoted-name distinction for `SourceTable`/`TableNo`.
            let stripped = strip_trailing_temporary(rest);
            let stripped = stripped.trim();
            let table_ref = if let Ok(n) = stripped.parse::<i64>() {
                ObjectRef::Id(n)
            } else {
                let raw = unquote_identifier(stripped);
                let normalized_lc = raw.fold_identifier();
                ObjectRef::Name { raw, normalized_lc }
            };
            ParsedType::Record { table_ref }
        }
        "codeunit" => parse_object_kind_type(ObjectKind::Codeunit, rest),
        "page" => parse_object_kind_type(ObjectKind::Page, rest),
        "report" => parse_object_kind_type(ObjectKind::Report, rest),
        "query" => parse_object_kind_type(ObjectKind::Query, rest),
        "xmlport" => parse_object_kind_type(ObjectKind::XmlPort, rest),
        "interface" => ParsedType::Interface {
            name: unquote_identifier(rest).fold_identifier(),
        },
        "enum" => ParsedType::EnumType {
            name: unquote_identifier(rest).fold_identifier(),
        },
        // Ref types
        "recordref" => ParsedType::RecordRef,
        "fieldref" => ParsedType::FieldRef,
        "keyref" => ParsedType::KeyRef,
        // JSON framework types
        "jsonobject" => ParsedType::Framework(FrameworkKind::JsonObject),
        "jsontoken" => ParsedType::Framework(FrameworkKind::JsonToken),
        "jsonarray" => ParsedType::Framework(FrameworkKind::JsonArray),
        "jsonvalue" => ParsedType::Framework(FrameworkKind::JsonValue),
        // HTTP framework types
        "httpclient" => ParsedType::Framework(FrameworkKind::HttpClient),
        "httprequestmessage" => ParsedType::Framework(FrameworkKind::HttpRequestMessage),
        "httpresponsemessage" => ParsedType::Framework(FrameworkKind::HttpResponseMessage),
        "httpheaders" => ParsedType::Framework(FrameworkKind::HttpHeaders),
        "httpcontent" => ParsedType::Framework(FrameworkKind::HttpContent),
        // Stream types
        "instream" => ParsedType::Framework(FrameworkKind::InStream),
        "outstream" => ParsedType::Framework(FrameworkKind::OutStream),
        // Text / string types
        "textbuilder" => ParsedType::Framework(FrameworkKind::TextBuilder),
        "text" | "code" | "label" => ParsedType::Framework(FrameworkKind::Text),
        "bigtext" => ParsedType::Framework(FrameworkKind::BigText),
        "secrettext" => ParsedType::Framework(FrameworkKind::SecretText),
        // Collection types
        "list" => ParsedType::Framework(FrameworkKind::List),
        "dictionary" => ParsedType::Framework(FrameworkKind::Dictionary),
        // XML types — all tokens starting with "xml" (XmlDocument, XmlElement, …)
        s if s.starts_with("xml") => ParsedType::Framework(FrameworkKind::Xml),
        // Media / blob
        "blob" => ParsedType::Framework(FrameworkKind::Blob),
        "media" | "mediaset" => ParsedType::Framework(FrameworkKind::Media),
        // Dialog
        "dialog" => ParsedType::Framework(FrameworkKind::Dialog),
        // Date / time value types (callable methods)
        "date" => ParsedType::Framework(FrameworkKind::Date),
        "datetime" => ParsedType::Framework(FrameworkKind::DateTime),
        "time" => ParsedType::Framework(FrameworkKind::Time),
        "duration" => ParsedType::Framework(FrameworkKind::Duration),
        // GUID
        "guid" => ParsedType::Framework(FrameworkKind::Guid),
        // Notification / error
        "notification" => ParsedType::Framework(FrameworkKind::Notification),
        "errorinfo" => ParsedType::Framework(FrameworkKind::ErrorInfo),
        // Misc platform value types
        "recordid" => ParsedType::Framework(FrameworkKind::RecordId),
        "moduleinfo" => ParsedType::Framework(FrameworkKind::ModuleInfo),
        "datatransfer" => ParsedType::Framework(FrameworkKind::DataTransfer),
        "sessionsettings" => ParsedType::Framework(FrameworkKind::SessionSettings),
        "filterpagebuilder" => ParsedType::Framework(FrameworkKind::FilterPageBuilder),
        "file" => ParsedType::Framework(FrameworkKind::File),
        "fileupload" => ParsedType::Framework(FrameworkKind::FileUpload),
        "numbersequence" => ParsedType::Framework(FrameworkKind::NumberSequence),
        "version" => ParsedType::Framework(FrameworkKind::Version),
        "controladdin" => ParsedType::ControlAddIn {
            name: unquote_identifier(rest).fold_identifier(),
        },
        // Variant — runtime-typed, genuinely dynamic
        "variant" => ParsedType::Dynamic,
        // Primitive numeric / boolean types and anything else unrecognized
        _ => ParsedType::Primitive,
    }
}

// ---------------------------------------------------------------------------
// infer_receiver_type
// ---------------------------------------------------------------------------

/// Phase A: infer the [`ReceiverType`] of a member-call receiver expression.
///
/// `receiver_lc` is the lowercased receiver text: usually a simple identifier,
/// but Step 0 also recognizes the `currpage.<part>.page` compound form (a
/// subpage-instance receiver) — any other compound expression that reaches here
/// unrecognized falls through to `Unknown` (fail-closed).
///
/// Inference order:
/// 0. **`CurrPage.<part>.Page` subpage-instance receivers** — see the module
///    doc's Step 0. Checked first because it is a COMPOUND (dotted) receiver
///    text that none of steps 1-4 would otherwise positively type.
/// 1. **Singletons** — `this`, `currpage`/`page`, `currreport`/`report`, and
///    other platform-provided names that are never declared as AL variables.
/// 2. **Variable lookup** — `routine.params` → `routine.locals` →
///    `object_globals`, matched by lowercased name; the declared type is
///    classified via [`classify_type_text`] and names are resolved against the
///    graph.  A variable named `rec`/`xrec` (idiomatic in Codeunits) is found
///    here and classified by its declared type, shadowing the implicit-Rec step.
/// 3. **Implicit Rec / xRec** — only when no variable named `rec`/`xrec` was
///    found in step 2: resolves to the object's implicit record type; returns
///    `Unknown` for object kinds with no implicit record (e.g. Codeunit).
/// 4. **Static framework type name** — bare identifier matching a framework type
///    (`XmlDocument`, `Text`, `File`, `Version`, …) with no variable found;
///    returned as `Framework(kind)`.
/// 5. **Compound call-result receiver (`Func().Method()`)** — see the module
///    doc's Step 5. Requires both `receiver_expr` (Task 2) and `bare_ctx`
///    (Task 3) to be populated; a no-op otherwise.
/// 6. **Compound framework property/method + `this.<rest>` receiver** — see
///    the module doc's Step 6. Requires only `receiver_expr` (Task 2); a
///    no-op otherwise.
/// 7. **Unknown** — no positive typing found.
///
/// # `receiver_expr` (Task 2 enabling primitive)
///
/// `receiver_expr` carries the PARSED receiver `Expr` — `Some((file, id))` when
/// the call site's [`CalleeShape::Member`] populated a `receiver` `ExprId`
/// (`file.ir.expr(id)` recovers the structured node: `ExprKind::Call{..}` /
/// `Member{..}` / …), `None` when the caller has no such node in scope (e.g.
/// the `RecordOp` shape, which does not carry one). Steps 0-4 are UNCHANGED by
/// this parameter and continue to dispatch purely on `receiver_lc`; Step 5
/// (Task 3) is the first consumer.
///
/// # `bare_ctx` (Task 3 enabling primitive)
///
/// `Some((surface, with_state))` when the caller can supply the two extra
/// inputs Step 5 needs to run [`crate::program::resolve::resolver::resolve_bare`]
/// as a type query (`resolve_full_program`'s real `CalleeShape::Member`
/// resolution path); `None` for callers with no such context in scope (unit
/// tests, `semantic_golden.rs`, the `RecordOp` shape) — Step 5 is then a
/// no-op, resolution-neutral exactly like `receiver_expr` for those callers.
///
/// [`CalleeShape::Member`]: crate::program::resolve::extract::CalleeShape::Member
#[allow(clippy::too_many_arguments)] // 6 pre-existing params + `bare_ctx` (Task 3); each is a distinct identity/lookup input, grouping would obscure call sites (mirrors `resolve_in_object`'s precedent).
pub fn infer_receiver_type(
    receiver_lc: &str,
    routine: &RoutineDecl,
    object_globals: &[VarDecl],
    from_object: &ObjectNode,
    graph: &ProgramGraph,
    index: &ResolveIndex,
    receiver_expr: Option<(&AlFile, ExprId)>,
    bare_ctx: Option<(&DeclSurface, WithState)>,
) -> ReceiverType {
    // -----------------------------------------------------------------------
    // Step 0 — `CurrPage.<part>.Page` subpage-instance receivers (Task 7).
    //
    // A page's `part(<part>; <SubPage>)` control's SUBPAGE INSTANCE is
    // accessed as `CurrPage.<part>.Page.<method>()`; resolving `<part>.Page`
    // to the target Page object lets `resolve_member`'s ordinary `Object` arm
    // dispatch the subpage's user procedures. This is DISTINCT from
    // `CurrPage.<part>.<method>()` (no `.Page`), which addresses the CONTROL
    // itself — that shape falls through to `Unknown` here, never fabricated
    // as a subpage call (Step 0b below handles the analogous bare-control
    // shape for `UserControl` controls specifically; a bare `Part` control
    // reference has no equivalent — a subpage Part exposes no callable
    // surface of its own, only through `.Page`). `SystemPart`/`UserControl`
    // controls and any chain deeper than one `.Page` accessor also fall
    // through here: a wrong subpage is a false `Source` edge, the cardinal
    // sin, so anything short of an exact single-segment `<part>.Page` shape
    // resolving to exactly one in-closure Page object declines rather than
    // guesses.
    // -----------------------------------------------------------------------
    if let Some(rest) = receiver_lc.strip_prefix("currpage.")
        && let Some(part_name_lc) = parse_currpage_dot_page_segment(rest)
        && let Some(control) = find_page_control(&part_name_lc, from_object, graph, index)
        && control.kind == PageControlKind::Part
        && let ObjectRefResolution::Unique(page_id) = index.resolve_object_ref(
            graph,
            from_object.id.clone(),
            ObjectKind::Page,
            &control.target,
        )
        && let Some(page_obj) = graph.objects.iter().find(|o| o.id == page_id)
    {
        return ReceiverType::Object {
            kind: ObjectKind::Page,
            name_lc: page_obj.name.fold_identifier(),
            id: Some(page_id),
        };
    }

    // -----------------------------------------------------------------------
    // Step 0b — `CurrPage.<usercontrol>` ControlAddIn receivers (receiver-
    // closure plan, Task 1). A page's `usercontrol(<control>; <AddinType>)`
    // control is addressed DIRECTLY as `CurrPage.<control>.<Method>(...)` —
    // no `.Page` accessor (unlike the `Part` subpage-instance shape above, a
    // usercontrol has no subpage OBJECT of its own; its JS-side add-in
    // methods are invoked straight off the control reference). CLOSED-IF-
    // KNOWN (round-2 closer, BINDING): see
    // [`resolve_control_addin_receiver`]'s doc for the full tri-state gate —
    // Ambiguous/Degraded/genuinely-unresolved-non-platform all decline to
    // `Unknown` right there, never reaching `resolve_member`'s Catalog path.
    // `SystemPart` controls are explicitly OUT of this arm (native platform
    // components rendered by the client shell, not JS add-ins — no
    // `controladdin` object backs them at all, so there is nothing to gate
    // against; a closed SystemPart catalog is future work if real call sites
    // ever surface). A bare single segment with NO further chain is required
    // (`parse_currpage_bare_control_segment`) — a `.Page`-suffixed or deeper
    // chain on a UserControl still falls through Step 0's decline above,
    // never fabricated.
    // -----------------------------------------------------------------------
    if let Some(rest) = receiver_lc.strip_prefix("currpage.")
        && let Some(control_name_lc) = parse_currpage_bare_control_segment(rest)
        && let Some(control) = find_page_control(&control_name_lc, from_object, graph, index)
        && control.kind == PageControlKind::UserControl
    {
        return resolve_control_addin_receiver(&control.target, from_object, graph, index);
    }

    // -----------------------------------------------------------------------
    // Step 2 — variable lookup (params → locals → the routine's own
    // named-return binding → object globals), via the shared
    // [`caller_scope_symbol`] helper (T3, receiver-closure-and-arg-increments
    // plan) — see its doc for the full proven precedence order and the
    // SAME-SCOPE-ONLY malformed-duplicate rule.
    //
    // NOTE: `rec`/`xrec` are looked up here too.  A Codeunit routine that
    // declares `var Rec: Record Customer` must resolve to
    // `Record{Some(customer_id)}`, not to `infer_implicit_rec(Codeunit)`
    // which would return `Unknown`.  The implicit-Rec IDENTITY fallback
    // fires only in Step 3b when NO variable named `rec`/`xrec` was found
    // (Step 3a, immediately below Step 2, independently handles a quoted
    // FIELD receiver — see its doc; the two never overlap since `rec`/
    // `xrec` are never written quoted).
    //
    // QUOTE-PARITY FIX (record-field chains plan, Task 4 round-2 addendum):
    // `receiver_lc` is sliced from RAW SOURCE TEXT (`full.rs`'s
    // `receiver_text.fold_identifier()`) and so RETAINS AL quote
    // characters for a quoted identifier (e.g. `"\"file blob\""`), while
    // `Param`/`VarDecl` names are stored ALREADY UNQUOTED — `ident_text`
    // (`al_syntax::lower`) strips the wrapping quotes at lowering time.
    // Comparing the two directly, as this step did before this fix, meant a
    // QUOTED identifier naming a real local/param/global var (e.g. a var
    // declared `"Sales Header Filter": Record "Sales Header"`, or a helper
    // local shadowing a field-like name, `"File Blob": Text[100]`) could
    // NEVER match here — it silently fell through past Step 2 instead, an
    // AL-scoping violation (a var/param/global ALWAYS shadows a same-named
    // field) that would have been unsound once Step 3a's field lookup
    // landed. `unquote_identifier` (this module's existing quote-stripping
    // helper, already used by `infer_compound_member_receiver`'s
    // member-name normalization) mirrors `ident_text`'s own convention
    // exactly, so the comparison key now sees what the var/param/global's
    // OWN unquoted name would have been for the identical source spelling.
    // Gated on the SAME bare-identifier shape Step 4 (below) already
    // established (no `.`/`(` — a genuinely compound receiver text is left
    // untouched here, since no real var/param/global name could ever equal
    // a multi-segment string anyway; the guard just keeps this step within
    // its own documented "bare identifier" scope).
    // -----------------------------------------------------------------------

    let lookup_lc: String = if is_atomic_receiver_token(receiver_lc) {
        unquote_identifier(receiver_lc)
    } else {
        receiver_lc.to_string()
    };

    match caller_scope_symbol(&lookup_lc, routine, object_globals) {
        // SAME-SCOPE malformed duplicate (T3 round-2 closer): the routine's
        // named-return binding collides with a param/local of the identical
        // name — never legal AL, so decline outright for this identifier
        // rather than guess which one wins.
        CallerScopeSymbol::MalformedDuplicate => return ReceiverType::Unknown,
        CallerScopeSymbol::Found(Some(ty)) => {
            return parsed_type_to_receiver(classify_type_text(ty), from_object, graph, index);
        }
        // Found but no declared type text, or not found at all in caller
        // scope — fall through (never a guess); Step 2b/3a/3b/4+ may still
        // resolve this identifier via a different mechanism entirely (a
        // dataitem name / implicit-Rec field / framework static name — none
        // of which are "the same symbol found here with no type").
        CallerScopeSymbol::Found(None) | CallerScopeSymbol::NotFound => {}
    }

    // -----------------------------------------------------------------------
    // Step 2b — report DATAITEM-NAME receiver (dataitem-receivers plan, Task
    // 1). Reached ONLY on a Step 2 miss — a var/param/global of the same name
    // ALWAYS shadows a dataitem (AL scoping; mirrors L2's `report_dataitem_
    // record_vars` skip-on-collision seeding, `ir_walk.rs:1864-1883` — a
    // precedent this fresh-engine step deliberately does NOT import, see the
    // module doc's clean-room note). Report/ReportExtension only.
    //
    // `lookup_lc` is the SAME quote-aware unquoted lookup key Step 2 just
    // computed, so a dot-bearing QUOTED dataitem name
    // (`"Sales Cr.Memo Header Filter"`, 5/16 of a real CDO report's dataitem
    // names) matches correctly here too — the naive dot-substring guard this
    // task's `is_atomic_receiver_token` replaces mislabeled it
    // `CompoundReceiver` before it could ever reach this step. A dataitem
    // name is in scope as a record var across ALL the report's routines (not
    // merely the enclosing dataitem's own trigger — see `ObjectDecl.
    // report_dataitems`'s doc), so this lookup is routine-independent.
    //
    // Fail-closed collisions (`resolve_dataitem_source_table`, below): a
    // same-named report PROCEDURE anywhere in the visible object(s) declines
    // (AL's parens-optional zero-arg call makes `Name.X()` structurally
    // ambiguous between "the dataitem record" and "a parens-less call to a
    // same-named procedure" — mirrors Step 3a's `table_scope_has_routine`
    // guard); a name duplicated across the own+extended-base dataitem maps
    // also declines (an unprovable ambiguity, never pick one).
    // -----------------------------------------------------------------------

    if matches!(
        from_object.id.kind,
        ObjectKind::Report | ObjectKind::ReportExtension
    ) && let Some(table_id) =
        resolve_dataitem_source_table(&lookup_lc, from_object, graph, index)
    {
        return ReceiverType::Record {
            table: Some(table_id),
        };
    }

    // -----------------------------------------------------------------------
    // Step 3a — bare implicit-Rec field receiver, QUOTED (record-field chains
    // plan, Task 4) AND UNQUOTED (implicit-self table fields, T3 receiver-
    // closure-and-arg-increments plan — widens the SAME machinery to drop the
    // quote requirement). Reached ONLY on a Step 2 miss — AL scoping means a
    // same-named local/param/global var (or, as of T3, the routine's own
    // named-return binding) ALWAYS shadows a field, and Step 2's quote-parity
    // fix (above) is exactly what makes that precedence correctly enforceable
    // for a quoted name; this step never runs before Step 2, and never
    // overrides a Step 2 hit.
    //
    // AL lets a Table/TableExtension procedure reference the implicit
    // `Rec`'s OWN field by BARE NAME with no `Rec.` prefix at all —
    // `"File Blob".CreateInStream(Stream)` (quoted) or
    // `Attachment.CreateInStream(Stream)` (unquoted) inside a Table's
    // procedure means exactly `Rec."File Blob"`/`Rec.Attachment...`. This
    // mirrors `resolver.rs`'s `resolve_bare` Step 3 implicit-Rec precedent for
    // BARE CALLS: the same STRICT `ObjectKind` guard, the same `with_state`
    // with-guard, and (as of pageext-merge-and-final-residual plan, Task 2)
    // the SAME per-kind table lookup (`resolver::implicit_rec_table_id`) —
    // widened from Table/TableExtension to ALSO cover Page/PageExtension via
    // the page's own `SourceTable` (Codeunit/Report(Extension) remain
    // excluded, exactly like `resolve_bare`'s Step 3 — a Codeunit's
    // `TableNo`/a Report's dataitems are a DIFFERENT mechanism, out of this
    // step's scope). The known real site:
    // `"View (Blob)".CreateInStream(ReadStream)` in Page 6175411's own
    // procedure (`CDOPageDefaultFilters.Page.al:88`), `"View (Blob)"` =
    // `field(28; ...; Blob)` on the page's SourceTable
    // (`CDOPageDefaultfilter.Table.al:35`).
    //
    // The with-guard requires the same `WithState::NoWithProven` proof
    // `resolve_bare`'s Step 3 requires — a bare reference inside an
    // un-modeled `with` block could silently mean a DIFFERENT record's
    // field — a false `Source` edge, the cardinal sin — sourced from the
    // same `bare_ctx` Steps 5/6 already thread through; a caller supplying
    // no `bare_ctx` — unit tests, `semantic_golden.rs` — makes this step a
    // no-op, exactly like Step 5.
    //
    // AMBIGUITY GUARD (round-2 soundness correction, PROVEN precedence layer
    // per T3's closer — "insert the arm AFTER the routine-shadow check"):
    // AL's parens are OPTIONAL on a zero-argument call (`Rec.Insert;`
    // compiles — the Code Cop AA0008 flags the missing parens as a STYLE
    // issue, not a compile error), so a bare name is structurally ambiguous
    // between a field reference and a parens-less call to a same-named
    // routine somewhere in the SAME visibility-scoped table surface. A
    // same-named routine anywhere in that surface (`ResolveIndex::
    // table_scope_has_routine`, checked FIRST) declines this step entirely —
    // never guess which of the two a bare name means. This is exactly why
    // fields sit LAST among value symbols: Step 2 (params/locals/
    // named-return binding/globals) and this routine-shadow check both run
    // BEFORE the field lookup below ever executes.
    //
    // PAGE/PAGEEXTENSION SELF-SHADOW (Task 2 widening, closes a gap
    // `table_scope_has_routine` alone does not cover): for Table/
    // TableExtension, `table_id` IS (or directly extends) `from_object`
    // itself, so `table_scope_has_routine` checking `table_id`'s own routine
    // surface incidentally ALSO checks the calling object's own routines.
    // For Page/PageExtension, `table_id` is a DIFFERENT object (the page's
    // SourceTable) — a routine the PAGE ITSELF declares (not the table)
    // sharing the bare name is a same-object precedence question
    // `table_scope_has_routine` cannot see, since it never inspects
    // `from_object`'s own routine set when `from_object` isn't table-kind.
    // `index.routines_in_object(&from_object.id, ..)` closes this
    // independently — a no-op for Table/TableExtension (already covered) and
    // the missing half for Page/PageExtension.
    //
    // `ResolveIndex::field_in_table` is itself the fail-closed gate (unique
    // visible match across base + closure-visible extensions, or `None`);
    // an unknown field name, an ambiguous duplicate, or a same-named routine
    // (table-scope OR the calling object's own) all fall through to Step 3b
    // / eventually `Unknown` — never a partial guess.
    //
    // UNQUOTED WIDENING (T3): `receiver_lc != "rec" && receiver_lc !=
    // "xrec"` defensively excludes the two identity spellings from this
    // widened unquoted branch — they are handled by Step 3b immediately
    // below, and though a field literally named `Rec`/`XRec` is not
    // meaningful AL (reserved-shaped, unquoted; would need to be quoted to
    // even declare), this keeps Step 3a from ever intercepting the identity
    // fallback even in a pathological/decompiled input. The quoted branch
    // needs no such exclusion — `"rec"`/`"xrec"` (quoted) never equals the
    // bare `rec`/`xrec` spelling Step 3b matches on.
    // -----------------------------------------------------------------------

    if is_atomic_receiver_token(receiver_lc)
        && (receiver_lc.starts_with('"') || (receiver_lc != "rec" && receiver_lc != "xrec"))
        && let Some((_, with_state)) = bare_ctx
        && with_state == WithState::NoWithProven
        && matches!(
            from_object.id.kind,
            ObjectKind::Table
                | ObjectKind::TableExtension
                | ObjectKind::Page
                | ObjectKind::PageExtension
        )
    {
        let table_id = implicit_rec_table_id(from_object, graph, index);
        if let Some(table_id) = table_id {
            let field_lc = unquote_identifier(receiver_lc);
            let routine_shadowed =
                index.table_scope_has_routine(graph, from_object, &table_id, &field_lc)
                    || !index
                        .routines_in_object(&from_object.id, &field_lc)
                        .is_empty();
            if !routine_shadowed
                && let Some(field) = index.field_in_table(graph, from_object, &table_id, &field_lc)
            {
                return parsed_type_to_receiver(
                    classify_type_text(&field.type_text),
                    from_object,
                    graph,
                    index,
                );
            }
        }
    }

    // -----------------------------------------------------------------------
    // Step 3b — implicit Rec / xRec identity (fallback: no variable named
    // rec/xrec found in Step 2; Step 3a's quoted-field lookup never applies
    // here since `receiver_lc` is never quoted for the literal `rec`/`xrec`
    // spelling).
    // -----------------------------------------------------------------------

    if receiver_lc == "rec" || receiver_lc == "xrec" {
        return infer_implicit_rec(routine, from_object, graph, index);
    }

    // -----------------------------------------------------------------------
    // Step 3c — platform singletons (T1.5, deep-review remediation plan —
    // formerly Step 1, which ran BEFORE Step 2's variable lookup; see the
    // module doc's retirement note). Reached only on a Step 2/2b/3a/3b miss —
    // a declared local/param/named-return/global var, a report dataitem
    // name, or an implicit-Rec table field ALL shadow a same-named platform
    // singleton; AL scoping means a declaration always wins over an ambient
    // compiler-provided identifier.
    //
    // Compiler-probed (`al.exe` 18.0.37.11445, `alc` backend, against real
    // `Microsoft_System_28.0.48590.0`/`Microsoft_Application`/`Microsoft_Base
    // Application`/`Microsoft_System Application`/`Microsoft_Business
    // Foundation_28.0.46665.48632` packages — the CDO workspace's own
    // `.alpackages`):
    //   - `var Session: Codeunit "Probe Shadow Target"` + `Session.DoWork()`
    //     (a real member of the DECLARED type, not of the platform `Session`
    //     singleton) compiles clean and dispatches to the declared
    //     codeunit — a declared var fully shadows the singleton. Control:
    //     with NO such var declared, the identical `Session.DoWork()` fails
    //     `error AL0132: 'Session' does not contain a definition for
    //     'DoWork'` — proving `DoWork` is not a real Session member and the
    //     positive case above is a genuine shadow, not a coincidental
    //     Session API match.
    //   - A same-named table FIELD wins too, which is why this step must run
    //     AFTER Step 3a, not merely after Step 2: a `field(2; Session; Blob)`
    //     on a Table lets `Session.CreateInStream(...)` compile bare inside
    //     that table's own procedure (`CreateInStream` is confirmed, by the
    //     same control technique, to not be a real Session member either).
    //   - None of `currpage`/`page`/`currreport`/`report`/`session`/
    //     `navapp`/`database`/`isolatedstorage`/`taskscheduler`/`system`/
    //     `companyproperty`/`sessioninformation` are AL reserved words — all
    //     twelve compile with ZERO diagnostics as a declared local/global var
    //     name, so none of them get early/exceptional treatment; every one
    //     is checked in this same step. `this` is the sole name with ANY
    //     compiler opinion: declaring/using it emits `warning AL0848: 'this'
    //     is a keyword from version '14.0'`, but both the declaration and
    //     the shadowed call still compile clean (a SOFT, warn-only
    //     reservation) — so `this` shadows exactly like its siblings and
    //     also moves here, matching the L3 sibling
    //     (`engine/l3/receiver_type.rs:283-318`) exactly, which checks every
    //     platform singleton — `this` included — in this identical position.
    // -----------------------------------------------------------------------

    if receiver_lc == "this" {
        return ReceiverType::SelfObject;
    }

    let singleton = match receiver_lc {
        "currpage" | "page" => Some(FrameworkKind::PageInstance),
        "currreport" | "report" => Some(FrameworkKind::ReportInstance),
        "session" => Some(FrameworkKind::Session),
        "navapp" => Some(FrameworkKind::NavApp),
        "database" => Some(FrameworkKind::Database),
        "isolatedstorage" => Some(FrameworkKind::IsolatedStorage),
        "taskscheduler" => Some(FrameworkKind::TaskScheduler),
        "system" => Some(FrameworkKind::System),
        "companyproperty" => Some(FrameworkKind::CompanyProperty),
        "sessioninformation" => Some(FrameworkKind::SessionInformation),
        _ => None,
    };
    if let Some(kind) = singleton {
        return ReceiverType::Framework(kind);
    }

    // -----------------------------------------------------------------------
    // Step 4 — static framework type name used as a static receiver
    // (`XmlDocument.Create(...)`, `Text.CopyStr(...)`, `Version.Create(...)`
    // — in each of these, `receiver_lc` is the BARE type name — `Create`/
    // `CopyStr` is the separate `method`, never part of `receiver_lc`
    // itself). A real variable of the same name would have been found in
    // Step 2 and would shadow this path. Only framework value types classify
    // here; Record/Object/Interface/Enum type names fall through to Unknown.
    //
    // BARE-IDENTIFIER GUARD (Task 4 fix; centralized as `is_atomic_receiver_
    // token` by the dataitem-receivers plan, Task 1): `classify_type_text`
    // only runs when `receiver_lc` is a genuine ATOMIC identifier (bare, or a
    // single quoted token with no unquoted `.`) — never on a COMPOUND
    // receiver text. Without this guard, a chained call whose
    // receiver is itself a further call/member expression rooted in an
    // `Xml*`-named base (e.g. the OUTER `.AsXmlNode()` in `XmlElement.
    // Create('root').AsXmlNode()`, whose `receiver_lc` is the WHOLE inner
    // text `"xmlelement.create('root')"`) would spuriously match
    // `classify_type_text`'s `s.starts_with("xml")` catch-all — a
    // fail-OPEN hole discovered while adding Task 4's Xml chain-table
    // entries: an untabled/wrong-arity Xml chain (e.g. the 0-arg
    // `XmlElement.Create()`, which this task deliberately leaves untabled)
    // would incorrectly short-circuit to `Framework(Xml)` HERE, bypassing
    // Steps 5/6's real per-hop chain-typing entirely, rather than declining.
    // Every other `classify_type_text` arm is an EXACT full-string match
    // (`"httpclient"`, `"jsonobject"`, …), which a multi-segment
    // `receiver_lc` could never satisfy — `"xml"` is the ONLY prefix
    // wildcard, so this guard is the general, principled fix (matches this
    // step's own doc: "bare identifier"), not an Xml-specific patch.
    // Steps 5/6 (compound receiver chains, including the SAME `Xml` case)
    // remain fully unaffected — they operate on `receiver_expr`'s STRUCTURED
    // AST node, never on this string, and already type each hop's base via
    // its own recursive bare-identifier call ([`infer_receiver_type_for_expr`]'s
    // `Identifier` arm), which was never subject to this bug.
    // -----------------------------------------------------------------------

    if is_atomic_receiver_token(receiver_lc)
        && let ParsedType::Framework(kind) = classify_type_text(receiver_lc)
    {
        return ReceiverType::Framework(kind);
    }

    // -----------------------------------------------------------------------
    // Step 4b — bare enum-type-name receiver (Task 4, receiver-closure-and-
    // arg-increments plan — site (G): `"CDO Send on Posting".FromInteger(...)`).
    //
    // AL lets an Enum TYPE be referenced by its bare name (quoted or not, no
    // `Enum::` prefix) as a receiver for the TYPE-static surface
    // (`FromInteger`/`Names`/`Ordinals`) — but UNLIKE `Enum::"Type"`
    // (`infer_receiver_type_for_expr`'s `QualifiedEnum` arm, unambiguous by
    // construction: `Enum::` is reserved grammar, no variable/field/routine
    // could ever be named that), a BARE name is syntactically identical to a
    // var/field/routine reference. Steps 2/3a above already prove there is no
    // param/local/named-return/global/field shadow for `receiver_lc` (a hit
    // there would have returned already) — this step adds the two checks
    // Steps 2/3a do NOT cover: a same-named ROUTINE reachable via a
    // parens-less bare call, and the programmatic Enum-name collision rule.
    //
    // Gate (round-2 closer, BINDING — ALL must hold, fail-closed):
    // 1. Exactly ONE `Enum` object resolves to this name in `from_object`'s
    //    dependency closure (`ResolveIndex::resolve_object_ref`, the SAME
    //    fail-closed primitive `infer_receiver_type_for_expr`'s `Enum::"Type"`
    //    arm uses).
    // 2. Zero objects of ANY OTHER kind share the identical normalized name,
    //    ANYWHERE in the whole graph (`enum_type_name_collision_free` —
    //    deliberately NOT closure-scoped, per the round-2 closer's literal
    //    "over the whole object index": a same-name Table in an unrelated app
    //    is still a real naming collision this engine has no compiler-level
    //    disambiguation for, so it must decline rather than assume the Enum
    //    reading).
    // 3. No same-named routine reachable via a parens-less bare call
    //    (`object_scope_has_bare_routine_shadow` — mirrors Step 3a's
    //    `table_scope_has_routine` precedent, generalized to every object
    //    kind since a routine-name shadow is not table-specific).
    //
    // WITH-GUARD (Task 3, roadmap-closure plan — symmetry fix): the SAME
    // `bare_ctx`/`WithState::NoWithProven` gate Step 3a requires (above) is
    // required here too. A bare enum-type-name reference is exactly as
    // syntactically ambiguous as a bare field reference — inside an
    // un-modeled `with` block, `"CDO Send on Posting".FromInteger(...)`
    // could actually mean a FIELD of the (unproven) with-target record
    // rather than the enum's type-static surface, which this step had no
    // way to rule out before this fix (unlike Step 3a's arm, this one is not
    // restricted to Table/Page-kind objects — a `with` block can wrap ANY
    // record-typed receiver in ANY object kind, so the gate is unconditional
    // here rather than paired with an object-kind `matches!`). No `bare_ctx`
    // supplied (unit tests, `semantic_golden.rs`, the `RecordOp` shape) makes
    // this step a no-op exactly like Step 3a and Step 5 — resolution-neutral
    // for those callers, never a regression.
    // -----------------------------------------------------------------------

    if is_atomic_receiver_token(receiver_lc)
        && let Some((_, with_state)) = bare_ctx
        && with_state == WithState::NoWithProven
    {
        let name_raw = unquote_identifier(receiver_lc);
        let name_lc = name_raw.fold_identifier();
        let object_ref = ObjectRef::Name {
            raw: name_raw,
            normalized_lc: name_lc.clone(),
        };
        if let ObjectRefResolution::Unique(_) =
            index.resolve_object_ref(graph, from_object.id.clone(), ObjectKind::Enum, &object_ref)
            && enum_type_name_collision_free(&name_lc, graph)
            && !object_scope_has_bare_routine_shadow(from_object, &name_lc, graph, index)
        {
            return ReceiverType::EnumTypeStatic { name_lc };
        }
    }

    // -----------------------------------------------------------------------
    // Step 5 — compound call-result receiver (`Func().Method()`, Task 3).
    //
    // Only engages when BOTH `receiver_expr` (the parsed receiver node, Task
    // 2) and `bare_ctx` (the `DeclSurface`/`WithState` Step 5 needs to run
    // `resolve_bare` as a type query, Task 3) are populated — a no-op
    // otherwise, so callers that don't supply them (unit tests,
    // `semantic_golden.rs`, the `RecordOp` shape) are unaffected.
    // -----------------------------------------------------------------------

    if let Some((file, expr_id)) = receiver_expr
        && let Some((surface, with_state)) = bare_ctx
        && let Some(recv) = infer_call_result_receiver(
            file,
            expr_id,
            routine,
            object_globals,
            from_object,
            graph,
            index,
            surface,
            with_state,
        )
    {
        return recv;
    }

    // -----------------------------------------------------------------------
    // Step 6 — compound framework property/method + `this.<rest>` receiver
    // (beyond-1B.3b Task 4) + cross-object call-result chain receiver
    // (`Var.Method().X()`, plan v2.1 Task 3 — see [`infer_compound_member_receiver`]'s
    // new arm).
    //
    // The framework/`this.<rest>` sub-cases only need `receiver_expr` (Task
    // 2) — unlike Step 5, they never call `resolve_bare`, so they do NOT
    // gate on `bare_ctx`. The NEW cross-object-chain sub-case DOES need a
    // `DeclSurface` (it calls `resolve_member` as a type-query, which needs one
    // to build routes) — threaded here as `Option<&DeclSurface>` extracted
    // from `bare_ctx`, so it is a no-op for callers with no `bare_ctx` in
    // scope (unit tests that pass `None`, `semantic_golden.rs`, the
    // `RecordOp` shape), exactly like Step 5, while the framework/`this.`
    // sub-cases remain resolution-neutral either way.
    // -----------------------------------------------------------------------

    if let Some((file, expr_id)) = receiver_expr {
        let recv = infer_receiver_type_for_expr(
            file,
            expr_id,
            routine,
            object_globals,
            from_object,
            graph,
            index,
            bare_ctx.map(|(surface, _)| surface),
        );
        if !matches!(recv, ReceiverType::Unknown) {
            return recv;
        }
    }

    // -----------------------------------------------------------------------
    // Step 7 — Unknown.
    // -----------------------------------------------------------------------

    ReceiverType::Unknown
}

/// The programmatic Enum collision rule (Task 4, receiver-closure-and-arg-
/// increments plan, round-2 closer — BINDING: `same_normalized_name &&
/// object_kind != Enum` over the WHOLE object index, never a hardcoded kind
/// subset). Returns `true` when NO object of any non-`Enum` kind anywhere in
/// `graph.objects` shares `name_lc` — i.e. it is safe to interpret a bare
/// name as the enum TYPE reference. Deliberately whole-graph, not
/// closure-scoped: a same-name Table in an app `from_object` doesn't even
/// depend on is still a genuine naming collision this engine cannot resolve
/// the real AL compiler's disambiguation for, so it must decline rather than
/// assume the Enum reading.
fn enum_type_name_collision_free(name_lc: &str, graph: &ProgramGraph) -> bool {
    !graph
        .objects
        .iter()
        .any(|o| o.id.kind != ObjectKind::Enum && o.name.eq_fold_identifier(name_lc))
}

/// Whether a same-named ROUTINE is reachable from `from_object` via a
/// parens-less bare call — the routine-shadow half of Step 4b's gate. A bare
/// enum-type-name receiver is syntactically identical to a parens-less call
/// to a same-named procedure (AL's parens-optional rule), so this must
/// decline exactly like Step 3a's `ResolveIndex::table_scope_has_routine`
/// precedent for fields — generalized here to every object kind (a routine
/// shadow is not table-specific the way a FIELD shadow is): for
/// Table/TableExtension, mirrors `table_scope_has_routine`'s base+extension
/// visibility-scoped search; for every other object kind, checks the
/// object's OWN declared routines directly (`ResolveIndex::routines_in_object`).
fn object_scope_has_bare_routine_shadow(
    from_object: &ObjectNode,
    name_lc: &str,
    graph: &ProgramGraph,
    index: &ResolveIndex,
) -> bool {
    match from_object.id.kind {
        ObjectKind::Table => {
            index.table_scope_has_routine(graph, from_object, &from_object.id, name_lc)
        }
        ObjectKind::TableExtension => {
            match resolve_tableext_base_table(from_object, graph, index) {
                Some(table_id) => {
                    index.table_scope_has_routine(graph, from_object, &table_id, name_lc)
                }
                // Base table unresolvable: conservative — cannot PROVE no
                // routine shadow exists, so treat as a shadow (decline the
                // whole Step 4b gate) rather than risk a wrong pick.
                None => true,
            }
        }
        _ => !index
            .routines_in_object(&from_object.id, name_lc)
            .is_empty(),
    }
}

/// Step 6's AST-native entry point: type an arbitrary `Expr` node directly
/// from the IR — never by re-parsing source text — recursing through
/// `Member`/`Call` chains to find a `Framework`-typed base for the compound
/// framework-property/method step, or the bare `this` identifier for the
/// `this.<rest>` step (both in [`infer_compound_member_receiver`]).
///
/// Dispatch:
/// - `Identifier`/`QuotedIdentifier` — the base case: type it exactly like a
///   bare receiver name via [`infer_receiver_type`]'s Steps 0-4 (`receiver_expr`
///   and `bare_ctx` both `None` — this deliberately does NOT recurse into
///   Steps 5-6 again for a bare identifier; Step 4's `rec`/singleton/framework
///   lookup is Step 6's whole base case, so recursing further here would only
///   ever re-derive the same `Unknown` a second time, never additional
///   coverage. Terminates by construction — no cycle risk).
///
///   **Quote-parity guard (round-2 fix):** the IR's `QuotedIdentifier(name)`
///   stores `name` ALREADY UNQUOTED (the lowerer strips quotes — see
///   `extract.rs`'s `classify_call`), whereas the TOP-LEVEL `receiver_lc`
///   [`infer_receiver_type`] itself dispatches on is sliced from RAW SOURCE
///   TEXT and so ALWAYS retains any quote characters. Feeding the bare
///   unquoted name into a fresh `infer_receiver_type` call would therefore
///   run Steps 0-4 on a DIFFERENT string than the top-level call would have
///   seen for the same site — concretely, Step 4's naive first-whitespace-
///   token match (`classify_type_text`) can then spuriously match a quoted
///   FIELD name that merely STARTS WITH a framework keyword word (e.g. a
///   `Blob` field literally named `"File Blob"` unquotes to `"file blob"`,
///   whose first token `"file"` collides with the `File` framework type —
///   verified as a REAL CDO false-positive during this task's CDO gate: the
///   table's own implicit-Rec field `"File Blob"` was mis-typed
///   `Framework(File)` and `.CreateInStream`/`.CreateOutStream` false-
///   resolved to the `File` catalog instead of staying the honest
///   `Unknown` a Blob FIELD reference correctly is — the cardinal sin this
///   whole plan exists to prevent). Field-type indexing was itself the
///   DEFERRED record-field mechanism at the time this guard was written; it
///   has since LANDED (record-field chains plan Task 3, see
///   [`infer_compound_member_receiver`]'s record-field arm), but this
///   quote-parity guard remains load-bearing regardless — it protects EVERY
///   Step-4 framework-name lookup a quoted field/var name could spuriously
///   collide with, not only the now-resolved Blob-field case. So a
///   `QuotedIdentifier` is RE-QUOTED before the recursive call, exactly
///   reproducing what `receiver_text.fold_identifier()` would have
///   produced for the same source site — restoring BYTE-FOR-BYTE parity
///   with Steps 0-4's existing (conservative) quoted-name behavior, never
///   granting quoted names new resolving power Task 4 doesn't intend to add.
/// - `Member{object, member, ..}` — the property-access form (`<base>.<member>`,
///   no parens): delegate to [`infer_compound_member_receiver`] with
///   `is_method: false`, `arity: 0`.
/// - `Call{function, args}` whose `function` derefs to `Member{object, member,
///   ..}` — the method-call form (`<base>.<member>(args)`): delegate to
///   [`infer_compound_member_receiver`] with `is_method: true`,
///   `arity: args.len()`. A `Call` whose `function` is anything else (a bare
///   identifier call, i.e. the Step-5 shape already handled at the TOP level
///   only — not recursively here) declines.
/// - Anything else (`Index`, `Literal`, `Binary`, …) — declines. Fail-closed by
///   construction: every arm either delegates to more fail-closed logic or
///   returns `Unknown` directly.
///
/// `surface` (plan v2.1 Task 3 enabling primitive): `Some` when the caller
/// can supply the `DeclSurface` [`infer_compound_member_receiver`]'s new
/// cross-object call-result chain arm needs to run `resolve_member` as a
/// type-query; `None` for callers with no such context in scope — that arm
/// is then a no-op there, exactly like [`infer_receiver_type`]'s `bare_ctx`.
/// Threaded unchanged through every recursive call so a multi-hop chain's
/// BASE typing (itself possibly another compound receiver) can reach the new
/// arm too — a 3-level chain whose middle hop cannot be typed (no
/// `surface`, or the middle hop itself declines) correctly propagates
/// `Unknown` rather than partially guessing.
#[allow(clippy::too_many_arguments)] // 7 pre-existing params + `surface` (plan v2.1 Task 3); each is a distinct identity/lookup input, grouping would obscure the recursive call sites.
fn infer_receiver_type_for_expr(
    file: &AlFile,
    expr_id: ExprId,
    routine: &RoutineDecl,
    object_globals: &[VarDecl],
    from_object: &ObjectNode,
    graph: &ProgramGraph,
    index: &ResolveIndex,
    surface: Option<&DeclSurface>,
) -> ReceiverType {
    match &file.ir.expr(expr_id).kind {
        ExprKind::Identifier(name) => {
            let name_lc = name.fold_identifier();
            infer_receiver_type(
                &name_lc,
                routine,
                object_globals,
                from_object,
                graph,
                index,
                None,
                None,
            )
        }
        ExprKind::QuotedIdentifier(name) => {
            // Quote-parity guard — see this function's doc. Re-quote so
            // Steps 0-4 see EXACTLY the string the top-level `receiver_lc`
            // (sliced from raw source text) would have carried for the same
            // site, never a bare unquoted name a quoted field/var reference
            // never actually is.
            let requoted_lc = format!("\"{}\"", name.fold_identifier());
            infer_receiver_type(
                &requoted_lc,
                routine,
                object_globals,
                from_object,
                graph,
                index,
                None,
                None,
            )
        }
        ExprKind::Member { object, member, .. } => infer_compound_member_receiver(
            file,
            *object,
            member,
            false,
            0,
            routine,
            object_globals,
            from_object,
            graph,
            index,
            surface,
        ),
        ExprKind::Call { function, args } => {
            if let ExprKind::Member { object, member, .. } = &file.ir.expr(*function).kind {
                infer_compound_member_receiver(
                    file,
                    *object,
                    member,
                    true,
                    args.len(),
                    routine,
                    object_globals,
                    from_object,
                    graph,
                    index,
                    surface,
                )
            } else {
                // A bare-identifier call (`Func(...)`) reaching HERE (i.e. as
                // the BASE of a deeper chain, not the top-level receiver) is
                // the Step-5 shape recursed one level deeper than Step 5
                // handles — deliberately out of scope (single-hop
                // `<Framework>.<rest>`/`this.<rest>`/cross-object chains
                // target the OUTER receiver only, not nested bare-call
                // chains); decline rather than guess.
                ReceiverType::Unknown
            }
        }
        // `Enum::Value` / `Enum::"Type"` (Task 4, receiver-closure-and-arg-
        // increments plan — sites (D)/(F)). By AL grammar construction
        // (`qualified_enum_value` in tree-sitter-al's grammar.js), the ONLY
        // legitimate uses of `X::Y` outside the dedicated `database_reference`
        // rule (Database/Page/Report/Codeunit/XmlPort/Query — a SEPARATE
        // grammar rule) are: (a) `Enum::"Type"`, the TYPE reference itself
        // (`enum_type` derefs to the literal `Enum` keyword — lowered as
        // `ExprKind::Identifier("Enum")` per `RawKind::KeywordIdentifier`'s
        // lowering), and (b) a QUALIFIED VALUE literal (`enum_type` is
        // anything else — a field/member chain, a nested `QualifiedEnum` for
        // `Enum::"Type"::"Value"`, a subscript/call-result base, …).
        //
        // TRUE INVARIANT (Task 4 review fix — corrects an earlier, FALSE
        // "grammar-level guarantee" claim this comment used to make): the
        // grammar only guarantees the SHAPE `X::Y`, not that `X` is
        // Enum-typed. `qualified_enum_value.enum_type` also accepts a
        // Member/field-access whose declared type is something else
        // entirely — most notably an **Option**-typed field/var
        // (`Rec."Legacy Status"::Open`, common legacy AL), which parses to
        // the IDENTICAL `QualifiedEnum` shape as a genuine Enum field.
        // Trusting every non-keyword shape as enum-VALUE-typed blind would
        // be a guess, not a proof — so this arm now recurses the SAME
        // base-typing every other compound-receiver arm here already uses
        // ([`infer_receiver_type_for_expr`]) on `enum_type` itself, and only
        // accepts the VALUE-instance surface when that base ACTUALLY
        // resolves to an Enum shape (`EnumType` — a declared `Enum "X"`
        // var/field, or `EnumTypeStatic` for the nested
        // `Enum::"Type"::"Value"` case). Anything else (`Primitive` for an
        // Option field, `Record`, `Unknown`, …) declines — never guess.
        ExprKind::QualifiedEnum { enum_type, value } => {
            let is_enum_keyword = matches!(
                &file.ir.expr(*enum_type).kind,
                ExprKind::Identifier(n) if n.eq_ignore_ascii_case("enum")
            );
            if is_enum_keyword {
                // `Enum::"Type"` — the TYPE-static receiver. Fail-closed
                // existence check (task brief: "resolve the enum object,
                // fail-closed") — `value` is a raw string sliced from source
                // with no parser-level guarantee it names a real Enum object
                // (unlike a declared var's type text, which the AL compiler
                // itself already validated), so a typo'd/renamed enum name
                // must decline here, never be trusted blind.
                let name_raw = unquote_identifier(value);
                let name_lc = name_raw.fold_identifier();
                let object_ref = ObjectRef::Name {
                    raw: name_raw,
                    normalized_lc: name_lc.clone(),
                };
                return match index.resolve_object_ref(
                    graph,
                    from_object.id.clone(),
                    ObjectKind::Enum,
                    &object_ref,
                ) {
                    ObjectRefResolution::Unique(_) => ReceiverType::EnumTypeStatic { name_lc },
                    ObjectRefResolution::Ambiguous
                    | ObjectRefResolution::OutOfClosure
                    | ObjectRefResolution::Unresolved => ReceiverType::Unknown,
                };
            }
            // Any other `X::Value` shape: verify `enum_type` ACTUALLY types
            // Enum before accepting VALUE-instance dispatch (see this arm's
            // doc — the TRUE invariant, not a grammar guarantee). `name_lc`
            // is not consulted by dispatch (every `EnumType` arm matches
            // `{ .. }`), so once the base is proven Enum-shaped no further
            // typing of `enum_type` itself is needed.
            match infer_receiver_type_for_expr(
                file,
                *enum_type,
                routine,
                object_globals,
                from_object,
                graph,
                index,
                surface,
            ) {
                ReceiverType::EnumType { .. } | ReceiverType::EnumTypeStatic { .. } => {
                    ReceiverType::EnumType {
                        name_lc: String::new(),
                    }
                }
                _ => ReceiverType::Unknown,
            }
        }
        _ => ReceiverType::Unknown,
    }
}

/// Step 6's shared implementation for both sub-cases (framework-property/method
/// chain and `this.<rest>`) — dispatches on whether `object_expr_id` is
/// literally the bare `this` identifier.
///
/// - **`this.<rest>`**: when `object_expr_id` derefs to `Identifier`/
///   `QuotedIdentifier` matching `"this"` (case-insensitively — AL identifiers
///   are case-insensitive), `is_method: true` (a `this.Method(...)` CALL form)
///   declines immediately — deliberately DEFERRED (see the module doc's Step
///   6b): typing a same-object procedure's return type needs
///   `resolve_bare`-style routine lookup, out of this step's scope. The
///   property form (`is_method: false`) resolves `member` via
///   [`infer_this_member`] against the SELF-ONLY `object_globals` scope.
/// - **Framework chain**: recursively type `object_expr_id` via
///   [`infer_receiver_type_for_expr`]; if it resolves to `Framework(kind)`,
///   look up `(kind, member_lc, is_method, arity)` via the CONTEXT-SENSITIVE
///   [`zero_arg_aware_lookup`] wrapper around the versioned
///   [`framework_return_kind`] table (receiver-closure plan v2.1 Task 2 — see
///   that function's doc for the parens-optional fallback rule). A lookup
///   miss declines IMMEDIATELY (correction, Task 4: does NOT fall through to
///   the cross-object-chain arm below — a `Framework` base has no
///   source/ABI procedures to type-query, so falling through could never
///   resolve anything there anyway; this arm's `if let` unconditionally
///   `return`s either the mapped kind or `Unknown`).
/// - **`RecordRef`/`FieldRef`/`KeyRef` chain** (Task 4, chain-tables plan):
///   the SAME recursive base-typing; if it resolves to one of the three
///   `*Ref` unit variants, look up `(kind, member_lc, is_method, arity)` via
///   the SAME [`zero_arg_aware_lookup`] wrapper around the versioned
///   [`recordref_family_return_kind`] table (a DISTINCT family from
///   `framework_return_kind`; every entry there is currently arity ≥1, so
///   the wrapper is a behavior-preserving no-op for this family today — see
///   the Task 2 report's pre-flip audit). A lookup miss also declines
///   IMMEDIATELY, for the identical reason — a `*Ref` base has no
///   source/ABI procedures to type-query either.
/// - **Cross-object call-result chain** (plan v2.1 Task 3): STRICTLY the
///   procedure-CALL form (`is_method`; a bare `Member` — a field/property
///   access — is never this arm, round-1 I7). When `base_ty` is `Object`/
///   `Record`/`SelfObject`/`Interface` (proven by the SAME recursive typing
///   above) and a `surface` is available, types the base call's RETURN
///   TYPE via a PURE [`resolve_member`] type-query — see
///   [`infer_cross_object_chain_receiver`] for the full guard. Untyped/
///   `Unknown`/`Primitive`/`Dynamic`/`*Ref` bases, or any decline along the
///   way, fall through to `Unknown` — never a partial guess.
///
/// # Context-sensitive zero-arg lookup (receiver-closure plan v2.1 Task 2)
///
/// [`zero_arg_aware_lookup`] wraps every `(.., is_method, arity)` table probe
/// this function makes ([`framework_return_kind`], [`recordref_family_return_kind`],
/// [`enum_chain_return_kind`]) — see its doc for the parens-optional fallback
/// rule. It replaces this function's PRIOR contract, which passed `is_method`/
/// `arity` straight through unmodified: that was WRONG for a bare `Member`
/// hop, because AL's parens are OPTIONAL on a zero-arg procedure call (the
/// user's standing correction — see the al-parens-optional-procedure-calls
/// memory; also documented in this module's ROUND-2 SOUNDNESS CORRECTION
/// above, which fixed the identical error for the record-field arm below).
/// `Response.Content.ReadAs(X)` (no parens on `Content`) is structurally
/// IDENTICAL AST to a genuine `Content` FIELD/property read — both parse to
/// `ExprKind::Member{is_method: false}` — so a bare `Member` hop must ALSO
/// try the table's zero-arg METHOD row before declining, not just the exact
/// (`is_method: false`) row.
#[allow(clippy::too_many_arguments)] // mirrors infer_receiver_type_for_expr's identity/lookup inputs plus member/is_method/arity — grouping would obscure the dispatch.
fn infer_compound_member_receiver(
    file: &AlFile,
    object_expr_id: ExprId,
    member: &str,
    is_method: bool,
    arity: usize,
    routine: &RoutineDecl,
    object_globals: &[VarDecl],
    from_object: &ObjectNode,
    graph: &ProgramGraph,
    index: &ResolveIndex,
    surface: Option<&DeclSurface>,
) -> ReceiverType {
    // `member` (from `ExprKind::Member`/`Call{function: Member{..}}`) may
    // itself be RAW WITH QUOTES (mirrors `extract.rs::classify_call`'s own
    // `strip_quote_chars(member)` before use) — unquote before matching so a
    // quoted member name (`Response."Content"()`, however rare in practice)
    // normalizes the same way an unquoted one does, rather than silently
    // missing the table via a stray embedded quote character.
    let member_lc = unquote_identifier(member).fold_identifier();

    if is_this_identifier(file, object_expr_id) {
        if is_method {
            // `this.Method(...)` call-result chaining — deferred, decline.
            return ReceiverType::Unknown;
        }
        return infer_this_member(&member_lc, object_globals, from_object, graph, index);
    }

    let base_ty = infer_receiver_type_for_expr(
        file,
        object_expr_id,
        routine,
        object_globals,
        from_object,
        graph,
        index,
        surface,
    );

    if let ReceiverType::Framework(kind) = &base_ty {
        if let Some(returned) = zero_arg_aware_lookup(is_method, arity, |m, a| {
            framework_return_kind(kind, &member_lc, m, a)
        }) {
            return ReceiverType::Framework(returned);
        }
        return ReceiverType::Unknown;
    }

    // `RecordRef`/`FieldRef`/`KeyRef` chain (Task 4, chain-tables plan) —
    // same fail-closed mechanism as the `Framework` arm just above, a
    // DISTINCT family (`recordref_returns::recordref_family_return_kind`):
    // a table-miss declines immediately, same as `Framework`'s table-miss —
    // it does NOT fall through to the cross-object-chain arm below (a `*Ref`
    // base has no source/ABI procedures to type-query either, exactly like
    // `Framework`).
    if let Some(family) = RecordRefFamilyKind::from_receiver_type(&base_ty) {
        if let Some(returned) = zero_arg_aware_lookup(is_method, arity, |m, a| {
            recordref_family_return_kind(&family, &member_lc, m, a)
        }) {
            return returned.to_receiver_type();
        }
        return ReceiverType::Unknown;
    }

    // EnumType-as-chain-base (Task 3, record-field chains plan): `Ordinals()`/
    // `Names()` invoked on an Enum VALUE receiver (typically reached one hop
    // up via the record-field arm just below, e.g. `Rec."Doc Status".
    // Ordinals().Count()`) both return `List of [...]` — see
    // `enum_chain_return_kind`'s doc. Same immediate-decline-on-miss
    // discipline as the `Framework`/`RecordRef`-family arms above: an
    // `EnumType` base has no source/ABI procedures to type-query either, so a
    // table miss never falls through to the cross-object-chain arm below.
    if let ReceiverType::EnumType { .. } = &base_ty {
        if let Some(returned) = zero_arg_aware_lookup(is_method, arity, |m, a| {
            enum_chain_return_kind(&member_lc, m, a)
        }) {
            return ReceiverType::Framework(returned);
        }
        return ReceiverType::Unknown;
    }

    // EnumTypeStatic-as-chain-base (Task 4, receiver-closure-and-arg-
    // increments plan): `Ordinals()`/`Names()` invoked on the enum TYPE
    // reference itself (`Enum::"Type".Ordinals()`, real CDO sites (F)) return
    // the SAME `List of [...]` shape as the VALUE-instance chain just above —
    // reuses the identical `enum_chain_return_kind` table (both surfaces agree
    // on Ordinals/Names; only `AsInteger`/`FromInteger` differ between the two
    // — see `member_catalog.rs`'s split). `FromInteger`'s own chain-return
    // (the type itself) stays OUT of this table by the SAME deliberate
    // exclusion `enum_chain_return_kind`'s doc already documents for the
    // VALUE-instance case — no measured CDO need to chain PAST it.
    if let ReceiverType::EnumTypeStatic { .. } = &base_ty {
        if let Some(returned) = zero_arg_aware_lookup(is_method, arity, |m, a| {
            enum_chain_return_kind(&member_lc, m, a)
        }) {
            return ReceiverType::Framework(returned);
        }
        return ReceiverType::Unknown;
    }

    // Record-field member access (`Rec."Field".X()` / `Rec.Field.X()`) — Task
    // 3, record-field chains plan. STRICTLY the non-method (bare `Member`,
    // never a `Call`) AST shape: `!is_method` — the exact opposite gate of
    // the cross-object-chain arm just below.
    //
    // ROUND-2 SOUNDNESS CORRECTION: a bare `Member{object, member}` node
    // (`is_method: false`, no argument list AT ALL — not even an empty
    // `()`) is NOT proof this is a field/property access. AL's parens are
    // OPTIONAL on a zero-argument call (`Rec.Insert;` compiles — the Code
    // Cop AA0008 flags the missing parens as a STYLE issue, not a compile
    // error): a parens-less call to a same-named PROCEDURE parses to the
    // IDENTICAL AST shape as a field reference. (This doc previously claimed
    // "a bare `Member` is never a procedure-call chain" — true as the
    // `is_method` GATE distinguishing this arm from the cross-object-CHAIN
    // arm below, but wrong as a claim that `!is_method` rules out a
    // procedure call altogether; a parens-less call is exactly such a case,
    // just not a *chain*.) So: a same-named ROUTINE anywhere in the SAME
    // visibility-scoped table surface (`ResolveIndex::table_scope_has_
    // routine`, base + closure-visible extensions — checked FIRST, before
    // the field lookup) declines this arm entirely — never guess which of
    // the two `member_lc` means.
    //
    // Only engages when `base_ty` proves a `Record` receiver with a
    // RESOLVED table (`table: Some(..)` — an out-of-closure/unresolved
    // table has no field surface to consult and falls through to
    // `Unknown`, the same fail-closed contract every other arm here uses).
    // `member_lc` already handles BOTH a quoted (`"Error Message"`) and
    // unquoted (`BlobField`) member name identically (see this function's
    // top — `Rec.` syntactically disambiguates a field access from a
    // bare-identifier variable reference either way, so both spellings are
    // safe to route through the SAME field lookup).
    //
    // `ResolveIndex::field_in_table` is itself the fail-closed gate (unique
    // visible match or `None`); a lookup miss (unknown field name, ambiguous
    // duplicate, a same-named routine, or the base object simply isn't
    // Table/TableExtension) falls through past this arm to the final
    // `Unknown` below, exactly like every other declined arm.
    if !is_method
        && let ReceiverType::Record {
            table: Some(table_id),
        } = &base_ty
        && !index.table_scope_has_routine(graph, from_object, table_id, &member_lc)
        && let Some(field) = index.field_in_table(graph, from_object, table_id, &member_lc)
    {
        let field: FieldNode = field;
        let parsed = classify_type_text(&field.type_text);
        return parsed_type_to_receiver(parsed, from_object, graph, index);
    }

    // Cross-object call-result chain (plan v2.1 Task 3) — see this
    // function's doc. `is_method` gates the shape (procedure-CALL form
    // only); `surface` gates on the caller having supplied one
    // (resolution-neutral otherwise, mirrors Step 5's `bare_ctx` gate).
    if is_method
        && let Some(bm) = surface
        && matches!(
            base_ty,
            ReceiverType::Object { .. }
                | ReceiverType::Record { .. }
                | ReceiverType::SelfObject
                | ReceiverType::Interface { .. }
        )
        && let Some(recv) = infer_cross_object_chain_receiver(
            &base_ty,
            &member_lc,
            arity,
            from_object,
            graph,
            index,
            bm,
        )
    {
        return recv;
    }

    ReceiverType::Unknown
}

/// Context-sensitive AL zero-arg member-table lookup (receiver-closure plan
/// v2.1 Task 2; the parens-optional correction — see the
/// al-parens-optional-procedure-calls memory, and this module's ROUND-2
/// SOUNDNESS CORRECTION doc comment on the record-field arm, which fixed the
/// identical error for a DIFFERENT arm earlier).
///
/// AL's parens are OPTIONAL on a zero-arg procedure call: `Response.Content`
/// (no parens) and `Response.Content()` (parens) invoke the SAME zero-arg
/// procedure when `Content` is one — and the no-parens form is INDISTINGUISHABLE
/// at the AST level from a genuine property/field read (both parse to
/// `ExprKind::Member`, `is_method: false`). Every table in this family
/// ([`framework_return_kind`], [`recordref_family_return_kind`],
/// [`enum_chain_return_kind`]) is keyed on `(.., is_method, arity)`, so a bare
/// `Member` node must be tried against BOTH the exact property row
/// (`is_method: false`, kept for a future table that adds one — see the Task
/// 2 report's pre-flip audit: neither table has one today) AND the zero-arg
/// METHOD row (`is_method: true, arity: 0`, the parens-less-call case) as a
/// fallback — trying only one silently drops real call sites.
///
/// - A genuine `Call` node (`is_method: true`) is UNCHANGED: `lookup` is
///   invoked with the caller's real `(is_method, arity)` directly, no
///   fallback — the `Call` AST shape already proves a procedure invocation
///   (parenthesized, real args), so there is no property-vs-method ambiguity
///   left to resolve. A zero-arg `Call` (`Response.Content()`) still reaches
///   `lookup(true, 0)` exactly as before.
/// - A bare `Member` (`is_method: false`, `arity` is always `0` at every call
///   site — see [`infer_receiver_type_for_expr`]'s `Member` arm) tries the
///   property row (`lookup(false, 0)`) FIRST, then the method row
///   (`lookup(true, 0)`) as a fallback.
/// - Both rows existing with DIFFERING return kinds is a fail-closed conflict
///   (`None` — the caller declines this arm, same as a table miss): the
///   table cannot honestly claim which the source site means without deeper
///   AST evidence this lookup doesn't have. UNREACHABLE today (audited: no
///   `is_method: false` row exists in any of the three tables yet), but the
///   branch exists so a FUTURE property-row addition can't silently
///   mis-resolve a real ambiguity.
/// - Both rows existing with the SAME return kind resolves normally — no
///   ambiguity, both readings agree.
fn zero_arg_aware_lookup<T: PartialEq>(
    is_method: bool,
    arity: usize,
    lookup: impl Fn(bool, usize) -> Option<T>,
) -> Option<T> {
    if is_method {
        return lookup(true, arity);
    }
    // A bare `Member` (`is_method: false`) is structurally a zero-arg
    // form by construction — the ONE call site that ever constructs one
    // (`infer_receiver_type_for_expr`'s `ExprKind::Member` arm) hardcodes
    // `arity: 0` literally, since `ExprKind::Member` carries no argument
    // list at all to count. `arity` is therefore UNUSED below (both probes
    // are hardcoded to `0`); this assertion documents and enforces that
    // caller invariant so a future call site passing a stray non-zero arity
    // for a bare Member is caught in debug builds rather than silently
    // ignored.
    debug_assert_eq!(
        arity, 0,
        "zero_arg_aware_lookup: a bare Member (is_method=false) must carry arity 0 \
         by construction — got {arity}; a non-Member caller must pass is_method=true"
    );
    match (lookup(false, 0), lookup(true, 0)) {
        (Some(prop), Some(method)) => {
            if prop == method {
                Some(prop)
            } else {
                None // conflicting return kinds -> fail closed, never guess
            }
        }
        (Some(prop), None) => Some(prop),
        (None, Some(method)) => Some(method),
        (None, None) => None,
    }
}

/// Cross-object call-result chain (plan v2.1 Task 3): type a `Var.Method()`
/// PREFIX's result by a PURE [`resolve_member`] type-query on `base_ty`,
/// converting the resolved procedure's declared return type to a
/// [`ReceiverType`] exactly like Step 2's declared-variable path
/// ([`parsed_type_to_receiver`]).
///
/// # Route-count guard
///
/// `resolve_member(base_ty, member_lc, arity, ..)` must yield EXACTLY ONE
/// [`crate::program::resolve::edge::Route`]. For `Object`/`Record`/
/// `SelfObject` bases this is `resolve_member`'s own unconditional contract
/// (every arm of its `match` returns a single-element `Vec`). For an
/// `Interface` base it fans out to every implementer — exactly one route
/// means exactly one implementer in the closed-world closure; more than one
/// (a genuinely polymorphic prefix) declines here, never a guessed pick.
///
/// A route whose target carries no routine identity at all
/// (`RouteTarget::Unresolved` — arity mismatch/ambiguous overload/access
/// excluded — or `RouteTarget::Builtin`, a platform-intrinsic method with no
/// modeled return type) also declines: there is nothing to read a
/// `return_type` from.
///
/// # Single-implementer interface prefix
///
/// Once the route-count guard already passed (exactly one implementer),
/// PREFERS reading the return type from the INTERFACE's own declared method
/// signature when the graph models one ([`interface_own_routine_node`]) —
/// AL requires every implementer's signature to match the interface's
/// exactly, so this can never be a looser answer than the implementer's, and
/// sidesteps needing to know the implementer's own tier/ABI-ness. Falls back
/// to the resolved implementer's own routine node
/// ([`routine_node_for_type_query`], which also applies the ABI-PREFIX
/// UNIQUENESS GUARD for an `AbiSymbol` target) when the interface's own
/// signature isn't modeled.
fn infer_cross_object_chain_receiver(
    base_ty: &ReceiverType,
    member_lc: &str,
    arity: usize,
    from_object: &ObjectNode,
    graph: &ProgramGraph,
    index: &ResolveIndex,
    surface: &DeclSurface,
) -> Option<ReceiverType> {
    let (_shape, routes) = resolve_member(
        base_ty,
        member_lc,
        arity,
        from_object,
        graph,
        index,
        surface,
    );
    let [route] = routes.as_slice() else {
        return None;
    };
    if matches!(
        route.target,
        RouteTarget::Unresolved | RouteTarget::Builtin(_)
    ) {
        return None;
    }

    if let ReceiverType::Interface { name_lc } = base_ty
        && let Some(node) =
            interface_own_routine_node(name_lc, member_lc, arity, from_object, graph, index)
    {
        return receiver_from_routine_node(node, from_object, graph, index);
    }

    let node = routine_node_for_type_query(route, arity, from_object, graph, index)?;
    receiver_from_routine_node(node, from_object, graph, index)
}

/// The interface's OWN declared member signature (name+arity match), when
/// modeled — see [`infer_cross_object_chain_receiver`]'s doc. Interface
/// members carry no access modifier in AL (they are always the public
/// contract), so no visibility filtering applies here (unlike
/// `resolve_member`'s implementer dispatch). `None` when the interface
/// object itself is not resolvable from `from_object`'s app, or zero/more-
/// than-one same-arity candidate is declared (defensive — a single interface
/// declaration should never itself be arity-ambiguous, but this never
/// guesses).
fn interface_own_routine_node<'g>(
    name_lc: &str,
    member_lc: &str,
    arity: usize,
    from_object: &ObjectNode,
    graph: &'g ProgramGraph,
    index: &ResolveIndex,
) -> Option<&'g RoutineNode> {
    let iface = graph.resolve_object(from_object.id.app, ObjectKind::Interface, name_lc)?;
    let matched: Vec<&RoutineNodeId> = index
        .routines_in_object(&iface.id, member_lc)
        .iter()
        .filter(|rid| rid.params_count == arity)
        .collect();
    let [rid] = matched.as_slice() else {
        return None;
    };
    graph
        .routines
        .binary_search_by(|probe| probe.id.cmp(rid))
        .ok()
        .map(|i| &graph.routines[i])
}

/// Convert a resolved prefix routine's declared return type into a
/// [`ReceiverType`] — the shared tail of [`infer_cross_object_chain_receiver`]'s
/// two paths (interface's own signature, or the resolved implementer/routine).
///
/// Declines (`None`) on: no declared return type; a scalar/primitive return
/// (`classify_type_text` → `ParsedType::Primitive`); a collapsed ABI-overload
/// survivor (`node.abi_overload_collapsed` — Task 3 review fix, see
/// [`RoutineNode::abi_overload_collapsed`]'s doc: its `return_type` may
/// belong to a DIFFERENT raw declaration than the one actually selected, so
/// it is untrustworthy by construction); or — Task 2's structured
/// cross-validation — an ABI-sourced return type whose `Subtype` `(name, id)`
/// pair disagrees with the object the name resolves to (`node.return_type_id`
/// is `Some` only for an ABI/SymbolOnly-ingested routine whose declared
/// Subtype carried both fields; applies uniformly regardless of which
/// `RouteTarget` shape supplied `node`, per `AbiRoutine::return_type_id`'s
/// doc). Cross-validation only applies when the parsed return type resolved
/// to an `Object`/`Record` (the only shapes carrying a resolved
/// `ObjectNodeId` to check an id against); any other shape (`Interface`,
/// `EnumType`, `Framework`, …) has no identity to cross-check and passes
/// through unconditionally — those shapes carry no risk of a false `Source`
/// edge to a WRONG object.
fn receiver_from_routine_node(
    node: &RoutineNode,
    from_object: &ObjectNode,
    graph: &ProgramGraph,
    index: &ResolveIndex,
) -> Option<ReceiverType> {
    // Task 3 review fix: `routine_node_for_type_query` already applies this
    // same check to the `RouteTarget`-resolved node, but `interface_own_
    // routine_node`'s result reaches this function WITHOUT going through
    // that choke point (interface members carry no access/visibility dance
    // to guard) — check again here so BOTH callers are covered.
    if node.abi_overload_collapsed {
        return None;
    }
    let return_type = node.return_type.as_deref()?;
    let parsed = classify_type_text(return_type);
    if matches!(parsed, ParsedType::Primitive) {
        return None;
    }
    let receiver = parsed_type_to_receiver(parsed, from_object, graph, index);

    if let Some((_name, id)) = &node.return_type_id {
        let resolved_obj = match &receiver {
            ReceiverType::Object { id: Some(oid), .. } => object_by_id(graph, oid),
            ReceiverType::Record {
                table: Some(table_id),
            } => object_by_id(graph, table_id),
            _ => None,
        };
        match resolved_obj {
            Some(obj) if obj.declared_id == Some(*id) => {}
            _ => return None,
        }
    }

    Some(receiver)
}

/// `graph.objects` is kept sorted by `ObjectNodeId` (see `build.rs`'s Step 4)
/// — an O(log n) `binary_search_by` here mirrors the same idiom
/// `graph.routines.binary_search_by(|probe| probe.id.cmp(rid))` already uses
/// throughout `resolver.rs`, replacing an O(n) linear `.find` (Task 3 review
/// finding 2).
pub(crate) fn object_by_id<'g>(
    graph: &'g ProgramGraph,
    oid: &ObjectNodeId,
) -> Option<&'g ObjectNode> {
    graph
        .objects
        .binary_search_by(|probe| probe.id.cmp(oid))
        .ok()
        .map(|i| &graph.objects[i])
}

/// `true` when `expr_id` derefs to a bare `this` identifier (case-insensitive
/// — AL identifiers are case-insensitive), the ONLY shape the `this.<rest>`
/// step (module doc Step 6b) recognizes. A `"this"` `QuotedIdentifier` (i.e.
/// written `"this"` with quotes, which in AL would refer to a DIFFERENTLY
/// -named symbol, not the self-reference keyword) is deliberately EXCLUDED —
/// only the unquoted keyword form is the self-reference.
fn is_this_identifier(file: &AlFile, expr_id: ExprId) -> bool {
    matches!(
        &file.ir.expr(expr_id).kind,
        ExprKind::Identifier(name) if name.eq_ignore_ascii_case("this")
    )
}

/// `this.<rest>` member resolution (module doc Step 6b): resolve `member_lc`
/// against the SELF-ONLY scope AL's `this` keyword actually permits — object
/// GLOBALS only (`object_globals`), never `routine.params`/`routine.locals`.
///
/// Per Microsoft's AL language documentation ("Use the `this` keyword for
/// codeunit self-reference"), `this` is a self-reference allowing a symbol
/// reference to be "a member of the object itself"; the System Application's
/// own adoption note describes it as "referencing methods and globals within
/// the same object". Locals and parameters are NOT members of the object —
/// they belong to the routine's own stack frame — so `this.` cannot address
/// them; a same-named local/param simply does not shadow a global reached via
/// `this.` (that is the entire point of the keyword: disambiguating from a
/// same-named local). This function only ever resolves `member_lc` against
/// `object_globals`, matching that documented scope exactly — never `routine`
/// at all. See `tests/r0-corpus/ws-compound-framework/PROOF.md` for the full
/// citation (no AL compiler was available in this task's execution
/// environment; the semantics above are spec-stated per Microsoft Learn, not
/// `alc`-verified).
///
/// `this.<method>(...)` (a CALL, dispatching a same-object PROCEDURE's return
/// type) is handled by the caller ([`infer_compound_member_receiver`]),
/// which declines before ever reaching here — this function is reached only
/// for the property form.
fn infer_this_member(
    member_lc: &str,
    object_globals: &[VarDecl],
    from_object: &ObjectNode,
    graph: &ProgramGraph,
    index: &ResolveIndex,
) -> ReceiverType {
    let Some(ty) = object_globals
        .iter()
        .find(|v| v.name.fold_identifier() == member_lc)
        .and_then(|v| v.ty.as_deref())
    else {
        return ReceiverType::Unknown;
    };
    parsed_type_to_receiver(classify_type_text(ty), from_object, graph, index)
}

/// Step 5's implementation: type a `Func().Method()` compound receiver by the
/// return type of the bare same-scope function `Func()`.
///
/// `expr_id` must dereference (via `file.ir.expr`) to a structured
/// `ExprKind::Call{function, args}` node — the receiver of the OUTER member
/// call (`.Method()`), i.e. the `Func(...)` sub-expression.  Every other
/// shape reaching here (a `Member` function — the `Obj.Method().X()`
/// cross-object chain — or anything else) declines to `None` (fail-closed;
/// Step 5 is not the shape's home). A `Member`-function shape specifically
/// then falls through to Step 6's cross-object-chain arm (plan v2.1 Task 3),
/// which may resolve it; anything else genuinely falls through to `Unknown`.
///
/// Fail-closed at every step (see the module doc's Step 5 for the full
/// rationale):
/// 1. **Bare-identifier guard** — `function` must be `Identifier`/
///    `QuotedIdentifier`; a dotted/member function chain declines.
/// 2. **Local-shadowing guard** (round-2 gemini critical, checked BEFORE
///    typing) — `resolve_bare` resolves ROUTINE calls and cannot see
///    locals/params/globals, but in AL a same-named variable SHADOWS a
///    same-named procedure. If `function_lc` matches ANY of
///    `routine.params`/`routine.locals`/`object_globals`, decline — this
///    plan does not type variable-backed receivers (e.g. a local ARRAY named
///    `GetCustomer` makes `GetCustomer(1)` an index access, not a call).
/// 3. **`resolve_bare` type query** — call `resolve_bare` with `function_lc`
///    and `args.len()` as the arity; require the SINGLE returned `Route` (its
///    contract: always exactly one) to target `RouteTarget::Routine` — this
///    reuses `resolve_bare`'s own-object/extension-base/implicit-Rec/builtin
///    precedence, its same-arity-overload-ambiguity decline, its
///    builtin/intrinsic PROBE-THEN-DECIDE collision guard, and its
///    `with`-guard, for free. A `Builtin`/`AbiSymbol`/`Unresolved` target
///    (name absent, arity mismatch, ambiguous overload, or an unproven
///    builtin/Rec-shadow precedence collision) declines.
/// 4. **Non-scalar return-type guard** — the resolved routine's
///    `return_type` must be `Some` and parse (via [`classify_type_text`]) to
///    a non-`Primitive` shape; `None` (no declared return type) or a scalar
///    primitive (`Integer`, `Boolean`, …) declines — nothing to dispatch a
///    member call on.
/// 5. **Type conversion** — the parsed return type is resolved to a
///    [`ReceiverType`] via [`parsed_type_to_receiver`], the SAME
///    graph/`ResolveIndex`-backed, fail-closed conversion Step 2's
///    declared-variable path uses: a cross-app-ambiguous `Record`/`Object`
///    return inherits that path's decline-to-`None` (never guess), and an
///    `Interface` return becomes `ReceiverType::Interface` (Phase B fans out
///    to every implementer — polymorphic, never a concrete guess).
#[allow(clippy::too_many_arguments)] // 9 distinct identity/lookup inputs mirror `resolve_in_object`'s precedent; grouping would obscure call sites.
fn infer_call_result_receiver(
    file: &AlFile,
    expr_id: ExprId,
    routine: &RoutineDecl,
    object_globals: &[VarDecl],
    from_object: &ObjectNode,
    graph: &ProgramGraph,
    index: &ResolveIndex,
    surface: &DeclSurface,
    with_state: WithState,
) -> Option<ReceiverType> {
    // 0. Must be a structured Call whose function is a BARE identifier — a
    //    Member function (`Obj.Method()`) is the cross-object-chain shape
    //    Step 6 handles instead (plan v2.1 Task 3) and declines here.
    let ExprKind::Call { function, args } = &file.ir.expr(expr_id).kind else {
        return None;
    };
    let function_lc = match &file.ir.expr(*function).kind {
        ExprKind::Identifier(name) | ExprKind::QuotedIdentifier(name) => name.fold_identifier(),
        _ => return None,
    };

    // 1. Local-shadowing guard FIRST — see the doc above.
    let shadowed = routine
        .params
        .iter()
        .any(|p| p.name.fold_identifier() == function_lc)
        || routine
            .locals
            .iter()
            .any(|v| v.name.fold_identifier() == function_lc)
        || object_globals
            .iter()
            .any(|v| v.name.fold_identifier() == function_lc);
    if shadowed {
        return None;
    }

    // 2. Type-query `function_lc` via `resolve_bare`. Contract: usable ONLY
    //    when it resolved to exactly one `Route` of a `Routine` target — a
    //    genuine same-object overload ambiguity (Task 4,
    //    sigfp-and-ambiguous-reclassification plan: `resolve_bare` can now
    //    return >1 candidate routes for `DispatchShape::AmbiguousOverload`)
    //    has no single unambiguous return type to type-query, so the
    //    `[route]` slice pattern's `else` arm already declines correctly —
    //    no explicit shape check needed.
    let (_shape, routes) = resolve_bare(
        from_object,
        &function_lc,
        args.len(),
        graph,
        index,
        surface,
        with_state,
    );
    let [route] = routes.as_slice() else {
        return None;
    };
    let RouteTarget::Routine(ref rid) = route.target else {
        return None;
    };

    // 3. Non-scalar return-type guard.
    //
    // Task-3 review finding (folded in by Task 4): `graph.routines` is kept
    // sorted by `RoutineNodeId` (the same invariant `resolver.rs`'s
    // `lookup_routine_access`/`make_routine_route` rely on) — an O(n) linear
    // `.find` here was a needless scan when a `binary_search_by` mirrors that
    // existing idiom exactly, for both consistency and scaling.
    let return_type = graph
        .routines
        .binary_search_by(|probe| probe.id.cmp(rid))
        .ok()
        .map(|i| &graph.routines[i])?
        .return_type
        .as_deref()?;
    let parsed = classify_type_text(return_type);
    if matches!(parsed, ParsedType::Primitive) {
        return None;
    }

    // 4. Convert the parsed return type to a receiver — same fail-closed
    //    conversion Step 2's declared-variable path uses.
    Some(parsed_type_to_receiver(parsed, from_object, graph, index))
}

// ---------------------------------------------------------------------------
// Private helpers
// ---------------------------------------------------------------------------

/// Resolve the implicit record type for `Rec`/`xRec` based on the enclosing
/// object's kind. `routine` is consulted ONLY by the Report/ReportExtension
/// arm (dataitem-receivers plan, Task 1) — every other arm is unchanged and
/// routine-independent, exactly as before.
fn infer_implicit_rec(
    routine: &RoutineDecl,
    from_object: &ObjectNode,
    graph: &ProgramGraph,
    index: &ResolveIndex,
) -> ReceiverType {
    match from_object.id.kind {
        // A Table IS its own record.
        ObjectKind::Table => ReceiverType::Record {
            table: Some(from_object.id.clone()),
        },
        // A TableExtension's implicit record is the base table (Caller B).
        // Resolution goes through the same fail-closed
        // `ResolveIndex::resolve_object_ref` as Page's `SourceTable` below —
        // a guessed base table is the cardinal sin (I1), so anything short
        // of a single unambiguous in-closure match stays `Record{table:
        // None}`.
        ObjectKind::TableExtension => ReceiverType::Record {
            table: resolve_tableext_base_table(from_object, graph, index),
        },
        // A Page's implicit Rec is typed by its own `SourceTable` property
        // (Task 5). Resolution goes through the fail-closed
        // `ResolveIndex::resolve_object_ref`: a guessed table is the cardinal
        // sin (a wrong table produces a false `Source` edge), so anything
        // short of a single unambiguous in-closure match stays
        // `Record{table: None}` — builtins (SetRange/FindSet/…) still resolve
        // table-independently in Phase B; only a non-builtin method call on a
        // table-less Record becomes the honest `Unknown`.
        ObjectKind::Page => ReceiverType::Record {
            table: from_object
                .source_table
                .as_ref()
                .and_then(|r| resolve_source_table_ref(from_object.id.clone(), r, graph, index)),
        },
        // A PageExtension may declare its own `SourceTable`; when it does not,
        // its implicit Rec follows the BASE page's `SourceTable` instead — the
        // `extends` target is resolved to exactly one in-closure Page first
        // (same fail-closed rule), then that page's `source_table` is read and
        // resolved the same way. An own `SourceTable` that fails to resolve
        // does NOT fall through to the base page — it explicitly overrides the
        // base, so a failed override stays `None` rather than silently
        // reverting to inherited behavior.
        ObjectKind::PageExtension => {
            let table = if let Some(r) = &from_object.source_table {
                resolve_source_table_ref(from_object.id.clone(), r, graph, index)
            } else {
                resolve_pageext_base_source_table(from_object, graph, index)
            };
            ReceiverType::Record { table }
        }
        // A Codeunit's implicit Rec is typed by its own `TableNo` property
        // (Task 6 — the direct analog of Task 5's Page/`SourceTable` fix).
        // Unlike Page (which ALWAYS has an implicit Rec, typed or not), a
        // Codeunit only gets an implicit Rec when `TableNo` is declared at
        // all — `None` here means there is no implicit-Rec entity to type,
        // so this stays the honest `Unknown` (not `Record{table: None}`).
        // `Subtype = Test`/`TestRunner` codeunits fall into this same `None`
        // arm: they never declare `TableNo` (no statically-typed implicit
        // Rec — unhandled even in the legacy L3 engine), so nothing is
        // fabricated for them; `ObjectNode` does not track `Subtype` at all,
        // deliberately, since the `TableNo`-presence check alone already
        // produces the correct honest decline.
        //
        // When `TableNo` IS declared, resolution goes through the same
        // fail-closed `ResolveIndex::resolve_object_ref` as Page's
        // `SourceTable`, and mirrors its non-`Unique` treatment: a single
        // unambiguous in-closure match yields `Record{table: Some(id)}`;
        // anything else (cross-app ambiguity, out-of-closure, unresolved)
        // stays `Record{table: None}` rather than guessing — a wrong table
        // is a false `Source` edge, the cardinal sin. Builtins
        // (SetRange/FindSet/…) still resolve table-independently in Phase B
        // either way; only a non-builtin method call on a table-less Record
        // becomes the honest `Unknown`.
        ObjectKind::Codeunit => match &from_object.table_no {
            Some(r) => ReceiverType::Record {
                table: resolve_source_table_ref(from_object.id.clone(), r, graph, index),
            },
            None => ReceiverType::Unknown,
        },
        // Report / ReportExtension (dataitem-receivers plan, Task 1): a
        // report's implicit Rec is scoped PER-DATAITEM (each `dataitem(...)`
        // block sources its own table; a report can have several, nested),
        // not a single object-level `SourceTable` the way Page/PageExtension
        // are — so this arm is ROUTINE-CONTEXTUAL ONLY, never an
        // object-level fallback (see `resolve_report_implicit_rec_table`'s
        // doc). REQUESTPAGE ISOLATION holds by construction: a requestpage
        // trigger's `dataitem_source_table` is always `None` and its
        // `in_dataset_modify_context` is always `false` (the lowerer forces
        // dataset-context off descending into `requestpage`), so this arm
        // correctly declines to `Record{table: None}` for it — never
        // guessing the report's outermost/any dataitem's table.
        ObjectKind::Report | ObjectKind::ReportExtension => ReceiverType::Record {
            table: resolve_report_implicit_rec_table(routine, from_object, graph, index),
        },
        // All other object kinds have no implicit Rec.
        _ => ReceiverType::Unknown,
    }
}

/// Resolve a report/report-extension dataitem trigger's implicit-Rec table
/// (dataitem-receivers plan, Task 1) — ROUTINE-CONTEXTUAL ONLY. Two sources,
/// in order:
/// 1. `routine.dataitem_source_table` — set directly by the lowerer when the
///    trigger is nested inside an ACTUAL `dataitem(Name; Table)` block (the
///    common case; `al_syntax::lower::collect_routines`).
/// 2. The resolve-time fallback (Task 1's additive `modify()` lowering fix):
///    when (1) is absent but `routine.in_dataset_modify_context` is `true` (a
///    CONFIRMED report/report-extension `dataset { modify(<Name>) {..} }`
///    block — never a requestpage/layout/field/view `modify()`, per that
///    field's doc) and `routine.enclosing_member` names the modified
///    dataitem, look `<Name>` up via the SAME fail-closed
///    [`resolve_dataitem_source_table`] Step 2b uses (own + extended-base
///    dataitem maps, collision-guarded).
///
/// `enclosing_member`'s name text is already outer-quote-stripped
/// (`al_syntax::lower::ident_text`) — the SAME convention
/// [`node_extract::DataitemNode::name_lc`] storage uses — so a direct
/// lowercase comparison is consistent on both sides without re-unquoting.
fn resolve_report_implicit_rec_table(
    routine: &RoutineDecl,
    from_object: &ObjectNode,
    graph: &ProgramGraph,
    index: &ResolveIndex,
) -> Option<ObjectNodeId> {
    if let Some(table_name) = routine.dataitem_source_table.as_deref() {
        let table_ref = ObjectRef::Name {
            raw: table_name.to_string(),
            normalized_lc: table_name.fold_identifier(),
        };
        return resolve_source_table_ref(from_object.id.clone(), &table_ref, graph, index);
    }
    if routine.in_dataset_modify_context
        && let Some((member_name, _)) = routine.enclosing_member.as_ref()
    {
        let name_lc = member_name.fold_identifier();
        return resolve_dataitem_source_table(&name_lc, from_object, graph, index);
    }
    None
}

/// Resolve the DATAITEM-NAME lookup (Step 2b, dataitem-receivers plan Task 1)
/// and the report implicit-Rec `modify()` fallback (above): a unique
/// (case-insensitive, unquoted) name match among the VISIBLE report
/// dataitems — own `from_object.dataitems`, plus (ReportExtension only) the
/// extended BASE report's own dataitems, resolved via `extends_target` —
/// mirrors the PageExtension `SourceTable` fallback pattern
/// ([`resolve_pageext_base_source_table`]).
///
/// Fail-closed collisions (never guess, per the plan's binding round-1
/// addendum):
/// - a routine (ANY arity/access) of the SAME NAME exists in the report's own
///   routine set, or (ReportExtension) the extended base report's own routine
///   set ([`ResolveIndex::routines_in_object`]) — AL's parens-optional
///   zero-arg call makes `Name.X()` structurally ambiguous between "the
///   dataitem record" and "a parens-less call to a same-named procedure";
///   mirrors Step 3a's `table_scope_has_routine` guard. Over-declining here is
///   always the safe direction.
/// - the name resolves to more than one DISTINCT (own ∪ base) source-table
///   `ObjectRef` — an unprovable duplicate, decline rather than pick one.
///   IDENTICAL duplicates (harmless `#if`/`#else` re-parse duplication —
///   `collect_report_dataitems` walks both branches, mirroring `globals`/
///   `locals`; see `ObjectDecl.report_dataitems`'s doc) are deduped first, so
///   they never manufacture an artificial ambiguity.
fn resolve_dataitem_source_table(
    name_lc: &str,
    from_object: &ObjectNode,
    graph: &ProgramGraph,
    index: &ResolveIndex,
) -> Option<ObjectNodeId> {
    if !matches!(
        from_object.id.kind,
        ObjectKind::Report | ObjectKind::ReportExtension
    ) {
        return None;
    }

    let base_id = if from_object.id.kind == ObjectKind::ReportExtension {
        resolve_reportext_base_report(from_object, graph, index)
    } else {
        None
    };

    // Routine-name collision guard — own object's routines, plus the
    // extended base report's own routines for a ReportExtension (a direct
    // extension may reach the base's visible procedures bare — see
    // `ObjectKind::is_extension_kind`'s doc; over-declining is always safe).
    if !index
        .routines_in_object(&from_object.id, name_lc)
        .is_empty()
    {
        return None;
    }
    if let Some(base_id) = &base_id
        && !index.routines_in_object(base_id, name_lc).is_empty()
    {
        return None;
    }

    let mut matches: Vec<&ObjectRef> = from_object
        .dataitems
        .iter()
        .filter(|d| d.name_lc == name_lc)
        .map(|d| &d.source_table)
        .collect();

    if let Some(base_id) = &base_id
        && let Some(base_obj) = graph.objects.iter().find(|o| o.id == *base_id)
    {
        matches.extend(
            base_obj
                .dataitems
                .iter()
                .filter(|d| d.name_lc == name_lc)
                .map(|d| &d.source_table),
        );
    }

    // Dedupe IDENTICAL source-table refs — see this function's doc.
    let mut distinct: Vec<&ObjectRef> = Vec::new();
    for m in matches {
        if !distinct.contains(&m) {
            distinct.push(m);
        }
    }

    match distinct.as_slice() {
        [only] => resolve_source_table_ref(from_object.id.clone(), only, graph, index),
        _ => None,
    }
}

/// Resolve a ReportExtension's `extends_target` to the base Report's
/// `ObjectNodeId`, scoped from `from_object`'s own dependency closure via the
/// fail-closed [`ResolveIndex::resolve_object_ref`]. `None` when there is no
/// `extends_target`, or resolution is anything other than `Unique`
/// (ambiguous, out-of-closure, unresolved) — never guess. Mirrors
/// [`resolve_pageext_base_page`]'s template, `ObjectKind::Report` instead of
/// `ObjectKind::Page`.
fn resolve_reportext_base_report(
    from_object: &ObjectNode,
    graph: &ProgramGraph,
    index: &ResolveIndex,
) -> Option<ObjectNodeId> {
    let extends = from_object.extends_target.as_deref()?;
    let base_ref = ObjectRef::Name {
        raw: extends.to_string(),
        normalized_lc: extends.fold_identifier(),
    };
    match index.resolve_object_ref(graph, from_object.id.clone(), ObjectKind::Report, &base_ref) {
        ObjectRefResolution::Unique(id) => Some(id),
        ObjectRefResolution::Ambiguous
        | ObjectRefResolution::OutOfClosure
        | ObjectRefResolution::Unresolved => None,
    }
}

/// Resolve an object's `SourceTable` [`ObjectRef`] to a table `ObjectNodeId`,
/// scoped from `from`'s dependency closure via the fail-closed
/// [`ResolveIndex::resolve_object_ref`]. Only [`ObjectRefResolution::Unique`]
/// yields a table; `Ambiguous`/`OutOfClosure`/`Unresolved` all decline to
/// `None` rather than guess.
///
/// `pub(crate)`: also reused directly by `resolver.rs`'s `resolve_bare` Step 3
/// (beyond-1B.3b Task 3) for the Page implicit-Rec table lookup — the exact
/// same fail-closed rule the EXPLICIT `Rec.Foo()` receiver-inference path
/// (this module) already established for Tasks 5-7.
pub(crate) fn resolve_source_table_ref(
    from: ObjectNodeId,
    source_table: &ObjectRef,
    graph: &ProgramGraph,
    index: &ResolveIndex,
) -> Option<ObjectNodeId> {
    match index.resolve_object_ref(graph, from, ObjectKind::Table, source_table) {
        ObjectRefResolution::Unique(id) => Some(id),
        ObjectRefResolution::Ambiguous
        | ObjectRefResolution::OutOfClosure
        | ObjectRefResolution::Unresolved => None,
    }
}

/// Resolve a TableExtension's `extends_target` to the base Table's
/// `ObjectNodeId`, scoped from `from_object`'s own dependency closure via the
/// fail-closed [`ResolveIndex::resolve_object_ref`] (Caller B, I1). `None`
/// when there is no `extends_target`, or resolution is anything other than
/// `Unique` (ambiguous, out-of-closure, unresolved) — never guess. Mirrors
/// [`resolve_pageext_base_page`]'s template, `ObjectKind::Table` instead of
/// `ObjectKind::Page`. `extends_target` is always a NAME in AL grammar (a
/// TableExtension cannot `extends` by numeric id), so this always builds an
/// [`ObjectRef::Name`], unlike `SourceTable`/`TableNo` which may be numeric.
///
/// `pub(crate)`: also reused directly by `resolver.rs`'s `resolve_bare` Step 3
/// (beyond-1B.3b Task 3) for the TableExtension implicit-Rec table lookup —
/// literally "`resolve_object_ref(Table, extends_target)`" as the task brief
/// specifies, via this existing helper rather than re-deriving it.
pub(crate) fn resolve_tableext_base_table(
    from_object: &ObjectNode,
    graph: &ProgramGraph,
    index: &ResolveIndex,
) -> Option<ObjectNodeId> {
    let extends = from_object.extends_target.as_deref()?;
    let base_ref = ObjectRef::Name {
        raw: extends.to_string(),
        normalized_lc: extends.fold_identifier(),
    };
    resolve_source_table_ref(from_object.id.clone(), &base_ref, graph, index)
}

/// Resolve a PageExtension's `extends_target` to the base Page's
/// `ObjectNodeId`, scoped from `from_object`'s own dependency closure via the
/// fail-closed [`ResolveIndex::resolve_object_ref`]. `None` when there is no
/// `extends_target`, or resolution is anything other than `Unique`
/// (ambiguous, out-of-closure, unresolved) — never guess. Shared by
/// [`resolve_pageext_base_source_table`] (Task 5's implicit-`Rec` base-page
/// lookup) and [`find_page_control`] (Task 7's PageExtension control merge).
fn resolve_pageext_base_page(
    from_object: &ObjectNode,
    graph: &ProgramGraph,
    index: &ResolveIndex,
) -> Option<ObjectNodeId> {
    let extends = from_object.extends_target.as_deref()?;
    let base_ref = ObjectRef::Name {
        raw: extends.to_string(),
        normalized_lc: extends.fold_identifier(),
    };
    match index.resolve_object_ref(graph, from_object.id.clone(), ObjectKind::Page, &base_ref) {
        ObjectRefResolution::Unique(id) => Some(id),
        ObjectRefResolution::Ambiguous
        | ObjectRefResolution::OutOfClosure
        | ObjectRefResolution::Unresolved => None,
    }
}

/// Resolve a PageExtension's inherited `SourceTable`: follow `extends_target`
/// to exactly one in-closure base Page, then read and resolve THAT page's own
/// `source_table`. Any decline at either hop (ambiguous extends target,
/// extends target out of closure/unresolved, base page has no `SourceTable`,
/// or the base page's `SourceTable` itself fails to resolve) yields `None`.
///
/// Both hops are scoped from `from_object`'s own closure (the extension's),
/// consistent with every other lookup in this module keying off the CALLING
/// object's app — not the base page's.
///
/// `pub(crate)`: also reused directly by `resolver.rs`'s `resolve_bare` Step 3
/// (beyond-1B.3b Task 3) for the PageExtension implicit-Rec table lookup.
pub(crate) fn resolve_pageext_base_source_table(
    from_object: &ObjectNode,
    graph: &ProgramGraph,
    index: &ResolveIndex,
) -> Option<ObjectNodeId> {
    let base_id = resolve_pageext_base_page(from_object, graph, index)?;
    let base_page = graph.objects.iter().find(|o| o.id == base_id)?;
    resolve_source_table_ref(
        from_object.id.clone(),
        base_page.source_table.as_ref()?,
        graph,
        index,
    )
}

/// Find a `CurrPage.<part>` layout control by lowercased name, in the set
/// visible to `from_object`: its own `page_controls` first; for a
/// `PageExtension` with no matching control of its own, also the extended
/// BASE page's controls (merged — mirrors L3's `symbol_table::
/// page_controls_for`), resolved via the fail-closed
/// [`resolve_pageext_base_page`] rather than a raw name lookup. An own
/// PageExtension control of the same name always shadows the base page's
/// (checked first, short-circuits before the base-page hop).
///
/// Returns an owned clone — `PageControlNode` is small (`Vec`-backed) and
/// this sidesteps unifying the lifetime of a borrow from `from_object` with
/// one from `graph.objects` in a single return type.
fn find_page_control(
    name_lc: &str,
    from_object: &ObjectNode,
    graph: &ProgramGraph,
    index: &ResolveIndex,
) -> Option<PageControlNode> {
    if let Some(c) = from_object
        .page_controls
        .iter()
        .find(|c| c.name_lc == name_lc)
    {
        return Some(c.clone());
    }
    if from_object.id.kind != ObjectKind::PageExtension {
        return None;
    }
    let base_id = resolve_pageext_base_page(from_object, graph, index)?;
    let base_page = graph.objects.iter().find(|o| o.id == base_id)?;
    base_page
        .page_controls
        .iter()
        .find(|c| c.name_lc == name_lc)
        .cloned()
}

/// Parse the text following `"currpage."` (already lowercased by the caller)
/// for the `<part>.page` subpage-instance shape (Task 7): a single, possibly
/// quoted, control-name segment followed by EXACTLY one trailing `.page`
/// accessor and nothing else. Returns the control name, quotes stripped
/// (already lowercase since the input is).
///
/// Returns `None` — decline, honest `Unknown` — for: a bare part with no
/// `.page` accessor (`CurrPage.Lines` — the CONTROL, distinct from the
/// subpage INSTANCE); a chain deeper than one `.page` accessor
/// (`CurrPage.Lines.Page.Foo`); or any other shape.
fn parse_currpage_dot_page_segment(rest: &str) -> Option<String> {
    let (segment, remainder) = if let Some(after_quote) = rest.strip_prefix('"') {
        // Quoted control name: the segment runs to the next `"`. An escaped
        // `""` literal-quote inside the name is not handled here (matching
        // this module's existing `unquote_identifier`, which doesn't either)
        // — such a name simply fails the `page_controls` lookup and declines.
        let close = after_quote.find('"')?;
        (&after_quote[..close], &after_quote[close + 1..])
    } else {
        match rest.find('.') {
            Some(i) => (&rest[..i], &rest[i..]),
            None => (rest, ""),
        }
    };
    if segment.is_empty() || remainder != ".page" {
        return None;
    }
    Some(segment.to_string())
}

/// Parse the text following `"currpage."` (already lowercased by the caller)
/// for a BARE, single, possibly-quoted control-name segment with NOTHING
/// trailing it at all (Task 1's Step 0b — the `UserControl` sibling of
/// [`parse_currpage_dot_page_segment`], which requires a trailing `.page`
/// instead of nothing). Returns the control name, quotes stripped (already
/// lowercase since the input is).
///
/// Returns `None` — decline, honest `Unknown`, never fabricated — for: an
/// empty segment, or ANY trailing remainder at all (a `.page` accessor, a
/// deeper chain, anything) — those shapes are Step 0's / a generic decline's
/// territory, never this one's.
fn parse_currpage_bare_control_segment(rest: &str) -> Option<String> {
    let (segment, remainder) = if let Some(after_quote) = rest.strip_prefix('"') {
        let close = after_quote.find('"')?;
        (&after_quote[..close], &after_quote[close + 1..])
    } else {
        match rest.find('.') {
            Some(i) => (&rest[..i], &rest[i..]),
            None => (rest, ""),
        }
    };
    if segment.is_empty() || !remainder.is_empty() {
        return None;
    }
    Some(segment.to_string())
}

/// Convert a [`ParsedType`] (pure string parse) to a [`ReceiverType`] by
/// resolving names against the graph.
///
/// `from_object` (rather than a bare `AppRef`) is required so the `Record`
/// arm can drive [`ResolveIndex::resolve_object_ref`] (needs the full
/// `ObjectNodeId`, not just the app) — the fail-closed, shape-preserving
/// resolution Caller A needs (I1).
///
/// `pub(crate)`: T3 (pageext-merge-and-final-residual plan) reuses this
/// EXACT conversion in `arg_dispatch::type_one_arg`'s new `Call{function:
/// Member{..}}` arm to type a `Var.Method()` call-RESULT argument's base —
/// the SAME "declared type text -> ReceiverType" step Step 2's declared-
/// variable receiver typing and Step 6's cross-object-chain base typing both
/// already use, just needed one module over.
pub(crate) fn parsed_type_to_receiver(
    pt: ParsedType,
    from_object: &ObjectNode,
    graph: &ProgramGraph,
    index: &ResolveIndex,
) -> ReceiverType {
    match pt {
        ParsedType::Record { table_ref } => {
            // Reuses the same fail-closed, shape-preserving helper Task 5's
            // Page `SourceTable` resolution uses: `resolve_object_ref`'s
            // `Id`/`Name` arms already dispatch on `table_ref`'s shape
            // (`ObjectRef::Id`/`Name` — losslessly carried from
            // `classify_type_text`), so `Record 18` and `Record "18"` can
            // never be conflated, and >1 in-closure dependency match DECLINES
            // to `None` rather than guessing (I1).
            let table = resolve_source_table_ref(from_object.id.clone(), &table_ref, graph, index);
            ReceiverType::Record { table }
        }
        ParsedType::Object { kind, object_ref } => {
            // Task 2 (mirrors I1): the SAME fail-closed, shape-preserving
            // `resolve_object_ref` the `Record` arm above uses — `object_ref`
            // is losslessly shaped (`ObjectRef::Id`/`Name`) by
            // `parse_object_kind_type`, so `Codeunit 80` and `Codeunit "80"`
            // can never be conflated here either. A `Unique` resolution
            // carries the resolved `id` UP FRONT, so `resolve_member`'s
            // `Object` arm short-circuits on it directly (mirrors Task 7's
            // `CurrPage.<part>.Page` carried-id short-circuit) instead of
            // re-deriving it from `name_lc` — no redundant second lookup for
            // the (common) resolved case.
            let (id, name_lc) =
                resolve_object_ref_lc(kind, &object_ref, from_object.id.clone(), graph, index);
            ReceiverType::Object { kind, name_lc, id }
        }
        ParsedType::Interface { name } => ReceiverType::Interface { name_lc: name },
        ParsedType::EnumType { name } => ReceiverType::EnumType { name_lc: name },
        ParsedType::ControlAddIn { name } => {
            // Direct-var retrofit (Task 1 round-1 addendum): `var X: ControlAddIn
            // "Foo"` gets the SAME closed-if-known gate as the `CurrPage.<usercontrol>`
            // Step-0b arm — no separate open-accept path survives for this shape.
            let target = ObjectRef::Name {
                raw: name.clone(),
                normalized_lc: name,
            };
            resolve_control_addin_receiver(&target, from_object, graph, index)
        }
        ParsedType::RecordRef => ReceiverType::RecordRef,
        ParsedType::FieldRef => ReceiverType::FieldRef,
        ParsedType::KeyRef => ReceiverType::KeyRef,
        ParsedType::Framework(kind) => ReceiverType::Framework(kind),
        ParsedType::Primitive => ReceiverType::Primitive,
        ParsedType::Dynamic => ReceiverType::Dynamic,
    }
}

/// Small, CLOSED allowlist of Microsoft-shipped platform `ControlAddIn`
/// CLASSES whose JS-side method surface this engine treats as an
/// unconditional `builtin` Catalog invocation when NO source/symbol
/// declaration for the name is reachable from the caller (Task 1's
/// `ControlAddInSurface::TruePlatform` outcome).
///
/// Currently just `webpageviewer`. Grounded empirically against the real CDO
/// corpus (`grep -rn "usercontrol(" **/*.al`): every one of its 7
/// `CurrPage.<control>.SetContent(...)` sites declares
/// `usercontrol(WebPageViewer; WebPageViewer)` / `usercontrol(WebViewer;
/// WebPageViewer)` — the BARE, unqualified identifier `WebPageViewer`, not
/// the fully-qualified name Microsoft's System Application actually ships
/// the object under
/// (`"Microsoft.Dynamics.Nav.Client.WebPageViewer"` — verified directly from
/// that dependency `.app`'s `SymbolReference.json`, `ControlAddIns[].Name`;
/// see also
/// <https://learn.microsoft.com/en-us/dynamics365/business-central/application/system-application/controladdin/microsoft.dynamics.nav.client.webpageviewer>).
/// Since this engine's `ObjectIndex`/`ResolveIndex` key ABI-ingested objects
/// by their SymbolReference `Name` verbatim, the bare `WebPageViewer`
/// reference genuinely has ZERO reachable candidate from this engine's
/// point of view — `Unresolved`, not `Unique` — even though the real AL
/// compiler accepts it. `"microsoft.dynamics.nav.client.webpageviewer"` is
/// ALSO listed, defensively, for the (currently unobserved in CDO) case
/// where a caller writes the fully-qualified quoted name AND that
/// dependency's SymbolReference somehow still isn't ingested/reachable —
/// harmless to list twice, since a NAME THAT DOES resolve
/// (`ObjectRefResolution::Unique`) always takes the `Declared` gate first,
/// never this fallback.
const TRUE_PLATFORM_CONTROL_ADDINS: &[&str] = &[
    "webpageviewer",
    "microsoft.dynamics.nav.client.webpageviewer",
];

/// Resolve a `ControlAddIn` receiver's tri-state surface (Task 1, round-2
/// closer, BINDING): the SAME gate for both the `CurrPage.<usercontrol>` Step-0b
/// arm and the direct-var `ControlAddIn "Foo"` retrofit.
///
/// - **Resolved, uniquely, cleanly** (`ObjectRefResolution::Unique`, and the
///   resolved object's owning file parsed cleanly — [`ObjectNode::parse_incomplete`]
///   `false`) → [`ReceiverType::ControlAddIn`] with
///   [`ControlAddInSurface::Declared`], carrying every declared procedure's
///   `(name_lc, arity)` (events already structurally excluded — see
///   [`ControlAddInSurface::Declared`]'s doc). Phase B (`resolve_member`) does
///   the actual member+arity gate.
/// - **Resolved but Degraded** (`Unique`, `parse_incomplete: true`) → declines
///   to `Unknown` UNCONDITIONALLY, even if the called member happens to
///   textually match something in the (untrustworthy) routine list — a
///   parse-recovered file's extracted routines could be spurious CST
///   artifacts, so a "match" here is not provably a real declared procedure
///   either. Never guess in either direction.
/// - **Ambiguous** (`ObjectRefResolution::Ambiguous`, ≥2 reachable
///   declarations) → `Unknown`. **OutOfClosure** (declared somewhere in the
///   snapshot, but not in `from_object`'s dependency closure) → also
///   `Unknown` — a real declared type we simply cannot verify the surface of
///   from here; closed-if-known means never falling back to open-accept just
///   because verification is unavailable.
/// - **Unresolved** (no candidate anywhere) → open-accept
///   ([`ControlAddInSurface::TruePlatform`]) ONLY when the name is on the
///   [`TRUE_PLATFORM_CONTROL_ADDINS`] allowlist; otherwise `Unknown` (a
///   genuinely-unknown control name — could be a typo, a dependency that
///   isn't indexed, anything; never guessed open).
fn resolve_control_addin_receiver(
    target: &ObjectRef,
    from_object: &ObjectNode,
    graph: &ProgramGraph,
    index: &ResolveIndex,
) -> ReceiverType {
    let name_lc = match target {
        ObjectRef::Name { normalized_lc, .. } => normalized_lc.clone(),
        ObjectRef::Id(n) => n.to_string(),
    };
    match index.resolve_object_ref(
        graph,
        from_object.id.clone(),
        ObjectKind::ControlAddIn,
        target,
    ) {
        ObjectRefResolution::Unique(oid) => {
            let Some(obj) = graph.objects.iter().find(|o| o.id == oid) else {
                // Defensive — `resolve_object_ref` only ever returns an id it read
                // out of `graph.objects` itself; structurally unreachable.
                return ReceiverType::Unknown;
            };
            if obj.parse_incomplete {
                return ReceiverType::Unknown;
            }
            let procedures: Vec<(String, usize)> = graph
                .routines
                .iter()
                .filter(|r| r.id.object == oid)
                .map(|r| (r.id.name_lc.clone(), r.id.params_count))
                .collect();
            ReceiverType::ControlAddIn {
                name_lc,
                surface: ControlAddInSurface::Declared { procedures },
            }
        }
        ObjectRefResolution::Ambiguous | ObjectRefResolution::OutOfClosure => ReceiverType::Unknown,
        ObjectRefResolution::Unresolved => {
            if TRUE_PLATFORM_CONTROL_ADDINS.contains(&name_lc.as_str()) {
                ReceiverType::ControlAddIn {
                    name_lc,
                    surface: ControlAddInSurface::TruePlatform,
                }
            } else {
                ReceiverType::Unknown
            }
        }
    }
}

/// Resolve a losslessly-shaped [`ObjectRef`] (Task 2, mirrors I1) to a target
/// `ObjectNodeId` and its canonical lowercased name, via the same fail-closed,
/// dependency-closure-scoped [`ResolveIndex::resolve_object_ref`] the `Record`
/// arm's `SourceTable`/`TableNo` resolution (Tasks 5/6) already uses.
///
/// A numeric AL object id (`Codeunit 80`) is never conflated with a codeunit
/// literally NAMED `"80"` (`Codeunit "80"`) the way the old
/// `name.trim().parse::<i64>()` re-parse of an ALREADY-unquoted string used
/// to (both collapsed to numeric id 80) — `object_ref`'s `Id`/`Name` shape is
/// dispatched directly, with no string re-parsing at all.
///
/// Only [`ObjectRefResolution::Unique`] returns a resolved id; `Ambiguous`/
/// `OutOfClosure`/`Unresolved` all decline to `None` — never guess (the
/// cardinal sin) — falling back to [`object_ref_fallback_lc`] for `name_lc`
/// so `resolve_member`'s `Object` arm can still attempt its own by-name
/// lookup for the (rare, dormant — digit-named AL objects are ~never seen in
/// real BC) unresolved case.
fn resolve_object_ref_lc(
    kind: ObjectKind,
    object_ref: &ObjectRef,
    from: ObjectNodeId,
    graph: &ProgramGraph,
    index: &ResolveIndex,
) -> (Option<ObjectNodeId>, String) {
    match index.resolve_object_ref(graph, from, kind, object_ref) {
        ObjectRefResolution::Unique(id) => {
            let name_lc = graph
                .objects
                .iter()
                .find(|o| o.id == id)
                .map(|o| o.name.fold_identifier())
                .unwrap_or_else(|| object_ref_fallback_lc(object_ref));
            (Some(id), name_lc)
        }
        ObjectRefResolution::Ambiguous
        | ObjectRefResolution::OutOfClosure
        | ObjectRefResolution::Unresolved => (None, object_ref_fallback_lc(object_ref)),
    }
}

/// The lowercased display text of an [`ObjectRef`], used only as the
/// `name_lc` fallback when [`resolve_object_ref_lc`]'s shape-aware resolution
/// did not find a unique target — a numeric id renders as its decimal text
/// (matching legacy `resolve_object_name_lc` fallback behavior), never
/// re-derived by parsing a string as `i64`.
fn object_ref_fallback_lc(object_ref: &ObjectRef) -> String {
    match object_ref {
        ObjectRef::Name { normalized_lc, .. } => normalized_lc.clone(),
        ObjectRef::Id(n) => n.to_string(),
    }
}

/// Build a [`ParsedType::Object`] for the given kind and raw name portion,
/// classifying quoted-vs-bare EXACTLY as `classify_type_text`'s `Record` arm
/// does (Task 2, mirrors I1): a bare numeric string is [`ObjectRef::Id`];
/// ANYTHING else — including a QUOTED numeric string, since the quote
/// characters make it fail the `i64` parse before unquoting — is
/// [`ObjectRef::Name`]. This decides shape BEFORE any unquoting happens, so
/// `Codeunit 80` (numeric id) and `Codeunit "80"` (a codeunit literally named
/// `"80"`) can never be conflated by a later re-parse of an already-unquoted
/// string.
fn parse_object_kind_type(kind: ObjectKind, name_rest: &str) -> ParsedType {
    let trimmed = name_rest.trim();
    let object_ref = if let Ok(n) = trimmed.parse::<i64>() {
        ObjectRef::Id(n)
    } else {
        let raw = unquote_identifier(trimmed);
        let normalized_lc = raw.fold_identifier();
        ObjectRef::Name { raw, normalized_lc }
    };
    ParsedType::Object { kind, object_ref }
}

/// Whether `s` (a lowercased receiver-text token, quote characters preserved
/// exactly as written) is an ATOMIC AL identifier — a single bare/quoted name
/// — as opposed to a COMPOUND receiver chain (`A.B`, `A.B()`, `A."B.C"`, …).
/// Centralized (dataitem-receivers plan, Task 1; round-1 review addendum) —
/// the single predicate shared by every atomic-vs-compound receiver guard:
/// [`infer_receiver_type`]'s Step 2 (`lookup_lc`), Step 3a (bare quoted-field
/// receiver), Step 4 (framework-name guard), and `full.rs`'s
/// `CompoundReceiver` relabeling.
///
/// Replaces the naive `!s.contains('.')` check those call sites used before
/// this task: a QUOTED identifier may legally contain an EMBEDDED period
/// (`"Sales Cr.Memo Header Filter"` — 5/16 of a real CDO report's dataitem
/// names), and the naive check mislabeled it compound, since the interior dot
/// sits inside quotes and is therefore NOT a segment separator at all.
///
/// Two atomic shapes:
/// - **Unquoted**: no `.` and no `(` anywhere (unchanged from before). The
///   `(` exclusion is a CALL-SHAPE guard here — an unquoted `foo(1)` is a
///   call, never a bare identifier.
/// - **Quoted**: the ENTIRE string is a single quoted token — `len() > 2`
///   (excludes the degenerate empty-quote `""`), starts AND ends with `"`,
///   those are the ONLY two `"` characters in the string (excludes an
///   escaped-quote AL identifier — `""` doubling to embed a literal quote,
///   e.g. `"a""b"` — from this fast path; an unusual doubled-quote-escaped
///   name fails closed to COMPOUND here, never silently mishandled).
///   Judged PURELY on quote-parity — an interior `(` inside a well-formed
///   quoted span is just a character of the identifier, never a call-shape
///   signal (a quoted span can never itself be a call target, so there is
///   nothing for a paren guard to protect against there). Real BC field
///   names routinely contain parens — `"View (Blob)"`, `"Request Page
///   (XML)"` — and MUST classify atomic (Task 1 review-fix: the prior
///   version applied the unquoted branch's `(` exclusion BEFORE the
///   quote-parity check, so any well-formed quoted token containing a paren
///   wrongly fell to COMPOUND — an 8-site CDO regression, since fixed).
///   `"A.B".C` and `"A.B"."C.D"` both have an UNQUOTED `.` after/between the
///   quoted span(s) — a real segment separator — and so correctly stay
///   COMPOUND (caught by the quoted branch's `ends_with('"')`/exactly-2-quotes
///   check, since `.C`/`."C.D"` trails past the closing quote).
///
/// Unsupported/malformed forms (unequal quote counts, a lone `"`, …) fail
/// closed to COMPOUND — never guessed atomic.
pub(crate) fn is_atomic_receiver_token(s: &str) -> bool {
    if s.starts_with('"') {
        // Any quoted-shaped token — with or without an interior dot OR an
        // interior paren — must be well-formed to count as atomic: closes
        // with exactly the matching pair of quotes (no stray/escaped quote
        // chars) and is non-degenerate (`len() > 2` excludes the empty
        // quoted identifier `""`). A malformed/unsupported quoted shape
        // fails closed to COMPOUND here, consistent with every other
        // decline in this module. Deliberately NO `(` exclusion in this
        // branch — see the doc comment above.
        return s.len() > 2 && s.ends_with('"') && s.matches('"').count() == 2;
    }
    // Unquoted branch: `(` is a call-shape guard (`foo(1)` is never a bare
    // identifier); `.` is a segment separator.
    !s.contains('(') && !s.contains('.')
}

/// Strip surrounding double-quotes from an identifier token.  Returns the
/// token unchanged if not quoted; returns an empty string for an empty input.
///
/// Port of al-sem `unquoteName`.
pub(crate) fn unquote_identifier(s: &str) -> String {
    let trimmed = s.trim();
    if trimmed.len() >= 2 && trimmed.starts_with('"') && trimmed.ends_with('"') {
        trimmed[1..trimmed.len() - 1].to_string()
    } else {
        trimmed.to_string()
    }
}

/// Outcome of [`caller_scope_symbol`] — the SAME caller-scope-EXACT lookup
/// tier order shared by `receiver.rs`'s Step 2 and `arg_dispatch.rs`'s
/// `type_one_arg` (T3, receiver-closure-and-arg-increments plan; module doc's
/// "one shared helper... must not drift" mandate).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CallerScopeSymbol<'a> {
    /// No param / local / named-return binding / global matches this name.
    NotFound,
    /// A matching symbol was found at the first tier that matches (param,
    /// then local, then the named-return binding, then global) — its
    /// declared type text, or `None` if the symbol itself has no type text
    /// (a malformed/incomplete declaration). Found-with-no-type STILL
    /// shadows: it does not fall through to a lower tier (mirrors
    /// `arg_dispatch.rs`'s pre-existing, more-correct semantics — a
    /// shadowing declaration with unknown type must degrade to untyped, not
    /// silently resolve via a DIFFERENT, lower-precedence symbol of the same
    /// name).
    Found(Option<&'a str>),
    /// A same-tier duplicate this function cannot safely resolve — either:
    /// (a) SAME-SCOPE BINDING clash (T3 round-2 closer): `name` matches the
    /// routine's named-return binding AND ALSO a param or local of the
    /// IDENTICAL name in the SAME routine. This can never legally happen in
    /// valid AL — a named return value can't coexist with a param/local of
    /// the same name (compile error) — so if malformed source somehow
    /// reaches here anyway, the caller MUST decline outright rather than
    /// pick a winner. Never fires for a same-name GLOBAL — shadowing a
    /// global is ordinary, VALID AL precedence (the binding wins; see
    /// [`CallerScopeSymbol::Found`]). Or: (b) a genuinely CONFLICTING
    /// param-vs-param / local-vs-local duplicate (T4-C medium (e)) — a
    /// `#if`/`#else` union-read (see `al_syntax::lower`'s doc: preproc
    /// branches are NOT evaluated, so both arms' declarations survive) that
    /// declared the SAME name with DIFFERENT type text in each branch.
    /// IDENTICAL duplicates (same name AND type, harmless re-parse
    /// duplication) are deduped first and never reach this variant — mirrors
    /// `ResolveIndex::field_in_table`'s and `resolve_dataitem_source_table`'s
    /// established "dedupe identical, decline on genuine conflict" pattern,
    /// closing the ONE caller-scope site that previously used raw
    /// first-match-wins `Vec::iter().find()` with no duplicate awareness at
    /// all.
    MalformedDuplicate,
}

/// Dedupe `hits` (every param/local matching `name`, in declaration order) by
/// `ty` — an identical-type duplicate (harmless `#if`/`#else` re-parse
/// duplication) collapses to one; a genuinely different type is a real,
/// unprovable conflict. Mirrors `ResolveIndex::field_in_table`'s provenance
/// dedup. Returns `Ok(None)` for no hits, `Ok(Some(ty))` for exactly one
/// distinct type, `Err(())` for more than one distinct type (decline).
fn dedupe_type_hits<'a>(hits: &[Option<&'a str>]) -> Result<Option<Option<&'a str>>, ()> {
    let mut distinct: Vec<Option<&'a str>> = Vec::new();
    for &ty in hits {
        if !distinct.contains(&ty) {
            distinct.push(ty);
        }
    }
    match distinct.as_slice() {
        [] => Ok(None),
        [ty] => Ok(Some(*ty)),
        _ => Err(()),
    }
}

/// Caller-scope-EXACT variable-symbol lookup: **param → local → named-return
/// binding → global**, case-insensitive (ASCII). This is the ONE place both
/// `receiver.rs`'s Step 2 (bare-identifier receiver typing) and
/// `arg_dispatch.rs`'s `type_one_arg` (caller-scope-exact arg typing) resolve
/// a bare identifier against the calling routine's own scope — sharing it
/// means the two lookups structurally cannot drift (T3, receiver-closure-and-
/// arg-increments plan).
///
/// # The proven precedence (T3 report has the full compiler-fixture citation)
///
/// AL's bare-identifier precedence inside a routine is, in order: (1)
/// params/locals — mutually exclusive with EACH OTHER and with a named-return
/// binding of the same name (any collision is a compile error, i.e. malformed
/// source if it ever reaches this function); (2) the routine's own
/// named-return-value binding, if declared (`procedure X() Ret: Type` —
/// `RoutineDecl.return_name`/`return_type`) — scoped to THIS routine only,
/// carrying the parsed return-type TEXT VERBATIM so it dispatches exactly
/// like an explicit local; (3) object globals. A named-return binding
/// SHADOWS a global of the same name (ordinary AL scoping — the binding is
/// effectively a routine-local symbol), which is why it is checked here
/// BEFORE globals, not after.
///
/// Later, WIDER precedence layers this function does NOT itself decide
/// (documented here so the full order is legible in one place, per the T3
/// closer's "prove the WHOLE order before inserting the arm" mandate): (4)
/// for a CALL/parens-optional-call shape, a same-named ROUTINE anywhere in
/// the visibility-scoped table surface (`ResolveIndex::table_scope_has_
/// routine`) — checked by callers of THIS function, e.g. Step 3a's routine-
/// shadow guard, never here (this function has no routine-catalog access);
/// (5) implicit-self TABLE FIELDS (`ResolveIndex::field_in_table`) — LAST
/// among value symbols, reached only once (1)-(4) all miss. `receiver.rs`'s
/// Step 3a is exactly step (5), and it already runs strictly after Step 2
/// (which calls this function) — so the ordering this function embodies for
/// (1)-(3), combined with Step 3a's existing position + its own routine-
/// shadow check for (4), together realize the full proven order without
/// requiring any single function to encode all five tiers itself.
pub(crate) fn caller_scope_symbol<'a>(
    name: &str,
    routine: &'a RoutineDecl,
    object_globals: &'a [VarDecl],
) -> CallerScopeSymbol<'a> {
    let param_types: Vec<Option<&'a str>> = routine
        .params
        .iter()
        .filter(|p| p.name.eq_fold_identifier(name))
        .map(|p| p.ty.as_deref())
        .collect();
    let local_types: Vec<Option<&'a str>> = routine
        .locals
        .iter()
        .filter(|v| v.name.eq_fold_identifier(name))
        .map(|v| v.ty.as_deref())
        .collect();
    let global_types: Vec<Option<&'a str>> = object_globals
        .iter()
        .filter(|v| v.name.eq_fold_identifier(name))
        .map(|v| v.ty.as_deref())
        .collect();
    let (Ok(param_hit), Ok(local_hit), Ok(global_hit)) = (
        dedupe_type_hits(&param_types),
        dedupe_type_hits(&local_types),
        dedupe_type_hits(&global_types),
    ) else {
        // A genuinely conflicting same-tier duplicate (different type text
        // under the same name) — unprovable, decline outright rather than
        // pick a winner (see `CallerScopeSymbol::MalformedDuplicate`'s doc,
        // case (b)).
        return CallerScopeSymbol::MalformedDuplicate;
    };
    let return_hit = routine
        .return_name
        .as_deref()
        .is_some_and(|rn| rn.eq_fold_identifier(name));

    if return_hit && (param_hit.is_some() || local_hit.is_some()) {
        return CallerScopeSymbol::MalformedDuplicate;
    }
    if let Some(ty) = param_hit {
        return CallerScopeSymbol::Found(ty);
    }
    if let Some(ty) = local_hit {
        return CallerScopeSymbol::Found(ty);
    }
    if return_hit {
        return CallerScopeSymbol::Found(routine.return_type.as_deref());
    }
    if let Some(ty) = global_hit {
        return CallerScopeSymbol::Found(ty);
    }
    CallerScopeSymbol::NotFound
}

/// Strip a trailing `\s+temporary\s*$` modifier (case-insensitive) from a
/// Record type's name portion. A verbatim duplicate of L3's
/// `record_types::strip_trailing_temporary` — NOT imported from there
/// because `src/program/resolve` is enforced L3-independent (see
/// `tests/program_resolve_harness.rs`'s
/// `resolve_module_has_no_stray_engine_l3_l2_imports` guard; the module's own
/// doc names `builtins.rs::global_builtins` as the ONE sanctioned exception).
/// Both copies shared the same char-boundary panic/mis-parse bug (T2.4); if
/// this logic changes again, update `engine::l3::record_types`'s copy too.
///
/// Char-boundary safe by construction: never computes a byte offset from a
/// RE-CASED copy of the string and slices the ORIGINAL with it — that old
/// approach panics whenever a character's `to_lowercase()` byte length
/// differs from its own (e.g. `ẞ`, U+1E9E, 3 bytes → `ß`, 2 bytes) and, even
/// where it doesn't panic, silently misjudges the whitespace boundary for
/// characters that GROW under lowering (e.g. Turkish `İ`, U+0130, 2 bytes →
/// `i̇`, 3 bytes), leaving a real `İ Temporary` table name un-stripped.
/// Instead this walks `char_indices()` on the original string directly and
/// ASCII-folds each char against the literal ASCII word "temporary" (a
/// faithful port of the TS `/i` flag, itself a simple ASCII fold for a
/// pure-ASCII pattern) — every byte offset used to slice comes straight from
/// `trimmed_end`, so it is always a valid char boundary.
fn strip_trailing_temporary(s: &str) -> String {
    const WORD: &str = "temporary";
    let trimmed_end = s.trim_end();
    let indices: Vec<(usize, char)> = trimmed_end.char_indices().collect();
    let word_len = WORD.chars().count();
    if indices.len() <= word_len {
        // No room for a preceding whitespace char — `\s+temporary` needs one.
        return trimmed_end.to_string();
    }
    let tail_start = indices.len() - word_len;
    let tail_matches = indices[tail_start..]
        .iter()
        .zip(WORD.chars())
        .all(|(&(_, c), w)| c.eq_ignore_ascii_case(&w));
    if !tail_matches || !indices[tail_start - 1].1.is_whitespace() {
        return trimmed_end.to_string();
    }
    let word_byte_start = indices[tail_start].0;
    trimmed_end[..word_byte_start].to_string()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use al_syntax::ir::{ObjectKind, Origin, Param, Point, RoutineDecl, RoutineKind, VarDecl};

    use crate::program::graph::{ObjectIndex, ProgramGraph};
    use crate::program::node::{AppRef, ObjKey, ObjectNodeId, RoutineNodeId};
    use crate::program::node_extract::AbiParams;
    use crate::program::node_extract::{Access, DataitemNode, ObjectNode};
    use crate::program::resolve::index::ResolveIndex;
    use crate::program::topology::DependencyGraph;
    use crate::snapshot::{AppId, TrustTier};

    // -----------------------------------------------------------------------
    // Test helpers
    // -----------------------------------------------------------------------

    fn test_origin() -> Origin {
        Origin {
            kind_text: "",
            byte: 0..0,
            start: Point { row: 0, column: 0 },
            end: Point { row: 0, column: 0 },
        }
    }

    /// Build a minimal `ProgramGraph` with one app containing:
    /// - Table "Customer" (declared_id = 18)
    /// - Codeunit "MyCodeunit" (declared_id = 50100)
    /// - A Table with no declared_id, named "SalesHeader" (for extension tests)
    fn build_test_graph() -> (ProgramGraph, AppRef) {
        let mut apps = crate::program::node::AppRegistry::default();
        let app_id = AppId {
            guid: String::new(),
            name: "TestApp".into(),
            publisher: "Test".into(),
            version: "1.0.0.0".into(),
        };
        let app = apps.intern(&app_id);

        let topology = DependencyGraph::default();

        let make_obj =
            |app: AppRef, kind: ObjectKind, name: &str, declared_id: Option<i64>| ObjectNode {
                id: ObjectNodeId {
                    app,
                    kind,
                    key: match declared_id {
                        Some(n) => ObjKey::Id(n),
                        None => ObjKey::Name(name.to_ascii_lowercase()),
                    },
                },
                name: name.to_string(),
                declared_id,
                extends_target: None,
                implements: vec![],
                tier: TrustTier::Workspace,
                source_table: None,
                table_no: None,
                source_table_temporary: false,
                page_controls: vec![],
                fields: vec![],
                dataitems: vec![],
                parse_incomplete: false,
            };

        let mut objects = vec![
            make_obj(app, ObjectKind::Table, "Customer", Some(18)),
            make_obj(app, ObjectKind::Codeunit, "MyCodeunit", Some(50100)),
            make_obj(app, ObjectKind::Table, "SalesHeader", None),
        ];
        objects.sort_by(|a, b| a.id.cmp(&b.id));

        let obj_index = ObjectIndex::build(&objects);

        let graph = ProgramGraph {
            apps,
            topology,
            objects,
            routines: vec![],
            obj_index,
            ..Default::default()
        };
        (graph, app)
    }

    /// Build a `RoutineDecl` with:
    /// - param `CuParam: Codeunit "MyCodeunit"`
    /// - param `CuNumParam: Codeunit 50100` (numeric id reference)
    /// - local `Cust: Record Customer`
    /// - local `J: JsonObject`
    /// - local `RecTmp: Record Customer temporary`
    fn build_test_routine() -> RoutineDecl {
        let o = test_origin();
        RoutineDecl {
            kind: RoutineKind::Procedure,
            name: "TestProc".into(),
            name_origin: o.clone(),
            params: vec![
                Param {
                    name: "CuParam".into(),
                    by_ref: false,
                    ty: Some("Codeunit \"MyCodeunit\"".into()),
                    origin: o.clone(),
                },
                Param {
                    name: "CuNumParam".into(),
                    by_ref: false,
                    ty: Some("Codeunit 50100".into()),
                    origin: o.clone(),
                },
            ],
            return_type: None,
            return_name: None,
            locals: vec![
                VarDecl {
                    name: "Cust".into(),
                    ty: Some("Record Customer".into()),
                    temporary: false,
                    origin: o.clone(),
                },
                VarDecl {
                    name: "J".into(),
                    ty: Some("JsonObject".into()),
                    temporary: false,
                    origin: o.clone(),
                },
                VarDecl {
                    name: "RecTmp".into(),
                    ty: Some("Record Customer temporary".into()),
                    temporary: true,
                    origin: o.clone(),
                },
                VarDecl {
                    name: "Iface".into(),
                    ty: Some("Interface \"IMyInterface\"".into()),
                    temporary: false,
                    origin: o.clone(),
                },
                VarDecl {
                    name: "EnumVar".into(),
                    ty: Some("Enum \"Color\"".into()),
                    temporary: false,
                    origin: o.clone(),
                },
            ],
            attributes: vec![],
            attributes_parsed: vec![],
            access_modifier: None,
            parse_incomplete: false,
            dataitem_source_table: None,
            enclosing_member: None,
            in_dataset_modify_context: false,
            body: None,
            origin: o,
        }
    }

    /// Build a `ObjectNode` of the given kind for the test app.
    fn make_object_node(
        app: AppRef,
        kind: ObjectKind,
        name: &str,
        declared_id: Option<i64>,
        extends_target: Option<String>,
    ) -> ObjectNode {
        ObjectNode {
            id: ObjectNodeId {
                app,
                kind,
                key: match declared_id {
                    Some(n) => ObjKey::Id(n),
                    None => ObjKey::Name(name.to_ascii_lowercase()),
                },
            },
            name: name.to_string(),
            declared_id,
            extends_target,
            implements: vec![],
            tier: TrustTier::Workspace,
            source_table: None,
            table_no: None,
            source_table_temporary: false,
            page_controls: vec![],
            fields: vec![],
            dataitems: vec![],
            parse_incomplete: false,
        }
    }

    // -----------------------------------------------------------------------
    // classify_type_text tests
    // -----------------------------------------------------------------------

    #[test]
    fn classify_record_quoted() {
        assert_eq!(
            classify_type_text("Record \"Customer\""),
            ParsedType::Record {
                table_ref: ObjectRef::Name {
                    raw: "Customer".into(),
                    normalized_lc: "customer".into()
                }
            }
        );
    }

    #[test]
    fn classify_record_unquoted() {
        assert_eq!(
            classify_type_text("Record Customer"),
            ParsedType::Record {
                table_ref: ObjectRef::Name {
                    raw: "Customer".into(),
                    normalized_lc: "customer".into()
                }
            }
        );
    }

    #[test]
    fn classify_record_temporary() {
        assert_eq!(
            classify_type_text("Record Customer temporary"),
            ParsedType::Record {
                table_ref: ObjectRef::Name {
                    raw: "Customer".into(),
                    normalized_lc: "customer".into()
                }
            }
        );
    }

    #[test]
    fn classify_record_quoted_temporary() {
        assert_eq!(
            classify_type_text("Record \"Customer\" temporary"),
            ParsedType::Record {
                table_ref: ObjectRef::Name {
                    raw: "Customer".into(),
                    normalized_lc: "customer".into()
                }
            }
        );
    }

    // -- T2.4 (3): strip_trailing_temporary char-boundary safety, via THIS
    // call site (the receiver.rs copy was a verbatim duplicate of L3's, now
    // deduplicated — regression-proving it through `classify_type_text`
    // rather than calling the shared helper directly). --

    #[test]
    fn classify_record_unicode_prefix_no_space_does_not_panic() {
        // "ẞ" (3 UTF-8 bytes) directly against "Temporary" (no separating
        // whitespace) used to panic mid-char on the old byte-length-mismatch
        // slice. `\s+temporary` shouldn't strip here regardless — the point is
        // reaching that decision must not panic.
        //
        // `normalized_lc` is `ßtemporary` (not `ẞtemporary`) since the
        // Unicode-fold arc: `ẞ` (U+1E9E, LATIN CAPITAL LETTER SHARP S) has a
        // genuine 1:1 simple lowercase mapping to `ß` (U+00DF) — `fold_identifier`
        // folds it like any other cased letter, unlike the old ASCII-only fold,
        // which left every non-ASCII byte (including `ẞ`) untouched.
        assert_eq!(
            classify_type_text("Record ẞTemporary"),
            ParsedType::Record {
                table_ref: ObjectRef::Name {
                    raw: "ẞTemporary".into(),
                    normalized_lc: "ßtemporary".into()
                }
            }
        );
    }

    #[test]
    fn classify_record_turkish_i_temporary_strips_correctly() {
        // Turkish "İ" GROWS under `to_lowercase()` (2 bytes → 3) — the old
        // byte-math bug silently failed to recognize " temporary" as the
        // modifier here, leaving the table name mis-parsed as
        // "İ temporary" instead of "İ".
        //
        // `normalized_lc` is `i` (not `İ`): `fold_identifier`'s simple 1:1 fold
        // takes `char::to_lowercase('İ').next()` (see crates/al-syntax/src/casing.rs's
        // doc) — plain `i`, deliberately never the 2-char `i̇` `to_lowercase()`
        // would produce. The RAW name (`table_ref.raw`) stays `İ`, unfolded — only
        // the fold key changes.
        assert_eq!(
            classify_type_text("Record İ temporary"),
            ParsedType::Record {
                table_ref: ObjectRef::Name {
                    raw: "İ".into(),
                    normalized_lc: "i".into()
                }
            }
        );
    }

    // -- I1 Caller-A shape-preservation: numeric id vs quoted numeric name --

    #[test]
    fn classify_record_numeric_id() {
        // `Record 18` (unquoted digits) is a NUMERIC id reference.
        assert_eq!(
            classify_type_text("Record 18"),
            ParsedType::Record {
                table_ref: ObjectRef::Id(18)
            }
        );
    }

    #[test]
    fn classify_record_quoted_numeric_name() {
        // `Record "18"` is a table literally NAMED "18" — must NOT be
        // confused with the numeric id reference `Record 18` (I1 shape bug:
        // both used to collapse to the same string "18" once quotes were
        // stripped, silently coercing a quoted name into a guessed id).
        assert_eq!(
            classify_type_text("Record \"18\""),
            ParsedType::Record {
                table_ref: ObjectRef::Name {
                    raw: "18".into(),
                    normalized_lc: "18".into()
                }
            }
        );
    }

    #[test]
    fn classify_record_numeric_id_temporary() {
        assert_eq!(
            classify_type_text("Record 18 temporary"),
            ParsedType::Record {
                table_ref: ObjectRef::Id(18)
            }
        );
    }

    #[test]
    fn classify_codeunit_numeric() {
        // `Codeunit 80` (unquoted digits) is a NUMERIC id reference.
        assert_eq!(
            classify_type_text("Codeunit 80"),
            ParsedType::Object {
                kind: ObjectKind::Codeunit,
                object_ref: ObjectRef::Id(80)
            }
        );
    }

    #[test]
    fn classify_codeunit_named() {
        assert_eq!(
            classify_type_text("Codeunit \"Sales-Post\""),
            ParsedType::Object {
                kind: ObjectKind::Codeunit,
                object_ref: ObjectRef::Name {
                    raw: "Sales-Post".into(),
                    normalized_lc: "sales-post".into()
                }
            }
        );
    }

    #[test]
    fn classify_codeunit_quoted_numeric_name() {
        // `Codeunit "80"` is a codeunit literally NAMED "80" — must NOT be
        // confused with the numeric id reference `Codeunit 80` (Task 2, the
        // I1 shape bug mirrored: both used to collapse to the same string
        // "80" once quotes were stripped, silently coercing a quoted name
        // into a guessed id).
        assert_eq!(
            classify_type_text("Codeunit \"80\""),
            ParsedType::Object {
                kind: ObjectKind::Codeunit,
                object_ref: ObjectRef::Name {
                    raw: "80".into(),
                    normalized_lc: "80".into()
                }
            }
        );
    }

    #[test]
    fn classify_page_numeric_and_quoted_numeric_name() {
        assert_eq!(
            classify_type_text("Page 80"),
            ParsedType::Object {
                kind: ObjectKind::Page,
                object_ref: ObjectRef::Id(80)
            }
        );
        assert_eq!(
            classify_type_text("Page \"80\""),
            ParsedType::Object {
                kind: ObjectKind::Page,
                object_ref: ObjectRef::Name {
                    raw: "80".into(),
                    normalized_lc: "80".into()
                }
            }
        );
    }

    #[test]
    fn classify_report_numeric_and_quoted_numeric_name() {
        assert_eq!(
            classify_type_text("Report 80"),
            ParsedType::Object {
                kind: ObjectKind::Report,
                object_ref: ObjectRef::Id(80)
            }
        );
        assert_eq!(
            classify_type_text("Report \"80\""),
            ParsedType::Object {
                kind: ObjectKind::Report,
                object_ref: ObjectRef::Name {
                    raw: "80".into(),
                    normalized_lc: "80".into()
                }
            }
        );
    }

    #[test]
    fn classify_query_numeric_and_quoted_numeric_name() {
        assert_eq!(
            classify_type_text("Query 80"),
            ParsedType::Object {
                kind: ObjectKind::Query,
                object_ref: ObjectRef::Id(80)
            }
        );
        assert_eq!(
            classify_type_text("Query \"80\""),
            ParsedType::Object {
                kind: ObjectKind::Query,
                object_ref: ObjectRef::Name {
                    raw: "80".into(),
                    normalized_lc: "80".into()
                }
            }
        );
    }

    #[test]
    fn classify_xmlport_numeric_and_quoted_numeric_name() {
        assert_eq!(
            classify_type_text("XmlPort 80"),
            ParsedType::Object {
                kind: ObjectKind::XmlPort,
                object_ref: ObjectRef::Id(80)
            }
        );
        assert_eq!(
            classify_type_text("XmlPort \"80\""),
            ParsedType::Object {
                kind: ObjectKind::XmlPort,
                object_ref: ObjectRef::Name {
                    raw: "80".into(),
                    normalized_lc: "80".into()
                }
            }
        );
    }

    #[test]
    fn classify_json_object() {
        assert_eq!(
            classify_type_text("JsonObject"),
            ParsedType::Framework(FrameworkKind::JsonObject)
        );
    }

    #[test]
    fn classify_integer_is_primitive() {
        assert_eq!(classify_type_text("Integer"), ParsedType::Primitive);
    }

    #[test]
    fn classify_variant_is_dynamic() {
        assert_eq!(classify_type_text("Variant"), ParsedType::Dynamic);
    }

    #[test]
    fn classify_interface() {
        assert_eq!(
            classify_type_text("Interface \"IFoo\""),
            ParsedType::Interface {
                name: "ifoo".into()
            }
        );
    }

    #[test]
    fn classify_enum() {
        assert_eq!(
            classify_type_text("Enum \"Color\""),
            ParsedType::EnumType {
                name: "color".into()
            }
        );
    }

    #[test]
    fn classify_text_with_length() {
        assert_eq!(
            classify_type_text("Text[200]"),
            ParsedType::Framework(FrameworkKind::Text)
        );
    }

    #[test]
    fn classify_code_with_length() {
        assert_eq!(
            classify_type_text("Code[20]"),
            ParsedType::Framework(FrameworkKind::Text)
        );
    }

    #[test]
    fn classify_recordref() {
        assert_eq!(classify_type_text("RecordRef"), ParsedType::RecordRef);
    }

    #[test]
    fn classify_fieldref() {
        assert_eq!(classify_type_text("FieldRef"), ParsedType::FieldRef);
    }

    #[test]
    fn classify_keyref() {
        assert_eq!(classify_type_text("KeyRef"), ParsedType::KeyRef);
    }

    #[test]
    fn classify_http_client() {
        assert_eq!(
            classify_type_text("HttpClient"),
            ParsedType::Framework(FrameworkKind::HttpClient)
        );
    }

    #[test]
    fn classify_xml_document() {
        assert_eq!(
            classify_type_text("XmlDocument"),
            ParsedType::Framework(FrameworkKind::Xml)
        );
    }

    #[test]
    fn classify_list() {
        assert_eq!(
            classify_type_text("List of [Integer]"),
            ParsedType::Framework(FrameworkKind::List)
        );
    }

    // Fix 2 — FileUpload / NumberSequence / Version
    #[test]
    fn classify_fileupload() {
        assert_eq!(
            classify_type_text("FileUpload"),
            ParsedType::Framework(FrameworkKind::FileUpload)
        );
    }

    #[test]
    fn classify_numbersequence() {
        assert_eq!(
            classify_type_text("NumberSequence"),
            ParsedType::Framework(FrameworkKind::NumberSequence)
        );
    }

    #[test]
    fn classify_version() {
        assert_eq!(
            classify_type_text("Version"),
            ParsedType::Framework(FrameworkKind::Version)
        );
    }

    // -----------------------------------------------------------------------
    // infer_receiver_type tests
    // -----------------------------------------------------------------------

    #[test]
    fn infer_local_record_resolves_table_id() {
        let (graph, app) = build_test_graph();
        let index = ResolveIndex::build(&graph);
        let routine = build_test_routine();
        let from_obj = make_object_node(app, ObjectKind::Codeunit, "CallerCu", Some(999), None);

        // "cust" → local `Cust: Record Customer` → table Customer resolved
        let customer_node = graph
            .resolve_object(app, ObjectKind::Table, "Customer")
            .unwrap();
        let expected_id = customer_node.id.clone();

        let result =
            infer_receiver_type("cust", &routine, &[], &from_obj, &graph, &index, None, None);
        assert_eq!(
            result,
            ReceiverType::Record {
                table: Some(expected_id)
            }
        );
    }

    #[test]
    fn infer_local_record_temporary_resolves_table_id() {
        let (graph, app) = build_test_graph();
        let index = ResolveIndex::build(&graph);
        let routine = build_test_routine();
        let from_obj = make_object_node(app, ObjectKind::Codeunit, "CallerCu", Some(999), None);

        let customer_node = graph
            .resolve_object(app, ObjectKind::Table, "Customer")
            .unwrap();
        let expected_id = customer_node.id.clone();

        // "rectmp" → local `RecTmp: Record Customer temporary` → same resolution
        let result = infer_receiver_type(
            "rectmp",
            &routine,
            &[],
            &from_obj,
            &graph,
            &index,
            None,
            None,
        );
        assert_eq!(
            result,
            ReceiverType::Record {
                table: Some(expected_id)
            }
        );
    }

    #[test]
    fn infer_local_json_object() {
        let (graph, app) = build_test_graph();
        let index = ResolveIndex::build(&graph);
        let routine = build_test_routine();
        let from_obj = make_object_node(app, ObjectKind::Codeunit, "CallerCu", Some(999), None);

        let result = infer_receiver_type("j", &routine, &[], &from_obj, &graph, &index, None, None);
        assert_eq!(result, ReceiverType::Framework(FrameworkKind::JsonObject));
    }

    // -----------------------------------------------------------------------
    // I1 Caller-A shape-preservation: `Record 18` (numeric id) vs
    // `Record "18"` (a table literally NAMED "18") must resolve to two
    // DIFFERENT tables, never conflated by a lossy string round-trip.
    // -----------------------------------------------------------------------

    /// Single-app fixture: Table id=18 "Customer" AND a separate table
    /// literally NAMED "18" (`declared_id: None` — its only identity is the
    /// digit-string name). Proves the two are distinguishable.
    fn build_numeric_name_shape_fixture() -> (ProgramGraph, AppRef) {
        let mut apps = crate::program::node::AppRegistry::default();
        let app_id = AppId {
            guid: String::new(),
            name: "ShapeApp".into(),
            publisher: "Test".into(),
            version: "1.0.0.0".into(),
        };
        let app = apps.intern(&app_id);
        let topology = DependencyGraph::default();

        let mut objects = vec![
            make_object_node(app, ObjectKind::Table, "Customer", Some(18), None),
            make_object_node(app, ObjectKind::Table, "18", None, None),
        ];
        objects.sort_by(|a, b| a.id.cmp(&b.id));

        let obj_index = ObjectIndex::build(&objects);
        let graph = ProgramGraph {
            apps,
            topology,
            objects,
            routines: vec![],
            obj_index,
            ..Default::default()
        };
        (graph, app)
    }

    /// Routine with `ById: Record 18` (numeric) and `ByQuotedName: Record
    /// "18"` (quoted digit-string name) params.
    fn build_numeric_name_shape_routine() -> RoutineDecl {
        let o = test_origin();
        RoutineDecl {
            kind: RoutineKind::Procedure,
            name: "TestProc".into(),
            name_origin: o.clone(),
            params: vec![
                Param {
                    name: "ById".into(),
                    by_ref: false,
                    ty: Some("Record 18".into()),
                    origin: o.clone(),
                },
                Param {
                    name: "ByQuotedName".into(),
                    by_ref: false,
                    ty: Some("Record \"18\"".into()),
                    origin: o.clone(),
                },
            ],
            return_type: None,
            return_name: None,
            locals: vec![],
            attributes: vec![],
            attributes_parsed: vec![],
            access_modifier: None,
            parse_incomplete: false,
            dataitem_source_table: None,
            enclosing_member: None,
            in_dataset_modify_context: false,
            body: None,
            origin: o,
        }
    }

    #[test]
    fn caller_a_record_numeric_id_resolves_by_id_not_name() {
        let (graph, app) = build_numeric_name_shape_fixture();
        let index = ResolveIndex::build(&graph);
        let routine = build_numeric_name_shape_routine();
        let from_obj = make_object_node(app, ObjectKind::Codeunit, "CallerCu", Some(999), None);

        let customer_id = graph
            .resolve_object(app, ObjectKind::Table, "Customer")
            .unwrap()
            .id
            .clone();

        // "byid" -> `ById: Record 18` (numeric) -> table id 18 ("Customer"),
        // NEVER the table literally named "18".
        let result =
            infer_receiver_type("byid", &routine, &[], &from_obj, &graph, &index, None, None);
        assert_eq!(
            result,
            ReceiverType::Record {
                table: Some(customer_id)
            }
        );
    }

    #[test]
    fn caller_a_record_quoted_numeric_name_resolves_by_name_not_id() {
        let (graph, app) = build_numeric_name_shape_fixture();
        let index = ResolveIndex::build(&graph);
        let routine = build_numeric_name_shape_routine();
        let from_obj = make_object_node(app, ObjectKind::Codeunit, "CallerCu", Some(999), None);

        let named_18_id = graph
            .resolve_object(app, ObjectKind::Table, "18")
            .unwrap()
            .id
            .clone();
        let customer_id = graph
            .resolve_object(app, ObjectKind::Table, "Customer")
            .unwrap()
            .id
            .clone();
        assert_ne!(
            named_18_id, customer_id,
            "fixture sanity: the two tables must be distinct"
        );

        // "byquotedname" -> `ByQuotedName: Record "18"` (quoted name) -> the
        // table literally NAMED "18", NEVER coerced into table id 18
        // ("Customer") — the I1 shape bug this test locks in the fix for.
        let result = infer_receiver_type(
            "byquotedname",
            &routine,
            &[],
            &from_obj,
            &graph,
            &index,
            None,
            None,
        );
        assert_eq!(
            result,
            ReceiverType::Record {
                table: Some(named_18_id)
            }
        );
    }

    // -----------------------------------------------------------------------
    // Task 2 (mirrors I1): `<Kind> 80` (numeric id) vs `<Kind> "80"` (an
    // object literally NAMED "80") must resolve to two DIFFERENT objects,
    // never conflated by a lossy string round-trip — the `ParsedType::Object`
    // sibling of the `ParsedType::Record` fix directly above. Covers every
    // kind `resolve_object_ref_lc`/`resolve_member`'s `Object` arm serves.
    // -----------------------------------------------------------------------

    /// Single-app fixture, parametrized by `kind`: an object DECLARED with
    /// id 80 ("RealById") AND a separate object of the SAME kind literally
    /// NAMED "80" (`declared_id: None` — its only identity is the
    /// digit-string name), plus a `CallerCu` Codeunit (id 999) to serve as
    /// `from_object`. Mirrors `build_numeric_name_shape_fixture` above,
    /// generalized across object kinds.
    fn build_object_numeric_name_shape_fixture(kind: ObjectKind) -> (ProgramGraph, AppRef) {
        let mut apps = crate::program::node::AppRegistry::default();
        let app_id = AppId {
            guid: String::new(),
            name: "ObjShapeApp".into(),
            publisher: "Test".into(),
            version: "1.0.0.0".into(),
        };
        let app = apps.intern(&app_id);
        let topology = DependencyGraph::default();

        let mut objects = vec![
            make_object_node(app, kind, "RealById", Some(80), None),
            make_object_node(app, kind, "80", None, None),
            make_object_node(app, ObjectKind::Codeunit, "CallerCu", Some(999), None),
        ];
        objects.sort_by(|a, b| a.id.cmp(&b.id));

        let obj_index = ObjectIndex::build(&objects);
        let graph = ProgramGraph {
            apps,
            topology,
            objects,
            routines: vec![],
            obj_index,
            ..Default::default()
        };
        (graph, app)
    }

    /// Routine with `ById: <keyword> 80` (numeric) and `ByQuotedName:
    /// <keyword> "80"` (quoted digit-string name) params, for the given AL
    /// object-kind keyword (`"Codeunit"`/`"Page"`/`"Report"`/`"Query"`/
    /// `"XmlPort"`).
    fn build_object_numeric_name_shape_routine(keyword: &str) -> RoutineDecl {
        let o = test_origin();
        RoutineDecl {
            kind: RoutineKind::Procedure,
            name: "TestProc".into(),
            name_origin: o.clone(),
            params: vec![
                Param {
                    name: "ById".into(),
                    by_ref: false,
                    ty: Some(format!("{keyword} 80")),
                    origin: o.clone(),
                },
                Param {
                    name: "ByQuotedName".into(),
                    by_ref: false,
                    ty: Some(format!("{keyword} \"80\"")),
                    origin: o.clone(),
                },
            ],
            return_type: None,
            return_name: None,
            locals: vec![],
            attributes: vec![],
            attributes_parsed: vec![],
            access_modifier: None,
            parse_incomplete: false,
            dataitem_source_table: None,
            enclosing_member: None,
            in_dataset_modify_context: false,
            body: None,
            origin: o,
        }
    }

    /// Shared assertion body for the per-kind Task 2 shape-preservation
    /// tests below: `<keyword> 80` must resolve to the numeric-id-80 object
    /// (`id: Some`, carried up front — Task 2's other half of the mirror);
    /// `<keyword> "80"` must resolve to the DIFFERENT object literally named
    /// "80", never the id-80 object — the exact pre-fix collapse bug.
    fn assert_object_kind_shape_preserved(kind: ObjectKind, keyword: &str) {
        let (graph, app) = build_object_numeric_name_shape_fixture(kind);
        let index = ResolveIndex::build(&graph);
        let routine = build_object_numeric_name_shape_routine(keyword);
        let from_obj = graph
            .objects
            .iter()
            .find(|o| o.name == "CallerCu")
            .unwrap()
            .clone();

        let by_id_id = graph
            .resolve_object(app, kind, "RealById")
            .unwrap()
            .id
            .clone();
        let by_name_id = graph.resolve_object(app, kind, "80").unwrap().id.clone();
        assert_ne!(
            by_id_id, by_name_id,
            "fixture sanity: the two {keyword} objects must be distinct"
        );

        let by_id =
            infer_receiver_type("byid", &routine, &[], &from_obj, &graph, &index, None, None);
        assert_eq!(
            by_id,
            ReceiverType::Object {
                kind,
                name_lc: "realbyid".into(),
                id: Some(by_id_id),
            },
            "{keyword} 80 (numeric) must resolve to the id-80 object"
        );

        let by_quoted_name = infer_receiver_type(
            "byquotedname",
            &routine,
            &[],
            &from_obj,
            &graph,
            &index,
            None,
            None,
        );
        assert_eq!(
            by_quoted_name,
            ReceiverType::Object {
                kind,
                name_lc: "80".into(),
                id: Some(by_name_id),
            },
            "{keyword} \"80\" (quoted name) must resolve to the object literally \
             named \"80\", never the numeric id-80 object (pre-fix collapse bug)"
        );
    }

    #[test]
    fn caller_a_mirror_object_codeunit_numeric_vs_quoted_name_shape_preserved() {
        assert_object_kind_shape_preserved(ObjectKind::Codeunit, "Codeunit");
    }

    #[test]
    fn caller_a_mirror_object_page_numeric_vs_quoted_name_shape_preserved() {
        assert_object_kind_shape_preserved(ObjectKind::Page, "Page");
    }

    #[test]
    fn caller_a_mirror_object_report_numeric_vs_quoted_name_shape_preserved() {
        assert_object_kind_shape_preserved(ObjectKind::Report, "Report");
    }

    #[test]
    fn caller_a_mirror_object_query_numeric_vs_quoted_name_shape_preserved() {
        assert_object_kind_shape_preserved(ObjectKind::Query, "Query");
    }

    #[test]
    fn caller_a_mirror_object_xmlport_numeric_vs_quoted_name_shape_preserved() {
        assert_object_kind_shape_preserved(ObjectKind::XmlPort, "XmlPort");
    }

    #[test]
    fn infer_param_codeunit_by_name() {
        let (graph, app) = build_test_graph();
        let index = ResolveIndex::build(&graph);
        let routine = build_test_routine();
        let from_obj = make_object_node(app, ObjectKind::Codeunit, "CallerCu", Some(999), None);
        let mycodeunit_id = graph
            .resolve_object(app, ObjectKind::Codeunit, "MyCodeunit")
            .unwrap()
            .id
            .clone();

        // "cuparam" → param `CuParam: Codeunit "MyCodeunit"` → Object{Codeunit,
        // "mycodeunit"}, `id` carried up front (Task 2: mirrors I1's `Record`
        // — a `Unique` `resolve_object_ref` match is resolved in Phase A, not
        // re-derived by a redundant Phase B by-name lookup).
        let result = infer_receiver_type(
            "cuparam",
            &routine,
            &[],
            &from_obj,
            &graph,
            &index,
            None,
            None,
        );
        assert_eq!(
            result,
            ReceiverType::Object {
                kind: ObjectKind::Codeunit,
                name_lc: "mycodeunit".into(),
                id: Some(mycodeunit_id)
            }
        );
    }

    #[test]
    fn infer_param_codeunit_by_number() {
        let (graph, app) = build_test_graph();
        let index = ResolveIndex::build(&graph);
        let routine = build_test_routine();
        let from_obj = make_object_node(app, ObjectKind::Codeunit, "CallerCu", Some(999), None);
        let mycodeunit_id = graph
            .resolve_object(app, ObjectKind::Codeunit, "MyCodeunit")
            .unwrap()
            .id
            .clone();

        // "cunumparam" → param `CuNumParam: Codeunit 50100` → resolves to
        // "mycodeunit", `id` carried up front (Task 2, mirrors I1).
        let result = infer_receiver_type(
            "cunumparam",
            &routine,
            &[],
            &from_obj,
            &graph,
            &index,
            None,
            None,
        );
        assert_eq!(
            result,
            ReceiverType::Object {
                kind: ObjectKind::Codeunit,
                name_lc: "mycodeunit".into(),
                id: Some(mycodeunit_id)
            }
        );
    }

    #[test]
    fn infer_singleton_currpage() {
        let (graph, app) = build_test_graph();
        let index = ResolveIndex::build(&graph);
        let routine = build_test_routine();
        let from_obj = make_object_node(app, ObjectKind::Page, "MyPage", Some(50200), None);

        let result = infer_receiver_type(
            "currpage",
            &routine,
            &[],
            &from_obj,
            &graph,
            &index,
            None,
            None,
        );
        assert_eq!(result, ReceiverType::Framework(FrameworkKind::PageInstance));
    }

    #[test]
    fn infer_singleton_page() {
        let (graph, app) = build_test_graph();
        let index = ResolveIndex::build(&graph);
        let routine = build_test_routine();
        let from_obj = make_object_node(app, ObjectKind::Codeunit, "CallerCu", Some(999), None);

        // bare "page" singleton
        let result =
            infer_receiver_type("page", &routine, &[], &from_obj, &graph, &index, None, None);
        assert_eq!(result, ReceiverType::Framework(FrameworkKind::PageInstance));
    }

    #[test]
    fn infer_singleton_currreport() {
        let (graph, app) = build_test_graph();
        let index = ResolveIndex::build(&graph);
        let routine = build_test_routine();
        let from_obj = make_object_node(app, ObjectKind::Report, "MyReport", Some(50300), None);

        let result = infer_receiver_type(
            "currreport",
            &routine,
            &[],
            &from_obj,
            &graph,
            &index,
            None,
            None,
        );
        assert_eq!(
            result,
            ReceiverType::Framework(FrameworkKind::ReportInstance)
        );
    }

    #[test]
    fn infer_singleton_session() {
        let (graph, app) = build_test_graph();
        let index = ResolveIndex::build(&graph);
        let routine = build_test_routine();
        let from_obj = make_object_node(app, ObjectKind::Codeunit, "CallerCu", Some(999), None);

        let result = infer_receiver_type(
            "session",
            &routine,
            &[],
            &from_obj,
            &graph,
            &index,
            None,
            None,
        );
        assert_eq!(result, ReceiverType::Framework(FrameworkKind::Session));
    }

    #[test]
    fn infer_singleton_database() {
        let (graph, app) = build_test_graph();
        let index = ResolveIndex::build(&graph);
        let routine = build_test_routine();
        let from_obj = make_object_node(app, ObjectKind::Codeunit, "CallerCu", Some(999), None);

        let result = infer_receiver_type(
            "database",
            &routine,
            &[],
            &from_obj,
            &graph,
            &index,
            None,
            None,
        );
        assert_eq!(result, ReceiverType::Framework(FrameworkKind::Database));
    }

    #[test]
    fn infer_this_is_self_object() {
        let (graph, app) = build_test_graph();
        let index = ResolveIndex::build(&graph);
        let routine = build_test_routine();
        let from_obj = make_object_node(app, ObjectKind::Codeunit, "CallerCu", Some(999), None);

        let result =
            infer_receiver_type("this", &routine, &[], &from_obj, &graph, &index, None, None);
        assert_eq!(result, ReceiverType::SelfObject);
    }

    // -----------------------------------------------------------------------
    // T1.5 (deep-review remediation plan, H-4) — a declared variable/field
    // must shadow a same-named platform singleton (Step 3c now runs strictly
    // after Step 2/2b/3a/3b's higher-precedence declared-symbol lookups; see
    // Step 3c's doc comment for the `al.exe` compiler-probe transcript this
    // ordering is grounded in). Before this fix, the singleton match ran as
    // Step 1, BEFORE Step 2's variable lookup, so a declared `Session`/
    // `NavApp`/etc. var was silently discarded in favor of the platform
    // singleton — a false `builtin` edge (or a false `Unknown` for a
    // `Record Session` virtual-table declaration).
    // -----------------------------------------------------------------------

    /// POSITIVE (fixture 1, the bug report's exact shape): `var Session:
    /// Codeunit "MyCodeunit"` must shadow the platform `Session` singleton —
    /// the receiver types as the declared Codeunit, never
    /// `Framework(FrameworkKind::Session)`.
    #[test]
    fn t1_5_declared_session_var_shadows_singleton() {
        let (graph, app) = build_test_graph();
        let index = ResolveIndex::build(&graph);
        let routine = build_test_routine();
        let from_obj = make_object_node(app, ObjectKind::Codeunit, "CallerCu", Some(999), None);
        let mycodeunit_id = graph
            .resolve_object(app, ObjectKind::Codeunit, "MyCodeunit")
            .unwrap()
            .id
            .clone();
        let globals = vec![var_decl("Session", "Codeunit \"MyCodeunit\"")];

        let result = infer_receiver_type(
            "session", &routine, &globals, &from_obj, &graph, &index, None, None,
        );
        assert_eq!(
            result,
            ReceiverType::Object {
                kind: ObjectKind::Codeunit,
                name_lc: "mycodeunit".into(),
                id: Some(mycodeunit_id)
            },
            "a declared `Session` var must shadow the platform Session singleton"
        );
    }

    /// POSITIVE (fixture 2, virtual-table shape): `var Session: Record
    /// Session` — the declared var still shadows the singleton and types as
    /// `Record{None}` (no "Session" Table object exists in the test graph,
    /// mirroring `infer_record_unresolvable_table_is_record_none`'s
    /// established pattern for any other unresolvable declared Record type)
    /// rather than `Framework(FrameworkKind::Session)`.
    #[test]
    fn t1_5_declared_session_record_var_shadows_singleton() {
        let (graph, app) = build_test_graph();
        let index = ResolveIndex::build(&graph);
        let routine = build_test_routine();
        let from_obj = make_object_node(app, ObjectKind::Codeunit, "CallerCu", Some(999), None);
        let globals = vec![var_decl("Session", "Record Session")];

        let result = infer_receiver_type(
            "session", &routine, &globals, &from_obj, &graph, &index, None, None,
        );
        assert_eq!(
            result,
            ReceiverType::Record { table: None },
            "a declared `Session: Record Session` var must shadow the platform \
             singleton and type as Record (unresolved table id, same as any \
             other undeclared-in-graph Record type)"
        );
    }

    /// NEGATIVE / regression guard (fixture 3): with NO declared var/param/
    /// global named `Session` anywhere in scope, the bare receiver must
    /// still resolve to the platform Session singleton — Step 3c is reached
    /// on the Step 2 miss and must still fire. The fix must not break
    /// genuine singleton usage.
    #[test]
    fn t1_5_undeclared_session_still_resolves_singleton() {
        let (graph, app) = build_test_graph();
        let index = ResolveIndex::build(&graph);
        let routine = build_test_routine();
        let from_obj = make_object_node(app, ObjectKind::Codeunit, "CallerCu", Some(999), None);

        let result = infer_receiver_type(
            "session",
            &routine,
            &[],
            &from_obj,
            &graph,
            &index,
            None,
            None,
        );
        assert_eq!(
            result,
            ReceiverType::Framework(FrameworkKind::Session),
            "with no declared var, `Session` must still resolve to the platform \
             singleton — the fix must not break genuine singleton usage"
        );
    }

    /// GENERALITY (fixture 4): a DIFFERENT singleton name (`NavApp`, not
    /// `Session`) is also shadowed by a declared var, and still resolves to
    /// the platform singleton when undeclared — proves the fix applies to
    /// the whole match arm, not merely the one name the bug report named.
    #[test]
    fn t1_5_declared_navapp_var_shadows_singleton() {
        let (graph, app) = build_test_graph();
        let index = ResolveIndex::build(&graph);
        let routine = build_test_routine();
        let from_obj = make_object_node(app, ObjectKind::Codeunit, "CallerCu", Some(999), None);
        let mycodeunit_id = graph
            .resolve_object(app, ObjectKind::Codeunit, "MyCodeunit")
            .unwrap()
            .id
            .clone();
        let globals = vec![var_decl("NavApp", "Codeunit \"MyCodeunit\"")];

        let result = infer_receiver_type(
            "navapp", &routine, &globals, &from_obj, &graph, &index, None, None,
        );
        assert_eq!(
            result,
            ReceiverType::Object {
                kind: ObjectKind::Codeunit,
                name_lc: "mycodeunit".into(),
                id: Some(mycodeunit_id)
            },
            "a declared `NavApp` var must shadow the platform NavApp singleton too"
        );

        let result_undeclared = infer_receiver_type(
            "navapp",
            &routine,
            &[],
            &from_obj,
            &graph,
            &index,
            None,
            None,
        );
        assert_eq!(
            result_undeclared,
            ReceiverType::Framework(FrameworkKind::NavApp),
            "with no declared var, `NavApp` must still resolve to the platform singleton"
        );
    }

    /// PRECEDENCE (compiler-probed, `al.exe` control-tested — see Step 3c's
    /// doc comment): a same-named implicit-Rec table FIELD wins over the
    /// platform singleton too, not merely a declared var — proving Step 3c
    /// must sit AFTER Step 3a, not merely after Step 2. A Blob field
    /// literally named `Session` on the Customer table, accessed bare
    /// (`Session.CreateInStream(...)`-shaped) inside the table's own
    /// procedure, must type as `Framework(Blob)`, never
    /// `Framework(FrameworkKind::Session)`.
    #[test]
    fn t1_5_table_field_named_session_wins_over_singleton() {
        let (mut graph, _app) = build_test_graph();
        let customer_idx = graph
            .objects
            .iter()
            .position(|o| o.name == "Customer")
            .unwrap();
        graph.objects[customer_idx].fields.push(FieldNode {
            name_lc: "session".to_string(),
            type_text: "Blob".to_string(),
        });
        let index = ResolveIndex::build(&graph);
        let routine = routine_with_locals(vec![]);
        let from_obj = graph.objects[customer_idx].clone();
        let surface = DeclSurface::build(&graph, &[]);

        let result = infer_receiver_type(
            "session",
            &routine,
            &[],
            &from_obj,
            &graph,
            &index,
            None,
            Some((&surface, WithState::NoWithProven)),
        );
        assert_eq!(
            result,
            ReceiverType::Framework(FrameworkKind::Blob),
            "a same-named implicit-Rec table field must win over the platform \
             singleton (Step 3a runs before Step 3c)"
        );
    }

    /// `this` shadow (compiler-probed: `this` draws only a soft `AL0848`
    /// warning, never a hard error, and still shadows exactly like its
    /// singleton siblings — see Step 3c's doc). A declared global var
    /// literally named `This` must shadow the self-reference.
    #[test]
    fn t1_5_declared_this_var_shadows_self_object() {
        let (graph, app) = build_test_graph();
        let index = ResolveIndex::build(&graph);
        let routine = build_test_routine();
        let from_obj = make_object_node(app, ObjectKind::Codeunit, "CallerCu", Some(999), None);
        let mycodeunit_id = graph
            .resolve_object(app, ObjectKind::Codeunit, "MyCodeunit")
            .unwrap()
            .id
            .clone();
        let globals = vec![var_decl("This", "Codeunit \"MyCodeunit\"")];

        let result = infer_receiver_type(
            "this", &routine, &globals, &from_obj, &graph, &index, None, None,
        );
        assert_eq!(
            result,
            ReceiverType::Object {
                kind: ObjectKind::Codeunit,
                name_lc: "mycodeunit".into(),
                id: Some(mycodeunit_id)
            },
            "a declared `This` var must shadow the self-object reference"
        );
    }

    #[test]
    fn infer_rec_in_table_is_self() {
        let (graph, app) = build_test_graph();
        let index = ResolveIndex::build(&graph);
        let routine = build_test_routine();
        // from_object IS the Customer table
        let customer_node = graph
            .resolve_object(app, ObjectKind::Table, "Customer")
            .unwrap()
            .clone();

        let result = infer_receiver_type(
            "rec",
            &routine,
            &[],
            &customer_node,
            &graph,
            &index,
            None,
            None,
        );
        assert_eq!(
            result,
            ReceiverType::Record {
                table: Some(customer_node.id.clone())
            }
        );
    }

    #[test]
    fn infer_xrec_in_table_is_self() {
        let (graph, app) = build_test_graph();
        let index = ResolveIndex::build(&graph);
        let routine = build_test_routine();
        let customer_node = graph
            .resolve_object(app, ObjectKind::Table, "Customer")
            .unwrap()
            .clone();

        let result = infer_receiver_type(
            "xrec",
            &routine,
            &[],
            &customer_node,
            &graph,
            &index,
            None,
            None,
        );
        assert_eq!(
            result,
            ReceiverType::Record {
                table: Some(customer_node.id.clone())
            }
        );
    }

    #[test]
    fn infer_rec_in_table_extension_resolves_base() {
        let (graph, app) = build_test_graph();
        let index = ResolveIndex::build(&graph);
        let routine = build_test_routine();
        // A TableExtension extending "Customer"
        let te_obj = make_object_node(
            app,
            ObjectKind::TableExtension,
            "CustomerExt",
            Some(50400),
            Some("Customer".into()),
        );

        let customer_node = graph
            .resolve_object(app, ObjectKind::Table, "Customer")
            .unwrap();
        let expected_id = customer_node.id.clone();

        let result = infer_receiver_type("rec", &routine, &[], &te_obj, &graph, &index, None, None);
        assert_eq!(
            result,
            ReceiverType::Record {
                table: Some(expected_id)
            }
        );
    }

    #[test]
    fn infer_rec_in_table_extension_ambiguous_base_declines_to_none() {
        // Reuses `build_page_rec_fixture`'s "AmbTable" (declared in BOTH `a`
        // and `b`, neither is `w`) — Caller B (`infer_implicit_rec`'s
        // TableExtension arm) must DECLINE (`Record{table: None}`), never
        // silently pick the lowest `ObjectNodeId` (I1).
        let (graph, w) = build_page_rec_fixture();
        let index = ResolveIndex::build(&graph);
        let routine = build_test_routine();

        let te_obj = make_object_node(
            w,
            ObjectKind::TableExtension,
            "AmbExt",
            Some(50230),
            Some("AmbTable".into()),
        );

        let result = infer_receiver_type("rec", &routine, &[], &te_obj, &graph, &index, None, None);
        assert_eq!(result, ReceiverType::Record { table: None });
    }

    #[test]
    fn infer_rec_in_table_extension_out_of_closure_base_declines_to_none() {
        // Reuses `build_page_rec_fixture`'s "Orphan" table, declared in an
        // app `w` does not depend on.
        let (graph, w) = build_page_rec_fixture();
        let index = ResolveIndex::build(&graph);
        let routine = build_test_routine();

        let te_obj = make_object_node(
            w,
            ObjectKind::TableExtension,
            "OrphanExt",
            Some(50231),
            Some("Orphan".into()),
        );

        let result = infer_receiver_type("rec", &routine, &[], &te_obj, &graph, &index, None, None);
        assert_eq!(result, ReceiverType::Record { table: None });
    }

    #[test]
    fn infer_rec_in_page_is_record_none() {
        // No `SourceTable` property at all (`source_table: None` on ObjectNode)
        // — `make_object_node` never sets it, matching a Page with no property.
        let (graph, app) = build_test_graph();
        let index = ResolveIndex::build(&graph);
        let routine = build_test_routine();
        let page_obj = make_object_node(app, ObjectKind::Page, "CustomerCard", Some(21), None);

        let result =
            infer_receiver_type("rec", &routine, &[], &page_obj, &graph, &index, None, None);
        assert_eq!(result, ReceiverType::Record { table: None });
    }

    // -----------------------------------------------------------------------
    // infer_implicit_rec — Page/PageExtension SourceTable resolution (Task 5)
    // -----------------------------------------------------------------------

    /// Multi-app fixture for Page/PageExtension `SourceTable` resolution tests:
    /// - `w` (the `from`/workspace app): Table "Customer" (id 18, own
    ///   declaration) + Page "CustomerPage" (id 50200, `SourceTable = Customer`).
    ///   `w` depends on `a` and `b`.
    /// - `a`, `b`: BOTH declare Table "AmbTable" — a genuine cross-app name
    ///   collision, neither app is `w` itself, so it is `Ambiguous` from `w`'s
    ///   perspective.
    /// - `orphan`: Table "Orphan" (id 900), declared but NOT a dependency of
    ///   `w` — out of `w`'s closure.
    fn build_page_rec_fixture() -> (ProgramGraph, AppRef) {
        let mut apps = crate::program::node::AppRegistry::default();
        let mk_id = |name: &str| crate::snapshot::AppId {
            guid: String::new(),
            name: name.into(),
            publisher: "Test".into(),
            version: "1.0.0.0".into(),
        };
        let w = apps.intern(&mk_id("PageRecW"));
        let a = apps.intern(&mk_id("PageRecA"));
        let b = apps.intern(&mk_id("PageRecB"));
        let orphan = apps.intern(&mk_id("PageRecOrphan"));

        let mut topology = crate::program::topology::DependencyGraph::default();
        topology.add_dependency(w, a);
        topology.add_dependency(w, b);
        // `orphan` intentionally never wired in as a dependency of `w`.

        let mut customer_page =
            make_object_node(w, ObjectKind::Page, "CustomerPage", Some(50200), None);
        customer_page.source_table = Some(ObjectRef::Name {
            raw: "Customer".into(),
            normalized_lc: "customer".into(),
        });

        let mut objects = vec![
            make_object_node(w, ObjectKind::Table, "Customer", Some(18), None),
            customer_page,
            make_object_node(a, ObjectKind::Table, "AmbTable", Some(700), None),
            make_object_node(b, ObjectKind::Table, "AmbTable", Some(701), None),
            make_object_node(orphan, ObjectKind::Table, "Orphan", Some(900), None),
        ];
        objects.sort_by(|x, y| x.id.cmp(&y.id));

        let obj_index = ObjectIndex::build(&objects);
        let graph = ProgramGraph {
            apps,
            topology,
            objects,
            routines: vec![],
            obj_index,
            ..Default::default()
        };
        (graph, w)
    }

    fn amb_table_ref() -> ObjectRef {
        ObjectRef::Name {
            raw: "AmbTable".into(),
            normalized_lc: "ambtable".into(),
        }
    }

    fn orphan_table_ref() -> ObjectRef {
        ObjectRef::Name {
            raw: "Orphan".into(),
            normalized_lc: "orphan".into(),
        }
    }

    #[test]
    fn infer_rec_in_page_resolves_own_source_table_unique() {
        let (graph, w) = build_page_rec_fixture();
        let index = ResolveIndex::build(&graph);
        let routine = build_test_routine();

        let mut page = make_object_node(w, ObjectKind::Page, "CardPage", Some(50201), None);
        page.source_table = Some(ObjectRef::Name {
            raw: "Customer".into(),
            normalized_lc: "customer".into(),
        });

        let customer_id = graph
            .resolve_object(w, ObjectKind::Table, "Customer")
            .unwrap()
            .id
            .clone();

        let result = infer_receiver_type("rec", &routine, &[], &page, &graph, &index, None, None);
        assert_eq!(
            result,
            ReceiverType::Record {
                table: Some(customer_id)
            }
        );
    }

    #[test]
    fn infer_rec_in_page_ambiguous_source_table_declines_to_none() {
        let (graph, w) = build_page_rec_fixture();
        let index = ResolveIndex::build(&graph);
        let routine = build_test_routine();

        let mut page = make_object_node(w, ObjectKind::Page, "AmbPage", Some(50202), None);
        page.source_table = Some(amb_table_ref());

        // "AmbTable" is declared in BOTH `a` and `b` (neither is `w`) — must
        // DECLINE to None, never guess one of the two.
        let result = infer_receiver_type("rec", &routine, &[], &page, &graph, &index, None, None);
        assert_eq!(result, ReceiverType::Record { table: None });
    }

    #[test]
    fn infer_rec_in_page_out_of_closure_source_table_declines_to_none() {
        let (graph, w) = build_page_rec_fixture();
        let index = ResolveIndex::build(&graph);
        let routine = build_test_routine();

        let mut page = make_object_node(w, ObjectKind::Page, "OrphanPage", Some(50203), None);
        page.source_table = Some(orphan_table_ref());

        // "Orphan" is declared, but in an app `w` does not depend on.
        let result = infer_receiver_type("rec", &routine, &[], &page, &graph, &index, None, None);
        assert_eq!(result, ReceiverType::Record { table: None });
    }

    #[test]
    fn caller_a_e2e_two_dep_apps_same_name_table_declines_not_pick_first_source() {
        // Two DEPENDENCY apps (`a`/`b` in `build_page_rec_fixture`) both
        // declare `"AmbTable"` — an AL-illegal same-name collision WITHIN one
        // real compile closure, but a genuine cross-app collision in a merged
        // whole-program snapshot (I1). Neither is `w`'s own app, so Caller A
        // (`parsed_type_to_receiver`'s `Record` arm, reached via a declared
        // local `var R: Record "AmbTable"`) must DECLINE (`Record{table:
        // None}`) end to end through BOTH Phase A (receiver-type inference)
        // and Phase B (member-call resolution) — never silently pick the
        // lower `ObjectNodeId` as a confident (and possibly WRONG) `Source`
        // route, the cardinal sin.
        let (graph, w) = build_page_rec_fixture();
        let index = ResolveIndex::build(&graph);

        let o = test_origin();
        let routine = RoutineDecl {
            kind: RoutineKind::Procedure,
            name: "Test".into(),
            name_origin: o.clone(),
            params: vec![],
            return_type: None,
            return_name: None,
            locals: vec![VarDecl {
                name: "R".into(),
                ty: Some("Record \"AmbTable\"".into()),
                temporary: false,
                origin: o.clone(),
            }],
            attributes: vec![],
            attributes_parsed: vec![],
            access_modifier: None,
            parse_incomplete: false,
            dataitem_source_table: None,
            enclosing_member: None,
            in_dataset_modify_context: false,
            body: None,
            origin: o,
        };
        let from_obj = make_object_node(w, ObjectKind::Codeunit, "Caller", Some(50300), None);

        // Phase A: infer_receiver_type must decline, never resolve to either
        // dep's AmbTable.
        let receiver =
            infer_receiver_type("r", &routine, &[], &from_obj, &graph, &index, None, None);
        assert_eq!(receiver, ReceiverType::Record { table: None });

        // Phase B: a non-builtin method call on the declined receiver stays
        // the honest Unknown (not a fabricated Source route to either dep's
        // table) — closes the loop end to end (mirrors the already-covered
        // `resolve_member_record_table_none_emits_unknown` invariant in
        // `resolver.rs`, now driven by a genuine Phase-A ambiguity decline
        // rather than a hand-constructed `Record{table: None}`).
        let surface = crate::program::resolve::decl_surface::DeclSurface::build(&graph, &[]);
        let (shape, routes) = crate::program::resolve::resolver::resolve_member(
            &receiver,
            "nonbuiltinproc",
            0,
            &from_obj,
            &graph,
            &index,
            &surface,
        );
        assert_eq!(shape, crate::program::resolve::edge::DispatchShape::Exact);
        assert_eq!(routes.len(), 1);
        assert!(matches!(
            routes[0].evidence,
            crate::program::resolve::edge::Evidence::Unknown(_)
        ));
        assert_eq!(
            routes[0].target,
            crate::program::resolve::edge::RouteTarget::Unresolved
        );
    }

    #[test]
    fn infer_rec_in_pageext_with_no_own_source_table_inherits_base_page() {
        let (graph, w) = build_page_rec_fixture();
        let index = ResolveIndex::build(&graph);
        let routine = build_test_routine();

        // Extends "CustomerPage" (SourceTable = Customer) but declares no
        // SourceTable of its own.
        let page_ext = make_object_node(
            w,
            ObjectKind::PageExtension,
            "CustomerPageExt",
            Some(50210),
            Some("CustomerPage".into()),
        );

        let customer_id = graph
            .resolve_object(w, ObjectKind::Table, "Customer")
            .unwrap()
            .id
            .clone();

        let result =
            infer_receiver_type("rec", &routine, &[], &page_ext, &graph, &index, None, None);
        assert_eq!(
            result,
            ReceiverType::Record {
                table: Some(customer_id)
            }
        );
    }

    #[test]
    fn infer_rec_in_pageext_own_source_table_overrides_base_even_when_it_declines() {
        let (graph, w) = build_page_rec_fixture();
        let index = ResolveIndex::build(&graph);
        let routine = build_test_routine();

        // Extends "CustomerPage" (a page with a perfectly good SourceTable),
        // but ALSO declares its own (ambiguous) SourceTable — the own
        // declaration must win and DECLINE, never silently fall back to the
        // base page's Customer.
        let mut page_ext = make_object_node(
            w,
            ObjectKind::PageExtension,
            "OverridePageExt",
            Some(50211),
            Some("CustomerPage".into()),
        );
        page_ext.source_table = Some(amb_table_ref());

        let result =
            infer_receiver_type("rec", &routine, &[], &page_ext, &graph, &index, None, None);
        assert_eq!(result, ReceiverType::Record { table: None });
    }

    #[test]
    fn infer_rec_in_pageext_unresolvable_extends_target_declines_to_none() {
        let (graph, w) = build_page_rec_fixture();
        let index = ResolveIndex::build(&graph);
        let routine = build_test_routine();

        // Extends a base page that does not exist anywhere in the snapshot.
        let page_ext = make_object_node(
            w,
            ObjectKind::PageExtension,
            "DanglingExt",
            Some(50212),
            Some("NoSuchBasePage".into()),
        );

        let result =
            infer_receiver_type("rec", &routine, &[], &page_ext, &graph, &index, None, None);
        assert_eq!(result, ReceiverType::Record { table: None });
    }

    #[test]
    fn infer_rec_in_report_stays_none_even_if_source_table_were_present() {
        // Defensive: a Report's implicit Rec is ROUTINE-CONTEXTUAL ONLY
        // (dataitem-receivers plan, Task 1) — it is NEVER seeded from the
        // object-level `SourceTable` property (real extraction never sets one
        // from a per-dataitem source; this constructs it directly to lock in
        // the exclusion regardless of data presence). `build_test_routine()`
        // carries no `dataitem_source_table`/`in_dataset_modify_context`, so
        // the implicit Rec must stay honest `Record{table: None}` even though
        // `report.source_table` is (deliberately, artificially) populated —
        // see `infer_rec_in_report_dataitem_trigger_resolves_dataitem_table`
        // below for the POSITIVE routine-contextual case.
        let (graph, w) = build_page_rec_fixture();
        let index = ResolveIndex::build(&graph);
        let routine = build_test_routine();

        let mut report = make_object_node(w, ObjectKind::Report, "SomeReport", Some(50220), None);
        report.source_table = Some(ObjectRef::Name {
            raw: "Customer".into(),
            normalized_lc: "customer".into(),
        });

        let result = infer_receiver_type("rec", &routine, &[], &report, &graph, &index, None, None);
        assert_eq!(result, ReceiverType::Record { table: None });
    }

    // -----------------------------------------------------------------------
    // Dataitem-receivers plan, Task 1: report-dataitem receivers.
    // -----------------------------------------------------------------------

    fn dataitem(name: &str, table_lc: &str, table_raw: &str) -> DataitemNode {
        DataitemNode {
            name_lc: name.to_ascii_lowercase(),
            name: name.to_string(),
            source_table: ObjectRef::Name {
                raw: table_raw.to_string(),
                normalized_lc: table_lc.to_string(),
            },
        }
    }

    /// POSITIVE (routine-contextual implicit Rec): a trigger nested inside an
    /// ACTUAL `dataitem(Cust; Customer)` block — `routine.dataitem_source_table
    /// = Some("Customer")` — types the explicit `Rec.` receiver by the
    /// dataitem's source table, mirrors `ws-page-rec`'s Page/SourceTable
    /// precedent but for the per-dataitem Report case.
    #[test]
    fn infer_rec_in_report_dataitem_trigger_resolves_dataitem_table() {
        let (graph, w) = build_page_rec_fixture();
        let index = ResolveIndex::build(&graph);
        let routine = RoutineDecl {
            dataitem_source_table: Some("Customer".to_string()),
            ..build_test_routine()
        };
        let report = make_object_node(w, ObjectKind::Report, "SomeReport", Some(50221), None);
        let customer_id = graph
            .resolve_object(w, ObjectKind::Table, "Customer")
            .unwrap()
            .id
            .clone();

        let result = infer_receiver_type("rec", &routine, &[], &report, &graph, &index, None, None);
        assert_eq!(
            result,
            ReceiverType::Record {
                table: Some(customer_id)
            }
        );
    }

    /// REQUESTPAGE ISOLATION (binding, round-1 addendum): a requestpage
    /// trigger carries NEITHER `dataitem_source_table` NOR
    /// `in_dataset_modify_context` (the lowerer never threads either while
    /// descending `requestpage`) — even with an `enclosing_member` present
    /// (a requestpage control's own name), the implicit Rec must stay
    /// `Record{table: None}`, never fabricate a dataitem's table.
    #[test]
    fn infer_rec_in_report_requestpage_trigger_never_binds_dataitem_table() {
        let (graph, w) = build_page_rec_fixture();
        let index = ResolveIndex::build(&graph);
        let routine = RoutineDecl {
            dataitem_source_table: None,
            in_dataset_modify_context: false,
            enclosing_member: Some(("SomeControl".to_string(), test_origin())),
            ..build_test_routine()
        };
        let report = make_object_node(w, ObjectKind::Report, "SomeReport", Some(50222), None);

        let result = infer_receiver_type("rec", &routine, &[], &report, &graph, &index, None, None);
        assert_eq!(result, ReceiverType::Record { table: None });
    }

    /// POSITIVE (the `modify()` lowerer fallback): NO `dataitem_source_table`
    /// (the lowerer cannot itself resolve a `modify(Cust)` target to a
    /// table), but `in_dataset_modify_context = true` + `enclosing_member =
    /// "Cust"` — the resolve-time fallback looks `Cust` up in the report's
    /// own dataitem map and resolves the implicit Rec exactly as the direct
    /// case does.
    #[test]
    fn infer_rec_in_report_modify_fallback_resolves_via_enclosing_member() {
        let (graph, w) = build_page_rec_fixture();
        let index = ResolveIndex::build(&graph);
        let routine = RoutineDecl {
            dataitem_source_table: None,
            in_dataset_modify_context: true,
            enclosing_member: Some(("Cust".to_string(), test_origin())),
            ..build_test_routine()
        };
        let mut report = make_object_node(w, ObjectKind::Report, "SomeReport", Some(50223), None);
        report.dataitems = vec![dataitem("Cust", "customer", "Customer")];
        let customer_id = graph
            .resolve_object(w, ObjectKind::Table, "Customer")
            .unwrap()
            .id
            .clone();

        let result = infer_receiver_type("rec", &routine, &[], &report, &graph, &index, None, None);
        assert_eq!(
            result,
            ReceiverType::Record {
                table: Some(customer_id)
            }
        );
    }

    /// NEGATIVE: `in_dataset_modify_context = false` (a requestpage/layout/
    /// field/view `modify()`, or no confirmed dataset context at all) must
    /// NEVER trigger the fallback, even with a matching `enclosing_member`
    /// name and a real dataitem of that name on the report.
    #[test]
    fn infer_rec_in_report_modify_fallback_declines_without_dataset_context() {
        let (graph, w) = build_page_rec_fixture();
        let index = ResolveIndex::build(&graph);
        let routine = RoutineDecl {
            dataitem_source_table: None,
            in_dataset_modify_context: false,
            enclosing_member: Some(("Cust".to_string(), test_origin())),
            ..build_test_routine()
        };
        let mut report = make_object_node(w, ObjectKind::Report, "SomeReport", Some(50224), None);
        report.dataitems = vec![dataitem("Cust", "customer", "Customer")];

        let result = infer_receiver_type("rec", &routine, &[], &report, &graph, &index, None, None);
        assert_eq!(result, ReceiverType::Record { table: None });
    }

    /// POSITIVE (Step 2b): a bare dataitem-name receiver
    /// (`Cust.GetDisplayName()`) resolves `Record{table: Customer}` — the
    /// dataitem name is in scope as a record var across ALL the report's
    /// routines (not merely inside its own trigger), so a routine with NO
    /// dataitem context at all still resolves it. `routine_with_locals(vec![])`
    /// (not `build_test_routine()`, which declares an UNRELATED local also
    /// named `Cust: Record Customer` — that would shadow this receiver at
    /// Step 2 before Step 2b ever ran, silently testing the wrong step).
    #[test]
    fn infer_receiver_type_step2b_dataitem_name_resolves() {
        let (graph, w) = build_page_rec_fixture();
        let index = ResolveIndex::build(&graph);
        let routine = routine_with_locals(vec![]);
        let mut report = make_object_node(w, ObjectKind::Report, "SomeReport", Some(50225), None);
        report.dataitems = vec![dataitem("Cust", "customer", "Customer")];
        let customer_id = graph
            .resolve_object(w, ObjectKind::Table, "Customer")
            .unwrap()
            .id
            .clone();

        let result =
            infer_receiver_type("cust", &routine, &[], &report, &graph, &index, None, None);
        assert_eq!(
            result,
            ReceiverType::Record {
                table: Some(customer_id)
            }
        );
    }

    /// POSITIVE (Step 2b, quoted + embedded period — the naive dot-guard
    /// fix): a QUOTED dataitem name containing an embedded period resolves,
    /// exactly like the real CDO `"Sales Cr.Memo Header Filter"` shape.
    #[test]
    fn infer_receiver_type_step2b_dot_bearing_quoted_dataitem_name_resolves() {
        let (graph, w) = build_page_rec_fixture();
        let index = ResolveIndex::build(&graph);
        let routine = build_test_routine();
        let mut report = make_object_node(w, ObjectKind::Report, "SomeReport", Some(50226), None);
        report.dataitems = vec![dataitem(
            "Sales Cr.Memo Header Filter",
            "customer",
            "Customer",
        )];
        let customer_id = graph
            .resolve_object(w, ObjectKind::Table, "Customer")
            .unwrap()
            .id
            .clone();

        let result = infer_receiver_type(
            "\"sales cr.memo header filter\"",
            &routine,
            &[],
            &report,
            &graph,
            &index,
            None,
            None,
        );
        assert_eq!(
            result,
            ReceiverType::Record {
                table: Some(customer_id)
            }
        );
    }

    /// NEGATIVE (a genuinely compound receiver stays compound): `"A.B".C` —
    /// an UNQUOTED dot AFTER the closing quote — must never be treated as an
    /// atomic dataitem-name lookup, even if a dataitem happened to be named
    /// exactly `A.B` (it structurally can't be reached: `lookup_lc` stays the
    /// raw compound text, which no `name_lc` can ever equal).
    #[test]
    fn infer_receiver_type_step2b_unquoted_compound_receiver_stays_unknown() {
        let (graph, w) = build_page_rec_fixture();
        let index = ResolveIndex::build(&graph);
        let routine = build_test_routine();
        let mut report = make_object_node(w, ObjectKind::Report, "SomeReport", Some(50227), None);
        report.dataitems = vec![dataitem("A.B", "a.b", "Customer")];

        let result = infer_receiver_type(
            "\"a.b\".c",
            &routine,
            &[],
            &report,
            &graph,
            &index,
            None,
            None,
        );
        assert_eq!(result, ReceiverType::Unknown);
    }

    /// NEGATIVE (var shadows dataitem, AL scoping): a local var of the SAME
    /// name as a real dataitem must win — Step 2 runs strictly before Step
    /// 2b.
    #[test]
    fn infer_receiver_type_step2b_local_var_shadows_dataitem_name() {
        let (graph, w) = build_page_rec_fixture();
        let index = ResolveIndex::build(&graph);
        let routine = routine_with_locals(vec![var_decl("Cust", "Record Customer")]);
        let mut report = make_object_node(w, ObjectKind::Report, "SomeReport", Some(50228), None);
        // A DIFFERENT table than the var's declared type, so a mistaken
        // Step-2b hit would be observably distinguishable from the correct
        // Step-2 var hit.
        report.dataitems = vec![dataitem("Cust", "orphan", "Orphan")];
        let customer_id = graph
            .resolve_object(w, ObjectKind::Table, "Customer")
            .unwrap()
            .id
            .clone();

        let result =
            infer_receiver_type("cust", &routine, &[], &report, &graph, &index, None, None);
        assert_eq!(
            result,
            ReceiverType::Record {
                table: Some(customer_id)
            },
            "the local var must win over the same-named dataitem"
        );
    }

    /// NEGATIVE (collision guard, fail-closed): a dataitem name that is ALSO
    /// a report procedure name must decline — AL's parens-optional zero-arg
    /// call makes `Name.X()` structurally ambiguous between the dataitem
    /// record and a parens-less call to the procedure. `routine_with_locals(
    /// vec![])`, NOT `build_test_routine()` — see the sibling positive test's
    /// doc for why (an unrelated `Cust` local would shadow this receiver at
    /// Step 2, making the collision guard below untested).
    #[test]
    fn infer_receiver_type_step2b_declines_when_same_named_routine_exists() {
        let (mut graph, w) = build_page_rec_fixture();
        let report_id = ObjectNodeId {
            app: w,
            kind: ObjectKind::Report,
            key: ObjKey::Id(50229),
        };
        let mut report = make_object_node(w, ObjectKind::Report, "SomeReport", Some(50229), None);
        report.dataitems = vec![dataitem("Cust", "customer", "Customer")];
        graph.objects.push(report.clone());
        graph.objects.sort_by(|a, b| a.id.cmp(&b.id));
        graph.routines = vec![make_routine_node(report_id, "Cust")];
        graph.routines.sort_by(|x, y| x.id.cmp(&y.id));
        let index = ResolveIndex::build(&graph);
        let routine = routine_with_locals(vec![]);

        let result =
            infer_receiver_type("cust", &routine, &[], &report, &graph, &index, None, None);
        assert_eq!(result, ReceiverType::Unknown);
    }

    /// NEGATIVE (duplicate-across-own-and-base guard, fail-closed): a
    /// ReportExtension declares its OWN dataitem with the same name as one
    /// on its extended BASE report, resolving to a DIFFERENT table — an
    /// unprovable ambiguity, never pick one. `routine_with_locals(vec![])` —
    /// see the sibling positive test's doc for why not `build_test_routine()`.
    #[test]
    fn infer_receiver_type_step2b_declines_on_duplicate_name_across_own_and_base() {
        let (mut graph, w) = build_page_rec_fixture();
        let mut base = make_object_node(w, ObjectKind::Report, "BaseReport", Some(50230), None);
        base.dataitems = vec![dataitem("Cust", "customer", "Customer")];
        let mut ext = make_object_node(
            w,
            ObjectKind::ReportExtension,
            "ExtReport",
            Some(50231),
            Some("BaseReport".to_string()),
        );
        ext.dataitems = vec![dataitem("Cust", "orphan", "Orphan")];
        graph.objects.push(base);
        graph.objects.push(ext.clone());
        graph.objects.sort_by(|a, b| a.id.cmp(&b.id));
        let index = ResolveIndex::build(&graph);
        let routine = routine_with_locals(vec![]);

        let result = infer_receiver_type("cust", &routine, &[], &ext, &graph, &index, None, None);
        assert_eq!(result, ReceiverType::Unknown);
    }

    /// POSITIVE (ReportExtension base fallback): an extension with NO
    /// dataitems of its own still resolves a dataitem-name receiver naming
    /// the extended BASE report's dataitem — mirrors the PageExtension
    /// `SourceTable` fallback pattern. `routine_with_locals(vec![])` — see
    /// the first Step 2b positive test's doc for why not `build_test_routine()`.
    #[test]
    fn infer_receiver_type_step2b_reportextension_resolves_via_base_dataitem() {
        let (mut graph, w) = build_page_rec_fixture();
        let mut base = make_object_node(w, ObjectKind::Report, "BaseReport", Some(50232), None);
        base.dataitems = vec![dataitem("Cust", "customer", "Customer")];
        let ext = make_object_node(
            w,
            ObjectKind::ReportExtension,
            "ExtReport",
            Some(50233),
            Some("BaseReport".to_string()),
        );
        graph.objects.push(base);
        graph.objects.push(ext.clone());
        graph.objects.sort_by(|a, b| a.id.cmp(&b.id));
        let index = ResolveIndex::build(&graph);
        let routine = routine_with_locals(vec![]);
        let customer_id = graph
            .resolve_object(w, ObjectKind::Table, "Customer")
            .unwrap()
            .id
            .clone();

        let result = infer_receiver_type("cust", &routine, &[], &ext, &graph, &index, None, None);
        assert_eq!(
            result,
            ReceiverType::Record {
                table: Some(customer_id)
            }
        );
    }

    // -----------------------------------------------------------------------
    // `is_atomic_receiver_token` (centralized quote-aware token guard).
    // -----------------------------------------------------------------------

    #[test]
    fn is_atomic_receiver_token_cases() {
        assert!(is_atomic_receiver_token("cust"), "plain unquoted");
        assert!(is_atomic_receiver_token("\"file blob\""), "quoted, no dot");
        assert!(
            is_atomic_receiver_token("\"sales cr.memo header filter\""),
            "quoted, embedded dot"
        );
        assert!(!is_atomic_receiver_token("a.b"), "unquoted compound");
        assert!(
            !is_atomic_receiver_token("\"a.b\".c"),
            "quoted dot then trailing unquoted segment"
        );
        assert!(
            !is_atomic_receiver_token("\"a.b\".\"c.d\""),
            "two quoted segments joined by an unquoted dot"
        );
        assert!(!is_atomic_receiver_token("foo()"), "call form");
        assert!(!is_atomic_receiver_token("\"\""), "degenerate empty quote");
        assert!(
            !is_atomic_receiver_token("\"a\"\"b\""),
            "escaped-quote identifier fails closed to compound"
        );
    }

    /// Task 1 review-fix regression guard: a well-formed QUOTED identifier
    /// containing an interior paren is a real BC field-name shape (`"View
    /// (Blob)"`, `"Request Page (XML)"`) and MUST classify atomic — the
    /// paren-exclusion is a CALL-SHAPE guard that only applies to the
    /// UNQUOTED branch (`foo(1)` is a call; a quoted span never is). The
    /// pre-fix version applied the unquoted branch's `contains('(')` check
    /// before the quote-parity check and wrongly failed these to COMPOUND —
    /// an 8-site CDO regression (Table 6175282/:172,:179,
    /// 6175284/:900,:911, 6175307/:287,:298 +2 in
    /// `CDOPageDefaultfilter.Table.al`), since fixed.
    #[test]
    fn is_atomic_receiver_token_quoted_paren_is_atomic() {
        assert!(
            is_atomic_receiver_token("\"view (blob)\""),
            "quoted identifier with interior paren must stay atomic"
        );
        assert!(
            is_atomic_receiver_token("\"request page (xml)\""),
            "quoted identifier with interior paren must stay atomic"
        );
    }

    /// Companion negatives for the same review-fix: the unquoted call-shape
    /// guard and the quoted-then-trailing-segment compound shape must both
    /// still correctly decline, exactly as before the fix.
    #[test]
    fn is_atomic_receiver_token_paren_fix_negatives() {
        assert!(
            !is_atomic_receiver_token("foo(1)"),
            "unquoted call-shape with an argument must still decline"
        );
        assert!(
            !is_atomic_receiver_token("\"a.b\".c"),
            "quoted segment followed by an unquoted trailing segment must still decline"
        );
        assert!(
            !is_atomic_receiver_token("\"\""),
            "degenerate empty quote must still decline"
        );
    }

    #[test]
    fn infer_rec_in_codeunit_is_unknown() {
        let (graph, app) = build_test_graph();
        let index = ResolveIndex::build(&graph);
        let routine = build_test_routine();
        let cu_obj = make_object_node(app, ObjectKind::Codeunit, "CallerCu", Some(999), None);

        let result = infer_receiver_type("rec", &routine, &[], &cu_obj, &graph, &index, None, None);
        assert_eq!(result, ReceiverType::Unknown);
    }

    // -----------------------------------------------------------------------
    // infer_implicit_rec — Codeunit TableNo resolution (Task 6)
    //
    // Reuses `build_page_rec_fixture`'s Customer (in `w`)/AmbTable (cross-app
    // ambiguous, in `a` and `b`)/Orphan (out of `w`'s closure) tables — the
    // same topology shapes Task 5 exercised for Page's `SourceTable`, now
    // driving a Codeunit's `TableNo` instead.
    // -----------------------------------------------------------------------

    #[test]
    fn infer_rec_in_codeunit_resolves_table_no_unique() {
        let (graph, w) = build_page_rec_fixture();
        let index = ResolveIndex::build(&graph);
        let routine = build_test_routine();

        let mut cu = make_object_node(w, ObjectKind::Codeunit, "ItemCu", Some(50230), None);
        cu.table_no = Some(ObjectRef::Name {
            raw: "Customer".into(),
            normalized_lc: "customer".into(),
        });

        let customer_id = graph
            .resolve_object(w, ObjectKind::Table, "Customer")
            .unwrap()
            .id
            .clone();

        let result = infer_receiver_type("rec", &routine, &[], &cu, &graph, &index, None, None);
        assert_eq!(
            result,
            ReceiverType::Record {
                table: Some(customer_id)
            }
        );
    }

    #[test]
    fn infer_rec_in_codeunit_no_table_no_is_unknown() {
        let (graph, w) = build_page_rec_fixture();
        let index = ResolveIndex::build(&graph);
        let routine = build_test_routine();

        // No `TableNo` declared at all — this is also the shape of a
        // `Subtype = Test`/`TestRunner` codeunit (never declares `TableNo`):
        // no implicit-Rec entity exists at all, so this is the honest
        // `Unknown`, NOT `Record{table: None}` (that variant is reserved for
        // "a Record entity exists but its table failed to resolve", which
        // does not apply when there is no `TableNo` to resolve in the first
        // place).
        let cu = make_object_node(w, ObjectKind::Codeunit, "PlainCu", Some(50231), None);

        let result = infer_receiver_type("rec", &routine, &[], &cu, &graph, &index, None, None);
        assert_eq!(result, ReceiverType::Unknown);
    }

    #[test]
    fn infer_rec_in_codeunit_ambiguous_table_no_declines_to_record_none() {
        let (graph, w) = build_page_rec_fixture();
        let index = ResolveIndex::build(&graph);
        let routine = build_test_routine();

        let mut cu = make_object_node(w, ObjectKind::Codeunit, "AmbCu", Some(50232), None);
        cu.table_no = Some(amb_table_ref());

        // "AmbTable" is declared in BOTH `a` and `b` (neither is `w`) — must
        // DECLINE, never guess one of the two. `TableNo` IS present, so this
        // stays `Record{table: None}` (mirroring Page's non-`Unique`
        // treatment: builtins still resolve table-independently in Phase B),
        // not `Unknown`.
        let result = infer_receiver_type("rec", &routine, &[], &cu, &graph, &index, None, None);
        assert_eq!(result, ReceiverType::Record { table: None });
    }

    #[test]
    fn infer_rec_in_codeunit_out_of_closure_table_no_declines_to_record_none() {
        let (graph, w) = build_page_rec_fixture();
        let index = ResolveIndex::build(&graph);
        let routine = build_test_routine();

        let mut cu = make_object_node(w, ObjectKind::Codeunit, "OrphanCu", Some(50233), None);
        cu.table_no = Some(orphan_table_ref());

        // "Orphan" is declared, but in an app `w` does not depend on.
        let result = infer_receiver_type("rec", &routine, &[], &cu, &graph, &index, None, None);
        assert_eq!(result, ReceiverType::Record { table: None });
    }

    #[test]
    fn infer_unknown_name_is_unknown() {
        let (graph, app) = build_test_graph();
        let index = ResolveIndex::build(&graph);
        let routine = build_test_routine();
        let from_obj = make_object_node(app, ObjectKind::Codeunit, "CallerCu", Some(999), None);

        let result = infer_receiver_type(
            "notdeclaredanywhere",
            &routine,
            &[],
            &from_obj,
            &graph,
            &index,
            None,
            None,
        );
        assert_eq!(result, ReceiverType::Unknown);
    }

    #[test]
    fn infer_object_globals_lookup() {
        let (graph, app) = build_test_graph();
        let index = ResolveIndex::build(&graph);
        let routine = build_test_routine();
        let from_obj = make_object_node(app, ObjectKind::Codeunit, "CallerCu", Some(999), None);
        let mycodeunit_id = graph
            .resolve_object(app, ObjectKind::Codeunit, "MyCodeunit")
            .unwrap()
            .id
            .clone();

        let o = test_origin();
        let globals = vec![VarDecl {
            name: "GlobalCu".into(),
            ty: Some("Codeunit \"MyCodeunit\"".into()),
            temporary: false,
            origin: o,
        }];

        let result = infer_receiver_type(
            "globalcu", &routine, &globals, &from_obj, &graph, &index, None, None,
        );
        assert_eq!(
            result,
            ReceiverType::Object {
                kind: ObjectKind::Codeunit,
                name_lc: "mycodeunit".into(),
                id: Some(mycodeunit_id)
            }
        );
    }

    #[test]
    fn infer_local_interface_type() {
        let (graph, app) = build_test_graph();
        let index = ResolveIndex::build(&graph);
        let routine = build_test_routine();
        let from_obj = make_object_node(app, ObjectKind::Codeunit, "CallerCu", Some(999), None);

        let result = infer_receiver_type(
            "iface",
            &routine,
            &[],
            &from_obj,
            &graph,
            &index,
            None,
            None,
        );
        assert_eq!(
            result,
            ReceiverType::Interface {
                name_lc: "imyinterface".into()
            }
        );
    }

    #[test]
    fn infer_local_enum_type() {
        let (graph, app) = build_test_graph();
        let index = ResolveIndex::build(&graph);
        let routine = build_test_routine();
        let from_obj = make_object_node(app, ObjectKind::Codeunit, "CallerCu", Some(999), None);

        let result = infer_receiver_type(
            "enumvar",
            &routine,
            &[],
            &from_obj,
            &graph,
            &index,
            None,
            None,
        );
        assert_eq!(
            result,
            ReceiverType::EnumType {
                name_lc: "color".into()
            }
        );
    }

    #[test]
    fn infer_param_shadows_globals() {
        // A param and a global with the same lowercased name — param wins.
        let (graph, app) = build_test_graph();
        let index = ResolveIndex::build(&graph);
        let routine = build_test_routine();
        let from_obj = make_object_node(app, ObjectKind::Codeunit, "CallerCu", Some(999), None);
        let mycodeunit_id = graph
            .resolve_object(app, ObjectKind::Codeunit, "MyCodeunit")
            .unwrap()
            .id
            .clone();

        let o = test_origin();
        // Global also named "CuParam" but with a different type
        let globals = vec![VarDecl {
            name: "CuParam".into(),
            ty: Some("JsonObject".into()),
            temporary: false,
            origin: o,
        }];

        // Should resolve via the PARAM (Codeunit "MyCodeunit"), not the global (JsonObject)
        let result = infer_receiver_type(
            "cuparam", &routine, &globals, &from_obj, &graph, &index, None, None,
        );
        assert_eq!(
            result,
            ReceiverType::Object {
                kind: ObjectKind::Codeunit,
                name_lc: "mycodeunit".into(),
                id: Some(mycodeunit_id)
            }
        );
    }

    #[test]
    fn infer_record_unresolvable_table_is_record_none() {
        // A local `var R: Record "NonExistentTable"` — Record{None} not Unknown
        let (graph, app) = build_test_graph();
        let index = ResolveIndex::build(&graph);
        let o = test_origin();
        let routine_with_dep_record = RoutineDecl {
            kind: RoutineKind::Procedure,
            name: "P".into(),
            name_origin: o.clone(),
            params: vec![],
            return_type: None,
            return_name: None,
            locals: vec![VarDecl {
                name: "R".into(),
                ty: Some("Record \"NonExistentTable\"".into()),
                temporary: false,
                origin: o.clone(),
            }],
            attributes: vec![],
            attributes_parsed: vec![],
            access_modifier: None,
            parse_incomplete: false,
            dataitem_source_table: None,
            enclosing_member: None,
            in_dataset_modify_context: false,
            body: None,
            origin: o,
        };

        let from_obj = make_object_node(app, ObjectKind::Codeunit, "CallerCu", Some(999), None);
        let result = infer_receiver_type(
            "r",
            &routine_with_dep_record,
            &[],
            &from_obj,
            &graph,
            &index,
            None,
            None,
        );
        // Record with unresolvable table → Record{None} (not Unknown)
        assert_eq!(result, ReceiverType::Record { table: None });
    }

    // Fix 1 — rec/xrec variable lookup before implicit-rec
    #[test]
    fn infer_rec_local_in_codeunit_resolves_via_variable() {
        // A Codeunit routine with `var Rec: Record Customer` — `Rec.SetRange(...)`
        // must resolve to Record{Some(customer_id)}, NOT Unknown (which was the
        // bug: the old code hit infer_implicit_rec(Codeunit) → Unknown before the
        // variable lookup).
        let (graph, app) = build_test_graph();
        let index = ResolveIndex::build(&graph);
        let o = test_origin();
        let routine_with_rec_local = RoutineDecl {
            kind: RoutineKind::Procedure,
            name: "TestRecLocal".into(),
            name_origin: o.clone(),
            params: vec![],
            return_type: None,
            return_name: None,
            locals: vec![VarDecl {
                name: "Rec".into(),
                ty: Some("Record Customer".into()),
                temporary: false,
                origin: o.clone(),
            }],
            attributes: vec![],
            attributes_parsed: vec![],
            access_modifier: None,
            parse_incomplete: false,
            dataitem_source_table: None,
            enclosing_member: None,
            in_dataset_modify_context: false,
            body: None,
            origin: o,
        };
        let cu_obj = make_object_node(app, ObjectKind::Codeunit, "CallerCu", Some(999), None);

        let customer_node = graph
            .resolve_object(app, ObjectKind::Table, "Customer")
            .unwrap();
        let expected_id = customer_node.id.clone();

        // receiver "rec" (lc) → local variable `Rec: Record Customer` → Record{Some(customer_id)}
        let result = infer_receiver_type(
            "rec",
            &routine_with_rec_local,
            &[],
            &cu_obj,
            &graph,
            &index,
            None,
            None,
        );
        assert_eq!(
            result,
            ReceiverType::Record {
                table: Some(expected_id)
            }
        );
    }

    // Fix 3 — static framework type name as receiver
    #[test]
    fn infer_static_xml_document_receiver() {
        // `XmlDocument.Create(...)` — bare `XmlDocument` with no matching variable
        // must type as Framework(Xml), not Unknown.
        let (graph, app) = build_test_graph();
        let index = ResolveIndex::build(&graph);
        let routine = build_test_routine();
        let from_obj = make_object_node(app, ObjectKind::Codeunit, "CallerCu", Some(999), None);

        let result = infer_receiver_type(
            "xmldocument",
            &routine,
            &[],
            &from_obj,
            &graph,
            &index,
            None,
            None,
        );
        assert_eq!(result, ReceiverType::Framework(FrameworkKind::Xml));
    }

    #[test]
    fn infer_static_text_receiver() {
        // `Text.CopyStr(...)` — bare `Text` with no matching variable must type
        // as Framework(Text), not Unknown.
        let (graph, app) = build_test_graph();
        let index = ResolveIndex::build(&graph);
        let routine = build_test_routine();
        let from_obj = make_object_node(app, ObjectKind::Codeunit, "CallerCu", Some(999), None);

        let result =
            infer_receiver_type("text", &routine, &[], &from_obj, &graph, &index, None, None);
        assert_eq!(result, ReceiverType::Framework(FrameworkKind::Text));
    }

    // -----------------------------------------------------------------------
    // parse_currpage_dot_page_segment — low-level shape parse (Task 7)
    // -----------------------------------------------------------------------

    #[test]
    fn parse_currpage_segment_unquoted_part_dot_page() {
        assert_eq!(
            parse_currpage_dot_page_segment("lines.page"),
            Some("lines".to_string())
        );
    }

    #[test]
    fn parse_currpage_segment_bare_part_no_page_is_none() {
        // `CurrPage.Lines` (no `.Page`) — the CONTROL, not the subpage
        // instance.
        assert_eq!(parse_currpage_dot_page_segment("lines"), None);
    }

    #[test]
    fn parse_currpage_segment_deep_chain_is_none() {
        assert_eq!(parse_currpage_dot_page_segment("lines.page.foo"), None);
    }

    #[test]
    fn parse_currpage_segment_quoted_part_dot_page() {
        assert_eq!(
            parse_currpage_dot_page_segment("\"sub lines\".page"),
            Some("sub lines".to_string())
        );
    }

    #[test]
    fn parse_currpage_segment_malformed_unterminated_quote_is_none() {
        assert_eq!(parse_currpage_dot_page_segment("\"unterminated.page"), None);
    }

    #[test]
    fn parse_currpage_segment_empty_is_none() {
        assert_eq!(parse_currpage_dot_page_segment(""), None);
    }

    // -----------------------------------------------------------------------
    // infer_receiver_type — `CurrPage.<part>.Page` subpage-instance
    // receivers (Task 7)
    //
    // Fixture: workspace app `w` with:
    // - Page "SubPage" (id 50310) — the subpage instance target.
    // - Page "HostPage" (id 50311) with THREE controls: `Lines` (Part →
    //   SubPage), `"Sub Lines"` (Part → SubPage, quoted name), `Notes`
    //   (SystemPart), `MyAddIn` (UserControl).
    // - PageExtension "HostPageExt" (id 50312, extends HostPage) with NO
    //   controls of its own — must inherit HostPage's via the merge.
    // -----------------------------------------------------------------------

    fn build_currpage_fixture() -> (ProgramGraph, AppRef) {
        let mut apps = crate::program::node::AppRegistry::default();
        let mk_id = |name: &str| crate::snapshot::AppId {
            guid: String::new(),
            name: name.into(),
            publisher: "Test".into(),
            version: "1.0.0.0".into(),
        };
        let w = apps.intern(&mk_id("CurrPageW"));
        let topology = crate::program::topology::DependencyGraph::default();

        let subpage = make_object_node(w, ObjectKind::Page, "SubPage", Some(50310), None);

        let mut host = make_object_node(w, ObjectKind::Page, "HostPage", Some(50311), None);
        let subpage_target = ObjectRef::Name {
            raw: "SubPage".into(),
            normalized_lc: "subpage".into(),
        };
        host.page_controls = vec![
            PageControlNode {
                name_lc: "lines".into(),
                kind: PageControlKind::Part,
                target: subpage_target.clone(),
            },
            PageControlNode {
                name_lc: "sub lines".into(),
                kind: PageControlKind::Part,
                target: subpage_target,
            },
            PageControlNode {
                name_lc: "notes".into(),
                kind: PageControlKind::SystemPart,
                target: ObjectRef::Name {
                    raw: "Notes".into(),
                    normalized_lc: "notes".into(),
                },
            },
            PageControlNode {
                name_lc: "myaddin".into(),
                kind: PageControlKind::UserControl,
                target: ObjectRef::Name {
                    raw: "MyAddIn".into(),
                    normalized_lc: "myaddin".into(),
                },
            },
        ];

        let host_ext = make_object_node(
            w,
            ObjectKind::PageExtension,
            "HostPageExt",
            Some(50312),
            Some("HostPage".into()),
        );

        let mut objects = vec![subpage, host, host_ext];
        objects.sort_by(|a, b| a.id.cmp(&b.id));
        let obj_index = ObjectIndex::build(&objects);
        let graph = ProgramGraph {
            apps,
            topology,
            objects,
            routines: vec![],
            obj_index,
            ..Default::default()
        };
        (graph, w)
    }

    /// Test (a), POSITIVE: `CurrPage.Lines.Page` resolves to the SubPage
    /// object, carrying its id mechanically.
    #[test]
    fn infer_currpage_part_page_resolves_subpage_object_with_id() {
        let (graph, w) = build_currpage_fixture();
        let index = ResolveIndex::build(&graph);
        let routine = build_test_routine();
        let host = graph
            .objects
            .iter()
            .find(|o| o.name == "HostPage")
            .unwrap()
            .clone();
        let subpage_id = graph
            .resolve_object(w, ObjectKind::Page, "SubPage")
            .unwrap()
            .id
            .clone();

        let result = infer_receiver_type(
            "currpage.lines.page",
            &routine,
            &[],
            &host,
            &graph,
            &index,
            None,
            None,
        );
        assert_eq!(
            result,
            ReceiverType::Object {
                kind: ObjectKind::Page,
                name_lc: "subpage".into(),
                id: Some(subpage_id),
            }
        );
    }

    /// POSITIVE, quoted control name: `CurrPage."Sub Lines".Page` resolves
    /// identically — quotes must be stripped when matching `page_controls`.
    #[test]
    fn infer_currpage_quoted_part_page_resolves_subpage_object() {
        let (graph, w) = build_currpage_fixture();
        let index = ResolveIndex::build(&graph);
        let routine = build_test_routine();
        let host = graph
            .objects
            .iter()
            .find(|o| o.name == "HostPage")
            .unwrap()
            .clone();
        let subpage_id = graph
            .resolve_object(w, ObjectKind::Page, "SubPage")
            .unwrap()
            .id
            .clone();

        let result = infer_receiver_type(
            "currpage.\"sub lines\".page",
            &routine,
            &[],
            &host,
            &graph,
            &index,
            None,
            None,
        );
        assert_eq!(
            result,
            ReceiverType::Object {
                kind: ObjectKind::Page,
                name_lc: "subpage".into(),
                id: Some(subpage_id),
            }
        );
    }

    /// Test (b), NEGATIVE — control vs subpage: `CurrPage.Lines` (no
    /// `.Page`) is the CONTROL, not the subpage instance — must stay
    /// `Unknown`, never fabricated as `SubPage`.
    #[test]
    fn infer_currpage_bare_part_no_page_accessor_stays_unknown() {
        let (graph, _w) = build_currpage_fixture();
        let index = ResolveIndex::build(&graph);
        let routine = build_test_routine();
        let host = graph
            .objects
            .iter()
            .find(|o| o.name == "HostPage")
            .unwrap()
            .clone();

        let result = infer_receiver_type(
            "currpage.lines",
            &routine,
            &[],
            &host,
            &graph,
            &index,
            None,
            None,
        );
        assert_eq!(result, ReceiverType::Unknown);
    }

    /// Test (c), NEGATIVE — deep chain: `CurrPage.Lines.Page.Foo` (more than
    /// one remaining segment) stays `Unknown`.
    #[test]
    fn infer_currpage_deep_chain_beyond_dot_page_stays_unknown() {
        let (graph, _w) = build_currpage_fixture();
        let index = ResolveIndex::build(&graph);
        let routine = build_test_routine();
        let host = graph
            .objects
            .iter()
            .find(|o| o.name == "HostPage")
            .unwrap()
            .clone();

        let result = infer_receiver_type(
            "currpage.lines.page.foo",
            &routine,
            &[],
            &host,
            &graph,
            &index,
            None,
            None,
        );
        assert_eq!(result, ReceiverType::Unknown);
    }

    /// Test (d), NEGATIVE — unknown part: `CurrPage.Nope.Page` (no control
    /// named "Nope") stays `Unknown`.
    #[test]
    fn infer_currpage_unknown_part_stays_unknown() {
        let (graph, _w) = build_currpage_fixture();
        let index = ResolveIndex::build(&graph);
        let routine = build_test_routine();
        let host = graph
            .objects
            .iter()
            .find(|o| o.name == "HostPage")
            .unwrap()
            .clone();

        let result = infer_receiver_type(
            "currpage.nope.page",
            &routine,
            &[],
            &host,
            &graph,
            &index,
            None,
            None,
        );
        assert_eq!(result, ReceiverType::Unknown);
    }

    /// Test (e), NEGATIVE — SystemPart: even WITH a `.Page` accessor, a
    /// SystemPart control must NOT resolve to a fabricated Object/Framework
    /// route — Task 7 scope is `Part` only.
    #[test]
    fn infer_currpage_systempart_dot_page_stays_unknown_not_fabricated() {
        let (graph, _w) = build_currpage_fixture();
        let index = ResolveIndex::build(&graph);
        let routine = build_test_routine();
        let host = graph
            .objects
            .iter()
            .find(|o| o.name == "HostPage")
            .unwrap()
            .clone();

        let result = infer_receiver_type(
            "currpage.notes.page",
            &routine,
            &[],
            &host,
            &graph,
            &index,
            None,
            None,
        );
        assert_eq!(result, ReceiverType::Unknown);
    }

    /// Test (e), NEGATIVE — UserControl: same as SystemPart, `.Page` on a
    /// UserControl must decline, not fabricate a route.
    #[test]
    fn infer_currpage_usercontrol_dot_page_stays_unknown_not_fabricated() {
        let (graph, _w) = build_currpage_fixture();
        let index = ResolveIndex::build(&graph);
        let routine = build_test_routine();
        let host = graph
            .objects
            .iter()
            .find(|o| o.name == "HostPage")
            .unwrap()
            .clone();

        let result = infer_receiver_type(
            "currpage.myaddin.page",
            &routine,
            &[],
            &host,
            &graph,
            &index,
            None,
            None,
        );
        assert_eq!(result, ReceiverType::Unknown);
    }

    /// NEGATIVE — bare SystemPart/UserControl (no `.Page` at all) also stay
    /// `Unknown`, exercising the ordinary "no .page suffix" decline path for
    /// these control kinds too.
    /// RENAMED (Task 1, M1 — was `infer_currpage_bare_systempart_and_usercontrol_stay_unknown`,
    /// split in two): a `SystemPart` control's bare form ALWAYS declines —
    /// `SystemPart` is explicitly OUT of the Step 0b `ControlAddIn` arm
    /// entirely (native platform components, not JS add-ins; no `controladdin`
    /// object could ever back one). This assertion is unaffected by Task 1.
    #[test]
    fn infer_currpage_bare_systempart_stays_unknown() {
        let (graph, _w) = build_currpage_fixture();
        let index = ResolveIndex::build(&graph);
        let routine = build_test_routine();
        let host = graph
            .objects
            .iter()
            .find(|o| o.name == "HostPage")
            .unwrap()
            .clone();

        assert_eq!(
            infer_receiver_type(
                "currpage.notes",
                &routine,
                &[],
                &host,
                &graph,
                &index,
                None,
                None
            ),
            ReceiverType::Unknown
        );
    }

    /// RENAMED (Task 1, M1 — was `infer_currpage_bare_systempart_and_usercontrol_stay_unknown`,
    /// split in two): a bare `UserControl` whose declared addin type
    /// (`"MyAddIn"`) has NO reachable source/symbol declaration in
    /// `build_currpage_fixture` (only Page/PageExtension objects exist there)
    /// AND is not on the `TRUE_PLATFORM_CONTROL_ADDINS` allowlist stays
    /// `Unknown` — `ObjectRefResolution::Unresolved` + non-platform name,
    /// Task 1's tri-state gate's genuinely-absent-and-non-platform outcome.
    /// This is now a NARROWER claim than the old test's name implied — a
    /// usercontrol whose addin type IS resolvable now resolves (see the
    /// dedicated `infer_currpage_usercontrol_*` tests below) — so this test
    /// exists specifically to prove the undeclared/non-platform case still
    /// declines, not that usercontrols categorically never resolve.
    #[test]
    fn infer_currpage_bare_usercontrol_undeclared_nonplatform_name_stays_unknown() {
        let (graph, _w) = build_currpage_fixture();
        let index = ResolveIndex::build(&graph);
        let routine = build_test_routine();
        let host = graph
            .objects
            .iter()
            .find(|o| o.name == "HostPage")
            .unwrap()
            .clone();

        assert_eq!(
            infer_receiver_type(
                "currpage.myaddin",
                &routine,
                &[],
                &host,
                &graph,
                &index,
                None,
                None
            ),
            ReceiverType::Unknown
        );
    }

    /// PageExtension merge: `HostPageExt` (extends `HostPage`, no controls
    /// of its own) inherits `HostPage`'s `Lines` control via the fail-closed
    /// base-page lookup — mirrors L3's `page_controls_for` merge.
    #[test]
    fn infer_currpage_pageext_inherits_base_page_control() {
        let (graph, w) = build_currpage_fixture();
        let index = ResolveIndex::build(&graph);
        let routine = build_test_routine();
        let host_ext = graph
            .objects
            .iter()
            .find(|o| o.name == "HostPageExt")
            .unwrap()
            .clone();
        let subpage_id = graph
            .resolve_object(w, ObjectKind::Page, "SubPage")
            .unwrap()
            .id
            .clone();

        let result = infer_receiver_type(
            "currpage.lines.page",
            &routine,
            &[],
            &host_ext,
            &graph,
            &index,
            None,
            None,
        );
        assert_eq!(
            result,
            ReceiverType::Object {
                kind: ObjectKind::Page,
                name_lc: "subpage".into(),
                id: Some(subpage_id),
            }
        );
    }

    // -----------------------------------------------------------------------
    // infer_receiver_type — Step 0b: `CurrPage.<usercontrol>` ControlAddIn
    // receivers, closed-if-known gating (receiver-closure plan, Task 1)
    //
    // Fixture: workspace app `w` (depends on `Dep1`/`Dep2`, each declaring a
    // `controladdin "Shared.Addin"` — the cross-app AMBIGUOUS collision case)
    // with Page "AddinHost" (id 50320) with SIX usercontrol/systempart
    // controls:
    // - `Editor` -> "CDO.Editor" (source-declared IN `w`: `InitEditor(2
    //   params)` / `GetHTML(0 params)` — mirrors the real CDO.Editor
    //   controladdin exactly; an `OnSaveHTML` EVENT is deliberately absent
    //   from the declared-routine list here, modeling
    //   `al_syntax::lower::collect_routines`'s structural exclusion of
    //   `event_declaration` nodes — see the al-syntax lowering tests for
    //   proof events never become `RoutineDecl`s in the first place).
    // - `Broken` -> "Broken.Addin" (source-declared IN `w`, but its owning
    //   object's `parse_incomplete` is `true` — the Degraded case).
    // - `Shared` -> "Shared.Addin" (declared in BOTH `Dep1` AND `Dep2` — the
    //   Ambiguous case).
    // - `Viewer` -> `WebPageViewer` (no declaration anywhere — the
    //   TruePlatform allowlist case, grounded in the real CDO corpus).
    // - `Nope` -> "Totally.Unknown" (no declaration anywhere, NOT on the
    //   allowlist — genuinely-absent-and-non-platform).
    // - `SysPart` (SystemPart kind — OUT of the arm entirely, regardless of
    //   its target).
    // -----------------------------------------------------------------------

    fn build_control_addin_fixture() -> (ProgramGraph, AppRef) {
        let mut apps = crate::program::node::AppRegistry::default();
        let mk_id = |name: &str| crate::snapshot::AppId {
            guid: String::new(),
            name: name.into(),
            publisher: "Test".into(),
            version: "1.0.0.0".into(),
        };
        let w = apps.intern(&mk_id("ControlAddInW"));
        let dep1 = apps.intern(&mk_id("ControlAddInDep1"));
        let dep2 = apps.intern(&mk_id("ControlAddInDep2"));
        let mut topology = crate::program::topology::DependencyGraph::default();
        topology.add_dependency(w, dep1);
        topology.add_dependency(w, dep2);

        let mk_target = |raw: &str| ObjectRef::Name {
            raw: raw.to_string(),
            normalized_lc: raw.to_ascii_lowercase(),
        };

        let mut host = make_object_node(w, ObjectKind::Page, "AddinHost", Some(50320), None);
        host.page_controls = vec![
            PageControlNode {
                name_lc: "editor".into(),
                kind: PageControlKind::UserControl,
                target: mk_target("CDO.Editor"),
            },
            PageControlNode {
                name_lc: "broken".into(),
                kind: PageControlKind::UserControl,
                target: mk_target("Broken.Addin"),
            },
            PageControlNode {
                name_lc: "shared".into(),
                kind: PageControlKind::UserControl,
                target: mk_target("Shared.Addin"),
            },
            PageControlNode {
                name_lc: "viewer".into(),
                kind: PageControlKind::UserControl,
                target: mk_target("WebPageViewer"),
            },
            PageControlNode {
                name_lc: "nope".into(),
                kind: PageControlKind::UserControl,
                target: mk_target("Totally.Unknown"),
            },
            PageControlNode {
                name_lc: "syspart".into(),
                kind: PageControlKind::SystemPart,
                target: mk_target("Whatever"),
            },
        ];

        let editor = make_object_node(w, ObjectKind::ControlAddIn, "CDO.Editor", None, None);
        let mut broken = make_object_node(w, ObjectKind::ControlAddIn, "Broken.Addin", None, None);
        broken.parse_incomplete = true;
        let shared_dep1 =
            make_object_node(dep1, ObjectKind::ControlAddIn, "Shared.Addin", None, None);
        let shared_dep2 =
            make_object_node(dep2, ObjectKind::ControlAddIn, "Shared.Addin", None, None);

        let routines = vec![
            control_addin_routine(editor.id.clone(), "InitEditor", 2),
            control_addin_routine(editor.id.clone(), "GetHTML", 0),
            control_addin_routine(broken.id.clone(), "Foo", 0),
        ];

        let mut objects = vec![host, editor, broken, shared_dep1, shared_dep2];
        objects.sort_by(|a, b| a.id.cmp(&b.id));
        let obj_index = ObjectIndex::build(&objects);

        let graph = ProgramGraph {
            apps,
            topology,
            objects,
            routines,
            obj_index,
            ..Default::default()
        };
        (graph, w)
    }

    /// Minimal `RoutineNode` builder with a configurable arity — mirrors
    /// `make_routine_node` (below) but that helper hardcodes `params_count: 0`,
    /// which the ControlAddIn fixture's `InitEditor(2 params)` needs to defeat.
    fn control_addin_routine(obj_id: ObjectNodeId, name: &str, params_count: usize) -> RoutineNode {
        RoutineNode {
            id: RoutineNodeId {
                object: obj_id,
                name_lc: name.to_ascii_lowercase(),
                enclosing_member_lc: None,
                params_count,
                sig_fp: 0,
            },
            name: name.to_string(),
            is_trigger: false,
            access: Access::Public,
            tier: TrustTier::Workspace,
            event_subscribers: vec![],
            subscriber_instance_manual: false,
            publisher_kind: None,
            include_sender: None,
            abi_routine_kind: None,
            abi_event_kind: None,
            param_sig_key: String::new(),
            return_type: None,
            return_type_id: None,
            abi_overload_collapsed: false,
            source_overload_aliased: false,
            abi_params: AbiParams::Missing,
        }
    }

    fn control_addin_host(graph: &ProgramGraph) -> ObjectNode {
        graph
            .objects
            .iter()
            .find(|o| o.name == "AddinHost")
            .unwrap()
            .clone()
    }

    /// POSITIVE — Resolved: a source-declared, cleanly-parsed usercontrol
    /// types as `ControlAddIn { surface: Declared { procedures } }`, carrying
    /// BOTH declared procedures with their real arity.
    #[test]
    fn infer_currpage_usercontrol_declared_resolves_control_addin_declared() {
        let (graph, _w) = build_control_addin_fixture();
        let index = ResolveIndex::build(&graph);
        let routine = build_test_routine();
        let host = control_addin_host(&graph);

        let result = infer_receiver_type(
            "currpage.editor",
            &routine,
            &[],
            &host,
            &graph,
            &index,
            None,
            None,
        );
        match result {
            ReceiverType::ControlAddIn { name_lc, surface } => {
                assert_eq!(name_lc, "cdo.editor");
                match surface {
                    ControlAddInSurface::Declared { mut procedures } => {
                        procedures.sort();
                        assert_eq!(
                            procedures,
                            vec![("gethtml".to_string(), 0), ("initeditor".to_string(), 2)]
                        );
                    }
                    other => panic!("expected Declared, got {other:?}"),
                }
            }
            other => panic!("expected ControlAddIn, got {other:?}"),
        }
    }

    /// NEGATIVE — Degraded: a source-declared usercontrol whose owning file
    /// did not parse cleanly declines to `Unknown` UNCONDITIONALLY, even
    /// though it resolved to exactly one object — a parse-recovered routine
    /// list cannot be trusted enough to prove either presence or absence.
    #[test]
    fn infer_currpage_usercontrol_degraded_declines_unknown() {
        let (graph, _w) = build_control_addin_fixture();
        let index = ResolveIndex::build(&graph);
        let routine = build_test_routine();
        let host = control_addin_host(&graph);

        assert_eq!(
            infer_receiver_type(
                "currpage.broken",
                &routine,
                &[],
                &host,
                &graph,
                &index,
                None,
                None
            ),
            ReceiverType::Unknown
        );
    }

    /// NEGATIVE — Ambiguous: two dependency apps in the closure both declare
    /// `controladdin "Shared.Addin"` — an unprovable cross-app collision,
    /// declines rather than guessing either one.
    #[test]
    fn infer_currpage_usercontrol_ambiguous_declines_unknown() {
        let (graph, _w) = build_control_addin_fixture();
        let index = ResolveIndex::build(&graph);
        let routine = build_test_routine();
        let host = control_addin_host(&graph);

        assert_eq!(
            infer_receiver_type(
                "currpage.shared",
                &routine,
                &[],
                &host,
                &graph,
                &index,
                None,
                None
            ),
            ReceiverType::Unknown
        );
    }

    /// POSITIVE — TruePlatform: `WebPageViewer` has no reachable declaration
    /// anywhere in the fixture, but IS on the platform allowlist — resolves
    /// to `ControlAddIn { surface: TruePlatform }` (Phase B open-accepts any
    /// method on this outcome, mirroring the pre-Task-1 policy scoped down to
    /// just this allowlist).
    #[test]
    fn infer_currpage_usercontrol_true_platform_resolves() {
        let (graph, _w) = build_control_addin_fixture();
        let index = ResolveIndex::build(&graph);
        let routine = build_test_routine();
        let host = control_addin_host(&graph);

        assert_eq!(
            infer_receiver_type(
                "currpage.viewer",
                &routine,
                &[],
                &host,
                &graph,
                &index,
                None,
                None
            ),
            ReceiverType::ControlAddIn {
                name_lc: "webpageviewer".into(),
                surface: ControlAddInSurface::TruePlatform,
            }
        );
    }

    /// NEGATIVE — genuinely unknown control name: not declared anywhere, and
    /// not on the platform allowlist either. Never guessed open.
    #[test]
    fn infer_currpage_usercontrol_unknown_nonplatform_name_declines_unknown() {
        let (graph, _w) = build_control_addin_fixture();
        let index = ResolveIndex::build(&graph);
        let routine = build_test_routine();
        let host = control_addin_host(&graph);

        assert_eq!(
            infer_receiver_type(
                "currpage.nope",
                &routine,
                &[],
                &host,
                &graph,
                &index,
                None,
                None
            ),
            ReceiverType::Unknown
        );
    }

    /// NEGATIVE — `SystemPart` is explicitly OUT of the Step 0b arm entirely,
    /// regardless of its target — no `controladdin` object backs a
    /// SystemPart, so there is nothing to gate against. Default-decline
    /// (dated note, Task 1): a closed SystemPart catalog is future work only
    /// if real call sites ever surface.
    #[test]
    fn infer_currpage_systempart_bare_form_declines_unknown() {
        let (graph, _w) = build_control_addin_fixture();
        let index = ResolveIndex::build(&graph);
        let routine = build_test_routine();
        let host = control_addin_host(&graph);

        assert_eq!(
            infer_receiver_type(
                "currpage.syspart",
                &routine,
                &[],
                &host,
                &graph,
                &index,
                None,
                None
            ),
            ReceiverType::Unknown
        );
    }

    // -----------------------------------------------------------------------
    // Direct-var `var X: ControlAddIn "Foo"` retrofit (Task 1, round-1
    // addendum): the SAME closed-if-known gate applies via
    // `classify_type_text` -> `parsed_type_to_receiver` -> `resolve_control_addin_receiver`.
    // -----------------------------------------------------------------------

    #[test]
    fn classify_controladdin_quoted_name() {
        assert_eq!(
            classify_type_text("ControlAddIn \"CDO.Editor\""),
            ParsedType::ControlAddIn {
                name: "cdo.editor".into()
            }
        );
    }

    /// POSITIVE — direct-var Resolved: `var X: ControlAddIn "CDO.Editor"`
    /// (unquoted variable name aside — only the TYPE TEXT matters here)
    /// gates identically to the `CurrPage.<usercontrol>` path — same
    /// `Declared` surface, same procedures.
    #[test]
    fn direct_var_controladdin_declared_resolves_control_addin_declared() {
        let (graph, _w) = build_control_addin_fixture();
        let index = ResolveIndex::build(&graph);
        let host = control_addin_host(&graph);

        let parsed = classify_type_text("ControlAddIn \"CDO.Editor\"");
        let result = parsed_type_to_receiver(parsed, &host, &graph, &index);
        match result {
            ReceiverType::ControlAddIn { name_lc, surface } => {
                assert_eq!(name_lc, "cdo.editor");
                assert!(matches!(surface, ControlAddInSurface::Declared { .. }));
            }
            other => panic!("expected ControlAddIn, got {other:?}"),
        }
    }

    /// NEGATIVE, end-to-end — the direct-var retrofit's actual point: a
    /// typo'd method call on `var X: ControlAddIn "CDO.Editor"` must decline,
    /// not silently open-accept the way the pre-Task-1 `FrameworkKind::ControlAddIn`
    /// blanket policy would have. Chains the FULL pipeline this fixture
    /// proves each stage of individually: `classify_type_text` ->
    /// `parsed_type_to_receiver` -> `resolve_member`.
    #[test]
    fn direct_var_controladdin_typo_end_to_end_declines_unknown() {
        use crate::program::resolve::edge::{Evidence, RouteTarget, UnknownReason};
        use crate::program::resolve::resolver::resolve_member;

        let (graph, _w) = build_control_addin_fixture();
        let index = ResolveIndex::build(&graph);
        let host = control_addin_host(&graph);
        let surface = crate::program::resolve::decl_surface::DeclSurface::build(&graph, &[]);

        let parsed = classify_type_text("ControlAddIn \"CDO.Editor\"");
        let receiver = parsed_type_to_receiver(parsed, &host, &graph, &index);

        let (_, routes) =
            resolve_member(&receiver, "inteditor", 2, &host, &graph, &index, &surface);
        assert_eq!(routes.len(), 1);
        assert_eq!(
            routes[0].evidence,
            Evidence::Unknown(UnknownReason::MemberNotFound)
        );
        assert!(matches!(routes[0].target, RouteTarget::Unresolved));
    }

    // -----------------------------------------------------------------------
    // infer_receiver_type — Task 2 enabling primitive: `receiver_expr` threading
    // -----------------------------------------------------------------------

    /// Task 2 invariant: `infer_receiver_type` ACCEPTS a real
    /// `Some((&AlFile, ExprId))` for a `Func().M()` call site (the structured
    /// receiver `ExprKind::Call{..}` a resolver could fetch via
    /// `file.ir.expr(id)`) and — since Steps 0-4 dispatch purely on
    /// `receiver_lc`, unchanged by this task, AND `bare_ctx` (Task 3's Step 5
    /// enabling primitive) is `None` here — still returns exactly what it
    /// returned before this parameter existed: `Unknown` (`"func()"` matches
    /// none of Steps 0-4, and Step 5 is a no-op without `bare_ctx`).
    /// Resolution-neutral by construction; see the `infer_call_result_*`
    /// tests below for Step 5's actual (Task 3) behavior with `bare_ctx`
    /// populated.
    #[test]
    fn infer_receiver_type_accepts_threaded_call_receiver_and_stays_neutral() {
        use crate::program::resolve::extract::{CalleeShape, extract_sites};

        let src = r#"
codeunit 50100 "C"
{
    procedure Run()
    begin
        Func().M();
    end;
    procedure Func(): Codeunit "C" begin end;
}
"#;
        let file = al_syntax::parse(src);
        let sites = extract_sites(&file, src, "C.al", &std::collections::HashSet::new());
        let member_site = sites
            .iter()
            .find(|s| matches!(&s.shape, CalleeShape::Member { method, .. } if method.eq_ignore_ascii_case("m")))
            .expect("Func().M() must classify as a Member call");
        let CalleeShape::Member {
            receiver_text,
            receiver,
            ..
        } = &member_site.shape
        else {
            unreachable!("filtered to Member above");
        };
        let receiver_id = receiver.expect("Member.receiver must be populated");

        let (graph, app) = build_test_graph();
        let index = ResolveIndex::build(&graph);
        let routine = build_test_routine();
        let from_obj = make_object_node(app, ObjectKind::Codeunit, "CallerCu", Some(999), None);

        let receiver_lc = receiver_text.to_ascii_lowercase();
        let result = infer_receiver_type(
            &receiver_lc,
            &routine,
            &[],
            &from_obj,
            &graph,
            &index,
            Some((&file, receiver_id)),
            None,
        );
        assert_eq!(
            result,
            ReceiverType::Unknown,
            "threading a real receiver_expr must not change Task 2's resolution outcome"
        );
    }

    // -----------------------------------------------------------------------
    // infer_receiver_type — Step 6 (beyond-1B.3b Task 4): compound framework
    // property/method chains + `this.<rest>`.
    // -----------------------------------------------------------------------

    fn routine_with_locals(locals: Vec<VarDecl>) -> RoutineDecl {
        let o = test_origin();
        RoutineDecl {
            kind: RoutineKind::Procedure,
            name: "Run".into(),
            name_origin: o.clone(),
            params: vec![],
            return_type: None,
            return_name: None,
            locals,
            attributes: vec![],
            attributes_parsed: vec![],
            access_modifier: None,
            parse_incomplete: false,
            dataitem_source_table: None,
            enclosing_member: None,
            in_dataset_modify_context: false,
            body: None,
            origin: o,
        }
    }

    fn var_decl(name: &str, ty: &str) -> VarDecl {
        VarDecl {
            name: name.into(),
            ty: Some(ty.into()),
            temporary: false,
            origin: test_origin(),
        }
    }

    /// Parse `src`, extract the sole `Member` call site whose method matches
    /// `method_lc`, and return `(AlFile, receiver_text, receiver ExprId)`.
    fn parse_member_site(src: &str, method_lc: &str) -> (al_syntax::ir::AlFile, String, ExprId) {
        use crate::program::resolve::extract::{CalleeShape, extract_sites};

        let file = al_syntax::parse(src);
        let sites = extract_sites(&file, src, "T.al", &std::collections::HashSet::new());
        let site = sites
            .iter()
            .find(|s| matches!(&s.shape, CalleeShape::Member { method, .. } if method.eq_ignore_ascii_case(method_lc)))
            .unwrap_or_else(|| panic!("no Member call site with method {method_lc:?} found"));
        let CalleeShape::Member {
            receiver_text,
            receiver,
            ..
        } = &site.shape
        else {
            unreachable!("filtered to Member above");
        };
        let receiver_id = receiver.expect("Member.receiver must be populated");
        (file, receiver_text.clone(), receiver_id)
    }

    /// POSITIVE: `Response.Content().ReadAs(Foo)` — `Response: HttpResponseMessage`
    /// → `Content()` (real AL zero-arg method, table-verified) → `HttpContent`,
    /// so the receiver of `.ReadAs(...)` types `Framework(HttpContent)`.
    #[test]
    fn framework_chain_http_response_content_resolves_to_http_content() {
        let src = r#"
codeunit 50100 "C"
{
    procedure Run()
    var
        Response: HttpResponseMessage;
        Foo: Text;
    begin
        Response.Content().ReadAs(Foo);
    end;
}
"#;
        let (file, receiver_text, receiver_id) = parse_member_site(src, "readas");
        assert_eq!(receiver_text.to_ascii_lowercase(), "response.content()");

        let (graph, app) = build_test_graph();
        let index = ResolveIndex::build(&graph);
        let routine = routine_with_locals(vec![
            var_decl("Response", "HttpResponseMessage"),
            var_decl("Foo", "Text"),
        ]);
        let from_obj = make_object_node(app, ObjectKind::Codeunit, "CallerCu", Some(999), None);

        let result = infer_receiver_type(
            &receiver_text.to_ascii_lowercase(),
            &routine,
            &[],
            &from_obj,
            &graph,
            &index,
            Some((&file, receiver_id)),
            None,
        );
        assert_eq!(result, ReceiverType::Framework(FrameworkKind::HttpContent));
    }

    /// POSITIVE: `JToken.AsObject().Get('key', X)` — `JToken: JsonToken` →
    /// `AsObject()` (table-verified) → `JsonObject`, so the receiver of
    /// `.Get(...)` types `Framework(JsonObject)`.
    #[test]
    fn framework_chain_jsontoken_asobject_resolves_to_json_object() {
        let src = r#"
codeunit 50100 "C"
{
    procedure Run()
    var
        JToken: JsonToken;
        X: JsonToken;
    begin
        JToken.AsObject().Get('key', X);
    end;
}
"#;
        let (file, receiver_text, receiver_id) = parse_member_site(src, "get");
        assert_eq!(receiver_text.to_ascii_lowercase(), "jtoken.asobject()");

        let (graph, app) = build_test_graph();
        let index = ResolveIndex::build(&graph);
        let routine = routine_with_locals(vec![
            var_decl("JToken", "JsonToken"),
            var_decl("X", "JsonToken"),
        ]);
        let from_obj = make_object_node(app, ObjectKind::Codeunit, "CallerCu", Some(999), None);

        let result = infer_receiver_type(
            &receiver_text.to_ascii_lowercase(),
            &routine,
            &[],
            &from_obj,
            &graph,
            &index,
            Some((&file, receiver_id)),
            None,
        );
        assert_eq!(result, ReceiverType::Framework(FrameworkKind::JsonObject));
    }

    /// POSITIVE: `this.DialogWindow.Open()` — `this`-strip resolves
    /// `DialogWindow` against the object-GLOBALS-only scope (`Dialog` global),
    /// so the receiver of `.Open()` types `Framework(Dialog)`.
    #[test]
    fn this_strip_dialogwindow_resolves_to_dialog() {
        let src = r#"
codeunit 50100 "C"
{
    procedure Run()
    begin
        this.DialogWindow.Open();
    end;
}
"#;
        let (file, receiver_text, receiver_id) = parse_member_site(src, "open");
        assert_eq!(receiver_text.to_ascii_lowercase(), "this.dialogwindow");

        let (graph, app) = build_test_graph();
        let index = ResolveIndex::build(&graph);
        let routine = routine_with_locals(vec![]);
        let object_globals = vec![var_decl("DialogWindow", "Dialog")];
        let from_obj = make_object_node(app, ObjectKind::Codeunit, "CallerCu", Some(999), None);

        let result = infer_receiver_type(
            &receiver_text.to_ascii_lowercase(),
            &routine,
            &object_globals,
            &from_obj,
            &graph,
            &index,
            Some((&file, receiver_id)),
            None,
        );
        assert_eq!(result, ReceiverType::Framework(FrameworkKind::Dialog));
    }

    /// NEGATIVE: `this.DialogWindow.Open()` where `DialogWindow` is a LOCAL
    /// variable (or param), never declared as an object global — `this.`
    /// deliberately does NOT see locals/params (only `object_globals`), so
    /// this must stay `Unknown`, never fall back to a local-shadow guess.
    #[test]
    fn this_strip_ignores_locals_and_params() {
        let src = r#"
codeunit 50100 "C"
{
    procedure Run()
    var
        DialogWindow: Dialog;
    begin
        this.DialogWindow.Open();
    end;
}
"#;
        let (file, receiver_text, receiver_id) = parse_member_site(src, "open");

        let (graph, app) = build_test_graph();
        let index = ResolveIndex::build(&graph);
        // `DialogWindow` declared as a LOCAL on `routine`, NOT in `object_globals`.
        let routine = routine_with_locals(vec![var_decl("DialogWindow", "Dialog")]);
        let from_obj = make_object_node(app, ObjectKind::Codeunit, "CallerCu", Some(999), None);

        let result = infer_receiver_type(
            &receiver_text.to_ascii_lowercase(),
            &routine,
            &[], // no object globals — DialogWindow is NOT a member of the object
            &from_obj,
            &graph,
            &index,
            Some((&file, receiver_id)),
            None,
        );
        assert_eq!(
            result,
            ReceiverType::Unknown,
            "this. must resolve only against object globals, never locals/params"
        );
    }

    /// NEGATIVE: `this.Method()` (a CALL form, not a property) — deliberately
    /// DEFERRED (module doc Step 6b); must decline even when `Method` IS a
    /// framework-typed global's zero-arg conversion, since this shape isn't
    /// distinguishing a global from a same-object PROCEDURE without
    /// `resolve_bare`-style lookup, which this step doesn't perform.
    #[test]
    fn this_strip_call_form_declines() {
        let src = r#"
codeunit 50100 "C"
{
    procedure Run()
    begin
        this.DialogWindow().Open();
    end;
}
"#;
        let (file, receiver_text, receiver_id) = parse_member_site(src, "open");

        let (graph, app) = build_test_graph();
        let index = ResolveIndex::build(&graph);
        let routine = routine_with_locals(vec![]);
        let object_globals = vec![var_decl("DialogWindow", "Dialog")];
        let from_obj = make_object_node(app, ObjectKind::Codeunit, "CallerCu", Some(999), None);

        let result = infer_receiver_type(
            &receiver_text.to_ascii_lowercase(),
            &routine,
            &object_globals,
            &from_obj,
            &graph,
            &index,
            Some((&file, receiver_id)),
            None,
        );
        assert_eq!(result, ReceiverType::Unknown);
    }

    /// NEGATIVE: base not a known framework type — `Foo.Content().ReadAs(X)`
    /// where `Foo` is not declared anywhere (unresolved identifier); the
    /// recursive base-typing declines, so the whole chain declines.
    #[test]
    fn framework_chain_unknown_base_declines() {
        let src = r#"
codeunit 50100 "C"
{
    procedure Run()
    var
        Foo2: Text;
    begin
        Foo.Content().ReadAs(Foo2);
    end;
}
"#;
        let (file, receiver_text, receiver_id) = parse_member_site(src, "readas");

        let (graph, app) = build_test_graph();
        let index = ResolveIndex::build(&graph);
        let routine = routine_with_locals(vec![var_decl("Foo2", "Text")]);
        let from_obj = make_object_node(app, ObjectKind::Codeunit, "CallerCu", Some(999), None);

        let result = infer_receiver_type(
            &receiver_text.to_ascii_lowercase(),
            &routine,
            &[],
            &from_obj,
            &graph,
            &index,
            Some((&file, receiver_id)),
            None,
        );
        assert_eq!(result, ReceiverType::Unknown);
    }

    /// NEGATIVE: prop/method not in the table (table-miss = fail-closed) —
    /// `Response.Foo().ReadAs(X)`: `Response` types `Framework(HttpResponseMessage)`
    /// but `"foo"` is not a table entry for that kind.
    #[test]
    fn framework_chain_table_miss_declines() {
        let src = r#"
codeunit 50100 "C"
{
    procedure Run()
    var
        Response: HttpResponseMessage;
        X: Text;
    begin
        Response.Foo().ReadAs(X);
    end;
}
"#;
        let (file, receiver_text, receiver_id) = parse_member_site(src, "readas");

        let (graph, app) = build_test_graph();
        let index = ResolveIndex::build(&graph);
        let routine = routine_with_locals(vec![
            var_decl("Response", "HttpResponseMessage"),
            var_decl("X", "Text"),
        ]);
        let from_obj = make_object_node(app, ObjectKind::Codeunit, "CallerCu", Some(999), None);

        let result = infer_receiver_type(
            &receiver_text.to_ascii_lowercase(),
            &routine,
            &[],
            &from_obj,
            &graph,
            &index,
            Some((&file, receiver_id)),
            None,
        );
        assert_eq!(result, ReceiverType::Unknown);
    }

    /// REBASELINE (receiver-closure plan v2.1 Task 2 — corrects a WRONG
    /// negative test): `Response.Content.ReadAs(X)` (parens-less, idiomatic
    /// AL — `Content` written as a property, `is_method: false`) MUST
    /// resolve exactly like the parens'd form
    /// (`framework_chain_http_response_content_resolves_to_http_content`
    /// above), because AL's parens are OPTIONAL on a zero-arg procedure call
    /// (the user's standing correction — see the
    /// al-parens-optional-procedure-calls memory; this is its THIRD
    /// recurrence in this codebase). This test PREVIOUSLY asserted the
    /// opposite (`ReceiverType::Unknown`) under the false premise "AL
    /// procedures ALWAYS require parens" — that premise was wrong, so the
    /// old assertion was wrong too; this is a correctness rebaseline, not a
    /// behavior regression (per this project's "correctness over
    /// compatibility" working principle).
    #[test]
    fn framework_chain_parens_less_property_form_resolves_to_method() {
        let src = r#"
codeunit 50100 "C"
{
    procedure Run()
    var
        Response: HttpResponseMessage;
        X: Text;
    begin
        Response.Content.ReadAs(X);
    end;
}
"#;
        let (file, receiver_text, receiver_id) = parse_member_site(src, "readas");
        assert_eq!(receiver_text.to_ascii_lowercase(), "response.content");

        let (graph, app) = build_test_graph();
        let index = ResolveIndex::build(&graph);
        let routine = routine_with_locals(vec![
            var_decl("Response", "HttpResponseMessage"),
            var_decl("X", "Text"),
        ]);
        let from_obj = make_object_node(app, ObjectKind::Codeunit, "CallerCu", Some(999), None);

        let result = infer_receiver_type(
            &receiver_text.to_ascii_lowercase(),
            &routine,
            &[],
            &from_obj,
            &graph,
            &index,
            Some((&file, receiver_id)),
            None,
        );
        assert_eq!(result, ReceiverType::Framework(FrameworkKind::HttpContent));
    }

    /// NEGATIVE (still holds post-Task-2): a genuinely-absent zero-arg
    /// member in PROPERTY form (no parens) still declines — the parens-less
    /// fallback tries the method row too, but a member that's in NEITHER
    /// form's table (`"foo"` is not a `HttpResponseMessage` entry at all)
    /// stays fail-closed, exactly like the call-form
    /// `framework_chain_table_miss_declines` above. Proves the fallback
    /// doesn't fabricate coverage — it only rescues a real table entry
    /// written without parens, never an absent one.
    #[test]
    fn framework_chain_parens_less_table_miss_still_declines() {
        let src = r#"
codeunit 50100 "C"
{
    procedure Run()
    var
        Response: HttpResponseMessage;
        X: Text;
    begin
        Response.Foo.ReadAs(X);
    end;
}
"#;
        let (file, receiver_text, receiver_id) = parse_member_site(src, "readas");
        assert_eq!(receiver_text.to_ascii_lowercase(), "response.foo");

        let (graph, app) = build_test_graph();
        let index = ResolveIndex::build(&graph);
        let routine = routine_with_locals(vec![
            var_decl("Response", "HttpResponseMessage"),
            var_decl("X", "Text"),
        ]);
        let from_obj = make_object_node(app, ObjectKind::Codeunit, "CallerCu", Some(999), None);

        let result = infer_receiver_type(
            &receiver_text.to_ascii_lowercase(),
            &routine,
            &[],
            &from_obj,
            &graph,
            &index,
            Some((&file, receiver_id)),
            None,
        );
        assert_eq!(result, ReceiverType::Unknown);
    }

    /// POSITIVE (Task 2, the 4 ErrorInfo sites): `ErrInfo.CustomDimensions.
    /// ContainsKey(K)` — `ErrInfo: ErrorInfo` -> `CustomDimensions` (bare,
    /// parens-less property form, resolved via the SAME parens-optional
    /// fallback as the HTTP case above) -> `Framework(Dictionary)`, so the
    /// receiver of `.ContainsKey(...)` types `Framework(Dictionary)`.
    #[test]
    fn framework_chain_errorinfo_customdimensions_parens_less_resolves_to_dictionary() {
        let src = r#"
codeunit 50100 "C"
{
    procedure Run()
    var
        ErrInfo: ErrorInfo;
        K: Text;
    begin
        ErrInfo.CustomDimensions.ContainsKey(K);
    end;
}
"#;
        let (file, receiver_text, receiver_id) = parse_member_site(src, "containskey");
        assert_eq!(
            receiver_text.to_ascii_lowercase(),
            "errinfo.customdimensions"
        );

        let (graph, app) = build_test_graph();
        let index = ResolveIndex::build(&graph);
        let routine = routine_with_locals(vec![
            var_decl("ErrInfo", "ErrorInfo"),
            var_decl("K", "Text"),
        ]);
        let from_obj = make_object_node(app, ObjectKind::Codeunit, "CallerCu", Some(999), None);

        let result = infer_receiver_type(
            &receiver_text.to_ascii_lowercase(),
            &routine,
            &[],
            &from_obj,
            &graph,
            &index,
            Some((&file, receiver_id)),
            None,
        );
        assert_eq!(result, ReceiverType::Framework(FrameworkKind::Dictionary));
    }

    /// POSITIVE: the explicit-parens form of the same chain
    /// (`ErrInfo.CustomDimensions().Get(K)`) resolves identically — one
    /// table key serves both AST shapes via the `is_method: true` direct
    /// path (unchanged, no fallback needed).
    #[test]
    fn framework_chain_errorinfo_customdimensions_explicit_parens_resolves_to_dictionary() {
        let src = r#"
codeunit 50100 "C"
{
    procedure Run()
    var
        ErrInfo: ErrorInfo;
        K: Text;
    begin
        ErrInfo.CustomDimensions().Get(K);
    end;
}
"#;
        let (file, receiver_text, receiver_id) = parse_member_site(src, "get");
        assert_eq!(
            receiver_text.to_ascii_lowercase(),
            "errinfo.customdimensions()"
        );

        let (graph, app) = build_test_graph();
        let index = ResolveIndex::build(&graph);
        let routine = routine_with_locals(vec![
            var_decl("ErrInfo", "ErrorInfo"),
            var_decl("K", "Text"),
        ]);
        let from_obj = make_object_node(app, ObjectKind::Codeunit, "CallerCu", Some(999), None);

        let result = infer_receiver_type(
            &receiver_text.to_ascii_lowercase(),
            &routine,
            &[],
            &from_obj,
            &graph,
            &index,
            Some((&file, receiver_id)),
            None,
        );
        assert_eq!(result, ReceiverType::Framework(FrameworkKind::Dictionary));
    }

    // -----------------------------------------------------------------------
    // zero_arg_aware_lookup — the context-sensitive boundary helper itself
    // (Task 2). Exercised directly with synthetic closures so every branch
    // (including the "both rows exist, conflicting kinds -> fail closed"
    // branch that is UNREACHABLE via the real tables today, per the Task 2
    // report's pre-flip audit) is proven, not just the real-table cases.
    // -----------------------------------------------------------------------

    /// A genuine `Call` (`is_method: true`) looks up directly — no fallback,
    /// even when a differently-keyed "property" entry exists (proves the
    /// `Call` path never consults the fallback branch at all).
    #[test]
    fn zero_arg_lookup_call_form_uses_direct_lookup_only() {
        let result = zero_arg_aware_lookup(true, 0, |is_method, arity| match (is_method, arity) {
            (true, 0) => Some("method"),
            (false, 0) => Some("property"),
            _ => None,
        });
        assert_eq!(result, Some("method"));
    }

    /// A bare `Member` (`is_method: false`) with ONLY a method-form row
    /// (the real-world case today — the parens-less-call rescue).
    #[test]
    fn zero_arg_lookup_bare_member_falls_back_to_method_row() {
        let result = zero_arg_aware_lookup(false, 0, |is_method, arity| match (is_method, arity) {
            (true, 0) => Some("method"),
            _ => None,
        });
        assert_eq!(result, Some("method"));
    }

    /// A bare `Member` with ONLY a property-form row resolves via the
    /// property row directly (no method row to fall back to).
    #[test]
    fn zero_arg_lookup_bare_member_uses_property_row_when_present() {
        let result = zero_arg_aware_lookup(false, 0, |is_method, arity| match (is_method, arity) {
            (false, 0) => Some("property"),
            _ => None,
        });
        assert_eq!(result, Some("property"));
    }

    /// Both a property row AND a method row exist with the SAME kind — no
    /// ambiguity, resolves (both readings agree).
    #[test]
    fn zero_arg_lookup_bare_member_both_rows_agree_resolves() {
        let result = zero_arg_aware_lookup(false, 0, |is_method, arity| match (is_method, arity) {
            (false, 0) => Some("same"),
            (true, 0) => Some("same"),
            _ => None,
        });
        assert_eq!(result, Some("same"));
    }

    /// Both a property row AND a method row exist with CONFLICTING kinds —
    /// fail-closed (`None`), never a guess. Unreachable via the real tables
    /// today (audited: zero `is_method: false` rows exist in
    /// `framework_return_kind`/`recordref_family_return_kind`/
    /// `enum_chain_return_kind`) — this proves the guard holds for when a
    /// FUTURE property row is added.
    #[test]
    fn zero_arg_lookup_bare_member_conflicting_rows_declines() {
        let result = zero_arg_aware_lookup(false, 0, |is_method, arity| match (is_method, arity) {
            (false, 0) => Some("property_kind"),
            (true, 0) => Some("method_kind"),
            _ => None,
        });
        assert_eq!(result, None);
    }

    /// Neither row exists — a genuine table miss, unaffected by the
    /// fallback machinery.
    #[test]
    fn zero_arg_lookup_bare_member_neither_row_declines() {
        let result: Option<&str> =
            zero_arg_aware_lookup(false, 0, |_is_method, _arity| None::<&str>);
        assert_eq!(result, None);
    }

    /// NEGATIVE: wrong ARITY — `Response.Content(X).ReadAs(Y)` (1 arg) never
    /// matches the table's arity-0 entry.
    #[test]
    fn framework_chain_wrong_arity_declines() {
        let src = r#"
codeunit 50100 "C"
{
    procedure Run()
    var
        Response: HttpResponseMessage;
        X: HttpContent;
        Y: Text;
    begin
        Response.Content(X).ReadAs(Y);
    end;
}
"#;
        let (file, receiver_text, receiver_id) = parse_member_site(src, "readas");
        assert_eq!(receiver_text.to_ascii_lowercase(), "response.content(x)");

        let (graph, app) = build_test_graph();
        let index = ResolveIndex::build(&graph);
        let routine = routine_with_locals(vec![
            var_decl("Response", "HttpResponseMessage"),
            var_decl("X", "HttpContent"),
            var_decl("Y", "Text"),
        ]);
        let from_obj = make_object_node(app, ObjectKind::Codeunit, "CallerCu", Some(999), None);

        let result = infer_receiver_type(
            &receiver_text.to_ascii_lowercase(),
            &routine,
            &[],
            &from_obj,
            &graph,
            &index,
            Some((&file, receiver_id)),
            None,
        );
        assert_eq!(result, ReceiverType::Unknown);
    }

    /// NEGATIVE: a base whose recursion mis-types — `Response.Bar().Content().ReadAs(X)`:
    /// `Response.Bar()` itself is a table-miss (declines to `Unknown`), so the
    /// OUTER `.Content()` hop's base is `Unknown` (not `Framework`), and the
    /// whole chain declines — proving a mis-typed intermediate hop propagates
    /// rather than silently resetting to some other guess.
    #[test]
    fn framework_chain_recursion_mistype_declines() {
        let src = r#"
codeunit 50100 "C"
{
    procedure Run()
    var
        Response: HttpResponseMessage;
        X: Text;
    begin
        Response.Bar().Content().ReadAs(X);
    end;
}
"#;
        let (file, receiver_text, receiver_id) = parse_member_site(src, "readas");
        assert_eq!(
            receiver_text.to_ascii_lowercase(),
            "response.bar().content()"
        );

        let (graph, app) = build_test_graph();
        let index = ResolveIndex::build(&graph);
        let routine = routine_with_locals(vec![
            var_decl("Response", "HttpResponseMessage"),
            var_decl("X", "Text"),
        ]);
        let from_obj = make_object_node(app, ObjectKind::Codeunit, "CallerCu", Some(999), None);

        let result = infer_receiver_type(
            &receiver_text.to_ascii_lowercase(),
            &routine,
            &[],
            &from_obj,
            &graph,
            &index,
            Some((&file, receiver_id)),
            None,
        );
        assert_eq!(result, ReceiverType::Unknown);
    }

    /// NEGATIVE: a same-named member on a NON-framework type must NOT hit the
    /// table — `Cust.Content().ReadAs(X)` where `Cust: Record Customer` types
    /// `Record{..}`, not `Framework`, so the table lookup never even engages
    /// (short-circuited by the `Framework(kind)`-only guard), even though
    /// `"content"` IS a valid table member for `HttpResponseMessage`.
    #[test]
    fn framework_chain_non_framework_base_never_hits_table() {
        let src = r#"
codeunit 50100 "C"
{
    procedure Run()
    var
        Cust: Record Customer;
        X: Text;
    begin
        Cust.Content().ReadAs(X);
    end;
}
"#;
        let (file, receiver_text, receiver_id) = parse_member_site(src, "readas");

        let (graph, app) = build_test_graph();
        let index = ResolveIndex::build(&graph);
        let routine = routine_with_locals(vec![
            var_decl("Cust", "Record Customer"),
            var_decl("X", "Text"),
        ]);
        let from_obj = make_object_node(app, ObjectKind::Codeunit, "CallerCu", Some(999), None);

        let result = infer_receiver_type(
            &receiver_text.to_ascii_lowercase(),
            &routine,
            &[],
            &from_obj,
            &graph,
            &index,
            Some((&file, receiver_id)),
            None,
        );
        assert_eq!(result, ReceiverType::Unknown);
    }

    /// REGRESSION (round-2, found via the CDO gate's EXHAUSTIVE adjudication —
    /// see `.superpowers/sdd/task-4-report.md`): a QUOTED identifier whose
    /// UNQUOTED text merely STARTS WITH a framework keyword word must NOT
    /// collide with that framework type via Step 4's naive first-whitespace-
    /// token match. Real CDO: Table "CDO File"'s OWN `Blob` field
    /// `"File Blob"`, accessed bare (implicit Rec, inside the table's own
    /// procedure) as `"File Blob".CreateInStream(...)`, was FALSELY typed
    /// `Framework(File)` (unquoting "File Blob" → "file blob" → Step 4 matches
    /// the leading "file" token) and false-resolved `.CreateInStream`/
    /// `.CreateOutStream` to the `File` catalog instead of staying the honest
    /// `Unknown` a Blob FIELD reference correctly is — the quote-parity guard
    /// in `infer_receiver_type_for_expr` (re-quoting a `QuotedIdentifier`
    /// before recursing) fixes this by restoring byte-for-byte parity with
    /// the top-level `receiver_lc`'s (quoted, hence non-matching) string.
    #[test]
    fn quoted_identifier_never_collides_with_framework_keyword_via_recursion() {
        let src = r#"
codeunit 50100 "C"
{
    procedure Run()
    var
        Foo: InStream;
    begin
        "File Blob".CreateInStream(Foo);
    end;
}
"#;
        let (file, receiver_text, receiver_id) = parse_member_site(src, "createinstream");
        assert_eq!(receiver_text.to_ascii_lowercase(), "\"file blob\"");

        let (graph, app) = build_test_graph();
        let index = ResolveIndex::build(&graph);
        let routine = routine_with_locals(vec![var_decl("Foo", "InStream")]);
        let from_obj = make_object_node(app, ObjectKind::Codeunit, "CallerCu", Some(999), None);

        let result = infer_receiver_type(
            &receiver_text.to_ascii_lowercase(),
            &routine,
            &[], // "File Blob" is NOT a declared var/param/global (mirrors a table field)
            &from_obj,
            &graph,
            &index,
            Some((&file, receiver_id)),
            None,
        );
        assert_eq!(
            result,
            ReceiverType::Unknown,
            "a quoted name that merely unquotes to text starting with a framework \
             keyword (\"File Blob\" -> \"file blob\" -> \"File\") must never be \
             mis-typed as that framework type"
        );
    }

    /// NEGATIVE (field genuinely absent — was DEFERRED pre-Task-3, now a
    /// real field-lookup miss): `Rec.BlobField.CreateOutStream()` stays
    /// `Unknown`. `Rec` types `Record{table: Some(Customer)}`, so the
    /// record-field arm (record-field chains plan Task 3) DOES engage here —
    /// but `build_test_graph`'s synthetic "Customer" table declares zero
    /// fields, so `ResolveIndex::field_in_table` genuinely finds no
    /// `"blobfield"` and the arm falls through to `Unknown`, same outcome as
    /// before Task 3 landed (then for a different reason — the mechanism was
    /// unimplemented; now — the field doesn't exist). See
    /// `framework_chain_record_field_populated_resolves_to_catalog` below for
    /// the POSITIVE sibling proving the arm resolves once a real field exists.
    #[test]
    fn framework_chain_record_field_absent_stays_unknown() {
        let src = r#"
codeunit 50100 "C"
{
    procedure Run()
    var
        Rec: Record Customer;
    begin
        Rec.BlobField.CreateOutStream();
    end;
}
"#;
        let (file, receiver_text, receiver_id) = parse_member_site(src, "createoutstream");
        assert_eq!(receiver_text.to_ascii_lowercase(), "rec.blobfield");

        let (graph, app) = build_test_graph();
        let index = ResolveIndex::build(&graph);
        let routine = routine_with_locals(vec![var_decl("Rec", "Record Customer")]);
        let from_obj = make_object_node(app, ObjectKind::Codeunit, "CallerCu", Some(999), None);

        let result = infer_receiver_type(
            &receiver_text.to_ascii_lowercase(),
            &routine,
            &[],
            &from_obj,
            &graph,
            &index,
            Some((&file, receiver_id)),
            None,
        );
        assert_eq!(result, ReceiverType::Unknown);
    }

    /// POSITIVE (Task 3, record-field chains plan): the SAME shape as the
    /// negative above, except `Customer` now genuinely declares a `Blob`
    /// field named `"BlobField"` — `Rec.BlobField` must type
    /// `Framework(Blob)` (`classify_type_text` on the field's declared type
    /// text → `parsed_type_to_receiver`), unaffected by the member name
    /// being written unquoted here (quoted vs unquoted is exercised
    /// end-to-end by `tests/r0-corpus/ws-record-field-chain`).
    #[test]
    fn framework_chain_record_field_populated_resolves_framework_blob() {
        let src = r#"
codeunit 50100 "C"
{
    procedure Run()
    var
        Rec: Record Customer;
    begin
        Rec.BlobField.CreateOutStream();
    end;
}
"#;
        let (file, receiver_text, receiver_id) = parse_member_site(src, "createoutstream");

        let (mut graph, app) = build_test_graph();
        let customer_idx = graph
            .objects
            .iter()
            .position(|o| o.name == "Customer")
            .expect("Customer table must exist in build_test_graph");
        graph.objects[customer_idx].fields.push(FieldNode {
            name_lc: "blobfield".to_string(),
            type_text: "Blob".to_string(),
        });
        let index = ResolveIndex::build(&graph);
        let routine = routine_with_locals(vec![var_decl("Rec", "Record Customer")]);
        let from_obj = make_object_node(app, ObjectKind::Codeunit, "CallerCu", Some(999), None);

        let result = infer_receiver_type(
            &receiver_text.to_ascii_lowercase(),
            &routine,
            &[],
            &from_obj,
            &graph,
            &index,
            Some((&file, receiver_id)),
            None,
        );
        assert_eq!(result, ReceiverType::Framework(FrameworkKind::Blob));
    }

    /// POSITIVE (Task 3, record-field chains plan): the MULTI-LEVEL chain —
    /// `Rec."Doc Status".Ordinals().Count()`. `"Doc Status"` is an `Enum "DS"`
    /// field → the record-field arm types it `EnumType{name_lc: "ds"}`;
    /// `.Ordinals()` on that base is the NEW `enum_chain_return_kind` arm →
    /// `Framework(List)` — proving the two new arms compose (field arm feeds
    /// the enum-chain-base arm one hop up), exactly the real CDO shape
    /// (`Codeunit 6175455 "CDO E-Seal Setup Wizard"`,
    /// `Rec."eSeal Service".Ordinals().Count()`).
    #[test]
    fn framework_chain_enum_field_ordinals_resolves_framework_list() {
        let src = r#"
codeunit 50100 "C"
{
    procedure Run()
    var
        Rec: Record Customer;
        N: Integer;
    begin
        N := Rec."Doc Status".Ordinals().Count();
    end;
}
"#;
        let (file, receiver_text, receiver_id) = parse_member_site(src, "count");
        assert_eq!(
            receiver_text.to_ascii_lowercase(),
            "rec.\"doc status\".ordinals()"
        );

        let (mut graph, app) = build_test_graph();
        let customer_idx = graph
            .objects
            .iter()
            .position(|o| o.name == "Customer")
            .expect("Customer table must exist in build_test_graph");
        graph.objects[customer_idx].fields.push(FieldNode {
            name_lc: "doc status".to_string(),
            type_text: "Enum \"DS\"".to_string(),
        });
        let index = ResolveIndex::build(&graph);
        let routine = routine_with_locals(vec![
            var_decl("Rec", "Record Customer"),
            var_decl("N", "Integer"),
        ]);
        let from_obj = make_object_node(app, ObjectKind::Codeunit, "CallerCu", Some(999), None);

        let result = infer_receiver_type(
            &receiver_text.to_ascii_lowercase(),
            &routine,
            &[],
            &from_obj,
            &graph,
            &index,
            Some((&file, receiver_id)),
            None,
        );
        assert_eq!(result, ReceiverType::Framework(FrameworkKind::List));
    }

    // -----------------------------------------------------------------------
    // Task 4 (receiver-closure-and-arg-increments plan): enum-shape receivers
    // — sites (D)/(F)/(G).
    // -----------------------------------------------------------------------

    /// Extends `build_test_graph`'s graph with extra objects (Task 4's
    /// enum-shape fixtures need a real `Enum` object, and sometimes a
    /// same-name collision object, the base fixture doesn't carry) —
    /// re-sorts + rebuilds `obj_index` so the additions are visible to every
    /// graph-based lookup exactly like the base three.
    fn build_test_graph_with(extra: Vec<ObjectNode>) -> (ProgramGraph, AppRef) {
        let (mut graph, app) = build_test_graph();
        graph.objects.extend(extra);
        graph.objects.sort_by(|a, b| a.id.cmp(&b.id));
        graph.obj_index = ObjectIndex::build(&graph.objects);
        (graph, app)
    }

    /// POSITIVE, site (D) shape: `EMailLog."Linked to Table"::Customer.AsInteger()`
    /// — an enum-VALUE-literal chain (`ExprKind::QualifiedEnum` whose
    /// `enum_type` is the field-access `EMailLog."Linked to Table"`, NOT the
    /// literal `Enum` keyword). By grammar construction this is always
    /// enum-VALUE-typed, regardless of whether the field's declared type can
    /// be further resolved — `infer_receiver_type_for_expr`'s new
    /// `QualifiedEnum` arm returns the VALUE-instance `EnumType` unconditionally
    /// for this shape.
    #[test]
    fn qualified_enum_value_literal_chain_resolves_enum_type() {
        let src = r#"
codeunit 50100 "C"
{
    procedure Run()
    var
        EMailLog: Record Customer;
        N: Integer;
    begin
        N := EMailLog."Linked to Table"::Customer.AsInteger();
    end;
}
"#;
        let (file, receiver_text, receiver_id) = parse_member_site(src, "asinteger");
        assert_eq!(
            receiver_text.to_ascii_lowercase(),
            "emaillog.\"linked to table\"::customer"
        );

        let (mut graph, app) = build_test_graph();
        let customer_idx = graph
            .objects
            .iter()
            .position(|o| o.name == "Customer")
            .expect("Customer table must exist in build_test_graph");
        graph.objects[customer_idx].fields.push(FieldNode {
            name_lc: "linked to table".to_string(),
            type_text: "Enum \"Linked To Table Type\"".to_string(),
        });
        let index = ResolveIndex::build(&graph);
        let routine = routine_with_locals(vec![
            var_decl("EMailLog", "Record Customer"),
            var_decl("N", "Integer"),
        ]);
        let from_obj = make_object_node(app, ObjectKind::Codeunit, "CallerCu", Some(999), None);

        let result = infer_receiver_type(
            &receiver_text.to_ascii_lowercase(),
            &routine,
            &[],
            &from_obj,
            &graph,
            &index,
            Some((&file, receiver_id)),
            None,
        );
        assert_eq!(
            result,
            ReceiverType::EnumType {
                name_lc: String::new()
            }
        );
    }

    /// NEGATIVE (Task 4 review fix): `EMailLog."Legacy Status"::Open.AsInteger()`
    /// where `"Legacy Status"` is an **Option**-typed field, NOT an Enum —
    /// common legacy AL. Pre-fix, the `QualifiedEnum` arm accepted ANY
    /// non-`Enum::"Type"` qualified-value base as enum-VALUE-typed
    /// unconditionally (the review-flagged soundness gap: the grammar's
    /// `qualified_enum_value.enum_type` field is not itself constrained to
    /// Enum-typed bases, so an Option-qualified value reaches the identical
    /// branch a genuine Enum field does). The fix recurses
    /// `infer_receiver_type_for_expr` on the `enum_type` base and requires an
    /// ACTUAL Enum-shaped result (`EnumType`/`EnumTypeStatic`) before
    /// accepting VALUE-instance dispatch — an Option-typed field classifies
    /// `Primitive` (`classify_type_text`'s catch-all for unrecognized
    /// leading tokens), which is neither, so this declines to `Unknown`
    /// rather than guessing.
    #[test]
    fn qualified_enum_value_option_field_base_declines() {
        let src = r#"
codeunit 50100 "C"
{
    procedure Run()
    var
        EMailLog: Record Customer;
        N: Integer;
    begin
        N := EMailLog."Legacy Status"::Open.AsInteger();
    end;
}
"#;
        let (file, receiver_text, receiver_id) = parse_member_site(src, "asinteger");
        assert_eq!(
            receiver_text.to_ascii_lowercase(),
            "emaillog.\"legacy status\"::open"
        );

        let (mut graph, app) = build_test_graph();
        let customer_idx = graph
            .objects
            .iter()
            .position(|o| o.name == "Customer")
            .expect("Customer table must exist in build_test_graph");
        graph.objects[customer_idx].fields.push(FieldNode {
            name_lc: "legacy status".to_string(),
            type_text: "Option Open,Closed".to_string(),
        });
        let index = ResolveIndex::build(&graph);
        let routine = routine_with_locals(vec![
            var_decl("EMailLog", "Record Customer"),
            var_decl("N", "Integer"),
        ]);
        let from_obj = make_object_node(app, ObjectKind::Codeunit, "CallerCu", Some(999), None);

        let result = infer_receiver_type(
            &receiver_text.to_ascii_lowercase(),
            &routine,
            &[],
            &from_obj,
            &graph,
            &index,
            Some((&file, receiver_id)),
            None,
        );
        assert_eq!(result, ReceiverType::Unknown);
    }

    /// NEGATIVE (Task 4 review fix): `EMailLog."No Such Field"::Open.AsInteger()`
    /// where `"No Such Field"` does not exist on the base table (`Customer`)
    /// at all — the `enum_type` base itself fails to type at all (falls
    /// through `infer_compound_member_receiver`'s record-field arm, no field
    /// match, to `Unknown`), so the fix's Enum-shaped-result check correctly
    /// declines too, rather than the pre-fix behavior of blindly trusting
    /// ANY non-keyword qualified-value base regardless of whether its base
    /// even resolves to a real field/type.
    #[test]
    fn qualified_enum_value_unresolvable_field_base_declines() {
        let src = r#"
codeunit 50100 "C"
{
    procedure Run()
    var
        EMailLog: Record Customer;
        N: Integer;
    begin
        N := EMailLog."No Such Field"::Open.AsInteger();
    end;
}
"#;
        let (file, receiver_text, receiver_id) = parse_member_site(src, "asinteger");
        assert_eq!(
            receiver_text.to_ascii_lowercase(),
            "emaillog.\"no such field\"::open"
        );

        let (graph, app) = build_test_graph();
        let index = ResolveIndex::build(&graph);
        let routine = routine_with_locals(vec![
            var_decl("EMailLog", "Record Customer"),
            var_decl("N", "Integer"),
        ]);
        let from_obj = make_object_node(app, ObjectKind::Codeunit, "CallerCu", Some(999), None);

        let result = infer_receiver_type(
            &receiver_text.to_ascii_lowercase(),
            &routine,
            &[],
            &from_obj,
            &graph,
            &index,
            Some((&file, receiver_id)),
            None,
        );
        assert_eq!(result, ReceiverType::Unknown);
    }

    /// POSITIVE, site (F) shape: `Enum::"CDO Module Type".Ordinals()` — the
    /// `Enum::"Type"` TYPE-reference receiver, existence-checked against a
    /// real `Enum` object in the graph.
    #[test]
    fn qualified_enum_type_reference_resolves_enum_type_static() {
        let src = r#"
codeunit 50100 "C"
{
    procedure Run()
    var
        N: Integer;
    begin
        N := Enum::"CDO Module Type".Ordinals().Count();
    end;
}
"#;
        let (file, receiver_text, receiver_id) = parse_member_site(src, "ordinals");
        assert_eq!(
            receiver_text.to_ascii_lowercase(),
            "enum::\"cdo module type\""
        );

        let (graph, app) = build_test_graph_with(vec![make_object_node(
            AppRef(0),
            ObjectKind::Enum,
            "CDO Module Type",
            None,
            None,
        )]);
        let index = ResolveIndex::build(&graph);
        let routine = routine_with_locals(vec![var_decl("N", "Integer")]);
        let from_obj = make_object_node(app, ObjectKind::Codeunit, "CallerCu", Some(999), None);

        let result = infer_receiver_type(
            &receiver_text.to_ascii_lowercase(),
            &routine,
            &[],
            &from_obj,
            &graph,
            &index,
            Some((&file, receiver_id)),
            None,
        );
        assert_eq!(
            result,
            ReceiverType::EnumTypeStatic {
                name_lc: "cdo module type".to_string()
            }
        );
    }

    /// NEGATIVE, site (F) shape: `Enum::"No Such Enum".Ordinals()` where NO
    /// `Enum` object anywhere matches the name — the fail-closed existence
    /// check declines rather than trust the raw quoted string blind.
    #[test]
    fn qualified_enum_type_reference_unresolvable_enum_declines() {
        let src = r#"
codeunit 50100 "C"
{
    procedure Run()
    var
        N: Integer;
    begin
        N := Enum::"No Such Enum".Ordinals().Count();
    end;
}
"#;
        let (file, receiver_text, receiver_id) = parse_member_site(src, "ordinals");

        let (graph, app) = build_test_graph();
        let index = ResolveIndex::build(&graph);
        let routine = routine_with_locals(vec![var_decl("N", "Integer")]);
        let from_obj = make_object_node(app, ObjectKind::Codeunit, "CallerCu", Some(999), None);

        let result = infer_receiver_type(
            &receiver_text.to_ascii_lowercase(),
            &routine,
            &[],
            &from_obj,
            &graph,
            &index,
            Some((&file, receiver_id)),
            None,
        );
        assert_eq!(result, ReceiverType::Unknown);
    }

    /// POSITIVE, site (G) shape: a bare QUOTED enum-type-name receiver,
    /// `"CDO Send on Posting".FromInteger(...)` — unique Enum match, zero
    /// same-name objects of any other kind, no routine shadow. Reduces to a
    /// single atomic token, so this is tested directly through
    /// `infer_receiver_type` with no AST/`receiver_expr` needed at all.
    #[test]
    fn bare_quoted_enum_type_name_resolves_enum_type_static_when_unique_and_unshadowed() {
        let (graph, app) = build_test_graph_with(vec![make_object_node(
            AppRef(0),
            ObjectKind::Enum,
            "CDO Send on Posting",
            None,
            None,
        )]);
        let index = ResolveIndex::build(&graph);
        let routine = build_test_routine();
        let from_obj = make_object_node(app, ObjectKind::Codeunit, "CallerCu", Some(999), None);
        // Task 3 (roadmap-closure plan): Step 4b now gates on `bare_ctx`'s
        // `WithState` exactly like Step 3a — a realistic proven-no-`with`
        // context is required for the positive path (see
        // `step4b_declines_when_with_unproven`/`step4b_resolves_when_no_with_proven`
        // for the guard's own dedicated coverage).
        let surface = DeclSurface::build(&graph, &[]);

        let result = infer_receiver_type(
            "\"cdo send on posting\"",
            &routine,
            &[],
            &from_obj,
            &graph,
            &index,
            None,
            Some((&surface, WithState::NoWithProven)),
        );
        assert_eq!(
            result,
            ReceiverType::EnumTypeStatic {
                name_lc: "cdo send on posting".to_string()
            }
        );
    }

    /// POSITIVE, site (G) shape with an UNQUOTED bare name (no spaces) — the
    /// gate accepts either spelling identically.
    #[test]
    fn bare_unquoted_enum_type_name_resolves_enum_type_static() {
        let (graph, app) = build_test_graph_with(vec![make_object_node(
            AppRef(0),
            ObjectKind::Enum,
            "SendStatus",
            None,
            None,
        )]);
        let index = ResolveIndex::build(&graph);
        let routine = build_test_routine();
        let from_obj = make_object_node(app, ObjectKind::Codeunit, "CallerCu", Some(999), None);
        // Task 3 (roadmap-closure plan): see the sibling quoted-name test
        // above for why `bare_ctx` is now `Some(.., NoWithProven)` here.
        let surface = DeclSurface::build(&graph, &[]);

        let result = infer_receiver_type(
            "sendstatus",
            &routine,
            &[],
            &from_obj,
            &graph,
            &index,
            None,
            Some((&surface, WithState::NoWithProven)),
        );
        assert_eq!(
            result,
            ReceiverType::EnumTypeStatic {
                name_lc: "sendstatus".to_string()
            }
        );
    }

    /// NEGATIVE (collision rule): a same-named TABLE exists elsewhere in the
    /// whole object index — the programmatic collision rule
    /// (`same_normalized_name && kind != Enum`) declines even though the Enum
    /// itself resolves uniquely too. Proves the rule is whole-index, not
    /// closure-scoped or kind-hardcoded — the colliding Table lives in a
    /// DIFFERENT app that `from_object`'s app does not even depend on.
    #[test]
    fn bare_enum_type_name_collision_with_other_kind_declines() {
        let mut apps = crate::program::node::AppRegistry::default();
        let enum_app = apps.intern(&AppId {
            guid: String::new(),
            name: "EnumApp".into(),
            publisher: "Test".into(),
            version: "1.0.0.0".into(),
        });
        let stranger_app = apps.intern(&AppId {
            guid: String::new(),
            name: "StrangerApp".into(),
            publisher: "Test".into(),
            version: "1.0.0.0".into(),
        });
        let (mut graph, app) = build_test_graph_with(vec![make_object_node(
            enum_app,
            ObjectKind::Enum,
            "Ambiguous Name",
            None,
            None,
        )]);
        graph.apps = apps;
        graph.objects.push(make_object_node(
            stranger_app,
            ObjectKind::Table,
            "Ambiguous Name",
            None,
            None,
        ));
        graph.objects.sort_by(|a, b| a.id.cmp(&b.id));
        graph.obj_index = ObjectIndex::build(&graph.objects);
        let index = ResolveIndex::build(&graph);
        let routine = build_test_routine();
        let from_obj = make_object_node(app, ObjectKind::Codeunit, "CallerCu", Some(999), None);

        let result = infer_receiver_type(
            "\"ambiguous name\"",
            &routine,
            &[],
            &from_obj,
            &graph,
            &index,
            None,
            None,
        );
        assert_eq!(result, ReceiverType::Unknown);
    }

    /// NEGATIVE (routine shadow): the SAME name is ALSO a declared procedure
    /// on `from_object` — a parens-less bare call to that routine is exactly
    /// as syntactically plausible as the enum-type reading, so this must
    /// decline rather than guess.
    #[test]
    fn bare_enum_type_name_routine_shadow_declines() {
        let (mut graph, app) = build_test_graph_with(vec![make_object_node(
            AppRef(0),
            ObjectKind::Enum,
            "Send Status",
            None,
            None,
        )]);
        let from_obj = make_object_node(app, ObjectKind::Codeunit, "CallerCu", Some(999), None);
        graph
            .routines
            .push(make_routine_node(from_obj.id.clone(), "Send Status"));
        graph.routines.sort_by(|x, y| x.id.cmp(&y.id));
        let index = ResolveIndex::build(&graph);
        let routine = build_test_routine();

        let result = infer_receiver_type(
            "\"send status\"",
            &routine,
            &[],
            &from_obj,
            &graph,
            &index,
            None,
            None,
        );
        assert_eq!(result, ReceiverType::Unknown);
    }

    /// NEGATIVE (local shadow, defense-in-depth): a LOCAL var of the identical
    /// name already shadows via Step 2, long before Step 4b ever runs — proves
    /// the ordering holds end-to-end (not merely by code-review inspection).
    #[test]
    fn bare_enum_type_name_local_var_shadow_declines() {
        let (graph, app) = build_test_graph_with(vec![make_object_node(
            AppRef(0),
            ObjectKind::Enum,
            "MyStatus",
            None,
            None,
        )]);
        let index = ResolveIndex::build(&graph);
        let routine = routine_with_locals(vec![var_decl("MyStatus", "Integer")]);
        let from_obj = make_object_node(app, ObjectKind::Codeunit, "CallerCu", Some(999), None);

        let result = infer_receiver_type(
            "mystatus",
            &routine,
            &[],
            &from_obj,
            &graph,
            &index,
            None,
            None,
        );
        assert_eq!(result, ReceiverType::Primitive);
    }

    /// NEGATIVE (Task 3, roadmap-closure plan): Step 4b's with-guard —
    /// `WithState::InsideWith`/`Unknown` must decline a bare enum-type-name
    /// receiver, mirroring Step 3a's `step3a_page_declines_when_with_unproven`
    /// exactly. A bare name inside an un-modeled `with` block could actually
    /// mean a field of the with-target record rather than the enum
    /// type-static surface — the SAME false-`Source`-edge risk Step 3a
    /// guards against — so Step 4b must not silently prefer the enum
    /// reading whenever the `with` scope isn't proven empty.
    #[test]
    fn step4b_declines_when_with_unproven() {
        let (graph, app) = build_test_graph_with(vec![make_object_node(
            AppRef(0),
            ObjectKind::Enum,
            "CDO Send on Posting",
            None,
            None,
        )]);
        let index = ResolveIndex::build(&graph);
        let routine = build_test_routine();
        let from_obj = make_object_node(app, ObjectKind::Codeunit, "CallerCu", Some(999), None);
        let surface = DeclSurface::build(&graph, &[]);

        for ws in [WithState::InsideWith, WithState::Unknown] {
            let result = infer_receiver_type(
                "\"cdo send on posting\"",
                &routine,
                &[],
                &from_obj,
                &graph,
                &index,
                None,
                Some((&surface, ws)),
            );
            assert_eq!(
                result,
                ReceiverType::Unknown,
                "Step 4b must decline on a bare enum-type-name receiver under WithState {ws:?}"
            );
        }
    }

    /// POSITIVE (Task 3, roadmap-closure plan): `WithState::NoWithProven`
    /// leaves Step 4b's resolution untouched — the guard added above only
    /// excludes the two unproven states, it does not narrow the gate any
    /// further than Step 3a's own identical condition does.
    #[test]
    fn step4b_resolves_when_no_with_proven() {
        let (graph, app) = build_test_graph_with(vec![make_object_node(
            AppRef(0),
            ObjectKind::Enum,
            "CDO Send on Posting",
            None,
            None,
        )]);
        let index = ResolveIndex::build(&graph);
        let routine = build_test_routine();
        let from_obj = make_object_node(app, ObjectKind::Codeunit, "CallerCu", Some(999), None);
        let surface = DeclSurface::build(&graph, &[]);

        let result = infer_receiver_type(
            "\"cdo send on posting\"",
            &routine,
            &[],
            &from_obj,
            &graph,
            &index,
            None,
            Some((&surface, WithState::NoWithProven)),
        );
        assert_eq!(
            result,
            ReceiverType::EnumTypeStatic {
                name_lc: "cdo send on posting".to_string()
            }
        );
    }

    // -----------------------------------------------------------------------
    // Task 4 (record-field chains plan): a `RoutineNode` builder for the
    // routine-shadow guard tests below — mirrors `index.rs`'s own
    // `make_routine` test helper exactly (same field defaults).
    // -----------------------------------------------------------------------

    fn make_routine_node(obj_id: ObjectNodeId, name: &str) -> RoutineNode {
        RoutineNode {
            id: RoutineNodeId {
                object: obj_id,
                name_lc: name.to_ascii_lowercase(),
                enclosing_member_lc: None,
                params_count: 0,
                sig_fp: 0,
            },
            name: name.to_string(),
            is_trigger: false,
            access: Access::Public,
            tier: TrustTier::Workspace,
            event_subscribers: vec![],
            subscriber_instance_manual: false,
            publisher_kind: None,
            include_sender: None,
            abi_routine_kind: None,
            abi_event_kind: None,
            param_sig_key: String::new(),
            return_type: None,
            return_type_id: None,
            abi_overload_collapsed: false,
            source_overload_aliased: false,
            abi_params: AbiParams::Missing,
        }
    }

    // -----------------------------------------------------------------------
    // Task 4: Step 2 quote-parity fix (round-2 addendum) — a QUOTED
    // identifier naming a real local/param/global var must resolve AS THE
    // VAR, never fall through past Step 2.
    // -----------------------------------------------------------------------

    /// POSITIVE (c): a quoted RECORD VAR receiver with no colliding field.
    /// `"Sales Header Filter"` is NOT a made-up name — it IS a real Report
    /// dataitem in production CDO source (`Report 6175283 "CDO Update Output
    /// Profile"`, line 15: `dataitem("Sales Header Filter"; "Sales Header")`,
    /// referenced bare as `"Sales Header Filter".GetFilters()`/`.GetView()`
    /// at lines 507/426). This fixture reuses that name only to model the
    /// GENERIC mechanism under test (Step 2's quote-parity fix is
    /// receiver-name-agnostic and applies identically to any quoted
    /// `VarDecl`) — the object here is a Codeunit, not a Report, so Step 2b's
    /// dataitem-name lookup (dataitem-receivers plan, Task 1 — see
    /// `resolve_dataitem_source_table`'s tests below for THAT mechanism)
    /// never engages; this test exercises Step 2's var lookup only. Pre-fix,
    /// the raw quote-retaining `receiver_lc` never matched the unquoted
    /// `VarDecl` name and this fell through to `Unknown`
    /// (`UntrackedReceiver`).
    ///
    /// UPDATE (dataitem-receivers plan, Task 1): the real
    /// `"Sales Header Filter"` site this name is grounded in is now modeled
    /// end to end — see `tests/r0-corpus/ws-report-dataitem/` for the
    /// Report-object integration coverage (Step 2b resolves the dataitem
    /// NAME receiver directly; a local var of the identical name would still
    /// shadow it, exactly as Step 2's precedence over Step 2b requires).
    #[test]
    fn quote_parity_quoted_var_receiver_resolves_as_var() {
        let (graph, app) = build_test_graph();
        let customer_id = graph
            .objects
            .iter()
            .find(|o| o.name == "Customer")
            .unwrap()
            .id
            .clone();
        let index = ResolveIndex::build(&graph);
        // NOTE: `var_decl`'s name argument is used VERBATIM as `VarDecl.name`
        // (no unquoting — unlike the real lowerer's `ident_text`, which
        // strips AL quote characters at parse time), so this must be
        // written UNQUOTED to faithfully simulate what a real quoted
        // declaration `"Sales Header Filter": Record Customer` would
        // actually produce.
        let routine = routine_with_locals(vec![var_decl("Sales Header Filter", "Record Customer")]);
        let from_obj = make_object_node(app, ObjectKind::Codeunit, "CallerCu", Some(999), None);

        let result = infer_receiver_type(
            "\"sales header filter\"",
            &routine,
            &[],
            &from_obj,
            &graph,
            &index,
            None,
            None,
        );
        assert_eq!(
            result,
            ReceiverType::Record {
                table: Some(customer_id)
            },
            "a quoted identifier naming a real local var must resolve as the var"
        );
    }

    /// NEGATIVE / PRECEDENCE (d): a quoted name matching BOTH a local var
    /// AND a table field on the SAME object — the var MUST win (AL scoping:
    /// vars/params/globals always shadow a field). `from_object` is the
    /// Customer TABLE itself (Step 3a's field arm would otherwise engage),
    /// with a genuine `"File Blob"` Blob field declared — but a LOCAL var
    /// of the identical quoted name shadows it, so the result must be the
    /// var's declared type (`Record Customer`), never `Framework(Blob)`.
    #[test]
    fn quote_parity_var_and_field_same_quoted_name_var_wins() {
        let (mut graph, _app) = build_test_graph();
        let customer_idx = graph
            .objects
            .iter()
            .position(|o| o.name == "Customer")
            .unwrap();
        graph.objects[customer_idx].fields.push(FieldNode {
            name_lc: "file blob".to_string(),
            type_text: "Blob".to_string(),
        });
        let customer_id = graph.objects[customer_idx].id.clone();
        let index = ResolveIndex::build(&graph);
        // See the sibling test above for why `var_decl`'s argument here is
        // UNQUOTED.
        let routine = routine_with_locals(vec![var_decl("File Blob", "Record Customer")]);
        let from_obj = graph.objects[customer_idx].clone();
        let surface = DeclSurface::build(&graph, &[]);

        let result = infer_receiver_type(
            "\"file blob\"",
            &routine,
            &[],
            &from_obj,
            &graph,
            &index,
            None,
            Some((&surface, WithState::NoWithProven)),
        );
        assert_eq!(
            result,
            ReceiverType::Record {
                table: Some(customer_id)
            },
            "a var shadowing a same-named field must win — never the field, \
             even though Step 3a's Table-scope field lookup is fully wired here"
        );
    }

    // -----------------------------------------------------------------------
    // Task 4: Step 3a — bare implicit-Rec quoted-field receiver.
    // -----------------------------------------------------------------------

    /// POSITIVE (a): `"File Blob".CreateInStream(S)` inside a Table's own
    /// procedure — the implicit-Rec field types `Framework(Blob)`.
    #[test]
    fn step3a_bare_quoted_field_in_table_scope_resolves_blob() {
        let (mut graph, app) = build_test_graph();
        let customer_idx = graph
            .objects
            .iter()
            .position(|o| o.name == "Customer")
            .unwrap();
        graph.objects[customer_idx].fields.push(FieldNode {
            name_lc: "file blob".to_string(),
            type_text: "Blob".to_string(),
        });
        let index = ResolveIndex::build(&graph);
        let routine = routine_with_locals(vec![]);
        let from_obj = graph.objects[customer_idx].clone();
        let surface = DeclSurface::build(&graph, &[]);
        let _ = app;

        let result = infer_receiver_type(
            "\"file blob\"",
            &routine,
            &[],
            &from_obj,
            &graph,
            &index,
            None,
            Some((&surface, WithState::NoWithProven)),
        );
        assert_eq!(result, ReceiverType::Framework(FrameworkKind::Blob));
    }

    /// Task 1 review-fix regression guard: the SAME Step 3a shape as above,
    /// but the quoted field name carries an INTERIOR PAREN — mirrors the
    /// real CDO shape (Table 6175282 "CDO Update Output Profile Line",
    /// fields `"Request Page (XML)"` at rows :172/:179; also Table 6175284
    /// :900/:911, Table 6175307 :287/:298, and 2 sites in the `.dependencies`
    /// `CDOPageDefaultfilter.Table.al` :184/:193). The dataitem-receivers
    /// plan Task 1 centralized this step's quote guard into
    /// `is_atomic_receiver_token`, which (pre-fix) applied the UNQUOTED
    /// branch's `contains('(')` call-shape exclusion BEFORE the quote-parity
    /// check — so a well-formed quoted token containing a paren wrongly
    /// classified COMPOUND and this step never engaged, regressing these 8
    /// sites from `Catalog` (`Blob::createoutstream`/`createinstream`) to
    /// `Unknown(CompoundReceiver)`. Fixed: the quoted branch is now judged
    /// purely on quote-parity, so `"req page (xml)"` classifies atomic and
    /// this step's Blob-field lookup fires exactly as it did before Task 1.
    #[test]
    fn step3a_bare_quoted_field_with_interior_paren_resolves_blob() {
        let (mut graph, app) = build_test_graph();
        let customer_idx = graph
            .objects
            .iter()
            .position(|o| o.name == "Customer")
            .unwrap();
        graph.objects[customer_idx].fields.push(FieldNode {
            name_lc: "req page (xml)".to_string(),
            type_text: "Blob".to_string(),
        });
        let index = ResolveIndex::build(&graph);
        let routine = routine_with_locals(vec![]);
        let from_obj = graph.objects[customer_idx].clone();
        let surface = DeclSurface::build(&graph, &[]);
        let _ = app;

        let result = infer_receiver_type(
            "\"req page (xml)\"",
            &routine,
            &[],
            &from_obj,
            &graph,
            &index,
            None,
            Some((&surface, WithState::NoWithProven)),
        );
        assert_eq!(
            result,
            ReceiverType::Framework(FrameworkKind::Blob),
            "a quoted field name with an interior paren must still resolve via \
             Step 3a's Blob-field lookup, exactly like a paren-free quoted field name"
        );
    }

    /// POSITIVE (b): the SAME shape, inside a TableExtension's own procedure
    /// — resolves via the base+own field surface (`ResolveIndex::
    /// field_in_table`'s extension folding), for BOTH a field declared on
    /// the extension itself and one inherited from the base table.
    #[test]
    fn step3a_bare_quoted_field_in_tableextension_scope_resolves() {
        let (mut graph, app) = build_test_graph();
        let customer_idx = graph
            .objects
            .iter()
            .position(|o| o.name == "Customer")
            .unwrap();
        graph.objects[customer_idx].fields.push(FieldNode {
            name_lc: "base blob".to_string(),
            type_text: "Blob".to_string(),
        });
        let mut ext_obj = make_object_node(
            app,
            ObjectKind::TableExtension,
            "CustomerExt",
            Some(50200),
            Some("Customer".to_string()),
        );
        ext_obj.fields.push(FieldNode {
            name_lc: "ext note".to_string(),
            type_text: "Text[100]".to_string(),
        });
        graph.objects.push(ext_obj);
        graph.objects.sort_by(|a, b| a.id.cmp(&b.id));
        let index = ResolveIndex::build(&graph);
        let routine = routine_with_locals(vec![]);
        let from_obj = graph
            .objects
            .iter()
            .find(|o| o.name == "CustomerExt")
            .unwrap()
            .clone();
        let surface = DeclSurface::build(&graph, &[]);

        // The extension's OWN field.
        let result_own = infer_receiver_type(
            "\"ext note\"",
            &routine,
            &[],
            &from_obj,
            &graph,
            &index,
            None,
            Some((&surface, WithState::NoWithProven)),
        );
        assert_eq!(result_own, ReceiverType::Framework(FrameworkKind::Text));

        // The BASE table's field, folded into the extension's own scope.
        let result_base = infer_receiver_type(
            "\"base blob\"",
            &routine,
            &[],
            &from_obj,
            &graph,
            &index,
            None,
            Some((&surface, WithState::NoWithProven)),
        );
        assert_eq!(result_base, ReceiverType::Framework(FrameworkKind::Blob));
    }

    // -----------------------------------------------------------------------
    // pageext-merge-and-final-residual plan, Task 2: Step 3a widened to
    // Page/PageExtension via `resolver::implicit_rec_table_id` — the SAME
    // bare-field-chain machinery Table/TableExtension already had, now
    // reachable through a Page's/PageExtension's own `SourceTable`. Real
    // site: `"View (Blob)".CreateInStream(ReadStream)` in Page 6175411's own
    // procedure (`.dependencies/CDO/Page/CDOPageDefaultFilters.Page.al:88`),
    // `"View (Blob)"` = `field(28; ...; Blob)` on the page's SourceTable
    // (`CDOPageDefaultfilter.Table.al:35`).
    // -----------------------------------------------------------------------

    /// POSITIVE: a Page's own procedure references its SourceTable's Blob
    /// field by bare quoted name — the Site-A shape.
    #[test]
    fn step3a_bare_quoted_field_on_page_with_sourcetable_resolves_blob() {
        let (mut graph, w) = build_page_rec_fixture();
        let customer_idx = graph
            .objects
            .iter()
            .position(|o| o.name == "Customer")
            .unwrap();
        graph.objects[customer_idx].fields.push(FieldNode {
            name_lc: "view (blob)".to_string(),
            type_text: "Blob".to_string(),
        });
        let index = ResolveIndex::build(&graph);
        let routine = routine_with_locals(vec![]);
        let surface = DeclSurface::build(&graph, &[]);

        let mut page = make_object_node(
            w,
            ObjectKind::Page,
            "DefaultFiltersPage",
            Some(6175411),
            None,
        );
        page.source_table = Some(ObjectRef::Name {
            raw: "Customer".into(),
            normalized_lc: "customer".into(),
        });

        let result = infer_receiver_type(
            "\"view (blob)\"",
            &routine,
            &[],
            &page,
            &graph,
            &index,
            None,
            Some((&surface, WithState::NoWithProven)),
        );
        assert_eq!(result, ReceiverType::Framework(FrameworkKind::Blob));
    }

    /// POSITIVE: the SAME shape inside a PageExtension of a base Page that
    /// declares `SourceTable` — the base-lookup hop
    /// (`resolve_pageext_base_source_table`) is exercised, not just the
    /// direct Page case above.
    #[test]
    fn step3a_bare_quoted_field_on_pageextension_with_sourcetable_resolves_blob() {
        let (mut graph, w) = build_page_rec_fixture();
        let customer_idx = graph
            .objects
            .iter()
            .position(|o| o.name == "Customer")
            .unwrap();
        graph.objects[customer_idx].fields.push(FieldNode {
            name_lc: "view (blob)".to_string(),
            type_text: "Blob".to_string(),
        });
        let index = ResolveIndex::build(&graph);
        let routine = routine_with_locals(vec![]);
        let surface = DeclSurface::build(&graph, &[]);

        // `build_page_rec_fixture` already declares Page "CustomerPage" (id
        // 50200, `SourceTable = Customer`) in `graph.objects` — the base
        // this extension must resolve THROUGH (`graph.objects.iter().find`
        // inside `resolve_pageext_base_source_table`).
        let page_ext = make_object_node(
            w,
            ObjectKind::PageExtension,
            "CustomerPageExt",
            Some(50210),
            Some("CustomerPage".to_string()),
        );

        let result = infer_receiver_type(
            "\"view (blob)\"",
            &routine,
            &[],
            &page_ext,
            &graph,
            &index,
            None,
            Some((&surface, WithState::NoWithProven)),
        );
        assert_eq!(result, ReceiverType::Framework(FrameworkKind::Blob));
    }

    /// NEGATIVE: a Page WITHOUT a `SourceTable` declared has no implicit-Rec
    /// table to search at all — `implicit_rec_table_id` returns `None` and
    /// this step declines to `Unknown` rather than guess.
    #[test]
    fn step3a_page_without_sourcetable_declines() {
        let (graph, w) = build_page_rec_fixture();
        let index = ResolveIndex::build(&graph);
        let routine = routine_with_locals(vec![]);
        let surface = DeclSurface::build(&graph, &[]);

        let page = make_object_node(w, ObjectKind::Page, "NoSourcePage", Some(50299), None);

        let result = infer_receiver_type(
            "\"view (blob)\"",
            &routine,
            &[],
            &page,
            &graph,
            &index,
            None,
            Some((&surface, WithState::NoWithProven)),
        );
        assert_eq!(result, ReceiverType::Unknown);
    }

    /// NEGATIVE: the with-guard applies to the Page arm exactly like the
    /// Table arm — `InsideWith`/`Unknown` `WithState` must decline, never
    /// silently type a field inside an un-modeled `with` block.
    #[test]
    fn step3a_page_declines_when_with_unproven() {
        let (mut graph, w) = build_page_rec_fixture();
        let customer_idx = graph
            .objects
            .iter()
            .position(|o| o.name == "Customer")
            .unwrap();
        graph.objects[customer_idx].fields.push(FieldNode {
            name_lc: "view (blob)".to_string(),
            type_text: "Blob".to_string(),
        });
        let index = ResolveIndex::build(&graph);
        let routine = routine_with_locals(vec![]);
        let surface = DeclSurface::build(&graph, &[]);

        let mut page = make_object_node(
            w,
            ObjectKind::Page,
            "DefaultFiltersPage",
            Some(6175411),
            None,
        );
        page.source_table = Some(ObjectRef::Name {
            raw: "Customer".into(),
            normalized_lc: "customer".into(),
        });

        for ws in [WithState::InsideWith, WithState::Unknown] {
            let result = infer_receiver_type(
                "\"view (blob)\"",
                &routine,
                &[],
                &page,
                &graph,
                &index,
                None,
                Some((&surface, ws)),
            );
            assert_eq!(
                result,
                ReceiverType::Unknown,
                "Step 3a must decline on a Page under WithState {ws:?}"
            );
        }
    }

    /// NEGATIVE: the routine-shadow guard — the page's SOURCE TABLE ALSO
    /// declares a routine of the identical bare name (AL's parens-optional
    /// zero-arg call ambiguity) — must decline, mirroring the pre-existing
    /// Table-scope guard (`step3a_declines_when_same_named_routine_exists`).
    #[test]
    fn step3a_page_declines_when_sourcetable_routine_shadows_field() {
        let (mut graph, w) = build_page_rec_fixture();
        let customer_idx = graph
            .objects
            .iter()
            .position(|o| o.name == "Customer")
            .unwrap();
        graph.objects[customer_idx].fields.push(FieldNode {
            name_lc: "view (blob)".to_string(),
            type_text: "Blob".to_string(),
        });
        let customer_id = graph.objects[customer_idx].id.clone();
        graph
            .routines
            .push(make_routine_node(customer_id, "View (Blob)"));
        graph.routines.sort_by(|x, y| x.id.cmp(&y.id));
        let index = ResolveIndex::build(&graph);
        let routine = routine_with_locals(vec![]);
        let surface = DeclSurface::build(&graph, &[]);

        let mut page = make_object_node(
            w,
            ObjectKind::Page,
            "DefaultFiltersPage",
            Some(6175411),
            None,
        );
        page.source_table = Some(ObjectRef::Name {
            raw: "Customer".into(),
            normalized_lc: "customer".into(),
        });

        let result = infer_receiver_type(
            "\"view (blob)\"",
            &routine,
            &[],
            &page,
            &graph,
            &index,
            None,
            Some((&surface, WithState::NoWithProven)),
        );
        assert_eq!(
            result,
            ReceiverType::Unknown,
            "a same-named routine on the page's SourceTable must block field-typing"
        );
    }

    /// NEGATIVE (Task 2 self-shadow closer): the PAGE ITSELF — not its
    /// SourceTable — declares a routine of the identical bare name. Unlike
    /// Table/TableExtension (where `table_id == from_object.id`, so
    /// `table_scope_has_routine` incidentally covers this), a Page's
    /// SourceTable is a DIFFERENT object — this exercises the added
    /// `index.routines_in_object(&from_object.id, ..)` guard that closes
    /// that gap.
    #[test]
    fn step3a_page_declines_when_pages_own_routine_shadows_sourcetable_field() {
        let (mut graph, w) = build_page_rec_fixture();
        let customer_idx = graph
            .objects
            .iter()
            .position(|o| o.name == "Customer")
            .unwrap();
        graph.objects[customer_idx].fields.push(FieldNode {
            name_lc: "view (blob)".to_string(),
            type_text: "Blob".to_string(),
        });
        let mut page = make_object_node(
            w,
            ObjectKind::Page,
            "DefaultFiltersPage",
            Some(6175411),
            None,
        );
        page.source_table = Some(ObjectRef::Name {
            raw: "Customer".into(),
            normalized_lc: "customer".into(),
        });
        graph
            .routines
            .push(make_routine_node(page.id.clone(), "View (Blob)"));
        graph.routines.sort_by(|x, y| x.id.cmp(&y.id));
        let index = ResolveIndex::build(&graph);
        let routine = routine_with_locals(vec![]);
        let surface = DeclSurface::build(&graph, &[]);

        let result = infer_receiver_type(
            "\"view (blob)\"",
            &routine,
            &[],
            &page,
            &graph,
            &index,
            None,
            Some((&surface, WithState::NoWithProven)),
        );
        assert_eq!(
            result,
            ReceiverType::Unknown,
            "the PAGE's OWN routine of the identical name must block field-typing too, \
             not just the source table's own routine surface"
        );
    }

    /// NEGATIVE (e): a quoted-field-shaped receiver in a NON-Table/
    /// TableExtension/Page/PageExtension object (no implicit-Rec field
    /// surface reachable this way — Codeunit's `TableNo` is a different
    /// mechanism, out of this step's scope) must decline to `Unknown`, even
    /// with a fully-wired `bare_ctx`.
    #[test]
    fn step3a_non_table_scope_declines() {
        let (graph, app) = build_test_graph();
        let index = ResolveIndex::build(&graph);
        let routine = routine_with_locals(vec![]);
        let from_obj = make_object_node(app, ObjectKind::Codeunit, "CallerCu", Some(999), None);
        let surface = DeclSurface::build(&graph, &[]);

        let result = infer_receiver_type(
            "\"file blob\"",
            &routine,
            &[],
            &from_obj,
            &graph,
            &index,
            None,
            Some((&surface, WithState::NoWithProven)),
        );
        assert_eq!(result, ReceiverType::Unknown);
    }

    /// NEGATIVE (f): an unknown quoted name inside a Table's own procedure
    /// (no such field declared anywhere in scope) declines to `Unknown`.
    #[test]
    fn step3a_unknown_quoted_field_declines() {
        let (graph, app) = build_test_graph();
        let customer = graph
            .objects
            .iter()
            .find(|o| o.name == "Customer")
            .unwrap()
            .clone();
        let index = ResolveIndex::build(&graph);
        let routine = routine_with_locals(vec![]);
        let surface = DeclSurface::build(&graph, &[]);
        let _ = app;

        let result = infer_receiver_type(
            "\"no such field\"",
            &routine,
            &[],
            &customer,
            &graph,
            &index,
            None,
            Some((&surface, WithState::NoWithProven)),
        );
        assert_eq!(result, ReceiverType::Unknown);
    }

    /// `with_state` gating: Step 3a must NOT fire when the call site is
    /// inside an un-modeled `with` block (`InsideWith`/`Unknown`) — mirrors
    /// `resolve_bare`'s own Step 3 with-guard exactly.
    #[test]
    fn step3a_declines_inside_with() {
        let (mut graph, _app) = build_test_graph();
        let customer_idx = graph
            .objects
            .iter()
            .position(|o| o.name == "Customer")
            .unwrap();
        graph.objects[customer_idx].fields.push(FieldNode {
            name_lc: "file blob".to_string(),
            type_text: "Blob".to_string(),
        });
        let index = ResolveIndex::build(&graph);
        let routine = routine_with_locals(vec![]);
        let from_obj = graph.objects[customer_idx].clone();
        let surface = DeclSurface::build(&graph, &[]);

        for ws in [WithState::InsideWith, WithState::Unknown] {
            let result = infer_receiver_type(
                "\"file blob\"",
                &routine,
                &[],
                &from_obj,
                &graph,
                &index,
                None,
                Some((&surface, ws)),
            );
            assert_eq!(
                result,
                ReceiverType::Unknown,
                "Step 3a must decline under WithState {ws:?}"
            );
        }
    }

    /// `bare_ctx` gating: with no `bare_ctx` supplied (unit tests /
    /// `semantic_golden.rs` shape), Step 3a is a no-op — mirrors Step 5/6's
    /// identical `bare_ctx`-optionality contract.
    #[test]
    fn step3a_no_bare_ctx_is_noop() {
        let (mut graph, _app) = build_test_graph();
        let customer_idx = graph
            .objects
            .iter()
            .position(|o| o.name == "Customer")
            .unwrap();
        graph.objects[customer_idx].fields.push(FieldNode {
            name_lc: "file blob".to_string(),
            type_text: "Blob".to_string(),
        });
        let index = ResolveIndex::build(&graph);
        let routine = routine_with_locals(vec![]);
        let from_obj = graph.objects[customer_idx].clone();

        let result = infer_receiver_type(
            "\"file blob\"",
            &routine,
            &[],
            &from_obj,
            &graph,
            &index,
            None,
            None,
        );
        assert_eq!(result, ReceiverType::Unknown);
    }

    // -----------------------------------------------------------------------
    // Task 4 round-2 soundness correction: the routine-shadow guard
    // (`ResolveIndex::table_scope_has_routine`) — AL's parens are optional
    // on a zero-argument call, so a bare `Member` AST node is ambiguous
    // between a field access and a parens-less procedure call.
    // -----------------------------------------------------------------------

    /// Step 3a must decline (never type as a field) when a same-named
    /// ROUTINE exists anywhere in the visibility-scoped table surface —
    /// `"File Blob"` is BOTH a genuine `Blob` field AND a declared
    /// procedure on the same table; the ambiguity must fail closed.
    #[test]
    fn step3a_declines_when_same_named_routine_exists() {
        let (mut graph, _app) = build_test_graph();
        let customer_idx = graph
            .objects
            .iter()
            .position(|o| o.name == "Customer")
            .unwrap();
        graph.objects[customer_idx].fields.push(FieldNode {
            name_lc: "file blob".to_string(),
            type_text: "Blob".to_string(),
        });
        let customer_id = graph.objects[customer_idx].id.clone();
        // `make_routine_node`'s name arg mirrors `RoutineDecl.name` (already
        // unquoted by the real lowerer's `ident_text`) — UNQUOTED here too,
        // so `name_lc` genuinely matches `field_lc`'s unquoted lookup key.
        graph
            .routines
            .push(make_routine_node(customer_id, "File Blob"));
        graph.routines.sort_by(|x, y| x.id.cmp(&y.id));
        let index = ResolveIndex::build(&graph);
        let routine = routine_with_locals(vec![]);
        let from_obj = graph.objects[customer_idx].clone();
        let surface = DeclSurface::build(&graph, &[]);

        let result = infer_receiver_type(
            "\"file blob\"",
            &routine,
            &[],
            &from_obj,
            &graph,
            &index,
            None,
            Some((&surface, WithState::NoWithProven)),
        );
        assert_eq!(
            result,
            ReceiverType::Unknown,
            "a same-named routine anywhere in the table scope must block field-typing"
        );
    }

    /// The SAME guard, exercised on Task 3's `Rec."Field".X()` compound
    /// arm — the coordinator-required regression fixture: a table declares
    /// BOTH a field AND a procedure named `GetThing`; `Rec.GetThing` (a
    /// parens-less reference — `is_method: false`, structurally identical
    /// to a field access) must decline to `Unknown`, never mistyped as the
    /// field. The existing `framework_chain_record_field_populated_
    /// resolves_framework_blob` test is the CONTROL sibling (field only,
    /// no routine) proving the arm still resolves when there is no
    /// ambiguity.
    #[test]
    fn compound_record_field_arm_declines_when_same_named_routine_exists() {
        let src = r#"
codeunit 50100 "C"
{
    procedure Run()
    var
        Rec: Record Customer;
    begin
        Rec.GetThing.CreateOutStream();
    end;
}
"#;
        let (file, receiver_text, receiver_id) = parse_member_site(src, "createoutstream");
        assert_eq!(receiver_text.to_ascii_lowercase(), "rec.getthing");

        let (mut graph, app) = build_test_graph();
        let customer_idx = graph
            .objects
            .iter()
            .position(|o| o.name == "Customer")
            .unwrap();
        graph.objects[customer_idx].fields.push(FieldNode {
            name_lc: "getthing".to_string(),
            type_text: "Blob".to_string(),
        });
        let customer_id = graph.objects[customer_idx].id.clone();
        graph
            .routines
            .push(make_routine_node(customer_id, "GetThing"));
        graph.routines.sort_by(|x, y| x.id.cmp(&y.id));
        let index = ResolveIndex::build(&graph);
        let routine = routine_with_locals(vec![var_decl("Rec", "Record Customer")]);
        let from_obj = make_object_node(app, ObjectKind::Codeunit, "CallerCu", Some(999), None);

        let result = infer_receiver_type(
            &receiver_text.to_ascii_lowercase(),
            &routine,
            &[],
            &from_obj,
            &graph,
            &index,
            Some((&file, receiver_id)),
            None,
        );
        assert_eq!(
            result,
            ReceiverType::Unknown,
            "Rec.GetThing is a parens-less call to a same-named routine, never the field"
        );
    }

    // NOTE: the Task-3 review finding folded into Task 4 (`infer_call_result_
    // receiver`'s return-type lookup switched from a linear `.find` to
    // `graph.routines.binary_search_by`, mirroring `lookup_routine_access`/
    // `make_routine_route`) is a behavior-preserving refactor over the SAME
    // sorted `graph.routines` data — it is exercised end-to-end by the
    // existing Task 3 fixture suite (`ws_compound_call_result_*` in
    // `tests/program_resolve_harness.rs`, built via the real
    // `resolve_full_program` pipeline that populates and sorts `graph.routines`
    // exactly as production code does), which all continue to pass unchanged.
    // A hand-built unit `RoutineNode`/`DeclSurface`/`WithState` fixture here would
    // duplicate that coverage while risking drift from the real (much larger)
    // `RoutineNode` struct shape, so this is deliberately NOT re-tested with a
    // bespoke unit test.

    // -----------------------------------------------------------------------
    // T3 (receiver-closure-and-arg-increments plan): Step 2 — the named-
    // return-value binding synthesis + the SAME-SCOPE-ONLY malformed-
    // duplicate rule.
    // -----------------------------------------------------------------------

    /// POSITIVE: a routine's own named-return binding (`procedure X() Ret:
    /// Record Customer`) resolves a bare `Ret` receiver via the synthesized
    /// scoped symbol — mirrors `Ret.Get(...)` mid-body.
    #[test]
    fn step2_named_return_binding_resolves_via_synthesized_var() {
        let (graph, app) = build_test_graph();
        let index = ResolveIndex::build(&graph);
        let routine = RoutineDecl {
            return_name: Some("Ret".to_string()),
            return_type: Some("Record Customer".to_string()),
            ..routine_with_locals(vec![])
        };
        let from_obj = make_object_node(app, ObjectKind::Codeunit, "CallerCu", Some(999), None);
        let customer_id = graph
            .objects
            .iter()
            .find(|o| o.name == "Customer")
            .unwrap()
            .id
            .clone();

        let result =
            infer_receiver_type("ret", &routine, &[], &from_obj, &graph, &index, None, None);
        assert_eq!(
            result,
            ReceiverType::Record {
                table: Some(customer_id)
            }
        );
    }

    /// A QUOTED binding name resolves identically — `lookup_lc`'s existing
    /// quote-stripping (Step 2's quote-parity fix) applies to the binding
    /// exactly like a param/local.
    #[test]
    fn step2_quoted_named_return_binding_resolves() {
        let (graph, app) = build_test_graph();
        let index = ResolveIndex::build(&graph);
        let routine = RoutineDecl {
            return_name: Some("My Result".to_string()),
            return_type: Some("Record Customer".to_string()),
            ..routine_with_locals(vec![])
        };
        let from_obj = make_object_node(app, ObjectKind::Codeunit, "CallerCu", Some(999), None);
        let customer_id = graph
            .objects
            .iter()
            .find(|o| o.name == "Customer")
            .unwrap()
            .id
            .clone();

        let result = infer_receiver_type(
            "\"my result\"",
            &routine,
            &[],
            &from_obj,
            &graph,
            &index,
            None,
            None,
        );
        assert_eq!(
            result,
            ReceiverType::Record {
                table: Some(customer_id)
            }
        );
    }

    /// SHADOW (round-2 closer): the named-return binding SHADOWS a
    /// same-named GLOBAL (valid AL precedence) — the binding's type wins,
    /// never the global's.
    #[test]
    fn step2_named_return_binding_shadows_global() {
        let (graph, app) = build_test_graph();
        let index = ResolveIndex::build(&graph);
        let routine = RoutineDecl {
            return_name: Some("Ret".to_string()),
            return_type: Some("Record Customer".to_string()),
            ..routine_with_locals(vec![])
        };
        let globals = vec![var_decl("Ret", "Record SalesHeader")];
        let from_obj = make_object_node(app, ObjectKind::Codeunit, "CallerCu", Some(999), None);
        let customer_id = graph
            .objects
            .iter()
            .find(|o| o.name == "Customer")
            .unwrap()
            .id
            .clone();

        let result = infer_receiver_type(
            "ret", &routine, &globals, &from_obj, &graph, &index, None, None,
        );
        assert_eq!(
            result,
            ReceiverType::Record {
                table: Some(customer_id)
            },
            "the named-return binding must shadow a same-named global"
        );
    }

    /// SAME-SCOPE malformed duplicate (round-2 closer): a named-return
    /// binding colliding with a LOCAL of the identical name can never
    /// legally happen in valid AL (compile error) — must decline outright
    /// rather than guess a winner.
    #[test]
    fn step2_named_return_duplicate_with_local_declines() {
        let (graph, app) = build_test_graph();
        let index = ResolveIndex::build(&graph);
        let routine = RoutineDecl {
            return_name: Some("Ret".to_string()),
            return_type: Some("Record Customer".to_string()),
            ..routine_with_locals(vec![var_decl("Ret", "Record SalesHeader")])
        };
        let from_obj = make_object_node(app, ObjectKind::Codeunit, "CallerCu", Some(999), None);

        let result =
            infer_receiver_type("ret", &routine, &[], &from_obj, &graph, &index, None, None);
        assert_eq!(
            result,
            ReceiverType::Unknown,
            "a named-return binding colliding with a same-named local is malformed AL — decline"
        );
    }

    /// The SAME malformed-duplicate rule for a PARAM collision.
    #[test]
    fn step2_named_return_duplicate_with_param_declines() {
        let (graph, app) = build_test_graph();
        let index = ResolveIndex::build(&graph);
        let mut routine = RoutineDecl {
            return_name: Some("Ret".to_string()),
            return_type: Some("Record Customer".to_string()),
            ..routine_with_locals(vec![])
        };
        routine.params.push(Param {
            name: "Ret".to_string(),
            by_ref: false,
            ty: Some("Record SalesHeader".to_string()),
            origin: test_origin(),
        });
        let from_obj = make_object_node(app, ObjectKind::Codeunit, "CallerCu", Some(999), None);

        let result =
            infer_receiver_type("ret", &routine, &[], &from_obj, &graph, &index, None, None);
        assert_eq!(
            result,
            ReceiverType::Unknown,
            "a named-return binding colliding with a same-named param is malformed AL — decline"
        );
    }

    /// T4-C medium (e): a `#if`/`#else` union-read (al-syntax does not
    /// evaluate preproc conditions, so both arms survive — see
    /// `CallerScopeSymbol::MalformedDuplicate`'s doc) that declared the SAME
    /// local name with the SAME type in both branches must dedupe to ONE
    /// resolvable hit, not silently pick whichever the raw `Vec` order
    /// happened to put first (the pre-fix behavior — order-dependent but
    /// happened to be right here only by accident).
    #[test]
    fn caller_scope_identical_duplicate_local_dedupes_and_resolves() {
        let routine = routine_with_locals(vec![
            var_decl("Buf", "Record SalesHeader"),
            var_decl("Buf", "Record SalesHeader"),
        ]);
        assert_eq!(
            caller_scope_symbol("buf", &routine, &[]),
            CallerScopeSymbol::Found(Some("Record SalesHeader")),
            "an identical (name, type) duplicate must dedupe to one resolvable hit"
        );
    }

    /// T4-C medium (e): the SAME union-read, but the two branches declared
    /// the local with DIFFERENT types (a genuinely unprovable conflict) —
    /// must decline outright, mirroring `ResolveIndex::field_in_table`'s
    /// established dedupe-then-decline pattern instead of the pre-fix
    /// first-match-wins `Vec::iter().find()` (which silently returned
    /// whichever type happened to be declared first).
    #[test]
    fn caller_scope_conflicting_duplicate_local_declines() {
        let routine = routine_with_locals(vec![
            var_decl("Buf", "Record SalesHeader"),
            var_decl("Buf", "Record Customer"),
        ]);
        assert_eq!(
            caller_scope_symbol("buf", &routine, &[]),
            CallerScopeSymbol::MalformedDuplicate,
            "a conflicting (same name, different type) duplicate must decline, never guess"
        );
    }

    /// The SAME conflicting-duplicate decline, for a PARAM instead of a local.
    #[test]
    fn caller_scope_conflicting_duplicate_param_declines() {
        let mut routine = routine_with_locals(vec![]);
        routine.params.push(Param {
            name: "X".to_string(),
            by_ref: false,
            ty: Some("Integer".to_string()),
            origin: test_origin(),
        });
        routine.params.push(Param {
            name: "X".to_string(),
            by_ref: false,
            ty: Some("Text".to_string()),
            origin: test_origin(),
        });
        assert_eq!(
            caller_scope_symbol("x", &routine, &[]),
            CallerScopeSymbol::MalformedDuplicate,
            "a conflicting param duplicate must decline, never guess"
        );
    }

    /// NEGATIVE: no `return_name` at all (anonymous `: Type` return, or no
    /// return spec) — a bare identifier matching the OLD `return_type`-only
    /// text must never be treated as a synthesized binding.
    #[test]
    fn step2_no_return_name_does_not_synthesize_a_binding() {
        let (graph, app) = build_test_graph();
        let index = ResolveIndex::build(&graph);
        let routine = RoutineDecl {
            return_name: None,
            return_type: Some("Integer".to_string()),
            ..routine_with_locals(vec![])
        };
        let from_obj = make_object_node(app, ObjectKind::Codeunit, "CallerCu", Some(999), None);

        let result =
            infer_receiver_type("ret", &routine, &[], &from_obj, &graph, &index, None, None);
        assert_eq!(result, ReceiverType::Unknown);
    }

    /// NO CROSS-ROUTINE LEAKAGE: the binding is scoped to ITS OWN
    /// `RoutineDecl` only — a DIFFERENT routine (no binding of its own) must
    /// never resolve the same bare name via some other routine's binding.
    #[test]
    fn step2_named_return_binding_no_cross_routine_leakage() {
        let (graph, app) = build_test_graph();
        let index = ResolveIndex::build(&graph);
        let routine_with_binding = RoutineDecl {
            return_name: Some("Ret".to_string()),
            return_type: Some("Record Customer".to_string()),
            ..routine_with_locals(vec![])
        };
        let routine_without_binding = routine_with_locals(vec![]);
        let from_obj = make_object_node(app, ObjectKind::Codeunit, "CallerCu", Some(999), None);
        let customer_id = graph
            .objects
            .iter()
            .find(|o| o.name == "Customer")
            .unwrap()
            .id
            .clone();

        let with_binding = infer_receiver_type(
            "ret",
            &routine_with_binding,
            &[],
            &from_obj,
            &graph,
            &index,
            None,
            None,
        );
        assert_eq!(
            with_binding,
            ReceiverType::Record {
                table: Some(customer_id)
            }
        );

        let without_binding = infer_receiver_type(
            "ret",
            &routine_without_binding,
            &[],
            &from_obj,
            &graph,
            &index,
            None,
            None,
        );
        assert_eq!(
            without_binding,
            ReceiverType::Unknown,
            "a routine with no binding of its own must never resolve via a DIFFERENT routine's binding"
        );
    }

    /// END-TO-END (real lowerer through Step 2): the binding is referenced as
    /// the FIRST statement in the body (used-before-assignment — the engine
    /// is not flow-sensitive, so this resolves identically regardless of
    /// statement position).
    #[test]
    fn step2_named_return_binding_resolves_from_real_lowered_source_used_before_assignment() {
        let src = r#"
codeunit 50100 "C"
{
    procedure GetItem() Ret: Record Customer
    begin
        Ret.Get('1');
    end;
}
"#;
        let (file, receiver_text, receiver_id) = parse_member_site(src, "get");
        assert_eq!(receiver_text.to_ascii_lowercase(), "ret");

        let (graph, app) = build_test_graph();
        let index = ResolveIndex::build(&graph);
        let real_routine = &file.objects[0].routines[0];
        assert_eq!(real_routine.return_name.as_deref(), Some("Ret"));
        let from_obj = make_object_node(app, ObjectKind::Codeunit, "C", Some(50100), None);
        let customer_id = graph
            .objects
            .iter()
            .find(|o| o.name == "Customer")
            .unwrap()
            .id
            .clone();

        let result = infer_receiver_type(
            &receiver_text.to_ascii_lowercase(),
            real_routine,
            &[],
            &from_obj,
            &graph,
            &index,
            Some((&file, receiver_id)),
            None,
        );
        assert_eq!(
            result,
            ReceiverType::Record {
                table: Some(customer_id)
            },
            "a reference to the binding as the FIRST statement (used before any assignment) must resolve identically"
        );
    }

    // -----------------------------------------------------------------------
    // T3: Step 3a widened — bare UNQUOTED implicit-self table field receiver
    // (the same field-index machinery the quoted case above already uses,
    // minus the quote requirement).
    // -----------------------------------------------------------------------

    /// POSITIVE: `Attachment.CreateInStream(S)` (unquoted) inside a Table's
    /// own procedure — the implicit-Rec field types `Framework(Blob)`.
    #[test]
    fn step3a_bare_unquoted_field_in_table_scope_resolves_blob() {
        let (mut graph, _app) = build_test_graph();
        let customer_idx = graph
            .objects
            .iter()
            .position(|o| o.name == "Customer")
            .unwrap();
        graph.objects[customer_idx].fields.push(FieldNode {
            name_lc: "attachment".to_string(),
            type_text: "Blob".to_string(),
        });
        let index = ResolveIndex::build(&graph);
        let routine = routine_with_locals(vec![]);
        let from_obj = graph.objects[customer_idx].clone();
        let surface = DeclSurface::build(&graph, &[]);

        let result = infer_receiver_type(
            "attachment",
            &routine,
            &[],
            &from_obj,
            &graph,
            &index,
            None,
            Some((&surface, WithState::NoWithProven)),
        );
        assert_eq!(result, ReceiverType::Framework(FrameworkKind::Blob));
    }

    /// The SAME shape inside a TableExtension's own procedure — resolves for
    /// BOTH an own-extension field and a base-table field, unquoted.
    #[test]
    fn step3a_bare_unquoted_field_in_tableextension_scope_resolves() {
        let (mut graph, app) = build_test_graph();
        let customer_idx = graph
            .objects
            .iter()
            .position(|o| o.name == "Customer")
            .unwrap();
        graph.objects[customer_idx].fields.push(FieldNode {
            name_lc: "baseblob".to_string(),
            type_text: "Blob".to_string(),
        });
        let mut ext_obj = make_object_node(
            app,
            ObjectKind::TableExtension,
            "CustomerExt2",
            Some(50201),
            Some("Customer".to_string()),
        );
        ext_obj.fields.push(FieldNode {
            name_lc: "extnote".to_string(),
            type_text: "Text[100]".to_string(),
        });
        graph.objects.push(ext_obj);
        graph.objects.sort_by(|a, b| a.id.cmp(&b.id));
        let index = ResolveIndex::build(&graph);
        let routine = routine_with_locals(vec![]);
        let from_obj = graph
            .objects
            .iter()
            .find(|o| o.name == "CustomerExt2")
            .unwrap()
            .clone();
        let surface = DeclSurface::build(&graph, &[]);

        let result_own = infer_receiver_type(
            "extnote",
            &routine,
            &[],
            &from_obj,
            &graph,
            &index,
            None,
            Some((&surface, WithState::NoWithProven)),
        );
        assert_eq!(result_own, ReceiverType::Framework(FrameworkKind::Text));

        let result_base = infer_receiver_type(
            "baseblob",
            &routine,
            &[],
            &from_obj,
            &graph,
            &index,
            None,
            Some((&surface, WithState::NoWithProven)),
        );
        assert_eq!(result_base, ReceiverType::Framework(FrameworkKind::Blob));
    }

    /// The routine-shadow negative (unquoted form): a same-named PROCEDURE
    /// anywhere in the visibility-scoped table surface must decline this
    /// step entirely, never guess — mirrors the pre-existing quoted-form
    /// guard test exactly.
    #[test]
    fn step3a_unquoted_declines_when_same_named_routine_exists() {
        let (mut graph, _app) = build_test_graph();
        let customer_idx = graph
            .objects
            .iter()
            .position(|o| o.name == "Customer")
            .unwrap();
        graph.objects[customer_idx].fields.push(FieldNode {
            name_lc: "attachment".to_string(),
            type_text: "Blob".to_string(),
        });
        let customer_id = graph.objects[customer_idx].id.clone();
        graph
            .routines
            .push(make_routine_node(customer_id, "Attachment"));
        graph.routines.sort_by(|x, y| x.id.cmp(&y.id));
        let index = ResolveIndex::build(&graph);
        let routine = routine_with_locals(vec![]);
        let from_obj = graph.objects[customer_idx].clone();
        let surface = DeclSurface::build(&graph, &[]);

        let result = infer_receiver_type(
            "attachment",
            &routine,
            &[],
            &from_obj,
            &graph,
            &index,
            None,
            Some((&surface, WithState::NoWithProven)),
        );
        assert_eq!(
            result,
            ReceiverType::Unknown,
            "a same-named routine anywhere in the table scope must block unquoted field-typing too"
        );
    }

    /// NEGATIVE: a non-Table/TableExtension object is unaffected by the
    /// widening — the unquoted branch never fires outside Table scope.
    #[test]
    fn step3a_unquoted_non_table_object_declines() {
        let (graph, app) = build_test_graph();
        let index = ResolveIndex::build(&graph);
        let routine = routine_with_locals(vec![]);
        let from_obj = make_object_node(app, ObjectKind::Codeunit, "CallerCu", Some(999), None);
        let surface = DeclSurface::build(&graph, &[]);

        let result = infer_receiver_type(
            "attachment",
            &routine,
            &[],
            &from_obj,
            &graph,
            &index,
            None,
            Some((&surface, WithState::NoWithProven)),
        );
        assert_eq!(result, ReceiverType::Unknown);
    }

    /// GLOBAL-vs-FIELD precedence (the T3 proof note): a GLOBAL var shadows
    /// a same-named FIELD — Step 2 runs (and returns) strictly before Step
    /// 3a's widened field lookup ever executes.
    #[test]
    fn step2_global_var_shadows_same_named_field() {
        let (mut graph, _app) = build_test_graph();
        let customer_idx = graph
            .objects
            .iter()
            .position(|o| o.name == "Customer")
            .unwrap();
        graph.objects[customer_idx].fields.push(FieldNode {
            name_lc: "attachment".to_string(),
            type_text: "Blob".to_string(),
        });
        let index = ResolveIndex::build(&graph);
        let routine = routine_with_locals(vec![]);
        let globals = vec![var_decl("Attachment", "Text[100]")];
        let from_obj = graph.objects[customer_idx].clone();
        let surface = DeclSurface::build(&graph, &[]);

        let result = infer_receiver_type(
            "attachment",
            &routine,
            &globals,
            &from_obj,
            &graph,
            &index,
            None,
            Some((&surface, WithState::NoWithProven)),
        );
        assert_eq!(
            result,
            ReceiverType::Framework(FrameworkKind::Text),
            "a global var must shadow a same-named field (Step 2 runs before Step 3a)"
        );
    }

    /// Defensive guard: a bare UNQUOTED `rec` must NEVER be intercepted by
    /// the widened Step 3a field lookup, even if a table happens to declare
    /// a field literally named `rec` — it must keep falling through to Step
    /// 3b's identity fallback (the type distinguishes: `Text[50]` for the
    /// hypothetical field vs `Record{Customer}` for the identity).
    #[test]
    fn step3a_unquoted_rec_literal_still_falls_through_to_identity() {
        let (mut graph, _app) = build_test_graph();
        let customer_idx = graph
            .objects
            .iter()
            .position(|o| o.name == "Customer")
            .unwrap();
        graph.objects[customer_idx].fields.push(FieldNode {
            name_lc: "rec".to_string(),
            type_text: "Text[50]".to_string(),
        });
        let customer_id = graph.objects[customer_idx].id.clone();
        let index = ResolveIndex::build(&graph);
        let routine = routine_with_locals(vec![]);
        let from_obj = graph.objects[customer_idx].clone();
        let surface = DeclSurface::build(&graph, &[]);

        let result = infer_receiver_type(
            "rec",
            &routine,
            &[],
            &from_obj,
            &graph,
            &index,
            None,
            Some((&surface, WithState::NoWithProven)),
        );
        assert_eq!(
            result,
            ReceiverType::Record {
                table: Some(customer_id)
            },
            "bare unquoted `rec` must resolve via Step 3b identity, never Step 3a's widened field lookup"
        );
    }

    /// The pre-existing QUOTED path is UNCHANGED after the widening —
    /// `"View (Blob)".CreateInStream` still resolves via Step 3a exactly as
    /// before this task.
    #[test]
    fn step3a_quoted_field_with_space_and_parens_still_resolves_after_widening() {
        let (mut graph, _app) = build_test_graph();
        let customer_idx = graph
            .objects
            .iter()
            .position(|o| o.name == "Customer")
            .unwrap();
        graph.objects[customer_idx].fields.push(FieldNode {
            name_lc: "view (blob)".to_string(),
            type_text: "Blob".to_string(),
        });
        let index = ResolveIndex::build(&graph);
        let routine = routine_with_locals(vec![]);
        let from_obj = graph.objects[customer_idx].clone();
        let surface = DeclSurface::build(&graph, &[]);

        let result = infer_receiver_type(
            "\"view (blob)\"",
            &routine,
            &[],
            &from_obj,
            &graph,
            &index,
            None,
            Some((&surface, WithState::NoWithProven)),
        );
        assert_eq!(result, ReceiverType::Framework(FrameworkKind::Blob));
    }
}

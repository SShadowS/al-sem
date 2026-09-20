//! CST → IR lowering — the ONLY grammar-aware logic above the raw layer.
//!
//! Phase 1a (this step): outer structure — objects → routines → params / return
//! type / locals / globals. Statement/expression bodies (`RoutineDecl.body`) and
//! `temporary` detection are filled by the next Phase-1 step and validated against
//! the legacy walk under dual-run (spec §5). Unmodelled-but-present nodes are never
//! silently dropped — they surface as `SyntaxIssue` / IR `Unknown`.

use crate::casing::IdentifierFoldExt;
use crate::ir::{
    AlFile, BinaryOp, Block, BlockId, BlockItem, CaseBranch, Expr, ExprId, ExprKind, Ir, Literal,
    ObjectDecl, ObjectKind, Origin, Param, ParseStatus, Point, RoutineDecl, RoutineKind, Stmt,
    StmtId, StmtKind, SyntaxIssue, UnaryOp, VarDecl,
};
use crate::raw::{FieldName, RawKind, RawNode};

/// Maximum AL syntactic nesting depth (statement or expression) the statement/
/// expression lowerer will descend into via ordinary recursion.
///
/// `lower_stmt` / `lower_expr` (mutually recursive with several plumbing helpers:
/// `lower_branch`, `lower_code_block`, `lower_stmt_seq`, `lower_block_child`,
/// `lower_case_body`, `lower_opt_field`, `lower_branch_field`, …) are a classic
/// recursive-descent walk with no bound of its own — a generated or hostile file
/// (`x := 1 + 1 + … 50k terms`, or 50k-deep nested `if`) recurses the NATIVE call
/// stack proportionally to input size, and release builds are `panic=abort`, so a
/// stack overflow is an uncatchable process kill (T2.1). This budget makes the
/// caller's big stack (`al_sem::big_stack`) belt-and-suspenders rather
/// than load-bearing: past this depth, lowering fails closed — a `SyntaxIssue` +
/// `ExprKind::Unknown` / `StmtKind::Unknown` — instead of recursing further.
///
/// `depth` counts `lower_stmt` / `lower_expr` stack frames specifically (the two
/// truly self-recursive functions) — plumbing helpers forward it unchanged — so
/// it tracks AL syntactic nesting depth, not raw native-frame count.
///
/// This repo's other depth-budget precedent (`MAX_CBOR_DEPTH`,
/// `src/engine/gate/snapshot_deserialize.rs`) uses 256; **measured empirically**
/// against the actual small-stack red fixture this budget exists for (a 50k-term
/// binary-expression chain lowered on a 1 MiB thread — the LSP main thread's real
/// Windows stack size), 256 was NOT safe: an unoptimized debug build's `lower_expr`
/// / `lower_opt_field` frame pair is large enough that 256 levels came within the
/// failure margin (crashed at 256, passed at 224). 128 gives a comfortable ~2x
/// safety margin under that measured boundary, in the WORST case (debug,
/// unoptimized — a release build's smaller, optimized frames make this even safer
/// there). Still nowhere near what real AL source nests to, so a legitimate file
/// never trips it. Re-measure (`cargo test -p al-syntax deep_`) if this recursive
/// family's frame footprint changes materially.
const MAX_LOWER_DEPTH: u32 = 128;

/// Lower a parsed file root into the owned IR.
pub fn lower_file(root: RawNode, source: &str) -> AlFile {
    let parse_status = if root.has_error() {
        ParseStatus::Recovered
    } else {
        ParseStatus::Clean
    };
    let mut ir = Ir::new();
    let mut issues = Vec::new();
    let mut objects = Vec::new();
    collect_objects(root, source, &mut ir, &mut issues, &mut objects);
    AlFile {
        objects,
        ir,
        issues,
        parse_status,
    }
}

/// Walk for top-level object declarations, descending namespaces and preproc
/// wrappers (which may enclose objects in BC 24+ / `#if` builds).
fn collect_objects(
    node: RawNode,
    source: &str,
    ir: &mut Ir,
    issues: &mut Vec<crate::ir::SyntaxIssue>,
    out: &mut Vec<ObjectDecl>,
) {
    for child in node.named_children() {
        match object_kind_of(child.kind()) {
            Some(kind) => out.push(lower_object(child, kind, source, ir, issues)),
            None => {
                // Descend containers that may hold objects (namespace, preproc).
                if child.kind() == RawKind::NamespaceDeclaration || is_preproc_wrapper(child) {
                    collect_objects(child, source, ir, issues, out);
                }
            }
        }
    }
}

/// A `preproc_conditional*` wrapper node (`#if`/`#else` region). The lowerer
/// descends BOTH branches (legacy indexes both for BC version-compat).
///
/// **Union-read semantics, stated honestly (Task 3, preproc foundations
/// plan).** Every collector that calls this (objects via `collect_objects`,
/// routines via `collect_routines`, globals/locals via `collect_globals`,
/// statements via `lower_block_child`, object properties via
/// `collect_properties`, `implements` via `extract_implements`) treats `#if`
/// as TRANSPARENT: it unions the content of every branch — including a dead
/// `#if UNDEFINED_SYMBOL .. #endif` branch that would never compile in for
/// ANY real build — into one flat IR. This is a deliberate SUPERSET
/// over-approximation:
/// - **Sound for absence proofs** — if a member is absent from the union, it
///   is absent from every possible build, so "not found anywhere in the
///   union" is a valid non-existence witness.
/// - **NOT sound for resolution CONFIDENCE** — a resolved call route may
///   target dead-branch code that never actually compiles into the running
///   app. Nothing downstream should read "the engine resolved this call" as
///   proof the target is reachable in a specific build without also
///   accounting for `#if` conditionality.
/// - A singular per-branch VALUE (an object property like `SourceTable`) can
///   therefore disagree across branches after this union-read — the
///   consuming layer (`crate::program`) must degrade a genuine disagreement
///   rather than pick one (see `collect_properties`'s doc); a purely additive
///   list-valued union (`implements`) needs no such degrade (see
///   `extract_implements`'s doc).
fn is_preproc_wrapper(n: RawNode) -> bool {
    n.kind_str().starts_with("preproc_conditional")
}

/// Non-trivia named children, in document order — the ONE shared filter for every
/// positional argument/child-list read (H-8, Tier-1.4 preproc plan). A `comment` /
/// `multiline_comment` is a legal named child almost anywhere (grammar `extra`), so a
/// bare `named_children()` scan silently lets it occupy a real positional slot: as the
/// sole child of a `parenthesized_expression` (replacing the real inner expression), as
/// a phantom argument in a `call_expression`'s `argument_list` (breaking arity-exact
/// dispatch), or in an `[Attribute(...)]`'s `attribute_argument_list` (shifting every
/// later positional read — e.g. `parse_event_subscriber_ir`'s `attr.args[N]` — silently
/// unregistering the whole construct). Every one of those sites now goes through here.
fn structural_children(node: RawNode) -> Vec<RawNode> {
    node.named_children()
        .into_iter()
        .filter(|c| crate::schema::class_of(c.kind()) != crate::schema::Class::Trivia)
        .collect()
}

fn object_kind_of(k: RawKind) -> Option<ObjectKind> {
    use ObjectKind as O;
    Some(match k {
        RawKind::CodeunitDeclaration | RawKind::PreprocSplitDeclaration => O::Codeunit,
        RawKind::TableDeclaration => O::Table,
        RawKind::TableextensionDeclaration => O::TableExtension,
        RawKind::PageDeclaration => O::Page,
        RawKind::PageextensionDeclaration => O::PageExtension,
        RawKind::ReportDeclaration => O::Report,
        RawKind::ReportextensionDeclaration => O::ReportExtension,
        RawKind::QueryDeclaration => O::Query,
        RawKind::XmlportDeclaration => O::XmlPort,
        RawKind::EnumDeclaration => O::Enum,
        RawKind::EnumextensionDeclaration => O::EnumExtension,
        RawKind::InterfaceDeclaration => O::Interface,
        RawKind::ControladdinDeclaration => O::ControlAddIn,
        RawKind::EntitlementDeclaration => O::Entitlement,
        RawKind::PermissionsetDeclaration => O::PermissionSet,
        RawKind::PermissionsetextensionDeclaration => O::PermissionSetExtension,
        RawKind::ProfileDeclaration => O::Profile,
        _ => return None,
    })
}

fn lower_object(
    node: RawNode,
    kind: ObjectKind,
    source: &str,
    ir: &mut Ir,
    issues: &mut Vec<crate::ir::SyntaxIssue>,
) -> ObjectDecl {
    let id = node
        .field(FieldName::ObjectId)
        .and_then(|n| n.text(source).trim().parse::<i64>().ok());
    let name = node
        .field(FieldName::ObjectName)
        .map(|n| ident_text(n, source))
        .unwrap_or_default();

    // Routines: every procedure/trigger anywhere in the object subtree (incl. field
    // /action triggers nested in sections, and both #if/#else branches). A dataitem
    // trigger carries its enclosing dataitem's source table (implicit-`Rec` type).
    // `dataset_ctx` starts `false` (Task 1, dataitem-receivers plan) — it only ever
    // becomes `true` while descending a report/report-extension `dataset` section, and
    // is force-reset to `false` on entering `requestpage` (REQUESTPAGE ISOLATION).
    let mut routine_nodes = Vec::new();
    collect_routines(node, None, None, false, source, &mut routine_nodes);
    let routines = routine_nodes
        .into_iter()
        .map(
            |(r, attr_items, di_table, member, in_dataset_modify_context)| {
                lower_routine(
                    r,
                    attr_items,
                    di_table,
                    member,
                    in_dataset_modify_context,
                    source,
                    ir,
                    issues,
                )
            },
        )
        .collect();

    // Report dataitems (name, source-table) — a dataitem name is in scope as a record
    // var across all the report's routines. Reports only (empty otherwise).
    let mut report_dataitems = Vec::new();
    if matches!(kind, ObjectKind::Report | ObjectKind::ReportExtension) {
        collect_report_dataitems(node, source, &mut report_dataitems);
    }

    // Extension `extends` target (the grammar's `base_object` field) — None for
    // non-extension objects (no such field).
    let extends_target = node
        .field(FieldName::BaseObject)
        .map(|n| ident_text(n, source))
        .filter(|s| !s.is_empty());

    // `implements` interface names (Codeunit / Enum / Interface).
    let implements = if matches!(
        kind,
        ObjectKind::Codeunit | ObjectKind::Enum | ObjectKind::Interface
    ) {
        extract_implements(node, source)
    } else {
        Vec::new()
    };

    // Page controls (Page / PageExtension).
    let mut page_controls = Vec::new();
    if matches!(kind, ObjectKind::Page | ObjectKind::PageExtension) {
        collect_page_controls(node, source, &mut page_controls);
    }

    // Table fields + keys (Table / TableExtension).
    let mut fields = Vec::new();
    let mut keys = Vec::new();
    if matches!(kind, ObjectKind::Table | ObjectKind::TableExtension) {
        collect_table_fields_keys(node, source, &mut fields, &mut keys);
    }

    // Object globals: var_sections under the declaration_body (not inside routines).
    // Object-level properties (SourceTable / TableNo / PageType / …) are siblings.
    // Both collectors descend preproc wrappers (both `#if`/`#else` branches) —
    // `collect_properties` mirrors `collect_globals`'s established pattern (Task
    // 3, preproc foundations plan: a `#if`-wrapped property was previously
    // silently dropped by a flat `body.named_children()` scan; see that
    // function's doc for the union-read + program-layer-degrade contract).
    let mut globals = Vec::new();
    let mut properties = Vec::new();
    if let Some(body) = node.field(FieldName::Body) {
        for member in body.named_children() {
            collect_globals(member, source, &mut globals);
            collect_properties(member, source, &mut properties);
        }
    }

    ObjectDecl {
        kind,
        id,
        name,
        routines,
        globals,
        properties,
        report_dataitems,
        extends_target,
        implements,
        page_controls,
        fields,
        keys,
        origin: origin_of(node),
    }
}

/// `implements` interface names (unquoted, document order). Mirrors the legacy
/// `extract_implements_interfaces`: names after the `implements` keyword, or the
/// members of an `implements_clause` wrapper.
///
/// Descends preproc wrappers (Task 3, preproc foundations plan) — defensive
/// rather than fixing a live gap: the only grammar-reachable `#if`-conditional
/// `implements` shape today is `preproc_split_declaration` (`#if COND
/// codeunit 1 X implements A #else codeunit 1 X implements B #endif { .. }`),
/// which the grammar itself flattens — both branches' `_object_header`s
/// (hence both `implements_clause` nodes) are already direct siblings of
/// `node` with no wrapper in between, so the ORIGINAL flat loop already found
/// both unaided (verified: `implements_clause` is only ever reachable from
/// `_object_header`, which is never a `_body_element` — a generic
/// `preproc_conditional` cannot wrap it). The descend below keeps this walk
/// consistent with every other collector (`collect_globals`/
/// `collect_properties`/`lower_block_child`) and future-proofs against a
/// grammar evolution that wraps a body-scoped conditional interface clause.
///
/// A conflicting union (a different interface name per `#if` branch) is
/// captured as-is — BOTH names land in the result — and is intentionally
/// NEVER degraded at the program layer, unlike a singular property
/// (`SourceTable`/`TableNo`): every consumer of `ObjectDecl.implements`
/// (`ResolveIndex`'s interface-implementer index, `interface_route_
/// applicable`) only ever asks "does this object POSSIBLY implement
/// `iface`?" for ADDITIVE may-fire fan-out — never "pick the one interface
/// this object implements". Including an object under both of its
/// conditional interfaces over-approximates the implementer set (one branch
/// is always dead at compile time) but never fabricates a false SINGLE-
/// target confidence, so the union is sound without a degrade.
fn extract_implements(node: RawNode, source: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut saw_implements = false;
    extract_implements_walk(node, source, &mut saw_implements, &mut out);
    out
}

/// Recursive walk backing [`extract_implements`]. Returns `true` once a
/// declaration/code body boundary was crossed (propagated up so every
/// enclosing call also stops — mirrors the original flat loop's `break`).
fn extract_implements_walk(
    node: RawNode,
    source: &str,
    saw_implements: &mut bool,
    out: &mut Vec<String>,
) -> bool {
    for child in node.named_children() {
        match child.kind() {
            RawKind::ImplementsKeyword => *saw_implements = true,
            RawKind::ImplementsClause => {
                for sub in child.named_children() {
                    if matches!(sub.kind(), RawKind::Identifier | RawKind::QuotedIdentifier) {
                        out.push(ident_text(sub, source));
                    }
                }
            }
            RawKind::Identifier | RawKind::QuotedIdentifier if *saw_implements => {
                out.push(ident_text(child, source));
            }
            // Stop at the object body / a routine body.
            RawKind::DeclarationBody | RawKind::CodeBlock if *saw_implements => return true,
            _ if is_preproc_wrapper(child)
                && extract_implements_walk(child, source, saw_implements, out) =>
            {
                return true;
            }
            _ => {}
        }
    }
    false
}

/// Collect page `part` / `systempart` / `usercontrol` sections (name, kind, target),
/// document order. Recurses the layout but NOT into routine bodies. Mirrors the legacy
/// `extract_page_controls`.
fn collect_page_controls(node: RawNode, source: &str, out: &mut Vec<crate::ir::PageControl>) {
    let kind = match node.kind() {
        RawKind::PartSection => Some("part"),
        RawKind::SystempartSection => Some("systempart"),
        RawKind::UsercontrolSection => Some("usercontrol"),
        _ => None,
    };
    if let Some(kind) = kind
        && let (Some(name), Some(src)) =
            (node.field(FieldName::Name), node.field(FieldName::Source))
    {
        out.push(crate::ir::PageControl {
            name: ident_text(name, source),
            kind: kind.to_string(),
            target: ident_text(src, source),
        });
    }
    if node.kind() == RawKind::CodeBlock {
        return;
    }
    for c in node.named_children() {
        collect_page_controls(c, source, out);
    }
}

/// Collect table `field(...)` declarations + `key(...)` member-name lists (prune at
/// match, document order). Mirrors the legacy `index_table` / `classify_field`.
fn collect_table_fields_keys(
    node: RawNode,
    source: &str,
    fields: &mut Vec<crate::ir::FieldDecl>,
    keys: &mut Vec<Vec<String>>,
) {
    for child in node.named_children() {
        match child.kind() {
            RawKind::FieldDeclaration => fields.push(lower_field(child, source)),
            RawKind::KeyDeclaration => {
                let mut members = Vec::new();
                if let Some(list) = child.field(FieldName::Fields) {
                    for m in list.named_children() {
                        if matches!(m.kind(), RawKind::Identifier | RawKind::QuotedIdentifier) {
                            members.push(ident_text(m, source).fold_identifier());
                        }
                    }
                }
                keys.push(members);
            }
            _ => collect_table_fields_keys(child, source, fields, keys),
        }
    }
}

/// Lower a `field(<no>; <Name>; <Type>) { ... }` declaration.
fn lower_field(node: RawNode, source: &str) -> crate::ir::FieldDecl {
    let number = node
        .field(FieldName::Id)
        .and_then(|n| n.text(source).trim().parse::<i64>().ok())
        .unwrap_or(0);
    let name = node
        .field(FieldName::Name)
        .map(|n| ident_text(n, source))
        .unwrap_or_default();
    let data_type = node
        .field(FieldName::Type)
        .map(|n| n.text(source).trim().to_string())
        .unwrap_or_default();
    // FieldClass property (in the field's declaration_body): FlowField / FlowFilter /
    // Normal. Mirrors classify_field.
    let mut field_class = "Normal".to_string();
    if let Some(body) = node.field(FieldName::Body) {
        for member in body.named_children() {
            if member.kind() != RawKind::Property {
                continue;
            }
            let Some(pname) = member.field(FieldName::Name) else {
                continue;
            };
            if !pname.text(source).trim().eq_ignore_ascii_case("fieldclass") {
                continue;
            }
            let v = member
                .field(FieldName::Value)
                .map(|n| n.text(source).fold_identifier())
                .unwrap_or_default();
            if v.contains("flowfield") {
                field_class = "FlowField".to_string();
            } else if v.contains("flowfilter") {
                field_class = "FlowFilter".to_string();
            }
        }
    }
    let dt_lc = data_type.to_ascii_lowercase();
    let is_blob_like = dt_lc == "blob" || dt_lc == "media" || dt_lc == "mediaset";
    crate::ir::FieldDecl {
        number,
        name,
        data_type,
        field_class,
        is_blob_like,
    }
}

/// Collect every report `dataitem(Name; "Source Table")` (incl. nested) as
/// `(name, source-table)`, both unquoted, document order. Mirrors the legacy
/// `report_dataitem_record_vars`.
fn collect_report_dataitems(node: RawNode, source: &str, out: &mut Vec<(String, String)>) {
    for child in node.named_children() {
        if child.kind() == RawKind::ReportDataitem {
            let name = child
                .field(FieldName::Name)
                .map(|n| ident_text(n, source))
                .unwrap_or_default();
            let table = dataitem_table_name(child, source).unwrap_or_default();
            if !name.is_empty() && !table.is_empty() {
                out.push((name, table));
            }
        }
        // Descend (nested dataitems live under a dataitem's body); routine bodies hold
        // no dataitems so the extra recursion is harmless.
        collect_report_dataitems(child, source, out);
    }
}

/// Lower a `property` node (`name = value`). Name lowercased; value is the raw text
/// of the value field (trimmed). None when the name is missing.
fn lower_property(node: RawNode, source: &str) -> Option<crate::ir::ObjectProperty> {
    let name = node
        .field(FieldName::Name)?
        .text(source)
        .trim()
        .fold_identifier();
    let value = node
        .field(FieldName::Value)
        .map(|v| v.text(source).trim().to_string())
        .unwrap_or_default();
    Some(crate::ir::ObjectProperty {
        name,
        value,
        origin: origin_of(node),
    })
}

/// Collect object-level `property` declarations, descending preproc wrappers
/// (both `#if`/`#else` branches) — mirrors [`collect_globals`]'s established
/// pattern. Union-read: a `#if`-wrapped singular property (e.g. `SourceTable`,
/// `TableNo`) now surfaces EVERY syntactically-present value, one per branch —
/// this lowering layer never resolves the `#if` condition, it only reports
/// what is textually there (same superset semantics as objects/routines/
/// globals; see [`is_preproc_wrapper`]'s module-level doc). A singular
/// property with two DIFFERING branch values is therefore ambiguous at this
/// layer by construction; [`crate::program`] (the consumer) is responsible
/// for degrading such a conflict rather than silently picking one (first/
/// last-wins is the cardinal sin this fix exists to prevent) — see
/// `node_extract::singular_property_value`.
fn collect_properties(node: RawNode, source: &str, out: &mut Vec<crate::ir::ObjectProperty>) {
    match node.kind() {
        RawKind::Property => {
            if let Some(p) = lower_property(node, source) {
                out.push(p);
            }
        }
        _ if is_preproc_wrapper(node) => {
            for c in node.named_children() {
                collect_properties(c, source, out);
            }
        }
        _ => {}
    }
}

/// DFS collecting `(routine, attribute items, enclosing-dataitem-source-table,
/// enclosing-member, in-dataset-modify-context)` 5-tuples. AL has no nested routines, so
/// we do not descend into a routine once found. `attribute_item` nodes are SIBLINGS
/// preceding the routine (grammar v2+); accumulate them and attach to the next routine,
/// resetting on any other node. When the walk crosses into a report `dataitem(Name;
/// "Source Table")`, the (innermost) dataitem's source table is threaded down so a
/// dataitem trigger gets its implicit-`Rec` type.
///
/// `dataset_ctx` (Task 1, dataitem-receivers plan) tracks whether the walk is currently
/// inside a report/report-extension `dataset` section: `true` on entering
/// `dataset_section`/`report_dataitem`, force-reset to `false` on entering
/// `requestpage_section` (REQUESTPAGE ISOLATION — binding, never leaks a dataset
/// `modify()`'s context into a requestpage trigger), inherited unchanged otherwise. Used
/// only to compute `in_dataset_modify_context` at push time — see that tuple field's doc
/// and [`crate::ir::RoutineDecl::in_dataset_modify_context`].
#[allow(clippy::type_complexity)]
fn collect_routines<'t>(
    node: RawNode<'t>,
    dataitem_table: Option<&str>,
    member: Option<RawNode<'t>>,
    dataset_ctx: bool,
    source: &str,
    out: &mut Vec<(
        RawNode<'t>,
        Vec<RawNode<'t>>,
        Option<String>,
        Option<RawNode<'t>>,
        bool,
    )>,
) {
    let mut pending: Vec<RawNode<'t>> = Vec::new();
    for child in node.named_children() {
        match child.kind() {
            RawKind::AttributeItem => pending.push(child),
            RawKind::Procedure
            | RawKind::TriggerDeclaration
            | RawKind::InterfaceProcedure
            // `preproc_split_procedure` / `preproc_split_procedure_preamble` (H-6, Tier-1.4
            // preproc plan): a procedure whose HEADER (signature, and for the `_preamble`
            // variant also its `var` section) differs across `#if`/`#elif`/`#else` branches
            // but shares ONE body compiled into every build. Both `_procedure_header` and
            // `_routine_regular_body` are grammar-INLINED sub-rules, so their `field('name',
            // ..)` / `field('parameters', ..)` / `field('body', ..)` tags flatten straight
            // onto the wrapper node — repeated once per branch for name/parameters (VERIFIED
            // against the pinned grammar: `.field()` returns the FIRST match, mirroring the
            // already-established first-branch-wins policy `PreprocSplitDeclaration` uses for
            // object name/id) and ONCE (shared, after `#endif`) for `preproc_split_procedure`'s
            // body. Previously matched only the `_` catch-all below, which recurses without
            // ever emitting a `RoutineDecl` — the ENTIRE shared body (including calls that
            // compile into every build) had no routine, no edges, no diagnostics: the
            // fleet-confirmed H-6 defect. `lower_routine`'s body extraction has a dedicated
            // fallback for `preproc_split_procedure_preamble`, whose trailing `code_block` is
            // a BARE child (no `body` field) — see that fallback's doc.
            | RawKind::PreprocSplitProcedure
            | RawKind::PreprocSplitProcedurePreamble => {
                // `interface_procedure` (receiver-closure plan, Task 1): a SIGNATURE-ONLY
                // procedure declaration — no body, no access modifier — used by BOTH
                // `interface_body` and `controladdin_body` (the same grammar rule; a
                // controladdin's AL-callable procedures are declared this way, e.g.
                // `procedure InitEditor(x: Text)` with no trailing `begin/end` or even a
                // semicolon). Previously UNHANDLED here — silently invisible to
                // `RoutineDecl`/`RoutineNode` extraction (fell into the `_` catch-all
                // below, which never emits a routine) — meaning a controladdin's declared
                // procedure surface could never be checked at all. `lower_routine` already
                // degrades gracefully for the fields this node lacks: `FieldName::Modifier`
                // absent → `access_modifier: None`; `FieldName::Body` absent (or not a
                // `CodeBlock`) → `body: None`. An `event` declaration inside the SAME
                // body is a DISTINCT grammar node (`RawKind::EventDeclaration`) that this
                // match still does not handle — it falls to the `_` catch-all and stays
                // correctly unrepresented as a `RoutineDecl` (events are never
                // AL-callable, so the gate's "declared procedures" set must never
                // include them).
                //
                // `in_dataset_modify_context` is only ever meaningful when the enclosing
                // member is itself a `modify_modification` (an actual `dataitem(...)`
                // block already threads its own table via `dataitem_table` above — this
                // flag exists purely for the resolve-time fallback that case doesn't
                // need). See `RoutineDecl::in_dataset_modify_context`'s doc.
                //
                // DELIBERATELY `ModifyModification`-only, and it must NOT be folded into
                // a shared "is a modify wrapper" helper alongside the arm below: a
                // `modify_action_modification` is an ACTIONS-section modify and is never
                // report-dataset context, so widening this gate would hand the resolver's
                // dataitem-map fallback a context that does not exist.
                let in_dataset_modify_context =
                    dataset_ctx && member.is_some_and(|m| m.kind() == RawKind::ModifyModification);
                out.push((
                    child,
                    std::mem::take(&mut pending),
                    dataitem_table.map(str::to_string),
                    member,
                    in_dataset_modify_context,
                ));
            }
            RawKind::ReportDataitem => {
                pending.clear();
                // The innermost enclosing dataitem wins — including when its own source
                // table is absent/unparseable (→ None, NOT the outer table). Mirrors the
                // legacy `report_dataitem_source_table`, which takes the first (innermost)
                // enclosing dataitem's `table_name?` and stops (never inherits an outer).
                // A dataitem is also a named member → its triggers' enclosing member.
                // Being inside an actual dataitem is inherently dataset context (`true`
                // unconditionally — defensive, in practice already `true` by construction:
                // `report_dataitem` only appears under `dataset_section`/`report_body`).
                let inner = dataitem_table_name(child, source);
                collect_routines(child, inner.as_deref(), Some(child), true, source, out);
            }
            RawKind::DatasetSection => {
                pending.clear();
                collect_routines(child, dataitem_table, member, true, source, out);
            }
            RawKind::RequestpageSection => {
                // REQUESTPAGE ISOLATION (binding, dataitem-receivers plan round-1
                // addendum): force dataset context OFF regardless of the ambient value —
                // a requestpage trigger (or a requestpage/layout `modify()`) must NEVER
                // be treated as report-dataset context, even under a pathological/
                // decompiled nesting the real AL compiler would never accept ("parse
                // structure, don't validate").
                pending.clear();
                collect_routines(child, dataitem_table, member, false, source, out);
            }
            RawKind::ModifyModification | RawKind::ModifyActionModification => {
                pending.clear();
                // Both `modify_modification` (fields/dataset/layout) and
                // `modify_action_modification` (an `actions` section) carry the modified
                // member's name in their `target` field, not `name` — always treated as
                // a named member wrapper (unlike the generic gate below, no `Name`-field
                // probe needed). See `lower_routine`'s enclosing-member fallback for the
                // name extraction. THIS arm is the load-bearing gate: without it the
                // generic arm below inherits the outer member and the fallback never
                // sees the wrapper at all.
                collect_routines(child, dataitem_table, Some(child), dataset_ctx, source, out);
            }
            _ => {
                pending.clear();
                // A named, non-object, non-`_body` node becomes the enclosing MEMBER for
                // routines in its subtree (mirrors `enclosing_member_of`: a trigger's
                // parent — stepping up a `_body` wrapper — is the member, unless that is
                // the object). Sections (no `name`) and `_body` wrappers inherit.
                let child_member = if child.field(FieldName::Name).is_some()
                    && object_kind_of(child.kind()).is_none()
                    && !child.kind_str().ends_with("_body")
                {
                    Some(child)
                } else {
                    member
                };
                collect_routines(
                    child,
                    dataitem_table,
                    child_member,
                    dataset_ctx,
                    source,
                    out,
                );
            }
        }
    }
}

/// The unquoted source-table name of a `report_dataitem(Name; "Source Table")` node
/// (its `table_name` field). `None` if absent.
fn dataitem_table_name(node: RawNode, source: &str) -> Option<String> {
    node.field(FieldName::TableName)
        .map(|n| ident_text(n, source))
        .filter(|s| !s.is_empty())
}

/// Collect object-level var declarations, descending preproc wrappers (both
/// branches) but NOT routines/sections-with-their-own-scope.
fn collect_globals(node: RawNode, source: &str, out: &mut Vec<VarDecl>) {
    match node.kind() {
        RawKind::VarSection => extract_var_section(node, source, out),
        _ if is_preproc_wrapper(node) => {
            for c in node.named_children() {
                collect_globals(c, source, out);
            }
        }
        _ => {}
    }
}

#[allow(clippy::too_many_arguments)] // 7 pre-existing params + `in_dataset_modify_context`
// (dataitem-receivers plan, Task 1); each is a distinct piece of context
// `collect_routines`'s DFS threads down — grouping would obscure the call
// site, mirrors `infer_receiver_type`'s identical precedent.
fn lower_routine<'t>(
    node: RawNode<'t>,
    attr_items: Vec<RawNode<'t>>,
    dataitem_source_table: Option<String>,
    member: Option<RawNode<'t>>,
    in_dataset_modify_context: bool,
    source: &str,
    ir: &mut Ir,
    issues: &mut Vec<SyntaxIssue>,
) -> RoutineDecl {
    // Enclosing-member capture (E1): a member wrapper with a `name` → its (stripped)
    // name + the wrapper's origin (range). The engine unescapes the name + anchors.
    //
    // The two `modify_*` wrappers carry the modified member's name in their `target`
    // field, not `name` (neither `RawModifyModification` nor
    // `RawModifyActionModification` has a `name()` accessor at all), so they need a
    // fallback. Deliberately scoped to THOSE kinds only: the sibling `add*`/`move*`
    // modification nodes also carry a `target`, but a wrapper of that family's target is
    // never the declaring member of a routine in its body — either the body declares
    // that member itself (`addlast(area) { action(X) { trigger … } }`, where the ordinary
    // name gate finds `action(X)`) or the routine sits directly in the body and has no
    // declaring member at all (`None`, pinned by
    // `addlast_action_modification_target_is_not_an_enclosing_member` — which pins the
    // ARM list above, since this fallback is never reached for an `add*` node). `modify` is the
    // only form whose target IS the member the body belongs to. (`add`/`addfirst`/
    // `addlast` name a CONTAINER and `addfirst_views`/`addlast_views` have no target at
    // all, so "an anchor naming a different member" — what this comment used to say —
    // was not even true of the family.)
    //
    // The kind match here is belt-and-braces: the `collect_routines` arm above is what
    // decides these nodes are members, so the fallback only ever sees a node that arm
    // passed. The empty filter is NOT belt-and-braces: a malformed `modify()` parses with
    // a PRESENT, zero-width `target`, and a nameless `action()` does the same on the
    // `name` path. NOT tree-sitter error recovery, though it looks like it: both files
    // parse CLEAN — no `ERROR`, no `MISSING`, `has_error()` false — so `ParseStatus`
    // stays `Clean`, `parse_incomplete` stays false, and NOTHING downstream flags them.
    // (The grammar accepting a zero-width `identifier` in a required field looks like a
    // `tree-sitter-al` defect; not this layer's to fix, and the guard is needed either
    // way.) `Some("")` takes the discriminated arm of `to_stable_routine_id_from_parts`
    // and mints a different, meaningless stable id, so it must degrade to `None`.
    // Filtering the whole `enclosing_member` covers both paths with one expression
    // (mirroring `dataitem_table_name`, which filters for exactly this reason). Dropping
    // the member also drops its RANGE, so such a routine loses `enclosing_member_range`
    // and `originating_object` too: it keeps `trigger-page` (inserted unconditionally)
    // but loses `page-action` and its evidence containment range. That is the intended
    // trade — a meaningless id is worse than a missing anchor.
    let enclosing_member = member
        .and_then(|m| {
            let name_node = m.field(FieldName::Name).or_else(|| {
                if matches!(
                    m.kind(),
                    RawKind::ModifyModification | RawKind::ModifyActionModification
                ) {
                    m.field(FieldName::Target)
                } else {
                    None
                }
            });
            name_node.map(|n| (ident_text(n, source), origin_of(m)))
        })
        .filter(|(t, _)| !t.is_empty());
    let kind = if node.kind() == RawKind::TriggerDeclaration {
        RoutineKind::Trigger
    } else {
        RoutineKind::Procedure
    };

    // Attributes: lowercased names (for classify_kind / control-context guards) +
    // the full parsed form (name + raw text + lowered argument exprs).
    let mut attributes: Vec<String> = Vec::new();
    let mut attributes_parsed: Vec<crate::ir::AttributeIr> = Vec::new();
    for item in attr_items {
        let Some(content) = item.field(FieldName::Attribute) else {
            continue;
        };
        let Some(name_node) = content.field(FieldName::Name) else {
            continue;
        };
        let raw_name = name_node.text(source).trim().to_string();
        attributes.push(raw_name.fold_identifier());
        let mut args = Vec::new();
        if let Some(args_node) = content.field(FieldName::Arguments) {
            for list in args_node.named_children() {
                if list.kind() == RawKind::AttributeArgumentList {
                    // H-8: a comment interleaved between args must never occupy a
                    // positional slot — see `structural_children`'s doc (the
                    // `[EventSubscriber(...)]` silent-unregister case this fixes).
                    for arg in structural_children(list) {
                        args.push(lower_expr(arg, ir, issues, source, 0));
                    }
                }
            }
        }
        attributes_parsed.push(crate::ir::AttributeIr {
            name: raw_name,
            raw: item.text(source).to_string(),
            args,
        });
    }
    let name = node
        .field(FieldName::Name)
        .map(|n| routine_name_text(n, source))
        .unwrap_or_default();
    // Origin of the name identifier itself (for the LSP selection_range); fall back to
    // the whole-routine origin when the name node is absent.
    let name_origin = node
        .field(FieldName::Name)
        .map(origin_of)
        .unwrap_or_else(|| origin_of(node));

    let params = node
        .field(FieldName::Parameters)
        .map(|pl| {
            pl.named_children()
                .into_iter()
                .filter(|p| p.kind() == RawKind::Parameter)
                .map(|p| lower_param(p, source))
                .collect()
        })
        .unwrap_or_default();

    // `interface_procedure` (receiver-closure plan, Task 1): its return-type spec
    // lives on a nested, NAMED `interface_procedure_suffix` child (grammar:
    // `optional($.interface_procedure_suffix)`, not inlined — `interface_procedure`
    // itself has no `return_type` field), unlike `_procedure_header`'s
    // `_procedure_return_specification`/`_procedure_named_return`, which ARE
    // hidden/inlined directly onto `node`. Fall back to that child when the direct
    // field is absent, so a controladdin/interface procedure's declared return
    // type is captured with the same fidelity as an ordinary procedure's.
    let return_type = node
        .field(FieldName::ReturnType)
        .or_else(|| {
            node.named_children()
                .into_iter()
                .find(|c| c.kind() == RawKind::InterfaceProcedureSuffix)
                .and_then(|suffix| suffix.field(FieldName::ReturnType))
        })
        .map(|n| n.text(source).trim().to_string());

    // Named-return-value binding (T3, receiver-closure-and-arg-increments plan):
    // `procedure X() Ret: Record Y` — grammar's `_procedure_named_return` sets BOTH
    // `return_value` (the binding name) and `return_type` (captured above) together;
    // an anonymous `: Type` return sets neither. Previously discarded entirely — the
    // binding name never made it past this function, so `Ret.Get(...)` mid-body had no
    // way to type `Ret`. `ident_text` strips the outer quotes for a QUOTED binding name
    // (`"My Result": Record Y`), matching `Param`/`VarDecl` name storage convention.
    // Direct `node.field` (not the `InterfaceProcedureSuffix` fallback above) —
    // `interface_procedure`/`controladdin` signature-only declarations have no body to
    // reference a named return in anyway, so the fallback's extra reach is unneeded
    // here (a `None` there is correct: no binding to synthesize a scoped symbol from).
    let return_name = node
        .field(FieldName::ReturnValue)
        .map(|n| ident_text(n, source))
        .filter(|s| !s.is_empty());

    // Access modifier (`local`/`internal`/`protected`); None = public / trigger.
    let access_modifier = node.field(FieldName::Modifier).and_then(|m| {
        match m.text(source).trim().to_ascii_lowercase().as_str() {
            "local" => Some("local".to_string()),
            "internal" => Some("internal".to_string()),
            "protected" => Some("protected".to_string()),
            _ => None,
        }
    });

    // Locals: var_section child(ren) of the routine (+ preproc-wrapped).
    let mut locals = Vec::new();
    for child in node.named_children() {
        collect_globals(child, source, &mut locals);
    }

    let body = if let Some(cb) = node
        .field(FieldName::Body)
        .filter(|b| b.kind() == RawKind::CodeBlock)
    {
        Some(lower_code_block(cb, ir, issues, source, 0))
    } else if node.kind() == RawKind::PreprocSplitProcedurePreamble {
        // `preproc_split_procedure_preamble` (H-6): unlike `_routine_regular_body`'s
        // `field('body', $.code_block)`, this shape's shared trailing `code_block` (the
        // ONE body after `#endif`) is a BARE child with no field tag at all — VERIFIED
        // against the pinned grammar (`tree-sitter parse`), so `.field(Body)` above always
        // misses it for this one kind. Fall back to the last `CodeBlock`-kind child (there
        // is exactly one, by construction).
        node.named_children()
            .into_iter()
            .rev()
            .find(|c| c.kind() == RawKind::CodeBlock)
            .map(|cb| lower_code_block(cb, ir, issues, source, 0))
    } else {
        // `preproc_split_procedure_body` / `preproc_split_complete_body` (T1.4 review,
        // sibling-gap fix): unlike `preproc_split_procedure_preamble` above, `node.kind()`
        // is STILL plain `Procedure`/`TriggerDeclaration` for these two shapes — only the
        // BODY position is a `#if`-guarded choice (grammar: `choice($._routine_regular_body,
        // $.preproc_split_procedure_body, $.preproc_split_complete_body)`). Both wrapper
        // rules are NAMED (non-inlined), so neither one's `field('body', ..)` flattens onto
        // `node` itself — `node.field(Body)` above is unconditionally `None` for both,
        // and the `_preamble` branch above does not apply (`node.kind()` never becomes
        // one of these two). Recover directly from the wrapper child instead.
        node.named_children()
            .into_iter()
            .find(|c| {
                matches!(
                    c.kind(),
                    RawKind::PreprocSplitProcedureBody | RawKind::PreprocSplitCompleteBody
                )
            })
            .map(|wrapper| lower_preproc_split_routine_body(wrapper, ir, issues, source, 0))
    };

    RoutineDecl {
        kind,
        name,
        name_origin,
        params,
        return_type,
        return_name,
        locals,
        attributes,
        attributes_parsed,
        access_modifier,
        parse_incomplete: node.has_error(),
        dataitem_source_table,
        enclosing_member,
        in_dataset_modify_context,
        body,
        origin: origin_of(node),
    }
}

/// Recover a `preproc_split_procedure_body` / `preproc_split_complete_body` routine
/// body (T1.4 review, sibling-gap fix). Both are NAMED (non-inlined) choice arms of
/// `procedure`/`trigger_declaration`'s body position, so — unlike `_routine_regular_body`,
/// whose `field('body', code_block)` flattens straight onto the routine node — the
/// routine node's OWN `field(Body)` is always `None` for these two shapes; the real
/// content is one level down, on `wrapper`.
///
/// Both shapes place MULTIPLE `field('body', ..)`-tagged `statement_block` children
/// directly on `wrapper` — grammar-VERIFIED with `tree-sitter parse` for both:
/// `preproc_split_complete_body`'s `#if`/`#elif`/`#else` arms (each a COMPLETE,
/// mutually-exclusive body — grammar comment: "the entire body differs across
/// branches") AND `preproc_split_procedure_body`'s `#if`-branch content plus its
/// trailing SHARED tail (compiled into every build, after `#endif`). `field(Body)` is
/// SINGULAR — it returns only the first — so a body position here is Vec-shaped, not a
/// forced-singular slot; `children_by_field(Body)` is the correct read regardless of
/// which of the two wrapper kinds this is. Union-reading ALL of them (rather than only
/// the first) is required for call-graph soundness: every arm's calls are real,
/// reachable calls under SOME build, and this codebase's dominant `#if`-handling policy
/// is exactly this superset union-read (see `is_preproc_wrapper`'s doc) — mirrored by
/// the pre-existing two-`RoutineDecl` precedent for a `#if`-split HEADER
/// (`preproc_both_arms_distinct_signature_yield_two_routine_decls`). Taking only the
/// first branch here would silently drop the `#else`/`#elif` arms' calls entirely.
///
/// Always records a `SyntaxIssue` (never the ordinary path — the exact conditional
/// control flow across the split is not preserved), even when the recovered content is
/// empty.
fn lower_preproc_split_routine_body(
    wrapper: RawNode,
    ir: &mut Ir,
    issues: &mut Vec<SyntaxIssue>,
    source: &str,
    depth: u32,
) -> BlockId {
    push_unlowered_issue(wrapper, "routine body", issues);
    let mut items = Vec::new();
    for body in wrapper.children_by_field(FieldName::Body) {
        for child in body.named_children() {
            lower_block_child(child, ir, issues, source, &mut items, depth);
        }
    }
    ir.add_block(Block {
        items,
        origin: origin_of(wrapper),
    })
}

fn lower_param(node: RawNode, source: &str) -> Param {
    let by_ref = node.field(FieldName::Modifier).is_some();
    let name = node
        .field(FieldName::Name)
        .map(|n| ident_text(n, source))
        .unwrap_or_default();
    let ty = node
        .field(FieldName::Type)
        .map(|n| n.text(source).trim().to_string());
    Param {
        name,
        by_ref,
        ty,
        origin: origin_of(node),
    }
}

/// A `var_section` → its `var_body` → one `VarDecl` per declared name (`A, B: T`
/// yields two). `temporary` detection is refined in the parity step (false here).
fn extract_var_section(section: RawNode, source: &str, out: &mut Vec<VarDecl>) {
    let Some(body) = section.field(FieldName::Body) else {
        return;
    };
    for decl in body.named_children() {
        match decl.kind() {
            RawKind::VariableDeclaration => {
                let type_node = decl.field(FieldName::Type);
                let ty = type_node.map(|n| n.text(source).trim().to_string());
                let temporary = type_node
                    .map(|t| contains_kind(t, RawKind::TemporaryKeyword))
                    .unwrap_or(false);
                let names = decl.children_by_field(FieldName::Name);
                if names.is_empty() {
                    // single unnamed-by-field fallback: skip (no name to record)
                    continue;
                }
                for nm in names {
                    out.push(VarDecl {
                        name: ident_text(nm, source),
                        ty: ty.clone(),
                        temporary,
                        origin: origin_of(decl),
                    });
                }
            }
            _ if is_preproc_wrapper(decl) => {
                for c in decl.named_children() {
                    if c.kind() == RawKind::VariableDeclaration {
                        // shallow: flatten preproc-wrapped declarations (both branches)
                        let type_node = c.field(FieldName::Type);
                        let ty = type_node.map(|n| n.text(source).trim().to_string());
                        let temporary = type_node
                            .map(|t| contains_kind(t, RawKind::TemporaryKeyword))
                            .unwrap_or(false);
                        for nm in c.children_by_field(FieldName::Name) {
                            out.push(VarDecl {
                                name: ident_text(nm, source),
                                ty: ty.clone(),
                                temporary,
                                origin: origin_of(c),
                            });
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

// ---- body lowering (statements + expressions) ----
//
// First cut: preproc-wrapped statements are FLATTENED in document order (legacy
// recursively descends; the structured-vs-flat choice is settled by Phase 1
// dual-run). Unmodelled nodes become `Unknown` + a `SyntaxIssue` — never dropped.

/// `code_block` → its `statement_block` (v3) → a `Block`.
///
/// `preproc_split_code_block_end` (T1.4 review, sibling-gap fix) — grammar:
/// `code_block`'s closing arm is `choice(seq(end, ';'), $.preproc_split_code_block_end)`
/// — is a SIBLING of the (optional) `body` field, never nested inside it, present
/// whenever the closing `end` itself differs across `#if`/`#else` branches. The old
/// `cb.field(Body).unwrap_or(cb)` fallback only ever exposed it when `body` was ABSENT
/// (then `inner == cb`, so `lower_stmt_seq` walked `cb`'s own children and happened to
/// reach it); a `code_block` with real LEADING content before the split took the
/// `Some(body)` arm and never looked at the sibling again — its content (everything
/// after the split, including a `#else`-guarded tail AND — grammar-verified via
/// `tree-sitter parse` — a following unconditional `else` clause the grammar folds into
/// the SAME node) was silently dropped. Always checked now, regardless of whether
/// `body` was present, and recovered through the SAME generic dispatcher
/// [`lower_unmodelled_stmt`] uses for its other flat/fragmented shapes (filtering
/// `is_preproc_scaffold` first, since this sibling is walked directly rather than via
/// `lower_stmt`'s catch-all) — folded flat into the SAME block, not wrapped in an extra
/// synthetic statement.
fn lower_code_block(
    cb: RawNode,
    ir: &mut Ir,
    issues: &mut Vec<SyntaxIssue>,
    source: &str,
    depth: u32,
) -> BlockId {
    let mut items = Vec::new();
    if let Some(body) = cb.field(FieldName::Body) {
        for child in body.named_children() {
            lower_block_child(child, ir, issues, source, &mut items, depth);
        }
    }
    if let Some(split_end) = cb
        .named_children()
        .into_iter()
        .find(|c| c.kind() == RawKind::PreprocSplitCodeBlockEnd)
    {
        push_unlowered_issue(split_end, "statement", issues);
        for c in structural_children(split_end) {
            if is_preproc_scaffold(c.kind()) {
                continue;
            }
            lower_block_child(c, ir, issues, source, &mut items, depth);
        }
    }
    ir.add_block(Block {
        items,
        origin: origin_of(cb),
    })
}

/// A branch position (then/else/loop body): a `code_block`, a bare `statement_block`,
/// or a single statement. Always normalized to a `Block`.
fn lower_branch(
    node: RawNode,
    ir: &mut Ir,
    issues: &mut Vec<SyntaxIssue>,
    source: &str,
    depth: u32,
) -> BlockId {
    match node.kind() {
        RawKind::CodeBlock => lower_code_block(node, ir, issues, source, depth),
        RawKind::StatementBlock => lower_stmt_seq(node, origin_of(node), ir, issues, source, depth),
        _ => {
            let mut items = Vec::new();
            lower_block_child(node, ir, issues, source, &mut items, depth);
            ir.add_block(Block {
                items,
                origin: origin_of(node),
            })
        }
    }
}

fn lower_stmt_seq(
    container: RawNode,
    origin: Origin,
    ir: &mut Ir,
    issues: &mut Vec<SyntaxIssue>,
    source: &str,
    depth: u32,
) -> BlockId {
    let mut items = Vec::new();
    for child in container.named_children() {
        lower_block_child(child, ir, issues, source, &mut items, depth);
    }
    ir.add_block(Block { items, origin })
}

fn lower_block_child(
    node: RawNode,
    ir: &mut Ir,
    issues: &mut Vec<SyntaxIssue>,
    source: &str,
    items: &mut Vec<BlockItem>,
    depth: u32,
) {
    if is_preproc_wrapper(node) {
        for c in node.named_children() {
            lower_block_child(c, ir, issues, source, items, depth);
        }
        return;
    }
    // Statement-position keyword tokens are named children that leak from certain
    // wrappers: `begin`/`end` from an EMPTY `code_block` (v3 emits no `statement_block`
    // body, so `lower_code_block` falls back to the code_block itself), `else` from a
    // `case_else_branch` (lowered by iterating its direct children), etc. None are
    // statements — skip them, exactly as legacy `block_statements`/CFN `build_block` do.
    // Trivia (`comment` / `multiline_comment` / `pragma`) are named children of a
    // block but are NOT statements — skip them (legacy CFN `build_block` does too).
    // Lowering them as `Unknown` produced phantom "other" CFN nodes. `#region`/
    // `#endregion` (H-7, Tier-1.4 preproc plan) are the same kind of pure editor-fold
    // marker — previously MISSING here, so one reaching this point fell through to
    // `lower_stmt`'s catch-all and became a phantom `Unknown` statement with a
    // misleading "unlowered statement" issue for a marker that carries no content at
    // all.
    if matches!(
        node.kind(),
        RawKind::EmptyStatement
            | RawKind::BeginKeyword
            | RawKind::EndKeyword
            | RawKind::IfKeyword
            | RawKind::ThenKeyword
            | RawKind::ElseKeyword
            | RawKind::CaseKeyword
            | RawKind::OfKeyword
            | RawKind::RepeatKeyword
            | RawKind::UntilKeyword
            | RawKind::WhileKeyword
            | RawKind::ForKeyword
            | RawKind::DoKeyword
            | RawKind::ForeachKeyword
            | RawKind::InKeyword
            | RawKind::Comment
            | RawKind::MultilineComment
            | RawKind::Pragma
            | RawKind::PreprocRegion
            | RawKind::PreprocEndregion
    ) {
        return;
    }
    // A bare identifier in statement position is NOT a confident call: it is either
    // ERROR-recovery debris (a token rescued after a syntax error) or a parenless call
    // that omitted its `;` (a semicolon-less final statement). A real parenless call
    // owns its `;` and reduces to `call_statement` (handled in lower_stmt). We do not
    // fabricate a call edge from a bare identifier — that would manufacture spurious
    // `unknown` edges from parse-error debris (the moat-polluting case the grammar's
    // `call_statement` node was added to prevent). The residual (a genuine parenless
    // call written without a trailing `;`) is rare, never produces a FALSE edge, and is
    // still no worse than the legacy walk, which captured no parenless calls at all.
    if matches!(node.kind(), RawKind::Identifier | RawKind::QuotedIdentifier) {
        return;
    }
    // A `call_statement` whose subtree carries a parse error (e.g. tree-sitter
    // synthesized a MISSING `;`) is not a confident call either — skip it.
    if node.kind() == RawKind::CallStatement && node.has_error() {
        return;
    }
    let sid = lower_stmt(node, ir, issues, source, depth + 1);
    items.push(BlockItem::Stmt(sid));
}

fn lower_stmt(
    node: RawNode,
    ir: &mut Ir,
    issues: &mut Vec<SyntaxIssue>,
    source: &str,
    depth: u32,
) -> StmtId {
    let origin = origin_of(node);
    // Depth-budget check (T2.1, stack-overflow hardening): a pathologically deep
    // statement nesting (generated or hostile input, never real AL source) must
    // fail closed here rather than keep recursing toward a native stack overflow
    // — see [`MAX_LOWER_DEPTH`]'s doc.
    if depth > MAX_LOWER_DEPTH {
        issues.push(SyntaxIssue {
            message: format!(
                "statement nesting exceeds the {MAX_LOWER_DEPTH}-level lowering depth \
                 budget at `{}` — degrading to Unknown rather than recursing further \
                 (pathological input, not real AL source)",
                node.kind_str()
            ),
            origin: origin.clone(),
        });
        return ir.add_stmt(Stmt {
            kind: StmtKind::Unknown,
            origin,
        });
    }
    // A parenless no-arg call statement (`Initialize;`) — the grammar wraps a bare
    // identifier that owns its `;` in a `call_statement` (distinct from ERROR-recovery
    // debris, which lacks a terminator and stays a raw identifier — never reaching here,
    // see `lower_block_child`). Lower its `function` to a Call with empty args, ANCHORED
    // on the callee identifier (NOT the wrapper) so the call's source anchor
    // (endColumn/syntaxKind) is byte-identical to a parenless call's bare identifier —
    // preserving golden parity with the pre-grammar form.
    if node.kind() == RawKind::CallStatement {
        let callee = node.field(FieldName::Function).unwrap_or(node);
        let function = lower_expr(callee, ir, issues, source, depth + 1);
        let call = ir.add_expr(Expr {
            kind: ExprKind::Call {
                function,
                args: Vec::new(),
            },
            origin: origin_of(callee),
        });
        return ir.add_stmt(Stmt {
            kind: StmtKind::Call(call),
            origin: origin_of(callee),
        });
    }
    let kind = match node.kind() {
        RawKind::AssignmentStatement => {
            let target = lower_opt_field(node, FieldName::Left, ir, issues, source, depth);
            let value = lower_opt_field(node, FieldName::Right, ir, issues, source, depth);
            StmtKind::Assignment { target, value }
        }
        RawKind::CallExpression => StmtKind::Call(lower_expr(node, ir, issues, source, depth + 1)),
        // Parenless member / subscript call statements (`Rec.Find;`, `X[1];`) parse as a
        // bare member/subscript in statement position (the grammar leaves these UNCHANGED
        // — only a bare identifier became `call_statement`). AL has no field-access
        // statement, so these ARE calls — normalize to a Call with empty args.
        RawKind::MemberExpression | RawKind::SubscriptExpression => {
            let function = lower_expr(node, ir, issues, source, depth + 1);
            let call = ir.add_expr(Expr {
                kind: ExprKind::Call {
                    function,
                    args: Vec::new(),
                },
                origin: origin_of(node),
            });
            StmtKind::Call(call)
        }
        RawKind::IfStatement => StmtKind::If {
            cond: lower_opt_field(node, FieldName::Condition, ir, issues, source, depth),
            then_block: lower_branch_field(node, FieldName::ThenBranch, ir, issues, source, depth),
            else_block: node
                .field(FieldName::ElseBranch)
                .map(|b| lower_branch(b, ir, issues, source, depth)),
        },
        RawKind::WhileStatement => StmtKind::While {
            cond: lower_opt_field(node, FieldName::Condition, ir, issues, source, depth),
            body: lower_branch_field(node, FieldName::Body, ir, issues, source, depth),
        },
        RawKind::RepeatStatement => StmtKind::Repeat {
            body: lower_branch_field(node, FieldName::Body, ir, issues, source, depth),
            until: lower_opt_field(node, FieldName::Condition, ir, issues, source, depth),
        },
        RawKind::ForStatement => {
            let down = node
                .field(FieldName::Direction)
                .map(|d| d.text(source).eq_ignore_ascii_case("downto"))
                .unwrap_or(false);
            StmtKind::For {
                var: lower_opt_field(node, FieldName::Variable, ir, issues, source, depth),
                from: lower_opt_field(node, FieldName::Start, ir, issues, source, depth),
                to: lower_opt_field(node, FieldName::End, ir, issues, source, depth),
                down,
                body: lower_branch_field(node, FieldName::Body, ir, issues, source, depth),
            }
        }
        RawKind::ForeachStatement => StmtKind::Foreach {
            var: lower_opt_field(node, FieldName::Variable, ir, issues, source, depth),
            iterable: lower_opt_field(node, FieldName::Iterable, ir, issues, source, depth),
            body: lower_branch_field(node, FieldName::Body, ir, issues, source, depth),
        },
        RawKind::WithStatement => StmtKind::With {
            receiver: lower_opt_field(node, FieldName::Record, ir, issues, source, depth),
            body: lower_branch_field(node, FieldName::Body, ir, issues, source, depth),
        },
        RawKind::CaseStatement => {
            let scrutinee = lower_opt_field(node, FieldName::Expression, ir, issues, source, depth);
            let (branches, else_block) = lower_case_body(node, ir, issues, source, depth);
            StmtKind::Case {
                scrutinee,
                branches,
                else_block,
            }
        }
        RawKind::AsserterrorStatement => StmtKind::AssertError(lower_branch_field(
            node,
            FieldName::Body,
            ir,
            issues,
            source,
            depth,
        )),
        RawKind::ExitStatement => StmtKind::Exit(
            node.field(FieldName::ReturnValue)
                .map(|e| lower_expr(e, ir, issues, source, depth + 1)),
        ),
        RawKind::BreakStatement => StmtKind::Break,
        RawKind::ContinueStatement => StmtKind::Continue,
        RawKind::CodeBlock => StmtKind::Block(lower_code_block(node, ir, issues, source, depth)),
        _ => lower_unmodelled_stmt(node, &origin, ir, issues, source, depth),
    };
    ir.add_stmt(Stmt { kind, origin })
}

/// Preprocessor directive/marker nodes belonging to a preproc-split/guarded statement
/// wrapper (H-7, Tier-1.4 preproc plan) that never carry lowerable AL content
/// themselves: the `#if`/`#elif`/`#else`/`#endif` directive tokens (their OWN
/// `condition` field is a preprocessor-SYMBOL expression — `preproc_and_expression` /
/// `preproc_or_expression` / `preproc_not_expression` — never real AL code, since it
/// evaluates macro symbols like `CLEAN24`, not AL identifiers) and the `begin`/`end`
/// split markers used by the flat `if...then begin`-family constructs.
fn is_preproc_scaffold(k: RawKind) -> bool {
    matches!(
        k,
        RawKind::PreprocIf
            | RawKind::PreprocElif
            | RawKind::PreprocElse
            | RawKind::PreprocEndif
            | RawKind::PreprocSplitBegin
            | RawKind::PreprocSplitEnd
            | RawKind::PreprocSplitBraceClose
            | RawKind::PreprocSplitBraceCloseIfOnly
    )
}

/// Shared `SyntaxIssue` for every "grammar shape not natively modelled, recovering
/// shared/first-branch content generically" recovery path (H-7/T1.4-review doctrine:
/// never a silent drop). `noun` distinguishes what kind of construct is being
/// recovered (a statement vs. a routine body) in the message text; callers:
/// [`lower_unmodelled_stmt`], [`lower_code_block`]'s `preproc_split_code_block_end`
/// sibling recovery, and [`lower_preproc_split_routine_body`].
fn push_unlowered_issue(node: RawNode, noun: &str, issues: &mut Vec<SyntaxIssue>) {
    issues.push(SyntaxIssue {
        message: format!(
            "unlowered {noun} `{}` — recovering shared content for call-graph \
             completeness (exact preprocessor-conditional control flow not modelled)",
            node.kind_str()
        ),
        origin: origin_of(node),
    });
}

/// Recover a preproc-split/guarded STATEMENT-position construct (H-7, Tier-1.4 preproc
/// plan) — the eight grammar shapes `lower_stmt`'s main match does not natively model
/// (`preproc_split_if_statement`, `preproc_guarded_statement`,
/// `preproc_split_if_else_statement`, `preproc_split_if_then_begin`,
/// `preproc_split_if_begin_asymmetric`, `preproc_split_if_then_begin_else_shared`,
/// `preproc_split_if_begin_else`, `preproc_split_call_statement`). Never a silent drop:
/// a `SyntaxIssue` is always recorded (this is never the ordinary path), but the
/// returned `StmtKind` still carries every call the shared, always-compiled source
/// contains.
///
/// NOTE (T1.4 review fix): `preproc_split_code_block_end` — the closing `end` of a
/// `code_block` itself split across `#if`/`#else` — is NOT one of these shapes. It is
/// always a SIBLING of the `code_block`'s own (optional) `body` field, never a
/// statement-position node reachable through `lower_stmt` in its own right, and is
/// recovered directly by [`lower_code_block`] instead — see that function's doc. (An
/// earlier version of this comment incorrectly claimed it landed here "recovered
/// generically through `lower_block_child`"; that was only ever true when the
/// enclosing `code_block` had NO leading content, via a since-removed
/// `cb.field(Body).unwrap_or(cb)` fallback that happened to also expose this sibling as
/// one of `cb`'s own children.)
///
/// Three shapes get PRECISE modelling:
/// - `preproc_split_call_statement` is unambiguously a single call by construction (its
///   argument list, not its identity, is what's split) — the callee is the first
///   non-scaffold child, every remaining non-scaffold child is a unioned argument —
///   reconstructed as a real `StmtKind::Call`.
/// - `preproc_split_if_statement` / `preproc_guarded_statement` /
///   `preproc_split_if_else_statement` expose clean `condition` / `then_branch` /
///   `else_branch` fields (flattened up from their inlined `_preproc_if_header` /
///   `_then_branch` / `_else_branch` sub-rules — VERIFIED against the pinned grammar).
///   `condition`/`then_branch` may repeat once per `#if`/`#elif`/`#else` arm; `.field()`
///   takes the FIRST, mirroring the established first-branch-wins policy
///   (`PreprocSplitDeclaration`'s name/id resolution). Reconstructed as a real
///   `StmtKind::If`, indistinguishable from an ordinary `if`.
///
/// Everything else — the remaining begin/end-fragmented shapes
/// (`preproc_split_if_then_begin` and its asymmetric/shared/begin-else siblings), any
/// arm's content NOT consumed by the `If` reconstruction above (extra `#elif`/`#else`
/// arms, `preproc_guarded_statement`'s leading guard statements, a
/// `preproc_fragmented_else_tail`), and any other genuinely-unmodelled preproc/unknown
/// statement kind — holds its content as a FLAT `repeat($._statement)` run with no
/// field boundary: every leftover non-scaffold, non-trivia child is recovered
/// generically through [`lower_block_child`] (the SAME dispatcher real block content
/// uses, and the SAME dispatcher `lower_code_block`'s sibling recovery uses), so a
/// nested call is never lost even though the exact conditional shape (which statement
/// belongs to which `#if` arm) is not preserved. Genuinely empty recoveries (no
/// leftover content, no `If`) stay `StmtKind::Unknown` — never a fabricated empty
/// block.
fn lower_unmodelled_stmt(
    node: RawNode,
    origin: &Origin,
    ir: &mut Ir,
    issues: &mut Vec<SyntaxIssue>,
    source: &str,
    depth: u32,
) -> StmtKind {
    push_unlowered_issue(node, "statement", issues);

    if node.kind() == RawKind::PreprocSplitCallStatement {
        let mut children = structural_children(node)
            .into_iter()
            .filter(|c| !is_preproc_scaffold(c.kind()));
        return match children.next() {
            Some(callee) => {
                let function = lower_expr(callee, ir, issues, source, depth + 1);
                let args = children
                    .map(|a| lower_expr(a, ir, issues, source, depth + 1))
                    .collect();
                let call = ir.add_expr(Expr {
                    kind: ExprKind::Call { function, args },
                    origin: origin.clone(),
                });
                StmtKind::Call(call)
            }
            None => StmtKind::Unknown,
        };
    }

    let if_shape = matches!(
        node.kind(),
        RawKind::PreprocSplitIfStatement
            | RawKind::PreprocGuardedStatement
            | RawKind::PreprocSplitIfElseStatement
    );
    let cond_node = if if_shape {
        node.field(FieldName::Condition)
    } else {
        None
    };
    let then_node = if if_shape {
        node.field(FieldName::ThenBranch)
    } else {
        None
    };
    let else_node = if if_shape {
        node.field(FieldName::ElseBranch)
    } else {
        None
    };
    let reconstructed_if = then_node.map(|_| StmtKind::If {
        cond: match cond_node {
            Some(c) => lower_expr(c, ir, issues, source, depth + 1),
            None => ir.add_expr(Expr {
                kind: ExprKind::Unknown,
                origin: origin.clone(),
            }),
        },
        then_block: lower_branch_field(node, FieldName::ThenBranch, ir, issues, source, depth),
        else_block: else_node.map(|b| lower_branch(b, ir, issues, source, depth)),
    });
    let consumed_ids: [Option<usize>; 3] = [
        cond_node.map(|n| n.id()),
        then_node.map(|n| n.id()),
        else_node.map(|n| n.id()),
    ];

    let mut items = Vec::new();
    for c in structural_children(node) {
        if is_preproc_scaffold(c.kind()) || consumed_ids.contains(&Some(c.id())) {
            continue;
        }
        lower_block_child(c, ir, issues, source, &mut items, depth);
    }

    match (reconstructed_if, items.is_empty()) {
        (Some(if_kind), true) => if_kind,
        (Some(if_kind), false) => {
            let if_id = ir.add_stmt(Stmt {
                kind: if_kind,
                origin: origin.clone(),
            });
            items.push(BlockItem::Stmt(if_id));
            StmtKind::Block(ir.add_block(Block {
                items,
                origin: origin.clone(),
            }))
        }
        (None, true) => StmtKind::Unknown,
        (None, false) => StmtKind::Block(ir.add_block(Block {
            items,
            origin: origin.clone(),
        })),
    }
}

/// Lower `case_body` → (branches, else block).
fn lower_case_body(
    case_node: RawNode,
    ir: &mut Ir,
    issues: &mut Vec<SyntaxIssue>,
    source: &str,
    depth: u32,
) -> (Vec<CaseBranch>, Option<BlockId>) {
    let mut branches = Vec::new();
    let mut else_block = None;
    if let Some(body) = case_node.field(FieldName::Body) {
        collect_case_branches(
            body,
            ir,
            issues,
            source,
            &mut branches,
            &mut else_block,
            depth,
        );
    }
    // The UNCONDITIONAL `case_else_branch` is a DIRECT child of `case_statement` itself
    // (not `case_body`) — only consulted when no `#if`-nested else was already found by
    // `collect_case_branches` (first-match-wins, mirroring `PreprocSplitDeclaration`'s
    // established name/id policy; an unconditional else AND a conditional nested else
    // together in the same `case` is legal AL but vanishingly rare).
    if else_block.is_none()
        && let Some(else_node) = case_node
            .named_children()
            .into_iter()
            .find(|c| c.kind() == RawKind::CaseElseBranch)
    {
        else_block = Some(lower_case_else_branch(else_node, ir, issues, source, depth));
    }
    (branches, else_block)
}

/// Recursive worker for [`lower_case_body`] (H-7, Tier-1.4 preproc plan): walks one
/// `case_body` — or, recursively, one `preproc_conditional_case`'s own flat child list
/// (the grammar does not re-nest per `#if`/`#elif`/`#else` arm) — collecting every
/// branch additively (mirrors the established `implements`-list union-read policy: a
/// branch reachable in AT LEAST one build is a sound over-approximation to include) and
/// the singular `else_block` first-match-wins.
fn collect_case_branches(
    node: RawNode,
    ir: &mut Ir,
    issues: &mut Vec<SyntaxIssue>,
    source: &str,
    branches: &mut Vec<CaseBranch>,
    else_block: &mut Option<BlockId>,
    depth: u32,
) {
    for child in node.named_children() {
        match child.kind() {
            RawKind::CaseBranch => push_case_branch(child, ir, issues, source, branches, depth),
            RawKind::CaseElseBranch => {
                if else_block.is_none() {
                    *else_block = Some(lower_case_else_branch(child, ir, issues, source, depth));
                }
            }
            // `#if`-conditional case content: every arm's `case_branch`/`case_else_branch`
            // children are direct, FLAT children of this ONE wrapper (VERIFIED against the
            // pinned grammar — `preproc_conditional_case` is not re-nested per arm) —
            // union-read, recurse into the same wrapper node. Previously unmatched here
            // (silently skipped, no trace at all — worse than an empty branch).
            RawKind::PreprocConditionalCase => {
                collect_case_branches(child, ir, issues, source, branches, else_block, depth);
            }
            // "#if adds complete branches + provides a header for the next shared branch":
            // any direct `case_branch` children are complete extra branches (additive,
            // real pattern+body of their own); the wrapper's OWN flattened `pattern`
            // fields (unioned across every `#if`/`#elif`/`#else` arm's header-only
            // pattern, plus the shared trailing pattern) plus its single shared `body`
            // field make ONE more branch. Previously unmatched here (same silent-skip as
            // above).
            RawKind::PreprocSplitCaseExtended => {
                for extra in child
                    .named_children()
                    .into_iter()
                    .filter(|c| c.kind() == RawKind::CaseBranch)
                {
                    push_case_branch(extra, ir, issues, source, branches, depth);
                }
                let patterns = case_patterns(child, ir, issues, source, depth);
                if !patterns.is_empty() || child.field(FieldName::Body).is_some() {
                    let body =
                        lower_branch_field(child, FieldName::Body, ir, issues, source, depth);
                    branches.push(CaseBranch {
                        patterns,
                        body,
                        origin: origin_of(child),
                    });
                }
            }
            _ => {}
        }
    }
}

/// One `case_branch` → a `CaseBranch`. The grammar's `preproc_split_case_branch`
/// alternative still wraps its content in an OUTER `case_branch` node (VERIFIED against
/// the pinned grammar with `tree-sitter parse` — a `choice()` alternative that is
/// itself a single named symbol is NOT unwrapped here), so the outer node's OWN
/// `pattern`/`body` fields are unset in that case — reading them directly fabricates an
/// empty branch (the fleet-confirmed H-7 defect). Descend to the nested
/// `preproc_split_case_branch` (whose flattened `pattern` fields already union every
/// `#if`-arm's extra patterns with the shared trailing pattern, and whose `body` field
/// is the single shared body) before reading fields.
fn push_case_branch(
    case_branch_node: RawNode,
    ir: &mut Ir,
    issues: &mut Vec<SyntaxIssue>,
    source: &str,
    branches: &mut Vec<CaseBranch>,
    depth: u32,
) {
    let effective = case_branch_node
        .named_children()
        .into_iter()
        .find(|c| c.kind() == RawKind::PreprocSplitCaseBranch)
        .unwrap_or(case_branch_node);
    let patterns = case_patterns(effective, ir, issues, source, depth);
    let body = lower_branch_field(effective, FieldName::Body, ir, issues, source, depth);
    branches.push(CaseBranch {
        patterns,
        body,
        origin: origin_of(case_branch_node),
    });
}

/// A branch/extended-split node's `pattern` field values. The `pattern` field binds a
/// single value node per branch value (grammar rule `_case_pattern_item`), so the `,`
/// separators are NOT tagged `pattern`. The `is_named()` filter is kept as
/// defense-in-depth: an anonymous `,` token has no `RawKind` and would panic in
/// `lower_expr`.
fn case_patterns(
    node: RawNode,
    ir: &mut Ir,
    issues: &mut Vec<SyntaxIssue>,
    source: &str,
    depth: u32,
) -> Vec<ExprId> {
    node.children_by_field(FieldName::Pattern)
        .into_iter()
        .filter(|p| p.is_named())
        .map(|p| lower_expr(p, ir, issues, source, depth + 1))
        .collect()
}

/// A `case_else_branch`'s content (the `else` keyword + a `code_block` or bare
/// statements; no `body` field). Lower it like a branch body: a SOLE
/// `code_block`/`statement_block` is unwrapped (`lower_branch`) so we don't double-nest
/// a block inside the else block — matching then/loop branches and the legacy CFN,
/// which builds the code_block child directly.
fn lower_case_else_branch(
    else_node: RawNode,
    ir: &mut Ir,
    issues: &mut Vec<SyntaxIssue>,
    source: &str,
    depth: u32,
) -> BlockId {
    let content: Vec<RawNode> = else_node
        .named_children()
        .into_iter()
        .filter(|c| c.kind() != RawKind::ElseKeyword)
        .collect();
    match content.as_slice() {
        [only] => lower_branch(*only, ir, issues, source, depth),
        _ => lower_stmt_seq(else_node, origin_of(else_node), ir, issues, source, depth),
    }
}

/// Lower a required-expression field; missing → `Unknown` placeholder (recorded).
fn lower_opt_field(
    node: RawNode,
    f: FieldName,
    ir: &mut Ir,
    issues: &mut Vec<SyntaxIssue>,
    source: &str,
    depth: u32,
) -> ExprId {
    match node.field(f) {
        Some(e) => lower_expr(e, ir, issues, source, depth + 1),
        None => {
            let origin = origin_of(node);
            issues.push(SyntaxIssue {
                message: format!("missing `{:?}` on `{}`", f, node.kind_str()),
                origin: origin.clone(),
            });
            ir.add_expr(Expr {
                kind: ExprKind::Unknown,
                origin,
            })
        }
    }
}

fn lower_branch_field(
    node: RawNode,
    f: FieldName,
    ir: &mut Ir,
    issues: &mut Vec<SyntaxIssue>,
    source: &str,
    depth: u32,
) -> BlockId {
    match node.field(f) {
        Some(b) => lower_branch(b, ir, issues, source, depth),
        None => ir.add_block(Block {
            items: Vec::new(),
            origin: origin_of(node),
        }),
    }
}

fn lower_expr(
    node: RawNode,
    ir: &mut Ir,
    issues: &mut Vec<SyntaxIssue>,
    source: &str,
    depth: u32,
) -> ExprId {
    let origin = origin_of(node);
    // Depth-budget check (T2.1, stack-overflow hardening): mirrors `lower_stmt`'s
    // — see [`MAX_LOWER_DEPTH`]'s doc. This is the primary guard for the
    // `x := 1 + 1 + … 50k terms` pathological-expression shape (binary
    // expressions recurse through here directly).
    if depth > MAX_LOWER_DEPTH {
        issues.push(SyntaxIssue {
            message: format!(
                "expression nesting exceeds the {MAX_LOWER_DEPTH}-level lowering depth \
                 budget at `{}` — degrading to Unknown rather than recursing further \
                 (pathological input, not real AL source)",
                node.kind_str()
            ),
            origin: origin.clone(),
        });
        return ir.add_expr(Expr {
            kind: ExprKind::Unknown,
            origin,
        });
    }
    let kind = match node.kind() {
        RawKind::Identifier | RawKind::KeywordIdentifier => {
            ExprKind::Identifier(node.text(source).to_string())
        }
        RawKind::QuotedIdentifier => ExprKind::QuotedIdentifier(ident_text(node, source)),
        RawKind::MemberExpression => {
            let object = lower_opt_field(node, FieldName::Object, ir, issues, source, depth);
            // RAW member text (quotes preserved) — source-faithful; consumers strip
            // when needed. Legacy keeps quotes for var-assignment lhs names.
            let member_node = node.field(FieldName::Member);
            let member = member_node
                .map(|m| m.text(source).to_string())
                .unwrap_or_default();
            let member_origin = member_node
                .map(origin_of)
                .unwrap_or_else(|| origin_of(node));
            ExprKind::Member {
                object,
                member,
                member_origin,
            }
        }
        RawKind::CallExpression => {
            let function = lower_opt_field(node, FieldName::Function, ir, issues, source, depth);
            // H-8: a mid-argument comment (`P(a, /* c */ b)`) must never become a
            // phantom `Unknown` argument — see `structural_children`'s doc.
            let args = node
                .field(FieldName::Arguments)
                .map(|al| {
                    structural_children(al)
                        .into_iter()
                        .map(|a| lower_expr(a, ir, issues, source, depth + 1))
                        .collect()
                })
                .unwrap_or_default();
            ExprKind::Call { function, args }
        }
        RawKind::SubscriptExpression => ExprKind::Index {
            base: lower_opt_field(node, FieldName::Object, ir, issues, source, depth),
            index: lower_opt_field(node, FieldName::Index, ir, issues, source, depth),
        },
        // H-8: a leading comment (`(/* c */ Expr)`) must never be mistaken for the
        // real inner expression — see `structural_children`'s doc.
        RawKind::ParenthesizedExpression => match structural_children(node).into_iter().next() {
            Some(inner) => {
                ExprKind::Parenthesized(lower_expr(inner, ir, issues, source, depth + 1))
            }
            None => ExprKind::Unknown,
        },
        RawKind::UnaryExpression => ExprKind::Unary {
            op: unary_op(node, source),
            operand: lower_opt_field(node, FieldName::Operand, ir, issues, source, depth),
        },
        RawKind::AdditiveExpression
        | RawKind::MultiplicativeExpression
        | RawKind::ComparisonExpression
        | RawKind::LogicalExpression
        | RawKind::InExpression => ExprKind::Binary {
            op: binary_op(node, source),
            lhs: lower_opt_field(node, FieldName::Left, ir, issues, source, depth),
            rhs: lower_opt_field(node, FieldName::Right, ir, issues, source, depth),
        },
        RawKind::RangeExpression => ExprKind::RangeExpr {
            start: lower_opt_field(node, FieldName::Left, ir, issues, source, depth),
            end: lower_opt_field(node, FieldName::Right, ir, issues, source, depth),
        },
        RawKind::QualifiedEnumValue => ExprKind::QualifiedEnum {
            enum_type: lower_opt_field(node, FieldName::EnumType, ir, issues, source, depth),
            value: node
                .field(FieldName::Value)
                .map(|v| ident_text(v, source))
                .unwrap_or_default(),
        },
        RawKind::DatabaseReference => ExprKind::DatabaseReference(node.text(source).to_string()),
        RawKind::Boolean => ExprKind::Literal(Literal::Bool(
            node.text(source).eq_ignore_ascii_case("true"),
        )),
        RawKind::Integer => ExprKind::Literal(Literal::Int(node.text(source).to_string())),
        RawKind::Decimal => ExprKind::Literal(Literal::Decimal(node.text(source).to_string())),
        RawKind::StringLiteral | RawKind::VerbatimString => {
            ExprKind::Literal(Literal::Text(node.text(source).to_string()))
        }
        _ => {
            // Unmodelled expression container (in/is/as expression, list_literal,
            // ternary, …): lower its non-trivia children so nested calls/members are
            // still captured in the arena (completeness). The node itself is Unknown.
            for c in structural_children(node) {
                lower_expr(c, ir, issues, source, depth + 1);
            }
            ExprKind::Unknown
        }
    };
    ir.add_expr(Expr { kind, origin })
}

fn binary_op(node: RawNode, source: &str) -> BinaryOp {
    let t = node
        .field(FieldName::Operator)
        .map(|o| o.text(source).to_string())
        .unwrap_or_default();
    match t.to_ascii_lowercase().as_str() {
        "+" => BinaryOp::Add,
        "-" => BinaryOp::Sub,
        "*" => BinaryOp::Mul,
        "/" => BinaryOp::Div,
        "div" => BinaryOp::IntDiv,
        "mod" => BinaryOp::Mod,
        "=" => BinaryOp::Eq,
        "<>" => BinaryOp::Ne,
        "<" => BinaryOp::Lt,
        "<=" => BinaryOp::Le,
        ">" => BinaryOp::Gt,
        ">=" => BinaryOp::Ge,
        "and" => BinaryOp::And,
        "or" => BinaryOp::Or,
        "xor" => BinaryOp::Xor,
        "in" => BinaryOp::In,
        _ => BinaryOp::Other,
    }
}

fn unary_op(node: RawNode, source: &str) -> UnaryOp {
    let t = node
        .field(FieldName::Operator)
        .map(|o| o.text(source).to_string())
        .unwrap_or_default();
    match t.to_ascii_lowercase().as_str() {
        "not" => UnaryOp::Not,
        "-" => UnaryOp::Neg,
        _ => UnaryOp::Plus,
    }
}

/// True if `node`'s subtree contains a node of kind `k` (used for `temporary`
/// detection: `temporary_keyword` nested in the variable's `record_type`).
fn contains_kind(node: RawNode, k: RawKind) -> bool {
    node.kind() == k || node.named_children().iter().any(|c| contains_kind(*c, k))
}

/// Strip one layer of surrounding double quotes from a (quoted) identifier.
fn ident_text(n: RawNode, source: &str) -> String {
    let t = n.text(source);
    let bytes = t.as_bytes();
    if bytes.len() >= 2 && bytes[0] == b'"' && bytes[bytes.len() - 1] == b'"' {
        t[1..t.len() - 1].to_string()
    } else {
        t.to_string()
    }
}

/// A routine name node — either a plain `(quoted_)identifier` or a scoped
/// `member_trigger_name` (`Object::Member`). For the scoped form, join the two
/// (each unescaped) as `Object::Member` so the full qualified trigger name is kept
/// (`field("name")` on the old `multiple` shape returned only the object half).
fn routine_name_text(n: RawNode, source: &str) -> String {
    if n.kind() == RawKind::MemberTriggerName {
        let object = n
            .field(FieldName::Object)
            .map(|o| ident_text(o, source))
            .unwrap_or_default();
        let member = n
            .field(FieldName::Member)
            .map(|m| ident_text(m, source))
            .unwrap_or_default();
        format!("{object}::{member}")
    } else {
        ident_text(n, source)
    }
}

/// Build an [`Origin`] from a raw node (used pervasively by lowering).
pub(crate) fn origin_of(n: RawNode) -> Origin {
    let s = n.start_position();
    let e = n.end_position();
    Origin {
        kind_text: n.kind_str(),
        byte: n.byte_range(),
        start: Point {
            row: s.row as u32,
            column: s.column as u32,
        },
        end: Point {
            row: e.row as u32,
            column: e.column as u32,
        },
    }
}

#[cfg(test)]
mod tests {
    use crate::ir::{ExprKind, Literal, StmtKind};
    use crate::parse;

    /// Find the first `Case` statement in a parsed file.
    fn first_case(
        af: &crate::ir::AlFile,
    ) -> (&[crate::ir::CaseBranch], Option<crate::ir::BlockId>) {
        for s in af.ir.iter_stmts() {
            if let StmtKind::Case {
                branches,
                else_block,
                ..
            } = &s.kind
            {
                return (branches, *else_block);
            }
        }
        panic!("no Case statement lowered");
    }

    /// Regression for the case-pattern field-pollution grammar fix: `case 1, 2:` must
    /// yield TWO value patterns (the `,` separator is NOT a pattern), and lowering must
    /// not panic / emit Unknown on the comma. Pre-fix, `field('pattern', …)` spread over
    /// the inlined `,` tokens and `children_by_field("pattern")` returned anonymous commas.
    #[test]
    fn comma_case_pattern_yields_two_named_patterns_no_comma() {
        let src = r#"
codeunit 50000 T
{
    procedure P(I: Integer)
    begin
        case I of
            1, 2:
                Foo();
            3:
                Bar();
            else
                Baz();
        end;
    end;
}
"#;
        let af = parse(src);
        let (branches, else_block) = first_case(&af);
        assert_eq!(branches.len(), 2, "two value branches (1,2 and 3)");
        assert_eq!(branches[0].patterns.len(), 2, "`1, 2:` → two patterns");
        assert_eq!(branches[1].patterns.len(), 1, "`3:` → one pattern");
        assert!(else_block.is_some(), "else branch present");
        // Both patterns of branch 0 are integer literals — never a comma token.
        for &p in &branches[0].patterns {
            assert!(
                matches!(af.ir.expr(p).kind, ExprKind::Literal(Literal::Int(_))),
                "expected Int literal pattern"
            );
        }
        // No Unknown exprs anywhere in this clean snippet.
        assert!(
            af.ir
                .iter_exprs()
                .all(|e| !matches!(e.kind, ExprKind::Unknown)),
            "no Unknown exprs"
        );
    }

    /// `X in [..]` as a case pattern now binds the `pattern` field to a SINGLE named
    /// `in_expression` node, instead of the old inline `seq` that spread `left` /
    /// `operator` / `right` fields onto the branch. So the branch has exactly ONE
    /// pattern (not three), and lowering does not panic. (Call-result/boolean arg
    /// typing plan, Task 3: `in_expression` is now MODELED as `ExprKind::Binary{op:
    /// BinaryOp::In, ..}` — same as the other four comparison/logical RawKinds — so
    /// this pattern lowers to a real `Binary` node, not `Unknown`; the previously-
    /// documented "separate, pre-existing modeling gap" is closed.)
    #[test]
    fn in_expression_case_pattern_is_a_single_pattern() {
        let src = r#"
codeunit 50001 T
{
    procedure P(I: Integer): Boolean
    begin
        case true of
            I in [1, 2, 3]:
                exit(true);
            else
                exit(false);
        end;
    end;
}
"#;
        let af = parse(src);
        let (branches, _else) = first_case(&af);
        assert_eq!(branches.len(), 1, "one value branch (the `in` pattern)");
        assert_eq!(
            branches[0].patterns.len(),
            1,
            "`I in [..]:` is a single pattern, not left/op/right"
        );
    }

    /// A scoped member-trigger name (`Object::Member`) lowers to the FULL qualified
    /// name. Pre-fix, `_trigger_name` was an inlined `seq(id, '::', id)` so the `name`
    /// field was `multiple:true` over the `::` token, and `field("name")` returned only
    /// the object half (`UserTours`), silently dropping the member.
    #[test]
    fn member_trigger_name_lowers_to_qualified_name() {
        let src = r#"
codeunit 50000 T
{
    trigger UserTours::ShowTourWizard()
    begin
    end;
}
"#;
        let af = parse(src);
        let routine = af
            .objects
            .iter()
            .flat_map(|o| &o.routines)
            .find(|r| matches!(r.kind, crate::ir::RoutineKind::Trigger))
            .expect("a trigger routine");
        assert_eq!(routine.name, "UserTours::ShowTourWizard");
    }

    /// Dataitem-receivers plan, Task 1: `modify_modification` carries the
    /// modified member's name in its `target` field, not `name`
    /// (`RawModifyModification` has no `name()` accessor). Pre-fix, a
    /// trigger nested in `dataset { modify(X) { trigger .. } }` lost BOTH
    /// `enclosing_member` and any path to `dataitem_source_table` — the
    /// lowerer's generic Name-based member-wrapper gate never recognized the
    /// node at all. A report `dataset` `modify()` must ALSO set
    /// `in_dataset_modify_context = true` (the confirmed-dataset-context
    /// signal the resolver's fallback requires).
    #[test]
    fn modify_modification_target_becomes_enclosing_member_and_sets_dataset_context() {
        let src = r#"
report 50100 T
{
    dataset
    {
        modify(BaseDataitem)
        {
            trigger OnAfterGetRecord()
            begin
            end;
        }
    }
}
"#;
        let af = parse(src);
        let routine = af
            .objects
            .iter()
            .flat_map(|o| &o.routines)
            .find(|r| r.name.eq_ignore_ascii_case("OnAfterGetRecord"))
            .expect("the trigger nested in modify() must still be lowered");
        let (member_name, _origin) = routine
            .enclosing_member
            .as_ref()
            .expect("modify()'s Target must populate enclosing_member");
        assert_eq!(member_name, "BaseDataitem");
        assert!(
            routine.in_dataset_modify_context,
            "a dataset modify() must set the confirmed dataset-context flag"
        );
    }

    /// REQUESTPAGE ISOLATION (binding, dataitem-receivers plan round-1
    /// addendum): a `modify()` nested inside `requestpage { layout { .. } }`
    /// still gets its `enclosing_member` populated (the Target-field fix is
    /// general, not dataset-specific), but `in_dataset_modify_context` MUST
    /// stay `false` — this is NOT a report dataset `modify()`, and the
    /// resolver's dataitem-map fallback must never fire for it.
    #[test]
    fn modify_modification_inside_requestpage_never_sets_dataset_context() {
        let src = r#"
report 50101 T
{
    requestpage
    {
        layout
        {
            modify(SomeField)
            {
                trigger OnValidate()
                begin
                end;
            }
        }
    }
}
"#;
        let af = parse(src);
        let routine = af
            .objects
            .iter()
            .flat_map(|o| &o.routines)
            .find(|r| r.name.eq_ignore_ascii_case("OnValidate"))
            .expect("the trigger nested in the requestpage modify() must still be lowered");
        let (member_name, _origin) = routine
            .enclosing_member
            .as_ref()
            .expect("modify()'s Target must populate enclosing_member even outside dataset");
        assert_eq!(member_name, "SomeField");
        assert!(
            !routine.in_dataset_modify_context,
            "a requestpage/layout modify() must NEVER set dataset context"
        );
    }

    /// Task 1 review-fix pin (undescribed/untested finding): the
    /// `RawKind::ModifyModification` arm added to `collect_routines` is
    /// GLOBAL — it fires for ANY `modify()` block regardless of enclosing
    /// object kind, not only a report `dataset`/`requestpage`. A
    /// TableExtension's `fields { modify(Field) { trigger .. } }` is the
    /// most common real-world shape this touches. `enclosing_member` must
    /// populate (the Target-field fix is general), but
    /// `in_dataset_modify_context` must stay `false` — `dataset_ctx` is only
    /// ever forced `true` descending into a report `DatasetSection`/
    /// `ReportDataitem`, neither of which a TableExtension's `fields`
    /// section is — so the resolver's dataitem-map fallback correctly never
    /// fires here (inert on CDO, verified by the Task 1 CDO re-measure; this
    /// test pins the lowering behavior the resolver's inertness depends on).
    #[test]
    fn modify_modification_in_tableextension_fields_populates_member_not_dataset_context() {
        let src = r#"
tableextension 50100 "T Ext" extends Customer
{
    fields
    {
        modify(Name)
        {
            trigger OnBeforeValidate()
            begin
            end;
        }
    }
}
"#;
        let af = parse(src);
        let routine = af
            .objects
            .iter()
            .flat_map(|o| &o.routines)
            .find(|r| r.name.eq_ignore_ascii_case("OnBeforeValidate"))
            .expect(
                "the trigger nested in the TableExtension field modify() must still be lowered",
            );
        let (member_name, _origin) = routine
            .enclosing_member
            .as_ref()
            .expect("modify()'s Target must populate enclosing_member outside Report objects too");
        assert_eq!(member_name, "Name");
        assert!(
            !routine.in_dataset_modify_context,
            "a TableExtension field modify() must never set report dataset context"
        );
    }

    /// A dataitem trigger's `dataitem_source_table` (the direct, non-fallback
    /// path) is unaffected by the `modify()` fix — sanity guard against
    /// regressing the existing mechanism while adding the new one.
    #[test]
    fn report_dataitem_trigger_still_carries_dataitem_source_table_directly() {
        let src = r#"
report 50102 T
{
    dataset
    {
        dataitem(Cust; Customer)
        {
            trigger OnAfterGetRecord()
            begin
            end;
        }
    }
}
"#;
        let af = parse(src);
        let routine = af
            .objects
            .iter()
            .flat_map(|o| &o.routines)
            .find(|r| r.name.eq_ignore_ascii_case("OnAfterGetRecord"))
            .expect("the dataitem trigger must be lowered");
        assert_eq!(routine.dataitem_source_table.as_deref(), Some("Customer"));
        assert!(
            !routine.in_dataset_modify_context,
            "a real dataitem(...) trigger is not a modify() member — the flag stays false"
        );
    }

    /// Issue #41, T1: an ACTION modification (`actions { modify(X) { trigger
    /// … } }`) is the second member wrapper whose name lives in `target`
    /// rather than `name`. Pre-fix the trigger had no `enclosing_member` at
    /// all, so two sibling action `modify()` blocks each declaring
    /// `OnAfterAction()` minted the SAME stable routine id.
    ///
    /// The `origin.kind_text` assert is the ONE executable join between this
    /// lowerer test and the hand-stated `is_page_action_wrapper` test in
    /// `src/engine/root_classification.rs`: the classifier reads exactly this
    /// string (via `enclosing_member_range.syntax_kind`) to derive
    /// `page-action`. Without it, pointing `origin_of` at the trigger's own
    /// node instead of the wrapper would silently revert the classify half
    /// while every other assert here still passed.
    #[test]
    fn modify_action_modification_target_becomes_enclosing_member() {
        let src = r#"
pageextension 50110 "P Ext" extends "Customer Card"
{
    actions
    {
        modify(MyAction)
        {
            trigger OnAfterAction()
            begin
            end;
        }
    }
}
"#;
        let af = parse(src);
        let routine = af
            .objects
            .iter()
            .flat_map(|o| &o.routines)
            .find(|r| r.name.eq_ignore_ascii_case("OnAfterAction"))
            .expect("the trigger nested in the action modify() must still be lowered");
        let (member_name, origin) = routine
            .enclosing_member
            .as_ref()
            .expect("an action modify()'s Target must populate enclosing_member");
        assert_eq!(member_name, "MyAction");
        assert_eq!(
            origin.kind_text, "modify_action_modification",
            "the origin must be the WRAPPER's node — this string is what \
             root_classification::is_page_action_wrapper matches on"
        );
        // Cheap regression catch, NOT a pin of the ModifyModification-only kind
        // gate: `dataset_ctx` is false throughout a pageextension by
        // construction, so this would hold whatever that gate said.
        assert!(
            !routine.in_dataset_modify_context,
            "an actions-section modify() is never report-dataset context"
        );
    }

    /// Issue #41, T2: the IR layer strips only the OUTER quote pair
    /// (`ident_text`). Doubled-quote unescaping (`""` → `"`) happens later, in
    /// `ir_walk::ir_enclosing_member` (that is the L3/analyzer path; the program
    /// engine's `source_routine_node_id` folds the RAW, still-escaped text into
    /// `RoutineNodeId` — pre-existing for every quoted wrapper, not this issue's). This test pins that boundary rather
    /// than blurring it — asserting the fully unescaped name here would move
    /// the contract one layer down and make the engine's own unescape a no-op
    /// nobody notices losing.
    #[test]
    fn modify_action_modification_quoted_target_keeps_inner_escapes() {
        let src = r#"
pageextension 50111 "P Ext" extends "Customer Card"
{
    actions
    {
        modify("My ""Q"" Action")
        {
            trigger OnAfterAction()
            begin
            end;
        }
    }
}
"#;
        let af = parse(src);
        let routine = af
            .objects
            .iter()
            .flat_map(|o| &o.routines)
            .find(|r| r.name.eq_ignore_ascii_case("OnAfterAction"))
            .expect("the trigger nested in the quoted action modify() must be lowered");
        let (member_name, _origin) = routine
            .enclosing_member
            .as_ref()
            .expect("a quoted Target must populate enclosing_member");
        assert_eq!(
            member_name, r#"My ""Q"" Action"#,
            "outer quotes stripped, inner doubled quotes still escaped at the IR layer"
        );
    }

    /// Issue #41, T3 — the CONTROL that keeps the `target` fallback scoped to
    /// the two `modify_*` kinds. An `add*`/`move*` wrapper's `target` is an
    /// anchor or a container, never the declaring member of a routine in its
    /// body, so widening BOTH the wrapper arm and this fallback to that family
    /// would invent a member here. Widening the FALLBACK alone invents nothing
    /// and leaves this test green: `collect_routines` never passes an `add*`
    /// node as `member` (the generic arm needs a `name` field, which that
    /// family has not), so the fallback is unreachable for them. What this
    /// control pins is the PAIR — which is why the proof for it had to patch
    /// both sites.
    ///
    /// PARSE-SHAPE CONTRACT, deliberately not compilable AL: a real
    /// `addlast(X)` body declares `action(Y)` blocks and the trigger lives in
    /// one of those. Do NOT "correct" this fixture into
    /// `addlast(X) { action(Y) { trigger … } }` — the inner
    /// `action_declaration` has a `name` field, so the ordinary name gate
    /// makes IT the member and the test passes under the very widening it is
    /// supposed to catch. The grammar admits a `trigger_declaration` as a
    /// DIRECT child of `addlast_action_modification`'s body (verified with a
    /// real `tree-sitter parse`, v4.4.0), and only that un-shadowed shape can
    /// fail.
    #[test]
    fn addlast_action_modification_target_is_not_an_enclosing_member() {
        let src = r#"
pageextension 50112 "P Ext" extends "Customer Card"
{
    actions
    {
        addlast(Processing)
        {
            trigger OnDirect()
            begin
            end;
        }
    }
}
"#;
        let af = parse(src);
        let routine = af
            .objects
            .iter()
            .flat_map(|o| &o.routines)
            .find(|r| r.name.eq_ignore_ascii_case("OnDirect"))
            .expect("the trigger directly in the addlast() body must be lowered");
        assert_eq!(
            routine.enclosing_member, None,
            "an add*/move* target is an anchor or a container — never the \
             declaring member of a routine in its body"
        );
    }

    /// Issue #41, T4: a malformed `modify()` still parses, with a PRESENT,
    /// zero-width `target` node. Taking its text verbatim would mint
    /// `Some("")`, which takes the discriminated arm of
    /// `to_stable_routine_id_from_parts` and gives the routine a different,
    /// meaningless stable id from the same routine with no member at all. The
    /// `routines.len()` assert keeps the test non-vacuous: a future parse
    /// change that dropped the routine entirely would otherwise satisfy the
    /// `None` on its own.
    #[test]
    fn empty_action_modify_target_degrades_to_no_member() {
        let src = r#"
pageextension 50113 "P Ext" extends "Customer Card"
{
    actions
    {
        modify()
        {
            trigger OnAfterAction()
            begin
            end;
        }
    }
}
"#;
        let af = parse(src);
        let routines: Vec<_> = af.objects.iter().flat_map(|o| &o.routines).collect();
        assert_eq!(routines.len(), 1, "the trigger must still be lowered");
        assert_eq!(
            routines[0].enclosing_member, None,
            "a zero-width target must degrade to None, never Some(\"\")"
        );
    }

    /// Issue #41, T5: the twin of T4 for the OTHER wrapper kind. One shared
    /// guard covers both, so both are pinned — a per-kind copy of the filter
    /// would pass T4 alone.
    #[test]
    fn empty_dataset_modify_target_degrades_to_no_member() {
        let src = r#"
report 50114 T
{
    dataset
    {
        modify()
        {
            trigger OnAfterGetRecord()
            begin
            end;
        }
    }
}
"#;
        let af = parse(src);
        let routines: Vec<_> = af.objects.iter().flat_map(|o| &o.routines).collect();
        assert_eq!(routines.len(), 1, "the trigger must still be lowered");
        assert_eq!(
            routines[0].enclosing_member, None,
            "a zero-width target must degrade to None, never Some(\"\")"
        );
    }

    /// Issue #41 final panel (I1): the `in_dataset_modify_context` gate at the
    /// top of this file says, in a COMMENT, that it is deliberately
    /// `ModifyModification`-only and must not be folded into a shared
    /// modify-wrapper predicate. This test is what makes that executable.
    ///
    /// The shape is grammar-reachable and parses CLEAN (verified with
    /// `tree-sitter parse`: no `ERROR`, no `MISSING`): a `modify_modification`
    /// body is a `declaration_body`, which admits an `actions_section`. So
    /// `dataset_ctx` is `true` all the way down to the inner action modify.
    /// Fold the two kinds into one predicate and the flag flips to `true`,
    /// and the resolver's dataitem-map fallback starts looking an ACTION name
    /// up among dataitems -- binding the trigger's implicit `Rec` to whatever
    /// dataitem happens to share that name. Silent when it fires, so a test
    /// rather than a comment.
    ///
    /// The enclosing-member assert also pins the (correct) behaviour change
    /// this shape saw: before the action wrapper was captured, the trigger
    /// inherited `modify(Cust)`; now it is the action it is really in.
    ///
    /// PARSE-SHAPE CONTRACT: grammar-admitted, not compilable AL (a plain
    /// `report` has no dataset `modify()`, and a dataset modify has no actions
    /// section). Keep the nesting — it IS the test. Changing `report` to
    /// `reportextension` would preserve the discrimination; removing the
    /// `actions` nesting would contradict the test's own name and quietly
    /// delete the guard.
    #[test]
    fn action_modify_nested_in_a_dataset_modify_is_not_dataset_context() {
        let src = r#"
report 50115 T
{
    dataset
    {
        modify(Cust)
        {
            trigger OnPreDataItem()
            begin
            end;

            actions
            {
                modify(SomeAction)
                {
                    trigger OnAfterGetRecord()
                    begin
                    end;
                }
            }
        }
    }
}
"#;
        let af = parse(src);
        let all: Vec<_> = af.objects.iter().flat_map(|o| &o.routines).collect();
        assert_eq!(all.len(), 2, "both triggers must be lowered");

        // PRECONDITION, asserted rather than assumed: the sibling trigger sits
        // directly in the DATASET modify, so dataset context is genuinely true
        // at this depth. Without it, a future change that forced the flag off
        // at `actions_section` would make the assertion below vacuous and this
        // guard would stop guarding while staying green.
        let sibling = all
            .iter()
            .find(|r| r.name.eq_ignore_ascii_case("OnPreDataItem"))
            .expect("the dataset modify()'s own trigger must be lowered");
        assert!(
            sibling.in_dataset_modify_context,
            "precondition: a trigger directly in a dataset modify() IS dataset context"
        );

        let nested = all
            .iter()
            .find(|r| r.name.eq_ignore_ascii_case("OnAfterGetRecord"))
            .expect("the nested action trigger must be lowered");
        let (member_name, _origin) = nested
            .enclosing_member
            .as_ref()
            .expect("the INNER action modify is the enclosing member");
        assert_eq!(
            member_name, "SomeAction",
            "the innermost member wrapper wins, not the outer dataset modify()"
        );
        assert!(
            !nested.in_dataset_modify_context,
            "an ACTIONS-section modify is never report-dataset context, however it is \
             nested -- if this fails, the `in_dataset_modify_context` gate has been \
             widened to both modify kinds"
        );
    }

    /// Issue #41 final panel (N1): the empty-name guard is a `.filter` on the
    /// WHOLE `enclosing_member`, so it covers the `name` path as well as the
    /// `target` path. T4/T5 only reach it through `target`; this reaches it
    /// through `name`, so moving the filter back inside the `target`
    /// `or_else` fails here and nowhere else.
    ///
    /// `action()` with no name parses CLEAN -- `name: (identifier [r,c]-[r,c])`,
    /// zero width, no `ERROR` and no `MISSING`, so `has_error()` is false and
    /// NOTHING downstream flags this file. That is why the degrade to `None`
    /// has to happen here: `Some("")` would take the discriminated arm of
    /// `to_stable_routine_id_from_parts` and mint a meaningless id for a
    /// routine the engine otherwise considers perfectly parsed.
    #[test]
    fn empty_action_declaration_name_degrades_to_no_member() {
        let src = r#"
page 50116 P
{
    actions
    {
        area(Processing)
        {
            action()
            {
                trigger OnAction()
                begin
                end;
            }
        }
    }
}
"#;
        let af = parse(src);
        let routines: Vec<_> = af.objects.iter().flat_map(|o| &o.routines).collect();
        assert_eq!(routines.len(), 1, "the trigger must still be lowered");
        assert_eq!(
            routines[0].enclosing_member, None,
            "a zero-width `name` must degrade to None, never Some(\"\") -- the filter must \
             stay on the whole enclosing_member, not inside the target fallback"
        );
    }

    // -----------------------------------------------------------------------
    // Task 3 (preprocessor foundations plan): union-read pins + the two
    // flat-loop fixes (properties, implements) + the ParseStatus::Recovered
    // diagnostic fixture.
    // -----------------------------------------------------------------------

    /// The base union-read pin: a procedure declared inside a `#if
    /// UNDEFINED_SYMBOL .. #endif` branch (never true for any real build) is
    /// STILL lowered into `ObjectDecl.routines` — `is_preproc_wrapper`'s
    /// descent is unconditional, it never evaluates the condition. Documents
    /// the honest superset semantics: sound for an absence proof ("not found
    /// anywhere in the union" implies "absent from every build"), but this
    /// specific routine is NOT proof the call is reachable in any actual
    /// compiled build (see `is_preproc_wrapper`'s doc).
    #[test]
    fn preproc_undefined_branch_procedure_still_lowered_union_read() {
        let src = r#"
codeunit 50200 "Union Read"
{
    procedure Caller()
    begin
        Foo();
    end;

#if NEVER_DEFINED_SYMBOL
    procedure Foo()
    begin
    end;
#endif
}
"#;
        let af = parse(src);
        let names: Vec<&str> = af
            .objects
            .iter()
            .flat_map(|o| &o.routines)
            .map(|r| r.name.as_str())
            .collect();
        assert!(
            names.iter().any(|n| n.eq_ignore_ascii_case("Foo")),
            "a #if-UNDEFINED-branch procedure must still surface in the union-read \
             (sound-for-absence, not-sound-for-reachability); got routines: {names:?}"
        );
        assert!(names.iter().any(|n| n.eq_ignore_ascii_case("Caller")));
    }

    /// Both-arms `#if`/`#else` with DIFFERING signatures yields TWO distinct
    /// `RoutineDecl`s at the al-syntax layer (no dedup happens here at all —
    /// that is a `crate::program` (build.rs) concern, driven by `sig_fp`; see
    /// `preproc_both_arms_distinct_signature_yield_two_unmarked_source_
    /// overloads` / `preproc_same_signature_arms_collapse_to_one_unmarked_
    /// survivor` in `src/program/build.rs`'s test module for that half of the
    /// dedup interplay).
    #[test]
    fn preproc_both_arms_distinct_signature_yield_two_routine_decls() {
        let src = r#"
codeunit 50201 "Both Arms"
{
#if SOME_SYMBOL
    procedure Foo(X: Integer)
    begin
    end;
#else
    procedure Foo(Y: Text)
    begin
    end;
#endif
}
"#;
        let af = parse(src);
        let foos: Vec<_> = af
            .objects
            .iter()
            .flat_map(|o| &o.routines)
            .filter(|r| r.name.eq_ignore_ascii_case("Foo"))
            .collect();
        assert_eq!(
            foos.len(),
            2,
            "both #if/#else arms must lower to distinct RoutineDecls, not one \
             overwriting the other"
        );
        let param_types: Vec<Option<&str>> = foos
            .iter()
            .map(|r| r.params.first().and_then(|p| p.ty.as_deref()))
            .collect();
        assert!(
            param_types.contains(&Some("Integer")),
            "got param types: {param_types:?}"
        );
        assert!(
            param_types.contains(&Some("Text")),
            "got param types: {param_types:?}"
        );
    }

    /// The properties flat-loop fix: a `#if`-wrapped `SourceTable` property
    /// (previously silently dropped by `lower_object`'s flat
    /// `body.named_children()` scan — the ORIGINAL, now-fixed gap) is
    /// captured from BOTH branches. The program layer
    /// (`node_extract::singular_property_value`) is responsible for the
    /// fail-closed conflict degrade this union-read now enables; this test
    /// only pins the lowering-layer union-read itself.
    #[test]
    fn preproc_wrapped_source_table_property_captured_both_branches() {
        let src = r#"
page 50202 "Preproc Prop"
{
#if FOO
    SourceTable = Customer;
#else
    SourceTable = Vendor;
#endif

    layout
    {
    }
}
"#;
        let af = parse(src);
        let props: Vec<&crate::ir::ObjectProperty> = af.objects[0]
            .properties
            .iter()
            .filter(|p| p.name == "sourcetable")
            .collect();
        assert_eq!(
            props.len(),
            2,
            "both #if/#else SourceTable branches must be captured (union-read), \
             not just the first — got: {:?}",
            props.iter().map(|p| &p.value).collect::<Vec<_>>()
        );
        let values: Vec<&str> = props.iter().map(|p| p.value.as_str()).collect();
        assert!(values.contains(&"Customer"));
        assert!(values.contains(&"Vendor"));
    }

    /// Control: a NON-conditional property is unaffected by the fix (sanity
    /// guard against a regression in the common, unconditional case).
    #[test]
    fn plain_source_table_property_still_captured_control() {
        let src = r#"
page 50203 "Plain Prop"
{
    SourceTable = Customer;
    layout
    {
    }
}
"#;
        let af = parse(src);
        let props: Vec<&crate::ir::ObjectProperty> = af.objects[0]
            .properties
            .iter()
            .filter(|p| p.name == "sourcetable")
            .collect();
        assert_eq!(props.len(), 1);
        assert_eq!(props[0].value, "Customer");
    }

    /// The implements flat-loop's defensive descend fix, pinned against the
    /// only grammar-reachable `#if`-conditional `implements` shape
    /// (`preproc_split_declaration` — a whole-object header split): both
    /// branches' interface names are captured (a UNION, never degraded — see
    /// `extract_implements`'s doc for why this is sound without a
    /// program-layer conflict-degrade, unlike `SourceTable`/`TableNo`).
    #[test]
    fn preproc_split_declaration_implements_captured_both_branches() {
        let src = r#"
#if FOO
codeunit 50204 "Preproc Impl" implements IThing
#else
codeunit 50204 "Preproc Impl" implements IOther
#endif
{
}
"#;
        let af = parse(src);
        assert_eq!(af.objects.len(), 1, "one (preproc-split) object");
        let implements = &af.objects[0].implements;
        assert!(
            implements.iter().any(|i| i.eq_ignore_ascii_case("IThing")),
            "got implements: {implements:?}"
        );
        assert!(
            implements.iter().any(|i| i.eq_ignore_ascii_case("IOther")),
            "got implements: {implements:?}"
        );
    }

    /// The `ParseStatus::Recovered` diagnostic fixture: an unbalanced `#if`
    /// (no matching `#endif`) forces tree-sitter error recovery — the whole
    /// file's `parse_status` must report `Recovered`, the signal
    /// `crate::program`'s Recovered-file diagnostic (count + paths) consumes.
    #[test]
    fn unbalanced_if_directive_yields_recovered_parse_status() {
        let src = r#"
codeunit 50205 "Unbalanced"
{
    procedure Foo()
    begin
#if NEVER_CLOSED
        Bar();
    end;
}
"#;
        let af = parse(src);
        assert_eq!(
            af.parse_status,
            crate::ir::ParseStatus::Recovered,
            "an unbalanced #if must force error recovery, never report Clean"
        );
    }

    // -------------------------------------------------------------------
    // `interface_procedure` lowering (receiver-closure plan, Task 1) —
    // controladdin/interface signature-only procedures were previously
    // INVISIBLE to `RoutineDecl` extraction (a distinct grammar node,
    // `interface_procedure`, that `collect_routines` never matched).
    // -------------------------------------------------------------------

    /// A `controladdin` object's signature-only `procedure` declarations
    /// (no body, no trailing `;`) must be captured as `RoutineDecl`s with
    /// the right name + arity — the "declared procedures" surface Task 1's
    /// closed-if-known `CurrPage.<usercontrol>` gate depends on.
    #[test]
    fn controladdin_procedures_are_lowered_as_routines() {
        let src = r#"
controladdin "CDO.Editor"
{
    RequestedHeight = 500;

    event StartupCompleted();

    procedure InitEditor(mergeItemsAsJson: Text; localizationAsJson: Text)
    procedure GetHTML()
}
"#;
        let af = parse(src);
        assert_eq!(af.objects.len(), 1);
        let obj = &af.objects[0];
        assert_eq!(obj.routines.len(), 2, "two procedures, zero events");
        let init = obj
            .routines
            .iter()
            .find(|r| r.name.eq_ignore_ascii_case("InitEditor"))
            .expect("InitEditor must be lowered");
        assert_eq!(init.params.len(), 2, "arity must be captured");
        assert!(init.body.is_none(), "a signature has no body");
        assert!(!init.parse_incomplete);
        let get_html = obj
            .routines
            .iter()
            .find(|r| r.name.eq_ignore_ascii_case("GetHTML"))
            .expect("GetHTML must be lowered");
        assert_eq!(get_html.params.len(), 0);
    }

    /// An `event` declaration inside a controladdin is NEVER lowered as a
    /// `RoutineDecl` — events are not AL-callable, and the gate's declared
    /// surface must never accidentally admit one.
    #[test]
    fn controladdin_events_are_never_lowered_as_routines() {
        let src = r#"
controladdin "CDO.Editor"
{
    event OnSaveHTML(htmlString: Text);
    procedure SetHTML(html: Text)
}
"#;
        let af = parse(src);
        let obj = &af.objects[0];
        assert!(
            obj.routines
                .iter()
                .all(|r| !r.name.eq_ignore_ascii_case("OnSaveHTML")),
            "an event must never surface as a RoutineDecl: {:?}",
            obj.routines.iter().map(|r| &r.name).collect::<Vec<_>>()
        );
        assert_eq!(obj.routines.len(), 1, "only the procedure lowers");
    }

    /// An `interface` object's signature-only procedures lower identically
    /// (same grammar node, `interface_procedure`) — a bonus fidelity fix,
    /// not itself consumed by any resolver yet (interface dispatch routes
    /// through the implementer's own routine, not the interface's).
    #[test]
    fn interface_procedures_are_lowered_as_routines() {
        let src = r#"
interface "IFoo"
{
    procedure DoThing(x: Integer): Boolean;
}
"#;
        let af = parse(src);
        let obj = &af.objects[0];
        assert_eq!(obj.routines.len(), 1);
        let do_thing = &obj.routines[0];
        assert_eq!(do_thing.name, "DoThing");
        assert_eq!(do_thing.params.len(), 1);
        assert_eq!(
            do_thing.return_type.as_deref(),
            Some("Boolean"),
            "return type must be recovered from the nested interface_procedure_suffix child"
        );
    }

    /// T3 (receiver-closure-and-arg-increments plan): a NAMED return value
    /// (`procedure X() Ret: Record Y`) must capture the binding NAME on
    /// `RoutineDecl.return_name` — previously silently discarded entirely.
    #[test]
    fn named_return_value_binding_is_captured() {
        let src = r#"
codeunit 50100 "My Codeunit"
{
    procedure GetItem() Ret: Record Item
    begin
    end;
}
"#;
        let af = parse(src);
        let obj = &af.objects[0];
        let r = &obj.routines[0];
        assert_eq!(r.return_name.as_deref(), Some("Ret"));
        assert_eq!(r.return_type.as_deref(), Some("Record Item"));
    }

    /// An anonymous `: Type` return (no binding name) must capture `None` —
    /// never fabricate a name.
    #[test]
    fn anonymous_return_type_has_no_return_name() {
        let src = r#"
codeunit 50100 "My Codeunit"
{
    procedure GetCount(): Integer
    begin
    end;
}
"#;
        let af = parse(src);
        let obj = &af.objects[0];
        let r = &obj.routines[0];
        assert_eq!(r.return_name, None);
        assert_eq!(r.return_type.as_deref(), Some("Integer"));
    }

    /// A QUOTED binding name must be captured UNQUOTED (mirrors `Param`/
    /// `VarDecl` name storage convention — `ident_text` strips the outer
    /// quotes at lowering time).
    #[test]
    fn quoted_named_return_value_binding_is_unquoted() {
        let src = r#"
codeunit 50100 "My Codeunit"
{
    procedure GetItem() "My Result": Record Item
    begin
    end;
}
"#;
        let af = parse(src);
        let obj = &af.objects[0];
        let r = &obj.routines[0];
        assert_eq!(r.return_name.as_deref(), Some("My Result"));
        assert_eq!(r.return_type.as_deref(), Some("Record Item"));
    }

    /// A procedure with NO return spec at all (no `:`) must carry `None` for
    /// both — the common majority case, guards against a regression that
    /// fabricates a name/type out of thin air.
    #[test]
    fn no_return_spec_at_all_has_no_return_name_or_type() {
        let src = r#"
codeunit 50100 "My Codeunit"
{
    procedure DoSomething()
    begin
    end;
}
"#;
        let af = parse(src);
        let obj = &af.objects[0];
        let r = &obj.routines[0];
        assert_eq!(r.return_name, None);
        assert_eq!(r.return_type, None);
    }

    // ── Task T1.4 (deep-review-t1-4): preproc-split/guarded lowering + trivia ──

    /// Whether a `Call` targeting the bare identifier `name` is reachable by walking
    /// `Block`/`Stmt` LINKAGE from `start` — mirrors the engine's `walk_block_v2`/
    /// `walk_stmt_v2` (`src/program/resolve/extract.rs`) closely enough to prove a fix
    /// produces a DISCOVERABLE edge, not just an arena-orphaned node (the real
    /// call-graph walker only ever follows `Block.items`/`StmtKind` linkage, never a
    /// bare arena scan).
    fn call_reachable(af: &crate::ir::AlFile, start: crate::ir::BlockId, name: &str) -> bool {
        fn expr_is_call_to(af: &crate::ir::AlFile, id: crate::ir::ExprId, name: &str) -> bool {
            match &af.ir.expr(id).kind {
                ExprKind::Call { function, .. } => matches!(
                    &af.ir.expr(*function).kind,
                    ExprKind::Identifier(n) if n.eq_ignore_ascii_case(name)
                ),
                _ => false,
            }
        }
        fn walk_block(af: &crate::ir::AlFile, b: crate::ir::BlockId, name: &str) -> bool {
            af.ir.block(b).items.iter().any(|item| match item {
                crate::ir::BlockItem::Stmt(sid) => walk_stmt(af, &af.ir.stmt(*sid).kind, name),
                crate::ir::BlockItem::Preproc(g) => {
                    g.branches.iter().any(|bb| walk_block(af, *bb, name))
                }
            })
        }
        fn walk_stmt(af: &crate::ir::AlFile, kind: &StmtKind, name: &str) -> bool {
            match kind {
                StmtKind::Call(eid) => expr_is_call_to(af, *eid, name),
                StmtKind::If {
                    then_block,
                    else_block,
                    ..
                } => {
                    walk_block(af, *then_block, name)
                        || else_block.is_some_and(|b| walk_block(af, b, name))
                }
                StmtKind::Block(b) => walk_block(af, *b, name),
                StmtKind::Case {
                    branches,
                    else_block,
                    ..
                } => {
                    branches.iter().any(|br| walk_block(af, br.body, name))
                        || else_block.is_some_and(|b| walk_block(af, b, name))
                }
                StmtKind::While { body, .. }
                | StmtKind::Repeat { body, .. }
                | StmtKind::For { body, .. }
                | StmtKind::Foreach { body, .. }
                | StmtKind::With { body, .. }
                | StmtKind::AssertError(body) => walk_block(af, *body, name),
                StmtKind::Try { body, catch_block } => {
                    walk_block(af, *body, name)
                        || catch_block.is_some_and(|b| walk_block(af, b, name))
                }
                _ => false,
            }
        }
        walk_block(af, start, name)
    }

    /// H-6: `preproc_split_procedure` — the procedure header differs across `#if`/
    /// `#else` branches but the body (after `#endif`) is SHARED, compiled into every
    /// build. Pre-fix, this fell into `collect_routines`'s `_` catch-all (recurses
    /// without ever creating a `RoutineDecl`) — the routine, and the call in its
    /// shared body, silently ceased to exist: no routine, no edge, no diagnostic.
    #[test]
    fn preproc_split_procedure_shared_body_call_is_reachable() {
        let src = r#"
codeunit 50100 T
{
#if DEBUGMODE
    procedure Foo(A: Integer)
#else
    procedure Foo(A: Integer; B: Integer)
#endif
    begin
        DoWork(A);
    end;
}
"#;
        let af = parse(src);
        let obj = &af.objects[0];
        let r = obj
            .routines
            .iter()
            .find(|r| r.name == "Foo")
            .expect("preproc_split_procedure must yield a RoutineDecl named Foo");
        let body = r.body.expect("shared body must be lowered");
        assert!(
            call_reachable(&af, body, "DoWork"),
            "the shared body's call must be a discoverable edge"
        );
    }

    /// H-6: `preproc_split_procedure_preamble` — header AND `var` section differ
    /// across branches; the shared `code_block` (after `#endif`) has NO `body` field
    /// at all (VERIFIED against the pinned grammar with `tree-sitter parse`) — a
    /// stricter variant of the same defect, needing `lower_routine`'s dedicated
    /// CodeBlock fallback.
    #[test]
    fn preproc_split_procedure_preamble_shared_body_call_is_reachable() {
        let src = r#"
codeunit 50101 T
{
#if DEBUGMODE
    procedure Foo(A: Integer)
    var
        X: Integer;
#else
    procedure Foo(A: Integer; B: Integer)
#endif
    begin
        DoWork(A);
    end;
}
"#;
        let af = parse(src);
        let obj = &af.objects[0];
        let r = obj
            .routines
            .iter()
            .find(|r| r.name == "Foo")
            .expect("preproc_split_procedure_preamble must yield a RoutineDecl named Foo");
        let body = r
            .body
            .expect("shared body must be lowered via the CodeBlock fallback");
        assert!(call_reachable(&af, body, "DoWork"));
    }

    /// H-7: `preproc_conditional_case` — a whole extra case branch gated behind
    /// `#if`, nested INSIDE `case_body` (not a direct child of `case_statement`).
    /// Pre-fix, `lower_case_body` matched only plain `CaseBranch` under `case_body`,
    /// silently skipping this wrapper (and every branch inside it) — worse than an
    /// empty branch, no trace at all.
    #[test]
    fn preproc_conditional_case_branches_are_unioned() {
        let src = r#"
codeunit 50102 T
{
    procedure P(I: Integer)
    begin
        case I of
#if EXTRA
            1:
                DoOne();
#endif
            2:
                DoTwo();
        end;
    end;
}
"#;
        let af = parse(src);
        let (branches, _else) = first_case(&af);
        assert_eq!(
            branches.len(),
            2,
            "both the conditional branch and the plain branch must be present"
        );
    }

    /// H-7: `preproc_split_case_branch` — the grammar's `case_branch` alternative
    /// still wraps this in an OUTER `case_branch` node whose OWN `pattern`/`body`
    /// fields are unset (VERIFIED against the pinned grammar); reading them directly
    /// fabricates an EMPTY branch (the fleet-confirmed defect) instead of descending
    /// to the nested `preproc_split_case_branch`.
    #[test]
    fn preproc_split_case_branch_is_not_an_empty_block() {
        let src = r#"
codeunit 50103 T
{
    procedure P(I: Integer)
    begin
        case I of
#if EXTRA
            1,
#endif
            2:
                DoWork();
            3:
                DoThree();
        end;
    end;
}
"#;
        let af = parse(src);
        let (branches, _else) = first_case(&af);
        assert_eq!(branches.len(), 2);
        assert!(
            !branches[0].patterns.is_empty(),
            "the split branch's patterns must not be empty"
        );
        assert!(
            call_reachable(&af, branches[0].body, "DoWork"),
            "the split branch's shared body call must not be fabricated as an empty block"
        );
    }

    /// H-7: statement-position `#if` (`preproc_split_if_statement`) — the `if`'s
    /// CONDITION differs across `#if`/`#else` branches but the `then`-branch is
    /// SHARED. Pre-fix, `lower_stmt`'s `_` catch-all recorded an issue and returned
    /// bare `Unknown` without descending — the call in the shared then-branch had no
    /// edge. Post-fix this reconstructs a real `StmtKind::If`.
    #[test]
    fn preproc_split_if_statement_shared_then_branch_call_is_reachable() {
        let src = r#"
codeunit 50104 T
{
    procedure P(A: Boolean; B: Boolean)
    begin
#if DEBUGMODE
        if A then
#else
        if B then
#endif
            DoWork();
    end;
}
"#;
        let af = parse(src);
        let r = &af.objects[0].routines[0];
        let body = r.body.expect("body must be lowered");
        assert!(call_reachable(&af, body, "DoWork"));
    }

    /// H-7: `#region`/`#endregion` markers inside a body must never become a phantom
    /// `Unknown` statement — they are pure editor-fold markers with zero content.
    #[test]
    fn region_markers_produce_no_phantom_statement() {
        let src = r#"
codeunit 50105 T
{
    procedure P()
    begin
#region My Region
        DoWork();
#endregion
    end;
}
"#;
        let af = parse(src);
        let r = &af.objects[0].routines[0];
        let body = r.body.expect("body must be lowered");
        assert!(call_reachable(&af, body, "DoWork"));
        assert!(
            af.ir
                .iter_stmts()
                .all(|s| !matches!(s.kind, StmtKind::Unknown)),
            "#region/#endregion must never fabricate an Unknown statement"
        );
    }

    /// T1.4 review finding 1a: `preproc_split_procedure_body` — a plain (non-preamble,
    /// non-header-split) `procedure` whose VAR SECTION and `begin` differ across
    /// `#if`/`#else` but whose TAIL is shared, compiled into every build. Grammar-
    /// VERIFIED (`tree-sitter parse`): `preproc_split_procedure_body` is a NAMED
    /// (non-inlined) choice arm of `procedure`'s body position, so — unlike
    /// `_routine_regular_body`'s inlined `field('body', code_block)` — the routine
    /// node's OWN `field(Body)` is `None`; the pre-fix code's `PreprocSplitProcedurePreamble`-
    /// only fallback never caught this sibling shape at all, so the whole shared tail
    /// (and its call) was silently dropped: `RoutineDecl.body == None`, zero issues.
    #[test]
    fn preproc_split_procedure_body_shared_tail_call_is_reachable() {
        let src = r#"
codeunit 50200 T
{
    procedure P()
#if CLEAN24
    var
        X: Integer;
    begin
#else
    begin
#endif
        DoWork();
    end;
}
"#;
        let af = parse(src);
        let r = &af.objects[0].routines[0];
        let body = r
            .body
            .expect("preproc_split_procedure_body's shared tail must be lowered");
        assert!(
            call_reachable(&af, body, "DoWork"),
            "the shared tail's call must be a discoverable edge"
        );
    }

    /// T1.4 review finding 1a (union-read half): when the `#if`-branch of a
    /// `preproc_split_procedure_body` ALSO has its own content (not just the shared
    /// tail), that content and the shared tail are TWO SEPARATE `field('body', ..)`-
    /// tagged `statement_block`s on the SAME node (grammar-verified: the inlined
    /// `_pspb_if_branch`'s own `body` field flattens up alongside the wrapper's
    /// trailing one) — a single `.field(Body)` read only ever sees the FIRST. Both
    /// calls must survive: the `#if`-branch's is conditionally real, the tail's is
    /// unconditionally real, and dropping either would lose a genuine call-graph edge.
    #[test]
    fn preproc_split_procedure_body_unions_if_branch_and_shared_tail() {
        let src = r#"
codeunit 50201 T
{
    procedure P()
#if CLEAN24
    var
        X: Integer;
    begin
        DoIfBranch();
#else
    begin
#endif
        DoWork();
    end;
}
"#;
        let af = parse(src);
        let r = &af.objects[0].routines[0];
        let body = r
            .body
            .expect("preproc_split_procedure_body's content must be lowered");
        assert!(
            call_reachable(&af, body, "DoIfBranch"),
            "the #if-branch's own call must not be dropped by a first-field-only read"
        );
        assert!(
            call_reachable(&af, body, "DoWork"),
            "the shared tail's call must still be reachable"
        );
    }

    /// T1.4 review finding 1b (re-review): `preproc_split_complete_body` — EVERY
    /// `#if`/`#else` arm is a COMPLETE, mutually-exclusive body (grammar comment: "the
    /// entire body differs across branches"), no shared tail at all. Same missing-
    /// fallback defect as finding 1a: the routine node's `field(Body)` is `None` since
    /// `preproc_split_complete_body` is a named, non-inlined wrapper. Reviewer's live
    /// probe (verbatim): a first-branch-wins fix recovers `DoNew` but silently drops
    /// `DoOld` — this codebase's dominant policy is UNION-read across `#if` branches
    /// (see `is_preproc_wrapper`'s doc: sound for absence proofs, a deliberate superset
    /// over-approximation), matched by the pre-existing two-`RoutineDecl` precedent
    /// (`preproc_both_arms_distinct_signature_yield_two_routine_decls`) and this
    /// function's own `preproc_split_procedure_body` sibling one function up — both
    /// arms' calls must be reachable, not just the first.
    #[test]
    fn preproc_split_complete_body_unions_both_arms() {
        let src = r#"
codeunit 50202 T
{
    procedure P()
#if CLEAN24
    begin
        DoNew();
    end;
#else
    begin
        DoOld();
    end;
#endif
}
"#;
        let af = parse(src);
        let r = &af.objects[0].routines[0];
        let body = r
            .body
            .expect("preproc_split_complete_body's arms must be lowered");
        assert!(
            call_reachable(&af, body, "DoNew"),
            "the #if branch's call must be a discoverable edge"
        );
        assert!(
            call_reachable(&af, body, "DoOld"),
            "the #else branch's call must not be dropped by a first-field-only read"
        );
    }

    /// T1.4 review finding 2: `preproc_split_code_block_end` — a `code_block` whose
    /// closing `end` itself differs across `#if`/`#else` is a SIBLING child of
    /// `code_block`, never nested inside its `body` field. `lower_code_block`'s old
    /// `cb.field(Body).unwrap_or(cb)` trick only ever reached this sibling when `body`
    /// was ABSENT (empty leading run) — a `code_block` with real LEADING content before
    /// the split (here, `then_branch`'s `DoFirst()`) took the `Some(body)` arm and never
    /// looked at the sibling again, silently dropping both `DoSecond()` (the split's
    /// `#else` content) and `DoThird()` (the outer `if`'s unconditional `else` clause,
    /// folded into the SAME `preproc_split_code_block_end` node by the grammar).
    #[test]
    fn preproc_split_code_block_end_sibling_recovered_with_leading_content() {
        let src = r#"
codeunit 50203 T
{
    procedure P(X: Boolean)
    begin
        if X then
        begin
            DoFirst();
#if COND
        end;
#else
            DoSecond();
        end
        else begin
            DoThird();
        end;
#endif
    end;
}
"#;
        let af = parse(src);
        let r = &af.objects[0].routines[0];
        let body = r.body.expect("body must be lowered");
        assert!(
            call_reachable(&af, body, "DoFirst"),
            "the leading content before the split must still be reachable"
        );
        assert!(
            call_reachable(&af, body, "DoSecond"),
            "the split-end sibling's #else content must not be dropped"
        );
        assert!(
            call_reachable(&af, body, "DoThird"),
            "the split-end sibling's unconditional else-clause must not be dropped"
        );
    }

    /// H-8: a leading comment inside parens must not be mistaken for the real inner
    /// expression (pre-fix: `ParenthesizedExpression`'s first NAMED child was the
    /// comment, since a comment is a legal named child almost anywhere).
    #[test]
    fn parenthesized_leading_comment_does_not_replace_the_expression() {
        let src = r#"
codeunit 50106 T
{
    procedure P()
    var
        X: Integer;
    begin
        X := (/* c */ Foo());
    end;
}
"#;
        let af = parse(src);
        let found = af.ir.iter_exprs().any(|e| match &e.kind {
            ExprKind::Parenthesized(inner) => matches!(
                &af.ir.expr(*inner).kind,
                ExprKind::Call { function, .. }
                    if matches!(&af.ir.expr(*function).kind, ExprKind::Identifier(n) if n == "Foo")
            ),
            _ => false,
        });
        assert!(
            found,
            "the parenthesized expression must wrap the real call, not the comment"
        );
    }

    /// H-8: a mid-argument comment must not become a phantom `Unknown` argument,
    /// breaking arity-exact dispatch.
    #[test]
    fn mid_argument_comment_does_not_shift_arity() {
        let src = r#"
codeunit 50107 T
{
    procedure P()
    begin
        DoWork(1, /* mid */ 2);
    end;
}
"#;
        let af = parse(src);
        let call = af
            .ir
            .iter_exprs()
            .find_map(|e| match &e.kind {
                ExprKind::Call { function, args }
                    if matches!(&af.ir.expr(*function).kind, ExprKind::Identifier(n) if n == "DoWork") =>
                {
                    Some(args.clone())
                }
                _ => None,
            })
            .expect("DoWork call must exist");
        assert_eq!(
            call.len(),
            2,
            "the comment must not count as a third argument"
        );
        for &a in &call {
            assert!(matches!(
                af.ir.expr(a).kind,
                ExprKind::Literal(Literal::Int(_))
            ));
        }
    }
}

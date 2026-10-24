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
    StmtId, StmtKind, SyntaxIssue, TypeOp, UnaryOp, VarDecl,
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
        let kind = if child.kind() == RawKind::PreprocSplitDeclaration {
            Some(split_declaration_kind(child))
        } else {
            object_kind_of(child.kind())
        };
        match kind {
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

/// The kind of a header split across `#if` arms (`preproc_split_declaration`): its
/// FIRST header's object keyword, the first-branch-wins policy its name and id
/// already follow. It can split any object kind; it used to be lowered as a
/// Codeunit whatever it was (S9.0e: an `enum` in System Application).
fn split_declaration_kind(node: RawNode) -> ObjectKind {
    use ObjectKind as O;
    node.named_children()
        .into_iter()
        .find_map(|c| {
            Some(match c.kind() {
                RawKind::CodeunitKeyword => O::Codeunit,
                RawKind::EnumKeyword => O::Enum,
                RawKind::TableKeyword => O::Table,
                RawKind::PageKeyword => O::Page,
                RawKind::ReportKeyword => O::Report,
                RawKind::QueryKeyword => O::Query,
                RawKind::XmlportKeyword => O::XmlPort,
                RawKind::PermissionsetKeyword => O::PermissionSet,
                RawKind::TableextensionKeyword => O::TableExtension,
                RawKind::PageextensionKeyword => O::PageExtension,
                RawKind::ReportextensionKeyword => O::ReportExtension,
                RawKind::EnumextensionKeyword => O::EnumExtension,
                RawKind::PermissionsetextensionKeyword => O::PermissionSetExtension,
                RawKind::InterfaceKeyword => O::Interface,
                _ => return None,
            })
        })
        .unwrap_or(O::Other)
}

fn object_kind_of(k: RawKind) -> Option<ObjectKind> {
    use ObjectKind as O;
    Some(match k {
        RawKind::CodeunitDeclaration => O::Codeunit,
        // Its kind is its header's (`split_declaration_kind`); `Other` here only
        // keeps it an object for `enclosing_member` detection.
        RawKind::PreprocSplitDeclaration => O::Other,
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
    let cond_syms = CondIndex::new(node, source);
    let mut routine_nodes = Vec::new();
    // Entries still pending at the object's end dangle and attach to nothing.
    collect_routines(
        node.named_children(),
        None,
        None,
        false,
        &[],
        Vec::new(),
        &cond_syms,
        source,
        issues,
        &mut routine_nodes,
    );
    // A `#if`-split header yields one routine per arm (see `header_arms`). Each arm
    // takes the pending entries whose guard a build of that arm can satisfy, read
    // under that joined guard (`lower_routine` deduplicates them).
    let mut routines = Vec::new();
    for (r, attr_items, di_table, member, in_dataset_modify_context, ctx) in routine_nodes {
        let (arms, shared_from) = header_arms(r);
        let conds = split_conditions(r);
        for arm in arms {
            let a_syms = arm_symbols(r, &arm, source);
            routines.push(lower_routine(
                r,
                attr_items
                    .iter()
                    .filter_map(|e| {
                        consistent(&e.guard, &a_syms)
                            .filter(|_| arm_live(&conds, arm.start, source, &e.guard))
                            .map(|guard| PendingAttr {
                                node: e.node,
                                guard,
                            })
                    })
                    .collect(),
                di_table.clone(),
                member,
                in_dataset_modify_context,
                (&arm, shared_from),
                &ctx,
                source,
                ir,
                issues,
            ));
        }
    }

    // Report dataitems and XmlPort table elements (name, source-table) — the name is
    // in scope as a record var across all the object's routines. Empty otherwise.
    let mut dataitems = Vec::new();
    if matches!(
        kind,
        ObjectKind::Report | ObjectKind::ReportExtension | ObjectKind::XmlPort
    ) {
        collect_dataitems(node, source, &mut dataitems);
    }
    let mut query_columns = Vec::new();
    if kind == ObjectKind::Query {
        collect_query_columns(node, "", source, &mut query_columns);
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
    let mut protected_globals = Vec::new();
    let mut enum_values = Vec::new();
    if let Some(body) = node.field(FieldName::Body) {
        for member in body.named_children() {
            collect_globals(member, source, &mut globals);
            collect_protected_names(member, source, &mut protected_globals);
            collect_properties(member, source, &mut properties);
            if kind == ObjectKind::Enum {
                collect_enum_values(member, source, &mut enum_values);
            }
        }
    }
    if kind == ObjectKind::XmlPort {
        collect_xmlport_text_nodes(node, source, &mut globals);
    }
    let mut run_objects = Vec::new();
    collect_run_objects(node, source, &mut run_objects);

    ObjectDecl {
        kind,
        id,
        name,
        routines,
        globals,
        protected_globals,
        properties,
        dataitems,
        query_columns,
        extends_target,
        implements,
        page_controls,
        fields,
        keys,
        run_objects,
        enum_values,
        origin: origin_of(node),
    }
}

/// Collect enum `value` declarations, descending preproc wrappers (every arm),
/// as [`collect_properties`] does. A value's `Implementation` properties go
/// through [`collect_properties`] too, so an `#if` in the value body or around
/// the property value gives one entry per arm.
fn collect_enum_values(node: RawNode, source: &str, out: &mut Vec<crate::ir::EnumValueDecl>) {
    match node.kind() {
        RawKind::EnumValueDeclaration => {
            let Some(name) = node.field(FieldName::ValueName) else {
                return;
            };
            let mut properties = Vec::new();
            if let Some(body) = node.field(FieldName::Body) {
                for member in body.named_children() {
                    collect_properties(member, source, &mut properties);
                }
            }
            out.push(crate::ir::EnumValueDecl {
                name: ident_text(name, source),
                implementations: properties
                    .into_iter()
                    .filter(|p| p.name == "implementation")
                    .map(|p| p.value)
                    .collect(),
                origin: origin_of(node),
            });
        }
        _ if is_preproc_wrapper(node) => {
            for c in node.named_children() {
                collect_enum_values(c, source, out);
            }
        }
        _ => {}
    }
}

/// Every `RunObject` property value anywhere under `node`, outside code blocks
/// (see [`crate::ir::ObjectDecl::run_objects`]). Properties go through
/// [`lower_property`], so a whole-value `#if` gives one value per arm.
fn collect_run_objects(node: RawNode, source: &str, out: &mut Vec<String>) {
    for child in node.named_children() {
        match child.kind() {
            RawKind::CodeBlock => {}
            RawKind::Property => {
                let mut props = Vec::new();
                lower_property(child, source, &mut props);
                out.extend(
                    props
                        .into_iter()
                        .filter(|p| p.name == "runobject" && !p.value.is_empty())
                        .map(|p| p.value),
                );
            }
            _ => collect_run_objects(child, source, out),
        }
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
            RawKind::ImplementsClause => implements_clause_names(child, source, out),
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

/// The interface names of an `implements_clause`. An `#if` group in the list
/// (`preproc_conditional_implements`, grammar 8682cb5f: `implements IA, #if X IB,
/// #endif IC`) gives the names of every arm, nested groups included: the same
/// union [`extract_implements`] already reads across a whole-header `#if`.
fn implements_clause_names(clause: RawNode, source: &str, out: &mut Vec<String>) {
    for sub in clause.named_children() {
        match sub.kind() {
            RawKind::Identifier | RawKind::QuotedIdentifier => out.push(ident_text(sub, source)),
            RawKind::PreprocConditionalImplements => implements_clause_names(sub, source, out),
            _ => {}
        }
    }
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
            RawKind::KeyDeclaration => keys.push(key_members(child, source)),
            // A field or key whose HEADER differs per `#if` arm, with one shared body.
            // The header fields (`id`/`name`/`type`, or `name`/`fields`) repeat once per
            // arm on the node itself; `.field()` takes the first arm's, the
            // first-branch-wins policy of every split header (`PreprocSplitDeclaration`).
            // An arm may lead with complete fields/keys, which are ordinary children, so
            // the node is also descended — first, to keep document order.
            // `preproc_split_table_field_open` (grammar 2026-10) is the field whose BODY
            // opens inside the `#if` and closes after `#endif`; `preproc_split_key`
            // (2026-10) is the key analogue of `preproc_split_table_field`, which predates
            // them and was not read here before either.
            RawKind::PreprocSplitTableField | RawKind::PreprocSplitTableFieldOpen => {
                collect_table_fields_keys(child, source, fields, keys);
                fields.push(lower_field(child, source));
            }
            RawKind::PreprocSplitKey => {
                collect_table_fields_keys(child, source, fields, keys);
                keys.push(key_members(child, source));
            }
            _ => collect_table_fields_keys(child, source, fields, keys),
        }
    }
}

/// A key's member field names (unquoted, lowercased), from its `fields` list.
///
/// An `#if` group in the list (`preproc_conditional_field_list_items`, grammar
/// 8682cb5f) gives its first arm's names ([`conditional_list_arm`]): a key's fields
/// are ordered, so a union of the arms is a key no build declares. That is the
/// first-branch-wins policy `preproc_split_key` already follows for a split header.
fn key_members(key: RawNode, source: &str) -> Vec<String> {
    let mut members = Vec::new();
    if let Some(list) = key.field(FieldName::Fields) {
        let mut items = Vec::new();
        let mut run = Run::default();
        for m in list.named_children() {
            if m.kind() == RawKind::PreprocConditionalFieldListItems {
                conditional_list_arm(m, source, &mut run, &mut items);
            } else {
                items.push(m);
            }
        }
        for m in items {
            if matches!(m.kind(), RawKind::Identifier | RawKind::QuotedIdentifier) {
                members.push(ident_text(m, source).fold_identifier());
            }
        }
    }
    members
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
    // Field properties, read through `collect_properties`, which descends preproc
    // wrappers and whole-value `#if` arms (one value per arm). Every `body` is
    // read: a `preproc_split_table_field_open` has two, before and after its
    // `#endif`. Reading direct children only missed any `#if`-wrapped property
    // (#26 for ObsoleteState, #62 for FieldClass).
    let mut props = Vec::new();
    for body in node.children_by_field(FieldName::Body) {
        for member in body.named_children() {
            collect_properties(member, source, &mut props);
        }
    }
    let values_of = |name: &str| -> Vec<String> {
        props
            .iter()
            .filter(|p| p.name == name && !p.value.is_empty())
            .map(|p| p.value.trim_matches('\'').to_string())
            .collect()
    };
    // FieldClass: FlowField / FlowFilter / Normal. Mirrors classify_field.
    // ponytail: arms that DISAGREE fall back to "Normal" (the old answer). Its
    // consumers pull opposite ways (d1 wants "may be FlowField", d3/d22/d42
    // "surely FlowField"), so the honest model carries every arm and each
    // consumer picks any/all -- add that when a real workspace has the shape
    // (0 in CDO and DO when #62 was fixed).
    let classes: Vec<&str> = values_of("fieldclass")
        .iter()
        .map(|v| {
            let v = v.fold_identifier();
            if v.contains("flowfield") {
                "FlowField"
            } else if v.contains("flowfilter") {
                "FlowFilter"
            } else {
                "Normal"
            }
        })
        .collect();
    let field_class = match classes.split_first() {
        Some((first, rest)) if rest.iter().all(|c| c == first) => first.to_string(),
        _ => "Normal".to_string(),
    };
    let obsolete_state = values_of("obsoletestate");
    let obsolete_reason = values_of("obsoletereason");
    let dt_lc = data_type.to_ascii_lowercase();
    let is_blob_like = dt_lc == "blob" || dt_lc == "media" || dt_lc == "mediaset";
    crate::ir::FieldDecl {
        number,
        name,
        data_type,
        field_class,
        is_blob_like,
        obsolete_state,
        obsolete_reason,
    }
}

/// Collect every report `dataitem(Name; "Source Table")` (incl. nested) as
/// `(name, source-table)`, both unquoted, document order. Mirrors the legacy
/// `report_dataitem_record_vars`.
/// An XmlPort's `textelement(Name)` / `textattribute(Name)` nodes, as object
/// globals: each is a variable in the whole xmlport's scope, `Text` by default and
/// `BigText` with `TextType = BigText` (alc 18.0.41.45789: assigning one to a
/// `Date` fails AL0122 naming `Text` / `BigText`; both are read from an xmlport
/// procedure as well as from the node's own triggers).
fn collect_xmlport_text_nodes(node: RawNode, source: &str, out: &mut Vec<VarDecl>) {
    for child in node.named_children() {
        let is_text_node = match child.kind() {
            RawKind::XmlportElement => child
                .field(FieldName::ElementType)
                .is_some_and(|t| t.kind() == RawKind::TextelementKeyword),
            RawKind::XmlportAttribute => child
                .field(FieldName::AttributeType)
                .is_some_and(|t| t.kind() == RawKind::TextattributeKeyword),
            _ => false,
        };
        if is_text_node && let Some(name) = child.field(FieldName::Name) {
            let mut props = Vec::new();
            if let Some(body) = child.field(FieldName::Body) {
                for p in body.named_children() {
                    collect_properties(p, source, &mut props);
                }
            }
            let big = props
                .iter()
                .any(|p| p.name == "texttype" && p.value.eq_ignore_ascii_case("bigtext"));
            out.push(VarDecl {
                name: ident_text(name, source),
                ty: Some(if big { "BigText" } else { "Text" }.to_string()),
                temporary: false,
                origin: origin_of(child),
            });
        }
        collect_xmlport_text_nodes(child, source, out);
    }
}

/// A query's plain columns (see [`crate::ir::ObjectDecl::query_columns`]): each
/// `query_column` with a `field_name` and no `Method` property, paired with the
/// source table of its innermost enclosing `query_dataitem` (`table`).
fn collect_query_columns(
    node: RawNode,
    table: &str,
    source: &str,
    out: &mut Vec<(String, String, String)>,
) {
    for child in node.named_children() {
        match child.kind() {
            RawKind::QueryDataitem => {
                let inner = child
                    .field(FieldName::TableName)
                    .map(|n| ident_text(n, source))
                    .unwrap_or_default();
                collect_query_columns(child, &inner, source, out);
            }
            RawKind::QueryColumn => {
                let name = child.field(FieldName::Name).map(|n| ident_text(n, source));
                let field = child
                    .field(FieldName::FieldName)
                    .map(|n| ident_text(n, source));
                let mut props = Vec::new();
                if let Some(body) = child.field(FieldName::Body) {
                    for p in body.named_children() {
                        collect_properties(p, source, &mut props);
                    }
                }
                if let (Some(name), Some(field)) = (name, field)
                    && !table.is_empty()
                    && !props.iter().any(|p| p.name == "method")
                {
                    out.push((name, table.to_string(), field));
                }
            }
            _ => collect_query_columns(child, table, source, out),
        }
    }
}

fn collect_dataitems(node: RawNode, source: &str, out: &mut Vec<(String, String)>) {
    for child in node.named_children() {
        // `tableelement(Name; Table)` (S9.0e): an XmlPort's record variable.
        if child.kind() == RawKind::XmlportElement
            && child
                .field(FieldName::ElementType)
                .is_some_and(|t| t.kind() == RawKind::TableelementKeyword)
        {
            let name = child
                .field(FieldName::Name)
                .map(|n| ident_text(n, source))
                .unwrap_or_default();
            let table = child
                .field(FieldName::Source)
                .map(|n| ident_text(n, source))
                .unwrap_or_default();
            if !name.is_empty() && !table.is_empty() {
                out.push((name, table));
            }
        }
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
        collect_dataitems(child, source, out);
    }
}

/// Lower a `property` node (`name = value`) into `out`. Name lowercased; value is the
/// raw text of the value field (trimmed). Nothing when the name is missing.
///
/// A whole-value `#if` (`TableNo = #if X Customer; #else Vendor; #endif`) is a
/// `preproc_conditional_property_value` whose arms are its `value` fields (grammar
/// G6). It gives ONE entry per nonempty arm, nested conditionals included — the
/// one-per-branch contract of [`collect_properties`]. Its raw text, directives and
/// all, is not a value: as one entry it reached `singular_property_value` as a
/// confident single table name.
fn lower_property(node: RawNode, source: &str, out: &mut Vec<crate::ir::ObjectProperty>) {
    let Some(name_node) = node.field(FieldName::Name) else {
        return;
    };
    let name = name_node.text(source).trim().fold_identifier();
    let mut values = Vec::new();
    match node.field(FieldName::Value) {
        Some(v) => property_arm_values(v, source, &mut values),
        None => values.push(String::new()),
    }
    for value in values {
        out.push(crate::ir::ObjectProperty {
            name: name.clone(),
            value,
            origin: origin_of(node),
        });
    }
}

/// The value text(s) of one property value node: itself, or every arm of a
/// whole-value `#if`, recursively. A run of whole-value `#if` groups
/// (`preproc_conditional_property_value_sequence`, grammar 8682cb5f, B11:
/// `Caption = #if X 'a'; #endif #if not X 'b'; #endif`) holds one such group per
/// `value` field, and gives every arm of each.
fn property_arm_values(v: RawNode, source: &str, out: &mut Vec<String>) {
    if matches!(
        v.kind(),
        RawKind::PreprocConditionalPropertyValue | RawKind::PreprocConditionalPropertyValueSequence
    ) {
        for arm in v.children_by_field(FieldName::Value) {
            property_arm_values(arm, source, out);
        }
    } else {
        out.push(v.text(source).trim().to_string());
    }
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
        RawKind::Property => lower_property(node, source, out),
        _ if is_preproc_wrapper(node) => {
            for c in node.named_children() {
                collect_properties(c, source, out);
            }
        }
        _ => {}
    }
}

/// DFS collecting `(routine, pending attributes, enclosing-dataitem-source-table,
/// enclosing-member, in-dataset-modify-context, build context)` tuples. AL has no nested
/// routines, so we do not descend into a routine once found. When the walk crosses into
/// a report `dataitem(Name; "Source Table")`, the (innermost) dataitem's source table is
/// threaded down so a dataitem trigger gets its implicit-`Rec` type.
///
/// **Attributes (issue #80).** `attribute_item` nodes are SIBLINGS preceding the
/// routine (grammar v2+). Each one waits in `pending` as a [`PendingAttr`]: the node and
/// its GUARD, the build context under which it still waits (its `ctx`, plus every `#if`
/// path it crossed; none when `ctx` contradicts itself, which no build reaches). The
/// next routine takes every pending entry, sorted by start byte. The run survives:
/// - the grammar's `extras` (`grammar.js:539-547`: comments, `#pragma`,
///   `#region`/`#endregion`, `#define`/`#undef`), which cannot hold a routine;
/// - a single-chain `#if` wrapper (one `#if` … `#endif`). Its arms are walked exactly
///   as before (a live arm under `ctx` ∪ its symbols). An entry enters arm `k` when
///   that arm is live under the entry's guard and its symbols are `consistent` with
///   it (the join becomes the guard), and leaves the wrapper unconsumed by the
///   no-arm-taken path (`PreprocChain::falls_through`) on the same terms. After the
///   wrapper, `pending` is every arm's leftovers then the fall-through entries, settled
///   by [`settle_pending`]: guards are projected onto the symbols a later condition or
///   the attribute's own argument groups mention, exact duplicates are removed, and an
///   attribute with more than [`PENDING_PATH_CAP`] paths in the list being settled
///   (one scope's `pending`, e.g. an inner arm's; not counted across the object)
///   loses those paths, with ONE `SyntaxIssue`.
///
/// Any other node (a section, a field, a `var` section, a multi-chain wrapper, …)
/// ends the run: `pending` is cleared, and the node is walked with an empty run whose
/// leftovers are discarded, so an attribute never leaves the body it is in. The
/// function returns the entries still pending after `children`.
/// `cond_syms` is [`preproc_condition_symbols`] of the whole object.
///
/// `dataset_ctx` (Task 1, dataitem-receivers plan) tracks whether the walk is currently
/// inside a report/report-extension `dataset` section: `true` on entering
/// `dataset_section`/`report_dataitem`, force-reset to `false` on entering
/// `requestpage_section` (REQUESTPAGE ISOLATION — binding, never leaks a dataset
/// `modify()`'s context into a requestpage trigger), inherited unchanged otherwise. Used
/// only to compute `in_dataset_modify_context` at push time — see that tuple field's doc
/// and [`crate::ir::RoutineDecl::in_dataset_modify_context`].
///
/// `ctx` is the symbols the object-level `#if` branches around `children` decide
/// (see [`preproc_branches`]); each routine is pushed with it, so a routine that
/// exists only under `#if X` carries `X` (`RoutineDecl::preproc_context`). A branch
/// the enclosing ones decide false is not walked.
#[allow(clippy::type_complexity, clippy::too_many_arguments)]
fn collect_routines<'t>(
    children: Vec<RawNode<'t>>,
    dataitem_table: Option<&str>,
    member: Option<RawNode<'t>>,
    dataset_ctx: bool,
    ctx: &[(String, bool)],
    mut pending: Vec<PendingAttr<'t>>,
    cond_syms: &CondIndex,
    source: &str,
    issues: &mut Vec<crate::ir::SyntaxIssue>,
    out: &mut Vec<(
        RawNode<'t>,
        Vec<PendingAttr<'t>>,
        Option<String>,
        Option<RawNode<'t>>,
        bool,
        Vec<(String, bool)>,
    )>,
) -> Vec<PendingAttr<'t>> {
    for child in children {
        match child.kind() {
            RawKind::AttributeItem => {
                if let Some(guard) = consistent(&[], ctx) {
                    pending.push(PendingAttr { node: child, guard });
                }
            }
            // The grammar's `extras` (`grammar.js:539-547`) may sit anywhere and can
            // hold no routine, so they do not end an attribute run (#80).
            RawKind::Comment
            | RawKind::MultilineComment
            | RawKind::Pragma
            | RawKind::PreprocRegion
            | RawKind::PreprocEndregion
            | RawKind::PreprocDefine
            | RawKind::PreprocUndef => {}
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
                // Arms and fall-through return entries out of source order.
                pending.sort_by_key(|e| e.node.byte_range().start);
                out.push((
                    child,
                    std::mem::take(&mut pending),
                    dataitem_table.map(str::to_string),
                    member,
                    in_dataset_modify_context,
                    ctx.to_vec(),
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
                collect_routines(
                    child.named_children(),
                    inner.as_deref(),
                    Some(child),
                    true,
                    ctx,
                    Vec::new(),
                    cond_syms,
                    source,
                    issues,
                    out,
                );
            }
            RawKind::DatasetSection => {
                pending.clear();
                collect_routines(
                    child.named_children(),
                    dataitem_table,
                    member,
                    true,
                    ctx,
                    Vec::new(),
                    cond_syms,
                    source,
                    issues,
                    out,
                );
            }
            RawKind::RequestpageSection => {
                // REQUESTPAGE ISOLATION (binding, dataitem-receivers plan round-1
                // addendum): force dataset context OFF regardless of the ambient value —
                // a requestpage trigger (or a requestpage/layout `modify()`) must NEVER
                // be treated as report-dataset context, even under a pathological/
                // decompiled nesting the real AL compiler would never accept ("parse
                // structure, don't validate").
                pending.clear();
                collect_routines(
                    child.named_children(),
                    dataitem_table,
                    member,
                    false,
                    ctx,
                    Vec::new(),
                    cond_syms,
                    source,
                    issues,
                    out,
                );
            }
            // `preproc_split_modify` (grammar 2026-10): `#if modify(A) #else modify(B)
            // #endif { … }` — the target differs per arm, the body is shared. Its
            // `target` repeats per arm; the first arm's is the member (first-branch-wins).
            // It sits in a layout or an actions list, never a report dataset, so it is not
            // added to the `in_dataset_modify_context` gate above.
            RawKind::ModifyModification
            | RawKind::ModifyActionModification
            | RawKind::PreprocSplitModify => {
                pending.clear();
                // Both `modify_modification` (fields/dataset/layout) and
                // `modify_action_modification` (an `actions` section) carry the modified
                // member's name in their `target` field, not `name` — always treated as
                // a named member wrapper (unlike the generic gate below, no `Name`-field
                // probe needed). See `lower_routine`'s enclosing-member fallback for the
                // name extraction. THIS arm is the load-bearing gate: without it the
                // generic arm below inherits the outer member and the fallback never
                // sees the wrapper at all.
                collect_routines(
                    child.named_children(),
                    dataitem_table,
                    Some(child),
                    dataset_ctx,
                    ctx,
                    Vec::new(),
                    cond_syms,
                    source,
                    issues,
                    out,
                );
            }
            // An object-level `#if` (`#if X procedure Foo(..) .. #else procedure
            // Foo(..) .. #endif`): each live branch's routines exist under that
            // branch's symbols. Pending attributes cross a single-chain wrapper by
            // guard (see the doc above); any other wrapper ends the run.
            _ if is_preproc_wrapper(child) => {
                // One walk of the wrapper's children: its directives (read once per
                // pending entry below) and its branch contents.
                let (directives, contents) = chain_parts(child);
                let single = is_single_chain(&directives);
                let incoming = if single {
                    std::mem::take(&mut pending)
                } else {
                    pending.clear();
                    Vec::new()
                };
                // Each entry's own view of the chain: liveness under its guard.
                // Directives only: the branch contents are grouped once, above.
                let chains: Vec<_> = incoming
                    .iter()
                    .map(|e| chain_shape(&directives, source, &in_syms(&e.guard)))
                    .collect();
                let (shape, _) = chain_shape(&directives, source, &in_syms(ctx));
                let last = shape.len() - 1;
                for (k, ((syms, live), branch)) in shape.into_iter().zip(contents).enumerate() {
                    if !live {
                        continue;
                    }
                    // Entry 0 (before `#if`) and the last group (after `#endif`) are
                    // no arm; for a single chain both are empty (ledger P16).
                    let arm = single && k != 0 && k != last;
                    let admitted = if arm {
                        incoming
                            .iter()
                            .zip(&chains)
                            .filter_map(|(e, c)| admit(e, &c.0[k]))
                            .collect()
                    } else {
                        Vec::new()
                    };
                    let inner: Vec<(String, bool)> = ctx.iter().cloned().chain(syms).collect();
                    let left = collect_routines(
                        branch,
                        dataitem_table,
                        member,
                        dataset_ctx,
                        &inner,
                        admitted,
                        cond_syms,
                        source,
                        issues,
                        out,
                    );
                    if arm {
                        pending.extend(left);
                    }
                }
                pending.extend(incoming.into_iter().zip(chains).filter_map(|(e, c)| {
                    Some(PendingAttr {
                        guard: consistent(&e.guard, &c.1?)?,
                        node: e.node,
                    })
                }));
                settle_pending(&mut pending, child.byte_range().end, cond_syms, issues);
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
                    child.named_children(),
                    dataitem_table,
                    child_member,
                    dataset_ctx,
                    ctx,
                    Vec::new(),
                    cond_syms,
                    source,
                    issues,
                    out,
                );
            }
        }
    }
    pending
}

/// One attribute waiting for its routine (`collect_routines`), and its guard: the
/// build context, projected by [`settle_pending`], under which it still waits.
#[derive(Clone)]
struct PendingAttr<'t> {
    node: RawNode<'t>,
    guard: Syms,
}

/// More distinct guards than this for one attribute in one `pending` list being
/// settled: those paths are dropped, with a `SyntaxIssue` (see [`settle_pending`]).
const PENDING_PATH_CAP: usize = 64;

/// A wrapper with exactly one `#if` … `#endif` chain, the only shape an attribute
/// crosses (ledger P16: every wrapper that can hold an attribute has it).
fn is_single_chain(directives: &[RawNode]) -> bool {
    let count = |k| directives.iter().filter(|c| c.kind() == k).count();
    count(RawKind::PreprocIf) == 1 && count(RawKind::PreprocEndif) == 1
}

/// Entry `e` admitted to an arm `(syms, live, _)` of its own chain view: the arm is
/// live under `e`'s guard and its symbols agree with it; the join is the new guard.
fn admit<'t>(e: &PendingAttr<'t>, (syms, live): &(Syms, bool)) -> Option<PendingAttr<'t>> {
    if !live {
        return None;
    }
    Some(PendingAttr {
        node: e.node,
        guard: consistent(&e.guard, syms)?,
    })
}

/// Settle `pending` after a wrapper ending at byte `end`:
/// 1. **Project** each guard onto the symbols that can still matter: those a
///    `#if`/`#elif` condition starting at or after `end` mentions, or a condition
///    inside the attribute itself (its argument groups). Nothing else reads a guard,
///    so dropping the other literals changes no attachment and no reading, and keeps
///    `pending` small (40 unrelated pragma-only wrappers leave one entry).
/// 2. Remove exact duplicates (same node, same guard as a set), keeping the first.
/// 3. **Cap**: an attribute with more than [`PENDING_PATH_CAP`] guards left in THIS
///    `pending` list (the scope being settled, e.g. one arm's walk; not a count over
///    the whole object) loses those paths, with one `SyntaxIssue` at the attribute; it
///    is never attached under a wrong guard.
fn settle_pending(
    pending: &mut Vec<PendingAttr>,
    end: usize,
    cond: &CondIndex,
    issues: &mut Vec<crate::ir::SyntaxIssue>,
) {
    use std::collections::{HashMap, HashSet};
    if pending.is_empty() {
        return;
    }
    let cond_syms = cond.get();
    let at = |b: usize| cond_syms.syms.partition_point(|(s, _)| *s < b);
    for e in pending.iter_mut() {
        let own = e.node.byte_range();
        let inside = &cond_syms.syms[at(own.start)..at(own.end)];
        e.guard.retain(|(s, _)| {
            cond_syms.last.get(s.as_str()).is_some_and(|l| *l >= end)
                || inside.iter().any(|(_, t)| t == s)
        });
        e.guard.sort();
    }
    let mut seen: HashSet<(usize, Syms)> = HashSet::new();
    pending.retain(|e| seen.insert((e.node.id(), e.guard.clone())));
    let mut paths: HashMap<usize, usize> = HashMap::new();
    for e in pending.iter() {
        *paths.entry(e.node.id()).or_default() += 1;
    }
    let mut reported: HashSet<usize> = HashSet::new();
    for e in pending.iter() {
        let id = e.node.id();
        if paths[&id] > PENDING_PATH_CAP && reported.insert(id) {
            issues.push(crate::ir::SyntaxIssue {
                message: format!(
                    "attribute reaches its routine through more than {PENDING_PATH_CAP} distinct `#if` paths; those paths are not attached"
                ),
                origin: origin_of(e.node),
            });
        }
    }
    pending.retain(|e| paths[&e.node.id()] <= PENDING_PATH_CAP);
}

/// The condition symbols of an object ([`preproc_condition_symbols`]), plus each
/// symbol's last start byte. Built on first use: most objects have no pending
/// attribute at a wrapper and never need it.
struct CondIndex<'a, 't> {
    node: RawNode<'t>,
    source: &'a str,
    cell: std::cell::OnceCell<CondSyms>,
}

struct CondSyms {
    /// `(start byte, symbol)` in byte order.
    syms: Vec<(usize, String)>,
    /// symbol -> start byte of its last occurrence.
    last: std::collections::HashMap<String, usize>,
}

impl<'a, 't> CondIndex<'a, 't> {
    fn new(node: RawNode<'t>, source: &'a str) -> Self {
        Self {
            node,
            source,
            cell: std::cell::OnceCell::new(),
        }
    }

    fn get(&self) -> &CondSyms {
        self.cell.get_or_init(|| {
            let syms = preproc_condition_symbols(self.node, self.source);
            let last = syms.iter().map(|(b, s)| (s.clone(), *b)).collect();
            CondSyms { syms, last }
        })
    }
}

/// Every `(start byte, symbol)` that a `#if`/`#elif` condition anywhere under
/// `node` mentions (split headers and argument groups included), in byte order.
/// Iterative: it walks routine bodies, whose expression trees can be deep.
fn preproc_condition_symbols(node: RawNode, source: &str) -> Vec<(usize, String)> {
    // No text pre-check: the scanner also accepts `# if` and `#IF`, so only the
    // parsed tree can tell (issue #80 review).
    let mut out = Vec::new();
    let mut stack = vec![(node, false)];
    while let Some((n, in_cond)) = stack.pop() {
        if in_cond && n.kind() == RawKind::Identifier {
            out.push((n.byte_range().start, n.text(source).trim().to_string()));
            continue;
        }
        let cond = matches!(n.kind(), RawKind::PreprocIf | RawKind::PreprocElif)
            .then(|| n.field(FieldName::Condition))
            .flatten()
            .map(|c| c.id());
        for c in n.named_children() {
            stack.push((c, in_cond || Some(c.id()) == cond));
        }
    }
    out.sort();
    out
}

/// The unquoted source-table name of a `report_dataitem(Name; "Source Table")` node
/// (its `table_name` field). `None` if absent.
fn dataitem_table_name(node: RawNode, source: &str) -> Option<String> {
    node.field(FieldName::TableName)
        .map(|n| ident_text(n, source))
        .filter(|s| !s.is_empty())
}

/// The names of the globals declared in a `protected var` section, descending
/// preproc wrappers like [`collect_globals`]. An extension of the object can read
/// them (AL "protected variables").
fn collect_protected_names(node: RawNode, source: &str, out: &mut Vec<String>) {
    match node.kind() {
        RawKind::VarSection
            if node
                .named_children()
                .iter()
                .any(|c| c.kind() == RawKind::ProtectedKeyword) =>
        {
            let mut decls = Vec::new();
            extract_var_section(node, source, &mut decls);
            out.extend(decls.into_iter().map(|d| d.name));
        }
        _ if is_preproc_wrapper(node) => {
            for c in node.named_children() {
                collect_protected_names(c, source, out);
            }
        }
        _ => {}
    }
}

/// Collect object-level var declarations, descending preproc wrappers (both
/// branches) but NOT routines/sections-with-their-own-scope.
fn collect_globals(node: RawNode, source: &str, out: &mut Vec<VarDecl>) {
    match node.kind() {
        RawKind::VarSection => extract_var_section(node, source, out),
        // A `#if` arm that CONTINUES the var section before it and then starts
        // procedures (grammar 2026-10; AOAIDeploymentsImpl.Codeunit.al). The var
        // section ends inside the arm, so the node is the section's SIBLING; each
        // arm's continued declarations are a `variables` field (a `var_body`). Its
        // procedures are ordinary children, found by `collect_routines`.
        RawKind::PreprocSplitVarSectionTail => {
            for body in node.children_by_field(FieldName::Variables) {
                extract_var_body(body, source, out);
            }
        }
        _ if is_preproc_wrapper(node) => {
            for c in node.named_children() {
                collect_globals(c, source, out);
            }
        }
        _ => {}
    }
}

/// The header arms of a routine node, as byte ranges, plus where its shared tail
/// starts. A `preproc_split_procedure` / `preproc_split_procedure_preamble` repeats
/// its header (`name`, `parameters`, `modifier`, return, attributes and, for the
/// preamble, a `var` section) once per `#if`/`#elif`/`#else` arm, and every build
/// compiles exactly one arm with the shared body after `#endif`. Each arm is a real
/// routine under some build, so each lowers to its own `RoutineDecl` (the union-read
/// this lowerer applies to every `#if`; a whole procedure duplicated across arms
/// already yields two, `preproc_both_arms_distinct_signature_yield_two_routine_decls`).
/// Reading only the first arm made a call written for the `#else` signature an
/// `arityMismatch` (Base App `MfgCalculateBOMTree.CalcRoutingLineCosts`).
/// Any other routine node is one arm covering the whole node.
fn header_arms(node: RawNode) -> (Vec<std::ops::Range<usize>>, usize) {
    if !matches!(
        node.kind(),
        RawKind::PreprocSplitProcedure | RawKind::PreprocSplitProcedurePreamble
    ) {
        let r = node.byte_range();
        return (vec![r.clone()], r.end);
    }
    let mut arms = Vec::new();
    let mut open: Option<usize> = None;
    let mut shared_from = node.byte_range().end;
    for c in node.named_children() {
        match c.kind() {
            RawKind::PreprocIf | RawKind::PreprocElif | RawKind::PreprocElse => {
                if let Some(s) = open.take() {
                    arms.push(s..c.byte_range().start);
                }
                open = Some(c.byte_range().end);
            }
            RawKind::PreprocEndif => {
                if let Some(s) = open.take() {
                    arms.push(s..c.byte_range().start);
                }
                shared_from = c.byte_range().end;
            }
            _ => {}
        }
    }
    (arms, shared_from)
}

thread_local! {
    /// The build context of the routine body being lowered: the symbols its
    /// routine's conditions decide (`RoutineDecl::preproc_context`) plus those of
    /// every enclosing `#if` branch inside the body. A body `#if` branch the context
    /// decides false is left out (`lower_block_child`): `#else` under
    /// `#if not CLEAN27` compiles with `CLEAN27` defined, so Base App's `#else` arm of
    /// `CalcRoutingLineCosts`, which has no `sender` parameter, does not contain the
    /// body's `#if not CLEAN27` blocks that use `sender`. Every expression lowered
    /// under a non-empty context is recorded with it (`Ir::preproc_context`), so a
    /// call's overload selection sees its build. Empty outside routine bodies.
    /// ponytail: thread-local, not a lowering-context parameter; a lowering pass is
    /// one file on one thread, and `with_build_context` restores it on exit.
    static BUILD_CONTEXT: std::cell::RefCell<Vec<(String, bool)>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// The current build context (`BUILD_CONTEXT`).
fn build_context() -> Vec<(String, bool)> {
    BUILD_CONTEXT.with(|s| s.borrow().clone())
}

/// Run `f` with `extra` added to the build context, recording the expressions
/// `f` lowers with the whole resulting context.
fn with_build_context<R>(extra: &[(String, bool)], ir: &mut Ir, f: impl FnOnce(&mut Ir) -> R) -> R {
    if extra.is_empty() {
        return f(ir);
    }
    let (outer, full) = BUILD_CONTEXT.with(|s| {
        let mut s = s.borrow_mut();
        let outer = s.len();
        s.extend_from_slice(extra);
        (outer, s.clone())
    });
    let first = ir.expr_count();
    let r = f(ir);
    ir.record_preproc_context(first, &full);
    BUILD_CONTEXT.with(|s| s.borrow_mut().truncate(outer));
    r
}

/// Symbol literals that a build path decides, in the form of a build context
/// (`RoutineDecl::preproc_context`).
type Syms = Vec<(String, bool)>;

/// The children of a `#if` wrapper, grouped by branch. Each group carries the
/// symbols its branch decides (its own condition true, every earlier condition of
/// the chain false; nothing for content outside any branch) and whether it is live
/// under `ctx`: a branch `ctx` decides false, or one after a branch `ctx` decides
/// true, is dead. Directive nodes belong to no group.
fn preproc_branches<'t>(
    node: RawNode<'t>,
    source: &str,
    ctx: &[(String, bool)],
) -> Vec<(Syms, bool, Vec<RawNode<'t>>)> {
    let (directives, contents) = chain_parts(node);
    branches_of(&directives, contents, source, &in_syms(ctx))
}

/// [`preproc_branches`] over a wrapper already split by [`chain_parts`], with
/// liveness read through `lookup`.
fn branches_of<'t>(
    directives: &[RawNode],
    contents: Vec<Vec<RawNode<'t>>>,
    source: &str,
    lookup: &dyn Fn(&str) -> Option<bool>,
) -> Vec<(Syms, bool, Vec<RawNode<'t>>)> {
    chain_shape(directives, source, lookup)
        .0
        .into_iter()
        .zip(contents)
        .map(|((syms, live), items)| (syms, live, items))
        .collect()
}

/// One walk over the children of `node`: its directive nodes (`#if`/`#elif`/
/// `#else`/`#endif`, the input of [`chain_shape`]) and the other children grouped by
/// branch, aligned with [`chain_shape`]'s arms (entry 0 before the first directive,
/// one per directive after it).
fn chain_parts<'t>(node: RawNode<'t>) -> (Vec<RawNode<'t>>, Vec<Vec<RawNode<'t>>>) {
    let mut directives = Vec::new();
    let mut out = vec![Vec::new()];
    for c in node.named_children() {
        match c.kind() {
            RawKind::PreprocIf
            | RawKind::PreprocElif
            | RawKind::PreprocElse
            | RawKind::PreprocEndif => {
                directives.push(c);
                out.push(Vec::new());
            }
            _ => out.last_mut().expect("never empty").push(c),
        }
    }
    (directives, out)
}

/// The directive walk of [`preproc_branches`], over a wrapper's `directives` only
/// ([`chain_parts`]), with no branch contents: per arm its `(symbols, live under
/// the context `lookup` reads)`, plus the symbols of the build that takes no arm
/// (every `#if`/`#elif` condition false): `Some` only when the chain has no `#else`
/// and the context decides no arm true. The fall-through is read for the LAST chain
/// of the wrapper (a single-chain wrapper has one). It costs the directives, not the
/// wrapper's children, so it runs once per pending attribute.
fn chain_shape(
    directives: &[RawNode],
    source: &str,
    lookup: &dyn Fn(&str) -> Option<bool>,
) -> (Vec<(Syms, bool)>, Option<Syms>) {
    let mut out = vec![(Vec::new(), true)];
    let mut earlier: Vec<RawNode> = Vec::new();
    let mut taken = false;
    let mut has_else = false;
    let mut falls_through = None;
    for &c in directives {
        match c.kind() {
            RawKind::PreprocIf | RawKind::PreprocElif => {
                let cond = c.field(FieldName::Condition);
                let d = cond.and_then(|e| decide_preproc_by(e, source, lookup));
                let live = !taken && d != Some(false);
                taken |= d == Some(true);
                let mut syms = Vec::new();
                for e in &earlier {
                    assume_preproc(*e, false, source, &mut syms);
                }
                if let Some(e) = cond {
                    assume_preproc(e, true, source, &mut syms);
                    earlier.push(e);
                }
                out.push((syms, live));
            }
            RawKind::PreprocElse => {
                has_else = true;
                let mut syms = Vec::new();
                for e in &earlier {
                    assume_preproc(*e, false, source, &mut syms);
                }
                out.push((syms, !taken));
            }
            RawKind::PreprocEndif => {
                falls_through = (!has_else && !taken).then(|| {
                    let mut syms = Vec::new();
                    for e in &earlier {
                        assume_preproc(*e, false, source, &mut syms);
                    }
                    syms
                });
                earlier.clear();
                taken = false;
                has_else = false;
                out.push((Vec::new(), true));
            }
            _ => {}
        }
    }
    (out, falls_through)
}

/// The items of an `#if` group inside a POSITIONAL delimited list (grammar `main`
/// 8682cb5f, B7b-1: `key(PK; A #if X , B #endif , C)`, `[Attr(A, #if X B, #else C,
/// #endif D)]`), as one build reads them: the first arm that is live under `run`
/// and consistent with it, the first-branch-wins policy of every split header. A
/// union of the arms is no build at all and shifts every later position (an
/// `[EventSubscriber]`'s event name is `args[2]`). A group nested in the chosen arm
/// is read the same way; when every arm is decided false (an `#if` with no `#else`)
/// the group adds nothing. Appends to `out` in document order.
///
/// `run` is the RUNNING context of the whole list: the caller starts it once per
/// list and passes it to every group, and the chosen arm's symbols join it before
/// that arm's items are read. So a nested group reads under the arm that holds it,
/// and a later sibling group picks an arm the same build can take (issue #80). An
/// arm whose symbols contradict `run` (`#if Y and not Y`, or `#if not X` after an
/// earlier group chose `X`) is never picked.
///
/// LIMIT: `run` holds only symbol LITERALS. An arm chosen under a compound condition
/// such as `X or Y` records none (neither is forced), so it does not constrain a
/// later group under `not X and not Y`; the two arms can then pair although no build
/// takes both (pinned by `sibling_groups_under_a_disjunction_are_not_correlated`).
fn conditional_list_arm<'t>(
    group: RawNode<'t>,
    source: &str,
    run: &mut Run,
    out: &mut Vec<RawNode<'t>>,
) {
    // Entry 0 is the content before `#if` and the last the content after `#endif`
    // (live, and empty in a list group), so a group with no live arm reads nothing.
    // When no arm is consistent with `run`, `find_map` reaches that trailing entry
    // (empty syms, so consistent; empty items), and the group reads nothing.
    let (directives, contents) = chain_parts(group);
    let branches = branches_of(&directives, contents, source, &|s| run.get(s));
    // `Run::join` extends `run` in place, and only for the arm it returns.
    let Some((_, _, items)) = branches
        .into_iter()
        .skip(1)
        .filter(|(_, live, _)| *live)
        .find(|(syms, _, _)| run.join(syms))
    else {
        return;
    };
    for item in items {
        if item.kind() == group.kind() {
            conditional_list_arm(item, source, run, out);
        } else if crate::schema::class_of(item.kind()) != crate::schema::Class::Trivia {
            out.push(item);
        }
    }
}

/// `a ∪ b` in `a`'s order, then `b`'s new symbols; `None` when the two together give
/// one symbol both values (inside either operand too), which no build does.
/// Hashed: linear in the two lengths.
fn consistent(a: &[(String, bool)], b: &[(String, bool)]) -> Option<Syms> {
    use std::collections::hash_map::Entry;
    let mut seen: std::collections::HashMap<&str, bool> =
        std::collections::HashMap::with_capacity(a.len() + b.len());
    let mut out: Syms = Vec::new();
    for (s, v) in a.iter().chain(b) {
        match seen.entry(s.as_str()) {
            Entry::Occupied(w) if w.get() != v => return None,
            Entry::Occupied(_) => {}
            Entry::Vacant(slot) => {
                slot.insert(*v);
                out.push((s.clone(), *v));
            }
        }
    }
    Some(out)
}

/// The running context of one positional list ([`conditional_list_arm`]), as a
/// symbol index that every join and every condition lookup reads, so neither scans
/// a list. Nothing reads its literals in order, so it keeps none.
#[derive(Default)]
struct Run {
    index: std::collections::HashMap<String, bool>,
}

impl Run {
    /// A context started from `syms` (a build guard: one literal per symbol; the
    /// first wins, as a slice lookup reads it).
    fn new(syms: &[(String, bool)]) -> Self {
        let mut run = Self::default();
        for (s, v) in syms {
            run.index.entry(s.clone()).or_insert(*v);
        }
        run
    }

    fn get(&self, s: &str) -> Option<bool> {
        self.index.get(s).copied()
    }

    /// [`consistent`] `(self, b)` written into `self`: `false` (and `self`
    /// unchanged) when `b` contradicts itself or `self`. `self` is already
    /// consistent, so only `b`'s literals are checked and appended.
    fn join(&mut self, b: &[(String, bool)]) -> bool {
        let Some(b) = consistent(b, &[]) else {
            return false;
        };
        if b.iter().any(|(s, v)| self.get(s).is_some_and(|w| w != *v)) {
            return false;
        }
        for (s, v) in b {
            self.index.entry(s).or_insert(v);
        }
        true
    }
}

/// Record the symbol values that make preprocessor condition `cond` equal `value`,
/// where they follow (`A and B` true: both true; `A or B` false: both false).
/// Symbols match by exact text, as alc's do (see `al_syntax::ir::PreprocSymbols`).
fn assume_preproc(cond: RawNode, value: bool, source: &str, out: &mut Vec<(String, bool)>) {
    let kids = structural_children(cond);
    match cond.kind() {
        RawKind::Identifier => out.push((cond.text(source).trim().to_string(), value)),
        RawKind::PreprocParenthesizedExpression => {
            kids.into_iter()
                .for_each(|k| assume_preproc(k, value, source, out));
        }
        RawKind::PreprocNotExpression => {
            kids.into_iter()
                .for_each(|k| assume_preproc(k, !value, source, out));
        }
        RawKind::PreprocAndExpression if value => {
            kids.into_iter()
                .for_each(|k| assume_preproc(k, true, source, out));
        }
        RawKind::PreprocOrExpression if !value => {
            kids.into_iter()
                .for_each(|k| assume_preproc(k, false, source, out));
        }
        _ => {}
    }
}

/// Condition `cond` under decided symbols `syms`, three-valued: `None` when
/// undecided.
fn decide_preproc(cond: RawNode, source: &str, syms: &[(String, bool)]) -> Option<bool> {
    decide_preproc_by(cond, source, &in_syms(syms))
}

/// A symbol lookup over decided symbols `syms` (the first literal wins).
fn in_syms(syms: &[(String, bool)]) -> impl Fn(&str) -> Option<bool> + '_ {
    move |t| syms.iter().find(|(s, _)| s == t).map(|(_, v)| *v)
}

/// [`decide_preproc`] with each symbol's decided value read through `lookup`.
fn decide_preproc_by(
    cond: RawNode,
    source: &str,
    lookup: &dyn Fn(&str) -> Option<bool>,
) -> Option<bool> {
    let mut kids = structural_children(cond)
        .into_iter()
        .map(|k| decide_preproc_by(k, source, lookup));
    match cond.kind() {
        RawKind::Identifier => lookup(cond.text(source).trim()),
        RawKind::PreprocParenthesizedExpression => kids.next().flatten(),
        RawKind::PreprocNotExpression => kids.next().flatten().map(|v| !v),
        RawKind::PreprocAndExpression => {
            let (a, b) = (kids.next().flatten(), kids.next().flatten());
            match (a, b) {
                (Some(false), _) | (_, Some(false)) => Some(false),
                (Some(true), Some(true)) => Some(true),
                _ => None,
            }
        }
        RawKind::PreprocOrExpression => {
            let (a, b) = (kids.next().flatten(), kids.next().flatten());
            match (a, b) {
                (Some(true), _) | (_, Some(true)) => Some(true),
                (Some(false), Some(false)) => Some(false),
                _ => None,
            }
        }
        _ => None,
    }
}

/// The symbols arm `arm` of split routine `node` decides: its own `#if`/`#elif`
/// condition true, every earlier arm's condition false.
fn arm_symbols(node: RawNode, arm: &std::ops::Range<usize>, source: &str) -> Vec<(String, bool)> {
    let mut out = Vec::new();
    for c in node.named_children() {
        let end = c.byte_range().end;
        if end > arm.start {
            break;
        }
        if matches!(c.kind(), RawKind::PreprocIf | RawKind::PreprocElif)
            && let Some(cond) = c.field(FieldName::Condition)
        {
            assume_preproc(cond, end == arm.start, source, &mut out);
        }
    }
    out
}

/// The `#if`/`#elif` conditions of a split routine header, each with the end byte
/// of its directive, in order; empty for an unsplit routine (one arm, nothing to
/// decide). Read once per routine, then by [`arm_live`] once per pending entry.
fn split_conditions(node: RawNode) -> Vec<(RawNode, usize)> {
    if !matches!(
        node.kind(),
        RawKind::PreprocSplitProcedure | RawKind::PreprocSplitProcedurePreamble
    ) {
        return Vec::new();
    }
    node.named_children()
        .into_iter()
        .filter(|c| matches!(c.kind(), RawKind::PreprocIf | RawKind::PreprocElif))
        .filter_map(|c| Some((c.field(FieldName::Condition)?, c.byte_range().end)))
        .collect()
}

/// The arm starting at byte `arm_start` of a split routine with header conditions
/// `conds` ([`split_conditions`]) is reachable under `guard`: its own condition is
/// not decided false, and no earlier condition of the chain is decided true (the
/// rule [`chain_shape`] gives a wrapper arm's `live`).
fn arm_live(
    conds: &[(RawNode, usize)],
    arm_start: usize,
    source: &str,
    guard: &[(String, bool)],
) -> bool {
    for &(cond, end) in conds {
        if end > arm_start {
            break;
        }
        let d = decide_preproc(cond, source, guard);
        // The arm's own condition is dead when false, an earlier one when true.
        if d == Some(end != arm_start) {
            return false;
        }
    }
    true
}

/// One argument list of an attribute as one build reads it.
struct AttributeArgList<'t> {
    /// The `#if` groups in the list (each gets a `SyntaxIssue` when lowered).
    groups: Vec<RawNode<'t>>,
    /// The positional arguments that build reads, in order.
    items: Vec<RawNode<'t>>,
}

/// The argument lists of attribute `item` as the build `guard` reads them, before any
/// lowering. H-8: a comment interleaved between args never occupies a positional slot
/// (see `structural_children`'s doc: the `[EventSubscriber(...)]` silent-unregister
/// case). An `#if` group among the args (grammar 8682cb5f) gives the args of the arm
/// that build reads, for the same reason, with one running context per list started
/// from `guard`.
fn attribute_arg_selection<'t>(
    item: RawNode<'t>,
    guard: &[(String, bool)],
    source: &str,
) -> Vec<AttributeArgList<'t>> {
    let Some(args_node) = item
        .field(FieldName::Attribute)
        .and_then(|c| c.field(FieldName::Arguments))
    else {
        return Vec::new();
    };
    args_node
        .named_children()
        .into_iter()
        .filter(|l| l.kind() == RawKind::AttributeArgumentList)
        .map(|list| {
            let mut out = AttributeArgList {
                groups: Vec::new(),
                items: Vec::new(),
            };
            let mut run = Run::new(guard);
            for arg in structural_children(list) {
                if arg.kind() == RawKind::PreprocConditionalAttributeArgs {
                    out.groups.push(arg);
                    conditional_list_arm(arg, source, &mut run, &mut out.items);
                } else {
                    out.items.push(arg);
                }
            }
            out
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
// 7 pre-existing params + `in_dataset_modify_context`
// (dataitem-receivers plan, Task 1) + the header arm; each is a distinct piece of
// context `collect_routines`'s DFS threads down — grouping would obscure the call
// site, mirrors `infer_receiver_type`'s identical precedent.
fn lower_routine<'t>(
    node: RawNode<'t>,
    mut attr_items: Vec<PendingAttr<'t>>,
    dataitem_source_table: Option<String>,
    member: Option<RawNode<'t>>,
    in_dataset_modify_context: bool,
    (arm, shared_from): (&std::ops::Range<usize>, usize),
    object_ctx: &[(String, bool)],
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
    // `name` path. That IS tree-sitter error recovery (issue #56): it inserts a hidden
    // MISSING keyword token under the `keyword_as_identifier` -> `identifier` alias, so
    // the field is a present, zero-width `identifier`. `root.has_error()` is true and
    // `ParseStatus` is `Recovered`. Only the `tree-sitter parse` CLI hides this (it never
    // reaches the hidden leaf, prints no MISSING and exits 0). `parse_incomplete` of the
    // ROUTINE stays false, because the error sits in the member header, not in the
    // trigger's own subtree. The guard still matters for `action("")` (an empty QUOTED
    // name), which genuinely parses Clean, and for any file that is read past a Recovered
    // status. `Some("")` takes the discriminated arm of `to_stable_routine_id_from_parts`
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
                    RawKind::ModifyModification
                        | RawKind::ModifyActionModification
                        | RawKind::PreprocSplitModify
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

    // A header field of THIS arm (`header_arms`); for an unsplit routine the arm is the
    // whole node, so this is the field's first child, as `field` returns.
    let in_arm = |n: RawNode| arm.contains(&n.byte_range().start);
    let arm_field = |f: FieldName| node.children_by_field(f).into_iter().find(|c| in_arm(*c));
    let split = matches!(
        node.kind(),
        RawKind::PreprocSplitProcedure | RawKind::PreprocSplitProcedurePreamble
    );
    // The builds this routine exists in: the object-level `#if` branches around
    // it, plus its own arm when its header is split.
    let mut preproc_context = object_ctx.to_vec();
    if split {
        preproc_context.extend(arm_symbols(node, arm, source));
        // A split header's arm carries its own attributes inside the node; they are
        // read under the routine's own context. A context that contradicts itself
        // (an outer `#if X` around a `#if not X` arm) is no build: they attach nothing.
        if let Some(guard) = consistent(&preproc_context, &[]) {
            attr_items.extend(
                node.named_children()
                    .into_iter()
                    .filter(|c| c.kind() == RawKind::AttributeItem && in_arm(*c))
                    .map(|c| PendingAttr {
                        node: c,
                        guard: guard.clone(),
                    }),
            );
        }
    }

    // One attribute node can reach this routine by several build paths. Keep the
    // first entry (source order) per (node, argument selection): paths that read the
    // same arguments are one attribute, paths that read different ones are one per
    // build (union-read). Selecting before `lower_expr` leaves no dead `ExprId`. The
    // `#if`-in-arguments `SyntaxIssue` is deduplicated apart from this, by group node
    // (`reported`), so it is raised once per group however many selections keep it.
    let mut seen: std::collections::HashSet<(usize, Vec<Vec<usize>>)> =
        std::collections::HashSet::new();
    let mut reported: std::collections::HashSet<usize> = std::collections::HashSet::new();
    let mut selected = Vec::new();
    for e in &attr_items {
        let lists = attribute_arg_selection(e.node, &e.guard, source);
        let key = (
            e.node.id(),
            lists
                .iter()
                .map(|l| l.items.iter().map(|n| n.id()).collect())
                .collect(),
        );
        if seen.insert(key) {
            selected.push((e.node, lists));
        }
    }

    // Attributes: lowercased names (for classify_kind / control-context guards) +
    // the full parsed form (name + raw text + lowered argument exprs).
    let mut attributes: Vec<String> = Vec::new();
    let mut attributes_parsed: Vec<crate::ir::AttributeIr> = Vec::new();
    for (item, lists) in selected {
        let Some(content) = item.field(FieldName::Attribute) else {
            continue;
        };
        let Some(name_node) = content.field(FieldName::Name) else {
            continue;
        };
        let raw_name = name_node.text(source).trim().to_string();
        attributes.push(raw_name.fold_identifier());
        let mut args = Vec::new();
        for list in lists {
            for group in list.groups {
                if reported.insert(group.id()) {
                    issues.push(SyntaxIssue {
                        message: "`#if` inside attribute arguments — the first \
                                  arm this routine's build does not rule out is \
                                  read (the other arms are not modelled)"
                            .to_string(),
                        origin: origin_of(group),
                    });
                }
            }
            for arg in list.items {
                args.push(lower_expr(arg, ir, issues, source, 0));
            }
        }
        attributes_parsed.push(crate::ir::AttributeIr {
            name: raw_name,
            raw: item.text(source).to_string(),
            args,
            origin: origin_of(item),
        });
    }
    let name = arm_field(FieldName::Name)
        .map(|n| routine_name_text(n, source))
        .unwrap_or_default();
    // Origin of the name identifier itself (for the LSP selection_range); fall back to
    // the whole-routine origin when the name node is absent.
    let name_origin = arm_field(FieldName::Name)
        .map(origin_of)
        .unwrap_or_else(|| origin_of(node));

    let params = arm_field(FieldName::Parameters)
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
    let return_type = arm_field(FieldName::ReturnType)
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
    let return_name = arm_field(FieldName::ReturnValue)
        .map(|n| ident_text(n, source))
        .filter(|s| !s.is_empty());

    // Access modifier (`local`/`internal`/`protected`); None = public / trigger.
    let access_modifier = arm_field(FieldName::Modifier).and_then(|m| {
        match m.text(source).trim().to_ascii_lowercase().as_str() {
            "local" => Some("local".to_string()),
            "internal" => Some("internal".to_string()),
            "protected" => Some("protected".to_string()),
            _ => None,
        }
    });

    // Locals: var_section child(ren) of the routine (+ preproc-wrapped): this arm's
    // own (a split preamble's) plus the shared tail's.
    let mut locals = Vec::new();
    for child in node.named_children() {
        if in_arm(child) || child.byte_range().start >= shared_from {
            collect_globals(child, source, &mut locals);
        }
    }

    let body = with_build_context(&preproc_context, ir, |ir| {
        if let Some(cb) = node
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
        }
    });

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
        preproc_context,
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
    // Each arm's body under its own branch's symbols (`preproc_branches`); a
    // shared tail after `#endif` under none.
    let bodies: Vec<usize> = wrapper
        .children_by_field(FieldName::Body)
        .iter()
        .map(|b| b.id())
        .collect();
    for (syms, live, branch) in preproc_branches(wrapper, source, &build_context()) {
        if live {
            with_build_context(&syms, ir, |ir| {
                for body in branch.into_iter().filter(|b| bodies.contains(&b.id())) {
                    for child in body.named_children() {
                        lower_block_child(child, ir, issues, source, &mut items, depth);
                    }
                }
            });
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
    if let Some(body) = section.field(FieldName::Body) {
        extract_var_body(body, source, out);
    }
}

/// The declarations of one `var_body`.
fn extract_var_body(body: RawNode, source: &str, out: &mut Vec<VarDecl>) {
    for decl in body.named_children() {
        match decl.kind() {
            RawKind::VariableDeclaration => extract_var_decl(decl, source, out),
            _ if is_preproc_wrapper(decl) => {
                for c in decl.named_children() {
                    if c.kind() == RawKind::VariableDeclaration {
                        // shallow: flatten preproc-wrapped declarations (both branches)
                        extract_var_decl(c, source, out);
                    }
                }
            }
            _ => {}
        }
    }
}

/// One `VarDecl` per name of a `variable_declaration` (none when it has no name).
fn extract_var_decl(decl: RawNode, source: &str, out: &mut Vec<VarDecl>) {
    let type_node = decl.field(FieldName::Type);
    let ty = type_node.map(|n| n.text(source).trim().to_string());
    let temporary = type_node
        .map(|t| contains_kind(t, RawKind::TemporaryKeyword))
        .unwrap_or(false);
    let mut names = Vec::new();
    var_decl_names(decl, &mut names);
    for nm in names {
        out.push(VarDecl {
            name: ident_text(nm, source),
            ty: ty.clone(),
            temporary,
            origin: origin_of(decl),
        });
    }
}

/// The `name` fields of a `variable_declaration`, in document order. An `#if` group
/// in the name list (`preproc_conditional_var_names`, grammar 8682cb5f: `A, #if X B,
/// #endif C: Integer`) carries its names on its own `name` fields; every arm's are
/// read, nested groups included, the both-branches policy of the preproc-wrapped
/// declarations above. A name is a declaration, not a position, so the union is
/// sound.
fn var_decl_names<'t>(node: RawNode<'t>, out: &mut Vec<RawNode<'t>>) {
    let names = node.children_by_field(FieldName::Name);
    for c in node.named_children() {
        if c.kind() == RawKind::PreprocConditionalVarNames {
            var_decl_names(c, out);
        } else if names.iter().any(|n| n.byte_range() == c.byte_range()) {
            out.push(c);
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
///
/// The same holds for the other three split closings in `code_block`'s closing
/// `choice`, each a flat run of statements around directive and `end`/`else`/`begin`
/// keyword nodes (the keywords are skipped by `lower_block_child`):
/// `preproc_split_else_begin_over_endif` (`end else begin … #endif end;`; it was in
/// the closing choice before the 2026-10 grammar too, and its content was dropped
/// here until then), and the 2026-10 `preproc_split_block_end_in_else` /
/// `preproc_split_block_close_after_endif` pair, a procedure boundary inside an
/// `#else` (`… #else C(); end; local procedure Q() begin … #endif end;`).
///
/// That pair is the `#else` reading only. In the `#if` reading there is no Q and P
/// runs on to the final `end`, so a statement after the `#endif` belongs to P there.
/// Here it is attributed to Q alone: P has no edge to it (a missed edge, not a wrong
/// one).
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
    if let Some(split_end) = cb.named_children().into_iter().find(|c| {
        matches!(
            c.kind(),
            RawKind::PreprocSplitCodeBlockEnd
                | RawKind::PreprocSplitElseBeginOverEndif
                | RawKind::PreprocSplitBlockEndInElse
                | RawKind::PreprocSplitBlockCloseAfterEndif
        )
    }) {
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
        // Union-read every branch the build context leaves live, each under
        // its own branch's symbols (`BUILD_CONTEXT`).
        for (syms, live, children) in preproc_branches(node, source, &build_context()) {
            if live {
                with_build_context(&syms, ir, |ir| {
                    for c in children {
                        lower_block_child(c, ir, issues, source, items, depth);
                    }
                });
            }
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
    // A bare identifier in statement position is not a call. Since grammar B7b-3
    // (`main` 4d75846c) every bare name in statement position — `Bar;`, and `Bar`
    // before `end`/`else`/`until` — is a `call_statement` (lowered in `lower_stmt`), so
    // one reaching here is ERROR-recovery debris. Limit: in a file that already has
    // an ERROR, a name the parser recovers in statement position may now be a
    // `call_statement`, and is lowered as a call like any other. So is a half-typed
    // name before `end` (`begin Fo end`) and a misread or recovered `begin`/`end`
    // (grammar item 43): only `else` is skipped below, deliberately (pinned by
    // `bare_name_before_end_lowers_as_call`).
    if matches!(node.kind(), RawKind::Identifier | RawKind::QuotedIdentifier) {
        return;
    }
    // A `call_statement` whose subtree carries a parse error (e.g. tree-sitter
    // synthesized a MISSING name) is not a confident call either — skip it.
    if node.kind() == RawKind::CallStatement && node.has_error() {
        return;
    }
    // B7b-3: a split terminator (`if C then Bar #if X ; #endif else Baz;`, valid AL)
    // leaves the `else` as `(call_statement function: (identifier))` (grammar
    // deferred item 43). `else` is reserved, never a callee; `Baz` follows as its own
    // statement and keeps its call.
    if node.kind() == RawKind::CallStatement
        && let Some(f) = node.field(FieldName::Function)
        && f.kind() == RawKind::Identifier
        && f.text(source).eq_ignore_ascii_case("else")
    {
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
    // A parenless no-arg call statement (`Initialize;`, or `Initialize` before `end`) —
    // the grammar wraps a bare name in statement position in a `call_statement`; since
    // B7b-3 the `;` is the host's, not the call's (see `lower_block_child` for the
    // skipped forms). Lower its `function` to a Call with empty args, ANCHORED
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
            // The `operator` field is an `assignment_operator` node: `:=`, or a
            // compound `+=` / `-=` / `*=` / `/=` that also reads the target.
            let compound = node
                .field(FieldName::Operator)
                .is_some_and(|op| op.text(source).trim() != ":=");
            StmtKind::Assignment {
                target,
                value,
                compound,
            }
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
/// The begin/end-fragmented shapes (`preproc_split_if_then_begin` and its
/// asymmetric/shared/begin-else siblings) expose only `condition`: reconstructed as an
/// `If` on it whose then-block is the union of the remaining statements (S9.0e; the
/// condition was lowered as a statement before, a phantom statement-call site).
///
/// Everything else — any
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
    // The begin/end-fragmented `if` shapes carry only a `condition` field; the rest
    // is a flat statement run. The condition is the `if`'s, never a statement.
    let cond_only_shape = matches!(
        node.kind(),
        RawKind::PreprocSplitIfThenBegin
            | RawKind::PreprocSplitIfBeginAsymmetric
            | RawKind::PreprocSplitIfThenBeginElseShared
            | RawKind::PreprocSplitIfBeginElse
    );
    if cond_only_shape && let Some(c) = node.field(FieldName::Condition) {
        let cond = lower_expr(c, ir, issues, source, depth + 1);
        let mut items = Vec::new();
        for ch in structural_children(node) {
            if is_preproc_scaffold(ch.kind()) || ch.id() == c.id() {
                continue;
            }
            lower_block_child(ch, ir, issues, source, &mut items, depth);
        }
        // Union read: every arm's statements sit in the then-block, so each call
        // keeps its site even though the arm structure is not preserved.
        let then_block = ir.add_block(Block {
            items,
            origin: origin.clone(),
        });
        return StmtKind::If {
            cond,
            then_block,
            else_block: None,
        };
    }
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
    collect_case_branches_in(
        node.named_children(),
        ir,
        issues,
        source,
        branches,
        else_block,
        depth,
    );
}

/// [`collect_case_branches`] over a list of children (one `#if` arm's, or a
/// whole `case_body`'s).
fn collect_case_branches_in(
    children: Vec<RawNode>,
    ir: &mut Ir,
    issues: &mut Vec<SyntaxIssue>,
    source: &str,
    branches: &mut Vec<CaseBranch>,
    else_block: &mut Option<BlockId>,
    depth: u32,
) {
    // Depth budget (as in `lower_stmt`): nested `#if` case groups recurse here once
    // per level, so pathological nesting must stop instead of overflowing the stack.
    if depth > MAX_LOWER_DEPTH {
        if let Some(first) = children.first() {
            issues.push(SyntaxIssue {
                message: format!(
                    "case-group nesting exceeds the {MAX_LOWER_DEPTH}-level lowering depth \
                     budget at `{}` — dropping the deeper branches rather than recursing \
                     further (pathological input, not real AL source)",
                    first.kind_str()
                ),
                origin: origin_of(*first),
            });
        }
        return;
    }
    for child in children {
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
            // union-read, each arm under its own branch symbols (`preproc_branches`; an
            // arm the build context decides false is left out, as for statements).
            // Previously unmatched here (silently skipped, no trace at all — worse than
            // an empty branch).
            RawKind::PreprocConditionalCase => {
                for (syms, live, arm) in preproc_branches(child, source, &build_context()) {
                    if live {
                        with_build_context(&syms, ir, |ir| {
                            collect_case_branches_in(
                                arm,
                                ir,
                                issues,
                                source,
                                branches,
                                else_block,
                                depth + 1,
                            );
                        });
                    }
                }
            }
            // "#if adds complete branches + provides a header for the next shared branch":
            // any direct `case_branch` children are complete extra branches (additive,
            // real pattern+body of their own, each under its arm's symbols); the
            // wrapper's OWN flattened `pattern` fields (unioned across every
            // `#if`/`#elif`/`#else` arm's header-only pattern, plus the shared trailing
            // pattern) plus its single shared `body` field make ONE more branch, compiled
            // in every build. Previously unmatched here (same silent-skip as above).
            RawKind::PreprocSplitCaseExtended => {
                // An arm's items are a case group's (B7b-3): branches, nested groups
                // (which may hold branches) and `;` separators, read like a group's arm.
                // So a `case_else_branch` reached here (in a nested group) becomes the
                // case's else, first-match-wins, as in a plain group; before B7b-3's
                // full read it was dropped with its calls.
                for (syms, live, arm) in preproc_branches(child, source, &build_context()) {
                    if live {
                        with_build_context(&syms, ir, |ir| {
                            collect_case_branches_in(
                                arm,
                                ir,
                                issues,
                                source,
                                branches,
                                else_block,
                                depth + 1,
                            );
                        });
                    }
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
            // `else` / `else begin` alternating per `#if` arm as the case's else part
            // (grammar 2026-10, `case_body` admits `preproc_split_open_statement`;
            // MfgCarryOutAction.Codeunit.al, BC 29). Its arms open with `else`, so it
            // is the case's else: lowered by the generic split-statement recovery. If
            // an else was already found, it is still lowered — as a pattern-less
            // branch — rather than dropped with its calls.
            RawKind::PreprocSplitOpenStatement => {
                let body = lower_branch(child, ir, issues, source, depth);
                if else_block.is_none() {
                    *else_block = Some(body);
                } else {
                    branches.push(CaseBranch {
                        patterns: Vec::new(),
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
            let args = node
                .field(FieldName::Arguments)
                .map(|al| lower_arguments(al, ir, issues, source, depth))
                .unwrap_or_default();
            ExprKind::Call { function, args }
        }
        RawKind::SubscriptExpression => ExprKind::Index {
            base: lower_opt_field(node, FieldName::Object, ir, issues, source, depth),
            index: lower_opt_field(node, FieldName::Index, ir, issues, source, depth),
        },
        // H-8: a leading comment (`(/* c */ Expr)`) must never be mistaken for the
        // real inner expression — see `structural_children`'s doc.
        RawKind::ParenthesizedExpression => match expr_children(node).into_iter().next() {
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
        RawKind::TernaryExpression => ExprKind::Ternary {
            cond: lower_opt_field(node, FieldName::Condition, ir, issues, source, depth),
            then_value: lower_opt_field(node, FieldName::ThenValue, ir, issues, source, depth),
            else_value: lower_opt_field(node, FieldName::ElseValue, ir, issues, source, depth),
        },
        RawKind::IsExpression | RawKind::AsExpression => ExprKind::TypeOp {
            op: if node.kind() == RawKind::IsExpression {
                TypeOp::Is
            } else {
                TypeOp::As
            },
            value: lower_opt_field(node, FieldName::Left, ir, issues, source, depth),
            ty: match node.field(FieldName::Right) {
                Some(t) => t.text(source).to_string(),
                None => {
                    issues.push(SyntaxIssue {
                        message: format!("missing `Right` on `{}`", node.kind_str()),
                        origin: origin_of(node),
                    });
                    String::new()
                }
            },
        },
        RawKind::ListLiteral => ExprKind::List(
            expr_children(node)
                .into_iter()
                .map(|c| lower_expr(c, ir, issues, source, depth + 1))
                .collect(),
        ),
        RawKind::DatabaseReference => ExprKind::DatabaseReference(node.text(source).to_string()),
        RawKind::Boolean => ExprKind::Literal(Literal::Bool(
            node.text(source).eq_ignore_ascii_case("true"),
        )),
        RawKind::Integer => ExprKind::Literal(Literal::Int(node.text(source).to_string())),
        RawKind::Decimal => ExprKind::Literal(Literal::Decimal(node.text(source).to_string())),
        RawKind::StringLiteral | RawKind::VerbatimString => {
            ExprKind::Literal(Literal::Text(node.text(source).to_string()))
        }
        RawKind::PreprocConditionalOperand => {
            return lower_operand_group(node, ir, issues, source, depth);
        }
        _ => {
            // Unmodelled expression container: lower its non-trivia children so
            // nested calls/members are still captured in the arena (completeness).
            // The node itself is Unknown, and nothing links those children: a
            // container with calls inside needs a real variant (S9.0c gave one to
            // ternary, is/as and the list literal).
            for c in structural_children(node) {
                lower_expr(c, ir, issues, source, depth + 1);
            }
            ExprKind::Unknown
        }
    };
    ir.add_expr(Expr { kind, origin })
}

/// `preproc_conditional_operand` (grammar `main` 960ed312, B7b-2): an `#if` group in
/// place of a whole operand (`1 + #if X Foo() #else Bar() #endif`, or a receiver,
/// `#if X Foo #else Bar #endif (1)`), one `operand` per arm. An operand is one value,
/// so this reads ONE arm, the first a build can take ([`conditional_list_arm`]), as
/// the positional lists do: a union would be an expression no build has. The group
/// becomes that arm's expression. The other arms' calls are not lowered, and a
/// `SyntaxIssue` records that. Without this arm the group was an `Unknown`, which
/// links no child, so every call in every arm vanished.
fn lower_operand_group(
    node: RawNode,
    ir: &mut Ir,
    issues: &mut Vec<SyntaxIssue>,
    source: &str,
    depth: u32,
) -> ExprId {
    issues.push(SyntaxIssue {
        message: "`#if` group as an operand — only the first arm a build can take is \
                  lowered (the other arms' calls are not modelled)"
            .to_string(),
        origin: origin_of(node),
    });
    let mut arm = Vec::new();
    conditional_list_arm(node, source, &mut Run::default(), &mut arm);
    match arm.into_iter().find(|n| !is_expr_continuation(n.kind())) {
        Some(operand) => lower_expr(operand, ir, issues, source, depth + 1),
        None => ir.add_expr(Expr {
            kind: ExprKind::Unknown,
            origin: origin_of(node),
        }),
    }
}

/// An `#if` tail or prefix that continues an expression across a directive (grammar
/// B7b-2): a sibling of a value, never a value itself.
fn is_expr_continuation(k: RawKind) -> bool {
    matches!(
        k,
        RawKind::PreprocConditionalExpressionTail | RawKind::PreprocOperandPrefix
    )
}

/// [`structural_children`] without the `#if` tails and prefixes (grammar 960ed312
/// lets them stand beside the value of a parenthesis, a list element or an argument),
/// so they never take a value's slot. Their own operands are not lowered.
fn expr_children(node: RawNode) -> Vec<RawNode> {
    structural_children(node)
        .into_iter()
        .filter(|c| !is_expr_continuation(c.kind()))
        .collect()
}

/// An `argument_list`'s arguments, in document order.
///
/// - H-8: a mid-argument comment (`P(a, /* c */ b)`) must never become a phantom
///   `Unknown` argument — see `structural_children`'s doc.
/// - `preproc_conditional_arguments` (grammar `main`, 2026-10; an ERROR before): a
///   `#if` at an argument separator, `P(a, #if X b, #endif c)`. Its arms' arguments
///   are flat children between the directive nodes. They are UNION-read into the
///   list, the same policy `preproc_split_call_statement` uses for a split argument
///   list, so every call nested in any arm stays a reachable call site. The arity is
///   then the union's, which may match no single build; a `SyntaxIssue` records that.
/// - `preproc_conditional_expression_tail`: `_argument_expression` lets an argument
///   continue across a `#if` (`P(1 #if X + 2 #endif)`), and the tail is a SIBLING of
///   the argument it continues. It is not an argument, so it is not counted; lowering
///   it would only add an `Unknown` argument, which carries no reachable call either.
///   A `preproc_operand_prefix` before an argument (grammar 960ed312,
///   `P(#if X 1 + #endif 2)`) is skipped the same way: it counted as an extra
///   argument, so the call missed its target.
fn lower_arguments(
    al: RawNode,
    ir: &mut Ir,
    issues: &mut Vec<SyntaxIssue>,
    source: &str,
    depth: u32,
) -> Vec<ExprId> {
    let mut args = Vec::new();
    for c in expr_children(al) {
        match c.kind() {
            // A directive line inside the list (`#pragma warning disable` between
            // two arguments) is no argument (S9.0e: it inflated the arity, so
            // the call missed its 6-parameter target). The statement lowerer
            // skips the same kinds.
            RawKind::Pragma | RawKind::PreprocRegion | RawKind::PreprocEndregion => {}
            RawKind::PreprocConditionalArguments => {
                issues.push(SyntaxIssue {
                    message: "`#if` inside an argument list — every arm's arguments are \
                              union-read (the per-build arity is not modelled)"
                        .to_string(),
                    origin: origin_of(c),
                });
                for a in expr_children(c) {
                    if is_preproc_scaffold(a.kind()) {
                        continue;
                    }
                    args.push(lower_expr(a, ir, issues, source, depth + 1));
                }
            }
            _ => args.push(lower_expr(c, ir, issues, source, depth + 1)),
        }
    }
    args
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

    /// S9.0e: a header split across `#if` arms takes its first header's kind
    /// (System Application's `enum 8889 "Email Connector"` was lowered as a
    /// Codeunit).
    #[test]
    fn split_declaration_takes_its_headers_kind() {
        let src = "#if not CLEAN26
enum 8889 \"Email Connector\" implements \"A\", \"B\"
#else
enum 8889 \"Email Connector\" implements \"A\"
#endif
{
    Extensible = true;
    value(0; X) { }
}
                   #if not CLEAN27
table 50000 \"T One\"
#else
table 50000 \"T Two\"
#endif
{
    fields { field(1; A; Integer) { } }
}
";
        let af = parse(src);
        let kinds: Vec<_> = af
            .objects
            .iter()
            .map(|o| (o.kind, o.id, o.name.clone()))
            .collect();
        assert_eq!(
            kinds,
            vec![
                (
                    crate::ir::ObjectKind::Enum,
                    Some(8889),
                    "Email Connector".to_string()
                ),
                (
                    crate::ir::ObjectKind::Table,
                    Some(50000),
                    "T One".to_string()
                ),
            ]
        );
    }

    /// S9.0e: an XmlPort `tableelement(Name; Table)` is collected like a report
    /// dataitem, at any nesting depth.
    #[test]
    fn xmlport_table_elements_are_collected() {
        let src = "xmlport 50002 X
{
    schema
    {
        textelement(Root)
        {
            tableelement(Elem; T)
            {
                tableelement(\"Inner Line\"; \"Sales Line\") { }
            }
            textelement(Other) { }
        }
    }
}
";
        let af = parse(src);
        assert_eq!(
            af.objects[0].dataitems,
            vec![
                ("Elem".to_string(), "T".to_string()),
                ("Inner Line".to_string(), "Sales Line".to_string())
            ]
        );
    }

    /// S9.0e: `if C then begin` split across `#if` (`preproc_split_if_then_begin`,
    /// real code in Base Application's SalesPost) keeps `C` as the `if` condition. It
    /// used to be lowered through the statement dispatcher, a phantom statement call.
    #[test]
    fn split_if_then_begin_keeps_its_condition() {
        let src = "codeunit 50000 T
{
    procedure P()
    begin
        if not IsHandled then
#if not CLEAN27
            if SalesLine.Quantity <> 0 then begin
                Foo();
#endif
                Bar();
#if not CLEAN27
            end;
#endif
    end;
}
";
        let af = parse(src);
        let member_is_quantity = |id: crate::ir::ExprId| matches!(&af.ir.expr(id).kind, ExprKind::Member { member, .. } if member == "Quantity");
        let mut split_if = 0;
        for st in af.ir.iter_stmts() {
            match &st.kind {
                StmtKind::Call(e) => {
                    if let ExprKind::Call { function, .. } = &af.ir.expr(*e).kind {
                        assert!(
                            !member_is_quantity(*function),
                            "phantom call on the condition"
                        );
                    }
                }
                StmtKind::If {
                    cond, then_block, ..
                } => {
                    if let ExprKind::Binary { lhs, .. } = &af.ir.expr(*cond).kind
                        && member_is_quantity(*lhs)
                    {
                        split_if += 1;
                        assert_eq!(af.ir.block(*then_block).items.len(), 2, "Foo and Bar");
                    }
                }
                _ => {}
            }
        }
        assert_eq!(split_if, 1);
    }

    /// S9.0c: the ternary, `is`/`as` and the list literal are real expressions whose
    /// sub-expressions are linked, not an opaque `Unknown` with unlinked children
    /// (which lost every call inside them).
    #[test]
    fn ternary_typeop_and_list_link_their_operands() {
        let src = "codeunit 50000 T
{
    procedure P()
    var
        A: Integer;
        O: Codeunit Y;
    begin
        A := IsOn() ? GetA() : GetB(1);
        if A in [GetA(), /* c */ 2] then;
        if GetI() is \"Y\" then;
        O := GetI() as Y;
    end;
}
";
        let af = parse(src);
        let callee = |id: crate::ir::ExprId| match &af.ir.expr(id).kind {
            ExprKind::Call { function, .. } => match &af.ir.expr(*function).kind {
                ExprKind::Identifier(n) => n.clone(),
                _ => panic!("callee is not an identifier"),
            },
            _ => panic!("not a call: {}", af.ir.expr(id).origin.kind_text),
        };
        let mut seen = Vec::new();
        for e in af.ir.iter_exprs() {
            match &e.kind {
                ExprKind::Ternary {
                    cond,
                    then_value,
                    else_value,
                } => seen.push(format!(
                    "ternary {} {} {}",
                    callee(*cond),
                    callee(*then_value),
                    callee(*else_value)
                )),
                ExprKind::TypeOp { op, value, ty } => {
                    seen.push(format!("{op:?} {} {ty}", callee(*value)));
                }
                ExprKind::List(items) => {
                    let first = callee(items[0]);
                    let second = matches!(
                        &af.ir.expr(items[1]).kind,
                        ExprKind::Literal(Literal::Int(n)) if n == "2"
                    );
                    seen.push(format!("list {first} {second} {}", items.len()));
                }
                ExprKind::Unknown => panic!("Unknown at {:?}", e.origin.kind_text),
                _ => {}
            }
        }
        seen.sort();
        assert_eq!(
            seen,
            vec![
                "As GetI Y".to_string(),
                "Is GetI \"Y\"".to_string(),
                "list GetA true 2".to_string(),
                "ternary IsOn GetA GetB".to_string(),
            ]
        );
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

    /// Issue #56: pins the corrected claim. A zero-width member name (`modify()`,
    /// `action()`) is tree-sitter recovery, so the file is `Recovered`; an empty
    /// QUOTED name (`action("")`) is the shape that genuinely parses `Clean`.
    #[test]
    fn zero_width_member_names_are_recovered_but_empty_quoted_name_is_clean() {
        use crate::ir::ParseStatus;
        let modify = r#"
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
        let action = r#"
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
        let quoted = action.replace("action()", "action(\"\")");
        assert_ne!(quoted, action, "the replace must apply");
        assert_eq!(parse(modify).parse_status, ParseStatus::Recovered);
        assert_eq!(parse(action).parse_status, ParseStatus::Recovered);
        assert_eq!(parse(&quoted).parse_status, ParseStatus::Clean);
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
    /// `action()` with no name is tree-sitter error recovery (issue #56): a hidden
    /// MISSING keyword under the `identifier` alias gives `name: (identifier
    /// [r,c]-[r,c])`, zero width. `has_error()` is true and the file is `Recovered`
    /// (see `zero_width_member_names_are_recovered_but_empty_quoted_name_is_clean`).
    /// The degrade to `None` has to happen here regardless: `Some("")` would take
    /// the discriminated arm of `to_stable_routine_id_from_parts` and mint a
    /// meaningless id, and the routine itself is not marked `parse_incomplete`.
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

    /// `ObjectDecl::run_objects` (#63): every action's `RunObject`, nested in
    /// groups and `#if` arms, and none from a procedure body or another property.
    #[test]
    fn run_object_properties_collected_from_actions() {
        let src = r#"
page 50203 "Run Objects"
{
    PageType = List;
    CardPageId = "Some Card";
    actions
    {
        area(Navigation)
        {
            group(G)
            {
                action(OpenCard)
                {
                    RunObject = Page "Customer Card";
                }
            }
#if FOO
            action(RunCu)
            {
                RunObject = Codeunit 50100;
            }
#endif
        }
    }
    procedure P()
    begin
    end;
}
"#;
        let af = parse(src);
        assert_eq!(
            af.objects[0].run_objects,
            vec![
                "Page \"Customer Card\"".to_string(),
                "Codeunit 50100".to_string()
            ]
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

    /// A split header lowers to one routine per arm, each with its own parameters,
    /// modifier, attributes and (for a preamble) `var` section, and the shared body.
    /// Base App's `CalcRoutingLineCosts` has 6 parameters under `#if not CLEAN27` and
    /// 5 under `#else`; reading only the first arm made the `#else` call an
    /// `arityMismatch`.
    #[test]
    fn preproc_split_header_yields_one_routine_per_arm() {
        let src = r#"
codeunit 50102 T
{
#if not CLEAN27
    [Obsolete('x', '27.0')]
    local procedure Foo(A: Integer; B: Integer) R: Integer
    var
        X: Integer;
#elif SOMETHING
    internal procedure Foo(A: Integer; B: Integer; C: Integer)
#else
    procedure Foo(A: Integer)
    var
        Y: Integer;
#endif
    begin
        DoWork(A);
#if not CLEAN27
        OnlyFirst(B);
#else
        OnlyLater(A);
#endif
#if SOMETHING
        Undecided(A);
#endif
    end;
}
"#;
        let af = parse(src);
        let foos: Vec<_> = af.objects[0]
            .routines
            .iter()
            .filter(|r| r.name == "Foo")
            .collect();
        let shape: Vec<_> = foos
            .iter()
            .map(|r| {
                (
                    r.params.len(),
                    r.access_modifier.clone(),
                    r.return_name.clone(),
                    r.attributes.clone(),
                    r.locals.iter().map(|l| l.name.clone()).collect::<Vec<_>>(),
                )
            })
            .collect();
        assert_eq!(
            shape,
            vec![
                (
                    2,
                    Some("local".to_string()),
                    Some("R".to_string()),
                    vec!["obsolete".to_string()],
                    vec!["X".to_string()]
                ),
                (3, Some("internal".to_string()), None, vec![], vec![]),
                (1, None, None, vec![], vec!["Y".to_string()]),
            ]
        );
        // Each arm's body keeps only the body branches its own condition allows:
        // arm 1 has `CLEAN27` undefined; arms 2 and 3 have it defined; only arm 2
        // decides `SOMETHING` (true), arm 3 decides it false.
        let calls: Vec<Vec<bool>> = foos
            .iter()
            .map(|r| {
                let body = r.body.expect("every arm carries the shared body");
                ["DoWork", "OnlyFirst", "OnlyLater", "Undecided"]
                    .iter()
                    .map(|n| call_reachable(&af, body, n))
                    .collect()
            })
            .collect();
        assert_eq!(
            calls,
            vec![
                vec![true, true, false, true],
                vec![true, false, true, true],
                vec![true, false, true, false],
            ]
        );
    }

    /// A query's plain columns carry their innermost dataitem's table and their
    /// source field; a `Method` column and a filter are no plain column.
    #[test]
    fn query_plain_columns_are_lowered_with_their_dataitem_table() {
        let src = r#"
query 50000 Q
{
    elements
    {
        dataitem(D; "Detailed Cust. Ledg. Entry")
        {
            column(EntryType; "Entry Type") { }
            filter(PostingDate; "Posting Date") { }
            column(SumAmt; "Amount (LCY)") { Method = Sum; }
            column(Cnt) { Method = Count; }
            dataitem(Customer; Customer)
            {
                column(CustNo; "No.") { }
            }
        }
    }
}
"#;
        let af = parse(src);
        let cols: Vec<(&str, &str, &str)> = af.objects[0]
            .query_columns
            .iter()
            .map(|(a, b, c)| (a.as_str(), b.as_str(), c.as_str()))
            .collect();
        assert_eq!(
            cols,
            vec![
                ("EntryType", "Detailed Cust. Ledg. Entry", "Entry Type"),
                ("CustNo", "Customer", "No."),
            ]
        );
    }

    /// An XmlPort's text nodes are xmlport globals: `Text`, or `BigText` with
    /// `TextType = BigText`. A `fieldelement` is no variable.
    #[test]
    fn xmlport_text_nodes_are_globals() {
        let src = r#"
xmlport 50000 X
{
    schema
    {
        textelement(Root)
        {
            textattribute(Big)
            {
                TextType = BigText;
            }
            tableelement(Item; Item)
            {
                fieldelement(No; Item."No.") { }
            }
        }
    }
}
"#;
        let af = parse(src);
        let globals: Vec<(&str, Option<&str>)> = af.objects[0]
            .globals
            .iter()
            .map(|g| (g.name.as_str(), g.ty.as_deref()))
            .collect();
        assert_eq!(
            globals,
            vec![("Root", Some("Text")), ("Big", Some("BigText"))]
        );
    }

    /// The build context is recorded where it applies: on each routine (its
    /// object-level `#if` branch and split-header arm) and on each expression
    /// (its routine's context plus the body branches around it), including the
    /// arms of a `#if`-split complete body. A `#else` decides the negation of
    /// every earlier condition; `A and B` true decides both.
    #[test]
    fn build_contexts_are_recorded_on_routines_and_expressions() {
        let src = r#"
codeunit 50104 T
{
#if A and B
    procedure Obj()
    begin
        InObj();
    end;
#else
    procedure Obj()
    begin
        InObjElse();
    end;
#endif

#if not CLEAN27
    procedure Split(X: Integer)
#else
    procedure Split()
#endif
    begin
#if FEATURE
        InFeature();
#endif
        Plain();
    end;

    procedure Whole()
#if Y
    begin
        InY();
    end;
#else
    begin
        InNotY();
    end;
#endif

    procedure Cased(X: Integer)
    begin
        case X of
#if DOSMTP
            1:
                InSmtp();
            2:
#else
            2, 1:
#endif
                Shared();
        end;
    end;

    procedure Arms(X: Integer)
    begin
        case X of
#if DOSMTP
            1:
                InSmtp();
#else
            1:
                NotSmtp();
#endif
            2:
                Shared();
        end;
    end;
}
"#;
        fn call_in(af: &crate::ir::AlFile, b: crate::ir::BlockId, name: &str) -> crate::ir::ExprId {
            find_call(af, b, name).unwrap_or_else(|| panic!("no direct call to {name}"))
        }
        fn find_call(
            af: &crate::ir::AlFile,
            b: crate::ir::BlockId,
            name: &str,
        ) -> Option<crate::ir::ExprId> {
            af.ir.block(b).items.iter().find_map(|item| match item {
                crate::ir::BlockItem::Stmt(sid) => match &af.ir.stmt(*sid).kind {
                    StmtKind::Call(eid) => matches!(
                        &af.ir.expr(match &af.ir.expr(*eid).kind {
                            ExprKind::Call { function, .. } => *function,
                            _ => *eid,
                        })
                        .kind,
                        ExprKind::Identifier(n) if n == name
                    )
                    .then_some(*eid),
                    _ => None,
                },
                _ => None,
            })
        }
        let s = |v: &[(&str, bool)]| -> Vec<(String, bool)> {
            v.iter().map(|(n, b)| (n.to_string(), *b)).collect()
        };
        let af = parse(src);
        let routines = &af.objects[0].routines;
        let ctx_of = |i: usize| routines[i].preproc_context.clone();
        assert_eq!(routines.len(), 7);
        assert_eq!(ctx_of(0), s(&[("A", true), ("B", true)]));
        assert_eq!(ctx_of(1), Vec::new(), "`A and B` false decides neither");
        assert_eq!(ctx_of(2), s(&[("CLEAN27", false)]));
        assert_eq!(ctx_of(3), s(&[("CLEAN27", true)]));
        assert_eq!(ctx_of(4), Vec::new());
        let at = |r: usize, name: &str| {
            af.ir
                .preproc_context(call_in(&af, routines[r].body.expect("body"), name))
                .to_vec()
        };
        assert_eq!(at(0, "InObj"), s(&[("A", true), ("B", true)]));
        assert_eq!(at(1, "InObjElse"), Vec::new());
        assert_eq!(
            at(2, "InFeature"),
            s(&[("CLEAN27", false), ("FEATURE", true)])
        );
        assert_eq!(at(3, "Plain"), s(&[("CLEAN27", true)]));
        assert_eq!(at(4, "InY"), s(&[("Y", true)]));
        assert_eq!(at(4, "InNotY"), s(&[("Y", false)]));
        // A `case` split across `#if` arms: an arm's own branch carries the arm's
        // symbols, the shared branch none (CDO `CDOEMail.Codeunit.al:241`).
        let case_branches = |r: usize, name: &str| -> Vec<(String, bool)> {
            let StmtKind::Case { branches, .. } = &af
                .ir
                .stmt(
                    match af.ir.block(routines[r].body.expect("body")).items[0] {
                        crate::ir::BlockItem::Stmt(s) => s,
                        _ => panic!("statement"),
                    },
                )
                .kind
            else {
                panic!("case");
            };
            branches
                .iter()
                .find_map(|b| find_call(&af, b.body, name))
                .map(|e| af.ir.preproc_context(e).to_vec())
                .unwrap_or_else(|| panic!("no branch calls {name}"))
        };
        assert_eq!(case_branches(5, "InSmtp"), s(&[("DOSMTP", true)]));
        assert_eq!(case_branches(5, "Shared"), Vec::new());
        // Whole branches under `#if` arms (`preproc_conditional_case`): each arm's
        // branches carry that arm's symbols.
        assert_eq!(case_branches(6, "InSmtp"), s(&[("DOSMTP", true)]));
        assert_eq!(case_branches(6, "NotSmtp"), s(&[("DOSMTP", false)]));
        assert_eq!(case_branches(6, "Shared"), Vec::new());
    }

    /// Outside a split header's arm nothing is decided: every branch union-reads.
    #[test]
    fn preproc_branches_outside_a_split_arm_union_read() {
        let src = r#"
codeunit 50103 T
{
    procedure Foo()
    begin
#if not CLEAN27
        OnlyFirst();
#else
        OnlyLater();
#endif
    end;
}
"#;
        let af = parse(src);
        let body = af.objects[0].routines[0].body.expect("body");
        assert!(call_reachable(&af, body, "OnlyFirst"));
        assert!(call_reachable(&af, body, "OnlyLater"));
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

    /// Grammar B7b-3 (`main` 4d75846c): a `preproc_split_case_extended` arm may hold a
    /// nested `preproc_conditional_case`, and that group may hold whole branches
    /// (VERIFIED with `tree-sitter parse`). The extended arm read only its direct
    /// `case_branch` children, so `5: DoFive()` was dropped with its call. The `#else`
    /// arm holds a branch and a nested group of `;` `empty_statement`s (skipped).
    #[test]
    fn split_case_extended_nested_group_branch_is_kept() {
        let src = r#"
codeunit 50105 T
{
    procedure P(I: Integer)
    begin
        case I of
#if X
            1: DoOne();
#if Y
            5: DoFive();
#endif
            2:
#else
            6: DoSix()
#if Y
            ;
#else
            ;
#endif
            2:
#endif
                DoTwo();
        end;
    end;
}
"#;
        let af = parse(src);
        let (branches, else_block) = first_case(&af);
        assert_eq!(branches.len(), 4, "1, 5, 6 and the shared 2");
        assert!(else_block.is_none());
        for name in ["DoOne", "DoFive", "DoSix", "DoTwo"] {
            assert!(
                branches.iter().any(|b| call_reachable(&af, b.body, name)),
                "the branch calling {name} must be lowered"
            );
        }
    }

    /// Grammar B7b-3: a `case_else_branch` in a nested group inside a
    /// `preproc_split_case_extended` arm parses clean (VERIFIED with `tree-sitter
    /// parse`). The arm is read like a case group's, so it becomes the case's else
    /// (it was dropped with its call when the arm read only `case_branch`).
    #[test]
    fn split_case_extended_nested_group_else_is_the_case_else() {
        let src = r#"
codeunit 50107 T
{
    procedure P(I: Integer)
    begin
        case I of
#if X
            1: DoOne();
#if Y
            else
                DoElse();
#endif
            2:
#else
            2:
#endif
                DoTwo();
        end;
    end;
}
"#;
        let af = parse(src);
        let (_branches, else_block) = first_case(&af);
        let else_block = else_block.expect("the nested-group else is the case's else");
        assert!(call_reachable(&af, else_block, "DoElse"));
    }

    /// Grammar B7b-3 (`main` 4d75846c): every bare name in statement position is a
    /// `call_statement`, and a split terminator can leave the `else` of a valid
    /// `if C then Bar #if X ; #endif else Baz;` as `(call_statement function:
    /// (identifier))` (grammar deferred item 43; VERIFIED with `tree-sitter parse`).
    /// `else` is a reserved word, never a callee: no call to it may be lowered, while
    /// `Bar` (a parenless call) and `Baz` keep theirs.
    #[test]
    fn split_terminator_else_is_not_a_call() {
        let src = r#"
codeunit 50106 T
{
    procedure P(C: Boolean)
    begin
        if C then Bar
#if X
        ;
#endif
        else Baz;
    end;
}
"#;
        let callees = stmt_callees(&parse(src));
        assert!(callees.contains(&"bar".to_string()), "{callees:?}");
        assert!(callees.contains(&"baz".to_string()), "{callees:?}");
        assert!(!callees.contains(&"else".to_string()), "{callees:?}");
    }

    /// Grammar B7b-3: a bare name before `end` is a `call_statement`, so a half-typed
    /// name (or a misread `begin`/`end`, grammar item 43) lowers as a call. This is
    /// deliberate: only `else` is skipped (see `lower_block_child`).
    #[test]
    fn bare_name_before_end_lowers_as_call() {
        let src = "codeunit 50108 T\n{\n    procedure P()\n    begin\n        Fo\n    end;\n}\n";
        let callees = stmt_callees(&parse(src));
        assert_eq!(callees, vec!["fo".to_string()]);
    }

    /// `(callee, arity)` of every call with a plain-identifier callee that is LINKED
    /// under an assignment value, a call statement or an `if` condition (an arena
    /// expression no parent links is not a call site, so it is not listed).
    fn linked_calls(af: &crate::ir::AlFile) -> Vec<(String, usize)> {
        fn walk(af: &crate::ir::AlFile, id: crate::ir::ExprId, out: &mut Vec<(String, usize)>) {
            let k = &af.ir.expr(id).kind;
            if let ExprKind::Call { function, args } = k
                && let ExprKind::Identifier(n) = &af.ir.expr(*function).kind
            {
                out.push((n.to_ascii_lowercase(), args.len()));
            }
            for c in k.children() {
                walk(af, c, out);
            }
        }
        let mut out = Vec::new();
        for s in af.ir.iter_stmts() {
            match &s.kind {
                StmtKind::Assignment { value, .. } => walk(af, *value, &mut out),
                StmtKind::Call(e) => walk(af, *e, &mut out),
                StmtKind::If { cond, .. } => walk(af, *cond, &mut out),
                _ => {}
            }
        }
        out
    }

    /// Grammar B7b-2 (`main` 960ed312): an `#if` group in place of a whole operand is
    /// a `preproc_conditional_operand`, one `operand` field per arm, and as a callee
    /// it is the call's `function` (VERIFIED with a real parse's `to_sexp`). The
    /// first arm is the operand; the `#else` arm's call is not lowered (the stated
    /// one-arm limit). It was an `Unknown`, which links no child, so both calls were
    /// lost.
    #[test]
    fn operand_group_lowers_its_first_arm() {
        let src = r#"
codeunit 50109 T
{
    procedure P()
    begin
        I := 1 +
#if X
            Foo()
#else
            Bar()
#endif
            ;
        I :=
#if X
            Baz
#else
            Qux
#endif
            (1);
    end;
}
"#;
        let calls = linked_calls(&parse(src));
        assert_eq!(
            calls,
            vec![("foo".to_string(), 0), ("baz".to_string(), 1)],
            "{calls:?}"
        );
    }

    /// Grammar B7b-2 (`main` 960ed312): a `preproc_operand_prefix` may stand beside
    /// an argument, a list element or a parenthesised value (VERIFIED with a real
    /// parse's `to_sexp`). It is no value: it inflated the arity, so `Baz` had 3
    /// arguments, and as a parenthesis's first child it hid `Quux()`.
    #[test]
    fn operand_prefix_takes_no_value_slot() {
        let src = r#"
codeunit 50110 T
{
    procedure P()
    begin
        Baz(
#if X
            1 +
#endif
            Qux(), 2);
        I := (
#if X
            1 +
#endif
            Quux());
        if I in [
#if X
            1 +
#endif
            Corge(), 2] then;
    end;
}
"#;
        let af = parse(src);
        let calls = linked_calls(&af);
        assert_eq!(
            calls,
            vec![
                ("baz".to_string(), 2),
                ("qux".to_string(), 0),
                ("quux".to_string(), 0),
                ("corge".to_string(), 0),
            ],
            "{calls:?}"
        );
        assert!(
            af.ir
                .iter_exprs()
                .any(|e| matches!(&e.kind, ExprKind::List(items) if items.len() == 2)),
            "the list literal has two elements"
        );
    }

    /// Lowercased names of every `StmtKind::Call` whose callee is a plain identifier.
    fn stmt_callees(af: &crate::ir::AlFile) -> Vec<String> {
        af.ir
            .iter_stmts()
            .filter_map(|s| match &s.kind {
                StmtKind::Call(e) => match &af.ir.expr(*e).kind {
                    ExprKind::Call { function, .. } => match &af.ir.expr(*function).kind {
                        ExprKind::Identifier(n) => Some(n.to_ascii_lowercase()),
                        _ => None,
                    },
                    _ => None,
                },
                _ => None,
            })
            .collect()
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

    /// S9.0e: a `#pragma` line between two arguments is no argument. The grammar
    /// keeps it inside `argument_list`; counting it inflated the arity (CDO:
    /// `CreateDefaultPaymentMeans(..)` — 6 args read as 8, an arity mismatch).
    #[test]
    fn pragma_inside_an_argument_list_is_no_argument() {
        let src = r#"
codeunit 50106 T
{
    procedure P()
    begin
        F(A,
#pragma warning disable AA0139
            B,
#pragma warning restore AA0139
            C);
    end;
}
"#;
        let af = parse(src);
        let arg_counts: Vec<usize> = af
            .ir
            .iter_exprs()
            .filter_map(|e| match &e.kind {
                ExprKind::Call { args, .. } => Some(args.len()),
                _ => None,
            })
            .collect();
        assert_eq!(arg_counts, vec![3]);
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

    // ---- grammar `main` 2026-10 shapes (each was an ERROR, or dropped, before) ----

    /// The arguments of the first call to `name`, in the arena.
    fn call_args(af: &crate::ir::AlFile, name: &str) -> Vec<crate::ir::ExprId> {
        af.ir
            .iter_exprs()
            .find_map(|e| match &e.kind {
                ExprKind::Call { function, args }
                    if matches!(&af.ir.expr(*function).kind, ExprKind::Identifier(n) if n == name) =>
                {
                    Some(args.clone())
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("{name} call must exist"))
    }

    fn routine_body(af: &crate::ir::AlFile, name: &str) -> crate::ir::BlockId {
        af.objects[0]
            .routines
            .iter()
            .find(|r| r.name == name)
            .unwrap_or_else(|| panic!("routine {name} must exist"))
            .body
            .expect("routine must have a body")
    }

    /// `preproc_conditional_arguments`: every arm's arguments are real arguments in
    /// some build, so they are union-read into the list (never one `Unknown` argument
    /// standing for the whole `#if`, which hid the call nested in it).
    #[test]
    fn split_argument_list_union_reads_every_arm() {
        let src = r#"
codeunit 50110 T
{
    procedure P()
    begin
        DoWork(1,
#if not C28
            2
#else
            Inner()
#endif
            );
    end;
}
"#;
        let af = parse(src);
        let args = call_args(&af, "DoWork");
        assert_eq!(args.len(), 3, "1, then both arms' arguments");
        assert!(matches!(
            af.ir.expr(args[1]).kind,
            ExprKind::Literal(Literal::Int(_))
        ));
        assert!(
            matches!(af.ir.expr(args[2]).kind, ExprKind::Call { .. }),
            "the #else arm's call is an argument, so the engine reaches it"
        );
    }

    /// A `preproc_conditional_expression_tail` beside an argument continues that
    /// argument (`DoWork(1 #if X + 2 #endif, 3)`); it is not a second argument.
    #[test]
    fn argument_expression_tail_is_not_an_argument() {
        let src = r#"
codeunit 50111 T
{
    procedure P()
    begin
        DoWork(1
#if X
            + 2
#endif
            , 3);
    end;
}
"#;
        let af = parse(src);
        assert_eq!(call_args(&af, "DoWork").len(), 2);
    }

    /// A procedure boundary inside an `#else`: P's block closes in
    /// `preproc_split_block_end_in_else`, Q's in `preproc_split_block_close_after_endif`.
    /// Both are siblings of the block's `body`, like `preproc_split_code_block_end`.
    #[test]
    fn split_procedure_boundary_closings_are_recovered() {
        let src = r#"
codeunit 50112 T
{
    procedure P()
    begin
        First();
#if not C28
        IfArm();
#else
        ElseArm();
    end;

    local procedure Q()
    begin
        InQ();
#endif
        AfterEndif();
    end;
}
"#;
        let af = parse(src);
        assert_eq!(af.parse_status, crate::ir::ParseStatus::Clean);
        let p = routine_body(&af, "P");
        for name in ["First", "IfArm", "ElseArm"] {
            assert!(
                call_reachable(&af, p, name),
                "{name} must be reachable in P"
            );
        }
        let q = routine_body(&af, "Q");
        for name in ["InQ", "AfterEndif"] {
            assert!(
                call_reachable(&af, q, name),
                "{name} must be reachable in Q"
            );
        }
    }

    /// `preproc_split_else_begin_over_endif` (in `code_block`'s closing choice since
    /// before the 2026-10 grammar): its statements were dropped.
    #[test]
    fn split_else_begin_over_endif_content_is_recovered() {
        let src = r#"
codeunit 50113 T
{
    procedure P(A: Boolean; B: Boolean)
    begin
        if A then begin
            First();
#if not C28
            InIf();
        end else begin
            InElse();
#endif
            Shared();
        end;
    end;
}
"#;
        let af = parse(src);
        assert_eq!(af.parse_status, crate::ir::ParseStatus::Clean);
        let p = routine_body(&af, "P");
        for name in ["First", "InIf", "InElse", "Shared"] {
            assert!(call_reachable(&af, p, name), "{name} must be reachable");
        }
    }

    /// `preproc_split_open_statement` inside a `case_body`: `else` / `else begin`
    /// alternating per arm is the case's else part. It fell to the `_` arm of
    /// `collect_case_branches` and was dropped with every call in it.
    #[test]
    fn case_body_split_open_statement_is_the_case_else() {
        let src = r#"
codeunit 50114 T
{
    procedure P(K: Integer; H: Boolean)
    begin
        case K of
            1:
                One()
#if C28
            else
#else
            else begin
                InElseBegin();
#endif
                Shared();
#if not C28
            end;
#endif
        end;
    end;
}
"#;
        let af = parse(src);
        assert_eq!(af.parse_status, crate::ir::ParseStatus::Clean);
        let (_, else_block) = first_case(&af);
        let else_block = else_block.expect("the split else is the case's else");
        for name in ["InElseBegin", "Shared"] {
            assert!(call_reachable(&af, else_block, name), "{name} in the else");
        }
    }

    /// `preproc_split_var_section_tail`: a `#if` arm continues the global var section
    /// and then starts procedures. Its declarations are globals.
    #[test]
    fn split_var_section_tail_declarations_are_globals() {
        let src = r#"
codeunit 50115 T
{
    var
        A: Integer;
#if not CLEAN25
        B: Integer;

    procedure P()
    begin
    end;
#endif

    procedure Q()
    begin
    end;
}
"#;
        let af = parse(src);
        assert_eq!(af.parse_status, crate::ir::ParseStatus::Clean);
        let obj = &af.objects[0];
        let globals: Vec<&str> = obj.globals.iter().map(|g| g.name.as_str()).collect();
        assert_eq!(globals, ["A", "B"]);
        let routines: Vec<&str> = obj.routines.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(routines, ["P", "Q"]);
    }

    /// Split table fields and keys: a header that differs per arm with one shared body
    /// (`preproc_split_table_field`, `preproc_split_key`) and a body opened inside the
    /// `#if` (`preproc_split_table_field_open`). None was read; the field or key was
    /// missing from the object. The FieldClass after the `#endif` is read too.
    #[test]
    fn split_table_fields_and_keys_are_collected() {
        let src = r#"
table 50116 T
{
    fields
    {
        field(1; A; Integer) { }
#if not CLOUD
        field(2; B; Text[2048])
#else
        field(2; B; Text[250])
#endif
        {
        }
#if not S31
        field(3; C; Integer) { }
        field(4; D; Decimal)
        {
            Caption = 'D';
#endif
            FieldClass = FlowField;
        }
    }
    keys
    {
        key(PK; A) { }
#if not C28
        key(K2; B) { }
        key(K3; C, B)
#else
        key(K2; C, B)
#endif
        {
        }
    }
}
"#;
        let af = parse(src);
        assert_eq!(af.parse_status, crate::ir::ParseStatus::Clean);
        let obj = &af.objects[0];
        let fields: Vec<(i64, &str, &str)> = obj
            .fields
            .iter()
            .map(|f| (f.number, f.name.as_str(), f.field_class.as_str()))
            .collect();
        assert_eq!(
            fields,
            [
                (1, "A", "Normal"),
                (2, "B", "Normal"),
                (3, "C", "Normal"),
                (4, "D", "FlowField"),
            ]
        );
        assert_eq!(obj.fields[1].data_type, "Text[2048]", "first arm's header");
        assert_eq!(obj.keys, [vec!["a"], vec!["b"], vec!["c", "b"]]);
    }

    /// #26: a field's ObsoleteState / ObsoleteReason reach the IR -- plain, and
    /// in the `#if` form BC actually uses (every arm reported, none picked).
    #[test]
    fn field_obsolete_state_and_reason_reach_the_ir() {
        let src = r#"
table 50120 T
{
    fields
    {
        field(1; Gone; Code[20])
        {
            ObsoleteState = Removed;
            ObsoleteReason = 'Use New instead.';
        }
        field(2; Moving; Code[20])
        {
#if not CLEAN24
            ObsoleteState = Pending;
#else
            ObsoleteState = Removed;
#endif
            ObsoleteReason = 'Replaced.';
        }
        field(3; Live; Code[20]) { }
        field(4; Amt; Decimal)
        {
#if not CLEAN24
            FieldClass = FlowField;
#endif
        }
        field(5; Split; Decimal)
        {
#if not CLEAN24
            FieldClass = FlowField;
#else
            FieldClass = Normal;
#endif
        }
    }
}
"#;
        let af = parse(src);
        assert_eq!(af.parse_status, crate::ir::ParseStatus::Clean);
        let f = &af.objects[0].fields;
        // #62: an #if-wrapped FieldClass is read; disagreeing arms stay Normal.
        assert_eq!(f[3].field_class, "FlowField", "#if-wrapped FieldClass");
        assert_eq!(f[4].field_class, "Normal", "disagreeing arms");
        assert_eq!(f[0].field_class, "Normal");
        assert_eq!(f[0].obsolete_state, ["Removed"]);
        assert_eq!(f[0].obsolete_reason, ["Use New instead."]);
        assert_eq!(f[1].obsolete_state, ["Pending", "Removed"], "both #if arms");
        assert_eq!(f[1].obsolete_reason, ["Replaced."]);
        assert!(f[2].obsolete_state.is_empty() && f[2].obsolete_reason.is_empty());
    }

    /// A whole-value `#if` property gives one `ObjectProperty` per nonempty arm,
    /// nested conditionals included, never one entry holding the directive text.
    #[test]
    fn whole_value_preproc_property_gives_one_entry_per_arm() {
        let src = r#"
page 50118 P
{
    SourceTable =
#if A
        Customer
#elif B
#if C
        "Sales Header"
#else
        Item
#endif
#else
#endif
        ;
}
"#;
        let af = parse(src);
        assert_eq!(af.parse_status, crate::ir::ParseStatus::Clean);
        let values: Vec<&str> = af.objects[0]
            .properties
            .iter()
            .filter(|p| p.name == "sourcetable")
            .map(|p| p.value.as_str())
            .collect();
        assert_eq!(values, ["Customer", "\"Sales Header\"", "Item"]);
    }

    /// Grammar 8682cb5f (B11): a property value made of a RUN of whole-value `#if`
    /// groups (`preproc_conditional_property_value_sequence`) gives one entry per arm
    /// of every group, never one entry holding the directive text.
    #[test]
    fn property_value_run_gives_one_entry_per_arm_of_every_group() {
        let src = r#"
page 50119 P
{
    SourceTable =
#if X
        Customer;
#endif
#if not X
        Vendor;
#endif
}
"#;
        let af = parse(src);
        assert_eq!(af.parse_status, crate::ir::ParseStatus::Clean);
        let values: Vec<&str> = af.objects[0]
            .properties
            .iter()
            .filter(|p| p.name == "sourcetable")
            .map(|p| p.value.as_str())
            .collect();
        assert_eq!(values, ["Customer", "Vendor"]);
    }

    /// Grammar 8682cb5f (B7b-1): an `#if` group in an `implements` list
    /// (`preproc_conditional_implements`) — every arm's interfaces are read, in order.
    #[test]
    fn implements_list_group_reads_every_arm() {
        let src = r#"
codeunit 50120 C implements IA,
#if X
    IB,
#else
    "I D",
#endif
    IC
{
}
"#;
        let af = parse(src);
        assert_eq!(af.parse_status, crate::ir::ParseStatus::Clean);
        assert_eq!(af.objects[0].implements, ["IA", "IB", "I D", "IC"]);
    }

    /// Grammar 8682cb5f (B7b-1): an `#if` group in a key's field list
    /// (`preproc_conditional_field_list_items`) gives its FIRST arm's fields — key
    /// fields are ordered, and the union `a, b, d, c` is a key no build declares.
    #[test]
    fn key_field_list_group_reads_the_first_arm() {
        let src = r#"
table 50121 T
{
    fields
    {
        field(1; A; Integer) { }
    }
    keys
    {
        key(PK; A
#if X
            , B
#else
            , D
#endif
            , C)
        {
        }
    }
}
"#;
        let af = parse(src);
        assert_eq!(af.parse_status, crate::ir::ParseStatus::Clean);
        assert_eq!(af.objects[0].keys, [["a", "b", "c"]]);
    }

    /// Grammar 8682cb5f (B7b-1): an `#if` group in a variable name list
    /// (`preproc_conditional_var_names`) declares every arm's names, in order.
    #[test]
    fn var_name_list_group_declares_every_arm() {
        let src = r#"
codeunit 50122 C
{
    var
        G1,
#if X
        G2,
#endif
        G3: Integer;

    procedure P()
    var
        A,
#if X
        B,
#else
        D,
#endif
        C: Text;
    begin
    end;
}
"#;
        let af = parse(src);
        assert_eq!(af.parse_status, crate::ir::ParseStatus::Clean);
        let names = |v: &[crate::ir::VarDecl]| v.iter().map(|d| d.name.clone()).collect::<Vec<_>>();
        assert_eq!(names(&af.objects[0].globals), ["G1", "G2", "G3"]);
        let locals = &af.objects[0].routines[0].locals;
        assert_eq!(names(locals), ["A", "B", "D", "C"]);
        assert!(locals.iter().all(|d| d.ty.as_deref() == Some("Text")));
    }

    /// Grammar 8682cb5f (B7b-1): an `#if` group among attribute arguments
    /// (`preproc_conditional_attribute_args`) gives ONE arm's args, so every later
    /// position stays put (`[EventSubscriber]` reads its event name at `args[2]`). The
    /// arm is the first one the routine's build does not rule out: `First` takes the
    /// `#if X` arm, while `Second` exists only under `not X`, so it takes `#else`.
    #[test]
    fn attribute_args_group_reads_the_routines_arm() {
        let src = r#"
codeunit 50123 S
{
    [EventSubscriber(ObjectType::Table,
#if X
        Database::"New",
#else
        Database::"Old",
#endif
        'OnX', '', false, false)]
    local procedure First()
    begin
    end;

#if not X
    [EventSubscriber(ObjectType::Table,
#if X
        Database::"New",
#else
        Database::"Old",
#endif
        'OnX', '', false, false)]
    local procedure Second()
    begin
    end;
#endif
}
"#;
        let af = parse(src);
        assert_eq!(af.parse_status, crate::ir::ParseStatus::Clean);
        let table_arg = |name: &str| {
            let r = af.objects[0]
                .routines
                .iter()
                .find(|r| r.name == name)
                .expect("routine");
            let args = &r.attributes_parsed[0].args;
            assert_eq!(args.len(), 6, "{name}: one arm's args, positions kept");
            match &af.ir.expr(args[1]).kind {
                ExprKind::DatabaseReference(s) => s.clone(),
                _ => panic!("{name}: args[1] is not a database reference"),
            }
        };
        assert_eq!(table_arg("First"), "Database::\"New\"");
        assert_eq!(table_arg("Second"), "Database::\"Old\"");
    }

    /// `preproc_split_modify`: `#if modify(A) #else modify(B) #endif { … }`. A trigger
    /// in its body belongs to the modified member (the first arm's target), as it does
    /// for a plain `modify`.
    #[test]
    fn split_modify_target_is_the_enclosing_member() {
        let src = r#"
pageextension 50117 PE extends P
{
    layout
    {
#if not C28
        modify(A)
#else
        modify(B)
#endif
        {
            trigger OnAfterValidate()
            begin
            end;
        }
    }
}
"#;
        let af = parse(src);
        assert_eq!(af.parse_status, crate::ir::ParseStatus::Clean);
        let r = &af.objects[0].routines[0];
        assert_eq!(r.name, "OnAfterValidate");
        assert_eq!(
            r.enclosing_member.as_ref().map(|(n, _)| n.as_str()),
            Some("A")
        );
        assert!(!r.in_dataset_modify_context);
    }

    /// #71: an attribute keeps its own origin, so a routine's declaration span can
    /// start at its first attached attribute.
    #[test]
    fn attribute_origin_is_the_attribute_item() {
        let src = "codeunit 1 C\n{\n    [IntegrationEvent(false, false)]\n    [Obsolete('x', '1.0')]\n    procedure P()\n    begin\n    end;\n}\n";
        let af = parse(src);
        let r = &af.objects[0].routines[0];
        let rows: Vec<(u32, u32)> = r
            .attributes_parsed
            .iter()
            .map(|a| (a.origin.start.row, a.origin.start.column))
            .collect();
        assert_eq!(rows, vec![(2, 4), (3, 4)]);
        assert_eq!(
            r.origin.start.row, 4,
            "the routine node starts at the keyword"
        );
    }

    /// #71: a header split across `#if` arms, with a shared attribute above the
    /// `#if` and one attribute inside each arm. Each arm's routine keeps the shared
    /// attribute's origin and its OWN arm attribute's origin, never the other arm's
    /// and never the split node's.
    #[test]
    fn split_header_attribute_origins_are_the_attribute_items() {
        let src = "codeunit 1 C\n{\n    [Obsolete('x', '1.0')]\n#if X\n    [IntegrationEvent(false, false)]\n    procedure Foo(A: Integer)\n#else\n    [BusinessEvent(false)]\n    procedure Foo(A: Integer; B: Integer)\n#endif\n    begin\n    end;\n}\n";
        let af = parse(src);
        assert_eq!(af.parse_status, crate::ir::ParseStatus::Clean);
        let rows: Vec<Vec<(String, u32, u32)>> = af.objects[0]
            .routines
            .iter()
            .map(|r| {
                r.attributes_parsed
                    .iter()
                    .map(|a| (a.name.clone(), a.origin.start.row, a.origin.start.column))
                    .collect()
            })
            .collect();
        assert_eq!(
            rows,
            vec![
                vec![
                    ("Obsolete".to_string(), 2, 4),
                    ("IntegrationEvent".to_string(), 4, 4)
                ],
                vec![
                    ("Obsolete".to_string(), 2, 4),
                    ("BusinessEvent".to_string(), 7, 4)
                ],
            ]
        );
    }

    /// #71 + #80: an attribute separated from its routine by a comment IS attached
    /// (a comment is trivia between the two, not the end of the attribute run), and
    /// it keeps its own origin, so the routine's declaration span reaches the
    /// attribute's line (row 2), above the comment.
    #[test]
    fn attribute_separated_by_comment_is_attached() {
        let src = "codeunit 1 C\n{\n    [IntegrationEvent(false, false)]\n    // note\n    procedure P()\n    begin\n    end;\n}\n";
        let af = parse(src);
        let r = &af.objects[0].routines[0];
        assert_eq!(r.name, "P");
        let names: Vec<&str> = r
            .attributes_parsed
            .iter()
            .map(|a| a.name.as_str())
            .collect();
        assert_eq!(
            names,
            vec!["IntegrationEvent"],
            "#80: the comment must not drop the attribute"
        );
        let rows: Vec<u32> = r
            .attributes_parsed
            .iter()
            .map(|a| a.origin.start.row)
            .collect();
        assert_eq!(rows, vec![2]);
    }

    /// #71: a multi-line attribute's origin starts at its FIRST line (`[`), so a
    /// routine's declaration span starts there, not at the attribute's last line.
    #[test]
    fn multi_line_attribute_origin_starts_at_its_first_line() {
        let src = "codeunit 1 C\n{\n    [EventSubscriber(ObjectType::Codeunit, Codeunit::\"X\",\n        'OnFoo', '', false, false)]\n    local procedure H()\n    begin\n    end;\n}\n";
        let af = parse(src);
        assert_eq!(af.parse_status, crate::ir::ParseStatus::Clean);
        let r = &af.objects[0].routines[0];
        let rows: Vec<(u32, u32)> = r
            .attributes_parsed
            .iter()
            .map(|a| (a.origin.start.row, a.origin.end.row))
            .collect();
        assert_eq!(rows, vec![(2, 3)]);
        assert_eq!(r.origin.start.row, 4);
    }

    // ---------------------------------------------------------------------
    // #80: an attribute separated from its routine by a comment, a pragma or an
    // `#if` line. Each test asserts the EXACT attribute-name list of every
    // routine it names, and the event-name argument (`args[2]`) of every
    // `[EventSubscriber]`.
    // ---------------------------------------------------------------------

    /// #80: the attribute names of every routine called `name` in object `obj`,
    /// one list per routine (overloads and per-arm copies), in source order.
    fn attrs_all(af: &crate::ir::AlFile, obj: usize, name: &str) -> Vec<Vec<String>> {
        af.objects[obj]
            .routines
            .iter()
            .filter(|r| r.name == name)
            .map(|r| r.attributes_parsed.iter().map(|a| a.name.clone()).collect())
            .collect()
    }

    /// #80: the attribute names of the ONE routine called `name` in object `obj`.
    fn attrs(af: &crate::ir::AlFile, obj: usize, name: &str) -> Vec<String> {
        let all = attrs_all(af, obj, name);
        assert_eq!(all.len(), 1, "{name}: expected exactly one routine");
        all.into_iter().next().unwrap()
    }

    /// #80: `(first parameter name, attribute names)` of every routine called `name`
    /// in object `obj`, so two overloads (or two split-header arms) are told apart.
    fn attrs_by_param(
        af: &crate::ir::AlFile,
        obj: usize,
        name: &str,
    ) -> Vec<(String, Vec<String>)> {
        af.objects[obj]
            .routines
            .iter()
            .filter(|r| r.name == name)
            .map(|r| {
                (
                    r.params.first().map(|p| p.name.clone()).unwrap_or_default(),
                    r.attributes_parsed.iter().map(|a| a.name.clone()).collect(),
                )
            })
            .collect()
    }

    /// #80: the source text of argument `i` of every `[EventSubscriber]` on the ONE
    /// routine called `name` in object `obj`, in attribute order.
    fn subscriber_arg(
        af: &crate::ir::AlFile,
        src: &str,
        obj: usize,
        name: &str,
        i: usize,
    ) -> Vec<String> {
        let rs: Vec<_> = af.objects[obj]
            .routines
            .iter()
            .filter(|r| r.name == name)
            .collect();
        assert_eq!(rs.len(), 1, "{name}: expected exactly one routine");
        rs[0]
            .attributes_parsed
            .iter()
            .filter(|a| a.name.eq_ignore_ascii_case("EventSubscriber"))
            .map(|a| {
                let id = *a
                    .args
                    .get(i)
                    .unwrap_or_else(|| panic!("{name}: EventSubscriber has no args[{i}]"));
                src[af.ir.expr(id).origin.byte.clone()].to_string()
            })
            .collect()
    }

    fn names(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    /// A1 (#80): a line comment, `///` doc comments and a same-line trailing
    /// comment between an attribute and its routine do not drop the attribute.
    #[test]
    fn attribute_survives_a_line_comment() {
        let src = r#"codeunit 50100 T
{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::T, 'OnFire', '', false, false)]
    // note
    local procedure Handler()
    begin
    end;

    /// <summary>doc</summary>
    [Scope('OnPrem')]
    /// <summary>doc2</summary>
    procedure DocComment()
    begin
    end;

    [Scope('OnPrem')] // trailing
    procedure TrailingComment()
    begin
    end;
}
"#;
        let af = parse(src);
        assert_eq!(af.parse_status, crate::ir::ParseStatus::Clean);
        assert_eq!(attrs(&af, 0, "Handler"), names(&["EventSubscriber"]));
        assert_eq!(
            subscriber_arg(&af, src, 0, "Handler", 2),
            names(&["'OnFire'"])
        );
        assert_eq!(attrs(&af, 0, "DocComment"), names(&["Scope"]));
        assert_eq!(attrs(&af, 0, "TrailingComment"), names(&["Scope"]));
    }

    /// A2 (#80): a one-line and a multi-line block comment do not drop the attribute.
    #[test]
    fn attribute_survives_a_block_comment() {
        let src = r#"codeunit 50100 T
{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::T, 'OnFire', '', false, false)]
    /* note */
    local procedure Handler()
    begin
        Commit();
    end;

    [IntegrationEvent(false, false)]
    /*
       multi
       line
    */
    procedure OnFire()
    begin
    end;
}
"#;
        let af = parse(src);
        assert_eq!(af.parse_status, crate::ir::ParseStatus::Clean);
        assert_eq!(attrs(&af, 0, "Handler"), names(&["EventSubscriber"]));
        assert_eq!(
            subscriber_arg(&af, src, 0, "Handler", 2),
            names(&["'OnFire'"])
        );
        assert_eq!(attrs(&af, 0, "OnFire"), names(&["IntegrationEvent"]));
    }

    /// A3 (#80): `#pragma`, `#region`/`#endregion` and `#define`/`#undef` lines
    /// (the grammar's extras) do not drop the attribute.
    #[test]
    fn attribute_survives_pragma_and_region_lines() {
        let src = r#"codeunit 50100 T
{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::T, 'OnFire', '', false, false)]
#pragma warning disable AA0150
    local procedure Handler()
    begin
    end;
#pragma warning restore AA0150

    [IntegrationEvent(false, false)]
#region R
    procedure OnFire()
    begin
    end;
#endregion

    [Scope('OnPrem')]
#define FOO
#undef FOO
    procedure DefineUndef()
    begin
    end;
}
"#;
        let af = parse(src);
        assert_eq!(af.parse_status, crate::ir::ParseStatus::Clean);
        assert_eq!(attrs(&af, 0, "Handler"), names(&["EventSubscriber"]));
        assert_eq!(
            subscriber_arg(&af, src, 0, "Handler", 2),
            names(&["'OnFire'"])
        );
        assert_eq!(attrs(&af, 0, "OnFire"), names(&["IntegrationEvent"]));
        assert_eq!(attrs(&af, 0, "DefineUndef"), names(&["Scope"]));
    }

    /// A4 (#80): a 3-line attribute followed by a comment keeps all 6 arguments.
    #[test]
    fn multi_line_attribute_survives_a_comment() {
        let src = r#"codeunit 50100 T
{
    [EventSubscriber(ObjectType::Codeunit,
        Codeunit::T, 'OnFire', '',
        false, false)]
    // c
    local procedure Multi()
    begin
    end;
}
"#;
        let af = parse(src);
        assert_eq!(af.parse_status, crate::ir::ParseStatus::Clean);
        assert_eq!(attrs(&af, 0, "Multi"), names(&["EventSubscriber"]));
        let all: Vec<String> = (0..6)
            .map(|i| subscriber_arg(&af, src, 0, "Multi", i).join("|"))
            .collect();
        assert_eq!(
            all,
            names(&[
                "ObjectType::Codeunit",
                "Codeunit::T",
                "'OnFire'",
                "''",
                "false",
                "false"
            ])
        );
    }

    /// A5 (#80): two attributes split by a comment are both kept, in source order.
    #[test]
    fn attributes_split_by_a_comment_are_both_kept_in_order() {
        let src = r#"codeunit 50100 T
{
    [Scope('OnPrem')]
    // c
    [NonDebuggable]
    procedure TwoAttrs()
    begin
    end;
}
"#;
        let af = parse(src);
        assert_eq!(af.parse_status, crate::ir::ParseStatus::Clean);
        assert_eq!(
            attrs(&af, 0, "TwoAttrs"),
            names(&["Scope", "NonDebuggable"])
        );
    }

    /// A6 (#80): an `#if` that wraps only attributes gives the routine every live
    /// arm's attributes (union-read), plus any attribute written before the `#if`.
    #[test]
    fn if_wrapping_only_attributes_gives_every_live_arm() {
        let src = r#"codeunit 50100 T
{
#if not CLEAN25
    [Obsolete('x', '25.0')]
#endif
    procedure OnlyAttrs()
    begin
    end;

    [Scope('OnPrem')]
#if not CLEAN25
    [Obsolete('x', '25.0')]
#endif
    procedure MixedBefore()
    begin
    end;

    [Scope('OnPrem')]
#if X
    [NonDebuggable]
#else
    [Obsolete('x', '25.0')]
#endif
    procedure MixedBeforeElse()
    begin
    end;

#if CLEAN25
    [EventSubscriber(ObjectType::Codeunit, Codeunit::T, 'OnA', '', false, false)]
#else
    [EventSubscriber(ObjectType::Codeunit, Codeunit::T, 'OnB', '', false, false)]
#endif
    local procedure PerArm()
    begin
    end;

#if A
    [Scope('OnPrem')]
#elif B
    [NonDebuggable]
#else
    [Obsolete('x', '25.0')]
#endif
    procedure Elif()
    begin
    end;

#if A
#if B
    [Scope('OnPrem')]
#endif
#endif
    procedure Nested()
    begin
    end;

#if A
    // c
    [Scope('OnPrem')]
    /* d */
#endif
    procedure TriviaInArm()
    begin
    end;

    [Scope('OnPrem')]
#if not CLEAN25
#pragma warning disable AL0432
#endif
    procedure PragmaInIf()
    begin
    end;

    [Scope('OnPrem')]
#if A
#endif
    procedure EmptyIf()
    begin
    end;
}
"#;
        let af = parse(src);
        assert_eq!(af.parse_status, crate::ir::ParseStatus::Clean);
        assert_eq!(attrs(&af, 0, "OnlyAttrs"), names(&["Obsolete"]));
        assert_eq!(attrs(&af, 0, "MixedBefore"), names(&["Scope", "Obsolete"]));
        assert_eq!(
            attrs(&af, 0, "MixedBeforeElse"),
            names(&["Scope", "NonDebuggable", "Obsolete"])
        );
        assert_eq!(
            attrs(&af, 0, "PerArm"),
            names(&["EventSubscriber", "EventSubscriber"])
        );
        assert_eq!(
            subscriber_arg(&af, src, 0, "PerArm", 2),
            names(&["'OnA'", "'OnB'"])
        );
        assert_eq!(
            attrs(&af, 0, "Elif"),
            names(&["Scope", "NonDebuggable", "Obsolete"])
        );
        assert_eq!(attrs(&af, 0, "Nested"), names(&["Scope"]));
        assert_eq!(attrs(&af, 0, "TriviaInArm"), names(&["Scope"]));
        assert_eq!(attrs(&af, 0, "PragmaInIf"), names(&["Scope"]));
        assert_eq!(attrs(&af, 0, "EmptyIf"), names(&["Scope"]));
    }

    /// A7 (#80, negative control): an attribute in a dead arm attaches to nothing,
    /// and a routine in a dead arm is not collected. Inside `#if X`, the inner
    /// `#if not X` arm is dead, so `DeadArm` has no attribute. Inside `#if not X`,
    /// `#if X procedure P` is dead (P is absent) and the attribute falls through to Q.
    #[test]
    fn if_attribute_in_a_dead_arm_is_not_attached() {
        let src = r#"codeunit 50100 T
{
#if X
#if not X
    [Scope('OnPrem')]
#endif
    procedure DeadArm()
    begin
    end;
#endif

#if not X
    [Scope('OnPrem')]
#if X
    procedure P()
    begin
    end;
#endif
    procedure Q()
    begin
    end;
#endif
}
"#;
        let af = parse(src);
        assert_eq!(af.parse_status, crate::ir::ParseStatus::Clean);
        assert_eq!(attrs(&af, 0, "DeadArm"), Vec::<String>::new());
        assert!(attrs_all(&af, 0, "P").is_empty(), "P is in a dead arm");
        assert_eq!(attrs(&af, 0, "Q"), names(&["Scope"]));
    }

    /// A7b (#80): inside `#if X`, `[A] #if X procedure P #endif procedure Q` — the
    /// arm is decided TRUE, so no build falls through to Q: P has A, Q has nothing.
    #[test]
    fn fall_through_is_dead_when_an_arm_is_decided_true() {
        let src = r#"codeunit 50100 T
{
#if X
    [Scope('OnPrem')]
#if X
    procedure P()
    begin
    end;
#endif
    procedure Q()
    begin
    end;
#endif
}
"#;
        let af = parse(src);
        assert_eq!(af.parse_status, crate::ir::ParseStatus::Clean);
        assert_eq!(attrs(&af, 0, "P"), names(&["Scope"]));
        assert_eq!(attrs(&af, 0, "Q"), Vec::<String>::new());
    }

    /// A8 (#80): an attribute before an `#if` that wraps the routine reaches the
    /// routine in each arm. With an `#else` it does not leak to the next routine;
    /// without one it also reaches the next routine (the no-arm-taken build).
    #[test]
    fn attribute_before_if_reaches_each_arm_and_does_not_leak() {
        let src = r#"codeunit 50100 T
{
    [Scope('OnPrem')]
#if X
    procedure P(I: Integer)
    begin
    end;
#else
    procedure P(T: Text)
    begin
    end;
#endif
    procedure Q()
    begin
    end;

    [Scope('OnPrem')]
#if X
    procedure P2()
    begin
    end;
#endif
    procedure Q2()
    begin
    end;
}
"#;
        let af = parse(src);
        assert_eq!(af.parse_status, crate::ir::ParseStatus::Clean);
        assert_eq!(
            attrs_by_param(&af, 0, "P"),
            vec![
                ("I".to_string(), names(&["Scope"])),
                ("T".to_string(), names(&["Scope"]))
            ]
        );
        assert_eq!(attrs(&af, 0, "Q"), Vec::<String>::new());
        assert_eq!(attrs(&af, 0, "P2"), names(&["Scope"]));
        assert_eq!(attrs(&af, 0, "Q2"), names(&["Scope"]));
    }

    /// A9 (#80): one attribute that reaches a routine by two `#if` paths (a
    /// pragma-only `#if X/#else`) is attached ONCE. The later `#if X procedure Z`
    /// keeps `X` relevant, so the two paths stay distinct up to `P`.
    #[test]
    fn attribute_reaching_a_routine_by_two_paths_is_attached_once() {
        let src = r#"codeunit 50100 T
{
    [Scope('OnPrem')]
#if X
#pragma warning disable AA0001
#else
#pragma warning disable AA0002
#endif
    procedure P()
    begin
    end;

#if X
    procedure Z()
    begin
    end;
#endif
}
"#;
        let af = parse(src);
        assert_eq!(af.parse_status, crate::ir::ParseStatus::Clean);
        assert_eq!(attrs(&af, 0, "P"), names(&["Scope"]));
        assert_eq!(attrs(&af, 0, "Z"), Vec::<String>::new());
    }

    /// A10 (#80): a comment between an attribute and a trigger, and between an
    /// attribute and an interface procedure, does not drop the attribute.
    #[test]
    fn attribute_survives_a_comment_before_a_trigger_and_an_interface_procedure() {
        let src = r#"codeunit 50100 T
{
    [NonDebuggable]
    // c
    trigger OnRun()
    begin
    end;
}

interface I
{
    [Obsolete('x', '25.0')]
    // c
    procedure IProc();
}
"#;
        let af = parse(src);
        assert_eq!(af.parse_status, crate::ir::ParseStatus::Clean);
        assert_eq!(attrs(&af, 0, "OnRun"), names(&["NonDebuggable"]));
        assert_eq!(attrs(&af, 1, "IProc"), names(&["Obsolete"]));
    }

    /// A11 (#80, negative controls): a `var` section still ends the attribute run;
    /// an attribute dangling at the end of a table's `fields` body does not reach
    /// the table's procedure; an attribute dangling at the end of one object does
    /// not reach the next object.
    #[test]
    fn attribute_is_still_dropped_by_a_var_section_and_at_body_end() {
        let src = r#"codeunit 50100 T
{
    [Scope('OnPrem')]
    var
        G: Integer;

    procedure AfterVar()
    begin
    end;
}

table 50100 Tb
{
    fields
    {
        field(1; A; Integer) { }
        [Scope('OnPrem')]
    }

    procedure TP()
    begin
    end;
}

codeunit 50101 D1
{
    procedure P()
    begin
    end;

    [Scope('OnPrem')]
}

codeunit 50102 D2
{
    procedure First()
    begin
    end;
}
"#;
        let af = parse(src);
        assert_eq!(af.parse_status, crate::ir::ParseStatus::Clean);
        assert_eq!(attrs(&af, 0, "AfterVar"), Vec::<String>::new());
        assert_eq!(attrs(&af, 1, "TP"), Vec::<String>::new());
        assert_eq!(attrs(&af, 2, "P"), Vec::<String>::new());
        assert_eq!(attrs(&af, 3, "First"), Vec::<String>::new());
    }

    /// A13 (#80): `[A] #if X P #endif #if X R #endif Q`. A survives the first
    /// wrapper only in the not-X build, so the second `#if X` arm (R) never sees
    /// it, while P (X build) and Q (not-X build) both have it.
    #[test]
    fn fall_through_guard_blocks_a_later_arm_of_the_same_condition() {
        let src = r#"codeunit 50100 T
{
    [Scope('OnPrem')]
#if X
    procedure P()
    begin
    end;
#endif
#if X
    procedure R()
    begin
    end;
#endif
    procedure Q()
    begin
    end;
}
"#;
        let af = parse(src);
        assert_eq!(af.parse_status, crate::ir::ParseStatus::Clean);
        assert_eq!(attrs(&af, 0, "P"), names(&["Scope"]));
        assert_eq!(attrs(&af, 0, "R"), Vec::<String>::new());
        assert_eq!(attrs(&af, 0, "Q"), names(&["Scope"]));
    }

    /// #80 review: the A13 shape spelled `# if`/`# endif` and `#IF`/`#ENDIF`, which
    /// the grammar's scanner accepts. Projection must still see the second `X`
    /// condition, or the `{X:false}` fall-through guard loses `X` and `R` gets `Scope`.
    #[test]
    fn projection_sees_spaced_and_upper_case_directives() {
        let src = r#"codeunit 50100 T
{
    [Scope('OnPrem')]
# if X
    procedure P()
    begin
    end;
# endif
#IF X
    procedure R()
    begin
    end;
#ENDIF
    procedure Q()
    begin
    end;
}
"#;
        let af = parse(src);
        assert_eq!(af.parse_status, crate::ir::ParseStatus::Clean);
        assert_eq!(attrs(&af, 0, "P"), names(&["Scope"]));
        assert_eq!(attrs(&af, 0, "R"), Vec::<String>::new());
        assert_eq!(attrs(&af, 0, "Q"), names(&["Scope"]));
    }

    /// A14 (#80): an attribute inside `#if not X`, carried to H across the
    /// `#endif`, reads its `#if X 'OnA' #else 'OnB'` argument group under its own
    /// arm (not-X), so H subscribes to 'OnB' only.
    #[test]
    fn carried_attribute_args_read_under_their_arm() {
        let src = r#"codeunit 50100 T
{
#if not X
    [EventSubscriber(ObjectType::Codeunit, Codeunit::T,
#if X
        'OnA',
#else
        'OnB',
#endif
        '', false, false)]
#endif
    local procedure H()
    begin
    end;
}
"#;
        let af = parse(src);
        assert_eq!(af.parse_status, crate::ir::ParseStatus::Clean);
        assert_eq!(attrs(&af, 0, "H"), names(&["EventSubscriber"]));
        assert_eq!(subscriber_arg(&af, src, 0, "H", 2), names(&["'OnB'"]));
    }

    /// A15 (#80): an attribute inside `#if X` reaches only the `#if X` arm of the
    /// following split header, `P(I)`, never the `#else` arm `P(T)`.
    #[test]
    fn guarded_attribute_reaches_only_its_split_header_arm() {
        let src = r#"codeunit 50100 T
{
#if X
    [Scope('OnPrem')]
#endif
#if X
    procedure P(I: Integer)
#else
    procedure P(T: Text)
#endif
    begin
    end;
}
"#;
        let af = parse(src);
        assert_eq!(af.parse_status, crate::ir::ParseStatus::Clean);
        assert_eq!(
            attrs_by_param(&af, 0, "P"),
            vec![
                ("I".to_string(), names(&["Scope"])),
                ("T".to_string(), Vec::<String>::new())
            ]
        );
    }

    /// Final review F1 (#80): the guard `not (X or Y)` records `{X:false, Y:false}`,
    /// but the split header's first arm (`#if X or Y`) records no literals, so a
    /// literal-only check keeps it. That arm is unreachable under the guard (its
    /// condition is decided false), so only the `#else` arm `P(b)` gets the attribute.
    #[test]
    fn guarded_attribute_skips_an_unreachable_split_header_arm() {
        let src = r#"codeunit 50100 T
{
#if not (X or Y)
    [EventSubscriber(ObjectType::Codeunit, Codeunit::T, 'OnFire', '', false, false)]
#endif
#if X or Y
    procedure P(a: Integer)
#else
    procedure P(b: Text)
#endif
    begin
    end;
}
"#;
        let af = parse(src);
        assert_eq!(af.parse_status, crate::ir::ParseStatus::Clean);
        assert_eq!(
            attrs_by_param(&af, 0, "P"),
            vec![
                ("a".to_string(), Vec::<String>::new()),
                ("b".to_string(), names(&["EventSubscriber"]))
            ]
        );
    }

    /// Final review F3/G7 (#80): an attribute with an `#if` group in its arguments
    /// reaches H by two paths (`X` and `not X`) that read different arguments, so both
    /// selections survive, but the group is reported ONCE.
    #[test]
    fn attribute_arg_group_issue_is_reported_once() {
        let src = r#"codeunit 50100 T
{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::T,
#if X
        'OnA',
#else
        'OnB',
#endif
        '', false, false)]
#if X
#else
#endif
    local procedure H()
    begin
    end;
}
"#;
        let af = parse(src);
        assert_eq!(af.parse_status, crate::ir::ParseStatus::Clean);
        assert_eq!(
            subscriber_arg(&af, src, 0, "H", 2),
            names(&["'OnA'", "'OnB'"])
        );
        let n = af
            .issues
            .iter()
            .filter(|i| i.message.starts_with("`#if` inside attribute arguments"))
            .count();
        assert_eq!(n, 1, "issues: {:?}", af.issues);
    }

    /// Final review G5 (#80): `consistent` validates BOTH operands, so a contradiction
    /// inside the first one is not copied through.
    #[test]
    fn consistent_rejects_a_contradictory_first_operand() {
        use super::consistent;
        let x = |v| ("X".to_string(), v);
        assert_eq!(consistent(&[x(true), x(false)], &[]), None);
        assert_eq!(consistent(&[], &[x(true), x(false)]), None);
        assert_eq!(consistent(&[x(true)], &[x(true)]), Some(vec![x(true)]));
    }

    /// Final review G5 (#80): inside `#if X`, a split header's `#if not X` arm is no
    /// build, so the attribute inside that arm attaches nothing to it.
    #[test]
    fn contradictory_context_in_node_attribute_attaches_nothing() {
        let src = r#"codeunit 50100 T
{
#if X
#if not X
    [Scope('OnPrem')]
    procedure P(a: Integer)
#else
    procedure P(b: Text)
#endif
    begin
    end;
#endif
}
"#;
        let af = parse(src);
        assert_eq!(af.parse_status, crate::ir::ParseStatus::Clean);
        assert_eq!(
            attrs_by_param(&af, 0, "P"),
            vec![
                ("a".to_string(), Vec::<String>::new()),
                ("b".to_string(), Vec::<String>::new())
            ]
        );
    }

    /// Final review G6 (#80), a STATED LIMIT, not a goal: the running context records
    /// only symbol LITERALS. The first group is chosen under `X or Y`, which records
    /// none, so it does not constrain the second group (`not X and not Y`), and `H`
    /// reads `'PubA'` with `'OnA'` although no build has both. If this test starts
    /// failing because the context got smarter, update it and the CHANGELOG bullet.
    #[test]
    fn sibling_groups_under_a_disjunction_are_not_correlated() {
        let src = r#"codeunit 50100 T
{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::T,
#if X or Y
        'PubA',
#else
        'PubB',
#endif
#if not X and not Y
        'OnA',
#else
        'OnB',
#endif
        '', false, false)]
    local procedure H()
    begin
    end;
}
"#;
        let af = parse(src);
        assert_eq!(af.parse_status, crate::ir::ParseStatus::Clean);
        assert_eq!(subscriber_arg(&af, src, 0, "H", 2), names(&["'PubA'"]));
        assert_eq!(subscriber_arg(&af, src, 0, "H", 3), names(&["'OnA'"]));
    }

    /// Review round 2 (#80): the attribute flow scales linearly in two pathological
    /// shapes. (1) Many attributes before one `#if X` wrapper holding many procedures:
    /// each pending entry evaluates the wrapper's directives, never all its children.
    /// (2) One attribute under a long single-path guard (`#if X0 and … and Xn`)
    /// crossing many known-true `#if Xi` wrappers, with every `Xi` mentioned again
    /// later (one `#if X0 or … or Xn` after the routine) so no literal is projected
    /// away: each guard join is a hashed, linear check, not a nested scan. The sizes
    /// make the quadratic forms take minutes in a debug build (measured: see the #80
    /// review-fix report) against seconds now. Run on a big stack: the `and` and `or`
    /// chains nest `n` deep.
    #[test]
    fn attribute_flow_scales_linearly() {
        const ATTRS: usize = 20000;
        const SYMS: usize = 16000;
        const WRAPPERS: usize = 300;
        std::thread::Builder::new()
            .stack_size(256 << 20)
            .spawn(|| {
                let mut a = String::from("codeunit 50100 T\n{\n");
                for _ in 0..ATTRS {
                    a.push_str("    [Scope('OnPrem')]\n");
                }
                a.push_str("#if X\n");
                for i in 0..ATTRS {
                    a.push_str(&format!(
                        "    procedure P{i}(a: Integer)\n    begin\n    end;\n"
                    ));
                }
                a.push_str("#endif\n    procedure Q(b: Integer)\n    begin\n    end;\n}\n");
                let af = parse(&a);
                assert_eq!(af.parse_status, crate::ir::ParseStatus::Clean);
                let count = |af: &crate::ir::AlFile, name: &str| {
                    attrs_by_param(af, 0, name)
                        .into_iter()
                        .map(|(_, v)| v.len())
                        .collect::<Vec<_>>()
                };
                assert_eq!(count(&af, "P0"), vec![ATTRS]);
                assert_eq!(count(&af, "P1"), vec![0]);
                assert_eq!(count(&af, "Q"), vec![ATTRS]);

                let all: Vec<String> = (0..SYMS).map(|i| format!("X{i}")).collect();
                let mut b = format!("codeunit 50100 T\n{{\n#if {}\n", all.join(" and "));
                b.push_str("    [Scope('OnPrem')]\n");
                for s in &all[..WRAPPERS] {
                    b.push_str(&format!("#if {s}\n#endif\n"));
                }
                b.push_str("    procedure P(a: Integer)\n    begin\n    end;\n");
                b.push_str(&format!("#if {}\n#endif\n#endif\n}}\n", all.join(" or ")));
                let af = parse(&b);
                assert_eq!(af.parse_status, crate::ir::ParseStatus::Clean);
                assert_eq!(
                    attrs_by_param(&af, 0, "P"),
                    vec![("a".to_string(), vec!["Scope".to_string()])]
                );
            })
            .expect("spawn")
            .join()
            .expect("test thread");
    }

    /// A16 (#80): a contradictory path (`#if X … #elif not X` falling through, or
    /// an `#elif X` after `#if X`) describes no build, so it attaches nothing.
    #[test]
    fn contradictory_guard_paths_attach_nothing() {
        let src = r#"codeunit 50100 T
{
    [Scope('OnPrem')]
#if X
    procedure P()
    begin
    end;
#elif not X
    procedure R()
    begin
    end;
#endif
    procedure Q()
    begin
    end;

#if X
    [Scope('OnPrem')]
#elif X
    [NonDebuggable]
#endif
    procedure H()
    begin
    end;
}
"#;
        let af = parse(src);
        assert_eq!(af.parse_status, crate::ir::ParseStatus::Clean);
        assert_eq!(attrs(&af, 0, "P"), names(&["Scope"]));
        assert_eq!(attrs(&af, 0, "R"), names(&["Scope"]));
        assert_eq!(attrs(&af, 0, "Q"), Vec::<String>::new());
        assert_eq!(attrs(&af, 0, "H"), names(&["Scope"]));
    }

    /// A17 (#80): 40 pragma-only `#if Xi/#else` wrappers over distinct symbols,
    /// none read later, keep ONE pending entry, so the attribute still attaches
    /// (no 2^40 path blow-up, no cap).
    #[test]
    fn many_pragma_only_wrappers_stay_bounded() {
        let mut src = String::from("codeunit 50100 T\n{\n    [Scope('OnPrem')]\n");
        for i in 1..=40 {
            src.push_str(&format!(
                "#if X{i}\n#pragma warning disable AA0001\n#else\n#pragma warning disable AA0002\n#endif\n"
            ));
        }
        src.push_str("    procedure P()\n    begin\n    end;\n}\n");
        let af = parse(&src);
        assert_eq!(af.parse_status, crate::ir::ParseStatus::Clean);
        assert_eq!(attrs(&af, 0, "P"), names(&["Scope"]));
        assert!(
            af.issues.iter().all(|i| !i.message.contains("64")),
            "no path-cap issue expected"
        );
    }

    /// A18 (#80): a nested argument group is read under the arm chosen for its
    /// parent group: inside `#if Y`, the `#if not Y` arm is dead, so the reading
    /// is 'OnA' (never 'Never', which no build has).
    #[test]
    fn nested_attribute_arg_group_reads_under_the_chosen_arm() {
        let src = r#"codeunit 1 C
{
    [EventSubscriber(ObjectType::Codeunit, 1,
#if Y
#if not Y
        'Never',
#else
        'OnA',
#endif
#else
        'OnB',
#endif
        '', false, false)]
    local procedure H()
    begin
    end;
}
"#;
        let af = parse(src);
        assert_eq!(af.parse_status, crate::ir::ParseStatus::Clean);
        assert_eq!(attrs(&af, 0, "H"), names(&["EventSubscriber"]));
        assert_eq!(subscriber_arg(&af, src, 0, "H", 2), names(&["'OnA'"]));
    }

    /// A19 (#80): each exact path keeps its own argument reading. P exists only
    /// under X and Y, so it reads 'OnA'; H is reached under {X, not Y} and {not X},
    /// both of which read 'OnB', so H has exactly one subscriber, to 'OnB'.
    #[test]
    fn exact_paths_keep_their_argument_reading() {
        let src = r#"codeunit 1 C
{
    [EventSubscriber(ObjectType::Codeunit, 1,
#if X and Y
        'OnA',
#else
        'OnB',
#endif
        '', false, false)]
#if X
#if Y
    local procedure P()
    begin
    end;
#endif
#endif
    local procedure H()
    begin
    end;
}
"#;
        let af = parse(src);
        assert_eq!(af.parse_status, crate::ir::ParseStatus::Clean);
        assert_eq!(attrs(&af, 0, "P"), names(&["EventSubscriber"]));
        assert_eq!(subscriber_arg(&af, src, 0, "P", 2), names(&["'OnA'"]));
        assert_eq!(attrs(&af, 0, "H"), names(&["EventSubscriber"]));
        assert_eq!(subscriber_arg(&af, src, 0, "H", 2), names(&["'OnB'"]));
    }

    /// A20 (#80): sibling argument groups in one attribute share one running
    /// context: choosing `Codeunit::PubA` (X) makes the event-name group choose its
    /// X arm, 'OnA' — the pair one build really has.
    #[test]
    fn sibling_attribute_arg_groups_share_one_context() {
        let src = r#"codeunit 1 C
{
    [EventSubscriber(ObjectType::Codeunit,
#if X
        Codeunit::PubA,
#else
        Codeunit::PubB,
#endif
#if not X
        'OnB',
#else
        'OnA',
#endif
        '', false, false)]
    local procedure H2()
    begin
    end;
}
"#;
        let af = parse(src);
        assert_eq!(af.parse_status, crate::ir::ParseStatus::Clean);
        assert_eq!(attrs(&af, 0, "H2"), names(&["EventSubscriber"]));
        assert_eq!(
            subscriber_arg(&af, src, 0, "H2", 1),
            names(&["Codeunit::PubA"])
        );
        assert_eq!(subscriber_arg(&af, src, 0, "H2", 2), names(&["'OnA'"]));
    }

    /// A21 (#80): attributes keep source order on a routine even when one arrives
    /// by fall-through and the other from inside a consumed arm.
    #[test]
    fn attributes_keep_source_order_across_a_consuming_arm() {
        let src = r#"codeunit 1 C
{
    [Scope('OnPrem')]
#if X
    procedure P3()
    begin
    end;

    [NonDebuggable]
#endif
    procedure Q3()
    begin
    end;
}
"#;
        let af = parse(src);
        assert_eq!(af.parse_status, crate::ir::ParseStatus::Clean);
        assert_eq!(attrs(&af, 0, "P3"), names(&["Scope"]));
        assert_eq!(attrs(&af, 0, "Q3"), names(&["Scope", "NonDebuggable"]));
    }

    /// A22 (#80): seven pragma-only wrappers over X1..X7, then a routine guarded by
    /// all seven. Every symbol stays relevant, so the attribute reaches P/Q through
    /// more than 64 distinct paths. It is DROPPED, and loudly: exactly one
    /// `SyntaxIssue` names the 64-path limit, at the attribute.
    #[test]
    fn too_many_relevant_paths_drops_with_a_syntax_issue() {
        let src = r#"codeunit 1 C
{
    [Scope('OnPrem')]
#if X1
#pragma warning disable AA0001
#else
#pragma warning disable AA0002
#endif
#if X2
#pragma warning disable AA0001
#else
#pragma warning disable AA0002
#endif
#if X3
#pragma warning disable AA0001
#else
#pragma warning disable AA0002
#endif
#if X4
#pragma warning disable AA0001
#else
#pragma warning disable AA0002
#endif
#if X5
#pragma warning disable AA0001
#else
#pragma warning disable AA0002
#endif
#if X6
#pragma warning disable AA0001
#else
#pragma warning disable AA0002
#endif
#if X7
#pragma warning disable AA0001
#else
#pragma warning disable AA0002
#endif
#if X1 and X2 and X3 and X4 and X5 and X6 and X7
    procedure P()
    begin
    end;
#endif
    procedure Q()
    begin
    end;
}
"#;
        let af = parse(src);
        assert_eq!(af.parse_status, crate::ir::ParseStatus::Clean);
        assert_eq!(attrs(&af, 0, "P"), Vec::<String>::new());
        assert_eq!(attrs(&af, 0, "Q"), Vec::<String>::new());
        let cap: Vec<_> = af
            .issues
            .iter()
            .filter(|i| i.message.contains("64"))
            .collect();
        assert_eq!(
            cap.len(),
            1,
            "exactly one path-cap issue expected, issues: {:?}",
            af.issues.iter().map(|i| &i.message).collect::<Vec<_>>()
        );
        assert_eq!(
            (cap[0].origin.start.row, cap[0].origin.start.column),
            (2, 4),
            "the issue is at the attribute"
        );
    }

    /// A23 (#80): deduplication runs per split-header arm AFTER the arm filter. The
    /// pragma-only `#if X/#else` gives `[Scope]` two paths, {X} and {not X}; each
    /// split-header arm keeps the matching one, so BOTH `P2(I)` and `P2(T)` have it.
    #[test]
    fn dedup_after_split_header_arm_filter() {
        let src = r#"codeunit 1 C
{
    [Scope('OnPrem')]
#if X
#pragma warning disable AA0001
#else
#pragma warning disable AA0002
#endif
#if X
    procedure P2(I: Integer)
#else
    procedure P2(T: Text)
#endif
    begin
    end;
}
"#;
        let af = parse(src);
        assert_eq!(af.parse_status, crate::ir::ParseStatus::Clean);
        assert_eq!(
            attrs_by_param(&af, 0, "P2"),
            vec![
                ("I".to_string(), names(&["Scope"])),
                ("T".to_string(), names(&["Scope"]))
            ]
        );
    }

    /// The `Implementation` values of one lowered enum value, as `&str`s.
    fn enum_impls(v: &crate::ir::EnumValueDecl) -> Vec<&str> {
        v.implementations.iter().map(String::as_str).collect()
    }

    /// #264: an enum's values are lowered in document order, each with its raw
    /// `Implementation` property value; a value with no body property has none.
    #[test]
    fn enum_values_are_lowered_with_implementations() {
        let src = r#"
enum 50102 "K" implements "I", "J"
{
    value(0; A) { Implementation = "I" = "Impl A", "J" = "Impl J"; }
    value(1; "B C") { }
}
"#;
        let af = parse(src);
        assert_eq!(af.parse_status, crate::ir::ParseStatus::Clean);
        let values = &af.objects[0].enum_values;
        assert_eq!(values.len(), 2);
        assert_eq!(values[0].name, "A");
        assert_eq!(
            enum_impls(&values[0]),
            [r#""I" = "Impl A", "J" = "Impl J""#]
        );
        assert_eq!(values[1].name, "B C");
        assert!(values[1].implementations.is_empty());
    }

    /// #264: an `#if` around a value's `Implementation` gives one entry per arm.
    #[test]
    fn enum_value_implementation_if_arms_give_one_entry_per_arm() {
        let src = r#"
enum 50102 K implements I
{
    value(0; A)
    {
#if X
        Implementation = "I" = "P";
#else
        Implementation = "I" = "Q";
#endif
    }
}
"#;
        let af = parse(src);
        assert_eq!(af.parse_status, crate::ir::ParseStatus::Clean);
        let values = &af.objects[0].enum_values;
        assert_eq!(values.len(), 1);
        assert_eq!(enum_impls(&values[0]), [r#""I" = "P""#, r#""I" = "Q""#]);
    }

    /// #264 (Review Focus 5): a value declared only inside an `#if` wrapper is
    /// still lowered, with its implementation.
    #[test]
    fn enum_value_inside_if_wrapper_is_lowered() {
        let src = r#"
enum 50102 K implements I
{
    value(0; A) { Implementation = "I" = "O"; }
#if CLEAN27
    value(1; X) { Implementation = "I" = "P"; }
#endif
}
"#;
        let af = parse(src);
        assert_eq!(af.parse_status, crate::ir::ParseStatus::Clean);
        let values = &af.objects[0].enum_values;
        let names: Vec<&str> = values.iter().map(|v| v.name.as_str()).collect();
        assert_eq!(names, ["A", "X"]);
        assert_eq!(enum_impls(&values[1]), [r#""I" = "P""#]);
    }

    /// #264: only Enum objects carry enum values. The enumextension holds a
    /// `value` declaration, so it is the case that tells the kind guard apart.
    #[test]
    fn non_enum_objects_have_no_enum_values() {
        let src = r#"
codeunit 50100 C
{
}

table 50101 T
{
    fields
    {
        field(1; A; Integer) { }
    }
}

enumextension 50103 E extends K
{
    value(5; Z) { Implementation = "I" = "P"; }
}
"#;
        let af = parse(src);
        assert_eq!(af.parse_status, crate::ir::ParseStatus::Clean);
        assert_eq!(af.objects.len(), 3);
        assert!(af.objects.iter().all(|o| o.enum_values.is_empty()));
    }
}

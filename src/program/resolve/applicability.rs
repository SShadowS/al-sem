//! Route-level APPLICABILITY predicates for Phase 4 fan-out resolution.
//!
//! A WITNESS proves a route's target EXISTS; it does NOT prove the call SITE
//! dispatches to it.  A fresh-only fan-out route is justified (`fresh_ahead_*`)
//! ONLY when a route-level, SITE-CONTEXTUAL applicability predicate passes;
//! FAIL → `unverified_extra` (a real false edge).
//!
//! Three predicates are provided:
//! - [`interface_route_applicable`] — validates an interface-dispatch fan-out
//!   route against the call site's interface name, called member, and arity.
//! - [`implicit_trigger_route_applicable`] — validates an implicit-trigger
//!   fan-out route (OnInsert / OnModify / OnDelete / OnRename / OnValidate)
//!   against the record-op context.
//! - [`instance_builtin_route_applicable`] — validates that a method name is in
//!   THAT object-kind's instance-builtin catalog (kind-uniform, no per-site state).
//!
//! Clean-room: this module does NOT import from L3 logic.

use al_syntax::IdentifierFoldExt;
use al_syntax::ir::ObjectKind;

use crate::program::graph::ProgramGraph;
use crate::program::node::{ObjKey, ObjectNodeId, RoutineNodeId};
use crate::program::resolve::index::ResolveIndex;
use crate::program::resolve::member_catalog::{MemberCatalogKind, member_builtin};
use crate::program::resolve::receiver::FrameworkKind;

// ---------------------------------------------------------------------------
// Types for RecordOpCtx
// ---------------------------------------------------------------------------

/// Lowercased field name used as the field identity for a `Validate` trigger.
///
/// When the call site is `Rec.Validate(FieldName)`, the field name is lowercased
/// and stored here.  The corresponding `RoutineNodeId.enclosing_member_lc` on a
/// field-level `OnValidate` trigger carries the same lowercased field name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldRef(pub String);

/// The specific record-database operation kind at a call site.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecordOpKind {
    Insert,
    Modify,
    Delete,
    Rename,
    Validate,
}

/// Whether the record operation's run-trigger argument is `true`, `false`, or
/// conditionally guarded at the call site.
///
/// `False` suppresses all trigger edges unconditionally.
/// `True` fires triggers unconditionally.
/// `Guarded` fires triggers conditionally; the route is emitted with a
/// `RunTriggerGuarded` condition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunTrigger {
    True,
    False,
    Guarded,
}

/// Site-contextual description of a record-operation implicit-trigger dispatch.
///
/// This struct captures the CALL-SITE context needed by
/// [`implicit_trigger_route_applicable`] to validate that a proposed fan-out
/// route to a trigger really does fire for this particular operation and field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordOpCtx {
    /// The database operation kind (Insert / Modify / Delete / Rename / Validate).
    pub kind: RecordOpKind,
    /// The table node on which the operation is being called.
    pub table: ObjectNodeId,
    /// For `Validate`: the specific field being validated (lowercased name).
    /// `None` for Insert / Modify / Delete / Rename.
    pub field: Option<FieldRef>,
    /// Whether the run-trigger flag is statically true, false, or guarded.
    pub run_trigger: RunTrigger,
}

// ---------------------------------------------------------------------------
// Predicate: interface_route_applicable
// ---------------------------------------------------------------------------

/// Returns `true` iff a fresh fan-out route to `target` is applicable for a
/// call site that dispatches via interface `iface_lc` calling member
/// `called_member_lc` with arity `called_arity`.
///
/// # Conditions (all must hold)
/// 1. `target.name_lc == called_member_lc` — the route targets the right method.
/// 2. `target.params_count == called_arity` — arity matches the call site.
/// 3. The target's OBJECT implements `iface_lc` (exact case-insensitive match via
///    `ObjectNode.implements`, already lowercased).
/// 4. The match is UNAMBIGUOUS: exactly one routine in the target object has
///    `(name_lc == called_member_lc, params_count == called_arity)`.  Multiple
///    → `false` (caller emits Unresolved rather than a false-confident route).
///
/// AL has no explicit method-level interface wiring; the contract is an implicit
/// public-signature match.  Same-name-same-arity type-disambiguation is deferred
/// to Phase 1B.3 → those punt to `false` here.
pub fn interface_route_applicable(
    iface_lc: &str,
    called_member_lc: &str,
    called_arity: usize,
    target: &RoutineNodeId,
    graph: &ProgramGraph,
    index: &ResolveIndex,
) -> bool {
    // 1+2. Name + arity must match the call site.
    if target.name_lc != called_member_lc {
        return false;
    }
    if target.params_count != called_arity {
        return false;
    }

    // 3. Target's object must implement the interface.
    let Some(obj_node) = graph.objects.iter().find(|o| o.id == target.object) else {
        return false;
    };
    let implements_iface = obj_node
        .implements
        .iter()
        .any(|s| s.fold_identifier() == iface_lc);
    if !implements_iface {
        return false;
    }

    // 4. Unambiguous: exactly one routine in this object matches (name, arity).
    let matching_arity = index
        .routines_in_object(graph, &target.object, called_member_lc)
        .filter(|r| r.params_count == called_arity)
        .count();
    matching_arity == 1
}

// ---------------------------------------------------------------------------
// Site rule: which implicit-trigger routes a record-operation site can fire
// ---------------------------------------------------------------------------

/// The site rules of one record operation's implicit-trigger dispatch, read from
/// its arguments (engine-switch S3.4: the resolver applies them, so the fan-out
/// it emits is what can fire; the B3 adapter used to filter after the fact).
///
/// - `run_trigger`: the RunTrigger argument of `Insert` / `Modify` / `Delete` /
///   `DeleteAll` (argument 0) and `ModifyAll` (argument 2). A boolean literal is
///   `Some(b)`; a MISSING argument is `Some(false)`: measured on BC 28 (Cronus28,
///   2026-10-07), `Insert()`, `Modify()`, `Delete()`, `ModifyAll(F, V)` and
///   `DeleteAll()` run no table trigger, and `ModifyAll(F, V, true)` /
///   `DeleteAll(true)` run it once per row. Any other expression is `None`
///   (may fire). `Rename` and `Validate` take no RunTrigger and always fire.
///   The body pipeline's `run_trigger` field stays literal-only: d29 reads it
///   for the platform's database events, which are a different question.
/// - `validate_field`: `Validate`'s first argument's source text, quotes
///   stripped, doubled quotes collapsed, folded (`implicit_edges`'
///   `normalize_field_name`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TriggerSiteRule {
    pub is_validate: bool,
    pub run_trigger: Option<bool>,
    pub validate_field: Option<String>,
}

impl TriggerSiteRule {
    /// Read the rule for the operation `op_lc` (folded) from its arguments.
    #[must_use]
    pub fn of(
        op_lc: &str,
        args: &[al_syntax::ir::ExprId],
        file: &al_syntax::ir::AlFile,
        text: &str,
    ) -> Self {
        use al_syntax::ir::{ExprKind, Literal};
        let slot = match op_lc {
            "insert" | "modify" | "delete" | "deleteall" => Some(0),
            "modifyall" => Some(2),
            _ => None,
        };
        let run_trigger = slot.and_then(|i| match args.get(i) {
            None => Some(false),
            Some(&a) => match &file.ir.expr(a).kind {
                ExprKind::Literal(Literal::Bool(b)) => Some(*b),
                _ => None,
            },
        });
        let is_validate = op_lc == "validate";
        let validate_field = if is_validate {
            args.first()
                .and_then(|&a| text.get(file.ir.expr(a).origin.byte.clone()))
                .map(|raw| {
                    crate::program::body::node_util::strip_quotes(raw)
                        .replace("\"\"", "\"")
                        .fold_identifier()
                })
        } else {
            None
        };
        TriggerSiteRule {
            is_validate,
            run_trigger,
            validate_field,
        }
    }

    /// Can this site fire the trigger routine `target`? `RunTrigger = false`
    /// (a literal, or the argument left out) fires nothing; a `Validate` fires only its own
    /// field's `OnValidate`.
    #[must_use]
    pub fn admits(&self, target: &RoutineNodeId) -> bool {
        self.run_trigger != Some(false)
            && (!self.is_validate
                || (self.validate_field.is_some()
                    && target.enclosing_member_lc == self.validate_field))
    }
}

// ---------------------------------------------------------------------------
// Predicate: implicit_trigger_route_applicable
// ---------------------------------------------------------------------------

/// Returns `true` iff a fresh fan-out route to `target` is applicable for the
/// record-operation implicit-trigger dispatch described by `ctx`.
///
/// # Rules
/// - `ctx.run_trigger == False` → ALWAYS `false` (no trigger fires for
///   `Insert(false)`, `Modify(false)`, etc.).
/// - The target's `name_lc` must match the correct trigger name for `ctx.kind`:
///   - `Insert`   → `oninsert`   (object-level trigger; `enclosing_member_lc` is `None`)
///   - `Modify`   → `onmodify`   (same)
///   - `Delete`   → `ondelete`   (same)
///   - `Rename`   → `onrename`   (same)
///   - `Validate` → `onvalidate` AND `target.enclosing_member_lc` must equal
///     `ctx.field` (the SPECIFIC field being validated — a route to a different
///     field's `OnValidate` is NOT applicable).
/// - The target's object must be `ctx.table` (the base table) OR a
///   `TableExtension` of it (resolved via `index.table_extensions_of`).
pub fn implicit_trigger_route_applicable(
    ctx: &RecordOpCtx,
    target: &RoutineNodeId,
    graph: &ProgramGraph,
    index: &ResolveIndex,
) -> bool {
    // run_trigger == False → never fires any trigger.
    if ctx.run_trigger == RunTrigger::False {
        return false;
    }

    // Trigger name + field specificity check.
    let trigger_name_ok = match &ctx.kind {
        RecordOpKind::Insert => {
            target.name_lc == "oninsert" && target.enclosing_member_lc.is_none()
        }
        RecordOpKind::Modify => {
            target.name_lc == "onmodify" && target.enclosing_member_lc.is_none()
        }
        RecordOpKind::Delete => {
            target.name_lc == "ondelete" && target.enclosing_member_lc.is_none()
        }
        RecordOpKind::Rename => {
            target.name_lc == "onrename" && target.enclosing_member_lc.is_none()
        }
        RecordOpKind::Validate => {
            if target.name_lc != "onvalidate" {
                return false;
            }
            // Must target the SPECIFIC field's OnValidate.
            match (&ctx.field, &target.enclosing_member_lc) {
                (Some(FieldRef(field_lc)), Some(enc_lc)) => field_lc == enc_lc,
                // ctx has no field (shouldn't happen for Validate) or target
                // has no enclosing_member (an object-level trigger, not a field one).
                _ => false,
            }
        }
    };
    if !trigger_name_ok {
        return false;
    }

    // Object identity: target must be on ctx.table or a TableExtension of it.
    if target.object == ctx.table {
        return true;
    }

    // Look up the base table's lowercased name for the extension index.
    let table_name_lc: String = match &ctx.table.key {
        ObjKey::Name(s) => s.clone(),
        ObjKey::Id(_) => {
            // Resolve the name from the graph (needed when the table is id-keyed).
            graph
                .objects
                .iter()
                .find(|o| o.id == ctx.table)
                .map(|n| n.name.fold_identifier())
                .unwrap_or_default()
        }
    };

    if table_name_lc.is_empty() {
        return false;
    }

    index
        .table_extensions_of(&table_name_lc)
        .contains(&target.object)
}

// ---------------------------------------------------------------------------
// Predicate: instance_builtin_route_applicable
// ---------------------------------------------------------------------------

/// Returns `true` iff `method_lc` (already lowercased) is a known
/// instance-builtin method for the given object `kind`.
///
/// Delegates to the `member_builtin` catalog (Phase 3 clean-room catalog):
/// - `Page`   → `PAGE_INSTANCE` catalog via
///   `MemberCatalogKind::Framework(FrameworkKind::PageInstance)`.
/// - `Report` → `REPORT_INSTANCE` catalog via
///   `MemberCatalogKind::Framework(FrameworkKind::ReportInstance)`.
/// - All other kinds → `false`.
///
/// This predicate is kind-uniform (no per-object-instance data), which covers
/// every catalog member uniformly — `RunModal`/`Run`/`Close`/`SaveAsPdf`-class
/// category methods AND `SetRecord`/`SetTableView`/`GetRecord`/
/// `SetSelectionFilter`-class methods alike (argtype-dispatch-and-page-catalog
/// plan, Task 1: these are real, unconditional platform intrinsics — see
/// `resolver::is_metadata_sensitive_instance_method`'s doc for the
/// provenance). This predicate only checks CATALOG MEMBERSHIP of an already-
/// emitted route's method name (the `semantic_golden` applicability teeth's
/// soundness check) — it does not decide which methods `resolve_member`
/// emits a route for; that decision (the `SaveRecord`-only CurrPage
/// exclusion) lives entirely in `resolver::is_metadata_sensitive_instance_
/// method`, consulted only by the `Object{kind}` catalog-fallback arm.
pub fn instance_builtin_route_applicable(kind: ObjectKind, method_lc: &str) -> bool {
    match kind {
        ObjectKind::Page => member_builtin(
            MemberCatalogKind::Framework(&FrameworkKind::PageInstance),
            method_lc,
        ),
        ObjectKind::Report => member_builtin(
            MemberCatalogKind::Framework(&FrameworkKind::ReportInstance),
            method_lc,
        ),
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    use crate::program::graph::{ObjectIndex, ProgramGraph};
    use crate::program::node::{AppRef, AppRegistry, ObjKey, ObjectNodeId, RoutineNodeId};
    use crate::program::node_extract::{AbiParams, Access, ObjectNode, RoutineNode};
    use crate::program::topology::DependencyGraph;
    use crate::snapshot::{AppId, TrustTier};
    use al_syntax::ir::ObjectKind;

    // -------------------------------------------------------------------------
    // Fixture helpers
    // -------------------------------------------------------------------------

    fn make_app() -> (AppRegistry, AppRef) {
        let mut apps = AppRegistry::default();
        let r = apps.intern(&AppId {
            guid: String::new(),
            name: "TestApp".into(),
            publisher: "T".into(),
            version: "1.0.0.0".into(),
        });
        (apps, r)
    }

    fn make_obj(
        app: AppRef,
        kind: ObjectKind,
        name: &str,
        implements: Vec<&str>,
        extends_target: Option<&str>,
    ) -> ObjectNode {
        ObjectNode {
            id: ObjectNodeId {
                app,
                kind,
                key: ObjKey::Name(name.fold_identifier()),
            },
            name: name.to_string(),
            declared_id: None,
            extends_target: extends_target.map(str::to_string),
            implements: implements.into_iter().map(str::to_string).collect(),
            tier: TrustTier::Workspace,
            source_table: None,
            table_no: None,
            source_table_temporary: false,
            page_controls: vec![],
            fields: vec![],
            dataitems: vec![],
            query_columns: Vec::new(),
            protected_vars: Vec::new(),
            parse_incomplete: false,
        }
    }

    fn make_routine(
        obj_id: &ObjectNodeId,
        name: &str,
        params: usize,
        enclosing: Option<&str>,
    ) -> RoutineNode {
        RoutineNode {
            id: RoutineNodeId {
                object: obj_id.clone(),
                name_lc: name.fold_identifier(),
                enclosing_member_lc: enclosing.map(|s| s.fold_identifier()),
                params_count: params,
                sig_fp: 0,
            },
            name: name.to_string(),
            is_trigger: enclosing.is_some()
                || matches!(
                    name.fold_identifier().as_str(),
                    "oninsert" | "onmodify" | "ondelete" | "onrename" | "onvalidate"
                ),
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

    fn build_graph(
        apps: AppRegistry,
        objects: Vec<ObjectNode>,
        routines: Vec<RoutineNode>,
    ) -> (ProgramGraph, ResolveIndex) {
        let mut sorted_objects = objects;
        sorted_objects.sort_by(|a, b| a.id.cmp(&b.id));
        let obj_index = ObjectIndex::build(&sorted_objects);
        let graph = ProgramGraph {
            apps,
            topology: DependencyGraph::default(),
            objects: sorted_objects.into(),
            routines: routines
                .iter()
                .map(RoutineNode::clone)
                .collect::<Vec<_>>()
                .into(),
            obj_index,
            ..Default::default()
        };
        let index = ResolveIndex::build(&graph);
        (graph, index)
    }

    // -------------------------------------------------------------------------
    // Interface-route fixture
    // -------------------------------------------------------------------------

    /// Builds a minimal fixture:
    /// - Interface "IFoo"
    /// - Codeunit "FooImpl" implements IFoo, declares `Bar()` (0 params) and
    ///   `Baz(x)` (1 param).
    /// - Codeunit "Other" does NOT implement IFoo.
    fn iface_fixture() -> (ProgramGraph, ResolveIndex, ObjectNodeId, ObjectNodeId) {
        let (apps, app) = make_app();

        let iface_obj = make_obj(app, ObjectKind::Interface, "IFoo", vec![], None);
        let impl_obj = make_obj(app, ObjectKind::Codeunit, "FooImpl", vec!["IFoo"], None);
        let other_obj = make_obj(app, ObjectKind::Codeunit, "Other", vec![], None);

        let iface_id = iface_obj.id.clone();
        let impl_id = impl_obj.id.clone();

        let bar = make_routine(&impl_id, "bar", 0, None);
        let baz = make_routine(&impl_id, "baz", 1, None);

        let objects = vec![iface_obj, impl_obj, other_obj];
        let routines = vec![bar, baz];
        let (graph, index) = build_graph(apps, objects, routines);
        (graph, index, iface_id, impl_id)
    }

    // -------------------------------------------------------------------------
    // interface_route_applicable — positive
    // -------------------------------------------------------------------------

    #[test]
    fn interface_applicable_happy_path_bar() {
        let (graph, index, _iface_id, impl_id) = iface_fixture();
        let target = RoutineNodeId {
            object: impl_id,
            name_lc: "bar".into(),
            enclosing_member_lc: None,
            params_count: 0,
            sig_fp: 0,
        };
        assert!(
            interface_route_applicable("ifoo", "bar", 0, &target, &graph, &index),
            "FooImpl implements IFoo and has a unique Bar() → applicable"
        );
    }

    // -------------------------------------------------------------------------
    // interface_route_applicable — negative (the brief's mandatory negatives)
    // -------------------------------------------------------------------------

    /// Critical site-context test (gpt flagged): a UNIQUE Baz() route for a Bar()
    /// call is NOT applicable — the method name does not match the call site.
    #[test]
    fn interface_applicable_baz_route_for_bar_call_is_false() {
        let (graph, index, _iface_id, impl_id) = iface_fixture();
        // A route pointing at Baz (1-param) being considered for a call to Bar(0).
        let target = RoutineNodeId {
            object: impl_id,
            name_lc: "baz".into(),
            enclosing_member_lc: None,
            params_count: 1,
            sig_fp: 0,
        };
        assert!(
            !interface_route_applicable("ifoo", "bar", 0, &target, &graph, &index),
            "Baz() route for Bar() call must be false — name mismatch (site-context guard)"
        );
    }

    #[test]
    fn interface_applicable_object_does_not_implement_iface_is_false() {
        let (graph, index, _iface_id, _impl_id) = iface_fixture();
        let other_id = graph
            .objects
            .iter()
            .find(|o| o.name == "Other")
            .unwrap()
            .id
            .clone();
        let target = RoutineNodeId {
            object: other_id,
            name_lc: "bar".into(),
            enclosing_member_lc: None,
            params_count: 0,
            sig_fp: 0,
        };
        assert!(
            !interface_route_applicable("ifoo", "bar", 0, &target, &graph, &index),
            "Object does not implement IFoo → not applicable"
        );
    }

    #[test]
    fn interface_applicable_ambiguous_two_same_name_same_arity_is_false() {
        // Two routines with identical (name_lc, params_count) in the same object.
        let (apps, app) = make_app();
        let impl_obj = make_obj(app, ObjectKind::Codeunit, "FooImpl", vec!["IFoo"], None);
        let impl_id = impl_obj.id.clone();

        let bar1 = make_routine(&impl_id, "bar", 0, None);
        let bar2 = make_routine(&impl_id, "bar", 0, None); // exact duplicate

        let (graph, index) = build_graph(apps, vec![impl_obj], vec![bar1, bar2]);

        let target = RoutineNodeId {
            object: impl_id,
            name_lc: "bar".into(),
            enclosing_member_lc: None,
            params_count: 0,
            sig_fp: 0,
        };
        assert!(
            !interface_route_applicable("ifoo", "bar", 0, &target, &graph, &index),
            "Ambiguous (two Bar() in implementer) → not applicable"
        );
    }

    // -------------------------------------------------------------------------
    // Task 3 (preprocessor foundations plan): the `implements` union-is-sound
    // design decision. A `#if`-conditional `implements` clause (only ever
    // reachable via a whole-object `preproc_split_declaration` header split —
    // see `al_syntax::lower::extract_implements`'s doc) now unions BOTH
    // branches' interface names into `ObjectDecl.implements`/`ObjectNode.
    // implements`, even when they name DIFFERENT (conflicting) interfaces —
    // unlike a singular property (`SourceTable`/`TableNo`), this is
    // INTENTIONALLY NEVER degraded to "no confident value": every consumer of
    // `implements` (this function) only ever asks "MIGHT this object
    // implement `iface`?" for additive may-fire fan-out, never "which ONE
    // interface does it implement?" — so unioning two conditional branches
    // together only WIDENS the implementer set (a sound over-approximation:
    // one branch is always dead at compile time, but nothing here ever
    // fabricates a false SINGLE-target confidence the way a wrongly-picked
    // SourceTable would). This test proves the sound-without-degrade claim:
    // an object unioning `["IThing", "IOther"]` from conflicting `#if`
    // branches is a valid fan-out route target for EITHER interface.
    // -------------------------------------------------------------------------

    #[test]
    fn interface_applicable_conflicting_preproc_union_is_sound_for_both_interfaces() {
        let (apps, app) = make_app();

        let ithing = make_obj(app, ObjectKind::Interface, "IThing", vec![], None);
        let iother = make_obj(app, ObjectKind::Interface, "IOther", vec![], None);
        // Simulates the POST-fix union-read outcome of a `#if COND codeunit 1
        // X implements IThing #else codeunit 1 X implements IOther #endif`
        // split declaration: BOTH conditional interface names land in one
        // `implements` list, never just the first/last branch.
        let impl_obj = make_obj(
            app,
            ObjectKind::Codeunit,
            "SplitImpl",
            vec!["IThing", "IOther"],
            None,
        );
        let impl_id = impl_obj.id.clone();
        let run = make_routine(&impl_id, "run", 0, None);

        let (graph, index) = build_graph(apps, vec![ithing, iother, impl_obj], vec![run]);

        let target = RoutineNodeId {
            object: impl_id,
            name_lc: "run".into(),
            enclosing_member_lc: None,
            params_count: 0,
            sig_fp: 0,
        };
        assert!(
            interface_route_applicable("ithing", "run", 0, &target, &graph, &index),
            "the union must still route-apply for IThing (one of the two \
             conditional branches)"
        );
        assert!(
            interface_route_applicable("iother", "run", 0, &target, &graph, &index),
            "the union must ALSO still route-apply for IOther (the OTHER \
             conditional branch) — no false narrowing to only the first \
             branch captured"
        );
    }

    // -------------------------------------------------------------------------
    // implicit_trigger_route_applicable fixture
    // -------------------------------------------------------------------------

    /// Builds a trigger fixture:
    /// - Table "Customer" with OnInsert, OnModify, OnValidate(Name), OnValidate(No.)
    /// - TableExtension "CustomerExt" extending "Customer" with OnInsert
    /// - Table "Vendor" (unrelated)
    fn trigger_fixture() -> (ProgramGraph, ResolveIndex, ObjectNodeId, ObjectNodeId) {
        let (apps, app) = make_app();

        let table = make_obj(app, ObjectKind::Table, "Customer", vec![], None);
        let table_ext = make_obj(
            app,
            ObjectKind::TableExtension,
            "CustomerExt",
            vec![],
            Some("Customer"),
        );
        let unrelated = make_obj(app, ObjectKind::Table, "Vendor", vec![], None);

        let table_id = table.id.clone();
        let ext_id = table_ext.id.clone();

        let oninsert = make_routine(&table_id, "oninsert", 0, None);
        let onmodify = make_routine(&table_id, "onmodify", 0, None);
        let onvalidate_name = make_routine(&table_id, "onvalidate", 0, Some("Name"));
        let onvalidate_no = make_routine(&table_id, "onvalidate", 0, Some("No."));
        let ext_oninsert = make_routine(&ext_id, "oninsert", 0, None);

        let objects = vec![table, table_ext, unrelated];
        let routines = vec![
            oninsert,
            onmodify,
            onvalidate_name,
            onvalidate_no,
            ext_oninsert,
        ];
        let (graph, index) = build_graph(apps, objects, routines);
        (graph, index, table_id, ext_id)
    }

    // -------------------------------------------------------------------------
    // implicit_trigger_route_applicable — positive
    // -------------------------------------------------------------------------

    #[test]
    fn trigger_applicable_insert_base_table_true() {
        let (graph, index, table_id, _ext_id) = trigger_fixture();
        let ctx = RecordOpCtx {
            kind: RecordOpKind::Insert,
            table: table_id.clone(),
            field: None,
            run_trigger: RunTrigger::True,
        };
        let target = RoutineNodeId {
            object: table_id,
            name_lc: "oninsert".into(),
            enclosing_member_lc: None,
            params_count: 0,
            sig_fp: 0,
        };
        assert!(implicit_trigger_route_applicable(
            &ctx, &target, &graph, &index
        ));
    }

    #[test]
    fn trigger_applicable_insert_table_extension_true() {
        let (graph, index, table_id, ext_id) = trigger_fixture();
        let ctx = RecordOpCtx {
            kind: RecordOpKind::Insert,
            table: table_id,
            field: None,
            run_trigger: RunTrigger::True,
        };
        let target = RoutineNodeId {
            object: ext_id,
            name_lc: "oninsert".into(),
            enclosing_member_lc: None,
            params_count: 0,
            sig_fp: 0,
        };
        assert!(
            implicit_trigger_route_applicable(&ctx, &target, &graph, &index),
            "TableExtension of Customer also fires OnInsert"
        );
    }

    #[test]
    fn trigger_applicable_validate_correct_field_no_true() {
        let (graph, index, table_id, _ext_id) = trigger_fixture();
        let ctx = RecordOpCtx {
            kind: RecordOpKind::Validate,
            table: table_id.clone(),
            field: Some(FieldRef("no.".into())),
            run_trigger: RunTrigger::True,
        };
        let target = RoutineNodeId {
            object: table_id,
            name_lc: "onvalidate".into(),
            enclosing_member_lc: Some("no.".into()),
            params_count: 0,
            sig_fp: 0,
        };
        assert!(implicit_trigger_route_applicable(
            &ctx, &target, &graph, &index
        ));
    }

    // -------------------------------------------------------------------------
    // implicit_trigger_route_applicable — negative (brief's mandatory negatives)
    // -------------------------------------------------------------------------

    /// `Insert(false)` must never fire triggers regardless of the trigger target.
    #[test]
    fn trigger_applicable_insert_false_always_false() {
        let (graph, index, table_id, _ext_id) = trigger_fixture();
        let ctx = RecordOpCtx {
            kind: RecordOpKind::Insert,
            table: table_id.clone(),
            field: None,
            run_trigger: RunTrigger::False, // <— the critical flag
        };
        let target = RoutineNodeId {
            object: table_id,
            name_lc: "oninsert".into(),
            enclosing_member_lc: None,
            params_count: 0,
            sig_fp: 0,
        };
        assert!(
            !implicit_trigger_route_applicable(&ctx, &target, &graph, &index),
            "Insert(false) must never emit a trigger edge"
        );
    }

    /// Wrong trigger name: OnModify target for an Insert context.
    #[test]
    fn trigger_applicable_wrong_trigger_name_onmodify_for_insert_is_false() {
        let (graph, index, table_id, _ext_id) = trigger_fixture();
        let ctx = RecordOpCtx {
            kind: RecordOpKind::Insert,
            table: table_id.clone(),
            field: None,
            run_trigger: RunTrigger::True,
        };
        let target = RoutineNodeId {
            object: table_id,
            name_lc: "onmodify".into(), // <— wrong trigger
            enclosing_member_lc: None,
            params_count: 0,
            sig_fp: 0,
        };
        assert!(
            !implicit_trigger_route_applicable(&ctx, &target, &graph, &index),
            "OnModify target for Insert context must be false"
        );
    }

    /// Validate with the WRONG field: ctx.field is "No." but target is Name's OnValidate.
    #[test]
    fn trigger_applicable_validate_wrong_field_name_for_no_is_false() {
        let (graph, index, table_id, _ext_id) = trigger_fixture();
        let ctx = RecordOpCtx {
            kind: RecordOpKind::Validate,
            table: table_id.clone(),
            field: Some(FieldRef("no.".into())),
            run_trigger: RunTrigger::True,
        };
        // Target is the Name field's OnValidate, but ctx says No. is being validated.
        let target = RoutineNodeId {
            object: table_id,
            name_lc: "onvalidate".into(),
            enclosing_member_lc: Some("name".into()), // <— different field
            params_count: 0,
            sig_fp: 0,
        };
        assert!(
            !implicit_trigger_route_applicable(&ctx, &target, &graph, &index),
            "Validate(No.) must NOT fire Name's OnValidate"
        );
    }

    /// Trigger on an unrelated table (Vendor) must not fire for Customer Insert.
    #[test]
    fn trigger_applicable_unrelated_table_is_false() {
        let (graph, index, table_id, _ext_id) = trigger_fixture();
        let vendor_id = graph
            .objects
            .iter()
            .find(|o| o.name == "Vendor")
            .unwrap()
            .id
            .clone();
        let ctx = RecordOpCtx {
            kind: RecordOpKind::Insert,
            table: table_id, // Customer
            field: None,
            run_trigger: RunTrigger::True,
        };
        let target = RoutineNodeId {
            object: vendor_id, // Vendor — unrelated
            name_lc: "oninsert".into(),
            enclosing_member_lc: None,
            params_count: 0,
            sig_fp: 0,
        };
        assert!(
            !implicit_trigger_route_applicable(&ctx, &target, &graph, &index),
            "Vendor's OnInsert must not fire for Customer.Insert"
        );
    }

    // -------------------------------------------------------------------------
    // instance_builtin_route_applicable (brief's mandatory negatives + positive)
    // -------------------------------------------------------------------------

    #[test]
    fn instance_builtin_page_runmodal_is_true() {
        assert!(
            instance_builtin_route_applicable(ObjectKind::Page, "runmodal"),
            "(Page, runmodal) must be in PAGE_INSTANCE catalog"
        );
    }

    #[test]
    fn instance_builtin_page_unknown_method_is_false() {
        assert!(
            !instance_builtin_route_applicable(ObjectKind::Page, "notamethod"),
            "(Page, notamethod) must NOT be in the catalog"
        );
    }

    /// Critical: `RunModal` on Codeunit is NOT an instance-builtin (kind mismatch).
    #[test]
    fn instance_builtin_codeunit_runmodal_is_false() {
        assert!(
            !instance_builtin_route_applicable(ObjectKind::Codeunit, "runmodal"),
            "(Codeunit, runmodal) must be false — Codeunit has no PAGE_INSTANCE catalog"
        );
    }
}

#[cfg(test)]
mod trigger_site_rule_tests {
    use crate::program::resolve::edge::{EdgeKind, RouteTarget};

    /// S3.4 + S9.0c, pinned at the resolver's USE (a full program build, not the
    /// rule function alone): a table with `OnInsert`/`OnModify`/`OnDelete` and two
    /// fields with `OnValidate`. Each case is one procedure's single record op and
    /// the triggers its edge may carry. Measured on BC 28 (Cronus28, 2026-10-07):
    /// a write without the RunTrigger argument runs no trigger, and
    /// `ModifyAll(.., true)` / `DeleteAll(true)` run it per row.
    ///
    /// Discrimination (2026-10-07): mapping a missing argument back to `None`
    /// fails `insert`, `modify`, `delete`, `deleteall` and `modifyall` (they
    /// fire again); dropping `modifyall`/`deleteall` from
    /// `resolve_implicit_trigger` fails `modifyall-true` and `deleteall-true`;
    /// restored, all pass.
    #[test]
    fn implicit_trigger_edges_carry_only_what_the_site_can_fire() {
        let root = std::env::temp_dir().join(format!("s34-trig-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("app.json"),
            r#"{"id":"11111111-0000-0000-0000-0000000003b4","name":"H","publisher":"P","version":"1.0.0.0","dependencies":[]}"#,
        )
        .unwrap();
        std::fs::write(
            root.join("src/T.Table.al"),
            "table 50100 T\n{\n    fields\n    {\n        field(1; A; Code[20]) { trigger OnValidate() begin end; }\n        field(2; B; Code[20]) { trigger OnValidate() begin end; }\n    }\n    trigger OnInsert()\n    begin\n    end;\n    trigger OnModify()\n    begin\n    end;\n    trigger OnDelete()\n    begin\n    end;\n}\n",
        )
        .unwrap();
        let cases: &[(&str, &str, &[&str])] = &[
            ("insert", "R.Insert()", &[]),
            ("insertfalse", "R.Insert(false)", &[]),
            ("inserttrue", "R.Insert(true)", &[":oninsert"]),
            ("modify", "R.Modify()", &[]),
            ("modifyfalse", "R.Modify(false)", &[]),
            ("modifytrue", "R.Modify(true)", &[":onmodify"]),
            ("modifyvar", "R.Modify(X)", &[":onmodify"]),
            ("delete", "R.Delete()", &[]),
            ("deletetrue", "R.Delete(true)", &[":ondelete"]),
            ("modifyall", "R.ModifyAll(A, '')", &[]),
            ("modifyalltrue", "R.ModifyAll(A, '', true)", &[":onmodify"]),
            ("deleteall", "R.DeleteAll()", &[]),
            ("deletealltrue", "R.DeleteAll(true)", &[":ondelete"]),
            ("val", "R.Validate(A)", &["a:onvalidate"]),
        ];
        let body: String = cases
            .iter()
            .map(|(name, stmt, _)| {
                format!("    procedure {name}(X: Boolean)\n    var\n        R: Record T;\n    begin\n        {stmt};\n    end;\n\n")
            })
            .collect();
        std::fs::write(
            root.join("src/C.Codeunit.al"),
            format!("codeunit 50101 C\n{{\n{body}}}\n"),
        )
        .unwrap();
        let (_ctx, report, _) =
            crate::program::resolve::full::build_program_with_coverage(&root).unwrap();
        std::fs::remove_dir_all(&root).ok();
        let routes_of = |caller: &str| -> Vec<String> {
            report
                .edges
                .iter()
                .filter(|ce| {
                    ce.edge.kind == EdgeKind::ImplicitTrigger && ce.edge.from.name_lc == caller
                })
                .flat_map(|e| e.edge.routes.iter())
                .map(|r| match &r.target {
                    RouteTarget::Routine(id) => format!(
                        "{}:{}",
                        id.enclosing_member_lc.clone().unwrap_or_default(),
                        id.name_lc
                    ),
                    other => format!("{other:?}"),
                })
                .collect()
        };
        let wrong: Vec<String> = cases
            .iter()
            .filter_map(|(name, _, want)| {
                let got = routes_of(name);
                (got != *want).then(|| format!("{name}: {got:?} (want {want:?})"))
            })
            .collect();
        assert!(wrong.is_empty(), "cases {wrong:?}");
    }
}

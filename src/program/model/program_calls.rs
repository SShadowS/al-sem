//! B3 Phase A: the production call resolution of `alsem analyze`'s detectors.
//! [`attach_program_calls`] converts the program engine's call-site edges into
//! the model's `ResolvedCalls` and returns the model with them; it started
//! (Task 1) as a read-only join census ([`adapter_census_for_workspace`]).
//!
//! The conversion needs every model call site paired with exactly one program
//! edge. This module does the pairing and counts what does not pair, by
//! reason.
//!
//! # Join keys (code map A3, `docs/2026-10-04-b3-code-map.md`)
//!
//! - **Site**: `(unit, start.line, start.col, end.line, end.col)`. The L3 unit
//!   is `PAnchor::source_unit_id` minus its `"ws:"` prefix; the program unit
//!   is `CanonicalSpan::unit` (the virtual path). Both sides anchor the same
//!   IR node (the `Call` expression origin) and both use 0-based rows and
//!   BYTE columns, so no column conversion is done.
//! - **Callee check**: `callee_fp(PCallSite.callee_text)` must equal the
//!   program site's `callee_fingerprint`.
//! - **Caller check**: the L3 routine's declaration anchor
//!   `(unit, start_line, start_column)` must equal the program caller's
//!   `(virtual_path, origin.start)` from [`DeclSurface::get_with_path`].
//!
//! # Reasons
//!
//! Per L3 call site: `matched`, or unmatched as `no_program_site` (no program
//! edge at that span), `shape_mismatch` (the program sees a record op or
//! `Commit` there), `callee_fp_mismatch`, `caller_mismatch`.
//!
//! Per program workspace call-site edge that no L3 call site took:
//! - a record op (`EdgeKind::ImplicitTrigger`) whose operation can fire a
//!   table trigger (insert/modify/delete/validate/rename):
//!   `implicit_trigger_matched` when an `ModelRecordOperation` sits at the same
//!   span, else `implicit_trigger_unmatched`;
//! - any other record op, or a `Commit`: `operation_site` (no L3 call
//!   analogue; L3 keeps these as operation sites, code map A6);
//! - a call or run with an L3 operation site at the same span:
//!   `op_shape_mismatch` (L3 says record op, the program says call; until
//!   engine-switch S3.5 the bare implicit-`Rec` op, 571 sites on CDO, now 0).
//!   No L3 call site is involved, but the adapter takes implicit-trigger
//!   edges from the program, so these would get none;
//! - otherwise `program_only_site`.
//!
//! Per L3 operation site (not `error-call`, which is also a call site) with no
//! program edge at its span: `l3_op_no_program_site_trigger` for a
//! trigger-capable record op (insert/modify/delete/validate/rename), else
//! `l3_op_no_program_site_other`. `l3_record_operations` and
//! `l3_operation_sites` are the L3 totals.
//!
//! `operation_site` and the two implicit-trigger reasons are expected; they
//! are not call-site join misses.
//!
//! # The adapter (B3 Phase A, Task 3)
//!
//! [`resolved_calls_from_program`] builds L3's `ResolvedCalls` from the
//! program engine's edges, so detectors can run on the program engine's
//! resolution without changing. Stage 1: resolution only. It follows code
//! map B7 row by row:
//!
//! - **Dispatch kind** comes from the L2 callee shape (`Bare` → `Direct`,
//!   `Member` → `Method`, `ObjectRun` → the run kind), never from L3's
//!   resolver. Exceptions that keep L3's shape: an interface edge is
//!   `Interface`, a builtin is `Builtin`, a dynamic edge is `Dynamic`, and
//!   `CuVar.Run()` landing on a codeunit's `OnRun` is `CodeunitRun`.
//! - **Workspace callee**: `to` = the L3 routine with the same declaration
//!   anchor (the census's caller join), `Resolved`; bindings upgraded with
//!   that routine's parameters, as `upgrade_bindings` does.
//! - **Dependency callee** (a routine or ABI symbol outside the workspace
//!   app), `ObjectNotInGraph`, an ABI-collapsed overload, or a member decline
//!   (absent, arity, visibility, ambiguity) on a receiver outside the
//!   workspace: to-less `ExternalTarget` (`Opaque` for a run),
//!   `external_type_ref` from the program object node (or, with no node,
//!   from the L2 receiver type / run target; `None` for an opaque run into a
//!   workspace object). Bindings: stage 2 (`upgrade_dependency_bindings`,
//!   Task 6) upgrades an exact dependency callee's bindings with its
//!   `var`-ness from `DeclSurface` (source dependency) or
//!   `AbiParams::Complete` (symbol-only); `Missing`/`CollapsedUntrusted`,
//!   every other dependency shape, and stage 1 leave them
//!   `"unresolved-callee"`. A dependency
//!   callee never gets a `to`: L3's combined graph holds workspace routines
//!   only. For an OBJECT receiver L3 says `ExternalTarget` too; for a RECORD
//!   receiver (a dependency table procedure) L3 says
//!   `Unknown(RecordTableProcedure)` ("unresolved-call"), so the uncertainty
//!   kind and the confidence cap change. Counted apart
//!   (`adapter_external_record_receiver` / `_object_receiver` / `_other`).
//! - **Interface**: one `Interface`+`Maybe` edge per workspace implementer,
//!   sorted by `to`, `dispatch_meta` on the first (built from L3's symbol
//!   table: the program edge has no interface name); none → one
//!   `Unknown(InterfaceNoImpl)` edge. Dependency routes are dropped and
//!   counted; a workspace implementer with no L3 routine sends the whole site
//!   to the L3 fallback.
//! - **Ambiguous overload**: to-less `Ambiguous`, `candidates` = workspace L3
//!   ids. **Dynamic**: to-less `Dynamic`. **Other unknowns**: [`map_unknown`].
//! - **Implicit triggers**, keyed by `ModelRecordOperation.id`: one edge per
//!   workspace trigger route. The program's fan-out lists every trigger of
//!   the name on the table and its extensions, without the site rules, so
//!   the adapter applies them (`RunTrigger = false` fires nothing; a
//!   `Validate` fires only its field's `OnValidate`). Since engine-switch S9.1
//!   nothing compares them with L3's own trigger logic any more.
//!
//! **No fallback to L3** (engine-switch S3.1, S3.5; controller rulings 1-2
//! as they stood before). An L3 call site the program engine gave no usable
//! edge gets one `Unknown(NoProgramSite)` edge. A trigger-capable record op
//! with no matched program `ImplicitTrigger` edge gets no edge, and is counted
//! in `adapter_l3_trigger_ops`. Since S3.5 the program engine classifies
//! bare implicit-receiver ops as record ops too, so on CDO both counts are 0.
//!
//! **Order** is the legacy resolver's: call sites in routine order, then the
//! trigger edges in routine/op order (`edge_order_is_call_sites_then_triggers`).

use std::collections::{HashMap, HashSet};

use al_syntax::IdentifierFoldExt;
use serde::Serialize;

use super::calls::{
    BindingState, CallEdge, DispatchMeta, ExternalTypeRef, ResolvedCalls,
    UnknownReason as ModelReason, UpgradedBinding, initial_binding_state, mark_bindings_ambiguous,
    object_run_dispatch_kind, upgrade_bindings, upgrade_bindings_with,
};
use super::taxonomy::{DispatchKind, Resolution};
use super::workspace::{ModelEntities, ModelRecordOperation, ModelRoutine};
use crate::program::abi_ingest::object_kind_from_abi_type;
use crate::program::body::features::{PCallSite, PCallee};
use crate::program::graph::ProgramGraph;
use crate::program::node::{AppRef, ObjKey, ObjectNodeId, RoutineNodeId};
use crate::program::node_extract::{AbiParams, ObjectNode};
use crate::program::resolve::decl_surface::DeclSurface;
use crate::program::resolve::edge::{
    DispatchShape, EdgeKind, Evidence, Route, RouteTarget, UnknownReason as PReason, callee_fp,
};
use crate::program::resolve::full::{
    ClassifiedEdge, ObligationId, ProgramContext, ProgramReport, can_fire_trigger,
};
use crate::program::resolved::{EdgeView, ProgramMeta, ProgramScope, ResolvedProgram};
use crate::snapshot::TrustTier;
use al_syntax::ir::ObjectKind;

/// Exact site key: `(unit, start line, start col, end line, end col)`.
type SiteKey = (String, u32, u32, u32, u32);

/// One site that did not pair, with its reason.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UnmatchedSite {
    pub reason: &'static str,
    pub unit: String,
    pub start_line: u32,
    pub start_col: u32,
    pub end_line: u32,
    pub end_col: u32,
    /// The callee text (L3 side) or the site's source text (program side).
    pub text: String,
}

/// Matched and unmatched site counts by reason (see the module doc).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct SiteCensus {
    /// L3 call sites (`ModelRoutine::call_sites`) over the workspace.
    pub l3_call_sites: usize,
    /// Program workspace call-site edges (`ObligationId::CallSite`, primary app).
    pub program_sites: usize,
    pub matched: usize,
    pub no_program_site: usize,
    pub program_only_site: usize,
    pub callee_fp_mismatch: usize,
    pub caller_mismatch: usize,
    /// An L3 call site where the program sees a record op or `Commit`.
    pub shape_mismatch: usize,
    /// An L3 operation site where the program sees a call or run.
    pub op_shape_mismatch: usize,
    pub operation_site: usize,
    pub implicit_trigger_matched: usize,
    pub implicit_trigger_unmatched: usize,
    /// Program edges sharing a span with an earlier one (only the first joins).
    pub duplicate_program_span: usize,
    /// L3 call sites sharing a span with an earlier L3 call site. Each is still
    /// classified (so two can count `matched` against one program edge).
    pub duplicate_l3_span: usize,
    /// L3 record operations (`ModelRoutine::record_operations`).
    pub l3_record_operations: usize,
    /// L3 operation sites other than `error-call` (record ops, locks, commits).
    /// `error-call` sites are also call sites and are counted there.
    pub l3_operation_sites: usize,
    /// L3 trigger-capable record ops (Insert/Modify/Delete/Validate/Rename)
    /// with no program edge at their span.
    pub l3_op_no_program_site_trigger: usize,
    /// Any other L3 operation site (non-trigger record op, lock, commit) with
    /// no program edge at its span.
    pub l3_op_no_program_site_other: usize,
    /// Adapter counts ([`resolved_calls_from_program`]); always 0 from
    /// [`site_census`].
    ///
    /// L3 call sites whose edges came from the program engine.
    pub adapter_program_sites: usize,
    /// L3 call sites the program engine gave no usable edge for: every
    /// unmatched site plus `adapter_callee_outside_l3`. Since engine-switch
    /// S3.1 each gets one `Unknown(NoProgramSite)` edge (it used to fall back to
    /// L3's own resolver). The name is kept for the census's continuity.
    pub adapter_l3_fallback_sites: usize,
    /// Matched call sites whose workspace callee has no L3 routine (no
    /// declaration-anchor join); counted in `adapter_l3_fallback_sites`.
    pub adapter_callee_outside_l3: usize,
    /// L3 record ops whose trigger edges came from the program engine.
    pub adapter_program_trigger_ops: usize,
    /// Trigger-capable L3 record ops (Insert/Modify/Delete/Validate/Rename)
    /// with no matched program `ImplicitTrigger` edge. They get no edge. Until
    /// engine-switch S3.5 they kept L3's own trigger logic (controller ruling
    /// 1); the name is kept for the census's continuity.
    pub adapter_l3_trigger_ops: usize,
    /// Routes the adapter loses: a trigger or ambiguous-overload route that
    /// names no routine (unresolved), or a workspace routine with no L3
    /// routine. Dependency routes are kept since engine-switch S3.2
    /// (interface), S3.6 (trigger, ambiguous overload); see the counters below.
    pub adapter_routes_dropped: usize,
    /// Trigger routes into a dependency table (S3.6): recorded in
    /// `ResolvedCalls::external_targets` under the operation's id, no edge.
    pub adapter_trigger_dependency_routes: usize,
    /// Ambiguous-overload candidates in a dependency (S3.6): recorded in
    /// `ResolvedCalls::external_targets` under the call site's id. The edge
    /// keeps the workspace candidates only, or becomes an external call when
    /// every candidate is in a dependency.
    pub adapter_ambiguous_dependency_candidates: usize,
    /// Dependency implementer objects an interface site reaches (S3.2): one
    /// to-less `ExternalTarget` edge each.
    pub adapter_interface_dependency_impls: usize,
    /// Program trigger routes the model's own record operation says cannot fire
    /// (`RunTrigger = false`, or another field's `OnValidate`). Since engine-switch
    /// S3.4 the program resolver applies these rules itself, so this is an
    /// agreement check (expected 0) and nothing is filtered here.
    pub adapter_trigger_routes_filtered: usize,
    /// Call/run edges with more than one route outside an interface or
    /// ambiguous shape. Since engine-switch S3.6 they convert as an ambiguous
    /// candidate set (every route); they used to keep the first route only.
    pub adapter_multi_route_sites: usize,
    /// Call/run edges with no route at all.
    pub adapter_empty_route_sites: usize,
    /// `ExternalTarget` call edges by the L2 receiver type. A record
    /// receiver's (a dependency table procedure) is `RecordTableProcedure`
    /// ("unresolved-call") in L3, so these change the confidence cap; an
    /// object receiver's is `ExternalTarget` in L3 too. `other`: bare calls
    /// and receivers of any other type.
    pub adapter_external_record_receiver: usize,
    pub adapter_external_object_receiver: usize,
    pub adapter_external_other: usize,
    /// Member declines (absent, arity, visibility, ambiguity) on a receiver
    /// outside the workspace, mapped to `ExternalTarget`/`Opaque` rather
    /// than `MemberNotFound`/`Ambiguous`.
    pub adapter_external_member_decline: usize,
    /// Runs (`Page.Run(Page::X)`, `PageVar.Run()`) of a WORKSPACE object with
    /// no entry trigger: `Opaque` with no external type.
    pub adapter_workspace_run_no_entry: usize,
    /// Stage 2 (`upgrade_dependency_bindings`): call sites into a dependency
    /// routine where at least one binding actually changed (from
    /// `"unresolved-callee"` to `"resolved"`), by where the callee's
    /// `var`-ness came from: a source dependency's declaration
    /// (`DeclSurface`), or a symbol-only routine's `AbiParams::Complete`. A
    /// site whose record arguments all sit past the callee's parameters
    /// (e.g. `Page.Run(Page::X, Rec)` into an `OnOpenPage()`) is not counted.
    pub adapter_dep_bindings_source: usize,
    pub adapter_dep_bindings_symbol: usize,
    /// Call sites into a dependency routine whose parameters are
    /// `AbiParams::Missing` / `CollapsedUntrusted`, with an
    /// `"unresolved-callee"` binding within the routine's arity: left
    /// unchanged, because the var-ness is not trusted.
    pub adapter_dep_bindings_missing: usize,
    pub adapter_dep_bindings_collapsed: usize,
    /// Call sites whose dependency route names no routine at all (the
    /// placeholder ABI key of a run into a dependency object with no entry
    /// trigger) and that have an `"unresolved-callee"` binding: unchanged.
    pub adapter_dep_bindings_no_routine: usize,
    /// Every site counted under a reason other than `matched`,
    /// `operation_site` and `implicit_trigger_matched`, sorted.
    pub unmatched: Vec<UnmatchedSite>,
}

impl SiteCensus {
    /// The non-zero counters that mean a site or route the program engine
    /// resolved did not reach the converted calls whole (engine-switch S3.6,
    /// the site and route accounting gate). Empty when nothing was lost: every
    /// site joined, nothing fell back, no route was cut or dropped.
    /// Dependency routes kept as `external_targets` are not losses.
    pub fn losses(&self) -> Vec<(&'static str, usize)> {
        [
            ("no_program_site", self.no_program_site),
            ("program_only_site", self.program_only_site),
            ("callee_fp_mismatch", self.callee_fp_mismatch),
            ("caller_mismatch", self.caller_mismatch),
            ("shape_mismatch", self.shape_mismatch),
            ("op_shape_mismatch", self.op_shape_mismatch),
            (
                "implicit_trigger_unmatched",
                self.implicit_trigger_unmatched,
            ),
            ("duplicate_program_span", self.duplicate_program_span),
            ("duplicate_l3_span", self.duplicate_l3_span),
            (
                "l3_op_no_program_site_trigger",
                self.l3_op_no_program_site_trigger,
            ),
            (
                "l3_op_no_program_site_other",
                self.l3_op_no_program_site_other,
            ),
            ("adapter_l3_fallback_sites", self.adapter_l3_fallback_sites),
            ("adapter_callee_outside_l3", self.adapter_callee_outside_l3),
            ("adapter_l3_trigger_ops", self.adapter_l3_trigger_ops),
            ("adapter_routes_dropped", self.adapter_routes_dropped),
            (
                "adapter_trigger_routes_filtered",
                self.adapter_trigger_routes_filtered,
            ),
            ("adapter_multi_route_sites", self.adapter_multi_route_sites),
            ("adapter_empty_route_sites", self.adapter_empty_route_sites),
        ]
        .into_iter()
        .filter(|&(_, n)| n > 0)
        .collect()
    }
}

use crate::program::model::site_links::model_key as l3_key;

/// The source text a span covers (byte columns), when the unit is known.
fn span_text<'a>(texts: &HashMap<String, &'a str>, k: &SiteKey) -> Option<&'a str> {
    let text = texts.get(k.0.as_str())?;
    let offset = |line: u32, col: u32| -> Option<usize> {
        let mut start = 0usize;
        for _ in 0..line {
            start += text[start..].find('\n')? + 1;
        }
        Some(start + col as usize)
    };
    text.get(offset(k.1, k.2)?..offset(k.3, k.4)?)
}

/// The operation name of a record-op call's text: `Rec.Insert(true)` →
/// `insert`.
fn op_name(call_text: &str) -> String {
    let callee = call_text.split('(').next().unwrap_or("");
    let last = callee.rsplit('.').next().unwrap_or("").trim();
    last.trim_matches('"').fold_identifier()
}

/// The model's source-unit spelling of program file `path` in `app` (engine-switch
/// S7.3): a primary-app file is its virtual path (the `ws:` prefix dropped, as the
/// span keys drop it); a dependency file is `dep:<guid>:<path>`, as the cross-app
/// model spells it.
pub(crate) fn model_unit(graph: &ProgramGraph, primary: AppRef, app: AppRef, path: &str) -> String {
    if app == primary {
        path.to_string()
    } else {
        format!("dep:{}:{path}", graph.apps.resolve(app).guid)
    }
}

/// The apps whose routines the model holds: the primary app, and in a cross-app
/// model (S7.2) every dependency.
pub(crate) fn model_apps(
    graph: &ProgramGraph,
    primary: AppRef,
    ws: &ModelEntities,
) -> HashSet<AppRef> {
    let guids: HashSet<String> = ws
        .routines
        .iter()
        .map(|r| r.app_guid.to_ascii_lowercase())
        .collect();
    let mut apps: HashSet<AppRef> = graph
        .objects
        .iter()
        .map(|o| o.id.app)
        .filter(|a| guids.contains(&graph.apps.resolve(*a).guid.to_ascii_lowercase()))
        .collect();
    apps.insert(primary);
    apps
}

/// #160 §4.5 (10A-2): whether the join reads the program sites of caller `from`
/// (of a model app): a primary routine, a routine in the demand the model was
/// built from, or any routine when it was built without demand. The predicate
/// `demanded_population` applies to the same node ids.
pub(crate) fn caller_held(
    from: &RoutineNodeId,
    primary: AppRef,
    demand: Option<&HashSet<RoutineNodeId>>,
) -> bool {
    from.object.app == primary || demand.is_none_or(|d| d.contains(from))
}

/// Pair every L3 call site with the program edge at the same exact span and
/// count what does not pair, by reason. `view` must come from
/// `resolve_full_program_with(ctx)` (`ProgramReport::edge_view`, or a
/// `ProgramBody::edge_view` of the same build); `ws` is the L3 model of the same
/// workspace.
#[must_use]
pub fn site_census(view: EdgeView<'_>, ctx: &ProgramContext, ws: &ModelEntities) -> SiteCensus {
    join(view, ctx, ws).census
}

/// The census plus the pairs it found, for the adapter.
struct Join<'a> {
    census: SiteCensus,
    /// Matched L3 call sites: `(routine index, call-site index)` → edge.
    calls: HashMap<(usize, usize), &'a ClassifiedEdge>,
    /// L3 record ops with a program `ImplicitTrigger` edge at their span
    /// (the census's `implicit_trigger_matched`): `(routine index, op
    /// index)` → edge.
    ops: HashMap<(usize, usize), &'a ClassifiedEdge>,
}

fn join<'a>(view: EdgeView<'a>, ctx: &ProgramContext, ws: &ModelEntities) -> Join<'a> {
    span_join(view, ctx, ws, None)
}

/// The span join (J1), reading only the program sites of callers
/// [`caller_held`] admits under `demand`. Production passes `None` (every caller
/// of a model app, as it always read).
fn span_join<'a>(
    view: EdgeView<'a>,
    ctx: &ProgramContext,
    ws: &ModelEntities,
    demand: Option<&HashSet<RoutineNodeId>>,
) -> Join<'a> {
    let mut c = SiteCensus::default();
    let mut matched_calls: HashMap<(usize, usize), &'a ClassifiedEdge> = HashMap::new();
    let mut matched_ops: HashMap<(usize, usize), &'a ClassifiedEdge> = HashMap::new();
    let surface = ctx.decl_surface();
    let graph = ctx.graph();
    let primary = view.primary_app_ref;
    let apps = model_apps(graph, primary, ws);
    // Keyed by model unit (S7.3): dependency files too, when the model holds them.
    let mut texts: HashMap<String, &str> = ctx
        .parsed()
        .iter()
        .flat_map(|u| &u.files)
        .map(|f| (f.virtual_path.clone(), &*f.text))
        .collect();
    for unit in ctx.dep_bodies().unwrap_or_default() {
        let Some(app) = graph.apps.find(&unit.app) else {
            continue;
        };
        if apps.contains(&app) {
            for f in &unit.files {
                texts.insert(model_unit(graph, primary, app, &f.virtual_path), &*f.text);
            }
        }
    }
    let commit_fp = callee_fp("Commit");

    // The site links keep EVERY edge per span (S2b.6); this adapter still takes
    // the first in report order, as it always did — S3 converts all of them.
    let links = crate::program::model::site_links::SiteLinks::build(view);
    let mut program: HashMap<SiteKey, &'a ClassifiedEdge> = HashMap::new();
    for &app in &apps {
        for (k, edges) in links.sites_of(app) {
            let mut held = edges
                .iter()
                .copied()
                .filter(|ce| caller_held(&ce.edge.from, primary, demand));
            let Some(first) = held.next() else {
                continue;
            };
            let n = 1 + held.count();
            c.program_sites += n;
            c.duplicate_program_span += n - 1;
            let unit = model_unit(graph, primary, app, &k.0);
            program.insert((unit, k.1, k.2, k.3, k.4), first);
        }
    }
    let is_operation = |ce: &ClassifiedEdge| {
        ce.edge.kind == EdgeKind::ImplicitTrigger
            || (ce.edge.kind == EdgeKind::Call && ce.edge.site.callee_fingerprint == commit_fp)
    };

    let mut consumed: HashSet<SiteKey> = HashSet::new();
    let mut l3_ops: HashSet<SiteKey> = HashSet::new();
    let mut l3_record_ops: HashMap<SiteKey, Vec<(usize, usize)>> = HashMap::new();
    let mut misses: Vec<UnmatchedSite> = Vec::new();
    let mut unmatched = |reason: &'static str, k: &SiteKey, text: &str| {
        misses.push(UnmatchedSite {
            reason,
            unit: k.0.clone(),
            start_line: k.1,
            start_col: k.2,
            end_line: k.3,
            end_col: k.4,
            text: text.to_string(),
        });
    };
    let (mut matched, mut no_site, mut shape, mut fp, mut caller) = (0, 0, 0, 0, 0);
    let mut l3_call_sites = 0;
    let mut seen_l3_calls: HashSet<SiteKey> = HashSet::new();

    for (ri, r) in ws.routines.iter().enumerate() {
        for op in &r.operation_sites {
            if op.kind == "error-call" {
                continue;
            }
            c.l3_operation_sites += 1;
            let k = l3_key(&op.source_anchor);
            // Record ops (kind "record-op"/"lock") are handled with their
            // op name below; only `commit` is left here.
            if op.kind == "commit" && !program.contains_key(&k) {
                c.l3_op_no_program_site_other += 1;
                unmatched("l3_op_no_program_site_other", &k, "Commit");
            }
            l3_ops.insert(k);
        }
        for (oi, op) in r.record_operations.iter().enumerate() {
            c.l3_record_operations += 1;
            let k = l3_key(&op.source_anchor);
            if !program.contains_key(&k) {
                if can_fire_trigger(&op.op.fold_identifier()) {
                    c.l3_op_no_program_site_trigger += 1;
                    unmatched("l3_op_no_program_site_trigger", &k, &op.op);
                } else {
                    c.l3_op_no_program_site_other += 1;
                    unmatched("l3_op_no_program_site_other", &k, &op.op);
                }
            }
            l3_record_ops.entry(k).or_default().push((ri, oi));
        }
        let decl = &r.source_anchor;
        let decl_unit = decl
            .source_unit_id
            .strip_prefix("ws:")
            .unwrap_or(&decl.source_unit_id);
        for (ci, cs) in r.call_sites.iter().enumerate() {
            l3_call_sites += 1;
            let k = l3_key(&cs.source_anchor);
            if !seen_l3_calls.insert(k.clone()) {
                c.duplicate_l3_span += 1;
            }
            let Some(ce) = program.get(&k) else {
                no_site += 1;
                unmatched("no_program_site", &k, &cs.callee_text);
                continue;
            };
            consumed.insert(k.clone());
            if is_operation(ce) {
                shape += 1;
                unmatched("shape_mismatch", &k, &cs.callee_text);
            } else if callee_fp(&cs.callee_text) != ce.edge.site.callee_fingerprint {
                fp += 1;
                unmatched("callee_fp_mismatch", &k, &cs.callee_text);
            } else if surface
                .get_with_path(&ce.edge.from)
                .is_none_or(|(meta, path)| {
                    let unit = model_unit(graph, primary, ce.edge.from.object.app, path);
                    (
                        unit.as_str(),
                        meta.origin.start.row,
                        meta.origin.start.column,
                    ) != (decl_unit, decl.start_line, decl.start_column)
                })
            {
                caller += 1;
                unmatched("caller_mismatch", &k, &cs.callee_text);
            } else {
                matched += 1;
                matched_calls.insert((ri, ci), *ce);
            }
        }
    }

    let mut rest: Vec<(&SiteKey, &&'a ClassifiedEdge)> = program
        .iter()
        .filter(|(k, _)| !consumed.contains(*k))
        .collect();
    rest.sort_by(|a, b| a.0.cmp(b.0));
    let (mut op_site, mut it_matched, mut it_unmatched, mut prog_only, mut op_shape) =
        (0, 0, 0, 0, 0);
    for (k, ce) in rest {
        let text = span_text(&texts, k).unwrap_or("");
        if ce.edge.kind == EdgeKind::ImplicitTrigger {
            if !can_fire_trigger(&op_name(text)) {
                op_site += 1;
            } else if let Some(at) = l3_record_ops.get(k) {
                it_matched += 1;
                for &pos in at {
                    matched_ops.insert(pos, *ce);
                }
            } else {
                it_unmatched += 1;
                unmatched("implicit_trigger_unmatched", k, text);
            }
        } else if is_operation(ce) {
            op_site += 1;
        } else if l3_ops.contains(k) {
            op_shape += 1;
            unmatched("op_shape_mismatch", k, text);
        } else {
            prog_only += 1;
            unmatched("program_only_site", k, text);
        }
    }

    c.l3_call_sites = l3_call_sites;
    c.matched = matched;
    c.no_program_site = no_site;
    c.shape_mismatch = shape;
    c.op_shape_mismatch = op_shape;
    c.callee_fp_mismatch = fp;
    c.caller_mismatch = caller;
    c.operation_site = op_site;
    c.implicit_trigger_matched = it_matched;
    c.implicit_trigger_unmatched = it_unmatched;
    c.program_only_site = prog_only;
    debug_assert_eq!(
        c.l3_call_sites,
        c.matched + c.no_program_site + c.shape_mismatch + c.callee_fp_mismatch + c.caller_mismatch,
        "every L3 call site lands in exactly one bucket"
    );
    debug_assert_eq!(
        c.program_sites,
        c.duplicate_program_span
            + consumed.len()
            + c.operation_site
            + c.implicit_trigger_matched
            + c.implicit_trigger_unmatched
            + c.program_only_site
            + c.op_shape_mismatch,
        "every program site lands in exactly one bucket"
    );
    c.unmatched = misses;
    c.unmatched.sort_by(|a, b| {
        (&a.unit, a.start_line, a.start_col, a.reason).cmp(&(
            &b.unit,
            b.start_line,
            b.start_col,
            b.reason,
        ))
    });
    Join {
        census: c,
        calls: matched_calls,
        ops: matched_ops,
    }
}

/// The adapter, stage 1: L3's `ResolvedCalls` built from the program
/// engine's call-site edges (code map B7; see the module doc), plus the
/// census with its `adapter_*` counts. `view` must come from
/// `resolve_full_program_with(ctx)` (`ProgramReport::edge_view`, or a
/// `ProgramBody::edge_view` of the same build); `ws` is the L3 model of the same
/// workspace. Nothing here mutates `ws`.
///
/// `upgrade_dependency_bindings` (stage 2): upgrade the bindings of a call
/// into a dependency routine with that routine's parameter `var`-ness, as
/// for a workspace callee. Production passes `true`; tests pass `false` to see
/// stage 1 alone.
#[must_use]
pub fn resolved_calls_from_program(
    view: EdgeView<'_>,
    ctx: &ProgramContext,
    ws: &ModelEntities,
    upgrade_dependency_bindings: bool,
) -> (ResolvedCalls, SiteCensus) {
    adapter(view, ctx, ws, upgrade_dependency_bindings, None)
}

/// #160 steps 10 + 11a (§4.3, §4.5, §4.7, §4.8): the adapter over the rows WITH
/// their `RowNodes`, under the demand the model was built from (`None`: built
/// without demand). Until Task 2, `rows.row_nodes` and `demand` are not read
/// (Task 2 joins by key, checks the caller with `decl_anchor`, reads `j2_row` and
/// filters the program side by demand).
#[must_use]
#[cfg(test)]
pub(crate) fn resolved_calls_from_rows(
    view: EdgeView<'_>,
    ctx: &ProgramContext,
    rows: &crate::program::model::workspace::ModelRows,
    upgrade_dependency_bindings: bool,
    demand: Option<&HashSet<RoutineNodeId>>,
) -> (ResolvedCalls, SiteCensus) {
    let _ = (&rows.row_nodes, demand);
    adapter(
        view,
        ctx,
        &rows.workspace,
        upgrade_dependency_bindings,
        None,
    )
}

/// [`site_census`] over the rows with their `RowNodes` (see
/// [`resolved_calls_from_rows`]). Until Task 2, `row_nodes` and `demand` are
/// not read.
#[must_use]
#[cfg(test)]
pub(crate) fn site_census_rows(
    view: EdgeView<'_>,
    ctx: &ProgramContext,
    rows: &crate::program::model::workspace::ModelRows,
    demand: Option<&HashSet<RoutineNodeId>>,
) -> SiteCensus {
    let _ = (&rows.row_nodes, demand);
    join(view, ctx, &rows.workspace).census
}

/// The production step that points the detectors at the program engine's
/// calls: run the adapter (dependency bindings upgraded), build the event graph
/// from the program engine's subscriptions (S4.2), drop the program model, and
/// return the model with both. `alsem analyze` and
/// [`assemble_and_resolve_workspace_program`] both use it, so a test cannot
/// drift from the production path.
///
/// The program is the build the rows were assembled from: its body is consumed
/// (dropped here, after the adapter), and its meta is handed back. The meta's
/// `FreshCoverage` puts its dependency-ledger warnings on the model (#91), and its
/// input set gives the model's workspace diagnostics, with
/// `coverage.files_without_object` (#160 §4.2.4).
pub fn attach_program_calls(
    rows: crate::program::model::workspace::ModelRows,
    program: ResolvedProgram,
) -> (crate::program::model::workspace::Model, ProgramMeta) {
    let (model, meta, _census) = attach_program_calls_with(rows, program, None, None);
    (model, meta)
}

/// [`attach_program_calls`] for a cross-app model (engine-switch S7.3): `abi_rows`
/// joins a symbol-only dependency routine to its model row. Also returns the
/// adapter's census (#160 §5.2: the cross-app loss gate reads it through this,
/// the production path). `demand` (#160 §4.5, 10A-2): the demand the model was
/// built from; not read until Task 2.
fn attach_program_calls_with(
    rows: crate::program::model::workspace::ModelRows,
    program: ResolvedProgram,
    abi_rows: Option<&crate::program::model::workspace::AbiRowIds>,
    demand: Option<&HashSet<RoutineNodeId>>,
) -> (
    crate::program::model::workspace::Model,
    ProgramMeta,
    SiteCensus,
) {
    use crate::engine::perf_trace as pt;
    let _ = demand;
    let (calls, census) = {
        let _s = pt::span("b3", "b3.adapter");
        let body = program.body();
        adapter(body.edge_view(), &body.ctx, &rows.workspace, true, abi_rows)
    };
    // Engine-switch S4.2: the detector event graph, from the same program build.
    let events = {
        let _s = pt::span("b3", "b3.events");
        crate::program::model::events::program_event_graph(&program.body().ctx, &rows.workspace)
    };
    if crate::engine::census::enabled() {
        crate::census_hook::mark("census.program");
        program.census();
        // #160 §4.13, §4.10: the build-local `RowNodes`, and the collision census.
        rows.row_nodes
            .census()
            .emit("program.row_nodes", crate::engine::census::Kind::Shallow);
        eprintln!(
            "[collision-census] {:?}",
            crate::program::model::workspace::collision_census(&rows.workspace, &rows.row_nodes)
        );
    }
    let (meta, body) = program.into_parts();
    body.release();
    // #160 §4.1.4: everything retained from here on is post-attach.
    pt::mark_phase_start("post_attach");
    let mut resolved = crate::program::model::workspace::Model {
        workspace: rows.workspace,
        root_classifications: rows.root_classifications,
        primary_app: rows.primary_app,
        infra_diagnostics: rows.infra_diagnostics,
        workspace_diagnostics:
            crate::engine::gate::workspace_diagnostics::compute_workspace_diagnostics(
                &meta.inputs,
                Some(&meta.coverage.files_without_object),
            ),
        dependency_diagnostics: meta.coverage.dependency_diagnostics(),
        calls: std::sync::Arc::new(calls),
        events: std::sync::Arc::new(events),
    };
    // A `local` event raised only with temporary records makes its subscribers'
    // same-named `var` parameters temporary (engine-switch S8).
    crate::program::model::event_param_temp::prove_event_param_temps(&mut resolved);
    if crate::engine::census::enabled() {
        crate::census_hook::mark("census.model");
        census_model(&resolved);
    }
    (resolved, meta, census)
}

/// #160 §4.1.5: the `model` bucket, shallow: the rows' buffers, the calls and
/// the event graph (each behind its own `Arc`).
fn census_model(m: &crate::program::model::workspace::Model) {
    use crate::engine::census::Kind;
    let mut t = census_model_rows(m);
    let c = census_calls(&m.calls);
    t.bytes += c.bytes;
    t.allocs += c.allocs;
    t.emit("model", Kind::Shallow);
    // #160 §4.13: the rows' `file` and site keys (inline in the rows' buffer,
    // which `model` charges, plus the key vectors' heap).
    let mut k = crate::engine::census::Tally {
        bytes: m.workspace.routines.len()
            * (std::mem::size_of::<Option<crate::program::site_key::FileIx>>()
                + std::mem::size_of::<crate::program::site_key::SiteExprs>()),
        allocs: 0,
    };
    for r in &m.workspace.routines {
        let e = &r.site_exprs;
        k.buf::<u32>(e.calls.len())
            .buf::<u32>(e.record_ops.len())
            .buf::<u32>(e.op_sites.len());
    }
    k.emit("model.site_exprs", Kind::Exact);
}

/// The `model` bucket without its calls: the rows' buffers and the events
/// (shallow). The cross-app base keeps these when it replaces the calls, so it
/// charges them as `base.resolved.rows` (final panel F2-1).
pub(crate) fn census_model_rows(
    m: &crate::program::model::workspace::Model,
) -> crate::engine::census::Tally {
    use crate::engine::census::{Tally, event_graph};
    use crate::program::model::workspace as w;
    let mut t = Tally::default();
    t.buf::<w::ModelObject>(m.workspace.objects.len())
        .buf::<w::ModelTable>(m.workspace.tables.len())
        .buf::<w::ModelRoutine>(m.workspace.routines.len())
        .buf::<crate::engine::root_classification::RootClassification>(m.root_classifications.len())
        .buf::<crate::engine::root_classification::InfraDiagnostic>(m.infra_diagnostics.len())
        .buf::<crate::engine::l5::registry::Diagnostic>(m.workspace_diagnostics.len())
        .buf::<crate::engine::l5::registry::Diagnostic>(m.dependency_diagnostics.len())
        .arc::<crate::program::model::events::ProgramEvents>();
    let eg = event_graph(&m.events.graph);
    t.bytes += eg.bytes;
    t.allocs += eg.allocs;
    t
}

/// One `ResolvedCalls` behind its `Arc` (shallow): the edges, the upgraded
/// bindings, the diagnostics and the external targets.
pub(crate) fn census_calls(c: &ResolvedCalls) -> crate::engine::census::Tally {
    let mut t = crate::engine::census::Tally::default();
    t.arc::<ResolvedCalls>()
        .buf::<crate::program::model::calls::CallEdge>(c.edges.len())
        .buf::<(String, Vec<crate::program::model::calls::UpgradedBinding>)>(
            c.upgraded_bindings.len(),
        )
        .strs(c.upgraded_bindings.keys())
        .bufs(c.upgraded_bindings.values())
        .buf::<crate::engine::l5::registry::Diagnostic>(c.diagnostics.len())
        .buf::<crate::program::model::calls::ExternalTargetRef>(c.external_targets.len());
    t
}

/// [`assemble_and_resolve_workspace_program`] with the default model-instance id
/// and `roots.config.json` read: the model the detectors see in `alsem analyze`.
/// `None` if the build fails. For tests that pin detector output (r4/r4f
/// goldens). Until engine-switch S9.3 it built L3's disk model and attached the
/// program calls to it; no golden moved when it switched.
#[must_use]
pub fn assemble_and_resolve_workspace_with_program_calls(
    workspace: &std::path::Path,
) -> Option<crate::program::model::workspace::Model> {
    assemble_and_resolve_workspace_program(
        &crate::program::inputs::InputSet::read(workspace, &Default::default()),
        crate::program::model::workspace::MODEL_INSTANCE_ID_DEFAULT,
        false,
    )
}

/// The program-backed model every consumer moves onto in engine-switch S6: the
/// model projected from one program build (as `alsem analyze` builds it,
/// `gate::run::build_analysis_model`), with the program engine's calls and event
/// graph attached. `None` when the program build or the model assembly fails.
/// Every workspace input comes from `inputs`, the command's one read (#160).
#[must_use]
pub fn assemble_and_resolve_workspace_program(
    inputs: &std::sync::Arc<crate::program::inputs::InputSet>,
    model_instance_id: &str,
    skip_roots_config: bool,
) -> Option<crate::program::model::workspace::Model> {
    let program = ResolvedProgram::build(inputs, ProgramScope::Workspace).ok()?;
    let rows = crate::program::model::workspace::assemble_and_resolve_workspace_from_program(
        inputs,
        model_instance_id,
        skip_roots_config,
        &program.body().ctx,
        &program.body().parenless_calls,
    )?;
    Some(attach_program_calls(rows, program).0)
}

/// [`assemble_and_resolve_workspace_program`] over inline `(relative path, source)`
/// files (engine-switch S9.2), for tests that state their AL in code. The files
/// and an `app.json` with id `app_guid` are written to a temporary directory and
/// built exactly as on disk, so the model is the production one: unit ids are
/// `ws:<relative path>` (as the L3 inline builder spells them), `primary_app` is
/// that `app.json`, and no `roots.config.json` is read.
///
/// # Panics
/// On an absolute or `..` path, a name not ending in `.al`, an I/O failure, or a
/// failed build: test input.
#[must_use]
pub fn assemble_and_resolve_inline_program(
    files: &[(String, String)],
    app_guid: &str,
    model_instance_id: &str,
) -> crate::program::model::workspace::Model {
    let dir = tempfile::tempdir().expect("temp workspace");
    let app = serde_json::json!({
        "id": app_guid,
        "name": "Inline",
        "publisher": "Inline",
        "version": "1.0.0.0",
    });
    std::fs::write(dir.path().join("app.json"), app.to_string()).expect("write app.json");
    for (name, source) in files {
        let rel = std::path::Path::new(name);
        assert!(
            rel.components()
                .all(|c| matches!(c, std::path::Component::Normal(_))),
            "inline file name must be a plain relative path: {name}"
        );
        // The workspace discovery reads `.al` files only; any other name would be
        // silently absent from the model.
        assert!(
            rel.extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("al")),
            "inline file name must end in .al (the program engine reads no other file): {name}"
        );
        let path = dir.path().join(rel);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("create dir");
        std::fs::write(&path, source).unwrap_or_else(|e| panic!("write {name}: {e}"));
    }
    assemble_and_resolve_workspace_program(
        &crate::program::inputs::InputSet::read(dir.path(), &Default::default()),
        model_instance_id,
        true,
    )
    .unwrap_or_else(|| panic!("program build failed for inline workspace {app_guid}"))
}

/// [`assemble_and_resolve_inline_program`] with the default model-instance id.
#[must_use]
pub fn assemble_and_resolve_inline_program_default(
    files: &[(String, String)],
    app_guid: &str,
) -> crate::program::model::workspace::Model {
    assemble_and_resolve_inline_program(
        files,
        app_guid,
        crate::program::model::workspace::MODEL_INSTANCE_ID_DEFAULT,
    )
}

/// The CROSS-APP detector model and what the cross-app base reads besides it.
pub struct CrossAppProgram {
    /// The model, with calls and events attached for every body in it.
    pub resolved: crate::program::model::workspace::Model,
    /// The workspace's declared dependencies: app.json's, plus the implicit
    /// Microsoft tier the snapshot adds (d17's MinVersion side).
    pub declared_dependencies: Vec<crate::program::model::workspace::DeclaredDependencyDecl>,
    /// Every dependency app in the snapshot, `.app` path order (d17's resolved
    /// versions; the R3a-4 artifacts).
    pub dependency_apps: Vec<crate::program::model::workspace::DependencyApp>,
    /// The build's dependency coverage and ledger.
    pub coverage: crate::program::resolve::full::FreshCoverage,
    /// `internalsVisibleTo`, lower-case guids: exposing app -> its friend apps
    /// (engine-switch S8.5, d13).
    pub friends: std::collections::HashMap<String, std::collections::BTreeSet<String>>,
    /// Which dependency apps the build loaded, and how
    /// ([`crate::program::resolve::full::loaded_apps`], `alsem graph`'s `meta.apps`).
    pub loaded_apps: Vec<serde_json::Value>,
}

/// Why a cross-app build gave no model (#160 spec §4.4.5), so `alsem analyze`
/// classifies the failure without a second build. The coverage is boxed so the
/// `Err` stays small (`clippy::result_large_err`).
#[derive(Debug)]
pub enum CrossAppFailure {
    /// The program build's own `Err`.
    Build(String),
    /// Build `Ok`, assembly `None`: the build's coverage.
    Assembly {
        fresh: Box<crate::program::resolve::full::FreshCoverage>,
    },
    /// Build and assembly `Ok`, `primary_app` `None`.
    NoPrimaryApp {
        fresh: Box<crate::program::resolve::full::FreshCoverage>,
    },
}

/// The CROSS-APP detector model (engine-switch S7.3): the workspace and every
/// dependency in the snapshot (`assemble_and_resolve_cross_app_from_program`), from
/// one `FULL` program build, with the program engine's calls attached for EVERY
/// body in it — the workspace's, and each dependency's resolved from its own app
/// (`ProgramContext::resolve_dependency_bodies`) — and its event graph over all of
/// them. `Err` when the program build or the model assembly fails.
pub fn assemble_and_resolve_cross_app_program(
    inputs: &std::sync::Arc<crate::program::inputs::InputSet>,
    model_instance_id: &str,
    skip_roots_config: bool,
) -> Result<CrossAppProgram, CrossAppFailure> {
    cross_app_program_census(inputs, model_instance_id, skip_roots_config).map(|(p, _)| p)
}

/// The cross-app build up to the attach: the program, the demand the model was
/// built from, and the rows (#160: one helper shared by the production entry and
/// the test entries, so they cannot drift).
pub(crate) struct CrossAppAssembled {
    pub(crate) program: ResolvedProgram,
    pub(crate) demand: HashSet<RoutineNodeId>,
    pub(crate) rows: crate::program::model::workspace::ModelRows,
    pub(crate) abi_rows: crate::program::model::workspace::AbiRowIds,
}

pub(crate) fn cross_app_assemble(
    inputs: &std::sync::Arc<crate::program::inputs::InputSet>,
    model_instance_id: &str,
    skip_roots_config: bool,
) -> Result<CrossAppAssembled, CrossAppFailure> {
    use crate::engine::perf_trace as pt;
    let program =
        ResolvedProgram::build(inputs, ProgramScope::CrossApp).map_err(CrossAppFailure::Build)?;
    let body = program.body();
    let ctx = &body.ctx;
    // S8.2: only what the workspace can reach, and the code around the dependency
    // events it subscribes to (`program::resolve::demand`).
    let demand = {
        let _s = pt::span("crossapp", "crossapp.demand");
        crate::program::resolve::demand::cross_app_demand(
            ctx.primary_app_ref,
            ctx.graph(),
            body.all_edges(),
        )
    };
    let assembled = {
        let _s = pt::span("crossapp", "crossapp.model_assembly");
        crate::program::model::workspace::assemble_and_resolve_cross_app_from_program(
            inputs,
            model_instance_id,
            skip_roots_config,
            ctx,
            Some(&demand),
            &body.parenless_calls,
        )
    };
    let Some((rows, abi_rows)) = assembled else {
        // The `body()` borrow has ended: the coverage moves out (#91).
        return Err(CrossAppFailure::Assembly {
            fresh: Box::new(program.into_parts().0.coverage),
        });
    };
    Ok(CrossAppAssembled {
        program,
        demand,
        rows,
        abi_rows,
    })
}

/// #160 §5.10 (A8): the cross-app assembly's rows with their `RowNodes`, BEFORE
/// the attach (`RowNodes` is build-local; production never keeps it). `None` when
/// the build or the assembly fails. Test entry.
#[doc(hidden)]
#[must_use]
pub fn cross_app_rows_before_attach(
    inputs: &std::sync::Arc<crate::program::inputs::InputSet>,
    model_instance_id: &str,
) -> Option<crate::program::model::workspace::ModelRows> {
    cross_app_assemble(inputs, model_instance_id, true)
        .ok()
        .map(|a| a.rows)
}

/// [`assemble_and_resolve_cross_app_program`] plus the adapter's census, from
/// the same attach (#160 §5.2).
fn cross_app_program_census(
    inputs: &std::sync::Arc<crate::program::inputs::InputSet>,
    model_instance_id: &str,
    skip_roots_config: bool,
) -> Result<(CrossAppProgram, SiteCensus), CrossAppFailure> {
    use crate::engine::perf_trace as pt;
    let CrossAppAssembled {
        program,
        demand,
        rows,
        abi_rows,
    } = cross_app_assemble(inputs, model_instance_id, skip_roots_config)?;
    let body = program.body();
    let ctx = &body.ctx;
    let mut friends: std::collections::HashMap<String, std::collections::BTreeSet<String>> =
        std::collections::HashMap::new();
    for u in &ctx.snapshot().apps {
        for f in &u.internals_visible_to {
            if !f.app_id.is_empty() {
                friends
                    .entry(u.id.guid.to_ascii_lowercase())
                    .or_default()
                    .insert(f.app_id.to_ascii_lowercase());
            }
        }
    }
    let snap = ctx.snapshot();
    let declared_dependencies = snap
        .apps
        .iter()
        .find(|u| u.id == snap.workspace_app)
        .map(|u| {
            u.declared_deps
                .iter()
                .filter(|d| !d.app_id.is_empty())
                .map(
                    |d| crate::program::model::workspace::DeclaredDependencyDecl {
                        app_guid: d.app_id.clone(),
                        name: d.name.clone(),
                        min_version: if d.version.is_empty() {
                            "0.0.0.0".to_string()
                        } else {
                            d.version.clone()
                        },
                    },
                )
                .collect()
        })
        .unwrap_or_default();
    let mut deps: Vec<&crate::snapshot::AppUnit> = snap
        .apps
        .iter()
        .filter(|u| ctx.is_required_dependency(&u.id))
        .collect();
    deps.sort_by(|a, b| a.app_path.cmp(&b.app_path));
    let dependency_apps = deps
        .into_iter()
        .map(|u| crate::program::model::workspace::DependencyApp {
            guid: u.id.guid.clone(),
            name: u.id.name.clone(),
            version: u.id.version.clone(),
            has_source: u.source.is_some(),
        })
        .collect();
    let loaded_apps = program.loaded_apps();
    let (resolved, meta, census) = {
        let _s = pt::span("crossapp", "crossapp.attach_program_calls");
        attach_program_calls_with(rows, program, Some(&abi_rows), Some(&demand))
    };
    Ok((
        CrossAppProgram {
            resolved,
            declared_dependencies,
            dependency_apps,
            coverage: meta.coverage,
            friends,
            loaded_apps,
        },
        census,
    ))
}

/// The cross-app adapter census for `workspace` (#160 §5.2, T2): the production
/// cross-app build and attach ([`assemble_and_resolve_cross_app_program`]'s own
/// code), with the census it discards kept.
pub fn adapter_census_for_workspace_cross_app(
    workspace: &std::path::Path,
) -> Result<SiteCensus, String> {
    cross_app_program_census(
        &crate::program::inputs::InputSet::read(workspace, &Default::default()),
        crate::program::model::workspace::MODEL_INSTANCE_ID_DEFAULT,
        false,
    )
    .map(|(_, c)| c)
    .map_err(|e| format!("cross-app build of {}: {e:?}", workspace.display()))
}

/// The edge for a call site the program engine gave no usable edge for: one
/// to-less `Unknown(NoProgramSite)` edge, bindings in their initial state (a
/// record argument stays `"unresolved-callee"`). Engine-switch S3.1 — this used to
/// be the legacy resolver's answer.
fn no_program_edge(r: &ModelRoutine, cs: &PCallSite) -> (Vec<CallEdge>, Vec<UpgradedBinding>) {
    let mut e = CallEdge::base(&r.id, &cs.id, &cs.operation_id);
    e.resolution = Resolution::Unknown(ModelReason::NoProgramSite);
    (vec![e], initial_binding_state(cs).bindings)
}

/// The adapter: the model's calls from the program engine's edges, and the
/// census of how each site was produced.
fn adapter(
    view: EdgeView<'_>,
    ctx: &ProgramContext,
    ws: &ModelEntities,
    upgrade_dependency_bindings: bool,
    abi_rows: Option<&crate::program::model::workspace::AbiRowIds>,
) -> (ResolvedCalls, SiteCensus) {
    use crate::engine::perf_trace as pt;
    let j = {
        let _s = pt::span("b3", "b3.adapter.join");
        join(view, ctx, ws)
    };
    convert(j, view, ctx, ws, upgrade_dependency_bindings, abi_rows)
}

/// #160 step 10 (§4.3): the key join. Pairs model sites and program sites by
/// [`crate::program::site_key::SiteIx`], reading the program sites of the callers [`caller_held`] admits
/// (§4.5), checking the caller against the declaration anchor captured at
/// assembly (`RowNodes::decl_anchor`, §4.8), and classifying the program sites no
/// call site took by the resolver's `fires` (§4.4), with no source text. Same
/// buckets and rules as [`span_join`]. Until Task 2 switches `adapter` to it,
/// only the test-only parity oracle calls it.
#[cfg(test)]
fn key_join<'a>(
    view: EdgeView<'a>,
    ctx: &ProgramContext,
    ws: &ModelEntities,
    row_nodes: &crate::program::model::workspace::RowNodes,
    demand: Option<&HashSet<RoutineNodeId>>,
) -> Join<'a> {
    use crate::program::model::site_links::program_key;
    use crate::program::site_key::SiteIx;
    use std::collections::hash_map::Entry;
    let mut c = SiteCensus::default();
    let graph = ctx.graph();
    let primary = view.primary_app_ref;
    let apps = model_apps(graph, primary, ws);
    let commit_fp = callee_fp("Commit");
    let is_operation = |ce: &ClassifiedEdge| {
        ce.edge.kind == EdgeKind::ImplicitTrigger
            || (ce.edge.kind == EdgeKind::Call && ce.edge.site.callee_fingerprint == commit_fp)
    };
    let mut misses: Vec<UnmatchedSite> = Vec::new();
    let mut unmatched = |reason: &'static str, k: SiteKey, text: &str| {
        misses.push(UnmatchedSite {
            reason,
            unit: k.0,
            start_line: k.1,
            start_col: k.2,
            end_line: k.3,
            end_col: k.4,
            text: text.to_string(),
        });
    };
    // The model unit's spelling of a program site's span (§4.3 step 5: no text).
    let program_span = |ce: &ClassifiedEdge| {
        let (path, sl, sc, el, ec) = program_key(&ce.edge.site.span);
        let unit = model_unit(graph, primary, ce.edge.from.object.app, &path);
        (unit, sl, sc, el, ec)
    };

    // 1. Program side: one pass, first edge per key wins.
    let mut program: HashMap<SiteIx, &'a ClassifiedEdge> = HashMap::new();
    // A held call-site edge with no key (none in production: every resolution
    // the model reads was given its file) can pair with nothing.
    let mut unkeyed: Vec<&'a ClassifiedEdge> = Vec::new();
    for ce in view.all_edges() {
        if !matches!(ce.obligation_id, ObligationId::CallSite { .. })
            || !apps.contains(&ce.edge.from.object.app)
            || !caller_held(&ce.edge.from, primary, demand)
        {
            continue;
        }
        c.program_sites += 1;
        match ce.site {
            Some(s) => match program.entry(s.at) {
                Entry::Vacant(v) => {
                    v.insert(ce);
                }
                Entry::Occupied(_) => c.duplicate_program_span += 1,
            },
            None => unkeyed.push(ce),
        }
    }

    // 2. Model side: per row, `(row.file, expression index)`; duplicates per
    // category (a record op and its own operation site share one key by design).
    let mut consumed: HashSet<SiteIx> = HashSet::new();
    let mut model_ops: HashSet<SiteIx> = HashSet::new();
    let mut model_record_ops: HashMap<SiteIx, Vec<(usize, usize)>> = HashMap::new();
    let (mut seen_calls, mut seen_record_ops, mut seen_op_sites) =
        (HashSet::new(), HashSet::new(), HashSet::new());
    let (mut matched_calls, mut matched_ops) = (HashMap::new(), HashMap::new());
    let (mut matched, mut no_site, mut shape, mut fp, mut caller) = (0, 0, 0, 0, 0);
    for (ri, r) in ws.routines.iter().enumerate() {
        let has_sites = !(r.call_sites.is_empty()
            && r.record_operations.is_empty()
            && r.operation_sites.is_empty());
        debug_assert!(
            r.file.is_some() || !has_sites,
            "row {} has sites but no file: they cannot pair",
            r.id
        );
        let key = |e: &u32| r.file.map(|file| SiteIx { file, expr: *e });
        for (op, e) in r.operation_sites.iter().zip(&r.site_exprs.op_sites) {
            if op.kind == "error-call" {
                continue;
            }
            c.l3_operation_sites += 1;
            let k = key(e);
            if let Some(k) = k
                && !seen_op_sites.insert(k)
            {
                c.duplicate_l3_span += 1;
            }
            // Record ops are handled with their op name below; only `commit`
            // is left here.
            if op.kind == "commit" && k.is_none_or(|k| !program.contains_key(&k)) {
                c.l3_op_no_program_site_other += 1;
                unmatched(
                    "l3_op_no_program_site_other",
                    l3_key(&op.source_anchor),
                    "Commit",
                );
            }
            model_ops.extend(k);
        }
        for (oi, (op, e)) in r
            .record_operations
            .iter()
            .zip(&r.site_exprs.record_ops)
            .enumerate()
        {
            c.l3_record_operations += 1;
            let k = key(e);
            if let Some(k) = k
                && !seen_record_ops.insert(k)
            {
                c.duplicate_l3_span += 1;
            }
            if k.is_none_or(|k| !program.contains_key(&k)) {
                let reason = if can_fire_trigger(&op.op.fold_identifier()) {
                    c.l3_op_no_program_site_trigger += 1;
                    "l3_op_no_program_site_trigger"
                } else {
                    c.l3_op_no_program_site_other += 1;
                    "l3_op_no_program_site_other"
                };
                unmatched(reason, l3_key(&op.source_anchor), &op.op);
            }
            if let Some(k) = k {
                model_record_ops.entry(k).or_default().push((ri, oi));
            }
        }
        let decl = &r.source_anchor;
        for (ci, (cs, e)) in r.call_sites.iter().zip(&r.site_exprs.calls).enumerate() {
            c.l3_call_sites += 1;
            let k = key(e);
            if let Some(k) = k
                && !seen_calls.insert(k)
            {
                c.duplicate_l3_span += 1;
            }
            let Some((k, ce)) = k.and_then(|k| program.get(&k).map(|ce| (k, *ce))) else {
                no_site += 1;
                unmatched(
                    "no_program_site",
                    l3_key(&cs.source_anchor),
                    &cs.callee_text,
                );
                continue;
            };
            consumed.insert(k);
            if is_operation(ce) {
                shape += 1;
                unmatched("shape_mismatch", l3_key(&cs.source_anchor), &cs.callee_text);
            } else if callee_fp(&cs.callee_text) != ce.edge.site.callee_fingerprint {
                fp += 1;
                unmatched(
                    "callee_fp_mismatch",
                    l3_key(&cs.source_anchor),
                    &cs.callee_text,
                );
            } else if row_nodes.decl_anchor.get(&ce.edge.from)
                != Some(&(k.file, decl.start_line, decl.start_column))
            {
                caller += 1;
                unmatched(
                    "caller_mismatch",
                    l3_key(&cs.source_anchor),
                    &cs.callee_text,
                );
            } else {
                matched += 1;
                matched_calls.insert((ri, ci), ce);
            }
        }
    }

    // 3. The program sites no call site took (J4's buckets, by `fires`).
    let (mut op_site, mut it_matched, mut it_unmatched, mut prog_only, mut op_shape) =
        (0, 0, 0, 0, 0);
    for (k, ce) in program.iter().filter(|(k, _)| !consumed.contains(*k)) {
        if ce.edge.kind == EdgeKind::ImplicitTrigger {
            if !ce.site.is_some_and(|s| s.fires) {
                op_site += 1;
            } else if let Some(at) = model_record_ops.get(k) {
                it_matched += 1;
                for &pos in at {
                    matched_ops.insert(pos, *ce);
                }
            } else {
                it_unmatched += 1;
                unmatched("implicit_trigger_unmatched", program_span(ce), "");
            }
        } else if is_operation(ce) {
            op_site += 1;
        } else if model_ops.contains(k) {
            op_shape += 1;
            unmatched("op_shape_mismatch", program_span(ce), "");
        } else {
            prog_only += 1;
            unmatched("program_only_site", program_span(ce), "");
        }
    }
    for ce in unkeyed {
        prog_only += 1;
        unmatched("program_only_site", program_span(ce), "");
    }

    c.matched = matched;
    c.no_program_site = no_site;
    c.shape_mismatch = shape;
    c.op_shape_mismatch = op_shape;
    c.callee_fp_mismatch = fp;
    c.caller_mismatch = caller;
    c.operation_site = op_site;
    c.implicit_trigger_matched = it_matched;
    c.implicit_trigger_unmatched = it_unmatched;
    c.program_only_site = prog_only;
    debug_assert_eq!(
        c.l3_call_sites,
        c.matched + c.no_program_site + c.shape_mismatch + c.callee_fp_mismatch + c.caller_mismatch,
        "every model call site lands in exactly one bucket"
    );
    debug_assert_eq!(
        c.program_sites,
        c.duplicate_program_span
            + consumed.len()
            + c.operation_site
            + c.implicit_trigger_matched
            + c.implicit_trigger_unmatched
            + c.program_only_site
            + c.op_shape_mismatch,
        "every program site lands in exactly one bucket"
    );
    c.unmatched = misses;
    c.unmatched.sort_by(|a, b| {
        (&a.unit, a.start_line, a.start_col, a.reason).cmp(&(
            &b.unit,
            b.start_line,
            b.start_col,
            b.reason,
        ))
    });
    Join {
        census: c,
        calls: matched_calls,
        ops: matched_ops,
    }
}

/// The adapter's conversion of one join's pairs (#160: split from `adapter` so
/// the parity oracle can convert both joins' pairs the same way).
fn convert<'a>(
    j: Join<'a>,
    view: EdgeView<'a>,
    ctx: &ProgramContext,
    ws: &ModelEntities,
    upgrade_dependency_bindings: bool,
    abi_rows: Option<&crate::program::model::workspace::AbiRowIds>,
) -> (ResolvedCalls, SiteCensus) {
    use crate::engine::perf_trace as pt;
    let mut c = j.census;
    let surface = ctx.decl_surface();
    let graph = ctx.graph();
    let mut by_decl: HashMap<(&str, u32, u32), &ModelRoutine> = HashMap::new();
    {
        let _s = pt::span("b3", "b3.adapter.by_decl");
        for r in &ws.routines {
            let a = &r.source_anchor;
            let unit = a
                .source_unit_id
                .strip_prefix("ws:")
                .unwrap_or(&a.source_unit_id);
            by_decl
                .entry((unit, a.start_line, a.start_column))
                .or_insert(r);
        }
    }
    let objects = graph
        .objects
        .iter()
        .map(|o| (object_key(&o.id), o))
        .collect();
    let primary_objects = graph
        .objects
        .iter()
        .filter(|o| o.id.app == view.primary_app_ref)
        .map(|o| (o.id.kind, o.name.fold_identifier()))
        .collect();
    let empty_abi_rows = crate::program::model::workspace::AbiRowIds::new();
    let conv = Converter {
        primary: view.primary_app_ref,
        model_apps: model_apps(graph, view.primary_app_ref, ws),
        abi_rows: abi_rows.unwrap_or(&empty_abi_rows),
        routine_by_id: ws.routines.iter().map(|r| (r.id.as_str(), r)).collect(),
        upgrade_dependency_bindings,
        graph,
        surface: &surface,
        by_decl,
        objects,
        primary_objects,
        site_facts: view.site_facts,
        registry: ctx.registry(),
        targets: std::cell::RefCell::new(Vec::new()),
    };

    // The conversion loop to the end of the adapter.
    let _s_convert = pt::span("b3", "b3.adapter.convert");
    let mut edges: Vec<CallEdge> = Vec::new();
    let mut upgraded_bindings: HashMap<String, Vec<UpgradedBinding>> = HashMap::new();
    // The adapter emits no resolver diagnostics (the legacy resolver's
    // double-upgrade warning had no program-engine analogue to fire).
    let diagnostics = Vec::new();
    // Call sites in routine order, then the implicit-trigger edges in
    // routine/op order (the legacy resolver's order).
    for (ri, r) in ws.routines.iter().enumerate() {
        for (ci, cs) in r.call_sites.iter().enumerate() {
            let program_edge = j.calls.get(&(ri, ci));
            let converted = program_edge.and_then(|ce| {
                let out = conv.call(r, cs, ce, &mut c);
                if out.is_none() {
                    c.adapter_callee_outside_l3 += 1;
                }
                out
            });
            let (site_edges, bindings) = match converted {
                Some(x) => {
                    c.adapter_program_sites += 1;
                    x
                }
                None => {
                    // S3.1: no legacy fallback. The program engine gave nothing
                    // usable here, and that is reported as such.
                    c.adapter_l3_fallback_sites += 1;
                    no_program_edge(r, cs)
                }
            };
            edges.extend(site_edges);
            upgraded_bindings.insert(cs.id.clone(), bindings);
        }
    }
    for (ri, r) in ws.routines.iter().enumerate() {
        for (oi, op) in r.record_operations.iter().enumerate() {
            if let Some(ce) = j.ops.get(&(ri, oi)) {
                c.adapter_program_trigger_ops += 1;
                edges.extend(conv.triggers(r, op, ce, &mut c));
            } else if matches!(
                op.op.as_str(),
                "Insert" | "Modify" | "Delete" | "Validate" | "Rename"
            ) {
                // S3.5: no legacy fallback.
                c.adapter_l3_trigger_ops += 1;
            }
        }
    }
    (
        ResolvedCalls {
            edges,
            upgraded_bindings,
            diagnostics,
            external_targets: conv.targets.into_inner(),
        },
        c,
    )
}

/// Where a route's target lives relative to the model (engine-switch S7.3).
enum Target<'a> {
    /// A routine the model holds: an edge to it.
    Model(&'a ModelRoutine),
    /// A primary-app routine the model lacks: the site fails (S3.1).
    MissingFromModel,
    /// Anything else: a dependency target, a builtin, an unresolved route.
    Outside,
}

/// Object lookup key shared by a graph `ObjectNodeId` and an
/// `AbiRoutineKey` (which spells the kind as lowercase `Debug` text).
type ObjectKey = (AppRef, String, ObjKey);

fn object_key(id: &ObjectNodeId) -> ObjectKey {
    (
        id.app,
        format!("{:?}", id.kind).to_ascii_lowercase(),
        id.key.clone(),
    )
}

struct Converter<'a> {
    primary: AppRef,
    /// The apps whose routines the model holds (S7.3): the primary app, or every
    /// app in a cross-app model.
    model_apps: HashSet<AppRef>,
    /// The cross-app model's symbol-only rows by program id (S7.3); empty otherwise.
    abi_rows: &'a crate::program::model::workspace::AbiRowIds,
    routine_by_id: HashMap<&'a str, &'a ModelRoutine>,
    upgrade_dependency_bindings: bool,
    graph: &'a ProgramGraph,
    surface: &'a DeclSurface,
    /// L3 routines by declaration anchor `(unit, line, column)`.
    by_decl: HashMap<(&'a str, u32, u32), &'a ModelRoutine>,
    objects: HashMap<ObjectKey, &'a ObjectNode>,
    /// The primary app's objects by `(kind, folded name)` (S6.0): "is this
    /// receiver object ours?".
    primary_objects: std::collections::HashSet<(al_syntax::ir::ObjectKind, String)>,
    /// `ProgramReport::site_facts`: interfaces (S3.2) and receivers (S6.0).
    site_facts: &'a HashMap<ObligationId, crate::program::resolve::full::SiteFacts>,
    /// The dependency target registry (S3.3): body state per dependency routine.
    registry: crate::program::registry::DependencyRegistry<'a>,
    /// The dependency targets reached so far, in edge order (S3.3). Pushed only
    /// once an edge is final, so a site that then fails leaves none behind.
    targets: std::cell::RefCell<Vec<crate::program::model::calls::ExternalTargetRef>>,
}

impl<'a> Converter<'a> {
    /// The model routine for a program source routine, by declaration anchor: a
    /// primary-app routine, or a dependency routine of a cross-app model (S7.3).
    fn l3_routine(&self, id: &RoutineNodeId) -> Option<&'a ModelRoutine> {
        if !self.model_apps.contains(&id.object.app) {
            return None;
        }
        let (meta, path) = self.surface.get_with_path(id)?;
        let unit = model_unit(self.graph, self.primary, id.object.app, path);
        self.by_decl
            .get(&(
                unit.as_str(),
                meta.origin.start.row,
                meta.origin.start.column,
            ))
            .copied()
    }

    /// Where a route's target lives relative to the model (S7.3).
    fn target(&self, route: &Route) -> Target<'a> {
        match &route.target {
            RouteTarget::Routine(id) => match self.l3_routine(id) {
                Some(r) => Target::Model(r),
                // A primary routine must be in the model: the site is an honest
                // unknown (S3.1).
                None if id.object.app == self.primary => Target::MissingFromModel,
                None => Target::Outside,
            },
            // A run into a workspace object with no entry trigger.
            RouteTarget::AbiSymbol { key } if key.app == self.primary => Target::Outside,
            RouteTarget::AbiSymbol { .. } => Self::route_routine_id(route)
                .and_then(|id| self.abi_rows.get(&id))
                .and_then(|row| self.routine_by_id.get(row.as_str()).copied())
                .map_or(Target::Outside, Target::Model),
            RouteTarget::Builtin(_) | RouteTarget::Unresolved => Target::Outside,
        }
    }

    fn type_ref(&self, key: &ObjectKey) -> Option<ExternalTypeRef> {
        self.objects.get(key).map(|o| ExternalTypeRef {
            kind: format!("{:?}", o.id.kind),
            name: o.name.to_string(),
        })
    }

    /// The member call's receiver as the program resolver typed it (S6.0; it
    /// used to come from L3's receiver inference).
    fn receiver(
        &self,
        ce: &ClassifiedEdge,
    ) -> Option<&'a crate::program::resolve::full::ReceiverFact> {
        self.site_facts
            .get(&ce.obligation_id)
            .and_then(|f| f.receiver.as_ref())
    }

    /// The type a call into an object absent from the graph names: the
    /// object-typed receiver, or the `Kind::Name` of an object run.
    fn named_type_ref(&self, cs: &PCallSite, ce: &ClassifiedEdge) -> Option<ExternalTypeRef> {
        use crate::program::resolve::full::ReceiverFactType;
        match &cs.callee {
            PCallee::ObjectRun {
                object_kind,
                target_ref: Some(t),
                ..
            } => Some(ExternalTypeRef {
                kind: object_kind.clone(),
                name: t.clone(),
            }),
            PCallee::Member { .. } => match &self.receiver(ce)?.ty {
                ReceiverFactType::Object { kind, name } => Some(ExternalTypeRef {
                    kind: format!("{kind:?}"),
                    name: name.clone(),
                }),
                _ => None,
            },
            _ => None,
        }
    }

    /// One matched call site → its edges and bindings (B7). `None` when a
    /// workspace callee has no L3 routine: the caller falls back to L3.
    fn call(
        &self,
        r: &ModelRoutine,
        cs: &PCallSite,
        ce: &ClassifiedEdge,
        c: &mut SiteCensus,
    ) -> Option<(Vec<CallEdge>, Vec<UpgradedBinding>)> {
        let edge = &ce.edge;
        let mut state = initial_binding_state(cs);
        let is_run = edge.kind == EdgeKind::Run;
        // Dispatch kind from the L2 callee shape (ruling 4). A `Page.RunModal(..)`
        // style member call the program engine sees as a run takes the run kind
        // its receiver keyword names.
        let mut e = CallEdge::base(&r.id, &cs.id, &cs.operation_id);
        e.dispatch_kind = match &cs.callee {
            PCallee::Bare { .. } => DispatchKind::Direct,
            PCallee::Member { receiver, .. }
                if is_run
                    && matches!(
                        receiver.fold_identifier().as_str(),
                        "codeunit" | "page" | "report"
                    ) =>
            {
                object_run_dispatch_kind(match receiver.fold_identifier().as_str() {
                    "page" => "Page",
                    "report" => "Report",
                    _ => "Codeunit",
                })
            }
            PCallee::Member { .. } => DispatchKind::Method,
            PCallee::ObjectRun { object_kind, .. } => object_run_dispatch_kind(object_kind),
            PCallee::Unknown => DispatchKind::Unresolved,
        };
        let external = if is_run {
            Resolution::Opaque
        } else {
            Resolution::ExternalTarget
        };
        // S9.0d: the one `Multicast` a call site gets is an object run
        // (`resolver::dispatch_entry_trigger`): it reaches EVERY entry trigger,
        // the base object's and each extension's, so each route is its own
        // edge. An empty one is a run of an object whose source declares no
        // entry trigger: L3's run shape, `Opaque`, with no callee to name.
        if edge.shape == DispatchShape::Multicast {
            if edge.routes.is_empty() {
                // The L3 shape of a run that reaches no entry trigger, as it
                // was when the resolver still invented an Opaque trigger route
                // for one: into our own app, the run kind with no external
                // type; into a dependency, an external callee of that object.
                let target = self
                    .site_facts
                    .get(&ce.obligation_id)
                    .and_then(|f| f.run_target.as_ref());
                match target {
                    Some(t) if t.app != self.primary => {
                        e.resolution = external;
                        e.external_type_ref = self.type_ref(&object_key(t));
                    }
                    _ => {
                        c.adapter_workspace_run_no_entry += 1;
                        e.dispatch_kind = object_run_dispatch_kind(match target.map(|t| t.kind) {
                            Some(ObjectKind::Page) => "Page",
                            Some(ObjectKind::Report) => "Report",
                            _ => "Codeunit",
                        });
                        e.resolution = Resolution::Opaque;
                    }
                }
                return Some((vec![e], state.bindings));
            }
            let mut edges = Vec::with_capacity(edge.routes.len());
            for route in &edge.routes {
                let mut one = e.clone();
                self.route_into(&mut one, route, cs, ce, external, is_run, &mut state, c)?;
                edges.push(one);
            }
            return Some((edges, state.bindings));
        }
        // S3.6: no route is cut. A call edge with several routes outside an
        // interface or a run (the resolver makes none today: `Exact` has one
        // route) converts as the closed candidate set it is, never as its
        // first route.
        let shape = match edge.shape {
            DispatchShape::Exact if edge.routes.len() > 1 => {
                c.adapter_multi_route_sites += 1;
                DispatchShape::AmbiguousOverload
            }
            s => s,
        };
        match shape {
            DispatchShape::Polymorphic => {
                let edges = self.interface(r, cs, ce, &mut state, c)?;
                return Some((edges, state.bindings));
            }
            DispatchShape::AmbiguousOverload => {
                let mut ids = Vec::new();
                let mut dep_ref = None;
                let mut dep_routes = Vec::new();
                for route in &edge.routes {
                    match self.target(route) {
                        Target::Model(m) => ids.push(m.id.clone()),
                        Target::MissingFromModel => return None,
                        Target::Outside => {
                            dep_ref = dep_ref.or_else(|| self.dependency_ref(route));
                            dep_routes.push(route);
                        }
                    }
                }
                ids.sort();
                // The site converts (no `?` below): keep each dependency
                // candidate's identity (S3.6).
                for route in dep_routes {
                    if self.record_target(&cs.id, route) {
                        c.adapter_ambiguous_dependency_candidates += 1;
                    } else {
                        c.adapter_routes_dropped += 1;
                    }
                }
                if ids.is_empty()
                    && let Some(type_ref) = dep_ref
                {
                    // Every candidate is in a dependency: a dependency callee.
                    e.resolution = external;
                    e.external_type_ref = type_ref;
                } else {
                    e.resolution = Resolution::Ambiguous;
                    e.candidates = Some(ids);
                }
            }
            DispatchShape::DynamicOpen => {
                e.dispatch_kind = DispatchKind::Dynamic;
                e.resolution = Resolution::Unknown(if is_run {
                    ModelReason::DynamicObjectRunTarget
                } else {
                    ModelReason::DynamicReceiver
                });
            }
            DispatchShape::Exact | DispatchShape::Multicast => match edge.routes.first() {
                None => {
                    c.adapter_empty_route_sites += 1;
                    e.resolution = Resolution::Unknown(ModelReason::CalleeUnknown);
                }
                Some(route) => {
                    self.route_into(&mut e, route, cs, ce, external, is_run, &mut state, c)?
                }
            },
        }
        if e.resolution == Resolution::Ambiguous {
            mark_bindings_ambiguous(&mut state);
        }
        if e.resolution == Resolution::ExternalTarget {
            // L3 gives a record receiver's dependency callee
            // `Unknown(RecordTableProcedure)` ("unresolved-call"); an
            // object receiver's is already `ExternalTarget`. Count both.
            use crate::program::resolve::full::ReceiverFactType;
            match self.receiver(ce).map(|f| &f.ty) {
                Some(ReceiverFactType::Record { .. }) => c.adapter_external_record_receiver += 1,
                Some(ReceiverFactType::Object { .. }) => c.adapter_external_object_receiver += 1,
                _ => c.adapter_external_other += 1,
            }
        }
        Some((vec![e], state.bindings))
    }

    /// One route of a call site into `e`: its callee, resolution and kind.
    /// `None` when a workspace callee has no L3 routine (the caller falls back
    /// to L3).
    #[allow(clippy::too_many_arguments)]
    fn route_into(
        &self,
        e: &mut CallEdge,
        route: &crate::program::resolve::edge::Route,
        cs: &PCallSite,
        ce: &ClassifiedEdge,
        external: Resolution,
        is_run: bool,
        state: &mut BindingState,
        c: &mut SiteCensus,
    ) -> Option<()> {
        match (self.target(route), &route.target) {
            (Target::MissingFromModel, _) => return None,
            (Target::Model(callee), _) => {
                let id = Self::route_routine_id(route)?;
                let _ = upgrade_bindings(state, callee, &cs.id);
                e.to = Some(callee.id.clone());
                e.resolution = Resolution::Resolved;
                if let PCallee::Member { method, .. } = &cs.callee {
                    if method.fold_identifier() == "run"
                        && id.object.kind == al_syntax::ir::ObjectKind::Codeunit
                        && id.name_lc == "onrun"
                    {
                        // `CuVar.Run()` lands on OnRun: L3's codeunit-run shape.
                        e.dispatch_kind = DispatchKind::CodeunitRun;
                    } else if e.dispatch_kind == DispatchKind::Method {
                        e.receiver_type = self.receiver(ce).and_then(|f| f.type_text.clone());
                    }
                }
            }
            (Target::Outside, RouteTarget::Routine(_) | RouteTarget::AbiSymbol { .. }) => {
                e.resolution = external;
                e.external_type_ref = self.dependency_ref(route).flatten();
                self.record_target(&cs.id, route);
                if self.upgrade_dependency_bindings {
                    self.upgrade_dependency(state, route, &cs.id, c);
                }
            }
            (Target::Outside, RouteTarget::Builtin(_)) => {
                e.dispatch_kind = DispatchKind::Builtin;
                e.resolution = Resolution::Builtin;
            }
            (Target::Outside, RouteTarget::Unresolved) => {
                let reason = match route.evidence {
                    Evidence::Unknown(reason) => reason,
                    _ => PReason::IndexIntegrationGap,
                };
                let member_decline =
                    member_reason(reason) && self.receiver_outside_workspace(ce, route);
                if member_decline {
                    c.adapter_external_member_decline += 1;
                }
                // A run's entry trigger is "ambiguous" only when it is
                // ABI-collapse-marked: a dependency object.
                if reason == PReason::ObjectNotInGraph
                    || reason == PReason::AbiCollapsedOverload
                    || (is_run && reason == PReason::OverloadAmbiguous)
                    || member_decline
                {
                    e.resolution = external;
                    e.external_type_ref = self.named_type_ref(cs, ce);
                } else {
                    e.resolution = map_unknown(reason);
                    // L3 spells an unresolved bare call `Unresolved`.
                    if matches!(cs.callee, PCallee::Bare { .. })
                        && matches!(e.resolution, Resolution::Unknown(_))
                    {
                        e.dispatch_kind = DispatchKind::Unresolved;
                    }
                }
            }
        }
        Some(())
    }

    /// Stage 2: upgrade a dependency callee's bindings with its parameter
    /// `var`-ness (code map A4), as `upgrade_bindings` does for a workspace
    /// callee. A source dependency's comes from its declaration
    /// (`DeclSurface`); a symbol-only routine's from `AbiParams::Complete`
    /// only. `Missing` and `CollapsedUntrusted` leave the bindings
    /// `"unresolved-callee"`, as does a route with no routine behind it
    /// (the placeholder key of a run into an object with no entry trigger).
    /// Counted per source kind only when a binding actually changed (or, for
    /// the untrusted kinds, would have: an `"unresolved-callee"` binding sits
    /// within the callee's arity).
    fn upgrade_dependency(
        &self,
        state: &mut BindingState,
        route: &Route,
        callsite_id: &str,
        c: &mut SiteCensus,
    ) {
        let id = match &route.target {
            RouteTarget::Routine(id) => id.clone(),
            RouteTarget::AbiSymbol { key } => RoutineNodeId {
                object: ObjectNodeId {
                    app: key.app,
                    kind: object_kind_from_abi_type(&key.object_type),
                    key: if key.object_number != 0 {
                        ObjKey::Id(key.object_number)
                    } else {
                        ObjKey::Name(key.object_name_lc.as_str().into())
                    },
                },
                name_lc: key.routine_name_lc.as_str().into(),
                // ABI routines are object-level (`abi_ingest`).
                enclosing_member_lc: None,
                params_count: key.params_count,
                sig_fp: key.param_type_fp,
            },
            _ => return,
        };
        let node = self
            .graph
            .routines
            .binary_search_by(|probe| probe.id.cmp(&id))
            .ok()
            .map(|i| &self.graph.routines[i]);
        let (by_ref, counter): (Vec<bool>, &mut usize) = match (self.surface.get(&id), node) {
            (Some(meta), _) => (
                meta.params.iter().map(|p| p.by_ref).collect(),
                &mut c.adapter_dep_bindings_source,
            ),
            (None, Some(n)) => match &n.abi_params {
                AbiParams::Complete(ps) => (
                    ps.iter().map(|p| p.is_var).collect(),
                    &mut c.adapter_dep_bindings_symbol,
                ),
                // No trusted var-ness: change nothing. Count a site the
                // upgrade would have touched: an `"unresolved-callee"`
                // binding within the callee's arity (`UNKNOWN_ARITY`, a
                // real `Missing` routine's, covers every position).
                AbiParams::CollapsedUntrusted => {
                    if has_unresolved_below(state, id.params_count) {
                        c.adapter_dep_bindings_collapsed += 1;
                    }
                    return;
                }
                AbiParams::Missing => {
                    if has_unresolved_below(state, id.params_count) {
                        c.adapter_dep_bindings_missing += 1;
                    }
                    return;
                }
            },
            // No routine behind the route: the placeholder key of a run
            // into a dependency object with no entry trigger.
            (None, None) => {
                if has_unresolved_below(state, usize::MAX) {
                    c.adapter_dep_bindings_no_routine += 1;
                }
                return;
            }
        };
        let before = state.bindings.clone();
        let _ = upgrade_bindings_with(state, |i| by_ref.get(i).copied(), callsite_id);
        if state.bindings != before {
            *counter += 1;
        }
    }

    /// For a route into a dependency (a routine outside the workspace app,
    /// or an ABI symbol): `Some(type ref of its object)`; an ABI symbol in
    /// the workspace app (a run into an object with no entry trigger) is
    /// `Some(None)`. `None` for any other route.
    fn dependency_ref(&self, route: &Route) -> Option<Option<ExternalTypeRef>> {
        match &route.target {
            RouteTarget::Routine(id) if id.object.app != self.primary => {
                Some(self.type_ref(&object_key(&id.object)))
            }
            // A workspace object with no entry trigger (an opaque run into
            // our own app): no external type to name.
            RouteTarget::AbiSymbol { key } if key.app == self.primary => Some(None),
            RouteTarget::AbiSymbol { key } => {
                let okey = if key.object_number != 0 {
                    ObjKey::Id(key.object_number)
                } else {
                    ObjKey::Name(key.object_name_lc.as_str().into())
                };
                Some(self.type_ref(&(key.app, key.object_type.clone(), okey)))
            }
            _ => None,
        }
    }

    /// True when a member-lookup decline happened on an object outside the
    /// workspace: the route's receiver tier says so, or the resolver's receiver
    /// fact (S6.0) names an object the primary app does not declare, or a table
    /// that is unresolved or not the primary app's. Such a callee is a
    /// dependency callee.
    fn receiver_outside_workspace(&self, ce: &ClassifiedEdge, route: &Route) -> bool {
        use crate::program::resolve::full::ReceiverFactType;
        if let Some(tier) = route.receiver_tier {
            return tier != TrustTier::Workspace;
        }
        match self.receiver(ce).map(|f| &f.ty) {
            Some(ReceiverFactType::Object { kind, name }) => !self
                .primary_objects
                .contains(&(*kind, name.fold_identifier())),
            Some(ReceiverFactType::Record { table }) => {
                table.as_ref().is_none_or(|t| t.app != self.primary)
            }
            _ => false,
        }
    }

    /// The program routine a dependency route names: its own id for a source
    /// routine, rebuilt from the ABI key (as `abi_ingest` builds an ABI routine's
    /// id) for a symbol-only one. `None` for any other route.
    fn route_routine_id(route: &Route) -> Option<RoutineNodeId> {
        match &route.target {
            RouteTarget::Routine(id) => Some(id.clone()),
            RouteTarget::AbiSymbol { key } => Some(RoutineNodeId {
                object: ObjectNodeId {
                    app: key.app,
                    kind: object_kind_from_abi_type(&key.object_type),
                    key: if key.object_number != 0 {
                        ObjKey::Id(key.object_number)
                    } else {
                        ObjKey::Name(key.object_name_lc.as_str().into())
                    },
                },
                name_lc: key.routine_name_lc.as_str().into(),
                enclosing_member_lc: None,
                params_count: key.params_count,
                sig_fp: key.param_type_fp,
            }),
            _ => None,
        }
    }

    /// Record that site `site_id` (a call site, or a record operation) reaches
    /// the dependency routine `route` names (S3.3, S3.6), with that routine's
    /// body state from the registry. `false` when the route names no routine.
    fn record_target(&self, site_id: &str, route: &Route) -> bool {
        let Some(id) = Self::route_routine_id(route) else {
            return false;
        };
        let target = crate::program::model::model_routine_key(self.graph, &id);
        let body = self.registry.target(&id).map(|t| t.body);
        self.targets
            .borrow_mut()
            .push(crate::program::model::calls::ExternalTargetRef {
                callsite_id: site_id.to_string(),
                target,
                body,
            });
        true
    }

    /// The model's object id for a program object.
    fn model_object_id(&self, id: &ObjectNodeId) -> String {
        crate::program::model::model_object_id(self.graph, id)
    }

    /// An interface (Polymorphic) edge, converted from the PROGRAM engine alone
    /// (engine-switch S3.2; it used to read L3's receiver inference and symbol
    /// table):
    /// - one `Interface`+`Maybe` edge per workspace implementer, sorted by `to`;
    /// - one to-less `Interface`+`ExternalTarget` edge per DEPENDENCY implementer
    ///   object (its body is not in the model), sorted, with its
    ///   `external_type_ref` — these routes used to be dropped;
    /// - with no implementer reached, one to-less `Unknown(InterfaceNoImpl)` edge.
    ///
    /// `dispatch_meta` (on the first edge) names the interface the program
    /// resolver dispatched over (`ProgramReport::interface_sites`), counts every
    /// codeunit implementing it in the whole program, lists the ones no route
    /// reached as `"not-found"`, and lists the implementing enums. Bindings:
    /// ambiguous.
    fn interface(
        &self,
        r: &ModelRoutine,
        cs: &PCallSite,
        ce: &ClassifiedEdge,
        state: &mut BindingState,
        c: &mut SiteCensus,
    ) -> Option<Vec<CallEdge>> {
        mark_bindings_ambiguous(state);
        let mut callees: Vec<&ModelRoutine> = Vec::new();
        let mut dependency: Vec<ObjectNodeId> = Vec::new();
        for route in &ce.edge.routes {
            match (self.target(route), &route.target) {
                (Target::Model(m), _) => callees.push(m),
                // A workspace implementer with no model routine: the whole site
                // is an honest unknown (S3.1).
                (Target::MissingFromModel, _) => return None,
                (Target::Outside, RouteTarget::Routine(id)) => dependency.push(id.object.clone()),
                (Target::Outside, RouteTarget::AbiSymbol { key }) => {
                    dependency.push(ObjectNodeId {
                        app: key.app,
                        kind: object_kind_from_abi_type(&key.object_type),
                        key: if key.object_number != 0 {
                            ObjKey::Id(key.object_number)
                        } else {
                            ObjKey::Name(key.object_name_lc.as_str().into())
                        },
                    });
                }
                // An implementer the resolver could not resolve: it is listed in
                // `unresolved_impls` below (no route reached its object).
                (Target::Outside, RouteTarget::Unresolved | RouteTarget::Builtin(_)) => {}
            }
        }
        callees.sort_by(|a, b| a.id.cmp(&b.id));
        callees.dedup_by(|a, b| a.id == b.id);
        dependency.sort();
        dependency.dedup();
        // The site converts (no `?` below): record each dependency route's
        // routine (S3.3).
        for route in &ce.edge.routes {
            if !matches!(self.target(route), Target::Model(_)) {
                self.record_target(&cs.id, route);
            }
        }
        c.adapter_interface_dependency_impls += dependency.len();

        let name_lc = self
            .site_facts
            .get(&ce.obligation_id)
            .and_then(|f| f.interface.clone())
            .unwrap_or_default();
        let interface_name = self
            .graph
            .objects
            .iter()
            .filter(|o| o.id.kind == ObjectKind::Interface && o.name.fold_identifier() == name_lc)
            .min_by_key(|o| o.id.app != self.primary)
            .map_or_else(|| name_lc.clone(), |o| o.name.to_string());
        let implements = |kinds: &[ObjectKind]| -> Vec<&ObjectNode> {
            let mut v: Vec<&ObjectNode> = self
                .graph
                .objects
                .iter()
                .filter(|o| {
                    kinds.contains(&o.id.kind)
                        && o.implements.iter().any(|i| i.fold_identifier() == name_lc)
                })
                .collect();
            v.sort_by(|a, b| a.id.cmp(&b.id));
            v
        };
        let impls = implements(&[ObjectKind::Codeunit]);
        let reached = |o: &ObjectNode| {
            callees
                .iter()
                .any(|t| t.object_id == self.model_object_id(&o.id))
                || dependency.contains(&o.id)
        };
        let meta = DispatchMeta {
            interface_name,
            total_impls: impls.len(),
            unresolved_impls: impls
                .iter()
                .filter(|o| !reached(o))
                .map(|o| (self.model_object_id(&o.id), "not-found".to_string()))
                .collect(),
            enum_implementers: implements(&[ObjectKind::Enum, ObjectKind::EnumExtension])
                .iter()
                .map(|o| self.model_object_id(&o.id))
                .collect(),
        };
        let base = || {
            let mut e = CallEdge::base(&r.id, &cs.id, &cs.operation_id);
            e.dispatch_kind = DispatchKind::Interface;
            e
        };
        if callees.is_empty() && dependency.is_empty() {
            let mut e = base();
            e.resolution = Resolution::Unknown(ModelReason::InterfaceNoImpl);
            e.dispatch_meta = Some(meta);
            return Some(vec![e]);
        }
        let mut out: Vec<CallEdge> = callees
            .iter()
            .map(|t| {
                let mut e = base();
                e.to = Some(t.id.clone());
                e.resolution = Resolution::Maybe;
                e
            })
            .collect();
        out.extend(dependency.iter().map(|o| {
            let mut e = base();
            e.resolution = Resolution::ExternalTarget;
            e.external_type_ref = self.type_ref(&object_key(o));
            e
        }));
        out[0].dispatch_meta = Some(meta);
        Some(out)
    }

    /// A matched record op's program `ImplicitTrigger` edge → one edge per
    /// workspace trigger route, sorted by `to`. The program fan-out does not
    /// apply the site rules L3 and `implicit_trigger_route_applicable` apply,
    /// so they are applied here: an explicit `RunTrigger = false` fires
    /// nothing, and a `Validate` fires only its own field's `OnValidate`.
    /// `Validate` and `Rename` → `Resolved`, every other op → `Maybe`
    /// (`implicit_edges::trigger_mapping`).
    fn triggers(
        &self,
        r: &ModelRoutine,
        op: &ModelRecordOperation,
        ce: &ClassifiedEdge,
        c: &mut SiteCensus,
    ) -> Vec<CallEdge> {
        let is_validate = op.op.fold_identifier() == "validate";
        let field_lc = if is_validate {
            // The validated field: its first argument, quotes stripped, doubled
            // quotes collapsed, folded (the same normalisation as
            // `TriggerSiteRule`'s `validate_field`).
            op.field_arguments
                .as_ref()
                .and_then(|fa| fa.first())
                .map(|f| {
                    crate::program::body::node_util::strip_quotes(f)
                        .replace("\"\"", "\"")
                        .fold_identifier()
                })
        } else {
            None
        };
        let mut tos: Vec<String> = Vec::new();
        for route in &ce.edge.routes {
            // S3.6: a trigger outside the model (a dependency table's, in a
            // single-app model) keeps its identity and body state, but gets no
            // edge, and L3 never had an edge for it either. Whether such a
            // trigger should make the routine uncertain is a detector decision
            // (S8). In a cross-app model (S7.3) a dependency's trigger is a model
            // routine and gets its edge.
            let in_dependency = match (self.target(route), &route.target) {
                (Target::Model(_) | Target::MissingFromModel, _) => false,
                (Target::Outside, RouteTarget::Routine(_) | RouteTarget::AbiSymbol { .. }) => true,
                (Target::Outside, _) => false,
            };
            if in_dependency && self.record_target(&op.id, route) {
                c.adapter_trigger_dependency_routes += 1;
                continue;
            }
            let RouteTarget::Routine(id) = &route.target else {
                c.adapter_routes_dropped += 1;
                continue;
            };
            // S3.4: the program resolver applies the site rules
            // (`TriggerSiteRule`); this only cross-checks that it agrees with the
            // model's own record operation (expected 0), and filters nothing.
            let applies = op.run_trigger != Some(false)
                && (!is_validate
                    || (field_lc.is_some()
                        && id.enclosing_member_lc.as_deref() == field_lc.as_deref()));
            if !applies {
                c.adapter_trigger_routes_filtered += 1;
            }
            match self.l3_routine(id) {
                Some(t) => tos.push(t.id.clone()),
                None => c.adapter_routes_dropped += 1,
            }
        }
        tos.sort();
        tos.dedup();
        // Whether the trigger runs every time the op does: decided ONCE, here,
        // from the op and its literal RunTrigger (#96 D1); the typed graph
        // copies it. `resolution` below is L4's older, coarser rule and stays
        // as it is, so L4 does not move.
        let fires = crate::engine::l4::combined_graph::trigger_fires(&op.op, op.run_trigger);
        tos.into_iter()
            .map(|to| {
                let mut e = CallEdge::base(&r.id, &op.id, &op.id);
                e.to = Some(to);
                e.dispatch_kind = DispatchKind::ImplicitTrigger;
                e.fires = Some(fires);
                // Same rule as `implicit_edges::trigger_mapping`: Validate and
                // Rename always fire their trigger (Rename takes no RunTrigger).
                e.resolution = if is_validate || op.op.fold_identifier() == "rename" {
                    Resolution::Resolved
                } else {
                    Resolution::Maybe
                };
                e
            })
            .collect()
    }
}

/// True when an `"unresolved-callee"` binding sits at a position below
/// `arity` (the bindings are positional, as `upgrade_bindings` reads them).
fn has_unresolved_below(state: &BindingState, arity: usize) -> bool {
    state
        .bindings
        .iter()
        .enumerate()
        .any(|(i, b)| i < arity && b.binding_resolution == "unresolved-callee")
}

/// The member-lookup declines: a receiver object was found but the member
/// was not (absent, wrong arity, not visible, or ambiguous).
fn member_reason(reason: PReason) -> bool {
    use PReason as P;
    matches!(
        reason,
        P::MemberNotFound
            | P::ArityMismatch
            | P::ProtectedNotVisible
            | P::LocalNotVisible
            | P::InternalNotVisible
            | P::OverloadAmbiguous
    )
}

/// A program `UnknownReason` (other than the dependency-callee ones) → the
/// nearest L3 resolution. Ambiguity and absent/invisible members map to
/// L3's `Ambiguous` and `MemberNotFound` (they decide the uncertainty kind);
/// every other reason is `Unknown(_)`, whose L3 reason is diagnostic only.
fn map_unknown(reason: PReason) -> Resolution {
    use PReason as P;
    match reason {
        P::OverloadAmbiguous => Resolution::Ambiguous,
        P::ArityMismatch
        | P::MemberNotFound
        | P::ProtectedNotVisible
        | P::LocalNotVisible
        | P::InternalNotVisible => Resolution::MemberNotFound,
        P::CompoundReceiver => Resolution::Unknown(ModelReason::CompoundReceiver),
        P::UntrackedReceiver | P::ReceiverOutOfClosure => {
            Resolution::Unknown(ModelReason::UntrackedReceiver)
        }
        P::CatalogMiss => Resolution::Unknown(ModelReason::FrameworkMethodNotInCatalog),
        P::BuiltinPrecedenceCollision
        | P::WithScopeGuard
        | P::CodeunitTableNoExcluded
        | P::ReportRecExcluded => Resolution::Unknown(ModelReason::BareUnresolved),
        P::UnclassifiedCallee
        | P::IndexIntegrationGap
        | P::ObjectNotInGraph
        | P::AbiCollapsedOverload => Resolution::Unknown(ModelReason::CalleeUnknown),
    }
}

/// Build both models for `workspace` and run the adapter; return its census
/// (the [`site_census`] counts plus the `adapter_*` counts). The program
/// context uses the `Summary` dependency profile, as analyze's build does:
/// the resolver resolves workspace files only, so dependency bodies are not
/// read. Both models are resident together, as they are in `analyze` while
/// the adapter runs (analyze then drops them before the detector context).
pub fn adapter_census_for_workspace(workspace: &std::path::Path) -> Result<SiteCensus, String> {
    let (ctx, report, l3) = build_models(workspace)?;
    Ok(resolved_calls_from_program(report.edge_view(), &ctx, &l3.workspace, true).1)
}

pub(crate) fn build_models(
    workspace: &std::path::Path,
) -> Result<
    (
        ProgramContext,
        ProgramReport,
        crate::program::model::workspace::ModelRows,
    ),
    String,
> {
    build_models_with(&crate::program::inputs::InputSet::read(
        workspace,
        &Default::default(),
    ))
}

/// [`build_models`] over one already-read `InputSet`: the snapshot and the
/// model's file selection read the same set, so the workspace is read once.
pub(crate) fn build_models_with(
    inputs: &std::sync::Arc<crate::program::inputs::InputSet>,
) -> Result<
    (
        ProgramContext,
        ProgramReport,
        crate::program::model::workspace::ModelRows,
    ),
    String,
> {
    use crate::program::resolve::full::fresh_program_from_snapshot;
    let workspace = inputs.workspace.as_path();
    let (snap, _load) = crate::snapshot::SnapshotBuilder {
        workspace_root: workspace.to_path_buf(),
        local_providers: vec![],
    }
    .build_with_inputs(
        inputs,
        crate::snapshot::DependencySource::default(),
        &crate::program::dep_cache::DepCache::default(),
    )
    .map_err(|e| format!("snapshot build failed: {e:#}"))?;
    let (ctx, report) = fresh_program_from_snapshot(snap)?;
    // The production model (`assemble_and_resolve_workspace_program`), built from
    // the same program parse and resolution.
    let l3 = crate::program::model::workspace::assemble_and_resolve_workspace_from_program(
        inputs,
        crate::program::model::workspace::MODEL_INSTANCE_ID_DEFAULT,
        false,
        &ctx,
        &report.parenless_calls,
    )
    .ok_or_else(|| {
        format!(
            "L3 model: fail-closed/empty layout at {}",
            workspace.display()
        )
    })?;
    Ok((ctx, report, l3))
}

// ---------------------------------------------------------------------------
// #160 steps 10 + 11a, Task 1: test-only helpers and the old-vs-new oracles
// (spec §5.1 T1, §5.7 T7). The oracles compare on PRISTINE assembly and are
// deleted at Task 3 with the code they compare.
// ---------------------------------------------------------------------------

/// `u32::MAX - e` for every expression index (#160 §5.3): an index no
/// expression has, keeping distinct indices distinct and a record op's key equal
/// to its own operation site's.
#[cfg(test)]
pub(crate) fn unkey(exprs: &mut [u32]) {
    assert!(
        exprs.iter().all(|&e| e < u32::MAX / 2),
        "precondition: every expression index is below u32::MAX / 2"
    );
    for e in exprs {
        *e = u32::MAX - *e;
    }
}

/// The call pairs the PRODUCTION join accepts over `rows`: `(row, call site)`
/// -> program edge. Production joins by span until Task 2 makes this the key
/// join, with `rows.row_nodes` and `demand`.
#[cfg(test)]
pub(crate) fn accepted_call_pairs<'a>(
    view: EdgeView<'a>,
    ctx: &ProgramContext,
    rows: &crate::program::model::workspace::ModelRows,
    demand: Option<&HashSet<RoutineNodeId>>,
) -> HashMap<(usize, usize), &'a ClassifiedEdge> {
    let _ = (&rows.row_nodes, demand);
    join(view, ctx, &rows.workspace).calls
}

/// One `(row, site)` whose accepted program edge differs between the span join
/// and the key join (T1).
#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct PairDiff {
    /// `"call"` (a call site) or `"op"` (a record op).
    pub kind: &'static str,
    pub ri: usize,
    pub ix: usize,
    pub row_name: String,
    pub site_id: String,
    pub unit: String,
    pub start_line: u32,
    pub start_col: u32,
    /// The first route's routine target of each side's edge (`None`: no edge,
    /// or not a routine route).
    pub span_target: Option<RoutineNodeId>,
    pub key_target: Option<RoutineNodeId>,
    pub span_paired: bool,
    pub key_paired: bool,
}

/// T1's three comparisons (#160 §5.1).
#[cfg(test)]
pub(crate) struct ParityReport {
    /// Accepted pairs that differ, row-indexed, sorted.
    pub pairs: Vec<PairDiff>,
    /// Accepted call / op pairs on the span side (the "is anything compared?"
    /// precondition).
    pub span_call_pairs: usize,
    pub span_op_pairs: usize,
    /// Converted output groups (by call-site / op id) that differ.
    pub conversion: Vec<String>,
    pub groups: usize,
    /// Both joins' censuses after conversion (adapter counters included).
    pub span_census: SiteCensus,
    pub key_census: SiteCensus,
}

/// The span join under the 10A-2 demand filter (§4.5), for T1's census
/// comparison under one demand setting (the production span join stays
/// unfiltered until deleted).
#[cfg(test)]
fn span_join_under_demand<'a>(
    view: EdgeView<'a>,
    ctx: &ProgramContext,
    ws: &ModelEntities,
    demand: Option<&HashSet<RoutineNodeId>>,
) -> Join<'a> {
    span_join(view, ctx, ws, demand)
}

/// T1 (#160 §5.1): both joins and both conversions on the SAME pristine model.
#[cfg(test)]
pub(crate) fn key_join_parity(
    view: EdgeView<'_>,
    ctx: &ProgramContext,
    rows: &crate::program::model::workspace::ModelRows,
    abi_rows: Option<&crate::program::model::workspace::AbiRowIds>,
    demand: Option<&HashSet<RoutineNodeId>>,
) -> ParityReport {
    use std::collections::{BTreeMap, BTreeSet};
    let ws = &rows.workspace;
    let span = span_join_under_demand(view, ctx, ws, demand);
    let key = key_join(view, ctx, ws, &rows.row_nodes, demand);
    let target = |ce: Option<&&ClassifiedEdge>| {
        ce.and_then(|ce| ce.edge.routes.first())
            .and_then(|r| match &r.target {
                RouteTarget::Routine(id) => Some(id.clone()),
                _ => None,
            })
    };
    let mut pairs = Vec::new();
    for (kind, a, b) in [
        ("call", &span.calls, &key.calls),
        ("op", &span.ops, &key.ops),
    ] {
        let at: BTreeSet<(usize, usize)> = a.keys().chain(b.keys()).copied().collect();
        for (ri, ix) in at {
            let (x, y) = (a.get(&(ri, ix)), b.get(&(ri, ix)));
            let same = match (x, y) {
                (Some(x), Some(y)) => std::ptr::eq(*x, *y),
                (None, None) => true,
                _ => false,
            };
            if same {
                continue;
            }
            let r = &ws.routines[ri];
            let (site_id, anchor) = if kind == "call" {
                let cs = &r.call_sites[ix];
                (cs.id.clone(), &cs.source_anchor)
            } else {
                let op = &r.record_operations[ix];
                (op.id.clone(), &op.source_anchor)
            };
            pairs.push(PairDiff {
                kind,
                ri,
                ix,
                row_name: r.name.clone(),
                site_id,
                unit: anchor.source_unit_id.clone(),
                start_line: anchor.start_line,
                start_col: anchor.start_column,
                span_target: target(x),
                key_target: target(y),
                span_paired: x.is_some(),
                key_paired: y.is_some(),
            });
        }
    }
    pairs.sort();
    let (span_call_pairs, span_op_pairs) = (span.calls.len(), span.ops.len());
    let (span_calls, span_census) = convert(span, view, ctx, ws, true, abi_rows);
    let (key_calls, key_census) = convert(key, view, ctx, ws, true, abi_rows);
    let groups = |c: &ResolvedCalls| {
        let mut g: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for e in &c.edges {
            g.entry(e.callsite_id.clone())
                .or_default()
                .push(format!("edge {e:?}"));
        }
        for t in &c.external_targets {
            g.entry(t.callsite_id.clone())
                .or_default()
                .push(format!("target {t:?}"));
        }
        for (id, b) in &c.upgraded_bindings {
            g.entry(id.clone())
                .or_default()
                .push(format!("bindings {b:?}"));
        }
        g
    };
    let (gs, gk) = (groups(&span_calls), groups(&key_calls));
    let ids: BTreeSet<&String> = gs.keys().chain(gk.keys()).collect();
    let conversion = ids
        .iter()
        .filter(|id| gs.get(**id) != gk.get(**id))
        .map(|id| format!("{id}: span {:?} / key {:?}", gs.get(*id), gk.get(*id)))
        .collect();
    ParityReport {
        pairs,
        span_call_pairs,
        span_op_pairs,
        conversion,
        groups: gs.len(),
        span_census,
        key_census,
    }
}

/// T7 (#160 §5.7): over J2's whole domain (every node of a model app), today's
/// `Converter::l3_routine` answer (surface declaration -> first row at that
/// anchor) against `rows.row_nodes.j2_row`. Returns the nodes either side maps
/// and the differences.
#[cfg(test)]
pub(crate) fn j2_differences(
    ctx: &ProgramContext,
    rows: &crate::program::model::workspace::ModelRows,
) -> (usize, Vec<String>) {
    let ws = &rows.workspace;
    let graph = ctx.graph();
    let primary = ctx.primary_app_ref;
    let surface = ctx.decl_surface();
    let apps = model_apps(graph, primary, ws);
    let mut by_decl: HashMap<(&str, u32, u32), u32> = HashMap::new();
    for (ri, r) in ws.routines.iter().enumerate() {
        let a = &r.source_anchor;
        let unit = a
            .source_unit_id
            .strip_prefix("ws:")
            .unwrap_or(&a.source_unit_id);
        by_decl
            .entry((unit, a.start_line, a.start_column))
            .or_insert(u32::try_from(ri).expect("row index fits u32"));
    }
    let (mut mapped, mut diffs) = (0, Vec::new());
    for n in &graph.routines {
        let id = &n.id;
        if !apps.contains(&id.object.app) {
            continue;
        }
        let old = surface.get_with_path(id).and_then(|(meta, path)| {
            let unit = model_unit(graph, primary, id.object.app, path);
            by_decl
                .get(&(
                    unit.as_str(),
                    meta.origin.start.row,
                    meta.origin.start.column,
                ))
                .copied()
        });
        let new = rows.row_nodes.j2_row.get(id).copied();
        if old.is_some() || new.is_some() {
            mapped += 1;
        }
        if old != new {
            diffs.push(format!("{id:?}: J2 {old:?}, j2_row {new:?}"));
        }
    }
    (mapped, diffs)
}

/// The workspaces T1/T7/T8 compare on (#160 §5.1): every `tests/r0-corpus` dir
/// with a root `app.json`, and every `tests/fixtures` dir (to depth 3) with one.
#[cfg(test)]
pub(crate) fn oracle_fixture_workspaces() -> Vec<std::path::PathBuf> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut dirs: Vec<std::path::PathBuf> = std::fs::read_dir(root.join("tests/r0-corpus"))
        .expect("read tests/r0-corpus")
        .map(|e| e.expect("dir entry").path())
        .filter(|p| p.join("app.json").is_file())
        .collect();
    for e in walkdir::WalkDir::new(root.join("tests/fixtures")).max_depth(3) {
        let e = e.expect("walk tests/fixtures");
        if e.file_type().is_dir() && e.path().join("app.json").is_file() {
            dirs.push(e.path().to_path_buf());
        }
    }
    dirs.sort();
    dirs
}

/// Run `f` on the single-app build of `ws` (as `alsem analyze --single-app`
/// builds it) and on its cross-app build (`cross_app_assemble`), with the
/// universe's label, its abi rows and its demand. A failed build is `Err`.
#[cfg(test)]
pub(crate) fn for_both_universes(
    ws: &std::path::Path,
    mut f: impl FnMut(
        &str,
        EdgeView<'_>,
        &ProgramContext,
        &crate::program::model::workspace::ModelRows,
        Option<&crate::program::model::workspace::AbiRowIds>,
        Option<&HashSet<RoutineNodeId>>,
    ),
) -> Result<(), String> {
    let (ctx, report, rows) = build_models(ws)?;
    f("single-app", report.edge_view(), &ctx, &rows, None, None);
    drop((ctx, report, rows));
    let a = cross_app_assemble(
        &crate::program::inputs::InputSet::read(ws, &Default::default()),
        crate::program::model::workspace::MODEL_INSTANCE_ID_DEFAULT,
        false,
    )
    .map_err(|e| format!("cross-app build: {e:?}"))?;
    let body = a.program.body();
    f(
        "cross-app",
        body.edge_view(),
        &body.ctx,
        &a.rows,
        Some(&a.abi_rows),
        Some(&a.demand),
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Final panel r1 (F2-7): the `model` census charges the workspace
    /// diagnostics buffer (#160 §4.2.4). Two models that differ only in it.
    #[test]
    fn census_model_rows_counts_the_workspace_diagnostics() {
        use crate::engine::l5::registry::Diagnostic;
        use crate::program::model::workspace::{Model, ModelEntities};
        let model = |n: usize| Model {
            workspace: ModelEntities {
                objects: vec![],
                tables: vec![],
                routines: vec![],
            },
            root_classifications: vec![],
            primary_app: None,
            infra_diagnostics: vec![],
            workspace_diagnostics: vec![
                Diagnostic {
                    severity: "info".to_string(),
                    stage: "discover".to_string(),
                    message: "m".to_string(),
                };
                n
            ],
            dependency_diagnostics: vec![],
            calls: Default::default(),
            events: Default::default(),
        };
        let (none, three) = (census_model_rows(&model(0)), census_model_rows(&model(3)));
        assert_eq!(
            three.bytes - none.bytes,
            3 * std::mem::size_of::<Diagnostic>()
        );
    }

    /// #96: every record op the program resolver emits an `ImplicitTrigger` edge
    /// for passes the adapter's gate, the bulk `ModifyAll`/`DeleteAll` included
    /// (`op_name` folds the call text to these spellings).
    #[test]
    fn can_fire_trigger_admits_the_resolver_ops() {
        for op in [
            "insert",
            "modify",
            "delete",
            "modifyall",
            "deleteall",
            "validate",
            "rename",
        ] {
            assert!(can_fire_trigger(op), "{op}");
        }
        for op in ["get", "findset", "calcfields", "testfield"] {
            assert!(!can_fire_trigger(op), "{op}");
        }
        assert_eq!(op_name("T.ModifyAll(Amount, 0, true)"), "modifyall");
    }

    const APP_JSON: &str = r#"{"id":"b3b3b3b3-0000-0000-0000-000000000001","name":"B3 Census","publisher":"T","version":"1.0.0.0"}"#;

    /// Where `model_text` puts `dependency_diagnostics`.
    const DEPENDENCY_DIAGNOSTICS_AT: usize = 3;

    /// Everything a consumer reads from a model, as `Debug` text, minus
    /// `primary_app` (its name/publisher/version come from `app.json`, which the
    /// inline builder writes itself; the guid is compared).
    fn model_text(m: &crate::program::model::workspace::Model) -> Vec<String> {
        let mut out = vec![
            format!("{:?}", m.primary_app.as_ref().map(|a| &a.app_guid)),
            format!("{:?}", m.root_classifications),
            format!("{:?}", m.infra_diagnostics),
            format!("{:?}", m.dependency_diagnostics),
            format!("{:?}", m.events),
        ];
        out.extend(m.workspace.objects.iter().map(|o| format!("{o:?}")));
        out.extend(m.workspace.tables.iter().map(|t| format!("{t:?}")));
        out.extend(m.workspace.routines.iter().map(|r| format!("{r:?}")));
        out.extend(m.calls.edges.iter().map(|e| format!("{e:?}")));
        out
    }

    /// S9.2: the inline builder is the disk builder. Every single-app r0-corpus
    /// fixture (one root `app.json`, no `.alpackages`), passed inline with its
    /// relative paths, gives the same model as the fixture built from disk.
    #[test]
    fn the_inline_program_model_is_the_disk_model() {
        let corpus = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/r0-corpus");
        let mut compared = 0;
        let mut warned = 0;
        for entry in std::fs::read_dir(&corpus).unwrap() {
            let ws = entry.unwrap().path();
            let Ok(app) = std::fs::read_to_string(ws.join("app.json")) else {
                continue;
            };
            let mut files = Vec::new();
            let mut single_app = !ws.join(".alpackages").exists();
            for f in walkdir::WalkDir::new(&ws).sort_by_file_name() {
                let f = f.unwrap();
                let rel = f.path().strip_prefix(&ws).unwrap();
                if f.file_name() == "app.json" && rel != std::path::Path::new("app.json") {
                    single_app = false;
                }
                if f.path().extension().is_some_and(|e| e == "al") {
                    let rel = rel.to_string_lossy().replace('\\', "/");
                    files.push((rel, std::fs::read_to_string(f.path()).unwrap()));
                }
            }
            if !single_app || files.is_empty() {
                continue;
            }
            let manifest: serde_json::Value = serde_json::from_str(&app).unwrap();
            let guid = manifest["id"].as_str().unwrap();
            // A fixture that declares dependencies, or a platform/application
            // (implicit Microsoft ones), and ships no packages must get warnings.
            let declares = manifest["dependencies"]
                .as_array()
                .is_some_and(|d| !d.is_empty())
                || manifest.get("platform").is_some()
                || manifest.get("application").is_some();
            let must_warn = declares && !ws.join(".alpackages").exists();
            let inputs = crate::program::inputs::InputSet::read(&ws, &Default::default());
            let disk = assemble_and_resolve_workspace_program(&inputs, "r0", true).expect("disk");
            let inline = assemble_and_resolve_inline_program(&files, guid, "r0");
            let mut disk_text = model_text(&disk);
            let inline_text = model_text(&inline);
            // The inline builder writes an app.json with no dependencies, so it
            // has no dependency ledger to warn about. A fixture that declares
            // dependencies (or a platform/application, which add implicit ones)
            // gets "is missing" warnings on disk, as none has a package here.
            // Check that, then compare the field as the inline model has it.
            assert_eq!(inline.dependency_diagnostics, [], "{}", ws.display());
            let disk_deps = &disk.dependency_diagnostics;
            assert!(
                disk_deps.iter().all(|d| d.severity == "warning"
                    && d.stage == "dependencies"
                    && d.message.ends_with("is missing")),
                "{}: {disk_deps:?}",
                ws.display()
            );
            assert!(
                !must_warn || !disk_deps.is_empty(),
                "{}: declares dependencies with no packages, so the disk model must warn",
                ws.display()
            );
            warned += usize::from(must_warn);
            disk_text[DEPENDENCY_DIAGNOSTICS_AT] = inline_text[DEPENDENCY_DIAGNOSTICS_AT].clone();
            assert_eq!(inline_text, disk_text, "{}", ws.display());
            compared += 1;
        }
        assert!(compared >= 20, "only {compared} fixtures compared");
        // The dependency-warning check above must actually bite somewhere.
        assert!(
            warned >= 1,
            "no compared fixture declares a dependency without packages"
        );
    }

    /// The inputs inline tests pass: an app id that is no hex GUID, files at the
    /// root and in a folder. Units are spelled `ws:<path>`, calls resolve.
    #[test]
    fn inline_program_builder_takes_test_shaped_input() {
        let files = [
            (
                "A.Codeunit.al".to_string(),
                "codeunit 50100 A\n{\n    procedure P()\n    var\n        B: Codeunit B;\n    begin\n        B.Q();\n    end;\n}\n".to_string(),
            ),
            (
                "src/B.Codeunit.al".to_string(),
                "codeunit 50101 B\n{\n    procedure Q()\n    begin\n    end;\n}\n".to_string(),
            ),
        ];
        let m = assemble_and_resolve_inline_program_default(
            &files,
            "11111111-0000-0000-0000-0000000g1abc",
        );
        let units: Vec<&str> = m
            .workspace
            .routines
            .iter()
            .map(|r| r.source_anchor.source_unit_id.as_str())
            .collect();
        assert_eq!(units, ["ws:A.Codeunit.al", "ws:src/B.Codeunit.al"]);
        assert!(
            m.workspace
                .routines
                .iter()
                .all(|r| r.app_guid == "11111111-0000-0000-0000-0000000g1abc")
        );
        let calls = &m.calls.edges;
        assert!(calls.iter().any(|e| e.to.is_some()), "{calls:?}");
    }

    /// Write `files` (relative path, bytes) under a fresh workspace with a root
    /// `app.json`, then run the census.
    fn census(files: &[(&str, &[u8])]) -> SiteCensus {
        census_with(files, |_| {})
    }

    /// Code review CR4: `build_models` reads the workspace once. A file removed
    /// after the read is still in the snapshot AND the model's selection (both
    /// read the set); a second disk read for the snapshot would lack it and the
    /// model would fail closed (`ProgramFiles::Missing`).
    #[test]
    fn build_models_reads_the_workspace_once() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("app.json"), APP_JSON).unwrap();
        std::fs::write(dir.path().join("A.al"), "codeunit 50100 A { }").unwrap();
        std::fs::write(dir.path().join("B.al"), "codeunit 50101 B { }").unwrap();
        let inputs = crate::program::inputs::InputSet::read(dir.path(), &Default::default());
        std::fs::remove_file(dir.path().join("B.al")).unwrap();
        let built = build_models_with(&inputs);
        assert!(built.is_ok(), "{:?}", built.err());
    }

    /// [`census`], with `mutate` applied to the L3 model before the census:
    /// the way a test states a mismatch precondition by assignment.
    fn census_with(files: &[(&str, &[u8])], mutate: impl FnOnce(&mut ModelEntities)) -> SiteCensus {
        census_rows_with(files, |rows, _| mutate(&mut rows.workspace))
    }

    /// [`census_with`] whose `mutate` reaches the whole rows (`RowNodes`
    /// included) and sees the built context; the census reads the rows with
    /// their `RowNodes` (#160 §4.8), as production does from Task 2.
    fn census_rows_with(
        files: &[(&str, &[u8])],
        mutate: impl FnOnce(&mut crate::program::model::workspace::ModelRows, &ProgramContext),
    ) -> SiteCensus {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("app.json"), APP_JSON).unwrap();
        for (rel, bytes) in files {
            let p = dir.path().join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, bytes).unwrap();
        }
        let (ctx, report, mut l3) = build_models(dir.path()).unwrap();
        mutate(&mut l3, &ctx);
        site_census_rows(report.edge_view(), &ctx, &l3, None)
    }

    const TWO_CALLS: &str = "codeunit 50100 \"M\"\n{\n    procedure Foo()\n    begin\n    end;\n\n    procedure Bar()\n    begin\n    end;\n\n    procedure Caller()\n    begin\n        Foo();\n        Bar();\n    end;\n}\n";

    fn routine<'a>(
        ws: &'a mut ModelEntities,
        name: &str,
    ) -> &'a mut super::super::workspace::ModelRoutine {
        ws.routines.iter_mut().find(|r| r.name == name).unwrap()
    }

    /// Precondition by assignment: the L3 site at `Foo()`'s span claims
    /// callee text `Baz`, so the span pairs but the callee fingerprint does
    /// not.
    #[test]
    fn callee_text_disagreement_is_callee_fp_mismatch() {
        let c = census_with(&[("src/m.al", TWO_CALLS.as_bytes())], |ws| {
            let cs = &mut routine(ws, "Caller").call_sites[0];
            assert_eq!(cs.callee_text, "Foo");
            cs.callee_text = "Baz".to_string();
        });
        assert_eq!(
            counts(&c),
            SiteCensus {
                l3_call_sites: 2,
                program_sites: 2,
                matched: 1,
                callee_fp_mismatch: 1,
                ..Default::default()
            },
            "{c:#?}"
        );
    }

    /// Precondition by assignment: the L3 caller's declaration anchor is moved
    /// one column, so both its sites pair by span and callee but not by
    /// caller.
    #[test]
    fn caller_anchor_disagreement_is_caller_mismatch() {
        let c = census_with(&[("src/m.al", TWO_CALLS.as_bytes())], |ws| {
            routine(ws, "Caller").source_anchor.start_column += 1;
        });
        assert_eq!(
            counts(&c),
            SiteCensus {
                l3_call_sites: 2,
                program_sites: 2,
                caller_mismatch: 2,
                ..Default::default()
            },
            "{c:#?}"
        );
    }

    /// Precondition by assignment: the model's `Insert` and `SetRange` record
    /// ops and the `Commit` get keys no expression has (#160 §5.3: each
    /// expression index `e` of `P`'s 2 record ops and 3 operation sites becomes
    /// `u32::MAX - e`; it used to move their anchors, which no longer unpairs
    /// them), so no program edge sits at their keys. Insert is trigger-capable;
    /// SetRange and Commit are not. The program's record ops then find no model
    /// record op.
    #[test]
    #[ignore = "#160 Task 2: needs the key join in production (spec §4.12)"]
    fn l3_ops_with_no_program_site_are_counted_by_trigger_capability() {
        let cu = "codeunit 50101 \"C\"\n{\n    procedure P()\n    var\n        R: Record \"T\";\n    begin\n        R.SetRange(Code, 'X');\n        R.Insert(true);\n        Commit();\n    end;\n}\n";
        let table = "table 50100 \"T\"\n{\n    fields\n    {\n        field(1; Code; Code[20]) { }\n    }\n    trigger OnInsert()\n    begin\n    end;\n}\n";
        let c = census_with(
            &[("src/t.al", table.as_bytes()), ("src/c.al", cu.as_bytes())],
            |ws| {
                let r = routine(ws, "P");
                assert_eq!(r.record_operations.len(), 2);
                assert_eq!(r.operation_sites.len(), 3);
                unkey(&mut r.site_exprs.record_ops);
                unkey(&mut r.site_exprs.op_sites);
            },
        );
        assert_eq!(
            counts(&c),
            SiteCensus {
                program_sites: 3,
                operation_site: 2,
                implicit_trigger_unmatched: 1,
                l3_record_operations: 2,
                l3_operation_sites: 3,
                l3_op_no_program_site_trigger: 1,
                l3_op_no_program_site_other: 2,
                ..Default::default()
            },
            "{c:#?}"
        );
    }

    /// Precondition by assignment: a second model call site is a clone of the
    /// first, its span AND its key (#160 §5.3: the clone also copies its
    /// `site_exprs` entry). Both are classified (both `matched` against the one
    /// program edge) and the duplicate is counted.
    #[test]
    fn duplicate_l3_span_is_counted() {
        let c = census_with(&[("src/m.al", TWO_CALLS.as_bytes())], |ws| {
            let r = routine(ws, "Caller");
            r.call_sites[1] = r.call_sites[0].clone();
            // STEP-6: the walk stamps no key yet, so there may be none to copy.
            if let [first, ..] = r.site_exprs.calls[..] {
                r.site_exprs.calls[1] = first;
            }
        });
        assert_eq!(
            counts(&c),
            SiteCensus {
                l3_call_sites: 2,
                program_sites: 2,
                matched: 2,
                program_only_site: 1,
                duplicate_l3_span: 1,
                ..Default::default()
            },
            "{c:#?}"
        );
    }

    fn counts(c: &SiteCensus) -> SiteCensus {
        SiteCensus {
            unmatched: Vec::new(),
            ..c.clone()
        }
    }

    /// Non-ASCII text BEFORE a call on the same line: `Message('æøå');` puts
    /// 3 two-byte chars ahead of the second call, so a char- or UTF-16-column
    /// side would key it 3 columns off and it would not pair.
    #[test]
    fn non_ascii_before_call_on_same_line_pairs_by_byte_column() {
        let src = "codeunit 50100 \"Ærø CU\"\n{\n    procedure \"Kø Proc\"()\n    begin\n    end;\n\n    procedure Caller()\n    begin\n        Message('æøå'); \"Kø Proc\"(); Message('ü'); \"Kø Proc\"();\n    end;\n}\n";
        let c = census(&[("src/a.al", src.as_bytes())]);
        assert_eq!(
            counts(&c),
            SiteCensus {
                l3_call_sites: 4,
                program_sites: 4,
                matched: 4,
                ..Default::default()
            },
            "{c:#?}"
        );
    }

    /// Two nested same-callee calls on one line have different spans and pair
    /// one to one, with no positional tie-break.
    #[test]
    fn nested_same_callee_calls_on_one_line_pair_exactly() {
        let src = "codeunit 50100 \"N\"\n{\n    procedure F(I: Integer): Integer\n    begin\n        exit(I);\n    end;\n\n    procedure Caller()\n    begin\n        F(F(1)); F(F(F(2)));\n    end;\n}\n";
        let c = census(&[("src/a.al", src.as_bytes())]);
        assert_eq!(
            counts(&c),
            SiteCensus {
                l3_call_sites: 5,
                program_sites: 5,
                matched: 5,
                ..Default::default()
            },
            "{c:#?}"
        );
    }

    /// `Rec.Insert(true)` is an implicit-trigger record op on both sides;
    /// `SetRange` and `Commit` are operation sites with no L3 call analogue.
    #[test]
    fn implicit_trigger_and_operation_sites() {
        let table = "table 50100 \"T\"\n{\n    fields\n    {\n        field(1; Code; Code[20]) { }\n    }\n    trigger OnInsert()\n    begin\n    end;\n}\n";
        let cu = "codeunit 50101 \"C\"\n{\n    procedure P()\n    var\n        R: Record \"T\";\n    begin\n        R.SetRange(Code, 'X');\n        R.Insert(true);\n        Commit();\n        Message('m');\n    end;\n}\n";
        let c = census(&[("src/t.al", table.as_bytes()), ("src/c.al", cu.as_bytes())]);
        assert_eq!(
            counts(&c),
            SiteCensus {
                l3_call_sites: 1,
                program_sites: 4,
                matched: 1,
                operation_site: 2,
                implicit_trigger_matched: 1,
                l3_record_operations: 2,
                l3_operation_sites: 3,
                ..Default::default()
            },
            "{c:#?}"
        );
    }

    /// A UTF-8 BOM file. Both engines read source through the shared decoder
    /// (`crate::source_text`), which drops the BOM, so line-0 columns agree
    /// and the call on line 0 pairs like the one on a later line. (Before the
    /// shared decoder, the program engine kept the BOM and the line-0 call
    /// was unpaired on both sides — code map headline 4b.)
    #[test]
    fn bom_file_pairs_line_zero_calls() {
        let mut bytes = b"\xEF\xBB\xBF".to_vec();
        bytes.extend_from_slice(b"codeunit 50100 \"B\" { trigger OnRun() begin Message('a'); end;\n    procedure Q()\n    begin\n        Message('b');\n    end;\n}\n");
        let c = census(&[("src/b.al", &bytes)]);
        assert_eq!(
            counts(&c),
            SiteCensus {
                l3_call_sites: 2,
                program_sites: 2,
                matched: 2,
                ..Default::default()
            },
            "{c:#?}"
        );
    }

    /// A Windows-1252 byte (0xE6, "Kære") in a comment BEFORE a call on the
    /// same line. Both engines decode it to U+FFFD through the shared decoder,
    /// so the byte columns after it agree and the call pairs.
    #[test]
    fn non_utf8_byte_before_call_pairs() {
        let bytes = b"codeunit 50100 \"W\"\n{\n    procedure P()\n    begin\n        /* K\xE6re */ Message('a');\n    end;\n}\n";
        let c = census(&[("src/w.al", bytes)]);
        assert_eq!(
            counts(&c),
            SiteCensus {
                l3_call_sites: 1,
                program_sites: 1,
                matched: 1,
                ..Default::default()
            },
            "{c:#?}"
        );
    }

    /// A nested `app.json` hides its folder from L3 (app-scoped discovery),
    /// while the program engine walks it: its sites are program-only.
    #[test]
    fn nested_app_json_folder_is_program_only() {
        let a = "codeunit 50100 \"A\"\n{\n    procedure P()\n    begin\n        Message('a');\n    end;\n}\n";
        let b = "codeunit 50101 \"Bn\"\n{\n    procedure P()\n    begin\n        Message('b');\n    end;\n}\n";
        let nested_app = r#"{"id":"b3b3b3b3-0000-0000-0000-000000000002","name":"Nested","publisher":"T","version":"1.0.0.0"}"#;
        let c = census(&[
            ("src/a.al", a.as_bytes()),
            ("sub/app.json", nested_app.as_bytes()),
            ("sub/b.al", b.as_bytes()),
        ]);
        assert_eq!(
            counts(&c),
            SiteCensus {
                l3_call_sites: 1,
                program_sites: 2,
                matched: 1,
                program_only_site: 1,
                ..Default::default()
            },
            "{c:#?}"
        );
    }

    /// A bare implicit-`Rec` record op in a table: both engines make it a
    /// record op (S3.5; before, the program engine made it a bare call, the
    /// 571 `op_shape_mismatch` sites on CDO).
    #[test]
    fn bare_implicit_rec_op_is_a_record_op_in_both_engines() {
        let table = "table 50100 \"T\"\n{\n    fields\n    {\n        field(1; Code; Code[20]) { }\n    }\n    trigger OnModify()\n    begin\n    end;\n\n    procedure P()\n    begin\n        Modify();\n    end;\n}\n";
        let c = census(&[("src/t.al", table.as_bytes())]);
        assert_eq!(
            counts(&c),
            SiteCensus {
                program_sites: 1,
                implicit_trigger_matched: 1,
                l3_record_operations: 1,
                l3_operation_sites: 1,
                ..Default::default()
            },
            "{c:#?}"
        );
    }

    // -----------------------------------------------------------------------
    // #160 steps 10 + 11a (spec 2026-10-22): T4, T5, T9, T1, T7, T7a-c, T8.
    // -----------------------------------------------------------------------

    /// A workspace of `files` (relative path, text) with [`APP_JSON`], built as
    /// the single-app census builds it.
    fn single_app(
        files: &[(&str, &str)],
    ) -> (
        tempfile::TempDir,
        ProgramContext,
        ProgramReport,
        crate::program::model::workspace::ModelRows,
    ) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("app.json"), APP_JSON).unwrap();
        for (rel, text) in files {
            let p = dir.path().join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, text).unwrap();
        }
        let (ctx, report, rows) = build_models(dir.path()).unwrap();
        (dir, ctx, report, rows)
    }

    /// T4 (#160 §5.4): a copy of one program call-site edge (same site, same
    /// key) is a duplicate program key, counted and named as a loss.
    #[test]
    fn duplicate_program_key_is_counted() {
        let (_dir, ctx, mut report, rows) = single_app(&[("src/m.al", TWO_CALLS)]);
        let primary = report.primary_app_ref;
        let i = report
            .edges
            .iter()
            .position(|ce| {
                matches!(ce.obligation_id, ObligationId::CallSite { .. })
                    && ce.edge.from.object.app == primary
            })
            .unwrap();
        let copy = ClassifiedEdge {
            obligation_id: report.edges[i].obligation_id.clone(),
            edge: report.edges[i].edge.clone(),
            site: report.edges[i].site,
        };
        report.edges.push(copy);
        let c = site_census_rows(report.edge_view(), &ctx, &rows, None);
        assert_eq!(c.duplicate_program_span, 1, "{c:#?}");
        assert!(
            c.losses().contains(&("duplicate_program_span", 1)),
            "{:?}",
            c.losses()
        );
    }

    /// T4 (#160 §5.4): duplicates are counted per category. Two call sites of
    /// one row given one key by assignment are one duplicate; a record op and
    /// its own operation site at one key (the walk's normal shape) are none.
    #[test]
    #[ignore = "#160 Task 2: needs the key join in production (spec §4.12)"]
    fn duplicate_model_key_per_category() {
        let two_keys = census_with(&[("src/m.al", TWO_CALLS.as_bytes())], |ws| {
            let r = routine(ws, "Caller");
            // STEP-6: the walk stamps no key yet, so there may be none to copy.
            if let [first, ..] = r.site_exprs.calls[..] {
                r.site_exprs.calls[1] = first;
            }
        });
        assert_eq!(
            two_keys.duplicate_l3_span, 1,
            "two call sites at one key are one duplicate: {two_keys:#?}"
        );
        let table = "table 50100 \"T\"\n{\n    fields\n    {\n        field(1; Code; Code[20]) { }\n    }\n    trigger OnInsert()\n    begin\n    end;\n}\n";
        let cu = "codeunit 50101 \"C\"\n{\n    procedure P()\n    var\n        R: Record \"T\";\n    begin\n        R.Insert(true);\n    end;\n}\n";
        let shared = census_with(
            &[("src/t.al", table.as_bytes()), ("src/c.al", cu.as_bytes())],
            |ws| {
                let r = routine(ws, "P");
                assert_eq!(
                    r.site_exprs.record_ops, r.site_exprs.op_sites,
                    "precondition: the record op and its own operation site share one key"
                );
            },
        );
        assert_eq!(shared.duplicate_l3_span, 0, "{shared:#?}");
    }

    /// T5 (#160 §5.5): a call site whose key is right and whose anchor is moved
    /// one column pairs by key, and the span debug assertion on the accepted
    /// pair panics.
    #[test]
    #[cfg(debug_assertions)]
    #[should_panic]
    #[ignore = "#160 Task 2: needs the key join in production (spec §4.12)"]
    fn span_assertion_fires_on_a_key_pair_whose_spans_differ() {
        let _ = census_with(&[("src/m.al", TWO_CALLS.as_bytes())], |ws| {
            routine(ws, "Caller").call_sites[0]
                .source_anchor
                .start_column += 1;
        });
    }

    /// T9 (#160 §5.9): the caller check reads the declaration anchor captured
    /// at assembly. Hand-stated: the program's declaration of `Caller` (read
    /// from the `DeclSurface`, as assembly captures it) is recorded one column
    /// off in `row_nodes.decl_anchor`; the row is untouched. Its sites are
    /// `caller_mismatch`.
    #[test]
    #[ignore = "#160 Task 2: needs the key join in production (spec §4.12)"]
    fn caller_check_reads_captured_decl_anchor() {
        let c = census_rows_with(&[("src/m.al", TWO_CALLS.as_bytes())], |rows, ctx| {
            let (line, col) = {
                let row = rows
                    .workspace
                    .routines
                    .iter()
                    .find(|r| r.name == "Caller")
                    .unwrap();
                (row.source_anchor.start_line, row.source_anchor.start_column)
            };
            let node = ctx
                .graph()
                .routines
                .iter()
                .find(|n| n.id.object.app == ctx.primary_app_ref && &*n.id.name_lc == "caller")
                .expect("precondition: a program node for Caller")
                .id
                .clone();
            let surface = ctx.decl_surface();
            let (meta, path) = surface
                .get_with_path(&node)
                .expect("precondition: Caller has a program declaration");
            let file = ctx
                .file_ix(ctx.primary_app_ref, path)
                .expect("precondition: its file has a FileIx");
            let (dl, dc) = (meta.origin.start.row, meta.origin.start.column);
            assert_eq!(
                (dl, dc),
                (line, col),
                "precondition: the row's anchor is the program's declaration"
            );
            rows.row_nodes.decl_anchor.insert(node, (file, dl, dc + 1));
        });
        assert_eq!(
            counts(&c),
            SiteCensus {
                l3_call_sites: 2,
                program_sites: 2,
                caller_mismatch: 2,
                ..Default::default()
            },
            "{c:#?}"
        );
    }

    /// T1 (#160 §5.1): on every fixture, both universes, the key join accepts
    /// the same pairs as the span join, converts to the same calls and gives
    /// the same census (P2: 0 differences on these dirs).
    #[test]
    fn key_join_equals_span_join_on_the_fixtures() {
        let (mut runs, mut call_pairs) = (0, 0);
        let mut failures = Vec::new();
        for ws in oracle_fixture_workspaces() {
            let built = for_both_universes(&ws, |u, view, ctx, rows, abi, demand| {
                let p = key_join_parity(view, ctx, rows, abi, demand);
                runs += 1;
                call_pairs += p.span_call_pairs;
                if let Some(d) = p.pairs.first() {
                    failures.push(format!(
                        "{} ({u}): {} accepted pairs differ, first {d:?}",
                        ws.display(),
                        p.pairs.len()
                    ));
                }
                if let Some(g) = p.conversion.first() {
                    failures.push(format!(
                        "{} ({u}): {} converted groups differ, first {g}",
                        ws.display(),
                        p.conversion.len()
                    ));
                }
                if counts(&p.span_census) != counts(&p.key_census) {
                    failures.push(format!(
                        "{} ({u}): census differs: span {:?} / key {:?}",
                        ws.display(),
                        counts(&p.span_census),
                        counts(&p.key_census)
                    ));
                }
            });
            if let Err(e) = built {
                failures.push(format!("{}: {e}", ws.display()));
            }
        }
        assert!(runs > 200, "fixture precondition: {runs} runs");
        assert!(call_pairs > 0, "fixture precondition: accepted pairs exist");
        assert!(
            failures.is_empty(),
            "{} key/span differences:\n{}",
            failures.len(),
            failures.join("\n")
        );
    }

    /// R-PAIRS (#160 §3.5): `(row name, site index, virtual path, start line,
    /// start col)` of the 23 call sites whose accepted pair differs, key vs
    /// span, on cross-app CDO.
    fn r_pairs() -> std::collections::BTreeSet<(String, String, String, u32, u32)> {
        let token = "src/.dependencies/CC/Table/AccessToken.Table.al";
        let session = "src/Source/Core/CoreSessionManager.Codeunit.al";
        let pages = "src/Utilities/PageManagement.Codeunit.al";
        let rows: &[(&str, &str, &str, u32, u32)] = &[
            ("AcquireTokenFromCache", "cs0", token, 64, 8),
            ("AcquireTokenFromCache", "cs1", token, 65, 8),
            ("AcquireTokenFromCache", "cs2", token, 67, 8),
            ("AcquireTokenFromCache", "cs3", token, 72, 42),
            ("AcquireTokenFromCache", "cs4", token, 74, 42),
            ("AcquireTokenFromCache", "cs5", token, 76, 24),
            ("AcquireTokenFromCache", "cs6", token, 77, 24),
            ("AcquireTokenFromCache", "cs7", token, 81, 45),
            ("GetAccessToken", "cs0", session, 59, 8),
            ("GetAccessToken", "cs1", session, 60, 8),
            ("GetAccessToken", "cs3", session, 73, 38),
            ("GetAccessToken", "cs4", session, 76, 11),
            ("GetAccessToken", "cs5", session, 81, 11),
            ("GetAccessToken", "cs6", session, 85, 11),
            ("GetAccessToken", "cs7", session, 89, 15),
            ("GetFreshAccessToken", "cs0", session, 101, 8),
            ("GetFreshAccessToken", "cs1", session, 103, 8),
            ("GetFreshAccessToken", "cs2", session, 104, 11),
            ("GetFreshAccessToken", "cs3", session, 107, 12),
            ("GetSalesHeaderPageID", "cs0", pages, 297, 8),
            ("GetSalesHeaderPageID", "cs1", pages, 298, 11),
            ("GetSalesHeaderPageID", "cs2", pages, 299, 19),
            ("GetSalesHeaderPageID", "cs3", pages, 299, 42),
        ];
        rows.iter()
            .map(|(n, s, p, l, c)| (n.to_string(), s.to_string(), p.to_string(), *l, *c))
            .collect()
    }

    /// A model unit's virtual path: `ws:` and `dep:<guid>:` dropped.
    fn unit_path(unit: &str) -> &str {
        if let Some(rest) = unit.strip_prefix("dep:") {
            rest.split_once(':').map_or(rest, |(_, p)| p)
        } else {
            unit.strip_prefix("ws:").unwrap_or(unit)
        }
    }

    /// T1 on CDO (#160 §5.1; skips without `CDO_WS`, panics under
    /// `ENFORCE_CDO_WS=1`): single-app identical; cross-app the differing
    /// accepted pairs are EXACTLY R-PAIRS, the two non-equivalent ones carry the
    /// stated target nodes, the conversion is identical, and the census moves by
    /// exactly the stated deltas.
    #[test]
    fn key_join_equals_span_join_on_cdo() {
        let Some(ws) = crate::cdo::cdo_ws_or_enforce() else {
            return;
        };
        let mut universes = Vec::new();
        for_both_universes(&ws, |u, view, ctx, rows, abi, demand| {
            let p = key_join_parity(view, ctx, rows, abi, demand);
            eprintln!(
                "key/span parity on CDO ({u}): {} call pairs, {} op pairs, {} differ, {} groups, {} differ",
                p.span_call_pairs,
                p.span_op_pairs,
                p.pairs.len(),
                p.groups,
                p.conversion.len()
            );
            universes.push(u.to_string());
            assert!(p.span_call_pairs > 0, "CDO precondition ({u}): pairs");
            assert_eq!(p.conversion, Vec::<String>::new(), "CDO ({u}): conversion");
            if u == "single-app" {
                assert_eq!(p.pairs, vec![], "CDO single-app: accepted pairs");
                assert_eq!(counts(&p.key_census), counts(&p.span_census));
                return;
            }
            assert!(
                p.pairs.iter().all(|d| d.kind == "call"),
                "CDO cross-app: no op pair differs"
            );
            let got: std::collections::BTreeSet<_> = p
                .pairs
                .iter()
                .map(|d| {
                    (
                        d.row_name.clone(),
                        d.site_id.rsplit('/').next().unwrap().to_string(),
                        unit_path(&d.unit).to_string(),
                        d.start_line,
                        d.start_col,
                    )
                })
                .collect();
            assert_eq!(got.len(), p.pairs.len(), "R-PAIRS identities are unique");
            assert_eq!(got, r_pairs(), "CDO cross-app: the differing pairs are R-PAIRS");
            for site in ["cs4", "cs6"] {
                let d = p
                    .pairs
                    .iter()
                    .find(|d| d.row_name == "GetAccessToken" && d.site_id.ends_with(site))
                    .unwrap();
                let (s, k) = (d.span_target.as_ref().unwrap(), d.key_target.as_ref().unwrap());
                assert_eq!((&*s.name_lc, s.params_count), ("acquiretokenfromcache", 5));
                assert_eq!((&*k.name_lc, k.params_count), ("acquiretokenfromcache", 5));
                assert_eq!(s.sig_fp, 9686704692977790200, "{site}: span target");
                assert_eq!(k.sig_fp, 8102086367325882817, "{site}: key target");
            }
            let mut want = counts(&p.span_census);
            assert_eq!(
                (want.duplicate_program_span, want.duplicate_l3_span),
                (24, 23),
                "CDO precondition: the span join's duplicates under demand (P1)"
            );
            want.duplicate_program_span = 0;
            want.duplicate_l3_span = 0;
            want.operation_site += 1;
            assert_eq!(counts(&p.key_census), want, "CDO cross-app: census deltas");
        })
        .expect("CDO builds");
        assert_eq!(universes, ["single-app", "cross-app"]);
    }

    /// T7 (#160 §5.7): `j2_row` is J2, node for node, on every fixture (both
    /// universes).
    #[test]
    fn j2_row_equals_decl_join() {
        let mut mapped = 0;
        let mut failures = Vec::new();
        // §5.7: the set includes T7b's cross-node shape, which no r0 or
        // tests/fixtures dir holds (P15: on CDO only).
        let (shape, ..) = single_app(&[("src/s.al", ONE_ANCHOR_TWO_NODES)]);
        let shape_ws = shape.path().to_path_buf();
        for ws in oracle_fixture_workspaces().into_iter().chain([shape_ws]) {
            let built = for_both_universes(&ws, |u, _, ctx, rows, _, _| {
                let (n, diffs) = j2_differences(ctx, rows);
                mapped += n;
                if let Some(d) = diffs.first() {
                    failures.push(format!(
                        "{} ({u}): {} nodes differ, first {d}",
                        ws.display(),
                        diffs.len()
                    ));
                }
            });
            if let Err(e) = built {
                failures.push(format!("{}: {e}", ws.display()));
            }
        }
        assert!(mapped > 0, "fixture precondition: J2 maps nodes");
        assert!(
            failures.is_empty(),
            "{} J2 differences:\n{}",
            failures.len(),
            failures.join("\n")
        );
    }

    /// T7 on CDO (skips without `CDO_WS`, panics under `ENFORCE_CDO_WS=1`).
    #[test]
    fn j2_row_equals_decl_join_on_cdo() {
        let Some(ws) = crate::cdo::cdo_ws_or_enforce() else {
            return;
        };
        for_both_universes(&ws, |u, _, ctx, rows, _, _| {
            let (n, diffs) = j2_differences(ctx, rows);
            eprintln!(
                "J2 oracle on CDO ({u}): {n} nodes mapped, {} differ",
                diffs.len()
            );
            assert!(n > 0, "CDO precondition ({u}): J2 maps nodes");
            assert_eq!(diffs, Vec::<String>::new(), "CDO ({u})");
        })
        .expect("CDO builds");
    }

    /// T8 (#160 §5.8): the event graph read through `j5_node` is the span-map
    /// graph, `Debug` text and census, on every fixture (both universes).
    #[test]
    fn event_graph_j5_node_equals_span_join() {
        use crate::program::model::events::{
            program_event_graph, program_event_graph_with_row_nodes,
        };
        let (mut with_edges, mut dep_subscriber_edges, mut failures) = (0, 0, Vec::new());
        // §5.8: no r0 or tests/fixtures dir has a source-bearing dependency
        // subscriber, so one is hand-stated: the workspace raises a dependency
        // event whose subscriber `H` lives in the dependency's source.
        let shape = tempfile::tempdir().unwrap();
        super::adapter_tests::write_workspace(
            shape.path(),
            &[(
                "src/w.al",
                "codeunit 50101 \"W\"\n{\n    procedure Go()\n    var\n        P: Codeunit \"Dep Pub\";\n    begin\n        P.RaiseA();\n    end;\n}\n",
            )],
            Some((
                r#"{"Codeunits":[{"Id":60030,"Name":"Dep Pub","Methods":[{"Name":"RaiseA","Parameters":[]},{"Name":"OnA","Parameters":[]}]}]}"#,
                &[(
                    "src/DepPub.Codeunit.al",
                    "codeunit 60030 \"Dep Pub\"\n{\n    procedure RaiseA()\n    begin\n        OnA();\n    end;\n\n    [IntegrationEvent(false, false)]\n    procedure OnA()\n    begin\n    end;\n\n    [EventSubscriber(ObjectType::Codeunit, Codeunit::\"Dep Pub\", 'OnA', '', false, false)]\n    local procedure H()\n    begin\n    end;\n}\n",
                )],
            )),
        );
        let shape_ws = shape.path().to_path_buf();
        for ws in oracle_fixture_workspaces().into_iter().chain([shape_ws]) {
            let built = for_both_universes(&ws, |u, _, ctx, rows, _, _| {
                let old = program_event_graph(ctx, &rows.workspace);
                let new = program_event_graph_with_row_nodes(ctx, &rows.workspace, &rows.row_nodes);
                with_edges += usize::from(!old.graph.edges.is_empty());
                let dep_rows: HashSet<&str> = rows
                    .workspace
                    .routines
                    .iter()
                    .filter(|r| r.source_anchor.source_unit_id.starts_with("dep:"))
                    .map(|r| r.id.as_str())
                    .collect();
                dep_subscriber_edges += old
                    .graph
                    .edges
                    .iter()
                    .filter(|e| dep_rows.contains(e.subscriber_routine_id.as_str()))
                    .count();
                if format!("{:?}", old.graph) != format!("{:?}", new.graph) {
                    failures.push(format!(
                        "{} ({u}): graph differs ({} / {} edges)",
                        ws.display(),
                        old.graph.edges.len(),
                        new.graph.edges.len()
                    ));
                }
                if old.census != new.census {
                    failures.push(format!(
                        "{} ({u}): census {:?} / {:?}",
                        ws.display(),
                        old.census,
                        new.census
                    ));
                }
            });
            if let Err(e) = built {
                failures.push(format!("{}: {e}", ws.display()));
            }
        }
        assert!(with_edges > 0, "fixture precondition: event edges exist");
        assert!(
            dep_subscriber_edges > 0,
            "fixture precondition: a source-bearing dependency subscriber has an edge"
        );
        assert!(
            failures.is_empty(),
            "{} J5 differences:\n{}",
            failures.len(),
            failures.join("\n")
        );
    }

    /// T8 on CDO (skips without `CDO_WS`, panics under `ENFORCE_CDO_WS=1`).
    #[test]
    fn event_graph_j5_node_equals_span_join_on_cdo() {
        use crate::program::model::events::{
            program_event_graph, program_event_graph_with_row_nodes,
        };
        let Some(ws) = crate::cdo::cdo_ws_or_enforce() else {
            return;
        };
        for_both_universes(&ws, |u, _, ctx, rows, _, _| {
            let old = program_event_graph(ctx, &rows.workspace);
            let new = program_event_graph_with_row_nodes(ctx, &rows.workspace, &rows.row_nodes);
            eprintln!(
                "J5 oracle on CDO ({u}): {} / {} edges, census {:?}",
                old.graph.edges.len(),
                new.graph.edges.len(),
                old.census
            );
            assert!(
                !old.graph.edges.is_empty(),
                "CDO precondition ({u}): event edges"
            );
            assert!(
                format!("{:?}", old.graph) == format!("{:?}", new.graph),
                "CDO ({u}): the event graph differs"
            );
            assert_eq!(old.census, new.census, "CDO ({u}): census");
        })
        .expect("CDO builds");
    }

    /// The rows named `name`, by position (`RoutineIx`).
    fn rows_named(rows: &crate::program::model::workspace::ModelRows, name: &str) -> Vec<usize> {
        rows.workspace
            .routines
            .iter()
            .enumerate()
            .filter(|(_, r)| r.name == name)
            .map(|(ri, _)| ri)
            .collect()
    }

    /// The distinct workspace program nodes named `name_lc`, graph order.
    fn nodes_named(ctx: &ProgramContext, name_lc: &str) -> Vec<RoutineNodeId> {
        let mut out: Vec<RoutineNodeId> = Vec::new();
        for n in &ctx.graph().routines {
            if n.id.object.app == ctx.primary_app_ref
                && &*n.id.name_lc == name_lc
                && !out.contains(&n.id)
            {
                out.push(n.id.clone());
            }
        }
        out
    }

    /// T7a (#160 §5.10; the `GetPurchaseHeaderPageID` shape): one procedure
    /// declared in `#if A` / `#else` arms at two lines, same signature. One node,
    /// two rows at two anchors; J2 = the second arm's row (the surface keeps the
    /// last declaration); the first arm's site is `caller_mismatch` (today's
    /// answer, R-RESIDUAL's shape).
    #[test]
    fn j2_row_two_anchors_one_node_is_the_last_arm() {
        let src = "codeunit 50100 \"S\"\n{\n#if A\n    procedure P()\n    begin\n        Message('a');\n    end;\n#else\n    procedure P()\n    begin\n        Message('b');\n    end;\n#endif\n\n    procedure Caller()\n    begin\n        P();\n    end;\n}\n";
        let (_dir, ctx, report, rows) = single_app(&[("src/s.al", src)]);
        let p = rows_named(&rows, "P");
        assert_eq!(p.len(), 2, "precondition: two rows");
        let nodes = nodes_named(&ctx, "p");
        assert_eq!(nodes.len(), 1, "precondition: one node");
        let line = |ri: usize| rows.workspace.routines[ri].source_anchor.start_line;
        assert_eq!(
            (line(p[0]), line(p[1])),
            (3, 8),
            "precondition: two anchors"
        );
        let c = site_census_rows(report.edge_view(), &ctx, &rows, None);
        let mismatched: Vec<(u32, &str)> = c
            .unmatched
            .iter()
            .map(|u| (u.start_line, u.reason))
            .collect();
        assert_eq!(
            mismatched,
            [(5, "caller_mismatch")],
            "the first arm's site is caller_mismatch"
        );
        assert_eq!(
            rows.row_nodes.j2_row.get(&nodes[0]).copied(),
            Some(u32::try_from(p[1]).unwrap()),
            "j2_row = the second arm's row"
        );
    }

    /// T7b's source: a split header whose `#if` arms differ in a parameter type.
    const ONE_ANCHOR_TWO_NODES: &str = "codeunit 50100 \"S\"\n{\n#if A\n    procedure P(X: Integer)\n#else\n    procedure P(X: Text)\n#endif\n    begin\n        Message('p');\n    end;\n\n    procedure Caller()\n    begin\n        P(1);\n        P('x');\n    end;\n}\n";

    /// T7b (#160 §5.10; the `AcquireTokenFromCache` shape): a split header whose
    /// arms differ in a parameter type. Two nodes, two rows at one anchor; J2 of
    /// BOTH nodes = the first row (the recorded cross-node alias).
    #[test]
    fn j2_row_one_anchor_two_nodes_is_the_first_row() {
        let (_dir, ctx, _report, rows) = single_app(&[("src/s.al", ONE_ANCHOR_TWO_NODES)]);
        let p = rows_named(&rows, "P");
        assert_eq!(p.len(), 2, "precondition: two rows");
        let nodes = nodes_named(&ctx, "p");
        assert_eq!(nodes.len(), 2, "precondition: two nodes");
        assert_ne!(
            nodes[0].sig_fp, nodes[1].sig_fp,
            "precondition: two signatures"
        );
        let anchor = |ri: usize| {
            let a = &rows.workspace.routines[ri].source_anchor;
            (a.start_line, a.start_column)
        };
        assert_eq!(anchor(p[0]), anchor(p[1]), "precondition: one anchor");
        let first = Some(u32::try_from(p[0]).unwrap());
        for n in &nodes {
            assert_eq!(
                rows.row_nodes.j2_row.get(n).copied(),
                first,
                "j2_row of {n:?} = the first row"
            );
        }
    }

    /// T7c (#160 §5.10; the `GetSalesHeaderPageID` twins): a split header whose
    /// arms differ only in an attribute. One node, two rows at one anchor; J2 =
    /// the first row.
    #[test]
    fn j2_row_one_anchor_one_node_is_the_first_row() {
        let src = "codeunit 50100 \"S\"\n{\n#if A\n    [Obsolete('x', '27.0')]\n    procedure P()\n#else\n    procedure P()\n#endif\n    begin\n        Message('p');\n    end;\n\n    procedure Caller()\n    begin\n        P();\n    end;\n}\n";
        let (_dir, ctx, _report, rows) = single_app(&[("src/s.al", src)]);
        let p = rows_named(&rows, "P");
        assert_eq!(p.len(), 2, "precondition: two rows");
        let nodes = nodes_named(&ctx, "p");
        assert_eq!(nodes.len(), 1, "precondition: one node");
        let anchor = |ri: usize| {
            let a = &rows.workspace.routines[ri].source_anchor;
            (a.start_line, a.start_column)
        };
        assert_eq!(anchor(p[0]), anchor(p[1]), "precondition: one anchor");
        assert_eq!(
            rows.row_nodes.j2_row.get(&nodes[0]).copied(),
            Some(u32::try_from(p[0]).unwrap()),
            "j2_row = the first row"
        );
    }

    /// T9 (#160 §5.9, P8): `ws-cross-object-chain`, cross-app, has one collision
    /// group: two symbol-only `OnRun` rows of codeunit 60105 sharing one id.
    #[test]
    fn collision_census_counts_groups() {
        let ws = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/r0-corpus/ws-cross-object-chain");
        let a = cross_app_assemble(
            &crate::program::inputs::InputSet::read(&ws, &Default::default()),
            crate::program::model::workspace::MODEL_INSTANCE_ID_DEFAULT,
            false,
        )
        .expect("cross-app build");
        let rows = &a.rows.workspace.routines;
        let mut by_id: HashMap<&str, Vec<&ModelRoutine>> = HashMap::new();
        for r in rows {
            by_id.entry(r.id.as_str()).or_default().push(r);
        }
        let groups: Vec<&Vec<&ModelRoutine>> = by_id.values().filter(|g| g.len() > 1).collect();
        assert_eq!(groups.len(), 1, "precondition: one shared id");
        assert!(
            groups[0]
                .iter()
                .all(|r| r.name == "OnRun" && r.source_anchor.source_unit_id.is_empty()),
            "precondition: two symbol-only OnRun rows: {:?}",
            groups[0]
                .iter()
                .map(|r| (&r.name, &r.source_anchor.source_unit_id))
                .collect::<Vec<_>>()
        );
        let c = crate::program::model::workspace::collision_census(
            &a.rows.workspace,
            &a.rows.row_nodes,
        );
        assert_eq!(
            (c.collision_groups, c.collision_rows, c.rows_symbol_only),
            (1, 2, 2),
            "one group of two symbol-only rows: {c:?}"
        );
        assert_eq!(
            (c.routine_rows, c.distinct_ids),
            (rows.len(), by_id.len()),
            "{c:?}"
        );
    }
}

// The adapter tests: a child module, for private access.
#[cfg(test)]
#[path = "program_calls_adapter_tests.rs"]
mod adapter_tests;

#[cfg(test)]
mod no_fallback_tests {
    use super::*;

    /// S3.2, the SOURCE-bearing dependency arm (`RouteTarget::Routine` into
    /// another app; the symbol-only arm is pinned by
    /// `interface_with_workspace_and_dependency_implementers`). Hand-stated: the
    /// dependency `.app` ships source for interface `IDep` and its only
    /// implementer `DepImpl`; the workspace calls `X.Go` on an `IDep`. The site is
    /// one to-less `Interface`+`ExternalTarget` edge naming `DepImpl` — it used
    /// to be `InterfaceNoImpl` (the route was dropped).
    #[test]
    fn interface_into_a_source_bearing_dependency_keeps_its_implementer() {
        use crate::engine::deps::app_package_zip::test_apps;
        let root = std::env::temp_dir().join(format!("s32-src-dep-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(root.join(".alpackages")).unwrap();
        let dep = "dddddddd-0003-0000-0000-0000000003b2";
        std::fs::write(
            root.join("app.json"),
            format!(
                r#"{{"id":"11111111-0000-0000-0000-0000000003b2","name":"Host","publisher":"P","version":"1.0.0.0","dependencies":[{{"id":"{dep}","name":"Dep","publisher":"Microsoft","version":"1.0.0.0"}}]}}"#
            ),
        )
        .unwrap();
        std::fs::write(
            root.join("src/W.Codeunit.al"),
            "codeunit 50101 \"W\"\n{\n    procedure Caller()\n    var\n        X: Interface IDep;\n    begin\n        X.Go();\n    end;\n}\n",
        )
        .unwrap();
        let manifest = test_apps::manifest_xml(dep, "Dep");
        let symbols = br#"{"Interfaces":[{"Name":"IDep","Methods":[{"Name":"Go","Parameters":[]}]}],"Codeunits":[{"Id":60001,"Name":"DepImpl","ImplementedInterfaces":["IDep"],"Methods":[{"Name":"Go","Parameters":[]}]}]}"#;
        std::fs::write(
            root.join(".alpackages/dep.app"),
            test_apps::build_app(&[
                ("NavxManifest.xml", manifest.as_bytes()),
                ("SymbolReference.json", symbols),
                (
                    "src/IDep.Interface.al",
                    b"interface IDep\n{\n    procedure Go();\n}\n",
                ),
                (
                    "src/DepImpl.Codeunit.al",
                    b"codeunit 60001 DepImpl implements IDep\n{\n    procedure Go()\n    begin\n    end;\n}\n",
                ),
            ]),
        )
        .unwrap();

        let (ctx, report, l3) = build_models(&root).unwrap();
        let (calls, census) =
            resolved_calls_from_program(report.edge_view(), &ctx, &l3.workspace, true);
        std::fs::remove_dir_all(&root).ok();
        let caller = l3
            .workspace
            .routines
            .iter()
            .find(|r| r.name == "Caller")
            .unwrap();
        let cs = &caller.call_sites[0];
        let got: Vec<_> = calls
            .edges
            .iter()
            .filter(|e| e.callsite_id == cs.id)
            .collect();
        assert_eq!(got.len(), 1, "{got:?}");
        assert_eq!(got[0].dispatch_kind, DispatchKind::Interface);
        assert_eq!(got[0].to, None);
        assert_eq!(got[0].resolution, Resolution::ExternalTarget);
        assert_eq!(
            got[0].external_type_ref,
            Some(ExternalTypeRef {
                kind: "Codeunit".to_string(),
                name: "DepImpl".to_string()
            })
        );
        assert_eq!(census.adapter_interface_dependency_impls, 1);
        // S3.3: the dependency routine is named, and its body is NOT analysed
        // (so no detector may read its empty facts as "no effects").
        assert_eq!(
            calls.external_targets.len(),
            1,
            "{:?}",
            calls.external_targets
        );
        let t = &calls.external_targets[0];
        assert_eq!(t.callsite_id, cs.id);
        assert!(t.target.ends_with("/Codeunit/60001::go/0"), "{}", t.target);
        assert_eq!(
            t.body,
            Some(crate::program::registry::BodyState::NotAnalyzed)
        );
    }

    /// S3.1: a call site the program engine gives no edge for is an honest
    /// `Unknown(NoProgramSite)` — never the legacy resolver's answer. Hand-stated:
    /// one model call site is given a key no program edge has (#160 §5.3:
    /// `u32::MAX - e`; it used to move the site's span). That site would
    /// otherwise resolve (it is an ordinary workspace call).
    #[test]
    #[ignore = "#160 Task 2: needs the key join in production (spec §4.12)"]
    fn a_site_without_a_program_edge_is_unknown_not_legacy_resolved() {
        let ws = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/r0-corpus/ws-cross-object-chain");
        let (ctx, report, mut l3) = build_models(&ws).unwrap();
        let (ri, ci) = l3
            .workspace
            .routines
            .iter()
            .enumerate()
            .find_map(|(ri, r)| (!r.call_sites.is_empty()).then_some((ri, 0)))
            .unwrap();
        let cs_id = l3.workspace.routines[ri].call_sites[ci].id.clone();
        let before = resolved_calls_from_rows(report.edge_view(), &ctx, &l3, true, None).0;
        let resolved_before: Vec<_> = before
            .edges
            .iter()
            .filter(|e| e.callsite_id == cs_id)
            .collect();
        assert!(
            resolved_before.iter().any(|e| e.to.is_some()),
            "precondition: the site resolves while its program edge is found: {resolved_before:?}"
        );

        // STEP-6: the walk stamps no key yet, so there may be none to change.
        if let Some(e) = l3.workspace.routines[ri].site_exprs.calls.get_mut(ci) {
            unkey(std::slice::from_mut(e));
        }
        let (calls, census) = resolved_calls_from_rows(report.edge_view(), &ctx, &l3, true, None);
        let site: Vec<_> = calls
            .edges
            .iter()
            .filter(|e| e.callsite_id == cs_id)
            .collect();
        assert_eq!(site.len(), 1, "{site:?}");
        assert_eq!(site[0].to, None);
        assert_eq!(
            site[0].resolution,
            Resolution::Unknown(ModelReason::NoProgramSite)
        );
        assert_eq!(census.adapter_l3_fallback_sites, 1);
    }
}

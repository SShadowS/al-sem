//! B3 Phase A: the production call resolution of `alsem analyze`'s detectors.
//! [`attach_program_calls`] converts the program engine's call-site edges into
//! L3's `ResolvedCalls` and sets `L3Resolved.precomputed_calls`; it started
//! (Task 1) as a read-only join census, which `aldump --b3` still prints.
//!
//! The conversion needs every L3 call site paired with exactly one program
//! edge. This module does the pairing and counts what does not pair, by
//! reason. It lives under `engine::l3` because
//! `program::resolve` must not import `engine::l3`.
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
//!   `implicit_trigger_matched` when an `L3RecordOperation` sits at the same
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
//! - **Implicit triggers**, keyed by `L3RecordOperation.id`: one edge per
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
//! **Order** is `resolve_calls`'s: call sites in routine order, then the
//! trigger edges in routine/op order.

use std::collections::{HashMap, HashSet};

use al_syntax::IdentifierFoldExt;
use serde::Serialize;

use crate::engine::l2::features::{PCallSite, PCallee};
use crate::engine::l3::call_resolver::{
    BindingState, CallEdge, DispatchMeta, ExternalTypeRef, ResolvedCalls,
    UnknownReason as L3Reason, UpgradedBinding, initial_binding_state, mark_bindings_ambiguous,
    object_run_dispatch_kind, upgrade_bindings, upgrade_bindings_with,
};
use crate::engine::l3::l3_workspace::{L3RecordOperation, L3Routine, L3Workspace};
use crate::engine::l3::taxonomy::{DispatchKind, Resolution};
use crate::program::abi_ingest::object_kind_from_abi_type;
use crate::program::graph::ProgramGraph;
use crate::program::node::{AppRef, ObjKey, ObjectNodeId, RoutineNodeId};
use crate::program::node_extract::{AbiParams, ObjectNode};
use crate::program::resolve::decl_surface::DeclSurface;
use crate::program::resolve::edge::{
    DispatchShape, Edge, EdgeKind, Evidence, Route, RouteTarget, UnknownReason as PReason,
    callee_fp,
};
use crate::program::resolve::full::{ClassifiedEdge, ObligationId, ProgramContext, ProgramReport};
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
    /// L3 call sites (`L3Routine::call_sites`) over the workspace.
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
    /// L3 record operations (`L3Routine::record_operations`).
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
    /// Member runs (`PageVar.Run()`) on a WORKSPACE object with no entry
    /// trigger: `Opaque` with no external type, like `Page.Run(Page::X)`.
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

fn can_fire_trigger(op: &str) -> bool {
    matches!(op, "insert" | "modify" | "delete" | "validate" | "rename")
}

/// The model's source-unit spelling of program file `path` in `app` (engine-switch
/// S7.3): a primary-app file is its virtual path (the `ws:` prefix dropped, as the
/// span keys drop it); a dependency file is `dep:<guid>:<path>`, as the cross-app
/// model spells it.
fn model_unit(graph: &ProgramGraph, primary: AppRef, app: AppRef, path: &str) -> String {
    if app == primary {
        path.to_string()
    } else {
        format!("dep:{}:{path}", graph.apps.resolve(app).guid)
    }
}

/// The apps whose routines the model holds: the primary app, and in a cross-app
/// model (S7.2) every dependency.
fn model_apps(graph: &ProgramGraph, primary: AppRef, ws: &L3Workspace) -> HashSet<AppRef> {
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

/// Pair every L3 call site with the program edge at the same exact span and
/// count what does not pair, by reason. `report` must come from
/// `resolve_full_program_with(ctx)`; `ws` is the L3 model of the same
/// workspace.
#[must_use]
pub fn site_census(report: &ProgramReport, ctx: &ProgramContext, ws: &L3Workspace) -> SiteCensus {
    join(report, ctx, ws).census
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

fn join<'a>(report: &'a ProgramReport, ctx: &ProgramContext, ws: &L3Workspace) -> Join<'a> {
    let mut c = SiteCensus::default();
    let mut matched_calls: HashMap<(usize, usize), &'a ClassifiedEdge> = HashMap::new();
    let mut matched_ops: HashMap<(usize, usize), &'a ClassifiedEdge> = HashMap::new();
    let surface = ctx.decl_surface();
    let graph = ctx.graph();
    let primary = report.primary_app_ref;
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
    let links = crate::program::model::site_links::SiteLinks::build(report);
    let mut program: HashMap<SiteKey, &'a ClassifiedEdge> = HashMap::new();
    for &app in &apps {
        for (k, edges) in links.sites_of(app) {
            c.program_sites += edges.len();
            c.duplicate_program_span += edges.len() - 1;
            let unit = model_unit(graph, primary, app, &k.0);
            program.insert((unit, k.1, k.2, k.3, k.4), edges[0]);
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
/// census with its `adapter_*` counts. `report` must come from
/// `resolve_full_program_with(ctx)`; `ws` is the L3 model of the same
/// workspace. Nothing here mutates `ws`.
///
/// `upgrade_dependency_bindings` (stage 2): upgrade the bindings of a call
/// into a dependency routine with that routine's parameter `var`-ness, as
/// for a workspace callee. Production passes `true`; the B3 harness passes
/// `false` to diff this effect alone (`aldump --b3 --b3-deps`).
#[must_use]
pub fn resolved_calls_from_program(
    report: &ProgramReport,
    ctx: &ProgramContext,
    ws: &L3Workspace,
    upgrade_dependency_bindings: bool,
) -> (ResolvedCalls, SiteCensus) {
    let (calls, census, _) = adapter(report, ctx, ws, upgrade_dependency_bindings, false, None);
    (calls, census)
}

/// The production step that points the detectors at the program engine's
/// calls: run the adapter (no per-site notes, dependency bindings upgraded),
/// build the event graph from the program engine's subscriptions (S4.2), drop
/// the program model, and set `resolved.precomputed_calls` and
/// `resolved.precomputed_events`. `alsem
/// analyze` and [`assemble_and_resolve_workspace_with_program_calls`] both
/// use it, so a test cannot drift from the production path.
pub fn attach_program_calls(
    resolved: &mut crate::engine::l3::l3_workspace::L3Resolved,
    ctx: ProgramContext,
    report: ProgramReport,
) {
    attach_program_calls_with(resolved, ctx, report, None);
}

/// [`attach_program_calls`] for a cross-app model (engine-switch S7.3): `abi_rows`
/// joins a symbol-only dependency routine to its model row.
fn attach_program_calls_with(
    resolved: &mut crate::engine::l3::l3_workspace::L3Resolved,
    ctx: ProgramContext,
    report: ProgramReport,
    abi_rows: Option<&crate::program::model::workspace::AbiRowIds>,
) {
    use crate::engine::perf_trace as pt;
    let calls = {
        let _s = pt::span("b3", "b3.adapter");
        adapter(&report, &ctx, &resolved.workspace, true, false, abi_rows).0
    };
    // Engine-switch S4.2: the detector event graph, from the same program build.
    let events = {
        let _s = pt::span("b3", "b3.events");
        crate::program::model::events::program_event_graph(&ctx, &resolved.workspace)
    };
    resolved.precomputed_events = Some(std::sync::Arc::new(events));
    {
        let _s = pt::span("b3", "b3.report_drop");
        drop(report);
    }
    {
        let _s = pt::span("b3", "b3.ctx_drop");
        drop(ctx);
    }
    resolved.precomputed_calls = Some(std::sync::Arc::new(calls));
    // A `local` event raised only with temporary records makes its subscribers'
    // same-named `var` parameters temporary (engine-switch S8).
    crate::engine::l3::event_param_temp::prove_event_param_temps(resolved);
}

/// `assemble_and_resolve_workspace_default` plus the production call
/// resolution ([`attach_program_calls`]): the L3 model the detectors see in
/// `alsem analyze`. `None` if either build fails. For tests that pin detector
/// output (r4/r4f goldens).
#[must_use]
pub fn assemble_and_resolve_workspace_with_program_calls(
    workspace: &std::path::Path,
) -> Option<crate::engine::l3::l3_workspace::L3Resolved> {
    let (ctx, report, _) =
        crate::program::resolve::full::build_program_with_coverage(workspace).ok()?;
    let mut resolved =
        crate::engine::l3::l3_workspace::assemble_and_resolve_workspace_default(workspace)?;
    attach_program_calls(&mut resolved, ctx, report);
    Some(resolved)
}

/// The program-backed model every consumer moves onto in engine-switch S6: the
/// model projected from one program build (as `alsem analyze` builds it,
/// `gate::run::build_analysis_model`), with the program engine's calls and event
/// graph attached. Same signature as
/// `l3_workspace::assemble_and_resolve_workspace`, which it replaces consumer by
/// consumer. `None` when the program build or the model assembly fails.
#[must_use]
pub fn assemble_and_resolve_workspace_program(
    workspace: &std::path::Path,
    model_instance_id: &str,
    skip_roots_config: bool,
) -> Option<crate::engine::l3::l3_workspace::L3Resolved> {
    let (ctx, report, _) =
        crate::program::resolve::full::build_program_with_coverage(workspace).ok()?;
    let mut resolved =
        crate::engine::l3::l3_workspace::assemble_and_resolve_workspace_from_program(
            workspace,
            model_instance_id,
            skip_roots_config,
            &ctx,
            &report.parenless_calls,
        )?;
    attach_program_calls(&mut resolved, ctx, report);
    Some(resolved)
}

/// The CROSS-APP detector model and what the cross-app base reads besides it.
pub struct CrossAppProgram {
    /// The model, with calls and events attached for every body in it.
    pub resolved: crate::engine::l3::l3_workspace::L3Resolved,
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
}

/// The CROSS-APP detector model (engine-switch S7.3): the workspace and every
/// dependency in the snapshot (`assemble_and_resolve_cross_app_from_program`), from
/// one `FULL` program build, with the program engine's calls attached for EVERY
/// body in it — the workspace's, and each dependency's resolved from its own app
/// (`ProgramContext::resolve_dependency_bodies`) — and its event graph over all of
/// them. `None` when the program build or the model assembly fails.
#[must_use]
pub fn assemble_and_resolve_cross_app_program(
    workspace: &std::path::Path,
    model_instance_id: &str,
    skip_roots_config: bool,
) -> Option<CrossAppProgram> {
    use crate::engine::perf_trace as pt;
    let (ctx, mut report, coverage) = {
        let _s = pt::span("crossapp", "crossapp.program_build_full");
        crate::program::resolve::full::build_program_with_coverage_profiled(
            workspace,
            crate::program::profile::BuildProfile::FULL,
        )
        .ok()?
    };
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
    let dependency = {
        let _s = pt::span("crossapp", "crossapp.resolve_dependency_bodies");
        ctx.resolve_dependency_bodies()
    };
    report.edges.extend(dependency.edges);
    report.site_facts.extend(dependency.site_facts);
    report.parenless_calls.extend(dependency.parenless_calls);
    // S8.2: only what the workspace can reach, and the code around the dependency
    // events it subscribes to (`program::resolve::demand`).
    let demand = {
        let _s = pt::span("crossapp", "crossapp.demand");
        crate::program::resolve::demand::cross_app_demand(
            report.primary_app_ref,
            ctx.graph(),
            &report.edges,
        )
    };
    let (mut resolved, abi_rows) = {
        let _s = pt::span("crossapp", "crossapp.model_assembly");
        crate::program::model::workspace::assemble_and_resolve_cross_app_from_program(
            workspace,
            model_instance_id,
            skip_roots_config,
            &ctx,
            Some(&demand),
            &report.parenless_calls,
        )?
    };
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
    {
        let _s = pt::span("crossapp", "crossapp.attach_program_calls");
        attach_program_calls_with(&mut resolved, ctx, report, Some(&abi_rows));
    }
    Some(CrossAppProgram {
        resolved,
        declared_dependencies,
        dependency_apps,
        coverage,
        friends,
    })
}

/// The edge for a call site the program engine gave no usable edge for: one
/// to-less `Unknown(NoProgramSite)` edge, bindings in their initial state (a
/// record argument stays `"unresolved-callee"`). Engine-switch S3.1 — this used to
/// be the legacy resolver's answer.
fn no_program_edge(r: &L3Routine, cs: &PCallSite) -> (Vec<CallEdge>, Vec<UpgradedBinding>) {
    let mut e = CallEdge::base(&r.id, &cs.id, &cs.operation_id);
    e.resolution = Resolution::Unknown(L3Reason::NoProgramSite);
    (vec![e], initial_binding_state(cs).bindings)
}

/// How the adapter produced one site's edges, for the B3 detector-diff
/// harness's attribution (`b3_diff`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SiteNote {
    /// The census categories this site moved, e.g.
    /// `external-record-receiver`, `trigger-beyond-l3`,
    /// `l3-fallback:no_program_site`. `program-site` / `program-trigger`
    /// when the edges came from the program engine and no special counter
    /// moved.
    pub categories: Vec<String>,
    /// The program edge: shape and each route's evidence (with its
    /// `UnknownReason`). `None` when the edges came from L3's own resolver.
    pub program: Option<String>,
}

/// [`SiteNote`]s keyed like a `CallEdge`: `(from routine id, call-site or
/// record-op id)`.
pub type SiteNotes = HashMap<(String, String), SiteNote>;

/// The census counters a site can move, with the category name it gets.
type Counter = fn(&SiteCensus) -> usize;

const CATEGORY_COUNTERS: &[(&str, Counter)] = &[
    ("external-record-receiver", |c| {
        c.adapter_external_record_receiver
    }),
    ("external-object-receiver", |c| {
        c.adapter_external_object_receiver
    }),
    ("external-other", |c| c.adapter_external_other),
    ("external-member-decline", |c| {
        c.adapter_external_member_decline
    }),
    ("workspace-run-no-entry", |c| {
        c.adapter_workspace_run_no_entry
    }),
    ("dep-bindings-source", |c| c.adapter_dep_bindings_source),
    ("dep-bindings-symbol", |c| c.adapter_dep_bindings_symbol),
    // Not `_missing` / `_collapsed` / `_no_routine`: those sites keep their
    // bindings, so they are no cause of a difference.
    ("routes-dropped", |c| c.adapter_routes_dropped),
    ("trigger-dependency-routes", |c| {
        c.adapter_trigger_dependency_routes
    }),
    ("ambiguous-dependency-candidates", |c| {
        c.adapter_ambiguous_dependency_candidates
    }),
    ("trigger-routes-filtered", |c| {
        c.adapter_trigger_routes_filtered
    }),
    ("multi-route", |c| c.adapter_multi_route_sites),
    ("empty-route", |c| c.adapter_empty_route_sites),
    ("l3-fallback:callee-outside-l3", |c| {
        c.adapter_callee_outside_l3
    }),
];

fn counter_values(c: &SiteCensus) -> Vec<usize> {
    CATEGORY_COUNTERS.iter().map(|(_, f)| f(c)).collect()
}

/// The categories whose counters moved since `before`, or `default`.
fn moved_categories(before: &[usize], c: &SiteCensus, default: &str) -> Vec<String> {
    let moved: Vec<String> = CATEGORY_COUNTERS
        .iter()
        .zip(before)
        .filter(|((_, f), b)| f(c) != **b)
        .map(|((name, _), _)| (*name).to_string())
        .collect();
    if moved.is_empty() {
        vec![default.to_string()]
    } else {
        moved
    }
}

fn describe_program_edge(edge: &Edge) -> String {
    let routes: Vec<String> = edge
        .routes
        .iter()
        .map(|r| format!("{:?}", r.evidence))
        .collect();
    format!("{:?} {:?} [{}]", edge.kind, edge.shape, routes.join(", "))
}

/// [`resolved_calls_from_program`], plus one [`SiteNote`] per call site and
/// per record op that has edges on either path.
#[must_use]
pub fn resolved_calls_with_notes(
    report: &ProgramReport,
    ctx: &ProgramContext,
    ws: &L3Workspace,
    upgrade_dependency_bindings: bool,
) -> (ResolvedCalls, SiteCensus, SiteNotes) {
    adapter(report, ctx, ws, upgrade_dependency_bindings, true, None)
}

/// The adapter. `want_notes` is false on the production path
/// ([`resolved_calls_from_program`]): it then builds no [`SiteNote`] and
/// returns an empty map. The calls and census do not depend on it.
fn adapter(
    report: &ProgramReport,
    ctx: &ProgramContext,
    ws: &L3Workspace,
    upgrade_dependency_bindings: bool,
    want_notes: bool,
    abi_rows: Option<&crate::program::model::workspace::AbiRowIds>,
) -> (ResolvedCalls, SiteCensus, SiteNotes) {
    let mut notes = SiteNotes::new();
    let j = join(report, ctx, ws);
    // The join's reason per unmatched L3 call site, for fallback notes.
    let unmatched_reason: HashMap<SiteKey, &'static str> = if want_notes {
        j.census
            .unmatched
            .iter()
            .filter(|u| {
                matches!(
                    u.reason,
                    "no_program_site" | "shape_mismatch" | "callee_fp_mismatch" | "caller_mismatch"
                )
            })
            .map(|u| {
                (
                    (
                        u.unit.clone(),
                        u.start_line,
                        u.start_col,
                        u.end_line,
                        u.end_col,
                    ),
                    u.reason,
                )
            })
            .collect()
    } else {
        HashMap::new()
    };
    let mut c = j.census;
    let surface = ctx.decl_surface();
    let graph = ctx.graph();
    let mut by_decl: HashMap<(&str, u32, u32), &L3Routine> = HashMap::new();
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
    let objects = graph
        .objects
        .iter()
        .map(|o| (object_key(&o.id), o))
        .collect();
    let primary_objects = graph
        .objects
        .iter()
        .filter(|o| o.id.app == report.primary_app_ref)
        .map(|o| (o.id.kind, o.name.fold_identifier()))
        .collect();
    let empty_abi_rows = crate::program::model::workspace::AbiRowIds::new();
    let conv = Converter {
        primary: report.primary_app_ref,
        model_apps: model_apps(graph, report.primary_app_ref, ws),
        abi_rows: abi_rows.unwrap_or(&empty_abi_rows),
        routine_by_id: ws.routines.iter().map(|r| (r.id.as_str(), r)).collect(),
        upgrade_dependency_bindings,
        graph,
        surface: &surface,
        by_decl,
        objects,
        primary_objects,
        site_facts: &report.site_facts,
        registry: ctx.registry(),
        targets: std::cell::RefCell::new(Vec::new()),
    };

    let mut edges: Vec<CallEdge> = Vec::new();
    let mut upgraded_bindings: HashMap<String, Vec<UpgradedBinding>> = HashMap::new();
    // The adapter emits no resolver diagnostics (the legacy resolver's
    // double-upgrade warning had no program-engine analogue to fire).
    let diagnostics = Vec::new();
    // Same order as `resolve_calls`: call sites in routine order, then the
    // implicit-trigger edges in routine/op order.
    for (ri, r) in ws.routines.iter().enumerate() {
        for (ci, cs) in r.call_sites.iter().enumerate() {
            let before = if want_notes {
                counter_values(&c)
            } else {
                Vec::new()
            };
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
            if want_notes {
                let fallback = match unmatched_reason.get(&l3_key(&cs.source_anchor)) {
                    Some(reason) => format!("l3-fallback:{reason}"),
                    None => "program-site".to_string(),
                };
                notes.insert(
                    (r.id.clone(), cs.id.clone()),
                    SiteNote {
                        categories: moved_categories(&before, &c, &fallback),
                        program: program_edge.map(|ce| describe_program_edge(&ce.edge)),
                    },
                );
            }
            edges.extend(site_edges);
            upgraded_bindings.insert(cs.id.clone(), bindings);
        }
    }
    for (ri, r) in ws.routines.iter().enumerate() {
        for (oi, op) in r.record_operations.iter().enumerate() {
            let before = if want_notes {
                counter_values(&c)
            } else {
                Vec::new()
            };
            let key = || (r.id.clone(), op.id.clone());
            if let Some(ce) = j.ops.get(&(ri, oi)) {
                c.adapter_program_trigger_ops += 1;
                edges.extend(conv.triggers(r, op, ce, &mut c));
                if want_notes {
                    notes.insert(
                        key(),
                        SiteNote {
                            categories: moved_categories(&before, &c, "program-trigger"),
                            program: Some(describe_program_edge(&ce.edge)),
                        },
                    );
                }
            } else if matches!(
                op.op.as_str(),
                "Insert" | "Modify" | "Delete" | "Validate" | "Rename"
            ) {
                // S3.5: no legacy fallback.
                c.adapter_l3_trigger_ops += 1;
                if want_notes {
                    notes.insert(
                        key(),
                        SiteNote {
                            categories: vec!["no-program-trigger".to_string()],
                            program: None,
                        },
                    );
                }
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
        notes,
    )
}

/// Where a route's target lives relative to the model (engine-switch S7.3).
enum Target<'a> {
    /// A routine the model holds: an edge to it.
    Model(&'a L3Routine),
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
    routine_by_id: HashMap<&'a str, &'a L3Routine>,
    upgrade_dependency_bindings: bool,
    graph: &'a ProgramGraph,
    surface: &'a DeclSurface,
    /// L3 routines by declaration anchor `(unit, line, column)`.
    by_decl: HashMap<(&'a str, u32, u32), &'a L3Routine>,
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
    fn l3_routine(&self, id: &RoutineNodeId) -> Option<&'a L3Routine> {
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
            name: o.name.clone(),
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
        r: &L3Routine,
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
        // S3.6: no route is cut. A call or run edge with several routes outside
        // an interface (the resolver makes none today: `Multicast` is for
        // triggers and events, `Exact` has one route) converts as the closed
        // candidate set it is, never as its first route.
        let shape = match edge.shape {
            DispatchShape::Exact | DispatchShape::Multicast if edge.routes.len() > 1 => {
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
                    L3Reason::DynamicObjectRunTarget
                } else {
                    L3Reason::DynamicReceiver
                });
            }
            DispatchShape::Exact | DispatchShape::Multicast => {
                match edge.routes.first() {
                    None => {
                        c.adapter_empty_route_sites += 1;
                        e.resolution = Resolution::Unknown(L3Reason::CalleeUnknown);
                    }
                    Some(route) => match (self.target(route), &route.target) {
                        (Target::MissingFromModel, _) => return None,
                        (Target::Model(callee), _) => {
                            let id = Self::route_routine_id(route)?;
                            let _ = upgrade_bindings(&mut state, callee, &cs.id);
                            e.to = Some(callee.id.clone());
                            e.resolution = Resolution::Resolved;
                            if let PCallee::Member { method, .. } = &cs.callee {
                                if method.fold_identifier() == "run"
                                    && id.object.kind == al_syntax::ir::ObjectKind::Codeunit
                                    && id.name_lc == "onrun"
                                {
                                    // `CuVar.Run()` lands on OnRun: L3's
                                    // codeunit-run shape.
                                    e.dispatch_kind = DispatchKind::CodeunitRun;
                                } else if e.dispatch_kind == DispatchKind::Method {
                                    e.receiver_type =
                                        self.receiver(ce).and_then(|f| f.type_text.clone());
                                }
                            }
                        }
                        // A run into a workspace object with no entry trigger
                        // (`Page.Run(Page::X)` or `PageVar.Run()`): the run
                        // shape, with no external type — the object is ours.
                        (Target::Outside, RouteTarget::AbiSymbol { key })
                            if key.app == self.primary =>
                        {
                            // Our own app is source, so the only ABI-symbol route
                            // into it is a run's missing entry trigger (the
                            // resolver's `opaque_boundary_route`). The edge kind is
                            // `Run` for `Page.Run(..)` but `Call` for `PageVar.Run()`,
                            // so the trigger name is the invariant, not `is_run`.
                            debug_assert!(
                                matches!(
                                    key.routine_name_lc.as_str(),
                                    "onrun" | "onopenpage" | "onprereport"
                                ),
                                "workspace ABI-symbol route that is not a missing entry trigger: {key:?}"
                            );
                            c.adapter_workspace_run_no_entry += 1;
                            e.dispatch_kind = object_run_dispatch_kind(
                                match key.object_type.to_ascii_lowercase().as_str() {
                                    "page" => "Page",
                                    "report" => "Report",
                                    _ => "Codeunit",
                                },
                            );
                            e.resolution = Resolution::Opaque;
                        }
                        (
                            Target::Outside,
                            RouteTarget::Routine(_) | RouteTarget::AbiSymbol { .. },
                        ) => {
                            e.resolution = external;
                            e.external_type_ref = self.dependency_ref(route).flatten();
                            self.record_target(&cs.id, route);
                            if self.upgrade_dependency_bindings {
                                self.upgrade_dependency(&mut state, route, &cs.id, c);
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
                            // A run's entry trigger is "ambiguous" only when
                            // it is ABI-collapse-marked: a dependency object.
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
                    },
                }
            }
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
                        ObjKey::Name(key.object_name_lc.clone())
                    },
                },
                name_lc: key.routine_name_lc.clone(),
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
                    ObjKey::Name(key.object_name_lc.clone())
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
                        ObjKey::Name(key.object_name_lc.clone())
                    },
                },
                name_lc: key.routine_name_lc.clone(),
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
        r: &L3Routine,
        cs: &PCallSite,
        ce: &ClassifiedEdge,
        state: &mut BindingState,
        c: &mut SiteCensus,
    ) -> Option<Vec<CallEdge>> {
        mark_bindings_ambiguous(state);
        let mut callees: Vec<&L3Routine> = Vec::new();
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
                            ObjKey::Name(key.object_name_lc.clone())
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
            .map_or_else(|| name_lc.clone(), |o| o.name.clone());
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
            e.resolution = Resolution::Unknown(L3Reason::InterfaceNoImpl);
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
        r: &L3Routine,
        op: &L3RecordOperation,
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
                && (!is_validate || (field_lc.is_some() && id.enclosing_member_lc == field_lc));
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
        tos.into_iter()
            .map(|to| {
                let mut e = CallEdge::base(&r.id, &op.id, &op.id);
                e.to = Some(to);
                e.dispatch_kind = DispatchKind::ImplicitTrigger;
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
            | P::AccessFilteredOverload
    )
}

/// A program `UnknownReason` (other than the dependency-callee ones) → the
/// nearest L3 resolution. Ambiguity and absent/invisible members map to
/// L3's `Ambiguous` and `MemberNotFound` (they decide the uncertainty kind);
/// every other reason is `Unknown(_)`, whose L3 reason is diagnostic only.
fn map_unknown(reason: PReason) -> Resolution {
    use PReason as P;
    match reason {
        P::OverloadAmbiguous | P::AccessFilteredOverload => Resolution::Ambiguous,
        P::ArityMismatch
        | P::MemberNotFound
        | P::ProtectedNotVisible
        | P::LocalNotVisible
        | P::InternalNotVisible => Resolution::MemberNotFound,
        P::CompoundReceiver => Resolution::Unknown(L3Reason::CompoundReceiver),
        P::UntrackedReceiver | P::ReceiverOutOfClosure => {
            Resolution::Unknown(L3Reason::UntrackedReceiver)
        }
        P::CatalogMiss => Resolution::Unknown(L3Reason::FrameworkMethodNotInCatalog),
        P::BuiltinPrecedenceCollision
        | P::WithScopeGuard
        | P::CodeunitTableNoExcluded
        | P::ReportRecExcluded => Resolution::Unknown(L3Reason::BareUnresolved),
        P::UnclassifiedCallee
        | P::IndexIntegrationGap
        | P::ObjectNotInGraph
        | P::AbiCollapsedOverload => Resolution::Unknown(L3Reason::CalleeUnknown),
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
    Ok(resolved_calls_from_program(&report, &ctx, &l3.workspace, true).1)
}

pub(crate) fn build_models(
    workspace: &std::path::Path,
) -> Result<
    (
        ProgramContext,
        ProgramReport,
        crate::engine::l3::l3_workspace::L3Resolved,
    ),
    String,
> {
    use crate::program::resolve::full::{build_snapshot_res, fresh_program_from_snapshot};
    let (ctx, report) = fresh_program_from_snapshot(build_snapshot_res(workspace)?)?;
    // The production model (`assemble_and_resolve_workspace_program`), built from
    // the same program parse and resolution.
    let l3 = crate::program::model::workspace::assemble_and_resolve_workspace_from_program(
        workspace,
        crate::engine::l3::l3_workspace::MODEL_INSTANCE_ID_DEFAULT,
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

#[cfg(test)]
mod tests {
    use super::*;

    const APP_JSON: &str = r#"{"id":"b3b3b3b3-0000-0000-0000-000000000001","name":"B3 Census","publisher":"T","version":"1.0.0.0"}"#;

    /// Write `files` (relative path, bytes) under a fresh workspace with a root
    /// `app.json`, then run the census.
    fn census(files: &[(&str, &[u8])]) -> SiteCensus {
        census_with(files, |_| {})
    }

    /// [`census`], with `mutate` applied to the L3 model before the census:
    /// the way a test states a mismatch precondition by assignment.
    fn census_with(files: &[(&str, &[u8])], mutate: impl FnOnce(&mut L3Workspace)) -> SiteCensus {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("app.json"), APP_JSON).unwrap();
        for (rel, bytes) in files {
            let p = dir.path().join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, bytes).unwrap();
        }
        let (ctx, report, mut l3) = build_models(dir.path()).unwrap();
        mutate(&mut l3.workspace);
        site_census(&report, &ctx, &l3.workspace)
    }

    const TWO_CALLS: &str = "codeunit 50100 \"M\"\n{\n    procedure Foo()\n    begin\n    end;\n\n    procedure Bar()\n    begin\n    end;\n\n    procedure Caller()\n    begin\n        Foo();\n        Bar();\n    end;\n}\n";

    fn routine<'a>(
        ws: &'a mut L3Workspace,
        name: &str,
    ) -> &'a mut super::super::l3_workspace::L3Routine {
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

    /// Precondition by assignment: the L3 `Insert` and `SetRange` record ops
    /// and the `Commit` are moved off their spans, so no program edge sits
    /// where L3 has them. Insert is trigger-capable; SetRange and Commit are
    /// not. The program's record ops then find no L3 record op.
    #[test]
    fn l3_ops_with_no_program_site_are_counted_by_trigger_capability() {
        let cu = "codeunit 50101 \"C\"\n{\n    procedure P()\n    var\n        R: Record \"T\";\n    begin\n        R.SetRange(Code, 'X');\n        R.Insert(true);\n        Commit();\n    end;\n}\n";
        let table = "table 50100 \"T\"\n{\n    fields\n    {\n        field(1; Code; Code[20]) { }\n    }\n    trigger OnInsert()\n    begin\n    end;\n}\n";
        let c = census_with(
            &[("src/t.al", table.as_bytes()), ("src/c.al", cu.as_bytes())],
            |ws| {
                let r = routine(ws, "P");
                assert_eq!(r.record_operations.len(), 2);
                for op in &mut r.record_operations {
                    op.source_anchor.start_column += 100;
                }
                for op in &mut r.operation_sites {
                    op.source_anchor.start_column += 100;
                }
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

    /// Precondition by assignment: a second L3 call site carries the first
    /// one's span. Both are classified (both `matched` against the one
    /// program edge) and the duplicate is counted.
    #[test]
    fn duplicate_l3_span_is_counted() {
        let c = census_with(&[("src/m.al", TWO_CALLS.as_bytes())], |ws| {
            let r = routine(ws, "Caller");
            r.call_sites[1] = r.call_sites[0].clone();
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
}

#[cfg(test)]
mod adapter_tests {
    use super::*;
    use crate::engine::deps::app_package_zip::test_apps;
    use crate::engine::l3::call_resolver::resolve_calls;

    const DEP_GUID: &str = "dddddddd-b3b3-0000-0000-000000000003";

    /// The adapter's output next to L3's own, over one workspace.
    struct Adapted {
        calls: ResolvedCalls,
        census: SiteCensus,
        old: ResolvedCalls,
        ws: L3Workspace,
    }

    impl Adapted {
        fn routine(&self, name: &str) -> &L3Routine {
            let mut it = self.ws.routines.iter().filter(|r| r.name == name);
            let r = it.next().unwrap_or_else(|| panic!("no routine {name}"));
            assert!(it.next().is_none(), "routine name {name} not unique");
            r
        }
        /// The routines named `name` in the object named `object_name`.
        fn routines_in(&self, object_name: &str, name: &str) -> Vec<&L3Routine> {
            let ids: Vec<&str> = self
                .ws
                .objects
                .iter()
                .filter(|o| o.name == object_name)
                .map(|o| o.id.as_str())
                .collect();
            self.ws
                .routines
                .iter()
                .filter(|r| r.name == name && ids.contains(&r.object_id.as_str()))
                .collect()
        }
        fn site(&self, caller: &str, callee_text: &str) -> &PCallSite {
            self.routine(caller)
                .call_sites
                .iter()
                .find(|cs| cs.callee_text == callee_text)
                .unwrap_or_else(|| panic!("no site {callee_text} in {caller}"))
        }
        fn edges(&self, callsite_id: &str) -> Vec<CallEdge> {
            at(&self.calls, callsite_id)
        }
        fn bindings(&self, callsite_id: &str) -> Vec<UpgradedBinding> {
            self.calls.upgraded_bindings[callsite_id].clone()
        }
    }

    fn at(calls: &ResolvedCalls, callsite_id: &str) -> Vec<CallEdge> {
        calls
            .edges
            .iter()
            .filter(|e| e.callsite_id == callsite_id)
            .cloned()
            .collect()
    }

    fn b(i: u32, is_var: bool, res: &str) -> UpgradedBinding {
        UpgradedBinding {
            parameter_index: i,
            callee_parameter_is_var: is_var,
            binding_resolution: res.to_string(),
        }
    }

    /// An expected edge: `CallEdge::base` with the given fields.
    fn edge(
        from: &L3Routine,
        cs: &PCallSite,
        to: Option<&L3Routine>,
        kind: DispatchKind,
        res: Resolution,
    ) -> CallEdge {
        let mut e = CallEdge::base(&from.id, &cs.id, &cs.operation_id);
        e.to = to.map(|t| t.id.clone());
        e.dispatch_kind = kind;
        e.resolution = res;
        e
    }

    /// Write `files` under a fresh workspace (with a dependency app built from
    /// `dep_symbols` when given), build both models, apply `mutate` to the L3
    /// model, then run the adapter and L3's own resolver.
    fn adapt_with(
        files: &[(&str, &str)],
        dep_symbols: Option<&str>,
        mutate: impl FnOnce(&mut L3Workspace),
    ) -> Adapted {
        adapt_full(
            files,
            dep_symbols.map(|s| (s, &[][..])),
            true,
            |_| {},
            mutate,
        )
    }

    /// [`adapt_with`], with the dependency app's embedded `.al` source
    /// (a source dependency) next to its symbols, the stage-2 switch given,
    /// and `mutate_graph` applied to the program context after resolution
    /// (before the adapter).
    fn adapt_full(
        files: &[(&str, &str)],
        dep: Option<(&str, &[(&str, &str)])>,
        upgrade_dependency_bindings: bool,
        mutate_graph: impl FnOnce(&mut ProgramContext),
        mutate: impl FnOnce(&mut L3Workspace),
    ) -> Adapted {
        let dir = tempfile::tempdir().unwrap();
        let deps = if dep.is_some() {
            format!(
                r#","dependencies":[{{"id":"{DEP_GUID}","name":"B3 Dep","publisher":"Microsoft","version":"28.0.0.0"}}]"#
            )
        } else {
            String::new()
        };
        std::fs::write(
            dir.path().join("app.json"),
            format!(
                r#"{{"id":"b3b3b3b3-0000-0000-0000-000000000003","name":"B3 Adapter","publisher":"T","version":"1.0.0.0"{deps}}}"#
            ),
        )
        .unwrap();
        if let Some((symbols, sources)) = dep {
            let manifest = test_apps::manifest_xml(DEP_GUID, "B3 Dep");
            let mut entries: Vec<(&str, &[u8])> = vec![
                ("NavxManifest.xml", manifest.as_bytes()),
                ("SymbolReference.json", symbols.as_bytes()),
            ];
            entries.extend(sources.iter().map(|(p, t)| (*p, t.as_bytes())));
            let app = test_apps::build_app(&entries);
            let pk = dir.path().join(".alpackages");
            std::fs::create_dir_all(&pk).unwrap();
            std::fs::write(pk.join("Microsoft_B3 Dep_28.4.app"), app).unwrap();
        }
        for (rel, text) in files {
            let p = dir.path().join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, text).unwrap();
        }
        let (mut ctx, report, mut l3) = build_models(dir.path()).unwrap();
        mutate_graph(&mut ctx);
        mutate(&mut l3.workspace);
        let (calls, census) =
            resolved_calls_from_program(&report, &ctx, &l3.workspace, upgrade_dependency_bindings);
        let ws = l3.workspace;
        let old = {
            let symbols = crate::engine::l3::symbol_table::SymbolTable::build(
                &ws.objects,
                &ws.tables,
                &ws.routines,
            );
            resolve_calls(&ws, &symbols, &[], &[])
        };
        Adapted {
            calls,
            census,
            old,
            ws,
        }
    }

    /// [`adapt_with`], no mutation. Every site must come from the program
    /// engine: a row test that L3's fallback answered would prove nothing.
    fn adapt(files: &[(&str, &str)], dep_symbols: Option<&str>) -> Adapted {
        let a = adapt_with(files, dep_symbols, |_| {});
        assert_eq!(a.census.adapter_l3_fallback_sites, 0, "{:#?}", a.census);
        a
    }

    const TABLE: &str = "table 50100 \"T\"\n{\n    fields\n    {\n        field(1; A; Code[20])\n        {\n            trigger OnValidate()\n            begin\n            end;\n        }\n        field(2; B; Code[20])\n        {\n            trigger OnValidate()\n            begin\n            end;\n        }\n    }\n\n    trigger OnInsert()\n    begin\n    end;\n\n    trigger OnModify()\n    begin\n    end;\n}\n";

    /// Engine-switch S6.0: a resolved member call's `receiver_type` is the
    /// program resolver's receiver text: a declared variable's canonical type,
    /// else the resolved object rendered (implicit `Rec` -> the page's source
    /// table, which L3 rendered as `Record rec`).
    #[test]
    fn receiver_type_comes_from_the_program_resolver() {
        let helper =
            "codeunit 50110 \"My Helper\"\n{\n    procedure Ping()\n    begin\n    end;\n}\n";
        let table = "table 50111 \"Rt Cust\"\n{\n    fields\n    {\n        field(1; A; Code[20]) { }\n    }\n\n    procedure Touch()\n    begin\n    end;\n}\n";
        let page = "page 50112 \"Rt Page\"\n{\n    SourceTable = \"Rt Cust\";\n\n    trigger OnOpenPage()\n    var\n        H: Codeunit \"My Helper\";\n        Tmp: Record \"Rt Cust\" temporary;\n    begin\n        H.Ping();\n        Rec.Touch();\n        Tmp.Touch();\n    end;\n}\n";
        let a = adapt(
            &[
                ("src/h.al", helper),
                ("src/t.al", table),
                ("src/p.al", page),
            ],
            None,
        );
        let recv = |text: &str| {
            let cs = a.site("OnOpenPage", text);
            a.edges(&cs.id)
                .into_iter()
                .map(|e| e.receiver_type)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            recv("H.Ping"),
            vec![Some("Codeunit \"My Helper\"".to_string())]
        );
        assert_eq!(
            recv("Rec.Touch"),
            vec![Some("Record \"Rt Cust\"".to_string())]
        );
        // A declaration's own text wins over the rendered object.
        assert_eq!(
            recv("Tmp.Touch"),
            vec![Some("Record \"Rt Cust\" temporary".to_string())]
        );
    }

    /// Row "Call, Exact, Routine route in workspace": a bare call is
    /// `Direct`, a member call `Method` (with the receiver's declared type);
    /// both `Resolved` to the L3 routine, and the record argument's binding
    /// is upgraded with the callee's `var`-ness.
    #[test]
    fn exact_workspace_call() {
        let cu = "codeunit 50101 \"W\"\n{\n    procedure Foo(var R: Record \"T\"; I: Integer)\n    begin\n    end;\n\n    procedure Caller()\n    var\n        R: Record \"T\";\n        Other: Codeunit \"W\";\n    begin\n        Foo(R, 1);\n        Other.Foo(R, 2);\n    end;\n}\n";
        let a = adapt(&[("src/t.al", TABLE), ("src/w.al", cu)], None);
        let (caller, foo) = (a.routine("Caller"), a.routine("Foo"));
        let bare = a.site("Caller", "Foo");
        assert_eq!(
            a.edges(&bare.id),
            vec![edge(
                caller,
                bare,
                Some(foo),
                DispatchKind::Direct,
                Resolution::Resolved
            )]
        );
        assert_eq!(
            a.bindings(&bare.id),
            vec![b(0, true, "resolved"), b(1, false, "non-record-arg")]
        );
        let member = a.site("Caller", "Other.Foo");
        let mut want = edge(
            caller,
            member,
            Some(foo),
            DispatchKind::Method,
            Resolution::Resolved,
        );
        want.receiver_type = Some("Codeunit \"W\"".to_string());
        assert_eq!(a.edges(&member.id), vec![want]);
        assert_eq!(
            a.bindings(&member.id),
            vec![b(0, true, "resolved"), b(1, false, "non-record-arg")]
        );
        assert_eq!(a.census.adapter_program_sites, 2, "{:#?}", a.census);
    }

    /// Row "Run, Routine route in workspace": `Codeunit.Run` lands on the
    /// target's `OnRun` as `CodeunitRun`, `Resolved`.
    #[test]
    fn run_into_workspace() {
        let cu = "codeunit 50101 \"W\"\n{\n    procedure Caller()\n    begin\n        Codeunit.Run(Codeunit::\"Target\");\n    end;\n}\n";
        let target = "codeunit 50102 \"Target\"\n{\n    trigger OnRun()\n    begin\n    end;\n}\n";
        let a = adapt(&[("src/w.al", cu), ("src/r.al", target)], None);
        let cs = a.site("Caller", "Codeunit.Run");
        assert_eq!(
            a.edges(&cs.id),
            vec![edge(
                a.routine("Caller"),
                cs,
                Some(a.routine("OnRun")),
                DispatchKind::CodeunitRun,
                Resolution::Resolved
            )]
        );
        assert_eq!(a.bindings(&cs.id), vec![b(0, false, "non-record-arg")]);
    }

    /// Row "Run", member spelling: L2 reads `Page.RunModal(Page::"P")` as a
    /// member call, the program engine as a run. The run kind comes from the
    /// receiver keyword: `PageRun` to the page's `OnOpenPage`.
    #[test]
    fn member_spelled_page_run_into_workspace() {
        let cu = "codeunit 50101 \"W\"\n{\n    procedure Caller()\n    begin\n        Page.RunModal(Page::\"P\");\n    end;\n}\n";
        let page = "page 50102 \"P\"\n{\n    trigger OnOpenPage()\n    begin\n    end;\n}\n";
        let a = adapt(&[("src/w.al", cu), ("src/p.al", page)], None);
        let cs = a.site("Caller", "Page.RunModal");
        assert!(
            matches!(cs.callee, PCallee::Member { .. }),
            "{:?}",
            cs.callee
        );
        assert_eq!(
            a.edges(&cs.id),
            vec![edge(
                a.routine("Caller"),
                cs,
                Some(a.routine("OnOpenPage")),
                DispatchKind::PageRun,
                Resolution::Resolved
            )]
        );
    }

    /// Row "any other Unknown": a bare call to no routine at all → to-less
    /// `Unknown(_)`; L3 spells an unresolved bare call's kind `Unresolved`.
    #[test]
    fn other_unknown_bare_call() {
        let cu = "codeunit 50101 \"W\"\n{\n    procedure Caller()\n    var\n        R: Record \"T\";\n    begin\n        Nothing(R);\n    end;\n}\n";
        let a = adapt(&[("src/t.al", TABLE), ("src/w.al", cu)], None);
        let cs = a.site("Caller", "Nothing");
        assert_eq!(
            a.edges(&cs.id),
            vec![edge(
                a.routine("Caller"),
                cs,
                None,
                DispatchKind::Unresolved,
                Resolution::Unknown(L3Reason::BareUnresolved)
            )]
        );
        assert_eq!(a.bindings(&cs.id), vec![b(0, false, "unresolved-callee")]);
    }

    /// The dependency app for the interface and dependency-callee rows:
    /// interface `IDep` (`Go(var C: Record Customer)`), its dependency
    /// implementer `DepImpl`, codeunit 80 `Sales-Post` (`Post(var C)`), and
    /// table 18 `Customer` with a table procedure `DepProc()` and an
    /// `OnInsert` trigger symbol.
    const DEP_SYMBOLS: &str = r#"{"Tables":[{"Id":18,"Name":"Customer","Fields":[{"Id":1,"Name":"No.","TypeDefinition":{"Name":"Code"}}],"Methods":[{"Name":"DepProc","Parameters":[]},{"Name":"OnInsert","Parameters":[]}]}],"Interfaces":[{"Name":"IDep","Methods":[{"Name":"Go","Parameters":[{"Name":"C","IsVar":true,"TypeDefinition":{"Name":"Record","Subtype":{"Name":"Customer","Id":18}}}]}]}],"Codeunits":[{"Id":80,"Name":"Sales-Post","Methods":[{"Name":"Post","Parameters":[{"Name":"C","IsVar":true,"TypeDefinition":{"Name":"Record","Subtype":{"Name":"Customer","Id":18}}}]}]},{"Id":81,"Name":"DepImpl","ImplementedInterfaces":["IDep"],"Methods":[{"Name":"Go","Parameters":[{"Name":"C","IsVar":true,"TypeDefinition":{"Name":"Record","Subtype":{"Name":"Customer","Id":18}}}]}]}]}"#;

    /// Row "Polymorphic (interface)", engine-switch S3.2: one `Interface`+`Maybe`
    /// edge to the workspace implementer, `dispatch_meta` on it, and one to-less
    /// `Interface`+`ExternalTarget` edge naming the dependency implementer
    /// `DepImpl` (its route used to be dropped). `total_impls` counts both.
    /// Bindings: ambiguous.
    #[test]
    fn interface_with_workspace_and_dependency_implementers() {
        let ws_impl = "codeunit 50103 \"WsImpl\" implements IDep\n{\n    procedure Go(var C: Record Customer)\n    begin\n    end;\n}\n";
        let cu = "codeunit 50101 \"W\"\n{\n    procedure Caller()\n    var\n        X: Interface IDep;\n        C: Record Customer;\n    begin\n        X.Go(C);\n    end;\n}\n";
        let a = adapt(
            &[("src/i.al", ws_impl), ("src/w.al", cu)],
            Some(DEP_SYMBOLS),
        );
        let cs = a.site("Caller", "X.Go");
        let mut want = edge(
            a.routine("Caller"),
            cs,
            Some(a.routine("Go")),
            DispatchKind::Interface,
            Resolution::Maybe,
        );
        want.dispatch_meta = Some(DispatchMeta {
            interface_name: "IDep".to_string(),
            total_impls: 2,
            unresolved_impls: Vec::new(),
            enum_implementers: Vec::new(),
        });
        let mut dep = edge(
            a.routine("Caller"),
            cs,
            None,
            DispatchKind::Interface,
            Resolution::ExternalTarget,
        );
        dep.external_type_ref = Some(ExternalTypeRef {
            kind: "Codeunit".to_string(),
            name: "DepImpl".to_string(),
        });
        assert_eq!(a.edges(&cs.id), vec![want, dep]);
        assert_eq!(a.bindings(&cs.id), vec![b(0, false, "ambiguous")]);
        assert_eq!(a.census.adapter_routes_dropped, 0, "{:#?}", a.census);
        assert_eq!(
            a.census.adapter_interface_dependency_impls, 1,
            "{:#?}",
            a.census
        );
        // S3.3: the symbol-only implementer's routine is named, bodyless.
        let targets: Vec<_> = a
            .calls
            .external_targets
            .iter()
            .filter(|t| t.callsite_id == cs.id)
            .collect();
        assert_eq!(targets.len(), 1, "{targets:?}");
        assert!(
            targets[0].target.ends_with("/Codeunit/81::go/1"),
            "{}",
            targets[0].target
        );
        assert_eq!(
            targets[0].body,
            Some(crate::program::registry::BodyState::Bodyless)
        );
    }

    /// Minor 5, precondition by assignment: the workspace implementer's L3
    /// declaration anchor is moved, so its program route joins no L3
    /// routine. Since engine-switch S3.1 the site is one honest
    /// `Unknown(NoProgramSite)` edge (it used to fall back to L3) and is counted.
    #[test]
    fn interface_implementer_outside_l3_is_unknown() {
        let ws_impl = "codeunit 50103 \"WsImpl\" implements IDep\n{\n    procedure Go(var C: Record Customer)\n    begin\n    end;\n}\n";
        let cu = "codeunit 50101 \"W\"\n{\n    procedure Caller()\n    var\n        X: Interface IDep;\n        C: Record Customer;\n    begin\n        X.Go(C);\n    end;\n}\n";
        let a = adapt_with(
            &[("src/i.al", ws_impl), ("src/w.al", cu)],
            Some(DEP_SYMBOLS),
            |ws| {
                let go = ws.routines.iter_mut().find(|r| r.name == "Go").unwrap();
                go.source_anchor.start_column += 100;
            },
        );
        let cs = a.site("Caller", "X.Go");
        let got = a.edges(&cs.id);
        assert_eq!(got.len(), 1, "{got:?}");
        assert_eq!(
            got[0].resolution,
            Resolution::Unknown(L3Reason::NoProgramSite)
        );
        let c = &a.census;
        assert_eq!(
            (
                c.adapter_callee_outside_l3,
                c.adapter_l3_fallback_sites,
                c.adapter_program_sites
            ),
            (1, 1, 0),
            "{c:#?}"
        );
    }

    /// Minor 4: a run into a WORKSPACE object with no entry trigger is
    /// `Opaque` with no `external_type_ref` (the program route is an ABI
    /// symbol in our own app; there is no external type to name).
    #[test]
    fn run_into_workspace_object_without_entry_trigger() {
        let cu = "codeunit 50101 \"W\"\n{\n    procedure Caller()\n    begin\n        Codeunit.Run(Codeunit::\"Empty\");\n    end;\n}\n";
        let empty = "codeunit 50102 \"Empty\"\n{\n}\n";
        let a = adapt(&[("src/w.al", cu), ("src/e.al", empty)], None);
        let cs = a.site("Caller", "Codeunit.Run");
        let want = edge(
            a.routine("Caller"),
            cs,
            None,
            DispatchKind::CodeunitRun,
            Resolution::Opaque,
        );
        assert_eq!(a.edges(&cs.id), vec![want.clone()]);
        assert_eq!(at(&a.old, &cs.id), vec![want], "L3 agrees");
    }

    /// A run through a page VARIABLE on a workspace page with no entry
    /// trigger is the same shape as `Page.Run(Page::X)` on it: `PageRun`,
    /// `Opaque`, no external type (the object is ours, not external).
    #[test]
    fn page_variable_run_into_workspace_object_without_entry_trigger() {
        let cu = "codeunit 50101 \"W\"\n{\n    procedure Caller()\n    var\n        P: Page \"Empty\";\n    begin\n        P.RunModal();\n    end;\n}\n";
        let empty = "page 50102 \"Empty\"\n{\n}\n";
        let a = adapt(&[("src/w.al", cu), ("src/e.al", empty)], None);
        let cs = a.site("Caller", "P.RunModal");
        let want = edge(
            a.routine("Caller"),
            cs,
            None,
            DispatchKind::PageRun,
            Resolution::Opaque,
        );
        assert_eq!(a.edges(&cs.id), vec![want]);
        assert_eq!(
            a.census.adapter_external_object_receiver, 0,
            "{:#?}",
            a.census
        );
        assert_eq!(
            a.census.adapter_workspace_run_no_entry, 1,
            "{:#?}",
            a.census
        );
    }

    /// The same arm for a codeunit VARIABLE and a report VARIABLE (review
    /// Minor 7): `CuVar.Run()` on a codeunit with no `OnRun` and `RepVar.Run()`
    /// on a report with no trigger are `Opaque` runs of their own kind, with no
    /// external type, and both are counted.
    #[test]
    fn codeunit_and_report_variable_run_into_workspace_object_without_entry_trigger() {
        let cu = "codeunit 50101 \"W\"\n{\n    procedure Caller()\n    var\n        C: Codeunit \"EmptyCu\";\n        R: Report \"EmptyRep\";\n    begin\n        C.Run();\n        R.Run();\n    end;\n}\n";
        let empty_cu = "codeunit 50102 \"EmptyCu\"\n{\n}\n";
        let empty_rep = "report 50103 \"EmptyRep\"\n{\n}\n";
        let a = adapt(
            &[
                ("src/w.al", cu),
                ("src/c.al", empty_cu),
                ("src/r.al", empty_rep),
            ],
            None,
        );
        for (callee, kind) in [
            ("C.Run", DispatchKind::CodeunitRun),
            ("R.Run", DispatchKind::ReportRun),
        ] {
            let cs = a.site("Caller", callee);
            let want = edge(a.routine("Caller"), cs, None, kind, Resolution::Opaque);
            assert_eq!(a.edges(&cs.id), vec![want], "{callee}");
        }
        assert_eq!(
            (
                a.census.adapter_workspace_run_no_entry,
                a.census.adapter_external_object_receiver
            ),
            (2, 0),
            "{:#?}",
            a.census
        );
    }

    /// Row "AmbiguousOverload" into a dependency (S3.6): both overloads are
    /// in a dependency, so the edge is an external call, and both candidates
    /// keep their identity in `external_targets`. They used to be dropped.
    #[test]
    fn ambiguous_overload_in_a_dependency_keeps_its_candidates() {
        let symbols = r#"{"Codeunits":[{"Id":90,"Name":"Over","Methods":[{"Name":"F","Parameters":[{"Name":"A","TypeDefinition":{"Name":"Integer"}}]},{"Name":"F","Parameters":[{"Name":"A","TypeDefinition":{"Name":"Decimal"}}]}]}]}"#;
        let cu = "codeunit 50101 \"W\"\n{\n    procedure Caller()\n    var\n        O: Codeunit Over;\n        V: Variant;\n    begin\n        O.F(V);\n    end;\n}\n";
        let a = adapt(&[("src/w.al", cu)], Some(symbols));
        let cs = a.site("Caller", "O.F");
        let got = a.edges(&cs.id);
        assert_eq!(got.len(), 1, "{got:#?}");
        assert_eq!(got[0].to, None);
        assert_eq!(got[0].resolution, Resolution::ExternalTarget);
        let mut targets: Vec<&str> = a
            .calls
            .external_targets
            .iter()
            .filter(|t| t.callsite_id == cs.id)
            .map(|t| t.target.as_str())
            .collect();
        targets.sort();
        assert_eq!(targets.len(), 2, "{targets:?}");
        assert!(
            targets.iter().all(|t| t.ends_with("/Codeunit/90::f/1")),
            "{targets:?}"
        );
        assert_eq!(
            a.census.adapter_ambiguous_dependency_candidates, 2,
            "{:#?}",
            a.census
        );
        assert_eq!(a.census.adapter_routes_dropped, 0, "{:#?}", a.census);
    }

    /// Row "AmbiguousOverload": two same-arity overloads a `Variant`
    /// argument cannot pick between → to-less `Ambiguous` with both L3 ids
    /// as candidates; the record binding becomes `"ambiguous"`.
    #[test]
    fn ambiguous_overload() {
        let cu = "codeunit 50101 \"W\"\n{\n    procedure F(var R: Record \"T\"; A: Integer)\n    begin\n    end;\n\n    procedure F(var R: Record \"T\"; A: Decimal)\n    begin\n    end;\n\n    procedure Caller()\n    var\n        R: Record \"T\";\n        V: Variant;\n    begin\n        F(R, V);\n    end;\n}\n";
        let a = adapt(&[("src/t.al", TABLE), ("src/w.al", cu)], None);
        let cs = a.site("Caller", "F");
        let mut ids: Vec<String> = a
            .routines_in("W", "F")
            .iter()
            .map(|r| r.id.clone())
            .collect();
        ids.sort();
        assert_eq!(ids.len(), 2);
        let mut want = edge(
            a.routine("Caller"),
            cs,
            None,
            DispatchKind::Direct,
            Resolution::Ambiguous,
        );
        want.candidates = Some(ids);
        assert_eq!(a.edges(&cs.id), vec![want]);
        assert_eq!(
            a.bindings(&cs.id),
            vec![b(0, false, "ambiguous"), b(1, false, "non-record-arg")]
        );
    }

    /// Row "DynamicOpen": a run on a runtime target → to-less `Dynamic`.
    #[test]
    fn dynamic_run_target() {
        let cu = "codeunit 50101 \"W\"\n{\n    procedure Caller()\n    var\n        Id: Integer;\n    begin\n        Codeunit.Run(Id);\n    end;\n}\n";
        let a = adapt(&[("src/w.al", cu)], None);
        let cs = a.site("Caller", "Codeunit.Run");
        assert_eq!(
            a.edges(&cs.id),
            vec![edge(
                a.routine("Caller"),
                cs,
                None,
                DispatchKind::Dynamic,
                Resolution::Unknown(L3Reason::DynamicObjectRunTarget)
            )]
        );
        assert_eq!(a.bindings(&cs.id), vec![b(0, false, "non-record-arg")]);
    }

    /// Row "Catalog route (builtin)": to-less `Builtin`.
    #[test]
    fn builtin() {
        let cu = "codeunit 50101 \"W\"\n{\n    procedure Caller()\n    begin\n        Message('x');\n    end;\n}\n";
        let a = adapt(&[("src/w.al", cu)], None);
        let cs = a.site("Caller", "Message");
        assert_eq!(
            a.edges(&cs.id),
            vec![edge(
                a.routine("Caller"),
                cs,
                None,
                DispatchKind::Builtin,
                Resolution::Builtin
            )]
        );
        assert_eq!(a.bindings(&cs.id), vec![b(0, false, "non-record-arg")]);
    }

    /// Row "Routine route in a dependency": a call into a dependency
    /// codeunit and a dependency table procedure → to-less `ExternalTarget`
    /// carrying the program object's kind and name. Bindings stay
    /// `"unresolved-callee"` (stage 1).
    #[test]
    fn dependency_callee_is_external_target() {
        let cu = "codeunit 50101 \"W\"\n{\n    procedure Caller()\n    var\n        SP: Codeunit \"Sales-Post\";\n        C: Record Customer;\n    begin\n        SP.Post(C);\n        C.DepProc();\n        SP.Missing();\n    end;\n}\n";
        let a = adapt(&[("src/w.al", cu)], Some(DEP_SYMBOLS));
        let caller = a.routine("Caller");
        let post = a.site("Caller", "SP.Post");
        let mut want = edge(
            caller,
            post,
            None,
            DispatchKind::Method,
            Resolution::ExternalTarget,
        );
        want.external_type_ref = Some(ExternalTypeRef {
            kind: "Codeunit".to_string(),
            name: "Sales-Post".to_string(),
        });
        assert_eq!(a.edges(&post.id), vec![want]);
        // Stage 2: `Post(var C)` is `AbiParams::Complete` (see
        // `symbol_only_dependency_bindings_by_abi_params`).
        assert_eq!(a.bindings(&post.id), vec![b(0, true, "resolved")]);
        let proc_ = a.site("Caller", "C.DepProc");
        let mut want = edge(
            caller,
            proc_,
            None,
            DispatchKind::Method,
            Resolution::ExternalTarget,
        );
        want.external_type_ref = Some(ExternalTypeRef {
            kind: "Table".to_string(),
            name: "Customer".to_string(),
        });
        assert_eq!(a.edges(&proc_.id), vec![want]);
        // L3 calls the record receiver's case `Unknown(RecordTableProcedure)`:
        // the adapter changes its uncertainty kind, and counts it apart.
        assert_eq!(
            at(&a.old, &proc_.id)[0].resolution,
            Resolution::Unknown(L3Reason::RecordTableProcedure)
        );
        // A member the dependency's ABI lacks: a member decline on a
        // dependency receiver, also a dependency callee.
        let missing = a.site("Caller", "SP.Missing");
        let mut want = edge(
            caller,
            missing,
            None,
            DispatchKind::Method,
            Resolution::ExternalTarget,
        );
        want.external_type_ref = Some(ExternalTypeRef {
            kind: "Codeunit".to_string(),
            name: "Sales-Post".to_string(),
        });
        assert_eq!(a.edges(&missing.id), vec![want]);
        // S3.3: the two exact dependency callees are named (bodyless, symbol-only);
        // the member decline (`SP.Missing`) has no routine to name.
        let named: Vec<(&str, &str, Option<crate::program::registry::BodyState>)> = a
            .calls
            .external_targets
            .iter()
            .map(|t| {
                let site = if t.callsite_id == post.id {
                    "post"
                } else if t.callsite_id == proc_.id {
                    "proc"
                } else {
                    "other"
                };
                (site, t.target.rsplit("::").next().unwrap(), t.body)
            })
            .collect();
        let bodyless = Some(crate::program::registry::BodyState::Bodyless);
        assert_eq!(
            named,
            vec![
                ("post", "post/1", bodyless),
                ("proc", "depproc/0", bodyless)
            ]
        );
        let c = &a.census;
        assert_eq!(c.adapter_program_sites, 3, "{c:#?}");
        assert_eq!(
            (
                c.adapter_external_record_receiver,
                c.adapter_external_object_receiver,
                c.adapter_external_other,
                c.adapter_external_member_decline
            ),
            (1, 2, 0, 1),
            "{c:#?}"
        );
    }

    /// Stage 2 fixture: dependency table 18 `Customer` and codeunit 82
    /// `DepMix` with `Mix(var A: Record Customer; B: Record Customer)` and
    /// two same-arity `Ov` overloads a `Variant` cannot pick between.
    const DEP_MIX: &str = r#"{"Tables":[{"Id":18,"Name":"Customer","Fields":[{"Id":1,"Name":"No.","TypeDefinition":{"Name":"Code"}}]}],"Codeunits":[{"Id":82,"Name":"DepMix","Methods":[{"Name":"Mix","Parameters":[{"Name":"A","IsVar":true,"TypeDefinition":{"Name":"Record","Subtype":{"Name":"Customer","Id":18}}},{"Name":"B","IsVar":false,"TypeDefinition":{"Name":"Record","Subtype":{"Name":"Customer","Id":18}}}]},{"Name":"Ov","Parameters":[{"Name":"A","IsVar":true,"TypeDefinition":{"Name":"Record","Subtype":{"Name":"Customer","Id":18}}},{"Name":"X","IsVar":false,"TypeDefinition":{"Name":"Integer"}}]},{"Name":"Ov","Parameters":[{"Name":"A","IsVar":true,"TypeDefinition":{"Name":"Record","Subtype":{"Name":"Customer","Id":18}}},{"Name":"X","IsVar":false,"TypeDefinition":{"Name":"Decimal"}}]},{"Name":"Dup","Parameters":[{"Name":"A","IsVar":true,"TypeDefinition":{"Name":"Record","Subtype":{"Name":"Customer","Id":18}}}]},{"Name":"Dup","Parameters":[{"Name":"A","IsVar":true,"TypeDefinition":{"Name":"Record","Subtype":{"Name":"Customer","Id":18}}}]}]}]}"#;

    const MIX_CALLER: &str = "codeunit 50101 \"W\"\n{\n    procedure Caller()\n    var\n        M: Codeunit \"DepMix\";\n        A: Record Customer;\n        B: Record Customer;\n        V: Variant;\n    begin\n        M.Mix(A, B);\n        M.Ov(A, V);\n        M.Dup(A);\n    end;\n}\n";

    /// The `M.Mix` site of [`MIX_CALLER`] over [`DEP_MIX`], with the stage-2
    /// switch given and `abi_params` of `Mix`'s graph node replaced by
    /// `params` when given (precondition by assignment).
    fn mix(
        upgrade: bool,
        params: Option<AbiParams>,
    ) -> (Adapted, Vec<CallEdge>, Vec<UpgradedBinding>) {
        let a = adapt_full(
            &[("src/w.al", MIX_CALLER)],
            Some((DEP_MIX, &[][..])),
            upgrade,
            |ctx| {
                if let Some(p) = params {
                    let mut hit = 0;
                    for n in ctx.graph.routines.iter_mut() {
                        if n.id.name_lc == "mix" {
                            n.abi_params = p.clone();
                            hit += 1;
                        }
                    }
                    assert_eq!(hit, 1, "the assignment applied");
                }
            },
            |_| {},
        );
        let cs = a.site("Caller", "M.Mix").clone();
        let (edges, bindings) = (a.edges(&cs.id), a.bindings(&cs.id));
        (a, edges, bindings)
    }

    /// Stage 2, symbol-only dependency with `AbiParams::Complete`: the
    /// bindings are upgraded exactly as for a workspace callee (`"resolved"`,
    /// `var`-ness per parameter); the edge is unchanged and to-less.
    /// Without the switch they stay `"unresolved-callee"`.
    #[test]
    fn symbol_only_dependency_bindings_by_abi_params() {
        let (a, edges, bindings) = mix(true, None);
        assert_eq!(
            bindings,
            vec![b(0, true, "resolved"), b(1, false, "resolved")]
        );
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].to, None);
        assert_eq!(edges[0].resolution, Resolution::ExternalTarget);
        assert_eq!(
            (
                a.census.adapter_dep_bindings_source,
                a.census.adapter_dep_bindings_symbol
            ),
            (0, 1),
            "{:#?}",
            a.census
        );
        let (a0, edges0, bindings0) = mix(false, None);
        assert_eq!(
            bindings0,
            vec![
                b(0, false, "unresolved-callee"),
                b(1, false, "unresolved-callee")
            ]
        );
        assert_eq!(edges0, edges, "the switch moves bindings only");
        assert_eq!(a0.census.adapter_dep_bindings_symbol, 0);
    }

    /// Stage 2: `AbiParams::Missing` and `CollapsedUntrusted` are not
    /// trusted — the bindings stay `"unresolved-callee"`, counted apart.
    #[test]
    fn untrusted_abi_params_leave_bindings_unresolved() {
        let unresolved = vec![
            b(0, false, "unresolved-callee"),
            b(1, false, "unresolved-callee"),
        ];
        let (a, edges, bindings) = mix(true, Some(AbiParams::Missing));
        assert_eq!(bindings, unresolved);
        assert_eq!(edges[0].to, None);
        let c = &a.census;
        assert_eq!(
            (
                c.adapter_dep_bindings_symbol,
                c.adapter_dep_bindings_missing,
                c.adapter_dep_bindings_collapsed
            ),
            (0, 1, 0),
            "{c:#?}"
        );
        let (a, edges, bindings) = mix(true, Some(AbiParams::CollapsedUntrusted));
        assert_eq!(bindings, unresolved);
        assert_eq!(edges[0].to, None);
        let c = &a.census;
        assert_eq!(
            (
                c.adapter_dep_bindings_symbol,
                c.adapter_dep_bindings_missing,
                c.adapter_dep_bindings_collapsed
            ),
            (0, 0, 1),
            "{c:#?}"
        );
    }

    /// Stage 2 leaves a dependency overload set alone: an ambiguous pick
    /// (`Ov` with a `Variant`) and an ABI-collapsed overload (`Dup`, two
    /// identical raw entries) have no single callee, so their bindings
    /// stay what stage 1 gives, and no dependency callee gets a `to`.
    #[test]
    fn dependency_overload_sets_keep_stage_one_bindings() {
        let with = adapt_full(
            &[("src/w.al", MIX_CALLER)],
            Some((DEP_MIX, &[][..])),
            true,
            |_| {},
            |_| {},
        );
        let without = adapt_full(
            &[("src/w.al", MIX_CALLER)],
            Some((DEP_MIX, &[][..])),
            false,
            |_| {},
            |_| {},
        );
        for callee in ["M.Ov", "M.Dup"] {
            let cs = with.site("Caller", callee);
            assert_eq!(with.bindings(&cs.id), without.bindings(&cs.id), "{callee}");
            assert_eq!(with.edges(&cs.id), without.edges(&cs.id), "{callee}");
            assert!(
                with.edges(&cs.id).iter().all(|e| e.to.is_none()),
                "{callee}"
            );
            assert!(
                with.bindings(&cs.id)
                    .iter()
                    .all(|x| x.binding_resolution != "resolved"),
                "{callee}: {:?}",
                with.bindings(&cs.id)
            );
        }
        // Only `M.Mix` reached stage 2: neither overload set is an exact
        // route into one dependency routine.
        let c = &with.census;
        assert_eq!(
            (
                c.adapter_dep_bindings_source,
                c.adapter_dep_bindings_symbol,
                c.adapter_dep_bindings_missing,
                c.adapter_dep_bindings_collapsed
            ),
            (0, 1, 0, 0),
            "{c:#?}"
        );
    }

    /// Stage 2, source dependency: the callee's `var`-ness comes from its
    /// declaration (`DeclSurface`), giving the same binding strings L3 gives
    /// a workspace callee. The dependency `.app` embeds its source (L3 does
    /// not read dependencies); its callee stays to-less.
    #[test]
    fn source_dependency_bindings_by_declaration() {
        // The ABI deliberately says `R` is by value; the source says `var R`.
        // The upgrade must follow the declaration.
        let symbols = r#"{"Tables":[{"Id":70001,"Name":"DT","Fields":[{"Id":1,"Name":"A","TypeDefinition":{"Name":"Code"}}]}],"Pages":[{"Id":70002,"Name":"DP"}],"Codeunits":[{"Id":70000,"Name":"SrcDep","Methods":[{"Name":"Go","Parameters":[{"Name":"R","IsVar":false,"TypeDefinition":{"Name":"Record","Subtype":{"Name":"DT","Id":70001}}},{"Name":"S","IsVar":false,"TypeDefinition":{"Name":"Record","Subtype":{"Name":"DT","Id":70001}}}]}]}]}"#;
        let dep = "codeunit 70000 \"SrcDep\"\n{\n    procedure Go(var R: Record \"DT\"; S: Record \"DT\")\n    begin\n    end;\n}\n";
        let table = "table 70001 \"DT\"\n{\n    fields\n    {\n        field(1; A; Code[20]) { }\n    }\n}\n";
        let page = "page 70002 \"DP\"\n{\n    trigger OnOpenPage()\n    begin\n    end;\n}\n";
        let cu = "codeunit 50101 \"W\"\n{\n    procedure Caller()\n    var\n        D: Codeunit \"SrcDep\";\n        R: Record \"DT\";\n        S: Record \"DT\";\n    begin\n        D.Go(R, S);\n        Page.Run(Page::\"DP\", R);\n    end;\n}\n";
        let sources = [("src/d.al", dep), ("src/t.al", table), ("src/p.al", page)];
        let run = |upgrade: bool| {
            let a = adapt_full(
                &[("src/w.al", cu)],
                Some((symbols, &sources[..])),
                upgrade,
                |_| {},
                |_| {},
            );
            let cs = a.site("Caller", "D.Go").clone();
            let (e, bs) = (a.edges(&cs.id), a.bindings(&cs.id));
            (a, e, bs)
        };
        let (a, edges, bindings) = run(true);
        // A run into the dependency page's `OnOpenPage()`: the record
        // argument sits past its (zero) parameters, so nothing changes and
        // the site is not counted as an upgrade.
        let run_site = a.site("Caller", "Page.Run");
        assert_eq!(
            a.edges(&run_site.id)[0].resolution,
            Resolution::Opaque,
            "{:?}",
            a.edges(&run_site.id)
        );
        assert_eq!(
            a.bindings(&run_site.id),
            vec![
                b(0, false, "non-record-arg"),
                b(1, false, "unresolved-callee")
            ]
        );
        assert_eq!(
            bindings,
            vec![b(0, true, "resolved"), b(1, false, "resolved")],
            "{edges:?} {:#?}",
            a.census
        );
        assert_eq!(edges.len(), 1, "{edges:?}");
        assert_eq!(edges[0].to, None);
        assert_eq!(edges[0].resolution, Resolution::ExternalTarget);
        assert_eq!(
            (
                a.census.adapter_dep_bindings_source,
                a.census.adapter_dep_bindings_symbol
            ),
            (1, 0),
            "{:#?}",
            a.census
        );
        let (_, edges0, bindings0) = run(false);
        assert_eq!(
            bindings0,
            vec![
                b(0, false, "unresolved-callee"),
                b(1, false, "unresolved-callee")
            ]
        );
        assert_eq!(edges0, edges);
    }

    /// Stage 2, two symbol-only shapes: a codeunit with no object number
    /// (keyed by NAME, so the ABI key maps back through `ObjKey::Name`) is
    /// upgraded from `AbiParams::Complete`; a run into a dependency page
    /// with no entry trigger names no routine (a placeholder key), so its
    /// record argument stays `"unresolved-callee"`, counted apart.
    #[test]
    fn name_keyed_symbol_callee_and_no_routine_run() {
        let symbols = r#"{"Tables":[{"Id":18,"Name":"Customer","Fields":[{"Id":1,"Name":"No.","TypeDefinition":{"Name":"Code"}}]}],"Pages":[{"Id":83,"Name":"NoTrig"}],"Codeunits":[{"Name":"NamedDep","Methods":[{"Name":"Nm","Parameters":[{"Name":"A","IsVar":true,"TypeDefinition":{"Name":"Record","Subtype":{"Name":"Customer","Id":18}}}]}]}]}"#;
        let cu = "codeunit 50101 \"W\"\n{\n    procedure Caller()\n    var\n        N: Codeunit \"NamedDep\";\n        A: Record Customer;\n    begin\n        N.Nm(A);\n        Page.Run(Page::\"NoTrig\", A);\n    end;\n}\n";
        let a = adapt_full(
            &[("src/w.al", cu)],
            Some((symbols, &[][..])),
            true,
            |ctx| {
                let named = ctx.graph.routines.iter().any(|n| {
                    n.id.name_lc == "nm" && n.id.object.key == ObjKey::Name("nameddep".to_string())
                });
                assert!(named, "precondition: the codeunit is keyed by name");
            },
            |_| {},
        );
        let nm = a.site("Caller", "N.Nm");
        assert_eq!(a.bindings(&nm.id), vec![b(0, true, "resolved")]);
        assert!(a.edges(&nm.id).iter().all(|e| e.to.is_none()));
        let run = a.site("Caller", "Page.Run");
        assert_eq!(
            a.bindings(&run.id),
            vec![
                b(0, false, "non-record-arg"),
                b(1, false, "unresolved-callee")
            ]
        );
        let c = &a.census;
        assert_eq!(
            (
                c.adapter_dep_bindings_symbol,
                c.adapter_dep_bindings_missing,
                c.adapter_dep_bindings_no_routine
            ),
            (1, 0, 1),
            "{c:#?}"
        );
    }

    /// Row "ObjectNotInGraph": an object named but absent everywhere → a
    /// to-less `ExternalTarget` (a run: `Opaque`), the type ref from the L2
    /// receiver type / run target.
    #[test]
    fn object_not_in_graph_is_external_target() {
        let cu = "codeunit 50101 \"W\"\n{\n    procedure Caller()\n    var\n        N: Codeunit \"Nowhere\";\n    begin\n        N.Go();\n        Codeunit.Run(Codeunit::\"Nowhere\");\n    end;\n}\n";
        let a = adapt(&[("src/w.al", cu)], None);
        let caller = a.routine("Caller");
        let nowhere = || {
            Some(ExternalTypeRef {
                kind: "Codeunit".to_string(),
                name: "Nowhere".to_string(),
            })
        };
        let go = a.site("Caller", "N.Go");
        let mut want = edge(
            caller,
            go,
            None,
            DispatchKind::Method,
            Resolution::ExternalTarget,
        );
        want.external_type_ref = nowhere();
        assert_eq!(a.edges(&go.id), vec![want]);
        let run = a.site("Caller", "Codeunit.Run");
        let mut want = edge(
            caller,
            run,
            None,
            DispatchKind::CodeunitRun,
            Resolution::Opaque,
        );
        want.external_type_ref = nowhere();
        assert_eq!(a.edges(&run.id), vec![want]);
    }

    /// Row "ImplicitTrigger, workspace trigger route": `Insert` →
    /// `OnInsert` (`Maybe`); `Validate(A)` → field A's `OnValidate` only
    /// (`Resolved`); `Modify(false)` → nothing. Keyed by the record op id.
    #[test]
    fn implicit_trigger_to_workspace() {
        let cu = "codeunit 50101 \"W\"\n{\n    procedure Caller()\n    var\n        R: Record \"T\";\n    begin\n        R.Insert(true);\n        R.Validate(A, 'x');\n        R.Modify(false);\n    end;\n}\n";
        let a = adapt(&[("src/t.al", TABLE), ("src/w.al", cu)], None);
        let caller = a.routine("Caller");
        let op = |name: &str| {
            caller
                .record_operations
                .iter()
                .find(|o| o.op == name)
                .unwrap()
        };
        let on_validate_a = a
            .routines_in("T", "OnValidate")
            .into_iter()
            .find(|r| r.enclosing_member.as_deref() == Some("A"))
            .unwrap();
        let trig = |o: &L3RecordOperation, to: &L3Routine, res: Resolution| {
            let mut e = CallEdge::base(&caller.id, &o.id, &o.id);
            e.to = Some(to.id.clone());
            e.dispatch_kind = DispatchKind::ImplicitTrigger;
            e.resolution = res;
            e
        };
        assert_eq!(
            a.edges(&op("Insert").id),
            vec![trig(op("Insert"), a.routine("OnInsert"), Resolution::Maybe)]
        );
        assert_eq!(
            a.edges(&op("Validate").id),
            vec![trig(op("Validate"), on_validate_a, Resolution::Resolved)]
        );
        assert_eq!(a.edges(&op("Modify").id), vec![]);
        assert!(!a.calls.upgraded_bindings.contains_key(&op("Insert").id));
        assert_eq!(a.census.adapter_program_trigger_ops, 3, "{:#?}", a.census);
        // Engine-switch S3.4: the program resolver already dropped the two
        // routes (field B's `OnValidate`, `Modify(false)`'s `OnModify`) that the
        // adapter used to filter; its agreement check finds none left.
        assert_eq!(
            a.census.adapter_trigger_routes_filtered, 0,
            "{:#?}",
            a.census
        );
    }

    /// A trigger edge for the op `op` of routine `caller`.
    fn trigger_edge(caller: &L3Routine, op: &L3RecordOperation, to: &L3Routine) -> CallEdge {
        let mut e = CallEdge::base(&caller.id, &op.id, &op.id);
        e.to = Some(to.id.clone());
        e.dispatch_kind = DispatchKind::ImplicitTrigger;
        e.resolution = Resolution::Maybe;
        e
    }

    /// `Rename` (#9): BOTH engines treat `R.Rename(..)` as a record op -- L2
    /// emits an `L3RecordOperation`, and the program extractor (which reads
    /// the same `record_op_type` table) classifies a `RecordOp` that
    /// `resolve_implicit_trigger` routes to `OnRename`. The two pair up as a
    /// matched implicit trigger, the edge is `Resolved` (Rename takes no
    /// RunTrigger and always fires OnRename, measured on BC 28), L3's own
    /// answer agrees, and nothing counts as "beyond L3". Before #9 neither
    /// engine did this, and the site was an ordinary built-in call.
    #[test]
    fn rename_fires_on_rename_on_both_sides() {
        let table = "table 50100 \"T\"\n{\n    fields\n    {\n        field(1; Code; Code[20]) { }\n    }\n\n    trigger OnRename()\n    begin\n    end;\n}\n";
        let cu = "codeunit 50101 \"W\"\n{\n    procedure Caller()\n    var\n        R: Record \"T\";\n    begin\n        R.Rename('NEW');\n    end;\n}\n";
        let a = adapt(&[("src/t.al", table), ("src/w.al", cu)], None);
        let caller = a.routine("Caller");
        let op = caller
            .record_operations
            .iter()
            .find(|o| o.op == "Rename")
            .expect("L2: Rename is a record op");
        let mut want = trigger_edge(caller, op, a.routine("OnRename"));
        want.resolution = Resolution::Resolved;
        assert_eq!(a.edges(&op.id), vec![want.clone()]);
        assert_eq!(at(&a.old, &op.id), vec![want], "L3 agrees");
        let c = &a.census;
        assert_eq!(
            (c.implicit_trigger_matched, c.implicit_trigger_unmatched),
            (1, 0),
            "{c:#?}"
        );
    }

    /// Beyond L3: an `Insert` also fires a TableExtension's `OnInsert`. Both
    /// edges are emitted, sorted by `to`.
    #[test]
    fn table_extension_trigger_is_emitted_beyond_l3() {
        let table = "table 50100 \"T\"\n{\n    fields\n    {\n        field(1; Code; Code[20]) { }\n    }\n\n    trigger OnInsert()\n    begin\n    end;\n}\n";
        let ext = "tableextension 50110 \"TExt\" extends \"T\"\n{\n    trigger OnInsert()\n    begin\n    end;\n}\n";
        let cu = "codeunit 50101 \"W\"\n{\n    procedure Caller()\n    var\n        R: Record \"T\";\n    begin\n        R.Insert(true);\n    end;\n}\n";
        let a = adapt(
            &[("src/t.al", table), ("src/e.al", ext), ("src/w.al", cu)],
            None,
        );
        let caller = a.routine("Caller");
        let op = &caller.record_operations[0];
        let base = a.routines_in("T", "OnInsert");
        let extension = a.routines_in("TExt", "OnInsert");
        assert_eq!((base.len(), extension.len()), (1, 1));
        let l3 = at(&a.old, &op.id);
        assert_eq!(l3, vec![trigger_edge(caller, op, base[0])], "L3: base only");
        let mut want = vec![
            trigger_edge(caller, op, base[0]),
            trigger_edge(caller, op, extension[0]),
        ];
        want.sort_by(|x, y| x.to.cmp(&y.to));
        assert_eq!(a.edges(&op.id), want);
    }

    /// Row "ImplicitTrigger to dependency": a record op on a dependency
    /// table → no edge, but (S3.6) the trigger it reaches keeps its identity
    /// and body state in `external_targets`, under the operation's id.
    #[test]
    fn implicit_trigger_to_dependency_has_no_edge() {
        let cu = "codeunit 50101 \"W\"\n{\n    procedure Caller()\n    var\n        C: Record Customer;\n    begin\n        C.Insert(true);\n    end;\n}\n";
        let a = adapt(&[("src/w.al", cu)], Some(DEP_SYMBOLS));
        let op = &a.routine("Caller").record_operations[0];
        assert_eq!(op.op, "Insert");
        assert_eq!(a.edges(&op.id), vec![]);
        assert_eq!(a.census.adapter_program_trigger_ops, 1, "{:#?}", a.census);
        assert_eq!(a.census.adapter_routes_dropped, 0, "{:#?}", a.census);
        assert_eq!(
            a.census.adapter_trigger_dependency_routes, 1,
            "{:#?}",
            a.census
        );
        let targets: Vec<_> = a
            .calls
            .external_targets
            .iter()
            .filter(|t| t.callsite_id == op.id)
            .collect();
        assert_eq!(targets.len(), 1, "{targets:?}");
        assert!(
            targets[0].target.ends_with("::oninsert/0"),
            "{}",
            targets[0].target
        );
        assert_eq!(
            targets[0].body,
            Some(crate::program::registry::BodyState::Bodyless)
        );
    }

    /// S9.0c: a zero-argument call may drop its `()` inside an expression too
    /// (`if IsOn then`). The resolver keeps it as a call; the body walk takes the
    /// same read as a call site, so the adapter joins it rather than losing it as
    /// a `program_only_site`. The variable read `B` stays a read.
    #[test]
    fn parenless_call_in_an_expression_reaches_the_model() {
        let cu = "codeunit 50120 \"P\"
{
    procedure IsOn(): Boolean
    begin
        exit(true);
    end;

    procedure Caller()
    var
        B: Boolean;
    begin
        if IsOn then;
        B := IsOn;
        if B then;
    end;
}
";
        let a = adapt(&[("src/p.al", cu)], None);
        let sites: Vec<&str> = a
            .routine("Caller")
            .call_sites
            .iter()
            .map(|cs| cs.callee_text.as_str())
            .collect();
        assert_eq!(sites, vec!["IsOn", "IsOn"], "two parens-less calls, no `B`");
        let is_on = a.routine("IsOn").id.clone();
        for cs in &a.routine("Caller").call_sites {
            let to: Vec<Option<String>> = a.edges(&cs.id).into_iter().map(|e| e.to).collect();
            assert_eq!(to, vec![Some(is_on.clone())], "{}", cs.id);
        }
        assert_eq!(a.census.program_only_site, 0, "{:#?}", a.census);
    }

    /// S3.5 (was ruling 1): a bare implicit-`Rec` record op is a record op to
    /// the program engine too, and takes its trigger edge from it, the same
    /// edge L3 gives. It passes `true`: an argless write fires no trigger
    /// (measured on BC 28), so it would have no edge to compare.
    #[test]
    fn bare_record_op_takes_the_program_trigger_edge() {
        let table = "table 50100 \"T\"\n{\n    fields\n    {\n        field(1; Code; Code[20]) { }\n    }\n    trigger OnModify()\n    begin\n    end;\n\n    procedure P()\n    begin\n        Modify(true);\n    end;\n}\n";
        let a = adapt(&[("src/t.al", table)], None);
        let op = &a.routine("P").record_operations[0];
        let want = at(&a.old, &op.id);
        assert_eq!(want.len(), 1, "L3 gives the bare op its OnModify edge");
        assert_eq!(
            want[0].to.as_deref(),
            Some(a.routine("OnModify").id.as_str())
        );
        assert_eq!(a.edges(&op.id), want);
        assert_eq!(a.census.adapter_program_trigger_ops, 1, "{:#?}", a.census);
        assert_eq!(a.census.adapter_l3_trigger_ops, 0, "{:#?}", a.census);
    }

    /// S3.6, precondition by assignment: the resolver makes no call edge with
    /// two routes outside an interface or overload set, so one is built by
    /// giving `Foo()`'s exact edge a second route to `Bar`. The adapter keeps
    /// both as an ambiguous candidate set; it used to keep the first only.
    #[test]
    fn multi_route_call_keeps_every_route() {
        let cu = "codeunit 50101 \"W\"\n{\n    procedure Foo()\n    begin\n    end;\n\n    procedure Bar()\n    begin\n    end;\n\n    procedure Caller()\n    begin\n        Foo();\n    end;\n}\n";
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("app.json"),
            r#"{"id":"b3b3b3b3-0000-0000-0000-000000000003","name":"B3 Adapter","publisher":"T","version":"1.0.0.0"}"#,
        )
        .unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/w.al"), cu).unwrap();
        let (ctx, mut report, l3) = build_models(dir.path()).unwrap();
        let bar = ctx
            .graph
            .routines
            .iter()
            .find(|n| n.id.name_lc == "bar")
            .unwrap()
            .id
            .clone();
        let ce = report
            .edges
            .iter_mut()
            .find(|ce| ce.edge.from.name_lc == "caller" && ce.edge.kind == EdgeKind::Call)
            .unwrap();
        assert_eq!(ce.edge.shape, DispatchShape::Exact, "precondition");
        let mut second = ce.edge.routes[0].clone();
        second.target = RouteTarget::Routine(bar);
        ce.edge.routes.push(second);

        let ws = &l3.workspace;
        let (calls, census) = resolved_calls_from_program(&report, &ctx, ws, true);
        let caller = ws.routines.iter().find(|r| r.name == "Caller").unwrap();
        let id_of = |n: &str| ws.routines.iter().find(|r| r.name == n).unwrap().id.clone();
        let got = at(&calls, &caller.call_sites[0].id);
        assert_eq!(got.len(), 1, "{got:#?}");
        assert_eq!(got[0].resolution, Resolution::Ambiguous);
        let mut want = vec![id_of("Foo"), id_of("Bar")];
        want.sort();
        assert_eq!(got[0].candidates, Some(want));
        assert_eq!(census.adapter_multi_route_sites, 1, "{census:#?}");
    }

    /// S3.5, precondition by assignment: the op is moved off its span, so it
    /// pairs with no program edge. It used to keep L3's own trigger edge; it
    /// now gets none.
    #[test]
    fn unmatched_record_op_gets_no_l3_trigger_edge() {
        let table = "table 50100 \"T\"\n{\n    fields\n    {\n        field(1; Code; Code[20]) { }\n    }\n    trigger OnModify()\n    begin\n    end;\n\n    procedure P()\n    begin\n        Modify();\n    end;\n}\n";
        let a = adapt_with(&[("src/t.al", table)], None, |ws| {
            let r = ws.routines.iter_mut().find(|r| r.name == "P").unwrap();
            r.record_operations[0].source_anchor.start_column += 100;
        });
        let op = &a.routine("P").record_operations[0];
        assert_eq!(at(&a.old, &op.id).len(), 1, "precondition: L3 has an edge");
        assert_eq!(a.edges(&op.id), vec![]);
        assert_eq!(a.census.adapter_l3_trigger_ops, 1, "{:#?}", a.census);
        assert_eq!(a.census.adapter_program_trigger_ops, 0, "{:#?}", a.census);
    }

    /// Ruling 2 as replaced by engine-switch S3.1, precondition by assignment:
    /// the L3 site is moved off its span, so it pairs with no program edge. It
    /// used to keep L3's own (resolved) edge; it is now one honest
    /// `Unknown(NoProgramSite)` edge.
    #[test]
    fn unmatched_site_is_unknown_not_l3() {
        let cu = "codeunit 50101 \"W\"\n{\n    procedure Foo()\n    begin\n    end;\n\n    procedure Caller()\n    begin\n        Foo();\n    end;\n}\n";
        let a = adapt_with(&[("src/w.al", cu)], None, |ws| {
            let r = ws.routines.iter_mut().find(|r| r.name == "Caller").unwrap();
            r.call_sites[0].source_anchor.start_column += 100;
        });
        let cs = a.site("Caller", "Foo");
        assert!(
            at(&a.old, &cs.id).iter().any(|e| e.to.is_some()),
            "precondition: L3 resolves it"
        );
        let got = a.edges(&cs.id);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].to, None);
        assert_eq!(
            got[0].resolution,
            Resolution::Unknown(L3Reason::NoProgramSite)
        );
        assert_eq!(a.census.adapter_l3_fallback_sites, 1, "{:#?}", a.census);
        assert_eq!(a.census.adapter_program_sites, 0, "{:#?}", a.census);
    }

    /// Ruling 5: the adapter emits edges in `resolve_calls`'s order (call
    /// sites in routine order, then trigger edges); here every edge is one
    /// both engines agree on, so the two lists are identical.
    #[test]
    fn edge_order_matches_resolve_calls() {
        let cu = "codeunit 50101 \"W\"\n{\n    procedure Foo(var R: Record \"T\"; I: Integer)\n    begin\n        R.Insert(true);\n    end;\n\n    procedure Caller()\n    var\n        R: Record \"T\";\n    begin\n        R.Modify(true);\n        Foo(R, 1);\n        Message('x');\n        Foo(R, 2);\n    end;\n}\n";
        let a = adapt(&[("src/t.al", TABLE), ("src/w.al", cu)], None);
        assert_eq!(a.calls.edges.len(), 5);
        assert_eq!(a.calls.edges, a.old.edges);
        assert_eq!(a.calls.upgraded_bindings, a.old.upgraded_bindings);
    }

    /// The production path (no notes) gives the same calls as the harness
    /// path (notes) on every `tests/r0-corpus` fixture: skipping the notes
    /// changes nothing the detectors read.
    #[test]
    fn production_path_equals_the_notes_path_on_r0_corpus() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/r0-corpus");
        let mut compared = 0;
        for e in std::fs::read_dir(&root).unwrap() {
            let dir = e.unwrap().path();
            if !dir.join("app.json").is_file() {
                continue;
            }
            let Ok((ctx, report, l3)) = build_models(&dir) else {
                continue;
            };
            let ws = &l3.workspace;
            let prod = resolved_calls_from_program(&report, &ctx, ws, true).0;
            let harness = resolved_calls_with_notes(&report, &ctx, ws, true).0;
            assert_eq!(prod.edges, harness.edges, "{}", dir.display());
            assert_eq!(
                prod.upgraded_bindings,
                harness.upgraded_bindings,
                "{}",
                dir.display()
            );
            compared += 1;
        }
        assert!(compared > 100, "only {compared} fixtures compared");
    }

    /// Parity over every `tests/r0-corpus` fixture: wherever the program
    /// engine and L3 resolve a site to the same workspace routine(s), the
    /// adapter's edges equal L3's in `to`, `dispatch_kind`, `resolution`
    /// and bindings. Prints per-fixture counts (`--nocapture`).
    #[test]
    fn r0_corpus_parity_where_both_resolve_the_same_routine() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/r0-corpus");
        let mut dirs: Vec<_> = std::fs::read_dir(&root)
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.join("app.json").is_file())
            .collect();
        dirs.sort();
        // Per-site buckets: `agree` (both resolve the same workspace
        // routines, edges and bindings equal — asserted), `differing target`,
        // `L3 only` / `adapter only` (one side has a `to`), and to-less on both
        // sides with the same / a different `(dispatch kind, resolution)`.
        let mut total: std::collections::BTreeMap<&str, usize> = Default::default();
        let mut failures: Vec<String> = Vec::new();
        for dir in &dirs {
            let name = dir.file_name().unwrap().to_string_lossy().to_string();
            let Ok((ctx, report, l3)) = build_models(dir) else {
                *total.entry("skipped fixtures").or_default() += 1;
                eprintln!("parity {name}: skipped (model build failed)");
                continue;
            };
            let ws = &l3.workspace;
            // Stage 1: L3 never upgrades a dependency callee's bindings, so
            // the binding parity below holds only without stage 2.
            let (calls, census) = resolved_calls_from_program(&report, &ctx, ws, false);
            *total.entry("adapter: program sites").or_default() += census.adapter_program_sites;
            *total.entry("adapter: L3 fallback sites").or_default() +=
                census.adapter_l3_fallback_sites;
            *total.entry("adapter: program trigger ops").or_default() +=
                census.adapter_program_trigger_ops;
            *total.entry("adapter: L3 trigger ops").or_default() += census.adapter_l3_trigger_ops;
            let symbols = crate::engine::l3::symbol_table::SymbolTable::build(
                &ws.objects,
                &ws.tables,
                &ws.routines,
            );
            let old = resolve_calls(ws, &symbols, &[], &[]);
            let group = |rc: &ResolvedCalls| {
                let mut m: HashMap<String, Vec<CallEdge>> = HashMap::new();
                for e in &rc.edges {
                    m.entry(e.callsite_id.clone()).or_default().push(e.clone());
                }
                m
            };
            let (new_g, old_g) = (group(&calls), group(&old));
            let mut ids: Vec<(String, bool)> = Vec::new();
            for r in &ws.routines {
                ids.extend(r.call_sites.iter().map(|cs| (cs.id.clone(), true)));
                ids.extend(r.record_operations.iter().map(|o| (o.id.clone(), false)));
            }
            // The program edge behind each adapted site, for the printout.
            let j = join(&report, &ctx, ws);
            let mut program_of: HashMap<String, String> = HashMap::new();
            for (ri, r) in ws.routines.iter().enumerate() {
                let named = r
                    .call_sites
                    .iter()
                    .enumerate()
                    .map(|(ci, cs)| (cs.id.clone(), j.calls.get(&(ri, ci))))
                    .chain(
                        r.record_operations
                            .iter()
                            .enumerate()
                            .map(|(oi, o)| (o.id.clone(), j.ops.get(&(ri, oi)))),
                    );
                for (id, ce) in named {
                    let text = ce.map_or("no program edge".to_string(), |ce| {
                        let routes: Vec<_> = ce
                            .edge
                            .routes
                            .iter()
                            .map(|r| (&r.evidence, r.receiver_tier))
                            .collect();
                        format!("{:?} {:?} {routes:?}", ce.edge.kind, ce.edge.shape)
                    });
                    program_of.insert(id, text);
                }
            }
            let tos = |v: &[CallEdge]| {
                let mut t: Vec<String> = v.iter().filter_map(|e| e.to.clone()).collect();
                t.sort();
                t
            };
            // The whole edge, minus the diagnostic-only fields
            // (`candidates` only feeds the projection; `unknown_method_name`
            // and `receiver_shape` only feed `aldump` breakdowns).
            // Engine-switch S3.2: `dispatch_meta` is now the program engine's own
            // WHOLE-program view (L3's counts workspace implementers only), so it
            // is not compared; the dependency-implementer edges S3.2 adds (to-less
            // `Interface`+`ExternalTarget`) have no L3 counterpart and are dropped
            // from the adapter side before comparing (`comparable` below).
            // Engine-switch S6.0: `receiver_type` is the program resolver's
            // receiver text now. For a receiver with no declaration it renders
            // the resolved object (`Record Customer`, quoted multi-word names)
            // where L3 synthesized its own (`Record rec`, unquoted), so it is
            // not compared here; `receiver_type_comes_from_the_program_resolver`
            // pins it.
            let key = |e: &CallEdge| {
                let mut e = e.clone();
                e.candidates = None;
                e.unknown_method_name = None;
                e.receiver_shape = None;
                e.dispatch_meta = None;
                e.receiver_type = None;
                e
            };
            let comparable = |e: &CallEdge| {
                !(e.dispatch_kind == DispatchKind::Interface
                    && e.to.is_none()
                    && e.resolution == Resolution::ExternalTarget)
            };
            let same_bindings = |id: &str, is_call: bool| {
                !is_call || calls.upgraded_bindings.get(id) == old.upgraded_bindings.get(id)
            };
            let mut here: std::collections::BTreeMap<&str, usize> = Default::default();
            for (id, is_call) in &ids {
                let mut n: Vec<CallEdge> = new_g
                    .get(id)
                    .cloned()
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|e| comparable(e))
                    .collect();
                let mut o = old_g.get(id).cloned().unwrap_or_default();
                let (nt, ot) = (tos(&n), tos(&o));
                let bucket = match (nt.is_empty(), ot.is_empty()) {
                    (true, true) => {
                        let shape = |v: &[CallEdge]| {
                            v.iter()
                                .map(|e| (e.dispatch_kind, e.resolution))
                                .collect::<Vec<_>>()
                        };
                        Some(if shape(&n) == shape(&o) {
                            "to-less, same kind+resolution"
                        } else {
                            "to-less, different kind+resolution"
                        })
                    }
                    (true, false) => Some("L3 only resolved"),
                    (false, true) => Some("adapter only resolved"),
                    (false, false) if nt != ot => Some("differing target"),
                    (false, false) => None,
                };
                if bucket == Some("to-less, same kind+resolution") && !same_bindings(id, *is_call) {
                    failures.push(format!(
                        "{name} {id} [to-less, same kind+resolution] bindings: adapter {:?} vs L3 {:?}",
                        calls.upgraded_bindings.get(id),
                        old.upgraded_bindings.get(id),
                    ));
                }
                // Informational: same kind+resolution, but another field
                // (external_type_ref, dispatch_meta, ..) differs.
                if bucket == Some("to-less, same kind+resolution")
                    && n.iter().map(key).collect::<Vec<_>>()
                        != o.iter().map(key).collect::<Vec<_>>()
                {
                    *here
                        .entry("to-less, same kind+resolution, other fields differ")
                        .or_default() += 1;
                    eprintln!(
                        "  {name} [to-less same, fields differ] {id}: adapter {:?} vs L3 {:?}",
                        n.iter().map(key).collect::<Vec<_>>(),
                        o.iter().map(key).collect::<Vec<_>>()
                    );
                }
                if let Some(bucket) = bucket {
                    *here.entry(bucket).or_default() += 1;
                    if bucket != "to-less, same kind+resolution" {
                        let show = |v: &[CallEdge]| {
                            v.iter()
                                .map(|e| (e.dispatch_kind.as_str(), e.resolution, e.to.is_some()))
                                .collect::<Vec<_>>()
                        };
                        eprintln!(
                            "  {name} [{bucket}] {id}: adapter {:?} vs L3 {:?} (program: {})",
                            show(&n),
                            show(&o),
                            program_of[id]
                        );
                    }
                    continue;
                }
                n.sort_by(|a, b| a.to.cmp(&b.to));
                o.sort_by(|a, b| a.to.cmp(&b.to));
                let same_edges =
                    n.len() == o.len() && n.iter().zip(&o).all(|(x, y)| key(x) == key(y));
                if same_edges && same_bindings(id, *is_call) {
                    *here.entry("agree").or_default() += 1;
                } else {
                    failures.push(format!(
                        "{name} {id}: adapter {:?} / {:?} vs L3 {:?} / {:?}",
                        n.iter().map(key).collect::<Vec<_>>(),
                        calls.upgraded_bindings.get(id),
                        o.iter().map(key).collect::<Vec<_>>(),
                        old.upgraded_bindings.get(id),
                    ));
                }
            }
            eprintln!("parity {name}: {here:?}");
            for (k, v) in here {
                *total.entry(k).or_default() += v;
            }
        }
        eprintln!(
            "parity TOTAL over {} fixtures: {total:?}, mismatched {}",
            dirs.len(),
            failures.len()
        );
        assert!(
            total.get("agree").copied().unwrap_or(0) > 0,
            "the corpus exercised no comparable site"
        );
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }
}

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
        let (calls, census) = resolved_calls_from_program(&report, &ctx, &l3.workspace, true);
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
    /// one model call site is moved to a span no program edge has. That site
    /// would otherwise resolve (it is an ordinary workspace call).
    #[test]
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
        let before = resolved_calls_from_program(&report, &ctx, &l3.workspace, true).0;
        let resolved_before: Vec<_> = before
            .edges
            .iter()
            .filter(|e| e.callsite_id == cs_id)
            .collect();
        assert!(
            resolved_before.iter().any(|e| e.to.is_some()),
            "precondition: the site resolves while its program edge is found: {resolved_before:?}"
        );

        l3.workspace.routines[ri].call_sites[ci]
            .source_anchor
            .start_line += 9999;
        let (calls, census) = resolved_calls_from_program(&report, &ctx, &l3.workspace, true);
        let site: Vec<_> = calls
            .edges
            .iter()
            .filter(|e| e.callsite_id == cs_id)
            .collect();
        assert_eq!(site.len(), 1, "{site:?}");
        assert_eq!(site[0].to, None);
        assert_eq!(
            site[0].resolution,
            Resolution::Unknown(L3Reason::NoProgramSite)
        );
        assert_eq!(census.adapter_l3_fallback_sites, 1);
    }
}

//! 1B.3a Task 3: `resolve_full_program` + self-reported taxonomy'd metric.
//!
//! # Coverage contract
//!
//! Every parsed call/event obligation (each [`CalleeShape`] site in every
//! workspace source routine + every publisher event routine in the program
//! graph) gets a stable [`ObligationId`], tracked INLINE during the single
//! resolution pass in [`resolve_full_program_from_parts`] (an
//! `obligation_id_set` built alongside `classified_edges` — see that
//! function's body). [`resolve_full_program`] resolves each obligation to
//! exactly one classified [`ClassifiedEdge`].
//!
//! The **COVERAGE CONTRACT** is **distinct-id SET equality**:
//!
//! ```text
//! set(obligation_ids) == set(classified_edge.obligation_id)
//! ```
//!
//! [`coverage_holds`] returns `true` iff the two sets are equal.
//! `Unknown`/`HonestDynamic`/`HonestEmpty` edges ARE valid classified edges;
//! they fulfil the coverage contract. Only a silently-absent edge (an
//! obligation that produced no edge at all) violates it.
//!
//! (Historical note, sigfp-and-ambiguous-reclassification plan Task 2: a
//! separate `pub fn obligation_inventory` used to enumerate obligations as a
//! standalone pre-pass — reviewer-confirmed DEAD CODE with zero callers
//! outside its own definition (coverage was, and is, computed by the inline
//! tracking above, never by comparing against that separate enumeration).
//! Its own [`RoutineNodeId`] reconstruction was one of the 5 audited
//! `sig_fp`-hardcoded-`0` sites; since it had no live caller, it was deleted
//! rather than migrated to [`crate::program::sig_fp::source_routine_node_id`].)

use std::collections::{HashMap, HashSet};
use std::path::Path;

use al_syntax::IdentifierFoldExt;
use al_syntax::ir::ObjectKind;
use rayon::prelude::*;

use crate::engine::perf_trace as pt;
use crate::program::build::{DepLayer, assemble_program_graph, build_dep_layer_cached};
use crate::program::dep_cache::{DepCache, DepKey};
use crate::program::graph::ProgramGraph;
use crate::program::node::{AppRef, ObjKey, ObjectNodeId, RoutineNodeId};
use crate::program::node_extract::ObjectNode;
use crate::program::resolve::abi_check::{
    AbiIntegrityReport, abi_ingestion_integrity, build_raw_abi_index_from_snapshot,
};
use crate::program::resolve::arg_dispatch::{self, ArgDispatchInfo};
use crate::program::resolve::decl_surface::DeclSurface;
use crate::program::resolve::edge::{
    CanonicalSpan, DispatchShape, Edge, EdgeKind, Evidence, EvidenceKind, Histogram,
    OpenWorldReason, Route, RouteTarget, SetCompleteness, SiteId, UnknownReason, Witness,
    callee_fp, classify_obligation,
};
use crate::program::resolve::extract::{
    CalleeShape, WithState, extract_sites_for_routine, static_database_reference_target,
};
use crate::program::resolve::index::ResolveIndex;
use crate::program::resolve::member_catalog::is_entry_dispatch_builtin;
use crate::program::resolve::preflight_cache;
use crate::program::resolve::receiver::{
    FrameworkKind, ReceiverType, infer_receiver_type, is_atomic_receiver_token,
};
use crate::program::resolve::resolver::{
    emit_event_flow_edges, resolve_bare, resolve_bare_with_args, resolve_implicit_trigger,
    resolve_member_with_args, resolve_object_run,
};
use crate::program::sig_fp::source_routine_node_id;
use crate::snapshot::{
    AppSetSnapshot, AppUnit, DependencySource, ParsedFile, ParsedUnit, SnapshotBuilder,
    parse_snapshot,
};
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Stable identity of one parsed obligation.
///
/// - **`CallSite`** — mirrors [`SiteId`]: `(caller, span, callee_fp)`.
/// - **`Publisher`** — the publisher routine's node id.
///   One `Publisher` obligation per publisher routine in the graph.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ObligationId {
    CallSite {
        caller: RoutineNodeId,
        span: CanonicalSpan,
        callee_fp: u64,
    },
    Publisher(RoutineNodeId),
}

/// A classified edge annotated with the obligation it was resolved from.
pub struct ClassifiedEdge {
    pub obligation_id: ObligationId,
    pub edge: Edge,
}

/// Result of resolving ALL call-site obligations in ONE workspace file —
/// [`resolve_file_obligations`]'s return type. `flagged`/`indeterminate` are
/// this file's contribution to the T0.3 builtin-dispatch audit (see
/// [`FlaggedBuiltinDispatchSite`]/[`IndeterminateBuiltinDispatchSite`]);
/// [`resolve_full_program_from_parts`] aggregates every file's triple and
/// sorts the combined `flagged`/`indeterminate` populations once, after all
/// files have been processed.
pub(crate) struct FileResolution {
    pub edges: Vec<ClassifiedEdge>,
    pub flagged: Vec<FlaggedBuiltinDispatchSite>,
    pub indeterminate: Vec<IndeterminateBuiltinDispatchSite>,
}

// ---------------------------------------------------------------------------
// T0.3: builtin-dispatch justification audit (diagnostic-only)
// ---------------------------------------------------------------------------

/// One call site whose `Route` resolved to `RouteTarget::Builtin` via a
/// [`crate::program::resolve::member_catalog::ENTRY_DISPATCH_BUILTIN_IDS`]
/// entry AND whose target object is PROVEN statically named — a missed
/// entry-trigger dispatch (T0.3; see that const's doc for the classifier
/// gaps this makes visible). `object` is `"{ObjectKind}::{name_lc}"`
/// (e.g. `"Page::some page"`), always lowercased for deterministic sorting
/// regardless of which extraction path produced it (a declared receiver's
/// own type, or a call argument's `Page::"X"` reference).
///
/// Diagnostic-only: never consulted by `classify_obligation`/
/// `ObligationOutcome`, never compared against a semantic golden — does not
/// change any route/edge/histogram.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct FlaggedBuiltinDispatchSite {
    pub file: String,
    pub object: String,
    pub method: String,
    pub line: u32,
}

/// A call site whose method is in
/// [`crate::program::resolve::member_catalog::ENTRY_DISPATCH_BUILTIN_IDS`]
/// and whose route resolved to `Builtin`, but whose target could NOT be
/// proven statically (fail-closed — e.g. a runtime variable/expression
/// argument, or a receiver shape the audit does not attempt to prove).
/// Reported so the flagged population is honest about what it excludes,
/// never silently dropped. Diagnostic-only (see [`FlaggedBuiltinDispatchSite`]'s doc).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct IndeterminateBuiltinDispatchSite {
    pub file: String,
    pub method: String,
    pub line: u32,
}

/// T0.3 builtin-dispatch justification audit output: the deterministic,
/// sorted `flagged`/`indeterminate` populations produced by
/// [`resolve_full_program`]. See [`FlaggedBuiltinDispatchSite`]'s doc.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BuiltinDispatchAudit {
    pub flagged: Vec<FlaggedBuiltinDispatchSite>,
    pub indeterminate: Vec<IndeterminateBuiltinDispatchSite>,
}

/// Per-call-site signal threaded out of [`resolve_call_site_obligation`] for
/// the T0.3 audit — populated ONLY by the `CalleeShape::Member` arm (every
/// other arm returns `None`); see [`builtin_dispatch_finding`].
enum BuiltinDispatchFinding {
    Flagged { object: String, method: String },
    Indeterminate { method: String },
}

/// Coverage report: the distinct-id SET equality contract.
#[derive(Clone, Debug)]
pub struct Coverage {
    /// Total distinct obligation ids (the DENOMINATOR).
    pub parsed_obligations: usize,
    /// Total distinct edge obligation ids (the NUMERATOR).
    pub classified_edges: usize,
    /// Obligation ids present in the inventory but absent from the edge set
    /// — obligations for which the resolver emitted no edge (contract failure).
    pub missing: Vec<ObligationId>,
    /// Edge obligation ids present in the edge set but absent from the
    /// inventory (edges emitted without a corresponding obligation).
    pub extra: Vec<ObligationId>,
}

/// Full result of [`resolve_full_program`].
pub struct ProgramReport {
    /// All classified edges (whole-program scope: all source-bearing routines +
    /// all publisher routines in all apps).
    pub edges: Vec<ClassifiedEdge>,
    /// Coverage: distinct-id set equality between obligations and edges.
    pub coverage: Coverage,
    /// Taxonomy'd histogram over ALL edges.
    pub histogram: Histogram,
    /// Taxonomy'd histogram over PRIMARY-SCOPED edges only
    /// (edges whose `from.object.app == primary_app_ref`).
    pub primary_histogram: Histogram,
    /// ABI ingestion integrity: `AbiSymbol` route keys vs. raw dep SymbolReference.
    pub abi_integrity: AbiIntegrityReport,
    /// The workspace app's [`AppRef`] (use with [`is_primary_scope`]).
    pub primary_app_ref: AppRef,
    /// Count of publisher `EventFlow` edges SKIPPED by [`resolver::
    /// emit_event_flow_edges`]'s Task 1 dual-publisher source-overload-alias
    /// collision guard (sigfp-and-ambiguous-reclassification plan) —
    /// [`resolver::dual_publisher_alias_skip_count`]. Expected `0` outside
    /// the CDO-measured known dual-publisher pairs; a nonzero value beyond
    /// those signatures is a threshold alert (investigate, don't mask —
    /// collision-guard-observability addendum).
    pub event_flow_dual_publisher_alias_skips: usize,
    /// File paths of every parsed source file whose parse hit tree-sitter
    /// error recovery (`ParseStatus::Recovered`) — Task 3 (preprocessor
    /// foundations plan). ADDITIVE diagnostic, never gates resolution: see
    /// [`crate::snapshot::parse::recovered_file_paths`]'s doc for the
    /// absence-claim invariant this surfaces. Expected empty on a
    /// well-formed workspace; any entry means that file's IR may be missing
    /// content tree-sitter could not parse.
    pub recovered_files: Vec<String>,
    /// T0.3 builtin-dispatch justification audit — see [`BuiltinDispatchAudit`]'s
    /// doc. ADDITIVE diagnostic: never consulted by `histogram`/
    /// `classify_obligation`, does not change any route/edge.
    pub builtin_dispatch_audit: BuiltinDispatchAudit,
}

// ---------------------------------------------------------------------------
// Public functions
// ---------------------------------------------------------------------------

/// Returns `true` when the coverage contract holds: every obligation has
/// exactly one classified edge and no edge was emitted without an obligation.
pub fn coverage_holds(c: &Coverage) -> bool {
    c.missing.is_empty() && c.extra.is_empty()
}

/// Returns `true` when this edge's `from` routine belongs to the workspace
/// (primary) app — mirrors `--l3-call-graph-stats-cross-app` scoping.
pub fn is_primary_scope(edge: &ClassifiedEdge, primary_app_ref: AppRef) -> bool {
    edge.edge.from.object.app == primary_app_ref
}

// ---------------------------------------------------------------------------
// Core resolution
// ---------------------------------------------------------------------------

/// Inline helper: an Unknown-evidence Unresolved route (resolution failure).
/// Task 3: `reason` is REQUIRED — every call site supplies a diagnostic
/// [`UnknownReason`].
fn unknown_route(reason: UnknownReason) -> Route {
    Route {
        target: RouteTarget::Unresolved,
        evidence: Evidence::Unknown(reason),
        conditions: vec![],
        witness: Witness::None,
        receiver_tier: None,
    }
}

/// Task 3: classify `CalleeShape::Unknown`'s decline reason from the raw
/// callee text. A `callee_text` with >=2 dot separators (`A.B.C`) is a
/// multi-segment receiver chain the extractor structurally cannot classify
/// into a `Member { receiver_text, method }` shape (which only ever captures
/// ONE dot); anything else reaching `Unknown` is some other unclassifiable
/// call expression shape.
fn unclassified_callee_reason(callee_text: &str) -> UnknownReason {
    if callee_text.matches('.').count() >= 2 {
        UnknownReason::CompoundReceiver
    } else {
        UnknownReason::UnclassifiedCallee
    }
}

/// Derive [`SetCompleteness`] from the shape for member and similar calls.
fn completeness_for_shape(shape: DispatchShape) -> SetCompleteness {
    match shape {
        DispatchShape::Exact => SetCompleteness::Complete,
        DispatchShape::Polymorphic => SetCompleteness::Partial {
            reason: OpenWorldReason::ReverseDependentImplementers,
        },
        DispatchShape::DynamicOpen => SetCompleteness::Partial {
            reason: OpenWorldReason::RuntimeTypeUnbounded,
        },
        DispatchShape::Multicast => SetCompleteness::Partial {
            reason: OpenWorldReason::ReverseDependentExtensions,
        },
        // Task 3 (sigfp-and-ambiguous-reclassification plan): a same-object
        // overload-ambiguity candidate set is a SNAPSHOT-ENUMERATED, CLOSED
        // set — unlike Polymorphic's open-world reverse-dependent
        // implementers, no future dependent app can add another overload
        // candidate to an already-compiled object. `Complete`, not `Partial`.
        DispatchShape::AmbiguousOverload => SetCompleteness::Complete,
    }
}

/// T0.3 builtin-dispatch audit: classify one `CalleeShape::Member` call's
/// ALREADY-RESOLVED `routes` for the "entry-dispatching builtin absorbed a
/// statically-named target" bug class (see
/// `member_catalog::ENTRY_DISPATCH_BUILTIN_IDS`'s doc for the two classifier
/// gaps this makes visible). Returns `None` when no route in `routes`
/// actually landed on a flagged catalog entry — a no-op for the
/// overwhelming majority of member calls, including every OTHER
/// `PageInstance`/`ReportInstance` method (`SetRecord`, `Caption`, …).
///
/// Fail-closed (T0.3 constraint): a flagged method whose target cannot be
/// PROVEN static returns `Indeterminate`, never a guessed `Flagged`.
///
/// - `recv == ReceiverType::Object { kind, name_lc, .. }` (a declared
///   Page/Report-typed variable/param/global receiver, or the `CurrPage.
///   <part>.Page` subpage shape): the target is the receiver's OWN resolved
///   type — 100% proven, no argument inspection needed. `Flagged`.
/// - `recv == ReceiverType::Framework(PageInstance | ReportInstance)` (the
///   literal `Page`/`CurrPage`/`Report`/`CurrReport` singleton receiver,
///   `receiver.rs:714-715`): the target can ONLY come from a
///   `Page::"X"`/`Report::"X"`-shaped first argument
///   ([`static_database_reference_target`]). `Flagged` when present,
///   `Indeterminate` otherwise (e.g. a runtime variable/expression argument
///   — dynamic dispatch, or zero args — `CurrPage`/`CurrReport` self-dispatch,
///   deliberately not claimed as a foreign target by this audit).
/// - Any other receiver shape reaching a flagged route (not expected given
///   `member_catalog.rs`'s receiver-name-gated `Framework` mapping, but
///   fail-closed rather than assumed impossible): `Indeterminate`.
fn builtin_dispatch_finding(
    recv: &ReceiverType,
    method_lc: &str,
    routes: &[Route],
    file: &al_syntax::ir::AlFile,
    call_args: &[al_syntax::ir::ExprId],
) -> Option<BuiltinDispatchFinding> {
    let flagged = routes.iter().any(|r| match &r.target {
        RouteTarget::Builtin(bid) => is_entry_dispatch_builtin(bid),
        _ => false,
    });
    if !flagged {
        return None;
    }
    match recv {
        ReceiverType::Object { kind, name_lc, .. } => Some(BuiltinDispatchFinding::Flagged {
            object: format!("{kind:?}::{name_lc}"),
            method: method_lc.to_string(),
        }),
        ReceiverType::Framework(
            fk @ (FrameworkKind::PageInstance | FrameworkKind::ReportInstance),
        ) => {
            let kind_str = match fk {
                FrameworkKind::PageInstance => "Page",
                FrameworkKind::ReportInstance => "Report",
                _ => unreachable!("guarded by the outer match arm"),
            };
            match static_database_reference_target(file, call_args) {
                Some((target, _target_is_name)) => Some(BuiltinDispatchFinding::Flagged {
                    object: format!("{kind_str}::{}", target.fold_identifier()),
                    method: method_lc.to_string(),
                }),
                None => Some(BuiltinDispatchFinding::Indeterminate {
                    method: method_lc.to_string(),
                }),
            }
        }
        _ => Some(BuiltinDispatchFinding::Indeterminate {
            method: method_lc.to_string(),
        }),
    }
}

/// Resolve one call-site obligation to `(kind, shape, completeness, routes,
/// builtin_dispatch_finding)`. The 5th element is the T0.3 audit signal
/// (`Some` only from the `CalleeShape::Member` arm — see
/// [`builtin_dispatch_finding`]); every other arm returns `None`.
#[allow(clippy::too_many_arguments)]
fn resolve_call_site_obligation(
    shape: &CalleeShape,
    arity: usize,
    callee_text: &str,
    obj_node_opt: Option<&ObjectNode>,
    routine: &al_syntax::ir::RoutineDecl,
    obj: &al_syntax::ir::ObjectDecl,
    primary_app_ref: AppRef,
    graph: &ProgramGraph,
    index: &ResolveIndex,
    surface: &DeclSurface,
    with_state: WithState,
    // Task 2 enabling primitive: the parsed `AlFile` this obligation's call
    // site was extracted from, so a `CalleeShape::Member.receiver` `ExprId`
    // can be dereferenced into `infer_receiver_type`'s `receiver_expr` param.
    // Task 3 is the first consumer (Step 5, `Func().Method()` compound
    // receivers) — Steps 0-4 remain unaffected.
    file: &al_syntax::ir::AlFile,
    // argtype-dispatch-and-page-catalog plan, Task 2: the call site's raw
    // argument expression ids (`RawSiteV2::args`), typed ONCE below into
    // `ArgDispatchInfo` and threaded to `resolve_bare_with_args`/
    // `resolve_member_with_args` so `resolve_in_object`'s fail-closed pick
    // has real argument evidence to work with.
    call_args: &[al_syntax::ir::ExprId],
) -> (
    EdgeKind,
    DispatchShape,
    SetCompleteness,
    Vec<Route>,
    Option<BuiltinDispatchFinding>,
) {
    // Built ONCE per obligation (not per-arm): SOURCE-tier only (`arg_
    // dispatch`'s own SymbolOnly gate lives in `resolve_in_object`, but
    // there is nothing to type at all without a resolved calling object).
    // Task 2 review fix: `with_state` threads into arg typing too — a bare-
    // identifier arg can be REBOUND by an enclosing `with` block, exactly
    // the hazard `resolve_bare`'s Step 3 with-guard already exists to close
    // for bare CALLS (see `arg_dispatch`'s module doc, "`with`-scope gate
    // for bare-identifier args").
    let args_info: Vec<ArgDispatchInfo> = match obj_node_opt {
        Some(obj_node) => arg_dispatch::type_call_args(
            call_args,
            file,
            routine,
            &obj.globals,
            &obj_node.id,
            graph,
            index,
            surface,
            with_state,
        ),
        None => Vec::new(),
    };

    match shape {
        CalleeShape::Bare { name } => {
            let name_lc = name.fold_identifier();
            // Task 4 (sigfp-and-ambiguous-reclassification plan): thread the
            // REAL shape `resolve_bare` determined through — a bare call is
            // `DispatchShape::Exact` in every case except a genuine
            // same-object overload ambiguity, which is now
            // `DispatchShape::AmbiguousOverload` (previously hardcoded
            // `Exact` unconditionally, which would have mislabeled the
            // multi-route ambiguous case). `completeness_for_shape` maps
            // BOTH `Exact` and `AmbiguousOverload` to `SetCompleteness::
            // Complete`, so this is behavior-preserving for every other
            // shape.
            let (shape, routes) = if let Some(obj_node) = obj_node_opt {
                resolve_bare_with_args(
                    obj_node, &name_lc, arity, graph, index, surface, with_state, &args_info,
                )
            } else {
                (
                    DispatchShape::Exact,
                    vec![unknown_route(UnknownReason::IndexIntegrationGap)],
                )
            };
            (
                EdgeKind::Call,
                shape,
                completeness_for_shape(shape),
                routes,
                None,
            )
        }

        CalleeShape::Member {
            receiver_text,
            method,
            receiver,
        } => {
            let receiver_lc = receiver_text.fold_identifier();
            let method_lc = method.fold_identifier();
            let mut finding: Option<BuiltinDispatchFinding> = None;
            let (member_shape, mut routes) = if let Some(obj_node) = obj_node_opt {
                let recv = infer_receiver_type(
                    &receiver_lc,
                    routine,
                    &obj.globals,
                    obj_node,
                    graph,
                    index,
                    receiver.map(|id| (file, id)),
                    Some((surface, with_state)),
                );
                let (s, r) = resolve_member_with_args(
                    &recv, &method_lc, arity, obj_node, graph, index, surface, &args_info,
                );
                finding = builtin_dispatch_finding(&recv, &method_lc, &r, file, call_args);
                (s, r)
            } else {
                (
                    DispatchShape::Exact,
                    vec![unknown_route(UnknownReason::IndexIntegrationGap)],
                )
            };
            // Task 3: a COMPOUND `receiver_text` (`A.B.C`, an UNQUOTED `.`
            // segment separator) means Phase A was asked to type a
            // multi-segment/compound receiver chain — AL variable/singleton/
            // framework/dataitem names never contain an unquoted dot, so
            // `infer_receiver_type` structurally cannot match one (except the
            // narrow `CurrPage.<part>.Page` shape, which resolves and never
            // reaches here). Relabel the generic `UntrackedReceiver` tag with
            // the more specific `CompoundReceiver` in that case.
            //
            // `is_atomic_receiver_token` (dataitem-receivers plan, Task 1)
            // replaces the naive `receiver_lc.contains('.')` check here: a
            // QUOTED receiver with an EMBEDDED period
            // (`"Sales Cr.Memo Header Filter"`) is a single ATOMIC identifier,
            // not a compound chain, so it must NOT be relabeled
            // `CompoundReceiver` — the naive check mislabeled it before this
            // fix, hiding a real dataitem-name receiver behind the wrong
            // Unknown reason.
            if !is_atomic_receiver_token(&receiver_lc) {
                for r in &mut routes {
                    if matches!(
                        r.evidence,
                        Evidence::Unknown(UnknownReason::UntrackedReceiver)
                    ) {
                        r.evidence = Evidence::Unknown(UnknownReason::CompoundReceiver);
                    }
                }
            }
            let completeness = completeness_for_shape(member_shape);
            (EdgeKind::Call, member_shape, completeness, routes, finding)
        }

        CalleeShape::ObjectRun {
            object_kind,
            target_ref,
            target_is_name,
        } => {
            let okind_opt = match object_kind.as_str() {
                "Codeunit" => Some(ObjectKind::Codeunit),
                "Page" => Some(ObjectKind::Page),
                "Report" => Some(ObjectKind::Report),
                _ => None,
            };
            if let Some(okind) = okind_opt {
                let (shape, completeness, routes) = resolve_object_run(
                    primary_app_ref,
                    okind,
                    target_ref.as_deref(),
                    *target_is_name,
                    graph,
                    index,
                    surface,
                );
                (EdgeKind::Run, shape, completeness, routes, None)
            } else {
                // Unrecognised object kind — honest Unknown.
                (
                    EdgeKind::Run,
                    DispatchShape::Exact,
                    SetCompleteness::Complete,
                    vec![unknown_route(UnknownReason::UnclassifiedCallee)],
                    None,
                )
            }
        }

        CalleeShape::RecordOp { receiver_text, op } => {
            let receiver_lc = receiver_text.fold_identifier();
            let op_lc = op.fold_identifier();

            // Infer the record type from the receiver and look up its table
            // ObjectNode.  Falls back to honest-empty when the table is not found.
            let table_node_opt: Option<&ObjectNode> = if let Some(obj_node) = obj_node_opt {
                // `RecordOp` carries no `ExprId` (Task 2 scoped the primitive
                // to `CalleeShape::Member` only) — `None`/`None` here is
                // unchanged behavior, not a gap (Task 3's Step 5 is also
                // scoped to `CalleeShape::Member`).
                let recv = infer_receiver_type(
                    &receiver_lc,
                    routine,
                    &obj.globals,
                    obj_node,
                    graph,
                    index,
                    None,
                    None,
                );
                match recv {
                    ReceiverType::Record {
                        table: Some(ref tid),
                    } => graph.objects.iter().find(|o| o.id == *tid),
                    _ => None,
                }
            } else {
                None
            };

            let (shape, completeness, routes) = if let Some(table_node) = table_node_opt {
                resolve_implicit_trigger(&op_lc, table_node, graph, index, surface)
            } else {
                // No table resolved: honest-empty Multicast (open-world, no
                // known triggers, but we cannot say there are none).
                (
                    DispatchShape::Multicast,
                    SetCompleteness::Partial {
                        reason: OpenWorldReason::ReverseDependentExtensions,
                    },
                    vec![],
                )
            };
            (EdgeKind::ImplicitTrigger, shape, completeness, routes, None)
        }

        CalleeShape::Commit => {
            // `commit` is a global builtin — resolve_bare finds it in the
            // catalog (Step 4). Threading the real shape through (Task 4)
            // rather than hardcoding `Exact` costs nothing here (Step 4
            // always yields `Exact`) and stays consistent with the `Bare`
            // arm above for the case an object declares its OWN overloaded
            // 0-arity `commit` procedure (Step 1 would then reach it before
            // Step 4 ever runs) — structurally impossible in valid AL
            // (`Commit` is a reserved statement keyword; no compiling AL
            // source can declare a procedure that collides with it), so
            // this arm stays defensive-only rather than a live path any
            // real CDO/workspace source can reach.
            let (shape, routes) = if let Some(obj_node) = obj_node_opt {
                resolve_bare(obj_node, "commit", 0, graph, index, surface, with_state)
            } else {
                (
                    DispatchShape::Exact,
                    vec![unknown_route(UnknownReason::IndexIntegrationGap)],
                )
            };
            (
                EdgeKind::Call,
                shape,
                completeness_for_shape(shape),
                routes,
                None,
            )
        }

        CalleeShape::Unknown => {
            // Unclassifiable call expression — honest Unknown.
            (
                EdgeKind::Call,
                DispatchShape::Exact,
                SetCompleteness::Complete,
                vec![unknown_route(unclassified_callee_reason(callee_text))],
                None,
            )
        }
    }
}

/// Resolve ALL call-site obligations of ONE workspace file (T3 Task 6, the
/// LSP-migration arc's rung-1 incremental-updater primitive: re-resolving a
/// single saved file's obligations is exactly this call). Extracted
/// VERBATIM from [`resolve_full_program_from_parts`]'s Phase-1 per-file loop
/// body — same iteration order, same obligation-id construction. The
/// `ws_file_set` membership check stays in the caller (this function assumes
/// `pf` already passed it); `obligation_id_set`/`classified_edges`/`flagged`/
/// `indeterminate` are whole-run accumulators the caller owns — this
/// function returns its own contribution in a [`FileResolution`] instead of
/// mutating shared state, so per-file re-resolution (rung 1) never needs the
/// other files' accumulators in scope.
pub(crate) fn resolve_file_obligations(
    pf: &ParsedFile,
    primary_app_ref: AppRef,
    graph: &ProgramGraph,
    index: &ResolveIndex,
    surface: &DeclSurface,
    obj_node_map: &HashMap<ObjectNodeId, &ObjectNode>,
) -> FileResolution {
    let mut edges: Vec<ClassifiedEdge> = Vec::new();
    let mut flagged: Vec<FlaggedBuiltinDispatchSite> = Vec::new();
    let mut indeterminate: Vec<IndeterminateBuiltinDispatchSite> = Vec::new();

    for (obj_idx, obj) in pf.file.objects.iter().enumerate() {
        let obj_key = match obj.id {
            Some(n) => ObjKey::Id(n),
            None => ObjKey::Name(obj.name.fold_identifier()),
        };
        let obj_node_id = ObjectNodeId {
            app: primary_app_ref,
            kind: obj.kind,
            key: obj_key,
        };
        let obj_node_opt: Option<&ObjectNode> = obj_node_map.get(&obj_node_id).copied();

        // Record-typed global variable names for RecordOp / receiver inference.
        let globals_rec: HashSet<String> = obj
            .globals
            .iter()
            .filter(|v| {
                v.ty.as_deref()
                    .map(|ty| ty.trim().to_ascii_lowercase().starts_with("record"))
                    .unwrap_or(false)
            })
            .map(|v| v.name.fold_identifier())
            .collect();

        for (routine_idx, routine) in obj.routines.iter().enumerate() {
            let caller = source_routine_node_id(obj_node_id.clone(), routine);

            let sites = extract_sites_for_routine(
                &pf.file,
                &pf.text,
                &pf.virtual_path,
                &globals_rec,
                obj_idx,
                routine_idx,
            );

            for site in &sites {
                let fp = callee_fp(&site.callee_text);
                let obl_id = ObligationId::CallSite {
                    caller: caller.clone(),
                    span: site.span.clone(),
                    callee_fp: fp,
                };

                let (kind, shape, completeness, routes, finding) = resolve_call_site_obligation(
                    &site.shape,
                    site.arity,
                    &site.callee_text,
                    obj_node_opt,
                    routine,
                    obj,
                    primary_app_ref,
                    graph,
                    index,
                    surface,
                    site.with_state,
                    &pf.file,
                    &site.args,
                );

                match finding {
                    Some(BuiltinDispatchFinding::Flagged { object, method }) => {
                        flagged.push(FlaggedBuiltinDispatchSite {
                            file: pf.virtual_path.clone(),
                            object,
                            method,
                            line: site.span.start.line,
                        });
                    }
                    Some(BuiltinDispatchFinding::Indeterminate { method }) => {
                        indeterminate.push(IndeterminateBuiltinDispatchSite {
                            file: pf.virtual_path.clone(),
                            method,
                            line: site.span.start.line,
                        });
                    }
                    None => {}
                }

                edges.push(ClassifiedEdge {
                    obligation_id: obl_id,
                    edge: Edge {
                        from: caller.clone(),
                        site: SiteId {
                            caller: caller.clone(),
                            span: site.span.clone(),
                            callee_fingerprint: fp,
                        },
                        kind,
                        shape,
                        completeness,
                        routes,
                    },
                });
            }
        }
    }

    FileResolution {
        edges,
        flagged,
        indeterminate,
    }
}

/// Resolve all obligations and compute coverage.
///
/// This is the clean-room inner loop.  It does NOT call any L3 oracle.
/// Publishers are resolved via [`emit_event_flow_edges`]; all call-site
/// obligations are resolved via the shape-dispatch helpers.
fn resolve_full_program_from_parts(
    graph: &ProgramGraph,
    parsed: &[ParsedUnit],
    primary_app_ref: AppRef,
    ws_file_set: &HashSet<String>,
) -> (Vec<ClassifiedEdge>, Coverage, BuiltinDispatchAudit) {
    // Quick ObjectNodeId → &ObjectNode lookup.
    let obj_node_map: HashMap<ObjectNodeId, &ObjectNode> =
        graph.objects.iter().map(|o| (o.id.clone(), o)).collect();

    let index = ResolveIndex::build(graph);
    let surface = DeclSurface::build(graph, parsed);

    let mut obligation_id_set: HashSet<ObligationId> = HashSet::new();
    let mut classified_edges: Vec<ClassifiedEdge> = Vec::new();
    // T0.3: builtin-dispatch audit accumulators — sorted once, after the loop.
    let mut flagged: Vec<FlaggedBuiltinDispatchSite> = Vec::new();
    let mut indeterminate: Vec<IndeterminateBuiltinDispatchSite> = Vec::new();

    // ── Phase 1: resolve call-site obligations (workspace source routines) ────
    //
    // T3 Task 3 (F7): the ordered list of in-scope files is collected FIRST
    // (same nested-loop order as the old serial version: units in `parsed`
    // order, files in `unit.files` order, both already filtered to the
    // primary app / `ws_file_set`), then resolved with an INDEXED `par_iter`
    // — `collect()` on an indexed parallel iterator preserves that order, so
    // `file_results` is byte-identical in order to what the serial loop would
    // have produced one file at a time. Each `resolve_file_obligations` call
    // reads only immutable shared borrows (`graph`/`index`/`surface`/
    // `obj_node_map`) and returns its own `FileResolution` — no shared
    // mutable state crosses the parallel closure, so the accumulator inserts
    // below (which must stay sequential: `HashSet`/`Vec` accumulation order
    // matters for downstream determinism) are unaffected by evaluation order.
    //
    // Runs on a dedicated big-stack pool (`crate::big_stack`), not the rayon
    // global pool: the resolver's receiver/extraction walk recurses over the
    // AL expression tree and can overflow rayon's default ~1 MiB worker stack
    // on real BC files — the same hazard `snapshot::parse::parse_snapshot`
    // already guards against for the lowerer.
    let files_to_resolve: Vec<&ParsedFile> = parsed
        .iter()
        .filter(|unit| graph.apps.find(&unit.app) == Some(primary_app_ref))
        .flat_map(|unit| {
            unit.files
                .iter()
                .filter(|pf| ws_file_set.contains(&pf.virtual_path))
        })
        .collect();

    let file_results: Vec<FileResolution> = crate::big_stack::big_stack_pool().install(|| {
        files_to_resolve
            .par_iter()
            .map(|pf| {
                resolve_file_obligations(
                    pf,
                    primary_app_ref,
                    graph,
                    &index,
                    &surface,
                    &obj_node_map,
                )
            })
            .collect()
    });

    for file_res in file_results {
        // T3 Task 6: `resolve_file_obligations` no longer inserts into
        // `obligation_id_set` inline (it has no access to this whole-run
        // accumulator) — insert from the returned edges' obligation ids
        // instead. Identical set contents: every call-site obligation
        // that would have been inserted inline produces EXACTLY one
        // `ClassifiedEdge` carrying that same id (see the function's own
        // loop), so deriving the id set from the edges post-hoc is a
        // no-op change to the set's membership.
        for ce in &file_res.edges {
            obligation_id_set.insert(ce.obligation_id.clone());
        }
        classified_edges.extend(file_res.edges);
        flagged.extend(file_res.flagged);
        indeterminate.extend(file_res.indeterminate);
    }

    // ── Phase 2: publisher event flow obligations (all apps) ──────────────────
    // emit_event_flow_edges processes ALL graph.routines (no app filter).
    // We must track obligation ids in the same pass so coverage holds.
    let event_edges = emit_event_flow_edges(graph, &index, &surface);
    for edge in event_edges {
        // Each publisher routine emits exactly one EventFlow edge.
        let obl_id = ObligationId::Publisher(edge.from.clone());
        obligation_id_set.insert(obl_id.clone());
        classified_edges.push(ClassifiedEdge {
            obligation_id: obl_id,
            edge,
        });
    }

    // ── Coverage: distinct-id SET equality ────────────────────────────────────
    let edge_id_set: HashSet<ObligationId> = classified_edges
        .iter()
        .map(|ce| ce.obligation_id.clone())
        .collect();

    let mut missing: Vec<ObligationId> = obligation_id_set
        .difference(&edge_id_set)
        .cloned()
        .collect();
    missing.sort();

    let mut extra: Vec<ObligationId> = edge_id_set
        .difference(&obligation_id_set)
        .cloned()
        .collect();
    extra.sort();

    let coverage = Coverage {
        parsed_obligations: obligation_id_set.len(),
        classified_edges: edge_id_set.len(),
        missing,
        extra,
    };

    // T0.3: deterministic sort — the accumulation order above already follows
    // parsed-file/object/routine/site document order (no HashMap iteration),
    // but sorting here makes the output ORDER independent of that traversal
    // order too, per the audit's determinism constraint.
    flagged.sort();
    indeterminate.sort();
    let builtin_dispatch_audit = BuiltinDispatchAudit {
        flagged,
        indeterminate,
    };

    (classified_edges, coverage, builtin_dispatch_audit)
}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Full-program obligation coverage + self-reported taxonomy'd metric.
///
/// # Steps
///
/// 1. Build [`AppSetSnapshot`] from `workspace_root` (via [`SnapshotBuilder`]).
/// 2. Build [`ProgramGraph`] (intern apps, extract nodes, ingest ABI).
/// 3. Parse snapshot for call-site extraction and body lookup.
/// 4. Locate workspace app (primary scope).
/// 5. Resolve all obligations → [`ClassifiedEdge`] set.
/// 6. Compute coverage (distinct-id SET equality).
/// 7. Compute taxonomy'd histograms (whole-program + primary-scoped).
/// 8. Compute ABI ingestion integrity.
///
/// Returns `None` when snapshot build fails (fail-closed).
///
/// # L3 independence
///
/// This function does NOT invoke the L3 oracle.  It is the self-reported
/// north-star metric: the resolution outcome comes entirely from this engine.
#[must_use]
pub fn resolve_full_program(workspace_root: &Path) -> Option<ProgramReport> {
    let ctx = build_context(workspace_root)?;
    Some(resolve_full_program_with(&ctx))
}

/// The substrate-taking core of [`resolve_full_program`] (steps 5–8 of that
/// function's documented pipeline). Callers that already hold a
/// [`ProgramContext`] — e.g. a test harness that rebuilds the context once
/// and resolves it many times — call this directly instead of paying
/// `build_context`'s cost on every resolve.
#[must_use]
pub fn resolve_full_program_with(ctx: &ProgramContext) -> ProgramReport {
    // ── Steps 1–4: shared setup (snapshot → graph → parse → primary app) ──────
    let ProgramContext {
        snap,
        graph,
        parsed,
        primary_app_ref,
        ws_file_set,
        ..
    } = ctx;
    let primary_app_ref = *primary_app_ref;

    // ── Step 5: Resolve all obligations ──────────────────────────────────────
    let (edges, coverage, builtin_dispatch_audit) =
        resolve_full_program_from_parts(graph, parsed, primary_app_ref, ws_file_set);

    // ── Step 6: Histograms ────────────────────────────────────────────────────
    // Collect references to all underlying Edge structs.
    let all_edge_refs: Vec<&Edge> = edges.iter().map(|ce| &ce.edge).collect();
    // `Histogram::of_edges` takes `&[Edge]` — we need owned slices.
    // Build by iterating manually to avoid cloning.
    let histogram = {
        let mut h = Histogram::default();
        for e in &all_edge_refs {
            count_into_histogram(&mut h, e);
        }
        h
    };
    let primary_histogram = {
        let mut h = Histogram::default();
        for ce in &edges {
            if is_primary_scope(ce, primary_app_ref) {
                count_into_histogram(&mut h, &ce.edge);
            }
        }
        h
    };

    // ── Step 7: ABI integrity ─────────────────────────────────────────────────
    // Build a raw ABI index from dep .app files (independent of graph nodes).
    let raw_abi_index = build_raw_abi_index_from_snapshot(snap, &graph.apps);
    // Collect all underlying edges for the ABI check.
    let plain_edges: Vec<Edge> = edges.iter().map(|ce| ce.edge.clone()).collect();
    let abi_integrity = abi_ingestion_integrity(&plain_edges, &raw_abi_index);

    let event_flow_dual_publisher_alias_skips =
        crate::program::resolve::resolver::dual_publisher_alias_skip_count(&graph.routines);

    // Task 3 (preprocessor foundations plan): additive Recovered-parse
    // diagnostic — surfaced, never gating (see `recovered_files`'s doc).
    let recovered_files = crate::snapshot::parse::recovered_file_paths(parsed);

    ProgramReport {
        edges,
        coverage,
        histogram,
        primary_histogram,
        abi_integrity,
        primary_app_ref,
        event_flow_dual_publisher_alias_skips,
        recovered_files,
        builtin_dispatch_audit,
    }
}

/// Export-oriented entry: assemble the whole-program graph + classified edges +
/// primary app ref, WITHOUT computing histograms / coverage / ABI integrity.
///
/// Consumed by [`crate::program::graphify_export`], which needs the assembled
/// [`ProgramGraph`] (for node labels + app-name resolution) alongside the edges.
/// Returns `None` on snapshot build failure (fail-closed), same as
/// [`resolve_full_program`].
#[must_use]
pub fn resolve_full_program_for_export(
    workspace_root: &Path,
) -> Option<(ProgramGraph, Vec<ClassifiedEdge>, AppRef)> {
    let ctx = build_context(workspace_root)?;
    let (edges, _coverage, _builtin_dispatch_audit) = resolve_full_program_from_parts(
        &ctx.graph,
        &ctx.parsed,
        ctx.primary_app_ref,
        &ctx.ws_file_set,
    );
    Some((ctx.graph, edges, ctx.primary_app_ref))
}

/// Shared setup for the whole-program resolvers: snapshot → program graph →
/// parse → primary app ref + workspace file set. Single source of truth so
/// [`resolve_full_program`] and [`resolve_full_program_for_export`] cannot drift.
/// Returns `None` when the snapshot build fails or the workspace app is absent.
///
/// [`crate::lsp::snapshot::LspSnapshot::build_full`] is a second consumer of
/// this exact composition — it additionally needs the [`DepLayer`] this
/// function assembles `graph` from (to store as `Arc<DepLayer>` for a future
/// incremental rung-2 rebuild), so it calls this function directly rather
/// than re-deriving snapshot → parse → graph itself.
///
/// `pub` (shared-substrate refactor, 2026-07-15): a test harness that
/// resolves the same workspace many times builds this once via
/// [`build_context`] and resolves it repeatedly via
/// [`resolve_full_program_with`], instead of paying the full snapshot →
/// parse → graph cost on every resolve. Fields stay `pub(crate)` — external
/// consumers go through the `graph()`/`parsed()` accessors below.
pub struct ProgramContext {
    pub(crate) snap: AppSetSnapshot,
    pub(crate) graph: ProgramGraph,
    /// Every source-bearing unit — except on a shared-tier hit (LSP only,
    /// see [`build_context_from_snapshot_cached`]), where it holds the
    /// workspace unit alone.
    pub(crate) parsed: Vec<ParsedUnit>,
    pub(crate) primary_app_ref: AppRef,
    pub(crate) ws_file_set: HashSet<String>,
    /// The immutable dep layer `graph` was assembled from. Pre-T3-Task-8 this
    /// was built and immediately dropped inside `build_program_graph_from_parsed`
    /// (see [`assemble_program_graph`]'s doc); kept here so a caller that wants
    /// to REUSE it across rebuilds doesn't have to re-derive it a second time.
    pub(crate) dep_layer: DepLayer,
}

impl ProgramContext {
    /// The assembled whole-program graph (shared-substrate consumers only).
    #[must_use]
    pub fn graph(&self) -> &ProgramGraph {
        &self.graph
    }

    /// The parsed units backing `graph` (shared-substrate consumers only).
    #[must_use]
    pub fn parsed(&self) -> &[ParsedUnit] {
        &self.parsed
    }
}

pub fn build_context_res(workspace_root: &Path) -> Result<ProgramContext, String> {
    build_context_from_snapshot(build_snapshot_res(workspace_root)?)
}

/// [`build_context`] with an explicit [`DependencySource`] — the LSP server
/// and CLI index path, which let the user trade dependency depth for memory.
#[must_use]
pub fn build_context_with(
    workspace_root: &Path,
    dependency_source: DependencySource,
    dep_cache: &DepCache,
) -> Option<ProgramContext> {
    let snap = (SnapshotBuilder {
        workspace_root: workspace_root.to_path_buf(),
        // Local providers are NOT part of `DepKey`: a caller that sets them
        // must extend the key (provenance tier + content hash), or a local
        // checkout and the embedded source of the same `.app` would share.
        local_providers: vec![],
    })
    .build_with_options(dependency_source, dep_cache)
    .map(|(snap, _dropped)| snap)
    .ok()?;
    build_context_from_snapshot_cached(snap, dep_cache).ok()
}

/// Step 1 of [`build_context_res`], split out so a caller can inspect the
/// snapshot BEFORE paying for parse + resolve.
///
/// The preflight verdict cache needs exactly this seam: its key is derived from
/// the `AppSetSnapshot` (see `preflight_cache::cache_key` for why a cheaper
/// pre-snapshot key would be unsound), so the snapshot must exist before the
/// lookup, and everything after it must be skippable on a hit.
pub fn build_snapshot_res(workspace_root: &Path) -> Result<AppSetSnapshot, String> {
    let _s = pt::span("preflight", "preflight.snapshot_build");
    (SnapshotBuilder {
        workspace_root: workspace_root.to_path_buf(),
        local_providers: vec![],
    })
    .build()
    .map_err(|e| format!("snapshot build failed: {e:#}"))
}

/// Steps 2-3 of [`build_context_res`]: parse the snapshot, build the layered
/// graph, and locate the primary app. Split from [`build_snapshot_res`] purely
/// so the preflight cache can skip this half on a hit — behaviour is unchanged.
pub fn build_context_from_snapshot(snap: AppSetSnapshot) -> Result<ProgramContext, String> {
    build_context_from_snapshot_cached(snap, &DepCache::default())
}

/// [`build_context_from_snapshot`], sharing the dependency tier through
/// `dep_cache` with every other root that loads the same dependencies.
pub fn build_context_from_snapshot_cached(
    snap: AppSetSnapshot,
    dep_cache: &DepCache,
) -> Result<ProgramContext, String> {
    // ws_file_set: the true workspace source virtual paths (first AppUnit).
    // Excludes embedded dep apps whose AppId matches the workspace AppId.
    let ws_file_set: HashSet<String> = snap
        .apps
        .first()
        .and_then(|u| u.source.as_ref())
        .map(|s| s.files.iter().map(|f| f.virtual_path.clone()).collect())
        .unwrap_or_default();

    // ── Step 2: Parse ONCE, then build the layered graph from that SAME parse ──
    // (T3 Task 5: previously `build_program_graph` parsed the whole snapshot
    // internally to extract nodes, AND this function separately ran its own
    // standalone `parse_snapshot` for the resolver's body-walk below — a full
    // double-parse of every source-bearing app, dependencies included.)
    //
    // T3 Task 8: inlines `build_program_graph_from_parsed`'s own two steps
    // (`build_dep_layer` + find-or-synthesize the workspace `ParsedUnit` +
    // `assemble_program_graph`) rather than calling that wrapper, so the
    // `DepLayer` it builds internally survives into `ProgramContext` instead
    // of being dropped the moment `graph` is assembled. Behavior-preserving:
    // this is exactly what `build_program_graph_from_parsed` does, in the
    // same order (see that function's own doc, and the
    // `assemble_program_graph_matches_build_program_graph_field_by_field`
    // characterization test in `program::build`).
    //
    // A shared dependency tier whose LSP products are already published
    // answers everything the LSP needs from the dependencies, so on such a
    // hit only the workspace is parsed and `parsed` holds the workspace unit
    // alone. Only the LSP passes a shared cache; every other caller passes a
    // throwaway one, never hits, and still gets every unit.
    let shared_tier = dep_cache.get(&DepKey::of(&snap));
    let parsed = {
        let _s = pt::span("preflight", "preflight.parse_snapshot");
        if shared_tier.is_some() {
            snap.apps
                .iter()
                .filter(|u| u.id == snap.workspace_app)
                .filter_map(crate::snapshot::parse::parse_unit)
                .collect()
        } else {
            parse_snapshot(&snap)
        }
    };
    let dep_layer = {
        let _s = pt::span("preflight", "preflight.dep_layer");
        build_dep_layer_cached(
            &snap,
            &crate::program::abi_ingest::AbiCache::new(),
            &parsed,
            dep_cache,
        )
    };
    // `shared_tier` kept the entry alive, so the layer is built on it and the
    // dependency nodes never came from the workspace-only `parsed`.
    assert!(
        shared_tier
            .as_ref()
            .is_none_or(|t| Arc::ptr_eq(t, &dep_layer.dep_nodes)),
        "dependency layer was not built on the held shared entry; \
         dependency nodes would come from a workspace-only parse"
    );
    drop(shared_tier);

    // `snap.apps` is GUID-deduped upstream (H-2), so at most one parsed unit
    // can match the workspace identity.
    let empty_ws_unit;
    let ws_unit: &ParsedUnit = match parsed.iter().find(|u| u.app == snap.workspace_app) {
        Some(u) => u,
        None => {
            empty_ws_unit = ParsedUnit {
                app: snap.workspace_app.clone(),
                files: vec![],
            };
            &empty_ws_unit
        }
    };
    let graph = {
        let _s = pt::span("preflight", "preflight.assemble_graph");
        assemble_program_graph(&dep_layer, ws_unit, &snap)
    };

    // ── Step 3: Locate primary (workspace) app ────────────────────────────────
    let primary_app_ref = graph.apps.find(&snap.workspace_app).ok_or_else(|| {
        format!(
            "workspace app '{}' not present in the assembled program graph",
            snap.workspace_app.name
        )
    })?;

    Ok(ProgramContext {
        snap,
        graph,
        parsed,
        primary_app_ref,
        ws_file_set,
        dep_layer,
    })
}

#[must_use]
pub fn build_context(workspace_root: &Path) -> Option<ProgramContext> {
    build_context_res(workspace_root).ok()
}

// ---------------------------------------------------------------------------
// Preflight coverage status (see
// `docs/superpowers/specs/2026-07-17-preflight-fresh-coverage-design.md` §1)
// ---------------------------------------------------------------------------

/// Preflight coverage status from the FRESH resolver — a narrow, cheap-to-hold
/// summary factored from the SAME pipeline `aldump --program-call-graph-stats`
/// drives (`build_context_res` → `resolve_full_program_with`), not a second
/// hand-rolled pass.
///
/// NOT a bare `usize`: `coverage_holds == false` and `recovered_files > 0` can
/// each coexist with `unknown == 0` and must not launder into "coverage
/// complete" — every field is surfaced so a caller can distinguish "verified
/// clean" from "the instrument itself can't vouch for this run" (instrument-
/// honesty doctrine, CLAUDE.md "Resolution Coverage").
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FreshCoverage {
    /// `primaryScoped` `unknown` — TRUE resolution failures (`ambiguousResolved`
    /// excluded), the `realUnknownRate` definition.
    pub unknown: usize,
    /// The resolve run's own coverage contract (every obligation classified).
    pub coverage_holds: bool,
    /// Files whose parse was `ParseStatus::Recovered` — IR may have dropped
    /// content, so `unknown == 0` does NOT prove completeness over them.
    pub recovered_files: usize,
    /// Symbol-only dependency apps, from the FRESH snapshot
    /// (`AppUnit::source == None`) — one engine, one dependency universe.
    ///
    /// SCOPED to the primary app's reachable declared-dependency closure and
    /// excluding the primary itself: `load_all_apps` deliberately loads EVERY
    /// `.app` found in (ancestor) `.alpackages` folders without app.json
    /// filtering (`src/dependencies.rs`), so an unscoped scan would report
    /// unrelated cached packages as noise — and under `--require-dependencies`
    /// flip exit 4 on a package the primary app never actually depends on.
    ///
    /// EXEMPT: a symbol-only dep whose ABI surface (`AppUnit::abi`'s parsed
    /// `SymbolReference.json`) declares ZERO objects. No bodies exist to be
    /// opaque about — this mirrors the project's `honest_empty` doctrine
    /// (`src/program/resolve/edge.rs`'s `Histogram`). The motivating case is
    /// Microsoft's "Application" umbrella app (`Microsoft_Application_*.app`,
    /// present in ~every BC 24+ workspace): symbol-only with an empty
    /// `SymbolReference.json`, so the un-refined clause warned on every real
    /// workspace forever, devaluing the preflight. A symbol-only dep with
    /// ≥1 ABI object still counts (e.g. Base Application, which declares
    /// real tables/codeunits).
    ///
    /// Display identity = `AppId.name`; deduped, sorted (name, then guid) for
    /// deterministic messages.
    pub opaque_apps: Vec<String>,
}

/// Symbol-only dep app names in the primary app's reachable declared-dependency
/// closure. BFS over `AppUnit.declared_deps` GUIDs starting at the workspace app;
/// the snapshot may contain UNRELATED cached packages (`load_all_apps` loads every
/// `.app` in ancestor `.alpackages` without app.json filtering), so an unscoped
/// scan would report noise — and under `--require-dependencies` flip exit 4 on it.
///
/// A symbol-only dep whose ABI surface declares zero objects is EXEMPT (see
/// [`FreshCoverage::opaque_apps`]'s doc for the full rationale) — checked
/// directly against `AppUnit::abi`'s parsed object list, the ABI/SymbolReference
/// layer itself, rather than the assembled `ProgramGraph`'s downstream node
/// population (which could apply unrelated filtering/collapsing and would
/// answer a different question than "does this app's ABI declare anything at
/// all").
fn opaque_dependency_closure(snap: &AppSetSnapshot) -> Vec<String> {
    use std::collections::{HashMap, HashSet, VecDeque};
    let by_guid: HashMap<String, &AppUnit> = snap
        .apps
        .iter()
        .map(|u| (u.id.guid.to_ascii_lowercase(), u))
        .collect();
    let primary_guid = snap.workspace_app.guid.to_ascii_lowercase();
    let mut seen: HashSet<String> = HashSet::new();
    let mut queue: VecDeque<&AppUnit> = VecDeque::new();
    if let Some(primary) = by_guid.get(&primary_guid) {
        seen.insert(primary_guid.clone());
        queue.push_back(primary);
    }
    let mut opaque: Vec<(String, String)> = Vec::new(); // (name, guid) for stable sort
    while let Some(unit) = queue.pop_front() {
        for dep in &unit.declared_deps {
            let guid = dep.app_id.to_ascii_lowercase();
            if !seen.insert(guid.clone()) {
                continue;
            }
            if let Some(u) = by_guid.get(&guid) {
                let has_abi_objects = u.abi.as_ref().is_some_and(|abi| !abi.objects.is_empty());
                if u.source.is_none() && has_abi_objects {
                    opaque.push((u.id.name.clone(), u.id.guid.clone()));
                }
                queue.push_back(u);
            }
            // A declared dep ABSENT from the snapshot is a real gap, but
            // reporting it is an explicit spec follow-up (OUTSTANDING.md) —
            // not silently widened here.
        }
    }
    opaque.sort();
    opaque.dedup();
    opaque.into_iter().map(|(name, _)| name).collect()
}

/// Compute [`FreshCoverage`] for `workspace_root`: build the fresh program
/// context, resolve it once, and reduce the full [`ProgramReport`] down to the
/// tiny preflight status a caller can hold onto cheaply.
///
/// The `ctx` (snapshot + graph + parsed files — the whole semantic model) is
/// deliberately local to this function and dropped when it returns: callers
/// hold only the small [`FreshCoverage`] value, never the whole-program model
/// (spec §3's memory-sequencing requirement — `run_analyze` computes this
/// FIRST and lets it go before assembling the separate L3 model, so the two
/// semantic models are never resident together).
pub fn fresh_coverage(workspace_root: &Path) -> Result<FreshCoverage, String> {
    let snap = build_snapshot_res(workspace_root)?;

    // The verdict cache sits HERE, between the snapshot and everything
    // expensive: the key is derived from the snapshot (see
    // `preflight_cache::cache_key`), and parse + dep_layer + assemble + resolve
    // + teardown — 2,642 ms of a 3,171 ms DO run — are what a hit skips.
    //
    // `cache_key` returns `None` when any component of the key cannot be
    // established; that runs uncached rather than keying on a weaker identity.
    let cache_key = preflight_cache::cache_key(&snap);
    if let Some(key) = &cache_key
        && let Some(hit) = preflight_cache::lookup(key)
    {
        return Ok(hit);
    }

    let ctx = build_context_from_snapshot(snap)?;
    let report = {
        let _s = pt::span("preflight", "preflight.resolve_full");
        resolve_full_program_with(&ctx)
    };
    let opaque_apps = {
        let _s = pt::span("preflight", "preflight.opaque_closure");
        opaque_dependency_closure(&ctx.snap)
    };
    let out = FreshCoverage {
        unknown: report.primary_histogram.unknown,
        coverage_holds: coverage_holds(&report.coverage),
        recovered_files: report.recovered_files.len(),
        opaque_apps,
    };
    // Store only on the `Ok` path — which is the only path that reaches here.
    // `Err` (could-not-verify) is NEVER cached: it captures TRANSIENT
    // environment (a locked file, a dying disk), so persisting it would laminate
    // a one-time I/O flake into a lasting verdict. Degraded `Ok`s ARE cached —
    // they are as deterministic as clean ones, and the key moves on any edit.
    if let Some(key) = &cache_key {
        preflight_cache::store(key, &out);
    }

    // ctx (snapshot + graph + parsed) drops HERE — callers hold only the tiny
    // status struct, never the whole semantic model (spec §3 memory sequencing).
    // Dropped EXPLICITLY, inside a span, for the same reason `gate.teardown`
    // and `context.ctx_drop` are: this is the whole workspace semantic model
    // being freed, it was already being freed at exactly this point, and
    // leaving it unnamed is how 16.6 % of an 8020 run stayed invisible until
    // `docs/2026-07-31-profile-attribution.md`. `ProgramReport` is owned (it
    // borrows nothing from `ctx`), so the order below is free to choose.
    {
        let _s = pt::span("preflight", "preflight.ctx_drop");
        drop(report);
        drop(ctx);
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Increment histogram counters for one edge, mirroring [`Histogram::of_edges`].
fn count_into_histogram(h: &mut Histogram, e: &Edge) {
    use crate::program::resolve::edge::ObligationOutcome;

    h.total += 1;
    match classify_obligation(e) {
        ObligationOutcome::Resolved => {
            // Classify by best evidence (Source=0, Catalog=1, Abi/Opaque=2).
            let mut best: Option<u8> = None;
            for r in &e.routes {
                if r.evidence.kind() == EvidenceKind::Unknown
                    || r.target == RouteTarget::Unresolved
                    || !r.fires_by_default()
                {
                    continue;
                }
                let score: u8 = match r.evidence {
                    Evidence::Source => 0,
                    Evidence::Catalog => 1,
                    Evidence::Abi | Evidence::Opaque => 2,
                    Evidence::Unknown(_) => continue,
                };
                best = Some(best.map_or(score, |b: u8| b.min(score)));
            }
            match best {
                Some(0) => h.resolved_source += 1,
                Some(1) => h.resolved_catalog += 1,
                Some(_) => h.resolved_abi_external += 1,
                None => {
                    unreachable!("Resolved edge must have >=1 default-firing non-Unknown route")
                }
            }
        }
        ObligationOutcome::ConditionalResolved => h.conditional_resolved += 1,
        ObligationOutcome::HonestDynamic => h.honest_dynamic += 1,
        ObligationOutcome::HonestEmpty => h.honest_empty += 1,
        ObligationOutcome::Unknown => h.unknown += 1,
        ObligationOutcome::AmbiguousResolved => h.ambiguous_resolved += 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::program::node::ObjKey;
    use crate::program::resolve::edge::{Condition, SourcePos};

    fn rid(name: &str) -> RoutineNodeId {
        RoutineNodeId {
            object: ObjectNodeId {
                app: AppRef(0),
                kind: ObjectKind::Codeunit,
                key: ObjKey::Id(1),
            },
            name_lc: name.to_string(),
            enclosing_member_lc: None,
            params_count: 0,
            sig_fp: 0,
        }
    }

    fn ambiguous_route(name: &str) -> Route {
        Route {
            target: RouteTarget::Routine(rid(name)),
            evidence: Evidence::Source,
            conditions: vec![Condition::AmbiguousDispatch],
            witness: Witness::SourceSpan {
                file: "f.al".into(),
                span: (0, 1),
            },
            receiver_tier: None,
        }
    }

    fn edge_with(shape: DispatchShape, completeness: SetCompleteness, routes: Vec<Route>) -> Edge {
        let caller = rid("c");
        Edge {
            from: caller.clone(),
            site: SiteId {
                caller,
                span: CanonicalSpan {
                    unit: "u".into(),
                    start: SourcePos { line: 1, col: 1 },
                    end: SourcePos { line: 1, col: 2 },
                },
                callee_fingerprint: 1,
            },
            kind: EdgeKind::Call,
            shape,
            completeness,
            routes,
        }
    }

    /// `completeness_for_shape(AmbiguousOverload) == Complete` (Task 3): the
    /// candidate set is a snapshot-enumerated CLOSED set, unlike
    /// Polymorphic's open-world `Partial { ReverseDependentImplementers }`.
    #[test]
    fn completeness_for_ambiguous_overload_shape_is_complete() {
        assert_eq!(
            completeness_for_shape(DispatchShape::AmbiguousOverload),
            SetCompleteness::Complete
        );
    }

    /// `count_into_histogram` is a DOCUMENTED duplicate of
    /// `Histogram::of_edges` (full.rs's own module doc calls this out) — Task
    /// 3 requires BOTH copies stay in lockstep. Pins the `ambiguous_resolved`
    /// arm here independently of `edge.rs`'s own `Histogram::of_edges` test.
    #[test]
    fn count_into_histogram_counts_ambiguous_resolved_like_of_edges() {
        let edges = vec![
            edge_with(
                DispatchShape::AmbiguousOverload,
                SetCompleteness::Complete,
                vec![ambiguous_route("overload_a"), ambiguous_route("overload_b")],
            ),
            edge_with(
                DispatchShape::Exact,
                SetCompleteness::Complete,
                vec![Route {
                    target: RouteTarget::Routine(rid("helper")),
                    evidence: Evidence::Source,
                    conditions: vec![],
                    witness: Witness::SourceSpan {
                        file: "f.al".into(),
                        span: (0, 1),
                    },
                    receiver_tier: None,
                }],
            ),
        ];

        // The `count_into_histogram`-driven path (what `resolve_full_program`
        // actually calls).
        let mut h = Histogram::default();
        for e in &edges {
            count_into_histogram(&mut h, e);
        }
        assert_eq!(h.ambiguous_resolved, 1);
        assert_eq!(h.resolved_source, 1);
        assert_eq!(h.unknown, 0);
        assert_eq!(h.total, 2);

        // The two copies must agree exactly (the "documented duplicate" contract).
        let h2 = Histogram::of_edges(&edges);
        assert_eq!(
            h, h2,
            "count_into_histogram must mirror Histogram::of_edges"
        );
    }

    // -----------------------------------------------------------------------
    // Task 3 (preprocessor foundations plan): the `recovered_files`
    // diagnostic, wired end to end through `resolve_full_program` — no
    // CDO_WS needed, a bare on-disk temp workspace suffices (mirrors
    // `snapshot::tests::write_minimal_app_json`'s pattern).
    // -----------------------------------------------------------------------

    fn write_minimal_workspace(dir: &std::path::Path) {
        let app_json = r#"{
    "id": "22222222-0000-0000-0000-000000000002",
    "name": "Task3 Recovered Probe",
    "publisher": "probe",
    "version": "1.0.0.0"
}"#;
        std::fs::write(dir.join("app.json"), app_json).expect("write app.json");
    }

    #[test]
    fn resolve_full_program_reports_recovered_file_with_its_path() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_minimal_workspace(dir.path());
        std::fs::write(
            dir.path().join("Clean.al"),
            "codeunit 50000 T { procedure Foo() begin end; }",
        )
        .expect("write Clean.al");
        // An unbalanced #if forces tree-sitter error recovery.
        std::fs::write(
            dir.path().join("Broken.al"),
            "codeunit 50001 T { procedure Foo() begin\n#if NEVER_CLOSED\nBar();\nend; }",
        )
        .expect("write Broken.al");

        let report = resolve_full_program(dir.path()).expect("resolve_full_program");
        assert_eq!(
            report.recovered_files,
            vec!["Task3 Recovered Probe::Broken.al".to_string()],
            "only Broken.al must be reported, with its path — got {:?}",
            report.recovered_files
        );
    }

    #[test]
    fn resolve_full_program_recovered_files_empty_when_workspace_is_clean() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_minimal_workspace(dir.path());
        std::fs::write(
            dir.path().join("Clean.al"),
            "codeunit 50000 T { procedure Foo() begin end; }",
        )
        .expect("write Clean.al");

        let report = resolve_full_program(dir.path()).expect("resolve_full_program");
        assert!(
            report.recovered_files.is_empty(),
            "a whole-clean workspace must report zero recovered files; got {:?}",
            report.recovered_files
        );
    }

    // -----------------------------------------------------------------------
    // T3 (LSP-migration arc) Task 3: real (CDO-scale) stage-split wall-clock
    // measurement, feeding the arc's rung-1/rung-2 incremental-updater
    // budgets (`docs/superpowers/plans/2026-07-12-t3-lsp-migration.md`).
    //
    // Lives HERE — a `#[cfg(test)]` unit test inside `full.rs` itself, not
    // under `tests/` — on purpose: `resolve_full_program_from_parts` is a
    // private fn, invisible to any external-crate integration test (every
    // `tests/*.rs` file compiles as its own crate). A child module of `full`
    // sees private items of its ancestor for free, so this needs ZERO
    // visibility widening. `benches/engine_stages.rs` (also an external
    // crate) instead benches only the PUBLIC stages plus `resolve_full_
    // program`'s total and derives the same "resolve inner loop" number by
    // subtraction — see that bench file's module doc.
    // -----------------------------------------------------------------------

    /// Prints the program-engine's real per-stage wall-clock split — snapshot
    /// / parse / build(graph) / `ResolveIndex::build` / `DeclSurface::build` /
    /// resolve (inner loop, DERIVED by subtraction) — median of 3 runs, on
    /// the real CDO workspace.
    ///
    /// `build_program_graph` calls `parse_snapshot` INTERNALLY (to extract
    /// object/routine nodes) and `resolve_full_program_from_parts` is called
    /// AFTER a second, standalone `parse_snapshot` (mirroring `build_context`,
    /// which this test intentionally does NOT call so each stage boundary
    /// stays separately timed) — so two derived numbers are computed rather
    /// than measured directly: `build(graph) only` = `build_program_graph`
    /// total minus `parse`, and `resolve inner loop only` =
    /// `resolve_full_program_from_parts` total minus the standalone
    /// `ResolveIndex::build`/`DeclSurface::build` times (that function rebuilds
    /// both internally; timing them standalone first gives the subtrahend).
    ///
    /// Run: `CDO_WS=<path> cargo test --release stage_split -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn stage_split_wall_clock_on_cdo() {
        let Some(ws) = std::env::var_os("CDO_WS")
            .map(std::path::PathBuf::from)
            .filter(|p| p.exists())
        else {
            eprintln!("stage_split_wall_clock_on_cdo: CDO_WS unset or missing, skipping");
            return;
        };

        const RUNS: usize = 3;

        fn median(mut xs: Vec<std::time::Duration>) -> std::time::Duration {
            xs.sort();
            xs[xs.len() / 2]
        }

        let mut snapshot_times = Vec::with_capacity(RUNS);
        let mut parse_times = Vec::with_capacity(RUNS);
        let mut ws_only_parse_times = Vec::with_capacity(RUNS);
        let mut build_graph_total_times = Vec::with_capacity(RUNS);
        let mut resolve_index_times = Vec::with_capacity(RUNS);
        let mut body_map_times = Vec::with_capacity(RUNS);
        let mut resolve_from_parts_total_times = Vec::with_capacity(RUNS);

        for run in 0..RUNS {
            let t0 = std::time::Instant::now();
            let snap = (SnapshotBuilder {
                workspace_root: ws.clone(),
                local_providers: vec![],
            })
            .build()
            .expect("CDO snapshot build");
            snapshot_times.push(t0.elapsed());

            let cache = crate::program::abi_ingest::AbiCache::new();
            let t1 = std::time::Instant::now();
            // Fully-qualified (not top-level imported): this ignored
            // benchmark is the ONLY caller left using the parse-internally
            // wrapper directly — everything else (production
            // `build_context`, T3 Task 5) uses `build_program_graph_from_parsed`
            // to avoid a top-level import that the plain (non-test) build
            // would otherwise flag unused.
            let graph = crate::program::build::build_program_graph(&snap, &cache);
            build_graph_total_times.push(t1.elapsed());

            let t2 = std::time::Instant::now();
            let parsed = parse_snapshot(&snap);
            parse_times.push(t2.elapsed());

            // Workspace-only parse (excludes all dependency apps' source) —
            // isolates "dep-parse" for the rung-2 budget (a workspace-file
            // save never needs to re-parse unchanged dependency source; see
            // this test's results-doc consumer for the rung-2 definition).
            let ws_only_snap = AppSetSnapshot {
                apps: vec![snap.apps[0].clone()],
                workspace_app: snap.workspace_app.clone(),
                world: snap.world.clone(),
            };
            let t2b = std::time::Instant::now();
            let _ws_only_parsed = parse_snapshot(&ws_only_snap);
            ws_only_parse_times.push(t2b.elapsed());

            let t3 = std::time::Instant::now();
            let index = ResolveIndex::build(&graph);
            resolve_index_times.push(t3.elapsed());
            drop(index);

            let t4 = std::time::Instant::now();
            let surface = DeclSurface::build(&graph, &parsed);
            body_map_times.push(t4.elapsed());
            drop(surface);

            let primary_app_ref = graph
                .apps
                .find(&snap.workspace_app)
                .expect("workspace app must be present in the graph");
            let ws_file_set: HashSet<String> = snap
                .apps
                .first()
                .and_then(|u| u.source.as_ref())
                .map(|s| s.files.iter().map(|f| f.virtual_path.clone()).collect())
                .unwrap_or_default();

            let t5 = std::time::Instant::now();
            let (edges, coverage, _audit) =
                resolve_full_program_from_parts(&graph, &parsed, primary_app_ref, &ws_file_set);
            resolve_from_parts_total_times.push(t5.elapsed());

            assert!(
                coverage_holds(&coverage),
                "run {run}: coverage contract must hold on CDO"
            );
            assert!(!edges.is_empty(), "run {run}: CDO must produce edges");
        }

        let snapshot_med = median(snapshot_times);
        let parse_med = median(parse_times);
        let ws_only_parse_med = median(ws_only_parse_times);
        let build_graph_total_med = median(build_graph_total_times);
        let resolve_index_med = median(resolve_index_times);
        let body_map_med = median(body_map_times);
        let resolve_from_parts_total_med = median(resolve_from_parts_total_times);

        let build_graph_only = build_graph_total_med.saturating_sub(parse_med);
        let dep_parse_only = parse_med.saturating_sub(ws_only_parse_med);
        let index_plus_body_map = resolve_index_med + body_map_med;
        let resolve_inner_loop = resolve_from_parts_total_med
            .saturating_sub(resolve_index_med)
            .saturating_sub(body_map_med);
        // rung-2 = everything minus snapshot minus dep-parse (a workspace
        // save doesn't need to reload .alpackages or re-parse unchanged dep
        // source) — see the T3 plan's Task 3 brief.
        let rung2_budget =
            ws_only_parse_med + build_graph_only + index_plus_body_map + resolve_inner_loop;

        if index_plus_body_map > std::time::Duration::from_millis(30) {
            eprintln!(
                "\n*** RED FLAG: ResolveIndex::build + DeclSurface::build = {index_plus_body_map:?} \
                 > 30ms on CDO scale — Task 9's documented contingency applies (transient \
                 rebuild breaks the rung-1 100ms budget). ***\n"
            );
        }

        eprintln!("=== stage_split_wall_clock_on_cdo (median of {RUNS} runs, CDO_WS={ws:?}) ===");
        eprintln!("snapshot                                          : {snapshot_med:?}");
        eprintln!("parse (parse_snapshot, standalone, ws+deps)       : {parse_med:?}");
        eprintln!("  -> parse, workspace-only [derived input]        : {ws_only_parse_med:?}");
        eprintln!("  -> dep-parse only [derived]                     : {dep_parse_only:?}");
        eprintln!("build_program_graph (TOTAL, incl. internal parse) : {build_graph_total_med:?}");
        eprintln!("  -> build(graph) only [derived]                  : {build_graph_only:?}");
        eprintln!("ResolveIndex::build                               : {resolve_index_med:?}");
        eprintln!("DeclSurface::build                                    : {body_map_med:?}");
        eprintln!(
            "  -> ResolveIndex + DeclSurface combined               : {index_plus_body_map:?}"
        );
        eprintln!(
            "resolve_full_program_from_parts (TOTAL, incl. index+DeclSurface rebuild): {resolve_from_parts_total_med:?}"
        );
        eprintln!("  -> resolve inner loop only [derived]             : {resolve_inner_loop:?}");
        eprintln!(
            "  -> RUNG-2 BUDGET (ws-parse + build(graph) + index+DeclSurface + resolve): {rung2_budget:?}"
        );
    }

    // -----------------------------------------------------------------------
    // T3 (LSP-migration arc) Task 6: `resolve_file_obligations` — the
    // per-file resolve entry point extracted VERBATIM from this function's
    // own Phase-1 `for pf in &unit.files` loop body. This test IS the
    // acceptance bar the task brief demands: per-file output must equal the
    // full run's Phase-1 edges filtered to that file, AND concatenating
    // every file's output in file order must equal the full run's Phase-1
    // edge list EXACTLY (order included).
    // -----------------------------------------------------------------------

    #[test]
    fn resolve_file_obligations_matches_full_run_per_file_and_in_concatenation() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_minimal_workspace(dir.path());
        std::fs::write(
            dir.path().join("A.al"),
            r#"codeunit 50000 A
{
    procedure Foo()
    begin
        Bar();
        Helper();
    end;

    procedure Helper()
    begin
    end;
}
"#,
        )
        .expect("write A.al");
        std::fs::write(
            dir.path().join("B.al"),
            r#"codeunit 50001 B
{
    procedure Bar()
    begin
    end;

    procedure Baz()
    begin
        Bar();
    end;
}
"#,
        )
        .expect("write B.al");

        let ctx = build_context(dir.path()).expect("build_context");
        let ProgramContext {
            graph,
            parsed,
            primary_app_ref,
            ws_file_set,
            ..
        } = &ctx;
        let primary_app_ref = *primary_app_ref;

        // The full-run baseline (production entry point).
        let (full_edges, coverage, _audit) =
            resolve_full_program_from_parts(graph, parsed, primary_app_ref, ws_file_set);
        assert!(coverage_holds(&coverage), "fixture coverage must hold");

        // Phase-1 (call-site) edges only, in the full run's own order —
        // Phase 2 (Publisher/event-flow) edges are appended after Phase 1
        // and are out of scope for this per-file comparison.
        let phase1_full: Vec<&ClassifiedEdge> = full_edges
            .iter()
            .filter(|ce| matches!(ce.obligation_id, ObligationId::CallSite { .. }))
            .collect();
        assert!(
            !phase1_full.is_empty(),
            "fixture must produce at least one call-site edge"
        );

        // Rebuild the SAME index/surface/obj_node_map
        // `resolve_full_program_from_parts` builds internally (it is a
        // private inner helper with no other seam to observe from) — this
        // mirrors its own setup exactly.
        let obj_node_map: HashMap<ObjectNodeId, &ObjectNode> =
            graph.objects.iter().map(|o| (o.id.clone(), o)).collect();
        let index = ResolveIndex::build(graph);
        let surface = DeclSurface::build(graph, parsed);

        // Walk in the EXACT same order `resolve_full_program_from_parts`
        // does: parsed units (filtered to the primary app) x unit.files
        // (filtered to ws_file_set).
        let mut per_file_concat: Vec<ClassifiedEdge> = Vec::new();
        let mut checked_files = 0usize;
        for unit in parsed {
            let Some(app_ref) = graph.apps.find(&unit.app) else {
                continue;
            };
            if app_ref != primary_app_ref {
                continue;
            }
            for pf in &unit.files {
                if !ws_file_set.contains(&pf.virtual_path) {
                    continue;
                }
                let file_res = resolve_file_obligations(
                    pf,
                    primary_app_ref,
                    graph,
                    &index,
                    &surface,
                    &obj_node_map,
                );

                // Per-file assertion: this file's edges equal the full run's
                // Phase-1 edges filtered to this file's virtual_path.
                let expected: Vec<&ClassifiedEdge> = phase1_full
                    .iter()
                    .copied()
                    .filter(|ce| ce.edge.site.span.unit == pf.virtual_path)
                    .collect();
                assert_eq!(
                    file_res.edges.len(),
                    expected.len(),
                    "file {} edge count mismatch",
                    pf.virtual_path
                );
                for (got, want) in file_res.edges.iter().zip(expected.iter()) {
                    assert_eq!(
                        &got.obligation_id, &want.obligation_id,
                        "file {}",
                        pf.virtual_path
                    );
                    assert_eq!(&got.edge, &want.edge, "file {}", pf.virtual_path);
                }

                checked_files += 1;
                per_file_concat.extend(file_res.edges);
            }
        }
        assert!(
            checked_files >= 2,
            "fixture must exercise >=2 workspace files"
        );

        // Concatenation-in-file-order equals the full run's Phase-1 edge
        // list EXACTLY (order included).
        assert_eq!(per_file_concat.len(), phase1_full.len());
        for (got, want) in per_file_concat.iter().zip(phase1_full.iter()) {
            assert_eq!(&got.obligation_id, &want.obligation_id);
            assert_eq!(&got.edge, &want.edge);
        }
    }

    // -----------------------------------------------------------------------
    // Task 1: build_context_res — Result-returning context builder
    // -----------------------------------------------------------------------

    #[test]
    fn build_context_res_preserves_error_text_for_missing_workspace() {
        let result = build_context_res(std::path::Path::new("Z:/definitely/not/a/workspace/xyzzy"));
        match result {
            Err(err) => {
                assert!(
                    err.contains("snapshot build failed"),
                    "error text must preserve the real snapshot-build failure, got: {err}"
                );
            }
            Ok(_) => panic!("nonexistent workspace must return Err"),
        }
    }

    #[test]
    fn build_context_matches_res_variant_on_success() {
        // Any committed small fixture workspace works; ws-d2 is suitable.
        let ws = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/ws-d2");
        assert!(build_context_res(&ws).is_ok());
        assert!(build_context(&ws).is_some());
    }

    // -----------------------------------------------------------------------
    // Task 2: FreshCoverage + fresh_coverage(ws) + opaque dependency closure
    // -----------------------------------------------------------------------

    #[test]
    fn fresh_coverage_matches_direct_resolve_on_neutral_fixture() {
        let ws = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/r0-corpus/ws-e2e");
        let fc = fresh_coverage(&ws).expect("neutral fixture resolves");
        let report = resolve_full_program(&ws).expect("same fixture");
        assert_eq!(fc.unknown, report.primary_histogram.unknown);
        assert_eq!(fc.coverage_holds, coverage_holds(&report.coverage));
        assert_eq!(fc.recovered_files, report.recovered_files.len());
        assert!(fc.opaque_apps.is_empty(), "ws-e2e has no dependencies");
    }

    #[test]
    fn fresh_coverage_reports_symbol_only_dep_in_closure() {
        let ws = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/r0-corpus/ws-baseapp-closure");
        let fc = fresh_coverage(&ws).expect("fixture resolves");
        // The committed Microsoft Base Application .app is symbol-only (no embedded
        // source) and declared by the fixture's app.json — it must appear by NAME.
        assert!(
            fc.opaque_apps
                .iter()
                .any(|n| n.contains("Base Application")),
            "opaque_apps = {:?}",
            fc.opaque_apps
        );
        // The primary app itself must never be listed.
        assert!(!fc.opaque_apps.iter().any(|n| n.is_empty()));
    }

    /// A symbol-only dep whose `SymbolReference.json` declares ZERO objects
    /// provably hides nothing (no bodies exist to be opaque about) — the
    /// Microsoft "Application" umbrella app's real-world shape (present in
    /// ~every BC 24+ workspace). It must be EXEMPT from `opaque_apps`, unlike
    /// `fresh_coverage_reports_symbol_only_dep_in_closure`'s Base Application
    /// fixture, which declares a real table and stays reported.
    #[test]
    fn fresh_coverage_exempts_empty_abi_symbol_only_dep() {
        let ws = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/r0-corpus/ws-empty-abi-dep");
        let fc = fresh_coverage(&ws).expect("fixture resolves");
        assert!(
            fc.opaque_apps.is_empty(),
            "an empty-ABI symbol-only dep must not be reported opaque: {:?}",
            fc.opaque_apps
        );
    }

    #[test]
    fn fresh_coverage_err_on_missing_workspace() {
        assert!(fresh_coverage(std::path::Path::new("Z:/no/such/ws")).is_err());
    }

    /// Spec §5 pin: a dependency with EMBEDDED SOURCE must never be reported
    /// opaque — distinguishing "not opaque because source-bearing" from "no
    /// deps at all" (`fresh_coverage_matches_direct_resolve_on_neutral_fixture`
    /// above only proves the latter: ws-e2e has zero declared deps, so its
    /// empty `opaque_apps` is vacuous for THIS claim).
    ///
    /// `tests/r3a4-fixtures/ws`'s sole dependency ("Dep Chain",
    /// `cccccccc-…`) embeds `DepChain.Codeunit.al` directly inside the `.app`
    /// package (see `tests/r3/r3a4_differential.rs`'s fixture doc) and is the
    /// workspace's ONLY declared dependency, so any non-empty `opaque_apps`
    /// would be unambiguously attributable to it — verified directly against
    /// `AppUnit::source` below rather than assumed from the fixture's name.
    ///
    /// The sibling `tests/r3a5-fixtures/ws` fixture (which a prior review pass
    /// suggested) does NOT qualify for this pin: it declares a SECOND,
    /// symbol-only dep ("Symbol Only Util") whose `SymbolReference.json`
    /// carries a real Codeunit object, so it legitimately DOES land in
    /// `opaque_apps` — an `is_empty()` assertion against that fixture would
    /// fail for a reason unrelated to the source-bearing dep this test pins.
    #[test]
    fn fresh_coverage_source_bearing_dep_not_opaque() {
        let ws = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/r3a4-fixtures/ws");

        // Verify the dep is genuinely source-bearing BEFORE trusting the
        // opaque_apps assertion below — never assume from the fixture name.
        let snap = (SnapshotBuilder {
            workspace_root: ws.clone(),
            local_providers: vec![],
        })
        .build()
        .expect("r3a4 fixture snapshot builds");
        let chain_dep = snap
            .apps
            .iter()
            .find(|u| {
                u.id.guid
                    .eq_ignore_ascii_case("cccccccc-0001-0000-0000-000000000001")
            })
            .expect("Dep Chain app present in snapshot");
        assert!(
            chain_dep.source.is_some(),
            "Dep Chain must be genuinely source-bearing for this pin to be meaningful"
        );
        // Non-vacuity guard: the dep must be IN the primary's declared closure —
        // if a future fixture edit dropped it from app.json while the .app stayed
        // in .alpackages, the BFS would skip it and the empty-opaque assertion
        // below would pass for the wrong reason.
        assert!(
            snap.apps[0].declared_deps.iter().any(|d| d
                .app_id
                .eq_ignore_ascii_case("cccccccc-0001-0000-0000-000000000001")),
            "Dep Chain must be DECLARED by the primary app.json (closure membership)"
        );

        let fc = fresh_coverage(&ws).expect("r3a4 fixture resolves");
        assert!(
            fc.opaque_apps.is_empty(),
            "a source-bearing dep must never be reported opaque: {:?}",
            fc.opaque_apps
        );
    }
}

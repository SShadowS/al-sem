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
use crate::program::build::{DepInput, DepLayer, assemble_program_graph, build_dep_layer_cached};
use crate::program::dep_cache::{DepCache, DepKey};
use crate::program::dep_summary::parse_for_build;
use crate::program::graph::ProgramGraph;
use crate::program::node::{AppRef, ObjKey, ObjectNodeId, RoutineNodeId};
use crate::program::node_extract::ObjectNode;
use crate::program::profile::{BuildProfile, DependencyBodies};
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
    /// See [`ProgramReport::site_facts`].
    pub site_facts: Vec<(ObligationId, SiteFacts)>,
    /// The parens-less value reads this file's resolution kept as calls.
    pub parenless: Vec<al_syntax::ir::ExprId>,
}

/// The parens-less value reads (`X.M` / `M` without `()`) the resolver kept as
/// calls, by owning app and file (`ParsedFile::virtual_path`). The body walk
/// takes these as calls too, so its call sites match the program's edges.
pub type ParenlessCalls = HashMap<(AppRef, String), HashSet<al_syntax::ir::ExprId>>;

/// What the resolver knew about one call site beyond its edge, for the B3
/// adapter (engine-switch S3.2 interfaces, S6.0 receivers). Only member call
/// sites get an entry.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SiteFacts {
    /// The interface (folded name) an interface-receiver call dispatched over.
    pub interface: Option<String>,
    /// The member call's receiver as the resolver typed it.
    pub receiver: Option<ReceiverFact>,
    /// The object an object run (`Page.Run(Page::X)`, `PageVar.RunModal()`)
    /// runs, when its entry triggers resolved to an EMPTY set (S9.0d: the
    /// object's source declares none). The edge itself then names no routine;
    /// the adapter needs the object to keep its run shape.
    pub run_target: Option<ObjectNodeId>,
    /// S9.0d: the build context the call site sits in (`Ir::preproc_context`:
    /// its `#if` branches and its routine's arm); empty for unconditional code.
    pub build_context: Vec<(String, bool)>,
    /// S9.0d: the trigger routines this record operation's `TriggerSiteRule`
    /// removed because its RunTrigger is false (a `false` literal, or left
    /// out). The compiler's call graph keeps such a trigger edge; the oracle
    /// audit reads this to tell that limit from a missing edge.
    pub run_trigger_excluded: Vec<RoutineNodeId>,
    /// S9.0d: a record operation written without a receiver (`Insert(true)` on
    /// the implicit `Rec`). The compiler's call graph has no trigger edges from
    /// one.
    pub unqualified_record_op: bool,
}

/// A member call's receiver (engine-switch S6.0): what the adapter used to ask
/// L3's receiver inference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReceiverFact {
    /// The receiver's type text: the declaration's, canonicalized like the
    /// model's variable types (`program::body::scope::canonicalize_type_text`),
    /// when the receiver is a parameter, local, named return value or object
    /// global; otherwise (implicit `Rec`, a `CurrPage` part, a dataitem, a call
    /// result) rendered from the resolved object, `Record <table>` /
    /// `<Kind> <object>`, with the object's own name. `None` when neither exists.
    pub type_text: Option<String>,
    pub ty: ReceiverFactType,
}

/// The receiver's type, as far as the adapter needs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReceiverFactType {
    /// An object (`Codeunit`, `Page`, …): its kind, and its name as the
    /// declaration writes it (unquoted), else folded.
    Object {
        kind: ObjectKind,
        name: String,
    },
    /// A record; `table` is the resolved table, if any.
    Record {
        table: Option<ObjectNodeId>,
    },
    Other,
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
    /// Per member call site, keyed by its obligation: the interface it
    /// dispatched over (engine-switch S3.2) and its receiver (S6.0). The edge
    /// itself does not carry them; the B3 adapter needs them.
    pub site_facts: HashMap<ObligationId, SiteFacts>,
    /// See [`ParenlessCalls`]: the workspace files' parens-less calls.
    pub parenless_calls: ParenlessCalls,
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
    // The app that owns the calling body (engine-switch S7.1): the primary app,
    // or a dependency for owning-app body resolution.
    caller_app: AppRef,
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
    // S9.0e: the call site's build context (`Ir::preproc_context`), for the
    // overload build narrowing in `resolve_in_object`.
    build: &[(String, bool)],
    // Engine-switch S3.2 / S6.0: the member arm fills the interface (for an
    // `Interface`-typed receiver) and the receiver fact.
    facts_out: &mut SiteFacts,
    // Engine-switch S3.4: the file's source text, so a record operation's
    // `Validate` field argument can be read (`TriggerSiteRule`).
    text: &str,
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
                // A report dataitem trigger's implicit Rec (S9.0e).
                let report_rec_table = matches!(
                    obj_node.id.kind,
                    ObjectKind::Report | ObjectKind::ReportExtension
                )
                .then(|| {
                    crate::program::resolve::receiver::resolve_report_implicit_rec_table(
                        routine, obj_node, graph, index,
                    )
                })
                .flatten();
                resolve_bare_with_args(
                    obj_node,
                    &name_lc,
                    arity,
                    graph,
                    index,
                    surface,
                    with_state,
                    &args_info,
                    build,
                    report_rec_table.as_ref(),
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
                if let ReceiverType::Interface { name_lc } = &recv {
                    facts_out.interface = Some(name_lc.clone());
                }
                facts_out.receiver = Some(receiver_fact(
                    &recv,
                    &receiver_lc,
                    routine,
                    &obj.globals,
                    obj_node,
                    graph,
                    index,
                ));
                let (s, r) = resolve_member_with_args(
                    &recv, &method_lc, arity, obj_node, graph, index, surface, &args_info, build,
                );
                finding = builtin_dispatch_finding(&recv, &method_lc, &r, file, call_args);
                // An empty `Multicast` from an object receiver is a run of an
                // object that declares no entry trigger (`dispatch_entry_trigger`).
                if s == DispatchShape::Multicast
                    && r.is_empty()
                    && let ReceiverType::Object { kind, name_lc, id } = &recv
                {
                    facts_out.run_target =
                        crate::program::resolve::resolver::object_receiver_target(
                            kind,
                            name_lc,
                            id.as_ref(),
                            obj_node,
                            graph,
                        )
                        .map(|o| o.id.clone());
                }
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
                "XmlPort" => Some(ObjectKind::XmlPort),
                _ => None,
            };
            if let Some(okind) = okind_opt {
                let (shape, completeness, routes) = resolve_object_run(
                    caller_app,
                    okind,
                    target_ref.as_deref(),
                    *target_is_name,
                    graph,
                    index,
                    surface,
                );
                if shape == DispatchShape::Multicast
                    && routes.is_empty()
                    && let Some(t) = target_ref
                {
                    facts_out.run_target = crate::program::resolve::resolver::object_run_target(
                        caller_app,
                        okind,
                        t,
                        *target_is_name,
                        graph,
                        index,
                    )
                    .map(|o| o.id.clone());
                }
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
                let (shape, completeness, mut routes) =
                    resolve_implicit_trigger(&op_lc, table_node, graph, index, surface);
                // S3.4: emit only the triggers this site can fire (a literal
                // `RunTrigger = false` fires none; `Validate` fires its field's
                // `OnValidate` only). Non-routine routes (an honest unknown for a
                // collapse-marked trigger) are kept.
                let rule = crate::program::resolve::applicability::TriggerSiteRule::of(
                    &op_lc, call_args, file, text,
                );
                if rule.run_trigger == Some(false) {
                    facts_out.run_trigger_excluded = routes
                        .iter()
                        .filter_map(|r| match &r.target {
                            RouteTarget::Routine(id) => Some(id.clone()),
                            _ => None,
                        })
                        .collect();
                }
                routes.retain(|r| match &r.target {
                    RouteTarget::Routine(id) => rule.admits(id),
                    _ => true,
                });
                facts_out.unqualified_record_op = !callee_text.contains('.');
                (shape, completeness, routes)
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
    caller_app: AppRef,
    graph: &ProgramGraph,
    index: &ResolveIndex,
    surface: &DeclSurface,
    obj_node_map: &HashMap<ObjectNodeId, &ObjectNode>,
) -> FileResolution {
    let mut edges: Vec<ClassifiedEdge> = Vec::new();
    let mut flagged: Vec<FlaggedBuiltinDispatchSite> = Vec::new();
    let mut indeterminate: Vec<IndeterminateBuiltinDispatchSite> = Vec::new();
    let mut site_facts: Vec<(ObligationId, SiteFacts)> = Vec::new();
    let mut parenless: Vec<al_syntax::ir::ExprId> = Vec::new();

    for (obj_idx, obj) in pf.file.objects.iter().enumerate() {
        let obj_key = match obj.id {
            Some(n) => ObjKey::Id(n),
            None => ObjKey::Name(obj.name.fold_identifier().into()),
        };
        let obj_node_id = ObjectNodeId {
            app: caller_app,
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
            // Report dataitems and XmlPort table elements are record variables too
            // (S9.0e), so `Item.Modify(true)` on one is a record op with its trigger
            // edge. An object global of the same name shadows the element.
            // ponytail: a non-record LOCAL of the same name is not excluded here;
            // receiver inference still types it by the local.
            .chain(
                obj.dataitems
                    .iter()
                    .map(|(name, _)| name.fold_identifier())
                    .filter(|n| !obj.globals.iter().any(|g| g.name.fold_identifier() == *n)),
            )
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

            // A declared variable shadows a same-named procedure, so a bare
            // parens-less read of one is a variable read, never a call.
            let shadows: HashSet<String> = routine
                .params
                .iter()
                .map(|p| p.name.fold_identifier())
                .chain(routine.locals.iter().map(|v| v.name.fold_identifier()))
                .chain(routine.return_name.iter().map(|n| n.fold_identifier()))
                .chain(obj.globals.iter().map(|v| v.name.fold_identifier()))
                .collect();

            for site in &sites {
                if site.parenless
                    && let CalleeShape::Bare { name } = &site.shape
                    && shadows.contains(&name.fold_identifier())
                {
                    continue;
                }
                let fp = callee_fp(&site.callee_text);
                let obl_id = ObligationId::CallSite {
                    caller: caller.clone(),
                    span: site.span.clone(),
                    callee_fp: fp,
                };

                let mut facts = SiteFacts {
                    build_context: pf.file.ir.preproc_context(site.expr).to_vec(),
                    ..SiteFacts::default()
                };
                let (kind, shape, completeness, routes, finding) = resolve_call_site_obligation(
                    &site.shape,
                    site.arity,
                    &site.callee_text,
                    obj_node_opt,
                    routine,
                    obj,
                    caller_app,
                    graph,
                    index,
                    surface,
                    site.with_state,
                    &pf.file,
                    &site.args,
                    pf.file.ir.preproc_context(site.expr),
                    &mut facts,
                    &pf.text,
                );
                // A parens-less value read is a call only when it reaches a
                // routine; otherwise it is a variable, field or built-in read.
                // ponytail: a parens-less built-in (`Rec.Count`) is dropped too,
                // as the compiler's graph does; keep it if built-in reads matter.
                if site.parenless
                    && (kind != EdgeKind::Call
                        || routes.is_empty()
                        || !routes.iter().all(|r| {
                            matches!(
                                r.target,
                                RouteTarget::Routine(_) | RouteTarget::AbiSymbol { .. }
                            )
                        }))
                {
                    continue;
                }
                if site.parenless {
                    parenless.push(site.expr);
                }
                if facts != SiteFacts::default() {
                    site_facts.push((obl_id.clone(), facts));
                }

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
        site_facts,
        parenless,
    }
}

/// The [`ReceiverFact`] for a member call whose receiver typed as `recv`
/// (engine-switch S6.0). An object's name is the declaration's as written when
/// the receiver is a declared variable naming that object, else the folded name.
fn receiver_fact(
    recv: &ReceiverType,
    receiver_lc: &str,
    routine: &al_syntax::ir::RoutineDecl,
    object_globals: &[al_syntax::ir::VarDecl],
    from_object: &ObjectNode,
    graph: &ProgramGraph,
    index: &ResolveIndex,
) -> ReceiverFact {
    use crate::program::node_extract::ObjectRef;
    use crate::program::resolve::index::ObjectRefResolution;
    use crate::program::resolve::receiver::{
        ParsedType, classify_type_text, receiver_declared_type,
    };
    let declared = receiver_declared_type(receiver_lc, routine, object_globals);
    // The object's own name, by id or by a unique name in the caller's closure.
    let node_name =
        |id: Option<&ObjectNodeId>, kind: ObjectKind, name_lc: &str| -> Option<String> {
            let id = match id {
                Some(id) => id.clone(),
                None => match index.resolve_object_ref(
                    graph,
                    from_object.id.clone(),
                    kind,
                    &ObjectRef::Name {
                        raw: name_lc.to_string(),
                        normalized_lc: name_lc.to_string(),
                    },
                ) {
                    ObjectRefResolution::Unique(id) => id,
                    _ => return None,
                },
            };
            let i = graph.objects.binary_search_by(|o| o.id.cmp(&id)).ok()?;
            Some(graph.objects[i].name.to_string())
        };
    // AL spelling: quoted when the name is not a plain identifier.
    let al_name = |n: &str| {
        if n.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            n.to_string()
        } else {
            format!("\"{n}\"")
        }
    };
    let rendered = match recv {
        ReceiverType::Object { kind, name_lc, id } => {
            node_name(id.as_ref(), *kind, name_lc).map(|n| format!("{kind:?} {}", al_name(&n)))
        }
        ReceiverType::Record { table: Some(id) } => {
            node_name(Some(id), ObjectKind::Table, "").map(|n| format!("Record {}", al_name(&n)))
        }
        _ => None,
    };
    let ty = match recv {
        ReceiverType::Object { kind, name_lc, .. } => {
            let written = declared.and_then(|t| match classify_type_text(t) {
                ParsedType::Object {
                    kind: k,
                    object_ref: ObjectRef::Name { raw, .. },
                } if k == *kind => Some(raw),
                _ => None,
            });
            ReceiverFactType::Object {
                kind: *kind,
                name: written.unwrap_or_else(|| name_lc.clone()),
            }
        }
        ReceiverType::Record { table } => ReceiverFactType::Record {
            table: table.clone(),
        },
        _ => ReceiverFactType::Other,
    };
    ReceiverFact {
        type_text: declared
            .map(crate::program::body::scope::canonicalize_type_text)
            .or(rendered),
        ty,
    }
}

/// The object map `resolve_file_obligations` reads for files owned by `app`. Its
/// only lookups use ids built from the caller's app, so it holds that app's
/// objects only: any other app's would be dead weight, retained per root by the
/// idle updater.
pub fn app_object_map(graph: &ProgramGraph, app: AppRef) -> HashMap<ObjectNodeId, &ObjectNode> {
    graph
        .objects
        .iter()
        .filter(|o| o.id.app == app)
        .map(|o| (o.id.clone(), o))
        .collect()
}

/// Resolve all obligations and compute coverage.
///
/// This is the clean-room inner loop.  It does NOT call any L3 oracle.
/// Publishers are resolved via [`emit_event_flow_edges`]; all call-site
/// obligations are resolved via the shape-dispatch helpers.
///
/// `surface` is built by the caller (normally [`ProgramContext::decl_surface`]):
/// workspace decls over the dependency tier's frozen `dep_meta`.
fn resolve_full_program_from_parts(
    graph: &ProgramGraph,
    parsed: &[ParsedUnit],
    surface: &DeclSurface,
    primary_app_ref: AppRef,
    ws_file_set: &HashSet<String>,
) -> (
    Vec<ClassifiedEdge>,
    Coverage,
    BuiltinDispatchAudit,
    HashMap<ObligationId, SiteFacts>,
    ParenlessCalls,
) {
    let mut site_facts: HashMap<ObligationId, SiteFacts> = HashMap::new();
    // Quick ObjectNodeId → &ObjectNode lookup.
    let obj_node_map = app_object_map(graph, primary_app_ref);

    let index = ResolveIndex::build(graph);

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

    let file_results: Vec<(String, FileResolution)> =
        crate::big_stack::big_stack_pool().install(|| {
            files_to_resolve
                .par_iter()
                .map(|pf| {
                    let r = resolve_file_obligations(
                        pf,
                        primary_app_ref,
                        graph,
                        &index,
                        surface,
                        &obj_node_map,
                    );
                    (pf.virtual_path.clone(), r)
                })
                .collect()
        });

    let mut parenless_calls = ParenlessCalls::new();
    for (path, file_res) in file_results {
        if !file_res.parenless.is_empty() {
            parenless_calls.insert(
                (primary_app_ref, path),
                file_res.parenless.into_iter().collect(),
            );
        }
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
        site_facts.extend(file_res.site_facts);
    }

    // ── Phase 2: publisher event flow obligations (all apps) ──────────────────
    // emit_event_flow_edges processes ALL graph.routines (no app filter).
    // We must track obligation ids in the same pass so coverage holds.
    let event_edges = emit_event_flow_edges(graph, surface);
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

    (
        classified_edges,
        coverage,
        builtin_dispatch_audit,
        site_facts,
        parenless_calls,
    )
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
    let (edges, coverage, builtin_dispatch_audit, site_facts, parenless_calls) =
        resolve_full_program_from_parts(
            graph,
            parsed,
            &ctx.decl_surface(),
            primary_app_ref,
            ws_file_set,
        );

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
    let recovered_files = ctx.recovered_files();

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
        site_facts,
        parenless_calls,
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
    let (edges, _coverage, _builtin_dispatch_audit, _site_facts, _parenless) =
        resolve_full_program_from_parts(
            &ctx.graph,
            &ctx.parsed,
            &ctx.decl_surface(),
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
    /// The workspace unit alone (empty when the workspace has no source).
    /// Dependency trees, when the profile keeps them, live in the dependency
    /// tier: see [`ProgramContext::dep_bodies`].
    pub(crate) parsed: Vec<ParsedUnit>,
    pub(crate) primary_app_ref: AppRef,
    pub(crate) ws_file_set: HashSet<String>,
    /// The immutable dep layer `graph` was assembled from. Pre-T3-Task-8 this
    /// was built and immediately dropped inside `build_program_graph_from_parsed`
    /// (see [`assemble_program_graph`]'s doc); kept here so a caller that wants
    /// to REUSE it across rebuilds doesn't have to re-derive it a second time.
    pub(crate) dep_layer: DepLayer,
    /// What this build was asked to keep (spec §4).
    pub(crate) profile: BuildProfile,
}

impl ProgramContext {
    /// What this build was asked to keep.
    #[must_use]
    pub fn profile(&self) -> BuildProfile {
        self.profile
    }

    /// The app-set snapshot the graph was built from.
    #[must_use]
    pub fn snapshot(&self) -> &AppSetSnapshot {
        &self.snap
    }

    /// Whether `app` is a dependency the workspace app requires, directly or
    /// transitively (its declared closure, the workspace itself excluded). An app
    /// that depends ON the workspace (a test app in an ancestor `.alpackages`) is
    /// loaded too, but is not part of the workspace's world (engine-switch S7.4).
    #[must_use]
    pub fn is_required_dependency(&self, app: &crate::snapshot::AppId) -> bool {
        self.graph.apps.find(app).is_some_and(|r| {
            r != self.primary_app_ref
                && self
                    .graph
                    .topology
                    .closure(self.primary_app_ref)
                    .contains(&r)
        })
    }

    /// The assembled whole-program graph (shared-substrate consumers only).
    #[must_use]
    pub fn graph(&self) -> &ProgramGraph {
        &self.graph
    }

    /// The workspace's parsed unit (at most one). Dependency trees are in
    /// [`Self::dep_bodies`].
    #[must_use]
    pub fn parsed(&self) -> &[ParsedUnit] {
        &self.parsed
    }

    /// The dependency `ParsedUnit`s, in `snap.apps` order: `Some` exactly
    /// when the profile is `DependencyBodies::Keep` (a `Keep` build never
    /// shares a tier built without them — see `DepKey`).
    #[must_use]
    pub fn dep_bodies(&self) -> Option<&[ParsedUnit]> {
        self.dep_layer
            .dep_nodes
            .bodies
            .as_deref()
            .map(Vec::as_slice)
    }

    /// Every parsed unit in `snap.apps` order (workspace first, then the
    /// dependency bodies): what `parse_snapshot` would give. Needs `Keep`.
    ///
    /// # Panics
    /// When the profile does not keep dependency bodies — a reader of
    /// dependency trees must declare `Keep` (spec §4).
    #[must_use]
    pub fn all_units(&self) -> Vec<&ParsedUnit> {
        let deps = self
            .dep_bodies()
            .expect("reading dependency trees needs DependencyBodies::Keep");
        self.parsed.iter().chain(deps).collect()
    }

    /// The workspace unit of `parsed` (at most one: `snap.apps` is
    /// GUID-deduped upstream) — the only unit the local `DeclSurface` tier
    /// and the workspace recovered list read.
    fn workspace_unit(&self) -> &[ParsedUnit] {
        self.parsed
            .iter()
            .position(|u| u.app == self.snap.workspace_app)
            .map_or(&[][..], |i| std::slice::from_ref(&self.parsed[i]))
    }

    /// Workspace decls over the dependency tier's frozen `dep_meta`. The
    /// dependency `RoutineMeta` always comes from the tier, never from
    /// dependency `ParsedUnit`s.
    #[must_use]
    pub fn decl_surface(&self) -> DeclSurface {
        DeclSurface::build(&self.graph, self.workspace_unit())
            .with_frozen(Arc::clone(&self.dep_layer.dep_nodes.dep_meta))
    }

    /// The dependency target registry (engine-switch S2b.5): every dependency
    /// routine's declaration and body state. No dependency body is analysed yet.
    #[must_use]
    pub fn registry(&self) -> crate::program::registry::DependencyRegistry<'_> {
        crate::program::registry::DependencyRegistry::new(
            &self.graph,
            &self.dep_layer.dep_nodes.dep_meta,
        )
    }

    /// `"<app name>::<virtual path>"` of every `Recovered` source file,
    /// sorted: the dependency tier's list plus the workspace's own. See
    /// [`crate::snapshot::parse::recovered_file_paths`] for the invariant.
    #[must_use]
    pub fn recovered_files(&self) -> Vec<String> {
        let mut paths = self.dep_layer.dep_nodes.recovered.clone();
        paths.extend(crate::snapshot::parse::recovered_file_paths(
            self.workspace_unit(),
        ));
        paths.sort();
        paths
    }

    /// Owning-app body resolution (engine-switch S7.1): every call site in every
    /// source-bearing dependency body, resolved from the dependency's own view —
    /// its declared closure, its visibility, its friends — exactly as a workspace
    /// file is resolved from the workspace's. Separate from [`ProgramReport`]: the
    /// north-star histogram stays the workspace's call sites plus event flow.
    ///
    /// Units in `snap.apps` order, files sorted by virtual path, each file's
    /// edges in document order.
    ///
    /// # Panics
    /// When the profile does not keep dependency bodies (`FULL`).
    #[must_use]
    pub fn resolve_dependency_bodies(&self) -> DependencyBodyResolution {
        let bodies = self
            .dep_bodies()
            .expect("resolving dependency bodies needs DependencyBodies::Keep");
        let index = ResolveIndex::build(&self.graph);
        let surface = self.decl_surface();
        let mut files: Vec<(AppRef, &ParsedFile)> = Vec::new();
        for unit in bodies {
            let Some(app) = self.graph.apps.find(&unit.app) else {
                continue;
            };
            if app == self.primary_app_ref {
                continue;
            }
            let start = files.len();
            files.extend(unit.files.iter().map(|pf| (app, pf)));
            files[start..].sort_by(|a, b| a.1.virtual_path.cmp(&b.1.virtual_path));
        }
        let mut maps: HashMap<AppRef, HashMap<ObjectNodeId, &ObjectNode>> = HashMap::new();
        for (app, _) in &files {
            maps.entry(*app)
                .or_insert_with(|| app_object_map(&self.graph, *app));
        }
        let results: Vec<FileResolution> = crate::big_stack::big_stack_pool().install(|| {
            files
                .par_iter()
                .map(|(app, pf)| {
                    resolve_file_obligations(pf, *app, &self.graph, &index, &surface, &maps[app])
                })
                .collect()
        });
        let mut out = DependencyBodyResolution::default();
        for ((app, pf), r) in files.iter().zip(results) {
            out.edges.extend(r.edges);
            out.site_facts.extend(r.site_facts);
            if !r.parenless.is_empty() {
                out.parenless_calls.insert(
                    (*app, pf.virtual_path.clone()),
                    r.parenless.into_iter().collect(),
                );
            }
        }
        out
    }
}

/// What [`ProgramContext::resolve_dependency_bodies`] returns.
#[derive(Default)]
pub struct DependencyBodyResolution {
    /// One edge per dependency call site; `edge.from.object.app` is the owning app.
    pub edges: Vec<ClassifiedEdge>,
    /// The receiver/interface facts of those sites (see [`SiteFacts`]).
    pub site_facts: HashMap<ObligationId, SiteFacts>,
    /// See [`ParenlessCalls`]: the dependency files' parens-less calls.
    pub parenless_calls: ParenlessCalls,
}

pub fn build_context_res(workspace_root: &Path) -> Result<ProgramContext, String> {
    build_context_from_snapshot(build_snapshot_res(workspace_root)?, BuildProfile::FULL)
}

/// [`build_context`] with an explicit [`DependencySource`] — the LSP server
/// and CLI index path, which let the user trade dependency depth for memory.
///
/// When `dep_cache` hits, the dependencies are not parsed again; a `Keep`
/// profile still gets [`ProgramContext::dep_bodies`] from the shared tier.
///
/// Dependency text is extracted only when something will read it
/// (engine-switch S10.1b). The snapshot is first built without the text of
/// any source the cache already knows. If such a source is present, the build
/// continues only on a live shared tier whose LSP products are already
/// built: the hit skips the dependency parse and `LspSnapshot::from_context`
/// reuses those products, so nothing reads dependency text. The tier is held
/// until the build returns, so it cannot die in between. Otherwise the
/// snapshot is built again with the text.
#[must_use]
pub fn build_context_with(
    workspace_root: &Path,
    dependency_source: DependencySource,
    profile: BuildProfile,
    dep_cache: &DepCache,
) -> Option<ProgramContext> {
    let builder = SnapshotBuilder {
        workspace_root: workspace_root.to_path_buf(),
        // Local providers are NOT part of `DepKey`: a caller that sets them
        // must extend the key (provenance tier + content hash), or a local
        // checkout and the embedded source of the same `.app` would share.
        local_providers: vec![],
    };
    let (snap, _dropped) = builder
        .build_deferring_dependency_text(dependency_source, dep_cache)
        .ok()?;
    let mut held = None;
    let snap = if snap.has_deferred_dependency_text() {
        held = dep_cache
            .get(&DepKey::of(&snap, profile))
            .filter(|tier| tier.lsp.get().is_some());
        if held.is_some() {
            snap
        } else {
            drop(snap);
            builder
                .build_with_options(dependency_source, dep_cache)
                .ok()?
                .0
        }
    } else {
        snap
    };
    crate::census_hook::mark("1.snapshot");
    let ctx = build_context_from_snapshot_cached(snap, profile, dep_cache).ok();
    drop(held);
    ctx
}

/// Step 1 of [`build_context_res`], split out so a caller can inspect the
/// snapshot BEFORE paying for parse + resolve.
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
/// graph, and locate the primary app. Split from [`build_snapshot_res`] so a
/// caller can hold the snapshot first — behaviour is unchanged.
pub fn build_context_from_snapshot(
    snap: AppSetSnapshot,
    profile: BuildProfile,
) -> Result<ProgramContext, String> {
    build_context_from_snapshot_cached(snap, profile, &DepCache::default())
}

/// [`build_context_from_snapshot`], sharing the dependency tier through
/// `dep_cache` with every other root that loads the same dependencies.
pub fn build_context_from_snapshot_cached(
    snap: AppSetSnapshot,
    profile: BuildProfile,
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
    // Each dependency file is summarized as it is parsed; under `Summary` its
    // tree dies right there, under `Keep` the trees go into the tier. A live
    // shared tier carries everything resolution reads from the dependencies
    // (`dep_meta`, `recovered`, and the bodies when the key keeps them), so on
    // a hit only the workspace is parsed. `parsed` is the workspace unit
    // alone, always.
    let shared_tier = dep_cache.get(&DepKey::of(&snap, profile));
    let mut parse = {
        let _s = pt::span("preflight", "preflight.parse_snapshot");
        parse_for_build(&snap, profile, shared_tier.is_some())
    };
    let parsed: Vec<ParsedUnit> = parse.workspace.take().into_iter().collect();
    crate::census_hook::mark("2.parse");
    let dep_layer = {
        let _s = pt::span("preflight", "preflight.dep_layer");
        build_dep_layer_cached(
            &snap,
            &crate::program::abi_ingest::AbiCache::new(),
            DepInput::Built(parse),
            profile,
            dep_cache,
        )
    };
    // `shared_tier` kept the entry alive, so the layer is built on it and the
    // dependency nodes never came from the dependency-less parse.
    assert!(
        shared_tier
            .as_ref()
            .is_none_or(|t| Arc::ptr_eq(t, &dep_layer.dep_nodes)),
        "dependency layer was not built on the held shared entry; \
         dependency nodes would come from a workspace-only parse"
    );
    drop(shared_tier);
    crate::census_hook::mark("3.dep_layer");

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
    crate::census_hook::mark("4.assemble_graph");

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
        profile,
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
#[derive(Debug, Clone, PartialEq, Eq)]
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
    /// Every dependency of the primary app's reachable declared closure, as
    /// declared and as found (engine-switch S5.2, spec G14). Sorted by guid.
    pub ledger: Vec<LedgerEntry>,
    /// Dependency packages whose manifest could not be read, so their identity
    /// (and whether the primary needs them) is unknown: `"{path}: {error}"`,
    /// sorted.
    pub unidentified_packages: Vec<String>,
    /// The primary app's `[EventSubscriber]`s that bound to no publisher
    /// (ambiguous, orphaned, or publisher object not found; engine-switch
    /// S5.2b). Such a subscriber's dispatch is unknown, yet it is no call edge,
    /// so `unknown` does not count it.
    pub unbound_subscriptions: usize,
}

/// One dependency in the primary app's reachable declared closure: what an app
/// asked for, and what the snapshot holds for it (engine-switch S5.2, G14).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LedgerEntry {
    /// Lowercase app guid.
    pub guid: String,
    /// The name the declaration gives.
    pub name: String,
    /// The app that declared it (first in breadth-first order from the primary).
    pub declared_by: String,
    /// The declared minimum version (an AL dependency version is a minimum).
    pub declared_version: String,
    /// A Microsoft Application/Platform-tier app. For the primary app these
    /// come from app.json's `application`/`platform` fields, added by
    /// `dependencies::append_implicit_ms_tier_deps`, and may legitimately be
    /// absent (e.g. "Business Foundation" before BC 25).
    pub ms_tier: bool,
    /// The app found in the snapshot, `None` when it is missing.
    pub found: Option<LedgerApp>,
    /// Not found because its package is on disk but could not be read (the
    /// loader's error). `None` when found, or when no package was there.
    pub unreadable: Option<String>,
}

/// What the snapshot holds for a [`LedgerEntry`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LedgerApp {
    pub version: String,
    /// The trust tier of its source (`SymbolOnly` when it has none).
    pub tier: crate::snapshot::TrustTier,
    /// Objects its `SymbolReference.json` declares (`None`: no ABI parsed).
    pub abi_objects: Option<usize>,
    /// Its `SymbolReference.json` could not be read or parsed
    /// (`ProgramGraph::abi_ingest_errors`); the app then looks empty.
    pub ingest_error: Option<String>,
}

impl LedgerEntry {
    /// The found version is below the declared minimum.
    pub fn below_declared_version(&self) -> bool {
        let parse = |v: &str| -> Vec<u64> {
            v.split('.')
                .map(|p| p.trim().parse().unwrap_or(0))
                .collect()
        };
        self.found
            .as_ref()
            .is_some_and(|f| parse(&f.version) < parse(&self.declared_version))
    }
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

/// The preflight's program build: the context (`Summary` dependency
/// profile) and its one resolve. [`build_program_with_coverage`] reduces it
/// with [`reduce_fresh_coverage`] and keeps it for the B3 adapter.
pub fn fresh_program_from_snapshot(
    snap: AppSetSnapshot,
) -> Result<(ProgramContext, ProgramReport), String> {
    // Reads no dependency body; it DOES read edge details, which a later step
    // will declare in the profile (spec §4).
    fresh_program_from_snapshot_with(
        snap,
        BuildProfile {
            dependency_bodies: DependencyBodies::Summary,
        },
    )
}

/// [`fresh_program_from_snapshot`] under a stated profile: `FULL` for a reader of
/// dependency bodies (the cross-app model, engine-switch S7.3).
pub fn fresh_program_from_snapshot_with(
    snap: AppSetSnapshot,
    profile: BuildProfile,
) -> Result<(ProgramContext, ProgramReport), String> {
    let ctx = build_context_from_snapshot(snap, profile)?;
    let report = {
        let _s = pt::span("preflight", "preflight.resolve_full");
        resolve_full_program_with(&ctx)
    };
    Ok((ctx, report))
}

/// The one program build the detectors' call resolution starts from: snapshot,
/// context + report, and the preflight status reduced from them. `alsem
/// analyze` and the r4/r4f test helper both use it so the two cannot drift.
///
/// There is no verdict cache: the caller needs the context and report, which a
/// cached verdict cannot give (the B3 removal of the preflight cache, see
/// CHANGELOG).
pub fn build_program_with_coverage(
    workspace_root: &Path,
) -> Result<(ProgramContext, ProgramReport, FreshCoverage), String> {
    build_program_with_coverage_profiled(
        workspace_root,
        BuildProfile {
            dependency_bodies: DependencyBodies::Summary,
        },
    )
}

/// [`build_program_with_coverage`] under a stated profile (engine-switch S7.3: the
/// cross-app model reads dependency bodies, so it builds `FULL`).
pub fn build_program_with_coverage_profiled(
    workspace_root: &Path,
    profile: BuildProfile,
) -> Result<(ProgramContext, ProgramReport, FreshCoverage), String> {
    let (snap, load) = {
        let _s = pt::span("preflight", "preflight.snapshot_build");
        (SnapshotBuilder {
            workspace_root: workspace_root.to_path_buf(),
            local_providers: vec![],
        })
        .build_with_diagnostics()
        .map_err(|e| format!("snapshot build failed: {e:#}"))?
    };
    let (ctx, report) = fresh_program_from_snapshot_with(snap, profile)?;
    let fc = reduce_fresh_coverage(&ctx, &report, &load.unreadable);
    Ok((ctx, report, fc))
}

/// Reduce a [`fresh_program_from_snapshot`] result to the preflight status.
/// `unreadable` is the dependency loader's list of packages it could not read
/// (`DependencyLoadReport::unreadable`).
pub fn reduce_fresh_coverage(
    ctx: &ProgramContext,
    report: &ProgramReport,
    unreadable: &[crate::dependencies::UnreadableDependency],
) -> FreshCoverage {
    let opaque_apps = {
        let _s = pt::span("preflight", "preflight.opaque_closure");
        opaque_dependency_closure(&ctx.snap)
    };
    let ledger = {
        let _s = pt::span("preflight", "preflight.ledger");
        dependency_ledger(&ctx.snap, &ctx.graph, unreadable)
    };
    let mut unidentified_packages: Vec<String> = unreadable
        .iter()
        .filter(|u| u.guid.is_none())
        .map(|u| format!("{}: {}", u.path.display(), u.error))
        .collect();
    unidentified_packages.sort();
    let unbound_subscriptions = {
        use crate::program::resolve::index::{SubscriberIndex, SubscriptionOutcome};
        SubscriberIndex::build(&ctx.graph)
            .subscriptions()
            .iter()
            .filter(|s| {
                s.subscriber.object.app == ctx.primary_app_ref
                    && !matches!(s.outcome, SubscriptionOutcome::Bound(_))
            })
            .count()
    };
    FreshCoverage {
        unbound_subscriptions,
        unknown: report.primary_histogram.unknown,
        coverage_holds: coverage_holds(&report.coverage),
        recovered_files: report.recovered_files.len(),
        opaque_apps,
        ledger,
        unidentified_packages,
    }
}

/// The primary app's reachable declared dependencies, as declared and as found
/// (engine-switch S5.2, G14): the same breadth-first walk as
/// [`opaque_dependency_closure`], keeping every declaration, including one whose
/// app is missing from the snapshot. A missing app's own dependencies are unknown,
/// so the walk does not continue through it.
fn dependency_ledger(
    snap: &AppSetSnapshot,
    graph: &ProgramGraph,
    unreadable: &[crate::dependencies::UnreadableDependency],
) -> Vec<LedgerEntry> {
    let unreadable_by_guid: HashMap<String, &str> = unreadable
        .iter()
        .filter_map(|u| Some((u.guid.as_ref()?.to_ascii_lowercase(), u.error.as_str())))
        .collect();
    use std::collections::{HashMap, VecDeque};
    let by_guid: HashMap<String, &AppUnit> = snap
        .apps
        .iter()
        .map(|u| (u.id.guid.to_ascii_lowercase(), u))
        .collect();
    let errors: HashMap<String, &str> = graph
        .abi_ingest_errors
        .iter()
        .map(|e| {
            (
                graph.apps.resolve(e.app).guid.to_ascii_lowercase(),
                e.message.as_str(),
            )
        })
        .collect();
    let ms_tier: HashSet<String> = crate::dependencies::MS_APPLICATION_TIER
        .iter()
        .chain(crate::dependencies::MS_PLATFORM_TIER)
        .map(|(g, _)| g.to_ascii_lowercase())
        .collect();
    let primary_guid = snap.workspace_app.guid.to_ascii_lowercase();
    let mut entries: HashMap<String, LedgerEntry> = HashMap::new();
    let mut queue: VecDeque<&AppUnit> = VecDeque::new();
    if let Some(primary) = by_guid.get(&primary_guid) {
        queue.push_back(primary);
    }
    while let Some(unit) = queue.pop_front() {
        for dep in &unit.declared_deps {
            let guid = dep.app_id.to_ascii_lowercase();
            if guid == primary_guid || entries.contains_key(&guid) {
                continue;
            }
            let found = by_guid.get(&guid).map(|u| {
                queue.push_back(u);
                LedgerApp {
                    version: u.id.version.clone(),
                    tier: u
                        .source
                        .as_ref()
                        .map_or(crate::snapshot::TrustTier::SymbolOnly, |s| s.tier),
                    abi_objects: u.abi.as_ref().map(|a| a.objects.len()),
                    ingest_error: errors.get(&guid).map(|m| m.to_string()),
                }
            });
            entries.insert(
                guid.clone(),
                LedgerEntry {
                    unreadable: if found.is_none() {
                        unreadable_by_guid.get(&guid).map(|m| m.to_string())
                    } else {
                        None
                    },
                    ms_tier: ms_tier.contains(&guid),
                    guid,
                    name: dep.name.clone(),
                    declared_by: unit.id.name.clone(),
                    declared_version: dep.version.clone(),
                    found,
                },
            );
        }
    }
    let mut ledger: Vec<LedgerEntry> = entries.into_values().collect();
    ledger.sort_by(|a, b| a.guid.cmp(&b.guid));
    ledger
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
    use crate::snapshot::parse_snapshot;

    fn rid(name: &str) -> RoutineNodeId {
        RoutineNodeId {
            object: ObjectNodeId {
                app: AppRef(0),
                kind: ObjectKind::Codeunit,
                key: ObjKey::Id(1),
            },
            name_lc: name.into(),
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

    /// S9.0c (found by the compiler oracle): a zero-argument call may drop its
    /// `()`, also inside an expression. A bare `M` or `X.M` read as a value is a
    /// call when it reaches a routine; a variable, a field or a shadowing local
    /// of the same name is not.
    #[test]
    fn parenless_call_in_an_expression_is_a_call_site() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_minimal_workspace(dir.path());
        std::fs::write(
            dir.path().join("C.al"),
            "table 50001 R
{
    fields
    {
        field(1; Code; Code[20]) { }
    }
}
             codeunit 50002 D
{
    procedure Flag(): Boolean
    begin
        exit(true);
    end;
}
             codeunit 50000 C
{
    procedure IsOn(): Boolean
    begin
        exit(true);
    end;

             procedure Caller()
    var
        Other: Codeunit D;
        T: Record R;
        B: Boolean;
    begin
             if IsOn then;
             if not Other.Flag then;
             B := IsOn;
             if T.Code = '' then;
             if B then;
             exit;
    end;

             procedure Shadowed()
    var
        IsOn: Boolean;
    begin
             if IsOn then;
    end;
}
",
        )
        .expect("write C.al");

        let report = resolve_full_program(dir.path()).expect("resolve_full_program");
        let calls = |caller: &str| -> Vec<(u32, String)> {
            let mut v: Vec<(u32, String)> = report
                .edges
                .iter()
                .filter(|ce| ce.edge.from.name_lc == caller && ce.edge.kind == EdgeKind::Call)
                .flat_map(|ce| {
                    ce.edge.routes.iter().map(move |r| match &r.target {
                        RouteTarget::Routine(id) => {
                            (ce.edge.site.span.start.line, id.name_lc.to_string())
                        }
                        other => (ce.edge.site.span.start.line, format!("{other:?}")),
                    })
                })
                .collect();
            v.sort();
            v
        };
        // Lines are 0-based: `if IsOn then;` is line 27.
        assert_eq!(
            calls("caller"),
            vec![
                (27, "ison".to_string()),
                (28, "flag".to_string()),
                (29, "ison".to_string()),
            ],
            "IsOn / Other.Flag read as values are calls; the field T.Code and the variable B are not"
        );
        assert_eq!(
            calls("shadowed"),
            vec![],
            "a local named IsOn shadows the procedure"
        );
    }

    /// S9.0c (found by the compiler oracle): a call inside a ternary, an `in` list
    /// or an `is`/`as` operand is a call site. The lowerer used to make those
    /// containers an opaque `Unknown`, so the calls had no edge.
    #[test]
    fn calls_inside_ternary_list_and_typeop_are_call_sites() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_minimal_workspace(dir.path());
        std::fs::write(
            dir.path().join("C.al"),
            "codeunit 50000 C
{
             procedure F(): Boolean
    begin
    end;
             procedure A(): Integer
    begin
    end;
             procedure B(): Integer
    begin
    end;
             procedure G(): Codeunit C
    begin
    end;
             procedure Caller()
    var
        X: Integer;
        O: Codeunit C;
    begin
             X := F() ? A() : B();
             if X in [A(), B()] then;
             if G() is C then;
             O := G() as C;
    end;
}
",
        )
        .expect("write C.al");

        let report = resolve_full_program(dir.path()).expect("resolve_full_program");
        let mut calls: Vec<(u32, String)> = report
            .edges
            .iter()
            .filter(|ce| ce.edge.from.name_lc == "caller" && ce.edge.kind == EdgeKind::Call)
            .flat_map(|ce| {
                ce.edge.routes.iter().map(move |r| match &r.target {
                    RouteTarget::Routine(id) => {
                        (ce.edge.site.span.start.line, id.name_lc.to_string())
                    }
                    other => (ce.edge.site.span.start.line, format!("{other:?}")),
                })
            })
            .collect();
        calls.sort();
        let want: Vec<(u32, String)> = [
            (19, "a"),
            (19, "b"),
            (19, "f"),
            (20, "a"),
            (20, "b"),
            (21, "g"),
            (22, "g"),
        ]
        .into_iter()
        .map(|(l, n)| (l, n.to_string()))
        .collect();
        assert_eq!(calls, want);
    }

    /// S9.0e: a member call on a `DotNet` receiver is a .NET interop leaf, a
    /// catalog route `DotNet::<alias>::<member>`, not an unknown (`catalogMiss`,
    /// 3,331 sites in CDO's dependency bodies).
    #[test]
    fn dotnet_member_call_is_a_catalog_leaf() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_minimal_workspace(dir.path());
        std::fs::write(
            dir.path().join("C.al"),
            "codeunit 50000 C
{
    procedure P()
    var
        Enc: DotNet \"Encoding\";
        B: DotNet Array;
    begin
        B := Enc.GetBytes('a');
    end;
}
",
        )
        .expect("write C.al");
        let report = resolve_full_program(dir.path()).expect("resolve_full_program");
        let routes: Vec<(String, EvidenceKind)> = report
            .edges
            .iter()
            .filter(|ce| ce.edge.from.name_lc == "p")
            .flat_map(|ce| ce.edge.routes.iter())
            .map(|r| (format!("{:?}", r.target), r.evidence.kind()))
            .collect();
        assert_eq!(
            routes,
            vec![(
                "Builtin(BuiltinId(\"DotNet::encoding::getbytes\"))".to_string(),
                EvidenceKind::Catalog
            )]
        );
    }

    /// S9.0e: inside a report dataitem trigger, a bare call falls back to the
    /// dataitem table's procedures (`if not IsInventoriableItem() then` in Base
    /// Application's Get Demand To Reserve; reportRecExcluded before). A report
    /// procedure has no implicit Rec, so the same call there stays unresolved.
    #[test]
    fn report_dataitem_trigger_bare_call_reaches_the_dataitem_table() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_minimal_workspace(dir.path());
        std::fs::write(
            dir.path().join("C.al"),
            "table 50001 T\n{\n    fields\n    {\n        field(1; Code; Code[20]) { }\n    }\n\n\
             procedure IsSpecial(): Boolean\n    begin\n    end;\n}\n\
             report 50003 R\n{\n    dataset\n    {\n        dataitem(D; T)\n        {\n\
             trigger OnAfterGetRecord()\n            begin\n                if IsSpecial() then;\n            end;\n        }\n    }\n\n\
             procedure Helper()\n    begin\n        if IsSpecial() then;\n    end;\n}\n",
        )
        .expect("write C.al");
        let report = resolve_full_program(dir.path()).expect("resolve_full_program");
        let routes = |caller: &str| -> Vec<String> {
            report
                .edges
                .iter()
                .filter(|ce| ce.edge.from.name_lc == caller)
                .flat_map(|ce| ce.edge.routes.iter())
                .map(|r| match &r.target {
                    RouteTarget::Routine(id) => id.name_lc.to_string(),
                    _ => format!("{:?}", r.evidence),
                })
                .collect()
        };
        assert_eq!(routes("onaftergetrecord"), vec!["isspecial"]);
        assert_eq!(routes("helper"), vec!["Unknown(ReportRecExcluded)"]);
    }

    /// S9.0e: inside a report dataitem trigger, a bare field receiver is the
    /// dataitem record's field (`"Item Ledger Entry Type".AsInteger()` in Base
    /// Application's Item Register - Value). A report procedure of the same name
    /// shadows it (parens-optional call), so that case declines.
    #[test]
    fn report_dataitem_bare_field_receiver_types_by_the_dataitem_table() {
        let src = |shadow: &str| {
            format!(
                "enum 50002 S
{{
    value(0; A) {{ }}
}}
                 table 50001 T
{{
    fields
    {{
        field(1; \"My Status\"; Enum S) {{ }}
    }}
}}
                 report 50003 R
{{
    dataset
    {{
        dataitem(D; T)
        {{
                 trigger OnAfterGetRecord()
            var
                I: Integer;
            begin
                 I := \"My Status\".AsInteger();
            end;
        }}
    }}
{shadow}}}
"
            )
        };
        let routes = |text: String| -> Vec<EvidenceKind> {
            let dir = tempfile::tempdir().expect("tempdir");
            write_minimal_workspace(dir.path());
            std::fs::write(dir.path().join("C.al"), text).expect("write C.al");
            let report = resolve_full_program(dir.path()).expect("resolve_full_program");
            report
                .edges
                .iter()
                .filter(|ce| ce.edge.from.name_lc == "onaftergetrecord")
                .flat_map(|ce| ce.edge.routes.iter().map(|r| r.evidence.kind()))
                .collect()
        };
        assert_eq!(routes(src("")), vec![EvidenceKind::Catalog]);
        assert_eq!(
            routes(src("    procedure \"My Status\"(): Integer
    begin
    end;
")),
            vec![EvidenceKind::Unknown],
            "a same-named report procedure shadows the field"
        );
    }

    /// S9.0e: an XmlPort `tableelement(Name; Table)` name is a record variable
    /// across the XmlPort (untrackedReceiver before), and on it, as on a report
    /// dataitem name, `Modify(true)` is a record op that fires the table trigger.
    #[test]
    fn xmlport_table_element_and_report_dataitem_are_record_variables() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_minimal_workspace(dir.path());
        std::fs::write(
            dir.path().join("C.al"),
            "table 50001 T
{
    fields
    {
        field(1; Code; Code[20]) { }
    }
             trigger OnModify()
    begin
    end;
}
             xmlport 50002 X
{
    schema
    {
        textelement(Root)
        {
             tableelement(Elem; T)
            {
                trigger OnAfterGetRecord()
                begin
             Elem.Modify(true);
                    Elem.FieldCaption(Code);
                end;
            }
        }
    }
}
             report 50003 R
{
    dataset
    {
        dataitem(Di; T)
        {
             trigger OnPreDataItem()
            begin
                Di.Modify(true);
            end;
        }
    }
}
",
        )
        .expect("write C.al");
        let report = resolve_full_program(dir.path()).expect("resolve_full_program");
        let mut got: Vec<(String, EdgeKind, String)> = report
            .edges
            .iter()
            .filter(|ce| {
                matches!(
                    ce.edge.from.name_lc.as_str(),
                    "onaftergetrecord" | "onpredataitem"
                )
            })
            .flat_map(|ce| {
                ce.edge.routes.iter().map(move |r| {
                    let to = match &r.target {
                        RouteTarget::Routine(id) => id.name_lc.to_string(),
                        _ => format!("{:?}", r.evidence.kind()),
                    };
                    (ce.edge.from.name_lc.to_string(), ce.edge.kind, to)
                })
            })
            .collect();
        got.sort_by(|a, b| (&a.0, &a.2).cmp(&(&b.0, &b.2)));
        assert!(
            got.iter().all(|(_, _, to)| to != "Unknown"),
            "no unknown route: {got:?}"
        );
        let triggers: Vec<&str> = got
            .iter()
            .filter(|(_, k, _)| *k == EdgeKind::ImplicitTrigger)
            .map(|(from, _, to)| {
                assert_eq!(to, "onmodify");
                from.as_str()
            })
            .collect();
        assert_eq!(
            triggers,
            vec!["onaftergetrecord", "onpredataitem"],
            "{got:?}"
        );
    }

    /// S9.0e: XmlPort calls (untrackedReceiver / memberNotFound before):
    /// `currXMLport.Skip()` is an XmlPort instance builtin; `XmlPort.Run`/`Export`
    /// with a static id and a variable's `Import()` run the XmlPort's own
    /// `OnPreXmlPort`.
    #[test]
    fn xmlport_calls_resolve() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_minimal_workspace(dir.path());
        std::fs::write(
            dir.path().join("C.al"),
            "xmlport 50002 X
{
    schema
    {
        textelement(Root)
        {
             trigger OnBeforePassVariable()
            begin
                currXMLport.Skip();
            end;
        }
    }
             trigger OnPreXmlPort()
    begin
    end;
}
             codeunit 50000 C
{
    procedure P()
    var
        V: XmlPort X;
        OutS: OutStream;
    begin
             XmlPort.Run(XmlPort::X);
        Xmlport.Export(Xmlport::X, OutS);
        V.Import();
    end;
}
",
        )
        .expect("write C.al");
        let report = resolve_full_program(dir.path()).expect("resolve_full_program");
        let mut got: Vec<(String, String)> = report
            .edges
            .iter()
            .filter(|ce| matches!(ce.edge.from.name_lc.as_str(), "p" | "onbeforepassvariable"))
            .flat_map(|ce| {
                ce.edge.routes.iter().map(move |r| {
                    let to = match &r.target {
                        RouteTarget::Routine(id) => id.name_lc.to_string(),
                        RouteTarget::Builtin(b) => b.0.clone(),
                        _ => format!("{:?}", r.evidence.kind()),
                    };
                    (ce.edge.from.name_lc.to_string(), to)
                })
            })
            .collect();
        got.sort();
        let want: Vec<(String, String)> = [
            ("onbeforepassvariable", "XmlPortInstance::skip"),
            ("p", "onprexmlport"),
            ("p", "onprexmlport"),
            ("p", "onprexmlport"),
        ]
        .into_iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect();
        assert_eq!(got, want);
    }

    /// S9.0e: `X[i].M()` on a declared `array[..] of T` types the receiver as `T`
    /// (untrackedReceiver before; 368 sites in CDO's dependency bodies).
    #[test]
    fn array_element_receiver_types_by_the_element_type() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_minimal_workspace(dir.path());
        std::fs::write(
            dir.path().join("C.al"),
            "codeunit 50002 D
{
    procedure Flag()
    begin
    end;
}
             codeunit 50000 C
{
    var
        G: array[2, 3] of Codeunit D;
             procedure P()
    var
        M: array[2] of Codeunit D;
        T: array[2] of Text[30];
    begin
             M[1].Flag();
        G[1, 2].Flag();
        T[1].ToUpper();
    end;
}
",
        )
        .expect("write C.al");
        let report = resolve_full_program(dir.path()).expect("resolve_full_program");
        let mut got: Vec<String> = report
            .edges
            .iter()
            .filter(|ce| ce.edge.from.name_lc == "p")
            .flat_map(|ce| ce.edge.routes.iter())
            .map(|r| match &r.target {
                RouteTarget::Routine(id) => id.name_lc.to_string(),
                RouteTarget::Builtin(b) => b.0.clone(),
                _ => format!("{:?}", r.evidence.kind()),
            })
            .collect();
        got.sort();
        assert_eq!(got, vec!["Text::toupper", "flag", "flag"]);
    }

    /// S9.0e: an extension reads its base object's `protected var` globals
    /// (untrackedReceiver before; e.g. Base Application's
    /// `AsmRequisitionLine.TableExt.al` on `Requisition Line`'s `Item`). A plain
    /// `var` of the base stays invisible.
    #[test]
    fn extension_reads_the_base_objects_protected_vars() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_minimal_workspace(dir.path());
        std::fs::write(
            dir.path().join("C.al"),
            "codeunit 50002 Mgt
{
    procedure Calc()
    begin
    end;
}
             table 50001 T
{
    fields
    {
        field(1; Code; Code[20]) { }
    }
             var
        Hidden: Codeunit Mgt;

    protected var
        Other: Record T;
        M: Codeunit Mgt;
}
             tableextension 50003 TX extends T
{
    procedure P()
    begin
             Other.Get('x');
        M.Calc();
        Hidden.Calc();
    end;
}
",
        )
        .expect("write C.al");
        let report = resolve_full_program(dir.path()).expect("resolve_full_program");
        let mut got: Vec<(u32, String)> = report
            .edges
            .iter()
            .filter(|ce| ce.edge.from.name_lc == "p")
            .flat_map(|ce| {
                ce.edge.routes.iter().map(move |r| {
                    let to = match &r.target {
                        RouteTarget::Routine(id) => id.name_lc.to_string(),
                        RouteTarget::Builtin(b) => b.0.clone(),
                        _ => format!("{:?}", r.evidence.kind()),
                    };
                    (ce.edge.site.span.start.line, to)
                })
            })
            .collect();
        got.sort();
        let to: Vec<&str> = got.iter().map(|(_, t)| t.as_str()).collect();
        assert_eq!(to, vec!["Record::get", "calc", "Unknown"], "{got:?}");
    }

    /// S9.0e: a nested bare name in a receiver chain reaches the implicit-`Rec`
    /// field step: in a table, `"Account Type"::Customer.AsInteger()` types
    /// `"Account Type"` as the enum field (no Enum object has that name), so the
    /// literal is an enum value (compoundReceiver before).
    #[test]
    fn enum_field_literal_receiver_types_through_the_implicit_rec() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_minimal_workspace(dir.path());
        std::fs::write(
            dir.path().join("C.al"),
            "enum 50001 \"Gen. Account Kind\"\n{\n    value(0; Customer) { }\n}\n\
             table 50002 T\n{\n    fields\n    {\n        field(1; \"Account Type\"; Enum \"Gen. Account Kind\") { }\n    }\n\n\
             procedure P(): Integer\n    begin\n        exit(\"Account Type\"::Customer.AsInteger());\n    end;\n}\n",
        )
        .expect("write C.al");
        let report = resolve_full_program(dir.path()).expect("resolve_full_program");
        let got: Vec<String> = report
            .edges
            .iter()
            .filter(|ce| ce.edge.from.name_lc == "p")
            .flat_map(|ce| ce.edge.routes.iter())
            .map(|r| match &r.target {
                RouteTarget::Builtin(b) => b.0.clone(),
                _ => format!("{:?}", r.evidence.kind()),
            })
            .collect();
        assert_eq!(got, vec!["Enum::asinteger"]);
    }

    /// S9.0e compound receivers: `this.Func().M()` types by `Func`'s return; any
    /// member of a .NET value is a .NET leaf; `"Type"::Value.AsInteger()` is an
    /// enum value when `"Type"` is a unique Enum (compoundReceiver before).
    #[test]
    fn compound_this_dotnet_and_enum_literal_receivers() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_minimal_workspace(dir.path());
        std::fs::write(
            dir.path().join("C.al"),
            "enum 50001 \"My Kind\"
{
    value(0; A) { }
    value(1; \"Big B\") { }
}
             codeunit 50002 Svc
{
    procedure Ping()
    begin
    end;
}
             codeunit 50000 C
{
    procedure Service(): Codeunit Svc
    begin
    end;

             procedure P()
    var
        Enc: DotNet Encoding;
        I: Integer;
    begin
             this.Service().Ping();
        Enc.UTF8.GetBytes('a');
        I := \"My Kind\"::\"Big B\".AsInteger();
    end;
}
",
        )
        .expect("write C.al");
        let report = resolve_full_program(dir.path()).expect("resolve_full_program");
        let mut got: Vec<(u32, String)> = report
            .edges
            .iter()
            .filter(|ce| ce.edge.from.name_lc == "p")
            .flat_map(|ce| {
                ce.edge.routes.iter().map(move |r| {
                    let to = match &r.target {
                        RouteTarget::Routine(id) => id.name_lc.to_string(),
                        RouteTarget::Builtin(b) => b.0.clone(),
                        _ => format!("{:?}", r.evidence.kind()),
                    };
                    (ce.edge.site.span.start.line, to)
                })
            })
            .collect();
        got.sort();
        let to: Vec<&str> = got.iter().map(|(_, t)| t.as_str()).collect();
        assert_eq!(
            to,
            vec!["ping", "service", "DotNet::*::getbytes", "Enum::asinteger"],
            "{got:?}"
        );
    }

    /// S9.0e chain sources: a built-in function result (`Format(..)`), record
    /// platform methods and system fields, `RecordId.GetRecord()`,
    /// `FieldRef.Record()`, `TextBuilder.ToText()` and Text methods
    /// (compoundReceiver before).
    #[test]
    fn builtin_and_record_chain_receivers_resolve() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_minimal_workspace(dir.path());
        std::fs::write(
            dir.path().join("C.al"),
            "table 50001 T\n{\n    fields\n    {\n        field(1; Code; Code[20]) { }\n    }\n}\n\
             codeunit 50000 C\n{\n    procedure P()\n    var\n        R: Record T;\n        FR: FieldRef;\n        EI: ErrorInfo;\n        TB: TextBuilder;\n        X: Text;\n        I: Integer;\n    begin\n\
             X := Format(I).Trim();\n\
             X := R.Count().ToText();\n\
             X := R.SystemCreatedAt.ToText();\n\
             I := R.RecordId.GetRecord().Number();\n\
             I := FR.Record().Number();\n\
             I := EI.RecordId.GetRecord().Number();\n\
             X := TB.ToText().TrimEnd('|').ToLower();\n    end;\n}\n",
        )
        .expect("write C.al");
        let report = resolve_full_program(dir.path()).expect("resolve_full_program");
        let unknown: Vec<String> = report
            .edges
            .iter()
            .filter(|ce| ce.edge.from.name_lc == "p")
            .filter(|ce| {
                ce.edge
                    .routes
                    .iter()
                    .any(|r| r.evidence.kind() == EvidenceKind::Unknown)
            })
            .map(|ce| format!("line {}", ce.edge.site.span.start.line))
            .collect();
        assert_eq!(unknown, Vec::<String>::new());
        let ids: std::collections::BTreeSet<String> = report
            .edges
            .iter()
            .filter(|ce| ce.edge.from.name_lc == "p")
            .flat_map(|ce| ce.edge.routes.iter())
            .filter_map(|r| match &r.target {
                RouteTarget::Builtin(b) => Some(b.0.clone()),
                _ => None,
            })
            .collect();
        for want in [
            "Text::trim",
            "Scalar::totext",
            "DateTime::totext",
            "RecordRef::number",
            "Text::trimend",
            "Text::tolower",
        ] {
            assert!(ids.contains(want), "{want} missing from {ids:?}");
        }
    }

    /// S9.0e: a `List`/`Dictionary` element is typed from the collection's
    /// declared type text — a declared var, `this.Global`, `Text.Split`, a
    /// nested `Get`, and `Keys()`.
    #[test]
    fn collection_element_receivers_resolve() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_minimal_workspace(dir.path());
        std::fs::write(
            dir.path().join("C.al"),
            "codeunit 50000 C\n{\n    var\n        G: Dictionary of [Integer, Text];\n\n    procedure P()\n    var\n        D: Dictionary of [Text, Dictionary of [Text, Text]];\n        L: List of [Text];\n        X: Text;\n        B: Boolean;\n    begin\n\
             X := L.Get(1).ToLower();\n\
             X := X.Split(',').Get(2).TrimEnd(']');\n\
             X := D.Get('a').Get('b').ToUpper();\n\
             B := this.G.Get(1).StartsWith('x');\n\
             X := D.Keys().Get(1).Trim();\n    end;\n}\n",
        )
        .expect("write C.al");
        let report = resolve_full_program(dir.path()).expect("resolve_full_program");
        let unknown: Vec<String> = report
            .edges
            .iter()
            .filter(|ce| ce.edge.from.name_lc == "p")
            .filter(|ce| {
                ce.edge
                    .routes
                    .iter()
                    .any(|r| r.evidence.kind() == EvidenceKind::Unknown)
            })
            .map(|ce| format!("line {}", ce.edge.site.span.start.line))
            .collect();
        assert_eq!(unknown, Vec::<String>::new());
        let ids: std::collections::BTreeSet<String> = report
            .edges
            .iter()
            .filter(|ce| ce.edge.from.name_lc == "p")
            .flat_map(|ce| ce.edge.routes.iter())
            .filter_map(|r| match &r.target {
                RouteTarget::Builtin(b) => Some(b.0.clone()),
                _ => None,
            })
            .collect();
        for want in [
            "Text::tolower",
            "Text::trimend",
            "Text::toupper",
            "Text::startswith",
            "Text::trim",
        ] {
            assert!(ids.contains(want), "{want} missing from {ids:?}");
        }
    }

    /// S9.0e: a `this.Global.Method()` argument types by the method's return,
    /// so it picks an overload (CDO's `RaiseActionError(.., this.ErrorActions.
    /// GetObjectId(), ..)`).
    #[test]
    fn this_global_call_result_argument_picks_an_overload() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_minimal_workspace(dir.path());
        std::fs::write(
            dir.path().join("C.al"),
            "codeunit 50001 W\n{\n    procedure F(I: Integer)\n    begin\n    end;\n\n    procedure F(T: Text)\n    begin\n    end;\n}\n\
             codeunit 50002 G\n{\n    procedure GetInt(): Integer\n    begin\n    end;\n}\n\
             codeunit 50000 C\n{\n    var\n        H: Codeunit G;\n        Wk: Codeunit W;\n\n    procedure P()\n    begin\n        Wk.F(this.H.GetInt());\n    end;\n}\n",
        )
        .expect("write C.al");
        let report = resolve_full_program(dir.path()).expect("resolve_full_program");
        let f_targets: Vec<(DispatchShape, usize)> = report
            .edges
            .iter()
            .filter(|ce| ce.edge.from.name_lc == "p")
            .filter(|ce| {
                ce.edge
                    .routes
                    .iter()
                    .any(|r| matches!(&r.target, RouteTarget::Routine(rid) if rid.name_lc == "f"))
            })
            .map(|ce| (ce.edge.shape, ce.edge.routes.len()))
            .collect();
        assert_eq!(f_targets, vec![(DispatchShape::Exact, 1)]);
    }

    /// S9.0d (compiler-oracle triage): a run reaches the entry triggers the
    /// SOURCE declares, the base object's and each page extension's, and none
    /// is invented for an object that declares none (a workspace
    /// `ConfirmationDialog` page with no triggers used to get an Opaque
    /// `onopenpage` boundary).
    #[test]
    fn object_runs_reach_the_declared_entry_triggers_only() {
        use crate::program::resolve::edge::{ObligationOutcome, classify_obligation};
        let dir = tempfile::tempdir().expect("tempdir");
        write_minimal_workspace(dir.path());
        let src = "page 50000 NoTriggers\n{\n    PageType = ConfirmationDialog;\n}\n\
             page 50001 WithOpen\n{\n    trigger OnOpenPage()\n    begin\n    end;\n}\n\
             pageextension 50002 WithOpenExt extends WithOpen\n{\n    trigger OnOpenPage()\n    begin\n    end;\n}\n\
             codeunit 50003 NoOnRun\n{\n    procedure X()\n    begin\n    end;\n}\n\
             codeunit 50000 C\n{\n    procedure P()\n    var\n        WP: Page NoTriggers;\n    begin\n        \
             Page.RunModal(Page::NoTriggers); // static\n        \
             WP.RunModal(); // typed\n        \
             Page.Run(Page::WithOpen); // ext\n        \
             Codeunit.Run(Codeunit::NoOnRun); // cu\n    end;\n}\n";
        std::fs::write(dir.path().join("C.al"), src).expect("write C.al");
        let report = resolve_full_program(dir.path()).expect("resolve_full_program");
        let at = |tag: &str| -> (Vec<ObligationOutcome>, Vec<String>) {
            let line = src.lines().position(|l| l.contains(tag)).expect("tag") as u32;
            let edges: Vec<_> = report
                .edges
                .iter()
                .filter(|ce| ce.edge.from.name_lc == "p" && ce.edge.site.span.start.line == line)
                .collect();
            let targets = edges
                .iter()
                .flat_map(|ce| ce.edge.routes.iter())
                .map(|r| match &r.target {
                    RouteTarget::Routine(rid) => format!("{:?}:{}", rid.object.key, rid.name_lc),
                    other => format!("{other:?}"),
                })
                .collect();
            (
                edges
                    .iter()
                    .map(|ce| classify_obligation(&ce.edge))
                    .collect(),
                targets,
            )
        };
        let empty = (vec![ObligationOutcome::HonestEmpty], Vec::<String>::new());
        assert_eq!(at("// static"), empty);
        assert_eq!(at("// typed"), empty);
        assert_eq!(at("// cu"), empty);
        let (outcomes, mut targets) = at("// ext");
        targets.sort();
        assert_eq!(outcomes, vec![ObligationOutcome::Resolved]);
        assert_eq!(
            targets,
            vec![
                "Id(50001):onopenpage".to_string(),
                "Id(50002):onopenpage".to_string()
            ]
        );
    }

    /// S9.0e: an overload that exists only under a `#if` arm is a candidate
    /// only for a call in a build where that arm compiles. `Foo` is two
    /// object-level arms whose parameters the argument typer cannot tell apart
    /// (Base App `OnBeforeUpdateColumnCaptions`: `array[15] of Text[80]` vs
    /// `of Text`). A call inside `#if not CLEAN27` / `#else` binds its own arm
    /// (`P`); so does a call in a split caller's arm (`R`, Continia
    /// `CoreSessionManager`); a call with no build context stays ambiguous (`Q`).
    #[test]
    fn overload_candidates_are_narrowed_to_the_call_sites_build() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_minimal_workspace(dir.path());
        let src = "codeunit 50000 C\n{\n\
#if not CLEAN27\n    procedure Foo(var A: array[15] of Text[80])\n    begin\n        Mark80();\n    end;\n\
#else\n    procedure Foo(var A: array[15] of Text)\n    begin\n        MarkText();\n    end;\n\
#endif\n\n    procedure Mark80()\n    begin\n    end;\n\n    procedure MarkText()\n    begin\n    end;\n\n\
    procedure P()\n    var\n        C80: array[15] of Text[80];\n        C: array[15] of Text;\n    begin\n\
#if not CLEAN27\n        Foo(C80); // p80\n#else\n        Foo(C); // ptext\n#endif\n    end;\n\n\
    procedure Q()\n    var\n        C80: array[15] of Text[80];\n    begin\n        Foo(C80); // q\n    end;\n\n\
#if not CLEAN27\n    procedure R(var X: array[15] of Text[80])\n#else\n    procedure R(var X: array[15] of Text)\n#endif\n\
    begin\n        Foo(X); // r\n    end;\n}\n";
        std::fs::write(dir.path().join("C.al"), src).expect("write C.al");
        let report = resolve_full_program(dir.path()).expect("resolve_full_program");
        let line_of = |tag: &str| src.lines().position(|l| l.contains(tag)).expect("tag") as u32;
        let foo_targets = |from: &str, line: u32| -> Vec<Vec<RoutineNodeId>> {
            report
                .edges
                .iter()
                .filter(|ce| ce.edge.from.name_lc == from && ce.edge.site.span.start.line == line)
                .map(|ce| {
                    ce.edge
                        .routes
                        .iter()
                        .filter_map(|r| match &r.target {
                            RouteTarget::Routine(rid) if rid.name_lc == "foo" => Some(rid.clone()),
                            _ => None,
                        })
                        .collect()
                })
                .collect()
        };
        let arm_calling = |marker: &str| -> RoutineNodeId {
            report
                .edges
                .iter()
                .find(|ce| {
                    ce.edge.routes.iter().any(
                        |r| matches!(&r.target, RouteTarget::Routine(rid) if rid.name_lc == marker),
                    )
                })
                .expect("marker call")
                .edge
                .from
                .clone()
        };
        let (foo80, footext) = (arm_calling("mark80"), arm_calling("marktext"));
        assert_ne!(foo80, footext);
        assert_eq!(
            foo_targets("p", line_of("// p80")),
            vec![vec![foo80.clone()]]
        );
        assert_eq!(
            foo_targets("p", line_of("// ptext")),
            vec![vec![footext.clone()]]
        );
        let q = foo_targets("q", line_of("// q"));
        assert_eq!(q.len(), 1);
        assert_eq!(q[0].len(), 2, "no build context: both arms stay candidates");
        let mut r: Vec<Vec<RoutineNodeId>> = foo_targets("r", line_of("// r"));
        r.sort();
        let mut expected = vec![vec![foo80], vec![footext]];
        expected.sort();
        assert_eq!(
            r, expected,
            "each arm of R binds the Foo arm of its own build"
        );
    }

    /// S9.0e: the same declaration in two `#if` arms collapses to one routine
    /// node, which exists in either arm's build, so it keeps only the symbols
    /// both arms decide alike (none here). Keeping the first arm's `X` would let
    /// the build narrowing drop it from a call in the `#else` build and bind the
    /// other overload.
    #[test]
    fn a_collapsed_same_signature_arm_pair_exists_in_both_builds() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_minimal_workspace(dir.path());
        let src = "codeunit 50000 C\n{\n\
#if X\n    local procedure Foo(A: Integer)\n    begin\n    end;\n\
#else\n    procedure Foo(A: Integer)\n    begin\n    end;\n#endif\n\n\
    procedure Foo(T: Text)\n    begin\n    end;\n\n\
    procedure P()\n    begin\n#if not X\n        Foo(Untyped); // call\n#endif\n    end;\n}\n";
        std::fs::write(dir.path().join("C.al"), src).expect("write C.al");
        let report = resolve_full_program(dir.path()).expect("resolve_full_program");
        let line = src
            .lines()
            .position(|l| l.contains("// call"))
            .expect("tag") as u32;
        let targets: Vec<usize> = report
            .edges
            .iter()
            .filter(|ce| ce.edge.from.name_lc == "p" && ce.edge.site.span.start.line == line)
            .map(|ce| ce.edge.routes.len())
            .collect();
        assert_eq!(targets, vec![2], "both overloads stay candidates");
    }

    /// S9.0e, alc-probed: an XmlPort `textattribute` with `TextType = BigText`
    /// is a `BigText` variable of the xmlport, so `X.AddText(..)` reaches
    /// `BigText.AddText` (Base App `ImportExportWorkflow`).
    #[test]
    fn xmlport_text_node_receivers_resolve() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_minimal_workspace(dir.path());
        std::fs::write(
            dir.path().join("C.al"),
            "xmlport 50000 X\n{\n    schema\n    {\n        textelement(Root)\n        {\n            textattribute(Big)\n            {\n                TextType = BigText;\n\n                trigger OnBeforePassVariable()\n                begin\n                    Big.AddText('a');\n                end;\n            }\n        }\n    }\n\n    procedure P()\n    begin\n        Root := Root.ToUpper();\n    end;\n}\n",
        )
        .expect("write C.al");
        let report = resolve_full_program(dir.path()).expect("resolve_full_program");
        let unresolved: Vec<_> = report
            .edges
            .iter()
            .filter(|ce| {
                ce.edge
                    .routes
                    .iter()
                    .any(|r| matches!(r.target, RouteTarget::Unresolved))
            })
            .map(|ce| ce.edge.site.span.start.line)
            .collect();
        assert_eq!(unresolved, Vec::<u32>::new());
        assert!(
            report.edges.len() >= 2,
            "both member calls are edges: {}",
            report.edges.len()
        );
    }

    /// S9.0e, alc-probed: a plain query column has its source field's type, so
    /// `QV.EntryType.AsInteger()` reaches the enum's `AsInteger` (Base App
    /// `ReconcileCustandVendAccs`). A `Method` column's type is not modelled.
    #[test]
    fn query_column_receivers_type_as_their_source_field() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_minimal_workspace(dir.path());
        std::fs::write(
            dir.path().join("C.al"),
            "enum 50000 E\n{\n    value(0; A) { }\n}\n\
             table 50000 T\n{\n    fields\n    {\n        field(1; \"Entry Type\"; Enum E) { }\n        field(2; Amt; Decimal) { }\n    }\n}\n\
             query 50000 Q\n{\n    elements\n    {\n        dataitem(D; T)\n        {\n            column(EntryType; \"Entry Type\") { }\n            column(SumAmt; Amt) { Method = Sum; }\n        }\n    }\n}\n\
             codeunit 50000 C\n{\n    procedure P()\n    var\n        QV: Query Q;\n        I: Integer;\n        S: Text;\n    begin\n        I := QV.EntryType.AsInteger();\n        S := QV.SumAmt.ToText();\n    end;\n}\n",
        )
        .expect("write C.al");
        let report = resolve_full_program(dir.path()).expect("resolve_full_program");
        let outcome = |line: u32| -> Vec<bool> {
            report
                .edges
                .iter()
                .filter(|ce| ce.edge.from.name_lc == "p" && ce.edge.site.span.start.line == line)
                .map(|ce| {
                    ce.edge
                        .routes
                        .iter()
                        .any(|r| matches!(r.target, RouteTarget::Unresolved))
                })
                .collect()
        };
        let line_of = |needle: &str| {
            std::fs::read_to_string(dir.path().join("C.al"))
                .expect("read C.al")
                .lines()
                .position(|l| l.contains(needle))
                .expect("line") as u32
        };
        assert_eq!(outcome(line_of("EntryType.AsInteger")), vec![false]);
        assert_eq!(outcome(line_of("SumAmt.ToText")), vec![true]);
    }

    /// S9.0e: a procedure header split across `#if`/`#else` is one routine per
    /// arm, so the call written for each arm's signature binds that arm's routine
    /// (Base App `MfgCalculateBOMTree.CalcRoutingLineCosts`, 6 vs 5 parameters).
    /// The `#else` arm's body drops the `#if not CLEAN27` block that uses the
    /// parameter only the first arm declares (`sender` there).
    #[test]
    fn split_header_arms_each_bind_their_own_call() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_minimal_workspace(dir.path());
        std::fs::write(
            dir.path().join("C.al"),
            "codeunit 50000 C\n{\n    procedure P()\n    begin\n#if not CLEAN27\n        Callee(1, this);\n#else\n        Callee(1);\n#endif\n    end;\n\n\
             procedure Hook()\n    begin\n    end;\n\n\
             #if not CLEAN27\n    local procedure Callee(A: Integer; var sender: Codeunit C)\n#else\n    local procedure Callee(A: Integer)\n#endif\n    begin\n#if not CLEAN27\n        sender.Hook();\n#endif\n    end;\n}\n",
        )
        .expect("write C.al");
        let report = resolve_full_program(dir.path()).expect("resolve_full_program");
        let unresolved: Vec<_> = report
            .edges
            .iter()
            .filter(|ce| {
                ce.edge
                    .routes
                    .iter()
                    .any(|r| matches!(r.target, RouteTarget::Unresolved))
            })
            .map(|ce| (ce.edge.from.name_lc.clone(), ce.edge.from.params_count))
            .collect();
        assert_eq!(unresolved, vec![]);
        let hook_callers: Vec<_> = report
            .edges
            .iter()
            .filter(|ce| {
                ce.edge.routes.iter().any(
                    |r| matches!(&r.target, RouteTarget::Routine(rid) if rid.name_lc == "hook"),
                )
            })
            .map(|ce| ce.edge.from.params_count)
            .collect();
        assert_eq!(hook_callers, vec![2]);
        let mut targets: Vec<(DispatchShape, Vec<usize>)> = report
            .edges
            .iter()
            .filter(|ce| ce.edge.from.name_lc == "p")
            .map(|ce| {
                let arities = ce
                    .edge
                    .routes
                    .iter()
                    .filter_map(|r| match &r.target {
                        RouteTarget::Routine(rid) if rid.name_lc == "callee" => {
                            Some(rid.params_count)
                        }
                        _ => None,
                    })
                    .collect();
                (ce.edge.shape, arities)
            })
            .collect();
        targets.sort_by_key(|(_, a)| a.clone());
        assert_eq!(
            targets,
            vec![
                (DispatchShape::Exact, vec![1]),
                (DispatchShape::Exact, vec![2])
            ]
        );
    }

    /// S9.0e, alc-probed: a bare `CreateTask()` has no global form (AL0118 in
    /// a codeunit), so in a report dataitem trigger the dataitem table's own
    /// `CreateTask` binds; and a page with no `SourceTable` binds a bare
    /// `Caption(..)` to the page itself.
    #[test]
    fn grounded_bare_calls_bind_the_table_or_the_page() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_minimal_workspace(dir.path());
        std::fs::write(
            dir.path().join("C.al"),
            "table 50000 T\n{\n    fields\n    {\n        field(1; Code; Code[20]) { }\n    }\n\n    procedure CreateTask()\n    begin\n    end;\n}\n\
             report 50000 R\n{\n    dataset\n    {\n        dataitem(H; T)\n        {\n            trigger OnAfterGetRecord()\n            begin\n                CreateTask();\n            end;\n        }\n    }\n}\n\
             page 50000 P\n{\n    trigger OnOpenPage()\n    begin\n        Caption('x');\n    end;\n}\n",
        )
        .expect("write C.al");
        let report = resolve_full_program(dir.path()).expect("resolve_full_program");
        let targets: Vec<String> = report
            .edges
            .iter()
            .filter(|ce| {
                matches!(
                    ce.edge.from.name_lc.as_str(),
                    "onaftergetrecord" | "onopenpage"
                )
            })
            .flat_map(|ce| ce.edge.routes.iter())
            .map(|r| match &r.target {
                RouteTarget::Routine(rid) => format!("routine {}", rid.name_lc),
                RouteTarget::Builtin(b) => format!("builtin {}", b.0),
                other => format!("{other:?}"),
            })
            .collect();
        let page_caption = crate::program::resolve::member_catalog::member_builtin_id(
            crate::program::resolve::member_catalog::MemberCatalogKind::Framework(
                &crate::program::resolve::receiver::FrameworkKind::PageInstance,
            ),
            "caption",
        )
        .expect("caption is a PageInstance member");
        assert!(
            targets.contains(&"routine createtask".to_string()),
            "{targets:?}"
        );
        assert!(
            targets.contains(&format!("builtin {}", page_caption.0)),
            "{targets:?}"
        );
    }

    /// S9.0e: a namespace-qualified enum type name types as the enum type,
    /// with or without `Enum::`; a name that is no enum stays Unknown.
    #[test]
    fn namespace_qualified_enum_type_receivers_resolve() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_minimal_workspace(dir.path());
        std::fs::write(
            dir.path().join("E.al"),
            "namespace Microsoft.Foo;\n\nenum 50000 \"X Type\"\n{\n    value(0; A) { }\n}\n",
        )
        .expect("write E.al");
        std::fs::write(
            dir.path().join("C.al"),
            "codeunit 50000 C\n{\n    procedure P()\n    var\n        E: Enum Microsoft.Foo.\"X Type\";\n    begin\n\
             E := Microsoft.Foo.\"X Type\".FromInteger(0);\n\
             E := Enum::Microsoft.Foo.\"X Type\".FromInteger(0);\n\
             E := Microsoft.Foo.\"No Such\".FromInteger(0);\n    end;\n}\n",
        )
        .expect("write C.al");
        let report = resolve_full_program(dir.path()).expect("resolve_full_program");
        let in_p = || report.edges.iter().filter(|ce| ce.edge.from.name_lc == "p");
        let first = in_p()
            .map(|ce| ce.edge.site.span.start.line)
            .min()
            .expect("edges in P");
        let unknown: Vec<u32> = in_p()
            .filter(|ce| {
                ce.edge
                    .routes
                    .iter()
                    .any(|r| r.evidence.kind() == EvidenceKind::Unknown)
            })
            .map(|ce| ce.edge.site.span.start.line)
            .collect();
        assert_eq!(unknown, vec![first + 2]);
    }

    /// S9.0e: an unpicked overload set types its chain when every candidate
    /// returns the same type (`Regex.Replace(..).Split(..)`); different return
    /// types decline.
    #[test]
    fn same_return_overloads_type_their_chain() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_minimal_workspace(dir.path());
        std::fs::write(
            dir.path().join("C.al"),
            "codeunit 50001 R\n{\n    procedure Same(T: Text): Text\n    begin\n    end;\n\n    procedure Same(I: Integer): Text\n    begin\n    end;\n\n    procedure Mixed(T: Text): Text\n    begin\n    end;\n\n    procedure Mixed(I: Integer): Integer\n    begin\n    end;\n}\n\
             codeunit 50000 C\n{\n    procedure P()\n    var\n        Reg: Codeunit R;\n        V: Variant;\n        X: Text;\n    begin\n\
             X := Reg.Same(V).ToLower();\n\
             X := Reg.Mixed(V).ToLower();\n    end;\n}\n",
        )
        .expect("write C.al");
        let report = resolve_full_program(dir.path()).expect("resolve_full_program");
        let in_p = || report.edges.iter().filter(|ce| ce.edge.from.name_lc == "p");
        let first = in_p()
            .map(|ce| ce.edge.site.span.start.line)
            .min()
            .expect("edges in P");
        let unknown: Vec<u32> = in_p()
            .filter(|ce| {
                ce.edge
                    .routes
                    .iter()
                    .any(|r| r.evidence.kind() == EvidenceKind::Unknown)
            })
            .map(|ce| ce.edge.site.span.start.line)
            .collect();
        // Only the `Mixed(..).ToLower()` line declines.
        assert_eq!(unknown, vec![first + 1]);
    }

    /// S9.0e: an operator result types its receiver — a comparison is a
    /// Boolean, arithmetic on numbers is a number, `Date - Date` an Integer.
    /// A Text `+` and `Date + Integer` decline.
    #[test]
    fn operator_result_receivers_resolve() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_minimal_workspace(dir.path());
        std::fs::write(
            dir.path().join("C.al"),
            "codeunit 50000 C\n{\n    procedure P()\n    var\n        A: Integer;\n        B: Decimal;\n        D1: Date;\n        D2: Date;\n        X: Text;\n    begin\n\
             X := (A / B).ToText();\n\
             X := (-B).ToText();\n\
             X := (D1 - D2).ToText();\n\
             X := (A < B).ToText();\n\
             X := (X + X).ToLower();\n\
             X := (D1 + A).ToText();\n    end;\n}\n",
        )
        .expect("write C.al");
        let report = resolve_full_program(dir.path()).expect("resolve_full_program");
        let unknown: Vec<u32> = report
            .edges
            .iter()
            .filter(|ce| ce.edge.from.name_lc == "p")
            .filter(|ce| {
                ce.edge
                    .routes
                    .iter()
                    .any(|r| r.evidence.kind() == EvidenceKind::Unknown)
            })
            .map(|ce| ce.edge.site.span.start.line)
            .collect();
        // Lines are 0-based: the two declining sites are the last two.
        let first = report
            .edges
            .iter()
            .filter(|ce| ce.edge.from.name_lc == "p")
            .map(|ce| ce.edge.site.span.start.line)
            .min()
            .expect("edges in P");
        assert_eq!(unknown, vec![first + 4, first + 5]);
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
            // The surface is built inside the timed window, so the total
            // still includes the DeclSurface build, as the label says.
            let (edges, coverage, _audit, _ifaces, _parenless) = resolve_full_program_from_parts(
                &graph,
                &parsed,
                &DeclSurface::build(&graph, &parsed),
                primary_app_ref,
                &ws_file_set,
            );
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
        let (full_edges, coverage, _audit, _ifaces, _parenless) = resolve_full_program_from_parts(
            graph,
            parsed,
            &ctx.decl_surface(),
            primary_app_ref,
            ws_file_set,
        );
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
        let obj_node_map = app_object_map(graph, primary_app_ref);
        let index = ResolveIndex::build(graph);
        let surface = ctx.decl_surface();

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

    /// `app_object_map` of the primary app holds exactly the primary app's objects, and
    /// resolving a file with it gives the same edges as the whole-graph map
    /// (the only lookups use primary-app ids).
    #[test]
    fn workspace_object_map_is_workspace_only_and_resolves_identically() {
        let ws = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/r0-corpus/ws-baseapp-closure");
        let ctx = build_context(&ws).expect("build_context");
        let ProgramContext {
            graph,
            parsed,
            primary_app_ref,
            ws_file_set,
            ..
        } = &ctx;
        let primary = *primary_app_ref;

        let ws_map = app_object_map(graph, primary);
        let full_map: HashMap<ObjectNodeId, &ObjectNode> =
            graph.objects.iter().map(|o| (o.id.clone(), o)).collect();
        let ws_count = graph.objects.iter().filter(|o| o.id.app == primary).count();
        assert!(ws_count > 0, "fixture has a workspace object");
        assert!(
            full_map.len() > ws_count,
            "fixture must carry dependency objects so the maps can differ"
        );
        assert_eq!(ws_map.len(), ws_count);
        assert!(ws_map.keys().all(|id| id.app == primary));

        let index = ResolveIndex::build(graph);
        let surface = ctx.decl_surface();
        let mut checked = 0;
        for unit in parsed
            .iter()
            .filter(|u| graph.apps.find(&u.app) == Some(primary))
        {
            for pf in unit
                .files
                .iter()
                .filter(|pf| ws_file_set.contains(&pf.virtual_path))
            {
                let a = resolve_file_obligations(pf, primary, graph, &index, &surface, &ws_map);
                let b = resolve_file_obligations(pf, primary, graph, &index, &surface, &full_map);
                assert!(!a.edges.is_empty());
                assert_eq!(a.edges.len(), b.edges.len());
                for (x, y) in a.edges.iter().zip(b.edges.iter()) {
                    assert_eq!(x.obligation_id, y.obligation_id);
                    assert_eq!(x.edge, y.edge);
                }
                checked += 1;
            }
        }
        assert!(checked >= 1);
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
    // Task 2: FreshCoverage + opaque dependency closure, through the
    // production build (`build_program_with_coverage`, what analyze runs)
    // -----------------------------------------------------------------------

    fn fresh_coverage(ws: &Path) -> Result<FreshCoverage, String> {
        build_program_with_coverage(ws).map(|(_, _, fc)| fc)
    }

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

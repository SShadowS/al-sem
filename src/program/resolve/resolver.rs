//! Bare-call resolution: resolve an unqualified `Foo()` call to its [`Route`]
//! target(s), topology-scoped, with evidence and witness.
//!
//! # Precedence (first hit wins)
//!
//! 1. **Own object** — a procedure named `name_lc` declared in `from_object`.
//! 2. **Extension base** — if `from_object` is a `*Extension`, search the base
//!    object (`TableExtension`→`Table`, `PageExtension`→`Page`, …).
//! 3. **Implicit-Rec** (beyond-1B.3b Task 3) — a bare call inside a `Table`/
//!    `Page`/`TableExtension`/`PageExtension` implicitly dispatches to `Rec` as
//!    a LAST-RESORT fallback, after Steps 1-2 have had first refusal. Every
//!    other object kind (Codeunit/Report/XmlPort/Query/…) structurally skips
//!    this step. Gated on `WithState::NoWithProven` (a bare call lexically
//!    inside a `with X do` is NEVER eligible — see [`crate::program::resolve::
//!    extract::WithState`]) and on [`resolve_in_table_scope`] (Task 2's
//!    visibility-scoped table∪extensions search). A table-scope candidate that
//!    collides in name+arity with a global builtin is an UNPROVEN precedence —
//!    fail closed to `Unknown` rather than assume the table wins — UNLESS the
//!    colliding name is compiler-GROUNDED as never having a bare-call form
//!    anywhere in AL (pageext-merge-and-final-residual plan, Task 2 — see
//!    [`INSTANCE_ONLY_NEVER_BARE`]'s doc), in which case the table candidate
//!    wins outright (no collision at all — the "global"/"page intrinsic"
//!    reading was never a real option). `Update`/`Close`/`Run`/`RunModal`/…
//!    are exactly this proven-never-bare case: real AL methods, but on
//!    Page/Codeunit/Report/… INSTANCES, always reached through an explicit
//!    receiver (`CurrPage.Update()`, `MyCodeunit.Run()`) — never a bare
//!    unqualified call.
//! 4. **Global builtin** — `is_global_builtin(name_lc)` → `Catalog` route,
//!    UNLESS `name_lc` is in [`INSTANCE_ONLY_NEVER_BARE`] (same grounding as
//!    step 3 — a name with no bare form anywhere skips the catalog on this
//!    unqualified path too, falling through to `Unknown` instead of a false
//!    `Catalog` route).
//! 5. **Unknown** — genuine resolution failure.
//!
//! # Arity matching (Phase 2 / Phase 3 Task 0; ambiguity guard beyond-1B.3b Task 2)
//!
//! `RoutineNodeId` now carries `params_count`, so each overload (same name,
//! different arity) is a distinct node in the graph and index.
//! `routines_in_object` returns one entry per distinct overload.  An overload
//! matches when `rid.params_count == arity`.  When EXACTLY ONE match is found
//! it is returned.  When the name is found but NO overload matches the arity,
//! OR more than one same-arity overload matches (a genuine SOURCE-overload
//! collision — post-Task-2 (sigfp-and-ambiguous-reclassification plan)
//! source `sig_fp` is a real fingerprint, so this now means either
//! genuinely distinct overloads full arg-type dispatch would be needed to
//! disambiguate, or a residual same-id `source_overload_aliased` collision
//! [`resolve_in_object`]'s degraded-set guard catches — see its doc), an
//! `Unknown` route is emitted — no
//! false-confident edge to a wrong-arity OR ambiguously-picked target.  The
//! caller still stops at that precedence level (does NOT fall through to
//! extension-base / global-builtin), mirroring L3's MemberNotFound stop
//! semantics while surfacing the gap honestly.  Name-absent ⇒ `None` ⇒ fall
//! through.
//!
//! # Witness↔evidence contract
//!
//! `Evidence::Source` ⇒ `Witness::SourceSpan`
//! `Evidence::Catalog`⇒ `Witness::CatalogEntry`
//! `Evidence::Unknown`⇒ `Witness::None`

use al_syntax::IdentifierFoldExt;
use al_syntax::ir::ObjectKind;
use phf::phf_set;

use crate::program::abi_ingest::object_kind_from_abi_type;
use crate::program::graph::ProgramGraph;
use crate::program::node::{AppRef, ObjKey, ObjectNodeId, RoutineNodeId};
use crate::program::node_extract::{Access, ObjectNode, RoutineNode};
use crate::program::resolve::arg_dispatch::{
    ArgDispatchInfo, ParamDispatchInfo, candidate_param_infos, candidate_param_infos_abi,
    pick_candidate,
};
use crate::program::resolve::builtins::{catalog_version, global_builtin_id};
use crate::program::resolve::decl_surface::DeclSurface;
use crate::program::resolve::edge::{
    AbiEventKind, AbiRoutineKey, AbiRoutineKind, BuiltinId, CanonicalSpan, Condition,
    DispatchShape, Edge, EdgeKind, Evidence, EvidenceKind, OpenWorldReason, Route, RouteTarget,
    SetCompleteness, SiteId, SourcePos, UnknownReason, Witness, callee_fp,
};
use crate::program::resolve::extract::WithState;
use crate::program::resolve::index::ResolveIndex;
use crate::program::resolve::member_catalog::{
    MemberCatalogKind, member_builtin, member_builtin_id,
};
use crate::program::resolve::receiver::{
    ControlAddInSurface, FrameworkKind, ReceiverType, resolve_pageext_base_source_table,
    resolve_source_table_ref, resolve_tableext_base_table,
};
use crate::snapshot::TrustTier;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Map a trust tier to the resolution evidence level.
///
/// Source-bearing tiers (Workspace / EmbeddedSource / LocalSource{Verified,
/// Approximate}) produce `Evidence::Source`; symbol-only tables produce `Abi`.
fn tier_evidence(tier: TrustTier) -> Evidence {
    match tier {
        TrustTier::Workspace
        | TrustTier::EmbeddedSource
        | TrustTier::LocalSourceVerified
        | TrustTier::LocalSourceApproximate => Evidence::Source,
        TrustTier::SymbolOnly => Evidence::Abi,
    }
}

/// For an extension object kind, return the corresponding base object kind.
///
/// Returns `None` for non-extension kinds.
fn extension_base_kind(kind: ObjectKind) -> Option<ObjectKind> {
    match kind {
        ObjectKind::TableExtension => Some(ObjectKind::Table),
        ObjectKind::PageExtension => Some(ObjectKind::Page),
        ObjectKind::ReportExtension => Some(ObjectKind::Report),
        ObjectKind::EnumExtension => Some(ObjectKind::Enum),
        ObjectKind::PermissionSetExtension => Some(ObjectKind::PermissionSet),
        _ => None,
    }
}

/// Build a `Route` for a resolved routine.
///
/// - If the routine is in the `DeclSurface` (source-bearing): `Evidence::Source` +
///   `Witness::SourceSpan { file: virtual_path, span: (start_byte, end_byte) }`.
/// - If the routine is `SymbolOnly` (dep boundary, body unavailable): `Evidence::Opaque` +
///   `Witness::AbiSymbol` — the symbol identity is retained as a boundary marker.
/// - If the routine is NOT in the `DeclSurface` (integration gap on a source-bearing tier):
///   `Evidence::Unknown` + `Witness::None`.  Surface this as Unknown so the gap shows up
///   in `real_unknown_rate` rather than being silently hidden.
fn make_routine_route(
    rid: &RoutineNodeId,
    obj_tier: TrustTier,
    surface: &DeclSurface,
    graph: &ProgramGraph,
) -> Route {
    if let Some((meta, path)) = surface.get_with_path(rid) {
        Route {
            target: RouteTarget::Routine(rid.clone()),
            evidence: tier_evidence(obj_tier),
            conditions: vec![],
            witness: Witness::SourceSpan {
                file: path.to_string(),
                span: (meta.origin.byte.start as u32, meta.origin.byte.end as u32),
            },
            receiver_tier: None,
        }
    } else if obj_tier == TrustTier::SymbolOnly {
        // SymbolOnly routine: body unavailable (loaded from .app SymbolReference,
        // no source parse).  We know the symbol exists at this ABI boundary, so
        // return Opaque rather than Unknown.  This preserves identity across the
        // boundary and classifies the route as `Resolved` (not `Unknown`) in the
        // obligation metric, matching L3's External treatment of dep symbols.
        let (obj_num, obj_name_lc) = match &rid.object.key {
            ObjKey::Id(n) => (*n, String::new()),
            ObjKey::Name(s) => (0i64, s.clone()),
        };
        // Read the ABI-sourced routine/event kinds from the graph node.
        // `graph.routines` is sorted by RoutineNodeId (see build.rs), enabling
        // O(log n) lookup. Falls back to Procedure/None when the node is absent
        // (integration gap — should not happen for a valid SymbolOnly boundary).
        let opt_node = graph
            .routines
            .binary_search_by(|probe| probe.id.cmp(rid))
            .ok()
            .map(|i| &graph.routines[i]);
        let routine_kind = opt_node
            .and_then(|n| n.abi_routine_kind.clone())
            .unwrap_or(AbiRoutineKind::Procedure);
        let event_kind = opt_node
            .and_then(|n| n.abi_event_kind.clone())
            .unwrap_or(AbiEventKind::None);
        let key = AbiRoutineKey {
            app: rid.object.app,
            object_type: format!("{:?}", rid.object.kind).to_ascii_lowercase(),
            object_number: obj_num,
            object_name_lc: obj_name_lc,
            routine_name_lc: rid.name_lc.clone(),
            params_count: rid.params_count,
            param_type_fp: rid.sig_fp,
            routine_kind,
            event_kind,
        };
        Route {
            target: RouteTarget::AbiSymbol { key: key.clone() },
            evidence: Evidence::Opaque,
            conditions: vec![],
            witness: Witness::AbiSymbol { key },
            receiver_tier: None,
        }
    } else {
        // Source-tier DeclSurface miss: integration bug.  Surface it as Unknown so
        // the gap shows up in `real_unknown_rate` rather than being silently hidden.
        unresolved_route(UnknownReason::IndexIntegrationGap)
    }
}

/// Build an `Unresolved`/`Unknown(reason)` route — the shared constructor for
/// every genuine resolution-failure route (Task 3: the `reason` argument is
/// REQUIRED, so every call site is forced to supply a diagnostic
/// [`UnknownReason`]).
fn unresolved_route(reason: UnknownReason) -> Route {
    Route {
        target: RouteTarget::Unresolved,
        evidence: Evidence::Unknown(reason),
        conditions: vec![],
        witness: Witness::None,
        receiver_tier: None,
    }
}

/// Like [`unresolved_route`] but additionally tags the resolved RECEIVER
/// object's [`TrustTier`] (reason-split Task 2's `receiver_tier` diagnostic —
/// see [`Route::receiver_tier`]'s doc). Used ONLY at
/// `UnknownReason::MemberNotFound` emission sites where a receiver OBJECT was
/// in fact resolved (member-absent-on-a-resolved-surface) — never for
/// `ObjectNotInGraph`, where no resolved receiver exists to tag.
fn unresolved_route_with_tier(reason: UnknownReason, tier: TrustTier) -> Route {
    Route {
        receiver_tier: Some(tier),
        ..unresolved_route(reason)
    }
}

/// Whether `rid` is currently marked [`RoutineNode::abi_overload_collapsed`]
/// on `graph` — the COLLAPSE-MARKER GUARD shared by every `make_routine_
/// route` call site (Task 2 review fix). Before this fix, only TWO of the
/// SIX sites that ultimately build a route for a specific `RoutineNodeId`
/// consulted the marker: [`resolve_in_object`]'s single-visible-candidate
/// arm (the PLAIN-DISPATCH MARKER GUARD, Task 2 round-2) and
/// [`routine_node_for_type_query`] (the CHAIN-type-query guard, Task 3
/// review fix). The other FOUR — [`resolve_object_run`], `resolve_member`'s
/// own inline `Codeunit.Run(arity<=1)` special case, [`resolve_implicit_
/// trigger`]'s trigger fan-out, and [`emit_event_flow_edges`]'s subscriber
/// fan-out — look up a routine directly by ROLE (entry trigger / trigger
/// name / subscriber match) rather than through either selection boundary,
/// so a collapse-marked survivor could reach a confident `Opaque`/`Source`
/// route through any of them unguarded. Centralizing the lookup here means
/// a future call site gets the same protection by construction rather than
/// by remembering to copy the check.
fn routine_is_collapse_marked(rid: &RoutineNodeId, graph: &ProgramGraph) -> bool {
    graph
        .routines
        .binary_search_by(|probe| probe.id.cmp(rid))
        .ok()
        .is_some_and(|i| graph.routines[i].abi_overload_collapsed)
}

/// Whether `rid` is currently marked [`RoutineNode::source_overload_aliased`]
/// on `graph` — the SOURCE-tier sibling of [`routine_is_collapse_marked`]
/// (whole-branch review F1). Consulted ONLY by [`resolve_in_object`]'s `_`
/// arm prevalidation: the binding precondition for constructing
/// `DispatchShape::AmbiguousOverload` is "NO candidate is collapse-marked,
/// ABI **or source-alias**" (sigfp-and-ambiguous-reclassification plan,
/// round-1 addendum), but before this fix only the ABI marker was actually
/// consulted there. A `source_overload_aliased` survivor is one of ≥2
/// GENUINELY DISTINCT source overloads whose `sig_fp` collided onto ONE
/// `RoutineNodeId` (see `build::dedup_routines_preserving_genuine_overloads`'s
/// doc) — left unguarded, such a pair could reach `resolve_in_object`'s `_`
/// arm as TWO candidates sharing one id, both resolving through the SAME
/// `DeclSurface` entry (`DeclSurface` is keyed by `RoutineNodeId`), and construct an
/// `AmbiguousOverload` shape with two IDENTICAL-target concrete routes — the
/// last laundering path out of `unknown` this prevalidation exists to close.
fn routine_is_source_aliased(rid: &RoutineNodeId, graph: &ProgramGraph) -> bool {
    graph
        .routines
        .binary_search_by(|probe| probe.id.cmp(rid))
        .ok()
        .is_some_and(|i| graph.routines[i].source_overload_aliased)
}

/// Try to resolve `name_lc` with `arity` arguments inside `obj_id`, as called
/// from the identity `from_object` (Task 1 — beyond-1B.3b-follow-up:
/// PER-CANDIDATE access filtering; see [`routine_candidate_is_visible`]).
///
/// Returns the UNIQUE arity-matched, VISIBLE-from-`from_object` overload as a
/// `Source` route. Returns `None` only when the name is absent entirely in
/// `obj_id` (genuine absence — callers may fall through to a further
/// precedence level). Every other outcome (arity mismatch, access exclusion,
/// or an unresolved overload ambiguity) is `Some(Unresolved{Unknown(reason)})`
/// — a decline that STOPS at this precedence level rather than falling
/// through (mirrors L3's MemberNotFound stop semantics; see module doc).
///
/// # Selection rule (the overload-narrowing guard — do NOT weaken)
///
/// Reason-split (Task 2): this rule structurally produces exactly FOUR
/// distinct `Unknown` shapes, now tagged with FOUR distinct [`UnknownReason`]
/// values (previously all four collapsed into one `OverloadAmbiguous`
/// bucket) — see each variant's own doc for the full rationale.
///
/// 1. Zero arity-matched candidates (`pre_filter_count == 0`): name found but
///    no overload matches the arity → `Unknown(ArityMismatch)` — nothing to
///    be ambiguous BETWEEN.
/// 2. `pre_filter_count >= 1`: partition the arity-matched set by
///    [`routine_candidate_is_visible`].
///    - **0 visible** → access excluded every arity-matched candidate →
///      `Unknown(access_exclusion_reason(..))` (falls back to
///      `IndexIntegrationGap` only in the defensive case where a candidate
///      caused the exclusion but the reason-finder couldn't re-derive why —
///      should never happen, since the two functions apply the identical
///      per-`Access` rule).
///    - **Exactly 1 visible AND `pre_filter_count == 1`** → the visible
///      candidate WAS the only overload to begin with; access filtering
///      changed nothing about cardinality → resolve it (subject to the
///      collapse-marker guard below → `Unknown(AbiCollapsedOverload)`).
///    - **Exactly 1 visible BUT `pre_filter_count > 1`** → access narrowed an
///      originally-AMBIGUOUS same-arity set down to one. This is NOT a safe
///      selection: the pre-filter set was ambiguous (no arg-type evidence to
///      pick between overloads — full arg-type dispatch is deferred), so
///      access removing the OTHER sibling(s) doesn't prove the call meant
///      THIS one. Selecting the lone survivor would MANUFACTURE a false
///      `Source` route from what is actually still an unproven overload
///      choice → `Unknown(AccessFilteredOverload)`.
///    - **>1 visible** → genuine unresolved ambiguity (mirrors the
///      interface-implementer fan-out's `>1 candidates → Unresolved` rule) →
///      `Unknown(OverloadAmbiguous)`. Never pick-first.
///
/// **SymbolOnly tier (Task 1 — FULL source-tier selection discipline, no
/// exception):** `params_count` is populated from the ABI (real `Parameters[]
/// .len()`, or the `UNKNOWN_ARITY` sentinel when the field was absent/
/// unparseable — see `abi_ingest::UNKNOWN_ARITY`'s tri-state contract), and
/// `access` is populated from `IsProtected`/the local/internal drop at
/// ingestion (`abi_ingest::ingest_abi`) — so a SymbolOnly candidate now flows
/// through EXACTLY the same arity + per-candidate-visibility selection below
/// as a source-tier one. There is no `candidates.first()` short-circuit: an
/// order-dependent pick on a multi-candidate ABI set is a false-`Source`/
/// `Opaque` vector the pre-Task-1 code was exposed to (a `protected` ABI
/// sibling could shadow a `public` one, or vice versa, purely by JSON array
/// order). An `UNKNOWN_ARITY`-sentinel candidate structurally never matches a
/// real call's `arity` (see the sentinel's doc), so it silently drops out of
/// `matched` below — never emitting, exactly like a genuine arity mismatch.
///
/// # Return shape (Task 4, sigfp-and-ambiguous-reclassification plan)
///
/// Returns `Option<(DispatchShape, Vec<Route>)>` (the file's own
/// `(DispatchShape, Vec<Route>)` tuple convention — see [`member_catalog_route`]
/// etc.): `None` only on genuine name-absence (unchanged). Every `Some` outcome
/// is `(DispatchShape::Exact, vec![single route])` EXCEPT the genuine `>1`
/// visible, prevalidated-concrete candidate case, which is
/// `(DispatchShape::AmbiguousOverload, vec![one route per candidate])` — see
/// the `_` arm's doc below for the prevalidation contract.
#[allow(clippy::too_many_arguments)] // 8 pre-existing params + `args` (Task 2, argtype-dispatch-and-page-catalog plan); each is a distinct identity/lookup input, grouping would obscure call sites.
fn resolve_in_object(
    obj_id: &ObjectNodeId,
    obj_tier: TrustTier,
    name_lc: &str,
    arity: usize,
    from_object: &ObjectNodeId,
    graph: &ProgramGraph,
    index: &ResolveIndex,
    surface: &DeclSurface,
    // Task 2 (argtype-dispatch-and-page-catalog plan): the call site's typed
    // arguments, consulted ONLY by the `_` arm's fail-closed pick (see
    // `arg_dispatch`'s module doc) — every other arm ignores this entirely.
    // Empty for every 0-arity call and for every call site that has no
    // argument-typing context available (bare-call/receiver type-query
    // helpers below — see `resolve_bare`/`resolve_member`'s `args = &[]`
    // wrappers).
    args: &[ArgDispatchInfo],
) -> Option<(DispatchShape, Vec<Route>)> {
    let candidates = index.routines_in_object(obj_id, name_lc);
    if candidates.is_empty() {
        return None;
    }

    // Arity-exact match: collect EVERY overload whose params_count == arity.
    // With params_count in RoutineNodeId, each overload is normally a distinct
    // node — but two DISTINCT overloads sharing (object, name_lc, params_count)
    // collide onto one `RoutineNodeId` when their `sig_fp` also matches.
    // Post-Task-2 (sigfp-and-ambiguous-reclassification plan), SOURCE `sig_fp`
    // is a REAL fingerprint (`sig_fp::source_param_sig_fp`, an fnv1a fold of
    // every parameter's normalized type text + by-ref flag) — NOT always `0`
    // as it was pre-Task-2 (see node.rs's now-historical note) — so a genuine
    // same-name/same-arity SOURCE overload pair almost always gets DISTINCT
    // ids and never reaches this collision path at all. A residual collision
    // (the fingerprint itself aliases despite the two declarations' real
    // content, tracked independently in `param_sig_key`, genuinely differing)
    // still reaches here: `build_program_graph`'s dedup
    // (`dedup_routines_preserving_genuine_overloads`) keeps BOTH survivors
    // under the shared id rather than collapsing to one, and marks EVERY
    // survivor in that run [`RoutineNode::source_overload_aliased`] — the `_`
    // arm below (whole-branch review F1) treats any such marked candidate as
    // degraded, exactly like an ABI `abi_overload_collapsed` survivor, rather
    // than trust two same-id candidates as a genuine `>1`-DISTINCT-target
    // ambiguity. An ABI `sig_fp` (`abi_ingest::param_type_fp`) now folds a
    // length-delimited canonical tuple of every parameter's outer kind +
    // Subtype id + raw Subtype name + a degradation tag (Task 2 round-2
    // addendum — previously: only the OUTER type keyword, never a `Subtype`,
    // so two genuinely DIFFERENT overloads differing only by an object-typed
    // parameter's Subtype silently collided). Two ABI entries now collide
    // onto one `RoutineNodeId` ONLY when their ENTIRE canonical tuple matches
    // — a true re-parse duplicate, or a residual fingerprint collision this
    // engine cannot further distinguish (either way, `dedup_routines_
    // preserving_genuine_overloads` collapses that run to ONE survivor and
    // flags it `RoutineNode::abi_overload_collapsed`, since an ABI routine's
    // `param_sig_key` is hardcoded empty — no independent content signature
    // beyond the tuple already folded into `sig_fp`). So `matched.len() > 1`
    // HERE means either genuinely DISTINCT `RoutineNodeId`s (different
    // `sig_fp`) — REAL, unresolved overload ambiguity this engine cannot
    // break by parameter count alone, absent further evidence — OR a residual
    // same-id `source_overload_aliased` collision (caught by the `_` arm's
    // degraded-set guard below, never trusted as distinct); an
    // `UNKNOWN_ARITY`-sentinel candidate (Task 1 tri-state arity) never lands
    // in `matched` at all, since it can never equal a real call's `arity`.
    let matched: Vec<&RoutineNodeId> = candidates
        .iter()
        .filter(|rid| rid.params_count == arity)
        .collect();
    let pre_filter_count = matched.len();
    if pre_filter_count == 0 {
        // Name found but no arity-matched overload: emit Unknown rather than
        // a false-confident route to a wrong-arity candidate. Does NOT fall
        // through to extension-base / global-builtin — mirrors L3's
        // MemberNotFound stop. Reason-split Task 2: nothing to be AMBIGUOUS
        // between (zero candidates survived the arity filter) — distinct from
        // OverloadAmbiguous, which now means genuine >1-candidate ambiguity.
        return Some((
            DispatchShape::Exact,
            vec![unresolved_route(UnknownReason::ArityMismatch)],
        ));
    }

    // Per-candidate visibility filter (Task 1): partition the arity-matched
    // set by whether it is visible from `from_object`'s identity.
    let visible: Vec<&RoutineNodeId> = matched
        .iter()
        .copied()
        .filter(|rid| routine_candidate_is_visible(rid, from_object, graph, index))
        .collect();

    match visible.len() {
        0 => {
            let reason = access_exclusion_reason(obj_id, name_lc, arity, from_object, graph, index)
                .unwrap_or(UnknownReason::IndexIntegrationGap);
            Some((DispatchShape::Exact, vec![unresolved_route(reason)]))
        }
        // Overload-narrowing guard: only select the lone survivor when it was
        // ALSO the lone candidate before visibility filtering. If access
        // narrowed an originally-ambiguous (`pre_filter_count > 1`) set down
        // to one, that is NOT a safe selection — fall through to the `_` arm.
        1 if pre_filter_count == 1 => {
            let rid0 = visible[0];
            // PLAIN-DISPATCH MARKER GUARD (Task 2 round-2, the round-1
            // critical fold-in): before this fix, `abi_overload_collapsed`
            // was consulted ONLY by the chain-type-query boundary
            // (`routine_node_for_type_query`) — a marked survivor could
            // still resolve CONFIDENTLY right here via ordinary PLAIN
            // dispatch (a qualified call like `DepCollapse.Get(X)`, never
            // chained onward), an unguarded false-`Source`/`Opaque` vector.
            // `resolve_in_object` is the choke point for NAME+ARITY overload
            // SELECTION — every dispatch path that narrows a candidate SET
            // down to one by name+arity+visibility (bare-call Step 1/2/3,
            // `resolve_member`'s Object/SelfObject/Interface arms) funnels
            // through here, so placing the guard HERE closes all of them at
            // once rather than one branch at a time. It is NOT, however, the
            // only route-construction site in this module (corrected — the
            // former claim that it was "the SINGLE choke point every
            // plain-call AND qualified-member dispatch path funnels through"
            // was factually wrong, Task 2 review fix): entry-trigger dispatch
            // ([`resolve_object_run`], and `resolve_member`'s own inline
            // `Codeunit.Run(arity<=1)` special case) and multicast fan-out
            // ([`resolve_implicit_trigger`]'s trigger routes,
            // [`emit_event_flow_edges`]'s subscriber routes) each look up a
            // routine directly by ROLE rather than through this candidate-set
            // selection, so each of those four sites carries its OWN
            // [`routine_is_collapse_marked`] guard rather than inheriting
            // this one — see that helper's doc for the full enumeration. A
            // collapse-marked node is the arbitrary (or genuinely
            // indistinguishable) survivor of ≥2 raw ABI entries that
            // fingerprint-collided (see `build::dedup_routines_preserving_
            // genuine_overloads`'s doc) — its `return_type`/identity may not
            // even be the one the caller meant, so it must decline exactly
            // like a genuine >1-candidate ambiguity, never silently resolve,
            // no matter which of the five sites reached it. Reason-split Task
            // 2: this is an ABI ingestion-fidelity admission, NOT a live
            // candidate-set ambiguity — `AbiCollapsedOverload`, distinct from
            // `OverloadAmbiguous` (this guard only; the OTHER four
            // `routine_is_collapse_marked` call sites enumerated above are
            // unchanged by Task 2 and still emit `OverloadAmbiguous`).
            if routine_is_collapse_marked(rid0, graph) {
                return Some((
                    DispatchShape::Exact,
                    vec![unresolved_route(UnknownReason::AbiCollapsedOverload)],
                ));
            }
            Some((
                DispatchShape::Exact,
                vec![make_routine_route(rid0, obj_tier, surface, graph)],
            ))
        }
        // pre_filter_count == 1 was already handled by the guarded arm above;
        // reaching `visible.len() == 1` here means `pre_filter_count > 1` —
        // access narrowed an originally-ambiguous same-arity set down to one
        // survivor. NOT a safe selection (see the doc above): the decided
        // reason-split Task 2 label for this shape.
        1 => Some((
            DispatchShape::Exact,
            vec![unresolved_route(UnknownReason::AccessFilteredOverload)],
        )),
        // >1 visible: genuine unresolved ambiguity (sigfp-and-ambiguous-
        // reclassification plan, Task 4 — round-2 closer #1 PREVALIDATION):
        // every candidate must be CONCRETE — not collapse-marked (ABI or
        // source-alias) AND its constructed route must carry non-`Unknown`
        // evidence (a source-tier candidate absent from `DeclSurface` would
        // otherwise silently degrade `make_routine_route` to
        // `Unknown(IndexIntegrationGap)`) — BEFORE the
        // `DispatchShape::AmbiguousOverload` shape is ever constructed. A
        // SINGLE non-concrete candidate degrades the WHOLE set back to
        // today's pre-Task-4 `Unknown(OverloadAmbiguous)` behavior (shape
        // `Exact`, ONE route) — never partially construct, and never emit a
        // mixed/degraded `AmbiguousOverload` set (see
        // `edge::classify_obligation`'s degraded-set backstop, which exists
        // as defense-in-depth for exactly this contract, not as the live
        // mechanism). Only when EVERY candidate survives prevalidation do we
        // return one concrete route per candidate, each tagged
        // `Condition::AmbiguousDispatch` (round-1 addendum: "T4 — strict
        // `AmbiguousResolved` preconditions").
        _ => {
            // F1 (whole-branch review fix): the prevalidation contract above
            // is "NO candidate is collapse-marked, ABI **or source-alias**"
            // — check BOTH markers, not just the ABI one (see
            // `routine_is_source_aliased`'s doc for the exact laundering path
            // this closes). Cheap belt: an ID-level dedup shrink of `visible`
            // is ALSO degraded, regardless of either marker — two candidates
            // sharing one `RoutineNodeId` (however that duplication arose)
            // can never be a genuine `>1`-DISTINCT-target ambiguity, so
            // deduping down to fewer entries than routes is never a valid
            // `AmbiguousOverload` input.
            let dedup_shrinks = {
                let mut ids: Vec<&RoutineNodeId> = visible.clone();
                ids.sort();
                ids.dedup();
                ids.len() != visible.len()
            };
            let degraded = dedup_shrinks
                || visible.iter().any(|rid| {
                    routine_is_collapse_marked(rid, graph) || routine_is_source_aliased(rid, graph)
                });
            if degraded {
                return Some((
                    DispatchShape::Exact,
                    vec![unresolved_route(UnknownReason::OverloadAmbiguous)],
                ));
            }

            // Task 2 (argtype-dispatch-and-page-catalog plan; lifted off
            // SOURCE-only by Task 2 of the roadmap-closure plan): attempt a
            // fail-closed arg-type pick over `visible` BEFORE constructing
            // the AmbiguousOverload route set — see `arg_dispatch`'s module
            // doc for the full hardened rule set. Per-candidate metadata now
            // comes from DeclSurface FIRST, falling back to the ABI-AWARE route
            // (`arg_dispatch::candidate_param_infos_abi`) ONLY when DeclSurface
            // has no entry — see `candidate_param_infos_either`'s doc for why
            // "no DeclSurface entry" (not `obj_tier`) is the correct trigger.
            // `visible` already passed the `degraded` prevalidation above, so
            // every SOURCE candidate reaching the pick is individually
            // CONCRETE (non-collapse-marked, non-source-aliased) by
            // construction; the ABI route ADDITIONALLY declines its own
            // `AbiParams::CollapsedUntrusted`/`Missing` states (structural,
            // not merely a convention check) — no unknown-metadata candidate
            // is ever filtered out of the competition, its mere presence
            // degrades the whole call (module doc's cardinal rule).
            if !args.is_empty() {
                let mut candidate_params = Vec::with_capacity(visible.len());
                let mut all_known = true;
                for rid in &visible {
                    match candidate_param_infos_either(rid, graph, index, surface) {
                        Some(p) => candidate_params.push(p),
                        None => {
                            all_known = false;
                            break;
                        }
                    }
                }
                if all_known && let Some(picked_idx) = pick_candidate(args, &candidate_params) {
                    let rid0 = visible[picked_idx];
                    return Some((
                        DispatchShape::Exact,
                        vec![make_routine_route(rid0, obj_tier, surface, graph)],
                    ));
                }
            }

            let candidate_routes: Vec<Route> = visible
                .iter()
                .map(|rid| make_routine_route(rid, obj_tier, surface, graph))
                .collect();
            if candidate_routes
                .iter()
                .any(|r| r.evidence.kind() == EvidenceKind::Unknown)
            {
                return Some((
                    DispatchShape::Exact,
                    vec![unresolved_route(UnknownReason::OverloadAmbiguous)],
                ));
            }
            let routes = candidate_routes
                .into_iter()
                .map(|mut r| {
                    r.conditions.push(Condition::AmbiguousDispatch);
                    r
                })
                .collect();
            Some((DispatchShape::AmbiguousOverload, routes))
        }
    }
}

/// Per-candidate parameter metadata lookup for the arg-type pick (Task 2,
/// roadmap-closure plan) — `DeclSurface` FIRST (the pre-existing SOURCE-tier
/// route, unchanged), then the ABI-AWARE fallback
/// ([`candidate_param_infos_abi`]) ONLY when `DeclSurface` has no entry for
/// `rid`. "No `DeclSurface` entry" — not `rid.object`'s tier — is deliberately
/// the trigger: every SOURCE-tier routine has a `DeclSurface` entry and every
/// `TrustTier::SymbolOnly` routine never does (see the `arg_dispatch` module
/// doc's "SOURCE tier only" section, now extended by the ABI fallback below
/// it), so the two routes can never disagree about which one applies to a
/// given candidate — there is exactly one correct answer per `rid`, never a
/// choice. Returns `None` — "no metadata", degrading the WHOLE call per
/// `arg_dispatch`'s cardinal rule — when NEITHER route has complete metadata
/// for `rid`.
fn candidate_param_infos_either(
    rid: &RoutineNodeId,
    graph: &ProgramGraph,
    index: &ResolveIndex,
    surface: &DeclSurface,
) -> Option<Vec<ParamDispatchInfo>> {
    if let Some(meta) = surface.get(rid) {
        return candidate_param_infos(meta, &rid.object, graph, index);
    }
    candidate_param_infos_abi(rid, graph, index)
}

/// Whether a single, CONCRETE arity-matched routine candidate `rid` is
/// visible from the calling object's identity `from_object` — the
/// PER-CANDIDATE counterpart to [`object_has_visible_member_candidate`]
/// (which only answers EXISTENTIALLY: "does SOME visible candidate exist").
/// [`resolve_in_object`] needs to know WHICH SPECIFIC candidate(s) survive so
/// it can apply the overload-narrowing guard rather than blindly picking the
/// sole visible survivor of an originally-ambiguous set.
///
/// Mirrors the per-`Access` rule [`object_has_visible_member_candidate`]
/// already established (see that function's doc for the full soundness
/// rationale) — RESOLVED OBJECT IDENTITY, never a lowercased-name comparison:
///
/// - [`Access::Public`] → always visible.
/// - [`Access::Local`] → visible ONLY when `rid.object == *from_object` (the
///   candidate's declaring object IS the calling object itself).
/// - [`Access::Internal`] → visible when `rid.object.app == from_object.app`
///   (app-scoped), OR when `rid.object.app`'s manifest declares
///   `from_object.app` a friend via `<InternalsVisibleTo>` (Task 1.5 —
///   see [`internal_visible_across`]). Cross-app `internal` from a
///   true-stranger app still fails closed.
/// - [`Access::Protected`] → visible when `rid.object == *from_object` (self)
///   OR `index.object_extends(graph, from_object, &rid.object)` is `true`
///   (`from_object` is a DIRECT, kind-compatible extension of the
///   candidate's declaring object).
/// - Lookup miss (`None`) → fails closed (excluded), never assumed visible.
///
/// A pure DELEGATE to [`object_access_visible_from`] (Task 2 fold-in, review
/// finding from Task 1): the two functions applied the IDENTICAL per-`Access`
/// rule as two independently-maintained copies — this one keyed by
/// `RoutineNodeId`, that one by `ObjectNodeId` + an already-looked-up
/// `Access`. Two copies of the same rule is exactly the kind of latent drift
/// vector this plan closes elsewhere (see [`object_access_visible_from`]'s
/// own doc) — collapsing to one predicate means a future rule change can
/// never update one copy and silently miss the other.
fn routine_candidate_is_visible(
    rid: &RoutineNodeId,
    from_object: &ObjectNodeId,
    graph: &ProgramGraph,
    index: &ResolveIndex,
) -> bool {
    object_access_visible_from(
        &rid.object,
        lookup_routine_access(graph, rid),
        from_object,
        graph,
        index,
    )
}

/// Whether `caller_app` may see `exposing_app`'s `internal` members —
/// same-app (AL's default `internal` scoping), OR `exposing_app`'s own
/// manifest declares `caller_app` a friend via
/// `<InternalsVisibleTo><Module .../></InternalsVisibleTo>` (Task 1.5).
///
/// Friendship is declared BY the app EXPOSING the internals, never inferred
/// from the reverse direction: `graph.friends` is keyed by the exposing
/// app's [`AppRef`], so `friends[A].contains(B)` means "A trusts B", and
/// does NOT imply `friends[B].contains(A)` — a caller B that itself grants A
/// friend access does not thereby gain access to B's own internals from A's
/// side. See [`crate::program::build::build_program_graph`] Step 3b for how
/// `graph.friends` is populated (GUID-first, name+publisher-fallback
/// resolution of each `<Module>` entry against the snapshot; entries whose
/// app is outside the closure are silently skipped, open-world).
fn internal_visible_across(exposing_app: AppRef, caller_app: AppRef, graph: &ProgramGraph) -> bool {
    exposing_app == caller_app
        || graph
            .friends
            .get(&exposing_app)
            .is_some_and(|f| f.contains(&caller_app))
}

/// Whether `obj_id` (at trust tier `obj_tier`) carries a visible source/ABI
/// candidate routine matching `method_lc`/`arity` — used by the Record-receiver
/// source-shadows-catalog precedence check (beyond-1B.3b Task 1) to determine
/// CARDINALITY across a multi-object scope (base table ∪ TableExtensions)
/// WITHOUT committing to a route.
///
/// A SymbolOnly (ABI/dep) object counts ANY name match as a candidate —
/// EXISTENCE only, deliberately arity-deferred (Task 1: a same-name ABI
/// sibling might carry the `UNKNOWN_ARITY` sentinel, or simply a different
/// arity than the one being probed here, so a name-only scan is the correct,
/// conservative existence signal; [`resolve_in_object`] is where real
/// arity-EXACT matching happens for what actually gets to emit). A source
/// tier object counts only an EXACT arity match.
fn object_has_member_candidate(
    obj_id: &ObjectNodeId,
    obj_tier: TrustTier,
    method_lc: &str,
    arity: usize,
    index: &ResolveIndex,
) -> bool {
    let candidates = index.routines_in_object(obj_id, method_lc);
    if candidates.is_empty() {
        return false;
    }
    if obj_tier == TrustTier::SymbolOnly {
        return true;
    }
    candidates.iter().any(|rid| rid.params_count == arity)
}

/// Look up the declared [`Access`] of `rid` in `graph.routines` (already
/// sorted by `RoutineNodeId` — binary-searchable, mirroring
/// `make_routine_route`'s existing `graph.routines.binary_search_by` lookup
/// pattern). Returns `None` on a lookup miss — should never happen for a
/// `RoutineNodeId` sourced from `index.routines_in_object` (the index is
/// built directly from `graph.routines`), but if it ever does, the caller
/// ([`object_has_visible_member_candidate`]) fails closed rather than
/// assuming the routine is visible.
fn lookup_routine_access(graph: &ProgramGraph, rid: &RoutineNodeId) -> Option<Access> {
    graph
        .routines
        .binary_search_by(|probe| probe.id.cmp(rid))
        .ok()
        .map(|i| graph.routines[i].access)
}

/// The shared per-`Access` visibility predicate: is a candidate DECLARED IN
/// `obj_id` with declared access `access` visible from the CALLING object's
/// identity `from_object`? Factored out (Task 1) so [`object_has_visible_
/// member_candidate`]'s source/ABI arity-filtered scan and its SymbolOnly
/// NAME-ONLY scan share EXACTLY one rule — two independent copies could
/// silently drift and reopen the exact soundness gap this function closes.
/// [`routine_candidate_is_visible`] (the PER-CANDIDATE selection rule
/// `resolve_in_object` uses) DELEGATES to this same predicate too (Task 2
/// fold-in) — every per-candidate/per-object visibility check in this module
/// now traces back to this ONE function.
///
/// RESOLVED OBJECT IDENTITY, never a lowercased-name comparison — every
/// branch compares [`ObjectNodeId`]s or [`AppRef`]s, both derived from
/// `graph`/`index` identity, never from `Origin`/source text.
///
/// - [`Access::Public`] → always visible.
/// - [`Access::Local`] → visible ONLY when `obj_id == from_object` (the
///   candidate's declaring object IS the calling object itself — AL's
///   `local` is OBJECT-scoped, not app-scoped; this was the first latent
///   false-`Source` beyond-1B.3b Task 1 closed: the pre-fix code treated ANY
///   same-app candidate as visible, so a same-app but DIFFERENT object's
///   `local` procedure false-resolved to `Source`).
/// - [`Access::Internal`] → visible when `obj_id.app == from_object.app`
///   (app-scoped), OR when `obj_id.app`'s manifest declares
///   `from_object.app` a friend via `<InternalsVisibleTo>` — AL's
///   friend-app exception, modeled by [`internal_visible_across`] (Task
///   1.5, closing the over-decline the app-scoped-only version of this rule
///   left: measurement proved 100% of the resulting `InternalNotVisible`
///   bucket was AL-LEGAL friend calls, not genuine access violations).
///   Cross-app `internal` from a true stranger (no friend declaration in
///   either direction) still fails closed to `Unknown`.
/// - [`Access::Protected`] → visible when `obj_id == from_object` (self) OR
///   `index.object_extends(graph, from_object, obj_id)` is `true` — `from_object`
///   is a DIRECT, kind-compatible extension of the candidate's declaring
///   object (see [`ResolveIndex::object_extends`] for the full DIRECT +
///   KIND-COMPATIBLE + never-reverse + never-peer contract). This closes the
///   second latent false-`Source` beyond-1B.3b Task 1 closed: the pre-fix
///   code left `Protected` completely unfiltered for any same-app candidate,
///   including a same-app-but-unrelated object AND a PEER extension of the
///   same base (the sibling-bleed case). SAME rule for a SymbolOnly base
///   (Task 1): AL lets a workspace extension of a dep object call the dep's
///   `protected` members, and `object_extends` is already tier-agnostic.
/// - Lookup miss (`None`) → fails closed (excluded), never assumed visible.
fn object_access_visible_from(
    obj_id: &ObjectNodeId,
    access: Option<Access>,
    from_object: &ObjectNodeId,
    graph: &ProgramGraph,
    index: &ResolveIndex,
) -> bool {
    match access {
        Some(Access::Public) => true,
        Some(Access::Local) => obj_id == from_object,
        Some(Access::Internal) => internal_visible_across(obj_id.app, from_object.app, graph),
        Some(Access::Protected) => {
            obj_id == from_object || index.object_extends(graph, from_object, obj_id)
        }
        None => false,
    }
}

/// Like [`object_has_member_candidate`], but additionally excludes a
/// candidate whose declared [`Access`] is not visible from `from_object` —
/// the caller-identity-aware visibility check (beyond-1B.3b Task 1,
/// superseding the app-scoped Task 2 version). See [`object_access_visible_
/// from`] for the full per-`Access` rule (shared by both branches below).
///
/// # SymbolOnly (Task 1 — NAME-ONLY `.any()` scan, no shortcut)
///
/// `access` is now populated from the real ABI (`IsProtected`/local/internal
/// drop at ingestion — `abi_ingest::ingest_abi`), so a `protected` ABI
/// routine is a genuine, non-trivial visibility check, not a hardcoded
/// `Public` no-op. The scan is deliberately NAME-ONLY (the UN-arity-filtered
/// candidate list), mirroring [`object_has_member_candidate`]'s own
/// arity-deferred SymbolOnly existence check: a protected first sibling must
/// never hide a visible public one purely by JSON-array order, and a real
/// arity mismatch on ONE same-name candidate must not suppress a DIFFERENT
/// same-name candidate that IS both visible and arity-correct. This function
/// answers EXISTENTIALLY ("is SOME visible candidate reachable") — it is an
/// existence/diagnostics gate consumed by callers to decide whether to even
/// ATTEMPT `resolve_in_object`, never edge evidence by itself:
/// `resolve_in_object` alone performs the arity-EXACT, per-candidate
/// selection that decides what (if anything) actually emits.
fn object_has_visible_member_candidate(
    obj_id: &ObjectNodeId,
    obj_tier: TrustTier,
    method_lc: &str,
    arity: usize,
    from_object: &ObjectNodeId,
    graph: &ProgramGraph,
    index: &ResolveIndex,
) -> bool {
    if !object_has_member_candidate(obj_id, obj_tier, method_lc, arity, index) {
        return false;
    }
    if obj_tier == TrustTier::SymbolOnly {
        return index
            .routines_in_object(obj_id, method_lc)
            .iter()
            .any(|rid| {
                object_access_visible_from(
                    obj_id,
                    lookup_routine_access(graph, rid),
                    from_object,
                    graph,
                    index,
                )
            });
    }
    index
        .routines_in_object(obj_id, method_lc)
        .iter()
        .filter(|rid| rid.params_count == arity)
        .any(|rid| {
            object_access_visible_from(
                obj_id,
                lookup_routine_access(graph, rid),
                from_object,
                graph,
                index,
            )
        })
}

/// Diagnostic companion to [`object_has_visible_member_candidate`] (Task 3):
/// when NO visible candidate exists for `method_lc`/`arity` in `obj_id`,
/// determine WHY the most specific excluded candidate (if any) was excluded
/// — `Local`/`Internal`/`Protected` access not visible from `from_object`'s
/// identity. Returns `None` when no same-name/arity candidate exists in
/// `obj_id` at all (genuine absence, not a visibility exclusion) — mirrors
/// [`object_has_visible_member_candidate`]'s own per-access rule exactly, so
/// the two never disagree on WHETHER a candidate is visible, only on WHY not.
///
/// Tier-agnostic (Task 1): SymbolOnly `access` is now populated from the real
/// ABI (`IsProtected`/local+internal drop at ingestion), so a SymbolOnly
/// candidate can be genuinely access-excluded (most commonly `protected`,
/// from a non-extension caller) — the previous hardcoded "SymbolOnly is
/// always Public, never access-excluded" short-circuit is gone; the
/// arity-filtered scan + per-`Access` reason derivation below apply
/// identically to both tiers (every call site here is already arity-KNOWN,
/// so no separate name-only variant is needed, unlike [`object_has_visible_
/// member_candidate`]'s existence-only scan).
fn access_exclusion_reason(
    obj_id: &ObjectNodeId,
    method_lc: &str,
    arity: usize,
    from_object: &ObjectNodeId,
    graph: &ProgramGraph,
    index: &ResolveIndex,
) -> Option<UnknownReason> {
    index
        .routines_in_object(obj_id, method_lc)
        .iter()
        .filter(|rid| rid.params_count == arity)
        .find_map(|rid| match lookup_routine_access(graph, rid) {
            Some(Access::Local) if obj_id != from_object => Some(UnknownReason::LocalNotVisible),
            Some(Access::Internal)
                if !internal_visible_across(obj_id.app, from_object.app, graph) =>
            {
                Some(UnknownReason::InternalNotVisible)
            }
            Some(Access::Protected)
                if obj_id != from_object && !index.object_extends(graph, from_object, obj_id) =>
            {
                Some(UnknownReason::ProtectedNotVisible)
            }
            _ => None,
        })
}

/// The outcome of a [`resolve_in_extendable_scope`] search (Table/Page/Report,
/// via [`resolve_in_table_scope`]/[`resolve_in_page_scope`]/
/// [`resolve_in_report_scope`]) — sufficient for the caller to know not just
/// WHETHER it resolved, but on decline, WHY (Task 3's diagnostic
/// [`UnknownReason`] payload).
enum TableScopeOutcome {
    /// Exactly one visible candidate — resolved.
    Resolved(DispatchShape, Vec<Route>),
    /// `>1` visible candidates — honest ambiguous `Unknown`. Callers MUST
    /// return this immediately (never fall through to the catalog — source
    /// ambiguity still shadows a same-named intrinsic).
    Ambiguous,
    /// Zero visible candidates. `access_excluded` is `Some(reason)` when a
    /// same-name/arity candidate existed in scope but was excluded by the
    /// caller-identity access filter — the most specific decline reason
    /// available; `None` when no candidate existed in scope at all (name
    /// genuinely absent — the caller should fall through / use its own
    /// default reason).
    NotVisible {
        access_excluded: Option<UnknownReason>,
    },
}

/// The ONE intentional divergence between the Table/Page/Report extendable-
/// scope resolvers (roadmap-closure plan, Task 1 — see the pre-refactor
/// behavioral inventory in the task report for the dimension-by-dimension
/// proof that this is the only place they differ): what to do when ZERO
/// scope objects (base ∪ visible extensions) carry an arity+visibility
/// match, but a diagnostic is still owed. Passed to
/// [`resolve_in_extendable_scope`] by each thin per-kind wrapper.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ZeroMatchStrategy {
    /// The Table/Record policy (pre-Task-1 behavior, unchanged): scan `scope`
    /// for the first same-name/EXACT-arity candidate that exists but was
    /// excluded by the caller-identity access filter, and report that as
    /// `access_excluded` — never forward to [`resolve_in_object`]. A
    /// same-name-WRONG-arity candidate is invisible to this scan (it filters
    /// on `arity` too), so a pure arity mismatch on the Table/Record arm
    /// falls through to the caller's own generic default reason
    /// (`access_excluded: None`), exactly as it always has.
    AccessExcludedReason,
    /// The Page/Report policy: when no scope object has a matching arity,
    /// check whether the routine NAME (any arity) is declared SOMEWHERE in
    /// scope, and if so forward to the first (deterministic, scope-order)
    /// name-bearing object so [`resolve_in_object`]'s own internal
    /// diagnostic — `ArityMismatch`, `AccessFilteredOverload`,
    /// `LocalNotVisible`, … — survives exactly as a single-object dispatch
    /// would have produced it. See [`resolve_in_page_scope`]'s doc for the
    /// full "why Page/Report diverges from Table" rationale (the
    /// `ArityMismatch`-preservation requirement) and the al-compile probe
    /// (`.superpowers/sdd/task-1-report.md`) that grounds extending this
    /// policy to Report: AL0135 ("no argument given that corresponds to the
    /// required formal parameter") is the compiler's OWN distinct diagnostic
    /// class for a wrong-arity call to a real, name-resolved routine —
    /// disjoint from AL0132 ("does not contain a definition for") — so
    /// collapsing a wrong-arity ReportExtension call into a bare
    /// `MemberNotFound` would misrepresent what the real compiler reports,
    /// exactly the failure mode this variant exists to avoid.
    PreserveArityMismatch,
}

/// Resolve `name_lc`/`arity` against a VISIBILITY-SCOPED extendable-object
/// scope: `base_id` plus every extension of it (as returned by
/// `extensions_of`) reachable in `from_object`'s compile-time app dependency
/// closure. The shared engine behind [`resolve_in_table_scope`]/
/// [`resolve_in_page_scope`]/[`resolve_in_report_scope`] (roadmap-closure
/// plan, Task 1 — unifies what were two ~90%-identical hand-copies,
/// generalized to a third kind via [`ZeroMatchStrategy`] rather than a third
/// copy; see the task report's pre-refactor behavioral inventory for the
/// dimension-by-dimension proof that the zero-match branch below is the ONLY
/// place Table/Page ever diverged).
///
/// # Visibility scoping (the beyond-1B.3b Task 2 soundness fix)
///
/// Two INDEPENDENT fail-closed filters narrow the raw scope before
/// cardinality is counted — either one dropping a candidate can turn a false
/// `Source` into a correct decline:
///
/// 1. **Closure filter.** `extensions_of` (whichever of
///    [`ResolveIndex::table_extensions_of`]/[`ResolveIndex::page_extensions_of`]/
///    [`ResolveIndex::report_extensions_of`] the caller passes) is
///    whole-snapshot (`WorldMode::AnalyzedSnapshot` — no app-scoping). An
///    extension declared in an app OUTSIDE `from_object`'s transitive
///    dependency closure is a symbol `from_object`'s own app never imported —
///    the real AL compiler could never have resolved a call to it. Such an
///    extension is dropped from `scope` entirely, not merely deprioritized.
///    The base object (`base_id`) is gated the same way, defense-in-depth (it
///    is normally already closure-validated by the receiver-inference stage
///    that produced it — see `receiver::resolve_source_table_ref` — but
///    re-checking here makes this helper safe to call independent of that
///    upstream guarantee).
/// 2. **Access filter.** A candidate procedure whose declared [`Access`] is
///    not visible from `from_object`'s identity is excluded from the
///    candidate count — `Local` requires `from_object` to BE the candidate's
///    declaring object, `Internal` requires the same app, `Protected`
///    requires self OR a direct kind-compatible extension relationship (see
///    [`object_has_visible_member_candidate`] for the full per-access
///    rationale). Tier-agnostic: a SymbolOnly candidate's `access` carries the
///    real ABI `IsProtected` modifier, so this filter can genuinely exclude a
///    SymbolOnly candidate too.
///
/// # Aggregate-then-adjudicate (round-1 review addendum, BINDING)
///
/// Every visible candidate object (base ∪ every visible extension) is
/// collected FIRST; a base-vs-extension or extension-vs-extension exact-
/// duplicate same-arity pair is a genuine `>1` ambiguity, never first-wins.
/// **AL0440** ("already defines a method ... with the same parameter types")
/// makes an EXACT duplicate signature a compile error in real AL for
/// Page/Report, for BOTH shapes — base-vs-extension AND cross-extension
/// alike (corrected, Task 3 roadmap-closure plan: a live `al compile` probe,
/// same methodology as Task 1's, showed the SAME AL0440 diagnostic for both
/// cases; the AL0115/AL0226 citations this comment previously carried were
/// never independently probed and were wrong) — this ambiguity path is
/// DEFENSIVE-ONLY against malformed/synthetic source there, not a live
/// production case (for Table it is the pre-existing, unchanged live
/// behavior).
///
/// # Cardinality
///
/// - `>1` DISTINCT scope objects each carrying a visible arity-EXACT match →
///   [`TableScopeOutcome::Ambiguous`] (never pick-first; source/extension
///   ambiguity still shadows a same-named intrinsic).
/// - Exactly 1 → [`TableScopeOutcome::Resolved`], a single `Source`/`Abi`/
///   `Opaque` route via [`resolve_in_object`].
/// - 0 arity+visibility matches anywhere in scope → dispatch on `zero_match`
///   (see [`ZeroMatchStrategy`]'s doc for the two policies and why they
///   differ; this is the ONLY divergence point between the three
///   per-kind wrappers).
///
/// Deterministic: `scope` is explicitly sorted by `ObjectNodeId` before
/// cardinality is counted.
#[allow(clippy::too_many_arguments)] // 8 pre-existing params + the extension-index fn-pointer + the zero-match policy (Task 1, roadmap-closure plan): each is a distinct identity/lookup/policy input, grouping would obscure call sites.
fn resolve_in_extendable_scope(
    from_object: &ObjectNode,
    base_id: ObjectNodeId,
    name_lc: &str,
    arity: usize,
    graph: &ProgramGraph,
    index: &ResolveIndex,
    surface: &DeclSurface,
    args: &[ArgDispatchInfo],
    extensions_of: for<'a> fn(&'a ResolveIndex, &str) -> &'a [ObjectNodeId],
    zero_match: ZeroMatchStrategy,
) -> TableScopeOutcome {
    let closure = graph.topology.closure(from_object.id.app);

    if !closure.contains(&base_id.app) {
        return TableScopeOutcome::NotVisible {
            access_excluded: None,
        };
    }
    let Some((base_tier, base_name_lc)) = graph
        .objects
        .iter()
        .find(|o| o.id == base_id)
        .map(|o| (o.tier, o.name.fold_identifier()))
    else {
        return TableScopeOutcome::NotVisible {
            access_excluded: None,
        };
    };

    // Visible scope: the base object plus every extension of it that is
    // reachable in `from_object`'s app dependency closure.
    let mut scope: Vec<(ObjectNodeId, TrustTier)> = vec![(base_id.clone(), base_tier)];
    for ext_id in extensions_of(index, &base_name_lc) {
        if !closure.contains(&ext_id.app) {
            // Outside from_object's dependency closure: invisible, not a
            // candidate (the Task 2 soundness fix).
            continue;
        }
        let Some(ext_tier) = graph
            .objects
            .iter()
            .find(|o| &o.id == ext_id)
            .map(|o| o.tier)
        else {
            continue;
        };
        scope.push((ext_id.clone(), ext_tier));
    }
    // Deterministic ordering (candidate vectors sorted by stable ObjectNodeId).
    scope.sort_by(|(a, _), (b, _)| a.cmp(b));

    let mut candidate_objects = scope.iter().filter(|(oid, tier)| {
        object_has_visible_member_candidate(
            oid,
            *tier,
            name_lc,
            arity,
            &from_object.id,
            graph,
            index,
        )
    });
    let first = candidate_objects.next();
    let second = candidate_objects.next();

    match (first, second) {
        (Some(_), Some(_)) => TableScopeOutcome::Ambiguous,
        (Some((oid, tier)), None) => match resolve_in_object(
            oid,
            *tier,
            name_lc,
            arity,
            &from_object.id,
            graph,
            index,
            surface,
            args,
        ) {
            Some((shape, routes)) => TableScopeOutcome::Resolved(shape, routes),
            // Defensive: `object_has_visible_member_candidate` already
            // confirmed a visible arity match exists, so `resolve_in_object`
            // should always return `Some` here.
            None => TableScopeOutcome::NotVisible {
                access_excluded: Some(UnknownReason::IndexIntegrationGap),
            },
        },
        (None, _) => match zero_match {
            ZeroMatchStrategy::AccessExcludedReason => {
                // Zero visible candidates in the whole scope — diagnose WHY,
                // in scope order (deterministic): the first same-name/arity
                // candidate that exists but is access-excluded, if any.
                let access_excluded = scope.iter().find_map(|(oid, _tier)| {
                    access_exclusion_reason(oid, name_lc, arity, &from_object.id, graph, index)
                });
                TableScopeOutcome::NotVisible { access_excluded }
            }
            ZeroMatchStrategy::PreserveArityMismatch => {
                // No scope object has BOTH a matching arity and visibility
                // for `name_lc`. Before declaring true absence, check whether
                // the bare NAME (any arity, tier-agnostic existence — mirrors
                // `resolve_in_object`'s own initial `index.routines_in_object`
                // check) exists anywhere in scope, so a genuine arity/access
                // diagnostic is preserved rather than collapsed into a bare
                // "not found". A deterministic (scope-order) pick among
                // multiple name-bearing-but-non-matching objects is safe —
                // none of them can produce a `Source` route: by construction,
                // `resolve_in_object` on a name-bearing-but-non-arity-
                // visible-matching object always returns an `Unknown`-shaped
                // route (never a real match) — the arity+visibility scan
                // above already proved no candidate on ANY scope object
                // clears that bar, so this fallback object's own internal
                // arity/visibility filter can only reach its `ArityMismatch`
                // or `visible.len() == 0` (access-exclusion) branches.
                match scope
                    .iter()
                    .find(|(oid, _)| !index.routines_in_object(oid, name_lc).is_empty())
                {
                    Some((oid, tier)) => match resolve_in_object(
                        oid,
                        *tier,
                        name_lc,
                        arity,
                        &from_object.id,
                        graph,
                        index,
                        surface,
                        args,
                    ) {
                        Some((shape, routes)) => TableScopeOutcome::Resolved(shape, routes),
                        None => TableScopeOutcome::NotVisible {
                            access_excluded: None,
                        },
                    },
                    None => TableScopeOutcome::NotVisible {
                        access_excluded: None,
                    },
                }
            }
        },
    }
}

/// Resolve `name_lc`/`arity` against the VISIBILITY-SCOPED table scope: the
/// base table `table_id` plus every `TableExtension` of it that is reachable
/// in `from_object`'s compile-time app dependency closure (beyond-1B.3b Task
/// 2; extracted from `resolve_member`'s `Record` arm so a future caller with
/// the same scope+cardinality need — e.g. `resolve_bare`'s implicit-Rec
/// lookup — can reuse the identical algorithm rather than re-deriving it).
/// A thin wrapper over [`resolve_in_extendable_scope`] (roadmap-closure plan,
/// Task 1) with [`ZeroMatchStrategy::AccessExcludedReason`] — the pre-Task-1
/// Table/Record policy: a same-name-WRONG-arity candidate never counts as
/// "present" for diagnostic purposes on this arm (see that variant's doc).
/// See [`resolve_in_extendable_scope`]'s doc for the full visibility-scoping,
/// aggregate-then-adjudicate, and cardinality rules (shared by all three
/// kinds).
#[allow(clippy::too_many_arguments)] // 7 pre-existing params + `args` (Task 2, argtype-dispatch-and-page-catalog plan).
fn resolve_in_table_scope(
    from_object: &ObjectNode,
    table_id: ObjectNodeId,
    name_lc: &str,
    arity: usize,
    graph: &ProgramGraph,
    index: &ResolveIndex,
    surface: &DeclSurface,
    args: &[ArgDispatchInfo],
) -> TableScopeOutcome {
    resolve_in_extendable_scope(
        from_object,
        table_id,
        name_lc,
        arity,
        graph,
        index,
        surface,
        args,
        ResolveIndex::table_extensions_of,
        ZeroMatchStrategy::AccessExcludedReason,
    )
}

/// Resolve `name_lc`/`arity` against the VISIBILITY-SCOPED Page **object**
/// scope: the base Page `page_id` plus every `PageExtension` of it reachable
/// in `from_object`'s compile-time app dependency closure — the `Page` analog
/// of [`resolve_in_table_scope`] (pageext-merge-and-final-residual plan,
/// Task 1). Closes the engine gap the plan's grounding report identified: a
/// `PageExtension`'s routines are indexed under the EXTENSION's own
/// `ObjectNodeId` (`node_extract::extract_nodes`), so a base-Page-typed
/// receiver (`ReceiverType::Object{kind: Page, ..}`) could never reach them
/// via a plain [`resolve_in_object`] call on the base alone. A thin wrapper
/// over [`resolve_in_extendable_scope`] (roadmap-closure plan, Task 1) with
/// [`ZeroMatchStrategy::PreserveArityMismatch`].
///
/// # Divergence from [`resolve_in_table_scope`]: `ArityMismatch` preservation
///
/// Unlike the Table/Record arm — whose cardinality check folds arity-EXACT
/// matching into object EXISTENCE, so a same-name-wrong-arity candidate never
/// counts as "present" (meaning `resolve_in_object`'s own `ArityMismatch`
/// branch is provably unreachable through that path; see that function's
/// `Defensive:` comment) — this policy preserves the PRE-TASK-1 per-object
/// `ArityMismatch`/access-exclusion diagnostic quality: when NO scope object
/// has an arity+visibility match ANYWHERE, but the routine NAME (any arity)
/// is declared somewhere in scope, the first (deterministic, scope-order)
/// name-bearing object is still forwarded to [`resolve_in_object`] so its own
/// internal per-object diagnostic (`ArityMismatch`, `AccessFilteredOverload`,
/// `LocalNotVisible`, …) survives exactly as the single-object dispatch
/// produced it pre-merge — required so the merge is a pure ADDITIVE gain
/// (extensions become reachable) and never a diagnostic regression for a
/// base-only call whose arity happens to be wrong. See
/// [`ZeroMatchStrategy::PreserveArityMismatch`]'s doc for the al-compile
/// probe that independently grounds extending this SAME policy to Report.
///
/// See [`resolve_in_extendable_scope`]'s doc for the full visibility-scoping,
/// aggregate-then-adjudicate, and cardinality rules (shared by all three
/// kinds).
#[allow(clippy::too_many_arguments)] // 7 pre-existing params + `args`, mirrors resolve_in_table_scope's identical attribute.
fn resolve_in_page_scope(
    from_object: &ObjectNode,
    page_id: ObjectNodeId,
    name_lc: &str,
    arity: usize,
    graph: &ProgramGraph,
    index: &ResolveIndex,
    surface: &DeclSurface,
    args: &[ArgDispatchInfo],
) -> TableScopeOutcome {
    resolve_in_extendable_scope(
        from_object,
        page_id,
        name_lc,
        arity,
        graph,
        index,
        surface,
        args,
        ResolveIndex::page_extensions_of,
        ZeroMatchStrategy::PreserveArityMismatch,
    )
}

/// Resolve `name_lc`/`arity` against the VISIBILITY-SCOPED Report **object**
/// scope: the base Report `report_id` plus every `ReportExtension` of it
/// reachable in `from_object`'s compile-time app dependency closure — the
/// `Report` analog of [`resolve_in_page_scope`] (roadmap-closure plan,
/// Task 1; previously deferred — see the pageext-merge-and-final-residual
/// plan's Task 1 doc note, now superseded by this function). Closes the same
/// class of engine gap: a `ReportExtension`'s routines are indexed under the
/// EXTENSION's own `ObjectNodeId` (`node_extract::extract_nodes`), so a
/// base-Report-typed receiver (`ReceiverType::Object{kind: Report, ..}`)
/// could never reach them via a plain [`resolve_in_object`] call on the base
/// alone. A thin wrapper over [`resolve_in_extendable_scope`] with
/// [`ZeroMatchStrategy::PreserveArityMismatch`] — the SAME policy as Page,
/// grounded independently for Report by an `al compile` probe (the
/// grammar repo's minimal-probe methodology,
/// `tree-sitter-al/CLAUDE.md` § "Validating AL Syntax Questions"): a
/// same-app `ReportExtension` procedure called through a base-Report-typed
/// variable receiver (`R: Report "ProbeReport"; R.OneArgProc(5);`) compiles
/// cleanly (positive control — the merge itself is real, compiler-verified
/// AL semantics, not just an engine assumption), and calling it with the
/// WRONG arity (`R.OneArgProc();`) reports `AL0135: There is no argument
/// given that corresponds to the required formal parameter 'X' of
/// 'OneArgProc(Integer)'` — a DISTINCT diagnostic class from the genuine
/// "member not found" case (`AL0132: 'Report ProbeReport' does not contain a
/// definition for 'NoSuchProc'`, confirmed on the same fixture). The real
/// compiler treats a wrong-arity call to a name-resolved routine as its own
/// diagnostic category, never collapsing it into "not found" — exactly what
/// `PreserveArityMismatch` (as opposed to Table's `AccessExcludedReason`)
/// preserves. Full probe transcript in `.superpowers/sdd/task-1-report.md`.
///
/// See [`resolve_in_extendable_scope`]'s doc for the full visibility-scoping,
/// aggregate-then-adjudicate, and cardinality rules (shared by all three
/// kinds), and [`resolve_in_page_scope`]'s doc for the full `ArityMismatch`-
/// preservation rationale this function reuses unchanged.
#[allow(clippy::too_many_arguments)] // 7 pre-existing params + `args`, mirrors resolve_in_table_scope's/resolve_in_page_scope's identical attribute.
fn resolve_in_report_scope(
    from_object: &ObjectNode,
    report_id: ObjectNodeId,
    name_lc: &str,
    arity: usize,
    graph: &ProgramGraph,
    index: &ResolveIndex,
    surface: &DeclSurface,
    args: &[ArgDispatchInfo],
) -> TableScopeOutcome {
    resolve_in_extendable_scope(
        from_object,
        report_id,
        name_lc,
        arity,
        graph,
        index,
        surface,
        args,
        ResolveIndex::report_extensions_of,
        ZeroMatchStrategy::PreserveArityMismatch,
    )
}

/// Compute the implicit-`Rec` table `ObjectNodeId` for `resolve_bare`'s Step 3
/// (bare unqualified calls implicitly dispatching to `Rec` — beyond-1B.3b
/// Task 3), by `from_object`'s kind. Reuses the SAME fail-closed per-kind
/// lookups `infer_implicit_rec` (`receiver.rs`) already established for the
/// EXPLICIT `Rec.Foo()` member-call case (Tasks 5-7) — a guessed table is the
/// cardinal sin either way, so there is exactly one correct answer per kind
/// and no reason to re-derive it.
///
/// Deliberately narrower than `infer_implicit_rec`: `Codeunit` (`TableNo`) is
/// NOT handled here. `resolve_bare`'s Step 3 caller already structurally
/// excludes every kind but `{Table, Page, TableExtension, PageExtension}`
/// before this is ever called (AL's bare-implicit-dispatch fallback is a
/// Page/Table source-record mechanism, not a Codeunit `TableNo` one), so this
/// function is never invoked for a Codeunit — its `_ => None` arm is
/// defense-in-depth, not a live path.
///
/// Returns `None` when there is no unique in-closure table (no declared
/// property, ambiguous cross-app name, out-of-closure, unresolved) — the
/// caller falls through to Step 4 rather than guess.
///
/// `pub(crate)`: also reused by `receiver.rs`'s Step 3a (pageext-merge-and-
/// final-residual plan, Task 2) — the implicit-Rec bare-FIELD arm widened
/// from Table/TableExtension to also cover Page/PageExtension needs the
/// EXACT same per-kind table lookup this function already establishes for
/// the bare-CALL case; re-deriving it a second time would risk the two
/// falling out of sync on a future kind addition.
pub(crate) fn implicit_rec_table_id(
    from_object: &ObjectNode,
    graph: &ProgramGraph,
    index: &ResolveIndex,
) -> Option<ObjectNodeId> {
    match from_object.id.kind {
        ObjectKind::Table => Some(from_object.id.clone()),
        ObjectKind::Page => from_object
            .source_table
            .as_ref()
            .and_then(|r| resolve_source_table_ref(from_object.id.clone(), r, graph, index)),
        ObjectKind::TableExtension => resolve_tableext_base_table(from_object, graph, index),
        ObjectKind::PageExtension => resolve_pageext_base_source_table(from_object, graph, index),
        _ => None,
    }
}

/// Names compiler-GROUNDED to have **no bare-call form anywhere in AL**
/// (pageext-merge-and-final-residual plan, Task 2 — the round-1 review
/// addenda's GLOBAL, unconditional narrowing). Every entry here is a
/// documented AL method that is ALWAYS reached through an explicit receiver
/// (`CurrPage.Update()`, `MyCodeunit.Run()`, `Page.RunModal(...)`) — never a
/// bare unqualified call — in EVERY context checked: page trigger/action/
/// procedure, pageextension, table/tableextension, report/reportextension
/// (+`CurrReport` analogs), XmlPort, codeunit `OnRun`.
///
/// # The compiler-grounding matrix
///
/// This set is EXACTLY `member_catalog::PAGE_INSTANCE` (verified: every one
/// of its 19 members is ALSO in `GLOBAL_BUILTIN_METHODS` — the union-of-all-
/// 97-types catalog `is_global_builtin` treats as a sound bare-callable
/// allowlist). The generator (`tools/gen-al-builtins/Program.cs`) extracts
/// `TypeName_MethodName` keys from the AL compiler DLL's
/// `ClassDocumentationResources` and unions the method name across ALL 97
/// types with ZERO regard for whether that type's methods require a
/// receiver — the doc's own soundness rationale ("no other bare-call target
/// in the language") is true ONLY for names whose SOLE owning type is a
/// true global bucket. Cross-referencing the generator's own per-type dump
/// (`tools/gen-al-builtins/out/member_builtins.json`) shows every one of
/// these 19 names is owned EXCLUSIVELY by receiver-qualified instance types
/// (`Page`, `RequestPage`, `Codeunit`/`CodeunitInstance`, `Report`/
/// `ReportInstance`, `Xmlport`/`XmlportInstance`, `Dialog`, `File`,
/// `QueryInstance`, `RecordRef`, `TestPage`, `Debugger`, `TestField`,
/// `RecordId`, `FilterPageBuilder`) — NEVER by the "System" pseudo-bucket
/// the same JSON shows houses genuinely bare-global names (`Format`,
/// `Today`, `GuiAllowed`, `CreateGuid`). `Message`/`Error`/`Confirm` are the
/// one deliberate near-miss: also documented under a receiver-shaped bucket
/// (`Dialog`), but MS Learn's own text is explicit that they "can be
/// invoked without specifying the data type name" — i.e. genuinely global —
/// so they are NOT in this set (left probing/colliding, honest per an
/// UNCERTAIN name's rule below).
///
/// | Name (PAGE_INSTANCE) | Owning types (compiler DLL) | Bare form? | Citation |
/// |---|---|---|---|
/// | `run` | Codeunit, CodeunitInstance, Page, Report, ReportInstance, Xmlport, XmlportInstance | No — always `Obj.Run(...)` / `Type.Run(...)` static | <https://learn.microsoft.com/en-us/dynamics365/business-central/dev-itpro/developer/methods-auto/page/page-run--method>, <https://learn.microsoft.com/en-us/dynamics365/business-central/dev-itpro/developer/methods-auto/codeunit/codeunit-run-method> |
/// | `runmodal` | FilterPageBuilder, Page, Report, ReportInstance | No — `CurrPage.RunModal()`/`Page.RunModal(...)`/`Report.RunModal(...)` | <https://learn.microsoft.com/en-us/dynamics365/business-central/dev-itpro/developer/methods-auto/page/page-runmodal--method> |
/// | `close` | Dialog, File, Page, QueryInstance, RecordRef, RequestPage, TestPage | No — `CurrPage.Close()`/`MyDialog.Close()` | <https://learn.microsoft.com/en-us/dynamics365/business-central/dev-itpro/developer/methods-auto/page/page-close-method> |
/// | `update` | Dialog, Page, RequestPage | No — `CurrPage.Update([Boolean])` idiom is ALWAYS receiver-qualified (riskiest per the round-1 addenda; explicitly verified) | <https://learn.microsoft.com/en-us/dynamics365/business-central/dev-itpro/developer/methods-auto/page/page-update-method> |
/// | `activate` | Page, RequestPage, Debugger, TestField | No | <https://learn.microsoft.com/en-us/dynamics365/business-central/dev-itpro/developer/methods-auto/page/page-activate-method> |
/// | `cancelbackgroundtask` | Page | No | <https://learn.microsoft.com/en-us/dynamics365/business-central/dev-itpro/developer/methods-auto/page/page-cancelbackgroundtask-method> |
/// | `caption` | FieldRef, Page, RecordRef, RequestPage, TestField, TestPage, TestPart, TestRequestPage | No | <https://learn.microsoft.com/en-us/dynamics365/business-central/dev-itpro/developer/methods-auto/page/page-caption-method> |
/// | `editable` | Page, RequestPage, TestField, TestPage, TestPart, TestRequestPage | No | <https://learn.microsoft.com/en-us/dynamics365/business-central/dev-itpro/developer/methods-auto/page/page-editable-method> |
/// | `enqueuebackgroundtask` | Page | No | <https://learn.microsoft.com/en-us/dynamics365/business-central/dev-itpro/developer/methods-auto/page/page-enqueuebackgroundtask-method> |
/// | `getbackgroundparameters` | Page | No | <https://learn.microsoft.com/en-us/dynamics365/business-central/dev-itpro/developer/methods-auto/page/page-getbackgroundparameters-method> |
/// | `getrecord` | Page, RecordId | No — `CurrPage.GetRecord(Rec)` | <https://learn.microsoft.com/en-us/dynamics365/business-central/dev-itpro/developer/methods-auto/page/page-getrecord-method> |
/// | `lookupmode` | Page, RequestPage | No | <https://learn.microsoft.com/en-us/dynamics365/business-central/dev-itpro/developer/methods-auto/page/page-lookupmode-method> |
/// | `objectid` | Page, ReportInstance, RequestPage | No | <https://learn.microsoft.com/en-us/dynamics365/business-central/dev-itpro/developer/methods-auto/page/page-objectid-method> |
/// | `promptmode` | Page | No | <https://learn.microsoft.com/en-us/dynamics365/business-central/dev-itpro/developer/methods-auto/page/page-promptmode-method> |
/// | `saverecord` | Page, RequestPage | No | <https://learn.microsoft.com/en-us/dynamics365/business-central/dev-itpro/developer/methods-auto/page/page-saverecord-method> |
/// | `setbackgroundtaskresult` | Page | No | <https://learn.microsoft.com/en-us/dynamics365/business-central/dev-itpro/developer/methods-auto/page/page-setbackgroundtaskresult-method> |
/// | `setrecord` | Page | No — `CurrPage.SetRecord(Rec)` | <https://learn.microsoft.com/en-us/dynamics365/business-central/dev-itpro/developer/methods-auto/page/page-setrecord-method> |
/// | `setselectionfilter` | Page, RequestPage | No | <https://learn.microsoft.com/en-us/dynamics365/business-central/dev-itpro/developer/methods-auto/page/page-setselectionfilter-method> |
/// | `settableview` | Page, ReportInstance, XmlportInstance | No | <https://learn.microsoft.com/en-us/dynamics365/business-central/dev-itpro/developer/methods-auto/page/page-settableview-method> |
///
/// # Scope discipline (an uncertain name is left colliding, honest)
///
/// This set is INTENTIONALLY narrow — exactly the grounded 19, not every
/// name in `GLOBAL_BUILTIN_METHODS` that "looks" instance-scoped (e.g.
/// `Insert`/`Modify`/`Validate` are real `Record` methods AND also appear in
/// the 785-name union, but have NOT been individually grounded per-context
/// here — a table-scope candidate colliding with one of THOSE still fails
/// closed to `BuiltinPrecedenceCollision`, unchanged). Widening this set to
/// any NEW name requires the same per-name × per-context grounding (page
/// trigger/action/procedure, pageextension, table/tableextension, report/
/// reportextension, XmlPort, codeunit `OnRun`) and an MS Learn citation —
/// never a guess by naming-convention resemblance.
static INSTANCE_ONLY_NEVER_BARE: phf::Set<&'static str> = phf_set! {
    "activate", "cancelbackgroundtask", "caption", "close", "editable",
    "enqueuebackgroundtask", "getbackgroundparameters", "getrecord",
    "lookupmode", "objectid", "promptmode", "run", "runmodal", "saverecord",
    "setbackgroundtaskresult", "setrecord", "setselectionfilter",
    "settableview", "update",
};

/// Whether `name_lc` is compiler-GROUNDED to have no bare-call form anywhere
/// in AL (see [`INSTANCE_ONLY_NEVER_BARE`]'s doc for the full per-name
/// citation table). Consumed by BOTH `resolve_bare` sites that currently
/// treat ANY `GLOBAL_BUILTIN_METHODS`/`PageInstance` catalog hit as a valid
/// bare-call reading: the Step 3 PROBE-THEN-DECIDE collision guard
/// ([`is_bare_builtin_or_page_intrinsic`], below) and the Step 4 plain
/// catalog fallback (`resolve_bare_with_args`'s own body). A proven name
/// short-circuits BOTH to "not a builtin reading" — never a partial fix
/// applied to only one of the two call sites.
fn is_proven_never_bare_call(name_lc: &str) -> bool {
    INSTANCE_ONLY_NEVER_BARE.contains(name_lc)
}

/// Whether `name_lc` is a global builtin OR a bare-callable page/instance
/// intrinsic (`member_catalog`'s `PageInstance` set: `Update`/`Close`/
/// `SetRecord`/…) — the collision set `resolve_bare`'s Step 3 PROBE-THEN-
/// DECIDE guard checks AFTER finding a table-scope candidate (never gates the
/// probe itself; see the Step 3 doc in this function's body). A bare call to
/// one of these names inside a Page/Table trigger is textually ambiguous
/// between "the implicit-Rec table's own procedure" and "the platform
/// intrinsic" — with no compiler-verified precedence rule captured here,
/// fail-closed to `Unknown` on any such collision rather than pick a side.
///
/// # GLOBAL suppression (pageext-merge-and-final-residual plan, Task 2 —
/// round-1 review addenda, BINDING)
///
/// [`is_proven_never_bare_call`] is checked FIRST and short-circuits this
/// whole function to `false` — a proven-never-bare name is not a real
/// "global or page-intrinsic" reading AT ALL, so there is no collision to
/// detect: the table-scope candidate simply wins (Step 3's caller returns
/// its routes directly, never reaching this guard's `Unknown` branch). This
/// is the fix for the real CDO site (`CDOEMailJobs.Page.al:125`'s bare
/// `Run()` vs `CDOEMailJob.Table.al:192`'s `procedure Run()`): `run` ∈
/// `PAGE_INSTANCE` ∧ ∈ `GLOBAL_BUILTIN_METHODS`, but is compiler-grounded to
/// have NO bare form anywhere — the table's own procedure was always the
/// only real candidate.
///
/// # Why this checks `Global`/`Framework(PageInstance)` only, never `Record`
/// (beyond-1B.3b Task 1, Item 4 — NOT a bug, do not "fix" by adding a
/// `Record` branch here)
///
/// `resolve_member`'s `Record` arm (see the comment there) deliberately lets
/// a visible source/ABI table candidate SHADOW a same-named `Record` catalog
/// builtin with NO collision guard — that is corpus-validated correct AL
/// precedence (42 real CDO `builtin-catalog-fp-collision` instances, e.g.
/// `Record::fieldno`, `Record::setrecfilter`; see
/// `resolve_member_record_source_proc_shadows_same_named_builtin`). This
/// function's collision set is exactly the TWO catalogs (`Global`,
/// `Framework(PageInstance)`) where the reverse holds — Step 3's own
/// table-scope candidate has NO compiler-verified precedence over those, so
/// it fails closed instead. Adding `Record` here would regress the
/// beyond-1B.3b Task 1 source-shadows-catalog fix back into a false
/// `Unknown`.
fn is_bare_builtin_or_page_intrinsic(name_lc: &str) -> bool {
    if is_proven_never_bare_call(name_lc) {
        return false;
    }
    global_builtin_id(name_lc).is_some()
        || member_builtin(
            MemberCatalogKind::Framework(&FrameworkKind::PageInstance),
            name_lc,
        )
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Resolve a bare (unqualified) call to `name_lc` with `arity` arguments from
/// the context of `from_object`.
///
/// `with_state` is the call site's [`WithState`] (beyond-1B.3b Task 3): Step 3
/// (implicit-Rec) only runs when this is `NoWithProven` — see the module doc
/// and [`WithState`] itself for the two-signal fail-closed soundness
/// argument. Every OTHER precedence step is unaffected by `with_state` (a
/// `with` block does not change own-object/extension-base/builtin lookup).
///
/// Returns `(DispatchShape, Vec<Route>)` (Task 4, sigfp-and-ambiguous-
/// reclassification plan — the file's own tuple convention, mirroring
/// [`resolve_member`]): a bare call is single-dispatch (`DispatchShape::Exact`,
/// exactly one route) in every case EXCEPT a genuine same-object overload
/// ambiguity resolved via [`resolve_in_object`]'s prevalidated candidate-set
/// arm, which returns `DispatchShape::AmbiguousOverload` with one route per
/// candidate — see that function's doc.
pub fn resolve_bare(
    from_object: &ObjectNode,
    name_lc: &str,
    arity: usize,
    graph: &ProgramGraph,
    index: &ResolveIndex,
    surface: &DeclSurface,
    with_state: WithState,
) -> (DispatchShape, Vec<Route>) {
    resolve_bare_with_args(
        from_object,
        name_lc,
        arity,
        graph,
        index,
        surface,
        with_state,
        &[],
    )
}

/// The arg-typed variant of [`resolve_bare`] — `resolve_full_program`'s real
/// call-site resolution uses this so Task 2's fail-closed pick
/// (`resolve_in_object`'s `_` arm) has the call's typed arguments available.
/// [`resolve_bare`] is a thin `args = &[]` wrapper kept for every pre-Task-2
/// call site (every unit test in this module, `receiver.rs`'s bare-call type
/// query) that has no argument-typing context available or relevant — an
/// empty `args` slice is behavior-neutral (Task 2's pick never fires without
/// arguments to type).
#[allow(clippy::too_many_arguments)] // 7 pre-existing params + `args` (Task 2, argtype-dispatch-and-page-catalog plan).
pub(crate) fn resolve_bare_with_args(
    from_object: &ObjectNode,
    name_lc: &str,
    arity: usize,
    graph: &ProgramGraph,
    index: &ResolveIndex,
    surface: &DeclSurface,
    with_state: WithState,
    args: &[ArgDispatchInfo],
) -> (DispatchShape, Vec<Route>) {
    // 1. Own object.
    if let Some((shape, routes)) = resolve_in_object(
        &from_object.id,
        from_object.tier,
        name_lc,
        arity,
        &from_object.id,
        graph,
        index,
        surface,
        args,
    ) {
        return (shape, routes);
    }

    // Task 3: running diagnostic reason for the eventual Step 5 fallback, in
    // case no earlier step resolves. Steps 2/3 below OVERWRITE this with a
    // more specific finding as they run; the DEFAULT (`MemberNotFound`)
    // survives when nothing more specific was found — genuine absence.
    let mut reason = UnknownReason::MemberNotFound;

    // 2. Extension base (Task 1.5 — access-filtered). `resolve_in_object`
    // itself does ZERO access filtering, so gate it behind the SAME
    // caller-identity-aware visibility check Task 1 established
    // (`object_has_visible_member_candidate`): the calling object is the
    // extension (`from_object`), the candidate object is the resolved base
    // (`base_id`). Per the Task-1 rule: base `Local` is NEVER visible to a
    // bare call from an extension (base-self only, even though the caller IS
    // a direct extension); cross-app `Internal` requires the same app;
    // `Protected` is visible (the caller is by construction a direct,
    // kind-compatible extension of `base_id`, so the self-or-extends check
    // trivially holds — incidentally safe, not accidentally permissive);
    // `Public` is always visible. When the base member is not visible, Step 2
    // declines entirely (no `resolve_in_object` call) and falls through to
    // Step 3/4/5, exactly like the pre-existing "no candidate at all"
    // fallthrough shape.
    if let Some(base_kind) = extension_base_kind(from_object.id.kind)
        && let Some(extends_target) = from_object.extends_target.as_deref()
        && let Some(base_obj) = graph.resolve_object(from_object.id.app, base_kind, extends_target)
    {
        let base_id = base_obj.id.clone();
        let base_tier = base_obj.tier;
        if object_has_visible_member_candidate(
            &base_id,
            base_tier,
            name_lc,
            arity,
            &from_object.id,
            graph,
            index,
        ) {
            if let Some((shape, routes)) = resolve_in_object(
                &base_id,
                base_tier,
                name_lc,
                arity,
                &from_object.id,
                graph,
                index,
                surface,
                args,
            ) {
                return (shape, routes);
            }
        } else if let Some(r) =
            access_exclusion_reason(&base_id, name_lc, arity, &from_object.id, graph, index)
        {
            reason = r;
        }
    }

    // 3. Implicit-Rec (beyond-1B.3b Task 3). Every guard below is
    // independently fail-closed; any of them declining routes straight past
    // this step to Step 4/5 rather than guessing.
    //
    // (0) STRICT ObjectKind guard: bare-implicit-Rec dispatch is structurally
    // a Page/Table source-record mechanism in AL — ONLY these four kinds are
    // eligible. Every other kind (Codeunit/Report/XmlPort/Query/…) skips this
    // step entirely, no accidental leakage via `implicit_rec_table_id`'s own
    // (defense-in-depth) kind match. Task 3: tag WHY it's skipped for the two
    // named, high-volume excluded kinds (Codeunit/Report(Extension)) so the
    // eventual Step 5 Unknown carries that context rather than the generic
    // `MemberNotFound` default.
    if matches!(
        from_object.id.kind,
        ObjectKind::Table
            | ObjectKind::Page
            | ObjectKind::TableExtension
            | ObjectKind::PageExtension
    ) {
        // (1) with-guard: Step 3 runs ONLY on a proven with-free call site.
        // `InsideWith`/`Unknown` (the AST places the site inside a `with`, or
        // the two with-detection signals disagree) skip Step 3 — a false
        // `Source` inside an unrepresented `with` is the fatal case this
        // guards against (see `WithState`'s doc).
        if with_state == WithState::NoWithProven {
            // (2) Compute the implicit-Rec table id by kind; no unique
            // in-closure table → fall through (nothing to search).
            if let Some(table_id) = implicit_rec_table_id(from_object, graph, index) {
                // (3) Visibility-scoped table ∪ extensions search (Task 2):
                // `NotVisible` falls through to Step 4/5 (tagging WHY when a
                // candidate existed but was access-excluded); `Resolved` is a
                // clean Source/Abi/Opaque route; `Ambiguous` is an honest
                // ambiguous Unknown (>1 visible candidate — never pick-first,
                // never falls through to the catalog).
                match resolve_in_table_scope(
                    from_object,
                    table_id,
                    name_lc,
                    arity,
                    graph,
                    index,
                    surface,
                    args,
                ) {
                    TableScopeOutcome::Resolved(shape, routes) => {
                        // (4) Builtin/intrinsic PROBE-THEN-DECIDE: the probe
                        // (step 3) already ran; a same-name+arity table-scope
                        // candidate exists AND `name_lc` is also a global
                        // builtin or a bare-callable page/instance intrinsic
                        // is an UNPROVEN precedence collision — fail closed
                        // to `Unknown` rather than assume the table wins
                        // (never emit `Catalog` here; Step 4 below is the
                        // only place that does).
                        if is_bare_builtin_or_page_intrinsic(name_lc) {
                            return (
                                DispatchShape::Exact,
                                vec![unresolved_route(UnknownReason::BuiltinPrecedenceCollision)],
                            );
                        }
                        return (shape, routes);
                    }
                    TableScopeOutcome::Ambiguous => {
                        return (
                            DispatchShape::Exact,
                            vec![unresolved_route(UnknownReason::OverloadAmbiguous)],
                        );
                    }
                    TableScopeOutcome::NotVisible { access_excluded } => {
                        if let Some(r) = access_excluded {
                            reason = r;
                        }
                    }
                }
            } else {
                // No unique in-closure implicit-Rec table (ambiguous
                // cross-app name, out-of-closure, or unresolved).
                reason = UnknownReason::ReceiverOutOfClosure;
            }
        } else {
            // Lexically inside a `with` block (or with-freedom unproven).
            reason = UnknownReason::WithScopeGuard;
        }
    } else if matches!(from_object.id.kind, ObjectKind::Codeunit) {
        reason = UnknownReason::CodeunitTableNoExcluded;
    } else if matches!(
        from_object.id.kind,
        ObjectKind::Report | ObjectKind::ReportExtension
    ) {
        reason = UnknownReason::ReportRecExcluded;
    }

    // 4. Global builtin. GLOBAL suppression (pageext-merge-and-final-residual
    // plan, Task 2 — round-1 addenda, BINDING): `is_proven_never_bare_call`
    // gates this catalog fallback too, not just Step 3's collision guard
    // above — a name with NO bare form anywhere in AL must never win a
    // `Catalog` route here either, even with zero table-scope candidate in
    // play (e.g. a bare `Run()` in a Codeunit with no own `Run` procedure:
    // Step 3 never runs at all for a Codeunit, so this Step 4 fallback was
    // the ONLY place the false `Catalog` route could have come from — see
    // [`is_proven_never_bare_call`]'s doc).
    if !is_proven_never_bare_call(name_lc)
        && let Some(builtin_id) = global_builtin_id(name_lc)
    {
        return (
            DispatchShape::Exact,
            vec![Route {
                target: RouteTarget::Builtin(builtin_id.clone()),
                evidence: Evidence::Catalog,
                conditions: vec![],
                witness: Witness::CatalogEntry {
                    id: builtin_id,
                    catalog_version: catalog_version().to_string(),
                },
                receiver_tier: None,
            }],
        );
    }

    // 5. Unknown. Reason-split Task 2: the `MemberNotFound` DEFAULT (never
    // overwritten by an earlier step) means Step 1's own-object
    // `resolve_in_object` call found the name absent entirely — the receiver
    // (from_object itself) IS resolved by construction, so tag its tier.
    // Every other `reason` value here (ReceiverOutOfClosure/WithScopeGuard/
    // CodeunitTableNoExcluded/ReportRecExcluded/an access-exclusion reason)
    // is untagged — tier is `MemberNotFound`-specific (see its doc).
    if reason == UnknownReason::MemberNotFound {
        return (
            DispatchShape::Exact,
            vec![unresolved_route_with_tier(reason, from_object.tier)],
        );
    }
    (DispatchShape::Exact, vec![unresolved_route(reason)])
}

// ---------------------------------------------------------------------------
// ObjectRun resolution
// ---------------------------------------------------------------------------

/// Entry-trigger name for an object kind (Phase-2 correction over L3's always-"OnRun").
///
/// | Kind       | Trigger         |
/// |------------|-----------------|
/// | Codeunit   | `"onrun"`       |
/// | Page       | `"onopenpage"`  |
/// | Report     | `"onprereport"` |
/// | Query/Other| `"onrun"` (best-effort) |
fn entry_trigger_name(kind: ObjectKind) -> &'static str {
    match kind {
        ObjectKind::Page => "onopenpage",
        ObjectKind::Report => "onprereport",
        _ => "onrun",
    }
}

/// Build a single Opaque boundary route (target exists but is not in our source).
fn opaque_boundary_route(key: AbiRoutineKey) -> Route {
    Route {
        target: RouteTarget::AbiSymbol { key: key.clone() },
        evidence: Evidence::Opaque,
        conditions: vec![],
        witness: Witness::AbiSymbol { key },
        receiver_tier: None,
    }
}

/// Shared entry-trigger dispatch machinery: given an ALREADY-RESOLVED target
/// object, look up its entry trigger ([`entry_trigger_name`]) and build the
/// resulting route — collapse-marker guard, Opaque-boundary-when-absent, and
/// Source-route-when-present. Used by BOTH `resolve_object_run` (T1.3,
/// keyword-receiver form: `Codeunit.Run` / `Page.RunModal` / `Report.RunModal`)
/// and the typed-variable `Run`/`RunModal` special case in
/// `resolve_member_with_args`'s `ReceiverType::Object` arm — the SAME
/// machinery for both populations, never a parallel resolver (T1.3 brief).
fn dispatch_entry_trigger(
    target_id: &ObjectNodeId,
    target_tier: TrustTier,
    object_kind: ObjectKind,
    graph: &ProgramGraph,
    index: &ResolveIndex,
    surface: &DeclSurface,
) -> (DispatchShape, Vec<Route>) {
    let trigger_name = entry_trigger_name(object_kind);
    let candidates = index.routines_in_object(target_id, trigger_name);

    // Object-level triggers have `enclosing_member_lc == None`.
    let entry_rid = candidates
        .iter()
        .find(|r| r.enclosing_member_lc.is_none())
        .or_else(|| candidates.first());

    let Some(entry_rid) = entry_rid else {
        // Trigger not found in index — Opaque (e.g. an object with no explicit trigger).
        let (obj_num, obj_name_lc) = match &target_id.key {
            ObjKey::Id(n) => (*n, String::new()),
            ObjKey::Name(s) => (0i64, s.clone()),
        };
        let key = AbiRoutineKey {
            app: target_id.app,
            object_type: format!("{:?}", target_id.kind).to_ascii_lowercase(),
            object_number: obj_num,
            object_name_lc: obj_name_lc,
            routine_name_lc: trigger_name.to_string(),
            params_count: 0,
            param_type_fp: 0,
            routine_kind: AbiRoutineKind::Procedure,
            event_kind: AbiEventKind::None,
        };
        return (DispatchShape::Exact, vec![opaque_boundary_route(key)]);
    };

    // COLLAPSE-MARKER GUARD (Task 2 review fix): this entry-trigger lookup
    // bypasses `resolve_in_object`'s name+arity selection entirely (it picks
    // by ROLE — the object-level trigger — never by counting candidates), so
    // it must consult the marker itself; see `routine_is_collapse_marked`'s
    // doc for the full enumeration of guarded sites.
    if routine_is_collapse_marked(entry_rid, graph) {
        return (
            DispatchShape::Exact,
            vec![unresolved_route(UnknownReason::OverloadAmbiguous)],
        );
    }

    (
        DispatchShape::Exact,
        vec![make_routine_route(entry_rid, target_tier, surface, graph)],
    )
}

/// Resolve an `ObjectRun` dispatch (Codeunit.Run / Page.RunModal / Report.Run …)
/// to the entry trigger of the statically-named/numbered target object.
///
/// # Dispatch semantics
///
/// | Situation | Shape | Completeness | Routes |
/// |-----------|-------|-------------|--------|
/// | `target_ref` is `None` (runtime variable) | `DynamicOpen` | `Partial{RuntimeTypeUnbounded}` | `[{Unresolved, Unknown, None}]` |
/// | Target named/numbered but absent from graph | `Exact` | `Complete` | `[{AbiSymbol, Opaque, AbiSymbol}]` |
/// | Target found; entry trigger resolved | `Exact` | `Complete` | `[{Routine(trigger), Source/Abi, SourceSpan/…}]` |
/// | Target found; entry trigger absent from index | `Exact` | `Complete` | `[{AbiSymbol, Opaque, AbiSymbol}]` |
///
/// # Opaque-vs-Unknown choice (Phase 2 note)
///
/// When the target is not found in the graph or the entry trigger is not indexed,
/// we use `Evidence::Opaque` with `RouteTarget::AbiSymbol` because we know the
/// target *exists* somewhere — it just isn't in our source snapshot.
/// The `classify_obligation` metric counts a route with `AbiSymbol` target and
/// `Opaque` evidence as `Resolved` (not `Unknown`) because the symbol boundary
/// is known and retains its identity; this aligns with L3's External classification.
pub fn resolve_object_run(
    from: AppRef,
    object_kind: ObjectKind,
    target_ref: Option<&str>,
    target_is_name: bool,
    graph: &ProgramGraph,
    index: &ResolveIndex,
    surface: &DeclSurface,
) -> (DispatchShape, SetCompleteness, Vec<Route>) {
    let Some(target_ref) = target_ref else {
        // Dynamic target (a runtime variable) — known shape, open world.
        return (
            DispatchShape::DynamicOpen,
            SetCompleteness::Partial {
                reason: OpenWorldReason::RuntimeTypeUnbounded,
            },
            vec![unresolved_route(UnknownReason::UntrackedReceiver)],
        );
    };

    // Resolve the target object.
    let target_obj: Option<&ObjectNode> = if target_is_name {
        graph.resolve_object(from, object_kind, target_ref)
    } else {
        match target_ref.parse::<i64>() {
            Ok(n) => index
                .object_by_number(graph, from, object_kind, n)
                .and_then(|oid| graph.objects.iter().find(|o| o.id == oid)),
            Err(_) => None,
        }
    };

    let Some(target_obj) = target_obj else {
        // Target is named/numbered but absent from the entire graph (not in
        // workspace source, not in any dep's SymbolReference). We do NOT know
        // which app owns it, so creating an AbiSymbol with `app = from`
        // (caller's app) would be semantically wrong and would fail the
        // ABI ingestion integrity check. Emit Unknown/Unresolved: honest
        // failure — we cannot name the callee. Reason-split Task 2: the
        // RECEIVER OBJECT itself is absent — `ObjectNotInGraph`, not
        // `MemberNotFound` (which now means member-absent-on-a-RESOLVED
        // surface). No externality claim — see `ObjectNotInGraph`'s doc.
        return (
            DispatchShape::Exact,
            SetCompleteness::Complete,
            vec![unresolved_route(UnknownReason::ObjectNotInGraph)],
        );
    };

    // Look up the entry trigger and build the route — shared machinery (T1.3)
    // with the typed-variable special case in `resolve_member_with_args`.
    let (shape, routes) = dispatch_entry_trigger(
        &target_obj.id,
        target_obj.tier,
        object_kind,
        graph,
        index,
        surface,
    );
    (shape, SetCompleteness::Complete, routes)
}

// ---------------------------------------------------------------------------
// Implicit-trigger resolution (data-is-control-flow edges)
// ---------------------------------------------------------------------------

/// Resolve an implicit record-operation trigger fan-out.
///
/// Maps the AL record operation name to the corresponding object/field trigger
/// and collects routes from both the base table and all `TableExtension`s visible
/// in the whole-program snapshot.
///
/// # Trigger mapping
///
/// | `op`        | Trigger         |
/// |-------------|-----------------|
/// | `"insert"`  | `"oninsert"`    |
/// | `"modify"`  | `"onmodify"`    |
/// | `"delete"`  | `"ondelete"`    |
/// | `"validate"`| `"onvalidate"`  |
/// | `"rename"`  | `"onrename"`    |
///
/// # Validate-field approximation (Phase 2)
///
/// `Rec.Validate(FieldName)` targets `OnValidate` on **one specific field**,
/// but the field argument may not be captured in the extraction layer.  For
/// Phase 2 this function fans out to **all** `onvalidate` triggers on the
/// table and its extensions (one route per `(object, field)` pair), using
/// [`DispatchShape::Multicast`] with
/// `SetCompleteness::Partial{ReverseDependentExtensions}`.  The Task-6
/// differential gate measures the residual versus L3's per-field resolution.
///
/// For `insert/modify/delete/rename` (object-level triggers,
/// `enclosing_member_lc == None`) the fan-out is clean.
pub fn resolve_implicit_trigger(
    op: &str,
    table_object: &ObjectNode,
    graph: &ProgramGraph,
    index: &ResolveIndex,
    surface: &DeclSurface,
) -> (DispatchShape, SetCompleteness, Vec<Route>) {
    let trigger_name: &str = match op.fold_identifier().as_str() {
        "insert" => "oninsert",
        "modify" => "onmodify",
        "delete" => "ondelete",
        "validate" => "onvalidate",
        "rename" => "onrename",
        _ => {
            // Unrecognised op: honest empty Multicast.
            return (
                DispatchShape::Multicast,
                SetCompleteness::Partial {
                    reason: OpenWorldReason::ReverseDependentExtensions,
                },
                vec![],
            );
        }
    };

    let mut routes: Vec<Route> = Vec::new();

    // COLLAPSE-MARKER GUARD (Task 2 review fix): this trigger fan-out looks
    // up each candidate by ROLE (fixed trigger name), never through
    // `resolve_in_object`'s name+arity selection, so both loops below must
    // consult the marker themselves — see `routine_is_collapse_marked`'s
    // doc. A marked trigger declines to an honest Unknown route rather than
    // silently vanishing from the Multicast set (which would understate its
    // real cardinality) or resolving confidently to a possibly-wrong
    // identity.

    // Triggers on the base table itself.
    for rid in index.routines_in_object(&table_object.id, trigger_name) {
        if routine_is_collapse_marked(rid, graph) {
            routes.push(unresolved_route(UnknownReason::OverloadAmbiguous));
            continue;
        }
        routes.push(make_routine_route(rid, table_object.tier, surface, graph));
    }

    // Triggers on every TableExtension of this table (reverse-dep; whole-snapshot).
    let table_name_lc = table_object.name.fold_identifier();
    for ext_id in index.table_extensions_of(&table_name_lc) {
        let ext_tier = graph
            .objects
            .iter()
            .find(|o| &o.id == ext_id)
            .map(|o| o.tier)
            .unwrap_or(TrustTier::Workspace);
        for rid in index.routines_in_object(ext_id, trigger_name) {
            if routine_is_collapse_marked(rid, graph) {
                routes.push(unresolved_route(UnknownReason::OverloadAmbiguous));
                continue;
            }
            routes.push(make_routine_route(rid, ext_tier, surface, graph));
        }
    }

    (
        DispatchShape::Multicast,
        SetCompleteness::Partial {
            reason: OpenWorldReason::ReverseDependentExtensions,
        },
        routes,
    )
}

// ---------------------------------------------------------------------------
// Member-call resolution (Phase 3 Task 2)
// ---------------------------------------------------------------------------

/// Build a `(Exact, [Catalog route])` outcome for a recognized member builtin.
fn member_catalog_route(bid: BuiltinId) -> (DispatchShape, Vec<Route>) {
    (
        DispatchShape::Exact,
        vec![Route {
            target: RouteTarget::Builtin(bid.clone()),
            evidence: Evidence::Catalog,
            conditions: vec![],
            witness: Witness::CatalogEntry {
                id: bid,
                catalog_version: catalog_version().to_string(),
            },
            receiver_tier: None,
        }],
    )
}

/// Build a `(Exact, [Unknown route])` outcome (Task 3: `reason` is REQUIRED —
/// every caller supplies a diagnostic [`UnknownReason`]).
fn member_unknown_route(reason: UnknownReason) -> (DispatchShape, Vec<Route>) {
    (DispatchShape::Exact, vec![unresolved_route(reason)])
}

/// Like [`member_unknown_route`] but tags `receiver_tier` — see
/// [`unresolved_route_with_tier`]'s doc.
fn member_unknown_route_with_tier(
    reason: UnknownReason,
    tier: TrustTier,
) -> (DispatchShape, Vec<Route>) {
    (
        DispatchShape::Exact,
        vec![unresolved_route_with_tier(reason, tier)],
    )
}

/// Build a `(DynamicOpen, [Unknown blocker])` outcome for Dynamic receivers —
/// the receiver's static type is genuinely untracked (a runtime Variant), so
/// the single fixed reason is always [`UnknownReason::UntrackedReceiver`].
fn member_dynamic_open_route() -> (DispatchShape, Vec<Route>) {
    (
        DispatchShape::DynamicOpen,
        vec![unresolved_route(UnknownReason::UntrackedReceiver)],
    )
}

/// Collapse a per-IMPLEMENTER [`resolve_in_object`] result to a SINGLE route
/// for the `Interface` fan-out's per-implementer slot (Task 4, round-1 review
/// addendum "T4 — interface nesting OUT OF SCOPE", BINDING): an implementer's
/// OWN same-object overload ambiguity must NOT extend a nested candidate set
/// into the already-`Polymorphic` edge — flattening would corrupt both
/// Complete-vs-Partial completeness semantics (the interface fan-out is
/// `SetCompleteness::Partial{ReverseDependentImplementers}`, open-world; a
/// nested `AmbiguousOverload` candidate set is `Complete`, closed-world — the
/// two must never merge) and the per-implementer grouping (BC-Brain expects
/// one route per implementer, not a variable-width nested fan-out). A genuine
/// `DispatchShape::AmbiguousOverload` result collapses back to exactly the
/// single `Unresolved(OverloadAmbiguous)` route this per-implementer slot
/// always emitted for a same-object ambiguity BEFORE Task 4 existed — see the
/// `resolve_member_interface_implementer_own_overload_ambiguity_stays_nested_unresolved`
/// fixture. Every OTHER shape `resolve_in_object` returns is, by
/// construction, already exactly one route, so `routes.pop()` is a plain
/// unwrap of that invariant — `absent` (the `None`-name-absent fallback,
/// pre-built by each call site with its own tier/reason) is the only other
/// path.
fn interface_delegate_route(result: Option<(DispatchShape, Vec<Route>)>, absent: Route) -> Route {
    match result {
        Some((DispatchShape::AmbiguousOverload, _candidate_routes)) => {
            unresolved_route(UnknownReason::OverloadAmbiguous)
        }
        Some((_, mut routes)) => routes.pop().unwrap_or(absent),
        None => absent,
    }
}

/// Map an object kind to its instance-builtin [`FrameworkKind`] catalog, if any.
///
/// Returns `None` for kinds that have no instance-builtin catalog — their member
/// methods are source-declared procedures, not platform-intrinsic instance builtins.
fn object_instance_framework_kind(kind: ObjectKind) -> Option<FrameworkKind> {
    match kind {
        ObjectKind::Page => Some(FrameworkKind::PageInstance),
        ObjectKind::Report => Some(FrameworkKind::ReportInstance),
        // A `V: Query "Name"` variable's members (`Open`/`Read`/`Close`/...)
        // unconditionally exist on every Query object. Added 2026-09-13 after
        // 3 CDO sites resolved their RECEIVER to a workspace Query object and
        // then found no catalog to look the member up in — see
        // `member_catalog::ENTRY_DISPATCH_BUILTIN_IDS`'s doc for why the
        // ENTRY-DISPATCH exclusion was never a reason to withhold the catalog.
        ObjectKind::Query => Some(FrameworkKind::QueryInstance),
        _ => None,
    }
}

/// Returns `true` when `method_lc` is a CurrPage-ONLY instance method that must
/// stay excluded from the general `Object{kind}` catalog fallback for the given
/// object `kind`.
///
/// # History — the Phase 4 Task 1 rationale was WRONG, corrected by the
/// argtype-dispatch-and-page-catalog plan (Task 1)
///
/// Phase 4 Task 1 excluded `SetRecord`/`SetTableView`/`GetRecord`/
/// `SetSelectionFilter` (Page) and `SetTableView` (Report) here, reasoning that
/// "their argument/return type depends on the object's source table, so we
/// can't validate the argument." That conflated ARGUMENT-type validation with
/// member EXISTENCE — the resolver has never validated any catalog method's
/// argument types (`RunModal()`, `SaveAsPdf()`, etc. are accepted with zero
/// arity/type checking too), so the exclusion was withholding a `Catalog` edge
/// for a member that unconditionally EXISTS on every Page/Report object,
/// regardless of its source table. All 18 real-world CDO sites hitting this
/// exclusion (13 workspace-tier + 5 embedded-tier, all `SetTableView`/
/// `SetRecord`/`GetRecord` calls on ordinary declared Page/Report variables)
/// confirm this: L3's own `PAGE_INSTANCE`/`REPORT_INSTANCE` catalogs
/// (`engine::l3::member_builtins.rs:731-786`) already list every one of these
/// methods, and MS Learn documents them as unconditional Page/Report
/// instance-method members present since the earliest AL/NAV releases these
/// docs cover (no version gate needed — `SetTableView`/`SetRecord`/
/// `GetRecord`/`SetSelectionFilter` are ancient platform intrinsics, present
/// in every supported Business Central version):
/// - `Page.SetTableView` — <https://learn.microsoft.com/dynamics365/business-central/dev-itpro/developer/methods-auto/page/page-settableview-method>
/// - `Page.SetRecord` — <https://learn.microsoft.com/dynamics365/business-central/dev-itpro/developer/methods-auto/page/page-setrecord-method>
/// - `Page.GetRecord` — <https://learn.microsoft.com/dynamics365/business-central/dev-itpro/developer/methods-auto/page/page-getrecord-method>
/// - `Page.SetSelectionFilter` — <https://learn.microsoft.com/dynamics365/business-central/dev-itpro/developer/methods-auto/page/page-setselectionfilter-method>
/// - `Report.SetTableView` — <https://learn.microsoft.com/dynamics365/business-central/dev-itpro/developer/methods-auto/report/report-settableview-method>
///
/// # `SaveRecord` stays excluded — CurrPage-ONLY (round-2 closer I8, BINDING)
///
/// `Page.SaveRecord` (<https://learn.microsoft.com/dynamics365/business-central/dev-itpro/developer/methods-auto/page/page-saverecord-method>)
/// is documented and compiler-enforced as a member of the CURRENT PAGE CONTEXT
/// only — calling `.SaveRecord()` on a Page-typed VARIABLE (as opposed to the
/// `CurrPage` singleton) is a genuine AL compile error, not merely an
/// under-modelled edge case. `CurrPage`/bare `Page` singleton receivers never
/// reach this function at all — `infer_receiver_type`'s Step 1
/// (`receiver.rs:588-604`) types them `ReceiverType::Framework(PageInstance)`,
/// which `resolve_member`'s `Framework(kind)` arm resolves via an
/// UNCONDITIONAL catalog lookup (no exclusion check of any kind) — so
/// `CurrPage.SaveRecord()` already resolves via `Catalog` independent of this
/// function. This function is consulted ONLY by the `Object{kind}` arm, which
/// covers every OTHER Page-typed receiver: a declared Page variable/param/
/// global (`id: None`) and Step 0's `CurrPage.<part>.Page` subpage-instance
/// shape (`id: Some(..)`, a MECHANICALLY resolved page identity — see
/// `receiver.rs:560-577`). Keying the exclusion on the METHOD name alone
/// (never on whether an `id` happens to be carried) is deliberate: a resolved
/// page identity is NOT proof of CurrPage origin (a subpage instance has an
/// id too, and is not CurrPage either) — inferring CurrPage-ness from the
/// resolved page type/id would be the exact false-positive vector round-2
/// review flagged. The origin distinction is structural (which
/// `ReceiverType` variant Phase A produced), never data carried on the
/// variant.
///
/// Exclusion list:
/// - Page: `saverecord` only.
/// - Report: none.
fn is_metadata_sensitive_instance_method(kind: ObjectKind, method_lc: &str) -> bool {
    match kind {
        ObjectKind::Page => method_lc == "saverecord",
        _ => false,
    }
}

/// Resolve a member call (`receiver.method_lc(...)`) to its `(DispatchShape, Vec<Route>)`.
///
/// # Implemented arms (Phase 3 Task 2 + Task 3 + Phase 4 Task 2; precedence
/// fixed beyond-1B.3b Task 1)
/// - `RecordRef` / `FieldRef` / `KeyRef` / `Framework(_)` → catalog lookup (these
///   receivers have no source-declared procedures, so there is nothing to shadow).
/// - `Record{..}` → **source-before-catalog**: a visible source/ABI table method
///   (base table ∪ its TableExtensions) of matching name+arity SHADOWS a
///   same-named platform-intrinsic Record method (AL semantics). Cardinality:
///   exactly one visible candidate → `Source`/`Abi`/`Opaque`; more than one →
///   honest ambiguous `Unknown` (never pick-first, never fall through to the
///   catalog); zero candidates (or `table == None`) → consult the Record
///   builtin catalog; no catalog hit either → `Unknown`.
/// - `Object{kind, name_lc}` → resolve target object via `graph.resolve_object`, then
///   dispatch method via `resolve_in_object`.  Special case (arity≤1): `Codeunit.Run` →
///   entry `OnRun` trigger; `Page.Run`/`Page.RunModal` → `OnOpenPage`; `Report.Run`/
///   `Report.RunModal` → `OnPreReport` (T1.3 — shares `dispatch_entry_trigger` with
///   `resolve_object_run`'s keyword-receiver form).
/// - `SelfObject` → `resolve_in_object` on the calling object itself.
/// - `Interface{name_lc}` → `Polymorphic` fan-out to all known implementers via
///   `index.implementers_of`.  For each implementer: Source-tier → unique-arity-matched
///   `Routine` route, or `Unresolved` on name-absent / arity-mismatch / ambiguous
///   (Rule 1/2 — no reachability black hole, no guessed route).  SymbolOnly-tier (cross-app
///   `.app` dep) → `AbiSymbol` (Opaque boundary) via `resolve_in_object`.
///
/// # Deferred arms (TODO markers only)
/// - `Primitive` → Unknown; `Dynamic` → `DynamicOpen`; `Unknown` → Unknown.
pub fn resolve_member(
    receiver: &ReceiverType,
    method_lc: &str,
    arity: usize,
    from_object: &ObjectNode,
    graph: &ProgramGraph,
    index: &ResolveIndex,
    surface: &DeclSurface,
) -> (DispatchShape, Vec<Route>) {
    resolve_member_with_args(
        receiver,
        method_lc,
        arity,
        from_object,
        graph,
        index,
        surface,
        &[],
    )
}

/// The arg-typed variant of [`resolve_member`] — see [`resolve_bare_with_args`]'s
/// doc for the identical `resolve_bare`/`resolve_bare_with_args` split
/// rationale; [`resolve_member`] is the thin `args = &[]` wrapper.
#[allow(clippy::too_many_arguments)] // 7 pre-existing params + `args` (Task 2, argtype-dispatch-and-page-catalog plan).
pub(crate) fn resolve_member_with_args(
    receiver: &ReceiverType,
    method_lc: &str,
    arity: usize,
    from_object: &ObjectNode,
    graph: &ProgramGraph,
    index: &ResolveIndex,
    surface: &DeclSurface,
    args: &[ArgDispatchInfo],
) -> (DispatchShape, Vec<Route>) {
    match receiver {
        ReceiverType::RecordRef => {
            // Catalog-only by construction (Item 4 — see the `Record` arm's
            // comment below): `RecordRef` carries no table identity (unlike
            // `Record { table }`), so there is no source candidate to shadow
            // the catalog and no collision guard is needed OR possible here.
            if let Some(bid) = member_builtin_id(MemberCatalogKind::RecordRef, method_lc) {
                member_catalog_route(bid)
            } else {
                member_unknown_route(UnknownReason::CatalogMiss)
            }
        }
        ReceiverType::FieldRef => {
            if let Some(bid) = member_builtin_id(MemberCatalogKind::FieldRef, method_lc) {
                member_catalog_route(bid)
            } else {
                member_unknown_route(UnknownReason::CatalogMiss)
            }
        }
        ReceiverType::KeyRef => {
            if let Some(bid) = member_builtin_id(MemberCatalogKind::KeyRef, method_lc) {
                member_catalog_route(bid)
            } else {
                member_unknown_route(UnknownReason::CatalogMiss)
            }
        }
        ReceiverType::Framework(kind) => {
            if let Some(bid) = member_builtin_id(MemberCatalogKind::Framework(kind), method_lc) {
                member_catalog_route(bid)
            } else {
                member_unknown_route(UnknownReason::CatalogMiss)
            }
        }
        ReceiverType::ControlAddIn { surface, .. } => match surface {
            // Unconditional accept — matches the pre-Task-1 open policy, now
            // scoped to just the small platform allowlist
            // (`TRUE_PLATFORM_CONTROL_ADDINS`) rather than every
            // `ControlAddIn`-typed receiver. The `BuiltinId` text
            // (`"ControlAddIn::{method_lc}"`) is unchanged from before the
            // refactor — real CDO goldens already carry it
            // (`tests/goldens/semantic-edges/cdo-deanon-map.json`,
            // `object_lc=ControlAddIn::...`).
            ControlAddInSurface::TruePlatform => {
                member_catalog_route(BuiltinId(format!("ControlAddIn::{method_lc}")))
            }
            // Closed-if-known gate (Task 1): `method_lc`+`arity` must match a
            // declared procedure. The platform base-member union this arm
            // would ALSO check is, per MS-Learn research
            // (`resolve_control_addin_receiver`'s doc — a control add-in's
            // properties like `Visible`/`Editable` are page-layout DESIGN-TIME
            // properties, never `CurrPage.<control>.<member>` runtime member
            // calls, and no generic AL-callable base method is documented for
            // any control add-in), EMPTY — there is nothing to union in
            // beyond `procedures` itself. A future real base-member surface,
            // if ever discovered, is added here.
            ControlAddInSurface::Declared { procedures } => {
                let declared_match = procedures
                    .iter()
                    .any(|(name_lc, params_count)| name_lc == method_lc && *params_count == arity);
                if declared_match {
                    member_catalog_route(BuiltinId(format!("ControlAddIn::{method_lc}")))
                } else {
                    member_unknown_route(UnknownReason::MemberNotFound)
                }
            }
        },
        ReceiverType::Record { table } => {
            // Source-before-catalog (beyond-1B.3b Task 1 / Item 4 — see
            // `is_bare_builtin_or_page_intrinsic`'s doc for the mirror-image
            // rationale): AL semantics say a visible source/ABI table method
            // of matching name+arity SHADOWS a same-named platform-intrinsic
            // Record method (Catalog), and this arm is DELIBERATELY NOT
            // collision-guarded the way Step 3's bare-call probe is —
            // same-receiver Source-shadows-Catalog is corpus-validated
            // correct AL precedence (42 real CDO `builtin-catalog-fp-
            // collision` instances; see
            // `resolve_member_record_source_proc_shadows_same_named_builtin`).
            // Adding a collision guard here would regress that fix back into
            // a false `Unknown` — do not "fix" this. Gather every visible
            // source/ABI candidate across the base table AND its
            // TableExtensions FIRST — visibility-scoped to `from_object`'s
            // app dependency closure, with `Local`/`Internal`/`Protected`
            // candidates excluded per `from_object`'s CALLER IDENTITY
            // (beyond-1B.3b Task 1, caller-identity-aware; see
            // `object_has_visible_member_candidate`) — only consult the
            // catalog when that scope has ZERO candidates (or the table
            // itself is unresolved/not visible — builtins still resolve
            // table-independently in that case).
            //
            // Cardinality (gpt round-2): exactly one candidate object → resolve
            // it (Source/Abi/Opaque); more than one → honest ambiguous Unknown
            // — source ambiguity STILL shadows the catalog, never fall through
            // to a false intrinsic; zero → consult the catalog.
            //
            // Task 3: `reason` defaults to `CatalogMiss` (the catalog-fallback
            // outcome below); `table: None` means Phase A already declined to
            // pin a unique receiver table (ambiguous/out-of-closure/
            // unresolved) — `ReceiverOutOfClosure`. A `NotVisible` table-scope
            // outcome with an access-excluded candidate overrides the default.
            let mut reason = UnknownReason::CatalogMiss;
            if let Some(table_id) = table {
                match resolve_in_table_scope(
                    from_object,
                    table_id.clone(),
                    method_lc,
                    arity,
                    graph,
                    index,
                    surface,
                    args,
                ) {
                    TableScopeOutcome::Resolved(shape, routes) => return (shape, routes),
                    TableScopeOutcome::Ambiguous => {
                        return (
                            DispatchShape::Exact,
                            vec![unresolved_route(UnknownReason::OverloadAmbiguous)],
                        );
                    }
                    TableScopeOutcome::NotVisible { access_excluded } => {
                        if let Some(r) = access_excluded {
                            reason = r;
                        }
                    }
                }
            } else {
                reason = UnknownReason::ReceiverOutOfClosure;
            }

            // Zero visible source/ABI candidates in scope (or `table`
            // unresolved/not visible): Record built-in methods (SetRange,
            // Find, Insert, ...) are platform-intrinsic and resolve
            // table-independently.
            if let Some(bid) = member_builtin_id(MemberCatalogKind::Record, method_lc) {
                return member_catalog_route(bid);
            }
            member_unknown_route(reason)
        }
        ReceiverType::Object { kind, name_lc, id } => {
            // Resolve the target object (topology-scoped from the calling app).
            // Task 7: when Phase A already carried a resolved id MECHANICALLY
            // (Step 0's `CurrPage.<part>.Page` subpage receiver, proven via
            // the fail-closed `ResolveIndex::resolve_object_ref`), short-
            // circuit on it directly rather than re-resolving by name — a
            // second by-name lookup could in principle disagree with the id
            // Phase A already verified unique (e.g. a same-named object
            // resolving differently), silently substituting the WRONG
            // subpage for the one the control's `target` actually names.
            let target = match id {
                Some(id) => graph.objects.iter().find(|o| &o.id == id),
                None => graph.resolve_object(from_object.id.app, *kind, name_lc),
            };
            let Some(target) = target else {
                // Target not in the graph — honest Unknown (not Opaque: we have no
                // identity for an unresolvable typed receiver). Reason-split
                // Task 2: the RECEIVER OBJECT itself is absent — `ObjectNotInGraph`.
                return member_unknown_route(UnknownReason::ObjectNotInGraph);
            };
            let target_id = target.id.clone();
            let target_tier = target.tier;

            // Entry-trigger special case (arity≤1): a declared Codeunit/Page/
            // Report-typed receiver's `Run`/`RunModal` call dispatches to the
            // target's entry trigger, mirroring `resolve_object_run`'s
            // keyword-receiver semantics via the SAME shared machinery
            // (T1.3, deep-review-remediation plan — closes the second of the
            // two classifier gaps documented on `member_catalog::
            // ENTRY_DISPATCH_BUILTIN_IDS`). Codeunit has no RunModal member
            // (MS Learn documents RunModal only on Page/Report), so "runmodal"
            // is only an entry dispatch for Page/Report.
            let is_entry_trigger_dispatch = match *kind {
                ObjectKind::Codeunit => method_lc == "run",
                ObjectKind::Page | ObjectKind::Report => {
                    method_lc == "run" || method_lc == "runmodal"
                }
                _ => false,
            };
            if is_entry_trigger_dispatch && arity <= 1 {
                let (shape, routes) =
                    dispatch_entry_trigger(&target_id, target_tier, *kind, graph, index, surface);
                return (shape, routes);
            }

            // General dispatch: resolve the method among the target object's
            // procedures. Task 1 (pageext-merge-and-final-residual plan, Page;
            // roadmap-closure plan, Task 1, Report): a `Page`- or `Report`-
            // typed receiver merges in every closure-visible `PageExtension`/
            // `ReportExtension`'s routines FIRST — an extension's routines
            // are indexed under the EXTENSION's own `ObjectNodeId`
            // (`node_extract::extract_nodes`), structurally unreachable from a
            // base-typed receiver via a plain `resolve_in_object` call on the
            // base alone (the Table analog: `resolve_in_table_scope` +
            // `table_extensions_of`). See [`resolve_in_page_scope`]'s and
            // [`resolve_in_report_scope`]'s docs for the full closure/access/
            // ambiguity design (shared engine: [`resolve_in_extendable_scope`]).
            // Every other kind (Codeunit/XmlPort/Query/…) is UNCHANGED — no
            // measured CDO population motivates merging them, and no
            // `extends_target` reverse index exists for them.
            let mut reason = UnknownReason::MemberNotFound;
            let object_dispatch = if matches!(*kind, ObjectKind::Page | ObjectKind::Report) {
                let outcome = if *kind == ObjectKind::Page {
                    resolve_in_page_scope(
                        from_object,
                        target_id.clone(),
                        method_lc,
                        arity,
                        graph,
                        index,
                        surface,
                        args,
                    )
                } else {
                    resolve_in_report_scope(
                        from_object,
                        target_id.clone(),
                        method_lc,
                        arity,
                        graph,
                        index,
                        surface,
                        args,
                    )
                };
                match outcome {
                    TableScopeOutcome::Resolved(shape, routes) => Some((shape, routes)),
                    TableScopeOutcome::Ambiguous => {
                        // Genuine >1-visible-candidate ambiguity across
                        // base∪extensions — never fall through to the
                        // instance-builtin catalog (mirrors the Record arm's
                        // identical short-circuit; source/extension ambiguity
                        // still shadows a same-named intrinsic).
                        return (
                            DispatchShape::Exact,
                            vec![unresolved_route(UnknownReason::OverloadAmbiguous)],
                        );
                    }
                    TableScopeOutcome::NotVisible { access_excluded } => {
                        if let Some(r) = access_excluded {
                            reason = r;
                        }
                        None
                    }
                }
            } else {
                resolve_in_object(
                    &target_id,
                    target_tier,
                    method_lc,
                    arity,
                    &from_object.id,
                    graph,
                    index,
                    surface,
                    args,
                )
            };

            if let Some((shape, routes)) = object_dispatch {
                (shape, routes)
            } else {
                // Method name absent from target object's declared procedures
                // (and, for Page/Report, absent from every visible
                // extension's too). Fall through to the instance-builtin
                // catalog for kinds that have one
                // (Page→PageInstance, Report→ReportInstance, Query→QueryInstance),
                // EXCLUDING only the
                // CurrPage-only `SaveRecord` (see `is_metadata_sensitive_instance_
                // method`'s doc — argtype-dispatch-and-page-catalog plan, Task 1):
                // every other Page/Report instance-catalog method (SetTableView/
                // SetRecord/GetRecord/SetSelectionFilter-class) EXISTS
                // unconditionally on every Page/Report object and is not withheld
                // here.
                if !is_metadata_sensitive_instance_method(*kind, method_lc)
                    && let Some(fk) = object_instance_framework_kind(*kind)
                    && let Some(bid) =
                        member_builtin_id(MemberCatalogKind::Framework(&fk), method_lc)
                {
                    return member_catalog_route(bid);
                }
                // The receiver object WAS resolved (`target`/`target_tier`
                // above) — member-absent-on-a-resolved-surface defaults to
                // `MemberNotFound` (reason-split Task 2); a Page merge that
                // found a same-name candidate excluded by access (Local/
                // Internal/Protected) reports that specific reason instead
                // (Task 1 — the "different-app internal declines with the
                // right reason" fixture).
                member_unknown_route_with_tier(reason, target_tier)
            }
        }
        ReceiverType::SelfObject => {
            // Dispatch to the calling object's own declared procedures.
            if let Some((shape, routes)) = resolve_in_object(
                &from_object.id,
                from_object.tier,
                method_lc,
                arity,
                &from_object.id,
                graph,
                index,
                surface,
                args,
            ) {
                (shape, routes)
            } else {
                // Method not found in own object — the receiver (from_object
                // itself) IS resolved by construction; tag its tier
                // (reason-split Task 2).
                member_unknown_route_with_tier(UnknownReason::MemberNotFound, from_object.tier)
            }
        }
        ReceiverType::Interface { name_lc } => {
            // Phase 4 Task 2: fan out to all known implementers.
            //
            // For each implementer:
            //   SymbolOnly tier  → delegate directly to `resolve_in_object`, which
            //                      (Task 1) applies the SAME arity-exact +
            //                      per-candidate-visibility selection as source —
            //                      returns AbiSymbol (unique visible arity match),
            //                      or Unknown (arity mismatch / access-excluded /
            //                      ambiguous), never an order-dependent pick.
            //   Source tier      → count arity-matched overloads first:
            //                        0 candidates → Rule 1 Unresolved (method absent/arity mismatch)
            //                        1 candidate  → unique resolution via `resolve_in_object`
            //                        >1 candidates→ Rule 2 Unresolved (ambiguous, never guess)
            //
            // A known implementer that FAILS resolution MUST emit
            // `Route{Unresolved, Unknown}` and must NOT be dropped — silently
            // dropping it would create a reachability black hole where a
            // runtime-reachable target is invisible in the call graph.
            let implementers = index.implementers_of(name_lc);
            let mut routes: Vec<Route> = Vec::with_capacity(implementers.len());

            for impl_id in implementers {
                let impl_tier = graph
                    .objects
                    .iter()
                    .find(|o| &o.id == impl_id)
                    .map(|o| o.tier)
                    .unwrap_or(TrustTier::Workspace);

                if impl_tier == TrustTier::SymbolOnly {
                    // SymbolOnly: delegate to `resolve_in_object`'s full
                    // arity+visibility discipline (Task 1) — no pre-check
                    // needed here (unlike the source-tier `else` branch below)
                    // since `resolve_in_object` itself now returns Some(Unknown)
                    // on arity mismatch/access exclusion/ambiguity. The
                    // `unwrap_or` fires only when this implementer does not
                    // declare `method_lc` at all (`resolve_in_object` returns
                    // `None` only on a name-absent `candidates.is_empty()`).
                    let result = resolve_in_object(
                        impl_id,
                        impl_tier,
                        method_lc,
                        arity,
                        &from_object.id,
                        graph,
                        index,
                        surface,
                        args,
                    );
                    // The implementer object itself IS resolved (`impl_id`/
                    // `impl_tier` above) — tag its tier (reason-split Task 2)
                    // on the name-absent fallback. `impl_tier == SymbolOnly`
                    // here by construction (this branch), so this tier can
                    // never PROVE absence — see `MemberNotFound`'s doc. A
                    // nested `AmbiguousOverload` result collapses to a single
                    // route, never extends this Polymorphic edge — see
                    // `interface_delegate_route`'s doc (Task 4 round-1
                    // addendum, interface nesting OUT OF SCOPE).
                    let route = interface_delegate_route(
                        result,
                        unresolved_route_with_tier(UnknownReason::MemberNotFound, impl_tier),
                    );
                    routes.push(route);
                } else {
                    let candidates = index.routines_in_object(impl_id, method_lc);
                    if candidates.is_empty() {
                        // Method name absent from this implementer — Rule 1
                        // Unresolved. The implementer object IS resolved; tag
                        // its tier (reason-split Task 2).
                        routes.push(unresolved_route_with_tier(
                            UnknownReason::MemberNotFound,
                            impl_tier,
                        ));
                    } else {
                        let matching = candidates
                            .iter()
                            .filter(|r| r.params_count == arity)
                            .count();
                        match matching {
                            1 => {
                                // Unique arity-matched overload: guaranteed to
                                // resolve — the `unresolved_route` fallback is
                                // defensive (should never fire;
                                // `resolve_in_object` itself finds
                                // `matched.len() == 1`, so its `_` arm's
                                // `AmbiguousOverload` shape is structurally
                                // unreachable here too — `interface_delegate_
                                // route` handles it uniformly anyway).
                                let result = resolve_in_object(
                                    impl_id,
                                    impl_tier,
                                    method_lc,
                                    arity,
                                    &from_object.id,
                                    graph,
                                    index,
                                    surface,
                                    args,
                                );
                                let route = interface_delegate_route(
                                    result,
                                    unresolved_route(UnknownReason::IndexIntegrationGap),
                                );
                                routes.push(route);
                            }
                            _ => {
                                // 0 (arity mismatch) or >1 (ambiguous) — Rule 1+2 Unresolved.
                                // Never emit a guessed route to a wrong-arity or wrong-overload target.
                                routes.push(unresolved_route(UnknownReason::OverloadAmbiguous));
                            }
                        }
                    }
                }
            }

            (DispatchShape::Polymorphic, routes)
        }
        ReceiverType::EnumType { .. } => {
            // Enum VALUE-instance surface: AsInteger / Names / Ordinals (Task
            // 4, receiver-closure-and-arg-increments plan — the split-catalog
            // closer). `FromInteger` is NOT on this surface — see
            // `member_catalog.rs`'s `ENUM_VALUE`/`ENUM_TYPE_STATIC` split doc.
            if let Some(bid) = member_builtin_id(
                MemberCatalogKind::Framework(&FrameworkKind::Enum),
                method_lc,
            ) {
                member_catalog_route(bid)
            } else {
                member_unknown_route(UnknownReason::CatalogMiss)
            }
        }
        ReceiverType::EnumTypeStatic { .. } => {
            // Enum TYPE-static surface: FromInteger / Names / Ordinals (Task
            // 4). `AsInteger` is NOT on this surface (round-2 closer, BINDING).
            if let Some(bid) = member_builtin_id(
                MemberCatalogKind::Framework(&FrameworkKind::EnumTypeStatic),
                method_lc,
            ) {
                member_catalog_route(bid)
            } else {
                member_unknown_route(UnknownReason::CatalogMiss)
            }
        }
        ReceiverType::Primitive => {
            // Non-catalog type — honest Unknown (not a false resolution gap).
            member_unknown_route(UnknownReason::CatalogMiss)
        }
        ReceiverType::Dynamic => {
            // Variant-typed receiver — genuinely dynamic, not a resolution hole.
            member_dynamic_open_route()
        }
        ReceiverType::Unknown => member_unknown_route(UnknownReason::UntrackedReceiver),
    }
}

// ---------------------------------------------------------------------------
// Cross-object call-result chain support (plan v2.1 Task 3)
// ---------------------------------------------------------------------------

/// Resolve the [`RoutineNode`] a `resolve_member` type-query's SINGLE route
/// identifies, so `receiver.rs`'s cross-object call-result chain step (`Var.
/// Method().X()`, plan v2.1 Task 3) can read its declared `return_type`/
/// `return_type_id` — the shared lookup needed regardless of which
/// [`RouteTarget`] shape the route carries.
///
/// - [`RouteTarget::Routine`] — direct: `graph.routines` is sorted by
///   `RoutineNodeId` (see `build.rs`), so a `binary_search_by` finds the
///   exact node the route already resolved to (whatever its tier — a
///   source-bodied dependency reached via `Routine` carries a real
///   `return_type` from source parsing, never a `return_type_id`; see
///   `node_extract::extract_nodes`).
/// - [`RouteTarget::AbiSymbol`] — **the ABI-PREFIX UNIQUENESS GUARD**
///   (round-1 C1+C2, round-2 arity-PROOF). The route carries no routine id.
///   Prior to Task 2, ABI parameter types were DEGRADED — `abi_ingest::
///   param_type_fp` fingerprinted only the OUTER type keyword
///   (`AbiParameter::type_text` never carried a param's `Subtype`), so two
///   genuinely different same-arity overloads differing only in an
///   object-typed parameter's Subtype (`Get(X: Codeunit A)` vs `Get(X:
///   Codeunit B)`) could hash-collide onto the identical `RoutineNodeId`.
///   Task 2 closed that: `param_type_fp` now folds a length-delimited
///   canonical tuple (outer kind + Subtype id + raw Subtype name + a
///   degradation tag), so two overloads collide ONLY when their ENTIRE tuple
///   matches — a true duplicate, or a residual collision this engine cannot
///   further distinguish (either way, correctly collapse-marked, see
///   `build::dedup_routines_preserving_genuine_overloads`'s doc). This
///   function's uniqueness proof remains necessary regardless — a naive
///   `(object, name, arity)` re-lookup still CANNOT be trusted to reproduce
///   the exact candidate `resolve_member` selected when >1 same-name/
///   same-arity siblings exist (now genuinely distinct `RoutineNodeId`s,
///   not a fp collision) — this function instead requires ALL of:
///   - the declaring `ObjectNodeId` reconstructed from `key` (the same
///     `object_number != 0 ⇒ Id` / `else ⇒ Name` convention
///     `make_routine_route`/`abi_check::RawAbiIndex` already use, and the
///     same `object_kind_from_abi_type` reversal of the key's `{:?}`-derived
///     `object_type` string);
///   - the SAME arity matcher [`resolve_in_object`] uses
///     (`rid.params_count == dispatch_arity` — tri-state-safe by
///     construction: the `UNKNOWN_ARITY` sentinel can never equal a real
///     arity);
///   - EXACTLY ONE candidate surviving BOTH that arity filter AND
///     per-candidate visibility from `from_object`
///     ([`routine_candidate_is_visible`]) — same-name/same-arity siblings
///     (the exact degraded-collision case above), or a candidate excluded by
///     access, both decline. A unique surviving candidate is, by
///     construction, the one `resolve_member` itself selected (identical
///     filters, identical order).
/// - [`RouteTarget::Builtin`] / [`RouteTarget::Unresolved`] — no routine
///   identity at all; decline.
///
/// `dispatch_arity` MUST be the exact arity `resolve_member` was called
/// with to produce `route` (the same value, never re-derived) — see
/// `receiver.rs`'s round-1 M1 note.
///
/// **Collapsed-ABI-overload guard (Task 3 review fix; sibling landed in
/// plain dispatch by Task 2 round-2 — see [`resolve_in_object`]'s own
/// PLAIN-DISPATCH MARKER GUARD).** Whichever branch resolves a node, the
/// result is rejected when [`RoutineNode::abi_overload_collapsed`] is `true`
/// — such a node is the arbitrary (or genuinely indistinguishable) survivor
/// of ≥2 raw ABI overload entries that fingerprint-collided onto one
/// `RoutineNodeId` (a true duplicate, or — post Task 2 — a residual
/// canonical-tuple collision; see `abi_ingest::param_type_fp`'s doc), so its
/// `return_type`/`return_type_id` may belong to the WRONG declaration. This
/// is checked here — the single choke point both `RouteTarget` arms funnel
/// through for the CHAIN type-query path — rather than solely inside
/// [`resolve_abi_prefix_routine`], so a future `RouteTarget::Routine`
/// producer can never accidentally bypass it (today only source-tier
/// routines reach `RouteTarget::Routine`, and `abi_overload_collapsed` is
/// unconditionally `false` for those — see
/// `build::dedup_routines_preserving_genuine_overloads` — so this is a
/// no-op on the current call graph, purely defensive).
pub(crate) fn routine_node_for_type_query<'g>(
    route: &Route,
    dispatch_arity: usize,
    from_object: &ObjectNode,
    graph: &'g ProgramGraph,
    index: &ResolveIndex,
) -> Option<&'g RoutineNode> {
    let node = match &route.target {
        RouteTarget::Routine(rid) => graph
            .routines
            .binary_search_by(|probe| probe.id.cmp(rid))
            .ok()
            .map(|i| &graph.routines[i]),
        RouteTarget::AbiSymbol { key } => {
            resolve_abi_prefix_routine(key, dispatch_arity, from_object, graph, index)
        }
        RouteTarget::Builtin(_) | RouteTarget::Unresolved => None,
    }?;
    if node.abi_overload_collapsed {
        return None;
    }
    Some(node)
}

/// The ABI-PREFIX UNIQUENESS GUARD's implementation — see
/// [`routine_node_for_type_query`]'s doc for the full rationale. Note this
/// function only proves EXACTLY ONE `RoutineNodeId` is arity+visibility
/// selectable at the ABI boundary; it does NOT by itself prove that id was
/// never a dedup collapse of ≥2 raw ABI overloads — the caller,
/// [`routine_node_for_type_query`], applies that additional
/// `abi_overload_collapsed` check uniformly to whatever node this returns.
fn resolve_abi_prefix_routine<'g>(
    key: &AbiRoutineKey,
    dispatch_arity: usize,
    from_object: &ObjectNode,
    graph: &'g ProgramGraph,
    index: &ResolveIndex,
) -> Option<&'g RoutineNode> {
    let kind = object_kind_from_abi_type(&key.object_type);
    let obj_key = if key.object_number != 0 {
        ObjKey::Id(key.object_number)
    } else {
        ObjKey::Name(key.object_name_lc.clone())
    };
    let obj_id = ObjectNodeId {
        app: key.app,
        kind,
        key: obj_key,
    };

    let visible: Vec<&RoutineNodeId> = index
        .routines_in_object(&obj_id, &key.routine_name_lc)
        .iter()
        .filter(|rid| rid.params_count == dispatch_arity)
        .filter(|rid| routine_candidate_is_visible(rid, &from_object.id, graph, index))
        .collect();
    let [rid] = visible.as_slice() else {
        return None;
    };
    graph
        .routines
        .binary_search_by(|probe| probe.id.cmp(rid))
        .ok()
        .map(|i| &graph.routines[i])
}

// ---------------------------------------------------------------------------
// Publisher-anchored EventFlow edge emission (Phase 4b Task 3)
// ---------------------------------------------------------------------------

/// `RoutineNodeId`s where ≥2 [`RoutineNode::source_overload_aliased`]
/// sibling routines are BOTH publishers (`publisher_kind.is_some()`) — a
/// TRUE dual-publisher source-overload-alias collision (Task 1,
/// sigfp-and-ambiguous-reclassification plan).
///
/// [`DeclSurface::get_with_path`]'s lookup is keyed by `RoutineNodeId` alone and
/// `DeclSurface::build` is last-write-wins on that key (see its doc), so for an
/// aliased id it can only ever return ONE decl+span — there is no way to
/// tell, from inside [`emit_event_flow_edges`]'s loop, which of the ≥2
/// physically distinct publisher declarations that single answer actually
/// belongs to. Every loop iteration for the shared id would additionally
/// push an `Edge` with the IDENTICAL `(from, site)` pair (routes too, since
/// [`ResolveIndex::subscribers_of`] is also keyed by the shared id), which
/// would silently look like a harmless duplicate to any `(from, site)`
/// dedup downstream — but could just as easily be masking a dropped
/// fan-out once Task 2 gives each overload real per-candidate identity.
/// Single-publisher-sibling aliasing (one overload is a publisher, its
/// sibling is not) is NOT in this set — that shape still emits its one
/// edge unchanged this task; only a genuine dual-publisher collision is
/// unsafe enough to fail closed on.
fn dual_publisher_alias_ids(routines: &[RoutineNode]) -> std::collections::HashSet<RoutineNodeId> {
    let mut publisher_alias_counts: std::collections::HashMap<&RoutineNodeId, usize> =
        std::collections::HashMap::new();
    for r in routines {
        if r.source_overload_aliased && r.publisher_kind.is_some() {
            *publisher_alias_counts.entry(&r.id).or_insert(0) += 1;
        }
    }
    publisher_alias_counts
        .into_iter()
        .filter(|(_, n)| *n >= 2)
        .map(|(id, _)| id.clone())
        .collect()
}

/// Number of publisher routines whose `EventFlow` edge [`emit_event_flow_edges`]
/// SKIPPED under the Task 1 dual-publisher source-overload-alias collision
/// guard (each id in [`dual_publisher_alias_ids`] contributes exactly as many
/// skips as it has publisher-kind aliased siblings — 2 for the ordinary
/// aliased-pair case). A pure re-derivation of the SAME guard `emit_event_flow_
/// edges` applies, so it stays truthful without threading a counter through
/// that function's return type. Surfaced on `ProgramReport` for the report
/// path: a nonzero value beyond the CDO-measured known-pair signatures is a
/// threshold alert (investigate, don't mask — collision-guard-observability
/// addendum).
pub fn dual_publisher_alias_skip_count(routines: &[RoutineNode]) -> usize {
    let alias_ids = dual_publisher_alias_ids(routines);
    if alias_ids.is_empty() {
        return 0;
    }
    routines
        .iter()
        .filter(|r| r.source_overload_aliased && r.publisher_kind.is_some())
        .filter(|r| alias_ids.contains(&r.id))
        .count()
}

/// Emit one `EventFlow` `Multicast` edge per publisher event routine, with
/// routes to all its resolved subscribers (from [`ResolveIndex::subscribers_of`]).
///
/// # Edge contract
///
/// - `from` = the publisher `RoutineNodeId`.
/// - `site` = `SiteId` anchored at the publisher routine's name-origin span
///   (from the `surface`; synthetic zero-span when the publisher is not in the
///   `surface`, e.g. a SymbolOnly dep or an integration gap).
/// - `kind` = `EdgeKind::EventFlow`.
/// - `shape` = `DispatchShape::Multicast`.
/// - `completeness` = `SetCompleteness::Partial { ReverseDependentSubscribers }`.
/// - `routes` = one `Route` per subscriber entry, carrying its dispatch
///   `conditions` and the subscriber's source span as `Witness::SourceSpan`.
///   For SymbolOnly subscribers: `RouteTarget::AbiSymbol` + `Evidence::Opaque`
///   (mirrors [`make_routine_route`]'s SymbolOnly path) — EXCEPT a subscriber
///   marked [`RoutineNode::abi_overload_collapsed`] (Task 2 review fix), whose
///   route is SKIPPED entirely rather than emitted `Opaque` to a possibly-wrong
///   identity; see `routine_is_collapse_marked`'s doc.
///
/// A publisher with **zero** subscribers emits an edge with **empty routes** —
/// this is an honest "published, no subscribers in snapshot" state, classified
/// as `ObligationOutcome::HonestEmpty` by `classify_obligation`. The SAME empty-
/// routes shape also results when every subscriber that WOULD have matched was
/// collapse-marked — indistinguishable from "no subscribers" at this edge's
/// granularity, and correctly so: neither case has a trustworthy target.
///
/// # Dual-publisher source-overload-alias guard (Task 1, SKIP-ONLY)
/// A publisher whose id is in [`dual_publisher_alias_ids`] is SKIPPED
/// entirely — no edge (corrupted-span or synthetic-zero-span) is emitted for
/// it. See that function's doc for why the span cannot be trusted. This is a
/// narrower guard than the collapse-marker one above: it fires only for a
/// TRUE dual-publisher alias, never for a single-publisher-sibling pair
/// (whose one publisher keeps emitting its edge unchanged) and never for a
/// non-publisher alias (irrelevant to this function). Each skip is counted by
/// [`dual_publisher_alias_skip_count`] for the report path.
///
/// # Determinism
/// Publishers are iterated in `graph.routines` order (already sorted by
/// `RoutineNodeId`); subscriber routes within each edge are already sorted by
/// subscriber `RoutineNodeId` by [`ResolveIndex::build`].
pub fn emit_event_flow_edges(
    graph: &ProgramGraph,
    index: &ResolveIndex,
    surface: &DeclSurface,
) -> Vec<Edge> {
    let mut edges = Vec::new();
    let dual_publisher_alias = dual_publisher_alias_ids(&graph.routines);

    for pub_routine in &graph.routines {
        if pub_routine.publisher_kind.is_none() {
            continue;
        }
        if dual_publisher_alias.contains(&pub_routine.id) {
            // SKIP-ONLY guard (Task 1 addendum: never a synthetic span) —
            // counted via `dual_publisher_alias_skip_count`.
            continue;
        }

        let subs = index.subscribers_of(&pub_routine.id);

        // Build one Route per subscriber (sorted by subscriber RoutineNodeId — already
        // guaranteed by ResolveIndex::build).
        // COLLAPSE-MARKER GUARD (Task 2 review fix): this subscriber fan-out
        // looks up each candidate by ROLE (an already-matched subscriber
        // entry), never through `resolve_in_object`'s name+arity selection,
        // so it must consult the marker itself — see `routine_is_collapse_
        // marked`'s doc. Unlike the OTHER three guarded sites (which
        // substitute an honest Unknown route in place of the marked
        // candidate), a marked subscriber's route is SKIPPED entirely here:
        // `SetCompleteness::Partial{ReverseDependentSubscribers}` already
        // documents this Multicast set as open-world, so dropping one
        // untrustworthy candidate doesn't understate a otherwise-closed
        // cardinality the way it would in `resolve_implicit_trigger`'s
        // fan-out.
        let routes: Vec<Route> = subs
            .iter()
            .filter(|se| !routine_is_collapse_marked(&se.subscriber, graph))
            .map(|se| {
                // Subscriber tier: look up from graph.routines (linear scan; subscriber
                // lists are small in practice).
                let sub_tier = graph
                    .routines
                    .iter()
                    .find(|r| r.id == se.subscriber)
                    .map(|r| r.tier)
                    .unwrap_or(TrustTier::Workspace);

                // Build base route using make_routine_route (handles Source/Opaque/Unknown
                // tiers via surface), then inject the subscriber's conditions.
                let mut route = make_routine_route(&se.subscriber, sub_tier, surface, graph);
                route.conditions = se.conditions.clone();
                route
            })
            .collect();

        // SiteId: anchored at the publisher routine's name-origin span.
        let site = if let Some((decl, path)) = surface.get_with_path(&pub_routine.id) {
            SiteId {
                caller: pub_routine.id.clone(),
                span: CanonicalSpan {
                    unit: path.to_string(),
                    start: SourcePos {
                        line: decl.name_origin.start.row,
                        col: decl.name_origin.start.column,
                    },
                    end: SourcePos {
                        line: decl.name_origin.end.row,
                        col: decl.name_origin.end.column,
                    },
                },
                callee_fingerprint: callee_fp(&pub_routine.id.name_lc),
            }
        } else {
            // Publisher not in surface (SymbolOnly dep or integration gap):
            // use a synthetic zero-span site.
            SiteId {
                caller: pub_routine.id.clone(),
                span: CanonicalSpan {
                    unit: String::new(),
                    start: SourcePos { line: 0, col: 0 },
                    end: SourcePos { line: 0, col: 0 },
                },
                callee_fingerprint: callee_fp(&pub_routine.id.name_lc),
            }
        };

        edges.push(Edge {
            from: pub_routine.id.clone(),
            site,
            kind: EdgeKind::EventFlow,
            shape: DispatchShape::Multicast,
            completeness: SetCompleteness::Partial {
                reason: OpenWorldReason::ReverseDependentSubscribers,
            },
            routes,
        });
    }

    edges
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {

    /// The WIRING, not the catalog: `ObjectKind::Query` must map to a framework
    /// kind, or a `V: Query "X"; V.Open()` site resolves its receiver and then
    /// has no catalog to look the member up in — which is exactly how 3 CDO
    /// sites became `Unknown(MemberNotFound)` and held the north-star metric
    /// off zero (found 2026-09-13).
    ///
    /// This pins the LINK. `member_catalog`'s own tests pin the member SET; the
    /// set being right is useless if nothing routes to it, and that is the half
    /// that was actually missing.
    ///
    /// DISCRIMINATION PROOF (recorded 2026-09-13): delete the
    /// `ObjectKind::Query => Some(FrameworkKind::QueryInstance)` arm in
    /// `object_instance_framework_kind` so `Query` falls to `_ => None` — this
    /// test FAILS (`left: None, right: Some(QueryInstance)`). Restore it — it
    /// PASSES. Verified both directions.
    #[test]
    fn query_objects_route_to_the_query_instance_catalog() {
        assert_eq!(
            object_instance_framework_kind(ObjectKind::Query),
            Some(FrameworkKind::QueryInstance),
            "a Query object must reach an instance-builtin catalog"
        );
        // The end-to-end consequence: the three members CDO actually calls
        // resolve through that catalog. Stated literally, so this survives any
        // change to how the catalog is spelled.
        for m in ["open", "read", "close"] {
            let fk = object_instance_framework_kind(ObjectKind::Query)
                .expect("Query must have a framework kind");
            assert!(
                member_builtin_id(MemberCatalogKind::Framework(&fk), m).is_some(),
                "Query.{m}() must resolve via the instance catalog"
            );
        }
        // Page/Report unchanged — the positive control that keeps a blanket
        // `Some(..)` from passing this test.
        assert_eq!(
            object_instance_framework_kind(ObjectKind::Codeunit),
            None,
            "Codeunit still has no instance-builtin catalog"
        );
    }
    use super::*;

    use crate::engine::deps::symbol_reference::SubtypeTag;
    use crate::program::graph::{ObjectIndex, ProgramGraph};
    use crate::program::node::AppRegistry;
    use crate::program::node_extract::{
        AbiParamRetained, AbiParams, Access, ObjectNode, RoutineNode, extract_nodes,
    };
    use crate::program::resolve::arg_dispatch::{CanonicalArgType, LiteralKind};
    use crate::program::resolve::decl_surface::DeclSurface;
    use crate::program::resolve::edge::{
        Condition, DispatchShape, Edge, EdgeKind, Evidence, Histogram, ObligationOutcome,
        OpenWorldReason, RouteTarget, SetCompleteness, Witness, classify_obligation,
    };
    use crate::program::resolve::index::ResolveIndex;
    use crate::program::topology::DependencyGraph;
    use crate::snapshot::{AppId, ParsedFile, ParsedUnit, Provenance, TrustTier};
    use al_syntax::ir::ObjectKind;

    // -----------------------------------------------------------------------
    // Test helpers
    // -----------------------------------------------------------------------

    fn make_app_id(name: &str) -> AppId {
        AppId {
            guid: String::new(),
            name: name.into(),
            publisher: "Test".into(),
            version: "1.0.0.0".into(),
        }
    }

    /// Parse AL source into a `ParsedUnit` with the given virtual path.
    fn make_unit(app_id: AppId, virtual_path: &str, src: &'static str) -> ParsedUnit {
        let provenance = Provenance {
            app: app_id.clone(),
            tier: TrustTier::Workspace,
            content_hash: String::new(),
        };
        ParsedUnit {
            app: app_id,
            files: vec![ParsedFile {
                virtual_path: virtual_path.to_string(),
                file: std::sync::Arc::new(al_syntax::parse(src)),
                provenance,
                text: src.into(),
            }],
        }
    }

    /// Build a `ProgramGraph` from one or more `ParsedUnit`s and an optional
    /// dependency edge `(from_app_name, to_app_name)`.
    ///
    /// All units must be fully registered in the graph before returning.
    fn build_graph(units: &[ParsedUnit], dep_edge: Option<(&str, &str)>) -> ProgramGraph {
        let mut apps = AppRegistry::default();
        let mut objects: Vec<ObjectNode> = Vec::new();
        let mut routines: Vec<RoutineNode> = Vec::new();

        for unit in units {
            let app_ref = apps.intern(&unit.app);
            for pf in &unit.files {
                extract_nodes(
                    app_ref,
                    &pf.file,
                    pf.provenance.tier,
                    &mut objects,
                    &mut routines,
                );
            }
        }

        objects.sort_by(|a, b| a.id.cmp(&b.id));
        routines.sort_by(|a, b| a.id.cmp(&b.id));

        let mut topology = DependencyGraph::default();
        if let Some((from_name, to_name)) = dep_edge {
            let from_ref = apps.find_by_name(from_name).expect("from app");
            let to_ref = apps.find_by_name(to_name).expect("to app");
            topology.add_dependency(from_ref, to_ref);
        }

        let obj_index = ObjectIndex::build(&objects);
        ProgramGraph {
            apps,
            topology,
            objects,
            routines,
            obj_index,
            ..Default::default()
        }
    }

    /// Build a `ProgramGraph` from one or more `ParsedUnit`s and any number of
    /// dependency edges `(from_app_name, to_app_name)` — the multi-edge
    /// counterpart to [`build_graph`], needed for Task 2's cross-app
    /// visibility-scoping tests (e.g. App A depends on App B but NOT App C).
    fn build_graph_multi_dep(units: &[ParsedUnit], deps: &[(&str, &str)]) -> ProgramGraph {
        let mut apps = AppRegistry::default();
        let mut objects: Vec<ObjectNode> = Vec::new();
        let mut routines: Vec<RoutineNode> = Vec::new();

        for unit in units {
            let app_ref = apps.intern(&unit.app);
            for pf in &unit.files {
                extract_nodes(
                    app_ref,
                    &pf.file,
                    pf.provenance.tier,
                    &mut objects,
                    &mut routines,
                );
            }
        }

        objects.sort_by(|a, b| a.id.cmp(&b.id));
        routines.sort_by(|a, b| a.id.cmp(&b.id));

        let mut topology = DependencyGraph::default();
        for (from_name, to_name) in deps {
            let from_ref = apps.find_by_name(from_name).expect("from app");
            let to_ref = apps.find_by_name(to_name).expect("to app");
            topology.add_dependency(from_ref, to_ref);
        }

        let obj_index = ObjectIndex::build(&objects);
        ProgramGraph {
            apps,
            topology,
            objects,
            routines,
            obj_index,
            ..Default::default()
        }
    }

    /// Build a `ProgramGraph` from `ParsedUnit`s, dependency edges, and
    /// `internalsVisibleTo` friend-app authorizations — the Task 1.5
    /// counterpart to [`build_graph_multi_dep`]. `friends` is
    /// `(exposing_app_name, friend_app_name)`: the FIRST name's app is the
    /// one whose manifest declares the SECOND name's app a friend (mirrors
    /// `graph.friends`'s one-directional, exposing-app-keyed semantics —
    /// see `internal_visible_across`'s doc).
    fn build_graph_multi_dep_friends(
        units: &[ParsedUnit],
        deps: &[(&str, &str)],
        friends: &[(&str, &str)],
    ) -> ProgramGraph {
        let mut apps = AppRegistry::default();
        let mut objects: Vec<ObjectNode> = Vec::new();
        let mut routines: Vec<RoutineNode> = Vec::new();

        for unit in units {
            let app_ref = apps.intern(&unit.app);
            for pf in &unit.files {
                extract_nodes(
                    app_ref,
                    &pf.file,
                    pf.provenance.tier,
                    &mut objects,
                    &mut routines,
                );
            }
        }

        objects.sort_by(|a, b| a.id.cmp(&b.id));
        routines.sort_by(|a, b| a.id.cmp(&b.id));

        let mut topology = DependencyGraph::default();
        for (from_name, to_name) in deps {
            let from_ref = apps.find_by_name(from_name).expect("from app");
            let to_ref = apps.find_by_name(to_name).expect("to app");
            topology.add_dependency(from_ref, to_ref);
        }

        let mut friends_map: std::collections::HashMap<AppRef, std::collections::BTreeSet<AppRef>> =
            std::collections::HashMap::new();
        for (exposing_name, friend_name) in friends {
            let exposing_ref = apps.find_by_name(exposing_name).expect("exposing app");
            let friend_ref = apps.find_by_name(friend_name).expect("friend app");
            friends_map
                .entry(exposing_ref)
                .or_default()
                .insert(friend_ref);
        }

        let obj_index = ObjectIndex::build(&objects);
        ProgramGraph {
            apps,
            topology,
            objects,
            routines,
            obj_index,
            friends: friends_map,
            abi_ingest_errors: Default::default(),
        }
    }

    /// Find the `ObjectNode` with the given lowercase name in `graph.objects`.
    fn find_obj<'g>(graph: &'g ProgramGraph, name_lc: &str) -> &'g ObjectNode {
        graph
            .objects
            .iter()
            .find(|o| o.name.eq_ignore_ascii_case(name_lc))
            .unwrap_or_else(|| panic!("object {name_lc} not found in graph"))
    }

    // -----------------------------------------------------------------------
    // (a) bare call to an own-object procedure → Source evidence + SourceSpan
    // -----------------------------------------------------------------------

    #[test]
    fn bare_own_object_resolves_to_source_route() {
        let src: &'static str = r#"
codeunit 50100 "MyCU"
{
    procedure DoFoo()
    begin
    end;
}
"#;
        let app_id = make_app_id("TestApp");
        let unit = make_unit(app_id, "MyCU.al", src);
        let units = [unit];
        let graph = build_graph(&units, None);
        let index = ResolveIndex::build(&graph);
        let surface = DeclSurface::build(&graph, &units);

        let from_obj = find_obj(&graph, "MyCU");
        let (_shape, routes) = resolve_bare(
            from_obj,
            "dofoo",
            0,
            &graph,
            &index,
            &surface,
            WithState::NoWithProven,
        );

        assert_eq!(routes.len(), 1, "expected exactly one route");
        let r = &routes[0];
        assert!(
            matches!(r.target, RouteTarget::Routine(_)),
            "target must be Routine"
        );
        assert_eq!(r.evidence, Evidence::Source, "evidence must be Source");
        assert!(
            matches!(r.witness, Witness::SourceSpan { .. }),
            "witness must be SourceSpan; got {:?}",
            r.witness
        );
        // The file coordinate must point to the virtual path we supplied.
        let Witness::SourceSpan { ref file, span } = r.witness else {
            unreachable!()
        };
        assert_eq!(file, "MyCU.al");
        // Span must be a non-empty range.
        assert!(span.1 > span.0, "byte span must be non-empty");
    }

    // -----------------------------------------------------------------------
    // (b) bare `Message` → Builtin evidence + CatalogEntry witness
    // -----------------------------------------------------------------------

    #[test]
    fn bare_global_builtin_message_emits_catalog_route() {
        let src: &'static str = r#"
codeunit 50101 "CallerCU"
{
    procedure Run()
    begin
    end;
}
"#;
        let app_id = make_app_id("TestApp");
        let unit = make_unit(app_id, "CallerCU.al", src);
        let units = [unit];
        let graph = build_graph(&units, None);
        let index = ResolveIndex::build(&graph);
        let surface = DeclSurface::build(&graph, &units);

        let from_obj = find_obj(&graph, "CallerCU");
        // "message" (1 arg) is a recognized global builtin.
        let (_shape, routes) = resolve_bare(
            from_obj,
            "message",
            1,
            &graph,
            &index,
            &surface,
            WithState::NoWithProven,
        );

        assert_eq!(routes.len(), 1);
        let r = &routes[0];
        assert!(
            matches!(r.target, RouteTarget::Builtin(_)),
            "target must be Builtin"
        );
        assert_eq!(r.evidence, Evidence::Catalog);
        assert!(
            matches!(r.witness, Witness::CatalogEntry { .. }),
            "witness must be CatalogEntry"
        );
        // Builtin id must be the lowercased name.
        let RouteTarget::Builtin(ref bid) = r.target else {
            unreachable!()
        };
        assert_eq!(bid.0, "message");
        // CatalogEntry id must match.
        let Witness::CatalogEntry {
            ref id,
            ref catalog_version,
        } = r.witness
        else {
            unreachable!()
        };
        assert_eq!(id.0, "message");
        assert!(!catalog_version.is_empty());
    }

    // -----------------------------------------------------------------------
    // pageext-merge-and-final-residual plan, Task 2: the GLOBAL suppression
    // of compiler-grounded instance-only names (`INSTANCE_ONLY_NEVER_BARE`)
    // from the bare-call builtin candidate set — both the Step 3 collision
    // guard and the Step 4 plain catalog fallback. Real site:
    // `CDOEMailJobs.Page.al:125`'s bare `Run()` vs `CDOEMailJob.Table.al:192`'s
    // `procedure Run()`.
    // -----------------------------------------------------------------------

    /// POSITIVE (Site B): a Page's own action-trigger-style procedure calls
    /// bare `Run()`; the page's SourceTable declares its OWN `procedure
    /// Run()`. Pre-fix this collided (`run` ∈ `PAGE_INSTANCE` ∧ ∈
    /// `GLOBAL_BUILTIN_METHODS`) and fell back to
    /// `Unknown(BuiltinPrecedenceCollision)`; post-fix, `run` is
    /// compiler-grounded never-bare, so the table's own procedure wins
    /// outright.
    #[test]
    fn bare_run_on_page_resolves_to_sourcetable_procedure() {
        let src_table: &'static str = r#"
table 50900 "EMailJob"
{
    procedure Run()
    begin
    end;
}
"#;
        let src_page: &'static str = r#"
page 50901 "EMailJobsPage"
{
    SourceTable = EMailJob;

    procedure Test()
    begin
        Run();
    end;
}
"#;
        let app_id = make_app_id("TestApp");
        let unit_table = make_unit(app_id.clone(), "EMailJob.al", src_table);
        let unit_page = make_unit(app_id, "EMailJobsPage.al", src_page);
        let units = [unit_table, unit_page];
        let graph = build_graph(&units, None);
        let index = ResolveIndex::build(&graph);
        let surface = DeclSurface::build(&graph, &units);

        let from_obj = find_obj(&graph, "EMailJobsPage");
        let (shape, routes) = resolve_bare(
            from_obj,
            "run",
            0,
            &graph,
            &index,
            &surface,
            WithState::NoWithProven,
        );

        assert_eq!(shape, DispatchShape::Exact);
        assert_eq!(routes.len(), 1);
        assert!(
            matches!(routes[0].target, RouteTarget::Routine(_)),
            "the table's own Run() procedure must win, no collision; got {:?}",
            routes[0].target
        );
        assert_eq!(routes[0].evidence, Evidence::Source);
        let RouteTarget::Routine(ref rid) = routes[0].target else {
            unreachable!()
        };
        assert_eq!(rid.object.kind, ObjectKind::Table);
    }

    /// NEGATIVE (the critical fix): a Page's SourceTable does NOT declare
    /// `Run()` at all — no table-scope candidate exists. Pre-fix this fell
    /// through Step 3 (NotVisible, no collision to even detect) straight
    /// into Step 4's UNGUARDED `global_builtin_id("run")` fallback →
    /// `Catalog`/`Builtin` (a false edge — `run` has no bare-call form in
    /// AL). Post-fix: Step 4 is ALSO suppressed for a proven-never-bare
    /// name, so this correctly falls all the way to `Unknown`.
    #[test]
    fn bare_run_on_page_with_no_sourcetable_candidate_is_unknown_not_builtin() {
        let src_table: &'static str = r#"
table 50910 "Baz"
{
    procedure Foo()
    begin
    end;
}
"#;
        let src_page: &'static str = r#"
page 50911 "BazPage"
{
    SourceTable = Baz;

    procedure Test()
    begin
        Run();
    end;
}
"#;
        let app_id = make_app_id("TestApp");
        let unit_table = make_unit(app_id.clone(), "Baz.al", src_table);
        let unit_page = make_unit(app_id, "BazPage.al", src_page);
        let units = [unit_table, unit_page];
        let graph = build_graph(&units, None);
        let index = ResolveIndex::build(&graph);
        let surface = DeclSurface::build(&graph, &units);

        let from_obj = find_obj(&graph, "BazPage");
        let (shape, routes) = resolve_bare(
            from_obj,
            "run",
            0,
            &graph,
            &index,
            &surface,
            WithState::NoWithProven,
        );

        assert_eq!(shape, DispatchShape::Exact);
        assert_eq!(routes.len(), 1);
        assert!(
            !matches!(routes[0].target, RouteTarget::Builtin(_)),
            "must NEVER resolve to Builtin — `run` has no bare form in AL; got {:?}",
            routes[0].target
        );
        assert!(
            matches!(routes[0].evidence, Evidence::Unknown(_)),
            "expected Unknown evidence; got {:?}",
            routes[0].evidence
        );
    }

    /// NEGATIVE (the SAME suppression, GLOBAL not page-scoped): a Codeunit
    /// with no own `Run` procedure calls bare `Run()`. `resolve_bare`'s Step
    /// 3 is structurally skipped for every Codeunit (no implicit-Rec table),
    /// so pre-fix Step 1's miss fell straight to Step 4's unguarded
    /// `global_builtin_id("run")` → `Catalog` (false edge, page-collision
    /// logic never even in play here). Post-fix: `Unknown`.
    #[test]
    fn bare_run_on_codeunit_with_no_candidate_is_unknown_not_builtin() {
        let src: &'static str = r#"
codeunit 50920 "CallerCU2"
{
    procedure Test()
    begin
        Run();
    end;
}
"#;
        let app_id = make_app_id("TestApp");
        let unit = make_unit(app_id, "CallerCU2.al", src);
        let units = [unit];
        let graph = build_graph(&units, None);
        let index = ResolveIndex::build(&graph);
        let surface = DeclSurface::build(&graph, &units);

        let from_obj = find_obj(&graph, "CallerCU2");
        let (shape, routes) = resolve_bare(
            from_obj,
            "run",
            0,
            &graph,
            &index,
            &surface,
            WithState::NoWithProven,
        );

        assert_eq!(shape, DispatchShape::Exact);
        assert_eq!(routes.len(), 1);
        assert!(
            !matches!(routes[0].target, RouteTarget::Builtin(_)),
            "must NEVER resolve to Builtin in a Codeunit either; got {:?}",
            routes[0].target
        );
        assert!(matches!(routes[0].evidence, Evidence::Unknown(_)));
    }

    /// REGRESSION GUARD (scope discipline): an UNGROUNDED name that also
    /// happens to collide (`rename` ∈ `GLOBAL_BUILTIN_METHODS` — a real
    /// `Record.Rename` method — but NOT in `INSTANCE_ONLY_NEVER_BARE`, since
    /// it was never individually grounded per-context) must keep the
    /// PRE-EXISTING fail-closed collision behavior — this task's narrowing
    /// is deliberately scoped to the 19 grounded names only, never a
    /// blanket "any table-scope collision wins" change.
    #[test]
    fn bare_ungrounded_name_collision_on_page_remains_unproven_precedence() {
        let src_table: &'static str = r#"
table 50930 "Bar2"
{
    procedure Rename()
    begin
    end;
}
"#;
        let src_page: &'static str = r#"
page 50931 "Bar2Page"
{
    SourceTable = Bar2;

    procedure Test()
    begin
        Rename();
    end;
}
"#;
        let app_id = make_app_id("TestApp");
        let unit_table = make_unit(app_id.clone(), "Bar2.al", src_table);
        let unit_page = make_unit(app_id, "Bar2Page.al", src_page);
        let units = [unit_table, unit_page];
        let graph = build_graph(&units, None);
        let index = ResolveIndex::build(&graph);
        let surface = DeclSurface::build(&graph, &units);

        let from_obj = find_obj(&graph, "Bar2Page");
        let (shape, routes) = resolve_bare(
            from_obj,
            "rename",
            0,
            &graph,
            &index,
            &surface,
            WithState::NoWithProven,
        );

        assert_eq!(shape, DispatchShape::Exact);
        assert_eq!(routes.len(), 1);
        assert_eq!(
            routes[0].target,
            RouteTarget::Unresolved,
            "an ungrounded name's collision guard must remain unchanged; got {:?}",
            routes[0].target
        );
        assert_eq!(
            routes[0].evidence,
            Evidence::Unknown(UnknownReason::BuiltinPrecedenceCollision)
        );
    }

    // -----------------------------------------------------------------------
    // (c) bare nonexistent non-builtin → Unknown evidence + None witness
    // -----------------------------------------------------------------------

    #[test]
    fn bare_nonexistent_non_builtin_emits_unknown_route() {
        let src: &'static str = r#"
codeunit 50102 "AnotherCU"
{
    procedure Caller()
    begin
    end;
}
"#;
        let app_id = make_app_id("TestApp");
        let unit = make_unit(app_id, "AnotherCU.al", src);
        let units = [unit];
        let graph = build_graph(&units, None);
        let index = ResolveIndex::build(&graph);
        let surface = DeclSurface::build(&graph, &units);

        let from_obj = find_obj(&graph, "AnotherCU");
        let (_shape, routes) = resolve_bare(
            from_obj,
            "xyz_this_does_not_exist_at_all",
            0,
            &graph,
            &index,
            &surface,
            WithState::NoWithProven,
        );

        assert_eq!(routes.len(), 1);
        let r = &routes[0];
        assert_eq!(r.target, RouteTarget::Unresolved);
        assert!(matches!(r.evidence, Evidence::Unknown(_)));
        assert_eq!(r.witness, Witness::None);
    }

    /// Reason-split Task 2: the resolve_bare Step-5 default MemberNotFound
    /// (never overwritten by Step 2/3's more-specific reasons) is tagged with
    /// the receiver's (`from_object`'s own) `TrustTier` — a `Table` object
    /// kind is used deliberately: it is NEITHER an extension kind (Step 2
    /// skipped) NOR a Codeunit/Report (Step 3's kind-specific overwrites),
    /// and its OWN implicit-Rec table search (Step 3, `Table` → itself)
    /// legitimately finds zero visible candidates without an access-exclusion
    /// reason, so `reason` reaches Step 5 UNCHANGED — the untouched
    /// `MemberNotFound` default this test pins.
    #[test]
    fn bare_table_step5_default_member_not_found_tags_receiver_tier() {
        let src: &'static str = r#"
table 50108 "BareTableNF"
{
    procedure KnownProc()
    begin
    end;
}
"#;
        let app_id = make_app_id("TestApp");
        let unit = make_unit(app_id, "BareTableNF.al", src);
        let units = [unit];
        let graph = build_graph(&units, None);
        let index = ResolveIndex::build(&graph);
        let surface = DeclSurface::build(&graph, &units);

        let from_obj = find_obj(&graph, "BareTableNF");
        assert_eq!(
            from_obj.tier,
            TrustTier::Workspace,
            "fixture sanity: the workspace-parsed table must be Workspace-tier"
        );
        let (_shape, routes) = resolve_bare(
            from_obj,
            "xyz_this_does_not_exist_at_all",
            0,
            &graph,
            &index,
            &surface,
            WithState::NoWithProven,
        );

        assert_eq!(routes.len(), 1);
        let r = &routes[0];
        assert_eq!(r.target, RouteTarget::Unresolved);
        assert_eq!(
            r.evidence,
            Evidence::Unknown(UnknownReason::MemberNotFound),
            "the Step-5 default must stay MemberNotFound (member-absent-on-a-\
             resolved-surface: from_object itself); got {r:?}"
        );
        assert_eq!(
            r.receiver_tier,
            Some(TrustTier::Workspace),
            "MemberNotFound must tag the resolved receiver's (from_object's) \
             tier (reason-split Task 2's additive receiver_tier diagnostic)"
        );
    }

    // -----------------------------------------------------------------------
    // (d) extension calling a base-object proc → resolved via the base
    // -----------------------------------------------------------------------

    #[test]
    fn bare_extension_base_object_proc_is_resolved() {
        // App A: has TableExtension 50100 "CustomerExt" extending "Customer".
        // App B: has Table 50000 "Customer" with procedure "Init".
        // App A depends on App B.
        // resolve_bare from CustomerExt for "init" → resolves to Table Customer's Init.
        let src_a: &'static str = r#"
tableextension 50100 "CustomerExt" extends Customer
{
    procedure ExtProc()
    begin
    end;
}
"#;
        let src_b: &'static str = r#"
table 50000 Customer
{
    procedure Init()
    begin
    end;
}
"#;
        let app_a_id = make_app_id("AppA");
        let app_b_id = make_app_id("AppB");

        let unit_a = make_unit(app_a_id.clone(), "CustomerExt.al", src_a);
        let unit_b = make_unit(app_b_id, "Customer.al", src_b);
        let units = [unit_a, unit_b];

        let graph = build_graph(&units, Some(("AppA", "AppB")));
        let index = ResolveIndex::build(&graph);
        let surface = DeclSurface::build(&graph, &units);

        // from_object = the TableExtension in App A.
        let from_obj = find_obj(&graph, "CustomerExt");
        assert_eq!(from_obj.id.kind, ObjectKind::TableExtension);
        assert_eq!(
            from_obj.extends_target.as_deref(),
            Some("Customer"),
            "extends_target must be populated from IR"
        );

        // "init" is not in the extension itself — only in the base Table.
        let (_shape, routes) = resolve_bare(
            from_obj,
            "init",
            0,
            &graph,
            &index,
            &surface,
            WithState::NoWithProven,
        );

        assert_eq!(routes.len(), 1, "must resolve to exactly one route");
        let r = &routes[0];
        assert!(
            matches!(r.target, RouteTarget::Routine(_)),
            "target must be Routine (not Builtin/Unresolved)"
        );
        assert_eq!(r.evidence, Evidence::Source);
        assert!(
            matches!(r.witness, Witness::SourceSpan { .. }),
            "witness must be SourceSpan"
        );

        // Sanity: target routine is in the base table (App B), not the extension.
        let RouteTarget::Routine(ref rid) = r.target else {
            unreachable!()
        };
        let app_b_ref = graph.app_ref_by_name("AppB");
        assert_eq!(
            rid.object.app, app_b_ref,
            "resolved routine must live in AppB (the base table's app)"
        );
        assert_eq!(rid.name_lc, "init");
    }

    // -----------------------------------------------------------------------
    // Task 1.5 — resolve_bare Step 2 ("extension base") access filtering.
    //
    // Step 2 resolves a bare call against the caller's extended BASE object
    // via `resolve_in_object`, which does ZERO access filtering (unlike
    // `resolve_in_table_scope`, Task 1's caller-identity-aware path). Pre-fix,
    // ANY base member — regardless of declared `Access` — false-resolved to
    // `Source`. These fixtures pin the exact pre-fix wrong route (Source to
    // the inaccessible base member) and the post-fix honest `Unknown`, plus
    // two controls proving Step 2 still works for `Public`/`Protected` (the
    // extension trivially extends its own base, so `Protected` is visible).
    // -----------------------------------------------------------------------

    // (a) TableExtension `ExtA extends Base` bare-calls a `local procedure
    // L()` declared on `Base`. AL `local` is OBJECT-scoped — visible only to
    // `Base` itself, never to ANY of its extensions (even a direct one) — so
    // this must decline to `Unknown`. Pre-fix: false `Source` to `Base.L`.
    #[test]
    fn bare_extension_base_local_method_excluded() {
        let src_base: &'static str = r#"
table 52900 "Base"
{
    local procedure L()
    begin
    end;
}
"#;
        let src_ext: &'static str = r#"
tableextension 52901 "ExtA" extends Base
{
}
"#;
        let app_id = make_app_id("TestApp");
        let unit_base = make_unit(app_id.clone(), "Base.al", src_base);
        let unit_ext = make_unit(app_id, "ExtA.al", src_ext);
        let units = [unit_base, unit_ext];
        let graph = build_graph(&units, None);
        let index = ResolveIndex::build(&graph);
        let surface = DeclSurface::build(&graph, &units);

        let from_obj = find_obj(&graph, "ExtA");
        assert_eq!(from_obj.id.kind, ObjectKind::TableExtension);

        let (_shape, routes) = resolve_bare(
            from_obj,
            "l",
            0,
            &graph,
            &index,
            &surface,
            WithState::NoWithProven,
        );

        assert_eq!(routes.len(), 1);
        assert_eq!(
            routes[0].target,
            RouteTarget::Unresolved,
            "a base table's `local` procedure must NOT be visible to a bare \
             call from ANY of its extensions (base-self only); got {:?}",
            routes[0].target
        );
        assert!(matches!(routes[0].evidence, Evidence::Unknown(_)));
    }

    // (b) CONTROL: same shape as (a), but `Base` declares a `Public`
    // (default-visibility) `procedure Pub()` — Step 2 must still resolve this
    // to `Source`, unchanged by the access filter.
    #[test]
    fn bare_extension_base_public_method_control_resolves_to_source() {
        let src_base: &'static str = r#"
table 52902 "Base"
{
    procedure Pub()
    begin
    end;
}
"#;
        let src_ext: &'static str = r#"
tableextension 52903 "ExtA" extends Base
{
}
"#;
        let app_id = make_app_id("TestApp");
        let unit_base = make_unit(app_id.clone(), "Base.al", src_base);
        let unit_ext = make_unit(app_id, "ExtA.al", src_ext);
        let units = [unit_base, unit_ext];
        let graph = build_graph(&units, None);
        let index = ResolveIndex::build(&graph);
        let surface = DeclSurface::build(&graph, &units);

        let from_obj = find_obj(&graph, "ExtA");
        let (_shape, routes) = resolve_bare(
            from_obj,
            "pub",
            0,
            &graph,
            &index,
            &surface,
            WithState::NoWithProven,
        );

        assert_eq!(routes.len(), 1);
        assert!(
            matches!(routes[0].target, RouteTarget::Routine(_)),
            "a base table's `Public` procedure must still resolve via Step \
             2; got {:?}",
            routes[0].target
        );
        assert_eq!(routes[0].evidence, Evidence::Source);
    }

    // (c) cross-app `internal`: `ExtA` (App A) bare-calls an `internal
    // procedure I()` declared on `Base` (App B, a dependency of App A —
    // App A extends a base object it does not own). `internal` is app-scoped;
    // cross-app must decline to `Unknown`. Pre-fix: false `Source` to `Base.I`.
    #[test]
    fn bare_extension_base_cross_app_internal_method_excluded() {
        let src_base: &'static str = r#"
table 52904 "Base"
{
    internal procedure I()
    begin
    end;
}
"#;
        let src_ext: &'static str = r#"
tableextension 52905 "ExtA" extends Base
{
}
"#;
        let app_a = make_app_id("AppA");
        let app_b = make_app_id("AppB");
        let unit_base = make_unit(app_b, "Base.al", src_base);
        let unit_ext = make_unit(app_a, "ExtA.al", src_ext);
        let units = [unit_base, unit_ext];
        let graph = build_graph(&units, Some(("AppA", "AppB")));
        let index = ResolveIndex::build(&graph);
        let surface = DeclSurface::build(&graph, &units);

        let from_obj = find_obj(&graph, "ExtA");
        let (_shape, routes) = resolve_bare(
            from_obj,
            "i",
            0,
            &graph,
            &index,
            &surface,
            WithState::NoWithProven,
        );

        assert_eq!(routes.len(), 1);
        assert_eq!(
            routes[0].target,
            RouteTarget::Unresolved,
            "a cross-app `internal` base method must NOT be visible to a \
             bare call from an extension in a DIFFERENT app; got {:?}",
            routes[0].target
        );
        assert!(matches!(routes[0].evidence, Evidence::Unknown(_)));
    }

    // (d) CONTROL: `ExtA` bare-calls a `protected procedure P()` on `Base` —
    // the extension DOES see the base's `protected` member (Step 2's caller
    // is by construction a direct, kind-compatible extension of the base, so
    // the self-or-extends check always holds — confirms this incidentally-
    // safe path stays correct after the access filter is added).
    #[test]
    fn bare_extension_base_protected_method_control_resolves_to_source() {
        let src_base: &'static str = r#"
table 52906 "Base"
{
    protected procedure P()
    begin
    end;
}
"#;
        let src_ext: &'static str = r#"
tableextension 52907 "ExtA" extends Base
{
}
"#;
        let app_id = make_app_id("TestApp");
        let unit_base = make_unit(app_id.clone(), "Base.al", src_base);
        let unit_ext = make_unit(app_id, "ExtA.al", src_ext);
        let units = [unit_base, unit_ext];
        let graph = build_graph(&units, None);
        let index = ResolveIndex::build(&graph);
        let surface = DeclSurface::build(&graph, &units);

        let from_obj = find_obj(&graph, "ExtA");
        let (_shape, routes) = resolve_bare(
            from_obj,
            "p",
            0,
            &graph,
            &index,
            &surface,
            WithState::NoWithProven,
        );

        assert_eq!(routes.len(), 1);
        assert!(
            matches!(routes[0].target, RouteTarget::Routine(_)),
            "a base table's `protected` procedure must be visible to a bare \
             call from its own extension; got {:?}",
            routes[0].target
        );
        assert_eq!(routes[0].evidence, Evidence::Source);
    }

    // (e) PageExtension→base-Page variant of (a): same `local`-excluded rule
    // generalized to a non-Table extension kind.
    #[test]
    fn bare_pageextension_base_local_method_excluded() {
        let src_page: &'static str = r#"
page 52908 "BasePage"
{
    local procedure L()
    begin
    end;
}
"#;
        let src_ext: &'static str = r#"
pageextension 52909 "ExtA" extends BasePage
{
}
"#;
        let app_id = make_app_id("TestApp");
        let unit_page = make_unit(app_id.clone(), "BasePage.al", src_page);
        let unit_ext = make_unit(app_id, "ExtA.al", src_ext);
        let units = [unit_page, unit_ext];
        let graph = build_graph(&units, None);
        let index = ResolveIndex::build(&graph);
        let surface = DeclSurface::build(&graph, &units);

        let from_obj = find_obj(&graph, "ExtA");
        assert_eq!(from_obj.id.kind, ObjectKind::PageExtension);

        let (_shape, routes) = resolve_bare(
            from_obj,
            "l",
            0,
            &graph,
            &index,
            &surface,
            WithState::NoWithProven,
        );

        assert_eq!(routes.len(), 1);
        assert_eq!(
            routes[0].target,
            RouteTarget::Unresolved,
            "a base Page's `local` procedure must NOT be visible to a bare \
             call from ANY of its PageExtensions; got {:?}",
            routes[0].target
        );
        assert!(matches!(routes[0].evidence, Evidence::Unknown(_)));
    }

    // (f) PageExtension→base-Page variant of (b) CONTROL: `Public` still
    // resolves via Step 2.
    #[test]
    fn bare_pageextension_base_public_method_control_resolves_to_source() {
        let src_page: &'static str = r#"
page 52910 "BasePage"
{
    procedure Pub()
    begin
    end;
}
"#;
        let src_ext: &'static str = r#"
pageextension 52911 "ExtA" extends BasePage
{
}
"#;
        let app_id = make_app_id("TestApp");
        let unit_page = make_unit(app_id.clone(), "BasePage.al", src_page);
        let unit_ext = make_unit(app_id, "ExtA.al", src_ext);
        let units = [unit_page, unit_ext];
        let graph = build_graph(&units, None);
        let index = ResolveIndex::build(&graph);
        let surface = DeclSurface::build(&graph, &units);

        let from_obj = find_obj(&graph, "ExtA");
        let (_shape, routes) = resolve_bare(
            from_obj,
            "pub",
            0,
            &graph,
            &index,
            &surface,
            WithState::NoWithProven,
        );

        assert_eq!(routes.len(), 1);
        assert!(
            matches!(routes[0].target, RouteTarget::Routine(_)),
            "a base Page's `Public` procedure must still resolve via Step 2; \
             got {:?}",
            routes[0].target
        );
        assert_eq!(routes[0].evidence, Evidence::Source);
    }

    // -----------------------------------------------------------------------
    // Task 2 fold-in (cross-object-chains-and-protected-abi plan v2.1): the
    // genuine boundary fixture Task 1's fixture (g) missed.
    //
    // Task 1's (g)/(i) (`ws_protected_abi_wrong_arity_single_overload_no_emit`
    // in `tests/program_resolve_harness.rs`) proved "existence ≠ emission" for
    // a wrong-arity SymbolOnly candidate, but it drove the call through an
    // OBJECT-receiver dispatch (`resolve_member`'s Object arm →
    // `resolve_in_object` directly) — a path that never consults
    // `object_has_visible_member_candidate`'s existence boolean at all. Task
    // 1's OWN "caller audit" section identified the boolean's REAL callers as
    // exactly two: `resolve_bare` Step 2 (the extension-base gate below) and
    // `resolve_in_table_scope`'s cardinality filter. Neither was exercised by
    // (g). This fixture closes that gap: a SymbolOnly base object exposes a
    // SINGLE same-name candidate at the WRONG arity. `object_has_member_
    // candidate`'s SymbolOnly branch is deliberately arity-DEFERRED (an
    // `.any()` name-only scan — existence only), so Step 2's gate reports
    // "exists" and proceeds to call `resolve_in_object`; that function's own
    // arity-exact selection must then be the one and only place that decides,
    // correctly declining rather than leaking the existence boolean's
    // arity-blindness into a false emission.
    // -----------------------------------------------------------------------

    #[test]
    fn bare_extension_base_symbolonly_wrong_arity_existence_never_leaks_into_emission() {
        // App WS: ReportExtension 50100 "MyRptExt" extends Report "BaseRpt" (a
        // SymbolOnly dep object in App Dep, declaring the ONLY overload of
        // "DoFoo" at arity 0). App WS depends on App Dep.
        let ws_id = make_app_id("WS");
        let dep_id = make_app_id("DepApp");

        let mut apps = AppRegistry::default();
        let ws_ref = apps.intern(&ws_id);
        let dep_ref = apps.intern(&dep_id);

        let ext_obj_id = ObjectNodeId {
            app: ws_ref,
            kind: ObjectKind::ReportExtension,
            key: ObjKey::Id(50100),
        };
        let base_obj_id = ObjectNodeId {
            app: dep_ref,
            kind: ObjectKind::Report,
            key: ObjKey::Id(60000),
        };

        let objects = vec![
            ObjectNode {
                id: ext_obj_id.clone(),
                name: "MyRptExt".into(),
                declared_id: Some(50100),
                extends_target: Some("BaseRpt".into()),
                implements: vec![],
                tier: TrustTier::Workspace,
                source_table: None,
                table_no: None,
                source_table_temporary: false,
                page_controls: vec![],
                fields: vec![],
                dataitems: vec![],
                parse_incomplete: false,
            },
            ObjectNode {
                id: base_obj_id.clone(),
                name: "BaseRpt".into(),
                declared_id: Some(60000),
                extends_target: None,
                implements: vec![],
                tier: TrustTier::SymbolOnly,
                source_table: None,
                table_no: None,
                source_table_temporary: false,
                page_controls: vec![],
                fields: vec![],
                dataitems: vec![],
                parse_incomplete: false,
            },
        ];

        // The ONLY "DoFoo" overload on the SymbolOnly base is arity 0 (public).
        let routines = vec![RoutineNode {
            id: RoutineNodeId {
                object: base_obj_id.clone(),
                name_lc: "dofoo".into(),
                enclosing_member_lc: None,
                params_count: 0,
                sig_fp: 0,
            },
            name: "DoFoo".into(),
            is_trigger: false,
            access: Access::Public,
            tier: TrustTier::SymbolOnly,
            event_subscribers: vec![],
            subscriber_instance_manual: false,
            publisher_kind: None,
            include_sender: None,
            abi_routine_kind: Some(AbiRoutineKind::Procedure),
            abi_event_kind: Some(AbiEventKind::None),
            param_sig_key: String::new(),
            return_type: None,
            return_type_id: None,
            abi_overload_collapsed: false,
            source_overload_aliased: false,
            abi_params: AbiParams::Missing,
        }];

        let mut topology = DependencyGraph::default();
        topology.add_dependency(ws_ref, dep_ref);

        let obj_index = ObjectIndex::build(&objects);
        let graph = ProgramGraph {
            apps,
            topology,
            objects,
            routines,
            obj_index,
            ..Default::default()
        };
        let index = ResolveIndex::build(&graph);
        let surface = DeclSurface::build(&graph, &[]);

        let from_obj = graph
            .objects
            .iter()
            .find(|o| o.id == ext_obj_id)
            .expect("MyRptExt must exist");

        // Sanity: the arity-DEFERRED existence scan reports "exists" even
        // though the sole candidate is arity 0 and we are about to probe
        // arity 2 — proving the leak vector this test guards against is real
        // at the boolean layer, not merely hypothetical.
        assert!(
            object_has_visible_member_candidate(
                &base_obj_id,
                TrustTier::SymbolOnly,
                "dofoo",
                2,
                &from_obj.id,
                &graph,
                &index,
            ),
            "SymbolOnly existence scan is deliberately arity-deferred — it must \
             report 'exists' regardless of the requested arity"
        );

        // The REAL resolution call (Step 2 of resolve_bare) must NOT emit a
        // route to the wrong-arity candidate despite that existence signal.
        let (_shape, routes) = resolve_bare(
            from_obj,
            "dofoo",
            2,
            &graph,
            &index,
            &surface,
            WithState::NoWithProven,
        );

        assert_eq!(routes.len(), 1);
        let r = &routes[0];
        assert_eq!(
            r.target,
            RouteTarget::Unresolved,
            "existence-only must never leak into a false emission at the wrong \
             arity; got {r:?}"
        );
        assert!(
            matches!(r.evidence, Evidence::Unknown(UnknownReason::ArityMismatch)),
            "name found, no arity-matched overload → Unknown(ArityMismatch) \
             (reason-split Task 2 — was OverloadAmbiguous pre-split); got {r:?}"
        );
        assert_eq!(r.witness, Witness::None);
    }

    // -----------------------------------------------------------------------
    // Task 2 round-2 addendum: PLAIN-DISPATCH MARKER GUARD (round-1
    // critical). Before this fix, `abi_overload_collapsed` gated ONLY the
    // chain-type-query boundary (`routine_node_for_type_query`) — a marked
    // survivor could still resolve CONFIDENTLY via ordinary PLAIN dispatch
    // (`resolve_in_object`'s single-visible-candidate arm). These two
    // fixtures build a minimal graph with the marker ALREADY set (bypassing
    // ingestion/dedup entirely — this targets the SELECTION guard in
    // isolation) and prove: (f) a MARKED candidate declines even though it
    // is the sole arity-matched, visible candidate; (control) an UNMARKED
    // candidate in the identical shape still resolves normally — the guard
    // must not over-decline.
    // -----------------------------------------------------------------------

    fn plain_dispatch_marker_guard_fixture(
        collapsed: bool,
    ) -> (ProgramGraph, ResolveIndex, DeclSurface, ObjectNodeId) {
        let ws_id = make_app_id("WS");
        let dep_id = make_app_id("DepApp");

        let mut apps = AppRegistry::default();
        let ws_ref = apps.intern(&ws_id);
        let dep_ref = apps.intern(&dep_id);

        let caller_obj_id = ObjectNodeId {
            app: ws_ref,
            kind: ObjectKind::Codeunit,
            key: ObjKey::Id(50600),
        };
        let dep_obj_id = ObjectNodeId {
            app: dep_ref,
            kind: ObjectKind::Codeunit,
            key: ObjKey::Id(60104),
        };

        let objects = vec![
            ObjectNode {
                id: caller_obj_id.clone(),
                name: "Caller".into(),
                declared_id: Some(50600),
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
            },
            ObjectNode {
                id: dep_obj_id.clone(),
                name: "Dep Collapse".into(),
                declared_id: Some(60104),
                extends_target: None,
                implements: vec![],
                tier: TrustTier::SymbolOnly,
                source_table: None,
                table_no: None,
                source_table_temporary: false,
                page_controls: vec![],
                fields: vec![],
                dataitems: vec![],
                parse_incomplete: false,
            },
        ];

        // The ONLY "Get" overload visible at arity 1 — carries `abi_overload_
        // collapsed` directly (simulating dedup's post-collapse output)
        // rather than exercising real ABI ingestion, to isolate the
        // SELECTION guard from the fp/dedup mechanics Task 2's other tests
        // already cover.
        let routines = vec![RoutineNode {
            id: RoutineNodeId {
                object: dep_obj_id.clone(),
                name_lc: "get".into(),
                enclosing_member_lc: None,
                params_count: 1,
                sig_fp: 777,
            },
            name: "Get".into(),
            is_trigger: false,
            access: Access::Public,
            tier: TrustTier::SymbolOnly,
            event_subscribers: vec![],
            subscriber_instance_manual: false,
            publisher_kind: None,
            include_sender: None,
            abi_routine_kind: Some(AbiRoutineKind::Procedure),
            abi_event_kind: Some(AbiEventKind::None),
            param_sig_key: String::new(),
            return_type: Some("Codeunit \"Dep Http Content\"".into()),
            return_type_id: Some(("Dep Http Content".into(), 60101)),
            abi_overload_collapsed: collapsed,
            source_overload_aliased: false,
            abi_params: AbiParams::Missing,
        }];

        let mut topology = DependencyGraph::default();
        topology.add_dependency(ws_ref, dep_ref);

        let obj_index = ObjectIndex::build(&objects);
        let graph = ProgramGraph {
            apps,
            topology,
            objects,
            routines,
            obj_index,
            ..Default::default()
        };
        let index = ResolveIndex::build(&graph);
        let surface = DeclSurface::build(&graph, &[]);
        (graph, index, surface, caller_obj_id)
    }

    #[test]
    fn plain_dispatch_declines_on_collapse_marked_candidate() {
        use crate::program::resolve::receiver::ReceiverType;

        let (graph, index, surface, caller_obj_id) = plain_dispatch_marker_guard_fixture(true);
        let from_obj = graph
            .objects
            .iter()
            .find(|o| o.id == caller_obj_id)
            .expect("Caller must exist");

        let receiver = ReceiverType::Object {
            kind: ObjectKind::Codeunit,
            name_lc: "dep collapse".into(),
            id: None,
        };
        let (_shape, routes) =
            resolve_member(&receiver, "get", 1, from_obj, &graph, &index, &surface);

        assert_eq!(routes.len(), 1);
        let r = &routes[0];
        assert_eq!(
            r.target,
            RouteTarget::Unresolved,
            "a collapse-MARKED candidate must never resolve confidently via \
             PLAIN dispatch either — only the chain-type-query boundary was \
             guarded before this fix; got {r:?}"
        );
        assert!(
            matches!(
                r.evidence,
                Evidence::Unknown(UnknownReason::AbiCollapsedOverload)
            ),
            "expected Unknown(AbiCollapsedOverload) (reason-split Task 2 — was \
             OverloadAmbiguous pre-split); got {r:?}"
        );
        assert_eq!(r.witness, Witness::None);
        assert_eq!(
            r.receiver_tier, None,
            "AbiCollapsedOverload is not a MemberNotFound shape — no receiver_tier"
        );
    }

    /// Control: the IDENTICAL fixture shape, but UNMARKED — proves the new
    /// guard does not over-decline a genuinely trustworthy sole ABI
    /// candidate (the CDO-neutrality-critical property: 0 marked routines
    /// on CDO means this guard is dormant there by construction).
    #[test]
    fn plain_dispatch_resolves_unmarked_candidate_normally() {
        use crate::program::resolve::receiver::ReceiverType;

        let (graph, index, surface, caller_obj_id) = plain_dispatch_marker_guard_fixture(false);
        let from_obj = graph
            .objects
            .iter()
            .find(|o| o.id == caller_obj_id)
            .expect("Caller must exist");

        let receiver = ReceiverType::Object {
            kind: ObjectKind::Codeunit,
            name_lc: "dep collapse".into(),
            id: None,
        };
        let (_shape, routes) =
            resolve_member(&receiver, "get", 1, from_obj, &graph, &index, &surface);

        assert_eq!(routes.len(), 1);
        let r = &routes[0];
        assert_eq!(r.evidence, Evidence::Opaque);
        assert!(
            matches!(r.target, RouteTarget::AbiSymbol { .. }),
            "an UNMARKED sole ABI candidate must still resolve normally; got {r:?}"
        );
    }

    // -----------------------------------------------------------------------
    // Task 2 (roadmap-closure plan): the ABI param-type retention + SymbolOnly
    // dispatch lift, end-to-end through `resolve_member_with_args`.
    // -----------------------------------------------------------------------

    /// Two SymbolOnly overloads of "Get" at arity 1, differing only by
    /// `abi_params`: one `Complete` (a real Integer param), one `Missing`
    /// (simulating an ABI candidate whose metadata could not be retained).
    /// Real ingestion always pairs `Missing` with the `UNKNOWN_ARITY`
    /// sentinel (so it would never reach THIS same-arity set) — this fixture
    /// deliberately bypasses that pairing (hand-constructs both at the SAME
    /// `params_count`) to prove the structural guard holds independent of
    /// it, per the plan's "no unknown-metadata candidate is ever filtered
    /// out" rule.
    fn abi_missing_metadata_fixture() -> (ProgramGraph, ResolveIndex, DeclSurface, ObjectNodeId) {
        let ws_id = make_app_id("WS");
        let dep_id = make_app_id("DepApp");

        let mut apps = AppRegistry::default();
        let ws_ref = apps.intern(&ws_id);
        let dep_ref = apps.intern(&dep_id);

        let caller_obj_id = ObjectNodeId {
            app: ws_ref,
            kind: ObjectKind::Codeunit,
            key: ObjKey::Id(50610),
        };
        let dep_obj_id = ObjectNodeId {
            app: dep_ref,
            kind: ObjectKind::Codeunit,
            key: ObjKey::Id(60110),
        };

        let objects = vec![
            ObjectNode {
                id: caller_obj_id.clone(),
                name: "Caller".into(),
                declared_id: Some(50610),
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
            },
            ObjectNode {
                id: dep_obj_id.clone(),
                name: "Dep Overload".into(),
                declared_id: Some(60110),
                extends_target: None,
                implements: vec![],
                tier: TrustTier::SymbolOnly,
                source_table: None,
                table_no: None,
                source_table_temporary: false,
                page_controls: vec![],
                fields: vec![],
                dataitems: vec![],
                parse_incomplete: false,
            },
        ];

        fn abi_node(dep_obj_id: &ObjectNodeId, sig_fp: u64, abi_params: AbiParams) -> RoutineNode {
            RoutineNode {
                id: RoutineNodeId {
                    object: dep_obj_id.clone(),
                    name_lc: "get".into(),
                    enclosing_member_lc: None,
                    params_count: 1,
                    sig_fp,
                },
                name: "Get".into(),
                is_trigger: false,
                access: Access::Public,
                tier: TrustTier::SymbolOnly,
                event_subscribers: vec![],
                subscriber_instance_manual: false,
                publisher_kind: None,
                include_sender: None,
                abi_routine_kind: Some(AbiRoutineKind::Procedure),
                abi_event_kind: Some(AbiEventKind::None),
                param_sig_key: String::new(),
                return_type: None,
                return_type_id: None,
                abi_overload_collapsed: false,
                source_overload_aliased: false,
                abi_params,
            }
        }

        let routines = vec![
            abi_node(
                &dep_obj_id,
                111,
                AbiParams::Complete(vec![AbiParamRetained {
                    type_text: "Integer".into(),
                    is_var: false,
                    subtype_id: None,
                    subtype_raw_name: None,
                    subtype_tag: SubtypeTag::NoSubtype,
                }]),
            ),
            abi_node(&dep_obj_id, 222, AbiParams::Missing),
        ];

        let mut topology = DependencyGraph::default();
        topology.add_dependency(ws_ref, dep_ref);

        let obj_index = ObjectIndex::build(&objects);
        let graph = ProgramGraph {
            apps,
            topology,
            objects,
            routines,
            obj_index,
            ..Default::default()
        };
        let index = ResolveIndex::build(&graph);
        let surface = DeclSurface::build(&graph, &[]);
        (graph, index, surface, caller_obj_id)
    }

    /// Fixture (d): a `Missing`-metadata ABI candidate in the visible set
    /// degrades the WHOLE call — never a false-confident `Exact` pick to the
    /// `Complete` sibling, even though that sibling alone would otherwise
    /// exactly match the typed arg.
    #[test]
    fn resolve_member_degrades_when_one_abi_candidate_has_missing_metadata() {
        use crate::program::resolve::receiver::ReceiverType;

        let (graph, index, surface, caller_obj_id) = abi_missing_metadata_fixture();
        let from_obj = graph
            .objects
            .iter()
            .find(|o| o.id == caller_obj_id)
            .expect("Caller must exist");

        let receiver = ReceiverType::Object {
            kind: ObjectKind::Codeunit,
            name_lc: "dep overload".into(),
            id: None,
        };
        let args = [ArgDispatchInfo {
            canonical: Some(CanonicalArgType::Base("integer".into())),
            exact_text: Some("integer".into()),
            literal_kind: Some(LiteralKind::Integer),
            var_passable: false,
        }];
        let (shape, routes) = resolve_member_with_args(
            &receiver, "get", 1, from_obj, &graph, &index, &surface, &args,
        );

        assert_eq!(
            shape,
            DispatchShape::AmbiguousOverload,
            "a Missing-metadata candidate must NEVER be filtered out of the \
             competition to let the Complete sibling resolve — its mere \
             presence degrades the whole call; got shape {shape:?}, routes {routes:?}"
        );
        assert_eq!(routes.len(), 2);
        assert!(
            routes
                .iter()
                .all(|r| r.conditions.contains(&Condition::AmbiguousDispatch)),
            "both routes must carry AmbiguousDispatch; got {routes:?}"
        );
    }

    /// Fixtures (e)/(f), at the `candidate_param_infos_either` mechanism
    /// level (the generic per-candidate helper `resolve_in_object`'s gate
    /// calls): a real SOURCE candidate (a genuinely parsed `RoutineDecl`, a
    /// `DeclSurface` hit) mixed with an ABI candidate — proven directly against
    /// the helper rather than through `resolve_member`/`resolve_in_object`
    /// end-to-end, because ONE `ObjectNode` cannot legitimately carry two
    /// DIFFERENT tiers at once (an object is wholly SOURCE-parsed or wholly
    /// ABI-ingested by construction — see `TrustTier`'s doc); this
    /// nonetheless exercises the REAL contract `candidate_param_infos_either`
    /// promises: "no DeclSurface entry" (never `rid.object`'s tier) decides which
    /// route serves a given `rid`, so a SOURCE `rid` (found in `DeclSurface`) and
    /// an ABI `rid` (not in `DeclSurface`, but carrying `abi_params`) can
    /// coexist in one candidate list exactly as `resolve_in_object`'s loop
    /// assembles one.
    #[test]
    fn candidate_param_infos_either_mixed_source_and_complete_abi_pick_correctly() {
        let src: &'static str = r#"
codeunit 50611 "MixedCU"
{
    procedure GetValue(X: Text)
    begin
    end;
}
"#;
        let app_id = make_app_id("MixedApp");
        let unit = make_unit(app_id, "MixedCU.al", src);
        let units = [unit];
        let mut graph = build_graph(&units, None);
        let source_obj_id = find_obj(&graph, "MixedCU").id.clone();
        let source_rid = graph
            .routines
            .iter()
            .find(|r| r.id.object == source_obj_id && r.id.name_lc == "getvalue")
            .expect("source GetValue(Text) must be extracted")
            .id
            .clone();

        // Inject a SECOND same-name/same-arity overload on the SAME object,
        // carrying ABI metadata instead of a DeclSurface entry — see the fixture
        // doc for why this is legitimately synthetic.
        let abi_rid = RoutineNodeId {
            object: source_obj_id.clone(),
            name_lc: "getvalue".into(),
            enclosing_member_lc: None,
            params_count: 1,
            sig_fp: 999,
        };
        graph.routines.push(RoutineNode {
            id: abi_rid.clone(),
            name: "GetValue".into(),
            is_trigger: false,
            access: Access::Public,
            tier: TrustTier::SymbolOnly,
            event_subscribers: vec![],
            subscriber_instance_manual: false,
            publisher_kind: None,
            include_sender: None,
            abi_routine_kind: Some(AbiRoutineKind::Procedure),
            abi_event_kind: Some(AbiEventKind::None),
            param_sig_key: String::new(),
            return_type: None,
            return_type_id: None,
            abi_overload_collapsed: false,
            source_overload_aliased: false,
            abi_params: AbiParams::Complete(vec![AbiParamRetained {
                type_text: "Integer".into(),
                is_var: false,
                subtype_id: None,
                subtype_raw_name: None,
                subtype_tag: SubtypeTag::NoSubtype,
            }]),
        });
        graph.routines.sort_by(|a, b| a.id.cmp(&b.id));

        let index = ResolveIndex::build(&graph);
        let surface = DeclSurface::build(&graph, &units);

        let source_params = candidate_param_infos_either(&source_rid, &graph, &index, &surface)
            .expect("the source candidate must read via DeclSurface");
        let abi_params = candidate_param_infos_either(&abi_rid, &graph, &index, &surface)
            .expect("the ABI candidate must read via the AbiParams::Complete fallback");

        let args = [ArgDispatchInfo {
            canonical: Some(CanonicalArgType::Base("integer".into())),
            exact_text: Some("integer".into()),
            literal_kind: Some(LiteralKind::Integer),
            var_passable: false,
        }];
        let candidates = vec![source_params, abi_params];
        assert_eq!(
            pick_candidate(&args, &candidates),
            Some(1),
            "an Integer arg must pick the ABI (Integer) candidate over the source (Text) one"
        );
    }

    /// Fixture (f): the same mixed set, but the ABI sibling's metadata is
    /// `Missing` — `candidate_param_infos_either` declines for THAT
    /// candidate alone, which is exactly what makes `resolve_in_object`'s
    /// `all_known` flip false and degrade the WHOLE call (the no-filtering
    /// rule) rather than silently proceed on the source candidate alone.
    #[test]
    fn candidate_param_infos_either_mixed_source_and_incomplete_abi_declines_for_abi_side() {
        let src: &'static str = r#"
codeunit 50612 "MixedCU2"
{
    procedure GetValue(X: Text)
    begin
    end;
}
"#;
        let app_id = make_app_id("MixedApp2");
        let unit = make_unit(app_id, "MixedCU2.al", src);
        let units = [unit];
        let mut graph = build_graph(&units, None);
        let source_obj_id = find_obj(&graph, "MixedCU2").id.clone();
        let source_rid = graph
            .routines
            .iter()
            .find(|r| r.id.object == source_obj_id && r.id.name_lc == "getvalue")
            .expect("source GetValue(Text) must be extracted")
            .id
            .clone();

        let abi_rid = RoutineNodeId {
            object: source_obj_id.clone(),
            name_lc: "getvalue".into(),
            enclosing_member_lc: None,
            params_count: 1,
            sig_fp: 999,
        };
        graph.routines.push(RoutineNode {
            id: abi_rid.clone(),
            name: "GetValue".into(),
            is_trigger: false,
            access: Access::Public,
            tier: TrustTier::SymbolOnly,
            event_subscribers: vec![],
            subscriber_instance_manual: false,
            publisher_kind: None,
            include_sender: None,
            abi_routine_kind: Some(AbiRoutineKind::Procedure),
            abi_event_kind: Some(AbiEventKind::None),
            param_sig_key: String::new(),
            return_type: None,
            return_type_id: None,
            abi_overload_collapsed: false,
            source_overload_aliased: false,
            abi_params: AbiParams::Missing,
        });
        graph.routines.sort_by(|a, b| a.id.cmp(&b.id));

        let index = ResolveIndex::build(&graph);
        let surface = DeclSurface::build(&graph, &units);

        assert!(
            candidate_param_infos_either(&source_rid, &graph, &index, &surface).is_some(),
            "the source candidate alone is perfectly readable via DeclSurface"
        );
        assert!(
            candidate_param_infos_either(&abi_rid, &graph, &index, &surface).is_none(),
            "the no-filtering rule: a Missing ABI sibling must decline on its \
             own terms, never be quietly dropped so the source candidate \
             resolves alone"
        );
    }

    // -----------------------------------------------------------------------
    // Task 2 review fix: collapse-marker guard at every `make_routine_route`
    // call site. `resolve_in_object`'s PLAIN-DISPATCH MARKER GUARD above (and
    // `routine_node_for_type_query`'s CHAIN-type-query guard) were the only
    // two `abi_overload_collapsed` consultation points before this fix — but
    // FOUR other call sites look up a routine directly by ROLE (entry
    // trigger / trigger fan-out / event subscriber) rather than through
    // either of those name+arity selection boundaries, so a collapse-marked
    // survivor could still reach a confident `Opaque`/`Source` route through
    // any of them:
    //   1. `resolve_object_run` (Codeunit.Run/Page.RunModal/Report.Run's
    //      entry-trigger lookup).
    //   2. `resolve_member`'s own inline `Codeunit.Run(arity<=1)` special
    //      case (the member-call-shaped mirror of (1)).
    //   3. `resolve_implicit_trigger`'s base-table + TableExtension trigger
    //      fan-out (data-is-control-flow Multicast routes).
    //   4. `emit_event_flow_edges`'s subscriber fan-out.
    // Each pair below builds the identical marked-vs-unmarked minimal-graph
    // shape `plain_dispatch_marker_guard_fixture` established (the marker
    // set directly, bypassing ingestion/dedup — isolates the SELECTION
    // guard) and proves: (f) a MARKED candidate declines at THIS specific
    // site; (control) an UNMARKED candidate in the identical shape still
    // resolves normally — the guard must neither miss a site nor
    // over-decline an honest one.
    // -----------------------------------------------------------------------

    /// Shared fixture for the two entry-trigger bypass sites
    /// (`resolve_object_run`; `resolve_member`'s inline `Codeunit.Run
    /// (arity<=1)` arm): a SymbolOnly dep Codeunit whose SOLE `onrun` entry
    /// trigger candidate (0-arg — `sig_fp` folds to the fixed `0` for an
    /// empty `Parameters[]`, see `abi_ingest::param_type_fp`) carries
    /// `abi_overload_collapsed` directly, simulating dedup's post-collapse
    /// output for a literal duplicate raw `OnRun` JSON entry.
    fn entry_trigger_marker_guard_fixture(
        collapsed: bool,
    ) -> (
        ProgramGraph,
        ResolveIndex,
        DeclSurface,
        ObjectNodeId,
        ObjectNodeId,
    ) {
        let ws_id = make_app_id("WS");
        let dep_id = make_app_id("DepApp");

        let mut apps = AppRegistry::default();
        let ws_ref = apps.intern(&ws_id);
        let dep_ref = apps.intern(&dep_id);

        let caller_obj_id = ObjectNodeId {
            app: ws_ref,
            kind: ObjectKind::Codeunit,
            key: ObjKey::Id(50610),
        };
        let dep_obj_id = ObjectNodeId {
            app: dep_ref,
            kind: ObjectKind::Codeunit,
            key: ObjKey::Id(60150),
        };

        let objects = vec![
            ObjectNode {
                id: caller_obj_id.clone(),
                name: "Caller".into(),
                declared_id: Some(50610),
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
            },
            ObjectNode {
                id: dep_obj_id.clone(),
                name: "Dep Trigger Collapse".into(),
                declared_id: Some(60150),
                extends_target: None,
                implements: vec![],
                tier: TrustTier::SymbolOnly,
                source_table: None,
                table_no: None,
                source_table_temporary: false,
                page_controls: vec![],
                fields: vec![],
                dataitems: vec![],
                parse_incomplete: false,
            },
        ];

        let routines = vec![RoutineNode {
            id: RoutineNodeId {
                object: dep_obj_id.clone(),
                name_lc: "onrun".into(),
                enclosing_member_lc: None,
                params_count: 0,
                sig_fp: 0,
            },
            name: "OnRun".into(),
            is_trigger: false,
            access: Access::Public,
            tier: TrustTier::SymbolOnly,
            event_subscribers: vec![],
            subscriber_instance_manual: false,
            publisher_kind: None,
            include_sender: None,
            abi_routine_kind: Some(AbiRoutineKind::Procedure),
            abi_event_kind: Some(AbiEventKind::None),
            param_sig_key: String::new(),
            return_type: None,
            return_type_id: None,
            abi_overload_collapsed: collapsed,
            source_overload_aliased: false,
            abi_params: AbiParams::Missing,
        }];

        let mut topology = DependencyGraph::default();
        topology.add_dependency(ws_ref, dep_ref);

        let obj_index = ObjectIndex::build(&objects);
        let graph = ProgramGraph {
            apps,
            topology,
            objects,
            routines,
            obj_index,
            ..Default::default()
        };
        let index = ResolveIndex::build(&graph);
        let surface = DeclSurface::build(&graph, &[]);
        (graph, index, surface, caller_obj_id, dep_obj_id)
    }

    // --- Site 1: `resolve_object_run` ---------------------------------------

    #[test]
    fn object_run_declines_on_collapse_marked_entry_trigger() {
        let (graph, index, surface, _caller_obj_id, _dep_obj_id) =
            entry_trigger_marker_guard_fixture(true);
        let from = graph.apps.find_by_name("WS").expect("WS app");

        let (shape, completeness, routes) = resolve_object_run(
            from,
            ObjectKind::Codeunit,
            Some("Dep Trigger Collapse"),
            true,
            &graph,
            &index,
            &surface,
        );

        assert_eq!(shape, DispatchShape::Exact);
        assert_eq!(completeness, SetCompleteness::Complete);
        assert_eq!(routes.len(), 1);
        let r = &routes[0];
        assert_eq!(
            r.target,
            RouteTarget::Unresolved,
            "a collapse-MARKED entry trigger must never resolve confidently via \
             Codeunit.Run's resolve_object_run path either — this site bypasses \
             resolve_in_object entirely (Task 2 review fix); got {r:?}"
        );
        assert!(
            matches!(
                r.evidence,
                Evidence::Unknown(UnknownReason::OverloadAmbiguous)
            ),
            "expected Unknown(OverloadAmbiguous); got {r:?}"
        );
    }

    #[test]
    fn object_run_resolves_unmarked_entry_trigger_normally() {
        let (graph, index, surface, _caller_obj_id, _dep_obj_id) =
            entry_trigger_marker_guard_fixture(false);
        let from = graph.apps.find_by_name("WS").expect("WS app");

        let (_shape, _completeness, routes) = resolve_object_run(
            from,
            ObjectKind::Codeunit,
            Some("Dep Trigger Collapse"),
            true,
            &graph,
            &index,
            &surface,
        );

        assert_eq!(routes.len(), 1);
        let r = &routes[0];
        assert_eq!(r.evidence, Evidence::Opaque);
        assert!(
            matches!(r.target, RouteTarget::AbiSymbol { .. }),
            "an UNMARKED sole entry trigger must still resolve normally; got {r:?}"
        );
    }

    // --- Site 2: `resolve_member`'s inline Codeunit.Run(arity<=1) arm ------

    #[test]
    fn resolve_member_object_run_arm_declines_on_collapse_marked_entry_trigger() {
        use crate::program::resolve::receiver::ReceiverType;

        let (graph, index, surface, caller_obj_id, _dep_obj_id) =
            entry_trigger_marker_guard_fixture(true);
        let from_obj = graph
            .objects
            .iter()
            .find(|o| o.id == caller_obj_id)
            .expect("Caller must exist");

        let receiver = ReceiverType::Object {
            kind: ObjectKind::Codeunit,
            name_lc: "dep trigger collapse".into(),
            id: None,
        };
        let (shape, routes) =
            resolve_member(&receiver, "run", 0, from_obj, &graph, &index, &surface);

        assert_eq!(shape, DispatchShape::Exact);
        assert_eq!(routes.len(), 1);
        let r = &routes[0];
        assert_eq!(
            r.target,
            RouteTarget::Unresolved,
            "a collapse-MARKED entry trigger must never resolve confidently via \
             resolve_member's inline Codeunit.Run(arity<=1) arm either — this \
             site bypasses resolve_in_object entirely (Task 2 review fix); \
             got {r:?}"
        );
        assert!(
            matches!(
                r.evidence,
                Evidence::Unknown(UnknownReason::OverloadAmbiguous)
            ),
            "expected Unknown(OverloadAmbiguous); got {r:?}"
        );
    }

    #[test]
    fn resolve_member_object_run_arm_resolves_unmarked_entry_trigger_normally() {
        use crate::program::resolve::receiver::ReceiverType;

        let (graph, index, surface, caller_obj_id, _dep_obj_id) =
            entry_trigger_marker_guard_fixture(false);
        let from_obj = graph
            .objects
            .iter()
            .find(|o| o.id == caller_obj_id)
            .expect("Caller must exist");

        let receiver = ReceiverType::Object {
            kind: ObjectKind::Codeunit,
            name_lc: "dep trigger collapse".into(),
            id: None,
        };
        let (_shape, routes) =
            resolve_member(&receiver, "run", 0, from_obj, &graph, &index, &surface);

        assert_eq!(routes.len(), 1);
        let r = &routes[0];
        assert_eq!(r.evidence, Evidence::Opaque);
        assert!(
            matches!(r.target, RouteTarget::AbiSymbol { .. }),
            "an UNMARKED sole entry trigger must still resolve normally; got {r:?}"
        );
    }

    // --- Site 3: `resolve_implicit_trigger` ---------------------------------

    /// Fixture for the trigger-fan-out bypass site (`resolve_implicit_
    /// trigger`): a SymbolOnly dep Table whose SOLE `oninsert` object-level
    /// trigger candidate carries `abi_overload_collapsed` directly.
    fn implicit_trigger_marker_guard_fixture(
        collapsed: bool,
    ) -> (ProgramGraph, ResolveIndex, DeclSurface, ObjectNodeId) {
        let dep_id = make_app_id("DepApp");
        let mut apps = AppRegistry::default();
        let dep_ref = apps.intern(&dep_id);

        let table_obj_id = ObjectNodeId {
            app: dep_ref,
            kind: ObjectKind::Table,
            key: ObjKey::Id(60160),
        };

        let objects = vec![ObjectNode {
            id: table_obj_id.clone(),
            name: "Dep Trigger Table".into(),
            declared_id: Some(60160),
            extends_target: None,
            implements: vec![],
            tier: TrustTier::SymbolOnly,
            source_table: None,
            table_no: None,
            source_table_temporary: false,
            page_controls: vec![],
            fields: vec![],
            dataitems: vec![],
            parse_incomplete: false,
        }];

        let routines = vec![RoutineNode {
            id: RoutineNodeId {
                object: table_obj_id.clone(),
                name_lc: "oninsert".into(),
                enclosing_member_lc: None,
                params_count: 0,
                sig_fp: 0,
            },
            name: "OnInsert".into(),
            is_trigger: false,
            access: Access::Public,
            tier: TrustTier::SymbolOnly,
            event_subscribers: vec![],
            subscriber_instance_manual: false,
            publisher_kind: None,
            include_sender: None,
            abi_routine_kind: Some(AbiRoutineKind::Procedure),
            abi_event_kind: Some(AbiEventKind::None),
            param_sig_key: String::new(),
            return_type: None,
            return_type_id: None,
            abi_overload_collapsed: collapsed,
            source_overload_aliased: false,
            abi_params: AbiParams::Missing,
        }];

        let obj_index = ObjectIndex::build(&objects);
        let graph = ProgramGraph {
            apps,
            topology: DependencyGraph::default(),
            objects,
            routines,
            obj_index,
            ..Default::default()
        };
        let index = ResolveIndex::build(&graph);
        let surface = DeclSurface::build(&graph, &[]);
        (graph, index, surface, table_obj_id)
    }

    #[test]
    fn implicit_trigger_declines_route_for_collapse_marked_trigger() {
        let (graph, index, surface, table_obj_id) = implicit_trigger_marker_guard_fixture(true);
        let table_obj = graph
            .objects
            .iter()
            .find(|o| o.id == table_obj_id)
            .expect("table");

        let (shape, completeness, routes) =
            resolve_implicit_trigger("insert", table_obj, &graph, &index, &surface);

        assert_eq!(shape, DispatchShape::Multicast);
        assert_eq!(
            completeness,
            SetCompleteness::Partial {
                reason: OpenWorldReason::ReverseDependentExtensions
            }
        );
        assert_eq!(
            routes.len(),
            1,
            "a marked trigger must still contribute ONE honest Unknown route, \
             never silently vanish; got {routes:?}"
        );
        let r = &routes[0];
        assert_eq!(
            r.target,
            RouteTarget::Unresolved,
            "a collapse-MARKED table trigger must never resolve confidently via \
             resolve_implicit_trigger's fan-out either — this site bypasses \
             resolve_in_object entirely (Task 2 review fix); got {r:?}"
        );
        assert!(
            matches!(
                r.evidence,
                Evidence::Unknown(UnknownReason::OverloadAmbiguous)
            ),
            "expected Unknown(OverloadAmbiguous); got {r:?}"
        );
    }

    #[test]
    fn implicit_trigger_resolves_unmarked_trigger_normally() {
        let (graph, index, surface, table_obj_id) = implicit_trigger_marker_guard_fixture(false);
        let table_obj = graph
            .objects
            .iter()
            .find(|o| o.id == table_obj_id)
            .expect("table");

        let (_shape, _completeness, routes) =
            resolve_implicit_trigger("insert", table_obj, &graph, &index, &surface);

        assert_eq!(routes.len(), 1);
        let r = &routes[0];
        assert_eq!(r.evidence, Evidence::Opaque);
        assert!(
            matches!(r.target, RouteTarget::AbiSymbol { .. }),
            "an UNMARKED trigger must still resolve normally; got {r:?}"
        );
    }

    // --- Site 4: `emit_event_flow_edges` ------------------------------------

    /// Fixture for the event-subscriber-fan-out bypass site
    /// (`emit_event_flow_edges`): a SymbolOnly dep publisher/subscriber pair
    /// where the SOLE matching subscriber candidate carries `abi_overload_
    /// collapsed` directly.
    fn event_flow_marker_guard_fixture(
        collapsed: bool,
    ) -> (ProgramGraph, ResolveIndex, DeclSurface) {
        let dep_id = make_app_id("DepApp");
        let mut apps = AppRegistry::default();
        let dep_ref = apps.intern(&dep_id);

        let pub_obj_id = ObjectNodeId {
            app: dep_ref,
            kind: ObjectKind::Codeunit,
            key: ObjKey::Id(60170),
        };
        let sub_obj_id = ObjectNodeId {
            app: dep_ref,
            kind: ObjectKind::Codeunit,
            key: ObjKey::Id(60171),
        };

        let objects = vec![
            ObjectNode {
                id: pub_obj_id.clone(),
                name: "Dep Evt Pub".into(),
                declared_id: Some(60170),
                extends_target: None,
                implements: vec![],
                tier: TrustTier::SymbolOnly,
                source_table: None,
                table_no: None,
                source_table_temporary: false,
                page_controls: vec![],
                fields: vec![],
                dataitems: vec![],
                parse_incomplete: false,
            },
            ObjectNode {
                id: sub_obj_id.clone(),
                name: "Dep Evt Sub".into(),
                declared_id: Some(60171),
                extends_target: None,
                implements: vec![],
                tier: TrustTier::SymbolOnly,
                source_table: None,
                table_no: None,
                source_table_temporary: false,
                page_controls: vec![],
                fields: vec![],
                dataitems: vec![],
                parse_incomplete: false,
            },
        ];

        let publisher = RoutineNode {
            id: RoutineNodeId {
                object: pub_obj_id.clone(),
                name_lc: "onafterx".into(),
                enclosing_member_lc: None,
                params_count: 0,
                sig_fp: 0,
            },
            name: "OnAfterX".into(),
            is_trigger: false,
            access: Access::Public,
            tier: TrustTier::SymbolOnly,
            event_subscribers: vec![],
            subscriber_instance_manual: false,
            publisher_kind: Some(crate::program::resolve::event::PublisherKind::Integration),
            include_sender: Some(false),
            abi_routine_kind: Some(AbiRoutineKind::EventPublisher),
            abi_event_kind: Some(AbiEventKind::Integration),
            param_sig_key: String::new(),
            return_type: None,
            return_type_id: None,
            abi_overload_collapsed: false,
            source_overload_aliased: false,
            abi_params: AbiParams::Missing,
        };

        let subscriber = RoutineNode {
            id: RoutineNodeId {
                object: sub_obj_id.clone(),
                name_lc: "onafterxhandler".into(),
                enclosing_member_lc: None,
                params_count: 0,
                sig_fp: 0,
            },
            name: "OnAfterXHandler".into(),
            is_trigger: false,
            access: Access::Public,
            tier: TrustTier::SymbolOnly,
            event_subscribers: vec![crate::program::resolve::event::ParsedSubscriberArgs {
                publisher_object_type: "codeunit".into(),
                publisher_name: "dep evt pub".into(),
                event_name: "onafterx".into(),
                element: None,
                skip_on_missing_license: false,
                skip_on_missing_permission: false,
            }],
            subscriber_instance_manual: false,
            publisher_kind: None,
            include_sender: None,
            abi_routine_kind: Some(AbiRoutineKind::EventSubscriber),
            abi_event_kind: Some(AbiEventKind::None),
            param_sig_key: String::new(),
            return_type: None,
            return_type_id: None,
            abi_overload_collapsed: collapsed,
            source_overload_aliased: false,
            abi_params: AbiParams::Missing,
        };

        let mut routines = vec![publisher, subscriber];
        routines.sort_by(|a, b| a.id.cmp(&b.id));

        let obj_index = ObjectIndex::build(&objects);
        let graph = ProgramGraph {
            apps,
            topology: DependencyGraph::default(),
            objects,
            routines,
            obj_index,
            ..Default::default()
        };
        let index = ResolveIndex::build(&graph);
        let surface = DeclSurface::build(&graph, &[]);
        (graph, index, surface)
    }

    #[test]
    fn event_flow_skips_route_for_collapse_marked_subscriber() {
        let (graph, index, surface) = event_flow_marker_guard_fixture(true);
        let edges = emit_event_flow_edges(&graph, &index, &surface);
        let event_edges: Vec<&Edge> = edges
            .iter()
            .filter(|e| e.kind == EdgeKind::EventFlow)
            .collect();
        assert_eq!(
            event_edges.len(),
            1,
            "publisher must always produce an EventFlow edge"
        );
        let e = event_edges[0];
        assert!(
            e.routes.is_empty(),
            "a collapse-MARKED subscriber must never resolve confidently via \
             emit_event_flow_edges's subscriber fan-out either — this site \
             bypasses resolve_in_object entirely (Task 2 review fix): the \
             marked subscriber's route must be SKIPPED, not silently emitted \
             Opaque; got {:?}",
            e.routes
        );
    }

    #[test]
    fn event_flow_includes_route_for_unmarked_subscriber_normally() {
        let (graph, index, surface) = event_flow_marker_guard_fixture(false);
        let edges = emit_event_flow_edges(&graph, &index, &surface);
        let event_edges: Vec<&Edge> = edges
            .iter()
            .filter(|e| e.kind == EdgeKind::EventFlow)
            .collect();
        assert_eq!(event_edges.len(), 1);
        let e = event_edges[0];
        assert_eq!(
            e.routes.len(),
            1,
            "an UNMARKED subscriber must still be included normally"
        );
        let r = &e.routes[0];
        assert_eq!(r.evidence, Evidence::Opaque);
        assert!(matches!(r.target, RouteTarget::AbiSymbol { .. }));
    }

    // -----------------------------------------------------------------------
    // Witness↔evidence contract test
    // -----------------------------------------------------------------------

    #[test]
    fn witness_evidence_contract_holds_for_all_route_variants() {
        let src: &'static str = r#"
codeunit 50200 "ContractCU"
{
    procedure MyProc()
    begin
    end;
}
"#;
        let app_id = make_app_id("TestApp");
        let unit = make_unit(app_id, "ContractCU.al", src);
        let units = [unit];
        let graph = build_graph(&units, None);
        let index = ResolveIndex::build(&graph);
        let surface = DeclSurface::build(&graph, &units);
        let from_obj = find_obj(&graph, "ContractCU");

        // Source route: resolve to own procedure.
        let (_shape, src_routes) = resolve_bare(
            from_obj,
            "myproc",
            0,
            &graph,
            &index,
            &surface,
            WithState::NoWithProven,
        );
        assert_eq!(src_routes.len(), 1);
        let src_route = &src_routes[0];
        assert_eq!(src_route.evidence, Evidence::Source, "Source evidence");
        assert!(
            matches!(src_route.witness, Witness::SourceSpan { .. }),
            "Source evidence must pair with SourceSpan witness"
        );

        // Catalog route: global builtin.
        let (_shape, cat_routes) = resolve_bare(
            from_obj,
            "error",
            1,
            &graph,
            &index,
            &surface,
            WithState::NoWithProven,
        );
        assert_eq!(cat_routes.len(), 1);
        let cat_route = &cat_routes[0];
        assert_eq!(cat_route.evidence, Evidence::Catalog, "Catalog evidence");
        assert!(
            matches!(cat_route.witness, Witness::CatalogEntry { .. }),
            "Catalog evidence must pair with CatalogEntry witness"
        );

        // Unknown route: no match anywhere.
        let (_shape, unk_routes) = resolve_bare(
            from_obj,
            "zz_absolutely_no_match_xyz",
            0,
            &graph,
            &index,
            &surface,
            WithState::NoWithProven,
        );
        assert_eq!(unk_routes.len(), 1);
        let unk_route = &unk_routes[0];
        assert!(
            matches!(unk_route.evidence, Evidence::Unknown(_)),
            "Unknown evidence"
        );
        assert_eq!(
            unk_route.witness,
            Witness::None,
            "Unknown evidence must pair with None witness"
        );
    }

    // -----------------------------------------------------------------------
    // (e) name-found-but-no-arity-match → Unknown, NOT a wrong-arity Source
    // -----------------------------------------------------------------------

    #[test]
    fn bare_name_match_arity_mismatch_emits_unknown_route() {
        // DoFoo takes 0 params.  Calling with arity 2 → name found, no overload
        // matches → Unknown route (not a false-confident Source to DoFoo).
        let src: &'static str = r#"
codeunit 50103 "ArityMismatchCU"
{
    procedure DoFoo()
    begin
    end;
}
"#;
        let app_id = make_app_id("TestApp");
        let unit = make_unit(app_id, "ArityMismatchCU.al", src);
        let units = [unit];
        let graph = build_graph(&units, None);
        let index = ResolveIndex::build(&graph);
        let surface = DeclSurface::build(&graph, &units);

        let from_obj = find_obj(&graph, "ArityMismatchCU");
        // "dofoo" exists with arity 0; we request arity 2 → no match.
        let (_shape, routes) = resolve_bare(
            from_obj,
            "dofoo",
            2,
            &graph,
            &index,
            &surface,
            WithState::NoWithProven,
        );

        assert_eq!(routes.len(), 1);
        let r = &routes[0];
        assert_eq!(
            r.target,
            RouteTarget::Unresolved,
            "arity-mismatch must yield Unresolved target, not a Source route to the wrong-arity proc"
        );
        assert!(
            matches!(r.evidence, Evidence::Unknown(_)),
            "arity-mismatch must yield Unknown evidence"
        );
        assert_eq!(
            r.witness,
            Witness::None,
            "arity-mismatch must yield None witness"
        );
    }

    // -----------------------------------------------------------------------
    // Task-5 helper: find the AppRef for the first (and only) app in graph
    // -----------------------------------------------------------------------

    fn sole_app_ref(graph: &ProgramGraph) -> AppRef {
        graph
            .apps
            .find_by_name("TestApp")
            .expect("TestApp must be registered")
    }

    // -----------------------------------------------------------------------
    // Task-5 (a): Codeunit.Run to a known codeunit → Exact, OnRun, Source
    // -----------------------------------------------------------------------

    #[test]
    fn object_run_codeunit_to_known_codeunit_resolves_to_onrun() {
        let src: &'static str = r#"
codeunit 50200 "TargetCU"
{
    trigger OnRun()
    begin
    end;
}
"#;
        let app_id = make_app_id("TestApp");
        let unit = make_unit(app_id, "TargetCU.al", src);
        let units = [unit];
        let graph = build_graph(&units, None);
        let index = ResolveIndex::build(&graph);
        let surface = DeclSurface::build(&graph, &units);

        let from = sole_app_ref(&graph);
        let (shape, completeness, routes) = resolve_object_run(
            from,
            ObjectKind::Codeunit,
            Some("TargetCU"),
            true,
            &graph,
            &index,
            &surface,
        );

        assert_eq!(
            shape,
            DispatchShape::Exact,
            "shape must be Exact for resolved target"
        );
        assert_eq!(
            completeness,
            SetCompleteness::Complete,
            "completeness must be Complete"
        );
        assert_eq!(routes.len(), 1, "expected exactly one route");
        let r = &routes[0];
        assert!(
            matches!(r.target, RouteTarget::Routine(_)),
            "target must be Routine(onrun), got {:?}",
            r.target
        );
        assert_eq!(r.evidence, Evidence::Source);
        assert!(
            matches!(r.witness, Witness::SourceSpan { .. }),
            "witness must be SourceSpan"
        );
        let RouteTarget::Routine(ref rid) = r.target else {
            unreachable!()
        };
        assert_eq!(rid.name_lc, "onrun", "must resolve to the onrun trigger");
    }

    // -----------------------------------------------------------------------
    // Task-5 (b): Page.RunModal to a page → Exact, OnOpenPage (NOT OnRun)
    // -----------------------------------------------------------------------

    #[test]
    fn object_run_page_resolves_to_onopenpage_not_onrun() {
        let src: &'static str = r#"
page 50300 "SomePage"
{
    trigger OnOpenPage()
    begin
    end;
}
"#;
        let app_id = make_app_id("TestApp");
        let unit = make_unit(app_id, "SomePage.al", src);
        let units = [unit];
        let graph = build_graph(&units, None);
        let index = ResolveIndex::build(&graph);
        let surface = DeclSurface::build(&graph, &units);

        let from = sole_app_ref(&graph);
        let (shape, completeness, routes) = resolve_object_run(
            from,
            ObjectKind::Page,
            Some("SomePage"),
            true,
            &graph,
            &index,
            &surface,
        );

        assert_eq!(shape, DispatchShape::Exact);
        assert_eq!(completeness, SetCompleteness::Complete);
        assert_eq!(routes.len(), 1);
        let RouteTarget::Routine(ref rid) = routes[0].target else {
            panic!("target must be Routine, got {:?}", routes[0].target)
        };
        assert_eq!(
            rid.name_lc, "onopenpage",
            "Page must resolve to onopenpage, NOT onrun"
        );
        assert_eq!(routes[0].evidence, Evidence::Source);
    }

    // -----------------------------------------------------------------------
    // Task-5 (c): No static target → DynamicOpen + Unknown blocker
    // -----------------------------------------------------------------------

    #[test]
    fn object_run_no_target_emits_dynamic_open_with_unknown_blocker() {
        let src: &'static str = r#"
codeunit 50201 "CallerCU"
{
    procedure Run()
    begin
    end;
}
"#;
        let app_id = make_app_id("TestApp");
        let unit = make_unit(app_id, "CallerCU.al", src);
        let units = [unit];
        let graph = build_graph(&units, None);
        let index = ResolveIndex::build(&graph);
        let surface = DeclSurface::build(&graph, &units);

        let from = sole_app_ref(&graph);
        let (shape, completeness, routes) = resolve_object_run(
            from,
            ObjectKind::Codeunit,
            None, // no static target
            true,
            &graph,
            &index,
            &surface,
        );

        assert_eq!(
            shape,
            DispatchShape::DynamicOpen,
            "no-target must be DynamicOpen"
        );
        assert_eq!(
            completeness,
            SetCompleteness::Partial {
                reason: OpenWorldReason::RuntimeTypeUnbounded
            },
        );
        // Must include a blocker route — not an empty list (open world, not false resolved).
        assert!(!routes.is_empty(), "must include a blocker route");
        let r = &routes[0];
        assert_eq!(r.target, RouteTarget::Unresolved);
        assert!(matches!(r.evidence, Evidence::Unknown(_)));
        assert_eq!(r.witness, Witness::None);
    }

    // -----------------------------------------------------------------------
    // Task-5 (d): Target named but not in graph → Unknown (honest failure)
    // Updated by Task-3: opaque-boundary arm replaced with Unresolved/Unknown
    // to avoid creating AbiSymbol keys with the wrong (caller) app ref.
    // -----------------------------------------------------------------------

    #[test]
    fn object_run_target_not_in_graph_emits_unknown() {
        let src: &'static str = r#"
codeunit 50202 "AnotherCaller"
{
    procedure Run()
    begin
    end;
}
"#;
        let app_id = make_app_id("TestApp");
        let unit = make_unit(app_id, "AnotherCaller.al", src);
        let units = [unit];
        let graph = build_graph(&units, None);
        let index = ResolveIndex::build(&graph);
        let surface = DeclSurface::build(&graph, &units);

        let from = sole_app_ref(&graph);
        let (shape, completeness, routes) = resolve_object_run(
            from,
            ObjectKind::Codeunit,
            Some("NonExistentCU"), // not in graph
            true,
            &graph,
            &index,
            &surface,
        );

        assert_eq!(
            shape,
            DispatchShape::Exact,
            "not-in-graph is still an exact dispatch"
        );
        assert_eq!(
            completeness,
            SetCompleteness::Complete,
            "not-in-graph target is a known boundary (Complete, not RuntimeTypeUnbounded)"
        );
        assert_eq!(routes.len(), 1);
        let r = &routes[0];
        assert_eq!(
            r.target,
            RouteTarget::Unresolved,
            "target not in any indexed app must yield Unresolved (not AbiSymbol); got {:?}",
            r.target
        );
        assert_eq!(
            r.evidence,
            Evidence::Unknown(UnknownReason::ObjectNotInGraph),
            "not-found target must use Unknown(ObjectNotInGraph) (reason-split \
             Task 2 — the RECEIVER OBJECT itself is absent; was MemberNotFound \
             pre-split); got {r:?}"
        );
        assert_eq!(r.witness, Witness::None);
        assert_eq!(
            r.receiver_tier, None,
            "ObjectNotInGraph has no resolved receiver to tag"
        );
    }

    // -----------------------------------------------------------------------
    // Task-5 (d-ii): Target in graph but entry trigger not found → AbiSymbol Opaque
    // -----------------------------------------------------------------------

    #[test]
    fn object_run_entry_trigger_not_found_emits_opaque() {
        let src: &'static str = r#"
codeunit 50203 "NoTriggerCU"
{
    // Note: no OnRun trigger defined
    procedure SomeProc()
    begin
    end;
}
"#;
        let app_id = make_app_id("TestApp");
        let unit = make_unit(app_id, "NoTriggerCU.al", src);
        let units = [unit];
        let graph = build_graph(&units, None);
        let index = ResolveIndex::build(&graph);
        let surface = DeclSurface::build(&graph, &units);

        let from = sole_app_ref(&graph);
        let (shape, completeness, routes) = resolve_object_run(
            from,
            ObjectKind::Codeunit,
            Some("NoTriggerCU"), // in graph but no OnRun trigger
            true,
            &graph,
            &index,
            &surface,
        );

        assert_eq!(shape, DispatchShape::Exact);
        assert_eq!(
            completeness,
            SetCompleteness::Complete,
            "object exists; trigger-not-found is a known boundary (Complete)"
        );
        assert_eq!(routes.len(), 1);
        let r = &routes[0];
        assert!(
            matches!(r.target, RouteTarget::AbiSymbol { .. }),
            "target must be AbiSymbol; got {:?}",
            r.target
        );
        assert_eq!(r.evidence, Evidence::Opaque);
        assert!(
            matches!(r.witness, Witness::AbiSymbol { .. }),
            "AbiSymbol target must pair with AbiSymbol witness"
        );
    }

    // -----------------------------------------------------------------------
    // Task-5 (e): insert implicit-trigger on table with OnInsert → Multicast
    // -----------------------------------------------------------------------

    #[test]
    fn implicit_trigger_insert_resolves_to_oninsert_multicast() {
        let src: &'static str = r#"
table 50400 "SomeTable"
{
    trigger OnInsert()
    begin
    end;
}
"#;
        let app_id = make_app_id("TestApp");
        let unit = make_unit(app_id, "SomeTable.al", src);
        let units = [unit];
        let graph = build_graph(&units, None);
        let index = ResolveIndex::build(&graph);
        let surface = DeclSurface::build(&graph, &units);

        let table_obj = find_obj(&graph, "SomeTable");
        let (shape, completeness, routes) =
            resolve_implicit_trigger("insert", table_obj, &graph, &index, &surface);

        assert_eq!(
            shape,
            DispatchShape::Multicast,
            "implicit trigger must be Multicast"
        );
        assert_eq!(
            completeness,
            SetCompleteness::Partial {
                reason: OpenWorldReason::ReverseDependentExtensions
            },
        );
        assert_eq!(routes.len(), 1, "one oninsert trigger from the base table");
        let r = &routes[0];
        let RouteTarget::Routine(ref rid) = r.target else {
            panic!("target must be Routine, got {:?}", r.target)
        };
        assert_eq!(rid.name_lc, "oninsert");
        assert_eq!(r.evidence, Evidence::Source);
        assert!(matches!(r.witness, Witness::SourceSpan { .. }));
    }

    // -----------------------------------------------------------------------
    // (f) DeclSurface-miss on a source-tier graph routine → Unknown, NOT silent Abi
    // -----------------------------------------------------------------------

    #[test]
    fn bare_decl_surface_miss_emits_unknown_route() {
        // Build graph with a codeunit containing MyProc, but supply an EMPTY
        // slice to DeclSurface::build so the DeclSurface has no entries.  MyProc is in
        // the graph (ResolveIndex sees it) but absent from the DeclSurface — a real
        // integration gap that must surface as Unknown, not silent Abi degradation.
        let src: &'static str = r#"
codeunit 50104 "BodyMissCU"
{
    procedure MyProc()
    begin
    end;
}
"#;
        let app_id = make_app_id("TestApp");
        let unit = make_unit(app_id, "BodyMissCU.al", src);
        let units = [unit];
        let graph = build_graph(&units, None);
        let index = ResolveIndex::build(&graph);
        // Empty parsed slice: every graph routine is absent from the DeclSurface.
        let surface = DeclSurface::build(&graph, &[]);

        let from_obj = find_obj(&graph, "BodyMissCU");
        let (_shape, routes) = resolve_bare(
            from_obj,
            "myproc",
            0,
            &graph,
            &index,
            &surface,
            WithState::NoWithProven,
        );

        assert_eq!(routes.len(), 1);
        let r = &routes[0];
        assert_eq!(
            r.target,
            RouteTarget::Unresolved,
            "DeclSurface-miss must yield Unresolved (not Routine+Abi)"
        );
        assert!(
            matches!(r.evidence, Evidence::Unknown(_)),
            "DeclSurface-miss must yield Unknown evidence"
        );
        assert_eq!(
            r.witness,
            Witness::None,
            "DeclSurface-miss must yield None witness"
        );
    }

    // -----------------------------------------------------------------------
    // Task 0 (Phase 3): overloads with distinct params_count → distinct
    // RoutineNodeIds, resolved by arity
    // -----------------------------------------------------------------------

    #[test]
    fn overloads_distinct_by_arity_and_resolved_by_arity() {
        // A codeunit with Post() (0 params) and Post(x: Integer) (1 param).
        // After the arity discriminant, both must produce DISTINCT RoutineNodeIds
        // and resolve_bare must pick the arity-matched overload.
        let src: &'static str = r#"
codeunit 50300 "OverloadCU"
{
    procedure Post()
    begin
    end;
    procedure Post(x: Integer)
    begin
    end;
}
"#;
        let app_id = make_app_id("TestApp");
        let unit = make_unit(app_id, "OverloadCU.al", src);
        let units = [unit];
        let graph = build_graph(&units, None);
        let index = ResolveIndex::build(&graph);
        let surface = DeclSurface::build(&graph, &units);

        // Collect the RoutineNodeIds for "post" from the graph.
        let post_rids: Vec<_> = graph
            .routines
            .iter()
            .filter(|r| r.id.name_lc == "post")
            .map(|r| r.id.clone())
            .collect();
        assert_eq!(post_rids.len(), 2, "IR must expose two Post overloads");

        // Their params_counts must be 0 and 1.
        let mut counts: Vec<usize> = post_rids.iter().map(|r| r.params_count).collect();
        counts.sort();
        assert_eq!(
            counts,
            vec![0, 1],
            "Post() and Post(x: Integer) must have params_count 0 and 1"
        );

        // The two RoutineNodeIds must be DISTINCT (params_count is part of the key).
        assert_ne!(
            post_rids[0], post_rids[1],
            "overloads must have distinct RoutineNodeIds after adding params_count"
        );

        let from_obj = find_obj(&graph, "OverloadCU");

        // Resolve with arity=1 → must get the Post(x: Integer) overload (params_count=1).
        let (_shape1, routes1) = resolve_bare(
            from_obj,
            "post",
            1,
            &graph,
            &index,
            &surface,
            WithState::NoWithProven,
        );
        assert_eq!(routes1.len(), 1);
        let r1 = &routes1[0];
        assert!(
            matches!(r1.target, RouteTarget::Routine(_)),
            "arity=1 call must resolve to a Routine, not Unknown; got {:?}",
            r1.target
        );
        let RouteTarget::Routine(ref rid1) = r1.target else {
            unreachable!()
        };
        assert_eq!(
            rid1.params_count, 1,
            "arity=1 call must resolve to the 1-param overload"
        );

        // Resolve with arity=0 → must get the Post() overload (params_count=0).
        let (_shape0, routes0) = resolve_bare(
            from_obj,
            "post",
            0,
            &graph,
            &index,
            &surface,
            WithState::NoWithProven,
        );
        assert_eq!(routes0.len(), 1);
        let r0 = &routes0[0];
        assert!(
            matches!(r0.target, RouteTarget::Routine(_)),
            "arity=0 call must resolve to a Routine; got {:?}",
            r0.target
        );
        let RouteTarget::Routine(ref rid0) = r0.target else {
            unreachable!()
        };
        assert_eq!(
            rid0.params_count, 0,
            "arity=0 call must resolve to the 0-param overload"
        );
    }

    // -----------------------------------------------------------------------
    // Phase 3 Task 2 — resolve_member tests
    // -----------------------------------------------------------------------

    /// Build a minimal test graph, index and surface for resolve_member tests.
    fn minimal_resolve_member_fixtures() -> (
        ProgramGraph,
        ResolveIndex,
        crate::program::resolve::decl_surface::DeclSurface,
        crate::program::node_extract::ObjectNode,
    ) {
        use crate::program::node::{AppRegistry, ObjKey, ObjectNodeId};
        use crate::program::node_extract::ObjectNode;
        use crate::snapshot::{AppId, TrustTier};
        use al_syntax::ir::ObjectKind;

        let mut apps = AppRegistry::default();
        let app_id = AppId {
            guid: String::new(),
            name: "T".into(),
            publisher: "T".into(),
            version: "1.0.0.0".into(),
        };
        let app = apps.intern(&app_id);
        let graph = ProgramGraph {
            apps,
            topology: crate::program::topology::DependencyGraph::default(),
            objects: vec![],
            routines: vec![],
            obj_index: crate::program::graph::ObjectIndex::build(&[]),
            ..Default::default()
        };
        let index = ResolveIndex::build(&graph);
        let surface = crate::program::resolve::decl_surface::DeclSurface::build(&graph, &[]);
        let from_obj = ObjectNode {
            id: ObjectNodeId {
                app,
                kind: ObjectKind::Codeunit,
                key: ObjKey::Id(1),
            },
            name: "T".into(),
            declared_id: Some(1),
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
        (graph, index, surface, from_obj)
    }

    #[test]
    fn resolve_member_framework_json_object_catalog_route() {
        use crate::program::resolve::receiver::{FrameworkKind, ReceiverType};
        let (graph, index, surface, from_obj) = minimal_resolve_member_fixtures();

        let receiver = ReceiverType::Framework(FrameworkKind::JsonObject);
        let (shape, routes) =
            resolve_member(&receiver, "add", 1, &from_obj, &graph, &index, &surface);
        assert_eq!(shape, DispatchShape::Exact);
        assert_eq!(routes.len(), 1);
        assert!(
            matches!(routes[0].target, RouteTarget::Builtin(_)),
            "must be Builtin"
        );
        assert_eq!(routes[0].evidence, Evidence::Catalog);
        assert!(matches!(routes[0].witness, Witness::CatalogEntry { .. }));
        if let RouteTarget::Builtin(ref bid) = routes[0].target {
            assert_eq!(bid.0, "JsonObject::add");
        }
    }

    // -----------------------------------------------------------------------
    // `ReceiverType::ControlAddIn` dispatch (receiver-closure plan, Task 1) —
    // closed-if-known gating: `Declared` gates on name+arity against the
    // carried procedure list; `TruePlatform` open-accepts unconditionally.
    // -----------------------------------------------------------------------

    #[test]
    fn resolve_member_controladdin_declared_matching_call_is_catalog() {
        use crate::program::resolve::receiver::{ControlAddInSurface, ReceiverType};
        let (graph, index, surface, from_obj) = minimal_resolve_member_fixtures();

        let receiver = ReceiverType::ControlAddIn {
            name_lc: "cdo.editor".into(),
            surface: ControlAddInSurface::Declared {
                procedures: vec![("initeditor".to_string(), 2), ("gethtml".to_string(), 0)],
            },
        };
        let (shape, routes) = resolve_member(
            &receiver,
            "initeditor",
            2,
            &from_obj,
            &graph,
            &index,
            &surface,
        );
        assert_eq!(shape, DispatchShape::Exact);
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].evidence, Evidence::Catalog);
        if let RouteTarget::Builtin(ref bid) = routes[0].target {
            assert_eq!(bid.0, "ControlAddIn::initeditor");
        } else {
            panic!("expected Builtin route, got {:?}", routes[0].target);
        }
    }

    /// A zero-arg declared procedure (`GetHTML()`) also resolves — the arity
    /// gate is `==`, not just "name found somewhere".
    #[test]
    fn resolve_member_controladdin_declared_zero_arity_matching_call_is_catalog() {
        use crate::program::resolve::receiver::{ControlAddInSurface, ReceiverType};
        let (graph, index, surface, from_obj) = minimal_resolve_member_fixtures();

        let receiver = ReceiverType::ControlAddIn {
            name_lc: "cdo.editor".into(),
            surface: ControlAddInSurface::Declared {
                procedures: vec![("initeditor".to_string(), 2), ("gethtml".to_string(), 0)],
            },
        };
        let (_, routes) =
            resolve_member(&receiver, "gethtml", 0, &from_obj, &graph, &index, &surface);
        assert_eq!(routes[0].evidence, Evidence::Catalog);
    }

    /// NEGATIVE — a typo'd method name on a declared addin: not in the
    /// declared list, not on the (empty) platform base-member union →
    /// `Unknown(MemberNotFound)`, never a guessed Catalog.
    #[test]
    fn resolve_member_controladdin_declared_typo_is_unknown_member_not_found() {
        use crate::program::resolve::receiver::{ControlAddInSurface, ReceiverType};
        let (graph, index, surface, from_obj) = minimal_resolve_member_fixtures();

        let receiver = ReceiverType::ControlAddIn {
            name_lc: "cdo.editor".into(),
            surface: ControlAddInSurface::Declared {
                procedures: vec![("initeditor".to_string(), 2)],
            },
        };
        let (_, routes) = resolve_member(
            &receiver,
            "inteditor",
            2,
            &from_obj,
            &graph,
            &index,
            &surface,
        );
        assert_eq!(routes.len(), 1);
        assert_eq!(
            routes[0].evidence,
            Evidence::Unknown(UnknownReason::MemberNotFound)
        );
        assert!(matches!(routes[0].target, RouteTarget::Unresolved));
    }

    /// NEGATIVE — arity gate: the NAME matches a declared procedure but the
    /// call's arity does not — `Unknown(MemberNotFound)`, never a Catalog
    /// built by name alone (the arity closer, T1 BINDING).
    #[test]
    fn resolve_member_controladdin_declared_name_matches_wrong_arity_is_unknown() {
        use crate::program::resolve::receiver::{ControlAddInSurface, ReceiverType};
        let (graph, index, surface, from_obj) = minimal_resolve_member_fixtures();

        let receiver = ReceiverType::ControlAddIn {
            name_lc: "cdo.editor".into(),
            surface: ControlAddInSurface::Declared {
                procedures: vec![("initeditor".to_string(), 2)],
            },
        };
        let (_, routes) = resolve_member(
            &receiver,
            "initeditor",
            1,
            &from_obj,
            &graph,
            &index,
            &surface,
        );
        assert_eq!(
            routes[0].evidence,
            Evidence::Unknown(UnknownReason::MemberNotFound)
        );
    }

    /// NEGATIVE — an EVENT name is structurally never in the `procedures`
    /// list (events are never lowered as `RoutineDecl`s at all — see the
    /// al-syntax lowering tests) — calling one on a declared addin declines,
    /// exactly like any other undeclared member.
    #[test]
    fn resolve_member_controladdin_declared_event_name_is_unknown() {
        use crate::program::resolve::receiver::{ControlAddInSurface, ReceiverType};
        let (graph, index, surface, from_obj) = minimal_resolve_member_fixtures();

        let receiver = ReceiverType::ControlAddIn {
            name_lc: "cdo.editor".into(),
            surface: ControlAddInSurface::Declared {
                procedures: vec![("initeditor".to_string(), 2)],
            },
        };
        let (_, routes) = resolve_member(
            &receiver,
            "onsavehtml",
            1,
            &from_obj,
            &graph,
            &index,
            &surface,
        );
        assert_eq!(
            routes[0].evidence,
            Evidence::Unknown(UnknownReason::MemberNotFound)
        );
    }

    /// POSITIVE — `TruePlatform` open-accepts ANY method/arity — mirrors the
    /// pre-Task-1 universal-accept policy, now scoped to just the platform
    /// allowlist (grounded in the real CDO `WebPageViewer.SetContent(...)`
    /// call sites).
    #[test]
    fn resolve_member_controladdin_true_platform_any_method_is_catalog() {
        use crate::program::resolve::receiver::{ControlAddInSurface, ReceiverType};
        let (graph, index, surface, from_obj) = minimal_resolve_member_fixtures();

        let receiver = ReceiverType::ControlAddIn {
            name_lc: "webpageviewer".into(),
            surface: ControlAddInSurface::TruePlatform,
        };
        let (_, routes) = resolve_member(
            &receiver,
            "setcontent",
            1,
            &from_obj,
            &graph,
            &index,
            &surface,
        );
        assert_eq!(routes[0].evidence, Evidence::Catalog);
        if let RouteTarget::Builtin(ref bid) = routes[0].target {
            assert_eq!(bid.0, "ControlAddIn::setcontent");
        }
        // A wholly made-up method name ALSO resolves — genuinely
        // unconditional, since this engine cannot enumerate the JS-side
        // surface of an unreachable platform addin declaration.
        let (_, routes2) = resolve_member(
            &receiver,
            "totallymadeupmethod",
            7,
            &from_obj,
            &graph,
            &index,
            &surface,
        );
        assert_eq!(routes2[0].evidence, Evidence::Catalog);
    }

    /// The platform base-member union closer (T1 round-2, gemini CRITICAL):
    /// EXECUTABLE proof that the researched EMPTY union holds — none of the
    /// candidate "generic control surface" names a reviewer might expect
    /// (`Visible`/`Editable`/`Enabled`/`Update`/`Caption` — verified against
    /// MS Learn's Visible-property page, which documents these as PAGE-LAYOUT
    /// DESIGN-TIME properties set in the control's property sheet, never
    /// `CurrPage.<control>.<member>` runtime member calls; no generic
    /// AL-callable base method is documented for ANY control add-in beyond
    /// its own declared procedures) silently resolve on a `Declared` addin
    /// that doesn't itself declare them. If a real base surface is ever
    /// found, `resolver::resolve_member_with_args`'s `ControlAddInSurface::Declared`
    /// arm is where it gets unioned in — and this test's assertions would
    /// need updating alongside it (a deliberate tripwire, not incidental).
    #[test]
    fn resolve_member_controladdin_declared_no_platform_base_members_silently_resolve() {
        use crate::program::resolve::receiver::{ControlAddInSurface, ReceiverType};
        let (graph, index, surface, from_obj) = minimal_resolve_member_fixtures();

        let receiver = ReceiverType::ControlAddIn {
            name_lc: "cdo.editor".into(),
            surface: ControlAddInSurface::Declared {
                procedures: vec![("initeditor".to_string(), 2)],
            },
        };
        for candidate in ["visible", "editable", "enabled", "update", "caption"] {
            let (_, routes) =
                resolve_member(&receiver, candidate, 1, &from_obj, &graph, &index, &surface);
            assert_eq!(
                routes[0].evidence,
                Evidence::Unknown(UnknownReason::MemberNotFound),
                "candidate base member {candidate:?} must NOT silently resolve"
            );
        }
    }

    #[test]
    fn resolve_member_fieldref_value_catalog_route() {
        use crate::program::resolve::receiver::ReceiverType;
        let (graph, index, surface, from_obj) = minimal_resolve_member_fixtures();

        let receiver = ReceiverType::FieldRef;
        let (shape, routes) =
            resolve_member(&receiver, "value", 0, &from_obj, &graph, &index, &surface);
        assert_eq!(shape, DispatchShape::Exact);
        assert!(matches!(routes[0].target, RouteTarget::Builtin(_)));
        assert_eq!(routes[0].evidence, Evidence::Catalog);
    }

    #[test]
    fn resolve_member_record_builtin_catalog_route() {
        use crate::program::resolve::receiver::ReceiverType;
        let (graph, index, surface, from_obj) = minimal_resolve_member_fixtures();

        // Record builtin → Catalog route
        let receiver = ReceiverType::Record { table: None };
        let (shape, routes) = resolve_member(
            &receiver, "setrange", 2, &from_obj, &graph, &index, &surface,
        );
        assert_eq!(shape, DispatchShape::Exact);
        assert!(matches!(routes[0].target, RouteTarget::Builtin(_)));
        assert_eq!(routes[0].evidence, Evidence::Catalog);

        // Non-builtin Record method → Unknown
        let (_, routes2) = resolve_member(
            &receiver,
            "calculatediscount",
            0,
            &from_obj,
            &graph,
            &index,
            &surface,
        );
        assert_eq!(routes2[0].target, RouteTarget::Unresolved);
        assert!(matches!(routes2[0].evidence, Evidence::Unknown(_)));
    }

    #[test]
    fn resolve_member_primitive_is_unknown() {
        use crate::program::resolve::receiver::ReceiverType;
        let (graph, index, surface, from_obj) = minimal_resolve_member_fixtures();

        let (shape, routes) = resolve_member(
            &ReceiverType::Primitive,
            "totext",
            0,
            &from_obj,
            &graph,
            &index,
            &surface,
        );
        assert_eq!(shape, DispatchShape::Exact);
        assert!(matches!(routes[0].evidence, Evidence::Unknown(_)));
    }

    #[test]
    fn resolve_member_dynamic_is_dynamic_open() {
        use crate::program::resolve::receiver::ReceiverType;
        let (graph, index, surface, from_obj) = minimal_resolve_member_fixtures();

        let (shape, _routes) = resolve_member(
            &ReceiverType::Dynamic,
            "whatever",
            0,
            &from_obj,
            &graph,
            &index,
            &surface,
        );
        assert_eq!(shape, DispatchShape::DynamicOpen);
    }

    // -----------------------------------------------------------------------
    // Phase 3 Task 3 — Object dispatch + SelfObject tests
    // -----------------------------------------------------------------------

    // (a) Object receiver (Codeunit "MyTarget") + known method "dowork"
    //     → Exact, Routine, Source evidence, SourceSpan witness
    #[test]
    fn resolve_member_object_known_method_resolves_to_source_route() {
        use crate::program::resolve::receiver::ReceiverType;

        let src_target: &'static str = r#"
codeunit 50500 "MyTarget"
{
    procedure DoWork()
    begin
    end;
}
"#;
        let src_caller: &'static str = r#"
codeunit 50501 "Caller"
{
    procedure Trigger()
    begin
    end;
}
"#;
        let app_id = make_app_id("TestApp");
        let unit_target = make_unit(app_id.clone(), "MyTarget.al", src_target);
        let unit_caller = make_unit(app_id, "Caller.al", src_caller);
        let units = [unit_target, unit_caller];
        let graph = build_graph(&units, None);
        let index = ResolveIndex::build(&graph);
        let surface = DeclSurface::build(&graph, &units);

        let from_obj = find_obj(&graph, "Caller");
        let receiver = ReceiverType::Object {
            kind: ObjectKind::Codeunit,
            name_lc: "mytarget".into(),
            id: None,
        };
        let (shape, routes) =
            resolve_member(&receiver, "dowork", 0, from_obj, &graph, &index, &surface);

        assert_eq!(shape, DispatchShape::Exact);
        assert_eq!(routes.len(), 1);
        assert!(
            matches!(routes[0].target, RouteTarget::Routine(_)),
            "target must be Routine; got {:?}",
            routes[0].target
        );
        assert_eq!(routes[0].evidence, Evidence::Source);
        assert!(
            matches!(routes[0].witness, Witness::SourceSpan { .. }),
            "witness must be SourceSpan; got {:?}",
            routes[0].witness
        );
        let RouteTarget::Routine(ref rid) = routes[0].target else {
            unreachable!()
        };
        assert_eq!(rid.name_lc, "dowork");
    }

    // Task 7: `ReceiverType::Object`'s carried `id` (mechanically resolved by
    // Phase A — Step 0's `CurrPage.<part>.Page` subpage receiver) must make
    // `resolve_member` short-circuit on the id DIRECTLY rather than
    // re-resolving `name_lc` against the graph. Proven by giving a `name_lc`
    // that resolves to nothing (`"doesnotexist"`) alongside a valid `id`: if
    // the short-circuit were absent, this would fall back to the by-name
    // lookup and emit an honest Unknown; with it, the call resolves via the
    // carried id regardless of `name_lc`.
    #[test]
    fn resolve_member_object_carried_id_short_circuits_name_lookup() {
        use crate::program::resolve::receiver::ReceiverType;

        let src_target: &'static str = r#"
codeunit 50502 "RealTarget"
{
    procedure DoWork()
    begin
    end;
}
"#;
        let src_caller: &'static str = r#"
codeunit 50503 "Caller2"
{
    procedure Trigger()
    begin
    end;
}
"#;
        let app_id = make_app_id("TestApp");
        let unit_target = make_unit(app_id.clone(), "RealTarget.al", src_target);
        let unit_caller = make_unit(app_id, "Caller2.al", src_caller);
        let units = [unit_target, unit_caller];
        let graph = build_graph(&units, None);
        let index = ResolveIndex::build(&graph);
        let surface = DeclSurface::build(&graph, &units);

        let from_obj = find_obj(&graph, "caller2");
        let real_target_id = find_obj(&graph, "realtarget").id.clone();

        // `name_lc` is deliberately WRONG — a name that resolves to nothing —
        // to prove the carried `id` is what actually drives resolution.
        let receiver = ReceiverType::Object {
            kind: ObjectKind::Codeunit,
            name_lc: "doesnotexist".into(),
            id: Some(real_target_id.clone()),
        };
        let (shape, routes) =
            resolve_member(&receiver, "dowork", 0, from_obj, &graph, &index, &surface);

        assert_eq!(shape, DispatchShape::Exact);
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].evidence, Evidence::Source);
        let RouteTarget::Routine(ref rid) = routes[0].target else {
            panic!(
                "expected RouteTarget::Routine via the carried id, got {:?}",
                routes[0].target
            );
        };
        assert_eq!(rid.name_lc, "dowork");
        assert_eq!(
            rid.object, real_target_id,
            "must dispatch against the CARRIED id, not a (failed) by-name lookup"
        );
    }

    // (b) Codeunit.Run() (arity 0) → Exact, Routine(onrun), Source, SourceSpan
    #[test]
    fn resolve_member_codeunit_run_dispatches_to_onrun_trigger() {
        use crate::program::resolve::receiver::ReceiverType;

        let src_target: &'static str = r#"
codeunit 50502 "RunTarget"
{
    trigger OnRun()
    begin
    end;
}
"#;
        let src_caller: &'static str = r#"
codeunit 50503 "RunCaller"
{
    procedure CallRun()
    begin
    end;
}
"#;
        let app_id = make_app_id("TestApp");
        let unit_target = make_unit(app_id.clone(), "RunTarget.al", src_target);
        let unit_caller = make_unit(app_id, "RunCaller.al", src_caller);
        let units = [unit_target, unit_caller];
        let graph = build_graph(&units, None);
        let index = ResolveIndex::build(&graph);
        let surface = DeclSurface::build(&graph, &units);

        let from_obj = find_obj(&graph, "RunCaller");
        let receiver = ReceiverType::Object {
            kind: ObjectKind::Codeunit,
            name_lc: "runtarget".into(),
            id: None,
        };
        // arity=0 is Cu.Run() with no record argument
        let (shape, routes) =
            resolve_member(&receiver, "run", 0, from_obj, &graph, &index, &surface);

        assert_eq!(shape, DispatchShape::Exact);
        assert_eq!(routes.len(), 1);
        assert!(
            matches!(routes[0].target, RouteTarget::Routine(_)),
            "Codeunit.Run must resolve to the OnRun trigger; got {:?}",
            routes[0].target
        );
        assert_eq!(routes[0].evidence, Evidence::Source);
        assert!(matches!(routes[0].witness, Witness::SourceSpan { .. }));
        let RouteTarget::Routine(ref rid) = routes[0].target else {
            unreachable!()
        };
        assert_eq!(rid.name_lc, "onrun", "must target the OnRun trigger");
    }

    // (c) SelfObject + "helper" proc on the from_object → Exact, Routine, Source
    #[test]
    fn resolve_member_self_object_resolves_own_procedure() {
        use crate::program::resolve::receiver::ReceiverType;

        let src_self: &'static str = r#"
codeunit 50504 "SelfCU"
{
    procedure Helper()
    begin
    end;
    procedure Caller()
    begin
    end;
}
"#;
        let app_id = make_app_id("TestApp");
        let unit_self = make_unit(app_id, "SelfCU.al", src_self);
        let units = [unit_self];
        let graph = build_graph(&units, None);
        let index = ResolveIndex::build(&graph);
        let surface = DeclSurface::build(&graph, &units);

        let from_obj = find_obj(&graph, "SelfCU");
        let (shape, routes) = resolve_member(
            &ReceiverType::SelfObject,
            "helper",
            0,
            from_obj,
            &graph,
            &index,
            &surface,
        );

        assert_eq!(shape, DispatchShape::Exact);
        assert_eq!(routes.len(), 1);
        assert!(
            matches!(routes[0].target, RouteTarget::Routine(_)),
            "SelfObject must resolve to own-object Routine; got {:?}",
            routes[0].target
        );
        assert_eq!(routes[0].evidence, Evidence::Source);
        assert!(matches!(routes[0].witness, Witness::SourceSpan { .. }));
        let RouteTarget::Routine(ref rid) = routes[0].target else {
            unreachable!()
        };
        assert_eq!(rid.name_lc, "helper");
    }

    // (d) Object receiver + nonexistent method → Exact, Unresolved, Unknown, None
    #[test]
    fn resolve_member_object_nonexistent_method_emits_unknown() {
        use crate::program::resolve::receiver::ReceiverType;

        let src_target: &'static str = r#"
codeunit 50505 "AnotherTarget"
{
    procedure RealProc()
    begin
    end;
}
"#;
        let src_caller: &'static str = r#"
codeunit 50506 "AnotherCaller"
{
    procedure Go()
    begin
    end;
}
"#;
        let app_id = make_app_id("TestApp");
        let unit_target = make_unit(app_id.clone(), "AnotherTarget.al", src_target);
        let unit_caller = make_unit(app_id, "AnotherCaller.al", src_caller);
        let units = [unit_target, unit_caller];
        let graph = build_graph(&units, None);
        let index = ResolveIndex::build(&graph);
        let surface = DeclSurface::build(&graph, &units);

        let from_obj = find_obj(&graph, "AnotherCaller");
        let receiver = ReceiverType::Object {
            kind: ObjectKind::Codeunit,
            name_lc: "anothertarget".into(),
            id: None,
        };
        let (shape, routes) = resolve_member(
            &receiver,
            "doesnotexistatall",
            0,
            from_obj,
            &graph,
            &index,
            &surface,
        );

        assert_eq!(shape, DispatchShape::Exact);
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].target, RouteTarget::Unresolved);
        assert_eq!(
            routes[0].evidence,
            Evidence::Unknown(UnknownReason::MemberNotFound),
            "the target OBJECT resolved; only the method is absent — stays \
             MemberNotFound (reason-split Task 2); got {:?}",
            routes[0].evidence
        );
        assert_eq!(routes[0].witness, Witness::None);
        assert_eq!(
            routes[0].receiver_tier,
            Some(TrustTier::Workspace),
            "member-absent-on-a-resolved-surface must tag the resolved \
             receiver's (target object's) tier (reason-split Task 2)"
        );
    }

    // (e) Object receiver where the target object isn't in the graph → Unknown
    #[test]
    fn resolve_member_object_target_not_in_graph_emits_unknown() {
        use crate::program::resolve::receiver::ReceiverType;

        let src_caller: &'static str = r#"
codeunit 50507 "OrphanCaller"
{
    procedure Go()
    begin
    end;
}
"#;
        let app_id = make_app_id("TestApp");
        let unit_caller = make_unit(app_id, "OrphanCaller.al", src_caller);
        let units = [unit_caller];
        let graph = build_graph(&units, None);
        let index = ResolveIndex::build(&graph);
        let surface = DeclSurface::build(&graph, &units);

        let from_obj = find_obj(&graph, "OrphanCaller");
        // "ghosttarget" does not exist in the graph at all
        let receiver = ReceiverType::Object {
            kind: ObjectKind::Codeunit,
            name_lc: "ghosttarget".into(),
            id: None,
        };
        let (shape, routes) = resolve_member(
            &receiver,
            "anymethod",
            0,
            from_obj,
            &graph,
            &index,
            &surface,
        );

        assert_eq!(shape, DispatchShape::Exact);
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].target, RouteTarget::Unresolved);
        assert_eq!(
            routes[0].evidence,
            Evidence::Unknown(UnknownReason::ObjectNotInGraph),
            "reason-split Task 2 — the RECEIVER OBJECT itself is absent; was \
             MemberNotFound pre-split; got {:?}",
            routes[0].evidence
        );
        assert_eq!(routes[0].witness, Witness::None);
        assert_eq!(
            routes[0].receiver_tier, None,
            "ObjectNotInGraph has no resolved receiver to tag"
        );
    }

    // -----------------------------------------------------------------------
    // Phase 3 Task 4 — Record table-procedure dispatch tests
    // -----------------------------------------------------------------------

    // (a) Record receiver + proc on base table → Exact, Source, SourceSpan
    #[test]
    fn resolve_member_record_table_proc_on_base_resolves_to_source() {
        use crate::program::resolve::receiver::ReceiverType;

        let src_table: &'static str = r#"
table 50700 Customer
{
    procedure GetBalance()
    begin
    end;
}
"#;
        let src_caller: &'static str = r#"
codeunit 50701 "TableProcCaller"
{
    procedure Test()
    begin
    end;
}
"#;
        let app_id = make_app_id("TestApp");
        let unit_table = make_unit(app_id.clone(), "Customer.al", src_table);
        let unit_caller = make_unit(app_id, "TableProcCaller.al", src_caller);
        let units = [unit_table, unit_caller];
        let graph = build_graph(&units, None);
        let index = ResolveIndex::build(&graph);
        let surface = DeclSurface::build(&graph, &units);

        let table_obj = find_obj(&graph, "Customer");
        let receiver = ReceiverType::Record {
            table: Some(table_obj.id.clone()),
        };
        let from_obj = find_obj(&graph, "TableProcCaller");
        let (shape, routes) = resolve_member(
            &receiver,
            "getbalance",
            0,
            from_obj,
            &graph,
            &index,
            &surface,
        );

        assert_eq!(shape, DispatchShape::Exact);
        assert_eq!(routes.len(), 1);
        assert!(
            matches!(routes[0].target, RouteTarget::Routine(_)),
            "must resolve to Routine on the base table; got {:?}",
            routes[0].target
        );
        assert_eq!(routes[0].evidence, Evidence::Source);
        assert!(
            matches!(routes[0].witness, Witness::SourceSpan { .. }),
            "witness must be SourceSpan; got {:?}",
            routes[0].witness
        );
        let RouteTarget::Routine(ref rid) = routes[0].target else {
            unreachable!()
        };
        assert_eq!(rid.name_lc, "getbalance");
    }

    // (b) Record receiver + proc only on a TableExtension → resolves via extension
    #[test]
    fn resolve_member_record_table_proc_on_extension_resolves_to_source() {
        use crate::program::resolve::receiver::ReceiverType;

        // Base table has ExistingProc but NOT GetBalance.
        let src_table: &'static str = r#"
table 50800 Vendor
{
    procedure ExistingProc()
    begin
    end;
}
"#;
        // Extension adds GetBalance.
        let src_ext: &'static str = r#"
tableextension 50801 "VendorExt" extends Vendor
{
    procedure GetBalance()
    begin
    end;
}
"#;
        let src_caller: &'static str = r#"
codeunit 50802 "ExtCaller"
{
    procedure Test()
    begin
    end;
}
"#;
        let app_id = make_app_id("TestApp");
        let unit_table = make_unit(app_id.clone(), "Vendor.al", src_table);
        let unit_ext = make_unit(app_id.clone(), "VendorExt.al", src_ext);
        let unit_caller = make_unit(app_id, "ExtCaller.al", src_caller);
        let units = [unit_table, unit_ext, unit_caller];
        let graph = build_graph(&units, None);
        let index = ResolveIndex::build(&graph);
        let surface = DeclSurface::build(&graph, &units);

        let table_obj = find_obj(&graph, "Vendor");
        let receiver = ReceiverType::Record {
            table: Some(table_obj.id.clone()),
        };
        let from_obj = find_obj(&graph, "ExtCaller");
        let (shape, routes) = resolve_member(
            &receiver,
            "getbalance",
            0,
            from_obj,
            &graph,
            &index,
            &surface,
        );

        assert_eq!(shape, DispatchShape::Exact);
        assert_eq!(routes.len(), 1);
        assert!(
            matches!(routes[0].target, RouteTarget::Routine(_)),
            "must resolve to Routine via extension; got {:?}",
            routes[0].target
        );
        assert_eq!(routes[0].evidence, Evidence::Source);
        assert!(
            matches!(routes[0].witness, Witness::SourceSpan { .. }),
            "witness must be SourceSpan; got {:?}",
            routes[0].witness
        );
        let RouteTarget::Routine(ref rid) = routes[0].target else {
            unreachable!()
        };
        assert_eq!(
            rid.object.kind,
            ObjectKind::TableExtension,
            "proc must resolve to the TableExtension, not the base table"
        );
        assert_eq!(rid.name_lc, "getbalance");
    }

    // (c) Record builtin method, NO same-name source competitor → Catalog.
    // (beyond-1B.3b Task 1: renamed from `..._wins_catalog_first` — the table
    // here declares `GetBalance`, not `SetView`, so this was never actually
    // testing catalog-vs-source precedence; it tests that a genuine builtin
    // with zero visible source candidates still resolves Catalog.  The real
    // shadowing case is covered by the `ws-builtin-shadow` r0-corpus fixture
    // in `tests/program_resolve_harness.rs`.)
    #[test]
    fn resolve_member_record_builtin_with_no_source_competitor_resolves_catalog() {
        use crate::program::resolve::receiver::ReceiverType;

        let src_table: &'static str = r#"
table 50900 "SomeTable"
{
    procedure GetBalance()
    begin
    end;
}
"#;
        let src_caller: &'static str = r#"
codeunit 50901 "CatalogFirstCaller"
{
    procedure Test()
    begin
    end;
}
"#;
        let app_id = make_app_id("TestApp");
        let unit_table = make_unit(app_id.clone(), "SomeTable.al", src_table);
        let unit_caller = make_unit(app_id, "CatalogFirstCaller.al", src_caller);
        let units = [unit_table, unit_caller];
        let graph = build_graph(&units, None);
        let index = ResolveIndex::build(&graph);
        let surface = DeclSurface::build(&graph, &units);

        let table_obj = find_obj(&graph, "SomeTable");
        let receiver = ReceiverType::Record {
            table: Some(table_obj.id.clone()),
        };
        let from_obj = find_obj(&graph, "CatalogFirstCaller");

        // "setview" is a Record catalog builtin; "SomeTable" declares no
        // "setview" procedure (base table or extension) — zero source
        // candidates, so resolution correctly falls through to the catalog.
        let (shape, routes) =
            resolve_member(&receiver, "setview", 0, from_obj, &graph, &index, &surface);

        assert_eq!(shape, DispatchShape::Exact);
        assert_eq!(routes.len(), 1);
        assert!(
            matches!(routes[0].target, RouteTarget::Builtin(_)),
            "setview is a Record builtin — Catalog must win; got {:?}",
            routes[0].target
        );
        assert_eq!(routes[0].evidence, Evidence::Catalog);
        assert!(matches!(routes[0].witness, Witness::CatalogEntry { .. }));
    }

    // (d) table == None → Exact, Unknown (table unresolved, e.g. implicit Rec on Page)
    #[test]
    fn resolve_member_record_table_none_emits_unknown() {
        use crate::program::resolve::receiver::ReceiverType;

        let src_caller: &'static str = r#"
codeunit 51000 "NoneTableCaller"
{
    procedure Test()
    begin
    end;
}
"#;
        let app_id = make_app_id("TestApp");
        let unit_caller = make_unit(app_id, "NoneTableCaller.al", src_caller);
        let units = [unit_caller];
        let graph = build_graph(&units, None);
        let index = ResolveIndex::build(&graph);
        let surface = DeclSurface::build(&graph, &units);

        let from_obj = find_obj(&graph, "NoneTableCaller");
        let receiver = ReceiverType::Record { table: None };
        let (shape, routes) = resolve_member(
            &receiver,
            "getbalance",
            0,
            from_obj,
            &graph,
            &index,
            &surface,
        );

        assert_eq!(shape, DispatchShape::Exact);
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].target, RouteTarget::Unresolved);
        assert!(matches!(routes[0].evidence, Evidence::Unknown(_)));
        assert_eq!(routes[0].witness, Witness::None);
    }

    // (e) Method exists on neither base table nor any extension → Exact, Unknown
    #[test]
    fn resolve_member_record_proc_not_found_emits_unknown() {
        use crate::program::resolve::receiver::ReceiverType;

        let src_table: &'static str = r#"
table 51100 "Invoice"
{
    procedure ValidateTotal()
    begin
    end;
}
"#;
        let src_caller: &'static str = r#"
codeunit 51101 "NotFoundCaller"
{
    procedure Test()
    begin
    end;
}
"#;
        let app_id = make_app_id("TestApp");
        let unit_table = make_unit(app_id.clone(), "Invoice.al", src_table);
        let unit_caller = make_unit(app_id, "NotFoundCaller.al", src_caller);
        let units = [unit_table, unit_caller];
        let graph = build_graph(&units, None);
        let index = ResolveIndex::build(&graph);
        let surface = DeclSurface::build(&graph, &units);

        let table_obj = find_obj(&graph, "Invoice");
        let receiver = ReceiverType::Record {
            table: Some(table_obj.id.clone()),
        };
        let from_obj = find_obj(&graph, "NotFoundCaller");
        let (shape, routes) = resolve_member(
            &receiver,
            "nonexistentmethod",
            0,
            from_obj,
            &graph,
            &index,
            &surface,
        );

        assert_eq!(shape, DispatchShape::Exact);
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].target, RouteTarget::Unresolved);
        assert!(matches!(routes[0].evidence, Evidence::Unknown(_)));
        assert_eq!(routes[0].witness, Witness::None);
    }

    // -----------------------------------------------------------------------
    // beyond-1B.3b Task 1 — source shadows builtin (lookup precedence)
    // -----------------------------------------------------------------------

    // (f) A user table procedure whose NAME+ARITY matches a genuine Record
    // builtin (`FieldNo`) must SHADOW the catalog: resolves to the local
    // Source, not `builtin`. This is the exact shape of the 42 real CDO
    // `builtin-catalog-fp-collision` divergences (e.g. `Record::fieldno`,
    // `Record::setrecfilter`).
    #[test]
    fn resolve_member_record_source_proc_shadows_same_named_builtin() {
        use crate::program::resolve::receiver::ReceiverType;

        let src_table: &'static str = r#"
table 50950 "Acme"
{
    procedure FieldNo(FieldName: Text): Integer
    begin
        exit(0);
    end;
}
"#;
        let src_caller: &'static str = r#"
codeunit 50951 "ShadowCaller"
{
    procedure Test()
    begin
    end;
}
"#;
        let app_id = make_app_id("TestApp");
        let unit_table = make_unit(app_id.clone(), "Acme.al", src_table);
        let unit_caller = make_unit(app_id, "ShadowCaller.al", src_caller);
        let units = [unit_table, unit_caller];
        let graph = build_graph(&units, None);
        let index = ResolveIndex::build(&graph);
        let surface = DeclSurface::build(&graph, &units);

        let table_obj = find_obj(&graph, "Acme");
        let receiver = ReceiverType::Record {
            table: Some(table_obj.id.clone()),
        };
        let from_obj = find_obj(&graph, "ShadowCaller");

        // "fieldno" IS a genuine Record catalog builtin (arity 1) — but the
        // table declares its OWN FieldNo(FieldName: Text), matching arity 1.
        let (shape, routes) =
            resolve_member(&receiver, "fieldno", 1, from_obj, &graph, &index, &surface);

        assert_eq!(shape, DispatchShape::Exact);
        assert_eq!(routes.len(), 1);
        assert!(
            matches!(routes[0].target, RouteTarget::Routine(_)),
            "local FieldNo must SHADOW the catalog; got {:?}",
            routes[0].target
        );
        assert_eq!(
            routes[0].evidence,
            Evidence::Source,
            "shadowed call must be Source, not Catalog"
        );
        assert!(matches!(routes[0].witness, Witness::SourceSpan { .. }));
        let RouteTarget::Routine(ref rid) = routes[0].target else {
            unreachable!()
        };
        assert_eq!(rid.name_lc, "fieldno");
        assert_eq!(rid.object.kind, ObjectKind::Table);
    }

    // (g) Two sibling TableExtensions of the same base table both declare a
    // same-name/arity method that ALSO happens to be a Record builtin name:
    // ambiguous source competition must shadow the catalog with an honest
    // Unknown — never pick-first, never fall through to a false `builtin`.
    #[test]
    fn resolve_member_record_ambiguous_extension_competitors_shadow_catalog_as_unknown() {
        use crate::program::resolve::receiver::ReceiverType;

        let src_table: &'static str = r#"
table 50960 "Widget"
{
}
"#;
        // Two independent extensions of the SAME base table both add a
        // `Rename` procedure (arity 1) — `rename` IS a Record catalog builtin.
        let src_ext1: &'static str = r#"
tableextension 50961 "WidgetExt1" extends Widget
{
    procedure Rename(NewName: Text)
    begin
    end;
}
"#;
        let src_ext2: &'static str = r#"
tableextension 50962 "WidgetExt2" extends Widget
{
    procedure Rename(NewName: Text)
    begin
    end;
}
"#;
        let src_caller: &'static str = r#"
codeunit 50963 "AmbiguousCaller"
{
    procedure Test()
    begin
    end;
}
"#;
        let app_id = make_app_id("TestApp");
        let unit_table = make_unit(app_id.clone(), "Widget.al", src_table);
        let unit_ext1 = make_unit(app_id.clone(), "WidgetExt1.al", src_ext1);
        let unit_ext2 = make_unit(app_id.clone(), "WidgetExt2.al", src_ext2);
        let unit_caller = make_unit(app_id, "AmbiguousCaller.al", src_caller);
        let units = [unit_table, unit_ext1, unit_ext2, unit_caller];
        let graph = build_graph(&units, None);
        let index = ResolveIndex::build(&graph);
        let surface = DeclSurface::build(&graph, &units);

        let table_obj = find_obj(&graph, "Widget");
        let receiver = ReceiverType::Record {
            table: Some(table_obj.id.clone()),
        };
        let from_obj = find_obj(&graph, "AmbiguousCaller");

        let (shape, routes) =
            resolve_member(&receiver, "rename", 1, from_obj, &graph, &index, &surface);

        assert_eq!(shape, DispatchShape::Exact);
        assert_eq!(routes.len(), 1);
        assert_eq!(
            routes[0].target,
            RouteTarget::Unresolved,
            "ambiguous source competitors must NOT pick-first AND must NOT \
             fall through to a false Catalog hit; got {:?}",
            routes[0].target
        );
        assert!(
            matches!(routes[0].evidence, Evidence::Unknown(_)),
            "source ambiguity must shadow the catalog as honest Unknown"
        );
        assert_eq!(routes[0].witness, Witness::None);
    }

    // -----------------------------------------------------------------------
    // beyond-1B.3b Task 2 — `resolve_in_table_scope` visibility-scoping
    // characterization (closure filter + Local/Internal access filter).
    // -----------------------------------------------------------------------

    // (h) The BASE TABLE and one of its TableExtensions both declare the SAME
    // name+arity method (not just two sibling extensions, as in (g) above):
    // honest ambiguous Unknown — never pick-first, never fall through to a
    // false Catalog hit.
    #[test]
    fn resolve_member_record_base_and_extension_same_name_collision_is_unknown() {
        use crate::program::resolve::receiver::ReceiverType;

        let src_table: &'static str = r#"
table 52200 "CollBase"
{
    procedure DoWork()
    begin
    end;
}
"#;
        let src_ext: &'static str = r#"
tableextension 52201 "CollBaseExt" extends CollBase
{
    procedure DoWork()
    begin
    end;
}
"#;
        let src_caller: &'static str = r#"
codeunit 52202 "CollCaller"
{
    procedure Test()
    begin
    end;
}
"#;
        let app_id = make_app_id("TestApp");
        let unit_table = make_unit(app_id.clone(), "CollBase.al", src_table);
        let unit_ext = make_unit(app_id.clone(), "CollBaseExt.al", src_ext);
        let unit_caller = make_unit(app_id, "CollCaller.al", src_caller);
        let units = [unit_table, unit_ext, unit_caller];
        let graph = build_graph(&units, None);
        let index = ResolveIndex::build(&graph);
        let surface = DeclSurface::build(&graph, &units);

        let table_obj = find_obj(&graph, "CollBase");
        let receiver = ReceiverType::Record {
            table: Some(table_obj.id.clone()),
        };
        let from_obj = find_obj(&graph, "CollCaller");
        let (shape, routes) =
            resolve_member(&receiver, "dowork", 0, from_obj, &graph, &index, &surface);

        assert_eq!(shape, DispatchShape::Exact);
        assert_eq!(routes.len(), 1);
        assert_eq!(
            routes[0].target,
            RouteTarget::Unresolved,
            "base+extension same-name collision must NOT pick-first; got {:?}",
            routes[0].target
        );
        assert!(matches!(routes[0].evidence, Evidence::Unknown(_)));
        assert_eq!(routes[0].witness, Witness::None);
    }

    // (i) THE core Task 2 soundness fix: a TableExtension declared in an app
    // that is NOT in `from_object`'s dependency closure must NOT be counted
    // as a visible candidate. Pre-Task-2, `table_extensions_of` was
    // whole-snapshot (no closure filter) and this resolved to a false
    // `Source` route pointing at a symbol `from_object`'s own app never
    // imported — a real AL compile could never have produced that edge.
    #[test]
    fn resolve_member_record_extension_outside_dependency_closure_declines() {
        use crate::program::resolve::receiver::ReceiverType;

        // AppB: the base table, no procedures of its own.
        let src_table: &'static str = r#"
table 52300 "VisFoo"
{
}
"#;
        // AppC: a TableExtension of VisFoo declaring DoWork — AppC is NEVER
        // wired as a dependency of AppA below.
        let src_ext: &'static str = r#"
tableextension 52301 "VisFooExtC" extends VisFoo
{
    procedure DoWork()
    begin
    end;
}
"#;
        // AppA: the caller, depends on AppB only (NOT AppC).
        let src_caller: &'static str = r#"
codeunit 52302 "OutClosureCaller"
{
    procedure Test()
    begin
    end;
}
"#;
        let app_a = make_app_id("AppA");
        let app_b = make_app_id("AppB");
        let app_c = make_app_id("AppC");
        let unit_table = make_unit(app_b, "VisFoo.al", src_table);
        let unit_ext = make_unit(app_c, "VisFooExtC.al", src_ext);
        let unit_caller = make_unit(app_a, "OutClosureCaller.al", src_caller);
        let units = [unit_table, unit_ext, unit_caller];
        // AppA -> AppB only; AppC is never a dependency of AppA.
        let graph = build_graph_multi_dep(&units, &[("AppA", "AppB")]);
        let index = ResolveIndex::build(&graph);
        let surface = DeclSurface::build(&graph, &units);

        let table_obj = find_obj(&graph, "VisFoo");
        let receiver = ReceiverType::Record {
            table: Some(table_obj.id.clone()),
        };
        let from_obj = find_obj(&graph, "OutClosureCaller");
        let (shape, routes) =
            resolve_member(&receiver, "dowork", 0, from_obj, &graph, &index, &surface);

        assert_eq!(shape, DispatchShape::Exact);
        assert_eq!(routes.len(), 1);
        assert_eq!(
            routes[0].target,
            RouteTarget::Unresolved,
            "an extension declared in an app OUTSIDE from_object's dependency \
             closure must NOT resolve — from_object's AL compiler could never \
             have imported that symbol; got {:?}",
            routes[0].target
        );
        assert!(
            matches!(routes[0].evidence, Evidence::Unknown(_)),
            "must decline honestly, not fabricate a false Source"
        );
        assert_eq!(routes[0].witness, Witness::None);
    }

    // (j) The base table itself, declared in a DEPENDENCY app (cross-app
    // relative to `from_object`), with the candidate method marked
    // `internal`: not visible outside its declaring app — must decline, even
    // though the table's app IS in from_object's closure (the closure filter
    // alone is not sufficient; the access filter is independent).
    #[test]
    fn resolve_member_record_cross_app_base_table_internal_method_excluded() {
        use crate::program::resolve::receiver::ReceiverType;

        let src_table: &'static str = r#"
table 52500 "BaseIntFoo"
{
    internal procedure DoWork()
    begin
    end;
}
"#;
        let src_caller: &'static str = r#"
codeunit 52501 "BaseIntCallerA"
{
    procedure Test()
    begin
    end;
}
"#;
        let app_a = make_app_id("AppA");
        let app_b = make_app_id("AppB");
        let unit_table = make_unit(app_b, "BaseIntFoo.al", src_table);
        let unit_caller = make_unit(app_a, "BaseIntCallerA.al", src_caller);
        let units = [unit_table, unit_caller];
        let graph = build_graph_multi_dep(&units, &[("AppA", "AppB")]);
        let index = ResolveIndex::build(&graph);
        let surface = DeclSurface::build(&graph, &units);

        let table_obj = find_obj(&graph, "BaseIntFoo");
        let receiver = ReceiverType::Record {
            table: Some(table_obj.id.clone()),
        };
        let from_obj = find_obj(&graph, "BaseIntCallerA");
        let (shape, routes) =
            resolve_member(&receiver, "dowork", 0, from_obj, &graph, &index, &surface);

        assert_eq!(shape, DispatchShape::Exact);
        assert_eq!(routes.len(), 1);
        assert_eq!(
            routes[0].target,
            RouteTarget::Unresolved,
            "a cross-app `internal` method on the BASE TABLE itself must be \
             excluded, not just on extensions; got {:?}",
            routes[0].target
        );
        assert!(matches!(routes[0].evidence, Evidence::Unknown(_)));
    }

    // (k) A TableExtension declared in a DEPENDENCY app (in-closure) whose
    // candidate method is marked `internal`: excluded — not visible outside
    // its declaring app even though the app itself is reachable.
    #[test]
    fn resolve_member_record_cross_app_extension_internal_method_excluded() {
        use crate::program::resolve::receiver::ReceiverType;

        let src_table: &'static str = r#"
table 52400 "IntFoo"
{
}
"#;
        let src_ext: &'static str = r#"
tableextension 52401 "IntFooExtB" extends IntFoo
{
    internal procedure DoWork()
    begin
    end;
}
"#;
        let src_caller: &'static str = r#"
codeunit 52402 "IntCallerA"
{
    procedure Test()
    begin
    end;
}
"#;
        let app_a = make_app_id("AppA");
        let app_b = make_app_id("AppB");
        // Both the table AND the extension live in AppB (a dependency of
        // AppA) — only the extension's app differing from from_object's app
        // matters, not which app it extends.
        let unit_table = make_unit(app_b.clone(), "IntFoo.al", src_table);
        let unit_ext = make_unit(app_b, "IntFooExtB.al", src_ext);
        let unit_caller = make_unit(app_a, "IntCallerA.al", src_caller);
        let units = [unit_table, unit_ext, unit_caller];
        let graph = build_graph_multi_dep(&units, &[("AppA", "AppB")]);
        let index = ResolveIndex::build(&graph);
        let surface = DeclSurface::build(&graph, &units);

        let table_obj = find_obj(&graph, "IntFoo");
        let receiver = ReceiverType::Record {
            table: Some(table_obj.id.clone()),
        };
        let from_obj = find_obj(&graph, "IntCallerA");
        let (shape, routes) =
            resolve_member(&receiver, "dowork", 0, from_obj, &graph, &index, &surface);

        assert_eq!(shape, DispatchShape::Exact);
        assert_eq!(routes.len(), 1);
        assert_eq!(
            routes[0].target,
            RouteTarget::Unresolved,
            "a cross-app `internal` TableExtension method must be excluded; \
             got {:?}",
            routes[0].target
        );
        assert!(matches!(routes[0].evidence, Evidence::Unknown(_)));
    }

    // (l) Same shape as (k) but with `local` instead of `internal` — `local`
    // is even MORE restrictive (not even visible to other objects in its OWN
    // app), so it must a fortiori be excluded cross-app.
    #[test]
    fn resolve_member_record_cross_app_extension_local_method_excluded() {
        use crate::program::resolve::receiver::ReceiverType;

        let src_table: &'static str = r#"
table 52410 "LocFoo"
{
}
"#;
        let src_ext: &'static str = r#"
tableextension 52411 "LocFooExtB" extends LocFoo
{
    local procedure DoWork()
    begin
    end;
}
"#;
        let src_caller: &'static str = r#"
codeunit 52412 "LocCallerA"
{
    procedure Test()
    begin
    end;
}
"#;
        let app_a = make_app_id("AppA");
        let app_b = make_app_id("AppB");
        let unit_table = make_unit(app_b.clone(), "LocFoo.al", src_table);
        let unit_ext = make_unit(app_b, "LocFooExtB.al", src_ext);
        let unit_caller = make_unit(app_a, "LocCallerA.al", src_caller);
        let units = [unit_table, unit_ext, unit_caller];
        let graph = build_graph_multi_dep(&units, &[("AppA", "AppB")]);
        let index = ResolveIndex::build(&graph);
        let surface = DeclSurface::build(&graph, &units);

        let table_obj = find_obj(&graph, "LocFoo");
        let receiver = ReceiverType::Record {
            table: Some(table_obj.id.clone()),
        };
        let from_obj = find_obj(&graph, "LocCallerA");
        let (shape, routes) =
            resolve_member(&receiver, "dowork", 0, from_obj, &graph, &index, &surface);

        assert_eq!(shape, DispatchShape::Exact);
        assert_eq!(routes.len(), 1);
        assert_eq!(
            routes[0].target,
            RouteTarget::Unresolved,
            "a cross-app `local` TableExtension method must be excluded; got {:?}",
            routes[0].target
        );
        assert!(matches!(routes[0].evidence, Evidence::Unknown(_)));
    }

    // (m) Regression guard: a cross-app TableExtension method with NO access
    // modifier (Public, the default) must still resolve to Source — the
    // access filter must not over-exclude ordinary public cross-app methods.
    #[test]
    fn resolve_member_record_cross_app_extension_public_method_still_resolves() {
        use crate::program::resolve::receiver::ReceiverType;

        let src_table: &'static str = r#"
table 52420 "PubFoo"
{
}
"#;
        let src_ext: &'static str = r#"
tableextension 52421 "PubFooExtB" extends PubFoo
{
    procedure DoWork()
    begin
    end;
}
"#;
        let src_caller: &'static str = r#"
codeunit 52422 "PubCallerA"
{
    procedure Test()
    begin
    end;
}
"#;
        let app_a = make_app_id("AppA");
        let app_b = make_app_id("AppB");
        let unit_table = make_unit(app_b.clone(), "PubFoo.al", src_table);
        let unit_ext = make_unit(app_b, "PubFooExtB.al", src_ext);
        let unit_caller = make_unit(app_a, "PubCallerA.al", src_caller);
        let units = [unit_table, unit_ext, unit_caller];
        let graph = build_graph_multi_dep(&units, &[("AppA", "AppB")]);
        let index = ResolveIndex::build(&graph);
        let surface = DeclSurface::build(&graph, &units);

        let table_obj = find_obj(&graph, "PubFoo");
        let receiver = ReceiverType::Record {
            table: Some(table_obj.id.clone()),
        };
        let from_obj = find_obj(&graph, "PubCallerA");
        let (shape, routes) =
            resolve_member(&receiver, "dowork", 0, from_obj, &graph, &index, &surface);

        assert_eq!(shape, DispatchShape::Exact);
        assert_eq!(routes.len(), 1);
        assert!(
            matches!(routes[0].target, RouteTarget::Routine(_)),
            "a Public cross-app extension method must still resolve to \
             Source — the access filter must not over-exclude; got {:?}",
            routes[0].target
        );
        assert_eq!(routes[0].evidence, Evidence::Source);
        let RouteTarget::Routine(ref rid) = routes[0].target else {
            unreachable!()
        };
        assert_eq!(rid.object.kind, ObjectKind::TableExtension);
    }

    // -----------------------------------------------------------------------
    // beyond-1B.3b Task 1 — caller-identity-aware visibility: same-app
    // `local` is OBJECT-scoped (not app-scoped), cross-app `Protected` is
    // filtered via identity `object_extends`. The full access matrix from
    // the task brief, lettered (a)-(l); (e) and (l) (cross-app `local`/
    // `internal` exclusion) were ALREADY covered by the beyond-1B.3b Task 2
    // tests above (`resolve_member_record_cross_app_extension_local_method_
    // excluded`, `resolve_member_record_cross_app_base_table_internal_
    // method_excluded`, `resolve_member_record_cross_app_extension_internal_
    // method_excluded`) and are re-asserted green by this task's refactor,
    // not re-duplicated here. See `tests/r0-corpus/ws-visibility-local-
    // protected/COMPILER_PROOF.md` for the AL-compiler semantics backing
    // every lettered case.
    // -----------------------------------------------------------------------

    // (a) SELF: an object's OWN `local procedure`, called via a `Record`
    // variable of the object's OWN type (`Rec.DoWork()` from inside the same
    // table) — `Access::Local` visible to self.
    #[test]
    fn resolve_member_record_local_self_call_resolves_to_source() {
        use crate::program::resolve::receiver::ReceiverType;

        let src_table: &'static str = r#"
table 52600 "LocSelfFoo"
{
    procedure Wrapper()
    var
        R: Record LocSelfFoo;
    begin
        R.DoWork();
    end;

    local procedure DoWork()
    begin
    end;
}
"#;
        let app_id = make_app_id("TestApp");
        let unit_table = make_unit(app_id, "LocSelfFoo.al", src_table);
        let units = [unit_table];
        let graph = build_graph(&units, None);
        let index = ResolveIndex::build(&graph);
        let surface = DeclSurface::build(&graph, &units);

        let table_obj = find_obj(&graph, "LocSelfFoo");
        let receiver = ReceiverType::Record {
            table: Some(table_obj.id.clone()),
        };
        // The CALLING object IS the table itself — the self case.
        let (shape, routes) =
            resolve_member(&receiver, "dowork", 0, table_obj, &graph, &index, &surface);

        assert_eq!(shape, DispatchShape::Exact);
        assert_eq!(routes.len(), 1);
        assert!(
            matches!(routes[0].target, RouteTarget::Routine(_)),
            "an object's own `local` procedure must be visible to ITSELF; got {:?}",
            routes[0].target
        );
        assert_eq!(routes[0].evidence, Evidence::Source);
    }

    // (b) same-app DIFFERENT object: `Table Foo` (no `DoWork`) + a
    // `TableExtension FooExtB` (SAME app) declaring `local procedure
    // DoWork()`; a same-app but DIFFERENT `Codeunit CallerA` calls
    // `R.DoWork()` — AL `local` is OBJECT-scoped, so this must decline. THE
    // pre-fix bug: `object_has_visible_member_candidate`'s same-app branch
    // used to return `true` unconditionally, false-resolving this to
    // `Source` targeting `FooExtB.DoWork` (verified against unfixed code
    // during this task's TDD Step 2 — see the task report).
    #[test]
    fn resolve_member_record_same_app_extension_local_method_excluded() {
        use crate::program::resolve::receiver::ReceiverType;

        let src_table: &'static str = r#"
table 52610 "Foo"
{
}
"#;
        let src_ext: &'static str = r#"
tableextension 52611 "FooExtB" extends Foo
{
    local procedure DoWork()
    begin
    end;
}
"#;
        let src_caller: &'static str = r#"
codeunit 52612 "CallerA"
{
    procedure Test()
    var
        R: Record Foo;
    begin
        R.DoWork();
    end;
}
"#;
        let app_id = make_app_id("TestApp");
        let unit_table = make_unit(app_id.clone(), "Foo.al", src_table);
        let unit_ext = make_unit(app_id.clone(), "FooExtB.al", src_ext);
        let unit_caller = make_unit(app_id, "CallerA.al", src_caller);
        let units = [unit_table, unit_ext, unit_caller];
        let graph = build_graph(&units, None);
        let index = ResolveIndex::build(&graph);
        let surface = DeclSurface::build(&graph, &units);

        let table_obj = find_obj(&graph, "Foo");
        let receiver = ReceiverType::Record {
            table: Some(table_obj.id.clone()),
        };
        let from_obj = find_obj(&graph, "CallerA");
        let (shape, routes) =
            resolve_member(&receiver, "dowork", 0, from_obj, &graph, &index, &surface);

        assert_eq!(shape, DispatchShape::Exact);
        assert_eq!(routes.len(), 1);
        assert_eq!(
            routes[0].target,
            RouteTarget::Unresolved,
            "a same-app but DIFFERENT object's `local` TableExtension method \
             must be excluded (AL `local` is OBJECT-scoped, not app-scoped); \
             got {:?}",
            routes[0].target
        );
        assert!(matches!(routes[0].evidence, Evidence::Unknown(_)));
    }

    // (c) TableExtension `local` SELF-call: the extension declares its own
    // `local procedure` and calls it via `Rec.DoWork()` where `Rec` is typed
    // to the BASE table (the only way to reference a TableExtension's own
    // members) — the calling object (the extension) equals the candidate's
    // declaring object, so this is self, not the app-scoped case.
    #[test]
    fn resolve_member_record_tableext_local_self_call_resolves_to_source() {
        use crate::program::resolve::receiver::ReceiverType;

        let src_table: &'static str = r#"
table 52620 "Foo"
{
}
"#;
        let src_ext: &'static str = r#"
tableextension 52621 "FooExtC" extends Foo
{
    procedure Wrapper()
    var
        R: Record Foo;
    begin
        R.DoWork();
    end;

    local procedure DoWork()
    begin
    end;
}
"#;
        let app_id = make_app_id("TestApp");
        let unit_table = make_unit(app_id.clone(), "Foo.al", src_table);
        let unit_ext = make_unit(app_id, "FooExtC.al", src_ext);
        let units = [unit_table, unit_ext];
        let graph = build_graph(&units, None);
        let index = ResolveIndex::build(&graph);
        let surface = DeclSurface::build(&graph, &units);

        let table_obj = find_obj(&graph, "Foo");
        let receiver = ReceiverType::Record {
            table: Some(table_obj.id.clone()),
        };
        let from_obj = find_obj(&graph, "FooExtC");
        let (shape, routes) =
            resolve_member(&receiver, "dowork", 0, from_obj, &graph, &index, &surface);

        assert_eq!(shape, DispatchShape::Exact);
        assert_eq!(routes.len(), 1);
        assert!(
            matches!(routes[0].target, RouteTarget::Routine(_)),
            "a TableExtension's own `local` procedure must be visible to \
             ITSELF; got {:?}",
            routes[0].target
        );
        assert_eq!(routes[0].evidence, Evidence::Source);
        let RouteTarget::Routine(ref rid) = routes[0].target else {
            unreachable!()
        };
        assert_eq!(rid.object.kind, ObjectKind::TableExtension);
    }

    // (d) PEER-extension: `FooExtA` declares `local procedure DoWork()`;
    // sibling `FooExtB` (same base, same app) calls `R.DoWork()` — NOT self
    // (the calling object is FooExtB, the declaring object is FooExtA) — must
    // decline even though both are same-app AND both extend the same base.
    #[test]
    fn resolve_member_record_peer_extension_local_method_excluded() {
        use crate::program::resolve::receiver::ReceiverType;

        let src_table: &'static str = r#"
table 52630 "Foo"
{
}
"#;
        let src_ext_a: &'static str = r#"
tableextension 52631 "FooExtA" extends Foo
{
    local procedure DoWork()
    begin
    end;
}
"#;
        let src_ext_b: &'static str = r#"
tableextension 52632 "FooExtB" extends Foo
{
    procedure Wrapper()
    var
        R: Record Foo;
    begin
        R.DoWork();
    end;
}
"#;
        let app_id = make_app_id("TestApp");
        let unit_table = make_unit(app_id.clone(), "Foo.al", src_table);
        let unit_ext_a = make_unit(app_id.clone(), "FooExtA.al", src_ext_a);
        let unit_ext_b = make_unit(app_id, "FooExtB.al", src_ext_b);
        let units = [unit_table, unit_ext_a, unit_ext_b];
        let graph = build_graph(&units, None);
        let index = ResolveIndex::build(&graph);
        let surface = DeclSurface::build(&graph, &units);

        let table_obj = find_obj(&graph, "Foo");
        let receiver = ReceiverType::Record {
            table: Some(table_obj.id.clone()),
        };
        let from_obj = find_obj(&graph, "FooExtB");
        let (shape, routes) =
            resolve_member(&receiver, "dowork", 0, from_obj, &graph, &index, &surface);

        assert_eq!(shape, DispatchShape::Exact);
        assert_eq!(routes.len(), 1);
        assert_eq!(
            routes[0].target,
            RouteTarget::Unresolved,
            "a PEER extension's `local` method must never be visible to a \
             sibling extension; got {:?}",
            routes[0].target
        );
        assert!(matches!(routes[0].evidence, Evidence::Unknown(_)));
    }

    // (f) SELF: the declaring TABLE calls its OWN `protected procedure` via
    // `Rec.P()` — trivially visible (self), symmetric with (a).
    #[test]
    fn resolve_member_record_protected_self_call_resolves_to_source() {
        use crate::program::resolve::receiver::ReceiverType;

        let src_table: &'static str = r#"
table 52640 "Bar"
{
    procedure Wrapper()
    var
        R: Record Bar;
    begin
        R.P();
    end;

    protected procedure P()
    begin
    end;
}
"#;
        let app_id = make_app_id("TestApp");
        let unit_table = make_unit(app_id, "Bar.al", src_table);
        let units = [unit_table];
        let graph = build_graph(&units, None);
        let index = ResolveIndex::build(&graph);
        let surface = DeclSurface::build(&graph, &units);

        let table_obj = find_obj(&graph, "Bar");
        let receiver = ReceiverType::Record {
            table: Some(table_obj.id.clone()),
        };
        let (shape, routes) =
            resolve_member(&receiver, "p", 0, table_obj, &graph, &index, &surface);

        assert_eq!(shape, DispatchShape::Exact);
        assert_eq!(routes.len(), 1);
        assert!(
            matches!(routes[0].target, RouteTarget::Routine(_)),
            "an object's own `protected` procedure must be visible to \
             ITSELF; got {:?}",
            routes[0].target
        );
        assert_eq!(routes[0].evidence, Evidence::Source);
    }

    // (g) same-app NON-extension: `Table Bar` with `protected procedure P()`;
    // a same-app `Codeunit` — NOT an extension of Bar — calls `R.P()`. THE
    // pre-fix bug: `Access::Protected` was completely unfiltered for any
    // same-app candidate, false-resolving this to `Source` targeting
    // `Bar.P` (verified against unfixed code — see the task report).
    #[test]
    fn resolve_member_record_same_app_non_extension_protected_excluded() {
        use crate::program::resolve::receiver::ReceiverType;

        let src_table: &'static str = r#"
table 52650 "Bar"
{
    protected procedure P()
    begin
    end;
}
"#;
        let src_caller: &'static str = r#"
codeunit 52651 "CallerG"
{
    procedure Test()
    var
        R: Record Bar;
    begin
        R.P();
    end;
}
"#;
        let app_id = make_app_id("TestApp");
        let unit_table = make_unit(app_id.clone(), "Bar.al", src_table);
        let unit_caller = make_unit(app_id, "CallerG.al", src_caller);
        let units = [unit_table, unit_caller];
        let graph = build_graph(&units, None);
        let index = ResolveIndex::build(&graph);
        let surface = DeclSurface::build(&graph, &units);

        let table_obj = find_obj(&graph, "Bar");
        let receiver = ReceiverType::Record {
            table: Some(table_obj.id.clone()),
        };
        let from_obj = find_obj(&graph, "CallerG");
        let (shape, routes) = resolve_member(&receiver, "p", 0, from_obj, &graph, &index, &surface);

        assert_eq!(shape, DispatchShape::Exact);
        assert_eq!(routes.len(), 1);
        assert_eq!(
            routes[0].target,
            RouteTarget::Unresolved,
            "a same-app NON-extension object must NOT see a table's \
             `protected` procedure (not an extension of Bar → invisible); \
             got {:?}",
            routes[0].target
        );
        assert!(matches!(routes[0].evidence, Evidence::Unknown(_)));
    }

    // (g) supplemental: same shape as above, but the same-app caller is a
    // PAGE (SourceTable = Bar) rather than a Codeunit — the brief's example
    // shape. `object_has_visible_member_candidate` gates on the CALLING
    // object's identity/kind, never the receiver's own kind, so a Page
    // caller must decline identically to a Codeunit caller.
    #[test]
    fn resolve_member_record_same_app_page_non_extension_protected_excluded() {
        use crate::program::resolve::receiver::ReceiverType;

        let src_table: &'static str = r#"
table 52652 "Bar"
{
    protected procedure P()
    begin
    end;
}
"#;
        let src_page: &'static str = r#"
page 52653 "CallerGPage"
{
    SourceTable = Bar;

    procedure Test()
    var
        R: Record Bar;
    begin
        R.P();
    end;
}
"#;
        let app_id = make_app_id("TestApp");
        let unit_table = make_unit(app_id.clone(), "Bar.al", src_table);
        let unit_page = make_unit(app_id, "CallerGPage.al", src_page);
        let units = [unit_table, unit_page];
        let graph = build_graph(&units, None);
        let index = ResolveIndex::build(&graph);
        let surface = DeclSurface::build(&graph, &units);

        let table_obj = find_obj(&graph, "Bar");
        let receiver = ReceiverType::Record {
            table: Some(table_obj.id.clone()),
        };
        let from_obj = find_obj(&graph, "CallerGPage");
        let (shape, routes) = resolve_member(&receiver, "p", 0, from_obj, &graph, &index, &surface);

        assert_eq!(shape, DispatchShape::Exact);
        assert_eq!(routes.len(), 1);
        assert_eq!(
            routes[0].target,
            RouteTarget::Unresolved,
            "a same-app Page whose SourceTable is Bar, but which does NOT \
             extend Bar, must NOT see Bar's `protected` procedure; got {:?}",
            routes[0].target
        );
        assert!(matches!(routes[0].evidence, Evidence::Unknown(_)));
    }

    // (h) cross-app NON-extension: same shape as (g), but the caller is in a
    // DIFFERENT app (a dependency relationship, not an extension
    // relationship) — must decline a fortiori.
    #[test]
    fn resolve_member_record_cross_app_non_extension_protected_excluded() {
        use crate::program::resolve::receiver::ReceiverType;

        let src_table: &'static str = r#"
table 52660 "Bar"
{
    protected procedure P()
    begin
    end;
}
"#;
        let src_caller: &'static str = r#"
codeunit 52661 "CallerH"
{
    procedure Test()
    var
        R: Record Bar;
    begin
        R.P();
    end;
}
"#;
        let app_a = make_app_id("AppA");
        let app_b = make_app_id("AppB");
        let unit_table = make_unit(app_b, "Bar.al", src_table);
        let unit_caller = make_unit(app_a, "CallerH.al", src_caller);
        let units = [unit_table, unit_caller];
        let graph = build_graph_multi_dep(&units, &[("AppA", "AppB")]);
        let index = ResolveIndex::build(&graph);
        let surface = DeclSurface::build(&graph, &units);

        let table_obj = find_obj(&graph, "Bar");
        let receiver = ReceiverType::Record {
            table: Some(table_obj.id.clone()),
        };
        let from_obj = find_obj(&graph, "CallerH");
        let (shape, routes) = resolve_member(&receiver, "p", 0, from_obj, &graph, &index, &surface);

        assert_eq!(shape, DispatchShape::Exact);
        assert_eq!(routes.len(), 1);
        assert_eq!(
            routes[0].target,
            RouteTarget::Unresolved,
            "a cross-app NON-extension object must NOT see a dependency \
             table's `protected` procedure; got {:?}",
            routes[0].target
        );
        assert!(matches!(routes[0].evidence, Evidence::Unknown(_)));
    }

    // (i) valid extension → base protected: a `TableExtension` on `Bar`
    // calling `Bar`'s `protected P()` — `from_object` DIRECTLY extends the
    // declaring object, kind-compatible (TableExtension→Table).
    #[test]
    fn resolve_member_record_tableext_protected_base_resolves_to_source() {
        use crate::program::resolve::receiver::ReceiverType;

        let src_table: &'static str = r#"
table 52670 "Bar"
{
    protected procedure P()
    begin
    end;
}
"#;
        let src_ext: &'static str = r#"
tableextension 52671 "BarExtI" extends Bar
{
    procedure Wrapper()
    var
        R: Record Bar;
    begin
        R.P();
    end;
}
"#;
        let app_id = make_app_id("TestApp");
        let unit_table = make_unit(app_id.clone(), "Bar.al", src_table);
        let unit_ext = make_unit(app_id, "BarExtI.al", src_ext);
        let units = [unit_table, unit_ext];
        let graph = build_graph(&units, None);
        let index = ResolveIndex::build(&graph);
        let surface = DeclSurface::build(&graph, &units);

        let table_obj = find_obj(&graph, "Bar");
        let receiver = ReceiverType::Record {
            table: Some(table_obj.id.clone()),
        };
        let from_obj = find_obj(&graph, "BarExtI");
        let (shape, routes) = resolve_member(&receiver, "p", 0, from_obj, &graph, &index, &surface);

        assert_eq!(shape, DispatchShape::Exact);
        assert_eq!(routes.len(), 1);
        assert!(
            matches!(routes[0].target, RouteTarget::Routine(_)),
            "a TableExtension must see its BASE table's `protected` \
             procedure; got {:?}",
            routes[0].target
        );
        assert_eq!(routes[0].evidence, Evidence::Source);
        let RouteTarget::Routine(ref rid) = routes[0].target else {
            unreachable!()
        };
        assert_eq!(rid.object.kind, ObjectKind::Table);
    }

    // (i) PageExtension→base Page generalization: `ResolveIndex::object_extends`
    // is DIRECTLY exercised (not via `resolve_member`/`object_has_visible_
    // member_candidate` — Page member calls never route through
    // `resolve_in_table_scope`, which is Table/TableExtension-only; see
    // `ResolveIndex::table_extensions_of`) to prove the identity check is
    // GENERALIZED across extension kinds, not hardcoded to TableExtension —
    // the gpt/gemini round-1 convergent fix. A PageExtension of a base Page
    // is a DIRECT, kind-compatible extension relationship exactly like
    // TableExtension→Table.
    #[test]
    fn object_extends_generalizes_to_pageextension_base_page() {
        let src_page: &'static str = r#"
page 52680 "BasePage"
{
}
"#;
        let src_ext: &'static str = r#"
pageextension 52681 "BasePageExtI" extends BasePage
{
}
"#;
        let app_id = make_app_id("TestApp");
        let unit_page = make_unit(app_id.clone(), "BasePage.al", src_page);
        let unit_ext = make_unit(app_id, "BasePageExtI.al", src_ext);
        let units = [unit_page, unit_ext];
        let graph = build_graph(&units, None);
        let index = ResolveIndex::build(&graph);

        let base_page = find_obj(&graph, "BasePage");
        let page_ext = find_obj(&graph, "BasePageExtI");

        assert!(
            index.object_extends(&graph, &page_ext.id, &base_page.id),
            "a PageExtension must be recognized as DIRECTLY extending its \
             base Page — the kind-generalized object_extends contract"
        );
        // Kind-compat guard, exercised from the SAME fixture: the base Page
        // does not "extend" anything (not an extension kind at all), so the
        // reverse direction is trivially false — see the dedicated reverse
        // test below for the full base/extension asymmetry.
        assert!(!index.object_extends(&graph, &base_page.id, &page_ext.id));
    }

    // `object_extends` must be kind-compatible: a TableExtension's
    // `extends_target` resolves to a Table, never to a same-named object of
    // the WRONG kind. This fixture makes a `Table` and a `Page` share the
    // literal name `"Shared"` on purpose — a TableExtension naming `Shared`
    // in its `extends` clause must resolve against the Table, and
    // `object_extends` against the Page identity must be `false` even though
    // the raw name matches, proving the kind filter (not just identity) does
    // the work.
    #[test]
    fn object_extends_is_kind_compatible_not_name_only() {
        let src_table: &'static str = r#"
table 52690 "Shared"
{
}
"#;
        let src_page: &'static str = r#"
page 52691 "Shared"
{
}
"#;
        let src_ext: &'static str = r#"
tableextension 52692 "SharedExt" extends Shared
{
}
"#;
        let app_id = make_app_id("TestApp");
        let unit_table = make_unit(app_id.clone(), "SharedTable.al", src_table);
        let unit_page = make_unit(app_id.clone(), "SharedPage.al", src_page);
        let unit_ext = make_unit(app_id, "SharedExt.al", src_ext);
        let units = [unit_table, unit_page, unit_ext];
        let graph = build_graph(&units, None);
        let index = ResolveIndex::build(&graph);

        let table_obj = graph
            .objects
            .iter()
            .find(|o| o.id.kind == ObjectKind::Table && o.name.eq_ignore_ascii_case("Shared"))
            .expect("table Shared");
        let page_obj = graph
            .objects
            .iter()
            .find(|o| o.id.kind == ObjectKind::Page && o.name.eq_ignore_ascii_case("Shared"))
            .expect("page Shared");
        let ext_obj = find_obj(&graph, "SharedExt");

        assert!(
            index.object_extends(&graph, &ext_obj.id, &table_obj.id),
            "a TableExtension must extend the SAME-KIND (Table) object"
        );
        assert!(
            !index.object_extends(&graph, &ext_obj.id, &page_obj.id),
            "a TableExtension must NEVER be considered to extend a \
             same-NAMED but WRONG-KIND (Page) object"
        );
    }

    // `object_extends` must never be reverse: a base object does not
    // "extend" its own extension, even when the extension's `extends_target`
    // correctly names the base.
    #[test]
    fn object_extends_never_reverse() {
        let src_table: &'static str = r#"
table 52693 "RevBase"
{
}
"#;
        let src_ext: &'static str = r#"
tableextension 52694 "RevExt" extends RevBase
{
}
"#;
        let app_id = make_app_id("TestApp");
        let unit_table = make_unit(app_id.clone(), "RevBase.al", src_table);
        let unit_ext = make_unit(app_id, "RevExt.al", src_ext);
        let units = [unit_table, unit_ext];
        let graph = build_graph(&units, None);
        let index = ResolveIndex::build(&graph);

        let base_obj = find_obj(&graph, "RevBase");
        let ext_obj = find_obj(&graph, "RevExt");

        assert!(index.object_extends(&graph, &ext_obj.id, &base_obj.id));
        assert!(
            !index.object_extends(&graph, &base_obj.id, &ext_obj.id),
            "a base object must NEVER be considered to extend its own \
             extension (the relationship is not symmetric)"
        );
    }

    // `object_extends` must never treat sibling extensions of the same base
    // as extending EACH OTHER — the direct unit-level counterpart of (j)'s
    // end-to-end peer-bleed regression below.
    #[test]
    fn object_extends_never_peer() {
        let src_table: &'static str = r#"
table 52695 "PeerBase"
{
}
"#;
        let src_ext_a: &'static str = r#"
tableextension 52696 "PeerExtA" extends PeerBase
{
}
"#;
        let src_ext_b: &'static str = r#"
tableextension 52697 "PeerExtB" extends PeerBase
{
}
"#;
        let app_id = make_app_id("TestApp");
        let unit_table = make_unit(app_id.clone(), "PeerBase.al", src_table);
        let unit_ext_a = make_unit(app_id.clone(), "PeerExtA.al", src_ext_a);
        let unit_ext_b = make_unit(app_id, "PeerExtB.al", src_ext_b);
        let units = [unit_table, unit_ext_a, unit_ext_b];
        let graph = build_graph(&units, None);
        let index = ResolveIndex::build(&graph);

        let ext_a = find_obj(&graph, "PeerExtA");
        let ext_b = find_obj(&graph, "PeerExtB");

        assert!(
            !index.object_extends(&graph, &ext_b.id, &ext_a.id),
            "sibling extensions of the same base must NEVER be considered \
             to extend each other"
        );
        assert!(!index.object_extends(&graph, &ext_a.id, &ext_b.id));
    }

    // (j) PEER-extension `Protected` BLEED — the biggest latent false-`Source`
    // this task closes: `TableExtension ExtA` declares `protected P()`;
    // `TableExtension ExtB` (sibling, extends the SAME base) calls `R.P()`.
    // ExtB extends Bar, NOT ExtA — must decline. THE pre-fix bug: same-app
    // blanket-true made this false-resolve to `Source` targeting `ExtA.P`
    // (verified against unfixed code — see the task report).
    #[test]
    fn resolve_member_record_peer_extension_protected_bleed_excluded() {
        use crate::program::resolve::receiver::ReceiverType;

        let src_table: &'static str = r#"
table 52698 "Bar"
{
}
"#;
        let src_ext_a: &'static str = r#"
tableextension 52699 "BarExtA" extends Bar
{
    protected procedure P()
    begin
    end;
}
"#;
        let src_ext_b: &'static str = r#"
tableextension 52700 "BarExtB" extends Bar
{
    procedure Wrapper()
    var
        R: Record Bar;
    begin
        R.P();
    end;
}
"#;
        let app_id = make_app_id("TestApp");
        let unit_table = make_unit(app_id.clone(), "Bar.al", src_table);
        let unit_ext_a = make_unit(app_id.clone(), "BarExtA.al", src_ext_a);
        let unit_ext_b = make_unit(app_id, "BarExtB.al", src_ext_b);
        let units = [unit_table, unit_ext_a, unit_ext_b];
        let graph = build_graph(&units, None);
        let index = ResolveIndex::build(&graph);
        let surface = DeclSurface::build(&graph, &units);

        let table_obj = find_obj(&graph, "Bar");
        let receiver = ReceiverType::Record {
            table: Some(table_obj.id.clone()),
        };
        let from_obj = find_obj(&graph, "BarExtB");
        let (shape, routes) = resolve_member(&receiver, "p", 0, from_obj, &graph, &index, &surface);

        assert_eq!(shape, DispatchShape::Exact);
        assert_eq!(routes.len(), 1);
        assert_eq!(
            routes[0].target,
            RouteTarget::Unresolved,
            "a PEER extension's `protected` method must NEVER be visible to \
             a sibling extension of the same base (ExtB extends Bar, NOT \
             ExtA) — the sibling-bleed guard; got {:?}",
            routes[0].target
        );
        assert!(matches!(routes[0].evidence, Evidence::Unknown(_)));
    }

    // (k) same-app `internal`: `Table Foo` with `internal procedure P()`; a
    // same-app but DIFFERENT `Codeunit` calls `R.P()` — `Access::Internal` is
    // APP-scoped (unaffected by this task's `local`/`protected` fixes), so
    // this must resolve to `Source` regardless of self/extension status.
    #[test]
    fn resolve_member_record_same_app_internal_method_resolves_to_source() {
        use crate::program::resolve::receiver::ReceiverType;

        let src_table: &'static str = r#"
table 52701 "Foo"
{
    internal procedure P()
    begin
    end;
}
"#;
        let src_caller: &'static str = r#"
codeunit 52702 "CallerK"
{
    procedure Test()
    var
        R: Record Foo;
    begin
        R.P();
    end;
}
"#;
        let app_id = make_app_id("TestApp");
        let unit_table = make_unit(app_id.clone(), "Foo.al", src_table);
        let unit_caller = make_unit(app_id, "CallerK.al", src_caller);
        let units = [unit_table, unit_caller];
        let graph = build_graph(&units, None);
        let index = ResolveIndex::build(&graph);
        let surface = DeclSurface::build(&graph, &units);

        let table_obj = find_obj(&graph, "Foo");
        let receiver = ReceiverType::Record {
            table: Some(table_obj.id.clone()),
        };
        let from_obj = find_obj(&graph, "CallerK");
        let (shape, routes) = resolve_member(&receiver, "p", 0, from_obj, &graph, &index, &surface);

        assert_eq!(shape, DispatchShape::Exact);
        assert_eq!(routes.len(), 1);
        assert!(
            matches!(routes[0].target, RouteTarget::Routine(_)),
            "a same-app `internal` method must resolve to Source regardless \
             of self/extension status (Internal is app-scoped); got {:?}",
            routes[0].target
        );
        assert_eq!(routes[0].evidence, Evidence::Source);
    }

    // -----------------------------------------------------------------------
    // Phase 4 Task 1 — instance-builtin resolution for Object/Enum receivers
    // -----------------------------------------------------------------------

    // Test 1: Page Object receiver + `runmodal` → entry-trigger `Run` dispatch
    // into the target's declared `OnOpenPage`, NOT a `PageInstance::runmodal`
    // Catalog route. T1.3 (deep-review-remediation plan) fix: this test used
    // to assert the very bug T1.3 closes — a declared Page-typed variable's
    // `.RunModal()` falling through to the instance-builtin catalog instead
    // of the target's own trigger chain (see `member_catalog::ENTRY_
    // DISPATCH_BUILTIN_IDS`'s doc for the two classifier gaps; this is the
    // declared-variable population, closed via `resolve_member_with_args`'s
    // broadened entry-trigger special case + shared `dispatch_entry_trigger`).
    #[test]
    fn resolve_member_page_runmodal_dispatches_to_entry_trigger() {
        use crate::program::resolve::receiver::ReceiverType;

        let src_page: &'static str = r#"
page 50610 "MyPage"
{
    trigger OnOpenPage()
    begin
    end;
}
"#;
        let src_caller: &'static str = r#"
codeunit 50611 "PageCaller"
{
    procedure Go()
    begin
    end;
}
"#;
        let app_id = make_app_id("TestApp");
        let unit_page = make_unit(app_id.clone(), "MyPage.al", src_page);
        let unit_caller = make_unit(app_id, "PageCaller.al", src_caller);
        let units = [unit_page, unit_caller];
        let graph = build_graph(&units, None);
        let index = ResolveIndex::build(&graph);
        let surface = DeclSurface::build(&graph, &units);

        let from_obj = find_obj(&graph, "PageCaller");
        let receiver = ReceiverType::Object {
            kind: ObjectKind::Page,
            name_lc: "mypage".into(),
            id: None,
        };
        let (shape, routes) =
            resolve_member(&receiver, "runmodal", 0, from_obj, &graph, &index, &surface);

        assert_eq!(shape, DispatchShape::Exact);
        assert_eq!(routes.len(), 1);
        assert_eq!(
            routes[0].evidence,
            Evidence::Source,
            "must dispatch to MyPage's declared OnOpenPage trigger, not the \
             PageInstance::runmodal Catalog builtin (the T1.3 bug); got {:?}",
            routes[0]
        );
        assert!(
            matches!(routes[0].witness, Witness::SourceSpan { .. }),
            "witness must be SourceSpan; got {:?}",
            routes[0].witness
        );
        let RouteTarget::Routine(ref rid) = routes[0].target else {
            panic!(
                "expected RouteTarget::Routine (OnOpenPage), got {:?}",
                routes[0].target
            );
        };
        assert_eq!(rid.name_lc, "onopenpage");
        assert_eq!(rid.object.kind, ObjectKind::Page);
    }

    // Test 2: Report Object receiver + `saveaspdf` → Catalog route ReportInstance::saveaspdf
    #[test]
    fn resolve_member_report_saveaspdf_emits_catalog_route() {
        use crate::program::resolve::receiver::ReceiverType;

        let src_report: &'static str = r#"
report 50612 "MyReport"
{
    dataset
    {
    }
}
"#;
        let src_caller: &'static str = r#"
codeunit 50613 "ReportCaller"
{
    procedure Go()
    begin
    end;
}
"#;
        let app_id = make_app_id("TestApp");
        let unit_report = make_unit(app_id.clone(), "MyReport.al", src_report);
        let unit_caller = make_unit(app_id, "ReportCaller.al", src_caller);
        let units = [unit_report, unit_caller];
        let graph = build_graph(&units, None);
        let index = ResolveIndex::build(&graph);
        let surface = DeclSurface::build(&graph, &units);

        let from_obj = find_obj(&graph, "ReportCaller");
        let receiver = ReceiverType::Object {
            kind: ObjectKind::Report,
            name_lc: "myreport".into(),
            id: None,
        };
        let (shape, routes) = resolve_member(
            &receiver,
            "saveaspdf",
            0,
            from_obj,
            &graph,
            &index,
            &surface,
        );

        assert_eq!(shape, DispatchShape::Exact);
        assert_eq!(routes.len(), 1);
        assert!(
            matches!(routes[0].target, RouteTarget::Builtin(_)),
            "target must be Builtin; got {:?}",
            routes[0].target
        );
        assert_eq!(routes[0].evidence, Evidence::Catalog);
        assert!(
            matches!(routes[0].witness, Witness::CatalogEntry { .. }),
            "witness must be CatalogEntry; got {:?}",
            routes[0].witness
        );
        let RouteTarget::Builtin(ref bid) = routes[0].target else {
            unreachable!()
        };
        assert_eq!(bid.0, "ReportInstance::saveaspdf");
    }

    // Test 3: EnumType receiver + `asinteger` → Catalog route Enum::asinteger
    #[test]
    fn resolve_member_enum_asinteger_emits_catalog_route() {
        use crate::program::resolve::receiver::ReceiverType;

        let (graph, index, surface, from_obj) = minimal_resolve_member_fixtures();

        let receiver = ReceiverType::EnumType {
            name_lc: "myenum".into(),
        };
        let (shape, routes) = resolve_member(
            &receiver,
            "asinteger",
            0,
            &from_obj,
            &graph,
            &index,
            &surface,
        );

        assert_eq!(shape, DispatchShape::Exact);
        assert_eq!(routes.len(), 1);
        assert!(
            matches!(routes[0].target, RouteTarget::Builtin(_)),
            "target must be Builtin; got {:?}",
            routes[0].target
        );
        assert_eq!(routes[0].evidence, Evidence::Catalog);
        assert!(
            matches!(routes[0].witness, Witness::CatalogEntry { .. }),
            "witness must be CatalogEntry; got {:?}",
            routes[0].witness
        );
        let RouteTarget::Builtin(ref bid) = routes[0].target else {
            unreachable!()
        };
        assert_eq!(bid.0, "Enum::asinteger");
    }

    // Test 4 (CORRECTED, Task 4 — receiver-closure-and-arg-increments plan,
    // the enum catalog SPLIT): `frominteger` on an EnumType VALUE receiver is
    // now UNKNOWN, not Catalog. Pre-Task-4, `FromInteger` was wrongly
    // reachable from a VALUE-instance receiver via the single undifferentiated
    // `FrameworkKind::Enum` catalog — MS Learn (`enum-data-type`) documents
    // `FromInteger` as a STATIC method only; this was the exact bug the
    // round-2 closer's "SPLIT enum catalogs" mandate fixes. The TYPE-static
    // surface test immediately below covers the (now correctly gated)
    // positive case.
    #[test]
    fn resolve_member_enum_value_frominteger_is_unknown_not_catalog() {
        use crate::program::resolve::receiver::ReceiverType;

        let (graph, index, surface, from_obj) = minimal_resolve_member_fixtures();

        let receiver = ReceiverType::EnumType {
            name_lc: "myenum".into(),
        };
        let (shape, routes) = resolve_member(
            &receiver,
            "frominteger",
            0,
            &from_obj,
            &graph,
            &index,
            &surface,
        );

        assert_eq!(shape, DispatchShape::Exact);
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].target, RouteTarget::Unresolved);
        assert_eq!(
            routes[0].evidence,
            Evidence::Unknown(UnknownReason::CatalogMiss)
        );
    }

    // Test 4b (Task 4): EnumTypeStatic receiver + `frominteger` → Catalog
    // route EnumTypeStatic::frominteger — the TYPE-static surface's real home.
    #[test]
    fn resolve_member_enum_type_static_frominteger_emits_catalog_route() {
        use crate::program::resolve::receiver::ReceiverType;

        let (graph, index, surface, from_obj) = minimal_resolve_member_fixtures();

        let receiver = ReceiverType::EnumTypeStatic {
            name_lc: "myenum".into(),
        };
        let (shape, routes) = resolve_member(
            &receiver,
            "frominteger",
            0,
            &from_obj,
            &graph,
            &index,
            &surface,
        );

        assert_eq!(shape, DispatchShape::Exact);
        assert_eq!(routes.len(), 1);
        assert!(
            matches!(routes[0].target, RouteTarget::Builtin(_)),
            "target must be Builtin; got {:?}",
            routes[0].target
        );
        assert_eq!(routes[0].evidence, Evidence::Catalog);
        let RouteTarget::Builtin(ref bid) = routes[0].target else {
            unreachable!()
        };
        assert_eq!(bid.0, "EnumTypeStatic::frominteger");
    }

    // Test 4c (Task 4): EnumTypeStatic receiver + `ordinals`/`names` also
    // resolve (real CDO shape, `Enum::"Type".Ordinals()`); `asinteger` does
    // NOT (round-2 closer, BINDING: value-surface only).
    #[test]
    fn resolve_member_enum_type_static_ordinals_names_resolve_asinteger_declines() {
        use crate::program::resolve::receiver::ReceiverType;

        let (graph, index, surface, from_obj) = minimal_resolve_member_fixtures();
        let receiver = ReceiverType::EnumTypeStatic {
            name_lc: "myenum".into(),
        };

        for member in ["ordinals", "names"] {
            let (shape, routes) =
                resolve_member(&receiver, member, 0, &from_obj, &graph, &index, &surface);
            assert_eq!(shape, DispatchShape::Exact);
            assert_eq!(routes.len(), 1);
            assert_eq!(
                routes[0].evidence,
                Evidence::Catalog,
                "{member} must resolve on the TYPE-static surface"
            );
        }

        let (shape, routes) = resolve_member(
            &receiver,
            "asinteger",
            0,
            &from_obj,
            &graph,
            &index,
            &surface,
        );
        assert_eq!(shape, DispatchShape::Exact);
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].target, RouteTarget::Unresolved);
        assert_eq!(
            routes[0].evidence,
            Evidence::Unknown(UnknownReason::CatalogMiss),
            "asinteger must NOT resolve on the TYPE-static surface"
        );
    }

    // Test 5: EnumType receiver + unknown method → Unknown
    #[test]
    fn resolve_member_enum_unknown_method_emits_unknown() {
        use crate::program::resolve::receiver::ReceiverType;

        let (graph, index, surface, from_obj) = minimal_resolve_member_fixtures();

        let receiver = ReceiverType::EnumType {
            name_lc: "myenum".into(),
        };
        let (shape, routes) = resolve_member(
            &receiver,
            "nosuchmethod",
            0,
            &from_obj,
            &graph,
            &index,
            &surface,
        );

        assert_eq!(shape, DispatchShape::Exact);
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].target, RouteTarget::Unresolved);
        assert!(matches!(routes[0].evidence, Evidence::Unknown(_)));
        assert_eq!(routes[0].witness, Witness::None);
    }

    // -----------------------------------------------------------------------
    // Task 1 (argtype-dispatch-and-page-catalog plan): Page/Report
    // instance-catalog completion. `SetTableView`/`SetRecord`/`GetRecord`/
    // `SetSelectionFilter` (Page) and `SetTableView` (Report) are REAL,
    // always-present platform intrinsics — see `is_metadata_sensitive_
    // instance_method`'s doc for the MS Learn citations and the L3
    // `PAGE_INSTANCE`/`REPORT_INSTANCE` catalog precedent
    // (`engine::l3::member_builtins`). `SaveRecord` stays excluded from the
    // general `Object{Page}` catalog fallback — it is a CurrPage-ONLY
    // intrinsic (a compiler error on any other Page-typed expression); see
    // the two `..._saverecord_..._currpage_only` tests below for the positive
    // (Framework(PageInstance), the CurrPage singleton) / negative
    // (Object{Page}, a declared Page var — INCLUDING the id-carrying subpage
    // shape) split.
    // -----------------------------------------------------------------------

    // Test 6a: Page Object receiver + `settableview` → Catalog route.
    #[test]
    fn resolve_member_page_settableview_emits_catalog_route() {
        use crate::program::resolve::receiver::ReceiverType;

        let src_page: &'static str = r#"
page 50614 "AnotherPage"
{
}
"#;
        let src_caller: &'static str = r#"
codeunit 50615 "AnotherPageCaller"
{
    procedure Go()
    begin
    end;
}
"#;
        let app_id = make_app_id("TestApp");
        let unit_page = make_unit(app_id.clone(), "AnotherPage.al", src_page);
        let unit_caller = make_unit(app_id, "AnotherPageCaller.al", src_caller);
        let units = [unit_page, unit_caller];
        let graph = build_graph(&units, None);
        let index = ResolveIndex::build(&graph);
        let surface = DeclSurface::build(&graph, &units);

        let from_obj = find_obj(&graph, "AnotherPageCaller");
        let receiver = ReceiverType::Object {
            kind: ObjectKind::Page,
            name_lc: "anotherpage".into(),
            id: None,
        };
        let (shape, routes) = resolve_member(
            &receiver,
            "settableview",
            1,
            from_obj,
            &graph,
            &index,
            &surface,
        );

        assert_eq!(shape, DispatchShape::Exact);
        assert_eq!(routes.len(), 1);
        assert!(
            matches!(routes[0].target, RouteTarget::Builtin(_)),
            "target must be Builtin (a genuine platform intrinsic); got {:?}",
            routes[0].target
        );
        assert_eq!(routes[0].evidence, Evidence::Catalog);
        let RouteTarget::Builtin(ref bid) = routes[0].target else {
            unreachable!()
        };
        assert_eq!(bid.0, "PageInstance::settableview");
    }

    // Test 6b: Page Object receiver + `setrecord` → Catalog route.
    #[test]
    fn resolve_member_page_setrecord_emits_catalog_route() {
        use crate::program::resolve::receiver::ReceiverType;

        let src_page: &'static str = r#"
page 50620 "SetRecordPage"
{
}
"#;
        let src_caller: &'static str = r#"
codeunit 50621 "SetRecordPageCaller"
{
    procedure Go()
    begin
    end;
}
"#;
        let app_id = make_app_id("TestApp");
        let unit_page = make_unit(app_id.clone(), "SetRecordPage.al", src_page);
        let unit_caller = make_unit(app_id, "SetRecordPageCaller.al", src_caller);
        let units = [unit_page, unit_caller];
        let graph = build_graph(&units, None);
        let index = ResolveIndex::build(&graph);
        let surface = DeclSurface::build(&graph, &units);

        let from_obj = find_obj(&graph, "SetRecordPageCaller");
        let receiver = ReceiverType::Object {
            kind: ObjectKind::Page,
            name_lc: "setrecordpage".into(),
            id: None,
        };
        let (shape, routes) = resolve_member(
            &receiver,
            "setrecord",
            1,
            from_obj,
            &graph,
            &index,
            &surface,
        );

        assert_eq!(shape, DispatchShape::Exact);
        assert_eq!(routes.len(), 1);
        assert!(
            matches!(routes[0].target, RouteTarget::Builtin(_)),
            "target must be Builtin; got {:?}",
            routes[0].target
        );
        assert_eq!(routes[0].evidence, Evidence::Catalog);
        let RouteTarget::Builtin(ref bid) = routes[0].target else {
            unreachable!()
        };
        assert_eq!(bid.0, "PageInstance::setrecord");
    }

    // Test 6c: Page Object receiver + `getrecord` → Catalog route (both the
    // plain declared-var shape, `id: None`, AND the id-CARRYING subpage-
    // instance shape, `id: Some(..)` — Step 0's `CurrPage.<part>.Page`
    // mechanically resolved id — must be treated identically: the exclusion
    // narrowing must not depend on how the `Object{Page}` receiver was
    // constructed, only on the METHOD name).
    #[test]
    fn resolve_member_page_getrecord_emits_catalog_route_plain_and_id_carrying() {
        use crate::program::resolve::receiver::ReceiverType;

        let src_page: &'static str = r#"
page 50622 "GetRecordPage"
{
}
"#;
        let src_caller: &'static str = r#"
codeunit 50623 "GetRecordPageCaller"
{
    procedure Go()
    begin
    end;
}
"#;
        let app_id = make_app_id("TestApp");
        let unit_page = make_unit(app_id.clone(), "GetRecordPage.al", src_page);
        let unit_caller = make_unit(app_id, "GetRecordPageCaller.al", src_caller);
        let units = [unit_page, unit_caller];
        let graph = build_graph(&units, None);
        let index = ResolveIndex::build(&graph);
        let surface = DeclSurface::build(&graph, &units);

        let from_obj = find_obj(&graph, "GetRecordPageCaller");
        let page_id = find_obj(&graph, "GetRecordPage").id.clone();

        for receiver in [
            ReceiverType::Object {
                kind: ObjectKind::Page,
                name_lc: "getrecordpage".into(),
                id: None,
            },
            ReceiverType::Object {
                kind: ObjectKind::Page,
                name_lc: "getrecordpage".into(),
                id: Some(page_id.clone()),
            },
        ] {
            let (shape, routes) = resolve_member(
                &receiver,
                "getrecord",
                1,
                from_obj,
                &graph,
                &index,
                &surface,
            );

            assert_eq!(shape, DispatchShape::Exact);
            assert_eq!(routes.len(), 1);
            assert!(
                matches!(routes[0].target, RouteTarget::Builtin(_)),
                "target must be Builtin for receiver {:?}; got {:?}",
                receiver,
                routes[0].target
            );
            assert_eq!(routes[0].evidence, Evidence::Catalog);
            let RouteTarget::Builtin(ref bid) = routes[0].target else {
                unreachable!()
            };
            assert_eq!(bid.0, "PageInstance::getrecord");
        }
    }

    // Test 6d: Page Object receiver + `setselectionfilter` → Catalog route.
    #[test]
    fn resolve_member_page_setselectionfilter_emits_catalog_route() {
        use crate::program::resolve::receiver::ReceiverType;

        let src_page: &'static str = r#"
page 50624 "SelFilterPage"
{
}
"#;
        let src_caller: &'static str = r#"
codeunit 50625 "SelFilterPageCaller"
{
    procedure Go()
    begin
    end;
}
"#;
        let app_id = make_app_id("TestApp");
        let unit_page = make_unit(app_id.clone(), "SelFilterPage.al", src_page);
        let unit_caller = make_unit(app_id, "SelFilterPageCaller.al", src_caller);
        let units = [unit_page, unit_caller];
        let graph = build_graph(&units, None);
        let index = ResolveIndex::build(&graph);
        let surface = DeclSurface::build(&graph, &units);

        let from_obj = find_obj(&graph, "SelFilterPageCaller");
        let receiver = ReceiverType::Object {
            kind: ObjectKind::Page,
            name_lc: "selfilterpage".into(),
            id: None,
        };
        let (shape, routes) = resolve_member(
            &receiver,
            "setselectionfilter",
            1,
            from_obj,
            &graph,
            &index,
            &surface,
        );

        assert_eq!(shape, DispatchShape::Exact);
        assert_eq!(routes.len(), 1);
        assert!(
            matches!(routes[0].target, RouteTarget::Builtin(_)),
            "target must be Builtin; got {:?}",
            routes[0].target
        );
        assert_eq!(routes[0].evidence, Evidence::Catalog);
        let RouteTarget::Builtin(ref bid) = routes[0].target else {
            unreachable!()
        };
        assert_eq!(bid.0, "PageInstance::setselectionfilter");
    }

    // Test 6e: Report Object receiver + `settableview` → Catalog route.
    #[test]
    fn resolve_member_report_settableview_emits_catalog_route() {
        use crate::program::resolve::receiver::ReceiverType;

        let src_report: &'static str = r#"
report 50626 "SetTableViewReport"
{
    dataset
    {
    }
}
"#;
        let src_caller: &'static str = r#"
codeunit 50627 "SetTableViewReportCaller"
{
    procedure Go()
    begin
    end;
}
"#;
        let app_id = make_app_id("TestApp");
        let unit_report = make_unit(app_id.clone(), "SetTableViewReport.al", src_report);
        let unit_caller = make_unit(app_id, "SetTableViewReportCaller.al", src_caller);
        let units = [unit_report, unit_caller];
        let graph = build_graph(&units, None);
        let index = ResolveIndex::build(&graph);
        let surface = DeclSurface::build(&graph, &units);

        let from_obj = find_obj(&graph, "SetTableViewReportCaller");
        let receiver = ReceiverType::Object {
            kind: ObjectKind::Report,
            name_lc: "settableviewreport".into(),
            id: None,
        };
        let (shape, routes) = resolve_member(
            &receiver,
            "settableview",
            1,
            from_obj,
            &graph,
            &index,
            &surface,
        );

        assert_eq!(shape, DispatchShape::Exact);
        assert_eq!(routes.len(), 1);
        assert!(
            matches!(routes[0].target, RouteTarget::Builtin(_)),
            "target must be Builtin; got {:?}",
            routes[0].target
        );
        assert_eq!(routes[0].evidence, Evidence::Catalog);
        let RouteTarget::Builtin(ref bid) = routes[0].target else {
            unreachable!()
        };
        assert_eq!(bid.0, "ReportInstance::settableview");
    }

    // Test 6f (NEGATIVE/CONTROL, round-2 closer I8): `SaveRecord` on a
    // declared Page-typed VARIABLE — `Object{Page}` — is a compiler error in
    // real AL (SaveRecord only exists on the CurrPage context) and MUST stay
    // `Unknown`. Covers BOTH the plain shape (`id: None`, an ordinary
    // declared var) and the id-CARRYING shape (`id: Some(..)`, mechanically
    // identical to what Step 0's `CurrPage.<part>.Page` subpage-instance
    // receiver produces) — proving the exclusion is keyed on the METHOD
    // name alone, never on whether a page identity happens to be known. A
    // future implementation that tried to special-case "id is Some ⇒ this
    // must be CurrPage" would be exactly the false inference (gemini
    // round-2) this fixture forbids: a subpage instance is NOT CurrPage
    // either, so both `id` shapes must decline identically.
    #[test]
    fn resolve_member_page_saverecord_stays_unknown_currpage_only() {
        use crate::program::resolve::receiver::ReceiverType;

        let src_page: &'static str = r#"
page 50628 "SaveRecordPage"
{
}
"#;
        let src_caller: &'static str = r#"
codeunit 50629 "SaveRecordPageCaller"
{
    procedure Go()
    begin
    end;
}
"#;
        let app_id = make_app_id("TestApp");
        let unit_page = make_unit(app_id.clone(), "SaveRecordPage.al", src_page);
        let unit_caller = make_unit(app_id, "SaveRecordPageCaller.al", src_caller);
        let units = [unit_page, unit_caller];
        let graph = build_graph(&units, None);
        let index = ResolveIndex::build(&graph);
        let surface = DeclSurface::build(&graph, &units);

        let from_obj = find_obj(&graph, "SaveRecordPageCaller");
        let page_id = find_obj(&graph, "SaveRecordPage").id.clone();

        for receiver in [
            ReceiverType::Object {
                kind: ObjectKind::Page,
                name_lc: "saverecordpage".into(),
                id: None,
            },
            ReceiverType::Object {
                kind: ObjectKind::Page,
                name_lc: "saverecordpage".into(),
                id: Some(page_id),
            },
        ] {
            let (shape, routes) = resolve_member(
                &receiver,
                "saverecord",
                0,
                from_obj,
                &graph,
                &index,
                &surface,
            );

            assert_eq!(shape, DispatchShape::Exact);
            assert_eq!(routes.len(), 1);
            assert_eq!(
                routes[0].target,
                RouteTarget::Unresolved,
                "a Page-VARIABLE receiver (never CurrPage itself) must never \
                 get SaveRecord, regardless of a carried id; receiver={receiver:?}"
            );
            assert!(matches!(routes[0].evidence, Evidence::Unknown(_)));
            assert_eq!(routes[0].witness, Witness::None);
        }
    }

    // Test 6g (POSITIVE, round-2 closer I8): `CurrPage.SaveRecord()` — the
    // literal CurrPage singleton, `ReceiverType::Framework(PageInstance)` per
    // `infer_receiver_type`'s Step 1 — resolves via the unconditional
    // Framework-arm catalog lookup (`resolve_member`'s `Framework(kind)` arm
    // never consulted `is_metadata_sensitive_instance_method` — only the
    // `Object{kind}` arm's fallback did), so this passes BEFORE this task's
    // code change too. Pinned here as the explicit CurrPage-origin positive
    // counterpart to Test 6f's negative: the two tests together prove the
    // origin distinction is STRUCTURAL (a different `ReceiverType` variant
    // entirely — `Framework(PageInstance)` vs `Object{Page}` — never a flag
    // inferred from a resolved page id).
    #[test]
    fn resolve_member_framework_pageinstance_saverecord_emits_catalog_route() {
        use crate::program::resolve::receiver::{FrameworkKind, ReceiverType};

        let (graph, index, surface, from_obj) = minimal_resolve_member_fixtures();

        let receiver = ReceiverType::Framework(FrameworkKind::PageInstance);
        let (shape, routes) = resolve_member(
            &receiver,
            "saverecord",
            0,
            &from_obj,
            &graph,
            &index,
            &surface,
        );

        assert_eq!(shape, DispatchShape::Exact);
        assert_eq!(routes.len(), 1);
        assert!(
            matches!(routes[0].target, RouteTarget::Builtin(_)),
            "CurrPage.SaveRecord() must resolve via the Framework catalog; got {:?}",
            routes[0].target
        );
        assert_eq!(routes[0].evidence, Evidence::Catalog);
        let RouteTarget::Builtin(ref bid) = routes[0].target else {
            unreachable!()
        };
        assert_eq!(bid.0, "PageInstance::saverecord");
    }

    // Task 2 blast-radius regression guard: `CurrPage.Update()` — the
    // QUALIFIED/receiver-explicit form of the SAME name
    // (`INSTANCE_ONLY_NEVER_BARE` suppresses only the UNQUALIFIED bare-call
    // path in `resolve_bare`; `resolve_member`'s `Framework(kind)` arm never
    // touches `global_builtin_id`/`is_bare_builtin_or_page_intrinsic` at
    // all, so this must resolve identically before and after Task 2).
    // `Update` is the round-1 addenda's explicitly-flagged "riskiest" name
    // (the ubiquitous `CurrPage.Update(...)` idiom) — pinned here as its own
    // dedicated regression, not just inferred from `SaveRecord`'s sibling
    // test above.
    #[test]
    fn resolve_member_framework_pageinstance_update_emits_catalog_route() {
        use crate::program::resolve::receiver::{FrameworkKind, ReceiverType};

        let (graph, index, surface, from_obj) = minimal_resolve_member_fixtures();

        let receiver = ReceiverType::Framework(FrameworkKind::PageInstance);
        let (shape, routes) =
            resolve_member(&receiver, "update", 0, &from_obj, &graph, &index, &surface);

        assert_eq!(shape, DispatchShape::Exact);
        assert_eq!(routes.len(), 1);
        assert!(
            matches!(routes[0].target, RouteTarget::Builtin(_)),
            "CurrPage.Update() must still resolve via the Framework catalog \
             after Task 2's bare-call-only suppression; got {:?}",
            routes[0].target
        );
        assert_eq!(routes[0].evidence, Evidence::Catalog);
        let RouteTarget::Builtin(ref bid) = routes[0].target else {
            unreachable!()
        };
        assert_eq!(bid.0, "PageInstance::update");
    }

    // Test 7: Page Object receiver + declared proc (shadows catalog) → Source route.
    // Uses `Close` rather than `Run`/`RunModal` (T1.3, deep-review-remediation
    // plan): `Run`/`RunModal` are now special-cased (both Codeunit AND
    // Page/Report) to dispatch UNCONDITIONALLY to the target's entry trigger,
    // bypassing `resolve_in_object`/`resolve_in_page_scope` (and any declared
    // same-named procedure) entirely BY DESIGN — see the companion test
    // `resolve_member_page_declared_runmodal_proc_does_not_shadow_entry_
    // trigger` below and `resolve_member_object_run_arm_*`'s doc for the
    // pre-existing Codeunit precedent this now mirrors. `Close` is an
    // ordinary `PageInstance` catalog entry with no entry-trigger special
    // case, so it still exercises the general source-shadows-catalog
    // precedence this test was written to prove.
    #[test]
    fn resolve_member_page_declared_proc_shadows_catalog() {
        use crate::program::resolve::receiver::ReceiverType;

        // This page declares its own Close — should shadow the PageInstance catalog entry.
        let src_page: &'static str = r#"
page 50616 "PageWithClose"
{
    procedure Close()
    begin
    end;
}
"#;
        let src_caller: &'static str = r#"
codeunit 50617 "ShadowCaller"
{
    procedure Go()
    begin
    end;
}
"#;
        let app_id = make_app_id("TestApp");
        let unit_page = make_unit(app_id.clone(), "PageWithClose.al", src_page);
        let unit_caller = make_unit(app_id, "ShadowCaller.al", src_caller);
        let units = [unit_page, unit_caller];
        let graph = build_graph(&units, None);
        let index = ResolveIndex::build(&graph);
        let surface = DeclSurface::build(&graph, &units);

        let from_obj = find_obj(&graph, "ShadowCaller");
        let receiver = ReceiverType::Object {
            kind: ObjectKind::Page,
            name_lc: "pagewithclose".into(),
            id: None,
        };
        let (shape, routes) =
            resolve_member(&receiver, "close", 0, from_obj, &graph, &index, &surface);

        assert_eq!(shape, DispatchShape::Exact);
        assert_eq!(routes.len(), 1);
        assert!(
            matches!(routes[0].target, RouteTarget::Routine(_)),
            "declared proc must shadow catalog; target must be Routine, got {:?}",
            routes[0].target
        );
        assert_eq!(routes[0].evidence, Evidence::Source);
        assert!(matches!(routes[0].witness, Witness::SourceSpan { .. }));
    }

    /// T1.3 (deep-review-remediation plan) parity proof: a Page that declares
    /// its OWN `procedure RunModal()` does NOT shadow the entry-trigger
    /// dispatch — `RunModal` (like Codeunit's `Run`) is special-cased to
    /// bypass `resolve_in_page_scope` (and therefore any declared same-named
    /// procedure) entirely, exactly mirroring `resolve_member_object_run_
    /// arm_resolves_unmarked_entry_trigger_normally`'s pre-existing Codeunit
    /// precedent. `PageWithRunModal` declares no `OnOpenPage` trigger, so
    /// this also proves the Opaque-boundary-when-absent fallback fires for
    /// the declared-variable population, not just a Source hit.
    #[test]
    fn resolve_member_page_declared_runmodal_proc_does_not_shadow_entry_trigger() {
        use crate::program::resolve::receiver::ReceiverType;

        let src_page: &'static str = r#"
page 50616 "PageWithRunModal"
{
    procedure RunModal()
    begin
    end;
}
"#;
        let src_caller: &'static str = r#"
codeunit 50617 "ShadowCaller"
{
    procedure Go()
    begin
    end;
}
"#;
        let app_id = make_app_id("TestApp");
        let unit_page = make_unit(app_id.clone(), "PageWithRunModal.al", src_page);
        let unit_caller = make_unit(app_id, "ShadowCaller.al", src_caller);
        let units = [unit_page, unit_caller];
        let graph = build_graph(&units, None);
        let index = ResolveIndex::build(&graph);
        let surface = DeclSurface::build(&graph, &units);

        let from_obj = find_obj(&graph, "ShadowCaller");
        let receiver = ReceiverType::Object {
            kind: ObjectKind::Page,
            name_lc: "pagewithrunmodal".into(),
            id: None,
        };
        let (shape, routes) =
            resolve_member(&receiver, "runmodal", 0, from_obj, &graph, &index, &surface);

        assert_eq!(shape, DispatchShape::Exact);
        assert_eq!(routes.len(), 1);
        assert_eq!(
            routes[0].evidence,
            Evidence::Opaque,
            "the declared RunModal PROCEDURE must never be selected — RunModal \
             dispatches to OnOpenPage unconditionally, which is absent here; \
             got {:?}",
            routes[0]
        );
        assert!(
            matches!(routes[0].target, RouteTarget::AbiSymbol { .. }),
            "expected an Opaque AbiSymbol boundary (OnOpenPage not indexed); \
             got {:?}",
            routes[0].target
        );
    }

    // -----------------------------------------------------------------------
    // Phase 4 Task 2 — Interface Polymorphic fan-out tests
    // -----------------------------------------------------------------------

    /// Two codeunits both implementing IFoo, each with a unique Bar() (arity 0).
    /// Expect: Polymorphic shape, two Routine routes (Source evidence), each
    /// passing `interface_route_applicable`.
    #[test]
    fn resolve_member_interface_two_implementers_emits_two_routine_routes() {
        use crate::program::resolve::applicability::interface_route_applicable;
        use crate::program::resolve::receiver::ReceiverType;

        // Two implementers + a caller; all in one file for simplicity.
        let src: &'static str = r#"
codeunit 51300 "IFooImpl1" implements IFoo
{
    procedure Bar()
    begin
    end;
}

codeunit 51301 "IFooImpl2" implements IFoo
{
    procedure Bar()
    begin
    end;
}

codeunit 51399 "IfaceCaller1"
{
    procedure Trigger()
    begin
    end;
}
"#;
        let app_id = make_app_id("TestApp");
        let unit = make_unit(app_id, "IfaceTwo.al", src);
        let units = [unit];
        let graph = build_graph(&units, None);
        let index = ResolveIndex::build(&graph);
        let surface = DeclSurface::build(&graph, &units);

        let from_obj = find_obj(&graph, "IfaceCaller1");
        let receiver = ReceiverType::Interface {
            name_lc: "ifoo".into(),
        };
        let (shape, routes) =
            resolve_member(&receiver, "bar", 0, from_obj, &graph, &index, &surface);

        assert_eq!(
            shape,
            DispatchShape::Polymorphic,
            "shape must be Polymorphic"
        );
        assert_eq!(
            routes.len(),
            2,
            "two implementers → two routes; got {:?}",
            routes
        );

        // Both routes must be Routine (unique Bar() in each implementer).
        for r in &routes {
            assert!(
                matches!(r.target, RouteTarget::Routine(_)),
                "each implementer's Bar() route must be Routine; got {:?}",
                r.target
            );
            assert_eq!(r.evidence, Evidence::Source, "evidence must be Source");
        }

        // Each Routine must pass interface_route_applicable.
        for r in &routes {
            let RouteTarget::Routine(ref rid) = r.target else {
                unreachable!()
            };
            assert!(
                interface_route_applicable("ifoo", "bar", 0, rid, &graph, &index),
                "Routine route must pass interface_route_applicable; rid={:?}",
                rid
            );
        }
    }

    /// ONE implementer → exactly 1 Routine route.
    #[test]
    fn resolve_member_interface_one_implementer_emits_one_route() {
        use crate::program::resolve::receiver::ReceiverType;

        let src: &'static str = r#"
codeunit 51400 "OnlyImpl" implements IFoo
{
    procedure Bar()
    begin
    end;
}

codeunit 51499 "IfaceCaller2"
{
    procedure Trigger()
    begin
    end;
}
"#;
        let app_id = make_app_id("TestApp");
        let unit = make_unit(app_id, "IfaceOne.al", src);
        let units = [unit];
        let graph = build_graph(&units, None);
        let index = ResolveIndex::build(&graph);
        let surface = DeclSurface::build(&graph, &units);

        let from_obj = find_obj(&graph, "IfaceCaller2");
        let receiver = ReceiverType::Interface {
            name_lc: "ifoo".into(),
        };
        let (shape, routes) =
            resolve_member(&receiver, "bar", 0, from_obj, &graph, &index, &surface);

        assert_eq!(shape, DispatchShape::Polymorphic);
        assert_eq!(
            routes.len(),
            1,
            "one implementer → one route; got {:?}",
            routes
        );
        assert!(
            matches!(routes[0].target, RouteTarget::Routine(_)),
            "must be Routine route; got {:?}",
            routes[0].target
        );
        assert_eq!(routes[0].evidence, Evidence::Source);
    }

    /// Task 4 fixture (e), round-1 addendum "T4 — interface nesting OUT OF
    /// SCOPE" (BINDING): an implementer with its OWN same-object overload
    /// ambiguity (`Bar(p: Integer)` / `Bar(p: Text)`, both `Public`) inside an
    /// Interface Polymorphic fan-out must NOT extend that nested candidate
    /// set into the edge — the ambiguous implementer contributes exactly ONE
    /// `Unresolved(OverloadAmbiguous)` route (the pre-Task-4 shape), never
    /// `AmbiguousResolved`/`Complete`, and the edge's overall shape stays
    /// `Polymorphic` with exactly `implementers.len()` routes (2, not 3).
    #[test]
    fn resolve_member_interface_implementer_own_overload_ambiguity_stays_nested_unresolved() {
        use crate::program::resolve::receiver::ReceiverType;

        let src: &'static str = r#"
codeunit 51410 "IFooAmbigImpl" implements IFoo
{
    procedure Bar(p: Integer)
    begin
    end;

    procedure Bar(p: Text)
    begin
    end;
}

codeunit 51411 "IFooCleanImpl" implements IFoo
{
    procedure Bar(p: Integer)
    begin
    end;
}

codeunit 51499 "IfaceNestedCaller"
{
    procedure Trigger()
    begin
    end;
}
"#;
        let app_id = make_app_id("TestApp");
        let unit = make_unit(app_id, "IfaceNested.al", src);
        let units = [unit];
        let graph = build_graph(&units, None);
        let index = ResolveIndex::build(&graph);
        let surface = DeclSurface::build(&graph, &units);

        // Sanity: the ambiguous implementer genuinely has TWO same-arity
        // `Bar` candidates.
        let ambig_obj = find_obj(&graph, "IFooAmbigImpl");
        let bar_candidates = index.routines_in_object(&ambig_obj.id, "bar");
        assert_eq!(
            bar_candidates.len(),
            2,
            "fixture must produce TWO Bar candidates"
        );

        let from_obj = find_obj(&graph, "IfaceNestedCaller");
        let receiver = ReceiverType::Interface {
            name_lc: "ifoo".into(),
        };
        let (shape, routes) =
            resolve_member(&receiver, "bar", 1, from_obj, &graph, &index, &surface);

        assert_eq!(
            shape,
            DispatchShape::Polymorphic,
            "the OUTER edge stays Polymorphic — nesting must never corrupt \
             the fan-out's own shape"
        );
        assert_eq!(
            routes.len(),
            2,
            "exactly ONE route per implementer (2 implementers) — the \
             ambiguous implementer's OWN 2-candidate set must NOT extend \
             this vec to 3; got {routes:?}"
        );

        let unresolved_count = routes
            .iter()
            .filter(|r| r.target == RouteTarget::Unresolved)
            .count();
        assert_eq!(
            unresolved_count, 1,
            "exactly one route (the ambiguous implementer's) must be \
             Unresolved; got {routes:?}"
        );
        let ambiguous_route = routes
            .iter()
            .find(|r| r.target == RouteTarget::Unresolved)
            .expect("one Unresolved route must exist");
        assert_eq!(
            ambiguous_route.evidence,
            Evidence::Unknown(UnknownReason::OverloadAmbiguous),
            "got {:?}",
            ambiguous_route.evidence
        );
        assert!(
            !ambiguous_route
                .conditions
                .contains(&Condition::AmbiguousDispatch),
            "the collapsed nested-ambiguity route must NEVER carry \
             AmbiguousDispatch (that would misrepresent it as a live \
             AmbiguousResolved candidate); got {ambiguous_route:?}"
        );

        let resolved_count = routes
            .iter()
            .filter(|r| matches!(r.target, RouteTarget::Routine(_)))
            .count();
        assert_eq!(
            resolved_count, 1,
            "the clean implementer must still resolve normally; got {routes:?}"
        );

        // The edge-level classification must NEVER be AmbiguousResolved for
        // this shape (it is Polymorphic, not AmbiguousOverload, so
        // `classify_obligation` cannot take that branch regardless — this
        // assertion documents the invariant explicitly).
        let edge = Edge {
            from: bar_candidates[0].clone(),
            site: SiteId {
                caller: bar_candidates[0].clone(),
                span: CanonicalSpan {
                    unit: "IfaceNestedCaller.al".into(),
                    start: SourcePos { line: 1, col: 1 },
                    end: SourcePos { line: 1, col: 1 },
                },
                callee_fingerprint: 0,
            },
            kind: EdgeKind::Call,
            shape,
            completeness: SetCompleteness::Partial {
                reason: OpenWorldReason::ReverseDependentImplementers,
            },
            routes: routes.clone(),
        };
        assert_ne!(
            classify_obligation(&edge),
            ObligationOutcome::AmbiguousResolved,
            "nesting must never produce AmbiguousResolved"
        );
    }

    /// ZERO implementers → empty route vec (HonestEmpty).
    #[test]
    fn resolve_member_interface_zero_implementers_emits_empty_routes() {
        use crate::program::resolve::receiver::ReceiverType;

        // NobodyImpl does NOT implement IFoo.
        let src: &'static str = r#"
codeunit 51500 "NobodyImpl"
{
    procedure Bar()
    begin
    end;
}

codeunit 51599 "IfaceCaller3"
{
    procedure Trigger()
    begin
    end;
}
"#;
        let app_id = make_app_id("TestApp");
        let unit = make_unit(app_id, "IfaceZero.al", src);
        let units = [unit];
        let graph = build_graph(&units, None);
        let index = ResolveIndex::build(&graph);
        let surface = DeclSurface::build(&graph, &units);

        let from_obj = find_obj(&graph, "IfaceCaller3");
        let receiver = ReceiverType::Interface {
            name_lc: "ifoo".into(),
        };
        let (shape, routes) =
            resolve_member(&receiver, "bar", 0, from_obj, &graph, &index, &surface);

        assert_eq!(
            shape,
            DispatchShape::Polymorphic,
            "shape must be Polymorphic even with zero implementers"
        );
        assert!(
            routes.is_empty(),
            "zero implementers → empty routes (HonestEmpty); got {:?}",
            routes
        );
    }

    /// A resolves Bar(), B has Bar(x: Integer) (arity mismatch for arity-0 call) →
    /// `[Routine(A), Unresolved(B)]`.  B MUST emit Unresolved and NOT be dropped
    /// (Rule 1: no reachability black hole).
    #[test]
    fn resolve_member_interface_failing_implementer_emits_unresolved_not_dropped() {
        use crate::program::resolve::receiver::ReceiverType;

        // FooImpl1 has Bar() (0 params) — unique, resolves OK.
        // FooImpl2 has Bar(x: Integer) (1 param) — arity mismatch for Bar(0) → Unresolved.
        let src: &'static str = r#"
codeunit 51600 "FooImpl1" implements IFoo
{
    procedure Bar()
    begin
    end;
}

codeunit 51601 "FooImpl2" implements IFoo
{
    procedure Bar(x: Integer)
    begin
    end;
}

codeunit 51699 "IfaceCaller4"
{
    procedure Trigger()
    begin
    end;
}
"#;
        let app_id = make_app_id("TestApp");
        let unit = make_unit(app_id, "IfaceFail.al", src);
        let units = [unit];
        let graph = build_graph(&units, None);
        let index = ResolveIndex::build(&graph);
        let surface = DeclSurface::build(&graph, &units);

        let from_obj = find_obj(&graph, "IfaceCaller4");
        let receiver = ReceiverType::Interface {
            name_lc: "ifoo".into(),
        };
        let (shape, routes) =
            resolve_member(&receiver, "bar", 0, from_obj, &graph, &index, &surface);

        assert_eq!(shape, DispatchShape::Polymorphic);
        assert_eq!(
            routes.len(),
            2,
            "two implementers → two routes (B must NOT be silently dropped); got {:?}",
            routes
        );

        // One route must be Routine (A), one must be Unresolved (B).
        let routine_count = routes
            .iter()
            .filter(|r| matches!(r.target, RouteTarget::Routine(_)))
            .count();
        let unresolved_count = routes
            .iter()
            .filter(|r| r.target == RouteTarget::Unresolved)
            .count();
        assert_eq!(
            routine_count, 1,
            "exactly one Routine route (A); got {:?}",
            routes
        );
        assert_eq!(
            unresolved_count, 1,
            "exactly one Unresolved route (B, NOT dropped); got {:?}",
            routes
        );

        // The Unresolved route must have Unknown evidence and None witness.
        let unresolved_route = routes
            .iter()
            .find(|r| r.target == RouteTarget::Unresolved)
            .unwrap();
        assert!(matches!(unresolved_route.evidence, Evidence::Unknown(_)));
        assert_eq!(unresolved_route.witness, Witness::None);
    }

    /// SymbolOnly interface implementer → `AbiSymbol` route (Opaque evidence, not Unresolved).
    ///
    /// Validates the Phase-4 Task-2 fix: when a cross-app `.app` dependency implements an
    /// interface (SymbolOnly tier, no source body available), `resolve_member` must emit
    /// `RouteTarget::AbiSymbol` (Opaque boundary), not `Unresolved`.
    ///
    /// This test also directly validates the per-route gate predicate used in
    /// `run_member_resolution_harness` FreshOnly branch: `RouteTarget::AbiSymbol { .. }`
    /// must be classified as PASS (not `unverified_extra`).  The gate-gap fix adds
    /// `AbiSymbol { .. } => true` alongside the existing `Unresolved => true` arm.
    #[test]
    fn resolve_member_interface_symbol_only_implementer_emits_abi_symbol_route() {
        use crate::program::resolve::receiver::ReceiverType;

        // AppBSym provides a SymbolOnly codeunit "DepImpl" implementing IFoo.
        // Parsed from source to populate graph+index nodes, but the DeclSurface is built
        // WITHOUT this unit (empty parsed slice) — mirrors real SymbolOnly loading from
        // .app SymbolReference where bodies are unavailable.
        let src_caller: &'static str = r#"
codeunit 51800 "IfaceCallerSym"
{
    procedure Trigger()
    begin
    end;
}
"#;
        let src_dep: &'static str = r#"
codeunit 51801 "DepImpl" implements IFoo
{
    procedure Bar()
    begin
    end;
}
"#;
        let app_a_id = make_app_id("AppA");
        let app_b_id = make_app_id("AppBSym");

        let unit_caller = make_unit(app_a_id.clone(), "IfaceCallerSym.al", src_caller);
        // SymbolOnly dep unit: parsed to extract graph/index nodes with SymbolOnly tier.
        let unit_dep = ParsedUnit {
            app: app_b_id.clone(),
            files: vec![ParsedFile {
                virtual_path: "DepImpl.al".to_string(),
                file: std::sync::Arc::new(al_syntax::parse(src_dep)),
                provenance: Provenance {
                    app: app_b_id,
                    tier: TrustTier::SymbolOnly,
                    content_hash: String::new(),
                },
                text: src_dep.into(),
            }],
        };

        let all_units = [unit_caller, unit_dep];
        let graph = build_graph(&all_units, Some(("AppA", "AppBSym")));
        let index = ResolveIndex::build(&graph);
        // DeclSurface: empty — SymbolOnly routines have no parsed body in production.
        // A DeclSurface miss on a SymbolOnly-tier routine triggers the AbiSymbol path
        // in `make_routine_route`.
        let surface = DeclSurface::build(&graph, &[]);

        let from_obj = find_obj(&graph, "IfaceCallerSym");
        let receiver = ReceiverType::Interface {
            name_lc: "ifoo".into(),
        };
        let (shape, routes) =
            resolve_member(&receiver, "bar", 0, from_obj, &graph, &index, &surface);

        assert_eq!(shape, DispatchShape::Polymorphic);
        assert_eq!(
            routes.len(),
            1,
            "one SymbolOnly implementer → one route; got {:?}",
            routes
        );

        // Route must be AbiSymbol (Opaque boundary), NOT Unresolved or Routine.
        assert!(
            matches!(routes[0].target, RouteTarget::AbiSymbol { .. }),
            "SymbolOnly implementer must emit AbiSymbol (Opaque boundary); got {:?}",
            routes[0].target
        );
        assert_eq!(
            routes[0].evidence,
            Evidence::Opaque,
            "AbiSymbol route must carry Opaque evidence"
        );
        assert!(
            matches!(routes[0].witness, Witness::AbiSymbol { .. }),
            "AbiSymbol route must carry AbiSymbol witness; got {:?}",
            routes[0].witness
        );

        // Validate the per-route gate predicate (mirror of the fix in differential.rs).
        // The FreshOnly `is_interface_route` branch in `run_member_resolution_harness`
        // classifies each route as: Unresolved → PASS, Routine → applicability check,
        // AbiSymbol → PASS (the fix), _ → FAIL.
        let gate_pass = routes.iter().all(|r| match &r.target {
            RouteTarget::Unresolved => true,
            RouteTarget::AbiSymbol { .. } => true, // gate fix: was `_ => false` before
            _ => false,
        });
        assert!(
            gate_pass,
            "AbiSymbol route must pass the interface FreshOnly gate (not unverified_extra)"
        );
    }

    // Test 8: Page Object receiver + method not in catalog and not declared → Unknown
    #[test]
    fn resolve_member_page_method_not_in_catalog_emits_unknown() {
        use crate::program::resolve::receiver::ReceiverType;

        let src_page: &'static str = r#"
page 50618 "EmptyPage"
{
}
"#;
        let src_caller: &'static str = r#"
codeunit 50619 "EmptyPageCaller"
{
    procedure Go()
    begin
    end;
}
"#;
        let app_id = make_app_id("TestApp");
        let unit_page = make_unit(app_id.clone(), "EmptyPage.al", src_page);
        let unit_caller = make_unit(app_id, "EmptyPageCaller.al", src_caller);
        let units = [unit_page, unit_caller];
        let graph = build_graph(&units, None);
        let index = ResolveIndex::build(&graph);
        let surface = DeclSurface::build(&graph, &units);

        let from_obj = find_obj(&graph, "EmptyPageCaller");
        let receiver = ReceiverType::Object {
            kind: ObjectKind::Page,
            name_lc: "emptypage".into(),
            id: None,
        };
        let (shape, routes) = resolve_member(
            &receiver,
            "nosuchpagemethod",
            0,
            from_obj,
            &graph,
            &index,
            &surface,
        );

        assert_eq!(shape, DispatchShape::Exact);
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].target, RouteTarget::Unresolved);
        assert!(matches!(routes[0].evidence, Evidence::Unknown(_)));
        assert_eq!(routes[0].witness, Witness::None);
    }

    // -----------------------------------------------------------------------
    // Phase 4b Task 3 — emit_event_flow_edges tests
    // -----------------------------------------------------------------------

    /// Build a publisher + Manual subscriber graph from real AL source.
    /// Returns `(graph, units)` with both objects in one `"TestApp"` app.
    fn build_event_flow_fixture_manual() -> (ProgramGraph, Vec<ParsedUnit>) {
        let pub_src: &'static str = r#"
codeunit 50700 "EvtPub"
{
    [IntegrationEvent(false, false)]
    procedure OnAfterX()
    begin
    end;
}
"#;
        let sub_src: &'static str = r#"
codeunit 50701 "EvtManualSub"
{
    EventSubscriberInstance = Manual;

    [EventSubscriber(ObjectType::Codeunit, Codeunit::"EvtPub", 'OnAfterX', '', false, false)]
    local procedure OnAfterXHandler()
    begin
    end;
}
"#;
        let app_id = make_app_id("TestApp");
        let unit_pub = make_unit(app_id.clone(), "EvtPub.al", pub_src);
        let unit_sub = make_unit(app_id, "EvtManualSub.al", sub_src);
        let units = vec![unit_pub, unit_sub];
        let graph = build_graph(&units, None);
        (graph, units)
    }

    // (a) publisher OnAfterX + Manual subscriber → ONE EventFlow Edge with ManualBinding
    #[test]
    fn event_flow_manual_subscriber_emits_correct_edge() {
        let (graph, units) = build_event_flow_fixture_manual();
        let index = ResolveIndex::build(&graph);
        let surface = DeclSurface::build(&graph, &units);

        let edges = emit_event_flow_edges(&graph, &index, &surface);

        // Must produce exactly ONE EventFlow edge (for OnAfterX publisher).
        let event_edges: Vec<&Edge> = edges
            .iter()
            .filter(|e| e.kind == EdgeKind::EventFlow)
            .collect();
        assert_eq!(event_edges.len(), 1, "expected exactly one EventFlow edge");

        let e = event_edges[0];

        // Publisher is the `from`.
        let pub_obj = graph
            .objects
            .iter()
            .find(|o| o.name == "EvtPub")
            .expect("EvtPub object");
        let expected_pub_rid = graph
            .routines
            .iter()
            .find(|r| r.id.object == pub_obj.id && r.id.name_lc == "onafterx")
            .expect("OnAfterX publisher routine")
            .id
            .clone();
        assert_eq!(e.from, expected_pub_rid, "edge must be from the publisher");

        // Edge shape.
        assert_eq!(e.kind, EdgeKind::EventFlow);
        assert_eq!(e.shape, DispatchShape::Multicast);
        assert_eq!(
            e.completeness,
            SetCompleteness::Partial {
                reason: OpenWorldReason::ReverseDependentSubscribers
            }
        );

        // Exactly one route — the Manual subscriber.
        assert_eq!(e.routes.len(), 1, "one subscriber → one route");
        let r = &e.routes[0];

        // Route target is the subscriber Routine.
        let sub_obj = graph
            .objects
            .iter()
            .find(|o| o.name == "EvtManualSub")
            .expect("EvtManualSub object");
        let expected_sub_rid = graph
            .routines
            .iter()
            .find(|r| r.id.object == sub_obj.id && r.id.name_lc == "onafterxhandler")
            .expect("OnAfterXHandler subscriber routine")
            .id
            .clone();
        assert_eq!(
            r.target,
            RouteTarget::Routine(expected_sub_rid),
            "route target must be the subscriber"
        );

        // Route has ManualBinding condition.
        assert!(
            r.conditions.contains(&Condition::ManualBinding),
            "Manual subscriber must carry ManualBinding condition; got {:?}",
            r.conditions
        );

        // Subscriber is in the surface → Source evidence + SourceSpan witness.
        assert_eq!(r.evidence, Evidence::Source);
        assert!(
            matches!(r.witness, Witness::SourceSpan { .. }),
            "witness must be SourceSpan (subscriber in surface); got {:?}",
            r.witness
        );
    }

    // (b) Manual route NOT in default_reachable_routes, IS in may_reachable_routes
    #[test]
    fn event_flow_manual_route_excluded_from_default_reachable() {
        let (graph, units) = build_event_flow_fixture_manual();
        let index = ResolveIndex::build(&graph);
        let surface = DeclSurface::build(&graph, &units);

        let edges = emit_event_flow_edges(&graph, &index, &surface);
        let e = edges
            .iter()
            .find(|e| e.kind == EdgeKind::EventFlow)
            .expect("EventFlow edge");

        // (b) Manual route must NOT appear in default_reachable_routes (reachability contract).
        let default_routes: Vec<_> = e.default_reachable_routes().collect();
        assert!(
            default_routes.is_empty(),
            "ManualBinding route must NOT be in default_reachable_routes; got {:?}",
            default_routes
        );

        // But it MUST appear in may_reachable_routes.
        let may_routes: Vec<_> = e.may_reachable_routes().collect();
        assert_eq!(
            may_routes.len(),
            1,
            "ManualBinding route MUST be in may_reachable_routes"
        );
    }

    // (c) publisher with ZERO subscribers → empty routes → HonestEmpty
    #[test]
    fn event_flow_zero_subscribers_honest_empty() {
        let pub_src: &'static str = r#"
codeunit 50702 "NoSubPub"
{
    [IntegrationEvent(false, false)]
    procedure OnAfterY()
    begin
    end;
}
"#;
        let app_id = make_app_id("TestApp");
        let unit_pub = make_unit(app_id, "NoSubPub.al", pub_src);
        let units = vec![unit_pub];
        let graph = build_graph(&units, None);
        let index = ResolveIndex::build(&graph);
        let surface = DeclSurface::build(&graph, &units);

        let edges = emit_event_flow_edges(&graph, &index, &surface);

        let event_edges: Vec<&Edge> = edges
            .iter()
            .filter(|e| e.kind == EdgeKind::EventFlow)
            .collect();
        assert_eq!(
            event_edges.len(),
            1,
            "publisher must always produce an EventFlow edge"
        );

        let e = event_edges[0];
        assert!(
            e.routes.is_empty(),
            "zero subscribers → empty routes; got {:?}",
            e.routes
        );
        assert_eq!(
            classify_obligation(e),
            ObligationOutcome::HonestEmpty,
            "empty Multicast + Partial → HonestEmpty"
        );
    }

    // (c2) subscriber to a platform TABLE event (OnAfterDeleteEvent) wires via a
    // synthetic `PublisherKind::Platform` publisher injected on the table — the
    // fix for orphaned auto-event subscribers (data-is-control-flow wiring).
    #[test]
    fn platform_table_event_subscriber_wires_via_synthetic_publisher() {
        let tbl_src: &'static str = r#"
table 18 Customer
{
    fields { field(1; "No."; Code[20]) { } }
}
"#;
        let sub_src: &'static str = r#"
codeunit 50710 "CustDeleteSub"
{
    [EventSubscriber(ObjectType::Table, Database::Customer, 'OnAfterDeleteEvent', '', true, false)]
    local procedure OnDeleteCustomer(var Rec: Record Customer; RunTrigger: Boolean)
    begin
    end;
}
"#;
        let app_id = make_app_id("TestApp");
        let units = vec![
            make_unit(app_id.clone(), "Customer.al", tbl_src),
            make_unit(app_id, "CustDeleteSub.al", sub_src),
        ];
        // build_graph is the local test helper (extract only); run the injection
        // that build_program_graph applies in production.
        let mut graph = build_graph(&units, None);
        crate::program::build::inject_platform_event_publishers(&mut graph);

        // A synthetic Platform publisher for OnAfterDeleteEvent now sits on Customer.
        let cust = graph
            .objects
            .iter()
            .find(|o| o.name == "Customer")
            .expect("Customer table");
        let synth = graph
            .routines
            .iter()
            .find(|r| {
                r.id.object == cust.id
                    && r.id.name_lc == "onafterdeleteevent"
                    && r.publisher_kind
                        == Some(crate::program::resolve::event::PublisherKind::Platform)
            })
            .expect("synthetic platform publisher injected on Customer");

        // The subscriber binds to it → exactly one EventFlow edge, one route, Resolved.
        let index = ResolveIndex::build(&graph);
        let surface = DeclSurface::build(&graph, &units);
        let edges = emit_event_flow_edges(&graph, &index, &surface);
        let e = edges
            .iter()
            .find(|e| e.from == synth.id)
            .expect("EventFlow edge from the synthetic platform publisher");
        assert_eq!(
            e.routes.len(),
            1,
            "subscriber must bind as the single route"
        );
        assert_eq!(
            classify_obligation(e),
            ObligationOutcome::Resolved,
            "a bound subscriber makes the platform event Resolved, not orphaned"
        );
    }

    // (c3) subscriber to a platform PAGE event (OnOpenPageEvent) wires via a
    // synthetic Platform publisher on the page.
    #[test]
    fn platform_page_event_subscriber_wires_via_synthetic_publisher() {
        let pg_src: &'static str = r#"
page 21 "Customer Card"
{
    layout { area(Content) { } }
}
"#;
        let sub_src: &'static str = r#"
codeunit 50711 "CustCardOpenSub"
{
    [EventSubscriber(ObjectType::Page, Page::"Customer Card", 'OnOpenPageEvent', '', false, false)]
    local procedure OnOpenCustCard(var Rec: Record Customer)
    begin
    end;
}
"#;
        let app_id = make_app_id("TestApp");
        let units = vec![
            make_unit(app_id.clone(), "CustomerCard.al", pg_src),
            make_unit(app_id, "CustCardOpenSub.al", sub_src),
        ];
        let mut graph = build_graph(&units, None);
        crate::program::build::inject_platform_event_publishers(&mut graph);

        let pg = graph
            .objects
            .iter()
            .find(|o| o.name == "Customer Card")
            .expect("Customer Card page");
        let synth = graph
            .routines
            .iter()
            .find(|r| {
                r.id.object == pg.id
                    && r.id.name_lc == "onopenpageevent"
                    && r.publisher_kind
                        == Some(crate::program::resolve::event::PublisherKind::Platform)
            })
            .expect("synthetic platform publisher injected on the page");

        let index = ResolveIndex::build(&graph);
        let surface = DeclSurface::build(&graph, &units);
        let edges = emit_event_flow_edges(&graph, &index, &surface);
        let e = edges
            .iter()
            .find(|e| e.from == synth.id)
            .expect("EventFlow edge from the synthetic page publisher");
        assert_eq!(
            e.routes.len(),
            1,
            "subscriber must bind as the single route"
        );
    }

    // (d) non-Manual subscriber → route IS in default_reachable_routes
    #[test]
    fn event_flow_non_manual_subscriber_default_reachable() {
        let pub_src: &'static str = r#"
codeunit 50703 "DefaultPub"
{
    [IntegrationEvent(false, false)]
    procedure OnAfterZ()
    begin
    end;
}
"#;
        let sub_src: &'static str = r#"
codeunit 50704 "DefaultSub"
{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::"DefaultPub", 'OnAfterZ', '', false, false)]
    local procedure OnAfterZHandler()
    begin
    end;
}
"#;
        let app_id = make_app_id("TestApp");
        let unit_pub = make_unit(app_id.clone(), "DefaultPub.al", pub_src);
        let unit_sub = make_unit(app_id, "DefaultSub.al", sub_src);
        let units = vec![unit_pub, unit_sub];
        let graph = build_graph(&units, None);
        let index = ResolveIndex::build(&graph);
        let surface = DeclSurface::build(&graph, &units);

        let edges = emit_event_flow_edges(&graph, &index, &surface);
        let e = edges
            .iter()
            .find(|e| e.kind == EdgeKind::EventFlow)
            .expect("EventFlow edge");

        // Non-Manual subscriber: route must be in default_reachable_routes.
        let default_routes: Vec<_> = e.default_reachable_routes().collect();
        assert_eq!(
            default_routes.len(),
            1,
            "non-Manual route must be in default_reachable_routes"
        );
        assert!(
            !default_routes[0]
                .conditions
                .contains(&Condition::ManualBinding),
            "non-Manual route must not have ManualBinding"
        );
        assert_eq!(default_routes[0].evidence, Evidence::Source);
    }

    // (e) determinism: two calls with same inputs produce identical output
    #[test]
    fn event_flow_emission_is_deterministic() {
        let (graph, units) = build_event_flow_fixture_manual();
        let index = ResolveIndex::build(&graph);
        let surface = DeclSurface::build(&graph, &units);

        let edges1 = emit_event_flow_edges(&graph, &index, &surface);
        let edges2 = emit_event_flow_edges(&graph, &index, &surface);

        assert_eq!(
            edges1, edges2,
            "emit_event_flow_edges must be deterministic across calls"
        );
    }

    // -----------------------------------------------------------------------
    // ABI event-kind threading: Task 1B.3a fix
    //
    // Verifies that `make_routine_route` threads `abi_routine_kind`/`abi_event_kind`
    // from the `RoutineNode` into the `AbiRoutineKey` instead of hardcoding
    // `Procedure`/`None` for every SymbolOnly dep routine.
    // -----------------------------------------------------------------------

    /// Build a graph with a workspace codeunit + a SymbolOnly dep codeunit that
    /// has one event-publisher routine and one regular procedure.
    fn build_abi_kind_fixture() -> (ProgramGraph, Vec<ParsedUnit>) {
        let ws_id = make_app_id("WS");
        let dep_id = make_app_id("DepApp");

        let src: &'static str = r#"
codeunit 50000 "Caller"
{
    procedure Run()
    begin
    end;
}
"#;
        let unit = make_unit(ws_id.clone(), "Caller.al", src);
        let units = vec![unit];

        let mut apps = AppRegistry::default();
        let ws_ref = apps.intern(&ws_id);
        let dep_ref = apps.intern(&dep_id);

        let mut objects: Vec<ObjectNode> = Vec::new();
        let mut routines: Vec<RoutineNode> = Vec::new();

        // Extract workspace nodes from source.
        for pf in &units[0].files {
            extract_nodes(
                ws_ref,
                &pf.file,
                pf.provenance.tier,
                &mut objects,
                &mut routines,
            );
        }

        // SymbolOnly dep codeunit 50100 "DepCU".
        let dep_obj_id = ObjectNodeId {
            app: dep_ref,
            kind: ObjectKind::Codeunit,
            key: ObjKey::Id(50100),
        };
        objects.push(ObjectNode {
            id: dep_obj_id.clone(),
            name: "DepCU".into(),
            declared_id: Some(50100),
            extends_target: None,
            implements: vec![],
            tier: TrustTier::SymbolOnly,
            source_table: None,
            table_no: None,
            source_table_temporary: false,
            page_controls: vec![],
            fields: vec![],
            dataitems: vec![],
            parse_incomplete: false,
        });

        // Event-publisher routine: abi_routine_kind=EventPublisher, abi_event_kind=Integration.
        routines.push(RoutineNode {
            id: RoutineNodeId {
                object: dep_obj_id.clone(),
                name_lc: "ondepevent".into(),
                enclosing_member_lc: None,
                params_count: 1,
                sig_fp: 0,
            },
            name: "OnDepEvent".into(),
            is_trigger: false,
            access: Access::Public,
            tier: TrustTier::SymbolOnly,
            event_subscribers: vec![],
            subscriber_instance_manual: false,
            publisher_kind: None,
            include_sender: None,
            abi_routine_kind: Some(AbiRoutineKind::EventPublisher),
            abi_event_kind: Some(AbiEventKind::Integration),
            param_sig_key: String::new(),
            return_type: None,
            return_type_id: None,
            abi_overload_collapsed: false,
            source_overload_aliased: false,
            abi_params: AbiParams::Missing,
        });

        // Regular procedure: abi_routine_kind=Procedure, abi_event_kind=None.
        routines.push(RoutineNode {
            id: RoutineNodeId {
                object: dep_obj_id.clone(),
                name_lc: "dowork".into(),
                enclosing_member_lc: None,
                params_count: 0,
                sig_fp: 0,
            },
            name: "DoWork".into(),
            is_trigger: false,
            access: Access::Public,
            tier: TrustTier::SymbolOnly,
            event_subscribers: vec![],
            subscriber_instance_manual: false,
            publisher_kind: None,
            include_sender: None,
            abi_routine_kind: Some(AbiRoutineKind::Procedure),
            abi_event_kind: Some(AbiEventKind::None),
            param_sig_key: String::new(),
            return_type: None,
            return_type_id: None,
            abi_overload_collapsed: false,
            source_overload_aliased: false,
            abi_params: AbiParams::Missing,
        });

        objects.sort_by(|a, b| a.id.cmp(&b.id));
        routines.sort_by(|a, b| a.id.cmp(&b.id));

        let mut topology = DependencyGraph::default();
        topology.add_dependency(ws_ref, dep_ref);

        let obj_index = ObjectIndex::build(&objects);
        let graph = ProgramGraph {
            apps,
            topology,
            objects,
            routines,
            obj_index,
            ..Default::default()
        };
        (graph, units)
    }

    /// A SymbolOnly dep event-publisher resolved via `resolve_member` (Object
    /// receiver) must carry `AbiRoutineKey.routine_kind == EventPublisher` and
    /// `event_kind == Integration` — NOT the hardcoded `Procedure/None` that
    /// existed before Task 1B.3a.
    ///
    /// A SymbolOnly dep regular procedure must carry `Procedure/None` (unchanged).
    #[test]
    fn symbolonly_event_publisher_route_carries_correct_abi_kind() {
        let (graph, units) = build_abi_kind_fixture();
        let index = ResolveIndex::build(&graph);
        let surface = DeclSurface::build(&graph, &units);
        let from_obj = find_obj(&graph, "Caller");

        // --- event publisher: must be EventPublisher / Integration ---
        let (shape, routes) = resolve_member(
            &ReceiverType::Object {
                kind: ObjectKind::Codeunit,
                name_lc: "depcu".into(),
                id: None,
            },
            "ondepevent",
            1,
            from_obj,
            &graph,
            &index,
            &surface,
        );
        assert_eq!(shape, DispatchShape::Exact);
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].evidence, Evidence::Opaque);
        let RouteTarget::AbiSymbol { ref key } = routes[0].target else {
            panic!(
                "expected AbiSymbol target for event publisher, got {:?}",
                routes[0].target
            );
        };
        assert_eq!(
            key.routine_kind,
            AbiRoutineKind::EventPublisher,
            "dep event-publisher must carry EventPublisher kind in AbiRoutineKey"
        );
        assert_eq!(
            key.event_kind,
            AbiEventKind::Integration,
            "integration event must carry Integration event_kind in AbiRoutineKey"
        );

        // --- regular procedure: must be Procedure / None (unchanged) ---
        let (shape2, routes2) = resolve_member(
            &ReceiverType::Object {
                kind: ObjectKind::Codeunit,
                name_lc: "depcu".into(),
                id: None,
            },
            "dowork",
            0,
            from_obj,
            &graph,
            &index,
            &surface,
        );
        assert_eq!(shape2, DispatchShape::Exact);
        assert_eq!(routes2.len(), 1);
        assert_eq!(routes2[0].evidence, Evidence::Opaque);
        let RouteTarget::AbiSymbol { key: ref key2 } = routes2[0].target else {
            panic!(
                "expected AbiSymbol target for procedure, got {:?}",
                routes2[0].target
            );
        };
        assert_eq!(
            key2.routine_kind,
            AbiRoutineKind::Procedure,
            "regular dep procedure must carry Procedure kind"
        );
        assert_eq!(
            key2.event_kind,
            AbiEventKind::None,
            "regular dep procedure must carry None event_kind"
        );
    }

    // -----------------------------------------------------------------------
    // Task 1 (feat/resolve-access-uniform-and-compound-receiver): per-candidate
    // access filter in `resolve_in_object` — closes the `ReceiverType::Object`
    // arm (gap D) + both `Interface`-impl delegates (gaps F/G), which
    // previously did ZERO access filtering. `Codeunit.Run`/`resolve_object_run`
    // and event-subscriber dispatch are UNTOUCHED (they bypass
    // `resolve_in_object` entirely) — the negative/control fixtures below pin
    // that boundary too. See `.superpowers/sdd/task-1-report.md` and
    // `tests/r0-corpus/ws-object-interface-visibility/` for the full matrix +
    // compiler-semantics writeup.
    // -----------------------------------------------------------------------

    // --- POSITIVE controls: must still resolve to Source -------------------

    // (D-pos-1) Object receiver, cross-app, `public` method.
    #[test]
    fn resolve_member_object_cross_app_public_method_resolves_to_source() {
        use crate::program::resolve::receiver::ReceiverType;

        let src_target: &'static str = r#"
codeunit 53900 "PubXTarget"
{
    procedure DoWork()
    begin
    end;
}
"#;
        let src_caller: &'static str = r#"
codeunit 53901 "PubXCaller"
{
    procedure Trigger()
    begin
    end;
}
"#;
        let app_a = make_app_id("PrimaryApp1");
        let app_b = make_app_id("DepApp1");
        let unit_target = make_unit(app_b, "PubXTarget.al", src_target);
        let unit_caller = make_unit(app_a, "PubXCaller.al", src_caller);
        let units = [unit_target, unit_caller];
        let graph = build_graph_multi_dep(&units, &[("PrimaryApp1", "DepApp1")]);
        let index = ResolveIndex::build(&graph);
        let surface = DeclSurface::build(&graph, &units);

        let from_obj = find_obj(&graph, "PubXCaller");
        let receiver = ReceiverType::Object {
            kind: ObjectKind::Codeunit,
            name_lc: "pubxtarget".into(),
            id: None,
        };
        let (shape, routes) =
            resolve_member(&receiver, "dowork", 0, from_obj, &graph, &index, &surface);

        assert_eq!(shape, DispatchShape::Exact);
        assert_eq!(routes.len(), 1);
        assert!(
            matches!(routes[0].target, RouteTarget::Routine(_)),
            "a `public` cross-app Object-receiver method must still resolve \
             to Source (gap D positive control); got {:?}",
            routes[0].target
        );
        assert_eq!(routes[0].evidence, Evidence::Source);
    }

    // (D-pos-2) Object receiver, same-app, `internal` method.
    #[test]
    fn resolve_member_object_same_app_internal_method_resolves_to_source() {
        use crate::program::resolve::receiver::ReceiverType;

        let src_target: &'static str = r#"
codeunit 53910 "IntXTarget"
{
    internal procedure DoWork()
    begin
    end;
}
"#;
        let src_caller: &'static str = r#"
codeunit 53911 "IntXCaller"
{
    procedure Trigger()
    begin
    end;
}
"#;
        let app_id = make_app_id("TestApp");
        let unit_target = make_unit(app_id.clone(), "IntXTarget.al", src_target);
        let unit_caller = make_unit(app_id, "IntXCaller.al", src_caller);
        let units = [unit_target, unit_caller];
        let graph = build_graph(&units, None);
        let index = ResolveIndex::build(&graph);
        let surface = DeclSurface::build(&graph, &units);

        let from_obj = find_obj(&graph, "IntXCaller");
        let receiver = ReceiverType::Object {
            kind: ObjectKind::Codeunit,
            name_lc: "intxtarget".into(),
            id: None,
        };
        let (shape, routes) =
            resolve_member(&receiver, "dowork", 0, from_obj, &graph, &index, &surface);

        assert_eq!(shape, DispatchShape::Exact);
        assert_eq!(routes.len(), 1);
        assert!(
            matches!(routes[0].target, RouteTarget::Routine(_)),
            "a same-app `internal` Object-receiver method must resolve to \
             Source (gap D positive control — Internal is app-scoped); got {:?}",
            routes[0].target
        );
        assert_eq!(routes[0].evidence, Evidence::Source);
    }

    // (D-pos-3) Object receiver, direct PageExtension → base Page `protected` method.
    #[test]
    fn resolve_member_object_direct_extension_protected_method_resolves_to_source() {
        use crate::program::resolve::receiver::ReceiverType;

        let src_page: &'static str = r#"
page 53920 "ProtXBase"
{
    protected procedure Prot()
    begin
    end;
}
"#;
        let src_ext: &'static str = r#"
pageextension 53921 "ProtXBaseExt" extends ProtXBase
{
    procedure Trigger()
    begin
    end;
}
"#;
        let app_id = make_app_id("TestApp");
        let unit_page = make_unit(app_id.clone(), "ProtXBase.al", src_page);
        let unit_ext = make_unit(app_id, "ProtXBaseExt.al", src_ext);
        let units = [unit_page, unit_ext];
        let graph = build_graph(&units, None);
        let index = ResolveIndex::build(&graph);
        let surface = DeclSurface::build(&graph, &units);

        let from_obj = find_obj(&graph, "ProtXBaseExt");
        assert_eq!(from_obj.id.kind, ObjectKind::PageExtension);
        let receiver = ReceiverType::Object {
            kind: ObjectKind::Page,
            name_lc: "protxbase".into(),
            id: None,
        };
        let (shape, routes) =
            resolve_member(&receiver, "prot", 0, from_obj, &graph, &index, &surface);

        assert_eq!(shape, DispatchShape::Exact);
        assert_eq!(routes.len(), 1);
        assert!(
            matches!(routes[0].target, RouteTarget::Routine(_)),
            "a direct PageExtension calling its base's `protected` method \
             via an Object receiver must resolve to Source (gap D positive \
             control); got {:?}",
            routes[0].target
        );
        assert_eq!(routes[0].evidence, Evidence::Source);
    }

    // -----------------------------------------------------------------------
    // Task 1 (pageext-merge-and-final-residual plan): PageExtension routine
    // merge into base-Page member resolution. `ReceiverType::Object{kind:
    // Page}` must reach a method declared ONLY in a closure-visible
    // PageExtension, not just the base page's own procedures — the CDO
    // grounding's 7 `eCandidates` sites (`GetOutputProfile`/
    // `OnlyVendorsAreHandled`/`OnlyCustomersAreHandled`, all `internal`,
    // same-app). See `resolve_in_page_scope`'s doc for the full design.
    // -----------------------------------------------------------------------

    // (T1-pos-1) base-Page receiver, PageExtension-declared `internal`
    // procedure, same app — must resolve to Source (the 7 sites' exact shape).
    #[test]
    fn resolve_member_object_page_merge_same_app_internal_extension_resolves_to_source() {
        use crate::program::resolve::receiver::ReceiverType;

        let src_page: &'static str = r#"
page 61000 "PxMergeBase1"
{
    procedure BaseProc()
    begin
    end;
}
"#;
        let src_ext: &'static str = r#"
pageextension 61001 "PxMergeBase1Ext" extends "PxMergeBase1"
{
    internal procedure ExtProc()
    begin
    end;
}
"#;
        let src_caller: &'static str = r#"
codeunit 61002 "PxMergeCaller1"
{
    procedure Trigger()
    begin
    end;
}
"#;
        let app_id = make_app_id("PxMergeApp1");
        let unit_page = make_unit(app_id.clone(), "PxMergeBase1.al", src_page);
        let unit_ext = make_unit(app_id.clone(), "PxMergeBase1Ext.al", src_ext);
        let unit_caller = make_unit(app_id, "PxMergeCaller1.al", src_caller);
        let units = [unit_page, unit_ext, unit_caller];
        let graph = build_graph(&units, None);
        let index = ResolveIndex::build(&graph);
        let surface = DeclSurface::build(&graph, &units);

        let from_obj = find_obj(&graph, "PxMergeCaller1");
        let receiver = ReceiverType::Object {
            kind: ObjectKind::Page,
            name_lc: "pxmergebase1".into(),
            id: None,
        };
        let (shape, routes) =
            resolve_member(&receiver, "extproc", 0, from_obj, &graph, &index, &surface);

        assert_eq!(shape, DispatchShape::Exact);
        assert_eq!(routes.len(), 1);
        assert!(
            matches!(routes[0].target, RouteTarget::Routine(_)),
            "a base-Page-typed receiver calling a same-app `internal` \
             PageExtension procedure must resolve to Source (Task 1's \
             central fix); got {:?}",
            routes[0].target
        );
        assert_eq!(routes[0].evidence, Evidence::Source);
    }

    // (T1-neg-1) DIFFERENT-app internal extension member (no friend) —
    // must decline with InternalNotVisible, not a bare MemberNotFound.
    #[test]
    fn resolve_member_object_page_merge_different_app_internal_extension_declines() {
        use crate::program::resolve::receiver::ReceiverType;

        let src_page: &'static str = r#"
page 61010 "PxMergeBase2"
{
    procedure BaseProc()
    begin
    end;
}
"#;
        let src_ext: &'static str = r#"
pageextension 61011 "PxMergeBase2Ext" extends "PxMergeBase2"
{
    internal procedure ExtProc()
    begin
    end;
}
"#;
        let src_caller: &'static str = r#"
codeunit 61012 "PxMergeCaller2"
{
    procedure Trigger()
    begin
    end;
}
"#;
        // CallerApp depends on both PageApp (base) and ExtApp (extension) —
        // both in the caller's closure — but the extension's `internal`
        // procedure is declared in a DIFFERENT app than the caller, with no
        // InternalsVisibleTo friend grant either way.
        let app_page = make_app_id("PxMergePageApp2");
        let app_ext = make_app_id("PxMergeExtApp2");
        let app_caller = make_app_id("PxMergeCallerApp2");
        let unit_page = make_unit(app_page, "PxMergeBase2.al", src_page);
        let unit_ext = make_unit(app_ext, "PxMergeBase2Ext.al", src_ext);
        let unit_caller = make_unit(app_caller, "PxMergeCaller2.al", src_caller);
        let units = [unit_page, unit_ext, unit_caller];
        let graph = build_graph_multi_dep(
            &units,
            &[
                ("PxMergeCallerApp2", "PxMergePageApp2"),
                ("PxMergeCallerApp2", "PxMergeExtApp2"),
                ("PxMergeExtApp2", "PxMergePageApp2"),
            ],
        );
        let index = ResolveIndex::build(&graph);
        let surface = DeclSurface::build(&graph, &units);

        let from_obj = find_obj(&graph, "PxMergeCaller2");
        let receiver = ReceiverType::Object {
            kind: ObjectKind::Page,
            name_lc: "pxmergebase2".into(),
            id: None,
        };
        let (shape, routes) =
            resolve_member(&receiver, "extproc", 0, from_obj, &graph, &index, &surface);

        assert_eq!(shape, DispatchShape::Exact);
        assert_eq!(routes.len(), 1);
        assert_eq!(
            routes[0].target,
            RouteTarget::Unresolved,
            "a cross-app `internal` PageExtension procedure (no friend \
             grant) must stay honest Unknown, not a false Source; got {:?}",
            routes[0].target
        );
        assert!(
            matches!(
                routes[0].evidence,
                Evidence::Unknown(UnknownReason::InternalNotVisible)
            ),
            "must be excluded with the specific InternalNotVisible reason \
             (Task 1's access-filter fixture), not a bare MemberNotFound; \
             got {:?}",
            routes[0].evidence
        );
    }

    // (T1-neg-2) out-of-closure extension — the extension's app is never a
    // dependency of the caller's app, so its member must be structurally
    // INVISIBLE (MemberNotFound), never surfaced as an access exclusion.
    #[test]
    fn resolve_member_object_page_merge_out_of_closure_extension_invisible() {
        use crate::program::resolve::receiver::ReceiverType;

        let src_page: &'static str = r#"
page 61020 "PxMergeBase3"
{
    procedure BaseProc()
    begin
    end;
}
"#;
        let src_ext: &'static str = r#"
pageextension 61021 "PxMergeBase3Ext" extends "PxMergeBase3"
{
    procedure ExtProc()
    begin
    end;
}
"#;
        let src_caller: &'static str = r#"
codeunit 61022 "PxMergeCaller3"
{
    procedure Trigger()
    begin
    end;
}
"#;
        // CallerApp depends ONLY on PageApp — never on ExtApp — so the
        // extension is entirely out of the caller's dependency closure, even
        // though ExtApp itself depends on PageApp (real AL requires that for
        // `extends` to compile). The extension's `public` access does not
        // matter: out-of-closure means the object itself was never imported.
        let app_page = make_app_id("PxMergePageApp3");
        let app_ext = make_app_id("PxMergeExtApp3");
        let app_caller = make_app_id("PxMergeCallerApp3");
        let unit_page = make_unit(app_page, "PxMergeBase3.al", src_page);
        let unit_ext = make_unit(app_ext, "PxMergeBase3Ext.al", src_ext);
        let unit_caller = make_unit(app_caller, "PxMergeCaller3.al", src_caller);
        let units = [unit_page, unit_ext, unit_caller];
        let graph = build_graph_multi_dep(
            &units,
            &[
                ("PxMergeCallerApp3", "PxMergePageApp3"),
                ("PxMergeExtApp3", "PxMergePageApp3"),
            ],
        );
        let index = ResolveIndex::build(&graph);
        let surface = DeclSurface::build(&graph, &units);

        let from_obj = find_obj(&graph, "PxMergeCaller3");
        let receiver = ReceiverType::Object {
            kind: ObjectKind::Page,
            name_lc: "pxmergebase3".into(),
            id: None,
        };
        let (shape, routes) =
            resolve_member(&receiver, "extproc", 0, from_obj, &graph, &index, &surface);

        assert_eq!(shape, DispatchShape::Exact);
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].target, RouteTarget::Unresolved);
        assert!(
            matches!(
                routes[0].evidence,
                Evidence::Unknown(UnknownReason::MemberNotFound)
            ),
            "an out-of-closure extension must be structurally invisible \
             (MemberNotFound), never surfaced via an access-exclusion \
             reason; got {:?}",
            routes[0].evidence
        );
    }

    // (T1-amb-1) TWO caller-visible PageExtensions both declaring the same
    // viable member — genuine ambiguity, no first-wins (the
    // aggregate-then-adjudicate proof).
    #[test]
    fn resolve_member_object_page_merge_two_extensions_same_member_is_ambiguous() {
        use crate::program::resolve::receiver::ReceiverType;

        let src_page: &'static str = r#"
page 61030 "PxMergeBase4"
{
    procedure BaseProc()
    begin
    end;
}
"#;
        let src_ext_a: &'static str = r#"
pageextension 61031 "PxMergeBase4ExtA" extends "PxMergeBase4"
{
    procedure DupProc()
    begin
    end;
}
"#;
        let src_ext_b: &'static str = r#"
pageextension 61032 "PxMergeBase4ExtB" extends "PxMergeBase4"
{
    procedure DupProc()
    begin
    end;
}
"#;
        let src_caller: &'static str = r#"
codeunit 61033 "PxMergeCaller4"
{
    procedure Trigger()
    begin
    end;
}
"#;
        let app_id = make_app_id("PxMergeApp4");
        let unit_page = make_unit(app_id.clone(), "PxMergeBase4.al", src_page);
        let unit_ext_a = make_unit(app_id.clone(), "PxMergeBase4ExtA.al", src_ext_a);
        let unit_ext_b = make_unit(app_id.clone(), "PxMergeBase4ExtB.al", src_ext_b);
        let unit_caller = make_unit(app_id, "PxMergeCaller4.al", src_caller);
        let units = [unit_page, unit_ext_a, unit_ext_b, unit_caller];
        let graph = build_graph(&units, None);
        let index = ResolveIndex::build(&graph);
        let surface = DeclSurface::build(&graph, &units);

        let from_obj = find_obj(&graph, "PxMergeCaller4");
        let receiver = ReceiverType::Object {
            kind: ObjectKind::Page,
            name_lc: "pxmergebase4".into(),
            id: None,
        };
        let (shape, routes) =
            resolve_member(&receiver, "dupproc", 0, from_obj, &graph, &index, &surface);

        assert_eq!(shape, DispatchShape::Exact);
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].target, RouteTarget::Unresolved);
        assert!(
            matches!(
                routes[0].evidence,
                Evidence::Unknown(UnknownReason::OverloadAmbiguous)
            ),
            "two caller-visible PageExtensions both declaring the same \
             viable member must decline as a genuine ambiguity — no \
             first-wins (AL0440 makes this uncompilable in real source — \
             corrected, Task 3 roadmap-closure plan: live-probed, not the \
             AL0226 this comment previously guessed; this fixture is \
             DEFENSIVE-ONLY against malformed input); got \
             {:?}",
            routes[0].evidence
        );
    }

    // (T1-amb-2) base-vs-extension same-name-same-arity pair — the ambiguity
    // machinery fires too (defensive-only: AL0440 makes an exact base/
    // extension duplicate signature uncompilable in real AL — corrected,
    // Task 3 roadmap-closure plan: live-probed [`pageA.al`+`pageAExt.al`,
    // same procedure signature] AL0440 "already defines a method ... with
    // the same parameter types", not the AL0115 this comment previously
    // guessed).
    #[test]
    fn resolve_member_object_page_merge_base_vs_extension_duplicate_is_ambiguous() {
        use crate::program::resolve::receiver::ReceiverType;

        let src_page: &'static str = r#"
page 61040 "PxMergeBase5"
{
    procedure SameProc()
    begin
    end;
}
"#;
        let src_ext: &'static str = r#"
pageextension 61041 "PxMergeBase5Ext" extends "PxMergeBase5"
{
    procedure SameProc()
    begin
    end;
}
"#;
        let src_caller: &'static str = r#"
codeunit 61042 "PxMergeCaller5"
{
    procedure Trigger()
    begin
    end;
}
"#;
        let app_id = make_app_id("PxMergeApp5");
        let unit_page = make_unit(app_id.clone(), "PxMergeBase5.al", src_page);
        let unit_ext = make_unit(app_id.clone(), "PxMergeBase5Ext.al", src_ext);
        let unit_caller = make_unit(app_id, "PxMergeCaller5.al", src_caller);
        let units = [unit_page, unit_ext, unit_caller];
        let graph = build_graph(&units, None);
        let index = ResolveIndex::build(&graph);
        let surface = DeclSurface::build(&graph, &units);

        let from_obj = find_obj(&graph, "PxMergeCaller5");
        let receiver = ReceiverType::Object {
            kind: ObjectKind::Page,
            name_lc: "pxmergebase5".into(),
            id: None,
        };
        let (shape, routes) =
            resolve_member(&receiver, "sameproc", 0, from_obj, &graph, &index, &surface);

        assert_eq!(shape, DispatchShape::Exact);
        assert_eq!(routes.len(), 1);
        assert!(
            matches!(
                routes[0].evidence,
                Evidence::Unknown(UnknownReason::OverloadAmbiguous)
            ),
            "a base-vs-extension exact duplicate signature must decline as \
             a genuine ambiguity (defensive-only — AL0440 makes this \
             uncompilable in real AL, corrected Task 3 [live-probed, not the \
             AL0115 previously guessed]); got {:?}",
            routes[0].evidence
        );
    }

    // (T1-base-only) base-only calls are unchanged by the merge: a Page with
    // an extension present (that does NOT declare the called member) still
    // resolves the base's own procedure exactly as pre-Task-1, AND the
    // instance-builtin catalog fallback still fires when neither base nor
    // any extension declares the name.
    #[test]
    fn resolve_member_object_page_merge_base_only_unchanged() {
        use crate::program::resolve::receiver::ReceiverType;

        let src_page: &'static str = r#"
page 61050 "PxMergeBase6"
{
    procedure BaseProc()
    begin
    end;
}
"#;
        let src_ext: &'static str = r#"
pageextension 61051 "PxMergeBase6Ext" extends "PxMergeBase6"
{
    procedure UnrelatedExtProc()
    begin
    end;
}
"#;
        let src_caller: &'static str = r#"
codeunit 61052 "PxMergeCaller6"
{
    procedure Trigger()
    begin
    end;
}
"#;
        let app_id = make_app_id("PxMergeApp6");
        let unit_page = make_unit(app_id.clone(), "PxMergeBase6.al", src_page);
        let unit_ext = make_unit(app_id.clone(), "PxMergeBase6Ext.al", src_ext);
        let unit_caller = make_unit(app_id, "PxMergeCaller6.al", src_caller);
        let units = [unit_page, unit_ext, unit_caller];
        let graph = build_graph(&units, None);
        let index = ResolveIndex::build(&graph);
        let surface = DeclSurface::build(&graph, &units);

        let from_obj = find_obj(&graph, "PxMergeCaller6");
        let receiver = ReceiverType::Object {
            kind: ObjectKind::Page,
            name_lc: "pxmergebase6".into(),
            id: None,
        };

        // The base's own procedure still resolves to Source.
        let (shape, routes) =
            resolve_member(&receiver, "baseproc", 0, from_obj, &graph, &index, &surface);
        assert_eq!(shape, DispatchShape::Exact);
        assert_eq!(routes.len(), 1);
        assert!(matches!(routes[0].target, RouteTarget::Routine(_)));
        assert_eq!(routes[0].evidence, Evidence::Source);

        // A genuine platform-instrinsic (PageInstance catalog) member absent
        // from both base and extension still falls through to Catalog.
        let (shape2, routes2) =
            resolve_member(&receiver, "close", 0, from_obj, &graph, &index, &surface);
        assert_eq!(shape2, DispatchShape::Exact);
        assert_eq!(routes2.len(), 1);
        assert_eq!(routes2[0].evidence, Evidence::Catalog);
    }

    // (T1-arity) arity-mismatch on a base-only candidate must still surface
    // `ArityMismatch` (name found, wrong arity) — the merge must not
    // regress this pre-Task-1 per-object diagnostic into a bare
    // MemberNotFound/CatalogMiss.
    #[test]
    fn resolve_member_object_page_merge_arity_mismatch_preserved() {
        use crate::program::resolve::receiver::ReceiverType;

        let src_page: &'static str = r#"
page 61060 "PxMergeBase7"
{
    procedure OneArgProc(X: Integer)
    begin
    end;
}
"#;
        let src_ext: &'static str = r#"
pageextension 61061 "PxMergeBase7Ext" extends "PxMergeBase7"
{
    procedure UnrelatedExtProc()
    begin
    end;
}
"#;
        let src_caller: &'static str = r#"
codeunit 61062 "PxMergeCaller7"
{
    procedure Trigger()
    begin
    end;
}
"#;
        let app_id = make_app_id("PxMergeApp7");
        let unit_page = make_unit(app_id.clone(), "PxMergeBase7.al", src_page);
        let unit_ext = make_unit(app_id.clone(), "PxMergeBase7Ext.al", src_ext);
        let unit_caller = make_unit(app_id, "PxMergeCaller7.al", src_caller);
        let units = [unit_page, unit_ext, unit_caller];
        let graph = build_graph(&units, None);
        let index = ResolveIndex::build(&graph);
        let surface = DeclSurface::build(&graph, &units);

        let from_obj = find_obj(&graph, "PxMergeCaller7");
        let receiver = ReceiverType::Object {
            kind: ObjectKind::Page,
            name_lc: "pxmergebase7".into(),
            id: None,
        };
        // Call with arity 0 — the only declared "OneArgProc" takes 1 param.
        let (shape, routes) = resolve_member(
            &receiver,
            "oneargproc",
            0,
            from_obj,
            &graph,
            &index,
            &surface,
        );

        assert_eq!(shape, DispatchShape::Exact);
        assert_eq!(routes.len(), 1);
        assert!(
            matches!(
                routes[0].evidence,
                Evidence::Unknown(UnknownReason::ArityMismatch)
            ),
            "a wrong-arity call to a base-only candidate must stay \
             ArityMismatch (name found), not collapse into MemberNotFound/ \
             CatalogMiss via the merge; got {:?}",
            routes[0].evidence
        );
    }

    // (T1-cross-app-pos) a PUBLIC extension procedure from a dependent app —
    // the cross-app-legal case: the extension lives in a DIFFERENT app than
    // the caller, but the caller depends on it (directly or transitively)
    // and the member is `public`, so it must resolve to Source.
    #[test]
    fn resolve_member_object_page_merge_public_extension_cross_app_resolves() {
        use crate::program::resolve::receiver::ReceiverType;

        let src_page: &'static str = r#"
page 61070 "PxMergeBase8"
{
    procedure BaseProc()
    begin
    end;
}
"#;
        let src_ext: &'static str = r#"
pageextension 61071 "PxMergeBase8Ext" extends "PxMergeBase8"
{
    procedure PubExtProc()
    begin
    end;
}
"#;
        let src_caller: &'static str = r#"
codeunit 61072 "PxMergeCaller8"
{
    procedure Trigger()
    begin
    end;
}
"#;
        // CallerApp -> ExtApp -> PageApp (transitive closure): the caller
        // never directly depends on PageApp, only reaches it transitively
        // through ExtApp — proving `topology.closure` transitivity is what
        // makes both the base AND the extension visible.
        let app_page = make_app_id("PxMergePageApp8");
        let app_ext = make_app_id("PxMergeExtApp8");
        let app_caller = make_app_id("PxMergeCallerApp8");
        let unit_page = make_unit(app_page, "PxMergeBase8.al", src_page);
        let unit_ext = make_unit(app_ext, "PxMergeBase8Ext.al", src_ext);
        let unit_caller = make_unit(app_caller, "PxMergeCaller8.al", src_caller);
        let units = [unit_page, unit_ext, unit_caller];
        let graph = build_graph_multi_dep(
            &units,
            &[
                ("PxMergeCallerApp8", "PxMergeExtApp8"),
                ("PxMergeExtApp8", "PxMergePageApp8"),
            ],
        );
        let index = ResolveIndex::build(&graph);
        let surface = DeclSurface::build(&graph, &units);

        let from_obj = find_obj(&graph, "PxMergeCaller8");
        let receiver = ReceiverType::Object {
            kind: ObjectKind::Page,
            name_lc: "pxmergebase8".into(),
            id: None,
        };
        let (shape, routes) = resolve_member(
            &receiver,
            "pubextproc",
            0,
            from_obj,
            &graph,
            &index,
            &surface,
        );

        assert_eq!(shape, DispatchShape::Exact);
        assert_eq!(routes.len(), 1);
        assert!(
            matches!(routes[0].target, RouteTarget::Routine(_)),
            "a `public` PageExtension procedure in a transitively-depended- \
             on app must resolve to Source (the cross-app-legal case); got \
             {:?}",
            routes[0].target
        );
        assert_eq!(routes[0].evidence, Evidence::Source);
    }

    // -----------------------------------------------------------------------
    // Task 1 (roadmap-closure plan): ReportExtension routine merge into
    // base-Report member resolution — the `Report` analog of the
    // PageExtension merge block above, via `resolve_in_report_scope` (a thin
    // `resolve_in_extendable_scope` wrapper, `ZeroMatchStrategy::
    // PreserveArityMismatch`). Mirrors each Page fixture exactly (same
    // shape, same assertions) — the postcondition being proven is that the
    // unified engine produces IDENTICAL behavior for a third kind, not just
    // for the two it already had fixtures for. See `resolve_in_report_scope`'s
    // doc for the al-compile probe (AL0135 vs AL0132) grounding the
    // `PreserveArityMismatch` policy for Report specifically.
    // -----------------------------------------------------------------------

    // (T1-report-pos-1) base-Report receiver, ReportExtension-declared
    // `internal` procedure, same app — must resolve to Source.
    #[test]
    fn resolve_member_object_report_merge_same_app_internal_extension_resolves_to_source() {
        use crate::program::resolve::receiver::ReceiverType;

        let src_report: &'static str = r#"
report 62000 "RxMergeBase1"
{
    dataset
    {
    }

    procedure BaseProc()
    begin
    end;
}
"#;
        let src_ext: &'static str = r#"
reportextension 62001 "RxMergeBase1Ext" extends "RxMergeBase1"
{
    internal procedure ExtProc()
    begin
    end;
}
"#;
        let src_caller: &'static str = r#"
codeunit 62002 "RxMergeCaller1"
{
    procedure Trigger()
    begin
    end;
}
"#;
        let app_id = make_app_id("RxMergeApp1");
        let unit_report = make_unit(app_id.clone(), "RxMergeBase1.al", src_report);
        let unit_ext = make_unit(app_id.clone(), "RxMergeBase1Ext.al", src_ext);
        let unit_caller = make_unit(app_id, "RxMergeCaller1.al", src_caller);
        let units = [unit_report, unit_ext, unit_caller];
        let graph = build_graph(&units, None);
        let index = ResolveIndex::build(&graph);
        let surface = DeclSurface::build(&graph, &units);

        let from_obj = find_obj(&graph, "RxMergeCaller1");
        let receiver = ReceiverType::Object {
            kind: ObjectKind::Report,
            name_lc: "rxmergebase1".into(),
            id: None,
        };
        let (shape, routes) =
            resolve_member(&receiver, "extproc", 0, from_obj, &graph, &index, &surface);

        assert_eq!(shape, DispatchShape::Exact);
        assert_eq!(routes.len(), 1);
        assert!(
            matches!(routes[0].target, RouteTarget::Routine(_)),
            "a base-Report-typed receiver calling a same-app `internal` \
             ReportExtension procedure must resolve to Source (Task 1's \
             central fix, mirroring the Page merge); got {:?}",
            routes[0].target
        );
        assert_eq!(routes[0].evidence, Evidence::Source);
    }

    // (T1-report-neg-1) DIFFERENT-app internal extension member (no friend) —
    // must decline with InternalNotVisible, not a bare MemberNotFound.
    #[test]
    fn resolve_member_object_report_merge_different_app_internal_extension_declines() {
        use crate::program::resolve::receiver::ReceiverType;

        let src_report: &'static str = r#"
report 62010 "RxMergeBase2"
{
    dataset
    {
    }

    procedure BaseProc()
    begin
    end;
}
"#;
        let src_ext: &'static str = r#"
reportextension 62011 "RxMergeBase2Ext" extends "RxMergeBase2"
{
    internal procedure ExtProc()
    begin
    end;
}
"#;
        let src_caller: &'static str = r#"
codeunit 62012 "RxMergeCaller2"
{
    procedure Trigger()
    begin
    end;
}
"#;
        let app_report = make_app_id("RxMergeReportApp2");
        let app_ext = make_app_id("RxMergeExtApp2");
        let app_caller = make_app_id("RxMergeCallerApp2");
        let unit_report = make_unit(app_report, "RxMergeBase2.al", src_report);
        let unit_ext = make_unit(app_ext, "RxMergeBase2Ext.al", src_ext);
        let unit_caller = make_unit(app_caller, "RxMergeCaller2.al", src_caller);
        let units = [unit_report, unit_ext, unit_caller];
        let graph = build_graph_multi_dep(
            &units,
            &[
                ("RxMergeCallerApp2", "RxMergeReportApp2"),
                ("RxMergeCallerApp2", "RxMergeExtApp2"),
                ("RxMergeExtApp2", "RxMergeReportApp2"),
            ],
        );
        let index = ResolveIndex::build(&graph);
        let surface = DeclSurface::build(&graph, &units);

        let from_obj = find_obj(&graph, "RxMergeCaller2");
        let receiver = ReceiverType::Object {
            kind: ObjectKind::Report,
            name_lc: "rxmergebase2".into(),
            id: None,
        };
        let (shape, routes) =
            resolve_member(&receiver, "extproc", 0, from_obj, &graph, &index, &surface);

        assert_eq!(shape, DispatchShape::Exact);
        assert_eq!(routes.len(), 1);
        assert_eq!(
            routes[0].target,
            RouteTarget::Unresolved,
            "a cross-app `internal` ReportExtension procedure (no friend \
             grant) must stay honest Unknown, not a false Source; got {:?}",
            routes[0].target
        );
        assert!(
            matches!(
                routes[0].evidence,
                Evidence::Unknown(UnknownReason::InternalNotVisible)
            ),
            "must be excluded with the specific InternalNotVisible reason, \
             not a bare MemberNotFound; got {:?}",
            routes[0].evidence
        );
    }

    // (T1-report-neg-2) out-of-closure extension — the extension's app is
    // never a dependency of the caller's app, so its member must be
    // structurally INVISIBLE (MemberNotFound), never surfaced as an access
    // exclusion.
    #[test]
    fn resolve_member_object_report_merge_out_of_closure_extension_invisible() {
        use crate::program::resolve::receiver::ReceiverType;

        let src_report: &'static str = r#"
report 62020 "RxMergeBase3"
{
    dataset
    {
    }

    procedure BaseProc()
    begin
    end;
}
"#;
        let src_ext: &'static str = r#"
reportextension 62021 "RxMergeBase3Ext" extends "RxMergeBase3"
{
    procedure ExtProc()
    begin
    end;
}
"#;
        let src_caller: &'static str = r#"
codeunit 62022 "RxMergeCaller3"
{
    procedure Trigger()
    begin
    end;
}
"#;
        // CallerApp depends ONLY on ReportApp — never on ExtApp — so the
        // extension is entirely out of the caller's dependency closure, even
        // though ExtApp itself depends on ReportApp (real AL requires that
        // for `extends` to compile).
        let app_report = make_app_id("RxMergeReportApp3");
        let app_ext = make_app_id("RxMergeExtApp3");
        let app_caller = make_app_id("RxMergeCallerApp3");
        let unit_report = make_unit(app_report, "RxMergeBase3.al", src_report);
        let unit_ext = make_unit(app_ext, "RxMergeBase3Ext.al", src_ext);
        let unit_caller = make_unit(app_caller, "RxMergeCaller3.al", src_caller);
        let units = [unit_report, unit_ext, unit_caller];
        let graph = build_graph_multi_dep(
            &units,
            &[
                ("RxMergeCallerApp3", "RxMergeReportApp3"),
                ("RxMergeExtApp3", "RxMergeReportApp3"),
            ],
        );
        let index = ResolveIndex::build(&graph);
        let surface = DeclSurface::build(&graph, &units);

        let from_obj = find_obj(&graph, "RxMergeCaller3");
        let receiver = ReceiverType::Object {
            kind: ObjectKind::Report,
            name_lc: "rxmergebase3".into(),
            id: None,
        };
        let (shape, routes) =
            resolve_member(&receiver, "extproc", 0, from_obj, &graph, &index, &surface);

        assert_eq!(shape, DispatchShape::Exact);
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].target, RouteTarget::Unresolved);
        assert!(
            matches!(
                routes[0].evidence,
                Evidence::Unknown(UnknownReason::MemberNotFound)
            ),
            "an out-of-closure extension must be structurally invisible \
             (MemberNotFound), never surfaced via an access-exclusion \
             reason; got {:?}",
            routes[0].evidence
        );
    }

    // (T1-report-amb-1) TWO caller-visible ReportExtensions both declaring
    // the same viable member — genuine ambiguity, no first-wins (defensive:
    // AL0440 in real AL — corrected, Task 3 roadmap-closure plan: live-probed
    // [two `pageextension`s declaring the identical procedure both report
    // AL0440 "already defines a method ... with the same parameter types",
    // the identical class the base-vs-extension shape produces; not the
    // AL0226-class this comment previously guessed]).
    #[test]
    fn resolve_member_object_report_merge_two_extensions_same_member_is_ambiguous() {
        use crate::program::resolve::receiver::ReceiverType;

        let src_report: &'static str = r#"
report 62030 "RxMergeBase4"
{
    dataset
    {
    }

    procedure BaseProc()
    begin
    end;
}
"#;
        let src_ext_a: &'static str = r#"
reportextension 62031 "RxMergeBase4ExtA" extends "RxMergeBase4"
{
    procedure DupProc()
    begin
    end;
}
"#;
        let src_ext_b: &'static str = r#"
reportextension 62032 "RxMergeBase4ExtB" extends "RxMergeBase4"
{
    procedure DupProc()
    begin
    end;
}
"#;
        let src_caller: &'static str = r#"
codeunit 62033 "RxMergeCaller4"
{
    procedure Trigger()
    begin
    end;
}
"#;
        let app_id = make_app_id("RxMergeApp4");
        let unit_report = make_unit(app_id.clone(), "RxMergeBase4.al", src_report);
        let unit_ext_a = make_unit(app_id.clone(), "RxMergeBase4ExtA.al", src_ext_a);
        let unit_ext_b = make_unit(app_id.clone(), "RxMergeBase4ExtB.al", src_ext_b);
        let unit_caller = make_unit(app_id, "RxMergeCaller4.al", src_caller);
        let units = [unit_report, unit_ext_a, unit_ext_b, unit_caller];
        let graph = build_graph(&units, None);
        let index = ResolveIndex::build(&graph);
        let surface = DeclSurface::build(&graph, &units);

        let from_obj = find_obj(&graph, "RxMergeCaller4");
        let receiver = ReceiverType::Object {
            kind: ObjectKind::Report,
            name_lc: "rxmergebase4".into(),
            id: None,
        };
        let (shape, routes) =
            resolve_member(&receiver, "dupproc", 0, from_obj, &graph, &index, &surface);

        assert_eq!(shape, DispatchShape::Exact);
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].target, RouteTarget::Unresolved);
        assert!(
            matches!(
                routes[0].evidence,
                Evidence::Unknown(UnknownReason::OverloadAmbiguous)
            ),
            "two caller-visible ReportExtensions both declaring the same \
             viable member must decline as a genuine ambiguity — no \
             first-wins (AL0440, defensive-only against malformed input — \
             corrected Task 3 [live-probed, not the AL0226-class previously \
             guessed]); got {:?}",
            routes[0].evidence
        );
    }

    // (T1-report-arity-visible) a VISIBLE base-only candidate called with the
    // WRONG arity must still surface `ArityMismatch` (name found, wrong
    // arity) — the merge must not regress this pre-Task-1 per-object
    // diagnostic into a bare MemberNotFound/CatalogMiss. Grounded by the
    // al-compile probe on `resolve_in_report_scope`'s doc (AL0135).
    #[test]
    fn resolve_member_object_report_merge_visible_wrong_arity_preserves_arity_mismatch() {
        use crate::program::resolve::receiver::ReceiverType;

        let src_report: &'static str = r#"
report 62040 "RxMergeBase5"
{
    dataset
    {
    }

    procedure OneArgProc(X: Integer)
    begin
    end;
}
"#;
        let src_ext: &'static str = r#"
reportextension 62041 "RxMergeBase5Ext" extends "RxMergeBase5"
{
    procedure UnrelatedExtProc()
    begin
    end;
}
"#;
        let src_caller: &'static str = r#"
codeunit 62042 "RxMergeCaller5"
{
    procedure Trigger()
    begin
    end;
}
"#;
        let app_id = make_app_id("RxMergeApp5");
        let unit_report = make_unit(app_id.clone(), "RxMergeBase5.al", src_report);
        let unit_ext = make_unit(app_id.clone(), "RxMergeBase5Ext.al", src_ext);
        let unit_caller = make_unit(app_id, "RxMergeCaller5.al", src_caller);
        let units = [unit_report, unit_ext, unit_caller];
        let graph = build_graph(&units, None);
        let index = ResolveIndex::build(&graph);
        let surface = DeclSurface::build(&graph, &units);

        let from_obj = find_obj(&graph, "RxMergeCaller5");
        let receiver = ReceiverType::Object {
            kind: ObjectKind::Report,
            name_lc: "rxmergebase5".into(),
            id: None,
        };
        // Call with arity 0 — the only declared "OneArgProc" takes 1 param.
        let (shape, routes) = resolve_member(
            &receiver,
            "oneargproc",
            0,
            from_obj,
            &graph,
            &index,
            &surface,
        );

        assert_eq!(shape, DispatchShape::Exact);
        assert_eq!(routes.len(), 1);
        assert!(
            matches!(
                routes[0].evidence,
                Evidence::Unknown(UnknownReason::ArityMismatch)
            ),
            "a wrong-arity call to a VISIBLE base-only candidate must stay \
             ArityMismatch (name found), not collapse into MemberNotFound/ \
             CatalogMiss via the merge (the al-compile AL0135 probe); got \
             {:?}",
            routes[0].evidence
        );
    }

    // (T1-report-arity-invisible) an INVISIBLE (out-of-closure) extension
    // declaring the SAME name at the WRONG arity must NOT leak an
    // `ArityMismatch` — the closure filter runs BEFORE the zero-match
    // fallback ever sees the candidate, so an invisible wrong-arity
    // candidate stays structurally absent (MemberNotFound), exactly like the
    // out-of-closure fixture above.
    #[test]
    fn resolve_member_object_report_merge_invisible_wrong_arity_no_leak() {
        use crate::program::resolve::receiver::ReceiverType;

        let src_report: &'static str = r#"
report 62050 "RxMergeBase6"
{
    dataset
    {
    }
}
"#;
        let src_ext: &'static str = r#"
reportextension 62051 "RxMergeBase6Ext" extends "RxMergeBase6"
{
    procedure OneArgProc(X: Integer)
    begin
    end;
}
"#;
        let src_caller: &'static str = r#"
codeunit 62052 "RxMergeCaller6"
{
    procedure Trigger()
    begin
    end;
}
"#;
        // CallerApp depends ONLY on ReportApp — the extension (declaring
        // OneArgProc at arity 1) is entirely out of the caller's closure.
        let app_report = make_app_id("RxMergeReportApp6");
        let app_ext = make_app_id("RxMergeExtApp6");
        let app_caller = make_app_id("RxMergeCallerApp6");
        let unit_report = make_unit(app_report, "RxMergeBase6.al", src_report);
        let unit_ext = make_unit(app_ext, "RxMergeBase6Ext.al", src_ext);
        let unit_caller = make_unit(app_caller, "RxMergeCaller6.al", src_caller);
        let units = [unit_report, unit_ext, unit_caller];
        let graph = build_graph_multi_dep(
            &units,
            &[
                ("RxMergeCallerApp6", "RxMergeReportApp6"),
                ("RxMergeExtApp6", "RxMergeReportApp6"),
            ],
        );
        let index = ResolveIndex::build(&graph);
        let surface = DeclSurface::build(&graph, &units);

        let from_obj = find_obj(&graph, "RxMergeCaller6");
        let receiver = ReceiverType::Object {
            kind: ObjectKind::Report,
            name_lc: "rxmergebase6".into(),
            id: None,
        };
        // Call with arity 0 — the invisible extension's OneArgProc takes 1.
        let (shape, routes) = resolve_member(
            &receiver,
            "oneargproc",
            0,
            from_obj,
            &graph,
            &index,
            &surface,
        );

        assert_eq!(shape, DispatchShape::Exact);
        assert_eq!(routes.len(), 1);
        assert!(
            matches!(
                routes[0].evidence,
                Evidence::Unknown(UnknownReason::MemberNotFound)
            ),
            "an out-of-closure extension's wrong-arity candidate must NOT \
             leak ArityMismatch (it is invisible, full stop) — must stay \
             the generic MemberNotFound default; got {:?}",
            routes[0].evidence
        );
    }

    // (T1-report-arity-mixed) base declares the name at one arity, a VISIBLE
    // extension declares it at a DIFFERENT arity — calling with a THIRD arity
    // (matching neither) must deterministically land on ArityMismatch (via
    // the first scope-order name-bearing object), never Ambiguous and never
    // MemberNotFound.
    #[test]
    fn resolve_member_object_report_merge_mixed_base_extension_wrong_arity() {
        use crate::program::resolve::receiver::ReceiverType;

        let src_report: &'static str = r#"
report 62060 "RxMergeBase7"
{
    dataset
    {
    }

    procedure MixedProc(X: Integer)
    begin
    end;
}
"#;
        let src_ext: &'static str = r#"
reportextension 62061 "RxMergeBase7Ext" extends "RxMergeBase7"
{
    procedure MixedProc(X: Integer; Y: Integer)
    begin
    end;
}
"#;
        let src_caller: &'static str = r#"
codeunit 62062 "RxMergeCaller7"
{
    procedure Trigger()
    begin
    end;
}
"#;
        let app_id = make_app_id("RxMergeApp7");
        let unit_report = make_unit(app_id.clone(), "RxMergeBase7.al", src_report);
        let unit_ext = make_unit(app_id.clone(), "RxMergeBase7Ext.al", src_ext);
        let unit_caller = make_unit(app_id, "RxMergeCaller7.al", src_caller);
        let units = [unit_report, unit_ext, unit_caller];
        let graph = build_graph(&units, None);
        let index = ResolveIndex::build(&graph);
        let surface = DeclSurface::build(&graph, &units);

        let from_obj = find_obj(&graph, "RxMergeCaller7");
        let receiver = ReceiverType::Object {
            kind: ObjectKind::Report,
            name_lc: "rxmergebase7".into(),
            id: None,
        };
        // Call with arity 0 — base takes 1, extension takes 2; neither matches.
        let (shape, routes) = resolve_member(
            &receiver,
            "mixedproc",
            0,
            from_obj,
            &graph,
            &index,
            &surface,
        );

        assert_eq!(shape, DispatchShape::Exact);
        assert_eq!(routes.len(), 1);
        assert!(
            matches!(
                routes[0].evidence,
                Evidence::Unknown(UnknownReason::ArityMismatch)
            ),
            "a mixed base+extension wrong-arity call (neither side's arity \
             matches) must deterministically report ArityMismatch, never \
             Ambiguous (there is no arity+visibility match to BE ambiguous \
             between) and never a bare MemberNotFound; got {:?}",
            routes[0].evidence
        );
    }

    // (T1-report-base-only) base-only calls are unchanged by the merge: a
    // Report with an extension present (that does NOT declare the called
    // member) still resolves the base's own procedure exactly as
    // pre-Task-1, AND the instance-builtin catalog fallback still fires when
    // neither base nor any extension declares the name.
    #[test]
    fn resolve_member_object_report_merge_base_only_unchanged() {
        use crate::program::resolve::receiver::ReceiverType;

        let src_report: &'static str = r#"
report 62070 "RxMergeBase8"
{
    dataset
    {
    }

    procedure BaseProc()
    begin
    end;
}
"#;
        let src_ext: &'static str = r#"
reportextension 62071 "RxMergeBase8Ext" extends "RxMergeBase8"
{
    procedure UnrelatedExtProc()
    begin
    end;
}
"#;
        let src_caller: &'static str = r#"
codeunit 62072 "RxMergeCaller8"
{
    procedure Trigger()
    begin
    end;
}
"#;
        let app_id = make_app_id("RxMergeApp8");
        let unit_report = make_unit(app_id.clone(), "RxMergeBase8.al", src_report);
        let unit_ext = make_unit(app_id.clone(), "RxMergeBase8Ext.al", src_ext);
        let unit_caller = make_unit(app_id, "RxMergeCaller8.al", src_caller);
        let units = [unit_report, unit_ext, unit_caller];
        let graph = build_graph(&units, None);
        let index = ResolveIndex::build(&graph);
        let surface = DeclSurface::build(&graph, &units);

        let from_obj = find_obj(&graph, "RxMergeCaller8");
        let receiver = ReceiverType::Object {
            kind: ObjectKind::Report,
            name_lc: "rxmergebase8".into(),
            id: None,
        };

        // The base's own procedure still resolves to Source.
        let (shape, routes) =
            resolve_member(&receiver, "baseproc", 0, from_obj, &graph, &index, &surface);
        assert_eq!(shape, DispatchShape::Exact);
        assert_eq!(routes.len(), 1);
        assert!(matches!(routes[0].target, RouteTarget::Routine(_)));
        assert_eq!(routes[0].evidence, Evidence::Source);

        // A genuine platform-intrinsic (ReportInstance catalog) member absent
        // from both base and extension still falls through to Catalog.
        let (shape2, routes2) = resolve_member(
            &receiver,
            "saveaspdf",
            0,
            from_obj,
            &graph,
            &index,
            &surface,
        );
        assert_eq!(shape2, DispatchShape::Exact);
        assert_eq!(routes2.len(), 1);
        assert_eq!(routes2[0].evidence, Evidence::Catalog);
    }

    // (B-pos) bare call to the caller's own `local` procedure (gap B is
    // pre-gated/self-no-op — this pins that threading `from_object` through
    // Step 1 changed nothing observable).
    #[test]
    fn bare_own_local_procedure_resolves_to_source() {
        let src: &'static str = r#"
codeunit 53930 "BareLocalSelf"
{
    local procedure DoFoo()
    begin
    end;
}
"#;
        let app_id = make_app_id("TestApp");
        let unit = make_unit(app_id, "BareLocalSelf.al", src);
        let units = [unit];
        let graph = build_graph(&units, None);
        let index = ResolveIndex::build(&graph);
        let surface = DeclSurface::build(&graph, &units);

        let from_obj = find_obj(&graph, "BareLocalSelf");
        let (_shape, routes) = resolve_bare(
            from_obj,
            "dofoo",
            0,
            &graph,
            &index,
            &surface,
            WithState::NoWithProven,
        );

        assert_eq!(routes.len(), 1);
        assert!(
            matches!(routes[0].target, RouteTarget::Routine(_)),
            "a bare call to the caller's OWN `local` procedure must resolve \
             to Source (gap B self-no-op control); got {:?}",
            routes[0].target
        );
        assert_eq!(routes[0].evidence, Evidence::Source);
    }

    // (E-pos) `this.LocalProc()` — SelfObject receiver dispatching to the
    // caller's own `local` procedure (gap E is pre-gated/self-no-op).
    #[test]
    fn resolve_member_self_object_local_procedure_resolves_to_source() {
        use crate::program::resolve::receiver::ReceiverType;

        let src: &'static str = r#"
codeunit 53940 "SelfLocalCaller"
{
    local procedure LocalProc()
    begin
    end;

    procedure Trigger()
    begin
        this.LocalProc();
    end;
}
"#;
        let app_id = make_app_id("TestApp");
        let unit = make_unit(app_id, "SelfLocalCaller.al", src);
        let units = [unit];
        let graph = build_graph(&units, None);
        let index = ResolveIndex::build(&graph);
        let surface = DeclSurface::build(&graph, &units);

        let from_obj = find_obj(&graph, "SelfLocalCaller");
        let (shape, routes) = resolve_member(
            &ReceiverType::SelfObject,
            "localproc",
            0,
            from_obj,
            &graph,
            &index,
            &surface,
        );

        assert_eq!(shape, DispatchShape::Exact);
        assert_eq!(routes.len(), 1);
        assert!(
            matches!(routes[0].target, RouteTarget::Routine(_)),
            "`this.LocalProc()` (SelfObject receiver) must resolve its own \
             `local` procedure to Source (gap E self-no-op control); got {:?}",
            routes[0].target
        );
        assert_eq!(routes[0].evidence, Evidence::Source);
    }

    // --- NEGATIVES: must become honest Unknown ------------------------------

    // (D-neg-1) Object receiver, cross-app `internal` — pre-fix this
    // false-resolved to `RouteTarget::Routine(IntNTarget.Secret)`.
    #[test]
    fn resolve_member_object_cross_app_internal_method_excluded() {
        use crate::program::resolve::receiver::ReceiverType;

        let src_target: &'static str = r#"
codeunit 53950 "IntNTarget"
{
    internal procedure Secret()
    begin
    end;
}
"#;
        let src_caller: &'static str = r#"
codeunit 53951 "IntNCaller"
{
    procedure Trigger()
    begin
    end;
}
"#;
        let app_a = make_app_id("PrimaryApp2");
        let app_b = make_app_id("DepApp2");
        let unit_target = make_unit(app_b, "IntNTarget.al", src_target);
        let unit_caller = make_unit(app_a, "IntNCaller.al", src_caller);
        let units = [unit_target, unit_caller];
        let graph = build_graph_multi_dep(&units, &[("PrimaryApp2", "DepApp2")]);
        let index = ResolveIndex::build(&graph);
        let surface = DeclSurface::build(&graph, &units);

        let from_obj = find_obj(&graph, "IntNCaller");
        let receiver = ReceiverType::Object {
            kind: ObjectKind::Codeunit,
            name_lc: "intntarget".into(),
            id: None,
        };
        let (shape, routes) =
            resolve_member(&receiver, "secret", 0, from_obj, &graph, &index, &surface);

        assert_eq!(shape, DispatchShape::Exact);
        assert_eq!(routes.len(), 1);
        assert_eq!(
            routes[0].target,
            RouteTarget::Unresolved,
            "a cross-app `internal` method reached via an Object receiver \
             must NOT resolve to Source (gap D — pre-fix this false-resolved \
             to RouteTarget::Routine(IntNTarget.Secret)); got {:?}",
            routes[0].target
        );
        assert!(matches!(routes[0].evidence, Evidence::Unknown(_)));
    }

    // -----------------------------------------------------------------------
    // Task 1.5 (feat/resolve-access-uniform-and-compound-receiver, inserted
    // after Task 1): `internalsVisibleTo` friend apps. Task 1 correctly fails
    // closed on cross-app `internal`, but 100% of the resulting
    // `InternalNotVisible` bucket measured against CDO turned out to be
    // AL-LEGAL friend calls (the declaring app's manifest explicitly lists
    // the caller app in `<InternalsVisibleTo>`). `internal_visible_across`
    // (above) models the friend exception; these fixtures pin the full
    // matrix: friend-authorized resolves, a true-stranger control still
    // declines, friendship doesn't imply the reverse direction, and same-app
    // `internal` is unaffected. See `.superpowers/sdd/task-1.5-report.md` and
    // `tests/r0-corpus/ws-friend-app-internal/` for the compiler-semantics
    // writeup.
    // -----------------------------------------------------------------------

    // (1.5-a) cross-app `internal`, declaring app lists the caller as a
    // friend → must resolve to Source. Pre-fix (Task 1 alone) this is
    // `Unknown`/`InternalNotVisible` — asserted exactly below before the fix
    // narrative note (kept as documentation of the exact prior route; the
    // assertion itself checks the POST-fix behavior since this test file
    // only ships the fixed code).
    #[test]
    fn resolve_member_object_cross_app_internal_friend_authorized_resolves_to_source() {
        use crate::program::resolve::receiver::ReceiverType;

        let src_target: &'static str = r#"
codeunit 53970 "FriendTarget"
{
    internal procedure Secret()
    begin
    end;
}
"#;
        let src_caller: &'static str = r#"
codeunit 53971 "FriendCaller"
{
    procedure Trigger()
    begin
    end;
}
"#;
        // FriendTarget's app ("DepAppFriend") declares FriendCaller's app
        // ("PrimaryAppFriend") a friend via <InternalsVisibleTo>.
        let app_a = make_app_id("PrimaryAppFriend");
        let app_b = make_app_id("DepAppFriend");
        let unit_target = make_unit(app_b, "FriendTarget.al", src_target);
        let unit_caller = make_unit(app_a, "FriendCaller.al", src_caller);
        let units = [unit_target, unit_caller];
        let graph = build_graph_multi_dep_friends(
            &units,
            &[("PrimaryAppFriend", "DepAppFriend")],
            &[("DepAppFriend", "PrimaryAppFriend")],
        );
        let index = ResolveIndex::build(&graph);
        let surface = DeclSurface::build(&graph, &units);

        let from_obj = find_obj(&graph, "FriendCaller");
        let receiver = ReceiverType::Object {
            kind: ObjectKind::Codeunit,
            name_lc: "friendtarget".into(),
            id: None,
        };
        let (shape, routes) =
            resolve_member(&receiver, "secret", 0, from_obj, &graph, &index, &surface);

        assert_eq!(shape, DispatchShape::Exact);
        assert_eq!(routes.len(), 1);
        assert!(
            matches!(routes[0].target, RouteTarget::Routine(_)),
            "a cross-app `internal` method whose declaring app lists the \
             caller as an InternalsVisibleTo friend must resolve to Source \
             (Task 1.5); got {:?}",
            routes[0].target
        );
        assert_eq!(routes[0].evidence, Evidence::Source);
    }

    // (1.5-b) CONTROL: cross-app `internal`, declaring app does NOT list the
    // caller as a friend (a true stranger) — must stay honest Unknown.
    #[test]
    fn resolve_member_object_cross_app_internal_stranger_control_stays_unknown() {
        use crate::program::resolve::receiver::ReceiverType;

        let src_target: &'static str = r#"
codeunit 53972 "StrangerTarget"
{
    internal procedure Secret()
    begin
    end;
}
"#;
        let src_caller: &'static str = r#"
codeunit 53973 "StrangerCaller"
{
    procedure Trigger()
    begin
    end;
}
"#;
        let app_a = make_app_id("PrimaryAppStranger");
        let app_b = make_app_id("DepAppStranger");
        let unit_target = make_unit(app_b, "StrangerTarget.al", src_target);
        let unit_caller = make_unit(app_a, "StrangerCaller.al", src_caller);
        let units = [unit_target, unit_caller];
        // No friends entry at all — DepAppStranger declares NO friends.
        let graph =
            build_graph_multi_dep_friends(&units, &[("PrimaryAppStranger", "DepAppStranger")], &[]);
        let index = ResolveIndex::build(&graph);
        let surface = DeclSurface::build(&graph, &units);

        let from_obj = find_obj(&graph, "StrangerCaller");
        let receiver = ReceiverType::Object {
            kind: ObjectKind::Codeunit,
            name_lc: "strangertarget".into(),
            id: None,
        };
        let (shape, routes) =
            resolve_member(&receiver, "secret", 0, from_obj, &graph, &index, &surface);

        assert_eq!(shape, DispatchShape::Exact);
        assert_eq!(routes.len(), 1);
        assert_eq!(
            routes[0].target,
            RouteTarget::Unresolved,
            "a cross-app `internal` method whose declaring app does NOT \
             list the caller as a friend (a true stranger) must stay \
             honest Unknown (Task 1.5 control — declining a stranger is \
             sound, not an over-decline); got {:?}",
            routes[0].target
        );
        assert!(matches!(
            routes[0].evidence,
            Evidence::Unknown(UnknownReason::InternalNotVisible)
        ));
    }

    // (1.5-c) DIRECTIONALITY: friendship is declared BY the exposing app and
    // is never inherited by the app it names. App A declares App B a friend
    // (`friends[A] = {B}`), so B → A internal resolves. That grant is
    // one-directional: it must NOT be readable backwards as `friends[B] =
    // {A}`. The original version of this fixture only asserted the GRANTED
    // direction (B → A) plus a same-app B → B sanity check, never the actual
    // REVERSE call (A → B, where B declares no friends of its own) — so a
    // bidirectionality bug in `internal_visible_across` could have slipped
    // through untested (Task 1.5 review, minor finding (a), folded in here).
    // This now exercises all three: B → A resolves `Source` (granted); B → B
    // same-app sanity check; A → B, calling an `internal` member of the app
    // that RECEIVED trust rather than granted it, stays honest `Unknown`
    // (no reciprocal grant exists).
    #[test]
    fn resolve_member_object_cross_app_internal_friendship_not_bidirectional() {
        use crate::program::resolve::receiver::ReceiverType;

        let src_a_target: &'static str = r#"
codeunit 53974 "DirATarget"
{
    internal procedure SecretA()
    begin
    end;
}
"#;
        let src_b_target: &'static str = r#"
codeunit 53975 "DirBTarget"
{
    internal procedure SecretB()
    begin
    end;
}
"#;
        let src_b_caller: &'static str = r#"
codeunit 53976 "DirBCaller"
{
    procedure Trigger()
    begin
    end;
}
"#;
        let src_a_caller: &'static str = r#"
codeunit 53979 "DirACaller"
{
    procedure Trigger()
    begin
    end;
}
"#;
        let app_a = make_app_id("PrimaryAppDirA");
        let app_b = make_app_id("DepAppDirB");
        let unit_a_target = make_unit(app_a.clone(), "DirATarget.al", src_a_target);
        let unit_b_target = make_unit(app_b.clone(), "DirBTarget.al", src_b_target);
        let unit_b_caller = make_unit(app_b.clone(), "DirBCaller.al", src_b_caller);
        let unit_a_caller = make_unit(app_a, "DirACaller.al", src_a_caller);
        let units = [unit_a_target, unit_b_target, unit_b_caller, unit_a_caller];
        // Dependencies are declared BOTH ways (B depends on A, AND A depends
        // on B) purely so each app's caller is topologically able to reach
        // the other app's object — this fixture tests the resolver's access
        // predicate in isolation, not real-world AL's (acyclic)
        // dependency-graph constraints. Friends are declared ONE way only:
        // App A lists App B as a friend (`friends[A] = {B}`); App B declares
        // NO friends of its own (`friends[B]` absent). So B → A internal is
        // friend-authorized; A → B internal is NOT (B never granted A
        // anything) — there is no "friends[A].contains(B) implies
        // friends[B].contains(A)" shortcut in the implementation, and this
        // fixture proves it by actually calling in that direction, not just
        // asserting same-app control cases.
        let graph = build_graph_multi_dep_friends(
            &units,
            &[
                ("DepAppDirB", "PrimaryAppDirA"),
                ("PrimaryAppDirA", "DepAppDirB"),
            ],
            &[("PrimaryAppDirA", "DepAppDirB")],
        );
        let index = ResolveIndex::build(&graph);
        let surface = DeclSurface::build(&graph, &units);

        let from_obj = find_obj(&graph, "DirBCaller");

        // B → A: A declared B a friend → resolves Source.
        let receiver_a = ReceiverType::Object {
            kind: ObjectKind::Codeunit,
            name_lc: "diratarget".into(),
            id: None,
        };
        let (shape_a, routes_a) = resolve_member(
            &receiver_a,
            "secreta",
            0,
            from_obj,
            &graph,
            &index,
            &surface,
        );
        assert_eq!(shape_a, DispatchShape::Exact);
        assert_eq!(routes_a.len(), 1);
        assert!(
            matches!(routes_a[0].target, RouteTarget::Routine(_)),
            "B → A: A's manifest lists B as a friend, must resolve to \
             Source; got {:?}",
            routes_a[0].target
        );

        // B → B's own DirBTarget: same-app, unaffected — sanity check the
        // fixture's own app is still internally consistent.
        let receiver_b = ReceiverType::Object {
            kind: ObjectKind::Codeunit,
            name_lc: "dirbtarget".into(),
            id: None,
        };
        let (shape_b, routes_b) = resolve_member(
            &receiver_b,
            "secretb",
            0,
            from_obj,
            &graph,
            &index,
            &surface,
        );
        assert_eq!(shape_b, DispatchShape::Exact);
        assert_eq!(routes_b.len(), 1);
        assert!(
            matches!(routes_b[0].target, RouteTarget::Routine(_)),
            "B → B: same-app internal must still resolve to Source \
             (directionality fixture sanity check); got {:?}",
            routes_b[0].target
        );

        // A → B: A declared B a friend, but that grant runs ONE way — B
        // declared no friends of its own, so A calling B's `internal`
        // member must stay honest Unknown. This is the actual reverse-
        // direction proof: if `internal_visible_across` ever regressed to
        // a symmetric/bidirectional check, this assertion (not merely the
        // same-app B → B control above) would catch it.
        let from_obj_a = find_obj(&graph, "DirACaller");
        let receiver_b_target = ReceiverType::Object {
            kind: ObjectKind::Codeunit,
            name_lc: "dirbtarget".into(),
            id: None,
        };
        let (shape_rev, routes_rev) = resolve_member(
            &receiver_b_target,
            "secretb",
            0,
            from_obj_a,
            &graph,
            &index,
            &surface,
        );
        assert_eq!(shape_rev, DispatchShape::Exact);
        assert_eq!(routes_rev.len(), 1);
        assert_eq!(
            routes_rev[0].target,
            RouteTarget::Unresolved,
            "A → B: B never friended A back (friendship is declared BY \
             the exposing app, never inherited), so this must stay \
             honest Unknown, not Source; got {:?}",
            routes_rev[0].target
        );
        assert!(
            matches!(
                routes_rev[0].evidence,
                Evidence::Unknown(UnknownReason::InternalNotVisible)
            ),
            "A → B must be excluded with InternalNotVisible, not some \
             other reason; got {:?}",
            routes_rev[0].evidence
        );
    }

    // (1.5-d) same-app `internal` is unaffected by friend modeling (no
    // friends declared at all) — unchanged Task 1 positive control,
    // re-pinned here to keep the Task 1.5 matrix self-contained.
    #[test]
    fn resolve_member_object_same_app_internal_unaffected_by_friend_modeling() {
        use crate::program::resolve::receiver::ReceiverType;

        let src_target: &'static str = r#"
codeunit 53977 "SameAppTarget"
{
    internal procedure DoWork()
    begin
    end;
}
"#;
        let src_caller: &'static str = r#"
codeunit 53978 "SameAppCaller"
{
    procedure Trigger()
    begin
    end;
}
"#;
        let app_id = make_app_id("SoloFriendApp");
        let unit_target = make_unit(app_id.clone(), "SameAppTarget.al", src_target);
        let unit_caller = make_unit(app_id, "SameAppCaller.al", src_caller);
        let units = [unit_target, unit_caller];
        let graph = build_graph_multi_dep_friends(&units, &[], &[]);
        let index = ResolveIndex::build(&graph);
        let surface = DeclSurface::build(&graph, &units);

        let from_obj = find_obj(&graph, "SameAppCaller");
        let receiver = ReceiverType::Object {
            kind: ObjectKind::Codeunit,
            name_lc: "sameapptarget".into(),
            id: None,
        };
        let (shape, routes) =
            resolve_member(&receiver, "dowork", 0, from_obj, &graph, &index, &surface);

        assert_eq!(shape, DispatchShape::Exact);
        assert_eq!(routes.len(), 1);
        assert!(
            matches!(routes[0].target, RouteTarget::Routine(_)),
            "same-app internal must still resolve to Source with zero \
             friends declared (Task 1.5 unaffected-scope control); got {:?}",
            routes[0].target
        );
        assert_eq!(routes[0].evidence, Evidence::Source);
    }

    // (D-neg-2) Object receiver, same-app but DIFFERENT (non-extension)
    // object's `local` — pre-fix this false-resolved to
    // `RouteTarget::Routine(LocNTarget.Hidden)`.
    #[test]
    fn resolve_member_object_same_app_local_cross_object_excluded() {
        use crate::program::resolve::receiver::ReceiverType;

        let src_target: &'static str = r#"
codeunit 53960 "LocNTarget"
{
    local procedure Hidden()
    begin
    end;
}
"#;
        let src_caller: &'static str = r#"
codeunit 53961 "LocNCaller"
{
    procedure Trigger()
    begin
    end;
}
"#;
        let app_id = make_app_id("TestApp");
        let unit_target = make_unit(app_id.clone(), "LocNTarget.al", src_target);
        let unit_caller = make_unit(app_id, "LocNCaller.al", src_caller);
        let units = [unit_target, unit_caller];
        let graph = build_graph(&units, None);
        let index = ResolveIndex::build(&graph);
        let surface = DeclSurface::build(&graph, &units);

        let from_obj = find_obj(&graph, "LocNCaller");
        let receiver = ReceiverType::Object {
            kind: ObjectKind::Codeunit,
            name_lc: "locntarget".into(),
            id: None,
        };
        let (shape, routes) =
            resolve_member(&receiver, "hidden", 0, from_obj, &graph, &index, &surface);

        assert_eq!(shape, DispatchShape::Exact);
        assert_eq!(routes.len(), 1);
        assert_eq!(
            routes[0].target,
            RouteTarget::Unresolved,
            "a same-app but DIFFERENT object's `local` method reached via an \
             Object receiver must NOT resolve to Source (gap D — AL `local` \
             is OBJECT-scoped, not app-scoped; pre-fix this false-resolved \
             to RouteTarget::Routine(LocNTarget.Hidden)); got {:?}",
            routes[0].target
        );
        assert!(matches!(routes[0].evidence, Evidence::Unknown(_)));
    }

    // (D-neg-3) THE OVERLOAD-NARROWING GUARD: two textually distinct
    // same-arity overloads of `Foo` (Integer vs Text) differing only in
    // access modifier + param TYPE. STALE-CLAIM CORRECTION (Task 5 nit
    // sweep, 2026-07-04): this comment used to assert `RoutineNodeId`
    // collides for both here because "source `sig_fp` is always `0`" —
    // true only PRE the sigfp-and-ambiguous-reclassification plan's Task 2
    // (2026-07-03); post-fix, source `sig_fp` is a REAL per-parameter-type
    // fingerprint (`sig_fp::source_param_sig_fp`; see `resolve_in_object`'s
    // doc a few thousand lines above, which already carries the accurate
    // correction), so `Foo(Integer)`/`Foo(Text)` now get genuinely DISTINCT
    // `RoutineNodeId`s (proved directly by `sig_fp.rs`'s
    // `distinct_param_types_never_collide` unit test) — they do NOT
    // collide today. The guard this fixture proves does not depend on id
    // collision either way: `resolve_in_object`'s `pre_filter_count` counts
    // every arity-matched candidate regardless of whether their ids are
    // identical or distinct, so the pre-filter set is genuinely ambiguous
    // (2 same-arity candidates) purely from that count. Calling cross-app
    // with 1 (unproven-type) argument must NEVER resolve to Source, even
    // though exactly one physical overload (`Foo(Integer)`, `public`)
    // happens to be visible and the other (`Foo(Text)`, `internal`) is
    // cross-app-excluded — access alone cannot prove which overload the
    // call meant.
    #[test]
    fn resolve_member_object_mixed_access_same_arity_overload_never_resolves_to_source() {
        use crate::program::resolve::receiver::ReceiverType;

        let src_target: &'static str = r#"
codeunit 53970 "OverloadNTarget"
{
    procedure Foo(p: Integer)
    begin
    end;

    internal procedure Foo(p: Text)
    begin
    end;
}
"#;
        let src_caller: &'static str = r#"
codeunit 53971 "OverloadNCaller"
{
    procedure Trigger()
    begin
    end;
}
"#;
        let app_a = make_app_id("PrimaryApp3");
        let app_b = make_app_id("DepApp3");
        let unit_target = make_unit(app_b, "OverloadNTarget.al", src_target);
        let unit_caller = make_unit(app_a, "OverloadNCaller.al", src_caller);
        let units = [unit_target, unit_caller];
        let graph = build_graph_multi_dep(&units, &[("PrimaryApp3", "DepApp3")]);
        let index = ResolveIndex::build(&graph);
        let surface = DeclSurface::build(&graph, &units);

        // Sanity: both overloads really did survive as one params_count==1
        // collision (proves the fixture actually exercises the guard, not a
        // degenerate single-candidate case).
        let target_obj = find_obj(&graph, "OverloadNTarget");
        let foo_candidates = index.routines_in_object(&target_obj.id, "foo");
        assert_eq!(
            foo_candidates.len(),
            2,
            "fixture must produce TWO same-arity `Foo` candidates; got {:?}",
            foo_candidates
        );

        let from_obj = find_obj(&graph, "OverloadNCaller");
        let receiver = ReceiverType::Object {
            kind: ObjectKind::Codeunit,
            name_lc: "overloadntarget".into(),
            id: None,
        };
        let (shape, routes) =
            resolve_member(&receiver, "foo", 1, from_obj, &graph, &index, &surface);

        assert_eq!(shape, DispatchShape::Exact);
        assert_eq!(routes.len(), 1);
        assert!(
            !matches!(routes[0].target, RouteTarget::Routine(_)),
            "mixed-access same-arity overload (public Foo(Integer) + \
             internal Foo(Text)) called cross-app with an unproven-type arg \
             must NEVER resolve to Source — access-narrowing to the lone \
             visible overload would manufacture a false resolution (the \
             overload-narrowing guard); got {:?}",
            routes[0].target
        );
        assert_eq!(routes[0].target, RouteTarget::Unresolved);
        // NOTE (reason-split Task 2 investigation; STALE-CLAIM CORRECTED
        // Task 5 nit sweep, 2026-07-04): this comment originally claimed
        // the fixture's TWO same-arity SOURCE overloads (`Foo(Integer)`/
        // `Foo(Text)`) shared an IDENTICAL `RoutineNodeId` because "source
        // `sig_fp` is always 0" — true only PRE the
        // sigfp-and-ambiguous-reclassification plan's Task 2 (2026-07-03).
        // Verified directly (debug-printed `foo_candidates` on this exact
        // fixture): post-fix the two DO get genuinely distinct `sig_fp`s
        // (`69875687941676757` vs `7629489990184319135`), so there is no
        // `binary_search_by` id-collision non-determinism here anymore —
        // the observed reason today is deterministically
        // `Unknown(AccessFilteredOverload)` (verified, not `InternalNotVisible`
        // as this comment used to describe), matching the SAME
        // `AccessFilteredOverload` shape the sibling
        // `resolve_member_object_two_distinct_sig_fp_overloads_access_narrowed_to_one_declines`
        // test below deliberately constructs — the two tests are no longer
        // meaningfully different w.r.t. id-collision, only in HOW the
        // distinct ids arise (real source fingerprinting here vs. manual
        // construction there). Left as the original generic
        // `Evidence::Unknown(_)` assertion regardless (pinning the specific
        // reason isn't this fixture's job).
        assert!(matches!(routes[0].evidence, Evidence::Unknown(_)));
    }

    /// Reason-split Task 2 fixture: an `AccessFilteredOverload` probe that
    /// manually constructs the graph (mirrors
    /// `plain_dispatch_marker_guard_fixture`'s pattern) with two DISTINCT
    /// `sig_fp` values so the two same-arity candidates get genuinely
    /// DISTINCT `RoutineNodeId`s. (STALE-CLAIM CORRECTED, Task 5 nit sweep,
    /// 2026-07-04: this doc used to frame that as "sidestepping" a
    /// same-`id` collision the sibling test above "documents" — that framing
    /// predates the sigfp-and-ambiguous-reclassification plan's Task 2 fix;
    /// post-fix, source `sig_fp` is a real per-parameter-type fingerprint,
    /// so the sibling test's real-source-parsed overloads ALSO get distinct
    /// ids today, verified — see that test's own corrected NOTE. This
    /// fixture's manual construction is no longer a workaround for a
    /// collision the sibling test suffers; it is simply a more explicit,
    /// hand-controlled probe of the identical `AccessFilteredOverload`
    /// shape.) One candidate `Public` (always visible), one `Internal` (excluded
    /// cross-app, no friendship declared) — access narrows the ORIGINALLY
    /// `pre_filter_count == 2` set down to exactly ONE visible survivor, and
    /// the resolver must decline rather than select it.
    #[test]
    fn resolve_member_object_two_distinct_sig_fp_overloads_access_narrowed_to_one_declines() {
        use crate::program::resolve::receiver::ReceiverType;

        let ws_id = make_app_id("AccessFilteredWS");
        let dep_id = make_app_id("AccessFilteredDep");

        let mut apps = AppRegistry::default();
        let ws_ref = apps.intern(&ws_id);
        let dep_ref = apps.intern(&dep_id);

        let caller_obj_id = ObjectNodeId {
            app: ws_ref,
            kind: ObjectKind::Codeunit,
            key: ObjKey::Id(53990),
        };
        let target_obj_id = ObjectNodeId {
            app: dep_ref,
            kind: ObjectKind::Codeunit,
            key: ObjKey::Id(63990),
        };

        let objects = vec![
            ObjectNode {
                id: caller_obj_id.clone(),
                name: "AccFilteredCaller".into(),
                declared_id: Some(53990),
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
            },
            ObjectNode {
                id: target_obj_id.clone(),
                name: "AccFilteredTarget".into(),
                declared_id: Some(63990),
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
            },
        ];

        fn overload(
            target_obj_id: ObjectNodeId,
            sig_fp: u64,
            access: Access,
            param_sig_key: &str,
        ) -> RoutineNode {
            RoutineNode {
                id: RoutineNodeId {
                    object: target_obj_id,
                    name_lc: "foo".into(),
                    enclosing_member_lc: None,
                    params_count: 1,
                    sig_fp,
                },
                name: "Foo".into(),
                is_trigger: false,
                access,
                tier: TrustTier::Workspace,
                event_subscribers: vec![],
                subscriber_instance_manual: false,
                publisher_kind: None,
                include_sender: None,
                abi_routine_kind: Some(AbiRoutineKind::Procedure),
                abi_event_kind: Some(AbiEventKind::None),
                param_sig_key: param_sig_key.into(),
                return_type: None,
                return_type_id: None,
                abi_overload_collapsed: false,
                source_overload_aliased: false,
                abi_params: AbiParams::Missing,
            }
        }
        let routines = vec![
            overload(target_obj_id.clone(), 100, Access::Public, "integer"),
            overload(target_obj_id.clone(), 200, Access::Internal, "text"),
        ];

        let mut topology = DependencyGraph::default();
        topology.add_dependency(ws_ref, dep_ref);

        let obj_index = ObjectIndex::build(&objects);
        let graph = ProgramGraph {
            apps,
            topology,
            objects,
            routines,
            obj_index,
            ..Default::default()
        };
        let index = ResolveIndex::build(&graph);
        let surface = DeclSurface::build(&graph, &[]);

        // Sanity: two GENUINELY distinct RoutineNodeIds (differing sig_fp),
        // not a same-id collision.
        let candidates = index.routines_in_object(&target_obj_id, "foo");
        assert_eq!(
            candidates.len(),
            2,
            "fixture must produce TWO `foo` candidates"
        );
        assert_ne!(
            candidates[0].sig_fp, candidates[1].sig_fp,
            "fixture sanity: the two overloads must be genuinely distinct RoutineNodeIds"
        );

        let from_obj = graph
            .objects
            .iter()
            .find(|o| o.id == caller_obj_id)
            .expect("caller must exist");
        let receiver = ReceiverType::Object {
            kind: ObjectKind::Codeunit,
            name_lc: "accfilteredtarget".into(),
            id: None,
        };
        let (shape, routes) =
            resolve_member(&receiver, "foo", 1, from_obj, &graph, &index, &surface);

        assert_eq!(shape, DispatchShape::Exact);
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].target, RouteTarget::Unresolved);
        assert_eq!(
            routes[0].evidence,
            Evidence::Unknown(UnknownReason::AccessFilteredOverload),
            "access narrowed an originally-ambiguous (pre_filter_count==2) \
             same-arity set down to ONE visible survivor (Public) and \
             declined rather than select it — reason-split Task 2's \
             AccessFilteredOverload label; got {:?}",
            routes[0].evidence
        );
        assert_eq!(
            routes[0].receiver_tier, None,
            "AccessFilteredOverload is not a MemberNotFound shape — no receiver_tier"
        );
    }

    /// Task 4 (sigfp-and-ambiguous-reclassification plan) fixture (a): a
    /// GENUINE >1-visible same-arity overload (BOTH candidates `Public`, so
    /// access filtering removes NEITHER) is now candidate-carrying
    /// `AmbiguousResolved` — TWO concrete `Source` routes (one per overload,
    /// distinct post-T2 `RoutineNodeId`s via real `sig_fp`), each tagged
    /// `Condition::AmbiguousDispatch` and `fires_by_default() == false`, the
    /// edge `SetCompleteness::Complete`, and `classify_obligation` /
    /// `Histogram` both agreeing this is NOT `unknown` — the pre-Task-4
    /// behavior (single `Unresolved(OverloadAmbiguous)` route, `Exact`
    /// shape) this test used to pin. This is the ONLY same-object overload
    /// ambiguity shape henceforth; `AccessFilteredOverload` (the sibling test
    /// above, where access narrows the visible set to exactly one) is
    /// unaffected.
    #[test]
    fn resolve_member_object_genuine_two_public_same_arity_overload_becomes_ambiguous_resolved() {
        use crate::program::resolve::receiver::ReceiverType;

        let src_target: &'static str = r#"
codeunit 53972 "OverloadPTarget"
{
    procedure Bar(p: Integer)
    begin
    end;

    procedure Bar(p: Text)
    begin
    end;
}
"#;
        let src_caller: &'static str = r#"
codeunit 53973 "OverloadPCaller"
{
    procedure Trigger()
    begin
    end;
}
"#;
        let app_id = make_app_id("TestApp");
        let unit_target = make_unit(app_id.clone(), "OverloadPTarget.al", src_target);
        let unit_caller = make_unit(app_id, "OverloadPCaller.al", src_caller);
        let units = [unit_target, unit_caller];
        let graph = build_graph(&units, None);
        let index = ResolveIndex::build(&graph);
        let surface = DeclSurface::build(&graph, &units);

        // Sanity: two same-arity `Bar` candidates, BOTH `Public`, DISTINCT ids.
        let target_obj = find_obj(&graph, "OverloadPTarget");
        let bar_candidates = index.routines_in_object(&target_obj.id, "bar");
        assert_eq!(
            bar_candidates.len(),
            2,
            "fixture must produce TWO `Bar` candidates"
        );
        assert_ne!(
            bar_candidates[0], bar_candidates[1],
            "the two overloads must be genuinely distinct post-T2 RoutineNodeIds"
        );

        let from_obj = find_obj(&graph, "OverloadPCaller");
        let receiver = ReceiverType::Object {
            kind: ObjectKind::Codeunit,
            name_lc: "overloadptarget".into(),
            id: None,
        };
        let (shape, routes) =
            resolve_member(&receiver, "bar", 1, from_obj, &graph, &index, &surface);

        assert_eq!(shape, DispatchShape::AmbiguousOverload);
        assert_eq!(routes.len(), 2, "one route per candidate; got {routes:?}");

        let mut seen_ids = std::collections::HashSet::new();
        for r in &routes {
            assert_eq!(r.evidence, Evidence::Source, "got {r:?}");
            assert!(
                r.conditions.contains(&Condition::AmbiguousDispatch),
                "every candidate route must carry AmbiguousDispatch; got {r:?}"
            );
            assert!(
                !r.fires_by_default(),
                "an AmbiguousDispatch route must not fire by default; got {r:?}"
            );
            let RouteTarget::Routine(ref rid) = r.target else {
                panic!("expected a Routine target; got {r:?}");
            };
            assert!(
                seen_ids.insert(rid.clone()),
                "candidate routes must target DISTINCT RoutineNodeIds; got {routes:?}"
            );
        }

        // Excluded from default/must traversal...
        let edge = Edge {
            from: bar_candidates[0].clone(),
            site: SiteId {
                caller: bar_candidates[0].clone(),
                span: CanonicalSpan {
                    unit: "OverloadPCaller.al".into(),
                    start: SourcePos { line: 1, col: 1 },
                    end: SourcePos { line: 1, col: 1 },
                },
                callee_fingerprint: 0,
            },
            kind: EdgeKind::Call,
            shape,
            completeness: SetCompleteness::Complete,
            routes: routes.clone(),
        };
        assert_eq!(
            edge.default_reachable_routes().count(),
            0,
            "AmbiguousDispatch routes must be excluded from default reachability"
        );
        // ...but INCLUDED in may-reachability (the round-1 "inverse cardinal
        // sin" addendum: exactly one candidate WILL fire, so a may/
        // change-impact traversal must see BOTH).
        assert_eq!(
            edge.may_reachable_routes().count(),
            2,
            "AmbiguousDispatch routes MUST be may-reachable"
        );

        assert_eq!(shape, DispatchShape::AmbiguousOverload);
        // `completeness_for_shape(AmbiguousOverload) == Complete` (the
        // candidate set is snapshot-enumerated CLOSED, not open-world) is
        // already pinned by `full.rs`'s own Task 3 unit test; this fixture
        // constructs the edge with `SetCompleteness::Complete` directly
        // (mirroring what `resolve_call_site_obligation` actually produces
        // via that function) and asserts `classify_obligation` end-to-end.
        assert_eq!(
            classify_obligation(&edge),
            ObligationOutcome::AmbiguousResolved,
            "an all-concrete, all-AmbiguousDispatch candidate set classifies \
             AmbiguousResolved, NOT Unknown — the metric-definition change"
        );

        let h = Histogram::of_edges(std::slice::from_ref(&edge));
        assert_eq!(h.unknown, 0, "must NOT count toward unknown");
        assert_eq!(
            h.ambiguous_resolved, 1,
            "must count in the dedicated ambiguous_resolved bucket"
        );
    }

    /// Task 4 fixture (b): a THREE-overload genuine same-object ambiguity
    /// carries THREE candidate routes — proves the candidate-carrying arm
    /// generalizes past the 2-overload case.
    #[test]
    fn resolve_member_object_genuine_three_way_overload_becomes_ambiguous_resolved_with_3_routes() {
        use crate::program::resolve::receiver::ReceiverType;

        let src_target: &'static str = r#"
codeunit 53974 "Overload3Target"
{
    procedure Baz(p: Integer)
    begin
    end;

    procedure Baz(p: Text)
    begin
    end;

    procedure Baz(p: Decimal)
    begin
    end;
}
"#;
        let src_caller: &'static str = r#"
codeunit 53975 "Overload3Caller"
{
    procedure Trigger()
    begin
    end;
}
"#;
        let app_id = make_app_id("TestApp");
        let unit_target = make_unit(app_id.clone(), "Overload3Target.al", src_target);
        let unit_caller = make_unit(app_id, "Overload3Caller.al", src_caller);
        let units = [unit_target, unit_caller];
        let graph = build_graph(&units, None);
        let index = ResolveIndex::build(&graph);
        let surface = DeclSurface::build(&graph, &units);

        let target_obj = find_obj(&graph, "Overload3Target");
        let baz_candidates = index.routines_in_object(&target_obj.id, "baz");
        assert_eq!(
            baz_candidates.len(),
            3,
            "fixture must produce THREE candidates"
        );

        let from_obj = find_obj(&graph, "Overload3Caller");
        let receiver = ReceiverType::Object {
            kind: ObjectKind::Codeunit,
            name_lc: "overload3target".into(),
            id: None,
        };
        let (shape, routes) =
            resolve_member(&receiver, "baz", 1, from_obj, &graph, &index, &surface);

        assert_eq!(shape, DispatchShape::AmbiguousOverload);
        assert_eq!(routes.len(), 3, "one route per candidate; got {routes:?}");
        let mut seen_ids = std::collections::HashSet::new();
        for r in &routes {
            assert_eq!(r.evidence, Evidence::Source);
            assert!(r.conditions.contains(&Condition::AmbiguousDispatch));
            assert!(!r.fires_by_default());
            let RouteTarget::Routine(ref rid) = r.target else {
                panic!("expected a Routine target; got {r:?}");
            };
            assert!(seen_ids.insert(rid.clone()), "targets must be distinct");
        }
    }

    /// Task 4 fixture (c): a genuinely ambiguous (`>1` visible, same-arity)
    /// candidate set where ONE candidate is `abi_overload_collapsed`-marked
    /// must NOT construct `DispatchShape::AmbiguousOverload` at all (round-2
    /// closer #1, BINDING: "a mixed/collapsed candidate set must never
    /// CONSTRUCT `DispatchShape::AmbiguousOverload`") — it degrades the WHOLE
    /// set back to the pre-Task-4 single `Unresolved(OverloadAmbiguous)`
    /// route, `Exact` shape, exactly like today's behavior. Low-level
    /// `ProgramGraph` construction (mirrors `entry_trigger_marker_guard_
    /// fixture`) since `abi_overload_collapsed` can only be set directly on a
    /// `RoutineNode`, not produced by the parser fixture path.
    #[test]
    fn resolve_member_object_ambiguous_set_with_one_collapse_marked_candidate_stays_unknown() {
        use crate::program::resolve::receiver::ReceiverType;

        let ws_id = make_app_id("WS");
        let dep_id = make_app_id("DepApp");

        let mut apps = AppRegistry::default();
        let ws_ref = apps.intern(&ws_id);
        let dep_ref = apps.intern(&dep_id);

        let caller_obj_id = ObjectNodeId {
            app: ws_ref,
            kind: ObjectKind::Codeunit,
            key: ObjKey::Id(50611),
        };
        let dep_obj_id = ObjectNodeId {
            app: dep_ref,
            kind: ObjectKind::Codeunit,
            key: ObjKey::Id(60151),
        };

        let objects = vec![
            ObjectNode {
                id: caller_obj_id.clone(),
                name: "MixedCaller".into(),
                declared_id: Some(50611),
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
            },
            ObjectNode {
                id: dep_obj_id.clone(),
                name: "MixedTarget".into(),
                declared_id: Some(60151),
                extends_target: None,
                implements: vec![],
                tier: TrustTier::SymbolOnly,
                source_table: None,
                table_no: None,
                source_table_temporary: false,
                page_controls: vec![],
                fields: vec![],
                dataitems: vec![],
                parse_incomplete: false,
            },
        ];

        // Two DISTINCT (different sig_fp) same-arity `Foo` candidates — a
        // genuine >1-visible ambiguity — but ONE is collapse-marked.
        let routines = vec![
            RoutineNode {
                id: RoutineNodeId {
                    object: dep_obj_id.clone(),
                    name_lc: "foo".into(),
                    enclosing_member_lc: None,
                    params_count: 1,
                    sig_fp: 111,
                },
                name: "Foo".into(),
                is_trigger: false,
                access: Access::Public,
                tier: TrustTier::SymbolOnly,
                event_subscribers: vec![],
                subscriber_instance_manual: false,
                publisher_kind: None,
                include_sender: None,
                abi_routine_kind: Some(AbiRoutineKind::Procedure),
                abi_event_kind: Some(AbiEventKind::None),
                param_sig_key: String::new(),
                return_type: None,
                return_type_id: None,
                abi_overload_collapsed: true,
                source_overload_aliased: false,
                abi_params: AbiParams::Missing,
            },
            RoutineNode {
                id: RoutineNodeId {
                    object: dep_obj_id.clone(),
                    name_lc: "foo".into(),
                    enclosing_member_lc: None,
                    params_count: 1,
                    sig_fp: 222,
                },
                name: "Foo".into(),
                is_trigger: false,
                access: Access::Public,
                tier: TrustTier::SymbolOnly,
                event_subscribers: vec![],
                subscriber_instance_manual: false,
                publisher_kind: None,
                include_sender: None,
                abi_routine_kind: Some(AbiRoutineKind::Procedure),
                abi_event_kind: Some(AbiEventKind::None),
                param_sig_key: String::new(),
                return_type: None,
                return_type_id: None,
                abi_overload_collapsed: false,
                source_overload_aliased: false,
                abi_params: AbiParams::Missing,
            },
        ];

        let mut topology = DependencyGraph::default();
        topology.add_dependency(ws_ref, dep_ref);

        let obj_index = ObjectIndex::build(&objects);
        let graph = ProgramGraph {
            apps,
            topology,
            objects,
            routines,
            obj_index,
            ..Default::default()
        };
        let index = ResolveIndex::build(&graph);
        let surface = DeclSurface::build(&graph, &[]);

        let from_obj = graph
            .objects
            .iter()
            .find(|o| o.id == caller_obj_id)
            .expect("caller must exist");
        let receiver = ReceiverType::Object {
            kind: ObjectKind::Codeunit,
            name_lc: "mixedtarget".into(),
            id: None,
        };
        let (shape, routes) =
            resolve_member(&receiver, "foo", 1, from_obj, &graph, &index, &surface);

        assert_eq!(
            shape,
            DispatchShape::Exact,
            "a mixed/collapsed candidate set must NEVER construct AmbiguousOverload"
        );
        assert_eq!(
            routes.len(),
            1,
            "the degraded set stays ONE route; got {routes:?}"
        );
        assert_eq!(routes[0].target, RouteTarget::Unresolved);
        assert_eq!(
            routes[0].evidence,
            Evidence::Unknown(UnknownReason::OverloadAmbiguous),
            "got {:?}",
            routes[0].evidence
        );
        assert!(
            !routes[0].conditions.contains(&Condition::AmbiguousDispatch),
            "a degraded route must never carry AmbiguousDispatch; got {:?}",
            routes[0]
        );
    }

    /// F1 (whole-branch review fix): a genuinely ambiguous (`>1` visible,
    /// same-arity) candidate set where BOTH candidates carry the IDENTICAL
    /// `RoutineNodeId` — the residual sig_fp collision `build::
    /// dedup_routines_preserving_genuine_overloads` marks
    /// `source_overload_aliased` (two DISTINCT source overloads whose
    /// `sig_fp` collided) — must NOT construct `DispatchShape::
    /// AmbiguousOverload` either, exactly like the ABI `abi_overload_
    /// collapsed` sibling fixture above. Pre-fix, `resolve_in_object`'s `_`
    /// arm's `degraded` predicate consulted ONLY `abi_overload_collapsed`, so
    /// this pair sailed through prevalidation — neither route's evidence is
    /// `Unknown` (both candidates share the SAME `DeclSurface` entry, since
    /// `DeclSurface` is keyed by `RoutineNodeId` and both candidates carry the
    /// SAME one) — and constructed an `AmbiguousOverload` shape with two
    /// IDENTICAL-target routes: a genuine unresolved collision laundered into
    /// a confident-looking multi-route resolution, the last laundering path
    /// the binding precondition "NO candidate is collapse-marked, ABI OR
    /// source-alias" was meant to close. The REAL `RoutineNodeId` (computed
    /// via the same [`crate::program::sig_fp::source_routine_node_id`]
    /// constructor production code uses) is reused for BOTH synthetic
    /// `RoutineNode` entries below, so the `DeclSurface` built from ONE real
    /// parsed declaration satisfies both `make_routine_route` lookups —
    /// reproducing "two IDENTICAL-target concrete routes" faithfully rather
    /// than relying on the separate Unknown-evidence prevalidation (which
    /// would mask this specific gap: an absent `DeclSurface` entry, not a
    /// same-id collision, is what THAT check exists to catch).
    #[test]
    fn resolve_member_object_ambiguous_set_with_source_alias_candidates_stays_unknown() {
        use crate::program::resolve::receiver::ReceiverType;
        use crate::program::sig_fp::source_routine_node_id;

        let app_id = make_app_id("AliasWS");
        let mut apps = AppRegistry::default();
        let app_ref = apps.intern(&app_id);

        let caller_obj_id = ObjectNodeId {
            app: app_ref,
            kind: ObjectKind::Codeunit,
            key: ObjKey::Id(50700),
        };
        let target_obj_id = ObjectNodeId {
            app: app_ref,
            kind: ObjectKind::Codeunit,
            key: ObjKey::Id(60152),
        };

        let objects = vec![
            ObjectNode {
                id: caller_obj_id.clone(),
                name: "AliasCaller".into(),
                declared_id: Some(50700),
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
            },
            ObjectNode {
                id: target_obj_id.clone(),
                name: "AliasTarget".into(),
                declared_id: Some(60152),
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
            },
        ];

        // Real parsed source for the target's ONE `Foo` procedure — its
        // production-computed `RoutineNodeId` (a real `sig_fp`) is reused
        // (cloned) as BOTH synthetic candidates' id below, simulating the
        // residual collision `dedup_routines_preserving_genuine_overloads`
        // marks `source_overload_aliased`, rather than fabricating an
        // arbitrary `sig_fp` integer that would not roundtrip through a real
        // `DeclSurface` lookup the same way.
        let target_src: &'static str = r#"
codeunit 60152 "AliasTarget"
{
    procedure Foo(X: Integer)
    begin
    end;
}
"#;
        let unit = make_unit(app_id.clone(), "AliasTarget.al", target_src);
        let routine_decl = &unit.files[0].file.objects[0].routines[0];
        let real_id = source_routine_node_id(target_obj_id.clone(), routine_decl);

        // Two DISTINCT source declarations (distinct `param_sig_key`) whose
        // `sig_fp` collided onto the SAME `RoutineNodeId` — both survivors of
        // such a run are marked `source_overload_aliased` (never collapsed to
        // one, per `dedup_routines_preserving_genuine_overloads`'s doc).
        let routines = vec![
            RoutineNode {
                id: real_id.clone(),
                name: "Foo".into(),
                is_trigger: false,
                access: Access::Public,
                tier: TrustTier::Workspace,
                event_subscribers: vec![],
                subscriber_instance_manual: false,
                publisher_kind: None,
                include_sender: None,
                abi_routine_kind: None,
                abi_event_kind: None,
                param_sig_key: "integer_variant_a".into(),
                return_type: None,
                return_type_id: None,
                abi_overload_collapsed: false,
                source_overload_aliased: true,
                abi_params: AbiParams::Missing,
            },
            RoutineNode {
                id: real_id.clone(),
                name: "Foo".into(),
                is_trigger: false,
                access: Access::Public,
                tier: TrustTier::Workspace,
                event_subscribers: vec![],
                subscriber_instance_manual: false,
                publisher_kind: None,
                include_sender: None,
                abi_routine_kind: None,
                abi_event_kind: None,
                param_sig_key: "integer_variant_b".into(),
                return_type: None,
                return_type_id: None,
                abi_overload_collapsed: false,
                source_overload_aliased: true,
                abi_params: AbiParams::Missing,
            },
        ];

        let obj_index = ObjectIndex::build(&objects);
        let graph = ProgramGraph {
            apps,
            topology: DependencyGraph::default(),
            objects,
            routines,
            obj_index,
            ..Default::default()
        };
        let index = ResolveIndex::build(&graph);
        let units = [unit];
        let surface = DeclSurface::build(&graph, &units);

        // Sanity preconditions: both candidates share the identical id, and
        // the DeclSurface resolves it (non-Unknown evidence for BOTH) — the
        // exact shape the pre-fix `degraded` predicate failed to catch.
        assert_eq!(
            index.routines_in_object(&target_obj_id, "foo").len(),
            2,
            "both source-aliased survivors must be indexed under the same id"
        );
        assert!(
            surface.get(&real_id).is_some(),
            "DeclSurface must resolve the shared id (non-Unknown evidence precondition)"
        );

        let from_obj = graph
            .objects
            .iter()
            .find(|o| o.id == caller_obj_id)
            .expect("caller must exist");
        let receiver = ReceiverType::Object {
            kind: ObjectKind::Codeunit,
            name_lc: "aliastarget".into(),
            id: None,
        };
        let (shape, routes) =
            resolve_member(&receiver, "foo", 1, from_obj, &graph, &index, &surface);

        assert_eq!(
            shape,
            DispatchShape::Exact,
            "a source-alias-marked candidate set must NEVER construct AmbiguousOverload; got {routes:?}"
        );
        assert_eq!(
            routes.len(),
            1,
            "the degraded set stays ONE route; got {routes:?}"
        );
        assert_eq!(routes[0].target, RouteTarget::Unresolved);
        assert_eq!(
            routes[0].evidence,
            Evidence::Unknown(UnknownReason::OverloadAmbiguous),
            "got {:?}",
            routes[0].evidence
        );
        assert!(
            !routes[0].conditions.contains(&Condition::AmbiguousDispatch),
            "a degraded route must never carry AmbiguousDispatch; got {:?}",
            routes[0]
        );
    }

    // (D-neg-4) Object receiver, same-app but UNRELATED (non-extension)
    // object's `protected` method.
    #[test]
    fn resolve_member_object_same_app_non_extension_protected_excluded() {
        use crate::program::resolve::receiver::ReceiverType;

        let src_target: &'static str = r#"
codeunit 53980 "ProtNTarget"
{
    protected procedure P()
    begin
    end;
}
"#;
        let src_caller: &'static str = r#"
codeunit 53981 "ProtNCaller"
{
    procedure Trigger()
    begin
    end;
}
"#;
        let app_id = make_app_id("TestApp");
        let unit_target = make_unit(app_id.clone(), "ProtNTarget.al", src_target);
        let unit_caller = make_unit(app_id, "ProtNCaller.al", src_caller);
        let units = [unit_target, unit_caller];
        let graph = build_graph(&units, None);
        let index = ResolveIndex::build(&graph);
        let surface = DeclSurface::build(&graph, &units);

        let from_obj = find_obj(&graph, "ProtNCaller");
        let receiver = ReceiverType::Object {
            kind: ObjectKind::Codeunit,
            name_lc: "protntarget".into(),
            id: None,
        };
        let (shape, routes) = resolve_member(&receiver, "p", 0, from_obj, &graph, &index, &surface);

        assert_eq!(shape, DispatchShape::Exact);
        assert_eq!(routes.len(), 1);
        assert_eq!(
            routes[0].target,
            RouteTarget::Unresolved,
            "a same-app but UNRELATED (non-extension) object's `protected` \
             method reached via an Object receiver must NOT resolve to \
             Source (gap D); got {:?}",
            routes[0].target
        );
        assert!(matches!(routes[0].evidence, Evidence::Unknown(_)));
    }

    // (D-neg-5) Object receiver, cross-app UNRELATED (non-extension)
    // object's `protected` method — a fortiori excluded vs the same-app case.
    #[test]
    fn resolve_member_object_cross_app_non_extension_protected_excluded() {
        use crate::program::resolve::receiver::ReceiverType;

        let src_target: &'static str = r#"
codeunit 53990 "ProtXNTarget"
{
    protected procedure P()
    begin
    end;
}
"#;
        let src_caller: &'static str = r#"
codeunit 53991 "ProtXNCaller"
{
    procedure Trigger()
    begin
    end;
}
"#;
        let app_a = make_app_id("PrimaryApp4");
        let app_b = make_app_id("DepApp4");
        let unit_target = make_unit(app_b, "ProtXNTarget.al", src_target);
        let unit_caller = make_unit(app_a, "ProtXNCaller.al", src_caller);
        let units = [unit_target, unit_caller];
        let graph = build_graph_multi_dep(&units, &[("PrimaryApp4", "DepApp4")]);
        let index = ResolveIndex::build(&graph);
        let surface = DeclSurface::build(&graph, &units);

        let from_obj = find_obj(&graph, "ProtXNCaller");
        let receiver = ReceiverType::Object {
            kind: ObjectKind::Codeunit,
            name_lc: "protxntarget".into(),
            id: None,
        };
        let (shape, routes) = resolve_member(&receiver, "p", 0, from_obj, &graph, &index, &surface);

        assert_eq!(shape, DispatchShape::Exact);
        assert_eq!(routes.len(), 1);
        assert_eq!(
            routes[0].target,
            RouteTarget::Unresolved,
            "a cross-app UNRELATED (non-extension) object's `protected` \
             method reached via an Object receiver must NOT resolve to \
             Source (gap D, a fortiori excluded vs the same-app case); \
             got {:?}",
            routes[0].target
        );
        assert!(matches!(routes[0].evidence, Evidence::Unknown(_)));
    }

    // (D-neg-6) Object receiver, WRONG-KIND extension: a Table and a Page
    // share the literal name "Shared"; a PageExtension `extends Shared`
    // resolves (kind-compatibly) against the PAGE, never the Table. The
    // Table's `protected` method must stay invisible — the PageExtension
    // does NOT directly extend the Table (wrong kind), despite sharing the
    // extends-target NAME with an object it DOES directly extend.
    #[test]
    fn resolve_member_object_wrong_kind_extension_protected_excluded() {
        use crate::program::resolve::receiver::ReceiverType;

        let src_table: &'static str = r#"
table 54000 "Shared"
{
    protected procedure P()
    begin
    end;
}
"#;
        let src_page: &'static str = r#"
page 54001 "Shared"
{
}
"#;
        let src_ext: &'static str = r#"
pageextension 54002 "SharedExt" extends Shared
{
    procedure Trigger()
    begin
    end;
}
"#;
        let app_id = make_app_id("TestApp");
        let unit_table = make_unit(app_id.clone(), "SharedTable.al", src_table);
        let unit_page = make_unit(app_id.clone(), "SharedPage.al", src_page);
        let unit_ext = make_unit(app_id, "SharedExt.al", src_ext);
        let units = [unit_table, unit_page, unit_ext];
        let graph = build_graph(&units, None);
        let index = ResolveIndex::build(&graph);
        let surface = DeclSurface::build(&graph, &units);

        let from_obj = find_obj(&graph, "SharedExt");
        assert_eq!(from_obj.id.kind, ObjectKind::PageExtension);
        let table_obj = graph
            .objects
            .iter()
            .find(|o| o.name.eq_ignore_ascii_case("shared") && o.id.kind == ObjectKind::Table)
            .expect("table Shared");
        let page_obj = graph
            .objects
            .iter()
            .find(|o| o.name.eq_ignore_ascii_case("shared") && o.id.kind == ObjectKind::Page)
            .expect("page Shared");

        // Sanity: kind-compatible resolution — the extension relationship
        // holds against the Page, never the Table.
        assert!(index.object_extends(&graph, &from_obj.id, &page_obj.id));
        assert!(!index.object_extends(&graph, &from_obj.id, &table_obj.id));

        let receiver = ReceiverType::Object {
            kind: ObjectKind::Table,
            name_lc: "shared".into(),
            id: None,
        };
        let (shape, routes) = resolve_member(&receiver, "p", 0, from_obj, &graph, &index, &surface);

        assert_eq!(shape, DispatchShape::Exact);
        assert_eq!(routes.len(), 1);
        assert_eq!(
            routes[0].target,
            RouteTarget::Unresolved,
            "a PageExtension must NOT see a same-named-but-WRONG-KIND \
             Table's `protected` method via an Object receiver (gap D); \
             got {:?}",
            routes[0].target
        );
        assert!(matches!(routes[0].evidence, Evidence::Unknown(_)));
    }

    // (G-neg / G-pos combined) Interface fan-out: a `public` implementer and
    // a CROSS-APP `internal` implementer of the SAME interface member. AL
    // does not require an interface-implementing procedure to be `public`
    // (a compiler-valid construct) — the `internal` implementer dispatches
    // fine for a SAME-app caller (see the sibling positive test below) but
    // must be excluded for a caller in a DIFFERENT app, while the `public`
    // implementer's route is unaffected either way.
    #[test]
    fn resolve_member_interface_cross_app_internal_impl_excluded_public_impl_still_resolves() {
        use crate::program::resolve::receiver::ReceiverType;

        let src_pub_impl: &'static str = r#"
codeunit 54010 "PubImplX" implements IFoo
{
    procedure Bar()
    begin
    end;
}
"#;
        let src_int_impl: &'static str = r#"
codeunit 54011 "IntImplX" implements IFoo
{
    internal procedure Bar()
    begin
    end;
}
"#;
        let src_caller: &'static str = r#"
codeunit 54012 "IfaceCallerX"
{
    procedure Trigger()
    begin
    end;
}
"#;
        let app_impl = make_app_id("ImplAppX");
        let app_caller = make_app_id("CallerAppX");
        let unit_pub_impl = make_unit(app_impl.clone(), "PubImplX.al", src_pub_impl);
        let unit_int_impl = make_unit(app_impl, "IntImplX.al", src_int_impl);
        let unit_caller = make_unit(app_caller, "IfaceCallerX.al", src_caller);
        let units = [unit_pub_impl, unit_int_impl, unit_caller];
        let graph = build_graph_multi_dep(&units, &[("CallerAppX", "ImplAppX")]);
        let index = ResolveIndex::build(&graph);
        let surface = DeclSurface::build(&graph, &units);

        let from_obj = find_obj(&graph, "IfaceCallerX");
        let receiver = ReceiverType::Interface {
            name_lc: "ifoo".into(),
        };
        let (shape, routes) =
            resolve_member(&receiver, "bar", 0, from_obj, &graph, &index, &surface);

        assert_eq!(shape, DispatchShape::Polymorphic);
        assert_eq!(
            routes.len(),
            2,
            "two implementers -> two routes; got {:?}",
            routes
        );

        let pub_impl_id = find_obj(&graph, "PubImplX").id.clone();
        let int_impl_id = find_obj(&graph, "IntImplX").id.clone();

        let pub_route = routes
            .iter()
            .find(|r| matches!(&r.target, RouteTarget::Routine(rid) if rid.object == pub_impl_id))
            .expect("public implementer must have a Routine route");
        assert_eq!(pub_route.evidence, Evidence::Source);

        let int_route_resolved_to_source = routes
            .iter()
            .any(|r| matches!(&r.target, RouteTarget::Routine(rid) if rid.object == int_impl_id));
        assert!(
            !int_route_resolved_to_source,
            "the cross-app `internal` implementer must NOT emit a \
             Routine/Source route (gap G — pre-fix this false-resolved); \
             got {:?}",
            routes
        );

        // The other route (not the public implementer's) must be an honest
        // Unresolved/Unknown — the internal implementer's excluded route.
        let other_route = routes
            .iter()
            .find(|r| !matches!(&r.target, RouteTarget::Routine(rid) if rid.object == pub_impl_id))
            .expect("the internal implementer's route must be present, not dropped");
        assert_eq!(other_route.target, RouteTarget::Unresolved);
        assert!(matches!(other_route.evidence, Evidence::Unknown(_)));
    }

    // (G-pos) SAME-app `internal` interface implementer — a positive control
    // proving `internal` is app-scoped, not interface-scoped: the sibling
    // test above proves the CROSS-app case is excluded.
    #[test]
    fn resolve_member_interface_same_app_internal_impl_resolves_to_source() {
        use crate::program::resolve::receiver::ReceiverType;

        let src_int_impl: &'static str = r#"
codeunit 54020 "IntImplY" implements IFoo
{
    internal procedure Bar()
    begin
    end;
}
"#;
        let src_caller: &'static str = r#"
codeunit 54021 "IfaceCallerY"
{
    procedure Trigger()
    begin
    end;
}
"#;
        let app_id = make_app_id("TestApp");
        let unit_impl = make_unit(app_id.clone(), "IntImplY.al", src_int_impl);
        let unit_caller = make_unit(app_id, "IfaceCallerY.al", src_caller);
        let units = [unit_impl, unit_caller];
        let graph = build_graph(&units, None);
        let index = ResolveIndex::build(&graph);
        let surface = DeclSurface::build(&graph, &units);

        let from_obj = find_obj(&graph, "IfaceCallerY");
        let receiver = ReceiverType::Interface {
            name_lc: "ifoo".into(),
        };
        let (shape, routes) =
            resolve_member(&receiver, "bar", 0, from_obj, &graph, &index, &surface);

        assert_eq!(shape, DispatchShape::Polymorphic);
        assert_eq!(routes.len(), 1);
        assert!(
            matches!(routes[0].target, RouteTarget::Routine(_)),
            "a SAME-app `internal` interface implementer must resolve to \
             Source (gap G positive control — Internal is app-scoped, not \
             interface-scoped); got {:?}",
            routes[0].target
        );
        assert_eq!(routes[0].evidence, Evidence::Source);
    }

    // (D-neg-7) A user-defined member LITERALLY named `Run`, arity 2 —
    // the `Codeunit.Run(arity<=1)` OnRun-trigger special case only engages
    // for arity<=1, so this falls through to the GENERAL resolve_in_object
    // dispatch. Proves "Run" is not a blanket access-filter exemption — only
    // the actual entry-trigger dispatch is.
    #[test]
    fn resolve_member_object_user_defined_run_cross_app_internal_excluded_not_run_exempt() {
        use crate::program::resolve::receiver::ReceiverType;

        let src_target: &'static str = r#"
codeunit 54030 "RunNTarget"
{
    internal procedure Run(a: Integer; b: Integer)
    begin
    end;
}
"#;
        let src_caller: &'static str = r#"
codeunit 54031 "RunNCaller"
{
    procedure Trigger()
    begin
    end;
}
"#;
        let app_a = make_app_id("PrimaryApp5");
        let app_b = make_app_id("DepApp5");
        let unit_target = make_unit(app_b, "RunNTarget.al", src_target);
        let unit_caller = make_unit(app_a, "RunNCaller.al", src_caller);
        let units = [unit_target, unit_caller];
        let graph = build_graph_multi_dep(&units, &[("PrimaryApp5", "DepApp5")]);
        let index = ResolveIndex::build(&graph);
        let surface = DeclSurface::build(&graph, &units);

        let from_obj = find_obj(&graph, "RunNCaller");
        let receiver = ReceiverType::Object {
            kind: ObjectKind::Codeunit,
            name_lc: "runntarget".into(),
            id: None,
        };
        let (shape, routes) =
            resolve_member(&receiver, "run", 2, from_obj, &graph, &index, &surface);

        assert_eq!(shape, DispatchShape::Exact);
        assert_eq!(routes.len(), 1);
        assert_eq!(
            routes[0].target,
            RouteTarget::Unresolved,
            "a user-defined 2-arg `Run` procedure declared `internal` in a \
             cross-app codeunit must NOT resolve to Source — the \
             OnRun-trigger special case is scoped to arity<=1 and must not \
             blanket-exempt every member literally named \"Run\"; got {:?}",
            routes[0].target
        );
        assert!(matches!(routes[0].evidence, Evidence::Unknown(_)));
    }

    // --- Run/ObjectRun exemption control: bypasses resolve_in_object -------

    // (Run-control) `Codeunit.Run()` on a codeunit with NO `OnRun` trigger —
    // must emit an Opaque AbiSymbol boundary route, never a synthesized
    // Source. This path (`resolve_member`'s inline Codeunit.Run special
    // case) bypasses `resolve_in_object` entirely and is untouched by Task 1.
    #[test]
    fn resolve_member_codeunit_run_no_onrun_trigger_emits_opaque_not_source() {
        use crate::program::resolve::receiver::ReceiverType;

        let src_target: &'static str = r#"
codeunit 54040 "NoOnRunTarget"
{
    procedure SomethingElse()
    begin
    end;
}
"#;
        let src_caller: &'static str = r#"
codeunit 54041 "NoOnRunCaller"
{
    procedure Trigger()
    begin
    end;
}
"#;
        let app_id = make_app_id("TestApp");
        let unit_target = make_unit(app_id.clone(), "NoOnRunTarget.al", src_target);
        let unit_caller = make_unit(app_id, "NoOnRunCaller.al", src_caller);
        let units = [unit_target, unit_caller];
        let graph = build_graph(&units, None);
        let index = ResolveIndex::build(&graph);
        let surface = DeclSurface::build(&graph, &units);

        let from_obj = find_obj(&graph, "NoOnRunCaller");
        let receiver = ReceiverType::Object {
            kind: ObjectKind::Codeunit,
            name_lc: "noonruntarget".into(),
            id: None,
        };
        let (shape, routes) =
            resolve_member(&receiver, "run", 0, from_obj, &graph, &index, &surface);

        assert_eq!(shape, DispatchShape::Exact);
        assert_eq!(routes.len(), 1);
        assert!(
            !matches!(routes[0].target, RouteTarget::Routine(_)),
            "Codeunit.Run() on a codeunit with NO OnRun trigger must never \
             synthesize a Source route; got {:?}",
            routes[0].target
        );
        assert!(
            matches!(routes[0].target, RouteTarget::AbiSymbol { .. }),
            "must be an Opaque AbiSymbol boundary route (object exists, \
             OnRun trigger absent — unaffected by the access filter since \
             Codeunit.Run bypasses resolve_in_object entirely); got {:?}",
            routes[0].target
        );
        assert_eq!(routes[0].evidence, Evidence::Opaque);
    }
}

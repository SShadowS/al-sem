//! Fresh-side canonical edge projection ([`project_fresh`]), the
//! L3-INDEPENDENT span-based site matcher ([`match_sites`]), and the shared
//! evidence/witness contract check ([`witness_contract_holds`]).
//!
//! # 1B.3b Task 3: the L3 oracle is gone from this module
//!
//! This module used to also host the L3 oracle projection (`project_l3` and
//! friends) and FOUR dual-run "fresh vs L3" comparison gates
//! (`run_harness` / `run_site_harness` / `run_resolution_harness` /
//! `run_member_resolution_harness` / `run_implicit_trigger_harness` /
//! `run_event_flow_gate`) that validated the fresh resolver against a LIVE L3
//! build on every CDO-gated test run. 1B.3b Task 3 retired all of that: the
//! fresh resolver is now validated against the COMMITTED, FROZEN, ANONYMIZED
//! goldens in `semantic_golden.rs` (`run_cdo_semantic_audit_on` /
//! `run_cdo_trigger_audit` / `run_cdo_event_audit`) plus the ported fan-out
//! applicability teeth (`semantic_golden::route_applicability`) — both
//! L3-INDEPENDENT at gate time. The three L3-touching projections needed
//! only to MINT those frozen goldens (`project_l3` /
//! `project_l3_implicit_trigger_in_scope` / `project_l3_event_rows`) moved to
//! [`crate::program::l3_mint`], the lone surviving L3-oracle access point in
//! the library (used by the dev-mint tool, `src/bin/mint-goldens.rs`, and by
//! the in-repo `REGEN_TEMP_GOLDENS` fixture-regen paths in
//! `tests/program_resolve_harness.rs`).
//!
//! This module and `semantic_golden.rs` import NEITHER `engine::l3` NOR
//! `engine::l2` — the gate path is fully L3-INDEPENDENT.
//!
//! # `object_lc` encoding for `ObjKey::Id`
//! When an object's key is numeric (`ObjKey::Id(n)`), `object_lc` is written
//! as `format!("{n}")` — the decimal representation of the signed integer.
//! [`crate::program::l3_mint`]'s L3-side projections mirror this choice
//! exactly so the two stay comparable.
//!
//! # `Unresolved`/`Unknown` routes
//! A route whose `target` is `RouteTarget::Unresolved` projects to **no**
//! entry in `targets`.  The stub resolver emits only `Unresolved` routes, so
//! every stub edge projects to an empty `targets` set.

use std::collections::{BTreeSet, HashMap};
use std::path::Path;

use al_syntax::IdentifierFoldExt;
use al_syntax::ir::ObjectKind;

use crate::program::node::{AppRef, AppRegistry, ObjKey, RoutineNodeId};
use crate::program::resolve::edge::{
    BuiltinId, CanonicalSpan, Edge, EdgeKind, RouteTarget, SourcePos,
};

// ---------------------------------------------------------------------------
// Canonical types
// ---------------------------------------------------------------------------

/// Canonical, stable representation of a resolved route target.
///
/// `kind` encodes the object kind (see [`object_kind_tag`]) for `Routine`
/// targets, or a sentinel value (254 = `AbiSymbol`, 255 = `Builtin`) for other
/// target classes so the differential can bucket by target type.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CanonicalTarget {
    pub kind: u8,
    pub app: Option<String>,
    pub object_lc: String,
    pub routine_lc: Option<String>,
}

/// Canonical identity of a routine node — the differential key for the "from"
/// side of an edge.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CanonicalKey {
    pub app_guid: String,
    pub object_kind: String,
    pub object_lc: String,
    pub routine_lc: String,
}

/// Canonical identity of a call site — combines the caller, source span, and
/// a content-fingerprint of the callee expression.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CanonicalSiteKey {
    pub caller: CanonicalKey,
    pub span: CanonicalSpan,
    pub callee_fp: u64,
}

/// A call/behaviour edge in canonical form — the unit of comparison in the
/// dual-run differential.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CanonicalEdge {
    pub from: CanonicalKey,
    pub site: CanonicalSiteKey,
    pub kind: EdgeKind,
    /// The set of concrete route targets.  Empty when all routes are
    /// `Unresolved` (stub phase) or when the edge is a genuine zero-route
    /// fan-out.
    pub targets: BTreeSet<CanonicalTarget>,
}

/// Outcome of aligning one call site between the fresh resolver and the L3 oracle.
///
/// Indices refer to positions in the `fresh` / `l3` slices passed to
/// [`match_sites`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SiteMatch {
    /// Both sides agree: `fresh[fi]` ↔ `l3[li]` share the same strong key.
    Paired(usize, usize),
    /// A fresh site that has no L3 peer in its `(from, kind)` partition.
    FreshOnly(usize),
    /// An L3 site that has no fresh peer in its `(from, kind)` partition.
    L3Only(usize),
    /// Genuinely ambiguous duplicate leftovers — multiple sites on both sides
    /// share the same strong key and the counts are unequal after positional
    /// pairing.  The vecs carry the excess `fresh` and `l3` indices respectively.
    Unaligned(Vec<usize>, Vec<usize>),
}

// ---------------------------------------------------------------------------
// Shared helpers — BOTH project_fresh and project_l3 call these so the two
// projections cannot silently diverge in encoding.
// ---------------------------------------------------------------------------

/// Map an already-lowercased object-kind string to the stable `u8` discriminant
/// used in [`CanonicalTarget::kind`].
///
/// The L3 side lowercases its `object_type` string (e.g. `"Codeunit"` →
/// `"codeunit"`) and calls this helper so the tag cannot drift from the fresh
/// side's [`object_kind_tag`].  Values are fixed — do not reorder.
pub(crate) fn object_kind_str_to_tag(lc: &str) -> u8 {
    match lc {
        "codeunit" => 0,
        "table" => 1,
        "tableextension" => 2,
        "page" => 3,
        "pageextension" => 4,
        "report" => 5,
        "reportextension" => 6,
        "query" => 7,
        "xmlport" => 8,
        "enum" => 9,
        "enumextension" => 10,
        "interface" => 11,
        "controladdin" => 12,
        "entitlement" => 13,
        "permissionset" => 14,
        "permissionsetextension" => 15,
        "profile" => 16,
        _ => 255,
    }
}

/// Build a [`CanonicalKey`] from pre-resolved, already-lowercased components.
///
/// Both `project_fresh` (via [`routine_to_key`]) and
/// `crate::program::l3_mint::project_l3` funnel through this so the key
/// layout is identical on both sides.
pub(crate) fn make_canonical_key(
    app_guid: String,
    object_kind: String,
    object_lc: String,
    routine_lc: String,
) -> CanonicalKey {
    CanonicalKey {
        app_guid,
        object_kind,
        object_lc,
        routine_lc,
    }
}

// ---------------------------------------------------------------------------
// Internal helpers (fresh-side only)
// ---------------------------------------------------------------------------

/// Map `ObjectKind` to a stable `u8` discriminant for `CanonicalTarget.kind`.
///
/// Delegates to [`object_kind_str_to_tag`] via [`object_kind_str`] so the two
/// cannot drift.
fn object_kind_tag(k: ObjectKind) -> u8 {
    object_kind_str_to_tag(&object_kind_str(k))
}

/// `ObjectKind` → lowercase debug name (e.g. `"codeunit"`, `"table"`).
fn object_kind_str(k: ObjectKind) -> String {
    format!("{k:?}").to_ascii_lowercase()
}

/// `ObjKey` → canonical string.
///
/// `Id(n)` → decimal integer string; `Name(s)` → the already-lowercased name.
/// `project_l3` mirrors this: L3 objects always carry an `object_number: i64`,
/// so it uses `format!("{n}")` directly.
fn obj_key_lc(key: &ObjKey) -> String {
    match key {
        ObjKey::Id(n) => format!("{n}"),
        ObjKey::Name(s) => s.clone(),
    }
}

/// Resolve `AppRef` → guid string via the registry.
///
/// Falls back to an empty string when the ref is out-of-range (e.g. synthetic
/// test fixtures that use `AppRef(0)` against an empty registry).
fn app_guid(apps: &AppRegistry, r: AppRef) -> String {
    apps.try_resolve(r)
        .map(|id| id.guid.clone())
        .unwrap_or_default()
}

/// Project a `RoutineNodeId` to a `CanonicalKey` (fresh-side only).
fn routine_to_key(id: &RoutineNodeId, apps: &AppRegistry) -> CanonicalKey {
    make_canonical_key(
        app_guid(apps, id.object.app),
        object_kind_str(id.object.kind),
        obj_key_lc(&id.object.key),
        id.name_lc.clone(),
    )
}

/// Project one `RouteTarget` to a `CanonicalTarget`.
///
/// Returns `None` for `Unresolved` — those map to an empty targets set.
fn project_target(target: &RouteTarget, apps: &AppRegistry) -> Option<CanonicalTarget> {
    match target {
        RouteTarget::Unresolved => None,
        RouteTarget::Routine(id) => Some(CanonicalTarget {
            kind: object_kind_tag(id.object.kind),
            app: Some(app_guid(apps, id.object.app)),
            object_lc: obj_key_lc(&id.object.key),
            routine_lc: Some(id.name_lc.clone()),
        }),
        RouteTarget::Builtin(BuiltinId(bid)) => Some(CanonicalTarget {
            kind: 255,
            app: None,
            object_lc: bid.clone(),
            routine_lc: None,
        }),
        RouteTarget::AbiSymbol { key } => Some(CanonicalTarget {
            kind: 254,
            app: Some(app_guid(apps, key.app)),
            object_lc: if key.object_number != 0 {
                format!("{}", key.object_number)
            } else {
                key.object_name_lc.clone()
            },
            routine_lc: Some(key.routine_name_lc.clone()),
        }),
    }
}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Map each `Edge` to a [`CanonicalEdge`] for the differential harness.
///
/// `Unresolved` routes project to **no** `CanonicalTarget` entry (empty set).
/// `apps` is the `ProgramGraph`'s `AppRegistry`, used to resolve `AppRef` →
/// guid.
#[must_use]
pub fn project_fresh(edges: &[Edge], apps: &AppRegistry) -> Vec<CanonicalEdge> {
    let mut result: Vec<CanonicalEdge> = edges
        .iter()
        .map(|e| {
            let from = routine_to_key(&e.from, apps);
            let caller = routine_to_key(&e.site.caller, apps);
            let targets = e
                .all_routes()
                .filter_map(|r| project_target(&r.target, apps))
                .collect();
            CanonicalEdge {
                from,
                site: CanonicalSiteKey {
                    caller,
                    span: e.site.span.clone(),
                    callee_fp: e.site.callee_fingerprint,
                },
                kind: e.kind,
                targets,
            }
        })
        .collect();
    result.sort();
    result
}

// ---------------------------------------------------------------------------
// Span-based site matcher (spec §6.1)
// ---------------------------------------------------------------------------

/// Align fresh and L3 call sites WITHOUT relying on positional ordinals.
///
/// ## Algorithm (spec §6.1)
///
/// 1. **Partition** both slices into groups keyed by `(from, kind)`.  Sites
///    only ever match within the same group.
/// 2. **Within each group**, bucket sites by the *strong key*
///    `(span.unit, span.start.line, callee_fp)`.  Column offsets are ignored
///    because L3 uses UTF-16 columns while the fresh side uses byte columns —
///    they agree on ASCII-only source, and may differ by a small delta on
///    non-ASCII identifiers.
/// 3. **Pair positionally** within each strong-key bucket:
///    - Equal counts → all [`SiteMatch::Paired`].
///    - One side absent → [`SiteMatch::FreshOnly`] / [`SiteMatch::L3Only`]
///      for every site in that bucket.
///    - Both sides present, unequal counts → pair the `min` count, then emit
///      a single [`SiteMatch::Unaligned`] with the leftover indices.
///
/// **Cascade-resistance guarantee:** removing one L3 site changes at most ONE
/// bucket (→ the corresponding fresh site becomes [`SiteMatch::FreshOnly`])
/// and NEVER shifts the pairing of any other site.
#[must_use]
pub fn match_sites(fresh: &[CanonicalEdge], l3: &[CanonicalEdge]) -> Vec<SiteMatch> {
    type GroupKey = (CanonicalKey, EdgeKind);
    // Strong key: (unit, start_line, callee_fp) — col intentionally omitted.
    type StrongKey = (String, u32, u64);

    // Step 1: partition both slices into (from, kind) groups.
    let mut fresh_groups: HashMap<GroupKey, Vec<usize>> = HashMap::new();
    let mut l3_groups: HashMap<GroupKey, Vec<usize>> = HashMap::new();

    for (i, e) in fresh.iter().enumerate() {
        fresh_groups
            .entry((e.from.clone(), e.kind))
            .or_default()
            .push(i);
    }
    for (i, e) in l3.iter().enumerate() {
        l3_groups
            .entry((e.from.clone(), e.kind))
            .or_default()
            .push(i);
    }

    let mut all_group_keys: Vec<GroupKey> = fresh_groups
        .keys()
        .chain(l3_groups.keys())
        .cloned()
        .collect();
    all_group_keys.sort_unstable();
    all_group_keys.dedup();

    let mut result: Vec<SiteMatch> = Vec::new();
    let empty: Vec<usize> = Vec::new();

    for gk in all_group_keys {
        let fresh_idxs = fresh_groups.get(&gk).unwrap_or(&empty);
        let l3_idxs = l3_groups.get(&gk).unwrap_or(&empty);

        // Step 2: bucket by strong key within this group.
        let mut fresh_by_sk: HashMap<StrongKey, Vec<usize>> = HashMap::new();
        let mut l3_by_sk: HashMap<StrongKey, Vec<usize>> = HashMap::new();

        for &fi in fresh_idxs {
            let e = &fresh[fi];
            let sk = (
                e.site.span.unit.clone(),
                e.site.span.start.line,
                e.site.callee_fp,
            );
            fresh_by_sk.entry(sk).or_default().push(fi);
        }
        for &li in l3_idxs {
            let e = &l3[li];
            let sk = (
                e.site.span.unit.clone(),
                e.site.span.start.line,
                e.site.callee_fp,
            );
            l3_by_sk.entry(sk).or_default().push(li);
        }

        let mut all_sks: Vec<StrongKey> =
            fresh_by_sk.keys().chain(l3_by_sk.keys()).cloned().collect();
        all_sks.sort_unstable();
        all_sks.dedup();

        // Step 3: pair within each strong-key bucket.
        for sk in all_sks {
            let fis = fresh_by_sk.get(&sk).map(Vec::as_slice).unwrap_or(&[]);
            let lis = l3_by_sk.get(&sk).map(Vec::as_slice).unwrap_or(&[]);

            let pair_count = fis.len().min(lis.len());
            for i in 0..pair_count {
                result.push(SiteMatch::Paired(fis[i], lis[i]));
            }

            let extra_f = &fis[pair_count..];
            let extra_l = &lis[pair_count..];

            if pair_count == 0 {
                // One side is entirely absent → unambiguous.
                for &fi in extra_f {
                    result.push(SiteMatch::FreshOnly(fi));
                }
                for &li in extra_l {
                    result.push(SiteMatch::L3Only(li));
                }
            } else {
                // Some pairings happened; leftovers are genuinely ambiguous duplicates.
                if !extra_f.is_empty() || !extra_l.is_empty() {
                    result.push(SiteMatch::Unaligned(extra_f.to_vec(), extra_l.to_vec()));
                }
            }
        }
    }

    result
}

// ---------------------------------------------------------------------------
// Shared evidence/witness contract check
// ---------------------------------------------------------------------------

/// Returns `true` when the route's evidence/witness combination is valid.
///
/// Contract (spec §5.5):
/// - `Source`  → `SourceSpan` with non-empty file
/// - `Abi`     → `AbiSymbol`
/// - `Catalog` → `CatalogEntry`
/// - `Opaque`  → `AbiSymbol`
/// - `Unknown` → `None`
pub(crate) fn witness_contract_holds(route: &crate::program::resolve::edge::Route) -> bool {
    use crate::program::resolve::edge::{EvidenceKind, RouteTarget, Witness};
    // For Unresolved targets the evidence must be Unknown (per resolver invariants).
    // Check both the evidence type and the witness shape. Task 3: compares on
    // `Evidence::kind()` (the reason-agnostic projection), never the raw
    // `Evidence` value — this contract must hold identically regardless of
    // WHICH `UnknownReason` a route's `Unknown` evidence carries.
    match (route.evidence.kind(), &route.witness) {
        (EvidenceKind::Source, Witness::SourceSpan { file, .. }) => !file.is_empty(),
        (EvidenceKind::Abi, Witness::AbiSymbol { .. }) => true,
        (EvidenceKind::Catalog, Witness::CatalogEntry { .. }) => true,
        (EvidenceKind::Opaque, Witness::AbiSymbol { .. }) => true,
        (EvidenceKind::Unknown, Witness::None) => {
            // Unknown evidence must pair with Unresolved target.
            matches!(route.target, RouteTarget::Unresolved)
        }
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// CanonicalKey-keyed EventFlow projection (frozen-golden support)
// ---------------------------------------------------------------------------
//
// The now-removed live dual-run `run_event_flow_gate` (retired 1B.3b Task 3)
// keyed subscribers by L3's PROPRIETARY `stable_routine_id` (a
// normalized-signature hash fresh cannot independently reproduce) — fine for
// a LIVE comparison where both sides were computed in the same process, but
// unusable as the identity scheme for a COMMITTED frozen golden the fresh
// side must re-derive from scratch at audit time.
//
// [`CanonicalEventRow`] instead keys publisher/subscriber by the SAME
// `CanonicalKey` (app_guid + object_kind + object_lc + routine_lc) shared
// with `project_fresh` and [`crate::program::l3_mint`]'s L3-side projections.
// `L3Routine` exposes `app_guid`/`object_type`/`object_number`/`name` directly,
// so the L3 side builds the identical `CanonicalKey` shape WITHOUT going
// through L3's stable-id hash at all. `publisher_arity` carries the resolved
// overload's parameter count (event pairs are intentionally arity-agnostic —
// the same event name can have multiple `[IntegrationEvent]` overloads).

/// One resolved publisher→subscriber EventFlow row, keyed by the SAME
/// `CanonicalKey` shape used everywhere else in this differential — see the
/// section docs above for why this avoids L3's proprietary stable-id scheme
/// for the frozen-golden use case.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct CanonicalEventRow {
    pub publisher: CanonicalKey,
    pub event_name_lc: String,
    pub subscriber: CanonicalKey,
    pub publisher_arity: Option<usize>,
}

/// L3-INDEPENDENT: project the fresh resolver's resolved EventFlow
/// publisher→subscriber pairs for `workspace_root`, keyed by `CanonicalKey`.
///
/// Mirrors [`emit_event_flow_edges`][crate::program::resolve::resolver::emit_event_flow_edges]'s
/// own resolution (the SAME production function the live dual-run gate and
/// `resolve_full_program` use) — only `RouteTarget::Routine` routes
/// contribute a row; `AbiSymbol`/`Builtin`/`Unresolved` routes carry no
/// subscriber identity and are skipped (same convention as
/// [`project_target`]).
///
/// Used by the always-run synthetic EventFlow fixture test (1B.3b Task 1
/// Step 4) and by [`crate::program::resolve::semantic_golden::run_cdo_event_audit`]'s
/// fresh side — neither touches `engine::l3`.
#[must_use]
pub fn project_fresh_event_rows(workspace_root: &Path) -> Vec<CanonicalEventRow> {
    use crate::program::resolve::full::build_context;

    let Some(ctx) = build_context(workspace_root) else {
        return Vec::new();
    };
    project_fresh_event_rows_on(&ctx)
}

/// Substrate-taking core of [`project_fresh_event_rows`] — reads the program
/// graph and decl surface from `ctx` instead of rebuilding the
/// snapshot/graph/parse internally.
#[must_use]
pub fn project_fresh_event_rows_on(
    ctx: &crate::program::resolve::full::ProgramContext,
) -> Vec<CanonicalEventRow> {
    use crate::program::resolve::resolver::emit_event_flow_edges;

    let graph = &ctx.graph;
    let surface = ctx.decl_surface();
    let apps = &graph.apps;

    let fresh_edges = emit_event_flow_edges(graph, &surface);

    let mut rows: Vec<CanonicalEventRow> = Vec::new();
    for edge in &fresh_edges {
        if edge.kind != EdgeKind::EventFlow {
            continue;
        }
        let publisher = routine_to_key(&edge.from, apps);
        let event_name_lc = edge.from.name_lc.clone();
        let publisher_arity = Some(edge.from.params_count);
        for route in &edge.routes {
            if let RouteTarget::Routine(sub_rid) = &route.target {
                rows.push(CanonicalEventRow {
                    publisher: publisher.clone(),
                    event_name_lc: event_name_lc.clone(),
                    subscriber: routine_to_key(sub_rid, apps),
                    publisher_arity,
                });
            }
        }
    }
    rows.sort();
    rows
}

/// Independently verify that the subscriber routine `sub_rid` genuinely subscribes
/// to the named publisher event, by re-reading its raw `[EventSubscriber]` attributes
/// from the `ParsedUnit` IR at gate time.
///
/// This is INDEPENDENT of `RoutineNode.event_subscribers` (the index's cached parse
/// that built the edge). It calls [`crate::program::resolve::event::parse_event_subscriber_ir`]
/// directly on `RoutineDecl.attributes_parsed`, not on any pre-computed field.
///
/// Returns `true` (PASS) when:
/// 1. `sub_rid.params_count <= subscriber_arity_bound(publisher_params_count,
///    publisher_include_sender)` (parameter prefix check) — the SAME
///    CONDITIONAL Sender-tolerant bound `ResolveIndex::build`'s wiring
///    (`index.rs`) uses to admit a candidate in the first place, via the ONE
///    shared helper [`crate::program::resolve::event::subscriber_arity_bound`]
///    (Task 1, round-2: a blanket `+1` regardless of `IncludeSender` would be
///    SYNCHRONIZED WRONGNESS — see that function's doc). Sender param-TYPE
///    compatibility is NOT validated (arity-only check; documented residual).
/// 2. At least one `[EventSubscriber]` attribute in the subscriber's raw IR freshly
///    parses to match `(publisher_object_type_lc, publisher_name_lc, event_name_lc)`.
///
/// Returns `true` (fail-open) when:
/// - The subscriber's app is not found in `apps`, OR
/// - The subscriber's app is found in `apps` but has no corresponding `ParsedUnit`
///   (dep-boundary subscriber — source not in workspace; AbiSymbol routes are already
///   excluded before reaching the teeth so this path is for consistency only).
///
/// Returns `false` (FAIL → `unverified_extra`) when:
/// - `sub_rid.params_count` exceeds the Sender-tolerant bound above, OR
/// - The subscriber IS found in `parsed` but no freshly-parsed attribute names the
///   expected `(publisher_object_type_lc, publisher_name_lc, event_name_lc)` triple.
// Each param maps 1:1 to a piece of the canonical event-route identity (or, for
// `publisher_include_sender`, Task 1's arity-tolerance input) — a struct wrapper
// would only pay for itself by shrinking call sites, and most call sites here pass
// literals straight through (test fixtures), so grouping would only add a layer of
// indirection without reducing the argument count actually written at each site.
#[allow(clippy::too_many_arguments)]
pub fn verify_event_subscriber_route(
    sub_rid: &RoutineNodeId,
    publisher_object_type_lc: &str,
    publisher_name_lc: &str,
    event_name_lc: &str,
    publisher_params_count: usize,
    publisher_include_sender: Option<bool>,
    parsed: &[&crate::snapshot::ParsedUnit],
    apps: &AppRegistry,
) -> bool {
    use crate::program::node::ObjKey;
    use crate::program::resolve::event::{parse_event_subscriber_ir, subscriber_arity_bound};

    // ── Parameter prefix check (Sender-tolerant, CONDITIONAL — Task 1) ──────
    let max_allowed_arity =
        subscriber_arity_bound(publisher_params_count, publisher_include_sender);
    if sub_rid.params_count > max_allowed_arity {
        return false;
    }

    // ── Resolve subscriber app GUID ──────────────────────────────────────────
    let sub_app_id = match apps.try_resolve(sub_rid.object.app) {
        Some(id) => id,
        None => return true, // unknown AppRef → fail-open
    };

    // ── Find ParsedUnit for the subscriber's app ─────────────────────────────
    let Some(unit) = parsed.iter().find(|u| u.app.guid == sub_app_id.guid) else {
        return true; // dep-boundary subscriber — source not in snapshot → fail-open
    };

    // ── Scan files → object → routine → re-parse [EventSubscriber] attrs ────
    for pf in &unit.files {
        for obj in &pf.file.objects {
            if obj.kind != sub_rid.object.kind {
                continue;
            }
            let key_matches = match &sub_rid.object.key {
                ObjKey::Id(n) => obj.id == Some(*n),
                ObjKey::Name(name_lc) => obj.name.fold_identifier() == *name_lc,
            };
            if !key_matches {
                continue;
            }
            for r in &obj.routines {
                if r.name.fold_identifier() != sub_rid.name_lc {
                    continue;
                }
                if r.params.len() != sub_rid.params_count {
                    continue;
                }
                // Routine found — re-parse its [EventSubscriber] attrs fresh
                // (NOT from sub_rid or any cached RoutineNode field).
                let has_match = r
                    .attributes_parsed
                    .iter()
                    .filter(|a| a.name.eq_ignore_ascii_case("eventsubscriber"))
                    .filter_map(|a| parse_event_subscriber_ir(a, &pf.file.ir))
                    .any(|args| {
                        args.publisher_object_type == publisher_object_type_lc
                            && args.publisher_name == publisher_name_lc
                            && args.event_name == event_name_lc
                    });
                return has_match;
            }
        }
    }

    // Routine not found in any parsed file of this app → fail-open.
    true
}

// ---------------------------------------------------------------------------
// Test helper
// ---------------------------------------------------------------------------

/// Build a synthetic [`CanonicalEdge`] for use in matcher fixture tests.
///
/// * `caller` — colon-separated `"object_kind:object_lc:routine_lc"`,
///   e.g. `"cu:c:run"`.  `app_guid` is left empty.
/// * `span_start` — 0-based start line stored in the span.
/// * `fp` — callee fingerprint.
///
/// `from` is set equal to the caller key, `kind` is [`EdgeKind::Call`], and
/// `targets` is empty.  Column offsets default to 0 / 10.
#[doc(hidden)]
pub fn canonical_call_edge_for_test(caller: &str, span_start: u32, fp: u64) -> CanonicalEdge {
    let parts: Vec<&str> = caller.splitn(4, ':').collect();
    let caller_key = CanonicalKey {
        app_guid: String::new(),
        object_kind: parts.first().copied().unwrap_or("").to_string(),
        object_lc: parts.get(1).copied().unwrap_or("").to_string(),
        routine_lc: parts.get(2).copied().unwrap_or("").to_string(),
    };
    CanonicalEdge {
        from: caller_key.clone(),
        site: CanonicalSiteKey {
            caller: caller_key,
            span: CanonicalSpan {
                unit: "test_unit".to_string(),
                start: SourcePos {
                    line: span_start,
                    col: 0,
                },
                end: SourcePos {
                    line: span_start,
                    col: 10,
                },
            },
            callee_fp: fp,
        },
        kind: EdgeKind::Call,
        targets: BTreeSet::new(),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_side_object_kind_parity() {
        // Fresh derives the caller key's object_kind via Debug-lowercase; L3 derives it from
        // its `object_type` string lowercased. They MUST agree for every kind or sites silently
        // drop out of `matched`. This asserts the canonical spelling for each variant.
        use crate::program::node::ObjectKind;
        let cases = [
            (ObjectKind::Codeunit, "codeunit"),
            (ObjectKind::Table, "table"),
            (ObjectKind::TableExtension, "tableextension"),
            (ObjectKind::Page, "page"),
            (ObjectKind::PageExtension, "pageextension"),
            (ObjectKind::Report, "report"),
            (ObjectKind::ReportExtension, "reportextension"),
            (ObjectKind::XmlPort, "xmlport"),
            (ObjectKind::Query, "query"),
            (ObjectKind::Enum, "enum"),
            (ObjectKind::EnumExtension, "enumextension"),
            (ObjectKind::Interface, "interface"),
            (ObjectKind::ControlAddIn, "controladdin"),
            (ObjectKind::Entitlement, "entitlement"),
            (ObjectKind::PermissionSet, "permissionset"),
            (ObjectKind::PermissionSetExtension, "permissionsetextension"),
            (ObjectKind::Profile, "profile"),
            (ObjectKind::Other, "other"),
        ];
        for (k, expected) in cases {
            assert_eq!(
                format!("{k:?}").to_ascii_lowercase(),
                expected,
                "kind {k:?}"
            );
        }
    }

    #[test]
    fn project_fresh_round_trips_a_synthetic_edge() {
        // Build a tiny ProgramGraph-free CanonicalEdge directly from a synthetic Edge.
        // (Full CDO projection is exercised by the env-gated harness test, Task 7.)
        let edges = crate::program::resolve::stub::synthetic_unknown_edge_for_test();
        let apps = crate::program::node::AppRegistry::default();
        let canon = project_fresh(&edges, &apps);
        assert_eq!(canon.len(), 1);
        assert!(
            canon[0].targets.is_empty(),
            "stub Unknown edge has no concrete target"
        );
    }
}

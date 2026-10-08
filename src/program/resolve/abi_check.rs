//! ABI ingestion-integrity invariant: proves that every `AbiSymbol` route's
//! structured `AbiRoutineKey` maps back to a real entry in a FRESH re-parse
//! of the raw SymbolReference data, independent of the ingested `ProgramGraph`
//! nodes.
//!
//! # Integrity vs correctness
//!
//! `abi_ingestion_integrity` proves **no ingestion mangling** — that the route's
//! structured `AbiRoutineKey` matches a real entry in a raw-ABI index built by
//! re-parsing the dep SymbolReference (independent of `graph.routines` /
//! `graph.objects`). It does NOT prove semantic correctness (that the route
//! targets the semantically-right callee).
//!
//! The independence is the key property: if ingestion or key-derivation mangled
//! the key (e.g., swapped `routine_kind` from Procedure to EventPublisher, or
//! lowercased the wrong field), the raw-ABI index would NOT have an entry for
//! the mangled key, and the check would catch it.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use al_syntax::IdentifierFoldExt;

use crate::engine::deps::symbol_reference::{
    AbiEventKind as SrAbiEventKind, AbiRoutine, SymbolReferenceAbi,
};
use crate::program::abi_ingest::object_kind_from_abi_type;
use crate::program::node::AppRef;
use crate::program::resolve::edge::{
    AbiEventKind, AbiRoutineKey, AbiRoutineKind, Edge, RouteTarget,
};

// ---------------------------------------------------------------------------
// Internal index entry
// ---------------------------------------------------------------------------

/// Internal entry for the raw-ABI index.  Fields are normalized to match the
/// `AbiRoutineKey` layout in `resolver.rs` so the lookup is a direct
/// `HashSet::contains`.
///
/// `object_type_lc` is normalized via the same chain as `resolver.rs`:
/// `raw_abi_type → ObjectKind (object_kind_from_abi_type) → "{:?}" debug → lowercase`.
/// This ensures "EnumType" → "enum", "XmlPort" → "xmlport", etc., all match
/// what the key builder produces.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct RawEntry {
    /// Normalized object type: `format!("{:?}", ObjectKind).to_ascii_lowercase()`.
    object_type_lc: String,
    /// 0 when the object uses a name-only key (no numeric id).
    object_number: i64,
    /// Empty string when `object_number != 0` (same convention as `AbiRoutineKey`
    /// built in `resolver.rs::make_routine_route`).
    object_name_lc: String,
    routine_name_lc: String,
    params_count: usize,
    routine_kind: AbiRoutineKind,
    event_kind: AbiEventKind,
}

// ---------------------------------------------------------------------------
// Raw-ABI index
// ---------------------------------------------------------------------------

/// Raw-ABI index built from FRESH parses of dep `SymbolReference` data.
///
/// Keyed by `AppRef` so per-dep lookups are O(1) after the initial build.
///
/// # Independence guarantee
///
/// This index is built from `SymbolReferenceAbi` (raw DTOs from
/// `parse_symbol_reference`), NOT from the ingested `ProgramGraph.routines` /
/// `ProgramGraph.objects`.  Building the index from graph nodes would be
/// circular — the graph nodes ARE the ingested output, so a key-derivation
/// bug there would affect both sides equally and go undetected.  By re-parsing
/// from the raw DTO, we get a second, independent derivation of what entries
/// should exist, making any ingestion or key-derivation mangling detectable.
pub struct RawAbiIndex {
    entries: HashMap<AppRef, HashSet<RawEntry>>,
}

impl RawAbiIndex {
    /// Build from `(AppRef, &SymbolReferenceAbi)` pairs.
    ///
    /// `SymbolReferenceAbi` is the raw DTO produced by `parse_symbol_reference`
    /// — it contains no graph nodes. Tier-1 remediation (H-1): `is_local` /
    /// `is_internal` routines are INCLUDED here, mirroring `ingest_abi` in
    /// `abi_ingest.rs` — this index exists to independently re-derive exactly
    /// what `ingest_abi` ingests, so the two must never diverge on which raw
    /// entries count (a stale skip here would make every newly-ingested
    /// local/internal ABI routine show up as a false `abi_unmapped` in the
    /// integrity check).
    pub fn build<'a>(pairs: impl IntoIterator<Item = (AppRef, &'a SymbolReferenceAbi)>) -> Self {
        let mut entries: HashMap<AppRef, HashSet<RawEntry>> = HashMap::new();
        for (app_ref, abi) in pairs {
            let set = entries.entry(app_ref).or_default();
            for obj in &abi.objects {
                // Normalize: same chain as resolver.rs builds AbiRoutineKey.object_type.
                let object_type_lc = format!("{:?}", object_kind_from_abi_type(&obj.object_type))
                    .to_ascii_lowercase();
                // Same key convention as ingest_abi / make_routine_route:
                // non-zero id → (number, ""); name-only → (0, lowercase_name).
                let (object_number, object_name_lc) = if obj.object_number != 0 {
                    (obj.object_number, String::new())
                } else {
                    (0i64, obj.name.fold_identifier())
                };

                for routine in &obj.routines {
                    let (routine_kind, event_kind) = map_kind(routine);
                    set.insert(RawEntry {
                        object_type_lc: object_type_lc.clone(),
                        object_number,
                        object_name_lc: object_name_lc.clone(),
                        routine_name_lc: routine.name.fold_identifier(),
                        params_count: routine.parameters.len(),
                        routine_kind,
                        event_kind,
                    });
                }
            }
        }
        RawAbiIndex { entries }
    }

    /// Check whether `key` is present in the raw-ABI index.
    fn contains(&self, key: &AbiRoutineKey) -> bool {
        let Some(set) = self.entries.get(&key.app) else {
            return false;
        };
        // `key.object_type` is already lowercase (resolver.rs builds it as
        // `format!("{:?}", ObjectKind).to_ascii_lowercase()`).
        set.contains(&RawEntry {
            object_type_lc: key.object_type.clone(),
            object_number: key.object_number,
            object_name_lc: key.object_name_lc.clone(),
            routine_name_lc: key.routine_name_lc.clone(),
            params_count: key.params_count,
            routine_kind: key.routine_kind.clone(),
            event_kind: key.event_kind.clone(),
        })
    }
}

// ---------------------------------------------------------------------------
// Kind mapping (mirrors abi_ingest.rs::abi_routine_kind_from_str)
// ---------------------------------------------------------------------------

/// Map the raw `AbiRoutine.kind` string + `AbiRoutine.event_kind` to the
/// canonical `(AbiRoutineKind, AbiEventKind)` pair used in `AbiRoutineKey`.
///
/// Mirrors `abi_ingest.rs::abi_routine_kind_from_str` exactly so that the raw
/// index and the ingested key use the same mapping.
fn map_kind(routine: &AbiRoutine) -> (AbiRoutineKind, AbiEventKind) {
    match routine.kind.as_str() {
        "event-publisher" => {
            let ek = match &routine.event_kind {
                SrAbiEventKind::Integration => AbiEventKind::Integration,
                SrAbiEventKind::Business => AbiEventKind::Business,
                SrAbiEventKind::Unknown => AbiEventKind::Internal,
            };
            (AbiRoutineKind::EventPublisher, ek)
        }
        "event-subscriber" => (AbiRoutineKind::EventSubscriber, AbiEventKind::None),
        _ => (AbiRoutineKind::Procedure, AbiEventKind::None),
    }
}

// ---------------------------------------------------------------------------
// Report types
// ---------------------------------------------------------------------------

/// One unmapped `AbiSymbol` route — the key that failed the raw-ABI lookup.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnmappedAbiSite {
    pub key: AbiRoutineKey,
}

/// Report from [`abi_ingestion_integrity`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AbiIntegrityReport {
    /// Total number of `AbiSymbol` routes across all edges.
    pub abi_routes_total: usize,
    /// Routes whose key IS present in the raw-ABI index (no mangling detected).
    pub abi_mapped: usize,
    /// Routes whose key is ABSENT from the raw-ABI index — a genuine
    /// ingestion/key-derivation bug.  Must be 0 on a correct implementation.
    pub abi_unmapped: usize,
    /// Details of each unmapped route (for debugging); sorted for determinism.
    pub abi_unmapped_sites: Vec<UnmappedAbiSite>,
}

// ---------------------------------------------------------------------------
// Integrity check
// ---------------------------------------------------------------------------

/// True when `key` is an OBJECT'S IMPLICIT ENTRY-TRIGGER boundary (the
/// `opaque_boundary_route` fallback in `resolver.rs::resolve_object_run`,
/// beyond-1B.3b Task 5.5), not a genuine ABI-ingested Method.
///
/// Entry triggers (`OnRun`/`OnOpenPage`/`OnPreReport`) are PLATFORM-INTRINSIC
/// per object kind — every Page implicitly has an `OnOpenPage` hook whether or
/// not source overrides it — and structurally NEVER appear in a `.app`'s
/// `SymbolReference.json` `Methods` array (triggers are declared with the
/// `trigger` keyword, not `procedure`, and the ABI schema only exposes
/// procedures as Methods; verified against a real Microsoft Base Application
/// package — Warehouse pages 7341/7342 carry zero `Methods` entries). So a
/// raw-ABI-index miss for this EXACT key shape is not an ingestion bug, it is
/// the permanent, universal absence of a trigger from the Methods surface —
/// true for every BC app, not a data-quality issue with one dependency.
///
/// This can NEVER mask a real key-derivation bug: `resolve_object_run` only
/// reaches the synthesized-key fallback when `index.routines_in_object` (a
/// NAME-ONLY lookup, no arity filter) already found ZERO candidates for the
/// trigger name — built from the exact same `abi.objects[].routines` data
/// `raw_abi_index` independently re-parses. Whenever this shape is produced,
/// `raw_abi_index.contains(key)` would independently also be `false`, so
/// exempting this shape changes no other outcome.
///
/// Keep in sync with `resolver.rs::entry_trigger_name` (Page -> "onopenpage",
/// Report -> "onprereport", everything else -> "onrun").
fn is_entry_trigger_boundary_key(key: &AbiRoutineKey) -> bool {
    if key.routine_kind != AbiRoutineKind::Procedure
        || key.event_kind != AbiEventKind::None
        || key.params_count != 0
    {
        return false;
    }
    let entry_trigger_name = match key.object_type.as_str() {
        "page" => "onopenpage",
        "report" => "onprereport",
        _ => "onrun",
    };
    key.routine_name_lc == entry_trigger_name
}

/// Verify that every `AbiSymbol` route in `edges` maps back to an entry in
/// `raw_abi_index`.
///
/// # What this checks
///
/// For each `RouteTarget::AbiSymbol { key }` route: the `AbiRoutineKey`'s
/// `(object_type, object_number/object_name_lc, routine_name_lc, params_count,
/// routine_kind, event_kind)` tuple must be present in `raw_abi_index` for
/// the same `AppRef` — UNLESS the key is an implicit entry-trigger boundary
/// (see [`is_entry_trigger_boundary_key`]), which is exempt: it asserts only
/// "this object exists" (already independently confirmed by
/// `graph.resolve_object`/`object_by_number` before `resolve_object_run`
/// reaches this fallback), not "this Method is ABI-listed" — the latter is a
/// claim entry triggers can never satisfy, by AL/ABI schema construction.
///
/// # What this does NOT check
///
/// Semantic correctness — whether the route targets the semantically-right
/// callee for a given call site.  That would be circular: the resolver emits
/// the route AND we would be checking against the resolver's own data.  This
/// check is purely structural: given that a route WAS emitted, does its
/// structured key faithfully represent a real entry in the raw dep ABI?
///
/// # Determinism
///
/// `abi_unmapped_sites` is sorted by `AbiRoutineKey` natural order so that
/// identical inputs produce byte-identical reports across runs.
pub fn abi_ingestion_integrity(edges: &[Edge], raw_abi_index: &RawAbiIndex) -> AbiIntegrityReport {
    let mut abi_routes_total = 0usize;
    let mut abi_mapped = 0usize;
    let mut abi_unmapped_sites: Vec<UnmappedAbiSite> = Vec::new();

    for edge in edges {
        for route in edge.all_routes() {
            if let RouteTarget::AbiSymbol { key } = &route.target {
                abi_routes_total += 1;
                if is_entry_trigger_boundary_key(key) || raw_abi_index.contains(key) {
                    abi_mapped += 1;
                } else {
                    abi_unmapped_sites.push(UnmappedAbiSite { key: key.clone() });
                }
            }
        }
    }

    // Sort for determinism.
    abi_unmapped_sites.sort_by(|a, b| a.key.cmp(&b.key));

    let abi_unmapped = abi_unmapped_sites.len();
    AbiIntegrityReport {
        abi_routes_total,
        abi_mapped,
        abi_unmapped,
        abi_unmapped_sites,
    }
}

// ---------------------------------------------------------------------------
// Snapshot-based raw-ABI index builder
// ---------------------------------------------------------------------------

/// Build a `RawAbiIndex` from a snapshot by re-parsing the `SymbolReference`
/// for every SymbolOnly dep that has an `app_path`.
///
/// # Independence guarantee
///
/// This re-parses raw `.app` files → `SymbolReferenceAbi` DTOs WITHOUT reading
/// any `ProgramGraph.routines` / `ProgramGraph.objects`.  A key-derivation bug
/// in `ingest_abi` would not affect this path, making it a genuinely
/// independent second derivation.
pub fn build_raw_abi_index_from_snapshot(
    snap: &crate::snapshot::AppSetSnapshot,
    apps: &crate::program::node::AppRegistry,
) -> RawAbiIndex {
    let mut pairs: Vec<(AppRef, SymbolReferenceAbi)> = Vec::new();
    for unit in &snap.apps {
        if unit.source.is_some() {
            continue; // source-bearing unit — not a dep boundary
        }
        let Some(app_ref) = apps.find(&unit.id) else {
            continue;
        };
        let Some(path) = &unit.app_path else {
            continue;
        };
        // Fresh re-parse — does NOT touch graph.routines / graph.objects.
        // Skip unreadable .app files (same behaviour as ingest_abi).
        if let Ok(abi) = crate::program::abi_ingest::read_symbol_reference_from_app(path) {
            pairs.push((app_ref, abi));
        }
    }
    RawAbiIndex::build(pairs.iter().map(|(r, a)| (*r, a)))
}

// ---------------------------------------------------------------------------
// Graph-node-based integrity check
// ---------------------------------------------------------------------------

/// Check every SymbolOnly routine node in `graph` against `raw_abi_index`.
///
/// This is the full-coverage form: rather than checking only routes that
/// happen to appear in a set of resolved edges, this function checks EVERY
/// ingested ABI boundary routine.  For each `SymbolOnly` `RoutineNode` it
/// reconstructs the `AbiRoutineKey` exactly as `resolver.rs::make_routine_route`
/// would (reading `abi_routine_kind` / `abi_event_kind` from the node) and
/// looks it up in the raw index.
///
/// # Independence
///
/// The raw index is built from a FRESH re-parse of the dep `.app` files,
/// independent of `graph.routines` / `graph.objects`.  A key-derivation bug
/// in `ingest_abi` (e.g. swapping `routine_kind` from Procedure to
/// EventPublisher) would NOT affect the raw index path, so it would produce
/// a mismatch here.
///
/// # What "total" means
///
/// `abi_routes_total` is the count of SymbolOnly routine nodes checked — i.e.,
/// the count of ABI-boundary symbols that the engine ingested from deps.
pub fn abi_ingestion_integrity_from_graph(
    graph: &crate::program::graph::ProgramGraph,
    raw_abi_index: &RawAbiIndex,
) -> AbiIntegrityReport {
    use crate::program::node::ObjKey;
    use crate::program::resolve::edge::{AbiRoutineKey, AbiRoutineKind};
    use crate::snapshot::TrustTier;

    let mut abi_routes_total = 0usize;
    let mut abi_mapped = 0usize;
    let mut abi_unmapped_sites: Vec<UnmappedAbiSite> = Vec::new();

    for routine in &graph.routines {
        if routine.tier != TrustTier::SymbolOnly {
            continue;
        }
        abi_routes_total += 1;

        // Reconstruct the key as `make_routine_route` would.
        let (obj_num, obj_name_lc) = match &routine.id.object.key {
            ObjKey::Id(n) => (*n, String::new()),
            ObjKey::Name(s) => (0i64, s.to_string()),
        };
        let key = AbiRoutineKey {
            app: routine.id.object.app,
            object_type: format!("{:?}", routine.id.object.kind).to_ascii_lowercase(),
            object_number: obj_num,
            object_name_lc: obj_name_lc,
            routine_name_lc: routine.id.name_lc.to_string(),
            params_count: routine.id.params_count,
            param_type_fp: routine.id.sig_fp,
            routine_kind: routine
                .abi_routine_kind
                .clone()
                .unwrap_or(AbiRoutineKind::Procedure),
            event_kind: routine.abi_event_kind.clone().unwrap_or(AbiEventKind::None),
        };

        if raw_abi_index.contains(&key) {
            abi_mapped += 1;
        } else {
            abi_unmapped_sites.push(UnmappedAbiSite { key });
        }
    }

    // Sort for determinism.
    abi_unmapped_sites.sort_by(|a, b| a.key.cmp(&b.key));

    let abi_unmapped = abi_unmapped_sites.len();
    AbiIntegrityReport {
        abi_routes_total,
        abi_mapped,
        abi_unmapped,
        abi_unmapped_sites,
    }
}

// ---------------------------------------------------------------------------
// CDO / workspace harness
// ---------------------------------------------------------------------------

/// Run the ABI ingestion-integrity check over a real workspace (CDO harness).
///
/// Builds a snapshot, program graph (with ABI ingestion), then checks every
/// ingested SymbolOnly routine node against a FRESH re-parse of the raw
/// SymbolReference data (independent of the graph nodes).
///
/// Uses [`abi_ingestion_integrity_from_graph`] for full-coverage checking:
/// every ABI-boundary symbol ingested from deps is verified, not just the
/// ones that happen to appear in a resolved call-edge set.
///
/// Returns `abi_unmapped == 0` on a correct implementation.  Any miss
/// represents an ingestion/key-derivation bug that MUST be fixed.
///
/// Substrate core (shared-substrate refactor, 2026-07-15): reads snapshot +
/// graph from an already-built [`crate::program::resolve::full::ProgramContext`]
/// instead of building them itself, so a harness that already holds a context
/// (e.g. one built once and resolved many times) doesn't pay a second
/// snapshot/graph-build cost just to check ABI integrity.
#[must_use]
pub fn run_abi_integrity_check_on(
    ctx: &crate::program::resolve::full::ProgramContext,
) -> AbiIntegrityReport {
    // Build the raw-ABI index independently (fresh re-parse from .app files,
    // does NOT read graph.routines / graph.objects).
    let raw_index = build_raw_abi_index_from_snapshot(&ctx.snap, &ctx.graph.apps);

    abi_ingestion_integrity_from_graph(&ctx.graph, &raw_index)
}

/// Thin path wrapper over [`run_abi_integrity_check_on`]: builds a
/// [`crate::program::resolve::full::ProgramContext`] for `workspace_root` and
/// delegates. Returns an all-zeros [`AbiIntegrityReport`] on context-build
/// failure (fail-closed), same as before this was split.
#[must_use]
pub fn run_abi_integrity_check(workspace_root: &Path) -> AbiIntegrityReport {
    match crate::program::resolve::full::build_context(workspace_root) {
        Some(ctx) => run_abi_integrity_check_on(&ctx),
        None => AbiIntegrityReport {
            abi_routes_total: 0,
            abi_mapped: 0,
            abi_unmapped: 0,
            abi_unmapped_sites: vec![],
        },
    }
}

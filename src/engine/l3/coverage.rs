//! L3 COVERAGE (R2d) — the `AnalysisCoverage` "no silent clean" accounting,
//! ported from al-sem's `src/resolve/coverage.ts` (`buildCoverage`, 67 lines).
//!
//! This is the LAST source-only L3 sub-gate (R2a record-types, R2b call graph,
//! R2c event graph, R2d coverage). It is a pure function over the already-parity
//! resolved call graph (R2b) + the L2 routine flags (`bodyAvailable` /
//! `parseIncomplete`) + the workspace source units + the index-stage diagnostics.
//!
//! === The exact `buildCoverage` contract (Rev 2 MUST-FIXes) ===
//!
//!   - `sourceUnitsTotal` = count of `kind == "source"` units.
//!   - `sourceUnitsParsed` = that count MINUS the units whose `id` appears as the
//!     `sourceRef` of an index-stage diagnostic with EXACTLY
//!     `stage == "index"` && `severity == "warning"` && `sourceRef` present
//!     (`sourceRef === unit.id`). `info`-severity diagnostics do NOT decrement.
//!     SOURCE-ONLY: the corpus never emits such a warning (`failedUnitRefs` is
//!     empty → parsed == total), but the decrement logic is implemented correctly
//!     and exercised by the `warning_unparsed` vector.
//!   - `routinesTotal` = routine count; `routinesBodyAvailable` = count of
//!     `bodyAvailable`; `routinesParseIncomplete` = the StableRoutineId[] of the
//!     `parseIncomplete` routines. `bodyAvailable` and `parseIncomplete` are
//!     INDEPENDENT filters over the L2 flags, NOT a partition (a routine can be
//!     neither, e.g. a syntax-error body that still has a code block → both true).
//!   - `opaqueApps` = the appGuid[] of `sourceKind == "symbol-only"` apps. `[]`
//!     source-only (no deps); NON-EMPTY cross-app (R2.5b) — it lists the symbol-only
//!     dep app guids. As of R3a-0 (al-sem `81d538a`+`f1650ba`, Fix 2): `buildCoverage`
//!     reads `index.identity.apps.filter(sourceKind == "symbol-only")`, and
//!     `withDependencyArtifacts` now stamps the dep `AppIdentity`s (with `sourceKind`,
//!     from the artifact header) into `identity.apps` — so the symbol-only deps appear.
//!   - `unresolvedCallsites` = a MULTISET (`.map` over EDGES — NOT unique sites):
//!     every call-graph edge whose `resolution ∈ {unknown, ambiguous,
//!     member-not-found, external-target}` → its stable callsiteId. Duplicates are
//!     PRESERVED (an interface multi-edge callsite can emit the SAME stable id more
//!     than once); the projection sorts but NEVER dedups. `opaque`, `builtin`, and
//!     `maybe` resolutions are EXCLUDED.
//!   - `dynamicDispatchSites` = a MULTISET: edges with `dispatchKind == "dynamic"`
//!     → their stable operationId. Duplicates preserved; sorted; never deduped.
//!     `dynamicDispatchSites` is `OperationId[]` and `unresolvedCallsites` is
//!     `CallsiteId[]` — they are NOT array-subsets of each other.
//!
//! The projection ids are in STABLE form (StableRoutineId / StableCallsiteId /
//! StableOperationId), matching the golden generator byte-for-byte. Lists are
//! sorted with the same byte-order comparator the rest of L3 uses.

use std::collections::HashMap;

use super::call_graph_projection::cmp_stable;
use super::call_resolver::{CallEdge, DeclaredDependency, calls_for, resolve_calls};
use super::l3_workspace::{L3Resolved, L3Routine};
use super::symbol_table::SymbolTable;
use super::taxonomy::{DispatchKind, Resolution};

// ---------------------------------------------------------------------------
// Inputs the source-unit accounting needs (the L3 `buildCoverage` reads these
// from the index + the workspace providers). Kept minimal + owned so the offline
// vector path can construct them directly.
// ---------------------------------------------------------------------------

/// A workspace source unit (the `SourceUnit` subset `buildCoverage` reads: `id` +
/// `kind`). Only `kind == "source"` units are counted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoverageUnit {
    pub id: String,
    pub kind: String,
}

/// An index-stage diagnostic (the `Diagnostic` subset `buildCoverage` reads:
/// `stage` + `severity` + `sourceRef`). The MESSAGE is intentionally NOT carried —
/// `buildCoverage` keys ONLY on the (stage, severity, sourceRef) triple.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoverageDiagnostic {
    pub stage: String,
    pub severity: String,
    /// `sourceRef === SourceUnit.id` when present. `None` (absent) never decrements.
    pub source_ref: Option<String>,
}

// ---------------------------------------------------------------------------
// The AnalysisCoverage projection — the golden document shape. Field order +
// names match `scripts/r2d-l3cov-projection.ts` (and `model.ts AnalysisCoverage`).
// FORBIDDEN later-gate / L4 fields are structurally absent (built key-by-key).
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct AnalysisCoverage {
    #[serde(rename = "sourceUnitsTotal")]
    pub source_units_total: usize,
    #[serde(rename = "sourceUnitsParsed")]
    pub source_units_parsed: usize,
    #[serde(rename = "routinesTotal")]
    pub routines_total: usize,
    #[serde(rename = "routinesBodyAvailable")]
    pub routines_body_available: usize,
    /// SORTED StableRoutineId[] (duplicates preserved — never deduped).
    #[serde(rename = "routinesParseIncomplete")]
    pub routines_parse_incomplete: Vec<String>,
    /// appGuid[] of symbol-only apps — EMPTY source-only.
    #[serde(rename = "opaqueApps")]
    pub opaque_apps: Vec<String>,
    /// SORTED StableCallsiteId MULTISET (the 4 unresolved resolutions; dups preserved).
    #[serde(rename = "unresolvedCallsites")]
    pub unresolved_callsites: Vec<String>,
    /// SORTED StableOperationId MULTISET (dispatchKind == "dynamic"; dups preserved).
    #[serde(rename = "dynamicDispatchSites")]
    pub dynamic_dispatch_sites: Vec<String>,
}

// ---------------------------------------------------------------------------
// The 4 unresolved resolutions. Match EXACTLY — `opaque` / `builtin` / `maybe`
// are NOT counted (Rev 2 MUST-FIX #2).
// ---------------------------------------------------------------------------

fn is_unresolved_resolution(resolution: Resolution) -> bool {
    matches!(
        resolution,
        Resolution::Unknown(_)
            | Resolution::Ambiguous
            | Resolution::MemberNotFound
            | Resolution::ExternalTarget
    )
}

// ---------------------------------------------------------------------------
// Stable-id projection for callsite / operation ids. The internal RoutineId is
// `${modelInstanceId}/${hash}` (two `/`-parts) and the callsite / operation ids
// embed it as a prefix; rewrite the prefix to the StableRoutineId. Mirrors the
// `StableMap::stable_site` in call_graph_projection.rs.
// ---------------------------------------------------------------------------

fn stable_site(site_id: &str, by_internal: &HashMap<String, String>) -> String {
    match site_id.rsplit_once('/') {
        Some((prefix, suffix)) => {
            let stable_prefix = by_internal
                .get(prefix)
                .cloned()
                .unwrap_or_else(|| prefix.to_string());
            format!("{stable_prefix}/{suffix}")
        }
        None => site_id.to_string(),
    }
}

// ---------------------------------------------------------------------------
// The pure port of `buildCoverage`.
// ---------------------------------------------------------------------------

/// Build the `AnalysisCoverage` from the resolved-workspace inputs:
/// - `routines` — the L3 routines (carry `bodyAvailable` / `parseIncomplete` + the
///   StableRoutineId).
/// - `apps` — the workspace apps (for `opaqueApps`); a `(appGuid, sourceKind)` pair.
///   Source-only → all `"source"` → `opaqueApps` empty.
/// - `call_graph` — the resolved call edges (R2b), INTERNAL ids.
/// - `units` — the workspace source units.
/// - `index_diagnostics` — the index-stage diagnostics (for the decrement).
/// - `by_internal` — internal RoutineId → StableRoutineId map (for stable ids).
///
/// Multisets PRESERVE duplicates (the `.map` over edges); each is sorted with the
/// byte-order comparator at the end. The 4-resolution filter + the dynamic filter
/// match al-sem exactly.
pub fn build_coverage(
    routines: &[L3Routine],
    apps: &[(String, String)],
    call_graph: &[CallEdge],
    units: &[CoverageUnit],
    index_diagnostics: &[CoverageDiagnostic],
    by_internal: &HashMap<String, String>,
) -> AnalysisCoverage {
    // --- source units + the warning-diagnostic failedUnitRefs decrement. ---
    let source_units: Vec<&CoverageUnit> = units.iter().filter(|u| u.kind == "source").collect();
    let failed_unit_refs: std::collections::HashSet<&str> = index_diagnostics
        .iter()
        .filter(|d| d.stage == "index" && d.severity == "warning" && d.source_ref.is_some())
        .filter_map(|d| d.source_ref.as_deref())
        .collect();
    let source_units_parsed = source_units
        .iter()
        .filter(|u| !failed_unit_refs.contains(u.id.as_str()))
        .count();

    // --- opaque apps (symbol-only) — empty source-only. ---
    let opaque_apps: Vec<String> = apps
        .iter()
        .filter(|(_, source_kind)| source_kind == "symbol-only")
        .map(|(app_guid, _)| app_guid.clone())
        .collect();

    // --- routine counts + parse-incomplete StableRoutineIds. ---
    let routines_body_available = routines.iter().filter(|r| r.body_available).count();
    let mut routines_parse_incomplete: Vec<String> = routines
        .iter()
        .filter(|r| r.parse_incomplete)
        .map(|r| r.stable_routine_id.clone())
        .collect();
    routines_parse_incomplete.sort_by(|a, b| cmp_stable(a, b));

    // --- unresolved callsites MULTISET (4 resolutions; dups PRESERVED). ---
    let mut unresolved_callsites: Vec<String> = call_graph
        .iter()
        .filter(|e| is_unresolved_resolution(e.resolution))
        .map(|e| stable_site(&e.callsite_id, by_internal))
        .collect();
    unresolved_callsites.sort_by(|a, b| cmp_stable(a, b));

    // --- dynamic dispatch sites MULTISET (dispatchKind == "dynamic"; dups PRESERVED). ---
    let mut dynamic_dispatch_sites: Vec<String> = call_graph
        .iter()
        .filter(|e| e.dispatch_kind == DispatchKind::Dynamic)
        .map(|e| stable_site(&e.operation_id, by_internal))
        .collect();
    dynamic_dispatch_sites.sort_by(|a, b| cmp_stable(a, b));

    AnalysisCoverage {
        source_units_total: source_units.len(),
        source_units_parsed,
        routines_total: routines.len(),
        routines_body_available,
        routines_parse_incomplete,
        opaque_apps,
        unresolved_callsites,
        dynamic_dispatch_sites,
    }
}

// ---------------------------------------------------------------------------
// L3Resolved entry point — the post-resolve / pre-summary capture the dump reads.
// ---------------------------------------------------------------------------

impl L3Resolved {
    /// Build the `AnalysisCoverage` for the resolved workspace (R2d).
    ///
    /// SOURCE-ONLY capture: the resolved call graph is built ONCE here
    /// (`resolve_calls` with empty declared deps + empty fetched set — the same
    /// "read-once, post-resolve" capture `project_call_graph` uses), all apps are
    /// `"source"` (→ `opaqueApps` empty), and the workspace carries no index-stage
    /// warning (→ `failedUnitRefs` empty → parsed == total). `units` /
    /// `index_diagnostics` are passed in by the caller (the dump supplies the real
    /// discovered units; the offline path supplies the vector's units).
    pub fn project_coverage(
        &self,
        units: &[CoverageUnit],
        index_diagnostics: &[CoverageDiagnostic],
    ) -> AnalysisCoverage {
        let ws = &self.workspace;
        let symbols = SymbolTable::build(&ws.objects, &ws.tables, &ws.routines);
        let resolved = calls_for(self, &symbols);
        // Coverage is the primary app's: a cross-app model (engine-switch S8.3) also
        // holds the dependency code the workspace demands, which is not the
        // workspace's to cover. A single-app model holds nothing else.
        let primary = self
            .primary_app
            .as_ref()
            .map(|a| a.app_guid.to_ascii_lowercase());
        let in_primary = |guid: &str| {
            primary
                .as_deref()
                .is_none_or(|p| guid.eq_ignore_ascii_case(p))
        };
        let routines: Vec<L3Routine> = ws
            .routines
            .iter()
            .filter(|r| in_primary(&r.app_guid))
            .cloned()
            .collect();

        let by_internal: HashMap<String, String> = routines
            .iter()
            .map(|r| (r.id.clone(), r.stable_routine_id.clone()))
            .collect();

        // Source-only: every workspace app is `"source"`. The L3 model has no
        // dependency-app boundaries in this path (those arrive in R2.5), so
        // `opaqueApps` is structurally empty. Derive the app guids from the
        // objects' app guid (a single-app source workspace).
        let mut app_guids: Vec<String> = ws
            .objects
            .iter()
            .filter(|o| in_primary(&o.app_guid))
            .map(|o| o.app_guid.clone())
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();
        app_guids.sort();
        let apps: Vec<(String, String)> = app_guids
            .into_iter()
            .map(|g| (g, "source".to_string()))
            .collect();

        build_coverage(
            &routines,
            &apps,
            &resolved.edges,
            units,
            index_diagnostics,
            &by_internal,
        )
    }

    /// Disk-backed coverage capture (the emitter + differential entry point):
    /// re-discover the workspace's `.al` files as `ws:<relPosix>` source units,
    /// then build the coverage over the resolved model. SOURCE-ONLY: no index-stage
    /// warnings are produced (the discovery already skipped unreadable files, and
    /// tree-sitter never throws here), so `index_diagnostics` is empty and parsed ==
    /// total. Mirrors `assemble_and_resolve_workspace`'s discovery so the unit ids +
    /// order match `project_workspace` byte-for-byte.
    pub fn project_coverage_disk(&self, workspace: &std::path::Path) -> AnalysisCoverage {
        let units = coverage_source_units_for_workspace(workspace);
        let diagnostics: Vec<CoverageDiagnostic> = Vec::new();
        self.project_coverage(&units, &diagnostics)
    }

    /// CROSS-APP (R2.5b) coverage capture: the merged workspace+dep model with the
    /// real dep ledger. `apps` is `(appGuid, sourceKind)` for the workspace ("source")
    /// plus each dep ("symbol-only" | "app-source").
    ///
    /// FIXED (R3a-0 semantic-oracle epoch, al-sem `81d538a`+`f1650ba`+`93e360d`):
    ///   - `opaqueApps` = the `sourceKind == "symbol-only"` dep app guids. al-sem's Fix 2
    ///     stamps the dep `AppIdentity`s (with `sourceKind`, derived from the artifact
    ///     header) into `index.identity.apps` via `withDependencyArtifacts`, and
    ///     `buildCoverage` reads `identity.apps.filter(sourceKind=="symbol-only")`. So we
    ///     pass ALL apps (workspace + deps) to `build_coverage` and let its `symbol-only`
    ///     filter populate `opaqueApps`.
    ///   - The call resolution INSIDE coverage threads the REAL declared/fetched ledger
    ///     (Fix 1: al-sem now reads `identity.primaryDependencies` DURING resolve, in both
    ///     production AND the capture harness as of `93e360d`). The `unresolvedCallsites`
    ///     multiset matches al-sem's resolved `model.callGraph`.
    ///
    /// On the ALL-FETCHED R2.5b corpus (the prior `Lib Absent` unfetched dep was removed in
    /// al-sem `93e360d`) threading the real ledger is BYTE-INVARIANT vs the empty ledger:
    /// `has_unfetched_declared_dependency` is false, so the `gone.M()` member miss stays
    /// `external-target` and remains IN `unresolvedCallsites`.
    pub fn project_coverage_cross_app(
        &self,
        units: &[CoverageUnit],
        index_diagnostics: &[CoverageDiagnostic],
        apps: &[(String, String)],
        declared_dep_app_guids: &[String],
        fetched_app_guids: &[String],
    ) -> AnalysisCoverage {
        let ws = &self.workspace;
        let symbols = SymbolTable::build(&ws.objects, &ws.tables, &ws.routines);
        // Thread the REAL declared/fetched ledger — mirrors fixed production + capture al-sem
        // (reads primaryDependencies DURING resolve). Byte-invariant on the all-fetched corpus.
        let declared: Vec<DeclaredDependency> = declared_dep_app_guids
            .iter()
            .map(|app_guid| DeclaredDependency {
                app_guid: app_guid.clone(),
            })
            .collect();
        let resolved = resolve_calls(ws, &symbols, &declared, fetched_app_guids);

        let by_internal: HashMap<String, String> = ws
            .routines
            .iter()
            .map(|r| (r.id.clone(), r.stable_routine_id.clone()))
            .collect();

        // FIXED (R3a-0, Fix 2): pass ALL apps (workspace "source" + each dep). The
        // `symbol-only` filter in build_coverage now populates opaqueApps with the
        // symbol-only dep app guids (al-sem's identity.apps now carries them).
        build_coverage(
            &ws.routines,
            apps,
            &resolved.edges,
            units,
            index_diagnostics,
            &by_internal,
        )
    }
}

/// Re-discover a workspace's `.al` files as `ws:<relPosix>` source units, in the
/// rel-posix-sorted discovery order. Used by the disk-backed coverage capture so
/// `sourceUnitsTotal` matches al-sem's unit count. Returns an empty list on any
/// discovery failure (fail-closed — never throws).
///
/// App-scoped (engine-switch S5.1): a child directory with its own `app.json` is
/// another app, and the model analyses only the root app's files
/// (`select_program_files`, the disk L3 path). Counting the nested app's files here
/// reported them as parsed although no routine of theirs was analysed.
pub fn coverage_source_units_for_workspace(workspace: &std::path::Path) -> Vec<CoverageUnit> {
    use crate::engine::l2::l2_workspace::discover_al_files_app_scoped;
    let Ok(discovered) = discover_al_files_app_scoped(workspace) else {
        return Vec::new();
    };
    discovered
        .iter()
        .map(|f| CoverageUnit {
            id: format!("ws:{}", f.rel_posix),
            kind: "source".to_string(),
        })
        .collect()
}

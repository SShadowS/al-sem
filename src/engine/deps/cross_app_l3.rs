//! R2.5b CROSS-APP L3 wiring — feed the R2.5a merged index (workspace native
//! entities + projected `.app`-dep objects/tables/routines) into the SAME already
//! ported L3 pipeline (`l3_workspace::resolve` → call/event/coverage projections),
//! so cross-app callsites / record-vars / subscribers RESOLVE.
//!
//! ## What this is (and is NOT)
//!
//! - It is WIRING + the merged input. There is NO new L3 ALGORITHM here: the dep
//!   entities are converted to the EXACT `L3Object`/`L3Table`/`L3Routine` shape the
//!   native source path produces, APPENDED to the workspace AFTER the native
//!   entities (mirroring al-sem's `withDependencyArtifacts`, which `push`es dep
//!   entities last — `src/deps/dependency-artifact.ts:213-215`), and the standard
//!   `l3_workspace::resolve` runs over the merged whole.
//! - The append order is LOAD-BEARING: the symbol table is LAST-wins and the
//!   extension-field merge is FIRST-wins, both keyed off assembled order — so
//!   workspace-first / dep-last reproduces al-sem's collision/shadowing semantics.
//!
//! ## L4 / cone / summary LEAKAGE BOUNDARY (Rev 2 #5)
//!
//! The input to L3 is an **L3-only** merged index. The dep side comes from
//! `project_abi_to_index` (`ProjectedObject`/`ProjectedTable`/`ProjectedRoutine`),
//! whose Rust types STRUCTURALLY DO NOT CARRY any L4 field — there is no `summary`,
//! `intraAppCallEdges`, `citedOperationEvidence`, `depOrderIndex`, capability-cone,
//! or typed-edge field anywhere on these structs. So a poisoned merged model
//! "carrying" such fields cannot influence L3: there is nowhere for them to live.
//! The poison NEGATIVE test (`cross_app_l3_poison.rs`) proves this by constructing
//! the merged L3 with bogus extra-field-bearing inputs and asserting the projection
//! is byte-identical. DO NOT add an L4 field to the L3 entity structs — the boundary
//! is enforced by the type, not by a runtime strip.
//!
//! ## Capture-point mutation audit (Rev 2 #3) — dep-entity in-place mutations
//!
//! `resolveModel` mutates these on DEP-origin entities; `l3_workspace::resolve`
//! reproduces each, and the projections read POST-resolve values:
//!   1. **Extension-field merge** (`merge_extension_fields`): a dep `TableExtension`'s
//!      fields are merged INTO its base table (here: dep `Dep Vendor` gains the dep
//!      ext's `Rating`; a WORKSPACE `TableExtension` on a dep table merges its field
//!      onto the dep base table — both directions cross the app boundary). This is
//!      the ONLY mutation that touches a dep-ORIGIN identity field.
//!   2. **record-var `tableId` backfill** (`resolve_record_types`) + **`argumentBindings`
//!      upgrade** (`upgrade_bindings`): under `noDepSummaries:true` the dep routines
//!      carry EMPTY features (no record vars / call sites), so these mutate only the
//!      WORKSPACE caller's record vars / callsite bindings — never the dep routine.
//!      Confirmed against al-sem (dep routines: recordVariables=[], callSites=[]).
//!
//! So feeding the dep entities + running the unchanged `resolve` is sufficient — the
//! same three resolve sub-steps mutate the same fields al-sem mutates.

use std::path::Path;

use crate::engine::deps::merged_index::collect_app_paths;
use crate::engine::deps::projection::{ProjectedObject, ProjectedRoutine, ProjectedTable};
// The temp-state constructors are shared `pub(crate)` from `l2::scope` (ONE definition,
// compiler-enforced on any future `PTempState` shape change). Task 6 (G7, RV-4).
use crate::engine::l3::l3_workspace::{
    L3Object, L3Resolved, L3Routine, L3Table, L3Workspace, assemble_l3_workspace_from_disk, resolve,
};
use crate::program::model::abi_rows::append_dep_entities;

/// The merged-input context: the assembled+resolved cross-app workspace plus the
/// dep-app ledger the call-graph / coverage projections need (declared deps,
/// fetched app guids, and per-app sourceKind for `opaqueApps`).
pub struct CrossAppL3 {
    pub resolved: L3Resolved,
    /// Declared dependency app guids (from the workspace app.json) — drives the
    /// member-call opaque-vs-external-target split (`has_unfetched_declared_dependency`).
    pub declared_dep_app_guids: Vec<String>,
    /// Dep app guids actually FETCHED (a readable `.app` produced entities).
    pub fetched_app_guids: Vec<String>,
    /// `(appGuid, sourceKind)` for every app — workspace ("source") + each dep
    /// ("symbol-only" | "app-source"). Drives coverage `opaqueApps`.
    pub apps: Vec<(String, String)>,
    /// `(appGuid, version)` for every FETCHED dep `.app` (from its manifest identity).
    /// The resolved-version side of d17's MinVersion-vs-resolved drift check
    /// (al-sem `model.apps[].version`). ADDITIVE — only the d17 plumbing reads it.
    pub dep_app_versions: Vec<(String, String)>,
}

/// Test-only (Task 6, G7/RV-4): project a [`ProjectedAbi`] into a STANDALONE L3
/// workspace (no native source) and run the standard `resolve()` over it. Exercises
/// the SAME `dep_*_to_l3` conversion + the merged-whole resolve path the production
/// `build_cross_app_l3_impl` uses, so the synthesized per-param record-var temp
/// shapes (incl. the table-level override) are validated end-to-end without a `.app`.
#[doc(hidden)]
pub fn project_dep_abi_to_l3_for_test(
    projected: &crate::engine::deps::projection::ProjectedAbi,
) -> L3Workspace {
    let mut ws = L3Workspace {
        objects: Vec::new(),
        tables: Vec::new(),
        routines: Vec::new(),
    };
    append_dep_entities(
        &mut ws,
        &projected.objects,
        &projected.tables,
        &projected.routines,
    );
    resolve(&mut ws);
    ws
}

/// Build the cross-app L3 from a disk workspace (native `.al` source) + its dep
/// `.app`(s). `declared_dep_app_guids` is the workspace app.json `dependencies[]`
/// app-guid list (some may be UNFETCHED — declared but no `.app` present → the
/// opaque-vs-external-target split). `alpackages_path` is where the dep `.app`(s)
/// live (typically `<workspace>/.alpackages`).
///
/// Fail-closed: an unsound/empty native layout yields `None` (mirrors
/// `assemble_and_resolve_workspace`); a bad `.app` contributes nothing. Never panics.
pub fn build_cross_app_l3(
    workspace: &Path,
    alpackages_path: &Path,
    declared_dep_app_guids: &[String],
    model_instance_id: &str,
) -> Option<CrossAppL3> {
    build_cross_app_l3_impl(
        workspace,
        alpackages_path,
        declared_dep_app_guids,
        model_instance_id,
        false,
    )
}

/// R4 variant of [`build_cross_app_l3`] that PARSES the embedded `.al` source of
/// app-source deps (`includes_source`) instead of the symbol-only ABI projection —
/// mirroring al-sem's `noDepSummaries:false` ingestion (`dependency-pipeline.ts`:
/// `if (ref.includesSource) { parse+index embedded source } else { project ABI }`).
/// This materializes a dep's `OnRun` trigger / `[InternalProc]` / `[Obsolete]`
/// routines that the symbol reference omits — the substrate the d13/d16/d17
/// cross-app finding goldens require. ADDITIVE: the existing symbol-only callers
/// (R3a5 gate, aldump) keep [`build_cross_app_l3`].
pub fn build_cross_app_l3_r4(workspace: &Path, model_instance_id: &str) -> Option<CrossAppL3> {
    let declared = read_workspace_declared_dep_app_guids(workspace);
    let alpackages = workspace.join(".alpackages");
    build_cross_app_l3_impl(workspace, &alpackages, &declared, model_instance_id, true)
}

fn build_cross_app_l3_impl(
    workspace: &Path,
    alpackages_path: &Path,
    declared_dep_app_guids: &[String],
    model_instance_id: &str,
    parse_embedded_source: bool,
) -> Option<CrossAppL3> {
    // 1. Assemble the NATIVE workspace L3 model from `.al` source (pre-resolve).
    let mut ws = assemble_l3_workspace_from_disk(workspace, model_instance_id)?;

    // 2. Read + project the dep `.app`(s). Collect (appGuid, sourceKind) for the
    //    apps ledger; track which dep app guids were actually fetched.
    let mut dep_objects: Vec<ProjectedObject> = Vec::new();
    let mut dep_tables: Vec<ProjectedTable> = Vec::new();
    let mut dep_routines: Vec<ProjectedRoutine> = Vec::new();
    let mut apps: Vec<(String, String)> = Vec::new();
    let mut fetched_app_guids: Vec<String> = Vec::new();
    let mut dep_app_versions: Vec<(String, String)> = Vec::new();

    // The workspace app(s) are "source". Derive from the native objects' app guid.
    let mut ws_app_guids: Vec<String> = ws
        .objects
        .iter()
        .map(|o| o.app_guid.clone())
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();
    ws_app_guids.sort();
    for g in &ws_app_guids {
        apps.push((g.clone(), "source".to_string()));
    }

    // Embedded-source-parsed dep L3 entities (R4 path only). Appended AFTER the
    // ABI-projected ones so the workspace-first/deps-last order is preserved.
    let mut src_dep_objects: Vec<L3Object> = Vec::new();
    let mut src_dep_tables: Vec<L3Table> = Vec::new();
    let mut src_dep_routines: Vec<L3Routine> = Vec::new();

    for app_path in collect_app_paths(alpackages_path) {
        let Ok(bytes) = std::fs::read(&app_path) else {
            continue;
        };
        let Some(parsed) =
            crate::engine::deps::merged_index::parse_dep_app_public(&bytes, model_instance_id)
        else {
            continue;
        };
        fetched_app_guids.push(parsed.app_guid.clone());
        dep_app_versions.push((parsed.app_guid.clone(), parsed.version.clone()));
        apps.push((
            parsed.app_guid.clone(),
            if parsed.includes_source {
                "app-source".to_string()
            } else {
                "symbol-only".to_string()
            },
        ));

        if parse_embedded_source && parsed.includes_source {
            // --- embedded-source path (al-sem `if ref.includesSource`) ---
            // Parse the embedded `.al` units (sourceUnitId = `dep:<appGuid>:<relpath>`,
            // appGuid = dep guid) into FULL L3 entities (with bodies + attributes),
            // exactly the R3a-4 stabilizer pattern. resolve() runs over the MERGED
            // whole later, so DON'T resolve per-dep here.
            let embedded = crate::engine::deps::dep_artifact_l4::iterate_embedded_source(&bytes);
            let units: Vec<(String, String)> = embedded
                .iter()
                .map(|f| {
                    (
                        format!("dep:{}:{}", parsed.app_guid, f.relative_path),
                        f.content.clone(),
                    )
                })
                .collect();
            let dep_ws = crate::engine::l3::l3_workspace::assemble_workspace_units(
                &units,
                &parsed.app_guid,
                model_instance_id,
            );
            src_dep_objects.extend(dep_ws.objects);
            src_dep_tables.extend(dep_ws.tables);
            src_dep_routines.extend(dep_ws.routines);
        } else {
            // --- symbol-only ABI projection (default) ---
            dep_objects.extend(parsed.objects);
            dep_tables.extend(parsed.tables);
            dep_routines.extend(parsed.routines);
        }
    }

    // 3. MERGE: append dep entities (workspace first, deps last).
    append_dep_entities(&mut ws, &dep_objects, &dep_tables, &dep_routines);
    // 3b. Append embedded-source-parsed dep entities (R4 path). These are already L3
    //     entities (full bodies + attributes), so push directly — deps still last.
    ws.objects.extend(src_dep_objects);
    ws.tables.extend(src_dep_tables);
    ws.routines.extend(src_dep_routines);

    // 4. RESOLVE the merged whole (build_symbol_table → resolve_record_types →
    //    merge_extension_fields). Same `resolve` the native path runs — no new algo.
    resolve(&mut ws);

    // R4-F: classify AST roots over the MERGED whole, then overlay
    // `<workspace>/roots.config.json` (config lives at the workspace root).
    let (root_classifications, infra_diagnostics) =
        crate::engine::root_classification::compute_root_classifications(&ws, Some(workspace));

    Some(CrossAppL3 {
        resolved: L3Resolved {
            workspace: ws,
            root_classifications,
            // Cross-app path: primary_app is populated separately via the
            // workspace app.json that the gate's `read_workspace_apps` reads.
            // The cross-app L3 constructor has no workspace path here
            // (it receives a pre-assembled workspace), so primary_app = None.
            primary_app: None,
            infra_diagnostics,
            precomputed_calls: None,
            precomputed_events: None,
        },
        declared_dep_app_guids: declared_dep_app_guids.to_vec(),
        fetched_app_guids,
        apps,
        dep_app_versions,
    })
}

pub use crate::program::model::workspace::DeclaredDependencyDecl;

/// Read the workspace root `app.json` `dependencies[].id` app guids (the DECLARED
/// deps, some of which may be UNfetched). Returns `[]` on any read/parse miss
/// (fail-closed). Mirrors al-sem `parseWorkspaceDependencies` (the app-guid subset).
pub fn read_workspace_declared_dep_app_guids(workspace: &Path) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(workspace.join("app.json")) else {
        return Vec::new();
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
        return Vec::new();
    };
    let Some(deps) = value.get("dependencies").and_then(|d| d.as_array()) else {
        return Vec::new();
    };
    deps.iter()
        .filter_map(|d| {
            // al-sem accepts `id` (modern) or `appId` (legacy) — match its parser.
            d.get("id")
                .or_else(|| d.get("appId"))
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
        })
        .collect()
}

/// Convenience: build the cross-app L3 over a disk workspace that has its dep
/// `.app`(s) under `<workspace>/.alpackages`, reading the declared deps from the
/// workspace `app.json`. The single entry point the `aldump` cross-app mode drives.
pub fn build_cross_app_l3_from_workspace(
    workspace: &Path,
    model_instance_id: &str,
) -> Option<CrossAppL3> {
    let declared = read_workspace_declared_dep_app_guids(workspace);
    let alpackages = workspace.join(".alpackages");
    build_cross_app_l3(workspace, &alpackages, &declared, model_instance_id)
}

impl CrossAppL3 {
    /// Cross-app L3 record-type projection (R2.5b-a) — record vars now bind to dep
    /// StableTableIds; dep / ws TableExtension fields merged onto the dep base table.
    /// Identical to the source-only `project()` (record-types need no dep ledger).
    pub fn project_record_types(&self) -> crate::engine::l3::l3_workspace::L3RecordTypeProjection {
        self.resolved.project()
    }

    /// Cross-app L3 call-graph projection (R2.5b-b) — cross-app member calls resolve
    /// to dep StableRoutineIds; the dep ledger drives the opaque/external-target split.
    pub fn project_call_graph(
        &self,
    ) -> crate::engine::l3::call_graph_projection::L3CallGraphProjection {
        self.resolved
            .project_call_graph_cross_app(&self.declared_dep_app_guids, &self.fetched_app_guids)
    }

    /// Cross-app L3 event-graph projection (R2.5b-c) — ws subscriber → dep publisher,
    /// dep subscriber → ws publisher. Identical to the source-only `project_event_graph`
    /// (the event graph reads dep `attributes_parsed`, no dep ledger needed).
    pub fn project_event_graph(&self) -> crate::engine::l3::event_graph::L3EventGraphProjection {
        self.resolved.project_event_graph()
    }

    /// Cross-app L3 coverage projection (R2.5b-d). `opaqueApps` lists the symbol-only
    /// dep app guids (R3a-0 Fix 2, al-sem `81d538a`+`f1650ba`): `buildCoverage` reads
    /// `index.identity.apps.filter(sourceKind == "symbol-only")`, and
    /// `withDependencyArtifacts` now stamps the dep `AppIdentity`s (with `sourceKind`)
    /// into `identity.apps`, so the symbol-only deps are present. The observable
    /// cross-app coverage signal also includes the `unresolvedCallsites` /
    /// `dynamicDispatchSites` multiset delta (cross-app member calls that RESOLVED drop
    /// OUT; the external-target miss stays IN), and `routinesTotal` counts dep routines.
    ///
    /// The call resolution INSIDE this projection threads the REAL declared/fetched ledger
    /// (Fix 1: al-sem reads `identity.primaryDependencies` DURING resolve, in production AND
    /// the capture harness as of `93e360d`). On the all-fetched corpus the `gone.M()` member
    /// miss is `external-target` (genuinely — all declared deps fetched) and stays IN
    /// `unresolvedCallsites`; the unfetched-declared-dep member-`opaque` branch is proven by
    /// `tests/r3a0_unfetched_dep_opaque.rs`.
    pub fn project_coverage(
        &self,
        units: &[crate::engine::l3::coverage::CoverageUnit],
        index_diagnostics: &[crate::engine::l3::coverage::CoverageDiagnostic],
    ) -> crate::engine::l3::coverage::AnalysisCoverage {
        self.resolved.project_coverage_cross_app(
            units,
            index_diagnostics,
            &self.apps,
            &self.declared_dep_app_guids,
            &self.fetched_app_guids,
        )
    }

    /// Disk-backed cross-app coverage capture: re-discover the workspace `.al` files
    /// as source units (the dep `.app`s are NOT source units), then build coverage.
    pub fn project_coverage_disk(
        &self,
        workspace: &Path,
    ) -> crate::engine::l3::coverage::AnalysisCoverage {
        let units = crate::engine::l3::coverage::coverage_source_units_for_workspace(workspace);
        let diagnostics: Vec<crate::engine::l3::coverage::CoverageDiagnostic> = Vec::new();
        self.project_coverage(&units, &diagnostics)
    }
}

// ---------------------------------------------------------------------------
// Native oracles — #[cfg(test)]
// ---------------------------------------------------------------------------

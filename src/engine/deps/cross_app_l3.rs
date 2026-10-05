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
use crate::engine::l2::scope::{ts_known, ts_param_dependent};
use crate::engine::l3::l3_workspace::{
    L3Field, L3Object, L3PageControl, L3Parameter, L3Resolved, L3Routine, L3Table, L3Workspace,
    PageControlKind, RoutineVariables, assemble_l3_workspace_from_disk, resolve,
};

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

/// Convert one projected dep object into the L3 object shape (identical to the
/// native source path's `L3Object`). The dep object carries the same identity
/// (StableObjectId-independent internal id) the native path mints.
fn dep_object_to_l3(o: &ProjectedObject) -> L3Object {
    L3Object {
        id: o.id.clone(),
        app_guid: o.app_guid.clone(),
        object_type: o.object_type.clone(),
        object_number: o.object_number,
        name: o.name.clone(),
        source_table_name: o.source_table_name.clone(),
        extends_target_name: o.extends_target_name.clone(),
        implements_interfaces: o.implements_interfaces.clone(),
        // The ABI projection DOES carry `object_subtype` (projection.rs:116) —
        // forward it so native + ABI agree on the L3Object shape (d46 reads it).
        object_subtype: o.object_subtype.clone(),
        // The ABI projection DOES carry `page_type` (projection.rs) — forward it so
        // native + ABI agree on the L3Object shape and a cross-app `PageType=API`
        // dependency page classifies as `api-page` (mirrors the `object_subtype`
        // forward above and al-sem dependency-projection.ts).
        page_type: o.page_type.clone(),
        // The ABI projection DOES carry `inherent_commit_behavior` (projection.rs:121,
        // symbol_reference.rs:99) in canonical lower-case form — forward it so native
        // + ABI agree on the L3Object shape. Consumed by return_summary to merge
        // object-level commit behavior into each dep routine's commitBehavior.
        inherent_commit_behavior: o.inherent_commit_behavior.clone(),
        source_table_temporary: None,
        page_controls: o
            .page_controls
            .iter()
            .map(|(n, k, t)| L3PageControl {
                name: n.clone(),
                kind: match k.as_str() {
                    "systempart" => PageControlKind::SystemPart,
                    "usercontrol" => PageControlKind::UserControl,
                    _ => PageControlKind::Part,
                },
                target: t.clone(),
            })
            .collect(),
        // The ABI projection does not carry `SingleInstance` / page write-surface
        // booleans / a decl anchor — dep objects default to `None`, mirroring the
        // `source_table_temporary: None` default above.
        single_instance: None,
        editable: None,
        insert_allowed: None,
        modify_allowed: None,
        delete_allowed: None,
        source_anchor: None,
    }
}

/// Convert one projected dep field into the L3 field shape.
fn dep_field_to_l3(f: &crate::engine::deps::projection::ProjectedField) -> L3Field {
    L3Field {
        id: f.id.clone(),
        physical_table_id: f.physical_table_id.clone(),
        declaring_object_id: f.declaring_object_id.clone(),
        declaring_app_id: f.declaring_app_id.clone(),
        field_number: f.field_number,
        name: f.name.clone(),
        field_class: f.field_class.clone(),
        data_type: f.data_type.clone(),
        is_blob_like: f.is_blob_like,
    }
}

/// Convert one projected dep table into the L3 table shape.
fn dep_table_to_l3(t: &ProjectedTable) -> L3Table {
    L3Table {
        id: t.id.clone(),
        app_guid: t.app_guid.clone(),
        table_number: t.table_number,
        name: t.name.clone(),
        fields: t.fields.iter().map(dep_field_to_l3).collect(),
        // Dep (.app symbol) tables carry no parsed keys (the ABI projection does
        // not expose them); the cli-b snapshot corpus is source-only anyway.
        keys: Vec::new(),
        // Task 6 (G7, RV-4): forward the ABI `TableType = Temporary` marker so the
        // merged-whole `resolve()` table-level override (Task 4) upgrades a record var
        // typed on this dep table to Known(true) — native+ABI shape parity.
        is_temporary: t.is_temporary,
        // G-5: the ABI projection carries no extension-stub marker; dep tables are
        // treated as real (preserves the pre-G-5 LAST-wins semantics for dep sets).
        is_extension_stub: false,
    }
}

/// Convert one projected dep routine into the L3 routine shape. Dep routines carry
/// EMPTY features (no record vars / ops / variables / call sites) — matching
/// al-sem's dep routines under `noDepSummaries:true`. The `parameters` (for arity +
/// the event-graph publisher param shape), `attributes_parsed` (the event-graph
/// inputs), `kind`, and identity fields ARE carried.
fn dep_routine_to_l3(r: &ProjectedRoutine, object_type: &str) -> L3Routine {
    // objectId = `${appGuid}/${objectType}/${objectNumber}` — recover the parts.
    let parts: Vec<&str> = r.object_id.split('/').collect();
    let app_guid = parts.first().copied().unwrap_or("").to_string();
    let object_number = parts
        .get(2)
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or(0);

    // Task 6 (G7, RV-4): NET-NEW per-param record-var temp-state modeling for ABI
    // routines. The native source path (`l2::scope::extract_record_variables`)
    // synthesizes a `record_variables` entry for every RECORD-typed parameter, with a
    // base `temp_state` per the native rule:
    //   - param with the `temporary` marker (here `AbiParameter.is_temporary`) → Known(true)
    //   - by-var record param WITHOUT marker → ParameterDependent(param_index)
    //   - by-value record param → Known(false)
    // The table-level override (a param typed on a `TableType = Temporary` table →
    // Known(true), Task 4 precedence) is NOT applied here: we set `table_name` so the
    // merged-whole `resolve()` (resolve_record_types) backfills `table_id` and runs the
    // SAME final override pass that native uses — keeping ONE precedence rule everywhere.
    // `table_name` is derived from the param's `type_text` (`record_table_name_of`);
    // the ABI symbol format carries the subtype in the type text, so this is sufficient.
    // If the type text yields no table name, `table_name` stays None (resolve leaves
    // `table_id` None; the base temp_state still holds — engine never throws).
    let record_variables: Vec<crate::engine::l3::l3_workspace::L3RecordVariable> = r
        .parameters
        .iter()
        .filter(|p| p.is_record)
        .map(|p| {
            let pidx = p.index as u32;
            let temp_state = if p.is_temporary {
                ts_known(true)
            } else if p.is_var {
                ts_param_dependent(pidx)
            } else {
                ts_known(false)
            };
            crate::engine::l3::l3_workspace::L3RecordVariable {
                id: format!("{}/rv/{}", r.id, p.name.to_lowercase()),
                name: p.name.clone(),
                table_name: crate::engine::l3::record_types::record_table_name_of(&p.type_text),
                table_id: None,
                is_parameter: true,
                parameter_index: Some(pidx),
                temp_state,
                // Shape parity with native: the native body-walk param record var
                // hardcodes `scope: None` (l2/mod.rs:312 — only object-GLOBAL vars get
                // Some("global")). Match it so detectors treat dep and workspace params
                // identically. scope is unserialized / unread today (latent-parity).
                scope: None,
            }
        })
        .collect();

    let parameters = r
        .parameters
        .iter()
        .map(|p| L3Parameter {
            index: p.index as u32,
            name: p.name.clone(),
            type_text: p.type_text.clone(),
            is_var: p.is_var,
            is_record: p.is_record,
            // PARITY (R2.5b-c): al-sem's dep-routine parameters come from the ABI
            // symbol-reference projection (`dependency-projection.ts`), which does NOT
            // populate `tableName` on a record parameter (only the NATIVE grammar path
            // sets it). The event-graph publisher param shape (R2.5b-c) is the first
            // golden consumer of a dep routine's param `tableName`, and al-sem omits it
            // there — so a dep record param's `table_name` MUST be `None` to byte-match.
            // (Deriving it from `type_text` here over-populated vs al-sem; corrected.)
            table_name: None,
        })
        .collect();

    L3Routine {
        id: r.id.clone(),
        stable_routine_id: r.stable_routine_id.clone(),
        object_id: r.object_id.clone(),
        object_type: object_type.to_string(),
        name: r.name.clone(),
        kind: r.kind.clone(),
        attributes_parsed: r.attributes_parsed.clone(),
        app_guid,
        object_number,
        normalized_signature_hash: r.signature_fingerprint.clone(),
        body_available: r.body_available, // false for dep routines.
        parse_incomplete: false,
        record_variables,
        record_operations: Vec::new(),
        field_accesses: Vec::new(),
        // A dep routine's object globals are not modelled by the ABI at all.
        variables: RoutineVariables::default(),
        parameters,
        // The ABI symbol reference DOES expose access modifiers (`IsInternal`/`IsLocal`),
        // and `project_abi_to_index` already computes `ProjectedRoutine.access_modifier`
        // from them — faithful to al-sem `dependency-projection.ts`, which populates a dep
        // routine's `accessModifier`. Forward it (byte-invariant today: L3Routine.access_modifier
        // is not serialized into any gate, and d32 skips bodyless dep routines — but d13
        // cross-app-internal-call WILL read it, so dropping it would mis-scope d13 later).
        access_modifier: r.access_modifier.clone(),
        return_type: r.return_type.clone(),
        call_sites: Vec::new(),
        operation_sites: Vec::new(),
        statement_tree: None,
        loops: Vec::new(),
        // Dep routines are bodyless (ABI symbol-only) — no body anchor / refs /
        // unreachable statements. `body_available` is hardcoded false for projected
        // dep routines (deps/projection.rs), and the L5 detectors that read these
        // (d19/d20/d29) gate on `body_available` or `kind`, so the defaults are never
        // observed. If dep bodies ever become available, revisit these defaults
        // (empty identifier_references would otherwise read as d19 false-positives).
        source_anchor: crate::engine::l2::features::PAnchor {
            source_unit_id: String::new(),
            start_line: 0,
            start_column: 0,
            end_line: 0,
            end_column: 0,
            syntax_kind: String::new(),
        },
        identifier_references: Vec::new(),
        unreachable_statements: Vec::new(),
        // Dep routines are bodyless (ABI symbol-only) — no branching / assignments /
        // condition refs. d43 gates on the publisher carrying an IsHandled `var` param +
        // a primary role, so these defaults are never observed for a dep routine.
        has_branching: false,
        var_assignments: Vec::new(),
        condition_references: Vec::new(),
        // Dep routines are ABI symbol-only (no AST parent wrapper) — the enclosing-member
        // capture (E1) is a native-parser-only signal, so these are always `None` for a
        // projected dep routine. Additive: `L3Routine` is not `Serialize`-derived.
        enclosing_member: None,
        originating_object: None,
        enclosing_member_range: None,
        entry_temp_guard_receiver: None,
    }
}

/// Append the dep entities (objects/tables/routines) onto an already-assembled
/// native workspace, in al-sem's `withDependencyArtifacts` order: workspace FIRST
/// (already present), deps LAST. The routine→object_type lookup is built from the
/// dep objects so each dep routine carries its owning object's type.
fn append_dep_entities(
    workspace: &mut L3Workspace,
    objects: &[ProjectedObject],
    tables: &[ProjectedTable],
    routines: &[ProjectedRoutine],
) {
    use std::collections::HashMap;
    let object_type_by_id: HashMap<&str, &str> = objects
        .iter()
        .map(|o| (o.id.as_str(), o.object_type.as_str()))
        .collect();

    for o in objects {
        workspace.objects.push(dep_object_to_l3(o));
    }
    for t in tables {
        workspace.tables.push(dep_table_to_l3(t));
    }
    for r in routines {
        let object_type = object_type_by_id
            .get(r.object_id.as_str())
            .copied()
            .unwrap_or("");
        workspace.routines.push(dep_routine_to_l3(r, object_type));
    }
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
        },
        declared_dep_app_guids: declared_dep_app_guids.to_vec(),
        fetched_app_guids,
        apps,
        dep_app_versions,
    })
}

/// One declared workspace dependency `{appGuid, name, minVersion}` — the d17-relevant
/// subset of al-sem `ManifestDependency` (`parseWorkspaceDependencies`, explicit
/// `dependencies[]` only; missing `version` defaults to `"0.0.0.0"`). Read from the
/// workspace app.json. ADDITIVE — only the d17 plumbing reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeclaredDependencyDecl {
    pub app_guid: String,
    pub name: String,
    pub min_version: String,
}

/// Well-known Microsoft Application-tier app identities. BC apps never list these in
/// `dependencies[]` — they are implicit, declared via the `application` field in
/// app.json. Mirrors al-sem `MS_APPLICATION_TIER` in `workspace-dependencies.ts`
/// (same GUIDs, names, publisher, ORDER).
///
/// `pub(crate)` (beyond-1B.3b Task 5.5): this is the single source of truth for
/// the tier data — `crate::dependencies::append_implicit_ms_tier_deps` reuses it
/// (pure DATA, like `program::resolve::builtins::global_builtins`) to wire the
/// SAME implicit deps into the `src/program` topology closure, not just this
/// isolated `engine::l4` subsystem.
pub(crate) const MS_APPLICATION_TIER: &[(&str, &str)] = &[
    ("c1335042-3002-4257-bf8a-75c898ccb1b8", "Application"),
    ("437dbf0e-84ff-417a-965d-ed2bb9650972", "Base Application"),
    (
        "f3552374-a1f2-4356-848e-196002525837",
        "Business Foundation",
    ),
];

/// Well-known Microsoft Platform-tier app identities. BC apps never list these in
/// `dependencies[]` — they are implicit, declared via the `platform` field in
/// app.json. Mirrors al-sem `MS_PLATFORM_TIER` in `workspace-dependencies.ts`
/// (same GUIDs, names, publisher, ORDER).
///
/// `pub(crate)` (beyond-1B.3b Task 5.5) — see [`MS_APPLICATION_TIER`] doc.
pub(crate) const MS_PLATFORM_TIER: &[(&str, &str)] = &[
    ("63ca2fa4-4f03-4f2b-a480-172fef340d3f", "System Application"),
    ("8874ed3a-0643-4247-9ced-7a7002f7135d", "System"),
];

/// Read the workspace root `app.json` `dependencies[]` into `{appGuid, name,
/// minVersion}` rows, including IMPLICIT Microsoft Application-/Platform-tier deps
/// derived from the `application` / `platform` string fields. Returns `[]` on any
/// read/parse miss (fail-closed).
///
/// Mirrors al-sem `parseWorkspaceDependencies` (workspace-dependencies.ts) EXACTLY:
///   result = [...explicit deps..., ...MS_APPLICATION_TIER (if application set),
///              ...MS_PLATFORM_TIER (if platform set)]
/// The implicit entries use the `application`/`platform` version string as
/// `minVersion` and the well-known GUIDs/names from the tier constants above.
pub fn read_workspace_declared_dependencies(workspace: &Path) -> Vec<DeclaredDependencyDecl> {
    let Ok(text) = std::fs::read_to_string(workspace.join("app.json")) else {
        return Vec::new();
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
        return Vec::new();
    };

    // --- explicit dependencies[] ---
    let mut result: Vec<DeclaredDependencyDecl> = value
        .get("dependencies")
        .and_then(|d| d.as_array())
        .map(|deps| {
            deps.iter()
                .filter_map(|d| {
                    let app_guid = d
                        .get("id")
                        .or_else(|| d.get("appId"))
                        .and_then(|v| v.as_str())
                        .filter(|s| !s.is_empty())?
                        .to_string();
                    let name = d
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let min_version = d
                        .get("version")
                        .and_then(|v| v.as_str())
                        .unwrap_or("0.0.0.0")
                        .to_string();
                    Some(DeclaredDependencyDecl {
                        app_guid,
                        name,
                        min_version,
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    // --- implicit Microsoft Application-tier deps (app.json `application` field) ---
    // al-sem: `if (typeof parsed.application === "string" && parsed.application !== "")`
    if let Some(app_ver) = value
        .get("application")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
    {
        for (guid, name) in MS_APPLICATION_TIER {
            result.push(DeclaredDependencyDecl {
                app_guid: guid.to_string(),
                name: name.to_string(),
                min_version: app_ver.to_string(),
            });
        }
    }

    // --- implicit Microsoft Platform-tier deps (app.json `platform` field) ---
    // al-sem: `if (typeof parsed.platform === "string" && parsed.platform !== "")`
    if let Some(plat_ver) = value
        .get("platform")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
    {
        for (guid, name) in MS_PLATFORM_TIER {
            result.push(DeclaredDependencyDecl {
                app_guid: guid.to_string(),
                name: name.to_string(),
                min_version: plat_ver.to_string(),
            });
        }
    }

    result
}

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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Helper: write an `app.json` to a temp dir and return the dir path.
    fn write_app_json(content: &str) -> tempfile::TempDir {
        let dir = tempfile::TempDir::new().expect("tmp dir");
        fs::write(dir.path().join("app.json"), content).expect("write app.json");
        dir
    }

    // -----------------------------------------------------------------------
    // Oracle 1 — implicit MS dep synthesis (Fix 1)
    // -----------------------------------------------------------------------

    /// An app.json with explicit deps only (no `application`/`platform`) → no implicit deps.
    #[test]
    fn no_implicit_deps_when_no_application_or_platform() {
        let dir = write_app_json(
            r#"{
            "id": "aaaaaaaa-0000-0000-0000-000000000001",
            "name": "Test",
            "publisher": "PT",
            "version": "1.0.0.0",
            "dependencies": [
                {"id": "bbbbbbbb-0000-0000-0000-000000000002", "name": "Dep", "publisher": "P", "version": "2.0.0.0"}
            ]
        }"#,
        );
        let deps = read_workspace_declared_dependencies(dir.path());
        assert_eq!(deps.len(), 1, "only the explicit dep");
        assert_eq!(deps[0].app_guid, "bbbbbbbb-0000-0000-0000-000000000002");
    }

    /// An app.json with `application` → 3 MS Application-tier implicit deps appended after
    /// the explicit ones, in MS_APPLICATION_TIER order.
    #[test]
    fn implicit_application_tier_deps_appended_in_order() {
        let dir = write_app_json(
            r#"{
            "id": "aaaaaaaa-0000-0000-0000-000000000001",
            "name": "Test",
            "publisher": "PT",
            "version": "1.0.0.0",
            "application": "25.1.0.0"
        }"#,
        );
        let deps = read_workspace_declared_dependencies(dir.path());
        // No explicit deps → only the 3 Application-tier entries.
        assert_eq!(deps.len(), 3, "3 MS Application-tier implicit deps");
        // Verify GUIDs and minVersion match al-sem MS_APPLICATION_TIER exactly.
        assert_eq!(deps[0].app_guid, "c1335042-3002-4257-bf8a-75c898ccb1b8");
        assert_eq!(deps[0].name, "Application");
        assert_eq!(deps[0].min_version, "25.1.0.0");
        assert_eq!(deps[1].app_guid, "437dbf0e-84ff-417a-965d-ed2bb9650972");
        assert_eq!(deps[1].name, "Base Application");
        assert_eq!(deps[2].app_guid, "f3552374-a1f2-4356-848e-196002525837");
        assert_eq!(deps[2].name, "Business Foundation");
    }

    /// An app.json with `platform` → 2 MS Platform-tier implicit deps appended after the
    /// explicit ones, in MS_PLATFORM_TIER order.
    #[test]
    fn implicit_platform_tier_deps_appended_in_order() {
        let dir = write_app_json(
            r#"{
            "id": "aaaaaaaa-0000-0000-0000-000000000001",
            "name": "Test",
            "publisher": "PT",
            "version": "1.0.0.0",
            "platform": "25.0.0.0"
        }"#,
        );
        let deps = read_workspace_declared_dependencies(dir.path());
        assert_eq!(deps.len(), 2, "2 MS Platform-tier implicit deps");
        assert_eq!(deps[0].app_guid, "63ca2fa4-4f03-4f2b-a480-172fef340d3f");
        assert_eq!(deps[0].name, "System Application");
        assert_eq!(deps[0].min_version, "25.0.0.0");
        assert_eq!(deps[1].app_guid, "8874ed3a-0643-4247-9ced-7a7002f7135d");
        assert_eq!(deps[1].name, "System");
    }

    /// Full scenario: explicit dep + application + platform → explicit first, then
    /// application tier (3), then platform tier (2). Mirrors al-sem result order.
    #[test]
    fn explicit_then_application_then_platform_order() {
        let dir = write_app_json(
            r#"{
            "id": "aaaaaaaa-0000-0000-0000-000000000001",
            "name": "Test",
            "publisher": "PT",
            "version": "1.0.0.0",
            "dependencies": [
                {"id": "eeeeeeee-0000-0000-0000-000000000001", "name": "MyDep", "publisher": "P", "version": "3.0.0.0"}
            ],
            "application": "24.0.0.0",
            "platform": "24.0.0.0"
        }"#,
        );
        let deps = read_workspace_declared_dependencies(dir.path());
        // 1 explicit + 3 application + 2 platform = 6
        assert_eq!(deps.len(), 6, "1 explicit + 3 app-tier + 2 plat-tier = 6");
        assert_eq!(
            deps[0].app_guid, "eeeeeeee-0000-0000-0000-000000000001",
            "explicit first"
        );
        assert_eq!(
            deps[1].app_guid, "c1335042-3002-4257-bf8a-75c898ccb1b8",
            "Application"
        );
        assert_eq!(
            deps[2].app_guid, "437dbf0e-84ff-417a-965d-ed2bb9650972",
            "Base Application"
        );
        assert_eq!(
            deps[3].app_guid, "f3552374-a1f2-4356-848e-196002525837",
            "Business Foundation"
        );
        assert_eq!(
            deps[4].app_guid, "63ca2fa4-4f03-4f2b-a480-172fef340d3f",
            "System Application"
        );
        assert_eq!(
            deps[5].app_guid, "8874ed3a-0643-4247-9ced-7a7002f7135d",
            "System"
        );
    }

    /// Empty `application` string → no implicit application-tier deps (al-sem: `!== ""`).
    #[test]
    fn empty_application_string_produces_no_implicit_deps() {
        let dir = write_app_json(
            r#"{
            "id": "aaaaaaaa-0000-0000-0000-000000000001",
            "name": "Test",
            "publisher": "PT",
            "version": "1.0.0.0",
            "application": ""
        }"#,
        );
        let deps = read_workspace_declared_dependencies(dir.path());
        assert!(
            deps.is_empty(),
            "empty application string → no implicit deps"
        );
    }

    // -----------------------------------------------------------------------
    // Oracle 2 — role-scope filter: dep-anchored routines are filtered from
    // the edge walk (dep_routine_ids.contains(&e.from) → skip).
    // This is tested indirectly via detect_d17 in d17.rs; the cross_app_l3
    // test confirms the declared-deps list shape only.
    // -----------------------------------------------------------------------
}

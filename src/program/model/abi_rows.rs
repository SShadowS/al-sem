//! Dependency model rows from a symbol-only dependency's ABI (engine-switch S7.2,
//! moved from `engine::deps::cross_app_l3`): the `ProjectedObject`/`ProjectedTable`/
//! `ProjectedRoutine` of `engine::deps::projection::project_abi_to_index` turned into
//! the detector model's `ModelObject`/`ModelTable`/`ModelRoutine`. A dependency routine known
//! only from symbols is bodyless and carries no features; its record-typed
//! parameters become record variables so the temp-state rules see them.

use crate::engine::deps::projection::{ProjectedObject, ProjectedRoutine, ProjectedTable};
use crate::program::body::scope::{ts_known, ts_param_dependent};
use crate::program::model::workspace::{
    ModelEntities, ModelField, ModelObject, ModelPageControl, ModelParameter, ModelRoutine,
    ModelTable, PageControlKind, RoutineVariables,
};

/// Convert one projected dep object into the L3 object shape (identical to the
/// native source path's `ModelObject`). The dep object carries the same identity
/// (StableObjectId-independent internal id) the native path mints.
pub(crate) fn dep_object_to_l3(o: &ProjectedObject) -> ModelObject {
    ModelObject {
        id: o.id.clone(),
        app_guid: o.app_guid.clone(),
        object_type: o.object_type.clone(),
        object_number: o.object_number,
        name: o.name.clone(),
        source_table_name: o.source_table_name.clone(),
        extends_target_name: o.extends_target_name.clone(),
        implements_interfaces: o.implements_interfaces.clone(),
        // The ABI projection DOES carry `object_subtype` (projection.rs:116) —
        // forward it so native + ABI agree on the ModelObject shape (d46 reads it).
        object_subtype: o.object_subtype.clone(),
        // The ABI projection DOES carry `page_type` (projection.rs) — forward it so
        // native + ABI agree on the ModelObject shape and a cross-app `PageType=API`
        // dependency page classifies as `api-page` (mirrors the `object_subtype`
        // forward above and al-sem dependency-projection.ts).
        page_type: o.page_type.clone(),
        // The ABI projection DOES carry `inherent_commit_behavior` (projection.rs:121,
        // symbol_reference.rs:99) in canonical lower-case form — forward it so native
        // + ABI agree on the ModelObject shape. Consumed by return_summary to merge
        // object-level commit behavior into each dep routine's commitBehavior.
        inherent_commit_behavior: o.inherent_commit_behavior.clone(),
        source_table_temporary: None,
        page_controls: o
            .page_controls
            .iter()
            .map(|(n, k, t)| ModelPageControl {
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
fn dep_field_to_l3(f: &crate::engine::deps::projection::ProjectedField) -> ModelField {
    ModelField {
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
pub(crate) fn dep_table_to_l3(t: &ProjectedTable) -> ModelTable {
    ModelTable {
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
pub(crate) fn dep_routine_to_l3(r: &ProjectedRoutine, object_type: &str) -> ModelRoutine {
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
    let record_variables: Vec<crate::program::model::workspace::ModelRecordVariable> = r
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
            crate::program::model::workspace::ModelRecordVariable {
                id: format!("{}/rv/{}", r.id, p.name.to_lowercase()),
                name: p.name.clone(),
                table_name: crate::program::model::record_types::record_table_name_of(&p.type_text),
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
        .map(|p| ModelParameter {
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

    ModelRoutine {
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
        // routine's `accessModifier`. Forward it (byte-invariant today: ModelRoutine.access_modifier
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
        source_anchor: crate::program::body::features::PAnchor {
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
        // projected dep routine. Additive: `ModelRoutine` is not `Serialize`-derived.
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
pub(crate) fn append_dep_entities(
    workspace: &mut ModelEntities,
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

//! R2.5a MERGED-INDEX emitter — the Rust analogue of al-sem's
//! `scripts/r2.5a-merged-projection.ts` over the dependency entities.
//!
//! Reproduces, from one or more `.app`s, the DEPENDENCY-ENTITY subset of the
//! merged index in the SAME stable JSON shape (and exact key order) as the al-sem
//! Task 2 goldens (`scripts/r2.5a-goldens/<fixture>.r2.5a.golden.json`):
//!
//! ```text
//! .app(s)
//!   → (per .app) strip_app_header → NavxManifest.xml + SymbolReference.json
//!               → parse_app_manifest_xml + parse_symbol_reference
//!               → project_abi_to_index (manifest appGuid)
//!   → MERGE: concatenate every .app's projected objects/tables/routines
//!            (mirrors withDependencyArtifacts append + role-stamp)
//!   → merge_extension_fields_projected: append each dep TableExtension's fields
//!            INTO the base table (the L3 mergeExtensionFields capture-point
//!            invariant — Task 2's goldens were captured POST-resolveModel)
//!   → emit: objects/tables/routines/apps, TOP-LEVEL collections sorted by stable
//!            id; nested arrays keep SymbolReference order; optional props OMITTED
//!            when absent — byte-identical to the TS `JSON.stringify(_, null, 2)`.
//! ```
//!
//! ## The capture-point subtlety (critical)
//!
//! Task 2's golden was captured POST-`resolveModel`, which runs L3's
//! `mergeExtensionFields`: a dep `TableExtension`'s fields are physically merged
//! INTO the base table (rekeyed to the base table id, provenance kept on the
//! extension). So in the golden a base table carries the extension's field, AND
//! the extension's own table still retains it under a different StableFieldId (no
//! double-count). This emitter reproduces that merge — without it the table
//! goldens diverge.
//!
//! ## Scope (R2.5a only)
//!
//! Symbol-only: NO cross-app call/event/coverage resolution (that is R2.5b). The
//! ONLY L3 step reproduced is the extension-field merge, because it is the only
//! `resolveModel` step that mutates an IDENTITY-relevant projected field of the
//! dependency entities. The call/event/coverage builders touch graphs/coverage,
//! none of the projected identity fields — so they are correctly skipped.

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::engine::deps::app_manifest::parse_app_manifest_xml;
use crate::engine::deps::app_package_zip::{
    extract_navx_manifest_xml, extract_symbol_reference_json,
};
use crate::engine::deps::projection::{
    ProjectedField, ProjectedObject, ProjectedRoutine, ProjectedTable, project_abi_to_index,
};
use crate::engine::deps::symbol_reference::parse_symbol_reference;
use crate::engine::ids::{encode_field_id, encode_table_id, to_stable_field_id};
use crate::program::attributes::{AttributeArg, AttributeInfo};

// ===========================================================================
// Serializable projection — EXACT golden key order (mirrors the TS projectors).
// ===========================================================================

/// One dependency app identity row. Key order: appGuid, publisher, name, version,
/// sourceKind (matches `ProjectedApp` in r2.5a-merged-projection.ts).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct EmittedApp {
    #[serde(rename = "appGuid")]
    pub app_guid: String,
    pub publisher: String,
    pub name: String,
    pub version: String,
    /// `includesSource ? "app-source" : "symbol-only"`.
    #[serde(rename = "sourceKind")]
    pub source_kind: String,
}

/// One dependency object. Key order: stableObjectId, objectType, objectNumber,
/// name, sourceHash, then optional objectSubtype, pageType, sourceTableName,
/// extendsTargetName, implementsInterfaces, inherentCommitBehavior.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct EmittedObject {
    #[serde(rename = "stableObjectId")]
    pub stable_object_id: String,
    #[serde(rename = "objectType")]
    pub object_type: String,
    #[serde(rename = "objectNumber")]
    pub object_number: i64,
    pub name: String,
    #[serde(rename = "sourceHash")]
    pub source_hash: String,
    #[serde(rename = "objectSubtype", skip_serializing_if = "Option::is_none")]
    pub object_subtype: Option<String>,
    #[serde(rename = "pageType", skip_serializing_if = "Option::is_none")]
    pub page_type: Option<String>,
    #[serde(rename = "sourceTableName", skip_serializing_if = "Option::is_none")]
    pub source_table_name: Option<String>,
    #[serde(rename = "extendsTargetName", skip_serializing_if = "Option::is_none")]
    pub extends_target_name: Option<String>,
    #[serde(
        rename = "implementsInterfaces",
        skip_serializing_if = "Option::is_none"
    )]
    pub implements_interfaces: Option<Vec<String>>,
    #[serde(
        rename = "inherentCommitBehavior",
        skip_serializing_if = "Option::is_none"
    )]
    pub inherent_commit_behavior: Option<String>,
}

/// One table field. Key order: stableFieldId, fieldNumber, name, fieldClass,
/// dataType, isBlobLike.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct EmittedField {
    #[serde(rename = "stableFieldId")]
    pub stable_field_id: String,
    #[serde(rename = "fieldNumber")]
    pub field_number: i64,
    pub name: String,
    #[serde(rename = "fieldClass")]
    pub field_class: String,
    #[serde(rename = "dataType")]
    pub data_type: String,
    #[serde(rename = "isBlobLike")]
    pub is_blob_like: bool,
}

/// One table key. Key order: stableKeyId, fields (resolved field-id list).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct EmittedKey {
    #[serde(rename = "stableKeyId")]
    pub stable_key_id: String,
    pub fields: Vec<String>,
}

/// One table. Key order: stableTableId, tableNumber, name, fields, keys.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct EmittedTable {
    #[serde(rename = "stableTableId")]
    pub stable_table_id: String,
    #[serde(rename = "tableNumber")]
    pub table_number: i64,
    pub name: String,
    pub fields: Vec<EmittedField>,
    pub keys: Vec<EmittedKey>,
}

/// One routine parameter. Key order: name, typeText, isVar, isRecord.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct EmittedParameter {
    pub name: String,
    #[serde(rename = "typeText")]
    pub type_text: String,
    #[serde(rename = "isVar")]
    pub is_var: bool,
    #[serde(rename = "isRecord")]
    pub is_record: bool,
}

/// One routine. Key order: stableRoutineId, signatureFingerprint, kind, name,
/// parameters, attributesParsed, bodyAvailable, analysisRole, then optional
/// returnType, accessModifier (APPENDED last — matching the TS projector).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct EmittedRoutine {
    #[serde(rename = "stableRoutineId")]
    pub stable_routine_id: String,
    #[serde(rename = "signatureFingerprint")]
    pub signature_fingerprint: String,
    pub kind: String,
    pub name: String,
    pub parameters: Vec<EmittedParameter>,
    #[serde(rename = "attributesParsed")]
    pub attributes_parsed: Vec<EmittedAttribute>,
    #[serde(rename = "bodyAvailable")]
    pub body_available: bool,
    #[serde(rename = "analysisRole")]
    pub analysis_role: String,
    #[serde(rename = "returnType", skip_serializing_if = "Option::is_none")]
    pub return_type: Option<String>,
    #[serde(rename = "accessModifier", skip_serializing_if = "Option::is_none")]
    pub access_modifier: Option<String>,
}

/// One attribute. Key order: name, args. (The internal `AttributeInfo` carries a
/// `raw` field that the golden surface OMITS — we re-emit only name + args.)
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct EmittedAttribute {
    pub name: String,
    pub args: Vec<EmittedAttributeArg>,
}

/// One attribute arg. Key order: kind, text, then optional value, qualifier,
/// member.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct EmittedAttributeArg {
    pub kind: String,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub qualifier: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub member: Option<String>,
}

/// The top-level merged projection. Key order: apps, objects, tables, routines.
#[derive(Debug, Clone, Serialize, Default, PartialEq, Eq)]
pub struct MergedIndexProjection {
    pub apps: Vec<EmittedApp>,
    pub objects: Vec<EmittedObject>,
    pub tables: Vec<EmittedTable>,
    pub routines: Vec<EmittedRoutine>,
}

// ===========================================================================
// One dependency `.app` → its projected entities + identity + includesSource.
// ===========================================================================

/// The intermediate merge state for a single `.app`: the projection plus the
/// manifest-derived app identity needed to emit the App row. Public so the R2.5b
/// cross-app L3 wiring can reuse the exact `.app` → projected-entity parse.
pub struct DepAppParse {
    pub app_guid: String,
    pub name: String,
    pub publisher: String,
    pub version: String,
    pub includes_source: bool,
    pub objects: Vec<ProjectedObject>,
    pub tables: Vec<ProjectedTable>,
    pub routines: Vec<ProjectedRoutine>,
}

/// Public wrapper over [`parse_dep_app`] for the R2.5b cross-app L3 wiring — reads +
/// parses + projects a single `.app` from raw bytes (fail-closed: `None` on an
/// unreadable archive / missing manifest `<App>` Id). Reuses the IDENTICAL parse
/// the R2.5a merged-index emitter uses, so the dep entities carry the same identity.
pub fn parse_dep_app_public(app_bytes: &[u8], model_instance_id: &str) -> Option<DepAppParse> {
    parse_dep_app(app_bytes, model_instance_id)
}

/// Read + parse + project a single `.app` from raw bytes. Returns `None` when the
/// archive is unreadable / lacks a usable manifest `<App>` Id (fail-closed: never
/// panics; a bad `.app` contributes no entities, matching al-sem's resolver which
/// emits a diagnostic and drops the artifact).
fn parse_dep_app(app_bytes: &[u8], model_instance_id: &str) -> Option<DepAppParse> {
    let manifest_xml = extract_navx_manifest_xml(app_bytes)?;
    let manifest = parse_app_manifest_xml(&manifest_xml);
    if manifest.error.is_some() || manifest.identity.app_guid.is_empty() {
        return None;
    }
    let app_guid = manifest.identity.app_guid.clone();

    let sym_json = extract_symbol_reference_json(app_bytes)?;
    let abi = parse_symbol_reference(&sym_json);
    let projected = project_abi_to_index(&abi, &app_guid, model_instance_id);

    Some(DepAppParse {
        app_guid,
        name: manifest.identity.name,
        publisher: manifest.identity.publisher,
        version: manifest.identity.version,
        includes_source: manifest.includes_source,
        objects: projected.objects,
        tables: projected.tables,
        routines: projected.routines,
    })
}

// ===========================================================================
// `.app` discovery from a path (single .app OR a workspace/dir of .app(s)).
// ===========================================================================

/// Collect `.app` file paths under `path`. If `path` IS an `.app`, return just it.
/// If it is a directory, recurse and collect every `*.app` (case-insensitive
/// extension), SORTED by path so the merge/emit order is deterministic regardless
/// of filesystem iteration order. Skips unreadable entries (never panics).
pub fn collect_app_paths(path: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if path.is_file() {
        if has_app_ext(path) {
            out.push(path.to_path_buf());
        }
        return out;
    }
    if path.is_dir() {
        collect_app_paths_rec(path, &mut out);
    }
    out.sort();
    out
}

fn has_app_ext(p: &Path) -> bool {
    p.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("app"))
        .unwrap_or(false)
}

fn collect_app_paths_rec(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_dir() {
            collect_app_paths_rec(&p, out);
        } else if has_app_ext(&p) {
            out.push(p);
        }
    }
}

// ===========================================================================
// Top-level emit.
// ===========================================================================

/// Build the R2.5a merged-index dependency-entity projection from a path that is
/// either a single `.app` or a directory containing `.app`(s) (e.g. an
/// `.alpackages/`). `model_instance_id` is irrelevant to the emitted surface
/// (StableObjectId/StableRoutineId are modelInstanceId-independent — R0), but is
/// threaded through for the internal routine id used by the projection. Never
/// panics; unreadable `.app`s contribute nothing.
pub fn build_merged_index_from_path(path: &Path, model_instance_id: &str) -> MergedIndexProjection {
    let app_paths = collect_app_paths(path);
    let mut parses: Vec<DepAppParse> = Vec::new();
    for ap in &app_paths {
        if let Ok(bytes) = std::fs::read(ap)
            && let Some(parsed) = parse_dep_app(&bytes, model_instance_id)
        {
            parses.push(parsed);
        }
    }
    build_merged_index(parses)
}

/// Build the projection from already-parsed dep apps (the merge core). Exposed for
/// the oracle, which drives parsed apps directly.
fn build_merged_index(parses: Vec<DepAppParse>) -> MergedIndexProjection {
    // MERGE: append every app's entities (mirrors withDependencyArtifacts; the
    // role-stamp is already "dependency" from the projection).
    let mut apps: Vec<EmittedApp> = Vec::new();
    let mut all_objects: Vec<ProjectedObject> = Vec::new();
    let mut all_tables: Vec<ProjectedTable> = Vec::new();
    let mut all_routines: Vec<ProjectedRoutine> = Vec::new();

    for p in parses {
        apps.push(EmittedApp {
            app_guid: p.app_guid.clone(),
            publisher: p.publisher,
            name: p.name,
            version: p.version,
            source_kind: if p.includes_source {
                "app-source".to_string()
            } else {
                "symbol-only".to_string()
            },
        });
        all_objects.extend(p.objects);
        all_tables.extend(p.tables);
        all_routines.extend(p.routines);
    }

    // CAPTURE-POINT: run the extension-field merge over the merged tables (L3
    // mergeExtensionFields). The objects carry extends_target_name; resolve the
    // base table by name and append the extension's fields.
    merge_extension_fields_projected(&all_objects, &mut all_tables);

    // EMIT: build the serializable surface, TOP-LEVEL collections sorted by stable
    // id; nested arrays keep their SymbolReference order from the projection.
    let mut emitted_apps = apps;
    emitted_apps.sort_by(|a, b| cmp_stable(&a.app_guid, &b.app_guid));

    let mut emitted_objects: Vec<EmittedObject> = all_objects.iter().map(emit_object).collect();
    emitted_objects.sort_by(|a, b| cmp_stable(&a.stable_object_id, &b.stable_object_id));

    let mut emitted_tables: Vec<EmittedTable> = all_tables.iter().map(emit_table).collect();
    emitted_tables.sort_by(|a, b| cmp_stable(&a.stable_table_id, &b.stable_table_id));

    let mut emitted_routines: Vec<EmittedRoutine> = all_routines.iter().map(emit_routine).collect();
    emitted_routines.sort_by(|a, b| cmp_stable(&a.stable_routine_id, &b.stable_routine_id));

    MergedIndexProjection {
        apps: emitted_apps,
        objects: emitted_objects,
        tables: emitted_tables,
        routines: emitted_routines,
    }
}

/// Byte-order string compare — reproduces the TS `cmpStable` (`a < b ? -1 : …`).
fn cmp_stable(a: &str, b: &str) -> std::cmp::Ordering {
    a.cmp(b)
}

// ===========================================================================
// The extension-field merge over PROJECTED tables (capture-point invariant).
//
// TWIN of `crate::engine::l3::extension_fields::merge_extension_fields` (and the
// al-sem original `src/resolve/extension-fields.ts` `mergeExtensionFields`). This
// is the SAME algorithm specialized to the projected entity shape — the three
// copies MUST stay in lockstep: change one, change all (no extra guards / no
// behavioral drift). If you touch the resolution/dedup semantics here, mirror them
// in `l3/extension_fields.rs` and vice-versa.
//
// Walk objects for TableExtensions IN ASSEMBLED ORDER, resolve the base table by
// the extension's extends-target name (case-insensitive, LAST-wins), find the
// extension's own table by encoded id (LAST-wins), and append each not-yet-present
// field (FIRST-wins on fieldNumber, via the dedup `existing` set seeded from the
// base table) onto the base table — physically rekeyed (id / physicalTableId /
// stableFieldId → base table) but provenance (declaringObjectId / declaringAppId)
// kept on the extension. Recomputes the merged field's StableFieldId from the BASE
// table so it matches the golden (base table number, extension field number).
// ===========================================================================

fn merge_extension_fields_projected(objects: &[ProjectedObject], tables: &mut [ProjectedTable]) {
    for object in objects {
        if object.object_type != "TableExtension" {
            continue;
        }
        let Some(extends_target) = &object.extends_target_name else {
            continue;
        };
        let Some(base_idx) = table_index_by_name(tables, extends_target) else {
            continue;
        };
        let extension_table_id = encode_table_id(&object.app_guid, object.object_number);
        let Some(ext_idx) = table_index_by_id(tables, &extension_table_id) else {
            continue;
        };

        let base_table_id = tables[base_idx].id.clone();
        let base_app_guid = tables[base_idx].app_guid.clone();
        let base_table_number = tables[base_idx].table_number;

        let mut existing: std::collections::HashSet<i64> = tables[base_idx]
            .fields
            .iter()
            .map(|f| f.field_number)
            .collect();

        let ext_fields = tables[ext_idx].fields.clone();
        for field in ext_fields {
            if existing.contains(&field.field_number) {
                continue; // FIRST-wins on duplicate field number.
            }
            let merged = ProjectedField {
                id: encode_field_id(&base_table_id, field.field_number),
                // StableFieldId derives from the BASE table (physical relocation).
                stable_field_id: to_stable_field_id(
                    &base_app_guid,
                    base_table_number,
                    field.field_number,
                ),
                physical_table_id: base_table_id.clone(),
                // Provenance stays the extension's.
                declaring_object_id: object.id.clone(),
                declaring_app_id: object.app_guid.clone(),
                field_number: field.field_number,
                name: field.name.clone(),
                field_class: field.field_class.clone(),
                data_type: field.data_type.clone(),
                is_blob_like: field.is_blob_like,
            };
            tables[base_idx].fields.push(merged);
            existing.insert(field.field_number);
        }
    }
}

/// `tableByName`: case-insensitive, LAST-wins on collision. TWIN of
/// `l3::extension_fields::table_index_by_name` — keep in lockstep.
fn table_index_by_name(tables: &[ProjectedTable], name: &str) -> Option<usize> {
    let want = name.to_lowercase();
    let mut found = None;
    for (i, t) in tables.iter().enumerate() {
        if t.name.to_lowercase() == want {
            found = Some(i);
        }
    }
    found
}

/// `tableById`: LAST-wins on collision. TWIN of
/// `l3::extension_fields::table_index_by_id` — keep in lockstep.
fn table_index_by_id(tables: &[ProjectedTable], id: &str) -> Option<usize> {
    let mut found = None;
    for (i, t) in tables.iter().enumerate() {
        if t.id == id {
            found = Some(i);
        }
    }
    found
}

// ===========================================================================
// Per-entity emit (ProjectedX → EmittedX) — drops internal-only fields.
// ===========================================================================

fn emit_object(o: &ProjectedObject) -> EmittedObject {
    EmittedObject {
        stable_object_id: o.stable_object_id.clone(),
        object_type: o.object_type.clone(),
        object_number: o.object_number,
        name: o.name.clone(),
        source_hash: o.source_hash.clone(),
        object_subtype: o.object_subtype.clone(),
        page_type: o.page_type.clone(),
        source_table_name: o.source_table_name.clone(),
        extends_target_name: o.extends_target_name.clone(),
        implements_interfaces: o.implements_interfaces.clone(),
        inherent_commit_behavior: o.inherent_commit_behavior.clone(),
    }
}

fn emit_table(t: &ProjectedTable) -> EmittedTable {
    EmittedTable {
        stable_table_id: t.stable_table_id.clone(),
        table_number: t.table_number,
        name: t.name.clone(),
        fields: t.fields.iter().map(emit_field).collect(),
        keys: t
            .keys
            .iter()
            .map(|k| EmittedKey {
                stable_key_id: stable_key_id(&k.id, &t.stable_table_id),
                fields: k
                    .fields
                    .iter()
                    .map(|fid| internal_field_id_to_stable(fid))
                    .collect(),
            })
            .collect(),
    }
}

fn emit_field(f: &ProjectedField) -> EmittedField {
    EmittedField {
        stable_field_id: f.stable_field_id.clone(),
        field_number: f.field_number,
        name: f.name.clone(),
        field_class: f.field_class.clone(),
        data_type: f.data_type.clone(),
        is_blob_like: f.is_blob_like,
    }
}

/// Resolve an internal field id (`{appGuid}/table/{n}/{field}`) to its stable form
/// (`{appGuid}:Table:{n}#{field}`). The projection stores key field references as
/// internal ids; the golden emits StableFieldIds. Reproduces al-sem's
/// `cvt.toStableFieldId(fid)`.
fn internal_field_id_to_stable(internal: &str) -> String {
    // internal: "{appGuid}/table/{tableNumber}/{fieldNumber}"
    let marker = "/table/";
    if let Some(at) = internal.find(marker) {
        let app_guid = &internal[..at];
        let rest = &internal[at + marker.len()..];
        if let Some(slash) = rest.find('/') {
            let table_number = &rest[..slash];
            let field_number = &rest[slash + 1..];
            return format!("{app_guid}:Table:{table_number}#{field_number}");
        }
    }
    internal.to_string() // unexpected shape — pass through (visible divergence)
}

/// Stable KeyId form: `{stableTableId}#Key:{index}` from the internal
/// `{tableId}/key/{index}`. Reproduces al-sem `stableKeyId`.
fn stable_key_id(internal: &str, stable_table_id: &str) -> String {
    let marker = "/key/";
    if let Some(at) = internal.find(marker)
        && at > 0
    {
        let index = &internal[at + marker.len()..];
        return format!("{stable_table_id}#Key:{index}");
    }
    internal.to_string()
}

fn emit_routine(r: &ProjectedRoutine) -> EmittedRoutine {
    EmittedRoutine {
        stable_routine_id: r.stable_routine_id.clone(),
        signature_fingerprint: r.signature_fingerprint.clone(),
        kind: r.kind.clone(),
        name: r.name.clone(),
        parameters: r
            .parameters
            .iter()
            .map(|p| EmittedParameter {
                name: p.name.clone(),
                type_text: p.type_text.clone(),
                is_var: p.is_var,
                is_record: p.is_record,
            })
            .collect(),
        attributes_parsed: r.attributes_parsed.iter().map(emit_attribute).collect(),
        body_available: r.body_available,
        analysis_role: r.analysis_role.clone(),
        return_type: r.return_type.clone(),
        access_modifier: r.access_modifier.clone(),
    }
}

fn emit_attribute(a: &AttributeInfo) -> EmittedAttribute {
    EmittedAttribute {
        name: a.name.clone(),
        args: a.args.iter().map(emit_attribute_arg).collect(),
    }
}

fn emit_attribute_arg(a: &AttributeArg) -> EmittedAttributeArg {
    EmittedAttributeArg {
        kind: a.kind.clone(),
        text: a.text.clone(),
        value: a.value.clone(),
        qualifier: a.qualifier.clone(),
        member: a.member.clone(),
    }
}

/// Serialize the projection to the SAME stable text as al-sem's
/// `serializeProjection`: `JSON.stringify(proj, null, 2)` + trailing newline.
/// `serde_json::to_string_pretty` uses 2-space indent identically.
pub fn serialize_projection(proj: &MergedIndexProjection) -> String {
    let mut s = serde_json::to_string_pretty(proj).expect("projection serializes");
    s.push('\n');
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixtures_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/r2-5a-fixtures")
    }

    #[test]
    fn core_fixture_merges_extension_field_into_base_table() {
        let dir = fixtures_dir().join("core-symbol-only");
        let proj = build_merged_index_from_path(&dir, "r2.5a");
        // Base Widget table 50000 must carry 5 fields (4 own + 1 merged TableExt).
        let base = proj
            .tables
            .iter()
            .find(|t| t.table_number == 50000)
            .expect("base Widget table present");
        assert_eq!(base.fields.len(), 5, "4 own + 1 merged extension field");
        // The merged field (number 50) is keyed to the BASE table.
        let merged = base
            .fields
            .iter()
            .find(|f| f.field_number == 50)
            .expect("merged Extra Info field present on base");
        assert_eq!(
            merged.stable_field_id,
            "aaaaaaaa-0000-0000-0000-000000000001:Table:50000#50"
        );
        // The extension's own table 50700 still retains the field under its own id.
        let ext = proj
            .tables
            .iter()
            .find(|t| t.table_number == 50700)
            .expect("extension table present");
        let ext_field = ext
            .fields
            .iter()
            .find(|f| f.field_number == 50)
            .expect("Extra Info on extension table");
        assert_eq!(
            ext_field.stable_field_id,
            "aaaaaaaa-0000-0000-0000-000000000001:Table:50700#50"
        );
    }

    #[test]
    fn collect_app_paths_single_and_dir() {
        let single = fixtures_dir()
            .join("source-included")
            .join("bbbbbbbb-0000-0000-0000-000000000002.app");
        assert_eq!(collect_app_paths(&single), vec![single.clone()]);
        let dir = fixtures_dir().join("source-included");
        assert_eq!(collect_app_paths(&dir), vec![single]);
    }
}

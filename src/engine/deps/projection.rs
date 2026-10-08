//! Rust port of al-sem's `src/deps/dependency-projection.ts` — `projectAbiToIndex`.
//!
//! Turns a [`SymbolReferenceAbi`] (the neutral ABI DTO) into dependency model
//! entities: objects, tables (fields + keys), and routines — each carrying the
//! same identity al-sem mints, so a routine seen as native source and as a dep
//! symbol resolves to the IDENTICAL `StableRoutineId`.
//!
//! Reuses the R0/R1 encoders from [`crate::engine::ids`]: `encode_object_id`,
//! `to_stable_object_id`, `canonical_routine_signature` + `sha256_hex` (the
//! `abi_signature_hash` crux), `encode_routine_id`, `sha256_of_strings` (the dep
//! object `sourceHash`). The table/field/key encoders mirror al-sem's
//! `encodeTableId`/`encodeFieldId`/`encodeKeyId` (ported here; they live local to
//! `extension_fields.rs` in the engine, kept private there).
//!
//! ORDERING (R2.5a Rev 2 #4): every nested collection preserves SymbolReference
//! array order — `parameters`, `attributes_parsed`, attribute `args`, table
//! `fields` (Fields[] order), key resolved field-ids (the key's FieldNames[]
//! order). NO HashMap iteration leaks into output: field-name → id resolution
//! uses a `BTreeMap` keyed by lowercased name, but emission walks the ordered
//! `field_names` Vec, so output order is the JSON's. Tables collect into a
//! `BTreeMap<i64, _>` keyed by object number (al-sem uses `Map<number,Table>`
//! and emits `[...values()]` in insertion order; with distinct table numbers a
//! BTreeMap by number is deterministic and matches).

use crate::engine::deps::symbol_reference::{AbiRoutine, SymbolReferenceAbi};
use crate::engine::ids::{
    self, CanonicalRoutineKey, ParamSpec, encode_field_id, encode_key_id, encode_object_id,
    encode_table_id, sha256_hex, sha256_of_strings, to_stable_field_id, to_stable_object_id,
    to_stable_table_id,
};
use crate::program::attributes::AttributeInfo;
use std::collections::BTreeMap;

/// A projected dependency parameter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectedParameter {
    pub index: usize,
    pub name: String,
    pub type_text: String,
    pub is_var: bool,
    pub is_record: bool,
    /// Task 6 (G7, RV-4): the param carries a `temporary` marker
    /// (`TypeDefinition.Temporary == true`). Drives the synthesized record-var
    /// `Known(true)` temp shape in the ABI→L3 projection.
    pub is_temporary: bool,
}

/// A projected dependency routine — the R2.5a comparison surface for routines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectedRoutine {
    /// Internal RoutineId: `${modelInstanceId}/${canonicalRoutineKeyHash}`.
    pub id: String,
    /// `StableRoutineId`: `${stableObjectId}#${normalizedSignatureHash}`. A dep ABI
    /// has no member triggers, so the enclosing-member fold (task 4) never applies
    /// on this side and the id is byte-identical to the pre-discriminator schema.
    pub stable_routine_id: String,
    /// `StableObjectId` of the declaring object.
    pub stable_object_id: String,
    /// `signatureFingerprint` == `abi_signature_hash` == sha256Hex(canonical).
    pub signature_fingerprint: String,
    pub canonical_string: String,
    pub object_id: String,
    pub name: String,
    pub kind: String,
    pub parameters: Vec<ProjectedParameter>,
    pub return_type: Option<String>,
    /// "internal" | "local" | None (from IsInternal/IsLocal).
    pub access_modifier: Option<String>,
    pub attributes_parsed: Vec<AttributeInfo>,
    pub body_available: bool,
    pub analysis_role: String,
}

/// A projected dependency field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectedField {
    pub id: String,
    pub stable_field_id: String,
    pub physical_table_id: String,
    pub declaring_object_id: String,
    pub declaring_app_id: String,
    pub field_number: i64,
    pub name: String,
    pub field_class: String,
    pub data_type: String,
    pub is_blob_like: bool,
}

/// A projected dependency key (resolved field-id list, in FieldNames[] order).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectedKey {
    pub id: String,
    pub physical_table_id: String,
    pub declaring_object_id: String,
    pub fields: Vec<String>,
}

/// A projected dependency table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectedTable {
    pub id: String,
    pub stable_table_id: String,
    pub app_guid: String,
    pub table_number: i64,
    pub name: String,
    pub fields: Vec<ProjectedField>,
    pub keys: Vec<ProjectedKey>,
    /// Task 6 (G7, RV-4): the table declares `TableType = Temporary`. Forwarded to
    /// `ModelTable.is_temporary` so the merged-whole `resolve()` applies the SAME
    /// table-level temp override (Task 4) to ABI record vars as to native ones.
    pub is_temporary: bool,
}

/// A projected dependency object.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectedObject {
    pub id: String,
    pub stable_object_id: String,
    pub app_guid: String,
    pub object_type: String,
    pub object_number: i64,
    pub name: String,
    pub source_unit_id: String,
    pub source_hash: String,
    pub analysis_role: String,
    pub object_subtype: Option<String>,
    pub page_type: Option<String>,
    pub source_table_name: Option<String>,
    pub extends_target_name: Option<String>,
    pub implements_interfaces: Option<Vec<String>>,
    pub inherent_commit_behavior: Option<String>,
    /// Page controls (name, kind-string, target) forwarded from the ABI — see AbiObject.
    pub page_controls: Vec<(String, String, String)>,
}

/// The projection result — dependency model entities.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProjectedAbi {
    pub objects: Vec<ProjectedObject>,
    pub tables: Vec<ProjectedTable>,
    pub routines: Vec<ProjectedRoutine>,
}

/// Normalized signature hash for an ABI routine — mirrors `abiSignatureHash`:
/// `sha256Hex(canonicalRoutineSignature(name, params, returnType))`. `isRecord`/
/// `tableName` are irrelevant to the canonical form (it hashes types only).
pub fn abi_signature_hash(r: &AbiRoutine) -> String {
    let params: Vec<ParamSpec> = r
        .parameters
        .iter()
        .map(|p| ParamSpec {
            type_text: p.type_text.clone(),
            is_var: p.is_var,
        })
        .collect();
    sha256_hex(&ids::canonical_routine_signature(
        &r.name,
        &params,
        r.return_type_text.as_deref(),
    ))
}

/// The canonical signature string (lowercased) — exposed for vector assertions.
pub fn abi_canonical_string(r: &AbiRoutine) -> String {
    let params: Vec<ParamSpec> = r
        .parameters
        .iter()
        .map(|p| ParamSpec {
            type_text: p.type_text.clone(),
            is_var: p.is_var,
        })
        .collect();
    ids::canonical_routine_signature(&r.name, &params, r.return_type_text.as_deref())
}

/// `^Record\b` (case-insensitive) — mirrors the TS `isRecord` regex.
fn is_record_type(type_text: &str) -> bool {
    let lower = type_text.to_lowercase();
    if let Some(rest) = lower.strip_prefix("record") {
        // `\b` after "record": next char is a non-word char or end.
        rest.chars()
            .next()
            .map(|c| !(c.is_ascii_alphanumeric() || c == '_'))
            .unwrap_or(true)
    } else {
        false
    }
}

/// Project a single ABI routine to a [`ProjectedRoutine`]. Mirrors
/// `abiRoutineToRoutine`.
#[allow(clippy::too_many_arguments)]
fn abi_routine_to_routine(
    r: &AbiRoutine,
    object_id: &str,
    app_guid: &str,
    object_type: &str,
    object_number: i64,
    model_instance_id: &str,
) -> ProjectedRoutine {
    let normalized_hash = abi_signature_hash(r);
    let canonical = CanonicalRoutineKey {
        app_guid: app_guid.to_string(),
        object_type: object_type.to_string(),
        object_number,
        routine_kind: r.kind.clone(),
        routine_name: r.name.clone(),
        normalized_signature_hash: normalized_hash.clone(),
        // Always `None`: a dependency ABI exposes procedures and event
        // publishers/subscribers, never member triggers, and it carries no member
        // wrapper to name one with. The conditional append in
        // `encode_canonical_routine_key` therefore leaves every dep-side id
        // byte-identical, which is what keeps the cross-app join symmetric with
        // the source side by construction rather than by coincidence.
        enclosing_member: None,
    };

    // accessModifier: isInternal ? "internal" : isLocal ? "local" : undefined.
    let access_modifier = if r.is_internal {
        Some("internal".to_string())
    } else if r.is_local {
        Some("local".to_string())
    } else {
        None
    };

    let stable_object_id = to_stable_object_id(object_id);
    // `None` for the same reason as `canonical.enclosing_member` above — a dep ABI
    // has no member triggers — so every dep-side STABLE id is byte-identical too and
    // the cross-app stable-id join stays symmetric by construction.
    let stable_routine_id =
        ids::to_stable_routine_id_from_parts(&stable_object_id, &normalized_hash, None);

    ProjectedRoutine {
        id: ids::encode_routine_id(&canonical, model_instance_id),
        stable_routine_id,
        stable_object_id,
        signature_fingerprint: normalized_hash,
        canonical_string: abi_canonical_string(r),
        object_id: object_id.to_string(),
        name: r.name.clone(),
        kind: r.kind.clone(),
        parameters: r
            .parameters
            .iter()
            .enumerate()
            .map(|(index, p)| ProjectedParameter {
                index,
                name: p.name.clone(),
                type_text: p.type_text.clone(),
                is_var: p.is_var,
                is_record: is_record_type(&p.type_text),
                is_temporary: p.is_temporary,
            })
            .collect(),
        return_type: r.return_type_text.clone(),
        access_modifier,
        attributes_parsed: r.attributes_parsed.clone(),
        body_available: false,
        analysis_role: "dependency".to_string(),
    }
}

/// Project a [`SymbolReferenceAbi`] into dependency model entities. `app_guid` is
/// the dependency app identity — for parity with the TS pipeline it comes from the
/// MANIFEST `<App>` element (`ref.appGuid`), NOT `SymbolReference.json`'s `AppId`
/// (R2.5a Rev 2 #2). Mirrors `projectAbiToIndex`.
pub fn project_abi_to_index(
    abi: &SymbolReferenceAbi,
    app_guid: &str,
    model_instance_id: &str,
) -> ProjectedAbi {
    let source_unit_id = format!("dep:{app_guid}:__symbols__");
    let mut objects: Vec<ProjectedObject> = Vec::new();
    let mut routines: Vec<ProjectedRoutine> = Vec::new();
    // al-sem uses Map<number, Table> and emits insertion order; BTreeMap by the
    // table's object number is deterministic and matches with distinct numbers.
    let mut tables_by_number: BTreeMap<i64, ProjectedTable> = BTreeMap::new();

    for t in &abi.tables {
        let table_id = encode_table_id(app_guid, t.object_number);
        let object_id = encode_object_id(app_guid, "Table", t.object_number);
        let stable_table_id = to_stable_table_id(app_guid, t.object_number);

        let fields: Vec<ProjectedField> = t
            .fields
            .iter()
            .map(|f| ProjectedField {
                id: encode_field_id(&table_id, f.field_number),
                stable_field_id: to_stable_field_id(app_guid, t.object_number, f.field_number),
                physical_table_id: table_id.clone(),
                declaring_object_id: object_id.clone(),
                declaring_app_id: app_guid.to_string(),
                field_number: f.field_number,
                name: f.name.clone(),
                field_class: f.field_class.clone(),
                data_type: f.data_type.clone(),
                is_blob_like: f.is_blob_like,
            })
            .collect();

        // field-name (lowercased) → field id, for key resolution. BTreeMap is used
        // only as a lookup; key emission walks the ORDERED field_names Vec.
        let fields_by_name: BTreeMap<String, String> = fields
            .iter()
            .map(|f| (f.name.to_lowercase(), f.id.clone()))
            .collect();

        let keys: Vec<ProjectedKey> = t
            .keys
            .iter()
            .enumerate()
            .map(|(index, k)| ProjectedKey {
                id: encode_key_id(&table_id, index),
                physical_table_id: table_id.clone(),
                declaring_object_id: object_id.clone(),
                fields: k
                    .field_names
                    .iter()
                    .filter_map(|n| fields_by_name.get(&n.to_lowercase()).cloned())
                    .collect(),
            })
            .collect();

        tables_by_number.insert(
            t.object_number,
            ProjectedTable {
                id: table_id,
                stable_table_id,
                app_guid: app_guid.to_string(),
                table_number: t.object_number,
                name: t.name.clone(),
                fields,
                keys,
                is_temporary: t.is_temporary,
            },
        );
    }

    for o in &abi.objects {
        let object_id = encode_object_id(app_guid, &o.object_type, o.object_number);
        objects.push(ProjectedObject {
            id: object_id.clone(),
            stable_object_id: to_stable_object_id(&object_id),
            app_guid: app_guid.to_string(),
            object_type: o.object_type.clone(),
            object_number: o.object_number,
            name: o.name.clone(),
            source_unit_id: source_unit_id.clone(),
            source_hash: sha256_of_strings(&[
                app_guid.to_string(),
                o.object_type.clone(),
                o.object_number.to_string(),
            ]),
            analysis_role: "dependency".to_string(),
            object_subtype: o.object_subtype.clone(),
            page_type: o.page_type.clone(),
            source_table_name: o.source_table_name.clone(),
            extends_target_name: o.extends_target_name.clone(),
            implements_interfaces: o.implemented_interfaces.clone(),
            inherent_commit_behavior: o.inherent_commit_behavior.clone(),
            page_controls: o.page_controls.clone(),
        });
        for r in &o.routines {
            routines.push(abi_routine_to_routine(
                r,
                &object_id,
                app_guid,
                &o.object_type,
                o.object_number,
                model_instance_id,
            ));
        }
    }

    ProjectedAbi {
        objects,
        tables: tables_by_number.into_values().collect(),
        routines,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::deps::symbol_reference::{AbiEventKind, AbiParameter, SubtypeTag};

    fn routine(name: &str, params: Vec<AbiParameter>, ret: Option<&str>) -> AbiRoutine {
        AbiRoutine {
            name: name.to_string(),
            kind: "procedure".to_string(),
            event_kind: AbiEventKind::Unknown,
            parameters: params,
            return_type_text: ret.map(|s| s.to_string()),
            return_type_id: None,
            is_local: false,
            is_internal: false,
            is_protected: false,
            parameters_known: true,
            attributes: vec![],
            attributes_parsed: vec![],
        }
    }

    #[test]
    fn is_record_matches_word_boundary() {
        assert!(is_record_type("Record Customer"));
        assert!(is_record_type("record \"Sales Header\""));
        assert!(!is_record_type("RecordRef")); // \b fails: 'R' is a word char
        assert!(!is_record_type("Integer"));
    }

    #[test]
    fn signature_hash_stable_across_model_instances() {
        let r = routine(
            "RecordUnquoted",
            vec![AbiParameter {
                name: "c".to_string(),
                type_text: "Record Customer".to_string(),
                is_var: false,
                is_temporary: false,
                subtype_id: None,
                subtype_raw_name: None,
                subtype_tag: SubtypeTag::NoSubtype,
            }],
            Some("Integer"),
        );
        let h1 = abi_signature_hash(&r);
        let abi = SymbolReferenceAbi {
            objects: vec![super::super::symbol_reference::AbiObject {
                object_type: "Codeunit".to_string(),
                object_number: 50100,
                name: "X".to_string(),
                routines: vec![r],
                ..Default::default()
            }],
            ..Default::default()
        };
        let a = project_abi_to_index(&abi, "11111111-2222-3333-4444-555555555555", "instance-A");
        let b = project_abi_to_index(&abi, "11111111-2222-3333-4444-555555555555", "instance-B");
        assert_eq!(
            a.routines[0].stable_routine_id,
            b.routines[0].stable_routine_id
        );
        assert_eq!(a.routines[0].signature_fingerprint, h1);
        assert_ne!(a.routines[0].id, b.routines[0].id); // internal id differs
    }
}

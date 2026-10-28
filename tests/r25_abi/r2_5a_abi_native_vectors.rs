//! R2.5a ABI-vs-native SIGNATURE/ID parity — Rust replay of al-sem Task 1's
//! `abi-native-sig-vectors.json`.
//!
//! Source of truth: `tests/r2-5a-vectors/abi-native-sig-vectors.json` (copied from
//! al-sem `scripts/r2.5a-goldens/r2.5a-abi-native-sig-vectors.json`). The al-sem
//! task already proved native == ABI for every component (running BOTH pipelines
//! over the SAME `.app`); THIS test proves the Rust ABI projection reproduces the
//! recorded values byte-for-byte.
//!
//! Per row we reconstruct the ABI routine from its recorded `routineName`,
//! `abiParamTypeText`, `abiReturnType`, and `tuple.routineKind`, project it via
//! the Rust `.app`-reader projection, and assert the canonical signature string,
//! the `normalizedSignatureHash`, the `StableObjectId`, the `StableRoutineId`, and
//! the full internal `routineId` (under the file's `modelInstanceId`) EQUAL the
//! vector's recorded values.

use al_sem::engine::deps::projection::{abi_canonical_string, project_abi_to_index};
use al_sem::engine::deps::symbol_reference::{
    AbiEventKind, AbiObject, AbiParameter, AbiRoutine, SubtypeTag, SymbolReferenceAbi,
};
use serde_json::Value;

/// Reconstruct an `AbiRoutine` from a sig-vector row. `abiParamTypeText` is the
/// ordered list of param type texts; param names are irrelevant to the canonical
/// signature so we synthesize positional `pN`. No `var` params appear in this
/// vector set EXCEPT `VarPrimitive`, whose `var ` prefix is recorded in the
/// canonical string and comes from the IsVar flag — so we detect it from the
/// expected canonical string to keep the row self-describing.
fn routine_from_row(row: &Value) -> AbiRoutine {
    let name = row["routineName"].as_str().unwrap().to_string();
    let kind = row["tuple"]["routineKind"].as_str().unwrap().to_string();
    let params: Vec<AbiParameter> = row["abiParamTypeText"]
        .as_array()
        .unwrap()
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let type_text = t.as_str().unwrap().to_string();
            // The vector records `var ` in the canonical string, never in the
            // typeText. Recover the IsVar flag from the canonical string so the
            // reconstructed ABI matches the recorded signature.
            let canonical = row["canonicalString"].as_str().unwrap();
            // Heuristic: a param is `var` if the canonical form prefixes its
            // lowercased type with `var `. Only one such row (VarPrimitive) exists.
            let is_var = canonical.contains(&format!("var {}", type_text.to_lowercase()));
            AbiParameter {
                name: format!("p{i}"),
                type_text,
                is_var,
                is_temporary: false,
                subtype_id: None,
                subtype_raw_name: None,
                subtype_tag: SubtypeTag::NoSubtype,
            }
        })
        .collect();
    let return_type_text = match &row["abiReturnType"] {
        Value::String(s) => Some(s.clone()),
        _ => None,
    };
    AbiRoutine {
        name,
        kind,
        event_kind: AbiEventKind::Unknown,
        parameters: params,
        return_type_text,
        return_type_id: None,
        is_local: false,
        is_internal: false,
        is_protected: false,
        parameters_known: true,
        method_id: None,
        attributes: vec![],
        attributes_parsed: vec![],
    }
}

#[test]
fn abi_native_sig_vectors_replay_byte_for_byte() {
    let raw = include_str!("../r2-5a-vectors/abi-native-sig-vectors.json");
    let doc: Value = serde_json::from_str(raw).expect("sig vectors must parse");

    let app_guid = doc["appGuid"].as_str().unwrap();
    let object_type = doc["objectType"].as_str().unwrap();
    let object_number = doc["objectNumber"].as_i64().unwrap();
    let model_instance_id = doc["modelInstanceId"].as_str().unwrap();
    let rows = doc["rows"].as_array().expect("rows array");

    let declared = doc["count"].as_u64().expect("count") as usize;
    assert_eq!(declared, rows.len(), "declared count != actual rows");

    let mut failures = Vec::new();
    for (i, row) in rows.iter().enumerate() {
        let r = routine_from_row(row);

        // Build a single-object ABI and project it under the manifest appGuid.
        let abi = SymbolReferenceAbi {
            objects: vec![AbiObject {
                object_type: object_type.to_string(),
                object_number,
                name: "TestCodeunit".to_string(),
                routines: vec![r.clone()],
                ..Default::default()
            }],
            ..Default::default()
        };
        let projected = project_abi_to_index(&abi, app_guid, model_instance_id);
        assert_eq!(projected.routines.len(), 1, "one routine expected");
        let pr = &projected.routines[0];

        let exp_canonical = row["canonicalString"].as_str().unwrap();
        let exp_hash = row["normalizedSignatureHash"].as_str().unwrap();
        let exp_stable_object = row["stableObjectId"].as_str().unwrap();
        let exp_stable_routine = row["stableRoutineId"].as_str().unwrap();
        let exp_routine_id = row["routineId"].as_str().unwrap();

        // Cross-check: the canonical-string helper agrees with the projection too.
        let helper_canonical = abi_canonical_string(&r);

        let mut row_errs = Vec::new();
        if pr.canonical_string != exp_canonical || helper_canonical != exp_canonical {
            row_errs.push(format!(
                "canonical: expected {exp_canonical:?}, got {:?} (helper {:?})",
                pr.canonical_string, helper_canonical
            ));
        }
        if pr.signature_fingerprint != exp_hash {
            row_errs.push(format!(
                "hash: expected {exp_hash:?}, got {:?}",
                pr.signature_fingerprint
            ));
        }
        if pr.stable_object_id != exp_stable_object {
            row_errs.push(format!(
                "StableObjectId: expected {exp_stable_object:?}, got {:?}",
                pr.stable_object_id
            ));
        }
        if pr.stable_routine_id != exp_stable_routine {
            row_errs.push(format!(
                "StableRoutineId: expected {exp_stable_routine:?}, got {:?}",
                pr.stable_routine_id
            ));
        }
        if pr.id != exp_routine_id {
            row_errs.push(format!(
                "routineId: expected {exp_routine_id:?}, got {:?}",
                pr.id
            ));
        }

        if !row_errs.is_empty() {
            let rn = row["routineName"].as_str().unwrap();
            failures.push(format!(
                "\n  row #{i} ({rn}):\n    {}",
                row_errs.join("\n    ")
            ));
        }
    }

    assert!(
        failures.is_empty(),
        "{} of {} sig vectors FAILED:{}",
        failures.len(),
        rows.len(),
        failures.join("")
    );
    assert_eq!(rows.len(), 12, "expected 12 committed sig vectors");
}

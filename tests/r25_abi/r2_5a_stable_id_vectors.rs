//! R2.5a StableRoutineId modelInstanceId-independence — Rust replay of al-sem
//! Task 1's `stable-id-independence-vector.json`.
//!
//! Source of truth: `tests/r2-5a-vectors/stable-id-independence-vector.json`. The
//! invariant: the SAME ABI projected under two DIFFERENT `modelInstanceId`s yields
//! byte-identical `StableRoutineId` + `signatureFingerprint` (only the internal
//! `RoutineId` prefix changes). This is what lets the cross-app callGraph `to` key
//! stay modelInstanceId-independent so the `dep:<artifactKey>` cache key can be
//! deferred.
//!
//! Reconstruction: the 15 rows are the 12 signature-vector routines plus the 3
//! attribute-vector routines (OnBeforePost / OnAfterCalc / HandleEvent). We
//! rebuild each routine's ABI shape from those two source vector files (the sig
//! file gives param/return type text; the attr file gives the event attributes),
//! project under both modelInstanceIds, and assert against the recorded
//! independence values.

use al_sem::engine::deps::projection::{ProjectedRoutine, project_abi_to_index};
use al_sem::engine::deps::symbol_reference::{
    AbiEventKind, AbiObject, AbiParameter, AbiRoutine, SubtypeTag, SymbolReferenceAbi,
    parse_symbol_reference,
};
use serde_json::{Value, json};
use std::collections::HashMap;

const OBJECT_TYPE: &str = "Codeunit";
const OBJECT_NUMBER: i64 = 50100;

/// Reconstruct an `AbiRoutine` from a signature-vector row (same logic as
/// `r2_5a_abi_native_vectors.rs::routine_from_row`).
fn routine_from_sig_row(row: &Value) -> AbiRoutine {
    let name = row["routineName"].as_str().unwrap().to_string();
    let kind = row["tuple"]["routineKind"].as_str().unwrap().to_string();
    let canonical = row["canonicalString"].as_str().unwrap();
    let params: Vec<AbiParameter> = row["abiParamTypeText"]
        .as_array()
        .unwrap()
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let type_text = t.as_str().unwrap().to_string();
            let is_var = canonical.contains(&format!("var {}", type_text.to_lowercase()));
            AbiParameter {
                name: format!("p{i}"),
                type_text,
                is_var,
                is_temporary: false,
                subtype_id: None,
                subtype_raw_name: None,
                subtype_tag: SubtypeTag::NoSubtype,
                array_dims: None,
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

/// Reconstruct the 3 attribute-vector routines via the FULL parse path so their
/// derived kind + parsed attributes are authentic (the event routines have no
/// params / return, so their signature is `name():`).
fn attr_routines(attr_doc: &Value) -> Vec<AbiRoutine> {
    let rows = attr_doc["rows"].as_array().unwrap();
    let mut methods = Vec::new();
    for row in rows {
        let name = row["routineName"].as_str().unwrap();
        let attrs_in = row["abiAttributesParsed"].as_array().unwrap();
        let attributes: Vec<Value> = attrs_in
            .iter()
            .map(|a| {
                let attr_name = a["name"].as_str().unwrap();
                let args: Vec<Value> = a["args"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|arg| {
                        let kind = arg["kind"].as_str().unwrap();
                        let text = arg["text"].as_str().unwrap();
                        let v = if kind == "boolean" {
                            match text {
                                "true" => json!(true),
                                "false" => json!(false),
                                _ => json!(text),
                            }
                        } else {
                            json!(text)
                        };
                        json!({ "Value": v })
                    })
                    .collect();
                json!({ "Name": attr_name, "Arguments": args })
            })
            .collect();
        methods.push(json!({ "Name": name, "Parameters": [], "Attributes": attributes }));
    }
    let doc = json!({
        "AppId": "x",
        "Codeunits": [ { "Id": OBJECT_NUMBER, "Name": "AttrCodeunit", "Methods": methods } ],
    });
    let abi = parse_symbol_reference(&serde_json::to_string(&doc).unwrap());
    abi.objects.into_iter().next().unwrap().routines
}

/// Project one routine under `model_instance_id`, returning its projected form.
fn project_one(r: &AbiRoutine, app_guid: &str, model_instance_id: &str) -> ProjectedRoutine {
    let abi = SymbolReferenceAbi {
        objects: vec![AbiObject {
            object_type: OBJECT_TYPE.to_string(),
            object_number: OBJECT_NUMBER,
            name: "C".to_string(),
            routines: vec![r.clone()],
            ..Default::default()
        }],
        ..Default::default()
    };
    project_abi_to_index(&abi, app_guid, model_instance_id)
        .routines
        .into_iter()
        .next()
        .unwrap()
}

#[test]
fn stable_id_independence_vectors_replay_byte_for_byte() {
    let stable_raw = include_str!("../r2-5a-vectors/stable-id-independence-vector.json");
    let sig_raw = include_str!("../r2-5a-vectors/abi-native-sig-vectors.json");
    let attr_raw = include_str!("../r2-5a-vectors/abi-native-attr-vectors.json");

    let stable_doc: Value = serde_json::from_str(stable_raw).expect("stable vectors must parse");
    let sig_doc: Value = serde_json::from_str(sig_raw).expect("sig vectors must parse");
    let attr_doc: Value = serde_json::from_str(attr_raw).expect("attr vectors must parse");

    let app_guid = stable_doc["appGuid"].as_str().unwrap();
    let mid_a = stable_doc["modelInstanceIdA"].as_str().unwrap();
    let mid_b = stable_doc["modelInstanceIdB"].as_str().unwrap();
    let rows = stable_doc["rows"].as_array().expect("rows array");
    let declared = stable_doc["count"].as_u64().expect("count") as usize;
    assert_eq!(declared, rows.len(), "declared count != actual rows");

    // Build a name → reconstructed AbiRoutine map from the two source files.
    let mut by_name: HashMap<String, AbiRoutine> = HashMap::new();
    for row in sig_doc["rows"].as_array().unwrap() {
        let r = routine_from_sig_row(row);
        by_name.insert(r.name.clone(), r);
    }
    for r in attr_routines(&attr_doc) {
        by_name.insert(r.name.clone(), r);
    }

    let mut failures = Vec::new();
    for (i, row) in rows.iter().enumerate() {
        let name = row["routineName"].as_str().unwrap();
        let r = match by_name.get(name) {
            Some(r) => r,
            None => {
                failures.push(format!("\n  row #{i} ({name}): no reconstructed routine"));
                continue;
            }
        };

        let pa = project_one(r, app_guid, mid_a);
        let pb = project_one(r, app_guid, mid_b);

        let exp_stable = row["stableRoutineId"].as_str().unwrap();
        let exp_fp = row["signatureFingerprint"].as_str().unwrap();
        let exp_int_a = row["internalRoutineIdA"].as_str().unwrap();
        let exp_int_b = row["internalRoutineIdB"].as_str().unwrap();

        let mut row_errs = Vec::new();
        // Independence: stable id + fingerprint identical across instances.
        if pa.stable_routine_id != pb.stable_routine_id {
            row_errs.push(format!(
                "stableRoutineId differs across instances: {:?} vs {:?}",
                pa.stable_routine_id, pb.stable_routine_id
            ));
        }
        if pa.signature_fingerprint != pb.signature_fingerprint {
            row_errs.push("signatureFingerprint differs across instances".to_string());
        }
        // Match the recorded values.
        if pa.stable_routine_id != exp_stable {
            row_errs.push(format!(
                "StableRoutineId: expected {exp_stable:?}, got {:?}",
                pa.stable_routine_id
            ));
        }
        if pa.signature_fingerprint != exp_fp {
            row_errs.push(format!(
                "signatureFingerprint: expected {exp_fp:?}, got {:?}",
                pa.signature_fingerprint
            ));
        }
        if pa.id != exp_int_a {
            row_errs.push(format!(
                "internalRoutineIdA: expected {exp_int_a:?}, got {:?}",
                pa.id
            ));
        }
        if pb.id != exp_int_b {
            row_errs.push(format!(
                "internalRoutineIdB: expected {exp_int_b:?}, got {:?}",
                pb.id
            ));
        }
        // The internal ids MUST differ (modelInstanceId is in the prefix).
        if row["internalRoutineIdDiffers"].as_bool() == Some(true) && pa.id == pb.id {
            row_errs.push("internal ids expected to differ but matched".to_string());
        }

        if !row_errs.is_empty() {
            failures.push(format!(
                "\n  row #{i} ({name}):\n    {}",
                row_errs.join("\n    ")
            ));
        }
    }

    assert!(
        failures.is_empty(),
        "{} of {} stable-id vectors FAILED:{}",
        failures.len(),
        rows.len(),
        failures.join("")
    );
    assert_eq!(rows.len(), 15, "expected 15 committed stable-id vectors");
}

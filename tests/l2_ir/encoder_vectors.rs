//! R0 differential harness: every committed encoder vector must reproduce the
//! al-sem `expected` string byte-for-byte.
//!
//! Source of truth: `tests/r0-vectors/encoder-vectors.json` (copied from al-sem
//! `scripts/r0-goldens/encoder-vectors.json` @ f0ae38c). On mismatch the failure
//! message prints kind / note / input / expected / actual — locality matters
//! because these are hashes.

use al_sem::engine::ids;
use serde_json::Value;

/// Parse the `parameters` array of a vector into the `(typeText, isVar)` pairs
/// that `canonical_routine_signature` expects.
fn parse_params(v: &Value) -> Vec<ids::ParamSpec> {
    v.as_array()
        .map(|arr| {
            arr.iter()
                .map(|p| ids::ParamSpec {
                    type_text: p["typeText"].as_str().unwrap_or("").to_string(),
                    is_var: p["isVar"].as_bool().unwrap_or(false),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// `returnTypeText` may be JSON null → None.
fn opt_str(v: &Value) -> Option<String> {
    v.as_str().map(|s| s.to_string())
}

fn build_key(input: &Value) -> ids::CanonicalRoutineKey {
    ids::CanonicalRoutineKey {
        app_guid: input["appGuid"].as_str().unwrap().to_string(),
        object_type: input["objectType"].as_str().unwrap().to_string(),
        // Every committed vector names a numbered object, so its key is the number:
        // the same bytes the hash always took (#83).
        object_key: ids::object_key(Some(input["objectNumber"].as_i64().unwrap()), ""),
        routine_kind: input["routineKind"].as_str().unwrap().to_string(),
        routine_name: input["routineName"].as_str().unwrap().to_string(),
        normalized_signature_hash: input["normalizedSignatureHash"]
            .as_str()
            .unwrap()
            .to_string(),
        // The committed vectors predate the member discriminator and carry no
        // `enclosingMember` — which is exactly the point: `None` takes the
        // CONDITIONAL append's absent branch, so every vector here must stay
        // byte-identical. A vector moving is a shape/schema regression, not a
        // rebaseline. (`ids::encode_canonical_routine_key`, Task 3.)
        enclosing_member: input
            .get("enclosingMember")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
    }
}

/// Dispatch a single vector to its encoder, returning the produced string.
fn encode(kind: &str, input: &Value) -> String {
    match kind {
        "encodeObjectId" => ids::encode_object_id(
            input["appGuid"].as_str().unwrap(),
            input["objectType"].as_str().unwrap(),
            &ids::object_key(Some(input["objectNumber"].as_i64().unwrap()), ""),
        ),
        "toStableObjectId" => ids::to_stable_object_id(input["internalObjectId"].as_str().unwrap()),
        "canonicalRoutineSignature" => ids::canonical_routine_signature(
            input["name"].as_str().unwrap(),
            &parse_params(&input["parameters"]),
            opt_str(&input["returnTypeText"]).as_deref(),
        ),
        "sha256Hex" => ids::sha256_hex(input["value"].as_str().unwrap()),
        "sha256OfStrings" => {
            let parts: Vec<String> = input["parts"]
                .as_array()
                .unwrap()
                .iter()
                .map(|p| p.as_str().unwrap().to_string())
                .collect();
            ids::sha256_of_strings(&parts)
        }
        "encodeCanonicalRoutineKey" => ids::encode_canonical_routine_key(&build_key(input)),
        "encodeRoutineId" => ids::encode_routine_id(
            &build_key(&input["key"]),
            input["modelInstanceId"].as_str().unwrap(),
        ),
        "normalizedSignatureHash" => ids::normalized_signature_hash(
            input["name"].as_str().unwrap(),
            &parse_params(&input["parameters"]),
            opt_str(&input["returnTypeText"]).as_deref(),
        ),
        // Same contract as `build_key`'s `enclosingMember` (Task 4, for the STABLE
        // id): the committed vectors carry none, so every one takes the CONDITIONAL
        // fold's absent branch and must stay byte-identical.
        "toStableRoutineIdFromParts" => ids::to_stable_routine_id_from_parts(
            input["stableObjectId"].as_str().unwrap(),
            input["normalizedSignatureHash"].as_str().unwrap(),
            input.get("enclosingMember").and_then(|v| v.as_str()),
        ),
        "routineSignatureFingerprint" => ids::routine_signature_fingerprint(
            input["name"].as_str().unwrap(),
            &parse_params(&input["parameters"]),
            opt_str(&input["returnTypeText"]).as_deref(),
        ),
        "objectSignatureFingerprint" => ids::object_signature_fingerprint(
            input["objectType"].as_str().unwrap(),
            input["objectNumber"].as_i64().unwrap(),
            input["name"].as_str().unwrap(),
        ),
        other => panic!("unknown vector kind: {other}"),
    }
}

#[test]
fn all_encoder_vectors_pass_byte_for_byte() {
    let raw = include_str!("../r0-vectors/encoder-vectors.json");
    let doc: Value = serde_json::from_str(raw).expect("encoder-vectors.json must parse");

    let vectors = doc["vectors"].as_array().expect("vectors must be an array");

    // Cross-check the declared count so a truncated/extended file is caught.
    let declared = doc["count"].as_u64().expect("count must be present");
    assert_eq!(
        declared as usize,
        vectors.len(),
        "declared count ({declared}) != actual vector count ({})",
        vectors.len()
    );

    let mut failures = Vec::new();
    for (i, vec) in vectors.iter().enumerate() {
        let kind = vec["kind"].as_str().expect("vector kind must be a string");
        let note = vec["note"].as_str().unwrap_or("");
        let input = &vec["input"];
        let expected = vec["expected"].as_str().expect("expected must be a string");

        let actual = encode(kind, input);
        if actual != expected {
            failures.push(format!(
                "\n  vector #{i} kind={kind} note={note:?}\n    input    = {input}\n    expected = {expected:?}\n    actual   = {actual:?}"
            ));
        }
    }

    assert!(
        failures.is_empty(),
        "{} of {} encoder vectors FAILED:{}",
        failures.len(),
        vectors.len(),
        failures.join("")
    );

    // Belt-and-suspenders: prove we actually exercised all 48.
    assert_eq!(vectors.len(), 48, "expected 48 committed vectors");
}

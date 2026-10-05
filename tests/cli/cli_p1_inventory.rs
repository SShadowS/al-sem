//! P1 — `fingerprint --inventory-only` lean projection test.
//!
//! For the `ws-d8-commit-in-tx` in-repo corpus fixture, runs BOTH:
//!   1. The full `fingerprint --format json` (no query flag) → capability-snapshot
//!      envelope.
//!   2. The new `fingerprint --inventory-only --format json` → routine-inventory
//!      envelope.
//!
//! Assertions:
//!   (a) Projection-subset self-consistency: `apps`, `coverage`,
//!       `rootClassifications`, `identities` byte-identical between the two docs.
//!   (b) Heavy keys absent from the inventory doc.
//!   (c) Per-routine inventory: every entry has a non-empty `stableRoutineId` plus
//!       parseable `objectType`, `objectNumber`, `routineName` fields.

use std::path::PathBuf;

use al_sem::engine::l5::fingerprint_cli::{
    FingerprintFormat, FingerprintOptions, FingerprintOutput, run_fingerprint_pipeline,
};

fn corpus_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("r0-corpus")
}

const FIXTURE: &str = "ws-d8-commit-in-tx";

/// Build the base `FingerprintOptions` for the fixture.
fn base_opts(ws: &std::path::Path) -> FingerprintOptions<'_> {
    FingerprintOptions {
        workspace: ws,
        driver_version: "p1-test-v1",
        format: FingerprintFormat::Json,
        out: None,
        shard: None,
        witness_limit: None,
        roots: None,
        routine_selectors: Vec::new(),
        include_inherited: true,
        is_query_requested: false, // B0 path (full snapshot envelope)
        deterministic: true,
        strict: false,
        verbosity: "compact",
        inventory_only: false,
        no_roots_config: false,
    }
}

#[test]
fn inventory_only_projection_subset_self_consistency() {
    let ws = corpus_dir().join(FIXTURE);
    assert!(
        ws.is_dir(),
        "fixture {FIXTURE} not found at {}",
        ws.display()
    );

    // --- 1. Full capability-snapshot envelope ---
    let full_opts = base_opts(&ws);
    let full_result = run_fingerprint_pipeline(&full_opts)
        .unwrap_or_else(|e| panic!("full fingerprint pipeline error: {e}"));
    let full_json = match full_result.output {
        FingerprintOutput::Text(t) => t,
        _ => panic!("expected Text from full fingerprint"),
    };

    // --- 2. Inventory-only envelope ---
    let mut inv_opts = base_opts(&ws);
    inv_opts.inventory_only = true;
    let inv_result = run_fingerprint_pipeline(&inv_opts)
        .unwrap_or_else(|e| panic!("inventory-only fingerprint pipeline error: {e}"));
    let inv_json = match inv_result.output {
        FingerprintOutput::Text(t) => t,
        _ => panic!("expected Text from inventory-only fingerprint"),
    };

    // Parse both as serde_json Values.
    let full_doc: serde_json::Value =
        serde_json::from_str(&full_json).expect("full doc must parse as JSON");
    let inv_doc: serde_json::Value =
        serde_json::from_str(&inv_json).expect("inventory doc must parse as JSON");

    // (a) Envelope header checks: kind must be "routine-inventory", schemaVersion
    //     must be "1.1.0" for the inventory doc (engine-e2: additive enclosingMember/
    //     originatingObject fields).
    assert_eq!(
        inv_doc["kind"].as_str().unwrap_or(""),
        "routine-inventory",
        "inventory doc kind must be 'routine-inventory'"
    );
    assert_eq!(
        inv_doc["schemaVersion"].as_str().unwrap_or(""),
        "1.1.0",
        "inventory doc schemaVersion must be '1.1.0'"
    );

    // (a) Projection-subset self-consistency: apps, coverage, rootClassifications,
    //     identities must be BYTE-IDENTICAL (same sub-values from the same derivers).
    for key in &["apps", "coverage", "rootClassifications", "identities"] {
        let full_val = &full_doc["payload"][key];
        let inv_val = &inv_doc["payload"][key];
        assert_eq!(
            full_val, inv_val,
            "payload.{key} must be byte-identical between full and inventory docs"
        );
    }

    // (b) Heavy keys must be ABSENT from the inventory doc payload.
    let heavy_keys = &[
        "capabilityFacts",
        "typedEdges",
        "operationIndex",
        "callsiteIndex",
        "callsiteResolutions",
        "analysisGaps",
        "inputs",
        "inputsMetadata",
    ];
    let inv_payload = inv_doc["payload"]
        .as_object()
        .expect("inventory payload is object");
    for key in heavy_keys {
        assert!(
            !inv_payload.contains_key(*key),
            "inventory doc payload must NOT contain '{key}'"
        );
    }

    // (c) Per-routine inventory: every entry has non-empty stableRoutineId,
    //     non-empty objectType, a numeric objectNumber, and non-empty routineName.
    let routines_val = &inv_doc["payload"]["routineInventory"];
    let routines = routines_val
        .as_array()
        .expect("payload.routineInventory must be an array");
    assert!(
        !routines.is_empty(),
        "routineInventory must be non-empty for {FIXTURE}"
    );
    for (i, entry) in routines.iter().enumerate() {
        let stable_id = entry["stableRoutineId"]
            .as_str()
            .unwrap_or_else(|| panic!("entry[{i}].stableRoutineId must be a string"));
        assert!(
            !stable_id.is_empty(),
            "entry[{i}].stableRoutineId must not be empty"
        );

        let object_type = entry["objectType"]
            .as_str()
            .unwrap_or_else(|| panic!("entry[{i}].objectType must be a string"));
        assert!(
            !object_type.is_empty(),
            "entry[{i}].objectType must not be empty"
        );

        let object_number = entry["objectNumber"]
            .as_i64()
            .unwrap_or_else(|| panic!("entry[{i}].objectNumber must be an integer"));
        let _ = object_number; // parseable is the only assertion needed

        let routine_name = entry["routineName"]
            .as_str()
            .unwrap_or_else(|| panic!("entry[{i}].routineName must be a string"));
        assert!(
            !routine_name.is_empty(),
            "entry[{i}].routineName must not be empty"
        );
    }

    // (c) Determinism: running inventory-only twice yields identical output.
    let mut inv_opts2 = base_opts(&ws);
    inv_opts2.inventory_only = true;
    let inv_result2 = run_fingerprint_pipeline(&inv_opts2)
        .unwrap_or_else(|e| panic!("second inventory-only run error: {e}"));
    let inv_json2 = match inv_result2.output {
        FingerprintOutput::Text(t) => t,
        _ => panic!("expected Text from second inventory-only fingerprint"),
    };
    assert_eq!(
        inv_json, inv_json2,
        "inventory-only output must be deterministic (two runs must be byte-identical)"
    );
}

/// Exit code must be 0 for a valid workspace.
#[test]
fn inventory_only_exit_code_zero() {
    let ws = corpus_dir().join(FIXTURE);
    assert!(
        ws.is_dir(),
        "fixture {FIXTURE} not found at {}",
        ws.display()
    );
    let mut opts = base_opts(&ws);
    opts.inventory_only = true;
    let result = run_fingerprint_pipeline(&opts).expect("pipeline");
    assert_eq!(
        result.exit_code, 0,
        "inventory-only must exit 0 for valid workspace"
    );
}

/// `--inventory-only` with `--format cbor` must be rejected (json only).
#[test]
fn inventory_only_cbor_rejected() {
    use al_sem::engine::l5::fingerprint_cli::{
        SpecifiedFlags, default_format, reject_illegal_combos,
    };
    // Simulate: --inventory-only --format cbor (no query flags).
    let fmt = default_format(Some("cbor"), false).expect("cbor is a valid format");
    let specified = SpecifiedFlags::default();
    // rejectIllegalCombos must not reject for cbor alone (existing behavior),
    // but the CLI layer must reject --inventory-only + cbor.
    // We test this via the CLI combo-validator that will be added.
    let _ = reject_illegal_combos(specified, &fmt, false); // existing path: ok
    // The new rejection is in run_fingerprint_pipeline when inventory_only + non-json.
    // Test via the pipeline directly.
    let ws = corpus_dir().join(FIXTURE);
    assert!(ws.is_dir());
    let opts = FingerprintOptions {
        workspace: &ws,
        driver_version: "p1-test-v1",
        format: FingerprintFormat::Cbor,
        out: None,
        shard: None,
        witness_limit: None,
        roots: None,
        routine_selectors: Vec::new(),
        include_inherited: true,
        is_query_requested: false,
        deterministic: true,
        strict: false,
        verbosity: "compact",
        inventory_only: true,
        no_roots_config: false,
    };
    // Must return Err (rejected combo).
    let result = run_fingerprint_pipeline(&opts);
    assert!(
        result.is_err(),
        "--inventory-only + cbor must be rejected by the pipeline"
    );
}

// ===========================================================================
// engine-e2: enclosingMember / originatingObject inventory fields + 3-key sort.
//
// Builds a scratch workspace (NOT a corpus fixture — the differential suites
// enumerate `r0-corpus/` and would expect goldens for a new dir). Exercises the
// real `build_inventory_doc` path via `run_fingerprint_pipeline --inventory-only`.
// ===========================================================================

/// A scratch workspace under the OS temp dir. The `TempDir` removes it on drop
/// (panics included), so keep it alive for the whole test.
fn scratch_ws(tag: &str) -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::Builder::new()
        .prefix(&format!("alsem-cli-p1-inv-{tag}-"))
        .tempdir()
        .expect("create scratch ws");
    let dir = tmp.path().to_path_buf();
    std::fs::create_dir_all(dir.join("src")).expect("create scratch ws src dir");
    (tmp, dir)
}

/// Write a minimal one-app workspace (app.json + a single .al file) and run the
/// inventory-only pipeline, returning the parsed envelope.
fn inventory_doc_for(tag: &str, al_file: &str, al_source: &str) -> serde_json::Value {
    let (_tmp, ws) = scratch_ws(tag);
    std::fs::write(
        ws.join("app.json"),
        r#"{"id":"22222222-2222-2222-2222-222222222222","name":"E2 Inv","publisher":"PT","version":"1.0.0.0","dependencies":[]}"#,
    )
    .expect("write app.json");
    std::fs::write(ws.join("src").join(al_file), al_source).expect("write al source");

    let opts = FingerprintOptions {
        workspace: &ws,
        driver_version: "p1-test-v1",
        format: FingerprintFormat::Json,
        out: None,
        shard: None,
        witness_limit: None,
        roots: None,
        routine_selectors: Vec::new(),
        include_inherited: true,
        is_query_requested: false,
        deterministic: true,
        strict: false,
        verbosity: "compact",
        inventory_only: true,
        no_roots_config: false,
    };
    let result = run_fingerprint_pipeline(&opts)
        .unwrap_or_else(|e| panic!("inventory pipeline error for {tag}: {e}"));
    let json = match result.output {
        FingerprintOutput::Text(t) => t,
        _ => panic!("expected Text output"),
    };
    serde_json::from_str(&json).expect("inventory doc must parse")
}

const TWO_FIELD_TABLE: &str = r#"
table 50100 "Two Field"
{
    fields
    {
        field(1; "Bravo Field"; Integer)
        {
            trigger OnValidate()
            begin
            end;
        }
        field(2; "alpha field"; Integer)
        {
            trigger OnValidate()
            begin
            end;
        }
    }

    trigger OnInsert()
    begin
    end;
}
"#;

/// ⟨task 4⟩ Two field OnValidate triggers now carry DISTINCT `stableRoutineId`
/// (the member is folded into it) as well as distinct `enclosingMember`, and the
/// rows are emitted in deterministic `locale_compare(stableRoutineId)` order.
///
/// **This test's premise was inverted deliberately.** It used to assert the two
/// rows *share* one `stableRoutineId` and are ordered by the case-insensitive
/// `enclosingMember` tie-break — a recording of the collapse Task 4 closed. The
/// tie-break itself is KEPT and still asserted below on the tie it can still see;
/// it is now fail-closed cover for the residual duplicate-stable-id shapes (same
/// member name at different XMLport nesting paths; `#if`/`#else` alternatives of
/// one member — measured: 15 groups on BC Base App, 0 on DO), not the ordinary
/// case. Compare d1's `edge_target_matches_callsite_callee`, kept for the same
/// residual.
#[test]
fn two_field_rows_have_distinct_stable_ids_and_deterministic_order() {
    let doc = inventory_doc_for("two-field", "twofield.al", TWO_FIELD_TABLE);
    let rows = doc["payload"]["routineInventory"]
        .as_array()
        .expect("routineInventory array");

    // Collect the two OnValidate rows (same routineName "OnValidate").
    let validate_rows: Vec<&serde_json::Value> = rows
        .iter()
        .filter(|r| r["routineName"].as_str() == Some("OnValidate"))
        .collect();
    assert_eq!(
        validate_rows.len(),
        2,
        "two OnValidate rows expected, got {validate_rows:?}"
    );

    // DISTINCT stableRoutineId — the Task-4 fold. Their findings therefore get
    // distinct fingerprints and are independently baseline-able.
    let sid0 = validate_rows[0]["stableRoutineId"].as_str().unwrap();
    let sid1 = validate_rows[1]["stableRoutineId"].as_str().unwrap();
    assert_ne!(
        sid0, sid1,
        "the two field OnValidate rows must NOT share a stableRoutineId"
    );
    // Both keep the `{stableObjectId}#{64 lowercase hex}` SHAPE — the property
    // `alsem diff`'s join key and `stable_sub_id`'s split depend on.
    for sid in [sid0, sid1] {
        let (obj, hash) = sid.rsplit_once('#').expect("stable id is `{obj}#{hash}`");
        assert!(!obj.contains('#'), "exactly one `#` in {sid}");
        assert_eq!(hash.len(), 64, "hash part stays 64 bytes in {sid}");
        assert!(
            hash.bytes().all(al_sem::engine::ids::is_lower_hex),
            "hash part stays lowercase hex in {sid}"
        );
    }

    // Distinct enclosingMember, present on both.
    let m0 = validate_rows[0]["enclosingMember"]
        .as_str()
        .expect("row 0 enclosingMember present");
    let m1 = validate_rows[1]["enclosingMember"]
        .as_str()
        .expect("row 1 enclosingMember present");
    assert_ne!(m0, m1, "the two rows must carry distinct enclosingMember");

    // Deterministic order: the PRIMARY key (locale_compare on stableRoutineId) now
    // decides, since the two ids differ. Recomputed from the emitted ids rather than
    // hardcoded, so the assertion pins the SORT RULE and not one hash's accident.
    assert_eq!(
        al_sem::engine::ids::locale_compare(sid0, sid1),
        std::cmp::Ordering::Less,
        "inventory rows must be ordered by locale_compare(stableRoutineId)"
    );

    // The case-insensitive `enclosingMember` tie-break (RE-6) is KEPT for the
    // residual duplicate-stable-id shapes but can no longer be reached through this
    // fixture. Its own non-vacuous pin lives next to the comparator, in
    // `snapshot_full::tests::member_tie_break_is_case_insensitive_and_none_first`.

    // originatingObject present on the member-trigger rows (the declaring table).
    assert!(
        validate_rows[0]["originatingObject"].is_string(),
        "member-trigger row carries originatingObject"
    );

    // The object-level OnInsert trigger row has NO enclosingMember key.
    let oninsert = rows
        .iter()
        .find(|r| r["routineName"].as_str() == Some("OnInsert"))
        .expect("OnInsert row present");
    assert!(
        oninsert.get("enclosingMember").is_none(),
        "object-level OnInsert row must NOT carry an enclosingMember key"
    );
    assert!(
        oninsert.get("originatingObject").is_none(),
        "object-level OnInsert row must NOT carry an originatingObject key"
    );

    // Determinism: a second run is byte-identical.
    let doc2 = inventory_doc_for("two-field-2", "twofield.al", TWO_FIELD_TABLE);
    assert_eq!(
        doc["payload"]["routineInventory"], doc2["payload"]["routineInventory"],
        "inventory routine rows must be deterministic across runs"
    );
}

const OBJECT_LEVEL_CODEUNIT: &str = r#"
codeunit 50101 "Runner E2"
{
    trigger OnRun()
    begin
    end;

    procedure Helper()
    begin
    end;
}
"#;

/// An object-level trigger (OnRun) and a plain procedure carry NEITHER
/// enclosingMember NOR originatingObject keys.
#[test]
fn object_level_trigger_row_has_no_member_keys() {
    let doc = inventory_doc_for("obj-level", "runner.al", OBJECT_LEVEL_CODEUNIT);
    let rows = doc["payload"]["routineInventory"]
        .as_array()
        .expect("routineInventory array");

    for name in &["OnRun", "Helper"] {
        let row = rows
            .iter()
            .find(|r| r["routineName"].as_str() == Some(name))
            .unwrap_or_else(|| panic!("{name} row present"));
        assert!(
            row.get("enclosingMember").is_none(),
            "{name} (object-level / procedure) must NOT carry enclosingMember"
        );
        assert!(
            row.get("originatingObject").is_none(),
            "{name} (object-level / procedure) must NOT carry originatingObject"
        );
    }

    // schemaVersion is 1.1.0 on this doc too.
    assert_eq!(doc["schemaVersion"].as_str(), Some("1.1.0"));
}

/// `--inventory-only` with a query selector (e.g. `--routine`) must be REJECTED.
/// Without this, a selector makes `is_query_requested` true, which routes the
/// pipeline past the B0 inventory branch into the QUERY path — silently ignoring
/// `--inventory-only` and emitting a query result instead.
#[test]
fn inventory_only_query_selector_rejected() {
    let ws = corpus_dir().join(FIXTURE);
    assert!(
        ws.is_dir(),
        "fixture {FIXTURE} not found at {}",
        ws.display()
    );
    let opts = FingerprintOptions {
        workspace: &ws,
        driver_version: "p1-test-v1",
        format: FingerprintFormat::Json,
        out: None,
        shard: None,
        witness_limit: None,
        roots: None,
        routine_selectors: vec!["SomeRoutine".to_string()],
        include_inherited: true,
        // A --routine selector sets is_query_requested true at the CLI layer.
        is_query_requested: true,
        deterministic: true,
        strict: false,
        verbosity: "compact",
        inventory_only: true,
        no_roots_config: false,
    };
    // FingerprintRunResult is not Debug, so match the Result rather than unwrap_err().
    let msg = match run_fingerprint_pipeline(&opts) {
        Err(m) => m,
        Ok(_) => panic!("--inventory-only + query selector must be rejected by the pipeline"),
    };
    assert!(
        msg.contains("query selectors"),
        "rejection message must mention query selectors, got: {msg}"
    );
}

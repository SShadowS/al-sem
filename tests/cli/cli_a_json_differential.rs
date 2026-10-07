//! Stage A2 — `--format json` DocumentEnvelope differential test.
//!
//! For each fixture in the corpus, runs the Rust gate pipeline under
//! `--format json --deterministic` with `ALCH_DRIVER_VERSION_OVERRIDE=cli-a-json-v1`
//! and byte-compares the output against the vendored (Rust-owned) goldens at
//! `tests/cli-a-goldens/json/<fixture>.<slot>.json` — originally sourced from
//! al-sem's `scripts/cli-a-goldens/json/`, now retired.
//!
//! Two goldens per fixture:
//!   - `.default.json` — DEFAULT_DETECTORS (34 detectors).
//!   - `.all.json`     — ALL_DETECTORS (41 detectors, including opt-ins).
//!
//! ## Acceptance gate
//! All 20 × 2 = 40 goldens MUST byte-match; a divergence is a bug to fix, not
//! something to tolerate.
//!
//! ## Refresh
//! Goldens are Rust-owned baselines (the al-sem TS oracle is retired).
//! Rebaseline with `REGEN_TEMP_GOLDENS=1 cargo test --test cli cli_a_json_differential::`.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use al_sem::engine::gate::filter::Scope;
use al_sem::engine::gate::run::{AnalyzeArgs, OutputFormat, run_analyze_with_exit};

use crate::regen;

const TEST_NAME: &str = "cli_a_json_differential";

/// The version pin used for all JSON golden captures.
const JSON_VERSION_OVERRIDE: &str = "cli-a-json-v1";

/// Serialises the tests that mutate the process-global env var so they do not
/// race under cargo's parallel test threads.
static ENV_LOCK: Mutex<()> = Mutex::new(());

/// al-sem `DEFAULT_DETECTORS` names, in al-sem's declaration order.
const DEFAULT_DETECTOR_NAMES: &[&str] = &[
    "d1-db-op-in-loop",
    "d2-event-fanout-in-loop",
    "d3-missing-setloadfields",
    "d4-repeated-lookup-in-loop",
    "d5-set-based-opportunity",
    "d7-recursive-event-expansion",
    "d8-commit-in-transaction",
    "d9-transaction-span-summary",
    "d10-self-modifying-loop",
    "d11-modify-without-get",
    "d12-dead-integration-event",
    "d13-cross-app-internal-call",
    "d14-dead-routine",
    "d16-obsolete-routine-call",
    "d17-min-version-drift",
    "d18-constant-filter-in-loop",
    "d19-unused-parameter",
    "d20-unreachable-after-exit",
    "d21-read-without-load",
    "d22-flowfield-without-calcfields",
    "d29-subscriber-modify-on-event-record",
    "d32-constant-boolean-parameter",
    "d33-unfiltered-bulk-write",
    "d34-commit-in-loop",
    "d35-commit-in-event-subscriber",
    "d36-late-setloadfields",
    "d37-validate-without-persist",
    "d38-subscriber-to-obsolete-event",
    "d39-record-left-dirty-across-chain",
    "d41-transitive-filter-loss",
    "d42-cross-call-wrong-setloadfields",
    "d43-event-ishandled-skip",
    "d44-event-multi-subscriber-overlap",
    "d45-event-transitive-table-exposure",
];

/// The 20 fixtures under test (same corpus as stats + SARIF differentials).
const FIXTURES: &[&str] = &[
    "ws-d8-commit-in-tx",
    "ws-d34",
    "ws-d35",
    "ws-txn-d46-pos",
    "ws-txn-d47-pos-http-nocommit",
    "ws-txn-d47-pos-http-commit-after",
    "ws-txn-d47-pos-file",
    "ws-txn-d48-pos",
    "ws-txn-d49-pos-modify-message",
    "ws-txn-d49-pos-modify-runmodal",
    "ws-d51-pos",
    "ws-d51-jobqueue",
    "ws-txn-d46-neg",
    "ws-txn-d47-neg-readonly",
    "ws-txn-d47-neg-temp",
    "ws-txn-d48-neg",
    "ws-txn-d49-neg-no-write",
    "ws-d51-neg",
    "ws-d1-multi-caller",
    "ws-d14-dead-routine",
];

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn corpus_dir() -> PathBuf {
    repo_root().join("tests").join("r0-corpus")
}

/// In-repo vendored cli-a json golden corpus — the SOLE source. Originally only
/// the rebaselined-vs-al-sem fixtures lived here with the rest falling back to a
/// frozen al-sem archive; that fallback was retired (Task 3.6, al-sem parity
/// retirement) once the corpus was fully vendored, since the fallback checked a
/// path that no longer exists on disk and silently skipped every assertion.
fn local_json_dir() -> PathBuf {
    repo_root().join("tests").join("cli-a-goldens").join("json")
}

/// Resolve a golden by name against the in-repo vendored corpus.
fn resolve_golden(name: &str) -> PathBuf {
    local_json_dir().join(name)
}

/// Build detector string for `--detector` flag from a names slice.
fn detector_arg(names: &[&str]) -> String {
    names.join(",")
}

/// REGEN path (temp-state epoch rebaseline, Task 16; iter-2 gap rebaseline).
/// When `REGEN_TEMP_GOLDENS` is set, reconcile the golden against the ENGINE output
/// — the goldens are Rust-owned baselines (TS oracle retired). The write target is
/// the in-repo vendored dir (`local_json_dir()/<name>`); we write ONLY when the
/// engine output differs from the existing baseline, leaving an already-matching
/// golden untouched. Returns `true` when in regen mode (caller skips the assert).
fn maybe_regen(name: &str, rust: &str) -> bool {
    if !regen::regen_mode() {
        return false;
    }
    let resolved = resolve_golden(name);
    let baseline = std::fs::read_to_string(&resolved).ok();
    if baseline.as_deref() == Some(rust) {
        return true; // already byte-matches the resolved baseline — no vendoring needed
    }
    let dir = local_json_dir();
    std::fs::create_dir_all(&dir).unwrap_or_else(|e| panic!("regen mkdir {}: {e}", dir.display()));
    let golden_path = dir.join(name);
    std::fs::write(&golden_path, rust)
        .unwrap_or_else(|e| panic!("regen write {}: {e}", golden_path.display()));
    eprintln!(
        "REGEN cli-a-json vendored golden: {}",
        golden_path.display()
    );
    true
}

/// Run the Rust JSON pipeline for one fixture with the given detector list.
/// The env var `ALCH_DRIVER_VERSION_OVERRIDE` MUST be set by the caller before
/// this function is called (it reads the env at call time via `driver_version()`).
fn run_json(fixture: &str, detector_csv: &str) -> String {
    let fixture_dir = corpus_dir().join(fixture);
    assert!(
        fixture_dir.is_dir(),
        "{TEST_NAME}: fixture {fixture} not found at {}",
        fixture_dir.display()
    );
    let args = AnalyzeArgs {
        workspace: fixture_dir.to_string_lossy().to_string(),
        min_severity: None,
        detector: Some(detector_csv.to_string()),
        preset: None,
        scope: Scope::Primary,
        limit: None,
        format: OutputFormat::Json,
        sarif_version_override: None,
        fail_on: None,
        require_dependencies: false,
        baseline: None,
        update_baseline: false,
        disable_inline_suppression: false,
        group_by: None,
        deterministic: true,
        with_evidence: false,
        single_app: false,
    };
    // The pipeline returns (output, exit_code, warning); we only need the output.
    // The trailing newline is appended by the bin — add it here to match the golden.
    match run_analyze_with_exit(&args, "engine-default") {
        Ok((out, _, _)) => format!("{out}\n"),
        Err(e) => panic!("{TEST_NAME}: run_analyze failed for {fixture}: {e}"),
    }
}

/// Obtain the all-detectors CSV (all 41 detectors in registry order).
fn all_detector_csv() -> String {
    use al_sem::engine::l5::detectors::registered_detectors;
    registered_detectors()
        .into_iter()
        .map(|d| d.name)
        .collect::<Vec<_>>()
        .join(",")
}

/// Diff helper: produce a human-readable delta between two JSON strings
/// (golden vs rust). Tries to parse both; if either fails, falls back to
/// first-differing-line.
fn json_diff(fixture: &str, slot: &str, golden: &str, rust: &str) -> String {
    let gv: serde_json::Value = match serde_json::from_str(golden) {
        Ok(v) => v,
        Err(e) => return format!("golden JSON parse error: {e}"),
    };
    let rv: serde_json::Value = match serde_json::from_str(rust) {
        Ok(v) => v,
        Err(e) => return format!("rust JSON parse error: {e}"),
    };

    if gv == rv {
        // Byte mismatch but same JSON — whitespace/ordering issue only.
        let gl: Vec<&str> = golden.lines().collect();
        let rl: Vec<&str> = rust.lines().collect();
        for (i, (g, r)) in gl.iter().zip(rl.iter()).enumerate() {
            if g != r {
                return format!(
                    "[{fixture}/{slot}] byte mismatch at line {} (JSON values equal):\n  golden: {g}\n  rust:   {r}",
                    i + 1
                );
            }
        }
        format!("[{fixture}/{slot}] byte mismatch (different lengths, JSON values equal)")
    } else {
        // JSON values differ — produce a path-level diff.
        diff_values(fixture, slot, "", &gv, &rv)
    }
}

fn diff_values(
    fixture: &str,
    slot: &str,
    path: &str,
    gv: &serde_json::Value,
    rv: &serde_json::Value,
) -> String {
    match (gv, rv) {
        (serde_json::Value::Object(go), serde_json::Value::Object(ro)) => {
            let mut parts = Vec::new();
            for (k, gval) in go {
                let p = if path.is_empty() {
                    k.clone()
                } else {
                    format!("{path}.{k}")
                };
                match ro.get(k) {
                    Some(rval) if rval != gval => {
                        parts.push(diff_values(fixture, slot, &p, gval, rval));
                    }
                    None => parts.push(format!(
                        "[{fixture}/{slot}] .{p}: in golden, missing in rust"
                    )),
                    _ => {}
                }
            }
            for k in ro.keys() {
                if !go.contains_key(k) {
                    let p = if path.is_empty() {
                        k.clone()
                    } else {
                        format!("{path}.{k}")
                    };
                    parts.push(format!(
                        "[{fixture}/{slot}] .{p}: not in golden, present in rust"
                    ));
                }
            }
            if parts.is_empty() {
                format!("[{fixture}/{slot}] .{path}: objects differ (no field-level diff found)")
            } else {
                parts.join("\n")
            }
        }
        (serde_json::Value::Array(ga), serde_json::Value::Array(ra)) => {
            if ga.len() != ra.len() {
                return format!(
                    "[{fixture}/{slot}] .{path}[]: golden len={} rust len={}",
                    ga.len(),
                    ra.len()
                );
            }
            let mut parts = Vec::new();
            for (i, (gval, rval)) in ga.iter().zip(ra.iter()).enumerate() {
                if gval != rval {
                    let p = format!("{path}[{i}]");
                    parts.push(diff_values(fixture, slot, &p, gval, rval));
                }
            }
            if parts.is_empty() {
                format!("[{fixture}/{slot}] .{path}[]: arrays differ (no element-level diff)")
            } else {
                parts.join("\n")
            }
        }
        _ => format!(
            "[{fixture}/{slot}] .{path}: golden={} rust={}",
            serde_json::to_string(gv).unwrap_or_default(),
            serde_json::to_string(rv).unwrap_or_default()
        ),
    }
}

// ---------------------------------------------------------------------------
// Main byte-match test
// ---------------------------------------------------------------------------

#[test]
fn cli_a_json_byte_match() {
    let _env = crate::env_guard();
    let all_csv = all_detector_csv();
    let default_csv = detector_arg(DEFAULT_DETECTOR_NAMES);

    let mut divergences: Vec<String> = Vec::new();

    // Serialize env access across all sub-runs (ALCH_DRIVER_VERSION_OVERRIDE is process-global).
    let _guard = ENV_LOCK.lock().unwrap();
    // TODO: Audit that the environment access only happens in single-threaded code.
    unsafe { std::env::set_var("ALCH_DRIVER_VERSION_OVERRIDE", JSON_VERSION_OVERRIDE) };

    for &fixture in FIXTURES {
        for (slot, csv) in &[("default", default_csv.as_str()), ("all", all_csv.as_str())] {
            let name = format!("{fixture}.{slot}.json");
            let golden_path = resolve_golden(&name);
            let rust_out = run_json(fixture, csv);
            if maybe_regen(&name, &rust_out) {
                continue;
            }
            if !golden_path.exists() {
                divergences.push(format!(
                    "[{fixture}/{slot}] golden file missing: {}",
                    golden_path.display()
                ));
                continue;
            }
            let golden = std::fs::read_to_string(&golden_path).unwrap_or_else(|e| {
                panic!("{TEST_NAME}: failed to read {}: {e}", golden_path.display())
            });

            if rust_out != golden {
                let diff = json_diff(fixture, slot, &golden, &rust_out);
                divergences.push(diff);
            }
        }
    }

    // TODO: Audit that the environment access only happens in single-threaded code.
    unsafe { std::env::remove_var("ALCH_DRIVER_VERSION_OVERRIDE") };

    if !divergences.is_empty() {
        let mut msg = format!("{TEST_NAME}: {} divergence(s) found:\n", divergences.len());
        for d in &divergences {
            msg.push_str(&format!("  {d}\n"));
        }
        panic!("{msg}");
    }
}

// ---------------------------------------------------------------------------
// Anti-degenerate oracles
// ---------------------------------------------------------------------------

/// ws-txn-d46-neg (canonical 0-findings fixture) must produce diagnostics:[],
/// payload.findings:[], and bySeverity:{}, byDetector:{}.
#[test]
fn zero_findings_fixture_has_empty_maps() {
    let _env = crate::env_guard();
    let default_csv = detector_arg(DEFAULT_DETECTOR_NAMES);

    let _guard = ENV_LOCK.lock().unwrap();
    // TODO: Audit that the environment access only happens in single-threaded code.
    unsafe { std::env::set_var("ALCH_DRIVER_VERSION_OVERRIDE", JSON_VERSION_OVERRIDE) };
    let out = run_json("ws-txn-d46-neg", &default_csv);
    // TODO: Audit that the environment access only happens in single-threaded code.
    unsafe { std::env::remove_var("ALCH_DRIVER_VERSION_OVERRIDE") };

    let v: serde_json::Value =
        serde_json::from_str(&out).expect("zero-findings output must be valid JSON");

    assert_eq!(
        v["diagnostics"].as_array().map(|a| a.len()).unwrap_or(99),
        0,
        "ws-txn-d46-neg must have 0 diagnostics"
    );
    assert_eq!(
        v["payload"]["findings"]
            .as_array()
            .map(|a| a.len())
            .unwrap_or(99),
        0,
        "ws-txn-d46-neg must have 0 findings"
    );
    let by_sev = &v["payload"]["summary"]["bySeverity"];
    assert_eq!(
        by_sev,
        &serde_json::json!({}),
        "ws-txn-d46-neg bySeverity must be empty object"
    );
    let by_det = &v["payload"]["summary"]["byDetector"];
    assert_eq!(
        by_det,
        &serde_json::json!({}),
        "ws-txn-d46-neg byDetector must be empty object"
    );
}

/// Envelope fields for any fixture must match the pinned contract values.
#[test]
fn envelope_fields_are_correct() {
    let _env = crate::env_guard();
    let fixture_dir = corpus_dir().join("ws-txn-d46-neg");
    assert!(
        fixture_dir.is_dir(),
        "{TEST_NAME}: ws-txn-d46-neg fixture missing at {}",
        fixture_dir.display()
    );
    let default_csv = detector_arg(DEFAULT_DETECTOR_NAMES);

    let _guard = ENV_LOCK.lock().unwrap();
    // TODO: Audit that the environment access only happens in single-threaded code.
    unsafe { std::env::set_var("ALCH_DRIVER_VERSION_OVERRIDE", JSON_VERSION_OVERRIDE) };
    let out = run_json("ws-txn-d46-neg", &default_csv);
    // TODO: Audit that the environment access only happens in single-threaded code.
    unsafe { std::env::remove_var("ALCH_DRIVER_VERSION_OVERRIDE") };

    let v: serde_json::Value = serde_json::from_str(&out).expect("output must be valid JSON");

    assert_eq!(v["kind"], "analyze-report");
    assert_eq!(v["schemaVersion"], "1.0.0");
    assert_eq!(v["alsemVersion"], JSON_VERSION_OVERRIDE);
    assert_eq!(v["deterministic"], true);
    assert_eq!(v["generatedAt"], "1970-01-01T00:00:00Z");
}

// ---------------------------------------------------------------------------
// Native oracles for the diagnostics projection + threading (corpus-invisible)
// ---------------------------------------------------------------------------

/// (a) Direct `project_diagnostics` oracle: each `Diagnostic{severity,stage,message}`
/// projects to EXACTLY `{code:"DIAG-<stage>", severity, message}` — no anchor /
/// subject / sourceRef leaks through. Covers stages parse/discover/detect and
/// severities error/warning/info.
#[test]
fn project_diagnostics_shape_oracle() {
    use al_sem::engine::gate::format_json::project_diagnostics;
    use al_sem::engine::l5::registry::Diagnostic;

    let diags = vec![
        Diagnostic {
            severity: "error".to_string(),
            stage: "parse".to_string(),
            message: "boom".to_string(),
        },
        Diagnostic {
            severity: "warning".to_string(),
            stage: "discover".to_string(),
            message: "multi-app".to_string(),
        },
        Diagnostic {
            severity: "info".to_string(),
            stage: "detect".to_string(),
            message: "guard".to_string(),
        },
    ];

    let projected = project_diagnostics(&diags);
    let arr = projected.as_array().expect("diagnostics is an array");
    assert_eq!(arr.len(), 3, "all three diagnostics projected, in order");

    // Element 0: parse/error/boom.
    assert_eq!(arr[0]["code"], "DIAG-parse");
    assert_eq!(arr[0]["severity"], "error");
    assert_eq!(arr[0]["message"], "boom");
    // Element 1: discover/warning.
    assert_eq!(arr[1]["code"], "DIAG-discover");
    assert_eq!(arr[1]["severity"], "warning");
    assert_eq!(arr[1]["message"], "multi-app");
    // Element 2: detect/info.
    assert_eq!(arr[2]["code"], "DIAG-detect");
    assert_eq!(arr[2]["severity"], "info");
    assert_eq!(arr[2]["message"], "guard");

    // Exactly three keys per element — NO anchor / subject / sourceRef.
    for el in arr {
        let obj = el.as_object().expect("each diagnostic is an object");
        let mut keys: Vec<&str> = obj.keys().map(|s| s.as_str()).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            vec!["code", "message", "severity"],
            "diagnostic must carry ONLY code/message/severity (no anchor/subject/sourceRef)"
        );
    }
}

/// Create a unique scratch dir for a fail-closed / malformed workspace oracle.
/// The `TempDir` removes it on drop (panics included); keep it alive for the test.
fn scratch_ws(tag: &str) -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::Builder::new()
        .prefix(&format!("alsem-cli-a-json-{tag}-"))
        .tempdir()
        .expect("create scratch ws dir");
    let dir = tmp.path().to_path_buf();
    (tmp, dir)
}

/// Run the JSON pipeline directly over an arbitrary workspace path (not a corpus
/// fixture). Caller sets `ALCH_DRIVER_VERSION_OVERRIDE` and holds `ENV_LOCK`.
fn run_json_path(ws: &Path, detector_csv: &str) -> String {
    let args = AnalyzeArgs {
        workspace: ws.to_string_lossy().to_string(),
        min_severity: None,
        detector: Some(detector_csv.to_string()),
        preset: None,
        scope: Scope::Primary,
        limit: None,
        format: OutputFormat::Json,
        sarif_version_override: None,
        fail_on: None,
        require_dependencies: false,
        baseline: None,
        update_baseline: false,
        disable_inline_suppression: false,
        group_by: None,
        deterministic: true,
        with_evidence: false,
        single_app: false,
    };
    match run_analyze_with_exit(&args, "engine-default") {
        Ok((out, _, _)) => format!("{out}\n"),
        Err(e) => panic!("{TEST_NAME}: run_analyze failed for {}: {e}", ws.display()),
    }
}

/// (b) Fail-closed oracle: a MULTI-APP workspace (two `app.json` files) produces
/// an empty model, but its envelope `diagnostics` MUST contain the provider error
/// projected as `DIAG-discover`. This proves `empty_output_result` now threads the
/// real provider diagnostics (it previously hardcoded `diagnostics:&[]` — so before
/// the fix this assertion FAILS: the array was empty).
#[test]
fn fail_closed_multi_app_emits_discover_diagnostic() {
    let _env = crate::env_guard();
    let (_tmp, ws) = scratch_ws("multiapp");
    // Root app.json WITH a valid id (so the only fail-closed trigger is multi-app).
    std::fs::write(
        ws.join("app.json"),
        r#"{"id":"11111111-1111-1111-1111-111111111111","name":"A","publisher":"P","version":"1.0.0.0"}"#,
    )
    .unwrap();
    // A NESTED second app.json → multi-app fail-closed.
    let sub = ws.join("sub");
    std::fs::create_dir_all(&sub).unwrap();
    std::fs::write(
        sub.join("app.json"),
        r#"{"id":"22222222-2222-2222-2222-222222222222","name":"B","publisher":"P","version":"1.0.0.0"}"#,
    )
    .unwrap();
    // One .al file so the workspace looks real.
    std::fs::write(ws.join("Foo.al"), "codeunit 50100 Foo { }").unwrap();

    let default_csv = detector_arg(DEFAULT_DETECTOR_NAMES);
    let _guard = ENV_LOCK.lock().unwrap();
    // TODO: Audit that the environment access only happens in single-threaded code.
    unsafe { std::env::set_var("ALCH_DRIVER_VERSION_OVERRIDE", JSON_VERSION_OVERRIDE) };
    let out = run_json_path(&ws, &default_csv);
    // TODO: Audit that the environment access only happens in single-threaded code.
    unsafe { std::env::remove_var("ALCH_DRIVER_VERSION_OVERRIDE") };

    let v: serde_json::Value = serde_json::from_str(&out).expect("valid JSON");
    let diags = v["diagnostics"].as_array().expect("diagnostics array");
    assert!(
        !diags.is_empty(),
        "fail-closed multi-app MUST emit ≥1 diagnostic (empty_output_result must thread provider diagnostics)"
    );
    let has_multi_app_discover = diags.iter().any(|d| {
        d["code"] == "DIAG-discover"
            && d["severity"] == "error"
            && d["message"]
                .as_str()
                .is_some_and(|m| m.contains("multi-app source workspace unsupported"))
    });
    assert!(
        has_multi_app_discover,
        "expected a DIAG-discover error for the multi-app workspace; got {diags:?}"
    );
    // Empty model ⇒ zero findings.
    assert_eq!(
        v["payload"]["findings"].as_array().map(|a| a.len()),
        Some(0),
        "fail-closed workspace yields zero findings"
    );
}

/// (b') Fail-closed oracle: an id-LESS root `app.json` (readable JSON, no `id`)
/// also fails closed with a `DIAG-discover` error.
#[test]
fn fail_closed_idless_app_json_emits_discover_diagnostic() {
    let _env = crate::env_guard();
    let (_tmp, ws) = scratch_ws("idless");
    std::fs::write(
        ws.join("app.json"),
        r#"{"name":"A","publisher":"P","version":"1.0.0.0"}"#,
    )
    .unwrap();
    std::fs::write(ws.join("Foo.al"), "codeunit 50100 Foo { }").unwrap();

    let default_csv = detector_arg(DEFAULT_DETECTOR_NAMES);
    let _guard = ENV_LOCK.lock().unwrap();
    // TODO: Audit that the environment access only happens in single-threaded code.
    unsafe { std::env::set_var("ALCH_DRIVER_VERSION_OVERRIDE", JSON_VERSION_OVERRIDE) };
    let out = run_json_path(&ws, &default_csv);
    // TODO: Audit that the environment access only happens in single-threaded code.
    unsafe { std::env::remove_var("ALCH_DRIVER_VERSION_OVERRIDE") };

    let v: serde_json::Value = serde_json::from_str(&out).expect("valid JSON");
    let diags = v["diagnostics"].as_array().expect("diagnostics array");
    let has_idless_discover = diags.iter().any(|d| {
        d["code"] == "DIAG-discover"
            && d["severity"] == "error"
            && d["message"]
                .as_str()
                .is_some_and(|m| m.contains("has no string `id`"))
    });
    assert!(
        has_idless_discover,
        "expected a DIAG-discover error for the id-less root app.json; got {diags:?}"
    );
}

/// (c) Malformed-`.al` oracle: a SOUND workspace (valid root app.json) whose single
/// `.al` file contains NO object declaration surfaces a `DIAG-index` "No object
/// declaration found" diagnostic (al-sem `indexer.ts:56-63`). This is the
/// cheaply-reachable index-stage diagnostic. (The model is then empty ⇒ fail-closed
/// empty output, but the index diagnostic is preserved.)
#[test]
fn no_object_declaration_emits_index_diagnostic() {
    let _env = crate::env_guard();
    let (_tmp, ws) = scratch_ws("noobj");
    std::fs::write(
        ws.join("app.json"),
        r#"{"id":"33333333-3333-3333-3333-333333333333","name":"A","publisher":"P","version":"1.0.0.0"}"#,
    )
    .unwrap();
    // A .al file with NO object declaration (just a comment) → "No object declaration found".
    std::fs::write(ws.join("Empty.al"), "// just a comment, no object\n").unwrap();

    let default_csv = detector_arg(DEFAULT_DETECTOR_NAMES);
    let _guard = ENV_LOCK.lock().unwrap();
    // TODO: Audit that the environment access only happens in single-threaded code.
    unsafe { std::env::set_var("ALCH_DRIVER_VERSION_OVERRIDE", JSON_VERSION_OVERRIDE) };
    let out = run_json_path(&ws, &default_csv);
    // TODO: Audit that the environment access only happens in single-threaded code.
    unsafe { std::env::remove_var("ALCH_DRIVER_VERSION_OVERRIDE") };

    let v: serde_json::Value = serde_json::from_str(&out).expect("valid JSON");
    let diags = v["diagnostics"].as_array().expect("diagnostics array");
    let has_index_diag = diags.iter().any(|d| {
        d["code"] == "DIAG-index"
            && d["message"]
                .as_str()
                .is_some_and(|m| m.contains("No object declaration found in Empty.al"))
    });
    assert!(
        has_index_diag,
        "expected a DIAG-index 'No object declaration found' diagnostic; got {diags:?}"
    );
}

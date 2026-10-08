//! cli-a detectorStats differential test.
//!
//! For each fixture in `tests/r0-corpus/<fixture>`, runs the Rust L5 pipeline
//! over both the `default` (34 detectors) and `all` (41 detectors) slots and
//! byte-matches the serialized `detectorStats` array against the vendored
//! (Rust-owned) goldens at `tests/cli-a-goldens/stats/<fixture>.<slot>.json` —
//! originally sourced from al-sem's `scripts/cli-a-goldens/stats/`, now retired.
//!
//! ## Acceptance gate
//!
//! All 20 × 2 = 40 goldens MUST byte-match. Any divergence that is a genuine
//! TS/engine model difference (not a Rust bug) is reported as BLOCKED — do NOT
//! tolerate it; block the work item instead.
//!
//! ## Refresh (ignored)
//!
//! `#[ignore]` re-baseline test writes the Rust output into the in-repo vendored
//! goldens directory. Only run explicitly when intentionally updating the goldens.

use std::path::PathBuf;

use al_sem::engine::l5::detectors::registered_detectors;
use al_sem::engine::l5::registry::{run_detectors, serialize_detector_stats};
use al_sem::program::model::program_calls::assemble_and_resolve_workspace_program;
use al_sem::program::model::workspace::MODEL_INSTANCE_ID_DEFAULT;

use crate::regen;

const TEST_NAME: &str = "cli_a_stats_differential";

/// al-sem's DEFAULT_DETECTORS (34) by name, in registration order.
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

/// The 20 fixtures under test.
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

/// In-repo vendored cli-a stats golden corpus — the SOLE source. Originally only
/// the rebaselined-vs-al-sem fixtures lived here with the rest falling back to a
/// frozen al-sem archive; that fallback was retired (Task 3.6, al-sem parity
/// retirement) once the corpus was fully vendored (mirrors json/html/terminal).
fn local_stats_dir() -> PathBuf {
    repo_root()
        .join("tests")
        .join("cli-a-goldens")
        .join("stats")
}

/// Resolve a golden by name against the in-repo vendored corpus.
fn resolve_golden(name: &str) -> PathBuf {
    local_stats_dir().join(name)
}

/// REGEN path (iter-2 gap rebaseline). When `REGEN_TEMP_GOLDENS` is set, reconcile
/// the golden against the ENGINE output — the goldens are Rust-owned baselines (TS
/// oracle retired). The write target is the in-repo vendored dir
/// (`local_stats_dir()/<name>`); we write ONLY when the engine output differs from
/// the existing baseline, leaving an already-matching golden untouched. Returns
/// `true` when in regen mode (caller skips the assert).
fn maybe_regen(name: &str, rust: &str) -> bool {
    if !regen::regen_mode() {
        return false;
    }
    let resolved = resolve_golden(name);
    let baseline = std::fs::read_to_string(&resolved).ok();
    if baseline.as_deref() == Some(rust) {
        return true; // already byte-matches the resolved baseline — no vendoring needed
    }
    let dir = local_stats_dir();
    std::fs::create_dir_all(&dir).unwrap_or_else(|e| panic!("regen mkdir {}: {e}", dir.display()));
    let golden_path = dir.join(name);
    std::fs::write(&golden_path, rust)
        .unwrap_or_else(|e| panic!("regen write {}: {e}", golden_path.display()));
    eprintln!(
        "REGEN cli-a-stats vendored golden: {}",
        golden_path.display()
    );
    true
}

/// Run the Rust stats pipeline for one fixture, filtered to the given detector names
/// (preserving registry order).
fn run_stats(fixture: &str, names: &[&str]) -> String {
    let fixture_dir = corpus_dir().join(fixture);
    assert!(
        fixture_dir.is_dir(),
        "{TEST_NAME}: fixture {fixture} not found at {}",
        fixture_dir.display()
    );
    let all_detectors = registered_detectors();
    // Filter to `names`, preserving registry order (mirrors al-sem `select_detectors`).
    let name_set: std::collections::HashSet<&str> = names.iter().copied().collect();
    let selected: Vec<_> = all_detectors
        .into_iter()
        .filter(|d| name_set.contains(d.name.as_str()))
        .collect();

    match assemble_and_resolve_workspace_program(&fixture_dir, MODEL_INSTANCE_ID_DEFAULT, false) {
        Some(resolved) => {
            let output = run_detectors(&resolved, &selected);
            serialize_detector_stats(&output.detector_stats)
        }
        None => {
            // Workspace assembly failed — produce a stats array of empty stats for
            // each selected detector (mirrors al-sem's behaviour on an empty/invalid
            // workspace: each detector sees 0 candidates and emits 0 findings).
            use al_sem::engine::l5::registry::DetectorStats;
            let stats: Vec<DetectorStats> = selected
                .iter()
                .map(|d| DetectorStats::new(d.name.as_str(), 0, 0))
                .collect();
            serialize_detector_stats(&stats)
        }
    }
}

/// Struct for divergence reporting.
struct Divergence {
    fixture: String,
    slot: String,
    message: String,
}

#[test]
fn cli_a_stats_byte_match() {
    let all_names: Vec<&str> = {
        let dets = registered_detectors();
        dets.into_iter()
            .map(|d| -> &'static str {
                // Leak to get a 'static str — acceptable in test code.
                Box::leak(d.name.into_boxed_str())
            })
            .collect()
    };

    let mut divergences: Vec<Divergence> = Vec::new();

    for &fixture in FIXTURES {
        for (slot, names) in &[
            ("default", DEFAULT_DETECTOR_NAMES as &[&str]),
            ("all", all_names.as_slice()),
        ] {
            let name = format!("{fixture}.{slot}.json");
            let rust_out = run_stats(fixture, names);
            if maybe_regen(&name, &rust_out) {
                continue;
            }
            let golden_path = resolve_golden(&name);
            if !golden_path.exists() {
                divergences.push(Divergence {
                    fixture: fixture.to_string(),
                    slot: slot.to_string(),
                    message: format!("golden file missing: {}", golden_path.display()),
                });
                continue;
            }
            let golden = std::fs::read_to_string(&golden_path).unwrap_or_else(|e| {
                panic!("{TEST_NAME}: failed to read {}: {e}", golden_path.display())
            });

            if rust_out != golden {
                // Report field-level diff for easier debugging.
                let golden_val: serde_json::Value = serde_json::from_str(&golden)
                    .unwrap_or(serde_json::Value::String(golden.clone()));
                let rust_val: serde_json::Value = serde_json::from_str(&rust_out)
                    .unwrap_or(serde_json::Value::String(rust_out.clone()));
                let diff = diff_stats(&golden_val, &rust_val);
                divergences.push(Divergence {
                    fixture: fixture.to_string(),
                    slot: slot.to_string(),
                    message: diff,
                });
            }
        }
    }

    if !divergences.is_empty() {
        let mut msg = format!("{TEST_NAME}: {} divergence(s) found:\n", divergences.len());
        for d in &divergences {
            msg.push_str(&format!("  [{} / {}]: {}\n", d.fixture, d.slot, d.message));
        }
        panic!("{msg}");
    }
}

/// Produce a human-readable diff summary for two `detectorStats` arrays.
fn diff_stats(golden: &serde_json::Value, rust: &serde_json::Value) -> String {
    let ga = match golden.as_array() {
        Some(a) => a,
        None => return format!("golden is not an array: {golden}"),
    };
    let ra = match rust.as_array() {
        Some(a) => a,
        None => return format!("rust is not an array: {rust}"),
    };
    if ga.len() != ra.len() {
        return format!("array length: golden={} rust={}", ga.len(), ra.len());
    }
    let mut parts: Vec<String> = Vec::new();
    for (i, (gv, rv)) in ga.iter().zip(ra.iter()).enumerate() {
        if gv != rv {
            let gdet = gv.get("detector").and_then(|v| v.as_str()).unwrap_or("?");
            // Report field-level differences within the object.
            if let (Some(go), Some(ro)) = (gv.as_object(), rv.as_object()) {
                for (k, gval) in go {
                    match ro.get(k) {
                        Some(rval) if rval != gval => {
                            parts.push(format!(
                                "[{i}:{gdet}].{k}: golden={} rust={}",
                                compact(gval),
                                compact(rval)
                            ));
                        }
                        None => {
                            parts.push(format!(
                                "[{i}:{gdet}].{k}: golden={} rust=<missing>",
                                compact(gval)
                            ));
                        }
                        _ => {}
                    }
                }
                for (k, rval) in ro {
                    if !go.contains_key(k) {
                        parts.push(format!(
                            "[{i}:{gdet}].{k}: golden=<missing> rust={}",
                            compact(rval)
                        ));
                    }
                }
            } else {
                parts.push(format!(
                    "[{i}]: golden={} rust={}",
                    compact(gv),
                    compact(rv)
                ));
            }
        }
    }
    if parts.is_empty() {
        "no field-level diff found (byte diff only)".to_string()
    } else {
        parts.join("; ")
    }
}

fn compact(v: &serde_json::Value) -> String {
    serde_json::to_string(v).unwrap_or_else(|_| format!("{v:?}"))
}

/// Refresh test — writes Rust output into the in-repo vendored goldens dir. Run
/// with: cargo test --test cli cli_a_stats_differential::refresh -- --ignored
#[test]
#[ignore]
fn refresh() {
    let stats_dir = local_stats_dir();
    std::fs::create_dir_all(&stats_dir).expect("create stats_dir");

    let all_names: Vec<&str> = {
        let dets = registered_detectors();
        dets.into_iter()
            .map(|d| -> &'static str { Box::leak(d.name.into_boxed_str()) })
            .collect()
    };

    for &fixture in FIXTURES {
        for (slot, names) in &[
            ("default", DEFAULT_DETECTOR_NAMES as &[&str]),
            ("all", all_names.as_slice()),
        ] {
            let out = run_stats(fixture, names);
            let golden_path = stats_dir.join(format!("{fixture}.{slot}.json"));
            std::fs::write(&golden_path, &out).unwrap_or_else(|e| {
                panic!("refresh: failed to write {}: {e}", golden_path.display())
            });
            eprintln!("refresh: wrote {}", golden_path.display());
        }
    }
}

//! R4-F Stage-4 — scoped-guarantees stable-projection differential.
//!
//! For each committed al-sem golden under
//! `tests/r4f-goldens/<fixture>.scoped.golden.json`, run the Rust source-only
//! L0→L3 pass over the matching `tests/r0-corpus/<fixture>` workspace, compose the
//! CapabilitySnapshot, compute return summaries + isolated event ids, run the
//! digest witness + effects + occurrence-build + ORDERING-ENGINE path
//! (`project_r4f_scoped_guarantees`), pretty-serialize (serde_json pretty +
//! trailing newline — the exact on-disk golden form), and assert BYTE-equality.
//!
//! ## Anti-degenerate
//!
//! - Each of the 5 RELEVANT labels appears >=1 across the corpus.
//! - The negatives have entryCount 0.
//! - ws-txn-d49-neg-run-boundary has WRITE_PENDING_AT_UI with interveningBoundary "unknown".

use std::collections::HashSet;
use std::path::PathBuf;

use al_sem::engine::l3::program_calls::assemble_and_resolve_workspace_with_program_calls;
use al_sem::engine::l5::digest::project_r4f_scoped_guarantees;

use crate::regen;

/// The R4-F scoped-guarantees corpus (mirrors al-sem `R4F_SCOPED_GUARANTEE_FIXTURES`).
const FIXTURES: &[&str] = &[
    "ws-txn-d47-pos-http-nocommit",
    "ws-txn-d47-pos-http-commit-after",
    "ws-txn-d47-pos-file",
    "ws-txn-d47-crosshop-iobeforecommit",
    "ws-txn-d47-advisory-deduped",
    "ws-txn-d47-advisory-post-nowrite",
    "ws-txn-d47-event-pos",
    "ws-txn-d47-event-neg-isolated",
    "ws-txn-d47-neg-commit-between",
    "ws-txn-d47-neg-temp",
    "ws-txn-d47-neg-readonly",
    "ws-txn-d49-pos-modify-message",
    "ws-txn-d49-pos-modify-runmodal",
    "ws-txn-d49-neg-run-boundary",
    "ws-txn-d49-neg-commit-between",
    "ws-d51-pos",
    "ws-d51-jobqueue",
    "ws-d51-neg",
];

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn goldens_dir() -> PathBuf {
    repo_root().join("tests").join("r4f-goldens")
}

fn corpus_dir() -> PathBuf {
    repo_root().join("tests").join("r0-corpus")
}

fn run_rust(fixture: &str) -> String {
    let fixture_dir = corpus_dir().join(fixture);
    assert!(
        fixture_dir.is_dir(),
        "R4-F scoped golden for {fixture} has no matching in-repo fixture at {} \
         (offline corpus incomplete)",
        fixture_dir.display()
    );
    match assemble_and_resolve_workspace_with_program_calls(&fixture_dir) {
        Some(resolved) => project_r4f_scoped_guarantees(&resolved, fixture),
        None => format!(
            "{{\n  \"fixtureName\": \"{fixture}\",\n  \"entryCount\": 0,\n  \"entries\": []\n}}\n"
        ),
    }
}

fn run_rust_value(fixture: &str) -> serde_json::Value {
    serde_json::from_str(&run_rust(fixture)).expect("projection is valid JSON")
}

#[test]
fn r4f_scoped_guarantees_matches_goldens() {
    let mut failures: Vec<String> = Vec::new();
    for fixture in FIXTURES {
        let golden_path = goldens_dir().join(format!("{fixture}.scoped.golden.json"));
        let rust_text = run_rust(fixture);

        // REGEN path (Task T0.6 — this family previously had none). When
        // `REGEN_TEMP_GOLDENS=1`, write the ENGINE output straight to the golden
        // file instead of comparing — the goldens are Rust-owned baselines (TS
        // oracle retired). `run_rust` already returns the exact on-disk pretty +
        // trailing-newline form the assert path below reads.
        if regen::regen_mode() {
            std::fs::write(&golden_path, &rust_text)
                .unwrap_or_else(|e| panic!("regen write {}: {e}", golden_path.display()));
            eprintln!(
                "REGEN r4f scoped-guarantees golden: {}",
                golden_path.display()
            );
            continue;
        }

        let golden_text = std::fs::read_to_string(&golden_path).unwrap_or_else(|e| {
            panic!(
                "cannot read R4-F scoped golden {}: {e}",
                golden_path.display()
            )
        });
        if rust_text != golden_text {
            failures.push(format!(
                "MISMATCH {fixture}:\n--- GOLDEN ---\n{golden_text}\n--- RUST ---\n{rust_text}"
            ));
        }
    }
    if regen::regen_mode() {
        eprintln!(
            "REGEN r4f scoped-guarantees: wrote {} golden(s)",
            FIXTURES.len()
        );
        return;
    }

    assert!(
        failures.is_empty(),
        "R4-F ACCEPTANCE GATE: {} fixture(s) did NOT byte-match:\n{}",
        failures.len(),
        failures.join("\n\n")
    );
}

// ---------------------------------------------------------------------------
// Anti-degenerate guards
// ---------------------------------------------------------------------------

#[test]
fn anti_degenerate_all_five_labels_present() {
    let mut labels: HashSet<String> = HashSet::new();
    for fixture in FIXTURES {
        let proj = run_rust_value(fixture);
        for entry in proj.get("entries").and_then(|v| v.as_array()).unwrap() {
            for eff in entry.get("effects").and_then(|v| v.as_array()).unwrap() {
                for sg in eff
                    .get("scopedGuarantees")
                    .and_then(|v| v.as_array())
                    .unwrap()
                {
                    if let Some(l) = sg.get("label").and_then(|v| v.as_str()) {
                        labels.insert(l.to_string());
                    }
                }
            }
        }
    }
    for expected in [
        "WRITE_PENDING_AT_EXTERNAL_IO",
        "EXTERNAL_IO_BEFORE_COMMIT",
        "WRITE_PENDING_AT_UI",
        "IO_BEFORE_ESCAPING_ERROR",
        "EXTERNAL_IO_IN_EVENT_SUBSCRIBER_TXN",
    ] {
        assert!(
            labels.contains(expected),
            "expected label {expected} to appear >=1 across the corpus; saw {labels:?}"
        );
    }
}

#[test]
fn anti_degenerate_negatives_are_empty() {
    for fixture in [
        "ws-txn-d47-neg-temp",
        "ws-txn-d47-neg-readonly",
        "ws-txn-d47-neg-commit-between",
        "ws-txn-d47-event-neg-isolated",
        "ws-txn-d49-neg-commit-between",
        "ws-d51-neg",
    ] {
        let proj = run_rust_value(fixture);
        assert_eq!(
            proj.get("entryCount").and_then(|v| v.as_u64()),
            Some(0),
            "{fixture} must have entryCount 0 (negative)"
        );
    }
}

#[test]
fn anti_degenerate_d49_run_boundary_unknown() {
    // ws-txn-d49-neg-run-boundary: WRITE_PENDING_AT_UI root scope with
    // interveningBoundary "unknown".
    let proj = run_rust_value("ws-txn-d49-neg-run-boundary");
    let mut found = false;
    for entry in proj.get("entries").and_then(|v| v.as_array()).unwrap() {
        for eff in entry.get("effects").and_then(|v| v.as_array()).unwrap() {
            for sg in eff
                .get("scopedGuarantees")
                .and_then(|v| v.as_array())
                .unwrap()
            {
                if sg.get("label").and_then(|v| v.as_str()) == Some("WRITE_PENDING_AT_UI")
                    && sg.get("scope").and_then(|v| v.as_str()) == Some("root")
                    && sg.get("interveningBoundary").and_then(|v| v.as_str()) == Some("unknown")
                {
                    found = true;
                }
            }
        }
    }
    assert!(
        found,
        "ws-txn-d49-neg-run-boundary must carry WRITE_PENDING_AT_UI@root interveningBoundary=unknown"
    );
}

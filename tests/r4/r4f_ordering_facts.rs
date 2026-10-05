//! R4-F Stage-5b — ordering-facts M5 stable-projection differential.
//!
//! For each committed al-sem golden under
//! `tests/r4f-goldens/<fixture>.orderingfacts.golden.json`, run the Rust source-only
//! L0→L3 pass over the matching `tests/r0-corpus/<fixture>` workspace, run the
//! ordering-facts facade (`compute_ordering_facts`: composeSnapshot → return
//! summaries → isolated events → digest+ordering → resolve each scopedGuarantee to
//! its IO/write/commit anchors), project (`project_r4f_ordering_facts`), and assert
//! BYTE-equality (serde_json pretty + trailing newline — the on-disk golden form).
//!
//! ## Anti-degenerate
//! - The positives carry ≥1 resolved fact; the negatives carry routineCount 0.
//! - The empty write-occurrence segment (`||`) sorts BEFORE a `|hex|` segment
//!   (the M8 localeCompare collation, verified by the multi-fact positives).

use std::path::PathBuf;

use al_sem::engine::l3::program_calls::assemble_and_resolve_workspace_with_program_calls;
use al_sem::engine::l5::ordering_facts::project_r4f_ordering_facts;

use crate::regen;

/// The R4-F ordering-facts corpus (21 fixtures, mirroring the M5 goldens).
const FIXTURES: &[&str] = &[
    "ws-txn-d47-pos-http-nocommit",
    "ws-txn-d47-pos-http-commit-after",
    "ws-txn-d47-pos-file",
    "ws-txn-d47-crosshop-iobeforecommit",
    "ws-txn-d47-advisory-deduped",
    "ws-txn-d47-advisory-post-nowrite",
    "ws-txn-d47-event-pos",
    "ws-txn-d47-event-neg-clean",
    "ws-txn-d47-event-neg-isolated",
    "ws-txn-d47-neg-commit-between",
    "ws-txn-d47-neg-readonly",
    "ws-txn-d47-neg-temp",
    "ws-txn-d49-pos-modify-message",
    "ws-txn-d49-pos-modify-runmodal",
    "ws-txn-d49-neg-commit-between",
    "ws-txn-d49-neg-no-write",
    "ws-txn-d49-neg-run-boundary",
    "ws-txn-d49-neg-temp-write",
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
        "R4-F ordering-facts golden for {fixture} has no matching in-repo fixture at {} \
         (offline corpus incomplete)",
        fixture_dir.display()
    );
    match assemble_and_resolve_workspace_with_program_calls(&fixture_dir) {
        Some(resolved) => project_r4f_ordering_facts(&resolved, fixture),
        None => format!(
            "{{\n  \"fixtureName\": \"{fixture}\",\n  \"routineCount\": 0,\n  \"entries\": []\n}}\n"
        ),
    }
}

fn run_rust_value(fixture: &str) -> serde_json::Value {
    serde_json::from_str(&run_rust(fixture)).expect("projection is valid JSON")
}

#[test]
fn r4f_ordering_facts_matches_goldens() {
    let mut failures: Vec<String> = Vec::new();
    for fixture in FIXTURES {
        let golden_path = goldens_dir().join(format!("{fixture}.orderingfacts.golden.json"));
        let rust_text = run_rust(fixture);

        // REGEN path (Task T0.6 — this family previously had none). When
        // `REGEN_TEMP_GOLDENS=1`, write the ENGINE output straight to the golden
        // file instead of comparing — the goldens are Rust-owned baselines (TS
        // oracle retired). `run_rust` already returns the exact on-disk pretty +
        // trailing-newline form the assert path below reads.
        if regen::regen_mode() {
            std::fs::write(&golden_path, &rust_text)
                .unwrap_or_else(|e| panic!("regen write {}: {e}", golden_path.display()));
            eprintln!("REGEN r4f ordering-facts golden: {}", golden_path.display());
            continue;
        }

        let golden_text = std::fs::read_to_string(&golden_path).unwrap_or_else(|e| {
            panic!(
                "cannot read R4-F ordering-facts golden {}: {e}",
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
            "REGEN r4f ordering-facts: wrote {} golden(s)",
            FIXTURES.len()
        );
        return;
    }

    assert!(
        failures.is_empty(),
        "R4-F M5 ACCEPTANCE GATE: {} fixture(s) did NOT byte-match:\n{}",
        failures.len(),
        failures.join("\n\n")
    );
}

#[test]
fn anti_degenerate_negatives_are_empty() {
    for fixture in [
        "ws-txn-d47-event-neg-clean",
        "ws-txn-d47-event-neg-isolated",
        "ws-txn-d47-neg-commit-between",
        "ws-txn-d47-neg-readonly",
        "ws-txn-d47-neg-temp",
        "ws-txn-d49-neg-commit-between",
        "ws-txn-d49-neg-no-write",
        "ws-txn-d49-neg-temp-write",
        "ws-d51-neg",
    ] {
        let proj = run_rust_value(fixture);
        assert_eq!(
            proj.get("routineCount").and_then(|v| v.as_u64()),
            Some(0),
            "{fixture} must have routineCount 0 (negative)"
        );
    }
}

#[test]
fn anti_degenerate_positives_have_facts() {
    for fixture in [
        "ws-txn-d47-pos-http-nocommit",
        "ws-txn-d47-pos-file",
        "ws-txn-d47-event-pos",
        "ws-txn-d49-pos-modify-message",
        "ws-d51-pos",
        "ws-d51-jobqueue",
    ] {
        let proj = run_rust_value(fixture);
        let entries = proj.get("entries").and_then(|v| v.as_array()).unwrap();
        let total_facts: usize = entries
            .iter()
            .map(|e| {
                e.get("facts")
                    .and_then(|f| f.as_array())
                    .map_or(0, |a| a.len())
            })
            .sum();
        assert!(
            total_facts >= 1,
            "{fixture} must carry >=1 resolved ordering fact"
        );
    }
}

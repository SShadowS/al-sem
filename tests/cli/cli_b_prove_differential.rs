//! cli-b/b2 — the PROVE CLI differential.
//!
//! For each (fixture, routine, question) in the hardcoded `PROVE_CORPUS` const (which
//! originally mirrored al-sem's `scripts/cli-b-goldens/prove/manifest.json` entries;
//! that oracle is now retired), runs `run_prove_pipeline` with `deterministic:true`
//! and version `cli-b-v1`, and byte-compares the `.json` and `.human.txt` goldens
//! vendored (Rust-owned) at `tests/cli-b-goldens/prove/`.
//!
//! Additionally, verifies the dummy-doc case (ws-d8-commit-in-tx, NonExistentRoutineXYZ)
//! exits with code 2.
//!
//! ## Acceptance gate
//!
//! All 18 × 2 = 36 regular goldens + the 2 dummy-doc goldens (= 38 total) MUST
//! byte-match. This is ungated: divergence is either a Rust bug to fix or a model
//! difference to BLOCK — never something to tolerate.
//!
//! ## Refresh
//!
//! Goldens are Rust-owned baselines (the al-sem TS oracle is retired).
//! Rebaseline with `REGEN_TEMP_GOLDENS=1 cargo test --test cli cli_b_prove_differential::`.

use std::path::PathBuf;

use al_sem::engine::l5::prove::run_prove_pipeline;

use crate::regen;

const VERSION_OVERRIDE: &str = "cli-b-v1";

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// In-repo fixtures (Rust-owned; al-sem byte-parity retired — see CLAUDE.md).
fn fixtures_dir() -> PathBuf {
    repo_root().join("tests").join("r0-corpus")
}

/// In-repo Rust-owned goldens, regenerated via `REGEN_TEMP_GOLDENS=1`.
fn goldens_dir() -> PathBuf {
    repo_root()
        .join("tests")
        .join("cli-b-goldens")
        .join("prove")
}

/// When `REGEN_TEMP_GOLDENS` is set, write the golden (Rust-owned baseline) and
/// return true so the caller skips the byte-compare. al-sem byte-parity retired.
fn regen_golden(golden_path: &std::path::Path, got: &str) -> bool {
    if !regen::regen_mode() {
        return false;
    }
    if let Some(parent) = golden_path.parent() {
        std::fs::create_dir_all(parent)
            .unwrap_or_else(|e| panic!("regen mkdir {}: {e}", parent.display()));
    }
    std::fs::write(golden_path, got)
        .unwrap_or_else(|e| panic!("regen write {}: {e}", golden_path.display()));
    true
}

/// (fixture, golden_slug, routine, question, expected_exit_code)
const PROVE_CORPUS: &[(&str, &str, &str, &str, u8)] = &[
    // may-commit
    (
        "ws-d34",
        "ws-d34.may-commit",
        "DirectCommitInLoop",
        "may-commit",
        0,
    ),
    (
        "ws-d14-dead-routine",
        "ws-d14-dead-routine.may-commit",
        "OnRun",
        "may-commit",
        0,
    ),
    (
        "ws-d35",
        "ws-d35.may-commit",
        "OnBeforePrintSafe",
        "may-commit",
        0,
    ),
    // commits-on-success-path
    (
        "ws-d8-commit-in-tx",
        "ws-d8-commit-in-tx.commits-on-success-path",
        "HandlePosted",
        "commits-on-success-path",
        0,
    ),
    (
        "ws-d14-dead-routine",
        "ws-d14-dead-routine.commits-on-success-path",
        "OnRun",
        "commits-on-success-path",
        0,
    ),
    (
        "ws-d34",
        "ws-d34.commits-on-success-path",
        "DirectCommitInLoop",
        "commits-on-success-path",
        0,
    ),
    // writes-table
    (
        "ws-d1-multi-caller",
        "ws-d1-multi-caller.writes-table-mc-customer",
        "CallerA",
        "writes-table:MC Customer",
        0,
    ),
    (
        "ws-d34",
        "ws-d34.writes-table-customer",
        "DirectCommitInLoop",
        "writes-table:Customer",
        0,
    ),
    (
        "ws-d35",
        "ws-d35.writes-table-customer",
        "OnBeforePrintSafe",
        "writes-table:Customer",
        0,
    ),
    // publishes-event
    (
        "ws-d8-commit-in-tx",
        "ws-d8-commit-in-tx.publishes-event-onafterpostsalesdoc",
        "PostSalesDoc",
        "publishes-event:onafterpostsalesdoc",
        0,
    ),
    (
        "ws-d34",
        "ws-d34.publishes-event-onafterpost",
        "DirectCommitInLoop",
        "publishes-event:OnAfterPost",
        0,
    ),
    (
        "ws-d35",
        "ws-d35.publishes-event-onafterpost",
        "OnBeforePrintSafe",
        "publishes-event:OnAfterPost",
        0,
    ),
    // reaches-ui
    (
        "ws-d35",
        "ws-d35.reaches-ui",
        "OnBeforePrintSafe",
        "reaches-ui",
        0,
    ),
    (
        "ws-d34",
        "ws-d34.reaches-ui",
        "DirectCommitInLoop",
        "reaches-ui",
        0,
    ),
    (
        "ws-txn-d47-pos-http-nocommit",
        "ws-txn-d47-pos-http-nocommit.reaches-ui",
        "SendAfterModify",
        "reaches-ui",
        0,
    ),
    // throws-error
    (
        "ws-d51-pos",
        "ws-d51-pos.throws-error",
        "PostThenError",
        "throws-error",
        0,
    ),
    (
        "ws-d34",
        "ws-d34.throws-error",
        "DirectCommitInLoop",
        "throws-error",
        0,
    ),
    (
        "ws-d35",
        "ws-d35.throws-error",
        "OnBeforePrintSafe",
        "throws-error",
        0,
    ),
    // dummy-doc case (exit 2)
    (
        "ws-d8-commit-in-tx",
        "dummy-doc.may-commit",
        "NonExistentRoutineXYZ",
        "may-commit",
        2,
    ),
];

// ---------------------------------------------------------------------------
// Diagnostic helpers
// ---------------------------------------------------------------------------

fn first_diff(a: &[u8], b: &[u8]) -> Option<usize> {
    let n = a.len().min(b.len());
    for i in 0..n {
        if a[i] != b[i] {
            return Some(i);
        }
    }
    if a.len() != b.len() { Some(n) } else { None }
}

fn context_around(bytes: &[u8], pos: usize, radius: usize) -> String {
    let start = pos.saturating_sub(radius);
    let end = (pos + radius).min(bytes.len());
    String::from_utf8_lossy(&bytes[start..end])
        .replace('\n', "↵")
        .replace('\r', "↩")
}

// ---------------------------------------------------------------------------
// Per-entry runner
// ---------------------------------------------------------------------------

fn run_one(fixture: &str, routine: &str, question: &str) -> Result<(String, String, u8), String> {
    let fixture_dir = fixtures_dir().join(fixture);
    if !fixture_dir.is_dir() {
        return Err(format!(
            "fixture {fixture} not found at {}",
            fixture_dir.display()
        ));
    }

    match run_prove_pipeline(
        &al_sem::program::inputs::WorkspaceInputs::read(&fixture_dir, &crate::fingerprint_needs()),
        routine,
        question,
        VERSION_OVERRIDE,
        true,  // deterministic
        false, // cross-app, the default: no cli-b fixture has a dependency
    ) {
        Ok(result) => Ok((result.json_text, result.human_text, result.exit_code)),
        Err(msg) => Err(msg),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn json_matches_goldens() {
    let gdir = goldens_dir();
    for (fixture, golden_slug, routine, question, expected_exit) in PROVE_CORPUS {
        let (got_json, _, actual_exit) = run_one(fixture, routine, question).unwrap_or_else(|e| {
            panic!("{fixture}/{routine}/{question}: run_prove_pipeline failed: {e}")
        });

        assert_eq!(
            actual_exit, *expected_exit,
            "{fixture}/{routine}/{question}: exit code mismatch: got {actual_exit}, want {expected_exit}"
        );

        let golden_path = gdir.join(format!("{golden_slug}.json"));
        if regen_golden(&golden_path, &got_json) {
            continue;
        }
        let want = std::fs::read_to_string(&golden_path).unwrap_or_else(|_| {
            panic!(
                "{fixture}/{routine}/{question}: golden not found at {}",
                golden_path.display()
            )
        });

        if got_json != want {
            let got_b = got_json.as_bytes();
            let want_b = want.as_bytes();
            let pos = first_diff(got_b, want_b).unwrap_or(0);
            let got_ctx = context_around(got_b, pos, 120);
            let want_ctx = context_around(want_b, pos, 120);
            panic!(
                "{fixture}/{routine}/{question}: JSON golden mismatch at byte {pos}\n  got  (±120): {got_ctx:?}\n  want (±120): {want_ctx:?}\n  got len={}, want len={}",
                got_b.len(),
                want_b.len()
            );
        }
    }
}

#[test]
fn human_matches_goldens() {
    let gdir = goldens_dir();
    for (fixture, golden_slug, routine, question, _expected_exit) in PROVE_CORPUS {
        let (_, got_human, _) = run_one(fixture, routine, question).unwrap_or_else(|e| {
            panic!("{fixture}/{routine}/{question}: run_prove_pipeline failed: {e}")
        });

        let golden_path = gdir.join(format!("{golden_slug}.human.txt"));
        if regen_golden(&golden_path, &got_human) {
            continue;
        }
        let want = std::fs::read_to_string(&golden_path).unwrap_or_else(|_| {
            panic!(
                "{fixture}/{routine}/{question}: human golden not found at {}",
                golden_path.display()
            )
        });

        if got_human != want {
            let got_b = got_human.as_bytes();
            let want_b = want.as_bytes();
            let pos = first_diff(got_b, want_b).unwrap_or(0);
            let got_ctx = context_around(got_b, pos, 120);
            let want_ctx = context_around(want_b, pos, 120);
            panic!(
                "{fixture}/{routine}/{question}: human golden mismatch at byte {pos}\n  got  (±120): {got_ctx:?}\n  want (±120): {want_ctx:?}\n  got len={}, want len={}",
                got_b.len(),
                want_b.len()
            );
        }
    }
}

#[test]
fn dummy_doc_exits_2() {
    let fixture = "ws-d8-commit-in-tx";
    let routine = "NonExistentRoutineXYZ";
    let question = "may-commit";

    let (_, _, exit) = run_one(fixture, routine, question)
        .unwrap_or_else(|e| panic!("dummy-doc: run_prove_pipeline failed: {e}"));

    assert_eq!(exit, 2, "dummy-doc case must exit 2 (routine not resolved)");
}

// Rust-owned goldens regenerated in-process via
// `REGEN_TEMP_GOLDENS=1 cargo test --test cli cli_b_prove_differential::`.

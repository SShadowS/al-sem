//! cli-b/b1 — the full digest CLI differential.
//!
//! For each fixture in `DIGEST_CORPUS`, collect all `.al` source files in
//! `<fixture>/src/`, run `run_digest_pipeline` with `deterministic:true` and
//! version `cli-b-v1`, and byte-compare the `.json` and `.human.txt` goldens
//! vendored (Rust-owned) at `tests/cli-b-goldens/digest/` — originally sourced
//! from al-sem's `scripts/cli-b-goldens/digest/`, now retired.
//!
//! Additionally, for `ws-d8-commit-in-tx`, runs with the `.changed.diff` file
//! (from the same goldens directory) and compares the `.diff.json` golden.
//!
//! ## Acceptance gate
//!
//! All 20 × 2 = 40 regular goldens + the 1 diff golden MUST byte-match. This
//! is ungated: any divergence is either a Rust bug to fix or a genuine model
//! difference to BLOCK — never something to tolerate.
//!
//! ## Refresh
//!
//! Goldens are Rust-owned baselines (the al-sem TS oracle is retired).
//! Rebaseline with `REGEN_TEMP_GOLDENS=1 cargo test --test cli cli_b_digest_differential::`.

use std::path::{Path, PathBuf};

use al_sem::engine::l5::digest_cli::run_digest_pipeline;

use crate::regen;

const VERSION_OVERRIDE: &str = "cli-b-v1";

/// The digest corpus (same 20 fixtures as dump-digest.ts DIGEST_CORPUS).
pub(crate) const DIGEST_CORPUS: &[&str] = &[
    // transaction-integrity positives
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
    // opt-in ordering fixtures
    "ws-d51-pos",
    "ws-d51-jobqueue",
    // transaction-integrity negatives
    "ws-txn-d46-neg",
    "ws-txn-d47-neg-readonly",
    "ws-txn-d47-neg-temp",
    "ws-txn-d48-neg",
    "ws-txn-d49-neg-no-write",
    "ws-d51-neg",
    // non-txn positives
    "ws-d1-multi-caller",
    "ws-d14-dead-routine",
];

const DIFF_FIXTURE: &str = "ws-d8-commit-in-tx";

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
        .join("digest")
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

/// Collect all `.al` files in `<fixture>/src/`, sorted, workspace-relative.
/// Mirrors dump-digest.ts `collectAlFiles`.
pub(crate) fn collect_al_files(ws_dir: &Path) -> Vec<String> {
    let src_dir = ws_dir.join("src");
    if !src_dir.is_dir() {
        return Vec::new();
    }
    let mut files: Vec<String> = std::fs::read_dir(&src_dir)
        .expect("src dir readable")
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.path()
                .extension()
                .and_then(|x| x.to_str())
                .map(|x| x.eq_ignore_ascii_case("al"))
                .unwrap_or(false)
        })
        .map(|e| format!("src/{}", e.file_name().to_string_lossy()))
        .collect();
    files.sort();
    files
}

/// Find the first differing byte offset (for diagnostics).
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
// Per-fixture runner helpers
// ---------------------------------------------------------------------------

/// Run the full-files digest for one fixture and return (json_text, human_text).
fn run_files_digest(fixture: &str) -> (String, String) {
    let fixture_dir = fixtures_dir().join(fixture);
    assert!(
        fixture_dir.is_dir(),
        "cli-b digest fixture {fixture} not found at {}",
        fixture_dir.display()
    );

    let al_files = collect_al_files(&fixture_dir);
    assert!(
        !al_files.is_empty(),
        "{fixture}: no .al files found in src/ — fixture incomplete"
    );

    let result = run_digest_pipeline(
        &al_sem::program::inputs::InputSet::read(&fixture_dir, &Default::default()),
        Some(al_files),
        None, // no routine selectors
        None, // no diff
        VERSION_OVERRIDE,
        true,  // deterministic
        None,  // max_paths
        false, // cross-app, the CLI default (#66)
    )
    .unwrap_or_else(|e| panic!("{fixture}: run_digest_pipeline failed: {e}"));

    (result.json_text, result.human_text)
}

/// Run the --diff digest for the diff fixture and return json_text.
fn run_diff_digest(fixture: &str) -> String {
    let fixture_dir = fixtures_dir().join(fixture);
    assert!(
        fixture_dir.is_dir(),
        "cli-b digest fixture {fixture} not found at {}",
        fixture_dir.display()
    );

    let diff_path = goldens_dir().join(format!("{fixture}.changed.diff"));
    assert!(
        diff_path.is_file(),
        "diff file not found: {}",
        diff_path.display()
    );

    let diff_text = std::fs::read_to_string(&diff_path)
        .unwrap_or_else(|e| panic!("read diff file {}: {e}", diff_path.display()));

    let result = run_digest_pipeline(
        &al_sem::program::inputs::InputSet::read(&fixture_dir, &Default::default()),
        None, // no files
        None, // no routine selectors
        Some(diff_text),
        VERSION_OVERRIDE,
        true,  // deterministic
        None,  // max_paths
        false, // cross-app, the CLI default (#66)
    )
    .unwrap_or_else(|e| panic!("{fixture} (--diff): run_digest_pipeline failed: {e}"));

    result.json_text
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn json_matches_goldens() {
    for fixture in DIGEST_CORPUS {
        let (got_json, _) = run_files_digest(fixture);
        let golden_path = goldens_dir().join(format!("{fixture}.json"));
        if regen_golden(&golden_path, &got_json) {
            continue;
        }
        let want = std::fs::read_to_string(&golden_path)
            .unwrap_or_else(|_| panic!("{fixture}: golden not found at {}", golden_path.display()));

        if got_json != want {
            let got_b = got_json.as_bytes();
            let want_b = want.as_bytes();
            let pos = first_diff(got_b, want_b).unwrap_or(0);
            let got_ctx = context_around(got_b, pos, 120);
            let want_ctx = context_around(want_b, pos, 120);
            panic!(
                "{fixture}: JSON golden mismatch at byte {pos}\n  got  (±120): {got_ctx:?}\n  want (±120): {want_ctx:?}\n  got len={}, want len={}",
                got_b.len(),
                want_b.len()
            );
        }
    }
}

#[test]
fn human_matches_goldens() {
    for fixture in DIGEST_CORPUS {
        let (_, got_human) = run_files_digest(fixture);
        let golden_path = goldens_dir().join(format!("{fixture}.human.txt"));
        if regen_golden(&golden_path, &got_human) {
            continue;
        }
        let want = std::fs::read_to_string(&golden_path).unwrap_or_else(|_| {
            panic!(
                "{fixture}: human golden not found at {}",
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
                "{fixture}: human golden mismatch at byte {pos}\n  got  (±120): {got_ctx:?}\n  want (±120): {want_ctx:?}\n  got len={}, want len={}",
                got_b.len(),
                want_b.len()
            );
        }
    }
}

#[test]
fn diff_json_matches_golden() {
    let got = run_diff_digest(DIFF_FIXTURE);
    let golden_path = goldens_dir().join(format!("{DIFF_FIXTURE}.diff.json"));
    if regen_golden(&golden_path, &got) {
        return;
    }
    let want = std::fs::read_to_string(&golden_path).unwrap_or_else(|_| {
        panic!(
            "{DIFF_FIXTURE}: diff.json golden not found at {}",
            golden_path.display()
        )
    });

    if got != want {
        let got_b = got.as_bytes();
        let want_b = want.as_bytes();
        let pos = first_diff(got_b, want_b).unwrap_or(0);
        let got_ctx = context_around(got_b, pos, 120);
        let want_ctx = context_around(want_b, pos, 120);
        panic!(
            "{DIFF_FIXTURE} (--diff): JSON golden mismatch at byte {pos}\n  got  (±120): {got_ctx:?}\n  want (±120): {want_ctx:?}\n  got len={}, want len={}",
            got_b.len(),
            want_b.len()
        );
    }
}

// Rust-owned goldens regenerated in-process via
// `REGEN_TEMP_GOLDENS=1 cargo test --test cli cli_b_digest_differential::`.

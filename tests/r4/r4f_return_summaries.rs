//! R4-F — RETURN SUMMARIES stable-projection differential.
//!
//! For each committed al-sem golden under
//! `tests/r4f-goldens/<fixture>.returnsummary.golden.json`, run the Rust
//! source-only L0→L3 pass (`assemble_and_resolve_workspace_with_program_calls(...)`) over
//! the matching `tests/r0-corpus/<fixture>` workspace, compute the return
//! summaries (`project_r4f_return_summaries`), pretty-serialize (serde_json
//! pretty + trailing newline — the exact on-disk golden form), and assert
//! BYTE-equality.
//!
//! ## Anti-degenerate
//!
//! - `ws-d51-pos` MUST carry a summary with `allPathsError: true` AND
//!   `hasNormalReturnPath: false` (the always-error routine path, the
//!   canonical non-trivial case).

use std::path::PathBuf;

use al_sem::engine::return_summary::{R4FReturnSummaryProjection, project_r4f_return_summaries};
use al_sem::program::model::program_calls::assemble_and_resolve_workspace_with_program_calls;

use crate::regen;

/// The R4-F return-summaries corpus (8 fixtures).
const FIXTURES: &[&str] = &[
    "ws-d51-jobqueue",
    "ws-d51-pos",
    "ws-txn-d47-crosshop-iobeforecommit",
    "ws-txn-d47-event-pos",
    "ws-txn-d47-neg-commit-between",
    "ws-txn-d47-pos-http-commit-after",
    "ws-txn-d47-pos-http-nocommit",
    "ws-txn-d49-pos-modify-message",
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

/// Pretty-serialize + trailing newline — the exact on-disk golden form.
fn pretty_with_newline(proj: &R4FReturnSummaryProjection) -> String {
    let mut s =
        serde_json::to_string_pretty(proj).expect("serialize R4-F return-summary projection");
    s.push('\n');
    s
}

/// Run the Rust source-only L0→L3 pass + return-summary projection for one
/// fixture.
fn run_rust(fixture: &str) -> R4FReturnSummaryProjection {
    let fixture_dir = corpus_dir().join(fixture);
    assert!(
        fixture_dir.is_dir(),
        "R4-F return-summary golden for {fixture} has no matching in-repo fixture at {} \
         (offline corpus incomplete)",
        fixture_dir.display()
    );
    match assemble_and_resolve_workspace_with_program_calls(&fixture_dir) {
        Some(resolved) => project_r4f_return_summaries(&resolved, fixture),
        None => R4FReturnSummaryProjection {
            fixture_name: fixture.to_string(),
            summary_count: 0,
            summaries: vec![],
        },
    }
}

#[test]
fn r4f_return_summaries_match_goldens() {
    for fixture in FIXTURES {
        let golden_path = goldens_dir().join(format!("{fixture}.returnsummary.golden.json"));

        let projection = run_rust(fixture);
        let rust_text = pretty_with_newline(&projection);

        // REGEN path (Task T0.6 — this family previously had none). When
        // `REGEN_TEMP_GOLDENS=1`, write the ENGINE output straight to the golden
        // file instead of comparing — the goldens are Rust-owned baselines (TS
        // oracle retired). `pretty_with_newline` already produces the exact
        // on-disk form the assert path below reads.
        if regen::regen_mode() {
            std::fs::write(&golden_path, &rust_text)
                .unwrap_or_else(|e| panic!("regen write {}: {e}", golden_path.display()));
            eprintln!("REGEN r4f return-summary golden: {}", golden_path.display());
            continue;
        }

        let golden_text = std::fs::read_to_string(&golden_path).unwrap_or_else(|e| {
            panic!(
                "cannot read R4-F return-summary golden {}: {e}",
                golden_path.display()
            )
        });

        assert_eq!(
            rust_text,
            golden_text,
            "R4-F ACCEPTANCE GATE: {fixture} did NOT byte-match its golden ({})",
            golden_path.display()
        );
    }
}

#[test]
fn anti_degenerate_d51_pos_has_all_paths_error() {
    let projection = run_rust("ws-d51-pos");
    assert!(
        projection.summaries.iter().any(|s| {
            s.all_paths_error == serde_json::Value::Bool(true)
                && s.has_normal_return_path == serde_json::Value::Bool(false)
        }),
        "ws-d51-pos must produce a summary with allPathsError=true and \
         hasNormalReturnPath=false (always-error routine path)"
    );
}

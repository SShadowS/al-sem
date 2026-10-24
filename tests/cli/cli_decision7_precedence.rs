//! T9 (#277 spec D3, Decision 7): the policy load and fingerprint's
//! `--shard`/`--out` check run before any build. Three stated precedence
//! changes, one test each, spawned with the trace on:
//! 1. a bad policy AND a failing build: the policy error, `build.program` 0;
//! 2. `--shard` without `--out` AND a failing build: the shard/out error,
//!    `build.program` 0;
//! 3. `policy check --strict`, a bad policy AND an error-severity analyzer
//!    diagnostic: the policy error, exit 1 (before: the diagnostic lines).
//!
//! Each test first runs each failure alone, so both texts are known to differ.

use std::path::Path;
use std::process::Output;

use crate::cli_build_counters::{count, spawn_traced, unreadable_ws};

const APP_JSON: &str = r#"{"id":"11111111-1111-1111-1111-111111111111","name":"A","publisher":"p","version":"1.0.0.0"}"#;

/// A policy file that does not load.
fn bad_policy(dir: &Path) -> String {
    let p = dir.join("bad.policy.yaml");
    std::fs::write(&p, "version: [not, a, number\n").unwrap();
    p.to_string_lossy().into_owned()
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

#[test]
fn decision7_precedence_policy_and_build() {
    let Some((d, _g)) = unreadable_ws(APP_JSON) else {
        return;
    };
    let tmp = tempfile::tempdir().unwrap();
    let policy = bad_policy(tmp.path());
    let ws = d.path().to_str().unwrap();
    let good_ws = crate::cli_build_counters::ws_a();

    // Each failure alone.
    let (o, _) = spawn_traced(&["policy", "check", &good_ws, "--policy", &policy], &[]);
    assert_eq!(o.status.code(), Some(1), "{}", stderr(&o));
    assert!(
        stderr(&o).starts_with("policy load error: "),
        "{}",
        stderr(&o)
    );
    let (o, m) = spawn_traced(&["policy", "check", ws], &[]);
    assert_eq!(o.status.code(), Some(1), "{}", stderr(&o));
    assert!(
        stderr(&o).starts_with("al-sem policy check: the cross-app model could not be built"),
        "{}",
        stderr(&o)
    );
    assert_eq!(count(&m, "build.program"), 1, "precondition: the build ran");

    // Both: the policy error wins, and nothing is built.
    let (o, m) = spawn_traced(&["policy", "check", ws, "--policy", &policy], &[]);
    assert_eq!(count(&m, "build.program"), 0, "{m:?}\n{}", stderr(&o));
    assert_eq!(o.status.code(), Some(1), "{}", stderr(&o));
    assert!(
        stderr(&o).starts_with("policy load error: "),
        "{}",
        stderr(&o)
    );
    assert!(o.stdout.is_empty());
}

#[test]
fn decision7_precedence_shard_out_and_build() {
    let Some((d, _g)) = unreadable_ws(APP_JSON) else {
        return;
    };
    let ws = d.path().to_str().unwrap();
    let good_ws = crate::cli_build_counters::ws_a();
    const SHARD: &str = "--shard requires --out <directory>\n";

    let (o, _) = spawn_traced(&["fingerprint", &good_ws, "--shard", "all"], &[]);
    assert_eq!(o.status.code(), Some(1));
    assert_eq!(stderr(&o), SHARD);
    let (o, m) = spawn_traced(&["fingerprint", ws, "--format", "json"], &[]);
    assert_eq!(o.status.code(), Some(1));
    assert_eq!(stderr(&o), "fingerprint: workspace did not resolve\n");
    assert_eq!(count(&m, "build.program"), 1, "precondition: the build ran");

    let (o, m) = spawn_traced(&["fingerprint", ws, "--shard", "all"], &[]);
    assert_eq!(count(&m, "build.program"), 0, "{m:?}\n{}", stderr(&o));
    assert_eq!(o.status.code(), Some(1));
    assert_eq!(stderr(&o), SHARD);
    assert!(o.stdout.is_empty());
}

#[test]
fn decision7_precedence_policy_and_strict() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = crate::cli_diff_strict::error_ws(tmp.path(), "ws");
    let ws = ws.to_str().unwrap();
    let policy = bad_policy(tmp.path());

    // The error diagnostic alone, under --strict: its lines, exit 1.
    let (o, _) = spawn_traced(&["policy", "check", ws, "--strict"], &[]);
    assert_eq!(o.status.code(), Some(1));
    assert!(
        stderr(&o).starts_with(crate::cli_diff_strict::MULTI_APP_ERROR),
        "{}",
        stderr(&o)
    );
    // The bad policy alone (no --strict): the policy error.
    let (o, _) = spawn_traced(&["policy", "check", ws, "--policy", &policy], &[]);
    assert_eq!(o.status.code(), Some(1));
    assert!(
        stderr(&o).starts_with("policy load error: "),
        "{}",
        stderr(&o)
    );

    // Both: the policy error, exit 1, no diagnostic line.
    let (o, m) = spawn_traced(
        &["policy", "check", ws, "--strict", "--policy", &policy],
        &[],
    );
    assert_eq!(o.status.code(), Some(1));
    let err = stderr(&o);
    assert!(err.starts_with("policy load error: "), "{err}");
    assert!(
        !err.contains(crate::cli_diff_strict::MULTI_APP_ERROR),
        "{err}"
    );
    assert!(o.stdout.is_empty());
    assert_eq!(count(&m, "build.program"), 0, "{m:?}");
}

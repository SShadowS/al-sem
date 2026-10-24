//! #277 T12 (spec D4.1, D7, D9; 12A-17): a view that fails is each job's own
//! failure, in its own command's words; graph still exports off the program
//! the failing view built; the batch's other views run. A view's `log` lines
//! are printed once, on the run's stderr, however many jobs share the view.

use std::path::Path;
use std::process::Command;

use crate::cli_batch_plan::{argv, assert_each_standalone, batch, standalone_in};
use crate::cli_batch_writes::{job, run_manifest};
use crate::cli_build_counters::{count, policy_yaml, write_diff, ws_dep};

/// A workspace whose `app.json` has no `id`: its cross-app view builds the
/// program (graph reads it, D9) and then fails before the model.
fn no_id_ws() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(
        d.path().join("app.json"),
        r#"{"name":"A","publisher":"p","version":"1.0.0.0"}"#,
    )
    .unwrap();
    std::fs::create_dir_all(d.path().join("src")).unwrap();
    std::fs::write(d.path().join("src/A.al"), "codeunit 50100 A { }").unwrap();
    d
}

#[test]
fn batch_view_failure_per_job() {
    let x_dir = no_id_ws();
    let x = x_dir.path().to_string_lossy().into_owned();
    let diff = write_diff(x_dir.path(), "src/A.al", 1, "codeunit 50100 A { }");
    let (_d, y) = ws_dep();
    let policy = policy_yaml();
    let jobs = vec![
        argv(&["digest", &x, "--diff", &diff]),
        argv(&["policy", "check", &x, "--policy", &policy]),
        argv(&["graph", &x]),
        argv(&[
            "analyze",
            &y,
            "--single-app",
            "--format",
            "json",
            "--deterministic",
        ]),
        argv(&["events", "fanout", &y, "--single-app", "--deterministic"]),
    ];
    for j in &jobs[..3] {
        let (_, stderr, exit) = standalone_in(j, None);
        assert_ne!(exit, Some(0), "precondition: {j:?} fails alone");
        assert!(!stderr.is_empty(), "precondition: {j:?} says why");
    }
    let run = batch(&jobs);
    assert_each_standalone(&run, &jobs, None);
    // The failing view's program (for graph) and Y's single-app view.
    assert_eq!(
        count(&run.counters, "build.program"),
        2,
        "{:?}",
        run.counters
    );
}

/// #277 R7-8: two jobs spell one workspace differently and share its read
/// (the first job's spelling); graph's missing-`id` error still names its own
/// spelling, as standalone does.
#[test]
fn graph_error_names_its_own_spelling() {
    let x_dir = no_id_ws();
    let x = x_dir.path().to_string_lossy().into_owned();
    let other = format!("{x}/.");
    let jobs = vec![argv(&["events", "fanout", &x]), argv(&["graph", &other])];
    let (_, stderr, _) = standalone_in(&jobs[1], None);
    assert!(
        String::from_utf8_lossy(&stderr).contains(&format!("'{other}'")),
        "precondition: standalone names its spelling: {}",
        String::from_utf8_lossy(&stderr)
    );
    let run = batch(&jobs);
    assert_eq!(
        count(&run.counters, "inputs.read"),
        1,
        "precondition: one read"
    );
    assert_each_standalone(&run, &jobs, None);
}

/// The `log::warn!` lines of a shared view build (the snapshot cache, made
/// unwritable here) are view lines: printed once on the run's stderr, as a
/// one-job run prints them, and in no job's stderr file.
#[test]
fn batch_view_log_lines_once() {
    let ws = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/lsp-diff-deps")
        .to_string_lossy()
        .into_owned();
    let env = [
        ("XDG_CACHE_HOME", "/dev/null/no-cache"),
        ("RUST_LOG", "warn"),
    ];
    let is_log = |l: &&str| l.contains("al_sem::snapshot::cache");
    let alone = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .args(["events", "fanout", &ws])
        .envs(env)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&alone.stderr);
    let one = stderr.lines().filter(is_log).count();
    assert!(one > 0, "precondition: the view build logs: {stderr}");
    let d = tempfile::tempdir().unwrap();
    let jobs = [
        job(&["events", "fanout", &ws], None, None),
        job(&["events", "chains", &ws], None, None),
    ];
    let (out, _) = run_manifest(d.path(), &jobs, None, &env, None);
    assert_eq!(out.status.code(), Some(0));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(stderr.lines().filter(is_log).count(), one, "{stderr}");
    for i in 0..2 {
        let own = std::fs::read_to_string(d.path().join(format!("0.{i}.stderr"))).unwrap();
        assert_eq!(own.lines().filter(is_log).count(), 0, "job {i}: {own}");
    }
}

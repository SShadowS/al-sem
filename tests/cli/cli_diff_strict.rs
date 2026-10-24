//! `alsem diff --strict` in workspace mode (#89).
//!
//! `--strict` promises "Exit 1 on any error-severity analyzer diagnostic
//! (workspace mode)". These cases drive the real `alsem` binary (with
//! `RUST_LOG=off`, so a logger line can never add to stderr) over tempdir
//! workspaces:
//!   - a CLEAN workspace: a copy of `tests/fixtures/ws-diff-removed-field/a`;
//!   - an ERROR workspace: the same plus a nested `sub/app.json` (another id, no
//!     `.al` beside it), which yields the `multi-app source workspace
//!     unsupported` error diagnostic while the root app still loads (T2).
//!
//! Every failing case also asserts stderr has no `failed to load snapshot` line:
//! the input loaded, and the gate is what failed.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use crate::copy_dir;
use al_sem::engine::gate::diff::CoveragePolicy;
use al_sem::engine::gate::diff::cli::{DiffCliOptions, WORKSPACE_MODE_NOTE, run_diff};

pub(crate) const MULTI_APP_ERROR: &str = "error: multi-app source workspace unsupported";

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn fixture_side(side: &str) -> PathBuf {
    repo_root()
        .join("tests")
        .join("fixtures")
        .join("ws-diff-removed-field")
        .join(side)
}

fn diff_goldens_dir() -> PathBuf {
    repo_root().join("tests").join("cli-b-goldens").join("diff")
}

/// A clean workspace under `root/name`.
fn clean_ws(root: &Path, name: &str) -> PathBuf {
    let ws = root.join(name);
    copy_dir(&fixture_side("a"), &ws);
    ws
}

/// A clean workspace plus a nested second `app.json` (no `.al` beside it).
/// Also `cli_strict_events_policy`'s error workspace.
pub(crate) fn error_ws(root: &Path, name: &str) -> PathBuf {
    let ws = clean_ws(root, name);
    let sub = ws.join("sub");
    std::fs::create_dir_all(&sub).expect("mkdir sub");
    std::fs::write(
        sub.join("app.json"),
        r#"{ "id": "dd1f1e1d-0002-0002-0002-000000000002", "name": "Nested", "publisher": "DiffTest", "version": "1.0.0.0" }"#,
    )
    .expect("write nested app.json");
    ws
}

pub(crate) fn alsem_diff(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_alsem"))
        .arg("diff")
        .args(args)
        .env("RUST_LOG", "off")
        .output()
        .expect("run alsem diff")
}

fn s(p: &Path) -> String {
    p.to_string_lossy().to_string()
}

fn stderr_lines(out: &Output) -> Vec<String> {
    String::from_utf8_lossy(&out.stderr)
        .lines()
        .map(str::to_string)
        .collect()
}

/// The shared assertions of a strict-gate failure: exit 1, no stdout, no load
/// failure, no ws-mode note. Returns the stderr lines carrying the multi-app error.
fn assert_gate_failed(out: &Output) -> Vec<String> {
    let lines = stderr_lines(out);
    assert_eq!(out.status.code(), Some(1), "exit 1; stderr={lines:?}");
    assert!(out.stdout.is_empty(), "no stdout; got {:?}", out.stdout);
    assert!(
        !lines.iter().any(|l| l.contains("failed to load snapshot")),
        "the input must load; stderr={lines:?}"
    );
    assert!(
        !lines.iter().any(|l| l == WORKSPACE_MODE_NOTE),
        "no ws-mode note on a gate failure; stderr={lines:?}"
    );
    assert!(
        lines.iter().all(|l| ["error:", "warning:", "info:"]
            .iter()
            .any(|p| l.starts_with(p))),
        "a gate failure prints analyzer diagnostic lines only; stderr={lines:?}"
    );
    let errors: Vec<String> = lines
        .iter()
        .filter(|l| l.starts_with(MULTI_APP_ERROR))
        .cloned()
        .collect();
    assert!(
        !errors.is_empty(),
        "the analyzer error line; stderr={lines:?}"
    );
    errors
}

/// T1: an error on the NEW side exits 1, prints the error, writes nothing.
#[test]
fn t1_error_on_new_side_exits_1_and_writes_nothing() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let old = clean_ws(tmp.path(), "old");
    let new = error_ws(tmp.path(), "new");

    // An existing --out file is left byte-unchanged.
    let sentinel = tmp.path().join("sentinel.txt");
    std::fs::write(&sentinel, b"SENTINEL").expect("write sentinel");
    let out = alsem_diff(&[
        &s(&old),
        &s(&new),
        "--strict",
        "--deterministic",
        "--out",
        &s(&sentinel),
    ]);
    assert_gate_failed(&out);
    assert_eq!(
        std::fs::read(&sentinel).expect("read sentinel"),
        b"SENTINEL",
        "--out must not be touched"
    );

    // An absent --out file is not created.
    let absent = tmp.path().join("absent.txt");
    let out = alsem_diff(&[
        &s(&old),
        &s(&new),
        "--strict",
        "--deterministic",
        "--out",
        &s(&absent),
    ]);
    assert_gate_failed(&out);
    assert!(!absent.exists(), "--out must not be created");
}

/// T2 (control): the same inputs without --strict load and diff, exit 0.
#[test]
fn t2_same_inputs_without_strict_exit_0() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let old = clean_ws(tmp.path(), "old");
    let new = error_ws(tmp.path(), "new");
    let out = alsem_diff(&[&s(&old), &s(&new), "--deterministic"]);
    let lines = stderr_lines(&out);
    assert_eq!(out.status.code(), Some(0), "exit 0; stderr={lines:?}");
    assert!(!out.stdout.is_empty(), "the diff is printed");
    assert_eq!(lines, vec![WORKSPACE_MODE_NOTE.to_string()]);
}

/// T3: an error on the OLD side alone exits 1.
#[test]
fn t3_error_on_old_side_exits_1() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let old = error_ws(tmp.path(), "old");
    let new = clean_ws(tmp.path(), "new");
    let out = alsem_diff(&[&s(&old), &s(&new), "--strict", "--deterministic"]);
    assert_gate_failed(&out);
}

/// T4 (control): snapshot-input mode is byte-identical with and without --strict.
#[test]
fn t4_snapshot_input_strict_is_a_no_op() {
    let snaps = diff_goldens_dir().join("snapshots");
    let old = s(&snaps.join("schema-field-removed-old.snap.json"));
    let new = s(&snaps.join("schema-field-removed-new.snap.json"));
    for format in ["json", "human", "sarif"] {
        let run = |strict: bool| {
            run_diff(&DiffCliOptions {
                old_arg: &old,
                new_arg: &new,
                format,
                out: None,
                coverage_policy: CoveragePolicy::Loose,
                renames_path: None,
                fail_on: None,
                strict,
                deterministic: true,
                driver_version: "cli-b-v1",
            })
        };
        let plain = run(false);
        let strict = run(true);
        assert!(plain.output.is_some(), "{format}: a diff is produced");
        assert_eq!(plain.output, strict.output, "{format}: output");
        assert_eq!(plain.stderr_lines, strict.stderr_lines, "{format}: stderr");
        assert_eq!(plain.exit_code, strict.exit_code, "{format}: exit");
        assert_eq!(
            plain.error_message, strict.error_message,
            "{format}: error message"
        );
    }
}

/// T5 (control): a clean ws pair under --strict prints exactly the ws-mode goldens.
#[test]
fn t5_clean_ws_pair_strict_matches_ws_mode_goldens() {
    let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .arg("diff")
        .arg(fixture_side("a"))
        .arg(fixture_side("b"))
        .args(["--strict", "--deterministic", "--format", "human"])
        .env("RUST_LOG", "off")
        .env("ALCH_DRIVER_VERSION_OVERRIDE", "cli-b-v1")
        .output()
        .expect("run alsem diff");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "exit 0; stderr={stderr}");
    let read = |name: &str| {
        std::fs::read_to_string(diff_goldens_dir().join(name))
            .unwrap_or_else(|e| panic!("read golden {name}: {e}"))
    };
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        read("ws-mode-schema-field-removed.stdout.txt"),
        "stdout"
    );
    assert_eq!(
        stderr,
        read("ws-mode-schema-field-removed.stderr.txt"),
        "stderr is exactly the note"
    );
}

/// T6 (mixed): a clean snapshot file as old, the error workspace as new.
#[test]
fn t6_snapshot_old_error_ws_new_exits_1() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let old = diff_goldens_dir()
        .join("snapshots")
        .join("schema-field-removed-old.snap.json");
    let new = error_ws(tmp.path(), "new");
    let out = alsem_diff(&[&s(&old), &s(&new), "--strict", "--deterministic"]);
    assert_gate_failed(&out);
}

/// T7 (dedupe): the same error workspace on both sides prints its line once.
#[test]
fn t7_same_error_ws_both_sides_prints_line_once() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let ws = error_ws(tmp.path(), "ws");
    let out = alsem_diff(&[&s(&ws), &s(&ws), "--strict", "--deterministic"]);
    let errors = assert_gate_failed(&out);
    assert_eq!(errors.len(), 1, "printed exactly once; got {errors:?}");
}

/// T8 (order): the old side's error line comes before the new side's.
#[test]
fn t8_old_side_lines_before_new_side_lines() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let old = error_ws(tmp.path(), "old");
    let new = error_ws(tmp.path(), "new");
    let out = alsem_diff(&[&s(&old), &s(&new), "--strict", "--deterministic"]);
    let errors = assert_gate_failed(&out);
    assert_eq!(errors.len(), 2, "one line per side; got {errors:?}");
    assert!(
        errors[0].contains(&s(&old)) && !errors[0].contains(&s(&new)),
        "first line is the old side's; got {errors:?}"
    );
    assert!(
        errors[1].contains(&s(&new)),
        "second line is the new side's; got {errors:?}"
    );
}

/// T9 (position): the gate runs before the --renames overlay is loaded.
#[test]
fn t9_gate_runs_before_rename_overlay_load() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let old = clean_ws(tmp.path(), "old");
    let new = error_ws(tmp.path(), "new");
    let missing = tmp.path().join("no-such-overlay.json");
    let out = alsem_diff(&[
        &s(&old),
        &s(&new),
        "--strict",
        "--deterministic",
        "--renames",
        &s(&missing),
    ]);
    let lines = stderr_lines(&out);
    assert!(
        !lines
            .iter()
            .any(|l| l.contains("failed to load rename overlay")),
        "the overlay must not be loaded; stderr={lines:?}"
    );
    assert_gate_failed(&out);
}

/// T10 (collection keeps non-error lines): an old side with only an `info`
/// diagnostic (an `.al` file declaring no object) and a new side with the error
/// print BOTH, the old side's info first. Mutations: collect only error lines, or
/// drop the old side's lines -> the info line is missing.
#[test]
fn t10_non_error_lines_of_both_sides_are_printed_in_order() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let old = clean_ws(tmp.path(), "old");
    std::fs::write(old.join("Empty.al"), "// no object here\n").expect("write Empty.al");
    let new = error_ws(tmp.path(), "new");
    let out = alsem_diff(&[&s(&old), &s(&new), "--strict", "--deterministic"]);
    assert_gate_failed(&out);
    let lines = stderr_lines(&out);
    let info = lines
        .iter()
        .position(|l| {
            l.starts_with("info: No object declaration found in") && l.contains("Empty.al")
        })
        .unwrap_or_else(|| panic!("the old side's info line; stderr={lines:?}"));
    let error = lines
        .iter()
        .position(|l| l.starts_with(MULTI_APP_ERROR))
        .unwrap_or_else(|| panic!("the new side's error line; stderr={lines:?}"));
    assert!(info < error, "old side first; stderr={lines:?}");
}

//! #160 step 1 (spec §4.2.5, A1.4): one writer, one write-failure rule.
//!
//! A failed write of a command's output prints `al-sem: write error: <stdout|path>:
//! <OS message>` on stderr and exits with the command's own failure code: 2 for
//! analyze, 1 for every other command. It never aborts the process.

use std::process::{Command, Output, Stdio};

use crate::cli_build_counters::{write_diff, ws_a};

/// Spawn `alsem <args>` and close the read end of its stdout before it writes.
///
/// The drop runs right after `spawn` returns, while the child is still starting up
/// and building the program; the commands below take far longer than the drop, so
/// their one output write always meets a closed pipe. The only race is in the
/// "already written" direction, which these commands cannot reach that early.
fn closed_pipe(args: &[&str]) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn alsem");
    drop(child.stdout.take());
    child.wait_with_output().expect("wait alsem")
}

fn write_error_lines(stderr: &[u8], target: &str) -> usize {
    let prefix = format!("al-sem: write error: {target}: ");
    String::from_utf8_lossy(stderr)
        .lines()
        .filter(|l| l.starts_with(&prefix))
        .count()
}

/// A one-routine `--diff` against workspace A, in `dir`.
fn digest_diff(dir: &std::path::Path) -> String {
    write_diff(dir, "src/D1MultiCaller.al", 18, "        Cust.Modify();")
}

#[test]
fn analyze_closed_pipe_exits_2() {
    let out = closed_pipe(&["analyze", &ws_a(), "--format", "json", "--single-app"]);
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(2), "stderr: {err}");
    assert_eq!(write_error_lines(&out.stderr, "stdout"), 1, "stderr: {err}");
}

/// Code review CR2: with stderr closed too, the write-error message itself
/// fails; that must not panic (a panicking `eprintln!`) but still exit 2.
#[test]
fn analyze_closed_stdout_and_stderr_exits_2() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .args(["analyze", &ws_a(), "--format", "json", "--single-app"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn alsem");
    drop(child.stdout.take());
    drop(child.stderr.take());
    let status = child.wait().expect("wait alsem");
    assert_eq!(status.code(), Some(2), "{status:?}");
}

#[test]
fn digest_closed_pipe_exits_1() {
    let d = tempfile::tempdir().unwrap();
    let diff = digest_diff(d.path());
    let out = closed_pipe(&[
        "digest",
        &ws_a(),
        "--diff",
        &diff,
        "--deterministic",
        "--single-app",
    ]);
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "stderr: {err}");
    assert_eq!(write_error_lines(&out.stderr, "stdout"), 1, "stderr: {err}");
    assert_eq!(
        err.matches("write error").count(),
        1,
        "exactly one write-error message: {err}"
    );
}

#[test]
fn digest_out_unwritable_exits_1() {
    let d = tempfile::tempdir().unwrap();
    let diff = digest_diff(d.path());
    // A regular file used as a directory: the write fails on every platform.
    let file_not_dir = d.path().join("f");
    std::fs::write(&file_not_dir, "x").unwrap();
    let target = file_not_dir.join("out.json");
    let target_s = target.to_str().unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .args([
            "digest",
            &ws_a(),
            "--diff",
            &diff,
            "--deterministic",
            "--single-app",
            "--out",
            target_s,
        ])
        .output()
        .expect("spawn alsem");
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "stderr: {err}");
    assert_eq!(write_error_lines(&out.stderr, target_s), 1, "stderr: {err}");
}

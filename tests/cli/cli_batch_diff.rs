//! #277 T21 (spec D8, D7): a diff whose old side fails discards its new
//! side's job lines, also when a batch computed that new side first, on an
//! earlier view. The view-order cases (`[diff A B, diff B A]`, `[diff A B,
//! analyze A]`, `diff A A`, `diff bad A`) are `cli_batch_plan::batch_diff_sides`.

use crate::cli_batch_plan::{argv, batch_in};
#[cfg(debug_assertions)]
use crate::cli_batch_plan::{assert_each_standalone, standalone_in};
use crate::cli_build_counters::ws_a;

/// Every solve prints one non-convergence line (debug-only seam, spec T1 B1;
/// release builds have no seam, so its tests are debug-only, #277 R10-2).
#[cfg(debug_assertions)]
const SEAM: (&str, &str) = ("ALSEM_TEST_SUMMARIZE_DIAGNOSTIC", "1");
#[cfg(debug_assertions)]
const SEAM_LINE: &str = "warning: summarize: test seam: a forced non-convergence";

#[cfg(debug_assertions)]
#[test]
fn batch_diff_failed_old_side_drops_new_side_lines() {
    let a = ws_a();
    let bad_dir = tempfile::tempdir().unwrap();
    std::fs::write(
        bad_dir.path().join("app.json"),
        r#"{"name":"A","publisher":"p","version":"1.0.0.0"}"#,
    )
    .unwrap();
    let bad = bad_dir.path().to_string_lossy().into_owned();
    let diff = |o: &str, n: &str| {
        argv(&[
            "diff",
            o,
            n,
            "--strict",
            "--format",
            "json",
            "--deterministic",
        ])
    };
    // Job 0 makes A the first view, so job 1's new side (A) computes there,
    // before its old side (bad) fails on the second view.
    let jobs = vec![diff(&a, &a), diff(&bad, &a)];
    let with_seam = |j: &Vec<String>| {
        let out = std::process::Command::new(env!("CARGO_BIN_EXE_alsem"))
            .args(j)
            .env("RUST_LOG", "off")
            .env(SEAM.0, SEAM.1)
            .output()
            .unwrap();
        (
            String::from_utf8_lossy(&out.stderr).into_owned(),
            out.status.code(),
        )
    };
    // `diff A A`'s two sides share one view and one solve (D8), so its line
    // prints once, standalone and in a batch (stated, not compared: standalone
    // runs the same driver).
    let (stderr, _) = with_seam(&jobs[0]);
    assert_eq!(stderr.matches(SEAM_LINE).count(), 1, "one solve: {stderr}");
    let (stderr, exit) = with_seam(&jobs[1]);
    assert_eq!(exit, Some(1), "precondition: the old side fails alone");
    assert!(!stderr.contains(SEAM_LINE), "precondition: {stderr}");
    let run = batch_in(&jobs, None, &[SEAM]);
    // `assert_each_standalone` compares with a seam-less standalone run, so the
    // seam's lines are checked here and the rest of each job there.
    let (_, b_err) = run.streams(1);
    assert!(
        !String::from_utf8_lossy(&b_err).contains(SEAM_LINE),
        "job 1 replays its discarded new side's line: {}",
        String::from_utf8_lossy(&b_err)
    );
    let (_, b_err) = run.streams(0);
    assert_eq!(
        String::from_utf8_lossy(&b_err).matches(SEAM_LINE).count(),
        1,
        "job 0 replays its view's solve line once"
    );
    assert_eq!(standalone_in(&jobs[1], None).2, Some(1));
    let plain = batch_in(&jobs, None, &[]);
    assert_each_standalone(&plain, &jobs, None);
}

/// D6.3: `[retained-census] batch.held outputs= sides= inputs=` at each view's
/// start. `[diff A B, diff B A, analyze A --single-app]` visits A, then B. At
/// A's start only B's read is held (read before the first build, D3); at B's
/// start the two sides computed on A wait for B, analyze's output waits for
/// the diffs (job order), and no read of a later view is left.
#[test]
fn batch_held_census() {
    let a = ws_a();
    let (_d, b) = crate::cli_build_counters::ws_dep();
    let diff = |o: &str, n: &str| argv(&["diff", o, n, "--format", "json"]);
    let jobs = vec![
        diff(&a, &b),
        diff(&b, &a),
        argv(&["analyze", &a, "--single-app", "--format", "json"]),
    ];
    let run = batch_in(&jobs, None, &[("ALSEM_RETAINED_CENSUS", "1")]);
    assert_eq!(run.out.status.code(), Some(0));
    let held: Vec<[u64; 3]> = String::from_utf8_lossy(&run.out.stderr)
        .lines()
        .filter_map(|l| l.strip_prefix("[retained-census] batch.held "))
        .map(|rest| {
            let mut v = rest.split(' ').map(|kv| {
                let (_, n) = kv.split_once('=').expect("key=value");
                n.parse::<u64>().unwrap()
            });
            [v.next().unwrap(), v.next().unwrap(), v.next().unwrap()]
        })
        .collect();
    assert_eq!(held.len(), 2, "one line per view: {held:?}");
    let [outputs, sides, inputs] = held[0];
    assert!(outputs == 0 && sides == 0 && inputs > 0, "view A: {held:?}");
    let [outputs, sides, inputs] = held[1];
    assert!(outputs > 0 && sides > 0 && inputs == 0, "view B: {held:?}");
}

/// #277 R7-9: a view whose every demand was skipped opens no span. `diff bad
/// A`: the old side's view fails, so A's view (the new side alone) is
/// skipped: one `view.<i>` span, not two.
#[test]
fn a_skipped_view_opens_no_span() {
    let bad_dir = tempfile::tempdir().unwrap();
    std::fs::write(
        bad_dir.path().join("app.json"),
        r#"{"name":"A","publisher":"p","version":"1.0.0.0"}"#,
    )
    .unwrap();
    let bad = bad_dir.path().to_string_lossy().into_owned();
    let jobs = vec![argv(&["diff", &bad, &ws_a()])];
    let run = batch_in(&jobs, None, &[]);
    assert_eq!(
        run.lines()[0]["exit"],
        1,
        "precondition: the old side fails"
    );
    let views = crate::cli_batch_plan::spans(&run.events, crate::cli_batch_plan::is_view);
    assert_eq!(views.len(), 1, "{views:?}");
}

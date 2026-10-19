//! #160 step 0 (spec `2026-10-18-issue-160-steps-0-2-design.md` §4.1.2, §4.1.3):
//! how many times each command builds each heavy artifact, read from the
//! SHIPPED binary's own Chrome trace.
//!
//! Every counted builder emits `pt::counter_delta("build.<name>", 1)` at its
//! entry (or, for `build.graph_scc`, on the line before each top-level
//! `tarjan_scc` call, never inside it). The trace's `C` event carries the
//! running total under `args.<name>`; the reader keeps the LAST value per
//! name. One test per row of §4.1.3: the table is the literal expectation, and
//! a step that moves a count edits the row in the same commit.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;

/// Spawn `alsem <args>` with tracing on into a temp trace file, every other
/// `ALSEM_TRACE_*` knob removed (pattern: `cli_envelope_detector_set.rs`).
pub(crate) fn spawn_traced(
    args: &[&str],
    extra_env: &[(&str, &str)],
) -> (Output, BTreeMap<String, u64>) {
    let (out, events) = spawn_trace_events(env!("CARGO_BIN_EXE_alsem"), args, extra_env);
    (out, last_counters(&events))
}

/// [`spawn_traced`] for any binary, returning every trace event.
fn spawn_trace_events(
    exe: &str,
    args: &[&str],
    extra_env: &[(&str, &str)],
) -> (Output, Vec<Value>) {
    let dir = tempfile::tempdir().expect("tempdir");
    let trace = dir.path().join("trace.json");
    let mut cmd = Command::new(exe);
    cmd.args(args)
        .env_remove("ALSEM_TRACE_DETAIL")
        .env_remove("ALSEM_TRACE_SAMPLE_EVERY")
        .env_remove("ALSEM_TRACE_SCC_MIN")
        .env_remove("ALSEM_TRACE_STDERR")
        .env_remove("ALSEM_TRACE_EXIT_AFTER")
        .env_remove("ALSEM_CONES_CENSUS")
        .env_remove("ALSEM_RETAINED_CENSUS")
        .env("ALSEM_TRACE", "1")
        .env("ALSEM_TRACE_FILE", &trace);
    for (k, v) in extra_env {
        cmd.env(k, v);
    }
    let out = cmd.output().expect("spawn");
    let text = std::fs::read_to_string(&trace).unwrap_or_default();
    (out, serde_json::from_str(&text).unwrap_or_default())
}

/// The `phase.*` instant events, in trace order.
fn phase_events(events: &[Value]) -> Vec<String> {
    events
        .iter()
        .filter(|e| e.get("cat").and_then(Value::as_str) == Some("phase"))
        .filter_map(|e| e.get("name").and_then(Value::as_str).map(str::to_string))
        .collect()
}

/// LAST `C` event value per counter name (spec §4.1.2). `perf_trace`'s
/// `add_counter` writes the running total under `args.<name>`.
fn last_counters(events: &[Value]) -> BTreeMap<String, u64> {
    let mut m = BTreeMap::new();
    for ev in events {
        if ev.get("ph").and_then(Value::as_str) != Some("C") {
            continue;
        }
        let Some(name) = ev.get("name").and_then(Value::as_str) else {
            continue;
        };
        if let Some(v) = ev
            .get("args")
            .and_then(|a| a.get(name))
            .and_then(Value::as_u64)
        {
            m.insert(name.to_string(), v);
        }
    }
    m
}

/// A counter's last value; an absent name is 0.
pub(crate) fn count(m: &BTreeMap<String, u64>, name: &str) -> u64 {
    m.get(name).copied().unwrap_or(0)
}

/// One row of spec §4.1.3 / §4.4.8, in the table's column order.
pub(crate) struct Row {
    pub program: u64,
    pub bcg: u64,
    pub cone: u64,
    pub v2: u64,
    pub scc: u64,
    pub pd: u64,
    pub ctx: u64,
    pub snap: u64,
}

/// Every cell is checked and every mismatch reported, so a break that moves
/// two counters (a second context build also builds a second graph) names
/// both cells.
pub(crate) fn assert_row(case: &str, m: &BTreeMap<String, u64>, r: Row) {
    let want = [
        ("build.program", r.program),
        ("build.combined_graph", r.bcg),
        ("build.cone", r.cone),
        ("build.summaries_v2", r.v2),
        ("build.graph_scc", r.scc),
        ("build.pd_solve", r.pd),
        ("build.detector_context", r.ctx),
        ("build.snapshot", r.snap),
        // A1.1: one workspace read per program build (final panel r1, F2-3).
        ("inputs.read", r.program),
    ];
    let bad: Vec<String> = want
        .iter()
        .filter(|(name, w)| count(m, name) != *w)
        .map(|(name, w)| format!("[{case}] {name}: got {}, want {w}", count(m, name)))
        .collect();
    assert!(bad.is_empty(), "{}\nall counters: {m:?}", bad.join("\n"));
}

pub(crate) fn r0(ws: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/r0-corpus")
        .join(ws)
}

/// Workspace A: no dependency.
pub(crate) fn ws_a() -> String {
    r0("ws-d1-multi-caller").to_string_lossy().into_owned()
}

/// Workspace D: the dependency workspace, in a tempdir the caller keeps alive.
fn ws_dep() -> (tempfile::TempDir, String) {
    let d = tempfile::tempdir().expect("tempdir");
    crate::cli_digest_query_dependencies::write_dependency_workspace(d.path());
    let ws = d.path().to_string_lossy().into_owned();
    (d, ws)
}

/// A one-line unified diff (shape of `cli_digest_changed_spans.rs`'s `one_line_diff`).
pub(crate) fn write_diff(dir: &Path, path: &str, n: usize, text: &str) -> String {
    let p = dir.join("change.diff");
    std::fs::write(
        &p,
        format!("--- a/{path}\n+++ b/{path}\n@@ -{n},1 +{n},1 @@\n-{text} \n+{text}\n"),
    )
    .unwrap();
    p.to_string_lossy().into_owned()
}

fn policy_yaml() -> String {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/cli-c-policy/ws-policy-custom/al-sem.policy.yaml")
        .to_string_lossy()
        .into_owned()
}

/// Run and assert the process ran to an exit code (no signal / spawn failure)
/// and built a program at all (a usage error builds nothing; show its stderr).
fn run(args: &[&str]) -> BTreeMap<String, u64> {
    let (out, m) = spawn_traced(args, &[]);
    assert!(
        out.status.code().is_some() && count(&m, "build.program") > 0,
        "alsem {args:?} exited {:?} with no program build\nstderr: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
    m
}

// ---------------------------------------------------------------------------
// §4.1.3 rows
// ---------------------------------------------------------------------------

#[test]
fn analyze_cross_app_default() {
    let (_d, ws) = ws_dep();
    let m = run(&["analyze", &ws, "--format", "json"]);
    assert_row(
        "analyze cross-app",
        &m,
        Row {
            program: 1,
            bcg: 3,
            cone: 1,
            v2: 1,
            scc: 3,
            pd: 1,
            ctx: 1,
            snap: 0,
        },
    );
}

#[test]
fn analyze_cross_app_d47() {
    let (_d, ws) = ws_dep();
    let m = run(&[
        "analyze",
        &ws,
        "--format",
        "json",
        "--detector",
        "d47-io-unsafe-txn",
    ]);
    // `build.ordering_facts` has no §4.1.3 column: d47 is the one row that
    // folds the ordering cell, exactly once (`registry.rs`'s runner). Checked
    // first: a second fold also builds a second snapshot, which the row reports.
    assert_eq!(
        count(&m, "build.ordering_facts"),
        1,
        "[analyze cross-app d47] build.ordering_facts\nall counters: {m:?}"
    );
    assert_row(
        "analyze cross-app d47",
        &m,
        Row {
            program: 1,
            bcg: 4,
            cone: 1,
            v2: 1,
            scc: 3,
            pd: 1,
            ctx: 1,
            snap: 1,
        },
    );
}

#[test]
fn analyze_single_app() {
    let m = run(&["analyze", &ws_a(), "--single-app", "--format", "json"]);
    assert_row(
        "analyze single-app",
        &m,
        Row {
            program: 1,
            bcg: 1,
            cone: 1,
            v2: 1,
            scc: 2,
            pd: 1,
            ctx: 1,
            snap: 0,
        },
    );
}

#[test]
fn digest_cross_app() {
    let (d, ws) = ws_dep();
    let diff = write_diff(d.path(), "src/Work.al", 18, "        Customer.Insert();");
    let m = run(&["digest", &ws, "--diff", &diff, "--deterministic"]);
    assert_row(
        "digest cross-app",
        &m,
        Row {
            program: 1,
            bcg: 5,
            cone: 3,
            v2: 4,
            scc: 7,
            pd: 3,
            ctx: 2,
            snap: 1,
        },
    );
}

#[test]
fn digest_single_app() {
    let d = tempfile::tempdir().unwrap();
    let diff = write_diff(
        d.path(),
        "src/D1MultiCaller.al",
        18,
        "        Cust.Modify();",
    );
    let m = run(&[
        "digest",
        &ws_a(),
        "--diff",
        &diff,
        "--deterministic",
        "--single-app",
    ]);
    assert_row(
        "digest single-app",
        &m,
        Row {
            program: 1,
            bcg: 4,
            cone: 3,
            v2: 4,
            scc: 7,
            pd: 3,
            ctx: 2,
            snap: 1,
        },
    );
}

#[test]
fn prove() {
    let (_d, ws) = ws_dep();
    let m = run(&["prove", &ws, "CallLib", "may-commit", "--deterministic"]);
    assert_row(
        "prove",
        &m,
        Row {
            program: 1,
            bcg: 4,
            cone: 3,
            v2: 4,
            scc: 7,
            pd: 3,
            ctx: 2,
            snap: 1,
        },
    );
}

const FINGERPRINT_ROW: Row = Row {
    program: 1,
    bcg: 2,
    cone: 2,
    v2: 2,
    scc: 4,
    pd: 2,
    ctx: 1,
    snap: 1,
};

#[test]
fn fingerprint_json() {
    let (_d, ws) = ws_dep();
    let m = run(&["fingerprint", &ws, "--format", "json", "--deterministic"]);
    assert_row("fingerprint json", &m, FINGERPRINT_ROW);
}

#[test]
fn fingerprint_query() {
    let (_d, ws) = ws_dep();
    let m = run(&[
        "fingerprint",
        &ws,
        "--format",
        "json",
        "--routine",
        "CallLib",
        "--deterministic",
    ]);
    assert_row("fingerprint query", &m, FINGERPRINT_ROW);
}

#[test]
fn fingerprint_inventory_only() {
    let (_d, ws) = ws_dep();
    let m = run(&[
        "fingerprint",
        &ws,
        "--inventory-only",
        "--format",
        "json",
        "--deterministic",
    ]);
    assert_row("fingerprint --inventory-only", &m, FINGERPRINT_ROW);
}

/// `diff ws ws`: the row is per side, so the process total is twice it.
#[test]
fn diff_ws_ws() {
    let (_d, ws) = ws_dep();
    let m = run(&["diff", &ws, &ws, "--format", "json", "--deterministic"]);
    assert_row(
        "diff ws ws",
        &m,
        Row {
            program: 2,
            bcg: 2,
            cone: 2,
            v2: 2,
            scc: 4,
            pd: 2,
            ctx: 0,
            snap: 2,
        },
    );
}

/// `--strict` adds ctx 1, bcg 1, cone 1, v2 1, scc 2, pd 1 per side.
#[test]
fn diff_ws_ws_strict() {
    let (_d, ws) = ws_dep();
    let m = run(&[
        "diff",
        &ws,
        &ws,
        "--format",
        "json",
        "--deterministic",
        "--strict",
    ]);
    assert_row(
        "diff ws ws --strict",
        &m,
        Row {
            program: 2,
            bcg: 4,
            cone: 4,
            v2: 4,
            scc: 8,
            pd: 4,
            ctx: 2,
            snap: 2,
        },
    );
}

const EVENTS_CROSS_ROW: Row = Row {
    program: 1,
    bcg: 4,
    cone: 2,
    v2: 2,
    scc: 5,
    pd: 2,
    ctx: 2,
    snap: 0,
};

#[test]
fn events_fanout_cross_app() {
    let (_d, ws) = ws_dep();
    let m = run(&[
        "events",
        "fanout",
        &ws,
        "--format",
        "json",
        "--deterministic",
    ]);
    assert_row("events fanout cross-app", &m, EVENTS_CROSS_ROW);
}

#[test]
fn events_chains_cross_app() {
    let (_d, ws) = ws_dep();
    let m = run(&[
        "events",
        "chains",
        &ws,
        "--format",
        "json",
        "--deterministic",
    ]);
    assert_row("events chains cross-app", &m, EVENTS_CROSS_ROW);
}

#[test]
fn events_single_app() {
    let m = run(&[
        "events",
        "fanout",
        &ws_a(),
        "--format",
        "json",
        "--deterministic",
        "--single-app",
    ]);
    assert_row(
        "events single-app",
        &m,
        Row {
            program: 1,
            bcg: 2,
            cone: 2,
            v2: 2,
            scc: 4,
            pd: 2,
            ctx: 2,
            snap: 0,
        },
    );
}

#[test]
fn policy_check_cross_app() {
    let (_d, ws) = ws_dep();
    let policy = policy_yaml();
    let m = run(&[
        "policy",
        "check",
        &ws,
        "--policy",
        &policy,
        "--format",
        "json",
        "--deterministic",
    ]);
    assert_row(
        "policy check cross-app",
        &m,
        Row {
            program: 1,
            bcg: 4,
            cone: 2,
            v2: 2,
            scc: 5,
            pd: 2,
            ctx: 2,
            snap: 0,
        },
    );
}

#[test]
fn policy_check_single_app() {
    let policy = policy_yaml();
    let m = run(&[
        "policy",
        "check",
        &ws_a(),
        "--policy",
        &policy,
        "--format",
        "json",
        "--deterministic",
        "--single-app",
    ]);
    assert_row(
        "policy check single-app",
        &m,
        Row {
            program: 1,
            bcg: 2,
            cone: 2,
            v2: 2,
            scc: 4,
            pd: 2,
            ctx: 2,
            snap: 0,
        },
    );
}

const QUERY_CROSS_ROW: Row = Row {
    program: 1,
    bcg: 2,
    cone: 0,
    v2: 1,
    scc: 1,
    pd: 0,
    ctx: 0,
    snap: 0,
};

#[test]
fn query_touches_cross_app() {
    let (_d, ws) = ws_dep();
    let m = run(&["query", "touches", &ws, "--table", "18", "--format", "json"]);
    assert_row("query touches cross-app", &m, QUERY_CROSS_ROW);
}

#[test]
fn query_effects_cross_app() {
    let (_d, ws) = ws_dep();
    let m = run(&[
        "query",
        "effects",
        &ws,
        "--routine",
        "CallLib",
        "--format",
        "json",
    ]);
    assert_row("query effects cross-app", &m, QUERY_CROSS_ROW);
}

#[test]
fn query_single_app() {
    let m = run(&[
        "query",
        "touches",
        &ws_a(),
        "--table",
        "MC Customer",
        "--format",
        "json",
        "--single-app",
    ]);
    assert_row(
        "query single-app",
        &m,
        Row {
            program: 1,
            bcg: 1,
            cone: 0,
            v2: 1,
            scc: 1,
            pd: 0,
            ctx: 0,
            snap: 0,
        },
    );
}

/// Spec §8.1: spawned, because `cones_census::enabled()` is a process OnceLock
/// and its counters are process-global. `rest` is what compose spends outside
/// every timed component, charged per call as `elapsed - Σ component deltas`;
/// a negative value means two component timers overlap.
#[test]
fn census_rest_is_never_negative() {
    let (out, _) = spawn_traced(
        &["analyze", &ws_a(), "--single-app", "--format", "json"],
        &[("ALSEM_CONES_CENSUS", "1")],
    );
    let err = String::from_utf8_lossy(&out.stderr);
    let line = err
        .lines()
        .find(|l| l.starts_with("[cones-census]") && l.contains("rest="))
        .unwrap_or_else(|| panic!("no rest line:\n{err}"));
    let v: f64 = line
        .split("rest=")
        .nth(1)
        .unwrap()
        .split(|c: char| !(c == '-' || c == '.' || c.is_ascii_digit()))
        .next()
        .unwrap()
        .parse()
        .unwrap();
    assert!(
        v >= 0.0,
        "negative compose remainder (timer overlap): {line}"
    );
}

/// Spec §4.1.4 / §8.1: both peaks reach the trace. Windows only, because
/// `read_rss()` is `None` elsewhere and both counters are then 0. Spawned,
/// because the tracer is a process `OnceLock` read from the environment.
#[cfg(windows)]
#[test]
fn phase_peak_is_reported() {
    let (out, m) = spawn_traced(
        &["analyze", &ws_a(), "--single-app", "--format", "json"],
        &[],
    );
    assert!(
        out.status.code().is_some(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let post = count(&m, "peak.post_attach_mb");
    let proc_ = count(&m, "peak.process_mb");
    assert!(
        0 < post && post <= proc_,
        "post_attach={post} process={proc_}"
    );
}

/// Spec §4.1.4: `diff ws ws` builds a model per side. Each side's post-attach
/// phase ends when that side's model is dropped, so the second side's parse and
/// attach are never inside a phase and the reported peak is the maximum over
/// both sides' phases (code-review CR2).
#[test]
fn diff_ends_each_sides_phase() {
    let ws = ws_a();
    let (out, events) = spawn_trace_events(
        env!("CARGO_BIN_EXE_alsem"),
        &["diff", &ws, &ws, "--format", "json", "--deterministic"],
        &[],
    );
    assert!(
        out.status.code().is_some(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        phase_events(&events),
        ["phase.start", "phase.end", "phase.start", "phase.end"]
    );
}

/// Spec §4.1.4: `ALSEM_TRACE_EXIT_AFTER` exits from a span guard, where the
/// run guard never drops; both peaks must still reach the trace (code-review
/// CR6). Presence, not value: both are 0 off Windows.
#[test]
fn exit_after_still_reports_both_peaks() {
    let (out, m) = spawn_traced(
        &["analyze", &ws_a(), "--single-app", "--format", "json"],
        &[("ALSEM_TRACE_EXIT_AFTER", "context.capability_cones")],
    );
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    for name in ["peak.process_mb", "peak.post_attach_mb"] {
        assert!(m.contains_key(name), "{name} missing: {m:?}");
    }
}

/// Spec §4.1.4: only a binary that opens `pt::run` reports a phase peak, so
/// only there does `mark_phase_start` start a sampler. `aldump` opens none, yet
/// attaches (code-review CR10).
#[test]
fn aldump_starts_no_phase_sampler() {
    let (out, events) = spawn_trace_events(
        env!("CARGO_BIN_EXE_aldump"),
        &["--r4-findings", &ws_a()],
        &[],
    );
    assert!(
        out.status.success() && count(&last_counters(&events), "build.program") > 0,
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        phase_events(&events).is_empty(),
        "{:?}",
        phase_events(&events)
    );
}

/// Code-review CR8: one snapshot base build opens `r3a3.graph_build` and
/// `r3a3.clone_inputs` once each, as it opens `r3a3.direct_facts` once; a span
/// count is a build count. Final panel F2-6: the narrowed graph span has a new
/// name, so no `r3a3.calls_graph` (which covered the clones too) is left.
#[test]
fn r3a3_spans_open_once_per_base() {
    let (out, events) = spawn_trace_events(
        env!("CARGO_BIN_EXE_alsem"),
        &[
            "fingerprint",
            &ws_a(),
            "--format",
            "json",
            "--deterministic",
        ],
        &[],
    );
    assert!(
        out.status.code().is_some(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let opened = |name: &str| {
        events
            .iter()
            .filter(|e| e["ph"] == "B" && e["name"] == name)
            .count()
    };
    let bases = opened("r3a3.direct_facts");
    assert!(bases > 0, "no snapshot base built");
    assert_eq!(opened("r3a3.graph_build"), bases);
    assert_eq!(opened("r3a3.clone_inputs"), bases);
    assert_eq!(opened("r3a3.calls_graph"), 0, "the old span name is back");
}

/// Spec §4.1.5: d47 fills the ordering cell after the context is built, so its
/// bucket is printed at the fill, not at the build where the cell is still
/// empty (code-review CR3). The fixture is a d47 positive, so the routine it
/// reports has ordering facts.
#[test]
fn retained_census_counts_the_filled_ordering_cell() {
    let ws = r0("ws-txn-d47-pos-http-nocommit");
    let (out, _) = spawn_traced(
        &[
            "analyze",
            ws.to_str().unwrap(),
            "--single-app",
            "--format",
            "json",
            "--detector",
            "d47-io-unsafe-txn",
        ],
        &[("ALSEM_RETAINED_CENSUS", "1")],
    );
    let err = String::from_utf8_lossy(&out.stderr);
    let bytes: Vec<u64> = err
        .lines()
        .filter_map(|l| l.split("bucket=context.ordering_facts bytes=").nth(1))
        .map(|r| r.split(' ').next().unwrap().parse().unwrap())
        .collect();
    assert!(
        bytes.len() == 1 && bytes[0] > 0,
        "ordering bucket {bytes:?}:\n{err}"
    );
}

/// Spec §4.1.5 / A0.5 on the fixture: `ALSEM_RETAINED_CENSUS=1` changes no
/// stdout byte, prints tagged bucket lines on stderr, and the old
/// `[C1_CONE_CENSUS]` report it replaces is gone. Spawned, because
/// `census::enabled()` is a process `OnceLock` read from the environment.
#[test]
fn retained_census_changes_no_output() {
    let (_d, ws) = ws_dep();
    // A1b.5's precondition, hand-stated: one workspace `.al` file carries a
    // 64 KiB comment, so the workspace texts outweigh every other input the
    // `inputs` bucket counts (paths, `app.json`, the closure list).
    let al_files: Vec<std::path::PathBuf> = walkdir::WalkDir::new(&ws)
        .into_iter()
        .filter_map(Result::ok)
        .map(walkdir::DirEntry::into_path)
        .filter(|p| p.extension().is_some_and(|x| x == "al"))
        .collect();
    let padded = al_files.first().expect("the workspace has an .al file");
    let mut text = std::fs::read_to_string(padded).unwrap();
    text.push_str(&format!("\n// {}\n", "x".repeat(64 * 1024)));
    std::fs::write(padded, text).unwrap();
    let args = ["analyze", ws.as_str(), "--format", "sarif"];
    let (off, _) = spawn_traced(&args, &[]);
    let (on, _) = spawn_traced(&args, &[("ALSEM_RETAINED_CENSUS", "1")]);
    assert!(
        off.status.code().is_some() && !off.stdout.is_empty(),
        "{}",
        String::from_utf8_lossy(&off.stderr)
    );
    assert_eq!(off.stdout, on.stdout, "census changed stdout");
    let err = String::from_utf8_lossy(&on.stderr);
    for b in ["bucket=model ", "bucket=program.report ", "bucket=context."] {
        assert!(err.contains(b), "missing {b}:\n{err}");
    }
    assert!(
        err.lines()
            .filter(|l| l.contains("bucket="))
            .all(|l| l.ends_with("kind=exact") || l.ends_with("kind=shallow")),
        "an untagged bucket line:\n{err}"
    );
    assert!(
        !err.contains("[C1_CONE_CENSUS]"),
        "old report still printed"
    );
    // Final panel F2-1: the cross-app base keeps the model's rows and events
    // when it replaces the calls, so it charges them. They are the `model`
    // bucket less its calls: nonzero, and below the whole model.
    let bytes = |name: &str| -> u64 {
        let tag = format!("bucket={name} bytes=");
        err.lines()
            .find_map(|l| l.split(&tag).nth(1))
            .and_then(|r| r.split(' ').next()?.parse().ok())
            .unwrap_or_else(|| panic!("missing {tag}:\n{err}"))
    };
    // #160 A1b.5: the `inputs` bucket owns the workspace `.al` texts, so it is
    // at least their bytes on disk (the dependency's are `.app` packages).
    let al_bytes: u64 = al_files
        .iter()
        .map(|p| std::fs::metadata(p).unwrap().len())
        .sum();
    assert!(
        bytes("inputs") >= al_bytes,
        "inputs {} < .al bytes {al_bytes}:\n{err}",
        bytes("inputs")
    );
    let rows = bytes("base.resolved.rows");
    assert!(
        rows > 0 && rows < bytes("model"),
        "base.resolved.rows {rows}:\n{err}"
    );
}

/// Spec §8.2 `inputs::read_once`: every command reads the workspace ONCE
/// (`inputs.read` 1), `diff ws ws` once per side. Spawned: the counter lives
/// only in the trace file.
#[test]
fn inputs_read_once() {
    let (_d, ws) = ws_dep();
    let ws = ws.as_str();
    for args in [
        vec!["analyze", ws, "--format", "json"],
        vec!["fingerprint", ws, "--format", "json"],
        vec!["query", "touches", ws, "--table", "18", "--format", "json"],
        // Builds through `AnalysisTarget::build` (as digest and policy check do).
        vec!["events", "fanout", ws, "--format", "json"],
    ] {
        let m = run(&args);
        assert_eq!(count(&m, "inputs.read"), 1, "{args:?}\n{m:?}");
    }
    let m = run(&["diff", ws, ws, "--format", "json", "--deterministic"]);
    assert_eq!(count(&m, "inputs.read"), 2, "diff: one per side\n{m:?}");
}

/// Code review CR1: a bad `--preset` is a usage error (exit 3) found BEFORE the
/// workspace is read, as it was before step 1 moved the read out of the pipeline.
#[test]
fn analyze_bad_preset_reads_no_inputs() {
    let (out, m) = spawn_traced(&["analyze", &ws_a(), "--preset", "bogus"], &[]);
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(3), "stderr: {err}");
    assert_eq!(count(&m, "inputs.read"), 0, "{m:?}\nstderr: {err}");
}

/// Final panel r1 (F2-2): `fingerprint --inventory-only`'s usage errors are
/// found before the workspace is read, as they were before step 1.
#[test]
fn fingerprint_inventory_only_usage_errors_read_no_inputs() {
    let ws = ws_a();
    let ws = ws.as_str();
    for (args, want) in [
        (vec!["--shard", "all"], "cannot be combined with --shard"),
        (vec!["--format", "human"], "requires --format=json"),
        (
            vec!["--format", "json", "--routine", "X"],
            "cannot be combined with query selectors",
        ),
    ] {
        let mut argv = vec!["fingerprint", ws, "--inventory-only"];
        argv.extend(args);
        let (out, m) = spawn_traced(&argv, &[]);
        let err = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(1), "{argv:?} stderr: {err}");
        assert!(err.contains(want), "{argv:?} stderr: {err}");
        assert_eq!(count(&m, "inputs.read"), 0, "{argv:?} {m:?}\nstderr: {err}");
    }
}

/// Review focus 1: a relative workspace path runs as the absolute one does
/// (`InputSet.workspace` is the path as spelled, and the build asserts it
/// equals the builder's root).
#[test]
fn inputs_relative_workspace_path_runs() {
    let rel = "tests/r0-corpus/ws-d1-multi-caller";
    let analyze = |ws: &str| {
        Command::new(env!("CARGO_BIN_EXE_alsem"))
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .args(["analyze", ws, "--format", "json", "--single-app"])
            .output()
            .expect("spawn alsem")
    };
    let out = analyze(rel);
    let abs = analyze(&ws_a());
    assert_eq!(
        out.status.code(),
        abs.status.code(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let findings = |o: &Output| {
        serde_json::from_slice::<Value>(&o.stdout).expect("json")["payload"]["findings"]
            .as_array()
            .map(Vec::len)
    };
    assert!(
        findings(&abs).is_some_and(|n| n > 0),
        "precondition: findings"
    );
    assert_eq!(findings(&out), findings(&abs));
}

#[test]
fn graph() {
    let (_d, ws) = ws_dep();
    let m = run(&["graph", &ws]);
    assert_row(
        "graph",
        &m,
        Row {
            program: 1,
            bcg: 0,
            cone: 0,
            v2: 0,
            scc: 0,
            pd: 0,
            ctx: 0,
            snap: 0,
        },
    );
}

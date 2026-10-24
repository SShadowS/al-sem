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

/// #277 R5-2: `analyze.total` spans the whole analyze run, model build
/// included, as before the batch driver: `scripts/trace_summary.py` takes its
/// percentages off it. Precondition: the trace holds one `analyze.total` span
/// and a `build.program` counter.
#[test]
fn analyze_total_spans_the_build() {
    let (out, events) = spawn_trace_events(
        env!("CARGO_BIN_EXE_alsem"),
        &["analyze", &ws_a(), "--format", "json"],
        &[],
    );
    assert_eq!(out.status.code(), Some(0));
    let ts = |ph: &str, name: &str| -> Vec<f64> {
        events
            .iter()
            .filter(|e| e["ph"] == ph && e["name"] == name)
            .filter_map(|e| e["ts"].as_f64())
            .collect()
    };
    let (begin, end) = (ts("B", "analyze.total"), ts("E", "analyze.total"));
    assert_eq!((begin.len(), end.len()), (1, 1), "precondition: one span");
    let build = ts("C", "build.program");
    assert!(!build.is_empty(), "precondition: a program build");
    assert!(
        build.iter().all(|t| begin[0] <= *t && *t <= end[0]),
        "build.program at {build:?} outside analyze.total [{}, {}]",
        begin[0],
        end[0]
    );
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
    /// `build.witness_pass`: compositions that materialized raw facts (#160
    /// step 6 §4.9, §14.10).
    pub witness: u64,
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
        ("build.witness_pass", r.witness),
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
// §4.4.8 rows (after step 4; every row equals child 0-2 §4.4.8's "After step
// 4" table, 2026-10-20). Among the rows pinned before step 4, only `bcg`
// moved, and only in the rows that build a cross-app base, -1 each: the base's
// dependency half-build is no longer a counted `build_combined_graph`, so a
// relation counts one; no `scc` of those rows moved (the Tarjan moved into
// `ExecRelation::new`). Two rows are new at step 4:
// `analyze_cross_app_with_a_dependency_half` (bcg 1, scc 2) and
// `analyze_single_app_d47`, whose `scc` 2 includes the relation's Tarjan, new
// there because that plan fills no `l4`. The rule (M4): `bcg` = one per
// constructed relation + one per legacy-cut graph; `scc` = one per
// constructed relation + one per cone composition.
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
            bcg: 1,
            cone: 1,
            v2: 1,
            scc: 2,
            pd: 1,
            ctx: 1,
            snap: 0,
            witness: 0,
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
            bcg: 2,
            cone: 1,
            v2: 0,
            scc: 2,
            pd: 1,
            ctx: 1,
            snap: 1,
            witness: 1,
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
            witness: 0,
        },
    );
}

/// #160 step 4 (review of Tasks 4-5, finding 1): the cross-app default row on
/// a workspace whose dependency half is NOT empty. `ws_dep`'s dependency
/// makes no call, so `ExecRelation::new` skips its second `assemble` there;
/// here the workspace calls the dependency's `Outer()`, which calls its
/// `Inner()`, so the half-build runs (the typed edge it adds is what
/// `cross_app_commit_span_sees_dependency_writes` reads). One relation still
/// counts one `build.combined_graph`.
///
/// DISCRIMINATION (2026-10-20): building the dependency half through the
/// counted `build_combined_graph` inside `new`'s non-empty guard (spec
/// §5.5 (a), as specified) fails this with `build.combined_graph: got 2,
/// want 1`; restored, it passes. The `ws_dep` rows stay green under that
/// break.
#[test]
fn analyze_cross_app_with_a_dependency_half() {
    let dir = tempfile::tempdir().unwrap();
    crate::dep_dispatch_fixture::write_own(
        dir.path(),
        "codeunit 50172 \"Ws Commit\"\n{\n    procedure CommitAfterDep()\n    var\n        Lib: Codeunit \"Dep Lib\";\n    begin\n        Lib.Outer();\n        Commit();\n    end;\n}\n",
    );
    let ws = dir.path().to_string_lossy().into_owned();
    let m = run(&["analyze", &ws, "--format", "json"]);
    assert_row(
        "analyze cross-app, dependency half",
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
            witness: 0,
        },
    );
}

/// #160 step 4 (child spec §4.9, stated, now pinned): a single-app plan that
/// fills the relation but no `l4` (d47 alone: no `CORE_SUMMARIES`) runs the
/// relation's Tarjan, which before step 4 lived in `build_l4` and so did not
/// run here. A later lazy-SCC change must move this row on purpose.
#[test]
fn analyze_single_app_d47() {
    let m = run(&[
        "analyze",
        &ws_a(),
        "--single-app",
        "--format",
        "json",
        "--detector",
        "d47-io-unsafe-txn",
    ]);
    assert_row(
        "analyze single-app d47",
        &m,
        Row {
            program: 1,
            bcg: 2,
            cone: 1,
            v2: 0,
            scc: 2,
            pd: 1,
            ctx: 1,
            snap: 1,
            witness: 1,
        },
    );
}

/// #160 step 6 §14.10 (A1-18), T5: d47 beside a summaries detector is two
/// compositions, the view's one (derived rows, no raw) and the ordering cell's
/// cut (raw for every routine, so `witness` 1). Under the split the ordering
/// cell keeps the cut until the #161 follow-up.
#[test]
fn analyze_cross_app_d47_with_a_summaries_detector() {
    let (_d, ws) = ws_dep();
    let m = run(&[
        "analyze",
        &ws,
        "--format",
        "json",
        "--detector",
        "d47-io-unsafe-txn,d35-commit-in-event-subscriber",
    ]);
    assert_row(
        "analyze cross-app d47 + d35",
        &m,
        Row {
            program: 1,
            bcg: 2,
            cone: 2,
            v2: 1,
            scc: 3,
            pd: 2,
            ctx: 1,
            snap: 1,
            witness: 1,
        },
    );
}

/// #160 step 6 §4.9 (T5): digest composes once, over the view's relation:
/// the tree takes the composition's raw facts and its pd arena, so there is no
/// cut graph, no second cone, no second SCC and no second producer solve.
const DIGEST_ROW: Row = Row {
    program: 1,
    bcg: 1,
    cone: 1,
    v2: 1,
    scc: 2,
    pd: 1,
    ctx: 1,
    snap: 1,
    witness: 1,
};

#[test]
fn digest_cross_app() {
    let (d, ws) = ws_dep();
    let diff = write_diff(d.path(), "src/Work.al", 18, "        Customer.Insert();");
    let m = run(&["digest", &ws, "--diff", &diff, "--deterministic"]);
    assert_row("digest cross-app", &m, DIGEST_ROW);
}

/// Review Focus 2: a digest that resolves no root keeps no witness request
/// and builds no tree, and exits 2.
#[test]
fn digest_with_no_resolved_root_builds_no_tree() {
    let (_d, ws) = ws_dep();
    let (out, m) = spawn_traced(&["digest", &ws, "--routine", "NoSuchRoutine"], &[]);
    assert_eq!(out.status.code(), Some(2), "{out:?}");
    assert_eq!(
        count(&m, "build.detector_context"),
        1,
        "precondition: the run\n{m:?}"
    );
    assert_eq!(count(&m, "build.snapshot"), 0, "{m:?}");
    assert_eq!(count(&m, "build.witness_pass"), 0, "{m:?}");
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
    assert_row("digest single-app", &m, DIGEST_ROW);
}

/// #160 step 9 §4.6 (T16): prove is digest with one root, cross-app by
/// default: one composition over the view's relation, its tree read inside
/// `prepare` (no cut, so no second graph, cone, SCC or producer solve).
#[test]
fn prove() {
    let (_d, ws) = ws_dep();
    let m = run(&["prove", &ws, "CallLib", "may-commit", "--deterministic"]);
    assert_row("prove", &m, DIGEST_ROW);
}

/// T16: `--single-app` takes the same path over the workspace alone.
#[test]
fn prove_single_app() {
    let (_d, ws) = ws_dep();
    let m = run(&[
        "prove",
        &ws,
        "CallLib",
        "may-commit",
        "--deterministic",
        "--single-app",
    ]);
    assert_row("prove single-app", &m, DIGEST_ROW);
}

/// T16: a prove that resolves no root keeps no witness request and builds no
/// tree, as digest's no-root case; it exits 2 with the dummy document.
#[test]
fn prove_no_root() {
    let (_d, ws) = ws_dep();
    let (out, m) = spawn_traced(&["prove", &ws, "NoSuchRoutine", "may-commit"], &[]);
    assert_eq!(out.status.code(), Some(2), "{out:?}");
    assert_eq!(
        count(&m, "build.detector_context"),
        1,
        "precondition: the run\n{m:?}"
    );
    assert_eq!(count(&m, "build.snapshot"), 0, "{m:?}");
    assert_eq!(count(&m, "build.witness_pass"), 0, "{m:?}");
}

/// Spec §4.4.4 (A8): digest's no-input check runs right after the target
/// build, before `prepare` and before `compose_snapshot`. `build.snapshot`
/// witnesses the snapshot directly: since #160 step 3 the snapshot's coverage
/// fold no longer solves, so `build.summaries_v2` no longer witnesses it.
#[test]
fn run_digest_empty_diff_builds_no_context() {
    let d = tempfile::tempdir().unwrap();
    let diff = d.path().join("empty.diff");
    std::fs::write(&diff, "  \n").unwrap();
    let (out, m) = spawn_traced(&["digest", &ws_a(), "--diff", diff.to_str().unwrap()], &[]);
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains(
            "digest: at least one of changedFiles, changedRoutines, or diffText is required"
        ),
        "{stderr}"
    );
    assert_eq!(
        count(&m, "build.program"),
        1,
        "precondition: the target was built\n{m:?}"
    );
    assert_eq!(
        count(&m, "build.detector_context"),
        0,
        "the no-input check must run before prepare\n{m:?}"
    );
    assert_eq!(
        count(&m, "build.snapshot"),
        0,
        "the no-input check must run before compose_snapshot\n{m:?}"
    );
    assert_eq!(count(&m, "build.summaries_v2"), 0, "{m:?}");
}

const FINGERPRINT_ROW: Row = Row {
    program: 1,
    bcg: 2,
    cone: 2,
    v2: 1,
    scc: 3,
    pd: 2,
    ctx: 1,
    snap: 1,
    witness: 1,
};

#[test]
fn fingerprint_json() {
    let (_d, ws) = ws_dep();
    let m = run(&["fingerprint", &ws, "--format", "json", "--deterministic"]);
    assert_row("fingerprint json", &m, FINGERPRINT_ROW);
}

/// #160 step 9 §4.6 (T16): the `--routine` query reads a tree over the view's
/// relation (cross-app by default): digest's row, no cut.
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
    assert_row("fingerprint query", &m, DIGEST_ROW);
}

/// T16: `--single-app` takes the same path over the workspace alone.
#[test]
fn fingerprint_query_single_app() {
    let (_d, ws) = ws_dep();
    let m = run(&[
        "fingerprint",
        &ws,
        "--format",
        "json",
        "--routine",
        "CallLib",
        "--deterministic",
        "--single-app",
    ]);
    assert_row("fingerprint query single-app", &m, DIGEST_ROW);
}

/// #160 step 9 final panel S2 (spec §4.5): the `--routine` query's tree dies
/// before `det` is built, read off the shipped binary's trace in both
/// universes: `rel:Tree` comes before `ctx` (`build.detector_context`, the
/// first line of `build_detector_artifacts`). The whole sequence is pinned
/// too. DISCRIMINATION: moving `prepare`'s tree release after the det fill
/// puts `rel:Tree` after `ctx` and fails it.
#[test]
fn routine_query_releases_its_tree_before_det() {
    let (_d, ws) = ws_dep();
    for extra in [&[][..], &["--single-app"][..]] {
        let mut args = vec![
            "fingerprint",
            ws.as_str(),
            "--format",
            "json",
            "--routine",
            "CallLib",
            "--deterministic",
        ];
        args.extend_from_slice(extra);
        let got = release_sequence(&args);
        let at = |t: &str| got.iter().position(|c| c == t);
        let (tree, ctx) = (at("rel:Tree"), at("ctx"));
        assert!(
            tree.is_some() && ctx.is_some(),
            "precondition: {extra:?} {got:?}"
        );
        assert!(tree < ctx, "{extra:?}: the tree outlives det: {got:?}");
        assert_eq!(
            got,
            [
                "snap",
                "rel:Raw",
                "rel:Tree",
                "ctx",
                "rel:Caps",
                "rel:TreeRead",
                "rel:Det",
                "rel:L4",
                "rel:Ordering",
                "rel:Rel",
            ],
            "{extra:?}"
        );
    }
}

/// #160 step 9 final panel O4: a `--routine` whose selector fails composes no
/// tree (`build.snapshot` 0); it still runs the default set, for the
/// envelope's diagnostics. Precondition: the same command with a selector that
/// resolves composes one. DISCRIMINATION: sending a failing selector down the
/// tree path (`Err(diags) =>` taking the query plan) gives `build.snapshot` 1.
#[test]
fn failing_routine_selector_composes_no_tree() {
    let (_d, ws) = ws_dep();
    let snaps = |routine: &str| {
        let (out, m) = spawn_traced(
            &[
                "fingerprint",
                &ws,
                "--format",
                "json",
                "--routine",
                routine,
                "--deterministic",
            ],
            &[],
        );
        (
            out.status.code(),
            count(&m, "build.snapshot"),
            count(&m, "build.detector_context"),
        )
    };
    assert_eq!(snaps("CallLib"), (Some(0), 1, 1), "precondition");
    assert_eq!(snaps("NoSuchRoutine"), (Some(2), 0, 1));
}

/// #160 step 9 T21 (S′, spec §4.3): only a query with `--routine` moves to the
/// view's tree. Precondition: on `ws_dep()` the cross-app `--routine CallLib`
/// output differs from its `--single-app` one. Assertion: the human query
/// with no flag and the `--roots` query with no `--routine` are byte-equal
/// with and without `--single-app`, and stay on the cut (`build.combined_graph`
/// 2, `FINGERPRINT_ROW`). DISCRIMINATION: routing every query form to the new
/// path gives `combined_graph` 1 and different bytes.
#[test]
fn fingerprint_wide_query_stays_single_app() {
    let (_d, ws) = ws_dep();
    let routine = |extra: &[&str]| {
        let mut a = vec![
            "fingerprint",
            ws.as_str(),
            "--format",
            "json",
            "--routine",
            "CallLib",
            "--deterministic",
        ];
        a.extend_from_slice(extra);
        let (out, _) = spawn_traced(&a, &[]);
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    assert_ne!(
        routine(&[]),
        routine(&["--single-app"]),
        "precondition: the universes differ on ws_dep"
    );
    for form in [
        &["--format", "human"][..],
        &["--format", "json", "--roots", "public-procedure"][..],
    ] {
        let mut outs = Vec::new();
        for extra in [&[][..], &["--single-app"][..]] {
            let mut a = vec!["fingerprint", ws.as_str(), "--deterministic"];
            a.extend_from_slice(form);
            a.extend_from_slice(extra);
            let (out, m) = spawn_traced(&a, &[]);
            assert_eq!(out.status.code(), Some(0), "{a:?}: {out:?}");
            assert_row(&format!("{a:?}"), &m, FINGERPRINT_ROW);
            outs.push(out.stdout);
        }
        assert!(
            !outs[0].is_empty() && outs[0] == outs[1],
            "{form:?}: equal with and without --single-app"
        );
    }
}

/// #160 step 9 §4.6 (T16, T11): `--inventory-only` reads a tree over the
/// view's relation composed with no raw facts (`Roots(∅)`): digest's row with
/// no witness pass.
const INVENTORY_ROW: Row = Row {
    witness: 0,
    ..DIGEST_ROW
};

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
    assert_row("fingerprint --inventory-only", &m, INVENTORY_ROW);
}

/// T16: `--single-app` takes the same path over the workspace alone.
#[test]
fn fingerprint_inventory_only_single_app() {
    let (_d, ws) = ws_dep();
    let m = run(&[
        "fingerprint",
        &ws,
        "--inventory-only",
        "--format",
        "json",
        "--deterministic",
        "--single-app",
    ]);
    assert_row("fingerprint --inventory-only single-app", &m, INVENTORY_ROW);
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
            v2: 0,
            scc: 2,
            pd: 2,
            ctx: 0,
            snap: 2,
            witness: 2,
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
            v2: 2,
            scc: 6,
            pd: 4,
            ctx: 2,
            snap: 2,
            witness: 2,
        },
    );
}

const EVENTS_CROSS_ROW: Row = Row {
    program: 1,
    bcg: 1,
    cone: 1,
    v2: 1,
    scc: 2,
    pd: 1,
    ctx: 1,
    snap: 0,
    witness: 0,
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
            bcg: 1,
            cone: 1,
            v2: 1,
            scc: 2,
            pd: 1,
            ctx: 1,
            snap: 0,
            witness: 0,
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
            bcg: 1,
            cone: 1,
            v2: 1,
            scc: 2,
            pd: 1,
            ctx: 1,
            snap: 0,
            witness: 1,
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
            bcg: 1,
            cone: 1,
            v2: 1,
            scc: 2,
            pd: 1,
            ctx: 1,
            snap: 0,
            witness: 1,
        },
    );
}

const QUERY_CROSS_ROW: Row = Row {
    program: 1,
    bcg: 1,
    cone: 0,
    v2: 1,
    scc: 1,
    pd: 0,
    ctx: 0,
    snap: 0,
    witness: 0,
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
            witness: 0,
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

/// #160 step 7a §4.5/§6: the census `cone_scc:` line (the stop rule's S1
/// instrument) is fed from `compose_inherited_cones`' `ComposeStats`.
/// `ws-d7-event-cycle` holds one recursive 6-member root SCC: its set (3
/// entries) is built once and folded by each of the 6 members, with no walk, so
/// `walk_total` is exactly 0.0 ms. The SCC has no successor and no own entry.
#[test]
fn census_cone_scc_line_counts_the_recursive_sccs() {
    let (out, _) = spawn_traced(
        &[
            "analyze",
            &r0("ws-d7-event-cycle").to_string_lossy(),
            "--single-app",
            "--format",
            "json",
        ],
        &[("ALSEM_CONES_CENSUS", "1")],
    );
    let err = String::from_utf8_lossy(&out.stderr);
    let scc: Vec<&str> = err
        .lines()
        .filter(|l| l.starts_with("[cones-census]   cone_scc: "))
        .collect();
    assert_eq!(scc.len(), 1, "one cone_scc line:\n{err}");
    assert_eq!(
        scc[0],
        "[cones-census]   cone_scc: recursive=1 members_all=6 members_emitted=6 roots=1 \
         self_loops=0 max=6 sizes=1:0 2:0 3-9:1 10-99:0 100+:0 member_walks=0 own_entries=0 \
         sets_borrowed=0 sets_built=1 set_entries=18 walk_total=0.0ms own_entry_members=0 \
         with_successor=0 with_guarded_successor=0"
    );
    // S1: members_emitted equals the compose line's recursive emission count.
    let compose = err
        .lines()
        .find(|l| l.starts_with("[cones-census]   compose: "))
        .unwrap_or_else(|| panic!("no compose line:\n{err}"));
    let field = |line: &str, key: &str| -> String {
        let rest = line
            .split(key)
            .nth(1)
            .unwrap_or_else(|| panic!("{key} in {line}"));
        rest.split(' ').next().unwrap().to_string()
    };
    assert_eq!(
        field(scc[0], " members_emitted="),
        field(compose, "ms x"),
        "{compose}"
    );
}

/// #160 step 7b §4.8/§5 T9: the census `closed:` line. `ws-compose` has one
/// singleton reading a callee cone with a closed entry (at the 7b base its
/// `singleton pop:` line reads `closed_entries=1`), so at least one memo row is
/// built.
#[test]
fn census_closed_line_counts_the_memo() {
    let (out, _) = spawn_traced(
        &[
            "analyze",
            &r0("ws-compose").to_string_lossy(),
            "--single-app",
            "--format",
            "json",
        ],
        &[("ALSEM_CONES_CENSUS", "1")],
    );
    let err = String::from_utf8_lossy(&out.stderr);
    let closed: Vec<&str> = err
        .lines()
        .filter(|l| l.starts_with("[cones-census]   closed: "))
        .collect();
    assert_eq!(closed.len(), 1, "one closed line:\n{err}");
    let builds: u64 = closed[0]
        .split(" memo_builds=")
        .nth(1)
        .and_then(|r| r.split(' ').next())
        .unwrap_or_else(|| panic!("memo_builds in {}", closed[0]))
        .parse()
        .unwrap();
    assert!(builds > 0, "no memo row built: {}", closed[0]);
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

/// The trace of `alsem <args>` as tokens, in order: `ctx` per
/// `build.detector_context`, `snap` per `build.snapshot`, `rel:<Cell>` per
/// `view.release` (`View::release`'s instant).
fn release_sequence(args: &[&str]) -> Vec<String> {
    release_sequence_exit(args, 0)
}

/// [`release_sequence`] of a run that exits `code`.
fn release_sequence_exit(args: &[&str], code: i32) -> Vec<String> {
    let (out, events) = spawn_trace_events(env!("CARGO_BIN_EXE_alsem"), args, &[]);
    assert_eq!(
        out.status.code(),
        Some(code),
        "alsem {args:?}\nstderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    events
        .iter()
        .filter_map(|e| {
            let ph = e.get("ph").and_then(Value::as_str);
            match (ph, e.get("name").and_then(Value::as_str)) {
                (Some("C"), Some("build.detector_context")) => Some("ctx".to_string()),
                (Some("C"), Some("build.snapshot")) => Some("snap".to_string()),
                (Some("i"), Some("view.release")) => Some(format!(
                    "rel:{}",
                    e["args"]["cell"].as_str().unwrap_or("<no cell>")
                )),
                _ => None,
            }
        })
        .collect()
}

/// #160 step 9 (Task 2 review item 4): a prove that resolves no root drops its
/// cells in digest's no-root order, `l4` and `Effects` included, before its
/// exit 2. Precondition: the sequence holds `l4`'s release. DISCRIMINATION:
/// deleting the dummy branch's `release(Cell::L4)` / `release(Cell::Effects)`
/// in `prove_on_target` fails it.
#[test]
fn prove_with_no_root_releases_as_digest_does() {
    let (_d, ws) = ws_dep();
    let digest = release_sequence_exit(&["digest", &ws, "--routine", "NoSuchRoutine"], 2);
    let prove = release_sequence_exit(&["prove", &ws, "NoSuchRoutine", "may-commit"], 2);
    assert!(
        digest.iter().any(|c| c == "rel:L4"),
        "precondition: {digest:?}"
    );
    assert_eq!(prove, digest);
}

/// #160 final panel P1-5: each command drops its cells where the code says,
/// read off the shipped binary's trace. Policy check releases `l4`, the
/// ordering facts and the relation right after the run, and `det` and the raw
/// facts after its rule engine (#160 step 6 Task 3, D7 (7)); fingerprint and
/// each `diff --strict` side release every
/// cell before snapshot composition; events releases `l4` right after the run
/// and the rest after the report. Digest and prove (`finish_default_run`)
/// release `det`, the ordering facts and the relation before the snapshot and
/// `l4` after the query (§4.4.4). The relation's cell is `Cell::Rel` since
/// #160 step 4 (it was `Cell::Graph`).
///
/// DISCRIMINATION: deleting policy check's `release_all_but(&[Cell::Det,
/// Cell::Raw])` before the rule engine fails the policy case; deleting
/// `finish_default_run`'s `release_all_but(&[Cell::L4])` fails digest and
/// prove.
#[test]
fn views_release_their_cells_before_the_next_build() {
    let (d, ws) = ws_dep();
    let policy = policy_yaml();
    let diff = write_diff(d.path(), "src/Work.al", 18, "        Customer.Insert();");
    // `TreeRead` is released as `Raw` is, filled or not (step 9 final panel C8).
    let all = [
        "rel:Det",
        "rel:L4",
        "rel:Ordering",
        "rel:Rel",
        "rel:Raw",
        "rel:TreeRead",
    ];
    // #160 step 6 D7 (6): the det fill consumes the view's one composition
    // and `prepare` releases `Caps` right after it.
    let det = ["ctx", "rel:Caps"];
    let per_side: Vec<&str> = [&det[..], &all, &["snap"]].concat();
    // #160 step 6 D7: the tree first (it takes `Raw`, and is released once
    // the effects are read off it), then the run; `Effects` dies after the
    // query, beside `l4`.
    let tree_run = vec![
        "snap",
        "rel:Raw",
        "rel:Tree",
        "ctx",
        "rel:Caps",
        "rel:Det",
        "rel:Ordering",
        "rel:Rel",
        "rel:TreeRead",
        "rel:L4",
        "rel:Effects",
    ];
    let cases: [(&str, Vec<&str>, Vec<&str>); 6] = [
        (
            "policy check",
            vec![
                "policy", "check", &ws, "--policy", &policy, "--format", "json",
            ],
            vec![
                "ctx",
                "rel:Caps",
                "rel:L4",
                "rel:Ordering",
                "rel:Rel",
                "rel:TreeRead",
                "rel:Det",
                "rel:Raw",
            ],
        ),
        (
            "fingerprint",
            vec!["fingerprint", &ws, "--format", "json"],
            per_side.clone(),
        ),
        (
            "diff --strict",
            vec!["diff", &ws, &ws, "--format", "json", "--strict"],
            [&per_side[..], &per_side].concat(),
        ),
        (
            // `release_all_but` skips the `l4` events already released.
            "events fanout",
            vec!["events", "fanout", &ws, "--format", "json"],
            [&det[..], &["rel:L4"], &all[..1], &all[2..]].concat(),
        ),
        (
            "digest",
            vec!["digest", &ws, "--diff", &diff, "--deterministic"],
            tree_run.clone(),
        ),
        (
            // #160 step 9 D2: prove is digest with one root.
            "prove",
            vec!["prove", &ws, "CallLib", "may-commit", "--deterministic"],
            tree_run,
        ),
    ];
    let mut bad = Vec::new();
    for (name, args, want) in cases {
        let got = release_sequence(&args);
        if got != want {
            bad.push(format!("{name}: got {got:?}, want {want:?}"));
        }
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

/// #160 step 6 §4.7 (R1-30, A6.18), the live-cell checkpoint: at digest's
/// first run the view holds its effects, but no tree, no pd arena and no raw
/// facts (D7: the tree and the composition's raw half die before `det`).
#[test]
fn digest_holds_no_tree_at_its_first_run() {
    let (d, ws) = ws_dep();
    let diff = write_diff(d.path(), "src/Work.al", 18, "        Customer.Insert();");
    let (out, events) = spawn_trace_events(
        env!("CARGO_BIN_EXE_alsem"),
        &["digest", &ws, "--diff", &diff, "--deterministic"],
        &[],
    );
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let live: Vec<Vec<String>> = events
        .iter()
        .filter(|e| e.get("name").and_then(Value::as_str) == Some("view.live"))
        .map(|e| {
            e["args"]["cells"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|c| c.as_str().map(str::to_string))
                .collect()
        })
        .collect();
    assert_eq!(live.len(), 1, "one checkpoint: {live:?}");
    let cells = &live[0];
    assert!(
        cells.contains(&"Effects".to_string()),
        "precondition: {cells:?}"
    );
    for gone in ["Tree", "PdArena", "Raw", "Caps"] {
        assert!(
            !cells.contains(&gone.to_string()),
            "{gone} alive: {cells:?}"
        );
    }
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

/// #160 step 3: `project_r3a3` (`aldump --r3a3-cone-coverage`) takes its
/// coverage reasons from a scan, so it runs no L4 solve; its one Tarjan is the
/// cone's typed-edge SCC.
#[test]
fn aldump_r3a3_runs_no_solve() {
    let (out, events) = spawn_trace_events(
        env!("CARGO_BIN_EXE_aldump"),
        &["--r3a3-cone-coverage", &ws_a()],
        &[],
    );
    let m = last_counters(&events);
    assert!(
        out.status.success() && count(&m, "build.program") > 0,
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(count(&m, "build.summaries_v2"), 0, "{m:?}");
    assert_eq!(count(&m, "build.graph_scc"), 1, "{m:?}");
}

/// Code-review CR8: one snapshot base build opens `r3a3.graph_build` and
/// `r3a3.clone_inputs` once each, as it opens `r3a3.direct_facts_scan` once; a
/// span count is a build count. Final panel F2-6: the narrowed graph span has a
/// new name, so no `r3a3.calls_graph` (which covered the clones too) is left.
/// #160 step 3: likewise no `r3a3.direct_facts`, which also covered the
/// coverage fold's L4 solve.
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
    let bases = opened("r3a3.direct_facts_scan");
    assert!(bases > 0, "no snapshot base built");
    assert_eq!(opened("r3a3.graph_build"), bases);
    assert_eq!(opened("r3a3.clone_inputs"), bases);
    assert_eq!(opened("r3a3.calls_graph"), 0, "the old span name is back");
    assert_eq!(opened("r3a3.direct_facts"), 0, "the old span name is back");
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

/// #160 A4, after step 4: a single-app analyze computes its SCC once, in the
/// view's `ExecRelation::new`, so one `rel.scc` bucket is printed (by
/// `ExecRelation::single_app`, where `context.graph` printed before) and no
/// `l4.scc` (`build_l4` shares the relation's SCC).
///
/// DISCRIMINATION (2026-10-20, final panel F1-9): dropping `single_app`'s
/// `emit_census` call fails this with `rel.scc lines: left 0, right 1`. The
/// stderr ORDER is not pinned: the `rel.*` lines sit right after `model`
/// whether `new` or the owner prints them.
#[test]
fn retained_census_counts_the_single_app_scc_once() {
    let ws = r0("ws-d8-commit-in-tx");
    let (out, _) = spawn_traced(
        &[
            "analyze",
            ws.to_str().unwrap(),
            "--single-app",
            "--format",
            "json",
        ],
        &[("ALSEM_RETAINED_CENSUS", "1")],
    );
    let err = String::from_utf8_lossy(&out.stderr);
    let lines = |b: &str| err.lines().filter(|l| l.contains(b)).count();
    assert_eq!(lines("bucket=rel.scc "), 1, "rel.scc lines:\n{err}");
    assert_eq!(lines("bucket=l4.scc "), 0, "l4.scc lines:\n{err}");
}

/// #160 A4, the cross-app half, after step 4: the base builds the one
/// relation and prints its `rel.graph` / `rel.scc` once, with the base's
/// census (after `census.base`, where `base.graph` printed before); the
/// `View` takes it by `Arc::clone`, so no `base.graph`, `base.combined_scc`,
/// `context.graph` or `l4.scc` line is printed.
///
/// DISCRIMINATION (2026-10-20, final panel F1-9): dropping the base census's
/// `b.rel.emit_census()` fails this with `rel.graph lines: left 0, right 1`.
#[test]
fn retained_census_counts_the_cross_app_graph_and_scc_on_the_relation() {
    let (_d, ws) = ws_dep();
    let (out, _) = spawn_traced(
        &["analyze", &ws, "--format", "json"],
        &[("ALSEM_RETAINED_CENSUS", "1")],
    );
    let err = String::from_utf8_lossy(&out.stderr);
    let lines = |b: &str| err.lines().filter(|l| l.contains(b)).count();
    assert_eq!(lines("bucket=rel.graph "), 1, "rel.graph lines:\n{err}");
    assert_eq!(lines("bucket=rel.scc "), 1, "rel.scc lines:\n{err}");
    assert_eq!(lines("bucket=base.graph "), 0, "base.graph lines:\n{err}");
    assert_eq!(
        lines("bucket=base.combined_scc "),
        0,
        "base.combined_scc lines:\n{err}"
    );
    assert_eq!(lines("bucket=l4.scc "), 0, "l4.scc lines:\n{err}");
    assert_eq!(
        lines("bucket=context.graph "),
        0,
        "context.graph lines:\n{err}"
    );
}

/// #160 §4.4.4: `View::prepare`'s det fill opens the span the context build
/// had before the View, once per analyze: `crossapp.context_build` by
/// default, `context.build_total` under `--single-app`.
#[test]
fn analyze_det_fill_keeps_the_context_build_span() {
    let (_d, ws) = ws_dep();
    for (extra, span) in [
        (None, "crossapp.context_build"),
        (Some("--single-app"), "context.build_total"),
    ] {
        let mut args = vec!["analyze", ws.as_str(), "--format", "json"];
        args.extend(extra);
        let (_, events) = spawn_trace_events(env!("CARGO_BIN_EXE_alsem"), &args, &[]);
        let begins = events
            .iter()
            .filter(|e| e["ph"] == "B" && e["name"] == span)
            .count();
        assert_eq!(begins, 1, "{args:?}: `{span}` spans");
    }
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
    for b in [
        "bucket=model ",
        "bucket=program.report ",
        "bucket=context.",
        // The borrowed maps of the detector context (#160 task 21 review I-1).
        "bucket=context.fingerprint_index ",
        "bucket=l4.",
    ] {
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
    // Code review CR11: and `program.snapshot_texts` leaves them to `inputs`
    // (counted in both, the 64 KiB pad would be in both): it holds only the
    // dependency's texts, which are far below the pad.
    assert!(
        bytes("program.snapshot_texts") < 64 * 1024,
        "program.snapshot_texts {} counts the workspace texts:\n{err}",
        bytes("program.snapshot_texts")
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
/// (`WorkspaceInputs.workspace` is the path as spelled, and the build asserts it
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

/// One `.al` file `alsem` cannot read, beside `app_json`; `None` = skip.
pub(crate) fn unreadable_ws(
    app_json: &str,
) -> Option<(tempfile::TempDir, crate::unreadable::Guard)> {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("app.json"), app_json).unwrap();
    let f = d.path().join("A.al");
    std::fs::write(&f, "codeunit 50100 A { }").unwrap();
    let Some(g) = crate::unreadable::unreadable(&f) else {
        eprintln!("skipped: cannot make {} unreadable here", f.display());
        return None;
    };
    Some((d, g))
}

/// Spec §4.4.5 row 2: a failed cross-app build is classified from its own
/// error; the single-app rerun is gone.
#[test]
fn run_build_failed_row_runs_one_build() {
    let Some((d, _g)) = unreadable_ws(
        r#"{"id":"11111111-1111-1111-1111-111111111111","name":"A","publisher":"p","version":"1.0.0.0"}"#,
    ) else {
        return;
    };
    let (out, m) = spawn_traced(
        &["analyze", d.path().to_str().unwrap(), "--format", "json"],
        &[],
    );
    assert_eq!(
        count(&m, "build.program"),
        1,
        "the single-app rerun is gone\n{m:?}"
    );
    assert_eq!(out.status.code(), Some(2));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("analysis failure — program engine build failed: ")
            && err.contains("reading source "),
        "{err}"
    );
}

/// The same build-failed row through `AnalysisTarget::build` (digest; events,
/// policy check and query share its message): the failure line carries the
/// program build's own error. DISCRIMINATION: `.map_err(|_| ..)` with the bare
/// line fails this (no `reading source `).
#[test]
fn digest_build_failed_row_carries_the_build_error() {
    let Some((d, _g)) = unreadable_ws(
        r#"{"id":"11111111-1111-1111-1111-111111111111","name":"A","publisher":"p","version":"1.0.0.0"}"#,
    ) else {
        return;
    };
    let diff = write_diff(d.path(), "A.al", 1, "codeunit 50100 A { }");
    let (out, m) = spawn_traced(
        &["digest", d.path().to_str().unwrap(), "--diff", &diff],
        &[],
    );
    assert_eq!(count(&m, "build.program"), 1, "{m:?}");
    assert_eq!(out.status.code(), Some(1));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains(
            "digest: the cross-app model could not be built \
             (--single-app analyses the workspace alone): program engine build failed: "
        ) && err.contains("reading source "),
        "{err}"
    );
}

/// Spec §4.4.5 row 1 (A2.7): no id, one `Workspace` build, and the warning
/// text captured from the binary before step 2 (Windows: the sharing
/// violation's OS text). The text is the same under both profiles (the
/// snapshot fails first), so the profile is pinned by the trace: a `CrossApp`
/// build opens `crossapp.program_build_full`.
#[test]
fn run_no_id_row_runs_one_build_and_keeps_the_text() {
    let Some((d, _g)) = unreadable_ws(r#"{"name":"A","publisher":"p","version":"1.0.0.0"}"#) else {
        return;
    };
    let ws = d.path().to_str().unwrap();
    let (out, events) = spawn_trace_events(
        env!("CARGO_BIN_EXE_alsem"),
        &["analyze", ws, "--format", "json"],
        &[],
    );
    let m = last_counters(&events);
    assert_eq!(count(&m, "build.program"), 1, "{m:?}");
    assert!(
        !events
            .iter()
            .any(|e| e.get("name").and_then(Value::as_str) == Some("crossapp.program_build_full")),
        "the no-id row builds under the single-app profile"
    );
    let err = String::from_utf8_lossy(&out.stderr);
    let warning: Vec<&str> = err
        .lines()
        .filter(|l| l.starts_with("al-sem: warning: "))
        .collect();
    // Captured from the binary at 77577a4b, before this classification
    // change (Windows).
    let prefix = format!(
        "al-sem: warning: coverage could not be verified: snapshot build failed: \
         workspace source provider: reading source {}: ",
        d.path().join("A.al").display()
    );
    #[cfg(windows)]
    assert_eq!(
        warning,
        [format!(
            "{prefix}The process cannot access the file because it is being used by \
             another process. (os error 32)"
        )],
        "stderr: {err}"
    );
    // Linux: captured in WSL at 8e8d2c9f (final panel P1-2).
    #[cfg(unix)]
    assert_eq!(
        warning,
        [format!("{prefix}Permission denied (os error 13)")],
        "stderr: {err}"
    );
}

/// Spec §4.4.5 row 3: the cross-app build succeeds, the model assembles to
/// nothing (an `.al` file with no object). One build, and the warning reads
/// the build's own `Ok` coverage (`files_without_object`).
#[test]
fn run_assembly_row_runs_one_build_and_reads_its_coverage() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(
        d.path().join("app.json"),
        r#"{"id":"11111111-1111-1111-1111-111111111111","name":"A","publisher":"p","version":"1.0.0.0"}"#,
    )
    .unwrap();
    std::fs::write(d.path().join("A.al"), "// no object\n").unwrap();
    let (out, events) = spawn_trace_events(
        env!("CARGO_BIN_EXE_alsem"),
        &["analyze", d.path().to_str().unwrap(), "--format", "json"],
        &[],
    );
    let m = last_counters(&events);
    assert_eq!(count(&m, "build.program"), 1, "{m:?}");
    // Precondition: the build succeeded and the assembly ran.
    assert!(
        events
            .iter()
            .any(|e| e.get("name").and_then(Value::as_str) == Some("crossapp.model_assembly")),
        "the cross-app build reached its assembly"
    );
    assert_eq!(out.status.code(), Some(0));
    let err = String::from_utf8_lossy(&out.stderr);
    let warning: Vec<&str> = err
        .lines()
        .filter(|l| l.starts_with("al-sem: warning: "))
        .collect();
    assert_eq!(
        warning,
        ["al-sem: warning: coverage could not be verified: No object declaration found in A.al"],
        "stderr: {err}"
    );
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
            witness: 0,
        },
    );
}

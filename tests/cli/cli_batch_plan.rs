//! #277 Task 7: `alsem batch` plans one view per key (D4.1), one plan per
//! view (D5), and visits the views one after another (D6). Every case is
//! spawned with tracing on; counters come from the trace (D10), and every
//! job is compared with its own standalone run.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::{Command, Output};

use serde_json::Value;

use crate::cli_build_counters::{
    Row, assert_row, count, last_counters, policy_yaml, spawn_trace_events, write_diff, ws_a,
    ws_dep,
};

/// One `alsem batch` run of one batch: its process output, its trace and the
/// directory holding the manifest and the jobs' stream files.
pub(crate) struct BatchRun {
    pub out: Output,
    pub events: Vec<Value>,
    pub counters: BTreeMap<String, u64>,
    pub dir: tempfile::TempDir,
}

impl BatchRun {
    /// The run's job lines (D2.2).
    pub fn lines(&self) -> Vec<Value> {
        String::from_utf8_lossy(&self.out.stdout)
            .lines()
            .map(|l| serde_json::from_str(l).expect("a JSON job line"))
            .collect()
    }

    /// Job `i`'s stdout and stderr files.
    pub fn streams(&self, i: usize) -> (Vec<u8>, Vec<u8>) {
        let read = |ext: &str| std::fs::read(self.dir.path().join(format!("0.{i}.{ext}"))).unwrap();
        (read("stdout"), read("stderr"))
    }
}

/// The jobs as one batch, run from `cwd` (the test's own when `None`).
pub(crate) fn batch_in(jobs: &[Vec<String>], cwd: Option<&Path>, env: &[(&str, &str)]) -> BatchRun {
    let dir = tempfile::tempdir().unwrap();
    let manifest = dir.path().join("m.json");
    let jobs: Vec<Value> = jobs
        .iter()
        .map(|a| serde_json::json!({ "argv": a }))
        .collect();
    std::fs::write(&manifest, serde_json::json!({ "jobs": jobs }).to_string()).unwrap();
    let manifest = manifest.to_string_lossy().into_owned();
    let mut env = env.to_vec();
    env.push(("RUST_LOG", "off"));
    let (out, events) = match cwd {
        None => spawn_trace_events(env!("CARGO_BIN_EXE_alsem"), &["batch", &manifest], &env),
        Some(cwd) => spawn_in(cwd, &["batch", &manifest], &env),
    };
    let counters = last_counters(&events);
    BatchRun {
        out,
        events,
        counters,
        dir,
    }
}

pub(crate) fn batch(jobs: &[Vec<String>]) -> BatchRun {
    batch_in(jobs, None, &[])
}

/// `alsem <args>` from `cwd`, traced (`spawn_trace_events` with a cwd).
fn spawn_in(cwd: &Path, args: &[&str], env: &[(&str, &str)]) -> (Output, Vec<Value>) {
    let dir = tempfile::tempdir().unwrap();
    let trace = dir.path().join("trace.json");
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_alsem"));
    cmd.args(args)
        .current_dir(cwd)
        .env_remove("ALSEM_TRACE_DETAIL")
        .env_remove("ALSEM_RETAINED_CENSUS")
        .env_remove("ALSEM_CONES_CENSUS")
        .env("ALSEM_TRACE", "1")
        .env("ALSEM_TRACE_FILE", &trace);
    for (k, v) in env {
        cmd.env(k, v);
    }
    let out = cmd.output().unwrap();
    let text = std::fs::read_to_string(&trace).unwrap_or_default();
    (out, serde_json::from_str(&text).unwrap_or_default())
}

/// `args` as a manifest argv.
pub(crate) fn argv(args: &[&str]) -> Vec<String> {
    args.iter().map(|s| s.to_string()).collect()
}

/// `alsem <argv>` standalone from `cwd`: stdout, stderr, exit.
pub(crate) fn standalone_in(
    argv: &[String],
    cwd: Option<&Path>,
) -> (Vec<u8>, Vec<u8>, Option<i32>) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_alsem"));
    cmd.args(argv)
        .env("RUST_LOG", "off")
        .env_remove("ALSEM_TRACE");
    if let Some(cwd) = cwd {
        cmd.current_dir(cwd);
    }
    let out = cmd.output().unwrap();
    (out.stdout, out.stderr, out.status.code())
}

/// Every job's stdout, stderr and exit equal its standalone run's; the run
/// itself exits 0 with one line per job.
pub(crate) fn assert_each_standalone(run: &BatchRun, jobs: &[Vec<String>], cwd: Option<&Path>) {
    assert_eq!(
        run.out.status.code(),
        Some(0),
        "run stderr: {}",
        String::from_utf8_lossy(&run.out.stderr)
    );
    let lines = run.lines();
    assert_eq!(lines.len(), jobs.len());
    for (i, job) in jobs.iter().enumerate() {
        let (stdout, stderr, exit) = standalone_in(job, cwd);
        let (b_out, b_err) = run.streams(i);
        let what = format!("job {i} {job:?}");
        assert_eq!(lines[i]["job"], i, "{what}");
        assert_eq!(
            lines[i]["exit"].as_i64(),
            exit.map(i64::from),
            "{what}: exit"
        );
        assert_eq!(
            String::from_utf8_lossy(&b_out),
            String::from_utf8_lossy(&stdout),
            "{what}: stdout"
        );
        assert_eq!(
            String::from_utf8_lossy(&b_err),
            String::from_utf8_lossy(&stderr),
            "{what}: stderr"
        );
    }
}

/// The `B`/`E` pairs of every span whose name `keep` accepts: (name, begin, end).
pub(crate) fn spans(events: &[Value], keep: impl Fn(&str) -> bool) -> Vec<(String, f64, f64)> {
    let mut open: Vec<(String, f64)> = Vec::new();
    let mut out = Vec::new();
    for e in events {
        let (Some(name), Some(ts)) = (e["name"].as_str(), e["ts"].as_f64()) else {
            continue;
        };
        if !keep(name) {
            continue;
        }
        match e["ph"].as_str() {
            Some("B") => open.push((name.to_string(), ts)),
            Some("E") => {
                let at = open
                    .iter()
                    .rposition(|(n, _)| n == name)
                    .expect("an open span");
                let (n, b) = open.remove(at);
                out.push((n, b, ts));
            }
            _ => {}
        }
    }
    assert!(open.is_empty(), "unclosed spans: {open:?}");
    out
}

/// A `view.<i>` span name.
pub(crate) fn is_view(name: &str) -> bool {
    name.strip_prefix("view.")
        .is_some_and(|i| i.parse::<usize>().is_ok())
}

/// The `ts` of every fill: a counted builder's `build.*` counter event.
pub(crate) fn fills(events: &[Value]) -> Vec<f64> {
    events
        .iter()
        .filter(|e| e["ph"] == "C" && e["name"].as_str().is_some_and(|n| n.starts_with("build.")))
        .filter_map(|e| e["ts"].as_f64())
        .collect()
}

/// The `ts` of each `name` instant with `args.<arg> == v`.
fn instants(events: &[Value], name: &str) -> Vec<(f64, Value)> {
    events
        .iter()
        .filter(|e| e["ph"] == "i" && e["name"] == name)
        .map(|e| (e["ts"].as_f64().unwrap(), e["args"].clone()))
        .collect()
}

/// One cross-app view of `ws_dep()` holding analyze, digest, policy, events
/// and query (D10's first row).
fn five_jobs(ws: &str, diff: &str, policy: &str) -> Vec<Vec<String>> {
    vec![
        argv(&["analyze", ws, "--format", "json", "--deterministic"]),
        argv(&["digest", ws, "--diff", diff, "--deterministic"]),
        argv(&[
            "policy",
            "check",
            ws,
            "--policy",
            policy,
            "--format",
            "json",
            "--deterministic",
        ]),
        argv(&[
            "events",
            "fanout",
            ws,
            "--format",
            "json",
            "--deterministic",
        ]),
        argv(&[
            "query",
            "touches",
            ws,
            "--table",
            "18",
            "--format",
            "json",
            "--deterministic",
        ]),
    ]
}

/// T3 (D10, I1): one cross-app view with analyze, digest, policy, events and
/// query builds and fills everything once: the union of the five plans is
/// one plan, one witness pass, one `det`. Every job equals its standalone run.
#[test]
fn batch_counters_one_per_view() {
    let (d, ws) = ws_dep();
    let diff = write_diff(d.path(), "src/Work.al", 18, "        Customer.Insert();");
    let jobs = five_jobs(&ws, &diff, &policy_yaml());
    let run = batch(&jobs);
    assert_each_standalone(&run, &jobs, None);
    let one_view = || Row {
        program: 1,
        bcg: 1,
        cone: 1,
        v2: 1,
        scc: 2,
        pd: 1,
        ctx: 1,
        snap: 1,
        witness: 1,
        inputs: 1,
    };
    assert_row("five jobs, one view", &run.counters, one_view());

    // A second digest with other roots, the same flags: it shares the first
    // digest's tree (snap 1, G16: one tree over both jobs' roots) off the one
    // witness pass (witness 1), and still equals its standalone run.
    let mut six = jobs.clone();
    six.push(argv(&[
        "digest",
        &ws,
        "--routine",
        "CallLib",
        "--deterministic",
    ]));
    let run = batch(&six);
    assert_each_standalone(&run, &six, None);
    assert_row(
        "five jobs + a second digest",
        &run.counters,
        Row {
            snap: 1,
            ..one_view()
        },
    );
}

/// G16: many `prove` jobs on one view share ONE tree (snap 1) instead of one
/// whole-program tree each, and every job still equals its standalone run. Two
/// jobs ask the same routine (different questions), so a root shared by two
/// readers is covered too. Before, each job derived its own tree and kept its
/// identity table and coverage until it ran: 408 jobs over Base Application's
/// source passed 40 GB before the first output.
#[test]
fn batch_prove_jobs_share_one_tree() {
    let (_d, ws) = ws_dep();
    let jobs: Vec<Vec<String>> = [
        ("CallLib", "may-commit"),
        ("CommitAfterDep", "may-commit"),
        ("CallLib", "reaches-ui"),
        ("CommitAfterDep", "throws-error"),
    ]
    .iter()
    .map(|(routine, question)| argv(&["prove", &ws, routine, question, "--deterministic"]))
    .collect();
    let run = batch(&jobs);
    assert_each_standalone(&run, &jobs, None);
    assert_eq!(
        count(&run.counters, "build.snapshot"),
        1,
        "four prove jobs, one tree: {:?}",
        run.counters
    );
}

/// T3, two views of one workspace: analyze cross-app and `--single-app` build
/// twice (program 2, ctx 2) off ONE read (inputs 1).
#[test]
fn batch_counters_two_views_one_read() {
    let (_d, ws) = ws_dep();
    let jobs = vec![
        argv(&["analyze", &ws, "--format", "json", "--deterministic"]),
        argv(&[
            "analyze",
            &ws,
            "--single-app",
            "--format",
            "json",
            "--deterministic",
        ]),
    ];
    let run = batch(&jobs);
    assert_each_standalone(&run, &jobs, None);
    assert_row(
        "analyze cross-app + single-app",
        &run.counters,
        Row {
            program: 2,
            bcg: 2,
            cone: 2,
            v2: 2,
            scc: 4,
            pd: 2,
            ctx: 2,
            snap: 0,
            witness: 0,
            inputs: 1,
        },
    );
}

/// T3, case B (spec §9, D10): prove is a tree job on its view (`DIGEST_ROW`,
/// step 9 §4.6), so fingerprint B0 is the one cut reader left (D6.4, S′).
/// (a) B0 and `prove --single-app` on ONE single-app view: the view's half
/// once (program, v2, ctx, inputs 1; bcg, cone, pd 1; scc 2), prove's tree
/// off the view's one composition (snap 1, witness 1) and B0's own cut (bcg,
/// cone, scc, pd, snap, witness +1). (b) B0 and prove, cross-app by default:
/// two views, the cross-app one `DIGEST_ROW` (1, 1, 1, 1, 2, 1, 1, 1, 1, 1)
/// and the single-app one `FINGERPRINT_ROW` (1, 2, 2, 1, 3, 2, 1, 1, 1, 1),
/// one workspace read.
#[test]
fn batch_counters_cut_readers() {
    let (_d, ws) = ws_dep();
    let b0 = argv(&["fingerprint", &ws, "--format", "json", "--deterministic"]);
    let prove = |extra: &[&str]| {
        let mut a = vec![
            "prove",
            ws.as_str(),
            "CallLib",
            "may-commit",
            "--deterministic",
        ];
        a.extend_from_slice(extra);
        argv(&a)
    };
    let one_view = Row {
        program: 1,
        bcg: 2,
        cone: 2,
        v2: 1,
        scc: 3,
        pd: 2,
        ctx: 1,
        snap: 2,
        witness: 2,
        inputs: 1,
    };
    let two_views = Row {
        program: 2,
        bcg: 3,
        cone: 3,
        v2: 2,
        scc: 5,
        pd: 3,
        ctx: 2,
        snap: 2,
        witness: 2,
        inputs: 1,
    };
    for (case, jobs, row) in [
        (
            "(a) fingerprint + prove --single-app",
            vec![b0.clone(), prove(&["--single-app"])],
            one_view,
        ),
        ("(b) fingerprint + prove", vec![b0, prove(&[])], two_views),
    ] {
        let run = batch(&jobs);
        assert_each_standalone(&run, &jobs, None);
        assert_row(case, &run.counters, row);
    }
}

/// T3, a run asking `ORDERING_FACTS` (d47) beside a digest: the digest row
/// plus the ordering cell's cut composition (bcg, cone, scc, pd, snap,
/// witness +1; `analyze_cross_app_d47_with_a_summaries_detector`).
#[test]
fn batch_counters_ordering_cut() {
    let (d, ws) = ws_dep();
    let diff = write_diff(d.path(), "src/Work.al", 18, "        Customer.Insert();");
    let jobs = vec![
        argv(&[
            "analyze",
            &ws,
            "--format",
            "json",
            "--detector",
            "d47-io-unsafe-txn",
            "--deterministic",
        ]),
        argv(&["digest", &ws, "--diff", &diff, "--deterministic"]),
    ];
    let run = batch(&jobs);
    assert_each_standalone(&run, &jobs, None);
    assert_eq!(count(&run.counters, "build.ordering_facts"), 1);
    assert_row(
        "analyze d47 + digest",
        &run.counters,
        Row {
            program: 1,
            bcg: 2,
            cone: 2,
            v2: 1,
            scc: 3,
            pd: 2,
            ctx: 1,
            snap: 2,
            witness: 2,
            inputs: 1,
        },
    );
}

/// T4 (D4.1): two views of one workspace are never alive together: the
/// first view's drop instant comes before the second view's span opens, the
/// two spans do not overlap, and every fill lies inside a view span.
#[test]
fn batch_views_are_sequential() {
    let (_d, ws) = ws_dep();
    let jobs = vec![
        argv(&["analyze", &ws, "--format", "json"]),
        argv(&["analyze", &ws, "--single-app", "--format", "json"]),
    ];
    let run = batch(&jobs);
    assert_eq!(run.out.status.code(), Some(0));
    let views = spans(&run.events, is_view);
    assert_eq!(views.len(), 2, "two view spans: {views:?}");
    let drops = instants(&run.events, "view.drop");
    let drop0 = drops
        .iter()
        .find(|(_, a)| a["view"] == 0)
        .expect("view 0's drop")
        .0;
    let (v0, v1) = (&views[0], &views[1]);
    assert!(
        drop0 <= v0.2 && v0.2 <= v1.1,
        "view 1 opens after view 0 drops: {views:?}"
    );
    let first_fill_1 = fills(&run.events)
        .into_iter()
        .filter(|t| v1.1 <= *t && *t <= v1.2)
        .fold(f64::INFINITY, f64::min);
    assert!(
        drop0 < first_fill_1,
        "view 1's first fill after view 0's drop"
    );
    for t in fills(&run.events) {
        assert!(
            views.iter().any(|(_, b, e)| *b <= t && t <= *e),
            "a fill at {t} outside every view"
        );
    }
}

/// T15 (D5.5): analyze default and digest on one view ask the same run
/// (default detectors, primary scope): one `finish_run` for the key.
#[test]
fn analyze_and_digest_share_one_run() {
    let (d, ws) = ws_dep();
    let diff = write_diff(d.path(), "src/Work.al", 18, "        Customer.Insert();");
    let jobs = vec![
        argv(&["analyze", &ws, "--format", "json", "--deterministic"]),
        argv(&["digest", &ws, "--diff", &diff, "--deterministic"]),
    ];
    let run = batch(&jobs);
    assert_each_standalone(&run, &jobs, None);
    let runs = instants(&run.events, "view.run");
    let mut keys: Vec<String> = runs.iter().map(|(_, a)| a.to_string()).collect();
    assert!(!keys.is_empty(), "precondition: a run");
    keys.sort();
    let n = keys.len();
    keys.dedup();
    assert_eq!(n, keys.len(), "one finish_run per RunKey: {runs:?}");
    assert_eq!(n, 1, "analyze and digest share one key: {runs:?}");
}

/// T19 (D5.1a): an empty request composes only with a tree. (a) `digest
/// --tests` that resolves no root (census `witness.members=0`) composes its
/// tree: cone 1, snapshot 1, witness pass 0, alone and beside analyze. (b)
/// query alone and query + graph (no run, no request, no tree) compose zero
/// times. (c) case B (#160 step 9 D4): `fingerprint --inventory-only` (an
/// empty request and an `Inventory` tree read) composes once with no raw
/// facts, the same counts as (a), alone and beside analyze on its
/// (cross-app) view.
#[test]
fn empty_request_composes_only_with_a_tree() {
    let (_d, ws) = ws_dep();
    let digest = argv(&[
        "digest",
        &ws,
        "--routine",
        "NoSuchRoutine",
        "--tests",
        "--deterministic",
    ]);
    let inventory = argv(&[
        "fingerprint",
        &ws,
        "--inventory-only",
        "--format",
        "json",
        "--deterministic",
    ]);
    let analyze = argv(&["analyze", &ws, "--format", "json", "--deterministic"]);
    for jobs in [
        vec![digest.clone()],
        vec![digest, analyze.clone()],
        vec![inventory.clone()],
        vec![inventory, analyze],
    ] {
        let run = batch_in(&jobs, None, &[("ALSEM_RETAINED_CENSUS", "1")]);
        assert_each_standalone(&run, &jobs, None);
        let err = String::from_utf8_lossy(&run.out.stderr);
        assert!(err.contains("witness.members=0"), "precondition: {err}");
        let m = &run.counters;
        let what = format!("{jobs:?}: {m:?}");
        assert_eq!(count(m, "build.cone"), 1, "{what}");
        assert_eq!(count(m, "build.snapshot"), 1, "{what}");
        assert_eq!(count(m, "build.witness_pass"), 0, "{what}");
    }
    let query = argv(&[
        "query",
        "touches",
        &ws,
        "--table",
        "18",
        "--format",
        "json",
        "--deterministic",
    ]);
    for jobs in [vec![query.clone()], vec![query, argv(&["graph", &ws])]] {
        let run = batch(&jobs);
        assert_each_standalone(&run, &jobs, None);
        let m = &run.counters;
        assert_eq!(count(m, "build.program"), 1, "precondition: {m:?}");
        assert_eq!(count(m, "build.cone"), 0, "{m:?}");
        assert_eq!(count(m, "build.witness_pass"), 0, "{m:?}");
    }
}

/// Review Focus 2 (D4.1): the view key is the ABSOLUTE spelling, not the
/// canonical one: `ws`, `./ws` and `ws/` are one view; `../<dir>/ws` (a `..`
/// component) is a second. One `view` span per distinct `std::path::absolute`
/// (compared by components), every output equal to standalone.
#[test]
fn batch_view_key_spellings() {
    let corpus = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/r0-corpus");
    let spellings = [
        "ws-d1-multi-caller",
        "./ws-d1-multi-caller",
        "ws-d1-multi-caller/",
        "../r0-corpus/ws-d1-multi-caller",
    ];
    let mut distinct: Vec<std::path::PathBuf> = Vec::new();
    for s in spellings {
        let p = std::path::absolute(corpus.join(s)).unwrap();
        if !distinct.contains(&p) {
            distinct.push(p);
        }
    }
    assert_eq!(distinct.len(), 2, "precondition: two absolute spellings");
    let jobs: Vec<Vec<String>> = spellings
        .iter()
        .map(|s| {
            argv(&[
                "analyze",
                s,
                "--single-app",
                "--format",
                "json",
                "--deterministic",
            ])
        })
        .collect();
    let run = batch_in(&jobs, Some(&corpus), &[]);
    assert_each_standalone(&run, &jobs, Some(&corpus));
    assert_eq!(spans(&run.events, is_view).len(), distinct.len());
}

/// Plan Task 7 Step 2b (R6-2): each workspace path is read once with the
/// union of its jobs' needs, before the first view; every job holds that one
/// read. Two jobs on one view (analyze + digest, which needs the snapshot
/// rows) and two jobs on two views of one workspace: `inputs.read` 1 each,
/// output equal to standalone, and the driver's one-read assert (R5-3) holds.
#[test]
fn batch_reads_each_workspace_once() {
    let (d, ws) = ws_dep();
    let diff = write_diff(d.path(), "src/Work.al", 18, "        Customer.Insert();");
    for jobs in [
        vec![
            argv(&["analyze", &ws, "--format", "json", "--deterministic"]),
            argv(&["digest", &ws, "--diff", &diff, "--deterministic"]),
        ],
        vec![
            argv(&["analyze", &ws, "--format", "json", "--deterministic"]),
            argv(&[
                "analyze",
                &ws,
                "--single-app",
                "--format",
                "json",
                "--deterministic",
            ]),
        ],
    ] {
        let run = batch(&jobs);
        assert_each_standalone(&run, &jobs, None);
        assert_eq!(count(&run.counters, "inputs.read"), 1, "{jobs:?}");
    }
}

/// D9 in a batch: a graph job beside a model job on one cross-app view takes
/// the program the builder lends before the base is built from it: one
/// program build, both outputs equal to standalone.
#[test]
fn graph_and_a_model_job_share_one_program() {
    let (_d, ws) = ws_dep();
    let jobs = vec![
        argv(&["graph", &ws]),
        argv(&[
            "events",
            "fanout",
            &ws,
            "--format",
            "json",
            "--deterministic",
        ]),
    ];
    let run = batch(&jobs);
    assert_each_standalone(&run, &jobs, None);
    assert_eq!(
        count(&run.counters, "build.program"),
        1,
        "{:?}",
        run.counters
    );
}

/// T21 (D8; #277 E12-C1): `[diff A B, diff B A]` visits A, then B; job 2's new
/// side computes on A and its old side on B, and it finalizes after B. `diff
/// A A` shares one view; a diff whose old side fails stops there. Every job
/// equals its standalone run.
#[test]
fn batch_diff_sides() {
    let (_d, b) = ws_dep();
    let a = ws_a();
    // No `id`: its view fails before any build.
    let bad_dir = tempfile::tempdir().unwrap();
    std::fs::write(
        bad_dir.path().join("app.json"),
        r#"{"name":"A","publisher":"p","version":"1.0.0.0"}"#,
    )
    .unwrap();
    let bad = bad_dir.path().to_string_lossy().into_owned();
    let diff = |o: &str, n: &str| argv(&["diff", o, n, "--format", "json", "--deterministic"]);
    let jobs = vec![
        diff(&a, &b),
        diff(&b, &a),
        argv(&["analyze", &a, "--format", "json", "--deterministic"]),
        diff(&a, &a),
        diff(&bad, &a),
    ];
    let run = batch(&jobs);
    assert_each_standalone(&run, &jobs, None);
    // Views: A single-app (both diffs, `diff A A`, the new side of `diff bad
    // A`), B single-app, A cross-app (analyze); the bad view builds nothing.
    assert_eq!(
        count(&run.counters, "build.program"),
        3,
        "{:?}",
        run.counters
    );
}

/// #277 final r1 FR-9 (D6.1): `policy check --no-policy` declares no raw
/// read, so a policy with rules before it (which fills `raw`, its witness
/// pass) releases `raw` after its own last read, in its own compute; a later
/// reader's declaration would have kept it. Each job equals standalone.
#[test]
fn no_policy_reads_no_raw() {
    let (_d, ws) = ws_dep();
    let policy = policy_yaml();
    let no_policy = argv(&[
        "policy",
        "check",
        &ws,
        "--no-policy",
        "--format",
        "json",
        "--deterministic",
    ]);
    let with_rules = argv(&[
        "policy",
        "check",
        &ws,
        "--policy",
        &policy,
        "--format",
        "json",
        "--deterministic",
    ]);
    let jobs = vec![with_rules, no_policy];
    let run = batch(&jobs);
    assert_each_standalone(&run, &jobs, None);
    let compute = |j: usize| {
        instants(&run.events, "job.step")
            .into_iter()
            .find(|(_, a)| a["job"] == j && a["step"] == "compute")
            .map(|(ts, _)| ts)
            .expect("a compute step")
    };
    let raw: Vec<f64> = instants(&run.events, "view.release")
        .into_iter()
        .filter(|(_, a)| a["cell"] == "Raw")
        .map(|(ts, _)| ts)
        .collect();
    assert_eq!(raw.len(), 1, "raw released once: {raw:?}");
    let (job0, job1) = (compute(0), compute(1));
    assert!(
        job0 < raw[0] && raw[0] < job1,
        "in the policy's compute: raw {raw:?}, computes {job0} {job1}"
    );
}

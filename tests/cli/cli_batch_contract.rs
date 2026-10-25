//! #277 T1, T2 (spec E13, I4) and the job-line counts of R8-5 (D7, 12A-17).
//!
//! T1 is the contract test over every command: ONE batch holding every
//! batchable command and form, on `ws_dep()` and an r0 dir, both universes,
//! against each job run as its own SEPARATE `alsem` process. Per job: stdout
//! bytes, stderr bytes, exit and every file it writes are equal. The run has
//! the census and the trace off and `RUST_LOG=off`, so a job's stderr holds
//! only job lines (D7); the L4 seam is on, so every job whose standalone run
//! solves `l4` carries a job line to compare. A standalone run goes through
//! the same driver as the batch (D1), so the properties that driver alone
//! decides (one witness pass per view, how many times a job line is
//! replayed) are also stated as numbers, never only as equalities.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use serde_json::{Value, json};

use crate::cli_batch_plan::{BatchRun, argv};
use crate::cli_batch_writes::{job, run_manifest};
use crate::cli_build_counters::{policy_yaml, write_diff, ws_a, ws_dep};

/// The second policy of W's view (R9-1): its own rule, so its output differs
/// from the first policy's.
const POLICY2: &str = "policy2.yaml";
const POLICY2_YAML: &str = "version: 1
description: \"#277 R9-1: a second policy, one rule the first does not have\"
rules:
  - id: second-policy-writes
    title: \"Table writes are reviewed (second policy)\"
    severity: high
    when:
      capability.op: [insert, modify, delete]
    facts: any
";

/// Every solve prints one non-convergence line (debug-only seam, spec T1 B1).
/// Release builds have no seam: T1 then runs without its B1 precondition, and
/// the job-line count tests are debug-only (#277 R10-2).
const SEAM: (&str, &str) = ("ALSEM_TEST_SUMMARIZE_DIAGNOSTIC", "1");
#[cfg(debug_assertions)]
const SEAM_LINE: &str = "warning: summarize: test seam: a forced non-convergence";

/// `exe <args>` from `cwd` with `stdin` piped in: census and trace off,
/// `RUST_LOG=off`, `env` set.
fn spawn(exe: &str, args: &[String], cwd: &Path, env: &[(&str, &str)], stdin: &[u8]) -> Output {
    let mut cmd = Command::new(exe);
    cmd.args(args)
        .current_dir(cwd)
        .env("RUST_LOG", "off")
        .env_remove("ALSEM_TRACE")
        .env_remove("ALSEM_TRACE_FILE")
        .env_remove("ALSEM_RETAINED_CENSUS")
        .env_remove("ALSEM_TEST_SUMMARIZE_DIAGNOSTIC")
        .env_remove("ALSEM_TEST_GRAPH_COVERAGE_WARNING")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (k, v) in env {
        cmd.env(k, v);
    }
    let mut child = cmd.spawn().unwrap();
    child.stdin.take().unwrap().write_all(stdin).unwrap();
    child.wait_with_output().unwrap()
}

fn alsem(args: &[String], cwd: &Path, env: &[(&str, &str)], stdin: &[u8]) -> Output {
    spawn(env!("CARGO_BIN_EXE_alsem"), args, cwd, env, stdin)
}

/// Every file under `dir`, by its path relative to `dir`.
fn files(dir: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut out = BTreeMap::new();
    let mut todo = vec![dir.to_path_buf()];
    while let Some(d) = todo.pop() {
        for e in std::fs::read_dir(&d).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                todo.push(p);
            } else {
                let rel = p.strip_prefix(dir).unwrap().to_path_buf();
                out.insert(rel, std::fs::read(&p).unwrap());
            }
        }
    }
    out
}

/// One `alsem batch` of `jobs` from `cwd` with `stdin` piped in (the manifest
/// and the default streams in their own directory, so `cwd` holds only the
/// jobs' files); trace off, so the run carries no events.
fn run_batch(jobs: &[Vec<String>], cwd: &Path, env: &[(&str, &str)], stdin: &[u8]) -> BatchRun {
    let dir = tempfile::tempdir().unwrap();
    let manifest = dir.path().join("m.json");
    let jobs: Vec<Value> = jobs.iter().map(|a| json!({ "argv": a })).collect();
    std::fs::write(&manifest, json!({ "jobs": jobs }).to_string()).unwrap();
    let args = argv(&["batch", &manifest.to_string_lossy()]);
    let out = alsem(&args, cwd, env, stdin);
    assert_eq!(
        out.status.code(),
        Some(0),
        "run stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    BatchRun {
        out,
        events: vec![],
        counters: BTreeMap::new(),
        dir,
    }
}

/// Job `i`'s line, stdout and stderr.
fn job_of(b: &BatchRun, i: usize) -> (Value, Vec<u8>, Vec<u8>) {
    let line = b.lines().swap_remove(i);
    let (stdout, stderr) = b.streams(i);
    (line, stdout, stderr)
}

/// Job `i` of the batch equals `alone` (its separate run): exit, stdout,
/// stderr.
fn assert_job_equals(b: &BatchRun, i: usize, what: &str, alone: &Output) {
    let (line, stdout, stderr) = job_of(b, i);
    assert_eq!(line["job"], i, "{what}");
    assert_eq!(
        line["exit"].as_i64(),
        alone.status.code().map(i64::from),
        "{what}: exit"
    );
    assert!(
        stdout == alone.stdout,
        "{what}: stdout\nbatch: {}\nalone: {}",
        String::from_utf8_lossy(&stdout),
        String::from_utf8_lossy(&alone.stdout)
    );
    assert_eq!(
        String::from_utf8_lossy(&stderr),
        String::from_utf8_lossy(&alone.stderr),
        "{what}: stderr"
    );
}

/// The fixtures T1 and T2 share: `ws_dep()` (W, with a dependency), an r0
/// dir (R), `ws_split` (S: on W every job names the dependency's table
/// anyway, so a raw set shared across trees shows only on S, as T22 found)
/// and #265 A6d's workspace (D: a dependency finding `--scope all` reports and
/// `--scope primary` drops; W has none, so a run shared across scopes shows
/// only on D), their inputs, and files an EARLIER, separate run wrote.
struct Fixture {
    _w: tempfile::TempDir,
    _s: tempfile::TempDir,
    _d: tempfile::TempDir,
    inputs: tempfile::TempDir,
    w: String,
    r: String,
    s: String,
    d: String,
    diff: String,
    diff_text: Vec<u8>,
}

impl Fixture {
    fn new() -> Fixture {
        let (w_dir, w) = ws_dep();
        let inputs = tempfile::tempdir().unwrap();
        let diff = write_diff(
            inputs.path(),
            "src/Work.al",
            18,
            "        Customer.Insert();",
        );
        let diff_text = std::fs::read(&diff).unwrap();
        let s_dir = tempfile::tempdir().unwrap();
        crate::cli_batch_case_b::ws_split(s_dir.path());
        let (d_dir, d) = crate::cli_d35_unknown_commit::dependency_subscriber_ws();
        let fx = Fixture {
            d: d.to_string_lossy().into_owned(),
            _d: d_dir,
            s: s_dir.path().to_string_lossy().into_owned(),
            _s: s_dir,
            _w: w_dir,
            inputs,
            w,
            r: ws_a(),
            diff,
            diff_text,
        };
        // A baseline and a snapshot, each written by its own earlier run.
        for (args, file) in [
            (
                vec!["analyze", fx.w.as_str(), "--format", "json", "--baseline"],
                "b0.json",
            ),
            (
                vec!["fingerprint", fx.w.as_str(), "--format", "json", "--out"],
                "snap.json",
            ),
        ] {
            let mut a = argv(&args);
            a.push(fx.input(file));
            if file == "b0.json" {
                a.push("--update-baseline".into());
            }
            a.push("--deterministic".into());
            let out = alsem(&a, fx.inputs.path(), &[], b"");
            assert!(
                std::path::Path::new(&fx.input(file)).is_file(),
                "precondition: {file} written: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
        std::fs::write(fx.input("empty.diff"), "").unwrap();
        std::fs::write(fx.input(POLICY2), POLICY2_YAML).unwrap();
        fx
    }

    fn input(&self, name: &str) -> String {
        self.inputs.path().join(name).to_string_lossy().into_owned()
    }

    /// Every batchable command and form (spec T1), `--deterministic` where
    /// the command has it. Relative `--out`s land under the cwd.
    fn jobs(&self) -> Vec<Vec<String>> {
        let (w, r, s, d) = (
            self.w.as_str(),
            self.r.as_str(),
            self.s.as_str(),
            self.d.as_str(),
        );
        let (diff, policy) = (self.diff.as_str(), policy_yaml());
        let (b0, snap, empty, policy2) = (
            self.input("b0.json"),
            self.input("snap.json"),
            self.input("empty.diff"),
            self.input(POLICY2),
        );
        let det = |a: &[&str]| argv(&[a, &["--deterministic"]].concat());
        let mut jobs = vec![
            // W, cross-app.
            det(&["analyze", w, "--format", "sarif"]),
            det(&["analyze", w, "--format", "json"]),
            det(&["analyze", w, "--format", "html"]),
            det(&["analyze", w, "--scope", "all", "--format", "json"]),
            // No `--format`: auto is JSON off a terminal, in a batch always.
            det(&["analyze", w]),
            det(&["analyze", w, "--baseline", &b0, "--format", "json"]),
            det(&[
                "analyze",
                w,
                "--baseline",
                "base.json",
                "--update-baseline",
                "--format",
                "json",
            ]),
            det(&["digest", w, "--diff", diff]),
            det(&["digest", w, "--diff", diff, "--format", "human"]),
            det(&["digest", w, "--diff", "-"]),
            // A second digest with other roots: its own tree, same pass.
            det(&["digest", w, "--routine", "CallLib", "--tests"]),
            // An empty diff (exit 1), next to analyze.
            det(&["digest", w, "--diff", &empty]),
            det(&["prove", w, "CallLib", "may-commit"]),
            det(&["fingerprint", w, "--routine", "CallLib", "--format", "json"]),
            det(&["fingerprint", w, "--inventory-only"]),
            det(&["events", "fanout", w, "--format", "json"]),
            det(&["events", "chains", w, "--format", "json"]),
            // Two policies on one view (R9-1: two distinct policy files).
            det(&[
                "policy", "check", w, "--policy", &policy, "--format", "json",
            ]),
            det(&[
                "policy",
                "check",
                w,
                "--policy",
                &policy2,
                "--format",
                "sarif",
                "--out",
                "policy.sarif",
            ]),
            det(&["query", "touches", w, "--table", "18", "--format", "json"]),
            det(&[
                "query",
                "effects",
                w,
                "--routine",
                "CallLib",
                "--format",
                "json",
            ]),
            argv(&["graph", w, "--out", "graph.jsonl"]),
            // W, single-app: fingerprint B0 and diff sides, beside the
            // `--single-app` forms.
            det(&["fingerprint", w, "--format", "json"]),
            det(&[
                "fingerprint",
                w,
                "--format",
                "cbor.gz",
                "--out",
                "fp.cbor.gz",
            ]),
            det(&["fingerprint", w, "--format", "human"]),
            det(&["fingerprint", w, "--shard", "all", "--out", "shards"]),
            det(&["diff", w, w, "--format", "json"]),
            det(&["diff", &snap, w, "--format", "json"]),
            det(&["diff", w, w, "--strict", "--format", "json"]),
            det(&["analyze", w, "--single-app", "--format", "json"]),
            det(&["digest", w, "--single-app", "--diff", diff]),
            det(&["prove", w, "CallLib", "may-commit", "--single-app"]),
            det(&["events", "fanout", w, "--single-app", "--format", "json"]),
            det(&[
                "policy",
                "check",
                w,
                "--single-app",
                "--policy",
                &policy,
                "--format",
                "json",
            ]),
            det(&[
                "query",
                "touches",
                w,
                "--single-app",
                "--table",
                "18",
                "--format",
                "json",
            ]),
        ];
        jobs.extend([
            // R, cross-app.
            det(&["analyze", r, "--format", "json"]),
            det(&["digest", r, "--routine", "CallerA"]),
            det(&["prove", r, "CallerA", "may-commit"]),
            det(&["events", "chains", r, "--format", "json"]),
            det(&["policy", "check", r, "--format", "json"]),
            det(&[
                "query",
                "effects",
                r,
                "--routine",
                "CallerA",
                "--format",
                "json",
            ]),
            argv(&["graph", r]),
            // R, single-app.
            det(&["analyze", r, "--single-app", "--format", "sarif"]),
            det(&["digest", r, "--single-app", "--routine", "CallerA"]),
            det(&["fingerprint", r, "--format", "json"]),
            det(&["diff", r, r, "--format", "json"]),
            // S, cross-app: trees over different subjects on one pass.
            det(&["fingerprint", s, "--inventory-only", "--format", "json"]),
            det(&["fingerprint", s, "--routine", "CallLib", "--format", "json"]),
            det(&["fingerprint", s, "--routine", "DoWork", "--format", "json"]),
            det(&["digest", s, "--routine", "DoWork"]),
            det(&["digest", s, "--routine", "CallLib"]),
            // D, cross-app: the two scopes, two runs on one view.
            det(&["analyze", d, "--format", "json"]),
            det(&["analyze", d, "--scope", "all", "--format", "json"]),
        ]);
        jobs
    }
}

/// The jobs of each `view.<v>` span of a traced run, and each view's
/// `build.witness_pass` / `build.snapshot` increments, walked in file order
/// (one writer: file order is emission order).
fn views(events: &[Value]) -> Vec<(Vec<usize>, u64, u64)> {
    let mut out = Vec::new();
    let mut counters: BTreeMap<String, u64> = BTreeMap::new();
    let mut open: Option<(Vec<usize>, BTreeMap<String, u64>)> = None;
    let count = |m: &BTreeMap<String, u64>, k: &str| m.get(k).copied().unwrap_or(0);
    for e in events {
        let Some(name) = e["name"].as_str() else {
            continue;
        };
        match e["ph"].as_str() {
            Some("C") => {
                if let Some(v) = e["args"][name].as_u64() {
                    counters.insert(name.to_string(), v);
                }
            }
            Some("B") if crate::cli_batch_plan::is_view(name) => {
                open = Some((vec![], counters.clone()));
            }
            Some("E") if crate::cli_batch_plan::is_view(name) => {
                let (jobs, at) = open.take().expect("an open view");
                let delta = |k: &str| count(&counters, k) - count(&at, k);
                out.push((jobs, delta("build.witness_pass"), delta("build.snapshot")));
            }
            Some("i") if name == "job.step" => {
                if let Some((jobs, _)) = open.as_mut() {
                    let j = usize::try_from(e["args"]["job"].as_u64().unwrap()).unwrap();
                    if !jobs.contains(&j) {
                        jobs.push(j);
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// T1 (E13, A12.5): ONE batch with every job vs each job standalone.
#[test]
fn batch_equals_standalone() {
    let fx = Fixture::new();
    let jobs = fx.jobs();
    let stdin = fx.diff_text.as_slice();

    // Preconditions, from a separate traced run of the same manifest: every
    // view holds at least two jobs; both universes of both workspaces are
    // views; the cross-app view of W holds two tree jobs (two snapshots) off
    // ONE witness pass (Task 4's correction: the `build.witness_pass` count).
    let traced_dir = tempfile::tempdir().unwrap();
    let traced_cwd = tempfile::tempdir().unwrap();
    let manifest: Vec<Value> = jobs
        .iter()
        .map(|a| {
            job(
                &a.iter().map(String::as_str).collect::<Vec<_>>(),
                None,
                None,
            )
        })
        .collect();
    let (traced, events) = run_manifest(
        traced_dir.path(),
        &manifest,
        Some(traced_cwd.path()),
        &[],
        Some(stdin),
    );
    assert_eq!(
        traced.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&traced.stderr)
    );
    let vs = views(&events);
    for (jobs_on, _, _) in &vs {
        assert!(jobs_on.len() >= 2, "a view with one job: {vs:?}");
    }
    // W and R each have a cross-app and a single-app view (R9-6): a job's
    // view key read off its argv (its workspace; `--single-app`), skipping
    // fingerprint and diff, whose universe the argv does not spell.
    let key = |j: &[String]| {
        let ws = [fx.w.as_str(), fx.r.as_str()]
            .into_iter()
            .find(|w| j.iter().any(|a| a == w))?;
        let single = j.iter().any(|a| a == "--single-app");
        (single || !matches!(j[0].as_str(), "fingerprint" | "diff")).then_some((ws, single))
    };
    let keys: std::collections::BTreeSet<(&str, bool)> = vs
        .iter()
        .flat_map(|(on, _, _)| on.iter().filter_map(|&i| key(&jobs[i])))
        .collect();
    for ws in [fx.w.as_str(), fx.r.as_str()] {
        for single in [false, true] {
            assert!(
                keys.contains(&(ws, single)),
                "{ws} single-app {single}: a view of its own: {keys:?}"
            );
        }
    }
    let index_of = |args: &[&str]| {
        jobs.iter()
            .position(|j| j.iter().map(String::as_str).eq(args.iter().copied()))
            .unwrap()
    };
    let digest_roots = index_of(&[
        "digest",
        &fx.w,
        "--routine",
        "CallLib",
        "--tests",
        "--deterministic",
    ]);
    let (on_view, witness, snapshots) = vs
        .iter()
        .find(|(j, _, _)| j.contains(&digest_roots))
        .expect("the cross-app view of W");
    assert_eq!(*witness, 1, "one witness pass: {on_view:?}");
    assert!(*snapshots >= 2, "two tree jobs on one view: {snapshots}");

    let cwd = tempfile::tempdir().unwrap();
    let b = run_batch(&jobs, cwd.path(), &[SEAM], stdin);
    let mut written: BTreeMap<PathBuf, Vec<u8>> = BTreeMap::new();
    for (i, args) in jobs.iter().enumerate() {
        let own = tempfile::tempdir().unwrap();
        let alone = alsem(args, own.path(), &[SEAM], stdin);
        let what = format!("job {i} {args:?}");
        assert_job_equals(&b, i, &what, &alone);
        written.extend(files(own.path()));
    }
    // Every file each job wrote, at the same relative path, same bytes; and
    // the batch wrote no other.
    let got = files(cwd.path());
    assert_eq!(
        got.keys().collect::<Vec<_>>(),
        written.keys().collect::<Vec<_>>(),
        "the written files"
    );
    for (path, bytes) in &written {
        assert!(got[path] == *bytes, "{}: bytes differ", path.display());
    }

    // Preconditions the equalities above rely on, asserted.
    let empty = index_of(&[
        "digest",
        &fx.w,
        "--diff",
        &fx.input("empty.diff"),
        "--deterministic",
    ]);
    assert_eq!(job_of(&b, empty).0["exit"], 1, "the empty diff exits 1");
    assert!(
        written.len() >= 5,
        "the jobs write files: {:?}",
        written.keys()
    );
    #[cfg(debug_assertions)]
    {
        let digest = index_of(&["digest", &fx.w, "--diff", &fx.diff, "--deterministic"]);
        let (_, _, stderr) = job_of(&b, digest);
        let seam = String::from_utf8_lossy(&stderr).matches(SEAM_LINE).count();
        assert_eq!(seam, 1, "B1's precondition: digest's l4 line, once");
    }
    let primary = index_of(&["analyze", &fx.d, "--format", "json", "--deterministic"]);
    let scope = |i: usize| alsem(&jobs[i], cwd.path(), &[], b"").stdout;
    let (p, all) = (scope(primary), scope(primary + 1));
    assert!(
        p != all,
        "B3's precondition: D's two scopes report differently, standalone\n{}",
        String::from_utf8_lossy(&all)
    );
    // R9-1's precondition: the second policy job's standalone output differs
    // from the same job on the first policy, so a policy leaked across jobs
    // shows in the equalities above.
    let second = jobs
        .iter()
        .position(|j| j.iter().any(|a| a.ends_with(POLICY2)))
        .expect("the second policy job");
    let first_policy = policy_yaml();
    let on_first: Vec<String> = jobs[second]
        .iter()
        .map(|a| match a.ends_with(POLICY2) {
            true => first_policy.clone(),
            false => a.clone(),
        })
        .collect();
    let own = tempfile::tempdir().unwrap();
    alsem(&on_first, own.path(), &[], b"");
    let sarif = Path::new("policy.sarif");
    assert!(
        files(own.path())[sarif] != written[sarif],
        "R9-1's precondition: the two policies report differently, standalone"
    );
}

/// T2 (I4): a job's output does not depend on its neighbours. J (a digest
/// with its own roots) alone, as a one-job batch, vs J among every other job
/// of W's cross-app view. Precondition: J's subjects are a strict subset of
/// its view's union (the census's `witness.members`).
#[test]
fn batch_output_independent_of_neighbours() {
    let fx = Fixture::new();
    let all = fx.jobs();
    let w = fx.w.as_str();
    // W's cross-app view's jobs (none that writes a file of its own).
    let jobs: Vec<Vec<String>> = all
        .into_iter()
        .filter(|j| {
            j.iter().any(|a| a == w)
                && !j
                    .iter()
                    .any(|a| a == "--single-app" || a == "--out" || a == "--update-baseline")
                && !matches!(j[0].as_str(), "fingerprint" | "diff")
        })
        .collect();
    // (graph writes its `--out`, so it is no neighbour here.)
    for cmd in ["analyze", "digest", "prove", "events", "policy", "query"] {
        assert!(
            jobs.iter().any(|j| j[0] == cmd),
            "R9-2: a {cmd} neighbour on W's view"
        );
    }
    let j = jobs
        .iter()
        .position(|a| a.iter().any(|x| x == "CallLib") && a[0] == "digest")
        .expect("J");
    let census = [("ALSEM_RETAINED_CENSUS", "1")];
    let members = |b: &BatchRun| -> Vec<u64> {
        String::from_utf8_lossy(&b.out.stderr)
            .lines()
            .filter_map(|l| l.strip_prefix("[retained-census] witness.members="))
            .map(|r| r.split(' ').next().unwrap().parse().unwrap())
            .collect()
    };
    let cwd = tempfile::tempdir().unwrap();
    let stdin = fx.diff_text.as_slice();
    let alone = run_batch(std::slice::from_ref(&jobs[j]), cwd.path(), &census, stdin);
    let together = run_batch(&jobs, cwd.path(), &census, stdin);
    let (m_alone, m_union) = (members(&alone), members(&together));
    assert!(
        m_alone.len() == 1 && m_union.len() == 1 && m_alone[0] < m_union[0],
        "J's subjects a strict subset of the union: {m_alone:?} vs {m_union:?}"
    );
    let (line, stdout, stderr) = job_of(&alone, 0);
    let (t_line, t_stdout, t_stderr) = job_of(&together, j);
    assert_eq!(line["exit"], t_line["exit"]);
    assert_eq!(line["files"], t_line["files"]);
    assert_eq!(
        String::from_utf8_lossy(&stdout),
        String::from_utf8_lossy(&t_stdout)
    );
    assert_eq!(
        String::from_utf8_lossy(&stderr),
        String::from_utf8_lossy(&t_stderr)
    );
}

/// The number of times `line` appears in each job's stderr.
#[cfg(debug_assertions)]
fn line_counts(b: &BatchRun, jobs: usize, line: &str) -> Vec<usize> {
    (0..jobs)
        .map(|i| {
            String::from_utf8_lossy(&b.streams(i).1)
                .matches(line)
                .count()
        })
        .collect()
}

/// R8-5 (D7): job lines are replayed by count, stated, not by equality with
/// standalone (which runs the same driver). (a) On one view, each job whose
/// plan solves `l4` gets the solve's line once; graph, whose plan builds no
/// `l4`, gets none. (b) graph's coverage warning, once per graph job. In
/// both, the run's own stderr holds none of them.
#[cfg(debug_assertions)]
#[test]
fn batch_job_lines_by_count() {
    let a = ws_a();
    let cwd = tempfile::tempdir().unwrap();
    let jobs = vec![
        argv(&["events", "fanout", &a, "--format", "json"]),
        argv(&["events", "chains", &a, "--format", "json"]),
        argv(&["graph", &a]),
    ];
    let b = run_batch(&jobs, cwd.path(), &[SEAM], b"");
    assert_eq!(line_counts(&b, 3, SEAM_LINE), [1, 1, 0], "(a) one solve");
    assert!(!String::from_utf8_lossy(&b.out.stderr).contains(SEAM_LINE));

    let warning = "al-sem graph: warning: the coverage contract does not hold";
    let graphs = vec![argv(&["graph", &a]), argv(&["graph", &a]), jobs[0].clone()];
    let b = run_batch(
        &graphs,
        cwd.path(),
        &[("ALSEM_TEST_GRAPH_COVERAGE_WARNING", "1")],
        b"",
    );
    assert_eq!(line_counts(&b, 3, warning), [1, 1, 0], "(b) per graph job");
    assert!(!String::from_utf8_lossy(&b.out.stderr).contains(warning));
}

/// R8-5 (12A-17, F11): a library job line printed outside any driver step
/// (here `aldump`, which solves `l4` with no driver) reaches the process
/// stderr, once.
#[cfg(debug_assertions)]
#[test]
fn job_line_outside_a_step_reaches_stderr() {
    let a = ws_a();
    let cwd = tempfile::tempdir().unwrap();
    let out = spawn(
        env!("CARGO_BIN_EXE_aldump"),
        &argv(&["--r4-findings", &a]),
        cwd.path(),
        &[SEAM],
        b"",
    );
    assert_eq!(out.status.code(), Some(0));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(stderr.matches(SEAM_LINE).count(), 1, "{stderr}");
}

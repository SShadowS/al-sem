//! #277 Task 8 (spec D2.2, D2.4; T5, T24, Review Focus 1): a batch's writes.
//! Every write target is checked before any build (exit 3, nothing built, one
//! line naming both jobs); a job's own write failure is its line's exit and a
//! write-error line on the run's stderr, never the run's exit; a closed run
//! stdout is the run's failure (exit 1); jobs with no view run before the
//! first view and are written in job order; manifest stream paths resolve
//! against the manifest's directory, `--out` against the cwd; one stdin read
//! serves every `--diff -` job.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

use serde_json::{Value, json};

use crate::cli_batch_plan::{argv, assert_each_standalone, batch, standalone_in};
use crate::cli_build_counters::{count, last_counters, write_diff, ws_a, ws_dep};

/// A manifest run: `{"jobs": jobs}` written to `dir/m.json`, `alsem batch`
/// spawned from `cwd` (the test's own when `None`) with the trace on, `env`
/// set (after `RUST_LOG=off`) and `stdin` piped in when given.
pub(crate) fn run_manifest(
    dir: &Path,
    jobs: &[Value],
    cwd: Option<&Path>,
    env: &[(&str, &str)],
    stdin: Option<&[u8]>,
) -> (Output, Vec<Value>) {
    let manifest = dir.join("m.json");
    std::fs::write(&manifest, json!({ "jobs": jobs }).to_string()).unwrap();
    let trace = dir.join("trace.json");
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_alsem"));
    cmd.arg("batch")
        .arg(&manifest)
        .env_remove("ALSEM_RETAINED_CENSUS")
        .env("RUST_LOG", "off")
        .env("ALSEM_TRACE", "1")
        .env("ALSEM_TRACE_FILE", &trace)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(cwd) = cwd {
        cmd.current_dir(cwd);
    }
    for (k, v) in env {
        cmd.env(k, v);
    }
    let mut child = cmd.spawn().unwrap();
    let mut pipe = child.stdin.take().unwrap();
    pipe.write_all(stdin.unwrap_or_default()).unwrap();
    drop(pipe);
    let out = child.wait_with_output().unwrap();
    let text = std::fs::read_to_string(&trace).unwrap_or_default();
    (out, serde_json::from_str(&text).unwrap_or_default())
}

/// A manifest job: `argv`, and its stream paths when given.
pub(crate) fn job(args: &[&str], stdout: Option<&Path>, stderr: Option<&Path>) -> Value {
    let mut j = json!({ "argv": args });
    if let Some(p) = stdout {
        j["stdout"] = json!(p);
    }
    if let Some(p) = stderr {
        j["stderr"] = json!(p);
    }
    j
}

/// The run's job lines.
pub(crate) fn lines(out: &Output) -> Vec<Value> {
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| serde_json::from_str(l).expect("a JSON job line"))
        .collect()
}

/// The write-error lines on the run's stderr.
fn write_errors(out: &Output) -> Vec<String> {
    String::from_utf8_lossy(&out.stderr)
        .lines()
        .filter(|l| l.starts_with("al-sem: write error: "))
        .map(str::to_string)
        .collect()
}

/// T5 (b)-(b5), 12A-10: each conflicting manifest is refused before any build:
/// exit 3, `build.program` 0, nothing written, one stderr line naming both jobs.
#[test]
fn batch_write_conflicts_are_usage_errors() {
    let ws = ws_a();
    let (dep, dep_ws) = ws_dep();
    let d = tempfile::tempdir().unwrap();
    let p = |name: &str| d.path().join(name).to_string_lossy().into_owned();
    let (x, shard, base, pol) = (p("x.json"), p("shards"), p("b.json"), p("p.yaml"));
    let inner = p("shards/e.json");
    let app_json = dep.path().join("app.json").to_string_lossy().into_owned();
    let fp = |out: &str| {
        job(
            &["fingerprint", &ws, "--format", "json", "--out", out],
            None,
            None,
        )
    };
    let events_out = |out: &str| job(&["events", "fanout", &ws, "--out", out], None, None);
    let same = d.path().join("same.txt");
    let cases: Vec<(&str, Vec<Value>, &str)> = vec![
        (
            "(b) two jobs, one --out",
            vec![fp(&x), events_out(&x)],
            "both write",
        ),
        (
            "(b2) one job's stdout and stderr",
            vec![job(&["events", "fanout", &ws], Some(&same), Some(&same))],
            "both write",
        ),
        (
            "(b3) a --out inside another job's shard directory",
            vec![
                job(
                    &["fingerprint", &ws, "--shard", "all", "--out", &shard],
                    None,
                    None,
                ),
                events_out(&inner),
            ],
            "is inside the directory",
        ),
        (
            "(b3) two nested shard directories",
            vec![
                job(
                    &["fingerprint", &ws, "--shard", "all", "--out", &shard],
                    None,
                    None,
                ),
                job(
                    &["fingerprint", &ws, "--shard", "all", "--out", &inner],
                    None,
                    None,
                ),
            ],
            "nest",
        ),
        (
            "(b4) two --update-baseline on one baseline",
            vec![
                job(
                    &["analyze", &ws, "--baseline", &base, "--update-baseline"],
                    None,
                    None,
                ),
                job(
                    &[
                        "analyze",
                        &ws,
                        "--single-app",
                        "--baseline",
                        &base,
                        "--update-baseline",
                    ],
                    None,
                    None,
                ),
            ],
            "both write",
        ),
        (
            "(b5) fingerprint --out X, then diff X ws",
            vec![fp(&x), job(&["diff", &x, &ws], None, None)],
            "which batch 0 job 0 writes",
        ),
        (
            "(b5) --update-baseline B, then --baseline B",
            vec![
                job(
                    &["analyze", &ws, "--baseline", &base, "--update-baseline"],
                    None,
                    None,
                ),
                job(
                    &["analyze", &ws, "--single-app", "--baseline", &base],
                    None,
                    None,
                ),
            ],
            "which batch 0 job 0 writes",
        ),
        (
            "(b5) a --policy file another job writes",
            vec![
                job(&["policy", "check", &ws, "--policy", &pol], None, None),
                events_out(&pol),
            ],
            "which batch 0 job 1 writes",
        ),
        (
            "(b5) a workspace file another job writes",
            vec![
                job(&["events", "fanout", &dep_ws], None, None),
                events_out(&app_json),
            ],
            "reads the workspace",
        ),
    ];
    for (case, jobs, why) in cases {
        let run = tempfile::tempdir().unwrap();
        let (out, events) = run_manifest(run.path(), &jobs, None, &[], None);
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        assert_eq!(out.status.code(), Some(3), "{case}: {stderr}");
        assert_eq!(count(&last_counters(&events), "build.program"), 0, "{case}");
        assert!(out.stdout.is_empty(), "{case}: no job line");
        let names_both = (0..jobs.len()).all(|i| stderr.contains(&format!("batch 0 job {i}")));
        assert!(
            stderr.lines().count() == 1 && stderr.contains(why) && names_both,
            "{case}: {stderr}"
        );
        let written: Vec<_> = std::fs::read_dir(d.path()).unwrap().collect();
        assert!(written.is_empty(), "{case}: nothing written: {written:?}");
    }
    // The workspace's own file is untouched.
    assert!(
        std::fs::read_to_string(&app_json)
            .unwrap()
            .contains("\"id\"")
    );
}

/// T5 (a): a job that fails after an earlier one succeeded: both written, in
/// job order, each equal to its standalone run. And the write order itself:
/// two jobs whose writes both fail report their write errors in JOB order,
/// although job 1 (on view A) finishes before job 0 (whose last view is B).
#[test]
fn batch_writes_in_job_order() {
    let a = ws_a();
    let (_d, b) = ws_dep();
    let jobs = vec![
        argv(&[
            "events",
            "fanout",
            &a,
            "--format",
            "json",
            "--deterministic",
        ]),
        argv(&[
            "query",
            "touches",
            &a,
            "--table",
            "18",
            "--direction",
            "down",
        ]),
    ];
    let run = batch(&jobs);
    assert_ne!(
        standalone_in(&jobs[1], None).2,
        Some(0),
        "precondition: job 1 fails standalone"
    );
    assert_each_standalone(&run, &jobs, None);

    let d = tempfile::tempdir().unwrap();
    let missing = |n: &str| d.path().join("no-such-dir").join(n);
    let jobs = [
        job(&["diff", &a, &b], Some(&missing("0.out")), None),
        job(
            &["analyze", &a, "--single-app"],
            Some(&missing("1.out")),
            None,
        ),
    ];
    let (out, _) = run_manifest(d.path(), &jobs, None, &[], None);
    assert_eq!(out.status.code(), Some(0));
    let errors = write_errors(&out);
    assert_eq!(errors.len(), 2, "{errors:?}");
    assert!(
        errors[0].contains("0.out") && errors[1].contains("1.out"),
        "{errors:?}"
    );
}

/// T5 (c): the run's own stdout is a closed pipe: Decision 49's write error
/// on stderr, run exit 1 (a run-transport failure, not a job's).
#[test]
fn batch_closed_run_stdout_exits_1() {
    let d = tempfile::tempdir().unwrap();
    let manifest = d.path().join("m.json");
    let jobs = [job(&["events", "fanout", &ws_a()], None, None)];
    std::fs::write(&manifest, json!({ "jobs": jobs }).to_string()).unwrap();
    let (reader, writer) = std::io::pipe().unwrap();
    drop(reader);
    let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .arg("batch")
        .arg(&manifest)
        .env("RUST_LOG", "off")
        .stdout(writer)
        .stderr(Stdio::piped())
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{stderr}");
    assert!(
        stderr.starts_with("al-sem: write error: stdout: "),
        "{stderr}"
    );
    assert!(
        d.path().join("0.0.stdout").exists(),
        "the job itself was written"
    );
}

/// T5 (d): a job's own write failure is its line's exit (fingerprint 1,
/// analyze 2), with the write-error line on the run's stderr; the other jobs
/// are written and the run exits 0.
#[test]
fn batch_job_write_failures() {
    let ws = ws_a();
    let d = tempfile::tempdir().unwrap();
    let missing = d.path().join("no-such-dir");
    let fp_out = missing.join("fp.json").to_string_lossy().into_owned();
    let jobs = [
        job(
            &["fingerprint", &ws, "--format", "json", "--out", &fp_out],
            None,
            None,
        ),
        job(
            &["analyze", &ws, "--format", "json"],
            Some(&missing.join("a.sarif")),
            None,
        ),
        job(&["events", "fanout", &ws, "--format", "json"], None, None),
    ];
    let (out, _) = run_manifest(d.path(), &jobs, None, &[], None);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let lines = lines(&out);
    let exits: Vec<i64> = lines.iter().map(|l| l["exit"].as_i64().unwrap()).collect();
    assert_eq!(exits, [1, 2, 0]);
    let errors = write_errors(&out);
    assert_eq!(errors.len(), 2, "{errors:?}");
    assert!(
        errors[0].contains("fp.json") && errors[1].contains("a.sarif"),
        "{errors:?}"
    );
    let events_out = std::fs::read(d.path().join("0.2.stdout")).unwrap();
    assert!(!events_out.is_empty(), "the events job was written");
}

/// T24 (a), S18: a file/file `diff` (two snapshot files a separate run wrote)
/// between two workspace jobs runs before the first view, equals its
/// standalone run, and is written in job order (its line second).
#[test]
fn batch_zero_view_file_diff() {
    let ws = ws_a();
    let d = tempfile::tempdir().unwrap();
    let snap = |n: &str| {
        let p = d.path().join(n).to_string_lossy().into_owned();
        let (_, _, exit) = standalone_in(
            &argv(&[
                "fingerprint",
                &ws,
                "--format",
                "json",
                "--deterministic",
                "--out",
                &p,
            ]),
            None,
        );
        assert_eq!(exit, Some(0), "precondition: snapshot {n}");
        p
    };
    let (s1, s2) = (snap("s1.json"), snap("s2.json"));
    let jobs = vec![
        argv(&[
            "events",
            "fanout",
            &ws,
            "--format",
            "json",
            "--deterministic",
        ]),
        argv(&["diff", &s1, &s2, "--format", "json", "--deterministic"]),
        argv(&["analyze", &ws, "--format", "json", "--deterministic"]),
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

/// T24 (b): a batch whose every job fails its own validation builds nothing,
/// writes every job as standalone does, and exits 0.
#[test]
fn batch_every_job_fails_validation() {
    let ws = ws_a();
    let jobs = vec![
        argv(&["events", "fanout", &ws, "--format", "bad"]),
        argv(&["fingerprint", &ws, "--shard", "bad"]),
    ];
    let run = batch(&jobs);
    assert_each_standalone(&run, &jobs, None);
    assert_eq!(count(&run.counters, "build.program"), 0);
}

/// Review Focus 1 (D2.1): run from a cwd that is not the manifest's
/// directory, a manifest `stdout` lands under the manifest's directory and a
/// relative `--out` under the cwd, as standalone.
#[test]
fn batch_relative_paths_two_anchors() {
    let ws = ws_a();
    let (m, cwd) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let jobs = [job(
        &["fingerprint", &ws, "--format", "json", "--out", "f.json"],
        Some(Path::new("s.out")),
        None,
    )];
    let (out, _) = run_manifest(m.path(), &jobs, Some(cwd.path()), &[], None);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(lines(&out)[0]["exit"], 0);
    assert!(
        m.path().join("s.out").exists(),
        "stdout under the manifest's dir"
    );
    assert!(cwd.path().join("f.json").exists(), "--out under the cwd");
    assert!(!m.path().join("f.json").exists() && !cwd.path().join("s.out").exists());
}

/// D3 (§5.11): two `digest --diff -` jobs in one batch both get the one stdin
/// read, each equal to its standalone run on the same stdin.
#[test]
fn batch_stdin_read_once() {
    let ws = ws_a();
    let d = tempfile::tempdir().unwrap();
    let diff = std::fs::read(write_diff(
        d.path(),
        "src/D1MultiCaller.al",
        18,
        "        Cust.Modify();",
    ))
    .unwrap();
    let args = ["digest", ws.as_str(), "--diff", "-", "--deterministic"];
    let alone = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .args(args)
        .env("RUST_LOG", "off")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .and_then(|mut c| {
            c.stdin.take().unwrap().write_all(&diff)?;
            c.wait_with_output()
        })
        .unwrap();
    assert_eq!(
        alone.status.code(),
        Some(0),
        "precondition: the diff digests"
    );
    let human = [
        "digest",
        ws.as_str(),
        "--diff",
        "-",
        "--format",
        "human",
        "--deterministic",
    ];
    let jobs = [
        job(&args, None, None),
        job(&human, None, None),
        job(&args, None, None),
    ];
    let (out, _) = run_manifest(d.path(), &jobs, None, &[], Some(&diff));
    assert_eq!(out.status.code(), Some(0));
    for i in [0, 2] {
        let got = std::fs::read(d.path().join(format!("0.{i}.stdout"))).unwrap();
        assert_eq!(
            String::from_utf8_lossy(&got),
            String::from_utf8_lossy(&alone.stdout),
            "job {i}"
        );
    }
}

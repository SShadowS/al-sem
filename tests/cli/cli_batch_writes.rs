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
    run_manifest_doc(dir, &json!({ "jobs": jobs }), cwd, env, stdin)
}

/// [`run_manifest`] of a whole manifest document (`{"batches": …}`).
pub(crate) fn run_manifest_doc(
    dir: &Path,
    doc: &Value,
    cwd: Option<&Path>,
    env: &[(&str, &str)],
    stdin: Option<&[u8]>,
) -> (Output, Vec<Value>) {
    let manifest = dir.join("m.json");
    std::fs::write(&manifest, doc.to_string()).unwrap();
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

/// `dir/link` and `dir/b/link`, symlinks to `dir/sub/real`; `dir/b` exists.
/// The cases name paths under `dir` by their relative spelling. None where a
/// symlink needs a privilege.
fn symlinked_dir(dir: &Path) -> Option<impl Fn(&str) -> String> {
    std::fs::create_dir_all(dir.join("sub/real")).unwrap();
    std::fs::create_dir(dir.join("b")).unwrap();
    #[cfg(unix)]
    for link in ["link", "b/link"] {
        std::os::unix::fs::symlink(dir.join("sub/real"), dir.join(link)).unwrap();
    }
    let dir = dir.to_path_buf();
    cfg!(unix).then(move || move |rel: &str| dir.join(rel).to_string_lossy().into_owned())
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
    // R8-2: an upper-case `.AL` the source walk takes.
    let upper_al = dep.path().join("src/X.AL").to_string_lossy().into_owned();
    // R8-3: an `.app` in an ancestor `.alpackages` the loader reads.
    let a = tempfile::tempdir().unwrap();
    std::fs::create_dir(a.path().join("ws")).unwrap();
    let nested_ws = a.path().join("ws").to_string_lossy().into_owned();
    let ancestor_app = a
        .path()
        .join(".alpackages/y.app")
        .to_string_lossy()
        .into_owned();
    // R8-1: one directory under two spellings (a unix symlink; Windows needs
    // a privilege to make one, so the row is unix-only).
    let s = tempfile::tempdir().unwrap();
    let linked = symlinked_dir(s.path());
    let mut cases: Vec<(&str, Vec<Value>, &str)> = linked
        .iter()
        .flat_map(|at| {
            [
                (
                    "(R8-1) a write through a symlink, a read through its target",
                    vec![
                        events_out(&at("link/x.json")),
                        job(&["diff", &at("sub/real/x.json"), &ws], None, None),
                    ],
                    "which batch 0 job 0 writes",
                ),
                // `link/..` is the target's parent (`sub`), not `link`'s.
                (
                    "(R9-3) a write through link/.., a read of the target's parent",
                    vec![
                        events_out(&at("link/../x.json")),
                        job(&["diff", &at("sub/x.json"), &ws], None, None),
                    ],
                    "which batch 0 job 0 writes",
                ),
                // The loader walks the spelled workspace's ancestors (`b`),
                // not its target's (`sub`).
                (
                    "(R9-4) an .app beside a symlinked workspace's spelling",
                    vec![
                        job(&["events", "fanout", &at("b/link")], None, None),
                        events_out(&at("b/.alpackages/y.app")),
                    ],
                    "reads the workspace",
                ),
            ]
        })
        .collect();
    cases.extend([
        (
            "(R8-2) an upper-case .AL file in another job's workspace",
            vec![
                job(&["events", "fanout", &dep_ws], None, None),
                events_out(&upper_al),
            ],
            "reads the workspace",
        ),
        (
            "(R8-3) an .app in an ancestor .alpackages of another job's workspace",
            vec![
                job(&["events", "fanout", &nested_ws], None, None),
                events_out(&ancestor_app),
            ],
            "reads the workspace",
        ),
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
    ]);
    let refused = |case: &str, run: &Path, jobs: &[Value], why: &str| {
        let (out, events) = run_manifest(run, jobs, None, &[], None);
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
    };
    for (case, jobs, why) in cases {
        refused(case, tempfile::tempdir().unwrap().path(), &jobs, why);
    }
    // R8-4: a job's `--out` names another job's default stream file, which
    // the manifest puts beside itself.
    let run = tempfile::tempdir().unwrap();
    let stream = run.path().join("0.0.stdout").to_string_lossy().into_owned();
    refused(
        "(R8-4) a --out on another job's default stream file",
        run.path(),
        &[
            job(&["events", "fanout", &ws], None, None),
            events_out(&stream),
        ],
        "both write",
    );
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
    // A stream path that is a directory: its file cannot be created (a
    // missing stream directory is a usage error before any build, FR-13).
    let unwritable = |n: &str| {
        let p = d.path().join(n);
        std::fs::create_dir(&p).unwrap();
        p
    };
    let jobs = [
        job(&["diff", &a, &b], Some(&unwritable("0.out")), None),
        job(
            &["analyze", &a, "--single-app"],
            Some(&unwritable("1.out")),
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
    // A stream path that is a directory (a missing stream directory is a
    // usage error before any build, FR-13).
    let a_sarif = d.path().join("a.sarif");
    std::fs::create_dir(&a_sarif).unwrap();
    let jobs = [
        job(
            &["fingerprint", &ws, "--format", "json", "--out", &fp_out],
            None,
            None,
        ),
        job(&["analyze", &ws, "--format", "json"], Some(&a_sarif), None),
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
    // R8-4 (D2.2 order): job 1's stdout file failed, so its stderr file
    // (a writable default) was never created.
    assert!(
        !d.path().join("0.1.stderr").exists(),
        "a failed stdout file stops the job before its stderr file"
    );
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

/// #277 final r1 FR-2 (D2.1, T24 (b)): a job that fails its own validation
/// writes only its two streams. `fingerprint --shard` without `--out` and
/// `--shard bad`, run from the manifest's directory (their streams under the
/// cwd), and an `events fanout --format bad` whose `--out` names the next
/// job's stdout file, are each a job of exit 1 with its standalone text, and
/// the other job still runs.
#[test]
fn batch_failed_job_writes_only_its_streams() {
    let ws = ws_a();
    let d = tempfile::tempdir().unwrap();
    let jobs = [
        vec!["fingerprint", ws.as_str(), "--shard", "all"],
        vec!["fingerprint", ws.as_str(), "--shard", "bad"],
        vec![
            "events",
            "fanout",
            ws.as_str(),
            "--format",
            "bad",
            "--out",
            "0.3.stdout",
        ],
        vec![
            "events",
            "fanout",
            ws.as_str(),
            "--single-app",
            "--format",
            "json",
            "--deterministic",
        ],
    ];
    let manifest: Vec<Value> = jobs.iter().map(|a| job(a, None, None)).collect();
    let (out, _) = run_manifest(d.path(), &manifest, Some(d.path()), &[], None);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let got = lines(&out);
    for (i, a) in jobs.iter().enumerate() {
        let (stdout, stderr, exit) = standalone_in(&argv(a), Some(d.path()));
        if i < 3 {
            assert_eq!(exit, Some(1), "precondition: {a:?} fails its validation");
        }
        assert_eq!(got[i]["exit"].as_i64(), exit.map(i64::from), "job {i}");
        let read = |ext: &str| std::fs::read(d.path().join(format!("0.{i}.{ext}"))).unwrap();
        assert_eq!(read("stdout"), stdout, "job {i}: stdout");
        assert_eq!(
            String::from_utf8_lossy(&read("stderr")),
            String::from_utf8_lossy(&stderr),
            "job {i}: stderr"
        );
    }
}

/// `{"batches": …}` of one job per batch: `(argv, stdout)`.
fn one_job_batches(jobs: &[(Vec<&str>, Option<&Path>)]) -> Value {
    let batches: Vec<Value> = jobs
        .iter()
        .map(|(a, out)| json!({ "jobs": [job(a, *out, None)] }))
        .collect();
    json!({ "batches": batches })
}

/// #277 final r1 FR-3 (a) (D2.4): no job may write the manifest, which the
/// run reads. Five batches (more than `W`), batch 0 writing its stdout over
/// the manifest: a usage error before anything runs.
#[test]
fn batch_refuses_writing_the_manifest() {
    let ws = ws_a();
    let d = tempfile::tempdir().unwrap();
    let bad = vec!["events", "fanout", ws.as_str(), "--format", "bad"];
    let manifest = d.path().join("m.json");
    let mut jobs = vec![(bad.clone(), Some(manifest.as_path()))];
    jobs.extend((1..5).map(|_| (bad.clone(), None)));
    let (out, events) = run_manifest_doc(d.path(), &one_job_batches(&jobs), None, &[], None);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(3), "{stderr}");
    assert!(
        stderr.contains("the run reads") && stderr.contains("batch 0 job 0"),
        "{stderr}"
    );
    assert_eq!(count(&last_counters(&events), "build.program"), 0);
    assert!(!d.path().join("1.0.stdout").exists(), "nothing ran");
}

/// #277 final r1 FR-3 (b) (D3): whether `digest --changed foo.diff` names a
/// diff file or a routine is decided once, by the parent, before any job
/// runs. Batch 0 writes `foo.diff` (absent at the start); the digest, in batch
/// 4 (it starts only after a worker exits, `W` 4), still takes `foo.diff` as
/// a routine, as its standalone run before the batch did.
#[test]
fn batch_changed_is_decided_once() {
    let ws = ws_a();
    let d = tempfile::tempdir().unwrap();
    let digest = vec![
        "digest",
        ws.as_str(),
        "--changed",
        "foo.diff",
        "--deterministic",
    ];
    let (stdout, stderr, exit) = standalone_in(&argv(&digest), Some(d.path()));
    assert!(
        !d.path().join("foo.diff").exists(),
        "precondition: no foo.diff"
    );
    let analyze = vec!["analyze", ws.as_str(), "--format", "sarif"];
    let foo = d.path().join("foo.diff");
    let jobs = vec![
        (
            vec!["events", "fanout", ws.as_str(), "--format", "bad"],
            Some(foo.as_path()),
        ),
        (analyze.clone(), None),
        (analyze.clone(), None),
        (analyze, None),
        (digest, None),
    ];
    let (out, events) =
        run_manifest_doc(d.path(), &one_job_batches(&jobs), Some(d.path()), &[], None);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let w = events
        .iter()
        .find(|e| e["ph"] == "i" && e["name"] == "batch.workers")
        .map(|e| e["args"]["w"].clone());
    assert_eq!(
        w,
        Some(json!(4)),
        "precondition: batch 4 waits for a worker"
    );
    assert!(foo.exists(), "precondition: batch 0 wrote foo.diff");
    let got = lines(&out);
    assert_eq!(got[4]["exit"].as_i64(), exit.map(i64::from));
    let read = |ext: &str| std::fs::read(d.path().join(format!("4.0.{ext}"))).unwrap();
    assert_eq!(
        String::from_utf8_lossy(&read("stdout")),
        String::from_utf8_lossy(&stdout)
    );
    assert_eq!(
        String::from_utf8_lossy(&read("stderr")),
        String::from_utf8_lossy(&stderr)
    );
}

/// #277 final r1 FR-5 (D2.4): a workspace input that is a symlink to a file
/// outside the workspace (a source file, `app.json`) is read through the
/// link, so a job writing the link's target clashes with the read.
#[cfg(unix)]
#[test]
fn batch_refuses_writing_a_linked_workspace_input() {
    let (ws, outside) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let app = r#"{"id":"11111111-1111-1111-1111-111111111111","name":"A","publisher":"p","version":"1.0.0.0"}"#;
    std::fs::write(outside.path().join("app.json"), app).unwrap();
    std::fs::write(outside.path().join("A.al"), "codeunit 50100 A { }").unwrap();
    std::fs::create_dir(ws.path().join("src")).unwrap();
    std::os::unix::fs::symlink(outside.path().join("A.al"), ws.path().join("src/link.al")).unwrap();
    std::os::unix::fs::symlink(outside.path().join("app.json"), ws.path().join("app.json"))
        .unwrap();
    let w = ws.path().to_string_lossy().into_owned();
    for target in ["A.al", "app.json"] {
        let d = tempfile::tempdir().unwrap();
        let written = outside.path().join(target);
        let jobs = [
            job(&["events", "fanout", &w, "--single-app"], None, None),
            job(
                &["events", "fanout", &w, "--format", "bad"],
                Some(&written),
                None,
            ),
        ];
        let (out, events) = run_manifest(d.path(), &jobs, None, &[], None);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(3), "{target}: {stderr}");
        assert!(stderr.contains("reads the workspace"), "{target}: {stderr}");
        assert_eq!(count(&last_counters(&events), "build.program"), 0);
    }
}

/// #277 final r1 FR-13 (D2.1): a missing `--out-dir`, or a missing directory
/// of a manifest stream path, is a usage error before any build (exit 3).
#[test]
fn batch_missing_stream_directory_is_a_usage_error() {
    let ws = ws_a();
    let d = tempfile::tempdir().unwrap();
    let events = ["events", "fanout", ws.as_str(), "--single-app"];
    let manifest = d.path().join("m.json");
    let missing = d.path().join("missing");
    for (doc, out_dir) in [
        (
            json!({ "jobs": [job(&events, None, None)] }),
            Some(&missing),
        ),
        (
            json!({ "jobs": [job(&events, Some(&missing.join("x.out")), None)] }),
            None,
        ),
    ] {
        std::fs::write(&manifest, doc.to_string()).unwrap();
        let trace = d.path().join("trace.json");
        let _ = std::fs::remove_file(&trace);
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_alsem"));
        cmd.arg("batch")
            .env("RUST_LOG", "off")
            .env("ALSEM_TRACE", "1")
            .env("ALSEM_TRACE_FILE", &trace);
        if let Some(dir) = out_dir {
            cmd.arg("--out-dir").arg(dir);
        }
        let out = cmd.arg(&manifest).output().unwrap();
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(3), "{stderr}");
        assert!(stderr.contains("missing"), "names the directory: {stderr}");
        let text = std::fs::read_to_string(&trace).unwrap_or_default();
        let events: Vec<Value> = serde_json::from_str(&text).unwrap_or_default();
        assert_eq!(count(&last_counters(&events), "build.program"), 0);
        assert!(!missing.exists());
    }
}

/// #277 final r2 R2-1 (D2.4): a job that fails its own validation keeps its
/// reads in the write check. Batch 1's `digest --diff foo.diff` fails in
/// planning (no `foo.diff` yet), batch 0 writes `foo.diff`: a usage error
/// before anything runs, so no worker reads another job's output.
#[test]
fn batch_failed_job_keeps_its_reads() {
    let ws = ws_a();
    let d = tempfile::tempdir().unwrap();
    let jobs = vec![
        (
            vec![
                "digest",
                ws.as_str(),
                "--file",
                "src/D1MultiCaller.al",
                "--out",
                "foo.diff",
            ],
            None,
        ),
        (vec!["digest", ws.as_str(), "--diff", "foo.diff"], None),
    ];
    let (out, events) =
        run_manifest_doc(d.path(), &one_job_batches(&jobs), Some(d.path()), &[], None);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(3), "{stderr}");
    assert!(
        stderr.contains("batch 1 job 0 reads") && stderr.contains("batch 0 job 0 writes"),
        "{stderr}"
    );
    assert_eq!(count(&last_counters(&events), "build.program"), 0);
    assert!(!d.path().join("foo.diff").exists(), "nothing ran");
}

/// #277 final r2 R2-2 (D2.4): a linked directory in a workspace is a read
/// root. A new `.al` file under the target of `WS/shared`, and a new `.app`
/// under the target of a linked `WS/.alpackages`, are taken by the read, so
/// a job writing either clashes with it, though neither exists yet.
#[cfg(unix)]
#[test]
fn batch_refuses_writing_under_a_linked_directory() {
    let (ws, outside) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let app = r#"{"id":"11111111-1111-1111-1111-111111111111","name":"A","publisher":"p","version":"1.0.0.0"}"#;
    std::fs::write(ws.path().join("app.json"), app).unwrap();
    for (dir, link) in [("shared", "shared"), ("pkgs", ".alpackages")] {
        std::fs::create_dir(outside.path().join(dir)).unwrap();
        std::os::unix::fs::symlink(outside.path().join(dir), ws.path().join(link)).unwrap();
    }
    std::fs::write(outside.path().join("shared/A.al"), "codeunit 50100 A { }").unwrap();
    let w = ws.path().to_string_lossy().into_owned();
    for target in ["shared/New.al", "pkgs/New.app"] {
        let d = tempfile::tempdir().unwrap();
        let written = outside.path().join(target);
        let jobs = [
            job(&["events", "fanout", &w, "--single-app"], None, None),
            job(
                &["events", "fanout", &w, "--format", "bad"],
                Some(&written),
                None,
            ),
        ];
        let (out, events) = run_manifest(d.path(), &jobs, None, &[], None);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(3), "{target}: {stderr}");
        assert!(stderr.contains("reads the workspace"), "{target}: {stderr}");
        assert_eq!(count(&last_counters(&events), "build.program"), 0);
        assert!(!written.exists(), "{target}: nothing ran");
    }
}

/// #277 final r3 F1 (D2.4): a failed plan keeps every file its planning read
/// or probed. `policy check WS` auto-detects `WS/al-sem.policy.yaml`, which
/// does not load, so it fails in planning; a job writing that file clashes
/// with the read: exit 3, the policy untouched.
#[test]
fn batch_failed_plan_keeps_its_implicit_reads() {
    let ws = tempfile::tempdir().unwrap();
    let app = r#"{"id":"11111111-1111-1111-1111-111111111111","name":"A","publisher":"p","version":"1.0.0.0"}"#;
    std::fs::write(ws.path().join("app.json"), app).unwrap();
    std::fs::write(ws.path().join("A.al"), "codeunit 50100 A { }").unwrap();
    let policy = ws.path().join("al-sem.policy.yaml");
    std::fs::write(&policy, "rules: [").unwrap();
    let w = ws.path().to_string_lossy().into_owned();
    let d = tempfile::tempdir().unwrap();
    let jobs = [
        job(&["policy", "check", &w], None, None),
        job(
            &["events", "fanout", &w, "--format", "bad"],
            Some(&policy),
            None,
        ),
    ];
    let (out, events) = run_manifest(d.path(), &jobs, None, &[], None);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(3), "{stderr}");
    assert!(
        stderr.contains("batch 0 job 0 reads") && stderr.contains("batch 0 job 1 writes"),
        "{stderr}"
    );
    assert_eq!(count(&last_counters(&events), "build.program"), 0);
    assert_eq!(std::fs::read_to_string(&policy).unwrap(), "rules: [");
    assert!(!d.path().join("0.0.stdout").exists(), "nothing ran");
}

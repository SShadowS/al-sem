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
    // A refused run writes no trace (TLS-109): no earlier run's may be read.
    let _ = std::fs::remove_file(&trace);
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
    // A batch none of whose jobs reads stdin may exit before this write lands;
    // the test compares its output, so a closed pipe is not a failure.
    match pipe.write_all(stdin.unwrap_or_default()) {
        Err(e) if e.kind() != std::io::ErrorKind::BrokenPipe => panic!("write stdin: {e}"),
        _ => {}
    }
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

/// TLS-109/TLS-110: `jobs` run from `d` is refused before anything runs:
/// exit 3, one stderr line with `why`, nothing built, and every file in
/// `kept` still holds `KEEP` (no stream or `--out` was opened for writing).
fn assert_refused_untouched(case: &str, d: &Path, jobs: &[Value], why: &str, kept: &[&Path]) {
    let (out, events) = run_manifest(d, jobs, None, &[], None);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(3), "{case}: {stderr}");
    assert!(
        stderr.lines().count() == 1 && stderr.contains(why),
        "{case}: {stderr}"
    );
    assert_eq!(count(&last_counters(&events), "build.program"), 0, "{case}");
    for f in kept {
        assert_eq!(
            std::fs::read_to_string(f).unwrap(),
            "KEEP",
            "{case}: {} untouched",
            f.display()
        );
    }
}

/// A ready job writing `out` with `--out`, and a refused job whose stdout
/// is `stream`.
fn out_and_stream(ws: &str, out: &Path, stream: &Path) -> [Value; 2] {
    let out = out.to_string_lossy();
    [
        job(
            &["events", "fanout", ws, "--single-app", "--out", &out],
            None,
            None,
        ),
        job(
            &["events", "fanout", ws, "--format", "bad"],
            Some(stream),
            None,
        ),
    ]
}

/// TLS-109: a hard link is the file it links to. A stream that is a hard
/// link to another job's `--out` is refused, and the file is untouched (the
/// workspace case: `batch_refuses_a_hard_link_to_a_workspace_input`).
///
/// Discrimination (2026-11-21): `FileId`/`file_id` switched to the non-unix
/// (canonical path) fallback by editing their `cfg`s: fails at
/// "(hard link) another job's --out" (exit 0, not 3); restored, passes.
#[test]
fn batch_refuses_writing_through_a_hard_link() {
    let ws_a = ws_a();
    let d = tempfile::tempdir().unwrap();
    let real = d.path().join("real.json");
    std::fs::write(&real, "KEEP").unwrap();
    let linked = d.path().join("linked.out");
    std::fs::hard_link(&real, &linked).unwrap();
    assert_refused_untouched(
        "(hard link) another job's --out",
        d.path(),
        &out_and_stream(&ws_a, &real, &linked),
        "both write",
        &[&real],
    );
}

/// TLS-109: a symlink to a file, and a file reached through a symlinked
/// directory, are the file itself.
///
/// Discrimination (2026-11-21): `resolve` taking the path as spelled
/// (`symlink_metadata(base).map(|_| base.to_path_buf())` for `canonicalize`)
/// and `file_id` reading the link itself (`symlink_metadata` for
/// `metadata`): fails at "(symlink) a file link" (exit 0, not 3); restored,
/// passes.
#[cfg(unix)]
#[test]
fn batch_refuses_writing_through_a_symlink() {
    let ws_a = ws_a();
    let d = tempfile::tempdir().unwrap();
    let real = d.path().join("real.json");
    std::fs::write(&real, "KEEP").unwrap();
    let link = d.path().join("link.out");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    assert_refused_untouched(
        "(symlink) a file link",
        d.path(),
        &out_and_stream(&ws_a, &real, &link),
        "both write",
        &[&real],
    );
    let d = tempfile::tempdir().unwrap();
    std::fs::create_dir(d.path().join("sub")).unwrap();
    let real = d.path().join("sub/x.json");
    std::fs::write(&real, "KEEP").unwrap();
    std::os::unix::fs::symlink(d.path().join("sub"), d.path().join("ln")).unwrap();
    assert_refused_untouched(
        "(symlink) a directory link",
        d.path(),
        &out_and_stream(&ws_a, &real, &d.path().join("ln/x.json")),
        "both write",
        &[&real],
    );
}

/// TLS-109: `sub/../x.json` is `x.json`.
///
/// Discrimination (2026-11-21): `fold` dropping a `..` instead of popping
/// (`folded.pop()` deleted): fails (exit 0, not 3); restored, passes.
#[test]
fn batch_refuses_writing_through_dot_dot() {
    let ws_a = ws_a();
    let d = tempfile::tempdir().unwrap();
    std::fs::create_dir(d.path().join("sub")).unwrap();
    let real = d.path().join("x.json");
    std::fs::write(&real, "KEEP").unwrap();
    assert_refused_untouched(
        "(..) a parent spelling",
        d.path(),
        &out_and_stream(&ws_a, &real, &d.path().join("sub/../x.json")),
        "both write",
        &[&real],
    );
}

/// TLS-110: a refused job's directory target stays as it is. A refused
/// `fingerprint --shard bad --out D` whose own stdout is `D/x` is refused,
/// and `D`'s files are untouched. (Another job's write in `D` is that job's
/// own, as for a kept file: `batch_legitimate_manifests_still_run`.)
///
/// Discrimination (2026-11-21, r2): the kept directory's own-stream check
/// skipped (`*j == r` made `*j == r && false`): fails (exit 1, not 3: the write then
/// met the kept file's claim at its open); restored, passes.
#[test]
fn batch_refused_job_keeps_its_directory() {
    let ws = ws_a();
    let d = tempfile::tempdir().unwrap();
    let shards = d.path().join("shards");
    std::fs::create_dir(&shards).unwrap();
    let x = shards.join("x");
    std::fs::write(&x, "KEEP").unwrap();
    let s = shards.to_string_lossy().into_owned();
    assert_refused_untouched(
        "(refused dir) its own stdout",
        d.path(),
        &[job(
            &["fingerprint", &ws, "--shard", "bad", "--out", &s],
            Some(&x),
            None,
        )],
        "names the directory",
        &[&x],
    );
}

/// The app.json of [`tmp_ws`].
const APP: &str = r#"{"id":"11111111-1111-1111-1111-111111111111","name":"A","publisher":"p","version":"1.0.0.0"}"#;

/// A workspace of its own: [`APP`] and one codeunit. Returns it, its path,
/// and its app.json.
fn tmp_ws() -> (tempfile::TempDir, String, std::path::PathBuf) {
    let ws = tempfile::tempdir().unwrap();
    std::fs::write(ws.path().join("app.json"), APP).unwrap();
    std::fs::write(ws.path().join("A.al"), "codeunit 50100 A { }").unwrap();
    let app = ws.path().join("app.json");
    let w = ws.path().to_string_lossy().into_owned();
    (ws, w, app)
}

/// A run whose write was refused at its claim (TLS-109): exit 1, a write
/// error line saying so, and `kept` holds `want`.
fn assert_claim_refused(case: &str, out: &Output, kept: &Path, want: &str) {
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{case}: {stderr}");
    assert!(
        stderr.contains("al-sem: write error: ") && stderr.contains("refused"),
        "{case}: {stderr}"
    );
    assert_eq!(
        std::fs::read_to_string(kept).unwrap(),
        want,
        "{case}: untouched"
    );
}

/// TLS-109: a hard link, under a name no workspace read takes, to a
/// workspace's input is that input. A stream on it is refused before any job.
///
/// Discrimination (2026-11-21, r2): `WorkspaceRead::takes` without its
/// identity test of the existing inputs (the `taken.files` line deleted):
/// fails (exit 1, not 3: the write then met the input's claim at its open);
/// restored, passes.
#[test]
fn batch_refuses_a_hard_link_to_a_workspace_input() {
    let (_ws, w, app) = tmp_ws();
    let d = tempfile::tempdir().unwrap();
    let linked = d.path().join("app.txt");
    std::fs::hard_link(&app, &linked).unwrap();
    let (out, _) = run_manifest(
        d.path(),
        &[
            job(&["events", "fanout", &w, "--single-app"], None, None),
            job(
                &["events", "fanout", &w, "--format", "bad"],
                Some(&linked),
                None,
            ),
        ],
        None,
        &[],
        None,
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(3), "{stderr}");
    assert!(stderr.contains("reads the workspace"), "{stderr}");
    assert_eq!(std::fs::read_to_string(&app).unwrap(), APP);
}

/// TLS-109: a path changed between the write check and the open is still
/// checked, at the open. After the check (the debug seam
/// `ALSEM_TEST_AFTER_PREFLIGHT`), the refused job's stdout path becomes a
/// hard link to the other job's workspace's app.json: its open is refused,
/// the run exits 1, and app.json is untouched.
///
/// Discrimination (2026-11-21, r3): `claim::open_for_write` ignoring its
/// claim's refusal (`.or_else(|_| Ok(()))` on the claim): fails "(race) a
/// link made after the check" (exit 0, not 1); restored, passes. (r5) The
/// check's workspace-input claims skipped (`ws_reads.values().take(0)`):
/// fails (exit 0, not 1). (r5) The run's exit ignoring this process's failed
/// claim (`!whole || claim::failed()` made `!whole`): fails (exit 0, the
/// write refused); restored, both pass.
#[cfg(all(unix, debug_assertions))]
#[test]
fn batch_claims_the_file_it_opens() {
    let (_ws, w, app) = tmp_ws();
    let d = tempfile::tempdir().unwrap();
    let stream = d.path().join("late.out");
    let seam = format!("ln '{}' '{}'", app.display(), stream.display());
    let (out, _) = run_manifest(
        d.path(),
        &[
            job(&["events", "fanout", &w, "--single-app"], None, None),
            job(
                &["events", "fanout", &w, "--format", "bad"],
                Some(&stream),
                None,
            ),
        ],
        None,
        &[("ALSEM_TEST_AFTER_PREFLIGHT", &seam)],
        None,
    );
    assert_claim_refused("(race) a link made after the check", &out, &app, APP);
    let got = lines(&out);
    assert_eq!(got[0]["exit"].as_i64(), Some(0), "the other job ran");
    assert_eq!(
        got[1]["exit"].as_i64(),
        Some(1),
        "the refused write fails its job"
    );
}

/// TLS-109: two dangling links to one missing file are one file: the
/// preflight follows a dangling link's text.
///
/// Discrimination (2026-11-21, r2): `resolve` not following a dangling link
/// (its `read_link` branch's `hops < 40` made `hops > 40`): fails (exit 1,
/// not 3: the second open then met the first's claim, so nothing was
/// lost); restored, passes.
#[cfg(unix)]
#[test]
fn batch_refuses_two_dangling_links_to_one_file() {
    let ws = ws_a();
    let d = tempfile::tempdir().unwrap();
    let target = d.path().join("target.json");
    let (l1, l2) = (d.path().join("l1.json"), d.path().join("l2.out"));
    std::os::unix::fs::symlink(&target, &l1).unwrap();
    std::os::unix::fs::symlink(&target, &l2).unwrap();
    let (out, _) = run_manifest(d.path(), &out_and_stream(&ws, &l1, &l2), None, &[], None);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(3), "{stderr}");
    assert!(stderr.contains("both write"), "{stderr}");
    assert!(!target.exists(), "nothing ran");
}

/// TLS-109: a shard child is claimed like any write. `D/manifest.json`, a
/// hard link to the workspace's app.json, is refused at its open (no
/// preflight sees a shard child), and app.json is untouched.
///
/// Discrimination (2026-11-21, r2): `output::write_file` back to
/// `std::fs::write` (no claim): fails (exit 0, not 1); restored, passes.
/// Again on r3's claim (`.or_else(|_| Ok(()))` on the claim in
/// `open_for_write`): fails (exit 0, not 1).
#[test]
fn batch_claims_a_shard_child() {
    let (_ws, w, app) = tmp_ws();
    let d = tempfile::tempdir().unwrap();
    let shards = d.path().join("shards");
    std::fs::create_dir(&shards).unwrap();
    std::fs::hard_link(&app, shards.join("manifest.json")).unwrap();
    let s = shards.to_string_lossy().into_owned();
    let (out, _) = run_manifest(
        d.path(),
        &[job(
            &["fingerprint", &w, "--shard", "all", "--out", &s],
            None,
            None,
        )],
        None,
        &[],
        None,
    );
    assert_claim_refused("(shard child) a link to an input", &out, &app, APP);
}

/// TLS-109 (review r1 C): a stream that is a hard link to a file in a shard
/// directory. A ready job's directory is not walked: the shard child and the
/// stream meet at their claims, the first writer (job 0) keeps the file and
/// the stream is refused. A refused job's directory is walked: refused
/// before any job.
///
/// Discrimination: (2026-11-21, r3) `claim::open_for_write` ignoring its
/// claim's refusal (`.or_else(|_| Ok(()))`) fails "(ready dir)" (exit 0, not
/// 1); (r2) `KeptDir::holds` without its file test fails "(refused dir)"
/// (exit 1, not 3: the stream then met the kept file's claim); restored,
/// both pass.
#[test]
fn batch_refuses_a_stream_linked_into_a_shard_directory() {
    let ws = ws_a();
    let d = tempfile::tempdir().unwrap();
    let shards = d.path().join("shards");
    std::fs::create_dir(&shards).unwrap();
    let inside = shards.join("manifest.json");
    std::fs::write(&inside, "KEEP").unwrap();
    let linked = d.path().join("linked.out");
    std::fs::hard_link(&inside, &linked).unwrap();
    let s = shards.to_string_lossy().into_owned();
    let (out, _) = run_manifest(
        d.path(),
        &[
            job(
                &["fingerprint", &ws, "--shard", "all", "--out", &s],
                None,
                None,
            ),
            job(
                &["events", "fanout", &ws, "--format", "bad"],
                Some(&linked),
                None,
            ),
        ],
        None,
        &[],
        None,
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "(ready dir): {stderr}");
    assert!(stderr.contains("refused"), "(ready dir): {stderr}");
    let shard = std::fs::read_to_string(&inside).unwrap();
    assert!(
        shard.starts_with('{'),
        "(ready dir): job 0's shard: {shard}"
    );
    std::fs::write(&inside, "KEEP").unwrap();
    assert_refused_untouched(
        "(refused dir)",
        d.path(),
        &[job(
            &["fingerprint", &ws, "--shard", "bad", "--out", &s],
            Some(&linked),
            None,
        )],
        "names the directory",
        &[&inside],
    );
}

/// TLS-109: the trace file is published last and claimed like any write.
/// `ALSEM_TRACE_FILE` naming the manifest is refused at its open: no trace,
/// a stderr line, the manifest untouched, the run itself unharmed (exit 0).
///
/// Discrimination (2026-11-21, r5): `open_trace` ignoring its claim
/// (`let _ = claim(..)`): fails (the manifest holds the trace); restored,
/// passes. (r3) `perf_trace::defer_file` storing
/// `false` (the trace opened at start): fails (the manifest no longer the
/// one written: the trace truncated it before it was read); restored,
/// passes.
#[test]
fn batch_refuses_a_trace_file_on_the_manifest() {
    let ws = ws_a();
    let d = tempfile::tempdir().unwrap();
    let manifest = d.path().join("m.json");
    let jobs = [job(&["events", "fanout", &ws, "--single-app"], None, None)];
    let (out, _) = run_manifest(
        d.path(),
        &jobs,
        None,
        &[("ALSEM_TRACE_FILE", manifest.to_str().unwrap())],
        None,
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        std::fs::read_to_string(&manifest).unwrap(),
        json!({ "jobs": jobs }).to_string(),
        "{stderr}"
    );
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    assert!(
        stderr.contains("perf_trace: ") && stderr.contains("refused"),
        "{stderr}"
    );
}

/// TLS-109 (review r3): a trace file that is a hard link to a shard output
/// costs the trace, never the output: the trace opens after every job write
/// has claimed its file. `D/manifest.json` is a hard link to the named trace
/// file (outside `D`, so the check sees no clash): the shard is written, the
/// trace refused (a stderr line), the run exits 0.
///
/// Discrimination (2026-11-21, r4): the trace opened right after the check
/// (`start_trace_last()` moved before `run_one_batch`): fails (exit 1, not
/// 0: the trace took the file, the shard was refused); restored, passes.
#[test]
fn batch_trace_never_takes_a_shard_output() {
    let ws = ws_a();
    let d = tempfile::tempdir().unwrap();
    let trace = d.path().join("named-trace.json");
    std::fs::write(&trace, "KEEP").unwrap();
    let shards = d.path().join("shards");
    std::fs::create_dir(&shards).unwrap();
    let manifest_json = shards.join("manifest.json");
    std::fs::hard_link(&trace, &manifest_json).unwrap();
    let s = shards.to_string_lossy().into_owned();
    let (out, _) = run_manifest(
        d.path(),
        &[job(
            &["fingerprint", &ws, "--shard", "all", "--out", &s],
            None,
            None,
        )],
        None,
        &[("ALSEM_TRACE_FILE", trace.to_str().unwrap())],
        None,
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    assert!(
        stderr.contains("perf_trace: ") && stderr.contains("refused"),
        "{stderr}"
    );
    let shard = std::fs::read_to_string(&manifest_json).unwrap();
    assert!(shard.starts_with('{'), "the shard, not a trace: {shard}");
}

/// TLS-109 (review r4): a worker's trace never takes another worker's
/// output. Two batches (two workers); worker 0's trace file `T.w0` is a hard
/// link to `D/manifest.json`, which worker 1's shard job writes later
/// (worker 0 runs one refused job, worker 1 builds a model first). Traces are
/// published by the parent after every worker has ended: the shard is
/// written, `T.w0` refused (a stderr line), the run exits 0.
///
/// Discrimination (2026-11-21, r5): a worker publishing its own trace when
/// it ends (`perf_trace::publish_file(claim::open_trace)` at the end of
/// `run_worker`): fails (exit 1, not 0: worker 0's trace took the file and
/// the shard was refused); restored, passes.
#[test]
fn batch_worker_trace_never_takes_another_workers_output() {
    let ws = ws_a();
    let d = tempfile::tempdir().unwrap();
    let trace = d.path().join("t.json");
    let w0 = d.path().join("t.json.w0");
    std::fs::write(&w0, "KEEP").unwrap();
    let shards = d.path().join("shards");
    std::fs::create_dir(&shards).unwrap();
    let manifest_json = shards.join("manifest.json");
    std::fs::hard_link(&w0, &manifest_json).unwrap();
    let s = shards.to_string_lossy().into_owned();
    let doc = json!({ "batches": [
        { "jobs": [job(&["events", "fanout", &ws, "--format", "bad"], None, None)] },
        { "jobs": [job(&["fingerprint", &ws, "--shard", "all", "--out", &s], None, None)] },
    ]});
    let (out, _) = run_manifest_doc(
        d.path(),
        &doc,
        None,
        &[("ALSEM_TRACE_FILE", trace.to_str().unwrap())],
        None,
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    assert!(
        stderr.contains("t.json.w0") && stderr.contains("refused"),
        "{stderr}"
    );
    let shard = std::fs::read_to_string(&manifest_json).unwrap();
    assert!(shard.starts_with('{'), "the shard, not a trace: {shard}");
    let parent = std::fs::read_to_string(&trace).unwrap();
    assert!(parent.starts_with('['), "the parent's trace is published");
}

/// TLS-109 (review r4): a trace file may not truncate a refused job's named
/// file. Job 0 is refused and names `--out K`; `ALSEM_TRACE_FILE` is `K`:
/// the trace is refused (a stderr line), `K` untouched.
///
/// Discrimination (2026-11-21, r5): `open_trace` without its kept-list scan
/// (`read_dir(dir)?.take(0)`): fails (`K` holds the trace); restored,
/// passes.
#[test]
fn batch_trace_never_takes_a_kept_file() {
    let ws = ws_a();
    let d = tempfile::tempdir().unwrap();
    let kept = d.path().join("kept.json");
    std::fs::write(&kept, "KEEP").unwrap();
    let k = kept.to_string_lossy().into_owned();
    let (out, _) = run_manifest(
        d.path(),
        &[job(
            &["events", "fanout", &ws, "--format", "bad", "--out", &k],
            None,
            None,
        )],
        None,
        &[("ALSEM_TRACE_FILE", &k)],
        None,
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(std::fs::read_to_string(&kept).unwrap(), "KEEP", "{stderr}");
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    assert!(stderr.contains("refused"), "{stderr}");
}

/// TLS-110 (review r4): a refused job's kept list that cannot be read
/// refuses its writes (fail closed). After the check the seam makes the kept
/// list unreadable: the refused job's stream is refused, the run exits 1.
///
/// Discrimination (2026-11-21, r5): a kept list read error taken as no list
/// (`Err(e) => return Err(e)` made `Err(_) => String::new()`): fails (exit
/// 0, not 1); restored, passes.
#[cfg(all(unix, debug_assertions))]
#[test]
fn batch_refuses_when_a_kept_list_cannot_be_read() {
    let ws = ws_a();
    let d = tempfile::tempdir().unwrap();
    let kept = d.path().join("kept.json");
    std::fs::write(&kept, "KEEP").unwrap();
    let k = kept.to_string_lossy().into_owned();
    let (out, _) = run_manifest(
        d.path(),
        &[job(
            &["events", "fanout", &ws, "--format", "bad", "--out", &k],
            None,
            None,
        )],
        None,
        &[(
            "ALSEM_TEST_AFTER_PREFLIGHT",
            "chmod a-r \"$ALSEM_BATCH_CLAIMS\"/kept-*",
        )],
        None,
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{stderr}");
    assert!(stderr.contains("al-sem: write error: "), "{stderr}");
    assert_eq!(std::fs::read_to_string(&kept).unwrap(), "KEEP");
}

/// TLS-109 (review r2): a device is not a file a truncation could lose.
/// `--out /dev/null` standalone, and a batch stream `/dev/null`, write as
/// they always did (exit 0).
///
/// Discrimination (2026-11-21, r3): outside a batch, `open_for_write`
/// truncating with `set_len` as in a batch: fails "standalone" (exit 1,
/// EINVAL); in a batch, the regular-file test made `true`: fails "batch"
/// (exit 1, not 0); restored, both pass.
#[cfg(unix)]
#[test]
fn batch_and_standalone_write_to_a_device() {
    let ws = ws_a();
    let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .args(["fingerprint", &ws, "--format", "json", "--out", "/dev/null"])
        .env("RUST_LOG", "off")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "standalone: {stderr}");
    let d = tempfile::tempdir().unwrap();
    let (out, _) = run_manifest(
        d.path(),
        &[job(
            &["events", "fanout", &ws, "--single-app"],
            Some(Path::new("/dev/null")),
            None,
        )],
        None,
        &[],
        None,
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "batch: {stderr}");
    assert_eq!(lines(&out)[0]["exit"].as_i64(), Some(0), "batch: {stderr}");
}

/// TLS-109 (review r2): every write is its own claim. A shard job whose
/// stdout is spelled `D/manifest.json`, a link to a file outside `D` (so the
/// check sees no clash), writes that file twice: its stream, then its shard
/// child of the same spelling. The second open is refused (exit 1).
///
/// Discrimination (2026-11-21, r3): an existing marker no longer refusing
/// (`if !(reserved && ...)` made `if false`): fails (exit 0, not 1);
/// restored, passes.
#[cfg(unix)]
#[test]
fn batch_refuses_a_second_open_of_one_spelling() {
    let ws = ws_a();
    let d = tempfile::tempdir().unwrap();
    let shards = d.path().join("shards");
    std::fs::create_dir(&shards).unwrap();
    let outside = d.path().join("outside.json");
    std::fs::write(&outside, "KEEP").unwrap();
    let spelled = shards.join("manifest.json");
    std::os::unix::fs::symlink(&outside, &spelled).unwrap();
    let s = shards.to_string_lossy().into_owned();
    let (out, _) = run_manifest(
        d.path(),
        &[job(
            &["fingerprint", &ws, "--shard", "all", "--out", &s],
            Some(&spelled),
            None,
        )],
        None,
        &[],
        None,
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{stderr}");
    assert!(stderr.contains("refused"), "{stderr}");
}

/// A two-job run whose job 1 (refused, `--format bad`) has its stdout at
/// `late`, which the seam then makes a hard link to `target` after the check.
/// `job0`: job 0's argv. Returns the run.
#[cfg(all(unix, debug_assertions))]
fn late_link_run(d: &Path, job0: &[&str], ws: &str, target: &Path) -> Output {
    let late = d.join("late.out");
    let seam = format!("ln '{}' '{}'", target.display(), late.display());
    run_manifest(
        d,
        &[
            job(job0, None, None),
            job(
                &["events", "fanout", ws, "--format", "bad"],
                Some(&late),
                None,
            ),
        ],
        None,
        &[("ALSEM_TEST_AFTER_PREFLIGHT", &seam)],
        None,
    )
    .0
}

/// TLS-109 (review r2): an explicit read is claimed by the check. Job 0
/// reads `--baseline B`; after the check job 1's stdout becomes a link to
/// `B`: its open is refused, `B` is untouched.
///
/// Discrimination (2026-11-21, r3): the check's claim of a job's reads
/// skipped (`existing(&read).filter(|_| false)`): fails (exit 0, not 1);
/// restored, passes.
#[cfg(all(unix, debug_assertions))]
#[test]
fn batch_claims_an_explicit_read() {
    let ws = ws_a();
    let d = tempfile::tempdir().unwrap();
    let base = d.path().join("base.json");
    let text = r#"{"schemaVersion":"1","fingerprints":[]}"#;
    std::fs::write(&base, text).unwrap();
    let b = base.to_string_lossy().into_owned();
    let out = late_link_run(
        d.path(),
        &["analyze", &ws, "--single-app", "--baseline", &b],
        &ws,
        &base,
    );
    assert_claim_refused("(late link) a --baseline", &out, &base, text);
}

/// TLS-109/TLS-110 (review r2): a refused job's named file is kept from its
/// own streams at their open. Job 1 is refused and names `--out K`; after
/// the check its stdout becomes a link to `K`: refused, `K` untouched.
///
/// Discrimination (2026-11-21, r3): `claim::keep` writing nothing: fails
/// (exit 0, not 1); restored, passes.
#[cfg(all(unix, debug_assertions))]
#[test]
fn batch_keeps_a_refused_jobs_file_at_its_open() {
    let ws = ws_a();
    let d = tempfile::tempdir().unwrap();
    let kept = d.path().join("kept.json");
    std::fs::write(&kept, "KEEP").unwrap();
    let k = kept.to_string_lossy().into_owned();
    let late = d.path().join("late.out");
    let seam = format!("ln '{}' '{}'", kept.display(), late.display());
    let (out, _) = run_manifest(
        d.path(),
        &[job(
            &["events", "fanout", &ws, "--format", "bad", "--out", &k],
            Some(&late),
            None,
        )],
        None,
        &[("ALSEM_TEST_AFTER_PREFLIGHT", &seam)],
        None,
    );
    assert_claim_refused("(late link) a refused job's --out", &out, &kept, "KEEP");
}

/// TLS-109 (review r2): the manifest is claimed by the check, apart from any
/// trace file. After the check job 1's stdout becomes a link to the
/// manifest: refused, the manifest untouched.
///
/// Discrimination (2026-11-21, r3): the run's read of its manifest dropped
/// (`reads: vec![]` for "the run"): fails (exit 0, not 1); restored, passes.
#[cfg(all(unix, debug_assertions))]
#[test]
fn batch_claims_its_manifest() {
    let ws = ws_a();
    let d = tempfile::tempdir().unwrap();
    let manifest = d.path().join("m.json");
    let late = d.path().join("late.out");
    let seam = format!("ln '{}' '{}'", manifest.display(), late.display());
    let jobs = [
        job(&["events", "fanout", &ws, "--single-app"], None, None),
        job(
            &["events", "fanout", &ws, "--format", "bad"],
            Some(&late),
            None,
        ),
    ];
    let (out, _) = run_manifest(
        d.path(),
        &jobs,
        None,
        &[("ALSEM_TEST_AFTER_PREFLIGHT", &seam)],
        None,
    );
    let text = json!({ "jobs": jobs }).to_string();
    assert_claim_refused("(late link) the manifest", &out, &manifest, &text);
}

/// TLS-109 (review r2): workers claim in their parent's folder. Two
/// batches (two workers); after the check, batch 1's stdout becomes a link
/// to batch 0's workspace's app.json: its open in its worker is refused,
/// the parent exits 1, app.json is untouched, and batch 0's job ran.
///
/// Discrimination (2026-11-21, r3): the hand-off (`cmd.env(claim::ENV,
/// ...)` in `worker::spawn`) deleted: fails "batch 0's line" (each worker
/// refused to run: no job line, exit null); restored, passes.
#[cfg(all(unix, debug_assertions))]
#[test]
fn batch_workers_claim_in_the_parents_folder() {
    let (_ws, w, app) = tmp_ws();
    let d = tempfile::tempdir().unwrap();
    let late = d.path().join("late.out");
    let seam = format!("ln '{}' '{}'", app.display(), late.display());
    let doc = json!({ "batches": [
        { "jobs": [job(&["events", "fanout", &w, "--single-app"], None, None)] },
        { "jobs": [job(&["events", "fanout", &w, "--format", "bad"], Some(&late), None)] },
    ]});
    let (out, _) = run_manifest_doc(
        d.path(),
        &doc,
        None,
        &[("ALSEM_TEST_AFTER_PREFLIGHT", &seam)],
        None,
    );
    let got = lines(&out);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(got[0]["exit"].as_i64(), Some(0), "batch 0's line: {stderr}");
    assert_eq!(got[1]["exit"].as_i64(), Some(1), "batch 1's line: {stderr}");
    assert_claim_refused("(workers) a late link", &out, &app, APP);
}

/// TLS-109 (review r2): a worker started without its parent's claim folder
/// refuses to run (exit 3) and writes nothing.
#[test]
fn batch_worker_needs_its_claim_folder() {
    let d = tempfile::tempdir().unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .args(["batch", "--worker", "0"])
        .arg(d.path().join("m.json"))
        .env_remove("ALSEM_BATCH_CLAIMS")
        .env("RUST_LOG", "off")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(3), "{stderr}");
    assert!(stderr.contains("no claim folder"), "{stderr}");
    assert!(out.stdout.is_empty());
}

/// TLS-109 (review r2): a relative `--update-baseline` from the run's cwd
/// rewrites its baseline: the check's reservation and the write name it by
/// the same spelling.
///
/// Discrimination (2026-11-21, r3): the reservation's key spelled
/// differently from the write's (`write_key` given the resolved path):
/// fails (exit 1, not 0: the write met the reservation as a foreign claim);
/// restored, passes.
#[test]
fn batch_updates_a_relative_baseline() {
    let ws = ws_a();
    let d = tempfile::tempdir().unwrap();
    let text = r#"{"schemaVersion":"1","fingerprints":[]}"#;
    std::fs::write(d.path().join("base.json"), text).unwrap();
    let (out, _) = run_manifest(
        d.path(),
        &[job(
            &[
                "analyze",
                &ws,
                "--single-app",
                "--baseline",
                "base.json",
                "--update-baseline",
            ],
            None,
            None,
        )],
        Some(d.path()),
        &[],
        None,
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    assert_eq!(lines(&out)[0]["exit"].as_i64(), Some(0), "{stderr}");
    assert_ne!(
        std::fs::read_to_string(d.path().join("base.json")).unwrap(),
        text,
        "updated"
    );
}

/// TLS-109/TLS-110: what is not a clash still runs. An `--update-baseline`
/// writing the baseline it read, two disjoint `--out`s, and a ready job
/// writing in a refused job's directory: exit 0, each written.
///
/// Discrimination (2026-11-21, r3): the reads check refusing a job's own
/// `--update-baseline` too (`Some(w) != update || true`): fails (exit 3, not
/// 0); restored, passes.
#[test]
fn batch_legitimate_manifests_still_run() {
    let ws = ws_a();
    let d = tempfile::tempdir().unwrap();
    let base = d.path().join("base.json");
    std::fs::write(&base, r#"{"schemaVersion":"1","fingerprints":[]}"#).unwrap();
    let shards = d.path().join("shards");
    std::fs::create_dir(&shards).unwrap();
    let p = |f: &Path| f.to_string_lossy().into_owned();
    let (a, b, y) = (
        d.path().join("a.json"),
        d.path().join("b.json"),
        shards.join("y.json"),
    );
    let jobs = [
        job(
            &[
                "analyze",
                &ws,
                "--single-app",
                "--baseline",
                &p(&base),
                "--update-baseline",
            ],
            None,
            None,
        ),
        job(
            &["events", "fanout", &ws, "--single-app", "--out", &p(&a)],
            None,
            None,
        ),
        job(
            &["events", "fanout", &ws, "--single-app", "--out", &p(&b)],
            None,
            None,
        ),
        job(
            &["fingerprint", &ws, "--shard", "bad", "--out", &p(&shards)],
            None,
            None,
        ),
        job(
            &["events", "fanout", &ws, "--single-app", "--out", &p(&y)],
            None,
            None,
        ),
    ];
    let (out, _) = run_manifest(d.path(), &jobs, None, &[], None);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    let got = lines(&out);
    for i in [0, 1, 2, 4] {
        assert_eq!(got[i]["exit"].as_i64(), Some(0), "job {i}: {stderr}");
    }
    for f in [&a, &b, &y] {
        assert!(
            std::fs::metadata(f).unwrap().len() > 0,
            "{} written",
            f.display()
        );
    }
    assert_ne!(
        std::fs::read_to_string(&base).unwrap(),
        r#"{"schemaVersion":"1","fingerprints":[]}"#,
        "the baseline updated"
    );
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

/// A project for the read-coverage tests (TLS-109 review r3): `R/.git`
/// (auto-discovery's search root), the workspace `R/ws` ([`APP`], `src/A.al`),
/// `R/other` (an app that does not depend on it: a rejected candidate),
/// `R/rel` (a test app to name), `R/dep/B.al` (a source folder) and
/// `R/pkgs` (`.alpackages/x.app`, `src/a.al`). The tests compare a file's
/// bytes before and after.
fn project() -> tempfile::TempDir {
    let r = tempfile::tempdir().unwrap();
    let p = |rel: &str| r.path().join(rel);
    for dir in [
        ".git",
        "ws/src",
        "other",
        "rel",
        "dep",
        "pkgs/.alpackages",
        "pkgs/src",
    ] {
        std::fs::create_dir_all(p(dir)).unwrap();
    }
    std::fs::write(p("ws/app.json"), APP).unwrap();
    std::fs::write(p("ws/src/A.al"), "codeunit 50100 A { }").unwrap();
    let other = r#"{"id":"22222222-2222-2222-2222-222222222222","name":"O","publisher":"p","version":"1.0.0.0"}"#;
    std::fs::write(p("other/app.json"), other).unwrap();
    std::fs::write(p("rel/app.json"), other).unwrap();
    std::fs::write(p("dep/B.al"), "codeunit 50101 B { }").unwrap();
    std::fs::write(p("pkgs/.alpackages/x.app"), "not a real app").unwrap();
    std::fs::write(p("pkgs/src/a.al"), "codeunit 50102 C { }").unwrap();
    r
}

/// A run of `job0` (a ready job reading the project) and a refused job whose
/// stdout is `stream`, from `cwd`. Returns the run.
fn with_stream(cwd: Option<&Path>, job0: &[&str], stream: &Path) -> Output {
    let ws = ws_a();
    let d = tempfile::tempdir().unwrap();
    run_manifest(
        d.path(),
        &[
            job(job0, None, None),
            job(
                &["events", "fanout", &ws, "--format", "bad"],
                Some(stream),
                None,
            ),
        ],
        cwd,
        &[],
        None,
    )
    .0
}

/// The check refused the run (exit 3, a workspace read) and `file` is as it
/// was.
fn assert_read_refused(case: &str, out: &Output, file: &Path, want: &str) {
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(3), "{case}: {stderr}");
    assert!(stderr.contains("reads the workspace"), "{case}: {stderr}");
    assert_eq!(std::fs::read_to_string(file).unwrap(), want, "{case}");
}

/// TLS-109 (review r3): a test scope read takes every app it reads, also a
/// candidate it rejects (`R/other` does not depend on the workspace): a
/// stream on its `app.json` is refused.
///
/// Discrimination (2026-11-21, r4): `test_scope_reads` registering only the
/// accepted apps (`discover(..).apps` roots instead of `.read`): fails (exit
/// 0, not 3); restored, passes.
#[test]
fn batch_protects_a_rejected_test_scope_candidate() {
    let r = project();
    let ws = r.path().join("ws").to_string_lossy().into_owned();
    let app = r.path().join("other/app.json");
    let want = std::fs::read_to_string(&app).unwrap();
    let out = with_stream(
        None,
        &["tests", &ws, "--part", "roots", "--file", "src/A.al"],
        &app,
    );
    assert_read_refused("(scope) a rejected candidate", &out, &app, &want);
}

/// TLS-109 (review r3): a relative `--test-app` names the folder under the
/// cwd, as the job takes it: run from `R`, `--test-app rel` is `R/rel`, and
/// a stream on its `app.json` is refused.
///
/// Discrimination (2026-11-21, r4): `test_scope_reads` passing the
/// `--test-app` spelling unresolved (`discover` then joins it to the
/// workspace): fails (exit 0, not 3); restored, passes.
#[test]
fn batch_protects_a_relative_test_app() {
    let r = project();
    let app = r.path().join("rel/app.json");
    let want = std::fs::read_to_string(&app).unwrap();
    let out = with_stream(
        Some(r.path()),
        &[
            "tests",
            "ws",
            "--part",
            "roots",
            "--file",
            "src/A.al",
            "--test-app",
            "rel",
        ],
        &app,
    );
    assert_read_refused("(scope) a relative --test-app", &out, &app, &want);
}

/// TLS-109 (review r2): `impact --source-dep` is a folder relative to the
/// workspace, as the loader takes it: `../dep` is `R/dep`, whose sources are
/// inputs; a stream on one is refused.
///
/// Discrimination (2026-11-21, r4): the source dependency joined to the cwd
/// (`PathBuf::from(s)` for `ws.join(s)` in `job_paths`): fails (exit 0, not
/// 3); restored, passes.
#[test]
fn batch_protects_a_relative_source_dependency() {
    let r = project();
    let ws = r.path().join("ws").to_string_lossy().into_owned();
    let b = r.path().join("dep/B.al");
    let out = with_stream(
        None,
        &[
            "impact",
            &ws,
            "--part",
            "main",
            "--file",
            "src/A.al",
            "--source-dep",
            "../dep",
        ],
        &b,
    );
    assert_read_refused(
        "(impact) --source-dep ../dep",
        &out,
        &b,
        "codeunit 50101 B { }",
    );
}

/// TLS-109 (review r2): an `impact --package-root` folder is read for its
/// packages only: a stream on its `.alpackages/x.app` is refused, a new
/// file under its `src` is not.
///
/// Discrimination (2026-11-21, r4): package roots read as whole workspaces
/// (`(w, true)` for `(w, false)` in `check_write_sets`): fails "a new
/// source" (exit 3, not 0); restored, passes.
#[test]
fn batch_reads_a_package_root_for_its_packages() {
    let r = project();
    let ws = r.path().join("ws").to_string_lossy().into_owned();
    let pkgs = r.path().join("pkgs").to_string_lossy().into_owned();
    let job0 = [
        "impact",
        &ws,
        "--part",
        "main",
        "--file",
        "src/A.al",
        "--package-root",
        &pkgs,
    ];
    let app = r.path().join("pkgs/.alpackages/x.app");
    let out = with_stream(None, &job0, &app);
    assert_read_refused("(impact) a package", &out, &app, "not a real app");
    let out = with_stream(None, &job0, &r.path().join("pkgs/src/new.al"));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "a new source: {stderr}");
}

/// TLS-109 (review r3): a dangling link in a workspace is no input (the
/// source walk skips it), so the check skips it too instead of refusing the
/// run.
///
/// Discrimination (2026-11-21, r4): `taken_files` refusing on a missing
/// link target (its `NotFound` skip removed): fails (exit 3, not 0);
/// restored, passes.
#[cfg(unix)]
#[test]
fn batch_skips_a_dangling_link_in_a_workspace() {
    let (_ws, w, _) = tmp_ws();
    std::os::unix::fs::symlink(Path::new(&w).join("gone.al"), Path::new(&w).join("link.al"))
        .unwrap();
    let d = tempfile::tempdir().unwrap();
    let (out, _) = run_manifest(
        d.path(),
        &[job(&["events", "fanout", &w, "--single-app"], None, None)],
        None,
        &[],
        None,
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "{stderr}");
}

/// TLS-109 (review r2): on unix a `..` folds only over an existing folder:
/// `f.txt/../x.json` (a file before the `..`) is not `x.json`, so a write of
/// each is no clash (the first fails on its own, as standalone).
///
/// Discrimination (2026-11-21, r4): `fold` popping over any existing path
/// (its `canonical.is_dir()` guard removed): fails (exit 3 "both write", not
/// 0); restored, passes.
#[cfg(unix)]
#[test]
fn batch_dot_dot_after_a_file_is_not_its_parent() {
    let ws = ws_a();
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("f.txt"), "KEEP").unwrap();
    let (out, _) = run_manifest(
        d.path(),
        &out_and_stream(
            &ws,
            &d.path().join("f.txt/../x.json"),
            &d.path().join("x.json"),
        ),
        None,
        &[],
        None,
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "{stderr}");
}

/// TLS-110 (review r2): a refused job's existing file in its directory is
/// kept from its own streams at their open, also while another job writes
/// in that directory. Job 0 is refused and names `--out D` (`D/x` holds
/// `KEEP`); job 1 writes `D/y`; after the check job 0's stdout becomes a link
/// to `D/x`: refused, `D/x` untouched.
///
/// Discrimination (2026-11-21, r4): the kept list leaving out a kept
/// directory's files (`kept_dirs[d].files` not added): fails (exit 0, not
/// 1); restored, passes.
#[cfg(all(unix, debug_assertions))]
#[test]
fn batch_keeps_a_refused_jobs_directory_file_at_its_open() {
    let ws = ws_a();
    let d = tempfile::tempdir().unwrap();
    let shards = d.path().join("shards");
    std::fs::create_dir(&shards).unwrap();
    let x = shards.join("x");
    std::fs::write(&x, "KEEP").unwrap();
    let (s, y) = (
        shards.to_string_lossy().into_owned(),
        shards.join("y.json").to_string_lossy().into_owned(),
    );
    let late = d.path().join("late.out");
    let seam = format!("ln '{}' '{}'", x.display(), late.display());
    let (out, _) = run_manifest(
        d.path(),
        &[
            job(
                &["fingerprint", &ws, "--shard", "bad", "--out", &s],
                Some(&late),
                None,
            ),
            job(
                &["events", "fanout", &ws, "--single-app", "--out", &y],
                None,
                None,
            ),
        ],
        None,
        &[("ALSEM_TEST_AFTER_PREFLIGHT", &seam)],
        None,
    );
    assert_claim_refused("(late link) a kept directory's file", &out, &x, "KEEP");
}

/// TLS-109 (review r3): a claim that cannot be made fails the run, through
/// the worker's exit, not the claim folder. After the check the seam makes
/// the claim folder read-only: every worker's first claim fails, each job's
/// write fails, and the parent exits 1.
///
/// Discrimination (2026-11-21, r4): the parent not reading its workers'
/// claim exits (`claims_failed` not or'ed in `run_workers`): fails (exit 0,
/// not 1); restored, passes.
#[cfg(all(unix, debug_assertions))]
#[test]
fn batch_fails_when_a_claim_cannot_be_made() {
    let ws = ws_a();
    let d = tempfile::tempdir().unwrap();
    let saved = d.path().join("claims.txt");
    let seam = format!(
        "chmod a-w \"$ALSEM_BATCH_CLAIMS\"; printf %s \"$ALSEM_BATCH_CLAIMS\" > '{}'",
        saved.display()
    );
    let one = || json!({ "jobs": [job(&["events", "fanout", &ws, "--single-app"], None, None)] });
    let (out, _) = run_manifest_doc(
        d.path(),
        &json!({ "batches": [one(), one()] }),
        None,
        &[("ALSEM_TEST_AFTER_PREFLIGHT", &seam)],
        None,
    );
    // Let the run's folder be removed again.
    if let Ok(dir) = std::fs::read_to_string(&saved) {
        let _ = Command::new("chmod").args(["u+w", &dir]).status();
        let _ = std::fs::remove_dir_all(&dir);
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{stderr}");
    assert!(stderr.contains("al-sem: write error: "), "{stderr}");
    assert_eq!(lines(&out)[0]["exit"].as_i64(), Some(1), "{stderr}");
}

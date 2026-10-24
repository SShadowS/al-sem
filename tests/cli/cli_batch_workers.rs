//! #277 Task 10 (spec D2.5, D3, D10, D12; T8, T10, T17; Review Focus 4): a
//! run of two or more batches runs each batch in a worker process, at most
//! `W` at once (`batch.workers`), each with its own trace file; the parent
//! hands its one stdin read to the workers that need it, forwards their
//! stderr whole line by whole line, and gives the jobs of a worker that died
//! without its lines a null exit and the run exit 1.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

use serde_json::{Value, json};

use crate::cli_batch_writes::{job, lines, run_manifest_doc};
use crate::cli_build_counters::{write_diff, ws_a};

/// `batches` (each a list of argvs) as one manifest, run in `dir`.
fn run_batches(
    dir: &Path,
    batches: &[Vec<Vec<&str>>],
    env: &[(&str, &str)],
    stdin: Option<&[u8]>,
) -> (Output, Vec<Value>) {
    let batches: Vec<Value> = batches
        .iter()
        .map(|jobs| json!({ "jobs": jobs.iter().map(|a| job(a, None, None)).collect::<Vec<_>>() }))
        .collect();
    run_manifest_doc(dir, &json!({ "batches": batches }), None, env, stdin)
}

/// The trace's instants named `name`, in emission order.
fn instants<'a>(events: &'a [Value], name: &str) -> Vec<&'a Value> {
    events
        .iter()
        .filter(|e| e["ph"] == "i" && e["name"] == name)
        .collect()
}

/// The most workers alive at once, by the parent's start/exit instants.
fn most_alive(events: &[Value]) -> usize {
    let (mut alive, mut most) = (0usize, 0usize);
    for e in events.iter().filter(|e| e["ph"] == "i") {
        match e["name"].as_str() {
            Some("batch.worker.start") => alive += 1,
            Some("batch.worker.exit") => alive -= 1,
            _ => continue,
        }
        most = most.max(alive);
    }
    most
}

/// Worker `b`'s own trace (D10: `<ALSEM_TRACE_FILE>.w<b>`).
fn worker_trace(dir: &Path, b: usize) -> Vec<Value> {
    let text = std::fs::read_to_string(dir.join(format!("trace.json.w{b}")))
        .unwrap_or_else(|e| panic!("worker {b}'s trace file: {e}"));
    serde_json::from_str(&text).expect("a worker trace parses")
}

/// T17 (cli, the committed table): 1, 3 and 5 analyze batches run on 1, 3
/// and 4 workers, 3 batches with one `policy check` on 2; never more than
/// `W` workers alive. One batch runs in-process (no worker).
#[test]
fn batch_worker_count() {
    let ws = ws_a();
    let analyze = vec!["analyze", ws.as_str(), "--format", "sarif"];
    let policy = vec!["policy", "check", ws.as_str(), "--format", "json"];
    let cases = [
        ("1 batch", vec![vec![analyze.clone()]], 1),
        ("3 batches", vec![vec![analyze.clone()]; 3], 3),
        ("5 batches", vec![vec![analyze.clone()]; 5], 4),
        (
            "3 batches, one policy check",
            vec![vec![analyze.clone()], vec![policy], vec![analyze.clone()]],
            2,
        ),
    ];
    for (case, batches, w) in cases {
        let d = tempfile::tempdir().unwrap();
        let census = [("ALSEM_RETAINED_CENSUS", "1")];
        let env: &[(&str, &str)] = if case == "3 batches" { &census } else { &[] };
        let (out, events) = run_batches(d.path(), &batches, env, None);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(0), "{case}: {stderr}");
        let workers = instants(&events, "batch.workers");
        assert_eq!(workers.len(), 1, "{case}: one batch.workers");
        assert_eq!(workers[0]["args"]["w"], w, "{case}: W");
        let n = batches.len();
        let spawned = if n == 1 { 0 } else { n };
        assert_eq!(
            instants(&events, "batch.worker.start").len(),
            spawned,
            "{case}"
        );
        assert_eq!(
            instants(&events, "batch.worker.exit").len(),
            spawned,
            "{case}"
        );
        assert_eq!(
            most_alive(&events),
            if n == 1 { 0 } else { w },
            "{case}: alive"
        );
        let got = lines(&out);
        assert_eq!(got.len(), n, "{case}: one line per job");
        for (b, line) in got.iter().enumerate() {
            assert_eq!(
                (&line["batch"], &line["job"]),
                (&json!(b), &json!(0)),
                "{case}"
            );
            assert!(line["exit"].is_u64(), "{case}: batch {b} ran: {line}");
            assert!(line.get("worker").is_none(), "{case}: batch {b}");
        }
        for b in (0..spawned).filter(|_| n > 1) {
            let own = worker_trace(d.path(), b);
            assert_eq!(
                instants(&own, "batch.driver").len(),
                1,
                "{case}: worker {b}"
            );
        }
        if !env.is_empty() && cfg!(target_os = "linux") {
            for b in 0..n {
                let peak = format!("[w{b}] [batch] worker {b} peak_kib=");
                assert!(
                    stderr.lines().any(|l| l.starts_with(&peak)),
                    "{case}: {peak} in {stderr}"
                );
            }
            for l in stderr.lines() {
                let rest = l.get(1..).unwrap_or_default();
                assert!(!rest.contains("[w"), "{case}: two lines mixed: {l}");
            }
        }
    }
}

/// T8: `RAYON_NUM_THREADS` in a worker's environment sizes its big-stack
/// pool (F18: inherited, never set). 3, not the spec's 2, so the break
/// (`available_parallelism`) differs on a 2-core runner too.
#[test]
fn rayon_threads_size_the_big_stack_pool() {
    let ws = ws_a();
    let analyze = vec!["analyze", ws.as_str(), "--format", "sarif"];
    let d = tempfile::tempdir().unwrap();
    let batches = vec![vec![analyze.clone()], vec![analyze]];
    let (out, _) = run_batches(d.path(), &batches, &[("RAYON_NUM_THREADS", "3")], None);
    assert_eq!(out.status.code(), Some(0));
    for b in 0..2 {
        let own = worker_trace(d.path(), b);
        let pools = instants(&own, "pool.threads");
        assert!(!pools.is_empty(), "worker {b} built a big-stack pool");
        for p in pools {
            assert_eq!(p["args"]["threads"], 3, "worker {b}");
        }
    }
}

/// T10: two `digest --diff -` jobs split over two batches (and a third batch
/// that reads no stdin) each get the parent's one stdin read, equal to their
/// standalone run on the same stdin.
#[test]
fn batch_stdin_read_once_across_workers() {
    let ws = ws_a();
    let d = tempfile::tempdir().unwrap();
    let diff = std::fs::read(write_diff(
        d.path(),
        "src/D1MultiCaller.al",
        18,
        "        Cust.Modify();",
    ))
    .unwrap();
    let args = vec!["digest", ws.as_str(), "--diff", "-", "--deterministic"];
    let alone = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .args(&args)
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
    assert!(!alone.stdout.is_empty(), "precondition: a digest document");
    let analyze = vec!["analyze", ws.as_str(), "--format", "sarif"];
    let batches = vec![vec![args.clone()], vec![analyze], vec![args]];
    let (out, _) = run_batches(d.path(), &batches, &[], Some(&diff));
    assert_eq!(out.status.code(), Some(0));
    for b in [0, 2] {
        let got = std::fs::read(d.path().join(format!("{b}.0.stdout"))).unwrap();
        assert_eq!(
            String::from_utf8_lossy(&got),
            String::from_utf8_lossy(&alone.stdout),
            "batch {b}"
        );
    }
}

/// Review Focus 4: a worker that dies without its lines leaves its jobs
/// `"exit": null` with its status, and the run exits 1; the other worker's
/// lines stand. Debug-only: the kill is a debug seam (#277 R10-2).
#[cfg(debug_assertions)]
#[test]
fn batch_worker_killed() {
    let ws = ws_a();
    let analyze = vec!["analyze", ws.as_str(), "--format", "sarif"];
    let d = tempfile::tempdir().unwrap();
    let batches = vec![vec![analyze.clone()], vec![analyze.clone(), analyze]];
    let (out, _) = run_batches(d.path(), &batches, &[("ALSEM_TEST_WORKER_DIES", "1")], None);
    let got = lines(&out);
    assert_eq!(got.len(), 3, "one line per job");
    assert!(got[0]["exit"].is_u64(), "batch 0 ran: {}", got[0]);
    for line in &got[1..] {
        assert_eq!(line["batch"], 1);
        assert!(line["exit"].is_null(), "a job of the dead worker: {line}");
        let status = line["worker"].as_str().expect("the worker's status");
        if cfg!(unix) {
            assert!(status.contains("signal: 9"), "{status}");
        }
    }
    assert_eq!(out.status.code(), Some(1), "the run lost a worker's lines");
}

/// The debug seam that makes `alsem` take its stdout for a terminal.
#[cfg(debug_assertions)]
const TTY_SEAM: (&str, &str) = ("ALSEM_TEST_STDOUT_IS_TERMINAL", "1");

/// #277 final r1 FR-1 (A12.3): in a batch, analyze's `--format auto` (its
/// default) is JSON whatever the process's stdout is. Under the debug seam
/// standalone analyze prints terminal text (precondition); one batch and two
/// batches each write the JSON standalone prints off a terminal.
#[cfg(debug_assertions)]
#[test]
fn batch_auto_format_is_json_on_a_terminal() {
    let ws = ws_a();
    let args = vec!["analyze", ws.as_str(), "--deterministic"];
    let alone = |env: &[(&str, &str)]| {
        Command::new(env!("CARGO_BIN_EXE_alsem"))
            .args(&args)
            .env("RUST_LOG", "off")
            .envs(env.iter().copied())
            .output()
            .unwrap()
    };
    let (json_out, tty) = (alone(&[]), alone(&[TTY_SEAM]));
    serde_json::from_slice::<Value>(&json_out.stdout).expect("precondition: JSON off a terminal");
    assert_ne!(
        json_out.stdout, tty.stdout,
        "precondition: the seam is a terminal"
    );
    for n in [1, 2] {
        let d = tempfile::tempdir().unwrap();
        let batches = vec![vec![args.clone()]; n];
        let (out, _) = run_batches(d.path(), &batches, &[TTY_SEAM], None);
        assert_eq!(out.status.code(), Some(0), "{n} batches");
        for b in 0..n {
            let got = std::fs::read(d.path().join(format!("{b}.0.stdout"))).unwrap();
            assert_eq!(
                String::from_utf8_lossy(&got),
                String::from_utf8_lossy(&json_out.stdout),
                "{n} batches: batch {b}"
            );
        }
    }
}

/// `alsem batch` of `doc` (written to `dir/m.json`) with `stdin`, not waited.
fn spawn_manifest(dir: &Path, doc: &Value, stdin: Stdio) -> std::process::Child {
    let manifest = dir.join("m.json");
    std::fs::write(&manifest, doc.to_string()).unwrap();
    Command::new(env!("CARGO_BIN_EXE_alsem"))
        .arg("batch")
        .arg(&manifest)
        .env("RUST_LOG", "off")
        .env_remove("ALSEM_TRACE")
        .stdin(stdin)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap()
}

/// `{"batches": …}` of one job per batch.
fn batches_doc(batches: &[Vec<&str>]) -> Value {
    let batches: Vec<Value> = batches
        .iter()
        .map(|a| json!({ "jobs": [job(a, None, None)] }))
        .collect();
    json!({ "batches": batches })
}

/// #277 final r1 FR-7 (D3): stdin is read only for a job that reads it after
/// its own validation and `--changed` override, as standalone. A digest with
/// `--diff -` overridden by an existing `--changed` diff file, and one whose
/// `--format` fails, read none: two batches finish with stdin still open, each
/// job equal to its standalone run.
#[test]
fn batch_unread_stdin_does_not_block() {
    let ws = ws_a();
    let d = tempfile::tempdir().unwrap();
    let diff = write_diff(
        d.path(),
        "src/D1MultiCaller.al",
        18,
        "        Cust.Modify();",
    );
    let jobs = [
        vec![
            "digest",
            ws.as_str(),
            "--diff",
            "-",
            "--changed",
            &diff,
            "--deterministic",
        ],
        vec!["digest", ws.as_str(), "--diff", "-", "--format", "bad"],
    ];
    let mut child = spawn_manifest(d.path(), &batches_doc(&jobs), Stdio::piped());
    let open = child.stdin.take();
    let start = std::time::Instant::now();
    while child.try_wait().unwrap().is_none() {
        if start.elapsed() > std::time::Duration::from_secs(120) {
            let _ = child.kill();
            panic!("the run waits for stdin no job reads");
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    drop(open);
    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    let got = lines(&out);
    for (b, a) in jobs.iter().enumerate() {
        let alone = Command::new(env!("CARGO_BIN_EXE_alsem"))
            .args(a)
            .env("RUST_LOG", "off")
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert_eq!(got[b]["exit"].as_i64(), alone.status.code().map(i64::from));
        let read = |ext: &str| std::fs::read(d.path().join(format!("{b}.0.{ext}"))).unwrap();
        assert_eq!(read("stdout"), alone.stdout, "batch {b}: stdout");
        assert_eq!(read("stderr"), alone.stderr, "batch {b}: stderr");
    }
}

/// #277 final r1 FR-8 (D3): a stdin the parent cannot read (a directory) is
/// each `--diff -` job's own standalone error, exit 3, in every worker.
#[cfg(unix)]
#[test]
fn batch_stdin_read_error_is_each_jobs() {
    let ws = ws_a();
    let d = tempfile::tempdir().unwrap();
    let args = vec!["digest", ws.as_str(), "--diff", "-", "--deterministic"];
    let dir_stdin = || Stdio::from(std::fs::File::open(d.path()).unwrap());
    let alone = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .args(&args)
        .env("RUST_LOG", "off")
        .stdin(dir_stdin())
        .output()
        .unwrap();
    assert_eq!(
        alone.status.code(),
        Some(3),
        "precondition: standalone fails"
    );
    assert_eq!(
        String::from_utf8_lossy(&alone.stderr),
        "al-sem: digest: failed to read diff from stdin\n"
    );
    let out = spawn_manifest(d.path(), &batches_doc(&[args.clone(), args]), dir_stdin())
        .wait_with_output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let got = lines(&out);
    assert_eq!(got.len(), 2, "one line per job");
    for (b, line) in got.iter().enumerate() {
        assert_eq!(line["exit"], 3, "batch {b}");
        let err = std::fs::read(d.path().join(format!("{b}.0.stderr"))).unwrap();
        assert_eq!(err, alone.stderr, "batch {b}");
    }
}

/// #277 final r1 FR-10 (D12): a one-batch run, a worker in-process, prints
/// its `[batch] worker 0 peak_kib=` line under the census.
#[cfg(target_os = "linux")]
#[test]
fn one_batch_reports_its_peak() {
    let ws = ws_a();
    let d = tempfile::tempdir().unwrap();
    let batches = vec![vec![vec![
        "events",
        "fanout",
        ws.as_str(),
        "--format",
        "bad",
    ]]];
    let (out, _) = run_batches(d.path(), &batches, &[("ALSEM_RETAINED_CENSUS", "1")], None);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    assert!(
        stderr
            .lines()
            .any(|l| l.starts_with("[batch] worker 0 peak_kib=")),
        "{stderr}"
    );
}

/// #277 final r1 FR-14 (D12): admission counts only jobs that will build a
/// view. Four batches of jobs that fail their own validation, one of them a
/// `fingerprint --format bad` (whose kind would be the 8 GiB placeholder):
/// `W` is 4, not 3.
#[test]
fn admission_skips_failed_jobs() {
    let ws = ws_a();
    let d = tempfile::tempdir().unwrap();
    let bad = vec!["events", "fanout", ws.as_str(), "--format", "bad"];
    let mut batches = vec![vec![bad.clone()]; 4];
    batches[0].push(vec!["fingerprint", ws.as_str(), "--format", "bad"]);
    let (out, events) = run_batches(d.path(), &batches, &[], None);
    assert_eq!(out.status.code(), Some(0));
    let workers = instants(&events, "batch.workers");
    assert_eq!(workers[0]["args"]["w"], 4);
}

/// #277 final r2 R2-4 (D12): a bad `--policy` file fails the job in planning,
/// so it builds no view and counts for no admission row. Four batches of
/// `policy check --policy bad.yaml`: `W` is 4 (no policy cap), each job
/// equal to its standalone run.
#[test]
fn admission_skips_bad_policy_jobs() {
    let ws = ws_a();
    let d = tempfile::tempdir().unwrap();
    let bad = d.path().join("bad.yaml");
    std::fs::write(&bad, "rules: [").unwrap();
    let job = vec![
        "policy",
        "check",
        ws.as_str(),
        "--policy",
        bad.to_str().unwrap(),
    ];
    let (out, events) = run_batches(d.path(), &vec![vec![job.clone()]; 4], &[], None);
    assert_eq!(out.status.code(), Some(0));
    let workers = instants(&events, "batch.workers");
    assert_eq!(workers[0]["args"]["w"], 4);
    let alone = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .args(&job)
        .env("RUST_LOG", "off")
        .output()
        .unwrap();
    assert_eq!(alone.status.code(), Some(1), "precondition: a bad policy");
    let got = lines(&out);
    assert_eq!(got.len(), 4);
    for (b, line) in got.iter().enumerate() {
        assert_eq!(line["exit"], 1, "batch {b}");
        let read = |ext: &str| std::fs::read(d.path().join(format!("{b}.0.{ext}"))).unwrap();
        assert_eq!(read("stdout"), alone.stdout, "batch {b}: stdout");
        assert_eq!(
            String::from_utf8_lossy(&read("stderr")),
            String::from_utf8_lossy(&alone.stderr),
            "batch {b}: stderr"
        );
    }
}

/// #277 final r2 R2-1 (D2.5, D3): a file a plan reads is read once per run,
/// by the parent, and its bytes go to the worker. A `digest --diff` of a FIFO
/// with one writer, in the first of two batches, finishes (a second read
/// would wait for a writer that never comes) and equals its standalone run on
/// a regular file of the same bytes.
#[cfg(unix)]
#[test]
fn batch_diff_fifo_is_read_once() {
    let ws = ws_a();
    let d = tempfile::tempdir().unwrap();
    let regular = write_diff(
        d.path(),
        "src/D1MultiCaller.al",
        18,
        "        Cust.Modify();",
    );
    let bytes = std::fs::read(&regular).unwrap();
    let fifo = d.path().join("fifo.diff");
    let made = Command::new("mkfifo").arg(&fifo).status().unwrap();
    assert!(made.success(), "precondition: mkfifo");
    let writer = {
        let fifo = fifo.clone();
        std::thread::spawn(move || {
            // One writer: opened once, closed after the bytes.
            let mut f = std::fs::OpenOptions::new().write(true).open(fifo).unwrap();
            f.write_all(&bytes).unwrap();
        })
    };
    let fifo_arg = fifo.to_string_lossy().into_owned();
    let jobs = [
        vec![
            "digest",
            ws.as_str(),
            "--diff",
            &fifo_arg,
            "--deterministic",
        ],
        vec!["events", "fanout", ws.as_str(), "--single-app"],
    ];
    let mut child = spawn_manifest(d.path(), &batches_doc(&jobs), Stdio::null());
    let start = std::time::Instant::now();
    while child.try_wait().unwrap().is_none() {
        if start.elapsed() > std::time::Duration::from_secs(120) {
            let _ = child.kill();
            panic!("the run reads the FIFO twice");
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    writer.join().unwrap();
    let out = child.wait_with_output().unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let alone = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .args(["digest", ws.as_str(), "--diff", &regular, "--deterministic"])
        .env("RUST_LOG", "off")
        .output()
        .unwrap();
    assert_eq!(
        alone.status.code(),
        Some(0),
        "precondition: the diff digests"
    );
    assert_eq!(lines(&out)[0]["exit"], 0);
    let got = std::fs::read(d.path().join("0.0.stdout")).unwrap();
    assert_eq!(
        String::from_utf8_lossy(&got),
        String::from_utf8_lossy(&alone.stdout)
    );
}

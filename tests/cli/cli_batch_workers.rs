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
/// lines stand.
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

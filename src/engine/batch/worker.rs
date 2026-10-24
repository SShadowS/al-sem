//! #277 D2.5: a run of two or more batches runs each batch in a child
//! process (`alsem batch --worker <b> <manifest>`), at most `W` at once
//! (D12), started in manifest order. Each child gets its batch on its stdin
//! as a [`WorkerInput`] (its jobs, the parent's `--changed` decisions and
//! its one stdin read, D3) and never opens the manifest (#277 final r1 FR-3).
//! The parent forwards each child's stderr whole line by whole line with a
//! `[w<b>] ` prefix and prints the job lines in manifest order after the last
//! child exits. A child that dies without its lines leaves its jobs `"exit":
//! null` with the child's status (D2.2).

use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;

use serde::{Deserialize, Serialize};

use super::output::write_stderr;
use super::{JobLine, JobStreams};
use crate::engine::perf_trace as pt;

/// What a child runs, as the parent validated it: no child reads the
/// manifest or decides anything from the file system the parent decided.
#[derive(Serialize, Deserialize)]
pub struct WorkerInput {
    pub jobs: Vec<WorkerJob>,
    /// The parent's one stdin read; `None` when no job read stdin.
    pub stdin: Option<StdinRead>,
}

/// One job of a child's batch.
#[derive(Serialize, Deserialize)]
pub struct WorkerJob {
    pub argv: Vec<String>,
    pub streams: JobStreams,
    /// `digest --changed`: whether the parent took the value for an existing
    /// diff file (`None`: the job has no `--changed`).
    pub changed_is_file: Option<bool>,
}

/// The parent's stdin read: its text, or the read failed.
#[derive(Clone, Serialize, Deserialize)]
pub enum StdinRead {
    Text(String),
    Failed,
}

/// One batch as the parent sees it: its jobs' streams (for the lines of a
/// worker that died) and the [`WorkerInput`] it writes to the child's stdin.
pub struct WorkerBatch {
    pub streams: Vec<JobStreams>,
    pub input: Vec<u8>,
}

/// The run: the manifest every child is named (it never opens it) and `W`.
pub struct Workers<'a> {
    pub manifest: &'a Path,
    pub w: usize,
}

/// A child's stdout and how it ended (its status, or why it never ran).
type Ended = (Vec<u8>, String);

/// Run every batch in its own child. Returns the run's job lines, in
/// manifest order, and whether every worker printed all of its lines.
/// A worker that exits non-zero but printed all its lines counts as whole.
pub fn run_workers(run: &Workers<'_>, batches: &[WorkerBatch]) -> (Vec<u8>, bool) {
    let mut ended: Vec<Option<Ended>> = batches.iter().map(|_| None).collect();
    std::thread::scope(|s| {
        let (tx, rx) = mpsc::channel::<(usize, Ended)>();
        let mut reap = |(b, end): (usize, Ended)| {
            pt::instant_lazy(
                "batch",
                "batch.worker.exit",
                || serde_json::json!({ "b": b, "status": end.1 }),
            );
            ended[b] = Some(end);
        };
        let mut alive = 0;
        for (b, batch) in batches.iter().enumerate() {
            if alive == run.w {
                reap(rx.recv().expect("a running worker reports"));
                alive -= 1;
            }
            pt::instant_lazy(
                "batch",
                "batch.worker.start",
                || serde_json::json!({ "b": b }),
            );
            let input = batch.input.as_slice();
            let tx = tx.clone();
            match spawn(run, b) {
                Ok(child) => {
                    s.spawn(move || {
                        let end = supervise(s, b, child, input);
                        let _ = tx.send((b, end));
                    });
                }
                Err(e) => {
                    let _ = tx.send((b, (Vec::new(), format!("not started: {e}"))));
                }
            }
            alive += 1;
        }
        drop(tx);
        rx.into_iter().for_each(reap);
    });
    let mut out = Vec::new();
    let mut whole = true;
    for (b, (batch, end)) in batches.iter().zip(ended).enumerate() {
        let (stdout, status) = end.expect("every worker ended");
        let printed: Vec<&[u8]> = stdout
            .split_inclusive(|c| *c == b'\n')
            .filter(|l| l.ends_with(b"\n"))
            .take(batch.streams.len())
            .collect();
        printed.iter().for_each(|l| out.extend_from_slice(l));
        for (i, streams) in batch.streams.iter().enumerate().skip(printed.len()) {
            whole = false;
            let line = JobLine {
                batch: u32::try_from(b).expect("at most u32::MAX batches"),
                job: u32::try_from(i).expect("a batch of at most u32::MAX jobs"),
                exit: None,
                stdout: streams.stdout.clone(),
                stderr: streams.stderr.clone(),
                files: vec![],
                worker: Some(status.clone()),
            };
            out.extend(serde_json::to_vec(&line).expect("a job line serializes"));
            out.push(b'\n');
        }
    }
    (out, whole)
}

/// Start batch `b`'s child: the same binary, its batch on its stdin; its
/// trace file is `<ALSEM_TRACE_FILE>.w<b>` when the parent's is named (D10).
fn spawn(run: &Workers<'_>, b: usize) -> std::io::Result<Child> {
    let mut cmd = Command::new(std::env::current_exe()?);
    cmd.args(["batch", "--worker", &b.to_string()])
        .arg(run.manifest)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(mut file) = std::env::var_os("ALSEM_TRACE_FILE") {
        file.push(format!(".w{b}"));
        cmd.env("ALSEM_TRACE_FILE", file);
    }
    cmd.spawn()
}

/// Feed the child its input, forward its stderr, read its stdout, wait.
fn supervise<'s>(
    s: &'s std::thread::Scope<'s, '_>,
    b: usize,
    mut child: Child,
    input: &'s [u8],
) -> Ended {
    let mut pipe = child.stdin.take().expect("a piped stdin");
    // Its own thread: a child that writes before it reads cannot stall it.
    s.spawn(move || {
        let _ = pipe.write_all(input);
    });
    let err = child.stderr.take().expect("a piped stderr");
    let forward = s.spawn(move || forward(b, err));
    let mut stdout = Vec::new();
    let _ = child
        .stdout
        .take()
        .expect("a piped stdout")
        .read_to_end(&mut stdout);
    let _ = forward.join();
    let status = child
        .wait()
        .map_or_else(|e| e.to_string(), |s| s.to_string());
    (stdout, status)
}

/// Child `b`'s stderr on ours, one whole line per write, each `[w<b>] `.
fn forward(b: usize, err: impl Read) {
    let mut lines = BufReader::new(err);
    let mut line = Vec::new();
    loop {
        line.clear();
        line.extend_from_slice(format!("[w{b}] ").as_bytes());
        match lines.read_until(b'\n', &mut line) {
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }
        if !line.ends_with(b"\n") {
            line.push(b'\n');
        }
        let _ = write_stderr(&line);
    }
}

/// Under `ALSEM_RETAINED_CENSUS=1`, worker `b`'s peak (`VmHWM`) on its
/// stderr, so a bench can sum the workers' peaks (D12). Omitted where
/// `/proc/self/status` does not exist.
pub fn report_peak(b: u32) {
    if !crate::engine::census::enabled() {
        return;
    }
    let hwm = std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            let kb = s.lines().find_map(|l| l.strip_prefix("VmHWM:"))?;
            kb.trim().strip_suffix("kB")?.trim().parse::<u64>().ok()
        });
    if let Some(kib) = hwm {
        let _ = write_stderr(format!("[batch] worker {b} peak_kib={kib}\n").as_bytes());
    }
}

/// Test seam, compiled into debug builds only (#277 R10-1):
/// `ALSEM_TEST_WORKER_DIES=<b>` kills worker `b` after its jobs ran, before
/// it prints its lines (`batch_worker_killed`).
#[cfg(debug_assertions)]
pub fn test_seam_die(b: u32) {
    if std::env::var("ALSEM_TEST_WORKER_DIES").ok() == Some(b.to_string()) {
        let pid = std::process::id().to_string();
        let _ = Command::new("kill").args(["-KILL", &pid]).status();
        // Where there is no `kill`: die without the lines all the same.
        std::process::exit(9);
    }
}

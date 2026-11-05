//! The engine pool behind every context-server front end (design r7 §8, §9):
//! one worker thread per (workspace, universe), each owning that pair's
//! engine child through an [`EngineFront`], and the call path that turns a
//! validated tool call into one bounded answer frame.
//!
//! Two front ends drive it: `alsem mcp` ([`super::serve`]) and the editor's
//! LSP server (`crate::lsp::engine`, the `al-call-hierarchy/…` custom
//! requests). The pool knows nothing of either wire format: a [`Transport`]
//! wraps an envelope into the front end's response, writes a frame, reports
//! progress, and says what a cancelled request gets (MCP: nothing; LSP: a
//! `RequestCancelled` error). Everything else, and so every envelope byte,
//! is the same for both: the children bound (`--max-children`), the per
//! engine queue depth (`busy`), cancellation (`session/cancel` for the
//! running call), `session/refresh`, the output folder, and frame bounding.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Instant;

use serde_json::{Value, json};

use super::output::{OutputSink, SinkError};
use super::tools::{self, CommandCall, EngineCall, Universe};
use super::{Decide, WORKER_QUEUE_DEPTH, codes};
use crate::engine::batch::output::SUMMARY_CAP_BYTES;
use crate::engine::host::AskParams;
use crate::engine::host::front::{EngineFront, FrontConfig};
use crate::wire::{
    Answer, FailedOutcome, MAX_FRAME_BYTES, Outcome, SpillFile, SpillKind, Stderr, Stdout,
    bound_frame, encode_frame,
};

/// What a front end supplies for its wire format.
pub trait Transport: Send + Sync {
    /// The whole response message to request `id` whose result is `body`
    /// (an envelope, a refresh body or a status). `output`: the CLI's JSON
    /// parsed, when the stdout is inline JSON (MCP's `structuredContent`).
    fn response(&self, id: &Value, body: &Value, is_error: bool, output: Option<Value>) -> Value;
    /// Write one encoded frame (a response, or an error response).
    fn send(&self, frame: &[u8]);
    /// Request `id`, cancelled by the client, has ended: dropped while
    /// queued (`frame` is `None`), or answered after the cancel reached
    /// the engine (`frame` is the response that would have been sent).
    fn cancelled(&self, id: &Value, frame: Option<&[u8]>);
    /// A progress report for a call that asked for one with `token`:
    /// `done` 0 when the engine is asked, 1 when it answered.
    fn progress(&self, token: &Value, done: u32, message: &str);
}

/// The engine children's configuration (design §7).
#[derive(Clone, Debug)]
pub struct ChildConfig {
    /// Each child's `--memory-budget`: (cross-app, single-app).
    pub budgets: (u64, u64),
    /// Start each child with `--low-mem`.
    pub low_mem: bool,
    /// Each child's `--idle-timeout` in seconds (0: never).
    pub idle_timeout_secs: u64,
    /// The most children alive at once.
    pub max_children: usize,
}

/// How a pool starts its children.
pub struct PoolConfig {
    /// The `alsem` binary the engine children run.
    pub program: PathBuf,
    /// Extra environment for the engine children.
    pub child_env: Vec<(std::ffi::OsString, std::ffi::OsString)>,
    pub children: ChildConfig,
    /// The front end's private output folder (design §10).
    pub sink: OutputSink,
    /// The front end's answers to a command line's file-system questions.
    pub decide: Arc<dyn Decide>,
}

/// A call the pool queues on its engine's worker.
pub enum Request {
    /// A command tool: the CLI command line it became.
    Command {
        call: CommandCall,
        /// The client's progress token, when it asked for progress.
        progress: Option<Value>,
    },
    /// `session/refresh {wait}` (after leaving a failed state).
    Refresh(EngineCall),
}

/// The engines' key.
type Key = (String, Universe);

/// Why a call was not queued (the children bound, a full queue).
struct Refused {
    universe: Universe,
    message: String,
}

/// One part of a composite request ([`Pool::run_composite`]).
#[derive(Debug)]
pub enum SubOutcome {
    /// The part's response message, as the transport wrapped it.
    Answered(Value),
    /// Why the part was not computed (its engine could not be admitted, or
    /// it ended unanswered).
    NotComputed(String),
}

/// What a status shows of one engine, published by its worker.
#[derive(Default)]
struct Snapshot {
    started: bool,
    hello: Option<crate::engine::host::Hello>,
    child: Option<crate::engine::host::ChildStatus>,
    spawns: u64,
    deaths: u64,
    retired: u64,
    completed: u64,
    last_failure: Option<(FailedOutcome, String)>,
    stderr_tail: String,
    backoff_ms: u128,
    running_tool: Option<String>,
    /// What the running call is doing (MCP output plan 3c): `asking` until
    /// the child reports a phase, then the child's `pin` (freshness check
    /// and, when the inputs changed, the build), `prepare`, `compute`; or
    /// `refresh`. `None` when idle.
    activity: Option<(&'static str, Instant)>,
    /// When `child` (the child's `session/status`) was last read.
    child_read_at: Option<Instant>,
    /// The published revision's input descriptor (agent-tasks design §1.2
    /// M0-6), read once per generation after a call; immutable, so a reader
    /// holds its `Arc` without asking the child.
    /// Keyed by `<engineInstanceId>/<generation>`.
    descriptor: Option<(String, Arc<Value>)>,
}

/// A running command's bookkeeping: when it was queued, its cancel flag and
/// the worker's published state (its `activity`).
struct Running<'a> {
    queued: Instant,
    cancel: &'a AtomicBool,
    snapshot: &'a Mutex<Snapshot>,
}

/// The child's phase name as [`Snapshot::activity`] keeps it.
fn activity_name(phase: &str) -> &'static str {
    match phase {
        "pin" => "pin",
        "prepare" => "prepare",
        "compute" => "compute",
        _ => "asking",
    }
}

/// A process's resident set and its peak, in bytes, read now from
/// `/proc/<pid>/status` (Linux); `None` elsewhere or when it is gone.
fn live_rss(pid: u32) -> Option<(u64, u64)> {
    if !cfg!(target_os = "linux") {
        return None;
    }
    let s = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    let kib = |key: &str| -> Option<u64> {
        s.lines()
            .find_map(|l| l.strip_prefix(key))
            .and_then(|v| v.split_whitespace().next())
            .and_then(|n| n.parse().ok())
    };
    Some((kib("VmRSS:")? * 1024, kib("VmHWM:")? * 1024))
}

/// `alsem_status`'s freshness block for one engine (MCP output plan 3c, S2):
/// the revision that answers (`generation`, `inputsStamp`), the last build
/// (class, start, duration), whether a build may be running now, and the
/// child's memory (read live from the OS where it can be, else the child's
/// own last report). `now` is passed in so a test can state it.
fn freshness(s: &Snapshot, now: Instant) -> Value {
    let child = s.child.as_ref();
    let session = child.and_then(|c| c.session.as_ref());
    let revision = child.and_then(|c| c.revision.as_ref()).map(|r| {
        json!({
            "generation": r.generation,
            "inputsStamp": r.inputs_stamp,
            "engineInstanceId": r.engine_instance_id,
        })
    });
    let last_build = session.filter(|s| s["lastBuildMs"].is_u64()).map(|s| {
        json!({
            "class": s["lastBuildClass"],
            "startedAtUnixMs": s["lastBuildStartedUnixMs"],
            "durationMs": s["lastBuildMs"],
            "builds": s["builds"],
            "discardedBuilds": s["discardedBuilds"],
        })
    });
    let (activity, since) = match s.activity {
        Some((a, at)) => (
            a,
            Some(now.saturating_duration_since(at).as_millis() as u64),
        ),
        None => ("idle", None),
    };
    let pid = s.hello.as_ref().map(|h| h.pid);
    let live = pid.and_then(live_rss);
    let reported = child
        .and_then(|c| c.memory.as_ref())
        .and_then(|m| m["rssBytes"].as_u64());
    // Freshness checks that found the revision current, and the last check's
    // time: an ask or refresh with nothing changed builds nothing.
    let checks = session.map(|s| {
        json!({
            "freshHits": s["freshHits"],
            "lastCheckUs": s["lastCheckUs"],
        })
    });
    json!({
        "revision": revision,
        "lastBuild": last_build,
        "checks": checks,
        "activity": activity,
        "activityMs": since,
        // `null`: an unwaited refresh started a build the child runs on its
        // own; the next call (or a waited refresh) settles it.
        "building": match activity {
            "pin" | "refresh" => json!(true),
            "refresh-started" => Value::Null,
            _ => json!(false),
        },
        "rssBytes": live.map(|l| l.0).or(reported),
        "peakRssBytes": live.map(|l| l.1),
        "rssSource": if live.is_some() { "live" } else if reported.is_some() { "lastStatus" } else { "none" },
        "countersAgeMs": s.child_read_at.map(|at| now.saturating_duration_since(at).as_millis() as u64),
    })
}

/// A queued job.
enum Job {
    Command {
        id: Value,
        call: CommandCall,
        progress: Option<Value>,
        queued: Instant,
        /// Set by the client's cancellation.
        cancel: Arc<AtomicBool>,
        /// A composite's part: the answer frame goes here, not to the
        /// transport (an empty frame: dropped while queued).
        reply: Option<Sender<Vec<u8>>>,
    },
    Refresh {
        id: Value,
        call: EngineCall,
        cancel: Arc<AtomicBool>,
    },
    /// End the engine child (the `--max-children` bound): the worker stays,
    /// and its next job starts a new child.
    Retire,
    /// `session/notifyChanged` for these paths, to a running child only
    /// (a child started later checks the disk anyway).
    Notify(Vec<PathBuf>),
}

/// One engine's worker.
struct Worker {
    tx: Sender<Job>,
    pending: Arc<AtomicUsize>,
    snapshot: Arc<Mutex<Snapshot>>,
    thread: std::thread::JoinHandle<()>,
    /// When a call was last handed to it (least recently used goes first).
    last_used: Instant,
}

/// What every worker shares.
struct Shared {
    transport: Arc<dyn Transport>,
    sink: OutputSink,
    decide: Arc<dyn Decide>,
    program: PathBuf,
    child_env: Vec<(std::ffi::OsString, std::ffi::OsString)>,
    in_flight: Mutex<HashSet<String>>,
    /// The cancel flag of every queued or running engine call, by id.
    cancels: Mutex<HashMap<String, Arc<AtomicBool>>>,
    closing: AtomicBool,
    children: ChildConfig,
}

fn log(line: &str) {
    use std::io::Write;
    let _ = writeln!(std::io::stderr(), "al-sem: engine pool: {line}");
}

fn error(id: &Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

/// An id as the in-flight set holds it (its JSON, so `1` and `"1"` differ).
fn id_key(id: &Value) -> String {
    id.to_string()
}

impl Shared {
    /// A finished request: its id leaves the in-flight set, then its frame
    /// goes out.
    fn finish(&self, id: &Value, frame: &[u8]) {
        self.forget(id);
        self.transport.send(frame);
    }

    /// A request that leaves the pool.
    fn forget(&self, id: &Value) {
        self.in_flight
            .lock()
            .expect("in flight")
            .remove(&id_key(id));
        self.cancels.lock().expect("cancels").remove(&id_key(id));
    }

    /// A request that ends cancelled: it leaves the pool, and the
    /// transport says what the client gets.
    fn end_cancelled(&self, id: &Value, frame: Option<&[u8]>) {
        self.forget(id);
        self.transport.cancelled(id, frame);
    }

    fn send_value(&self, msg: &Value) {
        match encode_frame(msg) {
            Ok(f) => self.transport.send(&f),
            Err(e) => log(&format!("a response does not encode: {e}")),
        }
    }

    fn finish_value(&self, id: &Value, msg: &Value) {
        match encode_frame(msg) {
            Ok(f) => self.finish(id, &f),
            Err(e) => {
                log(&format!("a response does not encode: {e}"));
                self.forget(id);
            }
        }
    }
}

/// The engine children of one front end and their workers.
pub struct Pool {
    shared: Arc<Shared>,
    workers: Mutex<HashMap<Key, Worker>>,
}

impl Pool {
    /// A pool for `cfg` answering through `transport`; nothing is started
    /// until the first call.
    #[must_use]
    pub fn new(cfg: PoolConfig, transport: Arc<dyn Transport>) -> Pool {
        let children = ChildConfig {
            max_children: cfg.children.max_children.max(1),
            ..cfg.children
        };
        Pool {
            shared: Arc::new(Shared {
                transport,
                sink: cfg.sink,
                decide: cfg.decide,
                program: cfg.program,
                child_env: cfg.child_env,
                in_flight: Mutex::new(HashSet::new()),
                cancels: Mutex::new(HashMap::new()),
                closing: AtomicBool::new(false),
                children,
            }),
            workers: Mutex::new(HashMap::new()),
        }
    }

    fn workers(&self) -> MutexGuard<'_, HashMap<Key, Worker>> {
        self.workers.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// The output folder.
    #[must_use]
    pub fn sink(&self) -> &OutputSink {
        &self.shared.sink
    }

    /// The children's configuration.
    #[must_use]
    pub fn children(&self) -> &ChildConfig {
        &self.shared.children
    }

    /// The `alsem` binary the children run.
    #[must_use]
    pub fn program(&self) -> &Path {
        &self.shared.program
    }

    /// Whether a call with this id is queued or running.
    #[must_use]
    pub fn in_flight(&self, id: &Value) -> bool {
        self.shared
            .in_flight
            .lock()
            .expect("in flight")
            .contains(&id_key(id))
    }

    /// Cancel call `id`: a queued call is dropped, a running one is sent
    /// `session/cancel`; either way the transport's
    /// [`Transport::cancelled`] ends it. `false`: no such call.
    pub fn cancel(&self, id: &Value) -> bool {
        match self
            .shared
            .cancels
            .lock()
            .expect("cancels")
            .get(&id_key(id))
        {
            Some(flag) => {
                flag.store(true, Ordering::SeqCst);
                true
            }
            None => false,
        }
    }

    /// Tell every running child the paths `map` gives for its workspace
    /// (`session/notifyChanged`); a workspace `map` gives nothing is not
    /// told. Starts nothing.
    pub fn notify_changed(&self, map: &dyn Fn(&Path) -> Vec<PathBuf>) {
        for (key, w) in self.workers().iter() {
            let paths = map(Path::new(&key.0));
            if !paths.is_empty() {
                let _ = w.tx.send(Job::Notify(paths));
            }
        }
    }

    /// Hand `request` (call `id`) to its engine's worker, or answer it here
    /// (`busy`, a stopped worker).
    pub fn submit(&self, id: Value, request: Request) {
        if let Err(Refused { universe, message }) = self.enqueue(id.clone(), request, None) {
            let envelope = json!({
                "kind": "failed",
                "revision": null,
                "outcome": FailedOutcome::Busy.as_str(),
                "message": message,
                "universe": universe.as_str(),
            });
            let shared = &self.shared;
            shared.send_value(&shared.transport.response(&id, &envelope, true, None));
        }
    }

    /// A request that needs several engine children (agent-tasks design
    /// §1.2 M0-6: impact with tests, tests with the production root
    /// resolution, the impact base), run as a sequence of plain
    /// sub-requests, each part's outcome in order. The caller runs it off
    /// its reader loop (it blocks until the last part ends).
    ///
    /// **A part never holds one child while waiting for another child's
    /// admission**: a part is admitted only after the previous one has been
    /// answered, so its child is idle (released) by then; admission never
    /// waits. A part that cannot be admitted (every child busy, at the
    /// `--max-children` bound) is [`SubOutcome::NotComputed`] with the
    /// reason, and the remaining parts still run. A part admitted to a busy
    /// engine waits in that engine's own queue, holding no other.
    pub fn run_composite(&self, id: &Value, parts: Vec<CommandCall>) -> Vec<SubOutcome> {
        let mut out = Vec::with_capacity(parts.len());
        for (i, call) in parts.into_iter().enumerate() {
            let universe = call.universe;
            let (tx, rx) = channel::<Vec<u8>>();
            let sub_id = json!(format!("{}#{i}", id_key(id)));
            let request = Request::Command {
                call,
                progress: None,
            };
            if let Err(r) = self.enqueue(sub_id, request, Some(tx)) {
                out.push(SubOutcome::NotComputed(format!(
                    "the {} engine could not start: {}",
                    universe.as_str(),
                    r.message
                )));
                continue;
            }
            out.push(match rx.recv() {
                Ok(frame) if frame.is_empty() => {
                    SubOutcome::NotComputed("cancelled while queued".into())
                }
                Ok(frame) => match serde_json::from_slice(&frame) {
                    Ok(v) => SubOutcome::Answered(v),
                    Err(e) => SubOutcome::NotComputed(format!("the answer does not decode: {e}")),
                },
                Err(_) => SubOutcome::NotComputed("the engine's worker has stopped".into()),
            });
        }
        out
    }

    /// Queue `request` on its engine's worker, after the children bound;
    /// with `reply`, the answer frame goes there instead of the transport.
    fn enqueue(
        &self,
        id: Value,
        request: Request,
        reply: Option<Sender<Vec<u8>>>,
    ) -> Result<(), Refused> {
        let shared = &self.shared;
        let cancel = Arc::new(AtomicBool::new(false));
        let (key, job) = match request {
            Request::Refresh(c) => {
                let key = (c.workspace.clone().unwrap_or_default(), c.universe);
                (
                    key,
                    Job::Refresh {
                        id: id.clone(),
                        call: c,
                        cancel: Arc::clone(&cancel),
                    },
                )
            }
            Request::Command { call, progress } => (
                (call.workspace.clone(), call.universe),
                Job::Command {
                    id: id.clone(),
                    call,
                    progress,
                    queued: Instant::now(),
                    cancel: Arc::clone(&cancel),
                    reply,
                },
            ),
        };
        let mut workers = self.workers();
        // The children bound (design §7 `--max-children`): a call for an
        // engine with no child while the bound is reached ends the least
        // recently used idle child; when every child is busy it is `busy`.
        // A worker with calls queued is about to have a child, if it has none.
        let running = |w: &Worker| {
            w.pending.load(Ordering::SeqCst) > 0
                || w.snapshot.lock().expect("snapshot").hello.is_some()
        };
        let has_child = workers.get(&key).is_some_and(running);
        if !has_child {
            let live: Vec<&Key> = workers
                .iter()
                .filter(|(k, w)| **k != key && running(w))
                .map(|(k, _)| k)
                .collect();
            if live.len() >= shared.children.max_children {
                let idle = live
                    .iter()
                    .filter(|k| workers[**k].pending.load(Ordering::SeqCst) == 0)
                    .min_by_key(|k| workers[**k].last_used)
                    .map(|k| (*k).clone());
                match idle {
                    Some(k) => {
                        let w = workers.get(&k).expect("a live worker");
                        // Marked gone at once: the worker publishes on retiring.
                        w.snapshot.lock().expect("snapshot").hello = None;
                        let _ = w.tx.send(Job::Retire);
                    }
                    None => {
                        return Err(Refused {
                            universe: key.1,
                            message: format!(
                                "{} engine children are running (--max-children) and none is idle \
                                 (max-children {})",
                                shared.children.max_children, shared.children.max_children
                            ),
                        });
                    }
                }
            }
        }
        let worker = workers
            .entry(key.clone())
            .or_insert_with(|| spawn_worker(shared, &key));
        worker.last_used = Instant::now();
        if worker.pending.load(Ordering::SeqCst) >= WORKER_QUEUE_DEPTH {
            return Err(Refused {
                universe: key.1,
                message: format!("{WORKER_QUEUE_DEPTH} requests already queued for this engine"),
            });
        }
        shared
            .in_flight
            .lock()
            .expect("in flight")
            .insert(id_key(&id));
        shared
            .cancels
            .lock()
            .expect("cancels")
            .insert(id_key(&id), cancel);
        worker.pending.fetch_add(1, Ordering::SeqCst);
        if let Err(std::sync::mpsc::SendError(job)) = worker.tx.send(job) {
            worker.pending.fetch_sub(1, Ordering::SeqCst);
            if matches!(job, Job::Command { reply: Some(_), .. }) {
                // A composite's part: its coordinator hears it from the
                // dropped reply channel.
                shared.forget(&id);
            } else {
                shared.finish_value(
                    &id,
                    &error(
                        &id,
                        codes::INTERNAL_ERROR,
                        "the engine's worker has stopped",
                    ),
                );
            }
        }
        Ok(())
    }

    /// The input descriptor (agent-tasks design §1.2 M0-6) the worker of
    /// `(workspace, universe)` published for its child's revision, if any:
    /// read from the worker's snapshot, never through its busy front end.
    #[must_use]
    pub fn descriptor(&self, workspace: &str, universe: Universe) -> Option<Arc<Value>> {
        let workers = self.workers();
        let w = workers.get(&(workspace.to_string(), universe))?;
        let s = w.snapshot.lock().expect("snapshot");
        s.descriptor.as_ref().map(|(_, d)| Arc::clone(d))
    }

    /// Every engine (or the one `call` names), from the workers' published
    /// snapshots; starts nothing.
    #[must_use]
    pub fn engines(&self, call: &EngineCall) -> Vec<Value> {
        let workers = self.workers();
        let mut engines: Vec<Value> = workers
            .iter()
            .filter(|(k, _)| {
                call.workspace
                    .as_ref()
                    .is_none_or(|w| *w == k.0 && call.universe == k.1)
            })
            .map(|(k, w)| {
                let s = w.snapshot.lock().expect("snapshot");
                json!({
                    "workspace": k.0,
                    "universe": k.1.as_str(),
                    "queued": w.pending.load(Ordering::SeqCst),
                    "running": s.running_tool,
                    "started": s.started,
                    "freshness": freshness(&s, Instant::now()),
                    "engine": s.hello.as_ref().map(|h| json!({
                        "pid": h.pid,
                        "engineInstanceId": h.engine_instance_id,
                        "alsemVersion": h.alsem_version,
                        "protocol": h.protocol,
                        "grammarSha": h.grammar_sha,
                    })),
                    "child": s.child.as_ref().map(|c| json!({
                        "asks": c.asks,
                        "workspaceReads": c.workspace_reads,
                        "revision": c.revision,
                        "memory": c.memory,
                        "idleTimeoutSecs": c.idle_timeout_secs,
                        "lowMem": c.low_mem,
                    })),
                    "spawns": s.spawns,
                    "deaths": s.deaths,
                    "retired": s.retired,
                    "completed": s.completed,
                    "backoffMs": s.backoff_ms,
                    "lastFailure": s.last_failure.as_ref().map(|(o, m)| json!({"outcome": o.as_str(), "message": m})),
                    "stderrTail": s.stderr_tail,
                })
            })
            .collect();
        engines.sort_by(|a, b| {
            (a["workspace"].as_str(), a["universe"].as_str())
                .cmp(&(b["workspace"].as_str(), b["universe"].as_str()))
        });
        engines
    }

    /// End the pool: queued calls are dropped unanswered; with
    /// `cancel_running` each running call is cancelled first (else it
    /// finishes and is answered); then every engine child is shut down.
    pub fn shutdown(&self, cancel_running: bool) {
        self.shared.closing.store(true, Ordering::SeqCst);
        if cancel_running {
            for flag in self.shared.cancels.lock().expect("cancels").values() {
                flag.store(true, Ordering::SeqCst);
            }
        }
        let drained: Vec<Worker> = self.workers().drain().map(|(_, w)| w).collect();
        for w in drained {
            drop(w.tx);
            let _ = w.thread.join();
        }
    }
}

fn spawn_worker(shared: &Arc<Shared>, key: &Key) -> Worker {
    let (tx, rx) = channel::<Job>();
    let pending = Arc::new(AtomicUsize::new(0));
    let snapshot = Arc::new(Mutex::new(Snapshot::default()));
    let thread = {
        let shared = Arc::clone(shared);
        let pending = Arc::clone(&pending);
        let snapshot = Arc::clone(&snapshot);
        let key = key.clone();
        std::thread::Builder::new()
            .name(format!("alsem-engine-{}", key.1.as_str()))
            .spawn(move || work(&shared, &key, &rx, &pending, &snapshot))
            .expect("spawn a worker thread")
    };
    Worker {
        tx,
        pending,
        snapshot,
        thread,
        last_used: Instant::now(),
    }
}

/// A worker: one engine front end, its jobs one at a time.
fn work(
    shared: &Shared,
    key: &Key,
    rx: &Receiver<Job>,
    pending: &AtomicUsize,
    snapshot: &Mutex<Snapshot>,
) {
    let mut cfg = FrontConfig::new(&shared.program, &key.0);
    cfg.env.clone_from(&shared.child_env);
    // The child enforces its universe (an ask of the other is refused).
    cfg.universe = match key.1 {
        Universe::CrossApp => crate::engine::host::Universe::CrossApp,
        Universe::SingleApp => crate::engine::host::Universe::SingleApp,
    };
    // Its memory budget (its share of --total-budget) and idle timeout.
    let budget = match key.1 {
        Universe::CrossApp => shared.children.budgets.0,
        Universe::SingleApp => shared.children.budgets.1,
    };
    cfg.args.extend([
        "--memory-budget".into(),
        budget.to_string().into(),
        "--idle-timeout".into(),
        shared.children.idle_timeout_secs.to_string().into(),
    ]);
    if shared.children.low_mem {
        cfg.args.push("--low-mem".into());
    }
    let mut front = match EngineFront::new(cfg) {
        Ok(f) => Some(f),
        Err(e) => {
            log(&format!("engine front end for '{}': {e}", key.0));
            None
        }
    };
    while let Ok(job) = rx.recv() {
        if let Job::Retire = job {
            if let Some(front) = front.as_mut() {
                front.shutdown();
                publish(front, snapshot, false);
            }
            continue;
        }
        if let Job::Notify(paths) = job {
            if let (false, Some(front)) = (shared.closing.load(Ordering::SeqCst), front.as_mut())
                && let Err(e) = front.notify_changed_if_running(paths)
            {
                log(&format!("session/notifyChanged for '{}': {e}", key.0));
            }
            continue;
        }
        if shared.closing.load(Ordering::SeqCst) {
            pending.fetch_sub(1, Ordering::SeqCst);
            continue;
        }
        match job {
            Job::Retire | Job::Notify(_) => unreachable!("handled above"),
            // Cancelled while queued: dropped.
            Job::Command {
                id,
                cancel,
                reply: Some(reply),
                ..
            } if cancel.load(Ordering::SeqCst) => {
                pending.fetch_sub(1, Ordering::SeqCst);
                shared.forget(&id);
                let _ = reply.send(Vec::new());
            }
            Job::Command { id, cancel, .. } | Job::Refresh { id, cancel, .. }
                if cancel.load(Ordering::SeqCst) =>
            {
                pending.fetch_sub(1, Ordering::SeqCst);
                shared.end_cancelled(&id, None);
            }
            Job::Command {
                id,
                call,
                progress,
                queued,
                cancel,
                reply,
            } => {
                {
                    let mut s = snapshot.lock().expect("snapshot");
                    s.running_tool = Some(call.tool.to_string());
                    s.activity = Some(("asking", Instant::now()));
                }
                let frame = match front.as_mut() {
                    Some(front) => run_command(
                        shared,
                        front,
                        &id,
                        &call,
                        progress.as_ref(),
                        Running {
                            queued,
                            cancel: &cancel,
                            snapshot,
                        },
                    ),
                    None => encode_or_log(&shared.transport.response(
                        &id,
                        &json!({
                            "kind": "failed", "revision": null,
                            "outcome": FailedOutcome::EngineError.as_str(),
                            "message": "the engine front end could not be created",
                            "universe": call.universe.as_str(),
                        }),
                        true,
                        None,
                    )),
                };
                if let Some(front) = front.as_mut() {
                    publish(front, snapshot, true);
                } else {
                    let mut s = snapshot.lock().expect("snapshot");
                    s.running_tool = None;
                    s.activity = None;
                }
                pending.fetch_sub(1, Ordering::SeqCst);
                if let Some(reply) = reply {
                    // A composite's part: answered to its coordinator, the
                    // worker idle (its child released) before it reads it.
                    shared.forget(&id);
                    let _ = reply.send(frame);
                } else if cancel.load(Ordering::SeqCst) {
                    // Cancelled while it ran: the child stopped it.
                    shared.end_cancelled(&id, Some(&frame));
                } else {
                    shared.finish(&id, &frame);
                }
            }
            Job::Refresh { id, call, cancel } => {
                // `session/refresh {wait}` (design §8.2): a failed front end
                // is reset first, then the child brings its revision up to
                // date with the disk (with `wait`, before answering).
                {
                    let mut s = snapshot.lock().expect("snapshot");
                    s.running_tool = Some("alsem_refresh".to_string());
                    s.activity = Some(("refresh", Instant::now()));
                }
                let (outcome, is_error) = match front.as_mut() {
                    Some(front) => {
                        front.reset();
                        let r = front.refresh(call.wait);
                        // After a waited refresh the child is idle, so its
                        // counters (the new revision, the build's timing) are
                        // read for `alsem_status`; an unwaited one is still
                        // building, and its status would wait for the build.
                        publish(front, snapshot, call.wait);
                        if !call.wait && r.is_ok() {
                            // The child builds after answering: until the next
                            // call, nobody knows whether it still is.
                            snapshot.lock().expect("snapshot").activity =
                                Some(("refresh-started", Instant::now()));
                        }
                        match r {
                            Ok(v) if v.get("error").is_none() => (v, false),
                            Ok(v) => (v, true),
                            Err(e) => (json!({"error": e}), true),
                        }
                    }
                    None => {
                        let mut s = snapshot.lock().expect("snapshot");
                        s.running_tool = None;
                        s.activity = None;
                        drop(s);
                        (
                            json!({"error": "the engine front end could not be created"}),
                            true,
                        )
                    }
                };
                let body = json!({
                    "kind": "refresh",
                    "workspace": call.workspace,
                    "universe": call.universe.as_str(),
                    "waited": call.wait,
                    "engine": outcome,
                });
                pending.fetch_sub(1, Ordering::SeqCst);
                let msg = shared.transport.response(&id, &body, is_error, None);
                if cancel.load(Ordering::SeqCst) {
                    shared.end_cancelled(&id, Some(&encode_or_log(&msg)));
                } else {
                    shared.finish_value(&id, &msg);
                }
            }
        }
    }
    if let Some(mut front) = front {
        front.shutdown();
    }
}

fn encode_or_log(msg: &Value) -> Vec<u8> {
    encode_frame(msg).unwrap_or_else(|e| {
        log(&format!("a response does not encode: {e}"));
        Vec::new()
    })
}

/// Publish the front end's state for a status; `ask_status` asks a running
/// child its `session/status` (cheap) after a command.
fn publish(front: &mut EngineFront, snapshot: &Mutex<Snapshot>, ask_status: bool) {
    let child = if ask_status && front.hello().is_some() && front.child_exited() == Some(false) {
        front.status().ok()
    } else {
        None
    };
    // A revision this worker holds no descriptor of (a new generation, or a
    // new child): its descriptor, asked once.
    let published = child
        .as_ref()
        .and_then(|c| c.revision.as_ref())
        .map(|r| format!("{}/{}", r.engine_instance_id, r.generation));
    let held = snapshot
        .lock()
        .expect("snapshot")
        .descriptor
        .as_ref()
        .map(|(k, _)| k.clone());
    let descriptor = match published {
        Some(k) if held.as_ref() != Some(&k) => front
            .descriptor()
            .ok()
            .filter(|d| !d.is_null())
            .map(|d| (k, Arc::new(d))),
        _ => None,
    };
    let stats = front.stats();
    let tail = front.stderr_tail();
    let keep = tail.len().saturating_sub(4096);
    let mut start = keep;
    while !tail.is_char_boundary(start) {
        start += 1;
    }
    let mut s = snapshot.lock().expect("snapshot");
    s.started = true;
    s.hello = front.hello().cloned();
    if child.is_some() || s.hello.is_none() {
        s.child_read_at = child.as_ref().map(|_| Instant::now());
        s.child = child;
    }
    if descriptor.is_some() {
        s.descriptor = descriptor;
    }
    s.activity = None;
    s.spawns = stats.spawns;
    s.deaths = stats.deaths;
    s.retired = stats.retired;
    s.completed = stats.completed;
    s.last_failure = front.last_failure();
    s.stderr_tail = tail[start..].to_string();
    s.backoff_ms = front.current_backoff().as_millis();
    s.running_tool = None;
}

/// Bytes of a file the engine spilled, checked against its length and
/// hash, the file then removed.
fn take_spill(f: &SpillFile) -> Result<Vec<u8>, String> {
    let bytes = std::fs::read(&f.path).map_err(|e| format!("{}: {e}", f.path))?;
    if bytes.len() as u64 != f.bytes || crate::engine::ids::sha256_bytes_hex(&bytes) != f.sha256 {
        return Err(format!(
            "{}: length or hash differs from the answer",
            f.path
        ));
    }
    let _ = std::fs::remove_file(&f.path);
    Ok(bytes)
}

/// The value of `--<flag>=` in `argv`.
fn flag_value<'a>(argv: &'a [String], flag: &str) -> Option<&'a str> {
    let prefix = format!("--{flag}=");
    argv.iter()
        .take_while(|a| *a != "--")
        .find_map(|a| a.strip_prefix(&prefix))
}

/// A renderer's summary as the envelope carries it. The renderer keeps its
/// summary under [`SUMMARY_CAP_BYTES`] itself (degrading to counts, MCP output
/// plan 1b); one over it anyway (a renderer bug) is replaced by its size,
/// never spilled and never left to grow the frame.
fn bounded_summary(summary: Value) -> Value {
    let bytes = summary.to_string().len();
    if bytes <= SUMMARY_CAP_BYTES {
        summary
    } else {
        json!({"omitted": "over the summary cap", "bytes": bytes})
    }
}

/// The characters an inline stdout takes in what the client reads: the
/// parsed JSON re-serialized as `output` when there is one, else the text
/// escaped as `stdoutText`; in UTF-16 units, as a JavaScript client counts.
fn client_chars(text: &str, parsed: Option<&Value>) -> usize {
    let s = match parsed {
        Some(v) => v.to_string(),
        None => serde_json::to_string(text).unwrap_or_default(),
    };
    s.encode_utf16().count()
}

/// The extension of a file holding `argv`'s stdout.
fn extension(argv: &[String]) -> &'static str {
    match flag_value(argv, "format") {
        Some("json") => "json",
        _ => "txt",
    }
}

/// An infrastructure outcome answered instead of the CLI's.
type Refusal = (FailedOutcome, String);

/// Move a stdout the call must deliver as a file (an `out`, a digest, or a
/// stdout the engine spilled) into the output folder; the bytes come back
/// when they were materialized. `Err`: the outcome to answer instead
/// (`refusedPath` or `engineError`).
fn place_stdout(
    shared: &Shared,
    call: &CommandCall,
    stdout: Stdout,
) -> Result<(Stdout, Option<Vec<u8>>), Refusal> {
    let wants_file = call.out.is_some() || call.always_file;
    let was_file = matches!(stdout, Stdout::File(_));
    if !wants_file && !was_file {
        return Ok((stdout, None));
    }
    let bytes = match stdout {
        Stdout::Text(t) => t.into_bytes(),
        Stdout::File(f) => take_spill(&f).map_err(|e| (FailedOutcome::EngineError, e))?,
    };
    let refused = |e: SinkError| match e {
        SinkError::Refused(m) => (FailedOutcome::RefusedPath, m),
        SinkError::Io(m) => (
            FailedOutcome::RefusedPath,
            format!("writing the output: {m}"),
        ),
    };
    // Off Unix the sink writes nothing: a digest stays inline when it can.
    if !shared.sink.writable() && call.out.is_none() && !was_file {
        let text = String::from_utf8(bytes.clone()).expect("an inline stdout is UTF-8");
        return Ok((Stdout::Text(text), Some(bytes)));
    }
    let path = match &call.out {
        Some(out) => shared.sink.write_out(out, &bytes).map_err(refused)?,
        None => {
            let stem = call.tool.trim_start_matches("alsem_");
            shared
                .sink
                .write_new(stem, extension(&call.argv), &bytes)
                .map_err(refused)?
        }
    };
    let file = SpillFile::of(path.to_string_lossy().into_owned(), &bytes);
    Ok((Stdout::File(file), Some(bytes)))
}

/// A stderr or diagnostic file the engine spilled, moved into the output
/// folder.
fn place_spill(shared: &Shared, f: SpillFile, ext: &str) -> Result<SpillFile, String> {
    let bytes = take_spill(&f)?;
    let path = shared
        .sink
        .write_new("engine", ext, &bytes)
        .map_err(|e| e.to_string())?;
    Ok(SpillFile::of(path.to_string_lossy().into_owned(), &bytes))
}

/// One command call through the engine, answered as one bounded frame.
fn run_command(
    shared: &Shared,
    front: &mut EngineFront,
    id: &Value,
    call: &CommandCall,
    progress: Option<&Value>,
    job: Running<'_>,
) -> Vec<u8> {
    let Running {
        queued,
        cancel,
        snapshot,
    } = job;
    let queued_ms = queued.elapsed().as_millis();
    let notify = |p: u32, message: &str| {
        if let Some(token) = progress {
            shared.transport.progress(token, p, message);
        }
    };
    notify(0, &format!("{}: asking the engine", call.tool));
    let decisions = shared.decide.decide(&call.argv, call.diff_text.as_deref());
    let started = Instant::now();
    // A client cancellation becomes `session/cancel` for this ask; each phase
    // the child reports is what `alsem_status` says the engine is doing
    // (MCP output plan 3c).
    let mut on_phase = |p: &Value| {
        if let Some(phase) = p.get("phase").and_then(Value::as_str) {
            snapshot.lock().expect("snapshot").activity =
                Some((activity_name(phase), Instant::now()));
        }
    };
    let mut watch = crate::engine::host::front::Watch {
        cancel: Some(cancel),
        progress: Some(&mut on_phase),
    };
    let params = AskParams {
        argv: call.argv.clone(),
        diff_text: call.diff_text.clone(),
        decisions,
        max_inline_bytes: call.max_inline_bytes,
        freshness: match call.freshness {
            tools::Freshness::Fresh => crate::engine::resident::session::Freshness::Fresh,
            tools::Freshness::LatestBuilt => {
                crate::engine::resident::session::Freshness::LatestBuilt
            }
            tools::Freshness::Verify => crate::engine::resident::session::Freshness::Verify,
        },
        progress: true,
        ..AskParams::default()
    };
    let answer = front.ask_with(params, &mut watch);
    let compute_ms = started.elapsed().as_millis();
    let revision = answer.revision.clone();
    let meta = answer.meta.clone();
    let mut parsed = None;
    let outcome = match answer.outcome {
        Outcome::Cli {
            exit_code,
            stdout,
            stderr,
            changed_paths,
            changed_paths_truncated,
            summary,
        } => {
            let placed = place_stdout(shared, call, stdout).and_then(|(stdout, bytes)| {
                let stderr = match stderr {
                    Stderr::File(f) => Stderr::File(
                        place_spill(shared, f, "stderr")
                            .map_err(|e| (FailedOutcome::RefusedPath, e))?,
                    ),
                    lines => lines,
                };
                Ok((stdout, stderr, bytes))
            });
            match placed {
                Ok((stdout, stderr, _bytes)) => {
                    let json_format = flag_value(&call.argv, "format") == Some("json");
                    if let (Stdout::Text(t), true) = (&stdout, json_format) {
                        parsed = serde_json::from_str::<Value>(t).ok();
                    }
                    // Item T, T-W2-23: an inline answer the client would file
                    // anyway (its escaped form over the client's budget) goes
                    // to a file now, with its summary.
                    let stdout = match (stdout, call.client_budget_chars) {
                        (Stdout::Text(t), Some(budget))
                            if client_chars(&t, parsed.as_ref())
                                + tools::MCP_ENVELOPE_ROOM_CHARS
                                > budget
                                && shared.sink.writable() =>
                        {
                            let stem = call.tool.trim_start_matches("alsem_");
                            match shared
                                .sink
                                .write_new(stem, extension(&call.argv), t.as_bytes())
                            {
                                Ok(path) => {
                                    parsed = None;
                                    Stdout::File(SpillFile::of(
                                        path.to_string_lossy().into_owned(),
                                        t.as_bytes(),
                                    ))
                                }
                                Err(e) => {
                                    log(&format!(
                                        "an answer over the client budget stays inline: {e}"
                                    ));
                                    Stdout::Text(t)
                                }
                            }
                        }
                        (s, _) => s,
                    };
                    // The renderer's summary (MCP output plan 1b): built from its
                    // own data before the frame is bounded, so `bound_frame`
                    // counts it; kept when the stdout is a file, the only case
                    // an agent needs it.
                    let summary = summary
                        .filter(|_| matches!(stdout, Stdout::File(_)))
                        .map(bounded_summary);
                    Outcome::Cli {
                        exit_code,
                        stdout,
                        stderr,
                        changed_paths,
                        changed_paths_truncated,
                        summary,
                    }
                }
                Err((o, m)) => Outcome::failed(o, m),
            }
        }
        Outcome::Failed {
            outcome,
            message,
            diagnostic_file,
        } => Outcome::Failed {
            outcome,
            message,
            diagnostic_file: diagnostic_file
                .and_then(|f| place_spill(shared, f, "diagnostic").ok()),
        },
    };
    notify(1, &format!("{}: answered", call.tool));
    // The engine's timing, plus the front end's own queue time.
    let engine_timing = meta.timing.unwrap_or_default();
    let served = if meta.stale { "latest-built" } else { "fresh" };
    let answer = Answer {
        id: id.clone(),
        outcome,
        revision,
        meta,
    };
    let extras = json!({
        "universe": call.universe.as_str(),
        "freshness": {"requested": call.freshness.as_str(), "served": served},
        "timing": {
            "queuedMs": queued_ms as u64 + engine_timing.queued_ms,
            "waitedForBuildMs": engine_timing.waited_for_build_ms,
            "sharedFillMs": engine_timing.shared_fill_ms,
            "computeMs": compute_ms,
        },
    });
    let wrap = |a: &Answer| {
        let mut envelope = serde_json::to_value(a).unwrap_or(Value::Null);
        let is_error = envelope["kind"] == "failed";
        if let Some(m) = envelope.as_object_mut() {
            m.remove("id");
            if !is_error {
                m.entry("stale").or_insert(json!(false));
                m.entry("changedPaths").or_insert(json!([]));
                m.entry("changedPathsTruncated").or_insert(json!(false));
                // Every line the answer prints or serializes is 1-based (MCP
                // output plan 1a).
                m.insert(
                    "lineBase".into(),
                    json!(crate::engine::line_base::LINE_BASE),
                );
            }
            if let Some(e) = extras.as_object() {
                for (k, v) in e {
                    m.insert(k.clone(), v.clone());
                }
            }
        }
        let output = if is_error { None } else { parsed.clone() };
        shared.transport.response(id, &envelope, is_error, output)
    };
    let spill = |kind: SpillKind, bytes: &[u8]| -> std::io::Result<SpillFile> {
        let ext = match kind {
            SpillKind::Stdout => "stdout",
            SpillKind::Stderr => "stderr",
            SpillKind::Diagnostic => "diagnostic",
        };
        let path = shared
            .sink
            .write_new("answer", ext, bytes)
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        Ok(SpillFile::of(path.to_string_lossy().into_owned(), bytes))
    };
    bound_frame(answer, MAX_FRAME_BYTES, &wrap, &spill)
}

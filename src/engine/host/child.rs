//! The child side of the engine protocol: [`serve`] reads requests from the
//! front end on a reader thread, answers control requests (`status`,
//! `cancel`, `notifyChanged`) at once, and hands asks to ONE worker that
//! answers them in arrival order, each as one bounded frame. What an ask
//! computes is the [`Engine`]'s (the `alsem` binary's resident session);
//! this module owns framing, the queue, cancellation and deadlines,
//! bounding, and the private output directory spills go to.

use std::collections::VecDeque;
use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, mpsc};
use std::time::Instant;

use super::output::OutputDir;
use super::{
    AskParams, CancelParams, ChildStatus, Hello, MAX_ID_BYTES, Message, NotifyChangedParams,
    OpenParams, PROGRESS_METHOD, QUEUE_HEAVY, QUEUE_LIGHT, QUEUED_BYTES, RefreshParams, codes,
};
use crate::engine::resident::store::Stop;
use crate::wire::{
    Answer, FailedOutcome, MAX_FRAME_BYTES, Outcome, ReadFrame, Spill, WorkDone, bound_frame,
    decode_frame, encode_frame, read_frame,
};

/// What one ask may read of its request's state while it runs.
pub struct AskCx<'a> {
    /// Writes bytes too large to inline into the private output directory.
    pub spill: &'a Spill<'a>,
    /// `Some` once the ask is cancelled or its deadline passed: read between
    /// phases.
    pub stop: &'a (dyn Fn() -> Option<Stop> + Sync),
    /// Reports a phase (`pin`, `prepare`, `compute`) when the ask asked for
    /// progress; a no-op otherwise.
    pub progress: &'a (dyn Fn(&str) + Sync),
    /// How long the ask waited in the queue.
    pub queued_ms: u64,
}

/// What a child computes per request. Shared by the reader and the worker,
/// so every method takes `&self`.
pub trait Engine: Sync {
    /// `session/open`: the workspace this session serves; `instance` is the
    /// engine instance id of the hello.
    fn open(&self, params: &OpenParams, instance: &str) -> Result<(), String>;
    /// `session/ask`: one command's answer (the serve loop bounds it).
    fn ask(&self, id: &serde_json::Value, params: AskParams, cx: &AskCx<'_>) -> Answer;
    /// `session/status`.
    fn status(&self) -> ChildStatus;
    /// `session/notifyChanged`.
    fn notify_changed(&self, _paths: &[PathBuf]) {}
    /// `session/refresh`: the published revision after a `fresh` pin.
    fn refresh(&self, _cx: &AskCx<'_>) -> serde_json::Value {
        serde_json::json!({ "revision": null })
    }
}

/// What the reader thread hands the dispatcher.
enum Incoming {
    Frame(Vec<u8>),
    Oversized,
    Eof,
    Failed(String),
}

/// One queued unit of work for the worker.
enum Work {
    Ask {
        id: serde_json::Value,
        params: Box<AskParams>,
        received: Instant,
        deadline: Option<Instant>,
        cancel: Arc<AtomicBool>,
        light: bool,
        bytes: usize,
    },
    Refresh {
        id: serde_json::Value,
        wait: bool,
        received: Instant,
    },
}

#[derive(Default)]
struct QueueState {
    /// The heavy lane: every ask but query, and refreshes, in arrival order.
    heavy_items: VecDeque<Work>,
    /// The light lane: queries, in arrival order.
    light_items: VecDeque<Work>,
    heavy: usize,
    light: usize,
    bytes: usize,
    closing: bool,
    /// The asks in flight: each id and cancel flag.
    in_flight: Vec<(serde_json::Value, Arc<AtomicBool>)>,
    /// When the last unit of work finished (the idle clock).
    last_done: Option<Instant>,
}

impl QueueState {
    fn idle(&self) -> bool {
        self.heavy_items.is_empty() && self.light_items.is_empty() && self.in_flight.is_empty()
    }
}

/// How a child serves (design §7): its idle timeout (`None`: never) and how
/// many light workers answer queries beside the heavy worker.
#[derive(Clone, Copy, Debug)]
pub struct ServeConfig {
    pub idle_timeout: Option<std::time::Duration>,
    pub idle_grace: std::time::Duration,
    pub light_workers: usize,
}

impl Default for ServeConfig {
    fn default() -> Self {
        ServeConfig {
            idle_timeout: None,
            idle_grace: super::IDLE_GRACE,
            light_workers: super::LIGHT_WORKERS,
        }
    }
}

struct Shared<'o, W: Write> {
    out: Mutex<&'o mut W>,
    queue: Mutex<QueueState>,
    ready: Condvar,
    dir: Mutex<Option<Arc<OutputDir>>>,
    instance: String,
    #[cfg(any(feature = "engine-test-hooks", debug_assertions))]
    hook: Option<super::test_hooks::Hook>,
}

impl<W: Write> Shared<'_, W> {
    fn send(&self, msg: &Message) -> std::io::Result<()> {
        let frame = encode_frame(msg).map_err(|e| std::io::Error::other(e.to_string()))?;
        self.write(&frame)
    }

    fn write(&self, frame: &[u8]) -> std::io::Result<()> {
        let mut out = self.out.lock().unwrap_or_else(|p| p.into_inner());
        out.write_all(frame)?;
        out.flush()
    }

    /// `answer` as one bounded frame, spilled into the output directory.
    fn answer(&self, answer: Answer) -> std::io::Result<()> {
        let dir = self.dir.lock().unwrap_or_else(|p| p.into_inner()).clone();
        let spill = |kind, bytes: &[u8]| match &dir {
            Some(d) => d.spill(kind, bytes),
            None => Err(std::io::Error::other("no output directory")),
        };
        let wrap = |a: &Answer| serde_json::json!({"jsonrpc": "2.0", "id": a.id, "result": a});
        let frame = bound_frame(answer, MAX_FRAME_BYTES, &wrap, &spill);
        self.write(&frame)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, QueueState> {
        self.queue.lock().unwrap_or_else(|p| p.into_inner())
    }
}

/// A failed answer of a request that did no work.
fn no_work(id: serde_json::Value, outcome: FailedOutcome, message: &str) -> Answer {
    let mut a = Answer::new(id, Outcome::failed(outcome, message), None);
    if matches!(outcome, FailedOutcome::Cancelled | FailedOutcome::Deadline) {
        a.meta.work = Some(WorkDone {
            work_done_ms: 0,
            skipped_phases: vec!["pin".into(), "prepare".into(), "compute".into()],
        });
    }
    a
}

/// Whether `argv` is a light ask (design §3: query and status are light).
fn is_light(argv: &[String]) -> bool {
    argv.first().is_some_and(|c| c == "query")
}

/// Serve the protocol until the front end shuts the session down or closes
/// the input. The hello goes first. A frame over the limit is answered with
/// a parse error and the rest of its line, if any is left, discarded
/// without holding it ([`crate::wire::Oversized::skip_rest`]); a frame that
/// is not JSON, with a parse error; an id over [`MAX_ID_BYTES`], with an
/// invalid-request error. At end of input, or on `session/shutdown`, every
/// queued ask is answered first; the shutdown is answered last.
pub fn serve(
    engine: &dyn Engine,
    input: impl BufRead + Send + 'static,
    output: impl Write + Send,
) -> std::io::Result<()> {
    serve_with(engine, input, output, ServeConfig::default())
}

/// [`serve`] with an idle timeout and a light-worker count (design §4.5,
/// §7): a light ask (query) is answered by one of `light_workers` workers
/// beside the heavy one; once no request has arrived and no work has run
/// for the idle timeout, the child sends [`super::IDLE_EXIT_METHOD`] and
/// ends after the grace period unless a request arrives first.
pub fn serve_with(
    engine: &dyn Engine,
    mut input: impl BufRead + Send + 'static,
    mut output: impl Write + Send,
    config: ServeConfig,
) -> std::io::Result<()> {
    #[cfg(any(feature = "engine-test-hooks", debug_assertions))]
    let hook = super::test_hooks::from_env();
    #[cfg_attr(
        not(any(feature = "engine-test-hooks", debug_assertions)),
        allow(unused_mut)
    )]
    let mut hello = Hello::of_this_process();
    #[cfg(any(feature = "engine-test-hooks", debug_assertions))]
    match &hook {
        Some(super::test_hooks::Hook::NoHello) => {
            // Wait for the front end to give up and close our input.
            let _ = std::io::copy(&mut input, &mut std::io::sink());
            return Ok(());
        }
        Some(super::test_hooks::Hook::Protocol(p)) => hello.protocol = *p,
        _ => {}
    }
    let frame = encode_frame(&Message::notification(
        "engine/hello",
        serde_json::to_value(&hello)?,
    ))
    .map_err(|e| std::io::Error::other(e.to_string()))?;
    output.write_all(&frame)?;
    output.flush()?;
    // The reader: detached, so a shutdown never waits for a blocked read.
    let (tx, rx) = mpsc::channel::<Incoming>();
    std::thread::spawn(move || {
        loop {
            let next = match read_frame(&mut input) {
                Ok(ReadFrame::Frame(f)) => Incoming::Frame(f),
                Ok(ReadFrame::Eof) => Incoming::Eof,
                // Only what is left of THIS line: when its newline was the
                // last byte read, the next byte already starts a request.
                Ok(ReadFrame::Oversized(over)) => match over.skip_rest(&mut input) {
                    Ok(_) => Incoming::Oversized,
                    Err(e) => Incoming::Failed(e.to_string()),
                },
                Err(e) => Incoming::Failed(e.to_string()),
            };
            let end = matches!(next, Incoming::Eof | Incoming::Failed(_));
            if tx.send(next).is_err() || end {
                return;
            }
        }
    });
    let shared = Shared {
        out: Mutex::new(&mut output),
        queue: Mutex::new(QueueState::default()),
        ready: Condvar::new(),
        dir: Mutex::new(None),
        instance: hello.engine_instance_id.clone(),
        #[cfg(any(feature = "engine-test-hooks", debug_assertions))]
        hook,
    };
    std::thread::scope(|s| {
        let mut workers = vec![s.spawn(|| work(engine, &shared, false))];
        for _ in 0..config.light_workers {
            workers.push(s.spawn(|| work(engine, &shared, true)));
        }
        let ended = dispatch(engine, &rx, &shared, &config);
        shared.lock().closing = true;
        shared.ready.notify_all();
        let mut worked = Ok(());
        for w in workers {
            let r = w
                .join()
                .unwrap_or_else(|_| Err(std::io::Error::other("worker panicked")));
            if worked.is_ok() {
                worked = r;
            }
        }
        let ended = ended?;
        worked?;
        if let Some(id) = ended {
            shared.send(&Message::response(id, serde_json::json!({"ok": true})))?;
        }
        Ok(())
    })
}

/// The dispatcher: every frame in order. Returns the shutdown's id, or
/// `None` at end of input.
fn dispatch<W: Write + Send>(
    engine: &dyn Engine,
    rx: &mpsc::Receiver<Incoming>,
    shared: &Shared<'_, W>,
    config: &ServeConfig,
) -> std::io::Result<Option<serde_json::Value>> {
    let mut open = false;
    let mut last_frame = Instant::now();
    // Set once the idle notice went out: the grace period runs from it.
    let mut exiting: Option<Instant> = None;
    loop {
        let next = match config.idle_timeout {
            None => rx.recv().map_err(|_| mpsc::RecvTimeoutError::Disconnected),
            Some(idle) => {
                let slice = exiting.map_or(idle, |_| config.idle_grace);
                rx.recv_timeout(slice.min(std::time::Duration::from_millis(250)))
            }
        };
        let next = match next {
            Ok(i) => {
                last_frame = Instant::now();
                exiting = None;
                Ok(i)
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                let idle = config.idle_timeout.expect("only an idle clock times out");
                let (quiet, since) = {
                    let q = shared.lock();
                    let since = q.last_done.map_or(last_frame, |d| d.max(last_frame));
                    (q.idle(), since)
                };
                match exiting {
                    Some(at) if quiet && at.elapsed() >= config.idle_grace => return Ok(None),
                    None if quiet && since.elapsed() >= idle => {
                        shared.send(&Message::notification(
                            super::IDLE_EXIT_METHOD,
                            serde_json::json!({"idleSecs": since.elapsed().as_secs()}),
                        ))?;
                        exiting = Some(Instant::now());
                    }
                    _ => {}
                }
                continue;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => Err(()),
        };
        let frame = match next {
            Ok(Incoming::Frame(f)) => f,
            Ok(Incoming::Oversized) => {
                shared.send(&Message::error(
                    serde_json::Value::Null,
                    codes::PARSE_ERROR,
                    format!("frame exceeds {MAX_FRAME_BYTES} bytes"),
                ))?;
                continue;
            }
            Ok(Incoming::Eof) | Err(_) => return Ok(None),
            Ok(Incoming::Failed(e)) => return Err(std::io::Error::other(e)),
        };
        let msg: Message = match decode_frame(&frame) {
            Ok(m) => m,
            Err(e) => {
                shared.send(&Message::error(
                    serde_json::Value::Null,
                    codes::PARSE_ERROR,
                    e.to_string(),
                ))?;
                continue;
            }
        };
        let notification = msg.id.is_none();
        let id = msg.id.clone().unwrap_or(serde_json::Value::Null);
        if serde_json::to_vec(&id).map_or(usize::MAX, |b| b.len()) > MAX_ID_BYTES {
            shared.send(&Message::error(
                serde_json::Value::Null,
                codes::INVALID_REQUEST,
                format!("request id exceeds {MAX_ID_BYTES} bytes"),
            ))?;
            continue;
        }
        let params = msg.params.clone().unwrap_or(serde_json::Value::Null);
        let reply = match msg.method.as_deref() {
            Some("session/open") if open => {
                Message::error(id, codes::OPEN_FAILED, "the session is open already")
            }
            Some("session/open") => match serde_json::from_value::<OpenParams>(params) {
                Ok(p) => match open_session(engine, &p, shared) {
                    Ok(dir) => {
                        open = true;
                        let ok = Message::response(
                            id,
                            serde_json::json!({"ok": true, "outputDir": dir}),
                        );
                        #[cfg(any(feature = "engine-test-hooks", debug_assertions))]
                        if let Some(super::test_hooks::Hook::FloodAfterOpen(kind, count)) =
                            &shared.hook
                        {
                            shared.send(&ok)?;
                            flood(shared, *kind, *count)?;
                            continue;
                        }
                        #[cfg(any(feature = "engine-test-hooks", debug_assertions))]
                        if let Some(super::test_hooks::Hook::ExitAfterOpen) = &shared.hook {
                            shared.send(&ok)?;
                            std::process::exit(0);
                        }
                        ok
                    }
                    Err(why) => Message::error(id, codes::OPEN_FAILED, why),
                },
                Err(e) => Message::error(id, codes::INVALID_PARAMS, e.to_string()),
            },
            Some("session/ask" | "session/refresh") if !open => {
                Message::error(id, codes::NOT_OPEN, "session/open first")
            }
            Some("session/ask") => match serde_json::from_value::<AskParams>(params) {
                Ok(p) => {
                    if let Some(refused) = enqueue_ask(shared, id, p, frame.len()) {
                        shared.answer(refused)?;
                    }
                    continue;
                }
                Err(e) => Message::error(id, codes::INVALID_PARAMS, e.to_string()),
            },
            Some("session/refresh") => match serde_json::from_value::<RefreshParams>(params) {
                Ok(p) => {
                    {
                        let mut q = shared.lock();
                        q.heavy_items.push_back(Work::Refresh {
                            id: id.clone(),
                            wait: p.wait,
                            received: Instant::now(),
                        });
                    }
                    shared.ready.notify_all();
                    if p.wait {
                        continue;
                    }
                    Message::response(id, serde_json::json!({"started": true}))
                }
                Err(e) => Message::error(id, codes::INVALID_PARAMS, e.to_string()),
            },
            Some("session/cancel") => match serde_json::from_value::<CancelParams>(params) {
                Ok(p) => {
                    let found = cancel(shared, &p.id)?;
                    if notification {
                        continue;
                    }
                    Message::response(id, serde_json::json!({"cancelled": found}))
                }
                Err(e) if notification => {
                    let _ = e;
                    continue;
                }
                Err(e) => Message::error(id, codes::INVALID_PARAMS, e.to_string()),
            },
            Some("session/notifyChanged") => {
                match serde_json::from_value::<NotifyChangedParams>(params) {
                    Ok(p) => {
                        engine.notify_changed(&p.paths);
                        if notification {
                            continue;
                        }
                        Message::response(id, serde_json::json!({"ok": true}))
                    }
                    Err(_) if notification => continue,
                    Err(e) => Message::error(id, codes::INVALID_PARAMS, e.to_string()),
                }
            }
            Some("session/status") => {
                let mut status = engine.status();
                {
                    let q = shared.lock();
                    status.queued = (q.heavy_items.len() + q.light_items.len()) as u64;
                    status.in_flight = q.in_flight.first().map(|(id, _)| id.clone());
                    status.in_flight_all = q.in_flight.iter().map(|(id, _)| id.clone()).collect();
                }
                status.output_dir = shared
                    .dir
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .as_ref()
                    .map(|d| d.path().to_path_buf());
                Message::response(id, serde_json::to_value(status)?)
            }
            Some("session/shutdown") => return Ok(Some(id)),
            Some(_) if notification => continue,
            Some(other) => {
                Message::error(id, codes::METHOD_NOT_FOUND, format!("no method {other}"))
            }
            None => Message::error(id, codes::INVALID_REQUEST, "a request names its method"),
        };
        shared.send(&reply)?;
    }
}

/// Open the session: the private output directory first (refused if it
/// exists), then the engine. Returns the directory, or why not.
fn open_session<W: Write + Send>(
    engine: &dyn Engine,
    p: &OpenParams,
    shared: &Shared<'_, W>,
) -> Result<Option<PathBuf>, String> {
    // Windows fails closed: no directory, inline-only answers (§10).
    let dir = match OutputDir::create(&p.output_root, &shared.instance) {
        Ok(d) => Some(Arc::new(d)),
        Err(e) if e.kind() == std::io::ErrorKind::Unsupported => None,
        Err(e) => return Err(format!("output directory: {e}")),
    };
    engine.open(p, &shared.instance)?;
    let path = dir.as_ref().map(|d| d.path().to_path_buf());
    *shared.dir.lock().unwrap_or_else(|p| p.into_inner()) = dir;
    Ok(path)
}

/// Queue an ask, or the `busy` answer when the queue is full.
fn enqueue_ask<W: Write + Send>(
    shared: &Shared<'_, W>,
    id: serde_json::Value,
    params: AskParams,
    bytes: usize,
) -> Option<Answer> {
    let light = is_light(&params.argv);
    let received = Instant::now();
    let deadline = params
        .deadline_ms
        .map(|ms| received + std::time::Duration::from_millis(ms));
    {
        let mut q = shared.lock();
        let full = if light {
            q.light >= QUEUE_LIGHT
        } else {
            q.heavy >= QUEUE_HEAVY
        };
        if full || q.bytes + bytes > QUEUED_BYTES {
            drop(q);
            return Some(no_work(
                id,
                FailedOutcome::Busy,
                "the engine's queue is full; retry later",
            ));
        }
        if light {
            q.light += 1;
        } else {
            q.heavy += 1;
        }
        q.bytes += bytes;
        let ask = Work::Ask {
            id,
            params: Box::new(params),
            received,
            deadline,
            cancel: Arc::new(AtomicBool::new(false)),
            light,
            bytes,
        };
        if light {
            q.light_items.push_back(ask);
        } else {
            q.heavy_items.push_back(ask);
        }
    }
    shared.ready.notify_all();
    None
}

fn dequeued(q: &mut QueueState, light: bool, bytes: usize) {
    if light {
        q.light -= 1;
    } else {
        q.heavy -= 1;
    }
    q.bytes -= bytes;
}

/// Cancel `id`: a queued ask is answered `cancelled` at once; the ask in
/// flight gets its flag set. Returns whether the id was found.
fn cancel<W: Write + Send>(
    shared: &Shared<'_, W>,
    id: &serde_json::Value,
) -> std::io::Result<bool> {
    let removed = {
        let mut q = shared.lock();
        if let Some((_, flag)) = q.in_flight.iter().find(|(current, _)| current == id) {
            flag.store(true, Ordering::SeqCst);
            return Ok(true);
        }
        let is = |w: &Work| matches!(w, Work::Ask { id: i, .. } if i == id);
        let removed = match q.heavy_items.iter().position(is) {
            Some(i) => q.heavy_items.remove(i),
            None => match q.light_items.iter().position(is) {
                Some(i) => q.light_items.remove(i),
                None => None,
            },
        };
        match removed {
            Some(Work::Ask { light, bytes, .. }) => {
                dequeued(&mut q, light, bytes);
                true
            }
            _ => false,
        }
    };
    if removed {
        shared.answer(no_work(
            id.clone(),
            FailedOutcome::Cancelled,
            "cancelled while queued",
        ))?;
    }
    Ok(removed)
}

/// A worker of one lane (`light`: the queries; else every other ask and the
/// refreshes): one unit at a time, until closing with nothing queued in its
/// lane. A panic in a unit ends the process (as it does under the shipped
/// abort profile), so a front end sees the child die and restarts it, never
/// a child whose worker is gone while its reader still takes requests.
fn work<W: Write + Send>(
    engine: &dyn Engine,
    shared: &Shared<'_, W>,
    light: bool,
) -> std::io::Result<()> {
    loop {
        let (next, id) = {
            let mut q = shared.lock();
            loop {
                let popped = if light {
                    q.light_items.pop_front()
                } else {
                    q.heavy_items.pop_front()
                };
                if let Some(w) = popped {
                    let mut flight = None;
                    if let Work::Ask {
                        id,
                        cancel,
                        light,
                        bytes,
                        ..
                    } = &w
                    {
                        dequeued(&mut q, *light, *bytes);
                        q.in_flight.push((id.clone(), Arc::clone(cancel)));
                        flight = Some(id.clone());
                    }
                    break (w, flight);
                }
                if q.closing {
                    return Ok(());
                }
                q = shared.ready.wait(q).unwrap_or_else(|p| p.into_inner());
            }
        };
        let result =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(engine, shared, next)))
                .unwrap_or_else(|_| {
                    let _ = writeln!(
                        std::io::stderr(),
                        "al-sem: engine-host: a request panicked; the engine ends"
                    );
                    std::process::abort()
                });
        {
            let mut q = shared.lock();
            if let Some(id) = &id
                && let Some(i) = q.in_flight.iter().position(|(f, _)| f == id)
            {
                q.in_flight.remove(i);
            }
            q.last_done = Some(Instant::now());
        }
        result?;
    }
}

fn run<W: Write + Send>(
    engine: &dyn Engine,
    shared: &Shared<'_, W>,
    w: Work,
) -> std::io::Result<()> {
    let dir = shared.dir.lock().unwrap_or_else(|p| p.into_inner()).clone();
    let spill = |kind, bytes: &[u8]| match &dir {
        Some(d) => d.spill(kind, bytes),
        None => Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "no output directory on this platform: inline-only",
        )),
    };
    match w {
        Work::Ask {
            id,
            params,
            received,
            deadline,
            cancel,
            ..
        } => {
            let queued_ms = received.elapsed().as_millis() as u64;
            if deadline.is_some_and(|d| Instant::now() >= d) {
                return shared.answer(no_work(
                    id,
                    FailedOutcome::Deadline,
                    "the deadline passed while the ask was queued",
                ));
            }
            #[cfg(any(feature = "engine-test-hooks", debug_assertions))]
            if inject_ask_fault(shared)? {
                return Ok(());
            }
            let stop = || {
                if cancel.load(Ordering::SeqCst) {
                    Some(Stop::Cancelled)
                } else if deadline.is_some_and(|d| Instant::now() >= d) {
                    Some(Stop::Deadline)
                } else {
                    None
                }
            };
            let wants = params.progress;
            let progress = |phase: &str| {
                if wants {
                    let _ = shared.send(&Message::notification(
                        PROGRESS_METHOD,
                        serde_json::json!({"id": id, "phase": phase}),
                    ));
                }
            };
            let cx = AskCx {
                spill: &spill,
                stop: &stop,
                progress: &progress,
                queued_ms,
            };
            let answer = engine.ask(&id, *params, &cx);
            shared.answer(answer)
        }
        Work::Refresh { id, wait, received } => {
            let never = || None;
            let quiet = |_: &str| {};
            let cx = AskCx {
                spill: &spill,
                stop: &never,
                progress: &quiet,
                queued_ms: received.elapsed().as_millis() as u64,
            };
            let result = engine.refresh(&cx);
            if wait {
                shared.send(&Message::response(id, result))?;
            }
            Ok(())
        }
    }
}

/// The ask faults of the test hooks; `true` when the ask was answered (with
/// a broken frame) and the worker goes on.
#[cfg(any(feature = "engine-test-hooks", debug_assertions))]
fn inject_ask_fault<W: Write + Send>(shared: &Shared<'_, W>) -> std::io::Result<bool> {
    use super::test_hooks::Hook;
    match &shared.hook {
        Some(Hook::AbortOnAsk) => std::process::abort(),
        Some(Hook::SleepOnAsk(ms)) => {
            std::thread::sleep(std::time::Duration::from_millis(*ms));
            Ok(false)
        }
        Some(Hook::PanicOnAskOnce(marker)) => {
            if std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(marker)
                .is_ok()
            {
                panic!("engine test hook: a panic while answering an ask");
            }
            Ok(false)
        }
        Some(Hook::AbortOnAskOnce(marker)) => {
            if std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(marker)
                .is_ok()
            {
                std::process::abort();
            }
            Ok(false)
        }
        Some(Hook::OversizedFrameOnAsk) => {
            // More than the limit, in chunks, no newline: the front end must
            // stop reading at the limit and kill us (this write then fails
            // or blocks until the kill).
            let mut out = shared.out.lock().unwrap_or_else(|p| p.into_inner());
            let chunk = vec![b'x'; 1 << 20];
            for _ in 0..=(MAX_FRAME_BYTES >> 20) + 1 {
                out.write_all(&chunk)?;
            }
            out.write_all(b"\n")?;
            out.flush()?;
            Ok(true)
        }
        Some(Hook::GarbageFrameOnAsk) => {
            shared.write(b"this is not json\n")?;
            Ok(true)
        }
        _ => Ok(false),
    }
}

/// The flood of the `flood-after-open` hook: `count` small frames of `kind`,
/// unasked. A front end that kills us ends it with a failed write.
#[cfg(any(feature = "engine-test-hooks", debug_assertions))]
fn flood<W: Write + Send>(
    shared: &Shared<'_, W>,
    kind: super::test_hooks::Flood,
    count: usize,
) -> std::io::Result<()> {
    use super::test_hooks::Flood;
    // Let the front end take the `session/open` answer first: a failure
    // the flood causes discards what is queued, that answer included.
    std::thread::sleep(std::time::Duration::from_millis(500));
    for i in 0..count {
        match kind {
            Flood::Response => shared.send(&Message::response(
                serde_json::json!(0),
                serde_json::json!({}),
            ))?,
            Flood::Notify => shared.send(&Message::notification(
                &format!("engine/flood{i}"),
                serde_json::json!({}),
            ))?,
            Flood::Progress => shared.send(&Message::notification(
                PROGRESS_METHOD,
                serde_json::json!({"id": 7, "done": i}),
            ))?,
            Flood::Garbage => shared.write(b"x\n")?,
        }
    }
    Ok(())
}

/// The child's protocol stream: the process's real stdout, with fd 1 then
/// pointed at stderr, so a stray print anywhere in the engine lands in the
/// drained log and can never break the framing. On other platforms, stdout
/// itself.
#[cfg(unix)]
pub fn protocol_stdout() -> std::io::Result<std::fs::File> {
    use std::os::fd::FromRawFd;
    // SAFETY: plain descriptor calls on this process's own fds 1 and 2,
    // before any thread writes to stdout; `dup` returns a fresh fd this
    // function alone owns, handed to the `File`.
    unsafe {
        let proto = libc::dup(1);
        if proto < 0 || libc::dup2(2, 1) < 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(std::fs::File::from_raw_fd(proto))
    }
}

/// See the Unix version.
#[cfg(not(unix))]
pub fn protocol_stdout() -> std::io::Result<std::io::Stdout> {
    Ok(std::io::stdout())
}

#[cfg(test)]
#[path = "child_tests.rs"]
mod tests;

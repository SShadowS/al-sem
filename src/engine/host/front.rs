//! The front-end side of the engine protocol: [`EngineFront`] owns one child
//! (`alsem engine-host`), starts it on the first ask, and turns everything
//! that can go wrong with it into a defined `kind: "failed"` answer.
//!
//! Lifecycle (design r3 §3.2, review r2 #12):
//! - **Start.** Spawn, then the child's `engine/hello` within the hello
//!   deadline, with a protocol version in [`SUPPORTED_PROTOCOLS`]; then
//!   `session/open`. A missing or wrong hello kills the child.
//! - **Frames.** A reader thread reads the child's stdout one frame at a time
//!   ([`crate::wire::read_frame`]: at most the limit plus one byte of any
//!   line is held) and checks each frame's shape (a JSON-RPC response or a
//!   notification) BEFORE queueing it. A frame over the limit, or one of the
//!   wrong shape, kills the child (`protocol`); the reader stops at it and
//!   never recovers past it, so it has no delimiter boundary to get wrong.
//! - **The queue** of frames read but not yet taken is bounded (design r7
//!   §3, review r6 NEW 5): [`PROGRESS_METHOD`] notifications are coalesced
//!   per request (the newest replaces the queued one), and past
//!   `queue_frames` frames or `queue_bytes` bytes the READER kills the child
//!   at once, whether or not an ask is waiting, and records why: the pending
//!   ask (or the next one, or [`EngineFront::status`]) fails `protocol` with
//!   that reason, and [`EngineFront::queue_stats`] shows it before then.
//! - **stderr** is drained continuously by its own thread; the newest
//!   [`STDERR_RETAIN`] bytes are kept, never forwarded to stdout.
//! - **Death.** End of the child's stdout, or a write to it failing, fails the
//!   pending ask with the exit status and the stderr tail (`engineError`).
//! - **Restart** on the next ask, after a delay that starts at the initial
//!   backoff and doubles per death up to the maximum; it resets only after a
//!   completed `session/ask` or once the child has run for the stable period
//!   past its hello, never on a hello alone. [`DEATH_LIMIT`] deaths within
//!   [`DEATH_WINDOW`] fail every ask at once for [`FAILED_COOLDOWN`] or until
//!   [`EngineFront::reset`].
//! - **Shutdown** sends `session/shutdown`, waits for the exit up to the
//!   deadline, then kills. Dropping the front end kills and reaps the child,
//!   and removes the spill directory.

use std::collections::VecDeque;
use std::ffi::OsString;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use super::{
    AskParams, BACKOFF_INITIAL, BACKOFF_MAX, ChildStatus, DEATH_LIMIT, DEATH_WINDOW,
    FAILED_COOLDOWN, HELLO_DEADLINE, Hello, Message, OpenParams, PROGRESS_METHOD, QUEUE_BYTES,
    QUEUE_FRAMES, SHUTDOWN_DEADLINE, STABLE_AFTER, STDERR_RETAIN, SUPPORTED_PROTOCOLS,
};
use crate::wire::{Answer, FailedOutcome, ReadFrame, decode_frame};

/// How a front end starts and supervises its child. [`FrontConfig::new`]
/// gives the design's values; tests shorten the clocks.
#[derive(Clone, Debug)]
pub struct FrontConfig {
    /// The `alsem` binary.
    pub program: PathBuf,
    /// Its arguments (`engine-host`).
    pub args: Vec<OsString>,
    /// Extra environment for the child.
    pub env: Vec<(OsString, OsString)>,
    /// The workspace the session serves.
    pub workspace: PathBuf,
    /// Its universe (one child per workspace and universe; an ask of the
    /// other universe is refused `notThisWorkspace`).
    pub universe: super::Universe,
    pub hello_deadline: Duration,
    pub shutdown_deadline: Duration,
    pub backoff_initial: Duration,
    pub backoff_max: Duration,
    pub stable_after: Duration,
    pub death_limit: usize,
    pub death_window: Duration,
    pub failed_cooldown: Duration,
    /// The bound on queued, unread child frames ([`QUEUE_FRAMES`]).
    pub queue_frames: usize,
    /// The bound on queued, unread child bytes ([`QUEUE_BYTES`]).
    pub queue_bytes: usize,
}

impl FrontConfig {
    /// `program engine-host` serving `workspace`, with the design's clocks.
    #[must_use]
    pub fn new(program: impl Into<PathBuf>, workspace: impl Into<PathBuf>) -> FrontConfig {
        FrontConfig {
            program: program.into(),
            args: vec!["engine-host".into()],
            env: Vec::new(),
            workspace: workspace.into(),
            universe: super::Universe::CrossApp,
            hello_deadline: HELLO_DEADLINE,
            shutdown_deadline: SHUTDOWN_DEADLINE,
            backoff_initial: BACKOFF_INITIAL,
            backoff_max: BACKOFF_MAX,
            stable_after: STABLE_AFTER,
            death_limit: DEATH_LIMIT,
            death_window: DEATH_WINDOW,
            failed_cooldown: FAILED_COOLDOWN,
            queue_frames: QUEUE_FRAMES,
            queue_bytes: QUEUE_BYTES,
        }
    }
}

/// What the consumer takes from the child's output queue.
#[derive(Debug)]
enum Event {
    /// A protocol message, its shape checked.
    Frame(Message),
    /// Reading stopped on a protocol failure (an oversized or malformed
    /// frame, the queue bound, a read error); the reader killed the child.
    Bad(String),
    Eof,
}

impl Queue {
    /// Between requests (nothing asked is outstanding): drop the queued
    /// unsolicited notifications and say whether one was the child's
    /// [`super::IDLE_EXIT_METHOD`] (it is going away on its own).
    fn take_idle_notice(&self) -> bool {
        let mut s = self.lock();
        let mut idle = false;
        let mut kept = VecDeque::new();
        let mut bytes = 0;
        for q in s.items.drain(..) {
            if q.msg.method.as_deref() == Some(super::IDLE_EXIT_METHOD) {
                idle = true;
            } else if q.msg.method.is_none() {
                bytes += q.bytes;
                kept.push_back(q);
            }
        }
        s.items = kept;
        s.bytes = bytes;
        idle
    }
}

/// Why `recv` returned no event.
#[derive(Debug, PartialEq, Eq)]
enum RecvError {
    Timeout,
}

/// One queued frame.
#[derive(Debug)]
struct Queued {
    msg: Message,
    bytes: usize,
    /// The request a progress notification reports on (coalescing key).
    progress_of: Option<serde_json::Value>,
}

/// The queue's state under its lock.
#[derive(Debug, Default)]
struct QueueState {
    items: VecDeque<Queued>,
    bytes: usize,
    peak_frames: usize,
    peak_bytes: usize,
    /// Set once, when the reader stops: `Some(Bad)` wins over queued frames
    /// (the child is dead and the queue discarded), `Some(Eof)` is taken
    /// after them.
    end: Option<End>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum End {
    Eof,
    Bad(String),
}

/// The child's output between the reader thread and the consumer: bounded
/// by frames and bytes (design r7 §3).
#[derive(Debug, Default)]
struct Queue {
    state: Mutex<QueueState>,
    ready: Condvar,
}

/// The queue's counters (for tests and status).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct QueueStats {
    pub frames: usize,
    pub bytes: usize,
    pub peak_frames: usize,
    pub peak_bytes: usize,
    /// Why the reader stopped on a protocol failure (and killed the child).
    pub failure: Option<String>,
}

impl Queue {
    fn lock(&self) -> MutexGuard<'_, QueueState> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// The next event, waiting up to `timeout` (forever when `None`).
    fn recv(&self, timeout: Option<Duration>) -> Result<Event, RecvError> {
        let deadline = timeout.map(|t| Instant::now() + t);
        let mut s = self.lock();
        loop {
            if let Some(End::Bad(why)) = &s.end {
                return Ok(Event::Bad(why.clone()));
            }
            if let Some(q) = s.items.pop_front() {
                s.bytes -= q.bytes;
                return Ok(Event::Frame(q.msg));
            }
            if s.end == Some(End::Eof) {
                return Ok(Event::Eof);
            }
            s = match deadline {
                None => self.ready.wait(s).unwrap_or_else(|p| p.into_inner()),
                Some(d) => {
                    let left = d.saturating_duration_since(Instant::now());
                    if left.is_zero() {
                        return Err(RecvError::Timeout);
                    }
                    self.ready
                        .wait_timeout(s, left)
                        .unwrap_or_else(|p| p.into_inner())
                        .0
                }
            };
        }
    }

    /// Stop with `end`. A protocol failure discards what is queued.
    fn finish(&self, end: End) {
        let mut s = self.lock();
        if s.end.is_none() {
            if matches!(end, End::Bad(_)) {
                s.items.clear();
                s.bytes = 0;
            }
            s.end = Some(end);
        }
        drop(s);
        self.ready.notify_all();
    }

    fn stats(&self) -> QueueStats {
        let s = self.lock();
        QueueStats {
            frames: s.items.len(),
            bytes: s.bytes,
            peak_frames: s.peak_frames,
            peak_bytes: s.peak_bytes,
            failure: match &s.end {
                Some(End::Bad(why)) => Some(why.clone()),
                _ => None,
            },
        }
    }

    /// Queue one checked frame: a progress notification replaces the queued
    /// one for its request; anything past the bounds is refused (the
    /// caller then kills the child).
    fn push(&self, q: Queued, max_frames: usize, max_bytes: usize) -> Result<(), String> {
        let mut s = self.lock();
        let same = q.progress_of.as_ref().and_then(|r| {
            s.items
                .iter()
                .position(|i| i.progress_of.as_ref() == Some(r))
        });
        let (frames, bytes) = match same {
            Some(i) => (s.items.len(), s.bytes - s.items[i].bytes + q.bytes),
            None => (s.items.len() + 1, s.bytes + q.bytes),
        };
        if frames > max_frames || bytes > max_bytes {
            return Err(format!(
                "engine protocol error: the engine's unread output exceeds the front end's \
                 queue bound ({max_frames} frames, {max_bytes} bytes): {frames} frames, \
                 {bytes} bytes"
            ));
        }
        match same {
            Some(i) => s.items[i] = q,
            None => s.items.push_back(q),
        }
        s.bytes = bytes;
        s.peak_frames = s.peak_frames.max(frames);
        s.peak_bytes = s.peak_bytes.max(bytes);
        drop(s);
        self.ready.notify_all();
        Ok(())
    }
}

/// `frame` as a protocol message of a shape the front end takes: a response
/// (an id and exactly one of result and error, no method) or a notification
/// (a method, no id, no result or error).
fn checked(frame: &[u8]) -> Result<Queued, String> {
    let msg: Message = decode_frame(frame).map_err(|e| format!("engine protocol error: {e}"))?;
    let response = msg.method.is_none() && (msg.result.is_some() != msg.error.is_some());
    let notification =
        msg.method.is_some() && msg.id.is_none() && msg.result.is_none() && msg.error.is_none();
    if msg.jsonrpc != "2.0" || !(response || notification) {
        return Err(
            "engine protocol error: a frame that is no JSON-RPC response or notification"
                .to_string(),
        );
    }
    let progress_of = (msg.method.as_deref() == Some(PROGRESS_METHOD))
        .then(|| msg.params.as_ref().and_then(|p| p.get("id")).cloned())
        .flatten();
    Ok(Queued {
        bytes: frame.len(),
        msg,
        progress_of,
    })
}

/// The reader: frames from `reader` into `queue`, each checked before it is
/// queued, until end of input or a protocol failure. Returns the failure
/// (the queue already records it; the caller kills the child).
fn pump(
    mut reader: impl BufRead,
    queue: &Queue,
    max_frames: usize,
    max_bytes: usize,
) -> Option<String> {
    loop {
        let step = match crate::wire::read_frame(&mut reader) {
            Ok(ReadFrame::Frame(f)) => {
                checked(&f).and_then(|q| queue.push(q, max_frames, max_bytes))
            }
            Ok(ReadFrame::Eof) => {
                queue.finish(End::Eof);
                return None;
            }
            Ok(ReadFrame::Oversized(over)) => {
                Err(format!("engine protocol error: {}", over.error()))
            }
            Err(e) => Err(format!("engine protocol error: {e}")),
        };
        if let Err(why) = step {
            queue.finish(End::Bad(why.clone()));
            return Some(why);
        }
    }
}

/// The newest bytes of the child's stderr.
#[derive(Default)]
struct StderrTail {
    bytes: VecDeque<u8>,
    total: u64,
}

/// A started child.
struct Running {
    /// Shared with the reader thread, which kills the child on a protocol
    /// failure (`Child::kill` on a reaped child signals nothing).
    child: Arc<Mutex<Child>>,
    stdin: ChildStdin,
    frames: Arc<Queue>,
    stderr: Arc<Mutex<StderrTail>>,
    hello: Hello,
    since: Instant,
    /// The child announced its idle exit ([`super::IDLE_EXIT_METHOD`]).
    idle_notice: bool,
}

impl Running {
    fn child(&self) -> MutexGuard<'_, Child> {
        self.child.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Kill and reap.
    fn kill_and_reap(&self) -> Option<ExitStatus> {
        let mut c = self.child();
        let _ = c.kill();
        c.wait().ok()
    }
}

/// The front end's counters.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FrontStats {
    pub spawns: u64,
    pub deaths: u64,
    pub completed: u64,
    /// Children that ended on their idle timeout (never counted as deaths).
    pub retired: u64,
}

/// How a shutdown ended.
#[derive(Debug)]
pub struct ShutdownReport {
    /// The child answered `session/shutdown` and exited within the deadline.
    pub graceful: bool,
    pub status: Option<ExitStatus>,
}

/// One engine child and its supervision.
pub struct EngineFront {
    cfg: FrontConfig,
    output_root: tempfile::TempDir,
    running: Option<Running>,
    next_id: u64,
    backoff: Duration,
    not_before: Option<Instant>,
    deaths: VecDeque<Instant>,
    failed_until: Option<Instant>,
    last_stderr: String,
    last_failure: Option<(FailedOutcome, String)>,
    stats: FrontStats,
    /// The last call met the child's idle exit (its request was lost with
    /// it): the ask is sent once more to a new child.
    retired_mid_call: bool,
}

/// A failure before an answer: its outcome and message.
type Failure = (FailedOutcome, String);

/// What a caller watches an ask with ([`EngineFront::ask_with`]).
#[derive(Default)]
pub struct Watch<'a> {
    /// Set it (from another thread) to cancel the ask.
    pub cancel: Option<&'a AtomicBool>,
    /// Called with each progress notification's params (`{id, phase}`).
    pub progress: Option<&'a mut dyn FnMut(&serde_json::Value)>,
}

impl EngineFront {
    /// A front end for `cfg`; nothing is started until the first ask. The
    /// spill directory (the child's `outputRoot`) is a private temp folder
    /// owned by this front end.
    pub fn new(cfg: FrontConfig) -> std::io::Result<EngineFront> {
        Ok(EngineFront {
            backoff: cfg.backoff_initial,
            cfg,
            output_root: tempfile::Builder::new().prefix("alsem-engine-").tempdir()?,
            running: None,
            next_id: 1,
            not_before: None,
            deaths: VecDeque::new(),
            failed_until: None,
            last_stderr: String::new(),
            last_failure: None,
            stats: FrontStats::default(),
            retired_mid_call: false,
        })
    }

    /// Let the running child go without counting a death: close its input
    /// (an idle child ends on end of input), wait for it briefly, then kill
    /// and reap it.
    fn retire(&mut self) {
        let Some(run) = self.running.take() else {
            return;
        };
        let Running { child, stdin, .. } = run;
        drop(stdin);
        let mut child = child.lock().unwrap_or_else(|p| p.into_inner());
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            match child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                _ => {
                    let _ = child.kill();
                    let _ = child.wait();
                    break;
                }
            }
        }
        self.stats.retired += 1;
    }

    /// Ask the engine one command. Every failure is an answer: a dead,
    /// silent or misbehaving child gives `kind: "failed"` with its outcome.
    pub fn ask(&mut self, params: AskParams) -> Answer {
        self.ask_with(params, &mut Watch::default())
    }

    /// [`EngineFront::ask`], watched: once `watch.cancel` is set the child is
    /// sent `session/cancel` for this ask (it answers `cancelled`), and every
    /// progress notification for it is handed to `watch.progress` (ask with
    /// `progress: true` to get them).
    pub fn ask_with(&mut self, params: AskParams, watch: &mut Watch<'_>) -> Answer {
        let id = self.take_id();
        match self.ask_inner(&id, params, watch) {
            Ok(answer) => answer,
            Err((outcome, message)) => failed(id, outcome, message),
        }
    }

    /// Tell the child these paths changed (`session/notifyChanged`): the
    /// next freshness check hashes them again. Starts the child if needed.
    pub fn notify_changed(&mut self, paths: Vec<PathBuf>) -> Result<(), String> {
        self.ensure_running().map_err(|(_, m)| m)?;
        let params = serde_json::to_value(super::NotifyChangedParams { paths })
            .map_err(|e| e.to_string())?;
        self.notify("session/notifyChanged", params)
            .map_err(|(_, m)| m)
    }

    /// [`EngineFront::notify_changed`] to a running child only: `Ok(false)`,
    /// and nothing sent, when no child runs, or the running one announced
    /// its idle exit or was stopped by its reader (the next ask starts a
    /// new child, which checks the disk anyway). Never starts a child.
    pub fn notify_changed_if_running(&mut self, paths: Vec<PathBuf>) -> Result<bool, String> {
        let live = self
            .running
            .as_ref()
            .is_some_and(|r| !r.idle_notice && r.frames.stats().failure.is_none());
        if !live {
            return Ok(false);
        }
        let params = serde_json::to_value(super::NotifyChangedParams { paths })
            .map_err(|e| e.to_string())?;
        self.notify("session/notifyChanged", params)
            .map(|()| true)
            .map_err(|(_, m)| m)
    }

    /// `session/refresh {wait}`: with `wait`, the published revision after a
    /// fresh check (and any rebuild); without, `{started: true}` at once.
    pub fn refresh(&mut self, wait: bool) -> Result<serde_json::Value, String> {
        self.ensure_running().map_err(|(_, m)| m)?;
        let id = self.take_id();
        let params =
            serde_json::to_value(super::RefreshParams { wait }).map_err(|e| e.to_string())?;
        self.call(id, "session/refresh", params, None, &mut Watch::default())
            .map_err(|(_, m)| m)
    }

    /// One notification to the running child.
    fn notify(&mut self, method: &str, params: serde_json::Value) -> Result<(), Failure> {
        let frame = crate::wire::encode_frame(&Message::notification(method, params))
            .map_err(|e| (FailedOutcome::Protocol, e.to_string()))?;
        let run = self.running.as_mut().expect("a running child");
        if let Err(e) = run.stdin.write_all(&frame).and_then(|()| run.stdin.flush()) {
            return Err(self.died(format!("writing to the engine failed: {e}")));
        }
        Ok(())
    }

    fn ask_inner(
        &mut self,
        id: &serde_json::Value,
        params: AskParams,
        watch: &mut Watch<'_>,
    ) -> Result<Answer, Failure> {
        self.ensure_running()?;
        let params = serde_json::to_value(params)
            .map_err(|e| (FailedOutcome::Protocol, format!("encoding the ask: {e}")))?;
        self.retired_mid_call = false;
        let result = match self.call(id.clone(), "session/ask", params.clone(), None, watch) {
            // The ask crossed the child's idle exit: once more, to a new one.
            Err(_) if std::mem::take(&mut self.retired_mid_call) => {
                self.ensure_running()?;
                self.call(id.clone(), "session/ask", params, None, watch)?
            }
            r => r?,
        };
        match serde_json::from_value::<Answer>(result) {
            Ok(answer) if answer.id == *id => {
                // A completed ask: the child is doing useful work.
                self.stats.completed += 1;
                self.reset_backoff();
                Ok(answer)
            }
            Ok(answer) => Err(self.kill(
                FailedOutcome::Protocol,
                format!("engine answered id {} for ask {id}", answer.id),
            )),
            Err(e) => Err(self.kill(
                FailedOutcome::Protocol,
                format!("engine protocol error: the answer does not decode: {e}"),
            )),
        }
    }

    /// The child's status (starting it if needed).
    pub fn status(&mut self) -> Result<ChildStatus, String> {
        self.ensure_running().map_err(|(_, m)| m)?;
        let id = self.take_id();
        let v = self
            .call(
                id,
                "session/status",
                serde_json::Value::Null,
                None,
                &mut Watch::default(),
            )
            .map_err(|(_, m)| m)?;
        serde_json::from_value(v).map_err(|e| e.to_string())
    }

    /// The running child's hello and pid.
    #[must_use]
    pub fn hello(&self) -> Option<&Hello> {
        self.running.as_ref().map(|r| &r.hello)
    }

    /// The counters.
    #[must_use]
    pub fn stats(&self) -> FrontStats {
        self.stats.clone()
    }

    /// The delay the next restart waits after a death.
    #[must_use]
    pub fn current_backoff(&self) -> Duration {
        self.backoff
    }

    /// The newest stderr of the running child, or of the last one that died.
    #[must_use]
    pub fn stderr_tail(&self) -> String {
        match &self.running {
            Some(r) => tail_text(&r.stderr),
            None => self.last_stderr.clone(),
        }
    }

    /// Bytes of stderr the running child wrote in all (only the newest
    /// [`STDERR_RETAIN`] are kept).
    #[must_use]
    pub fn stderr_total(&self) -> u64 {
        self.running
            .as_ref()
            .map_or(0, |r| r.stderr.lock().expect("stderr tail").total)
    }

    /// Leave the failed state (an explicit refresh).
    pub fn reset(&mut self) {
        self.failed_until = None;
        self.deaths.clear();
        self.reset_backoff();
        self.not_before = None;
    }

    /// The spill directory the child writes into.
    #[must_use]
    pub fn output_root(&self) -> &std::path::Path {
        self.output_root.path()
    }

    /// Shut the child down: `session/shutdown`, then wait for its exit up to
    /// the deadline, then kill.
    pub fn shutdown(&mut self) -> ShutdownReport {
        let Some(mut run) = self.running.take() else {
            return ShutdownReport {
                graceful: true,
                status: None,
            };
        };
        let deadline = Instant::now() + self.cfg.shutdown_deadline;
        let id = serde_json::json!(self.next_id);
        self.next_id += 1;
        let asked = crate::wire::encode_frame(&Message::request(
            id.clone(),
            "session/shutdown",
            serde_json::Value::Null,
        ))
        .ok()
        .is_some_and(|f| {
            run.stdin
                .write_all(&f)
                .and_then(|()| run.stdin.flush())
                .is_ok()
        });
        let mut answered = false;
        while asked && !answered {
            let left = deadline.saturating_duration_since(Instant::now());
            match run.frames.recv(Some(left)) {
                Ok(Event::Frame(m)) => {
                    answered = m.id.as_ref() == Some(&id) && m.result.is_some();
                }
                _ => break,
            }
        }
        let Running { child, stdin, .. } = run;
        drop(stdin);
        let mut child = child.lock().unwrap_or_else(|p| p.into_inner());
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    return ShutdownReport {
                        graceful: answered,
                        status: Some(status),
                    };
                }
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(20));
                }
                _ => break,
            }
        }
        let _ = child.kill();
        ShutdownReport {
            graceful: false,
            status: child.wait().ok(),
        }
    }

    /// Start the child if none is running (as the first ask would), without
    /// asking it anything.
    pub fn ensure_started(&mut self) -> Result<(), String> {
        self.ensure_running().map_err(|(_, m)| m)
    }

    /// The running child's output-queue counters, and the protocol failure
    /// its reader stopped on (recorded even when no ask is waiting).
    #[must_use]
    pub fn queue_stats(&self) -> Option<QueueStats> {
        self.running.as_ref().map(|r| r.frames.stats())
    }

    /// Whether the running child has exited (reaping it if so; its status
    /// is kept for the front end's own reap).
    pub fn child_exited(&self) -> Option<bool> {
        self.running
            .as_ref()
            .map(|r| r.child().try_wait().is_ok_and(|s| s.is_some()))
    }

    /// The last failure that ended a child (its outcome and message).
    #[must_use]
    pub fn last_failure(&self) -> Option<(FailedOutcome, String)> {
        self.last_failure.clone()
    }

    fn take_id(&mut self) -> serde_json::Value {
        let id = serde_json::json!(self.next_id);
        self.next_id += 1;
        id
    }

    fn reset_backoff(&mut self) {
        self.backoff = self.cfg.backoff_initial;
        self.deaths.clear();
    }

    /// A child to ask: the running one (resetting the backoff once it has
    /// been stable), or a new one, after the backoff delay, unless the front
    /// end is in its failed state.
    fn ensure_running(&mut self) -> Result<(), Failure> {
        if let Some(until) = self.failed_until {
            if Instant::now() < until {
                return Err((
                    FailedOutcome::EngineError,
                    format!(
                        "engine failed: {} deaths within {} s; last stderr: {}",
                        self.cfg.death_limit,
                        self.cfg.death_window.as_secs(),
                        self.last_stderr
                    ),
                ));
            }
            self.reset();
        }
        // A child whose reader stopped on a protocol failure while nothing
        // was asked (the queue bound, a malformed frame) was killed by the
        // reader: this request fails with that reason and the death counts.
        let background = self.running.as_ref().and_then(|r| r.frames.stats().failure);
        if let Some(why) = background {
            return Err(self.kill(FailedOutcome::Protocol, why));
        }
        // A child that announced its idle exit is let go (no death, no
        // backoff) and a new one started for this request.
        let idle = self
            .running
            .as_ref()
            .is_some_and(|r| r.idle_notice || r.frames.take_idle_notice());
        if idle {
            self.retire();
        }
        if let Some(run) = &self.running {
            if run.since.elapsed() >= self.cfg.stable_after {
                self.reset_backoff();
            }
            return Ok(());
        }
        if let Some(at) = self.not_before.take() {
            std::thread::sleep(at.saturating_duration_since(Instant::now()));
        }
        self.start()
    }

    /// Spawn, take the hello, open the session.
    fn start(&mut self) -> Result<(), Failure> {
        let mut child = Command::new(&self.cfg.program)
            .args(&self.cfg.args)
            .envs(self.cfg.env.iter().map(|(k, v)| (k, v)))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| {
                (
                    FailedOutcome::EngineError,
                    format!("engine start failed: {}: {e}", self.cfg.program.display()),
                )
            })?;
        self.stats.spawns += 1;
        let stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");
        let stderr_pipe = child.stderr.take().expect("piped stderr");
        let stderr = Arc::new(Mutex::new(StderrTail::default()));
        let tail = Arc::clone(&stderr);
        std::thread::spawn(move || drain(stderr_pipe, &tail));
        let child = Arc::new(Mutex::new(child));
        let frames = Arc::new(Queue::default());
        {
            let (queue, child) = (Arc::clone(&frames), Arc::clone(&child));
            let (max_frames, max_bytes) = (self.cfg.queue_frames, self.cfg.queue_bytes);
            std::thread::spawn(move || {
                let reader = BufReader::with_capacity(1 << 16, stdout);
                if pump(reader, &queue, max_frames, max_bytes).is_some() {
                    // A protocol failure: end the child now, with no ask
                    // needed to notice (its output pipe closes with us).
                    let _ = child.lock().unwrap_or_else(|p| p.into_inner()).kill();
                }
            });
        }
        let since = Instant::now();
        let mut run = Running {
            child,
            stdin,
            frames,
            stderr,
            hello: Hello {
                protocol: 0,
                engine_instance_id: String::new(),
                alsem_version: String::new(),
                grammar_sha: String::new(),
                pid: 0,
            },
            since,
            idle_notice: false,
        };
        let hello = match run.frames.recv(Some(self.cfg.hello_deadline)) {
            Ok(Event::Frame(m)) => Some(m)
                .filter(|m| m.method.as_deref() == Some("engine/hello"))
                .and_then(|m| serde_json::from_value::<Hello>(m.params?).ok())
                .ok_or((
                    FailedOutcome::Protocol,
                    "engine protocol error: the first frame is not engine/hello".to_string(),
                )),
            Ok(Event::Bad(e)) => Err((FailedOutcome::Protocol, e)),
            Ok(Event::Eof) => Err((
                FailedOutcome::EngineError,
                "engine exited before its hello".to_string(),
            )),
            Err(RecvError::Timeout) => Err((
                FailedOutcome::EngineError,
                format!(
                    "engine start timeout: no hello within {} ms",
                    self.cfg.hello_deadline.as_millis()
                ),
            )),
        };
        let hello = match hello {
            Ok(h) if SUPPORTED_PROTOCOLS.contains(&h.protocol) => h,
            Ok(h) => {
                let why = format!(
                    "engine protocol mismatch: the engine speaks {}, this front end {:?}",
                    h.protocol, SUPPORTED_PROTOCOLS
                );
                self.running = Some(run);
                return Err(self.kill(FailedOutcome::Protocol, why));
            }
            Err((outcome, why)) => {
                self.running = Some(run);
                return Err(self.kill(outcome, why));
            }
        };
        run.hello = hello;
        run.since = Instant::now();
        self.running = Some(run);
        let open = OpenParams {
            workspace: self.cfg.workspace.clone(),
            output_root: self.output_root.path().to_path_buf(),
            universe: self.cfg.universe,
        };
        let id = self.take_id();
        let params = serde_json::to_value(open).expect("open params serialize");
        let deadline = Some(self.cfg.hello_deadline);
        match self.call(id, "session/open", params, deadline, &mut Watch::default()) {
            Ok(_) => Ok(()),
            Err((FailedOutcome::EngineError, why)) if self.running.is_some() => {
                // The child refused the session: it is alive but useless.
                Err(self.kill(FailedOutcome::EngineError, format!("session/open: {why}")))
            }
            Err(e) => Err(e),
        }
    }

    /// Send one request and wait for its response (notifications skipped).
    /// `Ok(result)`; a JSON-RPC error is `engineError` with the child left
    /// running; anything else kills the child.
    fn call(
        &mut self,
        id: serde_json::Value,
        method: &str,
        params: serde_json::Value,
        timeout: Option<Duration>,
        watch: &mut Watch<'_>,
    ) -> Result<serde_json::Value, Failure> {
        let frame = crate::wire::encode_frame(&Message::request(id.clone(), method, params))
            .map_err(|e| {
                (
                    FailedOutcome::Protocol,
                    format!("the request does not fit: {e}"),
                )
            })?;
        let run = self.running.as_mut().expect("a running child");
        if let Err(e) = run.stdin.write_all(&frame).and_then(|()| run.stdin.flush()) {
            return Err(self.died(format!("writing to the engine failed: {e}")));
        }
        let deadline = timeout.map(|t| Instant::now() + t);
        let mut cancel_sent = false;
        loop {
            if !cancel_sent && watch.cancel.is_some_and(|c| c.load(Ordering::SeqCst)) {
                cancel_sent = true;
                self.notify("session/cancel", serde_json::json!({ "id": id }))?;
            }
            // A watched call wakes up to look at its cancel flag.
            let slice = watch.cancel.map(|_| Duration::from_millis(25));
            let left = deadline.map(|d| d.saturating_duration_since(Instant::now()));
            let wait = match (left, slice) {
                (Some(l), Some(s)) => Some(l.min(s)),
                (l, s) => l.or(s),
            };
            let run = self.running.as_mut().expect("a running child");
            let event = run.frames.recv(wait);
            match event {
                Err(RecvError::Timeout)
                    if deadline.is_none_or(|d| Instant::now() < d) && slice.is_some() =>
                {
                    continue;
                }
                // Shapes were checked by the reader: a notification, or a
                // response with exactly one of result and error.
                Ok(Event::Frame(m)) if m.method.as_deref() == Some(super::IDLE_EXIT_METHOD) => {
                    // The child serves a request that reaches it after its
                    // notice; one that crosses its exit is retried once.
                    run.idle_notice = true;
                }
                Ok(Event::Frame(m)) if m.method.is_some() => {
                    let ours = m.method.as_deref() == Some(PROGRESS_METHOD)
                        && m.params.as_ref().and_then(|p| p.get("id")) == Some(&id);
                    if let (true, Some(f)) = (ours, watch.progress.as_mut()) {
                        f(m.params.as_ref().expect("checked above"));
                    }
                }
                Ok(Event::Frame(m)) if m.id.as_ref() == Some(&id) => {
                    if let Some(err) = m.error {
                        return Err((FailedOutcome::EngineError, err.message));
                    }
                    return m.result.ok_or_else(|| {
                        self.kill(
                            FailedOutcome::Protocol,
                            "engine protocol error: a response with no result".into(),
                        )
                    });
                }
                Ok(Event::Frame(m)) => {
                    let why = format!(
                        "engine protocol error: unexpected message for {id}: {}",
                        m.error
                            .map_or_else(|| format!("id {:?}", m.id), |e| e.message)
                    );
                    return Err(self.kill(FailedOutcome::Protocol, why));
                }
                Ok(Event::Bad(why)) => {
                    return Err(self.kill(FailedOutcome::Protocol, why));
                }
                Ok(Event::Eof) if run.idle_notice => {
                    self.retire();
                    self.retired_mid_call = true;
                    return Err((
                        FailedOutcome::EngineError,
                        "the engine ended on its idle timeout".into(),
                    ));
                }
                Ok(Event::Eof) => {
                    return Err(self.died("the engine closed its output".into()));
                }
                Err(RecvError::Timeout) => {
                    return Err(self.kill(
                        FailedOutcome::EngineError,
                        format!("the engine did not answer {method} in time"),
                    ));
                }
            }
        }
    }

    /// The child is gone (end of its output, a failed write): reap it and
    /// record the death.
    fn died(&mut self, why: String) -> Failure {
        let Some(run) = self.running.take() else {
            return self.failure(FailedOutcome::EngineError, why);
        };
        // It closed its output; give it a moment to exit before killing it.
        let deadline = Instant::now() + Duration::from_millis(500);
        let status = loop {
            let polled = run.child().try_wait();
            match polled {
                Ok(Some(s)) => break Some(s),
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                _ => break run.kill_and_reap(),
            }
        };
        let message = format!(
            "engine died ({why}; {}); stderr: {}",
            status.map_or_else(|| "status unknown".into(), describe),
            settle_tail(&run.stderr)
        );
        self.record_death(&run);
        self.failure(FailedOutcome::EngineError, message)
    }

    /// Kill the child for `why` and record the death.
    fn kill(&mut self, outcome: FailedOutcome, why: String) -> Failure {
        if let Some(run) = self.running.take() {
            run.kill_and_reap();
            let tail = settle_tail(&run.stderr);
            self.record_death(&run);
            if !tail.is_empty() {
                return self.failure(outcome, format!("{why}; stderr: {tail}"));
            }
        }
        self.failure(outcome, why)
    }

    /// `(outcome, why)`, recorded as the last failure.
    fn failure(&mut self, outcome: FailedOutcome, why: String) -> Failure {
        self.last_failure = Some((outcome, why.clone()));
        (outcome, why)
    }

    /// One death: the next start waits the current backoff, which doubles;
    /// too many deaths in the window enter the failed state.
    fn record_death(&mut self, run: &Running) {
        let now = Instant::now();
        self.stats.deaths += 1;
        self.last_stderr = tail_text(&run.stderr);
        self.not_before = Some(now + self.backoff);
        self.backoff = (self.backoff * 2).min(self.cfg.backoff_max);
        self.deaths.push_back(now);
        while self
            .deaths
            .front()
            .is_some_and(|t| now.duration_since(*t) > self.cfg.death_window)
        {
            self.deaths.pop_front();
        }
        if self.deaths.len() >= self.cfg.death_limit {
            self.failed_until = Some(now + self.cfg.failed_cooldown);
        }
    }
}

impl Drop for EngineFront {
    fn drop(&mut self) {
        if let Some(run) = self.running.take() {
            run.kill_and_reap();
        }
    }
}

/// A `kind: "failed"` answer for ask `id`.
fn failed(id: serde_json::Value, outcome: FailedOutcome, message: String) -> Answer {
    Answer::terminal(id, None, outcome, &message, None)
}

/// Copy `pipe` into `tail`, keeping its newest [`STDERR_RETAIN`] bytes.
fn drain(mut pipe: impl Read, tail: &Mutex<StderrTail>) {
    let mut buf = [0u8; 8192];
    loop {
        match pipe.read(&mut buf) {
            Ok(0) | Err(_) => return,
            Ok(n) => {
                let mut t = tail.lock().expect("stderr tail");
                t.total += n as u64;
                t.bytes.extend(&buf[..n]);
                let over = t.bytes.len().saturating_sub(STDERR_RETAIN);
                t.bytes.drain(..over);
            }
        }
    }
}

/// The tail as text, its last 2 KiB (what an answer's message carries).
fn tail_text(tail: &Mutex<StderrTail>) -> String {
    let t = tail.lock().expect("stderr tail");
    let bytes: Vec<u8> = t.bytes.iter().copied().collect();
    let from = bytes.len().saturating_sub(2048);
    String::from_utf8_lossy(&bytes[from..]).trim().to_string()
}

/// The tail once the drain thread has had a moment to read the dead child's
/// last bytes.
fn settle_tail(tail: &Mutex<StderrTail>) -> String {
    std::thread::sleep(Duration::from_millis(50));
    tail_text(tail)
}

/// An exit status in words: the code, or the signal.
fn describe(status: ExitStatus) -> String {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(sig) = status.signal() {
            return format!("killed by signal {sig}");
        }
    }
    status
        .code()
        .map_or_else(|| "no exit code".into(), |c| format!("exit code {c}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(msg: &Message) -> Vec<u8> {
        crate::wire::encode_frame(msg).unwrap()
    }

    fn response(id: u64) -> Vec<u8> {
        frame(&Message::response(
            serde_json::json!(id),
            serde_json::json!({}),
        ))
    }

    fn progress(request: u64, done: u64) -> Vec<u8> {
        frame(&Message::notification(
            PROGRESS_METHOD,
            serde_json::json!({"id": request, "done": done}),
        ))
    }

    /// `stream` pumped into a fresh queue with these bounds, nobody taking.
    fn pumped(stream: &[u8], frames: usize, bytes: usize) -> (Queue, Option<String>) {
        let queue = Queue::default();
        let failure = pump(stream, &queue, frames, bytes);
        (queue, failure)
    }

    /// Review r6 NEW 5: with nobody taking, a flood of valid small frames
    /// fills the queue only to its frame bound; the next frame is a protocol
    /// failure the queue records (and `recv` reports at once, the queued
    /// frames discarded) and the reader stops there.
    #[test]
    fn the_frame_bound_stops_a_flood_and_records_why() {
        let stream: Vec<u8> = (0..10_000).flat_map(response).collect();
        let (queue, failure) = pumped(&stream, 4, QUEUE_BYTES);
        let why = failure.expect("the flood is a protocol failure");
        assert!(why.contains("queue bound"), "{why}");
        let stats = queue.stats();
        assert_eq!(stats.peak_frames, 4);
        assert_eq!(stats.failure.as_deref(), Some(why.as_str()));
        assert!(matches!(queue.recv(None), Ok(Event::Bad(w)) if w == why));
        assert_eq!(queue.stats().frames, 0, "discarded");
    }

    /// The byte bound holds the same way, below the frame bound.
    #[test]
    fn the_byte_bound_stops_a_flood() {
        let one = response(1).len();
        let stream: Vec<u8> = (0..10_000).flat_map(|_| response(1)).collect();
        let (queue, failure) = pumped(&stream, QUEUE_FRAMES, 10 * one);
        assert!(failure.is_some_and(|w| w.contains("queue bound")));
        let stats = queue.stats();
        assert_eq!((stats.peak_frames, stats.peak_bytes), (10, 10 * one));
    }

    /// Progress notifications are coalesced per request: thousands of them
    /// for two requests hold two frames, the newest of each, and are no
    /// failure; frames queued before the end of input are still taken
    /// before it.
    #[test]
    fn progress_is_coalesced_per_request() {
        let mut stream: Vec<u8> = (0..5_000)
            .flat_map(|i| [progress(7, i), progress(8, i)].concat())
            .collect();
        stream.extend(response(3));
        let (queue, failure) = pumped(&stream, 4, QUEUE_BYTES);
        assert_eq!(failure, None);
        assert_eq!(queue.stats().peak_frames, 3);
        let mut taken = Vec::new();
        loop {
            match queue.recv(Some(Duration::from_secs(1))).unwrap() {
                Event::Frame(m) => taken.push(m),
                Event::Eof => break,
                Event::Bad(w) => panic!("{w}"),
            }
        }
        let done: Vec<_> = taken
            .iter()
            .map(|m| {
                m.params
                    .as_ref()
                    .map(|p| (p["id"].clone(), p["done"].clone()))
            })
            .collect();
        assert_eq!(
            done,
            [
                Some((serde_json::json!(7), serde_json::json!(4999))),
                Some((serde_json::json!(8), serde_json::json!(4999))),
                None
            ]
        );
    }

    /// Shape is checked before a frame is queued: a line that is not JSON,
    /// a request (method and id), a response with both result and error, a
    /// wrong `jsonrpc` each stop the reader with nothing of theirs queued.
    #[test]
    fn a_frame_of_the_wrong_shape_is_refused_before_queueing() {
        let request = frame(&Message::request(
            serde_json::json!(1),
            "session/ask",
            serde_json::Value::Null,
        ));
        let mut both = Message::response(serde_json::json!(1), serde_json::json!({}));
        both.error = Some(super::super::RpcError {
            code: 1,
            message: "x".into(),
        });
        let mut v1 = Message::response(serde_json::json!(1), serde_json::json!({}));
        v1.jsonrpc = "1.0".into();
        for bad in [b"x\n".to_vec(), request, frame(&both), frame(&v1)] {
            let (queue, failure) = pumped(&bad, QUEUE_FRAMES, QUEUE_BYTES);
            assert!(
                failure.is_some_and(|w| w.starts_with("engine protocol error")),
                "{}",
                String::from_utf8_lossy(&bad)
            );
            assert_eq!(queue.stats().peak_frames, 0);
        }
    }
}

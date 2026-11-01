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
//!   line is held). A frame over the limit, or one that does not parse as a
//!   protocol message, kills the child (`protocol`).
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
use std::io::{BufReader, Read, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::{
    AskParams, BACKOFF_INITIAL, BACKOFF_MAX, ChildStatus, DEATH_LIMIT, DEATH_WINDOW,
    FAILED_COOLDOWN, HELLO_DEADLINE, Hello, Message, OpenParams, SHUTDOWN_DEADLINE, STABLE_AFTER,
    STDERR_RETAIN, SUPPORTED_PROTOCOLS,
};
use crate::wire::{Answer, FailedOutcome, MESSAGE_INLINE_BYTES, Outcome, decode_frame};

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
    pub hello_deadline: Duration,
    pub shutdown_deadline: Duration,
    pub backoff_initial: Duration,
    pub backoff_max: Duration,
    pub stable_after: Duration,
    pub death_limit: usize,
    pub death_window: Duration,
    pub failed_cooldown: Duration,
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
            hello_deadline: HELLO_DEADLINE,
            shutdown_deadline: SHUTDOWN_DEADLINE,
            backoff_initial: BACKOFF_INITIAL,
            backoff_max: BACKOFF_MAX,
            stable_after: STABLE_AFTER,
            death_limit: DEATH_LIMIT,
            death_window: DEATH_WINDOW,
            failed_cooldown: FAILED_COOLDOWN,
        }
    }
}

/// What the reader thread saw on the child's stdout.
enum Event {
    Frame(Vec<u8>),
    /// A frame over the limit or a read error: reading stopped.
    Bad(String),
    Eof,
}

/// The newest bytes of the child's stderr.
#[derive(Default)]
struct StderrTail {
    bytes: VecDeque<u8>,
    total: u64,
}

/// A started child.
struct Running {
    child: Child,
    stdin: ChildStdin,
    frames: Receiver<Event>,
    stderr: Arc<Mutex<StderrTail>>,
    hello: Hello,
    since: Instant,
}

/// The front end's counters.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FrontStats {
    pub spawns: u64,
    pub deaths: u64,
    pub completed: u64,
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
    stats: FrontStats,
}

/// A failure before an answer: its outcome and message.
type Failure = (FailedOutcome, String);

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
            stats: FrontStats::default(),
        })
    }

    /// Ask the engine one command. Every failure is an answer: a dead,
    /// silent or misbehaving child gives `kind: "failed"` with its outcome.
    pub fn ask(&mut self, params: AskParams) -> Answer {
        let id = self.take_id();
        match self.ask_inner(&id, params) {
            Ok(answer) => answer,
            Err((outcome, message)) => failed(id, outcome, message),
        }
    }

    fn ask_inner(&mut self, id: &serde_json::Value, params: AskParams) -> Result<Answer, Failure> {
        self.ensure_running()?;
        let params = serde_json::to_value(params)
            .map_err(|e| (FailedOutcome::Protocol, format!("encoding the ask: {e}")))?;
        let result = self.call(id.clone(), "session/ask", params, None)?;
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
            .call(id, "session/status", serde_json::Value::Null, None)
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
            match run.frames.recv_timeout(left) {
                Ok(Event::Frame(f)) => {
                    answered = decode_frame::<Message>(&f)
                        .is_ok_and(|m| m.id.as_ref() == Some(&id) && m.result.is_some());
                }
                _ => break,
            }
        }
        drop(run.stdin);
        loop {
            match run.child.try_wait() {
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
        let _ = run.child.kill();
        ShutdownReport {
            graceful: false,
            status: run.child.wait().ok(),
        }
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
        let (tx, frames) = channel();
        std::thread::spawn(move || {
            let mut reader = BufReader::with_capacity(1 << 16, stdout);
            loop {
                let event = match crate::wire::read_frame(&mut reader) {
                    Ok(Some(f)) => Event::Frame(f),
                    Ok(None) => Event::Eof,
                    Err(e) => Event::Bad(e.to_string()),
                };
                let last = !matches!(event, Event::Frame(_));
                if tx.send(event).is_err() || last {
                    return;
                }
            }
        });
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
        };
        let hello = match run.frames.recv_timeout(self.cfg.hello_deadline) {
            Ok(Event::Frame(f)) => decode_frame::<Message>(&f)
                .ok()
                .filter(|m| m.method.as_deref() == Some("engine/hello"))
                .and_then(|m| serde_json::from_value::<Hello>(m.params?).ok())
                .ok_or((
                    FailedOutcome::Protocol,
                    "engine protocol error: the first frame is not engine/hello".to_string(),
                )),
            Ok(Event::Bad(e)) => Err((
                FailedOutcome::Protocol,
                format!("engine protocol error: {e}"),
            )),
            Ok(Event::Eof) | Err(RecvTimeoutError::Disconnected) => Err((
                FailedOutcome::EngineError,
                "engine exited before its hello".to_string(),
            )),
            Err(RecvTimeoutError::Timeout) => Err((
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
        };
        let id = self.take_id();
        let params = serde_json::to_value(open).expect("open params serialize");
        let deadline = Some(self.cfg.hello_deadline);
        match self.call(id, "session/open", params, deadline) {
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
        loop {
            let run = self.running.as_mut().expect("a running child");
            let event = match deadline {
                Some(d) => run
                    .frames
                    .recv_timeout(d.saturating_duration_since(Instant::now())),
                None => run
                    .frames
                    .recv()
                    .map_err(|_| RecvTimeoutError::Disconnected),
            };
            match event {
                Ok(Event::Frame(f)) => match decode_frame::<Message>(&f) {
                    Ok(m) if m.id.is_none() && m.method.is_some() => {}
                    Ok(m) if m.id.as_ref() == Some(&id) => {
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
                    Ok(m) => {
                        let why = format!(
                            "engine protocol error: unexpected message for {id}: {}",
                            m.error
                                .map_or_else(|| format!("id {:?}", m.id), |e| e.message)
                        );
                        return Err(self.kill(FailedOutcome::Protocol, why));
                    }
                    Err(e) => {
                        return Err(self.kill(
                            FailedOutcome::Protocol,
                            format!("engine protocol error: {e}"),
                        ));
                    }
                },
                Ok(Event::Bad(e)) => {
                    return Err(self.kill(
                        FailedOutcome::Protocol,
                        format!("engine protocol error: {e}"),
                    ));
                }
                Ok(Event::Eof) | Err(RecvTimeoutError::Disconnected) => {
                    return Err(self.died("the engine closed its output".into()));
                }
                Err(RecvTimeoutError::Timeout) => {
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
        let Some(mut run) = self.running.take() else {
            return (FailedOutcome::EngineError, why);
        };
        // It closed its output; give it a moment to exit before killing it.
        let deadline = Instant::now() + Duration::from_millis(500);
        let status = loop {
            match run.child.try_wait() {
                Ok(Some(s)) => break Some(s),
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                _ => {
                    let _ = run.child.kill();
                    break run.child.wait().ok();
                }
            }
        };
        let message = format!(
            "engine died ({why}; {}); stderr: {}",
            status.map_or_else(|| "status unknown".into(), describe),
            settle_tail(&run.stderr)
        );
        self.record_death(&run);
        (FailedOutcome::EngineError, message)
    }

    /// Kill the child for `why` and record the death.
    fn kill(&mut self, outcome: FailedOutcome, why: String) -> Failure {
        if let Some(mut run) = self.running.take() {
            let _ = run.child.kill();
            let _ = run.child.wait();
            let tail = settle_tail(&run.stderr);
            self.record_death(&run);
            if !tail.is_empty() {
                return (outcome, format!("{why}; stderr: {tail}"));
            }
        }
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
        if let Some(mut run) = self.running.take() {
            let _ = run.child.kill();
            let _ = run.child.wait();
        }
    }
}

/// A `kind: "failed"` answer for ask `id`.
fn failed(id: serde_json::Value, outcome: FailedOutcome, message: String) -> Answer {
    Answer {
        id,
        outcome: Outcome::Failed {
            outcome,
            message: cut_front(&message, MESSAGE_INLINE_BYTES),
            diagnostic_file: None,
        },
        revision: None,
    }
}

/// `text`'s first `max` bytes on a character boundary.
fn cut_front(text: &str, max: usize) -> String {
    let mut end = text.len().min(max);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_string()
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

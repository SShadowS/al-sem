//! The child's serve loop over stub engines: framing, the queue, cancel,
//! deadlines, busy, the output directory, shutdown order.

use std::sync::atomic::AtomicU64;
use std::time::Duration;

use super::*;
use crate::wire::FailedOutcome;
#[cfg(unix)]
use crate::wire::MESSAGE_INLINE_BYTES;

/// How a stub answers an ask.
#[derive(Clone, Copy, Default)]
enum Mode {
    /// At once, `busy` with the argv as message.
    #[default]
    Echo,
    /// Sleep this long, then echo.
    Sleep(u64),
    /// Wait until stopped, then answer the stop with work done.
    UntilStopped,
}

#[derive(Default)]
struct Stub {
    asks: AtomicU64,
    mode: Mode,
    changed: Mutex<Vec<PathBuf>>,
}

impl Engine for Stub {
    fn open(&self, _: &OpenParams, _: &str) -> Result<(), String> {
        Ok(())
    }
    fn ask(&self, id: &serde_json::Value, params: AskParams, cx: &AskCx<'_>) -> Answer {
        self.asks.fetch_add(1, Ordering::SeqCst);
        let echo = || {
            Answer::new(
                id.clone(),
                Outcome::failed(FailedOutcome::Busy, params.argv.join(" ")),
                None,
            )
        };
        match self.mode {
            Mode::Echo => echo(),
            Mode::Sleep(ms) => {
                std::thread::sleep(Duration::from_millis(ms));
                echo()
            }
            Mode::UntilStopped => {
                let start = Instant::now();
                let stop = loop {
                    if let Some(s) = (cx.stop)() {
                        break s;
                    }
                    assert!(start.elapsed() < Duration::from_secs(20), "never stopped");
                    std::thread::sleep(Duration::from_millis(5));
                };
                let outcome = match stop {
                    Stop::Cancelled => FailedOutcome::Cancelled,
                    Stop::Deadline => FailedOutcome::Deadline,
                };
                let mut a = Answer::new(id.clone(), Outcome::failed(outcome, "stopped"), None);
                a.meta.work = Some(WorkDone {
                    work_done_ms: start.elapsed().as_millis() as u64,
                    skipped_phases: vec!["compute".into()],
                });
                a
            }
        }
    }
    fn status(&self) -> ChildStatus {
        ChildStatus {
            asks: self.asks.load(Ordering::SeqCst),
            ..ChildStatus::default()
        }
    }
    fn notify_changed(&self, paths: &[PathBuf]) {
        self.changed.lock().unwrap().extend_from_slice(paths);
    }
}

fn line(msg: &Message) -> Vec<u8> {
    encode_frame(msg).unwrap()
}

/// The frames `serve` wrote for `input`, after its hello.
fn serve_all(stub: &Stub, input: Vec<u8>) -> Vec<Message> {
    let mut out = Vec::new();
    serve(stub, std::io::Cursor::new(input), &mut out).unwrap();
    let mut frames = out.split(|&b| b == b'\n').filter(|l| !l.is_empty());
    let hello: Message = decode_frame(frames.next().expect("a hello")).unwrap();
    assert_eq!(hello.method.as_deref(), Some("engine/hello"));
    let hello: Hello = serde_json::from_value(hello.params.unwrap()).unwrap();
    assert_eq!(hello.protocol, super::super::PROTOCOL_VERSION);
    frames.map(|f| decode_frame(f).unwrap()).collect()
}

fn open(dir: &std::path::Path) -> Message {
    Message::request(
        serde_json::json!(1),
        "session/open",
        serde_json::to_value(OpenParams {
            workspace: dir.into(),
            output_root: dir.into(),
            universe: super::super::Universe::CrossApp,
        })
        .unwrap(),
    )
}

fn ask_with(id: serde_json::Value, word: &str, deadline_ms: Option<u64>) -> Message {
    Message::request(
        id,
        "session/ask",
        serde_json::to_value(AskParams {
            argv: vec![word.into()],
            deadline_ms,
            ..AskParams::default()
        })
        .unwrap(),
    )
}

fn ask(id: serde_json::Value, word: &str) -> Message {
    ask_with(id, word, None)
}

fn by_id(replies: &[Message], id: serde_json::Value) -> &Message {
    replies
        .iter()
        .find(|m| m.id.as_ref() == Some(&id))
        .unwrap_or_else(|| panic!("a reply for {id}: {replies:?}"))
}

fn answer_of(m: &Message) -> Answer {
    serde_json::from_value(m.result.clone().expect("a result")).unwrap()
}

fn failed_of(m: &Message) -> FailedOutcome {
    match answer_of(m).outcome {
        Outcome::Failed { outcome, .. } => outcome,
        o => panic!("failed expected: {o:?}"),
    }
}

fn code(m: &Message) -> Option<i64> {
    m.error.as_ref().map(|e| e.code)
}

/// The child's side of the frame rules: an ask before `session/open` is
/// refused; a request line over 16 MiB is answered with a parse error and
/// skipped without being held, and the next request is served; a line that
/// is not JSON gets a parse error; an id over 1 KiB is refused at decode; an
/// unknown method is not found; `session/shutdown` answers after the queued
/// asks and ends the loop (a later line is never served).
#[test]
fn serve_bounds_and_refuses_what_it_must() {
    let dir = tempfile::tempdir().unwrap();
    let mut input = Vec::new();
    input.extend(line(&ask(serde_json::json!(0), "early")));
    input.extend(line(&open(dir.path())));
    input.extend(vec![b'x'; MAX_FRAME_BYTES + 5]);
    input.push(b'\n');
    input.extend(line(&ask(serde_json::json!(2), "after-oversize")));
    input.extend(b"not json\n");
    input.extend(line(&ask(serde_json::json!("i".repeat(2000)), "long-id")));
    let request = |id: u64, method: &str| {
        line(&Message::request(
            serde_json::json!(id),
            method,
            serde_json::Value::Null,
        ))
    };
    input.extend(request(3, "session/nope"));
    input.extend(request(4, "session/status"));
    input.extend(request(5, "session/shutdown"));
    input.extend(line(&ask(serde_json::json!(6), "never")));
    let stub = Stub::default();
    let replies = serve_all(&stub, input);
    assert_eq!(
        code(by_id(&replies, serde_json::json!(0))),
        Some(codes::NOT_OPEN)
    );
    let opened = by_id(&replies, serde_json::json!(1))
        .result
        .clone()
        .unwrap();
    assert_eq!(opened["ok"], true);
    assert!(opened["outputDir"].as_str().is_some());
    let parse: Vec<&Message> = replies
        .iter()
        .filter(|m| code(m) == Some(codes::PARSE_ERROR))
        .collect();
    assert_eq!(parse.len(), 2, "the oversized line and the non-JSON line");
    assert!(parse[0].error.as_ref().unwrap().message.contains("exceeds"));
    let a2 = answer_of(by_id(&replies, serde_json::json!(2)));
    assert_eq!(
        a2.outcome,
        Outcome::failed(FailedOutcome::Busy, "after-oversize")
    );
    assert!(
        replies
            .iter()
            .any(|m| m.id.is_none() && code(m) == Some(codes::INVALID_REQUEST)),
        "the long id is refused with a null id"
    );
    assert_eq!(
        code(by_id(&replies, serde_json::json!(3))),
        Some(codes::METHOD_NOT_FOUND)
    );
    assert!(by_id(&replies, serde_json::json!(4)).result.is_some());
    // The shutdown answer is the last frame, after the ask it followed.
    assert_eq!(replies.last().unwrap().id, Some(serde_json::json!(5)));
    assert!(replies.iter().all(|m| m.id != Some(serde_json::json!(6))));
    assert_eq!(stub.asks.load(Ordering::SeqCst), 1);
}

/// Review r6 NEW 4: recovery from an oversized request line at every
/// boundary, each followed by a valid request whose response must arrive. A
/// request of exactly the limit (newline included) is served; a line of the
/// limit plus one byte INCLUDING its newline is refused and the next request
/// is NOT skipped; a longer line is refused and its rest skipped; an
/// oversized last line at end of input is refused and the loop ends cleanly.
#[test]
fn serve_recovers_at_every_oversize_boundary() {
    let dir = tempfile::tempdir().unwrap();
    let status = |id: u64, pad: usize| {
        line(&Message::request(
            serde_json::json!(id),
            "session/status",
            serde_json::json!({ "pad": "a".repeat(pad) }),
        ))
    };
    let exact = {
        let base = status(10, 0).len();
        let f = status(10, MAX_FRAME_BYTES - base);
        assert_eq!(f.len(), MAX_FRAME_BYTES, "precondition: exactly the limit");
        f
    };
    let mut input = line(&open(dir.path()));
    input.extend(exact);
    input.extend(vec![b'x'; MAX_FRAME_BYTES]);
    input.push(b'\n');
    input.extend(status(11, 0));
    input.extend(vec![b'x'; MAX_FRAME_BYTES + 5]);
    input.push(b'\n');
    input.extend(status(12, 0));
    input.extend(vec![b'x'; MAX_FRAME_BYTES + 3]);
    let replies = serve_all(&Stub::default(), input);
    let ids: Vec<_> = replies.iter().map(|m| (m.id.clone(), code(m))).collect();
    let parse = (None, Some(codes::PARSE_ERROR));
    let ok = |id: u64| (Some(serde_json::json!(id)), None);
    assert_eq!(
        ids,
        [
            ok(1),
            ok(10),
            parse.clone(),
            ok(11),
            parse.clone(),
            ok(12),
            parse
        ],
        "{replies:?}"
    );
}

/// The child bounds its own response: an answer whose message is over 4 KiB
/// is cut, its whole text in a diagnostic file inside the session's private
/// instance directory `<outputRoot>/<engineInstanceId>/` (Unix: off Unix
/// there is no directory).
#[cfg(unix)]
#[test]
fn serve_spills_a_long_message_into_the_instance_directory() {
    let dir = tempfile::tempdir().unwrap();
    let long = "m".repeat(10_000);
    let mut input = line(&open(dir.path()));
    input.extend(line(&ask(serde_json::json!(9), &long)));
    let replies = serve_all(&Stub::default(), input);
    let out_dir = by_id(&replies, serde_json::json!(1))
        .result
        .clone()
        .unwrap()["outputDir"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(std::path::Path::new(&out_dir).starts_with(dir.path()));
    let Outcome::Failed {
        message,
        diagnostic_file: Some(f),
        ..
    } = answer_of(by_id(&replies, serde_json::json!(9))).outcome
    else {
        panic!("a cut message with its file")
    };
    assert_eq!(message.len(), MESSAGE_INLINE_BYTES);
    assert!(std::path::Path::new(&f.path).starts_with(&out_dir));
    assert_eq!(std::fs::read_to_string(&f.path).unwrap(), long);
}

/// Inline-only (no output directory, Windows): a failed spill is answered
/// `refusedPath` with its cause kept; any other failure, and a CLI answer,
/// pass unchanged.
///
/// DISCRIMINATION (2026-11-02): matching the prefix of another error
/// (`WireError::Json` in place of `WireError::Spill`) leaves the first
/// answer `protocol`: the `RefusedPath` assertion fails.
#[test]
fn inline_only_refuses_an_undeliverable_answer() {
    let spill = crate::wire::WireError::Spill("no output directory".into()).to_string();
    let a = refuse_undeliverable(Answer::new(
        serde_json::json!(1),
        Outcome::failed(FailedOutcome::Protocol, &spill),
        None,
    ));
    let Outcome::Failed {
        outcome, message, ..
    } = &a.outcome
    else {
        panic!("a failed answer")
    };
    assert_eq!(*outcome, FailedOutcome::RefusedPath);
    assert!(
        message.contains("inline-only") && message.ends_with(&spill),
        "{message}"
    );
    let other = refuse_undeliverable(Answer::new(
        serde_json::json!(2),
        Outcome::failed(FailedOutcome::Protocol, "encoding the answer failed"),
        None,
    ));
    assert!(matches!(
        other.outcome,
        Outcome::Failed {
            outcome: FailedOutcome::Protocol,
            ..
        }
    ));
}

/// A second `session/open` of an open session is refused; the first made
/// exactly one owner-only instance directory under the output root (the
/// pre-existing-directory refusal is `output::tests`).
#[cfg(unix)]
#[test]
fn open_makes_one_private_instance_directory() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let mut input = line(&open(dir.path()));
    input.extend(line(&open(dir.path())));
    let replies = serve_all(&Stub::default(), input);
    assert!(replies[0].result.is_some());
    assert_eq!(code(&replies[1]), Some(codes::OPEN_FAILED), "open twice");
    let made: Vec<_> = std::fs::read_dir(dir.path()).unwrap().collect();
    assert_eq!(made.len(), 1);
    let mode = made[0]
        .as_ref()
        .unwrap()
        .metadata()
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o700);
}

/// T-cancel at the protocol: a queued ask is answered `cancelled` at once
/// with no work done; the ask in flight stops at its next check and answers
/// `cancelled` with what it did. A notification needs no response.
///
/// DISCRIMINATION (2026-11-02): never setting the in-flight flag (the
/// `flag.store` line removed) leaves the first ask waiting: its stub fails
/// "never stopped" after 20 s (the worker thread panics; `serve` errs).
#[test]
fn cancel_stops_a_queued_and_an_in_flight_ask() {
    let dir = tempfile::tempdir().unwrap();
    let mut input = line(&open(dir.path()));
    input.extend(line(&ask(serde_json::json!("a"), "slow")));
    input.extend(line(&ask(serde_json::json!("b"), "queued")));
    let cancel = |id: &str| {
        line(&Message::notification(
            "session/cancel",
            serde_json::json!({ "id": id }),
        ))
    };
    input.extend(cancel("b"));
    input.extend(cancel("a"));
    let stub = Stub {
        mode: Mode::UntilStopped,
        ..Stub::default()
    };
    let replies = serve_all(&stub, input);
    assert_eq!(
        failed_of(by_id(&replies, serde_json::json!("b"))),
        FailedOutcome::Cancelled
    );
    let a = answer_of(by_id(&replies, serde_json::json!("a")));
    assert!(matches!(
        a.outcome,
        Outcome::Failed {
            outcome: FailedOutcome::Cancelled,
            ..
        }
    ));
    let b = answer_of(by_id(&replies, serde_json::json!("b")));
    assert_eq!(
        b.meta.work.as_ref().map(|w| w.work_done_ms),
        Some(0),
        "a queued ask did no work"
    );
    assert!(stub.asks.load(Ordering::SeqCst) <= 1, "b never ran");
}

/// Deadlines: an ask whose deadline passed while queued answers `deadline`
/// with no work; one that expires while computing stops at its next check.
#[test]
fn deadlines_expire_queued_and_running_asks() {
    let dir = tempfile::tempdir().unwrap();
    let mut input = line(&open(dir.path()));
    input.extend(line(&ask_with(
        serde_json::json!("running"),
        "slow",
        Some(300),
    )));
    input.extend(line(&ask_with(serde_json::json!("queued"), "x", Some(0))));
    let stub = Stub {
        mode: Mode::UntilStopped,
        ..Stub::default()
    };
    let replies = serve_all(&stub, input);
    assert_eq!(
        failed_of(by_id(&replies, serde_json::json!("running"))),
        FailedOutcome::Deadline
    );
    let q = answer_of(by_id(&replies, serde_json::json!("queued")));
    assert!(matches!(
        q.outcome,
        Outcome::Failed {
            outcome: FailedOutcome::Deadline,
            ..
        }
    ));
    assert_eq!(q.meta.work.unwrap().work_done_ms, 0);
    assert_eq!(
        stub.asks.load(Ordering::SeqCst),
        1,
        "the expired ask never ran"
    );
}

/// Busy: more heavy asks than the queue holds are answered `busy` at once;
/// every ask gets exactly one answer.
#[test]
fn a_full_queue_answers_busy() {
    let dir = tempfile::tempdir().unwrap();
    let mut input = line(&open(dir.path()));
    let n = QUEUE_HEAVY + 4;
    for i in 0..n {
        input.extend(line(&ask(serde_json::json!(100 + i), "explain")));
    }
    let stub = Stub {
        mode: Mode::Sleep(100),
        ..Stub::default()
    };
    let replies = serve_all(&stub, input);
    let answers: Vec<Answer> = replies
        .iter()
        .filter(|m| {
            m.id.as_ref()
                .and_then(|v| v.as_u64())
                .is_some_and(|i| i >= 100)
        })
        .map(answer_of)
        .collect();
    assert_eq!(answers.len(), n, "one answer per ask");
    let busy = answers
        .iter()
        .filter(|a| {
            matches!(&a.outcome, Outcome::Failed { message, .. } if message.contains("queue is full"))
        })
        .count();
    assert!(busy >= 3, "{busy} busy");
    assert_eq!(stub.asks.load(Ordering::SeqCst) as usize, n - busy);
}

/// `session/notifyChanged` reaches the engine; as a request it is answered.
#[test]
fn notify_changed_reaches_the_engine() {
    let dir = tempfile::tempdir().unwrap();
    let mut input = line(&open(dir.path()));
    input.extend(line(&Message::notification(
        "session/notifyChanged",
        serde_json::json!({"paths": ["/x/a.al"]}),
    )));
    input.extend(line(&Message::request(
        serde_json::json!(5),
        "session/notifyChanged",
        serde_json::json!({"paths": ["/x/b.al"]}),
    )));
    let stub = Stub::default();
    let replies = serve_all(&stub, input);
    assert!(by_id(&replies, serde_json::json!(5)).result.is_some());
    assert_eq!(
        *stub.changed.lock().unwrap(),
        vec![PathBuf::from("/x/a.al"), PathBuf::from("/x/b.al")]
    );
}

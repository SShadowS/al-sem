//! The child side of the engine protocol: [`serve`] reads requests from the
//! front end, one frame at a time, and writes one bounded frame per response.
//! What a request computes is the [`Engine`]'s (the `alsem` binary's
//! planner and job path); this module owns framing, dispatch, bounding and
//! the spill files.

use std::io::{BufRead, Write};
use std::path::PathBuf;

use super::{AskParams, ChildStatus, Hello, MAX_ID_BYTES, Message, OpenParams, codes};
use crate::wire::{
    Answer, MAX_FRAME_BYTES, Outcome, SpillFile, SpillKind, WireError, bound_frame, decode_frame,
    encode_frame, read_frame, skip_line,
};

/// What a child computes per request.
pub trait Engine {
    /// `session/open`: the workspace this session serves.
    fn open(&mut self, params: &OpenParams) -> Result<(), String>;
    /// `session/ask`: one command's outcome. `spill` writes a stdout too
    /// large to inline.
    fn ask(&mut self, params: AskParams, spill: &crate::wire::Spill<'_>) -> Outcome;
    /// `session/status`.
    fn status(&self) -> ChildStatus;
}

/// Serve the protocol until the front end shuts the session down or closes
/// the input. The hello goes first. A frame over the limit is answered with
/// a parse error and the rest of its line discarded without holding it; a
/// frame that is not JSON, with a parse error; an id over [`MAX_ID_BYTES`],
/// with an invalid-request error. Returns when the input ends (a child whose
/// stdin closes exits) or after answering `session/shutdown`.
pub fn serve(
    engine: &mut dyn Engine,
    mut input: impl BufRead,
    mut output: impl Write,
) -> std::io::Result<()> {
    #[cfg(any(feature = "engine-test-hooks", debug_assertions))]
    let hook = super::test_hooks::from_env();
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
    let instance = hello.engine_instance_id.clone();
    send(
        &mut output,
        &Message::notification("engine/hello", serde_json::to_value(&hello)?),
    )?;
    let mut output_root: Option<PathBuf> = None;
    let spills = std::cell::Cell::new(0u64);
    loop {
        let frame = match read_frame(&mut input) {
            Ok(Some(frame)) => frame,
            Ok(None) => return Ok(()),
            Err(WireError::FrameTooLarge(_)) => {
                skip_line(&mut input)?;
                send(
                    &mut output,
                    &Message::error(
                        serde_json::Value::Null,
                        codes::PARSE_ERROR,
                        format!("frame exceeds {MAX_FRAME_BYTES} bytes"),
                    ),
                )?;
                continue;
            }
            Err(e) => return Err(std::io::Error::other(e.to_string())),
        };
        let msg: Message = match decode_frame(&frame) {
            Ok(m) => m,
            Err(e) => {
                send(
                    &mut output,
                    &Message::error(serde_json::Value::Null, codes::PARSE_ERROR, e.to_string()),
                )?;
                continue;
            }
        };
        let id = msg.id.clone().unwrap_or(serde_json::Value::Null);
        if serde_json::to_vec(&id).map_or(usize::MAX, |b| b.len()) > MAX_ID_BYTES {
            send(
                &mut output,
                &Message::error(
                    serde_json::Value::Null,
                    codes::INVALID_REQUEST,
                    format!("request id exceeds {MAX_ID_BYTES} bytes"),
                ),
            )?;
            continue;
        }
        let params = msg.params.clone().unwrap_or(serde_json::Value::Null);
        let reply = match msg.method.as_deref() {
            Some("session/open") => match serde_json::from_value::<OpenParams>(params) {
                Ok(open) => match engine.open(&open) {
                    Ok(()) => {
                        output_root = Some(open.output_root);
                        Message::response(id, serde_json::json!({"ok": true}))
                    }
                    Err(why) => Message::error(id, codes::OPEN_FAILED, why),
                },
                Err(e) => Message::error(id, codes::INVALID_PARAMS, e.to_string()),
            },
            Some("session/ask") => {
                let Some(root) = &output_root else {
                    send(
                        &mut output,
                        &Message::error(id, codes::NOT_OPEN, "session/open first"),
                    )?;
                    continue;
                };
                let ask = match serde_json::from_value::<AskParams>(params) {
                    Ok(a) => a,
                    Err(e) => {
                        send(
                            &mut output,
                            &Message::error(id, codes::INVALID_PARAMS, e.to_string()),
                        )?;
                        continue;
                    }
                };
                #[cfg(any(feature = "engine-test-hooks", debug_assertions))]
                if inject_ask_fault(hook.as_ref(), &mut output)? {
                    continue;
                }
                let spill = |kind: SpillKind, bytes: &[u8]| {
                    spills.set(spills.get() + 1);
                    write_spill(root, &instance, spills.get(), kind, bytes)
                };
                let answer = Answer {
                    id: id.clone(),
                    outcome: engine.ask(ask, &spill),
                    revision: None,
                };
                let wrap =
                    |a: &Answer| serde_json::json!({"jsonrpc": "2.0", "id": a.id, "result": a});
                let frame = bound_frame(answer, MAX_FRAME_BYTES, &wrap, &spill);
                output.write_all(&frame)?;
                output.flush()?;
                continue;
            }
            Some("session/status") => Message::response(id, serde_json::to_value(engine.status())?),
            Some("session/shutdown") => {
                send(
                    &mut output,
                    &Message::response(id, serde_json::json!({"ok": true})),
                )?;
                return Ok(());
            }
            Some(other) => {
                Message::error(id, codes::METHOD_NOT_FOUND, format!("no method {other}"))
            }
            None => Message::error(id, codes::INVALID_REQUEST, "a request names its method"),
        };
        send(&mut output, &reply)?;
    }
}

/// One small protocol message as a frame on `output`.
fn send(output: &mut impl Write, msg: &Message) -> std::io::Result<()> {
    let frame = encode_frame(msg).map_err(|e| std::io::Error::other(e.to_string()))?;
    output.write_all(&frame)?;
    output.flush()
}

/// Spilled bytes in a new file under the session's output root: an opaque
/// name, created exclusively (an existing file is never overwritten).
fn write_spill(
    root: &std::path::Path,
    instance: &str,
    n: u64,
    kind: SpillKind,
    bytes: &[u8],
) -> std::io::Result<SpillFile> {
    let tag = match kind {
        SpillKind::Stdout => "stdout",
        SpillKind::Stderr => "stderr",
        SpillKind::Diagnostic => "diagnostic",
    };
    let path = root.join(format!("{instance}-{n}.{tag}"));
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)?;
    f.write_all(bytes)?;
    f.flush()?;
    Ok(SpillFile::of(path.to_string_lossy().into_owned(), bytes))
}

/// The ask faults of the test hooks; `true` when the ask was answered (with
/// a broken frame) and the loop goes on.
#[cfg(any(feature = "engine-test-hooks", debug_assertions))]
fn inject_ask_fault(
    hook: Option<&super::test_hooks::Hook>,
    output: &mut impl Write,
) -> std::io::Result<bool> {
    use super::test_hooks::Hook;
    match hook {
        Some(Hook::AbortOnAsk) => std::process::abort(),
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
            let chunk = vec![b'x'; 1 << 20];
            for _ in 0..=(MAX_FRAME_BYTES >> 20) + 1 {
                output.write_all(&chunk)?;
            }
            output.write_all(b"\n")?;
            output.flush()?;
            Ok(true)
        }
        Some(Hook::GarbageFrameOnAsk) => {
            output.write_all(b"this is not json\n")?;
            output.flush()?;
            Ok(true)
        }
        _ => Ok(false),
    }
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
mod tests {
    use super::*;
    use crate::wire::FailedOutcome;

    /// An engine that answers every ask with a fixed failure and counts.
    #[derive(Default)]
    struct Stub {
        asks: u64,
    }

    impl Engine for Stub {
        fn open(&mut self, _: &OpenParams) -> Result<(), String> {
            Ok(())
        }
        fn ask(&mut self, params: AskParams, _: &crate::wire::Spill<'_>) -> Outcome {
            self.asks += 1;
            Outcome::failed(FailedOutcome::Busy, params.argv.join(" "))
        }
        fn status(&self) -> ChildStatus {
            ChildStatus {
                asks: self.asks,
                ..ChildStatus::default()
            }
        }
    }

    fn line(msg: &Message) -> Vec<u8> {
        encode_frame(msg).unwrap()
    }

    /// The responses `serve` wrote for `input`, after its hello.
    fn serve_all(input: &[u8]) -> (Vec<Message>, Stub) {
        let mut stub = Stub::default();
        let mut out = Vec::new();
        serve(&mut stub, std::io::BufReader::new(input), &mut out).unwrap();
        let mut frames = out.split(|&b| b == b'\n').filter(|l| !l.is_empty());
        let hello: Message = decode_frame(frames.next().expect("a hello")).unwrap();
        assert_eq!(hello.method.as_deref(), Some("engine/hello"));
        let hello: Hello = serde_json::from_value(hello.params.unwrap()).unwrap();
        assert_eq!(hello.protocol, super::super::PROTOCOL_VERSION);
        (frames.map(|f| decode_frame(f).unwrap()).collect(), stub)
    }

    fn open(dir: &std::path::Path) -> Message {
        Message::request(
            serde_json::json!(1),
            "session/open",
            serde_json::to_value(OpenParams {
                workspace: dir.into(),
                output_root: dir.into(),
            })
            .unwrap(),
        )
    }

    fn ask(id: serde_json::Value, word: &str) -> Message {
        Message::request(
            id,
            "session/ask",
            serde_json::to_value(AskParams {
                argv: vec![word.into()],
                ..AskParams::default()
            })
            .unwrap(),
        )
    }

    /// The child's side of the frame rules: an ask before `session/open` is
    /// refused; a request line over 16 MiB is answered with a parse error
    /// and skipped without being held, and the next request is served; a
    /// line that is not JSON gets a parse error; an id over 1 KiB is
    /// refused at decode; an unknown method is not found; `session/shutdown`
    /// answers and ends the loop (a later line is never read).
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
        let (replies, stub) = serve_all(&input);
        let code = |m: &Message| m.error.as_ref().map(|e| e.code);
        assert_eq!(code(&replies[0]), Some(codes::NOT_OPEN));
        assert_eq!(replies[1].result, Some(serde_json::json!({"ok": true})));
        assert_eq!(code(&replies[2]), Some(codes::PARSE_ERROR));
        assert!(
            replies[2]
                .error
                .as_ref()
                .unwrap()
                .message
                .contains("exceeds")
        );
        let answer: Answer = serde_json::from_value(replies[3].result.clone().unwrap()).unwrap();
        assert_eq!(answer.id, serde_json::json!(2));
        assert_eq!(
            answer.outcome,
            Outcome::failed(FailedOutcome::Busy, "after-oversize")
        );
        assert_eq!(code(&replies[4]), Some(codes::PARSE_ERROR));
        assert_eq!(code(&replies[5]), Some(codes::INVALID_REQUEST));
        // `"id": null` on the wire (it reads back as no id).
        assert_eq!(replies[5].id, None);
        assert_eq!(code(&replies[6]), Some(codes::METHOD_NOT_FOUND));
        let status: ChildStatus =
            serde_json::from_value(replies[7].result.clone().unwrap()).unwrap();
        assert_eq!(status.asks, 1);
        assert_eq!(replies[8].result, Some(serde_json::json!({"ok": true})));
        assert_eq!(replies.len(), 9, "nothing after shutdown");
        assert_eq!(stub.asks, 1);
    }

    /// The child bounds its own response: an answer whose message is over
    /// 4 KiB is cut, its whole text in a diagnostic file under the session's
    /// output root (created exclusively).
    #[test]
    fn serve_spills_a_long_message_under_the_output_root() {
        let dir = tempfile::tempdir().unwrap();
        let long = "m".repeat(10_000);
        let mut input = line(&open(dir.path()));
        input.extend(line(&ask(serde_json::json!(9), &long)));
        let (replies, _) = serve_all(&input);
        let answer: Answer = serde_json::from_value(replies[1].result.clone().unwrap()).unwrap();
        let Outcome::Failed {
            message,
            diagnostic_file: Some(f),
            ..
        } = answer.outcome
        else {
            panic!("a cut message with its file: {:?}", answer.outcome)
        };
        assert_eq!(message.len(), crate::wire::MESSAGE_INLINE_BYTES);
        assert!(std::path::Path::new(&f.path).starts_with(dir.path()));
        assert_eq!(std::fs::read_to_string(&f.path).unwrap(), long);
    }
}

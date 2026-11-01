//! The exact-output wire codec of the context server (context-server design
//! r3 §3.2, §4.3 and §13 item 2; review r1 #8; r5 §3 and §8.3): one module
//! for the engine protocol and the MCP side.
//!
//! **Frames.** NDJSON: one JSON object per line, UTF-8, compact (`serde_json`
//! escapes every newline inside a string, so a frame holds exactly one `\n`,
//! its terminator), at most [`MAX_FRAME_BYTES`] including that newline. A
//! reader buffers at most the limit plus one byte of any line
//! ([`read_frame`]), so an oversized frame is refused before it is held.
//!
//! **A CLI answer's stdout** travels as the exact bytes, never re-serialized:
//! inline as `stdoutText`, a JSON **string** whose decoded value is the stdout
//! byte for byte (embedded newlines and the trailing newline included), or,
//! when it is not UTF-8, larger than the request's inline cap or too large
//! once escaped for one frame, as `stdoutFile` (a file the engine wrote, with
//! its length and SHA-256). Exactly one of the two (`kind: "cli"`). Embedding
//! the CLI's pretty JSON as raw JSON (`RawValue`) was rejected: its literal
//! newlines break the line framing, and re-serializing a parsed value does not
//! keep its bytes. Its stderr travels as lines (`stderr`) or, when the frame
//! would not fit, as `stderrFile` (the exact bytes).
//!
//! **Outcomes (r5 §8.3).** `kind: "cli"` is a CLI outcome: the command ran,
//! or failed its planning (`Planned::Failed`), or its model did not build (a
//! **semantic** build failure, the job's own `view_failed` text and exit).
//! `kind: "failed"` is an **infrastructure** outcome ([`FailedOutcome`]: the
//! engine died, a frame broke, the inputs moved during the build, ...) with a
//! `message` of at most [`MESSAGE_INLINE_BYTES`] inline and the whole text in
//! `diagnosticFile`. Either names the revision that answered, or `null`.
//!
//! **Bounding (r5 §3).** [`bound_frame`] encodes a complete response and, if
//! it is over the limit, spills in a fixed order (stdout, stderr, changed
//! paths, message) and ends with a minimal terminal frame, so every answer
//! fits one frame. The child bounds what it sends; a front end bounds what it
//! re-sends against its own limit, independently.

use std::io::BufRead;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::engine::batch::output::{JobOutput, WriteAction};

/// The largest frame in either direction, its newline included.
pub const MAX_FRAME_BYTES: usize = 16 * 1024 * 1024;

/// The largest inline stdout a request may ask for (`maxInlineBytes`).
pub const MAX_INLINE_BYTES: usize = 4 * 1024 * 1024;

/// The inline cap when a request names none.
pub const DEFAULT_INLINE_BYTES: usize = 65_536;

/// The most of a `kind: "failed"` message carried inline (r5 §8.3); the rest
/// is in its `diagnosticFile`.
pub const MESSAGE_INLINE_BYTES: usize = 4096;

/// The most `changedPaths` entries a bounded frame keeps (r4 §3).
pub const CHANGED_PATHS_KEPT: usize = 256;

/// Room a frame keeps for everything but the escaped stdout (the envelope's
/// other fields and the stderr lines are checked again at [`bound_frame`]).
const FRAME_HEADROOM: usize = 1024 * 1024;

/// What can go wrong encoding or decoding a frame or an answer.
#[derive(Debug, PartialEq, Eq)]
pub enum WireError {
    /// A frame above [`MAX_FRAME_BYTES`] (its size).
    FrameTooLarge(usize),
    /// A frame that is not one line (a raw newline before its end).
    NotOneLine,
    /// A frame whose JSON does not parse as the expected message.
    Json(String),
    /// `kind: "cli"` with both or neither of `stdoutText` / `stdoutFile`.
    StdoutNotExclusive,
    /// A job wrote a file (`--out`): the server's output sink handles those.
    FileWrite(String),
    /// Spilling stdout to a file failed.
    Spill(String),
    /// Reading a frame failed.
    Io(String),
}

impl std::fmt::Display for WireError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WireError::FrameTooLarge(n) => {
                write!(f, "frame of {n} bytes exceeds {MAX_FRAME_BYTES}")
            }
            WireError::NotOneLine => f.write_str("frame is not one line"),
            WireError::Json(e) => write!(f, "frame JSON: {e}"),
            WireError::StdoutNotExclusive => {
                f.write_str("a cli answer carries exactly one of stdoutText and stdoutFile")
            }
            WireError::FileWrite(p) => write!(f, "a job wrote a file ({p})"),
            WireError::Spill(e) => write!(f, "spilling stdout: {e}"),
            WireError::Io(e) => write!(f, "reading a frame: {e}"),
        }
    }
}

impl std::error::Error for WireError {}

/// A request's inline cap: its `maxInlineBytes`, clamped to
/// [`MAX_INLINE_BYTES`]; [`DEFAULT_INLINE_BYTES`] when it names none.
#[must_use]
pub fn clamp_inline(requested: Option<u64>) -> usize {
    requested.map_or(DEFAULT_INLINE_BYTES, |n| {
        usize::try_from(n).map_or(MAX_INLINE_BYTES, |n| n.min(MAX_INLINE_BYTES))
    })
}

/// Bytes the engine wrote to a file instead of inlining them: a stdout, a
/// stderr, or a diagnostic.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpillFile {
    pub path: String,
    pub bytes: u64,
    pub sha256: String,
}

/// The stage-0 name of a spilled stdout.
pub type StdoutFile = SpillFile;

impl SpillFile {
    /// The description of `bytes` written at `path`.
    #[must_use]
    pub fn of(path: String, bytes: &[u8]) -> SpillFile {
        SpillFile {
            path,
            bytes: bytes.len() as u64,
            sha256: crate::engine::ids::sha256_bytes_hex(bytes),
        }
    }
}

/// What a spill is of: names the file a [`Spill`] writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpillKind {
    Stdout,
    Stderr,
    Diagnostic,
}

/// Writes spilled bytes to a new file and describes it.
pub type Spill<'a> = dyn Fn(SpillKind, &[u8]) -> std::io::Result<SpillFile> + 'a;

/// A CLI answer's stdout: exactly one form.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Stdout {
    /// The exact bytes, valid UTF-8, inline.
    Text(String),
    /// The exact bytes, in a file.
    File(SpillFile),
}

/// A CLI answer's stderr: its lines (each without its newline), or the exact
/// bytes in a file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Stderr {
    Lines(Vec<String>),
    File(SpillFile),
}

/// The revision that answered (r3 §4.7).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RevisionId {
    pub engine_instance_id: String,
    pub generation: u64,
    pub inputs_stamp: String,
}

/// Why no command answered (r5 §8.3): an infrastructure outcome. A semantic
/// build failure is never one of these: it is the command's own CLI answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FailedOutcome {
    Cancelled,
    Deadline,
    Budget,
    Busy,
    /// The engine died, could not start, or answered with an error.
    EngineError,
    RefusedPath,
    NotThisWorkspace,
    /// A package changed while the build consumed it (`M_before`,
    /// `M_consumed`, `M_after` differ, r5 §5.1).
    InputsUnstable,
    /// A frame or codec failure.
    Protocol,
}

impl FailedOutcome {
    /// The wire spelling.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            FailedOutcome::Cancelled => "cancelled",
            FailedOutcome::Deadline => "deadline",
            FailedOutcome::Budget => "budget",
            FailedOutcome::Busy => "busy",
            FailedOutcome::EngineError => "engineError",
            FailedOutcome::RefusedPath => "refusedPath",
            FailedOutcome::NotThisWorkspace => "notThisWorkspace",
            FailedOutcome::InputsUnstable => "inputsUnstable",
            FailedOutcome::Protocol => "protocol",
        }
    }
}

/// What answered a request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// A CLI outcome: its exit, its stdout, its stderr; the paths that
    /// changed since the revision (none until freshness exists).
    Cli {
        exit_code: u8,
        stdout: Stdout,
        stderr: Stderr,
        changed_paths: Vec<String>,
        changed_paths_truncated: bool,
    },
    /// An infrastructure outcome: why no command answered.
    Failed {
        outcome: FailedOutcome,
        /// At most [`MESSAGE_INLINE_BYTES`].
        message: String,
        /// The whole message (or, for the terminal fallback, the whole
        /// encoded answer), when the inline one is cut.
        diagnostic_file: Option<SpillFile>,
    },
}

impl Outcome {
    /// A `kind: "failed"` outcome whose message is short enough inline; a
    /// longer one goes through [`bound_frame`], which cuts and spills it.
    #[must_use]
    pub fn failed(outcome: FailedOutcome, message: impl Into<String>) -> Outcome {
        Outcome::Failed {
            outcome,
            message: message.into(),
            diagnostic_file: None,
        }
    }
}

/// One request's answer: its id, its outcome, the revision (or `null`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RawAnswer", into = "RawAnswer")]
pub struct Answer {
    pub id: serde_json::Value,
    pub outcome: Outcome,
    pub revision: Option<RevisionId>,
}

/// [`Answer`]'s wire shape: the discriminator and every optional field,
/// checked into the typed form on decode.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawAnswer {
    id: serde_json::Value,
    kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    exit_code: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    stdout_text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    stdout_file: Option<SpillFile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    stderr: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    stderr_file: Option<SpillFile>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    changed_paths: Vec<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    changed_paths_truncated: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    outcome: Option<FailedOutcome>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    diagnostic_file: Option<SpillFile>,
    /// Always present; `null` before any revision exists.
    revision: Option<RevisionId>,
}

impl From<Answer> for RawAnswer {
    fn from(a: Answer) -> RawAnswer {
        let mut raw = RawAnswer {
            id: a.id,
            kind: String::new(),
            exit_code: None,
            stdout_text: None,
            stdout_file: None,
            stderr: None,
            stderr_file: None,
            changed_paths: Vec::new(),
            changed_paths_truncated: false,
            outcome: None,
            message: None,
            diagnostic_file: None,
            revision: a.revision,
        };
        match a.outcome {
            Outcome::Cli {
                exit_code,
                stdout,
                stderr,
                changed_paths,
                changed_paths_truncated,
            } => {
                raw.kind = "cli".into();
                raw.exit_code = Some(exit_code);
                match stdout {
                    Stdout::Text(t) => raw.stdout_text = Some(t),
                    Stdout::File(f) => raw.stdout_file = Some(f),
                }
                match stderr {
                    Stderr::Lines(l) => raw.stderr = Some(l),
                    Stderr::File(f) => raw.stderr_file = Some(f),
                }
                raw.changed_paths = changed_paths;
                raw.changed_paths_truncated = changed_paths_truncated;
            }
            Outcome::Failed {
                outcome,
                message,
                diagnostic_file,
            } => {
                raw.kind = "failed".into();
                raw.outcome = Some(outcome);
                raw.message = Some(message);
                raw.diagnostic_file = diagnostic_file;
            }
        }
        raw
    }
}

impl TryFrom<RawAnswer> for Answer {
    type Error = WireError;

    fn try_from(raw: RawAnswer) -> Result<Answer, WireError> {
        let outcome = match raw.kind.as_str() {
            "cli" => {
                let stdout = match (raw.stdout_text, raw.stdout_file) {
                    (Some(t), None) => Stdout::Text(t),
                    (None, Some(f)) => Stdout::File(f),
                    _ => return Err(WireError::StdoutNotExclusive),
                };
                let stderr = match (raw.stderr, raw.stderr_file) {
                    (Some(l), None) => Stderr::Lines(l),
                    (None, Some(f)) => Stderr::File(f),
                    _ => {
                        return Err(WireError::Json(
                            "a cli answer carries exactly one of stderr and stderrFile".into(),
                        ));
                    }
                };
                Outcome::Cli {
                    exit_code: raw
                        .exit_code
                        .ok_or_else(|| WireError::Json("a cli answer has an exitCode".into()))?,
                    stdout,
                    stderr,
                    changed_paths: raw.changed_paths,
                    changed_paths_truncated: raw.changed_paths_truncated,
                }
            }
            "failed" => Outcome::Failed {
                outcome: raw
                    .outcome
                    .ok_or_else(|| WireError::Json("a failed answer has an outcome".into()))?,
                message: raw.message.unwrap_or_default(),
                diagnostic_file: raw.diagnostic_file,
            },
            other => return Err(WireError::Json(format!("unknown kind '{other}'"))),
        };
        Ok(Answer {
            id: raw.id,
            outcome,
            revision: raw.revision,
        })
    }
}

/// A job's held writes as a `kind: "cli"` outcome: its stdout inline when it
/// is UTF-8, at most `inline_cap` bytes and small enough escaped for one
/// frame; else written by `spill` (which returns where). Its stderr as lines
/// (each without its newline). A job that wrote a file (`--out`) is refused:
/// the server's output sink handles those.
pub fn cli_outcome(
    out: &JobOutput,
    inline_cap: usize,
    spill: impl FnOnce(&[u8]) -> std::io::Result<SpillFile>,
) -> Result<Outcome, WireError> {
    let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
    for a in &out.actions {
        match a {
            WriteAction::Stdout(b) => stdout.extend_from_slice(b),
            WriteAction::Stderr(b) => stderr.extend_from_slice(b),
            WriteAction::File(p, _) | WriteAction::Dir(p) => {
                return Err(WireError::FileWrite(p.display().to_string()));
            }
        }
    }
    let inline = match String::from_utf8(stdout) {
        Ok(text) if text.len() <= inline_cap && escaped_len(&text) <= frame_budget() => {
            Ok(Stdout::Text(text))
        }
        Ok(text) => Err(text.into_bytes()),
        Err(e) => Err(e.into_bytes()),
    };
    let stdout = match inline {
        Ok(s) => s,
        Err(bytes) => Stdout::File(spill(&bytes).map_err(|e| WireError::Spill(e.to_string()))?),
    };
    Ok(Outcome::Cli {
        exit_code: out.exit,
        stdout,
        stderr: Stderr::Lines(stderr_lines(&stderr)),
        changed_paths: Vec::new(),
        changed_paths_truncated: false,
    })
}

/// Stderr bytes as lines, each without its newline (lossy UTF-8).
#[must_use]
pub fn stderr_lines(stderr: &[u8]) -> Vec<String> {
    let stderr = String::from_utf8_lossy(stderr);
    stderr
        .strip_suffix('\n')
        .unwrap_or(&stderr)
        .split('\n')
        .filter(|_| !stderr.is_empty())
        .map(str::to_string)
        .collect()
}

/// The bytes the lines stand for: each line and its newline.
#[must_use]
pub fn stderr_bytes(lines: &[String]) -> Vec<u8> {
    let mut out = Vec::new();
    for l in lines {
        out.extend_from_slice(l.as_bytes());
        out.push(b'\n');
    }
    out
}

/// The bytes `text` takes as a JSON string, quotes included.
fn escaped_len(text: &str) -> usize {
    serde_json::to_string(text).map_or(usize::MAX, |s| s.len())
}

/// The most an escaped stdout may take of one frame.
fn frame_budget() -> usize {
    MAX_FRAME_BYTES - FRAME_HEADROOM
}

/// `message` as one frame: compact JSON and its newline, at most
/// [`MAX_FRAME_BYTES`].
pub fn encode_frame<T: Serialize>(message: &T) -> Result<Vec<u8>, WireError> {
    encode_frame_within(message, MAX_FRAME_BYTES)
}

/// [`encode_frame`] against `limit` instead of [`MAX_FRAME_BYTES`].
pub fn encode_frame_within<T: Serialize>(message: &T, limit: usize) -> Result<Vec<u8>, WireError> {
    let mut bytes = serde_json::to_vec(message).map_err(|e| WireError::Json(e.to_string()))?;
    bytes.push(b'\n');
    if bytes.len() > limit {
        return Err(WireError::FrameTooLarge(bytes.len()));
    }
    debug_assert_eq!(
        bytes.iter().filter(|&&b| b == b'\n').count(),
        1,
        "compact JSON holds no raw newline"
    );
    Ok(bytes)
}

/// `text` cut to at most `max` bytes, on a character boundary.
fn cut(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// `answer` inside `wrap` (the transport's envelope around the answer: the
/// engine protocol's JSON-RPC response, or an MCP result) as ONE frame of at
/// most `limit` bytes (r5 §3). A `kind: "failed"` message over
/// [`MESSAGE_INLINE_BYTES`] is always cut, its whole text spilled. Then, while
/// the frame is over the limit, in order: (1) `stdoutText` → `stdoutFile`;
/// (2) `stderr` → `stderrFile` (the exact bytes, a line and its newline per
/// entry); (3) `changedPaths` cut to [`CHANGED_PATHS_KEPT`] with
/// `changedPathsTruncated`; (4) the terminal fallback: `kind: "failed"`,
/// outcome `protocol`, "answer exceeded the frame limit", with the whole
/// encoded answer as its `diagnosticFile`. A spill that fails ends in the
/// terminal fallback carrying the write error. The result always fits.
pub fn bound_frame(
    answer: Answer,
    limit: usize,
    wrap: &dyn Fn(&Answer) -> serde_json::Value,
    spill: &Spill<'_>,
) -> Vec<u8> {
    let encode = |a: &Answer| encode_frame_within(&wrap(a), limit);
    let id = answer.id.clone();
    let revision = answer.revision.clone();
    let terminal = |message: String, diagnostic_file: Option<SpillFile>| {
        let minimal = Answer {
            id: id.clone(),
            outcome: Outcome::Failed {
                outcome: FailedOutcome::Protocol,
                message: cut(&message, MESSAGE_INLINE_BYTES).to_string(),
                diagnostic_file,
            },
            revision: revision.clone(),
        };
        encode(&minimal).unwrap_or_else(|_| {
            // Not even the minimal frame fits (an id or a limit that leaves
            // no room): the same answer with an empty message and a null id.
            let bare = Answer {
                id: serde_json::Value::Null,
                outcome: Outcome::failed(FailedOutcome::Protocol, ""),
                revision: None,
            };
            let mut b = serde_json::to_vec(&wrap(&bare)).unwrap_or_default();
            b.push(b'\n');
            b
        })
    };
    let mut answer = answer;
    // The message rule holds whatever the frame size.
    if let Outcome::Failed {
        message,
        diagnostic_file,
        ..
    } = &mut answer.outcome
        && message.len() > MESSAGE_INLINE_BYTES
    {
        match spill(SpillKind::Diagnostic, message.as_bytes()) {
            Ok(f) => {
                *message = cut(message, MESSAGE_INLINE_BYTES).to_string();
                *diagnostic_file = Some(f);
            }
            Err(e) => return terminal(format!("spilling the message failed: {e}"), None),
        }
    }
    for step in 0..4 {
        match encode(&answer) {
            Ok(frame) => return frame,
            Err(WireError::FrameTooLarge(_)) => {}
            Err(e) => return terminal(format!("encoding the answer failed: {e}"), None),
        }
        let Outcome::Cli {
            stdout,
            stderr,
            changed_paths,
            changed_paths_truncated,
            ..
        } = &mut answer.outcome
        else {
            break;
        };
        let spilled = match step {
            0 => match stdout {
                Stdout::Text(t) => spill(SpillKind::Stdout, t.as_bytes()).map(|f| {
                    *stdout = Stdout::File(f);
                }),
                Stdout::File(_) => Ok(()),
            },
            1 => match stderr {
                Stderr::Lines(l) => spill(SpillKind::Stderr, &stderr_bytes(l)).map(|f| {
                    *stderr = Stderr::File(f);
                }),
                Stderr::File(_) => Ok(()),
            },
            2 => {
                if changed_paths.len() > CHANGED_PATHS_KEPT {
                    changed_paths.truncate(CHANGED_PATHS_KEPT);
                    *changed_paths_truncated = true;
                }
                Ok(())
            }
            _ => Ok(()),
        };
        if let Err(e) = spilled {
            return terminal(
                format!("answer exceeded the frame limit; spill failed: {e}"),
                None,
            );
        }
    }
    let whole = serde_json::to_vec(&answer).unwrap_or_default();
    match spill(SpillKind::Diagnostic, &whole) {
        Ok(f) => terminal("answer exceeded the frame limit".to_string(), Some(f)),
        Err(e) => terminal(
            format!("answer exceeded the frame limit; spill failed: {e}"),
            None,
        ),
    }
}

/// One frame (with or without its newline) decoded as `T`.
pub fn decode_frame<T: DeserializeOwned>(frame: &[u8]) -> Result<T, WireError> {
    if frame.len() > MAX_FRAME_BYTES {
        return Err(WireError::FrameTooLarge(frame.len()));
    }
    let body = frame.strip_suffix(b"\n").unwrap_or(frame);
    if body.contains(&b'\n') {
        return Err(WireError::NotOneLine);
    }
    serde_json::from_slice(body).map_err(|e| WireError::Json(e.to_string()))
}

/// The next frame of `reader`, its newline kept; `None` at end of input. A
/// line longer than [`MAX_FRAME_BYTES`] is an error, read (and held) no
/// further than the limit plus one byte: an oversized frame is refused
/// before it is buffered whole. The rest of that line stays unread
/// ([`skip_line`] discards it without holding it).
pub fn read_frame(reader: &mut impl BufRead) -> Result<Option<Vec<u8>>, WireError> {
    let mut line = Vec::new();
    let read = std::io::Read::take(&mut *reader, MAX_FRAME_BYTES as u64 + 1)
        .read_until(b'\n', &mut line)
        .map_err(|e| WireError::Io(e.to_string()))?;
    if read == 0 {
        return Ok(None);
    }
    if line.len() > MAX_FRAME_BYTES {
        return Err(WireError::FrameTooLarge(line.len()));
    }
    Ok(Some(line))
}

/// Discard the rest of the current line (after [`read_frame`] refused it),
/// a buffer at a time, holding none of it. Returns the bytes skipped.
pub fn skip_line(reader: &mut impl BufRead) -> std::io::Result<u64> {
    let mut skipped = 0u64;
    loop {
        let (done, used) = {
            let buf = reader.fill_buf()?;
            if buf.is_empty() {
                return Ok(skipped);
            }
            match buf.iter().position(|&b| b == b'\n') {
                Some(i) => (true, i + 1),
                None => (false, buf.len()),
            }
        };
        reader.consume(used);
        skipped += used as u64;
        if done {
            return Ok(skipped);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A pretty CLI document: embedded newlines, indentation, a tab and a
    /// quote escaped inside a string, non-ASCII text, a control character,
    /// and the trailing newline every CLI JSON output ends with.
    const PRETTY: &str = "{\n  \"cards\": [\n    {\n      \"name\": \"Post \\\"Order\\\"\\tæøå ✓\"\n    }\n  ],\n  \"raw\": \"\u{1}\"\n}\n";

    fn cli(stdout: &str) -> JobOutput {
        JobOutput {
            actions: vec![
                WriteAction::Stderr(b"warn: one\n".to_vec()),
                WriteAction::Stdout(stdout.as_bytes().to_vec()),
                WriteAction::Stderr(b"warn: two\n".to_vec()),
            ],
            exit: 0,
        }
    }

    fn no_spill(_: &[u8]) -> std::io::Result<SpillFile> {
        panic!("inline expected")
    }

    /// T-wire (codec): a CLI answer's stdout round-trips through a frame byte
    /// for byte, embedded newlines and the trailing newline included, and the
    /// frame is ONE line.
    #[test]
    fn stdout_text_round_trips_exact_bytes_in_one_line() {
        let outcome = cli_outcome(&cli(PRETTY), DEFAULT_INLINE_BYTES, no_spill).unwrap();
        let answer = Answer {
            id: serde_json::json!(7),
            outcome,
            revision: None,
        };
        let frame = encode_frame(&answer).unwrap();
        assert_eq!(frame.iter().filter(|&&b| b == b'\n').count(), 1);
        assert_eq!(frame.last(), Some(&b'\n'));
        let back: Answer = decode_frame(&frame).unwrap();
        assert_eq!(back, answer);
        let Outcome::Cli {
            stdout: Stdout::Text(text),
            stderr,
            exit_code,
            ..
        } = back.outcome
        else {
            panic!("a cli answer with inline stdout")
        };
        assert_eq!(text.as_bytes(), PRETTY.as_bytes(), "exact bytes");
        assert!(text.ends_with("}\n"), "the trailing newline is kept");
        assert_eq!(
            stderr,
            Stderr::Lines(vec!["warn: one".into(), "warn: two".into()])
        );
        assert_eq!(exit_code, 0);
        // On the wire: a JSON string, never the document embedded raw.
        let raw: serde_json::Value = serde_json::from_slice(&frame).unwrap();
        assert_eq!(raw["kind"], "cli");
        assert!(raw["stdoutText"].is_string());
        assert!(raw.get("stdoutFile").is_none());
        assert!(raw["revision"].is_null(), "revision: null is sent");
    }

    /// Several frames back to back read back one per line by real line
    /// framing, each decoding to what was sent.
    #[test]
    fn ndjson_frames_read_back_one_per_line() {
        let answers: Vec<Answer> = (0..3)
            .map(|i| Answer {
                id: serde_json::json!(format!("r{i}")),
                outcome: cli_outcome(&cli(&PRETTY.repeat(i + 1)), MAX_INLINE_BYTES, no_spill)
                    .unwrap(),
                revision: Some(RevisionId {
                    engine_instance_id: "e".into(),
                    generation: i as u64 + 1,
                    inputs_stamp: "al-sem-manifest-v1:00".into(),
                }),
            })
            .collect();
        let mut stream = Vec::new();
        for a in &answers {
            stream.extend(encode_frame(a).unwrap());
        }
        let mut reader = std::io::BufReader::new(&stream[..]);
        let mut got = Vec::new();
        while let Some(frame) = read_frame(&mut reader).unwrap() {
            got.push(decode_frame::<Answer>(&frame).unwrap());
        }
        assert_eq!(got, answers);
    }

    /// `kind: "cli"` carries exactly one of `stdoutText` / `stdoutFile`; a
    /// failed answer carries its outcome.
    #[test]
    fn stdout_text_xor_file() {
        let both = br#"{"id":1,"kind":"cli","exitCode":0,"stdoutText":"x","stdoutFile":{"path":"p","bytes":1,"sha256":"s"},"stderr":[],"revision":null}"#;
        let neither = br#"{"id":1,"kind":"cli","exitCode":0,"stderr":[],"revision":null}"#;
        for frame in [&both[..], &neither[..]] {
            let err = decode_frame::<Answer>(frame).unwrap_err();
            assert!(
                matches!(err, WireError::Json(ref m) if m.contains("exactly one")),
                "{err:?}"
            );
        }
        let failed = br#"{"id":1,"kind":"failed","outcome":"inputsUnstable","message":"no revision","revision":null}"#;
        let a: Answer = decode_frame(failed).unwrap();
        assert_eq!(
            a.outcome,
            Outcome::failed(FailedOutcome::InputsUnstable, "no revision")
        );
        assert_eq!(a.revision, None);
        let no_outcome = br#"{"id":1,"kind":"failed","message":"x","revision":null}"#;
        assert!(decode_frame::<Answer>(no_outcome).is_err());
    }

    /// The inline cap is clamped to 4 MiB; a stdout over the cap, one that is
    /// not UTF-8, and one under the cap whose escaped form would not fit a
    /// frame (control characters escape to six bytes each) all go to a file
    /// with their exact length and hash.
    #[test]
    fn large_or_binary_stdout_goes_to_a_file() {
        assert_eq!(clamp_inline(None), DEFAULT_INLINE_BYTES);
        assert_eq!(clamp_inline(Some(10)), 10);
        assert_eq!(clamp_inline(Some(u64::MAX)), MAX_INLINE_BYTES);
        let spilled = |stdout: Vec<u8>, cap: usize| {
            let out = JobOutput {
                actions: vec![WriteAction::Stdout(stdout.clone())],
                exit: 0,
            };
            let mut seen = None;
            let o = cli_outcome(&out, cap, |b| {
                seen = Some(b.to_vec());
                Ok(SpillFile::of("f".into(), b))
            })
            .unwrap();
            let Outcome::Cli {
                stdout: Stdout::File(f),
                ..
            } = o
            else {
                panic!("a file expected")
            };
            assert_eq!(seen.as_deref(), Some(&stdout[..]), "the exact bytes");
            assert_eq!(f.bytes, stdout.len() as u64);
            assert_eq!(f.sha256, crate::engine::ids::sha256_bytes_hex(&stdout));
        };
        spilled(
            vec![b'a'; MAX_INLINE_BYTES + 1],
            clamp_inline(Some(u64::MAX)),
        );
        spilled(vec![0xff, 0xfe, b'\n'], DEFAULT_INLINE_BYTES);
        spilled(vec![1u8; 3 * 1024 * 1024], MAX_INLINE_BYTES);
        // The same length of plain text stays inline.
        let plain = JobOutput {
            actions: vec![WriteAction::Stdout(vec![b'a'; 3 * 1024 * 1024])],
            exit: 0,
        };
        assert!(matches!(
            cli_outcome(&plain, MAX_INLINE_BYTES, no_spill).unwrap(),
            Outcome::Cli {
                stdout: Stdout::Text(_),
                ..
            }
        ));
    }

    /// A frame above 16 MiB is refused on both sides, and a job that wrote a
    /// file is not a wire answer. A refused line is skipped without holding
    /// it, and the next frame reads.
    #[test]
    fn frame_limit_and_file_writes() {
        let big = "x".repeat(MAX_FRAME_BYTES);
        assert!(matches!(
            encode_frame(&big),
            Err(WireError::FrameTooLarge(_))
        ));
        let mut line = vec![b'"'; MAX_FRAME_BYTES + 10];
        line.push(b'\n');
        line.extend_from_slice(b"{\"next\":1}\n");
        let mut reader = std::io::BufReader::new(&line[..]);
        assert!(matches!(
            read_frame(&mut reader),
            Err(WireError::FrameTooLarge(n)) if n == MAX_FRAME_BYTES + 1
        ));
        assert_eq!(skip_line(&mut reader).unwrap(), 10);
        let next = read_frame(&mut reader).unwrap().unwrap();
        assert_eq!(next, b"{\"next\":1}\n");
        assert_eq!(
            decode_frame::<serde_json::Value>(b"{\"a\":\n1}\n"),
            Err(WireError::NotOneLine)
        );
        let out = JobOutput {
            actions: vec![WriteAction::File("o.json".into(), b"{}".to_vec())],
            exit: 0,
        };
        assert_eq!(
            cli_outcome(&out, DEFAULT_INLINE_BYTES, no_spill),
            Err(WireError::FileWrite("o.json".into()))
        );
    }

    /// A spill sink that records what it was given, in memory.
    struct Sink(std::cell::RefCell<Vec<(SpillKind, Vec<u8>)>>);

    impl Sink {
        fn new() -> Sink {
            Sink(std::cell::RefCell::new(Vec::new()))
        }
        fn spill(&self, kind: SpillKind, bytes: &[u8]) -> std::io::Result<SpillFile> {
            let mut v = self.0.borrow_mut();
            v.push((kind, bytes.to_vec()));
            Ok(SpillFile::of(format!("spill{}", v.len()), bytes))
        }
        fn kinds(&self) -> Vec<SpillKind> {
            self.0.borrow().iter().map(|(k, _)| *k).collect()
        }
    }

    fn rpc(a: &Answer) -> serde_json::Value {
        serde_json::json!({"jsonrpc": "2.0", "id": a.id, "result": a})
    }

    fn cli_answer(stdout: &str, stderr: Vec<String>, changed: usize) -> Answer {
        Answer {
            id: serde_json::json!(1),
            outcome: Outcome::Cli {
                exit_code: 0,
                stdout: Stdout::Text(stdout.into()),
                stderr: Stderr::Lines(stderr),
                changed_paths: (0..changed).map(|i| format!("src/f{i}.al")).collect(),
                changed_paths_truncated: false,
            },
            revision: None,
        }
    }

    fn decoded(frame: &[u8]) -> Answer {
        let v: serde_json::Value = decode_frame(frame).unwrap();
        serde_json::from_value(v["result"].clone()).unwrap()
    }

    /// T-frame (r5 §3): the spill order. A frame that fits is sent as is;
    /// over the limit, stdout spills first; then stderr (its exact bytes);
    /// then changed paths are cut to 256; and an answer still too large ends
    /// in the minimal terminal frame whose diagnostic file holds the whole
    /// answer. Every result fits the limit.
    #[test]
    fn bound_frame_spills_in_order() {
        let limit = 4096;
        let small = cli_answer("ok\n", vec!["w".into()], 0);
        let sink = Sink::new();
        let f = bound_frame(small.clone(), limit, &rpc, &|k, b| sink.spill(k, b));
        assert_eq!(decoded(&f), small);
        assert!(sink.kinds().is_empty());

        // stdout alone is too big: only stdout spills.
        let sink = Sink::new();
        let big_out = "x".repeat(limit);
        let f = bound_frame(
            cli_answer(&big_out, vec!["w".into()], 0),
            limit,
            &rpc,
            &|k, b| sink.spill(k, b),
        );
        assert!(f.len() <= limit);
        assert_eq!(sink.kinds(), [SpillKind::Stdout]);
        assert_eq!(sink.0.borrow()[0].1, big_out.as_bytes());

        // stderr too: then stderr, as its exact bytes.
        let sink = Sink::new();
        let lines: Vec<String> = (0..600).map(|i| format!("warning {i}")).collect();
        let f = bound_frame(
            cli_answer(&big_out, lines.clone(), 0),
            limit,
            &rpc,
            &|k, b| sink.spill(k, b),
        );
        assert!(f.len() <= limit);
        assert_eq!(sink.kinds(), [SpillKind::Stdout, SpillKind::Stderr]);
        assert_eq!(sink.0.borrow()[1].1, stderr_bytes(&lines));
        let Outcome::Cli { stderr, .. } = decoded(&f).outcome else {
            panic!("cli")
        };
        assert!(matches!(stderr, Stderr::File(_)));

        // Changed paths: cut to 256 and flagged.
        let sink = Sink::new();
        // 1,000 paths (~15 KB) do not fit 8 KiB; 256 of them (~4 KB) do.
        let f = bound_frame(
            cli_answer(&big_out, lines.clone(), 1000),
            8 * 1024,
            &rpc,
            &|k, b| sink.spill(k, b),
        );
        assert!(f.len() <= 8 * 1024);
        let Outcome::Cli {
            changed_paths,
            changed_paths_truncated,
            ..
        } = decoded(&f).outcome
        else {
            panic!("cli")
        };
        assert_eq!((changed_paths.len(), changed_paths_truncated), (256, true));

        // Still too big after every step (256 paths do not fit 2 KiB): the
        // terminal fallback, the whole answer in its diagnostic file.
        let (sink, limit) = (Sink::new(), 2048);
        let f = bound_frame(cli_answer(&big_out, lines, 300), limit, &rpc, &|k, b| {
            sink.spill(k, b)
        });
        assert!(f.len() <= limit);
        let Outcome::Failed {
            outcome,
            message,
            diagnostic_file,
        } = decoded(&f).outcome
        else {
            panic!("the terminal fallback")
        };
        assert_eq!(outcome, FailedOutcome::Protocol);
        assert_eq!(message, "answer exceeded the frame limit");
        let whole = sink.0.borrow().last().unwrap().1.clone();
        assert_eq!(diagnostic_file.unwrap().bytes, whole.len() as u64);
        let back: Answer = serde_json::from_slice(&whole).unwrap();
        assert!(matches!(back.outcome, Outcome::Cli { .. }));
    }

    /// r5 §8.3: a failed answer's message is at most 4 KiB inline, the whole
    /// text in its diagnostic file, at any frame size; a spill that fails
    /// ends in the terminal fallback carrying the write error.
    #[test]
    fn a_long_failed_message_is_cut_and_spilled() {
        let long = "é".repeat(5000); // 10,000 bytes, two per character
        let answer = Answer {
            id: serde_json::json!("a"),
            outcome: Outcome::failed(FailedOutcome::InputsUnstable, long.clone()),
            revision: None,
        };
        let sink = Sink::new();
        let f = bound_frame(answer.clone(), MAX_FRAME_BYTES, &rpc, &|k, b| {
            sink.spill(k, b)
        });
        let Outcome::Failed {
            outcome,
            message,
            diagnostic_file,
        } = decoded(&f).outcome
        else {
            panic!("failed")
        };
        assert_eq!(outcome, FailedOutcome::InputsUnstable);
        assert_eq!(message.len(), MESSAGE_INLINE_BYTES);
        assert!(long.starts_with(&message));
        assert_eq!(diagnostic_file.unwrap().bytes, long.len() as u64);
        assert_eq!(sink.kinds(), [SpillKind::Diagnostic]);

        let f = bound_frame(answer, MAX_FRAME_BYTES, &rpc, &|_, _| {
            Err(std::io::Error::other("disk full"))
        });
        let Outcome::Failed {
            outcome,
            message,
            diagnostic_file,
        } = decoded(&f).outcome
        else {
            panic!("failed")
        };
        assert_eq!(outcome, FailedOutcome::Protocol);
        assert!(message.contains("disk full"), "{message}");
        assert_eq!(diagnostic_file, None);
    }
}

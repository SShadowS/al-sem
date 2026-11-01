//! The exact-output wire codec of the context server (context-server design
//! r3 §3.2, §4.3 and §13 item 2; review r1 #8): one module for the engine
//! protocol and the MCP side, a library with no process of its own yet.
//!
//! **Frames.** NDJSON: one JSON object per line, UTF-8, compact (`serde_json`
//! escapes every newline inside a string, so a frame holds exactly one `\n`,
//! its terminator), at most [`MAX_FRAME_BYTES`] including that newline.
//!
//! **A CLI answer's stdout** travels as the exact bytes, never re-serialized:
//! inline as `stdoutText`, a JSON **string** whose decoded value is the stdout
//! byte for byte (embedded newlines and the trailing newline included), or,
//! when it is not UTF-8, larger than the request's inline cap or too large
//! once escaped for one frame, as `stdoutFile` (a file the engine wrote, with
//! its length and SHA-256). Exactly one of the two (`kind: "cli"`). Embedding
//! the CLI's pretty JSON as raw JSON (`RawValue`) was rejected: its literal
//! newlines break the line framing, and re-serializing a parsed value does not
//! keep its bytes. A structured projection of the output, if a front end adds
//! one, is a separate field and is not byte-identical.
//!
//! **Outcomes.** `kind: "cli"` carries the command's exit code, its stdout and
//! its stderr lines; `kind: "failed"` carries why no command answered (a
//! refused request, a build that failed before any revision existed). Either
//! may name the revision that answered, or `null` when none exists.

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

/// Room a frame keeps for everything but the escaped stdout (the envelope's
/// other fields and the stderr lines are checked again at [`encode_frame`]).
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

/// A stdout the engine wrote to a file instead of inlining it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StdoutFile {
    pub path: String,
    pub bytes: u64,
    pub sha256: String,
}

impl StdoutFile {
    /// The description of `bytes` written at `path`.
    #[must_use]
    pub fn of(path: String, bytes: &[u8]) -> StdoutFile {
        StdoutFile {
            path,
            bytes: bytes.len() as u64,
            sha256: crate::engine::ids::sha256_bytes_hex(bytes),
        }
    }
}

/// A CLI answer's stdout: exactly one form.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Stdout {
    /// The exact bytes, valid UTF-8, inline.
    Text(String),
    /// The exact bytes, in a file.
    File(StdoutFile),
}

/// The revision that answered (r3 §4.7).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RevisionId {
    pub engine_instance_id: String,
    pub generation: u64,
    pub inputs_stamp: String,
}

/// What answered a request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The command ran: its exit, its stdout, its stderr lines.
    Cli {
        exit_code: u8,
        stdout: Stdout,
        stderr: Vec<String>,
    },
    /// No command answered, and why.
    Failed { error: String, stderr: Vec<String> },
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
    stdout_file: Option<StdoutFile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    stderr: Vec<String>,
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
            error: None,
            stderr: Vec::new(),
            revision: a.revision,
        };
        match a.outcome {
            Outcome::Cli {
                exit_code,
                stdout,
                stderr,
            } => {
                raw.kind = "cli".into();
                raw.exit_code = Some(exit_code);
                match stdout {
                    Stdout::Text(t) => raw.stdout_text = Some(t),
                    Stdout::File(f) => raw.stdout_file = Some(f),
                }
                raw.stderr = stderr;
            }
            Outcome::Failed { error, stderr } => {
                raw.kind = "failed".into();
                raw.error = Some(error);
                raw.stderr = stderr;
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
                Outcome::Cli {
                    exit_code: raw
                        .exit_code
                        .ok_or_else(|| WireError::Json("a cli answer has an exitCode".into()))?,
                    stdout,
                    stderr: raw.stderr,
                }
            }
            "failed" => Outcome::Failed {
                error: raw.error.unwrap_or_default(),
                stderr: raw.stderr,
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
    spill: impl FnOnce(&[u8]) -> std::io::Result<StdoutFile>,
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
    let stderr = String::from_utf8_lossy(&stderr);
    let stderr = stderr
        .strip_suffix('\n')
        .unwrap_or(&stderr)
        .split('\n')
        .filter(|_| !stderr.is_empty())
        .map(str::to_string)
        .collect();
    Ok(Outcome::Cli {
        exit_code: out.exit,
        stdout,
        stderr,
    })
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
    let mut bytes = serde_json::to_vec(message).map_err(|e| WireError::Json(e.to_string()))?;
    bytes.push(b'\n');
    if bytes.len() > MAX_FRAME_BYTES {
        return Err(WireError::FrameTooLarge(bytes.len()));
    }
    debug_assert_eq!(
        bytes.iter().filter(|&&b| b == b'\n').count(),
        1,
        "compact JSON holds no raw newline"
    );
    Ok(bytes)
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
/// line longer than [`MAX_FRAME_BYTES`] is an error, read no further than the
/// limit.
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

    fn no_spill(_: &[u8]) -> std::io::Result<StdoutFile> {
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
        } = back.outcome
        else {
            panic!("a cli answer with inline stdout")
        };
        assert_eq!(text.as_bytes(), PRETTY.as_bytes(), "exact bytes");
        assert!(text.ends_with("}\n"), "the trailing newline is kept");
        assert_eq!(stderr, ["warn: one", "warn: two"]);
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

    /// `kind: "cli"` carries exactly one of `stdoutText` / `stdoutFile`.
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
        let failed =
            br#"{"id":1,"kind":"failed","error":"no revision","stderr":[],"revision":null}"#;
        let a: Answer = decode_frame(failed).unwrap();
        assert_eq!(
            a.outcome,
            Outcome::Failed {
                error: "no revision".into(),
                stderr: vec![]
            }
        );
        assert_eq!(a.revision, None);
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
                Ok(StdoutFile::of("f".into(), b))
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
    /// file is not a wire answer.
    #[test]
    fn frame_limit_and_file_writes() {
        let big = "x".repeat(MAX_FRAME_BYTES);
        assert!(matches!(
            encode_frame(&big),
            Err(WireError::FrameTooLarge(_))
        ));
        let mut line = vec![b'"'; MAX_FRAME_BYTES + 10];
        line.push(b'\n');
        let mut reader = std::io::BufReader::new(&line[..]);
        assert!(matches!(
            read_frame(&mut reader),
            Err(WireError::FrameTooLarge(_))
        ));
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
}

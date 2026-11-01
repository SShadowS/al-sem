//! The engine child process and its protocol (context-server design r3 §3.2,
//! r4 §3, r5 §3; review r2 #12, r3 #11, r4 #11): the plumbing of stage 2.
//!
//! **Shape.** A front end (an MCP server, an LSP, or `alsem engine-ask`) owns
//! one child, `alsem engine-host`, per workspace and speaks line-framed
//! JSON-RPC 2.0 (NDJSON, the [`crate::wire`] codec) over the child's stdin
//! and stdout. The engine runs in a child because the shipped binaries abort
//! on panic (`release-ship`): a crash kills the child, never the front end,
//! which reports it and restarts the child with backoff. stderr is the
//! child's log, drained and tailed by the front end, never protocol.
//!
//! **What stage 2's child serves.** Each `session/ask` runs the one-shot job
//! path a standalone `alsem <command>` runs (`plan` → `make_job` → the batch
//! driver, `engine::batch`), its writes held, and answers the exact CLI bytes.
//! There is no resident revision yet (`revision: null` on every answer).
//!
//! **Protocol** (one JSON object per line, ≤ [`MAX_FRAME_BYTES`]):
//!
//! | Direction | Message | Shape |
//! |---|---|---|
//! | child → front | `engine/hello` (notification, the child's first line, within [`HELLO_DEADLINE`]) | [`Hello`] |
//! | front → child | `session/open` | [`OpenParams`] → `{ok: true}` |
//! | front → child | `session/ask` | [`AskParams`] → [`crate::wire::Answer`] |
//! | front → child | `session/status` | → [`ChildStatus`] |
//! | front → child | `session/shutdown` | → `{ok: true}`, then the child exits |
//!
//! A protocol version the front end does not support, a missing hello, a
//! frame over the limit or one that does not parse kills the child, and so
//! does output the front end has not read piling up past [`QUEUE_FRAMES`]
//! frames or [`QUEUE_BYTES`] bytes ([`PROGRESS_METHOD`] notifications are
//! coalesced per request first). A request
//! id over [`MAX_ID_BYTES`] is refused at decode. Errors use the JSON-RPC
//! codes in [`codes`].

pub mod child;
pub mod front;

use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};

pub use crate::wire::MAX_FRAME_BYTES;

/// The protocol version this build speaks.
pub const PROTOCOL_VERSION: u32 = 1;

/// The versions a front end of this build accepts in a child's hello.
pub const SUPPORTED_PROTOCOLS: &[u32] = &[1];

/// How long a child has to send its hello after it is spawned.
pub const HELLO_DEADLINE: Duration = Duration::from_secs(10);

/// How much of a child's stderr a front end keeps (the newest bytes).
pub const STDERR_RETAIN: usize = 64 * 1024;

/// How long `shutdown` waits for the child to exit before killing it.
pub const SHUTDOWN_DEADLINE: Duration = Duration::from_secs(10);

/// The first restart delay; it doubles per death up to [`BACKOFF_MAX`].
pub const BACKOFF_INITIAL: Duration = Duration::from_secs(1);

/// The longest restart delay.
pub const BACKOFF_MAX: Duration = Duration::from_secs(30);

/// A child that has run this long past its hello resets the backoff, as a
/// completed `session/ask` does; a hello alone never does.
pub const STABLE_AFTER: Duration = Duration::from_secs(60);

/// This many deaths within [`DEATH_WINDOW`] put the front end in the failed
/// state for [`FAILED_COOLDOWN`] (or until reset).
pub const DEATH_LIMIT: usize = 3;

/// See [`DEATH_LIMIT`].
pub const DEATH_WINDOW: Duration = Duration::from_secs(60);

/// See [`DEATH_LIMIT`].
pub const FAILED_COOLDOWN: Duration = Duration::from_secs(300);

/// The most bytes of the child's output a front end holds queued, unread
/// (design r7 §3, review r6 NEW 5); past it the child is killed (`protocol`).
pub const QUEUE_BYTES: usize = 64 * 1024 * 1024;

/// The most frames of the child's output a front end holds queued, unread;
/// past it the child is killed (`protocol`).
pub const QUEUE_FRAMES: usize = 1024;

/// The progress notification: `params.id` names the request it reports on.
/// A front end keeps only the newest one per request queued.
pub const PROGRESS_METHOD: &str = "engine/progress";

/// The largest request id, as JSON, a child accepts (r5 §3).
pub const MAX_ID_BYTES: usize = 1024;

/// The JSON-RPC 2.0 error codes the protocol uses.
pub mod codes {
    /// A frame that is not JSON, or over the frame limit.
    pub const PARSE_ERROR: i64 = -32700;
    /// A frame that is JSON but no valid request (an id over the limit).
    pub const INVALID_REQUEST: i64 = -32600;
    pub const METHOD_NOT_FOUND: i64 = -32601;
    pub const INVALID_PARAMS: i64 = -32602;
    /// `session/ask` before `session/open`.
    pub const NOT_OPEN: i64 = -32002;
    /// `session/open` refused (no workspace path).
    pub const OPEN_FAILED: i64 = -32001;
}

/// The child's first line.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Hello {
    pub protocol: u32,
    pub engine_instance_id: String,
    pub alsem_version: String,
    /// The committed `node-types.sha256` of the grammar the child was built
    /// against.
    pub grammar_sha: String,
    pub pid: u32,
}

impl Hello {
    /// This process's hello.
    #[must_use]
    pub fn of_this_process() -> Hello {
        Hello {
            protocol: PROTOCOL_VERSION,
            engine_instance_id: engine_instance_id(),
            alsem_version: crate::engine::gate::version::driver_version(),
            grammar_sha: GRAMMAR_SHA.trim().to_string(),
            pid: std::process::id(),
        }
    }
}

/// The grammar identity this build was generated from.
const GRAMMAR_SHA: &str =
    include_str!("../../../crates/al-syntax/src/raw/generated/node-types.sha256");

/// A fresh id for this engine process: its pid and start time, hashed.
fn engine_instance_id() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let seed = format!("{}:{now}", std::process::id());
    crate::engine::ids::sha256_bytes_hex(seed.as_bytes())[..16].to_string()
}

/// `session/open`: the workspace the session serves, and the directory the
/// child writes spilled answers into (created and owned by the front end;
/// the stage-2 seam for the anchored output sink of design r5 §10).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenParams {
    pub workspace: PathBuf,
    pub output_root: PathBuf,
}

/// The front end's answers to a request's file-system questions, decided
/// before it is sent (r3 §4.3: the child probes nothing for them): whether
/// `digest --changed` names a file, and the diff root.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Decisions {
    pub changed_is_file: Option<bool>,
    pub diff_root: Option<Result<crate::engine::l5::digest_cli::DiffRoot, String>>,
}

/// `session/ask`: one CLI command line (without the program name) and what
/// it reads from outside its argv.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AskParams {
    pub argv: Vec<String>,
    /// The stdin a `--diff -` reads; absent: such a request fails as an
    /// unreadable stdin does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diff_text: Option<String>,
    /// The front end's decisions; absent: the child decides through its own
    /// file system (a stage-2 convenience for a client with no decoder).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decisions: Option<Decisions>,
    /// The inline stdout cap (`crate::wire::clamp_inline`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_inline_bytes: Option<u64>,
}

/// `session/status`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChildStatus {
    pub pid: u32,
    pub workspace: Option<PathBuf>,
    /// `session/ask` requests answered.
    pub asks: u64,
    /// Workspace reads (acquisitions) the asks made: a request that fails
    /// its planning makes none.
    pub workspace_reads: u64,
}

/// One JSON-RPC 2.0 message, any of request, notification, response.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub jsonrpc: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}

/// A JSON-RPC error object.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
}

impl Message {
    /// A request.
    #[must_use]
    pub fn request(id: serde_json::Value, method: &str, params: serde_json::Value) -> Message {
        Message {
            jsonrpc: "2.0".into(),
            id: Some(id),
            method: Some(method.into()),
            params: Some(params),
            ..Message::default()
        }
    }

    /// A notification.
    #[must_use]
    pub fn notification(method: &str, params: serde_json::Value) -> Message {
        Message {
            jsonrpc: "2.0".into(),
            method: Some(method.into()),
            params: Some(params),
            ..Message::default()
        }
    }

    /// A successful response.
    #[must_use]
    pub fn response(id: serde_json::Value, result: serde_json::Value) -> Message {
        Message {
            jsonrpc: "2.0".into(),
            id: Some(id),
            result: Some(result),
            ..Message::default()
        }
    }

    /// An error response.
    #[must_use]
    pub fn error(id: serde_json::Value, code: i64, message: impl Into<String>) -> Message {
        Message {
            jsonrpc: "2.0".into(),
            id: Some(id),
            error: Some(RpcError {
                code,
                message: message.into(),
            }),
            ..Message::default()
        }
    }
}

/// The test-only fault hooks of the child (`ALSEM_ENGINE_TEST_HOOK`),
/// compiled into builds with the `engine-test-hooks` feature (so they work
/// under the abort profile, `release-ship`) and into debug builds (so the
/// test suite has them); a shipped binary has none.
#[cfg(any(feature = "engine-test-hooks", debug_assertions))]
pub mod test_hooks {
    /// The environment variable naming the hook.
    pub const ENV: &str = "ALSEM_ENGINE_TEST_HOOK";

    /// A fault the child injects.
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub enum Hook {
        /// Never send the hello (wait for stdin to close).
        NoHello,
        /// Announce this protocol version instead of the real one.
        Protocol(u32),
        /// Abort (as a panic does under `release-ship`) on receiving an ask.
        AbortOnAsk,
        /// Abort on an ask only if `marker` does not exist yet, creating it:
        /// the first child dies, its replacement answers.
        AbortOnAskOnce(std::path::PathBuf),
        /// Answer an ask with a line over the frame limit.
        OversizedFrameOnAsk,
        /// Answer an ask with a line that is not JSON.
        GarbageFrameOnAsk,
        /// After answering `session/open`, write `count` small frames of
        /// `kind` unasked (while the front end reads nothing), then serve on.
        FloodAfterOpen(Flood, usize),
    }

    /// What a flood writes.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Flood {
        /// Valid responses (`{"id":0,"result":{}}`).
        Response,
        /// Valid notifications, each of a different method.
        Notify,
        /// Progress notifications for one request (coalesced by the front).
        Progress,
        /// Short lines that are not JSON.
        Garbage,
    }

    /// The hook the environment names, if any.
    #[must_use]
    pub fn from_env() -> Option<Hook> {
        let raw = std::env::var(ENV).ok()?;
        let (name, arg) = raw.split_once(':').unwrap_or((&raw, ""));
        Some(match name {
            "no-hello" => Hook::NoHello,
            "protocol" => Hook::Protocol(arg.parse().ok()?),
            "abort-on-ask" => Hook::AbortOnAsk,
            "abort-on-ask-once" => Hook::AbortOnAskOnce(arg.into()),
            "oversized-frame-on-ask" => Hook::OversizedFrameOnAsk,
            "garbage-frame-on-ask" => Hook::GarbageFrameOnAsk,
            "flood-after-open" => {
                let (kind, count) = arg.split_once(':')?;
                let kind = match kind {
                    "response" => Flood::Response,
                    "notify" => Flood::Notify,
                    "progress" => Flood::Progress,
                    "garbage" => Flood::Garbage,
                    _ => return None,
                };
                Hook::FloodAfterOpen(kind, count.parse().ok()?)
            }
            _ => return None,
        })
    }
}

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
//! **What the child serves (stage 2).** A resident session
//! (`engine::resident`): the first ask builds a revision of the workspace
//! (cross-app through the stage-4 incremental model by default), later asks
//! are answered off it after a per-ask freshness check, and an edit refreshes
//! it. `explain`, `prove`, `digest` and `query` are answered off the
//! revision's retained artifacts; every other command runs the one-shot job
//! path over the revision's acquired read. Every answer is the CLI's exact
//! bytes and names the revision that answered (`revision: null` only for a
//! planning failure or a failed build).
//!
//! **Protocol** (one JSON object per line, ≤ [`MAX_FRAME_BYTES`]):
//!
//! | Direction | Message | Shape |
//! |---|---|---|
//! | child → front | `engine/hello` (notification, the child's first line, within [`HELLO_DEADLINE`]) | [`Hello`] |
//! | front → child | `session/open` | [`OpenParams`] → `{ok: true, outputDir}` |
//! | front → child | `session/ask` | [`AskParams`] → [`crate::wire::Answer`] |
//! | front → child | `session/cancel` (notification) | [`CancelParams`] |
//! | front → child | `session/notifyChanged` (notification) | [`NotifyChangedParams`] |
//! | front → child | `session/refresh` | [`RefreshParams`] → `{revision, waited}` |
//! | front → child | `session/status` | → [`ChildStatus`] |
//! | front → child | `session/shutdown` | → `{ok: true}` after every queued ask is answered, then the child exits |
//! | child → front | [`PROGRESS_METHOD`] (notification, only when the ask set `progress`) | `{id, phase}` |
//!
//! **Order of an ask** (design §4.3): decode (the CLI's clap) → validate
//! (the CLI's planning with no workspace bound: a planning failure is the
//! CLI's text and exit, 3 included, `revision: null`) → enforce the session's
//! workspace and universe (`notThisWorkspace`) → freshness and pin (one
//! revision for the whole ask) → plan again bound to the pinned read →
//! resolve → `ensure_prepared` → compute → one bounded frame. The child never
//! probes the file system for a request: the diff root and `--changed`'s
//! file-or-list are the front end's [`Decisions`] (mandatory when the
//! command reads paths), request files travel as bytes (`requestFiles`).
//!
//! **Queue.** One heavy worker answers every ask but query (and refreshes)
//! in arrival order, and [`LIGHT_WORKERS`] light workers answer queries
//! beside it (stage 3: a light ask never waits behind a heavy one); at most
//! [`QUEUE_HEAVY`] heavy and [`QUEUE_LIGHT`] light asks (query) wait, holding
//! at most [`QUEUED_BYTES`] of parameters; beyond, `busy` at once. A
//! cancelled queued ask is answered `cancelled` at once with no work; an ask
//! whose `deadlineMs` passed while queued answers `deadline` with no work; an
//! ask in flight reads its flag between phases and tree groups.
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
pub mod output;

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

/// The most heavy asks (explain, prove, digest, everything but query) a
/// child holds waiting (design §3 "Queues").
pub const QUEUE_HEAVY: usize = 16;

/// The most light asks (query) a child holds waiting.
pub const QUEUE_LIGHT: usize = 64;

/// The most bytes of ask parameters a child holds waiting.
pub const QUEUED_BYTES: usize = 16 * 1024 * 1024;

/// The workers that answer light asks (query) beside the one heavy worker
/// (design §4.5 "Lanes: one heavy, four light"), so a light ask never waits
/// behind a heavy one.
pub const LIGHT_WORKERS: usize = 4;

/// The notification a child sends when its idle timeout passed (design §7
/// `--idle-timeout`): it ends after [`IDLE_GRACE`] unless a request arrives
/// first (which it then serves, staying up). A front end that sees it, or
/// sees the child's output end after it, starts a new child for its next
/// request without counting a death.
pub const IDLE_EXIT_METHOD: &str = "engine/idleExit";

/// How long an idle child waits after [`IDLE_EXIT_METHOD`] before it ends.
pub const IDLE_GRACE: Duration = Duration::from_secs(10);

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

/// The committed `node-types.sha256` of the grammar this build links.
#[must_use]
pub fn grammar_sha() -> &'static str {
    GRAMMAR_SHA.trim()
}

/// A fresh id for this engine process: its pid and start time, hashed.
fn engine_instance_id() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let seed = format!("{}:{now}", std::process::id());
    crate::engine::ids::sha256_bytes_hex(seed.as_bytes())[..16].to_string()
}

/// The universe a session serves (one child per workspace and universe).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Universe {
    /// The workspace and the dependencies it requires (the CLI default).
    #[default]
    CrossApp,
    /// The workspace alone (`--single-app`).
    SingleApp,
}

impl Universe {
    /// The wire spelling.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Universe::CrossApp => "crossApp",
            Universe::SingleApp => "singleApp",
        }
    }
}

/// `session/open`: the workspace the session serves (an existing folder),
/// its universe, and the root under which the child creates its private
/// output directory `<outputRoot>/<engineInstanceId>/` (design §10: created
/// owner-only, refused if it exists; the open's result names it as
/// `outputDir`). An ask naming another workspace, or the other universe, is
/// refused `notThisWorkspace`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenParams {
    pub workspace: PathBuf,
    pub output_root: PathBuf,
    /// Absent: `crossApp`.
    #[serde(default)]
    pub universe: Universe,
}

/// The front end's answers to a request's file-system questions, decided
/// before it is sent (design §3, §4.3: the child probes nothing for them):
/// whether `digest --changed` names a file, and the diff root (the design's
/// `diffRootDecided`). Mandatory when the command reads paths (`digest
/// --diff/--file/--changed`, `explain --diff`): such an ask without them is
/// refused `protocol`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Decisions {
    pub changed_is_file: Option<bool>,
    pub diff_root: Option<Result<crate::engine::l5::digest_cli::DiffRoot, String>>,
}

/// `session/ask`: one CLI command line (without the program name) and
/// everything it reads from outside its argv.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AskParams {
    pub argv: Vec<String>,
    /// The stdin a `--diff -` reads; absent: such a request fails as an
    /// unreadable stdin does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diff_text: Option<String>,
    /// The bytes (or read error) of every file the command's planning reads
    /// (`--diff <path>`, a `--changed` diff file, a policy or baseline file)
    /// and every `exists` answer it asks, by the path as the argv spells it:
    /// what the front end's own planning read. The child never reads a
    /// request file itself; one not shipped reads as missing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_files: Option<crate::engine::batch::worker::PlanFiles>,
    /// The front end's decisions (see [`Decisions`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decisions: Option<Decisions>,
    /// `fresh` (absent), `latest-built` or `verify` (design §5.1).
    #[serde(default, skip_serializing_if = "is_fresh")]
    pub freshness: crate::engine::resident::session::Freshness,
    /// Milliseconds from the child's receipt: expired while queued →
    /// `deadline` with no work; while waiting or computing → `deadline` at
    /// the next phase boundary (a shared build or fill goes on for others).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deadline_ms: Option<u64>,
    /// The inline stdout cap (`crate::wire::clamp_inline`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_inline_bytes: Option<u64>,
    /// Send [`PROGRESS_METHOD`] notifications for this ask.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub progress: bool,
}

fn is_fresh(f: &crate::engine::resident::session::Freshness) -> bool {
    *f == crate::engine::resident::session::Freshness::Fresh
}

/// `session/cancel`: the ask to stop. A queued ask is answered `cancelled`
/// at once; one in flight stops at its next phase boundary and answers
/// `cancelled` with `workDoneMs` and `skippedPhases`; an unknown id is
/// ignored.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CancelParams {
    pub id: serde_json::Value,
}

/// `session/notifyChanged`: paths the front end saw change (a watcher):
/// each is hashed again on the next freshness check, whatever its stat says.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotifyChangedParams {
    pub paths: Vec<PathBuf>,
}

/// `session/refresh`: bring the published revision up to date with the
/// disk (a `fresh` pin). `wait: true` answers after it, with the revision;
/// `wait: false` answers `{started: true}` at once and refreshes in queue
/// order.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RefreshParams {
    #[serde(default)]
    pub wait: bool,
}

/// `session/status`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChildStatus {
    pub pid: u32,
    pub workspace: Option<PathBuf>,
    #[serde(default)]
    pub universe: Option<Universe>,
    /// `session/ask` requests answered.
    pub asks: u64,
    /// Workspace reads (acquisitions) the session made: one per build
    /// attempt; a request that fails its planning, or is answered off the
    /// published revision, makes none.
    pub workspace_reads: u64,
    /// The published revision (`null` before the first build, or after a
    /// failed one).
    #[serde(default)]
    pub revision: Option<crate::wire::RevisionId>,
    /// The session's builds and freshness checks.
    #[serde(default)]
    pub session: Option<serde_json::Value>,
    /// The published revision's artifact fills by kind, and its store's
    /// counters.
    #[serde(default)]
    pub fills: Option<serde_json::Value>,
    /// Asks waiting, and the id of the one in flight.
    #[serde(default)]
    pub queued: u64,
    #[serde(default)]
    pub in_flight: Option<serde_json::Value>,
    /// Every ask in flight (the heavy worker's and the light workers').
    #[serde(default)]
    pub in_flight_all: Vec<serde_json::Value>,
    /// The memory budget's counters (stage 3: `retainedBytes`, `reservedBytes`,
    /// `fillWorkers`, `overruns`, ...).
    #[serde(default)]
    pub memory: Option<serde_json::Value>,
    /// The idle timeout in seconds (`None`: never).
    #[serde(default)]
    pub idle_timeout_secs: Option<u64>,
    /// The private output directory.
    #[serde(default)]
    pub output_dir: Option<PathBuf>,
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
        /// Panic (a real Rust panic, not an abort) on an ask only if `marker`
        /// does not exist yet, creating it: under the abort profile the panic
        /// aborts; under an unwinding one the worker's guard ends the
        /// process. Either way the child dies and its replacement answers.
        PanicOnAskOnce(std::path::PathBuf),
        /// Sleep this many milliseconds before answering an ask (then
        /// answer it normally, its cancel flag read at its first phase).
        SleepOnAsk(u64),
        /// Answer an ask with a line over the frame limit.
        OversizedFrameOnAsk,
        /// Answer an ask with a line that is not JSON.
        GarbageFrameOnAsk,
        /// After answering `session/open`, write `count` small frames of
        /// `kind` unasked (while the front end reads nothing), then serve on.
        FloodAfterOpen(Flood, usize),
        /// After answering `session/open`, exit without reading another
        /// byte: the front end's next request write meets a closed pipe.
        ExitAfterOpen,
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
            "panic-on-ask-once" => Hook::PanicOnAskOnce(arg.into()),
            "sleep-on-ask" => Hook::SleepOnAsk(arg.parse().ok()?),
            "oversized-frame-on-ask" => Hook::OversizedFrameOnAsk,
            "garbage-frame-on-ask" => Hook::GarbageFrameOnAsk,
            "exit-after-open" => Hook::ExitAfterOpen,
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

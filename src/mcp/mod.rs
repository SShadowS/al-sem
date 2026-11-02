//! `alsem mcp`: the MCP front end of the context server (context-server
//! design r7 §8, §3, §10).
//!
//! An agent host (Claude Code and others) starts `alsem mcp` and speaks MCP
//! to it over stdio: JSON-RPC 2.0, one JSON object per line (NDJSON); the
//! server's logs go to stderr only. Each tool call becomes the command line
//! the CLI would take ([`tools`]), which an engine child (`alsem
//! engine-host`, one per workspace and universe, owned through
//! [`crate::engine::host::front::EngineFront`]) parses with the CLI's own
//! clap and answers with the CLI's exact bytes. The front end decides the
//! request's file-system questions itself (the diff root, whether `digest
//! --changed` names a file) through [`Decide`], and sends a diff's text in
//! the protocol (`--diff -` plus `diffText`), so the child never reads a
//! request file.
//!
//! **Lifecycle (§8.1).** `Uninitialized` accepts `initialize`, `ping` and
//! `server/discover` (always answered `-32601`: Claude Code probes it and
//! falls back to `initialize`); any other request is `-32002`.
//! `initialize` answers the client's protocol version when it is in
//! [`SUPPORTED_PROTOCOLS`], else [`DEFAULT_PROTOCOL`] (a missing one too),
//! and moves to `Initializing`; `notifications/initialized` moves to
//! `Ready`, where `tools/list` and `tools/call` are served. A second
//! `initialize` is `-32600`. Ids are numbers or strings; `null`, another
//! type, or an id still in flight is `-32600`. Progress notifications are
//! sent only for a call with `_meta.progressToken`, and carry it.
//!
//! **Results (§8.3, §8.4).** A tool call's result is the envelope: `kind:
//! "cli"` (the command ran, failed its planning, or its model did not build
//! — a semantic build failure is the command's own answer, `isError:
//! false`) with `exitCode`, `stdoutText` XOR `stdoutFile`, `stderr` XOR
//! `stderrFile`, `stale`, `changedPaths`, `universe`, `freshness`,
//! `timing`, `revision` (always present, `null` until the child is
//! resident); or `kind: "failed"` (`isError: true`), an infrastructure
//! outcome (`engineError`, `refusedPath`, `busy`, `protocol`, ...). The
//! envelope's JSON is the first text block (with `stdoutText` replaced by a
//! pointer), the inline stdout is the second text block exactly, and with a
//! negotiated `2025-06-18` the envelope is also `structuredContent` (with
//! `output`, the CLI's JSON parsed, when the stdout is inline JSON).
//! Explain defaults to the human card; digest always writes a file and
//! answers a summary; a stdout over `maxInlineBytes` (default 64 KiB) goes
//! to a file. Every file goes into the server's private output folder
//! ([`output`]).
//!
//! **What the child does not do yet** (stage-2 completion, built in
//! parallel): keep a revision between asks (every answer is a cold build,
//! `revision: null`), freshness modes (`latest-built` and `verify` are
//! served fresh, and the envelope says so), cancellation, deadlines.
//! The front end needs no change when the child becomes resident.

pub mod output;
mod server;
pub mod tools;

use std::path::PathBuf;

pub use server::serve;

use crate::engine::host::Decisions;

/// The protocol versions the server speaks, newest first.
pub const SUPPORTED_PROTOCOLS: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];

/// The version answered to a client that asks for none or an unsupported one.
pub const DEFAULT_PROTOCOL: &str = "2025-06-18";

/// The most calls queued for one engine; past it a call is `busy`
/// (design r7 §3, heavy depth).
pub const WORKER_QUEUE_DEPTH: usize = 16;

/// The server's `instructions` in its `initialize` result.
pub const INSTRUCTIONS: &str = "al-sem answers questions about Microsoft Dynamics 365 Business Central AL code by whole-program analysis. Use alsem_explain before editing a routine (its rules, writes, callers, tests), alsem_prove for yes/no safety questions, alsem_digest for the effects of a change, alsem_query_* for table effects. Answers are the exact output of the alsem CLI.";

/// The JSON-RPC error codes the server answers.
pub mod codes {
    pub const PARSE_ERROR: i64 = -32700;
    pub const INVALID_REQUEST: i64 = -32600;
    pub const METHOD_NOT_FOUND: i64 = -32601;
    pub const INVALID_PARAMS: i64 = -32602;
    pub const INTERNAL_ERROR: i64 = -32603;
    /// A request before the session is initialized.
    pub const NOT_INITIALIZED: i64 = -32002;
}

/// The front end's answers to a command line's file-system questions,
/// decided before it is sent (design r3 §4.3): the `alsem` binary decodes
/// the argv with its clap and runs the CLI's own decision code. `None`: the
/// argv does not decode (the child answers clap's error itself).
pub trait Decide: Send + Sync {
    fn decide(&self, argv: &[String], diff_text: Option<&str>) -> Option<Decisions>;
}

/// How the server runs.
#[derive(Clone, Debug)]
pub struct McpConfig {
    /// The `alsem` binary the engine children run.
    pub program: PathBuf,
    /// Extra environment for the engine children.
    pub child_env: Vec<(std::ffi::OsString, std::ffi::OsString)>,
    /// The defaults of a call that names no workspace, universe or freshness.
    pub defaults: tools::Defaults,
    /// The output root; the server's instance folder is created below it.
    pub output_root: PathBuf,
}

/// The default output root: `~/.al-sem/mcp-out` (a temp folder when the home
/// folder is unknown).
#[must_use]
pub fn default_output_root() -> PathBuf {
    crate::state_paths::state_path("mcp-out")
        .unwrap_or_else(|| std::env::temp_dir().join("alsem-mcp-out"))
}

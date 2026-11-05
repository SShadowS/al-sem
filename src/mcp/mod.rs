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
//! envelope's JSON without `stdoutText` is the first text block, the inline
//! stdout is the second text block exactly, and with a negotiated
//! `2025-06-18` the envelope is also `structuredContent`, holding the answer
//! once: `output` (the CLI's JSON parsed, no `stdoutText`) when the stdout is
//! inline JSON, else `stdoutText` (MCP output plan 1c: Claude Code reads and
//! size-checks `structuredContent` alone). `alsem_prove` answers human by
//! default; a call that names no `maxInlineBytes` gets 40 KiB.
//! Explain defaults to the human card; digest always writes a file; a stdout
//! over `maxInlineBytes` (default 40 KiB) goes to a file. A file answer
//! carries the renderer's `summary` when the command makes one (digest: per
//! root the effects by type, physical tables written, commits, boundaries and
//! unresolved counts), at most 8 KiB and inside the frame bound (MCP output
//! plan 1b). Every file goes into the server's private output folder
//! ([`output`]).
//!
//! **Engine children (stage 3).** Each child keeps its revision between
//! calls under a memory budget (`--memory-budget`, its share of
//! `--total-budget`) and ends after `--idle-timeout` with nothing to do; at
//! most `--max-children` run at once (a call for another workspace or
//! universe ends the least recently used idle child). A client's
//! `notifications/cancelled` drops a queued call or sends the running one
//! `session/cancel`, and the call is not answered (MCP). `alsem_refresh` is
//! `session/refresh {wait}`. The tools take no deadline (the design's §8.2
//! defines none), and the server sends no `session/notifyChanged`: it
//! watches no files; the default `fresh` check per call sees every edit.

pub mod output;
pub mod pool;
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

/// The default `--max-children` (design §7): engine children alive at once.
pub const DEFAULT_MAX_CHILDREN: usize = 2;

/// The server's `instructions` in its `initialize` result.
pub const INSTRUCTIONS: &str = "al-sem answers questions about Microsoft Dynamics 365 Business Central AL code by whole-program analysis of the workspace and the dependencies it loads. Every answer is the exact stdout of the alsem CLI.\n\
Which tool: before editing or reviewing a routine (its rules, what it writes, commits and raises, who calls it, which tests reach it, what the engine could not see): alsem_explain. A yes/no question about one routine (may it commit, does it write table T, raise event E, reach UI, throw): alsem_prove. The effects of a change (a diff or changed files) or of several roots at once: alsem_digest. Every record operation of one routine, reads, locks and temporary records included: alsem_query_effects. Who touches table T, or does routine R touch it, up or down the call stack: alsem_query_touches. Is the engine current or busy: alsem_status; rebuild now: alsem_refresh.\n\
Routine selectors, one grammar for every tool: a stable routine id (as JSON answers print it), `Owner::Routine` or `Owner.Routine` (the owner's type word and AL quotes optional: `Codeunit \"Sales-Post\"::OnRun`), or a bare routine name when it is unique in the workspace. An overloaded routine takes its parameter list: `Owner::Routine(Integer, Text)` by types (a type's length and `var` optional) or by parameter names; a `routine-ambiguous` answer prints each candidate in that form with its location and stable id. A name selects workspace routines only; explain, prove and digest also take a dependency routine's stable id.\n\
Lines are 1-based everywhere (every envelope and JSON document states `\"lineBase\": 1`). `ws:<path>:<line>` is relative to the workspace; `dep:<app name>:<path>:<line>` is inside a dependency (JSON writes `dep:<app guid>:<path>`).\n\
Size: an answer over 40 KiB (`maxInlineBytes`), or one over the client's 50,000 characters once escaped, goes to a file in the server's output folder; the envelope then has `stdoutFile` and a `summary` of at most 8 KiB. Read the summary first and open the file only for what it does not say. A digest always goes to a file. Human answers are 1-40 KB; JSON for a routine that calls Base Application is 100 KB-1 MB.\n\
Every command tool also takes `workspace` (default: the server's), `freshness` (fresh by default, the files as they are now; `freshness: latest-built` answers at once from the last built revision; verify answers from it and says whether it is stale), `singleApp: true` (the workspace without its dependencies), `out` (a file in the output folder) and `maxInlineBytes`.\n\
Dependency depth: explain, prove and digest follow every resolved call into dependency code with literal arguments, grouped per entry point; query lists operations one routine into dependency code and, for writes beyond, the same entry-point section explain prints.\n\
Words: `yes` means a path to the effect exists (a may-answer); `no` means proven absent under the stated assumptions; `unknown` and `no matching effect observed` mean none was seen but coverage is incomplete, so it is not a proof. A `coverage` line that says partial or incomplete names what was not followed (dynamic calls not resolved, interface calls open world, gaps); `assumptions` is what the answer takes as given (the loaded program; `closedWorld: true` assumes only loaded interface implementations); `limits` are caps that cut an answer. Temporary records are never counted as writes by any tool (explain lists them apart; query lists each under `all: true`). A caller-dependent write happens only when the caller passes a record that is not temporary.";

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
    /// Each child's `--memory-budget` (`None`: its universe's default),
    /// capped at `total_budget / max_children`.
    pub memory_budget: Option<u64>,
    /// Start each child with `--low-mem` (it keeps no dependency tree
    /// between revisions; its default budget is the low-mem one).
    pub low_mem: bool,
    /// Each child's `--idle-timeout` in seconds (0: never).
    pub idle_timeout_secs: u64,
    /// The most children alive at once; a call for another workspace or
    /// universe ends the least recently used idle child first.
    pub max_children: usize,
    /// The budget all children share (`None`: `max_children` times the
    /// per-child budget).
    pub total_budget: Option<u64>,
}

impl McpConfig {
    /// A configuration with the design's defaults (§7) for the engine
    /// children.
    #[must_use]
    pub fn new(program: PathBuf, defaults: tools::Defaults, output_root: PathBuf) -> McpConfig {
        McpConfig {
            program,
            child_env: Vec::new(),
            defaults,
            output_root,
            memory_budget: None,
            low_mem: false,
            idle_timeout_secs: crate::engine::resident::budget::DEFAULT_IDLE_TIMEOUT_SECS,
            max_children: DEFAULT_MAX_CHILDREN,
            total_budget: None,
        }
    }

    /// The `--memory-budget` an engine child of `universe` is started with.
    #[must_use]
    pub fn child_budget(&self, universe: tools::Universe) -> u64 {
        use crate::engine::resident::budget as b;
        let own = self.memory_budget.unwrap_or_else(|| {
            b::default_budget(universe == tools::Universe::SingleApp, self.low_mem)
        });
        match self.total_budget {
            Some(total) => own.min(total / self.max_children.max(1) as u64),
            None => own,
        }
    }
}

/// The default output root: `~/.al-sem/mcp-out` (a temp folder when the home
/// folder is unknown).
#[must_use]
pub fn default_output_root() -> PathBuf {
    crate::state_paths::state_path("mcp-out")
        .unwrap_or_else(|| std::env::temp_dir().join("alsem-mcp-out"))
}

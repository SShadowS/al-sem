//! The LSP front end of the context server (design 2026-11-01 §9): the
//! editor's server (`al-call-hierarchy`) answers explain / prove / digest /
//! query questions as a CLIENT of the resident engine child, `alsem
//! engine-host`, never with a second in-process engine (the design rejected
//! that: about 4 GB per editor window, and a blocked main loop).
//!
//! **Requests** (the frozen `al-call-hierarchy/` prefix, [`METHODS`]):
//! `explain`, `prove`, `digest`, `queryEffects`, `queryTouches`,
//! `engineStatus`, `engineRefresh`. Their params are the arguments of the
//! MCP tools of the same meaning (`alsem_explain`, ... — validated by the
//! same [`crate::mcp::tools::parse_call`], so every flag and default is the
//! CLI's), plus two optional LSP fields: `uri` (a document whose workspace
//! root answers, when `workspace` is not given; else the first root) and
//! `workDoneToken` (`$/progress` begin and end). The result is the MCP
//! envelope itself (design §8.3: `kind`, `revision`, `exitCode`,
//! `stdoutText` XOR `stdoutFile`, ...), so an answer's stdout is the CLI's
//! byte for byte. Invalid params are `-32602`; requests while the feature
//! is disabled are `-32803` (`RequestFailed`).
//!
//! **Off the main loop.** A command is handed to the engine [`Pool`] (one
//! worker thread per workspace and universe, the same pool `alsem mcp`
//! drives), so call hierarchy is answered while an engine request runs.
//! `$/cancelRequest` drops a queued request or sends the running one
//! `session/cancel`; either way the request is answered `RequestCancelled`
//! (`-32800`), with the engine's own answer (`outcome: cancelled`) as the
//! error's `data` when it ran. Edits the server sees (`didSave`, the file
//! watcher) go to each running child as `session/notifyChanged`, so the
//! next freshness check hashes those files again even when their size and
//! time did not change.
//!
//! **Lifecycle.** Nothing is read, created or started until the first engine
//! request: the config is read then, the output folder is created on the
//! first command, and the child is started by its first command
//! (`engineStatus` starts nothing). The child ends on its idle timeout
//! (`EngineFront` starts a new one on the next request) and when the
//! server shuts down. The engine's memory is the child's; the server holds
//! a pool of a few threads.
//!
//! **The engine executable.** The `alsem` binary: `engine.executable` from
//! the config, else `alsem` next to this server's own binary (the packages
//! ship both). Not the server binary itself: the engine decodes each
//! command line with the `alsem` CLI's clap and planning, which live in that
//! binary, and runs under its allocator settings. The CLI's file-system
//! decisions for a command that reads paths (`--diff`, `--file`) are made by
//! the same binary on this side (`alsem engine-ask --params`, [`HelperDecide`]),
//! so the child never probes the file system (design §3, §10).
//!
//! **Config** (read once, at the first engine request): the `engine`
//! section of `~/.al-sem/config.json`, overlaid by the first workspace
//! root's `.al-sem.json` ([`crate::config::EngineFileConfig`], pre-rename
//! spellings read too): `enabled`, `executable`, `memoryBudget`, `lowMem`,
//! `idleTimeoutSecs`, `maxChildren`.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, MutexGuard};

use lsp_server::{Message, RequestId};
use serde_json::{Map, Value, json};

use crate::config::EngineFileConfig;
use crate::engine::host::{AskParams, Decisions};
use crate::mcp::output::{OutputSink, new_instance_id};
use crate::mcp::pool::{ChildConfig, Pool, PoolConfig, Request, Transport};
use crate::mcp::tools::{self, Defaults, Freshness, ToolCall, Universe};
use crate::mcp::{Decide, codes};
use crate::wire::FailedOutcome;

/// The custom requests and the MCP tool each one mirrors.
pub const METHODS: &[(&str, &str)] = &[
    ("al-call-hierarchy/explain", "alsem_explain"),
    ("al-call-hierarchy/prove", "alsem_prove"),
    ("al-call-hierarchy/digest", "alsem_digest"),
    ("al-call-hierarchy/queryEffects", "alsem_query_effects"),
    ("al-call-hierarchy/queryTouches", "alsem_query_touches"),
    ("al-call-hierarchy/engineStatus", "alsem_status"),
    ("al-call-hierarchy/engineRefresh", "alsem_refresh"),
];

/// LSP's `RequestCancelled`.
pub const REQUEST_CANCELLED: i64 = -32800;
/// LSP's `RequestFailed`.
pub const REQUEST_FAILED: i64 = -32803;

/// The MCP tool `method` mirrors, if it is an engine request.
#[must_use]
pub fn tool_of(method: &str) -> Option<&'static str> {
    METHODS.iter().find(|(m, _)| *m == method).map(|(_, t)| *t)
}

/// Whether `method` is one of the engine requests.
#[must_use]
pub fn is_engine_method(method: &str) -> bool {
    tool_of(method).is_some()
}

/// Where the server's outbound messages go (the connection's sender).
pub type MessageSink = Arc<dyn Fn(Message) + Send + Sync>;

/// The engine settings, resolved from the config.
#[derive(Clone, Debug)]
pub struct EngineSettings {
    pub enabled: bool,
    /// The `alsem` binary, or why there is none.
    pub program: Result<PathBuf, String>,
    pub memory_budget: Option<u64>,
    pub low_mem: bool,
    pub idle_timeout_secs: u64,
    pub max_children: usize,
    /// Where the output folder is created (`~/.al-sem/lsp-out`).
    pub output_root: PathBuf,
}

impl EngineSettings {
    /// The settings `cfg` names, with the defaults for the rest.
    #[must_use]
    pub fn from_config(cfg: &EngineFileConfig) -> EngineSettings {
        let mut problems = Vec::new();
        let memory_budget = cfg
            .memory_budget
            .as_ref()
            .and_then(|b| b.bytes().map_err(|e| problems.push(e)).ok());
        let program = resolve_program(cfg.executable.as_deref().map(PathBuf::from));
        EngineSettings {
            enabled: cfg.enabled.unwrap_or(true),
            program: match (program, problems.pop()) {
                (Ok(p), None) => Ok(p),
                (Ok(_), Some(e)) => Err(format!("engine.memoryBudget: {e}")),
                (Err(e), _) => Err(e),
            },
            memory_budget,
            low_mem: cfg.low_mem.unwrap_or(false),
            idle_timeout_secs: cfg
                .idle_timeout_secs
                .unwrap_or(crate::engine::resident::budget::DEFAULT_IDLE_TIMEOUT_SECS),
            max_children: cfg
                .max_children
                .unwrap_or(crate::mcp::DEFAULT_MAX_CHILDREN)
                .max(1),
            output_root: crate::state_paths::state_path("lsp-out")
                .unwrap_or_else(|| std::env::temp_dir().join("alsem-lsp-out")),
        }
    }

    /// The settings the config files give (`~/.al-sem/config.json`, then
    /// `root`'s `.al-sem.json`).
    #[must_use]
    pub fn load(root: Option<&Path>) -> EngineSettings {
        EngineSettings::from_config(&EngineFileConfig::load_merged(root))
    }

    /// Each child's `--memory-budget` for `universe`.
    fn budget(&self, universe: Universe) -> u64 {
        self.memory_budget.unwrap_or_else(|| {
            crate::engine::resident::budget::default_budget(
                universe == Universe::SingleApp,
                self.low_mem,
            )
        })
    }
}

/// The `alsem` binary: the configured one, else `alsem` next to the running
/// binary. `Err` names what was looked for and how to set it.
pub fn resolve_program(configured: Option<PathBuf>) -> Result<PathBuf, String> {
    if let Some(p) = configured {
        return if p.is_file() {
            Ok(p)
        } else {
            Err(format!(
                "the engine executable {} (engine.executable) does not exist",
                p.display()
            ))
        };
    }
    let exe = std::env::current_exe()
        .map_err(|e| format!("the engine executable: this server's own path: {e}"))?;
    let sibling = exe.with_file_name(format!("alsem{}", std::env::consts::EXE_SUFFIX));
    if sibling.is_file() {
        Ok(sibling)
    } else {
        Err(format!(
            "no engine executable: {} does not exist; install the alsem binary next to \
             this server, or name it as \"engine\": {{\"executable\": \"...\"}} in \
             ~/.al-sem/config.json or the workspace's .al-sem.json",
            sibling.display()
        ))
    }
}

/// Whether a command line reads paths the front end must decide for it.
fn reads_paths(argv: &[String]) -> bool {
    argv.iter().take_while(|a| *a != "--").any(|a| {
        a.starts_with("--diff=") || a.starts_with("--file=") || a.starts_with("--changed=")
    })
}

/// The front end's decisions for a command line that reads paths, made by
/// the `alsem` binary's own decision code on this side: `alsem engine-ask
/// --params -- <argv>` (the diff text on its stdin) prints the
/// `session/ask` params it would send, decisions included. A command line
/// that reads no paths needs none (the child answers it exactly as with the
/// workspace as its diff root). `None` when the helper fails: the child then
/// refuses the ask, naming the missing decision.
pub struct HelperDecide {
    pub program: PathBuf,
}

impl Decide for HelperDecide {
    fn decide(&self, argv: &[String], diff_text: Option<&str>) -> Option<Decisions> {
        if !reads_paths(argv) {
            return None;
        }
        let mut child = Command::new(&self.program)
            .args(["engine-ask", "--params", "--"])
            .args(argv)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        let stdin = child.stdin.take();
        let text = diff_text.unwrap_or("").to_string();
        let writer = std::thread::spawn(move || {
            if let Some(mut stdin) = stdin {
                let _ = stdin.write_all(text.as_bytes());
            }
        });
        let out = child.wait_with_output().ok()?;
        let _ = writer.join();
        if !out.status.success() {
            return None;
        }
        serde_json::from_slice::<AskParams>(&out.stdout)
            .ok()?
            .decisions
    }
}

/// The LSP side of the engine pool.
struct LspTransport {
    sink: MessageSink,
}

impl LspTransport {
    fn send_value(&self, msg: Value) {
        match serde_json::from_value::<Message>(msg) {
            Ok(m) => (self.sink)(m),
            Err(e) => log::warn!("engine: a response is no LSP message: {e}"),
        }
    }
}

impl Transport for LspTransport {
    fn response(&self, id: &Value, body: &Value, _is_error: bool, _output: Option<Value>) -> Value {
        json!({"jsonrpc": "2.0", "id": id, "result": body})
    }

    fn send(&self, frame: &[u8]) {
        match serde_json::from_slice::<Value>(frame) {
            Ok(v) => self.send_value(v),
            Err(e) => log::warn!("engine: a response frame does not decode: {e}"),
        }
    }

    /// LSP: a cancelled request is still answered, `RequestCancelled`; the
    /// engine's own answer, when it ran, is the error's `data`.
    fn cancelled(&self, id: &Value, frame: Option<&[u8]>) {
        let data = frame
            .and_then(|f| serde_json::from_slice::<Value>(f).ok())
            .and_then(|v| v.get("result").cloned());
        let mut error = json!({"code": REQUEST_CANCELLED, "message": "the request was cancelled"});
        if let Some(d) = data {
            error["data"] = d;
        }
        self.send_value(json!({"jsonrpc": "2.0", "id": id, "error": error}));
    }

    fn progress(&self, token: &Value, done: u32, message: &str) {
        let value = if done == 0 {
            json!({"kind": "begin", "title": "al-sem", "message": message})
        } else {
            json!({"kind": "end", "message": message})
        };
        self.send_value(json!({
            "jsonrpc": "2.0",
            "method": "$/progress",
            "params": {"token": token, "value": value},
        }));
    }
}

/// The engine state once the first engine request arrived.
struct Started {
    settings: EngineSettings,
    /// Created by the first command (or refresh); `Err` when it could not be.
    pool: Option<Result<Pool, String>>,
}

/// The server's engine requests: one per server, shared with the file
/// watchers' forwarding threads.
pub struct LspEngines {
    sink: MessageSink,
    /// The root whose `.al-sem.json` is read (the first configured one).
    config_root: Option<PathBuf>,
    /// Settings to use instead of the config files (tests).
    preset: Option<EngineSettings>,
    state: Mutex<Option<Started>>,
}

impl LspEngines {
    /// Nothing is read or started here.
    #[must_use]
    pub fn new(sink: MessageSink, config_root: Option<PathBuf>) -> LspEngines {
        LspEngines {
            sink,
            config_root,
            preset: None,
            state: Mutex::new(None),
        }
    }

    /// With `settings` instead of the config files.
    #[must_use]
    pub fn with_settings(sink: MessageSink, settings: EngineSettings) -> LspEngines {
        LspEngines {
            preset: Some(settings),
            ..LspEngines::new(sink, None)
        }
    }

    fn lock(&self) -> MutexGuard<'_, Option<Started>> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Whether any engine request arrived (the config was read).
    #[must_use]
    pub fn started(&self) -> bool {
        self.lock().is_some()
    }

    /// Whether the engine pool exists (a command or refresh arrived).
    #[must_use]
    pub fn pool_created(&self) -> bool {
        self.lock()
            .as_ref()
            .is_some_and(|s| matches!(s.pool, Some(Ok(_))))
    }

    fn reply(&self, id: &Value, result: Result<Value, (i64, String)>) {
        let msg = match result {
            Ok(r) => json!({"jsonrpc": "2.0", "id": id, "result": r}),
            Err((code, message)) => {
                json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
            }
        };
        LspTransport {
            sink: Arc::clone(&self.sink),
        }
        .send_value(msg);
    }

    /// Answer engine request `method` (`id`, `params`): at once (status,
    /// invalid params, disabled) or through the pool's worker. `route` maps
    /// a document URI to its workspace root; `default_root` is the root of
    /// a request that names neither a workspace nor a URI. Never blocks on
    /// the engine.
    pub fn handle(
        &self,
        id: &RequestId,
        method: &str,
        params: Value,
        route: &dyn Fn(&str) -> Option<PathBuf>,
        default_root: Option<&Path>,
    ) {
        let id = serde_json::to_value(id).unwrap_or(Value::Null);
        let Some(tool) = tool_of(method) else {
            return self.reply(
                &id,
                Err((codes::METHOD_NOT_FOUND, format!("no method {method}"))),
            );
        };
        let mut args = match params {
            Value::Null => Map::new(),
            Value::Object(m) => m,
            _ => {
                return self.reply(
                    &id,
                    Err((codes::INVALID_PARAMS, "params must be an object".into())),
                );
            }
        };
        let uri = match args.remove("uri") {
            None | Some(Value::Null) => None,
            Some(Value::String(u)) => Some(u),
            Some(_) => {
                return self.reply(
                    &id,
                    Err((codes::INVALID_PARAMS, "'uri' must be a string".into())),
                );
            }
        };
        let progress = args
            .remove("workDoneToken")
            .filter(|t| t.is_string() || t.is_number());
        let root = match &uri {
            Some(u) => match route(u) {
                Some(r) => Some(r),
                None => {
                    return self.reply(
                        &id,
                        Err((
                            codes::INVALID_PARAMS,
                            format!("'uri' {u} is under no workspace root of this server"),
                        )),
                    );
                }
            },
            None => default_root.map(Path::to_path_buf),
        };
        let defaults = Defaults {
            workspace: root.map(|r| r.to_string_lossy().into_owned()),
            universe: Universe::CrossApp,
            freshness: Freshness::Fresh,
        };
        // The editor keeps the CLI's defaults (module doc): the MCP tools' own
        // (prove human, a 40 KiB inline cap; MCP output plan 1c) serve an
        // agent's client, not the extension.
        if tool == "alsem_prove" {
            args.entry("format").or_insert_with(|| json!("json"));
        }
        if tools::TOOL_NAMES.contains(&tool) && !matches!(tool, "alsem_status" | "alsem_refresh") {
            args.entry("maxInlineBytes")
                .or_insert_with(|| json!(crate::wire::DEFAULT_INLINE_BYTES));
        }
        let call = match tools::parse_call(tool, Some(&Value::Object(args)), &defaults) {
            Ok(c) => c,
            Err(why) => {
                let why = why.unwrap_or_else(|| format!("unknown tool '{tool}'"));
                return self.reply(&id, Err((codes::INVALID_PARAMS, why)));
            }
        };
        let mut state = self.lock();
        let started = state.get_or_insert_with(|| Started {
            settings: self
                .preset
                .clone()
                .unwrap_or_else(|| EngineSettings::load(self.config_root.as_deref())),
            pool: None,
        });
        if !started.settings.enabled {
            drop(state);
            return self.reply(
                &id,
                Err((
                    REQUEST_FAILED,
                    "al-sem engine requests are disabled (\"engine\": {\"enabled\": false} in \
                     ~/.al-sem/config.json or the workspace's .al-sem.json)"
                        .into(),
                )),
            );
        }
        let request = match call {
            ToolCall::Status(c) => {
                let body = status(started, &c);
                drop(state);
                return self.reply(&id, Ok(body));
            }
            ToolCall::Refresh(c) => Request::Refresh(c),
            ToolCall::Command(call) => Request::Command { call, progress },
            // A composite request: the editor front end has no method for it
            // yet (the MCP tool and the CLI do).
            ToolCall::Tests(_) => {
                drop(state);
                return self.reply(
                    &id,
                    Err((
                        codes::INVALID_PARAMS,
                        "alsem_tests is not served through the LSP front end; use the CLI \
                         (alsem tests) or the MCP tool"
                            .into(),
                    )),
                );
            }
            ToolCall::Impact(_) => {
                drop(state);
                return self.reply(
                    &id,
                    Err((
                        codes::INVALID_PARAMS,
                        "alsem_impact is not served through the LSP front end; use the CLI \
                         (alsem impact) or the MCP tool"
                            .into(),
                    )),
                );
            }
        };
        let universe = match &request {
            Request::Command { call, .. } => call.universe,
            Request::Refresh(c) => c.universe,
        };
        if started.pool.is_none() {
            started.pool = Some(create_pool(&started.settings, &self.sink));
        }
        match started.pool.as_ref().expect("created above") {
            Ok(pool) => pool.submit(id, request),
            Err(why) => {
                let envelope = json!({
                    "kind": "failed",
                    "revision": null,
                    "outcome": FailedOutcome::EngineError.as_str(),
                    "message": why,
                    "universe": universe.as_str(),
                });
                drop(state);
                self.reply(&id, Ok(envelope));
            }
        }
    }

    /// `$/cancelRequest` for `id`: `true` when it was an engine request
    /// still queued or running (it is then answered `RequestCancelled`).
    pub fn cancel(&self, id: &RequestId) -> bool {
        let id = serde_json::to_value(id).unwrap_or(Value::Null);
        match self.lock().as_ref().and_then(|s| s.pool.as_ref()) {
            Some(Ok(pool)) => pool.cancel(&id),
            _ => false,
        }
    }

    /// Edits the server saw: each running child whose workspace holds some
    /// of `paths` is sent them (`session/notifyChanged`, spelled under its
    /// workspace). Starts nothing.
    pub fn notify_changed(&self, paths: &[PathBuf]) {
        if let Some(Some(Ok(pool))) = self.lock().as_ref().map(|s| s.pool.as_ref()) {
            pool.notify_changed(&|workspace: &Path| {
                paths
                    .iter()
                    .filter_map(|p| {
                        crate::lsp::updater::rel_under_root(workspace, p)
                            .map(|rel| workspace.join(rel))
                    })
                    .collect()
            });
        }
    }

    /// End every engine child: running requests are cancelled, queued ones
    /// dropped. The output folder is removed when nothing was written to it.
    pub fn shutdown(&self) {
        if let Some(Some(Ok(pool))) = self.lock().take().map(|s| s.pool) {
            pool.shutdown(true);
            let _ = std::fs::remove_dir(pool.sink().instance());
        }
    }
}

/// The pool for `settings`: its output folder created, its children's
/// configuration set.
fn create_pool(settings: &EngineSettings, sink: &MessageSink) -> Result<Pool, String> {
    let program = settings.program.clone()?;
    let out = OutputSink::create(&settings.output_root, &new_instance_id())
        .map_err(|e| format!("the engine output folder: {e}"))?;
    Ok(Pool::new(
        PoolConfig {
            decide: Arc::new(HelperDecide {
                program: program.clone(),
            }),
            program,
            child_env: Vec::new(),
            children: ChildConfig {
                budgets: (
                    settings.budget(Universe::CrossApp),
                    settings.budget(Universe::SingleApp),
                ),
                low_mem: settings.low_mem,
                idle_timeout_secs: settings.idle_timeout_secs,
                max_children: settings.max_children,
            },
            sink: out,
        },
        Arc::new(LspTransport {
            sink: Arc::clone(sink),
        }),
    ))
}

/// `engineStatus`: the settings and every engine (or the one named), from
/// the workers' published snapshots; starts nothing.
fn status(started: &Started, call: &tools::EngineCall) -> Value {
    let s = &started.settings;
    let (engines, folder, written) = match &started.pool {
        Some(Ok(pool)) => (
            pool.engines(call),
            Some(pool.sink().instance().to_path_buf()),
            pool.sink().written(),
        ),
        _ => (Vec::new(), None, 0),
    };
    json!({
        "kind": "status",
        "server": {
            "pid": std::process::id(),
            "alsemVersion": env!("CARGO_PKG_VERSION"),
            "engineMode": "resident",
            "enabled": s.enabled,
            "executable": s.program.as_ref().ok(),
            "executableError": s.program.as_ref().err(),
            "outputFolder": folder,
            "outputBytes": written,
            "maxChildren": s.max_children,
            "childBudgetBytes": {
                "crossApp": s.budget(Universe::CrossApp),
                "singleApp": s.budget(Universe::SingleApp),
            },
            "idleTimeoutSecs": s.idle_timeout_secs,
            "lowMem": s.low_mem,
        },
        "engines": engines,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ByteSize;

    /// Only the explicit path-reading options need the helper: a routine
    /// selector, a positional after `--` or a format never do.
    /// DISCRIMINATION (2026-11-02): dropping the `take_while` (looking past
    /// `--`) fails the positional case; reverted, it passes.
    #[test]
    fn only_path_reading_command_lines_are_decided() {
        let argv = |a: &[&str]| a.iter().map(|s| (*s).to_string()).collect::<Vec<_>>();
        assert!(reads_paths(&argv(&["explain", "--diff=-", "--", "/ws"])));
        assert!(reads_paths(&argv(&["digest", "--file=a.al", "--", "/ws"])));
        assert!(!reads_paths(&argv(&[
            "explain",
            "--routine=--diff=x",
            "--format=human",
            "--",
            "/ws"
        ])));
        assert!(!reads_paths(&argv(&[
            "prove", "--", "/ws", "--diff=x", "q"
        ])));
        // A command that reads no paths is never sent to the helper (a
        // program that does not exist is never run).
        let d = HelperDecide {
            program: PathBuf::from("/nonexistent/alsem"),
        };
        assert_eq!(
            d.decide(&argv(&["explain", "--routine=A", "--", "/ws"]), None),
            None
        );
    }

    /// The executable: a configured one that exists, a configured one that
    /// does not (named in the error), and settings defaults.
    /// DISCRIMINATION (2026-11-02): accepting a configured path without the
    /// `is_file` check fails the missing case; reverted, it passes.
    #[test]
    fn the_engine_executable_is_resolved_from_the_config() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("alsem");
        std::fs::write(&exe, b"").unwrap();
        assert_eq!(resolve_program(Some(exe.clone())), Ok(exe.clone()));
        let missing = dir.path().join("nope");
        let err = resolve_program(Some(missing.clone())).unwrap_err();
        assert!(err.contains("engine.executable"), "{err}");
        let s = EngineSettings::from_config(&EngineFileConfig {
            executable: Some(exe.to_string_lossy().into_owned()),
            memory_budget: Some(ByteSize::Text("1GiB".into())),
            ..EngineFileConfig::default()
        });
        assert!(s.enabled, "enabled by default");
        assert_eq!(s.program, Ok(exe));
        assert_eq!(s.budget(Universe::CrossApp), 1 << 30);
        assert_eq!(s.max_children, crate::mcp::DEFAULT_MAX_CHILDREN);
        let bad = EngineSettings::from_config(&EngineFileConfig {
            memory_budget: Some(ByteSize::Text("lots".into())),
            executable: Some(dir.path().join("alsem").to_string_lossy().into_owned()),
            ..EngineFileConfig::default()
        });
        assert!(bad.program.unwrap_err().contains("memoryBudget"));
    }

    fn collecting() -> (MessageSink, Arc<Mutex<Vec<Message>>>) {
        let got = Arc::new(Mutex::new(Vec::new()));
        let g = Arc::clone(&got);
        (Arc::new(move |m| g.lock().unwrap().push(m)), got)
    }

    fn settings(enabled: bool) -> EngineSettings {
        EngineSettings {
            enabled,
            program: Err("no engine in this test".into()),
            memory_budget: None,
            low_mem: false,
            idle_timeout_secs: 600,
            max_children: 2,
            output_root: std::env::temp_dir(),
        }
    }

    /// Requests answered without an engine: invalid params, a disabled
    /// feature, a status (no pool, nothing started), a URI under no root,
    /// and a command whose executable is missing (a `failed` envelope).
    /// DISCRIMINATION (2026-11-02): ignoring `enabled` fails the last
    /// assertion (a `failed` envelope instead of `RequestFailed`); reverted,
    /// it passes.
    #[test]
    fn requests_answered_without_an_engine() {
        let (sink, got) = collecting();
        let engines = LspEngines::with_settings(Arc::clone(&sink), settings(true));
        let route = |_: &str| None;
        let ws = Path::new("/ws");
        let id = |n: i32| RequestId::from(n);
        engines.handle(
            &id(1),
            "al-call-hierarchy/explain",
            json!({}),
            &route,
            Some(ws),
        );
        engines.handle(
            &id(2),
            "al-call-hierarchy/engineStatus",
            Value::Null,
            &route,
            Some(ws),
        );
        engines.handle(
            &id(3),
            "al-call-hierarchy/explain",
            json!({"routines": ["A"], "uri": "file:///elsewhere/a.al"}),
            &route,
            Some(ws),
        );
        assert!(!engines.pool_created(), "status and refusals start nothing");
        engines.handle(
            &id(4),
            "al-call-hierarchy/prove",
            json!({"routine": "A", "question": "may-commit"}),
            &route,
            Some(ws),
        );
        let off = LspEngines::with_settings(Arc::clone(&sink), settings(false));
        off.handle(
            &id(5),
            "al-call-hierarchy/queryEffects",
            json!({"routine": "A"}),
            &route,
            Some(ws),
        );
        let msgs: Vec<Value> = got
            .lock()
            .unwrap()
            .iter()
            .map(|m| serde_json::to_value(m).unwrap())
            .collect();
        assert_eq!(msgs.len(), 5, "{msgs:?}");
        assert_eq!(msgs[0]["error"]["code"], codes::INVALID_PARAMS);
        assert_eq!(msgs[1]["result"]["kind"], "status");
        assert_eq!(msgs[1]["result"]["engines"], json!([]));
        assert_eq!(msgs[2]["error"]["code"], codes::INVALID_PARAMS);
        assert_eq!(msgs[3]["result"]["kind"], "failed");
        assert_eq!(msgs[3]["result"]["outcome"], "engineError");
        assert_eq!(msgs[3]["result"]["message"], "no engine in this test");
        assert_eq!(msgs[4]["error"]["code"], REQUEST_FAILED);
    }
}

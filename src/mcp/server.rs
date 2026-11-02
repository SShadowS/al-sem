//! The MCP server loop: JSON-RPC 2.0 over NDJSON on stdio (design r7 §8.1),
//! the lifecycle state machine, and one worker thread per (workspace,
//! universe) that owns that pair's engine child ([`EngineFront`]).
//!
//! The main thread reads requests and answers the light ones itself
//! (`initialize`, `ping`, `tools/list`, `alsem_status`, every error); a
//! `tools/call` of a command or of `alsem_refresh` is queued on its
//! engine's worker, which answers it when done, so a long build never
//! blocks `ping` or a call on another workspace. Responses are written one
//! whole line at a time under one lock.

use std::collections::{HashMap, HashSet};
use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use serde_json::{Value, json};

use super::output::{OutputSink, SinkError};
use super::tools::{self, CommandCall, Defaults, EngineCall, ToolCall, Universe};
use super::{DEFAULT_PROTOCOL, Decide, McpConfig, SUPPORTED_PROTOCOLS, WORKER_QUEUE_DEPTH, codes};
use crate::engine::host::AskParams;
use crate::engine::host::front::{EngineFront, FrontConfig};
use crate::wire::{
    Answer, FailedOutcome, MAX_FRAME_BYTES, Outcome, ReadFrame, SpillFile, SpillKind, Stderr,
    Stdout, bound_frame, encode_frame, read_frame,
};

/// Where the session is in the MCP lifecycle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    /// Before `initialize`.
    Uninitialized,
    /// `initialize` answered, `notifications/initialized` not yet seen.
    Initializing,
    Ready,
}

/// The response channel: whole lines, one writer at a time.
#[derive(Clone)]
struct Out(Arc<Mutex<Box<dyn Write + Send>>>);

impl Out {
    fn line(&self, frame: &[u8]) {
        let mut w = self.0.lock().unwrap_or_else(|p| p.into_inner());
        if w.write_all(frame).and_then(|()| w.flush()).is_err() {
            log("writing a response failed; the client is gone");
        }
    }

    fn send(&self, msg: &Value) {
        match encode_frame(msg) {
            Ok(f) => self.line(&f),
            Err(e) => log(&format!("a response does not encode: {e}")),
        }
    }
}

fn log(line: &str) {
    let _ = writeln!(std::io::stderr(), "al-sem: mcp: {line}");
}

fn result(id: &Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

fn error(id: &Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

/// What one incoming line is.
#[derive(Debug, PartialEq)]
enum Incoming {
    Request {
        id: Value,
        method: String,
        params: Value,
    },
    Notification {
        method: String,
    },
    /// A response to a request of ours (we send none): ignored.
    Response,
    /// Answer this error and go on.
    Invalid(Value),
}

/// Classify one frame (design r7 §8.1): ids are numbers or strings; `null`
/// and every other type are invalid requests.
fn classify(frame: &[u8]) -> Incoming {
    let null = Value::Null;
    let Ok(v) = serde_json::from_slice::<Value>(frame) else {
        return Incoming::Invalid(error(&null, codes::PARSE_ERROR, "parse error"));
    };
    let Value::Object(m) = v else {
        let why = if v.is_array() {
            "batches are not supported"
        } else {
            "a message is a JSON object"
        };
        return Incoming::Invalid(error(&null, codes::INVALID_REQUEST, why));
    };
    let id = m.get("id");
    let valid_id = matches!(id, Some(Value::Number(_) | Value::String(_)));
    let echo = if valid_id { id.unwrap() } else { &null };
    if m.get("jsonrpc") != Some(&json!("2.0")) {
        return Incoming::Invalid(error(
            echo,
            codes::INVALID_REQUEST,
            "jsonrpc must be \"2.0\"",
        ));
    }
    let Some(method) = m.get("method") else {
        if m.contains_key("result") || m.contains_key("error") {
            return Incoming::Response;
        }
        return Incoming::Invalid(error(echo, codes::INVALID_REQUEST, "no method"));
    };
    let Some(method) = method.as_str() else {
        return Incoming::Invalid(error(
            echo,
            codes::INVALID_REQUEST,
            "method must be a string",
        ));
    };
    match id {
        None => Incoming::Notification {
            method: method.to_string(),
        },
        Some(_) if !valid_id => Incoming::Invalid(error(
            &null,
            codes::INVALID_REQUEST,
            "a request id is a number or a string",
        )),
        Some(id) => Incoming::Request {
            id: id.clone(),
            method: method.to_string(),
            params: m.get("params").cloned().unwrap_or(Value::Null),
        },
    }
}

/// The protocol version answered to `initialize`'s `params`: the client's
/// when supported, else [`DEFAULT_PROTOCOL`] (a missing one included).
fn negotiate(params: &Value) -> &'static str {
    let asked = params.get("protocolVersion").and_then(Value::as_str);
    SUPPORTED_PROTOCOLS
        .iter()
        .copied()
        .find(|v| Some(*v) == asked)
        .unwrap_or(DEFAULT_PROTOCOL)
}

/// Whether `version` carries `structuredContent` (2025-06-18 and later).
fn has_structured(version: &str) -> bool {
    version >= "2025-06-18"
}

/// The engines' key.
type Key = (String, Universe);

/// What `alsem_status` shows of one engine, published by its worker.
#[derive(Default)]
struct Snapshot {
    started: bool,
    hello: Option<crate::engine::host::Hello>,
    child: Option<crate::engine::host::ChildStatus>,
    spawns: u64,
    deaths: u64,
    completed: u64,
    last_failure: Option<(FailedOutcome, String)>,
    stderr_tail: String,
    backoff_ms: u128,
    running_tool: Option<String>,
}

/// A queued job.
enum Job {
    Command {
        id: Value,
        call: CommandCall,
        progress: Option<Value>,
        queued: Instant,
    },
    Refresh {
        id: Value,
        call: EngineCall,
    },
}

/// One engine's worker.
struct Worker {
    tx: Sender<Job>,
    pending: Arc<AtomicUsize>,
    snapshot: Arc<Mutex<Snapshot>>,
    thread: std::thread::JoinHandle<()>,
}

/// What every worker shares.
struct Shared {
    out: Out,
    sink: OutputSink,
    decide: Arc<dyn Decide>,
    program: PathBuf,
    child_env: Vec<(std::ffi::OsString, std::ffi::OsString)>,
    in_flight: Mutex<HashSet<String>>,
    closing: AtomicBool,
    protocol: Mutex<&'static str>,
}

impl Shared {
    /// A finished request: its id leaves the in-flight set, then its line
    /// goes out.
    fn finish(&self, id: &Value, frame: &[u8]) {
        self.in_flight
            .lock()
            .expect("in flight")
            .remove(&id_key(id));
        self.out.line(frame);
    }

    fn finish_value(&self, id: &Value, msg: &Value) {
        match encode_frame(msg) {
            Ok(f) => self.finish(id, &f),
            Err(e) => {
                log(&format!("a response does not encode: {e}"));
                self.in_flight
                    .lock()
                    .expect("in flight")
                    .remove(&id_key(id));
            }
        }
    }
}

/// An id as the in-flight set holds it (its JSON, so `1` and `"1"` differ).
fn id_key(id: &Value) -> String {
    id.to_string()
}

/// Serve MCP on `input` / `output` until the input ends.
pub fn serve(
    cfg: McpConfig,
    decide: Arc<dyn Decide>,
    mut input: impl BufRead,
    output: Box<dyn Write + Send>,
) -> Result<(), String> {
    let sink = OutputSink::create(&cfg.output_root, &super::output::new_instance_id())?;
    let shared = Arc::new(Shared {
        out: Out(Arc::new(Mutex::new(output))),
        sink,
        decide,
        program: cfg.program,
        child_env: cfg.child_env,
        in_flight: Mutex::new(HashSet::new()),
        closing: AtomicBool::new(false),
        protocol: Mutex::new(DEFAULT_PROTOCOL),
    });
    let defaults = cfg.defaults;
    let mut state = State::Uninitialized;
    let mut workers: HashMap<Key, Worker> = HashMap::new();
    let out = shared.out.clone();
    loop {
        let frame = match read_frame(&mut input) {
            Ok(ReadFrame::Frame(f)) => f,
            Ok(ReadFrame::Eof) => break,
            Ok(ReadFrame::Oversized(o)) => {
                out.send(&error(
                    &Value::Null,
                    codes::PARSE_ERROR,
                    &format!("{}", o.error()),
                ));
                if o.skip_rest(&mut input).is_err() {
                    break;
                }
                continue;
            }
            Err(e) => {
                log(&format!("reading stdin: {e}"));
                break;
            }
        };
        if frame.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        let (id, method, params) = match classify(&frame) {
            Incoming::Invalid(e) => {
                out.send(&e);
                continue;
            }
            Incoming::Response => continue,
            Incoming::Notification { method } => {
                match method.as_str() {
                    "notifications/initialized" if state == State::Initializing => {
                        state = State::Ready;
                    }
                    "notifications/cancelled" => {
                        log("a cancellation was received; requests run to completion");
                    }
                    _ => {}
                }
                continue;
            }
            Incoming::Request { id, method, params } => (id, method, params),
        };
        if shared
            .in_flight
            .lock()
            .expect("in flight")
            .contains(&id_key(&id))
        {
            out.send(&error(
                &id,
                codes::INVALID_REQUEST,
                "a request with this id is still in flight",
            ));
            continue;
        }
        match method.as_str() {
            // Answered -32601 in every state: Claude Code probes it and
            // falls back to `initialize`.
            "server/discover" => {
                out.send(&error(
                    &id,
                    codes::METHOD_NOT_FOUND,
                    "no method server/discover",
                ));
            }
            "ping" => out.send(&result(&id, json!({}))),
            "initialize" if state == State::Uninitialized => {
                let version = negotiate(&params);
                *shared.protocol.lock().expect("protocol") = version;
                state = State::Initializing;
                out.send(&result(
                    &id,
                    json!({
                        "protocolVersion": version,
                        "capabilities": {"tools": {"listChanged": false}},
                        "serverInfo": {"name": "alsem", "version": env!("CARGO_PKG_VERSION")},
                        "instructions": super::INSTRUCTIONS,
                    }),
                ));
            }
            "initialize" => {
                out.send(&error(&id, codes::INVALID_REQUEST, "already initialized"));
            }
            _ if state != State::Ready => {
                out.send(&error(
                    &id,
                    codes::NOT_INITIALIZED,
                    "server not initialized",
                ));
            }
            "tools/list" => out.send(&result(&id, json!({"tools": tools::tool_list()}))),
            "tools/call" => {
                let name = params.get("name").and_then(Value::as_str).unwrap_or("");
                let call = match tools::parse_call(name, params.get("arguments"), &defaults) {
                    Ok(c) => c,
                    Err(None) => {
                        out.send(&error(
                            &id,
                            codes::INVALID_PARAMS,
                            &format!("unknown tool '{name}'"),
                        ));
                        continue;
                    }
                    Err(Some(why)) => {
                        out.send(&error(&id, codes::INVALID_PARAMS, &why));
                        continue;
                    }
                };
                let progress = params
                    .get("_meta")
                    .and_then(|m| m.get("progressToken"))
                    .filter(|t| t.is_string() || t.is_number())
                    .cloned();
                dispatch(&shared, &mut workers, &defaults, id, call, progress);
            }
            other => out.send(&error(
                &id,
                codes::METHOD_NOT_FOUND,
                &format!("no method {other}"),
            )),
        }
    }
    // The input ended: the client is shutting the server down (MCP stdio).
    // Queued calls are dropped unanswered; a call already running finishes,
    // then every engine child is shut down.
    shared.closing.store(true, Ordering::SeqCst);
    for (_, w) in workers.drain() {
        drop(w.tx);
        let _ = w.thread.join();
    }
    // An instance folder nothing was written to is removed (only if empty).
    let _ = std::fs::remove_dir(shared.sink.instance());
    Ok(())
}

/// Hand a validated call to its engine's worker, or answer it here.
fn dispatch(
    shared: &Arc<Shared>,
    workers: &mut HashMap<Key, Worker>,
    defaults: &Defaults,
    id: Value,
    call: ToolCall,
    progress: Option<Value>,
) {
    let (key, job) = match call {
        ToolCall::Status(c) => {
            let body = status(shared, workers, &c, defaults);
            let msg = result(&id, tool_result(shared, &body, false, None));
            shared.out.send(&msg);
            return;
        }
        ToolCall::Refresh(c) => {
            let key = (c.workspace.clone().unwrap_or_default(), c.universe);
            (
                key,
                Job::Refresh {
                    id: id.clone(),
                    call: c,
                },
            )
        }
        ToolCall::Command(c) => (
            (c.workspace.clone(), c.universe),
            Job::Command {
                id: id.clone(),
                call: c,
                progress,
                queued: Instant::now(),
            },
        ),
    };
    let worker = workers
        .entry(key.clone())
        .or_insert_with(|| spawn_worker(shared, &key));
    if worker.pending.load(Ordering::SeqCst) >= WORKER_QUEUE_DEPTH {
        let envelope = json!({
            "kind": "failed",
            "revision": null,
            "outcome": FailedOutcome::Busy.as_str(),
            "message": format!("{WORKER_QUEUE_DEPTH} requests already queued for this engine"),
            "universe": key.1.as_str(),
        });
        shared
            .out
            .send(&result(&id, tool_result(shared, &envelope, true, None)));
        return;
    }
    shared
        .in_flight
        .lock()
        .expect("in flight")
        .insert(id_key(&id));
    worker.pending.fetch_add(1, Ordering::SeqCst);
    if worker.tx.send(job).is_err() {
        worker.pending.fetch_sub(1, Ordering::SeqCst);
        shared.finish_value(
            &id,
            &error(
                &id,
                codes::INTERNAL_ERROR,
                "the engine's worker has stopped",
            ),
        );
    }
}

fn spawn_worker(shared: &Arc<Shared>, key: &Key) -> Worker {
    let (tx, rx) = channel::<Job>();
    let pending = Arc::new(AtomicUsize::new(0));
    let snapshot = Arc::new(Mutex::new(Snapshot::default()));
    let thread = {
        let shared = Arc::clone(shared);
        let pending = Arc::clone(&pending);
        let snapshot = Arc::clone(&snapshot);
        let key = key.clone();
        std::thread::Builder::new()
            .name(format!("alsem-mcp-{}", key.1.as_str()))
            .spawn(move || work(&shared, &key, &rx, &pending, &snapshot))
            .expect("spawn a worker thread")
    };
    Worker {
        tx,
        pending,
        snapshot,
        thread,
    }
}

/// A worker: one engine front end, its jobs one at a time.
fn work(
    shared: &Shared,
    key: &Key,
    rx: &Receiver<Job>,
    pending: &AtomicUsize,
    snapshot: &Mutex<Snapshot>,
) {
    let mut cfg = FrontConfig::new(&shared.program, &key.0);
    cfg.env.clone_from(&shared.child_env);
    let mut front = match EngineFront::new(cfg) {
        Ok(f) => Some(f),
        Err(e) => {
            log(&format!("engine front end for '{}': {e}", key.0));
            None
        }
    };
    while let Ok(job) = rx.recv() {
        if shared.closing.load(Ordering::SeqCst) {
            pending.fetch_sub(1, Ordering::SeqCst);
            continue;
        }
        match job {
            Job::Command {
                id,
                call,
                progress,
                queued,
            } => {
                snapshot.lock().expect("snapshot").running_tool = Some(call.tool.to_string());
                let frame = match front.as_mut() {
                    Some(front) => {
                        run_command(shared, front, &id, &call, progress.as_ref(), queued)
                    }
                    None => encode_or_log(&result(
                        &id,
                        tool_result(
                            shared,
                            &json!({
                                "kind": "failed", "revision": null,
                                "outcome": FailedOutcome::EngineError.as_str(),
                                "message": "the engine front end could not be created",
                                "universe": call.universe.as_str(),
                            }),
                            true,
                            None,
                        ),
                    )),
                };
                if let Some(front) = front.as_mut() {
                    publish(front, snapshot, true);
                }
                pending.fetch_sub(1, Ordering::SeqCst);
                shared.finish(&id, &frame);
            }
            Job::Refresh { id, call } => {
                if let Some(front) = front.as_mut() {
                    front.reset();
                    publish(front, snapshot, false);
                }
                let body = json!({
                    "kind": "refresh",
                    "workspace": call.workspace,
                    "universe": call.universe.as_str(),
                    "waited": call.wait,
                    "refreshed": front.is_some(),
                    "note": "every call reads the workspace as it is now (one-shot engine); a refresh only leaves a failed state",
                });
                pending.fetch_sub(1, Ordering::SeqCst);
                shared.finish_value(&id, &result(&id, tool_result(shared, &body, false, None)));
            }
        }
    }
    if let Some(mut front) = front {
        front.shutdown();
    }
}

fn encode_or_log(msg: &Value) -> Vec<u8> {
    encode_frame(msg).unwrap_or_else(|e| {
        log(&format!("a response does not encode: {e}"));
        Vec::new()
    })
}

/// Publish the front end's state for `alsem_status`; `ask_status` asks a
/// running child its `session/status` (cheap) after a command.
fn publish(front: &mut EngineFront, snapshot: &Mutex<Snapshot>, ask_status: bool) {
    let child = if ask_status && front.hello().is_some() && front.child_exited() == Some(false) {
        front.status().ok()
    } else {
        None
    };
    let stats = front.stats();
    let tail = front.stderr_tail();
    let keep = tail.len().saturating_sub(4096);
    let mut start = keep;
    while !tail.is_char_boundary(start) {
        start += 1;
    }
    let mut s = snapshot.lock().expect("snapshot");
    s.started = true;
    s.hello = front.hello().cloned();
    if child.is_some() || s.hello.is_none() {
        s.child = child;
    }
    s.spawns = stats.spawns;
    s.deaths = stats.deaths;
    s.completed = stats.completed;
    s.last_failure = front.last_failure();
    s.stderr_tail = tail[start..].to_string();
    s.backoff_ms = front.current_backoff().as_millis();
    s.running_tool = None;
}

/// `alsem_status`: the server and every engine (or the named one), from
/// the workers' published snapshots; starts nothing.
fn status(
    shared: &Shared,
    workers: &HashMap<Key, Worker>,
    call: &EngineCall,
    defaults: &Defaults,
) -> Value {
    let mut engines: Vec<Value> = workers
        .iter()
        .filter(|(k, _)| {
            call.workspace
                .as_ref()
                .is_none_or(|w| *w == k.0 && call.universe == k.1)
        })
        .map(|(k, w)| {
            let s = w.snapshot.lock().expect("snapshot");
            json!({
                "workspace": k.0,
                "universe": k.1.as_str(),
                "queued": w.pending.load(Ordering::SeqCst),
                "running": s.running_tool,
                "started": s.started,
                "engine": s.hello.as_ref().map(|h| json!({
                    "pid": h.pid,
                    "engineInstanceId": h.engine_instance_id,
                    "alsemVersion": h.alsem_version,
                    "protocol": h.protocol,
                    "grammarSha": h.grammar_sha,
                })),
                "child": s.child.as_ref().map(|c| json!({
                    "asks": c.asks,
                    "workspaceReads": c.workspace_reads,
                })),
                "spawns": s.spawns,
                "deaths": s.deaths,
                "completed": s.completed,
                "backoffMs": s.backoff_ms,
                "lastFailure": s.last_failure.as_ref().map(|(o, m)| json!({"outcome": o.as_str(), "message": m})),
                "stderrTail": s.stderr_tail,
            })
        })
        .collect();
    engines.sort_by(|a, b| {
        (a["workspace"].as_str(), a["universe"].as_str())
            .cmp(&(b["workspace"].as_str(), b["universe"].as_str()))
    });
    json!({
        "kind": "status",
        "server": {
            "pid": std::process::id(),
            "alsemVersion": env!("CARGO_PKG_VERSION"),
            "protocolVersion": *shared.protocol.lock().expect("protocol"),
            "defaultWorkspace": defaults.workspace,
            "defaultUniverse": defaults.universe.as_str(),
            "defaultFreshness": defaults.freshness.as_str(),
            "outputFolder": shared.sink.instance(),
            "outputBytes": shared.sink.written(),
            "engineMode": "one-shot",
        },
        "engines": engines,
    })
}

/// A `tools/call` result around `envelope`: its JSON as the first text
/// block (without `stdoutText`), the inline stdout exactly as the second,
/// and the envelope as `structuredContent` when the negotiated version has
/// it. `output`: the CLI's JSON parsed (structured content only).
fn tool_result(shared: &Shared, envelope: &Value, is_error: bool, output: Option<Value>) -> Value {
    let version = *shared.protocol.lock().expect("protocol");
    let mut head = envelope.clone();
    let stdout_text = head
        .as_object_mut()
        .and_then(|m| m.remove("stdoutText"))
        .and_then(|v| v.as_str().map(str::to_string));
    if stdout_text.is_some() {
        head["stdoutText"] = json!("<the next content block>");
    }
    let mut content = vec![json!({"type": "text", "text": head.to_string()})];
    if let Some(text) = stdout_text {
        content.push(json!({"type": "text", "text": text}));
    }
    let mut r = json!({"content": content, "isError": is_error});
    if has_structured(version) {
        let mut structured = envelope.clone();
        if let Some(o) = output {
            structured["output"] = o;
        }
        r["structuredContent"] = structured;
    }
    r
}

/// Bytes of a file the engine spilled, checked against its length and
/// hash, the file then removed.
fn take_spill(f: &SpillFile) -> Result<Vec<u8>, String> {
    let bytes = std::fs::read(&f.path).map_err(|e| format!("{}: {e}", f.path))?;
    if bytes.len() as u64 != f.bytes || crate::engine::ids::sha256_bytes_hex(&bytes) != f.sha256 {
        return Err(format!(
            "{}: length or hash differs from the answer",
            f.path
        ));
    }
    let _ = std::fs::remove_file(&f.path);
    Ok(bytes)
}

/// The value of `--<flag>=` in `argv`.
fn flag_value<'a>(argv: &'a [String], flag: &str) -> Option<&'a str> {
    let prefix = format!("--{flag}=");
    argv.iter()
        .take_while(|a| *a != "--")
        .find_map(|a| a.strip_prefix(&prefix))
}

/// A digest's summary: what an agent needs to decide whether to open the
/// file.
fn digest_summary(bytes: &[u8], json_format: bool) -> Value {
    if !json_format {
        let text = String::from_utf8_lossy(bytes);
        return json!({"format": "human", "lines": text.lines().count()});
    }
    let Ok(doc) = serde_json::from_slice::<Value>(bytes) else {
        return json!({"format": "json", "note": "the output is not JSON"});
    };
    const KEPT: usize = 50;
    let entries = doc["payload"]["entries"].as_array();
    let listed: Vec<Value> = entries
        .into_iter()
        .flatten()
        .take(KEPT)
        .map(|e| {
            let r = &e["routine"];
            json!({
                "routine": format!(
                    "{}::{}",
                    r["objectDisplay"].as_str().unwrap_or("?"),
                    r["display"].as_str().unwrap_or("?")
                ),
                "coverage": e["coverage"]["status"],
                "effects": e["effects"].as_array().map_or(0, Vec::len),
                "unresolved": e["unresolved"],
            })
        })
        .collect();
    let count = |v: &Value| v.as_array().map_or(0, Vec::len);
    json!({
        "format": "json",
        "schemaVersion": doc["schemaVersion"],
        "summary": doc["payload"]["summary"],
        "diagnostics": count(&doc["diagnostics"]),
        "changedRootsDiagnostics": count(&doc["payload"]["changedRootsDiagnostics"]),
        "entriesTotal": entries.map_or(0, Vec::len),
        "entries": listed,
    })
}

/// The extension of a file holding `argv`'s stdout.
fn extension(argv: &[String]) -> &'static str {
    match flag_value(argv, "format") {
        Some("json") => "json",
        _ => "txt",
    }
}

/// An infrastructure outcome answered instead of the CLI's.
type Refusal = (FailedOutcome, String);

/// Move a stdout the call must deliver as a file (an `out`, a digest, or a
/// stdout the engine spilled) into the output folder; the bytes come back
/// when they were materialized. `Err`: the outcome to answer instead
/// (`refusedPath` or `engineError`).
fn place_stdout(
    shared: &Shared,
    call: &CommandCall,
    stdout: Stdout,
) -> Result<(Stdout, Option<Vec<u8>>), Refusal> {
    let wants_file = call.out.is_some() || call.always_file;
    let was_file = matches!(stdout, Stdout::File(_));
    if !wants_file && !was_file {
        return Ok((stdout, None));
    }
    let bytes = match stdout {
        Stdout::Text(t) => t.into_bytes(),
        Stdout::File(f) => take_spill(&f).map_err(|e| (FailedOutcome::EngineError, e))?,
    };
    let refused = |e: SinkError| match e {
        SinkError::Refused(m) => (FailedOutcome::RefusedPath, m),
        SinkError::Io(m) => (
            FailedOutcome::RefusedPath,
            format!("writing the output: {m}"),
        ),
    };
    // Off Unix the sink writes nothing: a digest stays inline when it can.
    if !shared.sink.writable() && call.out.is_none() && !was_file {
        let text = String::from_utf8(bytes.clone()).expect("an inline stdout is UTF-8");
        return Ok((Stdout::Text(text), Some(bytes)));
    }
    let path = match &call.out {
        Some(out) => shared.sink.write_out(out, &bytes).map_err(refused)?,
        None => {
            let stem = call.tool.trim_start_matches("alsem_");
            shared
                .sink
                .write_new(stem, extension(&call.argv), &bytes)
                .map_err(refused)?
        }
    };
    let file = SpillFile::of(path.to_string_lossy().into_owned(), &bytes);
    Ok((Stdout::File(file), Some(bytes)))
}

/// A stderr or diagnostic file the engine spilled, moved into the output
/// folder.
fn place_spill(shared: &Shared, f: SpillFile, ext: &str) -> Result<SpillFile, String> {
    let bytes = take_spill(&f)?;
    let path = shared
        .sink
        .write_new("engine", ext, &bytes)
        .map_err(|e| e.to_string())?;
    Ok(SpillFile::of(path.to_string_lossy().into_owned(), &bytes))
}

/// One command call through the engine, answered as one bounded frame.
fn run_command(
    shared: &Shared,
    front: &mut EngineFront,
    id: &Value,
    call: &CommandCall,
    progress: Option<&Value>,
    queued: Instant,
) -> Vec<u8> {
    let queued_ms = queued.elapsed().as_millis();
    let notify = |p: u32, message: &str| {
        if let Some(token) = progress {
            shared.out.send(&json!({
                "jsonrpc": "2.0",
                "method": "notifications/progress",
                "params": {"progressToken": token, "progress": p, "total": 1, "message": message},
            }));
        }
    };
    notify(0, &format!("{}: asking the engine", call.tool));
    let decisions = shared.decide.decide(&call.argv, call.diff_text.as_deref());
    let started = Instant::now();
    let answer = front.ask(AskParams {
        argv: call.argv.clone(),
        diff_text: call.diff_text.clone(),
        decisions,
        max_inline_bytes: call.max_inline_bytes,
    });
    let compute_ms = started.elapsed().as_millis();
    let revision = answer.revision.clone();
    let mut summary = None;
    let mut parsed = None;
    let outcome = match answer.outcome {
        Outcome::Cli {
            exit_code,
            stdout,
            stderr,
            changed_paths,
            changed_paths_truncated,
        } => {
            let placed = place_stdout(shared, call, stdout).and_then(|(stdout, bytes)| {
                let stderr = match stderr {
                    Stderr::File(f) => Stderr::File(
                        place_spill(shared, f, "stderr")
                            .map_err(|e| (FailedOutcome::RefusedPath, e))?,
                    ),
                    lines => lines,
                };
                Ok((stdout, stderr, bytes))
            });
            match placed {
                Ok((stdout, stderr, bytes)) => {
                    let json_format = flag_value(&call.argv, "format") == Some("json");
                    if call.always_file {
                        summary = bytes.as_deref().map(|b| digest_summary(b, json_format));
                    }
                    if let (Stdout::Text(t), true) = (&stdout, json_format) {
                        parsed = serde_json::from_str::<Value>(t).ok();
                    }
                    Outcome::Cli {
                        exit_code,
                        stdout,
                        stderr,
                        changed_paths,
                        changed_paths_truncated,
                    }
                }
                Err((o, m)) => Outcome::failed(o, m),
            }
        }
        Outcome::Failed {
            outcome,
            message,
            diagnostic_file,
        } => Outcome::Failed {
            outcome,
            message,
            diagnostic_file: diagnostic_file
                .and_then(|f| place_spill(shared, f, "diagnostic").ok()),
        },
    };
    notify(1, &format!("{}: answered", call.tool));
    let answer = Answer {
        id: id.clone(),
        outcome,
        revision,
    };
    let extras = json!({
        "universe": call.universe.as_str(),
        "freshness": {"requested": call.freshness.as_str(), "served": "fresh"},
        "timing": {
            "queuedMs": queued_ms,
            "waitedForBuildMs": null,
            "sharedFillMs": null,
            "computeMs": compute_ms,
        },
    });
    let wrap = |a: &Answer| {
        let mut envelope = serde_json::to_value(a).unwrap_or(Value::Null);
        let is_error = envelope["kind"] == "failed";
        if let Some(m) = envelope.as_object_mut() {
            m.remove("id");
            if !is_error {
                m.entry("stale").or_insert(json!(false));
                m.entry("changedPaths").or_insert(json!([]));
                m.entry("changedPathsTruncated").or_insert(json!(false));
            }
            if let Some(e) = extras.as_object() {
                for (k, v) in e {
                    m.insert(k.clone(), v.clone());
                }
            }
            if let (Some(s), false) = (&summary, is_error) {
                m.insert("summary".into(), s.clone());
            }
        }
        let output = if is_error { None } else { parsed.clone() };
        result(id, tool_result(shared, &envelope, is_error, output))
    };
    let spill = |kind: SpillKind, bytes: &[u8]| -> std::io::Result<SpillFile> {
        let ext = match kind {
            SpillKind::Stdout => "stdout",
            SpillKind::Stderr => "stderr",
            SpillKind::Diagnostic => "diagnostic",
        };
        let path = shared
            .sink
            .write_new("answer", ext, bytes)
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        Ok(SpillFile::of(path.to_string_lossy().into_owned(), bytes))
    };
    bound_frame(answer, MAX_FRAME_BYTES, &wrap, &spill)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The id rules (design r7 §8.1): numbers and strings are ids; `null`,
    /// objects and booleans are invalid requests answered with a null id;
    /// a line that is not JSON is a parse error; a batch is refused.
    #[test]
    fn classify_follows_the_id_rules() {
        let req = |s: &str| classify(s.as_bytes());
        assert!(matches!(
            req(r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#),
            Incoming::Request { id, .. } if id == json!(1)
        ));
        assert!(matches!(
            req(r#"{"jsonrpc":"2.0","id":"a","method":"ping"}"#),
            Incoming::Request { id, .. } if id == json!("a")
        ));
        for bad in [
            r#"{"jsonrpc":"2.0","id":null,"method":"ping"}"#,
            r#"{"jsonrpc":"2.0","id":{"x":1},"method":"ping"}"#,
            r#"{"jsonrpc":"2.0","id":true,"method":"ping"}"#,
        ] {
            let Incoming::Invalid(e) = req(bad) else {
                panic!("{bad}")
            };
            assert_eq!(e["error"]["code"], codes::INVALID_REQUEST, "{bad}");
            assert_eq!(e["id"], Value::Null, "{bad}");
        }
        let Incoming::Invalid(e) = req("not json") else {
            panic!()
        };
        assert_eq!(e["error"]["code"], codes::PARSE_ERROR);
        let Incoming::Invalid(e) = req("[]") else {
            panic!()
        };
        assert_eq!(e["error"]["code"], codes::INVALID_REQUEST);
        assert_eq!(
            req(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#),
            Incoming::Notification {
                method: "notifications/initialized".into()
            }
        );
        assert_eq!(
            req(r#"{"jsonrpc":"2.0","id":1,"result":{}}"#),
            Incoming::Response
        );
    }

    /// The client's version is echoed when supported; anything else,
    /// a missing one included, gets the newest.
    #[test]
    fn versions_are_negotiated() {
        for v in SUPPORTED_PROTOCOLS {
            assert_eq!(negotiate(&json!({"protocolVersion": v})), *v);
        }
        assert_eq!(
            negotiate(&json!({"protocolVersion": "2099-01-01"})),
            "2025-06-18"
        );
        assert_eq!(negotiate(&json!({})), "2025-06-18");
        assert_eq!(negotiate(&json!({"protocolVersion": 7})), "2025-06-18");
        assert!(has_structured("2025-06-18"));
        assert!(!has_structured("2025-03-26"));
        assert!(!has_structured("2024-11-05"));
    }
}

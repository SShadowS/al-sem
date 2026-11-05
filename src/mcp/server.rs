//! The MCP server loop: JSON-RPC 2.0 over NDJSON on stdio (design r7 §8.1),
//! the lifecycle state machine, and the MCP [`Transport`] of the engine
//! [`Pool`] (one worker thread per (workspace, universe) that owns that
//! pair's engine child).
//!
//! The main thread reads requests and answers the light ones itself
//! (`initialize`, `ping`, `tools/list`, `alsem_status`, every error); a
//! `tools/call` of a command or of `alsem_refresh` is queued on its
//! engine's worker, which answers it when done, so a long build never
//! blocks `ping` or a call on another workspace. Responses are written one
//! whole line at a time under one lock.

use std::io::{BufRead, Write};
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};

use super::output::OutputSink;
use super::pool::{ChildConfig, Pool, PoolConfig, Request, Transport};
use super::tools::{self, Defaults, EngineCall, ToolCall, Universe};
use super::{DEFAULT_PROTOCOL, Decide, McpConfig, SUPPORTED_PROTOCOLS, codes};
use crate::wire::{ReadFrame, encode_frame, read_frame};

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
        params: Value,
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
            params: m.get("params").cloned().unwrap_or(Value::Null),
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

/// The MCP side of the engine pool: a result is a `tools/call` result, a
/// cancelled call is not answered (MCP), progress is
/// `notifications/progress` with the call's token.
struct McpTransport {
    out: Out,
    protocol: Mutex<&'static str>,
}

impl McpTransport {
    fn version(&self) -> &'static str {
        *self.protocol.lock().expect("protocol")
    }
}

impl Transport for McpTransport {
    fn response(&self, id: &Value, body: &Value, is_error: bool, output: Option<Value>) -> Value {
        result(id, tool_result(self.version(), body, is_error, output))
    }

    fn send(&self, frame: &[u8]) {
        self.out.line(frame);
    }

    /// MCP: the client stopped waiting for a cancelled call; nothing is sent.
    fn cancelled(&self, _id: &Value, _frame: Option<&[u8]>) {}

    fn progress(&self, token: &Value, done: u32, message: &str) {
        self.out.send(&json!({
            "jsonrpc": "2.0",
            "method": "notifications/progress",
            "params": {"progressToken": token, "progress": done, "total": 1, "message": message},
        }));
    }
}

/// Serve MCP on `input` / `output` until the input ends.
pub fn serve(
    cfg: McpConfig,
    decide: Arc<dyn Decide>,
    mut input: impl BufRead,
    output: Box<dyn Write + Send>,
) -> Result<(), String> {
    let sink = OutputSink::create(&cfg.output_root, &super::output::new_instance_id())?;
    let children = ChildConfig {
        budgets: (
            cfg.child_budget(Universe::CrossApp),
            cfg.child_budget(Universe::SingleApp),
        ),
        low_mem: cfg.low_mem,
        idle_timeout_secs: cfg.idle_timeout_secs,
        max_children: cfg.max_children.max(1),
    };
    let out = Out(Arc::new(Mutex::new(output)));
    let transport = Arc::new(McpTransport {
        out: out.clone(),
        protocol: Mutex::new(DEFAULT_PROTOCOL),
    });
    let pool = Pool::new(
        PoolConfig {
            program: cfg.program,
            child_env: cfg.child_env,
            children,
            sink,
            decide,
        },
        Arc::clone(&transport) as Arc<dyn Transport>,
    );
    let defaults = cfg.defaults;
    let mut state = State::Uninitialized;
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
            Incoming::Notification { method, params } => {
                match method.as_str() {
                    "notifications/initialized" if state == State::Initializing => {
                        state = State::Ready;
                    }
                    // MCP cancellation: a queued call is dropped, a running
                    // one is sent `session/cancel`; neither is answered (the
                    // client stopped waiting for it).
                    "notifications/cancelled" => {
                        if let Some(rid) = params.get("requestId") {
                            pool.cancel(rid);
                        }
                    }
                    _ => {}
                }
                continue;
            }
            Incoming::Request { id, method, params } => (id, method, params),
        };
        if pool.in_flight(&id) {
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
                *transport.protocol.lock().expect("protocol") = version;
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
                match call {
                    ToolCall::Status(c) => {
                        let body = status(&transport, &pool, &c, &defaults);
                        out.send(&transport.response(&id, &body, false, None));
                    }
                    ToolCall::Refresh(c) => pool.submit(id, Request::Refresh(c)),
                    ToolCall::Command(call) => {
                        pool.submit(id, Request::Command { call, progress });
                    }
                }
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
    pool.shutdown(false);
    // An instance folder nothing was written to is removed (only if empty).
    let _ = std::fs::remove_dir(pool.sink().instance());
    Ok(())
}

/// `alsem_status`: the server and every engine (or the named one), from
/// the workers' published snapshots; starts nothing.
fn status(transport: &McpTransport, pool: &Pool, call: &EngineCall, defaults: &Defaults) -> Value {
    let children = pool.children();
    // MCP output step 2: what this answer does NOT establish, and what to
    // do next, first (the reviews read the revision as a disk check).
    let next = match defaults.freshness.as_str() {
        "fresh" => {
            "nothing needed: the next command (freshness `fresh`, the default) checks the files \
             and refreshes first; alsem_refresh only prepares later `latest-built` answers"
        }
        _ => {
            "ask a command with freshness `fresh` or `verify` to check the files, or call \
             alsem_refresh to rebuild"
        }
    };
    json!({
        "kind": "status",
        "diskChecked": false,
        "freshnessNote": "This call does not read the workspace files: each engine's revision \
            names the inputs it was BUILT from, not whether they still match the disk.",
        "next": next,
        "server": {
            "pid": std::process::id(),
            "alsemVersion": env!("CARGO_PKG_VERSION"),
            "protocolVersion": transport.version(),
            "defaultWorkspace": defaults.workspace,
            "defaultUniverse": defaults.universe.as_str(),
            "defaultFreshness": defaults.freshness.as_str(),
            "outputFolder": pool.sink().instance(),
            "outputBytes": pool.sink().written(),
            "engineMode": "resident",
            "maxChildren": children.max_children,
            "childBudgetBytes": {
                "crossApp": children.budgets.0,
                "singleApp": children.budgets.1,
            },
            "idleTimeoutSecs": children.idle_timeout_secs,
            "lowMem": children.low_mem,
        },
        "engines": pool.engines(call),
    })
}

/// A `tools/call` result around `envelope`: its JSON as the first text
/// block (without `stdoutText`), the inline stdout exactly as the second,
/// and the envelope as `structuredContent` when the negotiated `version`
/// has it. `output`: the CLI's JSON parsed (structured content only).
///
/// One copy of the answer in what the client reads (MCP output plan 1c, P3/E1,
/// E6). Measured 2026-11-04 through Claude Code: with `structuredContent`
/// present, the client reads and size-checks `structuredContent` only (a
/// result whose text blocks held 46,167 characters and whose
/// `structuredContent` held 83,840 was reported as 82,195 characters and
/// saved to a file). So `structuredContent` carries the answer exactly once:
/// `output` (the CLI's JSON, parsed) when there is one, else `stdoutText`.
/// The text blocks keep the byte contract: the envelope without `stdoutText`
/// (no placeholder), then the inline stdout exactly.
fn tool_result(version: &str, envelope: &Value, is_error: bool, output: Option<Value>) -> Value {
    let mut head = envelope.clone();
    let stdout_text = head
        .as_object_mut()
        .and_then(|m| m.remove("stdoutText"))
        .and_then(|v| v.as_str().map(str::to_string));
    let mut content = vec![json!({"type": "text", "text": head.to_string()})];
    if let Some(text) = stdout_text {
        content.push(json!({"type": "text", "text": text}));
    }
    let mut r = json!({"content": content, "isError": is_error});
    if has_structured(version) {
        let structured = match output {
            Some(o) => {
                let mut s = head;
                s["output"] = o;
                s
            }
            None => envelope.clone(),
        };
        r["structuredContent"] = structured;
    }
    r
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
                method: "notifications/initialized".into(),
                params: Value::Null,
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

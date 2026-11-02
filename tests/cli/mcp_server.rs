//! `alsem mcp` end to end (context-server design r7 §8, §10, §11): an MCP
//! client drives the server over stdio (NDJSON JSON-RPC 2.0) through the
//! lifecycle, lists the tools, and calls each one; every command answer's
//! stdout is the CLI's own stdout byte for byte (decoded `stdoutText`, its
//! trailing newline included, or the bytes of `stdoutFile`), with the CLI's
//! exit code. Lifecycle errors (`-32002` before `initialize`, `-32601` for
//! `server/discover` in every state, `-32600` for bad, duplicate and
//! repeated requests), a semantic build failure kept as a CLI outcome, an
//! engine child killed mid-call (reported, then respawned), digest's file
//! plus summary, and refused output paths are each pinned here.
//!
//! The fault tests need the child's test hooks (`ALSEM_ENGINE_TEST_HOOK`,
//! debug builds or the `engine-test-hooks` feature); the hook is set on the
//! server process only, which its engine children inherit.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use serde_json::{Value, json};

const ALSEM: &str = env!("CARGO_BIN_EXE_alsem");

fn ws_explain() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/r0-corpus/ws-explain")
}

/// `alsem <argv>` as a process: (stdout, stderr, exit).
fn cli(argv: &[&str]) -> (Vec<u8>, Vec<u8>, u8) {
    cli_stdin(argv, None)
}

/// `alsem <argv>` fed `stdin`.
fn cli_stdin(argv: &[&str], stdin: Option<&str>) -> (Vec<u8>, Vec<u8>, u8) {
    let mut child = Command::new(ALSEM)
        .args(argv)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("alsem runs");
    let input = child.stdin.take().unwrap();
    if let Some(text) = stdin {
        let mut input = input;
        input.write_all(text.as_bytes()).unwrap();
    }
    let out = child.wait_with_output().unwrap();
    let code = u8::try_from(out.status.code().expect("an exit code")).unwrap();
    (out.stdout, out.stderr, code)
}

/// One `alsem mcp` server and its client end.
struct Server {
    child: Child,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
    /// Notifications read while waiting for responses.
    notifications: Vec<Value>,
    _root: tempfile::TempDir,
    root: PathBuf,
}

impl Server {
    fn start(workspace: &Path, hook: Option<&str>) -> Server {
        let root = tempfile::tempdir().unwrap();
        let mut cmd = Command::new(ALSEM);
        cmd.args(["mcp", "--workspace"])
            .arg(workspace)
            .arg("--output-root")
            .arg(root.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .env_remove("ALSEM_ENGINE_TEST_HOOK");
        if let Some(h) = hook {
            cmd.env("ALSEM_ENGINE_TEST_HOOK", h);
        }
        let mut child = cmd.spawn().expect("alsem mcp starts");
        let input = child.stdin.take().unwrap();
        let output = BufReader::new(child.stdout.take().unwrap());
        Server {
            child,
            input,
            output,
            notifications: Vec::new(),
            root: std::fs::canonicalize(root.path()).unwrap(),
            _root: root,
        }
    }

    /// A server initialized at `2025-06-18` and ready.
    fn ready(workspace: &Path, hook: Option<&str>) -> Server {
        let mut s = Server::start(workspace, hook);
        let r = s.request(json!(0), "initialize", json!({"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "test", "version": "1"}}));
        assert_eq!(r["result"]["protocolVersion"], "2025-06-18", "{r}");
        s.notify("notifications/initialized", json!({}));
        s
    }

    fn line(&mut self, v: &Value) {
        let mut text = v.to_string();
        text.push('\n');
        self.raw(text.as_bytes());
    }

    fn raw(&mut self, bytes: &[u8]) {
        self.input.write_all(bytes).unwrap();
        self.input.flush().unwrap();
    }

    fn notify(&mut self, method: &str, params: Value) {
        self.line(&json!({"jsonrpc": "2.0", "method": method, "params": params}));
    }

    fn send(&mut self, id: Value, method: &str, params: Value) {
        self.line(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
    }

    /// The next message with an id (a response), keeping notifications.
    fn response(&mut self) -> Value {
        loop {
            let mut line = String::new();
            let n = self.output.read_line(&mut line).unwrap();
            assert!(n > 0, "the server closed its stdout");
            assert!(line.ends_with('\n'), "one line per message");
            let v: Value = serde_json::from_str(&line).expect("each line is JSON");
            assert_eq!(v["jsonrpc"], "2.0", "{v}");
            if v.get("id").is_some() {
                return v;
            }
            self.notifications.push(v);
        }
    }

    fn request(&mut self, id: Value, method: &str, params: Value) -> Value {
        self.send(id.clone(), method, params);
        let r = self.response();
        assert_eq!(r["id"], id, "{r}");
        r
    }

    fn call(&mut self, id: i64, tool: &str, args: Value) -> Value {
        let r = self.request(
            json!(id),
            "tools/call",
            json!({"name": tool, "arguments": args}),
        );
        assert!(r.get("error").is_none(), "{tool}: {r}");
        r["result"].clone()
    }

    /// Close stdin and wait for the server's exit.
    fn finish(mut self) -> std::process::ExitStatus {
        drop(self.input);
        self.child.wait().unwrap()
    }
}

/// A tools/call result's envelope (its `structuredContent`), checked
/// against its text blocks: the first is the envelope's JSON without
/// `stdoutText`, the second (when the stdout is inline) the stdout exactly.
fn envelope(result: &Value) -> Value {
    let env = result["structuredContent"].clone();
    let content = result["content"].as_array().expect("content blocks");
    let head: Value = serde_json::from_str(content[0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(head["kind"], env["kind"]);
    if let Some(text) = env["stdoutText"].as_str() {
        assert_eq!(content.len(), 2);
        assert_eq!(content[1]["text"].as_str(), Some(text));
    }
    assert_eq!(
        result["isError"],
        json!(env["kind"] == "failed"),
        "isError iff kind failed: {result}"
    );
    assert!(env.get("revision").is_some(), "revision is always present");
    env
}

/// A cli envelope's (stdout bytes, stderr bytes, exit).
fn cli_bytes(env: &Value) -> (Vec<u8>, Vec<u8>, u8) {
    assert_eq!(env["kind"], "cli", "{env}");
    let stdout = match (env.get("stdoutText"), env.get("stdoutFile")) {
        (Some(Value::String(t)), None) => t.clone().into_bytes(),
        (None, Some(f)) => file_bytes(f),
        _ => panic!("stdoutText XOR stdoutFile: {env}"),
    };
    let stderr = match (env.get("stderr"), env.get("stderrFile")) {
        (Some(Value::Array(lines)), None) => lines
            .iter()
            .flat_map(|l| format!("{}\n", l.as_str().unwrap()).into_bytes())
            .collect(),
        (None, Some(f)) => file_bytes(f),
        _ => panic!("stderr XOR stderrFile: {env}"),
    };
    let exit = u8::try_from(env["exitCode"].as_u64().unwrap()).unwrap();
    (stdout, stderr, exit)
}

/// A spill file's bytes, checked against its length and hash.
fn file_bytes(f: &Value) -> Vec<u8> {
    let b = std::fs::read(f["path"].as_str().unwrap()).expect("the file exists");
    assert_eq!(b.len() as u64, f["bytes"].as_u64().unwrap());
    assert_eq!(
        al_sem::engine::ids::sha256_bytes_hex(&b),
        f["sha256"].as_str().unwrap()
    );
    b
}

const DIFF: &str = "--- a/src/post.al\n+++ b/src/post.al\n@@ -14,5 +14,5 @@\n         if IsHandled then\n             exit;\n-        Order.Posted := false;\n+        Order.Posted := true;\n         Order.Validate(Status, Order.Status::Posted);\n         Order.Modify(true);\n";

/// The transcript: lifecycle, tools/list, then each command tool, every
/// answer the CLI's own stdout byte for byte with its exit code, digest as
/// a file with a summary, a query over a small inline cap as a file, and
/// `out` inside the output folder.
#[test]
fn a_transcript_answers_what_the_cli_answers() {
    let ws = ws_explain();
    let w = ws.to_str().unwrap();
    let mut s = Server::start(&ws, None);

    // Before initialize: only initialize, ping and server/discover.
    let r = s.request(json!("d0"), "server/discover", json!({}));
    assert_eq!(r["error"]["code"], -32601, "{r}");
    let r = s.request(json!(1), "tools/list", json!({}));
    assert_eq!(r["error"]["code"], -32002, "{r}");
    let r = s.request(json!(2), "tools/call", json!({"name": "alsem_status"}));
    assert_eq!(r["error"]["code"], -32002, "{r}");
    let r = s.request(json!(3), "ping", json!({}));
    assert_eq!(r["result"], json!({}), "{r}");

    let r = s.request(
        json!(4),
        "initialize",
        json!({"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "t", "version": "1"}}),
    );
    assert_eq!(r["result"]["protocolVersion"], "2025-06-18");
    assert_eq!(r["result"]["serverInfo"]["name"], "alsem");
    assert!(r["result"]["capabilities"]["tools"].is_object());
    // Initializing: still no tools until `initialized`.
    let r = s.request(json!(5), "tools/list", json!({}));
    assert_eq!(r["error"]["code"], -32002, "{r}");
    let r = s.request(json!(6), "initialize", json!({}));
    assert_eq!(r["error"]["code"], -32600, "initialize twice: {r}");
    s.notify("notifications/initialized", json!({}));
    let r = s.request(json!("d1"), "server/discover", json!({}));
    assert_eq!(r["error"]["code"], -32601, "{r}");
    let r = s.request(json!(7), "initialize", json!({}));
    assert_eq!(r["error"]["code"], -32600, "{r}");

    let r = s.request(json!(8), "tools/list", json!({}));
    let tools = r["result"]["tools"].as_array().unwrap();
    let names: Vec<_> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert_eq!(
        names,
        [
            "alsem_explain",
            "alsem_prove",
            "alsem_digest",
            "alsem_query_effects",
            "alsem_query_touches",
            "alsem_status",
            "alsem_refresh"
        ]
    );
    let explain = &tools[0]["inputSchema"];
    assert_eq!(explain["type"], "object");
    assert_eq!(explain["properties"]["routines"]["minItems"], 1);
    assert_eq!(
        explain["properties"]["format"]["enum"],
        json!(["human", "json"])
    );
    assert_eq!(
        tools[1]["inputSchema"]["required"],
        json!(["routine", "question"])
    );

    // Each command tool against the CLI.
    let cases: Vec<(&str, Value, Vec<&str>, Option<&str>)> = vec![
        (
            "alsem_explain",
            json!({"routines": ["Archive"]}),
            vec!["explain", w, "--routine", "Archive"],
            None,
        ),
        (
            "alsem_explain",
            json!({"routines": ["Archive", "PostOrder"], "format": "json"}),
            vec![
                "explain",
                w,
                "--routine",
                "Archive",
                "--routine",
                "PostOrder",
                "--format",
                "json",
            ],
            None,
        ),
        (
            "alsem_explain",
            json!({"diff": DIFF, "format": "json"}),
            vec!["explain", w, "--diff", "-", "--format", "json"],
            Some(DIFF),
        ),
        (
            "alsem_explain",
            json!({"table": "Explain Order", "field": "Status"}),
            vec![
                "explain",
                w,
                "--table",
                "Explain Order",
                "--field",
                "Status",
            ],
            None,
        ),
        (
            "alsem_prove",
            json!({"routine": "PostOrder", "question": "may-commit", "deterministic": true}),
            vec!["prove", w, "PostOrder", "may-commit", "--deterministic"],
            None,
        ),
        (
            "alsem_query_effects",
            json!({"routine": "PostOrder", "deterministic": true}),
            vec![
                "query",
                "effects",
                w,
                "--routine",
                "PostOrder",
                "--deterministic",
            ],
            None,
        ),
        (
            "alsem_query_touches",
            json!({"table": "50400", "deterministic": true}),
            vec!["query", "touches", w, "--table", "50400", "--deterministic"],
            None,
        ),
        (
            "alsem_query_touches",
            json!({"table": "50400", "from": "PostOrder", "direction": "down", "format": "json", "deterministic": true}),
            vec![
                "query",
                "touches",
                w,
                "--table",
                "50400",
                "--from",
                "PostOrder",
                "--direction",
                "down",
                "--format",
                "json",
                "--deterministic",
            ],
            None,
        ),
    ];
    let mut id = 100;
    for (tool, args, argv, stdin) in &cases {
        id += 1;
        let expected = cli_stdin(argv, *stdin);
        assert!(
            expected.0.ends_with(b"\n"),
            "{argv:?}: a CLI output ends its line"
        );
        let env = envelope(&s.call(id, tool, args.clone()));
        assert!(
            env["stdoutText"].is_string(),
            "{tool}: inline under the cap: {env}"
        );
        assert_eq!(cli_bytes(&env), expected, "{tool} {args}");
        assert_eq!(env["universe"], "cross-app");
        assert_eq!(env["stale"], false);
        assert_eq!(env["revision"], Value::Null, "one-shot child: no revision");
    }

    // JSON output is also given parsed, as `output`.
    let env = s.call(
        120,
        "alsem_explain",
        json!({"routines": ["Archive"], "format": "json"}),
    );
    assert_eq!(
        env["structuredContent"]["output"]["schema"], "al-sem.explain/1",
        "{env}"
    );

    // Digest: always a file in the output folder, with a summary.
    let argv = ["digest", w, "--routine", "PostOrder", "--deterministic"];
    let expected = cli(&argv);
    let env = envelope(&s.call(
        130,
        "alsem_digest",
        json!({"routines": ["PostOrder"], "deterministic": true}),
    ));
    let file = &env["stdoutFile"];
    assert!(env.get("stdoutText").is_none(), "{env}");
    assert!(
        Path::new(file["path"].as_str().unwrap()).starts_with(&s.root),
        "{env}"
    );
    assert_eq!(cli_bytes(&env), expected);
    assert_eq!(env["summary"]["summary"]["rootsResolved"], 1, "{env}");
    assert_eq!(env["summary"]["entriesTotal"], 1, "{env}");
    assert_eq!(
        env["summary"]["entries"][0]["routine"],
        "Codeunit Explain Post::PostOrder"
    );

    // A stdout over the inline cap: the same bytes, as a file.
    let argv = ["query", "touches", w, "--table", "50400", "--deterministic"];
    let expected = cli(&argv);
    let env = envelope(&s.call(
        131,
        "alsem_query_touches",
        json!({"table": "50400", "deterministic": true, "maxInlineBytes": 16}),
    ));
    assert!(env["stdoutFile"].is_object(), "{env}");
    assert!(Path::new(env["stdoutFile"]["path"].as_str().unwrap()).starts_with(&s.root));
    assert_eq!(cli_bytes(&env), expected);

    // `out`: written inside the output folder, the same bytes.
    let argv = ["prove", w, "PostOrder", "may-commit", "--deterministic"];
    let expected = cli(&argv);
    let env = envelope(&s.call(
        132,
        "alsem_prove",
        json!({"routine": "PostOrder", "question": "may-commit", "deterministic": true, "out": "answers/prove.json"}),
    ));
    let path = PathBuf::from(env["stdoutFile"]["path"].as_str().unwrap());
    assert!(
        path.starts_with(&s.root) && path.ends_with("answers/prove.json"),
        "{env}"
    );
    assert_eq!(cli_bytes(&env), expected);
    // ... and again: the server's own file may be overwritten.
    let env = envelope(&s.call(
        133,
        "alsem_prove",
        json!({"routine": "PostOrder", "question": "may-commit", "deterministic": true, "out": path.to_str().unwrap()}),
    ));
    assert_eq!(cli_bytes(&env), expected);

    // Status shows the engine, and starts nothing new.
    let status = s.call(140, "alsem_status", json!({}));
    let st = &status["structuredContent"];
    assert_eq!(st["kind"], "status", "{status}");
    let engines = st["engines"].as_array().unwrap();
    assert_eq!(engines.len(), 1, "{status}");
    assert_eq!(
        engines[0]["spawns"], 1,
        "one child answered every call: {status}"
    );
    assert_eq!(engines[0]["deaths"], 0);
    assert!(engines[0]["engine"]["pid"].is_number());
    let refresh = s.call(141, "alsem_refresh", json!({}));
    assert_eq!(refresh["structuredContent"]["kind"], "refresh", "{refresh}");
    assert_eq!(refresh["isError"], false);

    assert!(s.notifications.is_empty(), "no progress without a token");
    assert!(s.finish().success());
}

/// Version negotiation: a supported version is echoed, an unsupported or
/// missing one gets 2025-06-18; an older version gets text blocks only, the
/// stdout exact in the second block.
#[test]
fn versions_are_negotiated_and_older_ones_get_text_only() {
    let ws = ws_explain();
    for (asked, answered) in [
        (json!({"protocolVersion": "1999-01-01"}), "2025-06-18"),
        (json!({}), "2025-06-18"),
        (json!({"protocolVersion": "2025-03-26"}), "2025-03-26"),
        (json!({"protocolVersion": "2024-11-05"}), "2024-11-05"),
    ] {
        let mut s = Server::start(&ws, None);
        let r = s.request(json!(1), "initialize", asked.clone());
        assert_eq!(r["result"]["protocolVersion"], answered, "{asked}: {r}");
        s.notify("notifications/initialized", json!({}));
        if answered != "2025-06-18" {
            let w = ws.to_str().unwrap();
            let expected = cli(&["explain", w, "--routine", "Archive"]);
            let r = s.call(2, "alsem_explain", json!({"routines": ["Archive"]}));
            assert!(r.get("structuredContent").is_none(), "{answered}: {r}");
            let content = r["content"].as_array().unwrap();
            assert_eq!(content[1]["text"].as_str().unwrap().as_bytes(), expected.0);
            let head: Value = serde_json::from_str(content[0]["text"].as_str().unwrap()).unwrap();
            assert_eq!(head["exitCode"], 0);
        }
        assert!(s.finish().success());
    }
}

/// Invalid requests (design r7 §8.1): `null`, object and boolean ids, a
/// line that is not JSON, a batch, an unknown method, an unknown tool and
/// invalid arguments each get their error, and the server goes on.
#[test]
fn invalid_requests_are_refused_and_the_server_goes_on() {
    let ws = ws_explain();
    let mut s = Server::ready(&ws, None);
    for (line, code) in [
        (r#"{"jsonrpc":"2.0","id":null,"method":"ping"}"#, -32600),
        (r#"{"jsonrpc":"2.0","id":{"a":1},"method":"ping"}"#, -32600),
        (r#"{"jsonrpc":"2.0","id":true,"method":"ping"}"#, -32600),
        ("this is not json", -32700),
        (r#"[{"jsonrpc":"2.0","id":9,"method":"ping"}]"#, -32600),
        (r#"{"jsonrpc":"1.0","id":9,"method":"ping"}"#, -32600),
    ] {
        s.raw(format!("{line}\n").as_bytes());
        let r = s.response();
        assert_eq!(r["error"]["code"], code, "{line}: {r}");
    }
    let r = s.request(json!(10), "resources/list", json!({}));
    assert_eq!(r["error"]["code"], -32601, "{r}");
    let r = s.request(json!(11), "tools/call", json!({"name": "alsem_nope"}));
    assert_eq!(r["error"]["code"], -32602, "{r}");
    for args in [
        json!({"routines": ["A"], "diff": "x"}),
        json!({"routines": []}),
        json!({"table": "T"}),
        json!({"routines": ["A"], "bogus": true}),
    ] {
        let r = s.request(
            json!(12),
            "tools/call",
            json!({"name": "alsem_explain", "arguments": args}),
        );
        assert_eq!(r["error"]["code"], -32602, "{args}: {r}");
    }
    let r = s.request(json!(13), "ping", json!({}));
    assert_eq!(r["result"], json!({}));
    assert!(s.finish().success());
}

/// A semantic build failure (a workspace with no app.json) is the
/// command's own CLI answer: `kind: "cli"`, `isError: false`, the CLI's
/// bytes and non-zero exit.
#[test]
fn a_semantic_build_failure_stays_a_cli_outcome() {
    let broken = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(broken.path().join("src")).unwrap();
    std::fs::write(broken.path().join("src/a.al"), "codeunit 50100 A { }").unwrap();
    let b = broken.path().to_str().unwrap();
    let expected = cli(&["explain", b, "--routine", "A", "--format", "json"]);
    assert_ne!(expected.2, 0, "precondition: the CLI fails it");
    let mut s = Server::ready(broken.path(), None);
    let r = s.call(
        1,
        "alsem_explain",
        json!({"routines": ["A"], "format": "json"}),
    );
    let env = envelope(&r);
    assert_eq!(r["isError"], false, "{r}");
    assert_eq!(cli_bytes(&env), expected);
    assert!(s.finish().success());
}

/// Output paths outside the server's folder are refused (`refusedPath`),
/// and nothing is written there.
#[test]
fn output_paths_outside_the_folder_are_refused() {
    let ws = ws_explain();
    let mut s = Server::ready(&ws, None);
    let elsewhere = tempfile::tempdir().unwrap();
    let target = elsewhere.path().join("x.json");
    for out in ["../escape.json", target.to_str().unwrap()] {
        let r = s.call(
            1,
            "alsem_prove",
            json!({"routine": "PostOrder", "question": "may-commit", "out": out}),
        );
        let env = envelope(&r);
        assert_eq!(env["kind"], "failed", "{out}: {r}");
        assert_eq!(env["outcome"], "refusedPath", "{out}: {r}");
        assert_eq!(r["isError"], true);
    }
    assert!(!target.exists());
    // `..` from the instance folder would be the output root itself.
    assert!(!s.root.join("escape.json").exists());
    assert!(s.finish().success());
}

/// Progress notifications only with a progressToken, each carrying it.
#[test]
fn progress_only_with_a_token() {
    let ws = ws_explain();
    let mut s = Server::ready(&ws, None);
    s.send(
        json!(1),
        "tools/call",
        json!({"name": "alsem_prove", "arguments": {"routine": "PostOrder", "question": "may-commit"}, "_meta": {"progressToken": "tok"}}),
    );
    let r = s.response();
    assert_eq!(r["result"]["isError"], false, "{r}");
    assert!(!s.notifications.is_empty(), "progress was sent");
    for n in &s.notifications {
        assert_eq!(n["method"], "notifications/progress");
        assert_eq!(n["params"]["progressToken"], "tok");
    }
    assert!(s.finish().success());
}

#[cfg(any(feature = "engine-test-hooks", debug_assertions))]
mod faults {
    use super::*;

    /// An engine child killed mid-call (it aborts on the ask, as a panic
    /// does under `release-ship`) is reported as `kind: "failed"`,
    /// `engineError`; the next call respawns the child and answers the
    /// CLI's bytes.
    #[test]
    fn a_killed_engine_is_reported_and_the_next_call_respawns() {
        let ws = ws_explain();
        let w = ws.to_str().unwrap();
        let marker = tempfile::tempdir().unwrap();
        let hook = format!("abort-on-ask-once:{}", marker.path().join("died").display());
        let mut s = Server::ready(&ws, Some(&hook));
        let args = json!({"routine": "PostOrder", "question": "may-commit", "deterministic": true});
        let r = s.call(1, "alsem_prove", args.clone());
        let env = envelope(&r);
        assert_eq!(env["kind"], "failed", "{r}");
        assert_eq!(env["outcome"], "engineError", "{r}");
        let expected = cli(&["prove", w, "PostOrder", "may-commit", "--deterministic"]);
        let env = envelope(&s.call(2, "alsem_prove", args));
        assert_eq!(cli_bytes(&env), expected);
        let st = s.call(3, "alsem_status", json!({}))["structuredContent"].clone();
        assert_eq!(st["engines"][0]["spawns"], 2, "{st}");
        assert_eq!(st["engines"][0]["deaths"], 1, "{st}");
        assert!(s.finish().success());
    }

    /// A request id still in flight is refused (`-32600`) while the call
    /// runs; the call itself is then answered once. The child never says
    /// hello, so the call stays in flight until the hello deadline (10 s)
    /// and fails `engineError`; `ping` is answered meanwhile.
    #[test]
    fn a_duplicate_id_in_flight_is_refused() {
        let ws = ws_explain();
        let mut s = Server::ready(&ws, Some("no-hello"));
        s.send(
            json!(7),
            "tools/call",
            json!({"name": "alsem_prove", "arguments": {"routine": "PostOrder", "question": "may-commit"}}),
        );
        let r = s.request(json!(8), "ping", json!({}));
        assert_eq!(r["result"], json!({}), "ping while a call runs: {r}");
        s.send(json!(7), "ping", json!({}));
        let dup = s.response();
        assert_eq!(dup["id"], 7);
        assert_eq!(dup["error"]["code"], -32600, "{dup}");
        let r = s.response();
        assert_eq!(r["id"], 7);
        let env = envelope(&r["result"]);
        assert_eq!(env["outcome"], "engineError", "{r}");
        // No longer in flight: the id may be used again.
        let r = s.request(json!(7), "ping", json!({}));
        assert_eq!(r["result"], json!({}));
        assert!(s.finish().success());
    }
}

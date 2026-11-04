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
pub(crate) struct Server {
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
        Server::start_args(workspace, hook, &[])
    }

    fn start_args(workspace: &Path, hook: Option<&str>, extra: &[&str]) -> Server {
        let root = tempfile::tempdir().unwrap();
        let mut cmd = Command::new(ALSEM);
        cmd.args(["mcp", "--workspace"])
            .arg(workspace)
            .arg("--output-root")
            .arg(root.path())
            .args(extra)
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
    pub(crate) fn ready(workspace: &Path, hook: Option<&str>) -> Server {
        Server::ready_args(workspace, hook, &[])
    }

    pub(crate) fn ready_args(workspace: &Path, hook: Option<&str>, extra: &[&str]) -> Server {
        let mut s = Server::start_args(workspace, hook, extra);
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

    pub(crate) fn call(&mut self, id: i64, tool: &str, args: Value) -> Value {
        let r = self.request(
            json!(id),
            "tools/call",
            json!({"name": tool, "arguments": args}),
        );
        assert!(r.get("error").is_none(), "{tool}: {r}");
        r["result"].clone()
    }

    /// Close stdin and wait for the server's exit.
    pub(crate) fn finish(mut self) -> std::process::ExitStatus {
        drop(self.input);
        self.child.wait().unwrap()
    }
}

/// A tools/call result's envelope (its `structuredContent`), checked
/// against its text blocks: the first is the envelope's JSON without
/// `stdoutText`, the second (when the stdout is inline) the stdout exactly.
///
/// MCP output plan 1c: the first block is the envelope with no `stdoutText`
/// key (no placeholder), and `structuredContent` holds the answer once:
/// `output` (no `stdoutText`) for an inline JSON stdout, else `stdoutText`.
/// The returned envelope is the head plus the inline stdout as `stdoutText`
/// (so [`cli_bytes`] reads the CLI's bytes from the text block).
pub(crate) fn envelope(result: &Value) -> Value {
    let structured = result["structuredContent"].clone();
    let content = result["content"].as_array().expect("content blocks");
    let head: Value = serde_json::from_str(content[0]["text"].as_str().unwrap()).unwrap();
    assert!(head.get("stdoutText").is_none(), "no placeholder: {head}");
    assert_eq!(head["kind"], structured["kind"]);
    let mut env = head.clone();
    if content.len() == 2 {
        let text = content[1]["text"].as_str().unwrap();
        env["stdoutText"] = json!(text);
        if structured.get("output").is_some() {
            assert!(
                structured.get("stdoutText").is_none(),
                "one copy: output XOR stdoutText: {structured}"
            );
            let mut without = structured.clone();
            without.as_object_mut().unwrap().remove("output");
            assert_eq!(without, head, "structuredContent is the head plus output");
        } else {
            assert_eq!(structured, env, "structuredContent is the envelope");
        }
    } else {
        assert_eq!(content.len(), 1);
        assert!(structured.get("stdoutText").is_none(), "{structured}");
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
pub(crate) fn cli_bytes(env: &Value) -> (Vec<u8>, Vec<u8>, u8) {
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
pub(crate) fn file_bytes(f: &Value) -> Vec<u8> {
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
            // The MCP default for prove is human (plan 1c); the CLI's, json.
            vec![
                "prove",
                w,
                "PostOrder",
                "may-commit",
                "--deterministic",
                "--format",
                "human",
            ],
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
        // MCP output plan 1a: the envelope states the line base of the answer.
        // Discrimination (2026-11-04): dropping the insert in `pool.rs`'s
        // `wrap` fails here (`null`); restored, it passes.
        assert_eq!(env["lineBase"], 1, "{tool}: {env}");
        // The resident child: every call off one revision of the workspace.
        assert_eq!(
            env["revision"]["generation"], 1,
            "one resident revision: {env}"
        );
    }

    // JSON output is also given parsed, as `output`.
    let env = s.call(
        120,
        "alsem_explain",
        json!({"routines": ["Archive"], "format": "json"}),
    );
    assert_eq!(
        env["structuredContent"]["output"]["schema"], "al-sem.explain/3",
        "{env}"
    );

    // The output folder (Unix; off Unix it writes nothing and these answers
    // stay inline or are refused: `the_inline_only_path_delivers_inline`).
    if cfg!(unix) {
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
        // The renderer's summary (MCP output plan 1b).
        assert_eq!(env["summary"]["kind"], "digest", "{env}");
        assert_eq!(env["summary"]["rootsResolved"], 1, "{env}");
        assert_eq!(env["summary"]["routinesListed"], 1, "{env}");
        assert_eq!(
            env["summary"]["routines"][0]["routine"],
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
        let argv = [
            "prove",
            w,
            "PostOrder",
            "may-commit",
            "--deterministic",
            "--format",
            "human",
        ];
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
    }

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
    // `alsem_refresh` is `session/refresh`: with `wait`, the child's
    // revision after its freshness check (nothing changed: generation 1);
    // without, `{started: true}` at once. DISCRIMINATION (2026-11-02): not
    // calling `front.refresh` (the stage-2 reset-only refresh) fails the
    // generation assertion (`engine: {}`).
    let waited = s.call(142, "alsem_refresh", json!({"wait": true}));
    let engine = &waited["structuredContent"]["engine"];
    assert_eq!(engine["revision"]["generation"], 1, "{waited}");
    assert!(engine["wallMs"].is_number(), "{waited}");
    let started = s.call(143, "alsem_refresh", json!({"wait": false}));
    assert_eq!(
        started["structuredContent"]["engine"]["started"], true,
        "{started}"
    );

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

/// What a Windows client gets (design §10, fail-closed: no handle-relative
/// creation, so neither the server's output folder nor the child's spill
/// directory writes anything; the `no-output-dir` hook takes that path on
/// any platform): a digest comes back inline (`stdoutText`, no summary: the
/// answer is there), a stdout over the request's inline cap comes back inline, every
/// answer is the CLI's bytes, an `out` is refused (`refusedPath`), and the
/// output folder stays empty.
///
/// DISCRIMINATION (2026-11-02): making `OutputSink::writable` ignore the
/// hook (`imp::WRITABLE` alone) sends the digest to the sink, which refuses
/// it (`refusedPath` "out '0001-digest.json': ... inline only"): the
/// `stdoutText` assertion fails. Not raising the child's inline cap
/// (`ask_inline_only`) fails it too, `refusedPath` from the child.
#[cfg(any(feature = "engine-test-hooks", debug_assertions))]
#[test]
fn the_inline_only_path_delivers_inline() {
    let ws = ws_explain();
    let w = ws.to_str().unwrap();
    let mut s = Server::ready(&ws, Some("no-output-dir"));
    let argv = ["digest", w, "--routine", "PostOrder", "--deterministic"];
    let expected = cli(&argv);
    let env = envelope(&s.call(
        1,
        "alsem_digest",
        json!({"routines": ["PostOrder"], "deterministic": true}),
    ));
    assert!(env["stdoutText"].is_string(), "{env}");
    assert!(env.get("stdoutFile").is_none(), "{env}");
    assert_eq!(cli_bytes(&env), expected);
    // MCP output plan 1b/1e: a summary goes with a file only; inline, the
    // answer itself is there.
    assert!(env.get("summary").is_none(), "{env}");
    let argv = ["query", "touches", w, "--table", "50400", "--deterministic"];
    let expected = cli(&argv);
    let env = envelope(&s.call(
        2,
        "alsem_query_touches",
        json!({"table": "50400", "deterministic": true, "maxInlineBytes": 16}),
    ));
    assert!(env["stdoutText"].is_string(), "{env}");
    assert_eq!(cli_bytes(&env), expected);
    let r = s.call(
        3,
        "alsem_prove",
        json!({"routine": "PostOrder", "question": "may-commit", "out": "answers/prove.json"}),
    );
    let env = envelope(&r);
    assert_eq!(env["outcome"], "refusedPath", "{r}");
    assert!(
        env["message"]
            .as_str()
            .is_some_and(|m| m.contains("inline only")),
        "{r}"
    );
    let written: Vec<_> = walk_files(&s.root);
    assert!(written.is_empty(), "nothing written: {written:?}");
    assert!(s.finish().success());
}

/// Every file under `dir`, recursively.
#[cfg(any(feature = "engine-test-hooks", debug_assertions))]
fn walk_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        if p.is_dir() {
            out.extend(walk_files(&p));
        } else {
            out.push(p);
        }
    }
    out
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

/// `--max-children` and `--total-budget` (design §7): with one child
/// allowed, a call on a second workspace ends the first workspace's idle
/// child before starting its own; a call back on the first starts a new
/// one. Each child is started with its share of the total budget.
///
/// DISCRIMINATION (2026-11-02): skipping the bound in `dispatch` (`if
/// live.len() >= usize::MAX`) fails "the first child was ended" (two
/// children run at once).
#[test]
fn the_children_bound_ends_the_least_recently_used_idle_child() {
    let ws = ws_explain();
    let other = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/r0-corpus/ws-d37");
    let mut s = Server::ready_args(&ws, None, &["--max-children", "1", "--total-budget", "3G"]);
    let q = json!({"routine": "Archive"});
    assert_eq!(
        envelope(&s.call(1, "alsem_query_effects", q.clone()))["kind"],
        "cli"
    );
    let first_pid = |st: &Value| -> Option<i64> {
        st["engines"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["workspace"].as_str() == ws.to_str())
            .and_then(|e| e["engine"]["pid"].as_i64())
    };
    let st = s.call(2, "alsem_status", json!({}))["structuredContent"].clone();
    let pid = first_pid(&st).expect("the first child runs");
    assert_eq!(st["server"]["maxChildren"], 1, "{st}");
    assert_eq!(
        st["server"]["childBudgetBytes"]["crossApp"],
        3u64 << 30,
        "its share of --total-budget: {st}"
    );
    let r = s.call(
        3,
        "alsem_query_effects",
        json!({"routine": "ForwardedAfterValidate", "workspace": other.to_str().unwrap()}),
    );
    let _ = envelope(&r);
    // The first child was ended (graceful shutdown, not a death).
    let started = std::time::Instant::now();
    loop {
        let alive = std::path::Path::new(&format!("/proc/{pid}")).exists()
            && !std::fs::read_to_string(format!("/proc/{pid}/stat"))
                .unwrap_or_default()
                .contains(") Z ");
        if !alive {
            break;
        }
        assert!(
            started.elapsed() < std::time::Duration::from_secs(20),
            "the first child was ended"
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    // Back on the first workspace: a new child answers.
    assert_eq!(
        envelope(&s.call(4, "alsem_query_effects", q))["kind"],
        "cli"
    );
    let st = s.call(5, "alsem_status", json!({}))["structuredContent"].clone();
    assert_ne!(first_pid(&st), Some(pid), "{st}");
    assert!(s.finish().success());
}

/// MCP output plan 3c (S2): `alsem_status` answers the freshness questions
/// per engine: the revision answering (its `generation` and `inputsStamp`,
/// the same the last answer's envelope names), the last build (class, start,
/// duration), whether a build runs now (idle here), and the child's memory,
/// read live from `/proc` on Linux. After a waited `alsem_refresh` the
/// status shows the refreshed counters, not the ones of the last call.
///
/// DISCRIMINATION (2026-11-05): dropping the `last_build_started_unix_ms`
/// stamp in `build_locked` fails at "startedAtUnixMs"; publishing a waited
/// refresh without reading the child's status (`publish(front, snapshot,
/// false)`) fails at "builds after the refresh" (its fresh check is not
/// counted); each restored, it passes.
#[test]
fn status_states_each_engines_freshness() {
    let ws = ws_explain();
    let mut s = Server::ready(&ws, None);
    let before = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    let st = s.call(1, "alsem_status", json!({}))["structuredContent"].clone();
    assert_eq!(st["engines"], json!([]), "starts nothing: {st}");
    let env = envelope(&s.call(
        2,
        "alsem_query_effects",
        json!({"routine": "Archive", "deterministic": true}),
    ));
    let st = s.call(3, "alsem_status", json!({}))["structuredContent"].clone();
    let f = &st["engines"][0]["freshness"];
    assert_eq!(f["revision"]["generation"], 1, "{st}");
    assert_eq!(
        f["revision"]["inputsStamp"], env["revision"]["inputsStamp"],
        "the revision the answer named: {st}"
    );
    assert!(
        ["cold", "P", "S", "B"].contains(&f["lastBuild"]["class"].as_str().unwrap_or("")),
        "{st}"
    );
    assert_eq!(f["lastBuild"]["builds"], 1, "{st}");
    assert!(f["lastBuild"]["durationMs"].is_u64(), "{st}");
    let started = f["lastBuild"]["startedAtUnixMs"]
        .as_u64()
        .unwrap_or_else(|| panic!("startedAtUnixMs: {st}"));
    assert!(started >= before, "started during this test: {st}");
    assert_eq!(f["activity"], "idle", "{st}");
    assert_eq!(f["building"], false, "{st}");
    assert!(f["countersAgeMs"].is_u64(), "{st}");
    if cfg!(target_os = "linux") {
        assert_eq!(f["rssSource"], "live", "{st}");
        assert!(f["rssBytes"].as_u64().unwrap() > 1 << 20, "{st}");
        assert!(f["peakRssBytes"].as_u64() >= f["rssBytes"].as_u64(), "{st}");
    }
    // A waited refresh: the counters are read after it.
    let r = s.call(4, "alsem_refresh", json!({"wait": true}));
    assert_eq!(r["isError"], false, "{r}");
    let st = s.call(5, "alsem_status", json!({}))["structuredContent"].clone();
    let f = &st["engines"][0]["freshness"];
    assert!(
        f["countersAgeMs"].as_u64().is_some_and(|ms| ms < 60_000),
        "{st}"
    );
    assert_eq!(f["revision"]["generation"], 1, "nothing changed: {st}");
    assert_eq!(f["activity"], "idle", "{st}");
    // A waited refresh with nothing changed checks freshness, builds nothing.
    assert_eq!(
        f["lastBuild"]["builds"], 1,
        "builds after the refresh: {st}"
    );
    assert!(
        f["checks"]["freshHits"].as_u64().is_some_and(|n| n >= 1),
        "builds after the refresh: the refresh's freshness check is counted: {st}"
    );
    assert!(s.finish().success());
}

#[cfg(any(feature = "engine-test-hooks", debug_assertions))]
mod faults {
    use super::*;

    /// MCP output plan 3c: while a call is in the engine's `pin` phase (its
    /// freshness check and build) `alsem_status` says so (`activity: pin`,
    /// `building: true`, the running tool named), and idle again after.
    /// The child sleeps 3 s right after reporting `pin`.
    ///
    /// DISCRIMINATION (2026-11-05): not recording the child's phase in
    /// `run_command` (the `on_phase` closure not storing it) fails at
    /// "activity pin" (`asking`); restored, it passes.
    #[test]
    fn status_shows_a_build_in_progress() {
        let ws = ws_explain();
        let mut s = Server::ready(&ws, Some("sleep-in-phase:pin:3000"));
        s.send(
            json!(7),
            "tools/call",
            json!({"name": "alsem_query_effects", "arguments": {"routine": "Archive"}}),
        );
        std::thread::sleep(std::time::Duration::from_millis(1000));
        let st = s.call(8, "alsem_status", json!({}))["structuredContent"].clone();
        let e = &st["engines"][0];
        assert_eq!(e["running"], "alsem_query_effects", "{st}");
        assert_eq!(e["freshness"]["activity"], "pin", "activity pin: {st}");
        assert_eq!(e["freshness"]["building"], true, "{st}");
        assert!(e["freshness"]["activityMs"].is_u64(), "{st}");
        let r = s.response();
        assert_eq!(r["id"], 7, "{r}");
        let st = s.call(9, "alsem_status", json!({}))["structuredContent"].clone();
        let f = &st["engines"][0]["freshness"];
        assert_eq!(
            (&f["activity"], &f["building"]),
            (&json!("idle"), &json!(false)),
            "{st}"
        );
        assert_eq!(f["revision"]["generation"], 1, "{st}");
        assert!(s.finish().success());
    }

    /// MCP cancellation (design §8, `notifications/cancelled`): a running
    /// call is sent `session/cancel` (the child stops it at its next phase)
    /// and is never answered, as MCP asks; its id is free again at once
    /// after, and the next call is answered normally. The child sleeps 1.5 s
    /// on each ask, so the cancel lands while the call runs.
    ///
    /// DISCRIMINATION (2026-11-02): ignoring the notification
    /// (`flag.store(false, ..)` in `serve`) fails it: the cancelled call is
    /// answered (its human text, `kind: "cli"`) where the next call's JSON
    /// was expected.
    #[test]
    fn a_cancelled_call_is_stopped_and_not_answered() {
        let ws = ws_explain();
        let mut s = Server::ready(&ws, Some("sleep-on-ask:1500"));
        s.send(
            json!(7),
            "tools/call",
            json!({"name": "alsem_query_effects", "arguments": {"routine": "Archive"}}),
        );
        std::thread::sleep(std::time::Duration::from_millis(500));
        s.notify(
            "notifications/cancelled",
            json!({"requestId": 7, "reason": "test"}),
        );
        // The next response is the ping's, not the cancelled call's.
        let r = s.request(json!(8), "ping", json!({}));
        assert_eq!(r["result"], json!({}), "{r}");
        let st = s.call(9, "alsem_status", json!({}));
        assert!(st["structuredContent"]["engines"].is_array(), "{st}");
        // Wait for the child to answer the cancelled ask (and the server
        // to drop it), then reuse its id: a normal answer, the first
        // response with id 7.
        std::thread::sleep(std::time::Duration::from_millis(2500));
        let env = envelope(&s.call(
            7,
            "alsem_query_effects",
            json!({"routine": "Archive", "format": "json"}),
        ));
        assert_eq!(env["kind"], "cli", "{env}");
        assert!(
            env["stdoutText"]
                .as_str()
                .is_some_and(|t| t.starts_with('{')),
            "the second call's JSON, not the first call's human text: {env}"
        );
        assert!(s.finish().success());
    }

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
        let expected = cli(&[
            "prove",
            w,
            "PostOrder",
            "may-commit",
            "--deterministic",
            "--format",
            "human",
        ]);
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

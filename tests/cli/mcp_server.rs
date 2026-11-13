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

    pub(crate) fn notify(&mut self, method: &str, params: Value) {
        self.line(&json!({"jsonrpc": "2.0", "method": method, "params": params}));
    }

    pub(crate) fn send(&mut self, id: Value, method: &str, params: Value) {
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

    pub(crate) fn request(&mut self, id: Value, method: &str, params: Value) -> Value {
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
            "alsem_tests",
            "alsem_impact",
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
        // Output validation §6.1: the front end passes `--surface=mcp`; the
        // CLI given the same flag answers the same bytes.
        let mut argv = argv.clone();
        argv.push("--surface=mcp");
        let expected = cli_stdin(&argv, *stdin);
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
        env["structuredContent"]["output"]["schema"], "al-sem.explain/4",
        "{env}"
    );

    // The output folder (Unix; off Unix it writes nothing and these answers
    // stay inline or are refused: `the_inline_only_path_delivers_inline`).
    if cfg!(unix) {
        // Digest: always a file in the output folder, with a summary.
        let argv = [
            "digest",
            w,
            "--routine",
            "PostOrder",
            "--deterministic",
            "--surface=mcp",
        ];
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
        // Round 2 G10: a compact row names its result-index row.
        assert_eq!(env["summary"]["routines"][0]["index"], 1, "{env}");
        assert_eq!(
            env["summary"]["routines"][0]["selector"],
            "Codeunit \"Explain Post\"::PostOrder"
        );

        // A stdout over the inline cap: the same bytes, as a file.
        let argv = [
            "query",
            "touches",
            w,
            "--table",
            "50400",
            "--deterministic",
            "--surface=mcp",
        ];
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
            "--surface=mcp",
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
    // Output validation §5.4 (F33): one copy — the text block is the
    // compact status (headline, check, refresh), the full document is
    // structuredContent.
    let text = status["content"][0]["text"].as_str().unwrap();
    assert!(
        text.starts_with(st["headline"].as_str().unwrap()) && text.len() < 600,
        "{} bytes: {text}",
        text.len()
    );
    assert!(
        text.contains("\n  check: ") && text.contains("\n  refresh: "),
        "{text}"
    );
    let engines = st["engines"].as_array().unwrap();
    assert_eq!(engines.len(), 1, "{status}");
    assert_eq!(
        engines[0]["details"]["spawns"], 1,
        "one child answered every call: {status}"
    );
    assert_eq!(engines[0]["details"]["deaths"], 0);
    assert!(engines[0]["details"]["engine"]["pid"].is_number());
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

/// Output validation §6.1 (F34): the MCP front end appends `--surface=mcp`,
/// so the renderer-owned follow-up text of an MCP answer (`more:` and
/// `next:` lines, fold notes, `re-analysis under an assumption:`, `ask:`)
/// names MCP inputs and tool calls, never a CLI flag or command. Source
/// excerpts, diffs and `---` headings are not checked (they may hold `--`).
///
/// Discrimination (2026-11-06): see the V4 commit message (break: drop the
/// flag from the front end).
#[test]
fn mcp_answers_name_mcp_inputs_not_cli_flags() {
    let ws = ws_explain();
    let mut s = Server::ready(&ws, None);
    let calls = [
        ("alsem_explain", json!({"routines": ["PostOrder"]})),
        (
            "alsem_prove",
            json!({"routine": "PostOrder", "question": "may-commit"}),
        ),
        ("alsem_query_effects", json!({"routine": "PostOrder"})),
        ("alsem_query_touches", json!({"table": "50400"})),
        (
            "alsem_digest",
            json!({"routines": ["PostOrder"], "format": "human"}),
        ),
    ];
    let follow_up_line = |l: &str| {
        let t = l.trim_start();
        t.starts_with("more:")
            || t.starts_with("next:")
            || t.starts_with("ask:")
            || t.starts_with("re-analysis under an assumption:")
            || l.contains(" lists them")
            || l.contains(" lists every")
            || l.contains(" lists up to")
    };
    let mut checked = 0usize;
    let mut failures = Vec::new();
    for (i, (tool, args)) in calls.iter().enumerate() {
        let env = envelope(&s.call(300 + i as i64, tool, args.clone()));
        let (stdout, _, code) = cli_bytes(&env);
        assert_eq!(code, 0, "{tool}: {env}");
        let text = String::from_utf8(stdout).unwrap();
        assert!(text.contains("\nmore: "), "precondition: {tool}: {text}");
        for l in text.lines().filter(|l| follow_up_line(l)) {
            checked += 1;
            let words = al_sem::engine::follow_up::cli_words_in(l);
            if !words.is_empty() {
                failures.push(format!("{tool}: {words:?} in: {l}"));
            }
        }
    }
    assert!(checked >= 5, "precondition: {checked} lines");
    assert!(failures.is_empty(), "{}", failures.join("\n"));
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
            let expected = cli(&["explain", w, "--routine", "Archive", "--surface=mcp"]);
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
    let expected = cli(&[
        "explain",
        b,
        "--routine",
        "A",
        "--format",
        "json",
        "--surface=mcp",
    ]);
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
    let argv = [
        "digest",
        w,
        "--routine",
        "PostOrder",
        "--deterministic",
        "--surface=mcp",
        // The indented document (round-2 G35: `compact: false`), so the
        // answer is over the summary cap.
        "--compact=false",
    ];
    let expected = cli(&argv);
    let env = envelope(&s.call(
        1,
        "alsem_digest",
        json!({"routines": ["PostOrder"], "deterministic": true, "compact": false}),
    ));
    assert!(env["stdoutText"].is_string(), "{env}");
    assert!(env.get("stdoutFile").is_none(), "{env}");
    assert_eq!(cli_bytes(&env), expected);
    // MCP output plan 1b/1e, round 2 G12: inline, the answer itself is
    // there; a JSON answer over 8 KiB carries the summary too, with no index
    // file (the result is inline).
    let big = env["stdoutText"].as_str().unwrap().len()
        > al_sem::engine::batch::output::SUMMARY_CAP_BYTES;
    assert!(big, "the fixture's digest JSON is over 8 KiB: {env}");
    assert_eq!(env["summary"]["kind"], "digest", "{env}");
    assert!(env["summary"].get("index").is_none(), "{env}");
    assert!(env["summary"].get("indexRows").is_none(), "{env}");
    let argv = [
        "query",
        "touches",
        w,
        "--table",
        "50400",
        "--deterministic",
        "--surface=mcp",
    ];
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
            .and_then(|e| e["details"]["engine"]["pid"].as_i64())
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
    let f = &st["engines"][0]["details"]["freshness"];
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
    let f = &st["engines"][0]["details"]["freshness"];
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

/// A copy of `ws-explain` a test may edit.
fn ws_explain_copy() -> tempfile::TempDir {
    fn copy(from: &Path, to: &Path) {
        std::fs::create_dir_all(to).unwrap();
        for e in std::fs::read_dir(from).unwrap() {
            let e = e.unwrap();
            if e.file_type().unwrap().is_dir() {
                copy(&e.path(), &to.join(e.file_name()));
            } else {
                std::fs::copy(e.path(), to.join(e.file_name())).unwrap();
            }
        }
    }
    let t = tempfile::tempdir().unwrap();
    copy(&ws_explain(), t.path());
    t
}

/// Agent-tasks design §2 (F-1, F-2, F-5, the refresh state): `alsem_status`
/// checks the disk against the revision the engine published. No engine:
/// nothing checked (`noEngine.check: null`, the "nothing was checked"
/// headline). After a call: FRESH at generation 1. After an edit: STALE
/// with the file, its category and the uncertain class, twice: the check
/// builds nothing (the waited refresh after it is the one that rebuilds,
/// to generation 2), and it reads the disk each time, not the last
/// answer. An unwaited refresh is `queued` until a later call shows it
/// completed.
///
/// DISCRIMINATION (2026-11-05): answering status without observing the
/// disk against the published descriptor (the rows' descriptors dropped
/// before `status_body`) fails at "fresh after the call" (`check: null`);
/// letting the status pin the engine (a fresh ask, run through
/// `pool.run_composite` before the rows are read) fails at "stale after the
/// edit" (the status built generation 2, then found it fresh). Each
/// restored, it passes.
#[test]
fn status_checks_the_disk_against_the_published_revision() {
    let dir = ws_explain_copy();
    let ws = std::fs::canonicalize(dir.path()).unwrap();
    let w = ws.to_str().unwrap();
    let mut s = Server::ready(&ws, None);
    let st = s.call(1, "alsem_status", json!({}))["structuredContent"].clone();
    assert_eq!(st["statusVersion"], 2, "{st}");
    assert_eq!(st["engines"], json!([]), "{st}");
    assert_eq!(st["noEngine"]["check"], Value::Null, "{st}");
    assert_eq!(
        st["headline"],
        format!(
            "no engine is running for {w} (cross-app), so nothing was checked. An analysis call builds it (about 25 s cross-app on CDO), for instance alsem_explain {{\"routines\": [\"<routine>\"]}}; a status call builds nothing."
        ),
        "{st}"
    );
    let env = envelope(&s.call(2, "alsem_query_effects", json!({"routine": "Archive"})));
    assert_eq!(env["revision"]["generation"], 1, "{env}");
    let st = s.call(3, "alsem_status", json!({}))["structuredContent"].clone();
    let e = &st["engines"][0];
    assert_eq!(e["check"]["fresh"], true, "fresh after the call: {st}");
    assert_eq!(e["check"]["complete"], true, "{st}");
    assert_eq!(e["check"]["hashing"], "cached", "{st}");
    assert_eq!(e["check"]["checkedRevision"]["generation"], 1, "{st}");
    assert_eq!(
        e["check"]["checkedRevision"]["inputsStamp"], env["revision"]["inputsStamp"],
        "{st}"
    );
    // Output validation V2 (§4.5, F22): the headline says how hard it
    // checked. Discrimination (2026-11-06): `strength` set to "hashed" for
    // every check fails the cached headline below; restored, it passes.
    assert_eq!(e["check"]["strength"], "size-and-time", "{st}");
    assert!(
        st["headline"].as_str().unwrap().starts_with(&format!(
            "cross-app engine for {w}: FRESH: no change seen since generation 1 (size and modification time, just now; verify: true hashes every input)."
        )),
        "{st}"
    );
    let st = s.call(30, "alsem_status", json!({"verify": true}))["structuredContent"].clone();
    let hashed = st["engines"][0]["check"]["filesHashed"].as_u64().unwrap();
    assert!(hashed > 0, "precondition: the verify check hashed: {st}");
    assert!(
        st["headline"].as_str().unwrap().starts_with(&format!(
            "cross-app engine for {w}: FRESH: no input changed since generation 1 (every input hashed just now: {hashed} inputs)."
        )),
        "{st}"
    );
    assert_eq!(e["refresh"]["state"], "idle", "{st}");

    let post = ws.join("src/post.al");
    let text = std::fs::read_to_string(&post).unwrap();
    std::fs::write(
        &post,
        text.replace(
            "        Order.Modify();\n        Commit();",
            "        Order.Modify(true);\n        Commit();",
        ),
    )
    .unwrap();
    for id in [4, 5] {
        let st = s.call(id, "alsem_status", json!({}))["structuredContent"].clone();
        let c = &st["engines"][0]["check"];
        assert_eq!(c["fresh"], false, "stale after the edit: {st}");
        assert_eq!(
            c["changes"],
            json!([{"category": "al-file", "path": post.to_str().unwrap()}]),
            "{st}"
        );
        assert_eq!(c["refreshClass"]["class"], "BodyOnly|Declaration", "{st}");
        assert_eq!(c["checkedRevision"]["generation"], 1, "{st}");
        assert!(
            st["headline"].as_str().unwrap().starts_with(&format!(
                "cross-app engine for {w}: STALE. 1 .al file changed on disk since generation 1. Refresh class: BodyOnly or Declaration"
            )),
            "{st}"
        );
    }
    let r = s.call(6, "alsem_refresh", json!({"wait": true}))["structuredContent"].clone();
    assert_eq!(r["did"], "rebuilt", "the refresh rebuilt: {r}");
    assert_eq!(r["class"], "BodyOnly", "{r}");
    assert_eq!(r["generation"], 2, "{r}");
    assert_eq!(r["files"], 1, "{r}");
    let st = s.call(7, "alsem_status", json!({}))["structuredContent"].clone();
    let e = &st["engines"][0];
    assert_eq!(e["check"]["fresh"], true, "{st}");
    assert_eq!(e["check"]["checkedRevision"]["generation"], 2, "{st}");
    assert_eq!(e["refresh"]["state"], "completed", "{st}");
    assert_eq!(e["refresh"]["did"], "rebuilt", "{st}");
    let r = s.call(8, "alsem_refresh", json!({"wait": true}))["structuredContent"].clone();
    assert_eq!(r["did"], "already-fresh", "{r}");

    // An unwaited refresh: queued until a later call shows its outcome.
    std::fs::write(&post, &text).unwrap();
    let r = s.call(9, "alsem_refresh", json!({"wait": false}))["structuredContent"].clone();
    assert_eq!(r["did"], "queued", "{r}");
    let st = s.call(10, "alsem_status", json!({}))["structuredContent"].clone();
    assert_eq!(st["engines"][0]["refresh"]["state"], "queued", "{st}");
    let _ = envelope(&s.call(11, "alsem_query_effects", json!({"routine": "Archive"})));
    let st = s.call(12, "alsem_status", json!({}))["structuredContent"].clone();
    let e = &st["engines"][0];
    assert_eq!(e["refresh"]["state"], "completed", "{st}");
    assert_eq!(e["refresh"]["did"], "rebuilt", "{st}");
    assert_eq!(e["check"]["fresh"], true, "{st}");
    assert_eq!(e["check"]["checkedRevision"]["generation"], 3, "{st}");
    assert!(s.finish().success());
}

/// F-8 (agent-tasks design §2.3): the `freshness` description says what
/// each mode does, and `verify` does it. A rewrite that keeps a file's size
/// and modification time is trusted by `fresh` (the answer stays at
/// generation 1, and a cached status says FRESH: the stated residual) and
/// seen by `verify`: a status with `verify: true` says STALE, and a command
/// asked `verify` rebuilds first (generation 2) and answers the new text.
///
/// DISCRIMINATION (2026-11-05): mapping the tool's `verify` to the
/// session's `Fresh` in `run_command` (the old description's "answer from
/// the built revision and report whether it is stale": no hash of every
/// input) fails at "verify rebuilds"; restored, it passes.
#[test]
fn a_stale_verify_rebuilds_as_its_description_says() {
    let r = s_tools_freshness_description();
    assert_eq!(r, al_sem::mcp::tools::FRESHNESS_DESCRIPTION);
    assert!(
        r.contains("verify: like fresh, but hashes every input instead of trusting size and modification time; rebuilds first when anything changed."),
        "{r}"
    );

    let dir = ws_explain_copy();
    let ws = std::fs::canonicalize(dir.path()).unwrap();
    let mut s = Server::ready(&ws, None);
    let q = json!({"routine": "Archive", "format": "json"});
    let env = envelope(&s.call(1, "alsem_query_effects", q.clone()));
    assert_eq!(env["revision"]["generation"], 1, "{env}");
    let post = ws.join("src/post.al");
    let before = std::fs::metadata(&post).unwrap();
    let text = std::fs::read_to_string(&post).unwrap();
    let (old, new) = (
        "        Order.Modify();\n        Commit();",
        "        Order.Insert();\n        Commit();",
    );
    assert_eq!(
        text.matches(old).count(),
        1,
        "precondition: the edit applies"
    );
    std::fs::write(&post, text.replace(old, new)).unwrap();
    std::fs::File::options()
        .write(true)
        .open(&post)
        .unwrap()
        .set_modified(before.modified().unwrap())
        .unwrap();
    let after = std::fs::metadata(&post).unwrap();
    assert_eq!(
        (after.len(), after.modified().unwrap()),
        (before.len(), before.modified().unwrap()),
        "precondition: size and modification time kept"
    );
    let env = envelope(&s.call(2, "alsem_query_effects", q.clone()));
    assert_eq!(
        env["revision"]["generation"], 1,
        "fresh trusts the stat: {env}"
    );
    // Engine limits wave 0 (B8): the answer that may be stale says which
    // check it passed and what that check cannot see, never plainly
    // `fresh`. DISCRIMINATION (2026-11-09): `answer_freshness` serving
    // `fresh` for a stat check (the old header) fails here; restored,
    // passes.
    assert_eq!(
        env["freshness"],
        json!({
            "requested": "fresh",
            "served": "fresh-by-size-and-time",
            "residual": "a rewrite that keeps a file's size and modification time is seen only \
                         with freshness: verify; unsaved editor buffers are not inputs",
        }),
        "the header names the residual: {env}"
    );
    let st = s.call(3, "alsem_status", json!({}))["structuredContent"].clone();
    assert_eq!(
        st["engines"][0]["check"]["fresh"], true,
        "the residual: {st}"
    );
    let st = s.call(4, "alsem_status", json!({"verify": true}))["structuredContent"].clone();
    let c = &st["engines"][0]["check"];
    assert_eq!(c["hashing"], "all", "{st}");
    assert_eq!(c["fresh"], false, "verify sees it: {st}");
    let mut v = q.clone();
    v["freshness"] = json!("verify");
    let env = envelope(&s.call(5, "alsem_query_effects", v));
    assert_eq!(env["revision"]["generation"], 2, "verify rebuilds: {env}");
    assert_eq!(env["freshness"]["served"], "verified", "{env}");
    assert!(
        env["stdoutText"].as_str().unwrap().contains("Insert"),
        "the new text answers: {env}"
    );
    assert!(s.finish().success());
}

/// The `freshness` property's description, as `tools/list` gives it.
fn s_tools_freshness_description() -> String {
    let tools = al_sem::mcp::tools::tool_list();
    let explain = tools
        .iter()
        .find(|t| t["name"] == "alsem_explain")
        .unwrap()
        .clone();
    explain["inputSchema"]["properties"]["freshness"]["description"]
        .as_str()
        .unwrap()
        .to_string()
}

#[cfg(any(feature = "engine-test-hooks", debug_assertions))]
mod faults {
    use super::*;

    /// Asks `alsem_status` (ids from `first_id` up) until `ok` holds on its
    /// `structuredContent`, and returns that answer. A fixed sleep before one
    /// status read raced a loaded host (TLS-39); this waits for the state, and
    /// a server that never reaches it still fails: when the running call is
    /// answered first (the state was missed for its whole run), or at the 30 s
    /// deadline.
    fn status_until(
        s: &mut Server,
        first_id: i64,
        what: &str,
        ok: impl Fn(&Value) -> bool,
    ) -> Value {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        let mut id = first_id;
        loop {
            s.send(
                json!(id),
                "tools/call",
                json!({"name": "alsem_status", "arguments": {}}),
            );
            let r = s.response();
            assert_eq!(r["id"], id, "{what}: never seen while the call ran: {r}");
            let st = r["result"]["structuredContent"].clone();
            if ok(&st) {
                return st;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "{what} within 30 s: {st}"
            );
            id += 1;
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }

    /// F-3 (agent-tasks design §2.5): `alsem_status` answers with a complete
    /// disk check while a heavy `alsem_explain` runs on the same engine (its
    /// child sleeps 3 s on every ask), and names the running call; the
    /// explain is answered after it.
    ///
    /// "While it runs" is checked in the same run, against the heavy call
    /// itself (TLS-15): from the status request, the status must answer in
    /// under a quarter of the time the explain still takes (about 2.5 s of
    /// its sleep remain; an idle status answers in tens of ms). A fixed
    /// 200 ms bound failed at 223-227 ms on a saturated host (2026-11-11),
    /// where host load lengthens both times alike.
    ///
    /// DISCRIMINATION (2026-11-05): making the status wait for the engine's
    /// running call before it reads the worker's snapshot (as a check
    /// through the busy front end would: `engine_rows` waiting while
    /// `running_tool` is set) fails at "answered first"; restored, it
    /// passes. A status slowed past the ratio but still answered first
    /// fails at "during the heavy call" (TLS-15).
    #[test]
    fn status_answers_during_a_heavy_call() {
        let ws = ws_explain();
        let mut s = Server::ready(&ws, Some("sleep-on-ask:3000"));
        let _ = envelope(&s.call(1, "alsem_query_effects", json!({"routine": "Archive"})));
        s.send(
            json!(7),
            "tools/call",
            json!({"name": "alsem_explain", "arguments": {"routines": ["Archive"]}}),
        );
        status_until(&mut s, 100, "the explain running", |st| {
            st["engines"][0]["details"]["running"] == "alsem_explain"
        });
        let start = std::time::Instant::now();
        s.send(
            json!(8),
            "tools/call",
            json!({"name": "alsem_status", "arguments": {}}),
        );
        let r = s.response();
        let status = start.elapsed();
        assert_eq!(r["id"], 8, "answered first: {r}");
        let st = &r["result"]["structuredContent"];
        let e = &st["engines"][0];
        assert_eq!(e["details"]["running"], "alsem_explain", "{st}");
        assert_eq!(e["check"]["complete"], true, "{st}");
        assert_eq!(e["check"]["fresh"], true, "{st}");
        let r = s.response();
        let explain = start.elapsed();
        assert_eq!(r["id"], 7, "{r}");
        assert_eq!(envelope(&r["result"])["kind"], "cli", "{r}");
        assert!(
            status * 4 < explain,
            "during the heavy call: status {status:?}, the explain {explain:?} from the same start"
        );
        assert!(s.finish().success());
    }

    /// The reader loop is never held by a check (agent-tasks design §2.2),
    /// and a check past its deadline is `complete: false`, never fresh
    /// (F-9): each check here waits 1.5 s under a 500 ms deadline
    /// (`status-check:1500:500`, read by the MCP process). A ping sent
    /// after the status is answered first; the status then says the check
    /// did not finish.
    ///
    /// DISCRIMINATION (2026-11-05): joining the checker thread on the
    /// reader loop (`h.join()` right after the spawn) fails at "the ping
    /// first"; restored, it passes.
    #[test]
    fn a_slow_check_holds_neither_the_reader_nor_its_deadline() {
        let ws = ws_explain();
        let w = ws.to_str().unwrap();
        let mut s = Server::ready(&ws, Some("status-check:1500:500"));
        let _ = envelope(&s.call(1, "alsem_query_effects", json!({"routine": "Archive"})));
        s.send(
            json!(2),
            "tools/call",
            json!({"name": "alsem_status", "arguments": {}}),
        );
        s.send(json!(3), "ping", json!({}));
        let r = s.response();
        assert_eq!(r["id"], 3, "the ping first: {r}");
        let r = s.response();
        assert_eq!(r["id"], 2, "{r}");
        let st = &r["result"]["structuredContent"];
        let c = &st["engines"][0]["check"];
        assert_eq!(c["complete"], false, "{st}");
        assert_eq!(c["fresh"], false, "{st}");
        assert_eq!(
            st["headline"],
            "cross-app engine: the check did not finish (the check did not finish within 500 ms); freshness unknown.",
            "{st} ({w})"
        );
        assert!(s.finish().success());
    }

    /// MCP output plan 3c: while a call is in the engine's `pin` phase (its
    /// freshness check and build) `alsem_status` says so (`activity: pin`,
    /// `building: true`, the running tool named), and idle again after.
    /// The child sleeps 3 s right after reporting `pin`.
    ///
    /// DISCRIMINATION (2026-11-05): not recording the child's phase in
    /// `run_command` (the `on_phase` closure not storing it) fails at
    /// "activity pin" (`asking`); restored, it passes. Re-proved after TLS-39
    /// (the status is now polled, not read once after a sleep): the same
    /// break fails at "activity pin: never seen while the call ran".
    #[test]
    fn status_shows_a_build_in_progress() {
        let ws = ws_explain();
        let mut s = Server::ready(&ws, Some("sleep-in-phase:pin:3000"));
        s.send(
            json!(7),
            "tools/call",
            json!({"name": "alsem_query_effects", "arguments": {"routine": "Archive"}}),
        );
        let st = status_until(&mut s, 100, "activity pin", |st| {
            st["engines"][0]["details"]["freshness"]["activity"] == "pin"
        });
        let e = &st["engines"][0]["details"];
        assert_eq!(e["running"], "alsem_query_effects", "{st}");
        assert_eq!(e["freshness"]["activity"], "pin", "activity pin: {st}");
        assert_eq!(e["freshness"]["building"], true, "{st}");
        assert!(e["freshness"]["activityMs"].is_u64(), "{st}");
        let r = s.response();
        assert_eq!(r["id"], 7, "{r}");
        let st = s.call(9, "alsem_status", json!({}))["structuredContent"].clone();
        let f = &st["engines"][0]["details"]["freshness"];
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
            "--surface=mcp",
        ]);
        let env = envelope(&s.call(2, "alsem_prove", args));
        assert_eq!(cli_bytes(&env), expected);
        let st = s.call(3, "alsem_status", json!({}))["structuredContent"].clone();
        assert_eq!(st["engines"][0]["details"]["spawns"], 2, "{st}");
        assert_eq!(st["engines"][0]["details"]["deaths"], 1, "{st}");
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

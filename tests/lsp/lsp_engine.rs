//! The context server's LSP front end end to end (design 2026-11-01 §9,
//! §11 T-lsp): the real `al-call-hierarchy` binary over stdio, its
//! `al-call-hierarchy/…` engine requests answered by an `alsem engine-host`
//! child it starts itself (the `alsem` binary next to it, the default).
//!
//! Each test copies the `ws-explain` fixture to a temp folder (it edits it),
//! gives the server a private home (no user config is read), and compares
//! every answer's stdout, stderr and exit with a cold `alsem` CLI run on the
//! same folder, byte for byte. Faults use the child's test hook
//! (`ALSEM_ENGINE_TEST_HOOK=sleep-on-ask:<ms>`, debug builds), set on the
//! server process and inherited by its child.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

const LSP: &str = env!("CARGO_BIN_EXE_al-call-hierarchy");
const ALSEM: &str = env!("CARGO_BIN_EXE_alsem");
const TIMEOUT: Duration = Duration::from_secs(120);

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/r0-corpus/ws-explain")
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap() {
        let e = e.unwrap();
        let target = to.join(e.file_name());
        if e.file_type().unwrap().is_dir() {
            copy_dir(&e.path(), &target);
        } else {
            std::fs::copy(e.path(), target).unwrap();
        }
    }
}

/// A private copy of the fixture: (its temp dir, its path as the server
/// sees it — canonical, so the CLI and the server spell it alike).
fn workspace() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path().canonicalize().unwrap().join("ws");
    copy_dir(&fixture(), &ws);
    (tmp, ws)
}

/// `alsem <argv>`: (stdout, stderr, exit).
fn cli(argv: &[&str]) -> (Vec<u8>, Vec<u8>, u8) {
    let out = Command::new(ALSEM)
        .args(argv)
        .stdin(Stdio::null())
        .output()
        .expect("alsem runs");
    let code = u8::try_from(out.status.code().expect("an exit code")).unwrap();
    (out.stdout, out.stderr, code)
}

/// One `Content-Length` frame, or `None` at EOF.
fn read_frame(out: &mut impl BufRead) -> Option<Value> {
    let mut len = None;
    loop {
        let mut line = String::new();
        if out.read_line(&mut line).unwrap() == 0 {
            return None;
        }
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            break;
        }
        let (name, value) = line.split_once(": ").expect("a header line");
        if name.eq_ignore_ascii_case("Content-Length") {
            len = Some(value.parse::<usize>().unwrap());
        }
    }
    let mut body = vec![0; len.expect("a Content-Length header")];
    out.read_exact(&mut body).unwrap();
    Some(serde_json::from_slice(&body).expect("a JSON body"))
}

/// One server and its client end.
struct Server {
    child: Child,
    stdin: ChildStdin,
    out: Receiver<Value>,
    /// Messages read while waiting for another one, in arrival order.
    held: Vec<Value>,
    _home: tempfile::TempDir,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Server {
    /// `al-call-hierarchy --lsp --no-telemetry [--no-watcher]` on `ws`,
    /// initialized; `hook` is the engine child's test hook.
    fn start(ws: &Path, hook: Option<&str>, watcher: bool) -> Server {
        let home = tempfile::tempdir().unwrap();
        let mut cmd = Command::new(LSP);
        cmd.args(["--lsp", "--no-telemetry"])
            .env("HOME", home.path())
            .env("USERPROFILE", home.path())
            .env("XDG_CONFIG_HOME", home.path())
            .env_remove("ALSEM_ENGINE_TEST_HOOK")
            .env_remove("RUST_LOG")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        if !watcher {
            cmd.arg("--no-watcher");
        }
        if let Some(h) = hook {
            cmd.env("ALSEM_ENGINE_TEST_HOOK", h);
        }
        let mut child = cmd.spawn().expect("al-call-hierarchy starts");
        let stdin = child.stdin.take().unwrap();
        let mut stdout = BufReader::new(child.stdout.take().unwrap());
        let (tx, out) = mpsc::channel();
        thread::spawn(move || {
            while let Some(msg) = read_frame(&mut stdout) {
                if tx.send(msg).is_err() {
                    break;
                }
            }
        });
        let mut s = Server {
            child,
            stdin,
            out,
            held: Vec::new(),
            _home: home,
        };
        let root = uri(ws);
        let init = s.request(
            json!(1),
            "initialize",
            json!({"processId": null, "rootUri": root, "capabilities": {},
                "workspaceFolders": [{"uri": root, "name": "ws"}]}),
        );
        assert!(init["result"]["capabilities"].is_object(), "{init}");
        s.notify("initialized", json!({}));
        s
    }

    fn write(&mut self, msg: &Value) {
        let body = msg.to_string();
        write!(self.stdin, "Content-Length: {}\r\n\r\n{body}", body.len()).unwrap();
        self.stdin.flush().unwrap();
    }

    fn notify(&mut self, method: &str, params: Value) {
        self.write(&json!({"jsonrpc": "2.0", "method": method, "params": params}));
    }

    fn send(&mut self, id: Value, method: &str, params: Value) {
        self.write(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
    }

    /// The next message matching `want`, holding the others.
    fn wait(&mut self, what: &str, want: impl Fn(&Value) -> bool) -> Value {
        if let Some(i) = self.held.iter().position(&want) {
            return self.held.remove(i);
        }
        let deadline = Instant::now() + TIMEOUT;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let msg = self
                .out
                .recv_timeout(left)
                .unwrap_or_else(|e| panic!("waiting for {what}: {e}"));
            if want(&msg) {
                return msg;
            }
            self.held.push(msg);
        }
    }

    fn response(&mut self, id: &Value) -> Value {
        let id = id.clone();
        self.wait(&format!("the response to {id}"), move |m| {
            m["id"] == id && m.get("method").is_none()
        })
    }

    fn request(&mut self, id: Value, method: &str, params: Value) -> Value {
        self.send(id.clone(), method, params);
        self.response(&id)
    }

    /// An engine request's envelope (its `result`).
    fn engine(&mut self, id: u64, method: &str, params: Value) -> Value {
        let r = self.request(json!(id), &format!("al-call-hierarchy/{method}"), params);
        assert!(r.get("error").is_none(), "{method}: {r}");
        r["result"].clone()
    }

    fn status(&mut self, id: u64) -> Value {
        self.engine(id, "engineStatus", json!({}))
    }

    /// `shutdown`, `exit`, and the process's exit.
    fn finish(mut self) -> std::process::ExitStatus {
        let r = self.request(json!("bye"), "shutdown", Value::Null);
        assert!(r.get("error").is_none(), "{r}");
        self.notify("exit", Value::Null);
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if let Some(s) = self.child.try_wait().unwrap() {
                return s;
            }
            assert!(Instant::now() < deadline, "the server did not exit");
            thread::sleep(Duration::from_millis(20));
        }
    }
}

fn uri(p: &Path) -> String {
    al_sem::protocol::path_to_uri(p).as_str().to_string()
}

/// An envelope's (stdout, stderr, exit), its files read and checked.
fn cli_bytes(env: &Value) -> (Vec<u8>, Vec<u8>, u8) {
    assert_eq!(env["kind"], "cli", "{env}");
    let file = |f: &Value| {
        let b = std::fs::read(f["path"].as_str().unwrap()).expect("the file exists");
        assert_eq!(b.len() as u64, f["bytes"].as_u64().unwrap());
        assert_eq!(
            al_sem::engine::ids::sha256_bytes_hex(&b),
            f["sha256"].as_str().unwrap()
        );
        b
    };
    let stdout = match (env.get("stdoutText"), env.get("stdoutFile")) {
        (Some(Value::String(t)), None) => t.clone().into_bytes(),
        (None, Some(f)) => file(f),
        _ => panic!("stdoutText XOR stdoutFile: {env}"),
    };
    let stderr = match (env.get("stderr"), env.get("stderrFile")) {
        (Some(Value::Array(lines)), None) => lines
            .iter()
            .flat_map(|l| format!("{}\n", l.as_str().unwrap()).into_bytes())
            .collect(),
        (None, Some(f)) => file(f),
        _ => panic!("stderr XOR stderrFile: {env}"),
    };
    let exit = u8::try_from(env["exitCode"].as_u64().unwrap()).unwrap();
    (stdout, stderr, exit)
}

/// The pids of `pid`'s child processes (Linux), else `None`.
fn children_of(pid: u32) -> Option<Vec<u32>> {
    let tasks = std::fs::read_dir(format!("/proc/{pid}/task")).ok()?;
    let mut pids = Vec::new();
    for t in tasks.flatten() {
        let text = std::fs::read_to_string(t.path().join("children")).unwrap_or_default();
        pids.extend(
            text.split_whitespace()
                .filter_map(|p| p.parse::<u32>().ok()),
        );
    }
    Some(pids)
}

/// The transcript: every engine request answers what the CLI answers, byte
/// for byte (stdout, stderr, exit), from one resident child; digest as a
/// file in the server's output folder; a request routed by a document
/// `uri`. DISCRIMINATION (2026-11-02): wrapping the envelope as an MCP
/// `tools/call` result (`{content: [envelope], isError}`) in
/// `LspTransport::response` fails `cli_bytes` (`kind` is null) at the
/// first explain; reverted, it passes.
#[test]
fn engine_requests_answer_what_the_cli_answers() {
    let (_tmp, ws) = workspace();
    let w = ws.to_str().unwrap();
    let mut s = Server::start(&ws, None, false);
    let cases: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "explain",
            json!({"routines": ["Archive"]}),
            vec!["explain", w, "--routine", "Archive"],
        ),
        (
            "explain",
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
        ),
        (
            "prove",
            json!({"routine": "PostOrder", "question": "may-commit", "deterministic": true}),
            vec!["prove", w, "PostOrder", "may-commit", "--deterministic"],
        ),
        (
            "queryEffects",
            json!({"routine": "PostOrder", "deterministic": true,
                "uri": uri(&ws.join("src/post.al"))}),
            vec![
                "query",
                "effects",
                w,
                "--routine",
                "PostOrder",
                "--deterministic",
            ],
        ),
        (
            "queryTouches",
            json!({"table": "50400", "deterministic": true}),
            vec!["query", "touches", w, "--table", "50400", "--deterministic"],
        ),
        (
            "digest",
            json!({"routines": ["PostOrder"], "deterministic": true}),
            vec!["digest", w, "--routine", "PostOrder", "--deterministic"],
        ),
    ];
    for (n, (method, params, argv)) in cases.iter().enumerate() {
        let expected = cli(argv);
        assert!(expected.0.ends_with(b"\n"), "{argv:?}");
        let env = s.engine(10 + n as u64, method, params.clone());
        assert_eq!(cli_bytes(&env), expected, "{method} {params}");
        assert_eq!(env["universe"], "cross-app", "{env}");
        assert_eq!(env["stale"], false, "{env}");
        assert_eq!(env["revision"]["generation"], 1, "one revision: {env}");
        if *method == "digest" {
            assert!(env["stdoutFile"].is_object(), "a digest is a file: {env}");
            assert_eq!(env["summary"]["entriesTotal"], 1, "{env}");
        }
    }
    let st = s.status(30);
    let engines = st["engines"].as_array().unwrap();
    assert_eq!(engines.len(), 1, "{st}");
    assert_eq!(engines[0]["spawns"], 1, "one child answered all: {st}");
    assert_eq!(engines[0]["deaths"], 0, "{st}");
    let folder = st["server"]["outputFolder"].as_str().unwrap();
    assert!(
        Path::new(folder).starts_with(s._home.path()),
        "the output folder is under ~/.al-sem: {st}"
    );
    // Invalid params are the MCP tool's own refusals, as -32602.
    let r = s.request(json!(31), "al-call-hierarchy/explain", json!({}));
    assert_eq!(r["error"]["code"], -32602, "{r}");
    assert!(s.finish().success());
}

/// The engine child is started by the first engine command, never by the
/// server's start or by call hierarchy, and ends with the server.
/// DISCRIMINATION (2026-11-02): an eager start (`run_server` submitting an
/// `engineRefresh` before the main loop) fails the first "no engine yet"
/// assertion (the status lists the engine, queued); reverted, it passes.
#[test]
fn the_engine_starts_on_the_first_engine_request_only() {
    let (_tmp, ws) = workspace();
    let mut s = Server::start(&ws, None, false);
    let pid = s.child.id();
    // Call hierarchy builds the root's snapshot: still no engine.
    let r = s.request(
        json!(2),
        "textDocument/prepareCallHierarchy",
        json!({"textDocument": {"uri": uri(&ws.join("src/post.al"))},
            "position": {"line": 4, "character": 16}}),
    );
    assert!(
        r["result"].as_array().is_some_and(|a| !a.is_empty()),
        "precondition: call hierarchy answers: {r}"
    );
    let st = s.status(3);
    assert_eq!(st["engines"], json!([]), "no engine yet: {st}");
    assert_eq!(st["server"]["outputFolder"], Value::Null, "{st}");
    if let Some(children) = children_of(pid) {
        assert!(children.is_empty(), "no child process yet: {children:?}");
    }
    let env = s.engine(4, "explain", json!({"routines": ["Archive"]}));
    assert_eq!(env["kind"], "cli", "{env}");
    let st = s.status(5);
    let engine_pid = st["engines"][0]["engine"]["pid"]
        .as_u64()
        .unwrap_or_else(|| panic!("a running engine: {st}")) as u32;
    if let Some(children) = children_of(pid) {
        assert!(
            children.contains(&engine_pid),
            "the engine is the server's child: {children:?} {st}"
        );
    }
    assert!(s.finish().success());
    // Ended with the server.
    let deadline = Instant::now() + Duration::from_secs(15);
    while Path::new(&format!("/proc/{engine_pid}")).exists()
        && std::fs::read_to_string(format!("/proc/{engine_pid}/stat"))
            .is_ok_and(|t| !t.contains(") Z "))
    {
        assert!(Instant::now() < deadline, "the engine outlived the server");
        thread::sleep(Duration::from_millis(50));
    }
}

/// Call hierarchy is answered while a long engine request runs: the engine
/// request is on a worker thread, not the main loop. The engine sleeps
/// 4 s before answering (test hook); the prepare sent after it is answered
/// first. DISCRIMINATION (2026-11-02): waiting in `LspEngines::handle`
/// until the submitted request leaves the pool (`while pool.in_flight(&id)`,
/// a blocking main loop) fails the order assertion.
#[cfg(any(feature = "engine-test-hooks", debug_assertions))]
#[test]
fn call_hierarchy_answers_while_an_engine_request_runs() {
    let (_tmp, ws) = workspace();
    let mut s = Server::start(&ws, Some("sleep-on-ask:4000"), false);
    let sent = Instant::now();
    s.send(
        json!(10),
        "al-call-hierarchy/explain",
        json!({"routines": ["Archive"]}),
    );
    s.send(
        json!(11),
        "textDocument/prepareCallHierarchy",
        json!({"textDocument": {"uri": uri(&ws.join("src/post.al"))},
            "position": {"line": 4, "character": 16}}),
    );
    let first = s.wait("a response", |m| {
        m.get("id").is_some() && m.get("method").is_none()
    });
    assert_eq!(first["id"], 11, "call hierarchy first: {first}");
    assert!(
        first["result"].as_array().is_some_and(|a| !a.is_empty()),
        "{first}"
    );
    assert!(
        sent.elapsed() < Duration::from_secs(4),
        "answered before the engine's sleep ended: {:?}",
        sent.elapsed()
    );
    let second = s.response(&json!(10));
    assert_eq!(second["result"]["kind"], "cli", "{second}");
    assert!(s.finish().success());
}

/// `$/cancelRequest` reaches the engine: the running request is sent
/// `session/cancel`, the engine answers `cancelled`, and the request is
/// answered `RequestCancelled` with that answer as its data. The engine
/// sleeps 3 s (test hook); the cancel is sent once the request is running
/// (its `$/progress` begin). DISCRIMINATION (2026-11-02): not forwarding
/// `$/cancelRequest` (`forward_to_engines`' arm doing nothing) fails the
/// error assertion (the request is answered `kind: cli`).
#[cfg(any(feature = "engine-test-hooks", debug_assertions))]
#[test]
fn cancel_request_cancels_the_engine_request() {
    let (_tmp, ws) = workspace();
    let mut s = Server::start(&ws, Some("sleep-on-ask:3000"), false);
    s.send(
        json!(20),
        "al-call-hierarchy/explain",
        json!({"routines": ["Archive"], "workDoneToken": "t20"}),
    );
    let begin = s.wait("the progress begin", |m| {
        m["method"] == "$/progress" && m["params"]["token"] == "t20"
    });
    assert_eq!(begin["params"]["value"]["kind"], "begin", "{begin}");
    s.notify("$/cancelRequest", json!({"id": 20}));
    let r = s.response(&json!(20));
    assert_eq!(r["error"]["code"], -32800, "{r}");
    assert_eq!(r["error"]["data"]["kind"], "failed", "{r}");
    assert_eq!(r["error"]["data"]["outcome"], "cancelled", "{r}");
    // The engine goes on serving.
    let st = s.status(21);
    assert_eq!(st["engines"][0]["deaths"], 0, "{st}");
    assert!(s.finish().success());
}

/// Rewrite `src/post.al` so `Archive` no longer commits, keeping the file's
/// length and modification time: a freshness check by stat alone sees no
/// change.
fn edit_keeping_stat(ws: &Path) {
    let path = ws.join("src/post.al");
    let before = std::fs::metadata(&path).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert_eq!(text.matches("        Commit();").count(), 1);
    let edited = text.replace("        Commit();", "        //mmit();");
    assert_eq!(edited.len(), text.len());
    std::fs::write(&path, edited).unwrap();
    let f = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
    f.set_modified(before.modified().unwrap()).unwrap();
    drop(f);
    let after = std::fs::metadata(&path).unwrap();
    assert_eq!(
        (after.len(), after.modified().unwrap()),
        (before.len(), before.modified().unwrap()),
        "precondition: the stat is unchanged"
    );
}

/// `didSave` reaches the engine as `session/notifyChanged`: after an edit
/// that keeps the file's size and time (which a stat check cannot see), a
/// save makes the next answer reflect the edit. DISCRIMINATION (2026-11-02):
/// dropping the forward (`forward_to_engines`' `didSave` arm doing nothing)
/// fails the second equality: the engine answers from the old revision.
#[test]
fn did_save_makes_the_next_answer_fresh() {
    let (_tmp, ws) = workspace();
    let w = ws.to_str().unwrap();
    let mut s = Server::start(&ws, None, false);
    let argv = ["explain", w, "--routine", "Archive"];
    let before = cli(&argv);
    let env = s.engine(10, "explain", json!({"routines": ["Archive"]}));
    assert_eq!(cli_bytes(&env), before);
    edit_keeping_stat(&ws);
    let after = cli(&argv);
    assert_ne!(after.0, before.0, "precondition: the edit changes the card");
    s.notify(
        "textDocument/didSave",
        json!({"textDocument": {"uri": uri(&ws.join("src/post.al"))}}),
    );
    let env = s.engine(11, "explain", json!({"routines": ["Archive"]}));
    assert_eq!(
        cli_bytes(&env),
        after,
        "the answer reflects the edit: {env}"
    );
    assert_eq!(env["revision"]["generation"], 2, "a new revision: {env}");
    assert!(s.finish().success());
}

/// The file watcher's events reach the engine too: with the watcher on (the
/// root built by a call hierarchy request), the same stat-preserving edit
/// with no `didSave` is seen by a later answer. DISCRIMINATION (2026-11-02):
/// starting the watcher on the updater's sender directly (no
/// `tee_to_engines`) fails the poll: every answer stays the old card.
#[test]
fn watcher_events_make_answers_fresh() {
    let (_tmp, ws) = workspace();
    let w = ws.to_str().unwrap();
    let mut s = Server::start(&ws, None, true);
    let r = s.request(
        json!(2),
        "textDocument/prepareCallHierarchy",
        json!({"textDocument": {"uri": uri(&ws.join("src/post.al"))},
            "position": {"line": 4, "character": 16}}),
    );
    assert!(r["result"].is_array(), "the root is built: {r}");
    let argv = ["explain", w, "--routine", "Archive"];
    let before = cli(&argv);
    assert_eq!(
        cli_bytes(&s.engine(10, "explain", json!({"routines": ["Archive"]}))),
        before
    );
    edit_keeping_stat(&ws);
    let after = cli(&argv);
    assert_ne!(after.0, before.0, "precondition: the edit changes the card");
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut id = 11;
    loop {
        let env = s.engine(id, "explain", json!({"routines": ["Archive"]}));
        let got = cli_bytes(&env);
        if got == after {
            break;
        }
        assert_eq!(got, before, "either card, nothing else: {env}");
        assert!(
            Instant::now() < deadline,
            "the watcher's event never reached the engine"
        );
        id += 1;
        thread::sleep(Duration::from_millis(250));
    }
    assert!(s.finish().success());
}

//! #222 T5e (spec §5.6, O24, O26): the real LSP binary with `--l4-host` sends
//! the editor exactly what it sends without the flag.
//!
//! Each run spawns `al-call-hierarchy --lsp [--l4-host] --no-watcher
//! --no-telemetry --verbose` on fixture T1a over stdio, sends `initialize`,
//! `initialized` and one `prepareCallHierarchy` (the root builds lazily inside
//! it, and its initial `publishDiagnostics` go out before the response: the
//! sync point, O26), waits for that response (the flag-on run also for the
//! stderr line `l4-host: published generation 1`), then `shutdown` and
//! `exit`. Stdout and stderr are read on two threads; every wait has a
//! timeout; the child is killed when its guard drops.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::l4_host_oracle::{T1A, write_ws};

const TIMEOUT: Duration = Duration::from_secs(60);

/// Kills the child if the test ends early.
struct Guard(Child);

impl Drop for Guard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// One `Content-Length` frame from `out`, or `None` at EOF. Panics on a
/// malformed frame: every frame must be well-formed JSON-RPC.
fn read_frame(out: &mut impl BufRead) -> Option<Value> {
    let mut len = None;
    loop {
        let mut line = String::new();
        if out.read_line(&mut line).unwrap() == 0 {
            assert!(len.is_none(), "EOF inside a frame header");
            return None;
        }
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            break;
        }
        let (name, value) = line.split_once(": ").expect("a header line");
        if name.eq_ignore_ascii_case("Content-Length") {
            len = Some(value.parse::<usize>().expect("a numeric Content-Length"));
        }
    }
    let mut body = vec![0; len.expect("a Content-Length header")];
    out.read_exact(&mut body).expect("the whole body");
    let msg: Value = serde_json::from_slice(&body).expect("a JSON body");
    assert_eq!(msg["jsonrpc"], "2.0", "a JSON-RPC 2.0 message: {msg}");
    Some(msg)
}

fn send(stdin: &mut ChildStdin, msg: Value) {
    let body = msg.to_string();
    write!(stdin, "Content-Length: {}\r\n\r\n{body}", body.len()).unwrap();
    stdin.flush().unwrap();
}

/// `value` re-serialized with sorted object keys.
fn canonical(value: &Value) -> String {
    fn sort(v: &Value) -> Value {
        match v {
            Value::Object(m) => {
                let sorted: BTreeMap<_, _> = m.iter().map(|(k, v)| (k.clone(), sort(v))).collect();
                Value::Object(sorted.into_iter().collect())
            }
            Value::Array(a) => Value::Array(a.iter().map(sort).collect()),
            other => other.clone(),
        }
    }
    sort(value).to_string()
}

/// Wait for the next stdout message, failing on timeout.
fn next(rx: &Receiver<Value>, deadline: Instant, what: &str) -> Value {
    let left = deadline.saturating_duration_since(Instant::now());
    rx.recv_timeout(left)
        .unwrap_or_else(|e| panic!("waiting for {what}: {e}"))
}

/// One session. Returns every server-to-client message (normalized, sorted)
/// and the number of `publishDiagnostics` before the prepare response.
fn session(dir: &Path, l4_host: bool) -> (Vec<String>, usize) {
    let home = tempfile::tempdir().unwrap();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_al-call-hierarchy"));
    cmd.args(["--lsp", "--no-watcher", "--no-telemetry", "--verbose"]);
    if l4_host {
        cmd.arg("--l4-host");
    }
    let mut guard = Guard(
        cmd.env("HOME", home.path())
            .env("USERPROFILE", home.path())
            .env("XDG_CONFIG_HOME", home.path())
            .env_remove("RUST_LOG")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn al-call-hierarchy"),
    );
    let mut stdin = guard.0.stdin.take().unwrap();
    let mut stdout = BufReader::new(guard.0.stdout.take().unwrap());
    let stderr = BufReader::new(guard.0.stderr.take().unwrap());
    let (out_tx, out) = mpsc::channel();
    let out_thread = thread::spawn(move || {
        while let Some(msg) = read_frame(&mut stdout) {
            if out_tx.send(msg).is_err() {
                break;
            }
        }
    });
    let (err_tx, err) = mpsc::channel();
    thread::spawn(move || {
        for line in stderr.lines().map_while(Result::ok) {
            if err_tx.send(line).is_err() {
                break;
            }
        }
    });

    let deadline = Instant::now() + TIMEOUT;
    let mut received = Vec::new();
    let root = format!("file://{}", dir.display());
    send(
        &mut stdin,
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "processId": null, "rootUri": root, "capabilities": {},
            "workspaceFolders": [{"uri": root, "name": "t1a"}]}}),
    );
    let init = next(&out, deadline, "the initialize response");
    assert_eq!(init["id"], 1, "{init}");
    received.push(init);
    send(
        &mut stdin,
        json!({"jsonrpc": "2.0", "method": "initialized", "params": {}}),
    );
    send(
        &mut stdin,
        json!({"jsonrpc": "2.0", "id": 2, "method": "textDocument/prepareCallHierarchy",
            "params": {"textDocument": {"uri": format!("{root}/src/A.al")},
                "position": {"line": 2, "character": 14}}}),
    );
    let mut diagnostics_before = 0;
    loop {
        let msg = next(&out, deadline, "the prepareCallHierarchy response");
        let is_response = msg["id"] == 2;
        if is_response {
            assert!(
                msg["result"].as_array().is_some_and(|a| !a.is_empty()),
                "precondition: the root built and prepare found A: {msg}"
            );
        }
        if msg["method"] == "textDocument/publishDiagnostics" {
            diagnostics_before += 1;
        }
        received.push(msg);
        if is_response {
            break;
        }
    }
    if l4_host {
        let want = "l4-host: published generation 1";
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let line = err
                .recv_timeout(left)
                .unwrap_or_else(|e| panic!("waiting for {want:?} on stderr: {e}"));
            if line.contains(want) {
                break;
            }
        }
    }
    send(
        &mut stdin,
        json!({"jsonrpc": "2.0", "id": 3, "method": "shutdown", "params": null}),
    );
    loop {
        let msg = next(&out, deadline, "the shutdown response");
        let done = msg["id"] == 3;
        received.push(msg);
        if done {
            break;
        }
    }
    send(&mut stdin, json!({"jsonrpc": "2.0", "method": "exit"}));
    drop(stdin);
    // Everything up to stdout EOF.
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match out.recv_timeout(left) {
            Ok(msg) => received.push(msg),
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(e) => panic!("waiting for stdout EOF: {e}"),
        }
    }
    out_thread.join().expect("well-formed frames only");
    while guard.0.try_wait().unwrap().is_none() {
        assert!(Instant::now() < deadline, "the server exits");
        thread::yield_now();
    }
    let mut all: Vec<String> = received.iter().map(canonical).collect();
    all.sort();
    (all, diagnostics_before)
}

/// T5e. Breaks: (i) `main.rs` drops the `l4_host` pass-through → the stderr
/// wait times out; (ii) an extra `publishDiagnostics` under the flag → the
/// multisets differ.
#[test]
fn t5e_flag_on_sends_nothing_new() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().canonicalize().unwrap();
    write_ws(&dir, T1A);
    let (on, on_before) = session(&dir, true);
    let (off, off_before) = session(&dir, false);
    assert_eq!(
        on, off,
        "the same server-to-client messages, with and without --l4-host"
    );
    assert_eq!(
        on_before, off_before,
        "the same publishDiagnostics count before the prepare response"
    );
}

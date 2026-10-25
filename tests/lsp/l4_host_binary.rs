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
//! timeout; the child is killed when its guard drops. The flag-on run is
//! traced, for the host's counters (final r1 FX-5). `host_panic_ends_the_server`
//! (final r1 FX-13) spawns the same server with the debug-only panic seam.

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

/// A spawned server: the child (killed on drop), its stdin, its stdout
/// frames, its stderr lines and the stdout reader thread.
struct Server {
    guard: Guard,
    stdin: ChildStdin,
    out: Receiver<Value>,
    err: Receiver<String>,
    out_thread: thread::JoinHandle<()>,
    _home: tempfile::TempDir,
}

/// Spawn `al-call-hierarchy <args> --no-watcher --no-telemetry --verbose`
/// with `env` set and a private home.
fn spawn(args: &[&str], env: &[(&str, &std::ffi::OsStr)]) -> Server {
    let home = tempfile::tempdir().unwrap();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_al-call-hierarchy"));
    cmd.args(args)
        .args(["--no-watcher", "--no-telemetry", "--verbose"])
        .env("HOME", home.path())
        .env("USERPROFILE", home.path())
        .env("XDG_CONFIG_HOME", home.path())
        .env_remove("RUST_LOG")
        .env_remove("AL_SEM_L4_HOST_TEST_PANIC");
    for (k, v) in env {
        cmd.env(k, v);
    }
    let mut guard = Guard(
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn al-call-hierarchy"),
    );
    let stdin = guard.0.stdin.take().unwrap();
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
    Server {
        guard,
        stdin,
        out,
        err,
        out_thread,
        _home: home,
    }
}

/// `dir` as a file URI (final r1 FX-3: the repo's own encoder, so a Windows
/// path or a space gives a valid URI).
fn uri(dir: &Path) -> String {
    al_sem::protocol::path_to_uri(dir).as_str().to_string()
}

/// A tempdir path the server sees as written: canonical (macOS `/tmp` is a
/// link), but never Windows' `\\?\` verbatim form, which no file URI has.
fn ws_dir(tmp: &tempfile::TempDir) -> std::path::PathBuf {
    if cfg!(windows) {
        tmp.path().to_path_buf()
    } else {
        tmp.path().canonicalize().unwrap()
    }
}

/// Send `initialize` (without waiting), `initialized` and one
/// `prepareCallHierarchy` on `A`, the request that builds the root.
fn open(
    stdin: &mut ChildStdin,
    root: &str,
    out: Option<(&Receiver<Value>, Instant)>,
) -> Option<Value> {
    send(
        stdin,
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "processId": null, "rootUri": root, "capabilities": {},
            "workspaceFolders": [{"uri": root, "name": "t1a"}]}}),
    );
    let init = out.map(|(out, deadline)| next(out, deadline, "the initialize response"));
    send(
        stdin,
        json!({"jsonrpc": "2.0", "method": "initialized", "params": {}}),
    );
    send(
        stdin,
        json!({"jsonrpc": "2.0", "id": 2, "method": "textDocument/prepareCallHierarchy",
            "params": {"textDocument": {"uri": format!("{root}/src/A.al")},
                "position": {"line": 2, "character": 14}}}),
    );
    init
}

/// One session. Returns every server-to-client message (normalized, sorted),
/// the number of `publishDiagnostics` before the prepare response, and the
/// stderr `published generation 1` line (flag on only).
fn session(
    dir: &Path,
    l4_host: bool,
    env: &[(&str, &std::ffi::OsStr)],
) -> (Vec<String>, usize, Option<String>) {
    let mut args = vec!["--lsp"];
    if l4_host {
        args.push("--l4-host");
    }
    let Server {
        guard,
        mut stdin,
        out,
        err,
        out_thread,
        _home,
    } = spawn(&args, env);
    let mut guard = guard;

    let deadline = Instant::now() + TIMEOUT;
    let mut received = Vec::new();
    let root = uri(dir);
    let init = open(&mut stdin, &root, Some((&out, deadline))).unwrap();
    assert_eq!(init["id"], 1, "{init}");
    received.push(init);
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
    let published = l4_host.then(|| {
        let want = "l4-host: published generation 1";
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let line = err
                .recv_timeout(left)
                .unwrap_or_else(|e| panic!("waiting for {want:?} on stderr: {e}"));
            if line.contains(want) {
                break line;
            }
        }
    });
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
    (all, diagnostics_before, published)
}

/// The last value of each counter (`ph: "C"`) in the Chrome trace at `path`.
fn trace_counters(path: &Path) -> BTreeMap<String, u64> {
    let text = std::fs::read_to_string(path).expect("the trace file");
    let events: Vec<Value> = serde_json::from_str(&text).expect("a JSON trace");
    let mut last = BTreeMap::new();
    for e in events.iter().filter(|e| e["ph"] == "C") {
        for (k, v) in e["args"].as_object().into_iter().flatten() {
            if let Some(v) = v.as_u64() {
                last.insert(k.clone(), v);
            }
        }
    }
    last
}

/// T5e. Breaks: (i) `main.rs` drops the `l4_host` pass-through → the stderr
/// wait times out; (ii) an extra `publishDiagnostics` under the flag → the
/// multisets differ. `main.rs` has one `run_server` call, so the default
/// branch (no `--lsp`) is the same call (final r1 FX-7).
///
/// The flag-on run is traced (final r1 FX-5): its one refresh counts
/// `host.refresh` 1, `host.full` 1, `host.sccs_resolved` the published
/// total and `host.sccs_seeded` 0. Break: drop any one emission → that
/// counter is missing.
#[test]
fn t5e_flag_on_sends_nothing_new() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = ws_dir(&tmp);
    write_ws(&dir, T1A);
    let trace_dir = tempfile::tempdir().unwrap();
    let trace = trace_dir.path().join("trace.json");
    let env: [(&str, &std::ffi::OsStr); 2] = [
        ("ALSEM_TRACE", "1".as_ref()),
        ("ALSEM_TRACE_FILE", trace.as_os_str()),
    ];
    let (on, on_before, published) = session(&dir, true, &env);
    let (off, off_before, _) = session(&dir, false, &[]);
    assert_eq!(
        on, off,
        "the same server-to-client messages, with and without --l4-host"
    );
    assert_eq!(
        on_before, off_before,
        "the same publishDiagnostics count before the prepare response"
    );
    let published = published.expect("flag on");
    let total: u64 = published
        .rsplit_once(" of ")
        .and_then(|(_, n)| n.trim().parse().ok())
        .unwrap_or_else(|| panic!("a total in {published:?}"));
    assert!(total > 0, "precondition: SCCs to solve");
    let c = trace_counters(&trace);
    let get = |k: &str| c.get(k).copied();
    assert_eq!(
        (
            get("host.refresh"),
            get("host.full"),
            get("host.sccs_resolved"),
            get("host.sccs_seeded")
        ),
        (Some(1), Some(1), Some(total), Some(0)),
        "host counters: {c:?}"
    );
}

/// Final r1 FX-13: a panic in the host's worker ends the server in every
/// profile, this unwinding test build included: the worker catches it,
/// logs it and aborts. The debug-only seam `AL_SEM_L4_HOST_TEST_PANIC`
/// panics inside the worker. Break: the worker only logs (no abort) → the
/// server keeps running and the exit wait times out.
// The panic seam is compiled only with debug assertions.
#[cfg(debug_assertions)]
#[test]
fn host_panic_ends_the_server() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = ws_dir(&tmp);
    write_ws(&dir, T1A);
    let Server {
        mut guard,
        mut stdin,
        out: _out,
        err,
        out_thread: _,
        _home,
    } = spawn(
        &["--lsp", "--l4-host"],
        &[("AL_SEM_L4_HOST_TEST_PANIC", "1".as_ref())],
    );
    open(&mut stdin, &uri(&dir), None);
    let deadline = Instant::now() + TIMEOUT;
    let status = loop {
        if let Some(status) = guard.0.try_wait().unwrap() {
            break status;
        }
        assert!(Instant::now() < deadline, "the server ends after the panic");
        thread::yield_now();
    };
    assert!(!status.success(), "a failure status: {status:?}");
    // The process is gone, so its stderr ends.
    let stderr: Vec<String> = err.iter().collect();
    assert!(
        stderr.iter().any(|l| l.contains("l4-host: panicked")),
        "the panic is logged: {stderr:?}"
    );
}

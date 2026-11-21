//! ENG-110 (spec "`alsem batch`" and "Correctness checks"): the dependency IR cache through
//! real `alsem` processes. A plain `alsem analyze` with the cache on publishes an entry and
//! writes a `start` and an `end` report line (the positive control); `alsem batch` with two
//! workers touches nothing; and cargo's `[env]` default keeps a spawned engine off.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::{Value, json};

use crate::cli_batch_writes::{job, run_manifest_doc};
use crate::symbol_app;

/// Dependency source files: above `SMALL_PACKAGE_MIN_FILES` (8), so the package is cached.
const DEP_FILES: usize = 12;

/// A workspace calling into one embedded-source dependency, and an isolated
/// `XDG_CACHE_HOME` (ENG-113's text cache lives under it).
struct Fixture {
    _tmp: tempfile::TempDir,
    ws: PathBuf,
    xdg: PathBuf,
    root: PathBuf,
}

fn fixture() -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path().join("ws");
    std::fs::create_dir_all(ws.join("src")).unwrap();
    std::fs::write(
        ws.join("app.json"),
        json!({
            "id": "11111111-0000-0000-0000-000000000110",
            "name": "Ws110",
            "publisher": "probe",
            "version": "1.0.0.0",
            "runtime": "13.0",
            "idRanges": [{"from": 50100, "to": 50199}],
            "dependencies": [{"id": "22222222-0000-0000-0000-000000000110", "name": "Dep110",
                "publisher": "probe", "version": "1.0.0.0"}]
        })
        .to_string(),
    )
    .unwrap();
    std::fs::write(
        ws.join("src/Ws.al"),
        "codeunit 50100 \"Ws Cu\"\n{\n    procedure Run()\n    var\n        D: Codeunit \"Dep Cu 0\";\n    begin\n        D.P0();\n    end;\n}\n",
    )
    .unwrap();
    let sources: Vec<(String, String)> = (0..DEP_FILES)
        .map(|i| {
            (
                format!("src/Dep{i}.Codeunit.al"),
                format!(
                    "codeunit {} \"Dep Cu {i}\"\n{{\n    procedure P{i}()\n    begin\n    end;\n}}\n",
                    60000 + i
                ),
            )
        })
        .collect();
    let refs: Vec<(&str, &str)> = sources
        .iter()
        .map(|(p, t)| (p.as_str(), t.as_str()))
        .collect();
    symbol_app::write_source_app(
        &ws.join(".alpackages/probe_Dep110_1.0.0.0.app"),
        "22222222-0000-0000-0000-000000000110",
        "Dep110",
        "1.0.0.0",
        "{}",
        &refs,
        "",
    );
    let xdg = tmp.path().join("xdg");
    let root = tmp.path().join("ir-root");
    Fixture {
        _tmp: tmp,
        ws,
        xdg,
        root,
    }
}

/// `alsem analyze <ws>` with `env` added.
fn analyze(f: &Fixture, env: &[(&str, &Path)]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_alsem"));
    cmd.args(["analyze", f.ws.to_str().unwrap(), "--format", "json"])
        .env("RUST_LOG", "off")
        .env("XDG_CACHE_HOME", &f.xdg);
    for (k, v) in env {
        cmd.env(k, v);
    }
    let out = cmd.output().unwrap();
    assert!(
        out.status.code().is_some(),
        "analyze ended by a signal: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    out
}

fn report(path: &Path) -> Vec<Value> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(|l| serde_json::from_str(l).expect("a JSON report line"))
        .collect()
}

/// Every `.bin` entry under `root`.
fn entries(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for kd in std::fs::read_dir(root).into_iter().flatten().flatten() {
        for e in std::fs::read_dir(kd.path()).into_iter().flatten().flatten() {
            if e.path().extension().is_some_and(|x| x == "bin") {
                out.push(e.path());
            }
        }
    }
    out
}

fn end_counter(lines: &[Value], name: &str) -> u64 {
    let ends: Vec<_> = lines.iter().filter(|l| l["event"] == "end").collect();
    assert_eq!(ends.len(), 1, "one end line: {lines:?}");
    ends[0]["counters"][name].as_u64().unwrap()
}

/// The positive control: the cache on, a fresh root: one entry published, a `start` and an
/// `end` line; the next run hits it.
#[test]
fn analyze_publishes_and_reports() {
    let f = fixture();
    let rep = f.root.with_extension("jsonl");
    let on = [
        ("ALSEM_IR_CACHE", Path::new("on")),
        ("ALSEM_IR_CACHE_DIR", f.root.as_path()),
        ("ALSEM_IR_CACHE_REPORT", rep.as_path()),
    ];
    let cold = analyze(&f, &on);
    let lines = report(&rep);
    assert!(
        lines.iter().any(|l| l["event"] == "start"),
        "a start line: {lines:?}\n{}",
        String::from_utf8_lossy(&cold.stderr)
    );
    assert_eq!(end_counter(&lines, "published_packages"), 1, "{lines:?}");
    assert_eq!(entries(&f.root).len(), 1);
    std::fs::remove_file(&rep).unwrap();
    let warm = analyze(&f, &on);
    let lines = report(&rep);
    assert_eq!(
        end_counter(&lines, "disk_hits"),
        DEP_FILES as u64,
        "{lines:?}"
    );
    // The report's `generatedAt` differs on every run.
    let doc = |o: &Output| {
        let mut v: Value = serde_json::from_slice(&o.stdout).expect("a JSON report");
        v.as_object_mut().unwrap().remove("generatedAt");
        v
    };
    assert_eq!(doc(&warm), doc(&cold), "the same analysis warm and cold");
}

/// `alsem batch` running `analyze <ws>` in `batches` batches (1: in the parent; 2: two
/// workers) with the cache on, `f.root` and report `rep`; returns the workers started.
fn batch(f: &Fixture, rep: &Path, batches: usize) -> usize {
    let dir = tempfile::tempdir().unwrap();
    let ws = f.ws.to_str().unwrap();
    let analyze = ["analyze", ws, "--format", "json"];
    let doc = json!({ "batches": (0..batches)
        .map(|_| json!({ "jobs": [job(&analyze, None, None)] }))
        .collect::<Vec<_>>() });
    let (out, trace) = run_manifest_doc(
        dir.path(),
        &doc,
        None,
        &[
            ("ALSEM_IR_CACHE", "on"),
            ("ALSEM_IR_CACHE_DIR", f.root.to_str().unwrap()),
            ("ALSEM_IR_CACHE_REPORT", rep.to_str().unwrap()),
            ("XDG_CACHE_HOME", f.xdg.to_str().unwrap()),
        ],
        None,
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    trace
        .iter()
        .filter(|e| e["ph"] == "i" && e["name"] == "batch.worker.start")
        .count()
}

/// Every file under `root` with its length, modification time and bytes (a `last-used`
/// refresh changes them).
fn tree_state(root: &Path) -> Vec<(PathBuf, u64, std::time::SystemTime, Vec<u8>)> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let m = e.metadata().unwrap();
            if m.is_dir() {
                stack.push(e.path());
            } else {
                let bytes = std::fs::read(e.path()).unwrap_or_default();
                out.push((e.path(), m.len(), m.modified().unwrap(), bytes));
            }
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// TLS-116: a batch run (parent and two workers, or one batch in the parent) reads,
/// writes and reports nothing, with the same environment as the control: on a cold root
/// nothing is made; on a warm root nothing is read (no `last-used` refresh) or rewritten.
#[test]
fn batch_with_workers_touches_nothing() {
    let f = fixture();
    let dir = tempfile::tempdir().unwrap();
    let rep = dir.path().join("report.jsonl");
    assert_eq!(batch(&f, &rep, 2), 2, "precondition: two workers ran");
    assert!(!f.root.exists(), "cold root: no ir folder made");
    assert!(!rep.exists(), "no report line");
    // A warm root, filled by a plain analyze (the control's path).
    analyze(
        &f,
        &[
            ("ALSEM_IR_CACHE", Path::new("on")),
            ("ALSEM_IR_CACHE_DIR", f.root.as_path()),
        ],
    );
    assert_eq!(entries(&f.root).len(), 1, "precondition: a warm root");
    let before = tree_state(&f.root);
    // `last-used` is refreshed by any use on another day only; a use today would leave it
    // alone, so make it a day old: a batch that read the cache would rewrite it.
    let last_used = entries(&f.root)[0].with_file_name("last-used");
    let day_old = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        - 2 * 86_400;
    std::fs::write(&last_used, day_old.to_string()).unwrap();
    let before_last_used = std::fs::read(&last_used).unwrap();
    for (batches, workers) in [(2, 2), (1, 0)] {
        assert_eq!(
            batch(&f, &rep, batches),
            workers,
            "precondition: {batches} batches"
        );
        assert!(!rep.exists(), "{batches} batches: no report line");
        assert_eq!(
            std::fs::read(&last_used).unwrap(),
            before_last_used,
            "{batches} batches: the cache was not read"
        );
        let after: Vec<_> = tree_state(&f.root)
            .into_iter()
            .filter(|(p, ..)| p != &last_used)
            .collect();
        let want: Vec<_> = before
            .iter()
            .filter(|(p, ..)| p != &last_used)
            .cloned()
            .collect();
        assert_eq!(after, want, "{batches} batches: nothing published");
    }
}

/// The cache off through a real process: every seam file `skipped_off`, nothing read or
/// published (spec Acceptance 2's off-sample rule).
#[test]
fn cache_off_counts_every_seam_file_as_skipped_off() {
    let f = fixture();
    let rep = f.root.with_extension("jsonl");
    analyze(
        &f,
        &[
            ("ALSEM_IR_CACHE", Path::new("off")),
            ("ALSEM_IR_CACHE_DIR", f.root.as_path()),
            ("ALSEM_IR_CACHE_REPORT", rep.as_path()),
        ],
    );
    let lines = report(&rep);
    let seam = end_counter(&lines, "seam_files");
    assert!(seam >= DEP_FILES as u64, "{lines:?}");
    assert_eq!(end_counter(&lines, "skipped_off"), seam, "{lines:?}");
    assert_eq!(end_counter(&lines, "disk_hits"), 0);
    assert_eq!(end_counter(&lines, "published_packages"), 0);
    assert!(!f.root.exists(), "nothing made");
}

/// sol r1: a non-UTF-8 command line never panics the report, with or without a sink.
#[test]
fn a_non_utf8_command_line_runs() {
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::process::CommandExt;
    let f = fixture();
    let rep = f.root.with_extension("jsonl");
    for with_report in [false, true] {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_alsem"));
        cmd.arg0(std::ffi::OsStr::from_bytes(b"alsem-\xff"))
            .args(["analyze", f.ws.to_str().unwrap(), "--format", "json"])
            .env("RUST_LOG", "off")
            .env("XDG_CACHE_HOME", &f.xdg)
            .env("ALSEM_IR_CACHE", "off");
        if with_report {
            cmd.env("ALSEM_IR_CACHE_REPORT", &rep);
        }
        let out = cmd.output().unwrap();
        assert!(
            out.status.code().is_some_and(|c| c != 101),
            "report {with_report}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let lines = report(&rep);
    let start = lines
        .iter()
        .find(|l| l["event"] == "start")
        .unwrap_or_else(|| panic!("a start line: {lines:?}"));
    // sol/Fable r2: the whole command line, losslessly: the non-UTF-8 arg0 as its bytes, and
    // every argument after it (five here; a run's mode, `--low-mem` say, is provable).
    assert_eq!(
        start["argv"],
        json!([
            {"hex": "616c73656d2dff"},
            "analyze",
            f.ws.to_str().unwrap(),
            "--format",
            "json"
        ]),
        "{lines:?}"
    );
}

/// sol r1: an entry that is there but refused counts as `refused`, and the gate's report
/// rule fails the phase on it.
#[test]
fn a_refused_entry_fails_the_gate_rule() {
    let f = fixture();
    let rep = f.root.with_extension("jsonl");
    let on = [
        ("ALSEM_IR_CACHE", Path::new("on")),
        ("ALSEM_IR_CACHE_DIR", f.root.as_path()),
        ("ALSEM_IR_CACHE_REPORT", rep.as_path()),
    ];
    analyze(&f, &on);
    let check = || {
        Command::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/scripts/ir-cache-report-check"
        ))
        .args(["--rule", "output"])
        .arg(&rep)
        .output()
        .unwrap()
    };
    assert!(
        check().status.success(),
        "precondition: a clean fill passes"
    );
    let entry = &entries(&f.root)[0];
    let mut b = std::fs::read(entry).unwrap();
    let at = b.len() - 40;
    b[at] ^= 1;
    std::fs::write(entry, &b).unwrap();
    std::fs::remove_file(&rep).unwrap();
    analyze(&f, &on);
    let lines = report(&rep);
    assert_eq!(
        end_counter(&lines, "refused"),
        DEP_FILES as u64,
        "{lines:?}"
    );
    let out = check();
    assert_eq!(
        out.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(String::from_utf8_lossy(&out.stdout).contains("refused"));
}

/// Fable r1: the engine child (`alsem mcp` -> `engine-host`) writes its start and end
/// lines; the front end, which parses nothing, writes none.
#[test]
fn the_engine_child_reports_start_and_end() {
    use std::io::{BufRead, BufReader, Write};
    use std::process::Stdio;
    let f = fixture();
    let rep = f.root.with_extension("jsonl");
    let out_root = tempfile::tempdir().unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .args(["mcp", "--workspace"])
        .arg(&f.ws)
        .arg("--output-root")
        .arg(out_root.path())
        .env("RUST_LOG", "off")
        .env("XDG_CACHE_HOME", &f.xdg)
        .env("ALSEM_IR_CACHE", "on")
        .env("ALSEM_IR_CACHE_DIR", &f.root)
        .env("ALSEM_IR_CACHE_REPORT", &rep)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    let mut output = BufReader::new(child.stdout.take().unwrap());
    let mut send = |v: Value| {
        writeln!(input, "{v}").unwrap();
        input.flush().unwrap();
    };
    let mut response = || loop {
        let mut line = String::new();
        assert!(
            output.read_line(&mut line).unwrap() > 0,
            "the server closed"
        );
        let v: Value = serde_json::from_str(&line).unwrap();
        if v.get("id").is_some() {
            return v;
        }
    };
    send(
        json!({"jsonrpc": "2.0", "id": 0, "method": "initialize", "params": {
        "protocolVersion": "2025-06-18", "capabilities": {},
        "clientInfo": {"name": "test", "version": "1"}}}),
    );
    response();
    send(json!({"jsonrpc": "2.0", "method": "notifications/initialized", "params": {}}));
    send(
        json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {
        "name": "alsem_explain", "arguments": {"routines": ["Run"], "format": "json"}}}),
    );
    let r = response();
    assert!(r.get("error").is_none(), "{r}");
    drop(input);
    assert!(child.wait().unwrap().success());
    let lines = report(&rep);
    let starts: Vec<_> = lines.iter().filter(|l| l["event"] == "start").collect();
    assert_eq!(starts.len(), 1, "one process used the seam: {lines:?}");
    assert!(
        starts[0]["argv"]
            .as_array()
            .unwrap()
            .iter()
            .any(|a| a == "engine-host"),
        "the engine child: {lines:?}"
    );
    assert!(end_counter(&lines, "eligible_files") > 0, "{lines:?}");
    assert_eq!(lines[1]["pid"], starts[0]["pid"], "its own end line");
}

/// `perf_trace`'s `ALSEM_TRACE_EXIT_AFTER` exits from a span guard, past the `main`'s drop;
/// it writes the end line itself.
#[test]
fn an_exit_after_still_writes_the_end_line() {
    let f = fixture();
    let rep = f.root.with_extension("jsonl");
    let trace = f.root.with_extension("trace.json");
    let out = analyze(
        &f,
        &[
            ("ALSEM_IR_CACHE", Path::new("on")),
            ("ALSEM_IR_CACHE_DIR", f.root.as_path()),
            ("ALSEM_IR_CACHE_REPORT", rep.as_path()),
            ("ALSEM_TRACE", Path::new("1")),
            ("ALSEM_TRACE_FILE", trace.as_path()),
            ("ALSEM_TRACE_EXIT_AFTER", Path::new("crossapp.base_total")),
        ],
    );
    assert_eq!(out.status.code(), Some(0));
    assert!(
        out.stdout.is_empty(),
        "the exit came before the analysis printed"
    );
    let lines = report(&rep);
    assert!(end_counter(&lines, "seam_files") > 0, "{lines:?}");
}

/// `.cargo/config.toml`'s `[env]` keeps every engine a test spawns off: with no value of its
/// own, an analyze writes nothing.
#[test]
fn cargo_env_defaults_off() {
    assert_eq!(
        std::env::var("ALSEM_IR_CACHE").as_deref(),
        Ok("off"),
        "cargo's [env] sets it for tests"
    );
    let f = fixture();
    analyze(&f, &[("ALSEM_IR_CACHE_DIR", f.root.as_path())]);
    assert!(entries(&f.root).is_empty(), "nothing written");
}

/// Fable r1: `aldump --session-refresh --verify`'s cache-off reference builds run under
/// `ir_cache::Suspend`: with the cache on, `--verify` adds no seam use to the report (each
/// reference build parses its dependencies, outside the cache).
#[test]
fn session_refresh_reference_builds_bypass_the_cache() {
    let f = fixture();
    let rep = f.root.with_extension("jsonl");
    // This analyze fills ENG-113's text cache (shared) and alsem's IR key; aldump has its own
    // key, so its first run below fills that and the second hits it. The two runs' seam counts
    // are compared, which a fill or a hit does not change.
    analyze(
        &f,
        &[
            ("ALSEM_IR_CACHE", Path::new("on")),
            ("ALSEM_IR_CACHE_DIR", f.root.as_path()),
        ],
    );
    let seam = |verify: bool| {
        let _ = std::fs::remove_file(&rep);
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_aldump"));
        cmd.arg("--session-refresh").arg(&f.ws);
        if verify {
            cmd.arg("--verify");
        }
        let out = cmd
            .env("RUST_LOG", "off")
            .env("XDG_CACHE_HOME", &f.xdg)
            .env("ALSEM_IR_CACHE", "on")
            .env("ALSEM_IR_CACHE_DIR", &f.root)
            .env("ALSEM_IR_CACHE_REPORT", &rep)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "verify {verify}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        if verify {
            let text = String::from_utf8_lossy(&out.stdout);
            assert!(
                text.contains("coldMs"),
                "precondition: reference builds ran: {text}"
            );
        }
        end_counter(&report(&rep), "seam_files")
    };
    let (plain, verified) = (seam(false), seam(true));
    assert!(plain > 0, "precondition: the session used the seam");
    assert_eq!(verified, plain, "the reference builds stayed off the cache");
}

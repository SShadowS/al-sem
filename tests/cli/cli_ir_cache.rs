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

/// TLS-116: a batch run (parent and two workers) reads, writes and reports nothing, with
/// the same environment as the control.
#[test]
fn batch_with_workers_touches_nothing() {
    let f = fixture();
    let dir = tempfile::tempdir().unwrap();
    let rep = dir.path().join("report.jsonl");
    let ws = f.ws.to_str().unwrap();
    let analyze = ["analyze", ws, "--format", "json"];
    let doc = json!({ "batches": [
        { "jobs": [job(&analyze, None, None)] },
        { "jobs": [job(&analyze, None, None)] },
    ]});
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
    let workers = trace
        .iter()
        .filter(|e| e["ph"] == "i" && e["name"] == "batch.worker.start")
        .count();
    assert_eq!(workers, 2, "precondition: two workers ran");
    assert!(entries(&f.root).is_empty(), "nothing written");
    assert!(!f.root.exists(), "no ir folder made");
    assert!(!rep.exists(), "no report line");
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

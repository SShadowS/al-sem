//! #277 T13 (D1): standalone is a batch of one. Every command's standalone
//! trace holds exactly one `batch.driver` instant with `jobs=1` (a command
//! that bypassed the driver would hold none: byte equality alone cannot tell
//! two correct drivers apart, the instant can); `alsem analyze` and the same
//! job as a one-job manifest give equal bytes and counters; and each
//! command's one-job manifest matches the `tests/gate-goldens/batch/one-job/`
//! family (the run line, its stream paths relative to the manifest
//! directory, and the job's stdout).

use std::path::Path;

use serde_json::Value;

use crate::batch_records::{batch_goldens, write_golden};
use crate::cli_batch_plan::{argv, batch_in, standalone_in};
use crate::cli_build_counters::{last_counters, spawn_trace_events};
use crate::regen::regen_mode;

/// The repository root: every job runs from it on a relative workspace, so
/// no output holds a machine's path.
fn root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

const WS: &str = "tests/r0-corpus/ws-d1-multi-caller";

/// Each command, by golden name.
fn commands() -> Vec<(&'static str, Vec<String>)> {
    let d = ["--deterministic"];
    let with = |args: &[&str]| argv(&[args, &d].concat());
    vec![
        ("analyze", with(&["analyze", WS, "--format", "json"])),
        ("digest", with(&["digest", WS, "--routine", "CallerA"])),
        ("prove", with(&["prove", WS, "CallerA", "may-commit"])),
        (
            "fingerprint",
            with(&["fingerprint", WS, "--format", "json"]),
        ),
        ("diff", with(&["diff", WS, WS, "--format", "json"])),
        (
            "events",
            with(&["events", "fanout", WS, "--format", "json"]),
        ),
        ("policy", with(&["policy", "check", WS, "--format", "json"])),
        (
            "query",
            with(&[
                "query",
                "effects",
                WS,
                "--routine",
                "CallerA",
                "--format",
                "json",
            ]),
        ),
        ("graph", argv(&["graph", WS])),
    ]
}

#[test]
fn standalone_is_a_batch_of_one() {
    for (name, job) in commands() {
        let args: Vec<&str> = job.iter().map(String::as_str).collect();
        let (out, events) = spawn_in_root(&args);
        assert!(out.status.code().is_some(), "{name}");
        let drivers: Vec<&Value> = events
            .iter()
            .filter(|e| e["ph"] == "i" && e["name"] == "batch.driver")
            .collect();
        assert_eq!(drivers.len(), 1, "{name}: one batch.driver");
        assert_eq!(drivers[0]["args"]["jobs"], 1, "{name}");
    }

    // `alsem analyze` and its one-job manifest: equal bytes and counters.
    let analyze = &commands()[0].1;
    let args: Vec<&str> = analyze.iter().map(String::as_str).collect();
    let (out, events) = spawn_in_root(&args);
    let run = batch_in(std::slice::from_ref(analyze), Some(root()), &[]);
    let (stdout, stderr) = run.streams(0);
    assert_eq!(stdout, out.stdout);
    assert_eq!(stderr, out.stderr);
    let builds = |m: std::collections::BTreeMap<String, u64>| {
        m.into_iter()
            .filter(|(k, _)| k.starts_with("build.") || k == "inputs.read")
            .collect::<Vec<_>>()
    };
    assert_eq!(builds(run.counters.clone()), builds(last_counters(&events)));

    // The one-job family.
    let dir = batch_goldens().join("one-job");
    for (name, job) in commands() {
        let run = batch_in(std::slice::from_ref(&job), Some(root()), &[]);
        assert_eq!(run.out.status.code(), Some(0), "{name}");
        let mut line: Value = run.lines().remove(0);
        let base = run.dir.path();
        for key in ["stdout", "stderr"] {
            let p = Path::new(line[key].as_str().unwrap()).to_path_buf();
            line[key] = p.strip_prefix(base).unwrap().to_string_lossy().into();
        }
        let line = format!("{}\n", serde_json::to_string_pretty(&line).unwrap());
        let (stdout, _) = run.streams(0);
        // The batch's stdout is the standalone stdout.
        assert_eq!(stdout, standalone_in(&job, Some(root())).0, "{name}");
        let (line_path, stdout_path) = (
            dir.join(format!("{name}.line.golden.json")),
            dir.join(format!("{name}.stdout.golden")),
        );
        assert!(
            line_path.is_file() && stdout_path.is_file(),
            "{name}: seed missing"
        );
        if regen_mode() {
            write_golden(&line_path, line.as_bytes());
            write_golden(&stdout_path, &stdout);
            continue;
        }
        assert_eq!(
            std::fs::read_to_string(&line_path).unwrap(),
            line,
            "{name}: line"
        );
        assert_eq!(
            String::from_utf8_lossy(&std::fs::read(&stdout_path).unwrap()),
            String::from_utf8_lossy(&stdout),
            "{name}: stdout"
        );
    }
}

/// `alsem <args>` from the repository root, traced.
fn spawn_in_root(args: &[&str]) -> (std::process::Output, Vec<Value>) {
    // `spawn_trace_events` runs in the test's cwd, which cargo sets to the
    // package root: the repository root.
    assert_eq!(std::env::current_dir().unwrap(), root());
    spawn_trace_events(env!("CARGO_BIN_EXE_alsem"), args, &[("RUST_LOG", "off")])
}

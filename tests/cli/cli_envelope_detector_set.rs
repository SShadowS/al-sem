//! Issue #68 (T3): digest, prove, fingerprint, events and policy run the SAME
//! detector set that `alsem analyze` runs with no `--detector`/`--preset` — the
//! registry's `default_detectors()`.
//!
//! The observer is the SHIPPED binary's own Chrome trace (`ALSEM_TRACE=1`,
//! `ALSEM_TRACE_FILE=<per-case file>`, set on the child process only): every
//! detector the process runs opens one `detector.<name>` begin (`"ph":"B"`)
//! span (`src/engine/l5/registry.rs`). So a call site, a selector or a
//! `run.rs` body that runs a different set fails here, whichever layer it is
//! in. `detector.result` is an instant event (`"ph":"i"`) and `detectors.total`
//! does not carry the `detector.` prefix, so neither is counted.
//!
//! Per case: (1) exit 0; (2) the trace is a JSON array; (3) every
//! `detector.*` begin name is a REGISTERED detector; (4) each name occurs
//! exactly once; (5) the name set equals `default_detectors()`'s names.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::process::Command;

use al_sem::engine::l5::detectors::{default_detectors, registered_detectors};
use serde_json::Value;

fn corpus(ws: &str) -> String {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("r0-corpus")
        .join(ws)
        .to_string_lossy()
        .into_owned()
}

/// Run `alsem <args>` with tracing on, assert exit 0, and return the multiset
/// of `detector.*` begin-span names (prefix stripped) -> occurrence count.
fn detector_spans(case: &str, args: &[String]) -> BTreeMap<String, usize> {
    let dir = tempfile::tempdir().expect("tempdir");
    let trace = dir.path().join("trace.json");
    // The child inherits the whole environment, so clear every other knob the
    // trace reads (`src/engine/perf_trace.rs` `Config::from_reader`): a stray
    // `ALSEM_TRACE_EXIT_AFTER` would end the run early, and the others change
    // what the trace holds or where it goes.
    let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .args(args)
        .env_remove("ALSEM_TRACE_DETAIL")
        .env_remove("ALSEM_TRACE_SAMPLE_EVERY")
        .env_remove("ALSEM_TRACE_SCC_MIN")
        .env_remove("ALSEM_TRACE_STDERR")
        .env_remove("ALSEM_TRACE_EXIT_AFTER")
        .env("ALSEM_TRACE", "1")
        .env("ALSEM_TRACE_FILE", &trace)
        .output()
        .expect("spawn alsem");
    assert!(
        out.status.success(),
        "[{case}] alsem {args:?} exited {:?}\nstderr: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
    let text = std::fs::read_to_string(&trace)
        .unwrap_or_else(|e| panic!("[{case}] no trace at {}: {e}", trace.display()));
    let events: Vec<Value> = serde_json::from_str(&text)
        .unwrap_or_else(|e| panic!("[{case}] trace is not a JSON array: {e}"));
    let mut spans: BTreeMap<String, usize> = BTreeMap::new();
    for ev in &events {
        if ev.get("ph").and_then(Value::as_str) != Some("B") {
            continue;
        }
        let Some(name) = ev.get("name").and_then(Value::as_str) else {
            continue;
        };
        if let Some(det) = name.strip_prefix("detector.") {
            *spans.entry(det.to_string()).or_default() += 1;
        }
    }
    spans
}

fn assert_runs_default_set(case: &str, args: &[&str]) {
    let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    let spans = detector_spans(case, &args);

    let registered: BTreeSet<String> = registered_detectors().into_iter().map(|d| d.name).collect();
    let unknown: Vec<&String> = spans.keys().filter(|n| !registered.contains(*n)).collect();
    assert!(
        unknown.is_empty(),
        "[{case}] ran unregistered detector(s): {unknown:?}"
    );

    let repeated: Vec<(&String, &usize)> = spans.iter().filter(|(_, c)| **c != 1).collect();
    assert!(
        repeated.is_empty(),
        "[{case}] detector(s) not run exactly once: {repeated:?}"
    );

    let expected: BTreeSet<String> = default_detectors().into_iter().map(|d| d.name).collect();
    let actual: BTreeSet<String> = spans.into_keys().collect();
    let missing: Vec<&String> = expected.difference(&actual).collect();
    let extra: Vec<&String> = actual.difference(&expected).collect();
    assert!(
        missing.is_empty() && extra.is_empty(),
        "[{case}] ran {} detectors, the analyze default set has {}\nmissing: {missing:?}\nextra: {extra:?}",
        actual.len(),
        expected.len()
    );
}

const WS: &str = "ws-d1-multi-caller";

#[test]
fn analyze_cross_app_runs_default_set() {
    let ws = corpus(WS);
    assert_runs_default_set("analyze", &["analyze", &ws, "--format", "json"]);
}

#[test]
fn analyze_single_app_runs_default_set() {
    let ws = corpus(WS);
    assert_runs_default_set(
        "analyze --single-app",
        &["analyze", &ws, "--format", "json", "--single-app"],
    );
}

#[test]
fn digest_cross_app_runs_default_set() {
    let ws = corpus(WS);
    assert_runs_default_set(
        "digest",
        &[
            "digest",
            &ws,
            "--file",
            "src/D1MultiCaller.al",
            "--deterministic",
        ],
    );
}

#[test]
fn digest_single_app_runs_default_set() {
    let ws = corpus(WS);
    assert_runs_default_set(
        "digest --single-app",
        &[
            "digest",
            &ws,
            "--file",
            "src/D1MultiCaller.al",
            "--deterministic",
            "--single-app",
        ],
    );
}

#[test]
fn prove_runs_default_set() {
    let ws = corpus("ws-d34");
    assert_runs_default_set(
        "prove",
        &[
            "prove",
            &ws,
            "DirectCommitInLoop",
            "may-commit",
            "--deterministic",
        ],
    );
}

#[test]
fn fingerprint_runs_default_set() {
    let ws = corpus(WS);
    assert_runs_default_set(
        "fingerprint",
        &["fingerprint", &ws, "--format", "json", "--deterministic"],
    );
}

#[test]
fn events_fanout_runs_default_set() {
    let ws = corpus(WS);
    assert_runs_default_set(
        "events fanout",
        &[
            "events",
            "fanout",
            &ws,
            "--format",
            "json",
            "--deterministic",
        ],
    );
}

#[test]
fn events_chains_runs_default_set() {
    let ws = corpus(WS);
    assert_runs_default_set(
        "events chains",
        &[
            "events",
            "chains",
            &ws,
            "--format",
            "json",
            "--deterministic",
        ],
    );
}

#[test]
fn policy_check_runs_default_set() {
    let ws = corpus(WS);
    assert_runs_default_set(
        "policy check",
        &[
            "policy",
            "check",
            &ws,
            "--format",
            "json",
            "--deterministic",
        ],
    );
}

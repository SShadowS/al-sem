//! #277 T7 (spec D11, 12A-5): the `Arc` census. One cross-app view of
//! `ws_dep()` holds four default-detector jobs (analyze, digest, policy,
//! events fanout). Shared allocations are counted once: the model, `rel`,
//! `det` and `l4` buckets print once, at the largest standalone job's bytes,
//! and the runs hold one allocation per DISTINCT `RunKey`. Union storage is
//! stated, not capped: `raw` lies between the largest and the sum of the
//! jobs' own. Census lines are read from the run's own stderr (Q-d), run
//! keys from its trace.

use std::collections::BTreeSet;

use serde_json::Value;

use crate::cli_batch_plan::{argv, batch_in};
use crate::cli_build_counters::{policy_yaml, spawn_trace_events, write_diff, ws_dep};

const CENSUS: (&str, &str) = ("ALSEM_RETAINED_CENSUS", "1");

/// The `[retained-census]` lines of a stderr, prefix stripped.
fn census(stderr: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(stderr)
        .lines()
        .filter_map(|l| l.strip_prefix("[retained-census] "))
        .map(str::to_string)
        .collect()
}

/// `key=value`'s value in a census line.
fn field(line: &str, key: &str) -> Option<u64> {
    line.split(' ')
        .find_map(|kv| kv.strip_prefix(key)?.strip_prefix('='))
        .map(|v| v.parse().unwrap())
}

/// The context maps a `DetectorContext` borrows, printed per construction
/// (one per run), not with `det`'s one fill.
const PER_CONTEXT: &[&str] = &[
    "context.routine_by_id",
    "context.objects_by_id",
    "context.table_by_id",
    "context.call_site_by_id",
    "context.fingerprint_index",
];

/// The buckets of the four shared cells: `(name, bytes)` per line.
fn shared_buckets(lines: &[String]) -> Vec<(String, u64)> {
    lines
        .iter()
        .filter_map(|l| {
            let name = l.strip_prefix("bucket=")?.split(' ').next()?;
            let shared = name == "model"
                || name.starts_with("rel.")
                || name.starts_with("l4.")
                || (name.starts_with("context.") && !PER_CONTEXT.contains(&name));
            shared.then(|| (name.to_string(), field(l, "bytes").unwrap()))
        })
        .collect()
}

/// `batch.shared cell=<cell>`'s line.
fn shared<'a>(lines: &'a [String], cell: &str) -> &'a str {
    let want = format!("batch.shared cell={cell} ");
    let hits: Vec<&String> = lines.iter().filter(|l| l.starts_with(&want)).collect();
    assert_eq!(hits.len(), 1, "one {cell} line: {lines:?}");
    hits[0]
}

/// The `witness.raw_bytes` of the run's composition (0 with none).
fn raw_bytes(lines: &[String]) -> u64 {
    let raw: Vec<u64> = lines
        .iter()
        .filter_map(|l| field(l, "witness.raw_bytes"))
        .collect();
    assert!(raw.len() <= 1, "at most one composition: {lines:?}");
    raw.first().copied().unwrap_or(0)
}

/// Each `view.run` instant's key: (detectors, demanded, scopePrimary).
fn run_keys(events: &[Value]) -> Vec<String> {
    events
        .iter()
        .filter(|e| e["ph"] == "i" && e["name"] == "view.run")
        .map(|e| e["args"].to_string())
        .collect()
}

#[test]
fn batch_arc_census() {
    let (d, ws) = ws_dep();
    let diff = write_diff(d.path(), "src/Work.al", 18, "        Customer.Insert();");
    let policy = policy_yaml();
    let d_ = ["--deterministic"];
    let with = |a: &[&str]| argv(&[a, &d_].concat());
    let jobs = vec![
        with(&["analyze", &ws, "--format", "json"]),
        with(&["digest", &ws, "--diff", &diff]),
        with(&[
            "policy", "check", &ws, "--policy", &policy, "--format", "json",
        ]),
        with(&["events", "fanout", &ws, "--format", "json"]),
    ];
    let run = batch_in(&jobs, None, &[CENSUS]);
    assert_eq!(
        run.out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&run.out.stderr)
    );
    let batch = census(&run.out.stderr);
    for i in 0..jobs.len() {
        let (_, stderr) = run.streams(i);
        assert!(census(&stderr).is_empty(), "job {i}: no census line");
    }

    // Each job standalone: its census and its run keys.
    let alone: Vec<(Vec<String>, Vec<String>)> = jobs
        .iter()
        .map(|job| {
            let args: Vec<&str> = job.iter().map(String::as_str).collect();
            let (out, events) = spawn_trace_events(
                env!("CARGO_BIN_EXE_alsem"),
                &args,
                &[CENSUS, ("RUST_LOG", "off")],
            );
            assert_eq!(out.status.code(), Some(0), "{job:?}");
            (census(&out.stderr), run_keys(&events))
        })
        .collect();

    // Precondition (O19): every job's run mask is the union mask, so `det`
    // and `l4` built at the union equal each job's own.
    let demanded = |keys: &[String]| -> BTreeSet<u64> {
        keys.iter()
            .map(|k| {
                serde_json::from_str::<Value>(k).unwrap()["demanded"]
                    .as_u64()
                    .unwrap()
            })
            .collect()
    };
    let batch_keys = run_keys(&run.events);
    let mut all = demanded(&batch_keys);
    for (_, keys) in &alone {
        all.extend(demanded(keys));
    }
    assert_eq!(all.len(), 1, "one distinct demanded mask: {all:?}");

    // Dedup: each shared bucket once, at the largest standalone's bytes.
    let got = shared_buckets(&batch);
    let names: BTreeSet<&String> = got.iter().map(|(n, _)| n).collect();
    assert_eq!(names.len(), got.len(), "each shared bucket once: {got:?}");
    assert!(
        names.contains(&"model".to_string()) && names.iter().any(|n| n.starts_with("context.")),
        "precondition: the model and det buckets print: {got:?}"
    );
    for (name, bytes) in &got {
        let max = alone
            .iter()
            .flat_map(|(c, _)| shared_buckets(c))
            .filter(|(n, _)| n == name)
            .map(|(_, b)| b)
            .max()
            .unwrap_or_else(|| panic!("{name}: no standalone prints it"));
        assert_eq!(*bytes, max, "{name}");
    }

    // Runs: one allocation per distinct key, the standalone bytes once each.
    let distinct: BTreeSet<&String> = batch_keys.iter().collect();
    assert_eq!(distinct.len(), batch_keys.len(), "one run per key");
    let mut want = 0;
    let mut seen = BTreeSet::new();
    for (c, keys) in &alone {
        assert_eq!(keys.len(), 1, "one run per standalone job");
        if seen.insert(keys[0].clone()) {
            want += field(shared(c, "runs"), "bytes").unwrap();
        }
    }
    assert_eq!(
        seen,
        distinct.into_iter().cloned().collect(),
        "the same keys"
    );
    let runs = shared(&batch, "runs");
    assert!(want > 0, "precondition: the runs hold bytes");
    assert_eq!(field(runs, "bytes"), Some(want), "{runs}");
    assert_eq!(field(runs, "allocs"), Some(seen.len() as u64), "{runs}");

    // job_refs, stated: every job reads the model, rel, l4, det and a run;
    // digest's tree and policy's rules read raw.
    for (cell, refs) in [
        ("model", 4),
        ("rel", 4),
        ("l4", 4),
        ("det", 4),
        ("raw", 2),
        ("runs", 4),
    ] {
        let line = shared(&batch, cell);
        assert_eq!(field(line, "job_refs"), Some(refs), "{line}");
    }

    // Union: raw grows with the subjects, no clone.
    let raws: Vec<u64> = alone.iter().map(|(c, _)| raw_bytes(c)).collect();
    let raw = raw_bytes(&batch);
    let (max, sum) = (*raws.iter().max().unwrap(), raws.iter().sum::<u64>());
    assert!(
        raw >= max && raw <= sum,
        "raw {raw} in [{max}, {sum}]: {raws:?}"
    );
}

/// #277 final r1 FR-4 (D11): with the census on, a query on a cross-app view
/// releases the base before the jobs after it print their output census
/// (inventory and routine fingerprints, prove); each prints its line and
/// equals its standalone run.
#[test]
fn output_census_after_a_released_base() {
    let (_d, ws) = ws_dep();
    let d_ = ["--deterministic"];
    let with = |a: &[&str]| argv(&[a, &d_].concat());
    let jobs = vec![
        with(&["query", "touches", &ws, "--table", "18", "--format", "json"]),
        with(&["fingerprint", &ws, "--inventory-only"]),
        with(&["prove", &ws, "CallLib", "may-commit"]),
        with(&[
            "fingerprint",
            &ws,
            "--routine",
            "CallLib",
            "--format",
            "json",
        ]),
    ];
    let run = batch_in(&jobs, None, &[CENSUS]);
    crate::cli_batch_plan::assert_each_standalone(&run, &jobs, None);
    let outputs = |stderr: &[u8]| {
        census(stderr)
            .into_iter()
            .filter(|l| l.starts_with("view.output "))
            .count()
    };
    let alone: usize = jobs
        .iter()
        .map(|job| {
            let args: Vec<&str> = job.iter().map(String::as_str).collect();
            let (out, _) = spawn_trace_events(env!("CARGO_BIN_EXE_alsem"), &args, &[CENSUS]);
            outputs(&out.stderr)
        })
        .sum();
    assert!(
        alone >= 2,
        "precondition: the jobs print output census lines"
    );
    assert_eq!(outputs(&run.out.stderr), alone, "each job's, as standalone");
}

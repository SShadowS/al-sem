//! `aldump --snapshot-memory <ws> [--single-app] [--hash]` measures
//! `compose_snapshot` so the cone-memory targets can be re-measured
//! (`docs/superpowers/specs/2026-10-31-cone-memory-design.md`, "Re-measuring").
//! These tests pin its JSON shape on a corpus fixture, the identity that ties
//! the counts together, and that `--hash` is deterministic across processes.
//! Timings and memory vary per run, so only their presence and type are checked.

use std::path::PathBuf;
use std::process::Command;

use serde_json::Value;

/// A fixture with inherited facts and a non-empty producer-solve arena.
fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/r0-corpus/ws-explain-calls")
}

fn run(extra: &[&str]) -> Value {
    let out = Command::new(env!("CARGO_BIN_EXE_aldump"))
        .arg("--snapshot-memory")
        .arg(fixture())
        .args(extra)
        .env("RUST_LOG", "off")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "aldump --snapshot-memory {extra:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).expect("stdout is one JSON document")
}

fn count(v: &Value, path: &str) -> u64 {
    v.pointer(path)
        .and_then(Value::as_u64)
        .unwrap_or_else(|| panic!("{path} is not a count in {v:#}"))
}

/// Phase names in order; every phase has a wall time and memory fields
/// (a number, or `null` off Linux).
fn phase_names(v: &Value) -> Vec<String> {
    let phases = v["phases"].as_array().expect("phases is an array");
    for p in phases {
        assert!(p["secs"].as_f64().is_some_and(|s| s >= 0.0), "{p}");
        for k in ["rssKiB", "peakRssKiB"] {
            assert!(p[k].is_u64() || p[k].is_null(), "{k} in {p}");
        }
    }
    phases
        .iter()
        .map(|p| p["name"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn reports_phases_counts_and_row_sizes() {
    let v = run(&[]);
    assert_eq!(v["scope"], "crossApp");
    assert_eq!(phase_names(&v), ["model", "coneBase", "composeSnapshot"]);
    assert!(v["marks"].is_array());
    assert!(v["peakRssKiB"].is_u64() || v["peakRssKiB"].is_null());
    assert_eq!(v["hash"], Value::Null, "no hashes without --hash");
    assert_eq!(count(&v, "/rowBytes/snapshotCapabilityFact"), 16);
    assert!(count(&v, "/rowBytes/capabilityFact") > 16);

    let inherited = count(&v, "/counts/inheritedFacts");
    let direct = count(&v, "/counts/directFacts");
    assert!(inherited > 0 && direct > 0, "fixture lost its facts: {v:#}");
    // Every snapshot row is one raw inherited fact or one direct fact.
    assert_eq!(
        count(&v, "/counts/snapshot/capabilityFacts"),
        inherited + direct
    );
    // One snapshot rep per direct fact, plus the projected inherited reps.
    assert!(count(&v, "/counts/snapshot/tables/reps") >= direct);
    assert!(count(&v, "/counts/inheritedTables/reps") > 0);
    assert!(count(&v, "/counts/inheritedTables/hops") > 0);
    assert!(count(&v, "/counts/pdArena/states") > 0);
    for k in [
        "/counts/modelRoutines",
        "/counts/modelCalls",
        "/counts/coneTypedEdges",
        "/counts/routinesWithInherited",
        "/counts/inheritedTables/subjects",
        "/counts/coneCoverage",
        "/counts/snapshot/tables/subjects",
        "/counts/snapshot/tables/hops",
        "/counts/snapshot/typedEdges",
        "/counts/snapshot/operationIndex",
        "/counts/snapshot/callsiteIndex",
        "/counts/snapshot/callsiteResolutions",
        "/counts/snapshot/coverage",
        "/counts/pdArena/links",
        "/counts/pdArena/routines",
    ] {
        count(&v, k);
    }

    assert_eq!(run(&["--single-app"])["scope"], "singleApp");
}

#[test]
fn hash_is_deterministic_and_covers_every_section() {
    let a = run(&["--hash"]);
    assert_eq!(
        phase_names(&a),
        ["model", "coneBase", "composeSnapshot", "hash"]
    );
    let h = &a["hash"];
    let sections = h["sections"].as_array().expect("sections");
    let names: Vec<&str> = sections
        .iter()
        .map(|s| s["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "identities",
            "capabilityFacts",
            "typedEdges",
            "operationIndex",
            "callsiteIndex",
            "callsiteResolutions",
            "analysisGaps",
            "coverage",
            "eventDeclarations",
            "rootClassifications",
            "routineOrderFrames",
        ]
    );
    let is_hex = |s: &Value| s.as_str().is_some_and(|s| s.len() == 64);
    assert!(sections.iter().all(|s| is_hex(&s["blake3"])), "{h:#}");
    assert!(is_hex(&h["document"]["blake3"]) && is_hex(&h["pdArena"]["blake3"]));
    // The document is the sections concatenated in order.
    let section_bytes: u64 = sections.iter().map(|s| s["bytes"].as_u64().unwrap()).sum();
    assert_eq!(count(&a, "/hash/document/bytes"), section_bytes);

    // A second process (another hash-map seed) hashes to the same values.
    let b = run(&["--hash"]);
    assert_eq!(a["hash"], b["hash"]);
}

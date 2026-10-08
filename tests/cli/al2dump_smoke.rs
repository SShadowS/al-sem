//! R1a Task 3 smoke test: run the workspace-level L2 features emitter
//! (`aldump --l2`) on the vendored ws-d2 fixture and assert:
//!   1. the output parses as JSON and has the golden top-level shape;
//!   2. a known routine's `features` (operationSite/recordOp + a loop callsite)
//!      match `ws-d2.l2.golden.json` (the golden is ground truth);
//!   3. NO forbidden later-gate / L3-resolved field key appears ANYWHERE in the
//!      output (recursive key scan).
//!
//! Task 3.3 (al-sem parity retirement) vendored the ws-d2 fixture tree and its
//! L2 golden in-repo (`tests/fixtures/ws-d2/`, `tests/al2dump-smoke-goldens/`);
//! this test no longer reads from any al-sem checkout and hard-requires its
//! inputs (no skip-gate). If ws-d2's L2 features diverge from the golden, that
//! is a real walker/emitter bug; the fix belongs in `src/engine/l2/**`, not here.

use al_sem::program::body::l2_workspace::project_workspace;
use std::path::PathBuf;

use crate::regen;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Vendored ws-d2 fixture (Task 3.3; see `tests/fixtures/ws-d2/PROVENANCE.md`).
fn ws_d2_dir() -> PathBuf {
    repo_root().join("tests").join("fixtures").join("ws-d2")
}

/// In-repo home for the L2 golden, regenerated from THIS engine (Task 3.3) —
/// Rust-owned baseline, not a copy of al-sem's TS output.
fn ws_d2_golden_path() -> PathBuf {
    repo_root()
        .join("tests")
        .join("al2dump-smoke-goldens")
        .join("ws-d2.l2.golden.json")
}

/// Keys that must NEVER appear anywhere in the L2 projection (later-gate / L3 —
/// mirrors `scripts/r1a-l2-projection.ts` FORBIDDEN_KEYS + the binding subset).
const FORBIDDEN_KEYS: &[&str] = &[
    // R1b: controlContext is now emitted + compared (no longer forbidden).
    // R1c: order + scopeFrames are now emitted + compared (no longer forbidden).
    // R1d: capabilityFactsDirect/Status/Reasons/Diagnostics now emitted + compared
    //   (no longer forbidden). Only the L3-resolved fields remain forbidden — a
    //   nested `tableId` (table-field ValueSource) still hard-fails.
    "resourceId",
    "tableId",
    "calleeParameterIsVar",
    "bindingResolution",
    "sourceTableId",
];

/// Recursively collect every object key in a JSON value.
fn scan_forbidden(value: &serde_json::Value, hits: &mut Vec<String>) {
    match value {
        serde_json::Value::Object(map) => {
            for (k, v) in map {
                if FORBIDDEN_KEYS.contains(&k.as_str()) {
                    hits.push(k.clone());
                }
                scan_forbidden(v, hits);
            }
        }
        serde_json::Value::Array(arr) => {
            for v in arr {
                scan_forbidden(v, hits);
            }
        }
        _ => {}
    }
}

#[test]
fn ws_d2_l2_features_match_golden_and_have_no_forbidden_fields() {
    let ws = ws_d2_dir();
    assert!(
        ws.is_dir(),
        "vendored ws-d2 fixture missing at {} (Task 3.3 vendoring)",
        ws.display()
    );

    let projection = project_workspace(&ws).expect("project_workspace should succeed on ws-d2");

    // (1) Serializes + parses as JSON, with the golden top-level shape.
    let json = serde_json::to_string_pretty(&projection).expect("projection serializes to JSON");
    let parsed: serde_json::Value =
        serde_json::from_str(&json).expect("emitted output parses as JSON");
    assert!(parsed.get("objects").is_some(), "top-level has `objects`");
    assert!(parsed.get("routines").is_some(), "top-level has `routines`");
    assert_eq!(projection.objects.len(), 3, "ws-d2 has 3 objects");
    assert_eq!(projection.routines.len(), 7, "ws-d2 has 7 routines");

    // (3) Forbidden-field guard: recursive key scan over the WHOLE output.
    let mut hits = Vec::new();
    scan_forbidden(&parsed, &mut hits);
    assert!(
        hits.is_empty(),
        "forbidden later-gate/L3 field(s) leaked into the L2 output: {hits:?}"
    );

    let golden_path = ws_d2_golden_path();

    // REGEN path (Task 3.3 vendoring): `REGEN_TEMP_GOLDENS=1` writes the ENGINE
    // projection to the in-repo golden instead of comparing — this is a
    // Rust-owned baseline, not a copy of al-sem's TS output.
    if regen::regen_mode() {
        let mut pretty =
            serde_json::to_string_pretty(&projection).expect("regen serialize l2 ws-d2");
        pretty.push('\n');
        std::fs::create_dir_all(golden_path.parent().expect("golden has a parent"))
            .expect("create al2dump-smoke-goldens dir");
        std::fs::write(&golden_path, pretty)
            .unwrap_or_else(|e| panic!("regen write {}: {e}", golden_path.display()));
        eprintln!("REGEN al2dump-smoke l2 golden: {}", golden_path.display());
        return;
    }

    // (2) Known-routine feature parity against the golden. Assert the FULL
    // projection equals it (the strongest smoke).
    assert!(
        golden_path.is_file(),
        "missing golden {} (run `REGEN_TEMP_GOLDENS=1 cargo test --test cli al2dump_smoke::`)",
        golden_path.display()
    );
    let golden_text = std::fs::read_to_string(&golden_path).expect("read ws-d2 golden");
    let golden: serde_json::Value =
        serde_json::from_str(&golden_text).expect("golden parses as JSON");
    assert_eq!(
        parsed, golden,
        "ws-d2 L2 projection must match ws-d2.l2.golden.json exactly (golden is ground truth)"
    );

    // Targeted assertions (independent of the golden file's presence on disk) on
    // the known subscriber routine `HandleProcessLine` — a record-op operation
    // site + recordOperation, and the publisher loop callsite shape.
    let handle = projection
        .routines
        .iter()
        .find(|r| r.name == "HandleProcessLine")
        .expect("ws-d2 has HandleProcessLine");
    assert_eq!(handle.kind, "event-subscriber");
    assert_eq!(handle.access_modifier.as_deref(), Some("local"));
    assert_eq!(
        handle.features.operation_sites.len(),
        1,
        "HandleProcessLine has one record-op operation site"
    );
    assert_eq!(handle.features.operation_sites[0].kind, "record-op");
    assert_eq!(
        handle.features.record_operations.len(),
        1,
        "HandleProcessLine has one record operation"
    );
    assert_eq!(handle.features.record_operations[0].op, "FindSet");
    assert_eq!(
        handle.features.record_operations[0].record_variable_name,
        "Customer"
    );

    let raise = projection
        .routines
        .iter()
        .find(|r| r.name == "RaiseInLoop")
        .expect("ws-d2 has RaiseInLoop");
    assert_eq!(raise.kind, "procedure");
    assert_eq!(raise.features.loops.len(), 1, "RaiseInLoop has one loop");
    assert_eq!(raise.features.loops[0].loop_type, "for");
    assert_eq!(
        raise.features.call_sites.len(),
        1,
        "RaiseInLoop has one call site"
    );
    // The callsite is nested in the loop — its loopStack carries the loop id.
    assert_eq!(
        raise.features.call_sites[0].loop_stack,
        vec![raise.features.loops[0].id.clone()],
        "the call site is inside the loop"
    );
}

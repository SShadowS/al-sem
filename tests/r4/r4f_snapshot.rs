//! R4-F Stage-2b — CapabilitySnapshot CONSUMED-CORE stable-projection differential.
//!
//! For each committed al-sem golden under
//! `tests/r4f-goldens/<fixture>.snapshot.golden.json`, run the Rust source-only
//! L0→L3 pass (`assemble_and_resolve_workspace_with_program_calls(...)`) over the matching
//! `tests/r0-corpus/<fixture>` workspace, compose + project the consumed-core
//! CapabilitySnapshot (`project_r4f_snapshot`), pretty-serialize (serde_json pretty
//! + trailing newline — the exact on-disk golden form), and assert BYTE-equality.
//!
//! ## Anti-degenerate
//!
//! - `ws-txn-d47-event-pos`: >=1 event-dispatch typedEdge (16-hex edgeId) AND
//!   >=1 eventDeclaration.
//! - `ws-txn-d47-crosshop-iobeforecommit`: >=1 capabilityFact with
//!   provenance "inherited".
//! - EVERY fixture: >=1 capabilityFact + non-empty identities.

use std::path::PathBuf;

use al_sem::engine::l5::snapshot::project_r4f_snapshot;
use al_sem::program::model::program_calls::assemble_and_resolve_workspace_with_program_calls;

use crate::regen;

/// The R4-F snapshot corpus (mirrors al-sem `R4F_SNAPSHOT_FIXTURES`).
const FIXTURES: &[&str] = &[
    "ws-txn-d47-pos-http-nocommit",
    "ws-txn-d47-crosshop-iobeforecommit",
    "ws-txn-d47-event-pos",
    "ws-txn-d49-pos-modify-message",
    "ws-d51-pos",
];

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn goldens_dir() -> PathBuf {
    repo_root().join("tests").join("r4f-goldens")
}

fn corpus_dir() -> PathBuf {
    repo_root().join("tests").join("r0-corpus")
}

/// Run the Rust source-only L0→L3 pass + snapshot projection for one fixture,
/// returning the pretty-serialized + trailing-newline STRING (the on-disk form).
fn run_rust(fixture: &str) -> String {
    let fixture_dir = corpus_dir().join(fixture);
    assert!(
        fixture_dir.is_dir(),
        "R4-F snapshot golden for {fixture} has no matching in-repo fixture at {} \
         (offline corpus incomplete)",
        fixture_dir.display()
    );
    match assemble_and_resolve_workspace_with_program_calls(&fixture_dir) {
        Some(resolved) => project_r4f_snapshot(&resolved, fixture),
        None => format!("{{\n  \"fixtureName\": \"{fixture}\"\n}}\n"),
    }
}

/// Parse the projection string into a `serde_json::Value` for field inspection
/// (anti-degenerate guards; field lookups are key-based, order-agnostic).
fn run_rust_value(fixture: &str) -> serde_json::Value {
    serde_json::from_str(&run_rust(fixture)).expect("projection is valid JSON")
}

#[test]
fn r4f_snapshot_matches_goldens() {
    for fixture in FIXTURES {
        let golden_path = goldens_dir().join(format!("{fixture}.snapshot.golden.json"));
        let golden_text = std::fs::read_to_string(&golden_path).unwrap_or_else(|e| {
            panic!(
                "cannot read R4-F snapshot golden {}: {e}",
                golden_path.display()
            )
        });

        let rust_text = run_rust(fixture);

        // Rust-owned baseline: `REGEN_TEMP_GOLDENS=1` rewrites the golden from THIS
        // engine (al-sem byte-parity retired — see CLAUDE.md). The anti-degenerate
        // oracles below assert the structural contract regardless.
        if regen::regen_mode() {
            std::fs::write(&golden_path, &rust_text)
                .unwrap_or_else(|e| panic!("regen write {}: {e}", golden_path.display()));
            eprintln!("REGEN r4f snapshot golden: {}", golden_path.display());
            continue;
        }

        assert_eq!(
            rust_text,
            golden_text,
            "R4-F ACCEPTANCE GATE: {fixture} did NOT byte-match its snapshot golden ({})",
            golden_path.display()
        );
    }
}

// ---------------------------------------------------------------------------
// Anti-degenerate guards
// ---------------------------------------------------------------------------

fn snapshot_of(value: &serde_json::Value) -> &serde_json::Value {
    value.get("snapshot").expect("projection has a `snapshot`")
}

#[test]
fn anti_degenerate_every_fixture_has_facts_and_identities() {
    for fixture in FIXTURES {
        let proj = run_rust_value(fixture);
        let snap = snapshot_of(&proj);
        let facts = snap
            .get("capabilityFacts")
            .and_then(|v| v.as_array())
            .expect("capabilityFacts is an array");
        assert!(!facts.is_empty(), "{fixture}: expected >=1 capabilityFact");
        let stable_ids = snap
            .get("identities")
            .and_then(|v| v.get("stableIds"))
            .and_then(|v| v.as_array())
            .expect("identities.stableIds is an array");
        assert!(
            !stable_ids.is_empty(),
            "{fixture}: expected non-empty identities"
        );
    }
}

#[test]
fn anti_degenerate_event_pos_has_event_dispatch_edge_and_declaration() {
    let proj = run_rust_value("ws-txn-d47-event-pos");
    let snap = snapshot_of(&proj);

    let typed_edges = snap
        .get("typedEdges")
        .and_then(|v| v.as_array())
        .expect("typedEdges is an array");
    let event_dispatch = typed_edges
        .iter()
        .find(|e| e.get("kind").and_then(|v| v.as_str()) == Some("event-dispatch"));
    let edge = event_dispatch.expect("ws-txn-d47-event-pos must have an event-dispatch typedEdge");
    let edge_id = edge
        .get("edgeId")
        .and_then(|v| v.as_str())
        .expect("event-dispatch edge carries an edgeId");
    assert_eq!(
        edge_id.len(),
        16,
        "event-dispatch edgeId must be 16 hex chars, got {edge_id:?}"
    );
    assert!(
        edge_id.chars().all(|c| c.is_ascii_hexdigit()),
        "event-dispatch edgeId must be hex, got {edge_id:?}"
    );

    let event_decls = snap
        .get("eventDeclarations")
        .and_then(|v| v.as_array())
        .expect("eventDeclarations is an array");
    assert!(
        !event_decls.is_empty(),
        "ws-txn-d47-event-pos must have >=1 eventDeclaration"
    );
}

#[test]
fn anti_degenerate_crosshop_has_inherited_fact() {
    let proj = run_rust_value("ws-txn-d47-crosshop-iobeforecommit");
    let snap = snapshot_of(&proj);
    let facts = snap
        .get("capabilityFacts")
        .and_then(|v| v.as_array())
        .expect("capabilityFacts is an array");
    assert!(
        facts
            .iter()
            .any(|f| { f.get("provenance").and_then(|v| v.as_str()) == Some("inherited") }),
        "ws-txn-d47-crosshop-iobeforecommit must have >=1 inherited capabilityFact"
    );
}

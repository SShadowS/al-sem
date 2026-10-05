//! R4-F Stage-3b — DIGEST effects stable-projection differential.
//!
//! For each committed al-sem golden under
//! `tests/r4f-goldens/<fixture>.digest.golden.json`, run the Rust source-only
//! L0→L3 pass over the matching `tests/r0-corpus/<fixture>` workspace, compose the
//! CapabilitySnapshot, run the digest witness + effects + occurrence-build path
//! (`project_r4f_digest_effects`), pretty-serialize (serde_json pretty + trailing
//! newline — the exact on-disk golden form), and assert BYTE-equality.
//!
//! The `occurrenceId` (= `factId`) is the parity crux:
//!   factId = sha256Hex(routineId|linkSignature|kind|terminalId|effectType)[0..16].
//!
//! ## Anti-degenerate
//!
//! - >=1 effect with a multi-hop viaPath (viaPaths[0].len >= 2) [event-pos
//!   > ProcessAndNotify HTTP, factId 2d2c85f05c8bac52].
//! - >=1 event-dispatch hop in event-pos.
//! - Every entry's effects each carry a 16-hex factId.

use std::path::PathBuf;

use al_sem::engine::l3::program_calls::assemble_and_resolve_workspace_with_program_calls;
use al_sem::engine::l5::digest::project_r4f_digest_effects;

use crate::regen;

/// The R4-F digest-effects corpus (mirrors al-sem `R4F_DIGEST_FIXTURES`).
const FIXTURES: &[&str] = &[
    "ws-txn-d47-pos-http-nocommit",
    "ws-txn-d47-crosshop-iobeforecommit",
    "ws-txn-d47-event-pos",
    "ws-txn-d49-pos-modify-message",
    "ws-txn-d49-pos-modify-runmodal",
    "ws-txn-d47-pos-file",
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

/// Run the Rust source-only L0→L3 pass + digest-effects projection for one fixture,
/// returning the pretty-serialized + trailing-newline STRING (the on-disk form).
fn run_rust(fixture: &str) -> String {
    let fixture_dir = corpus_dir().join(fixture);
    assert!(
        fixture_dir.is_dir(),
        "R4-F digest golden for {fixture} has no matching in-repo fixture at {} \
         (offline corpus incomplete)",
        fixture_dir.display()
    );
    match assemble_and_resolve_workspace_with_program_calls(&fixture_dir) {
        Some(resolved) => project_r4f_digest_effects(&resolved, fixture),
        None => format!(
            "{{\n  \"fixtureName\": \"{fixture}\",\n  \"entryCount\": 0,\n  \"entries\": []\n}}\n"
        ),
    }
}

fn run_rust_value(fixture: &str) -> serde_json::Value {
    serde_json::from_str(&run_rust(fixture)).expect("projection is valid JSON")
}

#[test]
fn r4f_digest_effects_matches_goldens() {
    for fixture in FIXTURES {
        let golden_path = goldens_dir().join(format!("{fixture}.digest.golden.json"));

        let rust_text = run_rust(fixture);

        // REGEN path (Task T0.6 — this family previously had none). When
        // `REGEN_TEMP_GOLDENS=1`, write the ENGINE output straight to the golden
        // file instead of comparing — the goldens are Rust-owned baselines (TS
        // oracle retired). `run_rust` already returns the exact on-disk pretty +
        // trailing-newline form the assert path below reads.
        if regen::regen_mode() {
            std::fs::write(&golden_path, &rust_text)
                .unwrap_or_else(|e| panic!("regen write {}: {e}", golden_path.display()));
            eprintln!("REGEN r4f digest-effects golden: {}", golden_path.display());
            continue;
        }

        let golden_text = std::fs::read_to_string(&golden_path).unwrap_or_else(|e| {
            panic!(
                "cannot read R4-F digest golden {}: {e}",
                golden_path.display()
            )
        });

        assert_eq!(
            rust_text,
            golden_text,
            "R4-F ACCEPTANCE GATE: {fixture} did NOT byte-match its digest golden ({})",
            golden_path.display()
        );
    }
}

// ---------------------------------------------------------------------------
// Anti-degenerate guards
// ---------------------------------------------------------------------------

#[test]
fn anti_degenerate_multi_hop_via_path_exists() {
    // >=1 effect with viaPaths[0].len >= 2 across the corpus (event-pos provides it).
    let mut found = false;
    for fixture in FIXTURES {
        let proj = run_rust_value(fixture);
        let entries = proj.get("entries").and_then(|v| v.as_array()).unwrap();
        for entry in entries {
            for eff in entry.get("effects").and_then(|v| v.as_array()).unwrap() {
                if let Some(first) = eff
                    .get("viaPaths")
                    .and_then(|v| v.as_array())
                    .and_then(|p| p.first())
                    .and_then(|p| p.as_array())
                    && first.len() >= 2
                {
                    found = true;
                }
            }
        }
    }
    assert!(
        found,
        "expected >=1 effect with a multi-hop viaPath (len >= 2)"
    );
}

#[test]
fn anti_degenerate_event_pos_multi_hop_http_fact_id() {
    // event-pos ProcessAndNotify HTTP — 2-hop path (call + event-dispatch), factId
    // 2d2c85f05c8bac52. Also asserts the event-dispatch hop kind is present.
    let proj = run_rust_value("ws-txn-d47-event-pos");
    let entries = proj.get("entries").and_then(|v| v.as_array()).unwrap();

    let mut found_fact_id = false;
    let mut found_event_dispatch_hop = false;
    for entry in entries {
        for eff in entry.get("effects").and_then(|v| v.as_array()).unwrap() {
            if eff.get("factId").and_then(|v| v.as_str()) == Some("2d2c85f05c8bac52") {
                found_fact_id = true;
            }
            for path in eff.get("viaPaths").and_then(|v| v.as_array()).unwrap() {
                for hop in path.as_array().unwrap() {
                    if hop.get("kind").and_then(|v| v.as_str()) == Some("event-dispatch") {
                        found_event_dispatch_hop = true;
                    }
                }
            }
        }
    }
    assert!(
        found_fact_id,
        "event-pos multi-hop HTTP factId 2d2c85f05c8bac52 must be present"
    );
    assert!(
        found_event_dispatch_hop,
        "event-pos must carry >=1 event-dispatch hop in a viaPath"
    );
}

#[test]
fn anti_degenerate_every_effect_has_16_hex_fact_id() {
    for fixture in FIXTURES {
        let proj = run_rust_value(fixture);
        let entries = proj.get("entries").and_then(|v| v.as_array()).unwrap();
        for entry in entries {
            for eff in entry.get("effects").and_then(|v| v.as_array()).unwrap() {
                let fact_id = eff
                    .get("factId")
                    .and_then(|v| v.as_str())
                    .unwrap_or_else(|| panic!("{fixture}: effect missing factId"));
                assert_eq!(
                    fact_id.len(),
                    16,
                    "{fixture}: factId must be 16 hex chars, got {fact_id:?}"
                );
                assert!(
                    fact_id.chars().all(|c| c.is_ascii_hexdigit()),
                    "{fixture}: factId must be lowercase hex, got {fact_id:?}"
                );
            }
        }
    }
}

//! cli-b/b0 — the full `CapabilitySnapshot` serializer differential.
//!
//! For each fixture in `SNAPSHOT_CORPUS`, run the Rust source-only L0→L3 pass over
//! the fixture, compose the FULL snapshot (deterministic, version
//! `cli-b-v1`), serialize it four ways (raw JSON / envelope JSON / CBOR / cbor.gz),
//! and byte/hex-compare each to the vendored (Rust-owned) golden under
//! `tests/cli-b-goldens/snapshot/` — originally sourced from al-sem's
//! `scripts/cli-b-goldens/snapshot/`, now retired. Plus the sharded JSON output
//! for the findings-rich fixture `ws-d8-commit-in-tx`.
//!
//! ## Acceptance gate
//!
//! All 20 × 4 = 80 goldens + the shard files MUST byte-match. This is ungated: a
//! divergence is either a Rust bug to fix or a genuine model difference to BLOCK —
//! never something to tolerate.
//!
//! ## Refresh
//!
//! Goldens are Rust-owned baselines (the al-sem TS oracle is retired).
//! Rebaseline with `REGEN_TEMP_GOLDENS=1 cargo test --test cli cli_b_snapshot_differential::`.
//!
//! ## Tracked want — localeCompare coverage
//!
//! The cli-b corpus has NO `.alpackages` directory, so the `inputs` / workspace-
//! fingerprint `localeCompare` sort (and the `#`-vs-`:` stableId collation) is
//! corpus-invisible: every fixture's input list is a single `app-json` (or +
//! `roots-config`), and the stableIds happen to ordinal-tie with ICU. The
//! `locale_compare` comparator is oracle-pinned directly in `engine::ids::tests`
//! (mixed-case + `#`/`:` ICU order). A future fixture with a `.alpackages/*.app`
//! whose filename has mixed case (e.g. `Base.app`) would make the ICU sort
//! corpus-VISIBLE in both the `inputs` order AND the `workspaceFingerprint` hash —
//! WANT: add such a fixture to lock the comparator end-to-end through the goldens.

use std::path::{Path, PathBuf};

use al_sem::engine::gate::model_instance_id::compute_gate_model_instance_id;
use al_sem::engine::gate::run::compute_analyzer_diagnostics;
use al_sem::engine::l3::l3_workspace::L3Resolved;
use al_sem::engine::l3::program_calls::assemble_and_resolve_workspace_program;
use al_sem::engine::l5::detectors::registered_detectors;
use al_sem::engine::l5::snapshot_full::{
    EnvelopeDiagnostic, FullSnapshotOptions, compose_full_snapshot, serialize_cbor,
    serialize_cbor_gz, serialize_envelope, serialize_json, serialize_sharded,
};

use crate::regen;

/// al-sem `DEFAULT_DETECTORS` (34) by name — `analyzeWorkspace` runs this set, so
/// the envelope's detector diagnostics (e.g. d43's substrate guard) must come from
/// exactly these (NOT the opt-in detectors, which `analyzeWorkspace` omits).
const DEFAULT_DETECTOR_NAMES: &[&str] = &[
    "d1-db-op-in-loop",
    "d2-event-fanout-in-loop",
    "d3-missing-setloadfields",
    "d4-repeated-lookup-in-loop",
    "d5-set-based-opportunity",
    "d7-recursive-event-expansion",
    "d8-commit-in-transaction",
    "d9-transaction-span-summary",
    "d10-self-modifying-loop",
    "d11-modify-without-get",
    "d12-dead-integration-event",
    "d13-cross-app-internal-call",
    "d14-dead-routine",
    "d16-blob-in-loop",
    "d17-non-setbased-on-large-table",
    "d18-event-subscriber-heavy",
    "d19-flowfield-in-loop",
    "d20-unbounded-result-set",
    "d21-temp-table-misuse",
    "d22-deprecated-api-use",
    "d29-onaftergetrecord-heavy",
    "d32-internal-event-publisher",
    "d33-event-without-subscribers",
    "d34-page-source-heavy",
    "d35-implicit-transaction-scope",
    "d36-redundant-calcfields",
    "d37-record-passed-by-value",
    "d38-page-trigger-heavy",
    "d39-codeunit-instantiation-in-loop",
    "d41-unindexed-filter",
    "d42-locktable-late",
    "d43-event-ishandled-skip",
    "d44-event-recursive-publish",
    "d45-text-encoding-mismatch",
];

/// Compute the analyzer diagnostics for the envelope, projected to the contract
/// shape (`code = "DIAG-<stage>"`). versionDiagnostic() is None under the version
/// override, so it is not prepended.
fn envelope_diagnostics(
    ws_path: &std::path::Path,
    resolved: &L3Resolved,
) -> Vec<EnvelopeDiagnostic> {
    let default_detectors: Vec<_> = registered_detectors()
        .into_iter()
        .filter(|d| DEFAULT_DETECTOR_NAMES.contains(&d.name.as_str()))
        .collect();
    compute_analyzer_diagnostics(ws_path, resolved, &default_detectors)
        .into_iter()
        .map(|d| EnvelopeDiagnostic {
            code: format!("DIAG-{}", d.stage),
            severity: d.severity,
            message: d.message,
        })
        .collect()
}

const VERSION_OVERRIDE: &str = "cli-b-v1";

/// The cli-b snapshot corpus (al-sem `dump-snapshot.ts` `SNAPSHOT_CORPUS`).
const SNAPSHOT_CORPUS: &[&str] = &[
    "ws-d8-commit-in-tx",
    "ws-d34",
    "ws-d35",
    "ws-txn-d46-pos",
    "ws-txn-d47-pos-http-nocommit",
    "ws-txn-d47-pos-http-commit-after",
    "ws-txn-d47-pos-file",
    "ws-txn-d48-pos",
    "ws-txn-d49-pos-modify-message",
    "ws-txn-d49-pos-modify-runmodal",
    "ws-d51-pos",
    "ws-d51-jobqueue",
    "ws-txn-d46-neg",
    "ws-txn-d47-neg-readonly",
    "ws-txn-d47-neg-temp",
    "ws-txn-d48-neg",
    "ws-txn-d49-neg-no-write",
    "ws-d51-neg",
    "ws-d1-multi-caller",
    "ws-d14-dead-routine",
    // ⟨task-4-review.md finding M-2⟩ Two sibling member-trigger bodies of one
    // object — proves the stable-id member discriminator's `contractFacts` split
    // (distinct `stableId` per OnAction body) end to end through a byte-compared
    // golden. See the fixture's own header comment for the full rationale.
    "ws-sibling-member-triggers",
    // #13 / #20: a temporary record needs no TableData permission -- but a
    // physical access to the same table (Mixed) and a parameter-dependent one
    // (ByParam) still do. See the fixture's comments.
    "ws-perm-temp",
];

const SHARD_FIXTURE: &str = "ws-d8-commit-in-tx";

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// In-repo fixtures (Rust-owned; al-sem byte-parity retired — see CLAUDE.md).
fn fixtures_dir() -> PathBuf {
    repo_root().join("tests").join("r0-corpus")
}

/// In-repo Rust-owned goldens, regenerated via `REGEN_TEMP_GOLDENS=1`.
fn goldens_dir() -> PathBuf {
    repo_root()
        .join("tests")
        .join("cli-b-goldens")
        .join("snapshot")
}

/// Byte-compare a golden, or rewrite it when `REGEN_TEMP_GOLDENS` is set
/// (Rust-owned baseline). Creates parent dirs on regen.
fn check_or_regen(golden_path: &Path, got: &[u8], label: &str) {
    if regen::regen_mode() {
        if let Some(parent) = golden_path.parent() {
            std::fs::create_dir_all(parent)
                .unwrap_or_else(|e| panic!("regen mkdir {}: {e}", parent.display()));
        }
        std::fs::write(golden_path, got)
            .unwrap_or_else(|e| panic!("regen write {}: {e}", golden_path.display()));
        return;
    }
    let want = std::fs::read(golden_path)
        .unwrap_or_else(|e| panic!("read {}: {e}", golden_path.display()));
    assert_eq!(
        got,
        want.as_slice(),
        "cli-b GATE: {label} did NOT byte-match ({})",
        golden_path.display()
    );
}

/// Compose the full snapshot tree for one fixture. Returns the tree, the resolved
/// model (for envelope diagnostics), and the workspace dir.
fn compose_for(fixture: &str) -> (al_sem::engine::gate::cbor::CborValue, L3Resolved, PathBuf) {
    let fixture_dir = fixtures_dir().join(fixture);
    assert!(
        fixture_dir.is_dir(),
        "cli-b fixture {fixture} not found at {}",
        fixture_dir.display()
    );
    // al-sem's `analyzeWorkspace` computes a content-derived `modelInstanceId`
    // (NOT the `r0` default the R4-F differential used) — the internal callsite /
    // routine ids (and the edgeIds hashed over them) embed it, so the snapshot
    // must be assembled with the SAME id the gate path computes.
    let model_instance_id = compute_gate_model_instance_id(&fixture_dir)
        .unwrap_or_else(|| panic!("{fixture}: could not compute modelInstanceId"));
    // Engine-switch S6.4: the model `alsem diff`'s workspace mode builds.
    let resolved = assemble_and_resolve_workspace_program(&fixture_dir, &model_instance_id, false)
        .unwrap_or_else(|| panic!("{fixture}: workspace did not resolve"));
    let opts = FullSnapshotOptions {
        workspace_dir: &fixture_dir,
        driver_version: VERSION_OVERRIDE,
        deterministic: true,
        roots_config_ignored: false,
    };
    let tree = compose_full_snapshot(&resolved, &opts);
    (tree, resolved, fixture_dir)
}

/// Just the tree (for raw/cbor tests that don't need diagnostics).
fn tree_for(fixture: &str) -> al_sem::engine::gate::cbor::CborValue {
    compose_for(fixture).0
}

fn hex_prefix(bytes: &[u8], n: usize) -> String {
    bytes
        .iter()
        .take(n)
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Find the first differing byte offset (for diagnostics).
fn first_diff(a: &[u8], b: &[u8]) -> Option<usize> {
    let n = a.len().min(b.len());
    for i in 0..n {
        if a[i] != b[i] {
            return Some(i);
        }
    }
    if a.len() != b.len() { Some(n) } else { None }
}

#[test]
fn raw_json_matches_goldens() {
    for fixture in SNAPSHOT_CORPUS {
        let tree = tree_for(fixture);
        let got = serialize_json(&tree);
        let golden_path = goldens_dir().join(format!("{fixture}.raw.json"));
        check_or_regen(&golden_path, got.as_bytes(), &format!("{fixture}.raw.json"));
    }
}

#[test]
fn envelope_json_matches_goldens() {
    for fixture in SNAPSHOT_CORPUS {
        let (tree, resolved, ws_dir) = compose_for(fixture);
        // versionDiagnostic() is None under the version override; the diagnostics
        // channel is the analyzer diagnostics (workspace + roots-overlay + detect).
        let diags = envelope_diagnostics(&ws_dir, &resolved);
        let got = serialize_envelope(&tree, VERSION_OVERRIDE, true, &diags);
        let golden_path = goldens_dir().join(format!("{fixture}.envelope.json"));
        check_or_regen(
            &golden_path,
            got.as_bytes(),
            &format!("{fixture}.envelope.json"),
        );
    }
}

#[test]
fn cbor_matches_goldens() {
    for fixture in SNAPSHOT_CORPUS {
        let tree = tree_for(fixture);
        let got = serialize_cbor(&tree);
        let golden_path = goldens_dir().join(format!("{fixture}.cbor"));
        if regen::regen_mode() {
            if let Some(p) = golden_path.parent() {
                std::fs::create_dir_all(p).ok();
            }
            std::fs::write(&golden_path, &got)
                .unwrap_or_else(|e| panic!("regen write {}: {e}", golden_path.display()));
            assert_eq!(
                got[0], 0xb9,
                "{fixture}.cbor must start with the map-16 header"
            );
            continue;
        }
        let want = std::fs::read(&golden_path)
            .unwrap_or_else(|e| panic!("read {}: {e}", golden_path.display()));
        if got != want {
            let off = first_diff(&got, &want);
            panic!(
                "cli-b GATE: {fixture}.cbor did NOT byte-match. got_len={}, want_len={}, \
                 first_diff_offset={:?}\n got[0..16]={}\nwant[0..16]={}",
                got.len(),
                want.len(),
                off,
                hex_prefix(&got, 16),
                hex_prefix(&want, 16),
            );
        }
        // The header is always the cbor-x always-map-16 header (0xb9).
        assert_eq!(
            got[0], 0xb9,
            "{fixture}.cbor must start with the map-16 header"
        );
    }
}

#[test]
fn cbor_gz_matches_goldens() {
    for fixture in SNAPSHOT_CORPUS {
        let tree = tree_for(fixture);
        let got = serialize_cbor_gz(&tree);
        let golden_path = goldens_dir().join(format!("{fixture}.cbor.gz"));
        if regen::regen_mode() {
            if let Some(p) = golden_path.parent() {
                std::fs::create_dir_all(p).ok();
            }
            std::fs::write(&golden_path, &got)
                .unwrap_or_else(|e| panic!("regen write {}: {e}", golden_path.display()));
            continue;
        }
        let want = std::fs::read(&golden_path)
            .unwrap_or_else(|e| panic!("read {}: {e}", golden_path.display()));
        if got != want {
            let off = first_diff(&got, &want);
            panic!(
                "cli-b GATE: {fixture}.cbor.gz did NOT byte-match. got_len={}, want_len={}, \
                 first_diff_offset={:?}\n got[0..16]={}\nwant[0..16]={}",
                got.len(),
                want.len(),
                off,
                hex_prefix(&got, 16),
                hex_prefix(&want, 16),
            );
        }
        // gzip header: 1f 8b 08 00 00000000 00 03.
        assert_eq!(
            hex_prefix(&got, 10),
            "1f 8b 08 00 00 00 00 00 00 03",
            "{fixture}.cbor.gz header must be normalized"
        );
    }
}

#[test]
fn shards_match_goldens() {
    let (tree, _resolved, _ws_dir) = compose_for(SHARD_FIXTURE);
    let shards_base = goldens_dir().join("shards").join(SHARD_FIXTURE);

    for (variant, primary_only) in [("all", false), ("primary", true)] {
        let files = serialize_sharded(&tree, VERSION_OVERRIDE, primary_only);
        let dir = shards_base.join(variant);
        if regen::regen_mode() {
            // Fresh write of the exact shard set (clear stale files first).
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir)
                .unwrap_or_else(|e| panic!("regen mkdir {}: {e}", dir.display()));
            for f in &files {
                std::fs::write(dir.join(&f.name), &f.bytes)
                    .unwrap_or_else(|e| panic!("regen write shard {}: {e}", f.name));
            }
            continue;
        }
        for f in &files {
            let golden_path = dir.join(&f.name);
            let want = std::fs::read(&golden_path)
                .unwrap_or_else(|e| panic!("read {}: {e}", golden_path.display()));
            assert_eq!(
                f.bytes,
                want,
                "cli-b GATE: shard {variant}/{} did NOT byte-match ({})",
                f.name,
                golden_path.display()
            );
        }
        // Confirm we produced exactly the golden file set.
        let mut got_names: Vec<String> = files.iter().map(|f| f.name.clone()).collect();
        got_names.sort();
        let mut golden_names: Vec<String> = std::fs::read_dir(&dir)
            .unwrap_or_else(|e| panic!("read shard dir {}: {e}", dir.display()))
            .flatten()
            .filter_map(|e| e.file_name().into_string().ok())
            .collect();
        golden_names.sort();
        assert_eq!(
            got_names, golden_names,
            "cli-b GATE: shard {variant} file set mismatch"
        );
    }
}

// Rust-owned goldens are regenerated in-process via `REGEN_TEMP_GOLDENS=1 cargo
// test --test cli cli_b_snapshot_differential::` (al-sem `dump-snapshot.ts` retired).

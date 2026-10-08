//! cli-b/b3 — FINGERPRINT CLI differential.
//!
//! For each fixture in `FINGERPRINT_CORPUS` (20 fixtures), runs the Rust fingerprint
//! pipeline and byte-compares the output against the vendored (Rust-owned) goldens
//! under `tests/cli-b-goldens/fingerprint/` — originally sourced from al-sem's
//! `scripts/cli-b-goldens/fingerprint/`, now retired.
//!
//! ## Goldens checked per fixture
//!
//! - `<fixture>.json`          — fingerprint-query JSON (query flags: includeInherited=true, witnessLimit=3)
//! - `<fixture>.human.txt`     — compact human text (same query flags)
//! - `<fixture>.cbor`          — CBOR snapshot (B0; no query flags)
//! - `<fixture>.cbor.gz`       — CBOR.gz snapshot (B0; no query flags)
//!
//! ## Extra goldens for `ws-d8-commit-in-tx`
//!
//! - `.witness-all.json`       — witnessLimit="all"
//! - `.witness-0.json`         — witnessLimit=0
//! - `.witness-false.json`     — witnessLimit=false (disabled)
//! - `.selector-error.json`    — selector error path (exit 2); valid doc in payload.diagnostics
//! - `.human-full.txt`         — full verbosity human text
//! - `shards/manifest.json`    — shard manifest (B0)
//! - `shards/primary.json`     — primary shard (B0)
//!
//! ## Acceptance gate
//!
//! All goldens MUST byte-match. Divergences are bugs to fix, not something to tolerate.
//!
//! ## Refresh
//!
//! Goldens are Rust-owned baselines (the al-sem TS oracle is retired).
//! Rebaseline with `REGEN_TEMP_GOLDENS=1 cargo test --test cli cli_b_fingerprint_differential::`.

use std::path::PathBuf;

use al_sem::engine::gate::model_instance_id::compute_gate_model_instance_id;
use al_sem::engine::l5::fingerprint_cli::{
    FingerprintFormat, FingerprintOptions, FingerprintOutput, ShardMode, run_fingerprint_pipeline,
};
use al_sem::engine::l5::fingerprint_query::WitnessLimit;
use al_sem::engine::l5::snapshot_full::{
    FullSnapshotOptions, compose_full_snapshot, serialize_cbor, serialize_cbor_gz,
    serialize_sharded,
};
use al_sem::program::model::program_calls::assemble_and_resolve_workspace_program;

use crate::regen;

const VERSION_OVERRIDE: &str = "cli-b-v1";

/// The fingerprint corpus (same 20 fixtures as the snapshot corpus).
const FINGERPRINT_CORPUS: &[&str] = &[
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
    // #13 / #20: temp-only, mixed temp+physical, and parameter-dependent access.
    "ws-perm-temp",
];

const WITNESS_FIXTURE: &str = "ws-d8-commit-in-tx";
const SHARD_FIXTURE: &str = "ws-d8-commit-in-tx";
const ERROR_FIXTURE: &str = "ws-d8-commit-in-tx";
const ERROR_SELECTOR: &str = "THIS_ROUTINE_DOES_NOT_EXIST_FOR_ERROR_TEST";

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
        .join("fingerprint")
}

/// When `REGEN_TEMP_GOLDENS` is set, write the golden (Rust-owned baseline) and
/// return true so the caller skips the byte-compare. al-sem byte-parity retired.
fn regen_golden(golden_path: &std::path::Path, got: &[u8]) -> bool {
    if !regen::regen_mode() {
        return false;
    }
    if let Some(parent) = golden_path.parent() {
        std::fs::create_dir_all(parent)
            .unwrap_or_else(|e| panic!("regen mkdir {}: {e}", parent.display()));
    }
    std::fs::write(golden_path, got)
        .unwrap_or_else(|e| panic!("regen write {}: {e}", golden_path.display()));
    true
}

fn fixture_dir(fixture: &str) -> PathBuf {
    fixtures_dir().join(fixture)
}

fn first_diff(a: &[u8], b: &[u8]) -> Option<usize> {
    let n = a.len().min(b.len());
    for i in 0..n {
        if a[i] != b[i] {
            return Some(i);
        }
    }
    if a.len() != b.len() { Some(n) } else { None }
}

/// Helper: compose full snapshot tree for the B0 path (cbor / cbor.gz / envelope).
fn compose_full_for(
    fixture: &str,
) -> (
    al_sem::engine::gate::cbor::CborValue,
    al_sem::program::model::workspace::Model,
    PathBuf,
) {
    let ws = fixture_dir(fixture);
    assert!(
        ws.is_dir(),
        "fixture {fixture} not found at {}",
        ws.display()
    );
    let model_id = compute_gate_model_instance_id(&ws)
        .unwrap_or_else(|| panic!("{fixture}: could not compute modelInstanceId"));
    let resolved = assemble_and_resolve_workspace_program(&ws, &model_id, false)
        .unwrap_or_else(|| panic!("{fixture}: workspace did not resolve"));
    let opts = FullSnapshotOptions {
        workspace_dir: &ws,
        driver_version: VERSION_OVERRIDE,
        deterministic: true,
        roots_config_ignored: false,
    };
    let tree = compose_full_snapshot(&resolved, &opts);
    (tree, resolved, ws)
}

// ===========================================================================
// B0 tests: CBOR and CBOR.gz (no query flags)
// ===========================================================================

#[test]
fn cbor_matches_goldens() {
    for fixture in FINGERPRINT_CORPUS {
        let (tree, _, _) = compose_full_for(fixture);
        let got = serialize_cbor(&tree);
        let golden_path = goldens_dir().join(format!("{fixture}.cbor"));
        if regen_golden(&golden_path, &got) {
            continue;
        }
        let want = std::fs::read(&golden_path)
            .unwrap_or_else(|e| panic!("read {}: {e}", golden_path.display()));
        if let Some(off) = first_diff(&got, &want) {
            panic!(
                "cli-b/b3 CBOR: {fixture}.cbor diverges at byte {off} (got={} want={})",
                got.len(),
                want.len()
            );
        }
    }
}

#[test]
fn cbor_gz_matches_goldens() {
    for fixture in FINGERPRINT_CORPUS {
        let (tree, _, _) = compose_full_for(fixture);
        let got = serialize_cbor_gz(&tree);
        let golden_path = goldens_dir().join(format!("{fixture}.cbor.gz"));
        if regen_golden(&golden_path, &got) {
            continue;
        }
        let want = std::fs::read(&golden_path)
            .unwrap_or_else(|e| panic!("read {}: {e}", golden_path.display()));
        if let Some(off) = first_diff(&got, &want) {
            panic!(
                "cli-b/b3 CBOR.GZ: {fixture}.cbor.gz diverges at byte {off} (got={} want={})",
                got.len(),
                want.len()
            );
        }
    }
}

// ===========================================================================
// Query JSON tests (with query flags: includeInherited=true, witnessLimit=3)
// ===========================================================================

#[test]
fn query_json_matches_goldens() {
    for fixture in FINGERPRINT_CORPUS {
        let ws = fixture_dir(fixture);
        let opts = FingerprintOptions {
            workspace: &ws,
            driver_version: VERSION_OVERRIDE,
            format: FingerprintFormat::Json,
            out: None,
            shard: None,
            witness_limit: Some(WitnessLimit::Capped(3)),
            roots: None,
            routine_selectors: Vec::new(),
            include_inherited: true,
            is_query_requested: true,
            deterministic: true,
            strict: false,
            verbosity: "compact",
            inventory_only: false,
            no_roots_config: false,
        };
        let result = run_fingerprint_pipeline(&opts)
            .unwrap_or_else(|e| panic!("{fixture}: fingerprint json pipeline error: {e}"));
        let got = match result.output {
            FingerprintOutput::Text(t) => t,
            _ => panic!("{fixture}: expected Text from json query"),
        };
        let golden_path = goldens_dir().join(format!("{fixture}.json"));
        if regen_golden(&golden_path, got.as_bytes()) {
            continue;
        }
        let want = std::fs::read_to_string(&golden_path)
            .unwrap_or_else(|e| panic!("read {}: {e}", golden_path.display()));
        assert_eq!(
            got, want,
            "cli-b/b3 QUERY JSON: {fixture}.json did NOT byte-match"
        );
    }
}

// ===========================================================================
// Human compact text tests
// ===========================================================================

#[test]
fn human_compact_matches_goldens() {
    for fixture in FINGERPRINT_CORPUS {
        let ws = fixture_dir(fixture);
        let opts = FingerprintOptions {
            workspace: &ws,
            driver_version: VERSION_OVERRIDE,
            format: FingerprintFormat::Human,
            out: None,
            shard: None,
            witness_limit: Some(WitnessLimit::Capped(3)),
            roots: None,
            routine_selectors: Vec::new(),
            include_inherited: true,
            is_query_requested: true,
            deterministic: true,
            strict: false,
            verbosity: "compact",
            inventory_only: false,
            no_roots_config: false,
        };
        let result = run_fingerprint_pipeline(&opts)
            .unwrap_or_else(|e| panic!("{fixture}: fingerprint human pipeline error: {e}"));
        let got = match result.output {
            FingerprintOutput::Text(t) => t,
            _ => panic!("{fixture}: expected Text from human query"),
        };
        let golden_path = goldens_dir().join(format!("{fixture}.human.txt"));
        if regen_golden(&golden_path, got.as_bytes()) {
            continue;
        }
        let want = std::fs::read_to_string(&golden_path)
            .unwrap_or_else(|e| panic!("read {}: {e}", golden_path.display()));
        assert_eq!(
            got, want,
            "cli-b/b3 HUMAN COMPACT: {fixture}.human.txt did NOT byte-match"
        );
    }
}

// ===========================================================================
// Witness mode variants for ws-d8-commit-in-tx
// ===========================================================================

#[test]
fn witness_all_matches_golden() {
    let ws = fixture_dir(WITNESS_FIXTURE);
    let opts = FingerprintOptions {
        workspace: &ws,
        driver_version: VERSION_OVERRIDE,
        format: FingerprintFormat::Json,
        out: None,
        shard: None,
        witness_limit: Some(WitnessLimit::All),
        roots: None,
        routine_selectors: Vec::new(),
        include_inherited: true,
        is_query_requested: true,
        deterministic: true,
        strict: false,
        verbosity: "compact",
        inventory_only: false,
        no_roots_config: false,
    };
    let result = run_fingerprint_pipeline(&opts)
        .unwrap_or_else(|e| panic!("{WITNESS_FIXTURE}: witness-all pipeline error: {e}"));
    let got = match result.output {
        FingerprintOutput::Text(t) => t,
        _ => panic!("expected Text"),
    };
    let golden_path = goldens_dir().join(format!("{WITNESS_FIXTURE}.witness-all.json"));
    if regen_golden(&golden_path, got.as_bytes()) {
        return;
    }
    let want = std::fs::read_to_string(&golden_path)
        .unwrap_or_else(|e| panic!("read {}: {e}", golden_path.display()));
    assert_eq!(
        got, want,
        "cli-b/b3: {WITNESS_FIXTURE}.witness-all.json did NOT byte-match"
    );
}

#[test]
fn witness_zero_matches_golden() {
    let ws = fixture_dir(WITNESS_FIXTURE);
    let opts = FingerprintOptions {
        workspace: &ws,
        driver_version: VERSION_OVERRIDE,
        format: FingerprintFormat::Json,
        out: None,
        shard: None,
        witness_limit: Some(WitnessLimit::Capped(0)),
        roots: None,
        routine_selectors: Vec::new(),
        include_inherited: true,
        is_query_requested: true,
        deterministic: true,
        strict: false,
        verbosity: "compact",
        inventory_only: false,
        no_roots_config: false,
    };
    let result = run_fingerprint_pipeline(&opts)
        .unwrap_or_else(|e| panic!("{WITNESS_FIXTURE}: witness-0 pipeline error: {e}"));
    let got = match result.output {
        FingerprintOutput::Text(t) => t,
        _ => panic!("expected Text"),
    };
    let golden_path = goldens_dir().join(format!("{WITNESS_FIXTURE}.witness-0.json"));
    if regen_golden(&golden_path, got.as_bytes()) {
        return;
    }
    let want = std::fs::read_to_string(&golden_path)
        .unwrap_or_else(|e| panic!("read {}: {e}", golden_path.display()));
    assert_eq!(
        got, want,
        "cli-b/b3: {WITNESS_FIXTURE}.witness-0.json did NOT byte-match"
    );
}

#[test]
fn witness_false_matches_golden() {
    let ws = fixture_dir(WITNESS_FIXTURE);
    let opts = FingerprintOptions {
        workspace: &ws,
        driver_version: VERSION_OVERRIDE,
        format: FingerprintFormat::Json,
        out: None,
        shard: None,
        witness_limit: Some(WitnessLimit::Disabled),
        roots: None,
        routine_selectors: Vec::new(),
        include_inherited: true,
        is_query_requested: true,
        deterministic: true,
        strict: false,
        verbosity: "compact",
        inventory_only: false,
        no_roots_config: false,
    };
    let result = run_fingerprint_pipeline(&opts)
        .unwrap_or_else(|e| panic!("{WITNESS_FIXTURE}: witness-false pipeline error: {e}"));
    let got = match result.output {
        FingerprintOutput::Text(t) => t,
        _ => panic!("expected Text"),
    };
    let golden_path = goldens_dir().join(format!("{WITNESS_FIXTURE}.witness-false.json"));
    if regen_golden(&golden_path, got.as_bytes()) {
        return;
    }
    let want = std::fs::read_to_string(&golden_path)
        .unwrap_or_else(|e| panic!("read {}: {e}", golden_path.display()));
    assert_eq!(
        got, want,
        "cli-b/b3: {WITNESS_FIXTURE}.witness-false.json did NOT byte-match"
    );
}

// ===========================================================================
// Selector error path (exit code 2 + valid doc in payload.diagnostics)
// ===========================================================================

#[test]
fn selector_error_json_matches_golden_and_exits_2() {
    let ws = fixture_dir(ERROR_FIXTURE);
    let opts = FingerprintOptions {
        workspace: &ws,
        driver_version: VERSION_OVERRIDE,
        format: FingerprintFormat::Json,
        out: None,
        shard: None,
        witness_limit: Some(WitnessLimit::Capped(3)),
        roots: None,
        routine_selectors: vec![ERROR_SELECTOR.to_string()],
        include_inherited: true,
        is_query_requested: true,
        deterministic: true,
        strict: false,
        verbosity: "compact",
        inventory_only: false,
        no_roots_config: false,
    };
    let result = run_fingerprint_pipeline(&opts)
        .unwrap_or_else(|e| panic!("{ERROR_FIXTURE}: selector-error pipeline error: {e}"));
    assert_eq!(result.exit_code, 2, "selector error must exit 2");
    let got = match result.output {
        FingerprintOutput::Text(t) => t,
        _ => panic!("expected Text for selector error json"),
    };
    let golden_path = goldens_dir().join(format!("{ERROR_FIXTURE}.selector-error.json"));
    if regen_golden(&golden_path, got.as_bytes()) {
        return;
    }
    let want = std::fs::read_to_string(&golden_path)
        .unwrap_or_else(|e| panic!("read {}: {e}", golden_path.display()));
    assert_eq!(
        got, want,
        "cli-b/b3: {ERROR_FIXTURE}.selector-error.json did NOT byte-match"
    );
}

// ===========================================================================
// Human full verbosity
// ===========================================================================

#[test]
fn human_full_matches_golden() {
    let ws = fixture_dir(WITNESS_FIXTURE);
    let opts = FingerprintOptions {
        workspace: &ws,
        driver_version: VERSION_OVERRIDE,
        format: FingerprintFormat::Human,
        out: None,
        shard: None,
        witness_limit: Some(WitnessLimit::Capped(3)),
        roots: None,
        routine_selectors: Vec::new(),
        include_inherited: true,
        is_query_requested: true,
        deterministic: true,
        strict: false,
        verbosity: "full",
        inventory_only: false,
        no_roots_config: false,
    };
    let result = run_fingerprint_pipeline(&opts)
        .unwrap_or_else(|e| panic!("{WITNESS_FIXTURE}: human-full pipeline error: {e}"));
    let got = match result.output {
        FingerprintOutput::Text(t) => t,
        _ => panic!("expected Text from human-full query"),
    };
    let golden_path = goldens_dir().join(format!("{WITNESS_FIXTURE}.human-full.txt"));
    if regen_golden(&golden_path, got.as_bytes()) {
        return;
    }
    let want = std::fs::read_to_string(&golden_path)
        .unwrap_or_else(|e| panic!("read {}: {e}", golden_path.display()));
    assert_eq!(
        got, want,
        "cli-b/b3: {WITNESS_FIXTURE}.human-full.txt did NOT byte-match"
    );
}

// ===========================================================================
// Shard output
// ===========================================================================

#[test]
fn shard_output_matches_goldens() {
    let ws = fixture_dir(SHARD_FIXTURE);
    let (tree, resolved, ws_path) = compose_full_for(SHARD_FIXTURE);

    let shards = serialize_sharded(&tree, VERSION_OVERRIDE, false);
    // Goldens live under shards/<fixture-name>/<shard-name>.
    let shard_dir = goldens_dir().join("shards").join(SHARD_FIXTURE);

    if regen::regen_mode() {
        let _ = std::fs::remove_dir_all(&shard_dir);
        std::fs::create_dir_all(&shard_dir)
            .unwrap_or_else(|e| panic!("regen mkdir {}: {e}", shard_dir.display()));
        for shard in &shards {
            std::fs::write(shard_dir.join(&shard.name), &shard.bytes)
                .unwrap_or_else(|e| panic!("regen write shard {}: {e}", shard.name));
        }
        drop((resolved, ws_path));
        drop(ws);
        return;
    }

    for shard in &shards {
        let golden_path = shard_dir.join(&shard.name);
        let want = std::fs::read(&golden_path)
            .unwrap_or_else(|e| panic!("read shard {}: {e}", golden_path.display()));
        if let Some(off) = first_diff(&shard.bytes, &want) {
            panic!(
                "cli-b/b3 SHARD: shards/{} diverges at byte {off} (got={} want={})",
                shard.name,
                shard.bytes.len(),
                want.len()
            );
        }
    }
    drop((resolved, ws_path)); // keep compiler happy
    drop(ws);
}

// ===========================================================================
// Items 5 / 15 / 17 — pipeline-level oracles (strict gate, per-mode stderr,
// shard --out duality). Driven on a real fixture (ws-d8) whose only analyzer
// diagnostic is severity "warning" (DIAG-detect), so the strict gate PASSES.
// ===========================================================================

fn opts_for<'a>(
    ws: &'a std::path::Path,
    format: FingerprintFormat,
    shard: Option<ShardMode>,
    strict: bool,
    is_query: bool,
) -> FingerprintOptions<'a> {
    FingerprintOptions {
        workspace: ws,
        driver_version: VERSION_OVERRIDE,
        format,
        out: None,
        shard,
        witness_limit: None,
        roots: None,
        routine_selectors: Vec::new(),
        include_inherited: true,
        is_query_requested: is_query,
        deterministic: true,
        strict,
        verbosity: "compact",
        inventory_only: false,
        no_roots_config: false,
    }
}

#[test]
fn item15_human_mode_routes_warning_to_stderr() {
    // Human mode prints analyze diagnostics to stderr at exit (fingerprint.ts:331).
    let ws = fixture_dir("ws-d8-commit-in-tx");
    let opts = opts_for(&ws, FingerprintFormat::Human, None, false, false);
    let r = run_fingerprint_pipeline(&opts).expect("pipeline");
    assert_eq!(r.exit_code, 0);
    // ws-d8 emits the d43 warning → must appear in stderr_diagnostics as `warning: ...`.
    assert!(
        r.stderr_diagnostics
            .iter()
            .any(|l| l.starts_with("warning: ")),
        "human mode must route analyzer warnings to stderr: {:?}",
        r.stderr_diagnostics
    );
}

#[test]
fn item15_json_query_mode_embeds_diags_no_stderr() {
    // JSON-query mode embeds analyze diagnostics in payload.diagnostics + NO stderr
    // (fingerprint.ts:281-297).
    let ws = fixture_dir("ws-d8-commit-in-tx");
    let opts = opts_for(&ws, FingerprintFormat::Json, None, false, true);
    let r = run_fingerprint_pipeline(&opts).expect("pipeline");
    assert_eq!(r.exit_code, 0);
    assert!(
        r.stderr_diagnostics.is_empty(),
        "json-query mode must NOT print analyze diagnostics to stderr"
    );
    // The diagnostics live in the JSON payload instead.
    if let FingerprintOutput::Text(t) = &r.output {
        assert!(t.contains("\"diagnostics\""));
    } else {
        panic!("expected Text output");
    }
}

#[test]
fn item15_shard_mode_no_stderr_diags() {
    // Shard path returns BEFORE the stderr loop (fingerprint.ts:223) → no stderr.
    let ws = fixture_dir("ws-d8-commit-in-tx");
    let mut opts = opts_for(
        &ws,
        FingerprintFormat::Json,
        Some(ShardMode::All),
        false,
        false,
    );
    opts.out = Some("."); // shard requires --out; value is a directory.
    let r = run_fingerprint_pipeline(&opts).expect("pipeline");
    assert_eq!(r.exit_code, 0);
    assert!(r.stderr_diagnostics.is_empty());
    assert!(matches!(r.output, FingerprintOutput::Shards(_)));
}

#[test]
fn item17_shard_without_out_errors_exit1() {
    // --shard requires --out <directory> (fingerprint.ts:210) → exit 1, message to stderr.
    let ws = fixture_dir("ws-d8-commit-in-tx");
    let opts = opts_for(
        &ws,
        FingerprintFormat::Json,
        Some(ShardMode::All),
        false,
        false,
    );
    let r = run_fingerprint_pipeline(&opts).expect("pipeline");
    assert_eq!(r.exit_code, 1);
    assert_eq!(
        r.selector_error_message.as_deref(),
        Some("--shard requires --out <directory>")
    );
}

#[test]
fn item5_strict_passes_on_warning_only_fixture() {
    // ws-d8 has only a warning → strict gate does NOT trip; normal output proceeds.
    let ws = fixture_dir("ws-d8-commit-in-tx");
    let opts = opts_for(&ws, FingerprintFormat::Json, None, true, false);
    let r = run_fingerprint_pipeline(&opts).expect("pipeline");
    assert_eq!(
        r.exit_code, 0,
        "strict must pass when no error-severity diagnostics"
    );
    assert!(matches!(r.output, FingerprintOutput::Text(_)));
}

// Rust-owned goldens regenerated in-process via
// `REGEN_TEMP_GOLDENS=1 cargo test --test cli cli_b_fingerprint_differential::`.

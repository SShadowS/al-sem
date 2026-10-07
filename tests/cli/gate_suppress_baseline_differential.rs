//! Stage-3b GATE inline-suppression + baseline differential — the Rust engine's
//! production `analyze` gate path (`engine::gate::run`) byte-matches the al-sem TS CLI's
//! Stage-3 goldens under `tests/gate-goldens/`.
//!
//! OFFLINE: goldens are committed; the corpus fixtures live under `tests/r0-corpus/`.
//! No subprocess — `run_analyze` / `run_analyze_with_exit` are called in-process.
//!
//! The al-sem capture (`scripts/dump-gate-suppress-baseline.ts`) pipeline is:
//!   analyzeWorkspace({deterministic:true, detectors}) → project → filter({}) →
//!   scope=primary → applyBaseline → applyInlineSuppressions → formatSarif/formatPrSummary
//! pinned to driver.version "gate-sarif-v1".
//!
//! Stage 3A — inline suppression (ws-inline-suppress, DEFAULT + d47-io-unsafe-txn):
//!   - WITH suppression → .suppressed.sarif.json (6 results) + .suppressed.prsummary.md.
//!   - WITHOUT suppression → .unsuppressed.sarif.json (7 results; +1 d47 from the pragma'd line).
//!
//! Stage 3B — baseline (ws-d8-commit-in-tx, transaction-integrity preset):
//!   - --baseline full → .baselined.sarif.json (0 results).
//!   - --baseline partial → .partial-baselined.sarif.json (1 result).
//!   - --update-baseline → round-trips the committed .baseline.json byte-for-byte.
//!   - the exit-code matrix (no-baseline / full / partial × fail-on) matches
//!     suppress-baseline-exit.json.

use std::path::PathBuf;

use al_sem::engine::gate::baseline::serialize_baseline;
use al_sem::engine::gate::filter::Scope;
use al_sem::engine::gate::presets::DEFAULT_DETECTOR_NAMES;
use al_sem::engine::gate::run::{AnalyzeArgs, OutputFormat, run_analyze, run_analyze_with_exit};

use crate::regen;

const PIN_VERSION: &str = "gate-sarif-v1";

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn corpus_dir() -> PathBuf {
    repo_root().join("tests").join("r0-corpus")
}

fn goldens_dir() -> PathBuf {
    repo_root().join("tests").join("gate-goldens")
}

fn read_golden(name: &str) -> String {
    let path = goldens_dir().join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read golden {}: {e}", path.display()))
}

/// REGEN path (mirrors the gate-sarif / cli-a / r4 harnesses). When
/// `REGEN_TEMP_GOLDENS` is set, write the ENGINE-produced string to the golden
/// file instead of comparing — the goldens are Rust-owned baselines. Returns
/// `true` when a regen write happened (the caller then skips the assert).
fn maybe_regen(name: &str, rust: &str) -> bool {
    if !regen::regen_mode() {
        return false;
    }
    let path = goldens_dir().join(name);
    std::fs::write(&path, rust).unwrap_or_else(|e| panic!("regen write {}: {e}", path.display()));
    eprintln!("REGEN gate-suppress golden: {}", path.display());
    true
}

/// `gate-goldens/suppress-baseline-manifest.json`'s `stageA.goldens` (3 files)
/// and `stageB.goldens` (4 files) lists were read by no test (Task T0.6 — a
/// silently deleted golden would pass unnoticed). Asserts each named file
/// still exists.
#[test]
fn manifest_named_goldens_exist() {
    let manifest_path = goldens_dir().join("suppress-baseline-manifest.json");
    let manifest: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(&manifest_path)
            .unwrap_or_else(|e| panic!("read {}: {e}", manifest_path.display())),
    )
    .unwrap_or_else(|e| panic!("{} not valid JSON: {e}", manifest_path.display()));
    let mut checked = 0usize;
    for stage in ["stageA", "stageB"] {
        let named = manifest
            .get(stage)
            .and_then(|s| s.get("goldens"))
            .and_then(|v| v.as_array())
            .unwrap_or_else(|| panic!("manifest missing {stage}.goldens array"));
        for entry in named {
            let name = entry.as_str().expect("goldens entry must be a string");
            let path = goldens_dir().join(name);
            assert!(
                path.is_file(),
                "suppress-baseline-manifest.json's {stage} names {name} but {} does not \
                 exist — a golden may have been silently deleted",
                path.display()
            );
            checked += 1;
        }
    }
    assert!(checked > 0, "no named goldens found across stageA/stageB");
}

/// The al-sem inline-suppress capture runs `[...DEFAULT_DETECTORS, d47-io-unsafe-txn]`
/// with `filterFindings(projected, {})` (no allow-list). In the Rust gate, passing the
/// explicit name list as `--detector` SELECTS exactly those detectors AND allow-lists
/// them — but since exactly those ran, the allow-list filter is a no-op, so the output
/// is byte-equivalent (see presets.rs `resolve_analyze_detectors` doc).
fn inline_suppress_detectors() -> String {
    let mut names: Vec<String> = DEFAULT_DETECTOR_NAMES
        .iter()
        .map(|s| s.to_string())
        .collect();
    names.push("d47-io-unsafe-txn".to_string());
    names.join(",")
}

/// Build args for the ws-inline-suppress fixture. `disable_suppression` toggles the
/// inline-suppression layer (the only difference between suppressed/unsuppressed goldens).
fn inline_suppress_args(disable_suppression: bool) -> AnalyzeArgs {
    let ws = corpus_dir().join("ws-inline-suppress");
    assert!(
        ws.is_dir(),
        "ws-inline-suppress fixture missing at {} (offline corpus incomplete)",
        ws.display()
    );
    AnalyzeArgs {
        workspace: ws.to_string_lossy().to_string(),
        min_severity: None,
        detector: Some(inline_suppress_detectors()),
        preset: None,
        scope: Scope::Primary,
        limit: None,
        format: OutputFormat::Sarif,
        sarif_version_override: Some(PIN_VERSION.to_string()),
        fail_on: None,
        require_dependencies: false,
        baseline: None,
        update_baseline: false,
        disable_inline_suppression: disable_suppression,
        group_by: None,
        deterministic: false,
        with_evidence: false,
        single_app: false,
    }
}

fn sarif_result_count(sarif: &str) -> usize {
    let v: serde_json::Value = serde_json::from_str(sarif).expect("SARIF is valid JSON");
    v["runs"][0]["results"]
        .as_array()
        .map(|a| a.len())
        .unwrap_or(0)
}

/// On a mismatch, print the first differing line for fast triage.
fn first_diff(label: &str, golden: &str, rust: &str) {
    let g: Vec<&str> = golden.lines().collect();
    let r: Vec<&str> = rust.lines().collect();
    let n = g.len().max(r.len());
    for i in 0..n {
        let gl = g.get(i).copied().unwrap_or("<absent>");
        let rl = r.get(i).copied().unwrap_or("<absent>");
        if gl != rl {
            eprintln!(
                "mismatch in {label} at line {}:\n  golden: {gl}\n  rust:   {rl}",
                i + 1
            );
            return;
        }
    }
    eprintln!(
        "mismatch in {label}: differ only in trailing bytes (len {} vs {})",
        golden.len(),
        rust.len()
    );
}

// ===========================================================================
// Stage 3A — inline suppression
// ===========================================================================

#[test]
fn inline_suppression_suppressed_sarif_byte_matches() {
    let args = inline_suppress_args(false); // suppression ON
    let rust = run_analyze(&args, "engine-default").expect("run_analyze");
    if maybe_regen("ws-inline-suppress.suppressed.sarif.json", &rust) {
        return;
    }
    let golden = read_golden("ws-inline-suppress.suppressed.sarif.json");
    if rust != golden {
        first_diff("ws-inline-suppress.suppressed.sarif", &golden, &rust);
    }
    assert_eq!(rust, golden, "suppressed SARIF did not byte-match");
    // Anti-degenerate: suppression leaves exactly 4 results (was 6 pre-iter-2;
    // G-15 now suppresses the two write-after-Get d3 findings on Rec/Rec2).
    assert_eq!(
        sarif_result_count(&rust),
        4,
        "expected 4 suppressed results"
    );
}

#[test]
fn inline_suppression_unsuppressed_sarif_byte_matches() {
    let args = inline_suppress_args(true); // suppression OFF
    let rust = run_analyze(&args, "engine-default").expect("run_analyze");
    if maybe_regen("ws-inline-suppress.unsuppressed.sarif.json", &rust) {
        return;
    }
    let golden = read_golden("ws-inline-suppress.unsuppressed.sarif.json");
    if rust != golden {
        first_diff("ws-inline-suppress.unsuppressed.sarif", &golden, &rust);
    }
    assert_eq!(rust, golden, "unsuppressed SARIF did not byte-match");
    // Anti-degenerate: WITHOUT suppression there are 5 results (the +1 d47 on the
    // pragma'd line is present). Was 7 pre-iter-2; G-15 now suppresses the two
    // write-after-Get d3 findings on Rec/Rec2 (the witness d3 on Rec3 survives).
    assert_eq!(
        sarif_result_count(&rust),
        5,
        "expected 5 unsuppressed results"
    );
}

#[test]
fn inline_suppression_prsummary_byte_matches() {
    let mut args = inline_suppress_args(false); // suppression ON
    args.format = OutputFormat::PrSummary;
    args.sarif_version_override = None; // PR-summary embeds no version
    let rust = run_analyze(&args, "engine-default").expect("run_analyze");
    if maybe_regen("ws-inline-suppress.suppressed.prsummary.md", &rust) {
        return;
    }
    let golden = read_golden("ws-inline-suppress.suppressed.prsummary.md");
    if rust != golden {
        first_diff("ws-inline-suppress.suppressed.prsummary", &golden, &rust);
    }
    assert_eq!(rust, golden, "suppressed PR-summary did not byte-match");
}

/// Anti-degenerate: suppression removes EXACTLY the d47 finding the directive+1 pragma
/// covers (5 → 4), and the wrong-detector pragma (d1-db-op-in-loop above a d47 IO call)
/// does NOT suppress its finding. We confirm this via the result counts AND by checking
/// the suppressed SARIF still carries the second (un-suppressed) d47 result.
///
/// Counts dropped from 7→5 / 6→4 after the iter-2 G-15 gap fix: the d3 findings on
/// `SuppressedIo` (Rec) and `WrongDirectiveIo` (Rec2) were write-after-Get
/// (`Name := …; Modify()`) and are now correctly SUPPRESSED. The anti-degenerate
/// WITNESS d3 on `UnsuppressedD3` (Rec3 reads the NORMAL field `Name`) SURVIVES
/// (2 findings: one per FindSet/Get retrieval), and the d47 invariant (2 → 1) is
/// unchanged — both core properties remain intact.
#[test]
fn anti_degenerate_suppression_removes_one_d47_keeps_wrong_detector() {
    let suppressed = run_analyze(&inline_suppress_args(false), "engine-default").unwrap();
    let unsuppressed = run_analyze(&inline_suppress_args(true), "engine-default").unwrap();
    assert_eq!(sarif_result_count(&unsuppressed), 5);
    assert_eq!(sarif_result_count(&suppressed), 4);

    // The wrong-detector pragma must leave its d47 finding intact: count d47 ruleIds.
    let count_d47 = |sarif: &str| -> usize {
        let v: serde_json::Value = serde_json::from_str(sarif).unwrap();
        v["runs"][0]["results"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|r| r["ruleId"].as_str() == Some("d47-io-unsafe-txn"))
            .count()
    };
    assert_eq!(
        count_d47(&unsuppressed),
        2,
        "two d47 findings before suppression"
    );
    assert_eq!(
        count_d47(&suppressed),
        1,
        "the correct-detector pragma removes one d47; the wrong-detector pragma keeps the other"
    );
}

// ===========================================================================
// Stage 3B — baseline
// ===========================================================================

/// Build args for the ws-d8-commit-in-tx fixture under the transaction-integrity preset.
fn d8_args(baseline: Option<&str>, update: bool, fail_on: Option<&str>) -> AnalyzeArgs {
    let ws = corpus_dir().join("ws-d8-commit-in-tx");
    assert!(
        ws.is_dir(),
        "ws-d8-commit-in-tx fixture missing at {} (offline corpus incomplete)",
        ws.display()
    );
    AnalyzeArgs {
        workspace: ws.to_string_lossy().to_string(),
        min_severity: None,
        detector: None,
        preset: Some("transaction-integrity".to_string()),
        scope: Scope::Primary,
        limit: None,
        format: OutputFormat::Sarif,
        sarif_version_override: Some(PIN_VERSION.to_string()),
        fail_on: fail_on.map(|s| s.to_string()),
        require_dependencies: false,
        baseline: baseline.map(|s| s.to_string()),
        update_baseline: update,
        disable_inline_suppression: false,
        group_by: None,
        deterministic: false,
        with_evidence: false,
        single_app: false,
    }
}

#[test]
fn baseline_full_zero_results_byte_matches() {
    let baseline_path = goldens_dir().join("ws-d8-commit-in-tx.baseline.json");
    let args = d8_args(Some(&baseline_path.to_string_lossy()), false, None);
    let rust = run_analyze(&args, "engine-default").expect("run_analyze");
    let golden = read_golden("ws-d8-commit-in-tx.baselined.sarif.json");
    if rust != golden {
        first_diff("ws-d8.baselined.sarif", &golden, &rust);
    }
    assert_eq!(rust, golden, "full-baselined SARIF did not byte-match");
    assert_eq!(sarif_result_count(&rust), 0, "full baseline → 0 results");
}

#[test]
fn baseline_partial_one_result_byte_matches() {
    let baseline_path = goldens_dir().join("ws-d8-commit-in-tx.partial-baseline.json");
    let args = d8_args(Some(&baseline_path.to_string_lossy()), false, None);
    let rust = run_analyze(&args, "engine-default").expect("run_analyze");
    assert_eq!(sarif_result_count(&rust), 1, "partial baseline → 1 result");
    if maybe_regen("ws-d8-commit-in-tx.partial-baselined.sarif.json", &rust) {
        return;
    }
    let golden = read_golden("ws-d8-commit-in-tx.partial-baselined.sarif.json");
    if rust != golden {
        first_diff("ws-d8.partial-baselined.sarif", &golden, &rust);
    }
    assert_eq!(rust, golden, "partial-baselined SARIF did not byte-match");
    assert_eq!(sarif_result_count(&rust), 1, "partial baseline → 1 result");
}

/// `--update-baseline` reproduces the committed full baseline byte-for-byte (proves the
/// edit-stable fingerprints + sorted-dedup + epoch generatedAt). We write to a temp path
/// (NOT the committed golden) and compare the written bytes to the committed file.
#[test]
fn update_baseline_round_trips_committed_file() {
    let tmp = std::env::temp_dir().join(format!("alsem-gate-baseline-{}.json", std::process::id()));
    // --update-baseline writes the current finding set (no baseline applied yet ⇒ both
    // findings), which is exactly the committed full baseline.
    let args = d8_args(Some(&tmp.to_string_lossy()), true, None);
    let _ = run_analyze(&args, "engine-default").expect("run_analyze");

    let written = std::fs::read_to_string(&tmp).expect("baseline written");
    let _ = std::fs::remove_file(&tmp);

    let committed = read_golden("ws-d8-commit-in-tx.baseline.json");
    if written != committed {
        first_diff("ws-d8.baseline.json round-trip", &committed, &written);
    }
    assert_eq!(
        written, committed,
        "--update-baseline did not reproduce the committed baseline byte-for-byte"
    );
}

/// The exit-code matrix matches suppress-baseline-exit.json:
///   fail-on=critical → 0 for noBaseline / full / partial (ws-d8 findings are 'high').
///   fail-on=high     → noBaseline=1, full=0, partial=1 (the 1→0 transition).
#[test]
fn exit_code_matrix_matches_golden() {
    let exit_golden: serde_json::Value =
        serde_json::from_str(&read_golden("suppress-baseline-exit.json")).unwrap();
    let m = &exit_golden["ws-d8-commit-in-tx"]["txn"];

    let full = goldens_dir().join("ws-d8-commit-in-tx.baseline.json");
    let partial = goldens_dir().join("ws-d8-commit-in-tx.partial-baseline.json");
    let full = full.to_string_lossy().to_string();
    let partial = partial.to_string_lossy().to_string();

    // (label, baseline, expected from golden key)
    let run_exit = |baseline: Option<&str>, fail_on: &str| -> u8 {
        let args = d8_args(baseline, false, Some(fail_on));
        let (_out, code, _warn) =
            run_analyze_with_exit(&args, "engine-default").expect("run_analyze_with_exit");
        code
    };

    for fail_on in ["critical", "high"] {
        let key = format!("fail-on={fail_on}");
        let g = &m[&key];
        let expect = |k: &str| -> u8 { g[k].as_u64().unwrap() as u8 };

        assert_eq!(
            run_exit(None, fail_on),
            expect("noBaseline"),
            "{key} noBaseline exit mismatch"
        );
        assert_eq!(
            run_exit(Some(&full), fail_on),
            expect("fullBaseline"),
            "{key} fullBaseline exit mismatch"
        );
        assert_eq!(
            run_exit(Some(&partial), fail_on),
            expect("partialBaseline"),
            "{key} partialBaseline exit mismatch"
        );
    }
}

/// Anti-degenerate (baseline): full baseline → 0 results + exit CLEAN(0) at fail-on=high;
/// partial baseline → 1 result + exit FINDINGS(1) at fail-on=high.
#[test]
fn anti_degenerate_baseline_zero_vs_partial_and_exit() {
    let full = goldens_dir()
        .join("ws-d8-commit-in-tx.baseline.json")
        .to_string_lossy()
        .to_string();
    let partial = goldens_dir()
        .join("ws-d8-commit-in-tx.partial-baseline.json")
        .to_string_lossy()
        .to_string();

    let (full_out, full_exit, _) =
        run_analyze_with_exit(&d8_args(Some(&full), false, Some("high")), "engine-default")
            .unwrap();
    assert_eq!(
        sarif_result_count(&full_out),
        0,
        "full baseline → 0 results"
    );
    assert_eq!(full_exit, 0, "full baseline → exit CLEAN");

    let (partial_out, partial_exit, _) = run_analyze_with_exit(
        &d8_args(Some(&partial), false, Some("high")),
        "engine-default",
    )
    .unwrap();
    assert_eq!(
        sarif_result_count(&partial_out),
        1,
        "partial baseline → 1 result"
    );
    assert_eq!(partial_exit, 1, "partial baseline → exit FINDINGS");
}

/// The Rust `serialize_baseline` over the no-baseline finding set reproduces the
/// committed full baseline file byte-for-byte (a pure-function cross-check of the
/// sorted-dedup + epoch, independent of the file-write path).
#[test]
fn serialize_baseline_matches_committed_fingerprints() {
    // The committed full baseline lists both fingerprints, sorted; the partial lists one.
    let committed_full = read_golden("ws-d8-commit-in-tx.baseline.json");
    let parsed: serde_json::Value = serde_json::from_str(&committed_full).unwrap();
    let fps: Vec<&str> = parsed["fingerprints"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_str().unwrap())
        .collect();
    assert_eq!(fps.len(), 2, "full baseline has 2 fingerprints");

    // Re-serialize from FindingSummary-like inputs (reuse the unit-tested helper path):
    // build summaries that only carry the fingerprints, in REVERSE order + a dup, to
    // prove the sort+dedup. We reach through the public serialize_baseline.
    use al_sem::engine::gate::projection::{FindingLocation, FindingSummary};
    let mk = |fp: &str| FindingSummary {
        id: fp.to_string(),
        fingerprint: fp.to_string(),
        detector: "d8-commit-in-transaction".to_string(),
        title: String::new(),
        root_cause: String::new(),
        severity: "high".to_string(),
        confidence_level: "high".to_string(),
        confidence_capped_by: None,
        primary_location: FindingLocation {
            file: "ws:src/A.al".to_string(),
            line: 1,
            column: 1,
            object_id: None,
            object_name: None,
            routine_id: None,
            routine_name: None,
        },
        terminal_location: None,
        affected_objects: vec![],
        affected_tables: vec![],
        path_count: 1,
        fix_hint: None,
    };
    // Reverse order + duplicate to exercise sort+dedup.
    let findings = vec![mk(fps[1]), mk(fps[0]), mk(fps[1])];
    let serialized = serialize_baseline(&findings);
    assert_eq!(
        serialized, committed_full,
        "serialize_baseline did not reproduce the committed baseline (sort/dedup/epoch)"
    );
}

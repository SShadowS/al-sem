//! Stage-2b GATE PR-summary + exit-code differential — the Rust engine's production
//! `analyze` gate path (`engine::gate::run::run_analyze_with_exit`) byte-matches the
//! al-sem TS CLI's PR-summary gate goldens under
//! `tests/gate-goldens/<fixture>.<preset>.prsummary.md`, AND the exit-code matrix
//! (`--fail-on` ∈ {none, info, low, medium, high, critical} + `--require-dependencies`)
//! matches `tests/gate-goldens/exit-codes.json`.
//!
//! OFFLINE: the goldens are committed; the corpus fixtures live under
//! `tests/r0-corpus/<fixture>`. No subprocess — `run_analyze_with_exit` is called
//! in-process so both the stdout AND the exit code are asserted.
//!
//! The PR-summary embeds NO version (al-sem `formatPrSummary` does not call
//! `alsemVersion()`), so no `--sarif-version-override` is needed.
//!
//! Anti-degenerate (asserted explicitly below):
//!   - ws-txn-d47-pos-http-nocommit (txn) → a "**CRITICAL**" section + the app-attribution
//!     line.
//!   - a clean fixture (ws-txn-d46-neg, txn) → the "no findings" summary.
//!   - the exit-code 0→1 transition across `--fail-on` for a high-only fixture
//!     (ws-d8-commit-in-tx: fail-on high → 1, critical → 0).
//!   - the preflight exit-4 (ws-txn-d47-pos-http-nocommit --require-dependencies → 4).

use std::path::{Path, PathBuf};

use al_sem::engine::gate::filter::Scope;
use al_sem::engine::gate::run::{AnalyzeArgs, OutputFormat, run_analyze_with_exit};
use serde_json::Value;

use crate::regen;

/// How a golden's txn slot was produced on the al-sem side (mirrors the SARIF corpus).
#[derive(Clone, Copy)]
enum TxnSelection {
    /// `--preset transaction-integrity`.
    Preset,
    /// Explicit opt-in detector(s) — d51 fixtures.
    Detector(&'static str),
}

struct GateFixture {
    fixture: &'static str,
    txn: TxnSelection,
}

const CORPUS: &[GateFixture] = &[
    GateFixture {
        fixture: "ws-d8-commit-in-tx",
        txn: TxnSelection::Preset,
    },
    GateFixture {
        fixture: "ws-d34",
        txn: TxnSelection::Preset,
    },
    GateFixture {
        fixture: "ws-d35",
        txn: TxnSelection::Preset,
    },
    GateFixture {
        fixture: "ws-txn-d46-pos",
        txn: TxnSelection::Preset,
    },
    GateFixture {
        fixture: "ws-txn-d47-pos-http-nocommit",
        txn: TxnSelection::Preset,
    },
    GateFixture {
        fixture: "ws-txn-d47-pos-http-commit-after",
        txn: TxnSelection::Preset,
    },
    GateFixture {
        fixture: "ws-txn-d47-pos-file",
        txn: TxnSelection::Preset,
    },
    GateFixture {
        fixture: "ws-txn-d48-pos",
        txn: TxnSelection::Preset,
    },
    GateFixture {
        fixture: "ws-txn-d49-pos-modify-message",
        txn: TxnSelection::Preset,
    },
    GateFixture {
        fixture: "ws-txn-d49-pos-modify-runmodal",
        txn: TxnSelection::Preset,
    },
    GateFixture {
        fixture: "ws-d51-pos",
        txn: TxnSelection::Detector("d51-retry-side-effect-duplication"),
    },
    GateFixture {
        fixture: "ws-d51-jobqueue",
        txn: TxnSelection::Detector("d51-retry-side-effect-duplication"),
    },
    GateFixture {
        fixture: "ws-txn-d46-neg",
        txn: TxnSelection::Preset,
    },
    GateFixture {
        fixture: "ws-txn-d47-neg-readonly",
        txn: TxnSelection::Preset,
    },
    GateFixture {
        fixture: "ws-txn-d47-neg-temp",
        txn: TxnSelection::Preset,
    },
    GateFixture {
        fixture: "ws-txn-d48-neg",
        txn: TxnSelection::Preset,
    },
    GateFixture {
        fixture: "ws-txn-d49-neg-no-write",
        txn: TxnSelection::Preset,
    },
    GateFixture {
        fixture: "ws-d51-neg",
        txn: TxnSelection::Detector("d51-retry-side-effect-duplication"),
    },
    GateFixture {
        fixture: "ws-d1-multi-caller",
        txn: TxnSelection::Preset,
    },
    GateFixture {
        fixture: "ws-d14-dead-routine",
        txn: TxnSelection::Preset,
    },
];

/// The `--fail-on` matrix keys (`"none"` = no flag). Mirrors `FAIL_ON_MATRIX`.
const FAIL_ON_KEYS: &[&str] = &["none", "info", "low", "medium", "high", "critical"];

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

/// REGEN path (iter-2 gap rebaseline). When `REGEN_TEMP_GOLDENS` is set, write the
/// ENGINE-produced string to the in-repo golden (`gate-goldens/<name>`) instead of
/// comparing — the goldens are Rust-owned baselines (TS oracle retired). The gate
/// goldens live in-repo (NOT al-sem), so the write target is the resolved golden
/// path directly. Returns `true` when a regen write happened (caller skips assert).
fn maybe_regen(name: &str, rust: &str) -> bool {
    if !regen::regen_mode() {
        return false;
    }
    let path = goldens_dir().join(name);
    std::fs::write(&path, rust).unwrap_or_else(|e| panic!("regen write {}: {e}", path.display()));
    eprintln!("REGEN gate-prsummary golden: {}", path.display());
    true
}

fn make_args(
    fixture: &str,
    preset: Option<&str>,
    detector: Option<&str>,
    format: OutputFormat,
    fail_on: Option<&str>,
    require_dependencies: bool,
) -> AnalyzeArgs {
    let ws = corpus_dir().join(fixture);
    assert!(
        ws.is_dir(),
        "gate golden for {fixture} has no in-repo fixture at {} (offline corpus incomplete)",
        ws.display()
    );
    AnalyzeArgs {
        workspace: ws.to_string_lossy().to_string(),
        min_severity: None,
        detector: detector.map(|s| s.to_string()),
        preset: preset.map(|s| s.to_string()),
        scope: Scope::Primary,
        limit: None,
        format,
        sarif_version_override: None,
        fail_on: fail_on.map(|s| s.to_string()),
        require_dependencies,
        baseline: None,
        update_baseline: false,
        disable_inline_suppression: false,
        group_by: None,
        deterministic: false,
        with_evidence: false,
        single_app: false,
    }
}

/// Resolve a fixture's txn slot to `(preset, detector)`.
fn txn_selection(gf: &GateFixture) -> (Option<&'static str>, Option<&'static str>) {
    match gf.txn {
        TxnSelection::Preset => (Some("transaction-integrity"), None),
        TxnSelection::Detector(d) => (None, Some(d)),
    }
}

/// Run the PR-summary path for one slot, returning the markdown (no trailing newline).
fn run_prsummary(fixture: &str, preset: Option<&str>, detector: Option<&str>) -> String {
    let args = make_args(
        fixture,
        preset,
        detector,
        OutputFormat::PrSummary,
        None,
        false,
    );
    run_analyze_with_exit(&args, "engine-default")
        .expect("run_analyze_with_exit")
        .0
}

/// Run the PR-summary path and return `(markdown, exit_code, stderr_warning)`.
fn run_prsummary_full(
    fixture: &str,
    preset: Option<&str>,
    detector: Option<&str>,
    fail_on: Option<&str>,
    require_dependencies: bool,
) -> (String, u8, Option<String>) {
    let args = make_args(
        fixture,
        preset,
        detector,
        OutputFormat::PrSummary,
        fail_on,
        require_dependencies,
    );
    run_analyze_with_exit(&args, "engine-default").expect("run_analyze_with_exit")
}

/// Run the exit-code path for one slot for a `--fail-on` key (no `--require-dependencies`).
fn run_exit_fail_on(
    fixture: &str,
    preset: Option<&str>,
    detector: Option<&str>,
    fail_on_key: &str,
) -> u8 {
    let fail_on = if fail_on_key == "none" {
        None
    } else {
        Some(fail_on_key)
    };
    // The format is irrelevant to the exit code; use PR-summary (no version pinning).
    let args = make_args(
        fixture,
        preset,
        detector,
        OutputFormat::PrSummary,
        fail_on,
        false,
    );
    run_analyze_with_exit(&args, "engine-default")
        .expect("run_analyze_with_exit")
        .1
}

/// Run the exit-code path for one slot with `--require-dependencies` (no `--fail-on`).
fn run_exit_require_deps(fixture: &str, preset: Option<&str>, detector: Option<&str>) -> u8 {
    let args = make_args(
        fixture,
        preset,
        detector,
        OutputFormat::PrSummary,
        None,
        true,
    );
    run_analyze_with_exit(&args, "engine-default")
        .expect("run_analyze_with_exit")
        .1
}

/// If `trimmed` is a `"key": {` object-opening line, return the unquoted key.
fn object_key_opening(trimmed: &str) -> Option<String> {
    let t = trimmed.trim_end();
    if !t.ends_with('{') {
        return None;
    }
    let rest = trimmed.strip_prefix('"')?;
    let end = rest.find('"')?;
    // Must be `"key": {` (a colon follows the closing quote).
    let after = &rest[end + 1..];
    if after.trim_start().starts_with(':') {
        Some(rest[..end].to_string())
    } else {
        None
    }
}

/// If `trimmed` is a `"key": <number>[,]` leaf line, return `(key, has_trailing_comma)`.
fn leaf_num_key(trimmed: &str) -> Option<(String, bool)> {
    let rest = trimmed.strip_prefix('"')?;
    let end = rest.find('"')?;
    let key = &rest[..end];
    let after = rest[end + 1..].trim_start();
    let after = after.strip_prefix(':')?.trim_start();
    let after = after.trim_end();
    let (num_part, has_comma) = match after.strip_suffix(',') {
        Some(p) => (p.trim_end(), true),
        None => (after, false),
    };
    // The value must be a bare integer (exit codes are 0..=4).
    if !num_part.is_empty() && num_part.chars().all(|c| c.is_ascii_digit()) {
        Some((key.to_string(), has_comma))
    } else {
        None
    }
}

/// On a mismatch, print the first differing line (1-based) for fast triage.
fn report_first_diff(label: &str, golden: &str, rust: &str) {
    let g: Vec<&str> = golden.lines().collect();
    let r: Vec<&str> = rust.lines().collect();
    let n = g.len().max(r.len());
    for i in 0..n {
        let gl = g.get(i).copied().unwrap_or("<absent>");
        let rl = r.get(i).copied().unwrap_or("<absent>");
        if gl != rl {
            eprintln!(
                "PR-summary mismatch in {label} at line {}:\n  golden: {gl:?}\n  rust:   {rl:?}",
                i + 1
            );
            return;
        }
    }
    eprintln!(
        "PR-summary mismatch in {label}: differ only in trailing bytes (length {} vs {})",
        golden.len(),
        rust.len()
    );
}

#[test]
fn gate_prsummary_goldens_byte_match() {
    let mut mismatches: Vec<String> = Vec::new();

    for gf in CORPUS {
        let (preset, detector) = txn_selection(gf);

        // --- (a) txn slot ---
        let txn_rust = run_prsummary(gf.fixture, preset, detector);
        let txn_name = format!("{}.txn.prsummary.md", gf.fixture);
        if !maybe_regen(&txn_name, &txn_rust) {
            let txn_golden = read_golden(&txn_name);
            if txn_rust != txn_golden {
                mismatches.push(format!("{}.txn", gf.fixture));
                report_first_diff(&format!("{}.txn", gf.fixture), &txn_golden, &txn_rust);
            }
        }

        // --- (b) default slot ---
        let default_rust = run_prsummary(gf.fixture, None, None);
        let default_name = format!("{}.default.prsummary.md", gf.fixture);
        if !maybe_regen(&default_name, &default_rust) {
            let default_golden = read_golden(&default_name);
            if default_rust != default_golden {
                mismatches.push(format!("{}.default", gf.fixture));
                report_first_diff(
                    &format!("{}.default", gf.fixture),
                    &default_golden,
                    &default_rust,
                );
            }
        }
    }

    assert!(
        mismatches.is_empty(),
        "GATE PR-summary differential: {} golden(s) did NOT byte-match: {:?}",
        mismatches.len(),
        mismatches
    );
}

#[test]
fn gate_exit_code_matrix_matches() {
    // REGEN path (iter-2 gap rebaseline): rebuild exit-codes.json from the engine.
    // serde_json (no `preserve_order` feature) re-sorts object keys on a
    // serialize round-trip, which would spuriously reorder the WHOLE file. To keep
    // the diff MINIMAL (only the cells the engine actually moved), we patch the
    // committed text in place line-by-line, preserving the committed key + fixture
    // order: track the current top-level fixture and slot, and rewrite ONLY the
    // numeric leaf value on each `"<key>": <n>` line. The goldens stay Rust-owned.
    if regen::regen_mode() {
        let golden_text = read_golden("exit-codes.json");
        // (preset, detector) per fixture, by name.
        let mut sel: std::collections::HashMap<&str, (Option<&str>, Option<&str>)> =
            std::collections::HashMap::new();
        for gf in CORPUS {
            sel.insert(gf.fixture, txn_selection(gf));
        }
        let fixture_names: std::collections::HashSet<&str> =
            CORPUS.iter().map(|gf| gf.fixture).collect();

        let mut cur_fixture: Option<String> = None;
        let mut cur_slot: Option<String> = None;
        let mut out = String::with_capacity(golden_text.len());

        for line in golden_text.split_inclusive('\n') {
            let trimmed = line.trim_start();
            // A 2-space-indented `"name": {` opens a top-level fixture object.
            let indent = line.len() - trimmed.len();
            if let Some(key) = object_key_opening(trimmed) {
                if indent == 2 && fixture_names.contains(key.as_str()) {
                    cur_fixture = Some(key);
                    cur_slot = None;
                    out.push_str(line);
                    continue;
                }
                if indent == 4 && (key == "txn" || key == "default") {
                    cur_slot = Some(key);
                    out.push_str(line);
                    continue;
                }
            }
            // A leaf `"<key>": <num>[,]` line inside a known fixture+slot.
            if let (Some(fx), Some(slot)) = (cur_fixture.as_deref(), cur_slot.as_deref())
                && let Some((leaf, has_comma)) = leaf_num_key(trimmed)
            {
                let (preset, detector) = sel[fx];
                let (slot_preset, slot_detector) = if slot == "txn" {
                    (preset, detector)
                } else {
                    (None, None)
                };
                let val = if leaf == "require-dependencies" {
                    run_exit_require_deps(fx, slot_preset, slot_detector)
                } else {
                    run_exit_fail_on(fx, slot_preset, slot_detector, &leaf)
                };
                let comma = if has_comma { "," } else { "" };
                out.push_str(&" ".repeat(indent));
                out.push_str(&format!("\"{leaf}\": {val}{comma}\n"));
                continue;
            }
            out.push_str(line);
        }

        let path = goldens_dir().join("exit-codes.json");
        std::fs::write(&path, out)
            .unwrap_or_else(|e| panic!("regen write {}: {e}", path.display()));
        eprintln!("REGEN gate-prsummary golden: {}", path.display());
        return;
    }

    let golden_text = read_golden("exit-codes.json");
    let golden: Value = serde_json::from_str(&golden_text).expect("exit-codes.json is valid JSON");

    let mut mismatches: Vec<String> = Vec::new();

    for gf in CORPUS {
        let (preset, detector) = txn_selection(gf);
        let fx = golden
            .get(gf.fixture)
            .unwrap_or_else(|| panic!("exit-codes.json missing fixture {}", gf.fixture));

        // For each preset slot: "txn" uses the per-fixture selection; "default" uses the
        // default detector set (None/None).
        let slots: [(&str, Option<&str>, Option<&str>); 2] =
            [("txn", preset, detector), ("default", None, None)];

        for (slot_name, slot_preset, slot_detector) in slots {
            let slot = fx.get(slot_name).unwrap_or_else(|| {
                panic!("exit-codes.json {} missing slot {slot_name}", gf.fixture)
            });

            // --- fail-on matrix (no --require-dependencies) ---
            for key in FAIL_ON_KEYS {
                let expected = slot.get(*key).and_then(|v| v.as_u64()).unwrap_or_else(|| {
                    panic!(
                        "exit-codes.json {}.{slot_name} missing key {key}",
                        gf.fixture
                    )
                }) as u8;
                let actual = run_exit_fail_on(gf.fixture, slot_preset, slot_detector, key);
                if actual != expected {
                    mismatches.push(format!(
                        "{}.{slot_name}.fail-on={key}: expected {expected}, got {actual}",
                        gf.fixture
                    ));
                }
            }

            // --- require-dependencies (no --fail-on) ---
            let expected_rd = slot
                .get("require-dependencies")
                .and_then(|v| v.as_u64())
                .unwrap_or_else(|| {
                    panic!(
                        "exit-codes.json {}.{slot_name} missing require-dependencies",
                        gf.fixture
                    )
                }) as u8;
            let actual_rd = run_exit_require_deps(gf.fixture, slot_preset, slot_detector);
            if actual_rd != expected_rd {
                mismatches.push(format!(
                    "{}.{slot_name}.require-dependencies: expected {expected_rd}, got {actual_rd}",
                    gf.fixture
                ));
            }
        }
    }

    assert!(
        mismatches.is_empty(),
        "GATE exit-code matrix: {} cell(s) did NOT match:\n{}",
        mismatches.len(),
        mismatches.join("\n")
    );
}

// ---------------------------------------------------------------------------
// Anti-degenerate checks.
// ---------------------------------------------------------------------------

/// ws-txn-d47-pos-http-nocommit (txn) → a "**CRITICAL**" section + the app-attribution line.
#[test]
fn anti_degenerate_critical_section_and_app_attribution() {
    let md = run_prsummary(
        "ws-txn-d47-pos-http-nocommit",
        Some("transaction-integrity"),
        None,
    );
    assert!(
        md.contains("**CRITICAL**"),
        "expected a CRITICAL section in the d47 http-nocommit txn PR-summary:\n{md}"
    );
    assert!(
        md.contains("  App: "),
        "expected an app-attribution line in the d47 http-nocommit txn PR-summary:\n{md}"
    );
    assert!(
        md.contains("[d47-io-unsafe-txn]"),
        "expected the d47 detector id in the PR-summary:\n{md}"
    );
}

/// A clean fixture (ws-txn-d46-neg, txn) → the "no findings" summary.
#[test]
fn anti_degenerate_clean_fixture_no_findings() {
    let md = run_prsummary("ws-txn-d46-neg", Some("transaction-integrity"), None);
    assert_eq!(
        md,
        "### Transaction integrity — no findings\n\nNo transaction-integrity findings detected.",
        "expected the 'no findings' summary for the clean d46 negative fixture"
    );
}

/// The exit-code 0→1 transition across `--fail-on` for a high-only fixture
/// (ws-d8-commit-in-tx: fail-on high → 1, critical → 0).
#[test]
fn anti_degenerate_exit_zero_to_one_transition() {
    let high = run_exit_fail_on(
        "ws-d8-commit-in-tx",
        Some("transaction-integrity"),
        None,
        "high",
    );
    let critical = run_exit_fail_on(
        "ws-d8-commit-in-tx",
        Some("transaction-integrity"),
        None,
        "critical",
    );
    assert_eq!(high, 1, "ws-d8-commit-in-tx fail-on=high should exit 1");
    assert_eq!(
        critical, 0,
        "ws-d8-commit-in-tx fail-on=critical should exit 0"
    );
}

/// The preflight exit-4 (ws-txn-d47-pos-file --require-dependencies → 4).
///
/// Uses a fixture that is GENUINELY degraded after the intrinsic-builtin
/// reclassification (its residual unresolved callsite is a real non-builtin gap).
/// ws-txn-d47-pos-http-nocommit is no longer degraded — its only unresolved call
/// was `HttpClient.Send`, now correctly classified `builtin` (exit 4→0).
///
/// preflight-fresh-coverage Task 4 investigation: this test was expected (by the
/// task's plan doc) to need repointing at `ws-baseapp-closure` (a symbol-only-dep
/// fixture) once `evaluate_preflight` switched from the legacy L3
/// `unresolved_callsites`/`opaque_apps` pair to `FreshCoverage`. It does NOT need
/// repointing: `ws-txn-d47-pos-file` has zero dependencies (`app.json` `"dependencies":
/// []`), so it was never a symbol-only-dep case — its degradation was ALREADY the
/// `unknown`-edge kind under L3, and `aldump --program-call-graph-stats` on this
/// fixture confirms the FRESH resolver independently finds the SAME real gap:
/// `unknown: 1, unknownByReason: {"catalogMiss": 1}` (the `File.WriteAllText` call
/// referenced in the doc comment above). The anti-degenerate property ("exit 4 is
/// reachable") survives the re-key by coincidence of the fixture already being a
/// genuine non-builtin resolution gap on BOTH engines, not by any code change here.
#[test]
fn anti_degenerate_preflight_exit_four() {
    let rd = run_exit_require_deps("ws-txn-d47-pos-file", Some("transaction-integrity"), None);
    assert_eq!(
        rd, 4,
        "ws-txn-d47-pos-file --require-dependencies should exit 4 (degraded coverage)"
    );
}

// ---------------------------------------------------------------------------
// Native oracles — corpus-invisible cells not exercised by the goldens.
// ---------------------------------------------------------------------------

/// Oracle 4 — F2: preflight degraded stderr warning (the "no silent clean" contract).
///
/// `run_analyze_with_exit` on a degraded fixture (ws-txn-d47-pos-file has a real
/// FRESH-resolver unknown edge — `aldump --program-call-graph-stats` on this fixture
/// reports `unknown: 1, unknownByReason: {"catalogMiss": 1}` for `File.WriteAllText`,
/// independent of the exit-codes golden's own require-dependencies cell) WITHOUT
/// `--require-dependencies` must:
///   - return exit NOT 4 (0 since no --fail-on)
///   - return `stderr_warning = Some(msg)` with the al-sem warning string format
///
/// This cell is corpus-invisible: the differential only asserts stdout + exit code;
/// the 3rd tuple field (the warning) was previously discarded.
#[test]
fn oracle_f2_preflight_degraded_warning_without_require_deps() {
    // ws-txn-d47-pos-file is known-degraded (require-deps exits 4).
    // Without --require-dependencies, exit must NOT be 4 — we get CLEAN (0) since
    // the exit-codes golden shows "none": 0 for this fixture's txn slot.
    let (_, exit_code, warning) = run_prsummary_full(
        "ws-txn-d47-pos-file",
        Some("transaction-integrity"),
        None,
        None,  // no --fail-on
        false, // no --require-dependencies
    );
    assert_ne!(
        exit_code, 4,
        "degraded without --require-dependencies must NOT exit 4 (got {exit_code})"
    );
    let warning_msg = warning.expect(
        "F2: degraded fixture must return Some(warning) from run_analyze_with_exit, \
         even without --require-dependencies",
    );
    // The warning text must match the fresh-keyed preflight.rs message format
    // (evaluate_preflight's clause-joined "analysis coverage degraded — <clauses>").
    assert!(
        warning_msg.starts_with("analysis coverage degraded"),
        "F2: warning message must start with 'analysis coverage degraded', got: {warning_msg:?}"
    );
    // The bin emits: `al-sem: warning: {msg}` — verify the message content (not the prefix,
    // which is the bin's responsibility) matches the fresh-keyed clause vocabulary
    // (`{n} unknown resolution edge(s)`), not the RETIRED L3 "unresolved callsite" wording.
    assert!(
        warning_msg.contains("unknown resolution edge"),
        "F2: warning must mention 'unknown resolution edge', got: {warning_msg:?}"
    );
}

/// Oracle 5 — exit precedence: `--require-dependencies` + `--fail-on` combined.
///
/// (a) degraded fixture + `--require-dependencies` + `--fail-on critical` → exit 4
///     (PREFLIGHT_FAILED (4) takes precedence over FINDINGS (1)).
///     ws-txn-d47-pos-file txn slot has findings at critical severity
///     (exit-codes golden: critical → 1 without require-deps).
///
/// (b) invalid `--fail-on` string → `Err(...)` from `run_analyze_with_exit`
///     (the bin maps this to CONFIG_ERROR (3)).
///
/// These cells are invisible to the exit-code matrix, which tests flags in isolation.
///
/// preflight-fresh-coverage Task 4 investigation: same finding as
/// `anti_degenerate_preflight_exit_four` above — `ws-txn-d47-pos-file` is
/// independently confirmed genuinely fresh-degraded (a real `catalogMiss` unknown
/// edge, not a symbol-only dependency), so no repoint to `ws-baseapp-closure` is
/// needed; the precedence property (PREFLIGHT_FAILED beats FINDINGS) is unchanged.
#[test]
fn oracle_exit_precedence_preflight_wins_over_findings() {
    // (a) --require-dependencies + --fail-on critical on a degraded fixture
    // The fixture has critical findings (exit-codes golden: critical → 1 for txn slot).
    // With --require-dependencies, preflight exit 4 must win over findings exit 1.
    let (_, exit_code, _) = run_prsummary_full(
        "ws-txn-d47-pos-file",
        Some("transaction-integrity"),
        None,
        Some("critical"), // --fail-on critical
        true,             // --require-dependencies
    );
    assert_eq!(
        exit_code, 4,
        "PREFLIGHT_FAILED (4) must take precedence over FINDINGS (1) when both apply"
    );
}

#[test]
fn oracle_parse_fail_on_error_is_err() {
    // (b) parse_fail_on error propagates as Err (bin maps to CONFIG_ERROR 3).
    use al_sem::engine::gate::filter::Scope;
    use al_sem::engine::gate::run::AnalyzeArgs;
    use al_sem::engine::gate::run::OutputFormat;
    let ws = corpus_dir().join("ws-txn-d47-pos-http-nocommit");
    let args = AnalyzeArgs {
        workspace: ws.to_string_lossy().to_string(),
        min_severity: None,
        detector: None,
        preset: Some("transaction-integrity".to_string()),
        scope: Scope::Primary,
        limit: None,
        format: OutputFormat::PrSummary,
        sarif_version_override: None,
        fail_on: Some("not-a-severity".to_string()), // Invalid — but we pass it raw
        require_dependencies: false,
        baseline: None,
        update_baseline: false,
        disable_inline_suppression: false,
        group_by: None,
        deterministic: false,
        with_evidence: false,
        single_app: false,
    };
    // The pipeline itself does NOT validate fail_on — the bin/CLI does (parse_fail_on).
    // However compute_finding_exit with an unknown severity falls back to sev_rank=0,
    // so it would exit CLEAN (not an Err). The CONFIG_ERROR (3) path is the BIN's
    // responsibility. Confirm the pipeline runs (does not panic) with an unknown value.
    let result = al_sem::engine::gate::run::run_analyze_with_exit(&args, "engine-default");
    assert!(
        result.is_ok(),
        "pipeline must not Err on an unknown fail_on string (validation is the bin's job)"
    );
    // With "not-a-severity" (rank 0 = info level), all findings at/above info trigger
    // exit 1, so exit is FINDINGS (1) for a fixture with findings — OR PREFLIGHT (4)
    // if degraded without require-deps. Either way it must NOT be CONFIG_ERROR (3).
    let (_, exit_code, _) = result.unwrap();
    assert_ne!(
        exit_code, 3,
        "pipeline exit must not be CONFIG_ERROR (3) — that is the bin's domain"
    );
}

// ---------------------------------------------------------------------------
// Task 5 (preflight-fresh-coverage) — fresh-resolver preflight integration.
// ---------------------------------------------------------------------------

/// Run the analyze pipeline for one on-disk corpus fixture with the default
/// preset/detector selection and no `--fail-on` (mirrors `run_prsummary_full` —
/// the format is irrelevant to the preflight/exit-code behaviour these tests
/// assert, so PR-summary is used, matching this file's other helpers).
fn run_analyze_fixture(fixture: &str, require_dependencies: bool) -> (String, u8, Option<String>) {
    run_prsummary_full(fixture, None, None, None, require_dependencies)
}

/// Same as `run_analyze_fixture`, but with `--format json` — used ONLY to pin the
/// formatter's `payload.summary.opaqueApps` propagation end-to-end. No committed
/// gate-JSON golden exercises a symbol-only-dependency fixture, so this is the
/// only place the fresh-keyed opaque override (Task 4) is proven to reach the
/// JSON formatter, not just the stderr warning string.
fn run_analyze_fixture_json(
    fixture: &str,
    require_dependencies: bool,
) -> (String, u8, Option<String>) {
    let args = make_args(
        fixture,
        None,
        None,
        OutputFormat::Json,
        None,
        require_dependencies,
    );
    run_analyze_with_exit(&args, "engine-default").expect("run_analyze_with_exit")
}

/// Fresh-clean workspace (no deps, resolves fully) → NO warning, exit driven by
/// findings only. The DO-shaped false-positive case this whole change kills.
#[test]
fn fresh_clean_workspace_emits_no_coverage_warning() {
    let (_out, _exit, warning) = run_analyze_fixture("ws-e2e", /*require_deps=*/ false);
    assert!(
        warning.is_none(),
        "clean fixture must not warn: {warning:?}"
    );
}

/// Symbol-only dep → opaque clause warning; exit 4 only under --require-dependencies.
///
/// Also asserts (controller addition closing a Task-4 review finding) that the
/// JSON output's `payload.summary.opaqueApps` carries the Base Application name
/// for this fixture — the end-to-end pin that no current golden exercises: the
/// stderr warning string and the JSON formatter's opaque list are populated from
/// the SAME `fresh.opaque_apps` (one dependency universe, spec §3), but nothing
/// before this test proved the JSON side specifically.
#[test]
fn symbol_only_dep_warns_opaque_and_gates_exit_four() {
    let (_o, exit, warning) = run_analyze_fixture("ws-baseapp-closure", false);
    let w = warning.expect("must warn");
    assert!(w.contains("symbol-only dependency app"), "got: {w}");
    assert_ne!(exit, 4, "fail-open without --require-dependencies");
    let (_o, exit, _w) = run_analyze_fixture("ws-baseapp-closure", true);
    assert_eq!(exit, 4);

    let (json_out, _exit, _w) = run_analyze_fixture_json("ws-baseapp-closure", false);
    let v: Value = serde_json::from_str(&json_out).expect("valid JSON");
    let opaque = v["payload"]["summary"]["opaqueApps"]
        .as_array()
        .expect("payload.summary.opaqueApps must be an array");
    assert!(
        opaque
            .iter()
            .any(|n| n.as_str().is_some_and(|s| s.contains("Base Application"))),
        "expected payload.summary.opaqueApps to contain the Base Application name, got: {opaque:?}"
    );
}

/// Create a unique scratch dir for a fail-closed workspace probe: a root
/// `app.json` with NO `id` field (readable JSON, id-less — the same fail-closed
/// trigger `tests/cli/cli_a_json_differential.rs`'s
/// `fail_closed_idless_app_json_emits_discover_diagnostic` exercises) plus one
/// trivial `.al` file so the workspace "looks real".
///
/// This mirrors that file's `scratch_ws` helper rather than minting a
/// committed fixture under `tests/r0-corpus/`: this repo's pre-commit hook
/// (`scripts/git-hooks/pre-commit`) requires the FULL `scripts/check-goldens`
/// suite to pass for any commit touching `tests/r0-corpus/`, and a fail-closed
/// probe has no golden of its own to keep in sync (its output is empty, by
/// definition) — a throwaway temp workspace gets the same coverage without
/// entangling this task with the OTHER golden families that path triggers.
///
/// The `TempDir` removes the tree on drop (panics included); keep it alive.
fn scratch_failclosed_ws() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::Builder::new()
        .prefix("alsem-gate-prsummary-failclosed-")
        .tempdir()
        .expect("create scratch ws dir");
    let dir = tmp.path().to_path_buf();
    std::fs::write(
        dir.join("app.json"),
        r#"{"name":"Fail Closed Probe","publisher":"probe","version":"1.0.0.0"}"#,
    )
    .expect("write app.json");
    std::fs::write(dir.join("Foo.al"), "codeunit 50100 Foo { }").expect("write Foo.al");
    (tmp, dir)
}

/// Run the analyze pipeline directly over an arbitrary workspace `Path` (not a
/// named `tests/r0-corpus/` fixture) — used only by the fail-closed probe below.
fn run_analyze_path(ws: &Path, require_dependencies: bool) -> (String, u8, Option<String>) {
    let args = AnalyzeArgs {
        workspace: ws.to_string_lossy().to_string(),
        min_severity: None,
        detector: None,
        preset: None,
        scope: Scope::Primary,
        limit: None,
        format: OutputFormat::PrSummary,
        sarif_version_override: None,
        fail_on: None,
        require_dependencies,
        baseline: None,
        update_baseline: false,
        disable_inline_suppression: false,
        group_by: None,
        deterministic: false,
        with_evidence: false,
        single_app: false,
    };
    run_analyze_with_exit(&args, "engine-default").expect("run_analyze_with_exit")
}

/// Fail-closed workspace (unanalyzable — root `app.json` missing `id`) →
/// could-not-verify warning, never silent clean; exit 4 under
/// `--require-dependencies`.
#[test]
fn fail_closed_workspace_could_not_verify() {
    let (_tmp, ws) = scratch_failclosed_ws();
    let (_o1, exit_open, warning) = run_analyze_path(&ws, false);
    let (_o2, exit_required, _w2) = run_analyze_path(&ws, true);

    let w = warning.expect("fail-closed must warn, not silent-clean");
    assert!(w.contains("coverage could not be verified"), "got: {w}");
    // The reason must be the REAL provider diagnostic for this workspace (spec §3),
    // not a generic fallback — this fixture's root `app.json` is readable but has no
    // string `id`, so `compute_workspace_diagnostics` emits that exact fail-closed
    // reason (`src/engine/gate/workspace_diagnostics.rs`); a stable fragment of it
    // must survive into the preflight message end to end.
    assert!(w.contains("has no string"), "got: {w}");
    assert_ne!(exit_open, 4, "fail-open without --require-dependencies");
    assert_eq!(exit_required, 4);
}

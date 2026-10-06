//! Stage-1b GATE-SARIF differential — the Rust engine's production `analyze` gate path
//! (`engine::gate::run::run_analyze`) byte-matches the al-sem TS CLI's SARIF gate
//! goldens under `tests/gate-goldens/<fixture>.<preset>.sarif.json`.
//!
//! OFFLINE: the goldens are committed; the corpus fixtures live under
//! `tests/r0-corpus/<fixture>`. No subprocess — `run_analyze` is called in-process.
//!
//! The differential always pins `driver.version` to "gate-sarif-v1" via the
//! `--sarif-version-override` CLI flag (the current, Rust-side mechanism; the
//! goldens were originally captured from al-sem using its own env-var-based
//! version pin, now retired).
//!
//! Two goldens per fixture:
//!   - `.txn.sarif.json`     — `--preset transaction-integrity` (except the d51
//!     fixtures, which the al-sem capture runs with the explicit opt-in detector
//!     `d51-retry-side-effect-duplication`).
//!   - `.default.sarif.json` — the default detector set.
//!
//! Anti-degenerate (asserted explicitly below):
//!   - ws-txn-d47-pos-http-nocommit (txn) → exactly 1 SARIF result, ruleId
//!     d47-io-unsafe-txn, level error.
//!   - a clean fixture (ws-txn-d46-neg, txn) → 0 results.
//!   - ≥1 fixture's codeFlows byte-match (covered by the full byte-match of the d47/d51
//!     positives, which carry codeFlows).

use std::path::PathBuf;

use al_sem::engine::gate::filter::Scope;
use al_sem::engine::gate::run::{AnalyzeArgs, OutputFormat, run_analyze};

use crate::regen;

const PIN_VERSION: &str = "gate-sarif-v1";

/// How a golden's txn slot was produced on the al-sem side.
#[derive(Clone, Copy)]
enum TxnSelection {
    /// `--preset transaction-integrity`.
    Preset,
    /// Explicit opt-in detector(s) — d51 fixtures.
    Detector(&'static str),
}

/// A gate-golden corpus entry — mirrors `dump-gate-sarif.ts` `GATE_SARIF_CORPUS`.
struct GateFixture {
    fixture: &'static str,
    txn: TxnSelection,
}

const CORPUS: &[GateFixture] = &[
    // --- transaction-integrity positives (standard gate preset) ---
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
    // --- d51 ordering fixtures (opt-in; txn slot runs d51 explicitly) ---
    GateFixture {
        fixture: "ws-d51-pos",
        txn: TxnSelection::Detector("d51-retry-side-effect-duplication"),
    },
    GateFixture {
        fixture: "ws-d51-jobqueue",
        txn: TxnSelection::Detector("d51-retry-side-effect-duplication"),
    },
    // --- transaction-integrity negatives (clean = 0 txn results) ---
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
    // --- non-txn positives (default-preset breadth + codeFlows) ---
    GateFixture {
        fixture: "ws-d1-multi-caller",
        txn: TxnSelection::Preset,
    },
    GateFixture {
        fixture: "ws-d14-dead-routine",
        txn: TxnSelection::Preset,
    },
];

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn corpus_dir() -> PathBuf {
    repo_root().join("tests").join("r0-corpus")
}

fn goldens_dir() -> PathBuf {
    repo_root().join("tests").join("gate-goldens")
}

/// Run the gate over one fixture for the given detector selection, returning the SARIF
/// string. The al-sem gate goldens were written from the raw `formatSarif()` string
/// (no trailing newline — `writeFileSync(file, sarifJson)`); the CLI appends "\n" at
/// output time, but the on-disk golden has none, so we compare against the bare string.
fn run_gate(workspace: &str, preset: Option<&str>, detector: Option<&str>) -> String {
    let ws = corpus_dir().join(workspace);
    assert!(
        ws.is_dir(),
        "gate golden for {workspace} has no in-repo fixture at {} (offline corpus incomplete)",
        ws.display()
    );
    let args = AnalyzeArgs {
        workspace: ws.to_string_lossy().to_string(),
        min_severity: None,
        detector: detector.map(|s| s.to_string()),
        preset: preset.map(|s| s.to_string()),
        scope: Scope::Primary,
        limit: None,
        format: OutputFormat::Sarif,
        sarif_version_override: Some(PIN_VERSION.to_string()),
        fail_on: None,
        require_dependencies: false,
        baseline: None,
        update_baseline: false,
        disable_inline_suppression: false,
        group_by: None,
        deterministic: false,
        with_evidence: false,
        single_app: false,
    };
    run_analyze(&args, "engine-default").expect("run_analyze")
}

/// REGEN path (temp-state epoch rebaseline, Task 16). When `REGEN_TEMP_GOLDENS`
/// is set, write the ENGINE-produced SARIF string to the golden file instead of
/// comparing — the goldens are Rust-owned baselines (the TS oracle is retired).
/// Returns `true` when a regen write happened (the caller then skips the assert).
fn maybe_regen(name: &str, rust: &str) -> bool {
    if !regen::regen_mode() {
        return false;
    }
    let path = goldens_dir().join(name);
    std::fs::write(&path, rust).unwrap_or_else(|e| panic!("regen write {}: {e}", path.display()));
    eprintln!("REGEN gate-sarif golden: {}", path.display());
    true
}

/// `gate-goldens/manifest.json`'s `fixtureCount` was read by no test (Task
/// T0.6 — a silently deleted `CORPUS` entry would pass unnoticed). Checks
/// `>=`, not `==`: `fixtureCount` is a frozen al-sem-era provenance floor, not
/// a live inventory that must match exactly forever.
#[test]
fn manifest_fixture_count_floor() {
    let manifest_path = goldens_dir().join("manifest.json");
    let manifest: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(&manifest_path)
            .unwrap_or_else(|e| panic!("read {}: {e}", manifest_path.display())),
    )
    .unwrap_or_else(|e| panic!("{} not valid JSON: {e}", manifest_path.display()));
    let claimed = manifest
        .get("fixtureCount")
        .and_then(|v| v.as_u64())
        .expect("manifest missing fixtureCount") as usize;
    assert!(
        CORPUS.len() >= claimed,
        "gate-goldens/manifest.json claims fixtureCount={claimed} but CORPUS only has {} \
         entries — a fixture may have been silently dropped",
        CORPUS.len()
    );
}

fn read_golden(name: &str) -> String {
    let path = goldens_dir().join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read golden {}: {e}", path.display()))
}

/// Count `runs[0].results`, and collect ruleIds / levels / whether any codeFlows exist.
fn parse_results(sarif: &str) -> (usize, Vec<String>, Vec<String>, bool) {
    let v: serde_json::Value = serde_json::from_str(sarif).expect("SARIF is valid JSON");
    let results = v["runs"][0]["results"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let mut rule_ids = Vec::new();
    let mut levels = Vec::new();
    let mut any_code_flows = false;
    for r in &results {
        if let Some(id) = r["ruleId"].as_str() {
            rule_ids.push(id.to_string());
        }
        if let Some(l) = r["level"].as_str() {
            levels.push(l.to_string());
        }
        if r.get("codeFlows")
            .and_then(|c| c.as_array())
            .map(|a| !a.is_empty())
            .unwrap_or(false)
        {
            any_code_flows = true;
        }
    }
    (results.len(), rule_ids, levels, any_code_flows)
}

#[test]
fn gate_sarif_goldens_byte_match() {
    let mut any_code_flows_matched = false;
    let mut mismatches: Vec<String> = Vec::new();

    for gf in CORPUS {
        // --- (a) txn slot ---
        let (preset, detector): (Option<&str>, Option<&str>) = match gf.txn {
            TxnSelection::Preset => (Some("transaction-integrity"), None),
            TxnSelection::Detector(d) => (None, Some(d)),
        };
        let txn_rust = run_gate(gf.fixture, preset, detector);
        if maybe_regen(&format!("{}.txn.sarif.json", gf.fixture), &txn_rust) {
            // Regen mode: no golden to byte-match against, but the freshly-written
            // content still counts toward the anti-degenerate codeFlows check —
            // otherwise this assertion would ALWAYS fail under REGEN_TEMP_GOLDENS=1
            // regardless of code changes, since every fixture takes this branch.
            let (_, _, _, cf) = parse_results(&txn_rust);
            if cf {
                any_code_flows_matched = true;
            }
            // also regen the default slot, then skip the byte-match asserts for this fixture
            let default_rust = run_gate(gf.fixture, None, None);
            maybe_regen(&format!("{}.default.sarif.json", gf.fixture), &default_rust);
            let (_, _, _, cf) = parse_results(&default_rust);
            if cf {
                any_code_flows_matched = true;
            }
            continue;
        }
        let txn_golden = read_golden(&format!("{}.txn.sarif.json", gf.fixture));
        if txn_rust != txn_golden {
            mismatches.push(format!("{}.txn", gf.fixture));
            report_first_diff(&format!("{}.txn", gf.fixture), &txn_golden, &txn_rust);
        } else {
            let (_, _, _, cf) = parse_results(&txn_rust);
            if cf {
                any_code_flows_matched = true;
            }
        }

        // --- (b) default slot ---
        let default_rust = run_gate(gf.fixture, None, None);
        let default_golden = read_golden(&format!("{}.default.sarif.json", gf.fixture));
        if default_rust != default_golden {
            mismatches.push(format!("{}.default", gf.fixture));
            report_first_diff(
                &format!("{}.default", gf.fixture),
                &default_golden,
                &default_rust,
            );
        } else {
            let (_, _, _, cf) = parse_results(&default_rust);
            if cf {
                any_code_flows_matched = true;
            }
        }
    }

    assert!(
        mismatches.is_empty(),
        "GATE-SARIF differential: {} golden(s) did NOT byte-match: {:?}",
        mismatches.len(),
        mismatches
    );
    assert!(
        any_code_flows_matched,
        "anti-degenerate: expected ≥1 byte-matched golden carrying codeFlows"
    );
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
                "GATE-SARIF mismatch in {label} at line {}:\n  golden: {gl}\n  rust:   {rl}",
                i + 1
            );
            return;
        }
    }
    eprintln!(
        "GATE-SARIF mismatch in {label}: differ only in trailing bytes (length {} vs {})",
        golden.len(),
        rust.len()
    );
}

/// Anti-degenerate: the d47 http-nocommit txn fixture produces EXACTLY 1 SARIF result,
/// ruleId d47-io-unsafe-txn, level error.
#[test]
fn anti_degenerate_d47_single_error_result() {
    let sarif = run_gate(
        "ws-txn-d47-pos-http-nocommit",
        Some("transaction-integrity"),
        None,
    );
    let (count, rule_ids, levels, code_flows) = parse_results(&sarif);
    assert_eq!(count, 1, "expected exactly 1 result for d47 http-nocommit");
    assert_eq!(rule_ids, vec!["d47-io-unsafe-txn".to_string()]);
    assert_eq!(levels, vec!["error".to_string()]);
    assert!(code_flows, "the d47 result must carry codeFlows");
}

/// Anti-degenerate: a clean fixture produces 0 SARIF results.
#[test]
fn anti_degenerate_clean_fixture_zero_results() {
    let sarif = run_gate("ws-txn-d46-neg", Some("transaction-integrity"), None);
    let (count, _, _, _) = parse_results(&sarif);
    assert_eq!(
        count, 0,
        "expected 0 results for the clean d46 negative fixture"
    );
}

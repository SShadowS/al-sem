//! Stage A3 — `--format terminal` byte-parity differential test.
//!
//! For each fixture in the terminal corpus, runs the Rust gate pipeline under
//! `--format terminal --deterministic` and byte-compares the output against
//! the vendored (Rust-owned) goldens at:
//!   `tests/cli-a-goldens/terminal/<fixture>.<variant>.txt`
//! (originally sourced from al-sem's `scripts/cli-a-goldens/terminal/`, now retired)
//!
//! Three variant families:
//!   - `plain`   — every fixture (21 including ws-rollup-multi-detector).
//!   - `nodep`   — ws-rollup-multi-detector only (same output, locks the code path).
//!   - `groupby-<key>` — five goldens for ws-d1-multi-caller.
//!
//! ## Acceptance gate
//! All 27 goldens MUST byte-match; a divergence is a bug to fix, not something
//! to tolerate.
//!
//! ## Refresh
//! Goldens are Rust-owned baselines (the al-sem TS oracle is retired).
//! Rebaseline with `REGEN_TEMP_GOLDENS=1 cargo test --test cli cli_a_terminal_differential::`.

use std::path::PathBuf;
use std::sync::Mutex;

use al_sem::engine::gate::filter::Scope;
use al_sem::engine::gate::run::{AnalyzeArgs, OutputFormat, run_analyze_with_exit};

use crate::regen;

const TEST_NAME: &str = "cli_a_terminal_differential";

/// Version override — terminal output doesn't embed a version, but set it for
/// consistency with the dump script env.
const TERMINAL_VERSION_OVERRIDE: &str = "cli-a-json-v1";

/// Serialises tests that mutate the process-global env var.
static ENV_LOCK: Mutex<()> = Mutex::new(());

/// al-sem `DEFAULT_DETECTORS` names, in al-sem's declaration order.
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
    "d16-obsolete-routine-call",
    "d17-min-version-drift",
    "d18-constant-filter-in-loop",
    "d19-unused-parameter",
    "d20-unreachable-after-exit",
    "d21-read-without-load",
    "d22-flowfield-without-calcfields",
    "d29-subscriber-modify-on-event-record",
    "d32-constant-boolean-parameter",
    "d33-unfiltered-bulk-write",
    "d34-commit-in-loop",
    "d35-commit-in-event-subscriber",
    "d36-late-setloadfields",
    "d37-validate-without-persist",
    "d38-subscriber-to-obsolete-event",
    "d39-record-left-dirty-across-chain",
    "d41-transitive-filter-loss",
    "d42-cross-call-wrong-setloadfields",
    "d43-event-ishandled-skip",
    "d44-event-multi-subscriber-overlap",
    "d45-event-transitive-table-exposure",
];

/// The 20 standard fixtures (same corpus as stats / JSON differentials).
const PLAIN_FIXTURES: &[&str] = &[
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
    // The rollup fixture — plain variant
    "ws-rollup-multi-detector",
];

const GROUP_BY_KEYS: &[&str] = &["object", "routine", "table", "detector", "file"];
const GROUP_BY_FIXTURE: &str = "ws-d1-multi-caller";

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn corpus_dir() -> PathBuf {
    repo_root().join("tests").join("r0-corpus")
}

/// In-repo vendored cli-a terminal golden corpus — the SOLE source. Originally
/// only the rebaselined-vs-al-sem fixtures lived here with the rest falling back
/// to a frozen al-sem archive; that fallback was retired (Task 3.6, al-sem parity
/// retirement) once the corpus was fully vendored, since the fallback checked a
/// path that no longer exists on disk and silently skipped every assertion.
fn local_terminal_dir() -> PathBuf {
    repo_root()
        .join("tests")
        .join("cli-a-goldens")
        .join("terminal")
}

/// Resolve a golden by name against the in-repo vendored corpus.
fn resolve_golden(name: &str) -> PathBuf {
    local_terminal_dir().join(name)
}

fn detector_arg(names: &[&str]) -> String {
    names.join(",")
}

/// REGEN path (temp-state epoch rebaseline, Task 16; iter-2 gap rebaseline).
/// When `REGEN_TEMP_GOLDENS` is set, reconcile the golden against the ENGINE output
/// — the goldens are Rust-owned baselines (TS oracle retired). The write target is
/// the in-repo vendored dir (`local_terminal_dir()/<name>`); we write ONLY when the
/// engine output differs from the existing baseline, leaving an already-matching
/// golden untouched. Returns `true` in regen mode.
fn maybe_regen(name: &str, rust: &str) -> bool {
    if !regen::regen_mode() {
        return false;
    }
    let resolved = resolve_golden(name);
    let baseline = std::fs::read_to_string(&resolved).ok();
    if baseline.as_deref() == Some(rust) {
        return true; // already byte-matches the resolved baseline — no vendoring needed
    }
    let dir = local_terminal_dir();
    std::fs::create_dir_all(&dir).unwrap_or_else(|e| panic!("regen mkdir {}: {e}", dir.display()));
    let golden_path = dir.join(name);
    std::fs::write(&golden_path, rust)
        .unwrap_or_else(|e| panic!("regen write {}: {e}", golden_path.display()));
    eprintln!(
        "REGEN cli-a-terminal vendored golden: {}",
        golden_path.display()
    );
    true
}

/// Run the Rust terminal pipeline for one fixture.
/// `group_by` is `None` for plain/nodep, `Some("object")` etc. for group-by.
fn run_terminal(fixture: &str, detector_csv: &str, group_by: Option<&str>) -> String {
    let fixture_dir = corpus_dir().join(fixture);
    assert!(
        fixture_dir.is_dir(),
        "{TEST_NAME}: fixture {fixture} not found at {}",
        fixture_dir.display()
    );
    let args = AnalyzeArgs {
        workspace: fixture_dir.to_string_lossy().to_string(),
        min_severity: None,
        detector: Some(detector_csv.to_string()),
        preset: None,
        scope: Scope::Primary,
        limit: None,
        format: OutputFormat::Terminal,
        sarif_version_override: None,
        fail_on: None,
        require_dependencies: false,
        baseline: None,
        update_baseline: false,
        disable_inline_suppression: false,
        group_by: group_by.map(|s| s.to_string()),
        deterministic: true,
        with_evidence: false,
        single_app: false,
    };
    match run_analyze_with_exit(&args, "engine-default") {
        Ok((out, _, _)) => format!("{out}\n"),
        Err(e) => panic!("{TEST_NAME}: run_analyze failed for {fixture}: {e}"),
    }
}

/// Produce a human-readable diff (first differing line).
fn text_diff(fixture: &str, slot: &str, golden: &str, rust: &str) -> String {
    let gl: Vec<&str> = golden.lines().collect();
    let rl: Vec<&str> = rust.lines().collect();
    for (i, (g, r)) in gl.iter().zip(rl.iter()).enumerate() {
        if g != r {
            return format!(
                "[{fixture}/{slot}] line {} mismatch:\n  golden: {g:?}\n  rust:   {r:?}",
                i + 1
            );
        }
    }
    if gl.len() != rl.len() {
        return format!(
            "[{fixture}/{slot}] length mismatch: golden {} lines, rust {} lines",
            gl.len(),
            rl.len()
        );
    }
    format!("[{fixture}/{slot}] byte mismatch (no line-level diff found)")
}

// ---------------------------------------------------------------------------
// Main byte-match test
// ---------------------------------------------------------------------------

#[test]
fn cli_a_terminal_byte_match() {
    let _env = crate::env_guard();
    let default_csv = detector_arg(DEFAULT_DETECTOR_NAMES);
    let mut divergences: Vec<String> = Vec::new();

    let _guard = ENV_LOCK.lock().unwrap();
    // TODO: Audit that the environment access only happens in single-threaded code.
    unsafe { std::env::set_var("ALCH_DRIVER_VERSION_OVERRIDE", TERMINAL_VERSION_OVERRIDE) };

    // --- plain goldens (21 fixtures) ---
    for &fixture in PLAIN_FIXTURES {
        let name = format!("{fixture}.plain.txt");
        let golden_path = resolve_golden(&name);
        let rust_out = run_terminal(fixture, &default_csv, None);
        if maybe_regen(&name, &rust_out) {
            continue;
        }
        if !golden_path.exists() {
            divergences.push(format!(
                "[{fixture}/plain] golden file missing: {}",
                golden_path.display()
            ));
            continue;
        }
        let golden = std::fs::read_to_string(&golden_path)
            .unwrap_or_else(|e| panic!("{TEST_NAME}: read error {}: {e}", golden_path.display()));
        if rust_out != golden {
            divergences.push(text_diff(fixture, "plain", &golden, &rust_out));
        }
    }

    // --- nodep golden (ws-rollup-multi-detector) ---
    {
        let fixture = "ws-rollup-multi-detector";
        let name = format!("{fixture}.nodep.txt");
        let golden_path = resolve_golden(&name);
        // nodep = same default detectors, no special flag (no external deps in fixture)
        let rust_out = run_terminal(fixture, &default_csv, None);
        if !maybe_regen(&name, &rust_out) {
            if !golden_path.exists() {
                divergences.push(format!(
                    "[{fixture}/nodep] golden file missing: {}",
                    golden_path.display()
                ));
            } else {
                let golden = std::fs::read_to_string(&golden_path).unwrap_or_else(|e| {
                    panic!("{TEST_NAME}: read error {}: {e}", golden_path.display())
                });
                if rust_out != golden {
                    divergences.push(text_diff(fixture, "nodep", &golden, &rust_out));
                }
            }
        }
    }

    // --- group-by goldens (5 × ws-d1-multi-caller) ---
    for &by in GROUP_BY_KEYS {
        let name = format!("{GROUP_BY_FIXTURE}.groupby-{by}.txt");
        let golden_path = resolve_golden(&name);
        let rust_out = run_terminal(GROUP_BY_FIXTURE, &default_csv, Some(by));
        if maybe_regen(&name, &rust_out) {
            continue;
        }
        if !golden_path.exists() {
            divergences.push(format!(
                "[{GROUP_BY_FIXTURE}/groupby-{by}] golden file missing: {}",
                golden_path.display()
            ));
            continue;
        }
        let golden = std::fs::read_to_string(&golden_path)
            .unwrap_or_else(|e| panic!("{TEST_NAME}: read error {}: {e}", golden_path.display()));
        if rust_out != golden {
            divergences.push(text_diff(
                GROUP_BY_FIXTURE,
                &format!("groupby-{by}"),
                &golden,
                &rust_out,
            ));
        }
    }

    // TODO: Audit that the environment access only happens in single-threaded code.
    unsafe { std::env::remove_var("ALCH_DRIVER_VERSION_OVERRIDE") };

    if !divergences.is_empty() {
        let mut msg = format!("{TEST_NAME}: {} divergence(s) found:\n", divergences.len());
        for d in &divergences {
            msg.push_str(&format!("  {d}\n"));
        }
        panic!("{msg}");
    }
}

// ---------------------------------------------------------------------------
// Anti-degenerate oracles
// ---------------------------------------------------------------------------

/// ws-txn-d46-neg (canonical 0-findings fixture) must emit the "No findings." line.
#[test]
fn zero_findings_fixture_shows_no_findings() {
    let _env = crate::env_guard();
    let default_csv = detector_arg(DEFAULT_DETECTOR_NAMES);
    let _guard = ENV_LOCK.lock().unwrap();
    // TODO: Audit that the environment access only happens in single-threaded code.
    unsafe { std::env::set_var("ALCH_DRIVER_VERSION_OVERRIDE", TERMINAL_VERSION_OVERRIDE) };
    let out = run_terminal("ws-txn-d46-neg", &default_csv, None);
    // TODO: Audit that the environment access only happens in single-threaded code.
    unsafe { std::env::remove_var("ALCH_DRIVER_VERSION_OVERRIDE") };
    assert!(
        out.contains("No findings."),
        "zero-findings fixture must contain 'No findings.' but got:\n{out}"
    );
}

/// ws-rollup-multi-detector must contain "3 detectors agree:" in its plain golden.
#[test]
fn rollup_fixture_has_3_detectors_agree() {
    let _env = crate::env_guard();
    let default_csv = detector_arg(DEFAULT_DETECTOR_NAMES);
    let _guard = ENV_LOCK.lock().unwrap();
    // TODO: Audit that the environment access only happens in single-threaded code.
    unsafe { std::env::set_var("ALCH_DRIVER_VERSION_OVERRIDE", TERMINAL_VERSION_OVERRIDE) };
    let out = run_terminal("ws-rollup-multi-detector", &default_csv, None);
    // TODO: Audit that the environment access only happens in single-threaded code.
    unsafe { std::env::remove_var("ALCH_DRIVER_VERSION_OVERRIDE") };
    assert!(
        out.contains("3 detectors agree:"),
        "rollup fixture must contain '3 detectors agree:' but got:\n{out}"
    );
}

/// group-by output for ws-d1-multi-caller must contain "Grouped by detector".
#[test]
fn group_by_detector_contains_header() {
    let _env = crate::env_guard();
    let default_csv = detector_arg(DEFAULT_DETECTOR_NAMES);
    let _guard = ENV_LOCK.lock().unwrap();
    // TODO: Audit that the environment access only happens in single-threaded code.
    unsafe { std::env::set_var("ALCH_DRIVER_VERSION_OVERRIDE", TERMINAL_VERSION_OVERRIDE) };
    let out = run_terminal(GROUP_BY_FIXTURE, &default_csv, Some("detector"));
    // TODO: Audit that the environment access only happens in single-threaded code.
    unsafe { std::env::remove_var("ALCH_DRIVER_VERSION_OVERRIDE") };
    assert!(
        out.contains("Grouped by detector"),
        "group-by output must contain 'Grouped by detector' but got:\n{out}"
    );
}

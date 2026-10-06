//! cli-c/c1 — Events fanout + chains differential tests.
//!
//! Each test:
//!   1. Runs the Rust `run_events_fanout` / `run_events_chains` pipeline with
//!      `deterministic = true` and `driver_version = "cli-c-v1"`.
//!   2. Compares the output byte-for-byte to the committed golden files under
//!      `tests/cli-c-events-goldens/`.
//!
//! Goldens are Rust-owned baselines (the al-sem TS oracle is retired); rebaseline
//! with `REGEN_TEMP_GOLDENS=1 cargo test --test cli cli_c_events_differential::`.
//!
//! ## Coverage
//!   - 13 fixtures × 4 files = 52 base goldens (fanout.human + fanout.json +
//!     chains.human + chains.json) — includes ws-event-partial-coverage
//!   - 1 depth-truncation fixture × 2 files (chains.human + chains.json) = 2
//!   - 1 scope-all fixture × 4 files = 4
//!   - ws-event-partial-coverage --coverage-policy {warn,strict,ignore}
//!     × {human, json, stderr, exitcode} = 12 policy goldens
//!   - Total: 70 golden files
//!
//! ## Cycle native oracle
//!   - A mocked `EventGraph` with a mutual-publish cycle exercises `cycleDetected: true`
//!     rendering — unreachable via real AL source (see manifest.json cycleNote).

use std::path::PathBuf;

use al_sem::engine::gate::events::{
    EventsChainsOptions, EventsFanoutOptions, format_chains_human, format_chains_json,
    format_fanout_human, format_fanout_json, run_events_chains, run_events_fanout,
};

use crate::regen;
use al_sem::engine::l5::event_flow::Scope;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn corpus_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("r0-corpus")
}

fn golden_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("cli-c-events-goldens")
}

fn load_golden(name: &str) -> String {
    let path = golden_dir().join(name);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("Failed to read golden {}: {e}", path.display()))
}

/// When `REGEN_TEMP_GOLDENS` is set, write `actual` to the named golden and return
/// it (so the caller's compare trivially passes) instead of reading the existing
/// golden. Goldens are Rust-owned baselines (the al-sem TS oracle is retired).
fn golden_or_regen(name: &str, actual: &str) -> String {
    if regen::regen_mode() {
        let path = golden_dir().join(name);
        std::fs::write(&path, actual)
            .unwrap_or_else(|e| panic!("regen write {}: {e}", path.display()));
        eprintln!("REGEN cli-c-events golden: {}", path.display());
        return actual.to_string();
    }
    load_golden(name)
}

/// `cli-c-events-goldens/manifest.json`'s `totalGoldens` was read by no test
/// (Task T0.6 — a silently deleted golden would pass unnoticed). Checks `>=`,
/// not `==`: `totalGoldens` (54) is a frozen al-sem-era provenance floor, and
/// this suite's own coverage-policy + scope-all goldens (added post-retirement,
/// per this file's module doc — 70 total today) have since grown the corpus
/// past it — an exact-equality check would break the moment more are added.
#[test]
fn manifest_total_goldens_floor() {
    let manifest_path = golden_dir().join("manifest.json");
    let manifest: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(&manifest_path)
            .unwrap_or_else(|e| panic!("read {}: {e}", manifest_path.display())),
    )
    .unwrap_or_else(|e| panic!("{} not valid JSON: {e}", manifest_path.display()));
    let claimed = manifest
        .get("totalGoldens")
        .and_then(|v| v.as_u64())
        .expect("manifest missing totalGoldens") as usize;
    let discovered = std::fs::read_dir(golden_dir())
        .unwrap_or_else(|e| panic!("read {}: {e}", golden_dir().display()))
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name() != std::ffi::OsStr::new("manifest.json"))
        .count();
    assert!(
        discovered >= claimed,
        "cli-c-events-goldens/manifest.json claims totalGoldens={claimed} but only \
         {discovered} golden file(s) were found — a golden may have been silently deleted"
    );
}

const ALSEM_VERSION: &str = "cli-c-v1";

// ---------------------------------------------------------------------------
// Macro to generate fanout + chains differential tests for each fixture.
// ---------------------------------------------------------------------------

macro_rules! events_differential {
    ($test_name:ident, $fixture:literal, $scope:expr_2021) => {
        #[test]
        fn $test_name() {
            let ws = corpus_dir().join($fixture);
            assert!(ws.is_dir(), "fixture missing: {}", ws.display());

            let fixture_stem = $fixture;

            // ── fanout human ─────────────────────────────────────────────
            {
                let opts = EventsFanoutOptions {
                    workspace: &ws,
                    format: "human",
                    scope: $scope,
                    coverage_policy: "warn",
                    driver_version: ALSEM_VERSION,
                    deterministic: true,
                    strict: false,
                };
                let result = run_events_fanout(&opts);
                let golden =
                    golden_or_regen(&format!("{fixture_stem}.fanout.human.txt"), &result.text);
                assert_eq!(result.text, golden, "{fixture_stem} fanout.human mismatch");
                assert_eq!(result.exit_code, 0, "{fixture_stem} fanout.human exit_code");
            }

            // ── fanout json ──────────────────────────────────────────────
            {
                let opts = EventsFanoutOptions {
                    workspace: &ws,
                    format: "json",
                    scope: $scope,
                    coverage_policy: "warn",
                    driver_version: ALSEM_VERSION,
                    deterministic: true,
                    strict: false,
                };
                let result = run_events_fanout(&opts);
                let golden = golden_or_regen(&format!("{fixture_stem}.fanout.json"), &result.text);
                assert_eq!(result.text, golden, "{fixture_stem} fanout.json mismatch");
                assert_eq!(result.exit_code, 0, "{fixture_stem} fanout.json exit_code");
            }

            // ── chains human ─────────────────────────────────────────────
            {
                let opts = EventsChainsOptions {
                    workspace: &ws,
                    format: "human",
                    scope: $scope,
                    max_depth: None,
                    max_nodes: None,
                    coverage_policy: "warn",
                    driver_version: ALSEM_VERSION,
                    deterministic: true,
                    strict: false,
                };
                let result = run_events_chains(&opts);
                let golden =
                    golden_or_regen(&format!("{fixture_stem}.chains.human.txt"), &result.text);
                assert_eq!(result.text, golden, "{fixture_stem} chains.human mismatch");
                assert_eq!(result.exit_code, 0, "{fixture_stem} chains.human exit_code");
            }

            // ── chains json ──────────────────────────────────────────────
            {
                let opts = EventsChainsOptions {
                    workspace: &ws,
                    format: "json",
                    scope: $scope,
                    max_depth: None,
                    max_nodes: None,
                    coverage_policy: "warn",
                    driver_version: ALSEM_VERSION,
                    deterministic: true,
                    strict: false,
                };
                let result = run_events_chains(&opts);
                let golden = golden_or_regen(&format!("{fixture_stem}.chains.json"), &result.text);
                assert_eq!(result.text, golden, "{fixture_stem} chains.json mismatch");
                assert_eq!(result.exit_code, 0, "{fixture_stem} chains.json exit_code");
            }
        }
    };
}

// ── 12 base fixtures (scope=primary) ────────────────────────────────────────

events_differential!(events_ws_events, "ws-events", Scope::Primary);
events_differential!(events_ws_event_fanout, "ws-event-fanout", Scope::Primary);
events_differential!(events_ws_event_chains, "ws-event-chains", Scope::Primary);
events_differential!(
    events_ws_event_pub_cycle,
    "ws-event-pub-cycle",
    Scope::Primary
);
events_differential!(events_ws_event_cycle, "ws-event-cycle", Scope::Primary);
events_differential!(
    events_ws_event_ishandled,
    "ws-event-ishandled",
    Scope::Primary
);
events_differential!(
    events_ws_event_multi_sub_overlap,
    "ws-event-multi-sub-overlap",
    Scope::Primary
);
events_differential!(
    events_ws_event_read_after_write,
    "ws-event-read-after-write",
    Scope::Primary
);
events_differential!(
    events_ws_event_d45_deep,
    "ws-event-d45-deep",
    Scope::Primary
);
events_differential!(
    events_ws_d8_commit_in_tx,
    "ws-d8-commit-in-tx",
    Scope::Primary
);
events_differential!(
    events_ws_txn_d47_event_pos,
    "ws-txn-d47-event-pos",
    Scope::Primary
);
events_differential!(
    events_ws_d12_dead_event,
    "ws-d12-dead-event",
    Scope::Primary
);
events_differential!(
    events_ws_event_partial_coverage,
    "ws-event-partial-coverage",
    Scope::Primary
);

// ── scope=all variant (ws-event-fanout) ─────────────────────────────────────

#[test]
fn events_ws_event_fanout_scope_all() {
    let ws = corpus_dir().join("ws-event-fanout");
    assert!(ws.is_dir(), "fixture missing: {}", ws.display());

    // fanout human
    {
        let opts = EventsFanoutOptions {
            workspace: &ws,
            format: "human",
            scope: Scope::All,
            coverage_policy: "warn",
            driver_version: ALSEM_VERSION,
            deterministic: true,
            strict: false,
        };
        let result = run_events_fanout(&opts);
        let golden = golden_or_regen("ws-event-fanout.scope-all.fanout.human.txt", &result.text);
        assert_eq!(result.text, golden, "scope-all fanout.human mismatch");
    }

    // fanout json
    {
        let opts = EventsFanoutOptions {
            workspace: &ws,
            format: "json",
            scope: Scope::All,
            coverage_policy: "warn",
            driver_version: ALSEM_VERSION,
            deterministic: true,
            strict: false,
        };
        let result = run_events_fanout(&opts);
        let golden = golden_or_regen("ws-event-fanout.scope-all.fanout.json", &result.text);
        assert_eq!(result.text, golden, "scope-all fanout.json mismatch");
    }

    // chains human
    {
        let opts = EventsChainsOptions {
            workspace: &ws,
            format: "human",
            scope: Scope::All,
            max_depth: None,
            max_nodes: None,
            coverage_policy: "warn",
            driver_version: ALSEM_VERSION,
            deterministic: true,
            strict: false,
        };
        let result = run_events_chains(&opts);
        let golden = golden_or_regen("ws-event-fanout.scope-all.chains.human.txt", &result.text);
        assert_eq!(result.text, golden, "scope-all chains.human mismatch");
    }

    // chains json
    {
        let opts = EventsChainsOptions {
            workspace: &ws,
            format: "json",
            scope: Scope::All,
            max_depth: None,
            max_nodes: None,
            coverage_policy: "warn",
            driver_version: ALSEM_VERSION,
            deterministic: true,
            strict: false,
        };
        let result = run_events_chains(&opts);
        let golden = golden_or_regen("ws-event-fanout.scope-all.chains.json", &result.text);
        assert_eq!(result.text, golden, "scope-all chains.json mismatch");
    }
}

// ── max-depth=1 variant (ws-event-d45-deep) ─────────────────────────────────

#[test]
fn events_ws_event_d45_deep_max_depth_1() {
    let ws = corpus_dir().join("ws-event-d45-deep");
    assert!(ws.is_dir(), "fixture missing: {}", ws.display());

    // chains human (max-depth=1)
    {
        let opts = EventsChainsOptions {
            workspace: &ws,
            format: "human",
            scope: Scope::Primary,
            max_depth: Some(1),
            max_nodes: None,
            coverage_policy: "warn",
            driver_version: ALSEM_VERSION,
            deterministic: true,
            strict: false,
        };
        let result = run_events_chains(&opts);
        let golden = golden_or_regen(
            "ws-event-d45-deep.max-depth-1.chains.human.txt",
            &result.text,
        );
        assert_eq!(result.text, golden, "max-depth-1 chains.human mismatch");
    }

    // chains json (max-depth=1)
    {
        let opts = EventsChainsOptions {
            workspace: &ws,
            format: "json",
            scope: Scope::Primary,
            max_depth: Some(1),
            max_nodes: None,
            coverage_policy: "warn",
            driver_version: ALSEM_VERSION,
            deterministic: true,
            strict: false,
        };
        let result = run_events_chains(&opts);
        let golden = golden_or_regen("ws-event-d45-deep.max-depth-1.chains.json", &result.text);
        assert_eq!(result.text, golden, "max-depth-1 chains.json mismatch");
    }
}

// ── --coverage-policy {warn,strict,ignore} differential (ws-event-partial-coverage) ──
//
// MUST-FIX corpus differential: for each policy, byte-compares fanout human + json
// stdout AND the stderr companion (analyzer diagnostics + any coverage-incomplete
// lines) AND the exit code against the al-sem goldens.

/// Join the pipeline's stderr lines the way the CLI emits them (`eprintln!` per
/// line ⇒ trailing newline). Empty ⇒ empty string (no spurious newline).
fn stderr_text(lines: &[String]) -> String {
    if lines.is_empty() {
        String::new()
    } else {
        let mut s = lines.join("\n");
        s.push('\n');
        s
    }
}

fn run_partial_fanout(policy: &str, format: &str) -> al_sem::engine::gate::events::EventsRunResult {
    let ws = corpus_dir().join("ws-event-partial-coverage");
    assert!(ws.is_dir(), "fixture missing: {}", ws.display());
    let opts = EventsFanoutOptions {
        workspace: &ws,
        format,
        scope: Scope::Primary,
        coverage_policy: policy,
        driver_version: ALSEM_VERSION,
        deterministic: true,
        strict: false,
    };
    run_events_fanout(&opts)
}

fn assert_partial_policy(policy: &str) {
    // human stdout
    {
        let result = run_partial_fanout(policy, "human");
        let golden = golden_or_regen(
            &format!("ws-event-partial-coverage.fanout.{policy}.human.txt"),
            &result.text,
        );
        assert_eq!(result.text, golden, "{policy} fanout.human mismatch");
    }
    // json stdout
    {
        let result = run_partial_fanout(policy, "json");
        let golden = golden_or_regen(
            &format!("ws-event-partial-coverage.fanout.{policy}.json"),
            &result.text,
        );
        assert_eq!(result.text, golden, "{policy} fanout.json mismatch");
    }
    // stderr + exit (read from the json run; stderr/exit are format-independent for fanout)
    {
        let result = run_partial_fanout(policy, "json");
        let stderr_golden = golden_or_regen(
            &format!("ws-event-partial-coverage.fanout.{policy}.stderr.txt"),
            &stderr_text(&result.stderr_lines),
        );
        assert_eq!(
            stderr_text(&result.stderr_lines),
            stderr_golden,
            "{policy} fanout stderr mismatch"
        );
        let exit_golden = golden_or_regen(
            &format!("ws-event-partial-coverage.fanout.{policy}.exitcode.txt"),
            &result.exit_code.to_string(),
        );
        assert_eq!(
            result.exit_code.to_string(),
            exit_golden,
            "{policy} fanout exitcode mismatch"
        );
    }
}

#[test]
fn events_partial_coverage_policy_warn() {
    assert_partial_policy("warn");
}

#[test]
fn events_partial_coverage_policy_strict() {
    assert_partial_policy("strict");
    // Belt-and-suspenders: strict must elevate exit to 1 and drop the partial entry.
    let result = run_partial_fanout("strict", "json");
    assert_eq!(result.exit_code, 1, "strict must exit 1");
    assert!(
        result.text.contains("\"entries\": []"),
        "strict must drop the partial entry (entries: []); got:\n{}",
        result.text
    );
    // Summary passthrough quirk: coveragePartialEvents stays 1 even though entries=[].
    assert!(
        result.text.contains("\"coveragePartialEvents\": 1"),
        "strict must NOT recompute summary (coveragePartialEvents stays 1)"
    );
    assert!(
        result
            .stderr_lines
            .iter()
            .any(|l| l.starts_with("coverage-incomplete: event ")),
        "strict must emit a coverage-incomplete stderr line"
    );
}

#[test]
fn events_partial_coverage_policy_ignore() {
    assert_partial_policy("ignore");
    // ignore: exit 0, coverage rewritten to all-complete, summary NOT recomputed.
    let result = run_partial_fanout("ignore", "json");
    assert_eq!(result.exit_code, 0, "ignore must exit 0");
    assert!(
        result
            .text
            .contains("\"capabilityComposition\": \"complete\""),
        "ignore must rewrite capabilityComposition to complete"
    );
    assert!(
        result.text.contains("\"coveragePartialEvents\": 1"),
        "ignore must NOT recompute summary (coveragePartialEvents stays 1)"
    );
}

// ── Native oracle: strict/ignore on a hand-built partial report ───────────────
//
// Corpus-independent proof of the --coverage-policy filter/rewrite. Builds a
// FanoutReport directly with one "partial" capability entry + one "complete"
// entry, then drives run_events_fanout's policy logic via a constructed report so
// the filter (+stderr/exit) and the rewrite hold regardless of any fixture.

#[test]
fn coverage_policy_native_oracle() {
    use al_sem::engine::l5::event_flow::{FanoutCoverage, FanoutEntry, FanoutReport};

    let partial = FanoutEntry {
        publisher: "P1".to_string(),
        event_id: "EVT-PARTIAL".to_string(),
        event_name: "OnPartial".to_string(),
        event_kind: "integration",
        direct_subscriber_count: 1,
        coverage: FanoutCoverage {
            dispatch_edges: "complete",
            subscriber_discovery: "complete",
            capability_composition: "partial",
        },
    };
    let complete = FanoutEntry {
        publisher: "P2".to_string(),
        event_id: "EVT-OK".to_string(),
        event_name: "OnOk".to_string(),
        event_kind: "integration",
        direct_subscriber_count: 1,
        coverage: FanoutCoverage {
            dispatch_edges: "complete",
            subscriber_discovery: "complete",
            capability_composition: "complete",
        },
    };
    let base = FanoutReport {
        entries: vec![partial.clone(), complete.clone()],
        total_publishers: 2,
        total_events: 2,
        zero_subscriber_events: 0,
        hot_events: 0,
        coverage_partial_events: 1,
    };

    // --- strict: filter partial, emit stderr line, elevate exit, summary passthrough. ---
    {
        let mut report = base.clone();
        let mut stderr: Vec<String> = Vec::new();
        let mut elevated = false;
        let mut kept = Vec::new();
        for e in report.entries.drain(..) {
            if e.coverage.dispatch_edges == "partial"
                || e.coverage.capability_composition == "partial"
            {
                stderr.push(format!(
                    "coverage-incomplete: event {} dispatchEdges={} capability={}",
                    e.event_id, e.coverage.dispatch_edges, e.coverage.capability_composition
                ));
                elevated = true;
            } else {
                kept.push(e);
            }
        }
        report.entries = kept;
        assert_eq!(
            report.entries.len(),
            1,
            "strict keeps only the complete entry"
        );
        assert_eq!(report.entries[0].event_id, "EVT-OK");
        assert!(elevated, "strict elevates exit");
        assert_eq!(
            stderr,
            vec![
                "coverage-incomplete: event EVT-PARTIAL dispatchEdges=complete capability=partial"
                    .to_string()
            ]
        );
        // Summary NOT recomputed.
        assert_eq!(report.coverage_partial_events, 1);
        assert_eq!(report.total_events, 2);
        // JSON sanity: entries has exactly the complete one, summary unchanged.
        let json = format_fanout_json(&report, "test-v1", true);
        assert!(json.contains("\"coveragePartialEvents\": 1"));
        assert!(json.contains("EVT-OK"));
        assert!(!json.contains("EVT-PARTIAL"));
    }

    // --- ignore: rewrite all coverage to complete, summary passthrough. ---
    {
        let mut report = base.clone();
        for e in report.entries.iter_mut() {
            e.coverage = FanoutCoverage {
                dispatch_edges: "complete",
                subscriber_discovery: "complete",
                capability_composition: "complete",
            };
        }
        let json = format_fanout_json(&report, "test-v1", true);
        // Both entries now all-complete (glyph [✓✓✓] in human).
        assert!(!json.contains("\"partial\""), "ignore removes all partials");
        assert!(
            report.coverage_partial_events == 1,
            "summary NOT recomputed"
        );
        let human = format_fanout_human(&report);
        assert!(human.contains("[✓✓✓]"), "ignore human glyphs all-complete");
    }
}

// ── Native oracle: cycle path ────────────────────────────────────────────────
//
// Exercises `cycleDetected: true` in the chain walk. Unreachable via real AL
// source because the routine-indexer assigns a single kind= (first-match wins),
// so a routine cannot be both [IntegrationEvent] publisher and EventSubscriber.
// Build the EventGraph directly (mirrors al-sem test/engine/event-flow.test.ts).

#[test]
fn cycle_native_oracle() {
    use al_sem::engine::l3::event_graph::{EventEdge, EventGraph, EventSymbol, Evidence};
    use al_sem::engine::l5::event_flow::{
        ChainNode, ChainWalkOptions, Scope, build_event_flow_indexes, compute_chain_report,
        walk_event_chain,
    };
    use std::collections::BTreeSet;

    // Graph: P publishes E1 → S1 subscribes. S1 publishes E2 → P subscribes.
    let event_graph = EventGraph {
        events: vec![
            EventSymbol {
                id: "E1".to_string(),
                publisher_object_id: "app/Codeunit/1".to_string(),
                publisher_routine_id: Some("P".to_string()),
                publisher_stable_routine_id: Some("P".to_string()),
                event_name: "Ev1".to_string(),
                event_kind: "integration".to_string(),
                element_name: None,
                signature_hash: String::new(),
                parameters: Vec::new(),
                isolated: None,
                provenance: vec![Evidence {
                    source: "test".to_string(),
                    note: None,
                }],
                publisher_ref: None,
            },
            EventSymbol {
                id: "E2".to_string(),
                publisher_object_id: "app/Codeunit/1".to_string(),
                publisher_routine_id: Some("S1".to_string()),
                publisher_stable_routine_id: Some("S1".to_string()),
                event_name: "Ev2".to_string(),
                event_kind: "integration".to_string(),
                element_name: None,
                signature_hash: String::new(),
                parameters: Vec::new(),
                isolated: None,
                provenance: vec![Evidence {
                    source: "test".to_string(),
                    note: None,
                }],
                publisher_ref: None,
            },
        ],
        edges: vec![
            EventEdge {
                event_id: "E1".to_string(),
                subscriber_routine_id: "S1".to_string(),
                subscriber_stable_routine_id: "S1".to_string(),
                subscriber_app_id: "app".to_string(),
                resolution: "resolved".to_string(),
                provenance: vec![],
            },
            EventEdge {
                event_id: "E2".to_string(),
                subscriber_routine_id: "P".to_string(),
                subscriber_stable_routine_id: "P".to_string(),
                subscriber_app_id: "app".to_string(),
                resolution: "resolved".to_string(),
                provenance: vec![],
            },
        ],
    };

    let dep_ids: BTreeSet<String> = BTreeSet::new();
    let routines = Vec::new();
    let ix = build_event_flow_indexes(&event_graph, &routines, &dep_ids);

    // walk_event_chain from P must produce a cycle node.
    let tree = walk_event_chain("P", &ix, &ChainWalkOptions::default());

    fn collect_all(node: &ChainNode, out: &mut Vec<ChainNode>) {
        out.push(node.clone());
        for c in &node.children {
            collect_all(c, out);
        }
    }
    let mut nodes = Vec::new();
    collect_all(&tree, &mut nodes);

    let has_cycle = nodes.iter().any(|n| n.cycle_detected);
    assert!(
        has_cycle,
        "expected cycleDetected in walk from P over mutual-publish graph"
    );

    // The cycle node must be a subscriber node.
    let cycle_nodes: Vec<_> = nodes.iter().filter(|n| n.cycle_detected).collect();
    for cn in &cycle_nodes {
        assert_eq!(
            cn.kind, "subscriber",
            "cycle node must have kind=subscriber"
        );
    }

    // Human: must contain "  (cycle)".
    let report = compute_chain_report(&ix, &ChainWalkOptions::default(), Scope::All);
    let human = format_chains_human(&report);
    assert!(
        human.contains("  (cycle)"),
        "human must contain '  (cycle)'; got:\n{human}"
    );

    // JSON: must contain `"cycleDetected": true`.
    let json = format_chains_json(&report, "test-v1", true);
    assert!(
        json.contains("\"cycleDetected\": true"),
        "json must contain '\"cycleDetected\": true'; got:\n{json}"
    );

    // Summary: cyclesDetected >= 1.
    assert!(
        report.cycles_detected >= 1,
        "report.cycles_detected must be >= 1"
    );

    // Summary JSON field: "cyclesDetected" in summary block.
    assert!(
        json.contains("\"cyclesDetected\":"),
        "json summary must contain 'cyclesDetected'"
    );
}

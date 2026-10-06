//! cli-c/c1 — EVENTS CLI pipeline.
//!
//! Ports:
//!   - `src/cli/format-events.ts`   → `format_fanout` / `format_chains`
//!   - `src/cli/events-fanout.ts`   → `run_events_fanout`
//!   - `src/cli/events-chains.ts`   → `run_events_chains`
//!
//! ## JSON envelope
//!
//! The events JSON is an **insertion-order** envelope (NOT the `DocumentEnvelope`
//! which sorts keys alphabetically). Keys are emitted exactly as TS does them via
//! `JSON.stringify({al_sem_version, generated_at, kind, summary, entries|chains}, undefined, 2)`.
//!
//! We build the JSON by hand using a tiny ordered-value tree — no `#[derive(Serialize)]`
//! on the engine structs.
//!
//! ## Scope
//!
//! `--scope primary` (default): keep only entries where the publisher OR any resolved
//! subscriber is a primary (non-dep) routine. `--scope all`: keep everything.
//!
//! ## Determinism
//!
//! `--deterministic` pins `generated_at = "0"` and reads the version override from
//! the caller rather than the live version. Used by every differential test.

use crate::engine::gate::model_instance_id::compute_gate_model_instance_id;
use crate::engine::gate::run::compute_analyzer_diagnostics;
use crate::engine::l3::l3_workspace::assemble_and_resolve_workspace;
use crate::engine::l5::detector_context::build_detector_context;
use crate::engine::l5::detectors::registered_detectors;
use crate::engine::l5::digest_cli::DEFAULT_DETECTOR_NAMES;
use crate::engine::l5::event_flow::{
    ChainNode, ChainReport, ChainWalkOptions, FanoutCoverage, FanoutReport, Scope,
    compute_chain_report, compute_fanout_report,
};

// ---------------------------------------------------------------------------
// JSON insertion-order serializer — the shared `gate::ordered_json` module (one
// source of truth for the cli-c hand-built envelopes; see that module's docs).
// ---------------------------------------------------------------------------

use crate::engine::gate::ordered_json::{Jv, serialize_jv};

// ---------------------------------------------------------------------------
// FanoutEntry → Jv
// ---------------------------------------------------------------------------

fn fanout_entry_to_jv(e: &crate::engine::l5::event_flow::FanoutEntry) -> Jv {
    Jv::Obj(vec![
        ("publisher".to_string(), Jv::s(&e.publisher)),
        ("eventId".to_string(), Jv::s(&e.event_id)),
        ("eventName".to_string(), Jv::s(&e.event_name)),
        ("eventKind".to_string(), Jv::s(e.event_kind)),
        (
            "directSubscriberCount".to_string(),
            Jv::n(e.direct_subscriber_count),
        ),
        (
            "coverage".to_string(),
            Jv::Obj(vec![
                (
                    "dispatchEdges".to_string(),
                    Jv::s(e.coverage.dispatch_edges),
                ),
                (
                    "subscriberDiscovery".to_string(),
                    Jv::s(e.coverage.subscriber_discovery),
                ),
                (
                    "capabilityComposition".to_string(),
                    Jv::s(e.coverage.capability_composition),
                ),
            ]),
        ),
    ])
}

// ---------------------------------------------------------------------------
// ChainNode → Jv
// ---------------------------------------------------------------------------

fn chain_node_to_jv(node: &ChainNode) -> Jv {
    match node.kind {
        "root" => {
            // root → {kind, routineId, children}
            let routine_id = node.routine_id.as_deref().unwrap_or("?");
            Jv::Obj(vec![
                ("kind".to_string(), Jv::s("root")),
                ("routineId".to_string(), Jv::s(routine_id)),
                (
                    "children".to_string(),
                    Jv::Arr(node.children.iter().map(chain_node_to_jv).collect()),
                ),
            ])
        }
        "event-dispatch" => {
            // event-dispatch → {kind, eventId, eventName, children[, depthTruncated:true]}
            let mut pairs: Vec<(String, Jv)> = Vec::new();
            pairs.push(("kind".to_string(), Jv::s("event-dispatch")));
            if let Some(eid) = &node.event_id {
                pairs.push(("eventId".to_string(), Jv::s(eid)));
            }
            if let Some(ename) = &node.event_name {
                pairs.push(("eventName".to_string(), Jv::s(ename)));
            }
            pairs.push((
                "children".to_string(),
                Jv::Arr(node.children.iter().map(chain_node_to_jv).collect()),
            ));
            if node.depth_truncated {
                pairs.push(("depthTruncated".to_string(), Jv::Bool(true)));
            }
            Jv::Obj(pairs)
        }
        "subscriber" => {
            // subscriber → {kind, routineId, children[, cycleDetected:true]}
            // TS `walkEventChain` only sets `depthTruncated` on event-dispatch
            // nodes — never on subscriber nodes — so there is no `depthTruncated`
            // branch here (matching format-events.ts).
            let mut pairs: Vec<(String, Jv)> = Vec::new();
            pairs.push(("kind".to_string(), Jv::s("subscriber")));
            if let Some(rid) = &node.routine_id {
                pairs.push(("routineId".to_string(), Jv::s(rid)));
            }
            pairs.push((
                "children".to_string(),
                Jv::Arr(node.children.iter().map(chain_node_to_jv).collect()),
            ));
            if node.cycle_detected {
                pairs.push(("cycleDetected".to_string(), Jv::Bool(true)));
            }
            Jv::Obj(pairs)
        }
        _ => Jv::Obj(vec![]),
    }
}

// ---------------------------------------------------------------------------
// JSON formatters
// ---------------------------------------------------------------------------

/// Serialize the fanout report to the insertion-order JSON envelope.
/// No trailing newline — matches `JSON.stringify(..., undefined, 2)`.
pub fn format_fanout_json(
    report: &FanoutReport,
    driver_version: &str,
    deterministic: bool,
) -> String {
    let generated_at = if deterministic {
        "0".to_string()
    } else {
        crate::engine::gate::format_json::pinned_or_now_iso8601(false)
    };

    let summary = Jv::Obj(vec![
        (
            "totalPublishers".to_string(),
            Jv::n(report.total_publishers),
        ),
        ("totalEvents".to_string(), Jv::n(report.total_events)),
        (
            "zeroSubscriberEvents".to_string(),
            Jv::n(report.zero_subscriber_events),
        ),
        ("hotEvents".to_string(), Jv::n(report.hot_events)),
        (
            "coveragePartialEvents".to_string(),
            Jv::n(report.coverage_partial_events),
        ),
    ]);

    let entries = Jv::Arr(report.entries.iter().map(fanout_entry_to_jv).collect());

    let envelope = Jv::Obj(vec![
        ("al_sem_version".to_string(), Jv::s(driver_version)),
        ("generated_at".to_string(), Jv::s(&generated_at)),
        ("kind".to_string(), Jv::s("events.fanout")),
        ("summary".to_string(), summary),
        ("entries".to_string(), entries),
    ]);

    serialize_jv(&envelope)
}

/// Serialize the chain report to the insertion-order JSON envelope.
pub fn format_chains_json(
    report: &ChainReport,
    driver_version: &str,
    deterministic: bool,
) -> String {
    let generated_at = if deterministic {
        "0".to_string()
    } else {
        crate::engine::gate::format_json::pinned_or_now_iso8601(false)
    };

    let summary = Jv::Obj(vec![
        ("totalRoots".to_string(), Jv::n(report.total_roots)),
        (
            "rootsWithEvents".to_string(),
            Jv::n(report.roots_with_events),
        ),
        ("maxChainDepth".to_string(), Jv::n(report.max_chain_depth)),
        ("cyclesDetected".to_string(), Jv::n(report.cycles_detected)),
        (
            "depthTruncatedNodes".to_string(),
            Jv::n(report.depth_truncated_nodes),
        ),
    ]);

    let chains = Jv::Arr(report.chains.iter().map(chain_node_to_jv).collect());

    let envelope = Jv::Obj(vec![
        ("al_sem_version".to_string(), Jv::s(driver_version)),
        ("generated_at".to_string(), Jv::s(&generated_at)),
        ("kind".to_string(), Jv::s("events.chains")),
        ("summary".to_string(), summary),
        ("chains".to_string(), chains),
    ]);

    serialize_jv(&envelope)
}

// ---------------------------------------------------------------------------
// Human formatters
// ---------------------------------------------------------------------------

fn coverage_glyph(s: &str) -> &'static str {
    match s {
        "complete" => "✓",
        "partial" => "≈",
        _ => "?",
    }
}

/// Render the fanout report as human text (matches `formatFanout` in `format-events.ts`).
pub fn format_fanout_human(report: &FanoutReport) -> String {
    let mut lines: Vec<String> = Vec::new();
    lines.push(format!(
        "Event fanout report  ({} publishers, {} events, {} hot)",
        report.total_publishers, report.total_events, report.hot_events
    ));
    lines.push(String::new());
    for e in &report.entries {
        let cov = format!(
            "[{}{}{}]",
            coverage_glyph(e.coverage.dispatch_edges),
            coverage_glyph(e.coverage.subscriber_discovery),
            coverage_glyph(e.coverage.capability_composition),
        );
        lines.push(format!(
            "  {}  ({}) → {} subscriber(s) {}",
            e.event_name, e.event_kind, e.direct_subscriber_count, cov
        ));
    }
    // join("\n") + "\n" — trailing newline matches TS `${lines.join("\n")}\n`
    let mut out = lines.join("\n");
    out.push('\n');
    out
}

fn render_chain(node: &ChainNode, depth: usize, lines: &mut Vec<String>) {
    let indent = "  ".repeat(depth);
    match node.kind {
        "root" => {
            let rid = node.routine_id.as_deref().unwrap_or("?");
            lines.push(format!("{indent}root {rid}"));
        }
        "event-dispatch" => {
            let name = node
                .event_name
                .as_deref()
                .or(node.event_id.as_deref())
                .unwrap_or("");
            let tail = if node.depth_truncated {
                "  (depth truncated)"
            } else {
                ""
            };
            lines.push(format!("{indent}↪ {name}{tail}"));
        }
        "subscriber" => {
            // TS `walkEventChain` never sets `depthTruncated` on a subscriber node
            // (only on event-dispatch), so the only reachable marker is `(cycle)`.
            let rid = node.routine_id.as_deref().unwrap_or("");
            let marker = if node.cycle_detected { "  (cycle)" } else { "" };
            lines.push(format!("{indent}• {rid}{marker}"));
        }
        _ => {}
    }
    for c in &node.children {
        render_chain(c, depth + 1, lines);
    }
}

/// Render the chain report as human text (matches `formatChains` in `format-events.ts`).
pub fn format_chains_human(report: &ChainReport) -> String {
    let mut lines: Vec<String> = Vec::new();
    lines.push(format!(
        "Event chains report  ({} roots, max depth {}, {} cycles, {} depth-truncated)",
        report.total_roots,
        report.max_chain_depth,
        report.cycles_detected,
        report.depth_truncated_nodes
    ));
    lines.push(String::new());
    for c in &report.chains {
        render_chain(c, 0, &mut lines);
    }
    let mut out = lines.join("\n");
    out.push('\n');
    out
}

// ---------------------------------------------------------------------------
// Pipeline result
// ---------------------------------------------------------------------------

/// Result returned by `run_events_fanout` and `run_events_chains`.
pub struct EventsRunResult {
    /// The text to write (JSON or human).
    pub text: String,
    /// 0 = ok, 1 = error.
    pub exit_code: u8,
    /// Lines to emit on stderr.
    pub stderr_lines: Vec<String>,
}

// ---------------------------------------------------------------------------
// Events fanout pipeline
// ---------------------------------------------------------------------------

/// Options for `run_events_fanout`.
pub struct EventsFanoutOptions<'a> {
    pub workspace: &'a std::path::Path,
    pub format: &'a str, // "human" | "json"
    pub scope: Scope,
    /// "warn" (default) | "strict" | "ignore". Mirrors `--coverage-policy` in
    /// al-sem `events-fanout.ts`. `strict` drops entries whose `dispatchEdges` OR
    /// `capabilityComposition` is "partial" (emitting a stderr line + exit 1);
    /// `ignore` rewrites every entry's coverage to all-"complete".
    pub coverage_policy: &'a str,
    pub driver_version: &'a str,
    pub deterministic: bool,
    pub strict: bool,
}

/// Run the fanout pipeline: load the workspace, compute indexes, compute the
/// report, format it, return the result. Mirrors `runEventsFanout` in al-sem.
pub fn run_events_fanout(opts: &EventsFanoutOptions) -> EventsRunResult {
    let model_id = match compute_gate_model_instance_id(opts.workspace) {
        Some(id) => id,
        None => {
            return EventsRunResult {
                text: String::new(),
                exit_code: 1,
                stderr_lines: vec![
                    "al-sem events fanout: could not compute modelInstanceId".to_string(),
                ],
            };
        }
    };

    let resolved = match assemble_and_resolve_workspace(opts.workspace, &model_id, false) {
        Some(r) => r,
        None => {
            return EventsRunResult {
                text: String::new(),
                exit_code: 1,
                stderr_lines: vec!["al-sem events fanout: workspace did not resolve".to_string()],
            };
        }
    };

    // `analyzeWorkspace` diagnostics: workspace + overlay + ALL default-detector
    // diagnostics (e.g. `d43-event-ishandled-skip`). events-fanout.ts emits these
    // to stderr at the end. `infra_diagnostics` alone is NOT enough — it omits the
    // detector-stage diagnostics that the goldens' `.stderr.txt` capture.
    let diag_lines = analyze_workspace_diagnostic_lines(opts.workspace, &resolved);

    if opts.strict && diag_lines.iter().any(|l| l.starts_with("error:")) {
        return EventsRunResult {
            text: String::new(),
            exit_code: 1,
            stderr_lines: diag_lines,
        };
    }

    // Build detector context to get event_flow_indexes + summaries + event_graph.
    // ⟨C1 Task 3 — R2/R3 caller classification⟩ DERIVED-ONLY: plain `ALL`, no
    // `RAW_INHERITED_FACTS`. `ctx.summaries` reaches only `compute_fanout`, which
    // reads `sum.coverage.as_ref().map(|c| c.inherited_status)` and NOTHING else
    // off the summary (`l5/event_flow.rs`'s `capabilityComposition` roll-up) —
    // coverage is composed under every output mode. No fact Vec is touched.
    let ctx = build_detector_context(&resolved, crate::engine::l5::registry::substrate::ALL);
    let ix = &ctx.event_flow_indexes;

    let mut report = compute_fanout_report(&ctx.event_graph, ix, &ctx.summaries, opts.scope);

    // --- --coverage-policy application (fanout only; chains validates but no-ops) ---
    //
    // CRITICAL: NEITHER branch recomputes `report.summary` — the summary counters
    // (totalEvents / coveragePartialEvents / …) pass through with their PRE-filter
    // values, exactly as al-sem does (it spreads `{ ...report, entries: ... }`).
    let mut coverage_stderr: Vec<String> = Vec::new();
    let mut coverage_exit_elevated = false;
    match opts.coverage_policy {
        "strict" => {
            // Drop entries where dispatchEdges OR capabilityComposition is "partial".
            // (subscriberDiscovery / "unknown" do NOT trigger a drop.)
            let mut kept: Vec<crate::engine::l5::event_flow::FanoutEntry> =
                Vec::with_capacity(report.entries.len());
            for e in report.entries.drain(..) {
                if e.coverage.dispatch_edges == "partial"
                    || e.coverage.capability_composition == "partial"
                {
                    coverage_stderr.push(format!(
                        "coverage-incomplete: event {} dispatchEdges={} capability={}",
                        e.event_id, e.coverage.dispatch_edges, e.coverage.capability_composition
                    ));
                    coverage_exit_elevated = true;
                } else {
                    kept.push(e);
                }
            }
            report.entries = kept;
        }
        "ignore" => {
            // Rewrite every entry's coverage to all-"complete".
            for e in report.entries.iter_mut() {
                e.coverage = FanoutCoverage {
                    dispatch_edges: "complete",
                    subscriber_discovery: "complete",
                    capability_composition: "complete",
                };
            }
        }
        _ => {} // "warn" (default): pass through unchanged.
    }

    let text = match opts.format {
        "json" => format_fanout_json(&report, opts.driver_version, opts.deterministic),
        _ => format_fanout_human(&report),
    };

    // stderr ordering mirrors al-sem: coverage-incomplete lines are written DURING
    // the strict filter (before stdout), then analyzer diagnostics at the very end.
    let mut stderr_lines = coverage_stderr;
    stderr_lines.extend(diag_lines);

    EventsRunResult {
        text,
        exit_code: if coverage_exit_elevated { 1 } else { 0 },
        stderr_lines,
    }
}

/// Compute the `analyzeWorkspace`-equivalent diagnostic lines (`<severity>: <message>`)
/// for the events commands' stderr. Runs the DEFAULT detector set so detector-stage
/// diagnostics (e.g. `d43-event-ishandled-skip`) surface exactly as al-sem does.
fn analyze_workspace_diagnostic_lines(
    workspace: &std::path::Path,
    resolved: &crate::engine::l3::l3_workspace::L3Resolved,
) -> Vec<String> {
    let default_detectors: Vec<_> = registered_detectors()
        .into_iter()
        .filter(|d| DEFAULT_DETECTOR_NAMES.contains(&d.name.as_str()))
        .collect();
    compute_analyzer_diagnostics(workspace, resolved, &default_detectors)
        .iter()
        .map(|d| format!("{}: {}", d.severity, d.message))
        .collect()
}

// ---------------------------------------------------------------------------
// Events chains pipeline
// ---------------------------------------------------------------------------

/// Options for `run_events_chains`.
pub struct EventsChainsOptions<'a> {
    pub workspace: &'a std::path::Path,
    pub format: &'a str,
    pub scope: Scope,
    /// "warn" | "strict" | "ignore". al-sem `events-chains.ts` VALIDATES this flag
    /// (rejecting unknown values) but NEVER applies it — chains ignores coverage
    /// policy entirely. Carried here only so the CLI layer can validate it; the
    /// pipeline below does NOT read it (matching TS exactly).
    pub coverage_policy: &'a str,
    pub max_depth: Option<usize>,
    pub max_nodes: Option<usize>,
    pub driver_version: &'a str,
    pub deterministic: bool,
    pub strict: bool,
}

/// Run the chains pipeline: load the workspace, compute indexes, walk chains,
/// format result, return. Mirrors `runEventsChains` in al-sem.
pub fn run_events_chains(opts: &EventsChainsOptions) -> EventsRunResult {
    let model_id = match compute_gate_model_instance_id(opts.workspace) {
        Some(id) => id,
        None => {
            return EventsRunResult {
                text: String::new(),
                exit_code: 1,
                stderr_lines: vec![
                    "al-sem events chains: could not compute modelInstanceId".to_string(),
                ],
            };
        }
    };

    let resolved = match assemble_and_resolve_workspace(opts.workspace, &model_id, false) {
        Some(r) => r,
        None => {
            return EventsRunResult {
                text: String::new(),
                exit_code: 1,
                stderr_lines: vec!["al-sem events chains: workspace did not resolve".to_string()],
            };
        }
    };

    let diag_lines = analyze_workspace_diagnostic_lines(opts.workspace, &resolved);

    if opts.strict && diag_lines.iter().any(|l| l.starts_with("error:")) {
        return EventsRunResult {
            text: String::new(),
            exit_code: 1,
            stderr_lines: diag_lines,
        };
    }

    // NOTE: `opts.coverage_policy` is intentionally UNUSED here — al-sem
    // `events-chains.ts` validates it but never applies it to the chain report.

    // ⟨C1 Task 3 — R2/R3 caller classification⟩ DERIVED-ONLY: plain `ALL`. The
    // chain report reads ONLY `ctx.event_flow_indexes`; `ctx.summaries` is never
    // consulted on this path at all.
    let ctx = build_detector_context(&resolved, crate::engine::l5::registry::substrate::ALL);
    let ix = &ctx.event_flow_indexes;

    let walk_opts = ChainWalkOptions {
        max_depth: opts.max_depth,
        max_nodes: opts.max_nodes,
    };

    let report = compute_chain_report(ix, &walk_opts, opts.scope);

    let text = match opts.format {
        "json" => format_chains_json(&report, opts.driver_version, opts.deterministic),
        _ => format_chains_human(&report),
    };

    EventsRunResult {
        text,
        exit_code: 0,
        stderr_lines: diag_lines,
    }
}

// ---------------------------------------------------------------------------
// Unit tests (cycle + depth-truncation rendering, pure serializer)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::l3::event_graph::{EventEdge, EventGraph, EventSymbol, Evidence};
    use crate::engine::l5::event_flow::{
        ChainWalkOptions, Scope, build_event_flow_indexes, compute_chain_report,
        compute_fanout_report,
    };
    use std::collections::{BTreeSet, HashMap};

    fn ev_sym(id: &str, pub_routine: Option<&str>, name: &str) -> EventSymbol {
        EventSymbol {
            id: id.to_string(),
            publisher_object_id: "app/Codeunit/1".to_string(),
            publisher_routine_id: pub_routine.map(|s| s.to_string()),
            publisher_stable_routine_id: pub_routine.map(|s| s.to_string()),
            event_name: name.to_string(),
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
        }
    }

    fn ev_edge(event_id: &str, sub: &str) -> EventEdge {
        EventEdge {
            event_id: event_id.to_string(),
            subscriber_routine_id: sub.to_string(),
            subscriber_stable_routine_id: sub.to_string(),
            subscriber_app_id: "app".to_string(),
            resolution: "resolved".to_string(),
            provenance: vec![],
        }
    }

    /// NATIVE ORACLE: cycle path — a subscriber (S1) that also publishes back to the
    /// root publisher (P). In real AL this requires the same routine to be both
    /// [IntegrationEvent] publisher AND a subscriber (not expressible via real AL
    /// source — see manifest.json cycleNote). Here we build the EventGraph directly.
    ///
    /// Graph: P publishes E1 → S1 subscribes. S1 publishes E2 → P subscribes.
    /// Walking from P: root(P) → event-dispatch(E1) → subscriber(S1) → event-dispatch(E2)
    ///                         → subscriber(P) [CYCLE].
    #[test]
    fn cycle_oracle_human_and_json() {
        let event_graph = EventGraph {
            events: vec![
                ev_sym("E1", Some("P"), "Ev1"),
                ev_sym("E2", Some("S1"), "Ev2"),
            ],
            edges: vec![ev_edge("E1", "S1"), ev_edge("E2", "P")],
        };

        let dep_ids: BTreeSet<String> = BTreeSet::new();
        // Build routines list (minimal — primary_routines = {P, S1})
        let routines: Vec<crate::engine::l3::l3_workspace::L3Routine> = Vec::new();
        let ix = build_event_flow_indexes(&event_graph, &routines, &dep_ids);

        let walk_opts = ChainWalkOptions::default();
        // Walk from P: should produce cycleDetected on the P subscriber node.
        let tree = crate::engine::l5::event_flow::walk_event_chain("P", &ix, &walk_opts);

        // Collect all nodes recursively.
        fn collect(node: &ChainNode, out: &mut Vec<ChainNode>) {
            out.push(node.clone());
            for c in &node.children {
                collect(c, out);
            }
        }
        let mut nodes = Vec::new();
        collect(&tree, &mut nodes);

        // There must be at least one node with cycle_detected = true.
        let has_cycle = nodes.iter().any(|n| n.cycle_detected);
        assert!(has_cycle, "expected a cycle_detected node in walk from P");

        // Human rendering must contain "  (cycle)".
        let report = compute_chain_report(&ix, &walk_opts, Scope::All);
        let human = format_chains_human(&report);
        assert!(
            human.contains("  (cycle)"),
            "human output must contain '  (cycle)'; got:\n{human}"
        );

        // JSON rendering must contain `"cycleDetected": true`.
        let json = format_chains_json(&report, "test-v1", true);
        assert!(
            json.contains("\"cycleDetected\": true"),
            "json output must contain '\"cycleDetected\": true'; got:\n{json}"
        );
    }

    /// Depth-truncation rendering: max_depth=1 truncates subscriber expansion.
    #[test]
    fn depth_truncation_rendering() {
        let event_graph = EventGraph {
            events: vec![ev_sym("E1", Some("P"), "OnP")],
            edges: vec![ev_edge("E1", "S1")],
        };
        let dep_ids: BTreeSet<String> = BTreeSet::new();
        let routines: Vec<crate::engine::l3::l3_workspace::L3Routine> = Vec::new();
        let ix = build_event_flow_indexes(&event_graph, &routines, &dep_ids);

        let walk_opts = ChainWalkOptions {
            max_depth: Some(1),
            max_nodes: None,
        };
        let report = compute_chain_report(&ix, &walk_opts, Scope::All);

        // Human: must contain "(depth truncated)".
        let human = format_chains_human(&report);
        assert!(
            human.contains("(depth truncated)"),
            "human must contain '(depth truncated)'; got:\n{human}"
        );

        // JSON: must contain `"depthTruncated": true`.
        let json = format_chains_json(&report, "test-v1", true);
        assert!(
            json.contains("\"depthTruncated\": true"),
            "json must contain '\"depthTruncated\": true'; got:\n{json}"
        );

        // Summary: depthTruncatedNodes = 1.
        assert_eq!(report.depth_truncated_nodes, 1);
    }

    /// Insertion-order JSON: fanout envelope keys appear in TS order.
    #[test]
    fn fanout_json_key_order() {
        let event_graph = EventGraph {
            events: vec![ev_sym("E1", Some("P"), "OnP")],
            edges: vec![],
        };
        let dep_ids: BTreeSet<String> = BTreeSet::new();
        let routines: Vec<crate::engine::l3::l3_workspace::L3Routine> = Vec::new();
        let ix = build_event_flow_indexes(&event_graph, &routines, &dep_ids);
        let summaries: HashMap<String, crate::engine::l5::full_summary::FullRoutineSummary> =
            HashMap::new();
        let report = compute_fanout_report(&event_graph, &ix, &summaries, Scope::All);
        let json = format_fanout_json(&report, "test-v1", true);

        // Top-level key order.
        let al_pos = json.find("\"al_sem_version\"").expect("al_sem_version");
        let gen_pos = json.find("\"generated_at\"").expect("generated_at");
        let kind_pos = json.find("\"kind\"").expect("kind");
        let summ_pos = json.find("\"summary\"").expect("summary");
        let ent_pos = json.find("\"entries\"").expect("entries");
        assert!(al_pos < gen_pos, "al_sem_version before generated_at");
        assert!(gen_pos < kind_pos, "generated_at before kind");
        assert!(kind_pos < summ_pos, "kind before summary");
        assert!(summ_pos < ent_pos, "summary before entries");
    }

    /// Insertion-order JSON: chains envelope keys appear in TS order.
    #[test]
    fn chains_json_key_order() {
        let event_graph = EventGraph {
            events: vec![ev_sym("E1", Some("P"), "OnP")],
            edges: vec![],
        };
        let dep_ids: BTreeSet<String> = BTreeSet::new();
        let routines: Vec<crate::engine::l3::l3_workspace::L3Routine> = Vec::new();
        let ix = build_event_flow_indexes(&event_graph, &routines, &dep_ids);
        let walk_opts = ChainWalkOptions::default();
        let report = compute_chain_report(&ix, &walk_opts, Scope::All);
        let json = format_chains_json(&report, "test-v1", true);

        let al_pos = json.find("\"al_sem_version\"").expect("al_sem_version");
        let gen_pos = json.find("\"generated_at\"").expect("generated_at");
        let kind_pos = json.find("\"kind\"").expect("kind");
        let summ_pos = json.find("\"summary\"").expect("summary");
        let chains_pos = json.find("\"chains\"").expect("chains");
        assert!(al_pos < gen_pos);
        assert!(gen_pos < kind_pos);
        assert!(kind_pos < summ_pos);
        assert!(summ_pos < chains_pos);
    }
}

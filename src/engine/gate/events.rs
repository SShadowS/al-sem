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
//! `JSON.stringify({al_sem_version, generated_at, kind, summary, entries|chains}, undefined, 2)`,
//! plus (#70) a `dependencies` statement right after `kind`.
//!
//! ## Model
//!
//! Cross-app by default (#70): the base `alsem analyze` builds, so a workspace
//! subscriber to a dependency event is seen. `--single-app` analyses the workspace
//! alone.
//!
//! We build the JSON by hand using a tiny ordered-value tree — no `#[derive(Serialize)]`
//! on the engine structs.
//!
//! ## Scope
//!
//! `--scope primary` (default): keep only entries where the publisher OR any resolved
//! subscriber is a primary (non-dep) routine. `--scope all`: keep everything, which
//! cross-app includes events between dependency routines only.
//!
//! ## Determinism
//!
//! `--deterministic` pins `generated_at = "0"` and reads the version override from
//! the caller rather than the live version. Used by every differential test.

use crate::engine::batch::job::{
    Doc, Job, JobFailure, PlanPart, ResolveCx, SideDoc, SideIx, ViewCx, ViewDemand, ViewFailure,
    ViewNeeds,
};
use crate::engine::batch::output::{JobOutput, WriteAction};
use crate::engine::gate::run::{analyzer_diagnostics_of_view, diagnostic_lines};
use crate::engine::l5::event_flow::{
    ChainNode, ChainReport, ChainWalkOptions, FanoutCoverage, FanoutReport, Scope,
    compute_chain_report, compute_fanout_report,
};
use crate::engine::l5::registry::Detector;
use crate::engine::session::{Cell, events_plan};
use crate::program::inputs::WorkspaceInputs;
use crate::program::resolve::full::LoadedDependencies;
use std::sync::Arc;

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
    dependencies: &LoadedDependencies,
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
        (
            "dependencies".to_string(),
            Jv::from_json(&dependencies.to_json()),
        ),
        ("summary".to_string(), summary),
        ("entries".to_string(), entries),
    ]);

    serialize_jv(&envelope)
}

/// Serialize the chain report to the insertion-order JSON envelope.
pub fn format_chains_json(
    report: &ChainReport,
    dependencies: &LoadedDependencies,
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
        (
            "dependencies".to_string(),
            Jv::from_json(&dependencies.to_json()),
        ),
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

/// An events command's early result: `stderr_lines`, no text, exit 1.
fn early(stderr_lines: Vec<String>) -> EventsRunResult {
    EventsRunResult {
        text: String::new(),
        exit_code: 1,
        stderr_lines,
    }
}

/// Which report an events job writes.
enum EventsReport<'a> {
    Fanout(&'a EventsFanoutOptions<'a>),
    Chains(&'a EventsChainsOptions<'a>),
}

/// `alsem events fanout` / `chains` as a batch job (#277 D1). Its document
/// is the [`EventsRunResult`]; its writes are the text (when non-empty, to
/// `--out` or stdout), then the stderr lines.
pub struct EventsJob<'a> {
    detectors: Vec<Detector>,
    report: EventsReport<'a>,
    out: Option<&'a str>,
}

impl<'a> EventsJob<'a> {
    pub fn fanout(opts: &'a EventsFanoutOptions<'a>, out: Option<&'a str>) -> EventsJob<'a> {
        Self::with(
            crate::engine::l5::detectors::default_detectors(),
            EventsReport::Fanout(opts),
            out,
        )
    }

    pub fn chains(opts: &'a EventsChainsOptions<'a>, out: Option<&'a str>) -> EventsJob<'a> {
        Self::with(
            crate::engine::l5::detectors::default_detectors(),
            EventsReport::Chains(opts),
            out,
        )
    }

    fn with(detectors: Vec<Detector>, report: EventsReport<'a>, out: Option<&'a str>) -> Self {
        EventsJob {
            detectors,
            report,
            out,
        }
    }

    /// The job's workspace, universe, `--strict` and error-line prefix.
    fn head(&self) -> (&'a Arc<WorkspaceInputs>, bool, bool, &'static str) {
        match self.report {
            EventsReport::Fanout(o) => (o.inputs, o.single_app, o.strict, "al-sem events fanout"),
            EventsReport::Chains(o) => (o.inputs, o.single_app, o.strict, "al-sem events chains"),
        }
    }

    /// The job's result, through the driver.
    fn run(&self) -> EventsRunResult {
        crate::engine::batch::run_doc(self)
            .one()
            .unwrap_or_else(|f| unreachable!("events fails no side: {f:?}"))
    }
}

impl Job for EventsJob<'_> {
    fn name(&self) -> &'static str {
        "events"
    }
    fn demands(&self) -> Vec<ViewDemand> {
        // #70: the cross-app base by default (a workspace subscriber to a
        // dependency event is BC's main events case), the workspace alone
        // under `--single-app`.
        let (inputs, single_app, ..) = self.head();
        ViewDemand::one(inputs, single_app, false, ViewNeeds::MODEL)
    }

    fn resolve(&self, _: SideIx, _: &ResolveCx<'_>) -> Result<PlanPart, JobFailure> {
        Ok(PlanPart {
            plan: events_plan(self.detectors.clone()),
            reads: vec![Cell::Det],
            carry: None,
        })
    }

    fn compute(&self, _: SideIx, cx: &mut ViewCx<'_>) -> Result<SideDoc, JobFailure> {
        let (_, _, strict, _) = self.head();
        let result = match events_head(cx, strict) {
            Err(early) => early,
            Ok(diag_lines) => match self.report {
                EventsReport::Fanout(o) => fanout_report(cx, o, diag_lines),
                EventsReport::Chains(o) => chains_report(cx, o, diag_lines),
            },
        };
        Ok(SideDoc::new(result))
    }

    fn view_failed(&self, _: SideIx, failure: &ViewFailure) -> Result<SideDoc, JobFailure> {
        // No model-instance id: `<cmd>: could not compute modelInstanceId`, exit 1.
        let (.., cmd) = self.head();
        Ok(SideDoc::new(early(vec![failure.target.message(cmd)])))
    }

    fn serialize(&self, doc: Doc) -> JobOutput {
        let r = doc
            .one::<EventsRunResult>()
            .unwrap_or_else(|f| unreachable!("events fails no side: {f:?}"));
        let mut actions = Vec::new();
        if !r.text.is_empty() {
            actions.push(WriteAction::out(self.out, r.text));
        }
        actions.extend(r.stderr_lines.iter().map(|l| WriteAction::line(l)));
        JobOutput {
            actions,
            exit: r.exit_code,
        }
    }
}

/// Both events commands' start (#160 §4.4.4), off the prepared view: the
/// analyzer diagnostic lines of its one run, which is taken once read. `Err`
/// is the command's early result: `--strict` with an error-severity
/// diagnostic.
///
/// The lines are `analyzeWorkspace`'s diagnostics: workspace + dependency-ledger
/// warnings + overlay + the default detectors' (e.g. `d43-event-ishandled-skip`).
/// events-fanout.ts emits them to stderr at the end; `infra_diagnostics` alone
/// omits the detector-stage ones the goldens' `.stderr.txt` capture.
fn events_head(cx: &mut ViewCx<'_>, strict: bool) -> Result<Vec<String>, EventsRunResult> {
    let key = cx.keys()[0].clone();
    let diag_lines = diagnostic_lines(&analyzer_diagnostics_of_view(cx.view(), &key));
    drop(cx.take_run(&key));
    // `l4` is filled when the run demands `CORE_SUMMARIES`; nothing after the
    // run reads it (as in policy check).
    cx.release(Cell::L4);
    if strict && diag_lines.iter().any(|l| l.starts_with("error:")) {
        return Err(early(diag_lines));
    }
    Ok(diag_lines)
}

/// After the report, its last reader: `det` holds the graph's `Arc`, so every
/// cell goes together. (`l4` went right after the run, in [`events_head`].)
fn release_cells(cx: &mut ViewCx<'_>) {
    cx.release_all_but(&[]);
}

// ---------------------------------------------------------------------------
// Events fanout pipeline
// ---------------------------------------------------------------------------

/// Options for `run_events_fanout`.
pub struct EventsFanoutOptions<'a> {
    /// The command's one read of the workspace (#160).
    pub inputs: &'a Arc<WorkspaceInputs>,
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
    /// `--single-app`: the workspace alone; default (#70) the cross-app base.
    pub single_app: bool,
}

/// Run the fanout pipeline: load the workspace, compute indexes, compute the
/// report, format it, return the result. Mirrors `runEventsFanout` in al-sem.
pub fn run_events_fanout(opts: &EventsFanoutOptions) -> EventsRunResult {
    events_fanout_with(crate::engine::l5::detectors::default_detectors(), opts)
}

/// [`run_events_fanout`] over a given detector set (#160 §4.4.4): the plan is
/// built from `detectors` by `events_plan`, so a set without `SUMMARIES` makes
/// `prepare` panic (A2.10).
pub(crate) fn events_fanout_with(
    detectors: Vec<Detector>,
    opts: &EventsFanoutOptions,
) -> EventsRunResult {
    EventsJob::with(detectors, EventsReport::Fanout(opts), None).run()
}

/// The fanout report off the prepared view, after [`events_head`].
fn fanout_report(
    cx: &mut ViewCx<'_>,
    opts: &EventsFanoutOptions,
    diag_lines: Vec<String>,
) -> EventsRunResult {
    // The report reads `det.summaries` (which `events_plan` declares), and of
    // each summary only `sum.coverage.as_ref().map(|c| c.inherited_status)`
    // (`l5/event_flow.rs`'s `capabilityComposition` roll-up). No fact Vec is
    // touched, so the derived-only cone the run built is enough.
    let det = cx.view().det();
    let mut report = compute_fanout_report(
        &det.event_graph,
        &det.event_flow_indexes,
        &det.summaries,
        opts.scope,
    );
    release_cells(cx);

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

    let dependencies = cx.view().target().dependencies();
    let text = match opts.format {
        "json" => format_fanout_json(
            &report,
            &dependencies,
            opts.driver_version,
            opts.deterministic,
        ),
        _ => dependencies.prefix_human(format_fanout_human(&report)),
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

// ---------------------------------------------------------------------------
// Events chains pipeline
// ---------------------------------------------------------------------------

/// Options for `run_events_chains`.
pub struct EventsChainsOptions<'a> {
    /// The command's one read of the workspace (#160).
    pub inputs: &'a Arc<WorkspaceInputs>,
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
    /// `--single-app`: the workspace alone; default (#70) the cross-app base.
    pub single_app: bool,
}

/// Run the chains pipeline: load the workspace, compute indexes, walk chains,
/// format result, return. Mirrors `runEventsChains` in al-sem.
pub fn run_events_chains(opts: &EventsChainsOptions) -> EventsRunResult {
    events_chains_with(crate::engine::l5::detectors::default_detectors(), opts)
}

/// [`run_events_chains`] over a given detector set: the same plan as
/// [`events_fanout_with`] (A2.10).
pub(crate) fn events_chains_with(
    detectors: Vec<Detector>,
    opts: &EventsChainsOptions,
) -> EventsRunResult {
    EventsJob::with(detectors, EventsReport::Chains(opts), None).run()
}

/// The chain report off the prepared view, after [`events_head`].
fn chains_report(
    cx: &mut ViewCx<'_>,
    opts: &EventsChainsOptions,
    diag_lines: Vec<String>,
) -> EventsRunResult {
    // NOTE: `opts.coverage_policy` is intentionally UNUSED here — al-sem
    // `events-chains.ts` validates it but never applies it to the chain report.

    // The chain report reads ONLY `det.event_flow_indexes`.
    let walk_opts = ChainWalkOptions {
        max_depth: opts.max_depth,
        max_nodes: opts.max_nodes,
    };
    let report = compute_chain_report(&cx.view().det().event_flow_indexes, &walk_opts, opts.scope);
    release_cells(cx);

    let dependencies = cx.view().target().dependencies();
    let text = match opts.format {
        "json" => format_chains_json(
            &report,
            &dependencies,
            opts.driver_version,
            opts.deterministic,
        ),
        _ => dependencies.prefix_human(format_chains_human(&report)),
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
    use crate::engine::l5::event_flow::{
        ChainWalkOptions, Scope, build_event_flow_indexes, compute_chain_report,
        compute_fanout_report,
    };
    use crate::program::model::events::{EventEdge, EventGraph, EventSymbol, Evidence};
    use std::collections::{BTreeSet, HashMap};

    /// A2.10: the bits the events report reads off `det` (`SUMMARIES`), and the
    /// one that also builds summaries (`TRANSACTION_SPANS`): d1 and d9 today.
    const DROPPED: u32 = crate::engine::l5::registry::substrate::SUMMARIES
        | crate::engine::l5::registry::substrate::TRANSACTION_SPANS;

    /// A2.10: fanout builds its plan from the set it is given.
    #[test]
    fn fanout_without_d1_d9() {
        crate::engine::l5::digest_cli::assert_prepare_refuses_a_reduced_set(
            DROPPED,
            "SUMMARIES",
            "ws-event-fanout",
            |set, inputs| {
                let _ = events_fanout_with(
                    set,
                    &EventsFanoutOptions {
                        inputs,
                        format: "json",
                        scope: Scope::Primary,
                        coverage_policy: "warn",
                        driver_version: "test",
                        deterministic: true,
                        strict: false,
                        single_app: false,
                    },
                );
            },
        );
    }

    /// A2.10: chains builds its plan from the set it is given.
    #[test]
    fn chains_without_d1_d9() {
        crate::engine::l5::digest_cli::assert_prepare_refuses_a_reduced_set(
            DROPPED,
            "SUMMARIES",
            "ws-event-fanout",
            |set, inputs| {
                let _ = events_chains_with(
                    set,
                    &EventsChainsOptions {
                        inputs,
                        format: "json",
                        scope: Scope::Primary,
                        coverage_policy: "warn",
                        max_depth: None,
                        max_nodes: None,
                        driver_version: "test",
                        deterministic: true,
                        strict: false,
                        single_app: false,
                    },
                );
            },
        );
    }

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
            conditional: false,
            mixed_readings: false,
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
        let routines: Vec<crate::program::model::workspace::ModelRoutine> = Vec::new();
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
        let json = format_chains_json(&report, &LoadedDependencies::default(), "test-v1", true);
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
        let routines: Vec<crate::program::model::workspace::ModelRoutine> = Vec::new();
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
        let json = format_chains_json(&report, &LoadedDependencies::default(), "test-v1", true);
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
        let routines: Vec<crate::program::model::workspace::ModelRoutine> = Vec::new();
        let ix = build_event_flow_indexes(&event_graph, &routines, &dep_ids);
        let summaries: HashMap<String, crate::engine::l5::full_summary::FullRoutineSummary> =
            HashMap::new();
        let report = compute_fanout_report(&event_graph, &ix, &summaries, Scope::All);
        let json = format_fanout_json(&report, &LoadedDependencies::default(), "test-v1", true);

        // Top-level key order.
        let al_pos = json.find("\"al_sem_version\"").expect("al_sem_version");
        let gen_pos = json.find("\"generated_at\"").expect("generated_at");
        let kind_pos = json.find("\"kind\"").expect("kind");
        let deps_pos = json.find("\"dependencies\"").expect("dependencies");
        let summ_pos = json.find("\"summary\"").expect("summary");
        let ent_pos = json.find("\"entries\"").expect("entries");
        assert!(al_pos < gen_pos, "al_sem_version before generated_at");
        assert!(gen_pos < kind_pos, "generated_at before kind");
        assert!(kind_pos < deps_pos, "kind before dependencies");
        assert!(deps_pos < summ_pos, "dependencies before summary");
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
        let routines: Vec<crate::program::model::workspace::ModelRoutine> = Vec::new();
        let ix = build_event_flow_indexes(&event_graph, &routines, &dep_ids);
        let walk_opts = ChainWalkOptions::default();
        let report = compute_chain_report(&ix, &walk_opts, Scope::All);
        let json = format_chains_json(&report, &LoadedDependencies::default(), "test-v1", true);

        let al_pos = json.find("\"al_sem_version\"").expect("al_sem_version");
        let gen_pos = json.find("\"generated_at\"").expect("generated_at");
        let kind_pos = json.find("\"kind\"").expect("kind");
        let deps_pos = json.find("\"dependencies\"").expect("dependencies");
        let summ_pos = json.find("\"summary\"").expect("summary");
        let chains_pos = json.find("\"chains\"").expect("chains");
        assert!(al_pos < gen_pos);
        assert!(gen_pos < kind_pos);
        assert!(kind_pos < deps_pos);
        assert!(deps_pos < summ_pos);
        assert!(summ_pos < chains_pos);
    }
}

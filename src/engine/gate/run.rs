//! `run_analyze` — the gate pipeline lib entry the `alsem analyze` bin wraps and the
//! differential tests call in-process. Mirrors the `analyze` action in al-sem
//! `src/cli/index.ts`.
//!
//! Stage 1 (SARIF + projection + filters): NO baseline, NO inline suppression.
//! Stage 2b (this layer): adds `--format pr-summary`, `--fail-on`, `--require-dependencies`,
//! the dependency-coverage preflight, and the CI exit-code contract.
//!
//! Pipeline:
//!   assemble_and_resolve_workspace(ws)  (L0→L3, source-only)
//!   → resolve the detector set (preset | --detector | default)
//!   → run_detectors (L4 inside the DetectorContext + L5) — pre-sorted Finding[]
//!   → project_finding per finding (display names + 1-based location)
//!   → filter_findings (min-severity, detector allow-list)
//!   → scope filter (primary drops dependency-anchored findings)
//!   → limit
//!   → format (sarif | pr-summary)
//!   → preflight + exit-code (--fail-on / --require-dependencies)
//!
//! Source-only: the transaction-integrity preset is intra-app, so this drives the
//! source-only `run_detectors` (not the cross-app variant). Every workspace object is
//! "primary", so the scope=primary filter keeps everything.

use std::path::Path;

use crate::engine::gate::app_attribution::App;
use crate::engine::gate::baseline::{apply_baseline, load_baseline, save_baseline};
use crate::engine::gate::exit_code::{compute_finding_exit, exit};
use crate::engine::gate::filter::{FilterOptions, Scope, filter_findings, scope_filter};
use crate::engine::gate::format_html::{HtmlFormatInputs, format_html};
use crate::engine::gate::format_json::{FindingEvidence, JsonFormatInputs, build_analyze_json};
use crate::engine::gate::format_pr_summary::format_pr_summary;
use crate::engine::gate::format_sarif::format_sarif;
use crate::engine::gate::format_terminal::{GroupBy, format_terminal, format_terminal_grouped};
use crate::engine::gate::inline_suppression::{apply_inline_suppressions, build_suppression_map};
use crate::engine::gate::model_instance_id::compute_gate_model_instance_id;
use crate::engine::gate::preflight::evaluate_preflight;
use crate::engine::gate::presets::resolve_analyze_detectors;
use crate::engine::gate::projection::{ProjectionIndex, project_finding};
use crate::engine::gate::version::driver_version;
use crate::engine::l3::coverage::AnalysisCoverage;
use crate::engine::l3::l3_workspace::{L3Resolved, assemble_and_resolve_workspace};
use crate::engine::l5::registry::run_detectors;
use crate::engine::perf_trace as pt;

/// Output format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputFormat {
    Sarif,
    PrSummary,
    /// Rich terminal output (colour, grouping). The future Stage A1 formatter.
    Terminal,
    /// Machine-readable JSON envelope (the future Stage A2 formatter).
    Json,
    /// Self-contained HTML report (the future Stage A3 formatter).
    Html,
}

/// Parsed `analyze` arguments.
#[derive(Debug, Clone)]
pub struct AnalyzeArgs {
    pub workspace: String,
    /// `--min-severity` (validated by the caller / CLI).
    pub min_severity: Option<String>,
    /// `--detector <ids>` (comma-separated). Mutually exclusive with `preset`.
    pub detector: Option<String>,
    /// `--preset <name>`.
    pub preset: Option<String>,
    /// `--scope` (default Primary).
    pub scope: Scope,
    /// `--limit`.
    pub limit: Option<usize>,
    /// `--format`.
    pub format: OutputFormat,
    /// `--sarif-version-override` — pins `driver.version` for byte-stable output.
    /// When `None`, `default_version` is used. (SARIF only; PR-summary embeds no version.)
    pub sarif_version_override: Option<String>,
    /// `--fail-on <sev>` — when `Some`, exit `FINDINGS` (1) if any kept finding is
    /// at/above this severity. Already-validated severity string (`None` ⇒ never gate).
    pub fail_on: Option<String>,
    /// `--require-dependencies` — make a degraded preflight FAIL (exit 4).
    pub require_dependencies: bool,
    /// `--baseline <path>` — when `Some`, load the baseline fingerprint set and drop any
    /// finding whose fingerprint is in it BEFORE inline suppression (Stage 3b).
    pub baseline: Option<String>,
    /// `--update-baseline` — when set together with `baseline`, write the current
    /// post-filter/scope/limit finding set to the baseline file (the new floor).
    pub update_baseline: bool,
    /// Disable inline `// al-sem-ignore` suppression. al-sem applies inline suppression
    /// unconditionally (default-ON, like a compiler pragma); this flag exists ONLY so the
    /// differential can capture the UN-suppressed SARIF (the +1 finding) — it has no CLI
    /// surface. Default `false` (suppression ON).
    pub disable_inline_suppression: bool,
    /// `--group-by <object|routine|table|detector|file>` — controls how the
    /// `terminal` formatter groups findings. Validated by the CLI before
    /// entering the pipeline. `None` means no explicit grouping was requested
    /// (the terminal formatter will use its default). Ignored by sarif/pr-summary/json/html.
    pub group_by: Option<String>,
    /// `--deterministic` — pins timestamps and version for byte-stable output.
    /// Used by the `json` formatter to pin `generatedAt` to the UNIX epoch.
    pub deterministic: bool,
    /// `--with-evidence` — opt-in augmentation of the `--format json` output: each
    /// finding gains an `evidencePath` (the stable-projected call chain) plus a
    /// POSITION-derived `enclosingMember`/`originatingObject` discriminator on its
    /// `primaryLocation`, and the envelope `schemaVersion` becomes `"1.1.0"`. When
    /// `false` (the default, and ALWAYS in the parity harness) the JSON output is
    /// byte-identical to today (all new keys absent, `schemaVersion "1.0.0"`). (RE-8)
    pub with_evidence: bool,
}

/// Read the workspace root `app.json` identity (`id` / `publisher` / `name` / `version`)
/// into the gate `App` registry. Mirrors al-sem `WorkspaceProvider.collect`:
///   - defaults: `publisher = "unknown"`, `name = "unknown"`, `version = "0.0.0.0"`.
///   - `appGuid` from the `id` (already validated by `compute_gate_model_instance_id`).
///
/// SOURCE-ONLY: exactly one app per run. Returns an empty registry if `app.json` is
/// unreadable (engine-never-throws; attribution then falls back to "(unknown app)").
fn read_workspace_apps(ws: &Path) -> Vec<App> {
    let Ok(text) = std::fs::read_to_string(ws.join("app.json")) else {
        return Vec::new();
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
        return Vec::new();
    };
    let Some(app_guid) = v
        .get("id")
        .and_then(|x| x.as_str())
        .filter(|s| !s.is_empty())
    else {
        return Vec::new();
    };
    let publisher = v
        .get("publisher")
        .and_then(|x| x.as_str())
        .unwrap_or("unknown")
        .to_string();
    let name = v
        .get("name")
        .and_then(|x| x.as_str())
        .unwrap_or("unknown")
        .to_string();
    let version = v
        .get("version")
        .and_then(|x| x.as_str())
        .unwrap_or("0.0.0.0")
        .to_string();
    vec![App {
        app_guid: app_guid.to_string(),
        publisher,
        name,
        version,
    }]
}

/// Why [`build_analysis_model`] produced no model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelFailure {
    /// The workspace layout yields no gate model instance id (fail-closed).
    NoModelInstanceId,
    /// The workspace model did not assemble (fail-closed / unreadable).
    AssemblyFailed,
    /// The program engine build failed while the model assembled: an error, since
    /// detectors must never quietly fall back to L3's own calls.
    ProgramBuildFailed(String),
}

/// The model `alsem analyze`'s detectors read, plus the program build's coverage.
pub struct AnalysisModel {
    /// The program engine's `FreshCoverage`, or its build error.
    pub fresh: Result<crate::program::resolve::full::FreshCoverage, String>,
    pub model: Result<L3Resolved, ModelFailure>,
}

/// THE production model builder for `alsem analyze` — and the one the engine-switch
/// harness (`engine::switch_dump`) dumps, so the harness can never measure a copy.
/// Each engine-switch step changes what this builds; nothing else should.
///
/// The program engine's build (B3 Phase A, spec §3/§7) gives the preflight's
/// `FreshCoverage` AND the call resolution the detectors read (attached after the
/// model exists). The program context and the model are resident together only
/// while the adapter runs; the context is dropped before this returns.
///
/// The model is assembled with the al-sem GATE modelInstanceId (content-derived,
/// UNPINNED) so the internal RoutineIds embedded in each finding's rootCauseKey —
/// and therefore the SARIF fingerprint hashed over them — byte-match the goldens.
pub fn build_analysis_model(ws_path: &Path) -> AnalysisModel {
    let (fresh, program) = {
        let _s = pt::span("preflight", "preflight.fresh_program");
        match crate::program::resolve::full::build_program_with_coverage(ws_path) {
            Ok((ctx, report, fc)) => (Ok(fc), Some((ctx, report))),
            Err(e) => (Err(e), None),
        }
    };
    // Its own span: this is a SECOND full `discover_al_files` disk walk of the
    // workspace (plus a sort + SHA-256 over one `ws:<rel>` string per file), and
    // it sat inside `analyze.total`'s unattributed self time until it was
    // measured — see `run_analyze_with_exit`'s `gate.teardown` note.
    let model_instance_id = {
        let _s = pt::span("gate", "gate.model_instance_id");
        compute_gate_model_instance_id(ws_path)
    };
    let Some(model_instance_id) = model_instance_id else {
        return AnalysisModel {
            fresh,
            model: Err(ModelFailure::NoModelInstanceId),
        };
    };
    let resolved = {
        let _s = pt::span("l3", "l3.assemble_resolve");
        assemble_and_resolve_workspace(ws_path, &model_instance_id, false)
    };
    let Some(mut resolved) = resolved else {
        return AnalysisModel {
            fresh,
            model: Err(ModelFailure::AssemblyFailed),
        };
    };
    let Some((ctx, report)) = program else {
        let why = fresh.as_ref().err().cloned().unwrap_or_default();
        return AnalysisModel {
            fresh,
            model: Err(ModelFailure::ProgramBuildFailed(why)),
        };
    };
    crate::engine::l3::program_calls::attach_program_calls(&mut resolved, ctx, report);
    AnalysisModel {
        fresh,
        model: Ok(resolved),
    }
}

/// The analysis coverage `alsem analyze` reports for `resolved`.
///
/// One dependency universe (spec §3): the formatter-visible opaqueApps follows the
/// FRESH snapshot. The L3 gate path resolves source-only with empty deps
/// (src/engine/l3/coverage.rs:239) — its opaque list is structurally empty, and
/// leaving it would let stderr say "N symbol-only apps" while JSON says [].
pub fn analysis_coverage(
    resolved: &L3Resolved,
    ws_path: &Path,
    fresh: &Result<crate::program::resolve::full::FreshCoverage, String>,
) -> AnalysisCoverage {
    let mut c = resolved.project_coverage_disk(ws_path);
    if let Ok(fc) = fresh {
        c.opaque_apps = fc.opaque_apps.clone();
    }
    c
}

/// Run the gate `analyze` pipeline and return the formatted output string WITHOUT the
/// trailing newline (the CLI / caller appends `"\n"`, matching al-sem's
/// `process.stdout.write(`${format(...)}\n`)`).
///
/// Backwards-compatible Stage-1 entry: SARIF only, no exit code. Prefer
/// `run_analyze_with_exit` for the full gate (pr-summary + exit code).
pub fn run_analyze(args: &AnalyzeArgs, default_version: &str) -> Result<String, String> {
    run_analyze_with_exit(args, default_version).map(|(out, _exit, _warn)| out)
}

/// Run the gate `analyze` pipeline, returning `(stdout, exit_code, stderr_warning)`.
///
/// The exit code follows the al-sem precedence:
///   CONFIG_ERROR (3) and ANALYSIS_FAILURE (2) are the CALLER's responsibility (bad
///   flags / a thrown pipeline — neither occurs here: detector/preset resolution errors
///   return `Err`, which the bin maps to 3). This fn computes:
///   PREFLIGHT_FAILED (4) > FINDINGS (1) > CLEAN (0).
///
/// The third tuple field is the preflight degraded warning message (F2: "no silent
/// clean" contract). `Some(msg)` when `pf.degraded`, `None` when coverage is complete.
/// The bin emits `al-sem: warning: {msg}` to stderr; tests may inspect it directly.
/// The warning is emitted REGARDLESS of `--require-dependencies` (only the
/// FAILED→exit-4 path needs that flag).
///
/// `default_version` is the engine's real version, used when no SARIF override is given.
///
/// Errors: detector/preset resolution failures (e.g. unknown preset, `--preset` +
/// `--detector` together). A workspace that fails to assemble (fail-closed / unreadable)
/// yields EMPTY findings (engine-never-throws) — empty SARIF / "no findings" PR-summary,
/// a could-not-verify preflight WARNING (never a fabricated clean — spec §3), and exit
/// CLEAN unless `--require-dependencies` is set, in which case exit PREFLIGHT_FAILED (4).
pub fn run_analyze_with_exit(
    args: &AnalyzeArgs,
    default_version: &str,
) -> Result<(String, u8, Option<String>), String> {
    let _analyze_span = pt::span("analyze", "analyze.total");
    let detectors = resolve_analyze_detectors(args.preset.as_deref(), args.detector.as_deref())?;

    let version = args
        .sarif_version_override
        .clone()
        .unwrap_or_else(|| default_version.to_string());

    // Assemble with the al-sem GATE modelInstanceId (content-derived, UNPINNED) so the
    // internal RoutineIds embedded in each finding's rootCauseKey — and therefore the
    // SARIF fingerprint hashed over them — byte-match the al-sem `analyze` CLI goldens.
    let ws_path = Path::new(&args.workspace);
    let AnalysisModel { fresh, model } = build_analysis_model(ws_path);
    let resolved = match model {
        Ok(r) => r,
        // Fail-closed layout / unreadable workspace → empty output; preflight says
        // could-not-verify (never a fabricated clean — spec §3), gated on `fresh`.
        Err(ModelFailure::NoModelInstanceId | ModelFailure::AssemblyFailed) => {
            return empty_output_result(args, &version, &fresh);
        }
        Err(ModelFailure::ProgramBuildFailed(why)) => {
            return Err(format!(
                "analysis failure — program engine build failed: {why}"
            ));
        }
    };

    // L4 + L5: run the selected detectors. Findings come pre-sorted by
    // (detector, primaryLocationKey, rootCauseKey) with dep-anchored findings already
    // role-scoped out (source-only ⇒ no-op).
    let run = {
        let _s = pt::span("l4_l5", "l4_l5.run_detectors");
        run_detectors(&resolved, &detectors)
    };
    // Capture diagnostics + detector stats for the Json formatter (consumed after filtering).
    //
    // al-sem `analyzeWorkspace` (src/index.ts:287-297) concatenates SIX diagnostic
    // sources, IN THIS ORDER, into the flat `result.diagnostics` the JSON envelope
    // serializes:
    //   1. workspace.diagnostics  — provider (remapped "discover") + index/parse
    //   2. depArtifacts.diagnostics — dependency-artifact resolution
    //   3. summarizeDiagnostics   — L4 `computeSummaries`
    //   4. loadedRootsConfig.diagnostics — roots.config.json LOADER (parse/schema)
    //   5. overlayDiagnostics     — roots.config.json OVERLAY (kinds-mismatch)
    //   6. detectDiagnostics      — L5 detector-emitted (e.g. d43 substrate guard)
    //
    // Each source preserves a deterministic (insertion / sorted-file) order — no
    // HashMap/HashSet iteration leaks into this concatenation (the determinism
    // contract). `infra_diagnostics` is the OVERLAY source (5); `run.diagnostics`
    // is DETECT (6). The TS-order #1 (workspace) goes FIRST so it precedes overlay.
    let run_diagnostics: Vec<crate::engine::l5::registry::Diagnostic> = {
        let _s = pt::span("gate", "gate.workspace_diagnostics");
        let mut all: Vec<crate::engine::l5::registry::Diagnostic> = Vec::new();
        // (1) workspace.diagnostics — provider (discover) + index, computed from disk.
        all.extend(
            crate::engine::gate::workspace_diagnostics::compute_workspace_diagnostics(ws_path),
        );
        // (2) depArtifacts.diagnostics — TRACKED GAP: the gate's source-only pipeline
        //     does not resolve `.app` dependency artifacts, so this source is always
        //     empty here. (When dep resolution is wired into the gate, emit it in this
        //     slot so a dep diagnostic lands in TS order before summarize.)
        // (3) summarizeDiagnostics — WIRED: L4 `compute_summaries*` (run inside
        //     `run_detectors`'s `DetectorContext` build) now surfaces the JACOBI
        //     fixed-point cap-hit here. Empty whenever every SCC converges.
        all.extend(run.summarize_diagnostics.iter().cloned());
        // (4) loadedRootsConfig.diagnostics — TRACKED GAP: the roots.config.json LOADER
        //     diagnostics (parse/schema errors) are not yet surfaced separately from the
        //     OVERLAY diagnostics by `compute_root_classifications`. The overlay
        //     diagnostics (5) below cover the kinds-mismatch case the corpus exercises.
        // (5) overlayDiagnostics — roots.config.json overlay (kinds-mismatch warnings).
        all.extend(resolved.infra_diagnostics.iter().map(|d| {
            crate::engine::l5::registry::Diagnostic {
                severity: d.severity.clone(),
                stage: d.stage.clone(),
                message: d.message.clone(),
            }
        }));
        // (6) detectDiagnostics — L5 detector-emitted (d43 substrate guard, ...).
        all.extend(run.diagnostics.iter().cloned());
        all
    };
    let run_detector_stats = run.detector_stats.clone();

    // Project each finding (display names + 1-based location), preserving order.
    // Span covers projection + filter + scope + limit + baseline + inline-suppression
    // (they share one lexical stretch through `paired`'s successive `retain` passes);
    // the local outlives the natural block boundary, so it is closed explicitly below
    // rather than by the enclosing scope.
    let _project_filter_span = pt::span("gate", "gate.project_filter_scope_baseline_suppress");
    let idx = ProjectionIndex::build(&resolved.workspace.objects, &resolved.workspace.routines);
    let mut paired: Vec<(
        crate::engine::gate::projection::FindingSummary,
        &crate::engine::l5::finding::Finding,
    )> = run
        .findings
        .iter()
        .map(|f| (project_finding(f, &idx), f))
        .collect();

    // --- filter: min-severity, then detector allow-list (only when `--detector` set). ---
    let detector_allow = args.detector.as_ref().map(|d| {
        d.split(',')
            .map(|s| s.trim().to_string())
            .collect::<Vec<_>>()
    });
    let opts = FilterOptions {
        min_severity: args.min_severity.clone(),
        detectors: detector_allow,
    };
    {
        let summaries: Vec<_> = paired.iter().map(|(s, _)| s.clone()).collect();
        let kept_ids: std::collections::HashSet<String> = filter_findings(summaries, &opts)
            .into_iter()
            .map(|s| s.id)
            .collect();
        paired.retain(|(s, _)| kept_ids.contains(&s.id));
    }

    // --- scope: primary drops dependency-anchored findings. Source-only ⇒ keep all. ---
    {
        let summaries: Vec<_> = paired.iter().map(|(s, _)| s.clone()).collect();
        let kept_ids: std::collections::HashSet<String> =
            scope_filter(summaries, args.scope, |_obj_id| false)
                .into_iter()
                .map(|s| s.id)
                .collect();
        paired.retain(|(s, _)| kept_ids.contains(&s.id));
    }

    // --- limit: first N (after scope). Order-preserving prefix. ---
    if let Some(n) = args.limit {
        paired.truncate(n);
    }

    // --- baseline suppression (al-sem index.ts:296-302) ---
    // Load the baseline fingerprint set (empty when no --baseline). Drop any finding
    // whose fingerprint is in it. --update-baseline saves the CURRENT (post-limit) set —
    // the new floor — BEFORE the drop, matching al-sem (`saveBaseline(path, limited)`).
    if let Some(path) = &args.baseline {
        if args.update_baseline {
            let summaries: Vec<_> = paired.iter().map(|(s, _)| s.clone()).collect();
            // Engine-never-throws: a write failure is surfaced as Err to the caller,
            // which the bin maps to a config/IO error message.
            save_baseline(Path::new(path), &summaries)
                .map_err(|e| format!("failed to write baseline '{path}': {e}"))?;
        }
        // A malformed baseline (exists but not valid JSON / non-array fingerprints) is
        // an analysis-failure, not a config error: al-sem's loadBaseline throws and the
        // CLI catch emits "al-sem: analysis failure — <msg>" + exits 2.  We surface
        // this as an Err tagged with the "analysis failure — " prefix so the bin can
        // distinguish it from config errors (exit 3) and use exit 2 instead.
        let baseline =
            load_baseline(Path::new(path)).map_err(|e| format!("analysis failure — {e}"))?;
        let summaries: Vec<_> = paired.iter().map(|(s, _)| s.clone()).collect();
        let kept_ids: std::collections::HashSet<String> = apply_baseline(&summaries, &baseline)
            .into_iter()
            .map(|s| s.id)
            .collect();
        paired.retain(|(s, _)| kept_ids.contains(&s.id));
    }

    // --- inline al-sem-ignore suppression (default-ON, al-sem index.ts:304-319) ---
    // Parsed from workspace source files on disk; only ws: units are scanned. The kept
    // set after this is `newFindings` — the set the exit gate and all formats use.
    if !args.disable_inline_suppression {
        let unit_ids: std::collections::HashSet<String> = paired
            .iter()
            .map(|(s, _)| s.primary_location.file.clone())
            .collect();
        let suppression_map = build_suppression_map(ws_path, unit_ids.iter().map(|s| s.as_str()));
        let summaries: Vec<_> = paired.iter().map(|(s, _)| s.clone()).collect();
        let outcome = apply_inline_suppressions(&summaries, &suppression_map);
        // outcome.kept holds indices into `summaries` (= into `paired`); retain them.
        let keep: std::collections::HashSet<usize> = outcome.kept.into_iter().collect();
        let mut i = 0usize;
        paired.retain(|_| {
            let k = keep.contains(&i);
            i += 1;
            k
        });
    }
    drop(_project_filter_span);

    // --- dependency-coverage preflight (al-sem Task 2) ---
    // NOTE: coverage is computed HERE (before the format switch) so it is available
    // to the Json formatter. The preflight evaluation + exit-code gate follow below.
    let coverage = {
        let _s = pt::span("gate", "gate.coverage");
        analysis_coverage(&resolved, ws_path, &fresh)
    };

    // --- format ---
    let output = {
        let _s = pt::span("gate", "gate.format");
        match args.format {
            OutputFormat::Sarif => {
                let summaries: Vec<_> = paired.iter().map(|(s, _)| s.clone()).collect();
                let raws: Vec<&crate::engine::l5::finding::Finding> =
                    paired.iter().map(|(_, r)| *r).collect();
                format_sarif(&summaries, &raws, &version)
            }
            OutputFormat::PrSummary => {
                let apps = read_workspace_apps(ws_path);
                format_pr_summary(&paired, &resolved.workspace.routines, &apps)
            }
            OutputFormat::Json => {
                let summaries: Vec<_> = paired.iter().map(|(s, _)| s.clone()).collect();
                // Opt-in evidence augmentation (aligned BY INDEX with `summaries`, which is
                // built from `paired` in the same order). None on the default path ⇒ output
                // byte-identical to today + schemaVersion "1.0.0".
                let evidence: Option<Vec<FindingEvidence>> = if args.with_evidence {
                    Some(build_finding_evidence(
                        &paired,
                        &resolved.workspace.routines,
                    ))
                } else {
                    None
                };
                build_analyze_json(&JsonFormatInputs {
                    findings: &summaries,
                    diagnostics: &run_diagnostics,
                    detector_stats: &run_detector_stats,
                    coverage: &coverage,
                    deterministic: args.deterministic,
                    driver_version: driver_version(),
                    evidence: evidence.as_deref(),
                })
            }
            OutputFormat::Terminal => {
                let summaries: Vec<_> = paired.iter().map(|(s, _)| s.clone()).collect();
                // group-by path: only when format==terminal AND group_by is set.
                if let Some(ref by_str) = args.group_by {
                    if let Some(by) = GroupBy::parse(by_str) {
                        format_terminal_grouped(&summaries, &coverage, by)
                    } else {
                        // Invalid group_by — the CLI validates this, so treat as plain.
                        format_terminal(&summaries, &coverage, &run_diagnostics)
                    }
                } else {
                    format_terminal(&summaries, &coverage, &run_diagnostics)
                }
            }
            OutputFormat::Html => {
                let primary_app = resolved.primary_app.as_ref();
                format_html(&HtmlFormatInputs {
                    findings: &paired,
                    resolved: &resolved,
                    coverage: &coverage,
                    primary_app,
                })
            }
        }
    };

    // --- dependency-coverage preflight (al-sem Task 2) ---
    // F2 FIX: always evaluate AND surface pf.degraded as a stderr warning (the
    // "no silent clean" contract — al-sem index.ts:263-264). The warn is INDEPENDENT
    // of --require-dependencies; only the FAILED→exit-4 path needs that flag.
    // We return the warning message as the 3rd tuple field; the bin emits it.
    let pf = evaluate_preflight(&fresh, args.require_dependencies);

    // The degraded warning message — None when coverage is complete (pf.degraded false).
    // Matches al-sem: `if (pf.degraded) process.stderr.write(`al-sem: warning: ${pf.message}\n`)`.
    let stderr_warning: Option<String> = if pf.degraded {
        Some(pf.message.clone())
    } else {
        None
    };

    // --- exit-code gate (precedence: PREFLIGHT_FAILED (4) > FINDINGS (1) > CLEAN (0)). ---
    let exit_code = if pf.failed {
        exit::PREFLIGHT_FAILED
    } else {
        // computeFindingExit over the KEPT findings' severities (no fail-on ⇒ CLEAN).
        let severities: Vec<&str> = paired.iter().map(|(s, _)| s.severity.as_str()).collect();
        compute_finding_exit(&severities, args.fail_on.as_deref())
    };

    // --- teardown, MEASURED ------------------------------------------------
    // These drops happened here anyway: `_analyze_span` is the FIRST local
    // declared, so it is the LAST dropped, and every structure below was
    // already being freed inside the `analyze.total` span. Naming them makes
    // that cost visible instead of leaving it in the span's unattributed self
    // time — a 4-run 8020 profile put `analyze.total`'s own self time at
    // 15.5 % of the whole run (≈ 11.8 s at the 76.2 s median), the SECOND
    // largest region in the profile, with nothing saying what was in it.
    //
    // Order is forced by the borrows, not chosen: `paired` holds `&Finding`s
    // into `run.findings` and `idx` borrows `resolved.workspace`, so both must
    // go before their owners. Nothing here has a `Drop` impl (the only two in
    // the engine are `perf_trace`'s own guards), so this is pure deallocation
    // and reordering it is not observable.
    {
        let _s = pt::span("gate", "gate.teardown");
        drop(paired);
        drop(idx);
        drop(run_diagnostics);
        drop(coverage);
        drop(run);
        drop(resolved);
    }

    Ok((output, exit_code, stderr_warning))
}

/// The empty-output path for a fail-closed / unreadable workspace: empty findings ⇒
/// empty SARIF or the "no findings" PR-summary or a zero-findings Json envelope.
///
/// The preflight is NEVER a fabricated clean here (spec §3): a fail-closed layout
/// is a can't-analyze state, not a verified-empty one, so this now evaluates
/// preflight as could-not-verify (`evaluate_preflight(&Err(reason), ...)`) — the
/// reason is the fresh resolver's OWN error text when `fresh` itself failed too,
/// else the real provider diagnostic's message (the SAME `compute_workspace_diagnostics`
/// call this function threads into the Json/Terminal output, hoisted once and reused
/// here) when one is available, else a fallback string for the (also fail-closed, but
/// diagnostic-free) case where the workspace legitimately has zero readable AL source
/// units. See `evaluate_preflight`'s could-not-verify doc for why this state is
/// first-class and never silently folded into "clean".
///
/// `ws` is the workspace root: the JSON envelope's `diagnostics` array is populated
/// with the real PROVIDER (fail-closed, remapped to `stage:"discover"`) + index
/// diagnostics for this path — al-sem's `workspace.diagnostics` (index.ts:157-173).
/// This is load-bearing: fail-closed (multi-app / id-less / unreadable `app.json`)
/// is a documented core behavior, and dropping its diagnostics would silently hide
/// WHY the model is empty. SARIF / PR-summary carry no diagnostics array, so they
/// are unaffected (parity with al-sem, whose SARIF/pr-summary likewise omit them).
///
/// Terminal and Html are NOT stubs: they render a genuine empty-workspace report
/// (zero findings, zero coverage), the same shape Sarif/PrSummary/Json produce here —
/// every arm returns `Ok`, never `Err`.
pub(crate) fn empty_output_result(
    args: &AnalyzeArgs,
    version: &str,
    fresh: &Result<crate::program::resolve::full::FreshCoverage, String>,
) -> Result<(String, u8, Option<String>), String> {
    let ws_path = Path::new(&args.workspace);
    // Computed ONCE and reused by every arm below (Json/Terminal) AND by the
    // could-not-verify `reason` derivation past the format switch — these are the
    // real provider/index diagnostics for this workspace (spec §3: the fail-closed
    // reason should be the actual diagnostic text, not a generic fallback, whenever
    // one is available).
    let diagnostics =
        crate::engine::gate::workspace_diagnostics::compute_workspace_diagnostics(ws_path);

    let out = match args.format {
        OutputFormat::Sarif => format_sarif(&[], &[], version),
        OutputFormat::PrSummary => format_pr_summary(&[], &[], &[]),
        OutputFormat::Json => {
            // Empty envelope: zero findings, zero stats, zero coverage — but the
            // real provider/index diagnostics (fail-closed reasons) are threaded.
            let empty_coverage = crate::engine::l3::coverage::AnalysisCoverage {
                source_units_total: 0,
                source_units_parsed: 0,
                routines_total: 0,
                routines_body_available: 0,
                routines_parse_incomplete: vec![],
                opaque_apps: vec![],
                unresolved_callsites: vec![],
                dynamic_dispatch_sites: vec![],
            };
            // Fail-closed path has zero findings; --with-evidence still bumps the
            // schemaVersion to "1.1.0" (an empty evidence slice carries no per-finding
            // keys), keeping the flag's schema signal consistent.
            let empty_evidence: Vec<FindingEvidence> = Vec::new();
            build_analyze_json(&JsonFormatInputs {
                findings: &[],
                diagnostics: &diagnostics,
                detector_stats: &[],
                coverage: &empty_coverage,
                deterministic: args.deterministic,
                driver_version: driver_version(),
                evidence: if args.with_evidence {
                    Some(&empty_evidence)
                } else {
                    None
                },
            })
        }
        OutputFormat::Terminal => {
            // Empty workspace → "No findings." terminal output.
            let empty_coverage = crate::engine::l3::coverage::AnalysisCoverage {
                source_units_total: 0,
                source_units_parsed: 0,
                routines_total: 0,
                routines_body_available: 0,
                routines_parse_incomplete: vec![],
                opaque_apps: vec![],
                unresolved_callsites: vec![],
                dynamic_dispatch_sites: vec![],
            };
            format_terminal(&[], &empty_coverage, &diagnostics)
        }
        OutputFormat::Html => {
            // Empty workspace → zero findings + zero coverage HTML report.
            let empty_coverage = crate::engine::l3::coverage::AnalysisCoverage {
                source_units_total: 0,
                source_units_parsed: 0,
                routines_total: 0,
                routines_body_available: 0,
                routines_parse_incomplete: vec![],
                opaque_apps: vec![],
                unresolved_callsites: vec![],
                dynamic_dispatch_sites: vec![],
            };
            // For fail-closed HTML, we need an empty resolved model.
            // The assemble_and_resolve_workspace failed, so build a minimal one.
            let primary_app = read_workspace_apps(ws_path).into_iter().next();
            // Build an empty L3Resolved for the HTML formatter.
            let empty_resolved = crate::engine::l3::l3_workspace::L3Resolved {
                workspace: crate::engine::l3::l3_workspace::L3Workspace {
                    objects: vec![],
                    tables: vec![],
                    routines: vec![],
                },
                root_classifications: vec![],
                primary_app: primary_app.clone(),
                infra_diagnostics: vec![],
                precomputed_calls: None,
            };
            format_html(&HtmlFormatInputs {
                findings: &[],
                resolved: &empty_resolved,
                coverage: &empty_coverage,
                primary_app: primary_app.as_ref(),
            })
        }
    };

    // Fail-closed is a can't-analyze state: preflight must say so, never
    // fabricate clean (spec §3). Reason precedence: (1) the fresh resolver's own
    // error text when `fresh` itself failed too; (2) else the real provider
    // diagnostic's message — the SAME `diagnostics` computed above, so e.g. an
    // id-less root `app.json` surfaces its actual "root app.json at {root} has no
    // string `id`…" text instead of a generic string; (3) else the fallback (some
    // fail-closed paths produce ZERO diagnostics — e.g. a valid app.json with no
    // readable .al at all).
    let reason = match fresh {
        Err(e) => e.clone(),
        Ok(_) => diagnostics
            .first()
            .map(|d| d.message.clone())
            .unwrap_or_else(|| "workspace contained no readable AL source units".to_string()),
    };
    let pf = evaluate_preflight(&Err(reason), args.require_dependencies);
    let exit = if pf.failed {
        exit::PREFLIGHT_FAILED
    } else {
        exit::CLEAN
    };
    Ok((out, exit, Some(pf.message)))
}

/// Build the opt-in `--with-evidence` augmentation for the kept findings, aligned BY
/// INDEX with `paired`. For each finding it computes:
///
/// - `evidence_path`: the internal `Finding.evidence_path` projected to STABLE form
///   (routineIds in `:`-form via the SAME internal→stable map the R4 finding projection
///   uses — `build_routine_stable_map`).
/// - `enclosing_member` / `originating_object`: the POSITION-derived discriminator
///   (RE-1/RE-2). NOT keyed off `enclosingRoutineId` (which COLLAPSES identically for two
///   field triggers). Instead: of the member-trigger routines whose `enclosing_member_range`
///   shares the source_unit_id AND CONTAINS the start (0-based line/col) of the finding's
///   PROJECTED `primaryLocation`, pick the SMALLEST containing range; use its member/
///   originating object. `None` when no member-trigger wrapper contains it (object-level /
///   procedure findings) — the engine never throws.
///
///   The match anchor MUST be the PROJECTED primary — i.e. the `actionable_anchor` when a
///   detector set one, else the internal `primary_location` — because `project_finding`
///   (projection.rs) promotes the actionable anchor to the emitted `primaryLocation` and
///   demotes the original to `terminalLocation`. We must discriminate against the SAME
///   location the consumer sees on `primaryLocation`. (Today the source-only pipeline never
///   sets `actionable_anchor`, so this reduces to `primary_location`; the distinction is
///   load-bearing only for a future cross-dependency run — and this is a FROZEN surface.)
fn build_finding_evidence(
    paired: &[(
        crate::engine::gate::projection::FindingSummary,
        &crate::engine::l5::finding::Finding,
    )],
    routines: &[crate::engine::l3::l3_workspace::L3Routine],
) -> Vec<FindingEvidence> {
    use crate::engine::l2::features::PAnchor;

    let stable_map = crate::engine::l4::summary::build_routine_stable_map(routines);

    // Position index: member-trigger routines that carry a wrapper range.
    let members: Vec<(&PAnchor, &Option<String>, &Option<String>)> = routines
        .iter()
        .filter_map(|r| {
            r.enclosing_member_range
                .as_ref()
                .map(|range| (range, &r.enclosing_member, &r.originating_object))
        })
        .collect();

    paired
        .iter()
        .map(|(_, finding)| {
            // The PROJECTED primaryLocation anchor: the actionable anchor when set, else
            // the internal primary. This is what `project_finding` emits as primaryLocation,
            // so the discriminator stays consistent with what the consumer sees.
            let loc = finding
                .actionable_anchor
                .as_ref()
                .unwrap_or(&finding.primary_location);
            // Find the smallest member-wrapper range (same source unit) containing the
            // finding's projected primaryLocation start. "Smallest" = fewest spanned lines,
            // then fewest spanned columns (a stable, deterministic total order on extent).
            let best = members
                .iter()
                .filter(|(range, _, _)| {
                    range.source_unit_id == loc.source_unit_id
                        && range_contains(range, loc.start_line, loc.start_column)
                })
                .min_by(|(a, _, _), (b, _, _)| range_extent_cmp(a, b));
            let (enclosing_member, originating_object) = match best {
                Some((_, member, oo)) => ((*member).clone(), (*oo).clone()),
                None => (None, None),
            };
            FindingEvidence {
                // Through `evidence_path_of`, not the field: a cohort-bearing d1
                // finding derives its path from `cohort_contexts[0].witness`
                // rather than storing it (see that function's doc). This surface
                // has NO cohort exclusion — unlike `project_finding` — so it is
                // the one place the field change would otherwise have been
                // visible.
                evidence_path: crate::engine::l5::finding::project_evidence_path(
                    &crate::engine::l5::finding::evidence_path_of(finding),
                    &stable_map,
                ),
                enclosing_member,
                originating_object,
            }
        })
        .collect()
}

/// 0-based containment: is `(line, column)` within `[start, end]` of `range` (inclusive)?
fn range_contains(range: &crate::engine::l2::features::PAnchor, line: u32, column: u32) -> bool {
    let after_start = (line, column) >= (range.start_line, range.start_column);
    let before_end = (line, column) <= (range.end_line, range.end_column);
    after_start && before_end
}

/// Total order on range EXTENT (smaller = tighter container): primary by spanned lines,
/// secondary by spanned columns on the start line. Deterministic for the smallest-range
/// selection (two disjoint field triggers never tie because only one contains the point).
fn range_extent_cmp(
    a: &crate::engine::l2::features::PAnchor,
    b: &crate::engine::l2::features::PAnchor,
) -> std::cmp::Ordering {
    let a_lines = a.end_line.saturating_sub(a.start_line);
    let b_lines = b.end_line.saturating_sub(b.start_line);
    a_lines.cmp(&b_lines).then_with(|| {
        let a_cols = a.end_column.saturating_sub(a.start_column);
        let b_cols = b.end_column.saturating_sub(b.start_column);
        a_cols.cmp(&b_cols)
    })
}

/// Aggregate the analyzer diagnostics for a resolved workspace, in al-sem
/// `analyzeWorkspace` (`src/index.ts:287-297`) concat order:
///   1. workspace.diagnostics  (provider/discover + index/parse)
///   2. depArtifacts.diagnostics (gate gap — empty, source-only)
///   3. summarizeDiagnostics    (L4 JACOBI cap-hit — empty unless an SCC fails to converge)
///   4. loadedRootsConfig.diagnostics (gate gap — covered by overlay below)
///   5. overlayDiagnostics      (roots.config kinds-mismatch — `infra_diagnostics`)
///   6. detectDiagnostics       (L5 detector-emitted, e.g. d43 substrate guard)
///
/// This is the source for the cli-b capability-snapshot envelope's `diagnostics`
/// channel (`projectDiagnostics`). It follows the `run_diagnostics` build in
/// `run_analyze`, but the output is the same only for the same `resolved`.
/// Its callers (events, policy, digest, fingerprint) pass an `L3Resolved`
/// without `precomputed_calls`, so the L4 cap-hit (3) and detector (6)
/// diagnostics come from L3's own calls, while `run_analyze`'s come from the
/// program engine's (B3 Phase A). They can differ on the same workspace until
/// those subcommands move to `attach_program_calls` (docs/OUTSTANDING.md).
pub fn compute_analyzer_diagnostics(
    ws_path: &Path,
    resolved: &crate::engine::l3::l3_workspace::L3Resolved,
    detectors: &[crate::engine::l5::registry::Detector],
) -> Vec<crate::engine::l5::registry::Diagnostic> {
    let run = run_detectors(resolved, detectors);
    let mut all: Vec<crate::engine::l5::registry::Diagnostic> = Vec::new();
    // (1) workspace.diagnostics.
    all.extend(crate::engine::gate::workspace_diagnostics::compute_workspace_diagnostics(ws_path));
    // (3) summarizeDiagnostics — L4 JACOBI fixed-point cap-hit.
    all.extend(run.summarize_diagnostics.iter().cloned());
    // (5) overlayDiagnostics — roots.config.json overlay (kinds-mismatch).
    all.extend(resolved.infra_diagnostics.iter().map(|d| {
        crate::engine::l5::registry::Diagnostic {
            severity: d.severity.clone(),
            stage: d.stage.clone(),
            message: d.message.clone(),
        }
    }));
    // (6) detectDiagnostics.
    all.extend(run.diagnostics.iter().cloned());
    all
}

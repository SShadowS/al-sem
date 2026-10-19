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
use std::sync::Arc;

use crate::engine::gate::app_attribution::App;
use crate::engine::gate::baseline::{apply_baseline, load_baseline_bytes, save_baseline};
use crate::engine::gate::exit_code::{compute_finding_exit, exit};
use crate::engine::gate::filter::{FilterOptions, Scope, filter_findings, scope_filter};
use crate::engine::gate::format_html::{HtmlFormatInputs, format_html};
use crate::engine::gate::format_json::{FindingEvidence, JsonFormatInputs, build_analyze_json};
use crate::engine::gate::format_pr_summary::format_pr_summary;
use crate::engine::gate::format_sarif::format_sarif;
use crate::engine::gate::format_terminal::{GroupBy, format_terminal, format_terminal_grouped};
use crate::engine::gate::inline_suppression::{apply_inline_suppressions, build_suppression_map};
use crate::engine::gate::preflight::evaluate_preflight;
use crate::engine::gate::presets::resolve_analyze_detectors;
use crate::engine::gate::projection::{ProjectionIndex, project_finding};
use crate::engine::l5::registry::run_detectors;
use crate::engine::perf_trace as pt;
use crate::program::inputs::{InputNeeds, InputSet};
use crate::program::model::coverage::AnalysisCoverage;
use crate::program::model::workspace::{Model, assemble_and_resolve_workspace_from_program};

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
    /// `--single-app` — analyse the workspace alone, as before engine-switch S8.3.
    /// The default analyses the workspace with the dependencies it requires (the
    /// cross-app model: dependency bodies, events, d13/d16/d17).
    pub single_app: bool,
}

impl AnalyzeArgs {
    /// The command's one read of `workspace` (#160 §4.2.6), with what `analyze`
    /// reads besides the build's inputs: the `--baseline` file.
    #[must_use]
    pub fn read_inputs(&self) -> Arc<InputSet> {
        InputSet::read(
            Path::new(&self.workspace),
            &InputNeeds {
                baseline: self.baseline.as_ref().map(std::path::PathBuf::from),
                ..InputNeeds::default()
            },
        )
    }
}

/// Read the workspace root `app.json` identity (`id` / `publisher` / `name` / `version`)
/// into the gate `App` registry. Mirrors al-sem `WorkspaceProvider.collect`:
///   - defaults: `publisher = "unknown"`, `name = "unknown"`, `version = "0.0.0.0"`.
///   - `appGuid` from the `id` (already validated by `compute_gate_model_instance_id`).
///
/// SOURCE-ONLY: exactly one app per run. Returns an empty registry if `app.json` is
/// unreadable (engine-never-throws; attribution then falls back to "(unknown app)").
/// Reads the `app.json` the command's `InputSet` read (#160).
fn read_workspace_apps(inputs: &InputSet) -> Vec<App> {
    crate::program::model::workspace::read_primary_app(inputs)
        .into_iter()
        .collect()
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

/// What the detectors run over: the workspace's own model, or the cross-app base
/// (engine-switch S8.3, the default).
pub enum AnalysisTarget {
    SingleApp(Box<Model>),
    CrossApp(Box<crate::engine::l4::capability_cone::R3a5CrossAppBase>),
}

impl AnalysisTarget {
    /// The model the projections, diagnostics and formats read.
    #[must_use]
    pub fn resolved(&self) -> &Model {
        match self {
            AnalysisTarget::SingleApp(r) => r.as_ref(),
            AnalysisTarget::CrossApp(b) => &b.resolved,
        }
    }

    /// Build the model `alsem events` / `alsem policy check` read (#70): the
    /// workspace alone (`single_app`), or the cross-app base `alsem analyze`
    /// builds by default. `command` prefixes the error line.
    /// The model-instance id is `inputs.model_instance_id`; a caller that has its
    /// own message for a missing id checks it first.
    pub fn build(
        inputs: &Arc<InputSet>,
        single_app: bool,
        command: &str,
    ) -> Result<AnalysisTarget, String> {
        let model_instance_id = inputs
            .model_instance_id
            .as_deref()
            .ok_or_else(|| format!("{command}: could not compute modelInstanceId"))?;
        if single_app {
            crate::program::model::program_calls::assemble_and_resolve_workspace_program(
                inputs,
                model_instance_id,
                false,
            )
            .map(|r| AnalysisTarget::SingleApp(Box::new(r)))
            .ok_or_else(|| format!("{command}: workspace did not resolve"))
        } else {
            crate::engine::l4::capability_cone::build_cross_app_base(inputs, model_instance_id)
                .map(|b| AnalysisTarget::CrossApp(Box::new(b)))
                .ok_or_else(|| {
                    format!(
                        "{command}: the cross-app model could not be built \
                         (--single-app analyses the workspace alone)"
                    )
                })
        }
    }

    /// What a detector context adds for this target: the cross-app inputs, or none.
    pub(crate) fn cross_app_inputs(
        &self,
    ) -> Option<crate::engine::l5::detector_context::CrossAppInputs<'_>> {
        match self {
            AnalysisTarget::SingleApp(_) => None,
            AnalysisTarget::CrossApp(b) => Some(b.cross_app_inputs()),
        }
    }

    /// The dependencies statement (#66): none single-app, the loaded apps cross-app.
    #[must_use]
    pub fn dependencies(&self) -> crate::program::resolve::full::LoadedDependencies {
        match self {
            AnalysisTarget::SingleApp(_) => Default::default(),
            AnalysisTarget::CrossApp(b) => crate::program::resolve::full::LoadedDependencies {
                cross_app: true,
                apps: b.loaded_apps.clone(),
            },
        }
    }

    /// Whether `r` belongs to the workspace app (#70): every routine single-app;
    /// cross-app, the routines that are not the base's dependency routines.
    #[must_use]
    pub fn is_primary_routine(&self, r: &crate::program::model::workspace::ModelRoutine) -> bool {
        match self {
            AnalysisTarget::SingleApp(_) => true,
            AnalysisTarget::CrossApp(b) => !b.dep_routine_ids.contains(&r.id),
        }
    }
}

/// The model `alsem analyze`'s detectors read, plus the program build's coverage.
pub struct AnalysisModel {
    /// The program engine's `FreshCoverage`, or its build error.
    pub fresh: Result<crate::program::resolve::full::FreshCoverage, String>,
    pub model: Result<AnalysisTarget, ModelFailure>,
    /// The program graph's workspace physical rows (engine-switch S2b.3), kept
    /// past the program context's drop; `None` when the model was not built.
    pub physical: Option<crate::program::physical::PhysicalIndex>,
    /// `census::object_fact_census` over the model and the program graph
    /// (engine-switch S5.4); `None` when the model was not built.
    pub object_facts: Option<Vec<String>>,
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
///
/// `single_app == false` (analyze's default since engine-switch S8.3) builds the
/// cross-app base instead: one `FULL` program build, the workspace plus the
/// dependency code it demands. The switch harness dumps the single-app model (it
/// needs the physical rows); the cross-app side is measured with
/// `aldump --r4-findings-cross-app`.
pub fn build_analysis_model(inputs: &Arc<InputSet>, single_app: bool) -> AnalysisModel {
    if !single_app {
        return build_cross_app_analysis_model(inputs);
    }
    let (fresh, program) = {
        let _s = pt::span("preflight", "preflight.fresh_program");
        match crate::program::resolved::ResolvedProgram::build(
            inputs,
            crate::program::profile::BuildProfile {
                dependency_bodies: crate::program::profile::DependencyBodies::Summary,
            },
        ) {
            Ok(program) => (Ok(program.meta.coverage.clone()), Some(program)),
            Err(e) => (Err(e), None),
        }
    };
    // The id `InputSet::read` computed (#160): no second disk walk.
    let Some(model_instance_id) = inputs.model_instance_id.as_deref() else {
        return AnalysisModel {
            fresh,
            model: Err(ModelFailure::NoModelInstanceId),
            physical: None,
            object_facts: None,
        };
    };
    let Some(mut program) = program else {
        // A workspace with no app-scoped AL file is the empty-output case;
        // otherwise the failed program build is an error. (Engine-switch S9.1:
        // this used to assemble L3's model from disk only to tell the two apart.)
        // The same rule as the `NestedApps::Skip` walk it replaces: a failed walk
        // left `al_files` empty, so it does not assemble either.
        let assembles = inputs.app_scoped_files().next().is_some();
        let model = if assembles {
            let why = fresh.as_ref().err().cloned().unwrap_or_default();
            Err(ModelFailure::ProgramBuildFailed(why))
        } else {
            Err(ModelFailure::AssemblyFailed)
        };
        return AnalysisModel {
            fresh,
            model,
            physical: None,
            object_facts: None,
        };
    };
    let body = program.body.take().expect("built once");
    // Engine-switch S2a: the model is projected from the program engine's parse.
    let resolved = {
        let _s = pt::span("l3", "l3.assemble_resolve");
        assemble_and_resolve_workspace_from_program(
            inputs,
            model_instance_id,
            false,
            &body.ctx,
            &body.parenless_calls,
        )
    };
    let Some(rows) = resolved else {
        return AnalysisModel {
            fresh,
            model: Err(ModelFailure::AssemblyFailed),
            physical: None,
            object_facts: None,
        };
    };
    // Taken before the adapter consumes (and drops) the program context.
    let physical = Some(body.ctx.graph().workspace_rows.clone());
    let object_facts = Some(crate::program::model::census::object_fact_census(
        &rows.workspace,
        body.ctx.graph(),
    ));
    // The same build's coverage (#91).
    let resolved = crate::program::model::program_calls::attach_program_calls(
        &program.meta.inputs,
        rows,
        body,
        &program.meta.coverage,
    );
    AnalysisModel {
        fresh,
        model: Ok(AnalysisTarget::SingleApp(Box::new(resolved))),
        physical,
        object_facts,
    }
}

/// [`build_analysis_model`]'s cross-app half. A workspace whose cross-app build
/// fails is classified as the single-app build would classify it; when that build
/// succeeds, the cross-app failure is a program-build failure (never a quiet
/// single-app run).
fn build_cross_app_analysis_model(inputs: &Arc<InputSet>) -> AnalysisModel {
    let base = inputs.model_instance_id.as_deref().and_then(|mi| {
        let _s = pt::span("crossapp", "crossapp.base_total");
        crate::engine::l4::capability_cone::build_cross_app_base(inputs, mi)
    });
    match base {
        Some(base) => AnalysisModel {
            fresh: Ok(base.coverage.clone()),
            model: Ok(AnalysisTarget::CrossApp(Box::new(base))),
            physical: None,
            object_facts: None,
        },
        None => {
            let single = build_analysis_model(inputs, true);
            let model = match single.model {
                Ok(_) => Err(ModelFailure::ProgramBuildFailed(
                    "the cross-app model could not be built".to_string(),
                )),
                Err(e) => Err(e),
            };
            AnalysisModel {
                fresh: single.fresh,
                model,
                physical: None,
                object_facts: None,
            }
        }
    }
}

/// `--scope primary`'s dependency test (engine-switch S5.3): a finding's object is a
/// dependency object when the model object's app is not the primary app. It used to
/// be `|_| false`. The analyze model holds the primary app only until S7/S8 add
/// dependency routines, so today this keeps every finding, as before; it is the
/// rule those steps rely on. With no primary app identity (no `app.json` id) nothing
/// is treated as a dependency.
pub fn dependency_object_predicate<'a>(
    resolved: &'a Model,
    idx: &'a ProjectionIndex<'a>,
) -> impl Fn(&str) -> bool + 'a {
    let primary = resolved.primary_app.as_ref().map(|a| a.app_guid.as_str());
    move |obj_id: &str| match (primary, idx.objects_by_id.get(obj_id)) {
        (Some(p), Some(o)) => !o.app_guid.eq_ignore_ascii_case(p),
        _ => false,
    }
}

/// The analysis coverage `alsem analyze` reports for `resolved`.
///
/// One dependency universe (spec §3): the formatter-visible opaqueApps follows the
/// FRESH snapshot. The L3 gate path resolves source-only with empty deps
/// (src/engine/l3/coverage.rs:239) — its opaque list is structurally empty, and
/// leaving it would let stderr say "N symbol-only apps" while JSON says [].
pub fn analysis_coverage(
    resolved: &Model,
    inputs: &InputSet,
    fresh: &Result<crate::program::resolve::full::FreshCoverage, String>,
) -> AnalysisCoverage {
    let mut c = resolved.project_coverage_inputs(inputs);
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
pub fn run_analyze(
    inputs: &Arc<InputSet>,
    args: &AnalyzeArgs,
    default_version: &str,
) -> Result<String, String> {
    run_analyze_with_exit(inputs, args, default_version).map(|(out, _exit, _warn)| out)
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
/// `inputs` is the command's one read of the workspace (#160 §4.2.6): every reader
/// below reads it, never the disk. `inputs.workspace` is `args.workspace`.
///
/// Errors: detector/preset resolution failures (e.g. unknown preset, `--preset` +
/// `--detector` together). A workspace that fails to assemble (fail-closed / unreadable)
/// yields EMPTY findings (engine-never-throws) — empty SARIF / "no findings" PR-summary,
/// a could-not-verify preflight WARNING (never a fabricated clean — spec §3), and exit
/// CLEAN unless `--require-dependencies` is set, in which case exit PREFLIGHT_FAILED (4).
pub fn run_analyze_with_exit(
    inputs: &Arc<InputSet>,
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
    let AnalysisModel { fresh, model, .. } = build_analysis_model(inputs, args.single_app);
    let target = match model {
        Ok(t) => t,
        // Fail-closed layout / unreadable workspace → empty output; preflight says
        // could-not-verify (never a fabricated clean — spec §3), gated on `fresh`.
        Err(ModelFailure::NoModelInstanceId | ModelFailure::AssemblyFailed) => {
            return empty_output_result(inputs, args, &version, &fresh);
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
    let resolved = target.resolved();
    let run = {
        let _s = pt::span("l4_l5", "l4_l5.run_detectors");
        match &target {
            AnalysisTarget::SingleApp(r) => run_detectors(r, &detectors),
            // The cross-app runner drops dependency-anchored findings itself for
            // `--scope primary` (routine, object and `dep:` location); `--scope all`
            // keeps them for the scope filter below.
            AnalysisTarget::CrossApp(base) => crate::engine::l5::registry::run_detectors_cross_app(
                base,
                &detectors,
                args.scope == Scope::Primary,
            ),
        }
    };
    // Capture diagnostics + detector stats for the Json formatter (consumed after
    // filtering), through the aggregation every other command uses, over THIS
    // run (the selected detectors), so analyze and the rest report one set (#91).
    let run_diagnostics: Vec<crate::engine::l5::registry::Diagnostic> = {
        let _s = pt::span("gate", "gate.workspace_diagnostics");
        analyzer_diagnostics_of_run(resolved, &run)
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
        let is_dependency = dependency_object_predicate(resolved, &idx);
        let kept_ids: std::collections::HashSet<String> =
            scope_filter(summaries, args.scope, is_dependency)
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
        let summaries: Vec<_> = paired.iter().map(|(s, _)| s.clone()).collect();
        let baseline = if args.update_baseline {
            // Engine-never-throws: a write failure is surfaced as Err to the caller,
            // which the bin maps to a config/IO error message.
            save_baseline(Path::new(path), &summaries)
                .map_err(|e| format!("failed to write baseline '{path}': {e}"))?;
            // The set applied is the set just written: every current finding. The
            // old file (`inputs.baseline`) is never read, so a malformed one never
            // errors, as when the file was overwritten before being read back (#160).
            summaries.iter().map(|s| s.fingerprint.clone()).collect()
        } else {
            // A malformed baseline (exists but not valid JSON / non-array
            // fingerprints) is an analysis-failure, not a config error: al-sem's
            // loadBaseline throws and the CLI catch emits "al-sem: analysis failure —
            // <msg>" + exits 2. We surface this as an Err tagged with the "analysis
            // failure — " prefix so the bin can distinguish it from config errors
            // (exit 3) and use exit 2 instead. The bytes are the ones the command's
            // `InputSet` read (#160).
            let file = inputs.baseline.as_ref().expect(
                "--baseline set but the InputSet was read without it (AnalyzeArgs::read_inputs)",
            );
            load_baseline_bytes(file).map_err(|e| format!("analysis failure — {e}"))?
        };
        let kept_ids: std::collections::HashSet<String> = apply_baseline(&summaries, &baseline)
            .into_iter()
            .map(|s| s.id)
            .collect();
        paired.retain(|(s, _)| kept_ids.contains(&s.id));
    }

    // --- inline al-sem-ignore suppression (default-ON, al-sem index.ts:304-319) ---
    // Parsed from the workspace source texts the `InputSet` read; only ws: units are
    // scanned. The kept set after this is `newFindings` — the set the exit gate and
    // all formats use.
    if !args.disable_inline_suppression {
        let unit_ids: std::collections::HashSet<String> = paired
            .iter()
            .map(|(s, _)| s.primary_location.file.clone())
            .collect();
        let suppression_map = build_suppression_map(inputs, unit_ids.iter().map(|s| s.as_str()));
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
        analysis_coverage(resolved, inputs, &fresh)
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
                let apps = read_workspace_apps(inputs);
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
                    driver_version: inputs.driver_version.clone(),
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
                    resolved,
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
        drop(target);
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
    inputs: &InputSet,
    args: &AnalyzeArgs,
    version: &str,
    fresh: &Result<crate::program::resolve::full::FreshCoverage, String>,
) -> Result<(String, u8, Option<String>), String> {
    // Computed ONCE and reused by every arm below (Json/Terminal) AND by the
    // could-not-verify `reason` derivation past the format switch — these are the
    // real provider/index diagnostics for this workspace (spec §3: the fail-closed
    // reason should be the actual diagnostic text, not a generic fallback, whenever
    // one is available). The index rows come from the program build's parse; a
    // failed build parsed nothing and gives none (#160 §4.2.4: no fallback parse).
    let diagnostics = crate::engine::gate::workspace_diagnostics::compute_workspace_diagnostics(
        inputs,
        fresh
            .as_ref()
            .ok()
            .map(|f| f.files_without_object.as_slice()),
    );

    let out = match args.format {
        OutputFormat::Sarif => format_sarif(&[], &[], version),
        OutputFormat::PrSummary => format_pr_summary(&[], &[], &[]),
        OutputFormat::Json => {
            // Empty envelope: zero findings, zero stats, zero coverage — but the
            // real provider/index diagnostics (fail-closed reasons) are threaded.
            let empty_coverage = crate::program::model::coverage::AnalysisCoverage {
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
                driver_version: inputs.driver_version.clone(),
                evidence: if args.with_evidence {
                    Some(&empty_evidence)
                } else {
                    None
                },
            })
        }
        OutputFormat::Terminal => {
            // Empty workspace → "No findings." terminal output.
            let empty_coverage = crate::program::model::coverage::AnalysisCoverage {
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
            let empty_coverage = crate::program::model::coverage::AnalysisCoverage {
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
            let primary_app = read_workspace_apps(inputs).into_iter().next();
            // Build an empty Model for the HTML formatter.
            let empty_resolved = crate::program::model::workspace::Model {
                workspace: crate::program::model::workspace::ModelEntities {
                    objects: vec![],
                    tables: vec![],
                    routines: vec![],
                },
                root_classifications: vec![],
                primary_app: primary_app.clone(),
                infra_diagnostics: vec![],
                workspace_diagnostics: vec![],
                dependency_diagnostics: vec![],
                calls: Default::default(),
                events: Default::default(),
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
    routines: &[crate::program::model::workspace::ModelRoutine],
) -> Vec<FindingEvidence> {
    use crate::program::body::features::PAnchor;

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
fn range_contains(range: &crate::program::body::features::PAnchor, line: u32, column: u32) -> bool {
    let after_start = (line, column) >= (range.start_line, range.start_column);
    let before_end = (line, column) <= (range.end_line, range.end_column);
    after_start && before_end
}

/// Total order on range EXTENT (smaller = tighter container): primary by spanned lines,
/// secondary by spanned columns on the start line. Deterministic for the smallest-range
/// selection (two disjoint field triggers never tie because only one contains the point).
fn range_extent_cmp(
    a: &crate::program::body::features::PAnchor,
    b: &crate::program::body::features::PAnchor,
) -> std::cmp::Ordering {
    let a_lines = a.end_line.saturating_sub(a.start_line);
    let b_lines = b.end_line.saturating_sub(b.start_line);
    a_lines.cmp(&b_lines).then_with(|| {
        let a_cols = a.end_column.saturating_sub(a.start_column);
        let b_cols = b.end_column.saturating_sub(b.start_column);
        a_cols.cmp(&b_cols)
    })
}

/// The analyzer diagnostics for a resolved single-app workspace:
/// [`analyzer_diagnostics_of_run`] over a run of the default detector set.
///
/// This is the source for the cli-b capability-snapshot envelope's `diagnostics`
/// channel (`projectDiagnostics`). It builds them as `run_analyze` does, but the
/// output is the same only for the same `resolved`.
/// Since engine-switch S6 its callers (fingerprint, prove, diff, and the
/// `--single-app` modes of events, policy and digest) pass a program-backed
/// single-app model
/// (`program_calls::assemble_and_resolve_workspace_program`), the kind
/// `run_analyze` builds for `--single-app`. The workspace (1), dependency (2)
/// and overlay (5) sources therefore agree with `alsem analyze --single-app`
/// whatever detectors it runs; the detect (6) and summarize (3) sources agree
/// only with an analyze run of the default detector set, since this function
/// always runs the default set. They
/// need not agree with plain `alsem analyze`, which is cross-app by default;
/// the cross-app twin is [`compute_analyzer_diagnostics_cross_app`].
///
/// It runs the registry's default set (`detectors::default_detectors()`, what
/// `alsem analyze` runs with no `--detector`/`--preset`) itself. There is no
/// detector parameter, so no caller outside this module can pick another set (#68).
pub fn compute_analyzer_diagnostics(
    resolved: &crate::program::model::workspace::Model,
) -> Vec<crate::engine::l5::registry::Diagnostic> {
    let run = run_detectors(resolved, &crate::engine::l5::detectors::default_detectors());
    analyzer_diagnostics_of_run(resolved, &run)
}

/// The target's analyzer diagnostics ([`compute_analyzer_diagnostics`] or, for a
/// cross-app base, [`compute_analyzer_diagnostics_cross_app`]) as
/// `<severity>: <message>` lines, the stderr form `alsem events` and
/// `alsem policy` print (and match `error:` against under `--strict`).
pub fn analyzer_diagnostic_lines(target: &AnalysisTarget) -> Vec<String> {
    match target {
        AnalysisTarget::SingleApp(r) => compute_analyzer_diagnostics(r),
        AnalysisTarget::CrossApp(b) => compute_analyzer_diagnostics_cross_app(b),
    }
    .iter()
    .map(|d| format!("{}: {}", d.severity, d.message))
    .collect()
}

/// [`compute_analyzer_diagnostics`] for a cross-app base (`alsem digest`'s
/// default mode, #66): the default set run by the cross-app runner, the
/// combination `alsem analyze` runs by default.
pub fn compute_analyzer_diagnostics_cross_app(
    base: &crate::engine::l4::capability_cone::R3a5CrossAppBase,
) -> Vec<crate::engine::l5::registry::Diagnostic> {
    let run = crate::engine::l5::registry::run_detectors_cross_app(
        base,
        &crate::engine::l5::detectors::default_detectors(),
        true,
    );
    analyzer_diagnostics_of_run(&base.resolved, &run)
}

/// THE analyzer-diagnostics aggregation, over a detector run: `alsem analyze`
/// (over the run of its selected detectors), [`compute_analyzer_diagnostics`] and
/// [`compute_analyzer_diagnostics_cross_app`] (over a default-set run) all build
/// their diagnostics here, so every command reports the same set (#91).
///
/// al-sem `analyzeWorkspace` (src/index.ts:287-297) concatenates SIX diagnostic
/// sources, IN THIS ORDER, into the flat `result.diagnostics` the JSON envelope
/// serializes:
///   1. workspace.diagnostics  — provider (remapped "discover") + index/parse
///   2. depArtifacts.diagnostics — dependency-artifact resolution
///   3. summarizeDiagnostics   — L4 `computeSummaries`
///   4. loadedRootsConfig.diagnostics — roots.config.json LOADER (parse/schema)
///   5. overlayDiagnostics     — roots.config.json OVERLAY (kinds-mismatch)
///   6. detectDiagnostics      — L5 detector-emitted (e.g. d43 substrate guard)
///
/// Each source preserves a deterministic (insertion / sorted-file) order — no
/// HashMap/HashSet iteration leaks into this concatenation (the determinism
/// contract). `dependency_diagnostics` is the DEPENDENCY source (2),
/// `infra_diagnostics` the OVERLAY source (5), `run.diagnostics` DETECT (6). The
/// TS-order #1 (workspace) goes FIRST so it precedes overlay.
fn analyzer_diagnostics_of_run(
    resolved: &crate::program::model::workspace::Model,
    run: &crate::engine::l5::registry::RunOutput,
) -> Vec<crate::engine::l5::registry::Diagnostic> {
    let mut all: Vec<crate::engine::l5::registry::Diagnostic> = Vec::new();
    // (1) workspace.diagnostics — provider (discover) + index, kept on the model
    //     by `attach_program_calls` from the command's `InputSet` and the build's
    //     objectless-file list (#160 §4.2.4).
    all.extend(resolved.workspace_diagnostics.iter().cloned());
    // (2) dependency diagnostics — the program build's dependency ledger
    //     (engine-switch S5.2b): missing, older-than-declared, unreadable and
    //     unidentified dependency packages, kept on the model by
    //     `attach_program_calls` (#91).
    all.extend(resolved.dependency_diagnostics.iter().cloned());
    // (3) summarizeDiagnostics — L4 `compute_summaries*` (run inside
    //     `run_detectors`'s `DetectorContext` build) surfaces the JACOBI
    //     fixed-point cap-hit here. Empty whenever every SCC converges.
    all.extend(run.summarize_diagnostics.iter().cloned());
    // (4) loadedRootsConfig.diagnostics — TRACKED GAP: the roots.config.json LOADER
    //     diagnostics (parse/schema errors) are not yet surfaced separately from the
    //     OVERLAY diagnostics by `compute_root_classifications`. The overlay
    //     diagnostics (5) below cover the kinds-mismatch case the corpus exercises.
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

#[cfg(test)]
mod tests {
    use crate::engine::l5::registry::{Diagnostic, RunOutput};
    use crate::program::model::workspace::{Model, ModelEntities};

    /// #91: the shared aggregation returns every source, in analyze's order:
    /// (1) workspace, (2) dependency, (3) summarize, (5) overlay, (6) detect.
    /// Every source is a hand-stated row; (1) is the model's
    /// `workspace_diagnostics` (#160 §4.2.4). Mutation: drop (2) from
    /// `analyzer_diagnostics_of_run` -> this fails.
    #[test]
    fn analyzer_diagnostics_of_run_keeps_every_source_in_order() {
        let d = |stage: &str| Diagnostic {
            severity: "warning".to_string(),
            stage: stage.to_string(),
            message: format!("{stage} message"),
        };
        let model = Model {
            workspace: ModelEntities {
                objects: vec![],
                tables: vec![],
                routines: vec![],
            },
            root_classifications: vec![],
            primary_app: None,
            infra_diagnostics: vec![crate::engine::root_classification::InfraDiagnostic {
                severity: "warning".to_string(),
                stage: "overlay".to_string(),
                message: "overlay message".to_string(),
            }],
            workspace_diagnostics: vec![d("discover")],
            dependency_diagnostics: vec![d("dependencies")],
            calls: Default::default(),
            events: Default::default(),
        };
        let run = RunOutput {
            findings: vec![],
            diagnostics: vec![d("detect")],
            detector_stats: vec![],
            summarize_diagnostics: vec![d("summarize")],
            d1_cohort_index: None,
        };
        let stages: Vec<String> = super::analyzer_diagnostics_of_run(&model, &run)
            .into_iter()
            .map(|d| d.stage)
            .collect();
        assert_eq!(
            stages,
            ["discover", "dependencies", "summarize", "overlay", "detect"]
        );
    }

    /// #91: `alsem analyze` builds its diagnostics through the one shared
    /// aggregation, not an inline copy of it. Its output cannot tell the two
    /// apart, so the call itself is pinned. Mutation: restore the inline
    /// `compute_workspace_diagnostics(` / `dependency_diagnostics(` block in
    /// `run_analyze_with_exit`, or read `.workspace_diagnostics` there (#160:
    /// source (1) is now a model field) -> this fails.
    #[test]
    fn run_analyze_with_exit_uses_the_shared_aggregation() {
        let src = include_str!("run.rs");
        let start = src
            .find("pub fn run_analyze_with_exit(")
            .expect("run_analyze_with_exit");
        let body = &src[start..];
        // Cut at the next top-level item, not the first column-0 `}`, which a raw
        // string inside the fn could hold.
        let end = ["\npub fn ", "\nfn ", "\n#[", "\n///"]
            .iter()
            .filter_map(|m| body[1..].find(m))
            .min()
            .expect("an item after run_analyze_with_exit");
        // The trace span's NAME holds `.workspace_diagnostics` too; it is a
        // label, not a read of the field, so it is cut out before the scan.
        let span = "\"gate.workspace_diagnostics\"";
        assert_eq!(body[..end + 1].matches(span).count(), 1, "the span label");
        let body = &body[..end + 1].replace(span, "");
        // An anchor from the function's last statement proves the cut did not
        // stop early.
        assert!(
            body.contains("Ok((output, exit_code, stderr_warning))"),
            "the cut must reach run_analyze_with_exit's later code"
        );
        assert!(
            body.contains("analyzer_diagnostics_of_run("),
            "run_analyze_with_exit must call analyzer_diagnostics_of_run"
        );
        for banned in [
            "compute_workspace_diagnostics(",
            ".workspace_diagnostics",
            "dependency_diagnostics(",
            "summarize_diagnostics",
            "infra_diagnostics",
        ] {
            assert!(
                !body.contains(banned),
                "run_analyze_with_exit calls {banned} itself; aggregate through \
                 analyzer_diagnostics_of_run so every command reports the same set"
            );
        }
    }
}

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

use crate::engine::batch::job::{
    Doc, Job, JobFailure, PlanPart, ResolveCx, SideDoc, SideIx, ViewCx, ViewDemand, ViewFailure,
    ViewKey, ViewNeeds, ViewUniverse, WorkspaceRead,
};
use crate::engine::batch::output::{JobOutput, WriteAction};
use crate::engine::gate::app_attribution::App;
use crate::engine::gate::baseline::{apply_baseline, load_baseline_bytes, serialize_baseline};
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
use crate::engine::session::{RunKey, View, analyze_plan};
use crate::program::inputs::{PolicyRequest, RequestInputs, WorkspaceInputs, WorkspaceNeeds};
use crate::program::model::coverage::AnalysisCoverage;
use crate::program::model::role::Role;
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
    /// The command's one read of `workspace` (#160 §4.2.6).
    #[must_use]
    pub fn read_inputs(&self) -> Arc<WorkspaceInputs> {
        WorkspaceInputs::read(Path::new(&self.workspace), &WorkspaceNeeds::default())
    }

    /// What `analyze` reads besides the workspace (#277 D3): the `--baseline` file.
    #[must_use]
    pub fn read_request(&self) -> RequestInputs {
        RequestInputs::read(
            Path::new(&self.workspace),
            PolicyRequest::None,
            self.baseline.as_ref().map(std::path::PathBuf::from),
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
/// Reads the `app.json` the command's `WorkspaceInputs` read (#160).
fn read_workspace_apps(inputs: &WorkspaceInputs) -> Vec<App> {
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

/// Why [`AnalysisTarget::build_target`] built no target.
#[derive(Debug)]
pub enum TargetFailure {
    NoModelInstanceId,
    /// The single-app workspace did not resolve.
    DidNotResolve,
    CrossApp(crate::program::model::program_calls::CrossAppFailure),
}

impl TargetFailure {
    /// The error line `command` prints.
    pub fn message(&self, command: &str) -> String {
        match self {
            TargetFailure::NoModelInstanceId => {
                format!("{command}: could not compute modelInstanceId")
            }
            TargetFailure::DidNotResolve => format!("{command}: workspace did not resolve"),
            TargetFailure::CrossApp(e) => AnalysisTarget::cross_app_failure_message(command, e),
        }
    }
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
    /// Through the one view builder ([`build_view_model`]).
    pub fn build(
        inputs: &Arc<WorkspaceInputs>,
        single_app: bool,
        command: &str,
    ) -> Result<AnalysisTarget, String> {
        Self::build_with(inputs, single_app, false, command)
    }

    /// [`AnalysisTarget::build`] with `--no-roots-config` (#160 step 9 D1): both
    /// arms skip the `roots.config.json` overlay when `no_roots_config`.
    pub fn build_with(
        inputs: &Arc<WorkspaceInputs>,
        single_app: bool,
        no_roots_config: bool,
        command: &str,
    ) -> Result<AnalysisTarget, String> {
        let key = ViewKey::new(&inputs.workspace, single_app, no_roots_config);
        match build_view_model(inputs, &key, ViewNeeds::MODEL, |_| {}) {
            Ok(view) => Ok(view.expect("a model view").target),
            Err(f) => Err(f.target.message(command)),
        }
    }

    /// The error line of a command whose cross-app build failed. A failed program
    /// build carries its own error, as `alsem analyze`'s Build row does.
    pub(crate) fn cross_app_failure_message(
        command: &str,
        failure: &crate::program::model::program_calls::CrossAppFailure,
    ) -> String {
        use crate::program::model::program_calls::CrossAppFailure;
        let head = format!(
            "{command}: the cross-app model could not be built \
         (--single-app analyses the workspace alone)"
        );
        match failure {
            CrossAppFailure::Build(why) => format!("{head}: program engine build failed: {why}"),
            CrossAppFailure::Assembly { .. } | CrossAppFailure::NoPrimaryApp { .. } => head,
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

    /// Whether `r` belongs to the workspace app (#70), by THE role rule
    /// (`Model::routine_role`, #220). A single-app model holds only the
    /// workspace's routines, so every one is primary there.
    #[must_use]
    pub fn is_primary_routine(&self, r: &crate::program::model::workspace::ModelRoutine) -> bool {
        self.resolved().routine_role(r) == Role::Primary
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
pub fn build_analysis_model(inputs: &Arc<WorkspaceInputs>, single_app: bool) -> AnalysisModel {
    let key = ViewKey::new(&inputs.workspace, single_app, false);
    match build_view_model(inputs, &key, ViewNeeds::ANALYZE, |_| {}) {
        Ok(view) => {
            let view = view.expect("a model view");
            AnalysisModel {
                fresh: view.fresh.expect("analyze's extras"),
                model: Ok(view.target),
                physical: view.physical,
                object_facts: view.object_facts,
            }
        }
        Err(f) => {
            let (fresh, model) = *f.analyze.expect("analyze's extras");
            AnalysisModel {
                fresh,
                model: Err(model),
                physical: None,
                object_facts: None,
            }
        }
    }
}

/// A view's model (#277 D4.1), and analyze's extras when its needs ask them:
/// the program coverage, and (single-app) the physical rows and the
/// object-fact census.
pub struct ViewModel {
    pub target: AnalysisTarget,
    pub fresh: Option<Result<crate::program::resolve::full::FreshCoverage, String>>,
    pub physical: Option<crate::program::physical::PhysicalIndex>,
    pub object_facts: Option<Vec<String>>,
}

/// THE view builder (#277 D4.1): every command's model is built here, so
/// views shared by several commands build one model. `needs` is the union of
/// the view's demands. A graph demand (`needs.graph`, cross-app only) gets
/// the program, or its build error, through `on_program` before the model is
/// assembled from it (D9); a view with no model demand builds the program
/// alone (`Ok(None)`).
///
/// A missing model-instance id: `AnalysisTarget`'s callers report it before
/// any build; analyze builds the single-app program first, for the coverage
/// its warning reads, or, beside graph, reads that of graph's program.
pub fn build_view_model(
    inputs: &Arc<WorkspaceInputs>,
    key: &ViewKey,
    needs: ViewNeeds,
    on_program: impl FnOnce(Result<&crate::program::resolved::ResolvedProgram, &str>),
) -> Result<Option<ViewModel>, ViewFailure> {
    use crate::program::resolved::{ProgramScope, ResolvedProgram};
    let extras = needs.analyze_extras;
    let single_app = key.universe == ViewUniverse::SingleApp;
    let model_instance_id = inputs.model_instance_id.as_deref();
    let no_id = || ViewFailure {
        target: TargetFailure::NoModelInstanceId,
        analyze: None,
        inputs: Arc::clone(inputs),
    };
    if single_app || (!needs.graph && model_instance_id.is_none()) {
        assert!(
            !needs.graph,
            "the call-graph export reads a cross-app program"
        );
        if !extras && model_instance_id.is_none() {
            return Err(no_id());
        }
        // A cross-app analyze with no id: one build under the single-app
        // profile, never `FULL`'s dependency bodies.
        return build_single_app(inputs, key.no_roots_config, extras).map(Some);
    }
    // The base span covers the program build and the base's, never the graph
    // export lent in between (#277 R6-5); a graph-only view builds no base.
    let base_span = || {
        needs
            .model
            .then(|| pt::span("crossapp", "crossapp.base_total"))
    };
    let mut span = base_span();
    let program = ResolvedProgram::build(inputs, ProgramScope::CrossApp);
    if needs.graph {
        drop(span);
        on_program(program.as_ref().map_err(String::as_str));
        span = base_span();
    }
    let _span = span;
    if !needs.model {
        return Ok(None);
    }
    let Some(mi) = model_instance_id else {
        // Beside graph, analyze's warning reads the coverage of the program
        // graph read: one build per view (A12.6; #277 final r1 FR-6). It is
        // reduced before the dependency bodies resolve; its equality with the
        // single-app build's is pinned on a workspace with a source-bearing
        // dependency (`graph_and_analyze_without_an_id_build_once`).
        return Err(ViewFailure {
            analyze: extras.then(|| {
                let coverage = program.map(|p| p.into_parts().0.coverage);
                Box::new((coverage, ModelFailure::NoModelInstanceId))
            }),
            ..no_id()
        });
    };
    use crate::program::model::program_calls::CrossAppFailure;
    match crate::engine::l4::capability_cone::build_cross_app_base_from(
        program,
        inputs,
        mi,
        key.no_roots_config,
    ) {
        Ok(base) => Ok(Some(ViewModel {
            fresh: extras.then(|| Ok(base.coverage.clone())),
            target: AnalysisTarget::CrossApp(Box::new(base)),
            physical: None,
            object_facts: None,
        })),
        // A failed cross-app build is classified from its own `Result` (#160
        // spec §4.4.5), with no second build.
        Err(e) => {
            let analyze = extras.then(|| match &e {
                CrossAppFailure::Build(why) => {
                    (Err(why.clone()), program_build_failure(inputs, why.clone()))
                }
                CrossAppFailure::Assembly { fresh } => {
                    (Ok((**fresh).clone()), ModelFailure::AssemblyFailed)
                }
                // Unreachable from step 1 (the id and `primary_app` share one
                // read); fail closed with today's outcome.
                CrossAppFailure::NoPrimaryApp { fresh } => (
                    Ok((**fresh).clone()),
                    ModelFailure::ProgramBuildFailed(
                        "the cross-app model could not be built".to_string(),
                    ),
                ),
            });
            Err(ViewFailure {
                target: TargetFailure::CrossApp(e),
                analyze: analyze.map(Box::new),
                inputs: Arc::clone(inputs),
            })
        }
    }
}

/// [`build_view_model`]'s single-app half: one `Workspace` program build, the
/// model projected from its parse (engine-switch S2a), its calls attached.
fn build_single_app(
    inputs: &Arc<WorkspaceInputs>,
    skip_roots_config: bool,
    extras: bool,
) -> Result<ViewModel, ViewFailure> {
    let program = {
        let _s = pt::span("preflight", "preflight.fresh_program");
        crate::program::resolved::ResolvedProgram::build(
            inputs,
            crate::program::resolved::ProgramScope::Workspace,
        )
    };
    // The id `WorkspaceInputs::read` computed (#160): no second disk walk.
    let id = inputs.model_instance_id.as_deref();
    // A missing model-instance id is reported first, as before.
    let target_failure = || match id {
        None => TargetFailure::NoModelInstanceId,
        Some(_) => TargetFailure::DidNotResolve,
    };
    let program = match program {
        Ok(program) => program,
        Err(why) => {
            return Err(ViewFailure {
                target: target_failure(),
                analyze: extras.then(|| {
                    let model = match id {
                        None => ModelFailure::NoModelInstanceId,
                        Some(_) => program_build_failure(inputs, why.clone()),
                    };
                    Box::new((Err(why), model))
                }),
                inputs: Arc::clone(inputs),
            });
        }
    };
    // The build's coverage, moved out once the program is done with (#91).
    let failed = |program: crate::program::resolved::ResolvedProgram, model| ViewFailure {
        target: target_failure(),
        analyze: extras.then(|| Box::new((Ok(program.into_parts().0.coverage), model))),
        inputs: Arc::clone(inputs),
    };
    let Some(model_instance_id) = id else {
        return Err(failed(program, ModelFailure::NoModelInstanceId));
    };
    let resolved = {
        let _s = pt::span("l3", "l3.assemble_resolve");
        assemble_and_resolve_workspace_from_program(
            inputs,
            model_instance_id,
            skip_roots_config,
            &program.body().ctx,
            &program.body().parenless_calls,
        )
    };
    let Some(rows) = resolved else {
        return Err(failed(program, ModelFailure::AssemblyFailed));
    };
    // Taken before the adapter consumes (and drops) the program context.
    let physical = extras.then(|| program.body().ctx.graph().workspace_rows.clone());
    let object_facts = extras.then(|| {
        crate::program::model::census::object_fact_census(
            &rows.workspace,
            program.body().ctx.graph(),
        )
    });
    // The same build's coverage (#91).
    let (resolved, meta) =
        crate::program::model::program_calls::attach_program_calls(rows, program);
    Ok(ViewModel {
        target: AnalysisTarget::SingleApp(Box::new(resolved)),
        fresh: extras.then_some(Ok(meta.coverage)),
        physical,
        object_facts,
    })
}

/// A failed program build with an id: an error, unless the workspace has no
/// app-scoped AL file (the empty-output case). (Engine-switch S9.1: this used to
/// assemble L3's model from disk only to tell the two apart.) The same rule as
/// the `NestedApps::Skip` walk it replaces: a failed walk left `al_files` empty,
/// so it does not assemble either.
fn program_build_failure(inputs: &WorkspaceInputs, why: String) -> ModelFailure {
    if inputs.app_scoped_files().next().is_some() {
        ModelFailure::ProgramBuildFailed(why)
    } else {
        ModelFailure::AssemblyFailed
    }
}

/// `--scope primary`'s dependency test (engine-switch S5.3): a finding's object is a
/// dependency object when its role is `Dependency` (`Model::object_role`, #220). It
/// used to be `|_| false`. With no primary app identity (no `app.json` id) nothing
/// is treated as a dependency; an object the index does not hold is not one either.
pub fn dependency_object_predicate<'a>(
    resolved: &'a Model,
    idx: &'a ProjectionIndex<'a>,
) -> impl Fn(&str) -> bool + 'a {
    move |obj_id: &str| {
        idx.fingerprints
            .object_row_by_id(obj_id)
            .is_some_and(|o| resolved.object_role(o) == Role::Dependency)
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
    inputs: &WorkspaceInputs,
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
    inputs: &Arc<WorkspaceInputs>,
    request: &RequestInputs,
    args: &AnalyzeArgs,
    default_version: &str,
) -> Result<String, String> {
    run_analyze_with_exit(inputs, request, args, default_version).map(|(out, ..)| out)
}

/// `run_analyze_with_exit`'s output: `(stdout, exit_code, stderr_warning, baseline_bytes)`.
pub type AnalyzeOutput = (String, u8, Option<String>, Option<Vec<u8>>);

/// Run the gate `analyze` pipeline, returning `(stdout, exit_code, stderr_warning,
/// baseline_bytes)`.
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
/// The fourth is the baseline `--update-baseline` writes (`Some` only with
/// `--baseline` and `--update-baseline`). Compute writes nothing (#277 D2.4): the
/// caller writes these bytes to `args.baseline` BEFORE the output, and a failed
/// write is its write failure, never an error of this function.
///
/// `inputs` is the command's one read of the workspace (#160 §4.2.6): every reader
/// below reads it, never the disk. `inputs.workspace` is `args.workspace`.
/// `request` holds the `--baseline` file, read with it (`AnalyzeArgs::read_request`).
///
/// Errors: detector/preset resolution failures (e.g. unknown preset, `--preset` +
/// `--detector` together). A workspace that fails to assemble (fail-closed / unreadable)
/// yields EMPTY findings (engine-never-throws) — empty SARIF / "no findings" PR-summary,
/// a could-not-verify preflight WARNING (never a fabricated clean — spec §3), and exit
/// CLEAN unless `--require-dependencies` is set, in which case exit PREFLIGHT_FAILED (4).
pub fn run_analyze_with_exit(
    inputs: &Arc<WorkspaceInputs>,
    request: &RequestInputs,
    args: &AnalyzeArgs,
    default_version: &str,
) -> Result<AnalyzeOutput, String> {
    // The whole run, build included; the bin opens the same span around its
    // standalone run (#277 R5-2).
    let _analyze_span = pt::span("analyze", "analyze.total");
    let job = AnalyzeJob::new(inputs, request, args, default_version)?;
    crate::engine::batch::run_doc(&job)
        .one()
        .unwrap_or_else(|f| unreachable!("analyze fails no side: {f:?}"))
}

/// `alsem analyze` as a batch job (#277 D1). Its document is
/// `Result<AnalyzeOutput, String>`; its writes are the baseline (when
/// `--update-baseline`), the degraded warning, then the report (D2.3, D2.4).
pub struct AnalyzeJob<'a> {
    inputs: &'a dyn WorkspaceRead,
    request: &'a RequestInputs,
    args: &'a AnalyzeArgs,
    /// The SARIF `driver.version`: the override, else the engine's.
    version: String,
    detectors: Vec<crate::engine::l5::registry::Detector>,
}

impl<'a> AnalyzeJob<'a> {
    /// `Err`: a detector/preset resolution failure, found before any build.
    pub fn new(
        inputs: &'a dyn WorkspaceRead,
        request: &'a RequestInputs,
        args: &'a AnalyzeArgs,
        default_version: &str,
    ) -> Result<AnalyzeJob<'a>, String> {
        let detectors =
            resolve_analyze_detectors(args.preset.as_deref(), args.detector.as_deref())?;
        let version = args
            .sarif_version_override
            .clone()
            .unwrap_or_else(|| default_version.to_string());
        Ok(AnalyzeJob {
            inputs,
            request,
            args,
            version,
            detectors,
        })
    }
}

impl Job for AnalyzeJob<'_> {
    fn name(&self) -> &'static str {
        "analyze"
    }
    /// The whole run, build included, as before the driver (#277 R5-2).
    fn standalone_span(&self) -> Option<&'static str> {
        Some("analyze.total")
    }
    fn demands(&self) -> Vec<ViewDemand> {
        // Assembled with the al-sem GATE modelInstanceId (content-derived, UNPINNED)
        // so the internal RoutineIds embedded in each finding's rootCauseKey — and
        // therefore the SARIF fingerprint hashed over them — byte-match the al-sem
        // `analyze` CLI goldens (`build_analysis_model`).
        ViewDemand::one(self.inputs, self.args.single_app, false, ViewNeeds::ANALYZE)
    }

    fn resolve(&self, _: SideIx, _: &ResolveCx<'_>) -> Result<PlanPart, JobFailure> {
        Ok(PlanPart {
            plan: analyze_plan(self.detectors.clone(), self.args.scope == Scope::Primary),
            reads: vec![],
            carry: None,
        })
    }

    fn compute(&self, _: SideIx, cx: &mut ViewCx<'_>) -> Result<SideDoc, JobFailure> {
        Ok(SideDoc::new(analyze_view(
            cx,
            self.request,
            self.args,
            &self.version,
        )))
    }

    fn view_failed(&self, _: SideIx, view: &ViewFailure) -> Result<SideDoc, JobFailure> {
        let (fresh, failure) = view
            .analyze
            .as_deref()
            .expect("analyze's view is built with its extras");
        Ok(SideDoc::new(match failure {
            // Fail-closed layout / unreadable workspace → empty output; preflight says
            // could-not-verify (never a fabricated clean — spec §3), gated on `fresh`.
            // No finding set, so no baseline is written (as before #277).
            ModelFailure::NoModelInstanceId | ModelFailure::AssemblyFailed => {
                empty_output_result(&view.inputs, self.args, &self.version, fresh)
                    .map(|(out, exit, warn)| -> AnalyzeOutput { (out, exit, warn, None) })
            }
            ModelFailure::ProgramBuildFailed(why) => Err(format!(
                "analysis failure — program engine build failed: {why}"
            )),
        }))
    }

    fn serialize(&self, doc: Doc) -> JobOutput {
        let doc = doc
            .one::<Result<AnalyzeOutput, String>>()
            .unwrap_or_else(|f| unreachable!("analyze fails no side: {f:?}"));
        match doc {
            Ok((mut out, exit_code, stderr_warning, baseline_bytes)) => {
                let mut actions = Vec::new();
                // `--update-baseline`: the baseline is the first write, before any
                // stderr line or stdout; a failure is a write error (exit 2) and
                // stdout stays unwritten (#277 D2.4, S17).
                if let (Some(path), Some(bytes)) = (&self.args.baseline, baseline_bytes) {
                    actions.push(WriteAction::File(path.into(), bytes));
                }
                // F2: the preflight degraded warning (the "no silent clean"
                // contract), before the report. Matches al-sem index.ts:263-264.
                if let Some(msg) = stderr_warning {
                    actions.push(WriteAction::line(&format!("al-sem: warning: {msg}")));
                }
                // al-sem appends a trailing newline to the formatted output.
                out.push('\n');
                actions.push(WriteAction::Stdout(out.into_bytes()));
                JobOutput {
                    actions,
                    exit: exit_code,
                }
            }
            // "analysis failure — …" is tagged for the errors al-sem maps to
            // EXIT.ANALYSIS_FAILURE (2), e.g. a malformed baseline file; every
            // other error is a config/usage error (3).
            Err(msg) => {
                let exit = if msg.starts_with("analysis failure — ") {
                    exit::ANALYSIS_FAILURE
                } else {
                    exit::CONFIG_ERROR
                };
                JobOutput::lines(&[format!("al-sem: {msg}")], exit)
            }
        }
    }

    fn write_failure_exit(&self) -> u8 {
        exit::ANALYSIS_FAILURE
    }
}

/// Analyze off its prepared view: projection, filters, baseline, format,
/// preflight and exit.
fn analyze_view(
    cx: &mut ViewCx<'_>,
    request: &RequestInputs,
    args: &AnalyzeArgs,
    version: &str,
) -> Result<AnalyzeOutput, String> {
    let inputs = Arc::clone(cx.inputs());
    let inputs = &inputs;
    // L4 + L5: the selected detectors ran on the view (#160 §4.4.4). Findings
    // come pre-sorted by (detector, primaryLocationKey, rootCauseKey). Cross-app,
    // the run drops dependency-anchored findings itself for `--scope primary`
    // (routine, object and `dep:` location); `--scope all` keeps them for the
    // scope filter below.
    let key = cx.keys()[0].clone();
    let run = cx.take_run(&key);
    // Today's context died inside the runner, before any projection: the view
    // holds none of its cells longer.
    cx.release_all_but(&[]);
    let fresh = cx.fresh();
    let resolved = cx.view().model();
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
    // The bytes are returned, never written here (#277 D2.4): the caller writes them.
    let mut baseline_bytes = None;
    if args.baseline.is_some() {
        let summaries: Vec<_> = paired.iter().map(|(s, _)| s.clone()).collect();
        let baseline = if args.update_baseline {
            // The set applied is the set to be written: every current finding. The
            // old file (`request.baseline`) is never read, so a malformed one never
            // errors, as when the file was overwritten before being read back (#160).
            baseline_bytes = Some(serialize_baseline(&summaries).into_bytes());
            summaries.iter().map(|s| s.fingerprint.clone()).collect()
        } else {
            // A malformed baseline (exists but not valid JSON / non-array
            // fingerprints) is an analysis-failure, not a config error: al-sem's
            // loadBaseline throws and the CLI catch emits "al-sem: analysis failure —
            // <msg>" + exits 2. We surface this as an Err tagged with the "analysis
            // failure — " prefix so the bin can distinguish it from config errors
            // (exit 3) and use exit 2 instead. The bytes are the ones the command's
            // `RequestInputs` read (#160, #277).
            let file = request.baseline.as_ref().expect(
                "--baseline set but the RequestInputs was read without it (AnalyzeArgs::read_request)",
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
    // Parsed from the workspace source texts the `WorkspaceInputs` read; only ws: units are
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
        analysis_coverage(resolved, inputs, fresh)
    };
    // --- format ---
    let output = {
        let _s = pt::span("gate", "gate.format");
        match args.format {
            OutputFormat::Sarif => {
                let summaries: Vec<_> = paired.iter().map(|(s, _)| s.clone()).collect();
                let raws: Vec<&crate::engine::l5::finding::Finding> =
                    paired.iter().map(|(_, r)| *r).collect();
                format_sarif(&summaries, &raws, version)
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
                    encrypted_packages: fresh
                        .as_ref()
                        .map_or(&[], |f| f.encrypted_packages.as_slice()),
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
    let pf = evaluate_preflight(fresh, args.require_dependencies);

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
    // These drops happened here anyway: the `analyze.total` span (opened by
    // the caller around the whole run) outlives them, and every structure
    // below was already being freed inside it. Naming them makes
    // that cost visible instead of leaving it in the span's unattributed self
    // time — a 4-run 8020 profile put `analyze.total`'s own self time at
    // 15.5 % of the whole run (≈ 11.8 s at the 76.2 s median), the SECOND
    // largest region in the profile, with nothing saying what was in it.
    //
    // Order is forced by the borrows, not chosen: `paired` holds `&Finding`s
    // into `run.findings` and `idx` borrows `resolved.workspace`, so both must
    // go before their owners. Nothing here has a `Drop` impl (the only two in
    // the engine are `perf_trace`'s own guards), so this is pure deallocation
    // and reordering it is not observable. The view's own cells (the model,
    // `det`, `l4`, the runs map) are freed by the batch driver when it drops
    // the view, in its `view.teardown` span (#277 E12-C3).
    {
        let _s = pt::span("gate", "gate.teardown");
        drop(paired);
        drop(idx);
        drop(run_diagnostics);
        drop(coverage);
        drop(run);
    }

    Ok((output, exit_code, stderr_warning, baseline_bytes))
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
    inputs: &WorkspaceInputs,
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
                encrypted_packages: fresh
                    .as_ref()
                    .map_or(&[], |f| f.encrypted_packages.as_slice()),
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
                        && range.contains(loc.start_line, loc.start_column)
                })
                .min_by(|(a, _, _), (b, _, _)| a.extent_cmp(b));
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

/// The analyzer diagnostics for a resolved single-app workspace:
/// [`analyzer_diagnostics_of_run`] over a run of the default detector set.
///
/// This is the source for the cli-b capability-snapshot envelope's `diagnostics`
/// channel (`projectDiagnostics`). It builds them as `run_analyze` does, but the
/// output is the same only for the same `resolved`.
/// Since #160 step 2 no command calls it: every command reads its `View`'s
/// run through [`analyzer_diagnostics_of_view`]. Its one caller is the cli-b
/// snapshot differential test, which passes a program-backed single-app model
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

/// Diagnostics as `<severity>: <message>` lines, the stderr form `alsem events`
/// and `alsem policy` print (and match `error:` against under `--strict`).
pub(crate) fn diagnostic_lines(diags: &[crate::engine::l5::registry::Diagnostic]) -> Vec<String> {
    diags
        .iter()
        .map(|d| format!("{}: {}", d.severity, d.message))
        .collect()
}

/// [`analyzer_diagnostics_of_run`] over the run `key` of a prepared `View`
/// (#160 §4.4.4): what `fingerprint`, `diff --strict`, digest, prove, `events
/// fanout` / `events chains` and `policy check` report, from the view's own run
/// instead of a context of their own.
pub(crate) fn analyzer_diagnostics_of_view(
    view: &View,
    key: &RunKey,
) -> Vec<crate::engine::l5::registry::Diagnostic> {
    analyzer_diagnostics_of_run(view.model(), &view.run(key))
}

/// [`compute_analyzer_diagnostics`] for a cross-app base (its one caller is
/// `tools/census-probe`; every command reads its View's run): the default
/// set run by the cross-app runner, the
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
/// (over the run of its selected detectors), [`analyzer_diagnostics_of_view`]
/// (fingerprint, `diff --strict`, digest, prove, `events fanout` / `events
/// chains` and `policy check`, over a `View`'s default-set run), [`compute_analyzer_diagnostics`] and
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
    //     by `attach_program_calls` from the command's `WorkspaceInputs` and the build's
    //     objectless-file list (#160 §4.2.4).
    all.extend(resolved.workspace_diagnostics.iter().cloned());
    // (2) dependency diagnostics — the program build's dependency ledger
    //     (engine-switch S5.2b): missing, older-than-declared, unreadable and
    //     unidentified dependency packages, kept on the model by
    //     `attach_program_calls` (#91).
    all.extend(resolved.dependency_diagnostics.iter().cloned());
    // (3) summarizeDiagnostics — L4 `compute_summaries*` surfaces the JACOBI
    //     fixed-point cap-hit here: on a `View` run, from the `l4` cell's
    //     `summarize_diagnostics` (`View::prepare`); on the `run_detectors`
    //     shims, from their `DetectorContext` build. Empty whenever every SCC
    //     converges.
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

    fn model_with_primary_app() -> Model {
        let mut m = crate::engine::l5::test_support::empty_resolved();
        m.primary_app = Some(crate::engine::gate::app_attribution::App {
            app_guid: "app".to_string(),
            publisher: "probe".to_string(),
            name: "Ws".to_string(),
            version: "1.0.0.0".to_string(),
        });
        m
    }

    /// #160 step 9 D1 (plan 1.2): `--no-roots-config` reaches both arms of
    /// `AnalysisTarget::build_with`. Precondition: with `false`, ws-d51's
    /// `roots.config.json` gives a root classification a `config_entry_id`, in
    /// both universes. Break: a literal `false` inside
    /// `build_cross_app_base_with` -> the cross-app half fails.
    #[test]
    fn build_with_no_roots_config_reaches_both_arms() {
        let ws = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/r0-corpus/ws-d51-jobqueue");
        let inputs = crate::program::inputs::WorkspaceInputs::read(&ws, &Default::default());
        let configured = |single_app: bool, no_roots_config: bool| -> usize {
            let target =
                super::AnalysisTarget::build_with(&inputs, single_app, no_roots_config, "test")
                    .unwrap_or_else(|e| panic!("build_with({single_app}): {e}"));
            target
                .resolved()
                .root_classifications
                .iter()
                .filter(|rc| rc.config_entry_id.is_some())
                .count()
        };
        for (universe, single_app) in [("single-app", true), ("cross-app", false)] {
            assert!(
                configured(single_app, false) > 0,
                "{universe}: precondition: the overlay gives a classification a config_entry_id"
            );
            assert_eq!(
                configured(single_app, true),
                0,
                "{universe}: --no-roots-config leaves no config_entry_id"
            );
        }
    }

    /// #220 review u-1: `is_primary_routine` decides by THE role rule in a
    /// SINGLE-APP target too. Hand-stated: a row whose app guid differs from
    /// the primary's reads not primary; the primary's (other case) reads
    /// primary. Break: the single-app arm's constant `true` -> fails.
    #[test]
    fn single_app_is_primary_routine_uses_role() {
        let target = super::AnalysisTarget::SingleApp(Box::new(model_with_primary_app()));
        let mut r = crate::engine::l5::test_support::routine("r0", "procedure");
        r.app_guid = "APP".to_string();
        assert!(target.is_primary_routine(&r));
        r.app_guid = "dep".to_string();
        assert!(!target.is_primary_routine(&r));
    }

    /// #220 review u-3: an object id the index does not hold is not a
    /// dependency object. Break: unknown -> `true` -> fails.
    #[test]
    fn dependency_object_predicate_unknown_object_is_false() {
        let m = model_with_primary_app();
        let idx = crate::engine::gate::projection::ProjectionIndex::build(&[], &[]);
        assert!(!super::dependency_object_predicate(&m, &idx)(
            "dep/Codeunit/1"
        ));
    }

    /// #91: `alsem analyze` builds its diagnostics through the one shared
    /// aggregation, not an inline copy of it. Its output cannot tell the two
    /// apart, so the call itself is pinned. Mutation: restore the inline
    /// `compute_workspace_diagnostics(` / `dependency_diagnostics(` block in
    /// `analyze_view` (the analyze job's compute, #277), or read
    /// `.workspace_diagnostics` there (#160: source (1) is now a model field)
    /// -> this fails.
    #[test]
    fn run_analyze_with_exit_uses_the_shared_aggregation() {
        let src = include_str!("run.rs");
        let start = src.find("\nfn analyze_view(").expect("analyze_view");
        let body = &src[start..];
        // Cut at the next top-level item, not the first column-0 `}`, which a raw
        // string inside the fn could hold.
        let end = ["\npub fn ", "\nfn ", "\n#[", "\n///"]
            .iter()
            .filter_map(|m| body[1..].find(m))
            .min()
            .expect("an item after analyze_view");
        // The trace span's NAME holds `.workspace_diagnostics` too; it is a
        // label, not a read of the field, so it is cut out before the scan.
        let span = "\"gate.workspace_diagnostics\"";
        assert_eq!(body[..end + 1].matches(span).count(), 1, "the span label");
        let body = &body[..end + 1].replace(span, "");
        // An anchor from the function's last statement proves the cut did not
        // stop early.
        assert!(
            body.contains("Ok((output, exit_code, stderr_warning, baseline_bytes))"),
            "the cut must reach analyze_view's later code"
        );
        assert!(
            body.contains("analyzer_diagnostics_of_run("),
            "analyze_view must call analyzer_diagnostics_of_run"
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
                "analyze_view calls {banned} itself; aggregate through \
                 analyzer_diagnostics_of_run so every command reports the same set"
            );
        }
    }

    /// #160 §4.4.4: fingerprint and `diff --strict` read their diagnostics off
    /// a `View`'s run and take the run before snapshot composition. Their
    /// counter rows match the parent's, so only a source scan sees a revert
    /// to their own context. Comment lines are cut first.
    ///
    /// Stated limits: only whole `//` lines are cut, so a banned name in a
    /// trailing or block comment trips the scan; and it checks presence, not
    /// order, so a `take_run` moved after the snapshot composition passes.
    #[test]
    fn fingerprint_and_diff_read_diagnostics_off_the_view() {
        for (name, src) in [
            (
                "fingerprint_cli.rs",
                include_str!("../l5/fingerprint_cli.rs"),
            ),
            ("diff/cli.rs", include_str!("diff/cli.rs")),
        ] {
            let code: String = src
                .lines()
                .filter(|l| !l.trim_start().starts_with("//"))
                .collect::<Vec<_>>()
                .join("\n");
            for required in ["analyzer_diagnostics_of_view(", "take_run("] {
                assert!(code.contains(required), "{name} must call {required}");
            }
            for banned in [
                "compute_analyzer_diagnostics(",
                "analyzer_diagnostic_lines(",
            ] {
                assert!(
                    !code.contains(banned),
                    "{name} calls {banned}; read the diagnostics off the View's run"
                );
            }
        }
    }
}

// #277 T16: the three single-app builders agree.
#[cfg(test)]
#[path = "run_builders_tests.rs"]
mod builders_tests;

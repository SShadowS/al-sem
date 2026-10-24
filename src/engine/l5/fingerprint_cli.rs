//! cli-b/b3 — FINGERPRINT CLI pipeline.
//!
//! `run_fingerprint_pipeline` assembles the workspace, composes the consumed-core
//! snapshot, runs the query, and format-dispatches per the contract from
//! `src/cli/fingerprint.ts`. Two forms branch off first (#160 step 9, S′),
//! each on a `View` of the target (cross-app by default, `--single-app` for
//! the workspace alone) instead of the single-app cut:
//!
//! - a query with `--routine` (rows 5 and 6 below): `run_routine_query`
//!   resolves the selectors from the model, reads the query off the view's
//!   tree inside `prepare`, and projects the owned result;
//! - `--inventory-only`: `run_inventory_only` assembles the five inventory keys
//!   off a tree with no raw facts.
//!
//! ## Format dispatch (fingerprint.ts:207-298)
//!
//! 1. `--shard`                              → B0 `serialize_sharded` (PRECEDES query)
//! 2. `--format cbor`                        → B0 `serialize_cbor`
//! 3. `--format cbor.gz`                     → B0 `serialize_cbor_gz`
//! 4. `--format json` + NO query flag        → B0 `serialize_envelope`
//! 5. `--format json` + query flag           → `fingerprint_query` → `project_fingerprint_query_full`
//! 6. `--format human`                       → `fingerprint_query` → `format_fingerprint_human`
//!
//! `isQueryRequested` = any of `--witness`, `--roots`, `--routine`, `--include-inherited`
//! specified (even with default values).
//!
//! Error path:
//!   - Selector unresolved / ambiguous → exit 2.
//!   - json: valid doc in payload.diagnostics, no stderr.
//!   - human: message to stderr, exit 2, no stdout.
//!   - `.app` input: rejected with exit 1 (no workspace).
//!
//! Illegal combos:
//!   - `--shard` + query flag     → exit 1 (error).
//!   - `--format cbor` + query    → exit 1 (error).
//!   - `--format cbor.gz` + query → exit 1 (error).

use crate::engine::batch::job::{
    Doc, Job, JobFailure, PlanPart, ResolveCx, SideDoc, SideIx, ViewCx, ViewDemand, ViewFailure,
    ViewNeeds,
};
use crate::engine::batch::output::{JobOutput, WriteAction};
use crate::engine::gate::run::analyzer_diagnostics_of_view;
use crate::engine::l4::effect_query_cli::OwnerIndex;
use crate::engine::l5::detectors::default_detectors;
use crate::engine::l5::digest_cli::primary_guid_of;
use crate::engine::l5::fingerprint_query::{
    FingerprintFilters, FingerprintQueryDiagnostic, WitnessLimit, fingerprint_query,
    format_fingerprint_human_verbosity, primary_stable_ids, project_fingerprint_query_full,
    resolve_routine_selectors, selector_failure_result,
};
use crate::engine::l5::selector_index::build_selector_indexes;
use crate::engine::l5::snapshot::compose_snapshot;
use crate::engine::l5::snapshot_full::{
    EnvelopeDiagnostic, FullSnapshotOptions, build_inventory_envelope, compose_full_snapshot,
    serialize_cbor, serialize_cbor_gz, serialize_envelope, serialize_sharded,
};
use crate::engine::session::{
    Cell, InventoryRead, QueryRead, RunKey, TreeReadOut, View, fingerprint_inventory_plan,
    fingerprint_plan, fingerprint_query_plan,
};

/// The format the fingerprint command outputs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FingerprintFormat {
    Json,
    Human,
    Cbor,
    CborGz,
}

impl FingerprintFormat {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "json" => Some(Self::Json),
            "human" => Some(Self::Human),
            "cbor" => Some(Self::Cbor),
            "cbor.gz" => Some(Self::CborGz),
            _ => None,
        }
    }
}

// ===========================================================================
// CLI flag validation — byte-faithful port of `src/cli/fingerprint.ts`.
//
// These are TS-faithful, corpus-invisible behaviors (validateRoots,
// normalizeWitness, rejectIllegalCombos, defaultFormat, isQueryRequested).
// `alsem.rs` delegates to them so the exact exit codes + stderr strings match.
// ===========================================================================

/// The valid `--roots` values, in declaration order (used verbatim in the
/// `validateRoots` error message).
///
/// SINGLE DEFINITION, deliberately. This module used to restate the twelve values
/// verbatim, so adding a kind to the classifier made the engine emit something the
/// shipped `alsem fingerprint --roots <kind>` refused as "unknown root kind". The
/// lists never actually diverged, but nothing prevented it -- and a test can only
/// observe that they currently agree, never that they must. Importing the one
/// definition makes divergence unrepresentable instead of detectable (#10).
use crate::engine::root_classification::ROOT_KIND_VALUES;

/// `validateRoots` (fingerprint.ts:67). Each value must be in `ROOT_KIND_VALUES`,
/// else exit-1 with `unknown root kind '<v>'; valid: <a, b, ...>`.
pub fn validate_roots(values: &[String]) -> Result<Vec<String>, String> {
    let mut out = Vec::with_capacity(values.len());
    for v in values {
        if !ROOT_KIND_VALUES.contains(&v.as_str()) {
            return Err(format!(
                "unknown root kind '{v}'; valid: {}",
                ROOT_KIND_VALUES.join(", ")
            ));
        }
        out.push(v.clone());
    }
    Ok(out)
}

/// `normalizeWitness` (fingerprint.ts:82). `None` → 3 (`Capped(3)`); `false` →
/// `Disabled`; `all` → `All`; digits in `0..=256` → `Capped(n)`; n>256 → exit-1
/// `--witness must be in 0..256 or 'all' (got N)`; anything else → exit-1
/// `invalid --witness value`.
pub fn normalize_witness(w: Option<&str>) -> Result<WitnessLimit, String> {
    match w {
        None => Ok(WitnessLimit::Capped(3)),
        Some("false") => Ok(WitnessLimit::Disabled),
        Some("all") => Ok(WitnessLimit::All),
        Some(s) => match s.parse::<usize>() {
            Ok(n) => {
                if n > 256 {
                    Err(format!("--witness must be in 0..256 or 'all' (got {n})"))
                } else {
                    Ok(WitnessLimit::Capped(n))
                }
            }
            Err(_) => Err("invalid --witness value".to_string()),
        },
    }
}

/// `flagName` (fingerprint.ts:94) — maps an internal flag name to its CLI spelling
/// for the combo-rejection messages.
fn flag_name(f: &str) -> &str {
    match f {
        "routineSelectors" => "routine",
        "includeInherited" => "include-inherited",
        other => other,
    }
}

/// Which query flags were explicitly specified on the CLI. Mirrors the
/// `_specifiedFlags` set built in `index.ts:434-439`.
#[derive(Debug, Clone, Copy, Default)]
pub struct SpecifiedFlags {
    /// `--roots` passed (TS: `cmdOpts.roots !== undefined`).
    pub roots: bool,
    /// `--routine` passed at least once (TS: array len > 0).
    pub routine_selectors: bool,
    /// `--no-include-inherited` passed (TS: `includeInherited === false`).
    pub include_inherited: bool,
    /// `--witness` passed with a value other than the default `"3"`
    /// (TS: `witness !== undefined && witness !== "3"`).
    pub witness: bool,
}

impl SpecifiedFlags {
    /// `isQueryRequested` (fingerprint.ts:100) — any query flag specified.
    pub fn is_query_requested(&self) -> bool {
        self.roots || self.routine_selectors || self.witness || self.include_inherited
    }

    /// The query-flag names in TS iteration order, for `rejectIllegalCombos`.
    fn specified_query_flags(&self) -> Vec<&'static str> {
        let mut v = Vec::new();
        if self.roots {
            v.push("roots");
        }
        if self.routine_selectors {
            v.push("routineSelectors");
        }
        if self.witness {
            v.push("witness");
        }
        if self.include_inherited {
            v.push("includeInherited");
        }
        v
    }
}

/// `defaultFormat` (fingerprint.ts:140). When `--format` is given it must be one
/// of human|json|cbor|cbor.gz (else exit-1 `unknown --format '<f>'; valid: ...`).
/// When omitted: `--shard` → json, else human.
pub fn default_format(format: Option<&str>, shard: bool) -> Result<FingerprintFormat, String> {
    const VALID: &[&str] = &["human", "json", "cbor", "cbor.gz"];
    match format {
        Some(f) => FingerprintFormat::parse(f)
            .ok_or_else(|| format!("unknown --format '{f}'; valid: {}", VALID.join(", "))),
        None => {
            if shard {
                Ok(FingerprintFormat::Json)
            } else {
                Ok(FingerprintFormat::Human)
            }
        }
    }
}

/// `rejectIllegalCombos` (fingerprint.ts:110). `--shard` cannot combine with any
/// query flag, and requires a serializer format; cbor/cbor.gz cannot combine with
/// query flags. Messages are byte-identical to TS.
pub fn reject_illegal_combos(
    specified: SpecifiedFlags,
    format: &FingerprintFormat,
    shard: bool,
) -> Result<(), String> {
    if shard {
        if let Some(f) = specified.specified_query_flags().into_iter().next() {
            return Err(format!(
                "--shard cannot be combined with --{}",
                flag_name(f)
            ));
        }
        if *format == FingerprintFormat::Human {
            return Err("--shard requires --format=json|cbor|cbor.gz".to_string());
        }
    }
    if (*format == FingerprintFormat::Cbor || *format == FingerprintFormat::CborGz)
        && let Some(f) = specified.specified_query_flags().into_iter().next()
    {
        return Err(format!(
            "--{} is only valid with --format=human or --format=json",
            flag_name(f)
        ));
    }
    Ok(())
}

/// Shard mode (the `--shard` value). Mirrors TS `"primary-only" | "all"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShardMode {
    PrimaryOnly,
    All,
}

/// Options for `run_fingerprint_pipeline`.
pub struct FingerprintOptions<'a> {
    /// The command's one read of the workspace (#160); the path is
    /// `inputs.workspace`.
    pub inputs: &'a std::sync::Arc<crate::program::inputs::WorkspaceInputs>,
    /// al-sem version string (e.g. `"cli-b-v1"`).
    pub driver_version: &'a str,
    /// Output format.
    pub format: FingerprintFormat,
    /// Output file (stdout when None).
    pub out: Option<&'a str>,
    /// Shard mode (None = single-file output).
    pub shard: Option<ShardMode>,
    /// Witness limit: None = use default (3 = golden default); Some(WitnessLimit) = explicit.
    pub witness_limit: Option<WitnessLimit>,
    /// Root-kind filter (None = all roots).
    pub roots: Option<std::collections::BTreeSet<String>>,
    /// Routine selectors (empty = all routines).
    pub routine_selectors: Vec<String>,
    /// When true, include inherited (transitive) facts.
    pub include_inherited: bool,
    /// When true, any of witness/roots/routine/include-inherited was explicitly specified.
    pub is_query_requested: bool,
    /// Pin timestamps for byte-stable output.
    pub deterministic: bool,
    /// `--strict`: exit 1 if any analyzer diagnostic has severity "error" (before any output).
    pub strict: bool,
    /// Verbosity for human output: "compact" | "full".
    pub verbosity: &'a str,
    /// `--inventory-only`: emit the lean routine-inventory projection instead of
    /// the full capability-snapshot. Implies json-only; rejected with any binary
    /// format (cbor / cbor.gz) or shard mode.
    pub inventory_only: bool,
    /// `--no-roots-config`: skip loading/overlaying `<workspace>/roots.config.json`
    /// even if present (AST-only root classification). When a config file DID
    /// exist and was skipped, `summary.rootsConfigIgnored` / `inputsMetadata.
    /// rootsConfigIgnored` in the output is set to `true` — never a bare `false`
    /// that would contradict the flag.
    pub no_roots_config: bool,
    /// `--single-app` (#160 step 9): the `--routine` query and `--inventory-only`
    /// analyse the workspace alone; the default loads the dependencies it
    /// requires. The other forms analyse the workspace alone in this release.
    pub single_app: bool,
}

/// Result returned from `run_fingerprint_pipeline`.
pub enum FingerprintOutput {
    /// Text output (JSON or human).
    Text(String),
    /// Binary output (CBOR or CBOR.gz).
    Binary(Vec<u8>),
    /// Multiple shard files (filename, text content).
    Shards(Vec<(String, Vec<u8>)>),
}

pub struct FingerprintRunResult {
    pub output: FingerprintOutput,
    /// Exit code: 0 = ok, 1 = strict/error, 2 = selector error.
    pub exit_code: u8,
    /// Selector error message for human format (emit to stderr at exit, no stdout).
    pub selector_error_message: Option<String>,
    /// Analyzer diagnostics to print to stderr AFTER writing output, in
    /// `<severity>: <message>` form. Empty for the JSON-query mode (those errors
    /// are embedded in `payload.diagnostics` — fingerprint.ts:297 vs 332).
    pub stderr_diagnostics: Vec<String>,
}

/// Format the human selector-error stderr block (fingerprint.ts:301-316).
/// Byte-identical to TS: unresolved → one line; ambiguous → header + one
/// `  - <display>  (<stableId>)` line per candidate.
pub fn format_selector_errors_human(diags: &[FingerprintQueryDiagnostic]) -> String {
    let mut out = String::new();
    for d in diags {
        match d {
            FingerprintQueryDiagnostic::SelectorUnresolved { selector } => {
                // triedForms join — the SELECTOR_FORMS list (fingerprint-query.ts:132).
                out.push_str(&format!(
                    "error: --routine '{selector}' did not match any routine (tried: stable-routine-id, full-display, two-segment, one-segment, object-qualified)\n"
                ));
            }
            FingerprintQueryDiagnostic::SelectorAmbiguous {
                selector,
                matched_form,
                candidates,
            } => {
                out.push_str(&format!(
                    "error: --routine '{selector}' is ambiguous (matched via {matched_form}); candidates:\n"
                ));
                for (stable_id, display) in candidates {
                    out.push_str(&format!("  - {display}  ({stable_id})\n"));
                }
            }
        }
    }
    out
}

/// `--inventory-only`: json-only; reject binary formats, shard mode, and query
/// selectors. The selector rejection is load-bearing: a query selector makes
/// `is_query_requested` true, which would route past the B0 inventory branch into
/// the QUERY path (silently ignoring --inventory-only). Reject up front instead.
/// The CLI calls this before it reads the workspace (#160 final panel r1), so a
/// usage error reads nothing; the pipeline checks again for library callers.
pub fn reject_inventory_only_combos(
    inventory_only: bool,
    shard: bool,
    format: &FingerprintFormat,
    is_query_requested: bool,
) -> Result<(), String> {
    if !inventory_only {
        return Ok(());
    }
    if shard {
        return Err("--inventory-only cannot be combined with --shard".to_string());
    }
    if *format != FingerprintFormat::Json {
        return Err(format!(
            "--inventory-only requires --format=json (got {format:?})"
        ));
    }
    if is_query_requested {
        return Err(
            "--inventory-only cannot be combined with query selectors (--routine/--roots/--witness/--no-include-inherited)"
                .to_string(),
        );
    }
    Ok(())
}

/// Run the full fingerprint pipeline.
///
/// Combo validation (`--shard`/cbor + query flags, `--format` value) is performed
/// upstream by the CLI (`default_format` + `reject_illegal_combos`) so the exact
/// TS exit codes / stderr land; this function assumes a legal invocation.
pub fn run_fingerprint_pipeline(opts: &FingerprintOptions) -> Result<FingerprintRunResult, String> {
    reject_inventory_only_combos(
        opts.inventory_only,
        opts.shard.is_some(),
        &opts.format,
        opts.is_query_requested,
    )?;

    match FingerprintJob::new(opts) {
        // A stderr-only error: the message, no output.
        Err(f) => Ok(FingerprintRunResult {
            output: FingerprintOutput::Text(String::new()),
            exit_code: f.exit,
            selector_error_message: Some(f.message()),
            stderr_diagnostics: Vec::new(),
        }),
        Ok(job) => crate::engine::batch::run_doc(&job)
            .one::<FingerprintDoc>()
            .unwrap_or_else(|f| unreachable!("fingerprint fails no side: {f:?}"))
            .into_result(opts),
    }
}

/// `alsem fingerprint` as a batch job (#277 D1). Case B (#160 step 9, S′):
/// `--routine` and `--inventory-only` read a tree on the view of their
/// universe (cross-app by default, `--single-app` the workspace alone); every
/// other form composes its own cut snapshot on the single-app view (D6.4).
/// Both views are the ones `--no-roots-config` names. Its document is a
/// [`FingerprintDoc`].
pub struct FingerprintJob<'a> {
    opts: &'a FingerprintOptions<'a>,
}

/// Which path a fingerprint invocation takes (#160 step 9 §4.3).
#[derive(Clone, Copy, PartialEq, Eq)]
enum FingerprintForm {
    /// B0 and the query forms with no `--routine`: the cut, single-app.
    Cut,
    /// `--routine`: the query off the view's tree.
    RoutineQuery,
    /// `--inventory-only`: the five keys off the view's tree.
    Inventory,
}

impl FingerprintForm {
    fn of(opts: &FingerprintOptions) -> FingerprintForm {
        if !go_b0(opts) && !opts.routine_selectors.is_empty() {
            FingerprintForm::RoutineQuery
        } else if opts.inventory_only {
            FingerprintForm::Inventory
        } else {
            FingerprintForm::Cut
        }
    }
}

/// The B0 path: shard / cbor / cbor.gz / json-no-query.
fn go_b0(opts: &FingerprintOptions) -> bool {
    opts.shard.is_some()
        || opts.format == FingerprintFormat::Cbor
        || opts.format == FingerprintFormat::CborGz
        || (opts.format == FingerprintFormat::Json && !opts.is_query_requested)
}

/// A fingerprint side: its finished result, or (`--routine`) the query's
/// owned result, projected only after the view is gone (#160 step 9 O3).
pub(crate) enum FingerprintDoc {
    Done(Result<FingerprintRunResult, String>),
    Query {
        result: crate::engine::l5::fingerprint_query::FingerprintQueryResult,
        analyzer_diags: Vec<(String, String, String)>,
        /// #239's hint lines per unresolved selector.
        declared: std::collections::HashMap<String, Vec<String>>,
    },
}

impl FingerprintDoc {
    fn into_result(self, opts: &FingerprintOptions) -> Result<FingerprintRunResult, String> {
        match self {
            FingerprintDoc::Done(r) => r,
            FingerprintDoc::Query {
                result,
                analyzer_diags,
                declared,
            } => Ok(emit_query(
                opts,
                &result,
                &query_filters(opts),
                &analyzer_diags,
                &|sel| declared.get(sel).cloned().unwrap_or_default(),
            )),
        }
    }
}

/// `--routine`'s selectors resolved by `resolve`: the pool, or the failing
/// selectors' diagnostics with #239's hint lines.
type SelectorResolution = Result<
    std::collections::BTreeSet<String>,
    (
        Vec<FingerprintQueryDiagnostic>,
        std::collections::HashMap<String, Vec<String>>,
    ),
>;

impl<'a> FingerprintJob<'a> {
    /// `Err`: `--shard` without `--out` (fingerprint.ts:210), exit 1, found
    /// before the build (#277 D3, Decision 7): it wins over a failing build
    /// and over the `--strict` gate.
    pub fn new(opts: &'a FingerprintOptions<'a>) -> Result<FingerprintJob<'a>, JobFailure> {
        if opts.shard.is_some() && opts.out.is_none() {
            return Err(JobFailure::line("--shard requires --out <directory>", 1));
        }
        Ok(FingerprintJob { opts })
    }
}

impl Job for FingerprintJob<'_> {
    fn name(&self) -> &'static str {
        "fingerprint"
    }
    fn demands(&self) -> Vec<ViewDemand> {
        let o = self.opts;
        // The cut forms stay single-app (S′); the tree forms take `--single-app`.
        let single_app = FingerprintForm::of(o) == FingerprintForm::Cut || o.single_app;
        ViewDemand::one(o.inputs, single_app, o.no_roots_config, ViewNeeds::MODEL)
    }

    fn resolve(&self, _: SideIx, cx: &ResolveCx<'_>) -> Result<PlanPart, JobFailure> {
        let opts = self.opts;
        let roots_config_ignored = opts.no_roots_config && opts.inputs.roots_config_loaded();
        let form = FingerprintForm::of(opts);
        let (plan, carry): (_, Option<Box<dyn std::any::Any + Send>>) = match form {
            FingerprintForm::Cut => (fingerprint_plan(default_detectors()), None),
            // D4: a composition with no raw facts and the inventory read.
            FingerprintForm::Inventory => (
                fingerprint_inventory_plan(
                    default_detectors(),
                    InventoryRead {
                        inputs: std::sync::Arc::clone(opts.inputs),
                        driver_version: opts.driver_version.to_string(),
                        deterministic: opts.deterministic,
                        roots_config_ignored,
                    },
                ),
                None,
            ),
            // D3 (final panel C5): the selectors resolved once, from the
            // model alone. A resolved pool asks the query read over it; a
            // failing selector asks no tree at all (O4).
            FingerprintForm::RoutineQuery => {
                let resolution = resolve_selectors(cx, opts);
                let plan = match &resolution {
                    Ok(pool) => fingerprint_query_plan(
                        default_detectors(),
                        pool.clone(),
                        QueryRead {
                            filters: query_filters(opts),
                            roots_config_ignored,
                        },
                    ),
                    Err(_) => fingerprint_plan(default_detectors()),
                };
                (plan, Some(Box::new(resolution)))
            }
        };
        Ok(PlanPart {
            plan,
            reads: if form == FingerprintForm::Cut {
                vec![]
            } else {
                vec![Cell::TreeRead]
            },
            carry,
        })
    }

    fn compute(&self, _: SideIx, cx: &mut ViewCx<'_>) -> Result<SideDoc, JobFailure> {
        Ok(SideDoc::new(match FingerprintForm::of(self.opts) {
            FingerprintForm::Cut => FingerprintDoc::Done(fingerprint_view(cx, self.opts)),
            FingerprintForm::RoutineQuery => routine_query_view(cx, self.opts),
            FingerprintForm::Inventory => FingerprintDoc::Done(inventory_view(cx, self.opts)),
        }))
    }

    fn view_failed(&self, _: SideIx, failure: &ViewFailure) -> Result<SideDoc, JobFailure> {
        Ok(SideDoc::new(FingerprintDoc::Done(Err(failure
            .target
            .message("fingerprint")))))
    }

    fn serialize(&self, doc: Doc) -> JobOutput {
        let r = match doc
            .one::<FingerprintDoc>()
            .unwrap_or_else(|f| unreachable!("fingerprint fails no side: {f:?}"))
            .into_result(self.opts)
        {
            Err(msg) => return JobOutput::lines(&[msg], 1),
            Ok(r) => r,
        };
        // A stderr-only error (selector error, strict gate): the message or
        // the diagnostics block, no output.
        if let Some(msg) = r.selector_error_message {
            return JobOutput::lines(&[msg], r.exit_code);
        }
        if r.exit_code == 1 && !r.stderr_diagnostics.is_empty() {
            return JobOutput::lines(&r.stderr_diagnostics, 1);
        }
        let mut actions = Vec::new();
        let out = self.opts.out;
        match r.output {
            // Empty text is not written.
            FingerprintOutput::Text(t) if t.is_empty() => {}
            FingerprintOutput::Text(t) => actions.push(WriteAction::out(out, t)),
            FingerprintOutput::Binary(b) => actions.push(WriteAction::out(out, b)),
            // Shards go into the `--out` directory (the current one when absent).
            FingerprintOutput::Shards(shards) => {
                let dir = std::path::Path::new(out.unwrap_or("."));
                actions.push(WriteAction::Dir(dir.to_path_buf()));
                actions.extend(
                    shards
                        .into_iter()
                        .map(|(name, bytes)| WriteAction::File(dir.join(name), bytes)),
                );
            }
        }
        // Per-mode stderr diagnostics AFTER the output (human/base-json/cbor).
        actions.extend(r.stderr_diagnostics.iter().map(|l| WriteAction::line(l)));
        JobOutput {
            actions,
            exit: r.exit_code,
        }
    }
}

/// The cut forms off their prepared view (never `Err`: the build errors are
/// the view's).
fn fingerprint_view(
    cx: &mut ViewCx<'_>,
    opts: &FingerprintOptions,
) -> Result<FingerprintRunResult, String> {
    // Honest `rootsConfigIgnored`: true only when a `roots.config.json` actually
    // existed (and would have loaded+validated) AND --no-roots-config skipped it.
    // Passing --no-roots-config in a workspace with no such file is a no-op, not
    // a lie — it must report `false`, same as never having passed the flag.
    let roots_config_ignored = opts.no_roots_config && opts.inputs.roots_config_loaded();

    // The default set's run (#160 §4.4.4); the run and its cells are released
    // before snapshot composition, as today's context died inside the runner.
    let key = cx.keys()[0].clone();
    let analyzer_diags = analyzer_diags_for(cx.view(), &key);
    drop(cx.take_run(&key));
    cx.release_all_but(&[]);
    let resolved = cx.view().model();

    // --strict gate: BEFORE composeSnapshot, before any output / sharding.
    if let Some(r) = strict_gate(opts, &analyzer_diags) {
        return Ok(r);
    }

    if go_b0(opts) {
        let envelope_diags: Vec<EnvelopeDiagnostic> = analyzer_diags
            .iter()
            .map(|(code, severity, message)| EnvelopeDiagnostic {
                code: code.clone(),
                severity: severity.clone(),
                message: message.clone(),
            })
            .collect();

        let full_opts = FullSnapshotOptions {
            inputs: opts.inputs,
            driver_version: opts.driver_version,
            deterministic: opts.deterministic,
            roots_config_ignored,
            universe: crate::engine::l5::snapshot_full::SnapshotUniverse::SingleApp,
        };
        let tree = compose_full_snapshot(resolved, &full_opts);

        // stderr diagnostics: base-json / cbor / shard print analyze errors to
        // stderr at exit (fingerprint.ts:242, 332). Shard path returns at :223
        // BEFORE the stderr loop, so shards emit NO stderr diagnostics.
        let stderr_diags: Vec<String> = if opts.shard.is_some() {
            Vec::new()
        } else {
            analyzer_diags
                .iter()
                .map(|(_, sev, msg)| format!("{sev}: {msg}"))
                .collect()
        };

        let output = if let Some(mode) = opts.shard {
            // serialize_sharded: primaryOnly = (mode == PrimaryOnly).
            let primary_only = mode == ShardMode::PrimaryOnly;
            let shards = serialize_sharded(&tree, resolved, opts.driver_version, primary_only);
            FingerprintOutput::Shards(shards.into_iter().map(|s| (s.name, s.bytes)).collect())
        } else if opts.format == FingerprintFormat::Cbor {
            FingerprintOutput::Binary(serialize_cbor(&tree))
        } else if opts.format == FingerprintFormat::CborGz {
            FingerprintOutput::Binary(serialize_cbor_gz(&tree))
        } else {
            // json, no query
            let text = serialize_envelope(
                &tree,
                opts.driver_version,
                opts.deterministic,
                &envelope_diags,
            );
            FingerprintOutput::Text(text)
        };

        return Ok(FingerprintRunResult {
            output,
            exit_code: 0,
            selector_error_message: None,
            stderr_diagnostics: stderr_diags,
        });
    }

    // Query path with no `--routine`: json-with-query or human, on the cut.
    // Compose the consumed-core snapshot (not the full CBOR tree). With no
    // selector, no selector can fail.
    let snap = compose_snapshot(resolved);
    let filters = query_filters(opts);
    let owners = OwnerIndex::build(resolved);
    let primary = primary_stable_ids(resolved);
    let result = fingerprint_query(
        &snap,
        &filters,
        None,
        roots_config_ignored,
        &owners,
        &|id| primary.contains(id),
    );
    Ok(emit_query(
        opts,
        &result,
        &filters,
        &analyzer_diags,
        &|_| Vec::new(),
    ))
}

/// The query's filters from the CLI options; the witness limit defaults to 3
/// (the golden capturePoint default).
fn query_filters(opts: &FingerprintOptions) -> FingerprintFilters {
    FingerprintFilters {
        roots: opts.roots.clone(),
        routine_selectors: opts.routine_selectors.clone(),
        include_inherited: opts.include_inherited,
        witness_limit: opts.witness_limit.unwrap_or(WitnessLimit::Capped(3)),
    }
}

/// `fingerprint --routine`'s selectors, resolved from the model alone (#160
/// step 9 D3, final panel C5), with #239's hint lines for each unresolved
/// selector, kept while the model's owner index is at hand (read only by a
/// human selector error).
fn resolve_selectors(cx: &ResolveCx<'_>, opts: &FingerprintOptions) -> SelectorResolution {
    let model = cx.model();
    let idx = build_selector_indexes(cx.ids(), primary_guid_of(model));
    let owners = OwnerIndex::build(model);
    resolve_routine_selectors(&opts.routine_selectors, &idx, &owners).map_err(|diags| {
        let mut declared = std::collections::HashMap::new();
        for d in &diags {
            if let FingerprintQueryDiagnostic::SelectorUnresolved { selector } = d {
                let lines = crate::engine::l5::selector_index::owner_check_candidates(
                    selector,
                    &idx,
                    |id, display| owners.qualified(id, display),
                );
                declared.insert(selector.clone(), lines);
            }
        }
        (diags, declared)
    })
}

/// `fingerprint --routine` (#160 step 9 D3) off its prepared view. A resolved
/// pool: the query was read off the view's tree inside `prepare` (the tree is
/// dropped there), and the projection runs after the view is gone, reading
/// only the owned result (O3, [`FingerprintDoc::Query`]). A failing selector:
/// no tree at all, the result off the model (O4).
///
/// `--strict` gates after `prepare`, before any output: its diagnostics are the
/// detector run's, and that run reads the one composition the tree reads, so
/// they exist only once the tree has been composed (the cut composed twice:
/// once for its run, once again, after the gate, for the snapshot).
fn routine_query_view(cx: &mut ViewCx<'_>, opts: &FingerprintOptions) -> FingerprintDoc {
    let roots_config_ignored = opts.no_roots_config && opts.inputs.roots_config_loaded();
    let resolution: SelectorResolution = cx.take_carry();
    let key = cx.keys()[0].clone();
    let analyzer_diags = analyzer_diags_for(cx.view(), &key);
    drop(cx.take_run(&key));
    let (result, declared) = match resolution {
        Ok(_) => {
            let TreeReadOut::FingerprintQuery(result) = cx.take_tree_read() else {
                unreachable!("a FingerprintQuery read leaves a query result")
            };
            cx.release_all_but(&[]);
            (result, Default::default())
        }
        Err((diags, declared)) => {
            cx.release_all_but(&[]);
            let result = selector_failure_result(cx.view().model(), diags, roots_config_ignored);
            (result, declared)
        }
    };
    if let Some(r) = strict_gate(opts, &analyzer_diags) {
        return FingerprintDoc::Done(Ok(r));
    }
    cx.view().census_output();
    FingerprintDoc::Query {
        result,
        analyzer_diags,
        declared,
    }
}

/// `fingerprint --inventory-only` (#160 step 9 D4) off its prepared view: a
/// composition with no raw facts, and the five inventory keys assembled off
/// its tree inside `prepare` (the tree is dropped there);
/// `build_inventory_envelope` runs unchanged over them. Its stderr
/// diagnostics and `--strict` are B0's (the inventory was a B0 form).
fn inventory_view(
    cx: &mut ViewCx<'_>,
    opts: &FingerprintOptions,
) -> Result<FingerprintRunResult, String> {
    let key = cx.keys()[0].clone();
    let analyzer_diags = analyzer_diags_for(cx.view(), &key);
    drop(cx.take_run(&key));
    let TreeReadOut::Inventory(keys) = cx.take_tree_read() else {
        unreachable!("an Inventory read leaves the inventory keys")
    };
    cx.release_all_but(&[]);
    if let Some(r) = strict_gate(opts, &analyzer_diags) {
        return Ok(r);
    }
    cx.view().census_output();
    let text = build_inventory_envelope(
        &crate::engine::gate::cbor::CborValue::Map(keys),
        cx.view().model(),
        opts.driver_version,
        opts.deterministic,
    );
    Ok(FingerprintRunResult {
        output: FingerprintOutput::Text(text),
        exit_code: 0,
        selector_error_message: None,
        stderr_diagnostics: analyzer_diags
            .iter()
            .map(|(_, sev, msg)| format!("{sev}: {msg}"))
            .collect(),
    })
}

/// `--strict` (fingerprint.ts:187): if any analyzer diagnostic is severity
/// "error", every analyzer diagnostic goes to stderr and the command exits 1,
/// before any output.
fn strict_gate(
    opts: &FingerprintOptions,
    analyzer_diags: &[(String, String, String)],
) -> Option<FingerprintRunResult> {
    if !opts.strict || !analyzer_diags.iter().any(|(_, sev, _)| sev == "error") {
        return None;
    }
    Some(FingerprintRunResult {
        output: FingerprintOutput::Text(String::new()),
        exit_code: 1,
        selector_error_message: None,
        stderr_diagnostics: analyzer_diags
            .iter()
            .map(|(_, sev, msg)| format!("{sev}: {msg}"))
            .collect(),
    })
}

/// The query's output (json or human) from its result, shared by the cut and
/// the view's tree. `declared_owners` gives #239's hint for an unresolved
/// selector: the owners that do declare its routine (read only by a human
/// selector error; a query with no failing selector passes an empty one).
fn emit_query(
    opts: &FingerprintOptions,
    result: &crate::engine::l5::fingerprint_query::FingerprintQueryResult,
    filters: &FingerprintFilters,
    analyzer_diags: &[(String, String, String)],
    declared_owners: &dyn Fn(&str) -> Vec<String>,
) -> FingerprintRunResult {
    let workspace = opts.inputs.workspace.as_path();
    let workspace_fp = crate::engine::l5::snapshot_full::workspace_fingerprint_of(
        opts.inputs,
        opts.driver_version,
    );
    let selector_errors: Vec<&FingerprintQueryDiagnostic> = result
        .diagnostics
        .iter()
        .filter(|d| {
            matches!(
                d,
                FingerprintQueryDiagnostic::SelectorUnresolved { .. }
                    | FingerprintQueryDiagnostic::SelectorAmbiguous { .. }
            )
        })
        .collect();
    let has_selector_errors = !selector_errors.is_empty();
    let exit_code: u8 = if has_selector_errors { 2 } else { 0 };

    if opts.format == FingerprintFormat::Human {
        if has_selector_errors {
            // Human selector errors → stderr, exit 2, no stdout (fingerprint.ts:301).
            // #239: an `<owner>::<routine>` selector whose owner declares no such
            // routine gets one hint line naming the owners that do.
            let mut msg = String::new();
            for d in selector_errors {
                msg.push_str(&format_selector_errors_human(std::slice::from_ref(d)));
                if let FingerprintQueryDiagnostic::SelectorUnresolved { selector } = d {
                    let declared = declared_owners(selector);
                    if !declared.is_empty() {
                        msg.push_str(&format!(
                            "  no routine of that name on the named owner; declared as: {}\n",
                            declared.join(", ")
                        ));
                    }
                }
            }
            return FingerprintRunResult {
                output: FingerprintOutput::Text(String::new()),
                exit_code: 2,
                // Strip the single trailing newline — alsem.rs re-adds it via eprintln!.
                selector_error_message: Some(msg.trim_end_matches('\n').to_string()),
                stderr_diagnostics: Vec::new(),
            };
        }
        let human = format_fingerprint_human_verbosity(result, opts.verbosity);
        // human format prints analyze diagnostics to stderr at exit (fingerprint.ts:331).
        let stderr_diags: Vec<String> = analyzer_diags
            .iter()
            .map(|(_, sev, msg)| format!("{sev}: {msg}"))
            .collect();
        return FingerprintRunResult {
            output: FingerprintOutput::Text(human),
            exit_code: 0,
            selector_error_message: None,
            stderr_diagnostics: stderr_diags,
        };
    }

    // json + query: selector errors are embedded in payload.diagnostics + exit 2,
    // and NO analyze errors go to stderr (fingerprint.ts:281-297).
    let json_text = project_fingerprint_query_full(
        result,
        &workspace.to_string_lossy(),
        filters,
        opts.deterministic,
        analyzer_diags,
        opts.driver_version,
        &workspace_fp,
    );

    FingerprintRunResult {
        output: FingerprintOutput::Text(json_text),
        exit_code,
        selector_error_message: None,
        stderr_diagnostics: Vec::new(),
    }
}

/// Build the analyzer diagnostics list as `(code, severity, message)` tuples,
/// shared by the strict gate, the B0 envelope path, and the query JSON envelope.
fn analyzer_diags_for(view: &View, key: &RunKey) -> Vec<(String, String, String)> {
    analyzer_diagnostics_of_view(view, key)
        .into_iter()
        .map(|d| (format!("DIAG-{}", d.stage), d.severity, d.message))
        .collect()
}

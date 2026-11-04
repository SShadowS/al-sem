//! `alsem query` — the shipped transport over
//! [`crate::engine::l4::effect_query::DbEffectQuery`], and the home of the
//! `SolveIx -> ModelRoutine` join that turns an index answer into a user-facing
//! one.
//!
//! ## Why this exists at all
//!
//! `ReverseEffectIndex` sat for an arc with **zero production callers** — every
//! `build()` was inside its own `#[cfg(test)]` module — because its intended
//! consumer (a VSCode hover) is months away and its wake condition lived in a
//! document that belonged to nobody. The general fix is not a better-written
//! wake condition; it is **putting the code on a path something already runs**.
//! So this module exists to be run: its goldens live in
//! `tests/cli-query-goldens/` and are wired into `scripts/check-goldens`, which
//! `scripts/git-hooks/pre-commit` enforces. `ReverseEffectIndex::build`, both
//! up-queries, the ancestor BFS and the facade therefore execute on every
//! golden run by anyone, forever, with no discipline required.
//!
//! ## Pipeline ownership
//!
//! It builds a `View` with query's plan (#160 §4.4.4): no detector run, the
//! `rel` and `l4` cells alone (assemble → `ExecRelation` with its SCC →
//! `build_l4`: `compute_summaries_v2_bundle_with_leaves` → `DbEffectQuery`). It never
//! builds a `DetectorContext`, so it costs `alsem analyze` exactly nothing
//! **by construction** — not by a flag that could be set wrong. The
//! `substrate::DB_EFFECT_REVERSE_INDEX` bit exists for the eventual
//! detector/LSP path, not for this CLI.
//!
//! ## No caps
//!
//! The unscoped up-query (`--table T` with no `--from`) is workspace-global and
//! its DO median answer is 377 routines. This surface makes that VISIBLE by
//! reporting `routineCount` before the list — it does **not** truncate the
//! list. Capping/sampling was explicitly rejected for this engine in a prior
//! arc; a count that says "377" and a complete list is honest, a silently
//! elided list is not.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use crate::engine::l4::routine_interner::SolveIx;

use serde_json::{Map, Value, json};

use crate::engine::batch::job::{
    Doc, Job, JobFailure, PlanPart, ResolveCx, SideDoc, SideIx, ViewCx, ViewDemand, ViewFailure,
    ViewNeeds, WorkspaceRead,
};
use crate::engine::batch::output::JobOutput;
use crate::engine::gate::format_json::{pinned_or_now_iso8601, serialize_document_value};
use crate::engine::gate::run::AnalysisTarget;
use crate::engine::l4::effect_lattice::TempStateKind;
use crate::engine::l4::effect_query::{
    AncestorTouch, CallerOutcome, CallerTouch, DbEffectQuery, TableTouch, TouchClass, TouchCounts,
    UNKNOWN_TABLE_ID, is_unknown_table,
};
use crate::engine::l4::operation_index::OperationIndex;
use crate::engine::line_base::{LINE_BASE, shown};
use crate::engine::session::{Cell, View, query_cone_plan, query_plan};
use crate::program::body::features::RECEIVER_PARAM_INDEX;
use crate::program::model::declaring::DeclaringObjects;
use crate::program::model::workspace::{Model, ModelRoutine, table_by_id_preferring_real};
use crate::program::resolve::full::LoadedDependencies;

/// The JSON envelope's `schemaVersion` for both `query` subcommands. 1.1.0 (#66):
/// additive `payload.dependencies` {mode, apps}. 2.0.0 (MCP output plan 1a):
/// every `line` is 1-based, and `payload.lineBase: 1` says so. 3.0.0 (MCP output
/// wave 2, spec §4.6): `query touches`' default `up` no longer carries
/// `witnesses` (only `--all` does); it states the probe counts, the
/// `notTouching` and `unknown` callers, and in the informative case one
/// `callers[]` row per touching caller; `down` adds `counts`, `beyond` and each
/// witness's `touchClass`. The spec gives `query effects`' W2-2 changes the same
/// 3.0.0, so W2-2 adds to this major rather than bumping again. 3.1.0 (wave
/// 3a, additive): an unresolved-table row whose receiver is a RecordRef says
/// `receiver: "RecordRef"`, and the summaries count them
/// (`tableNotResolved.recordRef`, `down.counts.tableNotResolvedRecordRef`);
/// the reserved `beyond` fields and `summary.scope` are filled from the
/// capability cone.
const QUERY_SCHEMA_VERSION: &str = "3.1.0";

/// Which direction(s) a `query touches` run reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Down,
    Up,
    Both,
}

impl Direction {
    pub fn parse(s: &str) -> Result<Self, String> {
        match s {
            "down" => Ok(Direction::Down),
            "up" => Ok(Direction::Up),
            "both" => Ok(Direction::Both),
            other => Err(format!(
                "query: --direction must be one of down|up|both (got {other:?})"
            )),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Direction::Down => "down",
            Direction::Up => "up",
            Direction::Both => "both",
        }
    }

    fn wants_down(self) -> bool {
        matches!(self, Direction::Down | Direction::Both)
    }

    fn wants_up(self) -> bool {
        matches!(self, Direction::Up | Direction::Both)
    }
}

/// One `alsem query` run's rendered output.
pub struct QueryRunResult {
    pub json_text: String,
    pub human_text: String,
    /// `0` = answered; `2` = a selector did not resolve to exactly one target
    /// (mirrors `alsem prove`'s unresolved-routine contract).
    pub exit_code: u8,
    /// The answer's summary for a file answer (MCP output plan 1e); `null` on a
    /// selector failure.
    pub summary: Value,
    /// `query touches`: the caller witnesses the answer built
    /// ([`DbEffectQuery::ancestor_witnesses_built`]); 0 in the default trivial
    /// case (T-W2-10). Not output — read by tests.
    #[doc(hidden)]
    pub ancestor_witnesses_built: usize,
}

// ---------------------------------------------------------------------------
// The owned substrate (one workspace load per invocation).
// ---------------------------------------------------------------------------

/// What a [`DbEffectQuery`] reads, off a [`View`] whose `l4` cell query's plan
/// filled (no detector run, #160 §4.4.4): the model and the `l4` analysis (the
/// bundle and its SCC) by borrow, so a query shares its view with the batch's
/// other jobs (#277 D9), and the relation's graph by its own `Arc`, so the
/// relation itself can go before the render ([`QueryJob`]'s compute).
///
/// Cross-app (the default, #66), the view is `alsem analyze`'s cross-app base:
/// its graph and SCC, with every source-bearing dependency routine a FIXED
/// leaf holding its own direct db effects, so a dependency's tables resolve
/// and are selectable. With no dependency there are no leaves and the same
/// graph, so the two modes agree.
pub struct QuerySubstrate<'a> {
    model: &'a Model,
    graph: std::sync::Arc<crate::engine::l4::combined_graph::PrimaryGraph>,
    l4: &'a crate::engine::l4::analysis::L4Analysis,
    /// Which dependencies the model holds (#66), stated in every document.
    pub dependencies: LoadedDependencies,
}

impl<'a> QuerySubstrate<'a> {
    /// The substrate of a view prepared with [`query_plan`].
    pub fn of(view: &'a View) -> Self {
        Self::with_graph(view, std::sync::Arc::clone(view.rel().primary()))
    }

    fn with_graph(
        view: &'a View,
        graph: std::sync::Arc<crate::engine::l4::combined_graph::PrimaryGraph>,
    ) -> Self {
        QuerySubstrate {
            model: view.model(),
            graph,
            l4: view.l4(),
            dependencies: view.target().dependencies(),
        }
    }

    /// The model the selectors and joins read.
    pub fn resolved(&self) -> &Model {
        self.model
    }

    /// The query surface over this substrate.
    pub fn query(&self) -> DbEffectQuery<'_> {
        DbEffectQuery::build(&self.l4.bundle, &self.l4.scc, &self.graph)
    }

    /// How many owners the graph and the SCC have (#160 Task 27 review I1).
    #[cfg(test)]
    pub(crate) fn shared_owners(&self) -> (usize, usize) {
        (
            std::sync::Arc::strong_count(&self.graph),
            std::sync::Arc::strong_count(&self.l4.scc),
        )
    }
}

/// The query's substrate off its prepared view, with what the render does not
/// read released first (#160 Task 27 review I1): the relation (its cone
/// payload; the substrate keeps the graph's `Arc`) and the cross-app base's
/// prepare-only parts. A later job on the view keeps what it declared.
pub(crate) fn released_substrate<'v>(cx: &'v mut ViewCx<'_>) -> QuerySubstrate<'v> {
    let graph = std::sync::Arc::clone(cx.view().rel().primary());
    cx.release_all_but(&[Cell::L4]);
    cx.release(Cell::Base);
    QuerySubstrate::with_graph(cx.view(), graph)
}

/// Test entry: a workspace's view prepared with [`query_plan`], outside the
/// batch driver, for tests that render many queries off one build.
#[doc(hidden)]
pub fn query_view(
    inputs: &std::sync::Arc<crate::program::inputs::WorkspaceInputs>,
    single_app: bool,
) -> Result<View, String> {
    let mut view = View::new(AnalysisTarget::build(inputs, single_app, "query")?);
    view.prepare(&query_plan());
    Ok(view)
}

/// What an `alsem query` job answers.
enum QueryAsk<'a> {
    Touches {
        table: &'a str,
        from: Option<&'a str>,
        direction: Direction,
        all: bool,
    },
    Effects {
        routine: &'a str,
        /// `--all`: every row in the human text (W2-2).
        all: bool,
    },
}

/// `alsem query touches` / `effects` as a batch job (#277 D1). Its document is
/// `Result<QueryRunResult, String>`; its writes are the `--format` text (to
/// `--out` or stdout), or an error line and exit 1.
pub struct QueryJob<'a> {
    inputs: &'a dyn WorkspaceRead,
    ask: QueryAsk<'a>,
    alsem_ver: &'a str,
    deterministic: bool,
    single_app: bool,
    /// `--format json`: the JSON text, else the human text.
    json: bool,
    out: Option<&'a str>,
    /// `--show-source` (W2-7): the cited line ±2 under each human location.
    show_source: bool,
}

impl<'a> QueryJob<'a> {
    /// `alsem query touches` (see [`run_query_touches_pipeline`]), written as
    /// JSON to stdout until [`QueryJob::written`]. `all` is `--all`: every
    /// witness, the callers' included, and no fold.
    #[allow(clippy::too_many_arguments)]
    pub fn touches(
        inputs: &'a dyn WorkspaceRead,
        table: &'a str,
        from: Option<&'a str>,
        direction: Direction,
        all: bool,
        alsem_ver: &'a str,
        deterministic: bool,
        single_app: bool,
    ) -> QueryJob<'a> {
        let ask = QueryAsk::Touches {
            table,
            from,
            direction,
            all,
        };
        Self::with(inputs, ask, alsem_ver, deterministic, single_app)
    }

    /// `alsem query effects` (see [`run_query_effects_pipeline`]).
    pub fn effects(
        inputs: &'a dyn WorkspaceRead,
        routine: &'a str,
        alsem_ver: &'a str,
        deterministic: bool,
        single_app: bool,
    ) -> QueryJob<'a> {
        let ask = QueryAsk::Effects {
            routine,
            all: false,
        };
        Self::with(inputs, ask, alsem_ver, deterministic, single_app)
    }

    /// `query effects --all` (W2-2): every row in the human text too.
    #[must_use]
    pub fn all_rows(mut self, all_rows: bool) -> QueryJob<'a> {
        if let QueryAsk::Effects { all, .. } = &mut self.ask {
            *all = all_rows;
        }
        self
    }

    fn with(
        inputs: &'a dyn WorkspaceRead,
        ask: QueryAsk<'a>,
        alsem_ver: &'a str,
        deterministic: bool,
        single_app: bool,
    ) -> QueryJob<'a> {
        QueryJob {
            inputs,
            ask,
            alsem_ver,
            deterministic,
            single_app,
            json: true,
            out: None,
            show_source: false,
        }
    }

    /// `--show-source` (W2-7).
    #[must_use]
    pub fn show_source(mut self, show_source: bool) -> QueryJob<'a> {
        self.show_source = show_source;
        self
    }

    /// The text `--format` names (`json` or human), to `--out` or stdout.
    pub fn written(self, json: bool, out: Option<&'a str>) -> QueryJob<'a> {
        QueryJob { json, out, ..self }
    }

    /// The job's result, through the driver.
    fn run(&self) -> Result<QueryRunResult, String> {
        crate::engine::batch::run_doc(self)
            .one()
            .unwrap_or_else(|f| unreachable!("query fails no side: {f:?}"))
    }
}

impl Job for QueryJob<'_> {
    fn name(&self) -> &'static str {
        "query"
    }
    fn demands(&self) -> Vec<ViewDemand> {
        ViewDemand::one(self.inputs, self.single_app, false, ViewNeeds::MODEL)
    }

    fn resolve(&self, _: SideIx, cx: &ResolveCx<'_>) -> Result<PlanPart, JobFailure> {
        // Wave 3a: a question about one routine's cone (effects, or touches
        // asked down from a routine) also reads the capability cone over
        // that root, for the writes beyond each dependency entry point. A
        // selector that does not resolve reads none (its answer is the
        // failure document, as before).
        let model = cx.model();
        let root = cone_root(model, &self.ask);
        let reads = if root.is_some() {
            vec![Cell::Rel, Cell::L4, Cell::Effects]
        } else {
            vec![Cell::Rel, Cell::L4]
        };
        Ok(PlanPart {
            plan: query_cone_plan(root.clone()),
            reads,
            carry: Some(Box::new(root)),
        })
    }

    fn compute(&self, _: SideIx, cx: &mut ViewCx<'_>) -> Result<SideDoc, JobFailure> {
        let workspace = cx.inputs().workspace.clone();
        let root: Option<String> = cx.take_carry();
        // The cone's entry first: the substrate below releases every cell
        // but `l4`, the tree's effects included.
        let cone = match root {
            Some(root) if cx.has_tree() => cone_entry(cx, root),
            _ => None,
        };
        let substrate = released_substrate(cx);
        let cone = cone.as_ref();
        let body = match self.ask {
            QueryAsk::Touches {
                table,
                from,
                direction,
                all,
            } => render_query_touches_body(
                &substrate,
                table,
                from,
                direction,
                all,
                self.alsem_ver,
                self.deterministic,
                cone,
            ),
            QueryAsk::Effects { routine, all } => render_query_effects_body(
                &substrate,
                routine,
                all,
                self.alsem_ver,
                self.deterministic,
                cone,
            ),
        };
        let sources = (self.show_source && !self.json)
            .then(|| crate::engine::source_excerpt::SourceTexts::read(&workspace));
        let result = finish_human(body, &substrate.dependencies, sources.as_ref());
        Ok(SideDoc::new(Ok::<_, String>(result)))
    }

    fn view_failed(&self, _: SideIx, failure: &ViewFailure) -> Result<SideDoc, JobFailure> {
        Ok(SideDoc::new(Err::<QueryRunResult, _>(
            failure.target.message("query"),
        )))
    }

    fn serialize(&self, doc: Doc) -> JobOutput {
        let r = doc
            .one::<Result<QueryRunResult, String>>()
            .unwrap_or_else(|f| unreachable!("query fails no side: {f:?}"));
        JobOutput::text_with_summary(
            self.out,
            r.map(|r| {
                let text = if self.json { r.json_text } else { r.human_text };
                (text, r.exit_code, r.summary)
            }),
        )
    }
}

/// The routine (stable id) whose capability cone a query reads (wave 3a):
/// `effects`' routine, or `touches`' `--from` when the question goes down
/// and its table resolves. `None` when the selector does not resolve to one
/// routine, so no tree is built for a failure document.
fn cone_root(model: &Model, ask: &QueryAsk<'_>) -> Option<String> {
    let selector = match *ask {
        QueryAsk::Effects { routine, .. } => routine,
        QueryAsk::Touches {
            table,
            from: Some(from),
            direction,
            ..
        } if direction.wants_down() => {
            if !matches!(
                resolve_table_selector(model, table),
                Resolution::Resolved(_)
            ) {
                return None;
            }
            from
        }
        QueryAsk::Touches { .. } => return None,
    };
    let join = RoutineJoin::build(model);
    match resolve_routine_selector(model, &join.declaring, selector) {
        Resolution::Resolved(r) => Some(r.stable_routine_id.clone()),
        _ => None,
    }
}

/// The root's digest entry over the query's tree, after the L4 temp-state
/// join and the literal pass, exactly as `alsem explain` reads it, so the
/// writes beyond each entry point are explain's own.
fn cone_entry(
    cx: &mut ViewCx<'_>,
    root: String,
) -> Option<crate::engine::l5::digest_cli::DigestEntryFull> {
    use crate::engine::l5::digest_cli::{
        ChangedRootsResult, DbTempStates, run_digest_query_full_from_entries,
    };
    let _s = crate::engine::perf_trace::span("query", "query.cone_entry");
    let mut effects = cx.take_effects();
    let view = cx.view();
    let model = view.model();
    let temp_states = DbTempStates::new(model, &view.l4().bundle);
    let roots = vec![root];
    let changed = ChangedRootsResult {
        roots: roots.clone(),
        diagnostics: Vec::new(),
        touched: HashMap::new(),
        spans: Default::default(),
    };
    run_digest_query_full_from_entries(
        &mut effects,
        &roots,
        &HashMap::new(),
        &temp_states,
        &changed,
        &OwnerIndex::build(model),
    )
    .entries
    .into_iter()
    .next()
}

/// The cone's may-persist writes beyond each dependency entry point, as the
/// renderers read them: the root's rollup and the context its lines need.
struct Beyond<'c> {
    entry: &'c crate::engine::l5::digest_cli::DigestEntryFull,
    rollup: crate::engine::l5::routine_rollup::RoutineRollup,
    rcx: crate::engine::l5::routine_rollup::RollupCx<'c>,
}

impl<'c> Beyond<'c> {
    fn new(
        entry: &'c crate::engine::l5::digest_cli::DigestEntryFull,
        model: &'c Model,
        deps: &LoadedDependencies,
    ) -> Self {
        let rcx = crate::engine::l5::routine_rollup::RollupCx::new(model, &deps.apps);
        let rollup = crate::engine::l5::routine_rollup::RoutineRollup::build(entry, &rcx);
        Beyond { entry, rollup, rcx }
    }

    fn writes(&self) -> Vec<crate::engine::l5::routine_rollup::BeyondWrites<'_>> {
        crate::engine::l5::routine_rollup::beyond_writes(&self.rollup)
    }

    /// Explain's `writes beyond dependency entry points` section, verbatim.
    fn lines(&self, all: bool) -> Vec<String> {
        crate::engine::l5::routine_rollup::beyond_lines(
            &self.rollup,
            self.entry,
            &self.rcx,
            crate::engine::l5::routine_rollup::RenderOpts::new(all, None),
        )
    }
}

/// `beyond[]` of one table: each entry point beyond which it may persist,
/// with the write kinds there.
fn beyond_of_table(
    writes: &[crate::engine::l5::routine_rollup::BeyondWrites<'_>],
    table_id: &str,
) -> Value {
    Value::Array(
        writes
            .iter()
            .filter(|w| w.tables.contains_key(table_id))
            .map(|w| {
                json!({
                    "entryPoint": w.name,
                    "sites": w.sites,
                    "ops": w.types_of.get(table_id).cloned().unwrap_or_default(),
                })
            })
            .collect(),
    )
}

/// The summary scope, by whether the cone was read.
fn scope_value(cone_read: bool) -> &'static str {
    if cone_read {
        "operations-one-routine-deep; writes-beyond-entry-points-full-cone"
    } else {
        "operations-one-routine-deep"
    }
}

// ---------------------------------------------------------------------------
// Selector resolution.
// ---------------------------------------------------------------------------

/// How a selector resolved. Rendered verbatim into the payload so an ambiguous
/// selector is never silently narrowed to its first candidate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolution<T> {
    Resolved(T),
    Unmatched,
    /// >= 2 candidates, rendered in deterministic (sorted) order.
    Ambiguous(Vec<String>),
}

impl<T> Resolution<T> {
    fn label(&self) -> &'static str {
        match self {
            Resolution::Resolved(_) => "resolved",
            Resolution::Unmatched => "unmatched",
            Resolution::Ambiguous(_) => "ambiguous",
        }
    }
}

/// Resolve `--table`: an exact internal table id, the literal
/// [`UNKNOWN_TABLE_ID`] sentinel, or a table NUMBER or case-insensitive NAME
/// (one tier: two distinct matches are ambiguous). In cross-app mode (#66) that
/// includes the dependencies' tables, so `--table 18` selects Base Application's
/// Customer.
///
/// Name resolution deliberately goes through [`table_by_id_preferring_real`] so
/// a `tableextension` STUB never wins an id collision with the real table it
/// extends (G-5) — a stub's fields are the extension's, and answering a
/// db-effect question against it would name the wrong object.
///
/// The `"unknown"` sentinel is accepted as a first-class selector: it is a real,
/// queryable effect population ("effects whose target table could not be
/// determined") and on DO it is the largest one. It is never produced by NAME
/// resolution — only by typing it exactly — so a workspace containing a table
/// literally named `unknown` cannot silently shadow the bucket, nor vice versa.
pub fn resolve_table_selector(resolved: &Model, selector: &str) -> Resolution<TableRef> {
    resolve_table_selector_in(&resolved.workspace.tables, selector)
}

fn resolve_table_selector_in(
    tables: &[crate::program::model::workspace::ModelTable],
    selector: &str,
) -> Resolution<TableRef> {
    if selector == UNKNOWN_TABLE_ID {
        return Resolution::Resolved(TableRef {
            id: UNKNOWN_TABLE_ID.to_string(),
            name: None,
        });
    }
    let real_by_id = table_by_id_preferring_real(tables);

    if let Some(t) = real_by_id.get(selector) {
        return Resolution::Resolved(TableRef {
            id: t.id.clone(),
            name: Some(t.name.clone()),
        });
    }

    // One tier: the table NUMBER (an all-digit selector; a `tableextension` stub's
    // number is the extension's, not a table's, so stubs never match it) or the
    // case-insensitive NAME. A table literally named "18" and table 18 are then
    // both candidates of an ambiguity, never one silently shadowing the other.
    let number: Option<i64> =
        if !selector.is_empty() && selector.bytes().all(|b| b.is_ascii_digit()) {
            selector.parse().ok()
        } else {
            None
        };
    let want = selector.to_lowercase();
    let mut hits: Vec<&&crate::program::model::workspace::ModelTable> = real_by_id
        .values()
        .filter(|t| {
            number.is_some_and(|n| !t.is_extension_stub && t.table_number == n)
                || t.name.to_lowercase() == want
        })
        .collect();
    hits.sort_by(|a, b| a.id.cmp(&b.id));
    match hits.len() {
        0 => Resolution::Unmatched,
        1 => Resolution::Resolved(TableRef {
            id: hits[0].id.clone(),
            name: Some(hits[0].name.clone()),
        }),
        _ => Resolution::Ambiguous(
            hits.iter()
                .map(|t| format!("{} ({})", t.name, t.id))
                .collect(),
        ),
    }
}

/// A resolved table target. `name` is `None` for the [`UNKNOWN_TABLE_ID`]
/// bucket, which has no name because it is not a table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableRef {
    pub id: String,
    pub name: Option<String>,
}

/// Resolve `--from` / `--routine` to exactly one workspace routine. Cascade,
/// most specific first: internal id, `stableRoutineId`, `Object.Routine`
/// (case-insensitive), bare routine name (case-insensitive). Candidates are
/// sorted by internal id so an ambiguity report is deterministic.
///
/// Only the workspace app's routines are candidates: a cross-app model (#66) also
/// holds dependency routines, and a bare `OnRun` must not become ambiguous with
/// every dependency's. `declaring` is the run's one [`DeclaringObjects`] (the
/// `RoutineJoin`'s), which names the `Object` part.
pub fn resolve_routine_selector<'a>(
    resolved: &'a Model,
    declaring: &DeclaringObjects<'_>,
    selector: &str,
) -> Resolution<&'a ModelRoutine> {
    let ws = &resolved.workspace;
    let object_name = |r: &ModelRoutine| declaring.of(r).map_or("", |o| o.name.as_str());
    let routines = || {
        ws.routines
            .iter()
            .filter(|r| is_primary_routine(resolved, r))
    };

    if let Some(r) = routines().find(|r| r.id == selector) {
        return Resolution::Resolved(r);
    }
    if let Some(r) = routines().find(|r| r.stable_routine_id == selector) {
        return Resolution::Resolved(r);
    }

    let want = selector.to_lowercase();
    let qualified: Vec<&ModelRoutine> = routines()
        .filter(|r| format!("{}.{}", object_name(r), r.name).to_lowercase() == want)
        .collect();
    let bare: Vec<&ModelRoutine> = routines()
        .filter(|r| r.name.to_lowercase() == want)
        .collect();

    // Most specific non-empty tier wins; an ambiguity inside a tier does NOT
    // fall through to a looser tier (that would answer a different question).
    let hits = if !qualified.is_empty() {
        qualified
    } else {
        bare
    };
    let mut hits = hits;
    hits.sort_by(|a, b| a.id.cmp(&b.id));
    match hits.len() {
        0 => Resolution::Unmatched,
        1 => Resolution::Resolved(hits[0]),
        _ => Resolution::Ambiguous(
            hits.iter()
                .map(|r| format!("{}.{} ({})", object_name(r), r.name, r.stable_routine_id))
                .collect(),
        ),
    }
}

/// Whether `r` belongs to the workspace app (#66). Routine selectors and the
/// workspace-global touch list stay there; a dependency routine still shows as an
/// effect origin or an ancestor reached through an event. No primary app identity
/// keeps every routine.
fn is_primary_routine(resolved: &Model, r: &ModelRoutine) -> bool {
    resolved.routine_role(r) == crate::program::model::role::Role::Primary
}

// ---------------------------------------------------------------------------
// The SolveIx -> ModelRoutine join (scope §1.5: SolveIx is not an identity).
// ---------------------------------------------------------------------------

/// The user-facing identity of a routine. `SolveIx` is an internal dense
/// index and `SummaryBundle::routine_id` yields the INTERNAL id
/// (`<appGuid>:Codeunit:6175271#<bodyhash>`) — neither is renderable, so every
/// surface joins through here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoutineRef {
    pub display: String,
    pub object_display: String,
    pub stable_id: String,
    pub file: String,
    pub line: u32,
    pub column: u32,
}

impl RoutineRef {
    /// The one header rule, [`owner_qualified`], for this routine (#78).
    pub fn qualified(&self) -> String {
        owner_qualified(&self.object_display, &self.display)
    }
}

/// The join table: internal routine id -> its `ModelRoutine`, plus each
/// routine's declaring object ([`DeclaringObjects`], #241: an `#if`-split object
/// has one id for two names, so the owner is never looked up by id alone).
pub struct RoutineJoin<'a> {
    by_id: HashMap<&'a str, &'a ModelRoutine>,
    declaring: DeclaringObjects<'a>,
}

impl<'a> RoutineJoin<'a> {
    pub fn build(resolved: &'a Model) -> Self {
        let ws = &resolved.workspace;
        RoutineJoin {
            by_id: ws.routines.iter().map(|r| (r.id.as_str(), r)).collect(),
            declaring: DeclaringObjects::new(&ws.objects),
        }
    }

    /// Join an internal routine id to its renderable identity. `None` for an id
    /// that is not a workspace routine (e.g. a retained dependency leaf) —
    /// callers render a placeholder rather than inventing a name.
    pub fn get(&self, routine_id: &str) -> Option<RoutineRef> {
        let r = self.by_id.get(routine_id)?;
        Some(self.of(r))
    }

    /// The model routine behind an internal id.
    fn routine(&self, routine_id: &str) -> Option<&'a ModelRoutine> {
        self.by_id.get(routine_id).copied()
    }

    pub fn of(&self, r: &ModelRoutine) -> RoutineRef {
        let object_name = self.declaring.of(r).map_or("", |o| o.name.as_str());
        RoutineRef {
            display: r.name.clone(),
            object_display: object_display(&r.object_type, object_name),
            stable_id: r.stable_routine_id.clone(),
            file: r.source_anchor.source_unit_id.clone(),
            line: r.source_anchor.start_line,
            column: r.source_anchor.start_column,
        }
    }
}

/// The one owner rule: `"<ObjectType> <ObjectName>"`, the bare type when the
/// name is missing. `query effects`, digest, fingerprint and prove all print it
/// (#78).
pub fn object_display(object_type: &str, object_name: &str) -> String {
    format!("{object_type} {object_name}").trim().to_string()
}

/// The one human-header rule: `<owner>::<routine>`, or the bare routine when
/// the owner is unknown (never `::<routine>`). The digest, fingerprint, prove
/// and `query` headers and the selector candidates all print it (#78).
pub fn owner_qualified(object_display: &str, routine_display: &str) -> String {
    if object_display.is_empty() {
        routine_display.to_string()
    } else {
        format!("{object_display}::{routine_display}")
    }
}

/// Stable routine id -> its owning object, for every workspace-app routine and
/// Interface / ControlAddIn declaration (#78). The owner comes from the model,
/// never the snapshot identity table: a tableextension stub can overwrite the
/// name of the real table with its own number there. (An object's id segment is
/// its key, `engine::ids::object_key`: the number, or `@` + the name for an
/// unnumbered object, #83.) Declaration-only rows read their
/// declaring object directly. Ordinary rows read their declaring object through
/// [`DeclaringObjects`], as `RoutineJoin` does, so the arms of an `#if`-split
/// object (two model objects with ONE internal id and two names) each name their
/// own routines (#241). Rows that share a stable id give one owner, the first
/// row's: for a routine declared with one signature in both arms, that is the
/// first arm.
///
/// The index holds borrowed rows only; a display string is built on lookup, so
/// a run pays for the roots and candidates it asks about.
#[derive(Default)]
pub struct OwnerIndex<'a> {
    /// stable routine id -> (object type, object name).
    by_stable_id: HashMap<&'a str, (&'a str, &'a str)>,
}

impl<'a> OwnerIndex<'a> {
    pub fn build(resolved: &'a Model) -> Self {
        use crate::program::model::role::Role;
        let ws = &resolved.workspace;
        let declaring = DeclaringObjects::new(&ws.objects);
        let mut by_stable_id: HashMap<&'a str, (&'a str, &'a str)> = HashMap::new();
        for r in ws
            .routines
            .iter()
            .filter(|r| is_primary_routine(resolved, r))
        {
            let name = declaring.of(r).map_or("", |o| o.name.as_str());
            by_stable_id
                .entry(r.stable_routine_id.as_str())
                .or_insert((r.object_type.as_str(), name));
        }
        for o in ws
            .objects
            .iter()
            .filter(|o| resolved.object_role(o) == Role::Primary)
        {
            for d in &o.declaration_only_routines {
                by_stable_id
                    .entry(d.stable_routine_id.as_str())
                    .or_insert((o.object_type.as_str(), o.name.as_str()));
            }
        }
        OwnerIndex { by_stable_id }
    }

    /// The owner's `object_display`, or `""` when the id is not a workspace
    /// routine: a fail-closed placeholder, never an invented name.
    pub fn object_display(&self, stable_id: &str) -> String {
        self.by_stable_id
            .get(stable_id)
            .map(|(t, n)| object_display(t, n))
            .unwrap_or_default()
    }

    /// `owner_qualified(owner, routine_display)`: a selector candidate's label.
    pub fn qualified(&self, stable_id: &str, routine_display: &str) -> String {
        owner_qualified(&self.object_display(stable_id), routine_display)
    }
}

/// Where the db operation behind one effect actually LIVES.
///
/// This is the join that makes an `inherited` answer actionable, and 98.8% of
/// real memberships are `inherited`: the effect surfaces on the routine you
/// asked about, but the `Cust.Modify()` itself sits in a callee possibly twelve
/// frames down. `operation_id` is the store's own per-operation identity, and
/// every db effect originates in some routine's `record_operations` entry — so
/// the anchor is a plain lookup, not an inference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationRef {
    pub owner: RoutineRef,
    pub file: String,
    pub line: u32,
    pub column: u32,
}

/// `operation_id` -> the routine + record operation that produced it, read
/// off the shared [`OperationIndex`] (MCP output wave 2, W2-0a).
pub struct OperationJoin<'a> {
    index: OperationIndex<'a>,
}

impl<'a> OperationJoin<'a> {
    pub fn build(resolved: &'a Model) -> Self {
        OperationJoin {
            index: OperationIndex::build(
                resolved,
                crate::engine::l4::operation_index::IndexScope::RecordOperations,
            ),
        }
    }

    /// `None` when the operation lives outside the workspace routine set (a
    /// retained dependency leaf's own effect) — render the id, invent nothing.
    /// Operation ids embed the owning routine's id, so only the rows of a
    /// collided id share one; then the LAST owning row's operation is the
    /// anchor (#160 step 11b §4.6, class P), never an ambiguity: ConTinAI
    /// reads `origin` directly.
    pub fn get(&self, operation_id: &str, routines: &RoutineJoin<'_>) -> Option<OperationRef> {
        let (r, op) = self.index.last_record_operation(operation_id)?;
        Some(OperationRef {
            owner: routines.of(r),
            file: op.source_anchor.source_unit_id.clone(),
            line: op.source_anchor.start_line,
            column: op.source_anchor.start_column,
        })
    }

    /// Q4 (MCP output wave 3a): the operation's receiver is a variable
    /// declared `RecordRef` (`ModelRecordOperation::receiver_is_record_ref`),
    /// read off the same claiming row [`Self::get`] reads. `false` for an
    /// operation outside the model's routines: nothing is invented.
    pub fn receiver_is_record_ref(&self, operation_id: &str) -> bool {
        self.index
            .last_record_operation(operation_id)
            .is_some_and(|(_, op)| op.receiver_is_record_ref)
    }
}

/// The human name of a row's table: a RecordRef's "table decided at run
/// time" (Q4) is said only where the receiver is one; any other unresolved
/// table is "table not resolved".
fn unresolved_label(record_ref: bool) -> &'static str {
    if record_ref {
        "a RecordRef (table decided at run time)"
    } else {
        "table not resolved"
    }
}

/// `o.line` / `rr.line` are the model's 0-based rows; every rendering below
/// writes them 1-based (`lineBase: 1`, [`crate::engine::line_base`]).
fn origin_json(origin: Option<&OperationRef>) -> Value {
    match origin {
        None => Value::Null,
        Some(o) => json!({
            "routine": routine_ref_json(&o.owner),
            "anchor": { "file": o.file, "line": shown(o.line), "column": o.column },
        }),
    }
}

/// `Object::Routine at file:line` — the human rendering of an effect's origin.
fn origin_human(origin: Option<&OperationRef>, operation_id: &str) -> String {
    match origin {
        Some(o) => format!("{} at {}:{}", o.owner.qualified(), o.file, shown(o.line)),
        None => format!("(outside the workspace: {operation_id})"),
    }
}

fn routine_ref_json(rr: &RoutineRef) -> Value {
    json!({
        "display": rr.display,
        "objectDisplay": rr.object_display,
        "stableId": rr.stable_id,
        "anchor": { "file": rr.file, "line": shown(rr.line), "column": rr.column },
    })
}

/// [`temp_state_str`]'s text for a state that depends on the routine's own
/// implicit `Rec` (the receiver index).
pub const RECEIVER_TEMP_STATE: &str = "parameter-dependent(rec)";

/// The one renderer of the temp-state vocabulary (`query effects`, and
/// `digest`/`prove`'s `detail.tempState`).
///
/// The reserved receiver index ([`RECEIVER_PARAM_INDEX`], `u32::MAX`, "the
/// implicit `Rec` a table procedure or trigger runs on") renders as
/// `parameter-dependent(rec)`: the raw `4294967295` is an internal sentinel,
/// not a parameter position a person can find in the signature (G49).
pub fn temp_state_str(t: &TempStateKind) -> String {
    match t {
        TempStateKind::Known(true) => "known(true)".to_string(),
        TempStateKind::Known(false) => "known(false)".to_string(),
        TempStateKind::ParameterDependent(i) if *i == RECEIVER_PARAM_INDEX => {
            RECEIVER_TEMP_STATE.to_string()
        }
        TempStateKind::ParameterDependent(i) => format!("parameter-dependent({i})"),
        TempStateKind::Unknown => "unknown".to_string(),
    }
}

/// `<appGuid>/table/50101` -> `MC Customer`. Same reason as [`RoutineJoin`]: an
/// internal id is not a thing a person reads. [`UNKNOWN_TABLE_ID`] is
/// deliberately absent from this map — it has no name because it is not a
/// table, and callers must label it as the bucket it is.
struct TableNames<'a>(HashMap<&'a str, &'a str>);

impl<'a> TableNames<'a> {
    fn build(resolved: &'a Model) -> Self {
        TableNames(
            table_by_id_preferring_real(&resolved.workspace.tables)
                .into_iter()
                .map(|(id, t)| (id, t.name.as_str()))
                .collect(),
        )
    }

    fn get(&self, table_id: &str) -> Option<&'a str> {
        self.0.get(table_id).copied()
    }
}

fn touch_json(
    t: &TableTouch<'_>,
    origin: Option<&OperationRef>,
    table_name: Option<Option<&str>>,
) -> Value {
    let mut m = Map::new();
    m.insert("op".into(), t.op.into());
    m.insert("operationId".into(), t.operation_id.into());
    m.insert("origin".into(), origin_json(origin));
    m.insert("tempState".into(), temp_state_str(t.temp_state).into());
    m.insert("via".into(), t.via.as_str().into());
    if let Some(name) = table_name {
        m.insert("tableId".into(), t.table_id.into());
        m.insert(
            "tableName".into(),
            name.map(Value::from).unwrap_or(Value::Null),
        );
        m.insert(
            "isUnknownBucket".into(),
            is_unknown_table(t.table_id).into(),
        );
    }
    Value::Object(m)
}

/// `query touches`'s summary for a file answer (MCP output plan 1e): the
/// payload's headlines (the table, `down`'s and `up`'s verdicts and counts)
/// with every list replaced by its length.
fn touches_summary(payload: &Map<String, Value>) -> Value {
    fn counts_only(v: &Value) -> Value {
        match v {
            Value::Object(m) => {
                let mut out = Map::new();
                for (k, x) in m {
                    match x {
                        Value::Array(a) if a.iter().any(Value::is_object) => {
                            out.insert(format!("{k}Count"), a.len().into());
                        }
                        Value::Object(_) => {
                            out.insert(k.clone(), counts_only(x));
                        }
                        _ => {
                            out.insert(k.clone(), x.clone());
                        }
                    }
                }
                Value::Object(out)
            }
            other => other.clone(),
        }
    }
    let mut s = counts_only(&Value::Object(payload.clone()));
    if let Some(m) = s.as_object_mut() {
        m.insert("kind".into(), "query.touches".into());
    }
    crate::engine::batch::output::capped_summary(s, "")
}

/// `query effects`'s summary for a file answer (MCP output plan 1e): the
/// effects counted by operation, by temp state and by table, and the
/// physical (not known-temporary) writes.
fn effects_summary(routine: &str, effects: &[TableTouch<'_>], tables: &TableNames<'_>) -> Value {
    use std::collections::BTreeMap;
    let mut by_op: BTreeMap<&str, u64> = BTreeMap::new();
    let mut by_temp: BTreeMap<String, u64> = BTreeMap::new();
    let mut table_set = std::collections::BTreeSet::new();
    let (mut unknown, mut physical_writes) = (0u64, 0u64);
    for e in effects {
        *by_op.entry(e.op).or_default() += 1;
        *by_temp.entry(temp_state_str(e.temp_state)).or_default() += 1;
        if is_unknown_table(e.table_id) {
            unknown += 1;
        } else {
            table_set.insert(tables.get(e.table_id).unwrap_or(e.table_id));
        }
        let write = matches!(
            e.op,
            "Insert" | "Modify" | "ModifyAll" | "Delete" | "DeleteAll" | "Rename"
        );
        if write && !matches!(e.temp_state, TempStateKind::Known(true)) {
            physical_writes += 1;
        }
    }
    crate::engine::batch::output::capped_summary(
        json!({
            "kind": "query.effects",
            "routine": routine,
            "effectCount": effects.len(),
            "byOp": by_op,
            "byTempState": by_temp,
            "notKnownTemporaryWrites": physical_writes,
            "tableCount": table_set.len(),
            "unknownTable": unknown,
        }),
        "",
    )
}

/// The document envelope. Every payload states how dependencies were loaded
/// (`payload.dependencies`, #66), the selector-failure documents included.
fn envelope(
    kind: &str,
    mut payload: Value,
    deps: &LoadedDependencies,
    alsem_ver: &str,
    deterministic: bool,
) -> String {
    if let Value::Object(m) = &mut payload {
        m.insert("dependencies".into(), deps.to_json());
        m.insert("lineBase".into(), LINE_BASE.into());
    }
    let mut env = Map::new();
    env.insert("alsemVersion".into(), alsem_ver.into());
    env.insert("deterministic".into(), deterministic.into());
    env.insert(
        "generatedAt".into(),
        pinned_or_now_iso8601(deterministic).into(),
    );
    env.insert("kind".into(), kind.into());
    env.insert("payload".into(), payload);
    env.insert("schemaVersion".into(), QUERY_SCHEMA_VERSION.into());
    serialize_document_value(Value::Object(env))
}

/// Render an unresolved/ambiguous selector as its own (exit-2) document, so a
/// bad selector produces a well-formed answer instead of a silent empty result.
fn selector_failure_doc<T>(
    kind: &str,
    selector_field: &str,
    selector: &str,
    resolution: &Resolution<T>,
    deps: &LoadedDependencies,
    alsem_ver: &str,
    deterministic: bool,
) -> QueryRunResult {
    let candidates: Vec<String> = match resolution {
        Resolution::Ambiguous(c) => c.clone(),
        _ => Vec::new(),
    };
    let payload = json!({
        selector_field: {
            "selector": selector,
            "resolution": resolution.label(),
            "candidates": candidates,
        }
    });
    let human = if candidates.is_empty() {
        format!("{kind}: {selector_field} {selector:?} matched no {selector_field}\n")
    } else {
        let mut s = format!("{kind}: {selector_field} {selector:?} is AMBIGUOUS:\n");
        for c in &candidates {
            s.push_str(&format!("  - {c}\n"));
        }
        s
    };
    QueryRunResult {
        json_text: envelope(kind, payload, deps, alsem_ver, deterministic),
        human_text: human,
        exit_code: 2,
        summary: Value::Null,
        ancestor_witnesses_built: 0,
    }
}

// ---------------------------------------------------------------------------
// `alsem query touches`
// ---------------------------------------------------------------------------

/// Run `alsem query touches <ws> --table T [--from R] [--direction ...]`.
///
/// Three shapes, per the facade's own decomposition:
///
/// - `--from` + `--direction down` — does R's transitive cone touch T, with
///   witnesses (`op`/`operationId`/`tempState`/`via`).
/// - `--from` + `--direction up` — the SCOPED ancestor query: which transitive
///   callers of R touch T, nearest first. Informative precisely when R itself
///   does not touch T.
/// - no `--from` — the workspace-global list, count first, uncapped.
#[allow(clippy::too_many_arguments)]
pub fn run_query_touches_pipeline(
    inputs: &std::sync::Arc<crate::program::inputs::WorkspaceInputs>,
    table_selector: &str,
    from: Option<&str>,
    direction: Direction,
    all: bool,
    alsem_ver: &str,
    deterministic: bool,
    single_app: bool,
) -> Result<QueryRunResult, String> {
    QueryJob::touches(
        inputs,
        table_selector,
        from,
        direction,
        all,
        alsem_ver,
        deterministic,
        single_app,
    )
    .run()
}

/// The pure rendering half of [`run_query_touches_pipeline`] — split out so the
/// goldens and the differential drive the EXACT code the CLI runs without
/// re-assembling the workspace per case.
///
/// `from: None` means the WORKSPACE-GLOBAL query, which has no down direction:
/// "down" is about one routine's cone. That combination is rejected here rather
/// than only in `alsem.rs`'s clap arm, so a library caller cannot get a global
/// list back labelled `"direction": "down"`. ⟨final-branch-review-l3.md M-1⟩ The
/// CLI does NOT pre-check it — `src/bin/alsem.rs`'s `run_query_touches_cmd`
/// deliberately defers to this function alone ("rejected by
/// `render_query_touches` itself, not duplicated here: one rule in one place"),
/// so `alsem query touches <ws> --table T --direction down` with no `--from`
/// pays a full workspace assembly + L3 resolve + L4 summary solve before
/// returning this exit-2 error. That is the TESTED behaviour (the
/// `d1-multi-caller.down-without-from` golden depends on it), so a pre-check
/// would be a perf fix here, not a doc fix.
pub fn render_query_touches(
    substrate: &QuerySubstrate,
    table_selector: &str,
    from: Option<&str>,
    direction: Direction,
    all: bool,
    alsem_ver: &str,
    deterministic: bool,
) -> QueryRunResult {
    with_dependencies_line(
        render_query_touches_body(
            substrate,
            table_selector,
            from,
            direction,
            all,
            alsem_ver,
            deterministic,
            None,
        ),
        &substrate.dependencies,
    )
}

/// Human output names the loaded dependency apps on its first line, only when
/// there are some (a workspace with no dependency reads the same in both
/// modes). An answer with a `scope:` line states them there instead (W2-2,
/// design §4.5 Q6: `dependencies_clause`), so it gets no first line.
fn with_dependencies_line(r: QueryRunResult, deps: &LoadedDependencies) -> QueryRunResult {
    finish_human(r, deps, None)
}

/// [`with_dependencies_line`], then (W2-7, 2g) `--show-source` excerpts
/// while the locations still name their app by guid, then readable
/// dependency locations (the JSON keeps the guid).
fn finish_human(
    mut r: QueryRunResult,
    deps: &LoadedDependencies,
    sources: Option<&crate::engine::source_excerpt::SourceTexts>,
) -> QueryRunResult {
    if !r.human_text.contains("\n  scope: ") {
        r.human_text = deps.prefix_human(r.human_text);
    }
    if let Some(s) = sources {
        r.human_text = crate::engine::source_excerpt::with_excerpts(&r.human_text, s);
    }
    r.human_text = deps.readable_locations(r.human_text);
    r
}

/// The scope line's dependencies clause (§4.5 Q6): a count, every app with
/// `--all`; nothing when none was loaded.
fn dependencies_clause(deps: &LoadedDependencies, all: bool) -> String {
    match (deps.apps.len(), all) {
        (0, _) => String::new(),
        (n, false) => format!(
            "; dependencies: {} (--all lists them)",
            plural(n, "app", "apps")
        ),
        (_, true) => format!("; {}", deps.human_line().unwrap_or_default()),
    }
}

#[allow(clippy::too_many_arguments)]
fn render_query_touches_body(
    substrate: &QuerySubstrate,
    table_selector: &str,
    from: Option<&str>,
    direction: Direction,
    all: bool,
    alsem_ver: &str,
    deterministic: bool,
    cone: Option<&crate::engine::l5::digest_cli::DigestEntryFull>,
) -> QueryRunResult {
    let deps = &substrate.dependencies;
    if from.is_none() && direction == Direction::Down {
        let payload = json!({
            "direction": direction.as_str(),
            "from": Value::Null,
            "error": "a `down` query is about one routine's cone — pass --from <routine>, \
                      or omit --direction for the workspace-global list",
        });
        return QueryRunResult {
            json_text: envelope("query.touches", payload, deps, alsem_ver, deterministic),
            human_text: "query.touches: --direction down requires --from <routine>\n".to_string(),
            exit_code: 2,
            summary: Value::Null,
            ancestor_witnesses_built: 0,
        };
    }
    let table = match resolve_table_selector(substrate.resolved(), table_selector) {
        Resolution::Resolved(t) => t,
        other => {
            return selector_failure_doc(
                "query.touches",
                "table",
                table_selector,
                &other,
                deps,
                alsem_ver,
                deterministic,
            );
        }
    };

    let join = RoutineJoin::build(substrate.resolved());
    let from_routine = match from {
        None => None,
        Some(sel) => match resolve_routine_selector(substrate.resolved(), &join.declaring, sel) {
            Resolution::Resolved(r) => Some(r),
            other => {
                return selector_failure_doc(
                    "query.touches",
                    "from",
                    sel,
                    &other,
                    deps,
                    alsem_ver,
                    deterministic,
                );
            }
        },
    };

    let query = substrate.query();
    let ops = OperationJoin::build(substrate.resolved());

    let mut payload = Map::new();
    payload.insert(
        "table".into(),
        json!({
            "selector": table_selector,
            "id": table.id,
            "name": table.name,
            "isUnknownBucket": is_unknown_table(&table.id),
            "resolution": "resolved",
        }),
    );
    payload.insert("direction".into(), direction.as_str().into());

    let mut human = String::new();
    let mut ancestor_witnesses_built = 0;
    let table_label = match &table.name {
        Some(n) => format!("{n} ({})", table.id),
        None => format!(
            "{} — the UNRESOLVED-TABLE bucket, not a table",
            UNKNOWN_TABLE_ID
        ),
    };

    match from_routine {
        None => {
            // U-global. Count first, then the complete list (no cap).
            payload.insert("from".into(), Value::Null);
            // Workspace routines only (#66): a cross-app model's dependency
            // routines are not selectable, so listing them would flood the answer
            // with rows nobody can scope.
            let routines: Vec<_> = query
                .routines_touching(&table.id)
                .into_iter()
                .filter(|&ix| {
                    join.routine(query.bundle().routine_id(ix))
                        .is_none_or(|r| is_primary_routine(substrate.resolved(), r))
                })
                .collect();
            let rows: Vec<Value> = routines
                .iter()
                .map(|&ix| {
                    let internal = query.bundle().routine_id(ix);
                    match join.get(internal) {
                        Some(rr) => routine_ref_json(&rr),
                        None => json!({ "display": internal, "objectDisplay": "", "stableId": Value::Null, "anchor": Value::Null }),
                    }
                })
                .collect();
            payload.insert(
                "up".into(),
                json!({
                    "scoped": false,
                    "routineCount": rows.len(),
                    "routines": rows,
                    "note": "workspace-global: every routine whose transitive cone touches this \
                             table. Scope it with --from <routine> --direction up.",
                }),
            );
            human.push_str(&format!("--- touches {table_label} ---\n"));
            human.push_str(&format!(
                "  workspace-global: {} routine(s) touch it\n",
                routines.len()
            ));
            let shown_n = fold_len(routines.len(), GLOBAL_LIST_CAP, all);
            for &ix in &routines[..shown_n] {
                let internal = query.bundle().routine_id(ix);
                match join.get(internal) {
                    Some(rr) => human.push_str(&format!(
                        "    {} — {}:{}\n",
                        rr.qualified(),
                        rr.file,
                        shown(rr.line)
                    )),
                    None => human.push_str(&format!("    {internal}\n")),
                }
            }
            if shown_n < routines.len() {
                human.push_str(&format!(
                    "    … +{} — --all lists them\n",
                    plural(routines.len() - shown_n, "routine", "routines")
                ));
            }
        }
        Some(r) => {
            let rr = join.of(r);
            payload.insert(
                "from".into(),
                json!({
                    "selector": from.unwrap_or(""),
                    "resolution": "resolved",
                    "routine": routine_ref_json(&rr),
                }),
            );
            let Some(rix) = query.routine_ix(&r.id) else {
                // Resolvable as a workspace routine but never interned by the
                // solve — state that rather than reporting a false "no".
                payload.insert(
                    "down".into(),
                    json!({ "answerable": false, "reason": "routine has no db-effect row" }),
                );
                human.push_str(&format!(
                    "--- {} vs {table_label} ---\n  not answerable: routine has no db-effect row\n",
                    rr.qualified()
                ));
                let summary = touches_summary(&payload);
                return QueryRunResult {
                    json_text: envelope(
                        "query.touches",
                        Value::Object(payload),
                        deps,
                        alsem_ver,
                        deterministic,
                    ),
                    human_text: human,
                    exit_code: 0,
                    summary,
                    ancestor_witnesses_built: 0,
                };
            };

            human.push_str(&format!("--- {} vs {table_label} ---\n", rr.qualified()));
            let beyond = cone.map(|e| Beyond::new(e, substrate.resolved(), deps));
            human.push_str(&format!(
                "{}{}\n",
                if beyond.is_some() {
                    TOUCHES_SCOPE_LINE_CONE
                } else {
                    TOUCHES_SCOPE_LINE
                }
                .trim_end(),
                dependencies_clause(deps, all)
            ));

            let cx = TouchCx {
                query: &query,
                join: &join,
                ops: &ops,
                hops: FirstHops::new(&query),
            };
            let touches_down = query.touches_table(rix, &table.id);
            if direction.wants_down() {
                render_touches_down(
                    &cx,
                    rix,
                    &table.id,
                    all,
                    beyond.as_ref(),
                    &mut payload,
                    &mut human,
                );
            }
            if direction.wants_up() {
                render_touches_up(
                    &cx,
                    rix,
                    &table.id,
                    touches_down,
                    all,
                    &mut payload,
                    &mut human,
                );
            }
            ancestor_witnesses_built = query.ancestor_witnesses_built();
        }
    }

    let summary = touches_summary(&payload);
    QueryRunResult {
        json_text: envelope(
            "query.touches",
            Value::Object(payload),
            deps,
            alsem_ver,
            deterministic,
        ),
        human_text: human,
        exit_code: 0,
        summary,
        ancestor_witnesses_built,
    }
}

/// The scope line every routine-scoped touches answer prints after its title.
const TOUCHES_SCOPE_LINE: &str = "  scope: per operation, one routine into dependency code; \
     temporary-record touches persist nothing but use the table's schema and run its triggers; \
     lines are 1-based\n";

/// [`TOUCHES_SCOPE_LINE`] when the capability cone was read (wave 3a): the
/// writes beyond each dependency entry point follow the whole cone.
const TOUCHES_SCOPE_LINE_CONE: &str = "  scope: dependency code: full cone for physical writes, \
     grouped per entry point; per-operation detail one routine deep; temporary-record touches \
     persist nothing but use the table's schema and run its triggers; lines are 1-based\n";

/// The default human list cap for touches (spec §3.8 rule 4's shape: the cap,
/// or every item when there are at most a quarter more). `--all` lists all.
const TOUCHES_LIST_CAP: usize = 8;
/// The workspace-global list's cap (tables' 12, all when 15 or fewer).
const GLOBAL_LIST_CAP: usize = 12;
/// How many owner routines the temporary-records line names.
const TEMP_OWNERS_SHOWN: usize = 4;

/// The shared fold rule (design §3.8 rule 4): one helper for every output.
use crate::engine::l5::routine_rollup::fold_len;

/// The record-operation family of an op kind (spec §4.5's buckets): lock,
/// write, validate (it runs the field's trigger and may write: never a read),
/// read, and unclassified for any other kind.
fn op_family(op: &str) -> &'static str {
    match op {
        "LockTable" => "lock",
        "Insert" | "Modify" | "ModifyAll" | "Delete" | "DeleteAll" | "Rename" => "write",
        "Validate" => "validate",
        "Get" | "Next" | "IsEmpty" | "CalcFields" | "CalcSums" => "read",
        o if o.starts_with("Find") || o.starts_with("Count") => "read",
        _ => "unclassified",
    }
}

/// The human temp-state note of a touch: nothing for a persistent record.
fn temp_note(t: &TempStateKind) -> String {
    match t {
        TempStateKind::Known(false) => String::new(),
        TempStateKind::Known(true) => " (temporary record)".to_string(),
        TempStateKind::Unknown => " (temp state unknown)".to_string(),
        TempStateKind::ParameterDependent(i) if *i == RECEIVER_PARAM_INDEX => {
            " (caller-dependent: Rec)".to_string()
        }
        TempStateKind::ParameterDependent(i) => {
            format!(" (caller-dependent: parameter {})", i + 1)
        }
    }
}

/// The human `via` note (spec §4.5): `direct` and `inherited` print nothing
/// here (the first hop of an inherited touch is W2-2's `firstHop`).
fn via_note(v: crate::engine::l4::effect_store::ViaRank) -> &'static str {
    use crate::engine::l4::effect_store::ViaRank;
    match v {
        ViaRank::Direct | ViaRank::Inherited => "",
        ViaRank::Dynamic => " (dynamic dispatch)",
        ViaRank::EventSubscriber => " (event subscriber)",
        ViaRank::ImplicitTrigger => " (table trigger)",
    }
}

/// What the touches renderers share: the query and the two joins.
struct TouchCx<'q, 'a> {
    query: &'q DbEffectQuery<'a>,
    join: &'q RoutineJoin<'a>,
    ops: &'q OperationJoin<'a>,
    /// W2-2: `through <first hop>` by the shared §4.5 rule.
    hops: FirstHops<'q, 'a>,
}

impl<'a> TouchCx<'_, 'a> {
    /// `[Op] <family> at <file>:<line> in <owner><temp><via>`, then `, through
    /// <first hop>` on a row that is not the routine's own, when a direct
    /// callee holds its complete contribution (design §4.5).
    fn witness_line(&self, t: &TableTouch<'a>) -> String {
        let at = match self.ops.get(t.operation_id, self.join) {
            Some(o) => format!("at {}:{} in {}", o.file, shown(o.line), o.owner.qualified()),
            None => format!("(outside the workspace: {})", t.operation_id),
        };
        let through = match t.via {
            crate::engine::l4::effect_store::ViaRank::Direct => String::new(),
            _ => match self.hops.of(t) {
                Ok(c) => format!(", through {}", self.routine_label(c)),
                Err(_) => String::new(),
            },
        };
        format!(
            "[{}] {} {at}{}{}{through}",
            t.op,
            op_family(t.op),
            temp_note(t.temp_state),
            via_note(t.via)
        )
    }

    /// The operation's owner, qualified, for the temporary-records line.
    fn owner_label(&self, t: &TableTouch<'_>) -> String {
        match self.ops.get(t.operation_id, self.join) {
            Some(o) => o.owner.qualified(),
            None => "(outside the workspace)".to_string(),
        }
    }

    /// A witness row: [`touch_json`] plus its [`TouchClass`].
    fn witness_json(&self, t: &TableTouch<'_>) -> Value {
        let mut v = touch_json(t, self.ops.get(t.operation_id, self.join).as_ref(), None);
        if let Value::Object(m) = &mut v {
            m.insert(
                "touchClass".into(),
                TouchClass::of(t.temp_state).as_str().into(),
            );
        }
        v
    }

    fn routine_json(&self, ix: crate::engine::l4::routine_interner::SolveIx) -> Value {
        let internal = self.query.bundle().routine_id(ix);
        match self.join.get(internal) {
            Some(x) => routine_ref_json(&x),
            None => {
                json!({ "display": internal, "objectDisplay": "", "stableId": Value::Null, "anchor": Value::Null })
            }
        }
    }

    fn routine_label(&self, ix: crate::engine::l4::routine_interner::SolveIx) -> String {
        let internal = self.query.bundle().routine_id(ix);
        match self.join.get(internal) {
            Some(x) => x.qualified(),
            None => internal.to_string(),
        }
    }
}

fn counts_json(c: &TouchCounts) -> Value {
    json!({
        "knownPersistent": c.known_persistent,
        "possiblyPersistent": {
            "tempStateUnknown": c.temp_state_unknown,
            "callerDependent": c.caller_dependent,
        },
        "temporary": c.temporary,
    })
}

/// `a known persistent, b possibly persistent (...), c on temporary records`.
fn counts_human(c: &TouchCounts) -> String {
    let mut parts = vec![format!("{} known persistent", c.known_persistent)];
    let pp = match (c.temp_state_unknown, c.caller_dependent) {
        (0, 0) => String::new(),
        (_, 0) => " (temp state unknown)".to_string(),
        (0, _) => " (caller-dependent)".to_string(),
        (u, d) => format!(" (temp state unknown {u}, caller-dependent {d})"),
    };
    parts.push(format!(
        "{} possibly persistent{pp}",
        c.possibly_persistent()
    ));
    parts.push(format!("{} on temporary records", c.temporary));
    parts.join(", ")
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// `down`: every witness in JSON (each with its class), the class counts and
/// `beyond` (wave 3a: each dependency entry point beyond which the cone may
/// persist a write to the table, empty when the cone was not read); in human
/// the persistent and possibly persistent witnesses (folded), one line for
/// the temporary-record ones and one for the entry points.
fn render_touches_down(
    cx: &TouchCx<'_, '_>,
    rix: crate::engine::l4::routine_interner::SolveIx,
    table_id: &str,
    all: bool,
    cone: Option<&Beyond<'_>>,
    payload: &mut Map<String, Value>,
    human: &mut String,
) {
    let beyond_writes = cone.map(Beyond::writes).unwrap_or_default();
    let beyond = beyond_of_table(&beyond_writes, table_id);
    let query = cx.query;
    let touches_down = query.touches_table(rix, table_id);
    let witnesses = query.touches(rix, table_id);
    let mut counts = TouchCounts::default();
    let mut by_op: std::collections::BTreeMap<&str, usize> = std::collections::BTreeMap::new();
    for w in &witnesses {
        counts.add(w.temp_state);
        *by_op.entry(w.op).or_default() += 1;
    }
    // Operations on a table not resolved "may be this table". They are outside
    // the four class counts, unless the query table IS that bucket (then they
    // are the witnesses themselves).
    let (not_resolved, not_resolved_ref) = if is_unknown_table(table_id) {
        let refs = witnesses
            .iter()
            .filter(|w| cx.ops.receiver_is_record_ref(w.operation_id))
            .count();
        (witnesses.len(), refs)
    } else {
        query
            .bundle()
            .db_effects(rix)
            .filter(|e| is_unknown_table(e.table_id))
            .fold((0, 0), |(n, r), e| {
                (
                    n + 1,
                    r + usize::from(cx.ops.receiver_is_record_ref(e.operation_id)),
                )
            })
    };
    let mut counts_v = counts_json(&counts);
    if let Value::Object(m) = &mut counts_v {
        m.insert("tableNotResolved".into(), not_resolved.into());
        // Q4 (3.1.0): of them, on a RecordRef receiver.
        m.insert("tableNotResolvedRecordRef".into(), not_resolved_ref.into());
        m.insert("byOp".into(), json!(by_op));
    }
    payload.insert(
        "down".into(),
        json!({
            "answerable": true,
            "touches": touches_down,
            "witnessCount": witnesses.len(),
            "counts": counts_v,
            "beyond": beyond,
            "witnesses": witnesses.iter().map(|t| cx.witness_json(t)).collect::<Vec<_>>(),
        }),
    );
    let beyond_n = beyond.as_array().map_or(0, Vec::len);

    if !touches_down && beyond_n > 0 {
        human.push_str("  down: no operation one routine into dependency code touches the table\n");
    } else if !touches_down {
        human.push_str("  down: no — this routine's cone does not touch the table\n");
    } else {
        human.push_str(&format!(
            "  down: yes — {}: {}\n",
            plural(witnesses.len(), "record touch", "record touches"),
            counts_human(&counts)
        ));
        let listed: Vec<&TableTouch<'_>> = if all {
            witnesses.iter().collect()
        } else {
            witnesses
                .iter()
                .filter(|w| TouchClass::of(w.temp_state) != TouchClass::Temporary)
                .collect()
        };
        let shown_n = fold_len(listed.len(), TOUCHES_LIST_CAP, all);
        for w in &listed[..shown_n] {
            human.push_str(&format!("    {}\n", cx.witness_line(w)));
        }
        if shown_n < listed.len() {
            human.push_str(&format!(
                "    … +{} (persistent or possibly persistent) — --all lists them\n",
                plural(listed.len() - shown_n, "touch", "touches")
            ));
        }
        if counts.temporary > 0 {
            let mut owners: std::collections::BTreeMap<String, usize> =
                std::collections::BTreeMap::new();
            for w in witnesses
                .iter()
                .filter(|w| TouchClass::of(w.temp_state) == TouchClass::Temporary)
            {
                *owners.entry(cx.owner_label(w)).or_default() += 1;
            }
            let mut ranked: Vec<(String, usize)> = owners.into_iter().collect();
            ranked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
            let shown_owners = fold_len(ranked.len(), TEMP_OWNERS_SHOWN, all);
            let mut names: Vec<String> = ranked[..shown_owners]
                .iter()
                .map(|(n, c)| format!("{n} {c}"))
                .collect();
            if shown_owners < ranked.len() {
                names.push(format!(
                    "… +{}",
                    plural(ranked.len() - shown_owners, "routine", "routines")
                ));
            }
            human.push_str(&format!(
                "    temporary records: {} in {} ({}){}\n",
                plural(counts.temporary, "touch", "touches"),
                plural(ranked.len(), "routine", "routines"),
                names.join(", "),
                if all { "" } else { " — --all lists them" }
            ));
        }
    }
    if not_resolved > 0 && !is_unknown_table(table_id) {
        let refs = if not_resolved_ref > 0 {
            format!(", {not_resolved_ref} of them on a RecordRef (table decided at run time)")
        } else {
            String::new()
        };
        human.push_str(&format!(
            "  table not resolved: {} in this cone{refs} (they may be this table) — --table unknown lists them\n",
            plural(not_resolved, "operation", "operations")
        ));
    }
    if beyond_n > 0 {
        let points: Vec<String> = beyond_writes
            .iter()
            .filter(|w| w.tables.contains_key(table_id))
            .map(|w| {
                let kinds: Vec<&str> = w
                    .types_of
                    .get(table_id)
                    .into_iter()
                    .flatten()
                    .map(|t| crate::engine::l5::routine_rollup::type_kind(t))
                    .collect();
                format!("{} ({})", w.name, kinds.join(", "))
            })
            .collect();
        let shown_n = fold_len(points.len(), TOUCHES_LIST_CAP, all);
        let mut text = points[..shown_n].join("; ");
        if shown_n < points.len() {
            text.push_str(&format!(
                "; … +{} — --all lists them",
                plural(points.len() - shown_n, "entry point", "entry points")
            ));
        }
        human.push_str(&format!(
            "  beyond dependency entry points (the whole cone, literal arguments followed): may \
             write it beyond {}: {text} — alsem prove <routine> writes-table:<table> gives the path\n",
            plural(beyond_n, "entry point", "entry points")
        ));
    }
}

/// `up`: every transitive caller probed ("has a summary row?", then
/// "touches?"). When the routine touches the table itself (the trivial case)
/// no caller witness is built: one counted line. Otherwise each touching
/// caller's witnesses are streamed into its first witness and counts. `--all`
/// adds every caller witness (the full product, as before W2-3).
fn render_touches_up(
    cx: &TouchCx<'_, '_>,
    rix: crate::engine::l4::routine_interner::SolveIx,
    table_id: &str,
    touches_down: bool,
    all: bool,
    payload: &mut Map<String, Value>,
    human: &mut String,
) {
    let query = cx.query;
    let callers: Vec<CallerTouch<'_>> = if touches_down {
        query.callers_membership(rix, table_id)
    } else {
        query.callers_aggregated(rix, table_id)
    };
    let of = |o: CallerOutcome| callers.iter().filter(move |c| c.outcome == o);
    let touching = of(CallerOutcome::Touches).count();
    let not_touching: Vec<&CallerTouch<'_>> = of(CallerOutcome::DoesNotTouch).collect();
    let unknown: Vec<&CallerTouch<'_>> = of(CallerOutcome::NoRow).collect();
    let note = if touches_down {
        "this routine itself touches the table, so its callers touch it through this routine \
         (summaries are transitive-down) — not a finding; each caller is still probed, and one \
         without a summary row is unknown"
    } else {
        "this routine does NOT touch the table; the touching callers reach it through other \
         branches"
    };
    let mut up = Map::new();
    up.insert("scoped".into(), true.into());
    up.insert("transitiveCallers".into(), callers.len().into());
    up.insert("callersTouching".into(), touching.into());
    up.insert("callersNotTouching".into(), not_touching.len().into());
    up.insert("callersUnknown".into(), unknown.len().into());
    up.insert("informative".into(), (!touches_down).into());
    up.insert("note".into(), note.into());
    up.insert(
        "notTouching".into(),
        not_touching
            .iter()
            .map(|c| json!({"depth": c.depth, "routine": cx.routine_json(c.routine)}))
            .collect::<Vec<_>>()
            .into(),
    );
    up.insert(
        "unknown".into(),
        unknown
            .iter()
            .map(|c| {
                json!({
                    "depth": c.depth,
                    "routine": cx.routine_json(c.routine),
                    "reason": "no summary row",
                })
            })
            .collect::<Vec<_>>()
            .into(),
    );
    if !touches_down {
        up.insert(
            "callers".into(),
            callers
                .iter()
                .filter_map(|c| {
                    let w = c.witnesses.as_ref()?;
                    Some(json!({
                        "depth": c.depth,
                        "routine": cx.routine_json(c.routine),
                        "witnessCount": w.count,
                        "counts": counts_json(&w.counts),
                        "firstWitness": cx.witness_json(&w.first),
                    }))
                })
                .collect::<Vec<_>>()
                .into(),
        );
    }
    // `--all` only: the full caller-witness product.
    let product: Vec<AncestorTouch<'_>> = if all {
        query.ancestors_touching(rix, table_id)
    } else {
        Vec::new()
    };
    if all {
        up.insert(
            "witnesses".into(),
            product
                .iter()
                .map(|a| {
                    let mut m = Map::new();
                    m.insert("depth".into(), a.depth.into());
                    m.insert("routine".into(), cx.routine_json(a.touch.routine));
                    if let Value::Object(t) = cx.witness_json(&a.touch) {
                        m.extend(t);
                    }
                    Value::Object(m)
                })
                .collect::<Vec<_>>()
                .into(),
        );
    }
    payload.insert("up".into(), Value::Object(up));

    let names = |list: &[&CallerTouch<'_>]| -> String {
        let n = fold_len(list.len(), 3, all);
        let mut v: Vec<String> = list[..n]
            .iter()
            .map(|c| cx.routine_label(c.routine))
            .collect();
        if n < list.len() {
            v.push(format!("… +{}", list.len() - n));
        }
        v.join(", ")
    };
    let mut anomalies = String::new();
    if !not_touching.is_empty() {
        anomalies.push_str(&format!("; not touching: {}", names(&not_touching)));
    }
    if !unknown.is_empty() {
        anomalies.push_str(&format!("; unknown (no summary row): {}", names(&unknown)));
    }
    if callers.is_empty() {
        human.push_str("  up: no transitive callers\n");
        return;
    }
    if touches_down {
        let who = if callers.len() == 1 {
            format!(" ({})", cx.routine_label(callers[0].routine))
        } else {
            String::new()
        };
        human.push_str(&format!(
            "  up: {}{who}: {} it, {}, {} unknown — through this routine; not a finding{anomalies}{}\n",
            plural(callers.len(), "transitive caller", "transitive callers"),
            plural(touching, "touches", "touch"),
            plural(not_touching.len(), "does not", "do not"),
            unknown.len(),
            if all { "" } else { " — --all lists their witnesses" },
        ));
    } else {
        human.push_str(&format!(
            "  up: {touching} of {} {} it through other branches, {}, {} unknown{anomalies}\n",
            plural(callers.len(), "transitive caller", "transitive callers"),
            if touching == 1 { "touches" } else { "touch" },
            plural(not_touching.len(), "does not", "do not"),
            unknown.len(),
        ));
        let touchers: Vec<&CallerTouch<'_>> =
            callers.iter().filter(|c| c.witnesses.is_some()).collect();
        let shown_n = fold_len(touchers.len(), TOUCHES_LIST_CAP, all);
        for c in &touchers[..shown_n] {
            let w = c.witnesses.as_ref().expect("filtered");
            if all {
                human.push_str(&format!(
                    "    +{} {}: {}: {}\n",
                    c.depth,
                    cx.routine_label(c.routine),
                    plural(w.count, "touch", "touches"),
                    counts_human(&w.counts)
                ));
                for a in product.iter().filter(|a| a.touch.routine == c.routine) {
                    human.push_str(&format!("      {}\n", cx.witness_line(&a.touch)));
                }
            } else {
                human.push_str(&format!(
                    "    +{} {}: {} ({}); first {}\n",
                    c.depth,
                    cx.routine_label(c.routine),
                    plural(w.count, "touch", "touches"),
                    counts_human(&w.counts),
                    cx.witness_line(&w.first)
                ));
            }
        }
        if shown_n < touchers.len() {
            human.push_str(&format!(
                "    … +{} — --all lists them\n",
                plural(touchers.len() - shown_n, "caller", "callers")
            ));
        }
    }
    if all && touches_down {
        for a in &product {
            human.push_str(&format!(
                "    +{} {} {}\n",
                a.depth,
                cx.routine_label(a.touch.routine),
                cx.witness_line(&a.touch)
            ));
        }
    }
}

// ---------------------------------------------------------------------------
// `alsem query effects`
// ---------------------------------------------------------------------------

/// Run `alsem query effects <ws> --routine R` — the complete transitive
/// down-list for one routine, WITH `via` (which the posting lists drop and
/// `ConeDerivedStore` never carried).
pub fn run_query_effects_pipeline(
    inputs: &std::sync::Arc<crate::program::inputs::WorkspaceInputs>,
    routine_selector: &str,
    alsem_ver: &str,
    deterministic: bool,
    single_app: bool,
) -> Result<QueryRunResult, String> {
    QueryJob::effects(
        inputs,
        routine_selector,
        alsem_ver,
        deterministic,
        single_app,
    )
    .run()
}

/// The pure rendering half of [`run_query_effects_pipeline`].
pub fn render_query_effects(
    substrate: &QuerySubstrate,
    routine_selector: &str,
    all: bool,
    alsem_ver: &str,
    deterministic: bool,
) -> QueryRunResult {
    with_dependencies_line(
        render_query_effects_body(
            substrate,
            routine_selector,
            all,
            alsem_ver,
            deterministic,
            None,
        ),
        &substrate.dependencies,
    )
}

/// The exclusive bucket of one row (design §4.5, F10): table not resolved
/// first, then the write class of its temp state, then the op family.
fn bucket(t: &TableTouch<'_>) -> &'static str {
    use crate::engine::l5::rollup::{WriteClass, classify};
    if is_unknown_table(t.table_id) {
        return "tableNotResolved";
    }
    match classify(&[temp_state_str(t.temp_state).as_str()]) {
        WriteClass::Temporary => "temporary",
        WriteClass::CallerDependent(_) => "callerDependent",
        WriteClass::Physical(_) => match op_family(t.op) {
            "lock" => "locks",
            "write" => "writes",
            "validate" => "validate",
            "read" => "reads",
            _ => "unclassified",
        },
    }
}

/// `firstHop` (design §4.5, R3g): the first direct callee, in sorted order,
/// whose summary holds a row equal to this row's complete contribution
/// (operation id, table, op kind and the temp state the row has here). Each
/// callee's rows are read once into a set (Fable N10).
struct FirstHops<'q, 'a> {
    query: &'q DbEffectQuery<'a>,
    sets: std::cell::RefCell<HashMap<SolveIx, Contributions<'a>>>,
}

/// One routine's rows as contribution keys: (operation id, table, op kind,
/// temp state).
type Contributions<'a> = std::collections::HashSet<(&'a str, &'a str, &'a str, String)>;

impl<'q, 'a> FirstHops<'q, 'a> {
    fn new(query: &'q DbEffectQuery<'a>) -> Self {
        FirstHops {
            query,
            sets: Default::default(),
        }
    }

    /// The callee, or why there is none.
    fn of(&self, t: &TableTouch<'a>) -> Result<SolveIx, &'static str> {
        let callees = self.query.direct_callees(t.routine);
        if callees.is_empty() {
            return Err("no-direct-callee-summary");
        }
        let key = (
            t.operation_id,
            t.table_id,
            t.op,
            temp_state_str(t.temp_state),
        );
        let mut sets = self.sets.borrow_mut();
        for &c in callees {
            let set = sets.entry(c).or_insert_with(|| {
                self.query
                    .bundle()
                    .db_effects(c)
                    .map(|e| {
                        (
                            e.operation_id,
                            e.table_id,
                            e.op,
                            temp_state_str(e.temp_state),
                        )
                    })
                    .collect()
            });
            if set.contains(&key) {
                return Ok(c);
            }
        }
        Err("no-callee-holds-the-contribution")
    }
}

#[allow(clippy::too_many_lines)]
fn render_query_effects_body(
    substrate: &QuerySubstrate,
    routine_selector: &str,
    all: bool,
    alsem_ver: &str,
    deterministic: bool,
    cone: Option<&crate::engine::l5::digest_cli::DigestEntryFull>,
) -> QueryRunResult {
    let deps = &substrate.dependencies;
    let beyond = cone.map(|e| Beyond::new(e, substrate.resolved(), deps));
    let join = RoutineJoin::build(substrate.resolved());
    let routine =
        match resolve_routine_selector(substrate.resolved(), &join.declaring, routine_selector) {
            Resolution::Resolved(r) => r,
            other => {
                return selector_failure_doc(
                    "query.effects",
                    "routine",
                    routine_selector,
                    &other,
                    deps,
                    alsem_ver,
                    deterministic,
                );
            }
        };

    let query = substrate.query();
    let ops = OperationJoin::build(substrate.resolved());
    let tables = TableNames::build(substrate.resolved());
    let rr = join.of(routine);

    let Some(rix) = query.routine_ix(&routine.id) else {
        // ⟨final-branch-review-l3.md M-2⟩ Resolvable as a workspace routine but
        // never interned by the solve — state that rather than reporting a false
        // `effectCount: 0`, indistinguishable from "this routine performs no DB
        // work". Mirrors `render_query_touches`'s down-without-rix branch above.
        // Unreachable today (`RoutineInterner::build_canonical` interns every
        // `ws.routines` entry, so `routine_ix` is always `Some` for a resolved
        // selector) — latent, not live, but the two siblings should agree on how
        // to report this state.
        let payload = json!({
            "routine": {
                "selector": routine_selector,
                "resolution": "resolved",
                "routine": routine_ref_json(&rr),
            },
            "answerable": false,
            "reason": "routine has no db-effect row",
        });
        let human = format!(
            "--- effects of {} ---\n  not answerable: routine has no db-effect row\n",
            rr.qualified()
        );
        return QueryRunResult {
            json_text: envelope("query.effects", payload, deps, alsem_ver, deterministic),
            human_text: human,
            exit_code: 0,
            summary: Value::Null,
            ancestor_witnesses_built: 0,
        };
    };
    let effects: Vec<TableTouch<'_>> = query.all_effects(rix);
    let hops = FirstHops::new(&query);
    let hop_label = |c: SolveIx| -> String {
        let internal = query.bundle().routine_id(c);
        join.get(internal)
            .map_or_else(|| internal.to_string(), |x| x.qualified())
    };

    // Rows: every field unchanged, plus `firstHop` (or why none) on a row
    // that is not the routine's own (ConTinAI reads `via == "direct"`).
    let rows: Vec<Value> = effects
        .iter()
        .map(|t| {
            let mut v = touch_json(
                t,
                ops.get(t.operation_id, &join).as_ref(),
                Some(tables.get(t.table_id)),
            );
            // Q4 (3.1.0, additive): an unresolved table's receiver, where it
            // is a RecordRef.
            if is_unknown_table(t.table_id)
                && ops.receiver_is_record_ref(t.operation_id)
                && let Value::Object(m) = &mut v
            {
                m.insert("receiver".into(), "RecordRef".into());
            }
            if t.via != crate::engine::l4::effect_store::ViaRank::Direct
                && let Value::Object(m) = &mut v
            {
                match hops.of(t) {
                    Ok(c) => {
                        let internal = query.bundle().routine_id(c);
                        // The routine's identity (its anchor is its own
                        // `query effects` answer's).
                        m.insert(
                            "firstHop".into(),
                            join.get(internal).map_or_else(
                                || json!({"display": internal}),
                                |x| {
                                    json!({
                                        "display": x.display,
                                        "objectDisplay": x.object_display,
                                        "stableId": x.stable_id,
                                    })
                                },
                            ),
                        );
                    }
                    Err(why) => {
                        m.insert("firstHop".into(), Value::Null);
                        m.insert("firstHopUnavailable".into(), why.into());
                    }
                }
            }
            v
        })
        .collect();

    // The buckets (§4.5), each a union by identity.
    #[derive(Default)]
    struct Bucket<'t> {
        rows: Vec<usize>,
        tables: BTreeSet<&'t str>,
        owners: BTreeSet<String>,
        ops: BTreeMap<&'t str, usize>,
    }
    let mut buckets: BTreeMap<&'static str, Bucket<'_>> = BTreeMap::new();
    // Q4: the "table not resolved" bucket split by whether the receiver is a
    // RecordRef (true) or not (false); the bucket itself keeps the total.
    let mut unresolved: BTreeMap<bool, Bucket<'_>> = BTreeMap::new();
    for (i, t) in effects.iter().enumerate() {
        let name = bucket(t);
        let owner = ops.get(t.operation_id, &join).map_or_else(
            || "(outside the workspace)".to_string(),
            |o| o.owner.qualified(),
        );
        let mut parts = vec![buckets.entry(name).or_default()];
        if name == "tableNotResolved" {
            parts.push(
                unresolved
                    .entry(ops.receiver_is_record_ref(t.operation_id))
                    .or_default(),
            );
        }
        for b in parts {
            b.rows.push(i);
            b.tables.insert(t.table_id);
            *b.ops.entry(t.op).or_default() += 1;
            b.owners.insert(owner.clone());
        }
    }
    // Per table, the may-persist and caller-dependent writes (+ locks on it).
    let by_table = |name: &str| -> Vec<(&str, Vec<usize>)> {
        let mut m: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
        for &i in buckets.get(name).map_or(&[][..], |b| b.rows.as_slice()) {
            m.entry(effects[i].table_id).or_default().push(i);
        }
        let mut v: Vec<(&str, Vec<usize>)> = m.into_iter().collect();
        let name_of = |id: &str| tables.get(id).unwrap_or(id).to_string();
        v.sort_by(|a, b| {
            b.1.len()
                .cmp(&a.1.len())
                .then(name_of(a.0).cmp(&name_of(b.0)))
        });
        v
    };
    let op_counts = |rows: &[usize]| -> BTreeMap<&str, usize> {
        let mut m = BTreeMap::new();
        for &i in rows {
            *m.entry(effects[i].op).or_default() += 1;
        }
        m
    };
    let count = |name: &str| buckets.get(name).map_or(0, |b| b.rows.len());
    let ntables = |name: &str| buckets.get(name).map_or(0, |b| b.tables.len());
    let (mut known, mut unknown_temp) = (0usize, 0usize);
    for &i in buckets.get("writes").map_or(&[][..], |b| b.rows.as_slice()) {
        match effects[i].temp_state {
            TempStateKind::Known(false) => known += 1,
            _ => unknown_temp += 1,
        }
    }
    // Wave 3a: the cone's may-persist writes beyond each dependency entry
    // point (empty when the cone was not read; `summary.scope` says which).
    let beyond_writes = beyond.as_ref().map(Beyond::writes).unwrap_or_default();
    let write_tables: Vec<Value> = ["writes", "callerDependent"]
        .iter()
        .flat_map(|b| by_table(b).into_iter().map(move |t| (*b, t)))
        .map(|(b, (id, rows))| {
            let first = &effects[rows[0]];
            let mut v = json!({
                "table": tables.get(id),
                "tableId": id,
                "ops": op_counts(&rows),
                "class": if b == "writes" { "physical" } else { "callerDependent" },
                "first": {"operationId": first.operation_id},
                "beyond": beyond_of_table(&beyond_writes, id),
            });
            if first.via != crate::engine::l4::effect_store::ViaRank::Direct
                && let Ok(c) = hops.of(first)
            {
                v["first"]["firstHop"] = json!(hop_label(c));
            }
            v
        })
        .collect();
    let own_write_tables: BTreeSet<&str> = ["writes", "callerDependent"]
        .iter()
        .filter_map(|k| buckets.get(*k))
        .flat_map(|b| b.tables.iter().copied())
        .collect();
    let beyond_tables: BTreeSet<&str> = beyond_writes
        .iter()
        .flat_map(|w| w.tables.keys().copied())
        .collect();
    let beyond_entry_points: Vec<Value> = beyond_writes
        .iter()
        .map(|w| {
            json!({
                "entryPoint": w.name,
                "targets": w.targets,
                "sites": w.sites,
                "tables": w.tables.iter().map(|(id, name)| json!({
                    "table": name,
                    "tableId": id,
                    "ops": w.types_of.get(id).cloned().unwrap_or_default(),
                })).collect::<Vec<_>>(),
                "unresolvedWrites": w.unresolved,
            })
        })
        .collect();
    let summary = json!({
        "writes": {"operations": count("writes"), "tables": ntables("writes"),
                   "knownPersistent": known, "tempStateUnknown": unknown_temp},
        "callerDependent": {"operations": count("callerDependent"), "tables": ntables("callerDependent")},
        "locks": {"operations": count("locks"), "tables": ntables("locks")},
        "validate": {"operations": count("validate"), "tables": ntables("validate")},
        "reads": {"operations": count("reads"), "tables": ntables("reads")},
        "unclassified": {"operations": count("unclassified"),
                         "ops": buckets.get("unclassified").map(|b| b.ops.clone()).unwrap_or_default()},
        "temporary": {"operations": count("temporary"), "tables": ntables("temporary"),
                      "routines": buckets.get("temporary").map_or(0, |b| b.owners.len())},
        "tableNotResolved": {"operations": count("tableNotResolved"),
                             "routines": buckets.get("tableNotResolved").map_or(0, |b| b.owners.len()),
                             // Q4 (3.1.0): of them, on a RecordRef receiver.
                             "recordRef": {
                                 "operations": unresolved.get(&true).map_or(0, |b| b.rows.len()),
                                 "routines": unresolved.get(&true).map_or(0, |b| b.owners.len()),
                             }},
        "byTable": write_tables,
        // Wave 3a (3.1.0): the writes beyond entry points, from the cone.
        "beyondWrites": {
            "tables": beyond_tables.len(),
            "entryPoints": beyond_writes.len(),
            "sites": beyond_writes.iter().map(|w| w.sites.len().max(1)).sum::<usize>(),
            "tablesOnlyBeyond": beyond_tables.difference(&own_write_tables).count(),
        },
        "beyondEntryPoints": beyond_entry_points,
        "scope": scope_value(beyond.is_some()),
    });

    let payload = json!({
        "routine": {
            "selector": routine_selector,
            "resolution": "resolved",
            "routine": routine_ref_json(&rr),
        },
        "effectCount": effects.len(),
        "summary": summary,
        "effects": rows,
    });

    // Human (design §4.5): summary first, then the buckets; every row with
    // --all.
    let mut human = format!("--- effects of {} ---\n", rr.qualified());
    let mut parts = vec![format!(
        "writes that may persist {} on {} ({known} known persistent, {unknown_temp} temp state unknown)",
        count("writes"),
        plural(ntables("writes"), "table", "tables")
    )];
    if !beyond_writes.is_empty() {
        parts.push(format!(
            "writes beyond dependency entry points on {} through {} ({} only there)",
            plural(beyond_tables.len(), "table", "tables"),
            plural(beyond_writes.len(), "entry point", "entry points"),
            beyond_tables.difference(&own_write_tables).count()
        ));
    }
    for (key, label) in [
        ("callerDependent", "caller-dependent writes"),
        ("locks", "locks"),
        ("validate", "validate"),
        ("reads", "reads"),
        ("unclassified", "unclassified"),
    ] {
        if count(key) > 0 {
            parts.push(format!(
                "{label} {} on {}",
                count(key),
                plural(ntables(key), "table", "tables")
            ));
        }
    }
    if count("temporary") > 0 {
        parts.push(format!(
            "temporary records {} on {} in {}",
            count("temporary"),
            plural(ntables("temporary"), "table", "tables"),
            plural(buckets["temporary"].owners.len(), "routine", "routines")
        ));
    }
    // Q4: RecordRef operations first, then the rest of "table not resolved".
    for (record_ref, label) in [
        (true, "on a RecordRef (table decided at run time)"),
        (false, "table not resolved"),
    ] {
        if let Some(b) = unresolved.get(&record_ref) {
            parts.push(format!(
                "{label} {} in {}",
                b.rows.len(),
                plural(b.owners.len(), "routine", "routines")
            ));
        }
    }
    human.push_str(&format!(
        "  summary: {} — {}\n",
        plural(effects.len(), "operation", "operations"),
        parts.join("; ")
    ));
    if beyond.is_some() {
        // Wave 3a: the writes beyond each entry point are the cone's, read
        // as explain reads them; the per-operation rows keep L4's depth.
        human.push_str(
            "  coverage: per operation, one routine into dependency code, and operations ruled \
             out by literal arguments are listed (alsem explain and prove drop them); writes \
             beyond dependency entry points follow the whole cone with literal arguments, as \
             alsem explain does; lines are 1-based\n",
        );
        human.push_str(&format!(
            "  scope: dependency code: full cone for physical writes, grouped per entry point; \
             per-operation detail one routine deep; temporary records are not writes{}\n",
            dependencies_clause(deps, all)
        ));
    } else {
        human.push_str(
            "  coverage: one routine into dependency code; operations ruled out by literal \
             arguments are listed (alsem explain and prove drop them); lines are 1-based\n",
        );
        human.push_str(&format!(
            "  scope: per operation, one routine into dependency code (alsem explain and prove \
             follow the whole cone); temporary records are not writes{}\n",
            dependencies_clause(deps, all)
        ));
    }
    let ops_text = |m: &BTreeMap<&str, usize>| -> String {
        let mut v: Vec<(&&str, &usize)> = m.iter().collect();
        v.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
        v.iter()
            .map(|(k, n)| format!("{k} {n}"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let table_name = |id: &str| -> String {
        if is_unknown_table(id) {
            "table not resolved".to_string()
        } else {
            tables.get(id).unwrap_or(id).to_string()
        }
    };
    let locks_on: BTreeMap<&str, usize> = buckets
        .get("locks")
        .map(|b| {
            let mut m = BTreeMap::new();
            for &i in &b.rows {
                *m.entry(effects[i].table_id).or_default() += 1;
            }
            m
        })
        .unwrap_or_default();
    for (key, header, note) in [
        ("writes", "writes that may persist", ""),
        (
            "callerDependent",
            "caller-dependent writes",
            "; written only if the caller passes a record that is not temporary",
        ),
    ] {
        let list = by_table(key);
        if list.is_empty() {
            continue;
        }
        human.push_str(&format!(
            "  {header} ({}{note}):\n",
            plural(list.len(), "table", "tables")
        ));
        let n = fold_len(list.len(), 12, all);
        for (id, rows) in &list[..n] {
            let first = &effects[rows[0]];
            let at = match ops.get(first.operation_id, &join) {
                Some(o) => format!("at {}:{} in {}", o.file, shown(o.line), o.owner.qualified()),
                None => format!("(outside the workspace: {})", first.operation_id),
            };
            let through = match first.via {
                crate::engine::l4::effect_store::ViaRank::Direct => String::new(),
                v => match hops.of(first) {
                    Ok(c) => format!(", through {}{}", hop_label(c), via_note(v)),
                    Err(_) => via_note(v).to_string(),
                },
            };
            let locks = locks_on
                .get(id)
                .map(|n| format!(" (+ LockTable {n})"))
                .unwrap_or_default();
            human.push_str(&format!(
                "    {}: {}{locks}{} — first {} {at}{through}\n",
                table_name(id),
                ops_text(&op_counts(rows)),
                temp_note(first.temp_state),
                first.op
            ));
        }
        if n < list.len() {
            human.push_str(&format!(
                "    … +{} — --all lists them\n",
                plural(list.len() - n, "table", "tables")
            ));
        }
    }
    // Wave 3a: explain's own section, from the same rollup.
    if let Some(b) = &beyond {
        for l in b.lines(all) {
            human.push_str("  ");
            human.push_str(&l);
            human.push('\n');
        }
    }
    // W2-5 (2e): LockTable is its own class, never a write. A table also
    // written says so (its write line carries `+ LockTable n`); a lock-only
    // table prints its first lock's own anchor and owner; locks on a table not
    // resolved are counted apart (they are in that bucket).
    // Q4: per receiver kind (a RecordRef first), so a RecordRef lock says so.
    let unresolved_locks: Vec<(bool, usize)> = [true, false]
        .into_iter()
        .map(|record_ref| {
            let n = unresolved.get(&record_ref).map_or(0, |b| {
                b.rows
                    .iter()
                    .filter(|&&i| op_family(effects[i].op) == "lock")
                    .count()
            });
            (record_ref, n)
        })
        .filter(|&(_, n)| n > 0)
        .collect();
    if buckets.contains_key("locks") || !unresolved_locks.is_empty() {
        let written: BTreeSet<&str> = ["writes", "callerDependent"]
            .iter()
            .filter_map(|k| buckets.get(*k))
            .flat_map(|b| b.tables.iter().copied())
            .collect();
        let mut items: Vec<(String, String)> = Vec::new();
        if let Some(b) = buckets.get("locks") {
            for &t in &b.tables {
                let name = table_name(t);
                let text = if written.contains(t) {
                    format!("{name} (also written)")
                } else {
                    let first = b
                        .rows
                        .iter()
                        .map(|&i| &effects[i])
                        .find(|e| e.table_id == t)
                        .expect("a locked table has a row");
                    match ops.get(first.operation_id, &join) {
                        Some(o) => format!(
                            "{name} at {}:{} in {}",
                            o.file,
                            shown(o.line),
                            o.owner.qualified()
                        ),
                        None => format!("{name} (outside the workspace: {})", first.operation_id),
                    }
                };
                items.push((name, text));
            }
        }
        items.sort();
        let n = buckets.get("locks").map_or(0, |b| b.tables.len());
        let mut line = format!(
            "  locks ({}; not writes): {}",
            plural(n, "table", "tables"),
            items
                .into_iter()
                .map(|(_, t)| t)
                .collect::<Vec<_>>()
                .join(", ")
        );
        for (i, (record_ref, count)) in unresolved_locks.into_iter().enumerate() {
            line.push_str(&format!(
                "{}{}: {}",
                if n > 0 || i > 0 { "; " } else { "" },
                unresolved_label(record_ref),
                plural(count, "lock", "locks")
            ));
        }
        human.push_str(&line);
        human.push('\n');
    }
    for (key, label) in [
        ("validate", "validate (runs the field's trigger; may write)"),
        ("reads", "reads"),
        ("unclassified", "unclassified"),
    ] {
        if let Some(b) = buckets.get(key) {
            human.push_str(&format!(
                "  {label}: {} on {} ({}){}\n",
                b.rows.len(),
                plural(b.tables.len(), "table", "tables"),
                ops_text(&b.ops),
                if all { "" } else { " — --all lists them" }
            ));
        }
    }
    if let Some(b) = buckets.get("temporary") {
        human.push_str(&format!(
            "  temporary records: {} on {} in {}{}\n",
            plural(b.rows.len(), "operation", "operations"),
            plural(b.tables.len(), "table", "tables"),
            plural(b.owners.len(), "routine", "routines"),
            if all { "" } else { " — --all lists them" }
        ));
    }
    // Q4: "RecordRef" only where the receiver is one; every other operation
    // on an unresolved table is "table not resolved".
    for (record_ref, label) in [
        (true, "on a RecordRef (table decided at run time)"),
        (false, "table not resolved"),
    ] {
        if let Some(b) = unresolved.get(&record_ref) {
            human.push_str(&format!(
                "  {label}: {} in {} ({}){}\n",
                plural(b.rows.len(), "operation", "operations"),
                plural(b.owners.len(), "routine", "routines"),
                ops_text(&b.ops),
                if all { "" } else { " — --all lists them" }
            ));
        }
    }
    human.push_str(
        "  next: alsem prove <routine> writes-table:<table> follows the whole cone for one table\n",
    );
    if all {
        human.push_str(&format!("  every operation ({}):\n", effects.len()));
    }
    for e in effects.iter().filter(|_| all) {
        let table = if is_unknown_table(e.table_id) {
            unresolved_label(ops.receiver_is_record_ref(e.operation_id)).to_string()
        } else {
            match tables.get(e.table_id) {
                Some(name) => name.to_string(),
                None => e.table_id.to_string(),
            }
        };
        human.push_str(&format!(
            "    [{}] on {} via {}, temp {} — {}\n",
            e.op,
            table,
            e.via.as_str(),
            temp_state_str(e.temp_state),
            origin_human(ops.get(e.operation_id, &join).as_ref(), e.operation_id)
        ));
    }

    let mut summary = effects_summary(&rr.qualified(), &effects, &tables);
    // Wave 3a: the MCP summary states the cone's writes beyond entry points.
    if beyond.is_some()
        && let Value::Object(m) = &mut summary
    {
        m.insert(
            "beyondWrites".into(),
            json!({"tables": beyond_tables.len(), "entryPoints": beyond_writes.len()}),
        );
    }
    QueryRunResult {
        json_text: envelope("query.effects", payload, deps, alsem_ver, deterministic),
        human_text: human,
        exit_code: 0,
        summary,
        ancestor_witnesses_built: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::program::model::workspace::{ModelDeclarationOnlyRoutine, ModelObject, ModelTable};

    fn table(app: &str, number: i64, name: &str, stub: bool) -> ModelTable {
        ModelTable {
            id: format!("{app}/table/{number}"),
            app_guid: app.to_string(),
            table_number: number,
            name: name.to_string(),
            fields: Vec::new(),
            keys: Vec::new(),
            is_temporary: false,
            is_extension_stub: stub,
        }
    }

    fn object(
        app: &str,
        object_type: &str,
        number: i64,
        name: &str,
        decls: &[&str],
    ) -> ModelObject {
        ModelObject {
            id: format!("{app}/{object_type}/{number}"),
            app_guid: app.to_string(),
            object_type: object_type.to_string(),
            object_number: number,
            object_key: number.to_string(),
            name: name.to_string(),
            source_table_name: None,
            extends_target_name: None,
            implements_interfaces: None,
            object_subtype: None,
            page_type: None,
            inherent_commit_behavior: None,
            source_table_temporary: None,
            page_controls: Vec::new(),
            single_instance: None,
            editable: None,
            insert_allowed: None,
            modify_allowed: None,
            delete_allowed: None,
            source_anchor: None,
            declaration_only_routines: decls
                .iter()
                .map(|s| ModelDeclarationOnlyRoutine {
                    name: (*s).to_string(),
                    stable_routine_id: (*s).to_string(),
                    source_anchor: crate::engine::l5::test_support::dummy_anchor(),
                    declaration_start_line: 0,
                })
                .collect(),
            run_actions: None,
        }
    }

    /// A routine of `obj`, by assignment over the shared test row.
    fn routine(obj: &ModelObject, id: &str, stable: &str) -> ModelRoutine {
        let mut r = crate::engine::l5::test_support::routine(id, "procedure");
        r.stable_routine_id = stable.to_string();
        r.object_id = obj.id.clone();
        r.object_type = obj.object_type.clone();
        r.app_guid = obj.app_guid.clone();
        r.object_number = obj.object_number;
        r
    }

    /// #78: a header names the owner when known and never prints `::<routine>`.
    ///
    /// Mutation: drop the empty-owner branch -> the second assert reads `::Run`.
    #[test]
    fn owner_qualified_omits_an_unknown_owner() {
        assert_eq!(owner_qualified("Codeunit Foo", "Run"), "Codeunit Foo::Run");
        assert_eq!(owner_qualified("", "Run"), "Run");
    }

    /// #78: the owner index holds workspace-app rows only, a declaration-only
    /// row reads its own object, the `#if` arms of one routine (rows of ONE
    /// object sharing a stable id) give that object, two model objects with one
    /// internal id and no anchors give the same owner as `RoutineJoin` (both read
    /// `DeclaringObjects`, whose no-anchor fallback is the last row), and an
    /// unknown id gives `""`. Which arm declares an anchored routine is
    /// `declaring_objects_contains_by_anchor_else_last_row`'s.
    ///
    /// Mutations: drop the routine role filter -> (b) `Codeunit DepCu`; drop the
    /// object role filter -> (c) `Interface DepIface`; make the no-anchor
    /// fallback the first row -> (f) `Codeunit Old`.
    #[test]
    fn owner_index_filters_to_primary_and_follows_routine_join() {
        let alpha = object("ws", "Codeunit", 1, "Alpha", &[]);
        let ws_iface = object("ws", "Interface", 3, "WsIface", &["decl-ws"]);
        let dep_cu = object("dep", "Codeunit", 4, "DepCu", &[]);
        let dep_iface = object("dep", "Interface", 5, "DepIface", &["decl-dep"]);
        // Two declarations with one internal id (`#if`-split objects).
        let old = object("ws", "Codeunit", 6, "Old", &[]);
        let new = object("ws", "Codeunit", 6, "New", &[]);
        assert_eq!(old.id, new.id, "precondition: one internal object id");
        let routines = vec![
            routine(&alpha, "r-alpha", "s-alpha"),
            routine(&dep_cu, "r-dep", "s-dep"),
            routine(&alpha, "r-arm1", "s-arms"),
            routine(&alpha, "r-arm2", "s-arms"),
            routine(&old, "r-split", "s-split"),
        ];
        let mut resolved = crate::engine::l5::test_support::empty_resolved();
        resolved.workspace.objects = vec![alpha, ws_iface, dep_cu, dep_iface, old, new];
        resolved.workspace.routines = routines;
        resolved.primary_app = Some(crate::engine::gate::app_attribution::App {
            app_guid: "ws".to_string(),
            publisher: String::new(),
            name: String::new(),
            version: String::new(),
        });
        let owners = OwnerIndex::build(&resolved);
        // (a) primary routine
        assert_eq!(owners.object_display("s-alpha"), "Codeunit Alpha");
        // (b) dependency routine absent
        assert_eq!(owners.object_display("s-dep"), "");
        // (c) dependency declaration-only row absent
        assert_eq!(owners.object_display("decl-dep"), "");
        // (d) primary declaration-only row
        assert_eq!(owners.object_display("decl-ws"), "Interface WsIface");
        // (e) the arms of one routine
        assert_eq!(owners.object_display("s-arms"), "Codeunit Alpha");
        // (f) one internal id, two names, no anchors: the last row, as `query effects`
        assert_eq!(
            owners.object_display("s-split"),
            RoutineJoin::build(&resolved)
                .get("r-split")
                .unwrap()
                .object_display
        );
        assert_eq!(owners.object_display("s-split"), "Codeunit New");
        // (g) unknown id
        assert_eq!(owners.object_display("nope"), "");
        assert_eq!(owners.qualified("nope", "Run"), "Run");
        assert_eq!(owners.qualified("s-alpha", "Run"), "Codeunit Alpha::Run");
    }

    /// An anchor in `file` from `(start_line, 0)` to `(end_line, 0)`.
    fn anchor(
        file: &str,
        start_line: u32,
        end_line: u32,
    ) -> crate::program::body::features::PAnchor {
        let mut a = crate::engine::l5::test_support::dummy_anchor();
        a.source_unit_id = file.to_string();
        a.start_line = start_line;
        a.end_line = end_line;
        a
    }

    /// U1 (#241). `DeclaringObjects::of` over hand-stated rows sharing one id:
    /// without anchors it gives the LAST row (the pre-#241 owner); with anchors
    /// it gives the row whose declaration contains the routine's start, in
    /// either row order, never a row of another file; of overlapping anchors,
    /// the innermost (latest start, then earliest end).
    ///
    /// Discrimination: make the no-anchor fallback pick the first row -> (a)
    /// reads `Old`; fails. Replace containment with "first row" -> (c) reads
    /// `Bar`'s routine as `Foo`; fails. Take the first containing row instead
    /// of the innermost -> (f) reads `Outer`; fails. Order by
    /// `PAnchor::extent_cmp` instead -> (g) reads `Outer` (a tie); fails.
    #[test]
    fn declaring_objects_contains_by_anchor_else_last_row() {
        let name = |o: Option<&ModelObject>| o.map(|o| o.name.clone());

        // (a) no anchors: the last row.
        let old = object("ws", "Codeunit", 6, "Old", &[]);
        let new = object("ws", "Codeunit", 6, "New", &[]);
        assert_eq!(old.id, new.id, "precondition: one internal object id");
        let r = routine(&old, "r", "s");
        let rows = vec![old, new];
        assert_eq!(
            name(DeclaringObjects::new(&rows).of(&r)),
            Some("New".into())
        );

        // (b) / (c) anchors: the containing row, whichever comes first.
        let mut arm1 = object("ws", "Codeunit", 7, "Foo", &[]);
        arm1.source_anchor = Some(anchor("ws:f.al", 1, 10));
        let mut arm2 = object("ws", "Codeunit", 7, "Bar", &[]);
        arm2.source_anchor = Some(anchor("ws:f.al", 12, 20));
        let mut in_foo = routine(&arm1, "r-foo", "s-foo");
        in_foo.source_anchor = anchor("ws:f.al", 3, 8);
        let mut in_bar = routine(&arm2, "r-bar", "s-bar");
        in_bar.source_anchor = anchor("ws:f.al", 14, 18);
        for rows in [
            vec![arm1.clone(), arm2.clone()],
            vec![arm2.clone(), arm1.clone()],
        ] {
            let d = DeclaringObjects::new(&rows);
            assert_eq!(name(d.of(&in_foo)), Some("Foo".into()), "(b)");
            assert_eq!(name(d.of(&in_bar)), Some("Bar".into()), "(c)");
        }

        // (d) a routine of another file is in neither row: the last row.
        let mut elsewhere = routine(&arm1, "r-x", "s-x");
        elsewhere.source_anchor = anchor("ws:g.al", 3, 8);
        let rows = vec![arm1, arm2];
        assert_eq!(
            name(DeclaringObjects::new(&rows).of(&elsewhere)),
            Some("Bar".into())
        );

        // (e) no row for the id.
        let other = object("ws", "Codeunit", 8, "Other", &[]);
        assert_eq!(
            name(DeclaringObjects::new(&rows).of(&routine(&other, "r", "s"))),
            None
        );

        // (f) overlapping (error-recovered) anchors: an outer row spanning the
        // whole file contains the routine too; the SMALLEST containing anchor
        // declares it, in either row order.
        let mut outer = object("ws", "Codeunit", 9, "Outer", &[]);
        outer.source_anchor = Some(anchor("ws:f.al", 0, 40));
        let mut inner = object("ws", "Codeunit", 9, "Inner", &[]);
        inner.source_anchor = Some(anchor("ws:f.al", 12, 20));
        let mut in_inner = routine(&inner, "r-in", "s-in");
        in_inner.source_anchor = anchor("ws:f.al", 14, 18);
        for rows in [
            vec![outer.clone(), inner.clone()],
            vec![inner.clone(), outer.clone()],
        ] {
            let d = DeclaringObjects::new(&rows);
            assert_eq!(name(d.of(&in_inner)), Some("Inner".into()), "(f)");
        }

        // (g) a strictly nested anchor whose line/column extent TIES its
        // container (outer (0,0)..(40,0), inner (0,5)..(40,0)): the innermost
        // (latest start) declares it, in either row order.
        let mut outer = object("ws", "Codeunit", 10, "Outer", &[]);
        outer.source_anchor = Some(anchor("ws:f.al", 0, 40));
        let mut inner = object("ws", "Codeunit", 10, "Inner", &[]);
        let mut inner_anchor = anchor("ws:f.al", 0, 40);
        inner_anchor.start_column = 5;
        inner.source_anchor = Some(inner_anchor);
        let mut in_inner = routine(&inner, "r-in", "s-in");
        in_inner.source_anchor = anchor("ws:f.al", 14, 18);
        for rows in [
            vec![outer.clone(), inner.clone()],
            vec![inner.clone(), outer.clone()],
        ] {
            let d = DeclaringObjects::new(&rows);
            assert_eq!(name(d.of(&in_inner)), Some("Inner".into()), "(g)");
        }
    }

    fn resolved_id(r: Resolution<TableRef>) -> String {
        match r {
            Resolution::Resolved(t) => t.id,
            other => panic!("not resolved: {other:?}"),
        }
    }

    /// #66: `--table <number>` selects the table with that number, in any app; a
    /// `tableextension` stub's number is not a table number.
    #[test]
    fn table_selector_number_tier() {
        let tables = vec![
            table("base", 18, "Customer", false),
            table("ws", 50160, "Order Pollution Flag", false),
            table("ws", 50170, "Customer Ext", true),
        ];
        assert_eq!(
            resolved_id(resolve_table_selector_in(&tables, "18")),
            "base/table/18"
        );
        assert_eq!(
            resolved_id(resolve_table_selector_in(&tables, "50160")),
            "ws/table/50160"
        );
        assert_eq!(
            resolve_table_selector_in(&tables, "50170"),
            Resolution::Unmatched
        );
        assert_eq!(
            resolved_id(resolve_table_selector_in(&tables, "customer")),
            "base/table/18"
        );
    }

    /// Two apps declaring the same number (or a table NAMED "18") is an ambiguity
    /// with every candidate listed, never one silently chosen.
    #[test]
    fn table_selector_number_ambiguity_lists_candidates() {
        let tables = vec![
            table("a", 18, "Customer", false),
            table("b", 18, "Other Customer", false),
            table("c", 50000, "18", false),
        ];
        assert_eq!(
            resolve_table_selector_in(&tables, "18"),
            Resolution::Ambiguous(vec![
                "Customer (a/table/18)".to_string(),
                "Other Customer (b/table/18)".to_string(),
                "18 (c/table/50000)".to_string(),
            ])
        );
    }

    /// A stub sharing a real table's id loses to the real table (G-5), so
    /// the number selects the real one.
    #[test]
    fn table_selector_number_prefers_the_real_table_over_its_stub() {
        let tables = vec![
            table("ws", 50100, "Ext Stub", true),
            table("ws", 50100, "Real Table", false),
        ];
        match resolve_table_selector_in(&tables, "50100") {
            Resolution::Resolved(t) => assert_eq!(t.name.as_deref(), Some("Real Table")),
            other => panic!("{other:?}"),
        }
    }
}

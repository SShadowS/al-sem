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

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use crate::engine::l4::routine_interner::SolveIx;

use serde_json::{Map, Value, json};

use crate::engine::batch::job::{
    Doc, Job, JobFailure, PlanPart, ResolveCx, SideDoc, SideIx, ViewCx, ViewDemand, ViewFailure,
    ViewNeeds, WorkspaceRead,
};
use crate::engine::batch::output::JobOutput;
use crate::engine::follow_up::{Input, Surface};
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
/// capability cone. 3.2.0 (output step 1b, additive): a touches `down` with
/// no indexed touch says what that means (`down.absence {answer: no |
/// unknown | beyond, index, reasons[]}`), the workspace-global list states
/// its `index` depth, and `--callers` adds the trivial case's `up.callers[]`
/// (`{depth, routine, outcome}`); `query effects` marks each row the cone
/// judges (`literalRoute {status, coneFirstHop?}`, `byTable[].first.literalRoute`,
/// `byTable[].literalRuledOut`, `summary.literalRoutes`), rows and `via`
/// unchanged. 3.3.0 (agent-tasks milestone D, additive): `touches --field`
/// answers with `field`, `searched`, `fieldSummary`, `fieldTouches[]`,
/// `declaredKeys[]`, `reasons[]` and, with no site, `absence`; a touches
/// answer without `--field` keeps its shape. 3.4.0 (additive): a touches
/// `up` names the caller relation its `transitiveCallers` counts
/// (`up.relation: "code-calls"`, [`UP_RELATION`]). 3.5.0 (additive,
/// callers-through-dependency design r2): a scoped touches `up` adds
/// `up.throughDependency`, the workspace callers that reach the routine only
/// by code calls through dependency code, each with its tier and witness
/// ([`render_through_dependency`]); `transitiveCallers` and the rest of `up`
/// are unchanged. 3.6.0 (output validation V1, F15/F16): `query effects`
/// splits the caller-dependent and temporary classes by op family as the
/// physical class is, so a caller-dependent or temporary read, lock, validate
/// or unclassified row moves to that family's bucket (`summary.reads`, …) and
/// out of `summary.callerDependent` / `summary.temporary` / `byTable`; a
/// caller-dependent row adds `parameterPosition` (1-based) and
/// `parameterName` (additive; `tempState` keeps the 0-based index). 3.7.0
/// (output validation V3, §2.1, additive): every routine object gains
/// `qualifiedDisplay` (the one routine display: member, AL quoting, an
/// overloaded name's parameter types), `signature`, `member` and `selector`;
/// `display` keeps the routine's name. 3.8.0 (output validation V2, §5.2
/// F61, §9.3 F74, additive): `query effects` adds
/// `payload.result.scope.families` (`reads`, `locks`, `validate`, `writes`,
/// `beyondDependencyCalls`, each `{depth, inventory, reasons, blockers}`);
/// `touches --field` splits its inventory (`fieldSummary.confirmed`,
/// `fieldSummary.numericCandidates`) and adds `filters`.
/// 3.9.0 (output validation V4, F76, additive): `payload.result.scope` in
/// every query document (analysis, reasons, assumptions, limits, universe,
/// scope), beside query effects' `families`.
/// 3.10.0 (output validation V6, F59, additive): table mode's
/// `up.directSites` adds `dependencyRoutines`, `workspaceByOp`,
/// `dependencyByOp`, `workspaceByObject` and `keyRelevant`; field mode's
/// `fieldSummary` adds `confirmedSites` and `candidateSites`.
const QUERY_SCHEMA_VERSION: &str = "3.10.0";

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
        /// `--callers` (step 1b): every caller named, no witness product.
        callers: bool,
        /// `--field` (agent-tasks D): the field's consumers, not the table's
        /// record operations ([`super::field_touches`]).
        field: Option<&'a str>,
        /// `--op-family write` (round 2 G8): table mode lists every
        /// record-write site.
        writes_only: bool,
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
    /// `--surface` (output validation §6.1): the words of every follow-up.
    surface: Surface,
    /// `--schema`: 5 (the default, [`super::query_v5`]), 4 (the 4.0.0
    /// document) or 3 (the 3.x document), each previous one kept for one
    /// release.
    schema: u32,
}

impl<'a> QueryJob<'a> {
    /// `--surface` (output validation §6.1).
    #[must_use]
    pub fn surface(mut self, surface: Surface) -> QueryJob<'a> {
        self.surface = surface;
        self
    }

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
            callers: false,
            field: None,
            writes_only: false,
        };
        Self::with(inputs, ask, alsem_ver, deterministic, single_app)
    }

    /// `query touches --op-family write` (round 2 G8): table mode lists
    /// every record-write site, temporary records included and tagged.
    #[must_use]
    pub fn writes_only(mut self, only: bool) -> QueryJob<'a> {
        if let QueryAsk::Touches { writes_only, .. } = &mut self.ask {
            *writes_only = only;
        }
        self
    }

    /// `query touches --field F` (agent-tasks D): every code consumer of
    /// field `F` of the table, by use kind.
    #[must_use]
    pub fn field(mut self, selector: Option<&'a str>) -> QueryJob<'a> {
        if let QueryAsk::Touches { field, .. } = &mut self.ask {
            *field = selector;
        }
        self
    }

    /// `query touches --callers` (step 1b): name every transitive caller,
    /// with its depth and location, without `--all`'s witness product.
    #[must_use]
    pub fn list_callers(mut self, list: bool) -> QueryJob<'a> {
        if let QueryAsk::Touches { callers, .. } = &mut self.ask {
            *callers = list;
        }
        self
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
            surface: Surface::Cli,
            schema: 5,
        }
    }

    /// `--schema 3` (output validation §7.2): the 3.x document; `--schema 4`
    /// (round-2 §4.6): the 4.0.0 document; each kept for one release; any
    /// other value is 5.0.0.
    #[must_use]
    pub fn schema(mut self, schema: u32) -> QueryJob<'a> {
        self.schema = schema;
        self
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

    /// The view this job reads: the model, and with `--field --from` asked
    /// up also the sited graph (of the universe asked: cross-app, or the
    /// workspace alone), whose R0 reverse walk names the callers (as `alsem
    /// tests` and impact read them). The solve graph names them only when
    /// the graph did not build, which the answer states. A touches `--from`
    /// asked up without `--field` reads the cross-app sited graph too, for
    /// `up.throughDependency` (design r2 §3.2; single-app has no dependency
    /// code and reads none). The engine host's pin rule is `alsem.rs`'
    /// `reads_sited`.
    fn needs(&self) -> ViewNeeds {
        match self.ask {
            QueryAsk::Touches {
                field: Some(_),
                from: Some(_),
                direction,
                ..
            } if direction.wants_up() => ViewNeeds::MODEL_SITED,
            QueryAsk::Touches {
                field: None,
                from: Some(_),
                direction,
                ..
            } if direction.wants_up() && !self.single_app => ViewNeeds::MODEL_SITED,
            _ => ViewNeeds::MODEL,
        }
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
        ViewDemand::one(self.inputs, self.single_app, false, self.needs())
    }

    fn resolve(&self, _: SideIx, cx: &ResolveCx<'_>) -> Result<PlanPart, JobFailure> {
        // Wave 3a: a question about one routine's cone (effects, or touches
        // asked down from a routine) also reads the capability cone over
        // that root, for the writes beyond each dependency entry point. A
        // selector that does not resolve reads none (its answer is the
        // failure document, as before).
        // Agent-tasks D: `--field` reads the model alone; with `--from` also
        // the solve graph, for the routine's cone and callers. No tree.
        if let QueryAsk::Touches {
            field: Some(_),
            from,
            ..
        } = self.ask
        {
            let (plan, reads) = if from.is_some() {
                (query_plan(), vec![Cell::Rel, Cell::L4])
            } else {
                (crate::engine::session::field_plan(), Vec::new())
            };
            return Ok(PlanPart {
                plan,
                reads,
                carry: None,
            });
        }
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
        if let QueryAsk::Touches {
            table,
            field: Some(field),
            from,
            direction,
            all,
            ..
        } = self.ask
        {
            let ask = FieldAsk {
                table,
                field,
                from,
                direction,
                all,
                surface: self.surface,
            };
            let body = if from.is_some() {
                // `Some(None)`: declared, its build did not finish.
                let sited = self.needs().sited.then(|| cx.sited());
                let substrate = released_substrate(cx);
                let query = substrate.query();
                render_field_touches(
                    substrate.resolved(),
                    &substrate.dependencies,
                    Some((&query, sited)),
                    &ask,
                    self.alsem_ver,
                    self.deterministic,
                )
            } else {
                let view = cx.view();
                render_field_touches(
                    view.model(),
                    &view.target().dependencies(),
                    None,
                    &ask,
                    self.alsem_ver,
                    self.deterministic,
                )
            };
            let deps = cx.view().target().dependencies();
            let sources = (self.show_source && !self.json)
                .then(|| crate::engine::source_excerpt::SourceTexts::read(&workspace));
            let hc = QueryHeaderCx::of(&self.ask, self.surface, self.single_app);
            let result = finish_human(body, &deps, sources.as_ref(), &hc);
            return Ok(SideDoc::new(Ok::<_, String>(result)));
        }
        let root: Option<String> = cx.take_carry();
        // The cone's entry first: the substrate below releases every cell
        // but `l4`, the tree's effects included.
        let cone = match root {
            Some(root) if cx.has_tree() => cone_entry(cx, root),
            _ => None,
        };
        // `up.throughDependency` (design r2): declared with the sited graph.
        // Single-app, and cross-app with no dependency loaded, have no
        // dependency code and answer alike (the no-dependency contract:
        // cross-app then reports exactly what single-app does).
        let no_dependency_code =
            self.single_app || cx.view().target().dependencies().apps.is_empty();
        let through = match (no_dependency_code, self.needs().sited) {
            (true, _) => UpThrough::NoDependencyCode,
            (false, true) => cx.sited().map_or(UpThrough::NotBuilt, UpThrough::Graph),
            (false, false) => UpThrough::NotRequested,
        };
        let substrate = released_substrate(cx);
        let cone = cone.as_ref();
        let body = match self.ask {
            QueryAsk::Touches {
                table,
                from,
                direction,
                all,
                callers,
                field: _,
                writes_only,
            } => render_query_touches_body(
                &substrate,
                table,
                from,
                direction,
                all,
                callers,
                writes_only,
                self.alsem_ver,
                self.deterministic,
                cone,
                through,
                self.surface,
                self.single_app,
            ),
            QueryAsk::Effects { routine, all } => render_query_effects_body(
                &substrate,
                routine,
                all,
                self.alsem_ver,
                self.deterministic,
                cone,
                self.surface,
                self.single_app,
            ),
        };
        let sources = (self.show_source && !self.json)
            .then(|| crate::engine::source_excerpt::SourceTexts::read(&workspace));
        let hc = QueryHeaderCx::of(&self.ask, self.surface, self.single_app);
        let result = finish_human(body, &substrate.dependencies, sources.as_ref(), &hc);
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
                let text = match (self.json, self.schema) {
                    (false, _) => r.human_text,
                    (true, 3) => r.json_text,
                    (true, 4) => v4_text(&r.json_text),
                    (true, _) => v5_text(&r.json_text),
                };
                (text, r.exit_code, r.summary)
            }),
        )
    }
}

/// The 4.0.0 text of a 3.x query document ([`super::query_v4`]); a text
/// that does not parse is returned as it is.
fn v4_text(v3: &str) -> String {
    match serde_json::from_str::<Value>(v3) {
        Ok(doc) => serialize_document_value(super::query_v4::document_from_v3(doc)),
        Err(_) => v3.to_string(),
    }
}

/// The 5.0.0 text of a 3.x query document ([`super::query_v4`] then
/// [`super::query_v5`], round-2 output validation §4.6); a text that does
/// not parse is returned as it is.
fn v5_text(v3: &str) -> String {
    match serde_json::from_str::<Value>(v3) {
        Ok(doc) => serialize_document_value(super::query_v5::document_from_v4(
            super::query_v4::document_from_v3(doc),
        )),
        Err(_) => v3.to_string(),
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
    fn lines(&self, all: bool, surface: Surface) -> Vec<String> {
        crate::engine::l5::routine_rollup::beyond_lines(
            &self.rollup,
            self.entry,
            &self.rcx,
            crate::engine::l5::routine_rollup::RenderOpts::new(all, None).with_surface(surface),
        )
    }
}

/// Whether the root's cone enters dependency code (engine limits B8), so
/// that `query effects`' one-dependency-routine-deep inventory is a limit of
/// the answer: true when the cone was not read (not known), when its search
/// was cut short, when it visits a dependency routine (the tree's boundary
/// rule, as explain's `dependency code: none reached`), or when an operation
/// is dependency code's (`in_dependency`).
fn enters_dependency_code(
    cone: Option<&Beyond<'_>>,
    effects: &[TableTouch<'_>],
    in_dependency: impl Fn(&str) -> bool,
) -> bool {
    let Some(b) = cone else {
        return true;
    };
    let t = &b.entry.unresolved_traversal;
    t.dependency_routines > 0
        || t.truncated
        || effects.iter().any(|e| in_dependency(e.operation_id))
}

/// Step 1b (astra top 5): how the root's cone, after the literal pass,
/// judges one `query effects` row's route. The rows stay L4's (spec §12.2:
/// never dropped, `via` unchanged); this only marks them, and only where the
/// cone can say something sound. The cone keeps no per-operation identity
/// for every row (one effect per kind and table carries one evidence
/// operation, and its `origin_operation_ids` need not hold every operation
/// of that kind), so a row is judged by operation only when the cone names
/// its operation (origin or evidence), and by kind and table only to say
/// the cone has no live write of that kind on that table at all. A row the
/// cone does not carry (a read, a lock, a temporary record, an operation
/// one routine into dependency code that the cone groups beyond an entry
/// point) is not judged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LiteralRoute<'c> {
    /// Every route is ruled out: an effect the operation originates from is
    /// literal-dead (an effect is dead only when every operation it
    /// originates from is, `literal_reach`).
    RuledOut,
    /// The cone keeps a live route entering through the row's first hop.
    ConeRoute,
    /// The cone keeps no live listed route for it; its searched live
    /// witness enters through this other first hop (a stable id). The row's
    /// first hop is the index's, which these literal arguments may rule out.
    OtherRoute(&'c str),
    /// A write of workspace code whose kind the cone (it follows the root's
    /// literal arguments through its parameter guards, and its search was
    /// not cut short) never performs on this table: no own effect of that
    /// kind on it stays live, and no dependency group lists it for that
    /// kind.
    NotInCone,
}

type ConeEffect = crate::engine::l5::digest_cli::DigestEffectFull;

/// The cone's effect type of an L4 write kind (`digest::map_op`: `Rename`
/// is a modify); `None` for a kind the kind-level judgement does not map
/// (`ModifyAll`, whose cone fact kind is not pinned here): it then judges
/// nothing.
fn write_effect_type(op: &str) -> Option<&'static str> {
    match op {
        "Insert" => Some("DB_INSERT"),
        "Modify" | "Rename" => Some("DB_MODIFY"),
        "Delete" | "DeleteAll" => Some("DB_DELETE"),
        _ => None,
    }
}

/// The cone's effects by origin operation and by evidence anchor, and the
/// (effect type, table) pairs it may write, for [`LiteralRoute`].
struct ConeRoutes<'c> {
    by_op: HashMap<&'c str, Vec<&'c ConeEffect>>,
    /// `(file, 0-based line, column)` of an own effect's evidence.
    by_anchor: HashMap<(&'c str, u32, u32), Vec<&'c ConeEffect>>,
    /// Live own writes, and every group's listed resources (dead or not):
    /// `(effect type, table id)`.
    kinds: std::collections::HashSet<(&'c str, &'c str)>,
    /// The cone's search was cut short: no `NotInCone`.
    truncated: bool,
}

impl<'c> ConeRoutes<'c> {
    fn new(entry: &'c crate::engine::l5::digest_cli::DigestEntryFull) -> Self {
        let mut by_op: HashMap<&str, Vec<_>> = HashMap::new();
        let mut by_anchor: HashMap<_, Vec<_>> = HashMap::new();
        let mut kinds = std::collections::HashSet::new();
        for e in &entry.effects {
            let ty = e.effect_type.as_str();
            if e.crossing.is_some() {
                for (id, _) in &e.group_resources {
                    kinds.insert((ty, id.as_str()));
                }
                continue;
            }
            for op in &e.origin_operation_ids {
                by_op.entry(op.as_str()).or_default().push(e);
            }
            if let (Some(f), Some(l), Some(c)) = (
                e.evidence.file.as_deref(),
                e.evidence.line,
                e.evidence.column,
            ) {
                by_anchor.entry((f, l, c)).or_default().push(e);
            }
            if !e.literal_unreachable
                && let Some((_, id)) = e.detail.iter().find(|(k, _)| k == "resourceId")
            {
                kinds.insert((ty, id.as_str()));
            }
        }
        ConeRoutes {
            by_op,
            by_anchor,
            kinds,
            truncated: entry.unresolved_traversal.truncated,
        }
    }

    /// `anchor`: the operation's `(file, 0-based line, column)`.
    /// `first_hop`: the row's first hop's stable id (`None` on the root's own
    /// row, which only `RuledOut` and `NotInCone` can mark). `write`: the
    /// row's `(effect type, table id)` when it is a write a workspace routine
    /// performs. `None`: not judged.
    fn judge(
        &self,
        operation_id: &str,
        anchor: Option<(&str, u32, u32)>,
        first_hop: Option<&str>,
        write: Option<(&str, &str)>,
    ) -> Option<LiteralRoute<'c>> {
        let by_op = self.by_op.get(operation_id).map_or(&[][..], Vec::as_slice);
        if by_op.iter().any(|e| e.literal_unreachable) {
            return Some(LiteralRoute::RuledOut);
        }
        let by_anchor = anchor
            .and_then(|a| self.by_anchor.get(&a))
            .map_or(&[][..], Vec::as_slice);
        let effects: Vec<&ConeEffect> = by_op
            .iter()
            .chain(by_anchor)
            .copied()
            .filter(|e| !e.literal_unreachable)
            .collect();
        if effects.is_empty() {
            return match write {
                Some(k) if !self.truncated && !self.kinds.contains(&k) => {
                    Some(LiteralRoute::NotInCone)
                }
                _ => None,
            };
        }
        let hop = first_hop?;
        let (mut listed, mut witness) = (false, None);
        for e in effects {
            for p in &e.via_paths {
                if let Some(h) = p.first() {
                    listed = true;
                    if h.inner.to_routine_id.as_deref() == Some(hop) {
                        return Some(LiteralRoute::ConeRoute);
                    }
                }
            }
            if let Some(Ok(w)) = &e.own_path
                && let Some(s) = w.steps.first()
            {
                if s.to == hop {
                    return Some(LiteralRoute::ConeRoute);
                }
                witness.get_or_insert(s.to.as_str());
            }
        }
        // A live listed route through another first hop says nothing about
        // this one (the listing is capped): not judged.
        if listed {
            return None;
        }
        witness.map(LiteralRoute::OtherRoute)
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

    // Output step 1b (fable #7): an `(…)` parameter list picks among
    // overloads, in every tool's one grammar.
    let (selector, wanted) = crate::engine::l5::selector_index::split_signature(selector);
    let signature_ok = |r: &ModelRoutine| {
        wanted
            .as_ref()
            .is_none_or(|w| crate::engine::l5::selector_index::signature_matches(w, &r.parameters))
    };
    let routines = || routines().filter(|r| signature_ok(r));
    let want = selector.to_lowercase();
    // `Object.Routine` as written, or (MCP output plan 3b: one selector
    // grammar for every tool) `Owner::Routine` / `Owner.Routine` with the
    // owner's type word and AL quotes optional, as explain, prove and digest
    // read it.
    let split = crate::engine::l5::selector_index::split_qualified(selector);
    let qualified: Vec<&ModelRoutine> = routines()
        .filter(|r| {
            format!("{}.{}", object_name(r), r.name).to_lowercase() == want
                || split
                    .as_ref()
                    .is_some_and(|q| q.selects(&r.object_type, object_name(r), &r.name))
        })
        .collect();
    let bare: Vec<&ModelRoutine> = routines()
        .filter(|r| r.name.to_lowercase() == want)
        .collect();
    // V3 §2.1: a member form (`Object."Field"::Trigger`,
    // `Object.action(Name)::OnAction`), the one display of a member trigger.
    let member: Vec<&ModelRoutine> = if qualified.is_empty() {
        let owner_display = |r: &ModelRoutine| object_key(&r.object_type, object_name(r));
        let rows = routines().filter_map(|r| {
            Some(crate::engine::l5::selector_index::MemberRow {
                id: &r.stable_routine_id,
                owner_display: owner_display(r),
                member: r.enclosing_member.as_deref()?,
                member_kind: r.member_detail.as_ref().map_or_else(
                    || {
                        crate::engine::l5::routine_rollup::infer_member_kind(
                            &r.object_type,
                            &r.name,
                        )
                    },
                    |d| d.kind,
                ),
                routine: &r.name,
            })
        });
        match crate::engine::l5::selector_index::resolve_member_selector(selector, rows) {
            Some(ids) => routines()
                .filter(|r| ids.contains(&r.stable_routine_id))
                .collect(),
            None => Vec::new(),
        }
    } else {
        Vec::new()
    };

    // Most specific non-empty tier wins; an ambiguity inside a tier does NOT
    // fall through to a looser tier (that would answer a different question).
    let hits = if !qualified.is_empty() {
        qualified
    } else if !member.is_empty() {
        member
    } else {
        bare
    };
    let mut hits = hits;
    hits.sort_by(|a, b| a.id.cmp(&b.id));
    match hits.len() {
        0 => Resolution::Unmatched,
        1 => Resolution::Resolved(hits[0]),
        // Step 1b: each candidate by the selector that picks it, its
        // location and stable id (overloads share owner and name).
        _ => Resolution::Ambiguous(
            hits.iter()
                .map(|r| {
                    crate::engine::l5::selector_index::candidate_label(
                        &owner_qualified(&object_display(&r.object_type, object_name(r)), &r.name),
                        r,
                    )
                })
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
    /// The one routine display without the member's caption (V3 §2.1).
    pub canonical: String,
    /// [`Self::canonical`] without its object: what a line grouped under
    /// the object prints (`"Table No."::OnValidate`, `Note(Text)`).
    pub in_object: String,
    /// The one routine display with the member's caption (headers).
    pub header: String,
    /// The JSON identity fields (`signature`, `member`, `selector`).
    pub identity: serde_json::Map<String, Value>,
}

impl RoutineRef {
    /// The one routine display (V3 §2.1) as a list row prints it (no
    /// member caption).
    pub fn qualified(&self) -> String {
        self.canonical.clone()
    }
}

/// The join table: internal routine id -> its `ModelRoutine`, plus each
/// routine's declaring object ([`DeclaringObjects`], #241: an `#if`-split object
/// has one id for two names, so the owner is never looked up by id alone).
pub struct RoutineJoin<'a> {
    by_id: HashMap<&'a str, &'a ModelRoutine>,
    declaring: DeclaringObjects<'a>,
    names: crate::engine::l5::routine_rollup::RoutineNames<'a>,
}

impl<'a> RoutineJoin<'a> {
    pub fn build(resolved: &'a Model) -> Self {
        let ws = &resolved.workspace;
        RoutineJoin {
            by_id: ws.routines.iter().map(|r| (r.id.as_str(), r)).collect(),
            declaring: DeclaringObjects::new(&ws.objects),
            names: crate::engine::l5::routine_rollup::RoutineNames::new(resolved, &[]),
        }
    }

    /// The one routine display's names (V3 §2.1).
    pub fn names(&self) -> &crate::engine::l5::routine_rollup::RoutineNames<'a> {
        &self.names
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
        use crate::engine::l5::routine_rollup::format_routine;
        let object_name = self.declaring.of(r).map_or("", |o| o.name.as_str());
        let parts = self.names.parts_of(r);
        let mut identity = self.names.identity_json(&r.stable_routine_id);
        identity.remove("display");
        RoutineRef {
            display: r.name.clone(),
            object_display: object_display(&r.object_type, object_name),
            stable_id: r.stable_routine_id.clone(),
            file: r.source_anchor.source_unit_id.clone(),
            line: r.source_anchor.start_line,
            column: r.source_anchor.start_column,
            canonical: format_routine(&parts, false),
            in_object: crate::engine::l5::routine_rollup::split_display(&parts).1,
            header: format_routine(&parts, true),
            identity,
        }
    }
}

/// The one owner display: `<ObjectType> <ObjectName>` with the name quoted as
/// AL needs it (`Codeunit "CDO Log Management"`, `Table Customer`), the bare
/// type when the name is missing. `query effects`, digest, fingerprint and
/// prove all print it (#78; round-2 output validation G41: through r1 §2.1's
/// formatter, [`crate::engine::l5::routine_rollup::format_object`]).
pub fn object_display(object_type: &str, object_name: &str) -> String {
    crate::engine::l5::routine_rollup::format_object(object_type, object_name, None)
        .trim()
        .to_string()
}

/// An owner's matching key: `<ObjectType> <ObjectName>` unquoted, what a
/// selector's owner part is compared with ([`crate::engine::l5::selector_index`]
/// reads both the unquoted and the as-written owner). Never printed.
pub fn object_key(object_type: &str, object_name: &str) -> String {
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

    /// The owner's matching key ([`object_key`], unquoted), or `""`: what a
    /// selector's owner part is compared with.
    pub fn object_key(&self, stable_id: &str) -> String {
        self.by_stable_id
            .get(stable_id)
            .map(|(t, n)| object_key(t, n))
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

    /// The operation is dependency code's: its owning row is not a
    /// workspace routine, or the model holds no owner for it (a retained
    /// dependency leaf). Touches' absence reasons count these (the cone
    /// entered dependency code, which the index follows one routine deep).
    fn in_dependency(&self, operation_id: &str, resolved: &Model) -> bool {
        self.index
            .last_record_operation(operation_id)
            .is_none_or(|(r, _)| !is_primary_routine(resolved, r))
    }
}

/// The human name of a row's table (§8.1 target resolution): a RecordRef's
/// "table not resolved (RecordRef)" (Q4) is said only where the receiver is
/// one; any other unresolved table is "table not resolved".
fn unresolved_label(record_ref: bool) -> &'static str {
    if record_ref {
        "a RecordRef (table not resolved: RecordRef)"
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

/// A routine's JSON: `display` (its name, as before) and `objectDisplay`,
/// plus (V3 §2.1, 3.7.0) `qualifiedDisplay` (the one display), `signature`,
/// `member` and `selector`.
fn routine_ref_json(rr: &RoutineRef) -> Value {
    let mut v = json!({
        "display": rr.display,
        "objectDisplay": rr.object_display,
        "qualifiedDisplay": rr.header,
        "stableId": rr.stable_id,
        "anchor": { "file": rr.file, "line": shown(rr.line), "column": rr.column },
    });
    for (k, x) in &rr.identity {
        v[k] = x.clone();
    }
    v
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
        // Round-2 G43: the §8.1 temp-state words (`temporary`, `not
        // temporary`, `caller-dependent`, `not resolved`), never
        // `known(false)`.
        let word = match e.temp_state {
            TempStateKind::Known(true) => "temporary",
            TempStateKind::Known(false) => "not temporary",
            TempStateKind::ParameterDependent(_) => "caller-dependent",
            _ => "not resolved",
        };
        *by_temp.entry(word.to_string()).or_default() += 1;
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
    // Output validation §5.2 (F43): the writes that may persist by table
    // (the first ten by operation count, with their counts), the table
    // total, and the caller-dependent and table-not-resolved writes apart.
    let mut by_table: BTreeMap<&str, BTreeMap<&str, u64>> = BTreeMap::new();
    let (mut caller_dependent, mut unresolved_writes) = (0u64, 0u64);
    for e in effects {
        match bucket(e) {
            "writes" => {
                let name = tables.get(e.table_id).unwrap_or(e.table_id);
                *by_table.entry(name).or_default().entry(e.op).or_default() += 1;
            }
            "callerDependent" => caller_dependent += 1,
            "tableNotResolved" if op_family(e.op) == "write" => unresolved_writes += 1,
            _ => {}
        }
    }
    let mut ranked: Vec<(&str, BTreeMap<&str, u64>)> = by_table.into_iter().collect();
    ranked.sort_by(|a, b| {
        let n = |m: &BTreeMap<&str, u64>| m.values().sum::<u64>();
        n(&b.1).cmp(&n(&a.1)).then(a.0.cmp(b.0))
    });
    let tables_written = ranked.len();
    // Round-2 G34: complete when 20 tables or fewer, else the first 10.
    let shown = if tables_written <= 20 {
        tables_written
    } else {
        10
    };
    let top: Vec<Value> = ranked
        .iter()
        .take(shown)
        .map(|(t, ops)| json!({"table": t, "ops": ops}))
        .collect();
    crate::engine::result_index::with_index(
        json!({
            "kind": "query.effects",
            "routine": routine,
            "effectCount": effects.len(),
            "byOp": by_op,
            "byTempState": by_temp,
            "notKnownTemporaryWrites": physical_writes,
            "tableCount": table_set.len(),
            "unknownTable": unknown,
            "writes": {
                "byTable": top,
                "tables": tables_written,
                "truncated": tables_written > shown,
                "callerDependent": caller_dependent,
                "tableNotResolved": unresolved_writes,
            },
        }),
        "",
        "",
        vec![json!({"routine": routine, "effectCount": effects.len(),
                    "tablesWritten": tables_written})],
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
    // Output validation V4 (F76, 3.9.0, additive): `payload.result.scope`, the
    // header's facts as fields, beside query effects' `families` (V2).
    let scope = query_scope_json(kind, &payload, deps);
    if let Value::Object(m) = &mut payload {
        m.insert("dependencies".into(), deps.to_json());
        m.insert("lineBase".into(), LINE_BASE.into());
        let result = m
            .entry("result")
            .or_insert_with(|| json!({}))
            .as_object_mut()
            .map(|r| r.entry("scope").or_insert_with(|| json!({})));
        if let Some(Value::Object(s)) = result
            && let Value::Object(add) = scope
        {
            for (k, v) in add {
                s.insert(k, v);
            }
        }
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
    // §9.8 (F78): every location says what it is.
    let mut env = Value::Object(env);
    crate::engine::result_header::tag_location_roles(&mut env);
    serialize_document_value(env)
}

/// `payload.result.scope` of a query document (F76): `analysis`
/// (`notAnalyzed` for a selector failure or a refused question, `partial`
/// for a `down` answer of `unknown`), `reasons`, `assumptions`, `limits`,
/// `universe` and the `scope` clause the human header prints.
fn query_scope_json(kind: &str, payload: &Value, deps: &LoadedDependencies) -> Value {
    use crate::engine::result_header::{Analysis, Header, dependencies_clause};
    let failed = payload.get("error").is_some()
        || payload.as_object().is_some_and(|m| {
            m.values().any(|v| {
                v.get("resolution")
                    .and_then(Value::as_str)
                    .is_some_and(|r| r != "resolved")
            })
        });
    let mut h = Header::new("query", String::new(), !deps.cross_app);
    if failed {
        h.analysis = Analysis::NotAnalyzed("the input did not resolve".to_string());
    } else if payload.pointer("/down/answer").and_then(Value::as_str) == Some("unknown") {
        h.analysis = Analysis::Partial(vec!["the answer names what it could not resolve".into()]);
    }
    let depth = if kind == "query.effects" {
        "reads, locks, validate: one dependency routine deep, not exhaustive beyond; writes \
         beyond dependency calls: whole cone with literal arguments"
    } else {
        "per operation, one routine into dependency code"
    };
    h.scope = format!("{depth}; {}", dependencies_clause(deps.apps.len()));
    // The families' blockers decide `analysis` too (`Header::scope_json`).
    let mut extra = Map::new();
    if let Some(f) = payload.pointer("/result/scope/families") {
        extra.insert("families".into(), f.clone());
    }
    h.scope_json(extra)
}

/// r3 §5.1 (A P1): a query effects answer's status for its envelope summary
/// and index row, from its `result.scope` (`scope`): `analysis`, `reasons`
/// and `scopes`, the operations' family (`writes`: one dependency routine
/// deep) and the deep writes' (`beyondDependencyCalls`: the whole cone)
/// apart, each with its depth and inventory.
fn effects_status(scope: &Value) -> Value {
    let family = |k: &str| {
        let f = &scope["families"][k];
        json!({"depth": f["depth"], "inventory": f["inventory"]})
    };
    json!({
        "analysis": scope["analysis"],
        "reasons": scope["reasons"],
        "scopes": {
            "operations": family("writes"),
            "deepWrites": family("beyondDependencyCalls"),
        },
    })
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
    selector_failure_doc_with(
        kind,
        selector_field,
        selector,
        resolution,
        deps,
        alsem_ver,
        deterministic,
        serde_json::Map::new(),
        None,
    )
}

/// A routine selector's failure document (V3 §2.4): an unmatched one with
/// the nearest workspace names (or where the name is found only in
/// dependency code) and the selector forms; an ambiguous one with every
/// candidate's complete parameter list and a ready `routines: [...]` value.
#[allow(clippy::too_many_arguments)]
fn routine_failure_doc(
    kind: &str,
    selector_field: &str,
    selector: &str,
    resolution: &Resolution<&ModelRoutine>,
    model: &Model,
    deps: &LoadedDependencies,
    alsem_ver: &str,
    deterministic: bool,
) -> QueryRunResult {
    use crate::engine::l5::selector_index as si;
    let names = crate::engine::l5::routine_rollup::RoutineNames::new(model, &[]);
    let (extra, human) = match resolution {
        Resolution::Ambiguous(labels) => {
            // Each label ends `(stable id <id>)` (`candidate_label`).
            let listed: Vec<si::AmbiguousCandidate> = labels
                .iter()
                .filter_map(|l| {
                    let id = l.rsplit_once("(stable id ")?.1.strip_suffix(')')?;
                    si::AmbiguousCandidate::of(&names, id)
                })
                .collect();
            if listed.len() != labels.len() {
                (serde_json::Map::new(), None)
            } else {
                // Round-2 G40: in source order (file, then line).
                let mut listed = listed;
                listed.sort_by_key(|c| {
                    let (file, line) = c.at.rsplit_once(':').unwrap_or((c.at.as_str(), ""));
                    (file.to_string(), line.parse::<u32>().unwrap_or(0))
                });
                let mut m = serde_json::Map::new();
                m.insert(
                    "candidateSelectors".into(),
                    listed.iter().map(|c| c.selector.clone()).collect(),
                );
                let lines =
                    si::ambiguity_lines(&format!("{kind}: {selector_field} "), selector, &listed);
                (m, Some(format!("{}\n", lines.join("\n"))))
            }
        }
        _ => {
            let owners = OwnerIndex::build(model);
            let hints = si::selector_hints(&names, |id| owners.object_key(id), selector);
            let mut lines = vec![crate::engine::l5::explain::unmatched_line(
                &format!("{kind}: {selector_field}: "),
                selector,
            )];
            lines.extend(
                si::hint_lines(&hints, &deps.apps)
                    .into_iter()
                    .map(|l| format!("  {l}")),
            );
            (
                si::hints_json(&hints),
                Some(format!("{}\n", lines.join("\n"))),
            )
        }
    };
    selector_failure_doc_with(
        kind,
        selector_field,
        selector,
        resolution,
        deps,
        alsem_ver,
        deterministic,
        extra,
        human,
    )
}

#[allow(clippy::too_many_arguments)]
fn selector_failure_doc_with<T>(
    kind: &str,
    selector_field: &str,
    selector: &str,
    resolution: &Resolution<T>,
    deps: &LoadedDependencies,
    alsem_ver: &str,
    deterministic: bool,
    extra: serde_json::Map<String, Value>,
    human_text: Option<String>,
) -> QueryRunResult {
    let candidates: Vec<String> = match resolution {
        Resolution::Ambiguous(c) => c.clone(),
        _ => Vec::new(),
    };
    let mut block = json!({
        "selector": selector,
        "resolution": resolution.label(),
        "candidates": candidates,
    });
    if let Value::Object(m) = &mut block {
        m.extend(extra);
    }
    let payload = json!({ selector_field: block });
    let human = if let Some(h) = human_text {
        h
    } else if candidates.is_empty() {
        format!("{kind}: {selector_field} {selector:?} matched no {selector_field}\n")
    } else {
        let mut s = format!("{kind}: {selector_field} {selector:?} is AMBIGUOUS:\n");
        for c in &candidates {
            s.push_str(&format!("  - {c}\n"));
        }
        if selector_field != "table" {
            s.push_str("  pass one as <owner>::<routine>(<parameter types>) or its stable id\n");
        }
        s
    };
    // §6.3 (round-2 n08): the outcome from the resolution, not the words.
    let summary = crate::engine::result_header::outcome_of_kinds([resolution.label()])
        .map_or(Value::Null, |o| json!({"kind": kind, "outcome": o}));
    QueryRunResult {
        json_text: envelope(kind, payload, deps, alsem_ver, deterministic),
        human_text: human,
        exit_code: 2,
        summary,
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
#[allow(clippy::too_many_arguments)]
pub fn render_query_touches(
    substrate: &QuerySubstrate,
    table_selector: &str,
    from: Option<&str>,
    direction: Direction,
    all: bool,
    callers: bool,
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
            callers,
            false,
            alsem_ver,
            deterministic,
            None,
            UpThrough::NotRequested,
            Surface::Cli,
            !substrate.dependencies.cross_app,
        ),
        &substrate.dependencies,
    )
}

/// What `up.throughDependency` reads (design r2 §3.2, §9).
#[derive(Clone, Copy)]
enum UpThrough<'g> {
    /// A library entry that declared no sited graph: no block.
    NotRequested,
    /// Single-app, or no dependency loaded: there is no dependency code
    /// (`notApplicable`).
    NoDependencyCode,
    /// Declared, and the graph did not build (`notComputed`).
    NotBuilt,
    /// The cross-app sited graph R0 walks.
    Graph(&'g crate::program::impact_graph::SitedGraph),
}

/// Human output names the loaded dependency apps on its first line, only when
/// there are some (a workspace with no dependency reads the same in both
/// modes). An answer with a `scope:` line states them there instead (W2-2,
/// design §4.5 Q6: `dependencies_clause`), so it gets no first line.
fn with_dependencies_line(r: QueryRunResult, deps: &LoadedDependencies) -> QueryRunResult {
    let hc = QueryHeaderCx {
        surface: Surface::Cli,
        single_app: !deps.cross_app,
        effects: r.json_text.contains("\"kind\": \"query.effects\""),
        all: false,
        callers: false,
        up: false,
        asked: String::new(),
    };
    finish_human(r, deps, None, &hc)
}

/// What a query answer's header needs beyond its own lines (output
/// validation §5.2).
pub(crate) struct QueryHeaderCx {
    pub surface: Surface,
    pub single_app: bool,
    /// `query effects` (else `query touches`).
    pub effects: bool,
    /// `--all`.
    pub all: bool,
    /// `--callers`.
    pub callers: bool,
    /// Asked up (or both) from a routine.
    pub up: bool,
    /// The inputs as given, for an answer with no title (a selector failure).
    pub asked: String,
}

impl QueryHeaderCx {
    fn of(ask: &QueryAsk<'_>, surface: Surface, single_app: bool) -> QueryHeaderCx {
        match *ask {
            QueryAsk::Effects { routine, all } => QueryHeaderCx {
                surface,
                single_app,
                effects: true,
                all,
                callers: false,
                up: false,
                asked: routine.to_string(),
            },
            QueryAsk::Touches {
                table,
                from,
                direction,
                all,
                callers,
                field,
                ..
            } => QueryHeaderCx {
                surface,
                single_app,
                effects: false,
                all,
                callers,
                up: from.is_some() && direction.wants_up() && field.is_none(),
                asked: format!(
                    "{table}{}{}",
                    field.map(|f| format!(" field {f}")).unwrap_or_default(),
                    from.map(|f| format!(" from {f} {}", direction.as_str()))
                        .unwrap_or_default()
                ),
            },
        }
    }
}

/// A query answer as the four header lines and its body (output validation
/// §5.2): the renderers print a `--- … ---` title, their result line
/// (`summary:`, `down:`, `up:`, `transitively:`, `consumers:`) and a
/// `scope:` line; the title is the header's inputs, the first result line
/// its answer, the scope line its scope clause. Every other line stays, in
/// order. An answer with no title (a selector failure) is `no answer`.
fn query_with_header(
    text: &str,
    deps: usize,
    exit: u8,
    hc: &QueryHeaderCx,
    families: Option<&Value>,
) -> String {
    use crate::engine::result_header::{Analysis, Header, dependencies_clause};
    let tool = if hc.effects {
        "query effects"
    } else {
        "query touches"
    };
    let lines: Vec<&str> = text.lines().collect();
    let title = lines
        .first()
        .and_then(|l| l.strip_prefix("--- "))
        .and_then(|l| l.strip_suffix(" ---"));
    let mut h = Header::new(tool, String::new(), hc.single_app);
    let mut body: Vec<String> = Vec::new();
    let mut scope: Vec<String> = Vec::new();
    match title {
        None => {
            h.inputs = hc.asked.clone();
            h.answer = format!(
                "no answer — {}",
                lines.first().map_or("", |l| l.trim()).trim_end_matches(':')
            );
            h.analysis = Analysis::NotAnalyzed(if exit == 0 {
                "nothing to analyze".to_string()
            } else {
                "the input did not resolve".to_string()
            });
            body.extend(lines.iter().skip(1).map(|l| (*l).to_string()));
        }
        Some(t) => {
            let t = t
                .strip_prefix("effects of ")
                .or_else(|| t.strip_prefix("touches "))
                .unwrap_or(t);
            h.inputs = t.to_string();
            let result_labels: &[&str] = if hc.effects {
                &["  summary: "]
            } else {
                &[
                    "  down: ",
                    "  up: ",
                    "  workspace sites: ",
                    "  consumers: ",
                    "  not answerable: ",
                ]
            };
            let mut answered = false;
            for (i, l) in lines.iter().enumerate().skip(1) {
                if let Some(s) = l.strip_prefix("  scope: ") {
                    scope.push(s.to_string());
                    continue;
                }
                if !answered && let Some(label) = result_labels.iter().find(|p| l.starts_with(**p))
                {
                    answered = true;
                    let rest = &l[label.len()..];
                    h.answer = if hc.effects {
                        rest.to_string()
                    } else {
                        format!("{}: {rest}", label.trim().trim_end_matches(':'))
                    };
                    // A result line with detail lines under it stays, as
                    // its label only (round-2 G25: one copy of the line).
                    let next_deeper = lines
                        .get(i + 1)
                        .is_some_and(|n| n.starts_with("    ") && !n.starts_with("    ["));
                    if !next_deeper {
                        continue;
                    }
                    body.push(format!("{} (as in answer):", label.trim_end_matches(": ")));
                    continue;
                }
                body.push((*l).to_string());
            }
            // An `unknown` verdict (`down: unknown — …`), not a count of
            // unknown callers.
            if h.answer.split(" — ").next().is_some_and(|a| {
                a.ends_with(": unknown") || a.starts_with("unknown") || a.contains(": unknown (")
            }) {
                h.analysis =
                    Analysis::Partial(vec!["the answer names what it could not resolve".into()]);
            }
            if let Some(f) = families {
                h.analysis = std::mem::replace(&mut h.analysis, Analysis::Complete)
                    .and(Analysis::of_families(f));
                // Round 2 G14: the limits the body states (`locks: none
                // observed (one dependency routine deep; …)`) are the
                // header's limits too.
                h.limits.extend(family_limits(f));
            }
        }
    }
    scope.push(dependencies_clause(deps));
    h.scope = scope.join("; ");
    let s = hc.surface;
    if title.is_some() && !hc.all {
        h.more.push(if hc.effects {
            format!("{} lists every operation", Input::All.on(s))
        } else {
            format!("{} lists every witness, no fold", Input::All.on(s))
        });
    }
    if title.is_some() && hc.up && !hc.callers && !hc.all {
        h.more
            .push(format!("{} names every caller", Input::Callers.on(s)));
    }
    h.more
        .push(format!("{} for every row", Input::FormatJson.on(s)));
    while body.last().is_some_and(|l| l.is_empty()) {
        body.pop();
    }
    // Round-2 G40: a lookup miss is its failure sentence once, then what
    // helps the next call.
    let mut out = if title.is_none() && exit == 2 {
        h.miss_human()
    } else {
        h.human()
    };
    for l in body {
        out.push_str(&l);
        out.push('\n');
    }
    out
}

/// The header limits a query effects document's families state (round 2
/// G14): the families inventoried one dependency routine deep and not
/// exhaustively, as one limit (`reads, locks and validations one dependency
/// routine deep`). The writes family counts only when no whole-cone
/// `beyondDependencyCalls` family follows the writes past it.
fn family_limits(families: &Value) -> Vec<String> {
    let whole_cone_writes = families.get("beyondDependencyCalls").is_some();
    let words: Vec<&str> = [
        ("reads", "reads"),
        ("locks", "locks"),
        ("validate", "validations"),
        ("writes", "writes"),
    ]
    .into_iter()
    .filter(|(k, _)| !(*k == "writes" && whole_cone_writes))
    .filter(|(k, _)| {
        families[*k]["depth"] == "oneDependencyRoutine"
            && families[*k]["inventory"] == "notExhaustive"
    })
    .map(|(_, w)| w)
    .collect();
    match words.as_slice() {
        [] => Vec::new(),
        [one] => vec![format!("{one} one dependency routine deep")],
        [init @ .., last] => vec![format!(
            "{} and {last} one dependency routine deep",
            init.join(", ")
        )],
    }
}

/// [`query_with_header`], then (W2-7, 2g) `--show-source` excerpts
/// while the locations still name their app by guid, then readable
/// dependency locations (the JSON keeps the guid).
fn finish_human(
    mut r: QueryRunResult,
    deps: &LoadedDependencies,
    sources: Option<&crate::engine::source_excerpt::SourceTexts>,
    hc: &QueryHeaderCx,
) -> QueryRunResult {
    // Round-2 astra M1: the header's analysis is the document's
    // (`payload.result.scope`, which counts the families' blockers).
    let families = if hc.effects {
        serde_json::from_str::<Value>(&r.json_text)
            .ok()
            .and_then(|d| d.pointer("/payload/result/scope/families").cloned())
    } else {
        None
    };
    r.human_text = query_with_header(
        &r.human_text,
        deps.apps.len(),
        r.exit_code,
        hc,
        families.as_ref(),
    );
    // Round-2 G40: an ambiguous routine selector offers the complete calls,
    // one per candidate, in this surface's words.
    if r.exit_code == 2
        && let Ok(doc) = serde_json::from_str::<Value>(&r.json_text)
    {
        fn find<'v>(v: &'v Value, key: &str) -> Option<&'v Value> {
            match v {
                Value::Object(m) => m.get(key).or_else(|| m.values().find_map(|x| find(x, key))),
                Value::Array(a) => a.iter().find_map(|x| find(x, key)),
                _ => None,
            }
        }
        if let Some(Value::Array(sels)) = find(&doc, "candidateSelectors") {
            use crate::engine::follow_up::{Cx, Tool};
            let sels: Vec<String> = sels
                .iter()
                .filter_map(|s| s.as_str().map(str::to_string))
                .collect();
            let (tool, args) = if hc.effects {
                (Tool::QueryEffects, Vec::new())
            } else {
                let table = doc
                    .pointer("/payload/table/selector")
                    .cloned()
                    .unwrap_or(Value::Null);
                (Tool::QueryTouches, vec![("table", table)])
            };
            let args: Vec<(&str, Value)> = args.into_iter().filter(|(_, v)| !v.is_null()).collect();
            for l in crate::engine::l5::selector_index::complete_call_lines(
                tool,
                Cx::new(hc.surface, hc.single_app),
                &sels,
                &args,
            ) {
                r.human_text.push_str(&l);
                r.human_text.push('\n');
            }
        }
    }
    if let Some(s) = sources {
        r.human_text = crate::engine::source_excerpt::with_excerpts(&r.human_text, s);
    }
    r.human_text = deps.readable_locations(r.human_text);
    r
}

#[allow(clippy::too_many_arguments)]
fn render_query_touches_body(
    substrate: &QuerySubstrate,
    table_selector: &str,
    from: Option<&str>,
    direction: Direction,
    all: bool,
    callers: bool,
    writes_only: bool,
    alsem_ver: &str,
    deterministic: bool,
    cone: Option<&crate::engine::l5::digest_cli::DigestEntryFull>,
    through: UpThrough<'_>,
    surface: Surface,
    single_app: bool,
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
                return routine_failure_doc(
                    "query.touches",
                    "from",
                    sel,
                    &other,
                    substrate.resolved(),
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
        // Round-2 G41: the table through r1 §2.1's formatter.
        Some(n) => format!("{} ({})", object_display("Table", n), table.id),
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
            // Step 1b: the depth the list stands on, in both forms.
            let unseen = if is_unknown_table(&table.id) {
                ""
            } else {
                ", or on a table not resolved,"
            };
            payload.insert(
                "up".into(),
                json!({
                    "scoped": false,
                    "routineCount": rows.len(),
                    "routines": rows,
                    "note": format!(
                        "workspace-global: every workspace routine whose transitive cone touches \
                         this table, by the per-operation index (one routine into dependency \
                         code). A routine whose only touch is deeper in dependency code{unseen} \
                         is not listed. Scope it with --from <routine> --direction up."
                    ),
                    // 3.2.0, additive.
                    "index": "operations-one-routine-deep",
                }),
            );
            human.push_str(&format!("--- touches {table_label} ---\n"));
            // Output validation V2 (§4.6, F23): the routine count is
            // transitive; the sites that touch the table directly are counted
            // apart, workspace and dependency sites apart, and listed (a listed
            // line is a site, never a routine header).
            let resolved = substrate.resolved();
            let names = crate::engine::l5::routine_rollup::RoutineNames::new(resolved, &[]);
            let unknown = is_unknown_table(&table.id);
            // Output validation §9.3 (F59): the sites by operation and by
            // object, and the key-relevant ones. The table's key fields, as
            // `field_touches` compares names.
            let key_fields: HashSet<String> = resolved
                .workspace
                .tables
                .iter()
                .filter(|t| t.id == table.id)
                .flat_map(|t| {
                    t.keys.iter().flat_map(move |k| {
                        k.fields.iter().filter_map(move |f| {
                            t.field_name(f).map(super::field_touches::field_key)
                        })
                    })
                })
                .collect();
            let mut ws_sites: Vec<TableSite> = Vec::new();
            let mut ws_routines: BTreeSet<&str> = BTreeSet::new();
            let mut dep_by_op: BTreeMap<String, usize> = BTreeMap::new();
            let mut dep_routines: BTreeSet<&str> = BTreeSet::new();
            for r in &resolved.workspace.routines {
                let primary = is_primary_routine(resolved, r);
                for o in &r.record_operations {
                    let hit = match o.table_id.as_deref() {
                        Some(t) => t == table.id || (unknown && is_unknown_table(t)),
                        None => unknown,
                    };
                    if !hit {
                        continue;
                    }
                    if primary {
                        ws_routines.insert(r.stable_routine_id.as_str());
                        ws_sites.push(TableSite {
                            file: o.source_anchor.source_unit_id.clone(),
                            line: o.source_anchor.start_line + 1,
                            op: o.op.clone(),
                            who: names.display(&r.stable_routine_id),
                            object: join.of(r).object_display,
                            key: key_relevance(
                                &o.op,
                                o.field_arguments
                                    .as_ref()
                                    .and_then(|a| a.first())
                                    .map(String::as_str),
                                &key_fields,
                            ),
                            unresolved: if unknown {
                                unresolved_table_reason(resolved, r, o)
                            } else {
                                String::new()
                            },
                            temp: o
                                .temp_state
                                .as_ref()
                                .map_or(TempStateKind::Unknown, TempStateKind::from_p_temp_state),
                        });
                    } else {
                        dep_routines.insert(r.stable_routine_id.as_str());
                        *dep_by_op.entry(o.op.clone()).or_default() += 1;
                    }
                }
            }
            ws_sites.sort_by(|a, b| (&a.file, a.line, &a.op).cmp(&(&b.file, b.line, &b.op)));
            let dep_sites: usize = dep_by_op.values().sum();
            let mut ws_by_op: BTreeMap<&str, usize> = BTreeMap::new();
            let mut by_object: BTreeMap<&str, usize> = BTreeMap::new();
            let mut by_key: BTreeMap<&str, usize> = BTreeMap::new();
            for s in &ws_sites {
                *ws_by_op.entry(s.op.as_str()).or_default() += 1;
                *by_object.entry(s.object.as_str()).or_default() += 1;
                if let Some(k) = s.key {
                    *by_key.entry(k).or_default() += 1;
                }
            }
            let ranked = |m: &BTreeMap<&str, usize>| -> Vec<(String, usize)> {
                let mut v: Vec<(String, usize)> =
                    m.iter().map(|(k, n)| ((*k).to_string(), *n)).collect();
                v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
                v
            };
            let dep_ranked = ranked(&dep_by_op.iter().map(|(k, n)| (k.as_str(), *n)).collect());
            let by_op_text = |v: &[(String, usize)]| -> String {
                v.iter()
                    .map(|(k, n)| format!("{k} {}", thousands(*n)))
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            if let Some(up) = payload.get_mut("up") {
                up["directSites"] = json!({
                    "workspaceSites": ws_sites.len(),
                    "workspaceRoutines": ws_routines.len(),
                    "dependencySites": dep_sites,
                    // §9.3 (F59, additive): by operation, by object, and the
                    // key-relevant sites (not exhaustive).
                    "dependencyRoutines": dep_routines.len(),
                    "workspaceByOp": ws_by_op,
                    "dependencyByOp": dep_by_op,
                    "workspaceByObject": by_object,
                    "keyRelevant": by_key,
                });
            }
            human.push_str(&format!(
                "  workspace sites: {} in {}{}\n",
                thousands(ws_sites.len()),
                plural(ws_routines.len(), "routine", "routines"),
                if ws_sites.is_empty() {
                    String::new()
                } else {
                    format!(" — by op: {}", by_op_text(&ranked(&ws_by_op)))
                }
            ));
            human.push_str(&format!(
                "  dependency sites: {} in {} the analysis loads (those the workspace reaches, and \
                 the callers of dependency events it subscribes to){}\n",
                thousands(dep_sites),
                plural(
                    dep_routines.len(),
                    "dependency routine",
                    "dependency routines"
                ),
                if dep_sites == 0 {
                    String::new()
                } else {
                    format!(", by op: {}", by_op_text(&dep_ranked))
                }
            ));
            if unknown {
                human.push_str(
                    "  table not resolved: each site's table is decided by a value the analysis \
                     does not resolve, and an unresolved write can be any table\n",
                );
            }
            if !by_object.is_empty() {
                let objects = ranked(&by_object);
                let shown_o = fold_len(objects.len(), TOUCHES_LIST_CAP, all);
                human.push_str(&format!(
                    "  by object: {}{}\n",
                    objects[..shown_o]
                        .iter()
                        .enumerate()
                        .map(|(i, (o, n))| if i == 0 {
                            format!("{o} {}", plural(*n, "site", "sites"))
                        } else {
                            format!("{o} {}", thousands(*n))
                        })
                        .collect::<Vec<_>>()
                        .join(", "),
                    if shown_o < objects.len() {
                        format!(
                            ", … +{} — {} lists them",
                            plural(objects.len() - shown_o, "object", "objects"),
                            Input::All.on(surface)
                        )
                    } else {
                        String::new()
                    }
                ));
            }
            human.push_str(&format!(
                "  transitive: {} one of the workspace or dependency sites (per operation, one \
                 routine into dependency code: a routine whose only touch is deeper in dependency \
                 code{unseen} is not counted)\n",
                plural(
                    routines.len(),
                    "workspace routine reaches",
                    "workspace routines reach"
                ),
            ));
            // Round 2 G8: the sites by family, record writes first, each
            // family with its own count and its share of the samples. `all`
            // lists every site; `opFamily: "write"` every record write.
            let every_site =
                all || fold_len(ws_sites.len(), GLOBAL_LIST_CAP, false) == ws_sites.len();
            let samples: Vec<&TableSite> = if every_site && !writes_only {
                let mut v: Vec<&TableSite> = ws_sites.iter().collect();
                v.sort_by_key(|s| s.family());
                v
            } else if writes_only {
                // r3 §7 R6: the write-only listing leads with the writes the
                // task asks about: persistent first, Insert before Modify
                // before Delete.
                let mut v: Vec<&TableSite> = ws_sites
                    .iter()
                    .filter(|s| s.family() == SiteFamily::Write)
                    .collect();
                v.sort_by(|a, b| a.write_order().cmp(&b.write_order()));
                v
            } else {
                family_samples(&ws_sites)
            };
            let mut family_json = Map::new();
            for f in SiteFamily::ALL {
                let of: Vec<&TableSite> = ws_sites.iter().filter(|s| s.family() == f).collect();
                let mut by: BTreeMap<&str, usize> = BTreeMap::new();
                for s in &of {
                    *by.entry(s.op.as_str()).or_default() += 1;
                }
                let routines_of: BTreeSet<&str> = of.iter().map(|s| s.who.as_str()).collect();
                let temporary = of.iter().filter(|s| s.is_temporary()).count();
                let mut fj = json!({
                    "sites": of.len(),
                    "routines": routines_of.len(),
                    "byOp": by,
                });
                if f == SiteFamily::Write {
                    fj["temporary"] = temporary.into();
                }
                family_json.insert(f.key().into(), fj);
                if ws_sites.is_empty() {
                    continue;
                }
                let by_text = by_op_text(&ranked(&by));
                let header = match f {
                    SiteFamily::Write if of.is_empty() => {
                        "  record writes: none observed\n".to_string()
                    }
                    // r3 §7 R6: the write-only listing's heading states the
                    // temp-state split its order follows.
                    SiteFamily::Write if writes_only => format!(
                        "  record writes: {} in {} — {}; by op: {}\n",
                        plural(of.len(), "site", "sites"),
                        plural(routines_of.len(), "routine", "routines"),
                        WriteClass::ALL
                            .iter()
                            .map(|c| format!(
                                "{} {}",
                                c.words(),
                                thousands(
                                    of.iter().filter(|s| WriteClass::of(&s.temp) == *c).count()
                                )
                            ))
                            .collect::<Vec<_>>()
                            .join(", "),
                        by_text,
                    ),
                    SiteFamily::Write => format!(
                        "  record writes: {} in {} — {} (temporary records: {})\n",
                        plural(of.len(), "site", "sites"),
                        plural(routines_of.len(), "routine", "routines"),
                        by_text,
                        thousands(temporary)
                    ),
                    SiteFamily::Validate if of.is_empty() => String::new(),
                    SiteFamily::Validate => format!(
                        "  validations: {} (a validation may lead to a write; it is not one)\n",
                        plural(of.len(), "site", "sites")
                    ),
                    SiteFamily::Lock if of.is_empty() => "  locks: none observed\n".to_string(),
                    SiteFamily::Lock => format!(
                        "  locks: {} — {by_text}\n",
                        plural(of.len(), "site", "sites")
                    ),
                    SiteFamily::Key => {
                        let parts: Vec<String> = KEY_USES
                            .iter()
                            .filter(|k| !(unknown && **k == KEY_FILTER))
                            .map(|k| {
                                let n = by_key.get(k).copied().unwrap_or(0);
                                match *k {
                                    KEY_GET => format!("{k} {} (primary key)", thousands(n)),
                                    _ => format!("{k} {}", thousands(n)),
                                }
                            })
                            .collect();
                        // Round 2 G22: the table's keys first, and what the
                        // key-relevant count does not search.
                        let keys = if unknown {
                            String::new()
                        } else {
                            table_keys_line(resolved, &table.id)
                        };
                        format!(
                            "{keys}  key-relevant (not exhaustive): {}; not searched: key and \
                             ordering uses in declarations (page SourceTableView, report \
                             DataItemTableView)\n",
                            parts.join(", ")
                        )
                    }
                    SiteFamily::Read | SiteFamily::Other if of.is_empty() => String::new(),
                    // r3 §5.2 (P7): `Get` is counted under key-relevant
                    // uses, so this family is the other reads.
                    SiteFamily::Read => format!(
                        "  other reads: {} — {by_text}\n",
                        plural(of.len(), "site", "sites")
                    ),
                    SiteFamily::Other => format!(
                        "  other operations: {} — {by_text}\n",
                        plural(of.len(), "site", "sites")
                    ),
                };
                human.push_str(&header);
                for s in samples.iter().filter(|s| s.family() == f) {
                    let table_note = if unknown {
                        format!(" — table: not resolved ({})", s.unresolved)
                    } else {
                        String::new()
                    };
                    let temp = if f == SiteFamily::Write {
                        temp_note(&s.temp)
                    } else {
                        String::new()
                    };
                    human.push_str(&format!(
                        "    [{}] at {}:{} in {}{temp}{table_note}\n",
                        s.op, s.file, s.line, s.who
                    ));
                }
                // r3 §5.1 (A P3): when the record writes are folded, the
                // complete write-only call for this table, ready to send.
                let shown = samples.iter().filter(|s| s.family() == f).count();
                if f == SiteFamily::Write && !writes_only && shown < of.len() {
                    let call = crate::engine::follow_up::FollowUp::new(
                        crate::engine::follow_up::Tool::QueryTouches,
                        crate::engine::follow_up::Cx::new(surface, single_app),
                        "every record-write site",
                    )
                    .arg("table", table_selector)
                    .arg("opFamily", "write");
                    human.push_str(&format!(
                        "    every record-write site ({}): {}\n",
                        thousands(of.len()),
                        call.text(surface)
                    ));
                }
            }
            if let Some(up) = payload.get_mut("up") {
                up["directSites"]["byFamily"] = Value::Object(family_json);
                if writes_only {
                    // Round 2 G8: `opFamily: "write"`, every record-write
                    // site with its own temp state.
                    up["opFamily"] = "write".into();
                    // r3 §7 R6 (additive): the sites by temp-state class,
                    // and `writeSites` in the human listing's order.
                    let mut by_class = Map::new();
                    for c in WriteClass::ALL {
                        let n = ws_sites
                            .iter()
                            .filter(|s| {
                                s.family() == SiteFamily::Write && WriteClass::of(&s.temp) == c
                            })
                            .count();
                        by_class.insert(c.key().into(), n.into());
                    }
                    up["writeSitesByTempState"] = Value::Object(by_class);
                    up["writeSites"] = samples
                        .iter()
                        .map(|s| {
                            json!({
                                "op": s.op,
                                "file": s.file,
                                "line": s.line,
                                "routine": s.who,
                                "tempState": temp_state_str(&s.temp),
                            })
                        })
                        .collect::<Vec<_>>()
                        .into();
                }
            }
            let listed = if writes_only {
                ws_sites
                    .iter()
                    .filter(|s| s.family() == SiteFamily::Write)
                    .count()
            } else {
                ws_sites.len()
            };
            if samples.len() < listed {
                human.push_str(&format!(
                    "    … +{} — {} lists them, and every routine\n",
                    plural(listed - samples.len(), "site", "sites"),
                    Input::All.on(surface)
                ));
            }
            let shown_n = if all { routines.len() } else { 0 };
            if all {
                human.push_str(&format!(
                    "  routines that touch it transitively ({}):\n",
                    routines.len()
                ));
            }
            for &ix in &routines[..shown_n] {
                let internal = query.bundle().routine_id(ix);
                match join.get(internal) {
                    // Round-2 G45: the location's role (its definition).
                    Some(rr) => human.push_str(&format!(
                        "    {} (defined at {}:{})\n",
                        rr.qualified(),
                        rr.file,
                        shown(rr.line)
                    )),
                    None => human.push_str(&format!("    {internal}\n")),
                }
            }
            if shown_n < routines.len() {
                human.push_str(&format!(
                    "  the {} that touch it transitively: {} lists them\n",
                    plural(routines.len(), "routine", "routines"),
                    Input::All.on(surface)
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
                    rr.header
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

            human.push_str(&format!("--- {} vs {table_label} ---\n", rr.header));
            // Round 2 G19: where the routine is (with `--show-source`, the
            // excerpt of its first statements follows).
            human.push_str(&format!("  defined at {}:{}\n", rr.file, shown(rr.line)));
            let beyond = cone.map(|e| Beyond::new(e, substrate.resolved(), deps));
            // Round-2 G27: the dependency-write clause only when the cone
            // enters dependency code.
            let enters = beyond.as_ref().is_none_or(|b| {
                b.entry.unresolved_traversal.dependency_routines > 0
                    || b.rollup.totals.entry_points > 0
                    || b.rollup.totals.crossing_sites > 0
            });
            human.push_str(match (&beyond, enters) {
                (Some(_), true) => TOUCHES_SCOPE_LINE_CONE,
                (Some(_), false) => "  scope: dependency code: none reached\n",
                (None, _) => TOUCHES_SCOPE_LINE,
            });

            let cx = TouchCx {
                query: &query,
                join: &join,
                ops: &ops,
                hops: FirstHops::new(&query),
                resolved: substrate.resolved(),
                surface,
                single_app,
                follow: (from.unwrap_or(""), table_selector),
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
                    callers,
                    through,
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

/// A caller row's distance with its unit (output validation §8.2, F53):
/// `1 hop`, `3 hops`.
fn hops_unit(n: usize) -> String {
    plural(n, "hop", "hops")
}

/// One operation on the asked table in a workspace routine's own body
/// (table mode, output validation §9.3).
struct TableSite {
    file: String,
    line: u32,
    op: String,
    who: String,
    object: String,
    key: Option<&'static str>,
    /// Round 2 G21: when the table is not resolved, the receiver and why
    /// ([`unresolved_table_reason`]).
    unresolved: String,
    /// The operation's own temp state (the body walk's, not a caller's).
    temp: TempStateKind,
}

/// Round 2 G22: the `keys:` line of table mode — the primary key and the
/// other declared keys by their field names, or nothing when the table
/// declares none in the loaded program.
///
/// Every declared key counts (round-3 n03: a key on a system field,
/// Customer's `SystemModifiedAt`, was dropped, so "+13" hid one of 18). The
/// keys are the real table's own (a `tableextension` stub sharing the id is
/// another object, G-5) and, after them, the keys its extensions add, each
/// named by its extension (output validation r3 L1): an extension's key is
/// never the primary key.
fn table_keys_line(model: &Model, table_id: &str) -> String {
    let quoted = |n: &str| format!("\"{}\"", n.replace('"', "\"\""));
    // With no base table loaded, no primary key is named.
    let Some(t) = model
        .workspace
        .tables
        .iter()
        .find(|t| t.id == table_id && !t.is_extension_stub)
    else {
        return String::new();
    };
    let key_text = |k: &crate::program::model::workspace::ModelKey| {
        let names: Vec<String> = k
            .fields
            .iter()
            .filter_map(|f| t.field_name(f).map(quoted))
            .collect();
        let text = if names.is_empty() {
            "fields not resolved".to_string()
        } else {
            names.join(", ")
        };
        match &k.declared_by {
            Some(ext) => format!("{text}; from tableextension {}", quoted(ext)),
            None => text,
        }
    };
    // The table's own keys first (its first is the primary key), then the
    // extensions'.
    let own: Vec<String> = t
        .keys
        .iter()
        .filter(|k| k.declared_by.is_none())
        .map(key_text)
        .collect();
    let added = t
        .keys
        .iter()
        .filter(|k| k.declared_by.is_some())
        .map(key_text);
    let (head, rest): (String, Vec<String>) = match own.split_first() {
        Some((pk, own_rest)) => (
            format!("primary key {pk}"),
            own_rest.iter().cloned().chain(added).collect(),
        ),
        // No own key in the loaded program: the extensions' keys alone,
        // never one of them as the primary key.
        None => {
            let rest: Vec<String> = added.collect();
            if rest.is_empty() {
                return String::new();
            }
            (
                "primary key not declared in the loaded program".to_string(),
                rest,
            )
        }
    };
    const SHOWN: usize = 4;
    let mut line = format!("  keys: {head}");
    if !rest.is_empty() {
        let shown: Vec<String> = rest.iter().take(SHOWN).map(|k| format!("({k})")).collect();
        line.push_str(&format!("; other keys: {}", shown.join(", ")));
        if rest.len() > SHOWN {
            line.push_str(&format!(", … +{}", rest.len() - SHOWN));
        }
    }
    line.push('\n');
    line
}

/// Round 2 G21 (r1 §8.1's target-resolution words): why a record
/// operation's table is not resolved, as `<receiver kind> <name>: <reason>`.
/// A `RecordRef`'s reason names the origins that reach the operation
/// ([`super::recref_origin`], round-3 c18): opened from a value the analysis
/// does not resolve, set by `GetTable`, copied from another RecordRef (a
/// global named with the routines that set it), passed by the caller, or not
/// opened in the routine; a `Record` names a table the loaded program does
/// not hold.
fn unresolved_table_reason(
    model: &Model,
    r: &crate::program::model::workspace::ModelRoutine,
    o: &crate::program::model::workspace::ModelRecordOperation,
) -> String {
    if !o.receiver_is_record_ref {
        let v = o.record_variable_name.as_str();
        return format!("Record {v}: its table is not in the loaded program");
    }
    super::recref_origin::recref_reason(model, r, o)
}

impl TableSite {
    /// The site's family in table mode (round 2 G8): record writes,
    /// validations, locks, key-relevant uses, reads, other operations.
    fn family(&self) -> SiteFamily {
        match op_family(&self.op) {
            "write" => SiteFamily::Write,
            "validate" => SiteFamily::Validate,
            "lock" => SiteFamily::Lock,
            _ if self.key.is_some() => SiteFamily::Key,
            "read" => SiteFamily::Read,
            _ => SiteFamily::Other,
        }
    }

    /// Whether the site is on a record known to be temporary.
    fn is_temporary(&self) -> bool {
        self.temp == TempStateKind::Known(true)
    }

    /// Output validation r3 §7 R6: the record-write listing's order key
    /// under `opFamily: "write"` — the temp-state class (persistent,
    /// caller-dependent, temp state not resolved, temporary), then the
    /// operation (Insert before Modify before Delete), then file and line.
    fn write_order(&self) -> (WriteClass, u8, &str, u32) {
        let op = match self.op.as_str() {
            "Insert" => 0,
            "Modify" => 1,
            "ModifyAll" => 2,
            "Rename" => 3,
            "Delete" => 4,
            "DeleteAll" => 5,
            _ => 6,
        };
        (WriteClass::of(&self.temp), op, &self.file, self.line)
    }
}

/// Output validation r3 §7 R6: a record write's temp-state class, in the
/// order a write listing prints them: the persistent writes first (the ones
/// a new mandatory field or a changed rule breaks), then the ones the caller
/// decides, then the ones whose temp state is not resolved (never folded
/// into either side), then the temporary ones.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum WriteClass {
    Persistent,
    CallerDependent,
    NotResolved,
    Temporary,
}

impl WriteClass {
    const ALL: [WriteClass; 4] = [
        WriteClass::Persistent,
        WriteClass::CallerDependent,
        WriteClass::NotResolved,
        WriteClass::Temporary,
    ];

    fn of(t: &TempStateKind) -> WriteClass {
        match t {
            TempStateKind::Known(false) => WriteClass::Persistent,
            TempStateKind::ParameterDependent(_) => WriteClass::CallerDependent,
            TempStateKind::Unknown => WriteClass::NotResolved,
            TempStateKind::Known(true) => WriteClass::Temporary,
        }
    }

    /// The heading's word for the class.
    fn words(self) -> &'static str {
        match self {
            WriteClass::Persistent => "persistent",
            WriteClass::CallerDependent => "caller-dependent",
            WriteClass::NotResolved => "not resolved",
            WriteClass::Temporary => "temporary",
        }
    }

    /// The JSON key of the class's count.
    fn key(self) -> &'static str {
        match self {
            WriteClass::Persistent => "persistent",
            WriteClass::CallerDependent => "callerDependent",
            WriteClass::NotResolved => "notResolved",
            WriteClass::Temporary => "temporary",
        }
    }
}

/// A table-mode site family (round 2 G8), in the order the answer prints
/// them: record writes first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum SiteFamily {
    Write,
    Validate,
    Lock,
    Key,
    Read,
    Other,
}

impl SiteFamily {
    const ALL: [SiteFamily; 6] = [
        SiteFamily::Write,
        SiteFamily::Validate,
        SiteFamily::Lock,
        SiteFamily::Key,
        SiteFamily::Read,
        SiteFamily::Other,
    ];

    /// The JSON key of the family's counts.
    fn key(self) -> &'static str {
        match self {
            SiteFamily::Write => "recordWrites",
            SiteFamily::Validate => "validations",
            SiteFamily::Lock => "locks",
            SiteFamily::Key => "keyRelevant",
            SiteFamily::Read => "reads",
            SiteFamily::Other => "other",
        }
    }
}

/// One family's sites with their operation kinds taken in turn (round 2
/// G8), so a family's samples show each kind before a second site of any:
/// the kinds by count (largest first, then name), each kind's sites in file
/// and line order.
fn interleave_ops<'s>(sites: &[&'s TableSite]) -> Vec<&'s TableSite> {
    let mut by_op: BTreeMap<&str, Vec<&'s TableSite>> = BTreeMap::new();
    for s in sites {
        by_op.entry(s.op.as_str()).or_default().push(s);
    }
    let mut ops: Vec<Vec<&'s TableSite>> = by_op.into_values().collect();
    ops.sort_by(|a, b| b.len().cmp(&a.len()).then(a[0].op.cmp(&b[0].op)));
    let mut out = Vec::with_capacity(sites.len());
    let mut round = 0;
    while out.len() < sites.len() {
        for v in &ops {
            if let Some(s) = v.get(round) {
                out.push(*s);
            }
        }
        round += 1;
    }
    out
}

/// How many of table mode's default samples ([`GLOBAL_LIST_CAP`]) may be
/// record writes (round 2 G8).
const WRITE_SAMPLES: usize = 6;

/// The default samples of table mode (round 2 G8): up to
/// [`GLOBAL_LIST_CAP`] sites shared by family, so every family present is
/// represented. Record writes take up to [`WRITE_SAMPLES`] first; the other
/// families then take one site each in their order, round after round,
/// until the budget is spent. Within the key-relevant family a key-field
/// `SetRange`/`SetFilter` comes before a `SetCurrentKey`, and both before a
/// `Get`. Each family's sites keep their file and line order. The result is
/// in family order.
fn family_samples(sites: &[TableSite]) -> Vec<&TableSite> {
    let key_rank = |s: &TableSite| match s.key {
        Some(KEY_FILTER) => 0,
        Some(KEY_CURRENT) => 1,
        _ => 2,
    };
    let mut by_family: BTreeMap<SiteFamily, Vec<&TableSite>> = BTreeMap::new();
    for s in sites {
        by_family.entry(s.family()).or_default().push(s);
    }
    for (f, v) in by_family.iter_mut() {
        if *f == SiteFamily::Key {
            v.sort_by_key(|s| key_rank(s));
        } else {
            *v = interleave_ops(v);
        }
    }
    let mut taken: BTreeMap<SiteFamily, usize> = BTreeMap::new();
    let writes = by_family
        .get(&SiteFamily::Write)
        .map_or(0, |v| v.len().min(WRITE_SAMPLES));
    taken.insert(SiteFamily::Write, writes);
    let mut budget = GLOBAL_LIST_CAP - writes;
    loop {
        let mut progressed = false;
        for f in &SiteFamily::ALL[1..] {
            if budget == 0 {
                break;
            }
            let have = by_family.get(f).map_or(0, Vec::len);
            let t = taken.entry(*f).or_default();
            if *t < have {
                *t += 1;
                budget -= 1;
                progressed = true;
            }
        }
        if budget == 0 || !progressed {
            break;
        }
    }
    let mut out = Vec::new();
    for f in SiteFamily::ALL {
        let n = taken.get(&f).copied().unwrap_or(0);
        if let Some(v) = by_family.get(&f) {
            out.extend(v.iter().take(n).copied());
        }
    }
    out
}

/// The key-relevant uses table mode counts (§9.3, F59; not exhaustive).
const KEY_GET: &str = "Get";
const KEY_CURRENT: &str = "SetCurrentKey";
const KEY_FILTER: &str = "SetRange/SetFilter on key fields";
const KEY_USES: [&str; 3] = [KEY_GET, KEY_CURRENT, KEY_FILTER];

/// Why a site is key-relevant, or `None`: a `Get` (it reads by the primary
/// key), a `SetCurrentKey`, or a `SetRange`/`SetFilter` whose field (the
/// first argument) is in one of the table's declared keys (`key_fields`,
/// [`super::field_touches::field_key`] spellings).
fn key_relevance(
    op: &str,
    first_arg: Option<&str>,
    key_fields: &HashSet<String>,
) -> Option<&'static str> {
    match op.to_ascii_lowercase().as_str() {
        "get" => Some(KEY_GET),
        "setcurrentkey" => Some(KEY_CURRENT),
        "setrange" | "setfilter"
            if first_arg
                .is_some_and(|a| key_fields.contains(&super::field_touches::field_key(a))) =>
        {
            Some(KEY_FILTER)
        }
        _ => None,
    }
}

/// Round 2 G62: what a query effects answer with no operation is about.
const ZERO_OPS_SCOPE: &str =
    "record operations only; not page-context restrictions or non-record effects";

/// The scope line every routine-scoped touches answer prints after its title.
const TOUCHES_SCOPE_LINE: &str = "  scope: per operation, one routine into dependency code\n";

/// [`TOUCHES_SCOPE_LINE`] when the capability cone was read (wave 3a): the
/// writes beyond each dependency entry point follow the whole cone.
const TOUCHES_SCOPE_LINE_CONE: &str = "  scope: dependency code: full cone for physical writes, \
     grouped per dependency routine entered; per-operation detail one routine deep\n";

/// The default human list cap for touches (spec §3.8 rule 4's shape: the cap,
/// or every item when there are at most a quarter more). `--all` lists all.
const TOUCHES_LIST_CAP: usize = 8;
/// The workspace-global list's cap (tables' 12, all when 15 or fewer).
const GLOBAL_LIST_CAP: usize = 12;
/// How many owner routines the temporary-records line names.
const TEMP_OWNERS_SHOWN: usize = 4;

/// The shared fold rule (design §3.8 rule 4): one helper for every output.
use crate::engine::l5::routine_rollup::{fold_len, thousands};

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
        TempStateKind::Unknown => " (temp state not resolved)".to_string(),
        TempStateKind::ParameterDependent(i) if *i == RECEIVER_PARAM_INDEX => {
            " (caller-dependent: Rec)".to_string()
        }
        TempStateKind::ParameterDependent(i) => {
            format!(" (caller-dependent: parameter {})", i + 1)
        }
    }
}

/// The words `query effects` uses for a caller-dependent temp state of the
/// root's own parameter (F16): `caller-dependent: parameter 3
/// (ToPurchaseHeader)`, 1-based like [`temp_note`], with the parameter's name
/// when `params` (the root's declared parameters) has it; `caller-dependent:
/// Rec` for the receiver. `None` for any other state.
fn caller_dependent_words(
    t: &TempStateKind,
    params: &[crate::program::model::workspace::ModelParameter],
) -> Option<String> {
    match t {
        TempStateKind::ParameterDependent(i) if *i == RECEIVER_PARAM_INDEX => {
            Some("caller-dependent: Rec".to_string())
        }
        TempStateKind::ParameterDependent(i) => {
            let name = params
                .iter()
                .find(|p| p.index == *i)
                .map(|p| format!(" ({})", p.name))
                .unwrap_or_default();
            Some(format!("caller-dependent: parameter {}{name}", i + 1))
        }
        _ => None,
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

/// How a caller reaches a touch, in words ([`via_note`]'s route).
fn reach_words(v: crate::engine::l4::effect_store::ViaRank) -> &'static str {
    use crate::engine::l4::effect_store::ViaRank;
    match v {
        ViaRank::Direct | ViaRank::Inherited => "through plain calls",
        ViaRank::Dynamic => "through dynamic dispatch",
        ViaRank::EventSubscriber => "through an event subscriber",
        ViaRank::ImplicitTrigger => "through a table trigger",
    }
}

/// What the touches renderers share: the query and the two joins.
struct TouchCx<'q, 'a> {
    query: &'q DbEffectQuery<'a>,
    join: &'q RoutineJoin<'a>,
    ops: &'q OperationJoin<'a>,
    /// W2-2: `through <first hop>` by the shared §4.5 rule.
    hops: FirstHops<'q, 'a>,
    /// The model the operations belong to (whose routines are the
    /// workspace's, for the absence reasons).
    resolved: &'q Model,
    /// `--surface` and `--single-app` (output validation §6.1).
    surface: Surface,
    single_app: bool,
    /// The routine (`--from`, as given) and the table name, for a complete
    /// follow-up call.
    follow: (&'q str, &'q str),
}

impl TouchCx<'_, '_> {
    fn fcx(&self) -> crate::engine::follow_up::Cx {
        crate::engine::follow_up::Cx::new(self.surface, self.single_app)
    }
}

impl<'a> TouchCx<'_, 'a> {
    /// `[Op] <family> at <file>:<line> in <owner><temp><via>`, then `, through
    /// <first hop>` on a row that is not the routine's own, when a direct
    /// callee holds its complete contribution (design §4.5).
    fn witness_line(&self, t: &TableTouch<'a>) -> String {
        let op = self.ops.get(t.operation_id, self.join);
        // §7.5 (F84): `through` only when it names a routine other than the
        // `in` one.
        let through = match t.via {
            crate::engine::l4::effect_store::ViaRank::Direct => String::new(),
            _ => match self.hops.of(t) {
                Ok(c) => {
                    let hop = self.routine_label(c);
                    if op.as_ref().is_some_and(|o| o.owner.qualified() == hop) {
                        String::new()
                    } else {
                        format!(", through {hop}")
                    }
                }
                Err(_) => String::new(),
            },
        };
        format!("{}{through}", self.touch_core(t))
    }

    /// [`Self::witness_line`] without its `through` clause: the touch
    /// itself, `[Op] <family> at <file>:<line> in <owner><temp><via>`.
    fn touch_core(&self, t: &TableTouch<'a>) -> String {
        format!("{}{}", self.touch_site(t), via_note(t.via))
    }

    /// The touch's site alone, `[Op] <family> at <file>:<line> in
    /// <owner><temp>`: [`Self::touch_core`] without how the caller reaches
    /// it (round-3 c17: two callers reaching one site differ only in the
    /// route, not in the touch).
    fn touch_site(&self, t: &TableTouch<'a>) -> String {
        let at = match self.ops.get(t.operation_id, self.join) {
            Some(o) => format!("at {}:{} in {}", o.file, shown(o.line), o.owner.qualified()),
            None => format!("(outside the workspace: {})", t.operation_id),
        };
        format!(
            "[{}] {} {at}{}",
            t.op,
            op_family(t.op),
            temp_note(t.temp_state)
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

    /// `--callers`' line: the caller and where it is declared.
    fn caller_line(&self, ix: crate::engine::l4::routine_interner::SolveIx) -> String {
        let internal = self.query.bundle().routine_id(ix);
        match self.join.get(internal) {
            // Round-2 G45: the location's role.
            Some(x) => format!(
                "{} (defined at {}:{})",
                x.qualified(),
                x.file,
                shown(x.line)
            ),
            None => internal.to_string(),
        }
    }
}

/// The kind of records a caller's touches are on (§7.1): one class when
/// they all are, else the counts.
fn class_words(c: &TouchCounts) -> String {
    match (c.known_persistent, c.possibly_persistent(), c.temporary) {
        (_, 0, 0) => "on persistent records".to_string(),
        (0, _, 0) => "on possibly persistent records".to_string(),
        (0, 0, _) => "on temporary records".to_string(),
        _ => counts_human(c),
    }
}

/// The touching callers of `query touches --direction up` (output
/// validation §7.1, F38): the first touch they share and their touch class
/// print once, then every caller once, by object (nearest object first),
/// with its definition line and hop distance; a caller prints its own first
/// touch or class only when it differs from the shared one. The first
/// `listed` callers; the caller folds the rest.
fn callers_by_object<'a>(
    cx: &TouchCx<'_, 'a>,
    touchers: &[&CallerTouch<'a>],
    listed: usize,
) -> String {
    let mut out = String::new();
    if touchers.is_empty() {
        return out;
    }
    // The first touch's site and class; its route apart (round-3 c17: the
    // site is what callers share or not, the route only how they reach it).
    let first = |c: &CallerTouch<'a>| {
        let w = c.witnesses.as_ref().expect("a touching caller");
        (cx.touch_site(&w.first), class_words(&w.counts))
    };
    let route = |c: &CallerTouch<'a>| {
        let w = c.witnesses.as_ref().expect("a touching caller");
        via_note(w.first.via)
    };
    // The most common first touch and class (ties: the nearest caller's).
    let most = |key: &dyn Fn(&CallerTouch<'a>) -> String| -> (String, usize) {
        let mut n: HashMap<String, (usize, usize)> = HashMap::new();
        for (i, c) in touchers.iter().enumerate() {
            let e = n.entry(key(c)).or_insert((0, i));
            e.0 += 1;
        }
        let (k, (count, _)) = n
            .into_iter()
            .max_by(|a, b| a.1.0.cmp(&b.1.0).then(b.1.1.cmp(&a.1.1)))
            .expect("non-empty");
        (k, count)
    };
    let (touch, with_touch) = most(&|c| first(c).0);
    let (class, _) = most(&|c| first(c).1);
    // The route most of the callers sharing the site take (ties: the
    // nearest caller's).
    let shared_route = {
        let mut n: Vec<(&str, usize)> = Vec::new();
        for c in touchers.iter().filter(|c| first(c).0 == touch) {
            match n.iter_mut().find(|(r, _)| *r == route(c)) {
                Some((_, k)) => *k += 1,
                None => n.push((route(c), 1)),
            }
        }
        n.iter()
            .rev()
            .max_by_key(|(_, k)| *k)
            .map_or("", |(r, _)| *r)
    };
    out.push_str(&format!(
        "    shared first touch ({} of {}): {touch}{shared_route}; touches {class}\n",
        with_touch,
        plural(touchers.len(), "caller", "callers"),
    ));
    // Round-2 G36: the listed callers whose first touch is another, in one
    // line (each with its own first touch), not inside the caller list.
    let name_of = |c: &CallerTouch<'a>| {
        let internal = cx.query.bundle().routine_id(c.routine);
        cx.join
            .get(internal)
            .map_or_else(|| internal.to_string(), |r| r.qualified())
    };
    let own: Vec<String> = touchers[..listed]
        .iter()
        .filter_map(|c| {
            let (own_touch, _) = first(c);
            (own_touch != touch)
                .then(|| format!("{} (first touch: {own_touch}{})", name_of(c), route(c)))
        })
        .collect();
    if !own.is_empty() {
        out.push_str(&format!(
            "    first touch not the shared one ({}): {}\n",
            plural(own.len(), "caller", "callers"),
            own.join("; ")
        ));
    }
    // Round-3 c17: a caller whose first touch is the shared one, reached
    // another way (an event subscriber, a table trigger), says so apart: the
    // touch is the same.
    let other_route: Vec<String> = touchers[..listed]
        .iter()
        .filter(|c| first(c).0 == touch && route(c) != shared_route)
        .map(|c| {
            let w = c.witnesses.as_ref().expect("a touching caller");
            format!("{} ({})", name_of(c), reach_words(w.first.via))
        })
        .collect();
    if !other_route.is_empty() {
        out.push_str(&format!(
            "    the shared first touch, reached another way ({}): {}\n",
            plural(other_route.len(), "caller", "callers"),
            other_route.join("; ")
        ));
    }
    // By object, in the order of each object's nearest caller.
    // (object display, file, its callers with their routine).
    type Group<'c, 't> = (String, String, Vec<(&'c CallerTouch<'t>, RoutineRef)>);
    let mut objects: Vec<Group<'_, '_>> = Vec::new();
    for c in &touchers[..listed] {
        let internal = cx.query.bundle().routine_id(c.routine);
        let Some(r) = cx.join.get(internal) else {
            objects.push((internal.to_string(), String::new(), Vec::new()));
            continue;
        };
        match objects
            .iter_mut()
            .find(|(o, f, _)| *o == r.object_display && *f == r.file)
        {
            Some((_, _, v)) => v.push((c, r)),
            None => objects.push((r.object_display.clone(), r.file.clone(), vec![(c, r)])),
        }
    }
    let routines = listed;
    out.push_str(&format!(
        "    callers ({} in {}), by object, nearest first; each object's callers by hop \
         distance, each with its definition line:\n",
        plural(routines, "routine", "routines"),
        plural(objects.len(), "object", "objects"),
    ));
    for (object, file, rows) in &objects {
        if rows.is_empty() {
            out.push_str(&format!("      {object}\n"));
            continue;
        }
        let head = rows[0].1.qualified();
        let object_words = head
            .strip_suffix(&rows[0].1.in_object)
            .and_then(|s| s.strip_suffix("::").or_else(|| s.strip_suffix('.')))
            .unwrap_or(object);
        out.push_str(&format!(
            "      {object_words} ({}) — {file}:\n",
            rows.len()
        ));
        let mut sorted: Vec<&(&CallerTouch<'_>, RoutineRef)> = rows.iter().collect();
        sorted.sort_by(|a, b| (a.0.depth, &a.1.in_object).cmp(&(b.0.depth, &b.1.in_object)));
        // Round-2 G36: one line per hop distance.
        let mut i = 0;
        while i < sorted.len() {
            let depth = sorted[i].0.depth;
            let mut names = Vec::new();
            while i < sorted.len() && sorted[i].0.depth == depth {
                let (c, r) = sorted[i];
                let (_, own_class) = first(c);
                names.push(format!(
                    "{} :{}{}",
                    r.in_object,
                    shown(r.line),
                    if own_class != class {
                        format!(" (touches {own_class})")
                    } else {
                        String::new()
                    }
                ));
                i += 1;
            }
            out.push_str(&format!(
                "        +{}: {}\n",
                hops_unit(depth as usize),
                names.join(", ")
            ));
        }
    }
    out
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

/// `a on persistent records, b on possibly persistent records (...), c on
/// temporary records`. The class is the RECORD's (its temp state), not the
/// operation's: a `CalcFields` read on a persistent record persists nothing
/// (fable #8, astra Task 4), so no class says the touch itself persists.
fn counts_human(c: &TouchCounts) -> String {
    let mut parts = vec![format!(
        "{} on {}",
        c.known_persistent,
        if c.known_persistent == 1 {
            "a persistent record"
        } else {
            "persistent records"
        }
    )];
    let pp = match (c.temp_state_unknown, c.caller_dependent) {
        (0, 0) => String::new(),
        (_, 0) => " (temp state not resolved)".to_string(),
        (0, _) => " (caller-dependent)".to_string(),
        (u, d) => format!(" (temp state not resolved {u}, caller-dependent {d})"),
    };
    parts.push(format!(
        "{} on {}{pp}",
        c.possibly_persistent(),
        if c.possibly_persistent() == 1 {
            "a possibly persistent record"
        } else {
            "possibly persistent records"
        }
    ));
    parts.push(format!("{} on temporary records", c.temporary));
    parts.join(", ")
}

/// Why a touches `down` that found no indexed operation on the table is not
/// a whole-cone "no" (astra Task 4, top 1). The per-operation index holds the
/// cone's operations one routine into dependency code; an operation on a
/// table not resolved may be this table, a call not resolved may reach it,
/// and code deeper in dependency code than one routine is followed only for
/// physical writes (the `beyond` section). Each is `(json, human)`.
fn absence_reasons(
    cone: Option<&Beyond<'_>>,
    not_resolved: usize,
    not_resolved_ref: usize,
    dependency_ops: usize,
    surface: Surface,
) -> Vec<(Value, String)> {
    let mut out = Vec::new();
    if not_resolved > 0 {
        let refs = if not_resolved_ref > 0 {
            format!(", {not_resolved_ref} of them on a RecordRef (table not resolved: RecordRef)")
        } else {
            String::new()
        };
        out.push((
            json!({"kind": "table-not-resolved", "operations": not_resolved,
                   "recordRef": not_resolved_ref}),
            format!(
                "{} on a table not resolved may be this table{refs} — {} lists them",
                plural(not_resolved, "operation", "operations"),
                Input::TableUnknown.on(surface)
            ),
        ));
    }
    let entry_points = cone.map_or(0, |b| b.rollup.totals.routines_entered);
    if entry_points > 0 || dependency_ops > 0 {
        let at = if entry_points > 0 {
            format!(
                " at {}",
                plural(
                    entry_points,
                    "dependency routine entered",
                    "dependency routines entered"
                )
            )
        } else {
            String::new()
        };
        out.push((
            json!({"kind": "dependency-depth", "entryPoints": entry_points,
                   "dependencyOperations": dependency_ops}),
            format!(
                "the cone enters dependency code{at}; operations there are indexed one routine \
                 deep, and deeper only physical writes are followed"
            ),
        ));
    }
    match cone {
        None => out.push((
            json!({"kind": "cone-not-read"}),
            "this answer did not read the whole cone: calls not resolved and code deeper than \
             one routine into dependency code were not checked"
                .to_string(),
        )),
        Some(b) => {
            let unresolved = b.entry.unresolved.len();
            if unresolved > 0 {
                out.push((
                    json!({"kind": "calls-not-resolved", "calls": unresolved}),
                    format!(
                        "{} in the cone not resolved may reach it — {} lists them",
                        plural(unresolved, "call", "calls"),
                        crate::engine::follow_up::Tool::Explain.on(surface)
                    ),
                ));
            }
            if unresolved == 0 && b.entry.coverage_status != "complete" {
                out.push((
                    json!({"kind": "coverage-incomplete", "status": b.entry.coverage_status,
                           "reasons": b.entry.coverage_reasons}),
                    format!(
                        "the cone's coverage is {} ({})",
                        b.entry.coverage_status,
                        b.entry.coverage_reasons.join(", ")
                    ),
                ));
            }
        }
    }
    out
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
    let beyond_n = beyond.as_array().map_or(0, Vec::len);
    // Step 1b (astra Task 4, top 1): an index miss is a whole-cone "no" only
    // when nothing leaves room for a touch the index cannot see. The table
    // bucket itself has no "may be this table" reason (its rows ARE them).
    let reasons = if touches_down {
        Vec::new()
    } else {
        let dependency_ops = query
            .bundle()
            .db_effects(rix)
            .filter(|e| cx.ops.in_dependency(e.operation_id, cx.resolved))
            .count();
        let (nr, nr_ref) = if is_unknown_table(table_id) {
            (0, 0)
        } else {
            (not_resolved, not_resolved_ref)
        };
        absence_reasons(cone, nr, nr_ref, dependency_ops, cx.surface)
    };
    let mut down = json!({
        "answerable": true,
        "touches": touches_down,
        "witnessCount": witnesses.len(),
        "counts": counts_v,
        "beyond": beyond,
        "witnesses": witnesses.iter().map(|t| cx.witness_json(t)).collect::<Vec<_>>(),
    });
    if !touches_down {
        // 3.2.0, additive: what the miss means. `no` only with no reason;
        // `beyond` when the whole cone may write it past an entry point.
        down["absence"] = json!({
            "answer": if beyond_n > 0 { "beyond" } else if reasons.is_empty() { "no" } else { "unknown" },
            "index": "operations-one-routine-deep",
            "reasons": reasons.iter().map(|r| r.0.clone()).collect::<Vec<_>>(),
        });
    }
    payload.insert("down".into(), down);

    if !touches_down {
        human.push_str(&if beyond_n > 0 {
            "  down: no indexed touch (per operation, one routine into dependency code); the \
             whole cone may write it beyond dependency calls (below)\n"
                .to_string()
        } else if reasons.is_empty() {
            "  down: no (every candidate decided) — no operation in this routine's cone touches \
             the table (the cone stays in workspace code, every call resolved, no table left \
             unresolved)\n"
                .to_string()
        } else {
            "  down: unknown — no indexed touch observed (per operation, one routine into \
             dependency code), which is not proof of absence:\n"
                .to_string()
        });
        if beyond_n == 0 {
            for (_, text) in &reasons {
                human.push_str(&format!("    {text}\n"));
            }
        }
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
                "    … +{} on persistent or possibly persistent records — {} lists them\n",
                plural(listed.len() - shown_n, "touch", "touches"),
                Input::All.on(cx.surface)
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
                if all {
                    String::new()
                } else {
                    format!(" — {} lists them", Input::All.on(cx.surface))
                }
            ));
        }
    }
    // On a miss with no `beyond` answer the operations are a reason above.
    if not_resolved > 0 && !is_unknown_table(table_id) && (touches_down || beyond_n > 0) {
        let refs = if not_resolved_ref > 0 {
            format!(", {not_resolved_ref} of them on a RecordRef (table not resolved: RecordRef)")
        } else {
            String::new()
        };
        human.push_str(&format!(
            "  table not resolved: {} in this cone{refs} (they may be this table) — {} lists them\n",
            plural(not_resolved, "operation", "operations"),
            Input::TableUnknown.on(cx.surface)
        ));
    }
    if beyond_n > 0 {
        use crate::engine::l5::routine_rollup::routines_entered;
        let table_writes: Vec<_> = beyond_writes
            .iter()
            .filter(|w| w.tables.contains_key(table_id))
            .cloned()
            .collect();
        let points: Vec<String> = table_writes
            .iter()
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
                "; … +{} — {} lists them",
                plural(
                    routines_entered(&table_writes[shown_n..]),
                    "dependency routine entered",
                    "dependency routines entered"
                ),
                Input::All.on(cx.surface)
            ));
        }
        // §6.1: the complete prove call for the path.
        let prove = crate::engine::follow_up::FollowUp::prove(
            cx.fcx(),
            cx.follow.0,
            &format!("writes-table:{}", cx.follow.1),
            "gives the path",
        );
        human.push_str(&format!(
            "  beyond dependency calls (the whole cone, literal arguments followed): may write it \
             beyond {}: {text} — {} gives the path\n",
            plural(
                routines_entered(&table_writes),
                "dependency routine entered",
                "dependency routines entered"
            ),
            prove.text(cx.surface)
        ));
    }
}

/// Which callers `up` names: the code-call relation of the solve graph
/// (`program::impact_graph::reverse::is_code_call_site`), never R0's
/// may-execute walk, which `alsem tests` and impact read. On local code the two
/// differ only by what a user starts on a page a caller opens (CDO
/// `WaitForMailLimit`: 964 against 1,030); the solve graph also does not
/// follow a path through dependency code: those callers are listed apart, in
/// `up.throughDependency` ([`render_through_dependency`]). Pinned by
/// `reverse::caller_agreement_tests` and `dep_callers`' tests.
const UP_RELATION: &str = "code-calls";
const UP_RELATION_LINE: &str = "  callers: by code calls of workspace code (calls, object runs and \
     the triggers a run runs, record triggers, event dispatch); not what a user starts on a page a \
     caller opens (the tests and impact tools follow it); paths through dependency code are listed \
     apart, before the caller list\n";

/// `up`: the solve graph's callers ([`render_touches_up_solve`]), then the
/// callers through dependency code ([`render_through_dependency`]).
#[allow(clippy::too_many_arguments)]
fn render_touches_up(
    cx: &TouchCx<'_, '_>,
    rix: crate::engine::l4::routine_interner::SolveIx,
    table_id: &str,
    touches_down: bool,
    all: bool,
    list_callers: bool,
    through: UpThrough<'_>,
    payload: &mut Map<String, Value>,
    human: &mut String,
) {
    let mut solve = String::new();
    render_touches_up_solve(
        cx,
        rix,
        table_id,
        touches_down,
        all,
        list_callers,
        payload,
        &mut solve,
    );
    let mut thr = String::new();
    if let Some(Value::Object(up)) = payload.get_mut("up") {
        render_through_dependency(cx, rix, through, all, list_callers, up, &mut thr);
    }
    // Output validation V2 (§5.1, c17): the via-dependency and
    // via-configuration callers come before the caller list, which can be
    // hundreds of lines long.
    let at = solve.find("\n    ").map_or(solve.len(), |i| i + 1);
    human.push_str(&solve[..at]);
    human.push_str(&thr);
    human.push_str(&solve[at..]);
}

/// `up.throughDependency` (query schema 3.5.0; design r2 §3.2, §6, §9): the
/// workspace routines that reach the routine only by code calls through
/// dependency code, from R0's code-call walk over the cross-app sited graph
/// (`impact_graph::dep_callers`). Disjoint from `up.transitiveCallers` by
/// construction, and any solve-graph caller is removed before listing. Each
/// caller has its tier (`code`, or `configured`: reachable only through a
/// configured-dispatch site, listed after the others), its hop count and its
/// witness ending at this routine. Potential callers: a witness shows graph
/// reachability, not a feasible execution, and zero is not absence (open
/// sites, hubs and what a user starts are outside the walk).
///
/// Human: one count line (the configured callers' arms by frequency); the 3
/// nearest code callers by default; `--callers` every caller with a path of
/// at most [`dep_callers::HUMAN_HOP_FOLD`] hops and the 3 nearest code callers
/// whatever their length, the rest counted; `--all`
/// every caller and every hop. JSON always has every caller and every hop.
fn render_through_dependency(
    cx: &TouchCx<'_, '_>,
    rix: crate::engine::l4::routine_interner::SolveIx,
    through: UpThrough<'_>,
    all: bool,
    list_callers: bool,
    up: &mut Map<String, Value>,
    human: &mut String,
) {
    use crate::program::impact_graph::dep_callers::{self, DepCaller, DepTier};
    let mut block = Map::new();
    block.insert("relation".into(), dep_callers::RELATION.into());
    let not = |block: &mut Map<String, Value>, key: &str, why: &str| {
        block.insert(key.into(), why.into());
    };
    let g = match through {
        UpThrough::NotRequested => return,
        UpThrough::NoDependencyCode => {
            not(&mut block, "notApplicable", "no dependency code loaded");
            up.insert("throughDependency".into(), Value::Object(block));
            human.push_str(
                "  through dependency code: not applicable (no dependency code loaded)\n",
            );
            return;
        }
        UpThrough::NotBuilt => {
            not(&mut block, "notComputed", "the sited graph did not build");
            up.insert("throughDependency".into(), Value::Object(block));
            human.push_str(
                "  through dependency code: not computed (the sited graph did not build)\n",
            );
            return;
        }
        UpThrough::Graph(g) => g,
    };
    let query = cx.query;
    let stable_of = |ix: crate::engine::l4::routine_interner::SolveIx| {
        cx.join
            .routine(query.bundle().routine_id(ix))
            .map(|r| r.stable_routine_id.as_str())
    };
    let roots = stable_of(rix)
        .map(|s| g.nodes_with_stable_id(s))
        .unwrap_or_default();
    if roots.is_empty() {
        not(
            &mut block,
            "notComputed",
            "the routine has no node in the sited graph",
        );
        up.insert("throughDependency".into(), Value::Object(block));
        human.push_str("  through dependency code: not computed (the routine has no node in the sited graph)\n");
        return;
    }
    let answer = dep_callers::callers_through_dependency(
        g,
        &roots,
        &crate::program::impact_graph::reverse::WalkOptions::default(),
    );
    // Disjoint from `up.transitiveCallers`, enforced (design r2 §3.2). A
    // stated LIMIT: on the fixture this filter removes nothing (a break that
    // drops it stays green), because the walk's own split already agrees with
    // the solve graph (`dep_callers` tests, invariant 1); it is a guard for
    // the day that agreement breaks, not a tested contribution.
    let solve: std::collections::HashSet<&str> = query
        .ancestors(rix)
        .into_iter()
        .filter_map(|(_, a)| stable_of(a))
        .collect();
    let by_stable: HashMap<&str, &ModelRoutine> = cx
        .resolved
        .workspace
        .routines
        .iter()
        .map(|r| (r.stable_routine_id.as_str(), r))
        .collect();
    let name = |n: u32| -> String {
        g.stable_id(n)
            .and_then(|s| by_stable.get(s.as_str()).copied())
            .and_then(|r| cx.join.get(&r.id))
            .map_or_else(
                || {
                    crate::engine::l5::routine_rollup::graph_display(g, n, Some(cx.resolved))
                        .unwrap_or_else(|| g.display(n))
                },
                |x| x.qualified(),
            )
    };
    let mut listed: Vec<(&DepCaller, String)> = answer
        .callers
        .iter()
        .filter(|c| !solve.contains(c.stable_id.as_str()))
        .map(|c| (c, name(c.node)))
        .collect();
    listed.sort_by(|a, b| (a.0.tier, a.0.hops.len(), &a.1).cmp(&(b.0.tier, b.0.hops.len(), &b.1)));
    let code: Vec<&(&DepCaller, String)> = listed
        .iter()
        .filter(|c| c.0.tier == DepTier::Code)
        .collect();
    let configured: Vec<&DepCaller> = listed
        .iter()
        .filter(|c| c.0.tier == DepTier::Configured)
        .map(|c| c.0)
        .collect();
    block.insert("engine".into(), "reverse-walk".into());
    block.insert("follows".into(), dep_callers::FOLLOWS.into());
    block.insert("feasibility".into(), "notChecked".into());
    block.insert("callers".into(), code.len().into());
    block.insert("configured".into(), configured.len().into());
    block.insert("catalog".into(), g.catalog().json());
    block.insert(
        "limited".into(),
        answer
            .limited
            .map_or(Value::Null, |l| dep_callers::limit_str(l).into()),
    );
    block.insert(
        "list".into(),
        listed
            .iter()
            .map(|(c, label)| {
                let routine = by_stable
                    .get(c.stable_id.as_str())
                    .and_then(|r| query.routine_ix(&r.id))
                    .map_or_else(|| json!({ "display": label }), |ix| cx.routine_json(ix));
                let mut m = Map::new();
                m.insert("routine".into(), routine);
                m.extend(dep_callers::caller_json(g, c, &name));
                Value::Object(m)
            })
            .collect::<Vec<_>>()
            .into(),
    );
    up.insert("throughDependency".into(), Value::Object(block));

    // Human.
    let limited = answer.limited.map_or(String::new(), |l| {
        format!(
            "; the walk stopped at its {} cap, so this is a lower bound",
            dep_callers::limit_str(l)
        )
    });
    let mismatch = g.catalog().entries.iter().any(|s| {
        matches!(
            s,
            crate::program::impact_graph::configured_dispatch::Status::Mismatch { .. }
        )
    });
    if listed.is_empty() {
        human.push_str(&format!(
            "  through dependency code: none by code calls{limited} (open sites, hubs and what a \
             user starts are outside this list)\n"
        ));
    } else {
        let mut line = format!(
            "  through dependency code (potential callers by code calls, path feasibility not \
             checked): {} more workspace {} [via-dependency]",
            code.len(),
            if code.len() == 1 { "caller" } else { "callers" }
        );
        if !configured.is_empty() {
            let arms: Vec<String> = dep_callers::arm_counts(&configured)
                .into_iter()
                .take(3)
                .map(|(e, a, n)| format!("{e}: {a} {n}"))
                .collect();
            line.push_str(&format!(
                "; {} more only through configured dispatch [via-configuration] (candidates: \
                 the binding lives in data the analysis has not read), including custom \
                 responses ({})",
                configured.len(),
                arms.join(", ")
            ));
        }
        line.push_str(&limited);
        // §7.1: how these populations relate to the transitive callers
        // (disjoint by construction: a solve-graph caller is removed above).
        line.push_str(if configured.is_empty() {
            "; disjoint from the transitive callers"
        } else {
            // Round-2 G36: the three populations by name.
            "; the transitive callers, the callers through dependency code and those only \
             through configured dispatch are disjoint"
        });
        if !list_callers && !all {
            line.push_str(&format!(" — {} lists them", Input::Callers.on(cx.surface)));
        }
        human.push_str(&line);
        human.push('\n');
    }
    if !configured.is_empty() || mismatch {
        human.push_str(&format!("    {}\n", g.catalog().human()));
    }
    let row = |c: &DepCaller, label: &str| {
        format!(
            "    +{} {}{label}{}\n",
            hops_unit(c.hops.len()),
            if c.tier == DepTier::Configured {
                "[via-configuration] "
            } else {
                "[via-dependency] "
            },
            dep_callers::witness_human(c, &name, all)
        )
    };
    if all {
        for (c, label) in &listed {
            human.push_str(&row(c, label));
        }
    } else if list_callers {
        // Every path of at most HUMAN_HOP_FOLD hops, and always the nearest
        // 3 code callers (the default view's), so the fold never hides them.
        let nearest: std::collections::HashSet<&str> = code
            .iter()
            .take(3)
            .map(|c| c.0.stable_id.as_str())
            .collect();
        let (short, long): (Vec<_>, Vec<_>) = listed.iter().partition(|c| {
            c.0.hops.len() <= dep_callers::HUMAN_HOP_FOLD
                || nearest.contains(c.0.stable_id.as_str())
        });
        for (c, label) in short {
            human.push_str(&row(c, label));
        }
        if !long.is_empty() {
            human.push_str(&format!(
                "    … +{} with paths over {} hops — {} lists them\n",
                plural(long.len(), "caller", "callers"),
                dep_callers::HUMAN_HOP_FOLD,
                Input::All.on(cx.surface)
            ));
        }
    } else {
        let shown = code.len().min(3);
        for (c, label) in &code[..shown] {
            human.push_str(&row(c, label));
        }
        if shown < code.len() {
            human.push_str(&format!(
                "    … +{} — {} lists them\n",
                plural(code.len() - shown, "caller", "callers"),
                Input::Callers.on(cx.surface)
            ));
        }
    }
}

/// The solve graph's half of `up`: every transitive caller probed ("has a summary row?", then
/// "touches?"). When the routine touches the table itself (the trivial case)
/// no caller witness is built: one counted line. Otherwise each touching
/// caller's witnesses are streamed into its first witness and counts. `--all`
/// adds every caller witness (the full product, as before W2-3).
/// `list_callers` (`--callers`, step 1b): every caller named, with its depth
/// and location, without that product; in the trivial case JSON `callers[]`
/// then holds `{depth, routine, outcome}`.
#[allow(clippy::too_many_arguments)]
fn render_touches_up_solve(
    cx: &TouchCx<'_, '_>,
    rix: crate::engine::l4::routine_interner::SolveIx,
    table_id: &str,
    touches_down: bool,
    all: bool,
    list_callers: bool,
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
        "this routine itself touches the table, so its callers inherit the touch through this \
         routine (summaries are transitive-down): they consume it, and no caller adds an \
         operation of its own here; each caller is still probed, and one without a summary row \
         is unknown"
    } else {
        "the index holds no touch of the table in this routine's cone (one routine into \
         dependency code; not proof of absence); the touching callers reach it through other \
         branches"
    };
    let mut up = Map::new();
    up.insert("scoped".into(), true.into());
    // 3.4.0, additive: which caller relation `transitiveCallers` counts.
    up.insert("relation".into(), UP_RELATION.into());
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
    if touches_down && list_callers {
        up.insert(
            "callers".into(),
            callers
                .iter()
                .map(|c| {
                    json!({
                        "depth": c.depth,
                        "routine": cx.routine_json(c.routine),
                        "outcome": match c.outcome {
                            CallerOutcome::Touches => "touches",
                            CallerOutcome::DoesNotTouch => "doesNotTouch",
                            CallerOutcome::NoRow => "unknown",
                        },
                    })
                })
                .collect::<Vec<_>>()
                .into(),
        );
    }
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
    human.push_str(UP_RELATION_LINE);

    let names = |list: &[&CallerTouch<'_>]| -> String {
        let n = fold_len(list.len(), 3, all || list_callers);
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
        // Step 1b (astra Task 4): the callers are this touch's consumers
        // (they inherit it), not a dismissed non-finding; `--callers` names
        // them without the witness product `--all` builds.
        let (all_in, callers_in) = (Input::All.on(cx.surface), Input::Callers.on(cx.surface));
        let more = match (list_callers || all, all) {
            (true, true) => String::new(),
            (true, false) => format!(" — {all_in} lists their witnesses"),
            (false, _) => format!(" — {callers_in} lists them, {all_in} their witnesses"),
        };
        human.push_str(&format!(
            "  up: {}{who} inherit{} this touch through this routine: {} it, {}, {} unknown{anomalies}{more}\n",
            plural(callers.len(), "transitive caller", "transitive callers"),
            if callers.len() == 1 { "s" } else { "" },
            plural(touching, "touches", "touch"),
            plural(not_touching.len(), "does not", "do not"),
            unknown.len(),
        ));
        if list_callers && !all {
            for c in &callers {
                human.push_str(&format!(
                    "    +{} {}\n",
                    hops_unit(c.depth as usize),
                    cx.caller_line(c.routine)
                ));
            }
        }
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
        let shown_n = fold_len(touchers.len(), TOUCHES_LIST_CAP, all || list_callers);
        if all {
            for c in &touchers[..shown_n] {
                let w = c.witnesses.as_ref().expect("filtered");
                human.push_str(&format!(
                    "    +{} {}: {}: {}\n",
                    hops_unit(c.depth as usize),
                    cx.routine_label(c.routine),
                    plural(w.count, "touch", "touches"),
                    counts_human(&w.counts)
                ));
                for a in product.iter().filter(|a| a.touch.routine == c.routine) {
                    human.push_str(&format!("      {}\n", cx.witness_line(&a.touch)));
                }
            }
        } else {
            human.push_str(&callers_by_object(cx, &touchers, shown_n));
        }
        if shown_n < touchers.len() {
            human.push_str(&format!(
                "    … +{} — {} lists them\n",
                plural(touchers.len() - shown_n, "caller", "callers"),
                Input::Callers.on(cx.surface)
            ));
        }
    }
    if all && touches_down {
        for a in &product {
            human.push_str(&format!(
                "    +{} {} {}\n",
                hops_unit(a.depth as usize),
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
            Surface::Cli,
            !substrate.dependencies.cross_app,
        ),
        &substrate.dependencies,
    )
}

/// The exclusive bucket of one row (design §4.5, F10): table not resolved
/// first, then the write class of its temp state, then the op family.
///
/// The caller-dependent and temporary classes are split by op family exactly
/// as the physical class is (F15): only a write is a caller-dependent WRITE
/// or a temporary-record write; a caller-dependent or temporary `Get` is a
/// read, a `LockTable` a lock, and so on.
fn bucket(t: &TableTouch<'_>) -> &'static str {
    use crate::engine::l5::rollup::{WriteClass, classify};
    if is_unknown_table(t.table_id) {
        return "tableNotResolved";
    }
    let class = classify(&[temp_state_str(t.temp_state).as_str()]);
    match op_family(t.op) {
        "write" => match class {
            WriteClass::Temporary => "temporary",
            WriteClass::CallerDependent(_) => "callerDependent",
            WriteClass::Physical(_) => "writes",
        },
        "lock" => "locks",
        "validate" => "validate",
        "read" => "reads",
        _ => "unclassified",
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
#[allow(clippy::too_many_arguments)]
fn render_query_effects_body(
    substrate: &QuerySubstrate,
    routine_selector: &str,
    all: bool,
    alsem_ver: &str,
    deterministic: bool,
    cone: Option<&crate::engine::l5::digest_cli::DigestEntryFull>,
    surface: Surface,
    single_app: bool,
) -> QueryRunResult {
    // §6.1: a fold note in the surface's words.
    let all_note = if all {
        String::new()
    } else {
        format!(" — {} lists them", Input::All.on(surface))
    };
    let deps = &substrate.dependencies;
    let beyond = cone.map(|e| Beyond::new(e, substrate.resolved(), deps));
    let join = RoutineJoin::build(substrate.resolved());
    let routine =
        match resolve_routine_selector(substrate.resolved(), &join.declaring, routine_selector) {
            Resolution::Resolved(r) => r,
            other => {
                return routine_failure_doc(
                    "query.effects",
                    "routine",
                    routine_selector,
                    &other,
                    substrate.resolved(),
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
            rr.header
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

    // Step 1b: each row's route as the cone judges it after the literal pass
    // (`None`: not judged, or the cone was not read).
    let cone_routes = beyond.as_ref().map(|b| ConeRoutes::new(b.entry));
    let literal: Vec<Option<LiteralRoute<'_>>> = effects
        .iter()
        .map(|t| {
            let routes = cone_routes.as_ref()?;
            let hop = match t.via {
                crate::engine::l4::effect_store::ViaRank::Direct => None,
                _ => hops
                    .of(t)
                    .ok()
                    .and_then(|c| join.get(query.bundle().routine_id(c)))
                    .map(|r| r.stable_id),
            };
            let origin = ops.get(t.operation_id, &join);
            let anchor = origin.as_ref().map(|o| (o.file.as_str(), o.line, o.column));
            let write = (matches!(bucket(t), "writes" | "callerDependent")
                && !ops.in_dependency(t.operation_id, substrate.resolved()))
            .then(|| write_effect_type(t.op))
            .flatten()
            .map(|ty| (ty, t.table_id));
            routes.judge(t.operation_id, anchor, hop.as_deref(), write)
        })
        .collect();
    let route_display = |stable: &str| -> String {
        beyond
            .as_ref()
            .map_or_else(|| stable.to_string(), |b| b.rcx.qualified(stable))
    };
    let literal_json = |r: LiteralRoute<'_>| -> Value {
        match r {
            LiteralRoute::RuledOut => json!({"status": "ruledOut"}),
            LiteralRoute::ConeRoute => json!({"status": "coneRoute"}),
            LiteralRoute::OtherRoute(s) => json!({
                "status": "otherRoute",
                "coneFirstHop": {"display": route_display(s), "stableId": s},
            }),
            LiteralRoute::NotInCone => json!({"status": "notInCone"}),
        }
    };
    // Both mean no route runs under the root's literal arguments.
    let ruled_out_row = |r: &Option<LiteralRoute<'_>>| {
        matches!(r, Some(LiteralRoute::RuledOut | LiteralRoute::NotInCone))
    };

    // Rows: every field unchanged, plus `firstHop` (or why none) on a row
    // that is not the routine's own (ConTinAI reads `via == "direct"`), and
    // (step 1b, additive) `literalRoute` on a row the cone judges.
    let rows: Vec<Value> = effects
        .iter()
        .zip(&literal)
        .map(|(t, lit)| {
            let mut v = touch_json(
                t,
                ops.get(t.operation_id, &join).as_ref(),
                Some(tables.get(t.table_id)),
            );
            // F16 (3.6.0, additive): a caller-dependent row's parameter,
            // 1-based, and its name (`tempState` keeps the 0-based index).
            if let (TempStateKind::ParameterDependent(i), Value::Object(m)) = (t.temp_state, &mut v)
                && *i != RECEIVER_PARAM_INDEX
            {
                m.insert("parameterPosition".into(), (u64::from(*i) + 1).into());
                m.insert(
                    "parameterName".into(),
                    routine
                        .parameters
                        .iter()
                        .find(|p| p.index == *i)
                        .map_or(Value::Null, |p| p.name.clone().into()),
                );
            }
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
                                    let mut h = json!({
                                        "display": x.display,
                                        "objectDisplay": x.object_display,
                                        "stableId": x.stable_id,
                                    });
                                    // Round-2 G41 (additive): every routine
                                    // row carries its guaranteed selector.
                                    if let Some(s) = x.identity.get("selector") {
                                        h["selector"] = s.clone();
                                    }
                                    h
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
            if let (Some(r), Value::Object(m)) = (lit, &mut v) {
                m.insert("literalRoute".into(), literal_json(*r));
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
            // Step 1b, additive: the first row's route and the table's rows
            // the literal arguments rule out.
            if let Some(r) = literal[rows[0]] {
                v["first"]["literalRoute"] = literal_json(r);
            }
            let ruled_out = rows.iter().filter(|&&i| ruled_out_row(&literal[i])).count();
            if cone_routes.is_some() {
                v["literalRuledOut"] = ruled_out.into();
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
            // Round-2 c11 (additive): the distinct routines they enter.
            "routinesEntered": crate::engine::l5::routine_rollup::routines_entered(&beyond_writes),
            "sites": beyond_writes.iter().map(|w| w.sites.len().max(1)).sum::<usize>(),
            "tablesOnlyBeyond": beyond_tables.difference(&own_write_tables).count(),
        },
        "beyondEntryPoints": beyond_entry_points,
        "scope": scope_value(beyond.is_some()),
    });
    // Step 1b, additive: how many rows each route judgement marks.
    let mut summary = summary;
    if cone_routes.is_some() {
        let n = |want: fn(&LiteralRoute<'_>) -> bool| {
            literal.iter().flatten().filter(|r| want(r)).count()
        };
        summary["literalRoutes"] = json!({
            "ruledOut": n(|r| *r == LiteralRoute::RuledOut),
            "notInCone": n(|r| *r == LiteralRoute::NotInCone),
            "otherRoute": n(|r| matches!(r, LiteralRoute::OtherRoute(_))),
            "coneRoute": n(|r| *r == LiteralRoute::ConeRoute),
            "notJudged": literal.iter().filter(|r| r.is_none()).count(),
        });
    }

    // Output validation V2 (§5.2, F61; query 3.8.0, additive): the scope of
    // each operation family as a structured object, readable without the
    // human text or the MCP envelope.
    let (call_not_resolved, open_world) = beyond.as_ref().map_or((0, 0), |b| {
        let open = b
            .entry
            .unresolved
            .iter()
            .filter(|u| u.open_world == Some(true))
            .count();
        (b.entry.unresolved.len() - open, open)
    });
    // Engine limits B8: the operations are inventoried one dependency
    // routine deep, which limits them only when the cone enters dependency
    // code. When it enters none (the cone was read, its search was not cut
    // short, and no operation is dependency code's), every family is the
    // whole cone, and only the calls not resolved and open-world dispatches
    // block it.
    let enters_dependency = enters_dependency_code(beyond.as_ref(), &effects, |op| {
        ops.in_dependency(op, substrate.resolved())
    });
    let family_scope = |what: &str| {
        if enters_dependency {
            json!({
                "depth": "oneDependencyRoutine",
                "inventory": "notExhaustive",
                "reasons": [format!("dependency code deeper than one routine is not inventoried for {what}")],
                "blockers": {"callNotResolved": call_not_resolved, "interfaceDispatchOpenWorld": open_world},
            })
        } else {
            let blocked = call_not_resolved + open_world > 0;
            json!({
                "depth": "wholeCone",
                "inventory": if blocked { "notExhaustive" } else { "exhaustive" },
                "reasons": if blocked {
                    json!([format!("calls not resolved and open-world interface dispatches may reach {what} the cone does not list")])
                } else {
                    json!([])
                },
                "blockers": {"callNotResolved": call_not_resolved, "interfaceDispatchOpenWorld": open_world},
            })
        }
    };
    let mut families = serde_json::Map::new();
    families.insert("reads".into(), family_scope("reads"));
    families.insert("locks".into(), family_scope("locks"));
    families.insert("validate".into(), family_scope("validations"));
    families.insert("writes".into(), family_scope("writes"));
    if beyond.is_some() {
        families.insert(
            "beyondDependencyCalls".into(),
            json!({
                "depth": "wholeCone",
                "inventory": if call_not_resolved + open_world == 0 { "exhaustive" } else { "notExhaustive" },
                "reasons": if call_not_resolved + open_world == 0 {
                    json!([])
                } else {
                    json!(["calls not resolved and open-world interface dispatches may reach writes the cone does not list"])
                },
                "blockers": {"callNotResolved": call_not_resolved, "interfaceDispatchOpenWorld": open_world},
            }),
        );
    }
    let payload = json!({
        "routine": {
            "selector": routine_selector,
            "resolution": "resolved",
            "routine": routine_ref_json(&rr),
        },
        "effectCount": effects.len(),
        "summary": summary,
        "effects": rows,
        "result": {"scope": {"families": families}},
    });
    // Round 2 G62 (query 4.x, additive): a zero-operation answer states its
    // scope in the JSON too.
    let mut payload = payload;
    if effects.is_empty() {
        payload["result"]["scope"]["covers"] = ZERO_OPS_SCOPE.into();
    }

    // Human (design §4.5): summary first, then the buckets; every row with
    // --all.
    let mut human = format!("--- effects of {} ---\n", rr.header);
    // Round 2 G19: where the routine is, and the route to its first write
    // (each hop the first direct callee holding the write's row).
    human.push_str(&format!("  defined at {}:{}\n", rr.file, shown(rr.line)));
    if let Some(first) = effects
        .iter()
        .find(|t| matches!(bucket(t), "writes" | "callerDependent"))
    {
        let mut route: Vec<String> = vec![routine.name.clone()];
        let mut cur = *first;
        for _ in 0..32 {
            if cur.via == crate::engine::l4::effect_store::ViaRank::Direct {
                break;
            }
            let Ok(c) = hops.of(&cur) else { break };
            route.push(hop_label(c));
            let Some(next) = query.all_effects(c).into_iter().find(|t| {
                (t.operation_id, t.table_id, t.op) == (cur.operation_id, cur.table_id, cur.op)
            }) else {
                break;
            };
            cur = next;
        }
        let at = match ops.get(first.operation_id, &join) {
            Some(o) => format!(" at {}:{}", o.file, shown(o.line)),
            None => String::new(),
        };
        human.push_str(&format!(
            "  route to the first write: {} → {}{at}\n",
            route.join(" → "),
            first.op
        ));
    }
    // Output validation V2 (§4.1, F14): a caller-dependent write is a write
    // risk; with no may-persist write the summary leads with it and its
    // condition, then the absence of any other write.
    let conditional: Vec<String> = by_table("callerDependent")
        .iter()
        .map(|(id, rows)| {
            let first = &effects[rows[0]];
            let at = match ops.get(first.operation_id, &join) {
                Some(o) => format!("{}:{}", o.file, shown(o.line)),
                None => "outside the workspace".to_string(),
            };
            let kinds: BTreeSet<&str> = rows.iter().map(|&i| effects[i].op).collect();
            let name = tables.get(id).unwrap_or(id);
            let condition = match first.temp_state {
                TempStateKind::ParameterDependent(i) if *i == RECEIVER_PARAM_INDEX => {
                    "when Rec is not temporary (Rec is temporary only when its caller's record \
                     is)"
                    .to_string()
                }
                TempStateKind::ParameterDependent(i) => {
                    let pname = routine
                        .parameters
                        .iter()
                        .find(|p| p.index == *i)
                        .map(|p| format!(", {}", p.name))
                        .unwrap_or_default();
                    format!(
                        "when the record its caller passes (parameter {}{pname}) is not temporary",
                        i + 1
                    )
                }
                _ => "when the record its caller passes is not temporary".to_string(),
            };
            // Round-2 G39: the operation's own verb.
            format!(
                "may {} persistent {name} records ({} at {at}) {condition}",
                crate::engine::vocab::write_verb(kinds.iter().copied()),
                kinds.iter().copied().collect::<Vec<_>>().join(", ")
            )
        })
        .collect();
    // Engine limits B8: a cone that enters no dependency code was searched
    // whole; only the calls it could not follow are left out.
    let whole_cone_search = crate::engine::vocab::none_observed(
        "the whole cone",
        if call_not_resolved + open_world > 0 {
            "calls not resolved"
        } else {
            ""
        },
    );
    let write_search = if !enters_dependency {
        whole_cone_search.clone()
    } else if beyond.is_some() {
        crate::engine::vocab::none_observed(
            "every operation one dependency routine deep, and the whole cone beyond dependency \
             calls",
            "calls not resolved",
        )
    } else {
        crate::engine::vocab::none_observed(
            "every operation one dependency routine deep",
            "dependency code deeper than one routine",
        )
    };
    // Round-2 G29: the answer is the operation count; writes, reads and
    // locks each print on their own line (`parts` is the writes line).
    let mut parts = Vec::new();
    if count("writes") > 0 {
        parts.push(format!(
            "may persist {} on {} ({known} not temporary, {unknown_temp} temp state not resolved)",
            count("writes"),
            plural(ntables("writes"), "table", "tables")
        ));
        parts.extend(conditional.iter().cloned());
    } else if !conditional.is_empty() {
        // The absence of any other write holds only when nothing is written
        // beyond a dependency call either (that part follows when it is).
        parts.push(if beyond_tables.is_empty() {
            format!(
                "{}; otherwise it changes a temporary record; no other write that may persist: {}",
                conditional.join("; "),
                write_search
            )
        } else {
            format!(
                "{}; otherwise it changes a temporary record",
                conditional.join("; ")
            )
        });
    } else if beyond_tables.is_empty() {
        parts.push(format!("may persist: {write_search}"));
    }
    if !beyond_writes.is_empty() {
        parts.push(format!(
            "beyond dependency calls on {} through {} ({} only there)",
            plural(beyond_tables.len(), "table", "tables"),
            plural(
                crate::engine::l5::routine_rollup::routines_entered(&beyond_writes),
                "dependency routine entered",
                "dependency routines entered"
            ),
            beyond_tables.difference(&own_write_tables).count()
        ));
    }
    if count("callerDependent") > 0 {
        parts.push(format!(
            "caller-dependent {} on {}",
            count("callerDependent"),
            plural(ntables("callerDependent"), "table", "tables")
        ));
    }
    // §9.2 (F57): `locks:` and `reads:` always print, with their scope, so
    // a lock question never depends on a missing line meaning zero. The
    // counts include every temp class and unresolved-table operations (F15).
    // A family with operations prints its own line, with its operation
    // counts, below (as do validate, temporary records and unresolved
    // tables); an empty one says so here.
    human.push_str(&format!(
        "  summary: {} (writes, reads and locks below)\n",
        plural(effects.len(), "operation", "operations"),
    ));
    if !parts.is_empty() {
        human.push_str(&format!("  writes: {}\n", parts.join("; ")));
    }
    for label in ["reads", "locks"] {
        if count(label) == 0 {
            // Engine limits B8: the depth clause only when the cone enters
            // dependency code.
            if enters_dependency {
                human.push_str(&format!(
                    "  {label}: none observed (one dependency routine deep; deeper dependency \
                     {label} are not inventoried)\n"
                ));
            } else {
                human.push_str(&format!("  {label}: {whole_cone_search}\n"));
            }
        }
    }
    if beyond.is_some() {
        // Wave 3a: the writes beyond each entry point are the cone's, read
        // as explain reads them; the per-operation rows keep L4's depth.
        // §5.2 (F61): the depth and inventory of each operation family; the
        // header's scope clause (§5.3: the static prose is in the tool
        // description).
        human.push_str(
            "  scope: reads, locks, validate: one dependency routine deep, not exhaustive beyond; \
             writes beyond dependency calls: whole cone with literal arguments, grouped per \
             dependency routine entered; operations and routes the literal arguments rule out \
             are listed and marked (explain and prove drop them)\n",
        );
    } else {
        human.push_str(
            "  scope: reads, locks, validate, writes: one dependency routine deep, not exhaustive \
             beyond (explain and prove follow the whole cone); operations ruled out by literal \
             arguments are listed (explain and prove drop them)\n",
        );
    }
    // Round 2 G62: a zero-operation answer says what it is about.
    if effects.is_empty() {
        human.push_str(&format!("  scope: {ZERO_OPS_SCOPE}\n"));
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
            let op = ops.get(first.operation_id, &join);
            let at = match &op {
                Some(o) => format!("at {}:{} in {}", o.file, shown(o.line), o.owner.qualified()),
                None => format!("(outside the workspace: {})", first.operation_id),
            };
            // §7.5 (F84): `through` only when it names a routine other than
            // the `in` one.
            let through = match first.via {
                crate::engine::l4::effect_store::ViaRank::Direct => String::new(),
                v => match hops.of(first) {
                    Ok(c)
                        if op
                            .as_ref()
                            .is_some_and(|o| o.owner.qualified() == hop_label(c)) =>
                    {
                        via_note(v).to_string()
                    }
                    Ok(c) => format!(", through {}{}", hop_label(c), via_note(v)),
                    Err(_) => via_note(v).to_string(),
                },
            };
            // Step 1b (astra top 5): the mark sits on the row it qualifies.
            let mark = match literal[rows[0]] {
                Some(LiteralRoute::RuledOut) => {
                    " (ruled out by the root's literal arguments: no route runs)".to_string()
                }
                Some(LiteralRoute::NotInCone) => " (ruled out by the root's literal arguments: \
                     the cone, which follows them, has no route to it)"
                    .to_string(),
                Some(LiteralRoute::OtherRoute(s)) => format!(
                    " (the index's route; the cone keeps no live route through it with the root's \
                     literal arguments, and its live route enters through {})",
                    route_display(s)
                ),
                _ => String::new(),
            };
            let ruled_out = rows.iter().filter(|&&i| ruled_out_row(&literal[i])).count();
            let ruled_out = match (ruled_out, rows.len()) {
                (0, _) => String::new(),
                // Every row, so the first one's mark says it.
                (k, n) if k == n && n == 1 => String::new(),
                (k, n) if k == n => format!("; all {n} ruled out by literal arguments"),
                (k, n) => format!("; {k} of {n} ruled out by literal arguments"),
            };
            let locks = locks_on
                .get(id)
                .map(|n| format!(" (+ LockTable {n})"))
                .unwrap_or_default();
            human.push_str(&format!(
                "    {}: {}{locks}{} — first {} {at}{through}{mark}{ruled_out}\n",
                table_name(id),
                ops_text(&op_counts(rows)),
                caller_dependent_words(first.temp_state, &routine.parameters)
                    .map_or_else(|| temp_note(first.temp_state), |w| format!(" ({w})")),
                first.op
            ));
        }
        if n < list.len() {
            human.push_str(&format!(
                "    … +{} — {} lists them\n",
                plural(list.len() - n, "table", "tables"),
                Input::All.on(surface)
            ));
        }
    }
    // Wave 3a: explain's own section, from the same rollup.
    if let Some(b) = &beyond {
        // A run of hops several paths share prints once (§7.5).
        for l in crate::engine::path_segments::segment(b.lines(all, surface)) {
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
            // F15: a caller-dependent or temporary row is counted here by
            // its op family; the note says how many of them are.
            let mut notes = Vec::new();
            let caller_dependent = b
                .rows
                .iter()
                .filter(|&&i| matches!(effects[i].temp_state, TempStateKind::ParameterDependent(_)))
                .count();
            if caller_dependent > 0 {
                notes.push(format!("{caller_dependent} caller-dependent"));
            }
            let temporary = b
                .rows
                .iter()
                .filter(|&&i| matches!(effects[i].temp_state, TempStateKind::Known(true)))
                .count();
            if temporary > 0 {
                notes.push(format!("{temporary} on temporary records"));
            }
            let notes = if notes.is_empty() {
                String::new()
            } else {
                format!("; {}", notes.join(", "))
            };
            human.push_str(&format!(
                "  {label}: {} on {} ({}{notes}){}\n",
                b.rows.len(),
                plural(b.tables.len(), "table", "tables"),
                ops_text(&b.ops),
                all_note
            ));
        }
    }
    if let Some(b) = buckets.get("temporary") {
        human.push_str(&format!(
            "  temporary records: {} on {} in {}{}\n",
            plural(b.rows.len(), "operation", "operations"),
            plural(b.tables.len(), "table", "tables"),
            plural(b.owners.len(), "routine", "routines"),
            all_note
        ));
    }
    // Q4: "RecordRef" only where the receiver is one; every other operation
    // on an unresolved table is "table not resolved".
    for (record_ref, label) in [
        (true, "on a RecordRef (table not resolved: RecordRef)"),
        (false, "table not resolved"),
    ] {
        if let Some(b) = unresolved.get(&record_ref) {
            human.push_str(&format!(
                "  {label}: {} in {} ({}){}\n",
                plural(b.rows.len(), "operation", "operations"),
                plural(b.owners.len(), "routine", "routines"),
                ops_text(&b.ops),
                if all {
                    String::new()
                } else {
                    format!(" — {} lists them", Input::All.on(surface))
                }
            ));
        }
    }
    // §6.2: the complete prove call for the first table it writes; none when
    // it writes no table (the summary already says so).
    if let Some(table) = ["writes", "callerDependent"].iter().find_map(|k| {
        buckets
            .get(k)
            .and_then(|b| b.tables.iter().find(|t| !is_unknown_table(t)))
    }) {
        // The resolved routine's own selector (V3 §2.1), else the input.
        let selector = rr
            .identity
            .iter()
            .find(|(k, _)| k.as_str() == "selector")
            .and_then(|(_, v)| v.as_str())
            .unwrap_or(routine_selector);
        let f = crate::engine::follow_up::FollowUp::prove(
            crate::engine::follow_up::Cx::new(surface, single_app),
            selector,
            &format!("writes-table:{}", table_name(table)),
            "follows the whole cone for one table, with its witness",
        );
        human.push_str(&format!("  next: {} — {}\n", f.text(surface), f.why));
    }
    // Round-2 G29: the operation rows print before the deep-write summary
    // (`writes beyond dependency calls`), after which they are spliced.
    let rows_at = human.len();
    if all {
        human.push_str(&format!("  every operation ({}):\n", effects.len()));
    }
    for (e, lit) in effects.iter().zip(&literal).filter(|_| all) {
        let table = if is_unknown_table(e.table_id) {
            unresolved_label(ops.receiver_is_record_ref(e.operation_id)).to_string()
        } else {
            match tables.get(e.table_id) {
                Some(name) => name.to_string(),
                None => e.table_id.to_string(),
            }
        };
        let mark = match lit {
            Some(LiteralRoute::RuledOut | LiteralRoute::NotInCone) => {
                " [ruled out by literal arguments]".to_string()
            }
            Some(LiteralRoute::OtherRoute(s)) => {
                format!(
                    " [the cone's live route enters through {}]",
                    route_display(s)
                )
            }
            _ => String::new(),
        };
        // F16: a caller-dependent row names its parameter as the table rows
        // do (1-based, with its name), never the raw 0-based index.
        let temp = match caller_dependent_words(e.temp_state, &routine.parameters) {
            Some(w) => w,
            // §8.1 temp-state words (V2); JSON keeps `tempState`.
            None => match e.temp_state {
                TempStateKind::Known(true) => "temporary".to_string(),
                TempStateKind::Known(false) => "not temporary".to_string(),
                _ => "temp state not resolved".to_string(),
            },
        };
        human.push_str(&format!(
            "    [{}] on {} via {}, {temp} — {}{mark}\n",
            e.op,
            table,
            e.via.as_str(),
            origin_human(ops.get(e.operation_id, &join).as_ref(), e.operation_id)
        ));
    }
    if all && let Some(at) = human[..rows_at].find("\n  writes beyond dependency calls") {
        let rows = human.split_off(rows_at);
        human.insert_str(at + 1, &rows);
    }

    let mut summary = effects_summary(&rr.qualified(), &effects, &tables);
    // r3 §5.1 (A P1): the envelope's summary and its index row carry the
    // analysis status, its reasons and the two scopes apart: the operations
    // (one dependency routine deep) and the deep writes (the whole cone).
    let status = effects_status(&query_scope_json("query.effects", &payload, deps));
    if let (Value::Object(m), Value::Object(s)) = (&mut summary, &status) {
        for (k, v) in s {
            m.insert(k.clone(), v.clone());
        }
        if let Some(Value::Array(rows)) = m.get_mut(crate::engine::result_index::ROWS_KEY) {
            for r in rows.iter_mut().filter_map(Value::as_object_mut) {
                for (k, v) in s {
                    r.insert(k.clone(), v.clone());
                }
            }
        }
    }
    // Wave 3a: the MCP summary states the cone's writes beyond dependency
    // calls (r3 §5.2: `dependencyCalls`, as the 5.0.0 payload names them).
    if beyond.is_some()
        && let Value::Object(m) = &mut summary
    {
        m.insert(
            "beyondWrites".into(),
            json!({"tables": beyond_tables.len(), "dependencyCalls": beyond_writes.len()}),
        );
        // Output validation §8.2 (F53): the two populations with their
        // units, so the operation tables and the whole-cone write tables
        // are never compared as one count.
        let line = format!(
            "{} on {} one routine deep; whole cone beyond {}: {} (writes only)",
            plural(effects.len(), "operation", "operations"),
            plural(
                m.get("tableCount").and_then(Value::as_u64).unwrap_or(0) as usize,
                "table",
                "tables"
            ),
            plural(
                crate::engine::l5::routine_rollup::routines_entered(&beyond_writes),
                "dependency routine entered",
                "dependency routines entered"
            ),
            plural(beyond_tables.len(), "table", "tables"),
        );
        m.insert("counts".into(), line.into());
        // Step 1b: and how many rows the root's literal arguments mark.
        m.insert(
            "literalRoutes".into(),
            payload["summary"]["literalRoutes"].clone(),
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

// ---------------------------------------------------------------------------
// `alsem query touches --field` (agent-tasks design §7, milestone D)
// ---------------------------------------------------------------------------

/// What a `query touches --field` run asks.
struct FieldAsk<'a> {
    table: &'a str,
    field: &'a str,
    from: Option<&'a str>,
    direction: Direction,
    all: bool,
    /// `--surface` (output validation §6.1).
    surface: Surface,
}

/// The human answer lists this many objects, and this many sites per object,
/// before folding (`--all` lists every site).
const FIELD_OBJECTS_CAP: usize = 12;
const FIELD_SITES_PER_OBJECT_CAP: usize = 6;

/// The `--from` scope of a field answer: each routine (model position) in
/// the routine's cone (`down`) or among its callers (`up`).
struct FieldScope {
    routine: RoutineRef,
    sides: HashMap<usize, &'static str>,
    down: usize,
    up: usize,
    /// The routine has no row in the solve, so only its own body is read.
    no_solve_row: bool,
    /// How the callers were found: `reverse-walk` (R0 over the sited graph)
    /// or `solve-graph` (single-app, or the sited graph did not build).
    up_engine: &'static str,
    /// Per caller (model position), the best tier R0 reached it by.
    tiers: HashMap<usize, crate::program::impact_graph::reverse::Tier>,
    /// R0 callers the model holds no body row for (symbol-only routines,
    /// dependency code not loaded): their bodies are not searched.
    up_not_in_model: usize,
    /// R0 callers apps outside the program may call (public, protected,
    /// internal with friends).
    up_external: usize,
    /// Nodes of the reverse cone that may run in a way no site shows.
    up_hidden: usize,
    /// The walk stopped at its budget or was cancelled.
    up_limited: bool,
    /// The sited graph was declared but did not build.
    sited_missing: bool,
}

/// What R0's reverse walk reports for one root, as the field answer reads
/// it: the best tier per node, external and hidden counts, a limit.
#[derive(Default)]
struct UpCollector {
    best: HashMap<u32, crate::program::impact_graph::reverse::Tier>,
    external: HashSet<u32>,
    hidden: HashSet<u32>,
    limited: bool,
}

impl crate::program::impact_graph::reverse::ConeVisitor for UpCollector {
    fn reach(
        &mut self,
        _: u32,
        s: crate::program::impact_graph::reverse::WalkState,
        _: u32,
        _: crate::program::impact_graph::reverse::Via,
        _: Option<crate::program::impact_graph::reverse::WalkState>,
    ) {
        let e = self.best.entry(s.node).or_insert(s.tier);
        *e = (*e).min(s.tier);
    }
    fn hidden(&mut self, _: u32, n: u32, _: crate::program::impact_graph::reverse::HiddenChannel) {
        self.hidden.insert(n);
    }
    fn external(
        &mut self,
        _: u32,
        n: u32,
        _: crate::program::impact_graph::reverse::ExternalWhy,
        _: crate::program::impact_graph::reverse::WalkState,
        _: u32,
    ) {
        self.external.insert(n);
    }
    fn limit(&mut self, _: crate::program::impact_graph::reverse::Limit) {
        self.limited = true;
    }
}

/// The `--from` scope: the routine and its transitive callees over the solve
/// graph (`down`: dependency code in the solve is a leaf one routine deep,
/// and a call not resolved has no edge), and its transitive callers (`up`)
/// by R0's reverse walk over the sited graph when it is given, as `alsem
/// tests` and impact read them (through dependency code, with tiers), else
/// over the solve graph.
fn field_scope(
    model: &Model,
    query: &DbEffectQuery<'_>,
    sited: Option<Option<&crate::program::impact_graph::SitedGraph>>,
    r: &ModelRoutine,
    direction: Direction,
    join: &RoutineJoin<'_>,
) -> FieldScope {
    let ix_of: HashMap<&str, usize> = model
        .workspace
        .routines
        .iter()
        .enumerate()
        .map(|(i, r)| (r.id.as_str(), i))
        .collect();
    let mut sides: HashMap<usize, &'static str> = HashMap::new();
    let (mut down, mut up) = (0, 0);
    let mut scope = FieldScope {
        routine: join.of(r),
        sides: HashMap::new(),
        down: 0,
        up: 0,
        no_solve_row: false,
        up_engine: "solve-graph",
        tiers: HashMap::new(),
        up_not_in_model: 0,
        up_external: 0,
        up_hidden: 0,
        up_limited: false,
        sited_missing: matches!(sited, Some(None)),
    };
    // Up by R0 when the sited graph is given (before the solve's own rows:
    // a routine with no solve row still has callers).
    let r0_up = match sited {
        Some(Some(g)) if direction.wants_up() => Some(g),
        _ => None,
    };
    if let Some(g) = r0_up {
        use crate::program::impact_graph::reverse::{WalkOptions, WalkScratch, walk_one};
        scope.up_engine = "reverse-walk";
        let by_stable: HashMap<&str, Vec<usize>> = model
            .workspace
            .routines
            .iter()
            .enumerate()
            .fold(HashMap::new(), |mut m, (i, x)| {
                m.entry(x.stable_routine_id.as_str()).or_default().push(i);
                m
            });
        let roots = g.nodes_with_stable_id(&r.stable_routine_id);
        let mut c = UpCollector::default();
        if let Some(&root) = roots.first() {
            walk_one(
                g,
                root,
                &WalkOptions::default(),
                &mut WalkScratch::default(),
                &mut c,
            );
            for (&n, &tier) in &c.best {
                if n == root || g.universal_hub() == Some(n) {
                    continue;
                }
                let at = g
                    .stable_id(n)
                    .and_then(|id| by_stable.get(id.as_str()))
                    .filter(|ixs| {
                        ixs.iter()
                            .any(|&i| model.workspace.routines[i].body_available)
                    });
                match at {
                    Some(ixs) => {
                        for &i in ixs {
                            sides.insert(i, "up");
                            scope.tiers.insert(i, tier);
                        }
                        up += 1;
                    }
                    None if g.stable_id(n).is_some() => scope.up_not_in_model += 1,
                    None => {}
                }
            }
        }
        scope.up_external = c.external.len();
        scope.up_hidden = c.hidden.len();
        scope.up_limited = c.limited;
    }
    let Some(rix) = query.routine_ix(&r.id) else {
        if let Some(&i) = ix_of.get(r.id.as_str()) {
            sides.insert(i, "down");
        }
        return FieldScope {
            sides,
            down: 1,
            up,
            no_solve_row: true,
            ..scope
        };
    };
    let model_ix = |s: SolveIx| ix_of.get(query.bundle().routine_id(s)).copied();
    if direction.wants_up() && r0_up.is_none() {
        for (_, a) in query.ancestors(rix) {
            if let Some(i) = model_ix(a) {
                sides.insert(i, "up");
                up += 1;
            }
        }
    }
    if direction.wants_down() {
        let mut seen: BTreeSet<SolveIx> = BTreeSet::new();
        let mut stack = vec![rix];
        while let Some(s) = stack.pop() {
            if !seen.insert(s) {
                continue;
            }
            if let Some(i) = model_ix(s) {
                // A routine in both the cone and the callers (a cycle) is
                // reported as down: its body runs below R too.
                sides.insert(i, "down");
            }
            stack.extend(query.direct_callees(s).iter().copied());
        }
        down = seen.len();
    }
    FieldScope {
        sides,
        down,
        up,
        ..scope
    }
}

/// A site's use list as text: `read+field assignment`. Output validation r3
/// §7 R6: a `ModifyAll` that names the field writes it to the database, so
/// its write is a `record write (ModifyAll)`, never a `field assignment` (an
/// assignment changes the record in memory only).
fn uses_text(uses: &[crate::program::body::record_op::FieldUse], source: &str) -> String {
    uses.iter()
        .map(|u| field_use_word(*u, source))
        .collect::<Vec<_>>()
        .join("+")
}

/// One field use in human words at a site whose source is `source` (r3 §7
/// R6): [`FieldUse::human`](crate::program::body::record_op::FieldUse::human),
/// except a `ModifyAll`'s write, which is record persistence.
fn field_use_word(u: crate::program::body::record_op::FieldUse, source: &str) -> &'static str {
    use crate::program::body::record_op::FieldUse;
    if u == FieldUse::Write && source == "ModifyAll" {
        FIELD_RECORD_WRITE
    } else {
        u.human()
    }
}

/// The human word of a field written to the database by `ModifyAll` (r3 §7
/// R6).
const FIELD_RECORD_WRITE: &str = "record write (ModifyAll)";

/// A field's use counts in human words (r3 §7 R6): the uses in their order,
/// with the `ModifyAll` writes counted apart from the in-memory assignments
/// (`field assignment 4, record write (ModifyAll) 1`). `also_read`: the
/// assignment sites that also read the field, noted on the assignment count.
fn field_use_counts_words(
    sites: &[super::field_touches::FieldSite<'_>],
    also_read: usize,
) -> Vec<String> {
    use crate::program::body::record_op::FieldUse;
    let mut out = Vec::new();
    for (u, n) in super::field_touches::use_counts(sites) {
        if u != FieldUse::Write {
            out.push(format!("{} {n}", u.human()));
            continue;
        }
        let persisted = sites
            .iter()
            .filter(|s| s.uses.contains(&FieldUse::Write) && s.source == "ModifyAll")
            .count();
        let assigned = n - persisted;
        if assigned > 0 {
            out.push(if also_read > 0 {
                format!("{} {assigned} ({also_read} also read)", u.human())
            } else {
                format!("{} {assigned}", u.human())
            });
        }
        if persisted > 0 {
            out.push(format!("{FIELD_RECORD_WRITE} {persisted}"));
        }
    }
    out
}

/// What `--from` walks: the solve, and the sited graph when the demand
/// declared it (`Some(None)`: declared, not built).
type FromGraphs<'q, 'g> = (
    &'q DbEffectQuery<'q>,
    Option<Option<&'g crate::program::impact_graph::SitedGraph>>,
);

fn render_field_touches(
    model: &Model,
    deps: &LoadedDependencies,
    query: Option<FromGraphs<'_, '_>>,
    ask: &FieldAsk<'_>,
    alsem_ver: &str,
    deterministic: bool,
) -> QueryRunResult {
    use super::field_touches::{self as ft, Candidate, TempClass};
    use crate::program::body::record_op::FieldUse;
    const KIND: &str = "query.touches";
    if ask.from.is_none() && ask.direction == Direction::Down {
        let payload = json!({
            "direction": ask.direction.as_str(),
            "from": Value::Null,
            "error": "a `down` query is about one routine's cone — pass --from <routine>, \
                      or omit --direction for every consumer",
        });
        return QueryRunResult {
            json_text: envelope(KIND, payload, deps, alsem_ver, deterministic),
            human_text: "query.touches: --direction down requires --from <routine>\n".to_string(),
            exit_code: 2,
            summary: Value::Null,
            ancestor_witnesses_built: 0,
        };
    }
    let table_ref = match resolve_table_selector(model, ask.table) {
        Resolution::Resolved(t) if !is_unknown_table(&t.id) => t,
        Resolution::Resolved(_) => {
            let payload = json!({
                "table": {"selector": ask.table, "resolution": "unknown-bucket"},
                "error": "--field needs a table: `unknown` is the bucket of operations whose \
                          table is not resolved",
            });
            return QueryRunResult {
                json_text: envelope(KIND, payload, deps, alsem_ver, deterministic),
                human_text: "query.touches: --field needs a table, not the `unknown` bucket\n"
                    .to_string(),
                exit_code: 2,
                summary: Value::Null,
                ancestor_witnesses_built: 0,
            };
        }
        other => {
            return selector_failure_doc(
                KIND,
                "table",
                ask.table,
                &other,
                deps,
                alsem_ver,
                deterministic,
            );
        }
    };
    let tables = table_by_id_preferring_real(&model.workspace.tables);
    let table = tables[table_ref.id.as_str()];
    let field = match ft::resolve_field(table, ask.field) {
        Ok(f) => f,
        Err(candidates) => {
            let res: Resolution<()> = if candidates.is_empty() {
                Resolution::Unmatched
            } else {
                Resolution::Ambiguous(candidates)
            };
            return selector_failure_doc(
                KIND,
                "field",
                ask.field,
                &res,
                deps,
                alsem_ver,
                deterministic,
            );
        }
    };
    let join = RoutineJoin::build(model);
    let scope = match ask.from {
        None => None,
        Some(sel) => match resolve_routine_selector(model, &join.declaring, sel) {
            Resolution::Resolved(r) => {
                let (q, sited) = query.expect("--field --from reads the solve (`query_plan`)");
                Some(field_scope(model, q, sited, r, ask.direction, &join))
            }
            other => {
                return routine_failure_doc(
                    KIND,
                    "from",
                    sel,
                    &other,
                    model,
                    deps,
                    alsem_ver,
                    deterministic,
                );
            }
        },
    };

    let in_scope = |ix: usize| scope.as_ref().is_none_or(|sc| sc.sides.contains_key(&ix));
    let answer = ft::field_touches(model, table, field, &in_scope);
    let routines = &model.workspace.routines;
    let mut sites: Vec<(&ft::FieldSite<'_>, RoutineRef, Option<&'static str>)> = answer
        .sites
        .iter()
        .map(|s| {
            let side = scope
                .as_ref()
                .and_then(|sc| sc.sides.get(&s.routine_ix).copied());
            (s, join.of(&routines[s.routine_ix]), side)
        })
        .collect();
    sites.sort_by(|a, b| {
        (
            &a.1.object_display,
            &a.1.display,
            &a.0.anchor.source_unit_id,
            a.0.anchor.start_line,
            a.0.anchor.start_column,
        )
            .cmp(&(
                &b.1.object_display,
                &b.1.display,
                &b.0.anchor.source_unit_id,
                b.0.anchor.start_line,
                b.0.anchor.start_column,
            ))
    });

    // Counts: sites, uses, temp classes, candidates.
    let site_list: Vec<ft::FieldSite<'_>> = sites.iter().map(|(s, _, _)| (*s).clone()).collect();
    let use_counts = ft::use_counts(&site_list);
    let read_write_sites = site_list
        .iter()
        .filter(|s| s.uses.contains(&FieldUse::Read) && s.uses.contains(&FieldUse::Write))
        .count();
    let mut temp_counts: BTreeMap<TempClass, usize> = BTreeMap::new();
    let mut candidate_counts: BTreeMap<Candidate, usize> = BTreeMap::new();
    for s in &site_list {
        *temp_counts.entry(s.temp).or_insert(0) += 1;
        if let Some(c) = s.candidate {
            *candidate_counts.entry(c).or_insert(0) += 1;
        }
    }
    let candidates: usize = candidate_counts.values().sum();
    let routine_set: BTreeSet<usize> = site_list.iter().map(|s| s.routine_ix).collect();
    let object_set: BTreeSet<&str> = sites
        .iter()
        .map(|(_, r, _)| r.object_display.as_str())
        .collect();

    // What was searched.
    let cov = &answer.coverage;
    let cross_app = !deps.apps.is_empty();
    let mut not_searched: Vec<Value> = ft::NOT_SEARCHED
        .iter()
        .map(|(k, w)| json!({"kind": k, "what": w}))
        .collect();
    // The human sentence is `field_touches::search_words` (one helper with
    // `explain --field`, output validation V2 §4.2).
    if cross_app {
        not_searched.push(json!({"kind": "dependency-code-not-demanded",
            "what": "dependency code the workspace does not call is not loaded"}));
    } else {
        not_searched.push(json!({"kind": "dependency-code",
            "what": "no dependency code is loaded (single-app, or no dependency)"}));
    }
    if cov.dependency_without_body > 0 {
        not_searched.push(json!({"kind": "routines-without-body",
            "routines": cov.dependency_without_body}));
    }
    let scope_label = match &scope {
        None => "every routine body".to_string(),
        Some(sc) => {
            let mut parts = Vec::new();
            if ask.direction.wants_down() {
                parts.push(format!(
                    "{}'s cone by the solve graph ({})",
                    sc.routine.qualified(),
                    plural(sc.down, "routine", "routines")
                ));
            }
            if ask.direction.wants_up() {
                use crate::program::impact_graph::reverse::Tier;
                if sc.up_engine == "reverse-walk" {
                    let tier_n = |t: Tier| sc.tiers.values().filter(|x| **x == t).count();
                    parts.push(format!(
                        "its callers by the reverse walk ({} with a body, by tier: local {}, \
                         via-dependency {}, via-hub {}; {} more without a body in the model)",
                        plural(sc.up, "routine", "routines"),
                        tier_n(Tier::Local),
                        tier_n(Tier::ThroughDependency),
                        tier_n(Tier::OnlyOpen),
                        sc.up_not_in_model
                    ));
                } else {
                    parts.push(format!(
                        "its callers by the solve graph ({})",
                        plural(sc.up, "routine", "routines")
                    ));
                }
            }
            parts.join(" and ")
        }
    };

    // Why an answer is not a whole "no" (and what may hide more).
    let mut reasons: Vec<(Value, String)> = vec![(
        json!({"kind": "declaration-consumers-not-searched"}),
        "declaration-level consumers (CalcFormula, TableRelation, columns) are not searched"
            .to_string(),
    )];
    if cross_app {
        reasons.push((
            json!({"kind": "dependency-code-not-demanded"}),
            "dependency code the workspace does not call is not loaded".to_string(),
        ));
    }
    if cov.by_number_not_literal > 0 {
        reasons.push((
            json!({"kind": "by-number-not-literal", "sites": cov.by_number_not_literal}),
            format!(
                "{} RecordRef.Field with a number that is not a literal (any field; not matched)",
                plural(cov.by_number_not_literal, "site uses", "sites use")
            ),
        ));
    }
    if let Some(sc) = &scope {
        if ask.direction.wants_down() || sc.up_engine == "solve-graph" {
            reasons.push((
                json!({"kind": "resolved-call-graph", "noSolveRow": sc.no_solve_row}),
                if sc.no_solve_row && ask.direction.wants_down() {
                    "the routine has no row in the call graph: only its own body was read in its \
                     cone"
                        .to_string()
                } else {
                    "the solve graph is the resolved call graph: calls not resolved, and \
                     dependency code deeper than one routine, are not followed"
                        .to_string()
                },
            ));
        }
        if ask.direction.wants_up() {
            if sc.sited_missing {
                reasons.push((
                    json!({"kind": "sited-graph-not-built"}),
                    "the sited graph did not build: callers by the solve graph, dependency code \
                     one routine deep"
                        .to_string(),
                ));
            }
            if sc.up_not_in_model > 0 {
                reasons.push((
                    json!({"kind": "callers-without-body", "routines": sc.up_not_in_model}),
                    format!(
                        "{} the model holds no body for are not searched",
                        plural(sc.up_not_in_model, "caller", "callers")
                    ),
                ));
            }
            if sc.up_external > 0 {
                reasons.push((
                    json!({"kind": "external-callers", "routines": sc.up_external}),
                    format!(
                        "{} may also be called by apps outside the program",
                        plural(
                            sc.up_external,
                            "routine in the reverse cone",
                            "routines in the reverse cone"
                        )
                    ),
                ));
            }
            if sc.up_hidden > 0 {
                reasons.push((
                    json!({"kind": "hidden-callers", "nodes": sc.up_hidden}),
                    format!(
                        "{} in the reverse cone may run in a way no call site shows",
                        plural(sc.up_hidden, "node", "nodes")
                    ),
                ));
            }
            if sc.up_limited {
                reasons.push((
                    json!({"kind": "reverse-walk-limited"}),
                    "the reverse walk stopped at its budget: callers beyond it are not listed"
                        .to_string(),
                ));
            }
        }
    }

    // JSON.
    let site_json: Vec<Value> = sites
        .iter()
        .map(|(s, rr, side)| {
            let mut v = json!({
                "kind": s.uses.first().map_or("", |u| u.as_str()),
                "uses": s.uses.iter().map(|u| u.as_str()).collect::<Vec<_>>(),
                "site": {
                    "file": s.anchor.source_unit_id,
                    "line": shown(s.anchor.start_line),
                    "column": s.anchor.start_column,
                },
                "routine": routine_ref_json(rr),
                "object": rr.object_display,
                "record": s.record,
                "source": s.source,
                "tempState": s.temp.as_str(),
                "candidate": s.candidate.map(Candidate::as_str),
                "role": s.role.as_str(),
            });
            if let Some(side) = s.copy_side {
                v["copySide"] = match side {
                    ft::CopySide::Receiver => "receiver".into(),
                    ft::CopySide::Source => "source".into(),
                };
            }
            if let Some(side) = side {
                v["scope"] = (*side).into();
            }
            if let Some(t) = scope
                .as_ref()
                .filter(|_| *side == Some("up"))
                .and_then(|sc| sc.tiers.get(&s.routine_ix))
            {
                v["tier"] = t.as_str().into();
            }
            v
        })
        .collect();
    let field_name_of = |id: &str| table.field_name(id).unwrap_or(id).to_string();
    let keys_json: Vec<Value> = answer
        .declared_keys
        .iter()
        .map(|&i| {
            let k = &table.keys[i];
            let mut v = json!({"id": k.id, "fields": k.fields.iter().map(|f| field_name_of(f)).collect::<Vec<_>>()});
            // r3 L1 (additive): the table extension that adds the key.
            if let Some(ext) = &k.declared_by {
                v["declaredBy"] = json!(ext);
            }
            v
        })
        .collect();
    let uses_json: Map<String, Value> = use_counts
        .iter()
        .map(|(u, n)| (u.as_str().to_string(), (*n).into()))
        .collect();
    let temp_json: Map<String, Value> = temp_counts
        .iter()
        .map(|(t, n)| (t.as_str().to_string(), (*n).into()))
        .collect();
    let cand_json: Map<String, Value> = candidate_counts
        .iter()
        .map(|(c, n)| (c.as_str().to_string(), (*n).into()))
        .collect();
    let mut payload = Map::new();
    payload.insert(
        "table".into(),
        json!({
            "selector": ask.table,
            "id": table.id,
            "name": table.name,
            "isUnknownBucket": false,
            "resolution": "resolved",
        }),
    );
    payload.insert(
        "field".into(),
        json!({
            "selector": ask.field,
            "id": field.id,
            "number": field.field_number,
            "name": field.name,
            "dataType": field.data_type,
            "fieldClass": field.field_class,
            "resolution": "resolved",
        }),
    );
    payload.insert("direction".into(), ask.direction.as_str().into());
    payload.insert(
        "from".into(),
        match &scope {
            None => Value::Null,
            Some(sc) => json!({
                "selector": ask.from.unwrap_or(""),
                "resolution": "resolved",
                "routine": routine_ref_json(&sc.routine),
            }),
        },
    );
    let kinds: Vec<&str> = [
        FieldUse::Read,
        FieldUse::Write,
        FieldUse::Validate,
        FieldUse::Filter,
        FieldUse::KeyUse,
        FieldUse::Load,
        FieldUse::Calc,
        FieldUse::Rule,
        FieldUse::Metadata,
        FieldUse::CopyCandidate,
        FieldUse::ByNumber,
    ]
    .iter()
    .map(|u| u.as_str())
    .chain(["declared-key"])
    .collect();
    payload.insert(
        "searched".into(),
        json!({
            "scope": scope_label,
            "workspaceRoutineBodies": cov.workspace_bodies,
            "dependencyRoutineBodies": cov.dependency_bodies,
            "kinds": kinds,
            "notSearched": not_searched,
        }),
    );
    payload.insert(
        "fieldSummary".into(),
        json!({
            "sites": site_list.len(),
            "routines": routine_set.len(),
            "objects": object_set.len(),
            "uses": uses_json,
            "readWriteSites": read_write_sites,
            "sitesByTempState": temp_json,
            "candidates": candidates,
            // §8.2 (F53, 3.10.0): the sites with no candidate reason and
            // those with one; they add up to `sites`.
            "confirmedSites": site_list.len() - candidates,
            "candidateSites": candidates,
            "candidatesByReason": cand_json,
            "byNumberNotLiteral": cov.by_number_not_literal,
            "fieldIndexSites": cov.field_index_sites,
            "declaredKeys": keys_json.len(),
            // §9.3 (F74, 3.8.0): the two inventories, and the filters.
            "confirmed": {"sites": sites.iter().filter(|(s, _, _)| !s.uses.contains(&FieldUse::ByNumber)).count()},
            "numericCandidates": {"sites": sites.iter().filter(|(s, _, _)| s.uses.contains(&FieldUse::ByNumber)).count()},
        }),
    );
    payload.insert(
        "filters".into(),
        json!({"sites": sites.iter().filter(|(s, _, _)| s.uses.contains(&FieldUse::Filter)).count()}),
    );
    payload.insert("fieldTouches".into(), Value::Array(site_json));
    payload.insert("declaredKeys".into(), Value::Array(keys_json));
    payload.insert(
        "reasons".into(),
        Value::Array(reasons.iter().map(|r| r.0.clone()).collect()),
    );
    if site_list.is_empty() {
        payload.insert(
            "absence".into(),
            json!({
                "answer": "unknown",
                "statement": "no indexed consumer observed",
                "reasons": reasons.iter().map(|r| r.0.clone()).collect::<Vec<_>>(),
            }),
        );
    }

    // Human.
    let mut human = format!(
        "--- touches field {} \"{}\" of {} ({}) ---\n",
        field.field_number,
        field.name,
        object_display("Table", &table.name),
        table.id
    );
    human.push_str(&format!(
        "  field: {}, {}\n",
        field.data_type, field.field_class
    ));
    // Output validation V2 (§9.3 field mode, F74): one search sentence (the
    // helper `explain --field` uses too), confirmed named references and
    // numeric candidates as two inventories, and `filters:` always.
    let (searched_w, not_searched_w) = ft::search_words(cov, cross_app);
    human.push_str(&format!(
        "  searched: {searched_w} (scope: {scope_label}); not searched: {not_searched_w}\n"
    ));
    let numeric: Vec<&ft::FieldSite<'_>> = sites
        .iter()
        .map(|(s, _, _)| s)
        .filter(|s| s.uses.contains(&FieldUse::ByNumber))
        .copied()
        .collect();
    let filters = sites
        .iter()
        .filter(|(s, _, _)| s.uses.contains(&FieldUse::Filter))
        .count();
    if site_list.is_empty() {
        human.push_str(&format!(
            "  consumers: {}, which is not proof of absence:\n",
            crate::engine::vocab::none_observed(&searched_w, &not_searched_w)
        ));
        for (_, text) in &reasons {
            human.push_str(&format!("    {text}\n"));
        }
    } else {
        // r3 §7 R6: the ModifyAll writes apart from the in-memory
        // assignments.
        let uses_h: Vec<String> = field_use_counts_words(&site_list, read_write_sites);
        let temp_h: Vec<String> = temp_counts
            .iter()
            .map(|(t, n)| format!("{} {n}", t.as_str()))
            .collect();
        let cand_h = if candidates == 0 {
            String::new()
        } else {
            let by: Vec<String> = candidate_counts
                .iter()
                .map(|(c, n)| format!("{} {n}", c.as_str()))
                .collect();
            format!("; candidates by reason: {}", by.join(", "))
        };
        // Output validation §8.2 (F53): confirmed sites and candidates
        // counted apart (they add up to the sites), each count with its
        // unit: uses per site (a site can have two), sites per temp state.
        human.push_str(&format!(
            "  consumers: {}, {} ({} in {} of {}) — uses (a site can have two): {}; sites by \
             record: {}{cand_h}\n",
            plural(site_list.len() - candidates, "site", "sites"),
            plural(candidates, "candidate", "candidates"),
            plural(site_list.len(), "site", "sites"),
            plural(routine_set.len(), "routine", "routines"),
            plural(object_set.len(), "object", "objects"),
            uses_h.join(", "),
            temp_h.join(", "),
        ));
        human.push_str(&format!(
            "    confirmed named references: {}; numeric candidates (RecordRef.Field by number): {}{}\n",
            plural(site_list.len() - numeric.len(), "site", "sites"),
            plural(numeric.len(), "site", "sites"),
            if numeric.is_empty() {
                ""
            } else {
                " — a numeric reference does not break on a rename (it may on a renumbering)"
            }
        ));
        // §5.3 (F85): no `note:` that repeats the `searched:` clause (the
        // not-loaded dependency code and the dynamic field numbers are its
        // `not searched:` part); the JSON keeps every reason.
    }
    // Round-2 G25: the search clause is the `searched:` line's; `filters:`
    // names it rather than printing it again.
    human.push_str(&format!(
        "  filters: {}\n",
        if filters == 0 {
            "none observed (same search)".to_string()
        } else {
            plural(filters, "site", "sites")
        }
    ));
    // §9.3 (F60): the declared keys first, so the verdict can say "above".
    let keys_line = if answer.declared_keys.is_empty() {
        "  declared keys holding the field: none\n".to_string()
    } else {
        let keys: Vec<String> = answer
            .declared_keys
            .iter()
            .map(|&i| {
                let k = &table.keys[i];
                let names: Vec<String> = k.fields.iter().map(|f| field_name_of(f)).collect();
                // r3 L1: a key a table extension adds is named by it.
                match &k.declared_by {
                    Some(ext) => format!(
                        "key {} ({}; from tableextension \"{}\")",
                        i + 1,
                        names.join(", "),
                        ext.replace('"', "\"\"")
                    ),
                    None => format!("key {} ({})", i + 1, names.join(", ")),
                }
            })
            .collect();
        format!("  declared keys: {}\n", keys.join("; "))
    };
    human.push_str(&keys_line);
    human.push_str(
        "  not a complete rename inventory: page and report columns, CalcFormula and \
         TableRelation are not searched (the compiler finds them); keys holding the field are \
         listed above\n",
    );
    // Output validation §8.2 (F54): what each tag on a site row means, for
    // the tags the rows carry, printed once under the first row.
    let legend: Option<String> = {
        let mut tags: BTreeSet<String> = BTreeSet::new();
        let mut methods = false;
        for (s, _, _) in &sites {
            tags.insert(s.temp.as_str().to_string());
            match s.source.as_str() {
                "access" | "implicit-access" => {
                    tags.insert(s.source.clone());
                }
                _ => methods = true,
            }
            if let Some(c) = s.candidate {
                tags.insert(format!("candidate: {}", c.as_str()));
            }
        }
        let mut parts: Vec<String> = tags
            .iter()
            .map(|t| {
                let what = match t.as_str() {
                    "persistent" => "the record is not temporary",
                    "temporary" => "a temporary record",
                    "caller-dependent" => "the record is a parameter: temporary or not as its caller passes it",
                    "unknown" => "the record's temp state is not resolved",
                    "access" => "a field named on a record (Rec.Field)",
                    "implicit-access" => "a bare field name inside the table's own code",
                    "candidate: table-not-resolved" => "the record's table is not resolved, so the site may not name this field",
                    "candidate: copy-table-not-resolved" => "the other record's table is not resolved, so the copy may not include this field",
                    "candidate: recordref-table-unknown" => "a RecordRef whose table is not resolved, so the number may name another table's field",
                    _ => "",
                };
                format!("{t}: {what}")
            })
            .collect();
        if methods {
            parts.push(
                "a method name (Field, SetRange, …): the record method that names the field".into(),
            );
        }
        (!sites.is_empty()).then(|| format!("  legend: {}\n", parts.join("; ")))
    };
    // Round-2 G25: the legend prints once, before the per-object list.
    if let Some(l) = &legend {
        human.push_str(l);
    }
    // Per object, folded unless --all.
    let mut by_object: Vec<(&str, Vec<usize>)> = Vec::new();
    for (i, (_, rr, _)) in sites.iter().enumerate() {
        match by_object.last_mut() {
            Some((o, v)) if *o == rr.object_display.as_str() => v.push(i),
            _ => by_object.push((rr.object_display.as_str(), vec![i])),
        }
    }
    let shown_objects = fold_len(by_object.len(), FIELD_OBJECTS_CAP, ask.all);
    for (object, ixs) in &by_object[..shown_objects] {
        let obj_sites: Vec<ft::FieldSite<'_>> = ixs.iter().map(|&i| sites[i].0.clone()).collect();
        let obj_uses: Vec<String> = field_use_counts_words(&obj_sites, 0);
        let obj_routines: BTreeSet<usize> = obj_sites.iter().map(|s| s.routine_ix).collect();
        human.push_str(&format!(
            "  {object}: {} in {} ({})\n",
            plural(ixs.len(), "site", "sites"),
            plural(obj_routines.len(), "routine", "routines"),
            obj_uses.join(", ")
        ));
        let shown_sites = fold_len(ixs.len(), FIELD_SITES_PER_OBJECT_CAP, ask.all);
        for &i in &ixs[..shown_sites] {
            let (s, rr, side) = &sites[i];
            let mut extra = vec![s.temp.as_str().to_string(), s.source.clone()];
            if let Some(c) = s.candidate {
                extra.push(format!("candidate: {}", c.as_str()));
            }
            if let Some(side) = side {
                match scope.as_ref().and_then(|sc| sc.tiers.get(&s.routine_ix)) {
                    Some(t) if *side == "up" => {
                        extra.push(format!("up, {}", crate::engine::vocab::tier(*t)))
                    }
                    _ => extra.push((*side).to_string()),
                }
            }
            human.push_str(&format!(
                "    {} {}:{} {} [{}]\n",
                rr.in_object,
                s.anchor.source_unit_id,
                shown(s.anchor.start_line),
                uses_text(&s.uses, &s.source),
                extra.join(", ")
            ));
        }
        if shown_sites < ixs.len() {
            human.push_str(&format!(
                "    … +{} — {} lists them\n",
                plural(ixs.len() - shown_sites, "site", "sites"),
                Input::All.on(ask.surface)
            ));
        }
    }
    if shown_objects < by_object.len() {
        human.push_str(&format!(
            "  … +{} — {} lists every site\n",
            plural(by_object.len() - shown_objects, "object", "objects"),
            Input::All.on(ask.surface)
        ));
    }
    let summary = touches_summary(&payload);
    QueryRunResult {
        json_text: envelope(KIND, Value::Object(payload), deps, alsem_ver, deterministic),
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

    fn site(op: &str, line: u32, key: Option<&'static str>) -> TableSite {
        TableSite {
            file: "ws:a.al".into(),
            line,
            op: op.into(),
            who: format!("R{line}"),
            object: "Codeunit X".into(),
            key,
            unresolved: String::new(),
            temp: TempStateKind::Known(false),
        }
    }

    /// Round-3 n03, hand-stated: Base Application's Customer shape, a key on
    /// a system field (`SystemModifiedAt`, never a declared `field`) among
    /// ordinary keys, and one whose member names no field. Every declared key
    /// counts: 1 primary + 6 others, 4 shown and `+2`; the system field
    /// prints by its name.
    #[test]
    fn a_key_on_a_system_field_counts_and_prints() {
        let src = "table 50100 Cust
{
    fields
    {
        field(1; \"No.\"; Code[20]) { }
        field(2; Name; Text[100]) { }
        field(3; City; Text[30]) { }
    }
    keys
    {
        key(Key1; \"No.\") { Clustered = true; }
        key(Key2; Name) { }
        key(Key3; City) { }
        key(Key4; SystemModifiedAt) { }
        key(Key5; Name, City) { }
        key(Key6; SystemId, Name) { }
        key(Key7; NotAField) { }
    }
}
";
        let model =
            crate::program::model::program_calls::assemble_and_resolve_inline_program_default(
                &[("src/Tab.al".to_string(), src.to_string())],
                "11111111-0000-0000-0000-0000000a0003",
            );
        let id = &model
            .workspace
            .tables
            .iter()
            .find(|t| t.name == "Cust")
            .expect("table")
            .id;
        assert_eq!(
            table_keys_line(&model, id),
            "  keys: primary key \"No.\"; other keys: (\"Name\"), (\"City\"), \
             (\"SystemModifiedAt\"), (\"Name\", \"City\"), … +2\n"
        );
    }

    /// Output validation r3 L1, hand-stated: a table extension's keys attach
    /// to the table it extends, named by the extension, after the table's own
    /// keys; a key over a base field and an extension field keeps both; the
    /// base's first key stays the primary key even though the extension's
    /// first key comes first in the extension.
    ///
    /// DISCRIMINATION (2026-11-08): (1) the key merge removed from
    /// `merge_extension_fields` (no extension key on the base) fails the
    /// keys line; (2) resolving the extension's key members against the
    /// extension's own fields only (`by_name` over the stub's fields) drops
    /// `"No."` from `("No.", "Ext Code")` and fails the same assert.
    #[test]
    fn a_table_extensions_keys_attach_to_the_table_it_extends() {
        let src = "table 50100 Cust
{
    fields
    {
        field(1; \"No.\"; Code[20]) { }
        field(2; Name; Text[100]) { }
    }
    keys
    {
        key(PK; \"No.\") { Clustered = true; }
        key(ByName; Name) { }
    }
}
tableextension 50101 \"Cust Ext\" extends Cust
{
    fields
    {
        field(50100; \"Ext Code\"; Code[10]) { }
    }
    keys
    {
        key(ExtKey; \"No.\", \"Ext Code\") { }
        key(ExtKey2; SystemModifiedAt) { }
    }
}
";
        let model =
            crate::program::model::program_calls::assemble_and_resolve_inline_program_default(
                &[("src/Tab.al".to_string(), src.to_string())],
                "11111111-0000-0000-0000-0000000a0004",
            );
        let table = model
            .workspace
            .tables
            .iter()
            .find(|t| t.name == "Cust" && !t.is_extension_stub)
            .expect("table");
        assert_eq!(
            table_keys_line(&model, &table.id),
            "  keys: primary key \"No.\"; other keys: (\"Name\"), (\"No.\", \"Ext Code\"; from \
             tableextension \"Cust Ext\"), (\"SystemModifiedAt\"; from tableextension \"Cust \
             Ext\")\n"
        );
        let ext: Vec<_> = table
            .keys
            .iter()
            .filter(|k| k.declared_by.is_some())
            .collect();
        assert_eq!(ext.len(), 2);
        assert_eq!(ext[0].declared_by.as_deref(), Some("Cust Ext"));
        // Distinct from the base keys: its own id.
        assert!(table.keys.iter().filter(|k| k.id == ext[0].id).count() == 1);
        assert_ne!(ext[0].id, table.keys[0].id);
    }

    /// Round 2 G14: the header's `limits:` agrees with the body. A query
    /// effects body that states a limit (`deeper dependency locks are not
    /// inventoried`, the families inventoried one dependency routine deep)
    /// never has `limits: none`. Hand-stated body and families (c11's
    /// shape: writes beyond dependency calls followed whole cone).
    #[test]
    fn a_body_limit_clause_implies_header_limits() {
        let one_deep = json!({"depth": "oneDependencyRoutine", "inventory": "notExhaustive",
                              "blockers": {"callNotResolved": 0, "interfaceDispatchOpenWorld": 0}});
        let families = json!({
            "reads": one_deep, "locks": one_deep, "validate": one_deep, "writes": one_deep,
            "beyondDependencyCalls": {"depth": "wholeCone", "inventory": "exhaustive",
                "blockers": {"callNotResolved": 0, "interfaceDispatchOpenWorld": 0}},
        });
        let body = "--- effects of Codeunit X::Y ---\n  summary: 1 operation — locks: none \
                    observed (one dependency routine deep; deeper dependency locks are not \
                    inventoried)\n";
        let hc = QueryHeaderCx {
            surface: Surface::Cli,
            single_app: false,
            effects: true,
            all: false,
            callers: false,
            up: false,
            asked: "Y".into(),
        };
        let out = query_with_header(body, 0, 0, &hc, Some(&families));
        let third = out.lines().nth(2).unwrap();
        assert!(out.contains("not inventoried"), "{out}");
        assert!(!third.contains("limits: none"), "{third}");
        assert!(
            third.contains("limits: reads, locks and validations one dependency routine deep"),
            "{third}"
        );
    }

    /// Round 2 G8: a table with many key-relevant reads, a few writes and a
    /// validation (hand-stated, the n03 shape): the default samples carry
    /// every record write (at most six), the validation and a key-field
    /// filter before the Gets, and stay within the cap.
    #[test]
    fn family_samples_spread_over_families_writes_first() {
        let mut sites: Vec<TableSite> = (1..=30).map(|l| site("Get", l, Some(KEY_GET))).collect();
        sites.push(site("SetRange", 40, Some(KEY_FILTER)));
        sites.push(site("Validate", 50, None));
        for l in 60..68 {
            sites.push(site("Modify", l, None));
        }
        sites.push(site("Insert", 68, None));
        sites.push(site("DeleteAll", 69, None));
        sites.push(site("CalcFields", 70, None));
        let got = family_samples(&sites);
        assert_eq!(got.len(), GLOBAL_LIST_CAP);
        let ops: Vec<&str> = got.iter().map(|s| s.op.as_str()).collect();
        // The writes first, each kind once before a second Modify.
        assert_eq!(
            &ops[..WRITE_SAMPLES],
            &[
                "Modify",
                "DeleteAll",
                "Insert",
                "Modify",
                "Modify",
                "Modify"
            ],
            "{ops:?}"
        );
        assert!(ops.contains(&"Validate"), "{ops:?}");
        assert!(ops.contains(&"CalcFields"), "{ops:?}");
        let filter = ops.iter().position(|o| *o == "SetRange").expect("filter");
        let get = ops.iter().position(|o| *o == "Get").expect("get");
        assert!(
            filter < get,
            "a key-field filter is sampled before the Gets: {ops:?}"
        );
    }

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
            enum_impls: None,
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

    /// Agent-tasks D (coordinator review): `touches --field --from R
    /// --direction up` takes R's callers from R0's reverse walk over the
    /// sited graph (`program::impact_graph::reverse`, as `alsem tests`
    /// reads them) cross-app, and from the solve graph single-app, where no
    /// sited graph is built. On code both see the same way (a workspace with
    /// no dependency: two call hops, an event publisher and its subscriber)
    /// the two must name the same callers; on CDO they differ through
    /// dependency code, which the solve keeps one routine deep (spec §7.3a).
    #[test]
    fn field_callers_agree_with_the_reverse_walk_on_local_code() {
        use crate::program::impact_graph::reverse::{Recorder, WalkOptions, WalkScratch, walk_one};
        let ws = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/field-touches/ws");
        let inputs = std::sync::Arc::new(crate::program::inputs::WorkspaceInputs::read(
            &ws,
            &Default::default(),
        ));
        // The solve graph's callers, as the answer computes them.
        let view = query_view(&inputs, false).expect("view");
        let substrate = QuerySubstrate::of(&view);
        let model = substrate.resolved();
        let query = substrate.query();
        let join = RoutineJoin::build(model);
        let kinds = model
            .workspace
            .routines
            .iter()
            .find(|r| r.name == "Kinds")
            .expect("Kinds");
        let ids = |sc: &FieldScope| -> BTreeSet<String> {
            sc.sides
                .keys()
                .map(|&i| model.workspace.routines[i].stable_routine_id.clone())
                .collect()
        };
        // R0's callers over the sited graph of the same workspace.
        let program = crate::program::resolved::ResolvedProgram::build(
            &inputs,
            crate::program::resolved::ProgramScope::CrossApp,
        )
        .expect("program");
        let g = crate::program::impact_graph::sited_from_program(&program, &|| false)
            .expect("sited graph");
        let root = g.nodes_with_stable_id(&kinds.stable_routine_id)[0];
        let mut rec = Recorder::default();
        walk_one(
            &g,
            root,
            &WalkOptions::default(),
            &mut WalkScratch::default(),
            &mut rec,
        );
        let r0: BTreeSet<String> = rec
            .states
            .iter()
            .filter(|s| s.1.node != root)
            .filter_map(|s| g.stable_id(s.1.node))
            .collect();
        // The answer's callers, by each engine.
        let by_r0 = field_scope(model, &query, Some(Some(&g)), kinds, Direction::Up, &join);
        let by_solve = field_scope(model, &query, None, kinds, Direction::Up, &join);
        assert_eq!(by_r0.up_engine, "reverse-walk");
        assert_eq!(by_solve.up_engine, "solve-graph");
        assert_eq!(r0.len(), 4, "the fixture's callers: {r0:?}");
        assert_eq!(ids(&by_r0), r0, "the answer reads the reverse walk");
        assert_eq!(ids(&by_solve), r0, "on local code the solve graph agrees");
    }
}

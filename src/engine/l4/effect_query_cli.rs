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

use std::collections::HashMap;

use serde_json::{Map, Value, json};

use crate::engine::batch::job::{
    Doc, Job, JobFailure, PlanPart, ResolveCx, SideDoc, SideIx, ViewCx, ViewDemand, ViewFailure,
    ViewKey, ViewNeeds,
};
use crate::engine::batch::output::{JobOutput, WriteAction};
use crate::engine::gate::format_json::{pinned_or_now_iso8601, serialize_document_value};
use crate::engine::gate::run::AnalysisTarget;
use crate::engine::l4::effect_lattice::TempStateKind;
use crate::engine::l4::effect_query::{
    AncestorTouch, DbEffectQuery, TableTouch, UNKNOWN_TABLE_ID, is_unknown_table,
};
use crate::engine::session::{Cell, View, query_plan};
use crate::program::model::declaring::DeclaringObjects;
use crate::program::model::workspace::{
    Model, ModelRecordOperation, ModelRoutine, table_by_id_preferring_real,
};
use crate::program::resolve::full::LoadedDependencies;

/// The JSON envelope's `schemaVersion` for both `query` subcommands. 1.1.0 (#66):
/// additive `payload.dependencies` {mode, apps}.
const QUERY_SCHEMA_VERSION: &str = "1.1.0";

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
}

// ---------------------------------------------------------------------------
// The owned substrate (one workspace load per invocation).
// ---------------------------------------------------------------------------

/// What a [`DbEffectQuery`] borrows, off a [`View`] whose `l4` cell query's
/// plan filled (no detector run, #160 §4.4.4): the model, the relation's graph
/// and the `l4` analysis (the bundle and its SCC). A borrow, so a query shares
/// its view with the batch's other jobs (#277 D9).
///
/// Cross-app (the default, #66), the view is `alsem analyze`'s cross-app base:
/// its graph and SCC, with every source-bearing dependency routine a FIXED
/// leaf holding its own direct db effects, so a dependency's tables resolve
/// and are selectable. With no dependency there are no leaves and the same
/// graph, so the two modes agree.
pub struct QuerySubstrate<'a> {
    view: &'a View,
    /// Which dependencies the model holds (#66), stated in every document.
    pub dependencies: LoadedDependencies,
}

impl<'a> QuerySubstrate<'a> {
    /// The substrate of a view prepared with [`query_plan`].
    pub fn of(view: &'a View) -> Self {
        QuerySubstrate {
            view,
            dependencies: view.target().dependencies(),
        }
    }

    /// The model the selectors and joins read.
    pub fn resolved(&self) -> &Model {
        self.view.model()
    }

    /// The query surface over this substrate.
    pub fn query(&self) -> DbEffectQuery<'_> {
        let l4 = self.view.l4();
        DbEffectQuery::build(&l4.bundle, &l4.scc, self.view.rel().primary())
    }
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
    },
    Effects {
        routine: &'a str,
    },
}

/// `alsem query touches` / `effects` as a batch job (#277 D1). Its document is
/// `Result<QueryRunResult, String>`; its writes are the `--format` text (to
/// `--out` or stdout), or an error line and exit 1.
pub struct QueryJob<'a> {
    inputs: &'a std::sync::Arc<crate::program::inputs::WorkspaceInputs>,
    ask: QueryAsk<'a>,
    alsem_ver: &'a str,
    deterministic: bool,
    single_app: bool,
    /// `--format json`: the JSON text, else the human text.
    json: bool,
    out: Option<&'a str>,
}

impl<'a> QueryJob<'a> {
    /// `alsem query touches` (see [`run_query_touches_pipeline`]), written as
    /// JSON to stdout until [`QueryJob::written`].
    pub fn touches(
        inputs: &'a std::sync::Arc<crate::program::inputs::WorkspaceInputs>,
        table: &'a str,
        from: Option<&'a str>,
        direction: Direction,
        alsem_ver: &'a str,
        deterministic: bool,
        single_app: bool,
    ) -> QueryJob<'a> {
        let ask = QueryAsk::Touches {
            table,
            from,
            direction,
        };
        Self::with(inputs, ask, alsem_ver, deterministic, single_app)
    }

    /// `alsem query effects` (see [`run_query_effects_pipeline`]).
    pub fn effects(
        inputs: &'a std::sync::Arc<crate::program::inputs::WorkspaceInputs>,
        routine: &'a str,
        alsem_ver: &'a str,
        deterministic: bool,
        single_app: bool,
    ) -> QueryJob<'a> {
        let ask = QueryAsk::Effects { routine };
        Self::with(inputs, ask, alsem_ver, deterministic, single_app)
    }

    fn with(
        inputs: &'a std::sync::Arc<crate::program::inputs::WorkspaceInputs>,
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
        }
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
    fn demands(&self) -> Vec<ViewDemand> {
        vec![ViewDemand {
            key: ViewKey::new(&self.inputs.workspace, self.single_app, false),
            side: SideIx(0),
            needs: ViewNeeds::MODEL,
            inputs: std::sync::Arc::clone(self.inputs),
        }]
    }

    fn resolve(&self, _: SideIx, _: &ResolveCx<'_>) -> Result<PlanPart, JobFailure> {
        Ok(PlanPart {
            plan: query_plan(),
            reads: vec![Cell::Rel, Cell::L4],
            carry: None,
        })
    }

    fn compute(&self, _: SideIx, cx: &mut ViewCx<'_>) -> Result<SideDoc, JobFailure> {
        let substrate = QuerySubstrate::of(cx.view());
        let result = match self.ask {
            QueryAsk::Touches {
                table,
                from,
                direction,
            } => render_query_touches(
                &substrate,
                table,
                from,
                direction,
                self.alsem_ver,
                self.deterministic,
            ),
            QueryAsk::Effects { routine } => {
                render_query_effects(&substrate, routine, self.alsem_ver, self.deterministic)
            }
        };
        Ok(SideDoc::new(Ok::<_, String>(result)))
    }

    fn view_failed(&self, _: SideIx, failure: &ViewFailure) -> Result<SideDoc, JobFailure> {
        Ok(SideDoc::new(Err::<QueryRunResult, _>(
            failure.target.message("query"),
        )))
    }

    fn serialize(&self, doc: Doc) -> JobOutput {
        match doc
            .one::<Result<QueryRunResult, String>>()
            .unwrap_or_else(|f| unreachable!("query fails no side: {f:?}"))
        {
            Err(msg) => JobOutput::lines(&[msg], 1),
            Ok(r) => JobOutput {
                actions: vec![WriteAction::out(
                    self.out,
                    if self.json { r.json_text } else { r.human_text },
                )],
                exit: r.exit_code,
            },
        }
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

/// `operation_id` -> the routine + record operation that produced it.
pub struct OperationJoin<'a> {
    by_id: HashMap<&'a str, (&'a ModelRoutine, &'a ModelRecordOperation)>,
}

impl<'a> OperationJoin<'a> {
    pub fn build(resolved: &'a Model) -> Self {
        let mut by_id: HashMap<&'a str, (&'a ModelRoutine, &'a ModelRecordOperation)> =
            HashMap::new();
        for r in &resolved.workspace.routines {
            for op in &r.record_operations {
                // Operation ids embed the owning routine's id, so only the rows
                // of a collided id share one; then the LAST owning row's
                // operation is the anchor (#160 step 11b §4.6, class P).
                by_id.insert(op.id.as_str(), (r, op));
            }
        }
        OperationJoin { by_id }
    }

    /// `None` when the operation lives outside the workspace routine set (a
    /// retained dependency leaf's own effect) — render the id, invent nothing.
    pub fn get(&self, operation_id: &str, routines: &RoutineJoin<'_>) -> Option<OperationRef> {
        let (r, op) = self.by_id.get(operation_id)?;
        Some(OperationRef {
            owner: routines.of(r),
            file: op.source_anchor.source_unit_id.clone(),
            line: op.source_anchor.start_line,
            column: op.source_anchor.start_column,
        })
    }
}

fn origin_json(origin: Option<&OperationRef>) -> Value {
    match origin {
        None => Value::Null,
        Some(o) => json!({
            "routine": routine_ref_json(&o.owner),
            "anchor": { "file": o.file, "line": o.line, "column": o.column },
        }),
    }
}

/// `Object::Routine at file:line` — the human rendering of an effect's origin.
fn origin_human(origin: Option<&OperationRef>, operation_id: &str) -> String {
    match origin {
        Some(o) => format!("{} at {}:{}", o.owner.qualified(), o.file, o.line),
        None => format!("(outside the workspace: {operation_id})"),
    }
}

fn routine_ref_json(rr: &RoutineRef) -> Value {
    json!({
        "display": rr.display,
        "objectDisplay": rr.object_display,
        "stableId": rr.stable_id,
        "anchor": { "file": rr.file, "line": rr.line, "column": rr.column },
    })
}

/// The one renderer of the temp-state vocabulary (`query effects`, and
/// `digest`/`prove`'s `detail.tempState`).
pub fn temp_state_str(t: &TempStateKind) -> String {
    match t {
        TempStateKind::Known(true) => "known(true)".to_string(),
        TempStateKind::Known(false) => "known(false)".to_string(),
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
pub fn run_query_touches_pipeline(
    inputs: &std::sync::Arc<crate::program::inputs::WorkspaceInputs>,
    table_selector: &str,
    from: Option<&str>,
    direction: Direction,
    alsem_ver: &str,
    deterministic: bool,
    single_app: bool,
) -> Result<QueryRunResult, String> {
    QueryJob::touches(
        inputs,
        table_selector,
        from,
        direction,
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
    alsem_ver: &str,
    deterministic: bool,
) -> QueryRunResult {
    with_dependencies_line(
        render_query_touches_body(
            substrate,
            table_selector,
            from,
            direction,
            alsem_ver,
            deterministic,
        ),
        &substrate.dependencies,
    )
}

/// Human output names the loaded dependency apps on its first line, only when
/// there are some (a workspace with no dependency reads the same in both modes).
fn with_dependencies_line(mut r: QueryRunResult, deps: &LoadedDependencies) -> QueryRunResult {
    r.human_text = deps.prefix_human(r.human_text);
    r
}

fn render_query_touches_body(
    substrate: &QuerySubstrate,
    table_selector: &str,
    from: Option<&str>,
    direction: Direction,
    alsem_ver: &str,
    deterministic: bool,
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
            for &ix in &routines {
                let internal = query.bundle().routine_id(ix);
                match join.get(internal) {
                    Some(rr) => human.push_str(&format!(
                        "    {} — {}:{}\n",
                        rr.qualified(),
                        rr.file,
                        rr.line
                    )),
                    None => human.push_str(&format!("    {internal}\n")),
                }
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
                };
            };

            human.push_str(&format!("--- {} vs {table_label} ---\n", rr.qualified()));

            let touches_down = query.touches_table(rix, &table.id);
            if direction.wants_down() {
                let witnesses = query.touches(rix, &table.id);
                payload.insert(
                    "down".into(),
                    json!({
                        "answerable": true,
                        "touches": touches_down,
                        "witnessCount": witnesses.len(),
                        "witnesses": witnesses
                            .iter()
                            .map(|t| touch_json(t, ops.get(t.operation_id, &join).as_ref(), None))
                            .collect::<Vec<_>>(),
                    }),
                );
                human.push_str(&format!(
                    "  down: {} ({} witness(es))\n",
                    if touches_down { "yes" } else { "no" },
                    witnesses.len()
                ));
                for w in &witnesses {
                    human.push_str(&format!(
                        "    [{}] via {}, temp {} — {}\n",
                        w.op,
                        w.via.as_str(),
                        temp_state_str(w.temp_state),
                        origin_human(ops.get(w.operation_id, &join).as_ref(), w.operation_id)
                    ));
                }
            }

            if direction.wants_up() {
                let all_ancestors = query.ancestors(rix);
                let ups: Vec<AncestorTouch<'_>> = query.ancestors_touching(rix, &table.id);
                let mut touching_routines: Vec<_> =
                    ups.iter().map(|a| a.touch.routine).collect::<Vec<_>>();
                touching_routines.sort_unstable();
                touching_routines.dedup();

                let rows: Vec<Value> = ups
                    .iter()
                    .map(|a| {
                        let internal = query.bundle().routine_id(a.touch.routine);
                        let mut m = Map::new();
                        m.insert("depth".into(), a.depth.into());
                        m.insert(
                            "routine".into(),
                            match join.get(internal) {
                                Some(x) => routine_ref_json(&x),
                                None => json!({ "display": internal, "objectDisplay": "", "stableId": Value::Null, "anchor": Value::Null }),
                            },
                        );
                        let origin = ops.get(a.touch.operation_id, &join);
                        if let Value::Object(t) = touch_json(&a.touch, origin.as_ref(), None) {
                            for (k, v) in t {
                                m.insert(k, v);
                            }
                        }
                        Value::Object(m)
                    })
                    .collect();

                // The answer-shaping rule: summaries are transitive-DOWN, so
                // when R itself touches T every ancestor trivially does. State
                // that instead of pretending the enumeration is a finding.
                let note = if touches_down {
                    "this routine itself touches the table, so every transitive caller does too \
                     (summaries are transitive-down) — the ancestor list is not a finding here"
                } else {
                    "this routine does NOT touch the table; these callers reach it through other \
                     branches"
                };
                payload.insert(
                    "up".into(),
                    json!({
                        "scoped": true,
                        "transitiveCallers": all_ancestors.len(),
                        "callersTouching": touching_routines.len(),
                        "informative": !touches_down,
                        "note": note,
                        "witnesses": rows,
                    }),
                );
                human.push_str(&format!(
                    "  up: {} of {} transitive caller(s) touch it{}\n",
                    touching_routines.len(),
                    all_ancestors.len(),
                    if touches_down {
                        " (trivially — this routine touches it itself)"
                    } else {
                        " through other branches"
                    }
                ));
                for a in &ups {
                    let internal = query.bundle().routine_id(a.touch.routine);
                    let label = match join.get(internal) {
                        Some(x) => x.qualified(),
                        None => internal.to_string(),
                    };
                    human.push_str(&format!(
                        "    +{} {} [{}] via {} — {}\n",
                        a.depth,
                        label,
                        a.touch.op,
                        a.touch.via.as_str(),
                        origin_human(
                            ops.get(a.touch.operation_id, &join).as_ref(),
                            a.touch.operation_id
                        )
                    ));
                }
            }
        }
    }

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
    alsem_ver: &str,
    deterministic: bool,
) -> QueryRunResult {
    with_dependencies_line(
        render_query_effects_body(substrate, routine_selector, alsem_ver, deterministic),
        &substrate.dependencies,
    )
}

fn render_query_effects_body(
    substrate: &QuerySubstrate,
    routine_selector: &str,
    alsem_ver: &str,
    deterministic: bool,
) -> QueryRunResult {
    let deps = &substrate.dependencies;
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
        };
    };
    let effects: Vec<TableTouch<'_>> = query.all_effects(rix);

    let payload = json!({
        "routine": {
            "selector": routine_selector,
            "resolution": "resolved",
            "routine": routine_ref_json(&rr),
        },
        "effectCount": effects.len(),
        "effects": effects
            .iter()
            .map(|t| {
                touch_json(
                    t,
                    ops.get(t.operation_id, &join).as_ref(),
                    Some(tables.get(t.table_id)),
                )
            })
            .collect::<Vec<_>>(),
    });

    let mut human = format!("--- effects of {} ---\n", rr.qualified());
    human.push_str(&format!("  {} effect(s)\n", effects.len()));
    for e in &effects {
        let table = if is_unknown_table(e.table_id) {
            format!("{UNKNOWN_TABLE_ID} (target table unresolved — not a table)")
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

    QueryRunResult {
        json_text: envelope("query.effects", payload, deps, alsem_ver, deterministic),
        human_text: human,
        exit_code: 0,
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

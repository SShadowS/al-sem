//! `alsem query` — the shipped transport over
//! [`crate::engine::l4::effect_query::DbEffectQuery`], and the home of the
//! `RoutineIx -> L3Routine` join that turns an index answer into a user-facing
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
//! Like `digest` and `prove`, this owns its own one-shot pipeline (assemble →
//! symbols → resolve_calls → event graph → combined graph → Tarjan →
//! `compute_summaries_v2_bundle_with_leaves` → `DbEffectQuery`). It never
//! touches a `DetectorContext`, so it costs `alsem analyze` exactly nothing
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
use std::path::Path;

use serde_json::{Map, Value, json};

use crate::engine::gate::format_json::{pinned_or_now_iso8601, serialize_document_value};
use crate::engine::l3::call_resolver::calls_for;
use crate::engine::l3::event_graph::events_for;
use crate::engine::l3::l3_workspace::{
    L3RecordOperation, L3Resolved, L3Routine, table_by_id_preferring_real,
};
use crate::engine::l3::symbol_table::SymbolTable;
use crate::engine::l4::combined_graph::{CombinedGraph, build_combined_graph};
use crate::engine::l4::effect_lattice::TempStateKind;
use crate::engine::l4::effect_query::{
    AncestorTouch, DbEffectQuery, TableTouch, UNKNOWN_TABLE_ID, is_unknown_table,
};
use crate::engine::l4::effect_store::SummaryBundle;
use crate::engine::l4::scc::{SccInputGraph, SccResult, tarjan_scc};
use crate::engine::l4::summary::RoutineSummary;
use crate::engine::l4::summary_runner::{FieldIndex, compute_summaries_v2_bundle_with_leaves};

/// The JSON envelope's `schemaVersion` for both `query` subcommands.
const QUERY_SCHEMA_VERSION: &str = "1.0.0";

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

/// The owned inputs a [`DbEffectQuery`] borrows. Held by value because the
/// query surface borrows the bundle AND the Tarjan result, and both must
/// outlive it.
pub struct QuerySubstrate {
    pub resolved: L3Resolved,
    pub graph: CombinedGraph,
    pub scc: SccResult,
    pub bundle: SummaryBundle,
}

impl QuerySubstrate {
    /// Assemble the workspace and run the L3→L4 substrate this query needs.
    /// The same assembly `cdo_whole_program_v2_matches_frozen_digest` uses,
    /// swapped to the BUNDLE entry point so the compact rows survive (the
    /// materializing `_core` shim would expand and then discard them).
    pub fn build(workspace: &Path, model_instance_id: &str) -> Result<Self, String> {
        // Engine-switch S6.7: the program-backed model (program calls and events).
        let resolved = crate::engine::l3::program_calls::assemble_and_resolve_workspace_program(
            workspace,
            model_instance_id,
            false,
        )
        .ok_or_else(|| "query: workspace did not resolve".to_string())?;
        Ok(Self::from_resolved(resolved))
    }

    /// The substrate steps alone, over an already-assembled workspace — the
    /// seam the differential reuses so it exercises this exact assembly rather
    /// than a parallel copy of it.
    pub fn from_resolved(resolved: L3Resolved) -> Self {
        let (graph, scc, bundle) = {
            let ws = &resolved.workspace;
            let symbols = SymbolTable::build(&ws.objects, &ws.tables, &ws.routines);
            // The resolved model's own calls and event graph (the program
            // engine's when attached; L3's source-only ones otherwise).
            let calls = calls_for(&resolved, &symbols);
            let event_graph = events_for(&resolved, &symbols);
            let graph = build_combined_graph(ws, &calls, &event_graph);

            let mut scc_adjacency: HashMap<String, Vec<String>> = HashMap::new();
            for (from, list) in &graph.edges_by_from {
                scc_adjacency.insert(from.clone(), list.iter().map(|e| e.to.clone()).collect());
            }
            let scc = tarjan_scc(&SccInputGraph {
                nodes: &graph.nodes,
                edges_by_from: &scc_adjacency,
            });

            let mut field_index: FieldIndex = HashMap::new();
            for table in &ws.tables {
                for field in &table.fields {
                    field_index
                        .entry((table.id.clone(), field.name.to_lowercase()))
                        .or_insert_with(|| field.id.clone());
                }
            }
            let no_leaves: HashMap<String, RoutineSummary> = HashMap::new();
            let (bundle, _map, _diags) = compute_summaries_v2_bundle_with_leaves(
                &ws.routines,
                &graph,
                &scc,
                &calls.upgraded_bindings,
                &field_index,
                &no_leaves,
            );
            (graph, scc, bundle)
        };
        QuerySubstrate {
            resolved,
            graph,
            scc,
            bundle,
        }
    }

    /// The query surface over this substrate.
    pub fn query(&self) -> DbEffectQuery<'_> {
        DbEffectQuery::build(&self.bundle, &self.scc, &self.graph)
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
/// [`UNKNOWN_TABLE_ID`] sentinel, or a case-insensitive table NAME.
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
pub fn resolve_table_selector(resolved: &L3Resolved, selector: &str) -> Resolution<TableRef> {
    if selector == UNKNOWN_TABLE_ID {
        return Resolution::Resolved(TableRef {
            id: UNKNOWN_TABLE_ID.to_string(),
            name: None,
        });
    }
    let tables = &resolved.workspace.tables;
    let real_by_id = table_by_id_preferring_real(tables);

    if let Some(t) = real_by_id.get(selector) {
        return Resolution::Resolved(TableRef {
            id: t.id.clone(),
            name: Some(t.name.clone()),
        });
    }

    let want = selector.to_lowercase();
    let mut hits: Vec<&&crate::engine::l3::l3_workspace::L3Table> = real_by_id
        .values()
        .filter(|t| t.name.to_lowercase() == want)
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
pub fn resolve_routine_selector<'a>(
    resolved: &'a L3Resolved,
    selector: &str,
) -> Resolution<&'a L3Routine> {
    let ws = &resolved.workspace;
    let object_name_by_id: HashMap<&str, &str> = ws
        .objects
        .iter()
        .map(|o| (o.id.as_str(), o.name.as_str()))
        .collect();

    if let Some(r) = ws.routines.iter().find(|r| r.id == selector) {
        return Resolution::Resolved(r);
    }
    if let Some(r) = ws.routines.iter().find(|r| r.stable_routine_id == selector) {
        return Resolution::Resolved(r);
    }

    let want = selector.to_lowercase();
    let qualified: Vec<&L3Routine> = ws
        .routines
        .iter()
        .filter(|r| {
            let obj = object_name_by_id.get(r.object_id.as_str()).unwrap_or(&"");
            format!("{obj}.{}", r.name).to_lowercase() == want
        })
        .collect();
    let bare: Vec<&L3Routine> = ws
        .routines
        .iter()
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
                .map(|r| {
                    let obj = object_name_by_id.get(r.object_id.as_str()).unwrap_or(&"");
                    format!("{}.{} ({})", obj, r.name, r.stable_routine_id)
                })
                .collect(),
        ),
    }
}

// ---------------------------------------------------------------------------
// The RoutineIx -> L3Routine join (scope §1.5: RoutineIx is not an identity).
// ---------------------------------------------------------------------------

/// The user-facing identity of a routine. `RoutineIx` is an internal dense
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

/// The join table: internal routine id -> its `L3Routine`, plus object names.
pub struct RoutineJoin<'a> {
    by_id: HashMap<&'a str, &'a L3Routine>,
    object_name_by_id: HashMap<&'a str, &'a str>,
}

impl<'a> RoutineJoin<'a> {
    pub fn build(resolved: &'a L3Resolved) -> Self {
        let ws = &resolved.workspace;
        RoutineJoin {
            by_id: ws.routines.iter().map(|r| (r.id.as_str(), r)).collect(),
            object_name_by_id: ws
                .objects
                .iter()
                .map(|o| (o.id.as_str(), o.name.as_str()))
                .collect(),
        }
    }

    /// Join an internal routine id to its renderable identity. `None` for an id
    /// that is not a workspace routine (e.g. a retained dependency leaf) —
    /// callers render a placeholder rather than inventing a name.
    pub fn get(&self, routine_id: &str) -> Option<RoutineRef> {
        let r = self.by_id.get(routine_id)?;
        Some(self.of(r))
    }

    pub fn of(&self, r: &L3Routine) -> RoutineRef {
        let object_name = self
            .object_name_by_id
            .get(r.object_id.as_str())
            .copied()
            .unwrap_or("");
        RoutineRef {
            display: r.name.clone(),
            object_display: format!("{} {}", r.object_type, object_name)
                .trim()
                .to_string(),
            stable_id: r.stable_routine_id.clone(),
            file: r.source_anchor.source_unit_id.clone(),
            line: r.source_anchor.start_line,
            column: r.source_anchor.start_column,
        }
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
    by_id: HashMap<&'a str, (&'a L3Routine, &'a L3RecordOperation)>,
}

impl<'a> OperationJoin<'a> {
    pub fn build(resolved: &'a L3Resolved) -> Self {
        let mut by_id: HashMap<&'a str, (&'a L3Routine, &'a L3RecordOperation)> = HashMap::new();
        for r in &resolved.workspace.routines {
            for op in &r.record_operations {
                // First writer wins: operation ids are unique per workspace by
                // construction (they embed the owning routine's id), so a
                // collision would be a bug upstream — never silently prefer the
                // later one, which would move the anchor without saying so.
                by_id.entry(op.id.as_str()).or_insert((r, op));
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
        Some(o) => format!(
            "{}::{} at {}:{}",
            o.owner.object_display, o.owner.display, o.file, o.line
        ),
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

fn temp_state_str(t: &TempStateKind) -> String {
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
    fn build(resolved: &'a L3Resolved) -> Self {
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

fn envelope(kind: &str, payload: Value, alsem_ver: &str, deterministic: bool) -> String {
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
        json_text: envelope(kind, payload, alsem_ver, deterministic),
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
    workspace: &Path,
    table_selector: &str,
    from: Option<&str>,
    direction: Direction,
    alsem_ver: &str,
    deterministic: bool,
) -> Result<QueryRunResult, String> {
    use crate::engine::gate::model_instance_id::compute_gate_model_instance_id;
    let model_id = compute_gate_model_instance_id(workspace)
        .ok_or_else(|| "query: could not compute modelInstanceId".to_string())?;
    let substrate = QuerySubstrate::build(workspace, &model_id)?;
    Ok(render_query_touches(
        &substrate,
        table_selector,
        from,
        direction,
        alsem_ver,
        deterministic,
    ))
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
    if from.is_none() && direction == Direction::Down {
        let payload = json!({
            "direction": direction.as_str(),
            "from": Value::Null,
            "error": "a `down` query is about one routine's cone — pass --from <routine>, \
                      or omit --direction for the workspace-global list",
        });
        return QueryRunResult {
            json_text: envelope("query.touches", payload, alsem_ver, deterministic),
            human_text: "query.touches: --direction down requires --from <routine>\n".to_string(),
            exit_code: 2,
        };
    }
    let table = match resolve_table_selector(&substrate.resolved, table_selector) {
        Resolution::Resolved(t) => t,
        other => {
            return selector_failure_doc(
                "query.touches",
                "table",
                table_selector,
                &other,
                alsem_ver,
                deterministic,
            );
        }
    };

    let from_routine = match from {
        None => None,
        Some(sel) => match resolve_routine_selector(&substrate.resolved, sel) {
            Resolution::Resolved(r) => Some(r),
            other => {
                return selector_failure_doc(
                    "query.touches",
                    "from",
                    sel,
                    &other,
                    alsem_ver,
                    deterministic,
                );
            }
        },
    };

    let query = substrate.query();
    let join = RoutineJoin::build(&substrate.resolved);
    let ops = OperationJoin::build(&substrate.resolved);

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
            let routines = query.routines_touching(&table.id);
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
                        "    {}::{} — {}:{}\n",
                        rr.object_display, rr.display, rr.file, rr.line
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
                    "--- {}::{} vs {table_label} ---\n  not answerable: routine has no db-effect row\n",
                    rr.object_display, rr.display
                ));
                return QueryRunResult {
                    json_text: envelope(
                        "query.touches",
                        Value::Object(payload),
                        alsem_ver,
                        deterministic,
                    ),
                    human_text: human,
                    exit_code: 0,
                };
            };

            human.push_str(&format!(
                "--- {}::{} vs {table_label} ---\n",
                rr.object_display, rr.display
            ));

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
                        Some(x) => format!("{}::{}", x.object_display, x.display),
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
    workspace: &Path,
    routine_selector: &str,
    alsem_ver: &str,
    deterministic: bool,
) -> Result<QueryRunResult, String> {
    use crate::engine::gate::model_instance_id::compute_gate_model_instance_id;
    let model_id = compute_gate_model_instance_id(workspace)
        .ok_or_else(|| "query: could not compute modelInstanceId".to_string())?;
    let substrate = QuerySubstrate::build(workspace, &model_id)?;
    Ok(render_query_effects(
        &substrate,
        routine_selector,
        alsem_ver,
        deterministic,
    ))
}

/// The pure rendering half of [`run_query_effects_pipeline`].
pub fn render_query_effects(
    substrate: &QuerySubstrate,
    routine_selector: &str,
    alsem_ver: &str,
    deterministic: bool,
) -> QueryRunResult {
    let routine = match resolve_routine_selector(&substrate.resolved, routine_selector) {
        Resolution::Resolved(r) => r,
        other => {
            return selector_failure_doc(
                "query.effects",
                "routine",
                routine_selector,
                &other,
                alsem_ver,
                deterministic,
            );
        }
    };

    let query = substrate.query();
    let join = RoutineJoin::build(&substrate.resolved);
    let ops = OperationJoin::build(&substrate.resolved);
    let tables = TableNames::build(&substrate.resolved);
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
            "--- effects of {}::{} ---\n  not answerable: routine has no db-effect row\n",
            rr.object_display, rr.display
        );
        return QueryRunResult {
            json_text: envelope("query.effects", payload, alsem_ver, deterministic),
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

    let mut human = format!("--- effects of {}::{} ---\n", rr.object_display, rr.display);
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
        json_text: envelope("query.effects", payload, alsem_ver, deterministic),
        human_text: human,
        exit_code: 0,
    }
}

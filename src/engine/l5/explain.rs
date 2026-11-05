//! `alsem explain` (intent-evidence spec
//! `docs/superpowers/specs/2026-10-29-intent-evidence-design.md` §4.4): one
//! evidence card per routine, with the routine's own rules, what it does (the
//! digest effects), how a user reaches it (page actions), the events it
//! raises, the tests that reach it, and the gaps (what the engine could not
//! see).
//!
//! Three ways to name the routines:
//!
//! - `--routine <selector>`: the one routine `prove` would resolve, by the same
//!   selector code (`digest_cli::resolve_changed_roots` with one routine
//!   selector): no match and an ambiguous match both exit 2, the family's
//!   no-root code, with a diagnostic that says which. Repeatable
//!   (context-server design r3 §6.12): each selector resolves alone, the cards
//!   are the union (sorted, each routine once; a selector given twice is one),
//!   the diagnostics are per selector in input order, exit 0 when any card
//!   and 2 when none. One distinct selector writes the single-routine document
//!   (`"selector"`) byte for byte as before; two or more write `"selectors"`
//!   in its place, the rest of the document as `--diff` writes it.
//! - `--diff <patch>`: every routine `digest --diff` resolves from the same
//!   diff, by the same call with the same diff root, so the two commands see
//!   the same changed set. A diff that resolves no routine exits 2.
//! - `--table <T> --field <F>`: every workspace routine that writes `T.F`
//!   ([`crate::engine::l5::field_writers`]), one card each. The table selector
//!   is `query touches`' (`resolve_table_selector`). An unknown table or field
//!   exits 2; a known field with no writer is an answer (exit 0, no card).
//!
//! **Rules across calls** (step 5, [`crate::engine::l5::cross_rules`]):
//! `rules.inherited` lists the callee rules that hold before the routine's
//! first write and the table-trigger rules its writes run (each with the write
//! named); `rules.callers` lists the rules its known callers run before
//! calling it, with who applies and who skips each. Table triggers a write
//! skips (`Modify(false)`, a plain assignment) are a `table-trigger-rules-skipped`
//! gap with the skipped rules; a call to an error collector is an
//! `error-collector` gap; event subscribers' own rules are listed under their
//! extension point (`subscriberRoutines`), not merged into `inherited`;
//! `reachedFrom` also follows a page action's `RunObject = Codeunit X` into
//! that codeunit's `OnRun`.
//!
//! The command builds the model with `ViewNeeds::EXPLAIN` (rules and card
//! text, both built at assembly) and computes over it with `explain_plan`: one
//! tree over the cards' routines, read as digest's per-root effects with the
//! test-coverage graph. No detector run and no `l4`.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};

use serde_json::{Map, Value, json};

use crate::engine::batch::job::{
    Doc, Job, JobFailure, PlanPart, ResolveCx, SideDoc, SideIx, ViewCx, ViewDemand, ViewFailure,
    ViewNeeds, WorkspaceRead,
};
use crate::engine::batch::output::JobOutput;
use crate::engine::gate::format_json::{serialize_normalized_value, sort_keys_keep_nulls};
use crate::engine::l4::effect_query_cli::{
    OwnerIndex, Resolution, object_display, owner_qualified, resolve_table_selector,
};
use crate::engine::l5::cross_rules::{
    CallerRule, CrossRules, CrossRulesIndex, EXPANSION_BUDGET, Inh, MAX_DEPTH,
};
use crate::engine::l5::digest_cli::{
    ChangedInput, ChangedRootsDiagnostic, ChangedRootsResult, DbTempStates, DiffRoot,
    DigestEffectFull, DigestEffects, DigestEntryFull, FullAnchor, anchor_to_value,
    changed_roots_diagnostic_human, changed_roots_diagnostic_to_value, primary_guid_of,
    resolve_changed_roots, run_digest_query_full_from_entries,
};
use crate::engine::l5::field_writers::{WriteKind, field_writes};
use crate::engine::l5::prove::TableAnswers;
use crate::engine::l5::rollup::classify_detail;
use crate::engine::l5::routine_rollup::{
    FAMILIES, OTHER_KINDS, RenderOpts, RollupCx, RollupTotals, RoutineRollup, TableWrites,
    beyond_lines, caller_dependent_lines, caps, class_json, commit_lines, crossing_key,
    dependency_gap_lines, event_lines, family_of, fold, header_lines, is_group, kind_lines,
    literal_dead_lines, no_path_lines, plural as rf_plural, temporary_lines, workspace_write_lines,
};
use crate::engine::l5::rules_index::{FieldWriters, RulesIndex, WriteSite, Writer as IndexWriter};
use crate::engine::session::{Cell, explain_plan};
use crate::program::body::features::PAnchor;
use crate::program::body::rules::{
    CmpOp, ConditionAtom, MessageSource, PRule, RuleKind, RuleMessage, Stops,
};
use crate::program::model::declaring::DeclaringRows;
use crate::program::model::role::Role;
use crate::program::model::workspace::{
    CardAction, Model, ModelField, ModelObject, ModelRoutine, ModelTable,
    table_by_id_preferring_real,
};
use al_syntax::IdentifierFoldExt;

/// The JSON schema id of the document. `/2` (MCP output plan 1a): every
/// `line` is 1-based, and the document says so (`lineBase: 1`). `/2.1`
/// (MCP output wave 2, W2-1, additive): a write, commit or event the root
/// cannot reach under literal arguments carries `literalUnreachable: true`.
/// `/3` (MCP output wave 2, W2-2, design §4.2): `cards[].summary` (the
/// rollup's `RollupTotals`), `coverage {status, reasons, assumptions,
/// limits, notes}`; each write carries `class`, `classReason`, `opKinds` and
/// its `tempState`; a group row names its `crossing` (listed once in
/// `effects.crossings`) with `resourceCount` and every resource, never an
/// empty `table`; an event group carries its count (`resourcesListed:
/// false`; the names with `--detail`); a write with no witness carries
/// `noPathKept` and the recovered `owner`; every kind family has its list;
/// `reachedFromSummary` and `gapsInDependencies`; dependency gaps and a page
/// action's `via` / `tooltip` only with `--detail`.
pub const SCHEMA: &str = "al-sem.explain/3";

/// How the command names its routines.
#[derive(Debug, Clone)]
pub enum ExplainMode {
    /// `--routine <selector>`, given once or more (context-server design r3
    /// §6.12): never empty.
    Routines(Vec<String>),
    /// `--diff <patch>`: the diff's text and what its paths are relative to.
    Diff { text: String, root: DiffRoot },
    /// `--table <T> --field <F>`.
    Field { table: String, field: String },
}

/// The command's inputs.
pub struct ExplainRequest<'a> {
    /// Handed to the driver; compute reads `ViewCx::inputs` (#277 R7-1).
    pub inputs: &'a dyn WorkspaceRead,
    pub mode: ExplainMode,
    pub single_app: bool,
}

/// `alsem explain` as a batch job (#277 D1). Its side document is
/// `Result<ExplainRunResult, String>`.
pub struct ExplainJob<'a> {
    req: ExplainRequest<'a>,
    /// `--format human`: the human text, else the JSON.
    human: bool,
    out: Option<&'a str>,
    /// `--detail` and `--max-paths` (design §5).
    opts: RenderOpts,
    /// `--show-source` (W2-7): the cited line ±2 under each human location.
    show_source: bool,
    /// `--writes-table T` (W2-2 gap (a)): prove's answer for each table.
    writes_tables: Vec<String>,
}

/// The command's two renderings and its exit code.
pub struct ExplainRunResult {
    pub json_text: String,
    pub human_text: String,
    /// 0: an answer (cards, or a resolved field with no writer); 2: no routine
    /// resolved, or an unknown table or field.
    pub exit_code: u8,
    /// The cards' summary for a file answer (MCP output plan 1e).
    pub summary: Value,
}

impl<'a> ExplainJob<'a> {
    #[must_use]
    pub fn new(req: ExplainRequest<'a>, human: bool, out: Option<&'a str>) -> ExplainJob<'a> {
        ExplainJob {
            req,
            human,
            out,
            opts: RenderOpts::new(false, None),
            show_source: false,
            writes_tables: Vec::new(),
        }
    }

    /// `--writes-table T`, repeated (W2-2 gap (a)).
    #[must_use]
    pub fn with_writes_tables(mut self, tables: Vec<String>) -> ExplainJob<'a> {
        self.writes_tables = tables;
        self
    }

    /// `--show-source` (W2-7).
    #[must_use]
    pub fn with_show_source(mut self, show_source: bool) -> ExplainJob<'a> {
        self.show_source = show_source;
        self
    }

    /// `--detail` (no fold, every list) and `--max-paths N` (design §5).
    #[must_use]
    pub fn with_render(mut self, detail: bool, max_paths: Option<usize>) -> ExplainJob<'a> {
        self.opts = RenderOpts::new(detail, max_paths);
        self
    }
}

/// One diagnostic of the resolve step: its JSON object and its human line.
struct Diagnostic {
    json: Value,
    human: String,
}

/// `--table/--field`'s resolved target and its writers.
struct FieldAnswer {
    table: String,
    table_id: String,
    field: String,
    captions: Vec<String>,
    /// Per writer (stable id, in id order): its write sites, in anchor order.
    writers: BTreeMap<String, Vec<(WriteKind, PAnchor)>>,
}

/// What `resolve` hands to `compute`.
struct Resolved {
    /// The cards' routines: stable ids, sorted, distinct.
    roots: Vec<String>,
    diagnostics: Vec<Diagnostic>,
    field: Option<FieldAnswer>,
    /// The mode's own answer is "nothing found" (exit 2).
    unresolved: bool,
}

impl Job for ExplainJob<'_> {
    fn name(&self) -> &'static str {
        "explain"
    }

    fn demands(&self) -> Vec<ViewDemand> {
        ViewDemand::one(
            self.req.inputs,
            self.req.single_app,
            false,
            ViewNeeds::EXPLAIN,
        )
    }

    fn resolve(&self, _: SideIx, cx: &ResolveCx<'_>) -> Result<PlanPart, JobFailure> {
        let model = cx.model();
        let resolved = match &self.req.mode {
            ExplainMode::Routines(selectors) => {
                // Each selector resolves alone, as one `--routine` always did
                // (prove's code and wording, G42); the cards are the union,
                // sorted and distinct, the diagnostics per selector in input
                // order. A selector given twice is one selector.
                let owners = OwnerIndex::build(model);
                let mut roots: BTreeSet<String> = BTreeSet::new();
                let mut diagnostics = Vec::new();
                for selector in distinct(selectors) {
                    let changed = resolve_changed_roots(
                        cx.ids(),
                        model,
                        &ChangedInput {
                            files: None,
                            routines: Some(vec![selector.clone()]),
                            diff_text: None,
                            diff_root: DiffRoot::default(),
                            workspace_files: &[],
                        },
                        primary_guid_of(model),
                        &owners,
                    );
                    diagnostics.extend(
                        changed
                            .diagnostics
                            .iter()
                            .map(|d| selector_diagnostic(selector, d)),
                    );
                    roots.extend(changed.roots);
                }
                Resolved {
                    unresolved: roots.is_empty(),
                    roots: roots.into_iter().collect(),
                    diagnostics,
                    field: None,
                }
            }
            ExplainMode::Diff { text, root } => {
                // `digest --diff`'s resolution, with digest's inputs: the same
                // changed set.
                let changed = resolve_changed_roots(
                    cx.ids(),
                    model,
                    &ChangedInput {
                        files: None,
                        routines: None,
                        diff_text: Some(text.clone()),
                        diff_root: root.clone(),
                        workspace_files: &cx.inputs().al_files,
                    },
                    primary_guid_of(model),
                    &OwnerIndex::build(model),
                );
                let diagnostics = changed
                    .diagnostics
                    .iter()
                    .map(|d| Diagnostic {
                        json: changed_roots_diagnostic_to_value(d),
                        human: changed_roots_diagnostic_human(d),
                    })
                    .collect();
                Resolved {
                    unresolved: changed.roots.is_empty(),
                    roots: changed.roots,
                    diagnostics,
                    field: None,
                }
            }
            ExplainMode::Field { table, field } => resolve_field(model, table, field),
        };
        // W2-2: a plan with a tree also declares `l4`, the temp-state join's
        // store (explain_plan); a plan with none reads neither.
        let reads = if resolved.roots.is_empty() {
            vec![Cell::Effects]
        } else {
            vec![Cell::Effects, Cell::L4]
        };
        Ok(PlanPart {
            // `--writes-table` reads the cones' gap counts (declared needs:
            // only then are they counted).
            plan: if self.writes_tables.is_empty() {
                explain_plan(resolved.roots.iter().cloned().collect())
            } else {
                crate::engine::session::counting_cone_gaps(explain_plan(
                    resolved.roots.iter().cloned().collect(),
                ))
            },
            reads,
            carry: Some(Box::new(resolved)),
        })
    }

    fn compute(&self, _: SideIx, cx: &mut ViewCx<'_>) -> Result<SideDoc, JobFailure> {
        let workspace = cx.inputs().workspace.clone();
        let resolved: Resolved = cx.take_carry();
        let has_tree = cx.has_tree();
        let mut effects = if has_tree {
            cx.take_effects()
        } else {
            DigestEffects::default()
        };
        let view = cx.view();
        // A served view carries its revision's indexes (plan 2026-11-03
        // W2): never build a second copy behind the artifact's back.
        let prepared = view.explain_indexes();
        assert!(
            prepared.is_some() || !view.is_served(),
            "a served explain view carries its revision's explain indexes"
        );
        let model = view.model();
        // W2-2 (design §3.1): the digest entries after the L4 temp-state join
        // (the plan declares `l4` with the tree), so each write has its class.
        let (full, table_answers) = if has_tree {
            let _s = crate::engine::perf_trace::span("explain", "explain.temp_join");
            let temp_states = DbTempStates::new(model, &view.l4().bundle);
            let changed = ChangedRootsResult {
                roots: resolved.roots.clone(),
                diagnostics: Vec::new(),
                touched: HashMap::new(),
                spans: Default::default(),
            };
            // The cones' facts a table answer reads, before the join takes
            // the cones.
            let cones = if self.writes_tables.is_empty() {
                HashMap::new()
            } else {
                crate::engine::l5::prove::cone_facts(&effects.unresolved)
            };
            let full = run_digest_query_full_from_entries(
                &mut effects,
                &resolved.roots,
                &HashMap::new(),
                &temp_states,
                &changed,
                &OwnerIndex::build(model),
            )
            .entries;
            // W2-2 gap (a): `--writes-table`, prove's own answer.
            let answers = crate::engine::l5::prove::table_answers(
                model,
                &temp_states,
                &full,
                &cones,
                &self.writes_tables,
            );
            (full, answers)
        } else {
            (Vec::new(), Default::default())
        };
        let apps = view.target().dependencies().apps;
        let sources = (self.show_source && self.human)
            .then(|| crate::engine::source_excerpt::SourceTexts::read(&workspace));
        let result = render(
            model,
            &self.req.mode,
            &resolved,
            &effects,
            &full,
            &apps,
            prepared,
            self.opts,
            sources.as_ref(),
            &table_answers,
        );
        drop(full);
        drop(effects);
        cx.release(Cell::Effects);
        if has_tree {
            cx.release(Cell::L4);
        }
        Ok(SideDoc::new(Ok::<_, String>(result)))
    }

    fn view_failed(&self, _: SideIx, failure: &ViewFailure) -> Result<SideDoc, JobFailure> {
        Ok(SideDoc::new(Err::<ExplainRunResult, _>(
            failure.target.message("explain"),
        )))
    }

    fn serialize(&self, doc: Doc) -> JobOutput {
        match doc.one::<Result<ExplainRunResult, String>>() {
            Err(f) => f,
            Ok(r) => JobOutput::text_with_summary(
                self.out,
                r.map(|r| {
                    let text = if self.human {
                        r.human_text
                    } else {
                        r.json_text
                    };
                    (text, r.exit_code, r.summary)
                }),
            ),
        }
    }
}

/// `selectors` in input order, each once.
fn distinct(selectors: &[String]) -> Vec<&String> {
    let mut seen = HashSet::new();
    selectors.iter().filter(|s| seen.insert(*s)).collect()
}

/// `--routine`'s diagnostic for one changed-roots diagnostic: prove's
/// `routine-not-found` / `routine-ambiguous` wording (G42).
fn selector_diagnostic(selector: &str, d: &ChangedRootsDiagnostic) -> Diagnostic {
    match d {
        ChangedRootsDiagnostic::SelectorAmbiguous { candidates, .. } => Diagnostic {
            json: json!({
                "kind": "routine-ambiguous",
                "selector": selector,
                "candidates": candidates,
            }),
            human: format!(
                "explain: '{selector}' names {} routines ({}); qualify it as \
                 <owner>::<routine> or pass a stable id",
                candidates.len(),
                candidates.join(", ")
            ),
        },
        ChangedRootsDiagnostic::SelectorUnmatched { .. } => Diagnostic {
            json: json!({ "kind": "routine-not-found", "selector": selector }),
            human: format!("explain: no routine of the program has the id or name '{selector}'"),
        },
        other => Diagnostic {
            json: changed_roots_diagnostic_to_value(other),
            human: changed_roots_diagnostic_human(other),
        },
    }
}

/// `--table/--field`: the table by `query touches`' selector, the field by
/// name (case-insensitive) or number, then every writer of it.
/// `--table/--field`'s target: the table by `query touches`' selector, the
/// field by name (case-insensitive) or number. `Err`: the diagnostic's JSON
/// and its human line (`explain --emit-policy` reads the same lookup).
pub(crate) fn find_field<'m>(
    model: &'m Model,
    table: &str,
    field: &str,
) -> Result<(&'m ModelTable, &'m ModelField), (Value, String)> {
    let t = match resolve_table_selector(model, table) {
        Resolution::Resolved(t) => t,
        Resolution::Unmatched => {
            return Err((
                json!({ "kind": "table-not-found", "table": table }),
                format!("explain: no table of the program has the name, number or id '{table}'"),
            ));
        }
        Resolution::Ambiguous(candidates) => {
            return Err((
                json!({ "kind": "table-ambiguous", "table": table, "candidates": candidates }),
                format!(
                    "explain: '{table}' names {} tables ({}); pass its id",
                    candidates.len(),
                    candidates.join(", ")
                ),
            ));
        }
    };
    let tables = table_by_id_preferring_real(&model.workspace.tables);
    let Some(mt) = tables.get(t.id.as_str()).copied() else {
        // The `unknown` bucket: not a table, so it has no fields.
        return Err((
            json!({ "kind": "field-not-found", "table": t.id, "field": field }),
            format!("explain: '{}' is not a table with fields", t.id),
        ));
    };
    let number: Option<i64> = field.trim().parse().ok();
    let want = field.trim().to_lowercase();
    let Some(mf) = mt
        .fields
        .iter()
        .find(|f| f.name.to_lowercase() == want)
        .or_else(|| number.and_then(|n| mt.fields.iter().find(|f| f.field_number == n)))
    else {
        return Err((
            json!({ "kind": "field-not-found", "table": mt.name, "field": field }),
            format!("explain: table '{}' has no field '{field}'", mt.name),
        ));
    };
    Ok((mt, mf))
}

fn resolve_field(model: &Model, table: &str, field: &str) -> Resolved {
    let (mt, mf) = match find_field(model, table, field) {
        Ok(found) => found,
        Err((json, human)) => {
            return Resolved {
                roots: vec![],
                diagnostics: vec![Diagnostic { json, human }],
                field: None,
                unresolved: true,
            };
        }
    };
    let mut writers: BTreeMap<String, Vec<(WriteKind, PAnchor)>> = BTreeMap::new();
    for w in field_writes(model) {
        if w.table_id == mt.id && w.field == mf.name {
            let r = &model.workspace.routines[w.routine_ix];
            writers
                .entry(r.stable_routine_id.clone())
                .or_default()
                .push((w.kind, w.anchor.clone()));
        }
    }
    for sites in writers.values_mut() {
        sites.sort_by(|a, b| anchor_key(&a.1).cmp(&anchor_key(&b.1)).then(a.0.cmp(&b.0)));
        sites.dedup_by(|a, b| a.0 == b.0 && anchor_key(&a.1) == anchor_key(&b.1));
    }
    Resolved {
        roots: writers.keys().cloned().collect(),
        diagnostics: vec![],
        field: Some(FieldAnswer {
            table: mt.name.clone(),
            table_id: mt.id.clone(),
            field: mf.name.clone(),
            captions: mf.captions.clone(),
            writers,
        }),
        unresolved: false,
    }
}

/// An anchor's sort key: file, line, column.
fn anchor_key(a: &PAnchor) -> (&str, u32, u32) {
    (&a.source_unit_id, a.start_line, a.start_column)
}

/// A model anchor in the family's JSON shape (digest's `anchor_to_value`).
fn anchor_json(a: &PAnchor) -> Value {
    anchor_to_value(&FullAnchor {
        source_kind: "source",
        file: Some(a.source_unit_id.clone()),
        line: Some(a.start_line),
        column: Some(a.start_column),
        excerpt: None,
    })
}

/// `file:line`, as digest's human output prints an anchor (1-based line).
fn anchor_human(a: &PAnchor) -> String {
    format!(
        "{}:{}",
        a.source_unit_id,
        crate::engine::line_base::shown(a.start_line)
    )
}

/// One text read from the IR's per-`#if`-arm list: `Ok(None)` when there is
/// none, `Ok(Some)` when every arm agrees, `Err(texts)` when the arms differ
/// (the card then shows `null` and a `text-differs-by-directive` gap).
fn one_text(texts: &[String]) -> Result<Option<String>, Vec<String>> {
    let distinct: BTreeSet<&String> = texts.iter().collect();
    match distinct.len() {
        0 => Ok(None),
        1 => Ok(distinct.into_iter().next().cloned()),
        _ => Err(texts.to_vec()),
    }
}

/// One gap: its sort key (kind, anchor, detail) and its JSON object.
struct Gap {
    key: (String, String, u32, u32, String),
    json: Value,
}

impl Gap {
    fn new(kind: &str, anchor: Option<&PAnchor>, mut fields: Map<String, Value>) -> Gap {
        fields.insert("kind".into(), kind.into());
        let (file, line, col) = anchor.map_or((String::new(), 0, 0), |a| {
            (a.source_unit_id.clone(), a.start_line, a.start_column)
        });
        if let Some(a) = anchor {
            fields.insert("anchor".into(), anchor_json(a));
        }
        let json = Value::Object(fields);
        Gap {
            key: (kind.to_string(), file, line, col, json.to_string()),
            json,
        }
    }
}

/// Everything explain reads that is a function of the model alone (plan
/// 2026-11-03 W2): the model indexes every card reads and the cross-call
/// rules' index. Owned (rows, objects and actions as indexes, ids as owned
/// strings), so a resident revision keeps one beside its model, built once,
/// and every explain over the revision reads it; the one-shot run builds one
/// per run, with the same constructor. Never changed after
/// [`ExplainIndexes::build`]: the walk state a request changes (the
/// cross-call rules' memo and expansion budget) is the request's own
/// ([`CrossRules::over`]).
pub struct ExplainIndexes {
    ix: IndexData,
    cross: CrossRulesIndex,
}

impl ExplainIndexes {
    /// The indexes of `model`.
    #[must_use]
    pub fn build(model: &Model) -> ExplainIndexes {
        ExplainIndexes {
            ix: IndexData::new(model),
            cross: CrossRulesIndex::new(model),
        }
    }
}

/// Record-operation id → the row a card joins it to: a collided id's FIRST
/// owning row (MCP output wave 2, W2-0a: the shared operation index's
/// explain selection). [`IndexData::new`]'s `op_row`.
pub(crate) fn op_rows(model: &Model) -> HashMap<String, usize> {
    crate::engine::l4::operation_index::OperationIndex::build(
        model,
        crate::engine::l4::operation_index::IndexScope::RecordOperations,
    )
    .first_record_operation_rows()
}

/// The model indexes every card reads, owned ([`ExplainIndexes`]).
struct IndexData {
    rows_of: HashMap<String, Vec<usize>>,
    callees: HashMap<usize, Vec<usize>>,
    callers: HashMap<usize, Vec<usize>>,
    declaring: DeclaringRows,
    /// record-operation id -> the row that holds it.
    op_row: HashMap<String, usize>,
    /// row -> table id -> the fields that row writes on that table.
    row_fields: HashMap<usize, HashMap<String, BTreeSet<String>>>,
    /// `ModifyAll` / `Validate` operation id -> the field it writes, as its
    /// table declares it (W2-4, design §4.1 rule 2d: a per-op fact).
    op_field: HashMap<String, String>,
    /// Codeunit object id -> the page actions whose `RunObject` runs it:
    /// (the page's object index, the action's index in its run actions, the
    /// raw `RunObject` value's index in the action's).
    run_by: HashMap<String, Vec<(usize, usize, usize)>>,
}

/// The fields a table write states (W2-4, [`Indexes::write_fields`]).
#[derive(Debug, Default)]
struct WriteFields {
    /// Owner row of an `Insert` / `Modify` -> the fields it assigns.
    assigned: BTreeMap<usize, BTreeSet<String>>,
    /// (`ModifyAll` | `Validate`, its field, its owner row).
    per_op: BTreeSet<(String, String, usize)>,
    /// Owner rows of a `Rename`.
    renames: BTreeSet<usize>,
}

impl WriteFields {
    /// Every field name stated (for the captions).
    fn names(&self) -> BTreeSet<&str> {
        self.assigned
            .values()
            .flatten()
            .map(String::as_str)
            .chain(self.per_op.iter().map(|(_, f, _)| f.as_str()))
            .collect()
    }

    /// The human clauses (`fields X assigns on this table (not
    /// operation-specific): …`; `ModifyAll sets: F (in Y)`; `renames (primary
    /// key) in Z`), or `None` when the write states no field.
    fn human(&self, ix: &Indexes<'_>, detail: bool) -> Option<String> {
        let mut clauses: Vec<String> = Vec::new();
        for (&row, f) in &self.assigned {
            let names = fold(f.iter().cloned().collect(), caps::FIELDS, "fields", detail);
            // T-W2-3: a folded list states its full count.
            let count = match names.hidden {
                0 => String::new(),
                _ => format!("; {} fields", names.total()),
            };
            clauses.push(format!(
                "fields {} assigns on this table (not operation-specific{count}): {}",
                ix.row(row).name,
                names.joined()
            ));
        }
        for (kind, field, row) in &self.per_op {
            let verb = if kind == "Validate" {
                "validates"
            } else {
                "ModifyAll sets"
            };
            clauses.push(format!("{verb}: {field} (in {})", ix.qualified(*row)));
        }
        for &row in &self.renames {
            clauses.push(format!("renames (primary key) in {}", ix.qualified(row)));
        }
        (!clauses.is_empty()).then(|| clauses.join("; "))
    }

    /// The JSON: `fields` by owner (qualified), `fieldOps` per operation,
    /// `renames` (owners).
    fn json(&self, ix: &Indexes<'_>) -> (Value, Value, Value) {
        let fields: Map<String, Value> = self
            .assigned
            .iter()
            .map(|(&row, f)| (ix.qualified(row), json!(f)))
            .collect();
        let ops: Vec<Value> = self
            .per_op
            .iter()
            .map(|(k, f, row)| json!({"op": k, "field": f, "in": ix.qualified(*row)}))
            .collect();
        let renames: Vec<String> = self.renames.iter().map(|&r| ix.qualified(r)).collect();
        (Value::Object(fields), json!(ops), json!(renames))
    }
}

/// [`IndexData`] read against the model it was built from: what the cards
/// call.
struct Indexes<'m> {
    model: &'m Model,
    data: &'m IndexData,
}

/// The objects a raw `RunObject` value names (`Codeunit "Post"`,
/// `Codeunit 50100`, `Codeunit::"Post"`): matched by type and by number or
/// name, those of the page's own app first.
fn run_object_targets<'m>(model: &'m Model, page_app: &str, raw: &str) -> Vec<&'m str> {
    let t = raw.trim();
    let (ty, rest) = match t.split_once("::") {
        Some((ty, rest)) => (ty.trim(), rest.trim()),
        None => match t.split_once(char::is_whitespace) {
            Some((ty, rest)) => (ty.trim(), rest.trim()),
            None => return Vec::new(),
        },
    };
    let number: Option<i64> = rest.parse().ok();
    let name = rest
        .trim_matches('"')
        .replace("\"\"", "\"")
        .fold_identifier();
    let matches: Vec<&ModelObject> = model
        .workspace
        .objects
        .iter()
        .filter(|o| o.object_type.eq_ignore_ascii_case(ty))
        .filter(|o| match number {
            Some(n) => o.object_number == n,
            None => o.name.fold_identifier() == name,
        })
        .collect();
    let own: Vec<&str> = matches
        .iter()
        .filter(|o| o.app_guid.eq_ignore_ascii_case(page_app))
        .map(|o| o.id.as_str())
        .collect();
    if own.is_empty() {
        matches.iter().map(|o| o.id.as_str()).collect()
    } else {
        own
    }
}

/// An error collector (spec §4.2 limits): a `Log…` procedure of BC's
/// `Error Message Management` codeunit or `Error Message` table. They record an
/// error and go on, so the check they make is not a rule. The engine knows no
/// other collector; this list is stated, not derived.
fn is_error_collector(ix: &Indexes<'_>, row: usize) -> bool {
    let (ty, obj) = ix.object_of(row);
    let name = ix.row(row).name.to_ascii_lowercase();
    let obj = obj.fold_identifier();
    name.starts_with("log")
        && ((ty == "Codeunit" && obj == "error message management")
            || (ty == "Table" && obj == "error message"))
}

/// A routine's name in a `via` path: a member trigger with its member
/// (`OnValidate (Amount)`).
fn via_name(ix: &Indexes<'_>, row: usize) -> String {
    let r = ix.row(row);
    match &r.enclosing_member {
        Some(m) => format!("{} ({m})", r.name),
        None => r.name.clone(),
    }
}

/// One inherited (or subscriber) rule: the rule's own JSON with the routine
/// that holds the check (`in`), the call path to it (`via`), and for a table
/// trigger's rule the write that runs it.
fn inh_json(ix: &Indexes<'_>, i: &Inh) -> Value {
    let mut v = rule_json(&i.rule);
    let m = v.as_object_mut().expect("a rule is an object");
    m.insert("in".into(), ix.qualified(i.owner).into());
    m.insert(
        "via".into(),
        json!(i.via.iter().map(|&r| via_name(ix, r)).collect::<Vec<_>>()),
    );
    m.insert(
        "trigger".into(),
        i.trigger.as_ref().map_or(
            Value::Null,
            |t| json!({ "op": t.op, "anchor": anchor_json(&t.anchor) }),
        ),
    );
    v
}

fn via_human(ix: &Indexes<'_>, i: &Inh) -> String {
    i.via
        .iter()
        .map(|&r| via_name(ix, r))
        .collect::<Vec<_>>()
        .join(" > ")
}

impl IndexData {
    fn new(model: &Model) -> IndexData {
        let ws = &model.workspace;
        let mut rows_of: HashMap<String, Vec<usize>> = HashMap::new();
        let mut ix_of: HashMap<&str, usize> = HashMap::new();
        for (i, r) in ws.routines.iter().enumerate() {
            rows_of
                .entry(r.stable_routine_id.clone())
                .or_default()
                .push(i);
            ix_of.entry(r.id.as_str()).or_insert(i);
        }
        let op_row = op_rows(model);
        let mut callees: HashMap<usize, Vec<usize>> = HashMap::new();
        let mut callers: HashMap<usize, Vec<usize>> = HashMap::new();
        for e in &model.calls.edges {
            let Some(to) = e.to.as_deref().and_then(|t| ix_of.get(t)) else {
                continue;
            };
            let from = e.from_ix.get();
            callees.entry(from).or_default().push(*to);
            callers.entry(*to).or_default().push(from);
        }
        for v in callees.values_mut().chain(callers.values_mut()) {
            v.sort_unstable();
            v.dedup();
        }
        let mut row_fields: HashMap<usize, HashMap<String, BTreeSet<String>>> = HashMap::new();
        let mut op_field: HashMap<String, String> = HashMap::new();
        for w in field_writes(model) {
            if let Some(op) = w.op_id {
                op_field
                    .entry(op.to_string())
                    .or_insert_with(|| w.field.to_string());
            }
            row_fields
                .entry(w.routine_ix)
                .or_default()
                .entry(w.table_id.to_string())
                .or_default()
                .insert(w.field.to_string());
        }
        let mut run_by: HashMap<String, Vec<(usize, usize, usize)>> = HashMap::new();
        for (oi, o) in ws.objects.iter().enumerate() {
            for (ai, a) in o.run_actions().iter().enumerate() {
                for (ri, raw) in a.run_object.iter().enumerate() {
                    for target in run_object_targets(model, &o.app_guid, raw) {
                        run_by
                            .entry(target.to_string())
                            .or_default()
                            .push((oi, ai, ri));
                    }
                }
            }
        }
        IndexData {
            rows_of,
            callees,
            callers,
            declaring: DeclaringRows::new(&ws.objects),
            op_row,
            row_fields,
            op_field,
            run_by,
        }
    }
}

impl<'m> Indexes<'m> {
    fn over(model: &'m Model, data: &'m IndexData) -> Indexes<'m> {
        Indexes { model, data }
    }

    fn row(&self, i: usize) -> &'m ModelRoutine {
        &self.model.workspace.routines[i]
    }

    /// The rows of a stable routine id, in model order.
    fn rows_of(&self, stable_id: &str) -> &'m [usize] {
        self.data.rows_of.get(stable_id).map_or(&[], Vec::as_slice)
    }

    /// The row that holds a record operation.
    fn op_row(&self, op: &str) -> Option<usize> {
        self.data.op_row.get(op).copied()
    }

    /// The fields `row` writes on `table_id`.
    fn row_fields(&self, row: usize, table_id: &str) -> Option<&'m BTreeSet<String>> {
        self.data.row_fields.get(&row)?.get(table_id)
    }

    /// The fields one table write states (design §4.1 rule 2d, by the
    /// ORIGINAL operation kind): `Insert` / `Modify` the fields their owning
    /// routine assigns on the table (a routine-level fact, so never
    /// operation-specific), one clause per owner; `ModifyAll` / `Validate` the
    /// operation's own field argument; `Rename` the primary key; `Delete`,
    /// `DeleteAll`, `LockTable` and a write with no op row nothing.
    fn write_fields(&self, ops: &[String], table_id: &str) -> WriteFields {
        let mut f = WriteFields::default();
        for op in ops {
            let Some(row) = self.op_row(op.as_str()) else {
                continue;
            };
            let Some(kind) = self.row(row).record_operations.iter().find(|o| o.id == *op) else {
                continue;
            };
            match kind.op.as_str() {
                "Insert" | "Modify" => {
                    if let Some(assigned) = self.row_fields(row, table_id) {
                        f.assigned
                            .entry(row)
                            .or_default()
                            .extend(assigned.iter().cloned());
                    }
                }
                k @ ("ModifyAll" | "Validate") => {
                    if let Some(field) = self.data.op_field.get(op.as_str()) {
                        f.per_op.insert((k.to_string(), field.clone(), row));
                    }
                }
                "Rename" => {
                    f.renames.insert(row);
                }
                _ => {}
            }
        }
        f
    }

    /// The page actions whose `RunObject` runs `object_id`: (the page's
    /// object index, the action, the raw `RunObject` value).
    fn run_by(&self, object_id: &str) -> Vec<(usize, &'m CardAction, &'m str)> {
        let objects = &self.model.workspace.objects;
        self.data
            .run_by
            .get(object_id)
            .map_or(&[][..], Vec::as_slice)
            .iter()
            .map(|&(oi, ai, ri)| {
                let a = &objects[oi].run_actions()[ai];
                (oi, a, a.run_object[ri].as_str())
            })
            .collect()
    }

    /// `(object type, object name)` of a row.
    fn object_of(&self, i: usize) -> (&'m str, &'m str) {
        let r = self.row(i);
        let name = self
            .data
            .declaring
            .of(&self.model.workspace.objects, r)
            .map_or("", |o| o.name.as_str());
        (r.object_type.as_str(), name)
    }

    /// `<Type> <Object>::<Routine>`.
    fn qualified(&self, i: usize) -> String {
        let (t, n) = self.object_of(i);
        owner_qualified(&object_display(t, n), &self.row(i).name)
    }

    /// Breadth-first from `start`, over `adj`: each reached row with the row
    /// it was first reached from (`None` for a start row).
    fn reach(
        &self,
        start: &[usize],
        adj: &HashMap<usize, Vec<usize>>,
    ) -> BTreeMap<usize, Option<usize>> {
        let mut seen: BTreeMap<usize, Option<usize>> = BTreeMap::new();
        let mut queue: VecDeque<usize> = VecDeque::new();
        for &s in start {
            if seen.insert(s, None).is_none() {
                queue.push_back(s);
            }
        }
        while let Some(n) = queue.pop_front() {
            for &m in adj.get(&n).map(Vec::as_slice).unwrap_or(&[]) {
                if let std::collections::btree_map::Entry::Vacant(v) = seen.entry(m) {
                    v.insert(Some(n));
                    queue.push_back(m);
                }
            }
        }
        seen
    }
}

/// The routine names on the path from `from` back to a start row of `reach`.
fn path_names(
    ix: &Indexes<'_>,
    reach: &BTreeMap<usize, Option<usize>>,
    from: usize,
) -> Vec<String> {
    let mut out = vec![ix.row(from).name.clone()];
    let mut at = from;
    while let Some(Some(next)) = reach.get(&at) {
        out.push(ix.row(*next).name.clone());
        at = *next;
    }
    out
}

/// Everything the document needs, rendered: over `prepared`, the indexes a
/// resident revision keeps for `model`, else over indexes built here.
#[allow(clippy::too_many_arguments)] // the card's inputs, one each
fn render(
    model: &Model,
    mode: &ExplainMode,
    resolved: &Resolved,
    effects: &DigestEffects,
    full: &[DigestEntryFull],
    apps: &[Value],
    prepared: Option<&ExplainIndexes>,
    opts: RenderOpts,
    sources: Option<&crate::engine::source_excerpt::SourceTexts>,
    table_answers: &TableAnswers,
) -> ExplainRunResult {
    use crate::engine::perf_trace::span;
    let built;
    let indexes = match prepared {
        Some(p) => p,
        None => {
            let _s = span("explain", "explain.indexes");
            built = ExplainIndexes::build(model);
            &built
        }
    };
    let ix = Indexes::over(model, &indexes.ix);
    // The walk is this run's own: its memo and expansion budget start empty
    // whatever earlier runs over the same indexes did.
    let cross = CrossRules::over(model, &indexes.cross);
    let tests_reaching = effects.coverage_graph.as_ref().map(|g| {
        let _s = span("explain", "explain.reaching_tests");
        crate::engine::l5::test_coverage::reaching_tests(
            g,
            &resolved.roots.iter().cloned().collect(),
        )
    });
    let cards_span = span("explain", "explain.cards");
    // W2-2: one rollup context for every card (names, the operation index).
    // Paths name overloads by short handles; each card ends with their
    // signatures (MCP output step 2).
    let rcx = RollupCx::new(model, apps).with_short_handles();
    // (anchor sort key, routine id, JSON, human text) per card.
    type Rendered = (Option<(String, u32, u32)>, String, Value, String);
    let mut cards: Vec<Rendered> = resolved
        .roots
        .iter()
        .map(|rid| {
            let entry = full.iter().find(|e| &e.routine_id == rid);
            let card = Card::build(&ix, &cross, rid, entry, &rcx, tests_reaching.as_ref(), opts);
            let (mut json, mut human) = (card.json(), card.human());
            // `--writes-table` (W2-2 gap (a)): prove's own answer per table,
            // before `next:`.
            if let Some((values, lines)) = table_answers.get(rid) {
                json["tableAnswers"] = json!(values);
                let at = human.find("\n  next:").map_or(human.len(), |i| i + 1);
                let text: String = lines.iter().map(|l| format!("  {l}\n")).collect();
                human.insert_str(at, &text);
            }
            // The overloads this card's paths named, with their signatures.
            let legend: String = rcx
                .take_legend(opts.detail)
                .iter()
                .map(|l| format!("  {l}\n"))
                .collect();
            if !legend.is_empty() {
                let at = human.find("\n  next:").map_or(human.len(), |i| i + 1);
                human.insert_str(at, &legend);
            }
            (card.sort_key.clone(), rid.clone(), json, human)
        })
        .collect();
    drop(cards_span);
    // Sorted by anchor (a card with no anchor last), then id.
    cards.sort_by(|a, b| (a.0.is_none(), &a.0, &a.1).cmp(&(b.0.is_none(), &b.0, &b.1)));

    let mode_name = match mode {
        ExplainMode::Routines(_) => "routine",
        ExplainMode::Diff { .. } => "diff",
        ExplainMode::Field { .. } => "field",
    };
    let mut doc = Map::new();
    doc.insert("schema".into(), SCHEMA.into());
    doc.insert(
        "lineBase".into(),
        crate::engine::line_base::LINE_BASE.into(),
    );
    doc.insert("mode".into(), mode_name.into());
    match mode {
        // One selector: the single-routine document, unchanged. Two or more:
        // the list of distinct selectors, in input order, in its place.
        ExplainMode::Routines(selectors) => match &distinct(selectors)[..] {
            [one] => {
                doc.insert("selector".into(), (*one).clone().into());
            }
            many => {
                doc.insert(
                    "selectors".into(),
                    Value::Array(many.iter().map(|s| (*s).clone().into()).collect()),
                );
            }
        },
        ExplainMode::Diff { .. } => {}
        ExplainMode::Field { table, field } => {
            doc.insert("selector".into(), json!({ "table": table, "field": field }));
        }
    }
    // `--table/--field`: each writer's effective rules, from the index d65
    // reads (spec §4.4, §4.5 step 2).
    let rules_index = resolved.field.as_ref().map(|f| {
        RulesIndex::build_where_over(model, &indexes.cross, |t, n| {
            t == f.table_id && n == f.field
        })
    });
    let entry = resolved
        .field
        .as_ref()
        .zip(rules_index.as_ref())
        .and_then(|(f, idx)| idx.field(&f.table_id, &f.field));
    if let Some(f) = &resolved.field {
        doc.insert("field".into(), field_json(&ix, f, entry));
    }
    doc.insert(
        "cards".into(),
        Value::Array(cards.iter().map(|c| c.2.clone()).collect()),
    );
    doc.insert(
        "diagnostics".into(),
        Value::Array(
            resolved
                .diagnostics
                .iter()
                .map(|d| d.json.clone())
                .collect(),
        ),
    );
    let json_text = serialize_normalized_value(&sort_keys_keep_nulls(Value::Object(doc)));

    let mut human = String::new();
    if let Some(f) = &resolved.field {
        human.push_str(&field_human(&ix, f, entry));
    }
    for (_, _, _, h) in &cards {
        human.push_str(h);
    }
    if cards.is_empty() && resolved.field.is_none() && resolved.diagnostics.is_empty() {
        human.push_str("explain: no routine resolved\n");
    }
    for d in &resolved.diagnostics {
        human.push_str(&d.human);
        human.push('\n');
    }
    let summary = explain_summary(&cards, resolved.diagnostics.len());
    // W2-7 (2g): `--show-source` excerpts (read while the locations still
    // name their app by guid), then readable dependency locations (the JSON
    // keeps the guid).
    let human = match sources {
        Some(s) => crate::engine::source_excerpt::with_excerpts(&human, s),
        None => human,
    };
    let human = crate::program::resolve::full::readable_locations(apps, human);
    ExplainRunResult {
        json_text,
        human_text: human,
        exit_code: if resolved.unresolved { 2 } else { 0 },
        summary,
    }
}

/// One rendered card: (sort key, routine id, JSON, human).
type RenderedCard = (Option<(String, u32, u32)>, String, Value, String);

/// What the cards say, for a front end that delivers them as a file (MCP output
/// plan 1e): each card's header line and the length of its lists, read off the
/// card's own JSON. Capped (`capped_summary`): cards are dropped from the end.
fn explain_summary(cards: &[RenderedCard], diagnostics: usize) -> Value {
    let len = |v: &Value, ptr: &str| v.pointer(ptr).and_then(Value::as_array).map_or(0, Vec::len);
    let listed: Vec<Value> = cards
        .iter()
        .map(|(_, _, j, h)| {
            json!({
                "card": h.lines().next().unwrap_or(""),
                "ownRules": len(j, "/rules/own"),
                "writes": len(j, "/effects/writes"),
                "commits": j.pointer("/effects/commits").cloned().unwrap_or(Value::Null),
                "events": len(j, "/effects/events"),
                "ui": len(j, "/effects/ui"),
                "reachedFrom": len(j, "/reachedFrom"),
                "tests": len(j, "/tests"),
                "gaps": len(j, "/gaps"),
            })
        })
        .collect();
    crate::engine::batch::output::capped_summary(
        json!({
            "kind": "explain",
            "cardCount": cards.len(),
            "diagnostics": diagnostics,
            "cards": listed,
        }),
        "cards",
    )
}

/// The writers in anchor order (of each one's first write site, then id), as
/// the cards are.
fn writers_by_anchor(f: &FieldAnswer) -> Vec<(&String, &Vec<(WriteKind, PAnchor)>)> {
    let mut w: Vec<_> = f.writers.iter().collect();
    fn key(s: &[(WriteKind, PAnchor)]) -> Option<(&str, u32, u32)> {
        s.first().map(|(_, a)| anchor_key(a))
    }
    w.sort_by(|a, b| (key(a.1), a.0).cmp(&(key(b.1), b.0)));
    w
}

/// The index's writer `id`, and its site at `a`.
fn index_site<'e>(
    entry: Option<&'e FieldWriters>,
    id: &str,
    a: &PAnchor,
) -> (Option<&'e IndexWriter>, Option<&'e WriteSite>) {
    let w = entry.and_then(|e| e.writers.iter().find(|w| w.stable_id == id));
    let s = w.and_then(|w| {
        w.sites
            .iter()
            .find(|s| anchor_key(&s.anchor) == anchor_key(a))
    });
    (w, s)
}

/// The `field` block of `--table/--field`. Each site lists its effective
/// rules (`rules`: own, inherited, table-trigger) and each writer the rules
/// every known caller applies (`callerRules`), from the rules index.
fn field_json(ix: &Indexes<'_>, f: &FieldAnswer, entry: Option<&FieldWriters>) -> Value {
    let writers: Vec<Value> = writers_by_anchor(f)
        .into_iter()
        .map(|(id, sites)| {
            let display = ix
                .rows_of(id.as_str())
                .first()
                .map_or_else(|| id.clone(), |&i| ix.qualified(i));
            let writer = entry.and_then(|e| e.writers.iter().find(|w| &w.stable_id == id));
            let caller_rules: Vec<Value> = writer
                .map(|w| {
                    w.caller_rules()
                        .into_iter()
                        .map(|i| inh_json(ix, i))
                        .collect()
                })
                .unwrap_or_default();
            json!({
                "id": id,
                "routine": display,
                "sites": sites
                    .iter()
                    .map(|(k, a)| {
                        let rules: Vec<Value> = index_site(entry, id, a)
                            .1
                            .map(|s| s.rules.iter().map(|i| inh_json(ix, i)).collect())
                            .unwrap_or_default();
                        json!({ "kind": k.as_str(), "anchor": anchor_json(a), "rules": rules })
                    })
                    .collect::<Vec<_>>(),
                "callerRules": caller_rules,
                "knownCallers": writer.map_or(0, |w| w.callers.len()),
            })
        })
        .collect();
    let caption = one_text(&f.captions);
    json!({
        "table": f.table,
        "tableId": f.table_id,
        "field": f.field,
        "caption": caption.clone().ok().flatten(),
        "captionArms": caption.err().unwrap_or_default(),
        "writers": writers,
    })
}

fn field_human(ix: &Indexes<'_>, f: &FieldAnswer, entry: Option<&FieldWriters>) -> String {
    let mut out = format!("field: {}.{}", f.table, f.field);
    if let Ok(Some(c)) = one_text(&f.captions) {
        out.push_str(&format!(" (caption \"{c}\")"));
    }
    out.push_str(&format!(": {} writer(s)\n", f.writers.len()));
    for (id, sites) in writers_by_anchor(f) {
        let display = ix
            .rows_of(id.as_str())
            .first()
            .map_or_else(|| id.clone(), |&i| ix.qualified(i));
        let at: Vec<String> = sites
            .iter()
            .map(|(k, a)| format!("{} at {}", k.as_str(), anchor_human(a)))
            .collect();
        out.push_str(&format!("  {display}: {}\n", at.join(", ")));
        for (_, a) in sites {
            let (_, site) = index_site(entry, id, a);
            let rules = site.map_or(&[][..], |s| s.rules.as_slice());
            if rules.is_empty() {
                out.push_str(&format!("    at {}: no rule\n", anchor_human(a)));
            }
            for i in rules {
                out.push_str(&format!(
                    "    at {}: {}\n",
                    anchor_human(a),
                    rule_human(&i.rule)
                ));
            }
        }
        if let Some(w) = entry.and_then(|e| e.writers.iter().find(|w| &w.stable_id == id)) {
            for i in w.caller_rules() {
                out.push_str(&format!(
                    "    every caller applies: {}\n",
                    rule_human(&i.rule)
                ));
            }
        }
    }
    out.push('\n');
    out
}

/// One routine's evidence card.
struct Card {
    sort_key: Option<(String, u32, u32)>,
    routine: Value,
    title: String,
    anchor: Option<PAnchor>,
    doc: Option<String>,
    own: Vec<PRule>,
    inherited: Vec<(Value, String)>,
    callers: Vec<(Value, String)>,
    /// The known routine callers, qualified (page-action triggers apart).
    known_callers: Vec<String>,
    /// Every known caller row, page-action triggers included (the caller
    /// rules' `knownCallers`).
    known_caller_rows: usize,
    writes: Vec<Value>,
    commits: &'static str,
    commit_sites: Vec<Value>,
    events: Vec<Value>,
    /// Per kind family ([`FAMILIES`]): own rows and group rows.
    families: BTreeMap<&'static str, Vec<Value>>,
    crossings: Vec<Value>,
    reached_from: Vec<(Value, String)>,
    extension_points: Vec<(Value, String)>,
    tests: Vec<(Value, String)>,
    gaps: Vec<Gap>,
    /// W2-2: the rollup's totals (with callers, page actions and tests).
    totals: RollupTotals,
    coverage: Value,
    gaps_in_dependencies: Value,
    /// The human lines of the shared sections, rendered once.
    header: Vec<String>,
    sections: Sections,
    opts: RenderOpts,
}

/// `(name, count)`, most first.
type Ranked = Vec<(String, usize)>;

/// The rollup's human sections, in card order (design §4.1).
#[derive(Default)]
struct Sections {
    commits: Vec<String>,
    workspace: Vec<String>,
    caller_dependent: Vec<String>,
    beyond: Vec<String>,
    no_path: Vec<String>,
    dead: Vec<String>,
    temporary: Vec<String>,
    events: Vec<String>,
    kinds: Vec<String>,
    dependency_gaps: Vec<String>,
}

/// A gap whose call site is in dependency code (folded by default, §4.2).
fn is_dependency_gap(g: &Gap) -> bool {
    g.json["kind"].as_str() == Some("unresolved-call")
        && g.json["anchor"]["file"]
            .as_str()
            .is_some_and(|f| !f.starts_with("ws:"))
}

/// A gap that is analysis incompleteness (`limits:`) or a note (§3.7).
fn gap_class(kind: &str) -> Option<&'static str> {
    match kind {
        "body-unavailable"
        | "parse-incomplete"
        | "symbol-only-dependency"
        | "cone-truncated"
        | "cross-call-rules-truncated" => Some("limit"),
        "table-trigger-rules-skipped"
        | "error-collector"
        | "text-differs-by-directive"
        | "field-caption-unknown" => Some("note"),
        _ => None,
    }
}

/// The entry of a root the digest query built none for (declaration-only, or
/// not in the tree): no effect, no gap.
fn empty_entry(rid: &str) -> DigestEntryFull {
    DigestEntryFull {
        routine_id: rid.to_string(),
        routine_display: String::new(),
        object_display: String::new(),
        routine_anchor: None,
        routine_anchors: Vec::new(),
        coverage_status: "unknown".to_string(),
        coverage_reasons: Vec::new(),
        effects: Vec::new(),
        unresolved: Vec::new(),
        unresolved_traversal: crate::engine::l5::unresolved_cone::UnresolvedTraversal {
            truncated: false,
            max_depth: 0,
            visited_routines: 0,
        },
        entry_diagnostics: Vec::new(),
        declaration_only: false,
    }
}

/// The routine names of an effect's shortest witness path (its hops'
/// targets); empty for an effect of the routine's own body.
fn via_names(e: &DigestEffectFull) -> Vec<String> {
    // Final CDO review, item 2: a row with no listed path names its
    // searched witness's routines (by name, as a listed path's hops do).
    if e.via_paths.iter().all(Vec::is_empty)
        && let Some(Ok(w)) = &e.own_path
    {
        return w
            .steps
            .iter()
            .map(|s| {
                s.display
                    .rsplit_once("::")
                    .map_or(s.display.as_str(), |(_, n)| n)
                    .to_string()
            })
            .collect();
    }
    e.via_paths
        .iter()
        .min_by_key(|p| p.len())
        .map(|p| {
            p.iter()
                .filter_map(|h| {
                    h.inner
                        .to_display
                        .clone()
                        .or_else(|| h.inner.callee_display.clone())
                })
                .collect()
        })
        .unwrap_or_default()
}

/// An effect's anchor: its evidence (an event publication's is already the
/// call into the publisher: digest's G32 rule, applied by the digest query).
fn effect_anchor(e: &DigestEffectFull) -> Value {
    anchor_to_value(&e.evidence)
}

/// `file:line` of an anchor already in the document's JSON shape, whose line
/// [`anchor_to_value`] wrote 1-based: printed as it is.
fn anchor_value_human(v: &Value) -> String {
    match (v["file"].as_str(), v["line"].as_u64()) {
        (Some(f), Some(l)) => format!("{f}:{l}"),
        (Some(f), None) => f.to_string(),
        _ => "no position kept".to_string(),
    }
}

/// W2-1, raw versus refined (design §3.1): a row the root cannot reach
/// under literal arguments stays listed with `literalUnreachable: true`, so it
/// never reads like a live one.
fn mark_dead(v: &mut Value, e: &DigestEffectFull) {
    if e.literal_unreachable
        && let Some(o) = v.as_object_mut()
    {
        o.insert("literalUnreachable".into(), true.into());
    }
}

fn detail<'e>(e: &'e DigestEffectFull, key: &str) -> Option<&'e str> {
    e.detail
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
}

impl Card {
    fn build(
        ix: &Indexes<'_>,
        cross: &CrossRules<'_>,
        rid: &str,
        full: Option<&DigestEntryFull>,
        rcx: &RollupCx<'_>,
        tests_reaching: Option<&BTreeMap<String, Vec<(String, bool)>>>,
        opts: RenderOpts,
    ) -> Card {
        let model = ix.model;
        let rows: Vec<usize> = ix.rows_of(rid).to_vec();
        let mut gaps: Vec<Gap> = Vec::new();

        // The routine: the first row (two `#if` arms share one stable id).
        let first = rows.first().copied();
        let (object_type, object_name) = first.map_or(("", ""), |i| ix.object_of(i));
        let name = first.map_or(rid.to_string(), |i| ix.row(i).name.clone());
        let anchor = first.map(|i| ix.row(i).source_anchor.clone());
        let text = first.map(|i| ix.row(i).card_text());
        let caption = text.map_or(Ok(None), |t| one_text(&t.object_caption));
        if let Err(arms) = &caption {
            gaps.push(Gap::new(
                "text-differs-by-directive",
                anchor.as_ref(),
                Map::from_iter([
                    ("what".to_string(), Value::from("objectCaption")),
                    ("values".to_string(), json!(arms)),
                ]),
            ));
        }
        let doc = text.and_then(|t| t.doc.clone());
        let routine = json!({
            "id": rid,
            "name": name,
            "objectType": object_type,
            "object": object_name,
            "objectCaption": caption.ok().flatten(),
            "doc": doc,
            "anchor": anchor.as_ref().map(anchor_json),
        });
        let title = owner_qualified(&object_display(object_type, object_name), &name);

        // Own rules, every row's, by anchor.
        let mut own: Vec<PRule> = rows
            .iter()
            .flat_map(|&i| ix.row(i).rules().to_vec())
            .collect();
        own.sort_by(|a, b| {
            anchor_key(&a.anchor)
                .cmp(&anchor_key(&b.anchor))
                .then(a.id.cmp(&b.id))
        });
        own.dedup_by(|a, b| a.id == b.id);

        // Rules across calls (step 5).
        let inherited: Vec<(Value, String)> = cross
            .inherited(&rows)
            .iter()
            .map(|i| {
                let from = match &i.trigger {
                    Some(t) => format!(
                        "inherits from table trigger {} (run by {} at {})",
                        via_human(ix, i),
                        t.op,
                        anchor_human(&t.anchor)
                    ),
                    None => format!("inherits via {}", via_human(ix, i)),
                };
                (inh_json(ix, i), format!("{from}: {}", rule_human(&i.rule)))
            })
            .collect();
        let known_callers = cross.known_callers(&rows);
        let names = |v: &[usize]| -> Vec<String> { v.iter().map(|&c| ix.qualified(c)).collect() };
        let callers: Vec<(Value, String)> = cross
            .caller_rules(&rows)
            .iter()
            .map(|c: &CallerRule| {
                let mut v = inh_json(ix, &c.inh);
                let m = v.as_object_mut().expect("a rule is an object");
                m.insert("presentIn".into(), json!(names(&c.present_in)));
                m.insert("appliedBy".into(), json!(names(&c.applied_by)));
                m.insert("skippedBy".into(), json!(names(&c.skipped_by)));
                m.insert(
                    "note".into(),
                    "applies only when called from the callers in appliedBy".into(),
                );
                let skipped = if c.skipped_by.is_empty() {
                    String::new()
                } else {
                    format!("; skipped by {}", names(&c.skipped_by).join(", "))
                };
                let applied = if c.applied_by.is_empty() {
                    format!(
                        "applied by none, present in {} but may not stop",
                        names(&c.present_in).join(", ")
                    )
                } else {
                    format!("applied by {}", names(&c.applied_by).join(", "))
                };
                (
                    v,
                    format!(
                        "caller rule ({applied}{skipped}): {}",
                        rule_human(&c.inh.rule)
                    ),
                )
            })
            .collect();
        for s in cross.skipped_triggers(&rows) {
            let mut fields = Map::from_iter([
                ("op".to_string(), Value::from(s.op.clone())),
                ("table".to_string(), Value::from(s.table.clone())),
                ("reason".to_string(), Value::from(s.reason)),
                (
                    "triggers".to_string(),
                    json!(
                        s.triggers
                            .iter()
                            .map(|&t| ix.qualified(t))
                            .collect::<Vec<_>>()
                    ),
                ),
                (
                    "rules".to_string(),
                    Value::Array(s.rules.iter().map(|i| inh_json(ix, i)).collect()),
                ),
            ]);
            fields.insert(
                "what".to_string(),
                Value::from(match &s.field {
                    Some(f) => format!("{} of {}.{f}", s.op, s.table),
                    None => format!("{} on {}", s.op, s.table),
                }),
            );
            if let Some(f) = &s.field {
                fields.insert("field".to_string(), Value::from(f.clone()));
            }
            gaps.push(Gap::new(
                "table-trigger-rules-skipped",
                Some(&s.anchor),
                fields,
            ));
        }

        for &i in &rows {
            let r = ix.row(i);
            if !r.body_available {
                gaps.push(Gap::new(
                    "body-unavailable",
                    Some(&r.source_anchor),
                    Map::from_iter([(
                        "detail".to_string(),
                        Value::from("the routine has no body in the model: its rules and effects are not visible"),
                    )]),
                ));
            } else if r.parse_incomplete {
                gaps.push(Gap::new(
                    "parse-incomplete",
                    Some(&r.source_anchor),
                    Map::from_iter([(
                        "detail".to_string(),
                        Value::from("the routine's body did not parse completely: rules in the unparsed part are missing"),
                    )]),
                ));
            }
        }

        // Effects (W2-2, design §4.1, §4.2): the root's digest entry after the
        // L4 temp-state join and the literal pass, and its rollup.
        let empty;
        let full = match full {
            Some(f) => f,
            None => {
                empty = empty_entry(rid);
                &empty
            }
        };
        let rollup = RoutineRollup::build(full, rcx);
        let effs: &[DigestEffectFull] = &full.effects;
        let tables = table_by_id_preferring_real(&model.workspace.tables);
        let mut writes = Vec::new();
        let mut commit_sites = Vec::new();
        let mut events = Vec::new();
        let mut extension_points: Vec<(Value, String)> = Vec::new();
        let mut families: BTreeMap<&'static str, Vec<Value>> = BTreeMap::new();
        let mut uncaptioned: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for e in effs {
            let via = via_names(e);
            let anchor = effect_anchor(e);
            let ty = e.effect_type.as_str();
            // 2a: a group row names its crossing (listed once under
            // `effects.crossings`), never an empty table, and is never an
            // extension point.
            if is_group(e) {
                let crossing = crossing_key(e);
                let mut v = match ty {
                    "DB_INSERT" | "DB_MODIFY" | "DB_DELETE" => {
                        let (class, reason) = class_json(classify_detail(&e.detail));
                        let mut v = json!({
                            "op": ty,
                            "crossing": crossing,
                            "class": class,
                            "classReason": reason,
                            "resourceCount": e.group_resources.len(),
                            "resources": e.group_resources.iter()
                                .map(|(id, d)| json!({"id": id, "display": d}))
                                .collect::<Vec<_>>(),
                            "conditionality": e.conditionality,
                        });
                        for k in ["tempState", "resourcesOriginalCount", "unresolvedResources"] {
                            if let Some(x) = detail(e, k) {
                                v[k] = json!(x);
                            }
                        }
                        v
                    }
                    "EVENT_PUBLISH" if opts.detail => json!({
                        "crossing": crossing,
                        "resourceCount": e.group_resources.len(),
                        "resourcesListed": true,
                        "events": e.group_resources.iter().map(|(id, _)| id).collect::<Vec<_>>(),
                    }),
                    // O2: explain folds an event group's names to a count.
                    "EVENT_PUBLISH" => json!({
                        "crossing": crossing,
                        "resourceCount": e.group_resources.len(),
                        "resourcesListed": false,
                    }),
                    _ => {
                        let mut v = json!({"type": ty, "crossing": crossing, "conditionality": e.conditionality});
                        // W2-6 (2f): a resource-less group's own terminal
                        // witness, or why it has none.
                        match &e.group_terminal_path {
                            Some(Ok(w)) if opts.path_cap > 0 => {
                                v["paths"] =
                                    json!([crate::engine::l5::prove::group_witness_value(w)]);
                                v["pathOutcome"] = json!(if w.through_entry_point {
                                    "found"
                                } else {
                                    "outside-entry-point"
                                });
                            }
                            Some(Ok(_)) => {}
                            Some(Err(n)) => v["pathOutcome"] = json!(n.reason),
                            None => {}
                        }
                        v
                    }
                };
                mark_dead(&mut v, e);
                match ty {
                    "DB_INSERT" | "DB_MODIFY" | "DB_DELETE" => writes.push(v),
                    "COMMIT" => commit_sites.push(v),
                    "EVENT_PUBLISH" => events.push(v),
                    "UI_ERROR" => families.entry("errors").or_default().push(v),
                    other => {
                        if let Some(f) = family_of(other) {
                            families.entry(f).or_default().push(v);
                        }
                    }
                }
                continue;
            }
            if ty.starts_with("DB_") && ty != "DB_READ" {
                let table_id = detail(e, "resourceId").unwrap_or("");
                let table = detail(e, "resourceDisplay").unwrap_or(table_id);
                // The fields the write states, by its original operation
                // kinds (W2-4, design §4.1 rule 2d).
                let wf = ix.write_fields(&e.origin_operation_ids, table_id);
                let fields = wf.names();
                let (fields_by_owner, field_ops, renames) = wf.json(ix);
                let mut captions = Map::new();
                if let Some(t) = tables.get(table_id) {
                    for f in &fields {
                        let Some(mf) = t.fields.iter().find(|x| x.name == *f) else {
                            continue;
                        };
                        match one_text(&mf.captions) {
                            Ok(Some(c)) => {
                                captions.insert((*f).to_string(), c.into());
                            }
                            Ok(None) if model.role_of_app(&t.app_guid) == Role::Dependency => {
                                uncaptioned
                                    .entry(t.name.clone())
                                    .or_default()
                                    .insert((*f).to_string());
                            }
                            Ok(None) => {}
                            Err(arms) => gaps.push(Gap::new(
                                "text-differs-by-directive",
                                None,
                                Map::from_iter([
                                    (
                                        "what".to_string(),
                                        Value::from(format!("caption of {}.{f}", t.name)),
                                    ),
                                    ("values".to_string(), json!(arms)),
                                ]),
                            )),
                        }
                    }
                }
                let class = classify_detail(&e.detail);
                let (class_name, reason) = class_json(class);
                let op_kinds: BTreeSet<&str> = e
                    .origin_operation_ids
                    .iter()
                    .filter_map(|op| rcx.op_kind(op))
                    .collect();
                let mut w = json!({
                    "op": ty,
                    "opKinds": op_kinds,
                    "table": table,
                    "tableId": table_id,
                    "class": class_name,
                    "classReason": reason,
                    "fields": fields_by_owner,
                    "fieldOps": field_ops,
                    "renames": renames,
                    "fieldCaptions": Value::Object(captions),
                    "conditionality": e.conditionality,
                    "via": via,
                    "anchor": anchor,
                });
                if let Some(t) = detail(e, "tempState") {
                    w["tempState"] = json!(t);
                }
                // §3.5, section 13: a write with no witness names the owner
                // the operation index recovered.
                if e.evidence.source_kind == "unavailable" {
                    w["noPathKept"] = json!(true);
                    if let Some(o) = e
                        .origin_operation_ids
                        .iter()
                        .find_map(|op| rcx.op_owner(op))
                    {
                        w["owner"] = json!({
                            "display": o.display,
                            "stableId": o.stable_id,
                            "outcome": o.outcome,
                            "anchor": o.anchor,
                        });
                    }
                }
                mark_dead(&mut w, e);
                writes.push(w);
            } else if ty == "COMMIT" {
                let mut c = json!({
                    "conditionality": e.conditionality,
                    "via": via,
                    "anchor": anchor,
                });
                mark_dead(&mut c, e);
                commit_sites.push(c);
            } else if ty == "EVENT_PUBLISH" {
                let event_id = detail(e, "resourceId").unwrap_or("");
                let symbol = model.events.graph.events.iter().find(|s| s.id == event_id);
                let event = symbol.map_or_else(
                    || detail(e, "resourceDisplay").unwrap_or(event_id).to_string(),
                    |s| s.event_name.clone(),
                );
                let mut ev = json!({
                    "event": event,
                    "eventId": event_id,
                    "conditionality": e.conditionality,
                    "via": via,
                    "anchor": anchor,
                });
                mark_dead(&mut ev, e);
                events.push(ev);
                // An extension point: an event the routine raises itself (a
                // call straight into the publisher).
                if via.len() <= 1 {
                    let subscribers = model
                        .events
                        .graph
                        .edges
                        .iter()
                        .filter(|x| x.event_id == event_id)
                        .count();
                    // Spec §5: each subscriber's own rules (and its
                    // top-level callees'), listed here, never merged into
                    // `inherited`: a subscriber runs only when its app is
                    // installed (and, `conditional`, may be skipped).
                    let mut subscriber_routines: Vec<(Value, Vec<String>)> = model
                        .events
                        .graph
                        .edges
                        .iter()
                        .filter(|x| x.event_id == event_id)
                        .filter_map(|x| {
                            let row = cross.row_of(&x.subscriber_routine_id)?;
                            let rules = cross.rules_run_by(row);
                            let human: Vec<String> = rules
                                .iter()
                                .map(|i| {
                                    format!(
                                        "subscriber {} (only when its app is installed): {}",
                                        ix.qualified(row),
                                        rule_human(&i.rule)
                                    )
                                })
                                .collect();
                            Some((
                                json!({
                                    "routine": ix.qualified(row),
                                    "id": x.subscriber_stable_routine_id,
                                    "app": x.subscriber_app_id,
                                    "conditional": x.conditional,
                                    "rules": rules.iter().map(|i| inh_json(ix, i)).collect::<Vec<_>>(),
                                }),
                                human,
                            ))
                        })
                        .collect();
                    subscriber_routines.sort_by_key(|a| a.0.to_string());
                    subscriber_routines.dedup_by(|a, b| a.0 == b.0);
                    let subscriber_human: Vec<String> = subscriber_routines
                        .iter()
                        .flat_map(|s| s.1.iter().cloned())
                        .collect();
                    let is_handled = symbol.is_some_and(|s| {
                        s.parameters.iter().any(|p| {
                            p.is_var
                                && (p.name.eq_ignore_ascii_case("IsHandled")
                                    || p.name.eq_ignore_ascii_case("Handled"))
                        })
                    });
                    let mut human = format!(
                        "extension point {event}: {subscribers} subscriber(s){}",
                        if is_handled { ", IsHandled" } else { "" }
                    );
                    for h in subscriber_human {
                        human.push_str("\n    ");
                        human.push_str(&h);
                    }
                    extension_points.push((
                        json!({
                            "event": event,
                            "eventId": event_id,
                            "subscribers": subscribers,
                            "subscriberRoutines": subscriber_routines
                                .into_iter()
                                .map(|s| s.0)
                                .collect::<Vec<_>>(),
                            "conditionalOnAppSet": true,
                            "isHandled": is_handled,
                            "anchor": anchor,
                        }),
                        human,
                    ));
                }
            } else if let Some(f) = family_of(ty) {
                // 2e: every kind family. The routine's own `Error` is a check,
                // shown under `rules` (as `prove reaches-ui` treats it).
                let mut v = json!({
                    "type": ty,
                    "conditionality": e.conditionality,
                    "via": via,
                    "anchor": anchor,
                });
                mark_dead(&mut v, e);
                families.entry(f).or_default().push(v);
            }
        }
        for (table, fields) in uncaptioned {
            gaps.push(Gap::new(
                "field-caption-unknown",
                None,
                Map::from_iter([
                    ("table".to_string(), Value::from(table)),
                    ("fields".to_string(), json!(fields)),
                    (
                        "detail".to_string(),
                        Value::from(
                            "a dependency table's field captions are read from its source only",
                        ),
                    ),
                ]),
            ));
        }
        let commits = rollup.totals.commits;

        // Reached from: the page actions whose trigger reaches a row of the
        // routine over the resolved calls (or is one).
        let up = ix.reach(&rows, &ix.data.callers);
        let mut reached_from = Vec::new();
        for &r in up.keys() {
            let row = ix.row(r);
            // A page action whose `RunObject` runs this codeunit's `OnRun`.
            if row.kind == "trigger"
                && row.object_type == "Codeunit"
                && row.name.eq_ignore_ascii_case("OnRun")
            {
                for (oi, a, raw) in ix.run_by(row.object_id.as_str()) {
                    let page = &model.workspace.objects[oi];
                    let caption = one_text(&a.caption).ok().flatten();
                    reached_from.push((
                        json!({
                            "kind": "PageAction",
                            "objectType": page.object_type,
                            "object": page.name,
                            "action": a.name,
                            "actionKind": a.kind,
                            "caption": caption,
                            "tooltip": one_text(&a.tooltip).ok().flatten(),
                            "runObject": raw,
                            "via": path_names(ix, &up, r),
                            "anchor": anchor_json(&a.anchor),
                        }),
                        format!(
                            "reached from page action {} on {}{} (RunObject = {raw}) at {}",
                            a.name,
                            object_display(&page.object_type, &page.name),
                            caption
                                .map(|c| format!(" (caption \"{c}\")"))
                                .unwrap_or_default(),
                            anchor_human(&a.anchor)
                        ),
                    ));
                }
            }
            let Some(t) = row.card_text.as_deref() else {
                continue;
            };
            for a in &t.actions {
                let (ot, on) = ix.object_of(r);
                let caption = one_text(&a.caption);
                let tooltip = one_text(&a.tooltip);
                for (what, v) in [("caption", &caption), ("tooltip", &tooltip)] {
                    if let Err(arms) = v {
                        gaps.push(Gap::new(
                            "text-differs-by-directive",
                            Some(&a.anchor),
                            Map::from_iter([
                                ("what".to_string(), Value::from(format!("action {what}"))),
                                ("values".to_string(), json!(arms)),
                            ]),
                        ));
                    }
                }
                let via = path_names(ix, &up, r);
                let caption = caption.ok().flatten();
                reached_from.push((
                    json!({
                        "kind": "PageAction",
                        "objectType": ot,
                        "object": on,
                        "action": a.name,
                        "actionKind": a.kind,
                        "caption": caption,
                        "tooltip": tooltip.ok().flatten(),
                        "runObject": Value::Null,
                        "via": via,
                        "anchor": anchor_json(&a.anchor),
                    }),
                    format!(
                        "reached from page action {} on {}{} at {}",
                        a.name,
                        object_display(ot, on),
                        caption
                            .map(|c| format!(" (caption \"{c}\")"))
                            .unwrap_or_default(),
                        anchor_human(&a.anchor)
                    ),
                ));
            }
        }
        reached_from.sort_by(|a, b| {
            a.0["anchor"]
                .to_string()
                .cmp(&b.0["anchor"].to_string())
                .then(a.1.cmp(&b.1))
        });
        reached_from.dedup_by(|a, b| a.0 == b.0);

        // Tests.
        let mut tests = Vec::new();
        for (test, via_event) in tests_reaching
            .and_then(|t| t.get(rid))
            .into_iter()
            .flatten()
        {
            let row = ix.rows_of(test.as_str()).first().copied();
            let display = row.map_or_else(|| test.clone(), |i| ix.qualified(i));
            let asserts = row
                .and_then(|i| ix.row(i).card_text.as_deref())
                .and_then(|t| t.own_asserts);
            let anchor = row.map(|i| anchor_json(&ix.row(i).source_anchor));
            tests.push((
                json!({
                    "test": display,
                    "id": test,
                    "asserts": asserts,
                    "viaEvent": via_event,
                    "anchor": anchor,
                }),
                format!(
                    "test {display}{}{}",
                    asserts
                        .map(|n| format!(" ({n} own assert(s))"))
                        .unwrap_or_default(),
                    if *via_event { ", via an event" } else { "" }
                ),
            ));
        }
        tests.sort_by(|a, b| a.1.cmp(&b.1));

        // Gaps of the cone: the unresolved call sites digest's cone lists,
        // and the symbol-only routines the resolved calls reach.
        {
            for item in &full.unresolved {
                let kind = if matches!(item.status.as_str(), "external" | "unfetched-dependency") {
                    "symbol-only-dependency"
                } else {
                    "unresolved-call"
                };
                let mut fields = Map::from_iter([
                    ("status".to_string(), Value::from(item.status.clone())),
                    (
                        "callee".to_string(),
                        Value::from(item.callee_display.clone()),
                    ),
                    (
                        "in".to_string(),
                        Value::from(item.owning_routine_display.clone()),
                    ),
                ]);
                // The cone item's own anchor, else the call site's in the
                // model (the ledger row of a to-less call can carry none).
                let site = ix
                    .rows_of(item.owning_routine.as_str())
                    .iter()
                    .flat_map(|&i| &ix.row(i).call_sites)
                    .find(|cs| cs.id == item.callsite_id)
                    .map(|cs| &cs.source_anchor);
                let mut g = match (&item.callsite_file, site) {
                    (Some(f), _) => {
                        fields.insert(
                            "anchor".to_string(),
                            anchor_to_value(&FullAnchor {
                                source_kind: "source",
                                file: Some(f.clone()),
                                line: item.callsite_line,
                                column: item.callsite_column,
                                excerpt: None,
                            }),
                        );
                        let mut g = Gap::new(kind, None, fields);
                        g.key.1 = f.clone();
                        g.key.2 = item.callsite_line.unwrap_or(0);
                        g.key.3 = item.callsite_column.unwrap_or(0);
                        g
                    }
                    (None, site) => Gap::new(kind, site, fields),
                };
                g.key.4.clear();
                g.key.4.push_str(&g.json.to_string());
                gaps.push(g);
            }
            if full.unresolved_traversal.truncated {
                gaps.push(Gap::new(
                    "cone-truncated",
                    None,
                    Map::from_iter([(
                        "detail".to_string(),
                        Value::from(format!(
                            "the call cone was cut at depth {}: effects and gaps past it are not listed",
                            full.unresolved_traversal.max_depth
                        )),
                    )]),
                ));
            }
        }
        let down = ix.reach(&rows, &ix.data.callees);
        let mut symbol_only: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        let mut seen_incomplete: HashSet<usize> = HashSet::new();
        for &r in down.keys() {
            let row = ix.row(r);
            // Error collectors (spec §4.2 limits): the check they make
            // records an error and goes on, so it is no rule.
            for cs in &row.call_sites {
                if let Some(&c) = cross
                    .targets_at(r, &cs.id)
                    .iter()
                    .find(|&&t| is_error_collector(ix, t))
                {
                    gaps.push(Gap::new(
                        "error-collector",
                        Some(&cs.source_anchor),
                        Map::from_iter([
                            ("callee".to_string(), Value::from(ix.qualified(c))),
                            ("routine".to_string(), Value::from(ix.qualified(r))),
                            (
                                "detail".to_string(),
                                Value::from(
                                    "an error collector logs the error and goes on: the check \
                                     around it does not stop, and is not listed as a rule",
                                ),
                            ),
                        ]),
                    ));
                }
            }
            if rows.contains(&r) {
                continue;
            }
            if !row.body_available {
                symbol_only
                    .entry(row.app_guid.clone())
                    .or_default()
                    .insert(ix.qualified(r));
            } else if row.parse_incomplete && seen_incomplete.insert(r) {
                gaps.push(Gap::new(
                    "parse-incomplete",
                    Some(&row.source_anchor),
                    Map::from_iter([("routine".to_string(), Value::from(ix.qualified(r)))]),
                ));
            }
        }
        for (app, callees) in symbol_only {
            gaps.push(Gap::new(
                "symbol-only-dependency",
                None,
                Map::from_iter([
                    ("app".to_string(), Value::from(app)),
                    ("callees".to_string(), json!(callees)),
                    (
                        "detail".to_string(),
                        Value::from("callee rules not visible: the app has no source"),
                    ),
                ]),
            ));
        }
        if cross.truncated() {
            gaps.push(Gap::new(
                "cross-call-rules-truncated",
                None,
                Map::from_iter([(
                    "detail".to_string(),
                    Value::from(format!(
                        "the walk over rules across calls stopped (at {EXPANSION_BUDGET} routine \
                         expansions, or a call chain {MAX_DEPTH} deep): rules.inherited, \
                         rules.callers and the subscriber and skipped-trigger rules may be \
                         incomplete"
                    )),
                )]),
            ));
        }
        gaps.sort_by(|a, b| a.key.cmp(&b.key));

        // W2-2: the rollup's totals, with explain's callers and tests (F16k).
        let mut totals = rollup.totals.clone();
        // F16j: a page action's trigger is counted under page actions, not as
        // a routine caller.
        let routine_callers: Vec<String> = known_callers
            .iter()
            .filter(|&&c| {
                let r = ix.row(c);
                !(r.kind == "trigger" && r.object_type.starts_with("Page"))
            })
            .map(|&c| ix.qualified(c))
            .collect();
        totals.callers = Some(routine_callers.len());
        totals.page_actions = Some(reached_from.len());
        totals.tests = Some(tests.len());
        let mut limits: Vec<String> = Vec::new();
        let mut notes: BTreeMap<&str, usize> = BTreeMap::new();
        for g in &gaps {
            let kind = g.json["kind"].as_str().unwrap_or("");
            match gap_class(kind) {
                Some("limit") if kind != "cone-truncated" => limits.push(kind.to_string()),
                Some("note") => *notes.entry(kind).or_default() += 1,
                _ => {}
            }
        }
        limits.sort();
        let mut counted: Vec<String> = Vec::new();
        for k in limits {
            match counted.last_mut() {
                Some(last) if last.split(' ').next() == Some(k.as_str()) => {
                    let n: usize = last
                        .split(' ')
                        .nth(1)
                        .and_then(|n| n.parse().ok())
                        .unwrap_or(1);
                    *last = format!("{k} {}", n + 1);
                }
                _ => counted.push(format!("{k} 1")),
            }
        }
        let mut header = header_lines(&rollup, full, &counted);
        header[0] = totals.human();
        if !notes.is_empty() {
            header.insert(
                4,
                format!(
                    "notes: {}",
                    notes
                        .iter()
                        .map(|(k, n)| format!("{k} {n}"))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            );
        }
        let fields_of = |t: &TableWrites| -> Option<String> {
            let ops: Vec<String> = t
                .rows
                .iter()
                .flat_map(|&i| full.effects[i].origin_operation_ids.iter().cloned())
                .collect();
            ix.write_fields(&ops, &t.table_id).human(ix, opts.detail)
        };
        let sections = Sections {
            commits: commit_lines(&rollup, full, rcx, opts),
            workspace: workspace_write_lines(&rollup, full, rcx, opts, &fields_of),
            caller_dependent: caller_dependent_lines(&rollup, full),
            beyond: beyond_lines(&rollup, full, rcx, opts),
            no_path: no_path_lines(&rollup, full, rcx, opts),
            dead: literal_dead_lines(&rollup, opts),
            temporary: temporary_lines(&rollup, opts),
            events: event_lines(&rollup, rcx, opts),
            kinds: kind_lines(&rollup, full, opts.detail),
            dependency_gaps: dependency_gap_lines(&rollup, opts),
        };
        let crossings: Vec<Value> = rollup
            .crossings
            .iter()
            .map(|c| {
                json!({
                    "key": c.id,
                    "boundary": c.name,
                    "targets": c.target_ids.iter().map(|t| rcx.qualified(t)).collect::<Vec<_>>(),
                    "targetIds": c.target_ids,
                    "sites": c.sites,
                    "prefix": c.prefix_row.map(|i| via_names(&full.effects[i])),
                })
            })
            .collect();
        let g = &rollup.gaps;
        let gaps_in_dependencies = json!({
            "total": g.dependency,
            "polymorphic": g.dependency_polymorphic,
            "dynamicTarget": g.dependency_dynamic,
            "notPlaced": g.not_placed.len(),
            "topCallees": g.top_callees.iter().take(caps::TOP_CALLEES)
                .map(|(c, n)| json!({"callee": c, "count": n}))
                .collect::<Vec<_>>(),
        });
        let coverage = json!({
            "status": full.coverage_status,
            "reasons": full.coverage_reasons,
            "assumptions": ["loaded-program"],
            "limits": counted,
            "notes": notes,
        });

        Card {
            sort_key: anchor
                .as_ref()
                .map(|a| (a.source_unit_id.clone(), a.start_line, a.start_column)),
            routine,
            title,
            anchor,
            doc,
            own,
            inherited,
            callers,
            known_callers: routine_callers,
            known_caller_rows: known_callers.len(),
            writes,
            commits,
            commit_sites,
            events,
            families,
            crossings,
            reached_from,
            extension_points,
            tests,
            gaps,
            totals,
            coverage,
            gaps_in_dependencies,
            header,
            sections,
            opts,
        }
    }

    fn json(&self) -> Value {
        let detail = self.opts.detail;
        let mut effects = json!({
            "writes": self.writes,
            "commits": self.commits,
            "commitSites": self.commit_sites,
            "events": self.events,
            "crossings": self.crossings,
        });
        for (key, _) in FAMILIES {
            let rows = self.families.get(key).cloned().unwrap_or_default();
            // The explicit other-kinds bucket (W2-5) is present only when used.
            if *key == OTHER_KINDS && rows.is_empty() {
                continue;
            }
            effects[*key] = json!(rows);
        }
        // `via` and `tooltip` of a page action only with `--detail` (§4.2).
        let reached_from: Vec<Value> = self
            .reached_from
            .iter()
            .map(|r| {
                let mut v = r.0.clone();
                if !detail && let Some(o) = v.as_object_mut() {
                    o.remove("via");
                    o.remove("tooltip");
                }
                v
            })
            .collect();
        json!({
            "routine": self.routine,
            "summary": self.totals.to_json(),
            "coverage": self.coverage,
            "rules": {
                "own": self.own.iter().map(rule_json).collect::<Vec<_>>(),
                "inherited": self.inherited.iter().map(|r| r.0.clone()).collect::<Vec<_>>(),
                "callers": self.callers.iter().map(|r| r.0.clone()).collect::<Vec<_>>(),
                "knownCallers": self.known_caller_rows,
            },
            "effects": effects,
            "reachedFrom": reached_from,
            "reachedFromSummary": self.reached_from_summary(),
            "extensionPoints": self.extension_points.iter().map(|r| r.0.clone()).collect::<Vec<_>>(),
            "tests": self.tests.iter().map(|r| r.0.clone()).collect::<Vec<_>>(),
            "gaps": self
                .gaps
                .iter()
                .filter(|g| detail || !is_dependency_gap(g))
                .map(|g| g.json.clone())
                .collect::<Vec<_>>(),
            "gapsInDependencies": self.gaps_in_dependencies,
        })
    }

    /// The page actions by caption and by page object (§4.1 section 8).
    fn reached_from_summary(&self) -> Value {
        let (captions, objects) = self.action_counts();
        json!({
            "actions": self.reached_from.len(),
            "objects": objects.len(),
            "byCaption": captions.iter().map(|(c, n)| json!({"caption": c, "count": n})).collect::<Vec<_>>(),
            "byObject": objects.iter().map(|(o, n)| json!({"object": o, "count": n})).collect::<Vec<_>>(),
        })
    }

    /// (caption, count) and (page object, count), most first.
    fn action_counts(&self) -> (Ranked, Ranked) {
        let mut captions: BTreeMap<String, usize> = BTreeMap::new();
        let mut objects: BTreeMap<String, usize> = BTreeMap::new();
        for (v, _) in &self.reached_from {
            let caption = v["caption"]
                .as_str()
                .or(v["action"].as_str())
                .unwrap_or("")
                .to_string();
            *captions.entry(caption).or_default() += 1;
            let object = object_display(
                v["objectType"].as_str().unwrap_or(""),
                v["object"].as_str().unwrap_or(""),
            );
            *objects.entry(object).or_default() += 1;
        }
        let ranked = |m: BTreeMap<String, usize>| {
            let mut v: Vec<(String, usize)> = m.into_iter().collect();
            v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
            v
        };
        (ranked(captions), ranked(objects))
    }

    fn human(&self) -> String {
        let mut out = format!(
            "{}{}\n",
            self.title,
            self.anchor
                .as_ref()
                .map(|a| format!("  {}", anchor_human(a)))
                .unwrap_or_default()
        );
        let mut line = |s: &str| {
            out.push_str("  ");
            out.push_str(s);
            out.push('\n');
        };
        let detail = self.opts.detail;
        // 1-6: summary, coverage, assumptions, limits, notes, scope.
        for h in &self.header {
            line(h);
        }
        if let Some(d) = &self.doc {
            line(&format!("doc: {}", d.replace('\n', " / ")));
        }
        // 7: rules, own and inherited.
        if self.own.is_empty() && self.inherited.is_empty() {
            line("rules: none of its own; inherited: none");
        } else {
            if self.own.is_empty() {
                line("rules: none of its own");
            }
            for r in &self.own {
                line(&rule_human(r));
            }
            if self.inherited.is_empty() {
                line("inherited rules: none");
            }
            for (_, h) in &self.inherited {
                line(h);
            }
        }
        // 8: callers with their caller rules, then the page actions.
        let names = fold(self.known_callers.clone(), caps::TABLES, "callers", detail);
        let actions = self.reached_from.len();
        let reached = if actions == 0 {
            String::new()
        } else {
            format!(
                "; reached from {}",
                rf_plural(actions, "page action", "page actions")
            )
        };
        if self.known_callers.is_empty() {
            line(&format!("callers: none{reached}"));
        } else {
            line(&format!(
                "callers: {} — {}{reached}",
                self.known_callers.len(),
                names.joined()
            ));
        }
        for (_, h) in &self.callers {
            line(&format!("  {h}"));
        }
        if actions <= 3 || detail {
            for (_, h) in &self.reached_from {
                line(&format!("  {h}"));
            }
        } else {
            let (captions, objects) = self.action_counts();
            let by = |v: Vec<(String, usize)>, cap: usize, unit: &'static str| {
                fold(
                    v.into_iter().map(|(c, n)| format!("{c} {n}")).collect(),
                    cap,
                    unit,
                    false,
                )
                .joined()
            };
            line(&format!(
                "  by caption ({}): {}",
                rf_plural(captions.len(), "caption", "captions"),
                by(captions, caps::CAPTIONS, "captions")
            ));
            line(&format!(
                "  most actions ({}): {} (--detail lists every action)",
                rf_plural(objects.len(), "page object", "page objects"),
                by(objects, caps::PAGE_OBJECTS, "page objects")
            ));
        }
        // 9-16: the shared effect sections.
        let s = &self.sections;
        for section in [
            &s.commits,
            &s.workspace,
            &s.caller_dependent,
            &s.beyond,
            &s.no_path,
            &s.dead,
            &s.temporary,
            &s.events,
        ] {
            // A section's shared path start, once (MCP output step 2).
            for l in &crate::engine::l5::routine_rollup::factor_prefix(section.clone()) {
                line(l);
            }
        }
        // 17: extension points (never a group).
        if self.extension_points.is_empty() {
            line("extension points: none (the routine publishes no event itself)");
        }
        for (_, h) in &self.extension_points {
            line(h);
        }
        // 18: kind families.
        for l in &s.kinds {
            line(l);
        }
        // 19: tests.
        if self.tests.is_empty() {
            line("tests: none reach it");
        }
        for (_, h) in &self.tests {
            line(h);
        }
        // 20: gaps in workspace code (dependency gaps fold, 21).
        let shown: Vec<&Gap> = self
            .gaps
            .iter()
            .filter(|g| detail || !is_dependency_gap(g))
            .collect();
        if !shown.is_empty() {
            line(&format!(
                "gaps in workspace code ({}):",
                rf_plural(shown.len(), "gap", "gaps")
            ));
        }
        for g in shown {
            let kind = g.json["kind"].as_str().unwrap_or("");
            let what = ["callee", "routine", "what", "table", "app", "status"]
                .iter()
                .filter_map(|k| g.json[*k].as_str().map(|v| format!("{k} {v}")))
                .collect::<Vec<_>>()
                .join(", ");
            let at = g
                .json
                .get("anchor")
                .map(|a| format!(" at {}", anchor_value_human(a)))
                .unwrap_or_default();
            line(&format!(
                "  gap: {kind}{}{at}",
                if what.is_empty() {
                    String::new()
                } else {
                    format!(" ({what})")
                }
            ));
            // The skipped table-trigger rules, one line each.
            for r in g.json["rules"].as_array().into_iter().flatten() {
                let when = r["whenText"].as_str().unwrap_or("");
                line(&format!(
                    "    skips {}: {}  [{} {}]",
                    r["in"].as_str().unwrap_or(""),
                    if when.is_empty() {
                        "always".to_string()
                    } else {
                        format!("rejects when {when}")
                    },
                    r["kind"].as_str().unwrap_or(""),
                    anchor_value_human(&r["anchor"])
                ));
            }
        }
        // 21: gaps in dependency code, as counts.
        for l in &s.dependency_gaps {
            line(l);
        }
        // 22: next.
        line(
            "next: alsem prove <routine> writes-table:<table> for one table with its witness; \
             may-commit for the commits; alsem query effects for every operation one routine deep",
        );
        out.push('\n');
        out
    }
}

fn cmp_text(op: CmpOp) -> &'static str {
    match op {
        CmpOp::Eq => "=",
        CmpOp::Ne => "<>",
        CmpOp::Lt => "<",
        CmpOp::Le => "<=",
        CmpOp::Gt => ">",
        CmpOp::Ge => ">=",
    }
}

fn atom_json(a: &ConditionAtom) -> Value {
    match a {
        ConditionAtom::FieldCompare {
            record_var,
            table,
            field,
            op,
            value,
        } => json!({
            "kind": "FieldCompare", "recordVar": record_var, "table": table,
            "field": field, "op": cmp_text(*op), "value": value,
        }),
        ConditionAtom::FieldEmpty {
            record_var,
            table,
            field,
            negated,
        } => json!({
            "kind": "FieldEmpty", "recordVar": record_var, "table": table,
            "field": field, "negated": negated,
        }),
        ConditionAtom::ParamBool { index, value } => {
            json!({ "kind": "ParamBool", "index": index, "value": value })
        }
        ConditionAtom::CallResult {
            callee_text,
            negated,
        } => json!({ "kind": "CallResult", "callee": callee_text, "negated": negated }),
        ConditionAtom::RecordFound {
            record_var,
            op,
            negated,
        } => {
            json!({ "kind": "RecordFound", "recordVar": record_var, "op": op, "negated": negated })
        }
        ConditionAtom::NoneOf { scrutinee, values } => {
            json!({ "kind": "NoneOf", "scrutinee": scrutinee, "values": values })
        }
        ConditionAtom::Other { text, anchor } => {
            json!({ "kind": "Other", "text": text, "anchor": anchor_json(anchor) })
        }
    }
}

fn message_json(m: &RuleMessage) -> Value {
    let (source, var) = match &m.source {
        MessageSource::Literal => ("Literal", None),
        MessageSource::Label { var } => ("Label", Some(var)),
        MessageSource::TextConst { var } => ("TextConst", Some(var)),
        MessageSource::ErrorInfo => ("ErrorInfo", None),
        MessageSource::Platform => ("Platform", None),
        MessageSource::Dynamic => ("Dynamic", None),
    };
    json!({
        "source": source,
        "var": var,
        "text": m.text,
        "args": m.args,
        "labelComment": m.label_comment,
        "candidates": m.candidates,
    })
}

fn rule_json(r: &PRule) -> Value {
    let (stops, reason) = match &r.stops {
        Stops::Always => ("always", None),
        Stops::MayNotStop { reason } => ("may-not-stop", Some(reason)),
    };
    json!({
        "id": r.id,
        "kind": format!("{:?}", r.kind),
        "stops": stops,
        "stopsReason": reason,
        "when": r.when.iter().map(atom_json).collect::<Vec<_>>(),
        "whenText": r.when_text,
        "message": r.message.as_ref().map(message_json),
        "comment": r.comment,
        "anchor": anchor_json(&r.anchor),
    })
}

/// One rule, one line: `rejects when C: "M"  [Kind file:line]`.
fn rule_human(r: &PRule) -> String {
    let when = if r.when_text.is_empty() {
        "always".to_string()
    } else {
        format!("when {}", r.when_text)
    };
    let msg = match &r.message {
        Some(m) => match (&m.text, &m.source) {
            (Some(t), _) => format!("\"{t}\""),
            (None, MessageSource::Platform) => "(platform message)".to_string(),
            (None, _) if !m.candidates.is_empty() => format!(
                "(one of: {})",
                m.candidates
                    .iter()
                    .map(|c| format!("\"{c}\""))
                    .collect::<Vec<_>>()
                    .join(" | ")
            ),
            (None, _) => "(message not known)".to_string(),
        },
        None => "(no message)".to_string(),
    };
    let verb = match r.kind {
        RuleKind::ConfirmExit => "exits silently",
        _ => "rejects",
    };
    let stops = match &r.stops {
        Stops::Always => String::new(),
        Stops::MayNotStop { reason } => format!(" (may not stop: {reason})"),
    };
    format!(
        "{verb} {when}: {msg}{stops}  [{:?} {}]",
        r.kind,
        anchor_human(&r.anchor)
    )
}

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
//! `reachedFrom` also follows a page action's `RunObject` into the triggers a
//! run of that object reaches (a codeunit's `OnRun`, a page's `OnOpenPage`
//! and its other lifecycle triggers, those of its extensions; the resolver's
//! rule, `impact_graph::reverse::run_reaches_trigger`).
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
    FAMILIES, HeaderFacts, OTHER_KINDS, RenderOpts, RollupCx, RollupTotals, RoutineRollup,
    TableWrites, beyond_lines, caller_dependent_lines, caps, class_json, commit_lines,
    crossing_key, dependency_gap_lines, event_lines, family_of, fold, format_routine, header_facts,
    is_group, kind_lines, literal_dead_lines, no_path_lines, plural as rf_plural, temporary_lines,
    workspace_write_lines,
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
/// `/4` (agent-tasks §6.6, additive): with `--callers-context` a card carries
/// `callersContext`, one object per callers' context question (`answer`,
/// `evidenceCount`, `entries`, `blockedByCounts`).
pub const SCHEMA: &str = "al-sem.explain/4";

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
    /// `--callers-context` (agent-tasks §6.6): each card's "callers'
    /// context" section. Only when asked: it needs the sited graph, which a
    /// resident engine builds only once a question reads it (M0-1), and the
    /// detector run's summaries.
    callers_context: bool,
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
            callers_context: false,
        }
    }

    /// `--callers-context` (agent-tasks §6.6).
    #[must_use]
    pub fn with_callers_context(mut self, on: bool) -> ExplainJob<'a> {
        self.callers_context = on;
        self
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
        self.opts = RenderOpts::new(detail, max_paths).with_surface(self.opts.surface);
        self
    }

    /// `--surface` (output validation §6.1): the words of every follow-up.
    #[must_use]
    pub fn with_surface(mut self, surface: crate::engine::follow_up::Surface) -> ExplainJob<'a> {
        self.opts = self.opts.with_surface(surface);
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
    number: i64,
    captions: Vec<String>,
    /// Per writer (stable id, in id order): its write sites, in anchor order.
    writers: BTreeMap<String, Vec<(WriteKind, PAnchor)>>,
    /// `(searched, not searched)`, from `field_touches::search_words`.
    search: (String, String),
    /// References that may write, not confirmed: each `kind at file:line`.
    candidates: Vec<String>,
    /// Round 2 G23: per-kind counts of [`Self::candidates`]: `RecordRef.Field
    /// by number`, `TransferFields copies`.
    candidate_kinds: (usize, usize),
    /// The workspace source unit (`ws:src/…`) of the object declaring the
    /// field, when a routine of that object names it.
    declaring_unit: Option<String>,
    /// `ws:src/…:<line>` of the field's declaration (filled in `compute`).
    declared_at: Option<String>,
    /// The field's own `OnValidate` trigger, by selector, when it has one.
    on_validate: Option<String>,
}

/// Round 2 G23: the 1-based line declaring field number `number` in the
/// workspace unit `unit` (`ws:src/…`): the first line that opens
/// `field(<number>;`, read off the file. `None` for a dependency unit, an
/// unreadable file or no match.
fn field_declaration_line(workspace: &std::path::Path, unit: &str, number: i64) -> Option<String> {
    let rel = unit.strip_prefix("ws:")?;
    let text = std::fs::read_to_string(workspace.join(rel)).ok()?;
    let want = number.to_string();
    let line = text.lines().position(|l| {
        let t = l.trim_start();
        let Some(rest) = t
            .get(..5)
            .filter(|h| h.eq_ignore_ascii_case("field"))
            .map(|_| t[5..].trim_start())
        else {
            return false;
        };
        let Some(args) = rest.strip_prefix('(') else {
            return false;
        };
        args.split(';').next().is_some_and(|n| n.trim() == want)
    })?;
    Some(format!("{unit}:{}", line + 1))
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
        let needs = if self.callers_context {
            ViewNeeds {
                graph: true,
                sited: true,
                ..ViewNeeds::EXPLAIN
            }
        } else {
            ViewNeeds::EXPLAIN
        };
        ViewDemand::one(self.req.inputs, self.req.single_app, false, needs)
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
        // `--writes-table` reads the cones' gap counts (declared needs:
        // only then are they counted).
        let mut plan = if self.writes_tables.is_empty() {
            explain_plan(resolved.roots.iter().cloned().collect())
        } else {
            crate::engine::session::counting_cone_gaps(explain_plan(
                resolved.roots.iter().cloned().collect(),
            ))
        };
        // `--callers-context`: the callers' walk reads the detector run's
        // summaries and cone writes, as prove's callers' questions do.
        if self.callers_context && !resolved.roots.is_empty() {
            plan = crate::engine::session::with_default_run(plan);
        }
        Ok(PlanPart {
            plan,
            reads,
            carry: Some(Box::new(resolved)),
        })
    }

    fn compute(&self, _: SideIx, cx: &mut ViewCx<'_>) -> Result<SideDoc, JobFailure> {
        let workspace = cx.inputs().workspace.clone();
        let mut resolved: Resolved = cx.take_carry();
        // Round 2 G23: the field's declaration line, read off its workspace
        // file (the model keeps no field anchor).
        if let Some(f) = resolved.field.as_mut() {
            f.declared_at = f
                .declaring_unit
                .as_deref()
                .and_then(|u| field_declaration_line(&workspace, u, f.number));
        }
        // `--callers-context`: each card's three callers' answers, off the
        // sited graph and the detector run (agent-tasks §6.6).
        // The graph is declared only with `--callers-context` (read it only
        // then: an undeclared read panics).
        let sited = self.callers_context.then(|| cx.sited()).flatten();
        let callers: HashMap<String, crate::engine::l5::callers_context::CallersSection> =
            match sited {
                Some(g) if !resolved.roots.is_empty() => {
                    let det = cx.view().det();
                    let ccx = crate::engine::l5::callers_context::CallersCx {
                        model: cx.view().model(),
                        g,
                        summaries: &det.summaries,
                        cone_derived: &det.cone_derived,
                        inputs: Some(cx.inputs()),
                        assume_no_dependents: false,
                        cancel: None,
                    };
                    // §6.1: the section's follow-ups name each card's selector.
                    let apps = cx.view().target().dependencies().apps;
                    let names = crate::engine::l5::routine_rollup::RoutineNames::new(
                        cx.view().model(),
                        &apps,
                    );
                    let fcx =
                        crate::engine::follow_up::Cx::new(self.opts.surface, self.req.single_app);
                    resolved
                        .roots
                        .iter()
                        .filter_map(|r| {
                            let selector = names.selector(r);
                            let hcx = crate::engine::l5::callers_context::HumanCx {
                                fcx,
                                selector: &selector,
                            };
                            crate::engine::l5::callers_context::section(&ccx, r, &hcx)
                                .map(|s| (r.clone(), s))
                        })
                        .collect()
                }
                _ => HashMap::new(),
            };
        // `--callers-context` also counts each card's callers through
        // dependency code (callers-through-dependency design r2 §11.3): the
        // sited graph is declared here, so it costs one walk per card.
        // Without the flag nothing is built and the card is unchanged (M0's
        // lazy rule; an answer never depends on whether a resident engine
        // happens to hold the graph).
        let through: HashMap<String, (Value, String)> =
            if self.callers_context && !resolved.roots.is_empty() {
                let view = cx.view();
                let no_dependency_code =
                    self.req.single_app || view.target().dependencies().apps.is_empty();
                resolved
                    .roots
                    .iter()
                    .map(|r| {
                        (
                            r.clone(),
                            through_dependency_card(
                                view.model(),
                                sited,
                                r,
                                no_dependency_code,
                                self.opts.surface,
                            ),
                        )
                    })
                    .collect()
            } else {
                HashMap::new()
            };
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
        // The test scope (agent-tasks C-7): the same discovery `alsem tests`
        // runs, files only. explain never starts a test universe: it says
        // what was found and that `alsem tests` selects from it.
        let test_scope = crate::engine::test_select::scope::discover(
            &workspace,
            &crate::engine::test_select::scope::ScopeRequest::default(),
        );
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
            &test_scope,
            &callers,
            &through,
            self.req.single_app,
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

/// A page action as the one display names it (V3 §2.1):
/// `Page "CDO E-Mail Templates".action(ExportTemplateAction "Export Template")`.
fn action_display(object_type: &str, object: &str, action: &str, caption: Option<&str>) -> String {
    use crate::engine::l5::routine_rollup::{MemberPart, format_member, format_object};
    format!(
        "{}{}",
        format_object(object_type, object, None),
        format_member(
            &MemberPart {
                kind: "action",
                name: action,
                caption,
            },
            true
        )
    )
}

/// `selectors` in input order, each once.
fn distinct(selectors: &[String]) -> Vec<&String> {
    let mut seen = HashSet::new();
    selectors.iter().filter(|s| seen.insert(*s)).collect()
}

/// The `routine-ambiguous` text explain and prove share (G42). Output step
/// 1b (fable #7): with the candidates' rows known, each is named by the
/// selector that picks it (its parameter types), its location and stable id,
/// since an overload's owner and name alone are the same for all of them.
pub(crate) fn ambiguity_text(
    prefix: &str,
    selector: &str,
    candidates: &[String],
    routines: &[String],
) -> String {
    if routines.is_empty() {
        return format!(
            "{prefix}'{selector}' names {} routines ({}); qualify it as <owner>::<routine> or \
             pass a stable id",
            candidates.len(),
            candidates.join(", ")
        );
    }
    format!(
        "{prefix}'{selector}' names {} routines: {}; pass one as \
         <owner>::<routine>(<parameter types>) or its stable id",
        routines.len(),
        routines.join("; ")
    )
}

/// The first line of an unmatched routine selector's failure (V3 §2.4),
/// explain's and prove's: a stable id that names no routine, else a name.
pub(crate) fn unmatched_line(prefix: &str, selector: &str) -> String {
    if selector.contains('#') && !selector.contains("::") {
        format!("{prefix}no routine in the loaded program has the stable id '{selector}'")
    } else {
        format!("{prefix}no routine in the loaded program is named '{selector}'")
    }
}

/// A dependency app's guid in a failure's text, named from the loaded
/// apps' rows (V3 §2.4: the resolve step has no app names).
fn name_apps(text: &str, apps: &[Value]) -> String {
    let mut out = text.to_string();
    for a in apps {
        if let (Some(g), Some(n)) = (a["appId"].as_str(), a["name"].as_str()) {
            out = out.replace(
                &format!("found only in {}:", g.to_ascii_lowercase()),
                &format!("found only in {n}:"),
            );
            out = out.replace(
                &format!("; {}:", g.to_ascii_lowercase()),
                &format!("; {n}:"),
            );
        }
    }
    out
}

/// `--routine`'s diagnostic for one changed-roots diagnostic: prove's
/// `routine-not-found` / `routine-ambiguous` wording (G42).
fn selector_diagnostic(selector: &str, d: &ChangedRootsDiagnostic) -> Diagnostic {
    match d {
        ChangedRootsDiagnostic::SelectorAmbiguous {
            candidates,
            routines,
            listed,
            ..
        } => {
            let mut json = json!({
                "kind": "routine-ambiguous",
                "selector": selector,
                "candidates": candidates,
            });
            if !routines.is_empty() {
                json["candidateRoutines"] = json!(routines);
            }
            // V3 §2.4: every candidate's selector, ready for `routines`.
            if !listed.is_empty() {
                json["candidateSelectors"] = json!(
                    listed
                        .iter()
                        .map(|c| c.selector.clone())
                        .collect::<Vec<_>>()
                );
            }
            let human = if listed.is_empty() {
                ambiguity_text("explain: ", selector, candidates, routines)
            } else {
                crate::engine::l5::selector_index::ambiguity_lines("explain: ", selector, listed)
                    .join("\n")
            };
            Diagnostic { json, human }
        }
        ChangedRootsDiagnostic::SelectorUnmatched { hints, .. } => {
            // V3 §2.4: the nearest names (or where the name is found only in
            // dependency code) and the selector forms. A dependency app is
            // named by guid here and by name at render (`name_apps`).
            let mut json = json!({ "kind": "routine-not-found", "selector": selector });
            if let Value::Object(m) = &mut json {
                m.extend(crate::engine::l5::selector_index::hints_json(hints));
            }
            let mut lines = vec![unmatched_line("explain: ", selector)];
            lines.extend(
                crate::engine::l5::selector_index::hint_lines(hints, &[])
                    .into_iter()
                    .map(|l| format!("  {l}")),
            );
            Diagnostic {
                json,
                human: lines.join("\n"),
            }
        }
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
    // Output validation V2 (§4.2, F17): what was searched (the helper the
    // touches field mode prints), and the references that may write but are
    // not confirmed writers: `RecordRef.Field` by number and `TransferFields`
    // copies, from the same field-use index.
    let touches = crate::engine::l4::field_touches::field_touches(model, mt, mf, &|_| true);
    let search = crate::engine::l4::field_touches::search_words(
        &touches.coverage,
        touches.coverage.dependency_bodies > 0,
    );
    let candidates: Vec<String> = touches
        .sites
        .iter()
        .filter_map(|s| {
            use crate::program::body::record_op::FieldUse;
            let kind = if s.uses.contains(&FieldUse::ByNumber) {
                "RecordRef.Field by number"
            } else if s.uses.contains(&FieldUse::CopyCandidate) {
                "TransferFields copy"
            } else {
                return None;
            };
            Some(format!(
                "{kind} at {}:{}",
                s.anchor.source_unit_id,
                s.anchor.start_line + 1
            ))
        })
        .collect();
    let candidate_kinds = (
        candidates
            .iter()
            .filter(|c| c.starts_with("RecordRef.Field by number"))
            .count(),
        candidates
            .iter()
            .filter(|c| c.starts_with("TransferFields copy"))
            .count(),
    );
    // Round 2 G23: the declaring object's workspace unit and the field's own
    // OnValidate trigger (the card's follow-up).
    let of_object: Vec<&ModelRoutine> = model
        .workspace
        .routines
        .iter()
        .filter(|r| r.object_id == mf.declaring_object_id)
        .collect();
    let declaring_unit = of_object
        .iter()
        .map(|r| r.source_anchor.source_unit_id.clone())
        .find(|u| u.starts_with("ws:"));
    let on_validate = of_object
        .iter()
        .find(|r| {
            r.kind == "trigger"
                && r.name.eq_ignore_ascii_case("OnValidate")
                && r.enclosing_member
                    .as_deref()
                    .is_some_and(|m| m.eq_ignore_ascii_case(&mf.name))
        })
        .map(|r| r.stable_routine_id.clone());
    Resolved {
        roots: writers.keys().cloned().collect(),
        diagnostics: vec![],
        field: Some(FieldAnswer {
            table: mt.name.clone(),
            table_id: mt.id.clone(),
            field: mf.name.clone(),
            number: mf.field_number,
            captions: mf.captions.clone(),
            writers,
            search,
            candidates,
            candidate_kinds,
            declaring_unit,
            declared_at: None,
            on_validate,
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
    /// Object id -> the page actions whose `RunObject` runs it:
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
    /// The one routine display (V3 §2.1).
    names: &'m crate::engine::l5::routine_rollup::RoutineNames<'m>,
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
    fn over(
        model: &'m Model,
        data: &'m IndexData,
        names: &'m crate::engine::l5::routine_rollup::RoutineNames<'m>,
    ) -> Indexes<'m> {
        Indexes { model, data, names }
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

    /// The page actions whose `RunObject` runs row `i`: `i` is a trigger a
    /// run of its object reaches with no user acting (the resolver's rule,
    /// [`crate::program::impact_graph::reverse::run_reaches_trigger`]: a
    /// codeunit's `OnRun`, a page's, report's or XMLport's lifecycle
    /// triggers), the actions running its object; or the same trigger of a
    /// page or report extension, the actions running the object it extends.
    fn run_by_trigger(&self, i: usize) -> Vec<(usize, &'m CardAction, &'m str)> {
        use al_syntax::ir::ObjectKind;
        let r = self.row(i);
        if r.kind != "trigger" {
            return Vec::new();
        }
        let kind = match r.object_type.as_str() {
            "Codeunit" => ObjectKind::Codeunit,
            "Page" => ObjectKind::Page,
            "PageExtension" => ObjectKind::PageExtension,
            "Report" => ObjectKind::Report,
            "ReportExtension" => ObjectKind::ReportExtension,
            "XMLport" => ObjectKind::XmlPort,
            _ => return Vec::new(),
        };
        if !crate::program::impact_graph::reverse::run_reaches_trigger(
            kind,
            &r.name.to_ascii_lowercase(),
            r.enclosing_member.is_some(),
        ) {
            return Vec::new();
        }
        let base_type = match kind {
            ObjectKind::PageExtension => "Page",
            ObjectKind::ReportExtension => "Report",
            _ => return self.run_by(r.object_id.as_str()),
        };
        let objects = &self.model.workspace.objects;
        let Some(base) = self
            .data
            .declaring
            .of(objects, r)
            .and_then(|o| o.extends_target_name.as_deref())
        else {
            return Vec::new();
        };
        let base = base.fold_identifier();
        let ids: BTreeSet<&str> = objects
            .iter()
            .filter(|o| o.object_type == base_type && o.name.fold_identifier() == base)
            .map(|o| o.id.as_str())
            .collect();
        ids.into_iter().flat_map(|id| self.run_by(id)).collect()
    }

    /// Whether row `i` is a page action's `OnAction` trigger: the rows whose
    /// actions explain counts under "reached from page actions" (F16j), so
    /// they are not counted as routine callers too.
    fn is_action_trigger(&self, i: usize) -> bool {
        self.row(i)
            .card_text
            .as_deref()
            .is_some_and(|t| !t.actions.is_empty())
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

    /// The one routine display of row `i` as a list row prints it (V3
    /// §2.1: no member caption; the card header has it).
    fn qualified(&self, i: usize) -> String {
        format_routine(&self.names.parts_of(self.row(i)), false)
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

/// What a routine itself changes (output validation V2, §5.1, F75): the
/// object globals it assigns and its own record operations that write, or
/// "none". One line, for the callers' context section.
fn own_state_changes(model: &crate::program::model::workspace::Model, root: &str) -> String {
    let Some(r) = model
        .workspace
        .routines
        .iter()
        .find(|r| r.stable_routine_id == root)
    else {
        return "own state changes: not known (no body in the model)".to_string();
    };
    let globals: BTreeSet<String> = r
        .variables
        .iter()
        .filter(|v| v.scope.as_deref() == Some("global"))
        .map(|v| v.name.to_ascii_lowercase())
        .collect();
    // The global as the assignment writes it (the model folds names).
    let mut set: BTreeMap<String, String> = BTreeMap::new();
    for a in &r.var_assignments {
        let head = a
            .lhs_name
            .split(['.', '['])
            .next()
            .unwrap_or("")
            .trim_matches('"');
        if globals.contains(&head.to_ascii_lowercase()) {
            set.entry(head.to_ascii_lowercase())
                .or_insert_with(|| head.to_string());
        }
    }
    let set: Vec<String> = set.into_values().collect();
    let kind = model
        .workspace
        .objects
        .iter()
        .find(|o| o.id == r.object_id)
        .map_or("object".to_string(), |o| o.object_type.to_ascii_lowercase());
    let writes: Vec<String> = r
        .record_operations
        .iter()
        .filter(|o| {
            matches!(
                o.op.as_str(),
                "Insert" | "Modify" | "ModifyAll" | "Delete" | "DeleteAll" | "Rename" | "Validate"
            )
        })
        .map(|o| format!("{} on {}", o.op, o.record_variable_name))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let mut parts = Vec::new();
    if !set.is_empty() {
        parts.push(format!(
            "sets {} {kind} {} ({})",
            set.len(),
            if set.len() == 1 { "global" } else { "globals" },
            set.into_iter().collect::<Vec<_>>().join(", ")
        ));
    }
    parts.push(if writes.is_empty() {
        "no record write of its own".to_string()
    } else {
        format!("record writes of its own: {}", writes.join(", "))
    });
    format!("own state changes: {}", parts.join("; "))
}

/// The routine's direct callees in source order, at most [`CALLS_SHOWN`]
/// named (output validation V2, §9.1, F55): `calls: Codeunit X::A (called
/// at :181); ::B (called at :190); +2 more` (round-2 G41, G45). Builtins and unresolved calls are not routines and are left out.
/// Impact prints the same line per changed routine (round-2 r2 §3.9, G15).
pub(crate) fn calls_line(
    model: &crate::program::model::workspace::Model,
    names: &crate::engine::l5::routine_rollup::RoutineNames<'_>,
    root: &str,
) -> Option<String> {
    let routines = &model.workspace.routines;
    let r = routines.iter().find(|r| r.stable_routine_id == root)?;
    let stable_of: HashMap<&str, &str> = routines
        .iter()
        .map(|x| (x.id.as_str(), x.stable_routine_id.as_str()))
        .collect();
    // A callee in the routine's own object prints as `::Name` (the header
    // names the object); one elsewhere by its whole display.
    let own: HashMap<&str, &crate::program::model::workspace::ModelRoutine> = routines
        .iter()
        .filter(|x| x.object_id == r.object_id)
        .map(|x| (x.stable_routine_id.as_str(), x))
        .collect();
    let to_of: HashMap<&str, &str> = model
        .calls
        .edges
        .iter()
        .filter(|e| e.from == r.id)
        .filter_map(|e| Some((e.callsite_id.as_str(), e.to.as_deref()?)))
        .collect();
    let mut sites: Vec<&crate::program::body::features::PCallSite> = r.call_sites.iter().collect();
    sites.sort_by_key(|c| (c.source_anchor.start_line, c.source_anchor.start_column));
    let mut seen = HashSet::new();
    let mut items: Vec<(String, u32)> = Vec::new();
    for c in sites {
        let Some(to) = to_of.get(c.id.as_str()) else {
            continue;
        };
        let Some(stable) = stable_of.get(to) else {
            continue;
        };
        if !seen.insert(*stable) {
            continue;
        }
        let display = names.display(stable);
        let shown = match (own.get(stable), display.split_once("::")) {
            (Some(x), Some((_, rest))) if x.enclosing_member.is_none() => format!("::{rest}"),
            _ => display.clone(),
        };
        items.push((shown, c.source_anchor.start_line + 1));
    }
    if items.is_empty() {
        return Some("calls: none (no call to a routine; builtins aside)".to_string());
    }
    let more = items.len().saturating_sub(CALLS_SHOWN);
    items.truncate(CALLS_SHOWN);
    // Round-2 G41, G45: each callee in the one display (`Codeunit
    // "X"::A`; one of the card's own object as `::A`, the header names the
    // object), with its location's role (`called at :184`).
    let shown: Vec<String> = items
        .iter()
        .map(|(shown, line)| format!("{shown} (called at :{line})"))
        .collect();
    Some(format!(
        "calls: {}{}",
        shown.join("; "),
        if more > 0 {
            format!("; +{more} more")
        } else {
            String::new()
        }
    ))
}

/// How many direct callees explain's `calls:` line names.
/// (§9.1 says at most 5; 3 keeps a large card under the client budget.)
const CALLS_SHOWN: usize = 3;

/// The `ChangeCompany` calls in a routine's workspace cone (output
/// validation V2, §9.1, F75): the routine's own body and every workspace
/// routine its resolved calls reach, each `file:line` (1-based). The analysis
/// tracks no company per record (§10), so a write in such a cone prints
/// "target company: not established by this analysis".
fn change_company_sites(
    model: &crate::program::model::workspace::Model,
    root: &str,
) -> Vec<String> {
    EffectEvidenceIndex::new(model).change_company_sites(root)
}

/// What the effect evidence of a routine reads, built once per model and
/// shared by every routine asked (round 2 G7: impact asks it per changed
/// routine): the call adjacency for the `ChangeCompany` cone, the rows by
/// stable id, and per row the fields it assigns on each table (explain's
/// own field-write index).
pub(crate) struct EffectEvidenceIndex<'m> {
    model: &'m Model,
    by_id: HashMap<&'m str, usize>,
    by_stable: HashMap<&'m str, usize>,
    out_edges: HashMap<&'m str, Vec<&'m str>>,
    row_fields: std::sync::OnceLock<HashMap<usize, HashMap<String, BTreeSet<String>>>>,
}

impl<'m> EffectEvidenceIndex<'m> {
    pub(crate) fn new(model: &'m Model) -> EffectEvidenceIndex<'m> {
        let routines = &model.workspace.routines;
        let mut by_stable: HashMap<&str, usize> = HashMap::new();
        for (i, r) in routines.iter().enumerate() {
            by_stable.entry(r.stable_routine_id.as_str()).or_insert(i);
        }
        let mut out_edges: HashMap<&str, Vec<&str>> = HashMap::new();
        for e in &model.calls.edges {
            if let Some(to) = &e.to {
                out_edges
                    .entry(e.from.as_str())
                    .or_default()
                    .push(to.as_str());
            }
        }
        EffectEvidenceIndex {
            model,
            by_id: routines
                .iter()
                .enumerate()
                .map(|(i, r)| (r.id.as_str(), i))
                .collect(),
            by_stable,
            out_edges,
            row_fields: std::sync::OnceLock::new(),
        }
    }

    /// The `ChangeCompany` call sites of `root`'s cone ([`change_company_sites`]).
    pub(crate) fn change_company_sites(&self, root: &str) -> Vec<String> {
        let routines = &self.model.workspace.routines;
        let Some(&start) = self.by_stable.get(root) else {
            return Vec::new();
        };
        let mut seen = HashSet::from([start]);
        let mut queue = VecDeque::from([start]);
        let mut sites = BTreeSet::new();
        while let Some(i) = queue.pop_front() {
            let r = &routines[i];
            for c in &r.call_sites {
                let t = c.callee_text.to_ascii_lowercase();
                if t == "changecompany" || t.ends_with(".changecompany") {
                    sites.insert(format!(
                        "{}:{}",
                        c.source_anchor.source_unit_id,
                        c.source_anchor.start_line + 1
                    ));
                }
            }
            for to in self.out_edges.get(r.id.as_str()).into_iter().flatten() {
                if let Some(&j) = self.by_id.get(to)
                    && seen.insert(j)
                {
                    queue.push_back(j);
                }
            }
        }
        sites.into_iter().collect()
    }

    /// The fields row `row` assigns on table `table_id` (explain's
    /// `row_fields`, the same field-write index).
    fn assigned(&self, row: usize, table_id: &str) -> Option<&BTreeSet<String>> {
        let index = self.row_fields.get_or_init(|| {
            let mut m: HashMap<usize, HashMap<String, BTreeSet<String>>> = HashMap::new();
            for w in field_writes(self.model) {
                m.entry(w.routine_ix)
                    .or_default()
                    .entry(w.table_id.to_string())
                    .or_default()
                    .insert(w.field.to_string());
            }
            m
        });
        index.get(&row)?.get(table_id)
    }
}

/// How many written tables the effect evidence names (explain's
/// tables-when-few rule).
const EVIDENCE_TABLES: usize = 3;

/// Round 2 G7 (r1 F56, F75): a routine's effect evidence by explain's rules,
/// for impact's effects clause. When it may write 1 to 3 tables: each table
/// with its operation kinds, the representative site, and the fields its
/// owning routine assigns on it (Insert and Modify, a routine-level fact as
/// explain says); a table written only beyond a dependency call says so.
/// When the cone writes, its `ChangeCompany` sites (the target company is
/// not established). JSON: `{tables: [{table, ops, at, assigns: [{by,
/// fields}], beyondDependencyCall}], changeCompany: [..]}`; `by` is null for
/// the routine itself.
pub(crate) fn effect_evidence(
    entry: &DigestEntryFull,
    r: &crate::engine::l5::routine_rollup::RoutineRollup,
    rcx: &RollupCx<'_>,
    index: &EffectEvidenceIndex<'_>,
) -> Value {
    let mut tables: Vec<Value> = Vec::new();
    if (1..=EVIDENCE_TABLES).contains(&r.may_write.len()) {
        for (id, name) in &r.may_write {
            let Some(t) = r.workspace_code.iter().find(|t| &t.table_id == id) else {
                tables.push(json!({"table": name, "beyondDependencyCall": true}));
                continue;
            };
            let rep = &entry.effects[t.rep];
            let at = match (&rep.evidence.file, rep.evidence.line) {
                (Some(f), Some(l)) => Some(format!("{f}:{}", crate::engine::line_base::shown(l))),
                _ => None,
            };
            // The fields each owning routine assigns on the table.
            let mut by_owner: BTreeMap<Option<String>, BTreeSet<String>> = BTreeMap::new();
            for &i in &t.rows {
                for op in &entry.effects[i].origin_operation_ids {
                    if !matches!(rcx.op_kind(op), Some("Insert" | "Modify")) {
                        continue;
                    }
                    let Some(owner) = rcx.op_owner(op) else {
                        continue;
                    };
                    let Some(&row) = index.by_stable.get(owner.stable_id.as_str()) else {
                        continue;
                    };
                    if let Some(fields) = index.assigned(row, id) {
                        let by =
                            (owner.stable_id != entry.routine_id).then(|| owner.display.clone());
                        by_owner
                            .entry(by)
                            .or_default()
                            .extend(fields.iter().cloned());
                    }
                }
            }
            tables.push(json!({
                "table": name,
                "ops": t.kinds,
                "at": at,
                "assigns": by_owner
                    .into_iter()
                    .map(|(by, fields)| json!({"by": by, "fields": fields}))
                    .collect::<Vec<_>>(),
                "beyondDependencyCall": false,
            }));
        }
    }
    let writes = entry
        .effects
        .iter()
        .any(|f| f.effect_type.starts_with("DB_"));
    let company = if writes {
        index.change_company_sites(&entry.routine_id)
    } else {
        Vec::new()
    };
    json!({"tables": tables, "changeCompany": company})
}

/// One card's callers through dependency code under `--callers-context`
/// (callers-through-dependency design r2 §11.3, `impact_graph::dep_callers`):
/// the JSON `throughDependency` (counts, the catalog, the 3 nearest code
/// callers with their witnesses) and its human line. `sited` is `None` when
/// the declared graph did not build. Explain's `callers:` count is the direct
/// callers; these are transitive, and only those the walk reaches without a
/// workspace-only path.
fn through_dependency_card(
    model: &Model,
    sited: Option<&crate::program::impact_graph::SitedGraph>,
    root: &str,
    no_dependency_code: bool,
    surface: crate::engine::follow_up::Surface,
) -> (Value, String) {
    use crate::program::impact_graph::dep_callers::{self, DepTier, is_dependency_node};
    let mut m = Map::new();
    m.insert("relation".into(), dep_callers::RELATION.into());
    if no_dependency_code {
        m.insert("notApplicable".into(), "no dependency code loaded".into());
        return (
            Value::Object(m),
            "through dependency code: not applicable (no dependency code loaded)".into(),
        );
    }
    let Some(g) = sited else {
        m.insert("notComputed".into(), "the sited graph did not build".into());
        return (
            Value::Object(m),
            "through dependency code: not computed (the sited graph did not build)".into(),
        );
    };
    let roots = g.nodes_with_stable_id(root);
    if roots.is_empty() {
        m.insert(
            "notComputed".into(),
            "the routine has no node in the sited graph".into(),
        );
        return (
            Value::Object(m),
            "through dependency code: not computed (the routine has no node in the sited graph)"
                .into(),
        );
    }
    let answer = dep_callers::callers_through_dependency(
        g,
        &roots,
        &crate::program::impact_graph::reverse::WalkOptions::default(),
    );
    // V3 §2.1/§2.2: the one routine display, from the model row, else from
    // the graph node's declared names.
    let names = crate::engine::l5::routine_rollup::RoutineNames::new(model, &[]);
    let name = |n: u32| -> String {
        g.stable_id(n)
            .and_then(|s| names.display_with(&s, false))
            .or_else(|| crate::engine::l5::routine_rollup::graph_display(g, n, Some(model)))
            .unwrap_or_else(|| g.display(n))
    };
    let code: Vec<&dep_callers::DepCaller> = answer
        .callers
        .iter()
        .filter(|c| c.tier == DepTier::Code)
        .collect();
    let configured: Vec<&dep_callers::DepCaller> = answer
        .callers
        .iter()
        .filter(|c| c.tier == DepTier::Configured)
        .collect();
    let nearest: Vec<&dep_callers::DepCaller> = code.iter().take(3).copied().collect();
    m.insert("engine".into(), "reverse-walk".into());
    m.insert("feasibility".into(), "notChecked".into());
    m.insert("callers".into(), code.len().into());
    m.insert("configured".into(), configured.len().into());
    m.insert("catalog".into(), g.catalog().json());
    m.insert(
        "limited".into(),
        answer
            .limited
            .map_or(Value::Null, |l| dep_callers::limit_str(l).into()),
    );
    m.insert(
        "nearest".into(),
        nearest
            .iter()
            .map(|c| {
                let mut row = Map::new();
                row.insert("routine".into(), name(c.node).into());
                row.insert("stableId".into(), c.stable_id.as_str().into());
                row.extend(dep_callers::caller_json(g, c, &name));
                Value::Object(row)
            })
            .collect::<Vec<_>>()
            .into(),
    );
    let limited = answer.limited.map_or(String::new(), |l| {
        format!(
            "; the walk stopped at its {} cap, so this is a lower bound",
            dep_callers::limit_str(l)
        )
    });
    let mut line = if code.is_empty() {
        "through dependency code: none by code calls".to_string()
    } else {
        let shown: Vec<String> = nearest
            .iter()
            .map(|c| {
                let via = c
                    .hops
                    .iter()
                    .find(|h| is_dependency_node(g, h.to))
                    .map(|h| format!(", via {}", name(h.to)))
                    .unwrap_or_default();
                let enters = c
                    .enters_at
                    .map(|n| format!(", enters at {}", name(n)))
                    .unwrap_or_default();
                format!("{} ({} hops{via}{enters})", name(c.node), c.hops.len())
            })
            .collect();
        format!(
            "through dependency code (potential callers by code calls, path feasibility not \
             checked): {} more — nearest: {}{}",
            code.len(),
            shown.join("; "),
            if code.len() > nearest.len() {
                match surface {
                    crate::engine::follow_up::Surface::Cli => {
                        " — alsem query touches --callers lists them"
                    }
                    crate::engine::follow_up::Surface::Mcp => {
                        " — alsem_query_touches with callers: true lists them"
                    }
                }
            } else {
                ""
            }
        )
    };
    if !configured.is_empty() {
        let arms: Vec<String> = dep_callers::arm_counts(&configured)
            .into_iter()
            .take(3)
            .map(|(e, a, n)| format!("{e}: {a} {n}"))
            .collect();
        line.push_str(&format!(
            "; {} only through configured dispatch ({})",
            configured.len(),
            arms.join(", ")
        ));
    }
    line.push_str(&limited);
    (Value::Object(m), line)
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
    test_scope: &crate::engine::test_select::scope::Scope,
    callers: &HashMap<String, crate::engine::l5::callers_context::CallersSection>,
    through: &HashMap<String, (Value, String)>,
    single_app: bool,
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
    // W2-2: one rollup context for every card (names, the operation index);
    // its routine names (V3 §2.1) are the cards' too.
    let rcx = RollupCx::new(model, apps);
    let ix = Indexes::over(model, &indexes.ix, &rcx.names);
    // The walk is this run's own: its memo and expansion budget start empty
    // whatever earlier runs over the same indexes did.
    let cross = CrossRules::over(model, &indexes.cross);
    // With the number of `[Test]` procedures loaded (step 1b): "none reach
    // it" and "no test code" are different answers.
    let tests_reaching = effects.coverage_graph.as_ref().map(|g| {
        let _s = span("explain", "explain.reaching_tests");
        (
            crate::engine::l5::test_coverage::reaching_tests(
                g,
                &resolved.roots.iter().cloned().collect(),
            ),
            g.test_procedures(),
        )
    });
    let cards_span = span("explain", "explain.cards");
    // §5.2: with one card its summary line is the answer's `answer:` line.
    let single = resolved.roots.len() == 1 && resolved.field.is_none();
    let fcx = crate::engine::follow_up::Cx::new(opts.surface, single_app);
    let mut cards: Vec<RenderedCard> = resolved
        .roots
        .iter()
        .map(|rid| {
            let entry = full.iter().find(|e| &e.routine_id == rid);
            let mut card = Card::build(
                &ix,
                &cross,
                rid,
                entry,
                &rcx,
                CardTests {
                    reaching: tests_reaching.as_ref().map(|(t, n)| (t, *n)),
                    scope: Some(test_scope),
                },
                opts,
            );
            // §6.2: the follow-ups this card does not answer itself, in the
            // surface's words (§6.1); JSON `followUps`.
            let selector = rcx.names.selector(rid);
            let no_path_table = card
                .writes
                .iter()
                .find(|w| w["noPathKept"] == json!(true))
                .and_then(|w| w["table"].as_str());
            let follow_ups =
                card.follow_ups(fcx, &selector, callers.contains_key(rid), no_path_table);
            let next: Vec<String> = follow_ups
                .iter()
                .map(|f| format!("{} — {}", f.text(opts.surface), f.why))
                .collect();
            let mut json = card.json();
            json["followUps"] = crate::engine::follow_up::json_list(&follow_ups);
            // Output validation V2 (§5.1, F29, F75): the routine's own state
            // changes, the target company of its writes, and the callers'
            // context section lead the card, right after its head.
            let mut lead: Vec<String> = Vec::new();
            // §9.1 (F55): the direct callees first, and the short route to
            // the UI effect the card names (a callee list alone does not say
            // how the routine reaches it).
            if let Some(l) = calls_line(model, &rcx.names, rid) {
                lead.push(l);
            }
            if let Some(e) = entry.and_then(|e| {
                e.effects
                    .iter()
                    .filter(|f| {
                        // UI, not an error (UI_ERROR is the errors family).
                        f.effect_type.starts_with("UI_")
                            && f.effect_type != "UI_ERROR"
                            && !f.literal_unreachable
                    })
                    .find(|f| f.via_paths.iter().any(|p| !p.is_empty()))
            }) && let Some(p) = e
                .via_paths
                .iter()
                .filter(|p| !p.is_empty())
                .min_by_key(|p| p.len())
            {
                // A short route (§9.1): each hop by its name, an overloaded
                // routine's parameter list left out (the card's sections
                // carry the full displays).
                let route: Vec<String> = rcx
                    .path_line(rid, p)
                    .split(" → ")
                    .map(|hop| {
                        // The routine's name: no object, no parameter list.
                        let name = hop.rfind("::").map_or(hop, |i| &hop[i + 2..]);
                        name.split('(').next().unwrap_or(name).trim().to_string()
                    })
                    .collect();
                // The routine and the last hops before the effect (a long
                // route elides its middle; `detail` and prove print it whole).
                const ROUTE_TAIL: usize = 2;
                let route = if route.len() > ROUTE_TAIL + 1 {
                    format!(
                        "{} → … → {}",
                        route[0],
                        route[route.len() - ROUTE_TAIL..].join(" → ")
                    )
                } else {
                    route.join(" → ")
                };
                // The place prints on the card's UI line; here the route.
                lead.push(format!("UI route: {} via {route}", e.effect_type));
            }
            let writes =
                entry.is_some_and(|e| e.effects.iter().any(|f| f.effect_type.starts_with("DB_")));
            if writes {
                let sites = change_company_sites(model, rid);
                if !sites.is_empty() {
                    json["targetCompany"] = json!({"established": false, "changeCompany": sites});
                    lead.push(format!(
                        "target company: not established by this analysis (ChangeCompany at {})",
                        sites.join(", ")
                    ));
                }
            }
            // `--callers-context` (agent-tasks §6.6): the callers' context
            // section. Round-2 G24: without `detail` the card leads with the
            // callers (its `answer:` line is the three callers' answers, the
            // `writes:` line joins the summary lines), and the lines above
            // follow the summary lines.
            let callers_first = callers.get(rid).is_some() && !opts.detail;
            let mut after_summary: Vec<String> = Vec::new();
            if let Some(section) = callers.get(rid) {
                json["callersContext"] = section.json.clone();
                if callers_first {
                    after_summary = std::mem::take(&mut lead);
                    let summary = &card.header.summary;
                    let (first, rest) = summary.split_once('\n').unwrap_or((summary, ""));
                    let writes = first.strip_prefix("summary — ").unwrap_or(first);
                    card.header.summary = format!(
                        "summary — {}\n    {writes}{}{rest}",
                        section.answer,
                        if rest.is_empty() { "" } else { "\n" }
                    );
                }
                lead.push(own_state_changes(model, rid));
                lead.extend(section.lines.iter().cloned());
            }
            let (head, body) =
                card.human_parts(single, &next, callers_first.then_some(&after_summary[..]));
            let head_len = head.len();
            let mut human = format!("{head}{body}");
            // Before the `next:` lines, else before the card's closing blank
            // line.
            let before_next = |h: &str| {
                h.find("\n  next:")
                    .map_or(h.len().saturating_sub(1), |i| i + 1)
            };
            // `--writes-table` (W2-2 gap (a)): prove's own answer per table,
            // before `next:`.
            if let Some((values, lines)) = table_answers.get(rid) {
                json["tableAnswers"] = json!(values);
                let at = before_next(&human);
                let text: String = lines.iter().map(|l| format!("  {l}\n")).collect();
                human.insert_str(at, &text);
            }
            // `--callers-context`: the callers through dependency code, on
            // the line after `callers:` (design r2 §11.3).
            if let Some((value, line)) = through.get(rid) {
                json["throughDependency"] = value.clone();
                let at = human
                    .find("\n  callers:")
                    .and_then(|i| human[i + 1..].find('\n').map(|j| i + 1 + j + 1))
                    .unwrap_or_else(|| before_next(&human));
                human.insert_str(at, &format!("    {line}\n"));
            }
            if !lead.is_empty() {
                let text: String = lead.iter().map(|l| format!("  {l}\n")).collect();
                human.insert_str(head_len, &text);
            }
            (
                card.sort_key.clone(),
                rid.clone(),
                json,
                human,
                (card.title.clone(), card.header.clone()),
            )
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
        Value::Array(cards.iter().map(|c| c.2.clone()).collect::<Vec<_>>()),
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
    // §5.2: the header's facts; JSON `result.scope` (F76).
    let no_answer =
        (resolved.unresolved && cards.is_empty() && resolved.field.is_none()).then(|| {
            resolved.diagnostics.first().map_or_else(
                || "no routine resolved".to_string(),
                |d| {
                    let first = name_apps(&d.human, apps)
                        .lines()
                        .next()
                        .unwrap_or("")
                        .to_string();
                    first
                        .strip_prefix("explain: ")
                        .map_or(first.clone(), str::to_string)
                },
            )
        });
    let lookup_miss = no_answer.is_some();
    let header = explain_header(
        mode,
        &cards,
        resolved,
        apps.len(),
        opts,
        single_app,
        no_answer,
    );
    doc.insert(
        "result".into(),
        json!({"scope": header.scope_json(Map::new())}),
    );
    // §9.8 (F78): every location says what it is.
    let mut doc = Value::Object(doc);
    crate::engine::result_header::tag_location_roles(&mut doc);
    let json_text = serialize_normalized_value(&sort_keys_keep_nulls(doc));

    // Round-2 G40: a lookup miss prints its failure sentence once (the
    // `answer:` line), then what helps the next call.
    let mut human = if lookup_miss {
        header.miss_human()
    } else {
        header.human()
    };
    if let Some(f) = &resolved.field {
        human.push_str(&field_human(&ix, f, entry, fcx));
    }
    // Round 2 G23: a field answer is the field card only; its writers'
    // cards are follow-up calls (the JSON keeps them).
    if resolved.field.is_none() {
        for c in &cards {
            human.push_str(&c.3);
        }
    }
    if cards.is_empty() && resolved.field.is_none() && resolved.diagnostics.is_empty() {
        human.push_str("explain: no routine resolved\n");
    }
    for (i, d) in resolved.diagnostics.iter().enumerate() {
        let text = name_apps(&d.human, apps);
        // The first diagnostic's first line is the miss's `answer:` line.
        let text = if lookup_miss && i == 0 {
            text.split_once('\n')
                .map_or("", |(_, rest)| rest)
                .to_string()
        } else {
            text
        };
        if !text.is_empty() {
            human.push_str(&text);
            human.push('\n');
        }
        if let Some(sels) = d.json["candidateSelectors"].as_array() {
            let sels: Vec<String> = sels
                .iter()
                .filter_map(|s| s.as_str().map(str::to_string))
                .collect();
            for l in crate::engine::l5::selector_index::complete_call_lines(
                crate::engine::follow_up::Tool::Explain,
                fcx,
                &sels,
                &[],
            ) {
                human.push_str(&l);
                human.push('\n');
            }
        }
    }
    let mut summary = explain_summary(&cards, resolved.diagnostics.len(), &header);
    // §6.3 (round-2 c26): a no-answer's outcome from its diagnostics' kinds.
    if resolved.unresolved
        && let Some(o) = crate::engine::result_header::outcome_of_kinds(
            resolved
                .diagnostics
                .iter()
                .filter_map(|d| d.json["kind"].as_str()),
        )
        && let Value::Object(m) = &mut summary
    {
        m.insert("outcome".into(), o.into());
    }
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

/// One rendered card: (sort key, routine id, JSON, human, (title, header
/// facts)).
type RenderedCard = (
    Option<(String, u32, u32)>,
    String,
    Value,
    String,
    (String, HeaderFacts),
);

/// The answer's four header lines (output validation §5.2): the resolved
/// inputs, the answer (one card: its summary line), the analysis,
/// assumptions, limits and scope of every card at once, and what shows more.
#[allow(clippy::too_many_arguments)]
fn explain_header(
    mode: &ExplainMode,
    cards: &[RenderedCard],
    resolved: &Resolved,
    deps: usize,
    opts: RenderOpts,
    single_app: bool,
    no_answer: Option<String>,
) -> crate::engine::result_header::Header {
    use crate::engine::follow_up::Input;
    use crate::engine::result_header::{Analysis, Header, dependencies_clause};
    let titles: Vec<String> = cards.iter().map(|c| c.4.0.clone()).collect();
    let listed = |titles: &[String]| {
        let f = fold(titles.to_vec(), 3, "routines", false);
        f.joined()
    };
    let inputs = match (mode, &resolved.field) {
        (_, Some(f)) => format!("field {}.\"{}\"", f.table, f.field),
        (ExplainMode::Diff { .. }, _) => match titles.len() {
            1 => format!("diff: 1 changed routine, {}", titles[0]),
            n => format!("diff: {n} changed routines, {}", listed(&titles)),
        },
        (ExplainMode::Routines(selectors), _) => match titles.len() {
            0 => distinct(selectors)
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join(", "),
            1 => titles[0].clone(),
            n => format!("{n} routines: {}", listed(&titles)),
        },
        (ExplainMode::Field { table, field }, None) => format!("field {table}.\"{field}\""),
    };
    let mut h = Header::new("explain", inputs, single_app);
    h.answer = match (&no_answer, &resolved.field, cards) {
        (Some(why), _, _) => format!("no answer — {why}"),
        (None, Some(f), _) => {
            // Round-2 G39: "writes" means a record write; a field's
            // writers are its field assignments and validations.
            let (searched, not_searched) = &f.search;
            match f.writers.len() {
                0 => format!(
                    "the field is assigned or validated by no routine: {}",
                    crate::engine::vocab::none_observed(searched, not_searched)
                ),
                1 => "the field is assigned or validated by 1 routine".to_string(),
                n => format!("the field is assigned or validated by {n} routines"),
            }
        }
        (None, None, [one]) => one.4.1.answer().to_string(),
        (None, None, many) => format!(
            "{} cards, one per routine; each card leads with its summary line",
            many.len()
        ),
    };
    let mut analysis = Analysis::Complete;
    let mut limits: Vec<String> = Vec::new();
    for c in cards {
        analysis = analysis.and(c.4.1.analysis.clone());
        for l in &c.4.1.limits {
            if !limits.contains(l) {
                limits.push(l.clone());
            }
        }
    }
    if let Some(f) = &resolved.field
        && !f.search.1.is_empty()
    {
        analysis = analysis.and(Analysis::Partial(vec![format!(
            "not searched: {}",
            f.search.1
        )]));
    }
    if no_answer.is_some() {
        analysis = Analysis::NotAnalyzed("no routine resolved".to_string());
    }
    h.analysis = analysis;
    h.limits = limits;
    let depth = match cards {
        [one] => one.4.1.depth.clone(),
        [] => String::new(),
        _ => "dependency code followed through every resolved call with literal arguments"
            .to_string(),
    };
    let mut scope: Vec<String> = Vec::new();
    if !depth.is_empty() {
        scope.push(depth);
        scope.push("no lock facts".to_string());
    }
    scope.push(dependencies_clause(deps));
    h.scope = scope.join("; ");
    let s = opts.surface;
    if !cards.is_empty() && !opts.detail {
        h.more
            .push(format!("{} lists every list in full", Input::Detail.on(s)));
    }
    h.more
        .push(format!("{} for every row", Input::FormatJson.on(s)));
    h
}

/// What the cards say, for a front end that delivers them as a file (MCP output
/// plan 1e): each card's header line and the length of its lists, read off the
/// card's own JSON. Capped (`capped_summary`): cards are dropped from the end.
fn explain_summary(
    cards: &[RenderedCard],
    diagnostics: usize,
    header: &crate::engine::result_header::Header,
) -> Value {
    let len = |v: &Value, ptr: &str| v.pointer(ptr).and_then(Value::as_array).map_or(0, Vec::len);
    let listed: Vec<Value> = cards
        .iter()
        .map(|(_, _, j, h, (_, facts))| {
            json!({
                "card": h.lines().next().unwrap_or(""),
                "selector": j.pointer("/routine/selector").cloned().unwrap_or(Value::Null),
                "summary": facts.answer(),
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
    // §5.2 (F42): each card's identity, summary line and asked verdicts are
    // a result index row, whatever the summary has room for.
    let rows: Vec<Value> = cards
        .iter()
        .map(|(_, rid, j, h, (_, facts))| {
            json!({
                "card": h.lines().next().unwrap_or(""),
                "stableId": rid,
                "selector": j.pointer("/routine/selector").cloned().unwrap_or(Value::Null),
                "summary": facts.answer(),
                "analysis": facts.analysis.human(),
                "tableAnswers": j.get("tableAnswers").cloned(),
            })
        })
        .collect();
    crate::engine::result_index::with_index(
        json!({
            "kind": "explain",
            "answer": header.answer,
            "analysis": header.analysis.human(),
            "cardCount": cards.len(),
            "diagnostics": diagnostics,
            "cards": listed,
        }),
        "cards",
        "selector",
        rows,
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
        // Output validation V2 (§4.2, additive): the search and the
        // reference candidates beside the confirmed writers.
        "fieldNumber": f.number,
        "searched": {"searched": f.search.0, "notSearched": f.search.1},
        "referenceCandidates": f.candidates,
    })
}

fn field_human(
    ix: &Indexes<'_>,
    f: &FieldAnswer,
    entry: Option<&FieldWriters>,
    fcx: crate::engine::follow_up::Cx,
) -> String {
    // Output validation V2 (§4.2, F17): a count with its search, never a
    // bare `0`; confirmed writers and reference candidates counted apart.
    let quote = |s: &str| {
        if s.chars().all(|c| c.is_alphanumeric() || c == '_') {
            s.to_string()
        } else {
            format!("\"{s}\"")
        }
    };
    let mut out = format!(
        "field: {}.{} (field {}",
        quote(&f.table),
        quote(&f.field),
        f.number
    );
    // Round 2 G23: where the field is declared.
    if let Some(at) = &f.declared_at {
        out.push_str(&format!(" declared at {at}"));
    }
    if let Ok(Some(c)) = one_text(&f.captions) {
        out.push_str(&format!(", caption \"{c}\""));
    }
    out.push_str(")\n");
    let (searched, not_searched) = &f.search;
    if f.writers.is_empty() {
        out.push_str(&format!(
            "  confirmed field writers (assignment or validation): {}\n",
            crate::engine::vocab::none_observed(searched, not_searched)
        ));
    } else {
        out.push_str(&format!(
            "  confirmed field writers (assignment or validation): {}\n",
            f.writers.len()
        ));
    }
    let kind_words = |k: WriteKind| match k {
        WriteKind::Assign => {
            "assignment (changes the record in memory; persisted by whoever modifies or inserts it)"
        }
        WriteKind::Validate => "Validate (also runs the field's OnValidate)",
        WriteKind::ModifyAll => "ModifyAll (a set-based write to the database)",
    };
    for (id, sites) in writers_by_anchor(f) {
        let display = ix
            .rows_of(id.as_str())
            .first()
            .map_or_else(|| id.clone(), |&i| ix.qualified(i));
        let at: Vec<String> = sites
            .iter()
            .map(|(k, a)| format!("{} at {}", kind_words(*k), anchor_human(a)))
            .collect();
        out.push_str(&format!("    {display} — {}\n", at.join(", ")));
        for (_, a) in sites {
            let (_, site) = index_site(entry, id, a);
            let rules = site.map_or(&[][..], |s| s.rules.as_slice());
            // Round 2 G23: a site with no rule prints nothing (the card
            // lists the rules a site has; `no rule` said nothing checked).
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
    // Round 2 G23: the reference candidates counted per kind.
    out.push_str(&format!(
        "  references that may write, not confirmed (RecordRef.Field by number {}, \
         TransferFields copies {}): {}\n",
        f.candidate_kinds.0,
        f.candidate_kinds.1,
        if f.candidates.is_empty() {
            "none observed (search below)".to_string()
        } else {
            let mut s = String::new();
            s.push_str(
                &f.candidates
                    .iter()
                    .take(5)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join("; "),
            );
            if f.candidates.len() > 5 {
                s.push_str(&format!("; … +{}", f.candidates.len() - 5));
            }
            s
        }
    ));
    out.push_str("  writer kinds searched: assignment, Validate, ModifyAll\n");
    out.push_str(&format!(
        "  searched: {searched}; not searched: {not_searched}\n"
    ));
    // Round 2 G23: complete calls for both follow-ups: the field's own
    // trigger (its card is not repeated here) and its other uses.
    use crate::engine::follow_up::{FollowUp, Tool};
    if let Some(t) = &f.on_validate {
        let call = FollowUp::new(Tool::Explain, fcx, "the field's OnValidate trigger")
            .arg("routines", vec![ix.names.selector(t)]);
        out.push_str(&format!(
            "  the field's OnValidate: {}\n",
            call.text(fcx.surface)
        ));
    }
    let writer_cards: Vec<String> = f
        .writers
        .keys()
        .filter(|w| f.on_validate.as_ref() != Some(*w))
        .map(|w| ix.names.selector(w))
        .collect();
    if !writer_cards.is_empty() {
        let call =
            FollowUp::new(Tool::Explain, fcx, "the writers' cards").arg("routines", writer_cards);
        out.push_str(&format!(
            "  the writers' cards: {}\n",
            call.text(fcx.surface)
        ));
    }
    let touches = FollowUp::new(Tool::QueryTouches, fcx, "reads, filters and validations")
        .arg("table", f.table.clone())
        .arg("field", f.field.clone());
    out.push_str(&format!(
        "  reads, filters and validations: {}\n",
        touches.text(fcx.surface)
    ));
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
    /// The direct callers as one set (round-2 r2 §2.3, G3).
    direct: DirectCallers,
    /// What runs a trigger (round-2 r2 §2.4, G4's W1 rows); `None` for a
    /// routine that is not a trigger or subscriber.
    run_by: Option<RunBy>,
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
    /// The sibling test scope (agent-tasks C-7): discovered, never analyzed
    /// here.
    test_scope: Option<crate::engine::test_select::scope::Scope>,
    gaps: Vec<Gap>,
    /// W2-2: the rollup's totals (with callers, page actions and tests).
    totals: RollupTotals,
    coverage: Value,
    gaps_in_dependencies: Value,
    /// The card's summary, analysis, limits and dependency depth (the
    /// answer's header line carries them, output validation §5.2).
    header: HeaderFacts,
    /// `notes: …` (gap notes), when any.
    notes: Option<String>,
    /// The human lines of the shared sections, rendered once.
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
            dependency_routines: 0,
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

/// Per routine, the `[Test]` procedures that reach it (`reaching_tests`).
type ReachingTests = BTreeMap<String, Vec<(String, bool)>>;

/// What a card knows about tests: the `[Test]` procedures that reach each
/// routine with the number loaded (`None`: no test search ran), and the
/// test scope discovery's result (C-7; the summary reads it too).
#[derive(Clone, Copy)]
struct CardTests<'a> {
    reaching: Option<(&'a ReachingTests, usize)>,
    scope: Option<&'a crate::engine::test_select::scope::Scope>,
}

/// The callers a caller rule names one by one before it counts them by
/// object kind (§7.7, F47).
const CALLER_RULE_NAMES: usize = 3;

/// Routine displays counted by their object's kind, most first (ties by
/// kind): `5 pages, 14 page extensions` (§7.7, F47).
fn by_object_kind(displays: &[String]) -> String {
    let mut by: BTreeMap<&str, usize> = BTreeMap::new();
    for d in displays {
        *by.entry(d.split(' ').next().unwrap_or("")).or_default() += 1;
    }
    let mut v: Vec<(&str, usize)> = by.into_iter().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    v.into_iter()
        .map(|(kind, n)| {
            let words = match kind {
                "PageExtension" => "page extension",
                "TableExtension" => "table extension",
                "ReportExtension" => "report extension",
                "EnumExtension" => "enum extension",
                "PageCustomization" => "page customization",
                "Codeunit" => "codeunit",
                "Page" => "page",
                "Table" => "table",
                "Report" => "report",
                "Query" => "query",
                "XmlPort" | "Xmlport" => "xmlport",
                other => other,
            };
            let many = match words {
                "query" => "queries".to_string(),
                w => format!("{w}s"),
            };
            rf_plural(n, words, &many)
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// The page objects with the most actions, most first, at most `cap` named
/// (§7.7, F47): a tie the cap would cut prints as one count, `6 page objects
/// with 5 each`, never as an arbitrary few of them; the rest is counted.
fn ties_folded(ranked: &[(String, usize)], cap: usize) -> String {
    let mut parts: Vec<String> = Vec::new();
    let mut named = 0;
    let mut i = 0;
    while i < ranked.len() && named < cap {
        let n = ranked[i].1;
        let tie = ranked[i..].iter().take_while(|(_, m)| *m == n).count();
        if named + tie <= cap {
            parts.extend(ranked[i..i + tie].iter().map(|(o, m)| format!("{o} {m}")));
            named += tie;
        } else {
            parts.push(format!(
                "{} with {n} each",
                rf_plural(tie, "page object", "page objects")
            ));
            named = cap;
        }
        i += tie;
    }
    if i < ranked.len() {
        parts.push(format!(
            "… +{}",
            rf_plural(ranked.len() - i, "page object", "page objects")
        ));
    }
    parts.join(", ")
}

/// "N page actions reach it in all" with its verb agreeing.
fn actions_reach(n: usize) -> String {
    if n == 1 {
        "1 page action reaches it in all".to_string()
    } else {
        format!(
            "{} page actions reach it in all",
            crate::engine::l5::routine_rollup::thousands(n)
        )
    }
}

/// The direct callers of a routine as one set (round-2 r2 §2.3, G3): the
/// set the caller rules count. Its routines and triggers by name, its page
/// actions (whose trigger calls it) counted apart; `reached`: every page
/// action that reaches it, directly or through callers (`reached_from`).
struct DirectCallers {
    total: usize,
    routines: Vec<String>,
    /// How the routines and triggers are called (`routines and page
    /// triggers`), singular and plural.
    routine_label: (&'static str, &'static str),
    actions: Vec<String>,
    action_objects: usize,
    reached: usize,
}

impl DirectCallers {
    /// The parts of the direct set: `13 routines and page triggers, 30 page
    /// actions`.
    fn parts(&self) -> Vec<String> {
        let mut parts = Vec::new();
        if !self.routines.is_empty() {
            parts.push(rf_plural(
                self.routines.len(),
                self.routine_label.0,
                self.routine_label.1,
            ));
        }
        if !self.actions.is_empty() {
            parts.push(rf_plural(self.actions.len(), "page action", "page actions"));
        }
        parts
    }

    /// The summary clause: `callers: 43 direct (13 routines and page
    /// triggers, 30 page actions); 118 page actions reach it in all`.
    fn summary(&self) -> String {
        let reach = if self.reached > 0 {
            format!("; {}", actions_reach(self.reached))
        } else {
            String::new()
        };
        if self.total == 0 {
            return format!(
                "callers: none{}",
                if reach.is_empty() { "" } else { " direct" }
            ) + &reach;
        }
        format!(
            "callers: {} direct ({}){reach}",
            self.total,
            self.parts().join(", ")
        )
    }

    /// The card's line after `callers: ` (`detail` lists every name).
    fn human(&self, detail: bool, detail_input: &str) -> String {
        let reach = if self.reached > 0 {
            format!(
                "; {} (directly or through callers)",
                actions_reach(self.reached)
            )
        } else {
            String::new()
        };
        if self.total == 0 {
            return format!("none direct{reach}");
        }
        let mut parts = Vec::new();
        if !self.routines.is_empty() {
            parts.push(format!(
                "{}: {}",
                rf_plural(
                    self.routines.len(),
                    self.routine_label.0,
                    self.routine_label.1
                ),
                fold(self.routines.clone(), caps::TABLES, "callers", detail).joined()
            ));
        }
        if !self.actions.is_empty() {
            let n = rf_plural(self.actions.len(), "page action", "page actions");
            let objects = rf_plural(self.action_objects, "object", "objects");
            parts.push(if detail {
                format!("{n} in {objects}: {}", self.actions.join(", "))
            } else {
                format!("{n} in {objects} ({detail_input} lists them)")
            });
        }
        format!("{} direct — {}{reach}", self.total, parts.join("; "))
    }

    fn json(&self) -> Value {
        json!({
            "direct": self.total,
            "directRoutines": self.routines.len(),
            "directPageActions": self.actions.len(),
            "pageActionsInAll": self.reached,
        })
    }
}

/// How a card names its direct callers that are not page actions, from
/// their kinds: `routines`, `routines and page triggers`, `page triggers`, …
fn routine_label(ix: &Indexes<'_>, rows: &[usize]) -> (&'static str, &'static str) {
    let triggers: Vec<usize> = rows
        .iter()
        .copied()
        .filter(|&i| ix.row(i).kind == "trigger")
        .collect();
    let procedures = rows.len() > triggers.len();
    let pages = triggers
        .iter()
        .all(|&i| matches!(ix.row(i).object_type.as_str(), "Page" | "PageExtension"));
    match (procedures, triggers.is_empty(), pages) {
        (_, true, _) => ("routine", "routines"),
        (true, false, true) => ("routine or page trigger", "routines and page triggers"),
        (true, false, false) => ("routine or trigger", "routines and triggers"),
        (false, false, true) => ("page trigger", "page triggers"),
        (false, false, false) => ("trigger", "triggers"),
    }
}

/// What runs a trigger (round-2 r2 §2.4, the W1 rows): a field's
/// `OnValidate` (its `Validate` calls and the pages on its table), `OnLookup`
/// and `OnAssistEdit` (a user step only), a page action (an entry point), an
/// event subscriber (its publishers and any direct code caller); every other
/// trigger with no direct caller says what runs it is not listed yet (W3).
/// The card's `run by:` line, its summary clause and its JSON.
struct RunBy {
    human: String,
    summary: String,
    json: Value,
    /// A page action's own card: its `reached from` list is the action
    /// itself, which `entry point: this action` already says (G26).
    action: bool,
    /// A table or page trigger (round 2 G4, W3): the card keeps its
    /// `callers:` line beside `run by:`.
    keep_callers: bool,
}

/// How many `Validate` sites a `run by:` line names by default.
const RUN_BY_SITES: usize = 3;

impl RunBy {
    fn of(
        ix: &Indexes<'_>,
        rows: &[usize],
        rid: &str,
        direct: &DirectCallers,
        opts: RenderOpts,
    ) -> Option<RunBy> {
        let &first = rows.first()?;
        let r = ix.row(first);
        if r.kind == "event-subscriber"
            || crate::program::attributes::find_attribute(&r.attributes_parsed, "EventSubscriber")
                .is_some()
        {
            return Some(Self::subscriber(ix, rid, direct, opts));
        }
        if r.kind != "trigger" {
            return None;
        }
        if ix.is_action_trigger(first) {
            let human = "run by: entry point: this action (no code caller)".to_string();
            return Some(RunBy {
                summary: human.clone(),
                human,
                json: json!({"kind": "page-action", "directCodeCallers": direct.total}),
                action: true,
                keep_callers: false,
            });
        }
        let name = r.name.to_ascii_lowercase();
        let field_trigger = matches!(r.object_type.as_str(), "Table" | "TableExtension")
            && r.enclosing_member.is_some()
            && matches!(name.as_str(), "onvalidate" | "onlookup" | "onassistedit");
        if field_trigger && let Some(rb) = Self::field(ix, r, &name, opts) {
            return Some(rb);
        }
        // Round 2 G4 (W3 rows): a table's record trigger and a page trigger.
        let object_level = r.enclosing_member.is_none();
        if object_level
            && matches!(r.object_type.as_str(), "Table" | "TableExtension")
            && matches!(
                name.as_str(),
                "oninsert" | "onmodify" | "ondelete" | "onrename"
            )
            && let Some(rb) = Self::table_trigger(ix, rows, r, opts)
        {
            return Some(rb);
        }
        if matches!(r.object_type.as_str(), "Page" | "PageExtension") {
            return Some(Self::page_trigger(ix, rows, first, r, opts));
        }
        // Other triggers (a codeunit's OnRun, report triggers, …).
        (direct.total == 0).then(|| {
            let human = "direct code callers: none found (what runs a trigger is not listed for \
                         this kind yet)"
                .to_string();
            RunBy {
                summary: human.clone(),
                human,
                json: json!({"kind": "not-listed", "directCodeCallers": 0}),
                action: false,
                keep_callers: false,
            }
        })
    }

    /// An event subscriber: the events that raise it (the model's bound
    /// subscriptions), then its direct code callers.
    fn subscriber(ix: &Indexes<'_>, rid: &str, direct: &DirectCallers, opts: RenderOpts) -> RunBy {
        let model = ix.model;
        let objects: HashMap<&str, &ModelObject> = model
            .workspace
            .objects
            .iter()
            .map(|o| (o.id.as_str(), o))
            .collect();
        let events: HashMap<&str, &crate::program::model::events::EventSymbol> = model
            .events
            .graph
            .events
            .iter()
            .map(|e| (e.id.as_str(), e))
            .collect();
        let mut publishers: BTreeSet<String> = BTreeSet::new();
        for e in model
            .events
            .graph
            .edges
            .iter()
            .filter(|e| e.subscriber_stable_routine_id == rid)
        {
            let name = match events.get(e.event_id.as_str()) {
                Some(s) => {
                    let object = objects.get(s.publisher_object_id.as_str()).map_or_else(
                        || s.publisher_object_id.clone(),
                        |o| {
                            crate::engine::l5::routine_rollup::format_object(
                                &o.object_type,
                                &o.name,
                                None,
                            )
                        },
                    );
                    format!("{object}::{}", s.event_name)
                }
                None => e.event_id.clone(),
            };
            publishers.insert(name);
        }
        let publishers: Vec<String> = publishers.into_iter().collect();
        let raised = if publishers.is_empty() {
            "the publisher: no bound subscription found in the loaded program".to_string()
        } else {
            format!("the publisher: {}", publishers.join(", "))
        };
        let (callers_human, callers_summary) = if direct.total == 0 {
            ("none found".to_string(), "none found".to_string())
        } else {
            (
                direct.human(opts.detail, opts.detail_input()),
                format!("{} direct", direct.total),
            )
        };
        RunBy {
            human: format!("run by: {raised}; direct code callers: {callers_human}"),
            summary: format!(
                "run by: the publisher ({}); direct code callers: {callers_summary}",
                rf_plural(publishers.len(), "event", "events")
            ),
            json: json!({"kind": "event-subscriber", "publishers": publishers,
                         "directCodeCallers": direct.total}),
            action: false,
            keep_callers: false,
        }
    }

    /// A table field's `OnValidate` (its `Validate` calls), `OnLookup` or
    /// `OnAssistEdit` (no code call runs them), with the pages on the
    /// field's table as potential UI surfaces. `None` when the field is not
    /// found in the model's tables.
    /// The table a table or table-extension routine `r` belongs to (the
    /// table's id is `<app>/table/<number>`, not the object id).
    fn trigger_table<'m>(ix: &Indexes<'m>, r: &ModelRoutine) -> Option<&'m ModelTable> {
        let model = ix.model;
        match r.object_type.as_str() {
            "Table" => model.workspace.tables.iter().find(|t| {
                !t.is_extension_stub
                    && t.app_guid == r.app_guid
                    && t.table_number == r.object_number
            }),
            _ => {
                let tables = table_by_id_preferring_real(&model.workspace.tables);
                let base = ix
                    .data
                    .declaring
                    .of(&model.workspace.objects, r)?
                    .extends_target_name
                    .as_deref()?;
                match resolve_table_selector(model, base) {
                    Resolution::Resolved(t) => tables.get(t.id.as_str()).copied(),
                    _ => None,
                }
            }
        }
    }

    /// Potential UI surfaces of a table: the pages whose source table it is
    /// (the sited graph's rule for a lifecycle site from a page into its
    /// source table's triggers, `impact_graph::project`), sorted.
    fn pages_on_table(model: &Model, table: &ModelTable) -> Vec<String> {
        use crate::engine::l4::field_touches::field_key;
        let table_key = field_key(&table.name);
        let mut pages: Vec<String> = model
            .workspace
            .objects
            .iter()
            .filter(|o| {
                o.object_type == "Page"
                    && o.source_table_name
                        .as_deref()
                        .is_some_and(|t| field_key(t) == table_key)
            })
            .map(|o| crate::engine::l5::routine_rollup::format_object("Page", &o.name, None))
            .collect();
        pages.sort();
        pages.dedup();
        pages
    }

    /// A table's record trigger (`OnInsert`, `OnModify`, `OnDelete`,
    /// `OnRename`; round 2 G4's W3 row): the record operations whose
    /// implicit-trigger edge reaches it (the resolver's dispatch rule: a
    /// `RunTrigger` that is not literally false for Insert/Modify/Delete,
    /// Rename by its own rule; the sited graph's `TriggerOf` sites are
    /// projected from these edges), and the pages on the table as potential
    /// UI surfaces.
    fn table_trigger(
        ix: &Indexes<'_>,
        rows: &[usize],
        r: &ModelRoutine,
        opts: RenderOpts,
    ) -> Option<RunBy> {
        use crate::program::model::taxonomy::DispatchKind;
        let model = ix.model;
        let table = Self::trigger_table(ix, r)?;
        let ids: HashSet<&str> = rows.iter().map(|&i| ix.row(i).id.as_str()).collect();
        let mut sites: Vec<(String, u32, String, String)> = Vec::new();
        for e in model.calls.edges.iter().filter(|e| {
            e.dispatch_kind == DispatchKind::ImplicitTrigger
                && e.to.as_deref().is_some_and(|t| ids.contains(t))
        }) {
            let from = e.from_ix.get();
            let Some(op) = ix
                .row(from)
                .record_operations
                .iter()
                .find(|o| o.id == e.operation_id)
            else {
                continue;
            };
            sites.push((
                op.source_anchor.source_unit_id.clone(),
                op.source_anchor.start_line + 1,
                op.op.clone(),
                ix.qualified(from),
            ));
        }
        sites.sort_by(|a, b| (&a.0, a.1, &a.2).cmp(&(&b.0, b.1, &b.2)));
        sites.dedup();
        let pages = Self::pages_on_table(model, table);
        let table_display = format!("\"{}\"", table.name.replace('"', "\"\""));
        let surfaces = if pages.is_empty() {
            format!("potential UI surfaces: no page on table {table_display} in the loaded program")
        } else {
            format!(
                "potential UI surfaces: {} on table {table_display} ({})",
                rf_plural(pages.len(), "page", "pages"),
                pages.join(", ")
            )
        };
        let ops = if sites.is_empty() {
            crate::engine::vocab::none_observed(
                "every record operation of the loaded program on this table",
                "operations on a RecordRef or a record whose table is not resolved",
            )
        } else {
            let cap = if opts.detail {
                usize::MAX
            } else {
                RUN_BY_SITES
            };
            let shown: Vec<String> = sites
                .iter()
                .take(cap)
                .map(|(f, l, op, who)| format!("{op} at {f}:{l} in {who}"))
                .collect();
            format!(
                "{} ({}{})",
                rf_plural(sites.len(), "site", "sites"),
                shown.join("; "),
                if sites.len() > shown.len() {
                    format!("; … +{}", sites.len() - shown.len())
                } else {
                    String::new()
                }
            )
        };
        let ops_short = if sites.is_empty() {
            "none observed".to_string()
        } else {
            rf_plural(sites.len(), "site", "sites")
        };
        let json_sites: Vec<Value> = sites
            .iter()
            .map(|(f, l, op, who)| json!({"op": op, "routine": who, "file": f, "line": l}))
            .collect();
        Some(RunBy {
            human: format!("run by: record operations that run it: {ops}; {surfaces}"),
            summary: format!(
                "run by: record operations: {ops_short}; potential UI surfaces: {}",
                rf_plural(pages.len(), "page", "pages")
            ),
            json: json!({"kind": "table-trigger", "table": table.name,
                         "recordOperations": json_sites, "potentialUiSurfaces": pages}),
            action: false,
            keep_callers: true,
        })
    }

    /// A page trigger (round 2 G4's W3 row): one a run of the page runs
    /// (`run_reaches_trigger`: `OnOpenPage`, `OnInit`, …) is run by the code
    /// runs of the page and the page actions whose `RunObject` opens it; any
    /// other page trigger runs while a user works in the open page.
    fn page_trigger(
        ix: &Indexes<'_>,
        rows: &[usize],
        first: usize,
        r: &ModelRoutine,
        opts: RenderOpts,
    ) -> RunBy {
        use crate::program::model::taxonomy::DispatchKind;
        let model = ix.model;
        let ids: HashSet<&str> = rows.iter().map(|&i| ix.row(i).id.as_str()).collect();
        let runs: BTreeSet<String> = model
            .calls
            .edges
            .iter()
            .filter(|e| {
                e.dispatch_kind == DispatchKind::PageRun
                    && e.to.as_deref().is_some_and(|t| ids.contains(t))
            })
            .map(|e| ix.qualified(e.from_ix.get()))
            .collect();
        let actions: Vec<String> = ix
            .run_by_trigger(first)
            .into_iter()
            .map(|(oi, a, _)| {
                let page = &model.workspace.objects[oi];
                let caption = one_text(&a.caption).ok().flatten();
                action_display(&page.object_type, &page.name, &a.name, caption.as_deref())
            })
            .collect();
        let object = crate::engine::l5::routine_rollup::format_object(
            &r.object_type,
            ix.object_of(first).1,
            None,
        );
        let reaches = !actions.is_empty()
            || !runs.is_empty()
            || ix.row(first).kind == "trigger" && {
                let kind = if r.object_type == "PageExtension" {
                    al_syntax::ir::ObjectKind::PageExtension
                } else {
                    al_syntax::ir::ObjectKind::Page
                };
                crate::program::impact_graph::reverse::run_reaches_trigger(
                    kind,
                    &r.name.to_ascii_lowercase(),
                    r.enclosing_member.is_some(),
                )
            };
        if !reaches {
            let human = format!(
                "run by: the page's lifecycle while a user works in {object} (a run of the page \
                 does not run this trigger by itself; no code call runs it)"
            );
            return RunBy {
                summary: "run by: a user working in the open page".to_string(),
                human,
                json: json!({"kind": "page-trigger", "runByRun": false}),
                action: false,
                keep_callers: true,
            };
        }
        let list = |v: &[String]| -> String {
            let cap = if opts.detail {
                usize::MAX
            } else {
                RUN_BY_SITES
            };
            let shown: Vec<&str> = v.iter().take(cap).map(String::as_str).collect();
            format!(
                "{}{}",
                shown.join(", "),
                if v.len() > shown.len() {
                    format!(", … +{}", v.len() - shown.len())
                } else {
                    String::new()
                }
            )
        };
        let runs: Vec<String> = runs.into_iter().collect();
        let runs_text = if runs.is_empty() {
            crate::engine::vocab::none_observed(
                "every resolved Page.Run and RunModal of the loaded program",
                "runs of a page held in a variable",
            )
        } else {
            format!(
                "{} ({})",
                rf_plural(runs.len(), "routine", "routines"),
                list(&runs)
            )
        };
        let actions_text = if actions.is_empty() {
            "none observed".to_string()
        } else {
            format!(
                "{} ({})",
                rf_plural(actions.len(), "page action", "page actions"),
                list(&actions)
            )
        };
        RunBy {
            human: format!(
                "run by: code runs of the page: {runs_text}; page actions that open it: \
                 {actions_text}"
            ),
            summary: format!(
                "run by: code runs of the page: {}; page actions that open it: {}",
                if runs.is_empty() {
                    "none observed".to_string()
                } else {
                    rf_plural(runs.len(), "routine", "routines")
                },
                rf_plural(actions.len(), "action", "actions")
            ),
            json: json!({"kind": "page-trigger", "runByRun": true, "codeRuns": runs,
                         "pageActions": actions}),
            action: false,
            keep_callers: true,
        }
    }

    fn field(ix: &Indexes<'_>, r: &ModelRoutine, name: &str, opts: RenderOpts) -> Option<RunBy> {
        use crate::engine::l4::field_touches::{field_key, field_touches};
        use crate::program::body::record_op::FieldUse;
        let model = ix.model;
        let table = Self::trigger_table(ix, r)?;
        let member = field_key(r.enclosing_member.as_deref()?);
        let field = table.fields.iter().find(|f| field_key(&f.name) == member)?;
        // Potential UI surfaces: the pages whose source table is the field's
        // table. Page columns are not indexed, so whether a page shows the
        // field is not known.
        let pages = Self::pages_on_table(model, table);
        let table_display = format!("\"{}\"", table.name.replace('"', "\"\""));
        let what_page = if name == "onvalidate" {
            "shows or edits"
        } else {
            "shows"
        };
        let surfaces = if pages.is_empty() {
            format!("potential UI surfaces: no page on table {table_display} in the loaded program")
        } else {
            format!(
                "potential UI surfaces: {} on table {table_display} ({}; whether a page \
                 {what_page} this field is not established)",
                rf_plural(pages.len(), "page", "pages"),
                pages.join(", ")
            )
        };
        let surfaces_short = format!(
            "potential UI surfaces: {}",
            rf_plural(pages.len(), "page", "pages")
        );
        if name != "onvalidate" {
            let step = if name == "onlookup" {
                "lookup"
            } else {
                "assist-edit"
            };
            return Some(RunBy {
                human: format!(
                    "run by: no code call (AL code cannot run a field's {}; a user's {step} \
                     does); {surfaces}",
                    r.name
                ),
                summary: format!("run by: no code call (a user step); {surfaces_short}"),
                json: json!({"kind": if name == "onlookup" { "field-lookup" } else { "field-assist-edit" },
                             "table": table.name, "field": field.name,
                             "potentialUiSurfaces": pages}),
                action: false,
                keep_callers: false,
            });
        }
        let touches = field_touches(model, table, field, &|_| true);
        let validates: Vec<_> = touches
            .sites
            .iter()
            .filter(|s| s.uses.contains(&FieldUse::Validate))
            .collect();
        let matched: Vec<_> = validates.iter().filter(|s| s.candidate.is_none()).collect();
        let candidates = validates.len() - matched.len();
        let cov = &touches.coverage;
        let (searched, _) =
            crate::engine::l4::field_touches::search_words(cov, cov.dependency_bodies > 0);
        let mut not = vec!["Validate through a FieldRef (dynamic field numbers)".to_string()];
        if cov.dependency_bodies > 0 {
            not.push("dependency code the workspace does not call (not loaded)".to_string());
        }
        if cov.dependency_without_body > 0 {
            not.push(format!(
                "{} known by symbols only",
                rf_plural(
                    cov.dependency_without_body,
                    "dependency routine",
                    "dependency routines"
                )
            ));
        }
        let candidate_clause = if candidates > 0 {
            format!(
                "; {} on a record whose table is not resolved",
                rf_plural(candidates, "candidate", "candidates")
            )
        } else {
            String::new()
        };
        let sites: Vec<Value> = matched
            .iter()
            .map(|s| {
                json!({"routine": ix.qualified(s.routine_ix),
                       "file": s.anchor.source_unit_id,
                       "line": s.anchor.start_line + 1})
            })
            .collect();
        let calls = if matched.is_empty() {
            format!(
                "{}{candidate_clause}",
                crate::engine::vocab::none_observed(&searched, &not.join("; "))
            )
        } else {
            let cap = if opts.detail {
                usize::MAX
            } else {
                RUN_BY_SITES
            };
            let shown: Vec<String> = matched
                .iter()
                .take(cap)
                .map(|s| {
                    format!(
                        "{} at {}:{}",
                        ix.qualified(s.routine_ix),
                        s.anchor.source_unit_id,
                        s.anchor.start_line + 1
                    )
                })
                .collect();
            format!(
                "{} ({}{}){candidate_clause}",
                rf_plural(matched.len(), "site", "sites"),
                shown.join("; "),
                if matched.len() > shown.len() {
                    format!("; … +{}", matched.len() - shown.len())
                } else {
                    String::new()
                }
            )
        };
        let calls_short = if matched.is_empty() {
            "none observed".to_string()
        } else {
            rf_plural(matched.len(), "site", "sites")
        };
        Some(RunBy {
            human: format!(
                "run by: Validate calls on the field: {calls}; {surfaces}; an assignment to the \
                 field does not run it"
            ),
            summary: format!("run by: Validate calls: {calls_short}; {surfaces_short}"),
            json: json!({"kind": "field-validate", "table": table.name, "field": field.name,
                         "validateSites": sites, "validateCandidates": candidates,
                         "searched": searched, "notSearched": not,
                         "potentialUiSurfaces": pages}),
            action: false,
            keep_callers: false,
        })
    }
}

impl Card {
    fn build(
        ix: &Indexes<'_>,
        cross: &CrossRules<'_>,
        rid: &str,
        full: Option<&DigestEntryFull>,
        rcx: &RollupCx<'_>,
        card_tests: CardTests<'_>,
        opts: RenderOpts,
    ) -> Card {
        let CardTests {
            reaching: tests_reaching,
            scope: test_scope,
        } = card_tests;
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
        let mut routine = json!({
            "id": rid,
            "name": name,
            "objectType": object_type,
            "object": object_name,
            "objectCaption": caption.ok().flatten(),
            "doc": doc,
            "anchor": anchor.as_ref().map(anchor_json),
        });
        // V3 §2.1 (additive): the one display, `signature`, `member`,
        // `selector` and the JSON-only `handle`.
        for (k, v) in ix.names.identity_json(rid) {
            routine[k] = v;
        }
        let handle = rcx.handle(rid);
        if !handle.is_empty() {
            routine["handle"] = json!(handle);
        }
        let title = match first {
            Some(i) => format_routine(&ix.names.parts_of(ix.row(i)), true),
            None => owner_qualified(&object_display(object_type, object_name), &name),
        };

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
                // Output validation §9.1 (F67): "skipped by" read like a
                // bypass and "not checked" like a proof of absence; each
                // side says what the analysis saw, with its count.
                // §7.7 (F47): more than a few callers without the check
                // print as counts by object kind, `detail` lists them.
                let skipped = if c.skipped_by.is_empty() {
                    String::new()
                } else if c.skipped_by.len() > CALLER_RULE_NAMES && !opts.detail {
                    format!(
                        "; no equivalent check recognized in {}: {} — {} lists them",
                        rf_plural(c.skipped_by.len(), "analyzed caller", "analyzed callers"),
                        by_object_kind(&names(&c.skipped_by)),
                        opts.detail_input()
                    )
                } else {
                    format!(
                        "; no equivalent check recognized in {}: {}",
                        rf_plural(c.skipped_by.len(), "analyzed caller", "analyzed callers"),
                        names(&c.skipped_by).join(", ")
                    )
                };
                let applied = if c.applied_by.is_empty() {
                    format!(
                        "checked by no caller; present in {}, but its check may not stop the call",
                        names(&c.present_in).join(", ")
                    )
                } else {
                    format!(
                        "checked by {}: {}",
                        rf_plural(c.applied_by.len(), "caller", "callers"),
                        names(&c.applied_by).join(", ")
                    )
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
            // A page action whose `RunObject` runs this row's object (a
            // codeunit's `OnRun`, a page's `OnOpenPage`, …; `run_by_trigger`).
            for (oi, a, raw) in ix.run_by_trigger(r) {
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
                        "reached from page action {} (RunObject = {raw}) at {}",
                        action_display(&page.object_type, &page.name, &a.name, caption.as_deref()),
                        anchor_human(&a.anchor)
                    ),
                ));
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
                        "reached from page action {} at {}",
                        action_display(ot, on, &a.name, caption.as_deref()),
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
        // One entry per action: an action reaches the routine once, even when
        // several reached rows lead back to it (a page's `OnOpenPage` and its
        // `OnAfterGetRecord`, a page and its extension).
        reached_from.dedup_by(|a, b| a.0["anchor"] == b.0["anchor"]);

        // Tests.
        let mut tests = Vec::new();
        for (test, via_event) in tests_reaching
            .and_then(|(t, _)| t.get(rid))
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
        // Round-2 r2 §2.3 (G3): "callers" is one set on every line of a card,
        // the direct callers (the caller rules' set): the routines and
        // triggers that call it, and the page actions whose trigger calls it
        // directly. A direct page action is also one of the actions that
        // reach it in all (`reached_from`).
        let routine_rows: Vec<usize> = known_callers
            .iter()
            .copied()
            .filter(|&c| !ix.is_action_trigger(c))
            .collect();
        let routine_callers: Vec<String> = routine_rows.iter().map(|&c| ix.qualified(c)).collect();
        let action_callers: Vec<String> = known_callers
            .iter()
            .filter(|&&c| ix.is_action_trigger(c))
            .map(|&c| ix.qualified(c))
            .collect();
        let action_objects = known_callers
            .iter()
            .filter(|&&c| ix.is_action_trigger(c))
            .map(|&c| ix.object_of(c))
            .collect::<BTreeSet<_>>()
            .len();
        let direct = DirectCallers {
            total: known_callers.len(),
            routines: routine_callers.clone(),
            routine_label: routine_label(ix, &routine_rows),
            actions: action_callers,
            action_objects,
            reached: reached_from.len(),
        };
        let run_by = RunBy::of(ix, &rows, rid, &direct, opts);
        totals.callers = Some(routine_callers.len());
        totals.callers_clause = Some(match &run_by {
            Some(rb) => rb.summary.clone(),
            None => direct.summary(),
        });
        totals.page_actions = Some(reached_from.len());
        totals.tests = Some(tests.len());
        totals.tests_loaded = tests_reaching.map(|(_, n)| n);
        // The summary's `tests:` clause reads the same scope as the detail
        // line (agent-tasks C, item 1).
        totals.sibling_tests =
            test_scope.map(|s| crate::engine::l5::routine_rollup::SiblingTests {
                apps: s.apps.len(),
                complete: s.complete,
            });
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
        let mut header = header_facts(&rollup, full, &counted);
        header.summary = totals.human(&crate::engine::l5::routine_rollup::writes_summary(
            &rollup, full,
        ));
        let notes_line = (!notes.is_empty()).then(|| {
            format!(
                "notes: {}",
                notes
                    .iter()
                    .map(|(k, n)| format!("{k} {n}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        });
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
            caller_dependent: caller_dependent_lines(&rollup, full, rcx),
            beyond: beyond_lines(&rollup, full, rcx, opts),
            no_path: no_path_lines(&rollup, full, rcx, opts),
            dead: literal_dead_lines(&rollup, opts),
            temporary: temporary_lines(&rollup, opts),
            events: {
                // Output validation §9.1 (F72): the events the cone raises
                // and the ones the routine publishes itself, on one line.
                let mut lines = event_lines(&rollup, rcx, opts);
                if let Some(l) = lines.first_mut() {
                    if extension_points.is_empty() {
                        l.push_str("; publishes: none of its own");
                    } else {
                        *l = l.replacen(", downstream)", ", its own and downstream)", 1);
                        l.push_str(&format!(
                            "; publishes: {} of its own (extension points below)",
                            extension_points.len()
                        ));
                    }
                }
                lines
            },
            kinds: kind_lines(&rollup, full, opts),
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
            direct,
            run_by,
            writes,
            commits,
            commit_sites,
            events,
            families,
            crossings,
            reached_from,
            extension_points,
            tests,
            test_scope: test_scope.cloned(),
            gaps,
            totals,
            coverage,
            gaps_in_dependencies,
            header,
            notes: notes_line,
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
            // Round-2 r2 §2.3-2.4 (additive): the direct callers as one set;
            // what runs a trigger.
            "callers": self.direct.json(),
            "runBy": self.run_by.as_ref().map(|rb| rb.json.clone()),
            "extensionPoints": self.extension_points.iter().map(|r| r.0.clone()).collect::<Vec<_>>(),
            "tests": self.tests.iter().map(|r| r.0.clone()).collect::<Vec<_>>(),
            "testScope": self.test_scope.as_ref().map(|s| json!({
                "discovery": s.origin.as_str(),
                "complete": s.complete,
                "apps": s.apps.iter().map(|a| a.name.clone()).collect::<Vec<_>>(),
                "skipped": s.skipped.len(),
                "summary": s.sibling_summary(),
                "analyzed": false,
            })),
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

    /// The card's human text as (head, body): the head is the title and the
    /// summary (output validation §5.2: with one card, the summary's first
    /// line is the answer's `answer:` line, so the card keeps the rest; the
    /// analysis, limits and scope are the header's third line, so a card
    /// prints its own only when the answer has several cards and its own
    /// differ). The lead sections (calls, the UI route, the callers' context)
    /// go between the two.
    ///
    /// Round-2 G24: with `callers_first` (the callers' context is shown and
    /// `detail` is off) the card leads with the callers: the head is the
    /// title, the body starts with the callers' line and its rules, then the
    /// summary lines and `callers_first`'s lines (calls, the UI route, the
    /// target company), the tests and a pointer to `detail` for the effect
    /// rows; the page-action captions and objects move behind `detail`.
    /// Round-2 G25: the summary's `callers and tests:` line is not printed
    /// when the body prints the callers (or `run by:`) and tests lines.
    fn human_parts(
        &self,
        single: bool,
        next: &[String],
        callers_first: Option<&[String]>,
    ) -> (String, String) {
        let compact = callers_first.is_some() && !self.opts.detail;
        let actions = self.reached_from.len();
        let body_prints_callers = self.run_by.is_some()
            || !(self.direct.total == 0 && actions == 0 && self.callers.is_empty());
        let rest: Vec<String> = self
            .header
            .rest()
            .into_iter()
            .filter_map(|l| match l.strip_prefix("callers and tests: ") {
                Some(_) if body_prints_callers => None,
                Some(_) => Some(self.direct.summary()),
                None => Some(l.to_string()),
            })
            .collect();
        let mut head = format!(
            "{}{}\n",
            self.title,
            self.anchor
                .as_ref()
                .map(|a| format!("  defined at {}", anchor_human(a)))
                .unwrap_or_default()
        );
        {
            let mut line = |s: &str| {
                head.push_str("  ");
                head.push_str(s);
                head.push('\n');
            };
            // Several cards: each card's own summary line first, then the
            // same lines a single card prints.
            if !single {
                line(&format!("summary — {}", self.header.answer()));
            }
            if !compact {
                for l in &rest {
                    line(l);
                }
            }
            if !single {
                if self.header.analysis != crate::engine::result_header::Analysis::Complete {
                    line(&self.header.analysis.human());
                }
                if !self.header.limits.is_empty() {
                    line(&format!("limits: {}", self.header.limits.join("; ")));
                }
            }
            if let Some(n) = &self.notes {
                line(n);
            }
        }
        let mut out = String::new();
        let mut line = |s: &str| {
            out.push_str("  ");
            out.push_str(s);
            out.push('\n');
        };
        let detail = self.opts.detail;
        if let Some(d) = &self.doc {
            line(&format!("doc: {}", d.replace('\n', " / ")));
        }
        // 7: rules, own and inherited (§5.3: an empty rules section is not
        // printed).
        if self.own.is_empty() && self.inherited.is_empty() {
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
        // 8: callers with their caller rules, then the page actions. Round-2
        // r2 §2.3-2.4: the direct callers as one set; a trigger says what
        // runs it (`run by:`) instead of `callers: none`.
        match &self.run_by {
            // Round 2 G4 (W3): a table or page trigger keeps its direct
            // callers' line (the routines whose operation or run reaches it).
            Some(rb) if rb.keep_callers && self.direct.total > 0 => {
                line(&rb.human);
                line(&format!(
                    "callers: {}",
                    self.direct.human(detail, self.opts.detail_input())
                ));
            }
            Some(rb) => line(&rb.human),
            // §5.3: `callers: none` with no page action is the summary's own
            // clause.
            None if self.direct.total == 0 && actions == 0 && self.callers.is_empty() => {}
            None => line(&format!(
                "callers: {}",
                self.direct.human(detail, self.opts.detail_input())
            )),
        }
        for (_, h) in &self.callers {
            line(&format!("  {h}"));
        }
        // A page action's own card: its only `reached from` is the action
        // itself, which `entry point: this action` says (G26).
        let actions = if self.run_by.as_ref().is_some_and(|rb| rb.action) || compact {
            0
        } else {
            actions
        };
        if let Some(after) = callers_first.filter(|_| compact) {
            // Round-2 G24: the effects as summary lines only.
            for l in &rest {
                line(l);
            }
            for l in after {
                line(l);
            }
            for l in self.tests_lines() {
                line(&l);
            }
            line(&format!(
                "effect rows (writes, commits, events, other kinds, gaps): {} lists them",
                self.opts.detail_input()
            ));
            for n in next {
                line(&format!("next: {n}"));
            }
            out.push('\n');
            return (head, out);
        }
        if actions == 0 {
        } else if actions <= 3 || detail {
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
                "  most actions ({}): {} ({} lists every action)",
                rf_plural(objects.len(), "page object", "page objects"),
                ties_folded(&objects, caps::PAGE_OBJECTS),
                self.opts.detail_input()
            ));
        }
        // 9-16: the shared effect sections. A run of hops several of their
        // paths share prints once, under a segment id, listed after the
        // last of them (output validation §7.5).
        let s = &self.sections;
        let effect_lines: Vec<String> = [
            &s.commits,
            &s.workspace,
            &s.caller_dependent,
            &s.beyond,
            &s.no_path,
            &s.dead,
            &s.temporary,
            &s.events,
        ]
        .into_iter()
        .flatten()
        .cloned()
        .collect();
        for l in &crate::engine::path_segments::segment(effect_lines) {
            line(l);
        }
        // 17: extension points (never a group); none is not printed (§5.3).
        for (_, h) in &self.extension_points {
            line(h);
        }
        // 18: kind families.
        for l in &s.kinds {
            line(l);
        }
        // 19: tests.
        for l in self.tests_lines() {
            line(&l);
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
        // 22: next (§6.2): only the follow-ups whose answer is not on the
        // card already.
        for n in next {
            line(&format!("next: {n}"));
        }
        out.push('\n');
        (head, out)
    }

    /// The card's tests lines (section 19).
    fn tests_lines(&self) -> Vec<String> {
        let mut out = Vec::new();
        // Output step 1b (astra Task 5, fable fact 2): "none reach it" read
        // the same with and without test code loaded.
        // Agent-tasks C-7: the sibling test apps are discovered, never
        // analyzed here (no test universe is built for explain).
        let siblings = self.test_scope.as_ref().filter(|s| !s.apps.is_empty());
        // Round 2 G60: the complete tests call for this routine.
        let selector = self.routine["selector"]
            .as_str()
            .or_else(|| self.routine["id"].as_str())
            .unwrap_or_default()
            .to_string();
        let tests_call = crate::engine::follow_up::FollowUp::new(
            crate::engine::follow_up::Tool::Tests,
            crate::engine::follow_up::Cx::new(self.opts.surface, false),
            "selects the tests",
        )
        .arg("routines", vec![selector]);
        let not_here = format!(
            "not analyzed in this call — {} selects them",
            tests_call.text(self.opts.surface)
        );
        if self.tests.is_empty() {
            out.push(match (self.totals.tests_loaded, siblings) {
                (None, _) => "tests: not searched in this answer".to_string(),
                (Some(0), Some(s)) => format!(
                    "tests: no [Test] procedure in the workspace app; {}; {not_here}",
                    s.sibling_summary()
                ),
                (Some(0), None) => format!(
                    "tests: no test code found (0 [Test] procedures in the workspace app; {}); \
                     that is not a reason to skip tests",
                    self.test_scope.as_ref().map_or_else(
                        || "sibling apps not searched".into(),
                        |s| s.sibling_summary()
                    )
                ),
                (Some(n), _) => format!(
                    "tests: {} loaded; none reaches it (not a reason to skip tests){}",
                    rf_plural(n, "[Test] procedure", "[Test] procedures"),
                    siblings
                        .map(|s| format!("; also {}, {not_here}", s.sibling_summary()))
                        .unwrap_or_default()
                ),
            });
        }
        for (_, h) in &self.tests {
            out.push(h.clone());
        }
        if let (false, Some(s)) = (self.tests.is_empty(), siblings) {
            out.push(format!("  also {}, {not_here}", s.sibling_summary()));
        }
        out
    }

    /// The follow-ups this card does not answer itself (§6.2): `writes-table`
    /// for a write the card shows without a witness, or when its writes are
    /// `none observed` (a partial analysis); `may-commit` when its commits
    /// are `yes` without a witness or `none observed`, never when they are
    /// `no (every candidate decided)`; the callers' context when the card has
    /// callers and does not show it.
    fn follow_ups(
        &self,
        cx: crate::engine::follow_up::Cx,
        selector: &str,
        callers_shown: bool,
        no_path_table: Option<&str>,
    ) -> Vec<crate::engine::follow_up::FollowUp> {
        use crate::engine::follow_up::{FollowUp, Tool};
        let mut out = Vec::new();
        let complete = self.header.analysis == crate::engine::result_header::Analysis::Complete;
        let t = &self.totals;
        if let Some(table) = no_path_table {
            out.push(FollowUp::prove(
                cx,
                selector,
                &format!("writes-table:{table}"),
                "the witness of a write this card lists without a path",
            ));
        } else if !complete && t.may_write_tables == 0 && t.unresolved_table_writes == 0 {
            out.push(FollowUp::prove(
                cx,
                selector,
                "writes-table:<table>",
                "whether it writes one table, with the blockers of an unknown",
            ));
        }
        let commits_unwitnessed = t.commits == "no observed"
            || (t.commits != "none" && t.commit_sites == 0 && t.commit_entry_points > 0);
        if commits_unwitnessed {
            out.push(FollowUp::prove(
                cx,
                selector,
                "may-commit",
                "the witness of a commit, or the blockers of an unknown",
            ));
        }
        // A page action's own card has no caller: its `reached from` is the
        // action itself (round-2 r2 §2.4, c05).
        let own_action = self.run_by.as_ref().is_some_and(|rb| rb.action);
        let has_callers =
            !self.known_callers.is_empty() || (!self.reached_from.is_empty() && !own_action);
        if has_callers && !callers_shown {
            out.push(
                FollowUp::new(
                    Tool::Explain,
                    cx,
                    "its callers' pending writes, intercepted errors and non-interactive entries",
                )
                .arg("routines", vec![selector.to_string()])
                .arg("callersContext", true),
            );
        }
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

//! The production parts of an impact answer as jobs (agent-tasks design
//! §8.4):
//!
//! - [`MainJob`] (`alsem impact --part main`), off the production MODEL: the
//!   changed roots (digest's `resolve_changed_roots`: digest's selector
//!   grammar and diff mapping), the line accounting ([`super::accounting`]),
//!   each root's CURRENT effect totals (digest's own plan and query, then
//!   the per-routine rollup every effects renderer reads, `RollupTotals`),
//!   and what the tests parts need (the production stamp, the workspace's own
//!   `[Test]` count, the test scope), as `alsem tests --part roots` gives it.
//! - [`UpJob`] (`alsem impact --part up`), off the production SITED graph
//!   only: who reaches each root ([`super::up`]).
//!
//! Each prints one JSON document; [`super::run`] runs them, then the tests
//! parts, and renders the answer.

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::{Value, json};

use crate::engine::batch::job::{
    Doc, HandOff, Job, JobFailure, PlanPart, ResolveCx, SideDoc, SideIx, ViewCx, ViewDemand,
    ViewFailure, ViewNeeds, WorkspaceRead,
};
use crate::engine::batch::output::JobOutput;
use crate::engine::l4::effect_query_cli::OwnerIndex;
use crate::engine::l5::diff_parser::parse_unified_diff;
use crate::engine::l5::digest_cli::{
    ChangedInput, ChangedRootsDiagnostic, ChangedRootsResult, DbTempStates, DiffRoot,
    changed_roots_diagnostic_human, finish_default_run, primary_guid_of, resolve_changed_roots,
    run_digest_query_full_from_entries,
};
use crate::engine::l5::routine_rollup::{RollupCx, RoutineRollup};
use crate::engine::session::Cell;
use crate::engine::test_select::jobs::scope_json;
use crate::engine::test_select::scope::ScopeRequest;
use crate::engine::test_select::universe::production_source_stamp;

use super::accounting::{Accounting, account};

/// The main part's schema.
pub const MAIN_SCHEMA: &str = "al-sem.impact-main";

/// What the main part resolves.
pub enum MainAsk {
    Routines(Vec<String>),
    Files(Vec<String>),
    Diff { text: String, root: DiffRoot },
}

/// The main part (see the module doc).
pub struct MainJob<'a> {
    pub inputs: &'a HandOff,
    pub single_app: bool,
    pub ask: MainAsk,
    pub scope: ScopeRequest,
    /// `--summaries` (agent-tasks B2): also print each root's semantic
    /// summary ([`super::keys`]), the contract snapshot (the complete routine
    /// inventory and the schema), the package files and dependency
    /// declarations this side read. Both sides of a before/after answer.
    pub summaries: bool,
    /// `--root-id` with `--summaries`: routines summarized as well when this
    /// side has them (the base side is asked the working tree's roots).
    pub extra_roots: Vec<String>,
}

/// What `resolve` hands `compute`: the roots and the document so far.
struct Carry {
    changed: ChangedRootsResult,
    doc: Value,
}

/// A diagnostic's structured form (its kind and where).
fn diagnostic_json(d: &ChangedRootsDiagnostic) -> Value {
    use ChangedRootsDiagnostic as D;
    let (kind, file, range) = match d {
        D::FileUnmatched { file } => ("file-unmatched", Some(file), None),
        D::FileAmbiguous { file, .. } => ("file-ambiguous", Some(file), None),
        D::SelectorUnmatched { .. } => ("selector-unmatched", None, None),
        D::SelectorAmbiguous { .. } => ("selector-ambiguous", None, None),
        D::DiffFileUnmatched { file } => ("diff-file-unmatched", Some(file), None),
        D::DiffFileAmbiguous { file, .. } => ("diff-file-ambiguous", Some(file), None),
        D::HunkOutsideRoutines {
            file,
            start_line,
            end_line,
        } => (
            "hunk-outside-routines",
            Some(file),
            Some((*start_line, *end_line)),
        ),
        D::DiffFileDeleted { file } => ("diff-file-deleted", Some(file), None),
        D::DiffFileNotAl { file } => ("diff-file-not-al", Some(file), None),
        D::DiffParseError { file, .. } => ("diff-parse-error", file.as_ref(), None),
    };
    let mut v = json!({"kind": kind, "text": changed_roots_diagnostic_human(d)});
    if let Some(f) = file {
        v["file"] = json!(f);
    }
    if let Some((a, b)) = range {
        v["startLine"] = json!(a);
        v["endLine"] = json!(b);
    }
    v
}

/// The accounting as the main part carries it.
fn accounting_json(a: &Accounting) -> Value {
    let files: Vec<Value> = a
        .files
        .iter()
        .map(|f| {
            json!({
                "path": f.path,
                "oldPath": f.old_path,
                "kind": f.kind,
                "workspaceFile": f.workspace_file,
                "added": f.added,
                "removed": f.removed,
                "buckets": f.buckets.to_json(),
                "unmapped": f.unmapped.map(|u| u.as_str()),
                "removedRoutineNames": f.removed_routines,
                "mismatch": f.mismatch.as_ref().map(|m| json!({"firstLine": m.line, "lines": m.lines})),
                "hunks": f.hunks.iter().map(|h| json!({
                    "startLine": h.start, "endLine": h.end,
                    "buckets": h.buckets.to_json(),
                    "routines": h.routines,
                })).collect::<Vec<_>>(),
            })
        })
        .collect();
    json!({
        "changedLines": a.changed_lines(),
        "totals": a.totals.to_json(),
        "files": files,
        "onlyIn": a.only_in.iter().map(|(p, w)| json!({"path": p, "workspaceFile": w})).collect::<Vec<_>>(),
        "errors": a.errors,
    })
}

impl Job for MainJob<'_> {
    fn name(&self) -> &'static str {
        "impact"
    }

    fn demands(&self) -> Vec<ViewDemand> {
        ViewDemand::one(self.inputs, self.single_app, false, ViewNeeds::MODEL)
    }

    fn resolve(&self, _: SideIx, cx: &ResolveCx<'_>) -> Result<PlanPart, JobFailure> {
        let model = cx.model();
        let inputs = cx.inputs();
        let owners = OwnerIndex::build(model);
        let (files, routines, diff_text, diff_root) = match &self.ask {
            MainAsk::Routines(r) => (None, Some(r.clone()), None, DiffRoot::default()),
            MainAsk::Files(f) => (Some(f.clone()), None, None, DiffRoot::default()),
            MainAsk::Diff { text, root } => (None, None, Some(text.clone()), root.clone()),
        };
        let mut changed = resolve_changed_roots(
            cx.ids(),
            model,
            &ChangedInput {
                files,
                routines,
                diff_text: diff_text.clone(),
                diff_root: diff_root.clone(),
                workspace_files: &inputs.al_files,
            },
            primary_guid_of(model),
            &owners,
        );
        if self.summaries {
            let have: std::collections::HashSet<&str> = model
                .workspace
                .routines
                .iter()
                .filter(|r| model.routine_role(r) == crate::program::model::role::Role::Primary)
                .map(|r| r.stable_routine_id.as_str())
                .collect();
            for id in &self.extra_roots {
                if have.contains(id.as_str()) && !changed.roots.contains(id) {
                    changed.roots.push(id.clone());
                }
            }
        }
        // Every changed line, when the change is a diff.
        let accounting = diff_text.as_deref().map(|t| {
            account(
                &parse_unified_diff(t),
                &diff_root,
                &inputs.al_files,
                &changed.spans,
            )
        });
        let ws = &model.workspace;
        let object_name: HashMap<&str, &str> = ws
            .objects
            .iter()
            .map(|o| (o.id.as_str(), o.name.as_str()))
            .collect();
        let rows: HashMap<&str, &crate::program::model::workspace::ModelRoutine> = ws
            .routines
            .iter()
            .map(|r| (r.stable_routine_id.as_str(), r))
            .collect();
        // Each root's anchor: its last touched declaration (digest's rule),
        // else its first.
        let mut first_span: HashMap<&str, usize> = HashMap::new();
        for (i, s) in changed.spans.spans.iter().enumerate() {
            first_span.entry(s.routine.as_str()).or_insert(i);
        }
        let mismatched: std::collections::HashSet<String> = accounting
            .iter()
            .flat_map(|a| a.mismatched())
            .filter_map(|f| f.workspace_file.as_ref().map(|w| format!("ws:{w}")))
            .collect();
        let roots: Vec<Value> = changed
            .roots
            .iter()
            .map(|id| {
                let r = rows.get(id.as_str());
                let span = changed
                    .touched
                    .get(id)
                    .and_then(|t| t.iter().next_back().copied())
                    .or_else(|| first_span.get(id.as_str()).copied())
                    .map(|i| &changed.spans.spans[i]);
                let lines = accounting
                    .as_ref()
                    .and_then(|a| a.routines.get(id).copied())
                    .unwrap_or_default();
                let uncertain = span.is_some_and(|s| mismatched.contains(&s.source_unit_id));
                let status = if uncertain {
                    "positionUncertain"
                } else if lines.signature > 0 {
                    "signatureChanged"
                } else {
                    "changed"
                };
                json!({
                    "stableId": id,
                    "display": r.map(|r| format!("{} {}::{}", r.object_type,
                        object_name.get(r.object_id.as_str()).copied().unwrap_or(""), r.name)),
                    "anchor": span.map(|s| json!({"file": s.source_unit_id, "line": s.anchor_line + 1})),
                    "status": status,
                    "changedLines": accounting.as_ref().map(|_| json!({"body": lines.body, "signature": lines.signature})),
                })
            })
            .collect();
        let primary = primary_guid_of(model).map(str::to_string);
        let workspace_tests = ws
            .routines
            .iter()
            .filter(|r| {
                model.routine_role(r) == crate::program::model::role::Role::Primary
                    && crate::program::attributes::find_attribute(&r.attributes_parsed, "Test")
                        .is_some()
            })
            .count();
        let scope = crate::engine::test_select::scope::discover(&inputs.workspace, &self.scope);
        let app = inputs
            .app_json_value
            .as_ref()
            .and_then(|v| v.as_ref().ok())
            .cloned()
            .unwrap_or(Value::Null);
        let doc = json!({
            "schema": MAIN_SCHEMA,
            "version": 1,
            "workspace": inputs.workspace.to_string_lossy(),
            "appId": primary,
            "appName": app.get("name"),
            "productionSourceStamp": primary.as_deref().and_then(|p| production_source_stamp(inputs, p)),
            "workspaceTests": workspace_tests,
            "roots": roots,
            "diagnostics": changed.diagnostics.iter().map(changed_roots_diagnostic_human).collect::<Vec<_>>(),
            "diagnosticsDetail": changed.diagnostics.iter().map(diagnostic_json).collect::<Vec<_>>(),
            "accounting": accounting.as_ref().map(accounting_json),
            "scope": scope_json(&scope),
        });
        let mut doc = doc;
        if self.summaries {
            let universe = if self.single_app {
                "single-app"
            } else {
                "cross-app"
            };
            doc["contract"] = super::delta::cbor_json(
                &crate::engine::l5::snapshot_full::contract_snapshot(model, universe),
            );
            doc["packages"] = json!(
                inputs
                    .closure
                    .files
                    .iter()
                    .map(|f| f.path.to_string_lossy().into_owned())
                    .collect::<Vec<_>>()
            );
            doc["appDependencies"] = app.get("dependencies").cloned().unwrap_or(json!([]));
            doc["sourceDependencies"] = match &inputs.source_dependencies {
                Ok(d) => json!(
                    d.iter()
                        .map(
                            |s| json!({"spelled": s.spelled, "root": s.root.to_string_lossy(),
                                        "contentHash": s.content_hash})
                        )
                        .collect::<Vec<_>>()
                ),
                Err(e) => json!({"error": e}),
            };
        }
        // Digest's plan over the roots: their raw facts, the tree and its
        // effects (no root: nothing to compose).
        let plan = crate::engine::session::digest_reads_plan(
            crate::engine::l5::detectors::default_detectors(),
            changed.roots.iter().cloned().collect(),
            false,
            false,
        );
        Ok(PlanPart {
            plan,
            reads: vec![Cell::Det, Cell::L4, Cell::Effects],
            carry: Some(Box::new(Carry { changed, doc })),
        })
    }

    fn compute(&self, _: SideIx, cx: &mut ViewCx<'_>) -> Result<SideDoc, JobFailure> {
        let Carry { changed, mut doc } = cx.take_carry();
        let dependencies = cx.view().target().dependencies();
        let key = cx.keys()[0].clone();
        let (_, tx) = finish_default_run(cx, &key);
        let mut effects = if cx.has_tree() {
            cx.take_effects()
        } else {
            Default::default()
        };
        let resolved = cx.view().model();
        let temp_states = DbTempStates::new(resolved, &cx.view().l4().bundle);
        let owners = OwnerIndex::build(resolved);
        let q = run_digest_query_full_from_entries(
            &mut effects,
            &changed.roots,
            &tx,
            &temp_states,
            &changed,
            &owners,
        );
        drop(effects);
        let rcx = RollupCx::new(resolved, &dependencies.apps);
        // One rollup per root, each entry dropped once its totals are taken.
        let mut totals: HashMap<String, Value> = HashMap::new();
        let mut handles: HashMap<String, String> = HashMap::new();
        let names = self.summaries.then(|| super::keys::Names::new(resolved));
        let mut summaries = serde_json::Map::new();
        for e in q.entries {
            let r = RoutineRollup::build(&e, &rcx);
            handles.insert(e.routine_id.clone(), rcx.handle(&e.routine_id));
            totals.insert(e.routine_id.clone(), r.totals.to_json());
            if let Some(names) = &names
                && let Some(s) =
                    super::keys::summarize(names, &e.routine_id, Some(&e), Some(&r.totals))
            {
                summaries.insert(e.routine_id.clone(), json!(s));
            }
        }
        if let Some(names) = &names {
            // A root the digest computed no entry for still has its own sites.
            for id in &changed.roots {
                if !summaries.contains_key(id)
                    && let Some(s) = super::keys::summarize(names, id, None, None)
                {
                    summaries.insert(id.clone(), json!(s));
                }
            }
            doc["summaries"] = Value::Object(summaries);
            doc["summarySchema"] = json!(super::keys::SUMMARY_SCHEMA);
        }
        if let Some(roots) = doc["roots"].as_array_mut() {
            for r in roots {
                let id = r["stableId"].as_str().unwrap_or("").to_string();
                r["effects"] = totals.remove(&id).unwrap_or(Value::Null);
                r["handle"] = json!(handles.remove(&id));
            }
        }
        doc["queryDiagnostics"] = json!(q.query_diagnostics.len());
        Ok(SideDoc::new(Ok::<Value, String>(doc)))
    }

    fn view_failed(&self, _: SideIx, failure: &ViewFailure) -> Result<SideDoc, JobFailure> {
        Ok(SideDoc::new(Err::<Value, String>(
            failure.target.message("impact"),
        )))
    }

    fn serialize(&self, doc: Doc) -> JobOutput {
        part_output(doc)
    }
}

fn part_output(doc: Doc) -> JobOutput {
    match doc.one::<Result<Value, String>>() {
        Err(f) => f,
        Ok(Ok(v)) => JobOutput::text(
            None,
            Ok((
                serde_json::to_string(&v).expect("a part serializes") + "\n",
                0,
            )),
        ),
        Ok(Err(e)) => JobOutput::line(&e, 1),
    }
}

/// The up part (see the module doc).
pub struct UpJob<'a> {
    inputs: &'a HandOff,
    roots: Vec<String>,
    assume_no_dependents: bool,
    detail: bool,
    single_app: bool,
    /// The workspace's `.al` texts by relative path, taken when the job
    /// is made (the read is the driver's once handed off): page-action
    /// captions are read from them.
    texts: HashMap<String, Arc<str>>,
}

/// What the up part is asked (`--root-id`, `--no-dependents`, `--detail`,
/// `--single-app`).
pub struct UpAsk {
    pub roots: Vec<String>,
    pub assume_no_dependents: bool,
    pub detail: bool,
    pub single_app: bool,
}

impl<'a> UpJob<'a> {
    #[must_use]
    pub fn new(inputs: &'a HandOff, ask: UpAsk) -> UpJob<'a> {
        let UpAsk {
            roots,
            assume_no_dependents,
            detail,
            single_app,
        } = ask;
        let read = inputs.get();
        let texts = read
            .al_files
            .iter()
            .filter_map(|f| {
                f.text
                    .as_ref()
                    .ok()
                    .map(|t| (f.rel_posix.clone(), Arc::clone(t)))
            })
            .collect();
        UpJob {
            inputs,
            roots,
            assume_no_dependents,
            detail,
            single_app,
            texts,
        }
    }
}

impl Job for UpJob<'_> {
    fn name(&self) -> &'static str {
        "impact-up"
    }

    fn demands(&self) -> Vec<ViewDemand> {
        ViewDemand::one(self.inputs, self.single_app, false, ViewNeeds::SITED)
    }

    fn resolve(&self, _: SideIx, _: &ResolveCx<'_>) -> Result<PlanPart, JobFailure> {
        unreachable!("the up part reads only the sited graph")
    }

    fn compute(&self, _: SideIx, _: &mut ViewCx<'_>) -> Result<SideDoc, JobFailure> {
        unreachable!("the up part reads only the sited graph")
    }

    fn view_failed(&self, _: SideIx, failure: &ViewFailure) -> Result<SideDoc, JobFailure> {
        Ok(SideDoc::new(Err::<Value, String>(
            failure.target.message("impact"),
        )))
    }

    fn sited(
        &self,
        _: SideIx,
        g: Result<&crate::program::impact_graph::SitedGraph, &str>,
    ) -> Result<SideDoc, JobFailure> {
        let g = match g {
            Ok(g) => g,
            Err(e) => {
                return Ok(SideDoc::new(Err::<Value, String>(format!(
                    "impact: the program did not build: {e}"
                ))));
            }
        };
        let mut captions = super::up::Captions::new(self.texts.clone());
        let v = super::up::up(
            g,
            &super::up::UpArgs {
                roots: &self.roots,
                assume_no_dependents: self.assume_no_dependents,
                keep: super::up::Keep::new(self.detail),
                cancel: None,
            },
            &mut captions,
        );
        Ok(SideDoc::new(Ok::<Value, String>(v)))
    }

    fn serialize(&self, doc: Doc) -> JobOutput {
        part_output(doc)
    }
}

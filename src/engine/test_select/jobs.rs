//! The two parts of a tests answer as jobs (agent-tasks design §4.2, §4.6):
//!
//! - [`RootsJob`] (`alsem tests --part roots`): in the PRODUCTION universe,
//!   off its model: the changed roots (digest's `resolve_changed_roots`, so
//!   the selector grammar and the diff mapping are digest's), their
//!   StableRoutineIds, the production stamp, the workspace's own `[Test]`
//!   count and the test scope ([`super::scope`]).
//! - [`SelectJob`] (`alsem tests --part select`): in one TEST universe, off
//!   its sited graph only (a graph-only engine serves it): the selection
//!   ([`super::select`]).
//!
//! Each prints one JSON document. `alsem tests` runs the roots part, then
//! the select part per test app, and renders both ([`super::render`]); the
//! MCP tool runs them as a composite request over the production engine and
//! one test engine per app, and renders them with the same function.

use serde_json::{Value, json};

use crate::engine::batch::job::{
    Doc, HandOff, Job, JobFailure, PlanPart, ResolveCx, SideDoc, SideIx, ViewCx, ViewDemand,
    ViewFailure, ViewNeeds, WorkspaceRead,
};
use crate::engine::batch::output::JobOutput;
use crate::engine::l4::effect_query_cli::OwnerIndex;
use crate::engine::l5::digest_cli::{
    ChangedInput, PathRoot, changed_roots_diagnostic_human, primary_guid_of, resolve_changed_roots,
};

use super::scope::{Scope, ScopeRequest};
use super::universe::production_source_stamp;

/// The roots part's schema.
pub const ROOTS_SCHEMA: &str = "al-sem.tests-roots";

/// What the roots part resolves.
pub enum RootsAsk {
    Routines(Vec<String>),
    Files(Vec<String>),
    /// The diff's text and what its paths are read against (#363: with anchors).
    Diff {
        text: String,
        root: PathRoot,
    },
}

/// The roots part (see the module doc).
pub struct RootsJob<'a> {
    pub inputs: &'a HandOff,
    pub single_app: bool,
    pub ask: RootsAsk,
    pub scope: ScopeRequest,
}

/// What the roots part carries from `resolve` to `compute`.
struct Carry(Value);

impl Job for RootsJob<'_> {
    fn name(&self) -> &'static str {
        "tests"
    }

    fn demands(&self) -> Vec<ViewDemand> {
        ViewDemand::one(self.inputs, self.single_app, false, ViewNeeds::MODEL)
    }

    fn resolve(&self, _: SideIx, cx: &ResolveCx<'_>) -> Result<PlanPart, JobFailure> {
        let model = cx.model();
        let inputs = cx.inputs();
        let owners = OwnerIndex::build(model);
        let (files, routines, diff_text, diff_root) = match &self.ask {
            RootsAsk::Routines(r) => (None, Some(r.clone()), None, PathRoot::default()),
            RootsAsk::Files(f) => (Some(f.clone()), None, None, PathRoot::default()),
            RootsAsk::Diff { text, root } => (None, None, Some(text.clone()), root.clone()),
        };
        let changed = resolve_changed_roots(
            cx.ids(),
            model,
            &ChangedInput {
                files,
                routines,
                diff_text,
                diff_root,
                workspace_files: &inputs.al_files,
            },
            primary_guid_of(model),
            &owners,
        );
        let ws = &model.workspace;
        // V3 §2.1: each root by the one routine display, with its parts.
        let routine_names = crate::engine::l5::routine_rollup::RoutineNames::new(model, &[]);
        let roots: Vec<Value> = changed
            .roots
            .iter()
            .map(|id| {
                let r = ws.routines.iter().find(|r| &r.stable_routine_id == id);
                // Round-2 G41: the one display (AL quoting), not the
                // unquoted `<type> <name>::<routine>`.
                let mut v = json!({
                    "stableId": id,
                    "display": r.map(|_| routine_names.display(id)),
                });
                for (k, x) in routine_names.identity_json(id) {
                    v[k] = x;
                }
                v
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
        let scope = super::scope::discover(&inputs.workspace, &self.scope);
        let app = inputs
            .app_json_value
            .as_ref()
            .and_then(|v| v.as_ref().ok())
            .cloned()
            .unwrap_or(Value::Null);
        let doc = json!({
            "schema": ROOTS_SCHEMA,
            "version": 1,
            "workspace": inputs.workspace.to_string_lossy(),
            "appId": primary,
            "appName": app.get("name"),
            "productionSourceStamp": primary.as_deref().and_then(|p| production_source_stamp(inputs, p)),
            "workspaceTests": workspace_tests,
            "roots": roots,
            "diagnostics": changed.diagnostics.iter().map(changed_roots_diagnostic_human).collect::<Vec<_>>(),
            "scope": scope_json(&scope),
        });
        Ok(PlanPart {
            plan: crate::engine::session::Plan::merge(std::iter::empty()),
            reads: vec![],
            carry: Some(Box::new(Carry(doc))),
        })
    }

    fn compute(&self, _: SideIx, cx: &mut ViewCx<'_>) -> Result<SideDoc, JobFailure> {
        let Carry(doc) = cx.take_carry();
        Ok(SideDoc::new(Ok::<Value, String>(doc)))
    }

    fn view_failed(&self, _: SideIx, failure: &ViewFailure) -> Result<SideDoc, JobFailure> {
        Ok(SideDoc::new(Err::<Value, String>(
            failure.target.message("tests"),
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

/// The scope as the parts and the answer carry it.
#[must_use]
pub fn scope_json(s: &Scope) -> Value {
    json!({
        "origin": s.origin.as_str(),
        "workspaceAppId": s.workspace_app_id,
        "complete": s.complete,
        "incomplete": s.incomplete,
        "searchRoot": s.search_root.as_ref().map(|r| r.0.to_string_lossy().to_string()),
        "searchRootIsGitTop": s.search_root.as_ref().map(|r| r.1),
        "searched": s.searched,
        "apps": s.apps.iter().map(|a| json!({
            "name": a.name, "root": a.root.to_string_lossy(), "appId": a.app_id,
            "files": a.files, "testCodeunits": a.test_codeunits,
            "testAttributeLines": a.test_attribute_lines, "found": a.found.as_str(),
        })).collect::<Vec<_>>(),
        "skipped": s.skipped.iter().map(|k| json!({
            "name": k.name, "root": k.root.to_string_lossy(), "reason": k.reason.as_str(),
            "detail": k.reason.human(),
        })).collect::<Vec<_>>(),
        "summary": s.sibling_summary(),
        // What `.al-sem.json` `testApps` said when this scope was found
        // (milestone C item 3: `alsem_status` compares it with the disk).
        "testConfig": s.test_config,
    })
}

/// The select part (see the module doc).
pub struct SelectJob<'a> {
    inputs: &'a HandOff,
    universe: super::universe::TestUniverse,
    roots: Vec<String>,
    /// From the read, taken when the job is made (the read is the driver's
    /// once handed off): the test app's declared dependencies and the
    /// production stamp of the bytes the universe consumed.
    declared: Vec<(String, String, String)>,
    stamp: Option<String>,
    /// Production routines probed apart (`--probe-id`, impact's nearest
    /// tested caller).
    probes: Vec<String>,
}

impl<'a> SelectJob<'a> {
    /// `--probe-id`: routines whose presence and nearest test the part
    /// reports apart, never adding to the selection.
    #[must_use]
    pub fn with_probes(mut self, probes: Vec<String>) -> SelectJob<'a> {
        self.probes = probes;
        self
    }

    /// The select part over the read `inputs` (made with the universe's
    /// injected inputs) for `roots`.
    #[must_use]
    pub fn new(
        inputs: &'a HandOff,
        universe: super::universe::TestUniverse,
        roots: Vec<String>,
    ) -> SelectJob<'a> {
        let read = inputs.get();
        let declared = declared_dependencies(&read);
        let stamp = production_source_stamp(&read, &universe.production_app_id);
        SelectJob {
            inputs,
            universe,
            roots,
            declared,
            stamp,
            probes: Vec::new(),
        }
    }
}

impl Job for SelectJob<'_> {
    fn name(&self) -> &'static str {
        "tests-select"
    }

    fn demands(&self) -> Vec<ViewDemand> {
        ViewDemand::one(self.inputs, false, false, ViewNeeds::SITED)
    }

    fn resolve(&self, _: SideIx, _: &ResolveCx<'_>) -> Result<PlanPart, JobFailure> {
        unreachable!("the select part reads only the sited graph")
    }

    fn compute(&self, _: SideIx, _: &mut ViewCx<'_>) -> Result<SideDoc, JobFailure> {
        unreachable!("the select part reads only the sited graph")
    }

    fn view_failed(&self, _: SideIx, failure: &ViewFailure) -> Result<SideDoc, JobFailure> {
        Ok(SideDoc::new(Err::<Value, String>(
            failure.target.message("tests"),
        )))
    }

    fn sited(
        &self,
        _: SideIx,
        g: Result<&crate::program::impact_graph::SitedGraph, &str>,
        _: &crate::engine::l5::digest_cli::ReachWorkers,
    ) -> Result<SideDoc, JobFailure> {
        let g = match g {
            Ok(g) => g,
            Err(e) => {
                return Ok(SideDoc::new(Err::<Value, String>(format!(
                    "tests: the test universe of {} did not build: {e}",
                    self.universe.test_root.display()
                ))));
            }
        };
        let key = self.universe.key();
        // Output validation r3 L4: `ALSEM_SELECT_TIMING=1` prints the select
        // phase's own time (the walks, not the universe build) to stderr, so
        // the per-root walk limit is measured on the phase it bounds.
        let started = std::time::Instant::now();
        let v = super::select::select(
            g,
            &super::select::SelectArgs {
                roots: &self.roots,
                universe_key: &key,
                test_app_id: &self.universe.test_app_id,
                production_app_id: &self.universe.production_app_id,
                production_stamp: self.stamp.clone(),
                declared: &self.declared,
                probes: &self.probes,
            },
        );
        if std::env::var_os("ALSEM_SELECT_TIMING").is_some() {
            eprintln!(
                "tests-select: {} roots, select phase {} ms",
                self.roots.len(),
                started.elapsed().as_millis()
            );
        }
        Ok(SideDoc::new(Ok::<Value, String>(v)))
    }

    fn serialize(&self, doc: Doc) -> JobOutput {
        part_output(doc)
    }
}

/// The `(id, name, publisher)` of every dependency the test app declares.
fn declared_dependencies(
    inputs: &crate::program::inputs::WorkspaceInputs,
) -> Vec<(String, String, String)> {
    let s = |v: &Value, k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
    inputs
        .app_json_value
        .as_ref()
        .and_then(|v| v.as_ref().ok())
        .and_then(|v| v.get("dependencies"))
        .and_then(|d| d.as_array())
        .into_iter()
        .flatten()
        .map(|d| {
            let id = if s(d, "id").is_empty() {
                s(d, "appId")
            } else {
                s(d, "id")
            };
            (id, s(d, "name"), s(d, "publisher"))
        })
        .collect()
}

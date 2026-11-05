//! **One tests answer, part by part** (agent-tasks design §4.3, §4.6): the
//! roots part in the production universe, then one select part per test
//! universe (the workspace's own tests, then each sibling test app, one at a
//! time), then the render. The CLI runs each part in this process; the MCP
//! tool runs each on an engine child (a composite request: a part never
//! holds one child while waiting for another). Both go through
//! [`orchestrate`], so they decide, retry and render alike.
//!
//! **Consistency** (§4.3). Each select part reports the production stamp of
//! the bytes its universe consumed; the roots part reports the production
//! universe's. When they differ, the roots part and that select part are
//! asked once more (fresh); if they still differ the disk is moving, and that
//! test app is reported not analyzed: `test universe built from different
//! production bytes; retry`. An answer never mixes stamps.

use serde_json::Value;

use super::render::{Part, RenderOpts, Rendered, render};
use super::universe::TestUniverse;

/// What `alsem tests` / `alsem_tests` asks.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TestsRequest {
    pub workspace: String,
    pub routines: Vec<String>,
    pub files: Vec<String>,
    /// `--diff`: a path or `-`.
    pub diff: Option<String>,
    pub diff_root: Option<String>,
    pub test_apps: Vec<String>,
    pub single_app: bool,
}

impl TestsRequest {
    /// The roots part's command line (without the program name).
    #[must_use]
    pub fn roots_argv(&self) -> Vec<String> {
        let mut a: Vec<String> = vec!["tests".into(), "--part".into(), "roots".into()];
        for r in &self.routines {
            a.push(format!("--routine={r}"));
        }
        for f in &self.files {
            a.push(format!("--file={f}"));
        }
        if let Some(d) = &self.diff {
            a.push(format!("--diff={d}"));
        }
        if let Some(d) = &self.diff_root {
            a.push(format!("--diff-root={d}"));
        }
        for t in &self.test_apps {
            a.push(format!("--test-app={t}"));
        }
        if self.single_app {
            a.push("--single-app".into());
        }
        a.push("--".into());
        a.push(self.workspace.clone());
        a
    }
}

/// A select part's command line over `u` for `roots`.
#[must_use]
pub fn select_argv(u: &TestUniverse, roots: &[String]) -> Vec<String> {
    select_argv_with(u, roots, &[])
}

/// [`select_argv`] with `probes`: production routines whose presence and
/// nearest test the part reports apart (impact's nearest tested caller),
/// never adding to the selection.
#[must_use]
pub fn select_argv_with(u: &TestUniverse, roots: &[String], probes: &[String]) -> Vec<String> {
    let mut a: Vec<String> = vec!["tests".into(), "--part".into(), "select".into()];
    if !u.is_workspace() {
        a.push(format!(
            "--production-root={}",
            u.production_root.to_string_lossy()
        ));
    }
    a.push(format!("--production-app-id={}", u.production_app_id));
    a.push(format!("--test-app-id={}", u.test_app_id));
    a.push(format!("--found={}", u.config));
    for r in roots {
        a.push(format!("--root-id={r}"));
    }
    for p in probes {
        a.push(format!("--probe-id={p}"));
    }
    a.push("--".into());
    a.push(u.test_root.to_string_lossy().to_string());
    a
}

/// Where a part runs.
pub enum Target<'a> {
    /// The production universe (the request's own).
    Production,
    /// A test universe (the workspace's own tests: [`TestUniverse::is_workspace`]).
    Tests(&'a TestUniverse),
}

/// Runs one part: its target and command line, to its JSON or why it was
/// not computed.
pub type RunPart<'r> = dyn FnMut(Target<'_>, Vec<String>) -> Result<Value, String> + 'r;

/// Answer `req`, each part through `run`.
pub fn orchestrate(
    req: &TestsRequest,
    opts: RenderOpts,
    run: &mut RunPart<'_>,
) -> Result<Rendered, String> {
    let mut roots = run(Target::Production, req.roots_argv())?;
    let ids: Vec<String> = roots["roots"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|r| r["stableId"].as_str().map(str::to_string))
        .collect();
    if ids.is_empty() {
        return Ok(render(&roots, &[], opts));
    }
    let us = universes(&roots, &req.workspace);
    let parts = select_parts(&mut roots, &ids, &us, run, &req.roots_argv())?;
    Ok(render(&roots, &parts, opts))
}

/// The test universes a roots part names: the workspace's own tests (when
/// it has any), then each sibling test app of its scope.
#[must_use]
pub fn universes(roots: &Value, workspace: &str) -> Vec<(Value, TestUniverse)> {
    let app_id = roots["appId"].as_str().unwrap_or("").to_string();
    let mut universes: Vec<(Value, TestUniverse)> = Vec::new();
    if roots["workspaceTests"].as_u64().unwrap_or(0) > 0 {
        universes.push((
            serde_json::json!({"name": "workspace", "root": workspace, "found": "workspace"}),
            TestUniverse::workspace(std::path::Path::new(workspace), &app_id),
        ));
    }
    for a in roots["scope"]["apps"]
        .as_array()
        .cloned()
        .unwrap_or_default()
    {
        let u = TestUniverse::new(
            std::path::Path::new(a["root"].as_str().unwrap_or("")),
            a["appId"].as_str().unwrap_or(""),
            std::path::Path::new(workspace),
            &app_id,
            a["found"].as_str().unwrap_or("auto"),
        );
        universes.push((a, u));
    }
    universes
}

/// One select part per universe over `ids`, with the consistency rule
/// (§4.3): a part built from other production bytes than `roots` is asked
/// once more together with the production part (`roots_argv`, whose answer
/// replaces `roots`); if the stamps still differ, that app is not analyzed.
pub fn select_parts(
    roots: &mut Value,
    ids: &[String],
    universes: &[(Value, TestUniverse)],
    run: &mut RunPart<'_>,
    roots_argv: &[String],
) -> Result<Vec<Part>, String> {
    let mut parts: Vec<Part> = Vec::new();
    for (app, u) in universes {
        let mut result = run(Target::Tests(u), select_argv(u, ids));
        let mut retried = false;
        loop {
            match &result {
                Ok(v) if v["productionSourceStamp"] != roots["productionSourceStamp"] => {
                    if retried {
                        result = Err(format!(
                            "test universe built from different production bytes (production {}, test universe {}); retry",
                            roots["productionSourceStamp"], v["productionSourceStamp"]
                        ));
                        break;
                    }
                    retried = true;
                    *roots = run(Target::Production, roots_argv.to_vec())?;
                    result = run(Target::Tests(u), select_argv(u, ids));
                }
                _ => break,
            }
        }
        parts.push(Part {
            app: app.clone(),
            result,
        });
    }
    Ok(parts)
}

#[cfg(test)]
#[path = "run_tests.rs"]
mod tests;

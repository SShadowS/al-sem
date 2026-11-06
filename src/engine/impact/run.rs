//! **One impact answer, part by part** (agent-tasks design §8.4): the
//! comparison tuple first (resolved once by the front: the CLI process, or
//! the MCP front end before it sends anything), then
//!
//! 1. the main part in the production universe (roots, accounting, current
//!    effects, test scope),
//! 2. the up part in the production universe (who reaches each root), off
//!    its sited graph,
//! 3. the tests: one select part per test universe, exactly as `alsem tests`
//!    runs them (`test_select::run::select_parts`, with its production-stamp
//!    retry), and, for a root a test universe does not have, a probe of its
//!    nearest production ancestors there ("nearest tested caller", §4.4),
//!
//! then the render. The CLI runs each part in this process; the MCP tool
//! runs each on an engine child, one at a time (a composite request: a part
//! never holds one child while waiting for another). A part that fails or
//! cannot be admitted is reported `not computed` with its reason, and the
//! rest of the answer stands (B-10).

use serde_json::{Value, json};

use super::before::BaseCx;
use super::compare::Comparison;
use super::render::{RenderOpts, Rendered, render};
use crate::engine::test_select::render::Part;
use crate::engine::test_select::run::{RunPart, Target, select_argv_with, select_parts, universes};

/// What `alsem impact` / `alsem_impact` asks.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ImpactRequest {
    pub workspace: String,
    pub routines: Vec<String>,
    pub files: Vec<String>,
    /// The change is a diff (its text reaches the main part on stdin).
    pub diff: bool,
    pub diff_root: Option<String>,
    /// `--base REV`.
    pub base: Option<String>,
    pub test_apps: Vec<String>,
    pub single_app: bool,
    /// Compute the tests (default on; `--no-tests`).
    pub tests: bool,
    pub assume_no_dependents: bool,
    pub detail: bool,
    /// Build the base and print the before/after delta (B2): `--base`
    /// unless `--no-delta`.
    pub delta: bool,
}

impl ImpactRequest {
    /// The main part's command line (a diff, `--base`'s included, is read
    /// from stdin).
    #[must_use]
    pub fn main_argv(&self) -> Vec<String> {
        let mut a: Vec<String> = vec!["impact".into(), "--part".into(), "main".into()];
        for r in &self.routines {
            a.push(format!("--routine={r}"));
        }
        for f in &self.files {
            a.push(format!("--file={f}"));
        }
        if self.diff || self.base.is_some() {
            a.push("--diff=-".into());
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
        if self.delta {
            a.push("--summaries".into());
        }
        a.push("--".into());
        a.push(self.workspace.clone());
        a
    }

    /// The up part's command line over `roots`.
    #[must_use]
    pub fn up_argv(&self, roots: &[String]) -> Vec<String> {
        let mut a: Vec<String> = vec!["impact".into(), "--part".into(), "up".into()];
        for r in roots {
            a.push(format!("--root-id={r}"));
        }
        if self.assume_no_dependents {
            a.push("--no-dependents".into());
        }
        if self.detail {
            a.push("--detail".into());
        }
        a.push("--".into());
        a.push(self.workspace.clone());
        a
    }
}

/// The ids of a part's roots.
fn root_ids(doc: &Value) -> Vec<String> {
    doc["roots"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|r| r["stableId"].as_str().map(str::to_string))
        .collect()
}

/// The tests block: the select parts' answer, rendered as `alsem tests`
/// renders it (al-sem.tests/1), with each changed root's nearest tested
/// caller filled where a probe found one.
pub struct TestsOutcome {
    pub doc: Value,
    pub parts: Vec<Part>,
    /// Per root id: its nearest tested caller (production ancestor a test
    /// universe has and a test reaches).
    pub nearest: std::collections::HashMap<String, Value>,
}

/// Answer `req` (its tuple `cmp` resolved by the front), each part through
/// `run`.
pub fn orchestrate(
    req: &ImpactRequest,
    cmp: Comparison,
    opts: RenderOpts,
    run: &mut RunPart<'_>,
    base: Option<&mut BaseCx<'_>>,
) -> Result<Rendered, String> {
    orchestrate_with(req, cmp, opts, run, &|| None, base)
}

/// The before/after part of an answer (B2): the base side's block and the
/// delta ([`super::delta::compute`]).
pub struct DeltaOutcome {
    pub block: Value,
    pub delta: Value,
}

/// [`orchestrate`], with `inputs_stamp` asked once the main part answered:
/// the revision's manifest stamp a resident engine answered from (the MCP
/// front end reads it off the part's envelope), the tuple's `head`.
pub fn orchestrate_with(
    req: &ImpactRequest,
    mut cmp: Comparison,
    opts: RenderOpts,
    run: &mut RunPart<'_>,
    inputs_stamp: &dyn Fn() -> Option<String>,
    base: Option<&mut BaseCx<'_>>,
) -> Result<Rendered, String> {
    let main_argv = req.main_argv();
    let mut main = run(Target::Production, main_argv.clone())?;
    cmp.inputs_stamp = inputs_stamp();
    let ids = root_ids(&main);
    let up = if ids.is_empty() {
        Err("no changed routine resolved".to_string())
    } else {
        run(Target::Production, req.up_argv(&ids))
    };
    let tests = if !req.tests {
        Err("not asked (--no-tests; MCP `tests: false`)".to_string())
    } else if ids.is_empty() {
        Err("no changed routine resolved".to_string())
    } else {
        tests_outcome(req, &mut main, &ids, up.as_ref().ok(), &main_argv, run)
    };
    // The base side last: the production parts hold no child while it is
    // admitted (a composite request never nests waits).
    let delta = match (req.delta, base) {
        (false, _) => Err("not asked".to_string()),
        (true, None) => Err("no base runner".to_string()),
        (true, Some(bcx)) => delta_outcome(req, &cmp, &main, bcx),
    };
    Ok(render(&cmp, &main, &up, &tests, &delta, opts))
}

/// The base side and the delta.
fn delta_outcome(
    req: &ImpactRequest,
    cmp: &Comparison,
    main: &Value,
    bcx: &mut BaseCx<'_>,
) -> Result<DeltaOutcome, String> {
    let diff_text = bcx.diff_text.clone();
    let b = super::before::base_side(
        std::path::Path::new(&req.workspace),
        req.single_app,
        req.diff_root.as_deref(),
        cmp,
        &diff_text,
        main,
        bcx,
    )?;
    let pre = b.block["preimageMismatches"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    if !pre.is_empty() {
        return Err(format!(
            "refused: the diff's pre-images do not match the comparison base in {} ({})",
            crate::engine::l5::routine_rollup::plural(pre.len(), "file", "files"),
            pre.iter()
                .filter_map(Value::as_str)
                .take(5)
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    let bs = super::delta::Side::of(&b.doc).map_err(|e| format!("the base side: {e}"))?;
    let ns = super::delta::Side::of(main).map_err(|e| format!("the working tree: {e}"))?;
    let changes: Vec<String> = b.block["dependencyDeclarationChanged"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|v| v.as_str().map(str::to_string))
        .collect();
    let note = (!changes.is_empty()).then(|| changes.join("; "));
    let delta = super::delta::compute(&bs, &ns, note.as_deref());
    Ok(DeltaOutcome {
        block: b.block,
        delta,
    })
}

/// The tests part of the answer (see [`TestsOutcome`]).
fn tests_outcome(
    req: &ImpactRequest,
    main: &mut Value,
    ids: &[String],
    up: Option<&Value>,
    main_argv: &[String],
    run: &mut RunPart<'_>,
) -> Result<TestsOutcome, String> {
    let us = universes(main, &req.workspace);
    let parts = select_parts(main, ids, &us, run, main_argv)?;
    // Roots no test universe has: probe their nearest production ancestors.
    let status_of = |id: &str| -> Vec<String> {
        parts
            .iter()
            .filter_map(|p| p.result.as_ref().ok())
            .filter_map(|v| {
                v["roots"]
                    .as_array()
                    .and_then(|rs| rs.iter().find(|r| r["stableId"] == id))
                    .and_then(|r| r["status"].as_str().map(str::to_string))
            })
            .collect()
    };
    let missing: Vec<&String> = ids
        .iter()
        .filter(|id| {
            let st = status_of(id);
            !st.is_empty()
                && st
                    .iter()
                    .all(|s| s == "not-in-projection" || s == "not-loaded")
        })
        .collect();
    let mut nearest = std::collections::HashMap::new();
    if let Some(up) = up
        && !missing.is_empty()
    {
        let ancestors_of = |id: &str| -> Vec<Value> {
            up["roots"]
                .as_array()
                .and_then(|rs| rs.iter().find(|r| r["stableId"] == id))
                .and_then(|r| r["nearestAncestors"].as_array().cloned())
                .unwrap_or_default()
        };
        let mut probes: Vec<String> = Vec::new();
        for id in &missing {
            for a in ancestors_of(id) {
                if let Some(s) = a["stableId"].as_str()
                    && !probes.iter().any(|p| p == s)
                {
                    probes.push(s.to_string());
                }
            }
        }
        let mut found: Vec<Value> = Vec::new();
        for (app, u) in &us {
            let Ok(v) = run(Target::Tests(u), select_argv_with(u, &[], &probes)) else {
                continue;
            };
            // Probes off other production bytes are not used.
            if v["productionSourceStamp"] != main["productionSourceStamp"] {
                continue;
            }
            for p in v["probes"].as_array().into_iter().flatten() {
                if p["status"] == "reached" {
                    let mut p = p.clone();
                    p["testApp"] = app["name"].clone();
                    found.push(p);
                }
            }
        }
        for id in missing {
            for a in ancestors_of(id) {
                if let Some(p) = found.iter().find(|p| p["stableId"] == a["stableId"]) {
                    nearest.insert(
                        id.clone(),
                        json!({"stableId": a["stableId"], "display": a["display"],
                               "hopsUp": a["hops"], "testApp": p["testApp"],
                               "nearestTest": p["nearest"], "nearestTestHops": p["nearestHops"],
                               "nearestTestTier": p["nearestTier"]}),
                    );
                    break;
                }
            }
        }
    }
    let rendered = crate::engine::test_select::render::render(
        main,
        &parts,
        crate::engine::test_select::render::RenderOpts {
            tier: crate::engine::test_select::render::TierPick::Local,
            max_tests: None,
            json: true,
            surface: Default::default(),
            single_app: false,
            // Impact embeds this block in `al-sem.impact/1`: it keeps the
            // tests version 1 shape until impact's next major.
            schema: 1,
        },
    );
    let mut doc: Value =
        serde_json::from_str(&rendered.text).map_err(|e| format!("tests block: {e}"))?;
    if let Some(changed) = doc["changed"].as_array_mut() {
        for c in changed {
            if let Some(n) = c["stableId"].as_str().and_then(|id| nearest.get(id)) {
                c["nearestTestedCaller"] = n.clone();
            }
        }
    }
    Ok(TestsOutcome {
        doc,
        parts,
        nearest,
    })
}

#[cfg(test)]
#[path = "run_tests.rs"]
mod tests;

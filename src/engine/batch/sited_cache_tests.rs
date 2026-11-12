//! TLS-28's guards (spec §3), on `tests/fixtures/up-parallel/ws` built
//! cross-app in-process: the up part (`UpJob`, `ViewNeeds::SITED`) through the
//! standalone driver, as `alsem impact` runs each part.

use std::sync::Arc;

use super::super::job::{
    Doc, Job, JobFailure, PlanPart, ResolveCx, SideDoc, SideIx, ViewCx, ViewDemand, ViewFailure,
    ViewKey, ViewNeeds,
};
use super::super::output::{JobOutput, WriteAction};
use super::{BUILDS, hits, with_sited_cache};
use crate::engine::batch::job::HandOff;
use crate::engine::impact::jobs::{UpAsk, UpJob};
use crate::engine::impact::up::up_par_tests::{fixture_dir, root_ids};
use crate::engine::resident::session::REVISION_READ;
use crate::program::inputs::WorkspaceInputs;

fn builds() -> usize {
    BUILDS.with(std::cell::Cell::get)
}

fn read(ws: &std::path::Path) -> Arc<WorkspaceInputs> {
    WorkspaceInputs::read(ws, &REVISION_READ)
}

/// One up part over the fixture's roots on a fresh read of `ws`: its JSON,
/// without `cost` (walk times and scratch bytes differ run to run).
fn up_part(ws: &std::path::Path, single_app: bool) -> serde_json::Value {
    let held = HandOff::new(read(ws));
    let job = UpJob::new(
        &held,
        UpAsk {
            roots: root_ids(),
            assume_no_dependents: false,
            detail: true,
            single_app,
        },
    );
    let out = crate::engine::batch::standalone_output(&job);
    assert_eq!(out.exit, 0, "{}", out.message());
    let stdout: Vec<u8> = out
        .actions
        .iter()
        .filter_map(|a| match a {
            WriteAction::Stdout(b) => Some(b.clone()),
            _ => None,
        })
        .flatten()
        .collect();
    let mut v: serde_json::Value = serde_json::from_slice(&stdout).unwrap();
    v.as_object_mut().unwrap().remove("cost");
    v
}

/// A model demand on `ws`'s cross-app view (the main part's shape): it
/// computes nothing.
struct ModelView(ViewKey, Arc<WorkspaceInputs>);

impl Job for ModelView {
    fn name(&self) -> &'static str {
        "model-view"
    }
    fn demands(&self) -> Vec<ViewDemand> {
        vec![ViewDemand {
            key: self.0.clone(),
            side: SideIx(0),
            needs: ViewNeeds::MODEL,
            inputs: Arc::clone(&self.1),
        }]
    }
    fn resolve(&self, _: SideIx, _: &ResolveCx<'_>) -> Result<PlanPart, JobFailure> {
        Ok(PlanPart {
            plan: crate::engine::session::Plan::merge(std::iter::empty()),
            reads: vec![],
            carry: None,
        })
    }
    fn compute(&self, _: SideIx, _: &mut ViewCx<'_>) -> Result<SideDoc, JobFailure> {
        Ok(SideDoc::new(()))
    }
    fn view_failed(&self, _: SideIx, f: &ViewFailure) -> Result<SideDoc, JobFailure> {
        panic!("the fixture builds: {f:?}")
    }
    // Answers a kept graph too, so a view wrongly served from one is caught by
    // the build count rather than by a panic.
    fn sited(
        &self,
        _: SideIx,
        _: Result<&crate::program::impact_graph::SitedGraph, &str>,
        _: &crate::engine::l5::digest_cli::ReachWorkers,
    ) -> Result<SideDoc, JobFailure> {
        Ok(SideDoc::new(()))
    }
    fn finalize(&self, _: Vec<Result<SideDoc, JobFailure>>) -> Doc {
        Doc::new(())
    }
    fn serialize(&self, _: Doc) -> JobOutput {
        unreachable!("driven, not serialized")
    }
}

fn model_view(ws: &std::path::Path) {
    model_view_on(ws, false);
}

fn model_view_on(ws: &std::path::Path, single_app: bool) {
    let inputs = read(ws);
    let job = ModelView(ViewKey::new(&inputs.workspace, single_app, false), inputs);
    super::super::drive(&[&job], &mut |_, _, _| 0);
}

/// Guard 1 (step 1): the up part, then the same part again (as tests-select
/// follows it): the second runs on the kept graph, and both equal a run with
/// no scope.
///
/// Discrimination (2026-11-12): `sited_cache::store` keeping a graph that is
/// not the one built (every site's anchor set to `None` before it is kept)
/// fails here at "the kept graph's answer" and in the step-2 test below at
/// "the model view's graph's answer"; restored, both pass. (Clearing the
/// hidden-caller flags instead passed: the fixture has none.)
#[test]
fn a_hit_gives_the_cold_answer() {
    let dir = fixture_dir();
    let ws = dir.path().join("ws");
    let cold = up_part(&ws, false);
    let (first, second, h, b) = with_sited_cache(|| {
        let (h0, b0) = (hits(), builds());
        let first = up_part(&ws, false);
        let second = up_part(&ws, false);
        (first, second, hits() - h0, builds() - b0)
    });
    assert_eq!((h, b), (1, 1), "precondition: one build, then one hit");
    assert_eq!(first, cold, "the storing run's answer");
    assert_eq!(second, cold, "the kept graph's answer");
}

/// Guard 1 (step 2): the main part's model view keeps the graph off its
/// program; the up part after it hits and gives the cold answer.
#[test]
fn the_model_views_graph_serves_the_up_part() {
    let dir = fixture_dir();
    let ws = dir.path().join("ws");
    let cold = up_part(&ws, false);
    let (up, h, b) = with_sited_cache(|| {
        let (h0, b0) = (hits(), builds());
        model_view(&ws);
        let up = up_part(&ws, false);
        (up, hits() - h0, builds() - b0)
    });
    assert_eq!(
        (h, b),
        (1, 1),
        "precondition: the model view built, the up part hit"
    );
    assert_eq!(up, cold, "the model view's graph's answer");
}

/// Guard 2: different input bytes, or a different view key, miss.
///
/// Discrimination (2026-11-12): `with_hit` ignoring the stamp (`kept.stamp ==
/// stamp || true`) fails at "changed bytes miss" (1 build seen, 2 expected);
/// restored, passes.
#[test]
fn a_different_read_or_key_misses() {
    let dir = fixture_dir();
    let ws = dir.path().join("ws");
    let b = with_sited_cache(|| {
        let b0 = builds();
        up_part(&ws, false);
        // One `.al` file gains a comment between the two reads.
        let al = walk_al(&ws);
        let text = std::fs::read_to_string(&al).unwrap();
        std::fs::write(&al, format!("// TLS-28\n{text}")).unwrap();
        up_part(&ws, false);
        builds() - b0
    });
    assert_eq!(b, 2, "changed bytes miss");

    // The same bytes in a second folder: another workspace, another key.
    let (one, two) = (fixture_dir(), fixture_dir());
    let (h, b) = with_sited_cache(|| {
        let (h0, b0) = (hits(), builds());
        up_part(&one.path().join("ws"), false);
        up_part(&two.path().join("ws"), false);
        (hits() - h0, builds() - b0)
    });
    assert_eq!((h, b), (0, 2), "another workspace's view misses");
}

/// Guard 3: a view that reads more than the sited graph builds, whatever is
/// kept; and outside a scope nothing is kept.
///
/// Discrimination (2026-11-12): `sited_only` returning `true` for every view
/// fails at "a model view after a kept graph builds" (1 build seen, 2
/// expected); restored, passes.
#[test]
fn uncovered_needs_build_and_no_scope_keeps_nothing() {
    let dir = fixture_dir();
    let ws = dir.path().join("ws");
    let b = with_sited_cache(|| {
        let b0 = builds();
        up_part(&ws, false);
        model_view(&ws);
        builds() - b0
    });
    assert_eq!(b, 2, "a model view after a kept graph builds");

    let (h, b) = {
        let (h0, b0) = (hits(), builds());
        up_part(&ws, false);
        up_part(&ws, false);
        (hits() - h0, builds() - b0)
    };
    assert_eq!((h, b), (0, 2), "no scope: every part builds");
}

/// Sol review r1: a build that keeps no graph itself (here a single-app
/// model view, as `impact --single-app --base` builds after its cross-app up
/// part) still drops the kept one: no graph lives through another build.
///
/// Discrimination (2026-11-12): clearing only before a participating view's
/// build (`if cache_stamp.is_some()` around `clear()` in `run_view`) fails at
/// "the single-app build dropped the kept graph"; restored, passes.
#[test]
fn any_build_drops_the_kept_graph() {
    let dir = fixture_dir();
    let ws = dir.path().join("ws");
    let (before, after) = with_sited_cache(|| {
        up_part(&ws, false);
        let before = super::kept();
        model_view_on(&ws, true);
        (before, super::kept())
    });
    assert!(before, "precondition: the up part kept its graph");
    assert!(!after, "the single-app build dropped the kept graph");
}

/// Opus review r1: impact's `--base` side runs its main part (a cross-app
/// model view) under `without_sited_cache`: it keeps no graph that no later
/// part reads, and the graph the scope held does not live through it.
///
/// Discrimination (2026-11-12): `without_sited_cache` running `f()` alone
/// (the scope left in place) fails at "no graph lives through the base part"
/// (the model view stored its own graph); restored, passes.
#[test]
fn the_base_side_keeps_nothing() {
    let dir = fixture_dir();
    let ws = dir.path().join("ws");
    let (before, inside, after) = with_sited_cache(|| {
        up_part(&ws, false);
        let before = super::kept();
        let inside = super::without_sited_cache(|| {
            model_view(&ws);
            super::kept()
        });
        (before, inside, super::kept())
    });
    assert!(before, "precondition: the up part kept its graph");
    assert!(!inside, "no graph lives through the base part");
    assert!(!after, "the base part kept nothing");
}

/// Sol review r1: two `.al` names that are not UTF-8 and read alike
/// (`b"\xff.al"`, `b"\xfe.al"` are both `\u{fffd}.al`) share one manifest
/// key, so the stamp would miss an edit to the first: such a read takes no
/// part, and every part builds. The rule is any manifest key with U+FFFD, so
/// it covers `src:` and `pkg:` names the same way (sol review r2: package
/// paths); this test states it with `.al` names.
///
/// Discrimination (2026-11-12): `stamp_for` without its `lossy` check fails
/// at "lossy twins: every part builds" (1 build seen, 2 expected); restored,
/// passes.
#[cfg(unix)]
#[test]
fn lossy_twin_names_take_no_part() {
    use std::os::unix::ffi::OsStrExt;
    let dir = fixture_dir();
    let ws = dir.path().join("ws");
    let al = walk_al(&ws);
    let folder = al.parent().unwrap();
    for (name, id) in [(&b"\xff.al"[..], 51490), (&b"\xfe.al"[..], 51491)] {
        std::fs::write(
            folder.join(std::ffi::OsStr::from_bytes(name)),
            format!("codeunit {id} \"UP Twin {id}\"\n{{\n}}\n"),
        )
        .unwrap();
    }
    let inputs = read(&ws);
    assert!(
        inputs
            .al_files
            .windows(2)
            .any(|w| w[0].rel_posix == w[1].rel_posix),
        "precondition: two files share a lossy name"
    );
    let b = with_sited_cache(|| {
        let b0 = builds();
        up_part(&ws, false);
        up_part(&ws, false);
        builds() - b0
    });
    assert_eq!(b, 2, "lossy twins: every part builds");
}

/// The first `.al` file under `ws`.
fn walk_al(ws: &std::path::Path) -> std::path::PathBuf {
    let mut stack = vec![ws.to_path_buf()];
    while let Some(d) = stack.pop() {
        let mut entries: Vec<_> = std::fs::read_dir(&d)
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        entries.sort();
        for p in entries {
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|e| e == "al") {
                return p;
            }
        }
    }
    panic!("no .al file under {}", ws.display())
}

//! #277 (#160 step 12): the batch driver. Standalone is a batch of one (D1):
//! `alsem <cmd>` builds one [`Job`] and runs [`run_standalone`], so a batch
//! and a standalone run share one code path.
//!
//! Per view, in order of first demand (D6): build → each demand's `resolve`
//! (D4.2) → the union of the parts (D5) → ONE `prepare` → each demand's
//! `compute`, in job order → the view is dropped. A job is finalized after its
//! last view; a job with no view (a failed validation) before the first.

pub mod job;
pub mod output;

use std::sync::Arc;

use crate::engine::gate::run::{AnalysisModel, AnalysisTarget, build_analysis_model};
use crate::engine::perf_trace as pt;
use crate::engine::session::{JobId, Plan, View};
use crate::program::inputs::WorkspaceInputs;
use crate::program::resolve::full::FreshCoverage;

use job::{Doc, Job, JobFailure, PlanPart, ResolveCx, SideDoc, ViewCx, ViewDemand, ViewFailure};
use output::replay;

/// Run one job as its own process does (D1): through the driver, then its
/// writes on the real streams in its standalone order (D2.3). Returns the exit.
pub fn run_standalone(job: &dyn Job) -> u8 {
    let doc = drive(&[job]).pop().expect("one document per job");
    replay(&job.serialize(doc), job.write_failure_exit())
}

/// One job's document, through the driver: a library entry point's run.
pub(crate) fn run_doc(job: &dyn Job) -> Doc {
    drive(&[job]).pop().expect("one document per job")
}

/// The driver: every job's document, in job order.
fn drive(jobs: &[&dyn Job]) -> Vec<Doc> {
    let demands: Vec<Vec<ViewDemand>> = jobs.iter().map(|j| j.demands()).collect();
    let mut keys = Vec::new();
    for d in demands.iter().flatten() {
        if !keys.contains(&d.key) {
            keys.push(d.key.clone());
        }
    }
    pt::instant_lazy(
        "batch",
        "batch.driver",
        || serde_json::json!({ "jobs": jobs.len(), "views": keys.len(), "workers": 1 }),
    );
    let mut sides: Vec<Vec<Option<Result<SideDoc, JobFailure>>>> = demands
        .iter()
        .map(|ds| ds.iter().map(|_| None).collect())
        .collect();
    let mut docs: Vec<Option<Doc>> = jobs.iter().map(|_| None).collect();
    let mut finalize = |j: usize, sides: &mut Vec<Option<Result<SideDoc, JobFailure>>>| {
        let done = sides
            .drain(..)
            .map(|s| s.expect("every side ran"))
            .collect();
        docs[j] = Some(jobs[j].finalize(done));
    };
    // (0) The jobs with no view.
    for (j, ds) in demands.iter().enumerate() {
        if ds.is_empty() {
            finalize(j, &mut sides[j]);
        }
    }
    for key in &keys {
        let on_view: Vec<(usize, &ViewDemand)> = demands
            .iter()
            .enumerate()
            .flat_map(|(j, ds)| ds.iter().map(move |d| (j, d)))
            .filter(|(_, d)| d.key == *key)
            .collect();
        for (j, d, side) in run_view(jobs, &on_view) {
            sides[j][usize::from(d)] = Some(side);
        }
        // (10) Every job whose last demand this view held.
        for (j, ds) in demands.iter().enumerate() {
            if ds.last().is_some_and(|d| d.key == *key) {
                finalize(j, &mut sides[j]);
            }
        }
    }
    docs.into_iter()
        .map(|d| d.expect("every job finalized"))
        .collect()
}

/// One view: its build, its demands' parts, one `prepare`, their computes.
/// Returns (job, side, result) per demand, in the order given.
fn run_view(
    jobs: &[&dyn Job],
    on_view: &[(usize, &ViewDemand)],
) -> Vec<(usize, u8, Result<SideDoc, JobFailure>)> {
    let (_, first) = on_view[0];
    let analyze_extras = first.needs.analyze_extras;
    // shortcut: two builders until Task 6's one view builder; a view's demands
    // agree on which.
    assert!(
        on_view
            .iter()
            .all(|(_, d)| d.needs.analyze_extras == analyze_extras),
        "one builder per view"
    );
    // The view is built from the first demand's read: every demand on it must
    // hold that same read, never a second one of the workspace.
    assert!(
        on_view
            .iter()
            .all(|(_, d)| Arc::ptr_eq(&d.inputs, &first.inputs)),
        "a view's demands share one WorkspaceInputs"
    );
    let single_app = first.key.universe == job::ViewUniverse::SingleApp;
    let built = build_view(&first.inputs, single_app, analyze_extras);
    let (target, fresh) = match built {
        Err(failure) => {
            return on_view
                .iter()
                .map(|(j, d)| (*j, d.side.0, jobs[*j].view_failed(d.side, &failure)))
                .collect();
        }
        Ok(built) => built,
    };
    let mut view = View::new(target);
    // (2b) Each demand's part, the identity table shared and dropped here.
    let parts: Vec<Result<PlanPart, JobFailure>> = {
        let cx = ResolveCx::new(view.model());
        on_view
            .iter()
            .map(|(j, d)| jobs[*j].resolve(d.side, &cx))
            .collect()
    };
    // What each demand reads, kept before the parts' plans merge (D6.1, D6.2).
    let reads: Vec<Option<(Vec<_>, Vec<_>, bool)>> = parts
        .iter()
        .map(|p| {
            p.as_ref()
                .ok()
                .map(|p| (p.reads.clone(), p.plan.keys(), p.plan.has_tree(JobId(0))))
        })
        .collect();
    let mut carries = Vec::new();
    let mut plans = Vec::new();
    let mut results: Vec<Option<Result<SideDoc, JobFailure>>> = Vec::new();
    for ((j, _), part) in on_view.iter().zip(parts) {
        match part {
            Ok(p) => {
                plans.push((job_id(*j), p.plan));
                carries.push(p.carry);
                results.push(None);
            }
            Err(f) => {
                carries.push(None);
                results.push(Some(Err(f)));
            }
        }
    }
    // (4)-(8) ONE plan, ONE prepare.
    view.prepare(&Plan::merge(plans));
    // (9) Each demand's compute, in job order.
    for (i, (j, d)) in on_view.iter().enumerate() {
        let Some((_, keys, has_tree)) = reads[i].clone() else {
            continue;
        };
        let later = || reads[i + 1..].iter().flatten();
        let mut cx = ViewCx {
            view: &mut view,
            fresh: fresh.as_ref(),
            job: job_id(*j),
            keys,
            has_tree,
            later_cells: later().flat_map(|r| r.0.clone()).collect(),
            later_runs: later().flat_map(|r| r.1.clone()).collect(),
            carry: carries[i].take(),
        };
        results[i] = Some(jobs[*j].compute(d.side, &mut cx));
    }
    drop(view);
    on_view
        .iter()
        .zip(results)
        .map(|((j, d), r)| (*j, d.side.0, r.expect("every demand computed")))
        .collect()
}

fn job_id(j: usize) -> JobId {
    JobId(u32::try_from(j).expect("a batch of at most u32::MAX jobs"))
}

/// The view's model, and analyze's program coverage when its builder ran.
type Built = (AnalysisTarget, Option<Result<FreshCoverage, String>>);

/// Build a view: analyze's builder (its `fresh`), or `AnalysisTarget`'s.
fn build_view(
    inputs: &Arc<WorkspaceInputs>,
    single_app: bool,
    analyze_extras: bool,
) -> Result<Built, Box<ViewFailure>> {
    if analyze_extras {
        let AnalysisModel { fresh, model, .. } = build_analysis_model(inputs, single_app);
        match model {
            Ok(target) => Ok((target, Some(fresh))),
            Err(failure) => Err(Box::new(ViewFailure::Model { fresh, failure })),
        }
    } else {
        AnalysisTarget::build_target(inputs, single_app)
            .map(|t| (t, None))
            .map_err(|f| Box::new(ViewFailure::Target(f)))
    }
}

#[cfg(test)]
mod tests {
    use super::job::*;
    use super::output::JobOutput;
    use super::*;
    use crate::program::inputs::WorkspaceNeeds;

    /// A job with one demand on `key` over `inputs`; it never gets past the
    /// driver's checks in these tests.
    struct OnView(ViewKey, Arc<WorkspaceInputs>);

    impl Job for OnView {
        fn demands(&self) -> Vec<ViewDemand> {
            vec![ViewDemand {
                key: self.0.clone(),
                side: SideIx(0),
                needs: ViewNeeds::default(),
                inputs: Arc::clone(&self.1),
            }]
        }
        fn resolve(&self, _: SideIx, _: &ResolveCx<'_>) -> Result<PlanPart, JobFailure> {
            panic!("resolved past the driver's checks")
        }
        fn compute(&self, _: SideIx, _: &mut ViewCx<'_>) -> Result<SideDoc, JobFailure> {
            panic!("computed past the driver's checks")
        }
        fn view_failed(&self, _: SideIx, _: &ViewFailure) -> Result<SideDoc, JobFailure> {
            panic!("built past the driver's checks")
        }
        fn serialize(&self, _: Doc) -> JobOutput {
            unreachable!()
        }
    }

    /// #277 R5-3: two demands on one view key with two reads of the workspace
    /// are refused before the build. Precondition: the two reads are distinct
    /// `Arc`s of one workspace, so the key alone cannot tell them apart.
    #[test]
    fn a_view_refuses_two_reads_of_its_workspace() {
        let d = tempfile::tempdir().unwrap();
        let read = || WorkspaceInputs::read(d.path(), &WorkspaceNeeds::default());
        let (a, b) = (read(), read());
        assert!(!Arc::ptr_eq(&a, &b), "precondition: two reads");
        let key = ViewKey::new(d.path(), true, false);
        let (ja, jb) = (OnView(key.clone(), a), OnView(key, b));
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| drive(&[&ja, &jb])));
        let msg = r.err().expect("the driver refuses the view");
        let msg = msg
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| msg.downcast_ref::<String>().cloned())
            .unwrap_or_default();
        assert_eq!(msg, "a view's demands share one WorkspaceInputs");
    }
}

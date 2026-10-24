//! #277 (#160 step 12): the batch driver. Standalone is a batch of one (D1):
//! `alsem <cmd>` builds one [`Job`] and runs [`run_standalone`], so a batch
//! and a standalone run share one code path.
//!
//! Per view, in order of first demand (D6): the one builder
//! (`build_view_model`; graph demands are done off its program, D9) → each
//! demand's `resolve` (D4.2) → the union of the parts (D5) → ONE `prepare` →
//! each demand's `compute`, in job order → the view is dropped. A job is
//! finalized after its last view; a job with no view (a failed validation, a
//! file/file `diff`) before the first. A job whose side failed reads no later
//! view (D8).

pub mod job;
pub mod output;

use std::sync::Arc;

use crate::engine::gate::run::{ViewModel, build_view_model};
use crate::engine::perf_trace as pt;
use crate::engine::session::{JobId, Plan, View};

use job::{Doc, Job, JobFailure, PlanPart, ResolveCx, SideDoc, ViewCx, ViewDemand};
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

/// A one-demand job's side on a target built already, through the driver's
/// steps ([`run_view`]): `resolve`, ONE `prepare`, `compute`. A test
/// hand-states the model's precondition between the build and the job
/// (CLAUDE.md), which the driver's own builder cannot.
#[cfg(test)]
pub(crate) fn run_on_target(
    job: &dyn Job,
    target: crate::engine::gate::run::AnalysisTarget,
) -> Result<SideDoc, JobFailure> {
    let d = job.demands().into_iter().next().expect("a job with a view");
    let mut view = View::new(target);
    let part = job.resolve(d.side, &ResolveCx::new(view.model()))?;
    let (keys, has_tree) = (part.plan.keys(), part.plan.has_tree(JobId(0)));
    view.prepare(&Plan::merge([(JobId(0), part.plan)]));
    let mut cx = ViewCx {
        view: &mut view,
        fresh: None,
        job: JobId(0),
        keys,
        has_tree,
        later_cells: vec![],
        later_runs: vec![],
        carry: part.carry,
    };
    job.compute(d.side, &mut cx)
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
        // Each demand on the view, and its slot (job, demand index).
        let mut on_view: Vec<(usize, &ViewDemand)> = Vec::new();
        let mut slots = Vec::new();
        for (j, ds) in demands.iter().enumerate() {
            for (k, d) in ds.iter().enumerate().filter(|(_, d)| d.key == *key) {
                // A job whose earlier side failed reads no later view (D8: a
                // standalone `diff` never loads its new side after its old
                // one failed); the failure stands for the side.
                let failed = sides[j].iter().flatten().find_map(|s| s.as_ref().err());
                match failed.cloned() {
                    Some(f) => sides[j][k] = Some(Err(f)),
                    None => {
                        on_view.push((j, d));
                        slots.push((j, k));
                    }
                }
            }
        }
        if !on_view.is_empty() {
            for ((j, k), side) in slots.into_iter().zip(run_view(jobs, &on_view)) {
                sides[j][k] = Some(side);
            }
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
/// Returns each demand's result, in the order given.
fn run_view(
    jobs: &[&dyn Job],
    on_view: &[(usize, &ViewDemand)],
) -> Vec<Result<SideDoc, JobFailure>> {
    let (_, first) = on_view[0];
    // The view is built from the first demand's read: every demand on it must
    // hold that same read, never a second one of the workspace.
    assert!(
        on_view
            .iter()
            .all(|(_, d)| Arc::ptr_eq(&d.inputs, &first.inputs)),
        "a view's demands share one WorkspaceInputs"
    );
    let needs = on_view
        .iter()
        .fold(first.needs, |n, (_, d)| n.union(d.needs));
    let finish = |results: Vec<Option<Result<SideDoc, JobFailure>>>| {
        results
            .into_iter()
            .map(|r| r.expect("every demand computed"))
            .collect()
    };
    let mut results: Vec<Option<Result<SideDoc, JobFailure>>> =
        on_view.iter().map(|_| None).collect();
    // (1) The one builder; graph demands are done off its program.
    let built = build_view_model(&first.inputs, &first.key, needs, |program| {
        for (i, (j, d)) in on_view.iter().enumerate() {
            if d.needs.graph {
                results[i] = Some(jobs[*j].program(d.side, program));
            }
        }
    });
    let (target, fresh) = match built {
        Ok(Some(ViewModel { target, fresh, .. })) => (target, fresh),
        Ok(None) => return finish(results),
        Err(failure) => {
            for (i, (j, d)) in on_view.iter().enumerate() {
                results[i].get_or_insert_with(|| jobs[*j].view_failed(d.side, &failure));
            }
            return finish(results);
        }
    };
    // The model demands; a graph demand is done.
    let on_view: Vec<(usize, (usize, &ViewDemand))> = on_view
        .iter()
        .copied()
        .enumerate()
        .filter(|(i, _)| results[*i].is_none())
        .collect();
    let mut view = View::new(target);
    // (2b) Each demand's part, the identity table shared and dropped here.
    let parts: Vec<Result<PlanPart, JobFailure>> = {
        let cx = ResolveCx::new(view.model());
        on_view
            .iter()
            .map(|(_, (j, d))| jobs[*j].resolve(d.side, &cx))
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
    for ((i, (j, _)), part) in on_view.iter().zip(parts) {
        match part {
            Ok(p) => {
                plans.push((job_id(*j), p.plan));
                carries.push(p.carry);
            }
            Err(f) => {
                carries.push(None);
                results[*i] = Some(Err(f));
            }
        }
    }
    // (4)-(8) ONE plan, ONE prepare.
    view.prepare(&Plan::merge(plans));
    // (9) Each demand's compute, in job order.
    for (k, (i, (j, d))) in on_view.iter().enumerate() {
        let Some((_, keys, has_tree)) = reads[k].clone() else {
            continue;
        };
        let later = || reads[k + 1..].iter().flatten();
        let mut cx = ViewCx {
            view: &mut view,
            fresh: fresh.as_ref(),
            job: job_id(*j),
            keys,
            has_tree,
            later_cells: later().flat_map(|r| r.0.clone()).collect(),
            later_runs: later().flat_map(|r| r.1.clone()).collect(),
            carry: carries[k].take(),
        };
        results[*i] = Some(jobs[*j].compute(d.side, &mut cx));
    }
    drop(view);
    // The view's model is gone: its post-attach phase ends (#160 §4.1.4), so
    // the next view's build is in no phase.
    pt::end_phase("post_attach");
    finish(results)
}

fn job_id(j: usize) -> JobId {
    JobId(u32::try_from(j).expect("a batch of at most u32::MAX jobs"))
}

#[cfg(test)]
mod tests {
    use super::job::*;
    use super::output::JobOutput;
    use super::*;
    use crate::program::inputs::{WorkspaceInputs, WorkspaceNeeds};

    /// A job with one demand on `key` over `inputs`; it never gets past the
    /// driver's checks in these tests.
    struct OnView(ViewKey, Arc<WorkspaceInputs>);

    impl Job for OnView {
        fn demands(&self) -> Vec<ViewDemand> {
            vec![ViewDemand {
                key: self.0.clone(),
                side: SideIx(0),
                needs: ViewNeeds::MODEL,
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

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
pub mod manifest;
pub mod output;

use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::engine::gate::run::{ViewModel, build_view_model};
use crate::engine::perf_trace as pt;
use crate::engine::session::{JobId, Plan, View};

use job::{Doc, Job, JobFailure, PlanPart, ResolveCx, SideDoc, ViewCx, ViewDemand};
use output::{JobOutput, Stream, replay, replay_on, write_error};

/// Run one job as its own process does (D1): through the driver, then its
/// writes on the real streams in its standalone order (D2.3). Returns the exit.
pub fn run_standalone(job: &dyn Job) -> u8 {
    let _total = job.standalone_span().map(|name| pt::span(job.name(), name));
    let out = serialize(0, job, run_doc(job));
    replay(&out, job.write_failure_exit())
}

/// One job's document, through the driver: a library entry point's run.
pub(crate) fn run_doc(job: &dyn Job) -> Doc {
    let mut doc = None;
    drive(&[job], &mut |_, d| doc = Some(d));
    doc.expect("one document per job")
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
        inputs: &d.inputs,
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

/// Where a batch job's two streams go (D2.1), resolved by the manifest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JobStreams {
    pub stdout: PathBuf,
    pub stderr: PathBuf,
}

/// One job's line on the run's stdout (D2.2), in manifest order. `exit` is
/// the job's own exit; `files` every file and directory its actions wrote.
#[derive(Debug, serde::Serialize)]
pub struct JobLine {
    pub batch: u32,
    pub job: u32,
    pub exit: Option<u8>,
    pub stdout: PathBuf,
    pub stderr: PathBuf,
    pub files: Vec<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worker: Option<String>,
}

/// Run batch `b` in this process (D2.5): one driver over every job, each
/// job written as soon as it and every earlier job are finished (D6 (11)).
/// A job's own write failure is its line's exit, never the run's (D2.2).
pub fn run_batch(b: u32, jobs: &[(&dyn Job, JobStreams)]) -> Vec<JobLine> {
    let only: Vec<&dyn Job> = jobs.iter().map(|(job, _)| *job).collect();
    let mut outputs: Vec<Option<JobOutput>> = jobs.iter().map(|_| None).collect();
    let mut lines = Vec::new();
    drive(&only, &mut |j, doc| {
        outputs[j] = Some(serialize(j, only[j], doc));
        while let Some(out) = outputs.get_mut(lines.len()).and_then(Option::take) {
            let i = lines.len();
            lines.push(write_job(b, i, jobs[i].0, &jobs[i].1, &out));
        }
    });
    lines
}

/// Job `j`'s document as its writes, inside its job span.
fn serialize(j: usize, job: &dyn Job, doc: Doc) -> JobOutput {
    let _s = job_span(j, job, "serialize");
    job.serialize(doc)
}

/// Job `i`'s writes: its stdout and stderr actions into its two files, its
/// own files where they name. A failure ends them (D2.2): the write error
/// line on the process's stderr, the job's write-failure exit.
fn write_job(b: u32, i: usize, job: &dyn Job, streams: &JobStreams, out: &JobOutput) -> JobLine {
    let create = |p: &Path| File::create(p).map_err(|e| write_error(&p.display().to_string(), &e));
    let (exit, files) = match (create(&streams.stdout), create(&streams.stderr)) {
        (Ok(mut stdout), Ok(mut stderr)) => {
            let (o, e) = (
                streams.stdout.display().to_string(),
                streams.stderr.display().to_string(),
            );
            replay_on(
                out,
                job.write_failure_exit(),
                Stream(&mut stdout, &o),
                Stream(&mut stderr, &e),
            )
        }
        _ => (job.write_failure_exit(), vec![]),
    };
    JobLine {
        batch: b,
        job: u32::try_from(i).expect("a batch of at most u32::MAX jobs"),
        exit: Some(exit),
        stdout: streams.stdout.clone(),
        stderr: streams.stderr.clone(),
        files,
        worker: None,
    }
}

/// Job `j`'s span for one `step` (D10): named `job.<j>.<cmd>`, with a
/// `job.step` instant naming the step.
fn job_span(j: usize, job: &dyn Job, step: &'static str) -> pt::SpanGuard {
    let span = pt::span("batch", format!("job.{j}.{}", job.name()));
    pt::instant_lazy(
        "batch",
        "job.step",
        || serde_json::json!({ "job": j, "cmd": job.name(), "step": step }),
    );
    span
}

/// The driver: hands each job's document to `done` as the job finalizes.
fn drive(jobs: &[&dyn Job], done: &mut dyn FnMut(usize, Doc)) {
    // A demand is dropped with its view, and with it its workspace read
    // (E12-C2).
    let mut demands: Vec<Vec<Option<ViewDemand>>> = jobs
        .iter()
        .map(|j| j.demands().into_iter().map(Some).collect())
        .collect();
    let mut keys = Vec::new();
    for d in demands.iter().flatten().flatten() {
        if !keys.contains(&d.key) {
            keys.push(d.key.clone());
        }
    }
    // Each job's last view in VISIT order (E12-C1): `[diff A B, diff B A]`
    // visits A then B, so job 2 (old side on B, new on A) ends at B.
    let last_view: Vec<Option<usize>> = demands
        .iter()
        .map(|ds| {
            ds.iter()
                .flatten()
                .map(|d| keys.iter().position(|k| *k == d.key).expect("a key"))
                .max()
        })
        .collect();
    pt::instant_lazy(
        "batch",
        "batch.driver",
        || serde_json::json!({ "jobs": jobs.len(), "views": keys.len(), "workers": 1 }),
    );
    let mut sides: Vec<Vec<Option<Result<SideDoc, JobFailure>>>> = demands
        .iter()
        .map(|ds| ds.iter().map(|_| None).collect())
        .collect();
    let mut finalize = |j: usize, sides: &mut Vec<Option<Result<SideDoc, JobFailure>>>| {
        let ran = sides
            .drain(..)
            .map(|s| s.expect("every side ran"))
            .collect();
        let doc = {
            let _s = job_span(j, jobs[j], "finalize");
            jobs[j].finalize(ran)
        };
        done(j, doc);
    };
    // (0) The jobs with no view.
    for (j, last) in last_view.iter().enumerate() {
        if last.is_none() {
            finalize(j, &mut sides[j]);
        }
    }
    for (v, key) in keys.iter().enumerate() {
        // Every fill of the view lies inside its span (D10); the next view
        // builds only after this one's drop instant (D4.1).
        let view_span = pt::span("batch", format!("view.{v}"));
        // Each demand on the view, and its slot (job, demand index).
        let mut on_view: Vec<(usize, &ViewDemand)> = Vec::new();
        let mut slots = Vec::new();
        for (j, ds) in demands.iter().enumerate() {
            for (k, d) in ds.iter().enumerate() {
                let Some(d) = d.as_ref().filter(|d| d.key == *key) else {
                    continue;
                };
                // A side after a failed EARLIER side (by side order) is not
                // read (D8: a standalone `diff` never loads its new side after
                // its old one failed); that failure stands for it.
                let failed = sides[j][..k]
                    .iter()
                    .flatten()
                    .find_map(|s| s.as_ref().err());
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
        for d in demands.iter_mut().flatten() {
            if d.as_ref().is_some_and(|d| d.key == *key) {
                *d = None;
            }
        }
        pt::instant_lazy("batch", "view.drop", || serde_json::json!({ "view": v }));
        drop(view_span);
        // (10) Every job whose last view this was.
        for (j, last) in last_view.iter().enumerate() {
            if *last == Some(v) {
                finalize(j, &mut sides[j]);
            }
        }
    }
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
                let _s = job_span(*j, jobs[*j], "compute");
                results[i] = Some(jobs[*j].program(d.side, program));
            }
        }
    });
    let (target, fresh) = match built {
        Ok(Some(ViewModel { target, fresh, .. })) => (target, fresh),
        Ok(None) => return finish(results),
        Err(failure) => {
            for (i, (j, d)) in on_view.iter().enumerate() {
                results[i].get_or_insert_with(|| {
                    let _s = job_span(*j, jobs[*j], "compute");
                    jobs[*j].view_failed(d.side, &failure)
                });
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
            .map(|(_, (j, d))| {
                let _s = job_span(*j, jobs[*j], "resolve");
                jobs[*j].resolve(d.side, &cx)
            })
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
            inputs: &d.inputs,
            fresh: fresh.as_ref(),
            job: job_id(*j),
            keys,
            has_tree,
            later_cells: later().flat_map(|r| r.0.clone()).collect(),
            later_runs: later().flat_map(|r| r.1.clone()).collect(),
            carry: carries[k].take(),
        };
        let _s = job_span(*j, jobs[*j], "compute");
        results[*i] = Some(jobs[*j].compute(d.side, &mut cx));
    }
    // Freeing the view is a large deallocation: named, so it is not left in an
    // enclosing span's self time (E12-C3, as analyze's `gate.teardown`).
    {
        let _s = pt::span("batch", "view.teardown");
        drop(view);
        drop(fresh);
    }
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
        fn name(&self) -> &'static str {
            "test"
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
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            drive(&[&ja, &jb], &mut |_, _| {})
        }));
        let msg = r.expect_err("the driver refuses the view");
        let msg = msg
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| msg.downcast_ref::<String>().cloned())
            .unwrap_or_default();
        assert_eq!(msg, "a view's demands share one WorkspaceInputs");
    }

    /// A hand-stated job (#277 Task 7): its demands, stated by assignment and
    /// handed to the driver once; `fail` fails that side at `resolve`; each
    /// computed side's document is what `compute` returns. Its document is
    /// every side's result.
    struct Probe {
        demands: std::cell::RefCell<Vec<ViewDemand>>,
        fail: Option<u8>,
        plan: fn() -> Plan,
        reads: Vec<crate::engine::session::Cell>,
        compute: Box<Compute>,
    }

    type Sides = Vec<Result<u8, JobOutput>>;
    type Compute = dyn Fn(SideIx, &mut ViewCx<'_>) -> u8;

    impl Probe {
        fn new(demands: Vec<(ViewKey, &Arc<WorkspaceInputs>)>) -> Probe {
            let demands = demands
                .into_iter()
                .enumerate()
                .map(|(i, (key, inputs))| {
                    let side = SideIx(u8::try_from(i).unwrap());
                    ViewDemand::new(key, side, ViewNeeds::MODEL, Arc::clone(inputs))
                })
                .collect();
            Probe {
                demands: std::cell::RefCell::new(demands),
                fail: None,
                plan: || Plan::merge(std::iter::empty()),
                reads: vec![],
                compute: Box::new(|side, _| side.0),
            }
        }
    }

    impl Job for Probe {
        fn name(&self) -> &'static str {
            "probe"
        }
        fn demands(&self) -> Vec<ViewDemand> {
            std::mem::take(&mut self.demands.borrow_mut())
        }
        fn resolve(&self, side: SideIx, _: &ResolveCx<'_>) -> Result<PlanPart, JobFailure> {
            if self.fail == Some(side.0) {
                return Err(JobOutput::line("side failed", 1));
            }
            Ok(PlanPart {
                plan: (self.plan)(),
                reads: self.reads.clone(),
                carry: None,
            })
        }
        fn compute(&self, side: SideIx, cx: &mut ViewCx<'_>) -> Result<SideDoc, JobFailure> {
            Ok(SideDoc::new((self.compute)(side, cx)))
        }
        fn view_failed(&self, _: SideIx, f: &ViewFailure) -> Result<SideDoc, JobFailure> {
            panic!("the fixture builds: {f:?}")
        }
        fn finalize(&self, sides: Vec<Result<SideDoc, JobFailure>>) -> Doc {
            let sides: Sides = sides.into_iter().map(|s| s.map(SideDoc::take)).collect();
            Doc::new(sides)
        }
        fn serialize(&self, _: Doc) -> JobOutput {
            unreachable!("driven, not serialized")
        }
    }

    /// Every job's sides, in job order.
    fn drive_probes(jobs: &[&dyn Job]) -> Vec<Sides> {
        let mut docs: Vec<Option<Sides>> = jobs.iter().map(|_| None).collect();
        drive(jobs, &mut |j, doc| docs[j] = Some(doc.take()));
        docs.into_iter().map(|d| d.expect("finalized")).collect()
    }

    fn r0(ws: &str) -> Arc<WorkspaceInputs> {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/r0-corpus")
            .join(ws);
        WorkspaceInputs::read(&path, &WorkspaceNeeds::default())
    }

    /// Two single-app views of one workspace: `--no-roots-config` off and on.
    fn two_keys(inputs: &WorkspaceInputs) -> (ViewKey, ViewKey) {
        (
            ViewKey::new(&inputs.workspace, true, false),
            ViewKey::new(&inputs.workspace, true, true),
        )
    }

    /// #277 T21 (the driver half, D8): a job of two sides on two views whose
    /// first side fails, beside a plain job on the second view. The plain job
    /// still computes; the failed job reads no second view, and its second
    /// side holds the first side's failure.
    #[test]
    fn a_failed_side_stands_for_the_later_side() {
        let ws = r0("ws-d1-multi-caller");
        let (k0, k1) = two_keys(&ws);
        let mut two = Probe::new(vec![(k0, &ws), (k1.clone(), &ws)]);
        two.fail = Some(0);
        two.compute =
            Box::new(|side, _| panic!("side {side:?} computed after its old side failed"));
        let plain = Probe::new(vec![(k1, &ws)]);
        let docs = drive_probes(&[&two, &plain]);
        let failed = Err(JobOutput::line("side failed", 1));
        assert_eq!(docs[1], vec![Ok(0)], "the plain job computes");
        assert_eq!(docs[0], vec![failed.clone(), failed]);
    }

    /// #277 E12-C1 (D8's `[diff A B, diff B A]`): views are visited in order of
    /// first demand, so job 1's old side (on B) runs AFTER its new side (on A),
    /// and the job finalizes after B, its last view in visit order, with both
    /// sides.
    #[test]
    fn a_job_finalizes_after_its_last_view_in_visit_order() {
        let (a, b) = (r0("ws-d1-multi-caller"), r0("ws-d14-dead-routine"));
        let key = |ws: &WorkspaceInputs| ViewKey::new(&ws.workspace, true, false);
        let ab = Probe::new(vec![(key(&a), &a), (key(&b), &b)]);
        let ba = Probe::new(vec![(key(&b), &b), (key(&a), &a)]);
        let docs = drive_probes(&[&ab, &ba]);
        assert_eq!(docs, vec![vec![Ok(0), Ok(1)], vec![Ok(0), Ok(1)]]);
    }

    /// #277 E12-C2: the driver drops a demand's workspace read with its view,
    /// so the next view builds without it (standalone `diff A B` holds only
    /// B's read during B's build). The job holds no read of its own.
    #[test]
    fn a_view_drops_its_read_before_the_next_view() {
        let (a, b) = (r0("ws-d1-multi-caller"), r0("ws-d14-dead-routine"));
        let key = |ws: &WorkspaceInputs| ViewKey::new(&ws.workspace, true, false);
        let mut job = Probe::new(vec![(key(&a), &a), (key(&b), &b)]);
        let a_read = Arc::downgrade(&a);
        drop(a);
        job.compute = Box::new(move |side, _| {
            if side.0 == 1 {
                assert!(a_read.upgrade().is_none(), "A's read outlived its view");
            }
            side.0
        });
        assert_eq!(drive_probes(&[&job]), vec![vec![Ok(0), Ok(1)]]);
    }

    /// #277 D6.1, D6.2: on one view, a cell or run an earlier job releases or
    /// takes stays for a later job that declared it. Job 0 takes the run and
    /// releases `l4`; job 1 reads both after it.
    #[test]
    fn a_later_reader_keeps_its_cells_and_runs() {
        use crate::engine::session::Cell;
        let ws = r0("ws-d1-multi-caller");
        let (k, _) = two_keys(&ws);
        let plan = || Plan::for_test(vec![vec![]], true, true, 0);
        let mut first = Probe::new(vec![(k.clone(), &ws)]);
        first.plan = plan;
        first.compute = Box::new(|_, cx| {
            let key = cx.keys()[0].clone();
            drop(cx.take_run(&key));
            cx.release(Cell::L4);
            cx.release_all_but(&[]);
            0
        });
        let mut later = Probe::new(vec![(k, &ws)]);
        later.plan = plan;
        later.reads = vec![Cell::L4];
        later.compute = Box::new(|_, cx| {
            let key = cx.keys()[0].clone();
            drop(cx.take_run(&key));
            let _ = cx.view().l4();
            1
        });
        assert_eq!(
            drive_probes(&[&first, &later]),
            vec![vec![Ok(0)], vec![Ok(1)]]
        );
    }
}

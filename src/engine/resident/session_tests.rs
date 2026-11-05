//! T-fresh, T-manifest, T-acquire-during/after, T-failed-build (design §5,
//! §11): a resident session over a temporary copy of a fixture with
//! packages, cross-app (the stage-4 workspace session refreshes it).

use std::sync::atomic::AtomicUsize;

use super::*;
use crate::engine::batch::output::JobOutput;
use crate::engine::resident::manifest::tests::copy_fixture;
use crate::engine::resident::store::Control;

const ROUTINE: &str = "CallSymbolOnlyDep";

fn session(ws: &Path) -> Arc<ResidentSession> {
    Arc::new(ResidentSession::new(ws.to_path_buf(), false, "test".into()))
}

fn never() -> Option<Stop> {
    None
}

fn revision(p: &Pinned) -> &Arc<Revision> {
    match &p.built {
        Built::Revision(r) => r,
        Built::Graph(_) => panic!("a model session publishes a model revision"),
        Built::Failed(f) => panic!("the build failed: {:?}", f.failure),
    }
}

/// The query answered off the pinned revision.
fn warm(p: &Pinned) -> JobOutput {
    let rev = revision(p);
    let job = crate::engine::l4::effect_query_cli::QueryJob::effects(
        &rev.inputs,
        ROUTINE,
        "test",
        true,
        false,
    );
    rev.answer(&job, &Control::unreserved()).unwrap()
}

/// The query's cold standalone run over a fresh read of the disk.
fn cold(ws: &Path) -> JobOutput {
    let inputs = WorkspaceInputs::read(ws, &REVISION_READ);
    let job = crate::engine::l4::effect_query_cli::QueryJob::effects(
        &inputs, ROUTINE, "test", true, false,
    );
    crate::engine::batch::standalone_output(&job)
}

fn edit(ws: &Path, from: &str, to: &str) {
    let p = ws.join("Caller.al");
    let t = std::fs::read_to_string(&p).unwrap();
    assert!(t.contains(from), "precondition: {from} in Caller.al");
    std::fs::write(&p, t.replace(from, to)).unwrap();
}

/// T-fresh / T-manifest: no edit between asks → the same revision, no
/// rebuild; a body edit → a refreshed revision (generation 2, class B)
/// whose answer equals a cold run over the edited disk.
///
/// DISCRIMINATION (2026-11-02): skipping the freshness check (`if true ||
/// observed == p.manifest`) fails "the edit is seen" (generation stays 1);
/// the same break fails the latest-built test ("stale.stale") and the
/// verify test ("verify sees it").
#[test]
fn no_edit_reuses_the_revision_and_an_edit_refreshes_it() {
    let dir = copy_fixture("tests/fixtures/lsp-diff-deps");
    let ws = dir.path();
    let s = session(ws);
    let first = s.pin(Freshness::Fresh, &never).unwrap();
    let r1 = Arc::clone(revision(&first));
    assert_eq!(r1.id.generation, 1);
    assert!(r1.id.inputs_stamp.starts_with("al-sem-manifest-v1:"));
    let a1 = warm(&first);
    assert_eq!(a1, cold(ws));
    assert!(
        a1.exit == 0,
        "precondition: the query answers ({:?})",
        a1.message()
    );
    let again = s.pin(Freshness::Fresh, &never).unwrap();
    assert!(
        Arc::ptr_eq(revision(&again), &r1),
        "no edit: the same revision"
    );
    assert_eq!(s.stats().builds, 1);
    assert_eq!(s.stats().fresh_hits, 1);
    edit(ws, "Compute(5)", "Compute(7)");
    let after = s.pin(Freshness::Fresh, &never).unwrap();
    let r2 = revision(&after);
    assert_eq!(r2.id.generation, 2, "the edit is seen");
    assert_ne!(r2.id.inputs_stamp, r1.id.inputs_stamp);
    assert_eq!(s.stats().last_build_class.as_deref(), Some("B"));
    let a2 = warm(&after);
    assert_eq!(a2, cold(ws), "the refreshed answer equals cold");
    // The old revision is intact for a request that still holds it.
    assert_eq!(warm(&first), a1);
}

/// `latest-built`: the published revision at once, `stale` with the changed
/// path; a background refresh then publishes the new revision.
#[test]
fn latest_built_answers_stale_and_refreshes_in_the_background() {
    let dir = copy_fixture("tests/fixtures/lsp-diff-deps");
    let ws = dir.path();
    let s = session(ws);
    let first = s.pin(Freshness::Fresh, &never).unwrap();
    let r1 = Arc::clone(revision(&first));
    edit(ws, "Compute(5)", "Compute(8)");
    let stale = s.pin(Freshness::LatestBuilt, &never).unwrap();
    assert!(stale.stale);
    assert!(Arc::ptr_eq(revision(&stale), &r1));
    let caller = ws.join("Caller.al").to_string_lossy().into_owned();
    assert_eq!(stale.changed_paths, vec![caller]);
    let start = Instant::now();
    while s
        .published()
        .and_then(|b| b.revision_id())
        .map(|r| r.generation)
        != Some(2)
    {
        assert!(
            start.elapsed().as_secs() < 120,
            "the background refresh published"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let now = s.pin(Freshness::LatestBuilt, &never).unwrap();
    assert!(!now.stale);
    assert_eq!(warm(&now), cold(ws));
}

/// A rewrite that keeps length and mtime: `fresh` misses it (the stated
/// residual, §5.3); `verify` hashes every input and refreshes; a change
/// notification on the path makes `fresh` re-hash it too.
#[test]
fn verify_and_notify_changed_see_a_rewrite_that_keeps_its_stat() {
    let dir = copy_fixture("tests/fixtures/lsp-diff-deps");
    let ws = dir.path();
    let p = ws.join("Caller.al");
    let s = session(ws);
    let first = s.pin(Freshness::Fresh, &never).unwrap();
    let r1 = Arc::clone(revision(&first));
    let mtime = std::fs::metadata(&p).unwrap().modified().unwrap();
    let same_stat = |from: &str, to: &str| {
        assert_eq!(from.len(), to.len(), "precondition: the length is kept");
        edit(ws, from, to);
        std::fs::File::options()
            .write(true)
            .open(&p)
            .unwrap()
            .set_modified(mtime)
            .unwrap();
    };
    same_stat("Compute(5)", "Compute(9)");
    let missed = s.pin(Freshness::Fresh, &never).unwrap();
    assert!(
        Arc::ptr_eq(revision(&missed), &r1),
        "precondition: fresh misses it"
    );
    let verified = s.pin(Freshness::Verify, &never).unwrap();
    assert_eq!(revision(&verified).id.generation, 2, "verify sees it");
    assert_eq!(warm(&verified), cold(ws));
    same_stat("Compute(9)", "Compute(4)");
    let missed = s.pin(Freshness::Fresh, &never).unwrap();
    assert_eq!(
        revision(&missed).id.generation,
        2,
        "precondition: fresh misses it"
    );
    s.notify_changed([p.clone()]);
    let notified = s.pin(Freshness::Fresh, &never).unwrap();
    assert_eq!(
        revision(&notified).id.generation,
        3,
        "the notification is seen"
    );
    assert_eq!(warm(&notified), cold(ws));
}

/// T-acquire-during: the read consumes B while the disk holds A before and
/// after (an overwrite and restore during acquisition): the build is
/// discarded and retried, and the published revision is A's.
///
/// DISCRIMINATION (2026-11-02): comparing only before and after
/// (`check_consistent(&m_before, &m_before, &m_after)`) publishes B's build:
/// "the first attempt is discarded" fails (0 discards).
#[test]
fn a_read_during_a_rewrite_is_discarded_and_retried() {
    let dir = copy_fixture("tests/fixtures/lsp-diff-deps");
    let ws = dir.path().to_path_buf();
    let p = ws.join("Caller.al");
    let a = std::fs::read_to_string(&p).unwrap();
    let b = a.replace("Compute(5)", "Compute(6)");
    let cold_a = cold(&ws);
    let (pa, pb) = (p.clone(), b.clone());
    let hook: BuildHook = Box::new(move |point, attempt| match (point, attempt) {
        (BuildPoint::BeforeRead, 0) => std::fs::write(&pa, &pb).unwrap(),
        (BuildPoint::AfterRead, 0) => std::fs::write(&pa, &a).unwrap(),
        _ => {}
    });
    let s = Arc::new(ResidentSession::new(ws.clone(), false, "test".into()).with_hook(hook));
    let pinned = s.pin(Freshness::Fresh, &never).unwrap();
    assert_eq!(
        s.stats().discarded_builds,
        1,
        "the first attempt is discarded"
    );
    assert_eq!(warm(&pinned), cold_a, "the published answer is A's");
}

/// T-acquire-after: a package acquired as A and overwritten after the read
/// (restored before `M_after`): the build consumes the owned image A, so it
/// publishes A's answer and is not discarded.
#[test]
fn a_package_overwritten_after_its_acquisition_is_not_consumed() {
    let dir = copy_fixture("tests/fixtures/lsp-diff-deps");
    let ws = dir.path().to_path_buf();
    let pkg = ws.join(".alpackages/aaaaaaaa-0000-0000-0000-000000000001.app");
    let a = std::fs::read(&pkg).unwrap();
    let cold_a = cold(&ws);
    let (pk, bytes) = (pkg.clone(), a.clone());
    let hook: BuildHook = Box::new(move |point, attempt| match (point, attempt) {
        (BuildPoint::AfterRead, 0) => std::fs::write(&pk, b"not a package").unwrap(),
        (BuildPoint::AfterBuild, 0) => std::fs::write(&pk, &bytes).unwrap(),
        _ => {}
    });
    let s = Arc::new(ResidentSession::new(ws.clone(), false, "test".into()).with_hook(hook));
    let pinned = s.pin(Freshness::Fresh, &never).unwrap();
    assert_eq!(s.stats().discarded_builds, 0);
    assert_eq!(warm(&pinned), cold_a);
}

/// Inputs that move during every attempt: `inputsUnstable` after the bound.
#[test]
fn inputs_moving_on_every_attempt_are_unstable() {
    let dir = copy_fixture("tests/fixtures/lsp-diff-deps");
    let ws = dir.path().to_path_buf();
    let p = ws.join("Caller.al");
    let n = Arc::new(AtomicUsize::new(0));
    let (pp, nn) = (p.clone(), Arc::clone(&n));
    let hook: BuildHook = Box::new(move |point, _| {
        if point == BuildPoint::AfterRead {
            let i = nn.fetch_add(1, Ordering::SeqCst);
            let t = std::fs::read_to_string(&pp).unwrap();
            std::fs::write(&pp, format!("{t}// {i}\n")).unwrap();
        }
    });
    let s = Arc::new(ResidentSession::new(ws, false, "test".into()).with_hook(hook));
    let err = s.pin(Freshness::Fresh, &never).err().unwrap();
    assert!(
        matches!(&err, PinError::Unstable(m) if m.contains("al:Caller.al") && m.contains("3 attempts")),
        "{err:?}"
    );
    assert_eq!(n.load(Ordering::SeqCst), BUILD_ATTEMPTS);
    assert!(s.published().is_none(), "nothing published");
}

/// T-failed-build: a workspace whose model does not build (no `app.json`
/// id) publishes its failure, `revision: null`; the failure keeps its read.
#[test]
fn a_failed_build_is_published_with_no_revision() {
    let dir = copy_fixture("tests/fixtures/lsp-diff-deps");
    let ws = dir.path();
    let p = ws.join("app.json");
    let t = std::fs::read_to_string(&p).unwrap();
    std::fs::write(&p, t.replace("\"id\"", "\"notId\"")).unwrap();
    let s = session(ws);
    let pinned = s.pin(Freshness::Fresh, &never).unwrap();
    let Built::Failed(f) = &pinned.built else {
        panic!("precondition: the build fails");
    };
    assert!(matches!(f.failure.target, TargetFailure::NoModelInstanceId));
    assert!(pinned.built.revision_id().is_none());
    assert_eq!(s.stats().builds, 1);
}

/// `--low-mem` (design §7) through the resident session: the query of a
/// routine calling into a source-bearing dependency, answered off each
/// revision, equals a cold run after the first build, a body edit (B) and a
/// declaration edit (S, every dependency file parsed again), in both modes.
#[test]
fn low_mem_answers_equal_cold_after_b_and_s() {
    fn query(inputs: &Arc<WorkspaceInputs>) -> crate::engine::l4::effect_query_cli::QueryJob<'_> {
        crate::engine::l4::effect_query_cli::QueryJob::effects(
            inputs,
            "CallEmbeddedSourceDep",
            "test",
            true,
            false,
        )
    }
    for low_mem in [false, true] {
        let dir = copy_fixture("tests/fixtures/lsp-diff-deps");
        let ws = dir.path();
        let s = Arc::new(
            ResidentSession::new(ws.to_path_buf(), false, "test".into()).with_low_mem(low_mem),
        );
        assert_eq!(s.low_mem(), low_mem);
        let check = |what: &str| {
            let p = s.pin(Freshness::Fresh, &never).unwrap();
            let rev = revision(&p);
            let warm = rev
                .answer(&query(&rev.inputs), &Control::unreserved())
                .unwrap();
            let inputs = WorkspaceInputs::read(ws, &REVISION_READ);
            let cold = crate::engine::batch::standalone_output(&query(&inputs));
            assert!(
                warm.exit == 0,
                "precondition: {what} answers ({:?})",
                warm.message()
            );
            assert_eq!(warm, cold, "low_mem={low_mem} {what}: warm equals cold");
            s.last_refresh()
        };
        check("first build");
        edit(ws, "DoWork(3)", "DoWork(4)");
        let r = check("body edit").expect("a refresh report");
        assert_eq!(r.class, ChangeClass::BodyOnly);
        assert_eq!(r.dependency_files_parsed, 0);
        edit(
            ws,
            "    procedure CallEmbeddedSourceDep()",
            "    procedure Added()\n    begin\n    end;\n\n    procedure CallEmbeddedSourceDep()",
        );
        let r = check("declaration edit").expect("a refresh report");
        assert_eq!(r.class, ChangeClass::Declaration);
        // No dependency file read the workspace codeunit (the finer S rule,
        // `program::session::dep_record`): none is resolved or parsed again.
        assert!(r.dependency_rule);
        assert_eq!(r.dependency_files_resolved, 0);
        assert_eq!(r.dependency_files_parsed, 0);
    }
}

/// A request that stops before its build begins builds nothing.
#[test]
fn a_stopped_request_builds_nothing() {
    let dir = copy_fixture("tests/fixtures/lsp-diff-deps");
    let s = session(dir.path());
    let err = s
        .pin(Freshness::Fresh, &|| Some(Stop::Deadline))
        .err()
        .unwrap();
    assert_eq!(err, PinError::Stopped(Stop::Deadline));
    assert_eq!(s.stats().builds, 0);
}

mod budget {
    //! T-budget (design §4.5, §7, §11): reserve-or-fail, the impossible
    //! budget, the eviction order, a workload refused before anything is
    //! read, and recovery after a size-reducing edit.

    use super::*;
    use crate::engine::resident::budget::{
        Budget, Case, Estimates, FakeMeter, InputSizes, MIB, Meter,
    };
    use crate::engine::resident::store::Guard;
    use std::sync::atomic::Ordering;
    use std::sync::{OnceLock, Weak};

    /// One artifact's resident cost under [`FillMeter`].
    const UNIT: u64 = 1_000;

    /// A meter whose RSS is `base` plus [`UNIT`] per artifact the published
    /// revision filled and did not evict: a fill leaves `UNIT` resident and
    /// an eviction frees it, as the real allocator roughly does.
    struct FillMeter {
        base: u64,
        session: OnceLock<Weak<ResidentSession>>,
    }

    impl Meter for FillMeter {
        fn rss(&self) -> u64 {
            let Some(s) = self.session.get().and_then(Weak::upgrade) else {
                return self.base;
            };
            match s.published() {
                Some(Built::Revision(r)) => {
                    let fills: u64 = r.fill_counts().values().sum();
                    self.base + UNIT * fills.saturating_sub(r.stats().evictions)
                }
                _ => self.base,
            }
        }
        fn peak(&self) -> u64 {
            self.rss()
        }
        fn reset_peak(&self) {}
    }

    /// Every case observed at `est`, so the estimates are the test's.
    fn small_estimates(single_app: bool, est: u64) -> Estimates {
        let mut e = Estimates::new(single_app);
        for case in [
            Case::Cold,
            Case::Incremental,
            Case::Trees,
            Case::Fill("fold"),
            Case::Fill("l4"),
            Case::Fill("det"),
            Case::Fill("run"),
            Case::Fill("raw"),
            Case::Fill("rules"),
            Case::Fill("ordering"),
            Case::Fill("explain_indexes"),
        ] {
            e.observe(case, est);
        }
        e
    }

    /// `job` answered off `rev` the way the child answers it: every fill
    /// and the trees reserve through the session.
    fn answer_budgeted(
        s: &ResidentSession,
        rev: &Revision,
        job: &dyn crate::engine::batch::job::Job,
    ) -> Result<JobOutput, crate::engine::resident::store::PrepareError> {
        let budget = s.budget().unwrap();
        let reserve = |kind: &'static str, _: usize| -> Result<Guard<'_>, String> {
            let case = if kind == "trees" {
                Case::Trees
            } else {
                Case::Fill(kind)
            };
            let estimate = budget.estimate(case, InputSizes::default());
            s.reserve_or_reclaim(budget, case, estimate, 1, &|| false)
                .map(|r| Box::new(r) as Guard<'_>)
                .map_err(|f| f.message())
        };
        rev.answer(
            job,
            &Control {
                stop: &|| None,
                reserve: &reserve,
            },
        )
    }

    fn prove<'a>(rev: &'a Revision) -> crate::engine::l5::prove::ProveJob<'a> {
        crate::engine::l5::prove::ProveJob {
            detectors: crate::engine::l5::detectors::default_detectors(),
            inputs: &rev.inputs,
            routine_selector: "ForwardedAfterValidate",
            question_text: "may-commit",
            alsem_ver: "test",
            deterministic: true,
            single_app: true,
            closed_world: false,
            assume_no_dependents: false,
            human: false,
            detail: false,
            out: None,
        }
    }

    fn explain(rev: &Revision) -> crate::engine::l5::explain::ExplainJob<'_> {
        use crate::engine::l5::explain::{ExplainJob, ExplainMode, ExplainRequest};
        ExplainJob::new(
            ExplainRequest {
                inputs: &rev.inputs,
                mode: ExplainMode::Routines(vec!["ForwardedAfterValidate".into()]),
                single_app: true,
            },
            false,
            None,
        )
    }

    /// Reserve-or-fail with real artifacts: a fill that does not fit
    /// evicts in the design's order (raw, rows, run, ...) only until it
    /// fits, never the L4 seed, and its answer is the cold one; a fill that
    /// cannot fit even above the retained floor fails at once with
    /// `budget`, evicting nothing.
    ///
    /// DISCRIMINATION (2026-11-02): reclaiming L4 first (`"l4"` at the head
    /// of `EVICTION_ORDER`) fails "L4 is never evicted" (the snapshot after
    /// explain is `[fold, det, run]`); dropping the impossibility check in
    /// `reserve_or_reclaim` (`Err(f) if false`) fails the last assertion
    /// (`impossible` 2, not 1: the refusal went through a needless reclaim
    /// and was refused again instead of failing at once).
    #[test]
    fn a_fill_that_does_not_fit_evicts_in_order_and_an_impossible_one_fails_at_once() {
        let dir = copy_fixture("tests/r0-corpus/ws-d37");
        let meter = Arc::new(FillMeter {
            base: 100 * UNIT,
            session: OnceLock::new(),
        });
        let budget = Arc::new(Budget::new(
            u64::MAX / 4,
            meter.clone(),
            small_estimates(true, 10),
        ));
        let s = Arc::new(
            ResidentSession::new(dir.path().to_path_buf(), true, "test".into())
                .with_budget(Arc::clone(&budget)),
        );
        let _ = meter.session.set(Arc::downgrade(&s));
        let pinned = s.pin(Freshness::Fresh, &never).unwrap();
        let rev = revision(&pinned);
        let cold_prove = crate::engine::batch::standalone_output(&prove(rev));
        let cold_explain = crate::engine::batch::standalone_output(&explain(rev));
        assert_eq!(answer_budgeted(&s, rev, &prove(rev)).unwrap(), cold_prove);
        let kinds =
            |r: &Revision| -> Vec<&'static str> { r.snapshot().keys().map(|k| k.kind()).collect() };
        assert!(kinds(rev).contains(&"run") && kinds(rev).contains(&"l4"));
        // Explain's indexes (plan 2026-11-03 W2) prepared up front, so the
        // explain below reserves once (its trees), as the test states.
        rev.prepare_keys(
            &[crate::engine::resident::revision::ArtifactKey::ExplainIndexes],
            &Control::unreserved(),
        )
        .unwrap();
        // Precondition by assignment: nothing more fits now.
        let rss = meter.rss();
        budget.set_budget(rss);
        assert_eq!(
            answer_budgeted(&s, rev, &explain(rev)).unwrap(),
            cold_explain
        );
        let after = kinds(rev);
        assert!(after.contains(&"l4"), "L4 is never evicted: {after:?}");
        assert!(!after.contains(&"run"), "the run went first: {after:?}");
        assert!(
            after.contains(&"det"),
            "and nothing more than needed: {after:?}"
        );
        let st = budget.status();
        assert_eq!((st.reclaims, st.evicted), (1, 1), "{st:?}");
        assert!(!st.admission_closed, "admission reopened");
        // Impossible: the floor (the L4 seed and the base) plus the estimate
        // exceeds the budget whatever is evicted.
        s.update_floor();
        budget.set_budget(budget.retained_floor() + 5);
        let evictions = rev.stats().evictions;
        let err = answer_budgeted(&s, rev, &prove(rev)).unwrap_err();
        assert!(
            matches!(&err, crate::engine::resident::store::PrepareError::Budget(m)
                if m.contains("cannot fit above the retained floor")),
            "{err:?}"
        );
        assert_eq!(
            rev.stats().evictions,
            evictions,
            "an impossible fill evicts nothing"
        );
        assert_eq!(budget.status().impossible, 1);
    }

    /// A workload whose pre-model estimate exceeds the budget is refused
    /// before anything is read (no acquisition, no model); a size-reducing
    /// edit lowers the estimate and the next request builds.
    ///
    /// DISCRIMINATION (2026-11-02): estimating the build from no sizes
    /// instead of the observed manifest's (`budget.estimate(case,
    /// Default::default())` in `build_locked`) fails "refused": the padded
    /// workspace is built (and read) in full.
    #[test]
    fn a_workload_over_the_budget_is_refused_before_reading_and_recovers_after_an_edit() {
        let dir = copy_fixture("tests/r0-corpus/ws-d37");
        let ws = dir.path();
        let pad = ws.join("src").join("Padding.al");
        // 200 KiB of comment: ~200 MiB at 1,000 bytes per `.al` byte.
        std::fs::write(&pad, "// padding\n".repeat(200 * 1024 / 11)).unwrap();
        let mut e = Estimates::new(true);
        e.per_al_byte = 1_000;
        e.floor = 0;
        let meter = Arc::new(FakeMeter::default());
        let budget = Arc::new(Budget::new(100 * MIB, meter, e));
        let reads = Arc::new(AtomicUsize::new(0));
        let r = Arc::clone(&reads);
        let s = Arc::new(
            ResidentSession::new(ws.to_path_buf(), true, "test".into())
                .with_budget(Arc::clone(&budget))
                .with_hook(Box::new(move |point, _| {
                    if point == BuildPoint::BeforeRead {
                        r.fetch_add(1, Ordering::SeqCst);
                    }
                })),
        );
        let err = s.pin(Freshness::Fresh, &never).err().expect("refused");
        let PinError::Budget(refusal) = err else {
            panic!("a budget refusal, got {err:?}")
        };
        assert!(
            refusal.impossible && refusal.case == Case::Cold,
            "{refusal:?}"
        );
        assert!(refusal.estimate > 100 * MIB);
        assert_eq!(reads.load(Ordering::SeqCst), 0, "refused before the read");
        assert!(s.published().is_none());
        std::fs::remove_file(&pad).unwrap();
        let pinned = s
            .pin(Freshness::Fresh, &never)
            .expect("recovery after the edit");
        assert!(pinned.built.revision_id().is_some());
        assert_eq!(reads.load(Ordering::SeqCst), 1);
    }
}

/// The sited graph of a cold cross-app build over the disk now.
fn sited_cold(ws: &Path) -> String {
    let inputs = WorkspaceInputs::read(ws, &REVISION_READ);
    let p = crate::program::resolved::ResolvedProgram::build(
        &inputs,
        crate::program::resolved::ProgramScope::CrossApp,
    )
    .expect("the fixture builds");
    crate::program::impact_graph::sited_from_program(&p, &|| false)
        .expect("not cancelled")
        .text()
}

/// M0-T3 (agent-tasks design §1.3), in the given memory mode: a revision
/// builds no sited graph until a question reads it; that question adds it
/// to the published revision (the same revision, no second read); from then
/// on every revision builds it, and after a body edit that adds a call the
/// new revision's graph has the new site and equals a cold build's.
///
/// DISCRIMINATION (2026-11-05): seeding the new revision with the previous
/// revision's graph (`rev.with_sited(prev's Arc)` in `build_locked`) fails
/// "the new call's site" in both modes.
fn sited_graph_follows_a_body_edit(low_mem: bool) {
    let dir = copy_fixture("tests/fixtures/lsp-diff-deps");
    let ws = dir.path();
    let s = Arc::new(
        ResidentSession::new(ws.to_path_buf(), false, "test".into()).with_low_mem(low_mem),
    );
    let first = s.pin(Freshness::Fresh, &never).unwrap();
    assert!(revision(&first).sited().is_none(), "built only when asked");
    let p = s.pin_sited(Freshness::Fresh, &never).unwrap();
    assert_eq!(p.rebuilt_for, Some("sited"));
    assert_eq!(revision(&p).id.generation, 1, "added to the revision");
    assert_eq!(s.stats().builds, 1, "no second build");
    let g = revision(&p).sited().expect("the addition made it");
    assert_eq!(g.text(), sited_cold(ws));
    let ends = |g: &crate::program::impact_graph::SitedGraph| {
        (
            g.routines_named("callsymbolonlydep")[0],
            g.routines_named("callembeddedsourcedep")[0],
        )
    };
    let (c, e) = ends(&g);
    assert!(
        g.sites_between(c, e).is_empty(),
        "precondition: no call yet"
    );
    edit(
        ws,
        "        \"Widget Mgt\".Compute(5);",
        "        \"Widget Mgt\".Compute(5);\n        CallEmbeddedSourceDep();",
    );
    let p2 = s.pin_sited(Freshness::Fresh, &never).unwrap();
    assert_eq!(p2.rebuilt_for, None, "every revision builds it now");
    assert_eq!(s.stats().last_build_class.as_deref(), Some("B"));
    let g2 = revision(&p2).sited().expect("built with the refresh");
    let (c2, e2) = ends(&g2);
    assert_eq!(g2.sites_between(c2, e2).len(), 1, "the new call's site");
    assert_eq!(g2.text(), sited_cold(ws));
    assert_eq!(s.stats().sited_builds, 2);
    // A sited-graph job answered off the model revision's artifact.
    let r2 = revision(&p2);
    let warm = r2
        .answer(&SitedTextJob { inputs: &r2.inputs }, &Control::unreserved())
        .unwrap();
    let cold_inputs = WorkspaceInputs::read(ws, &REVISION_READ);
    let cold = crate::engine::batch::standalone_output(&SitedTextJob {
        inputs: &cold_inputs,
    });
    assert_eq!(warm, cold);
}

#[test]
fn sited_graph_follows_a_body_edit_default_mode() {
    sited_graph_follows_a_body_edit(false);
}

#[test]
fn sited_graph_follows_a_body_edit_low_mem() {
    sited_graph_follows_a_body_edit(true);
}

/// M0-T8 (agent-tasks design §1.3): a request cancelled while the graph it
/// asked for is built ends that graph's build and is answered `stopped`; the
/// revision stays without it, and the next question builds it.
///
/// DISCRIMINATION (2026-11-05): ignoring the cancel (`build_sited(&p, &||
/// false)` in `add_sited_locked`) fails "the request stopped".
#[test]
fn a_cancelled_sited_build_publishes_no_graph() {
    let dir = copy_fixture("tests/fixtures/lsp-diff-deps");
    let ws = dir.path();
    let stop_now = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let f = Arc::clone(&stop_now);
    let s = Arc::new(
        ResidentSession::new(ws.to_path_buf(), false, "test".into()).with_hook(Box::new(
            move |point, _| {
                // Cancelled once the graph's program is next.
                if point == BuildPoint::BeforeSited {
                    f.store(true, Ordering::SeqCst);
                }
            },
        )),
    );
    let first = s.pin(Freshness::Fresh, &never).unwrap();
    let stop = || stop_now.load(Ordering::SeqCst).then_some(Stop::Cancelled);
    assert!(
        matches!(
            s.pin_sited(Freshness::Fresh, &stop),
            Err(PinError::Stopped(Stop::Cancelled))
        ),
        "the request stopped"
    );
    match s.published() {
        Some(Built::Revision(now)) => assert!(Arc::ptr_eq(revision(&first), &now), "no rebuild"),
        _ => panic!("a revision stays published"),
    }
    assert!(revision(&first).sited().is_none(), "no graph published");
    assert_eq!(s.stats().sited_builds, 0);
    stop_now.store(false, Ordering::SeqCst);
    let p = s.pin_sited(Freshness::Fresh, &never).unwrap();
    assert_eq!(p.rebuilt_for, Some("sited"));
    assert!(revision(&p).sited().is_some());
}

/// The revision a request asking for the sited graph is answered from keeps
/// every artifact already filled (CDO: the first impact or tests call made
/// the next digest fill 9-13 s again): the graph is added to the published
/// revision, built from a program over that revision's read, never by a
/// rebuild that publishes a new revision with an empty store. The store's
/// fold stays published and is not filled again, no workspace read is
/// made, and the graph equals a cold build's.
///
/// DISCRIMINATION (2026-11-05): `pin_sited` rebuilding as first built (the
/// `add_sited_locked` branch removed) fails "the same revision answers".
#[test]
fn the_sited_graph_is_added_to_the_published_revision_keeping_its_artifacts() {
    let dir = copy_fixture("tests/fixtures/lsp-diff-deps");
    let ws = dir.path();
    let s = Arc::new(ResidentSession::new(ws.to_path_buf(), false, "test".into()));
    let first = s.pin(Freshness::Fresh, &never).unwrap();
    let rev = Arc::clone(revision(&first));
    use crate::engine::resident::revision::ArtifactKey;
    rev.prepare_keys(&[ArtifactKey::Fold], &Control::unreserved())
        .expect("the fold fills");
    let fills = rev.stats().fills;
    assert!(fills > 0, "precondition: the fold was filled");
    let p = s.pin_sited(Freshness::Fresh, &never).unwrap();
    assert!(Arc::ptr_eq(revision(&p), &rev), "the same revision answers");
    assert_eq!(p.rebuilt_for, Some("sited"));
    assert_eq!(s.stats().builds, 1, "no workspace read, no build");
    assert_eq!(s.stats().sited_builds, 1);
    assert!(
        rev.snapshot().get(&ArtifactKey::Fold).is_some(),
        "the fold is kept"
    );
    rev.prepare_keys(&[ArtifactKey::Fold], &Control::unreserved())
        .unwrap();
    assert_eq!(rev.stats().fills, fills, "nothing is filled again");
    assert_eq!(rev.sited().expect("added").text(), sited_cold(ws));
    // The next ask finds it.
    let again = s.pin_sited(Freshness::Fresh, &never).unwrap();
    assert_eq!(again.rebuilt_for, None);
    assert!(Arc::ptr_eq(revision(&again), &rev));
}

/// A job reading the sited graph alone: it prints the graph's text.
struct SitedTextJob<'a> {
    inputs: &'a Arc<WorkspaceInputs>,
}

impl crate::engine::batch::job::Job for SitedTextJob<'_> {
    fn name(&self) -> &'static str {
        "sited-text"
    }
    fn demands(&self) -> Vec<crate::engine::batch::job::ViewDemand> {
        crate::engine::batch::job::ViewDemand::one(self.inputs, false, false, ViewNeeds::SITED)
    }
    fn resolve(
        &self,
        _: crate::engine::batch::job::SideIx,
        _: &crate::engine::batch::job::ResolveCx<'_>,
    ) -> Result<crate::engine::batch::job::PlanPart, crate::engine::batch::job::JobFailure> {
        unreachable!("reads only the sited graph")
    }
    fn compute(
        &self,
        _: crate::engine::batch::job::SideIx,
        _: &mut crate::engine::batch::job::ViewCx<'_>,
    ) -> Result<crate::engine::batch::job::SideDoc, crate::engine::batch::job::JobFailure> {
        unreachable!("reads only the sited graph")
    }
    fn view_failed(
        &self,
        _: crate::engine::batch::job::SideIx,
        _: &ViewFailure,
    ) -> Result<crate::engine::batch::job::SideDoc, crate::engine::batch::job::JobFailure> {
        unreachable!("done off the program")
    }
    fn sited(
        &self,
        _: crate::engine::batch::job::SideIx,
        g: Result<&crate::program::impact_graph::SitedGraph, &str>,
    ) -> Result<crate::engine::batch::job::SideDoc, crate::engine::batch::job::JobFailure> {
        Ok(crate::engine::batch::job::SideDoc::new(
            g.map(|g| g.text()).map_err(str::to_string),
        ))
    }
    fn serialize(&self, doc: crate::engine::batch::job::Doc) -> JobOutput {
        match doc.one::<Result<String, String>>().expect("one side") {
            Ok(t) => JobOutput::lines(&[t], 0),
            Err(e) => JobOutput::lines(&[e], 1),
        }
    }
}

/// M0-T4 (agent-tasks design §1.3): a graph-only session publishes a graph
/// revision; it answers a sited-graph job exactly as the one-shot driver
/// does (the same text, built from a cold program), refuses a model job with
/// the reason, and follows a body edit.
///
/// DISCRIMINATION (2026-11-05): letting the model job fall through to the
/// one-shot path (`return Ok(crate::engine::batch::standalone_output(job))`
/// in `GraphRevision::answer`) fails "a model job is refused".
#[test]
fn a_graph_only_session_answers_sited_jobs_and_refuses_model_jobs() {
    let dir = copy_fixture("tests/fixtures/lsp-diff-deps");
    let ws = dir.path();
    let s = Arc::new(ResidentSession::graph_only(ws.to_path_buf(), "test".into()));
    let graph = |p: &Pinned| match &p.built {
        Built::Graph(g) => Arc::clone(g),
        _ => panic!("a graph-only session publishes a graph revision"),
    };
    let p = s.pin(Freshness::Fresh, &never).unwrap();
    let g = graph(&p);
    let warm = g.answer(&SitedTextJob { inputs: &g.inputs }).unwrap();
    let cold_inputs = WorkspaceInputs::read(ws, &REVISION_READ);
    let cold = crate::engine::batch::standalone_output(&SitedTextJob {
        inputs: &cold_inputs,
    });
    assert_eq!(warm, cold, "the one-shot driver's answer");
    assert_eq!(warm.exit, 0);
    let model_job = crate::engine::l4::effect_query_cli::QueryJob::effects(
        &g.inputs, ROUTINE, "test", true, false,
    );
    match g.answer(&model_job) {
        Err(crate::engine::resident::store::PrepareError::Engine(m)) => {
            assert!(
                m.contains("graph-only") && m.contains("needs a model"),
                "{m}"
            );
        }
        Err(e) => panic!("a model job is refused, with the reason: {e:?}"),
        Ok(_) => panic!("a model job is refused"),
    }
    edit(
        ws,
        "        \"Widget Mgt\".Compute(5);",
        "        \"Widget Mgt\".Compute(5);\n        CallEmbeddedSourceDep();",
    );
    let p2 = s.pin(Freshness::Fresh, &never).unwrap();
    let g2 = graph(&p2);
    assert_eq!(s.descriptor().map(|d| d.universe), Some("graphOnly".into()));
    assert_eq!(g2.id.generation, 2);
    assert_eq!(s.stats().last_build_class.as_deref(), Some("B"));
    assert_eq!(g2.sited.text(), sited_cold(ws));
}

/// M0-T5 (agent-tasks design §1.3): a site's guards are read from the bytes
/// the revision was built from. A call under `case` and `if` is built; the
/// condition is then edited on disk; the guards still name the revision's
/// condition, outermost first, and the condition of the `if` (not its then
/// branch) holds no guard of its own.
///
/// DISCRIMINATION (2026-11-05): reading the workspace file from disk
/// (`std::fs::read_to_string(&f.abs_path)` in `guards::pinned_texts`) fails
/// "the revision's bytes".
#[test]
fn site_guards_read_the_pinned_bytes() {
    let dir = copy_fixture("tests/fixtures/lsp-diff-deps");
    let ws = dir.path();
    edit(
        ws,
        "        \"Widget Mgt\".Compute(5);",
        "        \"Widget Mgt\".Compute(5);\n        case 1 + 1 of\n            2:\n                if 3 = 3 then\n                    CallEmbeddedSourceDep();\n        end;",
    );
    let s = Arc::new(ResidentSession::graph_only(ws.to_path_buf(), "test".into()));
    let g = match s.pin(Freshness::Fresh, &never).unwrap().built {
        Built::Graph(g) => g,
        _ => panic!("a graph revision"),
    };
    let from = g.sited.routines_named("callsymbolonlydep")[0];
    let to = g.sited.routines_named("callembeddedsourcedep")[0];
    let sites = g.sited.sites_between(from, to);
    assert_eq!(sites.len(), 1, "precondition: the guarded call's site");
    edit(ws, "if 3 = 3 then", "if 4 = 4 then");
    let guards = g.site_guards(sites[0]).expect("guards");
    let shown: Vec<(&str, &str)> = guards.iter().map(|x| (x.kind, x.text.as_str())).collect();
    assert_eq!(
        shown,
        vec![("case", "1 + 1 = 2"), ("if", "3 = 3")],
        "the revision's bytes"
    );
    // A site with no anchor (a rule's edge) says so.
    let unanchored =
        (0..g.sited.site_count() as u32).find(|i| g.sited.sites()[*i as usize].anchor.is_none());
    if let Some(i) = unanchored {
        assert!(g.site_guards(i).is_err());
    }
}

/// M0-T6 (agent-tasks design §1.3): a session whose rows were built with
/// fewer needs (no rules) rebuilds them for a request that needs rules: one
/// rebuild of class `needs`, every workspace row projected again (none
/// reused), and the new model holds the rules; a second such request
/// reuses the revision.
///
/// DISCRIMINATION (2026-11-05): ignoring the recorded needs in the row
/// cache (dropping `&& self.needs == i.needs` from `CachedFile::matches`)
/// fails `rules_built(&p)` (the reused rows have none); ignoring them in the
/// session (`covered` always true) fails "rebuilt once".
#[test]
fn rows_built_with_fewer_needs_are_rebuilt_for_a_request_that_needs_more() {
    let dir = copy_fixture("tests/fixtures/lsp-diff-deps");
    let ws = dir.path();
    let s = Arc::new(
        ResidentSession::new(ws.to_path_buf(), false, "test".into())
            .with_needs(crate::engine::batch::job::ViewNeeds::MODEL),
    );
    let rules_built = |p: &Pinned| {
        revision(p)
            .target()
            .resolved()
            .workspace
            .routines
            .iter()
            .all(|r| r.rules.is_some())
    };
    let first = s.pin(Freshness::Fresh, &never).unwrap();
    assert!(!rules_built(&first), "precondition: built without rules");
    let explain = crate::program::model::workspace::AssemblyNeeds::EXPLAIN;
    let p = s.pin_needs(Freshness::Fresh, explain, &never).unwrap();
    assert_eq!(p.rebuilt_for, Some("needs"), "rebuilt once");
    assert_eq!(s.stats().needs_rebuilds, 1);
    assert_eq!(s.stats().last_build_class.as_deref(), Some("needs"));
    assert!(rules_built(&p));
    let rows = s.last_refresh().and_then(|r| r.rows).expect("row stats");
    assert!(
        rows.workspace_projected > 0 && rows.workspace_reused == 0,
        "projected again: {rows:?}"
    );
    let again = s.pin_needs(Freshness::Fresh, explain, &never).unwrap();
    assert_eq!(again.rebuilt_for, None);
    assert_eq!(revision(&again).id, revision(&p).id);
    assert_eq!(s.stats().needs_rebuilds, 1);
}

/// M0-6's descriptor (agent-tasks design §1.2): the published state's
/// manifest, with every file entry's stat, round-trips through the
/// descriptor; a cached observation against the rebuilt manifest finds the
/// state current, and one after an edit does not; the descriptor follows
/// the generation.
///
/// DISCRIMINATION (2026-11-05): dropping the stats from
/// `Manifest::descriptor_entries` fails "stats kept".
#[test]
fn the_input_descriptor_round_trips_the_published_manifest() {
    let dir = copy_fixture("tests/fixtures/lsp-diff-deps");
    let ws = dir.path();
    let s = session(ws);
    assert!(s.descriptor().is_none(), "nothing published yet");
    s.pin(Freshness::Fresh, &never).unwrap();
    let d = s.descriptor().expect("published");
    assert_eq!(d.generation, Some(1));
    assert_eq!(Some(d.inputs_stamp.clone()), s.published_stamp());
    assert_eq!(d.universe, "crossApp");
    let m = Manifest::from_descriptor_entries(&d.manifest).expect("round-trips");
    assert_eq!(m.descriptor_entries(), d.manifest, "stats kept");
    assert!(
        d.manifest.iter().any(|e| e.stat_len.is_some()),
        "stats kept"
    );
    let none = BTreeSet::new();
    let cached =
        |prev: &Manifest| Manifest::observe(ws, false, Hashing::Cached { prev, dirty: &none });
    assert_eq!(cached(&m), m, "current");
    edit(ws, "Compute(5)", "Compute(6)");
    assert_ne!(cached(&m), m, "an edit is seen");
    s.pin(Freshness::Fresh, &never).unwrap();
    assert_eq!(s.descriptor().and_then(|d| d.generation), Some(2));
}

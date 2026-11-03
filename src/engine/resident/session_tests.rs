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
            human: false,
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

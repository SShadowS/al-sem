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

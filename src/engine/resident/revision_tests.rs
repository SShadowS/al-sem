//! T-order and T-prepared on real artifacts (design §11): requests answered
//! in sequence off ONE resident revision, each byte-equal to its cold
//! standalone run, with the store's fill counters stated.

use std::collections::BTreeSet;
use std::sync::Arc;

use super::*;
use crate::engine::batch::job::ViewKey;
use crate::engine::batch::output::JobOutput;
use crate::engine::gate::run::build_view_model;
use crate::engine::l4::analysis::{L4Request, build_l4};
use crate::engine::l4::summary_runner::L4Reuse;
use crate::engine::resident::session::REVISION_NEEDS;
use crate::engine::session::explain_plan;
use crate::program::inputs::{WorkspaceInputs, WorkspaceNeeds};
use crate::program::model::role::Role;

/// One served command, built over a fixture's read.
#[derive(Clone, Copy, Debug)]
enum Cmd {
    Query,
    Digest,
    /// `digest --include-reads`.
    DigestReads,
    Explain,
    Prove,
}

pub(super) struct Fixture {
    pub(super) inputs: Arc<WorkspaceInputs>,
    pub(super) single_app: bool,
    /// The routine every command names.
    pub(super) routine: &'static str,
    _dir: Option<tempfile::TempDir>,
}

/// The binary's read for digest and prove (`FINGERPRINT_ROWS`).
const READ: WorkspaceNeeds = WorkspaceNeeds {
    build_only: false,
    fingerprint_rows: true,
};

pub(super) fn r0(ws: &str, routine: &'static str) -> Fixture {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/r0-corpus")
        .join(ws);
    Fixture {
        inputs: WorkspaceInputs::read(&path, &READ),
        single_app: true,
        routine,
        _dir: None,
    }
}

/// The rule-B fixture: a workspace over a source-bearing dependency, cross-app
/// (dependency calls, temporary records, events and commits).
pub(super) fn rule_b(routine: &'static str) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    crate::engine::l5::test_support::write_fixture_rule_b(dir.path());
    Fixture {
        inputs: WorkspaceInputs::read(dir.path(), &READ),
        single_app: false,
        routine,
        _dir: Some(dir),
    }
}

fn job(cmd: Cmd, f: &Fixture) -> Box<dyn Job + '_> {
    use crate::engine::l5::digest_cli::{DiffRoot, DigestJob, DigestRequest};
    use crate::engine::l5::explain::{ExplainJob, ExplainMode, ExplainRequest};
    match cmd {
        Cmd::Query => Box::new(crate::engine::l4::effect_query_cli::QueryJob::effects(
            &f.inputs,
            f.routine,
            "test",
            true,
            f.single_app,
        )),
        Cmd::Digest | Cmd::DigestReads => Box::new(DigestJob::new(
            DigestRequest {
                inputs: &f.inputs,
                changed_files: None,
                changed_routines: Some(vec![f.routine.to_string()]),
                diff_text: None,
                alsem_ver: "test",
                deterministic: true,
                single_app: f.single_app,
                tests: false,
                include_reads: matches!(cmd, Cmd::DigestReads),
                diff_root: DiffRoot::Workspace.into(),
            },
            false,
            None,
        )),
        Cmd::Explain => Box::new(ExplainJob::new(
            ExplainRequest {
                inputs: &f.inputs,
                mode: ExplainMode::Routines(vec![f.routine.to_string()]),
                single_app: f.single_app,
            },
            false,
            None,
        )),
        Cmd::Prove => Box::new(crate::engine::l5::prove::ProveJob {
            detectors: crate::engine::l5::detectors::default_detectors(),
            inputs: &f.inputs,
            routine_selector: f.routine,
            question_text: "may-commit",
            alsem_ver: "test",
            deterministic: true,
            single_app: f.single_app,
            closed_world: false,
            assume_no_dependents: false,
            human: false,
            detail: false,
            out: None,
            slice: None,
            surface: Default::default(),
        }),
    }
}

fn cold(cmd: Cmd, f: &Fixture) -> JobOutput {
    crate::engine::batch::standalone_output(&*job(cmd, f))
}

pub(super) fn test_id(generation: u64) -> RevisionId {
    RevisionId {
        engine_instance_id: "test".into(),
        generation,
        inputs_stamp: "al-sem-manifest-v1:test".into(),
    }
}

/// A revision over the fixture's read, built as the session builds one.
pub(super) fn revision(f: &Fixture) -> Revision {
    let key = ViewKey::new(&f.inputs.workspace, f.single_app, false);
    let built = build_view_model(&f.inputs, &key, REVISION_NEEDS, |_| {})
        .unwrap_or_else(|e| panic!("the fixture builds: {e:?}"))
        .expect("a model view");
    Revision::new(
        test_id(1),
        Arc::clone(&f.inputs),
        built.target,
        REVISION_NEEDS.assembly(),
    )
}

fn answer(rev: &Revision, cmd: Cmd, f: &Fixture) -> JobOutput {
    rev.answer(&*job(cmd, f), &Control::unreserved())
        .unwrap_or_else(|e| panic!("{cmd:?} answered: {e:?}"))
}

/// `order` answered in sequence off ONE revision, each answer equal to its
/// cold standalone run. Returns the revision's fill counts.
fn assert_order_equals_cold(f: &Fixture, order: &[Cmd]) -> BTreeMap<&'static str, u64> {
    let rev = revision(f);
    for &cmd in order {
        let warm = answer(&rev, cmd, f);
        let cold = cold(cmd, f);
        assert_eq!(
            warm.exit, cold.exit,
            "{order:?}: {cmd:?} on {}: exit",
            f.routine
        );
        assert_eq!(warm, cold, "{order:?}: {cmd:?} on {}", f.routine);
        assert!(
            warm.exit == 0,
            "precondition: {cmd:?} answers on {} ({:?})",
            f.routine,
            warm.message()
        );
    }
    rev.fill_counts()
}

/// T-order (§11): mixed-order sequences over one resident revision —
/// query→digest, digest→query, explain→prove, prove→explain, and two
/// requests on the same subjects — each answer byte-equal to its cold
/// one-shot run, single-app and cross-app. The revision composes once and
/// solves once whatever the order, and a repeated request fills nothing.
///
/// DISCRIMINATION (break with Edit, run, revert, run; 2026-11-02): the det
/// fill handing the harvest no composition (`caps = None` with the mask's
/// derived bits) fails at the first det fill ("Digest answered:
/// Engine(\"filling Det failed: SUMMARIES/TRANSACTION_SPANS demanded without
/// a composition …\")": the fill's panic is caught and fails the request);
/// the trees lent an EMPTY pd arena instead of the fold's fails `[Query,
/// Digest]: Digest on ForwardedAfterValidate` (bytes differ). Plan
/// 2026-11-03 W2 (2026-11-03): the explain indexes not retained across
/// requests (`answer_timed` evicting them after each answer) fails the
/// `explain_indexes` count of `[Digest, Digest, Explain, Explain]`.
#[test]
fn mixed_orders_over_one_revision_equal_cold_runs() {
    use Cmd::*;
    let fixtures = [
        r0("ws-d37", "ForwardedAfterValidate"),
        rule_b("WriteThenCommit"),
        rule_b("DepWriteThenCommit"),
    ];
    for f in &fixtures {
        for order in [
            &[Query, Digest][..],
            &[Digest, Query],
            &[Explain, Prove],
            &[Prove, Explain],
            &[Digest, Digest, Explain, Explain],
        ] {
            let fills = assert_order_equals_cold(f, order);
            let n = |k: &str| fills.get(k).copied().unwrap_or(0);
            assert_eq!(
                n("fold"),
                u64::from(order.iter().any(|c| !matches!(c, Query))),
                "{order:?}"
            );
            assert_eq!(
                n("l4"),
                u64::from(order.iter().any(|c| matches!(c, Query | Digest | Prove))),
                "{order:?}"
            );
            // Plan 2026-11-03 L6 (an amendment to design §6.10's eager
            // primary emission): the fold emits no raw facts; each view's
            // are one `Raw` fill, primary subjects too. Every request here
            // reads one view, the routine's.
            assert_eq!(
                n("raw"),
                u64::from(order.iter().any(|c| !matches!(c, Query))),
                "{order:?}: one emission for the one view"
            );
            assert!(n("det") <= 2, "{order:?}: {fills:?}");
            // Explain's indexes (plan 2026-11-03 W2): once per revision,
            // however many explains, and only for an explain.
            assert_eq!(
                n("explain_indexes"),
                u64::from(order.iter().any(|c| matches!(c, Explain))),
                "{order:?}: {fills:?}"
            );
        }
    }
}

fn effects_text(e: &DigestEffects) -> String {
    let unresolved: Vec<String> = e
        .unresolved
        .iter()
        .map(|(k, c)| format!("{k}: {:?} {:?}", c.items, c.traversal))
        .collect();
    format!(
        "{:?}\n{unresolved:?}\n{:?}\n{:?}",
        e.entries, e.coverage, e.identities
    )
}

/// T-prepared, the first dependency subject: a root inside a dependency
/// (rule B's `Dep Rec Lib.Raise`; no CLI selector names one today, so the
/// plan is built by hand) gets a `Raw` artifact of its own, and its effects
/// equal a cold one-shot view's over the same plan, after a primary request
/// on the same revision. Since plan 2026-11-03 L6 every view's raw facts,
/// primary or dependency, are one `Raw` fill emitted from the fold's
/// retained cones.
///
/// DISCRIMINATION (2026-11-02, before L6): borrowing the fold's primary map
/// for the dependency root (the `is_subset` test forced true) leaves its
/// tree without the root's raw facts: `assert_eq!(warm, cold)` fails (empty
/// against the inherited insert). L6's own proofs are on
/// `a_primary_root_gets_its_raw_facts_on_demand`.
#[test]
fn a_dependency_root_reads_a_raw_artifact() {
    let f = rule_b("WriteThenCommit");
    let rev = revision(&f);
    assert_eq!(answer(&rev, Cmd::Explain, &f), cold(Cmd::Explain, &f));
    let model = rev.target().resolved();
    let raise = model
        .workspace
        .routines
        .iter()
        .find(|r| r.name == "Raise" && model.routine_role(r) == Role::Dependency)
        .expect("precondition: the dependency's Raise is demanded")
        .stable_routine_id
        .clone();
    let plan = || Plan::merge([(JobId(0), explain_plan(BTreeSet::from([raise.clone()])))]);
    let (view, _) = rev
        .serve_plan(&plan(), &Control::unreserved(), &mut 0)
        .unwrap();
    let warm = effects_text(view.effects(JobId(0)));
    let mut cold_view = View::new(
        build_view_model(
            &f.inputs,
            &ViewKey::new(&f.inputs.workspace, false, false),
            REVISION_NEEDS,
            |_| {},
        )
        .unwrap()
        .unwrap()
        .target,
    );
    cold_view.prepare(&plan());
    let cold = effects_text(cold_view.effects(JobId(0)));
    assert!(
        cold.contains("Inherited") || cold.contains("inherited") || cold.contains("ransitive"),
        "precondition: the root's effects carry an inherited fact (its raw half): {cold}"
    );
    assert_eq!(warm, cold);
    let fills = rev.fill_counts();
    assert_eq!(fills.get("fold"), Some(&1));
    // Plan 2026-11-03 L6: one emission per view, the primary routine's (the
    // explain above) and the dependency root's.
    assert_eq!(
        fills.get("raw"),
        Some(&2),
        "one scoped emission per view, the dependency root's included"
    );
}

/// The det's core-summary harvest only (the fields an emptied
/// `CoreSummaries` empties), comparable across harvests.
fn core_harvest(det: &DetectorArtifacts) -> (BTreeMap<String, String>, BTreeMap<String, String>) {
    let unc = det
        .uncertainties_by_node
        .iter()
        .map(|(id, set)| {
            let values: Vec<&_> = det
                .uncertainties
                .elements(*set)
                .iter()
                .map(|u| det.uncertainties.value(*u))
                .collect();
            (id.clone(), format!("{values:?}"))
        })
        .collect();
    let roles = det
        .parameter_roles_by_routine
        .iter()
        .map(|(id, r)| (id.clone(), format!("{r:?}")))
        .collect();
    (unc, roles)
}

fn det_of(rev: &Revision) -> Arc<DetectorArtifacts> {
    match rev.snapshot().get(&ArtifactKey::Det).map(|a| &a.value) {
        Some(ArtifactValue::Det(d)) => Arc::clone(d),
        _ => panic!("a det is published"),
    }
}

/// T-order "digest→evict det→digest" and T-prepared "after eviction": the
/// det is evicted and refilled from the retained `L4` and its core
/// summaries, any number of times; every refill equals the first harvest and
/// the cold answers.
///
/// DISCRIMINATION (2026-11-02): a refill that finds the core summaries gone
/// (the det fill handing the harvest an empty owned `CoreSummaries` on every
/// det fill after the first, the "drop `core` after the harvest" break)
/// fails "refill 0: the core harvest" (the parameter roles are gone).
#[test]
fn det_evict_then_refill_reads_the_retained_core_summaries() {
    let f = r0("ws-d37", "ForwardedAfterValidate");
    let rev = revision(&f);
    let first = answer(&rev, Cmd::Prove, &f);
    assert_eq!(first, cold(Cmd::Prove, &f));
    let harvest = core_harvest(&det_of(&rev));
    assert!(
        !harvest.1.is_empty(),
        "precondition: the core summaries carry parameter roles"
    );
    for round in 0..2 {
        assert_eq!(
            rev.evict(&[ArtifactKey::Det]),
            1,
            "precondition: the det was published"
        );
        let again = answer(&rev, Cmd::Digest, &f);
        assert_eq!(
            core_harvest(&det_of(&rev)),
            harvest,
            "refill {round}: the core harvest"
        );
        assert_eq!(again, cold(Cmd::Digest, &f), "refill {round}: digest");
        let again = answer(&rev, Cmd::Prove, &f);
        assert_eq!(again, first, "refill {round}: prove");
    }
    let fills = rev.fill_counts();
    assert_eq!(fills.get("det"), Some(&3), "one harvest and two refills");
    assert_eq!(fills.get("l4"), Some(&1), "one solve");
    assert_eq!(fills.get("fold"), Some(&1), "one composition");
}

/// A digest followed by a seeded L4 update over the same revision reads the
/// retained core summaries (`L4Reuse::core`): every SCC is seeded, and the
/// seeded solve's projection equals a cold full solve's.
#[test]
fn digest_then_seeded_l4_update_reuses_the_retained_core() {
    let f = r0("ws-d37", "ForwardedAfterValidate");
    let rev = revision(&f);
    assert_eq!(answer(&rev, Cmd::Digest, &f), cold(Cmd::Digest, &f));
    let snap = rev.snapshot();
    let Some(ArtifactValue::L4(s)) = snap.get(&ArtifactKey::L4).map(|a| &a.value) else {
        panic!("digest solved");
    };
    let target = rev.target();
    let cancel = || false;
    let req = L4Request {
        extra_diagnostics: vec![],
        control: Some(crate::engine::l4::summary_runner::L4Control {
            cancel: &cancel,
            reuse: Some(L4Reuse {
                solve: &s.baseline,
                bundle: &s.l4.bundle,
                core: &s.core.0,
            }),
        }),
    };
    let cross = target.cross_app_inputs();
    let (l4, core, outcome) = build_l4_controlled(target.resolved(), &rev.rel, req, cross.as_ref())
        .expect("not cancelled");
    let outcome = outcome.expect("a controlled solve");
    assert!(outcome.total > 0, "precondition: SCCs to seed");
    assert_eq!(outcome.seeded, outcome.total, "every SCC seeded");
    let seeded = crate::engine::incremental::L4Projection::of(&l4, &core.0);
    let (cold_l4, cold_core) = build_l4(
        target.resolved(),
        &relation_of(target),
        L4Request {
            extra_diagnostics: vec![],
            control: None,
        },
        cross.as_ref(),
    );
    let cold = crate::engine::incremental::L4Projection::of(&cold_l4, &cold_core.0);
    assert!(
        cold.per_id.values().any(|(s, _)| s.is_some()),
        "precondition: the cold solve has core summaries"
    );
    assert_eq!(seeded, cold);
}

/// T-pin (§4.3): a job made over a read other than the pinned revision's is
/// refused (never computed against a newer or older model): one pinned
/// revision serves resolve, prepare and compute.
#[test]
fn a_job_over_another_read_is_refused() {
    let f = r0("ws-d37", "ForwardedAfterValidate");
    let rev = revision(&f);
    let other = Fixture {
        inputs: WorkspaceInputs::read(&f.inputs.workspace, &READ),
        single_app: true,
        routine: f.routine,
        _dir: None,
    };
    assert!(
        !Arc::ptr_eq(&other.inputs, &rev.inputs),
        "precondition: two reads"
    );
    let err = rev
        .answer(&*job(Cmd::Query, &other), &Control::unreserved())
        .unwrap_err();
    assert!(
        matches!(&err, PrepareError::Engine(m) if m.contains("another read")),
        "{err:?}"
    );
}

/// Engine limits B1: a job whose demand reads model rows the revision was not
/// built with is refused with its needs ([`PrepareError::Needs`]), never
/// computed (reading the missing guard arms panicked and killed the engine
/// child). Precondition, stated: the revision is built with
/// `REVISION_NEEDS`, which holds no guard arms, and the job is `prove
/// throws-error --match` (its demand is `ViewNeeds::PROVE_SLICE`). A
/// revision built with the widened needs answers it as a cold run does.
///
/// DISCRIMINATION (2026-11-09): deleting the `covers` check in
/// `answer_inner` fails this test with the panic "site guards not built:
/// declare ViewNeeds::site_guards"; restored, it passes.
#[test]
fn a_job_needing_rows_the_revision_lacks_is_refused_with_its_needs() {
    let f = r0("ws-explain", "PostOrder");
    let slice = crate::engine::l5::prove_slice::SliceRequest::of(
        "throws-error",
        Some("not released"),
        None,
        None,
        None,
        None,
    )
    .expect("a valid filter")
    .expect("a filter is given");
    let prove = || crate::engine::l5::prove::ProveJob {
        detectors: crate::engine::l5::detectors::default_detectors(),
        inputs: &f.inputs,
        routine_selector: f.routine,
        question_text: "throws-error",
        alsem_ver: "test",
        deterministic: true,
        single_app: f.single_app,
        closed_world: false,
        assume_no_dependents: false,
        human: false,
        detail: false,
        out: None,
        slice: Some(slice.clone()),
        surface: Default::default(),
    };
    let rev = revision(&f);
    assert!(
        !rev.model_needs().site_guards,
        "precondition: the revision holds no guard arms"
    );
    let err = rev.answer(&prove(), &Control::unreserved()).unwrap_err();
    assert_eq!(
        err,
        PrepareError::Needs(crate::engine::batch::job::ViewNeeds::PROVE_SLICE.assembly())
    );
    // The widened revision (the session's needs after `pin_needs`).
    let widened = REVISION_NEEDS.union(crate::engine::batch::job::ViewNeeds::PROVE_SLICE);
    let key = ViewKey::new(&f.inputs.workspace, f.single_app, false);
    let built = build_view_model(&f.inputs, &key, widened, |_| {})
        .unwrap_or_else(|e| panic!("the fixture builds: {e:?}"))
        .expect("a model view");
    let rev = Revision::new(
        test_id(2),
        Arc::clone(&f.inputs),
        built.target,
        widened.assembly(),
    );
    let warm = rev
        .answer(&prove(), &Control::unreserved())
        .expect("the widened revision answers");
    let cold = crate::engine::batch::standalone_output(&prove());
    assert_eq!(warm, cold);
}

/// A request that stops before its ready snapshot answers `cancelled` and
/// publishes nothing partial; the next request on the revision answers.
#[test]
fn a_cancelled_request_stops_between_phases() {
    let f = r0("ws-d37", "ForwardedAfterValidate");
    let rev = revision(&f);
    let stop = || Some(crate::engine::resident::store::Stop::Cancelled);
    let ctl = Control {
        stop: &stop,
        reserve: &crate::engine::resident::store::unreserved_guard,
    };
    let err = rev.answer(&*job(Cmd::Prove, &f), &ctl).unwrap_err();
    assert_eq!(err, PrepareError::Cancelled);
    assert!(
        rev.snapshot().is_empty(),
        "nothing filled for a cancelled request"
    );
    assert_eq!(answer(&rev, Cmd::Prove, &f), cold(Cmd::Prove, &f));
}

/// T-race (design §4.5, §11): a heavy and a light request answered
/// concurrently off ONE revision (as the child's heavy and light workers
/// do) while a third thread evicts every evictable kind and the L4 seed,
/// the three released together at a barrier each round: every answer is
/// its cold run's bytes, no request sees a partial snapshot, and the
/// artifacts are filled again after each eviction.
///
/// The refill is FORCED, not hoped for: between two rounds, with every
/// request returned (nothing pinned), every evictable artifact is evicted
/// (stated: none is left), so each round fills again what it reads. Before
/// 2026-11-04 the test relied on the concurrent evictor alone, whose bursts
/// skip every key a request in flight pins (`Store::evict`): under machine
/// load a run could see no burst land in a gap, and "l4 refilled after
/// eviction" failed with one l4 fill. The concurrent bursts stay, for the
/// interleavings inside a request (an eviction between its phases), and
/// now last exactly as long as the round's requests (they were 50 passes
/// with a `yield_now` each, which could end before a request reached its
/// later phases). Forcing every round to refill everything at once made a
/// real store deadlock reachable (2 hangs in 300 loaded runs, both heavy
/// requests in `Store::wait_for`, each waiting on a key the other held
/// claimed): fixed in `ensure_prepared` and pinned by the store's
/// `a_request_publishes_its_claims_before_it_waits_for_a_joined_key`. The
/// pins themselves are pinned
/// deterministically by the store's `eviction_skips_keys_pinned_for_assembly`
/// and `a_reclaim_skips_values_a_ready_snapshot_holds`.
///
/// DISCRIMINATION (break with Edit, run, revert, run; 2026-11-04): (a) no
/// between-rounds eviction (`if round > 0 && false`) fails "l4 refilled
/// after eviction" (35 to 37 l4 fills of 40 rounds, 5 of 5 runs);
/// (b) no pins at all (`pins += 0` in `ensure_prepared` and `Store::hold`)
/// fails with "a tree needs the fold, which is not published" or "the tree
/// base needs the fold" (5 of 5 runs); (c) the explain indexes' kind name
/// drifting from the name eviction uses (`ArtifactKey::kind` returning
/// `"explain-indexes"`) fails "explain_indexes refilled after eviction" (one
/// fill, 3 of 3 runs). Reverted, it passes.
#[test]
fn concurrent_requests_and_evictions_answer_the_cold_bytes() {
    const ROUNDS: usize = 40;
    let f = r0("ws-d37", "ForwardedAfterValidate");
    let rev = revision(&f);
    let (cold_prove, cold_query) = (cold(Cmd::Prove, &f), cold(Cmd::Query, &f));
    let (cold_explain, cold_digest) = (cold(Cmd::Explain, &f), cold(Cmd::Digest, &f));
    // Each round releases the four threads together: a rendezvous send per
    // thread (a thread that panicked has dropped its receiver, so the send
    // fails at once and nothing waits on it forever). The two heavy threads
    // ask the same root at once (they share its witness key). Each thread
    // then reports the end of its round on its OWN channel, so the next
    // round starts only once every request of this one has returned; a
    // thread that panicked has dropped its only sender, so that wait ends at
    // once too (one shared channel would wait forever on the others' clones).
    use std::sync::mpsc::{Receiver, channel, sync_channel};
    let (go_digest, digest_rx) = sync_channel::<()>(0);
    let (go_heavy, heavy_rx) = sync_channel::<()>(0);
    let (go_light, light_rx) = sync_channel::<()>(0);
    let (go_evict, evict_rx) = sync_channel::<()>(0);
    let (digest_done, digest_done_rx) = channel::<()>();
    let (heavy_done, heavy_done_rx) = channel::<()>();
    let (light_done, light_done_rx) = channel::<()>();
    let (evict_done, evict_done_rx) = channel::<()>();
    let dones: [&Receiver<()>; 4] = [
        &evict_done_rx,
        &heavy_done_rx,
        &digest_done_rx,
        &light_done_rx,
    ];
    // Keys of an evictable kind left after a between-rounds eviction (the
    // precondition, asserted once the threads are joined).
    let mut left = Vec::new();
    let requests_done = std::sync::atomic::AtomicBool::new(false);
    const KINDS: [&str; 11] = [
        "witness",
        "explain_indexes",
        "raw",
        "tree",
        "tree_base",
        "run",
        "det",
        "fold",
        "l4",
        "rules",
        "ordering",
    ];
    let (rev, f) = (&rev, &f);
    std::thread::scope(|s| {
        let heavy = s.spawn(move || {
            let mut outs = Vec::new();
            for i in 0.. {
                if heavy_rx.recv().is_err() {
                    break;
                }
                let cmd = if i % 2 == 0 { Cmd::Prove } else { Cmd::Explain };
                outs.push((cmd, answer(rev, cmd, f)));
                let _ = heavy_done.send(());
            }
            outs
        });
        let digests = s.spawn(move || {
            let mut outs = Vec::new();
            while digest_rx.recv().is_ok() {
                outs.push(answer(rev, Cmd::Digest, f));
                let _ = digest_done.send(());
            }
            outs
        });
        let light = s.spawn(move || {
            let mut outs = Vec::new();
            while light_rx.recv().is_ok() {
                outs.push(answer(rev, Cmd::Query, f));
                let _ = light_done.send(());
            }
            outs
        });
        // The evictor's burst lasts exactly as long as the round's requests
        // (main raises `requests_done` once all three returned): a burst of
        // a fixed number of passes could end before the requests reach
        // their later phases, and its `yield_now`s cost a full time slice
        // each on a saturated machine (50 per round made a loaded run take
        // minutes).
        let requests_done = &requests_done;
        let evictor = s.spawn(move || {
            while evict_rx.recv().is_ok() {
                while !requests_done.load(std::sync::atomic::Ordering::SeqCst) {
                    for k in KINDS {
                        rev.evict_kind(k);
                    }
                    std::thread::yield_now();
                }
                let _ = evict_done.send(());
            }
        });
        'rounds: for round in 0..ROUNDS {
            // The forced interleaving: from round 1 on, every artifact the
            // last round published leaves the store before this round's
            // requests start, so each request of this round fills again
            // what it reads. (Whether the concurrent evictor's bursts land
            // depends on the scheduler: a key pinned by a request in flight
            // is skipped, and under load a burst can miss every gap.)
            if round > 0 {
                for k in KINDS {
                    rev.evict_kind(k);
                }
                left = rev.keys_where(|k| KINDS.contains(&k.kind()));
                if !left.is_empty() {
                    break;
                }
            }
            requests_done.store(false, std::sync::atomic::Ordering::SeqCst);
            for go in [&go_evict, &go_heavy, &go_digest, &go_light] {
                if go.send(()).is_err() {
                    break 'rounds;
                }
            }
            for done in &dones[1..] {
                if done.recv().is_err() {
                    break 'rounds;
                }
            }
            requests_done.store(true, std::sync::atomic::Ordering::SeqCst);
            if dones[0].recv().is_err() {
                break;
            }
        }
        // An early exit leaves the evictor's burst free to end.
        requests_done.store(true, std::sync::atomic::Ordering::SeqCst);
        drop((go_heavy, go_digest, go_light, go_evict));
        for out in digests.join().unwrap() {
            assert_eq!(out, cold_digest, "Digest");
        }
        for (cmd, out) in heavy.join().unwrap() {
            let cold = if matches!(cmd, Cmd::Prove) {
                &cold_prove
            } else {
                &cold_explain
            };
            assert_eq!(&out, cold, "{cmd:?}");
        }
        for out in light.join().unwrap() {
            assert_eq!(out, cold_query);
        }
        // How many keys the bursts met unpinned is the scheduler's: not
        // asserted (the forced eviction above is).
        evictor.join().unwrap();
    });
    assert!(
        left.is_empty(),
        "precondition: a round starts with every evictable kind evicted: {left:?}"
    );
    // The tree store's artifacts (stage 5b) churn like the others: a
    // request's tree is pinned through its reads, and an evicted tree or
    // base is filled again, never read half-built. So do the witnesses
    // (plan 2026-11-03 W): one evicted between its claim and its
    // publication is discarded (the generation moved) and filled again; and
    // explain's indexes (W2), read by the explains of the heavy thread; and
    // the view's raw facts (L6: every heavy request reads a `Raw` artifact).
    // Every round's requests read l4, raw, a tree, the tree base and a
    // witness, and every odd round's heavy request (an explain) reads the
    // explain indexes; each round starts with none of them in the store, so
    // each is filled at least once per round that reads it.
    let fills = rev.fill_counts();
    for (kind, rounds) in [
        ("l4", ROUNDS),
        ("raw", ROUNDS),
        ("tree", ROUNDS),
        ("tree_base", ROUNDS),
        ("witness", ROUNDS),
        ("explain_indexes", ROUNDS / 2),
    ] {
        assert!(
            fills.get(kind).copied().unwrap_or(0) >= rounds as u64,
            "{kind} refilled after eviction: {fills:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// The witness memo (plan 2026-11-03, item W).
// ---------------------------------------------------------------------------

/// The witness fixture: G23's dead path listed first (`DocBoth`: prove's
/// literal reach prunes a listed witness path, so its pass MUTATES the
/// entry), a root with a table read (`ReadThenQueue`: `--include-reads`
/// changes its entry) and a third root, single-app.
const WITNESS_SOURCE: &str = r#"table 50230 "W Queue"
{
    fields { field(1; Code; Code[20]) { } }
    keys { key(PK; Code) { } }
}
codeunit 50231 "W Handler"
{
    procedure DocBoth()
    begin
        RunReportRec(false, 0);
        Hop1();
    end;

    local procedure Hop1()
    begin
        Hop2();
    end;

    local procedure Hop2()
    begin
        Hop3();
    end;

    local procedure Hop3()
    begin
        RunReportRec(true, 0);
    end;

    local procedure RunReportRec(UseQueue: Boolean; Depth: Integer)
    begin
        RunReportIDRec(UseQueue, Depth);
    end;

    local procedure RunReportIDRec(UseQueue: Boolean; Depth: Integer)
    begin
        if Depth < 3 then
            RunReportRec(UseQueue, Depth + 1);
        if UseQueue then
            QueueMail();
    end;

    procedure ReadThenQueue()
    var
        Q: Record "W Queue";
    begin
        if Q.Get('A') then
            QueueMail();
    end;

    procedure QueueTwice()
    begin
        QueueMail();
        QueueMail();
    end;

    local procedure QueueMail()
    var
        Q: Record "W Queue";
    begin
        Q.Code := 'X';
        Q.Insert();
    end;
}
"#;

fn witness_ws(routine: &'static str) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::write(
        dir.path().join("app.json"),
        r#"{"id":"23232323-0000-4000-8000-000000000231","name":"W","publisher":"t","version":"1.0.0.0"}"#,
    )
    .unwrap();
    std::fs::write(dir.path().join("src/W.al"), WITNESS_SOURCE).unwrap();
    Fixture {
        inputs: WorkspaceInputs::read(dir.path(), &READ),
        single_app: true,
        routine,
        _dir: Some(dir),
    }
}

/// `explain` over several routines at once.
fn explain_many<'a>(f: &'a Fixture, routines: &[&str]) -> Box<dyn Job + 'a> {
    use crate::engine::l5::explain::{ExplainJob, ExplainMode, ExplainRequest};
    Box::new(ExplainJob::new(
        ExplainRequest {
            inputs: &f.inputs,
            mode: ExplainMode::Routines(routines.iter().map(|r| r.to_string()).collect()),
            single_app: f.single_app,
        },
        false,
        None,
    ))
}

/// The number of distinct trees the revision published (one per view).
fn tree_keys(rev: &Revision) -> u64 {
    rev.keys_where(|k| matches!(k, ArtifactKey::Tree(_))).len() as u64
}

/// T-order, extended (plan 2026-11-03 W): every order of explain, prove and
/// digest on one root, and `[digest, digest --include-reads, digest]`, over
/// one revision: each answer equals its cold run; the root's witness is
/// filled once per distinct `(root, include_reads)` and found by every later
/// request; one tree per view, as before.
///
/// DISCRIMINATION (break with Edit, run, revert, run; 2026-11-03): (a) the
/// key without `include_reads` (every `Witness` key built with
/// `include_reads: false` in `needs_of`, `fill_witness` and `trees`, the
/// root pass still run with the request's flag) fails `[Digest,
/// DigestReads, Digest]: DigestReads` (bytes differ: the plain entry, no
/// `DB_READ`); reverted, it passes.
#[test]
fn every_order_of_the_heavy_commands_shares_one_witness() {
    use Cmd::*;
    let fixtures = [
        witness_ws("DocBoth"),
        witness_ws("ReadThenQueue"),
        r0("ws-d37", "ForwardedAfterValidate"),
        rule_b("WriteThenCommit"),
        rule_b("DepWriteThenCommit"),
    ];
    let reads = witness_ws("ReadThenQueue");
    assert_ne!(
        cold(Digest, &reads),
        cold(DigestReads, &reads),
        "precondition: --include-reads changes ReadThenQueue's digest"
    );
    for f in &fixtures {
        let orders: [[Cmd; 3]; 6] = [
            [Explain, Prove, Digest],
            [Explain, Digest, Prove],
            [Prove, Explain, Digest],
            [Prove, Digest, Explain],
            [Digest, Explain, Prove],
            [Digest, Prove, Explain],
        ];
        for order in &orders {
            let rev = revision(f);
            for &cmd in order {
                assert_eq!(answer(&rev, cmd, f), cold(cmd, f), "{order:?}: {cmd:?}");
            }
            let fills = rev.fill_counts();
            let n = |k: &str| fills.get(k).copied().unwrap_or(0);
            assert_eq!(n("witness"), 1, "{}: {order:?}: {fills:?}", f.routine);
            assert_eq!(n("tree"), tree_keys(&rev), "{order:?}: one tree per view");
            let (hit, miss, batches) = rev.witness_counts();
            assert_eq!((hit, miss, batches), (2, 1, 1), "{}: {order:?}", f.routine);
        }
    }
    for f in [&reads, &fixtures[0]] {
        let rev = revision(f);
        let order = [Digest, DigestReads, Digest];
        for cmd in order {
            assert_eq!(answer(&rev, cmd, f), cold(cmd, f), "{order:?}: {cmd:?}");
        }
        assert_eq!(rev.fill_counts().get("witness"), Some(&2), "{order:?}");
        assert_eq!(rev.witness_counts(), (1, 2, 2), "{order:?}");
    }
}

/// A digest after a prove on the same revision answers as a cold digest
/// does. Until W2-1 only prove ran the literal pass, so the revision kept the
/// UNPRUNED entry and each read pruned its own clone (plan 2026-11-03 W, §6
/// "aliasing"). Since W2-1 every effects reader runs the pass, and since
/// W2-1b the revision memoizes the entry AFTER it
/// (`digest_cli::passed_root_entries`), once per root, so a warm ask pays
/// nothing for it. The test pins that the digest is pruned as prove is (the
/// dead `RunReportRec(false, …)` path is gone) and that both answers equal
/// their cold runs in this order, with one witness fill.
#[test]
fn a_digest_after_a_prove_answers_as_a_cold_digest() {
    let f = witness_ws("DocBoth");
    let digest = cold(Cmd::Digest, &f);
    let text = String::from_utf8_lossy(match &digest.actions[..] {
        [crate::engine::batch::output::WriteAction::Stdout(b), ..] => b,
        other => panic!("precondition: digest writes stdout first: {other:?}"),
    })
    .into_owned();
    // W2-1: the digest is pruned as prove is: every listed path ran through
    // the dead `RunReportRec(false, …)` call, so none is left (the effect
    // stays, its paths marked truncated). Since the final CDO review (item
    // 2) the row carries its searched witness instead: the live route
    // through `Hop1`, never the dead first call.
    // Digest 4.0.0 (§7.3): the rows read back into their 3.x shape.
    let doc =
        crate::engine::l5::digest_v4::document_to_v3(crate::engine::l5::digest_v5::document_to_v4(
            serde_json::from_str(&text).expect("digest JSON"),
        ));
    let row = doc["payload"]["entries"][0]["effects"]
        .as_array()
        .and_then(|es| {
            es.iter()
                .find(|e| e["type"] == "DB_INSERT" && e.get("crossing").is_none())
        })
        .unwrap_or_else(|| panic!("precondition: DocBoth's insert row: {text}"));
    assert_eq!(row["pathsKept"], 0, "no listed path is left: {row}");
    assert_eq!(
        row["paths"][0]["searched"], true,
        "the searched witness: {row}"
    );
    let first = row["paths"][0]["hops"][0]["to"].as_str().unwrap_or("");
    assert!(
        doc["payload"]["routines"][first]["display"]
            .as_str()
            .is_some_and(|d| d.ends_with("::Hop1")),
        "precondition: DocBoth's digest lists no dead path: {text}"
    );
    let rev = revision(&f);
    assert_eq!(answer(&rev, Cmd::Prove, &f), cold(Cmd::Prove, &f), "prove");
    assert_eq!(answer(&rev, Cmd::Digest, &f), digest, "digest after prove");
    assert_eq!(rev.witness_counts(), (1, 1, 1));
}

/// Cross-view hit and the batch (plan 2026-11-03 W): an explain over three
/// roots computes their three witnesses in ONE pass over its tree (one
/// index build); a prove over one of them, a different view, fills no
/// witness and finds it. Both equal their cold runs.
///
/// DISCRIMINATION (2026-11-03): (d) no batch (each witness fill computing
/// its own root alone: `&& k == *key` in `fill_witness`'s absent filter)
/// fails "three roots, one pass" (`(0, 3, 3)` against `(0, 3, 1)`: three
/// passes, each its own index build); reverted, it passes.
#[test]
fn one_pass_computes_a_requests_witnesses_and_another_view_finds_them() {
    let f = witness_ws("ReadThenQueue");
    let roots = ["DocBoth", "QueueTwice", "ReadThenQueue"];
    let rev = revision(&f);
    let explain = explain_many(&f, &roots);
    let warm = rev.answer(&*explain, &Control::unreserved()).unwrap();
    assert_eq!(warm, crate::engine::batch::standalone_output(&*explain));
    assert_eq!(warm.exit, 0, "precondition: {}", warm.message());
    assert_eq!(rev.fill_counts().get("witness"), Some(&3));
    assert_eq!(rev.witness_counts(), (0, 3, 1), "three roots, one pass");
    let trees = tree_keys(&rev);
    assert_eq!(answer(&rev, Cmd::Prove, &f), cold(Cmd::Prove, &f));
    assert_eq!(
        rev.fill_counts().get("witness"),
        Some(&3),
        "the prove fills no witness"
    );
    assert!(
        tree_keys(&rev) > trees,
        "precondition: the prove reads another view"
    );
    assert_eq!(rev.witness_counts(), (1, 3, 1));
}

/// The witness batch reserves once: a request over three roots computes
/// their witnesses in its first witness fill, and its two later witness
/// fills are lookups in the request's scratch that reserve nothing. (Each
/// reserving again, at the case's estimate, which is the batch's observed
/// peak once observed, refused a 699-root diff on CDO: 2042 MiB per root
/// over a 7.4 GB RSS, after the batch itself had fitted.) Every other
/// fill still reserves, and the answer is its cold run's.
///
/// DISCRIMINATION (2026-11-05): `serve_plan` handing the store the
/// request's own `ctl` (no in-hand check) fails "one witness reservation"
/// (3 against 1); restored, passes.
#[test]
fn a_requests_witness_batch_reserves_once() {
    use std::sync::Mutex;
    let f = witness_ws("ReadThenQueue");
    let roots = ["DocBoth", "QueueTwice", "ReadThenQueue"];
    let rev = revision(&f);
    let kinds: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());
    let reserve = |kind: &'static str,
                   _: usize|
     -> Result<crate::engine::resident::store::Guard<'_>, String> {
        kinds.lock().unwrap().push(kind);
        Ok(Box::new(()))
    };
    let ctl = Control {
        stop: &|| None,
        reserve: &reserve,
    };
    let explain = explain_many(&f, &roots);
    let warm = rev.answer(&*explain, &ctl).unwrap();
    assert_eq!(warm, crate::engine::batch::standalone_output(&*explain));
    assert_eq!(
        rev.fill_counts().get("witness"),
        Some(&3),
        "precondition: three witness fills"
    );
    let kinds = kinds.into_inner().unwrap();
    let n = |k: &str| kinds.iter().filter(|x| **x == k).count();
    assert_eq!(n("witness"), 1, "one witness reservation: {kinds:?}");
    assert_eq!(n("fold"), 1, "every other fill reserves: {kinds:?}");
    assert_eq!(n("trees"), 1, "{kinds:?}");
}

/// A request's trees' reservation can reclaim kind after kind: the request
/// lets its ready snapshot go before it reserves, so the reclaim's first
/// eviction (an earlier request's witness) leaves the earlier request's
/// tree with one owner, and the tree goes too. Found on CDO (output
/// validation round 4, p09): 33 earlier trees (2.5 GB) stayed resident
/// through every reclaim, the floor read the whole RSS and an explain was
/// refused as impossible. The answer is still its cold run's.
///
/// DISCRIMINATION (2026-11-08): keeping the ready snapshot through the
/// trees' reservation (no `drop(snap)` before it) fails "the earlier tree
/// goes too" (0 evicted); restored, passes.
#[test]
fn a_requests_trees_reclaim_evicts_an_earlier_requests_tree() {
    use std::sync::Mutex;
    let f = witness_ws("ReadThenQueue");
    let rev = revision(&f);
    let first = explain_many(&f, &["DocBoth"]);
    rev.answer(&*first, &Control::unreserved()).unwrap();
    let held = |kind: &str| rev.keys_where(|k| k.kind() == kind).len();
    assert_eq!(held("tree"), 1, "precondition: the first request's tree");
    assert!(held("witness") >= 1, "precondition: its witness");
    let evicted: Mutex<Vec<(usize, usize)>> = Mutex::new(Vec::new());
    // The second request's trees' reservation reclaims, in the design's
    // order: the witnesses, then the trees.
    let reserve = |kind: &'static str,
                   _: usize|
     -> Result<crate::engine::resident::store::Guard<'_>, String> {
        if kind == "trees" {
            let w = rev.reclaim_kind("witness");
            let t = rev.reclaim_kind("tree");
            evicted.lock().unwrap().push((w, t));
        }
        Ok(Box::new(()))
    };
    let ctl = Control {
        stop: &|| None,
        reserve: &reserve,
    };
    let second = explain_many(&f, &["QueueTwice"]);
    let warm = rev.answer(&*second, &ctl).unwrap();
    assert_eq!(warm, crate::engine::batch::standalone_output(&*second));
    let evicted = evicted.into_inner().unwrap();
    assert_eq!(evicted.len(), 1, "one trees' reservation: {evicted:?}");
    assert!(evicted[0].0 >= 1, "the earlier witness goes: {evicted:?}");
    assert_eq!(evicted[0].1, 1, "the earlier tree goes too: {evicted:?}");
}

/// An impact main part over many roots streams its effects off the
/// revision's published tree (`TreeDemand::stream`; here one root per batch,
/// `root_batch: Some(1)`): its trees' reservation is ONE batch's (1 root, not
/// 3), held through compute and finished only once every batch was read,
/// no witness is filled, and the answer is the bytes of the cold run of the
/// same ask read whole.
///
/// DISCRIMINATION (2026-11-05): finishing the trees' reservation before
/// compute (`serve_plan_held` never handing it on) fails "finished after
/// every batch" (0 batches against 3); restored, passes.
#[test]
fn a_streamed_impact_reserves_one_batch_and_holds_it_through_compute() {
    use crate::engine::impact::jobs::{MainAsk, MainJob};
    use crate::engine::l5::digest_cli::STREAM_BATCHES;
    use std::sync::Mutex;
    let f = witness_ws("ReadThenQueue");
    let rev = revision(&f);
    fn main(hand: &crate::engine::batch::job::HandOff, root_batch: Option<usize>) -> MainJob<'_> {
        MainJob {
            inputs: hand,
            single_app: true,
            ask: MainAsk::Routines(
                ["DocBoth", "QueueTwice", "ReadThenQueue"]
                    .iter()
                    .map(|r| r.to_string())
                    .collect(),
            ),
            scope: Default::default(),
            summaries: false,
            extra_roots: Vec::new(),
            root_batch,
        }
    }
    assert!(f.single_app, "precondition: a single-app fixture");
    let log: Mutex<Vec<(&'static str, usize)>> = Mutex::new(Vec::new());
    struct Finish<'a>(&'a Mutex<Vec<(&'static str, usize)>>);
    impl crate::engine::resident::store::Held for Finish<'_> {
        fn finish(self: Box<Self>) -> u64 {
            let batches = STREAM_BATCHES.with(|n| n.get());
            self.0.lock().unwrap().push(("finished", batches));
            0
        }
    }
    let reserve = |kind: &'static str,
                   units: usize|
     -> Result<crate::engine::resident::store::Guard<'_>, String> {
        log.lock().unwrap().push((kind, units));
        if kind == "trees" {
            Ok(Box::new(Finish(&log)))
        } else {
            Ok(Box::new(()))
        }
    };
    let ctl = Control {
        stop: &|| None,
        reserve: &reserve,
    };
    STREAM_BATCHES.with(|n| n.set(0));
    let hand = crate::engine::batch::job::HandOff::new(Arc::clone(&f.inputs));
    let warm = rev.answer(&main(&hand, Some(1)), &ctl).unwrap();
    let log = log.into_inner().unwrap();
    assert_eq!(warm.exit, 0, "precondition: {}", warm.message());
    assert_eq!(
        log.iter()
            .filter(|(k, _)| *k == "trees")
            .collect::<Vec<_>>(),
        vec![&("trees", 1)],
        "one batch's reservation: {log:?}"
    );
    assert_eq!(
        log.iter()
            .filter(|(k, _)| *k == "finished")
            .collect::<Vec<_>>(),
        vec![&("finished", 3)],
        "finished after every batch: {log:?}"
    );
    assert_eq!(
        rev.fill_counts().get("witness"),
        None,
        "no witness is filled"
    );
    let whole = crate::engine::batch::job::HandOff::new(Arc::clone(&f.inputs));
    assert_eq!(
        warm,
        crate::engine::batch::standalone_output(&main(&whole, None))
    );
}

/// An effects read over more roots than the memo's cap memoizes none: it
/// computes its entries itself, publishes no witness, and answers its cold
/// run's bytes; a read within the cap still memoizes. (CDO: a 699-root
/// impact held 2+ GB of witnesses, twice over with its clones, and its
/// trees' reservation was refused.) The cap is lowered to 2 to reach it on
/// the fixture's three roots.
///
/// DISCRIMINATION (2026-11-05): `witness_groups` without its cap filter (the
/// tree read's own check kept) fails "no witness is filled" (3 fills);
/// restored, passes.
#[test]
fn a_read_over_more_roots_than_the_cap_memoizes_no_witness() {
    let f = witness_ws("ReadThenQueue");
    let rev = revision(&f);
    rev.set_witness_memo_max(2);
    let explain = explain_many(&f, &["DocBoth", "QueueTwice", "ReadThenQueue"]);
    let warm = rev.answer(&*explain, &Control::unreserved()).unwrap();
    assert_eq!(warm.exit, 0, "precondition: {}", warm.message());
    assert_eq!(warm, crate::engine::batch::standalone_output(&*explain));
    assert_eq!(
        rev.fill_counts().get("witness"),
        None,
        "no witness is filled"
    );
    assert!(
        !rev.snapshot()
            .keys()
            .any(|k| matches!(k, ArtifactKey::Witness { .. })),
        "none is published"
    );
    // Within the cap the memo serves as before.
    let two = explain_many(&f, &["DocBoth", "QueueTwice"]);
    let warm = rev.answer(&*two, &Control::unreserved()).unwrap();
    assert_eq!(warm, crate::engine::batch::standalone_output(&*two));
    assert_eq!(rev.fill_counts().get("witness"), Some(&2));
}

/// TLS-22: every route a revision runs the literal pass on obeys the
/// revision's worker count, observed (each pass's first wave held until
/// `n` roots are in flight, at most 10 s): a witness fill, a read over more
/// roots than the memo's cap (the tree read computes its own entries), and a
/// streamed impact. Each answer is its cold run's.
///
/// DISCRIMINATION (2026-11-11): a route passing the default setting instead
/// of the revision's (`fill_witness` calling `passed_root_entries` with
/// `default_reach_workers()`) fails the witness case; restored, passes.
#[test]
fn every_route_runs_the_pass_with_its_revisions_worker_count() {
    use crate::engine::l5::digest_cli::ReachWorkers;
    let roots = ["DocBoth", "QueueTwice", "ReadThenQueue"];
    let f = witness_ws("ReadThenQueue");
    let held = |rev: &Revision, n: usize| {
        *rev.reach_probe().first_wave.lock().unwrap() =
            Some((n, std::time::Duration::from_secs(10)));
    };
    let passes = |rev: &Revision| {
        rev.reach_probe()
            .passes
            .load(std::sync::atomic::Ordering::SeqCst)
    };
    // A witness fill: the request's three roots in one pass.
    // Pool size 7, no other test's (a held first wave must not share a pool
    // with another test's passes under a plain `cargo test`); the request's
    // three roots run three at once.
    let rev = revision(&f).with_reach_workers(ReachWorkers::new(7));
    held(&rev, 3);
    let explain = explain_many(&f, &roots);
    let warm = rev.answer(&*explain, &Control::unreserved()).unwrap();
    assert_eq!(warm, crate::engine::batch::standalone_output(&*explain));
    assert_eq!(rev.fill_counts().get("witness"), Some(&3), "precondition");
    assert!(passes(&rev) >= 1, "precondition: a pass ran");
    assert_eq!(rev.reach_probe().max(), 3, "a witness fill");
    // Over the memo's cap: the tree read's own entries.
    let rev = revision(&f).with_reach_workers(ReachWorkers::new(7));
    rev.set_witness_memo_max(2);
    held(&rev, 3);
    let warm = rev.answer(&*explain, &Control::unreserved()).unwrap();
    assert_eq!(warm, crate::engine::batch::standalone_output(&*explain));
    assert_eq!(rev.fill_counts().get("witness"), None, "precondition");
    assert!(passes(&rev) >= 1, "precondition: a pass ran");
    assert_eq!(rev.reach_probe().max(), 3, "a read over the cap");
    // A streamed impact, two roots per batch.
    use crate::engine::impact::jobs::{MainAsk, MainJob};
    let main = |hand, root_batch| MainJob {
        inputs: hand,
        single_app: true,
        ask: MainAsk::Routines(roots.iter().map(|r| r.to_string()).collect()),
        scope: Default::default(),
        summaries: false,
        extra_roots: Vec::new(),
        root_batch,
    };
    let rev = revision(&f).with_reach_workers(ReachWorkers::new(7));
    held(&rev, 2);
    let hand = crate::engine::batch::job::HandOff::new(Arc::clone(&f.inputs));
    let warm = rev
        .answer(&main(&hand, Some(2)), &Control::unreserved())
        .unwrap();
    let whole = crate::engine::batch::job::HandOff::new(Arc::clone(&f.inputs));
    assert_eq!(
        warm,
        crate::engine::batch::standalone_output(&main(&whole, None))
    );
    assert!(passes(&rev) >= 2, "precondition: two batches, two passes");
    assert_eq!(rev.reach_probe().max(), 2, "a streamed impact");
}

/// TLS-21: a revision answers `impact-up` with ITS worker count. At 3, with
/// the first three walks held until three are in flight, the probe sees
/// exactly 3 at once in a pool of exactly 3 threads, one task per walked
/// root, and master's answer. A production revision (serial) walks one root
/// at a time over the same hold.
///
/// DISCRIMINATION (2026-11-12): a revision whose `reach_workers` starts as
/// `default_reach_workers()` instead of `SERIAL` (`Revision::new`) fails the
/// production case (5 in flight); restored, passes.
#[test]
fn the_resident_up_walk_runs_on_the_revisions_count() {
    use crate::engine::impact::jobs::{UpAsk, UpJob};
    use crate::engine::impact::up::up_par_tests::{fixture_dir, root_ids};
    use crate::engine::l5::digest_cli::ReachWorkers;
    let dir = fixture_dir();
    let f = Fixture {
        inputs: WorkspaceInputs::read(&dir.path().join("ws"), &READ),
        single_app: false,
        routine: "",
        _dir: Some(dir),
    };
    let program = crate::program::resolved::ResolvedProgram::build(
        &f.inputs,
        crate::program::resolved::ProgramScope::CrossApp,
    )
    .expect("the fixture builds");
    let g = Arc::new(
        crate::program::impact_graph::sited_from_program(&program, &|| false).expect("built"),
    );
    let ask = || UpAsk {
        roots: root_ids(),
        assume_no_dependents: false,
        detail: false,
        single_app: false,
    };
    let held = |rev: &Revision, n: usize, timeout: u64| {
        *rev.reach_probe().first_wave.lock().unwrap() =
            Some((n, std::time::Duration::from_secs(timeout)));
    };
    let hand = crate::engine::batch::job::HandOff::new(Arc::clone(&f.inputs));
    let job = UpJob::new(&hand, ask());
    let cold = crate::engine::batch::standalone_output(&job);
    // At 3 workers.
    let rev = revision(&f)
        .with_sited(Arc::clone(&g))
        .with_reach_workers(ReachWorkers::new(3));
    held(&rev, 3, 10);
    let hand = crate::engine::batch::job::HandOff::new(Arc::clone(&f.inputs));
    let out = rev
        .answer(&UpJob::new(&hand, ask()), &Control::unreserved())
        .unwrap();
    assert_eq!(out.exit, 0, "{:?}", out.message());
    let p = rev.reach_probe();
    assert_eq!(
        p.up_tasks.load(std::sync::atomic::Ordering::SeqCst),
        5,
        "one task per walked root"
    );
    if std::thread::available_parallelism().map_or(1, usize::from) < 2 {
        eprintln!("one core: the parallel claim is not checked");
    } else {
        assert_eq!(p.max(), 3, "three walks at once");
        assert!(
            p.pool_threads.lock().unwrap().iter().all(|&n| n == 3),
            "a pool of exactly three: {:?}",
            p.pool_threads.lock().unwrap()
        );
    }
    let answer = |o: &JobOutput| -> serde_json::Value {
        let stdout: Vec<u8> = o
            .actions
            .iter()
            .filter_map(|a| match a {
                crate::engine::batch::output::WriteAction::Stdout(b) => Some(b.clone()),
                _ => None,
            })
            .flatten()
            .collect();
        let mut v: serde_json::Value = serde_json::from_slice(&stdout).expect("JSON");
        for k in ["walkMs", "walksUs", "scratchBytes"] {
            v["cost"][k] = serde_json::Value::Null;
        }
        v
    };
    assert_eq!(answer(&out), answer(&cold), "the cold run's answer");
    // A production revision: serial.
    let rev = revision(&f).with_sited(Arc::clone(&g));
    held(&rev, 3, 2);
    let hand = crate::engine::batch::job::HandOff::new(Arc::clone(&f.inputs));
    let out = rev
        .answer(&UpJob::new(&hand, ask()), &Control::unreserved())
        .unwrap();
    assert_eq!(answer(&out), answer(&cold));
    let p = rev.reach_probe();
    assert_eq!(p.up_tasks.load(std::sync::atomic::Ordering::SeqCst), 5);
    assert_eq!(p.max(), 1, "the resident engine walks one root at a time");
}

/// Without its tree a witness fill fails loudly, never answers wrong: a
/// plan's witnesses are filled off the request's own `Tree(scope)`.
///
/// DISCRIMINATION (2026-11-03): (c) dropping the `Tree` key from
/// `needs_of` (`if false && …`) fails the request with "filling Witness
/// { … } failed: a witness needs its tree, which is not published" (the
/// test sees the error where it expects the answer); reverted, it passes.
#[test]
fn a_witness_reads_the_requests_own_tree() {
    let f = witness_ws("DocBoth");
    let rev = revision(&f);
    let out = rev.answer(&*job(Cmd::Digest, &f), &Control::unreserved());
    match out {
        Ok(o) => assert_eq!(o, cold(Cmd::Digest, &f)),
        Err(e) => panic!("the digest failed: {e:?}"),
    }
    let snap = rev.snapshot();
    let tree = snap
        .keys()
        .find(|k| matches!(k, ArtifactKey::Tree(_)))
        .expect("precondition: the digest's tree is published");
    assert!(
        snap.keys()
            .any(|k| matches!(k, ArtifactKey::Witness { .. })),
        "the witness is published beside {tree:?}"
    );
}

/// A witness fill handed a snapshot that already holds its key returns the
/// published entry. The store does that when a request read the snapshot
/// before a concurrent request published the key: it claims the key once
/// that fill is over and fills off the newer snapshot.
/// `concurrent_requests_and_evictions_answer_the_cold_bytes` hit it about
/// one run in three once the producer solve ran on rayon (plan 2026-11-03
/// L2 changed the timing, not the rule). The state is stated by hand: the
/// key published, the request's scratch empty, and a plan with no witness
/// group (so nothing but the snapshot can answer).
///
/// DISCRIMINATION (2026-11-03): without the snapshot check at the top of
/// `fill_witness` this fails with "… is not a witness of the request's
/// plan" (the batch skips keys the snapshot holds); restored, it passes.
#[test]
fn a_witness_fill_returns_the_entry_its_snapshot_already_holds() {
    let f = witness_ws("DocBoth");
    let rev = revision(&f);
    let out = rev
        .answer(&*job(Cmd::Digest, &f), &Control::unreserved())
        .expect("the digest answers");
    assert_eq!(out, cold(Cmd::Digest, &f));
    let snap = rev.snapshot();
    let (key, published) = snap
        .keys()
        .find_map(|k| match snap.get(k).map(|a| &a.value) {
            Some(ArtifactValue::Witness(e)) => Some((k.clone(), Arc::clone(e))),
            _ => None,
        })
        .expect("precondition: the digest published its witness");
    let got = rev
        .fill_witness(&Plan::merge([]), &key, &snap, &WitnessScratch::default())
        .expect("the published entry");
    assert!(Arc::ptr_eq(&got, &published), "the published entry itself");
}

// ---------------------------------------------------------------------------
// Explain's indexes (plan 2026-11-03, item W2).
// ---------------------------------------------------------------------------

/// An answer's stdout, as text.
fn stdout(out: &JobOutput) -> String {
    match &out.actions[..] {
        [crate::engine::batch::output::WriteAction::Stdout(b), ..] => {
            String::from_utf8_lossy(b).into_owned()
        }
        other => panic!("precondition: the answer writes stdout first: {other:?}"),
    }
}

/// `explain --table <table> --field <field>`.
fn explain_field<'a>(f: &'a Fixture, table: &str, field: &str) -> Box<dyn Job + 'a> {
    use crate::engine::l5::explain::{ExplainJob, ExplainMode, ExplainRequest};
    Box::new(ExplainJob::new(
        ExplainRequest {
            inputs: &f.inputs,
            mode: ExplainMode::Field {
                table: table.to_string(),
                field: field.to_string(),
            },
            single_app: f.single_app,
        },
        false,
        None,
    ))
}

/// The explain indexes are filled once per revision, whatever the number
/// and the kind of explains (one routine, several, `--table/--field`), and
/// by explain alone; evicted, the next explain fills them again and still
/// answers its cold bytes.
///
/// DISCRIMINATION (break with Edit, run, revert, run; 2026-11-03): (e) the
/// artifact not retained across requests (`answer_timed` evicting
/// `explain_indexes` after each answer) fails "one fill for four explains"
/// (`Some(4)` against `Some(1)`; the B/S/F/P and cone tests fail too, on
/// `assert_answers_equal_cold`'s once-per-revision check). (A first try,
/// `Artifact::satisfies` false for it, is no test: the request refills it
/// forever and the run never ends.) (f) the
/// served view handed no indexes (`serve_plan` passing `None`) fails at the
/// first explain with "a served explain view carries its revision's explain
/// indexes" (it would otherwise build a second copy per request); each
/// reverted, it passes.
#[test]
fn explain_indexes_are_built_once_per_revision_and_refilled_after_eviction() {
    let f = witness_ws("DocBoth");
    let rev = revision(&f);
    for cmd in [Cmd::Query, Cmd::Digest, Cmd::Prove] {
        assert_eq!(answer(&rev, cmd, &f), cold(cmd, &f), "{cmd:?}");
    }
    assert_eq!(
        rev.fill_counts().get("explain_indexes"),
        None,
        "no other command reads them"
    );
    let jobs = [
        explain_many(&f, &["DocBoth"]),
        explain_many(&f, &["ReadThenQueue"]),
        explain_many(&f, &["DocBoth", "QueueTwice", "ReadThenQueue"]),
        explain_field(&f, "W Queue", "Code"),
    ];
    for job in &jobs {
        let warm = rev.answer(&**job, &Control::unreserved()).unwrap();
        assert_eq!(warm, crate::engine::batch::standalone_output(&**job));
        assert_eq!(warm.exit, 0, "precondition: {}", warm.message());
    }
    assert!(
        stdout(&crate::engine::batch::standalone_output(&*jobs[3])).contains("QueueMail"),
        "precondition: the field has a writer"
    );
    assert_eq!(
        rev.fill_counts().get("explain_indexes"),
        Some(&1),
        "one fill for four explains"
    );
    assert_eq!(rev.evict_kind("explain_indexes"), 1);
    for job in &jobs {
        let warm = rev.answer(&**job, &Control::unreserved()).unwrap();
        assert_eq!(
            warm,
            crate::engine::batch::standalone_output(&**job),
            "refilled"
        );
    }
    assert_eq!(rev.fill_counts().get("explain_indexes"), Some(&2));
}

/// How deep [`deep_ws`]'s call chain is: past `cross_rules::MAX_DEPTH`.
const DEEP_CHAIN: usize = 600;

/// A workspace whose `Deep0` starts a call chain deeper than the cross-call
/// rules' walk follows (its card states `cross-call-rules-truncated`), and
/// whose `Plain` inherits a callee's rule over a short chain (its card,
/// alone, states no truncation).
fn deep_ws() -> Fixture {
    let mut src = String::from(
        "table 50240 \"D Log\"\n{\n    fields { field(1; Id; Integer) { } field(2; Amount; Decimal) { } }\n    keys { key(PK; Id) { } }\n}\ncodeunit 50241 \"D Chain\"\n{\n",
    );
    for i in 0..DEEP_CHAIN {
        src.push_str(&format!(
            "    procedure Deep{i}()\n    begin\n        Deep{}();\n    end;\n\n",
            i + 1
        ));
    }
    src.push_str(&format!(
        "    procedure Deep{DEEP_CHAIN}()\n    begin\n    end;\n\n"
    ));
    src.push_str(
        "    procedure Plain(Amount: Decimal)\n    var\n        L: Record \"D Log\";\n    begin\n        CheckAmount(Amount);\n        L.Amount := Amount;\n        L.Insert();\n    end;\n\n    local procedure CheckAmount(Amount: Decimal)\n    begin\n        if Amount < 0 then\n            Error('Amount must not be negative');\n    end;\n}\n",
    );
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::write(
        dir.path().join("app.json"),
        r#"{"id":"24242424-0000-4000-8000-000000000240","name":"D","publisher":"t","version":"1.0.0.0"}"#,
    )
    .unwrap();
    std::fs::write(dir.path().join("src/D.al"), src).unwrap();
    Fixture {
        inputs: WorkspaceInputs::read(dir.path(), &READ),
        single_app: true,
        routine: "Plain",
        _dir: Some(dir),
    }
}

/// `body` on a thread with a large stack: the cross-call rules' walk
/// recurses once per call, [`DEEP_CHAIN`] deep (the binary's main thread has
/// the room; a test thread's default stack does not, in a debug build).
fn on_big_stack(body: impl FnOnce() + Send) {
    std::thread::scope(|s| {
        std::thread::Builder::new()
            .stack_size(1 << 30)
            .spawn_scoped(s, body)
            .expect("a test thread")
            .join()
            .unwrap_or_else(|p| std::panic::resume_unwind(p));
    });
}

/// The core correctness rule of W2: explain's per-revision indexes are
/// immutable, and each request walks them with a walk of its own, so an
/// answer never depends on what earlier requests did. Off one revision,
/// explain over `Deep0` (its walk stops: truncated) then over `Plain`
/// equals cold `Plain` (no truncation), and `Plain` then `Deep0` equals cold
/// `Deep0`; one request over both equals the cold run over both (one walk
/// shared by the run's cards, built in root-id order, as cold).
///
/// DISCRIMINATION (break with Edit, run, revert, run; 2026-11-03): (g) the
/// walk's stop shared across requests (a `truncated` flag kept in the
/// shared `CrossRulesIndex`, set by every walk over it and read by
/// `CrossRules::truncated`) fails `["Deep0", "Plain"]: Plain` (bytes
/// differ: Plain's card states `cross-call-rules-truncated`); reverted, it
/// passes.
#[test]
fn an_explain_answer_does_not_depend_on_earlier_requests() {
    on_big_stack(|| {
        let f = deep_ws();
        let cold_of =
            |roots: &[&str]| crate::engine::batch::standalone_output(&*explain_many(&f, roots));
        let (deep, plain, both) = (
            cold_of(&["Deep0"]),
            cold_of(&["Plain"]),
            cold_of(&["Deep0", "Plain"]),
        );
        const GAP: &str = "cross-call-rules-truncated";
        assert!(
            stdout(&deep).contains(GAP),
            "precondition: Deep0's walk stops"
        );
        assert!(
            !stdout(&plain).contains(GAP) && stdout(&plain).contains("must not be negative"),
            "precondition: Plain alone inherits CheckAmount's rule, untruncated: {}",
            stdout(&plain)
        );
        assert!(
            stdout(&both).contains(GAP),
            "precondition: the run over both stops"
        );
        for order in [["Deep0", "Plain"], ["Plain", "Deep0"]] {
            let rev = revision(&f);
            for root in order {
                let warm = rev
                    .answer(&*explain_many(&f, &[root]), &Control::unreserved())
                    .unwrap();
                let cold = if root == "Deep0" { &deep } else { &plain };
                assert_eq!(&warm, cold, "{order:?}: {root}");
            }
            let warm = rev
                .answer(&*explain_many(&f, &order), &Control::unreserved())
                .unwrap();
            assert_eq!(warm, cold_of(&order), "{order:?}: both in one request");
            assert_eq!(rev.fill_counts().get("explain_indexes"), Some(&1));
        }
    });
}

/// A producer reached over two call sites at one distance: `Twice` passes a
/// temporary record to `Fill` (which inserts into its parameter: a producer)
/// at two sites, so its state has two shortest links, and its own entry's
/// first hop is the lower-ranked edge's (plan 2026-11-03 L6: the emission
/// reads the entries back from the arena by `finalize`'s rule).
const TWO_SITES_SOURCE: &str = r#"table 50300 "P Rec"
{
    fields { field(1; Id; Integer) { } }
    keys { key(PK; Id) { } }
}
codeunit 50301 "P Lib"
{
    procedure Fill(var R: Record "P Rec")
    begin
        R.Id := 1;
        R.Insert();
    end;
}
codeunit 50302 "P Use"
{
    procedure Twice()
    var
        T: Record "P Rec" temporary;
        L: Codeunit "P Lib";
    begin
        L.Fill(T);
        L.Fill(T);
    end;

    procedure Outer()
    begin
        Twice();
    end;
}
"#;

fn two_sites_ws(routine: &'static str) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::write(
        dir.path().join("app.json"),
        r#"{"id":"23232323-0000-4000-8000-000000000302","name":"P","publisher":"t","version":"1.0.0.0"}"#,
    )
    .unwrap();
    std::fs::write(dir.path().join("src/P.al"), TWO_SITES_SOURCE).unwrap();
    Fixture {
        inputs: WorkspaceInputs::read(dir.path(), &READ),
        single_app: true,
        routine,
        _dir: Some(dir),
    }
}

/// The revision's published fold (filled first if absent).
fn fold_of(rev: &Revision) -> ReadySnapshot {
    rev.prepare_keys(&[ArtifactKey::Fold], &Control::unreserved())
        .expect("the fold fills");
    rev.snapshot()
}

/// `raw`'s entries as text: every subject sorted, materialized.
fn raw_text(raw: &InheritedFacts) -> String {
    let mut keys: Vec<&str> = raw.keys().collect();
    keys.sort_unstable();
    keys.into_iter()
        .map(|k| format!("{k} {:?}\n", raw.get(k).expect("listed").materialize()))
        .collect()
}

/// Every `Raw` artifact the revision holds, as `(subjects, text)`.
fn raw_artifacts(rev: &Revision) -> Vec<(BTreeSet<String>, String)> {
    let snap = rev.snapshot();
    snap.keys()
        .filter_map(|k| match (k, snap.get(k).map(|a| &a.value)) {
            (ArtifactKey::Raw(scope), Some(ArtifactValue::Raw(r))) => {
                Some((scope.clone(), raw_text(r)))
            }
            _ => None,
        })
        .collect()
}

/// Plan 2026-11-03 L6 (the on-demand emission's contract): the raw facts a
/// resident fold's retained cones emit, for EVERY routine of the model at
/// once, equal a raw-only composition's (`raw: All`, the one-shot path's
/// emission), entry for entry and key for key, on every r0 corpus app that
/// builds single-app, the rule-B cross-app fixture and the witness fixture
/// (a singleton over a recursive SCC, and that SCC's members).
///
/// DISCRIMINATION (break with Edit, run, revert, run; 2026-11-03): (a)
/// `resolved_entries_of` taking the HIGHEST-ranked shortest link (`rfind`)
/// fails on the two-sites fixture (`Twice`'s witness call site `cs1`
/// against `cs0`); without that fixture the break stayed green (no corpus
/// app has a state with two shortest links), which is why it is here. (b)
/// a recursive member's walk handed no sibling direct facts fails on
/// `ws-d7-event-cycle`. Each reverted, green.
#[test]
fn retained_emission_equals_a_composition_for_every_routine() {
    use crate::engine::l4::cone_derived::{ConeScope, WitnessScope};
    let corpus = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/r0-corpus");
    let mut dirs: Vec<std::path::PathBuf> = std::fs::read_dir(&corpus)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.join("app.json").exists())
        .collect();
    dirs.sort();
    let mut fixtures: Vec<Fixture> = Vec::new();
    for dir in &dirs {
        fixtures.push(Fixture {
            inputs: WorkspaceInputs::read(dir, &READ),
            single_app: true,
            routine: "",
            _dir: None,
        });
    }
    fixtures.push(rule_b(""));
    fixtures.push(witness_ws(""));
    fixtures.push(two_sites_ws(""));
    let (mut checked, mut inherited) = (0, 0);
    for f in &fixtures {
        let key = ViewKey::new(&f.inputs.workspace, f.single_app, false);
        let Ok(Some(built)) = build_view_model(&f.inputs, &key, REVISION_NEEDS, |_| {}) else {
            continue;
        };
        let rev = Revision::new(
            test_id(1),
            Arc::clone(&f.inputs),
            built.target,
            REVISION_NEEDS.assembly(),
        );
        let snap = fold_of(&rev);
        let Some(ArtifactValue::Fold(fold)) = snap.get(&ArtifactKey::Fold).map(|a| &a.value) else {
            panic!("the fold is published");
        };
        let model = rev.target().resolved();
        let all: BTreeSet<String> = model
            .workspace
            .routines
            .iter()
            .map(|r| r.id.clone())
            .collect();
        let (warm, report) = fold.cones.as_ref().expect("a cone store").emit_raw(
            &model.workspace.routines,
            &fold.caps.direct_full,
            fold.caps.pd.as_ref(),
            &all,
            &|_| None,
        );
        assert_eq!(report.adopted, 0);
        let (_, cold) = crate::engine::session::compose_cone(
            model,
            &crate::engine::session::relation_of(rev.target()),
            ConeScope {
                derived: false,
                raw: WitnessScope::All,
            },
        );
        assert_eq!(
            raw_text(&warm),
            raw_text(&cold),
            "{}: the retained emission equals the composition's",
            f.inputs.workspace.display()
        );
        checked += 1;
        inherited += warm
            .keys()
            .filter(|k| !warm.materialized(k).is_empty())
            .count();
    }
    assert!(
        checked >= 50,
        "precondition: most fixtures build ({checked})"
    );
    assert!(
        inherited >= 100,
        "precondition: many routines inherit facts ({inherited})"
    );
}

/// [`retained_emission_equals_a_composition_for_every_routine`] on the
/// cross-app CDO model (skips without `CDO_WS`, panics under
/// `ENFORCE_CDO_WS=1`): every primary routine and the first 200 dependency
/// routines (by internal id) emitted from a resident fold's cones equal a
/// composition's raw half over the same subjects.
#[test]
fn retained_emission_equals_a_composition_on_cdo() {
    use crate::engine::l4::cone_derived::{ConeScope, WitnessScope};
    let Some(ws) = crate::cdo::cdo_ws_or_enforce() else {
        return;
    };
    crate::big_stack::run_with_big_stack(move || {
        let inputs = WorkspaceInputs::read(&ws, &READ);
        let key = ViewKey::new(&inputs.workspace, false, false);
        let built = build_view_model(&inputs, &key, REVISION_NEEDS, |_| {})
            .expect("cross-app CDO builds")
            .expect("a model view");
        let rev = Revision::new(
            test_id(1),
            Arc::clone(&inputs),
            built.target,
            REVISION_NEEDS.assembly(),
        );
        let snap = fold_of(&rev);
        let Some(ArtifactValue::Fold(fold)) = snap.get(&ArtifactKey::Fold).map(|a| &a.value) else {
            panic!("the fold is published");
        };
        let model = rev.target().resolved();
        let mut subjects = crate::engine::session::WitnessRequest::primary().subjects(model);
        let primary = subjects.len();
        let dependency: BTreeSet<String> = model
            .workspace
            .routines
            .iter()
            .filter(|r| model.routine_role(r) == Role::Dependency)
            .map(|r| r.id.clone())
            .collect();
        subjects.extend(dependency.into_iter().take(200));
        assert!(
            primary > 1000,
            "precondition: CDO's primary routines ({primary})"
        );
        assert!(
            subjects.len() > primary,
            "precondition: dependency subjects"
        );
        let (warm, report) = fold.cones.as_ref().expect("a cone store").emit_raw(
            &model.workspace.routines,
            &fold.caps.direct_full,
            fold.caps.pd.as_ref(),
            &subjects,
            &|_| None,
        );
        assert!(report.emitted as usize > primary, "{report:?}");
        let (_, cold) = crate::engine::session::compose_cone(
            model,
            &crate::engine::session::relation_of(rev.target()),
            ConeScope {
                derived: false,
                raw: WitnessScope::Subjects(subjects),
            },
        );
        let (warm, cold) = (raw_text(&warm), raw_text(&cold));
        assert!(
            warm == cold,
            "the retained emission equals the composition's on CDO"
        );
    });
}

/// Plan 2026-11-03 L6, on one revision: the fold emits no raw facts; a
/// request on a primary root whose raw facts were never emitted fills ONE
/// `Raw` artifact for its view, emitted from the fold's cones (equal to the
/// one-shot emission for the same subjects), and answers what a cold run
/// answers; after the raw and the tree are evicted, the next request
/// re-emits and still equals cold; a tree refilled over a published `Raw`
/// emits nothing; a wider view adopts the published subject's entry and
/// emits only the new one. Singleton roots, a recursive member (`RunReportRec`)
/// and the cross-app rule-B fixture.
///
/// DISCRIMINATION (break with Edit, run, revert, run; 2026-11-03): (c) the
/// emission skipped (`emit_raw` recording no entry for an emitted subject)
/// fails `["DocBoth", "RunReportRec"]: first` (bytes differ from cold: the
/// root's tree has no inherited facts); reverted, green.
#[test]
fn a_primary_root_gets_its_raw_facts_on_demand() {
    let cases: [(Fixture, &[&str]); 3] = [
        (witness_ws("DocBoth"), &["DocBoth", "RunReportRec"]),
        (
            rule_b("WriteThenCommit"),
            &["WriteThenCommit", "FillAndPost"],
        ),
        (
            witness_ws("ReadThenQueue"),
            &["ReadThenQueue", "QueueTwice"],
        ),
    ];
    for (f, roots) in &cases {
        let rev = revision(f);
        let _ = fold_of(&rev);
        assert!(
            raw_artifacts(&rev).is_empty(),
            "{}: the fold emitted no raw facts",
            f.routine
        );
        // The first root's view: one emission, on demand.
        let one = explain_many(f, &roots[..1]);
        let warm = rev.answer(&*one, &Control::unreserved()).unwrap();
        assert_eq!(
            warm,
            crate::engine::batch::standalone_output(&*one),
            "{roots:?}: first"
        );
        assert_eq!(warm.exit, 0, "precondition: {}", warm.message());
        let raws = raw_artifacts(&rev);
        assert_eq!(raws.len(), 1, "one view, one raw: {raws:?}");
        let (scope, text) = &raws[0];
        assert!(
            text.contains("inherited"),
            "precondition: {:?} inherits facts: {text}",
            roots[0]
        );
        let model = rev.target().resolved();
        let oracle = crate::engine::session::emit_raw_ids(
            model,
            &crate::engine::session::relation_of(rev.target()),
            scope.clone(),
        );
        assert_eq!(*text, raw_text(&oracle), "the one-shot emission");
        let (emitted, adopted) = rev.raw_counts();
        assert_eq!((emitted, adopted), (scope.len() as u64, 0));
        // Evicted, then the same view again: re-emitted, equal to cold.
        assert!(rev.evict_kind("raw") + rev.evict_kind("tree") + rev.evict_kind("witness") >= 2);
        assert_eq!(
            answer(&rev, Cmd::Prove, f),
            cold(Cmd::Prove, f),
            "after eviction"
        );
        assert_eq!(rev.raw_counts().0, 2 * scope.len() as u64, "re-emitted");
        // A tree refilled over the published raw emits nothing.
        rev.evict_kind("tree");
        rev.evict_kind("witness");
        assert_eq!(
            answer(&rev, Cmd::Digest, f),
            cold(Cmd::Digest, f),
            "tree refill"
        );
        assert_eq!(
            rev.fill_counts().get("raw"),
            Some(&2),
            "no raw fill for a tree refill"
        );
        // A wider view adopts the first root's entry, emits the second's.
        let both = explain_many(f, roots);
        let warm = rev.answer(&*both, &Control::unreserved()).unwrap();
        assert_eq!(
            warm,
            crate::engine::batch::standalone_output(&*both),
            "{roots:?}: both"
        );
        let (_, adopted) = rev.raw_counts();
        assert_eq!(
            adopted,
            scope.len() as u64,
            "the published subject is adopted"
        );
        for (scope, text) in raw_artifacts(&rev) {
            let oracle = crate::engine::session::emit_raw_ids(
                model,
                &crate::engine::session::relation_of(rev.target()),
                scope.clone(),
            );
            assert_eq!(text, raw_text(&oracle), "{scope:?}: the one-shot emission");
        }
    }
}

/// Plan 2026-11-03 L6: a reader that needs every primary subject's raw facts
/// demands them explicitly — no served command does (each reads its roots'
/// view), but a plan over every primary routine (built by hand, as the
/// dependency-root test builds its plan) fills one `Raw` artifact holding
/// every primary routine that is a graph node, and its effects equal a cold
/// one-shot view's over the same plan.
#[test]
fn a_view_over_every_primary_routine_gets_them_all() {
    let f = rule_b("WriteThenCommit");
    let rev = revision(&f);
    let model = rev.target().resolved();
    let primary: BTreeSet<String> = model
        .workspace
        .routines
        .iter()
        .filter(|r| model.routine_role(r) == Role::Primary)
        .map(|r| r.stable_routine_id.clone())
        .collect();
    assert!(primary.len() >= 5, "precondition: {primary:?}");
    let plan = || Plan::merge([(JobId(0), explain_plan(primary.clone()))]);
    let (view, _) = rev
        .serve_plan(&plan(), &Control::unreserved(), &mut 0)
        .unwrap();
    let warm = effects_text(view.effects(JobId(0)));
    let mut cold_view = View::new(
        build_view_model(
            &f.inputs,
            &ViewKey::new(&f.inputs.workspace, false, false),
            REVISION_NEEDS,
            |_| {},
        )
        .unwrap()
        .unwrap()
        .target,
    );
    cold_view.prepare(&plan());
    assert_eq!(warm, effects_text(cold_view.effects(JobId(0))));
    let raws = raw_artifacts(&rev);
    assert_eq!(raws.len(), 1, "one view, one emission");
    let ids: BTreeSet<String> = crate::engine::session::WitnessRequest::primary().subjects(model);
    assert_eq!(
        raws[0].0, ids,
        "the view's subjects are every primary routine"
    );
    assert_eq!(rev.raw_counts(), (ids.len() as u64, 0));
}

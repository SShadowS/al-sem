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
        Cmd::Digest => Box::new(DigestJob::new(
            DigestRequest {
                inputs: &f.inputs,
                changed_files: None,
                changed_routines: Some(vec![f.routine.to_string()]),
                diff_text: None,
                alsem_ver: "test",
                deterministic: true,
                single_app: f.single_app,
                tests: false,
                include_reads: false,
                diff_root: DiffRoot::Workspace,
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
            human: false,
            out: None,
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
    Revision::new(test_id(1), Arc::clone(&f.inputs), built.target)
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
/// Digest]: Digest on ForwardedAfterValidate` (bytes differ).
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
            assert_eq!(n("raw"), 0, "every root is primary");
            assert!(n("det") <= 2, "{order:?}: {fills:?}");
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
/// plan is built by hand) is outside the fold's primary emission, so the
/// store fills a `Raw` artifact for it, and its effects equal a cold
/// one-shot view's over the same plan, after a primary request on the same
/// revision.
///
/// DISCRIMINATION (2026-11-02): borrowing the fold's primary map for the
/// dependency root (the `is_subset` test forced true) leaves its tree
/// without the root's raw facts: `assert_eq!(warm, cold)` fails (empty
/// against the inherited insert).
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
    assert_eq!(
        fills.get("raw"),
        Some(&1),
        "one scoped emission for the dependency root"
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

/// A request that stops before its ready snapshot answers `cancelled` and
/// publishes nothing partial; the next request on the revision answers.
#[test]
fn a_cancelled_request_stops_between_phases() {
    let f = r0("ws-d37", "ForwardedAfterValidate");
    let rev = revision(&f);
    let stop = || Some(crate::engine::resident::store::Stop::Cancelled);
    let ctl = Control {
        stop: &stop,
        reserve: &|_| Ok(()),
    };
    let err = rev.answer(&*job(Cmd::Prove, &f), &ctl).unwrap_err();
    assert_eq!(err, PrepareError::Cancelled);
    assert!(
        rev.snapshot().is_empty(),
        "nothing filled for a cancelled request"
    );
    assert_eq!(answer(&rev, Cmd::Prove, &f), cold(Cmd::Prove, &f));
}

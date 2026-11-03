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
            assert_eq!(n("raw"), 0, "every root is primary");
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
/// DISCRIMINATION (2026-11-02): dropping the assembly pins in
/// `ensure_prepared` (no `pins += 1`) fails it: an eviction lands between a
/// request's phases, and the request ends "missing from the ready snapshot"
/// (or a run without its det) instead of answering: 5 of 5 runs failed
/// (6 rounds failed 1 run in 3, hence 40). Plan 2026-11-03 W2 (2026-11-03):
/// the explain indexes' kind name drifting from the name eviction uses
/// (`ArtifactKey::kind` returning `"explain-indexes"`) fails "explain_indexes
/// refilled after eviction" (one fill: nothing ever evicted it); reverted,
/// it passes.
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
    // ask the same root at once (they share its witness key).
    let (go_digest, digest_rx) = std::sync::mpsc::sync_channel::<()>(0);
    let (go_heavy, heavy_rx) = std::sync::mpsc::sync_channel::<()>(0);
    let (go_light, light_rx) = std::sync::mpsc::sync_channel::<()>(0);
    let (go_evict, evict_rx) = std::sync::mpsc::sync_channel::<()>(0);
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
            }
            outs
        });
        let digests = s.spawn(move || {
            let mut outs = Vec::new();
            while digest_rx.recv().is_ok() {
                outs.push(answer(rev, Cmd::Digest, f));
            }
            outs
        });
        let light = s.spawn(move || {
            let mut outs = Vec::new();
            while light_rx.recv().is_ok() {
                outs.push(answer(rev, Cmd::Query, f));
            }
            outs
        });
        let evictor = s.spawn(move || {
            let mut evicted = 0;
            while evict_rx.recv().is_ok() {
                for _ in 0..50 {
                    for k in [
                        "witness",
                        "explain_indexes",
                        "tree",
                        "tree_base",
                        "run",
                        "det",
                        "fold",
                        "l4",
                        "rules",
                        "ordering",
                    ] {
                        evicted += rev.evict_kind(k);
                    }
                    std::thread::yield_now();
                }
            }
            evicted
        });
        for _ in 0..ROUNDS {
            let _ = go_evict.send(());
            let _ = go_heavy.send(());
            let _ = go_digest.send(());
            let _ = go_light.send(());
        }
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
        assert!(
            evictor.join().unwrap() > 0,
            "precondition: evictions happened"
        );
    });
    // The tree store's artifacts (stage 5b) churn like the others: a
    // request's tree is pinned through its reads, and an evicted tree or
    // base is filled again, never read half-built. So do the witnesses
    // (plan 2026-11-03 W): one evicted between its claim and its
    // publication is discarded (the generation moved) and filled again; and
    // explain's indexes (W2), read by the explains of the heavy thread.
    for kind in ["l4", "tree", "tree_base", "witness", "explain_indexes"] {
        assert!(
            rev.fill_counts().get(kind).copied().unwrap_or(0) > 1,
            "{kind} refilled after eviction: {:?}",
            rev.fill_counts()
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

/// The clone rule (plan 2026-11-03 W, §6 "aliasing"): prove's literal reach
/// prunes `DocBoth`'s dead witness path off ITS copy of the entry; a digest
/// after it on the same revision reads the unpruned memoized entry, as a
/// cold digest does.
///
/// DISCRIMINATION (2026-11-03): (b) memoizing the entry AFTER prove's pass
/// (the witness fill running `literal_reach_pass` over its batch when the
/// request's plan has `gaps_in_cone`) fails `digest after prove` (bytes
/// differ: the dead path is pruned); reverted, it passes. The literal break
/// "hand the read the stored entry instead of a clone" does not compile:
/// the value is behind an `Arc`, so a read can only clone it.
#[test]
fn a_digest_after_a_prove_reads_the_unpruned_witness() {
    let f = witness_ws("DocBoth");
    let digest = cold(Cmd::Digest, &f);
    let text = String::from_utf8_lossy(match &digest.actions[..] {
        [crate::engine::batch::output::WriteAction::Stdout(b), ..] => b,
        other => panic!("precondition: digest writes stdout first: {other:?}"),
    })
    .into_owned();
    assert!(
        text.matches("RunReportRec").count() >= 2,
        "precondition: DocBoth's digest lists the dead path too: {text}"
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

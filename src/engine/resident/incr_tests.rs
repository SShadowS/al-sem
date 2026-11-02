//! Stage 5's cold oracle (design §11, T-incr end to end): a resident session
//! over a temporary fixture workspace, edited step by step with hand-stated
//! edits; after each step every served answer equals a cold standalone run
//! over a fresh read of the edited disk, and every retained analysis
//! artifact the step reused equals the one a cold build computes.

use std::path::Path;
use std::sync::Arc;

use super::*;
use crate::engine::batch::job::Job;
use crate::engine::batch::output::JobOutput;
use crate::engine::incremental::L4Projection;
use crate::engine::l4::analysis::{L4Request, build_l4};
use crate::engine::resident::revision::{ArtifactKey, ArtifactValue};
use crate::engine::resident::store::Control;
use crate::engine::session::relation_of;

/// A served command.
#[derive(Clone, Copy, Debug)]
pub(super) enum Cmd {
    Query,
    Explain,
    Prove,
    Digest,
}

pub(super) const ALL: [Cmd; 4] = [Cmd::Query, Cmd::Explain, Cmd::Prove, Cmd::Digest];

pub(super) fn job<'a>(
    cmd: Cmd,
    inputs: &'a Arc<WorkspaceInputs>,
    routine: &'a str,
) -> Box<dyn Job + 'a> {
    use crate::engine::l5::digest_cli::{DiffRoot, DigestJob, DigestRequest};
    use crate::engine::l5::explain::{ExplainJob, ExplainMode, ExplainRequest};
    match cmd {
        Cmd::Query => Box::new(crate::engine::l4::effect_query_cli::QueryJob::effects(
            inputs, routine, "test", true, false,
        )),
        Cmd::Explain => Box::new(ExplainJob::new(
            ExplainRequest {
                inputs,
                mode: ExplainMode::Routines(vec![routine.to_string()]),
                single_app: false,
            },
            false,
            None,
        )),
        Cmd::Prove => Box::new(crate::engine::l5::prove::ProveJob {
            detectors: crate::engine::l5::detectors::default_detectors(),
            inputs,
            routine_selector: routine,
            question_text: "may-commit",
            alsem_ver: "test",
            deterministic: true,
            single_app: false,
            closed_world: false,
            human: false,
            out: None,
        }),
        Cmd::Digest => Box::new(DigestJob::new(
            DigestRequest {
                inputs,
                changed_files: None,
                changed_routines: Some(vec![routine.to_string()]),
                diff_text: None,
                alsem_ver: "test",
                deterministic: true,
                single_app: false,
                tests: false,
                include_reads: false,
                diff_root: DiffRoot::Workspace,
            },
            false,
            None,
        )),
    }
}

fn never() -> Option<Stop> {
    None
}

/// The session's published revision after a `fresh` pin.
pub(super) fn pin(s: &Arc<ResidentSession>) -> Arc<Revision> {
    match s.pin(Freshness::Fresh, &never).unwrap().built {
        Built::Revision(r) => r,
        Built::Failed(f) => panic!("the build failed: {:?}", f.failure),
    }
}

/// Every command on every routine answered off `rev`, each equal to its cold
/// standalone run over a fresh read of `ws`; at least one answer exits 0.
pub(super) fn assert_answers_equal_cold(step: &str, rev: &Revision, ws: &Path, routines: &[&str]) {
    let fresh = WorkspaceInputs::read(ws, &REVISION_READ);
    let mut answered = false;
    for routine in routines {
        for cmd in ALL {
            let warm: JobOutput = rev
                .answer(&*job(cmd, &rev.inputs, routine), &Control::unreserved())
                .unwrap_or_else(|e| panic!("{step}: {cmd:?} {routine}: {e:?}"));
            let cold = crate::engine::batch::standalone_output(&*job(cmd, &fresh, routine));
            assert_eq!(warm, cold, "{step}: {cmd:?} on {routine} equals cold");
            answered |= warm.exit == 0;
        }
    }
    assert!(answered, "precondition: {step}: some answer exits 0");
}

/// The revision's published L4 solve projected, and a cold full solve's over
/// the same model.
pub(super) fn l4_and_cold(rev: &Revision) -> (L4Projection, L4Projection) {
    let snap = rev.snapshot();
    let Some(ArtifactValue::L4(s)) = snap.get(&ArtifactKey::L4).map(|a| &a.value) else {
        panic!("the L4 solve is published");
    };
    let warm = L4Projection::of(&s.l4, &s.core.0);
    let target = rev.target();
    let cross = target.cross_app_inputs();
    let (l4, core) = build_l4(
        target.resolved(),
        &relation_of(target),
        L4Request {
            extra_diagnostics: vec![],
            control: None,
        },
        cross.as_ref(),
    );
    (warm, L4Projection::of(&l4, &core.0))
}

/// The pd arena as text: producers, states, links and edges in storage
/// order, `by_routine` sorted.
fn pd_text(pd: &crate::engine::l4::pd_solve::ConeArena) -> String {
    let by: std::collections::BTreeMap<&str, &Vec<usize>> =
        pd.by_routine.iter().map(|(k, v)| (&**k, v)).collect();
    format!(
        "{:?}\n{:?}\n{:?}\n{:?}\n{by:?}",
        pd.producers, pd.states, pd.links, pd.edges
    )
}

/// The revision's published fold's pd arena and a cold composition's over
/// the same model, as text; and the fold's reuse report.
pub(super) fn pd_and_cold(
    rev: &Revision,
) -> (String, String, crate::engine::l4::pd_solve::PdSolveReport) {
    let snap = rev.snapshot();
    let Some(ArtifactValue::Fold(f)) = snap.get(&ArtifactKey::Fold).map(|a| &a.value) else {
        panic!("the fold is published");
    };
    let warm = pd_text(f.caps.pd.as_ref().expect("the fold keeps its arena"));
    let target = rev.target();
    let model = target.resolved();
    let scope = crate::engine::l4::cone_derived::ConeScope {
        derived: true,
        raw: crate::engine::session::WitnessRequest::primary().scope(model),
    };
    let (caps, _) = crate::engine::session::compose_cone(model, &relation_of(target), scope);
    let cold = pd_text(caps.pd.as_ref().expect("a composition keeps its arena"));
    (warm, cold, f.pd_report)
}

fn rule_b_session() -> (tempfile::TempDir, Arc<ResidentSession>) {
    let dir = tempfile::tempdir().unwrap();
    crate::engine::l5::test_support::write_fixture_rule_b(dir.path());
    let s = Arc::new(ResidentSession::new(
        dir.path().to_path_buf(),
        false,
        "test".into(),
    ));
    (dir, s)
}

pub(super) fn edit(ws: &Path, file: &str, from: &str, to: &str) {
    let p = ws.join(file);
    let t = std::fs::read_to_string(&p).unwrap();
    assert_eq!(
        t.matches(from).count(),
        1,
        "precondition: one {from:?} in {file}"
    );
    std::fs::write(&p, t.replacen(from, to, 1)).unwrap();
}

const RULE_B_ROOTS: &[&str] = &["WriteThenCommit", "WriteCallCommit", "FillAndPost"];

/// Design §6.10 "Producers" end to end (T-pd-out on a served revision): the
/// rule-B workspace's `FillAndPost` passes a TEMPORARY Customer to the
/// dependency's `Fill`, which inserts into its parameter (a producer). A body
/// edit elsewhere (`WriteCallCommit`) re-solves no producer: the fold copies
/// every producer's states from the previous revision's arena. Then
/// `FillAndPost`'s record becomes PHYSICAL (the reviews' A→C→P counterexample:
/// the caller's argument changes from a temporary to a physical record):
/// `Fill`'s producer is re-solved, its state at `FillAndPost` changes, and
/// the answers on `FillAndPost` change with it. After each step the fold's pd
/// arena equals a cold composition's over the same model, and every served
/// answer equals its cold standalone run.
///
/// DISCRIMINATION (2026-11-02): treating every producer as reusable
/// (`reusable` ignoring both the reachability and the old-arena hits) fails
/// "temp → physical: Explain on FillAndPost equals cold" (the stale
/// temporary state at `FillAndPost` survives into the card); reverted, green.
#[test]
fn fold_reuses_untouched_producers_and_equals_cold() {
    let (dir, s) = rule_b_session();
    let ws = dir.path();
    let rev = pin(&s);
    assert_answers_equal_cold("cold", &rev, ws, RULE_B_ROOTS);
    let (warm, cold, report) = pd_and_cold(&rev);
    assert_eq!(warm, cold, "cold: the arena");
    assert!(
        report.producers > 0,
        "precondition: the fixture has producers"
    );
    assert_eq!(report.reused, 0, "cold: nothing to reuse");

    edit(
        ws,
        "src/Ws.al",
        "        L.Insert();\n        Lib.MaybeCommit(true);\n        Commit();",
        "        L.Insert();\n        L.Modify();\n        Lib.MaybeCommit(true);\n        Commit();",
    );
    let rev = pin(&s);
    assert_eq!(s.stats().last_build_class.as_deref(), Some("B"));
    assert_answers_equal_cold("body edit", &rev, ws, RULE_B_ROOTS);
    let (warm, cold, report) = pd_and_cold(&rev);
    assert_eq!(warm, cold, "body edit: the arena equals cold");
    assert_eq!(
        report.reused, report.producers,
        "body edit: every producer copied ({report:?})"
    );

    let before = cold;
    edit(
        ws,
        "src/Ws.al",
        "        T: Record Customer temporary;",
        "        T: Record Customer;",
    );
    let rev = pin(&s);
    assert_answers_equal_cold("temp → physical", &rev, ws, &["FillAndPost"]);
    let (warm, cold, report) = pd_and_cold(&rev);
    assert_ne!(cold, before, "precondition: the edit moves the arena");
    assert_eq!(warm, cold, "temp → physical: the arena equals cold");
    assert!(
        report.reused < report.producers,
        "temp → physical: Fill's producer re-solved ({report:?})"
    );
}

/// Design §6.9 (seeded L4, T-incr's L4 half): across a body edit (B), a
/// declaration edit (S) and a package-identity change (P) of the rule-B
/// workspace, each revision's solve starts from the previous revision's
/// (B and S: some SCCs seeded, none full; P: the seed key differs, a full
/// solve), its projection equals a cold full solve over the same model, and
/// every served answer equals its cold standalone run.
///
/// DISCRIMINATION (break with Edit, run, revert, run; 2026-11-02):
/// building every revision with `Revision::new` (no seed from the previous
/// revision) fails "B: the solve was seeded"; dropping the seed-key filter in
/// `Revision::seeded` fails "P: a full solve" (the package-identity change
/// leaves the solve's own digests' global half equal, so only the key
/// refuses the seed).
#[test]
fn seeded_l4_after_b_s_and_p_equals_a_cold_solve() {
    let (dir, s) = rule_b_session();
    let ws = dir.path();
    let rev = pin(&s);
    assert_answers_equal_cold("cold", &rev, ws, RULE_B_ROOTS);
    let (_, _, full) = rev.l4_report().expect("the cold revision solved");
    assert!(full, "cold: a full solve");
    let (warm, cold) = l4_and_cold(&rev);
    assert_eq!(warm, cold, "cold: the solve");

    // B: WriteCallCommit's body changes (it now also modifies the log).
    edit(
        ws,
        "src/Ws.al",
        "        L.Insert();\n        Lib.MaybeCommit(true);\n        Commit();",
        "        L.Insert();\n        L.Modify();\n        Lib.MaybeCommit(true);\n        Commit();",
    );
    let prev = rev;
    let rev = pin(&s);
    assert_eq!(s.stats().last_build_class.as_deref(), Some("B"));
    assert!(rev.has_pending_seed(), "B: the previous solve is the seed");
    drop(prev);
    assert_answers_equal_cold("B", &rev, ws, RULE_B_ROOTS);
    let (seeded, total, full) = rev.l4_report().expect("B solved");
    assert!(
        !full && seeded > 0,
        "B: the solve was seeded ({seeded} of {total})"
    );
    assert!(
        seeded < total,
        "precondition: B re-solves the edited routine's SCC"
    );
    assert!(!rev.has_pending_seed(), "B: the used seed is released");
    let (warm, cold) = l4_and_cold(&rev);
    assert_eq!(warm, cold, "B: the seeded solve equals a cold one");

    // S: a procedure is added, and CallWrite now commits.
    edit(
        ws,
        "src/Ws.al",
        "    procedure CallWrite()\n    var\n        Lib: Codeunit \"Dep Rec Lib\";\n    begin\n        Lib.Write();\n    end;",
        "    procedure CallWrite()\n    var\n        Lib: Codeunit \"Dep Rec Lib\";\n    begin\n        Lib.Write();\n        Added();\n    end;\n\n    procedure Added()\n    begin\n        Commit();\n    end;",
    );
    let rev = pin(&s);
    assert_eq!(s.stats().last_build_class.as_deref(), Some("S"));
    assert_answers_equal_cold("S", &rev, ws, &["CallWrite", "WriteThenCommit"]);
    let (seeded, total, full) = rev.l4_report().expect("S solved");
    assert!(
        !full && seeded > 0,
        "S: the solve was seeded ({seeded} of {total})"
    );
    let (warm, cold) = l4_and_cold(&rev);
    assert_eq!(warm, cold, "S: the seeded solve equals a cold one");

    // P: the app's version moves (a package-identity field).
    edit(
        ws,
        "app.json",
        "\"version\":\"1.0.0.0\",\"runtime\"",
        "\"version\":\"1.0.0.1\",\"runtime\"",
    );
    let rev = pin(&s);
    assert_eq!(s.stats().last_build_class.as_deref(), Some("P"));
    assert_answers_equal_cold("P", &rev, ws, RULE_B_ROOTS);
    let (_, _, full) = rev.l4_report().expect("P solved");
    assert!(full, "P: a full solve");
    let (warm, cold) = l4_and_cold(&rev);
    assert_eq!(warm, cold, "P: the solve");
}

//! Issue 18 — d50's medium→info DEMOTION had no corpus coverage at all, and the
//! r4 golden alone cannot give it any.
//!
//! d50 escalates a checked-Run finding to `medium` when some routine on the span
//! holds an explicit `Commit()` in its OWN body AND that commit passes all four
//! caps of `is_explicit_commit_proven_effective` (`d50.rs:132-204`, applied at
//! the call site `:341-344`). Cap 2
//! rejects a committer whose root classification carries a kind from
//! `D50_UNTRUSTED_ROOT_KINDS`, `onrun-codeunit` among them.
//!
//! ## Why a severity value cannot pin this, and this test must
//!
//! `info` is what d50 emits in BOTH of these worlds:
//!
//!   * the escalation witness is present and cap 2 DEMOTED it — the behaviour
//!     under test; and
//!   * there is no escalation witness on the span at all — nothing happened.
//!
//! MEASURED during the spec panel: with `ws-d50-medium`'s B `Commit()` removed
//! line-count-preservingly, B's r4 golden line is BYTE-IDENTICAL — same id, same
//! fingerprint, same `info`, same line. So the golden's `info` is not evidence
//! that the demotion ran; that is verbatim the rot mode this issue exists to
//! close, and it would reappear inside the very fixture added to close it.
//!
//! These tests pin the PRECONDITION directly, from the two fields d50 itself
//! reads (`d50.rs:223-228` and `:341`), so the golden's `info` becomes
//! attributable.
//!
//! ## Two lookups over `ctx.transaction_spans`, not one object
//!
//! No span can be both kinds: `seed_kind` is a single field
//! (`transaction_spans.rs:47`). And on a `CheckedRunImplicit` span
//! `commit_routine_id` is the SEED routine (`transaction_spans.rs:437`), NOT the
//! committer — asserting "B's OnRun is that span's `commit_routine_id`" yields
//! `RunChecked` and a red test whose tempting "fix" is to weaken the assertion.
//! So:
//!
//!   1. the `CheckedRunImplicit` span whose `commit_routine_id` is B's SEED has
//!      B's `OnRun` id in `routines_in_span`; and
//!   2. a separate `ExplicitCommit` span exists whose `commit_routine_id` IS B's
//!      `OnRun` id.
//!
//! ## Everything here is pinned to B by OBJECT-QUALIFIED id
//!
//! `ws-d50-medium` holds THREE `OnRun` triggers (B's, C's, and the shared
//! worker's) and TWO routines named `PostDoc` and `RunChecked` each — A and B
//! are ONE-LINE twins, so their bodies are byte-identical and only the object
//! header and the committer's declaration line separate them. A lookup by NAME,
//! by KIND, or by "some `ExplicitCommit` span exists in this fixture" would be
//! satisfied by C's rows while B's precondition had silently vanished — the
//! identical rot mode above. Every lookup below goes through [`routine_id`],
//! which is object-number qualified.
//!
//! ## Stated limit: these tests pin "capped", not "cap 2 capped"
//!
//! What is asserted is witness-present + B `info` + A `medium`. Caps 1/3/4 are
//! excluded only because A shares every property they read (`body_available` is
//! asserted directly in [`routine_id`], neither committer carries an attribute,
//! neither object carries `InherentCommitBehavior`), so a cap-1/3/4 regression
//! flips A too and is caught there. A regression that made cap 1 or 3 fire
//! **only for triggers** would keep both tests green with cap 2 dead. The live
//! evidence that cap 2 is today's active rejector is the D1 discrimination proof
//! (comment `"onrun-codeunit"` out of `D50_UNTRUSTED_ROOT_KINDS` and B flips to
//! `medium`), not anything asserted here.

use std::path::{Path, PathBuf};

use al_sem::engine::l3::l3_workspace::{
    L3Resolved, L3Routine, assemble_and_resolve_workspace_default,
};
use al_sem::engine::l5::detector_context::{DetectorContext, build_detector_context};
use al_sem::engine::l5::detectors::registered_detectors;
use al_sem::engine::l5::finding::Finding;
use al_sem::engine::l5::registry::{run_detectors, substrate};
use al_sem::engine::l5::transaction_spans::{SeedKind, TransactionSpan};

const FIXTURE: &str = "ws-d50-medium";
const DETECTOR: &str = "d50-checked-run-implicit-commit";

/// Codeunit A — `local procedure` committer, no root classification, UNCAPPED.
const OBJ_A: i64 = 50230;
/// Codeunit B — `trigger OnRun()` committer, `onrun-codeunit`, CAPPED.
const OBJ_B: i64 = 50231;
/// Codeunit C — both kinds of committer on one span.
const OBJ_C: i64 = 50232;

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("r0-corpus")
        .join(FIXTURE)
}

fn resolve_fixture() -> L3Resolved {
    let dir = fixture_dir();
    assert!(
        dir.is_dir(),
        "fixture workspace {} is missing",
        dir.display()
    );
    assemble_and_resolve_workspace_default(&dir)
        .unwrap_or_else(|| panic!("workspace assembly returned None for {}", dir.display()))
}

/// The assembled routine id for `name` **inside codeunit `object_number`**.
///
/// Object-qualified on purpose: see the module header. Also the anti-degenerate
/// guard — a fixture that failed to parse assembles zero routines, and every
/// assertion below would then be vacuous rather than red.
fn routine_id(resolved: &L3Resolved, object_number: i64, name: &str) -> String {
    let matches: Vec<&L3Routine> = resolved
        .workspace
        .routines
        .iter()
        .filter(|r| r.object_number == object_number && r.name == name)
        .collect();
    let assembled: Vec<String> = resolved
        .workspace
        .routines
        .iter()
        .map(|r| format!("{}::{} ({})", r.object_number, r.name, r.kind))
        .collect();
    assert_eq!(
        matches.len(),
        1,
        "expected exactly 1 assembled routine named {name:?} in codeunit {object_number}; \
         assembled routines: {assembled:?}"
    );
    let r = matches[0];
    assert!(
        r.body_available,
        "routine {object_number}::{name} assembled without a body — no callsite, no seed, \
         and every assertion below would be vacuous"
    );
    r.id.clone()
}

/// `object_number::name` for a routine id, for failure messages.
fn label_of(resolved: &L3Resolved, routine_id: &str) -> String {
    resolved
        .workspace
        .routines
        .iter()
        .find(|r| r.id == routine_id)
        .map(|r| format!("{}::{}", r.object_number, r.name))
        .unwrap_or_else(|| format!("<unknown {routine_id}>"))
}

/// The UNIQUE `CheckedRunImplicit` span seeded in `seed_routine_id`.
fn checked_span_of<'c>(ctx: &'c DetectorContext<'_>, seed_routine_id: &str) -> &'c TransactionSpan {
    let spans: Vec<&TransactionSpan> = ctx
        .transaction_spans
        .iter()
        .filter(|s| {
            s.seed_kind == SeedKind::CheckedRunImplicit && s.commit_routine_id == seed_routine_id
        })
        .collect();
    assert_eq!(
        spans.len(),
        1,
        "expected exactly 1 CheckedRunImplicit span seeded in {seed_routine_id}, found {}",
        spans.len()
    );
    spans[0]
}

/// Run the REGISTERED d50 over the fixture.
fn d50_findings(resolved: &L3Resolved) -> Vec<Finding> {
    let selected: Vec<_> = registered_detectors()
        .into_iter()
        .filter(|d| d.name == DETECTOR)
        .collect();
    assert_eq!(
        selected.len(),
        1,
        "{DETECTOR} must be registered exactly once"
    );
    run_detectors(resolved, &selected).findings
}

/// The single d50 finding whose primary checked-callsite anchor is enclosed by
/// `seed_routine_id` — the finding's subject (`d50.rs:264-272`).
fn finding_at<'f>(
    resolved: &L3Resolved,
    findings: &'f [Finding],
    seed_routine_id: &str,
) -> &'f Finding {
    let at: Vec<&Finding> = findings
        .iter()
        .filter(|f| f.primary_location.enclosing_routine_id == seed_routine_id)
        .collect();
    let subjects: Vec<String> = findings
        .iter()
        .map(|f| {
            format!(
                "{} ({})",
                label_of(resolved, &f.primary_location.enclosing_routine_id),
                f.severity
            )
        })
        .collect();
    assert_eq!(
        at.len(),
        1,
        "expected exactly 1 d50 finding at {}; all d50 subjects: {subjects:?}",
        label_of(resolved, seed_routine_id)
    );
    at[0]
}

// ---------------------------------------------------------------------------
// The precondition of the demotion.
// ---------------------------------------------------------------------------

/// B's `OnRun` really is an explicit committer really on B's checked-Run span —
/// and C really does carry BOTH kinds of committer on one span.
///
/// Both halves, from the exact fields `detect_d50` reads at `:341-344`:
/// `routines_with_explicit_commit` (built from `ExplicitCommit` spans at
/// `:223-228`) and `span.routines_in_span`. If either half stops holding, B's
/// `info` silently degenerates into "nothing happened" and the r4 golden does
/// not move — which is what this test exists to prevent.
///
/// C's half is the same guard one object over: C's `medium` survives losing
/// EITHER committer, so a severity value cannot say the set is still mixed.
#[test]
fn issue18_codeunit_b_onrun_is_an_explicit_committer_on_its_own_checked_span() {
    let resolved = resolve_fixture();
    let ctx = build_detector_context(&resolved, substrate::ALL);

    let b_onrun = routine_id(&resolved, OBJ_B, "OnRun");
    let b_post = routine_id(&resolved, OBJ_B, "PostDoc");
    let b_seed = routine_id(&resolved, OBJ_B, "RunChecked");

    // The twins and C's committer are DISTINCT routines. Stated executably, so
    // a future id-scheme change that collapsed them would fail here rather than
    // quietly make every assertion below satisfiable by the wrong object.
    let c_onrun = routine_id(&resolved, OBJ_C, "OnRun");
    let a_seed = routine_id(&resolved, OBJ_A, "RunChecked");
    assert_ne!(
        b_onrun, c_onrun,
        "B's and C's OnRun must be distinct routine ids, or matching on B is meaningless"
    );
    assert_ne!(
        b_seed, a_seed,
        "A's and B's seeds are byte-identical bodies in different objects; their ids must differ"
    );

    // 1 — membership. B's OnRun is on the span d50 iterates for B's seed.
    let span = checked_span_of(&ctx, &b_seed);
    let membership: Vec<String> = span
        .routines_in_span
        .iter()
        .map(|rid| label_of(&resolved, rid))
        .collect();
    assert!(
        span.routines_in_span.contains(&b_onrun),
        "B's OnRun ({b_onrun}) must be in the CheckedRunImplicit span seeded at B's \
         RunChecked — it is the routine cap 2 demotes. Membership: {membership:?}"
    );
    // Exactly the three routines of codeunit B, nothing else: contamination
    // from A, C or the worker would make the membership assertion above
    // satisfiable for the wrong reason.
    let mut expected = vec![b_onrun.clone(), b_post.clone(), b_seed.clone()];
    expected.sort();
    let mut got = span.routines_in_span.clone();
    got.sort();
    assert_eq!(
        got, expected,
        "B's checked-run span must contain EXACTLY B's OnRun, PostDoc and RunChecked. \
         Membership: {membership:?}"
    );

    // 2 — the witness. A SEPARATE ExplicitCommit span is seeded in B's OnRun,
    // which is what puts it in `routines_with_explicit_commit`.
    let explicit: Vec<&TransactionSpan> = ctx
        .transaction_spans
        .iter()
        .filter(|s| s.seed_kind == SeedKind::ExplicitCommit && s.commit_routine_id == b_onrun)
        .collect();
    let all_explicit: Vec<String> = ctx
        .transaction_spans
        .iter()
        .filter(|s| s.seed_kind == SeedKind::ExplicitCommit)
        .map(|s| label_of(&resolved, &s.commit_routine_id))
        .collect();
    assert_eq!(
        explicit.len(),
        1,
        "exactly one SeedKind::ExplicitCommit span must be seeded in B's OnRun ({b_onrun}) \
         — that is what `d50.rs:223-228` puts into `routines_with_explicit_commit`, and \
         without it B's `info` means 'no escalation witness at all' rather than \
         'the witness was capped'. ExplicitCommit committers in this fixture: {all_explicit:?}"
    );

    // 3 — C, the ANY-quantifier control, stated with the same two properties.
    // Without this, commenting out C's `OnRun` `Commit()` leaves `HelperC` to
    // carry the span on its own: C stays `medium`, the r4 golden is
    // BYTE-IDENTICAL, both tests here stay green, and C has silently become a
    // second copy of A with the mixed set it exists to provide now gone. That is
    // the identical rot mode described above, one object over.
    let c_seed = routine_id(&resolved, OBJ_C, "RunCheckedC");
    let c_post = routine_id(&resolved, OBJ_C, "PostDocC");
    let c_helper = routine_id(&resolved, OBJ_C, "HelperC");

    let c_span = checked_span_of(&ctx, &c_seed);
    let c_membership: Vec<String> = c_span
        .routines_in_span
        .iter()
        .map(|rid| label_of(&resolved, rid))
        .collect();
    let mut c_expected = vec![
        c_onrun.clone(),
        c_helper.clone(),
        c_post.clone(),
        c_seed.clone(),
    ];
    c_expected.sort();
    let mut c_got = c_span.routines_in_span.clone();
    c_got.sort();
    assert_eq!(
        c_got, c_expected,
        "C's checked-run span must contain EXACTLY C's OnRun, HelperC, PostDocC and \
         RunCheckedC. BOTH committers on ONE span is the whole of C's job; if the chain \
         shape ever returns, `OnRun` drops out of the span and `any` runs over an \
         all-uncapped set. Membership: {c_membership:?}"
    );

    for (committer, what) in [
        (&c_onrun, "OnRun (the CAPPED half)"),
        (&c_helper, "HelperC (the UNCAPPED half)"),
    ] {
        let seeded = ctx
            .transaction_spans
            .iter()
            .filter(|s| {
                s.seed_kind == SeedKind::ExplicitCommit && s.commit_routine_id == *committer
            })
            .count();
        assert_eq!(
            seeded, 1,
            "exactly one SeedKind::ExplicitCommit span must be seeded in C's {what} \
             ({committer}) — with only one of the two, `any` short-circuits over a uniform \
             set, C is a duplicate of A, and NOTHING moves: not C's severity, not the r4 \
             golden, not the assertions above. ExplicitCommit committers in this fixture: \
             {all_explicit:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Precondition AND outcome, joined in one place.
// ---------------------------------------------------------------------------

/// The demotion, stated as the conjunction it actually is: the escalation
/// witness IS on B's span, and B's finding is nonetheless `info` — while its
/// ONE-LINE twin A, whose committer differs only by being a `local procedure`,
/// is `medium`.
///
/// The r4 golden pins the two severities byte-for-byte but cannot say the
/// witness was ever there; the test above says the witness is there but not what
/// d50 did with it. Neither alone means "cap 2 demoted this".
#[test]
fn issue18_the_witness_is_present_and_codeunit_b_is_still_demoted_to_info() {
    let resolved = resolve_fixture();
    let ctx = build_detector_context(&resolved, substrate::ALL);

    let a_seed = routine_id(&resolved, OBJ_A, "RunChecked");
    let b_onrun = routine_id(&resolved, OBJ_B, "OnRun");
    let b_seed = routine_id(&resolved, OBJ_B, "RunChecked");

    // Precondition, restated here so this test is not parasitic on the other's
    // ordering: the witness is on B's span.
    assert!(
        checked_span_of(&ctx, &b_seed)
            .routines_in_span
            .contains(&b_onrun),
        "B's OnRun must be on B's checked-run span before its severity means anything"
    );
    assert!(
        ctx.transaction_spans
            .iter()
            .any(|s| s.seed_kind == SeedKind::ExplicitCommit && s.commit_routine_id == b_onrun),
        "B's OnRun must hold an explicit Commit() before its severity means anything"
    );

    // Precondition the final panel found missing: it must be `onrun-codeunit`
    // SPECIFICALLY that caps B. Without this, changing B's committer from
    // `trigger OnRun()` to `procedure OnRun()` — one line-count-preserving
    // edit — silently re-caps it under `public-procedure`, which is ALSO in
    // D50_UNTRUSTED_ROOT_KINDS. B would stay `info`, the r4 golden would be
    // byte-identical (the stable id hashes name/params/returnType, not kind),
    // the l2 snapshot would not move (PFeatures carries no kind, and
    // source_range is row/col), every other assertion here would pass — and
    // removing `"onrun-codeunit"` from the list, which is this whole issue's
    // point, would be a no-op AGAIN. The EXACT-set compare is deliberate:
    // `contains` would pass on a set that had silently gained a second kind.
    let b_kinds = ctx
        .root_classifications_by_routine
        .get(&b_onrun)
        .map(|rc| rc.kinds.clone())
        .unwrap_or_default();
    assert_eq!(
        b_kinds.as_slice(),
        ["onrun-codeunit"],
        "B's committer must classify as EXACTLY `onrun-codeunit` — the kind whose \
         presence in D50_UNTRUSTED_ROOT_KINDS this fixture exists to make falsifiable. \
         Got: {b_kinds:?}"
    );

    let findings = d50_findings(&resolved);
    assert_eq!(
        finding_at(&resolved, &findings, &b_seed).severity,
        "info",
        "B's committer is a `trigger OnRun()` → `onrun-codeunit` → in \
         D50_UNTRUSTED_ROOT_KINDS → cap 2 rejects it, so the escalation witness proven \
         present above must NOT escalate"
    );
    assert_eq!(
        finding_at(&resolved, &findings, &a_seed).severity,
        "medium",
        "A is B's ONE-LINE twin — identical chain, identical bodies, committer declared \
         `local procedure` instead of `trigger OnRun()`, so it carries no root \
         classification and cap 2 passes. If A were `info` too, B's `info` would be \
         evidence about the span machinery, not about the cap"
    );
}

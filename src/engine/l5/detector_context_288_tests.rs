//! #288 acceptance tests (the `## Design` of `.agent/issue-288/ledger.md`,
//! revision 5): a site that provably never runs adds no coverage gap and no
//! cone edge, and a site counts as dead only where its row's `unreachable`
//! marks are trusted (`ModelRoutine::dead_marks_trusted`).
//!
//! Every test builds its model with production assembly from inline AL
//! ([`SRC`], or a small source of its own) and asserts its hand-stated
//! preconditions first. The test names carry the matrix row (P1, C3, G11u, ...).
//! A "keeps" test is a guard against the filter: on the base, which filters
//! nothing, it holds by design.

use super::*;
use crate::engine::gate::run::AnalysisTarget;
use crate::engine::l4::capability_cone::{
    DeadSites, R3a3SourceBase, build_cross_app_base, build_r3a3_source_only_base,
    build_r3a3_source_only_base_over, project_r3a3,
};
use crate::engine::l4::combined_graph::build_combined_graph;
use crate::engine::l5::capability_query::{EffectPresence, may_commit_derived};
use crate::engine::l5::detectors::d35::detect_d35;
use crate::engine::l5::detectors::registered_detectors;
use crate::engine::l5::finding::Finding;
use crate::engine::l5::registry::substrate;
use crate::engine::l5::test_support::{call_site, routine, write_fixture_dep_ambiguous_after};
use crate::engine::session::{View, analyze_plan};
use crate::program::model::program_calls::assemble_and_resolve_inline_program_default;
use crate::program::model::workspace::{has_preproc_directive, source_clean};

const APP: &str = "11111111-0000-0000-0000-000000028800";

/// The #288 bench fixture (`/work/bench/288/rev5/fixture`) in one file, plus
/// the rows the matrix adds (M2, M3, G4's member and index forms, G6's Text
/// local, G7, G1's committing callee, G11u/G12u). Each interface call has its
/// own argument, so a test finds its line by that text.
const SRC: &str = r#"interface "I288 Strategy"
{
    procedure NextKm(Km: Integer): Integer;
}

enum 50280 "E288 Strategy" implements "I288 Strategy"
{
    Extensible = true;

    value(0; Plain)
    {
        Implementation = "I288 Strategy" = "C288 Plain";
    }
}

codeunit 50280 "C288 Plain" implements "I288 Strategy"
{
    procedure NextKm(Km: Integer): Integer
    begin
        exit(Km + 1);
    end;
}

table 50283 "T288 Log"
{
    fields
    {
        field(1; "No."; Integer) { }
        field(2; Msg; Text[100]) { }
    }
    keys { key(PK; "No.") { } }

    trigger OnInsert()
    begin
        Commit();
    end;
}

codeunit 50281 "C288 Subs"
{
    [EventSubscriber(ObjectType::Codeunit, 50, 'OnAfterPost', '', false, false)]
    local procedure OnAfterPostDead()
    var
        Strategy: Interface "I288 Strategy";
    begin
        Strategy := Enum::"E288 Strategy"::Plain;
        exit;
        Strategy.NextKm(1);
    end;

    [EventSubscriber(ObjectType::Codeunit, 50, 'OnAfterValidate', '', false, false)]
    local procedure OnAfterValidateLive()
    var
        Strategy: Interface "I288 Strategy";
    begin
        Strategy := Enum::"E288 Strategy"::Plain;
        Strategy.NextKm(2);
    end;

    [EventSubscriber(ObjectType::Codeunit, 50, 'OnBeforePrint', '', false, false)]
    local procedure OnBeforePrintAssertError()
    var
        Strategy: Interface "I288 Strategy";
    begin
        Strategy := Enum::"E288 Strategy"::Plain;
        asserterror Error('x');
        Strategy.NextKm(3);
    end;

    [EventSubscriber(ObjectType::Codeunit, 50, 'OnBeforeModify', '', false, false)]
    local procedure OnMixedLiveDead()
    var
        Strategy: Interface "I288 Strategy";
    begin
        Strategy := Enum::"E288 Strategy"::Plain;
        Strategy.NextKm(81);
        exit;
        Strategy.NextKm(82);
    end;

    [EventSubscriber(ObjectType::Codeunit, 50, 'OnAfterModify', '', false, false)]
    local procedure OnDeadThenLive(Flag: Boolean)
    var
        Strategy: Interface "I288 Strategy";
    begin
        Strategy := Enum::"E288 Strategy"::Plain;
        if Flag then begin
            exit;
            Strategy.NextKm(83);
        end;
        Strategy.NextKm(84);
    end;

    [EventSubscriber(ObjectType::Codeunit, 50, 'OnBeforeRename', '', false, false)]
    local procedure OnAllDead()
    var
        Strategy: Interface "I288 Strategy";
    begin
        Strategy := Enum::"E288 Strategy"::Plain;
        exit;
        Strategy.NextKm(85);
        Strategy.NextKm(86);
    end;

    [EventSubscriber(ObjectType::Codeunit, 51, 'OnAfterModify', '', false, false)]
    local procedure OnDeadObjectRun(Id: Integer)
    begin
        exit;
        Codeunit.Run(Id);
    end;

    [EventSubscriber(ObjectType::Codeunit, 51, 'OnAfterDelete', '', false, false)]
    local procedure OnDeadCommitCallee()
    begin
        exit;
        DoCommit();
    end;

    [EventSubscriber(ObjectType::Codeunit, 52, 'OnBeforeModify', '', false, false)]
    local procedure OnDeadTriggerInsert()
    var
        Log: Record "T288 Log";
    begin
        exit;
        Log.Insert(true);
    end;

    [EventSubscriber(ObjectType::Codeunit, 52, 'OnBeforeInsert', '', false, false)]
    local procedure OnLiveTriggerInsert()
    var
        Log: Record "T288 Log";
    begin
        Log.Insert(true);
    end;

    [EventSubscriber(ObjectType::Codeunit, 52, 'OnBeforeValidate', '', false, false)]
    local procedure OnRecursiveDead()
    begin
        RecA();
    end;

    [EventSubscriber(ObjectType::Codeunit, 52, 'OnAfterRename', '', false, false)]
    local procedure OnLiveWitness()
    begin
        Helper3();
        exit;
        DoCommit3();
    end;

    [EventSubscriber(ObjectType::Codeunit, 52, 'OnBeforePrint', '', false, false)]
    local procedure OnTextError()
    var
        Strategy: Interface "I288 Strategy";
    begin
        Strategy := Enum::"E288 Strategy"::Plain;
        Error('stop');
        Strategy.NextKm(5);
    end;

    [EventSubscriber(ObjectType::Codeunit, 52, 'OnBeforeDelete', '', false, false)]
    local procedure OnLabelError()
    var
        Strategy: Interface "I288 Strategy";
        StopLbl: Label 'stop %1';
    begin
        Strategy := Enum::"E288 Strategy"::Plain;
        Error(StopLbl, 1);
        Strategy.NextKm(6);
    end;

    [EventSubscriber(ObjectType::Codeunit, 52, 'OnAfterDelete', '', false, false)]
    local procedure OnTextLocalError()
    var
        Strategy: Interface "I288 Strategy";
        Msg: Text;
    begin
        Strategy := Enum::"E288 Strategy"::Plain;
        Msg := 'stop';
        Error(Msg);
        Strategy.NextKm(7);
    end;

    [EventSubscriber(ObjectType::Codeunit, 51, 'OnBeforeDelete', '', false, false)]
    local procedure OnCollectibleError()
    var
        Strategy: Interface "I288 Strategy";
        ErrInfo: ErrorInfo;
    begin
        Strategy := Enum::"E288 Strategy"::Plain;
        ErrInfo.Message('x');
        ErrInfo.Collectible(true);
        Error(ErrInfo);
        Strategy.NextKm(117);
    end;

    [EventSubscriber(ObjectType::Codeunit, 52, 'OnAfterPost', '', false, false)]
    local procedure OnParenErrorInfo()
    var
        Strategy: Interface "I288 Strategy";
        ErrInfo: ErrorInfo;
    begin
        Strategy := Enum::"E288 Strategy"::Plain;
        ErrInfo.Collectible(true);
        Error((ErrInfo));
        Strategy.NextKm(41);
    end;

    [EventSubscriber(ObjectType::Codeunit, 52, 'OnAfterValidate', '', false, false)]
    local procedure OnCallErrorInfo()
    var
        Strategy: Interface "I288 Strategy";
    begin
        Strategy := Enum::"E288 Strategy"::Plain;
        Error(MakeErrInfo());
        Strategy.NextKm(42);
    end;

    [EventSubscriber(ObjectType::Codeunit, 52, 'OnAfterInsert', '', false, false)]
    local procedure OnFieldError()
    var
        Strategy: Interface "I288 Strategy";
        Log: Record "T288 Log";
    begin
        Strategy := Enum::"E288 Strategy"::Plain;
        Error(Log.Msg);
        Strategy.NextKm(43);
    end;

    [EventSubscriber(ObjectType::Codeunit, 52, 'OnAfterModify', '', false, false)]
    local procedure OnArrayError()
    var
        Strategy: Interface "I288 Strategy";
        Msgs: array[2] of Text;
    begin
        Strategy := Enum::"E288 Strategy"::Plain;
        Error(Msgs[1]);
        Strategy.NextKm(44);
    end;

    local procedure RecA()
    begin
        exit;
        RecB();
    end;

    local procedure RecB()
    begin
        RecA();
        Commit();
    end;

    local procedure Helper3()
    begin
        Inner3();
    end;

    local procedure Inner3()
    begin
        Commit();
    end;

    local procedure DoCommit3()
    begin
        Commit();
    end;

    local procedure DoCommit()
    begin
        Commit();
    end;

    local procedure MakeErrInfo(): ErrorInfo
    var
        E: ErrorInfo;
    begin
        exit(E);
    end;
}

codeunit 50282 "C288 Preproc"
{
    [EventSubscriber(ObjectType::Codeunit, 53, 'OnAfterPost', '', false, false)]
    local procedure OnIfExitThenCall()
    var
        Strategy: Interface "I288 Strategy";
    begin
        Strategy := Enum::"E288 Strategy"::Plain;
#if EARLY288
        exit;
#endif
        Strategy.NextKm(11);
        DoCommitP();
    end;

    [EventSubscriber(ObjectType::Codeunit, 53, 'OnAfterValidate', '', false, false)]
    local procedure OnIfExitElseCall()
    var
        Strategy: Interface "I288 Strategy";
    begin
        Strategy := Enum::"E288 Strategy"::Plain;
#if EARLY288
        exit;
#else
        Strategy.NextKm(12);
        DoCommitP();
#endif
    end;

    [EventSubscriber(ObjectType::Codeunit, 53, 'OnBeforePrint', '', false, false)]
    local procedure OnIfArmPreprocExit(Flag: Boolean)
    var
        Strategy: Interface "I288 Strategy";
    begin
        Strategy := Enum::"E288 Strategy"::Plain;
        if Flag then begin
#if EARLY288
            exit;
#endif
        end else
            exit;
        Strategy.NextKm(13);
        DoCommitP();
    end;

    [EventSubscriber(ObjectType::Codeunit, 53, 'OnAfterInsert', '', false, false)]
    local procedure OnElifExit()
    var
        Strategy: Interface "I288 Strategy";
    begin
        Strategy := Enum::"E288 Strategy"::Plain;
#if A288
        Strategy.NextKm(14);
#elif B288
        exit;
#endif
        Strategy.NextKm(15);
        DoCommitP();
    end;

    [EventSubscriber(ObjectType::Codeunit, 53, 'OnAfterModify', '', false, false)]
    local procedure OnNestedExit()
    var
        Strategy: Interface "I288 Strategy";
    begin
        Strategy := Enum::"E288 Strategy"::Plain;
#if A288
#if B288
        exit;
#endif
#endif
        Strategy.NextKm(16);
        DoCommitP();
    end;

    [EventSubscriber(ObjectType::Codeunit, 53, 'OnAfterDelete', '', false, false)]
    local procedure OnSpacedDirective()
    var
        Strategy: Interface "I288 Strategy";
    begin
        Strategy := Enum::"E288 Strategy"::Plain;
# if SPACED288
        exit;
# endif
        Strategy.NextKm(17);
    end;

    [EventSubscriber(ObjectType::Codeunit, 53, 'OnBeforeDelete', '', false, false)]
    local procedure OnDirectiveElsewhere()
    var
        Strategy: Interface "I288 Strategy";
        i: Integer;
    begin
        Strategy := Enum::"E288 Strategy"::Plain;
#if G7288
        i := 1;
#endif
        exit;
        Strategy.NextKm(18);
    end;

    local procedure DoCommitP()
    begin
        Commit();
    end;
}

codeunit 50284 "C288 Global Text First"
{
#if TEXT288
    var
        StopValue: Text;
#else
    var
        StopValue: ErrorInfo;
#endif

    [EventSubscriber(ObjectType::Codeunit, 54, 'OnAfterPost', '', false, false)]
    local procedure OnGlobalTextFirst()
    var
        Strategy: Interface "I288 Strategy";
    begin
        Strategy := Enum::"E288 Strategy"::Plain;
        Error(StopValue);
        Strategy.NextKm(21);
    end;
}

codeunit 50285 "C288 Global ErrInfo First"
{
#if ERRINFO288
    var
        StopValue: ErrorInfo;
#else
    var
        StopValue: Text;
#endif

    [EventSubscriber(ObjectType::Codeunit, 54, 'OnAfterValidate', '', false, false)]
    local procedure OnGlobalErrInfoFirst()
    var
        Strategy: Interface "I288 Strategy";
    begin
        Strategy := Enum::"E288 Strategy"::Plain;
        Error(StopValue);
        Strategy.NextKm(22);
    end;
}

codeunit 50286 "C288 Global Text Plain"
{
    var
        StopText: Text;

    [EventSubscriber(ObjectType::Codeunit, 54, 'OnBeforePrint', '', false, false)]
    local procedure OnGlobalTextPlain()
    var
        Strategy: Interface "I288 Strategy";
    begin
        Strategy := Enum::"E288 Strategy"::Plain;
        Error(StopText);
        Strategy.NextKm(23);
    end;
}

codeunit 50287 "C288 Own Error"
{
    [EventSubscriber(ObjectType::Codeunit, 54, 'OnBeforeDelete', '', false, false)]
    local procedure OnOwnErrorText()
    var
        Strategy: Interface "I288 Strategy";
    begin
        Strategy := Enum::"E288 Strategy"::Plain;
        Error('x');
        Strategy.NextKm(24);
    end;

    procedure Error(Msg: Text)
    begin
    end;
}

codeunit 50288 "C288 Own Error Format"
{
    [EventSubscriber(ObjectType::Codeunit, 54, 'OnAfterInsert', '', false, false)]
    local procedure OnOwnErrorFormat()
    var
        Strategy: Interface "I288 Strategy";
    begin
        Strategy := Enum::"E288 Strategy"::Plain;
        Error('x %1', 1);
        Strategy.NextKm(25);
    end;

    procedure Error(Msg: Text; V: Integer)
    begin
    end;
}

codeunit 50290 "C288 Named Return"
{
    var
        StopValue: Text;

    [EventSubscriber(ObjectType::Codeunit, 55, 'OnAfterPost', '', false, false)]
    local procedure OnNamedReturnGap()
    begin
        WorkErr();
    end;

    [EventSubscriber(ObjectType::Codeunit, 55, 'OnAfterValidate', '', false, false)]
    local procedure OnNamedReturnEdge()
    begin
        WorkCommit();
    end;

    [EventSubscriber(ObjectType::Codeunit, 55, 'OnBeforePrint', '', false, false)]
    local procedure OnNamedReturnText()
    begin
        WorkText();
    end;

    local procedure WorkErr() StopValue: ErrorInfo
    var
        Strategy: Interface "I288 Strategy";
    begin
        Strategy := Enum::"E288 Strategy"::Plain;
        StopValue.Collectible(true);
        Error(StopValue);
        Strategy.NextKm(31);
    end;

    local procedure WorkCommit() StopValue: ErrorInfo
    begin
        StopValue.Collectible(true);
        Error(StopValue);
        DoCommit5();
    end;

    local procedure WorkText() StopValue: Text
    var
        Strategy: Interface "I288 Strategy";
    begin
        StopValue := 'stop';
        Strategy := Enum::"E288 Strategy"::Plain;
        Error(StopValue);
        Strategy.NextKm(32);
    end;

    local procedure DoCommit5()
    begin
        Commit();
    end;
}

codeunit 50291 "C288 Named Return U"
{
    var
        "STOPVÄLUE": Text;

    [EventSubscriber(ObjectType::Codeunit, 56, 'OnAfterPost', '', false, false)]
    local procedure OnNamedReturnGapU()
    begin
        WorkErrU();
    end;

    [EventSubscriber(ObjectType::Codeunit, 56, 'OnAfterValidate', '', false, false)]
    local procedure OnNamedReturnEdgeU()
    begin
        WorkCommitU();
    end;

    local procedure WorkErrU() "StopVälue": ErrorInfo
    var
        Strategy: Interface "I288 Strategy";
    begin
        Strategy := Enum::"E288 Strategy"::Plain;
        "StopVälue".Collectible(true);
        Error("STOPVÄLUE");
        Strategy.NextKm(33);
    end;

    local procedure WorkCommitU() "StopVälue": ErrorInfo
    begin
        "StopVälue".Collectible(true);
        Error("STOPVÄLUE");
        DoCommitU();
    end;

    local procedure DoCommitU()
    begin
        Commit();
    end;
}
"#;

fn build(src: &str) -> Model {
    assemble_and_resolve_inline_program_default(
        &[("src/T288.al".to_string(), src.to_string())],
        APP,
    )
}

/// The one row named `name`.
fn row<'m>(m: &'m Model, name: &str) -> &'m ModelRoutine {
    let rows: Vec<&ModelRoutine> = m
        .workspace
        .routines
        .iter()
        .filter(|r| r.name == name)
        .collect();
    assert_eq!(rows.len(), 1, "precondition: one routine {name}");
    rows[0]
}

/// The 1-based line of the one [`SRC`] line holding `text`.
fn line_of(text: &str) -> usize {
    let lines: Vec<usize> = SRC
        .lines()
        .enumerate()
        .filter(|(_, l)| l.contains(text))
        .map(|(i, _)| i + 1)
        .collect();
    assert_eq!(lines.len(), 1, "precondition: one line holds {text}");
    lines[0]
}

/// Precondition: `name`'s row holds a call site whose callee text contains
/// `callee` and that is marked `unreachable` (the mark the gate trusts or not).
fn assert_dead_site(m: &Model, name: &str, callee: &str) {
    let r = row(m, name);
    assert!(
        r.call_sites.iter().any(|cs| cs.callee_text.contains(callee)
            && cs.control_context.as_deref() == Some("unreachable")),
        "precondition: {name} has an unreachable call to {callee}: {:?}",
        r.call_sites
            .iter()
            .map(|cs| (&cs.callee_text, &cs.control_context))
            .collect::<Vec<_>>()
    );
}

/// What the detector context says about one routine: its coverage, its
/// may-commit answer and its d35 finding.
struct Seen {
    direct_status: String,
    inherited_status: String,
    reasons: Vec<String>,
    commits: EffectPresence,
    d35: Option<Finding>,
}

/// [`Seen`] for each of `names`, through the production detector context
/// (`build_detector_context`, the single-app shim) and d35.
fn seen(m: &Model, names: &[&str]) -> Vec<Seen> {
    let owned = build_detector_context(m, substrate::ALL);
    let ctx = owned.ctx();
    let findings = detect_d35(m, &ctx).unwrap().findings;
    names
        .iter()
        .map(|name| {
            let id = &row(m, name).id;
            let s = ctx.summaries.get(id).expect("a summary");
            let c = s.coverage.as_ref().expect("a coverage record");
            let prefix = format!("d35/{id}/");
            let mut d35: Vec<Finding> = findings
                .iter()
                .filter(|f| f.id.starts_with(&prefix))
                .cloned()
                .collect();
            assert!(d35.len() <= 1, "one d35 finding per subscriber");
            Seen {
                direct_status: c.direct_status.clone(),
                inherited_status: c.inherited_status.clone(),
                reasons: c.reasons.clone(),
                commits: may_commit_derived(&ctx.cone_derived, s),
                d35: d35.pop(),
            }
        })
        .collect()
}

fn seen1(m: &Model, name: &str) -> Seen {
    seen(m, &[name]).pop().unwrap()
}

fn assert_complete_no_d35(s: &Seen, name: &str) {
    assert_eq!(
        (
            s.direct_status.as_str(),
            s.inherited_status.as_str(),
            s.reasons.as_slice()
        ),
        ("complete", "complete", &[] as &[String]),
        "{name}: complete coverage, no reason"
    );
    assert!(s.d35.is_none(), "{name}: no d35 finding: {:?}", s.d35);
}

/// The subscriber keeps its own interface gap: direct `partial` with
/// `interface-open-world`.
fn assert_keeps_gap(s: &Seen, name: &str) {
    assert_eq!(
        (s.direct_status.as_str(), s.reasons.as_slice()),
        (
            "partial",
            &["interface-open-world".to_string()] as &[String]
        ),
        "{name}: the interface gap is kept"
    );
}

/// d35's unknown arm names `reasons` (the `(...)` text of its root cause).
fn assert_d35_reasons(s: &Seen, name: &str, reasons: &str) {
    let f = s
        .d35
        .as_ref()
        .unwrap_or_else(|| panic!("{name}: a d35 finding"));
    assert!(
        f.root_cause.contains(&format!("({reasons});")),
        "{name}: d35 names ({reasons}); got: {}",
        f.root_cause
    );
}

fn assert_d35_via_callee(s: &Seen, name: &str) {
    assert_eq!(
        s.d35.as_ref().map(|f| &*f.title),
        Some("Commit reachable from event subscriber (via callee)"),
        "{name}: d35 finds the committing callee"
    );
}

// ---------------------------------------------------------------------------
// P1-P4: every coverage producer drops a trusted dead gap
// ---------------------------------------------------------------------------

/// P1: the detector context. The reproducer's only gap is a dead interface
/// call after `exit;` in a trusted row: its coverage is complete and d35 says
/// nothing (base: "interface-open-world; partly analysed: OnAfterPostDead").
#[test]
fn p1_detector_context_drops_a_trusted_dead_gap() {
    let m = build(SRC);
    assert_dead_site(&m, "OnAfterPostDead", "NextKm");
    assert!(
        row(&m, "OnAfterPostDead").dead_marks_trusted,
        "precondition: the reproducer's row is trusted"
    );
    assert_complete_no_d35(&seen1(&m, "OnAfterPostDead"), "OnAfterPostDead");
}

/// The r3a3 projection's coverage record of `name`.
fn r3a3_coverage<'p>(
    m: &Model,
    p: &'p crate::engine::l4::capability_cone::R3a3Projection,
    name: &str,
) -> &'p crate::engine::l4::capability_cone::PCoverageRecord {
    let stable = &row(m, name).stable_routine_id;
    &p.summaries
        .iter()
        .find(|s| &s.routine_id == stable)
        .unwrap_or_else(|| panic!("an r3a3 summary for {name}"))
        .coverage
}

/// P2: the r3a3 projection's coverage fold.
#[test]
fn p2_project_r3a3_drops_a_trusted_dead_gap() {
    let m = build(SRC);
    assert_dead_site(&m, "OnAfterPostDead", "NextKm");
    let p = project_r3a3(&m);
    let c = r3a3_coverage(&m, &p, "OnAfterPostDead");
    assert_eq!(
        (
            c.direct_status.as_str(),
            c.inherited_status.as_str(),
            c.reasons.as_slice()
        ),
        ("complete", "complete", &[] as &[String]),
        "OnAfterPostDead: the r3a3 projection drops the dead gap"
    );
}

/// P3: the snapshot base's coverage fold.
#[test]
fn p3_snapshot_base_drops_a_trusted_dead_gap() {
    let m = build(SRC);
    assert_dead_site(&m, "OnAfterPostDead", "NextKm");
    let base = build_r3a3_source_only_base(&m);
    let c = &base.coverage[&row(&m, "OnAfterPostDead").id];
    assert_eq!(
        (
            c.direct_status.as_str(),
            c.inherited_status.as_str(),
            c.reasons.as_slice()
        ),
        ("complete", "complete", &[] as &[String]),
        "OnAfterPostDead: the snapshot base drops the dead gap"
    );
}

/// P4: the cross-app base's fold over the dependency half. The dependency's
/// `Inner` makes its one ambiguous call after `exit;`; `Inner`'s direct
/// coverage is complete in the base (`direct_coverage`) and in the view's
/// detector context. The break in `build_cross_app_base_with` fails only the
/// base assertion (the view folds in `det_cone_inputs_over`).
#[test]
fn p4_cross_app_base_drops_a_trusted_dead_dependency_gap() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().join("ws");
    write_fixture_dep_ambiguous_after(&ws, "exit;");
    let inputs = crate::program::inputs::WorkspaceInputs::read(&ws, &Default::default());
    let base = build_cross_app_base(&inputs, "r0").expect("cross-app base");
    let inner: Vec<&ModelRoutine> = base
        .ws_routines()
        .iter()
        .filter(|r| r.name == "Inner")
        .collect();
    assert_eq!(inner.len(), 1, "precondition: one Inner row");
    let inner_id = inner[0].id.clone();
    assert!(
        base.dep_routine_ids.contains(&inner_id),
        "precondition: Inner is dependency code"
    );
    assert!(
        inner[0].dead_marks_trusted
            && inner[0]
                .call_sites
                .iter()
                .any(|cs| cs.control_context.as_deref() == Some("unreachable")),
        "precondition: Inner is a trusted row with a dead call site"
    );
    assert!(
        base.rel()
            .dependency_uncertainty()
            .iter()
            .any(|u| u.from == inner_id && u.uncertainty.kind == "ambiguous-overload"),
        "precondition: the dependency half holds Inner's ambiguous-overload edge"
    );
    let want = ("complete".to_string(), Vec::<String>::new());
    assert_eq!(
        base.direct_coverage.get(&inner_id),
        Some(&want),
        "the cross-app base drops Inner's dead gap"
    );
    let d35 = registered_detectors()
        .into_iter()
        .find(|d| d.name == crate::engine::l5::detectors::d35::DETECTOR)
        .expect("d35 is registered");
    let mut view = View::new(AnalysisTarget::CrossApp(Box::new(base)));
    view.prepare(&analyze_plan(vec![d35], true));
    let c = view.det().summaries[&inner_id].coverage.clone().unwrap();
    assert_eq!(
        (c.direct_status, c.reasons),
        want,
        "the view's detector context drops Inner's dead gap"
    );
}

// ---------------------------------------------------------------------------
// C1-C6: the cone drops a trusted dead edge
// ---------------------------------------------------------------------------

/// C1: a dead `Codeunit.Run(Id)` adds no `object-run-unresolved` gap.
#[test]
fn c1_dead_object_run_is_no_gap() {
    let m = build(SRC);
    assert_dead_site(&m, "OnDeadObjectRun", "Codeunit.Run");
    assert_complete_no_d35(&seen1(&m, "OnDeadObjectRun"), "OnDeadObjectRun");
}

/// C2: a dead call to a committing callee adds no Commit.
#[test]
fn c2_dead_committing_callee_is_no_commit() {
    let m = build(SRC);
    assert_dead_site(&m, "OnDeadCommitCallee", "DoCommit");
    let s = seen1(&m, "OnDeadCommitCallee");
    assert_eq!(
        s.commits,
        EffectPresence::No,
        "OnDeadCommitCallee: no Commit"
    );
    assert!(s.d35.is_none(), "no d35 finding: {:?}", s.d35);
}

/// C3: a dead `Insert(true)` whose `OnInsert` commits adds no Commit (its
/// implicit-trigger edge carries an operation id, no call site); the live
/// control still commits.
#[test]
fn c3_dead_trigger_insert_is_no_commit() {
    let m = build(SRC);
    let dead = row(&m, "OnDeadTriggerInsert");
    assert!(
        dead.operation_sites
            .iter()
            .any(|op| op.control_context.as_deref() == Some("unreachable")),
        "precondition: the Insert is an unreachable operation site"
    );
    let s = seen(&m, &["OnDeadTriggerInsert", "OnLiveTriggerInsert"]);
    assert_eq!(
        (s[0].commits, s[1].commits),
        (EffectPresence::No, EffectPresence::Yes),
        "(dead, live) Insert(true): the dead one adds no Commit"
    );
}

/// The snapshot base's raw inherited facts of `name`.
fn raw_of<'b>(m: &Model, base: &'b R3a3SourceBase, name: &str) -> &'b [CapabilityFact] {
    base.raw
        .get(&row(m, name).id)
        .map(Vec::as_slice)
        .unwrap_or_default()
}

/// C4: `RecA -> RecB` is dead and `RecB -> RecA` live, so `{RecA, RecB}` is an
/// SCC only over the unfiltered graph. Filtered, the subscriber reaches only
/// `RecA`, which has no live fact: no raw inherited fact and no Commit.
#[test]
fn c4_dead_edge_breaks_a_recursive_scc() {
    let m = build(SRC);
    assert_dead_site(&m, "RecA", "RecB");
    let base = build_r3a3_source_only_base(&m);
    assert!(
        base.direct_full[&row(&m, "RecB").id]
            .iter()
            .any(|f| f.op == "commit"),
        "precondition: RecB commits directly"
    );
    let raw = raw_of(&m, &base, "OnRecursiveDead");
    assert_eq!(
        raw.iter().map(|f| (&f.subject, f.op)).collect::<Vec<_>>(),
        Vec::<(&String, &str)>::new(),
        "OnRecursiveDead inherits nothing (RecB is not in its cone)"
    );
    assert_eq!(
        seen1(&m, "OnRecursiveDead").commits,
        EffectPresence::No,
        "OnRecursiveDead: no Commit"
    );
}

/// C5: a live witness survives the filter: `Helper3(); exit; DoCommit3();`
/// still commits, by one inherited Commit whose first hop is the `Helper3()`
/// call site (`Helper3 -> Inner3`). On the base the one witness kept is the
/// dead `DoCommit3()` site.
#[test]
fn c5_live_witness_survives() {
    let m = build(SRC);
    assert_dead_site(&m, "OnLiveWitness", "DoCommit3");
    let sub = row(&m, "OnLiveWitness");
    let helper_cs = sub
        .call_sites
        .iter()
        .find(|cs| cs.callee_text == "Helper3")
        .expect("precondition: the Helper3 call site")
        .id
        .clone();
    let base = build_r3a3_source_only_base(&m);
    let commits: Vec<(&str, Option<&str>)> = raw_of(&m, &base, "OnLiveWitness")
        .iter()
        .filter(|f| f.op == "commit")
        .map(|f| (f.subject.as_str(), f.witness_callsite_id.as_deref()))
        .collect();
    assert_eq!(
        commits,
        vec![(sub.id.as_str(), Some(helper_cs.as_str()))],
        "OnLiveWitness: one inherited Commit, first hop the Helper3 call"
    );
    let s = seen1(&m, "OnLiveWitness");
    assert_eq!(s.commits, EffectPresence::Yes, "OnLiveWitness commits");
    assert_d35_via_callee(&s, "OnLiveWitness");
}

/// C6: `project_r3a3` builds its cone over the filtered graph: the dead
/// object run is no `object-run-unresolved` source there either.
#[test]
fn c6_project_r3a3_uses_the_filtered_graph() {
    let m = build(SRC);
    assert_dead_site(&m, "OnDeadObjectRun", "Codeunit.Run");
    let p = project_r3a3(&m);
    let c = r3a3_coverage(&m, &p, "OnDeadObjectRun");
    assert_eq!(
        (c.direct_status.as_str(), c.inherited_status.as_str()),
        ("complete", "complete"),
        "OnDeadObjectRun: complete in the r3a3 projection: {c:?}"
    );
}

// ---------------------------------------------------------------------------
// M1-M7: deadness is per (routine id, site)
// ---------------------------------------------------------------------------

fn assert_trusted(m: &Model, name: &str) {
    assert!(
        row(m, name).dead_marks_trusted,
        "precondition: {name}'s row is trusted"
    );
}

/// M1: live then dead, same kind: the live site keeps the gap.
#[test]
fn m1_live_then_dead_keeps_the_gap() {
    let m = build(SRC);
    assert_dead_site(&m, "OnMixedLiveDead", "NextKm");
    assert_trusted(&m, "OnMixedLiveDead");
    assert_keeps_gap(&seen1(&m, "OnMixedLiveDead"), "OnMixedLiveDead");
}

/// M2: dead then live, same kind: the live site keeps the gap.
#[test]
fn m2_dead_then_live_keeps_the_gap() {
    let m = build(SRC);
    assert_dead_site(&m, "OnDeadThenLive", "NextKm");
    assert_trusted(&m, "OnDeadThenLive");
    assert_keeps_gap(&seen1(&m, "OnDeadThenLive"), "OnDeadThenLive");
}

/// M3: every site of the kind is dead: complete.
#[test]
fn m3_all_dead_sites_drop_the_gap() {
    let m = build(SRC);
    let dead = row(&m, "OnAllDead")
        .call_sites
        .iter()
        .filter(|cs| {
            cs.callee_text.contains("NextKm")
                && cs.control_context.as_deref() == Some("unreachable")
        })
        .count();
    assert_eq!(dead, 2, "precondition: two dead NextKm sites");
    assert_complete_no_d35(&seen1(&m, "OnAllDead"), "OnAllDead");
}

/// The snapshot base over `m`'s combined graph with `OnAfterPostDead`'s one
/// interface uncertainty edge re-pointed at `site`.
fn base_with_reproducer_site(m: &Model, site: Option<&str>) -> R3a3SourceBase {
    let id = row(m, "OnAfterPostDead").id.clone();
    let mut graph = build_combined_graph(&m.workspace, &m.calls, &m.events.graph);
    let edges: Vec<&mut crate::engine::l4::combined_graph::UncertaintyEdge> = graph
        .primary
        .uncertainty_edges
        .iter_mut()
        .filter(|u| u.from == id && u.uncertainty.kind == "interface-open-world")
        .collect();
    assert_eq!(edges.len(), 1, "precondition: one interface edge");
    for e in edges {
        e.uncertainty.callsite_id = site.map(str::to_string);
    }
    build_r3a3_source_only_base_over(m, graph)
}

/// M4: an edge with no `callsite_id` keeps its gap, though the row is trusted
/// and its only site of the kind is dead.
#[test]
fn m4_edge_without_a_site_keeps_its_gap() {
    let m = build(SRC);
    assert_dead_site(&m, "OnAfterPostDead", "NextKm");
    assert_trusted(&m, "OnAfterPostDead");
    let base = base_with_reproducer_site(&m, None);
    let c = &base.coverage[&row(&m, "OnAfterPostDead").id];
    assert_eq!(
        c.direct_status, "partial",
        "a site-less edge keeps its gap: {c:?}"
    );
}

/// M5: an edge whose site is in no row keeps its gap.
#[test]
fn m5_edge_with_an_unknown_site_keeps_its_gap() {
    let m = build(SRC);
    assert_trusted(&m, "OnAfterPostDead");
    let base = base_with_reproducer_site(&m, Some("cs-in-no-row"));
    let c = &base.coverage[&row(&m, "OnAfterPostDead").id];
    assert_eq!(
        c.direct_status, "partial",
        "an edge at a site no row holds keeps its gap: {c:?}"
    );
}

/// Two rows of one id `Work` (an `#if A288` / `#else` pair, the step 11b
/// collision; the directives sit outside each procedure, so both rows are
/// trusted). Arm A makes its interface call after `exit;`; arm B is `arm_b`.
fn collided_src(arm_b: &str) -> String {
    let head = SRC.split("codeunit 50281").next().unwrap();
    format!(
        "{head}codeunit 50295 \"C288 Collided\"
{{
#if A288
    procedure Work()
    var
        Strategy: Interface \"I288 Strategy\";
    begin
        exit;
        Strategy.NextKm(61);
    end;
#else
    procedure Work()
    var
        Strategy: Interface \"I288 Strategy\";
    begin
{arm_b}
    end;
#endif
}}
"
    )
}

/// The collided `Work`'s two rows (preconditions: one id, both trusted, the
/// first row's `cs` site dead) and its detector-context coverage.
fn collided_work(arm_b: &str) -> (Vec<ModelRoutine>, CoverageRecordOwned) {
    let m = build(&collided_src(arm_b));
    let rows: Vec<ModelRoutine> = m
        .workspace
        .routines
        .iter()
        .filter(|r| r.name == "Work")
        .cloned()
        .collect();
    assert_eq!(rows.len(), 2, "precondition: Work is two rows");
    assert_eq!(rows[0].id, rows[1].id, "precondition: one id");
    assert!(
        rows.iter().all(|r| r.dead_marks_trusted),
        "precondition: both rows are trusted"
    );
    assert!(
        rows[0]
            .call_sites
            .iter()
            .any(|cs| cs.control_context.as_deref() == Some("unreachable")),
        "precondition: arm A's call is dead"
    );
    let owned = build_detector_context(&m, substrate::ALL);
    let ctx = owned.ctx();
    let c = ctx.summaries[&rows[0].id].coverage.clone().unwrap();
    (rows, (c.direct_status, c.reasons))
}

type CoverageRecordOwned = (String, Vec<String>);

/// M6: the same site id dead in one row and live in the other: the gap stays.
#[test]
fn m6_collided_dead_and_live_keeps_the_gap() {
    let (rows, cov) = collided_work("        Strategy.NextKm(62);");
    let site = |r: &ModelRoutine| r.call_sites[0].id.clone();
    assert_eq!(
        site(&rows[0]),
        site(&rows[1]),
        "precondition: both arms' calls share one site id"
    );
    assert_eq!(
        cov,
        (
            "partial".to_string(),
            vec!["interface-open-world".to_string()]
        ),
        "the live row's site keeps the gap"
    );
}

/// M7: the site dead in one row and absent from the other: no gap.
#[test]
fn m7_collided_dead_and_absent_drops_the_gap() {
    let (rows, cov) = collided_work("");
    assert!(
        rows[1].call_sites.is_empty(),
        "precondition: arm B has no call"
    );
    assert_eq!(
        cov,
        ("complete".to_string(), Vec::<String>::new()),
        "a row lacking the site does not vote"
    );
}

// ---------------------------------------------------------------------------
// G1-G12: the trust gate
// ---------------------------------------------------------------------------

/// G1: a preprocessor directive in the routine keeps the dead-marked gap AND
/// the dead-marked edge to the committing `DoCommitP`, in each shape the
/// lowerer splices.
#[test]
fn g1_directive_in_the_routine_keeps_gap_and_edge() {
    let m = build(SRC);
    let names = [
        "OnIfExitThenCall",
        "OnIfExitElseCall",
        "OnIfArmPreprocExit",
        "OnElifExit",
        "OnNestedExit",
    ];
    for name in names {
        assert_dead_site(&m, name, "DoCommitP");
    }
    let mut failures = Vec::new();
    for (name, s) in names.iter().zip(seen(&m, &names)) {
        if s.direct_status != "partial" || !s.reasons.iter().any(|r| r == "interface-open-world") {
            failures.push(format!(
                "{name}: gap dropped: {} {:?}",
                s.direct_status, s.reasons
            ));
        }
        if s.commits != EffectPresence::Yes {
            failures.push(format!(
                "{name}: edge to DoCommitP dropped: {:?}",
                s.commits
            ));
        }
    }
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}

/// G2: directives spelled with a space after `#` are detected.
#[test]
fn g2_spaced_directives_are_detected() {
    let spaced = "procedure P()\nbegin\n# if SPACED288\n    exit;\n# endif\nend;\n";
    assert!(!spaced.contains("#if") && !spaced.contains("#endif"));
    assert!(has_preproc_directive(spaced), "spaced `# if` / `# endif`");
    assert!(has_preproc_directive("  #ELSE\n"), "indented, upper case");
    assert!(
        !has_preproc_directive("#pragma warning disable AL0432\n#region R\n"),
        "#pragma and #region are not conditional directives"
    );
    let m = build(SRC);
    assert_dead_site(&m, "OnSpacedDirective", "NextKm");
    assert_keeps_gap(&seen1(&m, "OnSpacedDirective"), "OnSpacedDirective");
}

/// G3: `Error(ErrInfo); Call();` keeps the gap (a collectible error goes on).
#[test]
fn g3_error_errorinfo_keeps_the_gap() {
    let m = build(SRC);
    assert_dead_site(&m, "OnCollectibleError", "NextKm");
    assert_keeps_gap(&seen1(&m, "OnCollectibleError"), "OnCollectibleError");
}

/// G4: one-argument `Error` forms not proven Text keep the gap, one assertion
/// each: parenthesised, a call, a field, an array element.
#[test]
fn g4_unproven_one_argument_forms_keep_the_gap() {
    let m = build(SRC);
    let names = [
        "OnParenErrorInfo",
        "OnCallErrorInfo",
        "OnFieldError",
        "OnArrayError",
    ];
    for name in names {
        assert_dead_site(&m, name, "NextKm");
    }
    assert_all_keep_gap(&m, &names);
}

/// [`assert_keeps_gap`] for each of `names`, every one checked, so one break
/// shows which rows it moves.
fn assert_all_keep_gap(m: &Model, names: &[&str]) {
    let failures: Vec<String> = names
        .iter()
        .zip(seen(m, names))
        .filter(|(_, s)| {
            (s.direct_status.as_str(), s.reasons.as_slice())
                != (
                    "partial",
                    &["interface-open-world".to_string()] as &[String],
                )
        })
        .map(|(name, s)| format!("{name}: gap dropped: {} {:?}", s.direct_status, s.reasons))
        .collect();
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}

/// G5: `Error('t'); Call();` drops the gap (a string literal is Text).
#[test]
fn g5_error_string_literal_drops_the_gap() {
    let m = build(SRC);
    assert_dead_site(&m, "OnTextError", "NextKm");
    assert_complete_no_d35(&seen1(&m, "OnTextError"), "OnTextError");
}

/// G6: `Error(StopLbl, 1)` (two arguments) and `Error(TextLocal)` drop the gap.
#[test]
fn g6_error_format_and_text_local_drop_the_gap() {
    let m = build(SRC);
    let names = ["OnLabelError", "OnTextLocalError"];
    for name in names {
        assert_dead_site(&m, name, "NextKm");
    }
    for (name, s) in names.iter().zip(seen(&m, &names)) {
        assert_complete_no_d35(&s, name);
    }
}

/// G7: a directive elsewhere in the routine distrusts the whole row, so a
/// plain `exit; Call();` keeps its gap.
#[test]
fn g7_directive_elsewhere_keeps_a_plain_exit_gap() {
    let m = build(SRC);
    assert_dead_site(&m, "OnDirectiveElsewhere", "NextKm");
    assert_keeps_gap(&seen1(&m, "OnDirectiveElsewhere"), "OnDirectiveElsewhere");
}

/// G8: a global whose type is chosen by an `#if` around the object's `var`
/// section proves nothing, in both declaration orders; an unconditional Text
/// global in a directive-free object drops the gap.
#[test]
fn g8_global_types_across_if_prove_nothing() {
    let m = build(SRC);
    let names = [
        "OnGlobalTextFirst",
        "OnGlobalErrInfoFirst",
        "OnGlobalTextPlain",
    ];
    for name in names {
        assert_dead_site(&m, name, "NextKm");
    }
    let s = seen(&m, &names);
    assert_keeps_gap(&s[0], names[0]);
    assert_keeps_gap(&s[1], names[1]);
    assert_complete_no_d35(&s[2], names[2]);
}

/// G9: an own `procedure Error` shadows the builtin, so `Error('x')` and
/// `Error('x %1', 1)` run it and return: the gap stays.
#[test]
fn g9_own_error_procedure_keeps_the_gap() {
    let m = build(SRC);
    let names = ["OnOwnErrorText", "OnOwnErrorFormat"];
    for name in names {
        assert_dead_site(&m, name, "NextKm");
    }
    assert_all_keep_gap(&m, &names);
}

/// G10: fail closed. A row built without assembly (a test literal) is
/// untrusted, so its dead site is not in the set; the same row marked trusted
/// is. A range that does not index the source is not clean.
#[test]
fn g10_the_gate_fails_closed() {
    let mut cs = call_site("r/cs0", "Strategy.NextKm", vec![]);
    cs.control_context = Some("unreachable".to_string());
    let mut r = routine("r", "procedure");
    r.call_sites = vec![cs];
    let rows = vec![r];
    assert!(
        !DeadSites::of(&rows).contains("r", Some("r/cs0")),
        "a test-literal row is untrusted"
    );
    let mut trusted = rows.clone();
    trusted[0].dead_marks_trusted = true;
    assert!(
        DeadSites::of(&trusted).contains("r", Some("r/cs0")),
        "control: the same row, trusted, has the dead site"
    );
    let source = "begin end;";
    assert!(source_clean(source, &(0..5)), "control: an indexing range");
    assert!(
        !source_clean(source, &(4..99)),
        "a range past the source is not clean"
    );
}

/// The G11 assertions: `work` keeps its dead-marked interface gap, and its
/// subscriber `sub` stays unknown with `work` partly analysed.
fn assert_named_return_gap(work: &str, sub: &str, object: &str) {
    let m = build(SRC);
    assert_dead_site(&m, work, "NextKm");
    let s = seen(&m, &[work, sub]);
    assert_keeps_gap(&s[0], work);
    assert_d35_reasons(
        &s[1],
        sub,
        &format!("interface-open-world; partly analysed: {work} (Codeunit {object})"),
    );
}

/// G11: a named ErrorInfo return shadows a Text global of the same name, so
/// `Error(StopValue)` takes the ErrorInfo overload and the interface call
/// after it may run: the callee keeps its gap and the subscriber stays
/// unknown.
#[test]
fn g11_named_errorinfo_return_keeps_the_gap() {
    assert_named_return_gap("WorkErr", "OnNamedReturnGap", "50290");
}

/// G11u: the same with the reference `"STOPVÄLUE"`, the return `"StopVälue"`
/// and the global `"STOPVÄLUE"`: the reference names the return only under
/// AL's Unicode-aware identifier equality (`Ä`/`ä`).
#[test]
fn g11u_non_ascii_named_return_keeps_the_gap() {
    assert_named_return_gap("WorkErrU", "OnNamedReturnGapU", "50291");
}

/// G12 (ASCII): the same shadowing keeps the dead-marked edge to the
/// committing callee; the Text named return (control) drops the dead call.
#[test]
fn g12_named_errorinfo_return_keeps_the_edge() {
    let m = build(SRC);
    assert_dead_site(&m, "WorkCommit", "DoCommit5");
    assert_dead_site(&m, "WorkText", "NextKm");
    let s = seen(&m, &["OnNamedReturnEdge", "OnNamedReturnText"]);
    assert_eq!(
        s[0].commits,
        EffectPresence::Yes,
        "OnNamedReturnEdge commits"
    );
    assert_d35_via_callee(&s[0], "OnNamedReturnEdge");
    assert_complete_no_d35(&s[1], "OnNamedReturnText");
}

/// G12u: the non-ASCII spelling keeps the edge too.
#[test]
fn g12u_non_ascii_named_return_keeps_the_edge() {
    let m = build(SRC);
    assert_dead_site(&m, "WorkCommitU", "DoCommitU");
    let s = seen1(&m, "OnNamedReturnEdgeU");
    assert_eq!(s.commits, EffectPresence::Yes, "OnNamedReturnEdgeU commits");
    assert_d35_via_callee(&s, "OnNamedReturnEdgeU");
}

// ---------------------------------------------------------------------------
// D1, D1': d35 names what the fold keeps
// ---------------------------------------------------------------------------

/// D1: live then dead in a trusted row: d35 names only the live site, and the
/// subscriber leaves "partly analysed".
#[test]
fn d1_mixed_live_dead_names_only_the_live_site() {
    let m = build(SRC);
    assert_trusted(&m, "OnMixedLiveDead");
    let s = seen1(&m, "OnMixedLiveDead");
    assert_d35_reasons(
        &s,
        "OnMixedLiveDead",
        &format!(
            "interface-open-world; unresolved: Strategy.NextKm (line {})",
            line_of("NextKm(81)")
        ),
    );
}

/// D1': an untrusted row names its dead-marked site (the gap it keeps).
#[test]
fn d1p_untrusted_row_names_its_dead_site() {
    let m = build(SRC);
    assert_dead_site(&m, "OnCollectibleError", "NextKm");
    let s = seen1(&m, "OnCollectibleError");
    assert_d35_reasons(
        &s,
        "OnCollectibleError",
        &format!(
            "interface-open-world; unresolved: Strategy.NextKm (line {})",
            line_of("NextKm(117)")
        ),
    );
}

/// Controls the fixture keeps unchanged: a live call, and a call after
/// `asserterror Error(...)` (it runs), stay unknown with their site named.
#[test]
fn controls_live_and_asserterror_stay_unknown() {
    let m = build(SRC);
    let s = seen(&m, &["OnAfterValidateLive", "OnBeforePrintAssertError"]);
    for (name, s, arg) in [
        ("OnAfterValidateLive", &s[0], "NextKm(2)"),
        ("OnBeforePrintAssertError", &s[1], "NextKm(3)"),
    ] {
        assert_d35_reasons(
            s,
            name,
            &format!(
                "interface-open-world; unresolved: Strategy.NextKm (line {})",
                line_of(arg)
            ),
        );
    }
}

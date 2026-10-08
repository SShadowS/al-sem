//! G-19 (docs/engine-gaps.md) — closed-world temp inference for `local` routines.
//!
//! d1/d3/d10 fire inside a callee on a `var Record X` parameter that LACKS the
//! `temporary` keyword even when every caller in the app passes a temporary
//! local. Firing is OPEN-WORLD CORRECT in general (a public/internal routine can
//! be called elsewhere with a physical record), but there is a PROVABLY-SOUND
//! closed-world subset:
//!
//!   the routine is `local` (callable ONLY within its owning object — language
//!   rule) AND every same-object call site that could name it is RESOLVED AND
//!   every resolved caller edge is a binding-carrying kind (`direct`/`method`)
//!   AND each caller's argument for that parameter is proven `Known(true)`
//!   temporary (directly, or recursively through another closed-world-proven
//!   `local` forwarding param).
//!
//! Only that exact proof suppresses (d3/d10 skip; d1 downgrades to `info` like
//! any other Known(true) temp). EVERY control below must keep firing:
//!   - one caller passes a PHYSICAL record            → fires
//!   - the routine is public/internal (open world)    → fires
//!   - an unresolved same-object call names the routine → fires
//!   - the local routine has NO callers (no vacuous proof) → fires
//!   - the routine is an event subscriber (runtime-invoked) → fires
//!
//! GUARD (source-fix path): a param WITH the `temporary` keyword is Known(true)
//! by contract-trust — already suppressed with zero callers.
//!
//! Drives the REAL detectors in-process over inline AL workspaces, exactly like
//! `tests/gap_g13_temp_gate.rs`.

use al_sem::engine::l5::detectors::registered_detectors;
use al_sem::engine::l5::finding::Finding;
use al_sem::engine::l5::registry::run_detectors;
use al_sem::program::model::program_calls::assemble_and_resolve_inline_program_default;

const APP_GUID: &str = "11111111-0000-0000-0000-0000000g19ab";

/// Run a single detector in isolation over an inline workspace.
fn run_detector(detector_name: &str, files: &[(String, String)]) -> Vec<Finding> {
    let resolved = assemble_and_resolve_inline_program_default(files, APP_GUID);
    let selected: Vec<_> = registered_detectors()
        .into_iter()
        .filter(|d| d.name == detector_name)
        .collect();
    assert_eq!(
        selected.len(),
        1,
        "{detector_name} must be registered exactly once"
    );
    run_detectors(&resolved, &selected).findings
}

fn al(name: &str, body: &str) -> (String, String) {
    (format!("src/{name}.al"), body.to_string())
}

fn summarize(findings: &[Finding]) -> Vec<(String, String, String)> {
    findings
        .iter()
        .map(|f| (f.id.clone(), f.severity.clone(), f.root_cause.clone()))
        .collect()
}

const TABLE_SRC: &str = r#"
table 50190 "G19 Line"
{
    fields
    {
        field(1; "No."; Code[20]) { }
        field(2; Description; Text[100]) { }
        // Never read: gives d3 a column to trim (S8.5's "nothing to trim" rule
        // silences a table whose every loadable field is read).
        field(3; Amount; Decimal) { }
    }
    keys { key(PK; "No.") { } }
}
"#;

// ---------------------------------------------------------------------------
// d10 — the closed-world proof suppresses; every open-world control fires.
// ---------------------------------------------------------------------------

/// A `local` routine with a keyword-less by-var record param, called ONLY from
/// resolved sites that all pass `temporary` locals → the param is provably
/// temporary (closed world) → d10 must NOT fire on the in-loop self-Delete.
#[test]
fn d10_local_all_temp_callers_suppressed() {
    let cu = r#"
codeunit 50190 "G19 CW Temp"
{
    local procedure BulkPrune(var Buf: Record "G19 Line")
    begin
        if Buf.FindSet() then
            repeat
                Buf.Delete();
            until Buf.Next() = 0;
    end;

    procedure RunA()
    var
        TempLine: Record "G19 Line" temporary;
    begin
        BulkPrune(TempLine);
    end;

    procedure RunB()
    var
        TempOther: Record "G19 Line" temporary;
    begin
        BulkPrune(TempOther);
    end;
}
"#;
    let findings = run_detector(
        "d10-self-modifying-loop",
        &[al("G19Line", TABLE_SRC), al("G19CwTemp", cu)],
    );
    assert!(
        findings.is_empty(),
        "closed-world proven-temp param must suppress d10. findings: {:#?}",
        summarize(&findings)
    );
}

/// CONTROL: ONE caller passes a PHYSICAL record → no proof → d10 STILL fires.
#[test]
fn d10_local_physical_caller_still_fires() {
    let cu = r#"
codeunit 50191 "G19 CW Phys"
{
    local procedure BulkPrune(var Buf: Record "G19 Line")
    begin
        if Buf.FindSet() then
            repeat
                Buf.Delete();
            until Buf.Next() = 0;
    end;

    procedure RunA()
    var
        TempLine: Record "G19 Line" temporary;
    begin
        BulkPrune(TempLine);
    end;

    procedure RunB()
    var
        Line: Record "G19 Line";
    begin
        BulkPrune(Line);
    end;
}
"#;
    let findings = run_detector(
        "d10-self-modifying-loop",
        &[al("G19Line", TABLE_SRC), al("G19CwPhys", cu)],
    );
    assert_eq!(
        findings.len(),
        1,
        "a physical caller breaks the closed-world proof — d10 must fire. findings: {:#?}",
        summarize(&findings)
    );
}

/// CONTROL: the routine is PUBLIC (no `local`) — open world even though every
/// in-app caller passes temp → d10 STILL fires.
#[test]
fn d10_public_all_temp_callers_still_fires() {
    let cu = r#"
codeunit 50192 "G19 CW Public"
{
    procedure BulkPrune(var Buf: Record "G19 Line")
    begin
        if Buf.FindSet() then
            repeat
                Buf.Delete();
            until Buf.Next() = 0;
    end;

    procedure RunA()
    var
        TempLine: Record "G19 Line" temporary;
    begin
        BulkPrune(TempLine);
    end;
}
"#;
    let findings = run_detector(
        "d10-self-modifying-loop",
        &[al("G19Line", TABLE_SRC), al("G19CwPublic", cu)],
    );
    assert_eq!(
        findings.len(),
        1,
        "a public routine is open-world — all-temp in-app callers are NOT a proof. findings: {:#?}",
        summarize(&findings)
    );
}

/// CONTROL: `internal` is also open world (other objects in the app — and apps
/// granted internals access — can call it) → d10 STILL fires.
#[test]
fn d10_internal_all_temp_callers_still_fires() {
    let cu = r#"
codeunit 50193 "G19 CW Internal"
{
    internal procedure BulkPrune(var Buf: Record "G19 Line")
    begin
        if Buf.FindSet() then
            repeat
                Buf.Delete();
            until Buf.Next() = 0;
    end;

    procedure RunA()
    var
        TempLine: Record "G19 Line" temporary;
    begin
        BulkPrune(TempLine);
    end;
}
"#;
    let findings = run_detector(
        "d10-self-modifying-loop",
        &[al("G19Line", TABLE_SRC), al("G19CwInternal", cu)],
    );
    assert_eq!(
        findings.len(),
        1,
        "an internal routine is not closed-world — d10 must fire. findings: {:#?}",
        summarize(&findings)
    );
}

/// CONTROL: a same-object call site NAMES the routine but does not resolve
/// (arity mismatch) — a potential caller the engine cannot see through → the
/// closed world is broken → d10 STILL fires.
#[test]
fn d10_unresolved_same_object_caller_still_fires() {
    let cu = r#"
codeunit 50194 "G19 CW Unresolved"
{
    local procedure BulkPrune(var Buf: Record "G19 Line")
    begin
        if Buf.FindSet() then
            repeat
                Buf.Delete();
            until Buf.Next() = 0;
    end;

    procedure RunA()
    var
        TempLine: Record "G19 Line" temporary;
    begin
        BulkPrune(TempLine);
    end;

    procedure RunBad()
    var
        TempBad: Record "G19 Line" temporary;
    begin
        BulkPrune(TempBad, 1);
    end;
}
"#;
    let findings = run_detector(
        "d10-self-modifying-loop",
        &[al("G19Line", TABLE_SRC), al("G19CwUnresolved", cu)],
    );
    assert_eq!(
        findings.len(),
        1,
        "an unresolved name-matching same-object call breaks the proof — d10 must fire. findings: {:#?}",
        summarize(&findings)
    );
}

/// CONTROL: a `local` routine with NO callers at all — the proof is refused
/// (no vacuous dead-code suppression) → d10 STILL fires.
#[test]
fn d10_local_no_callers_still_fires() {
    let cu = r#"
codeunit 50195 "G19 CW Dead"
{
    local procedure BulkPrune(var Buf: Record "G19 Line")
    begin
        if Buf.FindSet() then
            repeat
                Buf.Delete();
            until Buf.Next() = 0;
    end;
}
"#;
    let findings = run_detector(
        "d10-self-modifying-loop",
        &[al("G19Line", TABLE_SRC), al("G19CwDead", cu)],
    );
    assert_eq!(
        findings.len(),
        1,
        "zero callers must NOT vacuously prove temp — d10 must fire. findings: {:#?}",
        summarize(&findings)
    );
}

/// CONTROL: a `local` EVENT SUBSCRIBER is runtime-invoked with publisher args —
/// never closed-world, even with an all-temp direct caller → d10 STILL fires.
#[test]
fn d10_local_event_subscriber_still_fires() {
    let publisher = r#"
codeunit 50196 "G19 CW Pub"
{
    [IntegrationEvent(false, false)]
    procedure OnDo(var Buf: Record "G19 Line")
    begin
    end;
}
"#;
    let subscriber = r#"
codeunit 50197 "G19 CW Sub"
{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::"G19 CW Pub", 'OnDo', '', false, false)]
    local procedure Handle(var Buf: Record "G19 Line")
    begin
        if Buf.FindSet() then
            repeat
                Buf.Delete();
            until Buf.Next() = 0;
    end;

    procedure RunLocal()
    var
        TempLine: Record "G19 Line" temporary;
    begin
        Handle(TempLine);
    end;
}
"#;
    let findings = run_detector(
        "d10-self-modifying-loop",
        &[
            al("G19Line", TABLE_SRC),
            al("G19CwPub", publisher),
            al("G19CwSub", subscriber),
        ],
    );
    assert_eq!(
        findings.len(),
        1,
        "an event subscriber is runtime-invoked — never closed-world. findings: {:#?}",
        summarize(&findings)
    );
}

/// A forwarding CHAIN: local Outer forwards its keyword-less by-var param to
/// local Inner (the op lives in Inner); the only root caller passes temp →
/// the proof chases the PD binding recursively → suppressed.
#[test]
fn d10_local_forwarding_chain_suppressed() {
    let cu = r#"
codeunit 50198 "G19 CW Chain"
{
    local procedure Outer(var Buf: Record "G19 Line")
    begin
        Inner(Buf);
    end;

    local procedure Inner(var Buf: Record "G19 Line")
    begin
        if Buf.FindSet() then
            repeat
                Buf.Delete();
            until Buf.Next() = 0;
    end;

    procedure Run()
    var
        TempLine: Record "G19 Line" temporary;
    begin
        Outer(TempLine);
    end;
}
"#;
    let findings = run_detector(
        "d10-self-modifying-loop",
        &[al("G19Line", TABLE_SRC), al("G19CwChain", cu)],
    );
    assert!(
        findings.is_empty(),
        "the PD forwarding chain through local routines must be chased to the temp root. findings: {:#?}",
        summarize(&findings)
    );
}

/// GUARD (source-fix path): the param carries the `temporary` KEYWORD →
/// contract-trust Known(true) — suppressed even with NO callers (this is the
/// recommended source fix for open-world shapes).
#[test]
fn d10_keyword_temporary_param_suppressed() {
    let cu = r#"
codeunit 50199 "G19 Keyword"
{
    local procedure BulkPrune(var Buf: Record "G19 Line" temporary)
    begin
        if Buf.FindSet() then
            repeat
                Buf.Delete();
            until Buf.Next() = 0;
    end;
}
"#;
    let findings = run_detector(
        "d10-self-modifying-loop",
        &[al("G19Line", TABLE_SRC), al("G19Keyword", cu)],
    );
    assert!(
        findings.is_empty(),
        "a keyword-`temporary` param is Known(true) by contract — must not fire. findings: {:#?}",
        summarize(&findings)
    );
}

// ---------------------------------------------------------------------------
// d1 — proven param resolves Known(true): severity downgraded to info.
// ---------------------------------------------------------------------------

/// The d1 in-loop Delete inside the proven-temp local routine must be
/// downgraded to `info` (the Known(true) temp treatment), not fire as a
/// temp-state-uncertain warning.
#[test]
fn d1_local_all_temp_callers_downgraded_to_info() {
    let cu = r#"
codeunit 50200 "G19 D1 Temp"
{
    local procedure BulkPrune(var Buf: Record "G19 Line")
    begin
        if Buf.FindSet() then
            repeat
                Buf.Delete();
            until Buf.Next() = 0;
    end;

    procedure RunA()
    var
        TempLine: Record "G19 Line" temporary;
    begin
        BulkPrune(TempLine);
    end;
}
"#;
    let findings = run_detector(
        "d1-db-op-in-loop",
        &[al("G19Line", TABLE_SRC), al("G19D1Temp", cu)],
    );
    assert!(
        !findings.is_empty(),
        "d1 still reports the in-loop op (as info)"
    );
    assert!(
        findings.iter().all(|f| f.severity == "info"),
        "a closed-world proven-temp param must downgrade every d1 path to info. findings: {:#?}",
        summarize(&findings)
    );
}

/// CONTROL: with a physical caller in the mix, d1 must keep a non-info finding.
#[test]
fn d1_local_physical_caller_keeps_firing() {
    let cu = r#"
codeunit 50201 "G19 D1 Phys"
{
    local procedure BulkPrune(var Buf: Record "G19 Line")
    begin
        if Buf.FindSet() then
            repeat
                Buf.Delete();
            until Buf.Next() = 0;
    end;

    procedure RunA()
    var
        Line: Record "G19 Line";
    begin
        BulkPrune(Line);
    end;
}
"#;
    let findings = run_detector(
        "d1-db-op-in-loop",
        &[al("G19Line", TABLE_SRC), al("G19D1Phys", cu)],
    );
    assert!(
        findings.iter().any(|f| f.severity != "info"),
        "a physical caller path must keep d1 firing above info. findings: {:#?}",
        summarize(&findings)
    );
}

// ---------------------------------------------------------------------------
// d3 — proven param treated like a Known(true) temp record (no SQL benefit).
// ---------------------------------------------------------------------------

/// d3 (missing SetLoadFields) on a retrieval+field-read of the proven-temp
/// param → suppressed (in-memory record, SetLoadFields has no SQL benefit).
#[test]
fn d3_local_all_temp_callers_suppressed() {
    let cu = r#"
codeunit 50202 "G19 D3 Temp"
{
    local procedure ReadDesc(var Buf: Record "G19 Line"): Text
    begin
        if Buf.FindFirst() then
            exit(Buf.Description);
    end;

    procedure Run(): Text
    var
        TempLine: Record "G19 Line" temporary;
    begin
        exit(ReadDesc(TempLine));
    end;
}
"#;
    let findings = run_detector(
        "d3-missing-setloadfields",
        &[al("G19Line", TABLE_SRC), al("G19D3Temp", cu)],
    );
    assert!(
        findings.is_empty(),
        "closed-world proven-temp param must suppress d3. findings: {:#?}",
        summarize(&findings)
    );
}

/// CONTROL: identical shape but the caller passes a PHYSICAL record → d3 fires.
#[test]
fn d3_local_physical_caller_still_fires() {
    let cu = r#"
codeunit 50203 "G19 D3 Phys"
{
    local procedure ReadDesc(var Buf: Record "G19 Line"): Text
    begin
        if Buf.FindFirst() then
            exit(Buf.Description);
    end;

    procedure Run(): Text
    var
        Line: Record "G19 Line";
    begin
        exit(ReadDesc(Line));
    end;
}
"#;
    let findings = run_detector(
        "d3-missing-setloadfields",
        &[al("G19Line", TABLE_SRC), al("G19D3Phys", cu)],
    );
    assert_eq!(
        findings.len(),
        1,
        "a physical caller breaks the proof — d3 must fire. findings: {:#?}",
        summarize(&findings)
    );
}

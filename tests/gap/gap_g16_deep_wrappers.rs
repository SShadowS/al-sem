//! Gap G-16 (docs/engine-gaps.md): record loaded via DEEPER wrappers /
//! record-assignment not recognized — extends G-10.
//!
//! `d11-modify-without-get` / `d21-read-without-load` must NOT fire when the
//! record WAS loaded, just not via a one-hop literal `Get`/`Find`:
//!
//! - (a) MULTI-HOP wrappers: the by-var arg is forwarded by-var through a
//!   bounded chain of resolved callees and a recognized load op lands on it
//!   deeper down (`FindTemplate` -> `FindTemplateWithReportID` -> `FindSet`),
//!   including Get-or-Insert facades (`InsertIfNotExists`: Reset; if not Get
//!   then Init+Insert — the record is in a defined state either way).
//! - (b) RECORD ASSIGNMENT: `RecB := RecA` loads `RecB` when `RecA` is itself
//!   provably loaded at the assignment point (prior Get/Find/call-load, a
//!   platform-loaded trigger `Rec`, or a further assignment from a loaded var
//!   — bounded chain).
//!
//! Suppression-direction guardrails (controls below): no load still fires; a
//! deep chain that never loads still fires; a load BEYOND the hop bound still
//! fires; assignment from an UNLOADED var still fires; assignment AFTER the op
//! or RHS loaded only after the assignment still fires.
//!
//! Drives the REAL detectors over inline AL workspaces (mirrors
//! `tests/gap_g10_load_wrappers.rs`).

use al_sem::engine::l5::detectors::registered_detectors;
use al_sem::engine::l5::finding::Finding;
use al_sem::engine::l5::registry::run_detectors;
use al_sem::program::model::program_calls::assemble_and_resolve_inline_program_default;

const APP_GUID: &str = "11111111-0000-0000-0000-000000g16abc";

/// Run d11 + d21 over an inline workspace and return all emitted findings.
fn run_g16_detectors(files: &[(String, String)]) -> Vec<Finding> {
    let resolved = assemble_and_resolve_inline_program_default(files, APP_GUID);
    let wanted = ["d11-modify-without-get", "d21-read-without-load"];
    let detectors: Vec<_> = registered_detectors()
        .into_iter()
        .filter(|d| wanted.contains(&d.name.as_str()))
        .collect();
    assert_eq!(
        detectors.len(),
        2,
        "d11/d21 must each be registered exactly once"
    );
    run_detectors(&resolved, &detectors).findings
}

fn al(name: &str, body: &str) -> (String, String) {
    (format!("src/{name}.al"), body.to_string())
}

const TABLE_SRC: &str = r#"
table 50160 "G16 Item"
{
    fields
    {
        field(1; "No."; Code[20]) { }
        field(2; Description; Text[100]) { }
    }
    keys { key(PK; "No.") { } }
}
"#;

// --- (a) multi-hop wrapper loads ------------------------------------------------

/// 2-hop chain: the caller passes `Item` by-var into `LoadIt`, which forwards
/// it by-var into `LoadInner`, which `Get`s it. After the call the record is
/// loaded → NO d11/d21.
#[test]
fn two_hop_wrapper_load_suppresses_d11_and_d21() {
    let cu_src = r#"
codeunit 50160 "G16 TwoHop"
{
    procedure ModifyAfterDeepLoad()
    var Item: Record "G16 Item";
    begin
        LoadIt(Item);
        Item.Modify();
    end;

    procedure ReadAfterDeepLoad()
    var Item: Record "G16 Item";
    begin
        LoadIt(Item);
        Item.TestField(Description);
    end;

    local procedure LoadIt(var R: Record "G16 Item")
    begin
        LoadInner(R);
    end;

    local procedure LoadInner(var R: Record "G16 Item")
    begin
        R.Get('A');
    end;
}
"#;
    let findings = run_g16_detectors(&[al("G16Item", TABLE_SRC), al("G16TwoHop", cu_src)]);
    assert!(
        findings.is_empty(),
        "a 2-hop by-var wrapper chain ending in Get loads the record — d11/d21 \
         must not fire. findings: {findings:#?}"
    );
}

/// Non-Get-named 2-hop wrapper: `FindTemplate` -> `FindTemplateWithReportID`
/// -> `FindSet` (the CDO shape from the gap evidence). NO d11.
#[test]
fn find_template_chain_suppresses_d11() {
    let cu_src = r#"
codeunit 50161 "G16 FindTemplate"
{
    procedure UseTemplate()
    var Template: Record "G16 Item";
    begin
        FindTemplate(Template);
        Template.Modify();
    end;

    local procedure FindTemplate(var T: Record "G16 Item")
    begin
        FindTemplateWithReportID(T);
    end;

    local procedure FindTemplateWithReportID(var T: Record "G16 Item")
    begin
        T.SetRange("No.", 'X');
        T.FindSet();
    end;
}
"#;
    let findings = run_g16_detectors(&[al("G16Item", TABLE_SRC), al("G16FindTemplate", cu_src)]);
    assert!(
        findings.is_empty(),
        "FindTemplate -> FindTemplateWithReportID -> FindSet loads the record — \
         d11 must not fire. findings: {findings:#?}"
    );
}

/// Get-or-Insert facade (`InsertIfNotExists` shape): `Reset; if not Get then
/// begin Init; Insert; end` — the record is in a defined state after the call
/// either way. One hop (regression guard on the G-10 summary). NO d11.
#[test]
fn insert_if_not_exists_facade_suppresses_d11() {
    let cu_src = r#"
codeunit 50162 "G16 GetOrInsert"
{
    procedure EnsureAndModify()
    var Item: Record "G16 Item";
    begin
        InsertIfNotExists(Item);
        Item.Modify();
    end;

    local procedure InsertIfNotExists(var R: Record "G16 Item")
    begin
        R.Reset();
        if not R.Get('S') then begin
            R.Init();
            R.Insert();
        end;
    end;
}
"#;
    let findings = run_g16_detectors(&[al("G16Item", TABLE_SRC), al("G16GetOrInsert", cu_src)]);
    assert!(
        findings.is_empty(),
        "a Get-or-Insert facade leaves the record loaded-or-inserted — d11 must \
         not fire. findings: {findings:#?}"
    );
}

/// Boolean facade loader forwarded one extra hop:
/// `GetSetupRec(var R): Boolean` -> `GetSetupRecInner(var R)` -> `R.Get`.
/// NO d11/d21.
#[test]
fn facade_loader_chain_suppresses_d11_and_d21() {
    let cu_src = r#"
codeunit 50163 "G16 Facade"
{
    procedure UseSetup()
    var Setup: Record "G16 Item";
    begin
        if GetSetupRec(Setup) then
            Setup.TestField(Description);
        Setup.Modify();
    end;

    local procedure GetSetupRec(var R: Record "G16 Item"): Boolean
    begin
        exit(GetSetupRecInner(R));
    end;

    local procedure GetSetupRecInner(var R: Record "G16 Item"): Boolean
    begin
        exit(R.Get('S'));
    end;
}
"#;
    let findings = run_g16_detectors(&[al("G16Item", TABLE_SRC), al("G16Facade", cu_src)]);
    assert!(
        findings.is_empty(),
        "a boolean facade loader chain loads the record — d11/d21 must not fire. \
         findings: {findings:#?}"
    );
}

// --- (b) record assignment from a loaded var -------------------------------------

/// `Cust := Loaded` where `Loaded` was `Get`-loaded strictly before — the LHS
/// is loaded too. NO d11/d21.
#[test]
fn assign_from_get_loaded_var_suppresses_d11_and_d21() {
    let cu_src = r#"
codeunit 50164 "G16 Assign"
{
    procedure ReadViaAssign()
    var
        Loaded: Record "G16 Item";
        Cust: Record "G16 Item";
    begin
        Loaded.Get('A');
        Cust := Loaded;
        Cust.TestField(Description);
    end;

    procedure ModifyViaAssign()
    var
        Loaded: Record "G16 Item";
        Cust: Record "G16 Item";
    begin
        Loaded.FindFirst();
        Cust := Loaded;
        Cust.Modify();
    end;
}
"#;
    let findings = run_g16_detectors(&[al("G16Item", TABLE_SRC), al("G16Assign", cu_src)]);
    assert!(
        findings.is_empty(),
        "RecB := RecA with RecA loaded makes RecB loaded — d11/d21 must not \
         fire. findings: {findings:#?}"
    );
}

/// `Cust := Rec` inside a page `OnAction` trigger — `Rec` is platform-loaded,
/// so the assignment loads `Cust`. NO d11/d21.
#[test]
fn assign_from_page_rec_suppresses_d11() {
    let page_src = r#"
page 50160 "G16 Item Card"
{
    PageType = Card;
    SourceTable = "G16 Item";

    actions
    {
        area(processing)
        {
            action(CopyAndModify)
            {
                trigger OnAction()
                var Cust: Record "G16 Item";
                begin
                    Cust := Rec;
                    Cust.Modify();
                end;
            }
        }
    }
}
"#;
    let findings = run_g16_detectors(&[al("G16Item", TABLE_SRC), al("G16ItemCard", page_src)]);
    assert!(
        findings.is_empty(),
        "Cust := Rec in a page trigger copies the platform-loaded Rec — d11 must \
         not fire on Cust. findings: {findings:#?}"
    );
}

/// Bounded assignment CHAIN: `B := A; C := B;` with `A` loaded — `C` is
/// loaded through two links. NO d11.
#[test]
fn assign_chain_from_loaded_var_suppresses_d11() {
    let cu_src = r#"
codeunit 50165 "G16 AssignChain"
{
    procedure ChainAssign()
    var
        A: Record "G16 Item";
        B: Record "G16 Item";
        C: Record "G16 Item";
    begin
        A.Get('A');
        B := A;
        C := B;
        C.Modify();
    end;
}
"#;
    let findings = run_g16_detectors(&[al("G16Item", TABLE_SRC), al("G16AssignChain", cu_src)]);
    assert!(
        findings.is_empty(),
        "an assignment chain from a loaded var keeps the record loaded — d11 \
         must not fire. findings: {findings:#?}"
    );
}

// --- CONTROLS (must STILL fire) ---------------------------------------------------

/// No load, no assignment — d11/d21 must both STILL fire.
#[test]
fn control_no_load_still_fires() {
    let cu_src = r#"
codeunit 50166 "G16 NoLoad"
{
    procedure MutateBlind()
    var Item: Record "G16 Item";
    begin
        Item.Modify();
    end;

    procedure ReadBlind()
    var Item: Record "G16 Item";
    begin
        Item.TestField("No.");
    end;
}
"#;
    let findings = run_g16_detectors(&[al("G16Item", TABLE_SRC), al("G16NoLoad", cu_src)]);
    assert!(
        findings
            .iter()
            .any(|f| f.detector == "d11-modify-without-get"
                && f.root_cause.contains("MutateBlind")),
        "d11 must still fire on Modify with no prior load. findings: {findings:#?}"
    );
    assert!(
        findings
            .iter()
            .any(|f| f.detector == "d21-read-without-load" && f.root_cause.contains("ReadBlind")),
        "d21 must still fire on TestField with no prior load. findings: {findings:#?}"
    );
}

/// A 2-hop chain whose deepest callee only FILTERS the by-var arg (never
/// loads it) proves nothing — d11 must STILL fire.
#[test]
fn control_deep_non_loading_chain_still_fires() {
    let cu_src = r#"
codeunit 50167 "G16 DeepFilter"
{
    procedure ModifyAfterDeepFilter()
    var Item: Record "G16 Item";
    begin
        FilterIt(Item);
        Item.Modify();
    end;

    local procedure FilterIt(var R: Record "G16 Item")
    begin
        FilterInner(R);
    end;

    local procedure FilterInner(var R: Record "G16 Item")
    begin
        R.SetRange("No.", 'A');
    end;
}
"#;
    let findings = run_g16_detectors(&[al("G16Item", TABLE_SRC), al("G16DeepFilter", cu_src)]);
    assert!(
        findings
            .iter()
            .any(|f| f.detector == "d11-modify-without-get"
                && f.root_cause.contains("ModifyAfterDeepFilter")),
        "d11 must still fire when the deep chain never loads the var-arg. \
         findings: {findings:#?}"
    );
}

/// The hop bound is REAL: a load that only happens 4 callee hops down is
/// beyond the bounded summary — d11 must STILL fire (no unbounded recursion).
#[test]
fn control_load_beyond_hop_bound_still_fires() {
    let cu_src = r#"
codeunit 50168 "G16 TooDeep"
{
    procedure ModifyAfterTooDeepLoad()
    var Item: Record "G16 Item";
    begin
        Hop1(Item);
        Item.Modify();
    end;

    local procedure Hop1(var R: Record "G16 Item")
    begin
        Hop2(R);
    end;

    local procedure Hop2(var R: Record "G16 Item")
    begin
        Hop3(R);
    end;

    local procedure Hop3(var R: Record "G16 Item")
    begin
        Hop4(R);
    end;

    local procedure Hop4(var R: Record "G16 Item")
    begin
        R.Get('A');
    end;
}
"#;
    let findings = run_g16_detectors(&[al("G16Item", TABLE_SRC), al("G16TooDeep", cu_src)]);
    assert!(
        findings
            .iter()
            .any(|f| f.detector == "d11-modify-without-get"
                && f.root_cause.contains("ModifyAfterTooDeepLoad")),
        "a load 4 hops down is beyond the bounded summary — d11 must still fire. \
         findings: {findings:#?}"
    );
}

/// `RecB := RecA` where RecA was NEVER loaded proves nothing — d11/d21 must
/// STILL fire.
#[test]
fn control_assign_from_unloaded_var_still_fires() {
    let cu_src = r#"
codeunit 50169 "G16 AssignUnloaded"
{
    procedure ModifyViaUnloadedAssign()
    var
        Source: Record "G16 Item";
        Cust: Record "G16 Item";
    begin
        Cust := Source;
        Cust.Modify();
    end;

    procedure ReadViaUnloadedAssign()
    var
        Source: Record "G16 Item";
        Cust: Record "G16 Item";
    begin
        Cust := Source;
        Cust.TestField(Description);
    end;
}
"#;
    let findings = run_g16_detectors(&[al("G16Item", TABLE_SRC), al("G16AssignUnloaded", cu_src)]);
    assert!(
        findings
            .iter()
            .any(|f| f.detector == "d11-modify-without-get"
                && f.root_cause.contains("ModifyViaUnloadedAssign")),
        "d11 must still fire when the assignment source was never loaded. \
         findings: {findings:#?}"
    );
    assert!(
        findings
            .iter()
            .any(|f| f.detector == "d21-read-without-load"
                && f.root_cause.contains("ReadViaUnloadedAssign")),
        "d21 must still fire when the assignment source was never loaded. \
         findings: {findings:#?}"
    );
}

/// The assignment must be STRICTLY BEFORE the op — `Cust.Modify(); Cust :=
/// Loaded;` proves nothing. d11 must STILL fire.
#[test]
fn control_assign_after_op_still_fires() {
    let cu_src = r#"
codeunit 50170 "G16 LateAssign"
{
    procedure ModifyThenAssign()
    var
        Loaded: Record "G16 Item";
        Cust: Record "G16 Item";
    begin
        Loaded.Get('A');
        Cust.Modify();
        Cust := Loaded;
    end;
}
"#;
    let findings = run_g16_detectors(&[al("G16Item", TABLE_SRC), al("G16LateAssign", cu_src)]);
    assert!(
        findings
            .iter()
            .any(|f| f.detector == "d11-modify-without-get"
                && f.root_cause.contains("ModifyThenAssign")),
        "d11 must still fire when the assignment comes AFTER the Modify. \
         findings: {findings:#?}"
    );
}

/// The RHS must be loaded BEFORE the assignment — `Cust := Source;
/// Source.Get(...); Cust.Modify();` copies an UNLOADED record. d11 must
/// STILL fire.
#[test]
fn control_rhs_loaded_after_assignment_still_fires() {
    let cu_src = r#"
codeunit 50171 "G16 RhsLateLoad"
{
    procedure AssignThenLoadSource()
    var
        Source: Record "G16 Item";
        Cust: Record "G16 Item";
    begin
        Cust := Source;
        Source.Get('A');
        Cust.Modify();
    end;
}
"#;
    let findings = run_g16_detectors(&[al("G16Item", TABLE_SRC), al("G16RhsLateLoad", cu_src)]);
    assert!(
        findings
            .iter()
            .any(|f| f.detector == "d11-modify-without-get"
                && f.root_cause.contains("AssignThenLoadSource")),
        "d11 must still fire when the RHS was loaded only AFTER the assignment. \
         findings: {findings:#?}"
    );
}

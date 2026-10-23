//! #148 — d35 on an UNKNOWN commit answer (ledger `.agent/issue-148/ledger.md`,
//! design D1/D2, acceptance rows A1-A3).
//!
//! The reporter's repro: an event subscriber calls a helper that runs a modal
//! page (`ReasonPage.RunModal()`). `RunModal` is not a commit, in the engine or in
//! Business Central. The page declares no `OnOpenPage`, so the run reaches no
//! routine and the analyze adapter records it as `object-run-unresolved`. The
//! subscriber's cone is then `partial`, and `mayCommit` answers UNKNOWN, not YES.
//! d35 still reports it (an unproven-absent commit is worth surfacing), but in its
//! own words: "may be reachable", naming the coverage reasons.
//!
//! Five variants run the REAL gate `analyze` pipeline (`run_analyze_with_exit`,
//! `--format json --with-evidence`) on inline tempdir workspaces (no r0-corpus
//! fixture, so no golden family):
//!   V1 the repro: one d35 finding with the UNKNOWN wording, at `possible` capped by
//!      `unresolved-call`, under the default selection AND `--detector d35` alone;
//!   V2 `Commit()` instead of RunModal: the unchanged YES wording;
//!   V3 RunModal removed: no finding;
//!   V4 the page's source declares an entry trigger (an empty `OnOpenPage`): the
//!      cone is complete and d35 reports nothing. Known limit: a page run reaches
//!      only the entry trigger, so a Commit in another page trigger (e.g.
//!      `OnQueryClosePage`) is not seen — filed as a discovery (ledger D3a);
//!   V5 `Commit()` AND the RunModal: one finding, YES wording (yes beats unknown).
//!
//! #256 (ledger `.agent/issue-256/ledger.md`, Design §Tests): the UNKNOWN answer
//! is reported at `medium`, the direct and via-callee answers stay `high`. Pinned
//! on V1 (finding + exact `bySeverity` + `--fail-on`), on a subscriber whose only
//! callee is a symbol-only dependency method, and on CentralGauge's reproduction
//! (the interface call in a helper, and made directly in the subscriber).

use std::path::{Path, PathBuf};

use al_sem::engine::gate::filter::Scope;
use al_sem::engine::gate::run::{AnalyzeArgs, OutputFormat, run_analyze_with_exit};
use serde_json::{Value, json};

const D35: &str = "d35-commit-in-event-subscriber";

const APP_JSON: &str = r#"{
  "id": "14814814-0000-0000-0000-000000000148",
  "name": "Issue 148 Repro",
  "publisher": "RC",
  "version": "1.0.0.0"
}"#;

const TABLE: &str = r#"table 50100 "Repro Order"
{
    fields
    {
        field(1; "No."; Code[20]) { }
    }
    keys
    {
        key(PK; "No.") { Clustered = true; }
    }
}
"#;

/// The reporter's page: no `OnOpenPage`.
const PAGE_NO_TRIGGER: &str = r#"page 50100 "Reason Dialog"
{
    PageType = StandardDialog;
    layout
    {
        area(Content) { }
    }
}
"#;

/// V4's page: the same, plus an empty `OnOpenPage`.
const PAGE_WITH_TRIGGER: &str = r#"page 50100 "Reason Dialog"
{
    PageType = StandardDialog;
    layout
    {
        area(Content) { }
    }
    trigger OnOpenPage()
    begin
    end;
}
"#;

/// The subscriber codeunit; `body` is `PromptForReason`'s statement list.
fn codeunit(body: &str) -> String {
    format!(
        r#"codeunit 50100 "Delete Governor"
{{
    [EventSubscriber(ObjectType::Table, Database::"Repro Order", 'OnBeforeDeleteEvent', '', false, false)]
    local procedure OnBeforeDeleteOrder(var Rec: Record "Repro Order"; RunTrigger: Boolean)
    begin
        if GuiAllowed() then
            PromptForReason();
    end;

    local procedure PromptForReason()
    var
        ReasonPage: Page "Reason Dialog";
    begin
{body}
    end;
}}
"#
    )
}

const RUNMODAL: &str =
    "        if ReasonPage.RunModal() <> Action::OK then\n            Error('Canceled.');";
const COMMIT: &str = "        Commit();";
const NONE: &str = "        Error('Canceled.');";

/// A scratch workspace holding `files` (`(path relative to the root, text)`).
fn write_files(tag: &str, files: &[(&str, &str)]) -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::Builder::new()
        .prefix(&format!("alsem-d35-{tag}-"))
        .tempdir()
        .expect("create scratch ws");
    let root = tmp.path().to_path_buf();
    for (rel, text) in files {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).expect("create dir");
        std::fs::write(&path, text).expect("write file");
    }
    (tmp, root)
}

fn write_workspace(tag: &str, body: &str, page: &str) -> (tempfile::TempDir, PathBuf) {
    write_files(
        tag,
        &[
            ("app.json", APP_JSON),
            ("src/ReproOrder.Table.al", TABLE),
            ("src/ReasonDialog.Page.al", page),
            ("src/DeleteGovernor.Codeunit.al", &codeunit(body)),
        ],
    )
}

fn analyze(ws: &Path, detector: Option<&str>) -> Value {
    analyze_with(ws, detector, None).0
}

/// The whole analyze report and the exit code, with `--fail-on <fail_on>`.
fn analyze_with(ws: &Path, detector: Option<&str>, fail_on: Option<&str>) -> (Value, u8) {
    let args = AnalyzeArgs {
        workspace: ws.to_string_lossy().to_string(),
        min_severity: None,
        detector: detector.map(str::to_string),
        preset: None,
        scope: Scope::Primary,
        limit: None,
        format: OutputFormat::Json,
        sarif_version_override: None,
        fail_on: fail_on.map(str::to_string),
        require_dependencies: false,
        baseline: None,
        update_baseline: false,
        disable_inline_suppression: false,
        group_by: None,
        deterministic: true,
        single_app: false,
        with_evidence: true,
    };
    let (out, exit) = match run_analyze_with_exit(&args.read_inputs(), &args, "i148-test") {
        Ok((out, exit, _)) => (out, exit),
        Err(e) => panic!("run_analyze failed: {e}"),
    };
    (
        serde_json::from_str(&out).expect("analyze JSON must parse"),
        exit,
    )
}

fn by_severity(report: &Value) -> &Value {
    &report["payload"]["summary"]["bySeverity"]
}

/// The d35 findings of one run.
fn d35_findings(report: &Value) -> Vec<Value> {
    report["payload"]["findings"]
        .as_array()
        .expect("payload.findings array")
        .iter()
        .filter(|f| f["detector"] == D35)
        .cloned()
        .collect()
}

fn run_variant(tag: &str, body: &str, page: &str, detector: Option<&str>) -> Vec<Value> {
    let (_tmp, ws) = write_workspace(tag, body, page);
    d35_findings(&analyze(&ws, detector))
}

const YES_TITLE: &str = "Commit reachable from event subscriber (via callee)";
const YES_ROOT_CAUSE: &str = "OnBeforeDeleteOrder is an event subscriber that transitively \
     reaches Commit through its callees \u{2014} the publisher cannot roll back the committed \
     state if its work later fails.";
const YES_NOTE: &str =
    "[EventSubscriber] OnBeforeDeleteOrder transitively commits (mayCommit(summary) == \"yes\")";
const YES_FIX: &str = "Remove the Commit from the subscriber path. If durable side effects are \
     required, schedule them outside the publisher's transaction (e.g. a job-queue entry \
     written without Commit, processed later).";

/// The finding's id must be `d35/<its own routine id>/transitive`.
fn assert_transitive_id(f: &Value) {
    let routine = f["primaryLocation"]["routineId"]
        .as_str()
        .expect("primaryLocation.routineId");
    assert_eq!(f["id"], format!("d35/{routine}/transitive"), "id: {f:#}");
}

fn assert_unknown_finding(findings: &[Value], label: &str) {
    assert_eq!(
        findings.len(),
        1,
        "{label}: expected exactly one d35 finding, got {findings:#?}"
    );
    let f = &findings[0];
    assert_eq!(
        f["title"], "Commit may be reachable from event subscriber (incomplete call coverage)",
        "{label}: title"
    );
    assert_eq!(
        f["rootCause"],
        "OnBeforeDeleteOrder is an event subscriber whose call coverage is incomplete \
         (object-run-unresolved); a Commit cannot be ruled out \u{2014} the publisher could \
         not roll back committed state if one exists.",
        "{label}: rootCause"
    );
    assert_eq!(
        f["confidence"],
        json!({"level": "possible", "cappedBy": ["unresolved-call"]}),
        "{label}: confidence"
    );
    // #256: an unknown answer is not a found Commit.
    assert_eq!(f["severity"], "medium", "{label}: severity");
    assert_transitive_id(f);
    let path = f["evidencePath"].as_array().expect("evidencePath");
    assert_eq!(path.len(), 1, "{label}: evidencePath {path:#?}");
    assert_eq!(
        path[0]["note"],
        "[EventSubscriber] OnBeforeDeleteOrder may commit (mayCommit(summary) == \"unknown\"; \
         incomplete coverage: object-run-unresolved)",
        "{label}: evidence note"
    );
    assert_eq!(
        f["fixHint"]["description"],
        "Check what could not be analysed (object-run-unresolved); if a Commit is \
         reachable, move it out of the subscriber or defer it.",
        "{label}: fixHint"
    );
}

fn assert_yes_finding(findings: &[Value], label: &str) {
    assert_eq!(
        findings.len(),
        1,
        "{label}: expected exactly one d35 finding, got {findings:#?}"
    );
    let f = &findings[0];
    assert_eq!(f["title"], YES_TITLE, "{label}: title");
    assert_eq!(f["rootCause"], YES_ROOT_CAUSE, "{label}: rootCause");
    assert_eq!(
        f["confidence"],
        json!({"level": "possible"}),
        "{label}: confidence"
    );
    assert_eq!(f["severity"], "high", "{label}: severity");
    assert_transitive_id(f);
    let path = f["evidencePath"].as_array().expect("evidencePath");
    assert_eq!(path.len(), 1, "{label}: evidencePath {path:#?}");
    assert_eq!(path[0]["note"], YES_NOTE, "{label}: evidence note");
    assert_eq!(f["fixHint"]["description"], YES_FIX, "{label}: fixHint");
}

#[test]
fn v1_runmodal_without_entry_trigger_reports_unknown_honestly() {
    let all = run_variant("v1-all", RUNMODAL, PAGE_NO_TRIGGER, None);
    assert_unknown_finding(&all, "V1 default selection");
    let (_tmp, ws) = write_workspace("v1-d35", RUNMODAL, PAGE_NO_TRIGGER);
    let report = analyze(&ws, Some(D35));
    assert_unknown_finding(&d35_findings(&report), "V1 --detector d35");
    // #256: counted under medium only — no `high` key at all.
    assert_eq!(by_severity(&report), &json!({"medium": 1}), "V1 bySeverity");
}

/// #256: an unknown-only d35 run no longer trips `--fail-on high`; `--fail-on
/// medium` still does.
#[test]
fn v1_unknown_only_passes_fail_on_high_and_fails_fail_on_medium() {
    let (_tmp, ws) = write_workspace("v1-failon", RUNMODAL, PAGE_NO_TRIGGER);
    let (_, high) = analyze_with(&ws, Some(D35), Some("high"));
    assert_eq!(high, 0, "--fail-on high on an unknown-only run");
    let (_, medium) = analyze_with(&ws, Some(D35), Some("medium"));
    assert_eq!(medium, 1, "--fail-on medium on an unknown-only run");
}

const SYM_GUID: &str = "25625625-0000-0000-0000-000000000256";

/// #256: a subscriber whose ONLY call is to a symbol-only dependency method (no
/// body to read) and no Commit anywhere in source → the unknown arm, `medium`.
#[test]
fn symbol_only_callee_is_unknown_at_medium() {
    let app_json = format!(
        r#"{{"id":"25625625-0000-0000-0000-000000000001","name":"Ws256","publisher":"probe","version":"1.0.0.0","runtime":"13.0","dependencies":[{{"id":"{SYM_GUID}","name":"SymLib","publisher":"probe","version":"1.0.0.0"}}]}}"#
    );
    let subscriber = r#"codeunit 50101 "Sym Subscriber"
{
    [EventSubscriber(ObjectType::Table, Database::"Repro Order", 'OnBeforeDeleteEvent', '', false, false)]
    local procedure OnBeforeDeleteOrder(var Rec: Record "Repro Order"; RunTrigger: Boolean)
    var
        L: Codeunit "Sym Lib";
    begin
        L.Work();
    end;
}
"#;
    let (_tmp, ws) = write_files(
        "sym",
        &[
            ("app.json", &app_json),
            ("src/ReproOrder.Table.al", TABLE),
            ("src/SymSubscriber.Codeunit.al", subscriber),
        ],
    );
    let symbols = format!(
        r#"{{"RuntimeVersion":"13.0","Codeunits":[{{"Id":50300,"Name":"Sym Lib","Methods":[{{"Name":"Work","Parameters":[]}}]}}],"AppId":"{SYM_GUID}","Name":"SymLib","Publisher":"probe","Version":"1.0.0.0"}}"#
    );
    crate::symbol_app::write_symbol_app(
        &ws.join(".alpackages/probe_SymLib_1.0.0.0.app"),
        SYM_GUID,
        "SymLib",
        "1.0.0.0",
        &symbols,
    );
    let findings = d35_findings(&analyze(&ws, None));
    assert_eq!(
        findings.len(),
        1,
        "expected one d35 finding, got {findings:#?}"
    );
    let f = &findings[0];
    assert_eq!(
        f["title"],
        "Commit may be reachable from event subscriber (incomplete call coverage)"
    );
    assert_eq!(f["severity"], "medium", "severity");
    assert_eq!(f["confidence"]["level"], "possible", "confidence level");
    assert_transitive_id(f);
}

// CentralGauge's standalone reproduction (#256 issue comment 6374601820), verbatim.
const CG_APP_JSON: &str = r#"{
  "id": "11111111-2222-4333-8444-555555555256",
  "name": "Demo Repro 256",
  "publisher": "Demo",
  "version": "1.0.0.0",
  "runtime": "14.0",
  "platform": "28.0.0.0",
  "application": "28.0.0.0",
  "idRanges": [{ "from": 50100, "to": 50149 }],
  "dependencies": []
}
"#;

const CG_STRATEGY: &str = r#"interface "Demo Strategy"
{
    procedure NextKm(CurrentKm: Integer): Integer;
}
"#;

const CG_IMPL: &str = r#"codeunit 50101 "Demo Impl" implements "Demo Strategy"
{
    procedure NextKm(CurrentKm: Integer): Integer
    begin
        exit(CurrentKm + 5000);
    end;
}
"#;

const CG_KIND: &str = r#"enum 50102 "Demo Kind" implements "Demo Strategy"
{
    Extensible = true;

    value(0; Standard)
    {
        Implementation = "Demo Strategy" = "Demo Impl";
    }
}
"#;

const CG_ITEM: &str = r#"table 50100 "Demo Item"
{
    fields
    {
        field(1; "No."; Code[20]) { }
        field(2; Kind; Enum "Demo Kind") { }
        field(3; Km; Integer) { }
        field(4; Blocked; Boolean) { }
    }

    keys
    {
        key(PK; "No.") { Clustered = true; }
    }
}
"#;

const CG_PUBLISHER: &str = r#"codeunit 50103 "Demo Publisher"
{
    procedure Register(var DemoItem: Record "Demo Item")
    begin
        DemoItem.Km := 0;
        OnAfterRegistered(DemoItem);
    end;

    procedure IsDue(ItemNo: Code[20]): Boolean
    var
        DemoItem: Record "Demo Item";
        Strategy: Interface "Demo Strategy";
    begin
        DemoItem.Get(ItemNo);
        Strategy := DemoItem.Kind;
        exit(DemoItem.Km >= Strategy.NextKm(DemoItem.Km));
    end;

    [IntegrationEvent(false, false)]
    local procedure OnAfterRegistered(var DemoItem: Record "Demo Item")
    begin
    end;
}
"#;

/// The reproduction's subscriber: the interface call is in the publisher's `IsDue`.
const CG_SUBSCRIBER: &str = r#"codeunit 50104 "Demo Subscriber"
{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::"Demo Publisher", 'OnAfterRegistered', '', false, false)]
    local procedure ReleaseOnRegistered(var DemoItem: Record "Demo Item")
    var
        DemoPublisher: Codeunit "Demo Publisher";
    begin
        if not DemoItem.Blocked then
            exit;
        if DemoPublisher.IsDue(DemoItem."No.") then
            exit;
        DemoItem.Blocked := false;
        DemoItem.Modify(true);
    end;
}
"#;

/// The variant CentralGauge did not test: the interface call made directly here.
const CG_SUBSCRIBER_DIRECT: &str = r#"codeunit 50104 "Demo Subscriber"
{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::"Demo Publisher", 'OnAfterRegistered', '', false, false)]
    local procedure ReleaseOnRegistered(var DemoItem: Record "Demo Item")
    var
        Strategy: Interface "Demo Strategy";
    begin
        if not DemoItem.Blocked then
            exit;
        Strategy := DemoItem.Kind;
        if Strategy.NextKm(DemoItem.Km) > 0 then
            exit;
        DemoItem.Blocked := false;
        DemoItem.Modify(true);
    end;
}
"#;

/// Run CentralGauge's workspace with `subscriber` under the DEFAULT detector
/// selection; assert its one d35 finding is the unknown arm at `medium` with
/// `confidence` and `fix`, and that the run counts `{"medium": 2}` (d3 + d35).
fn assert_cg_unknown_medium(tag: &str, subscriber: &str, confidence: Value, fix: &str) {
    let (_tmp, ws) = write_files(
        tag,
        &[
            ("app.json", CG_APP_JSON),
            ("DemoStrategy.Interface.al", CG_STRATEGY),
            ("DemoImpl.Codeunit.al", CG_IMPL),
            ("DemoKind.Enum.al", CG_KIND),
            ("DemoItem.Table.al", CG_ITEM),
            ("DemoPublisher.Codeunit.al", CG_PUBLISHER),
            ("DemoSubscriber.Codeunit.al", subscriber),
        ],
    );
    let report = analyze(&ws, None);
    let findings = d35_findings(&report);
    assert_eq!(
        findings.len(),
        1,
        "{tag}: expected one d35 finding, got {findings:#?}"
    );
    let f = &findings[0];
    assert_eq!(
        f["primaryLocation"]["routineName"], "ReleaseOnRegistered",
        "{tag}: routine"
    );
    assert_eq!(
        f["title"], "Commit may be reachable from event subscriber (incomplete call coverage)",
        "{tag}: title"
    );
    assert_eq!(f["severity"], "medium", "{tag}: severity");
    assert_eq!(f["confidence"], confidence, "{tag}: confidence");
    assert_eq!(f["fixHint"]["description"], fix, "{tag}: fixHint");
    assert_transitive_id(f);
    assert_eq!(
        by_severity(&report),
        &json!({"medium": 2}),
        "{tag}: bySeverity"
    );
}

#[test]
fn centralgauge_repro_is_unknown_at_medium() {
    assert_cg_unknown_medium(
        "cg",
        CG_SUBSCRIBER,
        json!({"level": "possible", "cappedBy": ["opaque-callee"]}),
        "Check what could not be analysed (interface-open-world; partly analysed: IsDue \
         (Codeunit 50103)); if a Commit is reachable, move it out of the subscriber or defer it.",
    );
}

#[test]
fn centralgauge_repro_direct_interface_call_is_unknown_at_medium() {
    assert_cg_unknown_medium(
        "cg-direct",
        CG_SUBSCRIBER_DIRECT,
        json!({"level": "possible", "cappedBy": ["dynamic-dispatch", "opaque-callee"]}),
        "Check what could not be analysed (interface-open-world; partly analysed: \
         ReleaseOnRegistered (Codeunit 50104)); if a Commit is reachable, move it out of the \
         subscriber or defer it.",
    );
}

#[test]
fn v2_real_commit_keeps_the_yes_wording() {
    assert_yes_finding(&run_variant("v2", COMMIT, PAGE_NO_TRIGGER, None), "V2");
}

#[test]
fn v3_no_runmodal_no_finding() {
    let f = run_variant("v3", NONE, PAGE_NO_TRIGGER, None);
    assert!(f.is_empty(), "V3: expected no d35 finding, got {f:#?}");
}

/// Pins only this: a page whose source declares an entry trigger yields a
/// complete cone and no d35 finding. It does NOT show that every page trigger is
/// analysed: a page run reaches only the entry trigger, so a Commit in, say,
/// `OnQueryClosePage` is not seen (a filed discovery, ledger D3a).
#[test]
fn v4_page_with_entry_trigger_has_no_finding() {
    let f = run_variant("v4", RUNMODAL, PAGE_WITH_TRIGGER, None);
    assert!(f.is_empty(), "V4: expected no d35 finding, got {f:#?}");
}

#[test]
fn v5_yes_beats_unknown() {
    let body = format!("{COMMIT}\n{RUNMODAL}");
    assert_yes_finding(&run_variant("v5", &body, PAGE_NO_TRIGGER, None), "V5");
}

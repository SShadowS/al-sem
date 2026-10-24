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
//! Five #148 variants run the REAL gate `analyze` pipeline (`run_analyze_with_exit`,
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
//!
//! #264 (spec `docs/superpowers/specs/2026-10-24-issue-264-closed-enum-interface-dispatch-design.md`,
//! Acceptance matrix A1–A9): an interface call whose value provably comes from a
//! NOT-extensible enum is a closed dispatch. The `i264_*` tests below pin it end
//! to end on CentralGauge's reproduction and on small variants of it.

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
    analyze_in(ws, detector, fail_on, Scope::Primary, false)
}

/// As [`analyze_with`], at `scope`, and with `--single-app` when `single_app`.
fn analyze_in(
    ws: &Path,
    detector: Option<&str>,
    fail_on: Option<&str>,
    scope: Scope,
    single_app: bool,
) -> (Value, u8) {
    let args = AnalyzeArgs {
        workspace: ws.to_string_lossy().to_string(),
        min_severity: None,
        detector: detector.map(str::to_string),
        preset: None,
        scope,
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
        single_app,
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
    assert_yes_finding_for(findings, "OnBeforeDeleteOrder", label);
}

/// One d35 YES finding (a Commit found through a callee) on `subscriber`.
fn assert_yes_finding_for(findings: &[Value], subscriber: &str, label: &str) {
    assert_eq!(
        findings.len(),
        1,
        "{label}: expected exactly one d35 finding, got {findings:#?}"
    );
    let f = &findings[0];
    assert_eq!(f["title"], YES_TITLE, "{label}: title");
    let root_cause = format!(
        "{subscriber} is an event subscriber that transitively reaches Commit through its \
         callees \u{2014} the publisher cannot roll back the committed state if its work \
         later fails."
    );
    assert_eq!(f["rootCause"], root_cause, "{label}: rootCause");
    assert_eq!(
        f["confidence"],
        json!({"level": "possible"}),
        "{label}: confidence"
    );
    assert_eq!(f["severity"], "high", "{label}: severity");
    assert_transitive_id(f);
    let path = f["evidencePath"].as_array().expect("evidencePath");
    assert_eq!(path.len(), 1, "{label}: evidencePath {path:#?}");
    let note = format!(
        "[EventSubscriber] {subscriber} transitively commits (mayCommit(summary) == \"yes\")"
    );
    assert_eq!(path[0]["note"], note, "{label}: evidence note");
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
    // The precondition: the answer is unknown BECAUSE the callee is symbol-only
    // (without the symbol app the reason would be `external-target` instead).
    assert_eq!(
        f["fixHint"]["description"],
        "Check what could not be analysed (opaque-dependency; partly analysed: Work \
         (Codeunit 50300)); if a Commit is reachable, move it out of the subscriber or defer it.",
        "fixHint names the symbol-only callee"
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
        // #265 A1: the gap is the subscriber's own call site, so that site is
        // named and the subscriber is not listed as "partly analysed".
        "Check what could not be analysed (interface-open-world; unresolved: Strategy.NextKm \
         (line 11)); if a Commit is reachable, move it out of the subscriber or defer it.",
    );
}

/// #265 A3: the subscriber makes the interface call itself AND calls `IsDue`,
/// which makes one too: its own site is named, and `IsDue` (another routine)
/// stays under "partly analysed".
const CG_SUBSCRIBER_DIRECT_AND_ISDUE: &str = r#"codeunit 50104 "Demo Subscriber"
{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::"Demo Publisher", 'OnAfterRegistered', '', false, false)]
    local procedure ReleaseOnRegistered(var DemoItem: Record "Demo Item")
    var
        DemoPublisher: Codeunit "Demo Publisher";
        Strategy: Interface "Demo Strategy";
    begin
        if not DemoItem.Blocked then
            exit;
        Strategy := DemoItem.Kind;
        if Strategy.NextKm(DemoItem.Km) > 0 then
            exit;
        if DemoPublisher.IsDue(DemoItem."No.") then
            exit;
        DemoItem.Blocked := false;
        DemoItem.Modify(true);
    end;
}
"#;

#[test]
fn centralgauge_direct_and_isdue_names_site_and_isdue() {
    let (_tmp, ws) = cg_files("i265-a3", CG_KIND, CG_SUBSCRIBER_DIRECT_AND_ISDUE, &[]);
    let reasons = "interface-open-world; unresolved: Strategy.NextKm (line 12); partly analysed: \
                   IsDue (Codeunit 50103)";
    for single_app in [false, true] {
        let label = if single_app {
            "single-app"
        } else {
            "cross-app"
        };
        let findings = d35_findings(&analyze_in(&ws, None, None, Scope::Primary, single_app).0);
        assert_eq!(
            findings.len(),
            1,
            "{label}: expected one d35 finding, got {findings:#?}"
        );
        let f = &findings[0];
        assert_eq!(
            f["primaryLocation"]["routineName"], "ReleaseOnRegistered",
            "{label}: routine"
        );
        assert_eq!(
            f["rootCause"],
            format!(
                "ReleaseOnRegistered is an event subscriber whose call coverage is incomplete \
                 ({reasons}); a Commit cannot be ruled out \u{2014} the publisher could not roll \
                 back committed state if one exists."
            ),
            "{label}: rootCause"
        );
        assert_eq!(
            f["fixHint"]["description"],
            format!(
                "Check what could not be analysed ({reasons}); if a Commit is reachable, move \
                 it out of the subscriber or defer it."
            ),
            "{label}: fixHint"
        );
    }
}

/// #265 CR3: the subscriber's interface call spans two lines with a comment
/// inside the callee; the site prints its structured callee, not that source.
const CG_SUBSCRIBER_DIRECT_SPLIT: &str = r#"codeunit 50104 "Demo Subscriber"
{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::"Demo Publisher", 'OnAfterRegistered', '', false, false)]
    local procedure ReleaseOnRegistered(var DemoItem: Record "Demo Item")
    var
        Strategy: Interface "Demo Strategy";
    begin
        if not DemoItem.Blocked then
            exit;
        Strategy := DemoItem.Kind;
        if Strategy // the kind's strategy
            .NextKm(DemoItem.Km) > 0 then
            exit;
        DemoItem.Blocked := false;
        DemoItem.Modify(true);
    end;
}
"#;

#[test]
fn own_site_spanning_lines_prints_the_structured_callee() {
    let (_tmp, ws) = cg_files("i265-cr3", CG_KIND, CG_SUBSCRIBER_DIRECT_SPLIT, &[]);
    let findings = d35_findings(&analyze(&ws, None));
    assert_eq!(
        findings.len(),
        1,
        "expected one d35 finding, got {findings:#?}"
    );
    assert_eq!(
        findings[0]["fixHint"]["description"],
        "Check what could not be analysed (interface-open-world; unresolved: Strategy.NextKm \
         (line 11)); if a Commit is reachable, move it out of the subscriber or defer it."
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

// ---------------------------------------------------------------------------
// #264 — interface dispatch closed over a non-extensible enum.
// ---------------------------------------------------------------------------

use al_sem::engine::l4::combined_graph::build_combined_graph;
use al_sem::engine::l5::snapshot::compose_snapshot;
use al_sem::program::model::taxonomy::DispatchKind;
use al_sem::program::model::workspace::Model;
use al_sem::program::resolve::edge::{DispatchShape, SetCompleteness};

/// CentralGauge's enum with its `Extensible = true;` line replaced by
/// `extensible` (`""` drops the property: absent means not extensible, P2a).
fn cg_kind(extensible: &str) -> String {
    let line = "    Extensible = true;\n";
    assert_eq!(
        CG_KIND.matches(line).count(),
        1,
        "precondition: one Extensible line"
    );
    CG_KIND.replace(line, extensible)
}

const CLOSED: &str = "    Extensible = false;\n";

/// CentralGauge's workspace with `kind` as the enum and `subscriber`, plus
/// `extra` files.
fn cg_files(
    tag: &str,
    kind: &str,
    subscriber: &str,
    extra: &[(&str, &str)],
) -> (tempfile::TempDir, PathBuf) {
    let mut files = vec![
        ("app.json", CG_APP_JSON),
        ("DemoStrategy.Interface.al", CG_STRATEGY),
        ("DemoImpl.Codeunit.al", CG_IMPL),
        ("DemoKind.Enum.al", kind),
        ("DemoItem.Table.al", CG_ITEM),
        ("DemoPublisher.Codeunit.al", CG_PUBLISHER),
        ("DemoSubscriber.Codeunit.al", subscriber),
    ];
    files.extend_from_slice(extra);
    write_files(tag, &files)
}

fn read_inputs(ws: &Path) -> std::sync::Arc<al_sem::program::inputs::InputSet> {
    al_sem::program::inputs::InputSet::read(ws, &crate::fingerprint_needs())
}

/// The single-app model (`alsem analyze --single-app`'s; with no dependency it
/// equals the cross-app one).
fn single_app_model(ws: &Path) -> Model {
    let id = al_sem::engine::gate::model_instance_id::compute_gate_model_instance_id(ws)
        .expect("model id");
    al_sem::program::model::program_calls::assemble_and_resolve_workspace_program(
        &read_inputs(ws),
        &id,
        false,
    )
    .expect("single-app model")
}

/// The default (cross-app) model `alsem analyze` builds.
fn cross_app_model(ws: &Path) -> Model {
    let id = al_sem::engine::gate::model_instance_id::compute_gate_model_instance_id(ws)
        .expect("model id");
    al_sem::program::model::program_calls::assemble_and_resolve_cross_app_program(
        &read_inputs(ws),
        &id,
        false,
    )
    .unwrap_or_else(|_| panic!("cross-app model"))
    .resolved
}

/// `(internal id, stable id)` of the one routine `object.routine`.
fn routine_ids(m: &Model, object: &str, routine: &str) -> (String, String) {
    let ws = &m.workspace;
    let hits: Vec<(String, String)> = ws
        .routines
        .iter()
        .filter(|r| {
            r.name == routine
                && ws
                    .objects
                    .iter()
                    .any(|o| o.id == r.object_id && o.name == object)
        })
        .map(|r| (r.id.clone(), r.stable_routine_id.clone()))
        .collect();
    assert_eq!(
        hits.len(),
        1,
        "one routine {object}.{routine}, got {hits:?}"
    );
    hits.into_iter().next().unwrap()
}

/// What every layer says about the ONE interface call site in a routine.
#[derive(Debug)]
struct Site {
    /// The snapshot row's `openWorld`.
    open_world: Option<bool>,
    /// The snapshot row's `candidates` (stable routine ids).
    candidates: Option<Vec<String>>,
    /// `Debug` of the meta-carrying model edge's `DispatchMeta`.
    meta: String,
    /// That meta's `enum_implementers` (model ids of the enums implementing I).
    enum_implementers: Vec<String>,
    /// A model edge of the site has no target (an implementer outside the model).
    has_to_less_edge: bool,
    /// The combined graph holds an `interface-open-world` uncertainty from the routine.
    open_world_uncertainty: bool,
}

fn interface_site(m: &Model, object: &str, routine: &str) -> Site {
    let (rid, stable) = routine_ids(m, object, routine);
    let edges: Vec<_> = m
        .calls
        .edges
        .iter()
        .filter(|e| e.from == rid && e.dispatch_kind == DispatchKind::Interface)
        .collect();
    let sites: std::collections::BTreeSet<&str> =
        edges.iter().map(|e| e.callsite_id.as_str()).collect();
    assert_eq!(
        sites.len(),
        1,
        "{object}.{routine}: precondition, exactly one interface call site, got {edges:#?}"
    );
    let dm = edges
        .iter()
        .find_map(|e| e.dispatch_meta.as_ref())
        .unwrap_or_else(|| panic!("{object}.{routine}: no DispatchMeta on {edges:#?}"));
    let rows: Vec<_> = compose_snapshot(m)
        .callsite_resolutions
        .into_iter()
        .filter(|r| r.from == stable && r.status == "polymorphic")
        .collect();
    assert_eq!(rows.len(), 1, "{object}.{routine}: one polymorphic row");
    let graph = build_combined_graph(&m.workspace, &m.calls, &m.events.graph);
    let open_world_uncertainty = graph
        .uncertainty_edges
        .iter()
        .any(|u| u.from == rid && u.uncertainty.kind == "interface-open-world");
    Site {
        open_world: rows[0].open_world,
        candidates: rows[0].candidates.clone(),
        meta: format!("{dm:?}"),
        enum_implementers: dm.enum_implementers.clone(),
        has_to_less_edge: edges.iter().any(|e| e.to.is_none()),
        open_world_uncertainty,
    }
}

/// A1: candidates present, `openWorld: false`, `closed_over` = the enum, no
/// `interface-open-world` uncertainty. `None` when it holds, else the reason.
fn closed_failure(site: &Site, want_candidates: &[String]) -> Option<String> {
    let mut why = Vec::new();
    if site.candidates.as_deref() != Some(want_candidates) {
        why.push(format!(
            "candidates {:?} != {want_candidates:?}",
            site.candidates
        ));
    }
    if site.open_world != Some(false) {
        why.push(format!("openWorld {:?}, want Some(false)", site.open_world));
    }
    let want_meta = format!("closed_over: {:?}", site.enum_implementers);
    if site.enum_implementers.is_empty() || !site.meta.contains(&want_meta) {
        why.push(format!("DispatchMeta lacks `{want_meta}`: {}", site.meta));
    }
    if site.open_world_uncertainty {
        why.push("an interface-open-world uncertainty is emitted".to_string());
    }
    (!why.is_empty()).then(|| why.join("; "))
}

fn assert_closed(site: &Site, want_candidates: &[String], label: &str) {
    if let Some(why) = closed_failure(site, want_candidates) {
        panic!("{label}: site should be CLOSED: {why}");
    }
}

/// The site is open in every layer: `openWorld: true`, no `closed_over`, the
/// `interface-open-world` uncertainty present.
fn assert_open(site: &Site, label: &str) {
    assert_eq!(site.open_world, Some(true), "{label}: openWorld, {site:#?}");
    assert!(
        !site.meta.contains("closed_over: [\""),
        "{label}: closed_over must be empty, {site:#?}"
    );
    assert!(
        site.open_world_uncertainty,
        "{label}: the interface-open-world uncertainty must stay, {site:#?}"
    );
}

/// The stable id of `Demo Impl.NextKm`, the repro's one implementation.
fn impl_next_km(m: &Model) -> Vec<String> {
    vec![routine_ids(m, "Demo Impl", "NextKm").1]
}

/// The program edge (resolver level) of the one interface site in `routine_lc`.
fn interface_completeness(ws: &Path, routine_lc: &str) -> SetCompleteness {
    let report =
        al_sem::program::resolve::full::resolve_full_program(ws).expect("program resolves");
    let hits: Vec<_> = report
        .edges
        .iter()
        .filter(|c| {
            c.edge.from.name_lc == *routine_lc && c.edge.shape == DispatchShape::Polymorphic
        })
        .collect();
    assert_eq!(hits.len(), 1, "one Polymorphic edge in {routine_lc}");
    hits[0].edge.completeness
}

/// T1 (A1, A2): CentralGauge's repro with `Extensible = false`, the call in the
/// publisher's `IsDue`: no d35 finding, and the site is closed in every layer.
#[test]
fn i264_t1_closed_enum_repro_has_no_d35_and_a_closed_site() {
    let (_tmp, ws) = cg_files("i264-t1", &cg_kind(CLOSED), CG_SUBSCRIBER, &[]);
    assert_closed_repro(&ws, "Demo Publisher", "IsDue", Vec::new(), "T1");
}

/// The closed repro's outcome: no d35 finding (default selection) and a closed
/// site, plus any `failures` the caller already found; all reported at once.
fn assert_closed_repro(
    ws: &Path,
    object: &str,
    routine: &str,
    mut failures: Vec<String>,
    label: &str,
) {
    let findings = d35_findings(&analyze(ws, None));
    if !findings.is_empty() {
        let titles: Vec<String> = findings
            .iter()
            .map(|f| format!("{} [{}]", f["title"], f["fixHint"]["description"]))
            .collect();
        failures.push(format!("expected no d35 finding, got {titles:?}"));
    }
    let m = single_app_model(ws);
    if let Some(why) = closed_failure(&interface_site(&m, object, routine), &impl_next_km(&m)) {
        failures.push(format!("site should be CLOSED: {why}"));
    }
    assert!(failures.is_empty(), "{label}:\n{}", failures.join("\n"));
}

/// T2 (A1, A2): the interface call made directly in the subscriber: no d35
/// finding, a closed site, and the resolver edge `Complete` (pinned on its own).
#[test]
fn i264_t2_closed_enum_direct_call_is_complete_with_no_d35() {
    let (_tmp, ws) = cg_files("i264-t2", &cg_kind(CLOSED), CG_SUBSCRIBER_DIRECT, &[]);
    let completeness = interface_completeness(&ws, "releaseonregistered");
    let failures = if completeness == SetCompleteness::Complete {
        Vec::new()
    } else {
        vec![format!(
            "the NextKm edge must be Complete, is {completeness:?}"
        )]
    };
    assert_closed_repro(
        &ws,
        "Demo Subscriber",
        "ReleaseOnRegistered",
        failures,
        "T2",
    );
}

/// T4 (A5): no `Extensible` property at all means not extensible (alc AL0504).
#[test]
fn i264_t4_absent_extensible_is_closed() {
    let (_tmp, ws) = cg_files("i264-t4", &cg_kind(""), CG_SUBSCRIBER, &[]);
    assert_closed_repro(&ws, "Demo Publisher", "IsDue", Vec::new(), "T4");
}

/// A3 control: `Extensible = true` stays open in every layer (the d35 side is
/// `centralgauge_repro_is_unknown_at_medium`, unchanged).
#[test]
fn i264_a3_extensible_true_stays_open() {
    let (_tmp, ws) = cg_files("i264-a3", CG_KIND, CG_SUBSCRIBER_DIRECT, &[]);
    let m = single_app_model(&ws);
    assert_open(
        &interface_site(&m, "Demo Subscriber", "ReleaseOnRegistered"),
        "A3",
    );
    assert!(
        matches!(
            interface_completeness(&ws, "releaseonregistered"),
            SetCompleteness::Partial { .. }
        ),
        "A3: the edge stays Partial"
    );
}

/// T3 (A8) and A4 (T5–T8, T6b) share one workspace over the CLOSED enum: one
/// interface call site per routine.
const SHAPES: &str = r#"codeunit 50110 "Shape Probe"
{
    var
        G: Interface "Demo Strategy";

    // ---- T3: every right-hand side below has a closed enum identity.
    procedure FromLocal()
    var
        K: Enum "Demo Kind";
        S: Interface "Demo Strategy";
    begin
        S := K;
        S.NextKm(1);
    end;

    procedure FromParam(K: Enum "Demo Kind")
    var
        S: Interface "Demo Strategy";
    begin
        S := K;
        S.NextKm(1);
    end;

    procedure FromField()
    var
        Rec: Record "Demo Item";
        S: Interface "Demo Strategy";
    begin
        Rec.Get('X');
        S := Rec.Kind;
        S.NextKm(1);
    end;

    procedure FromQuotedValue()
    var
        S: Interface "Demo Strategy";
    begin
        S := "Demo Kind"::Standard;
        S.NextKm(1);
    end;

    procedure FromEnumValue()
    var
        S: Interface "Demo Strategy";
    begin
        S := Enum::"Demo Kind"::Standard;
        S.NextKm(1);
    end;

    procedure FromInteger()
    var
        S: Interface "Demo Strategy";
    begin
        S := Enum::"Demo Kind".FromInteger(0);
        S.NextKm(1);
    end;

    procedure FromOwnCall()
    var
        S: Interface "Demo Strategy";
    begin
        S := PickKind();
        S.NextKm(1);
    end;

    local procedure PickKind(): Enum "Demo Kind"
    begin
        exit(Enum::"Demo Kind"::Standard);
    end;

    // ---- A4: each stays open.
    procedure CodeunitAfterField()
    var
        Rec: Record "Demo Item";
        Impl: Codeunit "Demo Impl";
        S: Interface "Demo Strategy";
    begin
        Rec.Get('X');
        S := Rec.Kind;
        S := Impl;
        S.NextKm(1);
    end;

    procedure ParamOnly(S: Interface "Demo Strategy")
    begin
        S.NextKm(1);
    end;

    procedure ParamAssigned(S: Interface "Demo Strategy")
    var
        Rec: Record "Demo Item";
    begin
        Rec.Get('X');
        S := Rec.Kind;
        S.NextKm(1);
    end;

    procedure GlobalAssigned()
    var
        Rec: Record "Demo Item";
    begin
        Rec.Get('X');
        G := Rec.Kind;
        G.NextKm(1);
    end;

    procedure PassedAsArgument()
    var
        Rec: Record "Demo Item";
        S: Interface "Demo Strategy";
    begin
        Rec.Get('X');
        S := Rec.Kind;
        Use(S);
        S.NextKm(1);
    end;

    local procedure Use(X: Interface "Demo Strategy")
    begin
    end;
}
"#;

fn shapes_model(tag: &str) -> (tempfile::TempDir, Model) {
    let (tmp, ws) = cg_files(
        tag,
        &cg_kind(CLOSED),
        CG_SUBSCRIBER,
        &[("ShapeProbe.Codeunit.al", SHAPES)],
    );
    let m = single_app_model(&ws);
    (tmp, m)
}

/// T3 (A8): each right-hand-side shape of the spec's §3 rule 4 closes the site.
#[test]
fn i264_t3_rhs_shapes_are_closed() {
    let (_tmp, m) = shapes_model("i264-t3");
    let want = impl_next_km(&m);
    let failures: Vec<String> = [
        "FromLocal",
        "FromParam",
        "FromField",
        "FromQuotedValue",
        "FromEnumValue",
        "FromInteger",
        "FromOwnCall",
    ]
    .iter()
    .filter_map(|r| {
        closed_failure(&interface_site(&m, "Shape Probe", r), &want).map(|w| format!("{r}: {w}"))
    })
    .collect();
    assert!(
        failures.is_empty(),
        "T3: shapes that should be CLOSED:\n{}",
        failures.join("\n")
    );
}

/// A4 (T5, T6, T6b, T7, T8): a later codeunit write, an interface parameter
/// (unassigned or assigned from the closed enum), a global, and an argument
/// occurrence each keep the site open, although the enum is closed.
#[test]
fn i264_a4_parameter_codeunit_global_and_argument_receivers_stay_open() {
    let (_tmp, m) = shapes_model("i264-a4");
    for (routine, t) in [
        ("CodeunitAfterField", "T5"),
        ("ParamOnly", "T6"),
        ("ParamAssigned", "T6b"),
        ("GlobalAssigned", "T7"),
        ("PassedAsArgument", "T8"),
    ] {
        assert_open(
            &interface_site(&m, "Shape Probe", routine),
            &format!("{t} {routine}"),
        );
    }
}

/// T22 (A9): `prove` and `digest` see no blocker for the closed site: prove's
/// obligations count neither an unresolved call site nor an open-world row,
/// and digest's `IsDue` entry lists no unresolved `NextKm` row.
#[test]
fn i264_t22_prove_and_digest_see_no_blocker_for_a_closed_site() {
    let (_tmp, ws) = cg_files("i264-t22", &cg_kind(CLOSED), CG_SUBSCRIBER, &[]);
    let prove = al_sem::engine::l5::prove::run_prove_pipeline(
        &read_inputs(&ws),
        "IsDue",
        "may-commit",
        "t",
        true,
    )
    .expect("prove runs");
    let prove: Value = serde_json::from_str(&prove.json_text).expect("prove json");
    let result = &prove["payload"]["result"];
    let mut failures = Vec::new();
    let counts = (
        &result["obligations"]["unresolvedCallsites"],
        &result["obligations"]["openWorldRows"],
    );
    if counts != (&json!(0), &json!(0)) {
        failures.push(format!(
            "prove obligations (unresolvedCallsites, openWorldRows) = {counts:?}, want (0, 0)"
        ));
    }
    let blockers: Vec<&Value> = result["blockedBy"]
        .as_array()
        .map(|b| b.iter().collect())
        .unwrap_or_default();
    let next_km: Vec<&&Value> = blockers
        .iter()
        .filter(|b| b["detail"].as_str().unwrap_or("").contains("NextKm"))
        .collect();
    if !next_km.is_empty() {
        failures.push(format!("prove blockers on the closed site: {next_km:?}"));
    }
    let digest = al_sem::engine::l5::digest_cli::run_digest_pipeline(
        &read_inputs(&ws),
        Some(vec!["DemoPublisher.Codeunit.al".to_string()]),
        None,
        None,
        "t",
        true,
        None,
        false,
        false,
    )
    .expect("digest runs");
    let digest: Value = serde_json::from_str(&digest.json_text).expect("digest json");
    let text = digest.to_string();
    assert!(
        text.contains("IsDue"),
        "T22 precondition: digest covers IsDue, got {digest:#}"
    );
    let rows: Vec<Value> = digest["payload"]["entries"]
        .as_array()
        .expect("payload.entries")
        .iter()
        .flat_map(|e| e["unresolved"].as_array().cloned().unwrap_or_default())
        .filter(|r| r["calleeDisplay"].as_str().unwrap_or("").contains("NextKm"))
        .collect();
    if !rows.is_empty() {
        failures.push(format!("digest lists unresolved NextKm rows: {rows:?}"));
    }
    assert!(failures.is_empty(), "T22:\n{}", failures.join("\n"));
}

const DEP264_GUID: &str = "26426426-0000-0000-0000-0000000000de";

fn dep264_app_json() -> String {
    format!(
        r#"{{"id":"26426426-0000-0000-0000-000000000001","name":"Ws264","publisher":"probe","version":"1.0.0.0","runtime":"13.0","idRanges":[{{"from":50100,"to":50199}}],"dependencies":[{{"id":"{DEP264_GUID}","name":"Dep264","publisher":"probe","version":"1.0.0.0"}}]}}"#
    )
}

/// A dependency that ships SOURCE (`sources`), at the workspace's `.alpackages`.
fn write_dep264(ws: &Path, symbols_body: &str, sources: &[(&str, &str)]) {
    let symbols = format!(
        r#"{{"RuntimeVersion":"13.0",{symbols_body},"AppId":"{DEP264_GUID}","Name":"Dep264","Publisher":"probe","Version":"1.0.0.0"}}"#
    );
    crate::symbol_app::write_source_app(
        &ws.join(".alpackages/probe_Dep264_1.0.0.0.app"),
        DEP264_GUID,
        "Dep264",
        "1.0.0.0",
        &symbols,
        sources,
        "",
    );
}

const DEP_STRATEGY: &str = r#"interface "Dep Strategy"
{
    procedure NextKm(CurrentKm: Integer): Integer;
}
"#;

const DEP_IMPL: &str = r#"codeunit 70001 "Dep Impl" implements "Dep Strategy"
{
    procedure NextKm(CurrentKm: Integer): Integer
    begin
        exit(CurrentKm + 1);
    end;
}
"#;

const DEP_STRATEGY_SYMBOLS: &str = r#""Interfaces":[{"Name":"Dep Strategy","Methods":[{"Name":"NextKm","Parameters":[{"Name":"CurrentKm","TypeDefinition":{"Name":"Integer"}}],"ReturnTypeDefinition":{"Name":"Integer"}}]}],"Codeunits":[{"Id":70001,"Name":"Dep Impl","ImplementedInterfaces":["Dep Strategy"],"Methods":[{"Name":"NextKm","Parameters":[{"Name":"CurrentKm","TypeDefinition":{"Name":"Integer"}}],"ReturnTypeDefinition":{"Name":"Integer"}}]}]"#;

/// T27's workspace: a CLOSED workspace enum over the dependency's interface,
/// one value mapped to the dependency's `Dep Impl`, one to the workspace's own.
const T27_WS: &str = r#"codeunit 50101 "Ws Impl" implements "Dep Strategy"
{
    procedure NextKm(CurrentKm: Integer): Integer
    begin
        exit(CurrentKm + 2);
    end;
}

enum 50102 "Ws Kind" implements "Dep Strategy"
{
    Extensible = false;

    value(0; FromDep)
    {
        Implementation = "Dep Strategy" = "Dep Impl";
    }
    value(1; FromWs)
    {
        Implementation = "Dep Strategy" = "Ws Impl";
    }
}

table 50100 "Ws Item"
{
    fields
    {
        field(1; "No."; Code[20]) { }
        field(2; Kind; Enum "Ws Kind") { }
    }
    keys
    {
        key(PK; "No.") { Clustered = true; }
    }
}

codeunit 50103 "Ws Caller"
{
    procedure Dispatch()
    var
        Rec: Record "Ws Item";
        S: Interface "Dep Strategy";
    begin
        Rec.Get('X');
        S := Rec.Kind;
        S.NextKm(1);
    end;
}
"#;

/// T27 (spec §5): with an implementer OUTSIDE the model (`--single-app`, the
/// dependency body is not loaded) the site stays open, `openWorld: true`. In the
/// default cross-app model the dependency body is demanded, every edge has a
/// target, and the same site closes (spec P4a / §5).
#[test]
fn i264_t27_dependency_implementer_outside_the_model_stays_open() {
    let (_tmp, ws) = write_files(
        "i264-t27",
        &[("app.json", &dep264_app_json()), ("src/Ws.al", T27_WS)],
    );
    write_dep264(
        &ws,
        DEP_STRATEGY_SYMBOLS,
        &[
            ("src/DepStrategy.al", DEP_STRATEGY),
            ("src/DepImpl.al", DEP_IMPL),
        ],
    );
    let single = single_app_model(&ws);
    let site = interface_site(&single, "Ws Caller", "Dispatch");
    assert!(
        site.has_to_less_edge,
        "T27 precondition: Dep Impl is outside the single-app model, {site:#?}"
    );
    assert_open(&site, "T27 single-app");

    let cross = cross_app_model(&ws);
    let mut want: Vec<String> = vec![
        routine_ids(&cross, "Dep Impl", "NextKm").1,
        routine_ids(&cross, "Ws Impl", "NextKm").1,
    ];
    want.sort();
    assert_closed(
        &interface_site(&cross, "Ws Caller", "Dispatch"),
        &want,
        "T27 cross-app",
    );
}

/// T9 (Discovery D1): an enum known only from a dependency's SYMBOLS (no
/// source, `Extensible` absent, which would mean closed in source) stays open:
/// symbol extensibility and value mappings are not modelled. Baseline control;
/// its discrimination proof (`closed_enum: true` in `abi_ingest`) is recorded
/// with Task 5, the first task that reads the fact.
#[test]
fn i264_t9_symbol_only_enum_stays_open() {
    const WS: &str = r#"table 50100 "Ws Item"
{
    fields
    {
        field(1; "No."; Code[20]) { }
        field(2; Kind; Enum "Dep Kind") { }
    }
    keys
    {
        key(PK; "No.") { Clustered = true; }
    }
}

codeunit 50103 "Ws Caller"
{
    procedure Dispatch()
    var
        Rec: Record "Ws Item";
        S: Interface "Dep Strategy";
    begin
        Rec.Get('X');
        S := Rec.Kind;
        S.NextKm(1);
    end;
}
"#;
    let (_tmp, ws) = write_files(
        "i264-t9",
        &[("app.json", &dep264_app_json()), ("src/Ws.al", WS)],
    );
    let symbols = format!(
        r#"{DEP_STRATEGY_SYMBOLS},"EnumTypes":[{{"Id":70002,"Name":"Dep Kind","ImplementedInterfaces":["Dep Strategy"]}}]"#
    );
    write_dep264(&ws, &symbols, &[]);

    let ctx = al_sem::program::resolve::full::build_context_res(&ws).expect("context");
    let kinds: Vec<_> = ctx
        .graph()
        .objects
        .iter()
        .filter(|o| o.name.as_ref() == "Dep Kind")
        .collect();
    assert_eq!(
        kinds.len(),
        1,
        "T9 precondition: the symbol enum is ingested"
    );
    assert!(
        !kinds[0].closed_enum && kinds[0].enum_impls.is_empty(),
        "{kinds:?}"
    );

    assert_open(
        &interface_site(&cross_app_model(&ws), "Ws Caller", "Dispatch"),
        "T9",
    );
}

/// T21 (A8, identity): a dependency table's field is typed with the
/// DEPENDENCY's extensible `Demo Kind`; the workspace declares its own CLOSED
/// `Demo Kind`. The value's enum is the dependency's, so the site stays open.
/// Defensive, stated precondition: alc rejects this closure (AL0197, enum
/// name declared by two extensions, probe P7); al-sem must still never close
/// it. The resolver-level twin with its discrimination proof is
/// `closed_enum::tests::i264_t21_field_enum_resolves_in_the_declaring_app`.
#[test]
fn i264_t21_same_name_enum_resolves_in_the_declaring_app() {
    const DEP_KIND_AND_TABLE: &str = r#"enum 70002 "Demo Kind" implements "Dep Strategy"
{
    Extensible = true;

    value(0; Standard)
    {
        Implementation = "Dep Strategy" = "Dep Impl";
    }
}

table 70003 "Dep Item"
{
    fields
    {
        field(1; "No."; Code[20]) { }
        field(2; Kind; Enum "Demo Kind") { }
    }
    keys
    {
        key(PK; "No.") { Clustered = true; }
    }
}
"#;
    const WS: &str = r#"enum 50102 "Demo Kind" implements "Dep Strategy"
{
    Extensible = false;

    value(0; Standard)
    {
        Implementation = "Dep Strategy" = "Dep Impl";
    }
}

codeunit 50103 "Ws Caller"
{
    procedure Dispatch()
    var
        Rec: Record "Dep Item";
        S: Interface "Dep Strategy";
    begin
        Rec.Get('X');
        S := Rec.Kind;
        S.NextKm(1);
    end;
}
"#;
    let (_tmp, ws) = write_files(
        "i264-t21",
        &[("app.json", &dep264_app_json()), ("src/Ws.al", WS)],
    );
    let symbols = format!(
        r#"{DEP_STRATEGY_SYMBOLS},"EnumTypes":[{{"Id":70002,"Name":"Demo Kind","ImplementedInterfaces":["Dep Strategy"],"Properties":[{{"Name":"Extensible","Value":"1"}}]}}],"Tables":[{{"Id":70003,"Name":"Dep Item","Fields":[{{"Id":1,"Name":"No.","TypeDefinition":{{"Name":"Code"}}}},{{"Id":2,"Name":"Kind","TypeDefinition":{{"Name":"Enum","Subtype":{{"Name":"Demo Kind"}}}}}}]}}]"#
    );
    write_dep264(
        &ws,
        &symbols,
        &[
            ("src/DepStrategy.al", DEP_STRATEGY),
            ("src/DepImpl.al", DEP_IMPL),
            ("src/DepKind.al", DEP_KIND_AND_TABLE),
        ],
    );
    let m = cross_app_model(&ws);
    assert_open(&interface_site(&m, "Ws Caller", "Dispatch"), "T21");
}

/// A codeunit `name` implementing `Demo Strategy`; `NextKm` commits when
/// `commits`.
fn strategy_impl(id: u32, name: &str, commits: bool) -> String {
    let commit = if commits { "        Commit();\n" } else { "" };
    format!(
        "codeunit {id} \"{name}\" implements \"Demo Strategy\"\n{{\n    procedure NextKm(CurrentKm: Integer): Integer\n    begin\n{commit}        exit(CurrentKm + 1);\n    end;\n}}\n"
    )
}

/// The closed `Demo Kind` with a second value `Fast` mapped to `Fast Impl`.
const KIND_TWO_VALUES: &str = r#"enum 50102 "Demo Kind" implements "Demo Strategy"
{
    Extensible = false;

    value(0; Standard)
    {
        Implementation = "Demo Strategy" = "Demo Impl";
    }
    value(1; Fast)
    {
        Implementation = "Demo Strategy" = "Fast Impl";
    }
}
"#;

/// T12: two values map two codeunits; the site is closed over both, and no
/// edge of it (the meta-carrying one or the other) emits the open world.
#[test]
fn i264_t12_two_candidates_emit_no_open_world() {
    let fast = strategy_impl(50105, "Fast Impl", false);
    let (_tmp, ws) = cg_files(
        "i264-t12",
        KIND_TWO_VALUES,
        CG_SUBSCRIBER,
        &[("FastImpl.Codeunit.al", &fast)],
    );
    let m = single_app_model(&ws);
    let mut want = vec![
        routine_ids(&m, "Demo Impl", "NextKm").1,
        routine_ids(&m, "Fast Impl", "NextKm").1,
    ];
    want.sort();
    assert_closed(&interface_site(&m, "Demo Publisher", "IsDue"), &want, "T12");
}

/// T13: the site is closed and its one candidate commits: d35 still reports
/// the Commit, as a YES finding through the callee.
#[test]
fn i264_t13_committing_candidate_still_reports_yes() {
    let committing = strategy_impl(50101, "Demo Impl", true);
    let (_tmp, ws) = cg_files(
        "i264-t13",
        &cg_kind(CLOSED),
        CG_SUBSCRIBER,
        &[("DemoImpl.Codeunit.al", &committing)],
    );
    assert_yes_finding_for(
        &d35_findings(&analyze(&ws, None)),
        "ReleaseOnRegistered",
        "T13",
    );
}

/// T14 (stated limit, spec non-goal): another codeunit implements the
/// interface, is NOT mapped by the enum, and commits. The routes still hold
/// every implementer, so d35 reports YES although no enum value reaches it.
#[test]
fn i264_t14_unmapped_committing_implementer_reports_yes() {
    let extra = strategy_impl(50106, "Extra Impl", true);
    let (_tmp, ws) = cg_files(
        "i264-t14",
        &cg_kind(CLOSED),
        CG_SUBSCRIBER,
        &[("ExtraImpl.Codeunit.al", &extra)],
    );
    assert_yes_finding_for(
        &d35_findings(&analyze(&ws, None)),
        "ReleaseOnRegistered",
        "T14",
    );
}

/// T29 (model half): writes from two closed enums close the site over both,
/// on the default (cross-app) model.
#[test]
fn i264_t29_two_closed_enums_list_both_in_closed_over() {
    const SECOND: &str = r#"enum 50107 "Second Kind" implements "Demo Strategy"
{
    Extensible = false;

    value(0; Standard)
    {
        Implementation = "Demo Strategy" = "Second Impl";
    }
}

table 50108 "Second Item"
{
    fields
    {
        field(1; "No."; Code[20]) { }
        field(2; Kind; Enum "Second Kind") { }
    }
    keys
    {
        key(PK; "No.") { Clustered = true; }
    }
}

codeunit 50109 "Two Kinds"
{
    procedure Dispatch()
    var
        A: Record "Demo Item";
        B: Record "Second Item";
        S: Interface "Demo Strategy";
    begin
        A.Get('X');
        B.Get('X');
        S := A.Kind;
        S := B.Kind;
        S.NextKm(1);
    end;
}
"#;
    let second_impl = strategy_impl(50110, "Second Impl", false);
    let (_tmp, ws) = cg_files(
        "i264-t29",
        &cg_kind(CLOSED),
        CG_SUBSCRIBER,
        &[
            ("Second.al", SECOND),
            ("SecondImpl.Codeunit.al", &second_impl),
        ],
    );
    let m = cross_app_model(&ws);
    let site = interface_site(&m, "Two Kinds", "Dispatch");
    assert_eq!(
        site.enum_implementers.len(),
        2,
        "precondition: two enums, {site:#?}"
    );
    let mut want = vec![
        routine_ids(&m, "Demo Impl", "NextKm").1,
        routine_ids(&m, "Second Impl", "NextKm").1,
    ];
    want.sort();
    assert_closed(&site, &want, "T29");
}

/// An extensible enum over `Demo Strategy`, mapped to the same `Demo Impl`.
const OPEN_KIND: &str = r#"enum 50105 "Open Kind" implements "Demo Strategy"
{
    Extensible = true;
    value(0; Other) { Implementation = "Demo Strategy" = "Demo Impl"; }
}
"#;

/// The subscriber declared twice under `#if CLEAN99 … #else … #endif`: one arm
/// writes the interface from the closed `Demo Kind`, the other from the
/// extensible `Open Kind`. Both arms share one routine id and one call-site id.
fn two_arm_subscriber(first: &str, second: &str) -> String {
    let arm = |value: &str| {
        format!(
            "    [EventSubscriber(ObjectType::Codeunit, Codeunit::\"Demo Publisher\", 'OnAfterRegistered', '', false, false)]\n    local procedure ReleaseOnRegistered(var DemoItem: Record \"Demo Item\")\n    var\n        Strategy: Interface \"Demo Strategy\";\n    begin\n        Strategy := {value};\n        if Strategy.NextKm(DemoItem.Km) > 0 then\n            exit;\n        DemoItem.Modify(true);\n    end;\n"
        )
    };
    format!(
        "codeunit 50104 \"Demo Subscriber\"\n{{\n#if CLEAN99\n{}#else\n{}#endif\n}}\n",
        arm(first),
        arm(second)
    )
}

/// Final panel FO1: whatever the arm order, one open arm keeps the shared site
/// open: d35 still reports the unknown Commit, and no layer calls it closed.
#[test]
fn i264_fo1_if_alternative_routines_close_only_when_every_arm_is_closed() {
    const CLOSED_VALUE: &str = "Enum::\"Demo Kind\"::Standard";
    const OPEN_VALUE: &str = "Enum::\"Open Kind\"::Other";
    for (label, first, second) in [
        ("open arm first", OPEN_VALUE, CLOSED_VALUE),
        ("closed arm first", CLOSED_VALUE, OPEN_VALUE),
    ] {
        let subscriber = two_arm_subscriber(first, second);
        let (_tmp, ws) = cg_files(
            &format!("i264-fo1-{}", label.replace(' ', "-")),
            &cg_kind(CLOSED),
            &subscriber,
            &[("OpenKind.Enum.al", OPEN_KIND)],
        );
        let findings = d35_findings(&analyze(&ws, None));
        assert!(
            !findings.is_empty(),
            "FO1 {label}: the open arm must keep the unknown Commit, got none"
        );
        let snapshot = compose_snapshot(&single_app_model(&ws));
        let closed_rows: Vec<_> = snapshot
            .callsite_resolutions
            .iter()
            // The subscriber's (50104) site only: the publisher's IsDue is closed.
            .filter(|r| {
                r.from.contains(":Codeunit:50104#")
                    && r.status == "polymorphic"
                    && r.open_world == Some(false)
            })
            .collect();
        assert!(closed_rows.is_empty(), "FO1 {label}: {closed_rows:#?}");
    }
}

/// #265 A6d's dependency: `Dep Pub.Raise` raises `OnRaised`; `Dep Sub` handles
/// it with an interface call of its own (`S.NextKm`, open world).
const DEP_PUB: &str = r#"codeunit 70010 "Dep Pub"
{
    procedure Raise()
    begin
        OnRaised();
    end;

    [IntegrationEvent(false, false)]
    local procedure OnRaised()
    begin
    end;
}
"#;

const DEP_SUB: &str = r#"codeunit 70011 "Dep Sub"
{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::"Dep Pub", 'OnRaised', '', false, false)]
    local procedure HandleRaised()
    var
        S: Interface "Dep Strategy";
    begin
        if S.NextKm(1) > 0 then
            exit;
    end;
}
"#;

const A6D_WS: &str = r#"codeunit 50101 "Ws Caller"
{
    procedure Go()
    var
        P: Codeunit "Dep Pub";
    begin
        P.Raise();
    end;
}
"#;

/// #265 A6d (stated limit, Discovery D1): a subscriber that lives in a SOURCE
/// dependency, reported at `--scope all` in the default cross-app mode, keeps
/// today's text: its own uncertainty edges are not on the detector context, so
/// no `unresolved:` segment, and it stays under "partly analysed".
#[test]
fn dependency_subscriber_keeps_todays_text() {
    let (_tmp, ws) = write_files(
        "i265-a6d",
        &[("app.json", &dep264_app_json()), ("src/Ws.al", A6D_WS)],
    );
    // DEP_STRATEGY_SYMBOLS's codeunit list plus `Dep Pub` and `Dep Sub`.
    let codeunits = DEP_STRATEGY_SYMBOLS
        .strip_suffix(']')
        .expect("precondition: the symbols end with the codeunit list");
    let symbols = format!(
        r#"{codeunits},{{"Id":70010,"Name":"Dep Pub","Methods":[{{"Name":"Raise","Parameters":[]}}]}},{{"Id":70011,"Name":"Dep Sub","Methods":[]}}]"#
    );
    write_dep264(
        &ws,
        &symbols,
        &[
            ("src/DepStrategy.al", DEP_STRATEGY),
            ("src/DepImpl.al", DEP_IMPL),
            ("src/DepPub.al", DEP_PUB),
            ("src/DepSub.al", DEP_SUB),
        ],
    );
    let findings = d35_findings(&analyze_in(&ws, None, None, Scope::All, false).0);
    let dep: Vec<&Value> = findings
        .iter()
        .filter(|f| f["primaryLocation"]["routineName"] == "HandleRaised")
        .collect();
    // Preconditions: the dependency subscriber has a d35 unknown finding, and
    // it names itself as partly analysed (so the next check is not vacuous).
    assert_eq!(
        dep.len(),
        1,
        "A6d precondition: one d35 finding on HandleRaised, got {findings:#?}"
    );
    let fix = dep[0]["fixHint"]["description"]
        .as_str()
        .expect("fixHint.description");
    assert!(
        fix.contains("partly analysed: HandleRaised (Codeunit 70011)"),
        "A6d precondition: listed under partly analysed: {fix}"
    );
    assert!(
        !fix.contains("unresolved:"),
        "A6d: no own-site segment: {fix}"
    );
}

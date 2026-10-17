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

fn write_workspace(tag: &str, body: &str, page: &str) -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::Builder::new()
        .prefix(&format!("alsem-i148-{tag}-"))
        .tempdir()
        .expect("create scratch ws");
    let root = tmp.path().to_path_buf();
    let src = root.join("src");
    std::fs::create_dir_all(&src).expect("create src dir");
    std::fs::write(root.join("app.json"), APP_JSON).expect("write app.json");
    std::fs::write(src.join("ReproOrder.Table.al"), TABLE).expect("write table");
    std::fs::write(src.join("ReasonDialog.Page.al"), page).expect("write page");
    std::fs::write(src.join("DeleteGovernor.Codeunit.al"), codeunit(body)).expect("write cu");
    (tmp, root)
}

fn analyze(ws: &Path, detector: Option<&str>) -> Value {
    let args = AnalyzeArgs {
        workspace: ws.to_string_lossy().to_string(),
        min_severity: None,
        detector: detector.map(str::to_string),
        preset: None,
        scope: Scope::Primary,
        limit: None,
        format: OutputFormat::Json,
        sarif_version_override: None,
        fail_on: None,
        require_dependencies: false,
        baseline: None,
        update_baseline: false,
        disable_inline_suppression: false,
        group_by: None,
        deterministic: true,
        single_app: false,
        with_evidence: true,
    };
    let out = match run_analyze_with_exit(&args, "i148-test") {
        Ok((out, _, _)) => out,
        Err(e) => panic!("run_analyze failed: {e}"),
    };
    serde_json::from_str(&out).expect("analyze JSON must parse")
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
    assert_eq!(f["severity"], "high", "{label}: severity");
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
    let only = run_variant("v1-d35", RUNMODAL, PAGE_NO_TRIGGER, Some(D35));
    assert_unknown_finding(&only, "V1 --detector d35");
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

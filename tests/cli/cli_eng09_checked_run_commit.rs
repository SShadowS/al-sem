//! ENG-09 (engine limits L16 item 2): a checked `Codeunit.Run` commits.
//! Before the fix `prove may-commit` answered `no` for
//! `if Codeunit.Run(X) then;` when X has no `Commit()` of its own, and
//! `unknown` for a run-time id.
//!
//! Measured on the BC 28.4 Linux sidecar (ENG-09 probe): after a checked run
//! of a codeunit that inserts a row and never calls `Commit()`, a second
//! checked run succeeds, which BC refuses while a write is pending (the
//! control: an unchecked run, or an `Insert` of the caller's own, makes that
//! second run fail). So the checked run committed its write. It did so under
//! `[CommitBehavior(Ignore)]` and `(Error)` as well, without an error, and in
//! the variable form `Ok := W.Run()`. Learn: "the implicit commit by
//! `Codeunit.Run` isn't affected by `CommitBehavior`".
//!
//! A run that fails commits nothing and the caller carries on (the run
//! returns `false`), so the commit is never `unconditional on success`.

use serde_json::Value;
use std::process::Command;

const SOURCE: &str = r#"table 50900 "CR Row" { fields { field(1; K; Integer) { } } keys { key(PK; K) { } } }
codeunit 50910 "CR Writer"
{
    trigger OnRun()
    var
        R: Record "CR Row";
    begin
        R.K := 1;
        R.Insert();
    end;
}
codeunit 50920 "CR Caller"
{
    procedure Checked()
    begin
        if Codeunit.Run(Codeunit::"CR Writer") then;
    end;
    procedure CheckedAssigned(): Boolean
    begin
        exit(Codeunit.Run(Codeunit::"CR Writer"));
    end;
    procedure Unchecked()
    begin
        Codeunit.Run(Codeunit::"CR Writer");
    end;
    procedure CheckedVar()
    var
        W: Codeunit "CR Writer";
    begin
        if W.Run() then;
    end;
    procedure UncheckedVar()
    var
        W: Codeunit "CR Writer";
    begin
        W.Run();
    end;
    procedure CheckedDynamic(Id: Integer)
    begin
        if Codeunit.Run(Id) then;
    end;
    procedure Outer()
    begin
        Checked();
    end;
    [CommitBehavior(CommitBehavior::Ignore)]
    procedure UnderIgnore()
    begin
        if Codeunit.Run(Codeunit::"CR Writer") then;
    end;
    [CommitBehavior(CommitBehavior::Error)]
    procedure UnderError()
    begin
        if Codeunit.Run(Codeunit::"CR Writer") then;
    end;
    procedure CheckedEmpty()
    var
        E: Codeunit "CR Empty";
    begin
        if E.Run() then;
    end;
    procedure CommitOnly()
    begin
        Commit();
    end;
    procedure Via()
    begin
        Checked();
    end;
    procedure ViaCommit()
    begin
        CommitOnly();
    end;
    [CommitBehavior(CommitBehavior::Ignore)]
    procedure MixedIgnore()
    begin
        CommitOnly();
        Via();
    end;
    [CommitBehavior(CommitBehavior::Error)]
    procedure MixedError()
    begin
        Checked();
        ViaCommit();
    end;
}
codeunit 50930 "CR Empty"
{
}
codeunit 50950 "CR Pub"
{
    [IntegrationEvent(false, false)]
    procedure OnThing()
    begin
    end;
}
codeunit 50951 "CR Subs"
{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::"CR Pub", 'OnThing', '', false, false)]
    local procedure SubRun()
    begin
        RunHelper();
    end;
    [EventSubscriber(ObjectType::Codeunit, Codeunit::"CR Pub", 'OnThing', '', false, false)]
    local procedure SubCommit()
    begin
        CommitHelper();
    end;
    local procedure RunHelper()
    begin
        if Codeunit.Run(Codeunit::"CR Writer") then;
    end;
    local procedure CommitHelper()
    begin
        Commit();
    end;
}
"#;

fn workspace() -> tempfile::TempDir {
    let t = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(t.path().join("src")).unwrap();
    std::fs::write(
        t.path().join("app.json"),
        r#"{"id":"e09e09e0-0000-4000-8000-0000000000aa","name":"ENG09","publisher":"t","version":"1.0.0.0","dependencies":[]}"#,
    )
    .unwrap();
    std::fs::write(t.path().join("src/cr.al"), SOURCE).unwrap();
    t
}

fn ask(ws: &str, routine: &str, question: &str, mode: &[&str]) -> Value {
    let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .args(
            ["prove", ws, &format!("CR Caller::{routine}"), question]
                .into_iter()
                .chain(mode.iter().copied()),
        )
        .env("RUST_LOG", "off")
        .output()
        .expect("spawn alsem");
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: Value = serde_json::from_slice(&out.stdout).expect("json");
    v["payload"]["result"].clone()
}

/// The 1-based line of the first line holding `text` after the line holding
/// `after`.
fn line_of(text: &str, after: &str) -> u64 {
    let start = SOURCE.lines().position(|l| l.contains(after)).expect(after);
    let at = SOURCE
        .lines()
        .skip(start)
        .position(|l| l.contains(text))
        .expect(text);
    (start + at + 1) as u64
}

/// A checked run commits, in every form, and the commit is the run's call
/// site; an unchecked run does not.
#[test]
fn a_checked_run_commits_at_its_call_site() {
    let t = workspace();
    let ws = t.path().to_str().unwrap();
    for (routine, call) in [
        ("Checked", "Codeunit.Run("),
        ("CheckedAssigned", "Codeunit.Run("),
        ("CheckedVar", "W.Run()"),
        ("CheckedDynamic", "Codeunit.Run(Id)"),
        // A codeunit with no `OnRun`: the run still starts it and commits.
        ("CheckedEmpty", "E.Run()"),
    ] {
        let r = ask(ws, routine, "may-commit", &["--single-app"]);
        assert_eq!(r["answer"], "yes", "{routine}: {r}");
        let ev = &r["evidence"][0];
        assert_eq!(ev["type"], "COMMIT", "{routine}: {r}");
        assert_eq!(
            ev["evidence"]["line"].as_u64(),
            Some(line_of(call, &format!("procedure {routine}("))),
            "{routine}: {r}"
        );
    }
    for routine in ["Unchecked", "UncheckedVar"] {
        let r = ask(ws, routine, "may-commit", &["--single-app"]);
        assert_eq!(r["answer"], "no", "{routine}: {r}");
    }
    // Through a plain call, and in the default (cross-app) mode.
    assert_eq!(
        ask(ws, "Outer", "may-commit", &["--single-app"])["answer"],
        "yes"
    );
    assert_eq!(ask(ws, "Checked", "may-commit", &[])["answer"], "yes");
}

/// A run that fails commits nothing, so the run's commit never makes
/// `commits-on-success-path` a definite yes.
#[test]
fn a_checked_runs_commit_is_never_unconditional() {
    let t = workspace();
    let ws = t.path().to_str().unwrap();
    for routine in ["Checked", "CheckedVar", "Outer"] {
        let r = ask(ws, routine, "commits-on-success-path", &["--single-app"]);
        assert_eq!(r["answer"], "unknown", "{routine}: {r}");
        assert_eq!(
            r["blockedBy"][0]["kind"], "non-unconditional-effect-exists",
            "{routine}: {r}"
        );
    }
}

/// `[CommitBehavior]` does not change the run's commit: it still commits
/// under `Ignore` and raises no error under `Error`.
#[test]
fn commit_behavior_does_not_change_a_checked_runs_commit() {
    let t = workspace();
    let ws = t.path().to_str().unwrap();
    for routine in ["UnderIgnore", "UnderError"] {
        let r = ask(ws, routine, "may-commit", &["--single-app"]);
        assert_eq!(r["answer"], "yes", "{routine}: {r}");
        let r = ask(ws, routine, "throws-error", &["--single-app"]);
        assert_eq!(r["answer"], "no", "{routine}: {r}");
        // The `--site`/`--match` slice applies the same rule.
        let site = format!(
            "src/cr.al:{}",
            line_of("Codeunit.Run(", &format!("procedure {routine}("))
        );
        let r = ask(
            ws,
            routine,
            "may-commit",
            &["--site", &site, "--single-app"],
        );
        assert_eq!(r["answer"], "yes", "{routine} --site: {r}");
        let r = ask(
            ws,
            routine,
            "throws-error",
            &["--match", "Codeunit.Run", "--single-app"],
        );
        assert_eq!(r["answer"], "no", "{routine} --match: {r}");
        assert_eq!(r["otherSites"], serde_json::json!([]), "{routine}: {r}");
    }
}

/// A root that reaches both a `Commit()` and a checked run: the scope applies
/// to the `Commit()` only, whichever of the two is nearer. Under `Ignore` the
/// run still commits (and only the run: the `Commit()` is ignored); under
/// `Error` the farther `Commit()` still raises its error.
#[test]
fn a_commit_and_a_checked_run_under_one_scope_stay_apart() {
    let t = workspace();
    let ws = t.path().to_str().unwrap();
    let r = ask(ws, "MixedIgnore", "may-commit", &["--single-app"]);
    assert_eq!(r["answer"], "yes", "{r}");
    let lines: Vec<u64> = r["evidence"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|e| e["evidence"]["line"].as_u64())
        .collect();
    assert_eq!(
        lines,
        vec![line_of("Codeunit.Run(", "procedure Checked(")],
        "only the run's commit survives Ignore: {r}"
    );
    let r = ask(ws, "MixedError", "throws-error", &["--single-app"]);
    assert_eq!(r["answer"], "yes", "the Commit() under Error: {r}");
    let r = ask(ws, "MixedError", "may-commit", &["--single-app"]);
    assert_eq!(r["answer"], "yes", "the run still commits: {r}");
}

/// d35 (a `Commit()` reachable from an event subscriber) asks about a commit
/// of the publisher's work, which a checked run's own commit never is: a
/// subscriber whose helper runs a codeunit checked is no finding, the
/// `Commit()` control is.
#[test]
fn a_checked_runs_commit_is_no_d35_finding() {
    let t = workspace();
    let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .args([
            "analyze",
            t.path().to_str().unwrap(),
            "--detector",
            "d35-commit-in-event-subscriber",
            "--format",
            "json",
        ])
        .env("RUST_LOG", "off")
        .output()
        .expect("spawn alsem");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let doc: Value = serde_json::from_slice(&out.stdout).expect("json");
    let flagged: Vec<&str> = doc["payload"]["findings"]
        .as_array()
        .expect("findings")
        .iter()
        .filter_map(|f| f["primaryLocation"]["routineName"].as_str())
        .collect();
    assert_eq!(flagged, vec!["SubCommit"], "{doc}");
}

const DEP_GUID: &str = "e09e09e0-0000-4000-8000-0000000000d1";

const DEP_SYMBOLS: &str = r#"{"RuntimeVersion":"13.0","Codeunits":[{"Id":50960,"Name":"CRD Both","Methods":[{"Name":"Both","Parameters":[]}]},{"Id":50961,"Name":"CRD Worker","Methods":[]}],"AppId":"e09e09e0-0000-4000-8000-0000000000d1","Name":"CRDep","Publisher":"probe","Version":"1.0.0.0"}"#;

/// One dependency routine that holds both kinds of commit: a `Commit()` and a
/// checked run.
const DEP_SOURCE: &str = r#"codeunit 50960 "CRD Both"
{
    procedure Both()
    begin
        Commit();
        if Codeunit.Run(Codeunit::"CRD Worker") then;
    end;
}
codeunit 50961 "CRD Worker"
{
    trigger OnRun()
    begin
    end;
}
"#;

const WS_SOURCE: &str = r#"codeunit 50970 "CRX Root"
{
    procedure Plain()
    var
        D: Codeunit "CRD Both";
    begin
        D.Both();
    end;
    [CommitBehavior(CommitBehavior::Ignore)]
    procedure UnderIgnore()
    var
        D: Codeunit "CRD Both";
    begin
        D.Both();
    end;
}
"#;

fn dep_workspace() -> tempfile::TempDir {
    let t = tempfile::tempdir().unwrap();
    let ws = t.path();
    std::fs::create_dir_all(ws.join("src")).unwrap();
    std::fs::write(
        ws.join("app.json"),
        format!(
            r#"{{"id":"e09e09e0-0000-4000-8000-0000000000d2","name":"CRX","publisher":"t","version":"1.0.0.0",
            "dependencies":[{{"id":"{DEP_GUID}","name":"CRDep","publisher":"probe","version":"1.0.0.0"}}]}}"#
        ),
    )
    .unwrap();
    std::fs::write(ws.join("src/Root.al"), WS_SOURCE).unwrap();
    crate::symbol_app::write_source_app(
        &ws.join(".alpackages/probe_CRDep_1.0.0.0.app"),
        DEP_GUID,
        "CRDep",
        "1.0.0.0",
        DEP_SYMBOLS,
        &[("src/CRDBoth.Codeunit.al", DEP_SOURCE)],
        "",
    );
    t
}

/// Beyond a dependency crossing, a `Commit()` and a checked run's commit are
/// two rows with two ids, never one: the run's row is marked `checked-run`
/// and ends on its own site, and under `Ignore` only the run's row is left.
#[test]
fn beyond_a_crossing_the_two_kinds_of_commit_stay_apart() {
    let t = dep_workspace();
    let ws = t.path().to_str().unwrap();
    let prove = |routine: &str| -> Value {
        let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
            .args(["prove", ws, &format!("CRX Root::{routine}"), "may-commit"])
            .env("RUST_LOG", "off")
            .output()
            .expect("spawn alsem");
        assert_eq!(
            out.status.code(),
            Some(0),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice::<Value>(&out.stdout).expect("json")["payload"]["result"].clone()
    };
    // Each COMMIT row beyond the crossing: (is it the run's, the dependency
    // site its witness ends on: `cs…` a call site, `op…` an operation).
    let rows = |r: &Value| -> Vec<(bool, String)> {
        let mut v: Vec<(bool, String)> = r["evidence"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|e| e["type"] == "COMMIT" && e["detail"]["boundary"].is_string())
            .map(|e| {
                let site = e["groupWitness"]["terminal"]["operationId"]
                    .as_str()
                    .and_then(|id| id.rsplit('/').next())
                    .unwrap_or("none")
                    .to_string();
                (e["detail"]["resourceId"] == "checked-run", site)
            })
            .collect();
        v.sort();
        v
    };
    let r = prove("Plain");
    assert_eq!(r["answer"], "yes", "{r}");
    assert_eq!(
        rows(&r),
        vec![(false, "op0".to_string()), (true, "cs0".to_string())],
        "the Commit() and the run, apart, each ending on its own site: {r}"
    );
    // Two rows, two ids (the ordering engine finds a row by its id).
    let ids: std::collections::BTreeSet<&str> = r["evidence"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["type"] == "COMMIT")
        .filter_map(|e| e["factId"].as_str())
        .collect();
    assert_eq!(ids.len(), 2, "one id per row: {r}");
    let r = prove("UnderIgnore");
    assert_eq!(r["answer"], "yes", "{r}");
    assert_eq!(
        rows(&r),
        vec![(true, "cs0".to_string())],
        "under Ignore only the run's commit is left: {r}"
    );
}

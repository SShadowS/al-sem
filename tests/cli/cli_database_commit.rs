//! #155: `Database.Commit()` is a commit, exactly as `Commit()` is.
//!
//! AL's `Commit` is a method of the `Database` type that may be called without
//! its receiver, so `Commit()`, `Commit`, `Database.Commit()` and
//! `Database.Commit` (any case) all commit the transaction. The body walk
//! recognised only a bare `Commit` callee: `Database.Commit()` became a call
//! site to the `Database::commit` builtin and produced no commit operation
//! site, so prove may-commit answered `no`, explain and digest listed no
//! COMMIT, and every analysis reading commit sites (d34, d35, transaction
//! spans and pending writes) missed it.
//!
//! A declared `Database` (a variable, here a codeunit with a `Commit`
//! procedure) shadows the platform's; its `Database.Commit()` is an ordinary
//! call and no commit.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

const FILE: &str = "ws:src/DbCommit.Codeunit.al";

/// Line numbers are pinned by the tests below: 6, 17, 22, 27, 32 and 40 are
/// the commits.
const DB_COMMIT: &str = r#"codeunit 50155 "Db Commit Mgt"
{
    [EventSubscriber(ObjectType::Codeunit, 50, 'OnAfterPost', '', false, false)]
    local procedure OnAfterPostDirect()
    begin
        Database.Commit();
    end;

    [EventSubscriber(ObjectType::Codeunit, 50, 'OnAfterValidate', '', false, false)]
    local procedure OnAfterValidateTransitive()
    begin
        PersistAudit();
    end;

    local procedure PersistAudit()
    begin
        Database.Commit();
    end;

    procedure Parenless()
    begin
        Database.Commit;
    end;

    procedure UpperCase()
    begin
        DATABASE.COMMIT();
    end;

    procedure BareParenless()
    begin
        Commit;
    end;

    procedure InLoop()
    var
        i: Integer;
    begin
        for i := 1 to 3 do
            Database.Commit();
    end;
}
"#;

const SHADOW: &str = r#"codeunit 50156 "Db Shadow"
{
    procedure Shadowed()
    var
        Database: Codeunit "Fake Db";
    begin
        Database.Commit();
    end;
}

codeunit 50157 "Fake Db"
{
    procedure Commit()
    begin
    end;
}
"#;

fn workspace(root: &Path) -> PathBuf {
    let ws = root.join("ws");
    std::fs::create_dir_all(ws.join("src")).unwrap();
    std::fs::write(
        ws.join("app.json"),
        r#"{"id":"c0c00155-0000-4000-8000-000000000155","name":"DbCommit","publisher":"t","version":"1.0.0.0"}"#,
    )
    .unwrap();
    std::fs::write(ws.join("src/DbCommit.Codeunit.al"), DB_COMMIT).unwrap();
    std::fs::write(ws.join("src/DbShadow.Codeunit.al"), SHADOW).unwrap();
    ws
}

fn alsem(args: &[&str]) -> (Option<i32>, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .args(args)
        .env("RUST_LOG", "off")
        .output()
        .unwrap();
    (
        out.status.code(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
    )
}

/// `prove <routine> may-commit`: the answer and the COMMIT rows' places.
fn may_commit(ws: &Path, routine: &str) -> (String, Vec<String>) {
    let (code, stdout) = alsem(&["prove", ws.to_str().unwrap(), routine, "may-commit"]);
    assert_eq!(code, Some(0), "prove {routine}: {stdout}");
    let v: Value = serde_json::from_str(&stdout).expect("prove JSON");
    let result = &v["payload"]["result"];
    let places = result["evidence"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter(|e| e["type"] == "COMMIT")
                .map(|e| {
                    format!(
                        "{}:{}",
                        e["evidence"]["file"].as_str().unwrap(),
                        e["evidence"]["line"]
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    (result["answer"].as_str().unwrap().to_string(), places)
}

/// The findings' detector names, sorted, and their ids.
fn findings(ws: &Path) -> Vec<(String, String)> {
    let (code, stdout) = alsem(&["analyze", ws.to_str().unwrap()]);
    assert!(code.is_some(), "analyze: {stdout}");
    let v: Value = serde_json::from_str(&stdout).expect("analyze JSON");
    let mut out: Vec<(String, String)> = v["payload"]["findings"]
        .as_array()
        .expect("findings")
        .iter()
        .map(|f| {
            (
                f["detector"].as_str().unwrap().to_string(),
                f["id"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    out.sort();
    out
}

/// Every spelling commits, at its own line: the control `Commit;` and four
/// `Database.Commit` forms, direct and one hop down.
#[test]
fn every_commit_spelling_may_commit_at_its_site() {
    let t = tempfile::tempdir().unwrap();
    let ws = workspace(t.path());
    for (routine, line) in [
        ("BareParenless", 32),
        ("OnAfterPostDirect", 6),
        ("PersistAudit", 17),
        ("OnAfterValidateTransitive", 17),
        ("Parenless", 22),
        ("UpperCase", 27),
        ("InLoop", 40),
    ] {
        assert_eq!(
            may_commit(&ws, &format!("Db Commit Mgt::{routine}")),
            ("yes".to_string(), vec![format!("{FILE}:{line}")]),
            "{routine}"
        );
    }
}

/// A declared `Database` shadows the platform's: no commit.
#[test]
fn a_shadowed_database_commit_is_no_commit() {
    let t = tempfile::tempdir().unwrap();
    let ws = workspace(t.path());
    assert_eq!(
        may_commit(&ws, "Db Shadow::Shadowed"),
        ("no".to_string(), vec![])
    );
}

/// explain's commit line and digest's COMMIT row name the `Database.Commit()`.
#[test]
fn explain_and_digest_list_the_database_commit() {
    let t = tempfile::tempdir().unwrap();
    let ws = workspace(t.path());
    let (code, stdout) = alsem(&[
        "explain",
        ws.to_str().unwrap(),
        "--routine",
        "Db Commit Mgt::OnAfterValidateTransitive",
    ]);
    assert_eq!(code, Some(0), "{stdout}");
    assert!(stdout.contains("commits: yes (1 site)"), "{stdout}");
    assert!(
        stdout.contains(&format!("COMMIT at {FILE}:17 (unconditional on success)")),
        "{stdout}"
    );

    let (code, stdout) = alsem(&[
        "digest",
        ws.to_str().unwrap(),
        "--routine",
        "Db Commit Mgt::PersistAudit",
    ]);
    assert_eq!(code, Some(0), "{stdout}");
    let v: Value = crate::as_v3(serde_json::from_str(&stdout).expect("digest JSON"));
    let commits: Vec<String> = v["payload"]["entries"][0]["effects"]
        .as_array()
        .expect("effects")
        .iter()
        .filter(|e| e["type"] == "COMMIT")
        .map(|e| {
            format!(
                "{}:{}",
                e["evidence"]["file"].as_str().unwrap(),
                e["evidence"]["line"]
            )
        })
        .collect();
    assert_eq!(commits, vec![format!("{FILE}:17")]);
}

/// d35 reports the subscriber that commits directly (at its op site) and the
/// one that commits through a callee; d34 the commit in a loop. The shadowed
/// call adds nothing.
#[test]
fn analyze_reports_the_database_commits() {
    let t = tempfile::tempdir().unwrap();
    let ws = workspace(t.path());
    let got = findings(&ws);
    let detectors: Vec<&str> = got.iter().map(|(d, _)| d.as_str()).collect();
    assert_eq!(
        detectors,
        vec![
            "d34-commit-in-loop",
            "d35-commit-in-event-subscriber",
            "d35-commit-in-event-subscriber"
        ],
        "{got:#?}"
    );
    let d35: Vec<&str> = got[1..].iter().map(|(_, id)| id.as_str()).collect();
    assert!(
        d35.iter().any(|id| id.ends_with("/op0"))
            && d35.iter().any(|id| id.ends_with("/transitive")),
        "one direct, one transitive: {d35:?}"
    );
}

/// A posting chain whose event subscriber writes and then commits (the
/// `ws-d8-commit-in-tx` shape): the transaction span (d9) and d35 are the same
/// whichever spelling the subscriber commits with.
fn posting_workspace(root: &Path, commit: &str) -> PathBuf {
    let ws = root.join(commit.replace(['.', '(', ')', ';'], ""));
    std::fs::create_dir_all(ws.join("src")).unwrap();
    std::fs::write(
        ws.join("app.json"),
        r#"{"id":"c0c00156-0000-4000-8000-000000000156","name":"DbCommitTx","publisher":"t","version":"1.0.0.0"}"#,
    )
    .unwrap();
    std::fs::write(
        ws.join("src/Posting.al"),
        r#"codeunit 50158 "Tx Posting"
{
    procedure PostDoc()
    var
        Header: Record "Tx Header";
        Line: Record "Tx Line";
    begin
        Header.Get('001');
        Header.Posted := true;
        Header.Modify();
        Line.SetRange("Document No.", Header."No.");
        Line.ModifyAll(Posted, true);
        OnAfterPostDoc(Header);
        Header.Modify();
    end;

    [IntegrationEvent(false, false)]
    procedure OnAfterPostDoc(Header: Record "Tx Header")
    begin
    end;
}

table 50158 "Tx Header"
{
    fields
    {
        field(1; "No."; Code[20]) { }
        field(2; Posted; Boolean) { }
    }
}

table 50159 "Tx Line"
{
    fields
    {
        field(1; "Document No."; Code[20]) { }
        field(2; Posted; Boolean) { }
    }
}

table 50160 "Tx Audit"
{
    fields
    {
        field(1; "Entry No."; Integer) { }
        field(2; "Doc No."; Code[20]) { }
    }
}
"#,
    )
    .unwrap();
    std::fs::write(
        ws.join("src/Subscriber.al"),
        format!(
            r#"codeunit 50161 "Tx Subscriber"
{{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::"Tx Posting", 'OnAfterPostDoc', '', true, true)]
    local procedure HandlePosted(Header: Record "Tx Header")
    var
        Audit: Record "Tx Audit";
    begin
        Audit.Init();
        Audit."Doc No." := Header."No.";
        Audit.Insert();
        {commit}
    end;
}}
"#
        ),
    )
    .unwrap();
    ws
}

#[test]
fn the_transaction_span_is_the_same_for_both_spellings() {
    let t = tempfile::tempdir().unwrap();
    let detectors = |commit: &str| -> Vec<String> {
        findings(&posting_workspace(t.path(), commit))
            .into_iter()
            .map(|(d, _)| d)
            .collect()
    };
    // The precondition: `Commit()` gives the span summary and d35.
    let bare = detectors("Commit();");
    assert_eq!(
        bare,
        vec![
            "d35-commit-in-event-subscriber".to_string(),
            "d9-transaction-span-summary".to_string()
        ]
    );
    assert_eq!(detectors("Database.Commit();"), bare);
}

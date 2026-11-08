//! r3 §7 R5 (spec `2026-11-08-output-validation-r3.md`): an error answer
//! states its boundary. A subscriber's error witness that ends in
//! dependency code counts the guards of the route the analysis replayed,
//! through the dependency entry to the operation, with their expressions
//! and polarity (never the bare `condition not resolved` class label when a
//! route was replayed), and says that whether the error escapes the
//! publisher is not established.
//!
//! The shape (m04, p08): `Bnd Subs.OnAfterThing` subscribes to `Bnd
//! Pub.OnAfterThing`; under `if Code = 'X'` it calls the dependency's
//! `Bnd Dep.Make`, whose `T.Insert()` (an implicit error) sits under `if
//! Code <> ''`.

use std::path::Path;
use std::process::Command;

const DEP_GUID: &str = "b0d00001-0000-4000-8000-0000000000b1";

const DEP_SYMBOLS: &str = r#"{"RuntimeVersion":"13.0","Codeunits":[{"Id":50500,"Name":"Bnd Dep","Methods":[{"Name":"Make","Parameters":[{"Name":"Code","TypeDefinition":{"Name":"Code","Subtype":{"Name":"20"}}}]}]},{"Id":50503,"Name":"Bnd Dep Two","Methods":[{"Name":"MakeBoth","Parameters":[{"Name":"Code","TypeDefinition":{"Name":"Code","Subtype":{"Name":"20"}}}]}]}],"Tables":[{"Id":50501,"Name":"Bnd Dep Rec","Fields":[{"Id":1,"Name":"Code","TypeDefinition":{"Name":"Code"}}]},{"Id":50502,"Name":"Bnd Dep Other","Fields":[{"Id":1,"Name":"Code","TypeDefinition":{"Name":"Code"}}]}],"AppId":"b0d00001-0000-4000-8000-0000000000b1","Name":"BndDep","Publisher":"probe","Version":"1.0.0.0"}"#;

/// `MakeBoth` writes each table under its own guard: `Bnd Dep Rec` under
/// `Code <> ''`, `Bnd Dep Other` under `Code = 'Y'` (line 13, its Insert on
/// line 15).
const DEP_TWO: &str = r#"codeunit 50503 "Bnd Dep Two"
{
    procedure MakeBoth(Code: Code[20])
    var
        T: Record "Bnd Dep Rec";
        U: Record "Bnd Dep Other";
    begin
        if Code <> '' then begin
            T.Init();
            T.Code := Code;
            T.Insert();
        end;
        if Code = 'Y' then begin
            U.Code := Code;
            U.Insert();
        end;
    end;
}
"#;

const DEP_OTHER: &str = r#"table 50502 "Bnd Dep Other"
{
    fields
    {
        field(1; Code; Code[20]) { }
    }
    keys
    {
        key(PK; Code) { Clustered = true; }
    }
}
"#;

/// Line 7 is `Make`'s guard, line 10 its `T.Insert()`.
const DEP_SOURCE: &str = r#"codeunit 50500 "Bnd Dep"
{
    procedure Make(Code: Code[20])
    var
        T: Record "Bnd Dep Rec";
    begin
        if Code <> '' then begin
            T.Init();
            T.Code := Code;
            T.Insert();
        end;
    end;
}
"#;

const DEP_TABLE: &str = r#"table 50501 "Bnd Dep Rec"
{
    fields
    {
        field(1; Code; Code[20]) { }
    }
    keys
    {
        key(PK; Code) { Clustered = true; }
    }
}
"#;

/// Line 8 is the subscriber's guard, line 9 its call into the dependency.
const WS_SOURCE: &str = r#"codeunit 50510 "Bnd Subs"
{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::"Bnd Pub", 'OnAfterThing', '', false, false)]
    local procedure OnAfterThing(Code: Code[20])
    var
        Dep: Codeunit "Bnd Dep";
    begin
        if Code = 'X' then
            Dep.Make(Code);
    end;
}

codeunit 50511 "Bnd Pub"
{
    procedure Run()
    begin
        OnAfterThing('A');
    end;

    [IntegrationEvent(false, false)]
    procedure OnAfterThing(Code: Code[20])
    begin
    end;
}

codeunit 50512 "Bnd Writer"
{
    procedure WriteBoth(Code: Code[20])
    var
        Two: Codeunit "Bnd Dep Two";
    begin
        Two.MakeBoth(Code);
    end;
}
"#;

fn workspace(root: &Path) -> std::path::PathBuf {
    let ws = root.join("ws");
    std::fs::create_dir_all(ws.join("src")).unwrap();
    std::fs::write(
        ws.join("app.json"),
        format!(
            r#"{{"id":"b0d00002-0000-4000-8000-0000000000b2","name":"Bnd","publisher":"t","version":"1.0.0.0",
            "dependencies":[{{"id":"{DEP_GUID}","name":"BndDep","publisher":"probe","version":"1.0.0.0"}}]}}"#
        ),
    )
    .unwrap();
    std::fs::write(ws.join("src/Bnd.Codeunit.al"), WS_SOURCE).unwrap();
    crate::symbol_app::write_source_app(
        &ws.join(".alpackages/probe_BndDep_1.0.0.0.app"),
        DEP_GUID,
        "BndDep",
        "1.0.0.0",
        DEP_SYMBOLS,
        &[
            ("src/BndDep.Codeunit.al", DEP_SOURCE),
            ("src/BndDepRec.Table.al", DEP_TABLE),
            ("src/BndDepTwo.Codeunit.al", DEP_TWO),
            ("src/BndDepOther.Table.al", DEP_OTHER),
        ],
        "",
    );
    ws
}

fn prove(ws: &Path, routine: &str, question: &str) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .args(["prove", "--format", "human", "--"])
        .arg(ws)
        .args([routine, question])
        .env("RUST_LOG", "off")
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// The witness ends in dependency code: its line names the operation and
/// its own site before the entry (R3), counts the replayed route's guards
/// through the entry (the subscriber's `Code = 'X'` and the dependency's
/// `Code <> ''`), lists them with their polarity, and the answer says the
/// publisher escape is not established.
///
/// DISCRIMINATION (2026-11-08): a group witness keeping no guards
/// (`GroupPaths::paths` setting `guards: None`) makes the answer fall back
/// to `condition not resolved` and fails the guard-count assert; restored,
/// passes. (Before prove read the printed witness's guards, removing the
/// `r3 §7 R5` block in `digest_cli` that copies them onto the row failed
/// it the same way.)
#[test]
fn a_dependency_error_witness_counts_the_replayed_route_and_states_the_escape() {
    let lines: Vec<&str> = DEP_SOURCE.lines().collect();
    assert_eq!(lines[9].trim(), "T.Insert();", "precondition: line 10");
    assert_eq!(
        lines[6].trim(),
        "if Code <> '' then begin",
        "precondition: line 7"
    );
    let tmp = tempfile::tempdir().unwrap();
    let ws = workspace(tmp.path());
    let text = prove(&ws, "Codeunit \"Bnd Subs\"::OnAfterThing", "throws-error");
    let answer = text
        .lines()
        .find(|l| l.starts_with("answer:"))
        .unwrap_or_else(|| panic!("an answer line: {text}"));
    assert!(
        answer.starts_with("answer: yes — Insert (implicit error) at dep:"),
        "{text}"
    );
    assert!(answer.contains("BndDep.Codeunit.al:10, beyond "), "{text}");
    assert!(
        answer.contains(", entered at ws:src/Bnd.Codeunit.al:9"),
        "{text}"
    );
    assert!(answer.contains("guards on the route, "), "{text}");
    assert!(!answer.contains("condition not resolved"), "{text}");
    let cond = text
        .lines()
        .find(|l| l.trim_start().starts_with("condition:"))
        .unwrap_or_else(|| panic!("a condition line: {text}"));
    assert!(cond.contains("Code <> ''"), "{cond}");
    assert!(
        text.contains("  whether it escapes Codeunit \"Bnd Pub\"::OnAfterThing: not established"),
        "{text}"
    );
    // The caller-interception follow-up stays.
    assert!(text.contains("whether a caller traps it:"), "{text}");
}

/// The publisher itself (it subscribes to nothing) names no publisher
/// escape, though its cone reaches the same error.
///
/// DISCRIMINATION (2026-11-08): `subscribed_publishers` ignoring the
/// subscriber filter (every edge's publisher) prints the escape line here
/// too and fails; restored, passes.
#[test]
fn a_non_subscriber_error_names_no_publisher_escape() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = workspace(tmp.path());
    let text = prove(&ws, "Codeunit \"Bnd Pub\"::Run", "throws-error");
    assert!(
        text.contains("answer: yes"),
        "precondition: an error: {text}"
    );
    assert!(!text.contains("whether it escapes"), "{text}");
}

/// r3 §7 R3/R5: a `writes-table` witness beyond a dependency entry prints
/// the asked table's own route: its terminal operation and site, and the
/// guards of that route (`Code = 'Y'`), never another resource's (`Code <>
/// ''`, `Bnd Dep Rec`'s).
///
/// DISCRIMINATION (2026-11-08): prove's `notes_of` reading the row's own
/// notes instead of the printed witness's (`witness_notes` skipped) loses
/// the asked route's guards: the answer says `condition not resolved` and
/// the guard-count assert fails; restored, passes.
#[test]
fn a_writes_table_witness_beyond_a_dependency_entry_prints_its_own_route() {
    let lines: Vec<&str> = DEP_TWO.lines().collect();
    assert_eq!(
        lines[12].trim(),
        "if Code = 'Y' then begin",
        "precondition: line 13"
    );
    assert_eq!(lines[14].trim(), "U.Insert();", "precondition: line 15");
    let tmp = tempfile::tempdir().unwrap();
    let ws = workspace(tmp.path());
    let text = prove(
        &ws,
        "Codeunit \"Bnd Writer\"::WriteBoth",
        "writes-table:Bnd Dep Other",
    );
    let answer = text
        .lines()
        .find(|l| l.starts_with("answer:"))
        .unwrap_or_else(|| panic!("an answer line: {text}"));
    assert!(answer.starts_with("answer: yes — "), "{text}");
    assert!(
        answer.contains("BndDepTwo.Codeunit.al:15, beyond "),
        "{text}"
    );
    assert!(
        answer.contains("(1 guard on the route, 0 evaluated)"),
        "{text}"
    );
    let cond = text
        .lines()
        .find(|l| l.trim_start().starts_with("condition:"))
        .unwrap_or_else(|| panic!("a condition line: {text}"));
    assert!(cond.contains("Code = 'Y'"), "{cond}");
    assert!(!cond.contains("Code <> ''"), "{cond}");
}

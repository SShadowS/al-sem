//! A procedure's named return value of a record type (`procedure P() Ret:
//! Record T`) is a record variable of that routine, like a local. The body pass
//! took only globals, parameters and locals as record receivers, so
//! `Ret.Get(…)` and `Ret.Insert()` were not record operations: `query touches`
//! left them out (round-2 fact check c30: CDO's
//! `GetEmailTemplateFromRecord`'s `EMailTemplateHeader.Get`, and two
//! `SendCode.Get` sites on CDO Send Code), and `digest` / `explain` saw no write
//! on the returned record. The resolver already typed the receiver.
//!
//! Pinned through the shipped binary, so the body walk, the model's record
//! typing and the query and digest projections are all on the path.

use std::path::Path;
use std::process::Command;

const SOURCE: &str = r#"table 50310 "NR Header"
{
    fields { field(1; Code; Code[20]) { } field(2; Description; Text[50]) { } }
    keys { key(PK; Code) { } }
}
codeunit 50311 "NR Helper"
{
    procedure GetHeader(HeaderCode: Code[20]) Header: Record "NR Header"
    begin
        Header.Get(HeaderCode);
    end;

    procedure CreateHeader() NewHeader: Record "NR Header"
    begin
        NewHeader.Init();
        NewHeader.Code := 'X';
        NewHeader.Insert();
    end;

    procedure BufferHeader() Buffer: Record "NR Header" temporary
    begin
        Buffer.Code := 'T';
        Buffer.Insert();
    end;
}
table 50312 "NR Log"
{
    fields { field(1; K; Integer) { } }
    keys { key(PK; K) { } }
}
codeunit 50313 "NR Subscriber"
{
    [EventSubscriber(ObjectType::Table, Database::"NR Header", 'OnAfterInsertEvent', '', false, false)]
    local procedure LogInsert(var Rec: Record "NR Header"; RunTrigger: Boolean)
    var
        Log: Record "NR Log";
    begin
        Log.Insert();
    end;
}
"#;

/// The other named-return shapes the body walk's scope reads: a scalar return
/// value that shares a field's name (a bare `Amount` is the return value, not
/// the implicit record's field) and a `RecordRef` return value (its operations
/// are a RecordRef's, as a local's are).
const SCOPE_SOURCE: &str = r#"table 50310 "NR Header"
{
    fields { field(1; Code; Code[20]) { } field(2; Amount; Decimal) { } }
    keys { key(PK; Code) { } }

    procedure Compute() Amount: Decimal
    begin
        Amount := 1;
    end;
}
codeunit 50311 "NR Ref"
{
    procedure OpenRef() Ref: RecordRef
    begin
        Ref.Open(50310);
        Ref.Modify();
    end;
}
"#;

fn workspace() -> tempfile::TempDir {
    workspace_with(SOURCE)
}

fn workspace_with(source: &str) -> tempfile::TempDir {
    let t = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(t.path().join("src")).unwrap();
    std::fs::write(
        t.path().join("app.json"),
        r#"{"id":"31313131-0000-4000-8000-000000000031","name":"NR","publisher":"t","version":"1.0.0.0","idRanges":[{"from":50310,"to":50319}]}"#,
    )
    .unwrap();
    std::fs::write(t.path().join("src/Probe.al"), source).unwrap();
    t
}

#[test]
fn a_scalar_return_value_named_like_a_field_is_not_a_field_write() {
    let t = workspace_with(SCOPE_SOURCE);
    let v = alsem(
        t.path(),
        &[
            "query",
            "touches",
            "--table",
            "NR Header",
            "--field",
            "Amount",
        ],
    );
    let touches = v["payload"]["fieldTouches"].as_array().cloned();
    assert!(
        touches.as_ref().is_some_and(|a| a.is_empty()),
        "`Amount := 1` assigns the return value: {v}"
    );
}

#[test]
fn an_operation_on_a_record_ref_return_value_is_a_record_ref_operation() {
    let t = workspace_with(SCOPE_SOURCE);
    let v = alsem(
        t.path(),
        &[
            "query",
            "effects",
            "--routine",
            "Codeunit \"NR Ref\"::OpenRef",
        ],
    );
    let text = v.to_string();
    assert!(
        text.contains("\"recordRef\":{\"operations\":1"),
        "`Ref.Modify()` is on a RecordRef: {text}"
    );
}

fn alsem(ws: &Path, args: &[&str]) -> serde_json::Value {
    let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .args(args)
        .arg(ws)
        .args(["--format", "json", "--deterministic"])
        .env("RUST_LOG", "off")
        .output()
        .unwrap();
    crate::as_v3(
        serde_json::from_slice(&out.stdout)
            .unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&out.stderr))),
    )
}

/// The 1-based line of `SOURCE` that holds `snippet` (exactly one).
fn line_of(snippet: &str) -> u64 {
    let hits: Vec<u64> = SOURCE
        .lines()
        .enumerate()
        .filter(|(_, l)| l.contains(snippet))
        .map(|(i, _)| i as u64 + 1)
        .collect();
    assert_eq!(hits.len(), 1, "{snippet:?}: {hits:?}");
    hits[0]
}

#[test]
fn query_touches_lists_record_operations_on_a_named_return_value() {
    let t = workspace();
    let v = alsem(t.path(), &["query", "touches", "--table", "NR Header"]);
    // `Header.Get`, `NewHeader.Init`, `NewHeader.Insert` and `Buffer.Insert`:
    // every operation in the fixture is on a named return value.
    let sites = &v["payload"]["up"]["directSites"];
    assert_eq!(sites["workspaceSites"], 4, "{v}");
    assert_eq!(sites["workspaceRoutines"], 3, "{v}");
    assert_eq!(
        sites["workspaceByOp"],
        serde_json::json!({"Get": 1, "Init": 1, "Insert": 2}),
        "{v}"
    );
    assert_eq!(sites["keyRelevant"], serde_json::json!({"Get": 1}), "{v}");
}

#[test]
fn explain_lists_the_write_on_a_named_return_value_at_its_line() {
    let t = workspace();
    let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .args([
            "explain",
            "--routine",
            "Codeunit \"NR Helper\"::CreateHeader",
        ])
        .arg(t.path())
        .args(["--format", "human"])
        .env("RUST_LOG", "off")
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{text}");
    let line = line_of("NewHeader.Insert();");
    assert!(
        text.contains("NR Header: Insert 1 — known persistent")
            && text.contains(&format!("Insert at ws:src/Probe.al:{line}")),
        "the Insert on the returned record in explain: {text}"
    );
    // The resolver sees the same record operation (its site extraction shares
    // `named_return_var`), so the Insert raises the table's OnAfterInsertEvent and
    // the cone follows it into the subscriber's write.
    let log = line_of("Log.Insert();");
    assert!(
        text.contains(&format!("Insert at ws:src/Probe.al:{log}"))
            && text.contains("CreateHeader → [event] Codeunit \"NR Subscriber\"::LogInsert"),
        "the subscriber's write through the table event: {text}"
    );
}

#[test]
fn digest_sees_writes_on_a_named_return_value_with_its_temp_state() {
    let t = workspace();
    let d = alsem(t.path(), &["digest", "--file", "src/Probe.al"]);
    let text = d.to_string();
    // The physical return value's Insert is a persistent write of the table; the
    // `temporary` one's Insert is a temporary-only write.
    let temp_states: Vec<String> = text
        .match_indices(
            "\"resourceId\":\"31313131-0000-4000-8000-000000000031/table/50310\",\"tempState\":\"",
        )
        .map(|(i, k)| text[i + k.len()..].split('"').next().unwrap().to_string())
        .collect();
    assert!(
        temp_states.iter().any(|s| s == "known(false)"),
        "a persistent write: {temp_states:?} {text}"
    );
    assert!(
        temp_states.iter().any(|s| s == "known(true)"),
        "a temporary write: {temp_states:?} {text}"
    );
    assert!(
        !text.contains("unresolved-receiver-type"),
        "no receiver left untyped: {text}"
    );
}

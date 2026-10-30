//! G36: a RecordRef write behind `if not R.FieldExist(N) then exit;` writes
//! only a table that has field `N`.
//!
//! Continia DO's `UpdateHandledFields(var FilterRecord: RecordRef)` modifies
//! whatever table the RecordRef holds, after `if not
//! FilterRecord.FieldExist(6175274) then exit;`. Every `writes-table` question
//! got an `unknown-table-write`; only tables a DO extension gives that field
//! (Sales Header, …) can be written there.
//!
//! The fixture also has a real table sharing its number with the extension
//! (`FG Other` 50400 / `FG Doc Ext` 50400, as DO's 6175275): the model's
//! extension-field merge looked the extension's stub up by that id and took
//! the table instead, so `FG Doc` never got field 50.

use std::process::Command;

/// The extension comes BEFORE the table sharing its number: the merge took
/// the last row of that id.
const SOURCE: &str = r#"tableextension 50400 "FG Doc Ext" extends "FG Doc"
{
    fields { field(50; Handled; Boolean) { } }
}
table 50400 "FG Other"
{
    fields { field(1; K; Integer) { } field(2; N; Text[20]) { } }
    keys { key(PK; K) { } }
}
table 50401 "FG Doc"
{
    fields { field(1; K; Integer) { } }
    keys { key(PK; K) { } }
}
table 50402 "FG Log"
{
    fields { field(1; K; Integer) { } }
    keys { key(PK; K) { } }
}
codeunit 50403 "FG Mgt"
{
    procedure SetHandled(V: Variant)
    var
        RecRef: RecordRef;
    begin
        RecRef.GetTable(V);
        Update(RecRef);
    end;

    procedure Update(var R: RecordRef)
    var
        F: FieldRef;
    begin
        if R.Number = 0 then
            exit;
        if not R.FieldExist(50) then
            exit;
        F := R.Field(50);
        F.Value := true;
        R.Modify();
    end;

    procedure Unguarded(var R: RecordRef)
    begin
        R.Modify();
    end;

    procedure Rebound(var R: RecordRef)
    var
        L: Record "FG Log";
    begin
        if not R.FieldExist(50) then
            exit;
        R.GetTable(L);
        R.Modify();
    end;
}
"#;

#[test]
fn a_field_guarded_recordref_write_writes_only_tables_with_the_field() {
    let t = tempfile::tempdir().unwrap();
    let ws = t.path().join("ws");
    std::fs::create_dir_all(ws.join("src")).unwrap();
    std::fs::write(
        ws.join("app.json"),
        r#"{"id":"34343434-0000-4000-8000-000000000034","name":"FG","publisher":"t","version":"1.0.0.0"}"#,
    )
    .unwrap();
    std::fs::write(ws.join("src/Mgt.al"), SOURCE).unwrap();
    let answer = |routine: &str, table: &str| -> String {
        let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
            .arg("prove")
            .arg(&ws)
            .args([routine, &format!("writes-table:{table}"), "--deterministic"])
            .env("RUST_LOG", "off")
            .output()
            .unwrap();
        let v: serde_json::Value = serde_json::from_slice(&out.stdout)
            .unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&out.stderr)));
        v["payload"]["result"]["answer"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    };
    // The extension gives `FG Doc` field 50; nothing gives it `FG Log` or
    // `FG Other` (whose number the extension shares).
    assert_eq!(answer("Update", "FG Doc"), "yes");
    assert_eq!(answer("Update", "FG Log"), "no");
    assert_eq!(answer("Update", "FG Other"), "no");
    // Asked by number, the same answers (G40: by number the guard was not
    // read, and `FG Log` answered `unknown`).
    assert_eq!(answer("Update", "50401"), "yes");
    assert_eq!(answer("Update", "50402"), "no");
    // Through the caller that hands it the RecordRef.
    assert_eq!(answer("SetHandled", "FG Log"), "no");
    // Controls: no guard, or a guard the RecordRef is rebound past.
    assert_eq!(answer("Unguarded", "FG Log"), "unknown");
    assert_eq!(answer("Rebound", "FG Log"), "unknown");
}

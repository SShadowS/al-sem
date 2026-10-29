//! An object-id reference (`Database::"T"`, `Page::"P"`, `Codeunit::"C"`) and
//! an enum value's `AsInteger()` are Integer arguments in overload selection.
//!
//! Base Application's `GetIntegrationTableMapping(Mapping, DATABASE::Contact)`
//! over `(…, RecordId)` / `(…, Integer)`, `AddRoleToList(Profiles,
//! Page::"…")` and System Application's `IsValidConnector(Connector.
//! AsInteger())` stayed `ambiguousResolved`: those arguments were untyped.

use std::process::Command;

const SOURCE: &str = r#"table 50420 "DR T" { fields { field(1; K; Integer) { } field(2; "Doc Type"; Enum "DR E") { } } keys { key(PK; K) { } } }
page 50421 "DR P" { SourceTable = "DR T"; }
enum 50422 "DR E" { value(0; A) { } }
codeunit 50423 "DR Lib"
{
    procedure Add(Id: Integer; X: Integer)
    begin
    end;

    procedure Add(Id: Integer; X: Text)
    begin
    end;

    procedure Check(DocType: Enum "DR E")
    begin
    end;

    procedure Check(var Rec: Record "DR T")
    begin
    end;
}
codeunit 50424 "DR Caller"
{
    procedure Go()
    var
        Lib: Codeunit "DR Lib";
        E: Enum "DR E";
        T: Record "DR T";
    begin
        Lib.Add(1, Database::"DR T");
        Lib.Add(1, Page::"DR P");
        Lib.Add(1, Codeunit::"DR Lib");
        Lib.Add(1, E.AsInteger());
        // A value of the field's enum, never a variable.
        Lib.Check(T."Doc Type"::A);
    end;
}
"#;

#[test]
fn object_references_and_as_integer_are_integer_arguments() {
    let t = tempfile::tempdir().unwrap();
    let ws = t.path().join("ws");
    std::fs::create_dir_all(ws.join("src")).unwrap();
    std::fs::write(
        ws.join("app.json"),
        r#"{"id":"36363636-0000-4000-8000-000000000036","name":"DR","publisher":"t","version":"1.0.0.0"}"#,
    )
    .unwrap();
    std::fs::write(ws.join("src/a.al"), SOURCE).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_aldump"))
        .arg("--program-call-graph-stats")
        .arg(&ws)
        .env("RUST_LOG", "off")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "aldump failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("stats JSON");
    let s = &v["primaryScoped"];
    assert_eq!(s["resolvedSource"], 5, "{s}");
    assert_eq!(s["ambiguousResolved"], 0, "{s}");
}

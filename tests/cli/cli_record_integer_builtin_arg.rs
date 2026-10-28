//! A record's `FieldNo(..)` / `Count()` as an argument is an Integer, so it
//! picks the `Integer` overload.
//!
//! Base Application's `RetenPolAllowedTables.AddAllowedTable(Database::"…",
//! Log.FieldNo(SystemCreatedAt))` has two 2-parameter overloads, `(Integer,
//! Integer)` and `(Integer, JsonArray)`. Argument typing knew record
//! built-ins only as `Text`/`RecordId`/`Guid`/`DateTime`, so the call stayed
//! `ambiguousResolved` (Continia DO: 12 such dependency sites).

use std::process::Command;

const SOURCE: &str = r#"table 50320 "RI T"
{
    fields { field(1; K; Integer) { } field(2; Stamp; DateTime) { } }
    keys { key(PK; K) { } }
}
codeunit 50321 "RI Lib"
{
    procedure Add(TableId: Integer; DateFieldNo: Integer)
    begin
    end;

    procedure Add(TableId: Integer; Filters: JsonArray)
    begin
    end;
}
codeunit 50322 "RI Caller"
{
    procedure Go()
    var
        T: Record "RI T";
        Lib: Codeunit "RI Lib";
    begin
        Lib.Add(Database::"RI T", T.FieldNo(Stamp));
        Lib.Add(Database::"RI T", T.Count());
    end;
}
"#;

#[test]
fn a_record_integer_builtin_argument_picks_the_integer_overload() {
    let t = tempfile::tempdir().unwrap();
    let ws = t.path().join("ws");
    std::fs::create_dir_all(ws.join("src")).unwrap();
    std::fs::write(
        ws.join("app.json"),
        r#"{"id":"30303030-0000-4000-8000-000000000030","name":"RI","publisher":"t","version":"1.0.0.0"}"#,
    )
    .unwrap();
    std::fs::write(ws.join("src/Caller.al"), SOURCE).unwrap();
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
    // Both `Add` calls bind to source; `FieldNo` and `Count` are catalog.
    assert_eq!(s["resolvedSource"], 2, "{s}");
    assert_eq!(s["ambiguousResolved"], 0, "{s}");
    assert_eq!(s["unknown"], 0, "{s}");
}

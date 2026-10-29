//! A report's dataitem passed by name is a record of its source table in
//! overload selection.
//!
//! Base Application's Replan Production Order report calls
//! `ReservMgt.SetReservSource("Prod. Order Component")`, the dataitem, over
//! `(Variant)` / `(RecordRef)`. The name is no variable, so the argument was
//! untyped and the call stayed `ambiguousResolved`; a record rules
//! `RecordRef` out, so the Variant overload binds.

use std::process::Command;

const SOURCE: &str = r#"table 50450 "DI T" { fields { field(1; K; Integer) { } } keys { key(PK; K) { } } }
codeunit 50451 "DI Lib"
{
    procedure SetSource(V: Variant)
    begin
    end;

    procedure SetSource(R: RecordRef)
    begin
    end;
}
report 50452 "DI R"
{
    dataset
    {
        dataitem("DI Line"; "DI T")
        {
        }
    }

    trigger OnPreReport()
    var
        Lib: Codeunit "DI Lib";
    begin
        Lib.SetSource("DI Line");
    end;
}
"#;

#[test]
fn a_dataitem_name_argument_is_a_record() {
    let t = tempfile::tempdir().unwrap();
    let ws = t.path().join("ws");
    std::fs::create_dir_all(ws.join("src")).unwrap();
    std::fs::write(
        ws.join("app.json"),
        r#"{"id":"41414141-0000-4000-8000-000000000041","name":"DI","publisher":"t","version":"1.0.0.0"}"#,
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
    assert_eq!(s["resolvedSource"], 1, "{s}");
    assert_eq!(s["ambiguousResolved"], 0, "{s}");
}

//! An array parameter takes part in overload selection.
//!
//! Base Application's `FormatAddress.VendorRemitToAddress` has
//! `(var Record "Remit Address"; var array[8] of Text[100])` and
//! `(var Record "Remit Address"; var Record "Remit Address Buffer")`. Argument
//! typing read `array[8] of Text[100]` as a malformed generic clause (`of` with
//! no `[..]`), so the array parameter was untyped and every call of the name
//! stayed `ambiguousResolved`, the record one included (Continia DO: 8 such
//! dependency sites).

use std::process::Command;

const SOURCE: &str = r#"table 50330 "AR A" { fields { field(1; K; Integer) { } } keys { key(PK; K) { } } }
table 50331 "AR B" { fields { field(1; K; Integer) { } } keys { key(PK; K) { } } }
codeunit 50332 "AR Lib"
{
    procedure Fmt(var A: Record "AR A"; var Addr: array[8] of Text[100])
    begin
    end;

    procedure Fmt(var A: Record "AR A"; var B: Record "AR B")
    begin
    end;

    procedure AddHeader(Name: Text; Value: Text)
    begin
    end;

    procedure AddHeader(Name: Text; Value: SecretText)
    begin
    end;
}
codeunit 50333 "AR Caller"
{
    procedure ToArray()
    var
        A: Record "AR A";
        Lib: Codeunit "AR Lib";
        Addr: array[8] of Text[100];
    begin
        Lib.Fmt(A, Addr);
    end;

    procedure ToRecord()
    var
        A: Record "AR A";
        B: Record "AR B";
        Lib: Codeunit "AR Lib";
    begin
        Lib.Fmt(A, B);
    end;

    procedure ToHeader()
    var
        Lib: Codeunit "AR Lib";
        Headers: array[2, 2] of Text;
    begin
        // An array element is its element type, Text: the exact match.
        Lib.AddHeader(Headers[1, 1], Headers[1, 2]);
    end;
}
"#;

#[test]
fn an_array_parameter_takes_part_in_overload_selection() {
    let t = tempfile::tempdir().unwrap();
    let ws = t.path().join("ws");
    std::fs::create_dir_all(ws.join("src")).unwrap();
    std::fs::write(
        ws.join("app.json"),
        r#"{"id":"31313131-0000-4000-8000-000000000031","name":"AR","publisher":"t","version":"1.0.0.0"}"#,
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
    assert_eq!(s["resolvedSource"], 3, "{s}");
    assert_eq!(s["ambiguousResolved"], 0, "{s}");
}

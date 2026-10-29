//! A table extension's method shadows a same-signature method of the base
//! table, for calls from the extension's own app. alc 18.0.43 binds such a
//! call to the extension (warning AL0523 "already defines a method … with the
//! same parameter types"), and reports AL0196 "The call is ambiguous" only
//! from a third app that sees both. Continia Approvals' page extensions call
//! `Rec.GetSalesPurchName()` on Continia Business Foundation's `CTS-CBF
//! Continia User Setup`, which both the table and Approvals' own table
//! extension declare `internal`; the engine left both calls ambiguous.

use std::process::Command;

const BASE_GUID: &str = "46464646-0000-4000-8000-000000000046";

const BASE_SOURCE: &str = r#"table 50600 "OE Setup"
{
    fields { field(1; Code; Code[20]) { } }
    keys { key(PK; Code) { } }

    procedure GetName(): Text[250]
    begin
        exit('base');
    end;
}
"#;

const WORKSPACE_SOURCE: &str = r#"tableextension 50610 "OE Setup Ext" extends "OE Setup"
{
    procedure GetName(): Text[250]
    begin
        exit('ext');
    end;
}
codeunit 50611 "OE Caller"
{
    procedure Call(var Setup: Record "OE Setup"): Text
    begin
        exit(Setup.GetName());
    end;
}
"#;

#[test]
fn a_call_from_the_extensions_own_app_binds_its_extension_method() {
    let t = tempfile::tempdir().unwrap();
    let ws = t.path().join("ws");
    std::fs::create_dir_all(ws.join("src")).unwrap();
    std::fs::write(
        ws.join("app.json"),
        format!(
            r#"{{"id":"47474747-0000-4000-8000-000000000047","name":"OE","publisher":"t","version":"1.0.0.0",
            "dependencies":[{{"id":"{BASE_GUID}","name":"OEBase","publisher":"probe","version":"1.0.0.0"}}]}}"#
        ),
    )
    .unwrap();
    std::fs::write(ws.join("src/a.al"), WORKSPACE_SOURCE).unwrap();
    crate::symbol_app::write_source_app(
        &ws.join(".alpackages/probe_OEBase_1.0.0.0.app"),
        BASE_GUID,
        "OEBase",
        "1.0.0.0",
        r#"{"Tables":[{"Id":50600,"Name":"OE Setup","Methods":[{"Name":"GetName","ReturnTypeDefinition":{"Name":"Text","Subtype":{"Name":"250"}}}]}]}"#,
        &[("src/OESetup.Table.al", BASE_SOURCE)],
        "",
    );
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
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let p = &v["primaryScoped"];
    assert_eq!(p["ambiguousResolved"], 0, "{p}");
    assert_eq!(p["unknown"], 0, "{p}");
    assert_eq!(p["resolvedSource"], 1, "{p}");
}

//! `Page.Run(0, Rec)` on a dependency's table runs that table's lookup page.
//!
//! Continia DO's `Page.Run(0, TempServDocLog)` is on Base Application's
//! `Service Document Log` (`LookupPageID = "Service Document Log"`). Only a
//! workspace table's `LookupPageId` was known, so the run on a dependency table
//! was an unresolved dynamic `Page.Run`, and `prove` reported it as an
//! `unresolved-callsite`.

use std::process::Command;

const DEP_GUID: &str = "32323232-0000-4000-8000-000000000032";

const DEP_SYMBOLS: &str = r#"{"AppId":"32323232-0000-4000-8000-000000000032","Name":"DepLog","Publisher":"probe","Version":"1.0.0.0",
"Tables":[{"Id":50340,"Name":"Dep Log"}],"Pages":[{"Id":50341,"Name":"Dep Log List"}]}"#;

const DEP_SOURCE: &str = r#"table 50340 "Dep Log"
{
    LookupPageID = "Dep Log List";
    fields { field(1; K; Integer) { } }
    keys { key(PK; K) { } }
}
page 50341 "Dep Log List"
{
    PageType = List;
    SourceTable = "Dep Log";

    trigger OnOpenPage()
    begin
        Message('open');
    end;
}
"#;

const WS: &str = r#"codeunit 50350 "Ws Log Viewer"
{
    procedure Show()
    var
        TempLog: Record "Dep Log" temporary;
    begin
        Page.Run(0, TempLog);
    end;
}
"#;

#[test]
fn a_zero_page_run_on_a_dependency_table_runs_its_lookup_page() {
    let t = tempfile::tempdir().unwrap();
    let ws = t.path().join("ws");
    std::fs::create_dir_all(ws.join("src")).unwrap();
    std::fs::write(
        ws.join("app.json"),
        format!(
            r#"{{"id":"33333333-0000-4000-8000-000000000033","name":"W","publisher":"t","version":"1.0.0.0",
            "dependencies":[{{"id":"{DEP_GUID}","name":"DepLog","publisher":"probe","version":"1.0.0.0"}}]}}"#
        ),
    )
    .unwrap();
    std::fs::write(ws.join("src/Viewer.al"), WS).unwrap();
    crate::symbol_app::write_source_app(
        &ws.join(".alpackages/probe_DepLog_1.0.0.0.app"),
        DEP_GUID,
        "DepLog",
        "1.0.0.0",
        DEP_SYMBOLS,
        &[("src/DepLog.al", DEP_SOURCE)],
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
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("stats JSON");
    let s = &v["primaryScoped"];
    // The one run reaches the dependency page's code.
    assert_eq!(s["total"], 1, "{s}");
    assert_eq!(s["resolvedSource"], 1, "{s}");
}

//! A table declared `ObsoleteState = Moved` binds no reference while the table
//! it moved to is visible.
//!
//! Base Application keeps `table 308 "No. Series"` as a `Moved` stub (`MovedTo`
//! Business Foundation) under `#if not CLEANSCHEMA27`, so for every app seeing
//! both, the name resolved to two tables: ambiguous, and `Record "No. Series"`
//! had no table (Continia CDN's `InvalidCharsInNoSeries(NoSeriesRec)` over
//! `(Code[20])` / `(Record "No. Series")` stayed `ambiguousResolved`).

use std::path::Path;
use std::process::Command;

const OLD_GUID: &str = "38383838-0000-4000-8000-000000000038";
const NEW_GUID: &str = "39393939-0000-4000-8000-000000000039";

const OLD_SOURCE: &str = r#"table 60000 "MV T"
{
    ObsoleteState = Moved;
    ObsoleteReason = 'Moved to New';
    MovedTo = '39393939-0000-4000-8000-000000000039';
    fields { field(1; Code; Code[20]) { } }
    keys { key(PK; Code) { } }
}
"#;

const NEW_SOURCE: &str = r#"table 60000 "MV T"
{
    fields { field(1; Code; Code[20]) { } }
    keys { key(PK; Code) { } }
}
"#;

const WS: &str = r#"codeunit 50440 "MV Lib"
{
    procedure Check(Code: Code[20])
    begin
    end;

    procedure Check(var Rec: Record "MV T")
    begin
    end;
}
codeunit 50441 "MV Caller"
{
    procedure Go()
    var
        Lib: Codeunit "MV Lib";
        Rec: Record "MV T";
    begin
        Lib.Check(Rec);
    end;
}
"#;

fn symbols(guid: &str, name: &str) -> String {
    format!(
        r#"{{"AppId":"{guid}","Name":"{name}","Publisher":"probe","Version":"1.0.0.0","Tables":[{{"Id":60000,"Name":"MV T"}}]}}"#
    )
}

fn app(ws: &Path, guid: &str, name: &str, source: &str) {
    crate::symbol_app::write_source_app(
        &ws.join(format!(".alpackages/probe_{name}_1.0.0.0.app")),
        guid,
        name,
        "1.0.0.0",
        &symbols(guid, name),
        &[("src/T.Table.al", source)],
        "",
    );
}

#[test]
fn a_moved_table_gives_way_to_the_table_it_moved_to() {
    let t = tempfile::tempdir().unwrap();
    let ws = t.path().join("ws");
    std::fs::create_dir_all(ws.join("src")).unwrap();
    std::fs::write(
        ws.join("app.json"),
        format!(
            r#"{{"id":"40404040-0000-4000-8000-000000000040","name":"MV","publisher":"t","version":"1.0.0.0",
            "dependencies":[{{"id":"{OLD_GUID}","name":"Old","publisher":"probe","version":"1.0.0.0"}},
                            {{"id":"{NEW_GUID}","name":"New","publisher":"probe","version":"1.0.0.0"}}]}}"#
        ),
    )
    .unwrap();
    std::fs::write(ws.join("src/a.al"), WS).unwrap();
    app(&ws, OLD_GUID, "Old", OLD_SOURCE);
    app(&ws, NEW_GUID, "New", NEW_SOURCE);
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

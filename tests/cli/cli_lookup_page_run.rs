//! `Page.Run(0, Rec)` runs the lookup page of `Rec`'s table: its
//! `LookupPageId`, or the one a table extension sets. With none, the platform
//! shows a page of its own, which runs no AL code. Either way the run was a
//! dynamic one (`Page.Run(<dynamic>)`) that `prove` blocked on (Continia DO:
//! `CDO E-Mail Log.ShowLog`).

use std::path::Path;
use std::process::Command;

const SOURCE: &str = r#"table 50410 "LP Log"
{
    fields { field(1; Code; Code[20]) { } }
    keys { key(PK; Code) { } }

    procedure ShowLog()
    begin
        Page.Run(0, Rec);
    end;
}
table 50411 "LP Entry"
{
    LookupPageId = "LP Entry List";
    fields { field(1; Code; Code[20]) { } }
    keys { key(PK; Code) { } }

    procedure ShowEntries()
    begin
        Page.Run(0, Rec);
    end;
}
page 50412 "LP Entry List"
{
    PageType = List;
    SourceTable = "LP Entry";

    trigger OnOpenPage()
    var
        Log: Record "LP Log";
    begin
        Log.Code := 'X';
        Log.Insert();
    end;
}
"#;

fn alsem(cwd: &Path, args: &[&str]) -> serde_json::Value {
    let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .args(args)
        .current_dir(cwd)
        .env("RUST_LOG", "off")
        .output()
        .unwrap();
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "{args:?}: {e}; stderr: {}",
            String::from_utf8_lossy(&out.stderr)
        )
    })
}

#[test]
fn page_run_zero_runs_the_tables_lookup_page() {
    let t = tempfile::tempdir().unwrap();
    let ws = t.path().join("ws");
    std::fs::create_dir_all(ws.join("src")).unwrap();
    std::fs::write(
        ws.join("app.json"),
        r#"{"id":"41414141-0000-4000-8000-000000000041","name":"LP","publisher":"t","version":"1.0.0.0"}"#,
    )
    .unwrap();
    std::fs::write(ws.join("src/a.al"), SOURCE).unwrap();
    let inv = alsem(
        t.path(),
        &["fingerprint", "ws", "--inventory-only", "--format", "json"],
    );
    let prove = |name: &str| -> serde_json::Value {
        let id = inv["payload"]["routineInventory"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["routineName"] == name)
            .unwrap_or_else(|| panic!("precondition: {name} is in the inventory"))
            ["stableRoutineId"]
            .as_str()
            .unwrap()
            .to_string();
        alsem(
            t.path(),
            &["prove", "ws", &id, "writes-table:LP Log", "--deterministic"],
        )["payload"]["result"]
            .clone()
    };
    // The lookup page's OnOpenPage writes the log.
    let entries = prove("ShowEntries");
    assert_eq!(entries["answer"], "yes", "{entries}");
    // No lookup page: no AL code runs.
    let log = prove("ShowLog");
    assert_eq!(log["answer"], "no", "{log}");
}

//! `writes-table` soundness on two shapes Base Application is full of:
//! - a record declared with a namespace-qualified type (`Record Ns."T"`) writes
//!   that table. The model took the whole dotted path as the table name, so the
//!   write named no table, and `writes-table:T` answered `no`;
//! - a write whose table is not known (a RecordRef opened on a computed
//!   number) may write any table: it rules out `no` (`unknown-table-write`).
//!   A RecordRef only ever opened temporary writes no table.

use std::path::Path;
use std::process::Command;

const SOURCE: &str = r#"namespace My.Ns;

table 50370 "NS Entry"
{
    fields { field(1; Code; Code[20]) { } }
    keys { key(PK; Code) { } }
}
codeunit 50372 "NS Work"
{
    procedure Qualified()
    var
        E: Record My.Ns."NS Entry";
    begin
        E.Modify();
    end;

    procedure AnyTable(TableNo: Integer)
    var
        RecRef: RecordRef;
    begin
        RecRef.Open(TableNo);
        RecRef.Modify();
    end;

    procedure AnyTempTable(TableNo: Integer)
    var
        RecRef: RecordRef;
    begin
        RecRef.Open(TableNo, true);
        RecRef.Modify();
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
fn qualified_and_unknown_table_writes_rule_out_no() {
    let t = tempfile::tempdir().unwrap();
    let ws = t.path().join("ws");
    std::fs::create_dir_all(ws.join("src")).unwrap();
    std::fs::write(
        ws.join("app.json"),
        r#"{"id":"37373737-0000-4000-8000-000000000037","name":"NS","publisher":"t","version":"1.0.0.0","runtime":"14.0"}"#,
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
            &[
                "prove",
                "ws",
                &id,
                "writes-table:NS Entry",
                "--deterministic",
            ],
        )["payload"]["result"]
            .clone()
    };
    let kinds = |r: &serde_json::Value| -> Vec<String> {
        r["blockedBy"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|b| b["kind"].as_str().map(str::to_string))
            .collect()
    };

    let q = prove("Qualified");
    assert_eq!(q["answer"], "yes", "the qualified record's table: {q}");

    let any = prove("AnyTable");
    assert_eq!(any["answer"], "unknown", "{any}");
    assert!(
        kinds(&any).contains(&"unknown-table-write".to_string()),
        "{any}"
    );

    let temp = prove("AnyTempTable");
    assert_eq!(temp["answer"], "no", "a temporary RecordRef: {temp}");
}

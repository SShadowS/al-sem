//! G17 / G17b: `alsem prove … writes-table:<T>` and temporary records.
//!
//! A write to a temporary record writes no table. `prove` matched every DB write
//! by table alone, so Continia DO's `TempEntry.Insert()` (declared `temporary`)
//! and `Rec.Modify()` on a `SourceTableTemporary` page answered `yes`, a
//! confident contradiction of a correct "writes no table" claim. The digest
//! already carries each write's `tempState` (#65), joined in the ROOT's frame, so
//! a caller passing a temporary record into a `var` parameter sees `known(true)`;
//! asked of the callee itself the state is the caller's, which is neither a yes
//! nor a no.
//!
//! Every routine of one temp workspace is asked through the real binary.

use std::path::Path;
use std::process::Command;

const SOURCE: &str = r#"table 50170 "G17 T"
{
    fields { field(1; Code; Code[20]) { } }
    keys { key(PK; Code) { } }
}
page 50171 "G17 Temp Page"
{
    SourceTable = "G17 T";
    SourceTableTemporary = true;

    trigger OnOpenPage()
    begin
        Rec.Modify();
    end;
}
codeunit 50172 "G17 Writes"
{
    procedure TempVariable()
    var
        TempT: Record "G17 T" temporary;
    begin
        TempT.Code := 'X';
        TempT.Insert();
    end;

    procedure ParamWrite(var T: Record "G17 T")
    begin
        T.Insert();
    end;

    procedure CallerPassesTemporary()
    var
        TempT: Record "G17 T" temporary;
    begin
        ParamWrite(TempT);
    end;

    procedure CallerPassesPersistent()
    var
        T: Record "G17 T";
    begin
        ParamWrite(T);
    end;

    procedure Persistent()
    var
        T: Record "G17 T";
    begin
        T.Code := 'X';
        T.Insert();
    end;

    // G19: a rename writes the row (its primary key).
    procedure RenamePersistent()
    var
        T: Record "G17 T";
    begin
        T.Get('A');
        T.Rename('B');
    end;

    procedure RenameTemporary()
    var
        TempT: Record "G17 T" temporary;
    begin
        TempT.Get('A');
        TempT.Rename('B');
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
fn writes_table_skips_temporary_records() {
    let t = tempfile::tempdir().unwrap();
    let ws = t.path().join("ws");
    std::fs::create_dir_all(ws.join("src")).unwrap();
    std::fs::write(
        ws.join("app.json"),
        r#"{"id":"17171717-0000-4000-8000-000000000017","name":"G17","publisher":"t","version":"1.0.0.0"}"#,
    )
    .unwrap();
    std::fs::write(ws.join("src/Writes.al"), SOURCE).unwrap();

    let inv = alsem(
        t.path(),
        &["fingerprint", "ws", "--inventory-only", "--format", "json"],
    );
    let id_of = |name: &str| -> String {
        inv["payload"]["routineInventory"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["routineName"] == name)
            .unwrap_or_else(|| panic!("precondition: routine {name} is in the inventory"))
            ["stableRoutineId"]
            .as_str()
            .unwrap()
            .to_string()
    };
    let prove = |name: &str| -> serde_json::Value {
        alsem(
            t.path(),
            &[
                "prove",
                "ws",
                &id_of(name),
                "writes-table:G17 T",
                "--deterministic",
            ],
        )["payload"]["result"]
            .clone()
    };

    // The controls: a persistent write, and one through the parameter.
    for routine in ["Persistent", "CallerPassesPersistent", "RenamePersistent"] {
        let r = prove(routine);
        assert_eq!(r["answer"], "yes", "{routine} writes the table: {r}");
    }
    // Temporary on every path: no table is written.
    for routine in [
        "TempVariable",
        "OnOpenPage",
        "CallerPassesTemporary",
        "RenameTemporary",
    ] {
        let r = prove(routine);
        assert_eq!(
            r["answer"], "no",
            "{routine} writes only a temporary record: {r}"
        );
    }
    // The callee: it depends on the caller.
    let r = prove("ParamWrite");
    assert_eq!(
        r["answer"], "unknown",
        "the callee depends on its caller: {r}"
    );
    let kinds: Vec<&str> = r["blockedBy"]
        .as_array()
        .unwrap_or_else(|| panic!("no blockers: {r}"))
        .iter()
        .filter_map(|b| b["kind"].as_str())
        .collect();
    assert!(
        kinds.contains(&"temp-state-depends-on-caller"),
        "the blocker must say why: {kinds:?}"
    );
}

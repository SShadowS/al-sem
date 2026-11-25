//! ENG-192: record_types' temp pass (a RecordRef `Open`ed in a routine without branching) reads
//! each record operation's temp state from the LAST bind of its RecordRef before it. One state per
//! variable gave a later `Open`'s state to the operations before it, across a `Close`, a `Clear`
//! or a `System.Clear` (Fable's reviews of ENG-191: a wrong `no` on master), kept an earlier state
//! past an `Open` whose temporary flag it could not read, and ignored an `Open` inside a loop,
//! which reaches the loop's earlier operations on the next iteration.

use std::path::Path;
use std::process::Command;

const SOURCE: &str = r#"table 50002 "PA Log"
{
    fields { field(1; Code; Code[20]) { } }
    keys { key(PK; Code) { } }
}
codeunit 50010 "PA"
{
    // Control: one physical Open.
    procedure RootPhysical()
    var
        R: RecordRef;
    begin
        R.Open(Database::"PA Log");
        R.Insert();
    end;

    // Control: one temporary Open, so the Insert writes no table.
    procedure RootTempOnly()
    var
        R: RecordRef;
    begin
        R.Open(Database::"PA Log", true);
        R.Insert();
    end;

    // The physical Insert comes before the temporary Open.
    procedure RootClose()
    var
        R: RecordRef;
    begin
        R.Open(Database::"PA Log");
        R.Insert();
        R.Close();
        R.Open(Database::"PA Log", true);
    end;

    procedure RootClear()
    var
        R: RecordRef;
    begin
        R.Open(Database::"PA Log");
        R.Insert();
        Clear(R);
        R.Open(Database::"PA Log", true);
    end;

    procedure RootSysClear()
    var
        R: RecordRef;
    begin
        R.Open(Database::"PA Log");
        R.Insert();
        System.Clear(R);
        R.Open(Database::"PA Log", true);
    end;

    // The last Open's flag is unknown: the Insert may be physical.
    procedure RootUnknownFlag(Flag: Boolean)
    var
        R: RecordRef;
    begin
        R.Open(Database::"PA Log", true);
        R.Close();
        R.Open(Database::"PA Log", Flag);
        R.Insert();
    end;

    // A bind's own arguments run before it: the Insert runs on the physical Open (Fable r1).
    procedure RootArgBind()
    var
        R: RecordRef;
    begin
        R.Open(Database::"PA Log");
        R.Open(Database::"PA Log", true, Co(R.Insert()));
        R.Close();
        R.Open(Database::"PA Log");
    end;

    // The temporary Open after the error never runs (Fable r1).
    procedure RootAssertError()
    var
        R: RecordRef;
    begin
        R.Open(Database::"PA Log");
        asserterror begin
            Error('x');
            R.Close();
            R.Open(Database::"PA Log", true);
        end;
        R.Insert();
        R.Close();
        R.Open(Database::"PA Log");
    end;

    // A GetTable of a record whose temp state is unknown (a parameter).
    procedure RootGetTableUnknown(var P: Record "PA Log")
    var
        R: RecordRef;
    begin
        R.Open(Database::"PA Log", true);
        R.Close();
        R.GetTable(P);
        R.Insert();
    end;

    // The Insert runs on the physical record's table, before the temporary GetTable.
    procedure RootGetTableFlip()
    var
        R: RecordRef;
        T: Record "PA Log" temporary;
        P: Record "PA Log";
    begin
        R.GetTable(P);
        R.Insert();
        R.GetTable(T);
    end;

    // The Insert runs on the temporary Open before the physical one: no write (master: yes).
    procedure RootTempFirst()
    var
        R: RecordRef;
    begin
        R.Open(Database::"PA Log", true);
        R.Insert();
        R.Close();
        R.Open(Database::"PA Log");
    end;

    procedure Co(B: Boolean): Text[30]
    begin
        exit(CompanyName());
    end;

    // The second iteration's Insert runs on the physical table the loop opened.
    procedure RootLoop()
    var
        R: RecordRef;
        I: Integer;
    begin
        R.Open(Database::"PA Log", true);
        for I := 1 to 2 do begin
            R.Insert();
            R.Close();
            R.Open(Database::"PA Log");
        end;
    end;

    // sol r1: the same through every other loop kind (each fills `loop_stack` on its own path).
    procedure RootWhile()
    var
        R: RecordRef;
        I: Integer;
    begin
        R.Open(Database::"PA Log", true);
        while I < 2 do begin
            R.Insert();
            R.Close();
            R.Open(Database::"PA Log");
            I := I + 1;
        end;
    end;

    procedure RootRepeat()
    var
        R: RecordRef;
        I: Integer;
    begin
        R.Open(Database::"PA Log", true);
        repeat
            R.Insert();
            R.Close();
            R.Open(Database::"PA Log");
            I := I + 1;
        until I >= 2;
    end;

    procedure RootForeach()
    var
        R: RecordRef;
        Items: List of [Integer];
        I: Integer;
    begin
        Items.Add(1);
        Items.Add(2);
        R.Open(Database::"PA Log", true);
        foreach I in Items do begin
            R.Insert();
            R.Close();
            R.Open(Database::"PA Log");
        end;
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

/// Master (c6f61f04) answered `no` for `RootClose`, `RootSysClear`, `RootUnknownFlag` and
/// `RootLoop`. Discrimination (2026-11-24, each break then restored, green): every operation
/// reading the variable's LAST bind (the old one-state rule) fails `RootClose`; an `Open` with an
/// unknown flag pushing no bind (the old skip) fails `RootUnknownFlag`; a variable bound inside a
/// loop kept fails `RootLoop`.
#[test]
fn a_record_operation_reads_the_last_bind_before_it() {
    let t = tempfile::tempdir().unwrap();
    let ws = t.path().join("ws");
    std::fs::create_dir_all(ws.join("src")).unwrap();
    std::fs::write(
        ws.join("app.json"),
        r#"{"id":"19219219-0000-4000-8000-000000000192","name":"E192","publisher":"t","version":"1.0.0.0"}"#,
    )
    .unwrap();
    std::fs::write(ws.join("src/a.al"), SOURCE).unwrap();
    let inv = alsem(
        t.path(),
        &["fingerprint", "ws", "--inventory-only", "--format", "json"],
    );
    let answer = |name: &str| -> String {
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
            &["prove", "ws", &id, "writes-table:PA Log", "--deterministic"],
        )["payload"]["result"]["answer"]
            .as_str()
            .unwrap()
            .to_string()
    };
    assert_eq!(answer("RootTempOnly"), "no", "precondition: the pass runs");
    assert_eq!(answer("RootPhysical"), "yes", "control");
    assert_eq!(answer("RootTempFirst"), "no", "the Insert is temporary");
    // Not `no` (master's wrong answer for `RootSysClear`). Not yet `yes` either: a `Clear` of
    // the variable makes the pass derive nothing for it, though `Clear` only unbinds (BC 28.4:
    // a write after it raises "The record is not open."). ENG-211 makes these `yes`.
    for root in ["RootClear", "RootSysClear"] {
        assert_eq!(answer(root), "unknown", "{root}");
    }
    for root in [
        "RootClose",
        "RootUnknownFlag",
        "RootLoop",
        "RootWhile",
        "RootRepeat",
        "RootForeach",
        "RootArgBind",
        "RootAssertError",
        "RootGetTableUnknown",
        "RootGetTableFlip",
    ] {
        assert_eq!(answer(root), "yes", "{root}: a physical write is reachable");
    }
}

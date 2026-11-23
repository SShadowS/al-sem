//! ENG-189: an in-place bind (`GetTable`, `Open`, `Get`, …) through a RecordRef holder that is
//! RETAINABLE at that point may rebind the record of every other retainable holder, as a place
//! that may run user code may (ENG-159's sound rule). The alias classes cannot join a local made
//! retainable by a call with the call's result or a joined parameter, so `L := Same(R);
//! L.GetTable(Doc)` used to leave `R` on its old table (a wrong `no`; Fable's review of ENG-161,
//! measured on master). BC 28.4: a returned RecordRef is its argument's handle (ENG-175).

use std::path::Path;
use std::process::Command;

const SOURCE: &str = r#"table 50000 "PA Doc"
{
    fields { field(1; Code; Code[20]) { } field(2; Extra; Integer) { } }
    keys { key(PK; Code) { } }
}
table 50001 "PA Entry"
{
    fields { field(1; Code; Code[20]) { } }
    keys { key(PK; Code) { } }
}
table 50002 "PA Log"
{
    fields { field(1; Code; Code[20]) { } }
    keys { key(PK; Code) { } }
}
codeunit 50010 "PA"
{
    procedure RootControlNo()
    var
        R: RecordRef;
        E: Record "PA Entry";
    begin
        R.GetTable(E);
        Guarded(R);
    end;

    procedure RootControlYes()
    var
        R: RecordRef;
        D: Record "PA Doc";
    begin
        R.GetTable(D);
        Guarded(R);
    end;

    // A returned holder is the caller's handle: L shares R's record.
    procedure RootSame()
    var
        R: RecordRef;
        L: RecordRef;
        E: Record "PA Entry";
        D: Record "PA Doc";
    begin
        R.GetTable(E);
        L := Same(R);
        L.GetTable(D);
        Guarded(R);
    end;

    // A callee aliases two arguments.
    procedure RootJoin()
    var
        R: RecordRef;
        L: RecordRef;
        E: Record "PA Entry";
        D: Record "PA Doc";
    begin
        R.GetTable(E);
        Join(L, R);
        L.GetTable(D);
        Guarded(R);
    end;

    // The same through a Variant out parameter.
    procedure RootJoinVar()
    var
        R: RecordRef;
        L: RecordRef;
        V: Variant;
        E: Record "PA Entry";
        D: Record "PA Doc";
    begin
        R.GetTable(E);
        ToVar(V, R);
        L := V;
        L.GetTable(D);
        Guarded(R);
    end;

    // Control: the alias is bound before R is passed anywhere, so R keeps its table.
    procedure RootBindFirst()
    var
        R: RecordRef;
        L: RecordRef;
        E: Record "PA Entry";
        D: Record "PA Doc";
    begin
        L.GetTable(D);
        R.GetTable(E);
        Guarded(R);
    end;

    // Control (BC 28.4, /work/.al-sem/engine-notes/eng189-probe): `FieldRef.Record()` is a COPY,
    // so rebinding it leaves R on its table.
    procedure RootFieldRecord()
    var
        R: RecordRef;
        L: RecordRef;
        F: FieldRef;
        E: Record "PA Entry";
        D: Record "PA Doc";
    begin
        R.GetTable(E);
        F := R.Field(1);
        L := F.Record();
        L.GetTable(D);
        Guarded(R);
    end;

    // G36: the write behind `FieldExist(2)` reaches only a table with field 2, unless the
    // record is rebound after the guard through an alias (`L` shares it since `Same(R)`).
    procedure FieldAfterBind(TableNo: Integer)
    var
        R: RecordRef;
        L: RecordRef;
        D: Record "PA Doc";
    begin
        R.Open(TableNo);
        L := Same(R);
        if not R.FieldExist(2) then
            exit;
        L.GetTable(D);
        R.Modify();
    end;

    procedure FieldNoBind(TableNo: Integer)
    var
        R: RecordRef;
        L: RecordRef;
    begin
        R.Open(TableNo);
        L := Same(R);
        if not R.FieldExist(2) then
            exit;
        R.Modify();
    end;

    local procedure Join(var A: RecordRef; B: RecordRef)
    begin
        A := B;
    end;

    local procedure ToVar(var V: Variant; B: RecordRef)
    begin
        V := B;
    end;

    local procedure Same(X: RecordRef): RecordRef
    begin
        exit(X);
    end;

    local procedure Guarded(var R: RecordRef)
    var
        Log: Record "PA Log";
    begin
        if R.Number <> Database::"PA Doc" then
            exit;
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

/// Discrimination (2026-11-23): the RecordRef flow's facts without the kept binds
/// (`recref_alias::kept_bind_points` not added to its user-code places) fail `RootSame`,
/// `RootJoin` and `RootJoinVar` (`no` again); G36 (`field_guard`) without them fails
/// `FieldAfterBind` (`no`). Each restored, passes. A kept bind rebinding its own receiver's class
/// too (no `UserCodePoint::except`) fails `cli_recref_get`'s `ShadowAliasFlow`.
#[test]
fn an_in_place_bind_through_a_kept_holder_rebinds_every_kept_holder() {
    let t = tempfile::tempdir().unwrap();
    let ws = t.path().join("ws");
    std::fs::create_dir_all(ws.join("src")).unwrap();
    std::fs::write(
        ws.join("app.json"),
        r#"{"id":"18918918-0000-4000-8000-000000000189","name":"E189","publisher":"t","version":"1.0.0.0"}"#,
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
    assert_eq!(answer("RootControlNo"), "no", "precondition");
    assert_eq!(answer("RootControlYes"), "yes", "precondition");
    assert_eq!(answer("RootBindFirst"), "no", "control");
    assert_eq!(answer("RootFieldRecord"), "no", "control (BC: a copy)");
    for root in ["RootSame", "RootJoin", "RootJoinVar"] {
        assert_eq!(answer(root), "yes", "{root}");
    }
    assert_eq!(answer("FieldNoBind"), "no", "precondition");
    assert_ne!(answer("FieldAfterBind"), "no");
}

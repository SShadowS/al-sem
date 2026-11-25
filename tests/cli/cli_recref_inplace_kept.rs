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

    // Controls (sol r1; BC 28.4, the module's contract): `Close` detaches only its own variable,
    // and `Open` needs a closed one, so neither reaches R through the kept alias.
    procedure RootClose()
    var
        R: RecordRef;
        L: RecordRef;
        E: Record "PA Entry";
    begin
        R.GetTable(E);
        L := Same(R);
        L.Close();
        Guarded(R);
    end;

    procedure RootCloseOpen()
    var
        R: RecordRef;
        L: RecordRef;
        E: Record "PA Entry";
    begin
        R.GetTable(E);
        L := Same(R);
        L.Close();
        L.Open(Database::"PA Doc");
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

    // The only alias the classes see (`A := R`) is in the other branch, so the in-place rule
    // leaves R out; the bind through A (joined to R by `Join`) still rebinds R (Fable r1, F1).
    procedure RootExceptExclusive(C: Boolean)
    var
        R: RecordRef;
        A: RecordRef;
        E: Record "PA Entry";
        D: Record "PA Doc";
    begin
        R.GetTable(E);
        Join(A, R);
        if C then
            A := R
        else
            A.GetTable(D);
        Guarded(R);
    end;

    // An array element joined to R by a call: the classes hold no array slots (F2).
    procedure RootSlot()
    var
        R: RecordRef;
        Slot: array[2] of RecordRef;
        E: Record "PA Entry";
        D: Record "PA Doc";
    begin
        R.GetTable(E);
        Join(Slot[1], R);
        Slot[1].GetTable(D);
        Guarded(R);
    end;

    // The receiver joined inside the bind's own arguments (F4).
    procedure RootJoinInArgs()
    var
        R: RecordRef;
        L: RecordRef;
        E: Record "PA Entry";
    begin
        R.GetTable(E);
        L.Get(JoinId(L, R));
        Guarded(R);
    end;

    // A `Get` bind (a record operation, not a call site).
    procedure RootGet(Id: RecordId)
    var
        R: RecordRef;
        L: RecordRef;
        E: Record "PA Entry";
    begin
        R.GetTable(E);
        L := Same(R);
        L.Get(Id);
        Guarded(R);
    end;

    // Control: R is retainable at the bind, L never is, so binding L moves nothing.
    procedure RootReceiverNotKept()
    var
        R: RecordRef;
        L: RecordRef;
        E: Record "PA Entry";
        D: Record "PA Doc";
    begin
        R.GetTable(E);
        Nop(R);
        L.GetTable(D);
        Guarded(R);
    end;

    // Control: a `Get` on an element of a RECORD array binds no RecordRef (Fable r2).
    procedure RootRecArrayGet()
    var
        R: RecordRef;
        Recs: array[2] of Record "PA Doc";
        E: Record "PA Entry";
    begin
        R.GetTable(E);
        Nop(R);
        if Recs[1].Get('X') then;
        Guarded(R);
    end;

    // G36: the write behind `FieldExist(2)` reaches only a table with field 2, unless the
    // record is rebound after the guard through an alias (`L` shares it since `Same(R)`);
    // PA Entry has no field 2.
    procedure FieldAfterBind(TableNo: Integer)
    var
        R: RecordRef;
        L: RecordRef;
        E: Record "PA Entry";
    begin
        R.Open(TableNo);
        L := Same(R);
        if not R.FieldExist(2) then
            exit;
        L.GetTable(E);
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

    local procedure JoinId(var A: RecordRef; B: RecordRef): RecordId
    begin
        A := B;
        exit(B.RecordId);
    end;

    local procedure Nop(X: RecordRef)
    begin
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
/// `FieldAfterBind` (`no`). Each restored, passes. A kept bind rebinding its own receiver too
/// (no `UserCodePoint::except`) fails `cli_recref_get`'s `ShadowAliasFlow`.
/// Fable r1 rows (2026-11-23, each break then restored, green): `rebinds` leaving out the
/// receiver's whole class fails `RootExceptExclusive`; a holder-array element not always counting
/// fails `RootSlot`; retainability read at the bind's start fails `RootJoinInArgs`; no
/// retainability check on the receiver at all fails `RootReceiverNotKept` (`yes`). Fable r2: any
/// array element counting, whatever its type, fails `RootRecArrayGet` (`yes`). sol r1
/// (2026-11-25): `kept_bind_points` keeping `Close` and `Open` fails `RootClose` and
/// `RootCloseOpen` (`unknown`, not `no`).
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
    let answer_for = |name: &str, query: &str| -> String {
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
            &["prove", "ws", &id, query, "--deterministic"],
        )["payload"]["result"]["answer"]
            .as_str()
            .unwrap()
            .to_string()
    };
    let answer = |name: &str| answer_for(name, "writes-table:PA Log");
    assert_eq!(answer("RootControlNo"), "no", "precondition");
    assert_eq!(answer("RootControlYes"), "yes", "precondition");
    assert_eq!(answer("RootBindFirst"), "no", "control");
    assert_eq!(
        answer("RootClose"),
        "no",
        "control (BC: Close detaches only L)"
    );
    assert_eq!(
        answer("RootCloseOpen"),
        "no",
        "control (BC: Open needs a closed L)"
    );
    assert_eq!(answer("RootFieldRecord"), "no", "control (BC: a copy)");
    assert_eq!(answer("RootReceiverNotKept"), "no", "control");
    assert_eq!(answer("RootRecArrayGet"), "no", "control");
    for root in [
        "RootSame",
        "RootJoin",
        "RootJoinVar",
        "RootExceptExclusive",
        "RootSlot",
        "RootJoinInArgs",
        "RootGet",
    ] {
        assert_eq!(answer(root), "yes", "{root}");
    }
    let entry = |name: &str| answer_for(name, "writes-table:PA Entry");
    assert_eq!(entry("FieldNoBind"), "no", "precondition");
    assert_ne!(entry("FieldAfterBind"), "no");
}

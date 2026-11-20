//! ENG-160: `RecordRef.Get(RecordId)` moves the RecordRef to the RecordId's table (measured on
//! BC 28.4: `docs/superpowers/notes/2026-11-20-eng160-recordref-table-moves.md`). In the IR it is a
//! record operation, never a call site, so every reader that looked for a RecordRef's binds among
//! the call sites alone missed it: a guard read the table the RecordRef held before (a false
//! `no`). `recref_alias::RecRefBind` is now the one list, and `recref_alias::bind_sites` hands
//! every reader the call sites and the record operations alike. One shape per reader, each with
//! a control that holds without the `Get`.

use std::collections::BTreeSet;
use std::path::Path;
use std::process::Command;

const SOURCE: &str = r#"table 50430 "GE Doc"
{
    fields { field(1; K; Integer) { } }
    keys { key(PK; K) { } }
}
table 50431 "GE Entry"
{
    fields { field(1; K; Integer) { } field(2; Name; Text[20]) { } }
    keys { key(PK; K) { } }
}
table 50432 "GE Log"
{
    fields { field(1; K; Integer) { } }
    keys { key(PK; K) { } }
}
table 50433 "GE Other"
{
    fields { field(1; K; Integer) { } }
    keys { key(PK; K) { } }
}
table 50436 "GE Link"
{
    fields { field(1; K; Integer) { } field(3; RecordId; RecordId) { } }
    keys { key(PK; K) { } }
}
codeunit 50434 "GE Mgt"
{
    var
        G: RecordRef;
        GId: RecordId;

    // G34/G35 (recref_flow): the guard in Guarded reads what R holds.
    procedure GetMoves()
    var
        R: RecordRef;
        E: Record "GE Entry";
        D: Record "GE Doc";
    begin
        R.GetTable(E);
        D.FindFirst();
        R.Get(D.RecordId);
        Guarded(R);
    end;

    procedure GetControl()
    var
        R: RecordRef;
        E: Record "GE Entry";
    begin
        R.GetTable(E);
        Guarded(R);
    end;

    // sol on ENG-160: a Get through an alias (one handle) moves R too.
    procedure AliasGetRoot()
    var
        R: RecordRef;
        E: Record "GE Entry";
    begin
        R.GetTable(E);
        AliasGet(R);
    end;

    local procedure AliasGet(var R: RecordRef)
    var
        A: RecordRef;
        D: Record "GE Doc";
    begin
        A := R;
        D.FindFirst();
        A.Get(D.RecordId);
        Guarded(R);
    end;

    // sol on ENG-160: a bind through an array slot R was put in moves R (GetTable as well:
    // the IR keeps no right side for an indexed target).
    procedure SlotGetRoot()
    var
        R: RecordRef;
        Slot: array[2] of RecordRef;
        E: Record "GE Entry";
        D: Record "GE Doc";
    begin
        R.GetTable(E);
        Slot[1] := R;
        D.FindFirst();
        Slot[1].Get(D.RecordId);
        Guarded(R);
    end;

    procedure SlotGetTableRoot()
    var
        R: RecordRef;
        Slot: array[2] of RecordRef;
        E: Record "GE Entry";
        D: Record "GE Doc";
    begin
        R.GetTable(E);
        Slot[1] := R;
        Slot[1].GetTable(D);
        Guarded(R);
    end;

    local procedure Guarded(var R: RecordRef)
    var
        Log: Record "GE Log";
    begin
        if R.Number <> Database::"GE Doc" then
            exit;
        Log.Insert();
    end;

    // G34 (exit_guards): a Get before the guard; the guard reads no entry value.
    procedure GetBeforeGuardRoot()
    var
        R: RecordRef;
        E: Record "GE Entry";
    begin
        R.GetTable(E);
        GetBeforeGuard(R);
    end;

    procedure GuardOnlyRoot()
    var
        R: RecordRef;
        E: Record "GE Entry";
    begin
        R.GetTable(E);
        GuardOnly(R);
    end;

    local procedure GetBeforeGuard(var R: RecordRef)
    var
        Log: Record "GE Log";
        D: Record "GE Doc";
    begin
        D.FindFirst();
        R.Get(D.RecordId);
        if R.Number <> Database::"GE Doc" then
            exit;
        Log.Insert();
    end;

    local procedure GuardOnly(var R: RecordRef)
    var
        Log: Record "GE Log";
    begin
        if R.Number <> Database::"GE Doc" then
            exit;
        Log.Insert();
    end;

    // G34 (exit_guards): a local's guard reads its `Unset` entry unless something binds it
    // first: a Get on it (sol on ENG-160), through an alias of it, or through an array slot.
    procedure LocalGet()
    var
        R: RecordRef;
        D: Record "GE Doc";
        L: Record "GE Log";
    begin
        D.FindFirst();
        R.Get(D.RecordId);
        if R.Number <> Database::"GE Doc" then
            exit;
        L.Insert();
    end;

    procedure LocalAliasGet()
    var
        R: RecordRef;
        A: RecordRef;
        D: Record "GE Doc";
        L: Record "GE Log";
    begin
        A := R;
        D.FindFirst();
        A.Get(D.RecordId);
        if R.Number <> Database::"GE Doc" then
            exit;
        L.Insert();
    end;

    procedure LocalSlotGet()
    var
        R: RecordRef;
        Slot: array[2] of RecordRef;
        D: Record "GE Doc";
        L: Record "GE Log";
    begin
        Slot[1] := R;
        D.FindFirst();
        Slot[1].Get(D.RecordId);
        if R.Number <> Database::"GE Doc" then
            exit;
        L.Insert();
    end;

    procedure LocalUnbound()
    var
        R: RecordRef;
        L: Record "GE Log";
    begin
        if R.Number <> Database::"GE Doc" then
            exit;
        L.Insert();
    end;

    // ENG-160: `Id := R.RecordId; …; R.Get(Id)` moves R back to the table the ID was taken from
    // (Continia DO's `RunReportID`): here an Entry, so the guard holds.
    procedure OwnIdRoot()
    var
        R: RecordRef;
        F: RecordRef;
        E: Record "GE Entry";
        Id: RecordId;
    begin
        R.GetTable(E);
        F.GetTable(E);
        Id := R.RecordId;
        R := F.Duplicate();
        R.Get(Id);
        Guarded(R);
    end;

    // The same with R handed to a Variant first (one handle, so one alias class), as
    // RunReportID does: the class takes the ID's sources too.
    procedure OwnIdAliasedRoot()
    var
        R: RecordRef;
        F: RecordRef;
        E: Record "GE Entry";
        V: Variant;
        Id: RecordId;
    begin
        R.GetTable(E);
        F.GetTable(E);
        V := R;
        Id := R.RecordId;
        R := F.Duplicate();
        R.Get(Id);
        Guarded(R);
    end;

    // A record's own ID: its table (GE Entry) has no field named RecordId.
    procedure OwnIdRecordRoot()
    var
        R: RecordRef;
        E: Record "GE Entry";
        E2: Record "GE Entry";
        Id: RecordId;
    begin
        R.GetTable(E);
        E2.FindFirst();
        Id := E2.RecordId;
        R.Get(Id);
        Guarded(R);
    end;

    // Fable on ENG-160: on a record whose table has a FIELD named RecordId, `Link.RecordId` is
    // that field, which may hold a Doc's ID.
    procedure OwnIdFieldRoot()
    var
        R: RecordRef;
        E: Record "GE Entry";
        Link: Record "GE Link";
        Id: RecordId;
    begin
        R.GetTable(E);
        Link.FindFirst();
        Id := Link.RecordId;
        R.Get(Id);
        Guarded(R);
    end;

    // The ID written by a `foreach`: it may be any record's.
    procedure OwnIdForeachRoot()
    var
        R: RecordRef;
        E: Record "GE Entry";
        D: Record "GE Doc";
        Id: RecordId;
        Ids: List of [RecordId];
    begin
        R.GetTable(E);
        D.FindFirst();
        Ids.Add(D.RecordId);
        Id := R.RecordId;
        foreach Id in Ids do begin
            R.Get(Id);
            Guarded(R);
        end;
    end;

    // The ID passed to a call that may write it.
    procedure OwnIdPassedRoot()
    var
        R: RecordRef;
        E: Record "GE Entry";
        Id: RecordId;
    begin
        R.GetTable(E);
        Id := R.RecordId;
        FillDocId(Id);
        R.Get(Id);
        Guarded(R);
    end;

    local procedure FillDocId(var Id: RecordId)
    var
        D: Record "GE Doc";
    begin
        D.FindFirst();
        Id := D.RecordId;
    end;

    // The ID taken from R, then computed by a function.
    procedure OwnIdComputedRoot()
    var
        R: RecordRef;
        E: Record "GE Entry";
        Id: RecordId;
    begin
        R.GetTable(E);
        Id := R.RecordId;
        Id := DocId();
        R.Get(Id);
        Guarded(R);
    end;

    local procedure DocId(): RecordId
    var
        D: Record "GE Doc";
    begin
        D.FindFirst();
        exit(D.RecordId);
    end;

    // A global ID that a called routine rewrites (it is not passed as an argument).
    procedure OwnIdGlobalRoot()
    var
        R: RecordRef;
        E: Record "GE Entry";
    begin
        R.GetTable(E);
        GId := R.RecordId;
        SetGIdDoc();
        R.Get(GId);
        Guarded(R);
    end;

    local procedure SetGIdDoc()
    var
        D: Record "GE Doc";
    begin
        D.FindFirst();
        GId := D.RecordId;
    end;

    // Fable on ENG-160: `R := O` between taking the ID and the Get moves O (one handle) to the
    // caller's Doc.
    procedure OwnIdAliasRoot()
    var
        R: RecordRef;
        D: Record "GE Doc";
    begin
        R.GetTable(D);
        OwnIdAlias(R);
    end;

    local procedure OwnIdAlias(var R: RecordRef)
    var
        O: RecordRef;
        E: Record "GE Entry";
        Id: RecordId;
    begin
        O.GetTable(E);
        Id := R.RecordId;
        R := O;
        R.Get(Id);
        Guarded(O);
    end;

    // G33 (literal_reach's table frame): R takes the Variant's table, unless a Get moves it.
    procedure FrameRoot()
    var
        E: Record "GE Entry";
    begin
        Frame(E);
    end;

    procedure FrameControlRoot()
    var
        E: Record "GE Entry";
    begin
        FrameControl(E);
    end;

    local procedure Frame(V: Variant)
    var
        R: RecordRef;
        Log: Record "GE Log";
        D: Record "GE Doc";
    begin
        R.GetTable(V);
        D.FindFirst();
        R.Get(D.RecordId);
        case R.Number of
            Database::"GE Doc":
                Log.Insert();
        end;
    end;

    local procedure FrameControl(V: Variant)
    var
        R: RecordRef;
        Log: Record "GE Log";
    begin
        R.GetTable(V);
        case R.Number of
            Database::"GE Doc":
                Log.Insert();
        end;
    end;

    // Fable on ENG-160: the frame's R aliased (`A := R`), then moved through the alias.
    procedure FrameAliasRoot()
    var
        E: Record "GE Entry";
    begin
        FrameAlias(E);
    end;

    local procedure FrameAlias(V: Variant)
    var
        R: RecordRef;
        A: RecordRef;
        Log: Record "GE Log";
        D: Record "GE Doc";
    begin
        R.GetTable(V);
        A := R;
        D.FindFirst();
        A.Get(D.RecordId);
        case R.Number of
            Database::"GE Doc":
                Log.Insert();
        end;
    end;

    // The frame's R put in an array slot, then moved through the slot.
    procedure FrameSlotRoot()
    var
        E: Record "GE Entry";
    begin
        FrameSlot(E);
    end;

    local procedure FrameSlot(V: Variant)
    var
        R: RecordRef;
        Slot: array[2] of RecordRef;
        Log: Record "GE Log";
        D: Record "GE Doc";
    begin
        R.GetTable(V);
        Slot[1] := R;
        Slot[1].GetTable(D);
        case R.Number of
            Database::"GE Doc":
                Log.Insert();
        end;
    end;

    // record_types: a local RecordRef only ever opened temporary holds a temporary table,
    // unless a Get moves it to a physical one.
    procedure TempGet(TableNo: Integer; Flag: Boolean)
    var
        R: RecordRef;
        D: Record "GE Doc";
    begin
        if Flag then
            R.Open(TableNo, true)
        else
            R.Open(TableNo, true);
        D.FindFirst();
        R.Get(D.RecordId);
        R.Delete();
    end;

    procedure TempControl(TableNo: Integer; Flag: Boolean)
    var
        R: RecordRef;
    begin
        if Flag then
            R.Open(TableNo, true)
        else
            R.Open(TableNo, true);
        R.Delete();
    end;

    // record_types' first pass (no branching): Open(…, true) derives a temporary table for every
    // operation, unless a Get moves it.
    procedure TempGetStraight()
    var
        R: RecordRef;
        D: Record "GE Doc";
    begin
        R.Open(Database::"GE Other", true);
        D.FindFirst();
        R.Get(D.RecordId);
        R.Delete();
    end;

    // Fable on ENG-160: the Get names the RecordRef quoted.
    procedure TempGetQuoted(TableNo: Integer; Flag: Boolean)
    var
        MyR: RecordRef;
        D: Record "GE Doc";
    begin
        if Flag then
            MyR.Open(TableNo, true)
        else
            MyR.Open(TableNo, true);
        D.FindFirst();
        "MyR".Get(D.RecordId);
        MyR.Delete();
    end;

    // Fable on ENG-160: a global opened temporary here, moved by a routine the call reaches.
    procedure TempGlobal()
    begin
        G.Open(Database::"GE Other", true);
        MoveG();
        G.Delete();
    end;

    local procedure MoveG()
    var
        D: Record "GE Doc";
    begin
        D.FindFirst();
        G.Get(D.RecordId);
    end;

    // The resolver's RecordRef table (`program::recordref_table`): opened on Doc, then moved.
    procedure OpenThenGet()
    var
        R: RecordRef;
        E: Record "GE Entry";
    begin
        R.Open(Database::"GE Doc");
        E.FindFirst();
        R.Get(E.RecordId);
        R.Delete();
    end;

    procedure OpenControl()
    var
        R: RecordRef;
    begin
        R.Open(Database::"GE Doc");
        R.Delete();
    end;

    // `field_touches`: the table a local RecordRef's `Field(n)` reads.
    procedure TouchAfterGet()
    var
        R: RecordRef;
        E: Record "GE Entry";
        F: FieldRef;
    begin
        R.Open(Database::"GE Doc");
        E.FindFirst();
        R.Get(E.RecordId);
        F := R.Field(2);
        F.Value := 'x';
    end;

    procedure TouchControl()
    var
        R: RecordRef;
        F: FieldRef;
    begin
        R.Open(Database::"GE Doc");
        F := R.Field(2);
        F.Value := 'x';
    end;

    // Fable on ENG-160: moved through an alias (`A := R`).
    procedure TouchAlias()
    var
        R: RecordRef;
        A: RecordRef;
        E: Record "GE Entry";
        F: FieldRef;
    begin
        R.Open(Database::"GE Doc");
        A := R;
        E.FindFirst();
        A.Get(E.RecordId);
        F := R.Field(2);
        F.Value := 'x';
    end;

    // Moved through an array slot.
    procedure TouchSlot()
    var
        R: RecordRef;
        Slot: array[2] of RecordRef;
        E: Record "GE Entry";
        F: FieldRef;
    begin
        R.Open(Database::"GE Doc");
        Slot[1] := R;
        Slot[1].GetTable(E);
        F := R.Field(2);
        F.Value := 'x';
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

fn workspace() -> tempfile::TempDir {
    let t = tempfile::tempdir().unwrap();
    let ws = t.path().join("ws");
    std::fs::create_dir_all(ws.join("src")).unwrap();
    std::fs::write(
        ws.join("app.json"),
        r#"{"id":"36363636-0000-4000-8000-000000000036","name":"GE","publisher":"t","version":"1.0.0.0"}"#,
    )
    .unwrap();
    std::fs::write(ws.join("src/a.al"), SOURCE).unwrap();
    t
}

#[test]
fn a_recordref_get_moves_the_table_for_every_reader() {
    let t = workspace();
    let inv = alsem(
        t.path(),
        &["fingerprint", "ws", "--inventory-only", "--format", "json"],
    );
    let answer = |name: &str, table: &str| -> String {
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
        let q = format!("writes-table:{table}");
        alsem(t.path(), &["prove", "ws", &id, &q, "--deterministic"])["payload"]["result"]["answer"]
            .as_str()
            .unwrap()
            .to_string()
    };
    // recref_flow: the Get may move R to a Doc, directly, through an alias or an array slot
    // (GetTable through a slot too).
    assert_eq!(answer("GetControl", "GE Log"), "no", "precondition");
    assert_eq!(answer("GetMoves", "GE Log"), "yes");
    // A Get with its own record ID keeps the table the ID was taken from; an ID a `foreach`, a
    // call or a function writes, or an alias in between, may move it anywhere.
    assert_eq!(answer("OwnIdRoot", "GE Log"), "no");
    assert_eq!(answer("OwnIdAliasedRoot", "GE Log"), "no");
    assert_eq!(answer("OwnIdRecordRoot", "GE Log"), "no");
    assert_eq!(answer("OwnIdFieldRoot", "GE Log"), "yes");
    assert_eq!(answer("OwnIdForeachRoot", "GE Log"), "yes");
    assert_eq!(answer("OwnIdPassedRoot", "GE Log"), "yes");
    assert_eq!(answer("OwnIdComputedRoot", "GE Log"), "yes");
    assert_eq!(answer("OwnIdGlobalRoot", "GE Log"), "yes");
    assert_eq!(answer("OwnIdAliasRoot", "GE Log"), "yes");
    assert_eq!(answer("AliasGetRoot", "GE Log"), "yes");
    assert_eq!(answer("SlotGetRoot", "GE Log"), "yes");
    assert_eq!(answer("SlotGetTableRoot", "GE Log"), "yes");
    // A Get before the guard: in a callee (the write-back reaches the caller's argument), and on a
    // local, directly, through an alias or through an array slot (exit_guards).
    assert_eq!(answer("GuardOnlyRoot", "GE Log"), "no", "precondition");
    assert_eq!(answer("GetBeforeGuardRoot", "GE Log"), "yes");
    assert_eq!(answer("LocalUnbound", "GE Log"), "no", "precondition");
    assert_eq!(answer("LocalGet", "GE Log"), "yes");
    assert_eq!(answer("LocalAliasGet", "GE Log"), "yes");
    assert_eq!(answer("LocalSlotGet", "GE Log"), "yes");
    // literal_reach's table frame, directly and through an alias.
    assert_eq!(answer("FrameControlRoot", "GE Log"), "no", "precondition");
    assert_eq!(answer("FrameRoot", "GE Log"), "yes");
    assert_eq!(answer("FrameAliasRoot", "GE Log"), "yes");
    assert_eq!(answer("FrameSlotRoot", "GE Log"), "yes");
    // record_types: after the Get the delete may hit a physical Doc (a quoted receiver alike; a
    // global a called routine moves).
    assert_eq!(answer("TempControl", "GE Doc"), "no", "precondition");
    assert_ne!(answer("TempGet", "GE Doc"), "no");
    assert_ne!(answer("TempGetQuoted", "GE Doc"), "no");
    assert_ne!(answer("TempGetStraight", "GE Doc"), "no");
    assert_ne!(answer("TempGlobal", "GE Doc"), "no");
    // The resolver's RecordRef table: opened on Doc, moved to an Entry.
    assert_eq!(answer("OpenControl", "GE Entry"), "no", "precondition");
    assert_ne!(answer("OpenThenGet", "GE Entry"), "no");
}

#[test]
fn a_field_of_a_recordref_moved_by_get_may_be_any_tables() {
    let t = workspace();
    let v = alsem(
        t.path(),
        &[
            "query",
            "touches",
            "ws",
            "--table",
            "GE Entry",
            "--field",
            "Name",
            "--format",
            "json",
            "--deterministic",
        ],
    );
    let lines: BTreeSet<u64> = v["payload"]["fieldTouches"]
        .as_array()
        .unwrap_or_else(|| panic!("fieldTouches: {v}"))
        .iter()
        .map(|s| s["site"]["line"].as_u64().unwrap())
        .collect();
    assert!(
        lines.contains(&field_line("TouchAfterGet")),
        "{lines:?}: {v}"
    );
    assert!(lines.contains(&field_line("TouchAlias")), "{lines:?}: {v}");
    assert!(lines.contains(&field_line("TouchSlot")), "{lines:?}: {v}");
    assert!(
        !lines.contains(&field_line("TouchControl")),
        "precondition: a RecordRef opened on Doc touches no Entry field: {lines:?}"
    );
}

/// The 1-based line of `F := R.Field(2);` in procedure `name` of [`SOURCE`].
fn field_line(name: &str) -> u64 {
    let head = format!("procedure {name}()");
    let start = SOURCE
        .lines()
        .position(|l| l.trim() == head)
        .unwrap_or_else(|| panic!("precondition: {name} is in the fixture"));
    let at = SOURCE
        .lines()
        .skip(start)
        .position(|l| l.trim() == "F := R.Field(2);")
        .expect("precondition: the procedure reads Field(2)");
    (start + at + 1) as u64
}

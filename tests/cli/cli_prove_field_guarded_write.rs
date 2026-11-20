//! G36: a RecordRef write behind `if not R.FieldExist(N) then exit;` writes
//! only a table that has field `N`.
//!
//! Continia DO's `UpdateHandledFields(var FilterRecord: RecordRef)` modifies
//! whatever table the RecordRef holds, after `if not
//! FilterRecord.FieldExist(6175274) then exit;`. Every `writes-table` question
//! got an `unknown-table-write`; only tables a DO extension gives that field
//! (Sales Header, …) can be written there.
//!
//! The fixture also has a real table sharing its number with the extension
//! (`FG Other` 50400 / `FG Doc Ext` 50400, as DO's 6175275): the model's
//! extension-field merge looked the extension's stub up by that id and took
//! the table instead, so `FG Doc` never got field 50.

use std::process::Command;

/// The extension comes BEFORE the table sharing its number: the merge took
/// the last row of that id.
const SOURCE: &str = r#"tableextension 50400 "FG Doc Ext" extends "FG Doc"
{
    fields { field(50; Handled; Boolean) { } }
}
table 50400 "FG Other"
{
    fields { field(1; K; Integer) { } field(2; N; Text[20]) { } }
    keys { key(PK; K) { } }
}
table 50401 "FG Doc"
{
    fields { field(1; K; Integer) { } }
    keys { key(PK; K) { } }
}
table 50402 "FG Log"
{
    fields { field(1; K; Integer) { } }
    keys { key(PK; K) { } }
}
codeunit 50404 "FG Store"
{
    SingleInstance = true;

    var
        H: RecordRef;

    procedure Save(var R: RecordRef)
    begin
        H := R;
    end;

    procedure Rebind()
    var
        L: Record "FG Log";
    begin
        H.GetTable(L);
    end;
}
codeunit 50403 "FG Mgt"
{
    var
        G: RecordRef;

    procedure SetHandled(V: Variant)
    var
        RecRef: RecordRef;
    begin
        RecRef.GetTable(V);
        Update(RecRef);
    end;

    procedure Update(var R: RecordRef)
    var
        F: FieldRef;
    begin
        if R.Number = 0 then
            exit;
        if not R.FieldExist(50) then
            exit;
        F := R.Field(50);
        F.Value := true;
        R.Modify();
    end;

    procedure Unguarded(var R: RecordRef)
    begin
        R.Modify();
    end;

    procedure Rebound(var R: RecordRef)
    var
        L: Record "FG Log";
    begin
        if not R.FieldExist(50) then
            exit;
        R.GetTable(L);
        R.Modify();
    end;

    // ENG-160: `R.Get(Id)` moves R to the RecordId's table (measured on BC 28.4); it is a
    // record operation, not a call site.
    procedure ReboundByGet(var R: RecordRef; Id: RecordId)
    begin
        if not R.FieldExist(50) then
            exit;
        R.Get(Id);
        R.Modify();
    end;

    // sol r6 on ENG-30: an alias of R rebound inside a `with`: the guard no longer holds.
    procedure AliasWith(var R: RecordRef)
    var
        A: RecordRef;
        W: Record "FG Doc";
        L: Record "FG Log";
    begin
        if not R.FieldExist(50) then
            exit;
        A := R;
        with W do
            A.GetTable(L);
        R.Modify();
    end;

    // ENG-145: `A := R` shares one record, and `A.GetTable(L)` rebinds R too (measured on
    // BC 28.4), with no `with` at all.
    procedure AliasPlain(var R: RecordRef)
    var
        A: RecordRef;
        L: Record "FG Log";
    begin
        if not R.FieldExist(50) then
            exit;
        A := R;
        A.GetTable(L);
        R.Modify();
    end;

    // The same through a quoted alias name.
    procedure AliasQuoted(var R: RecordRef)
    var
        "Alias Ref": RecordRef;
        L: Record "FG Log";
    begin
        if not R.FieldExist(50) then
            exit;
        "Alias Ref" := R;
        "Alias Ref".GetTable(L);
        R.Modify();
    end;

    // The same through a Variant holding the reference.
    procedure AliasVariant(var R: RecordRef)
    var
        V: Variant;
        A: RecordRef;
        L: Record "FG Log";
    begin
        if not R.FieldExist(50) then
            exit;
        V := R;
        A := V;
        A.GetTable(L);
        R.Modify();
    end;

    // sol on ENG-145: the global passed on as `this.G`.
    procedure ThisGlobal()
    begin
        if not G.FieldExist(50) then
            exit;
        RebindLog(this.G);
        G.Modify();
    end;

    // sol on ENG-145: two `var` parameters a caller may bind to one record (`Victim(R, R)`).
    procedure Victim(var A: RecordRef; var B: RecordRef)
    var
        L: Record "FG Log";
    begin
        if not A.FieldExist(50) then
            exit;
        B.GetTable(L);
        A.Modify();
    end;

    // sol on ENG-145: an array slot holding the alias.
    procedure ArraySlot(var R: RecordRef)
    var
        Slot: array[1] of RecordRef;
        L: Record "FG Log";
    begin
        if not R.FieldExist(50) then
            exit;
        Slot[1] := R;
        Slot[1].GetTable(L);
        R.Modify();
    end;

    // sol round 2: a variable literally named `"R[1]"` is one name, not an array element.
    procedure LiteralName()
    var
        "R[1]": RecordRef;
        D: Record "FG Doc";
        L: Record "FG Log";
    begin
        "R[1]".GetTable(D);
        if not "R[1]".FieldExist(50) then
            exit;
        "R[1]".GetTable(L);
        "R[1]".Modify();
    end;

    local procedure RebindLog(var R: RecordRef)
    var
        L: Record "FG Log";
    begin
        R.GetTable(L);
    end;

    // sol round 2 on ENG-145: a function's result hands back the global's record.
    procedure FunctionResult()
    var
        A: RecordRef;
        L: Record "FG Log";
    begin
        if not G.FieldExist(50) then
            exit;
        A := GetG();
        A.GetTable(L);
        G.Modify();
    end;

    local procedure GetG(): RecordRef
    begin
        exit(G);
    end;

    // sol round 2: a store elsewhere keeps the caller's record and rebinds it later.
    procedure Retained(var A: RecordRef)
    var
        Store: Codeunit "FG Store";
    begin
        if not A.FieldExist(50) then
            exit;
        Store.Rebind();
        A.Modify();
    end;

    // sol round 2, the RecordRef flow (G34): one record passed twice, rebound through one.
    procedure TwiceRoot()
    var
        R: RecordRef;
        D: Record "FG Doc";
    begin
        R.GetTable(D);
        Twice(R, R);
    end;

    // sol round 2, the RecordRef flow (G34): a store rebinds the record before the table guard.
    procedure RetainedNumberRoot()
    var
        R: RecordRef;
        D: Record "FG Doc";
    begin
        R.GetTable(D);
        RetainedNumber(R);
    end;

    local procedure RetainedNumber(var A: RecordRef)
    var
        Store: Codeunit "FG Store";
    begin
        Store.Rebind();
        if A.Number <> Database::"FG Log" then
            exit;
        A.Modify();
    end;

    local procedure Twice(var A: RecordRef; var B: RecordRef)
    var
        L: Record "FG Log";
    begin
        B.GetTable(L);
        if A.Number <> Database::"FG Log" then
            exit;
        A.Modify();
    end;

    // Control: a Duplicate() shares nothing, so the guard holds.
    procedure DuplicateControl(var R: RecordRef)
    var
        A: RecordRef;
        L: Record "FG Log";
    begin
        if not R.FieldExist(50) then
            exit;
        A := R.Duplicate();
        A.GetTable(L);
        R.Modify();
    end;
}
"#;

#[test]
fn a_field_guarded_recordref_write_writes_only_tables_with_the_field() {
    let t = tempfile::tempdir().unwrap();
    let ws = t.path().join("ws");
    std::fs::create_dir_all(ws.join("src")).unwrap();
    std::fs::write(
        ws.join("app.json"),
        r#"{"id":"34343434-0000-4000-8000-000000000034","name":"FG","publisher":"t","version":"1.0.0.0"}"#,
    )
    .unwrap();
    std::fs::write(ws.join("src/Mgt.al"), SOURCE).unwrap();
    let answer = |routine: &str, table: &str| -> String {
        let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
            .arg("prove")
            .arg(&ws)
            .args([routine, &format!("writes-table:{table}"), "--deterministic"])
            .env("RUST_LOG", "off")
            .output()
            .unwrap();
        let v: serde_json::Value = serde_json::from_slice(&out.stdout)
            .unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&out.stderr)));
        v["payload"]["result"]["answer"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    };
    // The extension gives `FG Doc` field 50; nothing gives it `FG Log` or
    // `FG Other` (whose number the extension shares).
    assert_eq!(answer("Update", "FG Doc"), "yes");
    assert_eq!(answer("Update", "FG Log"), "no");
    assert_eq!(answer("Update", "FG Other"), "no");
    // Asked by number, the same answers (G40: by number the guard was not
    // read, and `FG Log` answered `unknown`).
    assert_eq!(answer("Update", "50401"), "yes");
    assert_eq!(answer("Update", "50402"), "no");
    // Through the caller that hands it the RecordRef.
    assert_eq!(answer("SetHandled", "FG Log"), "no");
    // Controls: no guard, or a guard the RecordRef is rebound past.
    assert_eq!(answer("Unguarded", "FG Log"), "unknown");
    assert_eq!(answer("Rebound", "FG Log"), "unknown");
    // ENG-160: `R.Get(Id)` past the guard rebinds R too.
    assert_eq!(answer("ReboundByGet", "FG Log"), "unknown");
    // sol r6 on ENG-30: a routine with a `with` takes no field guard.
    assert_eq!(answer("AliasWith", "FG Log"), "unknown");
    // ENG-145: a rebinding through an alias (plain, quoted, through a Variant) ends the guard;
    // a Duplicate() shares nothing, so its guard holds.
    for routine in [
        "AliasPlain",
        "AliasQuoted",
        "AliasVariant",
        // sol on ENG-145: `this.G` passed on, two parameters one caller may alias, an array slot.
        "ThisGlobal",
        "Victim",
        "ArraySlot",
        // sol round 2: a variable literally named `"R[1]"` (`recref_alias::name_key`).
        "LiteralName",
    ] {
        assert_eq!(answer(routine, "FG Log"), "unknown", "{routine}");
    }
    // sol round 2: a function's result, a record retained in a store and rebound by a call, and
    // one record passed twice under a G34 table guard.
    for routine in [
        "FunctionResult",
        "Retained",
        "TwiceRoot",
        "RetainedNumberRoot",
    ] {
        assert_eq!(answer(routine, "FG Log"), "unknown", "{routine}");
    }
    assert_eq!(answer("DuplicateControl", "FG Log"), "no");
}

/// sol r6 on ENG-30: under `with Phys do`, a bare table-method call runs on `Phys` (physical),
/// not on the page's temporary `Rec`; the temporary `Rec` must not suppress its write.
const WITH_TEMP: &str = r#"table 50410 "WT Buf"
{
    fields { field(1; K; Integer) { } }
    keys { key(PK; K) { } }

    procedure ClearBuffer()
    begin
        DeleteAll();
    end;

    procedure Outer()
    var
        Phys: Record "WT Buf";
    begin
        with Phys do
            ClearBuffer();
    end;
}
codeunit 50411 "WT Caller"
{
    var
        Buf: Record "WT Buf" temporary;

    procedure DoIt()
    var
        Tmp: Record "WT Buf" temporary;
    begin
        Tmp.Outer();
    end;

    // sol r7: the object-global backfill gave `Buf` its temporary state back inside the `with`.
    procedure DoGlobal()
    var
        Phys: Record "WT Buf";
    begin
        with Phys do
            Buf.ClearBuffer();
    end;

    // sol r8: the RecordRef temp derivation stamped `Ref.Modify()` temporary from
    // `Ref.GetTable(Tmp)`, though the alias `A` rebinds it to `Phys` inside the `with`.
    procedure RefWith()
    var
        Ref: RecordRef;
        A: RecordRef;
        Tmp: Record "WT Buf" temporary;
        Phys: Record "WT Buf";
        W: Record "WT Buf";
    begin
        Ref.GetTable(Tmp);
        A := Ref;
        with W do
            A.GetTable(Phys);
        Ref.Modify();
    end;

    // A bare table method on `Rec` of a temporary page source: the operation's own temp state.
    procedure BareWith()
    var
        Phys: Record "WT Buf";
    begin
        with Phys do
            DeleteAll();
    end;

    // sol r9: a table subscriber that exits on a temporary record lost its edge when an opaque
    // routine's op (or the resolver's by-name check) proved the record temporary.
    procedure TmpWith()
    var
        Tmp: Record "WT Buf" temporary;
        W: Record "WT Buf";
    begin
        with W do
            Tmp.Insert();
    end;
}
table 50412 "WT Log"
{
    fields { field(1; K; Integer) { } }
    keys { key(PK; K) { } }
}
codeunit 50413 "WT Sub"
{
    [EventSubscriber(ObjectType::Table, Database::"WT Buf", 'OnBeforeInsertEvent', '', false, false)]
    local procedure OnBeforeInsert(var Rec: Record "WT Buf"; RunTrigger: Boolean)
    var
        L: Record "WT Log";
    begin
        if Rec.IsTemporary then
            exit;
        L.Insert();
    end;
}
"#;

#[test]
fn a_bare_table_method_under_with_runs_on_the_with_record() {
    let t = tempfile::tempdir().unwrap();
    let ws = t.path().join("ws");
    std::fs::create_dir_all(ws.join("src")).unwrap();
    std::fs::write(
        ws.join("app.json"),
        r#"{"id":"34343434-0000-4000-8000-000000000035","name":"WT","publisher":"t","version":"1.0.0.0"}"#,
    )
    .unwrap();
    std::fs::write(ws.join("src/WT.al"), WITH_TEMP).unwrap();
    let answer_on = |routine: &str, table: &str| -> serde_json::Value {
        let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
            .arg("prove")
            .arg(&ws)
            .args([routine, &format!("writes-table:{table}"), "--deterministic"])
            .env("RUST_LOG", "off")
            .output()
            .unwrap();
        serde_json::from_slice(&out.stdout)
            .unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&out.stderr)))
    };
    let answer = |routine: &str| answer_on(routine, "WT Buf");
    // sol r9: `Tmp.Insert()` inside a `with` keeps its table subscriber's edge (it exits on a
    // temporary record, which an opaque routine cannot prove), so the subscriber's write of
    // `WT Log` stays reachable. Without the gate in `table_event_edges` the answer was `no`.
    let v = answer_on("TmpWith", "WT Log");
    assert_eq!(
        v["payload"]["result"]["answer"].as_str(),
        Some("yes"),
        "TmpWith: {v:#}"
    );
    // The receiver's temp state is unknown under `with`: D1 reports it Uncertain, so the answer
    // is `yes` (the write may happen), never the `no` a temporary receiver gives, and each write's
    // evidence says `tempState: unknown` (D1's Uncertain; sol r7). DoIt's write runs on the
    // physical `Phys`; DoGlobal's on the temporary global `Buf` (resolving the `with` scope as
    // alc does, ENG-143, would make it `no`). BareWith (sol r8): a record operation in an opaque
    // routine has an unknown temp state. RefWith (sol r8): the RecordRef temp derivation no
    // longer stamps `Ref.Modify()` temporary; the opaque routine's RecordRef table is unknown.
    let v = answer("RefWith");
    assert_eq!(
        v["payload"]["result"]["answer"].as_str(),
        Some("unknown"),
        "RefWith: {v:#}"
    );
    for routine in ["DoIt", "DoGlobal", "BareWith"] {
        let v = answer(routine);
        let r = &v["payload"]["result"];
        assert_eq!(r["answer"].as_str(), Some("yes"), "{routine}: {v:#}");
        let states: Vec<_> = r["evidence"]
            .as_array()
            .expect("evidence")
            .iter()
            .map(|e| e["detail"]["tempState"].as_str())
            .collect();
        assert!(
            !states.is_empty() && states.iter().all(|s| *s == Some("unknown")),
            "{routine}: {states:?}"
        );
    }
}

/// ENG-145: record_types derives a RecordRef's temp state from its `GetTable`/`Open`. An alias
/// (`A := Ref`) shares the record, and `A.GetTable(Phys)` makes `Ref` physical (measured on
/// BC 28.4), so a RecordRef with an alias keeps no derived temp state. Each case has a control
/// with no alias, which stays temporary.
const TEMP_ALIAS: &str = r#"table 50420 "TA Buf"
{
    fields { field(1; K; Integer) { } }
    keys { key(PK; K) { } }
}
codeunit 50422 "TA Store"
{
    SingleInstance = true;

    var
        H: RecordRef;

    procedure Save(var R: RecordRef)
    begin
        H := R;
    end;

    procedure Rebind()
    var
        Phys: Record "TA Buf";
    begin
        H.GetTable(Phys);
    end;
}
// sol round 2: a global the routine never passes on, rebound through a store by a call.
codeunit 50423 "TA Glob"
{
    var
        TG2: RecordRef;

    procedure RetainedGlobalTemp()
    var
        Tmp: Record "TA Buf" temporary;
        Store: Codeunit "TA Store";
    begin
        TG2.GetTable(Tmp);
        Store.Rebind();
        TG2.Modify();
    end;
}
codeunit 50421 "TA Mgt"
{
    var
        TG: RecordRef;

    // No branching: the first derivation pass.
    procedure AliasGetTable()
    var
        Ref: RecordRef;
        A: RecordRef;
        Tmp: Record "TA Buf" temporary;
        Phys: Record "TA Buf";
    begin
        Ref.GetTable(Tmp);
        A := Ref;
        A.GetTable(Phys);
        Ref.Modify();
    end;

    procedure NoAliasGetTable()
    var
        Ref: RecordRef;
        Tmp: Record "TA Buf" temporary;
    begin
        Ref.GetTable(Tmp);
        Ref.Modify();
    end;

    // With branching: the "only ever opened temporary" pass.
    procedure AliasOpenTemp(B: Boolean)
    var
        RR: RecordRef;
        A: RecordRef;
        Phys: Record "TA Buf";
    begin
        if B then
            exit;
        RR.Open(Database::"TA Buf", true);
        A := RR;
        A.GetTable(Phys);
        RR.Insert();
    end;

    procedure NoAliasOpenTemp(B: Boolean)
    var
        RR: RecordRef;
    begin
        if B then
            exit;
        RR.Open(Database::"TA Buf", true);
        RR.Insert();
    end;

    // The same pass, the RecordRef passed on by a quoted name.
    procedure QuotedPassOpenTemp(B: Boolean)
    var
        "My RR": RecordRef;
    begin
        if B then
            exit;
        "My RR".Open(Database::"TA Buf", true);
        Rebind("My RR");
        "My RR".Insert();
    end;

    local procedure Rebind(var R: RecordRef)
    var
        Phys: Record "TA Buf";
    begin
        R.GetTable(Phys);
    end;

    // No branching, the RecordRef passed to a call that rebinds it (the orchestrator's shape).
    procedure HelperOpenTemp()
    var
        RR: RecordRef;
    begin
        RR.Open(Database::"TA Buf", true);
        Rebind(RR);
        RR.Modify();
    end;

    procedure HelperGetTable()
    var
        Ref: RecordRef;
        Tmp: Record "TA Buf" temporary;
    begin
        Ref.GetTable(Tmp);
        Rebind(Ref);
        Ref.Modify();
    end;

    // No branching, the RecordRef assigned a new one (no alias: the value is a call).
    procedure AssignedFromCall()
    var
        Ref: RecordRef;
        Tmp: Record "TA Buf" temporary;
    begin
        Ref.GetTable(Tmp);
        Ref := PhysRef();
        Ref.Modify();
    end;

    // sol on ENG-145, the temp derivation: `this.TG` passed on, two parameters a caller may
    // alias, an array slot.
    procedure ThisGlobalTemp()
    var
        Tmp: Record "TA Buf" temporary;
    begin
        TG.GetTable(Tmp);
        Rebind(this.TG);
        TG.Modify();
    end;

    procedure VictimTemp(var A: RecordRef; var B: RecordRef)
    var
        Tmp: Record "TA Buf" temporary;
        Phys: Record "TA Buf";
    begin
        A.GetTable(Tmp);
        B.GetTable(Phys);
        A.Modify();
    end;

    // sol round 2: a store elsewhere keeps the caller's record and rebinds it later.
    procedure RetainedTemp(var A: RecordRef)
    var
        Store: Codeunit "TA Store";
        Tmp: Record "TA Buf" temporary;
    begin
        A.GetTable(Tmp);
        Store.Rebind();
        A.Modify();
    end;

    procedure ArraySlotTemp()
    var
        Ref: RecordRef;
        Slot: array[1] of RecordRef;
        Tmp: Record "TA Buf" temporary;
        Phys: Record "TA Buf";
    begin
        Ref.GetTable(Tmp);
        Slot[1] := Ref;
        Slot[1].GetTable(Phys);
        Ref.Modify();
    end;

    local procedure PhysRef(): RecordRef
    var
        R: RecordRef;
        Phys: Record "TA Buf";
    begin
        R.GetTable(Phys);
        exit(R);
    end;
}
"#;

#[test]
fn a_recordref_alias_keeps_no_derived_temp_state() {
    let t = tempfile::tempdir().unwrap();
    let ws = t.path().join("ws");
    std::fs::create_dir_all(ws.join("src")).unwrap();
    std::fs::write(
        ws.join("app.json"),
        r#"{"id":"34343434-0000-4000-8000-000000000036","name":"TA","publisher":"t","version":"1.0.0.0"}"#,
    )
    .unwrap();
    std::fs::write(ws.join("src/TA.al"), TEMP_ALIAS).unwrap();
    let answer = |routine: &str| -> String {
        let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
            .arg("prove")
            .arg(&ws)
            .args([routine, "writes-table:TA Buf", "--deterministic"])
            .env("RUST_LOG", "off")
            .output()
            .unwrap();
        let v: serde_json::Value = serde_json::from_slice(&out.stdout)
            .unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&out.stderr)));
        v["payload"]["result"]["answer"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    };
    // An alias (or a quoted pass to a call that rebinds it) may make the record physical: the
    // write is not ruled out by a temporary state.
    for routine in ["AliasGetTable", "AliasOpenTemp", "QuotedPassOpenTemp"] {
        assert_eq!(answer(routine), "unknown", "{routine}");
    }
    // The no-branching pass, the RecordRef passed to a call or assigned: the same one rule.
    for routine in ["HelperOpenTemp", "HelperGetTable", "AssignedFromCall"] {
        assert_eq!(answer(routine), "unknown", "{routine}");
    }
    // sol on ENG-145: `this.TG`, two parameters a caller may alias, an array slot.
    for routine in [
        "ThisGlobalTemp",
        "VictimTemp",
        "ArraySlotTemp",
        "RetainedTemp",
        // A global never passed on: its temp state alone decided `no` (the user-code rule).
        "RetainedGlobalTemp",
    ] {
        assert_eq!(answer(routine), "unknown", "{routine}");
    }
    // Controls: no alias, the record stays temporary.
    for routine in ["NoAliasGetTable", "NoAliasOpenTemp"] {
        assert_eq!(answer(routine), "no", "{routine}");
    }
}

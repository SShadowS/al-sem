table 71050 "TLS50 Item"
{
    fields
    {
        field(1; PK; Code[20]) { }
        field(2; Val; Integer) { }
    }
    keys
    {
        key(Key1; PK) { Clustered = true; }
    }

    trigger OnInsert()
    var
        L: Codeunit "TLS50 Log";
    begin
        L.Add('T.OnInsert', Rec, xRec);
    end;

    trigger OnModify()
    var
        L: Codeunit "TLS50 Log";
    begin
        L.Add('T.OnModify', Rec, xRec);
    end;

    trigger OnDelete()
    var
        L: Codeunit "TLS50 Log";
    begin
        L.Add('T.OnDelete', Rec, xRec);
    end;
}

table 71051 "TLS50 Log"
{
    fields
    {
        field(1; "Entry No."; Integer) { AutoIncrement = true; }
        field(2; Msg; Text[250]) { }
    }
    keys
    {
        key(Key1; "Entry No.") { Clustered = true; }
    }
}

tableextension 71052 "TLS50 Item Ext" extends "TLS50 Item"
{
    trigger OnBeforeInsert()
    var
        L: Codeunit "TLS50 Log";
    begin
        L.Add('X.OnBeforeInsert', Rec, xRec);
    end;

    trigger OnAfterInsert()
    var
        L: Codeunit "TLS50 Log";
    begin
        L.Add('X.OnAfterInsert', Rec, xRec);
    end;

    trigger OnBeforeModify()
    var
        L: Codeunit "TLS50 Log";
    begin
        L.Add('X.OnBeforeModify', Rec, xRec);
        // Does a change made here reach the table trigger and the write?
        if Rec.PK = 'MC' then
            Rec.Val += 100;
    end;

    trigger OnAfterModify()
    var
        L: Codeunit "TLS50 Log";
    begin
        L.Add('X.OnAfterModify', Rec, xRec);
    end;

    trigger OnBeforeDelete()
    var
        L: Codeunit "TLS50 Log";
    begin
        L.Add('X.OnBeforeDelete', Rec, xRec);
    end;

    trigger OnAfterDelete()
    var
        L: Codeunit "TLS50 Log";
    begin
        L.Add('X.OnAfterDelete', Rec, xRec);
    end;
}

codeunit 71053 "TLS50 Log"
{
    /// One log line: the source, Rec.Val, xRec.Val and the stored row's Val
    /// (-1 when no row is stored), so a reader sees what each trigger holds.
    procedure Add(Src: Text; R: Record "TLS50 Item"; X: Record "TLS50 Item")
    var
        Log: Record "TLS50 Log";
        Db: Record "TLS50 Item";
        Stored: Integer;
    begin
        Stored := -1;
        if Db.Get(R.PK) then
            Stored := Db.Val;
        Log.Init();
        Log.Msg := CopyStr(StrSubstNo('%1 rec=%2 x=%3 db=%4', Src, R.Val, X.Val, Stored), 1, 250);
        Log.Insert(false);
    end;

    procedure AddRt(Src: Text; R: Record "TLS50 Item"; RunTrigger: Boolean)
    var
        Log: Record "TLS50 Log";
        Db: Record "TLS50 Item";
        Stored: Integer;
    begin
        Stored := -1;
        if Db.Get(R.PK) then
            Stored := Db.Val;
        Log.Init();
        Log.Msg := CopyStr(StrSubstNo('%1 rt=%2 rec=%3 db=%4', Src, RunTrigger, R.Val, Stored), 1, 250);
        Log.Insert(false);
    end;

    procedure AddRtX(Src: Text; R: Record "TLS50 Item"; X: Record "TLS50 Item"; RunTrigger: Boolean)
    var
        Log: Record "TLS50 Log";
        Db: Record "TLS50 Item";
        Stored: Integer;
    begin
        Stored := -1;
        if Db.Get(R.PK) then
            Stored := Db.Val;
        Log.Init();
        Log.Msg := CopyStr(StrSubstNo('%1 rt=%2 rec=%3 x=%4 db=%5', Src, RunTrigger, R.Val, X.Val, Stored), 1, 250);
        Log.Insert(false);
    end;

    procedure Text(): Text
    var
        Log: Record "TLS50 Log";
        T: Text;
    begin
        Log.SetCurrentKey("Entry No.");
        if Log.FindSet() then
            repeat
                if T <> '' then
                    T += ' | ';
                T += Log.Msg;
            until Log.Next() = 0;
        if T = '' then
            T := '(nothing ran)';
        exit(T);
    end;

    procedure Clear()
    var
        Log: Record "TLS50 Log";
    begin
        Log.DeleteAll(false);
    end;

    [EventSubscriber(ObjectType::Table, Database::"TLS50 Item", 'OnBeforeInsertEvent', '', false, false)]
    local procedure BI(var Rec: Record "TLS50 Item"; RunTrigger: Boolean)
    begin
        AddRt('E.OnBeforeInsertEvent', Rec, RunTrigger);
    end;

    [EventSubscriber(ObjectType::Table, Database::"TLS50 Item", 'OnAfterInsertEvent', '', false, false)]
    local procedure AI(var Rec: Record "TLS50 Item"; RunTrigger: Boolean)
    begin
        AddRt('E.OnAfterInsertEvent', Rec, RunTrigger);
    end;

    [EventSubscriber(ObjectType::Table, Database::"TLS50 Item", 'OnBeforeModifyEvent', '', false, false)]
    local procedure BM(var Rec: Record "TLS50 Item"; var xRec: Record "TLS50 Item"; RunTrigger: Boolean)
    begin
        AddRtX('E.OnBeforeModifyEvent', Rec, xRec, RunTrigger);
    end;

    [EventSubscriber(ObjectType::Table, Database::"TLS50 Item", 'OnAfterModifyEvent', '', false, false)]
    local procedure AM(var Rec: Record "TLS50 Item"; var xRec: Record "TLS50 Item"; RunTrigger: Boolean)
    begin
        AddRtX('E.OnAfterModifyEvent', Rec, xRec, RunTrigger);
    end;

    [EventSubscriber(ObjectType::Table, Database::"TLS50 Item", 'OnBeforeDeleteEvent', '', false, false)]
    local procedure BD(var Rec: Record "TLS50 Item"; RunTrigger: Boolean)
    begin
        AddRt('E.OnBeforeDeleteEvent', Rec, RunTrigger);
    end;

    [EventSubscriber(ObjectType::Table, Database::"TLS50 Item", 'OnAfterDeleteEvent', '', false, false)]
    local procedure AD(var Rec: Record "TLS50 Item"; RunTrigger: Boolean)
    begin
        AddRt('E.OnAfterDeleteEvent', Rec, RunTrigger);
    end;
}

codeunit 71054 "TLS50 Probe Tests"
{
    Subtype = Test;

    var
        L: Codeunit "TLS50 Log";

    local procedure Seed(Code: Code[20]; V: Integer)
    var
        I: Record "TLS50 Item";
    begin
        I.Init();
        I.PK := Code;
        I.Val := V;
        I.Insert(false);
        L.Clear();
    end;

    [Test]
    procedure InsertFalse()
    var
        I: Record "TLS50 Item";
    begin
        L.Clear();
        I.Init();
        I.PK := 'IF';
        I.Val := 5;
        I.Insert(false);
        Error('MEASURED InsertFalse: %1', L.Text());
    end;

    [Test]
    procedure InsertTrue()
    var
        I: Record "TLS50 Item";
    begin
        L.Clear();
        I.Init();
        I.PK := 'IT';
        I.Val := 5;
        I.Insert(true);
        Error('MEASURED InsertTrue: %1', L.Text());
    end;

    [Test]
    procedure ModifyFalse()
    var
        I: Record "TLS50 Item";
    begin
        Seed('MF', 1);
        I.Get('MF');
        I.Val := 2;
        I.Modify(false);
        Error('MEASURED ModifyFalse: %1', L.Text());
    end;

    [Test]
    procedure ModifyTrue()
    var
        I: Record "TLS50 Item";
    begin
        Seed('MT', 1);
        I.Get('MT');
        I.Val := 2;
        I.Modify(true);
        Error('MEASURED ModifyTrue: %1', L.Text());
    end;

    [Test]
    procedure DeleteFalse()
    var
        I: Record "TLS50 Item";
    begin
        Seed('DF', 1);
        I.Get('DF');
        I.Val := 9;
        I.Delete(false);
        Error('MEASURED DeleteFalse: %1', L.Text());
    end;

    [Test]
    procedure DeleteTrue()
    var
        I: Record "TLS50 Item";
    begin
        Seed('DT', 1);
        I.Get('DT');
        I.Val := 9;
        I.Delete(true);
        Error('MEASURED DeleteTrue: %1', L.Text());
    end;

    [Test]
    procedure ModifyAllFalse()
    var
        I: Record "TLS50 Item";
    begin
        Seed('AF', 1);
        I.SetRange(PK, 'AF');
        I.ModifyAll(Val, 3, false);
        Error('MEASURED ModifyAllFalse: %1', L.Text());
    end;

    [Test]
    procedure ModifyAllTrue()
    var
        I: Record "TLS50 Item";
    begin
        Seed('AT', 1);
        I.SetRange(PK, 'AT');
        I.ModifyAll(Val, 3, true);
        Error('MEASURED ModifyAllTrue: %1', L.Text());
    end;

    [Test]
    procedure DeleteAllFalse()
    var
        I: Record "TLS50 Item";
    begin
        Seed('EF', 1);
        I.SetRange(PK, 'EF');
        I.DeleteAll(false);
        Error('MEASURED DeleteAllFalse: %1', L.Text());
    end;

    [Test]
    procedure InsertOmit()
    var
        I: Record "TLS50 Item";
    begin
        L.Clear();
        I.Init();
        I.PK := 'IO';
        I.Val := 5;
        I.Insert();
        Error('MEASURED InsertOmit: %1', L.Text());
    end;

    [Test]
    procedure ModifyOmit()
    var
        I: Record "TLS50 Item";
    begin
        Seed('MO', 1);
        I.Get('MO');
        I.Val := 2;
        I.Modify();
        Error('MEASURED ModifyOmit: %1', L.Text());
    end;

    [Test]
    procedure DeleteOmit()
    var
        I: Record "TLS50 Item";
    begin
        Seed('DO', 1);
        I.Get('DO');
        I.Delete();
        Error('MEASURED DeleteOmit: %1', L.Text());
    end;

    [Test]
    procedure ModifyAllOmit()
    var
        I: Record "TLS50 Item";
    begin
        Seed('AO', 1);
        I.SetRange(PK, 'AO');
        I.ModifyAll(Val, 3);
        Error('MEASURED ModifyAllOmit: %1', L.Text());
    end;

    [Test]
    procedure DeleteAllOmit()
    var
        I: Record "TLS50 Item";
    begin
        Seed('EO', 1);
        I.SetRange(PK, 'EO');
        I.DeleteAll();
        Error('MEASURED DeleteAllOmit: %1', L.Text());
    end;

    /// The extension's OnBeforeModify adds 100 to Val for PK 'MC'.
    [Test]
    procedure ModifyChangedInOnBefore()
    var
        I: Record "TLS50 Item";
        After: Record "TLS50 Item";
    begin
        Seed('MC', 1);
        I.Get('MC');
        I.Val := 2;
        I.Modify(true);
        After.Get('MC');
        Error('MEASURED ModifyChangedInOnBefore: %1 || caller Rec after=%2 stored after=%3', L.Text(), I.Val, After.Val);
    end;

    [Test]
    procedure DeleteAllTrue()
    var
        I: Record "TLS50 Item";
    begin
        Seed('ET', 1);
        I.SetRange(PK, 'ET');
        I.DeleteAll(true);
        Error('MEASURED DeleteAllTrue: %1', L.Text());
    end;
}

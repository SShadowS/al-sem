codeunit 50116 "Out Field Ops"
{
    // W2-4 (2d): one root that writes `Out Item` with every operation kind
    // the field rule distinguishes, and a second writing routine.
    procedure RunAll()
    begin
        Create();
        CloseAll();
        Relabel();
        Purge();
        CreateOther();
        LockLog();
        LockAny(Database::"Out Log");
    end;

    // W2-5 (2e): a lock-only table, and a lock on a table decided at run time.
    local procedure LockLog()
    var
        Log: Record "Out Log";
    begin
        Log.LockTable();
    end;

    local procedure LockAny(TableNo: Integer)
    var
        RecRef: RecordRef;
    begin
        RecRef.Open(TableNo);
        RecRef.LockTable();
    end;

    local procedure Create()
    var
        Item: Record "Out Item";
    begin
        Item.Init();
        Item."No." := 'A';
        Item.Qty := 1;
        Item.Validate(Status, Item.Status::Closed);
        Item.LockTable();
        Item.Insert();
    end;

    local procedure CloseAll()
    var
        Item: Record "Out Item";
    begin
        Item.ModifyAll(Status, Item.Status::Closed);
    end;

    local procedure Relabel()
    var
        Item: Record "Out Item";
    begin
        if Item.Get('A') then
            Item.Rename('B');
    end;

    local procedure Purge()
    var
        Bin: Record "Out Bin";
    begin
        Bin.Code := 'X';
        Bin.DeleteAll();
    end;

    local procedure CreateOther()
    var
        Item: Record "Out Item";
    begin
        Item."No." := 'C';
        Item.Insert();
    end;
}

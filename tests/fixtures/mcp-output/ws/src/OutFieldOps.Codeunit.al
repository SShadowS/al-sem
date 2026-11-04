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
    end;

    local procedure Create()
    var
        Item: Record "Out Item";
    begin
        Item.Init();
        Item."No." := 'A';
        Item.Qty := 1;
        Item.Validate(Status, Item.Status::Closed);
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

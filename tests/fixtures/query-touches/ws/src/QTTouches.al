// The `alsem query touches` fixture (MCP output wave 2, W2-3, spec §4.6, T-W2-10).
// Each routine below is one shape the touches answer must count correctly:
// - WriteItem: a parameterized writer (its Insert is caller-dependent);
// - TempOnly: a routine that touches QT Item only on a temporary record;
// - CallWithTemp / CallWithReal: the caller frame rebinds WriteItem's Insert to a
//   temporary and to a persistent record;
// - Mixed: persistent, temporary and rebound touches in one cone;
// - Leaf: touches QT Other only, so its callers' QT Item touches are informative;
// - Top3 calls Leaf and touches nothing else (a caller that does not touch it);
// - Keyed: key-relevant sites for table mode (output validation §9.3): a
//   SetCurrentKey, a SetRange on the key field "No." (key-relevant) and a
//   SetFilter on Qty (in no key: not key-relevant);
// - Validates: a Validate, a validation and not a record write (round 2 G8).
table 50700 "QT Item"
{
    fields
    {
        field(1; "No."; Code[20]) { }
        field(2; Qty; Integer) { }
    }
    keys { key(PK; "No.") { } }
}

table 50701 "QT Other"
{
    fields
    {
        field(1; "No."; Code[20]) { }
    }
    keys { key(PK; "No.") { } }
}

codeunit 50700 "QT Touches"
{
    procedure WriteItem(var Item: Record "QT Item")
    begin
        Item.Insert();
    end;

    procedure TempOnly()
    var
        TempItem: Record "QT Item" temporary;
    begin
        TempItem.Insert();
        TempItem.FindFirst();
    end;

    procedure CallWithTemp()
    var
        TempItem: Record "QT Item" temporary;
    begin
        WriteItem(TempItem);
    end;

    procedure CallWithReal()
    var
        Item: Record "QT Item";
    begin
        WriteItem(Item);
    end;

    procedure Mixed()
    var
        Item: Record "QT Item";
    begin
        Item.Get('A');
        CallWithTemp();
        TempOnly();
    end;

    procedure Leaf()
    var
        Other: Record "QT Other";
    begin
        Other.Insert();
    end;

    procedure Top1()
    begin
        Leaf();
        CallWithReal();
    end;

    procedure Top2()
    var
        Item: Record "QT Item";
    begin
        Leaf();
        Item.Modify();
        Item.Delete();
    end;

    procedure Top3()
    begin
        Leaf();
    end;

    procedure Keyed()
    var
        Item: Record "QT Item";
    begin
        Item.SetCurrentKey("No.");
        Item.SetRange("No.", 'A');
        Item.SetFilter(Qty, '>0');
        if Item.FindFirst() then;
    end;

    procedure Entry()
    begin
        Top1();
        Top2();
        Top3();
        Mixed();
    end;

    // Round 2 G8: a validation, which table mode counts apart from the
    // record writes (a validation may lead to a write; it is not one).
    procedure Validates()
    var
        Item: Record "QT Item";
    begin
        Item.Validate(Qty, 1);
    end;
}

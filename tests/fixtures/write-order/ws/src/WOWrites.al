// The write-listing order fixture (output validation r3 §7 R6): `query touches
// --table "WO Item" --op-family write` lists the persistent writes first, then
// the caller-dependent ones, then those whose temp state is not resolved, then
// the temporary ones; Insert before Modify before Delete within each. The file
// order below is the reverse of that, so a listing in file order fails.
// - TempFirst: two writes on a temporary record (Delete, then Insert);
// - ViaRecRef: an Insert through a RecordRef opened with a Boolean parameter as
//   its temporary flag (its temp state is not resolved);
// - ByCaller: a Modify on a var parameter (caller-dependent);
// - Real: three writes on a persistent record (Delete, Modify, Insert).
table 50750 "WO Item"
{
    fields
    {
        field(1; "No."; Code[20]) { }
        field(2; Qty; Integer) { }
    }
    keys { key(PK; "No.") { } }
}

codeunit 50750 "WO Writes"
{
    procedure TempFirst()
    var
        TempItem: Record "WO Item" temporary;
    begin
        TempItem.Delete();
        TempItem.Insert();
    end;

    procedure ViaRecRef(UseTemp: Boolean)
    var
        RecRef: RecordRef;
    begin
        RecRef.Open(Database::"WO Item", UseTemp);
        RecRef.Insert();
    end;

    procedure ByCaller(var Item: Record "WO Item")
    begin
        Item.Modify();
    end;

    procedure Real()
    var
        Item: Record "WO Item";
    begin
        Item.Delete();
        Item.Modify();
        Item.Insert();
    end;
}

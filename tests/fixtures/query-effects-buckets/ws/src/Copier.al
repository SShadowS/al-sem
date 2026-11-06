// Output validation V1 (F15, F16): `CopyDoc` reads and writes the record its
// caller passes as its THIRD parameter (`ToRow`, 0-based index 2), so both of
// those operations are caller-dependent; it also reads a temporary record.
// Only the Modify is a write: the two Gets are reads.
table 50494 "QB Row"
{
    fields
    {
        field(1; "No."; Code[20]) { }
        field(2; Name; Text[30]) { }
    }
    keys
    {
        key(PK; "No.") { Clustered = true; }
    }
}

codeunit 50495 "QB Copier"
{
    procedure CopyDoc(FromNo: Code[20]; Mode: Integer; var ToRow: Record "QB Row")
    var
        TempRow: Record "QB Row" temporary;
    begin
        ToRow.Get(FromNo);
        ToRow.Name := 'copied';
        ToRow.Modify();
        if TempRow.Get(FromNo) then
            exit;
    end;
}

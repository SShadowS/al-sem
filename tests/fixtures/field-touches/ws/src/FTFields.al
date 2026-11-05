// The `alsem query touches --field` fixture (agent-tasks design §7, D-1..D-8).
// The field asked is "FT Item".Amount (field 2). Every consumer kind appears once
// or more, on temporary, persistent and caller-dependent records, with the
// shapes that must NOT count (Next(2), Get('X'), FieldIndex(2), Field(3), a
// TransferFields whose other table has no field 2, another table's Amount) and
// the candidates (a record of an unresolved table, a RecordRef parameter).
// Hand counts, by site: the table trigger 3, Kinds 16, ByNumberUnknown 1,
// Unresolved 2, the page 1 = 23 sites, 24 uses (Item.Amount += 1 is one site
// with uses read and write), 3 candidates.
table 50800 "FT Item"
{
    fields
    {
        field(1; "No."; Code[20]) { }
        field(2; Amount; Decimal)
        {
            trigger OnValidate()
            begin
                Amount := Amount + 1;
                TestField(Amount);
            end;
        }
        field(3; "Unit Price"; Decimal) { }
        field(4; Total; Decimal)
        {
            FieldClass = FlowField;
            CalcFormula = sum("FT Item".Amount);
        }
    }
    keys
    {
        key(PK; "No.") { }
        key(K2; Amount, "Unit Price") { }
    }
}

table 50801 "FT Copy"
{
    fields
    {
        field(1; "No."; Code[20]) { }
        field(2; Amount; Decimal) { }
    }
    keys { key(PK; "No.") { } }
}

table 50802 "FT NoMatch"
{
    fields
    {
        field(1; "No."; Code[20]) { }
        // No code uses it: "FT Item" has no field 5 for TransferFields to copy.
        field(5; Unused; Integer) { }
    }
    keys { key(PK; "No.") { } }
}

codeunit 50800 "FT Consumers"
{
    procedure Kinds(var Item: Record "FT Item")
    var
        Tmp: Record "FT Item" temporary;
        Copy: Record "FT Copy";
        NoMatch: Record "FT NoMatch";
        RecRef: RecordRef;
        FRef: FieldRef;
    begin
        Item.Amount += 1;
        Item."Unit Price" := Item.Amount;
        Item.Validate(Amount, 5);
        Item.SetRange(Amount, 0, 10);
        Item.SetFilter(Amount, '>0');
        Item.SetCurrentKey(Amount, "Unit Price");
        Item.SetLoadFields("No.", Amount);
        Item.CalcSums(Amount);
        Item.TestField(Amount);
        Item.FieldError(Amount);
        Message(Item.FieldCaption(Amount));
        Item.ModifyAll(Amount, 0);
        Item.Next(2);
        Item.Get('X');
        Tmp.Amount := 1;
        Copy.TransferFields(Item);
        Item.TransferFields(Copy);
        NoMatch.TransferFields(Item);
        Copy.Amount := 2;
        RecRef.GetTable(Item);
        FRef := RecRef.Field(2);
        FRef := RecRef.FieldIndex(2);
        FRef := RecRef.Field(3);
    end;

    procedure ByNumberUnknown(var RecRef: RecordRef; N: Integer)
    var
        FRef: FieldRef;
    begin
        FRef := RecRef.Field(2);
        FRef := RecRef.Field(N);
    end;

    procedure Unresolved()
    var
        Missing: Record "FT No Such Table";
    begin
        Missing.Amount := 1;
        Missing.SetRange(Amount);
    end;

    procedure OtherFieldOnly(var Item: Record "FT Item")
    begin
        Item."Unit Price" := 0;
        Item.Next(2);
    end;

    // A local named like the field, passed where a step and a key value go:
    // neither names a field.
    procedure ValuesNotFields(var Item: Record "FT Item")
    var
        Amount: Integer;
    begin
        Item.Next(Amount);
        Item.Get(Amount);
    end;
}

// Callers of Kinds, for `--from Kinds --direction up` (two hops, and an
// event subscriber one hop past a publisher). Neither uses Amount.
codeunit 50801 "FT Callers"
{
    procedure CallsKinds()
    var
        I: Record "FT Item";
        C: Codeunit "FT Consumers";
    begin
        I."Unit Price" := 1;
        C.Kinds(I);
        OnAfterKinds();
    end;

    procedure Top()
    begin
        CallsKinds();
    end;

    [IntegrationEvent(false, false)]
    local procedure OnAfterKinds()
    begin
    end;
}

codeunit 50802 "FT Subscriber"
{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::"FT Callers", 'OnAfterKinds', '', false, false)]
    local procedure HandleAfterKinds()
    var
        C: Codeunit "FT Consumers";
        I: Record "FT Item";
    begin
        C.Kinds(I);
    end;
}

page 50800 "FT Card"
{
    SourceTable = "FT Item";

    trigger OnAfterGetRecord()
    begin
        if Amount > 0 then
            exit;
    end;
}

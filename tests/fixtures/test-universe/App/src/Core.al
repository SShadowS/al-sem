table 50800 "TU Item"
{
    fields
    {
        field(1; "No."; Code[20])
        {
        }
        field(2; Qty; Decimal)
        {
        }
    }
}

codeunit 50801 "TU Core"
{
    procedure Post(var Item: Record "TU Item")
    begin
        Calc(Item);
        Leaf();
        Item.Modify();
    end;

    procedure Calc(var Item: Record "TU Item")
    begin
        Item.Qty := Item.Qty + 1;
    end;

    procedure Leaf()
    begin
    end;

    procedure Untested()
    begin
    end;

    procedure Audited()
    begin
    end;

    procedure Logged()
    begin
    end;
}

table 50803 "TU Log"
{
    fields
    {
        field(1; Id; Integer)
        {
        }
    }
}

codeunit 50804 "TU Log Subscribers"
{
    [EventSubscriber(ObjectType::Table, Database::"TU Log", 'OnAfterInsertEvent', '', false, false)]
    local procedure OnLogInserted(var Rec: Record "TU Log")
    var
        Core: Codeunit "TU Core";
    begin
        Core.Logged();
    end;
}

codeunit 50802 "TU Subscribers"
{
    [EventSubscriber(ObjectType::Table, Database::"TU Item", 'OnAfterModifyEvent', '', false, false)]
    local procedure OnItemModified(var Rec: Record "TU Item")
    var
        Core: Codeunit "TU Core";
    begin
        Core.Audited();
    end;
}

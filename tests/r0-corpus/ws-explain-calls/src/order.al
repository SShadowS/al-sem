table 50450 "Calls Order"
{
    Caption = 'Calls Order';

    fields
    {
        field(1; "No."; Code[20]) { }
        field(2; Status; Option)
        {
            OptionMembers = Open,Released,Posted;
        }
        field(3; Amount; Decimal)
        {
            trigger OnValidate()
            begin
                TestField(Status, Status::Open);
            end;
        }
        field(4; Blocked; Boolean) { }
    }

    trigger OnModify()
    begin
        if Blocked then
            Error('A blocked order cannot change.');
    end;
}

page 50451 "Calls Order Card"
{
    PageType = Card;
    SourceTable = "Calls Order";

    actions
    {
        area(Processing)
        {
            action(PostIt)
            {
                Caption = 'Post';
                ToolTip = 'Post the order.';
                RunObject = codeunit "Calls Post Run";
            }
        }
    }
}

codeunit 50452 "Calls Post Run"
{
    trigger OnRun()
    var
        Order: Record "Calls Order";
        Posting: Codeunit "Calls Post";
    begin
        Posting.PostOrder(Order);
    end;
}

codeunit 50459 "Error Message Management"
{
    procedure LogSimpleError(Msg: Text)
    begin
    end;
}

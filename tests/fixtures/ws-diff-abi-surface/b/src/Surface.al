table 50100 "Surf Log"
{
    fields
    {
        field(1; "Entry No."; Integer) { }
        field(2; Note; Text[100]) { }
    }

    trigger OnInsert()
    begin
    end;
}

codeunit 50100 "Surf Pub"
{
    procedure Calc()
    var
        Handled: Boolean;
    begin
        OnBeforeCalc(Handled, 1);
    end;

    [IntegrationEvent(false, false)]
    local procedure OnBeforeCalc(var Handled: Boolean; Qty: Decimal)
    begin
    end;
}

codeunit 50101 "Surf Sub"
{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::"Surf Pub", 'OnBeforeCalc', '', false, false)]
    local procedure HandleOnBeforeCalc(var Handled: Boolean; Qty: Decimal)
    var
        Log: Record "Surf Log";
    begin
        Log.Init();
        Log.Insert();
    end;
}

page 50100 "Surf Card"
{
    PageType = Card;
    SourceTable = "Surf Log";

    actions
    {
        area(Processing)
        {
            action(First)
            {
                trigger OnAction()
                begin
                end;
            }
            action(Second)
            {
                trigger OnAction()
                begin
                end;
            }
        }
    }
}

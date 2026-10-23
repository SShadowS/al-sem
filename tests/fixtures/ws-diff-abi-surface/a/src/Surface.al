table 50100 "Surf Log"
{
    fields
    {
        field(1; "Entry No."; Integer) { }
        field(2; Note; Text[100]) { }
    }
}

codeunit 50100 "Surf Pub"
{
    procedure Calc()
    var
        Handled: Boolean;
    begin
        OnBeforeCalc(Handled);
    end;

    [IntegrationEvent(false, false)]
    local procedure OnBeforeCalc(var Handled: Boolean)
    begin
    end;
}

codeunit 50101 "Surf Sub"
{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::"Surf Pub", 'OnBeforeCalc', '', false, false)]
    local procedure HandleOnBeforeCalc(var Handled: Boolean)
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
        }
    }
}

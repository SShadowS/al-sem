codeunit 51000 "CR Target"
{
    procedure Target()
    begin
    end;
}

codeunit 51001 "CR Direct"
{
    procedure CallIt()
    var
        T: Codeunit "CR Target";
    begin
        T.Target();
    end;
}

codeunit 51002 "CR Runnable"
{
    trigger OnRun()
    var
        T: Codeunit "CR Target";
    begin
        T.Target();
    end;
}

codeunit 51003 "CR Runner"
{
    procedure RunIt()
    begin
        Codeunit.Run(Codeunit::"CR Runnable");
    end;
}

codeunit 51004 "CR Publisher"
{
    [IntegrationEvent(false, false)]
    procedure OnSomething()
    begin
    end;
}

codeunit 51005 "CR Subscriber"
{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::"CR Publisher", 'OnSomething', '', false, false)]
    local procedure HandleSomething()
    var
        T: Codeunit "CR Target";
    begin
        T.Target();
    end;
}

codeunit 51006 "CR Raiser"
{
    procedure RaiseIt()
    var
        P: Codeunit "CR Publisher";
    begin
        P.OnSomething();
    end;
}

table 51000 "CR Entry"
{
    fields
    {
        field(1; "No."; Code[20]) { }
    }

    trigger OnModify()
    var
        T: Codeunit "CR Target";
    begin
        T.Target();
    end;
}

codeunit 51007 "CR Modifier"
{
    procedure ModifyIt()
    var
        E: Record "CR Entry";
    begin
        E.Get('A');
        E.Modify(true);
    end;
}

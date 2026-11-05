table 52900 "DC Dep Entry"
{
    fields
    {
        field(1; "Entry No."; Integer)
        {
        }
    }

    trigger OnInsert()
    var
        Engine: Codeunit "DC Engine";
    begin
        Engine.Run();
    end;
}

codeunit 52901 "DC Engine"
{
    procedure Run()
    begin
        Prepare();
        OnAfterRun();
    end;

    local procedure Prepare()
    begin
    end;

    [IntegrationEvent(false, false)]
    procedure OnAfterRun()
    begin
    end;
}

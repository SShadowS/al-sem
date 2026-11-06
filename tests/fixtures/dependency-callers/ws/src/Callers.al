table 52800 "DC Log"
{
    fields
    {
        field(1; "Entry No."; Integer)
        {
        }
    }
}

codeunit 52801 "DC Target"
{
    procedure Target()
    var
        Log: Record "DC Log";
    begin
        Log.Insert();
    end;
}

codeunit 52802 "DC Mid"
{
    procedure Step()
    var
        T: Codeunit "DC Target";
    begin
        T.Target();
    end;
}

codeunit 52803 "DC Sub"
{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::"DC Engine", 'OnAfterRun', '', false, false)]
    local procedure HandleAfterRun()
    var
        M: Codeunit "DC Mid";
    begin
        M.Step();
    end;

    [EventSubscriber(ObjectType::Codeunit, Codeunit::"Workflow Response Handling", 'OnAfterPostIt', '', false, false)]
    local procedure HandleAfterPostIt()
    var
        T: Codeunit "DC Target";
    begin
        T.Target();
    end;
}

codeunit 52804 "DC Caller"
{
    procedure Start()
    var
        Engine: Codeunit "DC Engine";
    begin
        Engine.Process();
    end;
}

codeunit 52805 "DC Direct"
{
    procedure CallTarget()
    var
        T: Codeunit "DC Target";
    begin
        T.Target();
    end;
}

codeunit 52806 "DC Toucher"
{
    procedure Touch()
    var
        E: Record "DC Dep Entry";
    begin
        E.Insert(true);
    end;
}

codeunit 52807 "DC Approver"
{
    procedure Approve()
    var
        Handling: Codeunit "Workflow Response Handling";
        StepInstance: Record "Workflow Step Instance";
        V: Variant;
    begin
        Handling.ExecuteResponse(V, StepInstance, V);
    end;
}

codeunit 52809 "DC Both"
{
    procedure Direct()
    var
        T: Codeunit "DC Target";
        Engine: Codeunit "DC Engine";
    begin
        T.Target();
        Engine.Process();
    end;

    procedure Mixed()
    var
        Engine: Codeunit "DC Engine";
        Handling: Codeunit "Workflow Response Handling";
        StepInstance: Record "Workflow Step Instance";
        V: Variant;
    begin
        Handling.ExecuteResponse(V, StepInstance, V);
        Engine.Process();
    end;
}

codeunit 52808 "DC Outer"
{
    procedure Outer()
    var
        C: Codeunit "DC Caller";
    begin
        C.Start();
    end;
}

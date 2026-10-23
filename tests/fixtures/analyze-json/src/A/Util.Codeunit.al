codeunit 50100 "Probe Util"
{
    procedure Target()
    begin
        One();
        One();
        Two();
        Three();
    end;

    procedure CallerA()
    begin
        Target();
        Target();
    end;

    procedure CallerB()
    begin
        Target();
    end;

    local procedure One()
    begin
    end;

    internal procedure Two()
    begin
    end;

    protected procedure Three()
    begin
    end;

    local procedure NeverCalled(A: Integer; B: Integer; C: Integer; D: Integer)
    begin
    end;

    procedure Over(X: Integer)
    begin
    end;

    procedure Over(X: Text)
    begin
    end;

    [IntegrationEvent(false, false)]
    procedure OnRaised()
    begin
    end;

    [IntegrationEvent(false, false)]
    procedure OnNeverRaised()
    begin
    end;

    [InternalEvent(false)]
    procedure OnSubscribedOnly()
    begin
    end;

    trigger OnRun()
    var
        Greeter: Interface "Probe Greeter";
        GreeterImpl: Codeunit "Probe Greeter Impl";
        ProbeBase: Record "Probe Base";
    begin
        CallerA();
        CallerB();
        Over(1);
        Over('x');
        OnRaised();
        Greeter := GreeterImpl;
        Greeter.Greet();
        ProbeBase.ExtProc();
    end;
}

codeunit 50922 "D54 Events"
{
    [IntegrationEvent(false, false)]
    procedure OnAfterThing()
    begin
    end;

    [IntegrationEvent(false, false)]
    procedure OnBeforeThing()
    begin
    end;
}

codeunit 50923 "D54 Demo"
{
    // FLAGGED (likely): publisher called directly from a TryFunction body.
    [TryFunction]
    procedure TryDirect()
    var
        Ev: Codeunit "D54 Events";
    begin
        Ev.OnAfterThing();
    end;

    // FLAGGED (possible): publisher reached through a helper.
    [TryFunction]
    procedure TryTransitive()
    begin
        Helper();
    end;

    local procedure Helper()
    var
        Ev: Codeunit "D54 Events";
    begin
        Ev.OnAfterThing();
    end;

    // FLAGGED twice (ENG-101): one TryFunction reaching two publishers is two
    // findings with two fingerprints, one per publisher.
    [TryFunction]
    procedure TryTwo()
    var
        Ev: Codeunit "D54 Events";
    begin
        Ev.OnBeforeThing();
        Ev.OnAfterThing();
    end;

    // NOT FLAGGED: same helper from a non-Try caller.
    procedure PlainCaller()
    begin
        Helper();
    end;
}

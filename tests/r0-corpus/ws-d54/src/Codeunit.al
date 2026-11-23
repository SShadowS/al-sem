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

codeunit 50924 "D54 Many Events"
{
    // FLAGGED five times, not six (ENG-101): the cap is 5 findings per
    // TryFunction, though each publisher has its own key.
    [TryFunction]
    procedure TrySix()
    begin
        OnE1();
        OnE2();
        OnE3();
        OnE4();
        OnE5();
        OnE6();
    end;

    [IntegrationEvent(false, false)]
    procedure OnE1()
    begin
    end;

    [IntegrationEvent(false, false)]
    procedure OnE2()
    begin
    end;

    [IntegrationEvent(false, false)]
    procedure OnE3()
    begin
    end;

    [IntegrationEvent(false, false)]
    procedure OnE4()
    begin
    end;

    [IntegrationEvent(false, false)]
    procedure OnE5()
    begin
    end;

    [IntegrationEvent(false, false)]
    procedure OnE6()
    begin
    end;
}

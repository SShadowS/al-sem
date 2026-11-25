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
    // FLAGGED six times (ENG-101): every publisher is its own finding; no
    // cap per TryFunction (it was 5).
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

    // FLAGGED once (ENG-101 sol r1): one TryFunction declared in both `#if` arms
    // (one routine id) reaching one publisher is ONE finding, not two under one
    // fingerprint; the other arm's declaration is listed in its evidence.
#if CLEAN
    [TryFunction]
    procedure TryPost()
    begin
        OnE1();
    end;
#else
    [TryFunction]
    procedure TryPost()
    begin
        OnE1();
    end;
#endif
}

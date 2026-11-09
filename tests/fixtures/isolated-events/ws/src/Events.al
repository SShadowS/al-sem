codeunit 53000 "Iso Publisher"
{
    procedure DoIsoWork()
    begin
        OnIsoEvent();
    end;

    procedure DoPlainWork()
    begin
        OnPlainEvent();
    end;

    // IncludeSender, GlobalVarAccess, Isolated.
    [IntegrationEvent(false, false, true)]
    local procedure OnIsoEvent()
    begin
    end;

    [IntegrationEvent(false, false)]
    local procedure OnPlainEvent()
    begin
    end;
}

codeunit 53001 "Iso Subscriber"
{
    // A workspace isolated event: the platform catches the error.
    [EventSubscriber(ObjectType::Codeunit, Codeunit::"Iso Publisher", 'OnIsoEvent', '', false, false)]
    local procedure HandleIso()
    begin
        Error('iso subscriber failed');
    end;

    // The control: a normal event lets the error through to DoPlainWork.
    [EventSubscriber(ObjectType::Codeunit, Codeunit::"Iso Publisher", 'OnPlainEvent', '', false, false)]
    local procedure HandlePlain()
    begin
        Error('plain subscriber failed');
    end;

    // A symbol-only dependency's isolated event ([BusinessEvent(False, True)]).
    [EventSubscriber(ObjectType::Codeunit, Codeunit::"Iso Dep Publisher", 'OnDepIso', '', false, false)]
    local procedure HandleDepIso()
    begin
        Error('dependency iso subscriber failed');
    end;

    // The control: the same dependency's normal event ([BusinessEvent(False)]).
    [EventSubscriber(ObjectType::Codeunit, Codeunit::"Iso Dep Publisher", 'OnDepPlain', '', false, false)]
    local procedure HandleDepPlain()
    begin
        Error('dependency plain subscriber failed');
    end;
}

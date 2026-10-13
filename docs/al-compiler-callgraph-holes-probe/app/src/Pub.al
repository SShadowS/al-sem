codeunit 50102 "P Pub"
{
    [IntegrationEvent(false, false)]
    procedure OnSomething()
    begin
    end;

    procedure Raise()
    begin
        OnSomething();
    end;
}
codeunit 50103 "P Sub"
{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::"P Pub", OnSomething, '', false, false)]
    local procedure HandleSomething()
    begin
    end;

    [EventSubscriber(ObjectType::Table, Database::"P Tab", OnAfterInsertEvent, '', false, false)]
    local procedure HandleAfterInsert(var Rec: Record "P Tab")
    begin
    end;

    [EventSubscriber(ObjectType::Page, Page::"P Page", OnOpenPageEvent, '', false, false)]
    local procedure HandleOpenPage()
    begin
    end;

    [EventSubscriber(ObjectType::Table, Database::"P Tab", OnAfterValidateEvent, Amount, false, false)]
    local procedure HandleAfterValidate()
    begin
    end;
}

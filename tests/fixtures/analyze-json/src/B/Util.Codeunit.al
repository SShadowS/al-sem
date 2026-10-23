codeunit 50101 "Probe Subs"
{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::"Probe Util", 'OnRaised', '', false, false)]
    local procedure HandleRaised()
    begin
    end;

    [EventSubscriber(ObjectType::Codeunit, Codeunit::"Probe Util", 'OnSubscribedOnly', '', false, false)]
    local procedure HandleSubscribedOnly()
    begin
    end;
}

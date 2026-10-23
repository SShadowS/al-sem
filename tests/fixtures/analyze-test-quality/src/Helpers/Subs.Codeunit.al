codeunit 50203 "TQ Subs"
{
    var
        Assert: Codeunit Assert;

    [EventSubscriber(ObjectType::Codeunit, Codeunit::"TQ Main Tests", 'OnTQEvent', '', false, false)]
    local procedure OnTQEventHandler()
    begin
        Assert.IsTrue(true, 'subscriber');
    end;
}

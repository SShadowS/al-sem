codeunit 50201 "TQ Helpers"
{
    var
        Assert: Codeunit Assert;

    procedure VerifyThing()
    begin
        Assert.AreEqual(1, 1, '');
    end;
}

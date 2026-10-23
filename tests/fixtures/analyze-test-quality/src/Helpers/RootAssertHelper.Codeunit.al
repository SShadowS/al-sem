codeunit 50204 "TQ Root Assert Helper"
{
    var
        Assert: Codeunit Assert;

    procedure Check()
    begin
        Assert.IsTrue(true, 'root helper');
    end;
}

codeunit 50250 "TQ Nested Helper"
{
    var
        Assert: Codeunit Assert;

    procedure NestedHelper()
    begin
        Assert.IsTrue(true, 'nested helper');
    end;
}

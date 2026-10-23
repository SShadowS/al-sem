codeunit 50300 "TQ Solo Tests"
{
    Subtype = Test;

    var
        Assert: Codeunit Assert;

    [Test]
    procedure WithAssert()
    begin
        Assert.IsTrue(true, '');
    end;

    [Test]
    procedure WithoutAssert()
    begin
    end;
}

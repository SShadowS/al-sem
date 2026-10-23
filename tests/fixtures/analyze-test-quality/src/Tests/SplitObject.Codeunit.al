#if CLEAN25
codeunit 50205 "TQ Split Object Tests"
{
    Subtype = Test;

    var
        Assert: Codeunit Assert;

    [Test]
    procedure W()
    begin
        Assert.IsTrue(true, 'clean');
    end;
}
#else
codeunit 50205 "TQ Split Object Tests"
{
    Subtype = Test;

    var
        Assert: Codeunit Assert;

    [Test]
    procedure W()
    begin
        Assert.IsTrue(true, 'legacy');
    end;
}
#endif

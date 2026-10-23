codeunit 50202 "TQ Split Tests"
{
    Subtype = Test;

    var
        Assert: Codeunit Assert;

    [Test]
    procedure U()
    begin
        S();
    end;

#if CLEAN25
    [Test]
    procedure T()
    begin
        Assert.IsTrue(true, '');
        HelperA();
    end;
#else
    [Test]
    procedure T()
    begin
        Assert.IsTrue(true, '');
        Assert.IsFalse(false, '');
        HelperB();
    end;
#endif

    local procedure HelperA()
    begin
        Assert.IsTrue(true, '');
    end;

    local procedure HelperB()
    begin
        Assert.IsTrue(true, '');
        Assert.IsFalse(false, '');
    end;

#if CLEAN25
    local procedure S()
    begin
        Assert.IsTrue(true, '');
    end;
#else
    local procedure S()
    begin
        Assert.IsTrue(true, '');
        Assert.IsFalse(false, '');
    end;
#endif
}

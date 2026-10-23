codeunit 50200 "TQ Main Tests"
{
    Subtype = Test;

    var
        Assert: Codeunit Assert;
        LibraryAssert: Codeunit "Library Assert";
        Helpers: Codeunit "TQ Helpers";
        NestedCu: Codeunit "TQ Nested Helper";
        TwinCu: Codeunit 50260;

    [Test]
    procedure ZeroAssert()
    var
        Cust: Record Customer;
    begin
        Cust.Init();
        Cust.Insert();
    end;

    [Test]
    procedure OnlyAssertError()
    begin
        asserterror Error('x');
    end;

    [Test]
    procedure AreEqualShapes()
    var
        X: Integer;
        T: Text;
        C: Enum "TQ Color";
        Expected: Integer;
        Cust: Record Customer;
        D: Decimal;
    begin
        Assert.AreEqual(5, X, '');
        Assert.AreEqual('', T, '');
        Assert.AreEqual(-5, X, '');
        Assert.AreEqual(Enum::"TQ Color"::Red, C, '');
        Assert.AreEqual(Database::Customer, X, '');
        Assert.AreEqual(Expected, X, '');
        Assert.AreEqual(Cust.Amount, X, '');
        Assert.AreNotEqual(Format(X), T, '');
        Assert.AreNearlyEqual(1.5, D, 0.01, '');
    end;

    [TestPermissions(TestPermissions::Disabled)]
    procedure PermissionsOnly()
    begin
        Assert.IsTrue(true, '');
    end;

    [ConfirmHandler]
    procedure ConfirmYes(Question: Text[1024]; var Reply: Boolean)
    begin
        Assert.IsTrue(true, '');
        Reply := true;
    end;

    [MessageHandler]
    procedure MessageSink(Message: Text[1024])
    begin
        Assert.IsTrue(true, '');
    end;

    local procedure VerifyX()
    begin
        Assert.IsTrue(true, '');
    end;

    [Test]
    procedure HiddenAssert()
    var
        Assert: Codeunit "Other";
    begin
        Assert.IsTrue(true, '');
    end;

    [Test]
    procedure RenamedAssert()
    var
        Asserter: Codeunit "Library Assert";
    begin
        Asserter.IsTrue(true, '');
        LibraryAssert.IsFalse(false, '');
    end;

    [Test]
    procedure ThisAssert()
    begin
        this.Assert.IsTrue(true, '');
    end;

    [Test]
    procedure ParamReceiver()
    begin
        Check(LibraryAssert);
    end;

    local procedure Check(A: Codeunit "Library Assert")
    begin
        A.IsTrue(true, '');
    end;

    [Test]
    procedure Traversal()
    var
        i: Integer;
        L: List of [Integer];
        Cust: Record Customer;
    begin
        if i = 0 then
            Assert.IsTrue(true, 'then');
        if i = 1 then
            i := 2
        else
            Assert.IsTrue(true, 'else');
        case i of
            1:
                Assert.IsTrue(true, 'case');
            else
                Assert.IsTrue(true, 'case-else');
        end;
        while i < 0 do
            Assert.IsTrue(true, 'while');
        repeat
            Assert.IsTrue(true, 'repeat');
        until true;
        for i := 1 to 1 do
            Assert.IsTrue(true, 'for');
        foreach i in L do
            Assert.IsTrue(true, 'foreach');
        begin
            Assert.IsTrue(true, 'block');
        end;
        asserterror begin
            Assert.Fail('');
        end;
        Assert.IsTrue(Cust.IsEmpty(), '');
    end;

    [Test]
    procedure InBodyIf()
    begin
#if CLEAN25
        Assert.IsTrue(true, 'clean');
#else
        Assert.IsTrue(true, 'legacy');
#endif
    end;

    [Test]
    procedure ReachesCycle()
    begin
        H();
        H();
        OnTQEvent();
    end;

    local procedure H()
    begin
        Assert.IsTrue(true, '');
        H2();
    end;

    local procedure H2()
    begin
        Assert.IsTrue(true, '');
        Assert.IsFalse(false, '');
        H();
    end;

    [IntegrationEvent(false, false)]
    procedure OnTQEvent()
    begin
    end;

    [Test]
    procedure CrossCodeunit()
    begin
        Helpers.VerifyThing();
    end;

    [Test]
    procedure CallsNested()
    begin
        NestedCu.NestedHelper();
    end;

    [Test]
    procedure CallsTwin()
    begin
        TwinCu.Twin();
    end;

    [Test]
    procedure DirectOnly()
    var
        Cust: Record Customer;
    begin
        Assert.IsTrue(true, '');
        Cust.FindFirst();
    end;

    [Test]
    procedure DateTimeBigIntegerLiterals()
    var
        Dt: Date;
        Tm: Time;
        DtTm: DateTime;
        Big: BigInteger;
    begin
        Assert.AreEqual(0D, Dt, '');
        Assert.AreEqual(0T, Tm, '');
        Assert.AreEqual(0DT, DtTm, '');
        Assert.AreEqual(20261023D, Dt, '');
        Assert.AreEqual(123L, Big, '');
        Assert.AreEqual(-123L, Big, '');
    end;
}

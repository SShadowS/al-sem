codeunit 50251 "TQ Nested Tests"
{
    Subtype = Test;

    var
        Assert: Codeunit Assert;
        NestedCu: Codeunit "TQ Nested Helper";

    [Test]
    procedure NestedTest()
    begin
        Assert.IsTrue(true, '');
        NestedCu.NestedHelper();
    end;
}

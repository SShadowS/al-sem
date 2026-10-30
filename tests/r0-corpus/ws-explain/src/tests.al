codeunit 50405 "Explain Tests"
{
    Subtype = Test;

    var
        Assert: Codeunit Assert;

    [Test]
    procedure PostUnreleasedFails()
    var
        Order: Record "Explain Order";
        Posting: Codeunit "Explain Post";
    begin
        asserterror Posting.PostOrder(Order);
        Assert.ExpectedError('Order');
        Assert.IsTrue(true, 'reached');
    end;
}

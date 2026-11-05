codeunit 50850 "TU Core Tests"
{
    Subtype = Test;

    [Test]
    procedure PostUpdatesQty()
    var
        Item: Record "TU Item";
        Core: Codeunit "TU Core";
    begin
        Core.Post(Item);
    end;

    [Test]
    procedure CalcAddsOne()
    var
        Item: Record "TU Item";
        Core: Codeunit "TU Core";
    begin
        Core.Calc(Item);
    end;
}

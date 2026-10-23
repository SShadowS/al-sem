codeunit 50000 "Repro Pricing"
{
    procedure Calc(var Buf: Record "Repro Line" temporary)
    begin
        AddLine(Buf, 'A', 1);
    end;

    local procedure AddLine(var Buf: Record "Repro Line" temporary; Description: Text[100]; Qty: Decimal)
    begin
        Buf.Init();
        Buf.Description := Description;
        Buf.Quantity := Qty;
        Buf.Insert();
    end;
}

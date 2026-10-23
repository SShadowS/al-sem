codeunit 50000 "Repro Pricing"
{
    procedure Calc(var Buf: Record "Repro Line" temporary)
    begin
        AddLine(Buf, 'A', 1, 'MAIN');
    end;

    local procedure AddLine(var Buf: Record "Repro Line" temporary; Description: Text[100]; Qty: Decimal; Branch: Code[10])
    begin
        Buf.Init();
        Buf.Description := Description;
        Buf.Quantity := Qty;
        Buf.Branch := Branch;
        Buf.Insert();
    end;
}

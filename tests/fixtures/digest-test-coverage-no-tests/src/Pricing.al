// #254: a workspace with production code and no tests. `digest --tests` must
// report testProceduresConsidered 0 and every changed routine uncovered.
codeunit 51000 "Nt Pricing"
{
    procedure NetPrice(Gross: Decimal): Decimal
    begin
        exit(Gross - Discount(Gross));
    end;

    procedure Discount(Gross: Decimal): Decimal
    begin
        exit(Gross * 10 / 100);
    end;
}

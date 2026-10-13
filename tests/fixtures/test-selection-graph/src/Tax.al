interface "Acc Tax Rate"
{
    procedure RatePct(): Decimal;
}

enum 50900 "Acc Region" implements "Acc Tax Rate"
{
    Extensible = false;

    value(0; DK)
    {
        Implementation = "Acc Tax Rate" = "Acc Tax DK";
    }
    value(1; DE)
    {
        Implementation = "Acc Tax Rate" = "Acc Tax DE";
    }
    value(2; Exempt)
    {
        Implementation = "Acc Tax Rate" = "Acc Tax Exempt";
    }
}

codeunit 50913 "Acc Tax Core"
{
    procedure TaxAmount(Amount: Decimal; Region: Enum "Acc Region"): Decimal
    var
        TaxRate: Interface "Acc Tax Rate";
    begin
        TaxRate := Region;
        exit(Amount * TaxRate.RatePct() / 100);
    end;

    procedure GrossAmount(Amount: Decimal; Region: Enum "Acc Region"): Decimal
    begin
        exit(Amount + TaxAmount(Amount, Region));
    end;
}

codeunit 50914 "Acc Tax DK" implements "Acc Tax Rate"
{
    procedure RatePct(): Decimal
    begin
        exit(25);
    end;
}

codeunit 50915 "Acc Tax DE" implements "Acc Tax Rate"
{
    procedure RatePct(): Decimal
    begin
        exit(19);
    end;
}

codeunit 50916 "Acc Tax Exempt" implements "Acc Tax Rate"
{
    procedure RatePct(): Decimal
    begin
        exit(0);
    end;
}

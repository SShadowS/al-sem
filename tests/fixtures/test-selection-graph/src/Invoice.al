codeunit 50920 "Acc Invoice Core"
{
    var
        TaxCore: Codeunit "Acc Tax Core";

    procedure InvoiceTotal(Amount: Decimal; Region: Enum "Acc Region"): Decimal
    var
        Total: Decimal;
    begin
        Total := TaxCore.GrossAmount(Amount, Region);
        OnAfterInvoiceTotal(Total);
        exit(Total);
    end;

    [IntegrationEvent(false, false)]
    local procedure OnAfterInvoiceTotal(var Total: Decimal)
    begin
    end;
}

codeunit 50922 "Acc Cash Rounding Subscriber"
{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::"Acc Invoice Core", 'OnAfterInvoiceTotal', '', false, false)]
    local procedure CashRoundOnInvoiceTotal(var Total: Decimal)
    begin
        Total := RoundToNickel(Total);
    end;

    local procedure RoundToNickel(Amount: Decimal): Decimal
    begin
        exit(Round(Amount, 0.05, '='));
    end;
}

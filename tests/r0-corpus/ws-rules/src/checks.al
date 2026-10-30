codeunit 50301 "Rules Checks"
{
    var
        GlobalOnlyLbl: Label 'Order %1 has no posting date.', Comment = '%1 = order number';
        SharedLbl: Label 'Global shared text.';

    /// Checks an order before it is posted.
    procedure CheckOrder(var Order: Record "Rules Order"; PostingPreview: Boolean)
    var
        NotReleasedErr: Label 'You cannot post %1 %2 because it is not released.', Comment = '%1 = document type, %2 = number';
        AmountTxt: TextConst ENU = 'The amount must be positive.', DAN = 'Beløbet skal være positivt.';
    begin
        // Only released orders can be posted.
        if Order.Status <> Order.Status::Released then
            Error(NotReleasedErr, Order."Document Type", Order."No."); // released only
        if Order.Amount <= 0 then
            Error(AmountTxt);
        // A customer is required.
        // It is checked before the blocked flag.
        Order.TestField("Customer No.");
        Order.TestField(Blocked, false);
        if PostingPreview then
            exit;
        if Order."Posting Date" = 0D then
            Error(StrSubstNo(GlobalOnlyLbl, Order."No."));
    end;

    procedure ConfirmDelete(var Order: Record "Rules Order")
    var
        DeleteQst: Label 'Delete order %1?';
        CancelledErr: Label 'The deletion was cancelled.';
    begin
        if not Confirm(DeleteQst, false, Order."No.") then
            exit;
        if GuiAllowed() then
            if not Confirm('Really delete?') then
                Error(CancelledErr);
        Order.Delete(true);
    end;

    procedure CheckStatus(var Order: Record "Rules Order")
    begin
        case Order.Status of
            Order.Status::Open:
                Order.TestField(Amount);
            Order.Status::Released, Order.Status::Pending:
                exit;
            else
                Error('Unknown status %1.', Order.Status);
        end;
    end;

    procedure CheckCustomer(CustomerNo: Code[20]; Amount: Decimal)
    var
        Customer: Record "Rules Customer";
    begin
        if not Customer.Get(CustomerNo) then
            Error('Customer %1 does not exist.', CustomerNo);
        if Customer.Blocked or (Customer."Credit Limit" < Amount) then
            Error('Customer %1 cannot buy.', CustomerNo);
        if Amount <= Customer."Credit Limit" then
            Message('Within the limit.')
        else
            Error('Over the limit.');
        case true of
            Amount > 1000:
                Error('Too large.');
        end;
    end;

    procedure CollectibleCheck(var Order: Record "Rules Order")
    var
        ErrInfo: ErrorInfo;
        TooLargeLbl: Label 'The amount is too large.';
    begin
        ErrInfo := ErrorInfo.Create(TooLargeLbl);
        if Order.Amount > 1000000 then
            Error(ErrInfo);
        if Order.Amount < 0 then
            Error('Negative amount.');
    end;

    procedure ShadowedLabel()
    var
        SharedLbl: Label 'Local shared text.';
    begin
        Error(SharedLbl);
    end;

    procedure FailWith(Msg: Text)
    begin
        Error(Msg);
    end;
}

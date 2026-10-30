codeunit 50454 "Calls Checks"
{
    procedure CheckOrder(var Order: Record "Calls Order"; Strict: Boolean)
    begin
        if Order."No." = '' then
            Error('The order has no number.');
        if Strict then
            Order.TestField(Amount);
        CheckCustomer(Order);
    end;

    procedure CheckCustomer(var Order: Record "Calls Order")
    begin
        if Order.Blocked then
            Error('The customer is blocked.');
    end;

    // Preview is assigned before the check, so a caller's argument does not decide it.
    procedure CheckPreview(var Order: Record "Calls Order"; Preview: Boolean)
    begin
        Preview := Preview or Order.Blocked;
        if Preview then
            Error('A preview cannot post.');
    end;

    procedure CheckCollectible(var Order: Record "Calls Order")
    var
        ErrInfo: ErrorInfo;
    begin
        if Order.Amount < 0 then begin
            ErrInfo.Message('A negative amount is collected.');
            Error(ErrInfo);
        end;
    end;

    procedure CheckBig(var Order: Record "Calls Order")
    begin
        if Order.Amount > 1000 then
            Error('The amount is too big.');
    end;

    procedure CheckLate(var Order: Record "Calls Order")
    begin
        if Order.Status = Order.Status::Posted then
            Error('The order is already posted.');
    end;

    procedure LogIfBlocked(var Order: Record "Calls Order")
    var
        ErrorMessageMgt: Codeunit "Error Message Management";
    begin
        if Order.Blocked then
            ErrorMessageMgt.LogSimpleError('The order is blocked.');
    end;

    // PingCheck and PongCheck call each other: the walk must stop.
    procedure PingCheck(var Order: Record "Calls Order")
    begin
        if Order.Amount < 0 then
            Error('The amount is negative.');
        PongCheck(Order);
    end;

    procedure PongCheck(var Order: Record "Calls Order")
    begin
        PingCheck(Order);
    end;
}

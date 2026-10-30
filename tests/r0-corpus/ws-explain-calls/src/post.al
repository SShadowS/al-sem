codeunit 50453 "Calls Post"
{
    procedure PostOrder(var Order: Record "Calls Order")
    var
        Checks: Codeunit "Calls Checks";
    begin
        Checks.CheckOrder(Order, true);
        Checks.CheckCollectible(Order);
        if Order.Amount > 100 then
            Checks.CheckBig(Order);
        OnBeforePostOrder(Order);
        Order.Status := Order.Status::Released;
        Order.Validate(Amount, 10);
        Order.Modify(true);
        Checks.CheckLate(Order);
    end;

    procedure QuickPost(var Order: Record "Calls Order")
    var
        Checks: Codeunit "Calls Checks";
    begin
        Checks.CheckOrder(Order, false);
        Checks.LogIfBlocked(Order);
        Order.Amount := 5;
        Order.Modify();
    end;

    procedure ForwardPost(var Order: Record "Calls Order"; StrictMode: Boolean)
    var
        Checks: Codeunit "Calls Checks";
    begin
        Checks.CheckOrder(Order, StrictMode);
        Order.Modify(true);
    end;

    procedure PreviewPost(var Order: Record "Calls Order")
    var
        Checks: Codeunit "Calls Checks";
    begin
        Checks.CheckPreview(Order, false);
        Order.Modify(true);
    end;

    procedure RecursivePost(var Order: Record "Calls Order")
    var
        Checks: Codeunit "Calls Checks";
    begin
        Checks.PingCheck(Order);
        Order.Modify();
    end;

    procedure Release(var Order: Record "Calls Order")
    begin
        Order.Status := Order.Status::Released;
        Order.Modify();
    end;

    procedure ReleaseChecked(var Order: Record "Calls Order")
    var
        Checks: Codeunit "Calls Checks";
    begin
        if Order.Status = Order.Status::Posted then
            Error('A posted order cannot be released.');
        Checks.CheckOrder(Order, true);
        Checks.CheckCollectible(Order);
        Release(Order);
    end;

    procedure ReleaseDirect(var Order: Record "Calls Order")
    begin
        Release(Order);
    end;

    [IntegrationEvent(false, false)]
    local procedure OnBeforePostOrder(var Order: Record "Calls Order")
    begin
    end;
}

codeunit 50455 "Calls Subscriber"
{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::"Calls Post", 'OnBeforePostOrder', '', false, false)]
    local procedure CheckBeforePost(var Order: Record "Calls Order")
    begin
        if Order.Amount = 0 then
            Error('A zero amount cannot post.');
    end;
}

codeunit 50401 "Explain Post"
{
    /// Posts a released order.
    /// It marks the order posted.
    procedure PostOrder(var Order: Record "Explain Order")
    var
        NotReleasedErr: Label 'Order %1 is not released.', Comment = '%1 = order number';
        IsHandled: Boolean;
    begin
        if Order.Status <> Order.Status::Released then
            Error(NotReleasedErr, Order."No.");
        Order.TestField(Amount);
        OnBeforePostOrder(Order, IsHandled);
        if IsHandled then
            exit;
        Order.Posted := true;
        Order.Validate(Status, Order.Status::Posted);
        Order.Modify(true);
        Message('Order %1 posted.', Order."No.");
    end;

    // Archives without any check.
    procedure Archive(var Order: Record "Explain Order")
    begin
        Order.Status := Order.Status::Archived;
        Order.Modify();
        Commit();
    end;

    // A ModifyAll writer of Status: listed by --field since step 6.
    procedure ReopenAll()
    var
        Order: Record "Explain Order";
    begin
        Order.ModifyAll(Status, Order.Status::Open);
    end;

    procedure CheckAmount(Amount: Decimal)
    begin
        if Amount < 0 then
            Error('Amount must not be negative.');
    end;

    [IntegrationEvent(false, false)]
    local procedure OnBeforePostOrder(var Order: Record "Explain Order"; var IsHandled: Boolean)
    begin
    end;
}

codeunit 50402 "Explain Other"
{
    // A second CheckAmount: the bare name is ambiguous.
    procedure CheckAmount(Amount: Decimal)
    begin
    end;
}

codeunit 50403 "Explain Subscriber"
{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::"Explain Post", 'OnBeforePostOrder', '', false, false)]
    local procedure HandleBeforePost(var Order: Record "Explain Order"; var IsHandled: Boolean)
    begin
    end;
}

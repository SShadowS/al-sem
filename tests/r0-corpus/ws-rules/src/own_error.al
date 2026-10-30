codeunit 50302 "Rules Own Error"
{
    procedure CheckLimit(var Order: Record "Rules Order")
    begin
        if Order.Amount < 0 then
            Error('Amount %1 is negative.', Order.Amount);
        Order.TestField(Status);
    end;

    // This object's own Error: a bare Error(...) call runs it and returns, so
    // it does not stop the caller. It hides the builtin for every bare call in
    // the object (alc: a one-argument Error(...) here is AL0135).
    procedure Error(Msg: Text; Value: Decimal)
    begin
        Message(Msg, Value);
    end;
}

codeunit 50303 "Rules Directives"
{
    procedure CheckVersioned(var Order: Record "Rules Order")
    var
#if CLEAN25
        VersionErr: Label 'The new text.';
#else
        VersionErr: Label 'The old text.';
#endif
    begin
        if Order.Amount = 0 then
            Error(VersionErr);
    end;
}

// Round-2 c06: which callers run a check before their call to SetUsage.
// The check sits two calls deep (HasRecipients -> FindLine), as CDO's
// HasRecipients -> FindEMailTemplateLine does. The app sits one folder down
// (App/) so the ids parent-baseline record, which covers every fixture with
// a root app.json, does not take it in.
table 50900 "CRP Order"
{
    fields
    {
        field(1; "No."; Code[20])
        {
        }
        field(2; Blocked; Boolean)
        {
        }
    }
}

codeunit 50901 "CRP Handler"
{
    var
        GlobalUsage: Integer;

    procedure SetUsage(Usage: Integer)
    begin
        GlobalUsage := Usage;
    end;

    procedure HasRecipients(var Order: Record "CRP Order"): Boolean
    begin
        if FindLine(Order) then
            exit(true);
        exit(false);
    end;

    procedure FindLine(var Order: Record "CRP Order"): Boolean
    begin
        if Order."No." = '' then
            Error('The order has no number.');
        exit(not Order.Blocked);
    end;
}

codeunit 50902 "CRP Callers"
{
    // Top-level, before the call: the check runs first.
    procedure TopLevel(var Order: Record "CRP Order")
    var
        Handler: Codeunit "CRP Handler";
        Enabled: Boolean;
    begin
        Enabled := Handler.HasRecipients(Order);
        Handler.SetUsage(89);
    end;

    // In the call's own branch, before it: the check runs first on every
    // path to the call, though neither call is top-level.
    procedure SameBranch(var Order: Record "CRP Order"; Active: Boolean)
    var
        Handler: Codeunit "CRP Handler";
        Enabled: Boolean;
    begin
        if Active then begin
            Enabled := Handler.HasRecipients(Order);
            Handler.SetUsage(89);
        end;
    end;

    // In the condition of the branch around the call: it runs first.
    procedure InCondition(var Order: Record "CRP Order")
    var
        Handler: Codeunit "CRP Handler";
    begin
        if Handler.HasRecipients(Order) then
            Handler.SetUsage(89);
    end;

    // In the condition of a nested branch around the call: it runs first,
    // though it is not top-level.
    procedure NestedCondition(var Order: Record "CRP Order"; Active: Boolean)
    var
        Handler: Codeunit "CRP Handler";
    begin
        if Active then
            if Handler.HasRecipients(Order) then
                Handler.SetUsage(89);
    end;

    // In the other branch: it does not run before the call.
    procedure OtherBranch(var Order: Record "CRP Order"; Active: Boolean)
    var
        Handler: Codeunit "CRP Handler";
        Enabled: Boolean;
    begin
        if Active then
            Enabled := Handler.HasRecipients(Order)
        else
            Handler.SetUsage(89);
    end;

    // After the call: it does not run before it.
    procedure After(var Order: Record "CRP Order")
    var
        Handler: Codeunit "CRP Handler";
        Enabled: Boolean;
    begin
        Handler.SetUsage(89);
        Enabled := Handler.HasRecipients(Order);
    end;
}

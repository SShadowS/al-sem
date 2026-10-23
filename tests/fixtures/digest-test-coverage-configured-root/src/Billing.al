// #254 matrix row 4: `roots.config.json` declares `Billing.Configured` with kind
// `test-procedure`. Only a `[Test]` attribute makes a test root, so it stays a
// production routine: counted, and uncovered (nothing calls it).
codeunit 51010 "Cr Billing"
{
    procedure Configured(): Integer
    begin
        exit(Charged());
    end;

    procedure Charged(): Integer
    begin
        exit(1);
    end;
}

codeunit 51011 "Cr Billing Tests"
{
    Subtype = Test;

    [Test]
    procedure ChargedIsOne()
    var
        Billing: Codeunit "Cr Billing";
    begin
        if Billing.Charged() <> 1 then
            Error('Charged should be 1');
    end;
}

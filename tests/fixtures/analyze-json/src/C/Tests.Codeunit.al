codeunit 50104 "Probe Tests"
{
    Subtype = Test;

    [Test]
    procedure TestSomething()
    begin
    end;

    [ConfirmHandler]
    procedure ConfirmYes(Question: Text[1024]; var Reply: Boolean)
    begin
        Reply := true;
    end;
}

codeunit 50271 "S H"
{
    procedure ResetIt(var R: Record "S Cust")
    begin
        R.Reset();
    end;

    procedure Dirty(var R: Record "S Cust")
    begin
        R.Validate(Name, 'x');
    end;

    procedure ReadsAddress(var R: Record "S Cust")
    begin
        Message(R.Address);
    end;

    procedure NeedsLoaded(var R: Record "S Cust")
    begin
        R.TestField(Name);
        R.Modify();
    end;

    procedure Num(var I: Integer)
    begin
        I := 1;
    end;
}

codeunit 50701 "Pin Writers"
{
    // Six writers of Amount apply TestField(Status, Open): 6 of 8 is the
    // majority bar exactly, so `explain --emit-policy` pins it.
    procedure SetOne(var O: Record "Pin Order")
    begin
        O.TestField(Status, O.Status::Open);
        O.Amount := 1;
        O.Modify();
    end;

    procedure SetTwo(var O: Record "Pin Order")
    begin
        O.TestField(Status, O.Status::Open);
        O.Validate(Amount, 2);
        O.Modify();
    end;

    procedure SetThree(var O: Record "Pin Order")
    begin
        O.TestField(Status, O.Status::Open);
        O.Amount := 3;
        O.Modify();
    end;

    procedure SetFour(var O: Record "Pin Order")
    begin
        O.TestField(Status, O.Status::Open);
        O.Amount := 4;
        O.Modify();
    end;

    procedure SetFive(var O: Record "Pin Order")
    begin
        O.TestField(Status, O.Status::Open);
        O.Amount := 5;
        O.Modify();
    end;

    procedure SetSix(var O: Record "Pin Order")
    begin
        O.TestField(Status, O.Status::Open);
        O.Amount := 6;
        O.Modify();
    end;

    // Violates the pin: no check at all.
    procedure Bypass(var O: Record "Pin Order")
    begin
        O.Amount := 0;
        O.Modify();
    end;

    // Violates the pin: the check may not stop (an ErrorInfo), so it is not
    // applied.
    procedure Collect(var O: Record "Pin Order")
    var
        ErrInfo: ErrorInfo;
    begin
        O.TestField(Status, O.Status::Open, ErrInfo);
        O.Amount := 9;
        O.Modify();
    end;
}

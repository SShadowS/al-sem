codeunit 50701 "Pin Writers"
{
    // `../bypass` with the two bypasses fixed: every writer of Amount applies
    // TestField(Status, Open), so the pin emitted from `../bypass` passes.
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

    // Fixed: the check added.
    procedure Bypass(var O: Record "Pin Order")
    begin
        O.TestField(Status, O.Status::Open);
        O.Amount := 0;
        O.Modify();
    end;

    // Fixed: a check that always stops.
    procedure Collect(var O: Record "Pin Order")
    begin
        O.TestField(Status, O.Status::Open);
        O.Amount := 9;
        O.Modify();
    end;
}

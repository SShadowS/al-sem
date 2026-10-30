codeunit 50651 "Guard Writers"
{
    // Followers: each applies TestField(Status, Open) before it writes. The
    // `or` check before it adds `not O.Blocked and not (O.Amount < 0)` to the
    // TestField's condition; the rule key leaves that out (it comes from an
    // earlier statement), so the key matches FollowOuter's.
    procedure FollowAssign(var O: Record "Guard Order")
    begin
        if (O.Blocked) or (O.Amount < 0) then
            Error('A blocked or negative order cannot change.');
        O.TestField(Status, O.Status::Open);
        O.Amount := 1;
        O.Quantity := 1;
        O.Price := 1;
        O.Discount := 1;
        O.Note := 'a';
        O."Code" := 'a';
        O.Modify();
    end;

    procedure FollowValidate(var Ord: Record "Guard Order")
    begin
        if (Ord.Blocked) or (Ord.Amount < 0) then
            Error('A blocked or negative order cannot change.');
        Ord.TestField(Status, Ord.Status::Open);
        Ord.Validate(Amount, 2);
        Ord.Validate(Quantity, 2);
        Ord.Validate(Price, 2);
        Ord.Validate(Discount, 2);
        Ord.Validate(Note, 'b');
        Ord.Validate("Code", 'b');
        Ord.Modify();
    end;

    // The rule comes from a callee that runs before the writes.
    procedure FollowInherited(var Rec: Record "Guard Order")
    begin
        if (Rec.Blocked) or (Rec.Amount < 0) then
            Error('A blocked or negative order cannot change.');
        CheckOpen(Rec);
        Rec.Amount := 3;
        Rec.Quantity := 3;
        Rec.Price := 3;
        Rec.Discount := 3;
        Rec.Note := 'c';
        Rec."Code" := 'c';
        Rec.Modify();
    end;

    procedure CheckOpen(var Rec: Record "Guard Order")
    begin
        Rec.TestField(Status, Rec.Status::Open);
    end;

    // The rule comes from its only caller.
    procedure FollowOuter(var O: Record "Guard Order")
    begin
        O.TestField(Status, O.Status::Open);
        WriteAmountInner(O);
    end;

    local procedure WriteAmountInner(var O: Record "Guard Order")
    begin
        O.Amount := 4;
        O.Modify();
    end;

    // Bypass: writes Amount with no check (a finding). Also writes Code,
    // which has no majority rule (3 of 5 writers).
    procedure BypassAmount(var O: Record "Guard Order")
    begin
        O.Amount := 0;
        O."Code" := 'x';
        O.Modify();
    end;

    // Bypass: the check sits in the other branch (a finding for Quantity).
    procedure BypassBranch(var O: Record "Guard Order"; Quick: Boolean)
    begin
        if Quick then
            O.TestField(Status, O.Status::Open)
        else begin
            O.Quantity := 0;
            O."Code" := 'y';
        end;
        O.Modify();
    end;

    // Bypass through ModifyAll (a finding for Price).
    procedure BypassModifyAll(var O: Record "Guard Order")
    begin
        O.ModifyAll(Price, 0);
    end;

    // A deliberate exception, marked for the gate (a finding here; the
    // analyze gate suppresses it).
    procedure ResetDiscount(var O: Record "Guard Order")
    begin
        // al-sem-ignore d65-guard-bypass: the year-end reset runs on posted orders too
        O.Discount := 0;
        O.Modify();
    end;

    // The check may not stop (an ErrorInfo): it does not count (a finding for Note).
    procedure CollectNote(var O: Record "Guard Order")
    var
        ErrInfo: ErrorInfo;
    begin
        O.TestField(Status, O.Status::Open, ErrInfo);
        O.Note := 'z';
        O.Modify();
    end;

    // Ref: three writers apply an `or` check, which is one `Other` atom. An
    // `Other` atom forms no key, so RefOnly, which does not apply it, is no
    // finding.
    procedure RefFirst(var O: Record "Guard Order")
    begin
        if (O.Blocked) or (O.Amount < 0) then
            Error('A blocked or negative order cannot change.');
        O.Ref := 'r1';
        O.Modify();
    end;

    procedure RefSecond(var O: Record "Guard Order")
    begin
        if (O.Blocked) or (O.Amount < 0) then
            Error('A blocked or negative order cannot change.');
        O.Ref := 'r2';
        O.Modify();
    end;

    procedure RefThird(var O: Record "Guard Order")
    begin
        if (O.Blocked) or (O.Amount < 0) then
            Error('A blocked or negative order cannot change.');
        O.Ref := 'r3';
        O.Modify();
    end;

    procedure RefOnly(var O: Record "Guard Order")
    begin
        O.TestField(Status, O.Status::Open);
        O.Ref := 'r';
        O.Modify();
    end;
}

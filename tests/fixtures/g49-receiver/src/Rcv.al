// G49: `Touch` modifies its own implicit `Rec`. The write's temp state is
// parameter-dependent on the receiver (the reserved index u32::MAX), and every
// user-facing rendering must say `parameter-dependent(rec)`.
table 50490 "Rcv Row"
{
    fields
    {
        field(1; Id; Integer) { }
        field(2; Name; Text[30]) { }
    }
    keys
    {
        key(PK; Id) { Clustered = true; }
    }

    procedure Touch()
    begin
        Name := 'x';
        Modify();
    end;

    // No caller: `prove` cannot settle the write and names the implicit Rec in
    // its `temp-state-depends-on-caller` blocker.
    procedure TouchAlone()
    begin
        Name := 'y';
        Modify();
    end;
}

// A caller that passes a persistent record, so `prove` on `Touch` names a
// caller frame that makes the write real.
codeunit 50491 "Rcv Caller"
{
    procedure TouchFirst()
    var
        Row: Record "Rcv Row";
    begin
        Row.Get(1);
        Row.Touch();
    end;
}

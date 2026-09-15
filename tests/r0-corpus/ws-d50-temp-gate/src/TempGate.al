// Issue 23 — d50's transaction-managing COUNT branch must count PHYSICAL table
// writes (as d8 already does), not temp-inclusive ones. A temporary record is
// in-memory only; it never dirties the transaction an implicit commit would
// split.
//
// FOUR writer procedures over the SAME three table objects. Per-routine
// `temporary` declarations decide temp-ness, and the cone counts are per
// routine, so sharing the table objects across routines cannot bleed.
//
//   BufferRows      T1,T2,T3 all temporary          inclusive 3 / physical 0
//   StageRows       T1,T2,T3 all physical           inclusive 3 / physical 3
//   StageMixedRows  T1,T2 physical, T3 temporary    inclusive 3 / physical 2
//   PostBuffers     T1,T2,T3 all temporary, POSTING name
//                                                   inclusive 3 / physical 0
//
// THE ISOLATION CONTRACT. Each writer holds its OWN checked
// `if Codeunit.Run(...) then;`, so each is its own CheckedRunImplicit seed.
// That alone is not enough: the four writers are also mutually independent —
// NO writer calls another, and there is NO common AL driver that calls more
// than one. A shared driver would inherit StageRows' three PHYSICAL writes
// through its own forward capability cone, land in BufferRows' BACKWARD span
// as a transaction manager, and d50 would still report at BufferRows' callsite
// after the fix — failing for a reason that has nothing to do with the change.
// The writers are therefore UNCALLED public procedures; transaction-span seed
// discovery scans routines without requiring root reachability. The shared
// worker's OnRun is empty and never calls back into a writer.
codeunit 50220 "D50 TG Writers"
{
    // 3 tables written, ALL temporary → inclusive 3 / physical 0.
    // Non-posting name → only the COUNT branch can make this routine a manager.
    procedure BufferRows()
    var
        TempBufHdr: Record "D50 TG Header" temporary;
        TempBufLine: Record "D50 TG Line" temporary;
        TempBufEntry: Record "D50 TG Entry" temporary;
    begin
        TempBufHdr.Init();
        TempBufHdr.Insert();
        TempBufLine.Init();
        TempBufLine.Insert();
        TempBufEntry.Init();
        TempBufEntry.Insert();
        if Codeunit.Run(Codeunit::"D50 TG Worker") then;
    end;

    // 3 tables written, ALL physical → inclusive 3 / physical 3.
    // The retained control: fires before AND after the fix.
    procedure StageRows()
    var
        StageHdr: Record "D50 TG Header";
        StageLine: Record "D50 TG Line";
        StageEntry: Record "D50 TG Entry";
    begin
        StageHdr.Init();
        StageHdr.Insert();
        StageLine.Init();
        StageLine.Insert();
        StageEntry.Init();
        StageEntry.Insert();
        if Codeunit.Run(Codeunit::"D50 TG Worker") then;
    end;

    // A same-shape clone of StageRows. The ONLY difference is the THIRD local
    // record declaration carrying `temporary` → inclusive 3 / physical 2, which
    // drops below TRANSACTION_THRESHOLD_TABLES (3) once the gate counts
    // physical writes.
    procedure StageMixedRows()
    var
        MixedHdr: Record "D50 TG Header";
        MixedLine: Record "D50 TG Line";
        MixedEntry: Record "D50 TG Entry" temporary;
    begin
        MixedHdr.Init();
        MixedHdr.Insert();
        MixedLine.Init();
        MixedLine.Insert();
        MixedEntry.Init();
        MixedEntry.Insert();
        if Codeunit.Run(Codeunit::"D50 TG Worker") then;
    end;

    // BufferRows' writes with a POSTING-style name: `^(Post|Apply|Release)[A-Z]`
    // matches, so the NAME branch short-circuits BEFORE the count is consulted.
    // Pins that the fix narrows the COUNT branch only.
    procedure PostBuffers()
    var
        TempPostHdr: Record "D50 TG Header" temporary;
        TempPostLine: Record "D50 TG Line" temporary;
        TempPostEntry: Record "D50 TG Entry" temporary;
    begin
        TempPostHdr.Init();
        TempPostHdr.Insert();
        TempPostLine.Init();
        TempPostLine.Insert();
        TempPostEntry.Init();
        TempPostEntry.Insert();
        if Codeunit.Run(Codeunit::"D50 TG Worker") then;
    end;
}

// The shared checked-Run target. Its OnRun is empty and calls back into no
// writer — part of the isolation contract.
codeunit 50221 "D50 TG Worker"
{
    trigger OnRun()
    begin
        // intentionally empty
    end;
}

table 50220 "D50 TG Header"
{
    fields
    {
        field(1; "No."; Code[20]) { }
        field(2; Status; Integer) { }
    }
}

table 50221 "D50 TG Line"
{
    fields
    {
        field(1; "Doc No."; Code[20]) { }
        field(2; Status; Integer) { }
    }
}

table 50222 "D50 TG Entry"
{
    fields
    {
        field(1; "Entry No."; Integer) { }
    }
}

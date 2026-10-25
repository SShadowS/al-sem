table 50270 "S Cust"
{
    fields
    {
        field(1; "No."; Code[20]) { }
        field(2; Name; Text[100]) { }
        field(3; Address; Text[100]) { }
        field(4; Amount; Decimal)
        {
            trigger OnValidate()
            var H: Codeunit "S H";
            begin
                // T7: field OnValidate forwards Rec to a dirtying helper (caller of Validate persists)
                H.Dirty(Rec);
                // T8: xRec forwarded to a reset helper, then a filter-sensitive op
                xRec.SetRange("No.", 'A');
                H.ResetIt(xRec);
                if xRec.IsEmpty() then;
            end;
        }
    }
    keys { key(PK; "No.") { } }

    trigger OnModify()
    var H: Codeunit "S H";
    begin
        // T9: table-level trigger, auto-persist
        H.Dirty(Rec);
    end;

    // T1 d41 Rec vs local
    procedure D41ViaRec()
    var H: Codeunit "S H";
    begin
        Rec.SetRange("No.", 'A');
        H.ResetIt(Rec);
        Rec.DeleteAll();
    end;

    procedure D41ViaLocal()
    var H: Codeunit "S H"; C: Record "S Cust";
    begin
        C.SetRange("No.", 'A');
        H.ResetIt(C);
        C.DeleteAll();
    end;

    // T1b bare filter calls on the implicit Rec
    procedure D41ViaBareRec()
    var H: Codeunit "S H";
    begin
        SetRange("No.", 'A');
        H.ResetIt(Rec);
        DeleteAll();
    end;

    // T2 d39 Rec (table procedure: dirt goes back to the caller) vs local
    procedure D39ViaRec()
    var H: Codeunit "S H";
    begin
        H.Dirty(Rec);
    end;

    procedure D39ViaLocal()
    var H: Codeunit "S H"; C: Record "S Cust";
    begin
        C.Get('A');
        H.Dirty(C);
    end;

    // T3 d42 Rec vs local
    procedure D42ViaRec()
    var H: Codeunit "S H";
    begin
        Rec.SetLoadFields("No.", Name);
        Rec.FindFirst();
        H.ReadsAddress(Rec);
    end;

    procedure D42ViaLocal()
    var H: Codeunit "S H"; C: Record "S Cust";
    begin
        C.SetLoadFields("No.", Name);
        C.FindFirst();
        H.ReadsAddress(C);
    end;

    // T4 d40 Rec vs local (d40 is opt-in)
    procedure D40ViaRec()
    var H: Codeunit "S H";
    begin
        H.NeedsLoaded(Rec);
    end;

    procedure D40ViaLocal()
    var H: Codeunit "S H"; C: Record "S Cust";
    begin
        H.NeedsLoaded(C);
    end;
}

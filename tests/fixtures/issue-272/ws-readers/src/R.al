table 50280 "R Cust"
{
    fields
    {
        field(1; "No."; Code[20]) { }
        field(2; Name; Text[100]) { }
    }
    keys { key(PK; "No.") { } }

    // ---- d37 helper persistence (Validate on Rec, then a helper) ----
    procedure D37Persists()       // helper Modify()s Rec by var -> suppressed
    var H: Codeunit "R H";
    begin
        Validate(Name, 'x');
        H.Persist(Rec);
    end;

    procedure D37NoPersist()      // helper only reads -> still fires
    var H: Codeunit "R H";
    begin
        Validate(Name, 'x');
        H.ReadOnly(Rec);
    end;

    procedure D37ByValue()        // by-value helper cannot persist the caller's Rec -> fires
    var H: Codeunit "R H";
    begin
        Validate(Name, 'x');
        H.PersistByVal(Rec);
    end;

    procedure D37Unresolved()     // no such procedure -> suppress-unknown
    var H: Codeunit "R H";
    begin
        Validate(Name, 'x');
        H.NoSuchProc(Rec);
    end;

    procedure D37Arity()          // arity mismatch -> suppress-unknown
    var H: Codeunit "R H";
    begin
        Validate(Name, 'x');
        H.Persist(Rec, 1);
    end;

    procedure D37WrongVar()       // the helper gets another record -> fires
    var H: Codeunit "R H"; Other: Record "R Cust";
    begin
        Validate(Name, 'x');
        H.Persist(Other);
    end;

    procedure D37Order()          // the helper runs BEFORE the Validate -> fires
    var H: Codeunit "R H";
    begin
        H.Persist(Rec);
        Validate(Name, 'x');
    end;

    // ---- d11 / d21: a helper that loads Rec by var ----
    procedure D11Loaded()         // helper Get()s Rec, then Modify -> no d11
    var H: Codeunit "R H";
    begin
        H.Loads(Rec);
        Modify();
    end;

    procedure D11ByValue()        // by-value loader -> d11 control
    var H: Codeunit "R H";
    begin
        H.LoadsByVal(Rec);
        Modify();
    end;

    procedure D11Order()          // load AFTER the Modify -> d11 control
    var H: Codeunit "R H";
    begin
        Modify();
        H.Loads(Rec);
    end;

    procedure D21Loaded()         // helper loads, then a read -> no d21
    var H: Codeunit "R H";
    begin
        H.Loads(Rec);
        TestField(Name);
    end;

    procedure D21WrongVar()       // helper loads another record -> d21 control
    var H: Codeunit "R H"; Other: Record "R Cust";
    begin
        H.Loads(Other);
        TestField(Name);
    end;

    // ---- d33: a helper that filters Rec by var ----
    procedure D33Filtered()       // helper SetRange()s Rec, then DeleteAll -> no d33
    var H: Codeunit "R H";
    begin
        H.Filters(Rec);
        DeleteAll();
    end;

    procedure D33ByValue()        // by-value filter -> d33 control
    var H: Codeunit "R H";
    begin
        H.FiltersByVal(Rec);
        DeleteAll();
    end;

    procedure D33Unfiltered()     // no filter at all -> d33 control
    begin
        DeleteAll();
    end;

    // ---- the same shapes on a local record (reference) ----
    procedure LocalD37Persists()
    var H: Codeunit "R H"; C: Record "R Cust";
    begin
        C.Get('A');
        C.Validate(Name, 'x');
        H.Persist(C);
    end;

    procedure LocalD33Filtered()
    var H: Codeunit "R H"; C: Record "R Cust";
    begin
        H.Filters(C);
        C.DeleteAll();
    end;

    procedure LocalD11Loaded()
    var H: Codeunit "R H"; C: Record "R Cust";
    begin
        H.Loads(C);
        C.Modify();
    end;
}

codeunit 50281 "R H"
{
    procedure Persist(var R: Record "R Cust")
    begin
        R.Modify();
    end;

    procedure ReadOnly(var R: Record "R Cust")
    begin
        Message(R.Name);
    end;

    procedure PersistByVal(R: Record "R Cust")
    begin
        R.Modify();
    end;

    procedure Loads(var R: Record "R Cust")
    begin
        R.Get('A');
    end;

    procedure LoadsByVal(R: Record "R Cust")
    begin
        R.Get('A');
    end;

    procedure Filters(var R: Record "R Cust")
    begin
        R.SetRange("No.", 'A');
    end;

    procedure FiltersByVal(R: Record "R Cust")
    begin
        R.SetRange("No.", 'A');
    end;
}

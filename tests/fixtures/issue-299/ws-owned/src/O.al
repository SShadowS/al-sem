table 50290 "O Cust"
{
    fields
    {
        field(1; "No."; Code[20]) { }
        field(2; Name; Text[100])
        {
            trigger OnLookup()
            begin
                Validate(Name, 'L');       // table field OnLookup -> no finding
            end;
        }
    }
    keys { key(PK; "No.") { } }

    procedure ProcModify()                 // receiver Modify, no load -> no d11
    begin
        Modify();
    end;

    procedure ProcValidate()               // receiver Validate, nothing after -> no d11, no d37
    begin
        Validate(Name, 'x');
    end;

    procedure ProcValidateThenNext()       // own reload after -> d37 fires
    begin
        Validate(Name, 'x');
        if Next() = 0 then;
    end;

    procedure ProcValidateThenHelperLoad() // var helper reload after -> d37 fires
    var H: Codeunit "O H";
    begin
        Validate(Name, 'x');
        H.Loads(Rec);
    end;

    procedure ProcValidateThenAssign()     // whole-record replacement after -> d37 fires
    var Other: Record "O Cust";
    begin
        Validate(Name, 'x');
        Rec := Other;
    end;

    procedure ProcValidateThenReset()      // Reset only clears filters -> no d37
    begin
        Validate(Name, 'x');
        Reset();
    end;

    procedure ProcShadow()                 // declared local Rec -> d11 x2; no d37 (Modify follows)
    var Rec: Record "O Cust";
    begin
        Rec.Validate(Name, 'x');
        Rec.Modify();
    end;

    procedure ProcNamedReturn() Rec: Record "O Cust" // named return Rec -> fail closed
    begin
        Rec.Validate(Name, 'x');
    end;

    procedure ProcLocal()                  // local record control -> d11 x2; no d37 (Modify follows)
    var Other: Record "O Cust";
    begin
        Other.Validate(Name, 'x');
        Other.Modify();
    end;

    procedure ProcShadowRecRef()           // declared local Rec: RecordRef -> no seeded row, not the receiver
    var Rec: RecordRef;
    begin
        Rec.Open(Database::"O Cust");
        Rec.Modify();
    end;

    procedure ProcValidateOppositeBranchAssign(B: Boolean) // assignment in the other arm -> d37 kept (conservative)
    var Other: Record "O Cust";
    begin
        if B then
            Validate(Name, 'x')
        else
            Rec := Other;
    end;
}

table 50295 "O Named"
{
    fields { field(1; "No."; Code[20]) { } field(2; Name; Text[100]) { } field(3; Rec; Text[10]) { } }
    keys { key(PK; "No.") { } }

    procedure MemberRecAssign()            // member target X.Rec := v is not a receiver reset -> no d37
    var Other: Record "O Named";
    begin
        Validate(Name, 'x');
        Other.Rec := 'v';
    end;
}

table 50291 "O Temp"
{
    TableType = Temporary;
    fields { field(1; "No."; Code[20]) { } field(2; Name; Text[100]) { } }
    keys { key(PK; "No.") { } }

    procedure TempModify()                 // receiver of a temporary table -> no d11
    begin
        Modify();
    end;
}

tableextension 50292 "O Cust Ext" extends "O Cust"
{
    procedure ExtProc()                    // -> no finding
    begin
        Validate(Name, 'e');
    end;

    trigger OnAfterModify()
    begin
        Validate(Name, 'y');               // after-write trigger -> d37 fires, no d11
    end;
}

page 50293 "O Card"
{
    PageType = Card;
    SourceTable = "O Cust";
    layout { area(Content) { field(NameF; Rec.Name) { } } }

    // page copies of #272's reader shapes (neither new rule covers pages)
    procedure D37Persists()
    var H: Codeunit "O H";
    begin
        Rec.Validate(Name, 'x');
        H.Persist(Rec);
    end;

    procedure D37NoPersist()
    var H: Codeunit "O H";
    begin
        Rec.Validate(Name, 'x');
        H.ReadOnly(Rec);
    end;

    procedure D37ByValue()
    var H: Codeunit "O H";
    begin
        Rec.Validate(Name, 'x');
        H.PersistByVal(Rec);
    end;

    procedure D37Unresolved()
    var H: Codeunit "O H";
    begin
        Rec.Validate(Name, 'x');
        H.NoSuchProc(Rec);
    end;

    procedure D37Arity()
    var H: Codeunit "O H";
    begin
        Rec.Validate(Name, 'x');
        H.Persist(Rec, 1);
    end;

    procedure D37WrongVar()
    var H: Codeunit "O H"; Other: Record "O Cust";
    begin
        Rec.Validate(Name, 'x');
        H.Persist(Other);
    end;

    procedure D37Order()
    var H: Codeunit "O H";
    begin
        H.Persist(Rec);
        Rec.Validate(Name, 'x');
    end;

    procedure D11Loaded()
    var H: Codeunit "O H";
    begin
        H.Loads(Rec);
        Rec.Modify();
    end;

    procedure D11ByValue()
    var H: Codeunit "O H";
    begin
        H.LoadsByVal(Rec);
        Rec.Modify();
    end;

    procedure D11Order()
    var H: Codeunit "O H";
    begin
        Rec.Modify();
        H.Loads(Rec);
    end;
}

codeunit 50294 "O H"
{
    procedure Persist(var R: Record "O Cust")
    begin
        R.Modify();
    end;

    procedure ReadOnly(var R: Record "O Cust")
    begin
        Message(R.Name);
    end;

    procedure PersistByVal(R: Record "O Cust")
    begin
        R.Modify();
    end;

    procedure Loads(var R: Record "O Cust")
    begin
        R.Get('A');
    end;

    procedure LoadsByVal(R: Record "O Cust")
    begin
        R.Get('A');
    end;

    procedure LocalControl()               // codeunit local control -> d11 x2; no d37 (Modify follows)
    var C: Record "O Cust";
    begin
        C.Validate(Name, 'x');
        C.Modify();
    end;
}

table 50296 "O Repl"
{
    fields { field(1; "No."; Code[20]) { } field(2; Name; Text[100]) { } }
    keys { key(PK; "No.") { } }

    procedure ProcAssignThenModify()       // routine replaced Rec with an unloaded record -> d11 fires
    var Other: Record "O Repl";
    begin
        Rec := Other;
        Modify();
    end;

    procedure ProcModifyThenAssign()       // replacement after the Modify -> no d11
    var Other: Record "O Repl";
    begin
        Modify();
        Rec := Other;
    end;

    procedure ProcValidateThenClear()      // Clear(Rec) drops the Validate, but d37's helper rule
    begin                                  // (unresolved call on Rec) suppresses it first -> no d37 (limit)
        Validate(Name, 'x');
        Clear(Rec);
    end;
}

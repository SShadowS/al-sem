// Global shadows of the implicit Rec. alc rejects these (AL0155: 'Rec' is already
// defined), so this is a robustness fixture, not real code.
table 50300 "G Cust"
{
    fields { field(1; "No."; Code[20]) { } }
    keys { key(PK; "No.") { } }
}

table 50301 "G Other"
{
    fields { field(1; "No."; Code[20]) { } }
    keys { key(PK; "No.") { } }
}

codeunit 50302 "G H"
{
    procedure Num(var I: Integer)
    begin
        I := 1;
    end;

    procedure ResetIt(var R: Record "G Other")
    begin
        R.Reset();
    end;

    procedure ResetCust(var R: Record "G Cust")
    begin
        R.Reset();
    end;
}

// GT1: table with a global scalar Rec
table 50303 "G TScalar"
{
    fields { field(1; "No."; Code[20]) { } }
    keys { key(PK; "No.") { } }
    var
        Rec: Integer;

    procedure ViaScalar()
    var H: Codeunit "G H";
    begin
        H.Num(Rec);
    end;
}

// GT2: table with a global record Rec of ANOTHER table
table 50304 "G TRecord"
{
    fields { field(1; "No."; Code[20]) { } }
    keys { key(PK; "No.") { } }
    var
        Rec: Record "G Other";

    procedure ViaRecord()
    var H: Codeunit "G H";
    begin
        Rec.SetRange("No.", 'A');
        H.ResetIt(Rec);
        Rec.FindFirst();
    end;
}

// GP1: page with a global scalar Rec
page 50305 "G PScalar"
{
    SourceTable = "G Cust";
    var
        Rec: Integer;

    procedure ViaScalar()
    var H: Codeunit "G H";
    begin
        H.Num(Rec);
    end;
}

// GP2: page with a global record Rec of ANOTHER table
page 50306 "G PRecord"
{
    SourceTable = "G Cust";
    var
        Rec: Record "G Other";

    procedure ViaRecord()
    var H: Codeunit "G H";
    begin
        Rec.SetRange("No.", 'A');
        H.ResetIt(Rec);
        Rec.FindFirst();
    end;
}

// GC1 (O15): a codeunit TableNo Rec is typed directly; an object global named Rec
// does not take it away (the global-shadow rule is for objects whose own record is
// Rec). Not checked against alc.
codeunit 50307 "G TableNo"
{
    TableNo = "G Cust";
    var
        Rec: Record "G Other";

    trigger OnRun()
    var H: Codeunit "G H";
    begin
        H.ResetCust(Rec);
    end;
}

// GR1 (O15): the same for a report dataitem Rec. Not checked against alc.
report 50308 "G Report"
{
    dataset
    {
        dataitem(D1; "G Cust")
        {
            trigger OnAfterGetRecord()
            var H: Codeunit "G H";
            begin
                H.ResetCust(Rec);
            end;
        }
    }
    var
        Rec: Record "G Other";
}

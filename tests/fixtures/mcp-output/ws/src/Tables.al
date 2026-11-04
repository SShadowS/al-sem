table 50100 "Out Doc"
{
    fields
    {
        field(1; "No."; Code[20]) { }
        field(2; Status; Option) { OptionMembers = Open,Issued; }
        field(3; Amount; Decimal) { }
    }
    keys { key(PK; "No.") { Clustered = true; } }
}

table 50102 "Out Item"
{
    fields
    {
        field(1; "No."; Code[20]) { }
        field(2; Status; Option) { OptionMembers = Open,Closed; }
        field(3; Qty; Decimal) { }
    }
    keys { key(PK; "No.") { Clustered = true; } }
}

table 50103 "Out Bin"
{
    fields
    {
        field(1; Code; Code[20]) { }
    }
    keys { key(PK; Code) { Clustered = true; } }
}

table 50101 "Out Log"
{
    fields
    {
        field(1; "Entry No."; Integer) { }
        field(2; Message; Text[100]) { }
    }
    keys { key(PK; "Entry No.") { Clustered = true; } }
}

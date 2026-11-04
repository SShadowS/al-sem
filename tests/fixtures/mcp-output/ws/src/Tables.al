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

table 50101 "Out Log"
{
    fields
    {
        field(1; "Entry No."; Integer) { }
        field(2; Message; Text[100]) { }
    }
    keys { key(PK; "Entry No.") { Clustered = true; } }
}

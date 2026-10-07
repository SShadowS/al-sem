table 50100 "IS Rec"
{
    fields
    {
        field(1; "No."; Integer) { }
        field(2; "Name"; Text[50]) { }
        // Unread by UnsuppressedD3: SetLoadFields(Name) would leave it unloaded,
        // so its d3 is genuine (a key + Name table has nothing to trim).
        field(3; "City"; Text[50]) { }
    }
    keys
    {
        key(PK; "No.") { Clustered = true; }
    }
}

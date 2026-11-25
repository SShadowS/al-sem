table 50800 Customer
{
    fields
    {
        field(1; "No."; Code[20]) { }
        field(2; Name; Text[100]) { }
        field(3; "Balance (LCY)"; Decimal) { FieldClass = FlowField; CalcFormula = sum("Ledger Entry".Amount where("Customer No." = field("No."))); }
    }
    keys { key(PK; "No.") { } }
}

table 50801 "Ledger Entry"
{
    fields
    {
        field(1; "Entry No."; Integer) { }
        field(2; "Customer No."; Code[20]) { }
        field(3; Amount; Decimal) { }
    }
    keys { key(PK; "Entry No.") { } }
}

table 50802 Vendor
{
    fields
    {
        field(1; "No."; Code[20]) { }
        field(3; "Balance (LCY)"; Decimal) { FieldClass = FlowField; CalcFormula = sum("Ledger Entry".Amount where("Customer No." = field("No."))); }
    }
    keys { key(PK; "No.") { } }
}

table 50650 "Guard Order"
{
    Caption = 'Guard Order';

    fields
    {
        field(1; "No."; Code[20]) { }
        field(2; Status; Option)
        {
            OptionMembers = Open,Released,Posted;
        }
        field(3; Amount; Decimal) { }
        field(4; Quantity; Decimal) { }
        field(5; Price; Decimal) { }
        field(6; Discount; Decimal) { }
        field(7; Note; Text[50]) { }
        field(8; Ref; Code[20]) { }
        field(9; "Code"; Code[20]) { }
        field(10; Blocked; Boolean) { }
    }
}

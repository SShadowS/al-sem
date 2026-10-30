table 50700 "Pin Order"
{
    Caption = 'Pin Order';

    fields
    {
        field(1; "No."; Code[20]) { }
        field(2; Status; Option)
        {
            OptionMembers = Open,Released;
        }
        field(3; Amount; Decimal) { }
    }
}

table 50400 "Explain Order"
{
    Caption = 'Explain Order';

    fields
    {
        field(1; "No."; Code[20])
        {
            Caption = 'Order No.';
        }
        field(2; Status; Option)
        {
            Caption = 'Order Status', Comment = 'The posting state';
            OptionMembers = Open,Released,Posted,Archived;
        }
        field(3; Amount; Decimal)
        {
            Caption = 'Amount';

            trigger OnValidate()
            begin
                TestField(Status, Status::Open);
            end;
        }
        field(4; Posted; Boolean) { }
    }
}

table 50300 "Rules Order"
{
    fields
    {
        field(1; "No."; Code[20]) { }
        field(2; "Document Type"; Option)
        {
            OptionMembers = Quote,Order;
        }
        field(3; Status; Option)
        {
            OptionMembers = Open,Released,Pending;
        }
        field(4; "Customer No."; Code[20]) { }
        field(5; Amount; Decimal)
        {
            trigger OnValidate()
            begin
                TestField(Status, Status::Open);
                if Amount < 0 then
                    FieldError(Amount, NegativeErr); // amounts are never negative
            end;
        }
        field(6; Blocked; Boolean) { }
        field(7; "Posting Date"; Date) { }
    }

    var
        NegativeErr: Label 'must not be negative';
}

table 50304 "Rules Customer"
{
    fields
    {
        field(1; "No."; Code[20]) { }
        field(2; Blocked; Boolean) { }
        field(3; "Credit Limit"; Decimal) { }
    }
}

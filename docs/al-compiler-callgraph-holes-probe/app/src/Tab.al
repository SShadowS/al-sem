table 50100 "P Tab"
{
    fields
    {
        field(1; "No."; Code[20]) { }
        field(2; Amount; Decimal)
        {
            trigger OnValidate()
            begin
                Helper();
            end;
        }
        field(3; Qty; Decimal)
        {
            trigger OnValidate()
            begin
                Helper();
            end;
        }
    }
    trigger OnInsert()
    begin
        Helper();
    end;

    trigger OnModify()
    begin
        Helper();
    end;

    trigger OnDelete()
    begin
        Helper();
    end;

    procedure Helper()
    begin
    end;

    procedure SelfInsert()
    begin
        Insert(true);
    end;
}

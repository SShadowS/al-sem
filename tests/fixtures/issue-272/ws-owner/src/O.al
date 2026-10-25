// #272 final panel r1: d39 owner-rule cases kept out of ws-shapes, whose R3a-2
// summary core is pinned against the pre-#272 base (A8).
table 50310 "O Cust"
{
    fields { field(1; "No."; Code[20]) { } field(2; Name; Text[50]) { } }
    keys { key(PK; "No.") { } }
}

// F1: a TableType = Temporary table; a page over it has no SourceTableTemporary.
table 50311 "O Temp"
{
    TableType = Temporary;
    fields { field(1; "No."; Code[20]) { } field(2; Name; Text[50]) { } }
    keys { key(PK; "No.") { } }
}

codeunit 50312 "O H"
{
    procedure Dirty(var R: Record "O Cust")
    begin
        R.Validate(Name, 'x');
    end;

    procedure DirtyT(var R: Record "O Temp")
    begin
        R.Validate(Name, 'x');
    end;
}

page 50313 "O Card"
{
    PageType = Card;
    SourceTable = "O Cust";

    // C1 control: OnAfterGetRecord loses the write (measured) -> d39 reports
    trigger OnAfterGetRecord()
    var H: Codeunit "O H";
    begin
        H.Dirty(Rec);
    end;

    // F2: the platform deletes Rec after OnDeleteRecord: a write is moot -> no d39
    trigger OnDeleteRecord(): Boolean
    var H: Codeunit "O H";
    begin
        H.Dirty(Rec);
    end;
}

page 50314 "O Temp Card"
{
    PageType = Card;
    SourceTable = "O Temp";

    actions
    {
        area(Processing)
        {
            action(T1)
            {
                // F1: Rec is temporary (TableType) -> no d39 (G-13)
                trigger OnAction()
                var H: Codeunit "O H";
                begin
                    H.DirtyT(Rec);
                end;
            }
        }
    }

    // F1: Rec is temporary (TableType) -> no d39 (G-13), although OnAfterGetRecord
    // is not a persisting trigger
    trigger OnAfterGetRecord()
    var H: Codeunit "O H";
    begin
        H.DirtyT(Rec);
    end;
}

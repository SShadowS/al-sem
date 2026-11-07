table 51200 "RB Ledger"
{
    fields
    {
        field(1; "No."; Code[20])
        {
            trigger OnValidate()
            begin
                Amount := 0;
            end;
        }
        field(2; Amount; Decimal)
        {
            trigger OnLookup()
            begin
            end;
        }
    }

    trigger OnInsert()
    begin
    end;
}

page 51201 "RB Ledger Card"
{
    PageType = Card;
    SourceTable = "RB Ledger";
}

table 51202 "RB Other"
{
    fields
    {
        field(1; Code; Code[10])
        {
        }
    }
}

page 51203 "RB Actions"
{
    PageType = List;
    SourceTable = "RB Other";

    actions
    {
        area(Processing)
        {
            action(Direct)
            {
                trigger OnAction()
                var
                    Mgt: Codeunit "RB Mgt";
                begin
                    Mgt.Target();
                end;
            }
            action(Indirect)
            {
                trigger OnAction()
                var
                    Mgt: Codeunit "RB Mgt";
                begin
                    Mgt.Middle();
                end;
            }
        }
    }
}

codeunit 51204 "RB Mgt"
{
    procedure Target()
    begin
    end;

    procedure Middle()
    begin
        Target();
    end;

    procedure ProcB()
    begin
        Target();
    end;

    procedure Validator()
    var
        Ledger: Record "RB Ledger";
    begin
        Ledger.Validate("No.", 'X');
    end;
}

codeunit 51205 "RB Subscriber"
{
    [EventSubscriber(ObjectType::Table, Database::"RB Ledger", 'OnAfterInsertEvent', '', false, false)]
    local procedure OnAfterInsertLedger(var Rec: Record "RB Ledger"; RunTrigger: Boolean)
    begin
    end;
}

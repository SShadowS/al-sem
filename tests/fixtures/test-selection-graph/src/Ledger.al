table 50950 "Sel Ledger"
{
    LookupPageId = "Sel Ledger List";

    fields
    {
        field(1; "No."; Code[20])
        {
            trigger OnValidate()
            var
                Hooks: Codeunit "Sel Ledger Hooks";
            begin
                Hooks.NoValidated();
            end;
        }
        field(2; Amount; Decimal)
        {
        }
    }

    trigger OnInsert()
    var
        Hooks: Codeunit "Sel Ledger Hooks";
    begin
        Hooks.Stamp();
    end;
}

page 50951 "Sel Ledger Card"
{
    PageType = Card;
    SourceTable = "Sel Ledger";

    layout
    {
        area(Content)
        {
            field("No."; Rec."No.")
            {
                trigger OnValidate()
                var
                    Hooks: Codeunit "Sel Ledger Hooks";
                begin
                    Hooks.CardNoValidated();
                end;
            }
        }
    }

    actions
    {
        area(Processing)
        {
            action(Recalc)
            {
                trigger OnAction()
                var
                    Hooks: Codeunit "Sel Ledger Hooks";
                begin
                    Hooks.RecalcFromPage();
                end;
            }
            action(History)
            {
                RunObject = Page "Sel Ledger History";
            }
        }
    }

    trigger OnAfterGetRecord()
    var
        Hooks: Codeunit "Sel Ledger Hooks";
    begin
        Hooks.Describe();
    end;
}

page 50952 "Sel Ledger List"
{
    PageType = List;
    SourceTable = "Sel Ledger";

    trigger OnOpenPage()
    var
        Hooks: Codeunit "Sel Ledger Hooks";
    begin
        Hooks.ListOpened();
    end;
}

page 50953 "Sel Ledger History"
{
    PageType = List;
    SourceTable = "Sel Ledger";

    trigger OnOpenPage()
    var
        Hooks: Codeunit "Sel Ledger Hooks";
    begin
        Hooks.HistoryOpened();
    end;
}

report 50980 "Sel Report"
{
    trigger OnPreReport()
    var
        Hooks: Codeunit "Sel Ledger Hooks";
    begin
        Hooks.ReportHook();
    end;
}

codeunit 50954 "Sel Ledger Hooks"
{
    procedure NoValidated()
    begin
    end;

    procedure CardNoValidated()
    begin
    end;

    procedure Stamp()
    begin
    end;

    procedure RecalcFromPage()
    begin
    end;

    procedure Describe()
    begin
    end;

    procedure ListOpened()
    begin
    end;

    procedure HistoryOpened()
    begin
    end;

    procedure ReportHook()
    begin
    end;

    procedure AfterInsertHook()
    begin
    end;

    procedure InModalHandler()
    begin
    end;

    procedure OpenCardModal()
    begin
        Page.RunModal(Page::"Sel Ledger Card");
    end;
}

codeunit 50955 "Sel Ledger Subscriber"
{
    [EventSubscriber(ObjectType::Table, Database::"Sel Ledger", 'OnAfterInsertEvent', '', false, false)]
    local procedure OnAfterInsertLedger(var Rec: Record "Sel Ledger"; RunTrigger: Boolean)
    var
        Hooks: Codeunit "Sel Ledger Hooks";
    begin
        Hooks.AfterInsertHook();
    end;
}

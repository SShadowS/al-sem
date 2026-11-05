page 51000 "CR Lifecycle Page"
{
    PageType = Card;
    SourceTable = "CR Entry";

    trigger OnOpenPage()
    var
        T: Codeunit "CR Target";
    begin
        T.Target();
    end;
}

page 51001 "CR Extended Page"
{
    PageType = Card;
    SourceTable = "CR Entry";
}

pageextension 51002 "CR Extension" extends "CR Extended Page"
{
    actions
    {
        addlast(Processing)
        {
            action(ExtGo)
            {
                trigger OnAction()
                var
                    T: Codeunit "CR Target";
                begin
                    T.Target();
                end;
            }
        }
    }

    trigger OnOpenPage()
    var
        T: Codeunit "CR Target";
    begin
        T.Target();
    end;
}

page 51003 "CR Action Page"
{
    PageType = Card;
    SourceTable = "CR Entry";

    actions
    {
        area(Processing)
        {
            action(Go)
            {
                trigger OnAction()
                var
                    T: Codeunit "CR Target";
                begin
                    T.Target();
                end;
            }
        }
    }
}

page 51004 "CR Part Page"
{
    PageType = CardPart;
    SourceTable = "CR Entry";

    trigger OnOpenPage()
    var
        T: Codeunit "CR Target";
    begin
        T.Target();
    end;
}

page 51005 "CR Host Page"
{
    PageType = Card;
    SourceTable = "CR Entry";

    layout
    {
        area(FactBoxes)
        {
            part(Info; "CR Part Page") { }
        }
    }
}

codeunit 51010 "CR Openers"
{
    procedure OpenLifecycle()
    begin
        Page.Run(Page::"CR Lifecycle Page");
    end;

    procedure OpenExtended()
    begin
        Page.Run(Page::"CR Extended Page");
    end;

    procedure OpenAction()
    begin
        Page.Run(Page::"CR Action Page");
    end;

    procedure OpenWithPart()
    begin
        Page.Run(Page::"CR Host Page");
    end;
}

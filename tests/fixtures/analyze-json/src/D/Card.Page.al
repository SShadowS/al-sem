page 50107 "Probe Card"
{
    PageType = Card;
    SourceTable = "Probe Base";

    layout
    {
        area(Content)
        {
            field("Code"; Rec."Code")
            {
            }
        }
    }

    actions
    {
        area(Processing)
        {
            action(First)
            {
                trigger OnAction()
                begin
                end;
            }
            action(Second)
            {
                trigger OnAction()
                begin
                end;
            }
        }
    }
}

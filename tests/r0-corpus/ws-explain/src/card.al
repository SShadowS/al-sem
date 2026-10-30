page 50404 "Explain Order Card"
{
    PageType = Card;
    SourceTable = "Explain Order";
    Caption = 'Order Card';

    layout
    {
        area(Content)
        {
            field("No."; Rec."No.") { }
        }
    }

    actions
    {
        area(Processing)
        {
            action(Post)
            {
                Caption = 'P&ost';
                ToolTip = 'Post the order.';

                trigger OnAction()
                var
                    Posting: Codeunit "Explain Post";
                begin
                    Posting.PostOrder(Rec);
                end;
            }
        }
    }
}

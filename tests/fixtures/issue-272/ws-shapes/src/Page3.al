page 50290 "S NoMod Card"
{
    PageType = Card;
    SourceTable = "S Cust";
    ModifyAllowed = false;
    layout { area(Content) { field(Name; Rec.Name) { } } }
    actions
    {
        area(Processing)
        {
            action(M1)
            {
                trigger OnAction()
                var H: Codeunit "S H";
                begin
                    // M1: ModifyAllowed=false page, the write is lost (measured) -> report
                    H.Dirty(Rec);
                end;
            }
        }
    }
}

pageextension 50291 "S RO List Ext" extends "S RO List"
{
    actions
    {
        addlast(Processing)
        {
            action(RX1)
            {
                trigger OnAction()
                var H: Codeunit "S H";
                begin
                    // RX1: pageext action on a read-only list (measured lost) -> report
                    H.Dirty(Rec);
                end;
            }
        }
    }
}

pageextension 50292 "S Card Ext2" extends "S Card"
{
    actions
    {
        modify(P1)
        {
            trigger OnAfterAction()
            var H: Codeunit "S H";
            begin
                // PX1: pageext OnAfterAction on an editable card (measured persisted) -> no d39
                H.Dirty(Rec);
            end;
        }
    }
}

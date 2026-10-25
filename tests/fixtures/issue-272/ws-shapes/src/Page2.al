page 50276 "S RO List"
{
    PageType = List;
    SourceTable = "S Cust";
    Editable = false;
    layout { area(Content) { repeater(G) { field(Name; Rec.Name) { } } } }
    actions
    {
        area(Processing)
        {
            action(R1)
            {
                trigger OnAction()
                var H: Codeunit "S H";
                begin
                    // R1: non-editable page, the write is lost -> report
                    H.Dirty(Rec);
                end;
            }
        }
    }
}

page 50277 "S AGR Card"
{
    PageType = Card;
    SourceTable = "S Cust";
    layout { area(Content) { field(Name; Rec.Name) { } } }

    trigger OnAfterGetRecord()
    var H: Codeunit "S H";
    begin
        // R2: OnAfterGetRecord is not persisted -> report
        H.Dirty(Rec);
    end;
}

codeunit 50278 "S Global Rec"
{
    var
        Rec: Record "S Cust";

    // G1: a codeunit global named Rec is a real record (not implicit)
    procedure ViaGlobalRec()
    var H: Codeunit "S H";
    begin
        Rec.SetRange("No.", 'A');
        H.ResetIt(Rec);
        Rec.FindFirst();
    end;
}

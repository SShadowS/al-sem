page 50272 "S Card"
{
    SourceTable = "S Cust";
    PageType = Card;

    layout
    {
        area(Content)
        {
            field(Name; Rec.Name)
            {
                trigger OnValidate()
                var H: Codeunit "S H";
                begin
                    // P3: page field OnValidate dirties Rec through a helper (page persists)
                    H.Dirty(Rec);
                end;
            }
        }
    }
    actions
    {
        area(Processing)
        {
            action(P1)
            {
                trigger OnAction()
                var H: Codeunit "S H";
                begin
                    // P1 d41 on page Rec
                    Rec.SetRange("No.", 'A');
                    H.ResetIt(Rec);
                    Rec.FindFirst();
                end;
            }
            action(P2)
            {
                trigger OnAction()
                var H: Codeunit "S H";
                begin
                    // P2 d39 on page Rec in an action
                    H.Dirty(Rec);
                end;
            }
            action(P4)
            {
                trigger OnAction()
                var H: Codeunit "S H";
                begin
                    // P4 d42 on page Rec
                    Rec.SetLoadFields("No.");
                    Rec.FindFirst();
                    H.ReadsAddress(Rec);
                end;
            }
        }
    }

    // P7 card page procedure dirties Rec through a helper (measured persisted) -> no d39
    procedure PageProcDirty()
    var H: Codeunit "S H";
    begin
        H.Dirty(Rec);
    end;

    // P5 page procedure
    procedure PageProcViaRec()
    var H: Codeunit "S H";
    begin
        CurrPage.SetSelectionFilter(Rec);
        H.ResetIt(Rec);
        Rec.DeleteAll();
    end;

    // S1 scalar shadow: a local Integer named Rec is not the implicit Rec
    procedure ShadowScalar()
    var H: Codeunit "S H"; Rec: Integer;
    begin
        H.Num(Rec);
    end;

    // S2 declared record Rec + undeclared xRec: xRec is still the implicit one -> d41
    procedure ShadowRecFwdXRec()
    var H: Codeunit "S H"; Rec: Record "S Cust";
    begin
        xRec.SetRange("No.", 'Z');
        H.ResetIt(xRec);
        xRec.DeleteAll();
    end;

    // S3 declared local xRec is a local record -> d41 as a local
    procedure ShadowDeclaredXRec()
    var H: Codeunit "S H"; xRec: Record "S Cust";
    begin
        xRec.SetRange("No.", 'Y');
        H.ResetIt(xRec);
        xRec.DeleteAll();
    end;

    // P6 OnModifyRecord: the page writes Rec after it (measured) -> no d39
    trigger OnModifyRecord(): Boolean
    var H: Codeunit "S H";
    begin
        H.Dirty(Rec);
    end;
}

page 50273 "S NoSource"
{
    PageType = Card;
    // N1: no SourceTable -> no implicit Rec
}

pageextension 50274 "S Card Ext" extends "S Card"
{
    actions
    {
        addlast(Processing)
        {
            action(E1)
            {
                trigger OnAction()
                var H: Codeunit "S H";
                begin
                    // E1 d41 on pageext Rec
                    Rec.SetRange("No.", 'B');
                    H.ResetIt(Rec);
                    Rec.FindFirst();
                end;
            }
        }
    }
}

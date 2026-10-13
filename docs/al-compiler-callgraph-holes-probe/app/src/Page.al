page 50100 "P Page"
{
    SourceTable = "P Tab";
    PageType = Card;
    layout { area(Content) { field(No; Rec."No.") { } } }
    actions
    {
        area(Processing)
        {
            action(Open)
            {
                RunObject = page "P Page";
            }
            action(Ins)
            {
                trigger OnAction()
                begin
                    Insert(true);
                    Rec.Validate(Amount, 1);
                end;
            }
        }
    }
    trigger OnOpenPage()
    var
        T: Codeunit "P Target";
    begin
        T.Inner();
    end;
}
report 50100 "P Report"
{
    ProcessingOnly = true;
    dataset { dataitem(D; "P Tab") { trigger OnAfterGetRecord() begin D.Helper(); end; } }
    trigger OnPreReport()
    var
        T: Codeunit "P Target";
    begin
        T.A();
    end;
}

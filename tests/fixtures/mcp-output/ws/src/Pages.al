page 50120 "Out Doc Card"
{
    PageType = Card;
    SourceTable = "Out Doc";

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
            action(Issue)
            {
                Caption = 'Issue';

                trigger OnAction()
                var
                    Issuer: Codeunit "Out Issue Document";
                begin
                    Issuer.IssueDoc(Rec);
                end;
            }
        }
    }
}

codeunit 50130 "Out Tests"
{
    Subtype = Test;

    [Test]
    procedure WriteLogInserts()
    var
        LogMgt: Codeunit "Out Log Mgt";
    begin
        LogMgt.WriteLog('test');
    end;
}

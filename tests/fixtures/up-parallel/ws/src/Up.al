// TLS-21: the up walk's fixture. Roots, in this order in the tests:
// - Alpha: two callers;
// - Beta: an event subscriber declared in both arms of an #if, subscribing to different
//   events, so two nodes;
// - a root that is not in the graph;
// - Gamma: called from a page action with a caption;
// - Delta: reached from a page that an action opens with RunObject;
// - Lonely: no callers.
table 51400 "UP Item"
{
    fields
    {
        field(1; "No."; Code[20]) { }
    }
}

codeunit 51400 "UP Core"
{
    procedure Alpha()
    begin
    end;

    // The arms subscribe to different events, so the graph keeps them as two nodes
    // (`split_decls`); the root's id without a suffix names both.
#if CLEAN
    [EventSubscriber(ObjectType::Codeunit, Codeunit::"UP Callers", 'OnFirst', '', false, false)]
    local procedure Beta()
    begin
    end;
#else
    [EventSubscriber(ObjectType::Codeunit, Codeunit::"UP Callers", 'OnSecond', '', false, false)]
    local procedure Beta()
    begin
        Alpha();
    end;
#endif

    procedure Gamma()
    begin
    end;

    procedure Delta()
    begin
    end;

    procedure Lonely()
    begin
    end;
}

codeunit 51401 "UP Callers"
{
    procedure CallerOne()
    var
        Core: Codeunit "UP Core";
    begin
        Core.Alpha();
    end;

    procedure CallerTwo()
    var
        Core: Codeunit "UP Core";
    begin
        Core.Alpha();
        OnFirst();
        OnSecond();
    end;

    [IntegrationEvent(false, false)]
    procedure OnFirst()
    begin
    end;

    [IntegrationEvent(false, false)]
    procedure OnSecond()
    begin
    end;
}

page 51400 "UP Card"
{
    PageType = Card;
    SourceTable = "UP Item";

    actions
    {
        area(Processing)
        {
            action(RunGamma)
            {
                Caption = 'Run Gamma';

                trigger OnAction()
                var
                    Core: Codeunit "UP Core";
                begin
                    Core.Gamma();
                end;
            }
            action(OpenList)
            {
                Caption = 'Open the list';
                RunObject = Page "UP List";
            }
        }
    }
}

page 51401 "UP List"
{
    PageType = List;
    SourceTable = "UP Item";

    trigger OnOpenPage()
    var
        Core: Codeunit "UP Core";
    begin
        Core.Delta();
    end;
}

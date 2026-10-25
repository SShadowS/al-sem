codeunit 50300 "W H"
{
    procedure Dirty(var C: Record "Dep Cust")
    begin
        C.Validate(Name, 'x');
    end;
}

page 50310 "W RO List"
{
    PageType = List;
    SourceTable = "Dep Cust";
    Editable = false;
    layout { area(Content) { repeater(G) { field(Name; Rec.Name) { } } } }
    actions
    {
        area(Processing)
        {
            action(W1)
            {
                trigger OnAction()
                var
                    H: Codeunit "W H";
                begin
                    H.Dirty(Rec);
                end;
            }
        }
    }
}

pageextension 50301 "W RO Ext" extends "Dep RO List"
{
    actions
    {
        addlast(Processing)
        {
            action(A1)
            {
                trigger OnAction()
                var
                    H: Codeunit "W H";
                begin
                    H.Dirty(Rec);
                end;
            }
        }
    }
}

pageextension 50302 "W NoMod Ext" extends "Dep NoMod Card"
{
    actions
    {
        addlast(Processing)
        {
            action(A2)
            {
                trigger OnAction()
                var
                    H: Codeunit "W H";
                begin
                    H.Dirty(Rec);
                end;
            }
        }
    }
}

pageextension 50303 "W Card Ext" extends "Dep Card"
{
    actions
    {
        addlast(Processing)
        {
            action(A3)
            {
                trigger OnAction()
                var
                    H: Codeunit "W H";
                begin
                    H.Dirty(Rec);
                end;
            }
        }
    }
}

pageextension 50305 "W RO Temp Ext" extends "Dep RO Temp List"
{
    actions
    {
        addlast(Processing)
        {
            action(A5)
            {
                trigger OnAction()
                var
                    H: Codeunit "W H";
                begin
                    H.Dirty(Rec);
                end;
            }
        }
    }
}

pageextension 50304 "W Editable Ext" extends "Dep Editable Card"
{
    actions
    {
        addlast(Processing)
        {
            action(A4)
            {
                trigger OnAction()
                var
                    H: Codeunit "W H";
                begin
                    H.Dirty(Rec);
                end;
            }
        }
    }
}

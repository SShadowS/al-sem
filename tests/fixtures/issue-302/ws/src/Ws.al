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

// A workspace read-only temporary page: its implicit `Rec` is temporary (silent),
// and so is the `Rec` of a pageext over it (`W RO Temp WsExt`).
page 50311 "W RO Temp Base"
{
    PageType = List;
    SourceTable = "Dep Cust";
    Editable = false;
    SourceTableTemporary = true;
    layout { area(Content) { repeater(G) { field(Name; Rec.Name) { } } } }
    actions
    {
        area(Processing)
        {
            action(W2)
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

// The same temporary page shape, but the action declares its own physical
// `Rec`: that record is not the page's temporary one, so d39 reports it, and d3
// (which skips a temporary record) reports `ReadLocal`'s `FindSet` + `Name` read.
page 50312 "W RO Temp Local"
{
    PageType = List;
    SourceTable = "Dep Cust";
    Editable = false;
    SourceTableTemporary = true;
    layout { area(Content) { repeater(G) { field(Name; Rec.Name) { } } } }
    actions
    {
        area(Processing)
        {
            action(W3)
            {
                trigger OnAction()
                var
                    H: Codeunit "W H";
                    Rec: Record "Dep Cust";
                begin
                    Rec.Get('1');
                    H.Dirty(Rec);
                end;
            }
        }
    }

    procedure ReadLocal()
    var
        Rec: Record "Dep Cust";
    begin
        Rec.FindSet();
        if Rec.Name <> '' then
            Rec.Name := Rec.Name;
    end;
}

pageextension 50306 "W RO Temp WsExt" extends "W RO Temp Base"
{
    actions
    {
        addlast(Processing)
        {
            action(A6)
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

// A pageext over the dependency's temporary page whose action declares its own
// physical `Rec`: reported (d39, d3), unlike the implicit one in `W RO Temp Ext`.
pageextension 50307 "W RO Temp LocalExt" extends "Dep RO Temp List"
{
    actions
    {
        addlast(Processing)
        {
            action(A7)
            {
                trigger OnAction()
                var
                    H: Codeunit "W H";
                    Rec: Record "Dep Cust";
                begin
                    Rec.Get('1');
                    H.Dirty(Rec);
                end;
            }
        }
    }

    procedure ReadLocal()
    var
        Rec: Record "Dep Cust";
    begin
        Rec.FindSet();
        if Rec.Name <> '' then
            Rec.Name := Rec.Name;
    end;
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

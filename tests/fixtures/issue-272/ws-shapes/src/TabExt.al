tableextension 50275 "S Cust Ext" extends "S Cust"
{
    // X1 d41 on tableext Rec
    procedure ExtD41ViaRec()
    var H: Codeunit "S H";
    begin
        Rec.SetRange("No.", 'C');
        H.ResetIt(Rec);
        Rec.FindFirst();
    end;

    trigger OnAfterModify()
    var H: Codeunit "S H";
    begin
        // X2 d39 OnAfterModify: written already, dirt is lost
        H.Dirty(Rec);
    end;

    trigger OnBeforeModify()
    var H: Codeunit "S H";
    begin
        // X3 d39 OnBeforeModify: platform writes afterwards
        H.Dirty(Rec);
    end;
}

tableextension 50279 "S Cust Ext2" extends "S Cust"
{
    // X4 table procedure that forwards Rec then reloads it: its own dirt is lost -> report
    procedure ExtReloadAfter()
    var H: Codeunit "S H";
    begin
        H.Dirty(Rec);
        Rec.Get(Rec."No.");
    end;

    // X7 OnAfterRename: written already (measured) -> report
    trigger OnAfterRename()
    var H: Codeunit "S H";
    begin
        H.Dirty(Rec);
    end;

    // X8 OnBeforeRename: the rename write follows (measured) -> no d39
    trigger OnBeforeRename()
    var H: Codeunit "S H";
    begin
        H.Dirty(Rec);
    end;

    // X9 OnAfterDelete: the row is gone, nothing to lose (moot) -> no d39
    trigger OnAfterDelete()
    var H: Codeunit "S H";
    begin
        H.Dirty(Rec);
    end;

    // X6 OnAfterInsert: written already (measured) -> report
    trigger OnAfterInsert()
    var H: Codeunit "S H";
    begin
        H.Dirty(Rec);
    end;

    // X5 xRec forwarded to a dirtying helper
    procedure ExtXRec()
    var H: Codeunit "S H";
    begin
        H.Dirty(xRec);
    end;
}

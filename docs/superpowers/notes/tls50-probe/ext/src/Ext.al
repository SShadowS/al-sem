tableextension 71060 "TLS50 Item Ext2" extends "TLS50 Item"
{
    trigger OnBeforeInsert()
    var
        L: Codeunit "TLS50 Log";
    begin
        L.Add('Y.OnBeforeInsert', Rec, xRec);
    end;

    trigger OnAfterInsert()
    var
        L: Codeunit "TLS50 Log";
    begin
        L.Add('Y.OnAfterInsert', Rec, xRec);
    end;

    trigger OnBeforeModify()
    var
        L: Codeunit "TLS50 Log";
    begin
        L.Add('Y.OnBeforeModify', Rec, xRec);
    end;

    trigger OnAfterModify()
    var
        L: Codeunit "TLS50 Log";
    begin
        L.Add('Y.OnAfterModify', Rec, xRec);
    end;

    trigger OnBeforeDelete()
    var
        L: Codeunit "TLS50 Log";
    begin
        L.Add('Y.OnBeforeDelete', Rec, xRec);
    end;

    trigger OnAfterDelete()
    var
        L: Codeunit "TLS50 Log";
    begin
        L.Add('Y.OnAfterDelete', Rec, xRec);
    end;
}

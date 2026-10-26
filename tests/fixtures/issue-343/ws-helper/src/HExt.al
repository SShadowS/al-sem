tableextension 50344 "H Job Ext" extends "H Job"
{
    trigger OnAfterModify()                  // A36: d39 (a bare call to a table procedure works on Rec)
    var
        M: Codeunit "H Mgt";
    begin
        M.DirtPublic(Rec);
        ReloadSelf();
        M.SaveRec(Rec);
    end;

    procedure DirtyMe()                      // validates the receiver
    begin
        Validate(Name, 'r');
    end;

    procedure ReloadSelf()                   // reloads the receiver
    begin
        if Get("No.") then;
    end;

    // The receiver and `Alias` are the same record when called as `Job.SaveAlias(Job, ..)`.
    procedure SaveAlias(var Alias: Record "H Job"; Template: Record "H Job")
    begin
        Alias := Template;
        Insert();
    end;
}

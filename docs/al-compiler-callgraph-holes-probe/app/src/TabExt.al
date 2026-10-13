tableextension 50100 "P Tab Ext" extends "P Tab"
{
    fields
    {
        modify(Amount)
        {
            trigger OnAfterValidate()
            begin
                Rec.Helper();
            end;
        }
    }
    trigger OnAfterInsert()
    begin
        Rec.Helper();
    end;
}

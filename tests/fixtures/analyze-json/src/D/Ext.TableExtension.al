tableextension 50106 "Probe Base Ext" extends "Probe Base"
{
    fields
    {
        field(50100; Note; Text[50])
        {
            trigger OnValidate()
            begin
            end;
        }
    }

    procedure ExtProc()
    begin
    end;
}

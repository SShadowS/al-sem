tableextension 50344 "H Job Ext" extends "H Job"
{
    procedure DirtyMe()                      // validates the receiver
    begin
        Validate(Name, 'r');
    end;
}

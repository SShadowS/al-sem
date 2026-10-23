codeunit 50260 "TQ Twin Nested"
{
    var
        RootHelper: Codeunit "TQ Root Assert Helper";

    procedure Twin()
    begin
        RootHelper.Check();
    end;
}

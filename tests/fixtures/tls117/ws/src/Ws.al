codeunit 50100 "Ws Cu"
{
    procedure Run()
    var
        D: Codeunit "Dep Cu 0";
    begin
        D.P0();
    end;
}

codeunit 50300 "W Reader"
{
    procedure Count(): Integer
    var
        C: Record "Dep Cust";
    begin
        exit(C.Count());
    end;
}

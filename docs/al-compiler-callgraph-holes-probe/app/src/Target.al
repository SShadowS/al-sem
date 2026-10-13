codeunit 50101 "P Target"
{
    trigger OnRun()
    begin
        Inner();
    end;

    procedure Inner() begin end;
    procedure A() begin end;
    procedure B() begin end;
    procedure NoParens(): Boolean begin exit(true); end;
    procedure Overl(i: Integer) begin end;
    procedure Overl(t: Text) begin end;
}

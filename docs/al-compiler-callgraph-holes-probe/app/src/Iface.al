interface "P Iface"
{
    procedure Go();
}
codeunit 50110 "P Impl A" implements "P Iface"
{
    procedure Go() begin end;
}
codeunit 50111 "P Impl B" implements "P Iface"
{
    procedure Go() begin end;
}

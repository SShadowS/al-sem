// CU 50201: Uses a codeunit-typed global variable to call MCR Helper.LocalHelper().
// LocalHelper is local, so this call is not visible from here: the program
// engine leaves it unresolved (LocalNotVisible) and adds no edge.
codeunit 50201 "MCR Caller"
{
	var
		Helper: Codeunit "MCR Helper";

	trigger OnRun()
	begin
		Helper.LocalHelper(); // member call: LocalNotVisible
	end;
}

// CU 50200: Has a local procedure that Caller (CU 50201) calls through a
// codeunit variable. A local procedure is not callable from another object, so
// the program engine does NOT resolve that call (LocalNotVisible). Only the
// old L3 resolver ignored access and made LocalHelper look reachable. D14 flags it.
codeunit 50200 "MCR Helper"
{
	// local procedure: only callable from within this object. The member call
	// from Caller.OnRun does not reach it, so there is no call graph edge to
	// LocalHelper and it is unreachable.
	local procedure LocalHelper()
	begin
	end;

	// Public procedure so the codeunit has at least one root (keeps it from being
	// entirely dead itself), but it does NOT call LocalHelper.
	procedure PublicEntry()
	begin
	end;
}

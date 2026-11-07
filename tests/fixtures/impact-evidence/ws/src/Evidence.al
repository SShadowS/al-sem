// Round 2 G7: impact names the tables a changed routine writes (when few),
// the fields it assigns, and the company context of its writes.
// - Configure: assigns two fields of "IE Setup" and modifies it, after a
//   ChangeCompany in a routine it calls;
// - SwitchCompany: the ChangeCompany call.
table 51300 "IE Setup"
{
    fields
    {
        field(1; "Primary Key"; Code[10])
        {
        }
        field(2; "Blob Folder"; Text[250])
        {
        }
        field(3; "Container Name"; Text[250])
        {
        }
    }
}

codeunit 51301 "IE Mgt"
{
    procedure Configure()
    var
        Setup: Record "IE Setup";
    begin
        SwitchCompany(Setup);
        Setup.Get();
        Setup."Blob Folder" := '';
        Setup."Container Name" := '';
        Setup.Modify();
    end;

    local procedure SwitchCompany(var Setup: Record "IE Setup")
    begin
        Setup.ChangeCompany('Other');
    end;
}

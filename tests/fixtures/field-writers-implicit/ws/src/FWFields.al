// The field writers fixture (`alsem explain --table T --field F`, d65's writer
// index): bare `F := …` writes through the implicit record. The field asked is
// "FW Item".Folder. Writers, by hand (each line carries `// writer`):
// the field's own OnValidate, the table procedure Normalize, the table
// extension's ExtSet, the page action SetFolder (its source table's Rec) and
// the qualified Item.Folder in Qualified = 5 routines, 5 sites.
// Not writers: a bare READ (ReadOnly), a local named like the field
// (Shadowed), another field's bare write (OtherField).
table 50810 "FW Item"
{
    fields
    {
        field(1; "No."; Code[20]) { }
        field(2; Folder; Text[250])
        {
            trigger OnValidate()
            begin
                if Folder <> '' then
                    Folder := Folder + '\'; // writer
            end;
        }
        field(3; Note; Text[50]) { }
    }
    keys { key(PK; "No.") { } }

    procedure Normalize()
    begin
        "Folder" := 'N'; // writer
    end;

    procedure ReadOnly(): Boolean
    begin
        exit(Folder = '');
    end;

    procedure OtherField()
    begin
        Note := '';
    end;
}

tableextension 50810 "FW Item Ext" extends "FW Item"
{
    procedure ExtSet()
    begin
        Folder := 'A'; // writer
    end;
}

page 50810 "FW Card"
{
    SourceTable = "FW Item";

    actions
    {
        area(Processing)
        {
            action(SetFolder)
            {
                trigger OnAction()
                begin
                    Folder := 'B'; // writer
                end;
            }
        }
    }
}

codeunit 50810 "FW Code"
{
    procedure Shadowed()
    var
        Folder: Text;
    begin
        Folder := 'C';
        Message(Folder);
    end;

    procedure Qualified(var Item: Record "FW Item")
    begin
        Item.Folder := 'D'; // writer
    end;
}

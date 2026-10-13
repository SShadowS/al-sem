codeunit 50928 "Acc Legacy Export"
{
    // Deliberately reached by no test: the "uncoverable" changed procedure.
    procedure ExportLine(ItemNo: Code[20]; Amount: Decimal): Text
    begin
        exit(ItemNo + ';' + Format(Amount));
    end;
}

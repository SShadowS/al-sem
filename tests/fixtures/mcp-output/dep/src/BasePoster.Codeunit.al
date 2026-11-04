codeunit 70100 "Base Poster"
{
    trigger OnRun()
    var
        DocLine: Record "Base Doc Line";
    begin
        DocLine.DeleteAll();
        PostDocument();
    end;

    local procedure PostDocument()
    var
        L01: Record "Base Ledger 01";
        L02: Record "Base Ledger 02";
        L03: Record "Base Ledger 03";
        L04: Record "Base Ledger 04";
        L05: Record "Base Ledger 05";
        L06: Record "Base Ledger 06";
        L07: Record "Base Ledger 07";
        L08: Record "Base Ledger 08";
        L09: Record "Base Ledger 09";
        L10: Record "Base Ledger 10";
        L11: Record "Base Ledger 11";
        L12: Record "Base Ledger 12";
        Register: Record "Base Register";
        TempL01: Record "Base Ledger 01" temporary;
    begin
        TempL01.Insert();
        L01.Insert();
        L02.Insert();
        L03.Insert();
        L04.Insert();
        L05.Insert();
        L06.Insert();
        L07.Insert();
        L08.Insert();
        L09.Insert();
        L10.Insert();
        L11.Insert();
        L12.Insert();
        Register.Get(1);
        Register."Last Entry No." += 1;
        Register.Modify();
        OnAfterPostDocument(Register);
        Codeunit.Run(Register."Last Entry No.");
        Commit();
    end;

    [IntegrationEvent(false, false)]
    local procedure OnAfterPostDocument(var Register: Record "Base Register")
    begin
    end;
}

codeunit 70101 "Base Calc"
{
    trigger OnRun()
    begin
        UpdatePrice('ITEM');
    end;

    local procedure UpdatePrice(ItemNo: Code[20])
    var
        Price: Record "Base Price";
    begin
        if Price.Get(ItemNo) then begin
            Price."Unit Price" := 1;
            Price.Modify();
        end;
    end;
}

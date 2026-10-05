// Issues #13 / #20: which TableData permissions a temporary record needs (none).
codeunit 50301 "PT Writer"
{
    // REQUIRES I on "PT Rec": the physical Insert. The temp Insert comes FIRST on
    // purpose -- a guard applied after the per-row dedup would let it claim the
    // row and then drop it, losing the physical requirement.
    procedure Mixed()
    var
        TempRec: Record "PT Rec" temporary;
        Rec: Record "PT Rec";
    begin
        TempRec."No." := 1;
        TempRec.Insert();
        Rec."No." := 2;
        Rec.Insert();
    end;

    // REQUIRES I: the record's temp state depends on the caller, so it is not
    // known-temp and stays required (conservative).
    procedure ByParam(var R: Record "PT Rec")
    begin
        R.Insert();
    end;

    // NO permission: only a temporary record is touched.
    procedure TempOnly()
    var
        TempRec: Record "PT Rec" temporary;
    begin
        if TempRec.Get(1) then
            TempRec.Modify();
    end;
}

// TLS-43: identifiers with non-ASCII capitals. The L2 builder once folded them with
// `to_ascii_lowercase` (`Ændring` -> `Ændring`) while the detectors folded with
// `to_lowercase` (`ændring`), so the two never met. Each case has a control that
// must fire, so the fixture shows the detector ran.
codeunit 50430 "Nordic Names"
{
    var
        Ålcust: Record "Nordic Order";

    // d19: `Ændring` is used, so it is not an unused parameter.
    procedure UsesÆndring(Ændring: Integer): Integer
    begin
        exit(Ændring + 1);
    end;

    // d19 control: `Ubrugt` is never used.
    procedure LeavesUbrugt(Ubrugt: Integer)
    begin
    end;

    // d11: `Ørder` is loaded by `HentØrder`, which takes it by var, before Modify
    // (the G-10 tier-2 suppression).
    procedure ModifyAfterHent()
    var
        Ørder: Record "Nordic Order";
    begin
        HentØrder(Ørder);
        Ørder.Beløb := 1;
        Ørder.Modify();
    end;

    // d11 control: `Ørder` is never loaded.
    procedure ModifyUnloaded()
    var
        Ørder: Record "Nordic Order";
    begin
        Ørder.Beløb := 2;
        Ørder.Modify();
    end;

    local procedure HentØrder(var Østed: Record "Nordic Order")
    begin
        Østed.Get('X');
    end;

    // d39: `GemAndet` reassigns its parameter before Modify, so the caller's
    // dirty record is never persisted (the helper does not persist it).
    procedure ValidatesThenGemAndet()
    var
        Ørder: Record "Nordic Order";
    begin
        Ørder.Get('X');
        SætBeløb(Ørder);
        GemAndet(Ørder);
    end;

    // d39 control: `GemSamme` persists the caller's record.
    procedure ValidatesThenGemSamme()
    var
        Ørder: Record "Nordic Order";
    begin
        Ørder.Get('X');
        SætBeløb(Ørder);
        GemSamme(Ørder);
    end;

    local procedure SætBeløb(var Ørd: Record "Nordic Order")
    begin
        Ørd.Validate(Beløb, 1);
        exit;
    end;

    local procedure GemAndet(var Ørd: Record "Nordic Order")
    var
        Andet: Record "Nordic Order";
    begin
        Ørd := Andet;
        Ørd.Modify();
    end;

    local procedure GemSamme(var Ørd: Record "Nordic Order")
    begin
        Ørd.Modify();
    end;

    // The global record-variable backfill: `Ålcust` is an object global, so
    // the argument binding of `HentØrder(Ålcust)` is `global`.
    procedure PassesGlobal()
    begin
        HentØrder(Ålcust);
    end;
}

table 50430 "Nordic Order"
{
    fields
    {
        field(1; "No."; Code[20]) { }
        field(2; Beløb; Decimal) { }
    }
}

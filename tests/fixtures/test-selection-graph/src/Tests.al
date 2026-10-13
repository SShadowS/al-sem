codeunit 50947 "Acc Loyalty Tests"
{
    Subtype = Test;

    [Test]
    procedure BonusAboveThousand()
    var
        Core: Codeunit "Acc Loyalty Core";
    begin
        if Core.PointsFor(1000) <> 150 then
            Error('PointsFor(1000) should be 150 with the bonus');
    end;

    [Test]
    procedure NightlyRecalcDecays()
    var
        Rules: Codeunit "Acc Loyalty Rules";
    begin
        if Rules.RecalcNightly() <> 270 then
            Error('Nightly recalc of 2500 should give 270 points');
    end;
}

codeunit 50944 "Acc Tax Tests"
{
    Subtype = Test;

    [Test]
    procedure GermanTax()
    var
        TaxCore: Codeunit "Acc Tax Core";
    begin
        if TaxCore.TaxAmount(100, Enum::"Acc Region"::DE) <> 19 then
            Error('DE tax on 100 should be 19');
    end;
}

codeunit 50946 "Acc Invoice Tests"
{
    Subtype = Test;

    [Test]
    procedure InvoiceTotalRounds()
    var
        InvoiceCore: Codeunit "Acc Invoice Core";
    begin
        if InvoiceCore.InvoiceTotal(80, Enum::"Acc Region"::DK) <> 100 then
            Error('DK invoice of 80 should be 100');
    end;
}

codeunit 50960 "Sel Page Tests"
{
    Subtype = Test;

    [Test]
    procedure CardShowsRecord()
    var
        Card: TestPage "Sel Ledger Card";
    begin
        Card.OpenView();
        Card.Close();
    end;

    [Test]
    [HandlerFunctions('CardHandler')]
    procedure OpenCardModally()
    var
        Hooks: Codeunit "Sel Ledger Hooks";
    begin
        Hooks.OpenCardModal();
    end;

    [ModalPageHandler]
    procedure CardHandler(var Card: TestPage "Sel Ledger Card")
    var
        Hooks: Codeunit "Sel Ledger Hooks";
    begin
        Hooks.InModalHandler();
    end;

    [Test]
    procedure InsertLedger()
    var
        Ledger: Record "Sel Ledger";
    begin
        Ledger."No." := 'L1';
        Ledger.Insert(true);
    end;
}

codeunit 50961 "Sel Dynamic Tests"
{
    Subtype = Test;

    [Test]
    procedure DynamicRun()
    var
        Dynamic: Codeunit "Sel Dynamic";
    begin
        Dynamic.RunById(50926);
    end;

    [Test]
    procedure VariantPing()
    var
        Dynamic: Codeunit "Sel Dynamic";
        Pingable: Codeunit "Sel Pingable";
    begin
        Dynamic.CallVariant(Pingable);
    end;

    [Test]
    procedure RecordRefInsert()
    var
        Dynamic: Codeunit "Sel Dynamic";
    begin
        Dynamic.InsertViaRef();
    end;

    [Test]
    procedure ReportSaved()
    var
        Dynamic: Codeunit "Sel Dynamic";
    begin
        Dynamic.SaveReport();
    end;
}

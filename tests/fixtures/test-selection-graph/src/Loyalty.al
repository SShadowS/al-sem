codeunit 50923 "Acc Loyalty Core"
{
    procedure PointsFor(Amount: Decimal): Decimal
    var
        Points: Decimal;
    begin
        Points := Round(Amount / 10, 1, '<');
        OnAfterCalcPoints(Amount, Points);
        exit(Points);
    end;

    [IntegrationEvent(false, false)]
    local procedure OnAfterCalcPoints(Amount: Decimal; var Points: Decimal)
    begin
    end;
}

codeunit 50924 "Acc Loyalty Bonus Subscriber"
{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::"Acc Loyalty Core", 'OnAfterCalcPoints', '', false, false)]
    local procedure AddBonusOnCalcPoints(Amount: Decimal; var Points: Decimal)
    begin
        if Amount >= 1000 then
            Points += 50;
    end;
}

codeunit 50925 "Acc Loyalty Rules"
{
    var
        RunState: Codeunit "Acc Run State";

    procedure RecalcNightly(): Decimal
    begin
        Codeunit.Run(Codeunit::"Acc Nightly Recalc");
        exit(RunState.GetLastRecalcPoints());
    end;
}

codeunit 50926 "Acc Nightly Recalc"
{
    trigger OnRun()
    var
        LoyaltyCore: Codeunit "Acc Loyalty Core";
        RunState: Codeunit "Acc Run State";
    begin
        RunState.SetLastRecalcPoints(ApplyDecay(LoyaltyCore.PointsFor(2500)));
    end;

    local procedure ApplyDecay(Points: Decimal): Decimal
    begin
        exit(Points * 90 / 100);
    end;
}

codeunit 50927 "Acc Run State"
{
    SingleInstance = true;

    var
        LastRecalcPoints: Decimal;

    procedure SetLastRecalcPoints(Points: Decimal)
    begin
        LastRecalcPoints := Points;
    end;

    procedure GetLastRecalcPoints(): Decimal
    begin
        exit(LastRecalcPoints);
    end;
}

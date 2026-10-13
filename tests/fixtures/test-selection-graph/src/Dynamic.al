codeunit 50970 "Sel Dynamic"
{
    procedure RunById(Id: Integer)
    begin
        Codeunit.Run(Id);
    end;

    procedure CallVariant(V: Variant)
    begin
        V.Ping();
    end;

    procedure InsertViaRef()
    var
        RecRef: RecordRef;
    begin
        RecRef.Open(Database::"Sel Ledger");
        RecRef.Insert(true);
    end;

    procedure SaveReport()
    begin
        Report.SaveAsXml(Report::"Sel Report", 'report.xml');
    end;
}

codeunit 50971 "Sel Pingable"
{
    procedure Ping()
    begin
    end;
}

codeunit 50972 "Sel Unbound Subscriber"
{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::"Acc Invoice Core", 'OnNoSuchEvent', '', false, false)]
    local procedure OnNoSuchEvent()
    begin
        ReachedOnlyFromNowhere();
    end;

    local procedure ReachedOnlyFromNowhere()
    begin
    end;
}

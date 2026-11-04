codeunit 50110 "Out Issue Document"
{
    procedure IssueDoc(var Doc: Record "Out Doc")
    var
        BasePoster: Codeunit "Base Poster";
        BaseCalc: Codeunit "Base Calc";
        LogMgt: Codeunit "Out Log Mgt";
        Sender: Codeunit "Out Sender";
        Handler: Interface "Out Handler";
        DynamicId: Integer;
    begin
        Doc.TestField(Status, Doc.Status::Open);
        Doc.Status := Doc.Status::Issued;
        Doc.Modify();
        BasePoster.Run();
        BaseCalc.Run();
        LogMgt.WriteLog('issued');
        Sender.Notify(Doc."No.");
        TaskScheduler.CreateTask(Codeunit::"Out Job", 0, true);
        DynamicId := Doc.Amount;
        Codeunit.Run(DynamicId);
        Handler := GetHandler();
        Handler.Handle(Doc);
        ScanFields(Database::"Out Doc" + DynamicId);
        OnAfterIssueDoc(Doc);
    end;

    local procedure GetHandler(): Interface "Out Handler"
    var
        Default: Codeunit "Out Default Handler";
    begin
        exit(Default);
    end;

    local procedure ScanFields(TableNo: Integer)
    var
        RecRef: RecordRef;
    begin
        RecRef.Open(TableNo);
        if RecRef.FindSet() then
            repeat
            until RecRef.Next() = 0;
    end;

    [IntegrationEvent(false, false)]
    local procedure OnAfterIssueDoc(var Doc: Record "Out Doc")
    begin
    end;
}

codeunit 50111 "Out Log Mgt"
{
    procedure WriteLog(Msg: Text[100])
    var
        Log: Record "Out Log";
        TempLog: Record "Out Log" temporary;
    begin
        TempLog.DeleteAll();
        Log."Entry No." := 0;
        Log.Message := Msg;
        Log.Insert();
    end;
}

codeunit 50112 "Out Sender"
{
    procedure Notify(DocNo: Code[20])
    var
        Client: HttpClient;
        Response: HttpResponseMessage;
    begin
        Client.Get('https://example.test/issued/' + DocNo, Response);
    end;
}

codeunit 50113 "Out Job"
{
    trigger OnRun()
    begin
    end;
}

interface "Out Handler"
{
    procedure Handle(var Doc: Record "Out Doc");
}

codeunit 50114 "Out Default Handler" implements "Out Handler"
{
    procedure Handle(var Doc: Record "Out Doc")
    begin
    end;
}

codeunit 50115 "Out Subscribers"
{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::"Out Issue Document", 'OnAfterIssueDoc', '', false, false)]
    local procedure LogIssued(var Doc: Record "Out Doc")
    var
        LogMgt: Codeunit "Out Log Mgt";
    begin
        LogMgt.WriteLog('subscriber');
    end;
}

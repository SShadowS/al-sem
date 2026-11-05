table 52902 "Workflow Step Instance"
{
    fields
    {
        field(1; ID; Guid)
        {
        }
        field(2; "Function Name"; Code[128])
        {
        }
    }
}

table 52903 "Workflow Response"
{
    fields
    {
        field(1; "Function Name"; Code[128])
        {
        }
    }
}

codeunit 52904 "Workflow Response Handling"
{
    procedure ExecuteResponse(var Variant: Variant; ResponseWorkflowStepInstance: Record "Workflow Step Instance"; xVariant: Variant)
    var
        WorkflowResponse: Record "Workflow Response";
    begin
        OnBeforeExecuteResponse(Variant);
        if not WorkflowResponse.Get(ResponseWorkflowStepInstance."Function Name") then
            exit;
        case WorkflowResponse."Function Name" of
            DoNothingCode():
                DoNothing();
            PostCode():
                PostIt(Variant);
            else
                OnExecuteWorkflowResponse(Variant);
        end;
    end;

    procedure DoNothingCode(): Code[128]
    begin
        exit('DONOTHING');
    end;

    procedure PostCode(): Code[128]
    begin
        exit('POST');
    end;

    local procedure DoNothing()
    begin
    end;

    local procedure PostIt(var Variant: Variant)
    begin
        OnAfterPostIt();
    end;

    [IntegrationEvent(false, false)]
    procedure OnAfterPostIt()
    begin
    end;

    [IntegrationEvent(false, false)]
    procedure OnBeforeExecuteResponse(var Variant: Variant)
    begin
    end;

    [IntegrationEvent(false, false)]
    procedure OnExecuteWorkflowResponse(var Variant: Variant)
    begin
    end;
}

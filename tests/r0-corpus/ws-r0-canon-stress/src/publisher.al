codeunit 70000 "Canon Stress Publisher"
{
    trigger OnRun()
    begin
    end;

    // --- Overloads: same name, different param types (incl. return-type-only). ---
    procedure Compute(a: Integer): Integer
    begin
        exit(a);
    end;

    procedure Compute(a: Integer; b: Code[20]): Integer
    begin
        exit(a);
    end;

    procedure Compute(a: Decimal): Decimal
    begin
        exit(a);
    end;

    // Return-type-only difference vs the no-arg form below.
    procedure Resolve(): Integer
    begin
        exit(0);
    end;

    procedure Resolve(): Text
    begin
        exit('');
    end;

    // --- Ugly parameter zoo. ---
    procedure DoWork(var Cust: Record Customer; "Sales Header": Record "Sales Header"; Items: array[10] of Integer; var TempBuffer: Record "Temp Buffer" temporary; Kind: Enum "Doc Kind"; Opt: Option Open,Closed; Postcode: Code[20]; Note: Text[100])
    begin
    end;

    // Mixed-case + quoted routine name with spaces.
    procedure "Mixed Case Routine"(Input: Text): Boolean
    begin
        exit(true);
    end;

    // --- Events. ---
    [IntegrationEvent(false, false)]
    procedure OnBeforePost(var Handled: Boolean)
    begin
    end;

    [BusinessEvent(false)]
    procedure OnAfterPost(DocNo: Code[20])
    begin
    end;

    // InternalEvent is an event-publisher too (G10), of event kind "internal".
    [InternalEvent(false)]
    procedure OnInternalSignal()
    begin
    end;

    // Obsolete + event combo.
    [Obsolete('use OnBeforePost', '24.0')]
    [IntegrationEvent(false, false)]
    procedure OnLegacyPost()
    begin
    end;
}

codeunit 70001 "Canon Stress Subscriber"
{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::"Canon Stress Publisher", 'OnBeforePost', '', false, false)]
    local procedure HandleBeforePost(var Handled: Boolean)
    begin
    end;
}

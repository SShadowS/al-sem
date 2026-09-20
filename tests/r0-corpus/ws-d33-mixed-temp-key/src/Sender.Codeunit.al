codeunit 50000 "D33 Sender"
{
    /// <summary>
    /// ⟨issue 33 / issue 32⟩ The mixed-temp-key fixture.
    ///
    /// `Inner` holds a KNOWN-TEMP insert of "D33 Rec" and, one hop further down,
    /// a PHYSICAL insert of the SAME table. Both carry the identical
    /// (op, resourceKind, resourceId, confidence) tuple, so before issue 33 they
    /// shared one `inheritedFactKey` and the nearer temp one won outright —
    /// `Run`'s inherited set held exactly ONE fact and the physical write of
    /// "D33 Rec" was invisible to every consumer of `writes_physical_tables_of`.
    ///
    /// With the temp class in the key `Run` inherits BOTH, which is the
    /// population `oracle_r3a3_inherited_factkey_dedup` needs in order to be
    /// able to fail at all.
    ///
    /// It is also issue 32's exact shape: the temp write precedes an external
    /// HTTP call and the physical write follows it. Unmasking the physical fact
    /// without the digest's terminal temp-class guard makes d47 report
    /// WRITE_PENDING_AT_EXTERNAL_IO here — graded physical from the fact,
    /// anchored at the in-memory `TempRec.Insert()` from the witness terminal.
    /// It must stay silent.
    /// </summary>
    procedure Run()
    begin
        Inner();
    end;

    procedure Inner()
    var
        TempRec: Record "D33 Rec" temporary;
        Client: HttpClient;
        Resp: HttpResponseMessage;
    begin
        TempRec.Init();
        TempRec."No." := 1;
        TempRec.Insert();
        Client.Get('https://example.test/ping', Resp);
        PhysWriter();
    end;

    procedure PhysWriter()
    var
        Rec: Record "D33 Rec";
    begin
        Rec.Init();
        Rec."No." := 2;
        Rec.Insert();
    end;

    /// <summary>
    /// CONTROL — a physical write that genuinely PRECEDES the external IO, on a
    /// table nothing else in this fixture touches. This one MUST fire
    /// WRITE_PENDING_AT_EXTERNAL_IO, on the pre-issue-33 engine and after, or
    /// `Run`'s silence would prove nothing about the guard.
    /// </summary>
    procedure ControlRun()
    begin
        ControlInner();
    end;

    procedure ControlInner()
    var
        Ctrl: Record "D33 Ctrl";
        Client: HttpClient;
        Resp: HttpResponseMessage;
    begin
        Ctrl.Init();
        Ctrl."No." := 1;
        Ctrl.Insert();
        Client.Get('https://example.test/ping', Resp);
    end;
}

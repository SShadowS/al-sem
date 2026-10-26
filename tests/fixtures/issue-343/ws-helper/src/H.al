table 50343 "H Job"
{
    fields
    {
        field(1; "No."; Code[20]) { }
        field(2; Name; Text[100]) { }
    }
    keys { key(PK; "No.") { } }

    procedure SaveMe()                       // persists the receiver on every path
    begin
        if Name = '' then begin
            Insert(true);
            exit;
        end;
        if Insert(true) then;
    end;
}

codeunit 50343 "H Mgt"
{
    // ---- dirtying callees -------------------------------------------------

    // The CDO shape: clean on every exit(false), dirty only through exit(true).
    local procedure InitJob(var Job: Record "H Job"): Boolean
    begin
        if Job.IsEmpty() then
            exit(false);
        Job.Init();
        Job.Validate(Name, 'x');
        exit(true);
    end;

    // Dirty on an exit(false) too: no return value is clean.
    local procedure InitJobDirtyOnFalse(var Job: Record "H Job"): Boolean
    begin
        Job.Validate(Name, 'x');
        if Job.IsEmpty() then
            exit(false);
        exit(true);
    end;

    local procedure Dirt(var Job: Record "H Job")
    begin
        Job.Validate(Name, 'x');
    end;

    // ---- persisting helpers -----------------------------------------------

    // Persists on every path: the Insert sits in the second `if` condition.
    local procedure InsertJob(var Job: Record "H Job"; OnHold: Boolean)
    begin
        if OnHold then begin
            Job.Insert(true);
            exit;
        end;
        if Job.Insert(true) then;
    end;

    // Persists on one path only.
    local procedure MaybeInsertJob(var Job: Record "H Job"; OnHold: Boolean)
    begin
        if OnHold then
            Job.Insert(true);
    end;

    // Persists on every path through a helper of its own.
    local procedure InsertJobViaHelper(var Job: Record "H Job")
    begin
        InsertJob(Job, false);
    end;

    // ---- callers ------------------------------------------------------------

    procedure CdoShape()                     // A1: no d39 (guard exit is the clean path)
    var
        Job: Record "H Job";
    begin
        if not InitJob(Job) then
            exit;
        Job."No." := 'A';
        InsertJob(Job, false);
    end;

    procedure ExitBeforeHelper(Flag: Boolean) // A2: d39 (exit on the call's own path)
    var
        Job: Record "H Job";
    begin
        if Flag then begin
            Dirt(Job);
            exit;
        end;
        InsertJob(Job, false);
    end;

    procedure ExitWithCall(Flag: Boolean): Boolean // A3: d39 (the call is the exit value)
    var
        Job: Record "H Job";
    begin
        if Flag then
            exit(InitJob(Job));
        InsertJob(Job, false);
    end;

    procedure PlainHelper()                  // A4: no d39
    var
        Job: Record "H Job";
    begin
        Dirt(Job);
        InsertJob(Job, true);
    end;

    procedure HelperOnOnePath()              // A5: d39 (helper persists on one path only)
    var
        Job: Record "H Job";
    begin
        Dirt(Job);
        MaybeInsertJob(Job, true);
    end;

    procedure ReceiverCall()                 // A6: no d39 (table procedure persists Rec)
    var
        Job: Record "H Job";
    begin
        Dirt(Job);
        Job.SaveMe();
    end;

    procedure GuardOnDirtyValue()            // A7: d39 (exit when the callee returned true)
    var
        Job: Record "H Job";
    begin
        if InitJob(Job) then
            exit;
        InsertJob(Job, false);
    end;

    procedure GuardDirtyOnFalse()            // A8: d39 (callee dirty on exit(false) too)
    var
        Job: Record "H Job";
    begin
        if not InitJobDirtyOnFalse(Job) then
            exit;
        InsertJob(Job, false);
    end;

    procedure NestedHelper()                 // A9: no d39 (helper persists through its own helper)
    var
        Job: Record "H Job";
    begin
        Dirt(Job);
        InsertJobViaHelper(Job);
    end;

    procedure PersistInOtherArm(Flag: Boolean) // A10: d39 (persist only in the other arm)
    var
        Job: Record "H Job";
    begin
        if Flag then
            Dirt(Job)
        else
            InsertJob(Job, false);
    end;

    // ---- revision 2: fail-closed shapes (spec round 1) ----------------------

    // Persists on every path, but Init drops the caller's write first (OP1).
    local procedure InitAndInsert(var Job: Record "H Job")
    begin
        Job.Init();
        Job."No." := 'X';
        Job.Insert();
    end;

    procedure HelperReinits()                // A11: d39 (helper Init before its Insert)
    var
        Job: Record "H Job";
    begin
        Dirt(Job);
        InitAndInsert(Job);
    end;

    procedure CallerReloads()                // A12: d39 (caller reloads before the helper)
    var
        Job: Record "H Job";
    begin
        Dirt(Job);
        if Job.Get('A') then;
        InsertJob(Job, false);
    end;

    // Already 'x': returns false, untouched. Else validates Name to 'x', returns true.
    local procedure MaybeDirty(var Job: Record "H Job"): Boolean
    begin
        if Job.Name = 'x' then
            exit(false);
        Job.Validate(Name, 'x');
        exit(true);
    end;

    procedure LoopGuard()                    // A13: d39 (pass 1 dirties, pass 2 exits on false)
    var
        Job: Record "H Job";
    begin
        // Pass 1: '' -> validated to 'x', true; "No." stays '', so pass 2: false -> exit.
        Job.Name := '';
        repeat
            if not MaybeDirty(Job) then
                exit;
        until Job."No." <> '';
        InsertJob(Job, false);
    end;

    local procedure SaveAndCount(var Job: Record "H Job"): Integer
    begin
        Job.Insert();
        exit(1);
    end;

    procedure ForHeader()                    // A14: d39 (the `for` bound runs once, before the body)
    var
        Job: Record "H Job";
        I: Integer;
    begin
        for I := 1 to SaveAndCount(Job) do
            Dirt(Job);
    end;

    procedure AssertErrorShape()             // A15: d39 (a caught error: the path goes on)
    var
        Job: Record "H Job";
    begin
        Dirt(Job);
        asserterror Error('expected');
    end;

    procedure TernaryShape(Flag: Boolean)    // A16: d39 (only one ternary alternative runs)
    var
        Job: Record "H Job";
        I: Integer;
    begin
        Dirt(Job);
        I := Flag ? SaveAndCount(Job) : 0;
    end;

    [IntegrationEvent(false, false)]
    local procedure OnBeforeInsertJob(var Job: Record "H Job"; var IsHandled: Boolean)
    begin
    end;

    local procedure InsertJobHandled(var Job: Record "H Job")
    var
        IsHandled: Boolean;
    begin
        OnBeforeInsertJob(Job, IsHandled);
        if IsHandled then
            exit;
        Job.Insert();
    end;

    procedure IsHandledHelper()              // A17: d39 (helper may exit before its Insert)
    var
        Job: Record "H Job";
    begin
        Dirt(Job);
        InsertJobHandled(Job);
    end;

    // Dirty on exit(false) through a call on the record (OP2a / S4).
    local procedure InitViaReceiver(var Job: Record "H Job"; Stop: Boolean): Boolean
    begin
        Job.DirtyMe();
        if Stop then
            exit(false);
        Job.Validate(Name, 'y');
        exit(true);
    end;

    procedure ReceiverInClean(Stop: Boolean) // A18: d39 (Stop = true: dirty via DirtyMe, then exit(false))
    var
        Job: Record "H Job";
    begin
        if not InitViaReceiver(Job, Stop) then
            exit;
        InsertJob(Job, false);
    end;

    [IntegrationEvent(false, false)]
    local procedure OnBeforeInitJob(var Job: Record "H Job"; var IsHandled: Boolean)
    begin
    end;

    // Dirty on exit(false) through a subscriber of a `var` publisher (OP2b).
    local procedure InitWithEvent(var Job: Record "H Job"): Boolean
    var
        IsHandled: Boolean;
    begin
        OnBeforeInitJob(Job, IsHandled);
        if IsHandled then
            exit(false);
        Job.Validate(Name, 'z');
        exit(true);
    end;

    procedure PublisherInClean()             // A19: d39
    var
        Job: Record "H Job";
    begin
        if not InitWithEvent(Job) then
            exit;
        InsertJob(Job, false);
    end;

    local procedure Lvl0(var Job: Record "H Job")
    begin
        Lvl1(Job);
    end;

    local procedure Lvl1(var Job: Record "H Job")
    begin
        Lvl2(Job);
    end;

    local procedure Lvl2(var Job: Record "H Job")
    begin
        Lvl3(Job);
    end;

    local procedure Lvl3(var Job: Record "H Job")
    begin
        Job.Insert();
    end;

    procedure ThreeLevels()                  // A20: no d39 (persist 3 helpers down)
    var
        Job: Record "H Job";
    begin
        Dirt(Job);
        Lvl1(Job);
    end;

    procedure FourLevels()                   // A21: d39 (4 helpers down: past the bound)
    var
        Job: Record "H Job";
    begin
        Dirt(Job);
        Lvl0(Job);
    end;

    // ---- revision 3 (spec round 2) ------------------------------------------

    procedure ReassignInLoop(Template: Record "H Job") // A22: d39 (pass 2's `Job := Template` drops pass 1's write)
    var
        Job: Record "H Job";
    begin
        repeat
            Job := Template;
            Dirt(Job);
        until Job."No." <> '';
        InsertJob(Job, false);
    end;

    local procedure Replace(var Job: Record "H Job"; Other: Record "H Job")
    begin
        Job := Other;
    end;

    procedure IndirectReplace(Other: Record "H Job") // A23: d39 (a helper replaces the record)
    var
        Job: Record "H Job";
    begin
        Dirt(Job);
        Replace(Job, Other);
        InsertJob(Job, false);
    end;

    local procedure Wrapper(var Job: Record "H Job")
    begin
        Job.DirtyMe();
    end;

    // Dirty on exit(false) through a wrapper around a call on the record.
    local procedure InitViaWrapper(var Job: Record "H Job"; Stop: Boolean): Boolean
    begin
        Wrapper(Job);
        if Stop then
            exit(false);
        Job.Validate(Name, 'w');
        exit(true);
    end;

    procedure WrappedReceiverInClean(Stop: Boolean) // A24: d39 (Stop = true: dirty via Wrapper, then exit(false))
    var
        Job: Record "H Job";
    begin
        if not InitViaWrapper(Job, Stop) then
            exit;
        InsertJob(Job, false);
    end;

    local procedure InsertWithEvent(var Job: Record "H Job")
    var
        IsHandled: Boolean;
    begin
        OnBeforeInsertJob(Job, IsHandled);
        Job.Insert();
    end;

    procedure PublisherBeforePersist()       // A25: d39 (a `var` publisher before the Insert, F5)
    var
        Job: Record "H Job";
    begin
        Dirt(Job);
        InsertWithEvent(Job);
    end;

    // Reads the record only.
    local procedure Touch(var Job: Record "H Job"): Boolean
    begin
        exit(Job.Name <> '');
    end;

    procedure PreservingHelper()             // A26: no d39 (the helper between provably leaves the record be)
    var
        Job: Record "H Job";
    begin
        Dirt(Job);
        if Touch(Job) then;
        InsertJob(Job, false);
    end;

    // Clean on exit(false): the helper before it provably makes no dirt.
    local procedure InitViaCleanHelper(var Job: Record "H Job"): Boolean
    begin
        if not Touch(Job) then
            exit(false);
        Job.Validate(Name, 'c');
        exit(true);
    end;

    procedure GuardWithCleanHelper()         // A27: no d39
    var
        Job: Record "H Job";
    begin
        if not InitViaCleanHelper(Job) then
            exit;
        InsertJob(Job, false);
    end;

    // ---- final panel round 1 ------------------------------------------------

    [TryFunction]
    local procedure TrySave(var Job: Record "H Job")
    begin
        Job.Insert();
    end;

    procedure TryHelperShape()               // A28: d39 (a [TryFunction] helper: its errors are caught)
    var
        Job: Record "H Job";
    begin
        Dirt(Job);
        if TrySave(Job) then;
    end;

    local procedure GetTemplate() Result: Record "H Job"
    begin
        Result."No." := 'T';
    end;

    procedure AssignFromCall()               // A29: d39 (`Job := <call>` replaces the record)
    var
        Job: Record "H Job";
    begin
        Dirt(Job);
        Job := GetTemplate();
        InsertJob(Job, false);
    end;

    var
        GJob: Record "H Job";
        GTemplate: Record "H Job";

    local procedure ResetGlobal()
    begin
        GJob := GTemplate;
    end;

    procedure GlobalTouch()                  // A30: d39 (a call that does not pass the global still reaches it)
    begin
        Dirt(GJob);
        ResetGlobal();
        InsertJob(GJob, false);
    end;

    local procedure CopyIn(var Job: Record "H Job")
    var
        Other: Record "H Job";
    begin
        Other.Validate(Name, 'lost');
        Job := Other;
    end;

    // Dirty on exit(false) through a helper that replaces the record.
    local procedure InitViaAssign(var Job: Record "H Job"; Stop: Boolean): Boolean
    begin
        if Stop then begin
            CopyIn(Job);
            exit(false);
        end;
        Job.Validate(Name, 'a');
        exit(true);
    end;

    procedure AssignInHelperClean(Stop: Boolean) // A31: d39
    var
        Job: Record "H Job";
    begin
        if not InitViaAssign(Job, Stop) then
            exit;
        InsertJob(Job, false);
    end;

    // Dirty on exit(false) through a direct whole-record assignment.
    local procedure InitAssignDirect(var Job: Record "H Job"; Other: Record "H Job"; Stop: Boolean): Boolean
    begin
        if Stop then begin
            Job := Other;
            exit(false);
        end;
        Job.Validate(Name, 'd');
        exit(true);
    end;

    procedure AssignDirectClean(Other: Record "H Job"; Stop: Boolean) // A32: d39
    var
        Job: Record "H Job";
    begin
        if not InitAssignDirect(Job, Other, Stop) then
            exit;
        InsertJob(Job, false);
    end;

    local procedure TouchAssert(var Job: Record "H Job"): Boolean
    begin
        asserterror Error('expected');
        exit(Job.Name <> '');
    end;

    procedure AssertInHelper()               // A33: d39 (the helper between holds an asserterror)
    var
        Job: Record "H Job";
    begin
        Dirt(Job);
        if TouchAssert(Job) then;
        InsertJob(Job, false);
    end;

    local procedure InsertJobUnless(var Job: Record "H Job"; Skip: Boolean)
    begin
        if Skip then
            exit;
        Job.Insert();
    end;

    procedure EarlyExitHelper(Skip: Boolean) // A34: d39 (the helper may exit before its Insert; no publisher)
    var
        Job: Record "H Job";
    begin
        Dirt(Job);
        InsertJobUnless(Job, Skip);
    end;

    procedure AliasSave(Template: Record "H Job") // A35: d39 (the receiver is also passed by var)
    var
        Job: Record "H Job";
    begin
        Dirt(Job);
        Job.SaveAlias(Job, Template);
    end;

    procedure DirtPublic(var Job: Record "H Job")
    begin
        Job.Validate(Name, 'p');
    end;

    procedure SaveRec(var Job: Record "H Job")
    begin
        Job.Insert();
    end;
}

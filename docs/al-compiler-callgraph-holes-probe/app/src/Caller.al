codeunit 50104 "P Caller"
{
    procedure RunStatic()
    begin
        Codeunit.Run(Codeunit::"P Target");
    end;

    procedure RunVar()
    var
        T: Codeunit "P Target";
    begin
        T.Run();
    end;

    procedure RunPage()
    var
        P: Page "P Page";
    begin
        Page.Run(Page::"P Page");
        P.RunModal();
        Report.Run(Report::"P Report");
    end;

    procedure Repeats()
    var
        T: Codeunit "P Target";
    begin
        T.A();
        T.B();
        T.A();
        T.A(); T.B();
    end;

    procedure SameLine()
    var
        T: Codeunit "P Target";
    begin
        T.B(); T.A();
    end;

    procedure Recurse(n: Integer)
    begin
        if n > 0 then
            Recurse(n - 1);
    end;

    procedure CallIface(I: Interface "P Iface")
    begin
        I.Go();
    end;

    procedure Writes()
    var
        R: Record "P Tab";
    begin
        R.Insert();
        R.Insert(false);
        R.Modify();
        R.Insert(true);
        R.DeleteAll(true);
        R.ModifyAll(Amount, 1, true);
        R.Validate(Amount, 5);
        R.Validate(Qty);
        R.Rename('X');
    end;

    procedure RefWrites()
    var
        RR: RecordRef;
        FR: FieldRef;
    begin
        RR.Open(Database::"P Tab");
        RR.Insert(true);
        FR := RR.Field(2);
        FR.Validate(5);
    end;

    procedure Sessions()
    var
        Sid: Integer;
    begin
        Session.StartSession(Sid, Codeunit::"P Target");
        TaskScheduler.CreateTask(Codeunit::"P Target", 0);
    end;

    procedure Preproc()
    var
        T: Codeunit "P Target";
    begin
#if ACTIVE
        T.A();
#else
        T.B();
#endif
    end;

    procedure Parens()
    var
        T: Codeunit "P Target";
    begin
        if T.NoParens then
            T.A();
    end;

    procedure Tern()
    var
        T: Codeunit "P Target";
        b: Boolean;
    begin
        b := T.NoParens() ? true : false;
    end;

    procedure Overloads()
    var
        T: Codeunit "P Target";
    begin
        T.Overl(1);
        T.Overl('x');
    end;

    procedure Raiser()
    var
        P: Codeunit "P Pub";
    begin
        P.Raise();
    end;

    procedure BindIt()
    var
        S: Codeunit "P Sub";
    begin
        BindSubscription(S);
    end;

    procedure BaseCall()
    var
        Cust: Record Customer;
    begin
        Cust.Insert(true);
        Cust.Validate(Name, 'x');
    end;
}

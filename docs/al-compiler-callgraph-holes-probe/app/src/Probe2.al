codeunit 50120 "P Probe2"
{
    procedure OnlyModifyAll()
    var
        R: Record "P Tab";
    begin
        R.ModifyAll(Amount, 1, true);
    end;

    procedure OnlyModifyAllNoTrigger()
    var
        R: Record "P Tab";
    begin
        R.ModifyAll(Amount, 1);
    end;

    procedure OnlyDeleteAllNoArg()
    var
        R: Record "P Tab";
    begin
        R.DeleteAll();
    end;

    procedure OnlyValidate()
    var
        R: Record "P Tab";
    begin
        R.Validate(Qty, 2);
    end;

    procedure InsertThenTrue()
    var
        R: Record "P Tab";
    begin
        R.Insert();
        R.Insert(true);
    end;

    procedure SymOnlyCodeunit()
    var
        NoSeries: Codeunit "No. Series";
        x: Code[20];
    begin
        x := NoSeries.GetNextNo('X');
    end;

    procedure SymOnlyRecord()
    var
        Cust: Record Customer;
    begin
        Cust.Insert(true);
    end;

    procedure SymOnlyRecordMethod()
    var
        Cust: Record Customer;
    begin
        Cust.GetInsertFromContactName();
    end;

    procedure WithRecArg()
    var
        R: Record "P Tab";
    begin
        Codeunit.Run(Codeunit::"P Target", R);
    end;

    procedure ReportVar()
    var
        Rep: Report "P Report";
    begin
        Rep.RunModal();
        Report.SaveAsPdf(Report::"P Report", 'x.pdf');
    end;
}

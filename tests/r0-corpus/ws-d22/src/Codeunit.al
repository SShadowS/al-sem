codeunit 50800 "D22 Demo"
{
    // FLAGGED: reads FlowField without prior CalcFields.
    procedure ReadFlowFieldWithoutCalc()
    var
        Customer: Record Customer;
        Amount: Decimal;
    begin
        Customer.Get('C0001');
        Amount := Customer."Balance (LCY)";
    end;

    // FLAGGED once (ENG-101): three reads of one FlowField on one record variable,
    // no CalcFields: one cause, one fix, ONE finding listing the three reads.
    procedure ReadFlowFieldThrice()
    var
        Customer: Record Customer;
        Amount: Decimal;
    begin
        Customer.Get('C0001');
        Amount := Customer."Balance (LCY)";
        Amount += Customer."Balance (LCY)";
        Amount += Customer."Balance (LCY)";
    end;

    // NOT FLAGGED: CalcFields is called before the FlowField read.
    procedure ReadFlowFieldAfterCalc()
    var
        Customer: Record Customer;
        Amount: Decimal;
    begin
        Customer.Get('C0001');
        Customer.CalcFields("Balance (LCY)");
        Amount := Customer."Balance (LCY)";
    end;

    // NOT FLAGGED: reads a Normal (non-Flow) field.
    procedure ReadNormalField()
    var
        Customer: Record Customer;
        N: Text;
    begin
        Customer.Get('C0001');
        N := Customer.Name;
    end;

    // FLAGGED twice (ENG-101 sol r1): one routine declared in both `#if` arms (one
    // routine id), reading the same field of a DIFFERENT table per arm: two
    // findings (one per table), never merged into one.
#if CLEAN
    procedure ReadBalanceArm()
    var
        R: Record Customer;
        Amount: Decimal;
    begin
        R.Get('C0001');
        Amount := R."Balance (LCY)";
    end;
#else
    procedure ReadBalanceArm()
    var
        R: Record Vendor;
        Amount: Decimal;
    begin
        R.Get('V0001');
        Amount := R."Balance (LCY)";
    end;
#endif
}

# Which RecordRef methods move its table or temporary state (ENG-160, BC 28.4)

Measured 2026-11-20 on the kraken BC 28.4 sidecar (`http://bc`, instance `BC`) with throwaway
test apps P159 (`71590`) and P160 (`71600`), unpublished afterwards. Each scenario is its own
`[Test]` with its own variables. It ends in `Error('MEASURED …')`, or in the platform's own
error, which is the result. Customer is table 18 and Vendor is table 23. The aliasing
measurements behind ENG-145 are in `2026-11-20-eng145-recordref-aliasing.md`.

## Results

| Scenario | Result |
|---|---|
| `R.Open(Customer); O.Open(Vendor); R.Copy(O)` | runtime error "Both target and source tables must refer to the same table." |
| `R.GetTable(Cust)` (physical); `O.GetTable(TmpCust)`; `R.Copy(O)` | R stays physical |
| `R.Open(Customer)`; `O.GetTable(TmpCust)`; `R.Copy(O, true)` | runtime error "The COPY function can only be used with the shareTable argument set to true if both records are temporary." |
| `R.Open(Customer, true)`; `O.GetTable(Cust)` (physical); `R.Copy(O)` | R stays 18, temporary |
| `R.GetTable(TmpA)`; `O.GetTable(TmpB)`; `R.Copy(O, true)` | R stays 18, temporary |
| `O.GetTable(Cust)` (physical or temporary); `R.Copy(O)` on a RecordRef never opened | runtime error "The record is not open." |
| `R.Open(Vendor); R.Close(); O.GetTable(Cust); R.Copy(O)` | runtime error "The record is not open." |
| `R.Open(Customer); R.Get(Vendor.RecordId)` (record exists) | true, R is 23 |
| `R.Open(Customer, true); R.Get(Vendor.RecordId)` | true, R is 23, **physical** |
| `R.Open(Customer); A := R; A.Get(Vendor.RecordId)` | true, R and A both 23 (one handle) |
| `R.Open(Customer); R.Get(<deleted Vendor's RecordId>)` | false, R stays 18 |
| `R.Open(Customer, true); R.Get(<physical Customer's RecordId>)` | false, R stays 18, temporary |
| `R.Get(Vendor.RecordId)` on a RecordRef never opened | true, R is 23 |
| `R.Open(Customer); R.GetBySystemId(<a Vendor's SystemId>)` | false, R stays 18 |
| `R.Open(Customer); F := R.Field(1); A := F.Record(); A.GetTable(Vend)` | R stays 18, A is 23: `FieldRef.Record()` hands back no shared handle |
| `R.Open(Customer); BindByValue(R)`, whose callee runs `X.GetTable(Vend)` on its by-value `X: RecordRef` | R is 23: a by-value RecordRef parameter shares the caller's handle |
| `R.Open(Customer); A := R; Clear(A)` | R stays 18, A is 0: `Clear` detaches only A |

## What the engine reads from it

- **`Get(RecordId)` moves the RecordRef** (through every alias) to the RecordId's table when it finds the
  record. A different table is physical. On the same table the temporary state stays. So:
  table unknown, temporary state unknown. A `Get` that finds nothing moves nothing, but the
  engine cannot tell the outcomes apart.
- **`Copy` moves nothing**: neither the table (another table is a runtime error) nor the
  temporary state (kept in all three combinations measured), and it binds no RecordRef that is
  not open (a runtime error).
- **`GetBySystemId` moves nothing** (it looks only in the RecordRef's own table).
- With `Open`, `GetTable` and `Close` (ENG-145's note), these are the RecordRef methods that bind
  or unbind it: `src/program/model/recref_alias.rs`'s `recref_method_move` is the one list.
- In the IR, `R.Get(…)`, `R.Copy(…)`, `R.Insert(…)` and the other record methods on a RecordRef
  are record OPERATIONS, never call sites (`ir_walk`). A reader that scans only call sites for
  `get` never sees one: that was ENG-160's bug.

## Probe sources

P159 (scenarios S1–S6 above; S1/S1b are the two `Copy` rows):

```al
codeunit 71590 "P159 Rebind Probe"
{
    Subtype = Test;

    [Test]
    procedure S1CopyOtherTable()
    var
        R, O : RecordRef;
    begin
        R.Open(Database::Customer);
        O.Open(Database::Vendor);
        R.Copy(O);
        Error('MEASURED S1 copy other table: R=%1', R.Number);
    end;

    [Test]
    procedure S1bCopyTempSameTable()
    var
        R, O : RecordRef;
        TmpCust: Record Customer temporary;
        Cust: Record Customer;
        Before: Boolean;
    begin
        R.GetTable(Cust);
        Before := R.IsTemporary();
        O.GetTable(TmpCust);
        R.Copy(O);
        Error('MEASURED S1b copy temp same table: R temp before=%1 after=%2', Before, R.IsTemporary());
    end;

    [Test]
    procedure S2GetRecordIdOtherTable()
    var
        R: RecordRef;
        Vend: Record Vendor;
        Id: RecordId;
        Ok: Boolean;
    begin
        Vend.Init();
        Vend."No." := 'P159X';
        Vend.Insert();
        Id := Vend.RecordId;
        R.Open(Database::Customer);
        Ok := R.Get(Id);
        Error('MEASURED S2 get recordid other table: ok=%1 R=%2', Ok, R.Number);
    end;

    [Test]
    procedure S3FieldRefRecordAlias()
    var
        R, A : RecordRef;
        F: FieldRef;
        Vend: Record Vendor;
    begin
        R.Open(Database::Customer);
        F := R.Field(1);
        A := F.Record();
        A.GetTable(Vend);
        Error('MEASURED S3 fieldref.record alias gettable: R=%1 A=%2', R.Number, A.Number);
    end;

    [Test]
    procedure S4ByValueParam()
    var
        R: RecordRef;
    begin
        R.Open(Database::Customer);
        BindByValue(R);
        Error('MEASURED S4 byvalue param gettable: R=%1', R.Number);
    end;

    [Test]
    procedure S6ClearAlias()
    var
        R, A : RecordRef;
    begin
        R.Open(Database::Customer);
        A := R;
        Clear(A);
        Error('MEASURED S6 clear alias: R=%1 A=%2', R.Number, A.Number);
    end;

    local procedure BindByValue(X: RecordRef)
    var
        Vend: Record Vendor;
    begin
        X.GetTable(Vend);
    end;
}
```

P160 rounds 1 to 3 (T1–T6, U1–U3, V1–V3), one codeunit per round; shown merged:

```al
codeunit 71600 "P160 Get Probe"
{
    Subtype = Test;

    [Test]
    procedure T1CopyShareTableTemp()
    var
        R, O : RecordRef;
        TmpCust: Record Customer temporary;
    begin
        R.Open(Database::Customer);
        O.GetTable(TmpCust);
        R.Copy(O, true);
        Error('MEASURED T1 copy sharetable from temp: R=%1 temp=%2', R.Number, R.IsTemporary());
    end;

    [Test]
    procedure T2GetOnTempRef()
    var
        R: RecordRef;
        Vend: Record Vendor;
        Ok: Boolean;
    begin
        Vend.Init();
        Vend."No." := 'P160A';
        Vend.Insert();
        R.Open(Database::Customer, true);
        Ok := R.Get(Vend.RecordId);
        Error('MEASURED T2 get on temp ref: ok=%1 R=%2 temp=%3', Ok, R.Number, R.IsTemporary());
    end;

    [Test]
    procedure T3GetThroughAlias()
    var
        R, A : RecordRef;
        Vend: Record Vendor;
        Ok: Boolean;
    begin
        Vend.Init();
        Vend."No." := 'P160B';
        Vend.Insert();
        R.Open(Database::Customer);
        A := R;
        Ok := A.Get(Vend.RecordId);
        Error('MEASURED T3 get through alias: ok=%1 R=%2 A=%3', Ok, R.Number, A.Number);
    end;

    [Test]
    procedure T4GetMissingOtherTable()
    var
        R: RecordRef;
        Vend: Record Vendor;
        Ok: Boolean;
    begin
        Vend.Init();
        Vend."No." := 'P160C';
        Vend.Insert();
        R.Open(Database::Customer);
        Vend.Delete();
        Ok := R.Get(Vend.RecordId);
        Error('MEASURED T4 get missing other table: ok=%1 R=%2', Ok, R.Number);
    end;

    [Test]
    procedure T5GetBySystemIdOther()
    var
        R: RecordRef;
        Vend: Record Vendor;
        Ok: Boolean;
    begin
        Vend.Init();
        Vend."No." := 'P160D';
        Vend.Insert();
        R.Open(Database::Customer);
        Ok := R.GetBySystemId(Vend.SystemId);
        Error('MEASURED T5 getbysystemid other table id: ok=%1 R=%2', Ok, R.Number);
    end;

    [Test]
    procedure T6GetClosedRef()
    var
        R: RecordRef;
        Vend: Record Vendor;
        Ok: Boolean;
    begin
        Vend.Init();
        Vend."No." := 'P160E';
        Vend.Insert();
        Ok := R.Get(Vend.RecordId);
        Error('MEASURED T6 get on never-opened ref: ok=%1 R=%2', Ok, R.Number);
    end;

    [Test]
    procedure U1CopyPhysicalIntoTemp()
    var
        R, O : RecordRef;
        Cust: Record Customer;
    begin
        R.Open(Database::Customer, true);
        O.GetTable(Cust);
        R.Copy(O);
        Error('MEASURED U1 copy physical into temp: R=%1 temp=%2', R.Number, R.IsTemporary());
    end;

    [Test]
    procedure U2GetSameTableOnTemp()
    var
        R: RecordRef;
        Cust: Record Customer;
        Ok: Boolean;
    begin
        Cust.Init();
        Cust."No." := 'P160U2';
        Cust.Insert();
        R.Open(Database::Customer, true);
        Ok := R.Get(Cust.RecordId);
        Error('MEASURED U2 get same table on temp: ok=%1 R=%2 temp=%3', Ok, R.Number, R.IsTemporary());
    end;

    [Test]
    procedure U3CopyTempIntoTempShare()
    var
        R, O : RecordRef;
        TmpA, TmpB : Record Customer temporary;
    begin
        R.GetTable(TmpA);
        O.GetTable(TmpB);
        R.Copy(O, true);
        Error('MEASURED U3 copy temp into temp share: R=%1 temp=%2', R.Number, R.IsTemporary());
    end;

    [Test]
    procedure V1CopyIntoUnopened()
    var
        R, O : RecordRef;
        Cust: Record Customer;
    begin
        O.GetTable(Cust);
        R.Copy(O);
        Error('MEASURED V1 copy into unopened: R=%1 temp=%2', R.Number, R.IsTemporary());
    end;

    [Test]
    procedure V2CopyTempIntoUnopened()
    var
        R, O : RecordRef;
        TmpCust: Record Customer temporary;
    begin
        O.GetTable(TmpCust);
        R.Copy(O);
        Error('MEASURED V2 copy temp into unopened: R=%1 temp=%2', R.Number, R.IsTemporary());
    end;

    [Test]
    procedure V3CopyIntoClosed()
    var
        R, O : RecordRef;
        Cust: Record Customer;
    begin
        R.Open(Database::Vendor);
        R.Close();
        O.GetTable(Cust);
        R.Copy(O);
        Error('MEASURED V3 copy into closed: R=%1', R.Number);
    end;
}
```

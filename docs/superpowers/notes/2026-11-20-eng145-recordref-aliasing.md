# ENG-145: RecordRef aliasing, measured on BC 28.4

Question: after `A := Ref` between two RecordRef variables, does rebinding `A` rebind `Ref`?
The literal pruning's RecordRef flow (G34/G35) assumed it shares, without a measurement; G36's
field guard and `record_types`' temp derivation did not look at aliases at all (sol, ENG-30 r6
and r8).

## How

A throwaway test codeunit on the kraken BC 28.4 sidecar (`http://bc`, instance `BC`), compiled
with alc 18.0.41, published at Dev scope with bc-dev, run with `bcdev_test_run`, then
uninstalled and unpublished. Every scenario has its own variables; the numbers travel in the
test's `Error` text. Customer = table 18, Vendor = table 23. Source below.

## Results (2026-11-20)

| Scenario | Code | Observed | Meaning |
|---|---|---|---|
| S1 | `Ref.Open(18); A := Ref; A.Close(); A.Open(23)` | `Ref.Number` = 18 | `Close` detaches `A` only |
| S2 | `Ref.Open(18); A := Ref; A.GetTable(Vend)` | `Ref.Number` = 23 | `GetTable` through the alias rebinds `Ref` |
| S3 | `Ref.Open(18); D := Ref.Duplicate(); D.Close(); D.Open(23)` | `Ref.Number` = 18 | control: `Duplicate` shares nothing |
| S4 | `Ref.GetTable(TmpCust); A := Ref; A.GetTable(PhysCust)` | `Ref.IsTemporary` Yes -> No | the temporary state follows the alias |
| S5 | `Ref.Open(18); A := Ref; A.Open(23)` (no `Close`) | error "The record is already open."; `Ref.Number` = 18 | `Open` cannot rebind a shared handle |
| S6 | `Ref.Open(18); A := Ref; Ref.GetTable(Vend)` | `A.Number` = 23 | symmetric |
| S7 | `Ref.Open(18); A := Ref; Ref.Close()` | `A.Number` = 18 | `Close` detaches `Ref` only |
| S7b | then `A.GetTable(Vend)` | `Ref.Number` = 0 | the closed variable is no longer reached |

So `A := B` makes both variables hold one record: `GetTable` through either rebinds both, with
the record's temporary state; `Close` detaches only the variable it is called on, and `Open`
needs a closed (so detached) variable.

## Use

`program::model::recref_alias::AliasClasses` is the one place the alias classes come from
(`recref_flow`, `field_guard`, `record_types`). Readers treat every rebinding through a class
member (`Open` and `Close` included) as reaching the class: sound, coarser than the platform.
Making `Open`/`Close` precise is ENG-157.

## Probe source

```al
codeunit 71450 "P145 Alias Probe"
{
    Subtype = Test;

    [Test]
    procedure MeasureAliasing()
    var
        Ref1, A1 : RecordRef;
        Ref2, A2 : RecordRef;
        Ref3, D3 : RecordRef;
        Ref4, A4 : RecordRef;
        Vend: Record Vendor;
        TmpCust: Record Customer temporary;
        PhysCust: Record Customer;
        S1, S2, S3 : Integer;
        S4Before, S4After : Boolean;
    begin
        Ref1.Open(Database::Customer);
        A1 := Ref1;
        A1.Close();
        A1.Open(Database::Vendor);
        S1 := Ref1.Number;

        Ref2.Open(Database::Customer);
        A2 := Ref2;
        A2.GetTable(Vend);
        S2 := Ref2.Number;

        Ref3.Open(Database::Customer);
        D3 := Ref3.Duplicate();
        D3.Close();
        D3.Open(Database::Vendor);
        S3 := Ref3.Number;

        Ref4.GetTable(TmpCust);
        S4Before := Ref4.IsTemporary();
        A4 := Ref4;
        A4.GetTable(PhysCust);
        S4After := Ref4.IsTemporary();

        Error('MEASURED S1=%1 S2=%2 S3=%3 S4=%4/%5', S1, S2, S3, S4Before, S4After);
    end;

    [Test]
    procedure MeasureOpenAndSymmetry()
    var
        Ref5, A5 : RecordRef;
        Ref6, A6 : RecordRef;
        Ref7, A7 : RecordRef;
        Vend: Record Vendor;
        S5, S6, S7, S7b : Integer;
        S5Err: Text;
    begin
        Ref5.Open(Database::Customer);
        A5 := Ref5;
        ClearLastError();
        if not TryOpen(A5) then
            S5Err := GetLastErrorText();
        S5 := Ref5.Number;

        Ref6.Open(Database::Customer);
        A6 := Ref6;
        Ref6.GetTable(Vend);
        S6 := A6.Number;

        Ref7.Open(Database::Customer);
        A7 := Ref7;
        Ref7.Close();
        S7 := A7.Number;
        A7.GetTable(Vend);
        S7b := Ref7.Number;

        Error('MEASURED S5=%1 err=[%2] S6=%3 S7=%4 S7b=%5', S5, S5Err, S6, S7, S7b);
    end;

    [TryFunction]
    local procedure TryOpen(var R: RecordRef)
    begin
        R.Open(Database::Vendor);
    end;
}
```

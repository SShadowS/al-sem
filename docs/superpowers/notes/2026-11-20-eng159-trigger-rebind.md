# Can a write's own trigger move the RecordRef it writes through? (ENG-159, BC 28.4)

Measured 2026-11-20 on the kraken BC 28.4 sidecar (`http://bc`, instance `BC`) with the
throwaway test app P159b (`71610`–`71613`, unpublished afterwards). The ENG-145 measurements
(`2026-11-20-eng145-recordref-aliasing.md`) show `A := R` shares one handle and a `GetTable`
through either rebinds both. The question here: a RecordRef `R` whose handle a SingleInstance
store keeps (`G := R`) writes, and the write's own trigger (or a table event subscriber) calls the
store, which rebinds `G` to another table's record. Does the write then go to the other table?

## Results

| Scenario | Result |
|---|---|
| X1: `R.Modify(true)`, `OnModify` calls `Store.Rebind()` (`G.GetTable(B)`) | runtime error "The record is not open." at the `Modify` |
| X2: `R.Modify(false)`, an `OnBeforeModifyEvent` subscriber calls `Store.Rebind()` | runtime error "The record is not open." at the `Modify` |
| X3: `R.Insert(true)`, `OnInsert` calls `Store.Rebind()` | platform crash: `ObjectDisposedException` on `NavRecord` in `NavGlobalTriggers.InsertAsync` |
| X4 (control): `R.Modify(true)`, nothing rebinds | PW A's row changed, PW B's not, `R.Number` 71610 |

## What the engine reads from it

A write whose own trigger or table event rebinds the handle it writes through does not write to
the other table: it fails. So the write's target is the table its RecordRef held when the write
began, and the user code the write runs may rebind kept records only for what runs AFTER it.
`field_guard` keeps a guarded write's guard for that write itself and ends it for the later
operations (ENG-159). A runtime error inside a `TryFunction` lets the caller go on with the handle
rebound; that is "after", and covered the same way.

## Probe source

`/work/.al-sem/engine-notes/eng159-trigger-probe/src/Probe.al` (the tables, the SingleInstance
`PW Store` with its event subscriber, and the four tests). Its core:

```al
codeunit 71612 "PW Store"
{
    SingleInstance = true;
    var
        G: RecordRef;
    procedure Keep(var R: RecordRef; Trig: Boolean; Evt: Boolean)
    begin
        G := R;
        // arms the trigger (Trig) or the OnBeforeModifyEvent subscriber (Evt)
    end;
    procedure Rebind()
    var
        B: Record "PW B";
    begin
        B.FindFirst();
        G.GetTable(B);
    end;
}

// X1
R.Open(Database::"PW A");
R.FindFirst();
R.Field(2).Value := 'changed';
S.Keep(R, true, false);
R.Modify(true);   // OnModify: S.Rebind() -> "The record is not open."
```

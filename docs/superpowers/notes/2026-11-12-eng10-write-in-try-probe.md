# ENG-10 probe: a write inside a used TryFunction (engine limits L23, item 6)

Measured 2026-11-12 on the BC 28.4 Linux sidecar (`http://bc`, instance `BC`, on premises; the
server's `DisableWriteInsideTryFunctions` was not changed, so it has its default). App
`e10e10e0-0000-4000-8000-0000000000e9` ("ENG10 Write In TryFunction Probe", ids 50940-50959),
removed after the run.

**First run (T00-T12) confirmed on Windows** by the owner (q-20261112T104134-338e9d4a): BC
28.0.54946.56477 on premises, Cronus28, test runner 130451, `DisableWriteInsideTryFunctions=true`,
TestPermissions=Disabled with a permission set. All 13 outcomes match the sidecar: T00, T02, T08
ok; T01, T03-T07, T09, T11, T12 raise "Call to the function X is not allowed…", not caught; T10's
checked run catches it (the try returns true). One difference: the scope in the message reads
`'RunTests'` on Windows, not `'RootMethodScope'`, so nothing may match on the scope name. **The
second and third runs (T13-T20) are sidecar-only**: not part of that confirmation.

## Question

Learn ("Handling errors using try methods", updated 2025-06-23), verbatim: "For Business Central
online, there are no restrictions on performing write transactions in try methods. For Business
Central on-premises, the Business Central Server prevents database write transactions within try
methods by default. If a try method contains a database write transaction, a runtime error
occurs. You can allow write transactions by setting the `DisableWriteInsideTryFunctions`
parameter in the Business Central Server configuration to `false`." The server settings page
gives the default `true`. Learn does not say whether the try call that uses the result catches
that runtime error. If it does not, `prove throws-error` under-reports it. (Online was not
measured; the sidecar is on premises.)

Learn also says "Arguments for a try function are evaluated inside the try function": its example
`Result := DoWork(DoInsert(Rec))` is "Not allowed", so a write in an argument of a used try call
is refused too (not measured here).

## Result

Each scenario is its own `[Test]`. It ends with `Error('MEASURED …')` when control gets there; a
failure with another text is BC's own error, which nothing caught.

| Scenario | Outcome |
|---|---|
| T00 control: `Insert`, no try function anywhere | ok, the row is written |
| T01 `Ok := TryInsert()` | **not caught**: "Call to the function 'INSERT' is not allowed inside the call to 'RootMethodScope' when it is used as a TryFunction." |
| T12 `if not TryInsert() then` | **not caught**, the same error |
| T02 `TryInsert();` (result not used) | ok, the row is written (an ordinary call) |
| T03 used try whose callee does the `Insert` | **not caught**, the same error |
| T04 `Modify`, T05 `ModifyAll`, T06 `Delete`, T07 `LockTable` (then `FindFirst`), T09 `Commit()` | **not caught**: the same error naming `MODIFY`, `MODIFYALL`, `DELETE`, `LOCKTABLE`, `COMMIT` |
| T08 `Insert` on a temporary record | ok |
| T10 `if Codeunit.Run(Writer) then;` inside the try (Writer's `OnRun` inserts) | the run returns false and catches the refusal ("…'INSERT' is not allowed…" is the last error); the try returns true |
| T11 `Codeunit.Run(Writer);` inside the try (unchecked) | **not caught**, the same error, raised in Writer's `OnRun` |

Second run, same day, **sidecar only** (review: shapes the first run did not cover):

| Scenario | Outcome |
|---|---|
| T13 `Ok := TryTakes(InsertReturning())`: the write is in the try call's argument | **not caught**: "…'INSERT' is not allowed inside the call to 'RootMethodScope'…", raised in `InsertReturning` |
| T14 `Ok := TryOuter()`, whose body is `if TryInsert() then;` | **not caught** (an outer used try does not catch it either) |
| T15 `Ok := TryAssertWrite()`, whose body is `asserterror DoInsert();` | caught by `asserterror`; the try returns true |
| T16 `Ok := TryRunsInnerTry()`, whose body is `if Codeunit.Run(Inner) then;`, `Inner` does `Ok := TryW()` and `TryW` inserts | caught by the checked run; the outer try returns true |
| T17 `TryOuter();` (result not used) around `if TryInsert() then;` | **not caught** ("…inside the call to 'T17_UnusedTry_InnerUsedTryWrites'…"): the inner used try is refused as before |

Third run, same day, **sidecar only** (review: which uses of the value make it a try call for the write):

| Scenario | Outcome |
|---|---|
| T18 `exit(TryInsert())` (in a helper whose result is assigned) | **not caught**: "…'INSERT' is not allowed inside the call to 'T18_ExitTry_Insert'…" |
| T19 `TakesBool(TryInsert())`: the value as an argument | **not caught**, the same error |
| T20 `Ok := AlwaysTrue() and TryInsert()` | **not caught**, the same error |
| T02 (again) `TryInsert();` as a statement | ok, the row is written |

So for the write refusal every use of the value counts as a try call; only a call in statement
position is ordinary. (Whether those wider shapes also CATCH an `Error()` was not measured; the
trap rule keeps reading only `Ok :=` and whole conditions, which is the safe side for catching.)

So: with the result used, a database write (and `LockTable`, `Commit()`) anywhere the try call
reaches raises an error that the try call does **not** catch; a checked `Codeunit.Run` between
them catches it as it catches any error. With the result not used the call is ordinary and the
write is allowed. A temporary record's write is allowed.

## On CDO (master d887bb47)

Of CDO's 30 `[TryFunction]` procedures, 5 have cones that may write (cross-app digest):

- `CDO Contnia Online PDF Mgt.`::`TryPerformRequest`, `TryMergePdf` and
  `TryPerformSignatureRequest` reach Continia Core's token cache. Path:
  `PerformRequest` → `if not TryPerformRequest(...)` (a used try,
  `CDOContniaOnlinePDFMgt.Codeunit.al:833`) → `CDO Http Management`::`Request`
  (`Core.RequestAccessToken(false)`, `CDOHttpManagement.Codeunit.al:212`) → `CSC Core`::
  `RequestAccessToken` → `Insert` on `CSC Access Token` (Continia Core `AccessToken.Table.al:115`).
  CDO knows this shape: it is bug #80680, and `PerformRequest` calls `EnsureAccessTokenWarm()`
  before the try so that the token is cached and the insert does not run inside it; the code
  comment says a caller that reaches the try another way must warm the token first, and that a
  subscriber handling `OnBeforeEnsureAccessTokenWarm` without warming "reintroduces bug #80680".
  That guard depends on data (a cached token), which the engine does not read, so the engine's
  answer is "may raise", not a confirmed defect. Whether the token can expire between the warm and
  the request was not checked.
- `CDO DO-eDocs Notific. Handler`::`ShowMigrationNotification` and `CDO Conflict Notific.
  Handler`::`TryShowConflictNotification` may write `My Notifications`.
- `CDO Legacy eDoc Dispatcher`::`TryRunReport` runs a report chosen at run time (its writes are
  unknown).

Whether alc or `altool graph` report a write inside a try function was not checked.

## Probe source

`app.json`: id above, `"platform": "28.0.0.0"`, `"runtime": "16.0"`, no dependencies.

`src/Probe.al`:

```al
table 50940 "WT Row"
{
    fields { field(1; K; Integer) { } field(2; V; Integer) { } }
    keys { key(PK; K) { } }
}
codeunit 50941 "WT Writer"
{
    trigger OnRun()
    var
        R: Record "WT Row";
    begin
        R.K := 900000 + Random(99999);
        R.Insert();
    end;
}
codeunit 50942 "WT Probe Test"
{
    Subtype = Test;

    local procedure Seed()
    var
        R: Record "WT Row";
    begin
        R.DeleteAll();
        R.K := 1;
        R.V := 0;
        R.Insert();
        Commit();
    end;

    local procedure Rows(): Integer
    var
        R: Record "WT Row";
    begin
        exit(R.Count());
    end;

    local procedure Measure(Name: Text; Ok: Boolean)
    begin
        Error('MEASURED %1 ok=%2 rows=%3 err=%4', Name, Ok, Rows(), GetLastErrorText());
    end;

    [Test]
    procedure T00_NoTry_Insert()
    begin
        Seed();
        ClearLastError();
        DoInsert();
        Measure('T00 no try, Insert', true);
    end;

    [Test]
    procedure T01_UsedTry_Insert()
    var
        Ok: Boolean;
    begin
        Seed();
        ClearLastError();
        Ok := TryInsert();
        Measure('T01 used try, Insert', Ok);
    end;

    [Test]
    procedure T02_UnusedTry_Insert()
    begin
        Seed();
        ClearLastError();
        TryInsert();
        Measure('T02 unused try, Insert', true);
    end;

    [Test]
    procedure T03_UsedTry_NestedInsert()
    var
        Ok: Boolean;
    begin
        Seed();
        ClearLastError();
        Ok := TryNested();
        Measure('T03 used try, Insert in a callee', Ok);
    end;

    [Test]
    procedure T04_UsedTry_Modify()
    var
        Ok: Boolean;
    begin
        Seed();
        ClearLastError();
        Ok := TryModify();
        Measure('T04 used try, Modify', Ok);
    end;

    [Test]
    procedure T05_UsedTry_ModifyAll()
    var
        Ok: Boolean;
    begin
        Seed();
        ClearLastError();
        Ok := TryModifyAll();
        Measure('T05 used try, ModifyAll', Ok);
    end;

    [Test]
    procedure T06_UsedTry_Delete()
    var
        Ok: Boolean;
    begin
        Seed();
        ClearLastError();
        Ok := TryDelete();
        Measure('T06 used try, Delete', Ok);
    end;

    [Test]
    procedure T07_UsedTry_LockTableRead()
    var
        Ok: Boolean;
    begin
        Seed();
        ClearLastError();
        Ok := TryLock();
        Measure('T07 used try, LockTable+FindFirst', Ok);
    end;

    [Test]
    procedure T08_UsedTry_TempInsert()
    var
        Ok: Boolean;
    begin
        Seed();
        ClearLastError();
        Ok := TryTempInsert();
        Measure('T08 used try, temporary Insert', Ok);
    end;

    [Test]
    procedure T09_UsedTry_Commit()
    var
        Ok: Boolean;
    begin
        Seed();
        ClearLastError();
        Ok := TryCommit();
        Measure('T09 used try, Commit', Ok);
    end;

    [Test]
    procedure T10_UsedTry_CheckedRunWriter()
    var
        Ok: Boolean;
    begin
        Seed();
        ClearLastError();
        Ok := TryCheckedRun();
        Measure('T10 used try, checked Codeunit.Run of a writer', Ok);
    end;

    [Test]
    procedure T11_UsedTry_UncheckedRunWriter()
    var
        Ok: Boolean;
    begin
        Seed();
        ClearLastError();
        Ok := TryUncheckedRun();
        Measure('T11 used try, unchecked Codeunit.Run of a writer', Ok);
    end;

    [Test]
    procedure T12_IfTry_Insert()
    var
        Ok: Boolean;
    begin
        Seed();
        ClearLastError();
        Ok := true;
        if not TryInsert() then
            Ok := false;
        Measure('T12 if not TryInsert()', Ok);
    end;

    [TryFunction]
    local procedure TryInsert()
    var
        R: Record "WT Row";
    begin
        R.K := 2;
        R.Insert();
    end;

    [TryFunction]
    local procedure TryNested()
    begin
        DoInsert();
    end;

    local procedure DoInsert()
    var
        R: Record "WT Row";
    begin
        R.K := 3;
        R.Insert();
    end;

    [TryFunction]
    local procedure TryModify()
    var
        R: Record "WT Row";
    begin
        R.Get(1);
        R.V := 7;
        R.Modify();
    end;

    [TryFunction]
    local procedure TryModifyAll()
    var
        R: Record "WT Row";
    begin
        R.ModifyAll(V, 8);
    end;

    [TryFunction]
    local procedure TryDelete()
    var
        R: Record "WT Row";
    begin
        R.Get(1);
        R.Delete();
    end;

    [TryFunction]
    local procedure TryLock()
    var
        R: Record "WT Row";
    begin
        R.LockTable();
        R.FindFirst();
    end;

    [TryFunction]
    local procedure TryTempInsert()
    var
        T: Record "WT Row" temporary;
    begin
        T.K := 4;
        T.Insert();
    end;

    [TryFunction]
    local procedure TryCommit()
    begin
        Commit();
    end;

    [TryFunction]
    local procedure TryCheckedRun()
    begin
        if Codeunit.Run(Codeunit::"WT Writer") then;
    end;

    [TryFunction]
    local procedure TryUncheckedRun()
    begin
        Codeunit.Run(Codeunit::"WT Writer");
    end;
}
```

Second run: add this codeunit before codeunit 50942, and these tests and helpers inside it.

```al
codeunit 50943 "WT Inner Try"
{
    trigger OnRun()
    var
        Ok: Boolean;
    begin
        Ok := TryW();
        Error('INNER ok=%1 err=%2', Ok, GetLastErrorText());
    end;

    [TryFunction]
    local procedure TryW()
    var
        R: Record "WT Row";
    begin
        R.K := 6;
        R.Insert();
    end;
}
```

```al
    [Test]
    procedure T13_UsedTry_WriteInArgument()
    var
        Ok: Boolean;
    begin
        Seed();
        ClearLastError();
        Ok := TryTakes(InsertReturning());
        Measure('T13 used try, write in its argument', Ok);
    end;

    [Test]
    procedure T14_OuterUsedTry_InnerUsedTryWrites()
    var
        Ok: Boolean;
    begin
        Seed();
        ClearLastError();
        Ok := TryOuter();
        Measure('T14 outer used try around an inner used try that writes', Ok);
    end;

    [Test]
    procedure T15_UsedTry_AsserterrorAroundWrite()
    var
        Ok: Boolean;
    begin
        Seed();
        ClearLastError();
        Ok := TryAssertWrite();
        Measure('T15 used try, asserterror around the write', Ok);
    end;

    [Test]
    procedure T16_OuterTry_CheckedRun_InnerTryWrites()
    var
        Ok: Boolean;
    begin
        Seed();
        ClearLastError();
        Ok := TryRunsInnerTry();
        Measure('T16 outer try, checked run, inner used try that writes', Ok);
    end;

    [Test]
    procedure T17_UnusedTry_InnerUsedTryWrites()
    begin
        Seed();
        ClearLastError();
        TryOuter();
        Measure('T17 unused outer try around an inner used try that writes', true);
    end;

    local procedure InsertReturning(): Boolean
    var
        R: Record "WT Row";
    begin
        R.K := 5;
        exit(R.Insert());
    end;

    [TryFunction]
    local procedure TryTakes(B: Boolean)
    begin
    end;

    [TryFunction]
    local procedure TryOuter()
    begin
        if TryInsert() then;
    end;

    [TryFunction]
    local procedure TryAssertWrite()
    begin
        asserterror DoInsert();
    end;

    [TryFunction]
    local procedure TryRunsInnerTry()
    begin
        if Codeunit.Run(Codeunit::"WT Inner Try") then;
    end;
```

Third run: add inside codeunit 50942.

```al
    [Test]
    procedure T18_ExitTry_Insert()
    var
        Ok: Boolean;
    begin
        Seed();
        ClearLastError();
        Ok := ExitTry();
        Measure('T18 exit(TryInsert())', Ok);
    end;

    [Test]
    procedure T19_TryAsArgument_Insert()
    begin
        Seed();
        ClearLastError();
        TakesBool(TryInsert());
        Measure('T19 TryInsert() as an argument', true);
    end;

    [Test]
    procedure T20_TryInAnd_Insert()
    var
        Ok: Boolean;
    begin
        Seed();
        ClearLastError();
        Ok := AlwaysTrue() and TryInsert();
        Measure('T20 Ok := AlwaysTrue() and TryInsert()', Ok);
    end;

    local procedure ExitTry(): Boolean
    begin
        exit(TryInsert());
    end;

    local procedure TakesBool(B: Boolean)
    begin
    end;

    local procedure AlwaysTrue(): Boolean
    begin
        exit(true);
    end;
```

## To run it on Windows

Publish the app on a BC 28 Cronus on premises (test framework installed, default
`DisableWriteInsideTryFunctions`), run codeunit 50942 `WT Probe Test`, and compare each test's
failure text with the table above. Also note the server's `DisableWriteInsideTryFunctions` value.

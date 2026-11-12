# ENG-09 probe: what a checked `Codeunit.Run` commits (L16 item 2, L28)

Measured 2026-11-11 on the BC 28.4 Linux sidecar (`http://bc`, instance `BC`). The probe app was
removed after the run.

**Confirmed on Windows** by the owner, 2026-11-12 (BC 28.0.54946.56477, Cronus28, test runner
130451, isolation off; q-20261112T021007-384a506a). The shape: an `Insert` pending, then
`Ok := Codeunit.Run(C)`. The run itself raises "An error occurred and the transaction is
stopped." The error is **not caught by the checked form**: the run does not return `false`. This
holds whether `C` succeeds or calls `Error()`. The caller's row count afterwards is 0, so the
insert was not committed. Controls: with no run the count is 0; with `Commit()` before `Error()`
it is 1. The sidecar run did not show the uncaught part, because F ran inside a `[TryFunction]`.

## What it tests

After a run, a second checked `Codeunit.Run` (inside a `[TryFunction]`) fails if and only if a
write is pending. So "second run ok" means the first run's write was committed, and "second run
fails" means a write is still pending.

## Result

The test fails on purpose: its `Error('MEASURED…')` is how the numbers come out.

| Scenario | Second checked run | Reading |
|---|---|---|
| A: unchecked `Codeunit.Run(Writer)` | fails: "An error occurred and the transaction is stopped." | the write stays pending |
| B: `if Codeunit.Run(Writer) then` | ok | the run committed its own `Insert` (Writer has no `Commit()`) |
| C: B under caller `[CommitBehavior(Ignore)]` | ok | committed all the same |
| D: B under caller `[CommitBehavior(Error)]` | ok, no error | committed all the same |
| E: `Ok := W.Run()` (codeunit variable) | ok | committed |
| F: control, the caller's own `Insert`, then a checked run | fails: "An error occurred and the transaction is stopped." | a write pending at a checked run is an error at the run (L28) |

So a checked run commits only its own transaction. It never commits a write of the caller: a
write pending at the run makes the run itself fail (F).

## To run it on Windows

Publish the app below on a BC 28 Cronus (test framework installed), run codeunit 50985
`CR Probe Test`, and read the `MEASURED` error text. The sidecar's outcome, in order: A ends
`second-checked-run=ERR …`, B, C and D end `second-checked-run=ok`, E ends `second=ok`, F ends
`second=ERR …`. (Only these outcomes were recorded, not the full text.)

`app.json`:

```json
{
  "id": "e09e09e0-0000-4000-8000-0000000000e9",
  "name": "ENG09 Checked Run Commit Probe",
  "publisher": "al-sem probes",
  "version": "1.0.0.0",
  "platform": "28.0.0.0",
  "runtime": "16.0",
  "idRanges": [ { "from": 50980, "to": 50999 } ],
  "dependencies": []
}
```

`src/Probe.al`:

```al
table 50980 "CR Row"
{
    fields { field(1; K; Integer) { } }
    keys { key(PK; K) { } }
}
codeunit 50981 "CR Writer"
{
    trigger OnRun()
    var
        R: Record "CR Row";
    begin
        R.K := Random(1000000000);
        R.Insert();
    end;
}
codeunit 50982 "CR Noop"
{
    trigger OnRun()
    begin
    end;
}
codeunit 50983 "CR Ignore"
{
    [CommitBehavior(CommitBehavior::Ignore)]
    procedure Go(): Text
    begin
        exit(Probe(true));
    end;

    procedure Probe(Checked: Boolean): Text
    var
        Ok: Boolean;
    begin
        if Checked then
            Ok := Codeunit.Run(Codeunit::"CR Writer")
        else
            Codeunit.Run(Codeunit::"CR Writer");
        ClearLastError();
        if TryNoop() then
            exit(StrSubstNo('first=%1 second-checked-run=ok', Ok));
        exit(StrSubstNo('first=%1 second-checked-run=ERR %2', Ok, GetLastErrorText()));
    end;

    [TryFunction]
    local procedure TryNoop()
    begin
        if Codeunit.Run(Codeunit::"CR Noop") then;
    end;
}
codeunit 50984 "CR Error"
{
    [CommitBehavior(CommitBehavior::Error)]
    procedure Go(): Text
    var
        P: Codeunit "CR Ignore";
    begin
        exit(P.Probe(true));
    end;
}
codeunit 50985 "CR Probe Test"
{
    Subtype = Test;

    [Test]
    procedure Measure()
    var
        P: Codeunit "CR Ignore";
        E: Codeunit "CR Error";
        W: Codeunit "CR Writer";
        R: Record "CR Row";
        Results: Text;
        Ok: Boolean;
    begin
        Commit();
        Results += ' || [A unchecked] ' + P.Probe(false);
        Commit();
        Results += ' || [B checked] ' + P.Probe(true);
        Commit();
        Results += ' || [C checked under Ignore] ' + P.Go();
        Commit();
        Results += ' || [D checked under Error] ' + E.Go();
        Commit();
        Ok := W.Run();
        ClearLastError();
        if TryOnly() then
            Results += StrSubstNo(' || [E var W.Run()] first=%1 second=ok', Ok)
        else
            Results += StrSubstNo(' || [E var W.Run()] first=%1 second=ERR %2', Ok, GetLastErrorText());
        Commit();
        R.K := -1;
        if R.Insert() then;
        ClearLastError();
        if TryOnly() then
            Results += ' || [F control: own pending insert] second=ok'
        else
            Results += ' || [F control: own pending insert] second=ERR ' + GetLastErrorText();
        Error('MEASURED%1', Results);
    end;

    [TryFunction]
    local procedure TryOnly()
    begin
        if Codeunit.Run(Codeunit::"CR Noop") then;
    end;
}
```

## Second probe (ENG-22): does `IsInWriteTransaction()` match the run's failure?

Measured 2026-11-12 on the BC 28.4 Linux sidecar. **Windows confirmation pending** (owner
q-20261112T043721-7da83bae). App
`e22e22e0-0000-4000-8000-0000000000e9` ("ENG22 IsInWriteTransaction Probe", ids 50960-50979,
removed after the run). Each state starts after a `Commit()` (nothing pending); the probe records
`IsInWriteTransaction()` and whether a checked run of an empty codeunit (inside a `[TryFunction]`)
succeeds.

| State | `IsInWriteTransaction()` | Checked run |
|---|---|---|
| A: nothing | No | ok |
| B: an `Insert` pending | Yes | fails ("An error occurred and the transaction is stopped.") |
| C: `Insert`, then `Commit()` | No | ok |
| D: a plain `Get` | No | ok |
| E: `LockTable`, then `FindSet` | Yes | fails |
| F: `ModifyAll` that matched no row | Yes | fails |
| G: `Insert`, then `if IsInWriteTransaction() then Commit();` | No | ok |

So in every state tried, the method was true exactly when the run failed: `if
IsInWriteTransaction() then Commit();` leaves nothing pending, and a locking read (E) or a write
statement that changed no row (F) opens the transaction as a write does. A first attempt with an
`Insert` inside a failing `[TryFunction]` was refused by this server ("Call to the function
'INSERT' is not allowed inside the call to 'RootMethodScope' when it is used as a TryFunction",
the `DisableWriteInsideTryFunctions` behaviour of engine limits L23) and was left out.

`src/Probe.al` (app.json as above, version 1.0.0.1):

```al
table 50960 "IW Row"
{
    fields { field(1; K; Integer) { } field(2; V; Integer) { } }
    keys { key(PK; K) { } }
}
codeunit 50961 "IW Noop"
{
    trigger OnRun()
    begin
    end;
}
codeunit 50962 "IW Probe Test"
{
    Subtype = Test;

    [Test]
    procedure Measure()
    var
        R: Record "IW Row";
        R2: Record "IW Row";
        Results: Text;
    begin
        R.DeleteAll();
        R.K := 1;
        R.Insert();
        Commit();

        Results += ' || [A nothing] ' + State();
        Commit();

        R.K := 2;
        R.Insert();
        Results += ' || [B pending insert] ' + State();
        Commit();

        R.K := 3;
        R.Insert();
        Commit();
        Results += ' || [C insert then Commit] ' + State();
        Commit();

        R2.Get(1);
        Results += ' || [D Get (read)] ' + State();
        Commit();

        R2.LockTable();
        R2.FindSet();
        Results += ' || [E LockTable + FindSet] ' + State();
        Commit();

        R2.Reset();
        R2.SetRange(K, 9999);
        R2.ModifyAll(V, 1);
        Results += ' || [F ModifyAll on no rows] ' + State();
        Commit();

        R.K := 4;
        R.Insert();
        if IsInWriteTransaction() then
            Commit();
        Results += ' || [G insert, guarded Commit] ' + State();
        Commit();

        Error('MEASURED%1', Results);
    end;

    local procedure State(): Text
    var
        Iwt: Boolean;
    begin
        Iwt := IsInWriteTransaction();
        ClearLastError();
        if TryRun() then
            exit(StrSubstNo('iwt=%1 run=ok', Iwt));
        exit(StrSubstNo('iwt=%1 run=ERR %2', Iwt, GetLastErrorText()));
    end;

    [TryFunction]
    local procedure TryRun()
    begin
        if Codeunit.Run(Codeunit::"IW Noop") then;
    end;
}
```

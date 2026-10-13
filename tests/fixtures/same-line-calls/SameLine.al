// #28: two calls on ONE line, in one caller, with the same callee text and
// different targets. `Codeunit.Run` is the callee text of both, so the old
// site key (unit, line, callee_fp) gave both calls one key, and the golden
// stored only the union {SLCFirst.OnRun, SLCSecond.OnRun}. A resolver that
// swapped the two targets matched that union. The whole-span key keeps them
// apart.
//
// Frozen in `tests/goldens/semantic-edges/same-line-calls-fixture.json`
// (EdgeKind::Run, minted from the fresh resolver):
//   • Codeunit.Run(Codeunit::SLCFirst)  -> {Codeunit SLCFirst.OnRun}
//   • Codeunit.Run(Codeunit::SLCSecond) -> {Codeunit SLCSecond.OnRun}
codeunit 50600 SLCFirst
{
    trigger OnRun()
    begin
    end;
}

codeunit 50601 SLCSecond
{
    trigger OnRun()
    begin
    end;
}

codeunit 50602 SLCCaller
{
    procedure RunBoth()
    begin
        Codeunit.Run(Codeunit::SLCFirst); Codeunit.Run(Codeunit::SLCSecond);
    end;
}

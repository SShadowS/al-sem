//! ENG-177: d11 (Modify without Get) and d21 (read without load) report an operation whose
//! record no path reaching it leaves loaded: a later copy of an unloaded record, or `exit`, cuts
//! the load. A load on SOME path still silences them (their "never loaded" rule), so the if/else,
//! `case`, conditional-expression and loop shapes that load on one branch stay silent.

use std::collections::BTreeSet;
use std::path::Path;
use std::process::Command;

const SOURCE: &str = r#"table 50500 "E Entry"
{
    fields { field(1; K; Integer) { } field(2; V; Integer) { } }
    keys { key(PK; K) { } }

    procedure LoadMe()
    begin
        Rec.Get(1);
    end;
}

codeunit 50501 "E Mgt"
{
    // Silent: one branch loads (both detectors are "never loaded on any path" rules).
    procedure IfElseCopy(B: Boolean)
    var L, N, A: Record "E Entry";
    begin
        L.Get(1);
        if B then A := L else A := N;
        A.Modify();
    end;

    procedure CaseCopy(I: Integer)
    var L, N, A: Record "E Entry";
    begin
        L.Get(1);
        case I of
            1: A := L;
            2: A := N;
        end;
        A.Modify();
    end;

    procedure TernaryCopy(B: Boolean)
    var L, N, A: Record "E Entry";
    begin
        L.Get(1);
        A := B ? L : N;
        A.Modify();
    end;

    procedure LoopCopy()
    var L, N, A: Record "E Entry";
    begin
        L.Get(1);
        A := L;
        repeat
            A.Modify();
            A := N;
        until N.Next() = 0;
    end;

    procedure IfLoad(B: Boolean)
    var A: Record "E Entry";
    begin
        if B then A.Get(1);
        A.Modify();
    end;

    procedure BreakOut()
    var A, N: Record "E Entry";
        I: Integer;
    begin
        A.Get(1);
        while I < 10 do begin
            if I = 5 then
                break;
            A := N;
            I += 1;
        end;
        A.Modify();
    end;

    procedure ElseLoads(B: Boolean)
    var A: Record "E Entry";
    begin
        if B then
            exit
        else
            A.Get(1);
        A.Modify();
    end;

    // Silent: no whole-record reset, or a value the walk cannot see.
    procedure FieldWrite()
    var A: Record "E Entry";
    begin
        A.Get(1);
        A.V := 5;
        A.Modify();
    end;

    procedure CallResult()
    var A: Record "E Entry";
    begin
        A.Get(1);
        A := Make();
        A.Modify();
    end;

    procedure ParamCopy(P: Record "E Entry")
    var A: Record "E Entry";
    begin
        A.Get(1);
        A := P;
        A.Modify();
    end;

    procedure Chain()
    var L, B, C: Record "E Entry";
    begin
        L.Get(1);
        B := L;
        C := B;
        C.Modify();
    end;

    // Silent (CDO triage): `Clear` leaves a well-defined state, as `Init` (the build-then-insert
    // pattern `Clear(R); R.TransferFields(Src); R.Validate(..); R.Insert()`).
    procedure ClearBuilds()
    var A: Record "E Entry";
    begin
        if A.FindLast() then;
        Clear(A);
        A.Validate(V, 1);
        A.Insert();
    end;

    // Silent (CDO triage): a record a call fills through a var argument has the callee's state.
    procedure FilledByCall()
    var Edited: Record "E Entry";
        A: Record "E Entry";
    begin
        A.FindFirst();
        Fill(Edited);
        A := Edited;
        A.Modify();
    end;

    // Silent (CDO triage): `TransferFields` from a loaded record, as `Copy`, on the branch
    // without the `Insert`.
    procedure TransferredOn(B: Boolean)
    var Src, A: Record "E Entry";
    begin
        Src.Get(1);
        A.TransferFields(Src, false);
        if B then
            A.Insert()
        else
            A.Modify();
    end;

    // Silent (CDO triage): a parens-less load in a condition is an op leaf of the tree.
    procedure ParenlessLoad()
    var L, A: Record "E Entry";
    begin
        if L.FINDSET then
            repeat
                A := L;
                A.MODIFY;
            until L.NEXT = 0;
    end;

    // Silent (Fable r1): a load the tree does not hold (a `case` pattern) leaves the walk unsure.
    procedure CasePattern()
    var A: Record "E Entry";
    begin
        case true of
            A.Get(1):
                A.Modify();
        end;
    end;

    // Silent (Fable r1): `asserterror` catches the error, the path goes on.
    procedure AssertCaught(B: Boolean)
    var A: Record "E Entry";
    begin
        if B then
            asserterror begin
                A.Get(1);
                Error('x');
            end;
        A.Modify();
    end;

    // Silent (Fable r1): a table procedure called ON a record may load it.
    procedure ReceiverLoads()
    var A, L2: Record "E Entry";
    begin
        A.Get(1);
        L2.LoadMe();
        A := L2;
        A.Modify();
    end;

    // Silent (Fable r1): a load later in the loop reaches the next pass.
    procedure LoopLaterLoad()
    var L, L2, A: Record "E Entry";
        I: Integer;
    begin
        L.Get(1);
        A := L;
        for I := 1 to 3 do begin
            A := L2;
            A.Modify();
            L2.FindFirst();
        end;
    end;

    // Silent: a `case` with no `else` may match no branch, keeping the load.
    procedure NoMatchKeeps(I: Integer)
    var N, A: Record "E Entry";
    begin
        A.Get(1);
        case I of
            1: A := N;
        end;
        A.Modify();
    end;

    // Silent: the path that breaks out carries the load.
    procedure BreakCarries()
    var N, A: Record "E Entry";
        I: Integer;
    begin
        while I < 3 do begin
            if I = 1 then begin
                A.Get(1);
                break;
            end;
            A := N;
            I += 1;
        end;
        A.Modify();
    end;

    // Silent: the path that continues carries the load to the loop's exit.
    procedure ContinueCarries()
    var N, A: Record "E Entry";
        I: Integer;
    begin
        while I < 3 do begin
            I += 1;
            if I = 1 then begin
                A.Get(1);
                continue;
            end;
            A := N;
        end;
        A.Modify();
    end;

    // Reported: the load reaches the operation on no path.
    procedure ErrorKills(B: Boolean)
    var A: Record "E Entry";
    begin
        if B then begin
            A.Get(1);
            Error('x');
        end;
        A.Modify();
    end;

    // Reported (Fable r2): a platform method called on the record loads nothing.
    procedure PlatformMethodKeepsKill()
    var N, A: Record "E Entry";
    begin
        A.Get(1);
        A := N;
        A.SetRecFilter();
        A.Modify();
    end;

    // Reported (Fable r2): a terminating `Error` handed the source record is reached, not unseen.
    procedure ErrorArgKeepsKill(B: Boolean)
    var N, A: Record "E Entry";
    begin
        if B then
            Error('x %1', N);
        A.Get(1);
        A := N;
        A.Modify();
    end;

    // Reported (Fable r2): a load no path reaches (after `exit`) is no unseen load.
    procedure DeadLoadKeepsKill()
    var N, A: Record "E Entry";
    begin
        A.Get(1);
        A := N;
        A.Modify();
        exit;
        N.Get(2);
    end;

    // Reported: a resolved by-value argument leaves the caller's record as it was.
    procedure ByValueKeepsKill()
    var N, A: Record "E Entry";
    begin
        A.Get(1);
        A := N;
        Log(A);
        A.Modify();
    end;

    // Silent: A is loaded from the third pass on; only the loop's fixed point (three rounds) sees it.
    procedure LoopThirdPass()
    var L, N, A: Record "E Entry";
        I: Integer;
    begin
        A.Get(1);
        A := N;
        while I < 3 do begin
            A.Modify();
            A := L;
            L.FindFirst();
            I += 1;
        end;
    end;

    procedure ParenlessKill()
    var N, A: Record "E Entry";
    begin
        A.FINDFIRST;
        A := N;
        A.MODIFY;
    end;

    procedure CopyKills()
    var L, N, A: Record "E Entry";
    begin
        L.Get(1);
        A := L;
        A := N;
        A.Modify();
    end;

    procedure CopyKillsLoad()
    var N, A: Record "E Entry";
    begin
        A.Get(1);
        A := N;
        A.Modify();
    end;

    procedure ChainKilled()
    var L, N, B, C: Record "E Entry";
    begin
        L.Get(1);
        B := L;
        B := N;
        C := B;
        C.Modify();
    end;

    procedure ExitKills(B: Boolean)
    var A: Record "E Entry";
    begin
        if B then begin
            A.Get(1);
            exit;
        end;
        A.Modify();
    end;

    procedure ReadCopyKills()
    var N, A: Record "E Entry";
    begin
        A.Get(1);
        A := N;
        A.TestField(V);
    end;

    procedure ReadCopyKept(B: Boolean)
    var L, N, A: Record "E Entry";
    begin
        L.Get(1);
        if B then A := L else A := N;
        A.TestField(V);
    end;

    // Reported (sol r1): a tested load loads only on the branch where it found a record; the only
    // path to the Modify is the failed Get's.
    procedure TestedGetExit()
    var A: Record "E Entry";
    begin
        if A.Get(1) then
            exit;
        A.Modify();
    end;

    // The same, parens-less.
    procedure TestedFindExit()
    var A: Record "E Entry";
    begin
        if A.FindFirst then
            exit;
        A.Modify();
    end;

    // Silent: the path that goes on is the one where Get found a record.
    procedure NotTestedGetExit()
    var A: Record "E Entry";
    begin
        if not A.Get(1) then
            exit;
        A.Modify();
    end;

    // Reported (sol r2): reaching the Modify means the whole `or` was false, so Get found nothing.
    procedure OrTestedExit(B: Boolean)
    var A: Record "E Entry";
    begin
        if (A.Get(1)) or B then
            exit;
        A.Modify();
    end;

    // Silent: a false `and` says nothing about which side was false; Get may have found one.
    procedure AndTestedExit(B: Boolean)
    var A: Record "E Entry";
    begin
        if A.Get(1) and B then
            exit;
        A.Modify();
    end;

    // Silent: past a false `not Get or B`, `not Get` was false, so Get found a record.
    procedure NotInOrExit(B: Boolean)
    var A: Record "E Entry";
    begin
        if not A.Get(1) or B then
            exit;
        A.Modify();
    end;

    // Reported (sol r1): a ternary's alternatives are exclusive; the Modify's has no load.
    procedure TernaryExclusive(B: Boolean)
    var
        A: Record "E Entry";
        Ok: Boolean;
    begin
        Ok := B ? A.Get(1) : A.Modify();
    end;

    // Silent: each alternative starts from the state before the ternary, where Get loaded A.
    procedure TernaryKeepsEntryLoad(B: Boolean)
    var
        A: Record "E Entry";
        Ok: Boolean;
    begin
        A.Get(1);
        Ok := B ? A.Modify() : false;
    end;

    // Silent: after the ternary, the Get's alternative may have run.
    procedure TernaryLoadThenModify(B: Boolean)
    var
        A: Record "E Entry";
        Ok: Boolean;
    begin
        Ok := B ? A.Get(1) : false;
        A.Modify();
    end;

    local procedure Log(R: Record "E Entry")
    begin
        Message('%1', R.K);
    end;

    local procedure Fill(var R: Record "E Entry")
    begin
        R.K := 7;
    end;

    local procedure Make(): Record "E Entry"
    var R: Record "E Entry";
    begin
        exit(R);
    end;
}
"#;

fn reported(dir: &Path, detector: &str) -> BTreeSet<String> {
    let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .args(["analyze", "ws", "--single-app", "--format", "json"])
        .current_dir(dir)
        .env("RUST_LOG", "off")
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("{e}; stderr: {}", String::from_utf8_lossy(&out.stderr)));
    v["payload"]["findings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| f["detector"] == detector)
        .map(|f| {
            f["primaryLocation"]["routineName"]
                .as_str()
                .unwrap()
                .to_string()
        })
        .collect()
}

/// Discrimination (2026-11-23): the detectors ignoring the walk (`record_may_be_loaded_at`
/// answering `true`) fail it (no `CopyKills`, `ExitKills`, ...); the walk's assignment not
/// copying its value's state (a copy always loads) fails `CopyKills`; `exit` not ending its path
/// fails `ExitKills`; a branch join taking AND instead of OR fails `IfElseCopy`; the statement
/// tree leaving out a parens-less record operation in a condition (`ir_walk::harvest`) fails
/// `ParenlessLoad`; a record handed to a call keeping its state fails `FilledByCall`;
/// `TransferFields` not loading fails `TransferredOn`. Round 2 (Fable r1): the unseen-load guard
/// off fails `CasePattern`; `asserterror` read as a plain block fails `AssertCaught`; a call ON
/// the record not loading it fails `ReceiverLoads`; a resolved by-value argument counted as a
/// possible load fails `ByValueKeepsKill`; a terminating `Error` not ending its path fails
/// `ErrorKills`; a `case` without `else` not keeping the no-match path fails `NoMatchKeeps`; a
/// `break` not recorded on its loop fails `BreakCarries`, a `continue` `ContinueCarries`; one loop
/// round instead of the fixed point fails `LoopThirdPass` and `LoopLaterLoad`. Round 3 (Fable
/// r2): any method called on the record loading it fails `PlatformMethodKeepsKill`; a terminating
/// `Error` call site left unseen fails `ErrorArgKeepsKill`; unreachable code not marked seen fails
/// `DeadLoadKeepsKill`. Each restored, passes. `ClearBuilds` guards the design (a `Clear` that unloaded, as the first draft did,
/// reported it), not a line. sol r1 and r2 (red proofs run 2026-11-27, each restored and green):
/// `may_load` reading the other branch's `missed_on` fails `TestedGetExit`, `TestedFindExit`,
/// `OrTestedExit` and `NotTestedGetExit`; `ir_walk::tested_ops`'s `and` passing a false value to its
/// operands fails `AndTestedExit`, its `or` passing none fails `OrTestedExit`, and `not` not
/// flipping fails `NotInOrExit` and `NotTestedGetExit`; the ternary run walked in sequence fails
/// `TernaryExclusive`; each side walked from an empty state instead of the run's entry fails
/// `TernaryKeepsEntryLoad` (`TernaryLoadThenModify` alone stayed green under that break: its load
/// is inside the ternary).
#[test]
fn a_load_must_reach_the_operation_on_some_path() {
    let t = tempfile::tempdir().unwrap();
    let ws = t.path().join("ws");
    std::fs::create_dir_all(ws.join("src")).unwrap();
    std::fs::write(
        ws.join("app.json"),
        r#"{"id":"e177e177-0000-4000-8000-000000000177","name":"E177","publisher":"t","version":"1.0.0.0"}"#,
    )
    .unwrap();
    std::fs::write(ws.join("src/a.al"), SOURCE).unwrap();
    let set = |names: &[&str]| names.iter().map(|s| s.to_string()).collect::<BTreeSet<_>>();
    assert_eq!(
        reported(t.path(), "d11-modify-without-get"),
        set(&[
            "ByValueKeepsKill",
            "ChainKilled",
            "CopyKills",
            "CopyKillsLoad",
            "DeadLoadKeepsKill",
            "ErrorArgKeepsKill",
            "ErrorKills",
            "ExitKills",
            "OrTestedExit",
            "ParenlessKill",
            "PlatformMethodKeepsKill",
            "TernaryExclusive",
            "TestedFindExit",
            "TestedGetExit",
        ])
    );
    assert_eq!(
        reported(t.path(), "d21-read-without-load"),
        set(&["ReadCopyKills"])
    );
}

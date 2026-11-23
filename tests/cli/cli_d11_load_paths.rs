//! ENG-177: d11 (Modify without Get) and d21 (read without load) report an operation whose
//! record no path reaching it leaves loaded: a later unloaded copy, `Clear` or `exit` cuts the
//! load. A load on SOME path still silences them (their "never loaded" rule), so the if/else,
//! `case`, conditional-expression and loop shapes that load on one branch stay silent.

use std::collections::BTreeSet;
use std::path::Path;
use std::process::Command;

const SOURCE: &str = r#"table 50500 "E Entry"
{
    fields { field(1; K; Integer) { } field(2; V; Integer) { } }
    keys { key(PK; K) { } }
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

    // Reported: the load reaches the operation on no path.
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

    procedure ClearKills()
    var A: Record "E Entry";
    begin
        A.Get(1);
        Clear(A);
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
/// answering `true`) fail it (no `CopyKills`, `ClearKills`, `ExitKills`, ...); the walk's
/// assignment not copying its value's state (a copy always loads) fails `CopyKills`; `exit` not
/// ending its path fails `ExitKills`; `Clear` not unloading fails `ClearKills`; a branch join
/// taking AND instead of OR fails `IfElseCopy`. Each restored, passes.
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
            "ChainKilled",
            "ClearKills",
            "CopyKills",
            "CopyKillsLoad",
            "ExitKills",
        ])
    );
    assert_eq!(
        reported(t.path(), "d21-read-without-load"),
        set(&["ReadCopyKills"])
    );
}

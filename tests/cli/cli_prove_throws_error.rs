//! G20: `alsem prove … throws-error` sees implicit runtime errors.
//!
//! A record lookup whose Boolean is discarded raises an error when no record
//! matches, and `TestField` / `FieldError` raise one when they fail: Continia
//! DO's `CDO E-Mail.Init` does an unguarded `DOSetup.Get();`, and `throws-error`
//! answered `no` with no evidence. They are now `error-implicit` facts
//! (`ERROR_IMPLICIT` effects), counted by `throws-error` only: the ordering
//! engine and the detectors keep reading `error-throw` as an explicit `Error`.
//!
//! Each routine of one temp workspace is asked through the real binary. The
//! guarded forms and an `HttpClient.Get` statement (no record) are controls.

use std::path::Path;
use std::process::Command;

const SOURCE: &str = r#"table 50180 "G20 T"
{
    fields { field(1; Code; Code[20]) { } field(2; Name; Text[50]) { } }
    keys { key(PK; Code) { } }
}
codeunit 50181 "G20 Errors"
{
    procedure UnguardedGet()
    var
        T: Record "G20 T";
    begin
        T.Get('A');
    end;

    procedure UnguardedFindSet()
    var
        T: Record "G20 T";
    begin
        T.FindSet();
    end;

    procedure TestFieldCall()
    var
        T: Record "G20 T";
    begin
        T.TestField(Name);
    end;

    procedure FieldErrorCall()
    var
        T: Record "G20 T";
    begin
        T.FieldError(Name);
    end;

    procedure GuardedGet(): Boolean
    var
        T: Record "G20 T";
    begin
        if T.Get('A') then
            exit(true);
        exit(false);
    end;

    procedure GuardedFindSet()
    var
        T: Record "G20 T";
    begin
        if T.FindSet() then
            repeat
            until T.Next() = 0;
    end;

    procedure HttpGet()
    var
        Client: HttpClient;
        Response: HttpResponseMessage;
    begin
        Client.Get('https://example.com', Response);
    end;

    // G50: a discarded Boolean result: a failed parse raises an error.
    procedure EvaluateStatement(VersionText: Text): Integer
    var
        Major: Integer;
    begin
        Evaluate(Major, VersionText);
        exit(Major);
    end;

    procedure EvaluateChecked(VersionText: Text): Integer
    var
        Major: Integer;
    begin
        if Evaluate(Major, VersionText) then
            exit(Major);
    end;

    procedure ReadJsonStatement(JsonText: Text)
    var
        Json: JsonObject;
    begin
        Json.ReadFrom(JsonText);
    end;

    // G52: an Evaluate that provably cannot fail raises nothing. Continia DO's
    // `CDO Conflict Notific. Handler` reads a notification id this way.
    procedure EvaluateLockedGuidLabel(): Guid
    var
        Id: Guid;
        IdTok: Label '{a7c3e1b4-9d2f-4a8e-b5c6-d7e8f9a0b1c2}', Locked = true;
    begin
        Evaluate(Id, IdTok);
        exit(Id);
    end;

    procedure EvaluateGuidLiteral(): Guid
    var
        Id: Guid;
    begin
        Evaluate(Id, '{A7C3E1B4-9D2F-4A8E-B5C6-D7E8F9A0B1C2}');
        exit(Id);
    end;

    procedure EvaluateIntegerLiteral(): Integer
    var
        N: Integer;
    begin
        Evaluate(N, '-42');
        exit(N);
    end;

    procedure EvaluateGlobalLockedInteger(): Integer
    var
        N: Integer;
    begin
        Evaluate(N, SevenTok);
        exit(N);
    end;

    // G52 controls: each may fail, or nothing proves it cannot.
    procedure EvaluateUnlockedGuidLabel(): Guid
    var
        Id: Guid;
        IdTxt: Label '{a7c3e1b4-9d2f-4a8e-b5c6-d7e8f9a0b1c2}';
    begin
        Evaluate(Id, IdTxt);
        exit(Id);
    end;

    procedure EvaluateLockedTextIntoGuid(): Guid
    var
        Id: Guid;
        IdTok: Label 'not-a-guid', Locked = true;
    begin
        Evaluate(Id, IdTok);
        exit(Id);
    end;

    procedure EvaluateUnbracedGuid(): Guid
    var
        Id: Guid;
    begin
        Evaluate(Id, 'a7c3e1b4-9d2f-4a8e-b5c6-d7e8f9a0b1c2');
        exit(Id);
    end;

    procedure EvaluateIntegerOverflow(): Integer
    var
        N: Integer;
    begin
        Evaluate(N, '99999999999');
        exit(N);
    end;

    procedure EvaluateDateLiteral(): Date
    var
        D: Date;
    begin
        Evaluate(D, '2020-01-01');
        exit(D);
    end;

    procedure EvaluatePreprocLabel(): Guid
    var
        Id: Guid;
#if CLEAN99
        IdTok: Label '{a7c3e1b4-9d2f-4a8e-b5c6-d7e8f9a0b1c2}', Locked = true;
#else
        IdTok: Label 'not-a-guid', Locked = true;
#endif
    begin
        Evaluate(Id, IdTok);
        exit(Id);
    end;

    var
        SevenTok: Label '7', Locked = true;
}
"#;

fn alsem(cwd: &Path, args: &[&str]) -> serde_json::Value {
    let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .args(args)
        .current_dir(cwd)
        .env("RUST_LOG", "off")
        .output()
        .unwrap();
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "{args:?}: {e}; stderr: {}",
            String::from_utf8_lossy(&out.stderr)
        )
    })
}

#[test]
fn throws_error_sees_implicit_runtime_errors() {
    let t = tempfile::tempdir().unwrap();
    let ws = t.path().join("ws");
    std::fs::create_dir_all(ws.join("src")).unwrap();
    std::fs::write(
        ws.join("app.json"),
        r#"{"id":"20202020-0000-4000-8000-000000000020","name":"G20","publisher":"t","version":"1.0.0.0"}"#,
    )
    .unwrap();
    std::fs::write(ws.join("src/Errors.al"), SOURCE).unwrap();

    let inv = alsem(
        t.path(),
        &["fingerprint", "ws", "--inventory-only", "--format", "json"],
    );
    let id_of = |name: &str| -> String {
        inv["payload"]["routineInventory"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["routineName"] == name)
            .unwrap_or_else(|| panic!("precondition: routine {name} is in the inventory"))
            ["stableRoutineId"]
            .as_str()
            .unwrap()
            .to_string()
    };
    let prove = |name: &str| -> serde_json::Value {
        alsem(
            t.path(),
            &[
                "prove",
                "ws",
                &id_of(name),
                "throws-error",
                "--deterministic",
            ],
        )["payload"]["result"]
            .clone()
    };

    for (routine, excerpt) in [
        ("UnguardedGet", "T.Get"),
        ("UnguardedFindSet", "T.FindSet"),
        ("TestFieldCall", "T.TestField"),
        ("FieldErrorCall", "T.FieldError"),
        ("EvaluateStatement", "Evaluate"),
        ("ReadJsonStatement", "Json.ReadFrom"),
        ("EvaluateUnlockedGuidLabel", "Evaluate"),
        ("EvaluateLockedTextIntoGuid", "Evaluate"),
        ("EvaluateUnbracedGuid", "Evaluate"),
        ("EvaluateIntegerOverflow", "Evaluate"),
        ("EvaluateDateLiteral", "Evaluate"),
        ("EvaluatePreprocLabel", "Evaluate"),
    ] {
        let r = prove(routine);
        assert_eq!(r["answer"], "yes", "{routine}: {r}");
        let evidence: Vec<(&str, &str)> = r["evidence"]
            .as_array()
            .unwrap_or_else(|| panic!("{routine}: no evidence: {r}"))
            .iter()
            .map(|e| {
                (
                    e["type"].as_str().unwrap_or(""),
                    e["evidence"]["excerpt"].as_str().unwrap_or(""),
                )
            })
            .collect();
        assert!(
            evidence
                .iter()
                .any(|(t, x)| *t == "ERROR_IMPLICIT" && x.starts_with(excerpt)),
            "{routine}: the evidence must be an ERROR_IMPLICIT at {excerpt:?}: {evidence:?}"
        );
    }
    for routine in [
        "GuardedGet",
        "GuardedFindSet",
        "HttpGet",
        "EvaluateChecked",
        "EvaluateLockedGuidLabel",
        "EvaluateGuidLiteral",
        "EvaluateIntegerLiteral",
        "EvaluateGlobalLockedInteger",
    ] {
        let r = prove(routine);
        assert_eq!(r["answer"], "no", "{routine} raises no implicit error: {r}");
    }
}

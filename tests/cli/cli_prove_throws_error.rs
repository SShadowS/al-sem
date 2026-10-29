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
    for routine in ["GuardedGet", "GuardedFindSet", "HttpGet"] {
        let r = prove(routine);
        assert_eq!(r["answer"], "no", "{routine} raises no implicit error: {r}");
    }
}

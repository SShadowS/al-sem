//! G23: `prove … writes-table` on a write behind `if P then` that every call
//! path binds `P` to `false`.
//!
//! Continia DO's `DocSave` calls `RunReport(false, …)`, which passes `UseQueue`
//! on to `RunReportID`, where `QueueMail` (the `CDO Queue Entry` insert) sits
//! under `if UseQueue then`. Outside a recursive SCC the cone's
//! `param_guard` already drops it (`DocSave` here); inside one it did not,
//! because a recursive SCC shares one cone (`DocSaveRecursive`, where
//! `RunReportRec` and `RunReportIDRec` call each other, as DO's 489-routine
//! SCC does): `literal_reach` follows the literals per root.
//!
//! The controls keep every case that can still run a `yes`: a literal `true`
//! (also in the recursive SCC), the root's own parameter, a reassigned
//! parameter, one passed to a `var`.

use std::path::Path;
use std::process::Command;

const SOURCE: &str = r#"table 50230 "G23 Queue"
{
    fields { field(1; Code; Code[20]) { } }
    keys { key(PK; Code) { } }
}
codeunit 50231 "G23 Handler"
{
    procedure DocSave()
    begin
        RunReport(false);
    end;

    procedure DocSaveRecursive()
    begin
        RunReportRec(false, 0);
    end;

    procedure DocQueueRecursive()
    begin
        RunReportRec(true, 0);
    end;

    local procedure RunReportRec(UseQueue: Boolean; Depth: Integer)
    begin
        RunReportIDRec(UseQueue, Depth);
    end;

    local procedure RunReportIDRec(UseQueue: Boolean; Depth: Integer)
    begin
        if Depth < 3 then
            RunReportRec(UseQueue, Depth + 1);
        if UseQueue then
            QueueMail();
    end;

    procedure DocQueue()
    begin
        RunReport(true);
    end;

    procedure DocEither(UseQueue: Boolean)
    begin
        RunReport(UseQueue);
    end;

    procedure DocReassigned()
    begin
        RunReportReassigning(false);
    end;

    procedure DocVarPassed()
    begin
        RunReportVarPassing(false);
    end;

    local procedure RunReport(UseQueue: Boolean)
    begin
        RunReportID(UseQueue);
    end;

    local procedure RunReportID(UseQueue: Boolean)
    begin
        if UseQueue then
            QueueMail();
    end;

    local procedure RunReportReassigning(UseQueue: Boolean)
    begin
        UseQueue := true;
        if UseQueue then
            QueueMail();
    end;

    local procedure RunReportVarPassing(UseQueue: Boolean)
    begin
        Flip(UseQueue);
        if UseQueue then
            QueueMail();
    end;

    local procedure Flip(var B: Boolean)
    begin
        B := not B;
    end;

    local procedure QueueMail()
    var
        Q: Record "G23 Queue";
    begin
        Q.Code := 'X';
        Q.Insert();
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
fn a_write_behind_a_literal_false_guard_is_not_a_write() {
    let t = tempfile::tempdir().unwrap();
    let ws = t.path().join("ws");
    std::fs::create_dir_all(ws.join("src")).unwrap();
    std::fs::write(
        ws.join("app.json"),
        r#"{"id":"23232323-0000-4000-8000-000000000023","name":"G23","publisher":"t","version":"1.0.0.0"}"#,
    )
    .unwrap();
    std::fs::write(ws.join("src/Shapes.al"), SOURCE).unwrap();
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
                "writes-table:G23 Queue",
                "--deterministic",
            ],
        )["payload"]["result"]
            .clone()
    };

    for routine in ["DocSave", "DocSaveRecursive"] {
        let r = prove(routine);
        assert_eq!(r["answer"], "no", "{routine}: the only path is dead: {r}");
        assert_eq!(r["obligations"]["coverageComplete"], true, "{r}");
    }

    for routine in [
        "DocQueue",
        "DocQueueRecursive",
        "DocEither",
        "DocReassigned",
        "DocVarPassed",
    ] {
        let r = prove(routine);
        assert_eq!(r["answer"], "yes", "{routine}: the path can run: {r}");
    }
}

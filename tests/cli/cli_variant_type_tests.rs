//! A Variant's own type tests (`V.IsRecord()`, `V.IsGuid()`, …) are platform
//! built-ins with no AL code behind them, so they resolve; any other member of
//! a Variant stays dynamic. They were open dynamic calls, so `prove` blocked on
//! them (Continia DO: `RecVariant.IsRecord`, `ReferenceVariant.IsGuid`,
//! `FilteringVariant.IsRecordRef` across 7 claims).

use std::path::Path;
use std::process::Command;

const SOURCE: &str = r#"codeunit 50400 "VA Cases"
{
    procedure Check(V: Variant): Boolean
    begin
        if V.IsRecord() then
            exit(true);
        exit(V.IsGuid());
    end;

    procedure Other(V: Variant)
    begin
        V.Whatever();
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
fn a_variants_type_tests_resolve_and_other_members_stay_dynamic() {
    let t = tempfile::tempdir().unwrap();
    let ws = t.path().join("ws");
    std::fs::create_dir_all(ws.join("src")).unwrap();
    std::fs::write(
        ws.join("app.json"),
        r#"{"id":"40404040-0000-4000-8000-000000000040","name":"VA","publisher":"t","version":"1.0.0.0"}"#,
    )
    .unwrap();
    std::fs::write(ws.join("src/a.al"), SOURCE).unwrap();
    let inv = alsem(
        t.path(),
        &["fingerprint", "ws", "--inventory-only", "--format", "json"],
    );
    let prove = |name: &str| -> serde_json::Value {
        let id = inv["payload"]["routineInventory"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["routineName"] == name)
            .unwrap_or_else(|| panic!("precondition: {name} is in the inventory"))
            ["stableRoutineId"]
            .as_str()
            .unwrap()
            .to_string();
        alsem(
            t.path(),
            &["prove", "ws", &id, "may-commit", "--deterministic"],
        )["payload"]["result"]
            .clone()
    };
    let check = prove("Check");
    assert_eq!(check["answer"], "no", "{check}");
    let other = prove("Other");
    assert_eq!(other["answer"], "unknown", "still dynamic: {other}");
}

//! G32: an event publication is the publisher routine's own fact, with no
//! operation to cite, so `publishes-event` evidence read `sourceKind:
//! unavailable`. Its anchor is now the call into the publisher (the last hop
//! of its shortest witness path), from the routine that raises it and from a
//! caller of that routine alike.

use std::path::Path;
use std::process::Command;

const SOURCE: &str = r#"codeunit 50380 "EV Cases"
{
    procedure Publishes()
    begin
        OnSomething();
    end;

    procedure Indirect()
    begin
        Publishes();
    end;

    [IntegrationEvent(false, false)]
    local procedure OnSomething()
    begin
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
fn publishes_event_evidence_cites_the_publish_call() {
    let t = tempfile::tempdir().unwrap();
    let ws = t.path().join("ws");
    std::fs::create_dir_all(ws.join("src")).unwrap();
    std::fs::write(
        ws.join("app.json"),
        r#"{"id":"38383838-0000-4000-8000-000000000038","name":"EV","publisher":"t","version":"1.0.0.0"}"#,
    )
    .unwrap();
    std::fs::write(ws.join("src/a.al"), SOURCE).unwrap();
    let inv = alsem(
        t.path(),
        &["fingerprint", "ws", "--inventory-only", "--format", "json"],
    );
    for routine in ["Publishes", "Indirect"] {
        let id = inv["payload"]["routineInventory"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["routineName"] == routine)
            .unwrap_or_else(|| panic!("precondition: {routine} is in the inventory"))
            ["stableRoutineId"]
            .as_str()
            .unwrap()
            .to_string();
        let r = alsem(
            t.path(),
            &[
                "prove",
                "ws",
                &id,
                "publishes-event:OnSomething",
                "--deterministic",
            ],
        )["payload"]["result"]
            .clone();
        assert_eq!(r["answer"], "yes", "{routine}: {r}");
        let ev = &r["evidence"][0]["evidence"];
        assert_eq!(ev["sourceKind"], "source", "{routine}: {r}");
        assert_eq!(ev["file"], "ws:src/a.al", "{routine}: {r}");
        assert_eq!(
            ev["line"], 4,
            "{routine}: the OnSomething() call (0-based): {r}"
        );
    }
}

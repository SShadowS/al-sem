//! Q6: `prove` follows the LOADED subscribers of every event a cone raises, so
//! an event is closed over them, unlike an interface. A `no` therefore holds
//! only if no app outside the program subscribes, whenever the cone raises an
//! event any app may subscribe to. Such a `no` carries
//! `assumptions: ["loaded-subscribers"]`:
//! - a table write, or a `Validate` (the platform table events; a temporary
//!   record raises them too);
//! - an explicit event publication;
//! - (a page opened, the platform page events: not asked here).
//!
//! A `no` whose cone raises nothing, and a `yes` (it stands on its witness),
//! carry no label.

use std::path::Path;
use std::process::Command;

const SOURCE: &str = r#"table 50300 "Q6 T"
{
    fields
    {
        field(1; Code; Code[20]) { }
        field(2; Name; Text[50]) { }
    }
    keys { key(PK; Code) { } }
}
codeunit 50301 "Q6 Cases"
{
    procedure Pure(): Integer
    begin
        exit(1 + 1);
    end;

    procedure Writes()
    var
        T: Record "Q6 T";
    begin
        T.Code := 'X';
        T.Insert();
    end;

    procedure ValidatesTemp()
    var
        T: Record "Q6 T" temporary;
    begin
        T.Validate(Name, 'x');
    end;

    procedure Publishes()
    begin
        OnSomething();
    end;

    procedure WritesAndCommits()
    var
        T: Record "Q6 T";
    begin
        T.Code := 'Y';
        T.Insert();
        Commit();
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
fn a_no_whose_cone_raises_a_subscribable_event_is_labelled() {
    let t = tempfile::tempdir().unwrap();
    let ws = t.path().join("ws");
    std::fs::create_dir_all(ws.join("src")).unwrap();
    std::fs::write(
        ws.join("app.json"),
        r#"{"id":"06060606-0000-4000-8000-000000000006","name":"Q6","publisher":"t","version":"1.0.0.0"}"#,
    )
    .unwrap();
    std::fs::write(ws.join("src/Cases.al"), SOURCE).unwrap();
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
    let labelled = serde_json::json!(["loaded-subscribers"]);

    let pure = prove("Pure");
    assert_eq!(pure["answer"], "no", "precondition: {pure}");
    assert!(pure.get("assumptions").is_none(), "nothing raised: {pure}");

    for routine in ["Writes", "ValidatesTemp", "Publishes"] {
        let r = prove(routine);
        assert_eq!(r["answer"], "no", "{routine}: {r}");
        assert_eq!(r["assumptions"], labelled, "{routine}: {r}");
    }

    let yes = prove("WritesAndCommits");
    assert_eq!(yes["answer"], "yes", "{yes}");
    assert!(yes.get("assumptions").is_none(), "a yes stands: {yes}");
}

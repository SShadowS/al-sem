//! G29: `alsem prove … --caller-publishers-loaded` assumes every app whose
//! publisher is on the call stack is loaded. System Application's `Telemetry
//! Logger` logs only through the loggers those publishers registered, so a
//! call on it then dispatches only to the loaded implementations. On Continia
//! DO this was the sole blocker of 8 claims (`TelemetryLogger.LogMessage`).
//!
//! The assumption must be visible and narrow:
//! - an answer that holds only under it carries
//!   `assumptions: ["caller-publishers-loaded"]` and
//!   `obligations.callerPublishersRows`;
//! - it waives no other interface (that takes `--closed-world`);
//! - it keys on the declaring app, not the name: a `Telemetry Logger` of
//!   another app stays open;
//! - `--closed-world` implies it, and an answer the narrower world already
//!   gives carries the narrower label.
//!
//! The workspace plays System Application (its app id), so the cataloged
//! interface is in source.

use std::path::Path;
use std::process::Command;

const SYSTEM_APPLICATION: &str = "63ca2fa4-4f03-4f2b-a480-172fef340d3f";

const SOURCE: &str = r#"table 50290 "G29 T"
{
    fields { field(1; Code; Code[20]) { } }
    keys { key(PK; Code) { } }
}
interface "Telemetry Logger"
{
    procedure LogMessage(Msg: Text);
}
codeunit 50291 "G29 System Logger" implements "Telemetry Logger"
{
    procedure LogMessage(Msg: Text)
    begin
    end;
}
interface "G29 Other Logger"
{
    procedure LogMessage(Msg: Text);
}
codeunit 50292 "G29 Other Impl" implements "G29 Other Logger"
{
    procedure LogMessage(Msg: Text)
    begin
    end;
}
codeunit 50293 "G29 Callers"
{
    procedure UseTelemetry(Logger: Interface "Telemetry Logger")
    begin
        Logger.LogMessage('x');
    end;

    procedure UseOther(Logger: Interface "G29 Other Logger")
    begin
        Logger.LogMessage('x');
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

/// A workspace with app id `app_id`, asked `writes-table:G29 T` per routine.
struct Ws {
    dir: tempfile::TempDir,
}

impl Ws {
    fn new(app_id: &str) -> Ws {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().join("ws");
        std::fs::create_dir_all(ws.join("src")).unwrap();
        std::fs::write(
            ws.join("app.json"),
            format!(
                r#"{{"id":"{app_id}","name":"System Application","publisher":"Microsoft","version":"1.0.0.0"}}"#
            ),
        )
        .unwrap();
        std::fs::write(ws.join("src/Shapes.al"), SOURCE).unwrap();
        Ws { dir }
    }

    fn prove(&self, routine: &str, flags: &[&str]) -> serde_json::Value {
        let inv = alsem(
            self.dir.path(),
            &["fingerprint", "ws", "--inventory-only", "--format", "json"],
        );
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
        let mut args = vec!["prove", "ws", &id, "writes-table:G29 T", "--deterministic"];
        args.extend_from_slice(flags);
        alsem(self.dir.path(), &args)["payload"]["result"].clone()
    }
}

fn blocks_on_dispatch(r: &serde_json::Value) -> bool {
    r["blockedBy"].as_array().is_some_and(|a| {
        a.iter()
            .any(|b| b["kind"].as_str() == Some("open-world-dispatch"))
    })
}

const CPL: &str = "--caller-publishers-loaded";

#[test]
fn caller_publishers_loaded_waives_only_publisher_scoped_sites() {
    let ws = Ws::new(SYSTEM_APPLICATION);

    // Precondition: the open world blocks on the Telemetry Logger.
    let open = ws.prove("UseTelemetry", &[]);
    assert_eq!(open["answer"], "unknown", "precondition: {open}");
    assert!(blocks_on_dispatch(&open), "precondition: {open}");
    assert!(open.get("assumptions").is_none(), "{open}");

    // Under the assumption: `no`, labelled, the waived row counted.
    let cpl = ws.prove("UseTelemetry", &[CPL]);
    assert_eq!(cpl["answer"], "no", "{cpl}");
    assert_eq!(
        cpl["assumptions"],
        serde_json::json!(["caller-publishers-loaded"]),
        "{cpl}"
    );
    assert_eq!(cpl["obligations"]["callerPublishersRows"], 1, "{cpl}");
    assert_eq!(cpl["obligations"]["openWorldRows"], 0, "{cpl}");
    assert!(cpl["obligations"].get("closedWorldRows").is_none(), "{cpl}");

    // `--closed-world` implies it; the narrower label wins.
    let closed = ws.prove("UseTelemetry", &["--closed-world"]);
    assert_eq!(closed["answer"], "no", "{closed}");
    assert_eq!(
        closed["assumptions"],
        serde_json::json!(["caller-publishers-loaded"]),
        "{closed}"
    );

    // Any other interface: the assumption does not reach it.
    let other = ws.prove("UseOther", &[CPL]);
    assert_eq!(other["answer"], "unknown", "{other}");
    assert!(blocks_on_dispatch(&other), "{other}");
    assert!(other.get("assumptions").is_none(), "{other}");
    let other_closed = ws.prove("UseOther", &["--closed-world"]);
    assert_eq!(other_closed["answer"], "no", "{other_closed}");
    assert_eq!(
        other_closed["assumptions"],
        serde_json::json!(["closed-world"]),
        "{other_closed}"
    );
}

#[test]
fn a_telemetry_logger_of_another_app_is_not_publisher_scoped() {
    let ws = Ws::new("29292929-0000-4000-8000-000000000029");
    let r = ws.prove("UseTelemetry", &[CPL]);
    assert_eq!(r["answer"], "unknown", "{r}");
    assert!(blocks_on_dispatch(&r), "{r}");
    assert!(r.get("assumptions").is_none(), "{r}");
}

//! G24: `alsem prove … --closed-world` assumes the loaded world is all there
//! is. An interface call then dispatches only to the loaded implementations,
//! which the cone already follows, so its open-world row stops blocking a
//! `no`. On Continia DO most undecided claims were blocked by interface
//! dispatch alone (`TelemetryLogger.LogMessage`, `CurrLineWithPrice.*`).
//!
//! The assumption must be visible and narrow:
//! - an answer that holds only under it carries `assumptions: ["closed-world"]`
//!   and `obligations.closedWorldRows`; one both worlds give carries neither;
//! - a site with an implementer outside the model still blocks: a closed enum
//!   mapping a value to a codeunit that is not loaded, or an interface no
//!   loaded codeunit implements;
//! - without the flag nothing changes.
//!
//! Each routine of one temp workspace is asked through the real binary.

use std::path::Path;
use std::process::Command;

const SOURCE: &str = r#"table 50240 "G24 T"
{
    fields { field(1; Code; Code[20]) { } }
    keys { key(PK; Code) { } }
}
interface "G24 Logger"
{
    procedure Log(Msg: Text);
}
codeunit 50241 "G24 Committing Logger" implements "G24 Logger"
{
    procedure Log(Msg: Text)
    begin
        Commit();
    end;
}
codeunit 50242 "G24 Quiet Logger" implements "G24 Logger"
{
    procedure Log(Msg: Text)
    begin
    end;
}
interface "G24 Sink"
{
    procedure Put();
}
codeunit 50243 "G24 Real Sink" implements "G24 Sink"
{
    procedure Put()
    begin
    end;
}
enum 50244 "G24 Sink Kind" implements "G24 Sink"
{
    Extensible = false;
    value(0; Real) { Implementation = "G24 Sink" = "G24 Real Sink"; }
    value(1; Missing) { Implementation = "G24 Sink" = "G24 Not Loaded Sink"; }
}
interface "G24 Orphan"
{
    procedure Put();
}
codeunit 50245 "G24 Callers"
{
    procedure UseLogger(Logger: Interface "G24 Logger")
    begin
        Logger.Log('x');
    end;

    procedure UseSink(Sink: Interface "G24 Sink")
    begin
        Sink.Put();
    end;

    procedure UseOrphan(Orphan: Interface "G24 Orphan")
    begin
        Orphan.Put();
    end;

    procedure WritesAndLogs(Logger: Interface "G24 Logger")
    var
        T: Record "G24 T";
    begin
        T.Code := 'X';
        T.Insert();
        Logger.Log('x');
    end;

    procedure LogsAndSinks(Logger: Interface "G24 Logger"; Sink: Interface "G24 Sink")
    begin
        Logger.Log('x');
        Sink.Put();
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
fn closed_world_waives_only_fully_loaded_interface_sites() {
    let t = tempfile::tempdir().unwrap();
    let ws = t.path().join("ws");
    std::fs::create_dir_all(ws.join("src")).unwrap();
    std::fs::write(
        ws.join("app.json"),
        r#"{"id":"24242424-0000-4000-8000-000000000024","name":"G24","publisher":"t","version":"1.0.0.0"}"#,
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
    let prove = |name: &str, question: &str, closed: bool| -> serde_json::Value {
        let id = id_of(name);
        let mut args = vec!["prove", "ws", &id, question, "--deterministic"];
        if closed {
            args.push("--closed-world");
        }
        alsem(t.path(), &args)["payload"]["result"].clone()
    };
    let kinds = |r: &serde_json::Value| -> Vec<String> {
        r["blockedBy"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|b| b["kind"].as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    };

    // Every implementer of the Logger is loaded: open world `unknown`, closed
    // world `no`, labelled.
    let open = prove("UseLogger", "writes-table:G24 T", false);
    assert_eq!(
        open["answer"], "unknown",
        "precondition, open world: {open}"
    );
    assert!(
        kinds(&open).contains(&"open-world-dispatch".to_string()),
        "precondition: the open world blocks on the interface: {open}"
    );
    assert!(open.get("assumptions").is_none(), "{open}");
    let closed = prove("UseLogger", "writes-table:G24 T", true);
    assert_eq!(closed["answer"], "no", "closed world: {closed}");
    assert_eq!(closed["assumptions"], serde_json::json!(["closed-world"]));
    assert_eq!(closed["obligations"]["closedWorldRows"], 1, "{closed}");
    assert_eq!(closed["obligations"]["openWorldRows"], 0, "{closed}");
    assert_eq!(closed["obligations"]["coverageComplete"], true, "{closed}");

    // The cone already follows the loaded implementations: a Commit in one of
    // them is a `yes` in both worlds, so it carries no label.
    let commit = prove("UseLogger", "may-commit", true);
    assert_eq!(commit["answer"], "yes", "{commit}");
    assert!(
        commit.get("assumptions").is_none(),
        "a yes both worlds give rests on no assumption: {commit}"
    );
    let writes = prove("WritesAndLogs", "writes-table:G24 T", true);
    assert_eq!(writes["answer"], "yes", "{writes}");
    assert!(writes.get("assumptions").is_none(), "{writes}");

    // A closed enum maps a value to a codeunit that is not loaded, and no
    // loaded codeunit implements the Orphan: even a closed world cannot follow
    // them, so both stay `unknown`, unlabelled.
    for routine in ["UseSink", "UseOrphan"] {
        let r = prove(routine, "writes-table:G24 T", true);
        assert_eq!(r["answer"], "unknown", "{routine}: {r}");
        assert!(
            kinds(&r).contains(&"open-world-dispatch".to_string()),
            "{routine}: the site still blocks: {r}"
        );
        assert!(r.get("assumptions").is_none(), "{routine}: {r}");
    }

    // Both worlds answer `unknown`: the result is the closed world's, so its
    // blockers are what is left once the assumption is made (the Sink), not
    // the Logger it waived. It decides nothing, so it carries no label.
    let both = prove("LogsAndSinks", "writes-table:G24 T", true);
    assert_eq!(both["answer"], "unknown", "{both}");
    assert!(both.get("assumptions").is_none(), "{both}");
    assert_eq!(both["obligations"]["closedWorldRows"], 1, "{both}");
    let open_world: Vec<&str> = both["blockedBy"]
        .as_array()
        .unwrap_or_else(|| panic!("no blockers: {both}"))
        .iter()
        .filter(|b| b["kind"] == "open-world-dispatch")
        .filter_map(|b| b["detail"].as_str())
        .collect();
    assert_eq!(
        open_world,
        ["Sink.Put in LogsAndSinks"],
        "only the site a closed world cannot follow blocks: {both}"
    );
}

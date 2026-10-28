//! G28: an interface declared `Access = Internal` can be implemented only by
//! its own app and the apps its manifest lists in `internalsVisibleTo`. When
//! every such friend is loaded, no app outside the program can implement it,
//! so a call on it dispatches only to the implementers the program holds. The
//! site is then closed in the OPEN world too: `prove` answers `no` with no
//! assumption label (on Continia DO: `CDO Module License`).
//!
//! It stays open when the interface is public, or when the manifest names a
//! friend that is not loaded (that friend could implement it).
//!
//! Each case is a temp workspace asked through the real binary.

use std::path::Path;
use std::process::Command;

fn source(access: &str) -> String {
    format!(
        r#"table 50280 "G28 T"
{{
    fields {{ field(1; Code; Code[20]) {{ }} }}
    keys {{ key(PK; Code) {{ }} }}
}}
interface "G28 License"
{{
    {access}
    procedure IsActive(): Boolean;
}}
codeunit 50281 "G28 Real License" implements "G28 License"
{{
    procedure IsActive(): Boolean
    begin
        exit(true);
    end;
}}
codeunit 50282 "G28 Caller"
{{
    procedure Check(License: Interface "G28 License"): Boolean
    begin
        exit(License.IsActive());
    end;
}}
"#
    )
}

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

/// `prove <Check> writes-table:G28 T` on a workspace with the given interface
/// `Access` line and `internalsVisibleTo` entries.
fn prove_check(access: &str, friends: &str) -> serde_json::Value {
    let t = tempfile::tempdir().unwrap();
    let ws = t.path().join("ws");
    std::fs::create_dir_all(ws.join("src")).unwrap();
    std::fs::write(
        ws.join("app.json"),
        format!(
            r#"{{"id":"28282828-0000-4000-8000-000000000028","name":"G28","publisher":"t","version":"1.0.0.0","internalsVisibleTo":[{friends}]}}"#
        ),
    )
    .unwrap();
    std::fs::write(ws.join("src/Shapes.al"), source(access)).unwrap();
    let inv = alsem(
        t.path(),
        &["fingerprint", "ws", "--inventory-only", "--format", "json"],
    );
    let id = inv["payload"]["routineInventory"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["routineName"] == "Check")
        .expect("precondition: Check is in the inventory")["stableRoutineId"]
        .as_str()
        .unwrap()
        .to_string();
    alsem(
        t.path(),
        &["prove", "ws", &id, "writes-table:G28 T", "--deterministic"],
    )["payload"]["result"]
        .clone()
}

fn blocks_on_dispatch(r: &serde_json::Value) -> bool {
    r["blockedBy"].as_array().is_some_and(|a| {
        a.iter()
            .any(|b| b["kind"].as_str() == Some("open-world-dispatch"))
    })
}

#[test]
fn internal_interface_with_every_friend_loaded_is_closed() {
    // Precondition: a public interface blocks the open world.
    let public = prove_check("", "");
    assert_eq!(public["answer"], "unknown", "precondition: {public}");
    assert!(blocks_on_dispatch(&public), "precondition: {public}");

    // Internal, no friends: closed, with no assumption.
    let internal = prove_check("Access = Internal;", "");
    assert_eq!(internal["answer"], "no", "{internal}");
    assert!(!blocks_on_dispatch(&internal), "{internal}");
    assert!(internal.get("assumptions").is_none(), "{internal}");
    assert_eq!(internal["obligations"]["openWorldRows"], 0, "{internal}");

    // Internal, but a friend that is not loaded could implement it: open.
    let friend = prove_check(
        "Access = Internal;",
        r#"{"id":"28282828-0000-4000-8000-0000000000ff","name":"G28 Tests","publisher":"t"}"#,
    );
    assert_eq!(friend["answer"], "unknown", "{friend}");
    assert!(blocks_on_dispatch(&friend), "{friend}");
}

/// A DEPENDENCY's internal interface closes the same way, on its own
/// manifest's friends: Continia CDN's `IEndpointCreator` (internal, every CDN
/// friend loaded), held in a local filled from a factory getter, was
/// dispatched open-world from CDN's own code.
fn prove_dep_run(dep_friends_xml: &str) -> serde_json::Value {
    let lib = r#"interface "G28 Dep Creator"
{
    Access = Internal;
    procedure Create();
}
codeunit 12 "G28 Dep Real Creator" implements "G28 Dep Creator"
{
    procedure Create()
    begin
    end;
}
codeunit 13 "G28 Dep Factory"
{
    var
        Creator: Interface "G28 Dep Creator";

    internal procedure SetCreator(NewCreator: Interface "G28 Dep Creator")
    begin
        Creator := NewCreator;
    end;

    internal procedure GetCreator(): Interface "G28 Dep Creator"
    begin
        exit(Creator);
    end;
}
codeunit 10 "Dep Lib"
{
    procedure Outer()
    var
        Factory: Codeunit "G28 Dep Factory";
        Creator: Interface "G28 Dep Creator";
    begin
        Creator := Factory.GetCreator();
        Creator.Create();
    end;

    [IntegrationEvent(false, false)]
    procedure OnDepEvent()
    begin
    end;
}
"#;
    let t = tempfile::tempdir().unwrap();
    crate::dep_dispatch_fixture::write_own_lib_manifest(t.path(), "", lib, dep_friends_xml);
    alsem(
        t.path(),
        &[
            "prove",
            t.path().to_str().unwrap(),
            "CallChain",
            "writes-table:Customer",
            "--deterministic",
        ],
    )["payload"]["result"]
        .clone()
}

#[test]
fn a_dependencys_internal_interface_with_every_friend_loaded_is_closed() {
    let closed = prove_dep_run("");
    assert_eq!(closed["answer"], "no", "{closed}");
    assert!(!blocks_on_dispatch(&closed), "{closed}");

    // A friend of the dependency that is not loaded could implement it.
    let open = prove_dep_run(
        r#"<InternalsVisibleTo><Module Id="28282828-0000-4000-8000-0000000000ff" Name="G28 Dep Tests" Publisher="probe" /></InternalsVisibleTo>"#,
    );
    assert_eq!(open["answer"], "unknown", "{open}");
    assert!(blocks_on_dispatch(&open), "{open}");
}

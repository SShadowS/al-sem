//! A call into a cataloged platform native (`Base64Convert`, Microsoft's `System`
//! symbol package) has known behaviour.
//!
//! Continia DO reaches `Base64Convert.ToBase64`/`FromBase64` through System
//! Application's `Base64 Convert Impl.`. They are `[Native]`: the `System`
//! package has no AL body, so each call was an `unfetched-dependency` (an
//! opaque cone), and the package brought its `symbol-only-boundary` gap, so
//! `prove … no` stayed `unknown`. `platform_natives` now models them as
//! having no database, UI or commit effect; `FromBase64` keeps its implicit
//! error (a malformed string).
//!
//! The control: a `[Native]` method the catalog does not name
//! (`Index Management.DisableKey` changes the database) stays opaque.

use std::path::Path;
use std::process::Command;

const SYSTEM_SYMBOLS: &str = r#"{"AppId":"8874ed3a-0643-4247-9ced-7a7002f7135d","Name":"System","Publisher":"Microsoft","Version":"29.0.0.0",
"Namespaces":[{"Name":"System","Namespaces":[{"Name":"Runtime","Codeunits":[
 {"Id":2000000024,"Name":"Base64Convert","Methods":[
   {"Id":54826059,"Name":"ToBase64","Parameters":[{"Name":"String","TypeDefinition":{"Name":"Text"}}],"ReturnTypeDefinition":{"Name":"Text"},"Attributes":[{"Name":"Native"}]},
   {"Id":54826060,"Name":"FromBase64","Parameters":[{"Name":"Base64String","TypeDefinition":{"Name":"Text"}}],"ReturnTypeDefinition":{"Name":"Text"},"Attributes":[{"Name":"Native"}]}]},
 {"Id":2000000030,"Name":"Index Management","Methods":[
   {"Id":54826061,"Name":"DisableKey","Parameters":[{"Name":"TableNo","TypeDefinition":{"Name":"Integer"}}],"Attributes":[{"Name":"Native"}]}]}
]}]}]}"#;

const SOURCE: &str = r#"table 50260 "N T"
{
    fields { field(1; Code; Code[20]) { } }
    keys { key(PK; Code) { } }
}
codeunit 50261 "N Caller"
{
    procedure Encode(): Text
    var
        B64: Codeunit "Base64Convert";
    begin
        exit(B64.ToBase64('x'));
    end;

    procedure Decode(): Text
    var
        B64: Codeunit "Base64Convert";
    begin
        exit(B64.FromBase64('eA=='));
    end;

    procedure Disable()
    var
        IM: Codeunit "Index Management";
    begin
        IM.DisableKey(18);
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
fn a_cataloged_platform_native_has_known_behaviour() {
    let t = tempfile::tempdir().unwrap();
    let ws = t.path().join("ws");
    std::fs::create_dir_all(ws.join("src")).unwrap();
    std::fs::create_dir_all(ws.join(".alpackages")).unwrap();
    std::fs::write(
        ws.join("app.json"),
        r#"{"id":"26262626-0000-4000-8000-000000000026","name":"N","publisher":"t","version":"1.0.0.0",
            "dependencies":[{"id":"8874ed3a-0643-4247-9ced-7a7002f7135d","name":"System","publisher":"Microsoft","version":"29.0.0.0"}]}"#,
    )
    .unwrap();
    std::fs::write(ws.join("src/Caller.al"), SOURCE).unwrap();
    crate::symbol_app::write_symbol_app(
        &ws.join(".alpackages/Microsoft_System.app"),
        "8874ed3a-0643-4247-9ced-7a7002f7135d",
        "System",
        "29.0.0.0",
        SYSTEM_SYMBOLS,
    );
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
    let prove = |name: &str, q: &str| -> serde_json::Value {
        alsem(
            t.path(),
            &["prove", "ws", &id_of(name), q, "--deterministic"],
        )["payload"]["result"]
            .clone()
    };

    for routine in ["Encode", "Decode"] {
        let r = prove(routine, "writes-table:N T");
        assert_eq!(
            r["answer"], "no",
            "{routine}: a known native writes nothing: {r}"
        );
        assert_eq!(r["obligations"]["coverageComplete"], true, "{routine}: {r}");
        assert_eq!(r["obligations"]["analysisGaps"], 0, "{routine}: {r}");
    }
    let r = prove("Decode", "throws-error");
    assert_eq!(r["answer"], "yes", "FromBase64 can raise an error: {r}");
    let r = prove("Encode", "throws-error");
    assert_eq!(r["answer"], "no", "ToBase64 raises none: {r}");

    let r = prove("Disable", "writes-table:N T");
    assert_eq!(
        r["answer"], "unknown",
        "an uncataloged native stays opaque: {r}"
    );
}

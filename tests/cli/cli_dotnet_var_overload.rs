//! A DotNet argument picks the DotNet overload over a native one, whatever its
//! alias.
//!
//! Base Application's `XMLDOMManagement.AddAttribute` has `(var XMLNode: DotNet
//! XmlNode; …)` and `(var ParentXmlNode: XmlNode; …)`; `ConfigXMLExchange`
//! passes a `DotNet XmlElement`. Every DotNet type canonicalizes to `dotnet`,
//! and the `var` exact-text rule called `XmlElement` against `XmlNode`
//! incompatible, so both candidates were out and the call stayed
//! `ambiguousResolved` (Continia DO: 7 such dependency sites). A `var` DotNet
//! parameter takes a derived class, so two aliases are undecided; the native
//! `XmlNode` one never takes a DotNet value.

use std::process::Command;

const SOURCE: &str = r#"codeunit 50410 "DN Lib"
{
    procedure AddAttribute(var Node: DotNet XmlNode; Name: Text)
    begin
    end;

    procedure AddAttribute(var Node: XmlNode; Name: Text)
    begin
    end;
}
codeunit 50411 "DN Caller"
{
    procedure Go()
    var
        Lib: Codeunit "DN Lib";
        Element: DotNet XmlElement;
    begin
        Lib.AddAttribute(Element, 'a');
    end;
}
"#;

#[test]
fn a_dotnet_argument_picks_the_dotnet_var_overload() {
    let t = tempfile::tempdir().unwrap();
    let ws = t.path().join("ws");
    std::fs::create_dir_all(ws.join("src")).unwrap();
    std::fs::write(
        ws.join("app.json"),
        r#"{"id":"35353535-0000-4000-8000-000000000035","name":"DN","publisher":"t","version":"1.0.0.0"}"#,
    )
    .unwrap();
    std::fs::write(ws.join("src/a.al"), SOURCE).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_aldump"))
        .arg("--program-call-graph-stats")
        .arg(&ws)
        .env("RUST_LOG", "off")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "aldump failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("stats JSON");
    let s = &v["primaryScoped"];
    assert_eq!(s["resolvedSource"], 1, "{s}");
    assert_eq!(s["ambiguousResolved"], 0, "{s}");
}

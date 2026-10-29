//! A Text argument converts into a `SecretText` parameter, so it is no
//! elimination there.
//!
//! System Application's `SignedXmlImpl.SetSigningKey(XmlString)` passes a Text
//! to overloads `(XmlString: SecretText)` / `(SignatureKey: Codeunit "Signature
//! Key")`. Text against SecretText counted as proven incompatible, so both
//! candidates were out and the call stayed `ambiguousResolved`; the Codeunit one
//! alone is, so the SecretText one binds (rule (B)).

use std::process::Command;

const SOURCE: &str = r#"codeunit 50430 "ST Key"
{
}
codeunit 50431 "ST Lib"
{
    procedure SetSigningKey(Key: SecretText)
    begin
    end;

    procedure SetSigningKey(Key: Codeunit "ST Key")
    begin
    end;
}
codeunit 50432 "ST Caller"
{
    procedure Go(XmlString: Text)
    var
        Lib: Codeunit "ST Lib";
    begin
        Lib.SetSigningKey(XmlString);
    end;
}
"#;

#[test]
fn a_text_argument_binds_the_secrettext_overload() {
    let t = tempfile::tempdir().unwrap();
    let ws = t.path().join("ws");
    std::fs::create_dir_all(ws.join("src")).unwrap();
    std::fs::write(
        ws.join("app.json"),
        r#"{"id":"37373737-0000-4000-8000-000000000037","name":"ST","publisher":"t","version":"1.0.0.0"}"#,
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

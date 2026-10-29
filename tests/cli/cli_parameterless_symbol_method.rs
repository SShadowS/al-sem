//! A symbol-only method with no parameters is called with none.
//!
//! The compiler leaves an empty array out of `SymbolReference.json`, so a
//! method that takes no parameters has no `Parameters` key at all (BC 29's
//! Base Application: 7211 methods so, one `"Parameters": []`). Ingestion read
//! the missing key as an unknown arity, which matches no call: every 0-argument
//! call to the platform's `System` package was an `arityMismatch` unknown
//! (Continia DO: 38 in System and Base Application, among them
//! `AgentUtilities.BlockPageFromBeingOpenedByAgent()` and
//! `IndexManagement.GetSupportedOptimizedTextSearchLanguages().Get(..)`, whose
//! `.Get` was a `compoundReceiver` unknown behind it).

use std::path::Path;
use std::process::Command;

/// Two platform methods spelled as the compiler writes them: no `Parameters`
/// key. `DisableKey` (with parameters) is the control the old reading already
/// resolved.
const SYSTEM_SYMBOLS: &str = r#"{"AppId":"8874ed3a-0643-4247-9ced-7a7002f7135d","Name":"System","Publisher":"Microsoft","Version":"29.0.0.0",
"Namespaces":[{"Name":"System","Namespaces":[{"Name":"Runtime","Codeunits":[
 {"Id":2000000029,"Name":"Index Management","Methods":[
   {"Id":-1270646793,"Name":"GetSupportedOptimizedTextSearchLanguages","ReturnTypeDefinition":{"Name":"Dictionary","TypeArguments":[{"Name":"Integer"},{"Name":"Text"}]},"Attributes":[{"Name":"Native"}]},
   {"Id":54826062,"Name":"GetCurrentOptimizedTextSearchLanguage","ReturnTypeDefinition":{"Name":"Integer"},"Attributes":[{"Name":"Native"}]},
   {"Id":54826061,"Name":"DisableKey","Parameters":[{"Name":"TableNo","TypeDefinition":{"Name":"Integer"}}],"Attributes":[{"Name":"Native"}]}]}
]}]}]}"#;

const SOURCE: &str = r#"codeunit 50271 "P Caller"
{
    procedure Name(): Text
    var
        IM: Codeunit "Index Management";
    begin
        exit(IM.GetSupportedOptimizedTextSearchLanguages().Get(IM.GetCurrentOptimizedTextSearchLanguage()));
    end;

    procedure Disable()
    var
        IM: Codeunit "Index Management";
    begin
        IM.DisableKey(18);
    end;
}
"#;

fn stats(ws: &Path) -> serde_json::Value {
    let out = Command::new(env!("CARGO_BIN_EXE_aldump"))
        .arg("--program-call-graph-stats")
        .arg(ws)
        .env("RUST_LOG", "off")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "aldump failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("stats JSON");
    v["primaryScoped"].clone()
}

#[test]
fn a_symbol_method_without_parameters_matches_a_call_with_none() {
    let t = tempfile::tempdir().unwrap();
    let ws = t.path().join("ws");
    std::fs::create_dir_all(ws.join("src")).unwrap();
    std::fs::write(
        ws.join("app.json"),
        r#"{"id":"27272727-0000-4000-8000-000000000027","name":"P","publisher":"t","version":"1.0.0.0",
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
    let s = stats(&ws);
    // The two parameterless calls and `DisableKey` reach the symbols; `.Get`
    // on the returned Dictionary is a built-in.
    assert_eq!(s["resolvedAbiExternal"], 3, "{s}");
    assert_eq!(s["unknown"], 0, "{s}");
}

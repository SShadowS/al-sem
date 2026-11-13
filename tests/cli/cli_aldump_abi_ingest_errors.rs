//! TLS-47 / engine limit L37: `aldump --program-call-graph-stats` does not list
//! the dependency packages the loader drops. A package whose
//! `SymbolReference.json` does not parse is dropped before the graph build (so
//! `ProgramGraph::abi_ingest_errors` stays empty); a call into it counts as
//! `unknown`, and nothing in the stats names the package. `alsem analyze`'s
//! dependency ledger is where it is reported.

use std::path::Path;
use std::process::Command;

use crate::symbol_app::write_symbol_app;

const DEP: &str = "cccc4747-0000-0000-0000-000000000047";

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn stats(ws: &Path) -> (serde_json::Value, String) {
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
    let text = String::from_utf8(out.stdout).unwrap();
    (serde_json::from_str(&text).expect("stats JSON"), text)
}

fn dep_app(ws: &Path, symbols: &str) {
    write_symbol_app(
        &ws.join(".alpackages/probe_AbiDep_1.0.0.0.app"),
        DEP,
        "AbiDep",
        "1.0.0.0",
        symbols,
    );
}

#[test]
fn a_call_into_a_dropped_package_is_unknown_and_unlisted() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    write(
        &ws.join("app.json"),
        &format!(
            r#"{{"id":"aaaa4747-0000-0000-0000-000000000047","name":"AbiWs","publisher":"probe","version":"1.0.0.0","runtime":"13.0","dependencies":[{{"id":"{DEP}","name":"AbiDep","publisher":"probe","version":"1.0.0.0"}}]}}"#
        ),
    );
    write(
        &ws.join("src/Cu.al"),
        "codeunit 50047 \"Abi Cu\"\n{\n    procedure A()\n    var\n        L: Codeunit \"Abi Lib\";\n    begin\n        L.Work();\n    end;\n}\n",
    );

    // A readable package: the call resolves through its ABI (non-vacuity).
    dep_app(
        ws,
        &format!(
            r#"{{"RuntimeVersion":"13.0","Codeunits":[{{"Id":50300,"Name":"Abi Lib","Methods":[{{"Name":"Work","Parameters":[]}}]}}],"AppId":"{DEP}","Name":"AbiDep","Publisher":"probe","Version":"1.0.0.0"}}"#
        ),
    );
    let (v, _) = stats(ws);
    let p = &v["primaryScoped"];
    assert_eq!(p["resolvedAbiExternal"], 1, "{p}");
    assert_eq!(p["unknown"], 0, "{p}");

    // The same package with symbols that do not parse: the loader drops it.
    dep_app(ws, "{ this is not JSON");
    let (v, text) = stats(ws);
    let p = &v["primaryScoped"];
    assert_eq!(p["resolvedAbiExternal"], 0, "{p}");
    assert_eq!(p["unknown"], 1, "the call into the dropped package: {p}");
    assert!(
        !text.contains("AbiDep"),
        "the stats do not name the dropped package (L37): {text}"
    );
}

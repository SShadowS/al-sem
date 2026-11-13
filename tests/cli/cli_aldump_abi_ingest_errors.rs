//! TLS-47: `aldump --program-call-graph-stats` prints the loaded dependencies
//! whose ABI failed to ingest (`abiIngestErrors`, and a stderr warning), and
//! engine limit L37: a package the loader drops is not in the JSON at all.
//!
//! The two failures differ because the loader's typed parse skips members it
//! does not name, while the ABI reader checks the whole value strictly (ENG-68):
//! a `1e400` in a skipped member loads the package and fails its ABI; text that
//! is not JSON drops the package. Calls into the dependency are `unknown` both
//! ways.

use std::path::Path;
use std::process::Command;

use crate::symbol_app::write_symbol_app;

const DEP: &str = "cccc4747-0000-0000-0000-000000000047";

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// The stats JSON and stderr.
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
    (
        serde_json::from_slice(&out.stdout).expect("stats JSON"),
        String::from_utf8(out.stderr).unwrap(),
    )
}

/// The dependency's symbols with one codeunit `Abi Lib` (method `Work`), plus
/// `extra` members at the top level.
fn symbols(extra: &str) -> String {
    format!(
        r#"{{"RuntimeVersion":"13.0",{extra}"Codeunits":[{{"Id":50300,"Name":"Abi Lib","Methods":[{{"Name":"Work","Parameters":[]}}]}}],"AppId":"{DEP}","Name":"AbiDep","Publisher":"probe","Version":"1.0.0.0"}}"#
    )
}

fn workspace(ws: &Path, symbols: &str) {
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
    write_symbol_app(
        &ws.join(".alpackages/probe_AbiDep_1.0.0.0.app"),
        DEP,
        "AbiDep",
        "1.0.0.0",
        symbols,
    );
}

/// A readable ABI: the call resolves and nothing is reported (non-vacuity).
#[test]
fn a_readable_dependency_reports_no_ingest_error() {
    let dir = tempfile::tempdir().unwrap();
    workspace(dir.path(), &symbols(""));
    let (v, err) = stats(dir.path());
    assert_eq!(v["primaryScoped"]["resolvedAbiExternal"], 1, "{v}");
    assert_eq!(v["abiIngestErrors"]["count"], 0, "{}", v["abiIngestErrors"]);
    assert!(!err.contains("could not be read"), "{err}");
}

/// The loader accepts the package (it skips `Junk`), the ABI reader rejects the
/// out-of-range number: the stats name the app and the reason, on stdout and
/// stderr.
///
/// Discrimination (2026-11-13): clearing `abi_ingest_errors` in
/// `resolve_full_program_reusing` before the report fails here (count 0);
/// reverted, passes.
#[test]
fn a_dependency_whose_abi_fails_to_ingest_is_reported() {
    let dir = tempfile::tempdir().unwrap();
    workspace(dir.path(), &symbols(r#""Junk":1e400,"#));
    let (v, err) = stats(dir.path());
    assert_eq!(v["primaryScoped"]["unknown"], 1, "{v}");
    let e = &v["abiIngestErrors"];
    assert_eq!(e["count"], 1, "{e}");
    let line = e["errors"][0].as_str().expect("an error line");
    assert!(
        line.starts_with("probe/AbiDep@1.0.0.0: ") && line.contains("number out of range"),
        "{line}"
    );
    assert!(
        err.contains("aldump: warning: dependency ABI could not be read: probe/AbiDep@1.0.0.0: "),
        "{err}"
    );
}

/// L37: symbols that are not JSON make the loader drop the package, so it is
/// not in the stats JSON, and the call into it is `unknown`. `alsem analyze`
/// reports it.
#[test]
fn a_package_the_loader_drops_is_not_in_the_stats() {
    let dir = tempfile::tempdir().unwrap();
    workspace(dir.path(), "{ this is not JSON");
    let (v, _) = stats(dir.path());
    assert_eq!(v["primaryScoped"]["unknown"], 1, "{v}");
    assert_eq!(v["abiIngestErrors"]["count"], 0, "{}", v["abiIngestErrors"]);
}

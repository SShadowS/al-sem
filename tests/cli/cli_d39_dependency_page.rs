//! #302: a dependency page known only from its symbols (no embedded `.al`) carries
//! its `Editable` / `InsertAllowed` / `ModifyAllowed` / `DeleteAllowed` /
//! `SourceTableTemporary` into the model, so d39's read-only page rule and opt-in
//! d64 judge it like a source page (spec: `## Design`, revision 3, in the issue
//! ledger; tests 2 and 3).
//!
//! Every test drives the REAL analyze pipeline (`run_analyze_with_exit`, default
//! cross-app mode) over a temp copy of a committed fixture under
//! `tests/fixtures/issue-302/`. The dependency `.app` is written into the copy's
//! `.alpackages` at test time by `symbol_app` (fixed zip timestamp, so its bytes are
//! a pure function of the fixture), symbol-only or with its `.al` embedded.
//!
//! The fixture leaves out a read-only base page with `SourceTableTemporary = true`
//! on purpose: a pageext over one still reports d39 (the pageext's `Rec` is not
//! marked temporary), a pre-existing false positive this test does not pin.

use std::path::{Path, PathBuf};

use al_sem::engine::gate::filter::Scope;
use al_sem::engine::gate::run::{AnalyzeArgs, OutputFormat, run_analyze_with_exit};

use crate::symbol_app;

const DEP_GUID: &str = "22222222-0000-0000-0000-000000000302";
const D39: &str = "d39-record-left-dirty-across-chain";
const D64: &str = "d64-api-page-write-surface";

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/issue-302")
        .join(name)
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// A temp copy of workspace `ws` with dependency `Dep302` (symbols from
/// `dep_dir`) in its `.alpackages`; `embed` also ships `dep/src/Dep.al`.
/// Returns the temp dir and the `.app` path.
fn workspace(ws: &str, dep_dir: &str, embed: bool) -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().expect("tempdir");
    crate::copy_dir(&fixture(ws), tmp.path());
    let symbols = read(&fixture(dep_dir).join("SymbolReference.json"));
    let app = tmp.path().join(".alpackages/probe_Dep302_1.0.0.0.app");
    if embed {
        let al = read(&fixture("dep/src/Dep.al"));
        symbol_app::write_source_app(
            &app,
            DEP_GUID,
            "Dep302",
            "1.0.0.0",
            &symbols,
            &[("src/Dep.al", &al)],
            "",
        );
    } else {
        symbol_app::write_symbol_app(&app, DEP_GUID, "Dep302", "1.0.0.0", &symbols);
    }
    (tmp, app)
}

/// The zip entry names of an `.app` (40-byte NAVX header, then a zip).
fn app_entries(app: &Path) -> Vec<String> {
    let bytes = std::fs::read(app).expect("read .app");
    let zip = zip::ZipArchive::new(std::io::Cursor::new(bytes[40..].to_vec())).expect("app zip");
    zip.file_names()
        .map(|n| n.expect("zip entry name").into_owned())
        .collect()
}

/// The page object `{ "Id": .., "Name": "<page>", ... }` text inside `symbols`.
fn page_text<'a>(symbols: &'a str, page: &str) -> &'a str {
    let start = symbols
        .find(&format!(r#""Name": "{page}""#))
        .unwrap_or_else(|| panic!("page {page} not in symbols"));
    let len = symbols[start..].find("]").expect("end of Properties");
    &symbols[start..start + len]
}

/// Assert, by hand, that page `page` carries exactly the write-surface property
/// literals `props` (`(name, value)`), each once, and no other one.
fn assert_page_props(symbols: &str, page: &str, props: &[(&str, &str)]) {
    let text = page_text(symbols, page);
    for name in [
        "Editable",
        "InsertAllowed",
        "ModifyAllowed",
        "DeleteAllowed",
        "SourceTableTemporary",
    ] {
        let want = props.iter().find(|(n, _)| *n == name);
        let literal_count = text.matches(&format!(r#"{{"Name":"{name}","#)).count();
        match want {
            Some((_, v)) => {
                let literal = format!(r#"{{"Name":"{name}","Value":"{v}"}}"#);
                assert_eq!(text.matches(&literal).count(), 1, "{page}: {literal}");
                assert_eq!(literal_count, 1, "{page}: one {name}");
            }
            None => assert_eq!(literal_count, 0, "{page}: no {name}"),
        }
    }
}

/// Run analyze; returns (stdout JSON, stderr warning).
fn analyze(ws: &Path, detector: &str, scope: Scope) -> (serde_json::Value, Option<String>) {
    let args = AnalyzeArgs {
        workspace: ws.to_string_lossy().to_string(),
        min_severity: None,
        detector: Some(detector.to_string()),
        preset: None,
        scope,
        limit: None,
        format: OutputFormat::Json,
        sarif_version_override: None,
        fail_on: None,
        require_dependencies: false,
        baseline: None,
        update_baseline: false,
        disable_inline_suppression: false,
        group_by: None,
        deterministic: true,
        single_app: false,
        with_evidence: false,
    };
    let (out, _, stderr, _) =
        run_analyze_with_exit(&args.read_inputs(), &args.read_request(), &args, "302-test")
            .unwrap_or_else(|e| panic!("run_analyze failed: {e}"));
    (
        serde_json::from_str(&out).expect("analyze JSON must parse"),
        stderr,
    )
}

/// `"<object> | <routine> | <fingerprint>"` for each `detector` finding, sorted.
fn rows(json: &serde_json::Value, detector: &str) -> Vec<String> {
    let mut out: Vec<String> = json["payload"]["findings"]
        .as_array()
        .expect("findings array")
        .iter()
        .filter(|f| f["detector"] == detector)
        .map(|f| {
            let loc = &f["primaryLocation"];
            format!(
                "{} | {} | {}",
                loc["objectName"].as_str().expect("objectName"),
                loc["routineName"].as_str().unwrap_or(""),
                f["fingerprint"].as_str().expect("fingerprint"),
            )
        })
        .collect();
    out.sort();
    out
}

/// `rows` without the fingerprint column.
fn names(rows: &[String]) -> Vec<String> {
    rows.iter()
        .map(|r| r.rsplit_once(" | ").unwrap().0.to_string())
        .collect()
}

fn opaque_apps(json: &serde_json::Value) -> &serde_json::Value {
    &json["payload"]["summary"]["opaqueApps"]
}

/// Test 2: a pageext over a symbol-only `Editable "0"` / `ModifyAllowed "0"`
/// dependency page reports d39 like its embedded-source twin, with the same
/// fingerprints. `Dep Card` (nothing declared) and `Dep Editable Card` (`"1"`) stay
/// modifiable; the workspace page `W RO List` is the control that proves the run
/// reaches the read-only rule at all.
#[test]
fn d39_reads_a_symbol_only_dependency_page() {
    let symbols = read(&fixture("dep/SymbolReference.json"));
    // Preconditions, by hand: the symbols carry exactly these properties.
    assert_eq!(
        symbols
            .matches(r#"{"Name":"Editable","Value":"0"}"#)
            .count(),
        1
    );
    assert_page_props(&symbols, "Dep RO List", &[("Editable", "0")]);
    assert_page_props(&symbols, "Dep NoMod Card", &[("ModifyAllowed", "0")]);
    assert_page_props(&symbols, "Dep Card", &[]);
    assert_page_props(
        &symbols,
        "Dep Editable Card",
        &[("Editable", "1"), ("ModifyAllowed", "1")],
    );

    let expected = [
        "W NoMod Ext | OnAction",
        "W RO Ext | OnAction",
        "W RO List | OnAction",
    ];

    // Symbol-only: the `.app` has no `.al`, and the run took the ABI path.
    let (sym_ws, sym_app) = workspace("ws", "dep", false);
    let entries = app_entries(&sym_app);
    assert!(
        entries.iter().all(|e| !e.ends_with(".al")),
        "symbol-only .app must hold no .al: {entries:?}"
    );
    let (sym, sym_err) = analyze(sym_ws.path(), D39, Scope::Primary);
    assert_eq!(opaque_apps(&sym), &serde_json::json!(["Dep302"]));
    let warning = sym_err.expect("symbol-only run warns on stderr");
    assert!(
        warning.contains("symbol-only dependency app(s): Dep302"),
        "{warning}"
    );
    let sym_rows = rows(&sym, D39);
    assert_eq!(names(&sym_rows), expected);

    // Embedded twin: the same dependency with its `.al`, read from source.
    let (emb_ws, emb_app) = workspace("ws", "dep", true);
    assert!(app_entries(&emb_app).contains(&"src/Dep.al".to_string()));
    let (emb, emb_err) = analyze(emb_ws.path(), D39, Scope::Primary);
    assert_eq!(opaque_apps(&emb), &serde_json::json!([]));
    assert!(
        !emb_err
            .as_deref()
            .unwrap_or("")
            .contains("symbol-only dependency"),
        "{emb_err:?}"
    );
    assert_eq!(rows(&emb, D39), sym_rows, "parity with the source path");
}

/// Test 3: opt-in d64 over three symbol-only API pages. `Dep API Closed` (all four
/// `"0"`) and `Dep API Temp` (`Editable "0"`, `SourceTableTemporary "1"`) are
/// skipped; `Dep API Open` (nothing declared) reports `info`, which proves d64
/// reaches dependency pages at all.
#[test]
fn d64_reads_symbol_only_api_page_properties() {
    let symbols = read(&fixture("dep-api/SymbolReference.json"));
    assert_page_props(
        &symbols,
        "Dep API Closed",
        &[
            ("Editable", "0"),
            ("InsertAllowed", "0"),
            ("ModifyAllowed", "0"),
            ("DeleteAllowed", "0"),
        ],
    );
    assert_page_props(
        &symbols,
        "Dep API Temp",
        &[("Editable", "0"), ("SourceTableTemporary", "1")],
    );
    assert_page_props(&symbols, "Dep API Open", &[]);
    assert_eq!(
        symbols
            .matches(r#"{"Name":"PageType","Value":"API"}"#)
            .count(),
        3
    );

    let (ws, app) = workspace("ws-api", "dep-api", false);
    let entries = app_entries(&app);
    assert!(entries.iter().all(|e| !e.ends_with(".al")), "{entries:?}");
    let (json, err) = analyze(ws.path(), D64, Scope::All);
    assert_eq!(opaque_apps(&json), &serde_json::json!(["Dep302"]));
    assert!(
        err.as_deref()
            .unwrap_or("")
            .contains("symbol-only dependency app(s): Dep302"),
        "{err:?}"
    );
    let got: Vec<(String, String)> = json["payload"]["findings"]
        .as_array()
        .expect("findings array")
        .iter()
        .filter(|f| f["detector"] == D64)
        .map(|f| {
            (
                f["primaryLocation"]["objectName"]
                    .as_str()
                    .expect("objectName")
                    .to_string(),
                f["severity"].as_str().expect("severity").to_string(),
            )
        })
        .collect();
    assert_eq!(got, [("Dep API Open".to_string(), "info".to_string())]);
}

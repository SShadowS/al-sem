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
//! `Dep RO Temp List` and the workspace page `W RO Temp Base` are read-only AND
//! `SourceTableTemporary`: the implicit `Rec` of a pageext over either is its base
//! page's temporary copy (`record_types`, #302 CR1), so d39's temp skip keeps
//! `W RO Temp Ext` and `W RO Temp WsExt` silent in both runs. A DECLARED physical
//! `Rec` local is its own record (#302 F5): `W RO Temp Local` (a temporary page) and
//! `W RO Temp LocalExt` (a pageext over one) keep reporting.

use std::path::{Path, PathBuf};

use al_sem::engine::gate::filter::Scope;
use al_sem::engine::gate::run::{AnalyzeArgs, OutputFormat, run_analyze_with_exit};

use crate::symbol_app;

const DEP_GUID: &str = "22222222-0000-0000-0000-000000000302";
const D39: &str = "d39-record-left-dirty-across-chain";
const D3: &str = "d3-missing-setloadfields";
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

/// The parsed `SymbolReference.json` of `dep_dir`.
fn symbols(dep_dir: &str) -> serde_json::Value {
    serde_json::from_str(&read(&fixture(dep_dir).join("SymbolReference.json")))
        .expect("fixture symbols parse")
}

/// Assert, by hand, the pages of `symbols` and the write-surface properties each
/// carries, exactly: `(page, [(name, value)])`, in file order.
fn assert_pages(symbols: &serde_json::Value, want: &[(&str, &[(&str, &str)])]) {
    const WRITE_SURFACE: [&str; 5] = [
        "Editable",
        "InsertAllowed",
        "ModifyAllowed",
        "DeleteAllowed",
        "SourceTableTemporary",
    ];
    let got: Vec<(String, Vec<(String, String)>)> = symbols["Pages"]
        .as_array()
        .expect("Pages array")
        .iter()
        .map(|p| {
            let props = p["Properties"]
                .as_array()
                .expect("Properties array")
                .iter()
                .map(|q| {
                    let s = |k: &str| q[k].as_str().expect("string Name/Value").to_string();
                    (s("Name"), s("Value"))
                })
                .filter(|(n, _)| WRITE_SURFACE.iter().any(|w| w.eq_ignore_ascii_case(n)))
                .collect();
            (p["Name"].as_str().expect("page Name").to_string(), props)
        })
        .collect();
    let want: Vec<(String, Vec<(String, String)>)> = want
        .iter()
        .map(|(page, props)| {
            let props = props
                .iter()
                .map(|(n, v)| (n.to_string(), v.to_string()))
                .collect();
            (page.to_string(), props)
        })
        .collect();
    assert_eq!(got, want);
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
/// modifiable; `Dep RO Temp List` and `W RO Temp Base` are read-only but temporary,
/// so the implicit `Rec` of their pages and pageexts stays silent too, while a
/// declared physical `Rec` local on a temporary page or pageext reports, in d39 and
/// in d3 (`ReadLocal`; d3 skips a temporary record, so it reads the local's own temp
/// state). The workspace page `W RO List` is the control that proves the run
/// reaches the read-only rule at all.
#[test]
fn d39_reads_a_symbol_only_dependency_page() {
    // Precondition, by hand: the symbols carry exactly these pages and properties.
    assert_pages(
        &symbols("dep"),
        &[
            ("Dep RO List", &[("Editable", "0")]),
            ("Dep NoMod Card", &[("ModifyAllowed", "0")]),
            ("Dep Card", &[]),
            (
                "Dep Editable Card",
                &[("Editable", "1"), ("ModifyAllowed", "1")],
            ),
            (
                "Dep RO Temp List",
                &[("Editable", "0"), ("SourceTableTemporary", "1")],
            ),
        ],
    );

    let expected = [
        "W NoMod Ext | OnAction",
        "W RO Ext | OnAction",
        "W RO List | OnAction",
        "W RO Temp Local | OnAction",
        "W RO Temp LocalExt | OnAction",
    ];
    let expected_d3 = [
        "W RO Temp Local | ReadLocal",
        "W RO Temp LocalExt | ReadLocal",
    ];
    let detectors = format!("{D39},{D3}");

    // Symbol-only: the `.app` has no `.al`, and the run took the ABI path.
    let (sym_ws, sym_app) = workspace("ws", "dep", false);
    let entries = app_entries(&sym_app);
    assert!(
        entries.iter().all(|e| !e.ends_with(".al")),
        "symbol-only .app must hold no .al: {entries:?}"
    );
    let (sym, sym_err) = analyze(sym_ws.path(), &detectors, Scope::Primary);
    assert_eq!(opaque_apps(&sym), &serde_json::json!(["Dep302"]));
    let warning = sym_err.expect("symbol-only run warns on stderr");
    assert!(
        warning.contains("symbol-only dependency app(s): Dep302"),
        "{warning}"
    );
    let sym_rows = rows(&sym, D39);
    assert_eq!(names(&sym_rows), expected);
    let sym_d3 = rows(&sym, D3);
    assert_eq!(names(&sym_d3), expected_d3);

    // Embedded twin: the same dependency with its `.al`, read from source.
    let (emb_ws, emb_app) = workspace("ws", "dep", true);
    assert!(app_entries(&emb_app).contains(&"src/Dep.al".to_string()));
    let (emb, emb_err) = analyze(emb_ws.path(), &detectors, Scope::Primary);
    assert_eq!(opaque_apps(&emb), &serde_json::json!([]));
    assert!(
        !emb_err
            .as_deref()
            .unwrap_or("")
            .contains("symbol-only dependency"),
        "{emb_err:?}"
    );
    assert_eq!(rows(&emb, D39), sym_rows, "parity with the source path");
    assert_eq!(rows(&emb, D3), sym_d3, "d3 parity with the source path");
}

/// Test 3: opt-in d64 over three symbol-only API pages. `Dep API Closed` (all four
/// `"0"`) and `Dep API Temp` (`Editable "0"`, `SourceTableTemporary "1"`) are
/// skipped; `Dep API Open` (nothing declared) reports `info`, which proves d64
/// reaches dependency pages at all.
#[test]
fn d64_reads_symbol_only_api_page_properties() {
    let symbols = symbols("dep-api");
    assert_pages(
        &symbols,
        &[
            (
                "Dep API Closed",
                &[
                    ("Editable", "0"),
                    ("InsertAllowed", "0"),
                    ("ModifyAllowed", "0"),
                    ("DeleteAllowed", "0"),
                ],
            ),
            (
                "Dep API Temp",
                &[("Editable", "0"), ("SourceTableTemporary", "1")],
            ),
            ("Dep API Open", &[]),
        ],
    );
    let page_types: Vec<&str> = symbols["Pages"]
        .as_array()
        .expect("Pages array")
        .iter()
        .flat_map(|p| p["Properties"].as_array().expect("Properties array"))
        .filter(|q| q["Name"] == "PageType")
        .map(|q| q["Value"].as_str().expect("PageType value"))
        .collect();
    assert_eq!(page_types, ["API"; 3]);

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

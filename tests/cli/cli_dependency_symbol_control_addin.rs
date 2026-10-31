//! A dependency page's `CurrPage.<usercontrol>.<procedure>()` resolves when the
//! package embeds the page but declares its control add-in only in symbols.
//!
//! Continia's CDN embeds `usercontrol(RefreshTimer; "CTS-CDN Refresh Timer")` and
//! calls `CurrPage.RefreshTimer.Schedule(..)`, but ships no `controladdin` `.al`.
//! A dependency with source took no objects from its symbols, so the add-in was
//! missing (`compoundReceiver`); and the symbols' add-ins were read with no
//! procedures at all, so once present it had no `Schedule` (`memberNotFound`).

use std::process::Command;

const DEP_GUID: &str = "28282828-0000-4000-8000-000000000028";

const DEP_SYMBOLS: &str = r#"{"AppId":"28282828-0000-4000-8000-000000000028","Name":"DepAddin","Publisher":"probe","Version":"1.0.0.0",
"Pages":[{"Id":50300,"Name":"Dep Timer Page"}],
"ControlAddIns":[{"Name":"Dep Timer",
  "Methods":[{"MethodKind":5,"Name":"Schedule","Parameters":[{"Name":"Ms","TypeDefinition":{"Name":"Integer"}}]},{"MethodKind":5,"Name":"Cancel"}],
  "Events":[{"Name":"Ready"}]}]}"#;

const DEP_PAGE: &str = r#"page 50300 "Dep Timer Page"
{
    layout
    {
        area(Content)
        {
            usercontrol(Timer; "Dep Timer")
            {
                trigger Ready()
                begin
                    Go();
                end;
            }
        }
    }

    procedure Go()
    begin
        CurrPage.Timer.Schedule(10);
        CurrPage.Timer.Cancel();
    end;
}
"#;

const WS: &str = r#"codeunit 50310 "Ws Runner"
{
    procedure Open()
    begin
        Page.Run(Page::"Dep Timer Page");
    end;
}
"#;

/// The workspace with the source dependency, written under `t`.
fn write_workspace(t: &tempfile::TempDir) -> std::path::PathBuf {
    let ws = t.path().join("ws");
    std::fs::create_dir_all(ws.join("src")).unwrap();
    std::fs::write(
        ws.join("app.json"),
        format!(
            r#"{{"id":"29292929-0000-4000-8000-000000000029","name":"W","publisher":"t","version":"1.0.0.0",
            "dependencies":[{{"id":"{DEP_GUID}","name":"DepAddin","publisher":"probe","version":"1.0.0.0"}}]}}"#
        ),
    )
    .unwrap();
    std::fs::write(ws.join("src/Runner.al"), WS).unwrap();
    crate::symbol_app::write_source_app(
        &ws.join(".alpackages/probe_DepAddin_1.0.0.0.app"),
        DEP_GUID,
        "DepAddin",
        "1.0.0.0",
        DEP_SYMBOLS,
        &[("src/DepTimerPage.Page.al", DEP_PAGE)],
        "",
    );
    ws
}

#[test]
fn a_dependency_usercontrol_resolves_against_its_symbol_only_addin() {
    let t = tempfile::tempdir().unwrap();
    let ws = write_workspace(&t);
    let out = Command::new(env!("CARGO_BIN_EXE_aldump"))
        .args(["--dependency-bodies-stats"])
        .arg(&ws)
        .arg("--sites")
        .env("RUST_LOG", "off")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "aldump failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("stats JSON");
    // Precondition: the dependency's page body was resolved at all.
    assert!(
        v["all"]["total"].as_u64().unwrap_or(0) >= 3,
        "the page's calls are counted: {}",
        v["all"]
    );
    assert_eq!(v["all"]["unknown"], 0, "{}", v["unknownSites"]);
}

/// The add-in's procedures map back to the raw ABI. The graph holds them as
/// SymbolOnly routines of a dependency WITH source; the ABI integrity check's
/// raw index re-read only symbol-only dependencies, so each was an unmapped
/// "ingestion bug" (on the pinned CDO baseline: the 9 procedures of the System
/// Application's legacy `Microsoft.Dynamics.Nav.Client.WebPageViewer`).
#[test]
fn a_source_dependency_symbol_addin_maps_back_to_the_raw_abi() {
    let t = tempfile::tempdir().unwrap();
    let ws = write_workspace(&t);
    let report = al_sem::program::resolve::abi_check::run_abi_integrity_check(&ws);
    // Precondition: the build ingested `Schedule` and `Cancel` as SymbolOnly
    // routines, so the check has something to look up.
    assert_eq!(report.abi_routes_total, 2, "{report:?}");
    assert_eq!(report.abi_unmapped, 0, "{report:?}");
}

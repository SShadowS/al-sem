//! ENG-151 — d19 on a method implementing a DEPENDENCY interface member, through
//! the real cross-app analyze pipeline. The cross-app model keeps only the
//! demanded routines of a dependency, and an interface member (no body) is never
//! demanded, so a dependency interface's `declaration_only_routines` is empty
//! there; d19 reads `ModelObject::interface_members` (all of them) instead. The
//! control `Other` (no interface declares it) is still flagged, so a dependency
//! interface does not silence its implementer's other methods. Found measuring
//! on CDO: System Application's "Default Email Connector v2" (`Send`,
//! `GetAccounts`) implements "Email Connector v2/v3/v4", which extend "Email
//! Connector", all System Application's.
//!
//! Mutation (2026-11-20): d19 reading the population-cut
//! `declaration_only_routines` instead of `interface_members` -> `Do` is
//! flagged (red); restored, green. (A first fix that called every dependency
//! interface's members unknown silenced `Other` too: this test caught it.)
//! Symbol-only case (review, Fable), each restored, green: d19 not reading the
//! interfaces' routines as members -> red (skip counts (0, 0)); `abi_rows`
//! dropping `interface_bases` -> red (the inherited `Do` not found).

use std::path::Path;

use al_sem::engine::gate::filter::Scope;
use al_sem::engine::gate::run::{AnalyzeArgs, OutputFormat, run_analyze_with_exit};

const DEP_GUID: &str = "15115115-0000-0000-0000-0000000000d1";
const D19: &str = "d19-unused-parameter";

const DEP_SOURCE: &str = "interface IDepBase\n{\n    procedure Do(X: Integer);\n}\n\ninterface IDepDer extends IDepBase\n{\n    procedure Extra();\n}\n";

const DEP_SYMBOLS: &str = r#"{"RuntimeVersion":"15.0","Interfaces":[{"Name":"IDepBase","Methods":[{"Name":"Do","Parameters":[{"Name":"X","TypeDefinition":{"Name":"Integer"}}]}]},{"Name":"IDepDer","ExtendedInterfaces":["IDepBase"],"Methods":[{"Name":"Extra","Parameters":[]}]}]}"#;

const WS_CODEUNIT: &str = "codeunit 50101 \"Ws Impl\" implements IDepDer\n{\n    procedure Do(X: Integer)\n    begin\n    end;\n\n    procedure Extra()\n    begin\n    end;\n\n    procedure Other(Y: Integer)\n    begin\n    end;\n}\n";

/// The workspace with the dependency shipping its source ([`DEP_SOURCE`]), or,
/// with `embed` false, symbols only (its members then reach the model as
/// routines, its bases through the ABI's `ExtendedInterfaces`).
fn workspace(root: &Path) -> std::path::PathBuf {
    workspace_with(root, true)
}

fn workspace_with(root: &Path, embed: bool) -> std::path::PathBuf {
    let ws = root.join("ws");
    std::fs::create_dir_all(ws.join("src")).unwrap();
    std::fs::write(
        ws.join("app.json"),
        format!(
            r#"{{"id":"15115115-0000-0000-0000-000000000001","name":"Ws151","publisher":"t","version":"1.0.0.0","runtime":"15.0",
            "dependencies":[{{"id":"{DEP_GUID}","name":"Dep151","publisher":"probe","version":"1.0.0.0"}}]}}"#
        ),
    )
    .unwrap();
    std::fs::write(ws.join("src/WsImpl.Codeunit.al"), WS_CODEUNIT).unwrap();
    crate::symbol_app::write_source_app(
        &ws.join(".alpackages/probe_Dep151_1.0.0.0.app"),
        DEP_GUID,
        "Dep151",
        "1.0.0.0",
        DEP_SYMBOLS,
        if embed {
            &[("src/DepIfaces.al", DEP_SOURCE)]
        } else {
            &[]
        },
        "",
    );
    ws
}

/// The routine names of the d19 findings, sorted.
fn d19_routines(ws: &Path) -> Vec<String> {
    let args = AnalyzeArgs {
        workspace: ws.to_string_lossy().to_string(),
        min_severity: None,
        detector: Some(D19.to_string()),
        preset: None,
        scope: Scope::Primary,
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
    let (out, _, _, _) =
        run_analyze_with_exit(&args.read_inputs(), &args.read_request(), &args, "151-test")
            .unwrap_or_else(|e| panic!("run_analyze failed: {e}"));
    let json: serde_json::Value = serde_json::from_str(&out).expect("analyze JSON");
    let mut v: Vec<String> = json["payload"]["findings"]
        .as_array()
        .expect("findings array")
        .iter()
        .filter(|f| f["detector"] == D19)
        .map(|f| {
            f["primaryLocation"]["routineName"]
                .as_str()
                .unwrap_or("")
                .to_string()
        })
        .collect();
    v.sort();
    let skip = |key: &str| {
        json["payload"]["summary"]["detectorStats"][0]["skipped"][key]
            .as_u64()
            .unwrap_or(0)
    };
    // Every case's interfaces are in the model: `Do` is a found member, never
    // an unknown-interface skip (which would pass the finding check alone).
    assert_eq!(
        (skip("interfaceMember"), skip("interfaceUnknown")),
        (1, 0),
        "skip counts"
    );
    v
}

/// `Do` implements the dependency's `IDepBase.Do` through `IDepDer`: not
/// flagged. `Other` implements nothing: flagged (the control).
#[test]
fn a_member_of_a_dependency_interface_is_not_flagged() {
    let dir = tempfile::tempdir().unwrap();
    let ws = workspace(dir.path());
    assert_eq!(d19_routines(&ws), ["Other"]);
}

/// The same with a SYMBOL-ONLY dependency (review, Fable): the members come
/// from the interfaces' routines, `IDepDer`'s base from the ABI's
/// `ExtendedInterfaces`.
#[test]
fn a_member_of_a_symbol_only_dependency_interface_is_not_flagged() {
    let dir = tempfile::tempdir().unwrap();
    let ws = workspace_with(dir.path(), false);
    assert_eq!(d19_routines(&ws), ["Other"]);
}

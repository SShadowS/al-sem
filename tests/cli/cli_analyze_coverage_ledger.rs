//! Engine-switch S5: what `alsem analyze` reports about its own coverage and the
//! app set (spec G9, G14). Preconditions are stated as files on disk and the
//! production path is run (`build_analysis_model` -> `analysis_coverage`).

use std::path::Path;

use al_sem::engine::gate::filter::Scope;
use al_sem::engine::gate::run::{
    AnalyzeArgs, OutputFormat, analysis_coverage, build_analysis_model, run_analyze_with_exit,
};

use crate::symbol_app::write_symbol_app;

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn app_json(guid: &str, name: &str) -> String {
    format!(
        r#"{{"id":"{guid}","name":"{name}","publisher":"probe","version":"1.0.0.0","runtime":"13.0","idRanges":[{{"from":50000,"to":59999}}]}}"#
    )
}

/// S5.1: a child directory with its own `app.json` is another app. The model
/// analyses the root app only, so coverage must not count the nested app's file
/// as a parsed source unit.
#[test]
fn coverage_counts_only_the_files_the_model_analysed() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    write(
        &ws.join("app.json"),
        &app_json("aaaa5555-0000-0000-0000-000000000051", "Root"),
    );
    write(
        &ws.join("src/Root.al"),
        "codeunit 50000 \"Root Cu\"\n{\n    procedure A()\n    begin\n    end;\n}\n",
    );
    write(
        &ws.join("nested/app.json"),
        &app_json("bbbb5555-0000-0000-0000-000000000051", "Nested"),
    );
    write(
        &ws.join("nested/src/Nested.al"),
        "codeunit 50001 \"Nested Cu\"\n{\n    procedure B()\n    begin\n    end;\n}\n",
    );

    let built = build_analysis_model(ws);
    let resolved = built.model.as_ref().expect("root app assembles");
    let names: Vec<&str> = resolved
        .workspace
        .routines
        .iter()
        .map(|r| r.name.as_str())
        .collect();
    assert_eq!(names, vec!["A"], "the model analyses the root app only");

    let c = analysis_coverage(resolved, ws, &built.fresh);
    assert_eq!(
        (c.source_units_total, c.source_units_parsed),
        (1, 1),
        "nested/src/Nested.al is not a unit of this analysis"
    );
}

const OLD: &str = "cccc5555-0000-0000-0000-000000000001";
const MISSING: &str = "cccc5555-0000-0000-0000-000000000002";
const BROKEN: &str = "cccc5555-0000-0000-0000-000000000003";

/// A workspace whose three declared dependencies are: present but older than
/// declared, absent, and present with an unreadable `SymbolReference.json`.
fn ledger_workspace(ws: &Path) {
    let dep = |guid: &str, name: &str, version: &str| {
        format!(r#"{{"id":"{guid}","name":"{name}","publisher":"probe","version":"{version}"}}"#)
    };
    write(
        &ws.join("app.json"),
        &format!(
            r#"{{"id":"aaaa5555-0000-0000-0000-000000000052","name":"LedgerWs","publisher":"probe","version":"1.0.0.0","runtime":"13.0","dependencies":[{},{},{}]}}"#,
            dep(OLD, "LedgerOld", "2.0.0.0"),
            dep(MISSING, "LedgerMissing", "1.0.0.0"),
            dep(BROKEN, "LedgerBroken", "1.0.0.0"),
        ),
    );
    write(
        &ws.join("src/Cu.al"),
        "codeunit 50002 \"Ledger Cu\"\n{\n    procedure A()\n    begin\n    end;\n}\n",
    );
    let empty = |guid: &str, name: &str| {
        format!(
            r#"{{"RuntimeVersion":"13.0","AppId":"{guid}","Name":"{name}","Publisher":"probe","Version":"1.0.0.0"}}"#
        )
    };
    write_symbol_app(
        &ws.join(".alpackages/probe_LedgerOld_1.0.0.0.app"),
        OLD,
        "LedgerOld",
        "1.0.0.0",
        &empty(OLD, "LedgerOld"),
    );
    write_symbol_app(
        &ws.join(".alpackages/probe_LedgerBroken_1.0.0.0.app"),
        BROKEN,
        "LedgerBroken",
        "1.0.0.0",
        "{ this is not JSON",
    );
}

/// S5.2 (G14): the ledger lists every declared dependency with what was found.
#[test]
fn ledger_records_missing_older_and_unreadable_dependencies() {
    let dir = tempfile::tempdir().unwrap();
    ledger_workspace(dir.path());
    let built = build_analysis_model(dir.path());
    let fc = built.fresh.as_ref().expect("program builds");
    // (guid, found, below declared version, unreadable on disk)
    let rows: Vec<(&str, bool, bool, bool)> = fc
        .ledger
        .iter()
        .map(|e| {
            (
                e.guid.as_str(),
                e.found.is_some(),
                e.below_declared_version(),
                e.unreadable.is_some(),
            )
        })
        .collect();
    assert_eq!(
        rows,
        vec![
            (OLD, true, true, false),
            (MISSING, false, false, false),
            (BROKEN, false, false, true),
        ],
        "the corrupt package is unreadable, not absent"
    );
}

fn analyze_args(ws: &Path) -> AnalyzeArgs {
    AnalyzeArgs {
        workspace: ws.to_string_lossy().to_string(),
        min_severity: None,
        detector: None,
        preset: None,
        scope: Scope::Primary,
        limit: None,
        format: OutputFormat::Json,
        sarif_version_override: None,
        fail_on: None,
        require_dependencies: true,
        baseline: None,
        update_baseline: false,
        disable_inline_suppression: true,
        group_by: None,
        deterministic: true,
        with_evidence: false,
    }
}

/// S5.2b: through `alsem analyze`, an unreadable declared dependency degrades the
/// preflight (and fails it under `--require-dependencies`, exit 4); the missing
/// and the older dependency are reported as `dependencies` diagnostics.
#[test]
fn analyze_degrades_on_an_unreadable_dependency_and_reports_the_others() {
    let dir = tempfile::tempdir().unwrap();
    ledger_workspace(dir.path());
    let (out, exit, warning) =
        run_analyze_with_exit(&analyze_args(dir.path()), "test").expect("analyze runs");
    let warning = warning.expect("degraded");
    assert!(
        warning.contains("1 unreadable dependency app(s): LedgerBroken"),
        "{warning}"
    );
    assert_eq!(exit, 4, "--require-dependencies fails a degraded preflight");

    let v: serde_json::Value = serde_json::from_str(&out).expect("json");
    let deps: Vec<String> = v["diagnostics"]
        .as_array()
        .expect("diagnostics")
        .iter()
        .filter(|d| d["code"] == "DIAG-dependencies")
        .map(|d| d["message"].as_str().unwrap().to_string())
        .collect();
    assert!(
        deps.iter()
            .any(|m| m.contains("LedgerMissing") && m.contains("missing")),
        "{deps:?}"
    );
    assert!(
        deps.iter()
            .any(|m| m.contains("LedgerOld") && m.contains("2.0.0.0")),
        "{deps:?}"
    );
}

/// S5.2b: a workspace subscriber to an object that does not exist binds to
/// nothing; no call edge is unknown, yet the preflight must not say "verified".
#[test]
fn analyze_degrades_on_an_unbound_event_subscription() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    write(
        &ws.join("app.json"),
        &app_json("aaaa5555-0000-0000-0000-000000000053", "SubWs"),
    );
    write(
        &ws.join("src/Sub.al"),
        r#"codeunit 50003 "Sub Cu"
{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::"No Such Codeunit", 'OnAfterX', '', false, false)]
    local procedure Handle()
    begin
    end;
}
"#,
    );
    let (_, _, warning) = run_analyze_with_exit(&analyze_args(ws), "test").expect("analyze runs");
    let warning = warning.expect("degraded");
    assert!(
        warning.contains("1 unbound event subscription(s)"),
        "{warning}"
    );
}

/// S5.3: `--scope primary` drops a finding whose object belongs to another app.
/// The analyze model holds the primary app only until S7/S8, so the precondition
/// is stated by assignment: one object of a real model is moved to another app.
/// LIMIT: this pins `dependency_object_predicate` and `scope_filter`, not the one
/// line in `run_analyze_with_exit` that passes the one to the other.
#[test]
fn scope_primary_drops_a_finding_on_another_apps_object() {
    use al_sem::engine::gate::filter::scope_filter;
    use al_sem::engine::gate::projection::{FindingLocation, FindingSummary, ProjectionIndex};
    use al_sem::engine::gate::run::dependency_object_predicate;

    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    write(
        &ws.join("app.json"),
        &app_json("aaaa5555-0000-0000-0000-000000000054", "ScopeWs"),
    );
    write(
        &ws.join("src/Two.al"),
        "codeunit 50004 \"Own Cu\"\n{\n    procedure A()\n    begin\n    end;\n}\n\
         codeunit 50005 \"Dep Cu\"\n{\n    procedure B()\n    begin\n    end;\n}\n",
    );
    let mut resolved = build_analysis_model(ws).model.expect("model");
    let dep = resolved
        .workspace
        .objects
        .iter_mut()
        .find(|o| o.name == "Dep Cu")
        .expect("Dep Cu");
    dep.app_guid = "dddd5555-0000-0000-0000-000000000054".to_string();
    let dep_id = dep.id.clone();
    let own_id = resolved
        .workspace
        .objects
        .iter()
        .find(|o| o.name == "Own Cu")
        .expect("Own Cu")
        .id
        .clone();

    let idx = ProjectionIndex::build(&resolved.workspace.objects, &resolved.workspace.routines);
    let finding = |id: &str, object: &str| FindingSummary {
        id: id.to_string(),
        fingerprint: "fp".to_string(),
        detector: "d".to_string(),
        title: "t".to_string(),
        root_cause: "rc".to_string(),
        severity: "low".to_string(),
        confidence_level: "confirmed".to_string(),
        confidence_capped_by: None,
        primary_location: FindingLocation {
            file: "ws:src/Two.al".to_string(),
            line: 1,
            column: 1,
            object_id: Some(object.to_string()),
            object_name: None,
            routine_id: None,
            routine_name: None,
        },
        terminal_location: None,
        affected_objects: vec![],
        affected_tables: vec![],
        fix_hint: None,
        path_count: 0,
    };
    let kept: Vec<String> = scope_filter(
        vec![finding("own", &own_id), finding("dep", &dep_id)],
        Scope::Primary,
        dependency_object_predicate(&resolved, &idx),
    )
    .into_iter()
    .map(|f| f.id)
    .collect();
    assert_eq!(kept, vec!["own".to_string()]);
}

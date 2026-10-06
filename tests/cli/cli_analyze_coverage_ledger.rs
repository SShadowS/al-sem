//! Engine-switch S5: what `alsem analyze` reports about its own coverage and the
//! app set (spec G9, G14). Preconditions are stated as files on disk and the
//! production path is run (`build_analysis_model` -> `analysis_coverage`).

use std::path::Path;

use al_sem::engine::gate::run::{analysis_coverage, build_analysis_model};

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

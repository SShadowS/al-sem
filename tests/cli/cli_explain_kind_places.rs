//! Engine limits wave 0 (B8): explain's kind lines (`errors at N places`,
//! `background tasks at N places`, …) name the dependency routines a group of
//! places enters ONCE, and list where they are entered, keeping every place
//! and every count. On CDO `CDO Update Handled Sales::SetHandled` (an event
//! with five subscribers raised at 10 `ModifyAll` sites) the lines repeated
//! the five routine names once per site, about 5 KB, and never showed the
//! sites.
//!
//! The shape: `Grp Caller.Run` calls the dependency's `Grp Dep.Make` at three
//! call sites (lines 7, 8 and 9); `Make`'s `T.Insert()` is an implicit error.
//! Each call site is its own dependency crossing with the same target.

use std::path::Path;
use std::process::Command;

const DEP_GUID: &str = "b0d00003-0000-4000-8000-0000000000c1";

const DEP_SYMBOLS: &str = r#"{"RuntimeVersion":"13.0","Codeunits":[{"Id":50600,"Name":"Grp Dep","Methods":[{"Name":"Make","Parameters":[]}]}],"Tables":[{"Id":50601,"Name":"Grp Dep Rec","Fields":[{"Id":1,"Name":"Code","TypeDefinition":{"Name":"Code"}}]}],"AppId":"b0d00003-0000-4000-8000-0000000000c1","Name":"GrpDep","Publisher":"probe","Version":"1.0.0.0"}"#;

const DEP_SOURCE: &str = r#"codeunit 50600 "Grp Dep"
{
    procedure Make()
    var
        T: Record "Grp Dep Rec";
    begin
        T.Init();
        T.Insert();
    end;
}
"#;

const DEP_TABLE: &str = r#"table 50601 "Grp Dep Rec"
{
    fields
    {
        field(1; Code; Code[20]) { }
    }
    keys
    {
        key(PK; Code) { Clustered = true; }
    }
}
"#;

/// Lines 7, 8 and 9 are the three calls into the dependency.
const WS_SOURCE: &str = r#"codeunit 50610 "Grp Caller"
{
    procedure Run()
    var
        Dep: Codeunit "Grp Dep";
    begin
        Dep.Make();
        Dep.Make();
        Dep.Make();
    end;
}
"#;

fn workspace(root: &Path) -> std::path::PathBuf {
    let ws = root.join("ws");
    std::fs::create_dir_all(ws.join("src")).unwrap();
    std::fs::write(
        ws.join("app.json"),
        format!(
            r#"{{"id":"b0d00004-0000-4000-8000-0000000000c2","name":"Grp","publisher":"t","version":"1.0.0.0",
            "dependencies":[{{"id":"{DEP_GUID}","name":"GrpDep","publisher":"probe","version":"1.0.0.0"}}]}}"#
        ),
    )
    .unwrap();
    std::fs::write(ws.join("src/Grp.Codeunit.al"), WS_SOURCE).unwrap();
    crate::symbol_app::write_source_app(
        &ws.join(".alpackages/probe_GrpDep_1.0.0.0.app"),
        DEP_GUID,
        "GrpDep",
        "1.0.0.0",
        DEP_SYMBOLS,
        &[
            ("src/GrpDep.Codeunit.al", DEP_SOURCE),
            ("src/GrpDepRec.Table.al", DEP_TABLE),
        ],
        "",
    );
    ws
}

fn explain(ws: &Path, routine: &str) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .arg("explain")
        .arg(ws)
        .args(["--routine", routine])
        .env("RUST_LOG", "off")
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// Three crossings into the same dependency routine print as one group:
/// the routine named once, the three places counted and each call site
/// listed; the line's own count stays 3.
///
/// DISCRIMINATION (2026-11-09): `crossing_places` opening a new group for
/// every crossing (the old shape) prints `ERROR_IMPLICIT beyond Codeunit
/// "Grp Dep" (50600)::Make` three times with no site and fails at the
/// errors line; restored, passes.
#[test]
fn explain_names_a_dependency_routine_entered_at_many_places_once() {
    let lines: Vec<&str> = WS_SOURCE.lines().collect();
    for l in [6, 7, 8] {
        assert_eq!(
            lines[l].trim(),
            "Dep.Make();",
            "precondition: line {}",
            l + 1
        );
    }
    let tmp = tempfile::tempdir().unwrap();
    let ws = workspace(tmp.path());
    let text = explain(&ws, "Codeunit \"Grp Caller\"::Run");
    let errors = text
        .lines()
        .find(|l| l.trim_start().starts_with("errors at "))
        .unwrap_or_else(|| panic!("an errors line: {text}"));
    assert_eq!(
        errors.trim(),
        "errors at 3 places: ERROR_IMPLICIT beyond Codeunit \"Grp Dep\" (50600)::Make (3 places, \
         entered at ws:src/Grp.Codeunit.al lines 7, 8, 9)",
        "{text}"
    );
    assert_eq!(errors.matches("::Make").count(), 1, "named once: {errors}");
}

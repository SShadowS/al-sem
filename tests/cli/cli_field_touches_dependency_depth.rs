//! ENG-213: `query touches --field F --from R --direction down` lists the field
//! sites of R's whole down cone, dependency code past its first routine
//! included. The answer's scope was the solve's combined graph, which takes
//! workspace callers only, so a site in a dependency routine reached through
//! another dependency routine was never searched and the answer read
//! "consumers: none observed".
//!
//! Workspace `Q Writer.CallDep` calls dependency `D Writer.WriteIt`, which calls
//! its local `Inner`, which assigns `D Tab.Name`. The one-hop control:
//! `CallDirect` calls `D Writer.Direct`, which assigns it itself.

use std::process::Command;

const DEP_GUID: &str = "21321321-0000-4000-8000-0000000000d1";
const DEP: &str = r#"table 50900 "D Tab"
{
    fields { field(1; Code; Code[20]) { } field(2; Name; Text[30]) { } }
    keys { key(PK; Code) { } }
}
codeunit 50901 "D Writer"
{
    procedure WriteIt()
    begin
        Inner();
    end;

    procedure Direct()
    var
        T: Record "D Tab";
    begin
        T.Name := 'y';
        T.Modify();
    end;

    local procedure Inner()
    var
        T: Record "D Tab";
    begin
        T.Name := 'x';
        T.Modify();
    end;
}
"#;
const SYMBOLS: &str = r#"{"RuntimeVersion":"15.0","Tables":[{"Id":50900,"Name":"D Tab","Fields":[{"Id":1,"Name":"Code","TypeDefinition":{"Name":"Code"}},{"Id":2,"Name":"Name","TypeDefinition":{"Name":"Text"}}]}],"Codeunits":[{"Id":50901,"Name":"D Writer","Methods":[{"Name":"WriteIt","Parameters":[]},{"Name":"Direct","Parameters":[]}]}]}"#;
const WS: &str = r#"codeunit 50951 "Q Writer"
{
    procedure CallDep()
    var
        W: Codeunit "D Writer";
    begin
        W.WriteIt();
    end;

    procedure CallDirect()
    var
        W: Codeunit "D Writer";
    begin
        W.Direct();
    end;
}
"#;

fn workspace(root: &std::path::Path) -> std::path::PathBuf {
    let ws = root.join("ws");
    std::fs::create_dir_all(ws.join("src")).unwrap();
    std::fs::write(
        ws.join("app.json"),
        format!(
            r#"{{"id":"21321321-0000-4000-8000-000000000213","name":"Q","publisher":"t","version":"1.0.0.0","runtime":"15.0",
            "dependencies":[{{"id":"{DEP_GUID}","name":"D","publisher":"probe","version":"1.0.0.0"}}]}}"#
        ),
    )
    .unwrap();
    std::fs::write(ws.join("src/q.al"), WS).unwrap();
    crate::symbol_app::write_source_app(
        &ws.join(".alpackages/probe_D_1.0.0.0.app"),
        DEP_GUID,
        "D",
        "1.0.0.0",
        SYMBOLS,
        &[("src/d.al", DEP)],
        "",
    );
    ws
}

/// The field answer's JSON `fieldTouches[]` routines, from `from`, down.
fn field_routines(ws: &str, from: &str) -> Vec<String> {
    let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .args([
            "query",
            "touches",
            ws,
            "--table",
            "D Tab",
            "--field",
            "Name",
            "--from",
            from,
            "--direction",
            "down",
            "--format",
            "json",
        ])
        .env("RUST_LOG", "off")
        .output()
        .expect("spawn alsem");
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "{from}: not JSON ({e}): {}",
            String::from_utf8_lossy(&out.stdout)
        )
    });
    // A row names its routine by reference into `payload.routines`.
    v["payload"]["fieldTouches"]
        .as_array()
        .unwrap_or_else(|| panic!("{from}: fieldTouches: {v}"))
        .iter()
        .map(|t| {
            let r = &t["routine"];
            r.as_str()
                .and_then(|k| v["payload"]["routines"][k]["display"].as_str())
                .map_or_else(|| r.to_string(), str::to_string)
        })
        .collect()
}

#[test]
fn a_field_site_two_dependency_calls_down_is_listed() {
    let dir = tempfile::tempdir().unwrap();
    let ws = workspace(dir.path());
    let w = ws.to_str().unwrap();
    let direct = field_routines(w, "CallDirect");
    assert!(
        direct.iter().any(|r| r.contains("Direct")),
        "control (one dependency call down): {direct:?}"
    );
    let deep = field_routines(w, "CallDep");
    assert!(
        deep.iter().any(|r| r.contains("Inner")),
        "the site in Inner (CallDep -> WriteIt -> Inner): {deep:?}"
    );
}

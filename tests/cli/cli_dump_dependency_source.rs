//! `aldump --dump-dependency-source <ws> <out>` writes the AL the engine reads
//! from inside each dependency `.app` to `<out>/<app guid>/<virtual path>`, the
//! path a dependency citation `dep:<guid>:<path>` names, with the same text.
//! Asked by the CONtinAI pipeline, so citations into Continia Core's
//! synthesized AL resolve to a file and line.

use std::process::Command;

const DEP_GUID: &str = "43434343-0000-4000-8000-000000000043";

const DEP_SOURCE: &str =
    "codeunit 50460 \"Dump Dep\"\n{\n    procedure P()\n    begin\n    end;\n}\n";

#[test]
fn embedded_dependency_source_is_written_at_its_citation_path() {
    let t = tempfile::tempdir().unwrap();
    let ws = t.path().join("ws");
    std::fs::create_dir_all(ws.join("src")).unwrap();
    std::fs::write(
        ws.join("app.json"),
        format!(
            r#"{{"id":"44444444-0000-4000-8000-000000000044","name":"W","publisher":"t","version":"1.0.0.0",
            "dependencies":[{{"id":"{DEP_GUID}","name":"DumpDep","publisher":"probe","version":"1.0.0.0"}}]}}"#
        ),
    )
    .unwrap();
    std::fs::write(ws.join("src/W.al"), "codeunit 50461 \"Dump W\" { }\n").unwrap();
    crate::symbol_app::write_source_app(
        &ws.join(".alpackages/probe_DumpDep_1.0.0.0.app"),
        DEP_GUID,
        "DumpDep",
        "1.0.0.0",
        r#"{"Codeunits":[{"Id":50460,"Name":"Dump Dep","Methods":[{"Name":"P"}]}]}"#,
        &[("src/Sub/Dep.Codeunit.al", DEP_SOURCE)],
        "",
    );
    let out_dir = t.path().join("out");
    let out = Command::new(env!("CARGO_BIN_EXE_aldump"))
        .arg("--dump-dependency-source")
        .arg(&ws)
        .arg(&out_dir)
        .env("RUST_LOG", "off")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "aldump failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let written = out_dir.join(DEP_GUID).join("src/Sub/Dep.Codeunit.al");
    assert_eq!(
        std::fs::read_to_string(&written).unwrap_or_else(|e| panic!("{}: {e}", written.display())),
        DEP_SOURCE
    );
    // The workspace's own source is not written: it is already a file.
    let dirs: Vec<_> = std::fs::read_dir(&out_dir)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(dirs, [std::ffi::OsString::from(DEP_GUID)]);
}

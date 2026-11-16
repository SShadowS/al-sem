//! ENG-93: `aldump --session-refresh` edits a CRLF workspace (CDO's line ending) for real,
//! and fails instead of reporting a step it did not exercise.
//!
//! Until ENG-93 the `body` edit looked for `"\n    begin\n"`, which a CRLF file never
//! contains: on CDO it changed nothing, the step refreshed an unchanged workspace, and
//! `--verify` reported it equal to a cold build. The body-only refresh path was never
//! exercised there.

use std::path::Path;
use std::process::{Command, Output};

const APP_JSON: &str = r#"{"id":"93939393-0000-4000-8000-000000000093","name":"SR","publisher":"t","version":"1.0.0.0"}"#;

fn crlf(s: &str) -> String {
    s.replace('\n', "\r\n")
}

fn workspace(dir: &Path, file: &str, source: &str) {
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(dir.join("app.json"), APP_JSON).unwrap();
    std::fs::write(dir.join("src").join(file), source).unwrap();
}

fn refresh(ws: &Path, extra: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_aldump"))
        .arg("--session-refresh")
        .arg(ws)
        .args(extra)
        .env("RUST_LOG", "off")
        .output()
        .unwrap()
}

fn rows(out: &Output) -> Vec<serde_json::Value> {
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| serde_json::from_str(l).expect("one JSON row per step"))
        .collect()
}

/// Every step of a CRLF codeunit (found without `--file`) reaches the session: the
/// `body` step changes one file, `noop` none, and no row carries a `noOpError`.
#[test]
fn every_step_edits_a_crlf_file() {
    let t = tempfile::tempdir().unwrap();
    let ws = t.path().join("ws");
    let source = crlf(
        "codeunit 50930 \"SR Lib\"\n{\n    procedure Run()\n    begin\n        Helper();\n    end;\n\n    procedure Helper()\n    begin\n    end;\n}\n",
    );
    workspace(&ws, "Lib.Codeunit.al", &source);

    let out = refresh(&ws, &[]);
    assert!(
        out.status.success(),
        "the harness failed on a CRLF file: {}\n{}",
        String::from_utf8_lossy(&out.stderr),
        String::from_utf8_lossy(&out.stdout)
    );
    let rows = rows(&out);
    let changed = |step: &str| {
        rows.iter()
            .find(|r| r["step"] == step)
            .unwrap_or_else(|| panic!("no {step} row in {rows:?}"))["changed"]
            .as_u64()
            .unwrap()
    };
    assert_eq!(changed("noop"), 0, "{rows:?}");
    for step in ["body", "body-restore", "newline", "decl", "signature"] {
        assert_eq!(
            changed(step),
            1,
            "{step} must change the CRLF file: {rows:?}"
        );
    }
    assert!(
        rows.iter().all(|r| r.get("noOpError").is_none()),
        "{rows:?}"
    );
    // The harness restores what it edited.
    assert_eq!(
        std::fs::read_to_string(ws.join("src/Lib.Codeunit.al")).unwrap(),
        source
    );
}

/// A file the `body` edit cannot change (no procedure body) fails the run, naming the
/// edit, and is left as it was.
#[test]
fn an_edit_that_changes_nothing_fails_the_run() {
    let t = tempfile::tempdir().unwrap();
    let ws = t.path().join("ws");
    let source = crlf("codeunit 50931 \"SR Empty\"\n{\n}\n");
    workspace(&ws, "Empty.Codeunit.al", &source);

    let out = refresh(&ws, &["--file", "src/Empty.Codeunit.al"]);
    assert!(!out.status.success(), "a no-op body edit must fail the run");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("the body edit changes nothing"),
        "stderr: {stderr}"
    );
    assert_eq!(
        std::fs::read_to_string(ws.join("src/Empty.Codeunit.al")).unwrap(),
        source
    );
}

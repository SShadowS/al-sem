//! `alsem_tests` through the MCP server (agent-tasks design §4.6-4.7, C-7,
//! C-10's revision vector): its stdout is `alsem tests`' bytes, it runs as a
//! composite request over the production engine and one test-universe engine
//! per test app, and explain never starts a test engine.
//!
//! Fixture: `tests/fixtures/test-universe`, copied into a temporary folder
//! with a `.git` marker (as `cli_tests_selection` does).

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{Value, json};

use crate::mcp_server::{Server, cli_bytes, envelope};

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap() {
        let e = e.unwrap();
        let t = to.join(e.file_name());
        if e.file_type().unwrap().is_dir() {
            copy_dir(&e.path(), &t);
        } else {
            std::fs::copy(e.path(), &t).unwrap();
        }
    }
}

fn repo() -> tempfile::TempDir {
    let t = tempfile::tempdir().unwrap();
    copy_dir(
        &PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/test-universe"),
        t.path(),
    );
    std::fs::create_dir(t.path().join(".git")).unwrap();
    t
}

fn cli(ws: &Path, args: &[&str]) -> (Vec<u8>, u8) {
    let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .arg("tests")
        .args(args)
        .arg("--")
        .arg(ws)
        .output()
        .unwrap();
    (out.stdout, out.status.code().unwrap_or(255) as u8)
}

fn engines(s: &mut Server, id: i64) -> Vec<Value> {
    let r = s.call(id, "alsem_status", json!({}));
    let body: Value = serde_json::from_str(r["content"][0]["text"].as_str().unwrap()).unwrap();
    body["engines"].as_array().cloned().unwrap_or_default()
}

/// The byte contract and the composite: `alsem_tests` answers exactly what
/// `alsem tests` prints (human and JSON); the envelope's `revisions` names the
/// production revision and the test universe's (`tests:<key>`); after it,
/// status lists a test engine. Before it, an explain call starts none (C-7).
///
/// DISCRIMINATION (2026-11-05): starting the select part on the production
/// engine (the `Target::Tests` arm keyed like `Target::Production`) is
/// refused by that engine ("workspace '…/Test' is not this session's"): the
/// answer is `test-app-not-analyzed` and the byte comparison fails;
/// reverted, passes.
#[test]
fn alsem_tests_answers_the_clis_bytes_through_two_engines() {
    let t = repo();
    let ws = t.path().join("App");
    let mut s = Server::ready(&ws, None);
    let r = s.call(1, "alsem_explain", json!({"routines": ["TU Core::Post"]}));
    let human = envelope(&r)["stdoutText"].as_str().unwrap().to_string();
    assert!(human.contains("1 test app discovered (TU Production Tests); not analyzed"));
    let before = engines(&mut s, 2);
    assert_eq!(before.len(), 1, "explain starts no test engine: {before:?}");

    for (i, (args, cli_args)) in [
        (
            json!({"routines": ["TU Core::Post"]}),
            vec!["--routine=TU Core::Post", "--tier=local", "--format=human"],
        ),
        (
            json!({"routines": ["TU Core::Audited"], "format": "json"}),
            vec![
                "--routine=TU Core::Audited",
                "--tier=local",
                "--format=json",
            ],
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let r = s.call(10 + i as i64, "alsem_tests", args);
        let env = envelope(&r);
        let (stdout, _, exit) = cli_bytes(&env);
        let (want, want_exit) = cli(&ws, &cli_args);
        assert_eq!(
            String::from_utf8_lossy(&stdout),
            String::from_utf8_lossy(&want),
            "the CLI's bytes"
        );
        assert_eq!(exit, want_exit);
        let revs = &env["revisions"];
        assert!(revs["production"]["revision"].is_object(), "{env}");
        assert_eq!(revs["tests"].as_array().unwrap().len(), 1, "{env}");
        assert!(
            revs["tests"][0]["universe"]
                .as_str()
                .unwrap()
                .starts_with("tests:"),
            "{env}"
        );
    }
    let after = engines(&mut s, 20);
    assert_eq!(
        after.len(),
        2,
        "the production and the test engine: {after:?}"
    );
    assert!(s.finish().success());
}

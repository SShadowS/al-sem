//! `alsem_impact` through the MCP server (agent-tasks design §8.4, B-8): its
//! stdout is `alsem impact`'s bytes but for the comparison tuple's inputs
//! stamp (a resident revision has one, a one-shot CLI run none), it runs as
//! a composite request (the production engine for the main and up parts, a
//! test engine for the select part), and an answer over the inline cap goes
//! to a file with its summary.
//!
//! Fixture: `tests/fixtures/test-universe`, copied into a temporary folder
//! with a `.git` marker (as `cli_impact` does).

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

fn cli(ws: &Path, args: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .arg("impact")
        .arg(ws)
        .args(args)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    String::from_utf8(out.stdout).unwrap()
}

/// The text with the run clause of the comparison line made equal: the one
/// place a resident answer differs from a one-shot run.
fn without_run(s: &str) -> String {
    s.replace("one-shot run (no resident revision)", "<run>")
        .lines()
        .map(|l| match l.find("; revision al-sem-manifest-v1:") {
            Some(i) => format!("{}; <run>", &l[..i]),
            None => l.to_string(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The byte contract (but for the inputs stamp) in human and JSON; the JSON
/// tuple's `head.inputsStamp` is the revision the envelope names; the
/// composite used a test engine; `tests: false` starts no test part.
///
/// DISCRIMINATION (2026-11-05): not handing the main part's revision to the
/// tuple (`orchestrate_with(…, &|| None)` in `Pool::run_impact`) prints
/// "one-shot run" in the MCP answer: fails at the comparison line's
/// `revision al-sem-manifest-v1:` assertion (before the JSON's
/// `head.inputsStamp` one); restored, passes.
#[test]
fn alsem_impact_answers_the_clis_bytes() {
    let t = repo();
    let ws = t.path().join("App");
    let mut s = Server::ready(&ws, None);
    let r = s.call(1, "alsem_impact", json!({"routines": ["TU Core::Post"]}));
    let env = envelope(&r);
    let (stdout, _, exit) = cli_bytes(&env);
    assert_eq!(exit, 0);
    let mcp = String::from_utf8(stdout).unwrap();
    let want = cli(&ws, &["--routine", "TU Core::Post"]);
    assert_eq!(without_run(&mcp), without_run(&want), "the CLI's bytes");
    assert!(
        mcp.lines()
            .next()
            .unwrap()
            .contains("revision al-sem-manifest-v1:"),
        "{mcp}"
    );
    assert!(
        env["revisions"]["production"]["revision"].is_object(),
        "{env}"
    );
    let parts = env["revisions"]["parts"].as_array().unwrap();
    assert!(
        parts
            .iter()
            .any(|p| p["universe"].as_str().unwrap().starts_with("tests:")),
        "a test engine answered a part: {env}"
    );

    let r = s.call(
        2,
        "alsem_impact",
        json!({"routines": ["TU Core::Calc"], "format": "json", "tests": false}),
    );
    let env = envelope(&r);
    let (stdout, _, _) = cli_bytes(&env);
    let mut mcp: Value = serde_json::from_slice(&stdout).unwrap();
    let stamp = env["revision"]["inputsStamp"].clone();
    assert!(stamp.is_string(), "{env}");
    assert_eq!(
        mcp["comparison"]["head"]["inputsStamp"], stamp,
        "the revision's stamp"
    );
    mcp["comparison"]["head"]["inputsStamp"] = Value::Null;
    let want: Value = serde_json::from_str(&cli(
        &ws,
        &[
            "--routine",
            "TU Core::Calc",
            "--format",
            "json",
            "--no-tests",
        ],
    ))
    .unwrap();
    assert_eq!(mcp, want);
    assert_eq!(
        env["revisions"]["parts"].as_array().unwrap().len(),
        2,
        "main and up only"
    );
    assert!(s.finish().success());
}

/// B-8's mechanism on the fixture: an answer over the inline cap goes to a
/// file, and the envelope carries the summary (the comparison, the
/// accounting line, the first routines, the tests line) with the full
/// counts.
#[test]
fn an_answer_over_the_cap_goes_to_a_file_with_its_summary() {
    let t = repo();
    let ws = t.path().join("App");
    let mut s = Server::ready(&ws, None);
    let r = s.call(
        1,
        "alsem_impact",
        json!({"routines": ["TU Core::Post", "TU Core::Calc"], "maxInlineBytes": 200}),
    );
    let env = envelope(&r);
    assert!(env.get("stdoutFile").is_some(), "{env}");
    let summary = env["summary"]["text"].as_str().unwrap();
    assert!(
        summary.starts_with("impact of the named routines"),
        "{summary}"
    );
    assert!(
        summary.contains("input: 2 changed routines resolved"),
        "{summary}"
    );
    assert!(summary.contains("tests: "), "{summary}");
    let (stdout, _, _) = cli_bytes(&env);
    assert!(stdout.len() > 200);
    assert!(s.finish().success());
}

/// Spec §12.1's refusals reach the client as invalid arguments.
#[test]
fn refused_combinations() {
    let t = repo();
    let ws = t.path().join("App");
    let mut s = Server::ready(&ws, None);
    for (i, args) in [
        json!({"base": "HEAD", "diff": "x", "delta": false}),
        json!({"routines": ["X"], "delta": true}),
        json!({"routines": ["X"], "files": ["y.al"]}),
        json!({"routines": ["X"], "diffRoot": "."}),
    ]
    .into_iter()
    .enumerate()
    {
        let r = s.request(
            json!(i),
            "tools/call",
            json!({"name": "alsem_impact", "arguments": args}),
        );
        let text = r.to_string();
        assert!(
            text.contains("'delta: false' drops it")
                || text.contains("'delta' goes with 'base'")
                || text.contains("exactly one of")
                || text.contains("goes with 'diff'"),
            "{text}"
        );
    }
    assert!(s.finish().success());
}

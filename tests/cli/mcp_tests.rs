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

/// Milestone C item 3, the `test-config` freshness category: after an
/// `alsem_tests` answer, a change of `.al-sem.json` `testApps` shows in
/// `alsem_status` on the production engine (category `test-config`, the
/// headline's "Test scope changed"), while the engine itself stays fresh
/// (it is no input of its revision). The next `alsem_tests` rediscovers (the
/// scope is off: no test universe is asked), the status agrees again, and
/// the test engine the earlier discovery started is retired with its
/// descriptor (the cached discovery is invalidated).
///
/// DISCRIMINATION (2026-11-05): (a) not recording the scope
/// (`record_test_scope` returning at its start) fails at the first status
/// call (no `testScope` on the check: `null` where `false` is due); (c)
/// `apply_test_config` never seeing a change (`changed` forced false) fails
/// at "the testApps change shows"; (b)
/// skipping the retire (`dropped` forced empty) fails at "the out-of-scope
/// test engine is retired"; each restored, passes.
#[test]
fn a_test_apps_change_shows_in_status_and_is_rediscovered() {
    let t = repo();
    let ws = t.path().join("App");
    let mut s = Server::ready(&ws, None);
    let production = |list: &[Value]| -> Value {
        list.iter()
            .find(|e| e["universe"] != "tests")
            .cloned()
            .unwrap_or_else(|| panic!("a production engine: {list:?}"))
    };
    let env = envelope(&s.call(1, "alsem_tests", json!({"routines": ["TU Core::Post"]})));
    assert_eq!(
        env["revisions"]["tests"].as_array().map(Vec::len),
        Some(1),
        "{env}"
    );
    let p = production(&engines(&mut s, 2));
    assert_eq!(p["check"]["testScope"]["changed"], false, "{p}");
    assert_eq!(p["check"]["fresh"], true, "{p}");

    std::fs::write(ws.join(".al-sem.json"), r#"{"testApps": []}"#).unwrap();
    let p = production(&engines(&mut s, 3));
    let check = &p["check"];
    assert_eq!(
        check["testScope"]["changed"], true,
        "the testApps change shows: {p}"
    );
    assert_eq!(check["testScope"]["now"], json!([]), "{p}");
    assert!(
        check["categories"]
            .as_array()
            .is_some_and(|c| c.contains(&json!("test-config"))),
        "{p}"
    );
    assert_eq!(
        check["fresh"], true,
        "the engine's own inputs did not move: {p}"
    );
    assert!(
        check["refreshClass"]["class"].is_null(),
        "no refresh of this engine: {p}"
    );
    assert!(
        p["headline"]
            .as_str()
            .is_some_and(|h| h.contains("Test scope changed")),
        "{p}"
    );

    let env = envelope(&s.call(4, "alsem_tests", json!({"routines": ["TU Core::Post"]})));
    assert_eq!(
        env["revisions"]["tests"],
        json!([]),
        "rediscovered: off: {env}"
    );
    let list = engines(&mut s, 5);
    let p = production(&list);
    assert_eq!(p["check"]["testScope"]["changed"], false, "{p}");
    let test_engine = list
        .iter()
        .find(|e| e["universe"] == "tests")
        .expect("the test worker is still listed");
    assert!(
        test_engine["details"]["engine"].is_null(),
        "the out-of-scope test engine is retired: {test_engine}"
    );
    assert!(s.finish().success());
}

/// Milestone C item 2: a client's cancel of `alsem_tests` reaches its parts.
/// The parts are sub-requests with ids of their own; the cancel names the
/// call's id. Every child sleeps 1.5 s on each ask (`sleep-on-ask`), so the
/// cancel lands while the roots part runs on the production engine: that
/// part is stopped (`session/cancel`), the select part is never queued (no
/// test engine starts), and the call is never answered, as MCP asks. Its id
/// is free again after, and a second call with it is answered normally.
///
/// DISCRIMINATION (2026-11-05): queueing the parts with flags of their own
/// (`self.enqueue(sub_id, request, Some(tx), None)` in `run_composite`)
/// fails it at "the cancelled roots part built no revision" (the roots part
/// ran on and published generation 1; the call is still not answered and no
/// test engine starts, because the coordinator checks the call's flag
/// between parts, so only the revision tells a stopped part from one run to
/// its end); restored, passes.
#[test]
fn a_cancelled_tests_call_stops_its_parts_and_is_not_answered() {
    let t = repo();
    let ws = t.path().join("App");
    let mut s = Server::ready(&ws, Some("sleep-on-ask:1500"));
    s.send(
        json!(7),
        "tools/call",
        json!({"name": "alsem_tests", "arguments": {"routines": ["TU Core::Post"]}}),
    );
    std::thread::sleep(std::time::Duration::from_millis(500));
    s.notify(
        "notifications/cancelled",
        json!({"requestId": 7, "reason": "test"}),
    );
    // Long enough for an uncancelled call to run all its parts (each ask
    // sleeps 1.5 s; the fixture builds in well under a second).
    std::thread::sleep(std::time::Duration::from_millis(6000));
    // The next response is the status call's: the cancelled call sent none.
    let list = engines(&mut s, 8);
    assert_eq!(
        list.len(),
        1,
        "no test engine started after the cancel: {list:?}"
    );
    // The roots part itself was stopped: the child reads the cancel at its
    // first phase, before it pins (builds) a revision, so none is published.
    assert!(
        list[0]["check"].is_null(),
        "the cancelled roots part built no revision: {list:?}"
    );
    let env = envelope(&s.call(
        7,
        "alsem_tests",
        json!({"routines": ["TU Core::Post"], "format": "json"}),
    ));
    assert_eq!(env["kind"], "cli", "{env}");
    assert_eq!(
        env["revisions"]["tests"].as_array().map(Vec::len),
        Some(1),
        "the second call ran every part: {env}"
    );
    assert!(s.finish().success());
}

//! Output validation V4 (spec 2026-11-06-output-validation-r1 §5.2, §6.3,
//! §12.1) through the MCP server: the envelope's `outcome` says what the
//! exit code means, and a file answer's summary stays bounded while the
//! result index beside it holds every requested row.
//!
//! Discrimination (2026-11-06): see the V4 envelope commit message.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::mcp_server::{Server, cli_bytes, envelope};

fn ws_explain() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/r0-corpus/ws-explain")
}

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

/// §12.1: the outcome mapping, every value an answer can give, with
/// `no-routine-changed` only for an impact whose accounting is complete.
#[test]
fn the_outcome_says_what_the_exit_code_means() {
    let ws = ws_explain();
    let mut s = Server::ready(&ws, None);
    let cases = [
        (
            "alsem_explain",
            json!({"routines": ["Archive"]}),
            0,
            "answered",
        ),
        (
            "alsem_prove",
            json!({"routine": "NoSuchRoutine", "question": "may-commit"}),
            2,
            "not-found",
        ),
        (
            "alsem_prove",
            json!({"routine": "CheckAmount", "question": "may-commit"}),
            2,
            "ambiguous",
        ),
    ];
    for (i, (tool, args, exit, outcome)) in cases.iter().enumerate() {
        let env = envelope(&s.call(i as i64 + 1, tool, args.clone()));
        assert_eq!(env["exitCode"], *exit, "{tool} {args}: {env}");
        assert_eq!(env["outcome"], *outcome, "{tool} {args}: {env}");
    }
    // Round-2 n08 / c26: the outcome comes from the tool's structured
    // resolution diagnostic (`summary.outcome`), not its words, so a filed
    // answer keeps it (an ambiguous digest selector read `not-found` once its
    // JSON went to a file); a real absence stays `not-found`. The digest
    // also counts the one routine it asked for.
    //
    // DISCRIMINATION (2026-11-07): `outcome_of_answer` ignoring the summary
    // (always the words) gives `not-found` for the filed explain ambiguity:
    // fails at `explain {"routines":["CheckAmount"],…}`; restored, passes.
    // (2026-11-06) `roots_requested` without the selector diagnostics gives
    // 0: fails at "rootsRequested"; restored, passes.
    for (id, args, outcome) in [
        (
            20,
            json!({"routines": ["CheckAmount"], "maxInlineBytes": 16}),
            "ambiguous",
        ),
        (
            21,
            json!({"routines": ["NoSuchRoutine"], "maxInlineBytes": 16}),
            "not-found",
        ),
    ] {
        let env = envelope(&s.call(id, "alsem_explain", args.clone()));
        assert!(env["stdoutFile"].is_object(), "precondition: filed: {env}");
        assert_eq!(env["exitCode"], 2, "{env}");
        assert_eq!(env["outcome"], outcome, "explain {args}: {env}");
    }
    let env = envelope(&s.call(
        9,
        "alsem_digest",
        json!({"routines": ["CheckAmount"], "format": "json", "maxInlineBytes": 16}),
    ));
    assert!(
        env["stdoutFile"].is_object(),
        "precondition: the answer is filed: {env}"
    );
    assert_eq!(env["exitCode"], 2, "{env}");
    assert_eq!(env["outcome"], "ambiguous", "a filed ambiguity: {env}");
    assert_eq!(env["summary"]["rootsRequested"], 1, "rootsRequested: {env}");
    // Round-3 n08/m01: the complete digest call over the candidates, and no
    // effect totals on a lookup failure.
    let next = &env["summary"]["next"];
    assert_eq!(next["tool"], "alsem_digest", "summary.next: {env}");
    let routines = next["arguments"]["routines"].as_array().expect("routines");
    assert_eq!(routines.len(), 2, "one selector per candidate: {env}");
    assert!(
        routines
            .iter()
            .all(|r| r.as_str().is_some_and(|s| s.contains("CheckAmount"))),
        "{env}"
    );
    for k in ["totalEffects", "totalUnresolved", "routines", "shared"] {
        assert!(
            env["summary"].get(k).is_none(),
            "no {k} on a lookup failure: {env}"
        );
    }
    let (stdout, _, _) = cli_bytes(&env);
    let doc: Value = serde_json::from_slice(&stdout).unwrap();
    assert_eq!(
        doc["payload"]["summary"]["rootsRequested"], 1,
        "the payload counts it too: {doc}"
    );
    assert!(s.finish().success());

    // Impact on the test-universe fixture (a git folder): a diff whose lines
    // all fall outside routines is a complete answer with no routine; a
    // selector that names nothing is input that did not map.
    let t = tempfile::tempdir().unwrap();
    copy_dir(
        &PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/test-universe"),
        t.path(),
    );
    std::fs::create_dir(t.path().join(".git")).unwrap();
    let app = t.path().join("App");
    let mut s = Server::ready(&app, None);
    let field_only = "diff --git a/App/src/Core.al b/App/src/Core.al
--- a/App/src/Core.al
+++ b/App/src/Core.al
@@ -8,3 +8,3 @@
         field(2; Qty; Decimal)
-        {   // quantity
+        {
         }
";
    let env = envelope(&s.call(
        10,
        "alsem_impact",
        json!({"diff": field_only, "tests": false}),
    ));
    let (stdout, _, _) = cli_bytes(&env);
    let text = String::from_utf8(stdout).unwrap();
    assert!(
        text.contains("input: 0 changed routines resolved"),
        "precondition: no routine changed: {text}"
    );
    assert_eq!(env["exitCode"], 0, "{env}");
    assert_eq!(env["outcome"], "no-routine-changed", "{env}");
    let env = envelope(&s.call(
        11,
        "alsem_impact",
        json!({"routines": ["TU Core.NoSuchRoutine"], "tests": false}),
    ));
    assert_eq!(env["exitCode"], 2, "{env}");
    assert_eq!(env["outcome"], "input-not-mapped", "{env}");
    assert!(s.finish().success());
}

/// A workspace whose one file holds `n` routines, each writing a table.
fn many_routines(dir: &Path, n: usize) {
    std::fs::write(
        dir.join("app.json"),
        r#"{"id":"a4a40000-0000-4000-8000-000000000044","name":"V4 Many","publisher":"probe","version":"1.0.0.0","platform":"1.0.0.0","application":"1.0.0.0","idRanges":[{"from":50100,"to":50199}],"runtime":"14.0"}"#,
    )
    .unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    let mut al = String::from(
        "table 50100 \"V4 Many Row\"\n{\n    fields\n    {\n        field(1; \"No.\"; Code[20]) { }\n    }\n}\n\ncodeunit 50101 \"V4 Many Writers With A Long Name\"\n{\n",
    );
    for i in 0..n {
        al.push_str(&format!(
            "    procedure WriteTheRowNumber{i:02}WithALongRoutineName()\n    var\n        Row: Record \"V4 Many Row\";\n    begin\n        Row.\"No.\" := '{i}';\n        Row.Insert();\n    end;\n\n"
        ));
    }
    al.push_str("}\n");
    std::fs::write(dir.join("src/Many.al"), al).unwrap();
}

/// §12.1 (overflow): requested rows past 8 KiB. The summary stays within
/// 8 KiB and keeps the headline, the analysis and the accounting, with
/// `index` and `summaryTruncated`; the result index holds every requested
/// root (its identity, verdict and test status), the same set the digest
/// document has.
#[test]
fn an_overflowing_summary_keeps_every_requested_row_in_the_index() {
    let t = tempfile::tempdir().unwrap();
    many_routines(t.path(), 60);
    let mut s = Server::ready(t.path(), None);
    let env = envelope(&s.call(
        1,
        "alsem_digest",
        json!({"files": ["src/Many.al"], "singleApp": true, "tests": true}),
    ));
    assert_eq!(env["exitCode"], 0, "{env}");
    let summary = &env["summary"];
    let bytes = summary.to_string().len();
    assert!(bytes <= 8 * 1024, "summary {bytes} bytes: {summary}");
    assert_eq!(summary["rootsResolved"], 60, "{summary}");
    let t_ = &summary["summaryTruncated"];
    assert_eq!(
        t_["rowsTotal"], 60,
        "precondition: the rows overflow: {summary}"
    );
    assert!(t_["rowsListed"].as_u64().unwrap() < 60, "{summary}");
    assert!(
        summary.get("indexRows").is_none(),
        "rows go to the file: {summary}"
    );
    let index = &summary["index"];
    assert_eq!(index["rows"], 60, "{summary}");
    let path = index["path"].as_str().expect("an index path");
    let bytes = std::fs::read(path).expect("the index file");
    assert_eq!(
        al_sem::engine::ids::sha256_bytes_hex(&bytes),
        index["sha256"].as_str().unwrap()
    );
    let doc: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(doc["schema"], "al-sem.result-index/1");
    let rows = doc["rows"].as_array().unwrap();
    for r in rows {
        assert!(
            r["answer"].is_string() && r["testStatus"].is_string(),
            "{r}"
        );
    }
    let indexed: BTreeSet<String> = rows
        .iter()
        .map(|r| r["stableId"].as_str().unwrap().to_string())
        .collect();
    let (answer, _, _) = cli_bytes(&env);
    let full: Value = serde_json::from_slice(&answer).unwrap();
    let requested: BTreeSet<String> = full["payload"]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["routine"]["stableId"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(requested.len(), 60, "precondition: 60 roots");
    assert_eq!(indexed, requested, "every requested root is in the index");
    assert!(s.finish().success());
}

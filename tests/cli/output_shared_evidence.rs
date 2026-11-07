//! Output validation V5, the semantic gate of §12.1: shared evidence
//! reconstructs each root's and each row's original meaning. The query
//! 4.0.0 and digest 4.0.0 documents state shared things once (§7.1–§7.3);
//! here, through the shipped binary on the cross-app fixture, each one
//! read back through its tables equals the 3.x document of the same input
//! (`--schema 3`, kept for one release): every digest root's entry, every
//! query row's operation location and routine, every touches caller's
//! first touch. The prove half (a witness's hops through its segment) is
//! `callers_context_tests::shared_witness_tails_print_once_and_expand_to_every_hop`.
//!
//! Discrimination (2026-11-06, each break made, the test run red, the break
//! reverted, green again): `digest_v4::share_effects` dropping the per-root
//! `status` (liveness) annotation fails `every_digest_root_reconstructs_its_entry`
//! at "root 0's entry"; `query_v4::flatten_origins` dropping `origin.line`
//! fails `every_query_row_keeps_its_operation_location` at "line";
//! `query_v4::intern_witnesses` giving every caller the first witness
//! (a constant key) fails `every_touches_caller_keeps_its_first_touch`.

use al_sem::engine::l4::query_v4;
use al_sem::engine::l5::digest_v4;
use serde_json::Value;

use crate::output_fixture::{ISSUE_DOC, ISSUE_DOC_Q, WRITE_LOG, run_alsem, workspace};

/// The query-touches fixture's workspace (`cli_query_differential`'s
/// `QT_WS`).
pub(crate) fn qt_workspace() -> String {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/query-touches/ws")
        .to_string_lossy()
        .into_owned()
}

/// `alsem <args> --format json` at `--schema 4` (the 4.0.0 document these
/// tests read back) and at `--schema <old>`, both parsed.
fn both(args: &[&str], old: &str) -> (Value, Value) {
    pair(args, Some("4"), old, 0)
}

/// `alsem <args> --format json` at `--schema <new>` (`None`: the default)
/// and at `--schema <old>`, both parsed; each run exits `code`.
fn pair(args: &[&str], new: Option<&str>, old: &str, code: i32) -> (Value, Value) {
    let run = |extra: &[&str]| -> Value {
        let mut a: Vec<&str> = args.to_vec();
        a.extend(["--format", "json"]);
        a.extend(extra);
        let (out, err, got) = run_alsem(&a);
        assert_eq!(got, code, "{a:?}: {err}\n{out}");
        serde_json::from_str(&out).unwrap_or_else(|e| panic!("{a:?}: {e}\n{out}"))
    };
    let first = match new {
        Some(n) => run(&["--schema", n]),
        None => run(&[]),
    };
    (first, run(&["--schema", old]))
}

/// Round-2 §4.6, the previous schema's reconstruction rule: the default
/// digest 5.0.0 and query 5.0.0 documents read back through `to_v4` equal
/// the `--schema 4` document of the same input, payload for payload — a
/// digest with `--writes-table` (its `tableRef` / `asked`), an ambiguous
/// digest selector (its one candidate array), query effects with writes
/// beyond a dependency call (`beyondDependencyCalls`, `origin.fileDisplay`)
/// and the touches documents.
#[test]
fn every_v4_document_is_rebuilt_from_v5() {
    use al_sem::engine::l4::query_v5;
    use al_sem::engine::l5::digest_v5;
    let ws = workspace().to_string_lossy().into_owned();
    let digest = [
        "digest",
        ws.as_str(),
        "--routine",
        ISSUE_DOC,
        "--routine",
        WRITE_LOG,
        "--writes-table",
        "Out Log",
    ];
    let (v5, v4) = pair(&digest, None, "4", 0);
    assert_eq!(v5["schemaVersion"], digest_v5::VERSION);
    let answers = v5["payload"]["entries"][0]["tableAnswers"].to_string();
    assert!(
        answers.contains("\"tableRef\"") && answers.contains("\"asked\""),
        "precondition: a table answer in 5.0.0's shape: {answers}"
    );
    assert_eq!(
        digest_v5::document_to_v4(v5)["payload"],
        v4["payload"],
        "digest --writes-table"
    );
    // An ambiguous selector: no answer (exit 2), its candidates once.
    let qt = qt_workspace();
    let ambiguous = ["digest", ws.as_str(), "--routine", "CheckAmount"];
    let (v5, v4) = pair(&ambiguous, None, "4", 2);
    let d = &v5["payload"]["changedRootsDiagnostics"][0];
    if d["kind"] == "selector-ambiguous" {
        assert!(d["candidates"][0]["stableId"].is_string(), "{d:#}");
        assert!(d.get("candidateRoutines").is_none(), "{d:#}");
    }
    assert_eq!(
        digest_v5::document_to_v4(v5)["payload"],
        v4["payload"],
        "digest ambiguous"
    );
    let queries: Vec<Vec<&str>> = vec![
        vec!["query", "effects", &ws, "--routine", ISSUE_DOC_Q],
        vec![
            "query",
            "touches",
            &qt,
            "--table",
            "QT Item",
            "--from",
            "Leaf",
            "--direction",
            "up",
            "--callers",
        ],
        vec![
            "query",
            "touches",
            &ws,
            "--table",
            "Base Register",
            "--from",
            ISSUE_DOC_Q,
        ],
    ];
    for args in &queries {
        let (v5, v4) = pair(args, None, "4", 0);
        assert_eq!(v5["schemaVersion"], query_v5::VERSION, "{args:?}");
        assert!(!v5.to_string().contains("\"entryPoint"), "{args:?}: {v5:#}");
        assert_eq!(
            query_v5::document_to_v4(v5)["payload"],
            v4["payload"],
            "{args:?}"
        );
    }
}

/// A digest of two roots, one of which (IssueDoc) calls the other
/// (WriteLog), so their cones share effects: each root's entry read back
/// through `payload.effects`, `crossings`, `pathTails` and
/// `unresolvedSites` is its 3.x entry, liveness and temp state included.
#[test]
fn every_digest_root_reconstructs_its_entry() {
    let ws = workspace().to_string_lossy().into_owned();
    for extra in [
        &[][..],
        &["--detail"][..],
        &["--include-reads", "--tests"][..],
    ] {
        let mut args = vec![
            "digest",
            ws.as_str(),
            "--routine",
            ISSUE_DOC,
            "--routine",
            WRITE_LOG,
        ];
        args.extend(extra);
        let (v4, v3) = both(&args, "3");
        assert_eq!(v4["schemaVersion"], digest_v4::VERSION, "{extra:?}");
        assert!(
            v3["schemaVersion"]
                .as_str()
                .is_some_and(|s| s.starts_with("3.")),
            "{extra:?}"
        );
        let refs: usize = v4["payload"]["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["effects"].as_array().map_or(0, Vec::len))
            .sum();
        let shared = v4["payload"]["effects"].as_object().map_or(0, |m| m.len());
        assert!(
            shared > 0 && shared < refs,
            "precondition ({extra:?}): the two roots share an effect ({shared} shared rows for \
             {refs} references)"
        );
        let back = digest_v4::document_to_v3(v4.clone());
        let (b, o) = (&back["payload"]["entries"], &v3["payload"]["entries"]);
        assert_eq!(b.as_array().map(Vec::len), o.as_array().map(Vec::len));
        for (i, (got, want)) in b
            .as_array()
            .unwrap()
            .iter()
            .zip(o.as_array().unwrap())
            .enumerate()
        {
            assert_eq!(got, want, "{extra:?}: root {i}'s entry");
        }
        assert_eq!(back["payload"], v3["payload"], "{extra:?}");
    }
}

/// query effects: every row's operation location is on the row
/// (`origin.file`, `line`, `column`, no join) and equals the 3.x
/// `origin.anchor`; its routine and first hop are the 3.x ones through
/// `payload.routines`; the whole payload reads back.
#[test]
fn every_query_row_keeps_its_operation_location() {
    let ws = workspace().to_string_lossy().into_owned();
    let (v4, v3) = both(&["query", "effects", &ws, "--routine", ISSUE_DOC_Q], "3");
    assert_eq!(v4["schemaVersion"], query_v4::VERSION);
    let rows4 = v4["payload"]["effects"].as_array().unwrap();
    let rows3 = v3["payload"]["effects"].as_array().unwrap();
    assert!(!rows3.is_empty(), "precondition: rows");
    assert_eq!(rows4.len(), rows3.len());
    let routines = &v4["payload"]["routines"];
    for (r4, r3) in rows4.iter().zip(rows3) {
        let (o4, o3) = (&r4["origin"], &r3["origin"]);
        for k in ["file", "line", "column"] {
            assert_eq!(o4[k], o3["anchor"][k], "{k}: {r4:#} vs {r3:#}");
        }
        let id = o4["routine"].as_str().expect("a routine id");
        assert_eq!(routines[id], o3["routine"], "{r4:#}");
        if let Some(h) = r4["firstHop"].as_str() {
            assert_eq!(routines[h]["stableId"], r3["firstHop"]["stableId"]);
        }
    }
    let back = query_v4::document_to_v3(v4);
    assert_eq!(back["payload"], v3["payload"]);
}

/// query touches up with `--callers`: every touching caller's first touch,
/// written once in `payload.witnesses` when shared, reads back to its 3.x
/// row; and so does every other touches document of the fixture.
#[test]
fn every_touches_caller_keeps_its_first_touch() {
    let ws = workspace().to_string_lossy().into_owned();
    // The query-touches fixture: three callers of `Leaf` touch `QT Item`
    // through other branches, two of them with one shared first touch.
    let qt = qt_workspace();
    let cases: Vec<Vec<&str>> = vec![
        vec![
            "query",
            "touches",
            &qt,
            "--table",
            "QT Item",
            "--from",
            "Leaf",
            "--direction",
            "up",
            "--callers",
        ],
        vec![
            "query",
            "touches",
            &ws,
            "--table",
            "Out Log",
            "--from",
            ISSUE_DOC_Q,
        ],
        vec![
            "query",
            "touches",
            &ws,
            "--table",
            "Base Register",
            "--from",
            ISSUE_DOC_Q,
        ],
        vec!["query", "touches", &ws, "--table", "Out Log"],
    ];
    for args in &cases {
        let (v4, v3) = both(args, "3");
        let back = query_v4::document_to_v3(v4.clone());
        assert_eq!(back["payload"], v3["payload"], "{args:?}");
    }
    let (v4, _) = both(&cases[0], "3");
    let callers = v4["payload"]["up"]["callers"].as_array().unwrap();
    assert!(
        callers.iter().any(|c| c["firstWitness"].is_string()),
        "precondition: a caller with a first touch: {v4:#}"
    );
    for c in callers {
        if let Some(w) = c["firstWitness"].as_str() {
            assert!(v4["payload"]["witnesses"][w].is_object(), "{w}");
        }
    }
}

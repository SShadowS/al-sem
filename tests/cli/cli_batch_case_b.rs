//! #277 Task 11, case B (spec §9): step 9's tree reads (prove's effects, the
//! `fingerprint --routine` query, `fingerprint --inventory-only`) are per-job
//! tree demands on one view's pass, each over its own subjects (D5.2, D5.4).

use std::path::Path;

use crate::cli_batch_plan::{argv, assert_each_standalone, batch, standalone_in};
use crate::cli_build_counters::count;

/// `ws_dep()`'s workspace and source dependency, except that `DoWork` writes
/// only the workspace's own table: the dependency's table 18 is reached only
/// through `CallLib` → `Dep Lib.InsertCust`, an inherited fact. (In
/// `ws_dep()` `DoWork` writes table 18 itself, so every job's output names
/// the dependency's identities anyway and a shared raw set would not show.)
fn ws_split(dir: &Path) {
    crate::cli_digest_query_dependencies::write_dependency_workspace(dir);
    std::fs::write(
        dir.join("src/Work.al"),
        "table 50160 \"Order Pollution Flag\"\n{\n    fields\n    {\n        \
         field(1; \"Entry No.\"; Integer)\n        {\n        }\n    }\n}\n\n\
         codeunit 50100 \"Ws Work\"\n{\n    procedure DoWork()\n    var\n        \
         Flag: Record \"Order Pollution Flag\";\n    begin\n        Flag.Insert();\n    \
         end;\n\n    procedure CallLib()\n    var\n        Lib: Codeunit \"Dep Lib\";\n    \
         begin\n        Lib.InsertCust();\n    end;\n}\n",
    )
    .unwrap();
}

/// A witness path that stops at a dependency routine (#160 step 9: its
/// `boundary` object) on an inherited fact.
fn reaches_a_dependency_routine(query: &str) -> bool {
    query.contains("\"provenance\": \"inherited\"") && query.contains("\"boundary\"")
}

/// T22 (D5.4; step 9 §4.7 rule 4): ONE cross-app batch of `fingerprint
/// --inventory-only`, `fingerprint --routine CallLib`, `fingerprint --routine
/// DoWork` and `prove CallLib may-commit` on [`ws_split`]. Each job's tree
/// reads its own subjects, so each job is byte-equal to its standalone run
/// (stdout, stderr, exit). Preconditions (standalone): the two queries
/// differ, and `CallLib`'s cone reaches a dependency routine (an inherited
/// fact whose path stops at the boundary) while `DoWork`'s does not, so a raw
/// set shared across the trees is observable. The view's request is the
/// union of the jobs' (one witness pass; four trees).
#[test]
fn batch_tree_reads_use_own_subjects() {
    let d = tempfile::tempdir().unwrap();
    ws_split(d.path());
    let ws = d.path().to_string_lossy().into_owned();
    let query = |routine: &str| {
        argv(&[
            "fingerprint",
            &ws,
            "--format",
            "json",
            "--routine",
            routine,
            "--deterministic",
        ])
    };
    let jobs = vec![
        argv(&[
            "fingerprint",
            &ws,
            "--inventory-only",
            "--format",
            "json",
            "--deterministic",
        ]),
        query("CallLib"),
        query("DoWork"),
        argv(&["prove", &ws, "CallLib", "may-commit", "--deterministic"]),
    ];
    let (r1, _, e1) = standalone_in(&jobs[1], None);
    let (r2, _, e2) = standalone_in(&jobs[2], None);
    assert_eq!(
        (e1, e2),
        (Some(0), Some(0)),
        "precondition: both queries run"
    );
    let (r1, r2) = (String::from_utf8_lossy(&r1), String::from_utf8_lossy(&r2));
    assert_ne!(r1, r2, "precondition: the two queries differ");
    assert!(
        reaches_a_dependency_routine(&r1) && !reaches_a_dependency_routine(&r2),
        "precondition: CallLib's cone reaches a dependency routine, DoWork's does not\n\
         CallLib: {r1}\nDoWork: {r2}"
    );

    let run = batch(&jobs);
    assert_each_standalone(&run, &jobs, None);
    let m = &run.counters;
    assert_eq!(count(m, "build.program"), 1, "one view: {m:?}");
    assert_eq!(count(m, "build.witness_pass"), 1, "one pass: {m:?}");
    assert_eq!(count(m, "build.snapshot"), 4, "one tree per job: {m:?}");
}

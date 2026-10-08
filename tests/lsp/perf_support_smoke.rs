//! Correctness checks for the synthetic AL corpus generator (Task T0.5).
//! Kept as its own always-compiled integration test target (rather than a
//! `#[cfg(test)]` block inside `perf_support/mod.rs`) because that file is
//! also `#[path]`-included by `benches/lsp_pipeline.rs`, a `harness = false`
//! bench where `#[test]` functions would compile as unreachable dead code.
//!
//! T3 Task 17: the corpus-indexing assertions below were rewritten off the
//! deleted legacy `Indexer`/`graph` pipeline onto the engine-backed
//! `LspSnapshot`/`lsp::handlers`/`lsp::updater` surface `tests/perf_bounds.rs`
//! already measures — but `perf_bounds.rs`'s checks only compile under
//! `#[cfg(not(debug_assertions))]` (a release-only gate), so THIS file is
//! kept (not deleted as "redundant") specifically to pin the corpus's own
//! contract (999-way fan-in / 3-way fan-out / rung classification / decl
//! counts) under a plain `cargo test` (debug profile) too — every ordinary
//! `cargo test` run, not just `cargo test --release --test perf_bounds`.

// The recursive-SCC half of `perf_support` (`generate_recursive_corpus` and
// friends — final-branch-review M-6) exists for `tests/perf_bounds.rs`'s
// release-only L4 gate and has no LSP-surface contract to pin here, so those
// items are legitimately unused in THIS crate's compilation of the
// `#[path]`-included module tree.
//
// ⟨fix wave FIX 3, finding 6⟩ Each recursive-corpus item carries its OWN
// `#[allow(dead_code)]` (in `tests/perf_support/mod.rs` itself) rather than a
// blanket allow on this `mod` declaration. `benches/lsp_pipeline.rs` and
// `benches/engine_stages.rs` still carry a blanket allow at their own include
// sites, but THIS was the last include site where an unused item in the
// EXISTING (non-recursive) corpus would still warn — a blanket allow here
// would have silenced that signal for every dead helper, present or future,
// not just the recursive ones. The generator's own recursive shape contract is
// asserted by that release gate, before it times anything (see
// `recursive_scc_db_effects_within_bound`).
#[path = "../perf_support/mod.rs"]
mod perf_support;

use al_sem::lsp::encoding::PositionEncoding;
use al_sem::lsp::handlers::{self, ItemData};
use al_sem::lsp::snapshot::LspSnapshot;
use al_sem::lsp::updater::{ChangeEvent, Rung, Updater};
use al_sem::protocol::path_to_uri;
use perf_support::{
    EVENT_ROUTINES_PER_FILE, HUB_INDEX, PROCS_PER_FILE, file_name, generate_corpus,
};
use std::fs;
use std::path::Path;
use tempfile::TempDir;

/// Total routines per generated file: the plain `Proc*` procedures plus the
/// event-bearing publisher/subscriber quartet (t3 whole-branch review — see
/// `tests/perf_support/mod.rs`'s "Event-bearing" doc section).
const ROUTINES_PER_FILE: usize = PROCS_PER_FILE + EVENT_ROUTINES_PER_FILE;

#[test]
fn generates_expected_file_count() {
    let dir = TempDir::new().unwrap();
    let n = generate_corpus(dir.path(), 10);
    assert_eq!(n, 10);
    let al_files: Vec<_> = fs::read_dir(dir.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|x| x == "al"))
        .collect();
    assert_eq!(al_files.len(), 10);
}

#[test]
fn corpus_is_deterministic_across_calls() {
    let dir_a = TempDir::new().unwrap();
    let dir_b = TempDir::new().unwrap();
    generate_corpus(dir_a.path(), 25);
    generate_corpus(dir_b.path(), 25);
    for i in 0..25 {
        let a = fs::read_to_string(dir_a.path().join(file_name(i))).unwrap();
        let b = fs::read_to_string(dir_b.path().join(file_name(i))).unwrap();
        assert_eq!(a, b, "file {i} content diverged between two generations");
    }
}

/// A minimal `app.json` so `LspSnapshot::build_full`/`build_full_with_parsed`
/// (which hard-require one at the workspace root) accept a
/// perf_support-generated directory as a workspace — mirrors
/// `tests/perf_bounds.rs`'s own `write_minimal_app_json` helper.
fn write_minimal_app_json(dir: &Path) {
    fs::write(
        dir.join("app.json"),
        r#"{
    "id": "00000000-0000-0000-0000-000000000002",
    "name": "PerfSmokeCorpus",
    "publisher": "test",
    "version": "1.0.0.0"
}"#,
    )
    .expect("write perf-smoke-corpus app.json");
}

#[test]
fn indexes_with_expected_definitions_and_hub_fan_in() {
    let dir = TempDir::new().unwrap();
    write_minimal_app_json(dir.path());
    let file_count = 20;
    generate_corpus(dir.path(), file_count);

    let snap = LspSnapshot::build_full(dir.path()).expect("build_full");
    let total_decls: usize = snap.decls_by_file.values().map(|v| v.len()).sum();
    assert_eq!(total_decls, file_count * ROUTINES_PER_FILE);

    // The hub's Proc0 must have exactly `file_count - 1` incoming calls (one
    // qualified call — via a declared `Hub` variable — from every other
    // file's Proc0).
    let hub_file = file_name(HUB_INDEX);
    let hub_proc0 = snap.decls_by_file[&hub_file]
        .iter()
        .find(|d| d.name == "Proc0")
        .expect("hub Proc0 decl")
        .id
        .clone();
    let incoming = handlers::incoming(&snap, PositionEncoding::Utf8, &ItemData { node: hub_proc0 });
    assert_eq!(incoming.len(), file_count - 1);

    // A non-hub file's Proc0 must have exactly 3 outgoing calls (1 cross-file
    // qualified + 2 local).
    let file1 = file_name(1);
    let f1_proc0 = snap.decls_by_file[&file1]
        .iter()
        .find(|d| d.name == "Proc0")
        .expect("file-1 Proc0 decl")
        .id
        .clone();
    let outgoing = handlers::outgoing(&snap, PositionEncoding::Utf8, &ItemData { node: f1_proc0 });
    assert_eq!(outgoing.len(), 3);

    // Sanity: `prepare` at the hub's Proc0 name-token position resolves.
    let hub_uri = path_to_uri(&dir.path().join(&hub_file))
        .as_str()
        .to_string();
    let prepared = handlers::prepare(&snap, PositionEncoding::Utf8, &hub_uri, 2, 15);
    assert!(prepared.is_some(), "sanity: prepare must find Proc0");

    // Non-vacuity (t3 whole-branch review): the corpus is now event-bearing
    // (2 publishers/file, each with exactly one real subscriber, wired to
    // the PREVIOUS file — see tests/perf_support/mod.rs's doc), so
    // `event_edges`/`publisher_fanout` must actually be populated at scale —
    // NOT the accidental zero that let the O(decls * event_edges) quadratic
    // in `compute_all` go undetected before this fix.
    assert_eq!(
        snap.ws_event_edges.len(),
        file_count * perf_support::PUBLISHERS_PER_FILE,
        "PUBLISHERS_PER_FILE publisher declarations per file, unconditionally emitted"
    );
    assert_eq!(
        snap.ws_publisher_fanout.len(),
        file_count * perf_support::PUBLISHERS_PER_FILE,
        "every publisher has exactly one real subscriber, so every publisher \
         gets a publisher_fanout entry"
    );
    assert!(
        snap.ws_publisher_fanout.values().all(|&n| n == 1),
        "every publisher in this corpus has EXACTLY one real subscriber"
    );
}

#[test]
fn rewrite_with_extra_procedure_adds_one_definition_and_takes_rung2() {
    let dir = TempDir::new().unwrap();
    write_minimal_app_json(dir.path());
    let file_count = 5;
    generate_corpus(dir.path(), file_count);

    let (base, parsed) =
        LspSnapshot::build_full_with_parsed(dir.path()).expect("build_full_with_parsed");
    let file1 = file_name(1);
    assert_eq!(base.decls_by_file[&file1].len(), ROUTINES_PER_FILE);

    let mut updater = Updater::new(dir.path().to_path_buf(), parsed);
    perf_support::rewrite_with_extra_procedure(dir.path(), file_count, 1);
    let batch = vec![ChangeEvent::FileSaved(dir.path().join(&file1))];
    let (next, rung) = updater
        .apply_batch(&base, &batch)
        .expect("apply_batch must succeed");

    assert_eq!(
        rung,
        Rung::Two,
        "a brand-new routine identity must take rung 2"
    );
    assert_eq!(
        next.decls_by_file[&file1].len(),
        ROUTINES_PER_FILE + 1,
        "rewritten file must contribute one extra definition (ProcExtra)"
    );
}

#[test]
fn body_only_comment_edit_adds_no_definitions_and_stays_rung1() {
    let dir = TempDir::new().unwrap();
    write_minimal_app_json(dir.path());
    let file_count = 5;
    generate_corpus(dir.path(), file_count);

    let (base, parsed) =
        LspSnapshot::build_full_with_parsed(dir.path()).expect("build_full_with_parsed");
    let file1 = file_name(1);
    assert_eq!(base.decls_by_file[&file1].len(), ROUTINES_PER_FILE);

    let mut updater = Updater::new(dir.path().to_path_buf(), parsed);
    perf_support::body_only_comment_edit(dir.path(), file_count, 1);
    let batch = vec![ChangeEvent::FileSaved(dir.path().join(&file1))];
    let (next, rung) = updater
        .apply_batch(&base, &batch)
        .expect("apply_batch must succeed");

    assert_eq!(rung, Rung::One, "a comment-only body edit must stay rung 1");
    assert_eq!(
        next.decls_by_file[&file1].len(),
        ROUTINES_PER_FILE,
        "a body-only comment edit must add ZERO definitions"
    );
}

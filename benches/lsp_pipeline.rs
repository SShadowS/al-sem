//! Criterion benches for the ENGINE-BACKED LSP surface (T3 LSP-migration
//! arc, Task 16): `LspSnapshot` / `lsp::handlers` / `lsp::updater` — the
//! surface Task 15 cut the server over to. Measures: `build_full` over the
//! 100/1000-file synthetic corpora, the 3 call-hierarchy query handlers
//! (`prepare` / `incoming` / `outgoing`) against a 1000-file batch-built
//! snapshot, `compute_all` (the diagnostics recompute run after every
//! snapshot swap — added in the t3 whole-branch review's blocker fix-wave),
//! and the incremental updater's rung-1 (body edit) / rung-2 (signature
//! edit) `apply_batch` paths — all in-process, no LSP stdio loop. This file
//! previously benched the LEGACY `indexer.rs`/`graph.rs`/`handlers.rs`
//! pipeline (T0.5); that pipeline is deleted in Task 17.
//!
//! Run: `cargo bench --bench lsp_pipeline` (or `cargo bench` for every bench
//! target). See `tests/perf_bounds.rs` for the release-only CI gate these
//! measurements feed into (including its module doc's explanation of why
//! `tests/perf_support`'s hub call goes through a declared variable, and
//! why `incoming`'s bound is measured separately from `prepare`/`outgoing`'s)
//! and `tests/perf_support/mod.rs` for the corpus shape/rationale
//! (deterministic, real cross-file fan-in/fan-out).

// `PROCS_PER_FILE`/`object_name` (only reached transitively via `file_name`)
// go legitimately unused by name in THIS crate's compilation — mirrors
// `benches/engine_stages.rs`'s identical `#[allow(dead_code)]` on the same
// `#[path]`-included module tree.
#[path = "../tests/perf_support/mod.rs"]
#[allow(dead_code)]
mod perf_support;

use al_sem::config::DiagnosticConfig;
use al_sem::lsp::diagnostics::compute_all;
use al_sem::lsp::encoding::PositionEncoding;
use al_sem::lsp::handlers::{self, ItemData};
use al_sem::lsp::snapshot::LspSnapshot;
use al_sem::lsp::updater::{ChangeEvent, Rung, Rung1Context, Updater};
use al_sem::protocol::path_to_uri;
use criterion::{Criterion, criterion_group, criterion_main};
use std::hint::black_box;
use std::path::Path;
use tempfile::TempDir;

/// A minimal `app.json` so `LspSnapshot::build_full`/`build_full_with_parsed`
/// (which hard-require one at the workspace root, via `SnapshotBuilder`)
/// accept the perf_support-generated directory as a workspace. No
/// `.alpackages` is written — the synthetic corpus has zero dependencies by
/// design.
fn write_minimal_app_json(dir: &Path) {
    std::fs::write(
        dir.join("app.json"),
        r#"{
    "id": "00000000-0000-0000-0000-000000000001",
    "name": "PerfCorpus",
    "publisher": "bench",
    "version": "1.0.0.0"
}"#,
    )
    .expect("write perf-corpus app.json");
}

fn corpus_dir(file_count: usize) -> TempDir {
    let dir = TempDir::new().unwrap();
    write_minimal_app_json(dir.path());
    perf_support::generate_corpus(dir.path(), file_count);
    dir
}

/// `LspSnapshot::build_full` over the 100- and 1000-file synthetic corpora
/// (CLAUDE.md targets: <500ms / <2s). Corpus generation happens once,
/// outside the timed closure — only the build itself is measured per
/// iteration.
fn bench_build_full(c: &mut Criterion) {
    let mut group = c.benchmark_group("build_full");

    for &file_count in &[100usize, 1000] {
        let dir = corpus_dir(file_count);
        group.bench_function(format!("{file_count}_files"), |b| {
            b.iter(|| {
                let snap = LspSnapshot::build_full(black_box(dir.path())).unwrap();
                black_box(snap.decls_by_file.len());
            });
        });
    }
    group.finish();
}

/// The 3 call-hierarchy query handlers against a 1000-file batch-built
/// snapshot. Building the snapshot happens once, outside the timed closures —
/// each iteration measures only the handler call, matching how a running LSP
/// server serves a steady-state request. See `tests/perf_bounds.rs`'s module
/// doc for why `incoming`'s CI bound differs from `prepare`/`outgoing`'s
/// (this corpus's hub deliberately has 999-way real fan-in, and every
/// distinct caller's position is re-derived live from its own current file
/// text rather than served from a precomputed span).
fn bench_queries(c: &mut Criterion) {
    let dir = corpus_dir(1000);
    let snap = LspSnapshot::build_full(dir.path()).expect("build_full");

    let mut group = c.benchmark_group("query_handlers_1000_files");

    // prepare: resolve a source position to a definition. Line 2 is
    // `    procedure Proc0()` in generated file content (see
    // perf_support::codeunit_source); character 15 lands inside its range.
    let prepare_uri = path_to_uri(&dir.path().join(perf_support::file_name(1)))
        .as_str()
        .to_string();
    group.bench_function("prepare", |b| {
        b.iter(|| {
            black_box(handlers::prepare(
                &snap,
                PositionEncoding::Utf8,
                black_box(&prepare_uri),
                2,
                15,
            ));
        });
    });

    // incoming: the hub's Proc0 has real fan-in (999 distinct callers, one
    // per other generated file, each via its own declared `Hub` variable —
    // see perf_support module doc).
    let hub_file = perf_support::file_name(perf_support::HUB_INDEX);
    let hub_proc0 = ItemData {
        node: snap.decls_by_file[&hub_file]
            .iter()
            .find(|d| d.name == "Proc0")
            .expect("hub Proc0 decl")
            .id
            .clone(),
    };
    group.bench_function("incoming", |b| {
        b.iter(|| {
            black_box(handlers::incoming(
                &snap,
                PositionEncoding::Utf8,
                black_box(&hub_proc0),
            ));
        });
    });

    // outgoing: any non-hub file's Proc0 has real fan-out (3 callees).
    let file1 = perf_support::file_name(1);
    let file1_proc0 = ItemData {
        node: snap.decls_by_file[&file1]
            .iter()
            .find(|d| d.name == "Proc0")
            .expect("file-1 Proc0 decl")
            .id
            .clone(),
    };
    group.bench_function("outgoing", |b| {
        b.iter(|| {
            black_box(handlers::outgoing(
                &snap,
                PositionEncoding::Utf8,
                black_box(&file1_proc0),
            ));
        });
    });

    group.finish();
}

/// `compute_all` — the full diagnostics recompute `on_swap` runs after
/// EVERY snapshot swap (t3 whole-branch review, blocker fix — see
/// `LspSnapshot::publisher_fanout`'s doc and `tests/perf_bounds.rs`'s
/// `compute_all_within_bound` for the CI gate this bench's numbers feed).
/// This corpus is event-bearing (see `tests/perf_support/mod.rs`'s doc), so
/// `event_edges`/`publisher_fanout` are genuinely populated at scale —
/// building the snapshot happens once, outside the timed closure.
fn bench_compute_all(c: &mut Criterion) {
    let dir = corpus_dir(1000);
    let snap = LspSnapshot::build_full(dir.path()).expect("build_full");
    let cfg = DiagnosticConfig::default();

    c.bench_function("compute_all_1000_files", |b| {
        b.iter(|| {
            black_box(compute_all(&snap, PositionEncoding::Utf8, &cfg));
        });
    });
}

/// The incremental updater's rung-1 (body-only edit) path against a
/// 1000-file snapshot (T3 Task 9 Step-3b CDO re-measurement: ~10.5ms
/// warm-context; CLAUDE.md target: <100ms). Every iteration re-saves the
/// SAME already-applied content (unchanged fingerprint relative to whatever
/// was just published), which is exactly rung 1's own gate condition — see
/// `tests/perf_bounds.rs`'s equivalent test for the full rationale.
fn bench_rung1_body_edit(c: &mut Criterion) {
    let dir = corpus_dir(1000);
    let (base, parsed) =
        LspSnapshot::build_full_with_parsed(dir.path()).expect("build_full_with_parsed");
    let mut updater = Updater::new(dir.path().to_path_buf(), parsed);
    let target = dir.path().join(perf_support::file_name(1));
    perf_support::body_only_comment_edit(dir.path(), 1000, 1);
    let batch = vec![ChangeEvent::FileSaved(target)];

    let (warm_snap, warm_rung) = updater
        .apply_batch(&base, &batch)
        .expect("apply_batch (warm-up)");
    assert_eq!(
        warm_rung,
        Rung::One,
        "a comment-only body edit must stay rung 1"
    );
    let mut cur = warm_snap;

    c.bench_function("rung1_body_edit_1000_files", |b| {
        b.iter(|| {
            let (next, rung) = updater
                .apply_batch(&cur, black_box(&batch))
                .expect("apply_batch must succeed");
            debug_assert_eq!(rung, Rung::One);
            cur = next;
            black_box(&cur);
        });
    });
}

/// The PRODUCTION rung-1 path: context built once (like `spawn_updater`'s
/// scoped-context loop), then reused across every iteration — measures what
/// a user's keystroke-save actually costs, unlike `rung1_body_edit_1000_files`
/// (kept as the worst-case: context rebuilt per call). See the 2026-07-14
/// improvement-hunt F6 finding.
fn bench_rung1_body_edit_scoped(c: &mut Criterion) {
    let dir = corpus_dir(1000);
    let (base, parsed) =
        LspSnapshot::build_full_with_parsed(dir.path()).expect("build_full_with_parsed");
    let mut updater = Updater::new(dir.path().to_path_buf(), parsed);
    let target = dir.path().join(perf_support::file_name(1));
    perf_support::body_only_comment_edit(dir.path(), 1000, 1);
    let batch = vec![ChangeEvent::FileSaved(target)];

    let ctx = Rung1Context::build(&base, updater.workspace());
    let (warm, _delta) = updater
        .apply_batch_scoped(&base, &batch, &ctx)
        .expect("a comment-only body edit must stay rung 1");
    let mut cur = warm;

    c.bench_function("rung1_body_edit_scoped_1000_files", |b| {
        b.iter(|| {
            let (next, _delta) = updater
                .apply_batch_scoped(&cur, black_box(&batch), &ctx)
                .expect("must stay rung 1");
            cur = next;
            black_box(&cur);
        });
    });
}

/// The incremental updater's rung-2 (definition-surface-change) path against
/// a 1000-file snapshot (T3 Task 9 Step-3b CDO re-measurement: ~1.464s;
/// REPLACES Task 3's superseded ~1.9s algebraic upper-bound estimate — see
/// `.superpowers/sdd/t3-stage-split.md`). Each iteration needs its OWN fresh
/// baseline: rung 2's gate compares against the CURRENTLY PUBLISHED
/// fingerprint, so re-using one already-escalated snapshot across iterations
/// would silently degrade to rung 1 on the 2nd+ iteration — see
/// `tests/perf_bounds.rs`'s equivalent test for the full rationale.
/// `iter_batched` is used (not plain `iter`) so that fresh-baseline SETUP
/// (corpus generation + `build_full_with_parsed` + the signature edit) runs
/// OUTSIDE the timed region for every iteration — measuring only
/// `apply_batch` itself, matching `tests/perf_bounds.rs`'s methodology
/// (a plain `iter` closure would time the setup too, inflating this number
/// far past the CI gate's own measurement of the identical operation).
/// `sample_size` is lowered (like `engine_stages.rs`'s heaviest group) since
/// setup rebuilds the whole snapshot+updater from scratch every iteration.
fn bench_rung2_signature_edit(c: &mut Criterion) {
    let mut group = c.benchmark_group("rung2_signature_edit");
    group.sample_size(20);
    group.bench_function("1000_files", |b| {
        b.iter_batched(
            || {
                let dir = corpus_dir(1000);
                let (base, parsed) = LspSnapshot::build_full_with_parsed(dir.path())
                    .expect("build_full_with_parsed");
                let updater = Updater::new(dir.path().to_path_buf(), parsed);
                perf_support::rewrite_with_extra_procedure(dir.path(), 1000, 1);
                let batch = vec![ChangeEvent::FileSaved(
                    dir.path().join(perf_support::file_name(1)),
                )];
                (dir, base, updater, batch)
            },
            |(dir, base, mut updater, batch)| {
                let (new_snap, rung) = updater
                    .apply_batch(&base, black_box(&batch))
                    .expect("apply_batch must succeed");
                debug_assert_eq!(rung, Rung::Two);
                // `dir` (the TempDir) must outlive the call above (the
                // batch's FileSaved path reads from it) — returned here so
                // Criterion drops it (and `new_snap`) AFTER timing ends,
                // never as part of the measured region.
                (dir, new_snap)
            },
            criterion::BatchSize::SmallInput,
        );
    });
    group.finish();
}

criterion_group!(
    benches,
    bench_build_full,
    bench_queries,
    bench_compute_all,
    bench_rung1_body_edit,
    bench_rung1_body_edit_scoped,
    bench_rung2_signature_edit
);
criterion_main!(benches);

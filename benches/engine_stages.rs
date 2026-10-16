//! Criterion benches for the program-engine (Pipeline 2) stage split — T3
//! (LSP-migration arc) Task 3: MEASUREMENT ONLY, no engine behavior changes.
//!
//! Splits `aldump --program-call-graph-stats`'s pipeline into its stages —
//! snapshot / parse / build(graph) / `ResolveIndex::build` / `DeclSurface::build`
//! / resolve — over the synthetic 100/1000-file perf corpus (same generator
//! `benches/lsp_pipeline.rs` uses). Real (CDO-scale) numbers come from a
//! separate `#[ignore]`d unit test inside `src/program/resolve/full.rs`
//! (`cargo test --release stage_split -- --ignored --nocapture`), because
//! that test needs the module-private `resolve_full_program_from_parts` —
//! see that test's doc comment for why.
//!
//! # Why "resolve" isn't benched directly here
//!
//! `resolve_full_program_from_parts` (the obligation-resolution inner loop,
//! `src/program/resolve/full.rs`) is a private fn — invisible to this bench,
//! which (like every `benches/*.rs` file) compiles as its own external crate.
//! `ResolveIndex::build` and `DeclSurface::build` ARE `pub`, so they're benched
//! directly below. The only public entry point that reaches the private
//! inner loop is [`resolve_full_program`], which re-does snapshot + parse +
//! graph-build + index/body-map-build ITSELF (see `full.rs`'s
//! `build_context`) — so its measurement is a TOTAL, not an isolated
//! "resolve" number. `.superpowers/sdd/t3-stage-split.md` (arc scratch, not
//! part of this commit) derives the inner-loop-only "resolve" number by
//! subtracting this bench's other stage numbers from that total; see its
//! Methodology section.
//!
//! Also note: `build_program_graph` calls `parse_snapshot` INTERNALLY (to
//! extract object/routine nodes), so its own measured total already
//! INCLUDES a parse pass — `engine_stage_build_program_graph_total` is not
//! "graph build alone"; the results doc derives that by subtracting
//! `engine_stage_parse`'s number too.

// `perf_support` is shared with `benches/lsp_pipeline.rs`, which exercises
// its full API (including `rewrite_with_extra_procedure`, for the
// single-file-reindex bench); this bench only needs the corpus generator, so
// that item is legitimately unused in THIS crate's compilation of the
// `#[path]`-included module tree.
#[path = "../tests/perf_support/mod.rs"]
#[allow(dead_code)]
mod perf_support;

use al_sem::program::abi_ingest::AbiCache;
use al_sem::program::build::build_program_graph;
use al_sem::program::resolve::decl_surface::DeclSurface;
use al_sem::program::resolve::full::resolve_full_program;
use al_sem::program::resolve::index::ResolveIndex;
use al_sem::snapshot::{AppSetSnapshot, SnapshotBuilder, parse_snapshot};
use criterion::{Criterion, criterion_group, criterion_main};
use std::hint::black_box;
use tempfile::TempDir;

/// A minimal `app.json` so `SnapshotBuilder` (which hard-requires one at the
/// workspace root) accepts the perf_support-generated directory as a
/// workspace. No `.alpackages` is written — the synthetic corpus has zero
/// dependencies by design, so `dependencies::load_all_apps` sees no
/// `.alpackages` folder and returns an empty dependency set (not an error).
fn write_minimal_app_json(dir: &std::path::Path) {
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

/// Generate an `file_count`-codeunit synthetic corpus (same generator as
/// `benches/lsp_pipeline.rs`) plus the `app.json` a `SnapshotBuilder` needs.
fn corpus_dir(file_count: usize) -> TempDir {
    let dir = TempDir::new().expect("tempdir");
    write_minimal_app_json(dir.path());
    perf_support::generate_corpus(dir.path(), file_count);
    dir
}

fn build_snapshot(dir: &std::path::Path) -> AppSetSnapshot {
    (SnapshotBuilder {
        workspace_root: dir.to_path_buf(),
        local_providers: vec![],
    })
    .build()
    .expect("snapshot build (perf corpus workspace)")
}

/// Stage 1: `SnapshotBuilder::build` — workspace scan + `.alpackages` dep
/// resolution (empty here — no deps in the synthetic corpus).
fn bench_snapshot(c: &mut Criterion) {
    let mut group = c.benchmark_group("engine_stage_snapshot");
    for &file_count in &[100usize, 1000] {
        let dir = corpus_dir(file_count);
        group.bench_function(format!("{file_count}_files"), |b| {
            b.iter(|| {
                let snap = build_snapshot(black_box(dir.path()));
                black_box(snap.apps.len());
            });
        });
    }
    group.finish();
}

/// Stage 2: `parse_snapshot` — deep-parse every source-bearing app's files
/// into the owned `al_syntax` IR.
fn bench_parse(c: &mut Criterion) {
    let mut group = c.benchmark_group("engine_stage_parse");
    for &file_count in &[100usize, 1000] {
        let dir = corpus_dir(file_count);
        let snap = build_snapshot(dir.path());
        group.bench_function(format!("{file_count}_files"), |b| {
            b.iter(|| {
                let parsed = parse_snapshot(black_box(&snap));
                black_box(parsed.len());
            });
        });
    }
    group.finish();
}

/// Stage 3 (TOTAL, includes an internal parse pass — see module doc):
/// `build_program_graph` — node/topology extraction into a `ProgramGraph`.
fn bench_build_program_graph_total(c: &mut Criterion) {
    let mut group = c.benchmark_group("engine_stage_build_program_graph_total");
    for &file_count in &[100usize, 1000] {
        let dir = corpus_dir(file_count);
        let snap = build_snapshot(dir.path());
        let cache = AbiCache::new();
        group.bench_function(format!("{file_count}_files"), |b| {
            b.iter(|| {
                let graph = build_program_graph(black_box(&snap), &cache);
                black_box(graph.routines.len());
            });
        });
    }
    group.finish();
}

/// Stage 4: `ResolveIndex::build` — the resolver's pre-built lookup indexes
/// over a `ProgramGraph` (object-by-number, routines-by-name, subscriber
/// wiring, etc). Part of the red-flag pair the T3 plan's Task 9 contingency
/// keys off (see this repo's `.superpowers/sdd/t3-stage-split.md`).
fn bench_resolve_index_build(c: &mut Criterion) {
    let mut group = c.benchmark_group("engine_stage_resolve_index_build");
    for &file_count in &[100usize, 1000] {
        let dir = corpus_dir(file_count);
        let snap = build_snapshot(dir.path());
        let graph = build_program_graph(&snap, &AbiCache::new());
        group.bench_function(format!("{file_count}_files"), |b| {
            b.iter(|| {
                let index = ResolveIndex::build(black_box(&graph));
                black_box(&index);
            });
        });
    }
    group.finish();
}

/// Stage 5: `DeclSurface::build` — maps every `RoutineNodeId` to its borrowed
/// `RoutineDecl` from the parsed IR. The other half of the red-flag pair.
fn bench_body_map_build(c: &mut Criterion) {
    let mut group = c.benchmark_group("engine_stage_body_map_build");
    for &file_count in &[100usize, 1000] {
        let dir = corpus_dir(file_count);
        let snap = build_snapshot(dir.path());
        let graph = build_program_graph(&snap, &AbiCache::new());
        let parsed = parse_snapshot(&snap);
        group.bench_function(format!("{file_count}_files"), |b| {
            b.iter(|| {
                let surface = DeclSurface::build(black_box(&graph), black_box(&parsed));
                black_box(&surface);
            });
        });
    }
    group.finish();
}

/// Stage 6 (TOTAL, see module doc): `resolve_full_program` — snapshot +
/// parse (again) + graph-build + index/body-map-build + the obligation
/// resolution inner loop + histogram computation, all from one workspace
/// root. `sample_size` is lowered because this is by far the heaviest
/// group (it rebuilds the entire pipeline from disk every iteration).
fn bench_resolve_full_program_total(c: &mut Criterion) {
    let mut group = c.benchmark_group("engine_stage_resolve_full_program_total");
    group.sample_size(20);
    for &file_count in &[100usize, 1000] {
        let dir = corpus_dir(file_count);
        group.bench_function(format!("{file_count}_files"), |b| {
            b.iter(|| {
                let report = resolve_full_program(black_box(dir.path())).unwrap();
                black_box(report.histogram.total);
            });
        });
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_snapshot,
    bench_parse,
    bench_build_program_graph_total,
    bench_resolve_index_build,
    bench_body_map_build,
    bench_resolve_full_program_total
);
criterion_main!(benches);

//! Library target for benchmarks. The binary continues to use `main.rs`.
//!
//! This file exists primarily so `criterion` benches can import telemetry
//! types via the crate name. Adding more modules here is fine, but be aware
//! that any module declared in both `lib.rs` and `main.rs` becomes a
//! duplicate compilation — keep this small.
//!
//! `config` is included alongside `telemetry` because `telemetry/mod.rs`
//! references `crate::config::TelemetryFileConfig`. Without it, the lib
//! crate cannot compile.

/// Code-quality metrics (cyclomatic complexity, quality score, findings) over
/// the owned IR (T3 Task 12 fix-wave): promoted from a binary-only `main.rs`
/// module to a library module because `src/lsp/lens.rs`/`diagnostics.rs`
/// need its `routine_complexity_ir`/`is_framework_invocation_attribute`
/// helpers. `main.rs` re-exports this alongside `config`/`telemetry`/`lsp`
/// (same pattern, see that comment below) instead of declaring its own
/// `mod analysis;`.
pub mod analysis;
pub mod app_package;
/// Shared big-stack execution for anywhere the `al_syntax` lowerer runs (T2.1,
/// stack-overflow hardening) — see the module doc.
pub mod big_stack;
/// Bounded-read helper shared by every zip/gzip decompression site (Task
/// T2.2, DoS hardening) — see the module doc.
pub mod capped_io;
/// No-op phase marks for the out-of-tree byte-census probe — see the module doc.
pub mod census_hook;
pub mod config;
pub mod dependencies;
pub mod engine;
/// Tree-sitter AL language bindings. Exposed from the library so additive
/// binaries (e.g. the R0 `aldump`) can parse without duplicating the `extern`
/// declaration. `main.rs` keeps its own `mod language;` for the LSP binary;
/// the duplicate compilation is benign and pre-existing in this repo.
pub mod language;
/// LSP-surface infrastructure for the program-engine-backed LSP server (the
/// T3 migration arc): position encoding, the def-surface/fingerprint model,
/// the `LspSnapshot`/`Updater`, and the request handlers/lens/diagnostics/
/// custom-request modules `server.rs` dispatches to. This IS the LSP
/// surface today — the legacy `graph`/`handlers`/`indexer`/`parser` pipeline
/// it replaced was deleted at T3 Task 17 (the differential harness that
/// licensed the deletion, and its CDO evidence, are recorded in CHANGELOG).
pub mod lsp;
pub mod program;
pub mod protocol;
/// The AL record-operation table both engines read.
pub mod record_ops;
pub mod snapshot;
/// The shared `.al` source decoder (lossy UTF-8, BOM dropped) — see the module doc.
pub mod source_text;
/// Per-user state locations, and the read-fallback that carries pre-rename installs.
pub mod state_paths;
pub mod telemetry;
/// Core AL object-type enum shared between lib and binary targets.
pub mod types;

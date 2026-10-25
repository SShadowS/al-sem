//! Umbrella test crate: LSP-surface + telemetry suites (test-crate
//! consolidation, 2026-07-15 spec).

#[path = "../common/cdo.rs"]
mod cdo;

#[path = "../common/symbol_app.rs"]
mod symbol_app;

#[path = "../common/dep_dispatch_fixture.rs"]
mod dep_dispatch_fixture;

mod l4_host_oracle;
mod lsp_incremental_parity;
mod perf_support_smoke;
mod program_graph;
mod snapshot_robustness;
// Was a crate-level `#![cfg(feature = "telemetry")]` when this file was its
// own crate; expressed here because inner attributes can't live in a
// non-root module.
#[cfg(feature = "telemetry")]
mod telemetry_integration;
mod telemetry_privacy_lint;

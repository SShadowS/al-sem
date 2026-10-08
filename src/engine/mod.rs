//! R0 migration engine — additive, isolated from the LSP binary.
//!
//! Everything under `engine` is part of the al-sem → Rust port and is gated by
//! the differential harness. It must not depend on or alter the LSP method
//! surface.

pub mod deps;
pub mod gate;
pub mod ids;
pub mod l4;
pub mod l5;
/// Permanent, env-gated performance tracing (spec 2026-07-18). Zero-cost when
/// `ALSEM_TRACE` is unset; emits a Chrome-Trace side file otherwise. See the
/// module doc for the disabled-path / crash-safety / threading contracts.
pub mod perf_trace;
pub mod return_summary;
pub mod root_classification;
pub mod snapshot;
pub mod switch_dump;

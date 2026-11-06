//! R0 migration engine — additive, isolated from the LSP binary.
//!
//! Everything under `engine` is part of the al-sem → Rust port and is gated by
//! the differential harness. It must not depend on or alter the LSP method
//! surface.

/// #277: the batch driver; standalone is a batch of one.
pub mod batch;
/// The retained byte census under `ALSEM_RETAINED_CENSUS=1` (#160 §4.1.5).
pub mod census;
pub mod deps;
/// Follow-ups in the asking surface's words (output validation §6.1).
pub mod follow_up;
pub mod gate;
/// The engine child process and its protocol (context-server stage 2).
pub mod host;
pub mod ids;
/// Agent-tasks B1: `alsem impact` (impact of a change, current revision).
pub mod impact;
/// #222: the incremental L4 host (`IncrementalHost`).
pub mod incremental;
pub(crate) mod joined_key;
pub mod l4;
pub mod l5;
/// The one line base of the engine-query surface (explain, prove, digest,
/// query, fingerprint, snapshots): 1-based, stated as `lineBase`.
pub mod line_base;
/// Permanent, env-gated performance tracing (spec 2026-07-18). Zero-cost when
/// `ALSEM_TRACE` is unset; emits a Chrome-Trace side file otherwise. See the
/// module doc for the disabled-path / crash-safety / threading contracts.
pub mod perf_trace;
/// The resident session of the context server (design §4-§5, stage 2): a
/// revision kept between requests, its artifact store, the manifest.
pub mod resident;
/// The four-line result header and `result.scope` (output validation §5.2).
pub mod result_header;
/// The complete compact result index beside a file answer (§5.2, F42).
pub mod result_index;
pub mod return_summary;
pub mod root_classification;
/// #160 step 2: `View`, `Plan`, `RunKey` (spec §4.4.3).
pub mod session;
pub mod snapshot;
/// W2-7: `--show-source` excerpts.
pub mod source_excerpt;
pub mod switch_dump;
pub mod test_select;
/// The output word table (output validation §8.1).
pub mod vocab;

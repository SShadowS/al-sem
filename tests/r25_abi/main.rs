//! Umbrella test crate: R2.5 ABI/dependency suites (test-crate consolidation, 2026-07-15 spec).
//! Engine-switch S9.6 deleted the R2.5a merged-index and R2.5b cross-app L3 suites with the
//! engine they measured; the ABI ingestion vectors remain.
#[path = "../common/regen.rs"]
mod regen;

mod r2_5a_abi_native_vectors;
mod r2_5a_attr_vectors;
mod r2_5a_stable_id_vectors;

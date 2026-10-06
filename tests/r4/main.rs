//! Umbrella test crate: R4 effect-summary suites (test-crate consolidation,
//! 2026-07-15 spec). One link target instead of seven.

#[path = "../common/regen.rs"]
mod regen;
#[path = "../common/symbol_app.rs"]
mod symbol_app;

mod b3_triage_r0;
mod r4_cross_app_program;
mod r4_differential;
mod r4_event_inventory;
mod r4f_digest_effects;
mod r4f_issue33_unmasked_physical_write;
mod r4f_ordering_facts;
mod r4f_return_summaries;
mod r4f_root_classifications;
mod r4f_scoped_guarantees;
mod r4f_snapshot;

//! Umbrella test crate: R3 obligation/trace suites (test-crate consolidation, 2026-07-15 spec).
#[path = "../common/regen.rs"]
mod regen;

#[path = "../common/temp_state_probe.rs"]
mod temp_state_probe;

mod r3_temp_class_parity;
mod r3_temp_class_parity_arena;

mod r3a1_differential;
mod r3a1_oracles;
mod r3a1_vectors;
mod r3a2_branch_aware;
mod r3a2_differential;
mod r3a2_implicit_rec_pin;
mod r3a2_oracles;
mod r3a2_vectors;
mod r3a3_differential;
mod r3a3_oracles;
mod r3a3_vectors;
mod r3a4_differential;
mod r3a4_oracles;
mod r3a4_vectors;
mod r3a5_differential;
mod r3a5_oracles;

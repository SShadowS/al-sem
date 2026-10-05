//! Umbrella test crate: L2/IR suites (test-crate consolidation, 2026-07-15 spec).

#[path = "../common/regen.rs"]
mod regen;

mod encoder_vectors;
mod ir_l2_snapshot;
mod ir_lowering_audit;
mod ir_robustness;
mod l2_receiver_oracles;
mod l2_vectors;
mod l2cap_oracles;
mod l2cap_vectors;
mod l2cc_oracles;
mod l2cc_vectors;
mod l2order_oracles;
mod l2order_vectors;
mod temp_rule_lint;

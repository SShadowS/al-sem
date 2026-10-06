//! Umbrella test crate: CLI/gate differential suites (test-crate
//! consolidation, 2026-07-15 spec).
//!
//! `ENV_LOCK` serializes the process-global `std::env` mutation in the
//! `cli_a_*` differentials (`ALCH_DRIVER_VERSION_OVERRIDE`). Under nextest
//! (process-per-test) the lock is uncontended; under plain `cargo test`
//! (libtest threads share this process) it is what makes those tests sound —
//! they raced even as separate files whenever one file ran multi-threaded.

use std::sync::{Mutex, MutexGuard, PoisonError};

#[path = "../common/regen.rs"]
mod regen;

pub static ENV_LOCK: Mutex<()> = Mutex::new(());

/// Hold this for the entire set-var → run → remove-var span of any test that
/// mutates process env. Poisoning-tolerant: a panicked holder must not
/// cascade-fail unrelated tests.
pub fn env_guard() -> MutexGuard<'static, ()> {
    ENV_LOCK.lock().unwrap_or_else(PoisonError::into_inner)
}

mod al2dump_smoke;
mod aldump_smoke;
mod cli_a_html_differential;
mod cli_a_json_differential;
mod cli_a_stats_differential;
mod cli_a_terminal_differential;
mod cli_a_with_evidence;
mod cli_analyze_coverage_ledger;
mod cli_analyze_program_calls;
mod cli_b_diff_differential;
mod cli_b_digest_differential;
mod cli_b_digest_exit_oracles;
mod cli_b_fingerprint_differential;
mod cli_b_fingerprint_oracles;
mod cli_b_prove_differential;
mod cli_b_snapshot_differential;
mod cli_c_cache_differential;
mod cli_c_events_differential;
mod cli_c_policy_differential;
mod cli_p1_enclosing_member;
mod cli_p1_inventory;
mod cli_query_differential;
mod cli_stderr_logger;
mod cli_workspace_source;
mod d1_downgraded_to_info_oracle;
mod gate_prsummary_differential;
mod gate_sarif_differential;
mod gate_suppress_baseline_differential;
mod temp_storage_failure;

//! Umbrella test crate: CLI/gate differential suites (test-crate
//! consolidation, 2026-07-15 spec).
//!
//! `ENV_LOCK` serializes the process-global `std::env` mutation in the
//! `cli_a_*` differentials (`ALCH_DRIVER_VERSION_OVERRIDE`). Under nextest
//! (process-per-test) the lock is uncontended; under plain `cargo test`
//! (libtest threads share this process) it is what makes those tests sound —
//! they raced even as separate files whenever one file ran multi-threaded.

use std::sync::{Mutex, MutexGuard, PoisonError};

#[path = "../common/symbol_app.rs"]
mod symbol_app;

#[path = "../common/regen.rs"]
mod regen;

#[path = "../common/temp_state_probe.rs"]
mod temp_state_probe;

#[path = "../common/dep_dispatch_fixture.rs"]
mod dep_dispatch_fixture;

pub static ENV_LOCK: Mutex<()> = Mutex::new(());

/// Hold this for the entire set-var → run → remove-var span of any test that
/// mutates process env. Poisoning-tolerant: a panicked holder must not
/// cascade-fail unrelated tests.
pub fn env_guard() -> MutexGuard<'static, ()> {
    ENV_LOCK.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Copy a directory tree (creating `dst`), for tests that edit a tempdir copy of a fixture.
pub(crate) fn copy_dir(src: &std::path::Path, dst: &std::path::Path) {
    std::fs::create_dir_all(dst).expect("mkdir");
    for entry in std::fs::read_dir(src).expect("read_dir") {
        let entry = entry.expect("dir entry");
        let to = dst.join(entry.file_name());
        if entry.file_type().expect("file type").is_dir() {
            copy_dir(&entry.path(), &to);
        } else {
            std::fs::copy(entry.path(), &to).expect("copy");
        }
    }
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
mod cli_attribute_trivia;
mod cli_b_diff_differential;
mod cli_b_digest_differential;
mod cli_b_digest_exit_oracles;
mod cli_b_fingerprint_differential;
mod cli_b_fingerprint_oracles;
mod cli_b_prove_differential;
mod cli_b_snapshot_differential;
mod cli_build_counters;
mod cli_c_cache_differential;
mod cli_c_events_differential;
mod cli_c_policy_differential;
mod cli_d35_unknown_commit;
mod cli_dependency_diagnostics;
mod cli_diff_strict;
mod cli_digest_changed_spans;
mod cli_digest_query_dependencies;
mod cli_digest_temp_state;
mod cli_envelope_detector_set;
mod cli_event_dispatch_first_hop;
mod cli_events_policy_dependencies;
mod cli_graph_export;
mod cli_implicit_trigger_cone;
mod cli_inputs_frozen;
mod cli_p1_enclosing_member;
mod cli_p1_inventory;
mod cli_query_differential;
mod cli_s6_consumers;
mod cli_sarif_rule_catalog;
mod cli_stderr_logger;
mod cli_workspace_fingerprint_closure;
mod cli_workspace_source;
mod d1_downgraded_to_info_oracle;
mod gate_prsummary_differential;
mod gate_sarif_differential;
mod gate_suppress_baseline_differential;
mod temp_storage_failure;

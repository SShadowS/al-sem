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

#[path = "../common/unreadable.rs"]
mod unreadable;

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

/// What `fingerprint`, `digest`, `prove` and `diff` read besides the build's
/// inputs (#160 §4.2.6): the snapshot hash rows. Harnesses that call those
/// pipelines read their `WorkspaceInputs` with it.
pub(crate) fn fingerprint_needs() -> al_sem::program::inputs::WorkspaceNeeds {
    al_sem::program::inputs::WorkspaceNeeds {
        fingerprint_rows: true,
        ..Default::default()
    }
}

mod al2dump_smoke;
mod aldump_smoke;
mod batch_records;
mod cli_11b_policy_collision;
mod cli_a_html_differential;
mod cli_a_json_differential;
mod cli_a_stats_differential;
mod cli_a_terminal_differential;
mod cli_a_with_evidence;
mod cli_analyze_call_hierarchy_json;
mod cli_analyze_coverage_ledger;
mod cli_analyze_program_calls;
mod cli_analyze_test_quality;
mod cli_array_param_overload;
mod cli_attribute_trivia;
mod cli_b_diff_differential;
mod cli_b_digest_differential;
mod cli_b_digest_exit_oracles;
mod cli_b_fingerprint_differential;
mod cli_b_fingerprint_oracles;
mod cli_b_prove_differential;
mod cli_b_snapshot_differential;
mod cli_batch_case_b;
mod cli_batch_census;
mod cli_batch_contract;
mod cli_batch_diff;
mod cli_batch_failures;
mod cli_batch_manifest;
mod cli_batch_merged_streams;
mod cli_batch_plan;
mod cli_batch_standalone;
mod cli_batch_traces;
mod cli_batch_workers;
mod cli_batch_writes;
mod cli_build_counters;
mod cli_c_cache_differential;
mod cli_c_events_differential;
mod cli_c_policy_differential;
mod cli_caller_owned_rec;
mod cli_d35_unknown_commit;
mod cli_d39_dependency_page;
mod cli_d39_helper_persists;
mod cli_dataitem_record_ops;
mod cli_decision7_precedence;
mod cli_dependency_diagnostics;
mod cli_dependency_lookup_page_run;
mod cli_dependency_roots;
mod cli_dependency_symbol_control_addin;
mod cli_diff_strict;
mod cli_digest_changed_spans;
mod cli_digest_include_reads;
mod cli_digest_query_dependencies;
mod cli_digest_temp_state;
mod cli_digest_test_coverage;
mod cli_dotnet_var_overload;
mod cli_envelope_detector_set;
mod cli_escaped_object_names;
mod cli_event_dispatch_first_hop;
mod cli_events_policy_dependencies;
mod cli_fingerprint_inventory_format;
mod cli_fingerprint_routine_cross_app;
mod cli_full_snapshot_split;
mod cli_graph_export;
mod cli_ids_and_line_bases;
mod cli_implicit_rec_argument;
mod cli_implicit_rec_bindings;
mod cli_implicit_trigger_cone;
mod cli_inputs_frozen;
mod cli_inventory_cross_app;
mod cli_lookup_page_run;
mod cli_loose_fingerprint;
mod cli_numberless_object_ids;
mod cli_object_display;
mod cli_object_ref_integer_arg;
mod cli_p1_enclosing_member;
mod cli_p1_inventory;
mod cli_parameterless_symbol_method;
mod cli_prove_closed_world;
mod cli_prove_cross_app;
mod cli_prove_dead_paths;
mod cli_prove_event_anchor;
mod cli_prove_field_guarded_write;
mod cli_prove_group_path;
mod cli_prove_interface_holder;
mod cli_prove_internal_interface;
mod cli_prove_loaded_subscribers;
mod cli_prove_platform_natives;
mod cli_prove_publisher_scoped;
mod cli_prove_reaches_ui;
mod cli_prove_table_branches;
mod cli_prove_table_writes;
mod cli_prove_temp_writes;
mod cli_prove_throws_error;
mod cli_prove_unknown_write_anchor;
mod cli_query_differential;
mod cli_record_integer_builtin_arg;
mod cli_recordref_open_temporary;
mod cli_s6_consumers;
mod cli_sarif_rule_catalog;
mod cli_secrettext_arg;
mod cli_snapshot_schema4;
mod cli_source_dependencies;
mod cli_split_object_readers;
mod cli_stderr_logger;
mod cli_step5_coverage_fold;
mod cli_strict_events_policy;
mod cli_this_argument;
mod cli_update_baseline_write;
mod cli_variant_type_tests;
mod cli_workspace_fingerprint_closure;
mod cli_workspace_source;
mod cli_write_failure;
mod d1_downgraded_to_info_oracle;
mod gate_prsummary_differential;
mod gate_sarif_differential;
mod gate_suppress_baseline_differential;
mod temp_storage_failure;

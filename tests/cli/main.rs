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

#[path = "../common/cdo.rs"]
mod cdo;

#[path = "../common/temp_state_probe.rs"]
mod temp_state_probe;

#[path = "../common/dep_dispatch_fixture.rs"]
mod dep_dispatch_fixture;

#[path = "../common/unreadable.rs"]
mod unreadable;

#[path = "../common/git.rs"]
mod git;

/// A digest 4.0.0 or query 4.0.0 document (output validation V5, §7.2,
/// §7.3) read back into its 3.x payload shape, through the documents' own
/// inverses (`digest_v4::to_v3`, `query_v4::to_v3`; exact, checked by
/// `output_shared_evidence`). The engine-fact tests read rows by their 3.x
/// fields; any other document is returned as it is.
pub(crate) fn as_v3(doc: serde_json::Value) -> serde_json::Value {
    let kind = doc["kind"].as_str().unwrap_or("").to_string();
    // Round-2 §4.6: a 5.0.0 document reads back through 4.0.0 first.
    let doc = match doc["schemaVersion"].as_str() {
        Some(al_sem::engine::l5::digest_v5::VERSION) if kind == "digest" => {
            al_sem::engine::l5::digest_v5::document_to_v4(doc)
        }
        Some(al_sem::engine::l4::query_v5::VERSION) if kind.starts_with("query.") => {
            al_sem::engine::l4::query_v5::document_to_v4(doc)
        }
        _ => doc,
    };
    match doc["schemaVersion"].as_str() {
        Some(al_sem::engine::l5::digest_v4::VERSION) if kind == "digest" => {
            al_sem::engine::l5::digest_v4::document_to_v3(doc)
        }
        Some(al_sem::engine::l4::query_v4::VERSION) if kind.starts_with("query.") => {
            al_sem::engine::l4::query_v4::document_to_v3(doc)
        }
        _ => doc,
    }
}

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

/// A scratch folder that lives for the whole test process, for data a `static` keeps
/// (a `OnceLock` cache). Never use a `tempfile::TempDir` there: a `static` is never
/// dropped, so the folder is never deleted. On 2026-11-06 such leaks (`output_cdo`'s
/// CDO dependency dump, ~12,000 files per `cdo-gate` run, and this crate's
/// `cli_implicit_trigger_cone` fixture) filled /tmp's inodes and broke every gate.
/// The folder is `<CARGO_TARGET_TMPDIR>/process-scratch/<name>/pid-<pid>`, created
/// empty; earlier processes' folders under `<name>` older than six hours are removed
/// first (a younger one may belong to a concurrent run).
pub(crate) fn process_scratch(name: &str) -> std::path::PathBuf {
    let root = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("process-scratch")
        .join(name);
    if let Ok(entries) = std::fs::read_dir(&root) {
        let stale = std::time::Duration::from_secs(6 * 3600);
        for e in entries.flatten() {
            let old = e
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.elapsed().ok())
                .is_some_and(|age| age > stale);
            if old {
                let _ = std::fs::remove_dir_all(e.path());
            }
        }
    }
    let dir = root.join(format!("pid-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create the process scratch folder");
    dir
}

/// A scoped thread's result, its panic carried on (with the thread's own
/// message) instead of turned into an `Err` unwrap. The CDO ratchets run their
/// independent one-shot `alsem` calls on scoped threads (`scripts/cdo-gate`'s
/// longest tests, 2026-11-09): only the test's own thread may call
/// `crate::cdo::cdo_ws_or_enforce`, whose coverage guard reads the thread name.
pub(crate) fn joined<T>(h: std::thread::ScopedJoinHandle<'_, T>) -> T {
    h.join().unwrap_or_else(|e| std::panic::resume_unwind(e))
}

/// `jobs` as one in-process batch (`al_sem::engine::batch::run_batch`, the
/// driver `alsem batch` and every standalone run use): each view is built
/// once for every job that reads it, and the jobs' plans are merged into one
/// prepare. Per job, in order: (exit, stdout, stderr). TLS-12: a test
/// comparing many cases of one workspace builds its model once instead of per
/// case. Panics when a job's stream file cannot be read (the batch creates
/// both before any write): an empty read would make two failed runs equal.
pub(crate) fn batch_in_process(
    jobs: &[&dyn al_sem::engine::batch::job::Job],
) -> Vec<(Option<u8>, Vec<u8>, Vec<u8>)> {
    use al_sem::engine::batch::{JobStreams, run_batch};
    let dir = tempfile::tempdir().unwrap();
    let streams: Vec<JobStreams> = (0..jobs.len())
        .map(|i| JobStreams {
            stdout: dir.path().join(format!("{i}.out")),
            stderr: dir.path().join(format!("{i}.err")),
        })
        .collect();
    let with: Vec<_> = jobs.iter().copied().zip(streams).collect();
    let lines = run_batch(0, &with);
    assert_eq!(lines.len(), jobs.len(), "one line per job");
    lines
        .into_iter()
        .map(|line| {
            let read = |p: &std::path::Path| {
                std::fs::read(p).unwrap_or_else(|e| panic!("job stream {}: {e}", p.display()))
            };
            (line.exit, read(&line.stdout), read(&line.stderr))
        })
        .collect()
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
mod cli_aldump_abi_ingest_errors;
mod cli_analyze_call_hierarchy_json;
mod cli_analyze_coverage_ledger;
mod cli_analyze_program_calls;
mod cli_analyze_test_quality;
mod cli_array_param_overload;
mod cli_attribute_trivia;
mod cli_b2_alternative_declarations;
mod cli_b2_arm_readers;
mod cli_b2_preproc_arms;
mod cli_b8_output;
mod cli_b9_fieldref_validate;
mod cli_b_diff_differential;
mod cli_b_digest_differential;
mod cli_b_digest_exit_oracles;
mod cli_b_fingerprint_differential;
mod cli_b_fingerprint_oracles;
mod cli_b_prove_differential;
mod cli_b_snapshot_differential;
mod cli_background_start_targets;
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
mod cli_commit_sites_up_chain;
mod cli_d35_unknown_commit;
mod cli_d39_dependency_page;
mod cli_d39_helper_persists;
mod cli_d65_guard_bypass;
mod cli_database_commit;
mod cli_dataitem_name_arg;
mod cli_dataitem_record_ops;
mod cli_dataset_operations;
mod cli_decision7_precedence;
mod cli_dependency_callers;
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
mod cli_dump_dependency_source;
mod cli_eng07_request_page;
mod cli_eng08_xmlport_import_error;
mod cli_eng09_checked_run_commit;
mod cli_eng10_write_in_try;
mod cli_eng12_record_ops;
mod cli_eng22_checked_run_pending_write;
mod cli_eng23_commit_arms;
mod cli_eng24_pending_writes;
mod cli_eng25_parenless_record_ops;
mod cli_eng26_protected_var_record_ops;
mod cli_eng27_d56_d14_arms;
mod cli_envelope_detector_set;
mod cli_error_boundary;
mod cli_escaped_object_names;
mod cli_event_dispatch_first_hop;
mod cli_events_policy_dependencies;
mod cli_explain_differential;
mod cli_explain_kind_places;
mod cli_explain_run_by;
mod cli_field_writers;
mod cli_fingerprint_inventory_format;
mod cli_fingerprint_rights;
mod cli_fingerprint_routine_cross_app;
mod cli_flowfield_reads;
mod cli_full_snapshot_split;
mod cli_git_env_isolation;
mod cli_graph_export;
mod cli_ids_and_line_bases;
mod cli_impact;
mod cli_impact_base;
mod cli_implicit_rec_argument;
mod cli_implicit_rec_bindings;
mod cli_implicit_trigger_cone;
mod cli_inputs_frozen;
mod cli_inventory_cross_app;
mod cli_lookup_page_run;
mod cli_loose_fingerprint;
mod cli_moved_table_resolution;
mod cli_named_return_record;
mod cli_numberless_object_ids;
mod cli_object_display;
mod cli_object_ref_integer_arg;
mod cli_overload_last_ambiguities;
mod cli_own_table_extension_method;
mod cli_p1_enclosing_member;
mod cli_p1_inventory;
mod cli_page_ui_permissions;
mod cli_parameterless_symbol_method;
mod cli_policy_pins;
mod cli_prove_closed_world;
mod cli_prove_commit_behavior;
mod cli_prove_cross_app;
mod cli_prove_dead_paths;
mod cli_prove_event_anchor;
mod cli_prove_field_guarded_write;
mod cli_prove_flow_values;
mod cli_prove_group_path;
mod cli_prove_interface_holder;
mod cli_prove_internal_interface;
mod cli_prove_literal_error_guards;
mod cli_prove_literal_routes;
mod cli_prove_loaded_subscribers;
mod cli_prove_platform_natives;
mod cli_prove_publisher_scoped;
mod cli_prove_reaches_ui;
mod cli_prove_recref_guard;
mod cli_prove_table_branches;
mod cli_prove_table_names;
mod cli_prove_table_writes;
mod cli_prove_temp_writes;
mod cli_prove_throws_error;
mod cli_prove_transpiled_guard;
mod cli_prove_traps;
mod cli_prove_unknown_write_anchor;
mod cli_prove_value_guards;
mod cli_query_differential;
mod cli_query_effects_buckets;
mod cli_query_field;
mod cli_receiver_temp_state;
mod cli_record_integer_builtin_arg;
mod cli_recordref_open_temporary;
mod cli_s6_consumers;
mod cli_sarif_rule_catalog;
mod cli_secrettext_arg;
mod cli_selector_round_trip;
mod cli_snapshot_memory;
mod cli_snapshot_schema4;
mod cli_source_dependencies;
mod cli_split_object_readers;
mod cli_stderr_logger;
mod cli_step5_coverage_fold;
mod cli_strict_events_policy;
mod cli_table_event_guards;
mod cli_table_event_subscribers;
mod cli_temp_blob_not_file;
mod cli_tests_selection;
mod cli_this_argument;
mod cli_typed_run_declared_procedure;
mod cli_up_parallel;
mod cli_update_baseline_write;
mod cli_variant_type_tests;
mod cli_workspace_fingerprint_closure;
mod cli_workspace_source;
mod cli_write_failure;
mod d1_downgraded_to_info_oracle;
mod engine_host;
mod gate_prsummary_differential;
mod gate_sarif_differential;
mod gate_scripts;
mod gate_suppress_baseline_differential;
mod ids_file_list_independence;
mod ids_parent_baseline;
mod mcp_composite;
mod mcp_envelope;
mod mcp_impact;
mod mcp_server;
mod mcp_tests;
mod output_cdo;
mod output_fixture;
mod output_follow_ups;
mod output_invariants;
mod output_invariants_w3;
mod output_shared_evidence;
mod output_vocabulary;
mod regen_atomic_writes;
mod temp_storage_failure;

//! `aldump <workspace>` — R0 differential-harness producer.
//! `aldump --l2 <workspace>` — R1a L2 features projection producer (Task 3).
//!
//! DEFAULT (no `--l2`): parses an AL workspace and emits the OBJECT/ROUTINE
//! IDENTITY SUBSET as JSON on stdout, in the EXACT shape of al-sem's committed
//! R0 "golden" files. R0 Task 5 diffs this output against those goldens; the
//! extraction logic (`engine::snapshot`) reproduces al-sem's identity derivation
//! precisely so the diff can pass.
//!
//! `--l2`: parses the workspace and emits the ALLOWLISTED L2 FEATURES PROJECTION
//! (`engine::l2::l2_workspace`) — objects + routines with metadata + per-routine
//! `features` (loops/operations/call-sites/record-ops/CFN skeleton/…), matching
//! the R1a goldens (`scripts/r1a-goldens/<fixture>.l2.golden.json`). Forbidden
//! later-gate / L3-resolved fields are structurally absent from the projection
//! types, so they can never appear here.
//!
//! DESIGN DEVIATION (R0, deliberate): the default mode emits the identity-subset
//! JSON directly rather than a v3-shaped CapabilitySnapshot — that subset carries
//! fields (routine sub-kind, `canonicalSignatureText`) a v3 envelope cannot.
//!
//! Output discipline: ONLY JSON goes to stdout; all logs/warnings go to stderr.
//! No absolute paths appear anywhere in the output.

use std::path::PathBuf;
use std::process::ExitCode;

use al_sem::engine::deps::cross_app_l3::build_cross_app_l3_from_workspace;
use al_sem::engine::deps::merged_index::{build_merged_index_from_path, serialize_projection};
use al_sem::engine::l2::l2_workspace::project_workspace;
use al_sem::engine::l3::l3_workspace::assemble_and_resolve_workspace_default;
use al_sem::engine::snapshot::snapshot_workspace;

/// modelInstanceId for the R2.5a merged-index emit. The emitted StableObjectId /
/// StableRoutineId are modelInstanceId-INDEPENDENT (R0), so this value never
/// reaches the output — it only feeds the internal routine id. Pinned to match the
/// al-sem dump's `MODEL_INSTANCE_ID` so any future internal-id surfacing stays
/// aligned.
const R2_5A_MODEL_INSTANCE_ID: &str = "r2.5a";

/// modelInstanceId for the R2.5b cross-app L3 emit (StableObjectId/StableRoutineId
/// are modelInstanceId-independent — R0; pinned to match the al-sem capture).
const R2_5B_MODEL_INSTANCE_ID: &str = "r2.5b";

fn usage() -> ExitCode {
    eprintln!(
        "usage: aldump [--l2 | --l3-record-types | --l3-call-graph | --l3-call-graph-stats | \
         --l3-call-graph-stats-cross-app | --l3-unknown-breakdown | --l3-unknown-breakdown-cross-app | \
         --l3-event-graph | --l3-coverage | --r2.5a-merged-index | --l3-cross-app | \
         --r3a1-combined-graph | --r3a2-summary-core | --r3a3-cone-coverage | \
         --r3a4-dep-hooks | --r3a5-cross-app-summary | --r4-findings | \
         --r4f-root-classifications | --r4f-return-summaries | --r4f-snapshot | \
         --r4f-digest-effects | --r4f-scoped-guarantees | --program-call-graph-stats | --b3 [--b3-deps] [--b3-triage <file.md>] | \
         --graphify-export | --graphify-export-fragments | --integration-points] \
         <workspace-or-.app>"
    );
    ExitCode::FAILURE
}

fn main() -> ExitCode {
    // Warnings go to stderr (a dropped dependency was once only a `warn!` that
    // nothing printed); `RUST_LOG` overrides the level. stdout is unchanged.
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    let mut l2 = false;
    let mut l3_record_types = false;
    let mut l3_call_graph = false;
    let mut l3_call_graph_stats = false;
    let mut l3_call_graph_stats_cross_app = false;
    let mut program_call_graph_stats = false;
    let mut b3 = false;
    let mut graphify_export = false;
    let mut graphify_export_fragments = false;
    let mut integration_points = false;
    let mut l3_unknown_breakdown = false;
    let mut l3_unknown_breakdown_cross_app = false;
    let mut l3_event_graph = false;
    let mut l3_coverage = false;
    let mut r2_5a_merged_index = false;
    let mut l3_cross_app = false;
    let mut r3a1_combined_graph = false;
    let mut r3a2_summary_core = false;
    let mut r3a3_cone_coverage = false;
    let mut r3a4_dep_hooks = false;
    let mut r3a5_cross_app_summary = false;
    let mut r4_findings = false;
    let mut r4f_root_classifications = false;
    let mut r4f_return_summaries = false;
    let mut r4f_snapshot = false;
    let mut r4f_digest_effects = false;
    let mut r4f_scoped_guarantees = false;
    let mut r4f_ordering_facts = false;
    let mut workspace_arg: Option<std::ffi::OsString> = None;
    // `--b3-triage <file.md>`: with `--b3`, also run the detector diff and
    // write its triage table there (the title is the file stem).
    let mut b3_triage: Option<std::path::PathBuf> = None;
    // `--b3-deps`: with `--b3-triage`, diff the adapter without its stage 2
    // (dependency-callee bindings) against the adapter with it.
    let mut b3_deps = false;

    let mut args = std::env::args_os().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--b3-triage" {
            let Some(path) = args.next() else {
                eprintln!("aldump: error: --b3-triage needs a file path");
                return usage();
            };
            b3_triage = Some(path.into());
            continue;
        }
        // `--l2` / `--l3-record-types` / `--l3-call-graph` / `--l3-event-graph` /
        // `--l3-coverage` / `--r2.5a-merged-index` flags (anywhere); else the
        // single positional.
        if arg == "--l2" {
            l2 = true;
            continue;
        }
        if arg == "--l3-record-types" {
            l3_record_types = true;
            continue;
        }
        if arg == "--l3-call-graph" {
            l3_call_graph = true;
            continue;
        }
        if arg == "--l3-call-graph-stats" {
            l3_call_graph_stats = true;
            continue;
        }
        if arg == "--l3-call-graph-stats-cross-app" {
            l3_call_graph_stats_cross_app = true;
            continue;
        }
        if arg == "--graphify-export" {
            graphify_export = true;
            continue;
        }
        if arg == "--graphify-export-fragments" {
            graphify_export_fragments = true;
            continue;
        }
        if arg == "--integration-points" {
            integration_points = true;
            continue;
        }
        if arg == "--l3-unknown-breakdown" {
            l3_unknown_breakdown = true;
            continue;
        }
        if arg == "--l3-unknown-breakdown-cross-app" {
            l3_unknown_breakdown_cross_app = true;
            continue;
        }
        if arg == "--l3-event-graph" {
            l3_event_graph = true;
            continue;
        }
        if arg == "--l3-coverage" {
            l3_coverage = true;
            continue;
        }
        if arg == "--r2.5a-merged-index" {
            r2_5a_merged_index = true;
            continue;
        }
        if arg == "--l3-cross-app" {
            l3_cross_app = true;
            continue;
        }
        if arg == "--r3a1-combined-graph" {
            r3a1_combined_graph = true;
            continue;
        }
        if arg == "--r3a2-summary-core" {
            r3a2_summary_core = true;
            continue;
        }
        if arg == "--r3a3-cone-coverage" {
            r3a3_cone_coverage = true;
            continue;
        }
        if arg == "--r3a4-dep-hooks" {
            r3a4_dep_hooks = true;
            continue;
        }
        if arg == "--r3a5-cross-app-summary" {
            r3a5_cross_app_summary = true;
            continue;
        }
        if arg == "--r4-findings" {
            r4_findings = true;
            continue;
        }
        if arg == "--r4f-root-classifications" {
            r4f_root_classifications = true;
            continue;
        }
        if arg == "--r4f-return-summaries" {
            r4f_return_summaries = true;
            continue;
        }
        if arg == "--r4f-snapshot" {
            r4f_snapshot = true;
            continue;
        }
        if arg == "--r4f-digest-effects" {
            r4f_digest_effects = true;
            continue;
        }
        if arg == "--r4f-scoped-guarantees" {
            r4f_scoped_guarantees = true;
            continue;
        }
        if arg == "--r4f-ordering-facts" {
            r4f_ordering_facts = true;
            continue;
        }
        if arg == "--program-call-graph-stats" {
            program_call_graph_stats = true;
            continue;
        }
        if arg == "--b3" {
            b3 = true;
            continue;
        }
        if arg == "--b3-deps" {
            b3_deps = true;
            continue;
        }
        if workspace_arg.is_some() {
            eprintln!("aldump: error: more than one workspace argument");
            return usage();
        }
        workspace_arg = Some(arg);
    }

    if [
        l2,
        l3_record_types,
        l3_call_graph,
        l3_call_graph_stats,
        l3_call_graph_stats_cross_app,
        l3_unknown_breakdown,
        l3_unknown_breakdown_cross_app,
        l3_event_graph,
        l3_coverage,
        r2_5a_merged_index,
        l3_cross_app,
        r3a1_combined_graph,
        r3a2_summary_core,
        r3a3_cone_coverage,
        r3a4_dep_hooks,
        r3a5_cross_app_summary,
        r4_findings,
        r4f_root_classifications,
        r4f_return_summaries,
        r4f_snapshot,
        r4f_digest_effects,
        r4f_scoped_guarantees,
        r4f_ordering_facts,
        program_call_graph_stats,
        b3,
        // T4-B: these three each guard their own dedicated `if`-block (like every
        // flag above) but were missing from this array — a combo like
        // `--graphify-export --l3-call-graph` silently ran whichever block's `if`
        // happened to come first in source order and dropped the other flag.
        graphify_export,
        graphify_export_fragments,
        integration_points,
    ]
    .iter()
    .filter(|f| **f)
    .count()
        > 1
    {
        eprintln!(
            "aldump: error: --l2 / --l3-record-types / --l3-call-graph / --l3-call-graph-stats / \
             --l3-call-graph-stats-cross-app / --l3-unknown-breakdown / \
             --l3-event-graph / --l3-coverage / --r2.5a-merged-index / --l3-cross-app / \
             --r3a1-combined-graph / --r3a2-summary-core / --r3a3-cone-coverage / \
             --r3a4-dep-hooks / --r3a5-cross-app-summary / --r4f-return-summaries / \
             --program-call-graph-stats / --b3 / \
             --graphify-export / --graphify-export-fragments / --integration-points are mutually exclusive"
        );
        return usage();
    }

    let Some(workspace_arg) = workspace_arg else {
        return usage();
    };
    let workspace = PathBuf::from(workspace_arg);

    if r3a4_dep_hooks {
        // R3a-4 DEP-HOOK PROJECTION: read the workspace's `.alpackages` dep `.app`(s),
        // build each dep's embedded-source PRODUCER artifact, drive the CONSUMER hooks
        // (inject_intra_app_call_edges / collect_cited_dep_evidence /
        // collect_dep_order_index) over a merged model whose routine membership =
        // workspace own routines + every dep's own routines, then STABLE-PROJECT every
        // id-bearing field (appGuid:Type:Num#sigHash — cache/modelInstanceId-independent)
        // and emit the producer payloads + consumed effect in the SAME stable shape /
        // key-order as the al-sem `cross-app-dep-hooks.r3a4.golden.json`. CAPTURE POINT:
        // post-inject/collect hooks; the R3a-5 cross-app cone is NOT projected here.
        //
        // `project_r3a4_from_workspace` itself is "engine-never-throws" (a missing dep
        // ledger is a legitimate empty answer, and several differential/oracle tests
        // call it directly expecting that always-succeeds shape) — it has no signal
        // for "the workspace itself is unusable". Task T0.1: gate that ONE genuine
        // failure mode at the CLI boundary with the SAME predicate every other L3-based
        // mode already uses, without touching the library function's tested contract.
        if assemble_and_resolve_workspace_default(&workspace).is_none() {
            eprintln!(
                "aldump: error: fail-closed/empty layout at {} — cannot compute R3a-4 dep-hook projection",
                workspace.display()
            );
            return ExitCode::FAILURE;
        }
        let projection = al_sem::engine::deps::r3a4_projection::project_r3a4_from_workspace(
            &workspace,
            "cross-app-dep-hooks",
        );
        return match serde_json::to_string_pretty(&projection) {
            Ok(json) => {
                println!("{json}");
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("aldump: error: failed to serialize R3a-4 dep-hook projection: {e}");
                ExitCode::FAILURE
            }
        };
    }

    if r3a5_cross_app_summary {
        // R3a-5 CROSS-APP FULL SUMMARY (the final R3a sub-gate): run the FULL
        // cross-app L4 path over the workspace + its `.alpackages` dep `.app`(s) WITH
        // the R3a-4 dep hooks — merged index → buildCombinedGraph →
        // injectIntraAppCallEdges → computeSummaries → the cone — and project EVERY
        // routine's FULL RoutineSummary (R3a-2 core + R3a-3 cone/coverage) in the SAME
        // stable shape/key-order as the al-sem `cross-app-full-summary.r3a5.golden.json`.
        // The dep routines arrive EMPTY-featured with a RETAINED summary + direct facts;
        // the injected intra-app typed edges let the cone propagate the dep's Insert
        // capabilityFactsDirect to the PRIMARY caller's capabilityFactsInherited.
        // CAPTURE POINT: post-computeSummaries WITH dep hooks. Fail-closed → empty.
        //
        // Same T0.1 gate as `--r3a4-dep-hooks` above: `project_r3a5_cross_app` itself
        // stays engine-never-throws (zero deps is legitimate, and its `empty` fallback
        // is exercised directly by differential/oracle tests), so the ONE genuine
        // failure — an unbuildable primary workspace — is caught at the CLI boundary
        // with the same predicate every other L3-based mode uses.
        if assemble_and_resolve_workspace_default(&workspace).is_none() {
            eprintln!(
                "aldump: error: fail-closed/empty layout at {} — cannot compute R3a-5 cross-app summary",
                workspace.display()
            );
            return ExitCode::FAILURE;
        }
        let projection = al_sem::engine::l4::capability_cone::project_r3a5_cross_app(
            &workspace,
            "r0",
            "cross-app-full-summary",
        );
        return match serde_json::to_string_pretty(&projection) {
            Ok(json) => {
                println!("{json}");
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!(
                    "aldump: error: failed to serialize R3a-5 cross-app summary projection: {e}"
                );
                ExitCode::FAILURE
            }
        };
    }

    if r3a2_summary_core {
        // R3a-2 SUMMARY CORE: run the SOURCE-ONLY L0→L3 pipeline → buildCombinedGraph
        // → tarjanScc → computeSummaries (the JACOBI fixed point), then project the
        // RoutineSummary CORE (dbEffects / uncertainties / parameterRoles /
        // inRecursiveCycle / hasUnresolvedCalls) in the SAME stable shape/key-order as
        // the al-sem `<fixture>.r3a2.golden.json`. CAPTURE POINT: POST-computeSummaries;
        // NO dep hooks (R3a-4); the cone/coverage (R3a-3) are never declared on the
        // projected types.
        //
        // Task T0.1: a fail-closed/empty layout is a genuine tool failure, not a
        // legitimate empty answer — exits non-zero with no stdout output.
        let Some(resolved) = assemble_and_resolve_workspace_default(&workspace) else {
            eprintln!(
                "aldump: error: fail-closed/empty layout at {} — cannot compute R3a-2 summary-core projection",
                workspace.display()
            );
            return ExitCode::FAILURE;
        };
        let projection = al_sem::engine::l4::summary::project_r3a2(&resolved);
        return match serde_json::to_string_pretty(&projection) {
            Ok(json) => {
                println!("{json}");
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("aldump: error: failed to serialize R3a-2 summary-core projection: {e}");
                ExitCode::FAILURE
            }
        };
    }

    if r3a3_cone_coverage {
        // R3a-3 CAPABILITY CONE + COVERAGE: run the SOURCE-ONLY L0→L3 pipeline, then
        // the cone/coverage pass (direct capability extraction over the resolved
        // features + the publisher-fact injection → composeInheritedCones), and emit
        // the stable projection (capabilityFactsDirect / capabilityFactsInherited /
        // coverage per routine) in the SAME shape/key-order as the al-sem
        // `<fixture>.r3a3.golden.json`. CAPTURE POINT: POST-computeSummaries cone pass;
        // NO dep hooks (R3a-4).
        //
        // Task T0.1: a fail-closed/empty layout is a genuine tool failure — exits
        // non-zero with no stdout output.
        let Some(resolved) = assemble_and_resolve_workspace_default(&workspace) else {
            eprintln!(
                "aldump: error: fail-closed/empty layout at {} — cannot compute R3a-3 cone+coverage projection",
                workspace.display()
            );
            return ExitCode::FAILURE;
        };
        let projection = al_sem::engine::l4::capability_cone::project_r3a3(&resolved);
        return match serde_json::to_string_pretty(&projection) {
            Ok(json) => {
                println!("{json}");
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("aldump: error: failed to serialize R3a-3 cone+coverage projection: {e}");
                ExitCode::FAILURE
            }
        };
    }

    if r4_findings {
        // R4 FINDINGS: run the SOURCE-ONLY L0→L3 pipeline, then the L5 harness
        // (build_detector_context → run_detectors over the registered detectors →
        // stable projection) and emit the R4FindingsProjection in the SAME
        // shape/key-order as the al-sem `<fixture>.r4.golden.json`. Only the ported
        // detectors are registered, so the projection carries their subset.
        //
        // Task T0.1: a fail-closed/empty layout is a genuine tool failure — exits
        // non-zero with no stdout output.
        let fixture_name = workspace
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let detectors = al_sem::engine::l5::detectors::registered_detectors();
        let detector_names: Vec<String> = detectors.iter().map(|d| d.name.clone()).collect();
        let Some(resolved) = assemble_and_resolve_workspace_default(&workspace) else {
            eprintln!(
                "aldump: error: fail-closed/empty layout at {} — cannot compute R4 findings projection",
                workspace.display()
            );
            return ExitCode::FAILURE;
        };
        let projection = al_sem::engine::l5::finding::project_r4_findings(
            &resolved,
            &detectors,
            &fixture_name,
            &detector_names,
        );
        return match serde_json::to_string_pretty(&projection) {
            Ok(json) => {
                println!("{json}");
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("aldump: error: failed to serialize R4 findings projection: {e}");
                ExitCode::FAILURE
            }
        };
    }

    if r4f_return_summaries {
        // R4-F RETURN SUMMARIES: run the SOURCE-ONLY L0→L3 pipeline, then compute
        // per-routine returnability summaries (spec §J5), and emit the stable
        // projection in the SAME shape/key-order as the al-sem
        // `<fixture>.returnsummary.golden.json`.
        //
        // Task T0.1: a fail-closed/empty layout is a genuine tool failure — exits
        // non-zero with no stdout output.
        let fixture_name = workspace
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let Some(resolved) = assemble_and_resolve_workspace_default(&workspace) else {
            eprintln!(
                "aldump: error: fail-closed/empty layout at {} — cannot compute R4-F return-summary projection",
                workspace.display()
            );
            return ExitCode::FAILURE;
        };
        let projection =
            al_sem::engine::return_summary::project_r4f_return_summaries(&resolved, &fixture_name);
        return match serde_json::to_string_pretty(&projection) {
            Ok(mut json) => {
                json.push('\n');
                print!("{json}");
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("aldump: error: failed to serialize R4-F return-summary projection: {e}");
                ExitCode::FAILURE
            }
        };
    }

    if r4f_snapshot {
        // R4-F SNAPSHOT (Stage-2b): run the SOURCE-ONLY L0→L3 pipeline, then compose
        // + project the CapabilitySnapshot CONSUMED-CORE (composeSnapshot's
        // ordering-facts subset) in the SAME shape/key-order as the al-sem
        // `<fixture>.snapshot.golden.json`. The projection re-projects the R3a
        // source-only base (cone facts / typed edges / event graph / coverage /
        // root classifications).
        //
        // Task T0.1: a fail-closed/empty layout is a genuine tool failure — exits
        // non-zero with no stdout output.
        let fixture_name = workspace
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let Some(resolved) = assemble_and_resolve_workspace_default(&workspace) else {
            eprintln!(
                "aldump: error: fail-closed/empty layout at {} — cannot compute R4-F snapshot projection",
                workspace.display()
            );
            return ExitCode::FAILURE;
        };
        let json = al_sem::engine::l5::snapshot::project_r4f_snapshot(&resolved, &fixture_name);
        // `project_r4f_snapshot` already appends a trailing newline.
        print!("{json}");
        return ExitCode::SUCCESS;
    }

    if r4f_digest_effects {
        // R4-F DIGEST EFFECTS (Stage-3b): run the SOURCE-ONLY L0→L3 pipeline, compose
        // the CapabilitySnapshot, then run the digest witness + effects + occurrence-build
        // path per reportable root, emitting the per-root DigestEffectResult[] (each with a
        // stable occurrenceId = factId) in the SAME shape/key-order as the al-sem
        // `<fixture>.digest.golden.json`.
        //
        // Task T0.1: a fail-closed/empty layout is a genuine tool failure — exits
        // non-zero with no stdout output.
        let fixture_name = workspace
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let Some(resolved) = assemble_and_resolve_workspace_default(&workspace) else {
            eprintln!(
                "aldump: error: fail-closed/empty layout at {} — cannot compute R4-F digest-effects projection",
                workspace.display()
            );
            return ExitCode::FAILURE;
        };
        let json = al_sem::engine::l5::digest::project_r4f_digest_effects(&resolved, &fixture_name);
        // `project_r4f_digest_effects` already appends a trailing newline.
        print!("{json}");
        return ExitCode::SUCCESS;
    }

    if r4f_scoped_guarantees {
        // R4-F SCOPED GUARANTEES (Stage-4): run the SOURCE-ONLY L0→L3 pipeline, compose
        // the CapabilitySnapshot, compute return summaries + isolated event ids, run the
        // digest + ORDERING-ENGINE path, and emit the per-root per-effect scopedGuarantees
        // (filtered to the 5 RELEVANT labels) in the al-sem `<fixture>.scoped.golden.json`
        // shape.
        //
        // Task T0.1: a fail-closed/empty layout is a genuine tool failure — exits
        // non-zero with no stdout output.
        let fixture_name = workspace
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let Some(resolved) = assemble_and_resolve_workspace_default(&workspace) else {
            eprintln!(
                "aldump: error: fail-closed/empty layout at {} — cannot compute R4-F scoped-guarantees projection",
                workspace.display()
            );
            return ExitCode::FAILURE;
        };
        let json =
            al_sem::engine::l5::digest::project_r4f_scoped_guarantees(&resolved, &fixture_name);
        print!("{json}");
        return ExitCode::SUCCESS;
    }

    if r4f_ordering_facts {
        // R4-F ORDERING FACTS (Stage-5b, M5): run the SOURCE-ONLY L0→L3 pipeline, then
        // the ordering-facts facade (compute_ordering_facts: composeSnapshot → return
        // summaries → isolated events → digest+ordering → resolve each scopedGuarantee
        // to its IO/write/commit anchors) and emit the per-routine resolved OrderingFact[]
        // in the al-sem `<fixture>.orderingfacts.golden.json` shape.
        //
        // Task T0.1: a fail-closed/empty layout is a genuine tool failure — exits
        // non-zero with no stdout output.
        let fixture_name = workspace
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let Some(resolved) = assemble_and_resolve_workspace_default(&workspace) else {
            eprintln!(
                "aldump: error: fail-closed/empty layout at {} — cannot compute R4-F ordering-facts projection",
                workspace.display()
            );
            return ExitCode::FAILURE;
        };
        let json = al_sem::engine::l5::ordering_facts::project_r4f_ordering_facts(
            &resolved,
            &fixture_name,
        );
        print!("{json}");
        return ExitCode::SUCCESS;
    }

    if r4f_root_classifications {
        // R4-F ROOT CLASSIFICATIONS: run the SOURCE-ONLY L0→L3 pipeline (which now
        // classifies AST roots + overlays `<workspace>/roots.config.json`), then
        // emit the STABLE RootClassification projection in the SAME shape/key-order
        // as the al-sem `<fixture>.rootclass.golden.json`.
        //
        // Task T0.1: a fail-closed/empty layout is a genuine tool failure — exits
        // non-zero with no stdout output.
        let fixture_name = workspace
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let Some(resolved) = assemble_and_resolve_workspace_default(&workspace) else {
            eprintln!(
                "aldump: error: fail-closed/empty layout at {} — cannot compute R4-F root-classification projection",
                workspace.display()
            );
            return ExitCode::FAILURE;
        };
        let projection = al_sem::engine::root_classification::project_r4f_root_classifications(
            &resolved,
            &fixture_name,
        );
        return match serde_json::to_string_pretty(&projection) {
            Ok(json) => {
                println!("{json}");
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!(
                    "aldump: error: failed to serialize R4-F root-classification projection: {e}"
                );
                ExitCode::FAILURE
            }
        };
    }

    if r3a1_combined_graph {
        // R3a-1 L4 GRAPH SUBSTRATE: run the SOURCE-ONLY L0→L3 pipeline, then
        // buildCombinedGraph → tarjanScc → projectR3a1, and emit the stable R3a-1
        // projection (combinedEdges + uncertaintyEdges + typedEdges + the
        // reverse-topo SCC list) in the SAME shape/key-order as the al-sem
        // `<fixture>.r3a1.golden.json`. CAPTURE POINT: POST-buildCombinedGraph /
        // POST-tarjanScc / PRE-computeSummaries — NO dep hooks, NO summaries (R3a-2+).
        //
        // Task T0.1: a fail-closed/empty layout is a genuine tool failure — exits
        // non-zero with no stdout output.
        let Some(resolved) = assemble_and_resolve_workspace_default(&workspace) else {
            eprintln!(
                "aldump: error: fail-closed/empty layout at {} — cannot compute R3a-1 projection",
                workspace.display()
            );
            return ExitCode::FAILURE;
        };
        let projection = resolved.project_r3a1_combined_graph();
        return match serde_json::to_string_pretty(&projection) {
            Ok(json) => {
                println!("{json}");
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("aldump: error: failed to serialize R3a-1 projection: {e}");
                ExitCode::FAILURE
            }
        };
    }

    if l3_cross_app {
        // R2.5b cross-app L3 SMOKE: read the workspace `.al` source + its dep `.app`(s)
        // under `<workspace>/.alpackages`, build the merged index, run the unchanged L3
        // pipeline over workspace+deps, and emit the four cross-app surfaces (record
        // types / call graph / event graph / coverage) as one JSON envelope. Task 1
        // proves the pipeline RUNS + produces NON-EMPTY cross-app resolution; Tasks 2-5
        // add the per-surface byte-goldens + matrices.
        //
        // Task T0.1: `None` here is exclusively a primary-workspace-unbuildable
        // failure (see the identical note on `--l3-call-graph-stats-cross-app` above)
        // — a genuine tool/layout failure, not a legitimate empty answer. Exits
        // non-zero with no stdout output.
        let Some(cross) = build_cross_app_l3_from_workspace(&workspace, R2_5B_MODEL_INSTANCE_ID)
        else {
            eprintln!(
                "aldump: error: fail-closed/empty layout at {} — primary workspace not buildable",
                workspace.display()
            );
            return ExitCode::FAILURE;
        };
        let envelope = serde_json::json!({
            "recordTypes": cross.project_record_types(),
            "callGraph": cross.project_call_graph(),
            "eventGraph": cross.project_event_graph(),
            "coverage": cross.project_coverage_disk(&workspace),
        });
        return match serde_json::to_string_pretty(&envelope) {
            Ok(json) => {
                println!("{json}");
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("aldump: error: failed to serialize cross-app envelope: {e}");
                ExitCode::FAILURE
            }
        };
    }

    if r2_5a_merged_index {
        // R2.5a merged-index projection: read the `.app`(s) at `workspace` (a single
        // `.app` OR a dir/`.alpackages` of them), project + merge (incl. the
        // extension-field merge — the post-resolveModel capture-point invariant),
        // and emit the dependency-entity subset in the SAME stable JSON shape as the
        // al-sem `*.r2.5a.golden.json`. NO cross-app L3 resolution (that is R2.5b).
        // `workspace` here is a single `.app` file OR a dir/`.alpackages` of them —
        // `build_merged_index_from_path` legitimately yields an all-empty projection
        // for a valid, dep-less path (never throws; several oracle/differential
        // tests call it directly expecting that contract), so it is NOT gated the
        // same way as the L3-workspace-based modes above. Output is byte-stable
        // (serialize_projection appends the trailing newline to match the TS
        // goldens).
        //
        // Task T0.1: the ONE thing this mode CAN distinguish as a genuine tool
        // failure is a `workspace` argument that doesn't exist on disk at all
        // (neither a readable `.app` file nor a directory) — gate that at the CLI
        // boundary without touching the library function's tested contract.
        if !workspace.exists() {
            eprintln!(
                "aldump: error: path does not exist: {} — cannot compute R2.5a merged-index projection",
                workspace.display()
            );
            return ExitCode::FAILURE;
        }
        let projection = build_merged_index_from_path(&workspace, R2_5A_MODEL_INSTANCE_ID);
        print!("{}", serialize_projection(&projection));
        return ExitCode::SUCCESS;
    }

    if l3_coverage {
        // L3 coverage projection (R2d): the resolved model's AnalysisCoverage —
        // sourceUnitsTotal/Parsed, routinesTotal/BodyAvailable, parseIncomplete
        // (StableRoutineId[]), opaqueApps (empty source-only), unresolvedCallsites
        // (StableCallsiteId multiset), dynamicDispatchSites (StableOperationId
        // multiset). Runs assemble→resolve→project_coverage_disk (reads the resolved
        // call graph + L2 routine flags; the post-resolve read the dump captures).
        //
        // Task T0.1: a fail-closed/empty layout is a genuine tool failure — exits
        // non-zero with no stdout output.
        let Some(resolved) = assemble_and_resolve_workspace_default(&workspace) else {
            eprintln!(
                "aldump: error: fail-closed/empty layout at {} — cannot compute L3 coverage projection",
                workspace.display()
            );
            return ExitCode::FAILURE;
        };
        let projection = resolved.project_coverage_disk(&workspace);
        return match serde_json::to_string_pretty(&projection) {
            Ok(json) => {
                println!("{json}");
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("aldump: error: failed to serialize L3 coverage projection: {e}");
                ExitCode::FAILURE
            }
        };
    }

    if l3_event_graph {
        // L3 event-graph projection (R2c): the resolved event graph — EventSymbols
        // (publishers + synthesized maybe/unknown) + EventEdges (subscribers,
        // open-world) — in stable id form. Runs assemble→resolve→build_event_graph
        // →project_event_graph (reads model.eventGraph; never re-runs the builder
        // for a later gate).
        //
        // Task T0.1: a fail-closed/empty layout is a genuine tool failure — exits
        // non-zero with no stdout output.
        let Some(resolved) = assemble_and_resolve_workspace_default(&workspace) else {
            eprintln!(
                "aldump: error: fail-closed/empty layout at {} — cannot compute L3 event-graph projection",
                workspace.display()
            );
            return ExitCode::FAILURE;
        };
        let projection = resolved.project_event_graph();
        return match serde_json::to_string_pretty(&projection) {
            Ok(json) => {
                println!("{json}");
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("aldump: error: failed to serialize L3 event-graph projection: {e}");
                ExitCode::FAILURE
            }
        };
    }

    if l3_call_graph_stats {
        // Honest-taxonomy histogram (spec §6/§8): bucket every resolved call edge
        // by ResolutionClass + report the LEGACY L3 real-`unknown` rate. Read-only
        // over the resolved edges (the same capture `--l3-call-graph` uses).
        //
        // Task T0.4: this is the legacy L3 engine, advisory only — the
        // authoritative metric is `aldump --program-call-graph-stats`
        // (`resolve_full_program`). The emitted rate key is `legacyL3UnknownRate`
        // (renamed from `realUnknownRate`, which that key's DIFFERENT semantics —
        // excluding memberNotFound/ambiguous — made unsafe to compare against the
        // fresh resolver's own `realUnknownRate`) plus an `advisory` field naming
        // the authoritative command. See CLAUDE.md's "Project Direction & The
        // Moat".
        //
        // FAIL-CLOSED IS FATAL HERE (Task T0.1): even though this surface is
        // advisory, `legacyL3UnknownRate` on an unusable workspace must not
        // silently read as a perfect 0.0. A `None` layout is a genuine
        // tool/layout failure, not a legitimate empty answer, so it exits
        // non-zero with NO stdout output (never a `Histogram::default()`
        // masquerading as a real result).
        use al_sem::engine::l3::call_resolver::{DeclaredDependency, resolve_calls};
        use al_sem::engine::l3::resolution_class::Histogram;
        use al_sem::engine::l3::symbol_table::SymbolTable;

        let Some(resolved) = assemble_and_resolve_workspace_default(&workspace) else {
            eprintln!(
                "aldump: error: fail-closed/empty layout at {} — cannot compute call-graph stats",
                workspace.display()
            );
            return ExitCode::FAILURE;
        };
        let ws = &resolved.workspace;
        let symbols = SymbolTable::build(&ws.objects, &ws.tables, &ws.routines);
        let no_deps: Vec<DeclaredDependency> = Vec::new();
        let no_fetched: Vec<String> = Vec::new();
        let r = resolve_calls(ws, &symbols, &no_deps, &no_fetched);
        let histogram = Histogram::of_edges(&r.edges);
        let mut value = serde_json::to_value(histogram).unwrap_or(serde_json::json!({}));
        if let Some(obj) = value.as_object_mut() {
            obj.insert(
                "legacyL3UnknownRate".to_string(),
                serde_json::json!(histogram.real_unknown_rate()),
            );
            obj.insert(
                "advisory".to_string(),
                serde_json::json!(
                    "legacy L3 engine; authoritative metric is --program-call-graph-stats"
                ),
            );
        }
        return match serde_json::to_string_pretty(&value) {
            Ok(json) => {
                println!("{json}");
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("aldump: error: failed to serialize call-graph stats: {e}");
                ExitCode::FAILURE
            }
        };
    }

    if l3_call_graph_stats_cross_app {
        // Deps-loaded PRIMARY-SCOPED honest-taxonomy histogram (spec §6/§8): build
        // the cross-app merged model (workspace + dep `.app`s), run call resolution
        // with the REAL declared/fetched dep ledger, then bucket the edges whose
        // `from` routine is a PRIMARY (workspace) routine — i.e. NOT in the dep set
        // (same role oracle as the L5 detectors: `dep_routine_ids = {r | r.app_guid
        // ∈ fetched_app_guids}`). This is the HONEST whole-program metric: dep
        // symbols are present for resolution, but the rate is measured over WORKSPACE
        // call sites only (dep-internal calls don't inflate the denominator). Emits
        // the same JSON shape as `--l3-call-graph-stats` plus `depAppsLoaded`.
        // If the workspace has no deps / fails to build, emits a clear message and
        // exits cleanly (fail-closed). ADDITIVE — does not change source-only path.
        use al_sem::engine::l3::call_resolver::{DeclaredDependency, resolve_calls};
        use al_sem::engine::l3::resolution_class::Histogram;
        use al_sem::engine::l3::symbol_table::SymbolTable;
        use std::collections::HashSet;

        match build_cross_app_l3_from_workspace(&workspace, R2_5B_MODEL_INSTANCE_ID) {
            // FATAL (Task T0.1, R2): `None` here means the PRIMARY workspace itself
            // could not be assembled (see `build_cross_app_l3_impl` — the `?` on
            // `assemble_l3_workspace_from_disk` is the ONLY `None` producer; a
            // workspace with zero deps still returns `Some`). That is a genuine
            // tool/layout failure, never a legitimate "no deps" answer, so it must
            // exit non-zero with no stdout JSON — a `"error"` key inside a
            // SUCCESS-exiting body is exactly the shape this task closes.
            None => {
                eprintln!(
                    "aldump: error: fail-closed/empty layout at {} — primary workspace not buildable",
                    workspace.display()
                );
                return ExitCode::FAILURE;
            }
            Some(cross) => {
                let ws = &cross.resolved.workspace;

                // Build dep_routine_ids: routines whose app_guid ∈ fetched_app_guids
                // (lowercased). This is the same oracle the L4/L5 cross-app paths use
                // (capability_cone.rs:2426-2431, detector_context.rs, etc.). A routine
                // NOT in this set is PRIMARY (workspace-owned).
                let fetched_lc: HashSet<String> = cross
                    .fetched_app_guids
                    .iter()
                    .map(|g| g.to_lowercase())
                    .collect();
                let dep_routine_ids: HashSet<String> = ws
                    .routines
                    .iter()
                    .filter(|r| fetched_lc.contains(&r.app_guid.to_lowercase()))
                    .map(|r| r.id.clone())
                    .collect();

                // Resolve calls over the MERGED model with the real dep ledger.
                let symbols = SymbolTable::build(&ws.objects, &ws.tables, &ws.routines);
                let declared: Vec<DeclaredDependency> = cross
                    .declared_dep_app_guids
                    .iter()
                    .map(|g| DeclaredDependency {
                        app_guid: g.clone(),
                    })
                    .collect();
                let resolved = resolve_calls(ws, &symbols, &declared, &cross.fetched_app_guids);

                // Scope to PRIMARY edges only — exclude any edge whose `from` routine
                // is a dep routine (dep-internal calls don't count toward the metric).
                let primary_edges: Vec<_> = resolved
                    .edges
                    .iter()
                    .filter(|e| !dep_routine_ids.contains(&e.from))
                    .collect();

                let histogram =
                    Histogram::of_edges(&primary_edges.into_iter().cloned().collect::<Vec<_>>());

                let mut value = serde_json::to_value(histogram).unwrap_or(serde_json::json!({}));
                if let Some(obj) = value.as_object_mut() {
                    obj.insert(
                        "legacyL3UnknownRate".to_string(),
                        serde_json::json!(histogram.real_unknown_rate()),
                    );
                    obj.insert(
                        "advisory".to_string(),
                        serde_json::json!(
                            "legacy L3 engine; authoritative metric is --program-call-graph-stats"
                        ),
                    );
                    obj.insert(
                        "depAppsLoaded".to_string(),
                        serde_json::json!(cross.fetched_app_guids.len()),
                    );
                }
                return match serde_json::to_string_pretty(&value) {
                    Ok(json) => {
                        println!("{json}");
                        ExitCode::SUCCESS
                    }
                    Err(e) => {
                        eprintln!(
                            "aldump: error: failed to serialize cross-app call-graph stats: {e}"
                        );
                        ExitCode::FAILURE
                    }
                };
            }
        }
    }

    if l3_unknown_breakdown {
        // Attribute every TRUE-`unknown` edge to its resolver cause (UnknownReason)
        // — the work-list for the typed-resolution phases. Read-only.
        //
        // Task T0.1: a fail-closed/empty layout is a genuine tool failure — exits
        // non-zero with no stdout output.
        use al_sem::engine::l3::call_resolver::{DeclaredDependency, resolve_calls};
        use al_sem::engine::l3::resolution_class::{Histogram, unknown_breakdown};
        use al_sem::engine::l3::symbol_table::SymbolTable;

        let Some(resolved) = assemble_and_resolve_workspace_default(&workspace) else {
            eprintln!(
                "aldump: error: fail-closed/empty layout at {} — cannot compute unknown breakdown",
                workspace.display()
            );
            return ExitCode::FAILURE;
        };
        let ws = &resolved.workspace;
        let symbols = SymbolTable::build(&ws.objects, &ws.tables, &ws.routines);
        let no_deps: Vec<DeclaredDependency> = Vec::new();
        let no_fetched: Vec<String> = Vec::new();
        let r = resolve_calls(ws, &symbols, &no_deps, &no_fetched);
        let (breakdown, framework_detail, shape_detail, bare_detail) = unknown_breakdown(&r.edges);
        let histogram = Histogram::of_edges(&r.edges);
        let value = serde_json::json!({
            "totalEdges": histogram.total,
            "unknownTotal": histogram.unknown,
            "legacyL3UnknownRate": histogram.real_unknown_rate(),
            "advisory": "legacy L3 engine; authoritative metric is --program-call-graph-stats",
            "byReason": breakdown,
            "bareCallDetail": bare_detail,
            "frameworkMethodDetail": framework_detail,
            "receiverShapeDetail": shape_detail,
        });
        return match serde_json::to_string_pretty(&value) {
            Ok(json) => {
                println!("{json}");
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("aldump: error: failed to serialize unknown breakdown: {e}");
                ExitCode::FAILURE
            }
        };
    }

    if l3_unknown_breakdown_cross_app {
        // DEPS-LOADED, PRIMARY-SCOPED unknown breakdown — the legacy L3 work-list
        // (advisory; Task T0.4 — the authoritative metric is
        // `aldump --program-call-graph-stats`). Same merged-model + primary-edge
        // scoping as `--l3-call-graph-stats-cross-app`
        // (deps present for resolution; metric measured over WORKSPACE call sites
        // only), but attributes every residual TRUE-`unknown` edge to its
        // `UnknownReason` so the real (whole-program) holes can be targeted directly
        // rather than inferred from the source-only breakdown. Fail-closed → message
        // + empty breakdown JSON; never throws.
        use al_sem::engine::l3::call_resolver::{DeclaredDependency, resolve_calls};
        use al_sem::engine::l3::resolution_class::{Histogram, unknown_breakdown};
        use al_sem::engine::l3::symbol_table::SymbolTable;
        use std::collections::HashSet;

        match build_cross_app_l3_from_workspace(&workspace, R2_5B_MODEL_INSTANCE_ID) {
            // FATAL (Task T0.1, R2): see the identical `None` note on
            // `--l3-call-graph-stats-cross-app` above — `None` is exclusively a
            // primary-workspace-unbuildable failure, never a legitimate "no deps"
            // result, so it must exit non-zero with no stdout JSON.
            None => {
                eprintln!(
                    "aldump: error: fail-closed/empty layout at {} — primary workspace not buildable",
                    workspace.display()
                );
                return ExitCode::FAILURE;
            }
            Some(cross) => {
                let ws = &cross.resolved.workspace;
                let fetched_lc: HashSet<String> = cross
                    .fetched_app_guids
                    .iter()
                    .map(|g| g.to_lowercase())
                    .collect();
                let dep_routine_ids: HashSet<String> = ws
                    .routines
                    .iter()
                    .filter(|r| fetched_lc.contains(&r.app_guid.to_lowercase()))
                    .map(|r| r.id.clone())
                    .collect();

                let symbols = SymbolTable::build(&ws.objects, &ws.tables, &ws.routines);
                let declared: Vec<DeclaredDependency> = cross
                    .declared_dep_app_guids
                    .iter()
                    .map(|g| DeclaredDependency {
                        app_guid: g.clone(),
                    })
                    .collect();
                let resolved = resolve_calls(ws, &symbols, &declared, &cross.fetched_app_guids);

                let primary_edges: Vec<_> = resolved
                    .edges
                    .iter()
                    .filter(|e| !dep_routine_ids.contains(&e.from))
                    .cloned()
                    .collect();

                let histogram = Histogram::of_edges(&primary_edges);
                let (breakdown, framework_detail, shape_detail, bare_detail) =
                    unknown_breakdown(&primary_edges);

                if std::env::var("ALDUMP_DEBUG_UNKNOWN").is_ok() {
                    use al_sem::engine::l3::resolution_class::{ResolutionClass, classify};
                    let rt_by_id: std::collections::HashMap<&str, &_> =
                        ws.routines.iter().map(|r| (r.id.as_str(), r)).collect();
                    let filter = std::env::var("ALDUMP_DEBUG_UNKNOWN").unwrap_or_default();
                    for e in &primary_edges {
                        if classify(e.resolution, e.dispatch_kind) != ResolutionClass::Unknown {
                            continue;
                        }
                        let shape = e.receiver_shape.as_deref().unwrap_or("-");
                        if !filter.is_empty() && filter != "1" && !shape.contains(&filter) {
                            continue;
                        }
                        let (oname, onum, rname) = rt_by_id
                            .get(e.from.as_str())
                            .map(|r| (r.object_type.as_str(), r.object_number, r.name.as_str()))
                            .unwrap_or(("?", 0, "?"));
                        eprintln!(
                            "UNK {oname} {onum} :: {rname} :: shape={shape} recvType={:?} method={:?} cs={}",
                            e.receiver_type, e.unknown_method_name, e.callsite_id
                        );
                    }
                }

                let value = serde_json::json!({
                    "totalEdges": histogram.total,
                    "unknownTotal": histogram.unknown,
                    "legacyL3UnknownRate": histogram.real_unknown_rate(),
                    "advisory": "legacy L3 engine; authoritative metric is --program-call-graph-stats",
                    "depAppsLoaded": cross.fetched_app_guids.len(),
                    "byReason": breakdown,
                    "bareCallDetail": bare_detail,
                    "frameworkMethodDetail": framework_detail,
                    "receiverShapeDetail": shape_detail,
                });
                return match serde_json::to_string_pretty(&value) {
                    Ok(json) => {
                        println!("{json}");
                        ExitCode::SUCCESS
                    }
                    Err(e) => {
                        eprintln!("aldump: error: failed to serialize cross-app breakdown: {e}");
                        ExitCode::FAILURE
                    }
                };
            }
        }
    }

    if l3_call_graph {
        // L3 call-graph projection (R2b): the resolved call graph (grouped
        // callsiteId → CallEdge[], multi-edge interface dispatch preserved,
        // group-level dispatchMeta) + the upgraded argumentBindings, all in stable
        // id form.
        //
        // Task T0.1: a fail-closed/empty layout is a genuine tool failure — exits
        // non-zero with no stdout output.
        let Some(resolved) = assemble_and_resolve_workspace_default(&workspace) else {
            eprintln!(
                "aldump: error: fail-closed/empty layout at {} — cannot compute L3 call-graph projection",
                workspace.display()
            );
            return ExitCode::FAILURE;
        };
        let projection = resolved.project_call_graph();
        return match serde_json::to_string_pretty(&projection) {
            Ok(json) => {
                println!("{json}");
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("aldump: error: failed to serialize L3 call-graph projection: {e}");
                ExitCode::FAILURE
            }
        };
    }

    if l3_record_types {
        // L3 record-type projection (R2a): resolved record-var/op StableTableIds
        // (omitted when unresolved) + per-Table merged fields.
        //
        // Task T0.1: a fail-closed/empty layout is a genuine tool failure — exits
        // non-zero with no stdout output.
        let Some(resolved) = assemble_and_resolve_workspace_default(&workspace) else {
            eprintln!(
                "aldump: error: fail-closed/empty layout at {} — cannot compute L3 record-type projection",
                workspace.display()
            );
            return ExitCode::FAILURE;
        };
        let projection = resolved.project();
        return match serde_json::to_string_pretty(&projection) {
            Ok(json) => {
                println!("{json}");
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("aldump: error: failed to serialize L3 projection: {e}");
                ExitCode::FAILURE
            }
        };
    }

    if b3_deps && (!b3 || b3_triage.is_none()) {
        eprintln!("aldump: error: --b3-deps needs --b3 and --b3-triage <file.md>");
        return usage();
    }
    if b3 {
        // B3 Phase A: the join census between L3 call sites and the program
        // engine's call-site edges, plus the adapter's counts
        // (`engine::l3::program_calls`). With `--b3-triage <file.md>`, also
        // the detector diff (`engine::l3::b3_diff`, code map C10).
        let census = match &b3_triage {
            None => al_sem::engine::l3::program_calls::adapter_census_for_workspace(&workspace),
            Some(out) => {
                use al_sem::engine::l3::b3_diff::{
                    carry_verdicts, detector_diff_for_workspace, triage_markdown,
                };
                detector_diff_for_workspace(&workspace, b3_deps).and_then(|d| {
                    let title = out
                        .file_stem()
                        .map(|s| s.to_string_lossy().to_string())
                        .unwrap_or_default();
                    let census = d.census.clone();
                    // Keep the verdicts already written into an existing table.
                    let md = triage_markdown(&title, &[(title.clone(), d)], &[]);
                    let md = match std::fs::read_to_string(out) {
                        Ok(old) => carry_verdicts(&md, &old),
                        Err(_) => md,
                    };
                    std::fs::write(out, md).map_err(|e| format!("{}: {e}", out.display()))?;
                    Ok(census)
                })
            }
        };
        return match census
            .and_then(|c| serde_json::to_string_pretty(&c).map_err(|e| e.to_string()))
        {
            Ok(json) => {
                println!("{json}");
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("aldump: error: --b3: {e}");
                ExitCode::FAILURE
            }
        };
    }

    if program_call_graph_stats {
        // 1B.3a Task 3: self-reported north-star metric.
        //
        // Runs `resolve_full_program` (clean-room, no L3 oracle) over the
        // workspace and prints:
        //   - Taxonomy'd Histogram for the whole program + primary-scoped variant
        //   - Coverage result (obligation SET equality)
        //   - ABI ingestion integrity summary
        //
        // Both `--l3-call-graph-stats` and `--l3-call-graph-stats-cross-app`
        // are KEPT unchanged; this flag is now fully independent of L3.
        use al_sem::program::resolve::edge::{
            unknown_reason_breakdown, unknown_receiver_tier_breakdown,
        };
        use al_sem::program::resolve::full::{
            coverage_holds, is_primary_scope, resolve_full_program,
        };

        let Some(r) = resolve_full_program(&workspace) else {
            eprintln!("aldump: error: resolve_full_program failed (snapshot build error)");
            return ExitCode::FAILURE;
        };

        let h = &r.histogram;
        let ph = &r.primary_histogram;
        let cov = &r.coverage;
        let abi = &r.abi_integrity;

        // Task 3: stratified `Unknown`-reason breakdown (charter §8). Purely
        // diagnostic — never changes `h`/`ph`/`cov` above. Rendered via
        // `UnknownReason::as_str()` (stable camelCase keys), never `Debug`.
        let whole_by_reason: std::collections::BTreeMap<String, usize> =
            unknown_reason_breakdown(r.edges.iter().map(|ce| &ce.edge))
                .into_iter()
                .map(|(reason, count)| (reason.as_str().to_string(), count))
                .collect();
        let primary_by_reason: std::collections::BTreeMap<String, usize> =
            unknown_reason_breakdown(
                r.edges
                    .iter()
                    .filter(|ce| is_primary_scope(ce, r.primary_app_ref))
                    .map(|ce| &ce.edge),
            )
            .into_iter()
            .map(|(reason, count)| (reason.as_str().to_string(), count))
            .collect();

        // Reason-split Task 2: ADDITIVE `receiver_tier` diagnostic, keyed
        // `"<reason>:<tier|none>"` — sibling of `unknownByReason` above, never
        // a replacement. Only `memberNotFound` routes ever carry `Some(tier)`
        // today (see `Route::receiver_tier`'s doc); every other reason
        // reports under its own `:none` key.
        fn tier_reason_key(
            reason: al_sem::program::resolve::edge::UnknownReason,
            tier: Option<al_sem::snapshot::TrustTier>,
        ) -> String {
            match tier {
                Some(t) => format!("{}:{}", reason.as_str(), t.as_str()),
                None => format!("{}:none", reason.as_str()),
            }
        }
        let whole_tier_by_reason: std::collections::BTreeMap<String, usize> =
            unknown_receiver_tier_breakdown(r.edges.iter().map(|ce| &ce.edge))
                .into_iter()
                .map(|((reason, tier), count)| (tier_reason_key(reason, tier), count))
                .collect();
        let primary_tier_by_reason: std::collections::BTreeMap<String, usize> =
            unknown_receiver_tier_breakdown(
                r.edges
                    .iter()
                    .filter(|ce| is_primary_scope(ce, r.primary_app_ref))
                    .map(|ce| &ce.edge),
            )
            .into_iter()
            .map(|((reason, tier), count)| (tier_reason_key(reason, tier), count))
            .collect();

        let value = serde_json::json!({
            // ── Whole-program histogram ──────────────────────────────────────
            "wholeProgram": {
                "total": h.total,
                "resolvedSource": h.resolved_source,
                "resolvedCatalog": h.resolved_catalog,
                "resolvedAbiExternal": h.resolved_abi_external,
                "conditionalResolved": h.conditional_resolved,
                "honestDynamic": h.honest_dynamic,
                "honestEmpty": h.honest_empty,
                "unknown": h.unknown,
                // Task 3 (sigfp-and-ambiguous-reclassification plan): closed
                // same-object overload-ambiguity candidate sets, honestly
                // excluded from `unknown`/`realUnknownRate` — see
                // `ObligationOutcome::AmbiguousResolved`'s doc. Wired by a
                // real producer (`resolve_in_object`) as of Task 4.
                "ambiguousResolved": h.ambiguous_resolved,
                "realUnknownRate": h.real_unknown_rate(),
                // Task 4 both-ways reporting (round-1 addendum, BINDING): the
                // LEGACY/advisory rate under the PRE-Task-4 metric definition
                // (counts `ambiguousResolved` as unknown too) — additive,
                // side-by-side with `realUnknownRate` so the metric-definition
                // change is never stat-juked. See `Histogram::legacy_
                // unknown_rate_including_ambiguous`'s doc.
                "realUnknownRateLegacyIncludingAmbiguous":
                    h.legacy_unknown_rate_including_ambiguous(),
                "unknownByReason": whole_by_reason,
                "unknownReceiverTier": whole_tier_by_reason,
            },
            // ── Primary-scoped histogram (workspace edges only) ──────────────
            // Mirrors --l3-call-graph-stats-cross-app scoping.
            "primaryScoped": {
                "total": ph.total,
                "resolvedSource": ph.resolved_source,
                "resolvedCatalog": ph.resolved_catalog,
                "resolvedAbiExternal": ph.resolved_abi_external,
                "conditionalResolved": ph.conditional_resolved,
                "honestDynamic": ph.honest_dynamic,
                "honestEmpty": ph.honest_empty,
                "unknown": ph.unknown,
                "ambiguousResolved": ph.ambiguous_resolved,
                "realUnknownRate": ph.real_unknown_rate(),
                "realUnknownRateLegacyIncludingAmbiguous":
                    ph.legacy_unknown_rate_including_ambiguous(),
                "unknownByReason": primary_by_reason,
                "unknownReceiverTier": primary_tier_by_reason,
            },
            // ── Coverage contract ────────────────────────────────────────────
            "coverage": {
                "parsedObligations": cov.parsed_obligations,
                "classifiedEdges": cov.classified_edges,
                "holds": coverage_holds(cov),
                "missingCount": cov.missing.len(),
                "extraCount": cov.extra.len(),
            },
            // ── ABI ingestion integrity ──────────────────────────────────────
            "abiIntegrity": {
                "abiRoutesTotal": abi.abi_routes_total,
                "abiMapped": abi.abi_mapped,
                "abiUnmapped": abi.abi_unmapped,
            },
            // ── Collision-guard observability (Task 1) ───────────────────────
            // Publisher EventFlow edges skipped by the dual-publisher
            // source-overload-alias guard (`resolver::emit_event_flow_edges`).
            // Expected 0 outside the CDO-measured known dual-publisher pairs.
            "eventFlowDualPublisherAliasSkips": r.event_flow_dual_publisher_alias_skips,
            // Task 3 (preprocessor foundations plan): additive, non-gating
            // ParseStatus::Recovered diagnostic — see `recovered_files`'s doc
            // on `ProgramReport`. Expected `count: 0` on a well-formed
            // workspace; a nonzero count means that many files' IR may be
            // missing content tree-sitter could not parse.
            "recoveredFiles": {
                "count": r.recovered_files.len(),
                "paths": r.recovered_files,
            },
        });

        return match serde_json::to_string_pretty(&value) {
            Ok(json) => {
                println!("{json}");
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("aldump: error: failed to serialize program-call-graph-stats: {e}");
                ExitCode::FAILURE
            }
        };
    }

    if graphify_export {
        // graphify ADAPTER: project the whole-program resolved call graph into a
        // graphify node-link extraction document (`{ nodes, edges, hyperedges }`),
        // consumed by graphify's `build_from_json` (see `graphify_export.rs` +
        // `U:\Git\graphify\adapter.md`). Fail-closed → snapshot build error.
        let Some(doc) = al_sem::program::graphify_export::export_workspace(&workspace) else {
            eprintln!("aldump: error: graphify export failed (snapshot build error)");
            return ExitCode::FAILURE;
        };
        return match serde_json::to_string_pretty(&doc) {
            Ok(json) => {
                println!("{json}");
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("aldump: error: failed to serialize graphify export: {e}");
                ExitCode::FAILURE
            }
        };
    }

    if graphify_export_fragments {
        // graphify INCREMENTAL: the graphify document partitioned into per-object
        // fragments + a content-hash manifest (`{ manifest, fragments, shared }`).
        // Diff the manifest across runs → only re-process the objects whose output
        // changed (see `program::graphify_export::FragmentSet`). Fail-closed.
        let Some(fs) = al_sem::program::graphify_export::export_workspace_fragments(&workspace)
        else {
            eprintln!("aldump: error: graphify fragment export failed (snapshot build error)");
            return ExitCode::FAILURE;
        };
        return match serde_json::to_string_pretty(&fs) {
            Ok(json) => {
                println!("{json}");
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("aldump: error: failed to serialize graphify fragments: {e}");
                ExitCode::FAILURE
            }
        };
    }

    if integration_points {
        // INTEGRATION-POINTS REPORT: the resolved event wiring as a "who-reacts-to-
        // what" slice scoped to the workspace's integration surface (see
        // `program::integration_report`). Fail-closed → snapshot build error.
        let Some(report) = al_sem::program::integration_report::report_workspace(&workspace) else {
            eprintln!("aldump: error: integration-points report failed (snapshot build error)");
            return ExitCode::FAILURE;
        };
        return match serde_json::to_string_pretty(&report) {
            Ok(json) => {
                println!("{json}");
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("aldump: error: failed to serialize integration-points report: {e}");
                ExitCode::FAILURE
            }
        };
    }

    if l2 {
        let projection = match project_workspace(&workspace) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("aldump: error: {e:#}");
                return ExitCode::FAILURE;
            }
        };
        match serde_json::to_string_pretty(&projection) {
            Ok(json) => {
                println!("{json}");
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("aldump: error: failed to serialize L2 projection: {e}");
                ExitCode::FAILURE
            }
        }
    } else {
        let snapshot = match snapshot_workspace(&workspace) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("aldump: error: {e:#}");
                return ExitCode::FAILURE;
            }
        };
        // Pretty-print with 2-space indent to mirror the goldens (the differ
        // parses structurally, so pretty-printing is a convenience).
        match serde_json::to_string_pretty(&snapshot) {
            Ok(json) => {
                println!("{json}");
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("aldump: error: failed to serialize snapshot: {e}");
                ExitCode::FAILURE
            }
        }
    }
}

//! R3a-5 EXIT GATE (= R3a COMPLETE) — the cross-app FULL RoutineSummary
//! DIFFERENTIAL + anti-degenerate CROSS-APP matrix.
//!
//! Runs the Rust R3a-5 cross-app L4 path (`project_r3a5_cross_app`) over the
//! committed workspace fixture (`tests/r3a5-fixtures/ws` — a source-bearing chain
//! dep `cccccccc-…` with DoIt→DoWrite→Insert + a symbol-only dep `55555555-…`)
//! and asserts it BYTE-MATCHES the al-sem golden
//! (`tests/r3a5-goldens/cross-app-full-summary.r3a5.golden.json`).
//!
//! ## What R3a-5 adds vs R3a-2/3 (source-only)
//!
//! The merged (workspace + dep-hook-injected) graph feeds the SAME L4 path, so a
//! PRIMARY routine calling a source-bearing dep routine INHERITS the dep's
//! `capabilityFactsDirect` (the cone fires cross-app via the injected
//! `intraAppCallEdges` → `typedEdges`) AND folds the dep's `via:"direct"` dbEffect
//! into its own `dbEffects` (`via:"inherited"`). The full RoutineSummary (R3a-2
//! core + R3a-3 cone/coverage) is compared per routine, STABLE-id form.
//!
//! ## Capture point (R3a-5)
//!
//! post-computeSummaries WITH dep hooks (the FULL `analyzeWorkspace` order:
//! merged index → buildCombinedGraph → injectIntraAppCallEdges → computeSummaries
//! → the cone).
//!
//! ## Strict byte-match comparison
//!
//! No allowlist tolerance: any structural divergence fails the test directly, and
//! the pretty JSON must be byte-identical to the golden file.

use std::path::PathBuf;

use crate::regen;

use al_sem::engine::l4::capability_cone::{R3a5FullSummaryProjection, project_r3a5_cross_app};
use serde_json::Value;

const R3A5_TEST_NAME: &str = "differential_r3a5_cross_app_summary_match_goldens";
const FIXTURE: &str = "cross-app-full-summary";

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn goldens_dir() -> PathBuf {
    repo_root().join("tests").join("r3a5-goldens")
}

fn ws_fixture_dir() -> PathBuf {
    repo_root().join("tests").join("r3a5-fixtures").join("ws")
}

#[derive(Debug, Clone)]
struct Divergence {
    fixture: String,
    path: String,
    golden_value: String,
    rust_value: String,
}

/// Recursively diff two projection values POSITIONALLY (both sides already sorted).
fn diff_value(fixture: &str, path: &str, golden: &Value, rust: &Value, out: &mut Vec<Divergence>) {
    match (golden, rust) {
        (Value::Object(g), Value::Object(r)) => {
            for (k, gv) in g {
                let child = format!("{path}.{k}");
                match r.get(k) {
                    Some(rv) => diff_value(fixture, &child, gv, rv, out),
                    None => out.push(Divergence {
                        fixture: fixture.to_string(),
                        path: format!("{child}:MISSING_IN_RUST"),
                        golden_value: compact(gv),
                        rust_value: "<absent>".to_string(),
                    }),
                }
            }
            for (k, rv) in r {
                if !g.contains_key(k) {
                    out.push(Divergence {
                        fixture: fixture.to_string(),
                        path: format!("{path}.{k}:EXTRA_IN_RUST"),
                        golden_value: "<absent>".to_string(),
                        rust_value: compact(rv),
                    });
                }
            }
        }
        (Value::Array(g), Value::Array(r)) => {
            if g.len() != r.len() {
                out.push(Divergence {
                    fixture: fixture.to_string(),
                    path: format!("{path}:LENGTH"),
                    golden_value: g.len().to_string(),
                    rust_value: r.len().to_string(),
                });
            }
            let n = g.len().min(r.len());
            for i in 0..n {
                diff_value(fixture, &format!("{path}[{i}]"), &g[i], &r[i], out);
            }
            for (i, gv) in g.iter().enumerate().skip(n) {
                out.push(Divergence {
                    fixture: fixture.to_string(),
                    path: format!("{path}[{i}]:MISSING_IN_RUST"),
                    golden_value: compact(gv),
                    rust_value: "<absent>".to_string(),
                });
            }
            for (i, rv) in r.iter().enumerate().skip(n) {
                out.push(Divergence {
                    fixture: fixture.to_string(),
                    path: format!("{path}[{i}]:EXTRA_IN_RUST"),
                    golden_value: "<absent>".to_string(),
                    rust_value: compact(rv),
                });
            }
        }
        _ => {
            if golden != rust {
                out.push(Divergence {
                    fixture: fixture.to_string(),
                    path: path.to_string(),
                    golden_value: compact(golden),
                    rust_value: compact(rust),
                });
            }
        }
    }
}

fn compact(v: &Value) -> String {
    serde_json::to_string(v).unwrap_or_else(|_| format!("{v:?}"))
}

/// Build the Rust R3a-5 projection over the workspace fixture.
fn rust_projection() -> R3a5FullSummaryProjection {
    project_r3a5_cross_app(&ws_fixture_dir(), FIXTURE)
}

#[test]
fn differential_r3a5_cross_app_summary_match_goldens() {
    let golden_path = goldens_dir().join(format!("{FIXTURE}.r3a5.golden.json"));
    let golden_text = std::fs::read_to_string(&golden_path)
        .unwrap_or_else(|e| panic!("read R3a-5 golden {}: {e}", golden_path.display()));
    let golden_json: Value = serde_json::from_str(&golden_text)
        .unwrap_or_else(|e| panic!("R3a-5 golden {} not valid JSON: {e}", golden_path.display()));
    // Shape guard — the golden must round-trip through the R3a5 serde type.
    let _: R3a5FullSummaryProjection =
        serde_json::from_value(golden_json.clone()).unwrap_or_else(|e| {
            panic!(
                "R3a-5 golden {} does not parse as R3a5FullSummaryProjection: {e}",
                golden_path.display()
            )
        });

    let projection = rust_projection();

    // REGEN path (temp-state epoch rebaseline, Task 16). When `REGEN_TEMP_GOLDENS`
    // is set, write the ENGINE projection to the golden file (matching the on-disk
    // pretty form) instead of comparing — the goldens are Rust-owned baselines (TS
    // oracle retired).
    if regen::regen_mode() {
        let mut pretty = serde_json::to_string_pretty(&projection)
            .unwrap_or_else(|e| panic!("regen serialize R3a-5: {e}"));
        pretty.push('\n');
        regen::write_golden(&golden_path, pretty)
            .unwrap_or_else(|e| panic!("regen write {}: {e}", golden_path.display()));
        eprintln!("REGEN r3a5 golden: {}", golden_path.display());
        return;
    }

    let rust_json = serde_json::to_value(&projection)
        .unwrap_or_else(|e| panic!("serialize Rust R3a-5 projection: {e}"));

    // --- structural positional diff ---
    let mut all_divergences: Vec<Divergence> = Vec::new();
    diff_value(FIXTURE, "", &golden_json, &rust_json, &mut all_divergences);
    all_divergences
        .sort_by(|a, b| (a.fixture.as_str(), &a.path).cmp(&(b.fixture.as_str(), &b.path)));

    let mut failure = String::new();
    if !all_divergences.is_empty() {
        failure.push_str(&format!(
            "\n{} R3a-5 divergence(s) found ({R3A5_TEST_NAME}):\n",
            all_divergences.len()
        ));
        for d in &all_divergences {
            failure.push_str(&format!(
                "  [{}] {}\n      golden = {}\n      rust   = {}\n",
                d.fixture, d.path, d.golden_value, d.rust_value
            ));
        }
    }
    assert!(
        failure.is_empty(),
        "R3a-5 cross-app summary differential FAILED:{failure}"
    );

    // --- BYTE-MATCH guard: the pretty JSON must be byte-identical to the golden
    // file (the strongest oracle). -----------------------------------------
    let rust_pretty = serde_json::to_string_pretty(&projection)
        .unwrap_or_else(|e| panic!("pretty-serialize Rust R3a-5 projection: {e}"));
    assert_eq!(
        rust_pretty.trim_end(),
        golden_text.trim_end(),
        "R3a-5 pretty JSON is NOT byte-identical to the golden"
    );

    eprintln!(
        "R3a-5 differential: 1 fixture, {} summaries, byte-match.",
        projection.summaries.len()
    );
}

/// ANTI-DEGENERATE CROSS-APP matrix (fail-on-zero) — the cross-app corpus must
/// exercise the dep-fact-propagation surface, and the Rust counts must EQUAL the
/// al-sem manifest's `matrix` block.
#[test]
fn r3a5_anti_degenerate_matrix_matches_manifest() {
    let projection = rust_projection();

    // Read the al-sem manifest matrix (ground truth captured at dump time).
    let manifest_path = goldens_dir().join("manifest.json");
    let manifest_text = std::fs::read_to_string(&manifest_path)
        .unwrap_or_else(|e| panic!("read R3a-5 manifest {}: {e}", manifest_path.display()));
    let manifest: Value = serde_json::from_str(&manifest_text)
        .unwrap_or_else(|e| panic!("R3a-5 manifest not valid JSON: {e}"));
    let mat = manifest
        .get("matrix")
        .unwrap_or_else(|| panic!("R3a-5 manifest carries no `matrix` block"));
    let u = |k: &str| mat.get(k).and_then(|v| v.as_u64()).unwrap_or(0) as usize;

    // --- fail-on-zero (the source-only "no dep facts propagated" green is a
    //     FAILURE here — the whole point of R3a-5 is the cross-app cone firing). -
    let mut degenerate: Vec<String> = Vec::new();
    if projection.primary_routines_with_inherited_dep_facts == 0 {
        degenerate.push(
            "primaryRoutinesWithInheritedDepFacts=0 (the cross-app cone must fire — a primary \
             must inherit a DEP-propagated capability fact)"
                .into(),
        );
    }
    if projection.primary_routines_with_dep_db_effects == 0 {
        degenerate.push(
            "primaryRoutinesWithDepDbEffects=0 (a primary must fold a dep-originated dbEffect)"
                .into(),
        );
    }
    if projection.coverages_with_opaque_apps_reason == 0 {
        degenerate.push(
            "coveragesWithOpaqueAppsReason=0 (a coverage must carry a cross-app opaque reason)"
                .into(),
        );
    }
    if projection.total_cross_app_inherited_facts == 0 {
        degenerate.push("totalCrossAppInheritedFacts=0 (no cross-app inherited fact)".into());
    }
    assert!(
        degenerate.is_empty(),
        "DEGENERATE R3a-5 cross-app matrix — the dep-fact propagation surface is hollow:\n  {}",
        degenerate.join("\n  ")
    );

    // --- pinned counts (the al-sem plan's exact targets) ----------------------
    assert_eq!(
        projection.primary_routines_with_inherited_dep_facts, 1,
        "primaryRoutinesWithInheritedDepFacts=1"
    );
    assert_eq!(
        projection.primary_routines_with_dep_db_effects, 1,
        "primaryRoutinesWithDepDbEffects=1"
    );
    assert_eq!(
        projection.coverages_with_opaque_apps_reason, 2,
        "coveragesWithOpaqueAppsReason=2"
    );
    // 2: the dependency's write and, since its write is a statement (its
    // Boolean discarded), that write's `error-implicit` (G20).
    assert_eq!(
        projection.total_cross_app_inherited_facts, 2,
        "totalCrossAppInheritedFacts=2"
    );

    // --- cross-check vs the al-sem manifest matrix (ground truth) -------------
    assert_eq!(
        projection.primary_routines_with_inherited_dep_facts,
        u("primaryRoutinesWithInheritedDepFacts"),
        "primaryRoutinesWithInheritedDepFacts vs manifest"
    );
    assert_eq!(
        projection.primary_routines_with_dep_db_effects,
        u("primaryRoutinesWithDepDbEffects"),
        "primaryRoutinesWithDepDbEffects vs manifest"
    );
    assert_eq!(
        projection.coverages_with_opaque_apps_reason,
        u("coveragesWithOpaqueAppsReason"),
        "coveragesWithOpaqueAppsReason vs manifest"
    );
    assert_eq!(
        projection.total_cross_app_inherited_facts,
        u("totalCrossAppInheritedFacts"),
        "totalCrossAppInheritedFacts vs manifest"
    );

    eprintln!(
        "R3a-5 matrix: primaryWithInheritedDepFacts={} primaryWithDepDbEffects={} \
         coveragesWithOpaqueAppsReason={} totalCrossAppInheritedFacts={} (== al-sem manifest)",
        projection.primary_routines_with_inherited_dep_facts,
        projection.primary_routines_with_dep_db_effects,
        projection.coverages_with_opaque_apps_reason,
        projection.total_cross_app_inherited_facts,
    );
}

/// Determinism: the cross-app projection is byte-stable across repeated runs, and
/// the stable ids are modelInstanceId-INDEPENDENT.
#[test]
fn r3a5_projection_is_byte_stable() {
    let a = serde_json::to_string(&rust_projection()).unwrap();
    let b = serde_json::to_string(&rust_projection()).unwrap();
    assert_eq!(a, b, "R3a-5 projection is byte-stable across runs");

    // NOTE (temp-state epoch, Task 16): the `!a.contains("\"r0/")` sub-assertion
    // was REMOVED here. It was a too-strict heuristic ("no internal
    // modelInstanceId-prefixed id leaks") that the designed cross-app temp-state
    // promotion now legitimately invalidates: a promoted dep record var binds a
    // `recordVariableId: "r0/<hash>/rv/<name>"` — `recordVariableId` is an
    // INTERNAL id that canonically carries the `r0/` model-instance prefix (the
    // same `r0/` form is present 361× in the r3a3 goldens). The determinism
    // (a == b) part above and the stable `<guid>:Type:Num#hash` routine-id checks
    // below remain the real invariants.
    // The dep + primary stable ids are present in the expected `<guid>:Type:Num#hash` form.
    assert!(
        a.contains("cccccccc-0001-0000-0000-000000000001:Codeunit:50300#"),
        "stable dep routine id form present"
    );
    assert!(
        a.contains("33333333-0005-0000-0000-000000000003:Codeunit:71000#"),
        "stable primary routine id form present"
    );
}

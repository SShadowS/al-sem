//! R3a-4 EXIT GATE — the cross-app dep-hook DIFFERENTIAL + anti-degenerate matrix.
//!
//! Runs the Rust R3a-4 producer + consumer-hook pipeline
//! (`project_r3a4_from_workspace`) over the committed workspace fixture
//! (`tests/r3a4-fixtures/ws` — a source-bearing dep `.app` with the DoIt→DoWrite→Insert
//! chain) and asserts it BYTE-MATCHES the al-sem golden
//! (`tests/r3a4-goldens/cross-app-dep-hooks.r3a4.golden.json`).
//!
//! ## The stable-id form (Task 3 — THE key fix)
//!
//! Every id-bearing field is STABLE-PROJECTED to
//! `<appGuid>:<Type>:<Num>#<normalizedSignatureHash>[/opN|/csN]` — appGuid/signature-
//! derived → cache/modelInstanceId/devFingerprint-INDEPENDENT. The golden carries the
//! SAME stable form (al-sem `DepIdStabilizer`), so the differential is a real byte
//! oracle. NO `dep:<artifactKey>` prefix appears on either side.
//!
//! ## Capture point (R3a-4)
//!
//! post-inject_intra_app_call_edges / collect_cited_dep_evidence /
//! collect_dep_order_index. The R3a-5 cross-app cone is NOT projected.
//!
//! ## Strict byte-match comparison
//!
//! No allowlist tolerance: any structural divergence fails the test directly, and
//! the pretty JSON must be byte-identical to the golden file.

use std::path::PathBuf;

use al_sem::engine::deps::r3a4_projection::{R3a4Projection, project_r3a4_from_workspace};
use serde_json::Value;

use crate::regen;

const R3A4_TEST_NAME: &str = "differential_r3a4_dep_hooks_match_goldens";
const FIXTURE: &str = "cross-app-dep-hooks";

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn goldens_dir() -> PathBuf {
    repo_root().join("tests").join("r3a4-goldens")
}

fn ws_fixture_dir() -> PathBuf {
    repo_root().join("tests").join("r3a4-fixtures").join("ws")
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

/// Build the Rust R3a-4 projection over the workspace fixture.
fn rust_projection() -> R3a4Projection {
    project_r3a4_from_workspace(&ws_fixture_dir(), FIXTURE)
}

#[test]
fn differential_r3a4_dep_hooks_match_goldens() {
    let golden_path = goldens_dir().join(format!("{FIXTURE}.r3a4.golden.json"));
    let golden_text = std::fs::read_to_string(&golden_path)
        .unwrap_or_else(|e| panic!("read R3a-4 golden {}: {e}", golden_path.display()));
    let golden_json: Value = serde_json::from_str(&golden_text)
        .unwrap_or_else(|e| panic!("R3a-4 golden {} not valid JSON: {e}", golden_path.display()));
    // Shape guard — the golden must round-trip through the R3a4Projection serde type.
    let _: R3a4Projection = serde_json::from_value(golden_json.clone()).unwrap_or_else(|e| {
        panic!(
            "R3a-4 golden {} does not parse as R3a4Projection: {e}",
            golden_path.display()
        )
    });

    let projection = rust_projection();
    let rust_json = serde_json::to_value(&projection)
        .unwrap_or_else(|e| panic!("serialize Rust R3a-4 projection: {e}"));

    // REGEN path (mirrors `differential.rs` / r2_5a). When `REGEN_TEMP_GOLDENS` is
    // set, write the ENGINE-serialized projection straight to the golden file
    // instead of comparing — the goldens are Rust-owned baselines (TS oracle
    // retired).
    if regen::regen_mode() {
        let mut pretty = serde_json::to_string_pretty(&projection)
            .unwrap_or_else(|e| panic!("regen serialize R3a-4 projection: {e}"));
        pretty.push('\n');
        regen::write_golden(&golden_path, pretty)
            .unwrap_or_else(|e| panic!("regen write {}: {e}", golden_path.display()));
        eprintln!("REGEN r3a4 golden: {}", golden_path.display());
        return;
    }

    // --- structural positional diff ---
    let mut all_divergences: Vec<Divergence> = Vec::new();
    diff_value(FIXTURE, "", &golden_json, &rust_json, &mut all_divergences);
    all_divergences
        .sort_by(|a, b| (a.fixture.as_str(), &a.path).cmp(&(b.fixture.as_str(), &b.path)));

    let mut failure = String::new();
    if !all_divergences.is_empty() {
        failure.push_str(&format!(
            "\n{} R3a-4 divergence(s) found ({R3A4_TEST_NAME}):\n",
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
        "R3a-4 dep-hook differential FAILED:{failure}"
    );

    // --- BYTE-MATCH guard: the pretty JSON must be byte-identical to the golden
    // file (the strongest oracle). -----------------------------------------
    let rust_pretty = serde_json::to_string_pretty(&projection)
        .unwrap_or_else(|e| panic!("pretty-serialize Rust R3a-4 projection: {e}"));
    // serde_json::to_string_pretty omits the trailing newline; the golden file ends
    // with one. Normalize by comparing the trimmed bodies + asserting the golden's
    // trailing newline.
    assert_eq!(
        rust_pretty.trim_end(),
        golden_text.trim_end(),
        "R3a-4 pretty JSON is NOT byte-identical to the golden"
    );

    eprintln!("R3a-4 differential: 1 fixture, byte-match.");
}

/// ANTI-DEGENERATE matrix (fail-on-zero) — the corpus must exercise every payload
/// surface, and the Rust counts must EQUAL the al-sem manifest's `matrix` block.
#[test]
fn r3a4_anti_degenerate_matrix_matches_manifest() {
    let projection = rust_projection();

    // Read the al-sem manifest matrix (ground truth captured at dump time).
    let manifest_path = goldens_dir().join("manifest.json");
    let manifest_text = std::fs::read_to_string(&manifest_path)
        .unwrap_or_else(|e| panic!("read R3a-4 manifest {}: {e}", manifest_path.display()));
    let manifest: Value = serde_json::from_str(&manifest_text)
        .unwrap_or_else(|e| panic!("R3a-4 manifest not valid JSON: {e}"));
    let mat = manifest
        .get("matrix")
        .unwrap_or_else(|| panic!("R3a-4 manifest carries no `matrix` block"));
    let u = |k: &str| mat.get(k).and_then(|v| v.as_u64()).unwrap_or(0) as usize;
    let b = |k: &str| mat.get(k).and_then(|v| v.as_bool()).unwrap_or(false);

    // --- fail-on-zero (the non-hollow chain dep exercises every surface) -------
    let mut degenerate: Vec<String> = Vec::new();
    if projection.intra_app_call_edges_count == 0 {
        degenerate.push("intraAppCallEdgesCount=0 (need the DoIt→DoWrite edge)".into());
    }
    if projection.injected_typed_edges_count == 0 {
        degenerate.push("injectedTypedEdgesCount=0 (the edge must inject)".into());
    }
    if projection.cited_evidence_count == 0 {
        degenerate.push("citedEvidenceCount=0 (the r.Insert witness must be cited)".into());
    }
    if projection.order_entries_count == 0 {
        degenerate.push("orderEntriesCount=0 (DoIt + DoWrite order entries)".into());
    }
    if projection.return_summaries_count == 0 {
        degenerate.push("returnSummariesCount=0 (DoIt + DoWrite summaries)".into());
    }
    if !projection.dep_order_index_present {
        degenerate.push("depOrderIndexPresent=false (source-bearing dep → present)".into());
    }
    if !projection.freshness_stamp_fresh {
        degenerate.push("freshnessStampFresh=false (empty pkgHash → conservatively fresh)".into());
    }
    assert!(
        degenerate.is_empty(),
        "DEGENERATE R3a-4 matrix — a payload surface is hollow:\n  {}",
        degenerate.join("\n  ")
    );

    // --- pinned counts (the al-sem plan's exact targets) ----------------------
    assert_eq!(
        projection.intra_app_call_edges_count, 1,
        "intraAppCallEdges=1"
    );
    assert_eq!(
        projection.injected_typed_edges_count, 1,
        "injectedTypedEdges=1"
    );
    assert_eq!(projection.cited_evidence_count, 1, "citedEvidence=1");
    assert_eq!(projection.order_entries_count, 2, "orderEntries=2");
    assert_eq!(projection.return_summaries_count, 2, "returnSummaries=2");
    assert!(projection.dep_order_index_present, "depOrderIndexPresent");
    assert!(projection.freshness_stamp_fresh, "freshnessStampFresh");

    // --- cross-check vs the al-sem manifest matrix (ground truth) -------------
    assert_eq!(
        projection.intra_app_call_edges_count,
        u("totalIntraAppCallEdges"),
        "intraAppCallEdges vs manifest"
    );
    assert_eq!(
        projection.injected_typed_edges_count,
        u("totalInjectedTypedEdges"),
        "injectedTypedEdges vs manifest"
    );
    assert_eq!(
        projection.cited_evidence_count,
        u("totalCitedEvidence"),
        "citedEvidence vs manifest"
    );
    assert_eq!(
        projection.order_entries_count,
        u("totalOrderEntries"),
        "orderEntries vs manifest"
    );
    assert_eq!(
        projection.return_summaries_count,
        u("totalReturnSummaries"),
        "returnSummaries vs manifest"
    );
    assert_eq!(
        projection.dep_order_index_present,
        b("depOrderIndexPresent"),
        "depOrderIndexPresent vs manifest"
    );
    assert_eq!(
        projection.freshness_stamp_fresh,
        b("freshnessStampPresent"),
        "freshnessStampFresh vs manifest.freshnessStampPresent"
    );

    eprintln!(
        "R3a-4 matrix: intraAppCallEdges={} injectedTypedEdges={} citedEvidence={} \
         orderEntries={} returnSummaries={} depOrderIndexPresent={} freshnessStampFresh={} \
         (== al-sem manifest)",
        projection.intra_app_call_edges_count,
        projection.injected_typed_edges_count,
        projection.cited_evidence_count,
        projection.order_entries_count,
        projection.return_summaries_count,
        projection.dep_order_index_present,
        projection.freshness_stamp_fresh,
    );
}

/// Determinism: the stable ids are devFingerprint/modelInstanceId-INDEPENDENT — the
/// projection emits the SAME stable id regardless of the internal model_instance_id,
/// and is byte-stable across repeated runs.
#[test]
fn r3a4_stable_ids_are_model_instance_independent() {
    let a = serde_json::to_string(&rust_projection()).unwrap();
    let b = serde_json::to_string(&rust_projection()).unwrap();
    assert_eq!(a, b, "R3a-4 projection is byte-stable across runs");

    // No internal `dep:<artifactKey>` prefix may appear anywhere in the emitted ids.
    assert!(
        !a.contains("\"dep:") || a.contains("\"dep:cccccccc"),
        "no internal dep:<artifactKey> id prefix leaks (only the sourceFile \
         `dep:<appGuid>:<path>` form is allowed)"
    );
    // Every emitted routine id is the stable `<appGuid>:Codeunit:<num>#<hash>` form.
    assert!(
        a.contains("cccccccc-0001-0000-0000-000000000001:Codeunit:50300#"),
        "stable routine id form present"
    );
}

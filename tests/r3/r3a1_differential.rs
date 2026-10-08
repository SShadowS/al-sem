//! R3a-1 — L4 GRAPH SUBSTRATE differential (combined graph + Tarjan SCC) over the
//! SOURCE-ONLY corpus + the anti-degenerate matrix.
//!
//! For each committed al-sem golden under `tests/r3a1-goldens/<fixture>.r3a1.golden.json`,
//! run the Rust source-only L0→L3→buildCombinedGraph→tarjanScc→projectR3a1
//! (`assemble_and_resolve_workspace_with_program_calls(...).project_r3a1_combined_graph()`)
//! over the matching `tests/r0-corpus/<fixture>` workspace and assert it
//! BYTE-MATCHES the golden (structural positional diff over the already-canonically-
//! sorted projection). The same `ws-*` SOURCE-ONLY corpus the al-sem dump read.
//!
//! ## Capture point (R3a-1)
//!
//! POST-`buildCombinedGraph` / POST-`tarjanScc` / PRE-`computeSummaries` — the L4
//! GRAPH + SCC ONLY. NO `RoutineSummary` fields, NO dep hooks (R3a-4), NO cross-app
//! (the `.app`-bearing + empty fail-closed fixtures the al-sem dump EXCLUDED are not
//! in the golden set, so they never enter this corpus). modelInstanceId is `r0` on
//! BOTH sides; the projection keys by stable ids (modelInstanceId-independent).
//!
//! ## ANTI-DEGENERATE matrix (fail-on-zero, Rev 2 #1/#4)
//!
//! Computed from the RUST output (proves the graph + SCC actually FIRE, not
//! "empty == empty"): ≥3 distinct combined-edge `kind`s; ≥1 `UncertaintyEdge`; ≥1
//! `event-dispatch` combined edge; nonzero `typedEdges`; ≥1 RECURSIVE SCC; ≥1
//! multi-member SCC. An oracle cross-check asserts the corpus-wide Rust matrix
//! equals the al-sem `manifest.json` matrix (ground truth).
//!
//! ## Divergence gating
//!
//! Direct strict comparison — the test fails on ANY divergence between the Rust
//! projection and the al-sem golden. No allowlist.

use std::collections::BTreeMap;
use std::path::PathBuf;

use al_sem::engine::l4::combined_graph::R3a1Projection;
use al_sem::program::model::program_calls::assemble_and_resolve_workspace_with_program_calls;
use serde_json::Value;

use crate::regen;

/// Keys that must NEVER appear on either side of the R3a-1 comparison — later-gate
/// (R3a-2/3/4) surfaces. Mirrors the al-sem manifest's `forbiddenKeys`.
const R3A1_FORBIDDEN_KEYS: &[&str] = &[
    "dbEffects",
    "uncertainties",
    "parameterRoles",
    "inRecursiveCycle",
    "hasUnresolvedCalls",
    "capabilityFactsDirect",
    "capabilityFactsInherited",
    "coverage",
    "summary",
    "fieldEffects",
    "citedDepEvidence",
    "depOrderIndex",
    "intraAppCallEdges",
];

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn goldens_dir() -> PathBuf {
    repo_root().join("tests").join("r3a1-goldens")
}

fn corpus_dir() -> PathBuf {
    repo_root().join("tests").join("r0-corpus")
}

/// A single divergence (golden vs rust), with a stable JSON-pointer-ish locator.
#[derive(Debug, Clone)]
struct Divergence {
    fixture: String,
    path: String,
    golden_value: String,
    rust_value: String,
}

/// Discover every `tests/r3a1-goldens/*.r3a1.golden.json`, sorted by fixture name
/// (skips manifest.json + r3a1-vectors.json).
fn discover_goldens() -> Vec<(String, PathBuf)> {
    let dir = goldens_dir();
    let mut out = Vec::new();
    let entries = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("failed to read R3a-1 goldens dir {}: {e}", dir.display()));
    for entry in entries {
        let entry = entry.expect("dir entry");
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.ends_with(".r3a1.golden.json") {
            continue;
        }
        let fixture = name.trim_end_matches(".r3a1.golden.json").to_string();
        out.push((fixture, entry.path()));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// The R3a-1 anti-degenerate matrix (mirrors the al-sem dump's `MatrixCounts` —
/// the corpus-wide totals the manifest carries).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct R3a1Matrix {
    /// combined-edge count per kind.
    edges_by_kind: BTreeMap<String, usize>,
    combined_edge_count: usize,
    uncertainty_edge_count: usize,
    /// combined edges of kind "event-dispatch".
    event_dispatch_edge_count: usize,
    typed_edge_count: usize,
    /// typed edges of kind "event-dispatch".
    typed_event_dispatch_count: usize,
    scc_count: usize,
    recursive_scc_count: usize,
    multi_member_scc_count: usize,
}

impl R3a1Matrix {
    fn add(&mut self, o: &R3a1Matrix) {
        for (k, v) in &o.edges_by_kind {
            *self.edges_by_kind.entry(k.clone()).or_insert(0) += v;
        }
        self.combined_edge_count += o.combined_edge_count;
        self.uncertainty_edge_count += o.uncertainty_edge_count;
        self.event_dispatch_edge_count += o.event_dispatch_edge_count;
        self.typed_edge_count += o.typed_edge_count;
        self.typed_event_dispatch_count += o.typed_event_dispatch_count;
        self.scc_count += o.scc_count;
        self.recursive_scc_count += o.recursive_scc_count;
        self.multi_member_scc_count += o.multi_member_scc_count;
    }
}

/// Compute the matrix for ONE projection `Value` (golden OR rust). The shapes are
/// identical, so the same walker serves both. Faithful port of al-sem `countMatrix`
/// (`scripts/dump-r3a1-combined-graph.ts`).
fn matrix_of(proj: &Value) -> R3a1Matrix {
    let mut m = R3a1Matrix::default();
    if let Some(edges) = proj.get("combinedEdges").and_then(|e| e.as_array()) {
        for e in edges {
            m.combined_edge_count += 1;
            let kind = e.get("kind").and_then(|k| k.as_str()).unwrap_or("");
            *m.edges_by_kind.entry(kind.to_string()).or_insert(0) += 1;
            if kind == "event-dispatch" {
                m.event_dispatch_edge_count += 1;
            }
        }
    }
    m.uncertainty_edge_count = proj
        .get("uncertaintyEdges")
        .and_then(|u| u.as_array())
        .map(|a| a.len())
        .unwrap_or(0);
    if let Some(typed) = proj.get("typedEdges").and_then(|t| t.as_array()) {
        m.typed_edge_count = typed.len();
        for te in typed {
            if te.get("kind").and_then(|k| k.as_str()) == Some("event-dispatch") {
                m.typed_event_dispatch_count += 1;
            }
        }
    }
    if let Some(sccs) = proj.get("sccs").and_then(|s| s.as_array()) {
        m.scc_count = sccs.len();
        for s in sccs {
            if s.get("recursive")
                .and_then(|r| r.as_bool())
                .unwrap_or(false)
            {
                m.recursive_scc_count += 1;
            }
            let members = s
                .get("members")
                .and_then(|mm| mm.as_array())
                .map(|a| a.len())
                .unwrap_or(0);
            if members > 1 {
                m.multi_member_scc_count += 1;
            }
        }
    }
    m
}

/// Read the al-sem manifest's corpus-wide `matrix` block as an `R3a1Matrix` (the
/// ground-truth oracle the Rust matrix is cross-checked against).
fn manifest_matrix() -> R3a1Matrix {
    let path = goldens_dir().join("manifest.json");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("read R3a-1 manifest {}: {e}", path.display()));
    let json: Value = serde_json::from_str(&text)
        .unwrap_or_else(|e| panic!("R3a-1 manifest {} not valid JSON: {e}", path.display()));
    let mat = json
        .get("matrix")
        .unwrap_or_else(|| panic!("R3a-1 manifest carries no `matrix` block"));
    let mut m = R3a1Matrix::default();
    if let Some(by_kind) = mat.get("edgesByKind").and_then(|e| e.as_object()) {
        for (k, v) in by_kind {
            m.edges_by_kind
                .insert(k.clone(), v.as_u64().unwrap_or(0) as usize);
        }
    }
    let u = |k: &str| mat.get(k).and_then(|v| v.as_u64()).unwrap_or(0) as usize;
    m.combined_edge_count = u("combinedEdgeCount");
    m.uncertainty_edge_count = u("uncertaintyEdgeCount");
    m.event_dispatch_edge_count = u("eventDispatchEdgeCount");
    m.typed_edge_count = u("typedEdgeCount");
    m.typed_event_dispatch_count = u("typedEventDispatchCount");
    m.scc_count = u("sccCount");
    m.recursive_scc_count = u("recursiveSccCount");
    m.multi_member_scc_count = u("multiMemberSccCount");
    m
}

/// Recursively collect every forbidden object-key in `value`, with its JSON
/// pointer path.
fn scan_forbidden(value: &Value, path: &str, hits: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            for (k, v) in map {
                let child = format!("{path}.{k}");
                if R3A1_FORBIDDEN_KEYS.contains(&k.as_str()) {
                    hits.push(child.clone());
                }
                scan_forbidden(v, &child, hits);
            }
        }
        Value::Array(arr) => {
            for (i, v) in arr.iter().enumerate() {
                scan_forbidden(v, &format!("{path}[{i}]"), hits);
            }
        }
        _ => {}
    }
}

/// Recursively diff two projection values POSITIONALLY (both sides are already
/// canonically sorted by the projection — combinedEdges by node→edgeSortKey,
/// uncertaintyEdges by uncertaintySortKey, typedEdges by emission order, sccs by
/// reverse-topo order with members sorted). The edge/SCC MULTISET + ORDER is the
/// comparison surface — positional comparison over the Vecs preserves both.
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

/// The R3a-1 combined-graph + SCC differential pass + anti-degenerate matrix. The
/// corpus is the full SOURCE-ONLY `ws-*` set (158 fixtures); both sides run the
/// SAME source over the byte-identical workspace fixtures.
#[test]
fn differential_r3a1_combined_graph_match_goldens() {
    let goldens = discover_goldens();
    assert!(
        !goldens.is_empty(),
        "no R3a-1 goldens discovered under {} — corpus missing?",
        goldens_dir().display()
    );

    let mut all_divergences: Vec<Divergence> = Vec::new();
    let mut forbidden_hits: Vec<String> = Vec::new();
    let mut rust_mat = R3a1Matrix::default();
    let mut golden_mat = R3a1Matrix::default();

    for (fixture, golden_path) in &goldens {
        let fixture_dir = corpus_dir().join(fixture);
        assert!(
            fixture_dir.is_dir(),
            "R3a-1 golden {} has no matching in-repo fixture at {} (offline corpus incomplete)",
            golden_path.display(),
            fixture_dir.display()
        );

        // Golden side: parse as JSON AND validate it parses as the R3a1Projection
        // serde type (shape guard — structurally omits every later-gate field).
        let golden_text = std::fs::read_to_string(golden_path)
            .unwrap_or_else(|e| panic!("read R3a-1 golden {}: {e}", golden_path.display()));
        let golden_json: Value = serde_json::from_str(&golden_text).unwrap_or_else(|e| {
            panic!(
                "R3a-1 golden {} is not valid JSON: {e}",
                golden_path.display()
            )
        });
        let _: R3a1Projection = serde_json::from_value(golden_json.clone()).unwrap_or_else(|e| {
            panic!(
                "R3a-1 golden {} does not parse as R3a1Projection: {e}",
                golden_path.display()
            )
        });

        // Rust side: source-only assemble+resolve → buildCombinedGraph → tarjanScc →
        // projectR3a1 → JSON. Fail-closed layouts yield an empty projection (never
        // throws). The al-sem dump EXCLUDED those fail-closed fixtures, so the golden
        // set never carries one — but the empty fallback keeps this total.
        let projection = match assemble_and_resolve_workspace_with_program_calls(&fixture_dir) {
            Some(resolved) => resolved.project_r3a1_combined_graph(),
            None => R3a1Projection {
                combined_edges: vec![],
                uncertainty_edges: vec![],
                typed_edges: vec![],
                sccs: vec![],
            },
        };
        let rust_json = serde_json::to_value(&projection)
            .unwrap_or_else(|e| panic!("serialize Rust R3a-1 projection for {fixture}: {e}"));

        // Rust-owned baseline: `REGEN_TEMP_GOLDENS=1` rewrites each golden from THIS
        // engine (al-sem byte-parity retired — see CLAUDE.md). The manifest `matrix`
        // block is then updated by hand to the Rust corpus totals.
        if regen::regen_mode() {
            let mut pretty = serde_json::to_string_pretty(&projection)
                .unwrap_or_else(|e| panic!("regen serialize r3a1 {fixture}: {e}"));
            pretty.push('\n');
            std::fs::write(golden_path, pretty)
                .unwrap_or_else(|e| panic!("regen write {}: {e}", golden_path.display()));
            eprintln!("REGEN r3a1 golden: {}", golden_path.display());
            continue;
        }

        // Forbidden later-gate field scan on BOTH sides.
        scan_forbidden(
            &golden_json,
            &format!("{fixture}:golden"),
            &mut forbidden_hits,
        );
        scan_forbidden(&rust_json, &format!("{fixture}:rust"), &mut forbidden_hits);

        // Anti-degenerate matrices (Rust drives the gate; golden is the oracle
        // cross-check at the per-fixture level via the corpus total).
        rust_mat.add(&matrix_of(&rust_json));
        golden_mat.add(&matrix_of(&golden_json));

        // Positional structural diff (both sides already canonically sorted).
        diff_value(fixture, "", &golden_json, &rust_json, &mut all_divergences);
    }

    if regen::regen_mode() {
        eprintln!(
            "REGEN r3a1: wrote {} golden(s); now update tests/r3a1-goldens/manifest.json `matrix` to the Rust totals",
            goldens.len()
        );
        return;
    }

    all_divergences
        .sort_by(|a, b| (a.fixture.as_str(), &a.path).cmp(&(b.fixture.as_str(), &b.path)));

    // --- Forbidden-field guard (hard fail, never allowlistable) -------------
    assert!(
        forbidden_hits.is_empty(),
        "FORBIDDEN later-gate field(s) leaked into the R3a-1 comparison \
         (golden or rust):\n  {}",
        forbidden_hits.join("\n  ")
    );

    // --- ANTI-DEGENERATE matrix gate (fail-on-zero, Rev 2 #1/#4) ------------
    eprintln!(
        "R3a-1 matrix ({} fixture(s)): edgesByKind={:?} combined={} unc={} evtDispatch={} \
         typed={} typedEvt={} sccs={} recursive={} multiMember={}",
        goldens.len(),
        rust_mat.edges_by_kind,
        rust_mat.combined_edge_count,
        rust_mat.uncertainty_edge_count,
        rust_mat.event_dispatch_edge_count,
        rust_mat.typed_edge_count,
        rust_mat.typed_event_dispatch_count,
        rust_mat.scc_count,
        rust_mat.recursive_scc_count,
        rust_mat.multi_member_scc_count,
    );
    let distinct_kinds = rust_mat.edges_by_kind.values().filter(|v| **v > 0).count();
    let mut degenerate: Vec<String> = Vec::new();
    if distinct_kinds < 3 {
        degenerate.push(format!(
            "distinctEdgeKinds={distinct_kinds} (<3) — need ≥3 distinct combined-edge kinds"
        ));
    }
    if rust_mat.uncertainty_edge_count == 0 {
        degenerate.push("uncertaintyEdgeCount=0 (need ≥1)".to_string());
    }
    if rust_mat.event_dispatch_edge_count == 0 {
        degenerate
            .push("eventDispatchEdgeCount=0 (need ≥1 event-dispatch combined edge)".to_string());
    }
    if rust_mat.typed_edge_count == 0 {
        degenerate.push("typedEdgeCount=0 (need nonzero)".to_string());
    }
    if rust_mat.recursive_scc_count == 0 {
        degenerate.push("recursiveSccCount=0 (need ≥1 RECURSIVE SCC)".to_string());
    }
    if rust_mat.multi_member_scc_count == 0 {
        degenerate.push("multiMemberSccCount=0 (need ≥1 multi-member SCC)".to_string());
    }
    assert!(
        degenerate.is_empty(),
        "DEGENERATE R3a-1 matrix — the combined graph + SCC are NOT exercising the \
         full surface (an empty/trivial port would pass a pure equality diff):\n  {}",
        degenerate.join("\n  "),
    );

    // Oracle cross-check #1: the Rust corpus matrix == the GOLDEN corpus matrix
    // (recomputed from the goldens — independent of the manifest).
    assert_eq!(
        rust_mat, golden_mat,
        "R3a-1 matrix MISMATCH: Rust corpus matrix != recomputed-from-goldens matrix\n  \
         rust   = {rust_mat:?}\n  golden = {golden_mat:?}",
    );
    // Oracle cross-check #2: the Rust corpus matrix == the al-sem `manifest.json`
    // matrix block (al-sem ground truth, captured at dump time).
    let manifest_mat = manifest_matrix();
    assert_eq!(
        rust_mat, manifest_mat,
        "R3a-1 matrix MISMATCH vs al-sem manifest.json oracle\n  rust     = {rust_mat:?}\n  \
         manifest = {manifest_mat:?}",
    );

    // --- Strict divergence gate (direct assert; no allowlist) --------------
    let mut failure = String::new();
    if !all_divergences.is_empty() {
        failure.push_str(&format!(
            "\n{} R3a-1 divergence(s) found:\n",
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
        "R3a-1 combined-graph + SCC differential FAILED:{failure}"
    );

    eprintln!(
        "R3a-1 differential: {} fixture(s), 0 divergences.",
        goldens.len(),
    );
}

//! R3a-3 — CAPABILITY CONE + COVERAGE differential (the LAST two RoutineSummary
//! fields) over the SOURCE-ONLY corpus + the anti-degenerate matrix.
//!
//! For each committed al-sem golden under `tests/r3a3-goldens/<fixture>.r3a3.golden.json`,
//! run the Rust source-only L0→L3→cone/coverage pass
//! (`assemble_and_resolve_workspace_with_program_calls(...)` → `project_r3a3(...)`) over the matching
//! `tests/r0-corpus/<fixture>` workspace and assert it BYTE-MATCHES the golden (structural
//! positional diff over the already-canonically-sorted projection). The SAME `ws-*`
//! SOURCE-ONLY corpus the al-sem dump read.
//!
//! ## Capture point (R3a-3)
//!
//! POST-`computeSummaries` cone pass — the final mutated `routine.summary`
//! `capabilityFactsDirect` / `capabilityFactsInherited` / `coverage`. NO dep hooks
//! (R3a-4); the R3a-2 CORE (dbEffects/uncertainties/parameterRoles) is NOT projected here.
//! modelInstanceId is `r0` on BOTH sides; the projection keys by stable ids.
//!
//! ## ANTI-DEGENERATE matrix (fail-on-zero) — REAL BFS counts (review note 1)
//!
//! The distribution / status counts (routinesWithInheritedFacts,
//! coveragesWithNonTrivialInheritedStatus, provenance/confidence/op/via/directStatus/
//! inheritedStatus distributions) are computed from the RUST projection and CROSS-CHECKED
//! against the al-sem `manifest.json` (ground truth) where comparable. The two PROXY
//! fields the al-sem manifest carries (`factsWithMoreThan1HopWitness` = every inherited
//! fact; `equalDistanceTies` = the tie fixture's inherited-fact routines) are NOT
//! recomputed against the manifest — instead the Rust side computes the GENUINE
//! BFS-distance counts (`compute_r3a3_real_matrix`): real >1-hop shortest witnesses + real
//! equal-distance ties (≥2 distinct first-hop edges reaching a key at the same minimum
//! distance). Both must be > 0 (the cone genuinely propagated multi-hop + a real tie fired).
//!
//! ## Divergence comparison
//!
//! The harness performs a direct strict comparison: any divergence between the
//! golden and the Rust projection fails the test outright. There is no allowlist.

use std::collections::BTreeMap;
use std::path::PathBuf;

use al_sem::engine::l4::capability_cone::{R3a3Projection, compute_r3a3_real_matrix, project_r3a3};
use al_sem::program::model::program_calls::assemble_and_resolve_workspace_with_program_calls;
use serde_json::Value;

use crate::regen;

const R3A3_TEST_NAME: &str = "differential_r3a3_cone_coverage_match_goldens";

/// Re-inline string-only array blocks to match the EXACT on-disk r3a3 golden form
/// (retired al-sem dumper): a `"key": [` block whose body is only quoted-string
/// elements collapses to `"key": ["a", "b"]` IFF the resulting line fits within
/// `INLINE_WIDTH` columns (one column per leading tab); otherwise it stays
/// expanded. Operates on tab-indented `PrettyFormatter` output. Validated
/// byte-identical against all 160 committed r3a3 goldens (only short `reasons`
/// arrays inline; long-id `unknownTargets` arrays stay expanded). REGEN-only.
const INLINE_WIDTH: usize = 80;

fn inline_short_string_arrays(expanded: &str) -> String {
    let lines: Vec<&str> = expanded.split('\n').collect();
    let mut out: Vec<String> = Vec::with_capacity(lines.len());
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        // Match a key-array opener: `<tabs>"key": [`
        let trimmed = line.trim_start_matches('\t');
        if trimmed.ends_with(": [") && trimmed.starts_with('"') {
            // Collect the body until the matching `<tabs>](,?)`.
            let mut elems: Vec<&str> = Vec::new();
            let mut j = i + 1;
            let mut all_strings = true;
            let mut close_idx = None;
            while j < lines.len() {
                let l = lines[j].trim_start_matches('\t');
                if l == "]" || l == "]," {
                    close_idx = Some(j);
                    break;
                }
                let elem = l.trim_end_matches(',');
                // A string element is a quoted token with no nested structure.
                if elem.starts_with('"') && elem.ends_with('"') && elem.len() >= 2 {
                    elems.push(elem);
                } else {
                    all_strings = false;
                    break;
                }
                j += 1;
            }
            if all_strings
                && !elems.is_empty()
                && let Some(ci) = close_idx
            {
                let tab_count = line.len() - line.trim_start_matches('\t').len();
                let trailing_comma = lines[ci].trim_start_matches('\t') == "],";
                let inline = format!("[{}]", elems.join(", "));
                // width = tabs + `"key": ` prefix + inline body
                let prefix_len = trimmed.len() - 1; // `"key": [` minus the `[`
                if tab_count + prefix_len + inline.len() <= INLINE_WIDTH {
                    let mut rebuilt = String::new();
                    for _ in 0..tab_count {
                        rebuilt.push('\t');
                    }
                    // `"key": ` then inline
                    rebuilt.push_str(&trimmed[..prefix_len]);
                    rebuilt.push_str(&inline);
                    if trailing_comma {
                        rebuilt.push(',');
                    }
                    out.push(rebuilt);
                    i = ci + 1;
                    continue;
                }
            }
        }
        out.push(line.to_string());
        i += 1;
    }
    out.join("\n")
}

/// Keys that must NEVER appear on either side of the R3a-3 comparison — the R3a-2
/// CORE fields + later-gate (R3a-4) dep-hook surfaces. Mirrors the al-sem manifest's
/// `forbiddenKeys`.
const R3A3_FORBIDDEN_KEYS: &[&str] = &[
    "dbEffects",
    "uncertainties",
    "parameterRoles",
    "inRecursiveCycle",
    "hasUnresolvedCalls",
    "fieldEffects",
    "citedDepEvidence",
    "depOrderIndex",
    "intraAppCallEdges",
];

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn goldens_dir() -> PathBuf {
    repo_root().join("tests").join("r3a3-goldens")
}

fn corpus_dir() -> PathBuf {
    repo_root().join("tests").join("r0-corpus")
}

#[derive(Debug, Clone)]
struct Divergence {
    fixture: String,
    path: String,
    golden_value: String,
    rust_value: String,
}

/// Discover every `tests/r3a3-goldens/*.r3a3.golden.json`, sorted by fixture name.
fn discover_goldens() -> Vec<(String, PathBuf)> {
    let dir = goldens_dir();
    let mut out = Vec::new();
    let entries = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("failed to read R3a-3 goldens dir {}: {e}", dir.display()));
    for entry in entries {
        let entry = entry.expect("dir entry");
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.ends_with(".r3a3.golden.json") {
            continue;
        }
        let fixture = name.trim_end_matches(".r3a3.golden.json").to_string();
        out.push((fixture, entry.path()));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// The R3a-3 distribution / status matrix (the al-sem manifest fields that are
/// computed IDENTICALLY on both sides → directly cross-checkable).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct R3a3DistMatrix {
    routine_count: usize,
    routines_with_inherited_facts: usize,
    coverages_with_non_trivial_inherited_status: usize,
    provenance: BTreeMap<String, usize>,
    confidence: BTreeMap<String, usize>,
    op: BTreeMap<String, usize>,
    via: BTreeMap<String, usize>,
    direct_status: BTreeMap<String, usize>,
    inherited_status: BTreeMap<String, usize>,
}

impl R3a3DistMatrix {
    fn add(&mut self, o: &R3a3DistMatrix) {
        self.routine_count += o.routine_count;
        self.routines_with_inherited_facts += o.routines_with_inherited_facts;
        self.coverages_with_non_trivial_inherited_status +=
            o.coverages_with_non_trivial_inherited_status;
        for (m, om) in [
            (&mut self.provenance, &o.provenance),
            (&mut self.confidence, &o.confidence),
            (&mut self.op, &o.op),
            (&mut self.via, &o.via),
            (&mut self.direct_status, &o.direct_status),
            (&mut self.inherited_status, &o.inherited_status),
        ] {
            for (k, v) in om {
                *m.entry(k.clone()).or_insert(0) += v;
            }
        }
    }
}

/// Compute the distribution matrix for ONE projection `Value` (golden OR rust).
fn dist_matrix_of(proj: &Value) -> R3a3DistMatrix {
    let mut m = R3a3DistMatrix::default();
    let Some(summaries) = proj.get("summaries").and_then(|s| s.as_array()) else {
        return m;
    };
    for s in summaries {
        m.routine_count += 1;
        let direct = s
            .get("capabilityFactsDirect")
            .and_then(|f| f.as_array())
            .cloned()
            .unwrap_or_default();
        let inherited = s
            .get("capabilityFactsInherited")
            .and_then(|f| f.as_array())
            .cloned()
            .unwrap_or_default();
        if !inherited.is_empty() {
            m.routines_with_inherited_facts += 1;
        }
        for f in direct.iter().chain(inherited.iter()) {
            let g = |k: &str| f.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
            *m.provenance.entry(g("provenance")).or_insert(0) += 1;
            *m.confidence.entry(g("confidence")).or_insert(0) += 1;
            *m.op.entry(g("op")).or_insert(0) += 1;
            *m.via.entry(g("via")).or_insert(0) += 1;
        }
        let cov = s.get("coverage");
        let ds = cov
            .and_then(|c| c.get("directStatus"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let is = cov
            .and_then(|c| c.get("inheritedStatus"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if is != "complete" {
            m.coverages_with_non_trivial_inherited_status += 1;
        }
        *m.direct_status.entry(ds).or_insert(0) += 1;
        *m.inherited_status.entry(is).or_insert(0) += 1;
    }
    m
}

/// Read the comparable subset of the al-sem manifest's corpus-wide `matrix`.
fn manifest_dist_matrix() -> R3a3DistMatrix {
    let path = goldens_dir().join("manifest.json");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("read R3a-3 manifest {}: {e}", path.display()));
    let json: Value = serde_json::from_str(&text)
        .unwrap_or_else(|e| panic!("R3a-3 manifest {} not valid JSON: {e}", path.display()));
    let mat = json
        .get("matrix")
        .unwrap_or_else(|| panic!("R3a-3 manifest carries no `matrix` block"));
    let u = |k: &str| mat.get(k).and_then(|v| v.as_u64()).unwrap_or(0) as usize;
    let dist = |k: &str| -> BTreeMap<String, usize> {
        let mut out = BTreeMap::new();
        if let Some(o) = mat.get(k).and_then(|v| v.as_object()) {
            for (kk, vv) in o {
                out.insert(kk.clone(), vv.as_u64().unwrap_or(0) as usize);
            }
        }
        out
    };
    R3a3DistMatrix {
        routine_count: u("routineCount"),
        routines_with_inherited_facts: u("routinesWithInheritedFacts"),
        coverages_with_non_trivial_inherited_status: u("coveragesWithNonTrivialInheritedStatus"),
        provenance: dist("provenanceDistribution"),
        confidence: dist("confidenceDistribution"),
        op: dist("opDistribution"),
        via: dist("viaDistribution"),
        direct_status: dist("directStatusDistribution"),
        inherited_status: dist("inheritedStatusDistribution"),
    }
}

/// Recursively collect every forbidden object-key in `value`.
fn scan_forbidden(value: &Value, path: &str, hits: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            for (k, v) in map {
                let child = format!("{path}.{k}");
                if R3A3_FORBIDDEN_KEYS.contains(&k.as_str()) {
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

#[test]
fn differential_r3a3_cone_coverage_match_goldens() {
    let goldens = discover_goldens();
    assert!(
        !goldens.is_empty(),
        "no R3a-3 goldens discovered under {} — corpus missing?",
        goldens_dir().display()
    );

    let mut all_divergences: Vec<Divergence> = Vec::new();
    let mut forbidden_hits: Vec<String> = Vec::new();
    let mut rust_dist = R3a3DistMatrix::default();
    let mut golden_dist = R3a3DistMatrix::default();
    // REAL (BFS-derived) counts — the self-validating proxies (review note 1).
    let mut real_routines_with_inherited = 0usize;
    let mut real_more_than_1_hop = 0usize;
    let mut real_equal_distance_ties = 0usize;

    for (fixture, golden_path) in &goldens {
        let fixture_dir = corpus_dir().join(fixture);
        assert!(
            fixture_dir.is_dir(),
            "R3a-3 golden {} has no matching in-repo fixture at {} (offline corpus incomplete)",
            golden_path.display(),
            fixture_dir.display()
        );

        let golden_text = std::fs::read_to_string(golden_path)
            .unwrap_or_else(|e| panic!("read R3a-3 golden {}: {e}", golden_path.display()));
        let golden_json: Value = serde_json::from_str(&golden_text).unwrap_or_else(|e| {
            panic!(
                "R3a-3 golden {} is not valid JSON: {e}",
                golden_path.display()
            )
        });
        // Shape guard — the golden must parse as the R3a3Projection serde type.
        let _: R3a3Projection = serde_json::from_value(golden_json.clone()).unwrap_or_else(|e| {
            panic!(
                "R3a-3 golden {} does not parse as R3a3Projection: {e}",
                golden_path.display()
            )
        });

        let resolved = assemble_and_resolve_workspace_with_program_calls(&fixture_dir);
        let projection = match &resolved {
            Some(r) => project_r3a3(r),
            None => R3a3Projection { summaries: vec![] },
        };
        let rust_json = serde_json::to_value(&projection)
            .unwrap_or_else(|e| panic!("serialize Rust R3a-3 projection for {fixture}: {e}"));

        // REGEN path (temp-state epoch rebaseline, Task 16). When
        // `REGEN_TEMP_GOLDENS` is set, write the ENGINE projection to the golden
        // file (matching the on-disk pretty form) instead of comparing — the
        // goldens are Rust-owned baselines (TS oracle retired). The r3a3 goldens
        // were dumped by al-sem with TAB indentation (`JSON.stringify(x, null,
        // "\t")`), so we serialize with a tab `PrettyFormatter` to keep the diff
        // minimal (the comparison is structural via serde_json::Value, so indent
        // is irrelevant to PASS — but matters for a reviewable diff).
        if regen::regen_mode() {
            // ORDER-PRESERVING regen: the (retired) al-sem golden orders the `extra`
            // object's keys differently from the Rust struct's field order, so a
            // naive re-serialize would churn ~all goldens with pure key-order noise
            // that is NOT a designed temp-state change. To keep the rebaseline diff
            // FAITHFUL to the ledger (only tempState flips + recordVariableId
            // bindings), we only REWRITE a golden when its CONTENT actually changed
            // (structural `serde_json::Value` inequality vs the on-disk golden).
            // Structurally-identical goldens (key-order-only deltas) are left
            // byte-for-byte untouched.
            if rust_json == golden_json {
                continue;
            }
            // Serialize the STRUCT (preserves field-declaration order — Value would
            // re-sort keys alphabetically) with a tab `PrettyFormatter`, then a
            // text post-pass re-inlines the short string-only arrays (`reasons`) to
            // match the al-sem golden form (validated byte-identical against the
            // committed corpus).
            let mut buf = Vec::new();
            let formatter = serde_json::ser::PrettyFormatter::with_indent(b"\t");
            let mut ser = serde_json::Serializer::with_formatter(&mut buf, formatter);
            serde::Serialize::serialize(&projection, &mut ser)
                .unwrap_or_else(|e| panic!("regen serialize R3a-3 {fixture}: {e}"));
            let expanded = String::from_utf8(buf).expect("utf8");
            let mut s = inline_short_string_arrays(&expanded);
            s.push('\n');
            std::fs::write(golden_path, s)
                .unwrap_or_else(|e| panic!("regen write {}: {e}", golden_path.display()));
            eprintln!(
                "REGEN r3a3 golden (CONTENT CHANGED): {}",
                golden_path.display()
            );
            continue;
        }

        scan_forbidden(
            &golden_json,
            &format!("{fixture}:golden"),
            &mut forbidden_hits,
        );
        scan_forbidden(&rust_json, &format!("{fixture}:rust"), &mut forbidden_hits);

        rust_dist.add(&dist_matrix_of(&rust_json));
        golden_dist.add(&dist_matrix_of(&golden_json));

        if let Some(r) = &resolved {
            let real = compute_r3a3_real_matrix(r);
            real_routines_with_inherited += real.routines_with_inherited_facts;
            real_more_than_1_hop += real.facts_with_more_than_1_hop_witness;
            real_equal_distance_ties += real.equal_distance_ties;
        }

        diff_value(fixture, "", &golden_json, &rust_json, &mut all_divergences);
    }

    // REGEN mode wrote every golden above and asserts nothing.
    if regen::regen_mode() {
        eprintln!("REGEN r3a3: wrote {} golden(s)", goldens.len());
        return;
    }

    all_divergences
        .sort_by(|a, b| (a.fixture.as_str(), &a.path).cmp(&(b.fixture.as_str(), &b.path)));

    // --- Forbidden-field guard (hard fail, never allowlistable) -------------
    assert!(
        forbidden_hits.is_empty(),
        "FORBIDDEN R3a-2-core / dep-hook field(s) leaked into the R3a-3 comparison \
         (golden or rust):\n  {}",
        forbidden_hits.join("\n  ")
    );

    // --- ANTI-DEGENERATE matrix gate (fail-on-zero) -------------------------
    eprintln!(
        "R3a-3 matrix ({} fixture(s)): routines={} withInheritedFacts(dist)={} \
         nonTrivialInheritedCov={} prov={:?} conf={:?} via={:?} \
         REAL: routinesWithInherited={} >1hop={} ties={}",
        goldens.len(),
        rust_dist.routine_count,
        rust_dist.routines_with_inherited_facts,
        rust_dist.coverages_with_non_trivial_inherited_status,
        rust_dist.provenance,
        rust_dist.confidence,
        rust_dist.via,
        real_routines_with_inherited,
        real_more_than_1_hop,
        real_equal_distance_ties,
    );

    let mut degenerate: Vec<String> = Vec::new();
    if rust_dist.routines_with_inherited_facts == 0 {
        degenerate.push(
            "routinesWithInheritedFacts=0 — need ≥1 routine with inherited capability facts \
             (the cone propagated)"
                .to_string(),
        );
    }
    if real_more_than_1_hop == 0 {
        degenerate.push(
            "REAL factsWithMoreThan1HopWitness=0 — need ≥1 inherited fact with a GENUINE \
             >1-hop shortest witness (BFS-derived)"
                .to_string(),
        );
    }
    if real_equal_distance_ties == 0 {
        degenerate.push(
            "REAL equalDistanceTies=0 — need ≥1 GENUINE equal-distance tie (≥2 distinct \
             first-hop edges reaching a key at the same minimum distance)"
                .to_string(),
        );
    }
    if rust_dist.coverages_with_non_trivial_inherited_status == 0 {
        degenerate.push(
            "coveragesWithNonTrivialInheritedStatus=0 — need ≥1 non-trivial inheritedStatus"
                .to_string(),
        );
    }
    // Each provenance/confidence/op/via kind family present.
    for kind in ["direct", "inherited"] {
        if rust_dist.provenance.get(kind).copied().unwrap_or(0) == 0 {
            degenerate.push(format!("provenance '{kind}' absent (need ≥1)"));
        }
    }
    for kind in ["static", "unresolved"] {
        if rust_dist.confidence.get(kind).copied().unwrap_or(0) == 0 {
            degenerate.push(format!("confidence '{kind}' absent (need ≥1)"));
        }
    }
    for kind in ["self", "call", "event-dispatch"] {
        if rust_dist.via.get(kind).copied().unwrap_or(0) == 0 {
            degenerate.push(format!("via '{kind}' absent (need ≥1)"));
        }
    }
    assert!(
        degenerate.is_empty(),
        "DEGENERATE R3a-3 matrix — the cone/coverage is NOT exercising the full surface:\n  {}",
        degenerate.join("\n  "),
    );

    // Oracle cross-check #1: the Rust corpus DIST matrix == the GOLDEN corpus DIST
    // matrix (recomputed from the goldens — independent of the manifest).
    assert_eq!(
        rust_dist, golden_dist,
        "R3a-3 distribution matrix MISMATCH: Rust corpus matrix != recomputed-from-goldens \
         matrix\n  rust   = {rust_dist:?}\n  golden = {golden_dist:?}",
    );
    // Oracle cross-check #2: the Rust corpus DIST matrix == the al-sem manifest's
    // comparable subset (al-sem ground truth, captured at dump time). The two PROXY
    // fields (factsWithMoreThan1HopWitness / equalDistanceTies) are intentionally NOT
    // cross-checked here — the Rust REAL counts above supersede them.
    let manifest = manifest_dist_matrix();
    assert_eq!(
        rust_dist, manifest,
        "R3a-3 distribution matrix MISMATCH vs al-sem manifest.json oracle\n  rust     = \
         {rust_dist:?}\n  manifest = {manifest:?}",
    );

    // --- Direct strict divergence gate (no allowlist) -----------------------
    let mut failure = String::new();
    if !all_divergences.is_empty() {
        failure.push_str(&format!(
            "\n{} R3a-3 divergence(s) found ({R3A3_TEST_NAME}):\n",
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
        "R3a-3 cone-coverage differential FAILED:{failure}"
    );

    eprintln!(
        "R3a-3 differential: {} fixture(s), 0 divergences.",
        goldens.len(),
    );
}

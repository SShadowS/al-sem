//! R3a-2 — FIXED-POINT SUMMARY CORE differential (the JACOBI summary core) over
//! the SOURCE-ONLY corpus + the anti-degenerate matrix.
//!
//! For each committed al-sem golden under `tests/r3a2-goldens/<fixture>.r3a2.golden.json`,
//! run the Rust source-only L0→L3→buildCombinedGraph→tarjanScc→computeSummaries→projectR3a2
//! (`assemble_and_resolve_workspace_with_program_calls(...)` → `project_r3a2(...)`) over the matching
//! `tests/r0-corpus/<fixture>` workspace and assert it BYTE-MATCHES the golden (structural
//! positional diff over the already-canonically-sorted projection). The SAME `ws-*`
//! SOURCE-ONLY corpus the al-sem dump read.
//!
//! ## Capture point (R3a-2)
//!
//! POST-`computeSummaries` — the final mutated `routine.summary` CORE (dbEffects /
//! uncertainties / parameterRoles / inRecursiveCycle / hasUnresolvedCalls). NO dep hooks
//! (R3a-4), NO cone/coverage (R3a-3 — never declared on the projected types). modelInstanceId
//! is `r0` on BOTH sides; the projection keys by stable ids (modelInstanceId-independent).
//!
//! ## readsFields / writesFields deferral
//!
//! Both sides emit `readsFields: []` / `writesFields: []` in the parameterRoles: al-sem's
//! source-only summaries carry empty field-id lists (field-id resolution is deferred to
//! R3a-3 on BOTH projections), so the differential passes verbatim — confirmed against the
//! goldens. This is NOT a fudge: the al-sem goldens themselves are empty here.
//!
//! ## ANTI-DEGENERATE matrix (fail-on-zero)
//!
//! Computed from the RUST output (proves the fixed point + composition actually FIRE):
//! ≥1 routine with inherited dbEffects (`via != "direct"`); ≥1 opaque-callee uncertainty;
//! ≥1 cross-call parameterRole exit-effect; the 4 reachable via-kinds present + `dynamic`
//! absent; ≥1 recursive-SCC routine + ≥1 recursive SCC. An oracle cross-check asserts the
//! corpus-wide Rust matrix equals the al-sem `manifest.json` matrix block (ground truth).
//! (The ≥2-iteration JACOBI requirement was asserted in the now-deleted
//! `r3a2_trace_differential.rs`, retired with the old Jacobi db-effect solver —
//! see `tests/l4_summary_differential.rs`'s module doc for its replacement,
//! the frozen-baseline anchor over the closed-form v2 solver.)
//!
//! ## Divergence comparison
//!
//! The harness performs a direct strict comparison: any divergence between the
//! golden and the Rust projection fails the test outright. There is no allowlist.

use std::collections::BTreeMap;
use std::path::PathBuf;

use al_sem::engine::l4::summary::{R3a2Projection, project_r3a2};
use al_sem::program::model::program_calls::assemble_and_resolve_workspace_with_program_calls;
use serde_json::Value;

use crate::regen;

const R3A2_TEST_NAME: &str = "differential_r3a2_summary_core_match_goldens";

/// Keys that must NEVER appear on either side of the R3a-2 comparison — later-gate
/// (R3a-3/4) surfaces. Mirrors the al-sem manifest's `forbiddenKeys`.
const R3A2_FORBIDDEN_KEYS: &[&str] = &[
    "capabilityFactsDirect",
    "capabilityFactsInherited",
    "coverage",
    "fieldEffects",
    "citedDepEvidence",
    "depOrderIndex",
    "intraAppCallEdges",
    "directStatus",
    "inheritedStatus",
    "unknownTargets",
];

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn goldens_dir() -> PathBuf {
    repo_root().join("tests").join("r3a2-goldens")
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

/// Discover every `tests/r3a2-goldens/*.r3a2.golden.json`, sorted by fixture name
/// (skips manifest.json + r3a2-vectors.json + the *.r3a2-trace.golden.json traces).
fn discover_goldens() -> Vec<(String, PathBuf)> {
    let dir = goldens_dir();
    let mut out = Vec::new();
    let entries = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("failed to read R3a-2 goldens dir {}: {e}", dir.display()));
    for entry in entries {
        let entry = entry.expect("dir entry");
        let name = entry.file_name().to_string_lossy().to_string();
        // The trace goldens share the `.r3a2-trace.golden.json` suffix; exclude them
        // before the `.r3a2.golden.json` check (which would otherwise NOT match them,
        // but be explicit for clarity).
        if name.ends_with(".r3a2-trace.golden.json") {
            continue;
        }
        if !name.ends_with(".r3a2.golden.json") {
            continue;
        }
        let fixture = name.trim_end_matches(".r3a2.golden.json").to_string();
        out.push((fixture, entry.path()));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// The R3a-2 anti-degenerate matrix (mirrors the al-sem manifest's corpus-wide
/// `matrix` block).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct R3a2Matrix {
    routine_count: usize,
    routines_with_inherited_effects: usize,
    /// dbEffect count per `via` kind.
    via_kinds: BTreeMap<String, usize>,
    recursive_cycle_routines: usize,
    unresolved_call_routines: usize,
    opaque_callee_uncertainties: usize,
    /// uncertainty count per `kind`.
    uncertainty_kinds: BTreeMap<String, usize>,
    parameter_roles_with_cross_call_exit_effect: usize,
}

impl R3a2Matrix {
    fn add(&mut self, o: &R3a2Matrix) {
        self.routine_count += o.routine_count;
        self.routines_with_inherited_effects += o.routines_with_inherited_effects;
        for (k, v) in &o.via_kinds {
            *self.via_kinds.entry(k.clone()).or_insert(0) += v;
        }
        self.recursive_cycle_routines += o.recursive_cycle_routines;
        self.unresolved_call_routines += o.unresolved_call_routines;
        self.opaque_callee_uncertainties += o.opaque_callee_uncertainties;
        for (k, v) in &o.uncertainty_kinds {
            *self.uncertainty_kinds.entry(k.clone()).or_insert(0) += v;
        }
        self.parameter_roles_with_cross_call_exit_effect +=
            o.parameter_roles_with_cross_call_exit_effect;
    }
}

/// `true` if a projected RecordRoleSummary carries a CROSS-CALL exit effect — any of
/// the composed exit-effect tri-states is non-`"no"`. Mirrors the al-sem dump's
/// `hasCrossCallExitEffect` predicate (`scripts/dump-r3a2-summary-core.ts`): the
/// exit-effect fields that compose across call sites.
fn role_has_cross_call_exit_effect(role: &Value) -> bool {
    // Mirrors al-sem `roleHasExitEffect` (scripts/dump-r3a2-summary-core.ts) EXACTLY:
    // these 7 exit-effect tri-states; NOT loadsFromDbParam / initialisesParam.
    const EXIT_EFFECT_FIELDS: &[&str] = &[
        "persistsCurrentRecord",
        "setBasedDbWrites",
        "validatesParam",
        "copiesIntoParam",
        "resetsFiltersOnParam",
        "mutatesParam",
        "dirtyAtExit",
    ];
    EXIT_EFFECT_FIELDS.iter().any(|f| {
        role.get(*f)
            .and_then(|v| v.as_str())
            .map(|s| s != "no")
            .unwrap_or(false)
    })
}

/// Compute the matrix for ONE projection `Value` (golden OR rust). The shapes are
/// identical, so the same walker serves both. Faithful port of al-sem `countMatrix`
/// (`scripts/dump-r3a2-summary-core.ts`).
fn matrix_of(proj: &Value) -> R3a2Matrix {
    let mut m = R3a2Matrix::default();
    let Some(summaries) = proj.get("summaries").and_then(|s| s.as_array()) else {
        return m;
    };
    for s in summaries {
        m.routine_count += 1;

        let db_effects = s.get("dbEffects").and_then(|e| e.as_array());
        let mut has_inherited = false;
        if let Some(effects) = db_effects {
            for e in effects {
                let via = e.get("via").and_then(|v| v.as_str()).unwrap_or("");
                *m.via_kinds.entry(via.to_string()).or_insert(0) += 1;
                if via != "direct" {
                    has_inherited = true;
                }
            }
        }
        if has_inherited {
            m.routines_with_inherited_effects += 1;
        }

        if s.get("inRecursiveCycle")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            m.recursive_cycle_routines += 1;
        }
        if s.get("hasUnresolvedCalls")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            m.unresolved_call_routines += 1;
        }

        if let Some(uncs) = s.get("uncertainties").and_then(|u| u.as_array()) {
            for u in uncs {
                let kind = u.get("kind").and_then(|k| k.as_str()).unwrap_or("");
                *m.uncertainty_kinds.entry(kind.to_string()).or_insert(0) += 1;
                if kind == "opaque-callee" {
                    m.opaque_callee_uncertainties += 1;
                }
            }
        }

        if let Some(roles) = s.get("parameterRoles").and_then(|r| r.as_array()) {
            for role in roles {
                if role_has_cross_call_exit_effect(role) {
                    m.parameter_roles_with_cross_call_exit_effect += 1;
                }
            }
        }
    }
    m
}

/// Read the al-sem manifest's corpus-wide `matrix` block as an `R3a2Matrix` (the
/// ground-truth oracle the Rust matrix is cross-checked against).
fn manifest_matrix() -> R3a2Matrix {
    let path = goldens_dir().join("manifest.json");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("read R3a-2 manifest {}: {e}", path.display()));
    let json: Value = serde_json::from_str(&text)
        .unwrap_or_else(|e| panic!("R3a-2 manifest {} not valid JSON: {e}", path.display()));
    let mat = json
        .get("matrix")
        .unwrap_or_else(|| panic!("R3a-2 manifest carries no `matrix` block"));
    let mut m = R3a2Matrix::default();
    let u = |k: &str| mat.get(k).and_then(|v| v.as_u64()).unwrap_or(0) as usize;
    m.routine_count = u("routineCount");
    m.routines_with_inherited_effects = u("routinesWithInheritedEffects");
    if let Some(vk) = mat.get("viaKinds").and_then(|v| v.as_object()) {
        for (k, v) in vk {
            m.via_kinds
                .insert(k.clone(), v.as_u64().unwrap_or(0) as usize);
        }
    }
    m.recursive_cycle_routines = u("recursiveCycleRoutines");
    m.unresolved_call_routines = u("unresolvedCallRoutines");
    m.opaque_callee_uncertainties = u("opaqueCalleeUncertainties");
    if let Some(uk) = mat.get("uncertaintyKinds").and_then(|v| v.as_object()) {
        for (k, v) in uk {
            m.uncertainty_kinds
                .insert(k.clone(), v.as_u64().unwrap_or(0) as usize);
        }
    }
    m.parameter_roles_with_cross_call_exit_effect = u("parameterRolesWithCrossCallExitEffect");
    m
}

/// Recursively collect every forbidden object-key in `value`, with its JSON pointer path.
fn scan_forbidden(value: &Value, path: &str, hits: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            for (k, v) in map {
                let child = format!("{path}.{k}");
                if R3A2_FORBIDDEN_KEYS.contains(&k.as_str()) {
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
/// canonically sorted by the projection — summaries by stable routineId, dbEffects by
/// effectKey→operationId, uncertainties by uncertaintyKey, parameterRoles by
/// parameterIndex).
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

/// The R3a-2 summary-core differential pass + anti-degenerate matrix. The corpus is
/// the full SOURCE-ONLY `ws-*` set (158 fixtures); both sides run the SAME source
/// over the byte-identical workspace fixtures.
#[test]
fn differential_r3a2_summary_core_match_goldens() {
    let goldens = discover_goldens();
    assert!(
        !goldens.is_empty(),
        "no R3a-2 goldens discovered under {} — corpus missing?",
        goldens_dir().display()
    );

    let mut all_divergences: Vec<Divergence> = Vec::new();
    let mut forbidden_hits: Vec<String> = Vec::new();
    let mut rust_mat = R3a2Matrix::default();
    let mut golden_mat = R3a2Matrix::default();

    for (fixture, golden_path) in &goldens {
        let fixture_dir = corpus_dir().join(fixture);
        assert!(
            fixture_dir.is_dir(),
            "R3a-2 golden {} has no matching in-repo fixture at {} (offline corpus incomplete)",
            golden_path.display(),
            fixture_dir.display()
        );

        // Golden side: parse as JSON AND validate it parses as the R3a2Projection
        // serde type (shape guard — structurally omits every later-gate field).
        let golden_text = std::fs::read_to_string(golden_path)
            .unwrap_or_else(|e| panic!("read R3a-2 golden {}: {e}", golden_path.display()));
        let golden_json: Value = serde_json::from_str(&golden_text).unwrap_or_else(|e| {
            panic!(
                "R3a-2 golden {} is not valid JSON: {e}",
                golden_path.display()
            )
        });
        let _: R3a2Projection = serde_json::from_value(golden_json.clone()).unwrap_or_else(|e| {
            panic!(
                "R3a-2 golden {} does not parse as R3a2Projection: {e}",
                golden_path.display()
            )
        });

        // Rust side: source-only assemble+resolve → buildCombinedGraph → tarjanScc →
        // computeSummaries → projectR3a2 → JSON. Fail-closed layouts yield an empty
        // projection (never throws); the al-sem dump EXCLUDED those, so the golden set
        // never carries one.
        let projection = match assemble_and_resolve_workspace_with_program_calls(&fixture_dir) {
            Some(resolved) => project_r3a2(&resolved),
            None => R3a2Projection { summaries: vec![] },
        };
        let rust_json = serde_json::to_value(&projection)
            .unwrap_or_else(|e| panic!("serialize Rust R3a-2 projection for {fixture}: {e}"));

        // REGEN path (temp-state epoch rebaseline, Task 16). When
        // `REGEN_TEMP_GOLDENS` is set, write the ENGINE projection to the golden
        // file (matching the on-disk pretty form) instead of comparing — the
        // goldens are Rust-owned baselines (TS oracle retired).
        if regen::regen_mode() {
            let mut pretty = serde_json::to_string_pretty(&projection)
                .unwrap_or_else(|e| panic!("regen serialize R3a-2 {fixture}: {e}"));
            pretty.push('\n');
            std::fs::write(golden_path, pretty)
                .unwrap_or_else(|e| panic!("regen write {}: {e}", golden_path.display()));
            eprintln!("REGEN r3a2 golden: {}", golden_path.display());
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
        // cross-check).
        rust_mat.add(&matrix_of(&rust_json));
        golden_mat.add(&matrix_of(&golden_json));

        // Positional structural diff (both sides already canonically sorted).
        diff_value(fixture, "", &golden_json, &rust_json, &mut all_divergences);
    }

    // REGEN mode wrote every golden above and asserts nothing.
    if regen::regen_mode() {
        eprintln!("REGEN r3a2: wrote {} golden(s)", goldens.len());
        return;
    }

    all_divergences
        .sort_by(|a, b| (a.fixture.as_str(), &a.path).cmp(&(b.fixture.as_str(), &b.path)));

    // --- Forbidden-field guard (hard fail, never allowlistable) -------------
    assert!(
        forbidden_hits.is_empty(),
        "FORBIDDEN later-gate field(s) leaked into the R3a-2 comparison \
         (golden or rust):\n  {}",
        forbidden_hits.join("\n  ")
    );

    // --- ANTI-DEGENERATE matrix gate (fail-on-zero) -------------------------
    eprintln!(
        "R3a-2 matrix ({} fixture(s)): routines={} inheritedEffects={} viaKinds={:?} \
         recursiveCycle={} unresolvedCall={} opaqueCallee={} uncertaintyKinds={:?} \
         crossCallExitEffect={}",
        goldens.len(),
        rust_mat.routine_count,
        rust_mat.routines_with_inherited_effects,
        rust_mat.via_kinds,
        rust_mat.recursive_cycle_routines,
        rust_mat.unresolved_call_routines,
        rust_mat.opaque_callee_uncertainties,
        rust_mat.uncertainty_kinds,
        rust_mat.parameter_roles_with_cross_call_exit_effect,
    );
    let mut degenerate: Vec<String> = Vec::new();
    if rust_mat.routines_with_inherited_effects == 0 {
        degenerate.push(
            "routinesWithInheritedEffects=0 — need ≥1 routine with inherited dbEffects \
             (via!=direct; the composition fired)"
                .to_string(),
        );
    }
    // The 4 REACHABLE via-kinds must be present; `dynamic` must be ABSENT (source-only
    // corpus has no resolved dynamic-dispatch effect edge).
    for via in [
        "direct",
        "implicit-trigger",
        "event-subscriber",
        "inherited",
    ] {
        if rust_mat.via_kinds.get(via).copied().unwrap_or(0) == 0 {
            degenerate.push(format!(
                "viaKind '{via}' absent (need ≥1 — the via-precedence ladder must exercise it)"
            ));
        }
    }
    if rust_mat.via_kinds.get("dynamic").copied().unwrap_or(0) != 0 {
        degenerate.push(format!(
            "viaKind 'dynamic' PRESENT (={}) — must be absent in the source-only corpus",
            rust_mat.via_kinds.get("dynamic").copied().unwrap_or(0)
        ));
    }
    if rust_mat.opaque_callee_uncertainties == 0 {
        degenerate
            .push("opaqueCalleeUncertainties=0 (need ≥1 opaque-callee uncertainty)".to_string());
    }
    if rust_mat.parameter_roles_with_cross_call_exit_effect == 0 {
        degenerate.push(
            "parameterRolesWithCrossCallExitEffect=0 (need ≥1 cross-call exit effect)".to_string(),
        );
    }
    if rust_mat.recursive_cycle_routines == 0 {
        degenerate.push("recursiveCycleRoutines=0 (need ≥1 recursive-SCC routine)".to_string());
    }
    assert!(
        degenerate.is_empty(),
        "DEGENERATE R3a-2 matrix — the fixed-point summary core is NOT exercising the \
         full surface (an empty/trivial port would pass a pure equality diff):\n  {}",
        degenerate.join("\n  "),
    );

    // Oracle cross-check #1: the Rust corpus matrix == the GOLDEN corpus matrix
    // (recomputed from the goldens — independent of the manifest).
    assert_eq!(
        rust_mat, golden_mat,
        "R3a-2 matrix MISMATCH: Rust corpus matrix != recomputed-from-goldens matrix\n  \
         rust   = {rust_mat:?}\n  golden = {golden_mat:?}",
    );
    // Oracle cross-check #2: the Rust corpus matrix == the al-sem `manifest.json`
    // matrix block (al-sem ground truth, captured at dump time).
    let manifest_mat = manifest_matrix();
    assert_eq!(
        rust_mat, manifest_mat,
        "R3a-2 matrix MISMATCH vs al-sem manifest.json oracle\n  rust     = {rust_mat:?}\n  \
         manifest = {manifest_mat:?}",
    );

    // --- Direct strict divergence gate (no allowlist) -----------------------
    let mut failure = String::new();
    if !all_divergences.is_empty() {
        failure.push_str(&format!(
            "\n{} R3a-2 divergence(s) found ({R3A2_TEST_NAME}):\n",
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
        "R3a-2 summary-core differential FAILED:{failure}"
    );

    eprintln!(
        "R3a-2 differential: {} fixture(s), 0 divergences.",
        goldens.len(),
    );
}

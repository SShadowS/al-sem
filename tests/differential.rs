//! R0 differential harness — the SAFETY NET for the al-sem → Rust engine
//! migration.
//!
//! For each committed golden identity file under `tests/r0-goldens/`, this runs
//! the Rust `snapshot_workspace()` on the matching in-repo source fixture under
//! `tests/r0-corpus/` and asserts the **identity subset matches** field-for-field.
//! The default `cargo test` runs entirely OFFLINE: no Bun, no al-sem checkout.
//! Everything it needs is committed in-repo. Goldens are Rust-owned baselines
//! (the al-sem TS oracle is retired); rebaseline with
//! `REGEN_TEMP_GOLDENS=1 cargo test --test differential`.
//!
//! SCOPE: the in-repo corpus is the FULL source-only `ws-*` set al-sem dumps
//! (157 fixtures as of R0 Task 7, including the `ws-r0-canon-stress` identity
//! stress fixture). The comparison logic is all real; the harness iterates
//! every `tests/r0-goldens/*.golden.json` and asserts an exact match — zero
//! tolerated divergences.
//!
//! ## Comparison rules
//!
//! - Objects are matched by `(stableObjectId, name)`, routines by
//!   `stableRoutineId`. `stableObjectId` alone is not unique: `Interface`/
//!   `ControlAddIn` objects carry no grammar-level object number, so every
//!   such object in an app synthesizes the SAME `stableObjectId`
//!   (`objectNumber = 0`) — `name` disambiguates (AL forbids two same-kind
//!   objects sharing both an id and a name).
//! - Every field is compared for equality: objects compare `name`, `kind`,
//!   `signatureFingerprint`; routines compare those plus `normalizedSignatureHash`
//!   and `canonicalSignatureText`.
//! - The differ MAY sort both sides (it does, by id) but MUST NOT transform any
//!   value — no lowercasing/trimming/normalizing. That belongs in the engines.
//! - A missing object/routine on either side, an extra one, or any unequal field
//!   is a divergence.
//!
//! ## Divergence record + `path` locator format
//!
//! Each divergence is `{ fixture, path, golden_value, rust_value }`. The `path`
//! is a stable, machine-checkable locator:
//!   - field mismatch: `objects["<stableObjectId>"].signatureFingerprint`
//!     or `routines["<stableRoutineId>"].canonicalSignatureText`
//!   - present in golden, absent in rust: `objects["<id>"]:MISSING_IN_RUST`
//!     / `routines["<id>"]:MISSING_IN_RUST`
//!   - present in rust, absent in golden: `objects["<id>"]:EXTRA_IN_RUST`
//!     / `routines["<id>"]:EXTRA_IN_RUST`
//!
//! ## Strict comparison
//!
//! No tolerance layer: the test FAILS on ANY divergence between golden and
//! Rust output. The full corpus is asserted to match exactly.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use al_sem::engine::snapshot::{
    IdentitySnapshot, ObjectIdentity, RoutineIdentity, snapshot_workspace,
};
use al_sem::program::body::features::L2Projection;
use al_sem::program::body::l2_workspace::project_workspace;
use al_sem::program::model::coverage::AnalysisCoverage;
use al_sem::program::model::events::EventGraphProjection;
use al_sem::program::model::program_calls::assemble_and_resolve_workspace_with_program_calls;
use al_sem::program::model::workspace::RecordTypeProjection;

#[path = "common/regen.rs"]
mod regen;

/// A single, machine-checkable divergence between a golden and the Rust output.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Divergence {
    fixture: String,
    /// Stable locator, e.g. `routines["<id>"].canonicalSignatureText`.
    path: String,
    golden_value: String,
    rust_value: String,
}

/// Repo root = the crate manifest dir (the worktree root). `tests/` lives
/// directly under it.
fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn goldens_dir() -> PathBuf {
    repo_root().join("tests").join("r0-goldens")
}

fn corpus_dir() -> PathBuf {
    repo_root().join("tests").join("r0-corpus")
}

/// The R1a L2-feature goldens live alongside the R0 goldens but in their own
/// dir; the corpus (source fixtures) is shared with R0 (`tests/r0-corpus/`).
fn r1a_goldens_dir() -> PathBuf {
    repo_root().join("tests").join("r1a-goldens")
}

/// Discover every `tests/r0-goldens/*.golden.json` (skipping `manifest.json`),
/// returning `(fixture_name, golden_path)` sorted by fixture name.
fn discover_goldens() -> Vec<(String, PathBuf)> {
    let dir = goldens_dir();
    let mut out = Vec::new();
    let entries = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("failed to read goldens dir {}: {e}", dir.display()));
    for entry in entries {
        let entry = entry.expect("dir entry");
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.ends_with(".golden.json") {
            continue; // skips manifest.json, README.md, etc.
        }
        let fixture = name.trim_end_matches(".golden.json").to_string();
        out.push((fixture, path));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// Parse a golden file into the SAME `IdentitySnapshot` structs the engine
/// produces.
/// Reads a `manifest.json`'s top-level `fixtureCount` field and asserts the
/// currently-discovered golden count is AT LEAST that many (Task T0.6 — these
/// `manifest.json` files were previously read by no test, so a silently
/// deleted golden would pass unnoticed). `fixtureCount` is a frozen al-sem-era
/// PROVENANCE floor, not a live inventory: the Rust engine's own corpus for
/// some families has grown past it since al-sem retirement, so this checks
/// `>=`, not `==` — an exact-equality check would break the moment a family
/// legitimately gains a fixture.
fn assert_meets_manifest_fixture_count_floor(manifest_dir: &Path, discovered: usize) {
    let manifest_path = manifest_dir.join("manifest.json");
    let manifest: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(&manifest_path)
            .unwrap_or_else(|e| panic!("read {}: {e}", manifest_path.display())),
    )
    .unwrap_or_else(|e| panic!("{} not valid JSON: {e}", manifest_path.display()));
    let claimed = manifest
        .get("fixtureCount")
        .and_then(|v| v.as_u64())
        .unwrap_or_else(|| panic!("{} missing fixtureCount", manifest_path.display()))
        as usize;
    assert!(
        discovered >= claimed,
        "{} claims fixtureCount={claimed} but only {discovered} golden(s) were discovered \
         under {} — a golden may have been silently deleted",
        manifest_path.display(),
        manifest_dir.display()
    );
}

fn parse_golden(path: &Path) -> IdentitySnapshot {
    let text = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("failed to read golden {}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| {
        panic!(
            "failed to parse golden {} as IdentitySnapshot: {e}",
            path.display()
        )
    })
}

/// Compare one fixture's golden vs Rust snapshot, producing every divergence.
/// Pure structural comparison — NO value transforms.
fn diff_snapshots(
    fixture: &str,
    golden: &IdentitySnapshot,
    rust: &IdentitySnapshot,
) -> Vec<Divergence> {
    let mut out = Vec::new();

    // --- Objects, keyed by (stableObjectId, name). ---
    //
    // `stableObjectId` alone is NOT a safe join key: `Interface`/`ControlAddIn`
    // objects carry no grammar-level object number (AL never assigns one), so
    // `engine::snapshot::extract_from_ir` synthesizes `objectNumber = 0` for
    // EVERY interface/control-add-in in an app — any two such objects in the
    // same app collide on the exact same `stableObjectId`
    // (`"{appGuid}:Interface:0"`). Collecting straight into a
    // `BTreeMap<&str, &ObjectIdentity>` keyed by that non-unique id used to
    // silently drop every colliding entry but the last (`BTreeMap::insert`
    // overwrites on a repeated key), so `ws-interface-dispatch`'s two
    // interfaces (`IEmpty`, `IProcessor`) were never both compared — only
    // whichever sorted last (`IProcessor`) was ever checked, letting the
    // committed golden's `IEmpty` row sit silently wrong (it held a stale
    // copy of `IProcessor`'s `signatureFingerprint`) while `cargo test --test
    // differential` reported green. Regenerating exposed this: the freshly
    // computed snapshot is fully deterministic (verified across repeated
    // fresh-process regens) and simply differs from the stale golden — this
    // was never engine nondeterminism, only a masked comparison. Keying by
    // `(stableObjectId, name)` — AL forbids two same-kind objects sharing both
    // an id AND a name — disambiguates every collision case and lets each
    // object be compared independently.
    let golden_objs: BTreeMap<(&str, &str), &ObjectIdentity> = golden
        .objects
        .iter()
        .map(|o| ((o.stable_object_id.as_str(), o.name.as_str()), o))
        .collect();
    let rust_objs: BTreeMap<(&str, &str), &ObjectIdentity> = rust
        .objects
        .iter()
        .map(|o| ((o.stable_object_id.as_str(), o.name.as_str()), o))
        .collect();

    for ((id, name), g) in &golden_objs {
        match rust_objs.get(&(*id, *name)) {
            None => out.push(Divergence {
                fixture: fixture.to_string(),
                path: format!("objects[{:?}]:MISSING_IN_RUST", id),
                golden_value: format!("{g:?}"),
                rust_value: "<absent>".to_string(),
            }),
            Some(r) => {
                // (name is part of the map key, so a hit guarantees name equality —
                // no per-field name comparison needed.)
                push_field(&mut out, fixture, &obj_path(id, "kind"), &g.kind, &r.kind);
                push_field(
                    &mut out,
                    fixture,
                    &obj_path(id, "signatureFingerprint"),
                    &g.signature_fingerprint,
                    &r.signature_fingerprint,
                );
            }
        }
    }
    for ((id, name), r) in &rust_objs {
        if !golden_objs.contains_key(&(*id, *name)) {
            out.push(Divergence {
                fixture: fixture.to_string(),
                path: format!("objects[{:?}]:EXTRA_IN_RUST", id),
                golden_value: "<absent>".to_string(),
                rust_value: format!("{r:?}"),
            });
        }
    }

    // --- Routines, keyed by stableRoutineId. ---
    let golden_routines: BTreeMap<&str, &RoutineIdentity> = golden
        .routines
        .iter()
        .map(|r| (r.stable_routine_id.as_str(), r))
        .collect();
    let rust_routines: BTreeMap<&str, &RoutineIdentity> = rust
        .routines
        .iter()
        .map(|r| (r.stable_routine_id.as_str(), r))
        .collect();

    for (id, g) in &golden_routines {
        match rust_routines.get(id) {
            None => out.push(Divergence {
                fixture: fixture.to_string(),
                path: format!("routines[{:?}]:MISSING_IN_RUST", id),
                golden_value: format!("{g:?}"),
                rust_value: "<absent>".to_string(),
            }),
            Some(r) => {
                push_field(&mut out, fixture, &rt_path(id, "name"), &g.name, &r.name);
                push_field(&mut out, fixture, &rt_path(id, "kind"), &g.kind, &r.kind);
                push_field(
                    &mut out,
                    fixture,
                    &rt_path(id, "signatureFingerprint"),
                    &g.signature_fingerprint,
                    &r.signature_fingerprint,
                );
                push_field(
                    &mut out,
                    fixture,
                    &rt_path(id, "normalizedSignatureHash"),
                    &g.normalized_signature_hash,
                    &r.normalized_signature_hash,
                );
                push_field(
                    &mut out,
                    fixture,
                    &rt_path(id, "canonicalSignatureText"),
                    &g.canonical_signature_text,
                    &r.canonical_signature_text,
                );
            }
        }
    }
    for (id, r) in &rust_routines {
        if !golden_routines.contains_key(id) {
            out.push(Divergence {
                fixture: fixture.to_string(),
                path: format!("routines[{:?}]:EXTRA_IN_RUST", id),
                golden_value: "<absent>".to_string(),
                rust_value: format!("{r:?}"),
            });
        }
    }

    // Stable order for human-readable reporting.
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

fn obj_path(id: &str, field: &str) -> String {
    format!("objects[{id:?}].{field}")
}

fn rt_path(id: &str, field: &str) -> String {
    format!("routines[{id:?}].{field}")
}

/// Emit a field divergence iff golden != rust. No transforms — exact compare.
fn push_field(out: &mut Vec<Divergence>, fixture: &str, path: &str, golden: &str, rust: &str) {
    if golden != rust {
        out.push(Divergence {
            fixture: fixture.to_string(),
            path: path.to_string(),
            golden_value: golden.to_string(),
            rust_value: rust.to_string(),
        });
    }
}

/// The default, offline differential test. Runs the Rust snapshot on every
/// in-repo golden's matching fixture, diffs, and asserts an exact match.
#[test]
fn differential_identity_subset_matches_goldens() {
    let goldens = discover_goldens();
    assert!(
        !goldens.is_empty(),
        "no goldens discovered under {} — corpus missing?",
        goldens_dir().display()
    );

    // Collect every divergence across every fixture.
    let mut all_divergences: Vec<Divergence> = Vec::new();
    for (fixture, golden_path) in &goldens {
        let fixture_dir = corpus_dir().join(fixture);
        assert!(
            fixture_dir.is_dir(),
            "golden {} has no matching in-repo fixture at {} (offline corpus incomplete)",
            golden_path.display(),
            fixture_dir.display()
        );

        let rust = snapshot_workspace(&fixture_dir)
            .unwrap_or_else(|e| panic!("snapshot_workspace failed on {fixture}: {e:#}"));

        // REGEN path (Task T0.6 — R0 identity previously had none). When
        // `REGEN_TEMP_GOLDENS=1`, write the ENGINE-serialized snapshot straight to
        // the golden file instead of comparing — the goldens are Rust-owned
        // baselines (TS oracle retired). Same serializer + form as the assert path
        // reads (`parse_golden` deserializes the identical `IdentitySnapshot`).
        if regen::regen_mode() {
            let mut pretty = serde_json::to_string_pretty(&rust)
                .unwrap_or_else(|e| panic!("regen serialize R0 identity {fixture}: {e}"));
            pretty.push('\n');
            std::fs::write(golden_path, pretty)
                .unwrap_or_else(|e| panic!("regen write {}: {e}", golden_path.display()));
            continue;
        }

        let golden = parse_golden(golden_path);
        let mut divs = diff_snapshots(fixture, &golden, &rust);
        all_divergences.append(&mut divs);
    }

    // REGEN mode wrote every golden above and asserts nothing.
    if regen::regen_mode() {
        eprintln!("REGEN R0 identity: wrote {} golden(s)", goldens.len());
        return;
    }

    // --- Strict comparison ---------------------------------------------------
    let mut failure = String::new();

    if !all_divergences.is_empty() {
        failure.push_str(&format!(
            "\n{} divergence(s) found:\n",
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
        "R0 differential harness FAILED:{failure}\n\
         (matched {} fixture(s); the goldens carry canonicalSignatureText so a \
         signature drift is human-readable above.)",
        goldens.len()
    );

    eprintln!(
        "R0 differential: {} fixture(s), 0 divergences.",
        goldens.len()
    );

    assert_meets_manifest_fixture_count_floor(&goldens_dir(), goldens.len());
}

// =============================================================================
// R1a L2-features differential pass
// =============================================================================
//
// For each `tests/r1a-goldens/<name>.l2.golden.json`, run the Rust L2 dump
// (`project_workspace`) on `tests/r0-corpus/<name>` (the same source fixtures
// the R0 pass uses), and compare the allowlisted R1a projection as PARSED
// structures.
//
// ## Comparison rules (R1 spec §2 / R1a plan Task 4)
//
//   - Both sides are validated to parse as the `L2Projection` serde type (which
//     STRUCTURALLY OMITS every forbidden field), then compared as the raw parsed
//     `serde_json::Value` so the path locator is precise to the leaf.
//   - The differ MAY sort the top-level `objects`/`routines` lists (by
//     stableObjectId / stableRoutineId) — and ONLY those. It MUST NOT transform
//     any value. All enumerated arrays (operationSites / recordOperations /
//     callSites / loops / fieldAccesses / statementTree.children /
//     conditionLeaves / recordVariables / variables / …) are compared
//     POSITIONALLY, in order, because their order is semantically meaningful.
//   - R1b/R1c/R1d: `controlContext` + `order` + `scopeFrames` +
//     `capabilityFactsDirect`/`capabilityStatus`/`capabilityReasons`/
//     `capabilityDiagnostics` are now part of the projection — emitted by the Rust
//     side, present in the goldens, and compared structurally. Capability facts
//     are compared POSITIONALLY (extraction order — al-sem pushes them in
//     fixed family-dispatch order, never sorts); reasons are dedup+lexicographic-
//     sorted and diagnostics are (sourceRef,message)-sorted on BOTH sides before
//     comparison. They are NO LONGER forbidden. MUST FAIL if EITHER side carries a
//     STILL-forbidden L3 field anywhere (`resourceId` / `tableId` /
//     `calleeParameterIsVar` / `bindingResolution` / `sourceTableId`) — a recursive
//     key scan on both parsed values, belt-and-suspenders even though the serde
//     types omit them (e.g. a `tableId` leaking through a nested `table-field`
//     ValueSource is a hard fail).
//
// ## Divergence record + `path` locator
//
// Each divergence is `{ fixture, path, golden, rust }`. The `path` is a JSON
// pointer-ish locator into the (top-sorted) projection, e.g.
//   routines[12].features.callSites[0].argumentInfos[1].kind
//   objects[2].sourceTableName:MISSING_IN_RUST
//   …:FORBIDDEN_FIELD
// so the locality (raw text / CFN node path / ExpressionInfo / operationId)
// falls straight out of the pointer.
//
// ## Strict comparison
//
// No tolerance layer: any divergence fails the test. Target: zero divergences.
//
// ## Scope gate (Task 4 vs Task 5)
//
// `R1A_L2_SET` selects which fixtures this test ASSERTS on:
//   - unset / "full" (the committed default, since R1a Task 5 / exit gate):
//     every `tests/r1a-goldens/*.l2.golden.json` — the full 152-fixture corpus.
//   - "small": ws-d2 + ws-r0-canon-stress only (the proven-green Task-4 subset),
//     kept for fast localized iteration during development.
// Either way the harness, forbidden-field scan, and gating are identical — only
// the fixture set differs. The committed default asserts FULL-corpus L2 parity.

/// Keys that must NEVER appear on either side of the L2 comparison (later-gate /
/// L3-resolved). Mirrors `al2dump_smoke::FORBIDDEN_KEYS` + the projection's
/// `r1a-l2-projection.ts` FORBIDDEN_KEYS.
const L2_FORBIDDEN_KEYS: &[&str] = &[
    // R1b: controlContext is now REQUIRED (compared, not forbidden) — removed.
    // R1c: order + scopeFrames are now EMITTED + compared (not forbidden) — removed.
    // R1d: capabilityFactsDirect / capabilityStatus / capabilityReasons /
    //   capabilityDiagnostics are now EMITTED + compared (not forbidden). The
    //   STILL-forbidden set is only the L3-resolved fields below (mirrors the
    //   refreshed manifest.json `forbiddenKeys`). The scan still HARD-FAILS if a
    //   `tableId` leaks through a nested `table-field` ValueSource.
    "resourceId",
    "tableId",
    "calleeParameterIsVar",
    "bindingResolution",
    "sourceTableId",
];

const L2_TEST_NAME: &str = "differential_l2_features_match_goldens";

/// Discover every `tests/r1a-goldens/*.l2.golden.json`, sorted by fixture name.
fn discover_l2_goldens() -> Vec<(String, PathBuf)> {
    let dir = r1a_goldens_dir();
    let mut out = Vec::new();
    let entries = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("failed to read L2 goldens dir {}: {e}", dir.display()));
    for entry in entries {
        let entry = entry.expect("dir entry");
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.ends_with(".l2.golden.json") {
            continue; // skips manifest.json, l2-vectors.json
        }
        let fixture = name.trim_end_matches(".l2.golden.json").to_string();
        out.push((fixture, entry.path()));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// Recursively collect every forbidden object-key in `value`, with its JSON
/// pointer path (so a leak is reported with locality, not just "present").
fn scan_forbidden_keys(value: &serde_json::Value, path: &str, hits: &mut Vec<String>) {
    match value {
        serde_json::Value::Object(map) => {
            for (k, v) in map {
                let child = format!("{path}.{k}");
                if L2_FORBIDDEN_KEYS.contains(&k.as_str()) {
                    hits.push(child.clone());
                }
                scan_forbidden_keys(v, &child, hits);
            }
        }
        serde_json::Value::Array(arr) => {
            for (i, v) in arr.iter().enumerate() {
                scan_forbidden_keys(v, &format!("{path}[{i}]"), hits);
            }
        }
        _ => {}
    }
}

/// Canonicalize a projection `Value` for comparison: sort ONLY the top-level
/// `objects` (by stableObjectId) and `routines` (by stableRoutineId) arrays. No
/// other transform. Returns a fresh value (does not mutate the input).
fn canonicalize_l2_top(value: &serde_json::Value) -> serde_json::Value {
    let mut v = value.clone();
    if let Some(obj) = v.as_object_mut() {
        if let Some(serde_json::Value::Array(arr)) = obj.get_mut("objects") {
            arr.sort_by(|a, b| {
                a.get("stableObjectId")
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .cmp(
                        b.get("stableObjectId")
                            .and_then(|x| x.as_str())
                            .unwrap_or(""),
                    )
            });
        }
        if let Some(serde_json::Value::Array(arr)) = obj.get_mut("routines") {
            arr.sort_by(|a, b| {
                a.get("stableRoutineId")
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .cmp(
                        b.get("stableRoutineId")
                            .and_then(|x| x.as_str())
                            .unwrap_or(""),
                    )
            });
        }
    }
    v
}

/// Recursively diff two canonicalized projection values POSITIONALLY, emitting a
/// `Divergence` per leaf mismatch / shape mismatch / missing-or-extra key/elem.
fn diff_l2_value(
    fixture: &str,
    path: &str,
    golden: &serde_json::Value,
    rust: &serde_json::Value,
    out: &mut Vec<Divergence>,
) {
    use serde_json::Value;
    match (golden, rust) {
        (Value::Object(g), Value::Object(r)) => {
            // Keys present in golden — compare or flag MISSING_IN_RUST.
            for (k, gv) in g {
                let child = format!("{path}.{k}");
                match r.get(k) {
                    Some(rv) => diff_l2_value(fixture, &child, gv, rv, out),
                    None => out.push(Divergence {
                        fixture: fixture.to_string(),
                        path: format!("{child}:MISSING_IN_RUST"),
                        golden_value: compact(gv),
                        rust_value: "<absent>".to_string(),
                    }),
                }
            }
            // Keys only in rust — EXTRA_IN_RUST.
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
            // Positional comparison up to the shorter length; surplus elems on
            // either side are reported as MISSING/EXTRA at their index.
            let n = g.len().min(r.len());
            for i in 0..n {
                diff_l2_value(fixture, &format!("{path}[{i}]"), &g[i], &r[i], out);
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

/// Compact single-line JSON for divergence reporting.
fn compact(v: &serde_json::Value) -> String {
    serde_json::to_string(v).unwrap_or_else(|_| format!("{v:?}"))
}

/// The L2-features differential pass. Gated by `R1A_L2_SET` (see module doc) —
/// committed default is the small proven-green set (ws-d2 + ws-r0-canon-stress).
#[test]
fn differential_l2_features_match_goldens() {
    let all_goldens = discover_l2_goldens();
    assert!(
        !all_goldens.is_empty(),
        "no L2 goldens discovered under {} — corpus missing?",
        r1a_goldens_dir().display()
    );
    let all_goldens_len = all_goldens.len();

    // Scope gate: which fixtures this test ASSERTS on. Committed default is the
    // FULL 152-fixture corpus (R1a exit gate); `small` is the dev subset.
    let set = std::env::var("R1A_L2_SET").unwrap_or_else(|_| "full".to_string());
    let small_set = ["ws-d2", "ws-r0-canon-stress"];
    let goldens: Vec<(String, PathBuf)> = match set.as_str() {
        "full" | "" => all_goldens,
        "small" => all_goldens
            .into_iter()
            .filter(|(f, _)| small_set.contains(&f.as_str()))
            .collect(),
        other => panic!("R1A_L2_SET={other:?} not recognized (expected `small` or `full`)"),
    };
    assert!(
        !goldens.is_empty(),
        "R1A_L2_SET={set:?} selected zero fixtures (small set = {small_set:?})"
    );

    let mut all_divergences: Vec<Divergence> = Vec::new();
    let mut forbidden_hits: Vec<String> = Vec::new();

    for (fixture, golden_path) in &goldens {
        let fixture_dir = corpus_dir().join(fixture);
        assert!(
            fixture_dir.is_dir(),
            "L2 golden {} has no matching in-repo fixture at {} (offline corpus incomplete)",
            golden_path.display(),
            fixture_dir.display()
        );

        // Golden side: parse as JSON (for the diff) AND validate it parses as the
        // allowlisted L2Projection serde type (shape guard).
        let golden_text = std::fs::read_to_string(golden_path)
            .unwrap_or_else(|e| panic!("read L2 golden {}: {e}", golden_path.display()));
        let golden_json: serde_json::Value =
            serde_json::from_str(&golden_text).unwrap_or_else(|e| {
                panic!("L2 golden {} is not valid JSON: {e}", golden_path.display())
            });
        let _: L2Projection = serde_json::from_value(golden_json.clone()).unwrap_or_else(|e| {
            panic!(
                "L2 golden {} does not parse as L2Projection: {e}",
                golden_path.display()
            )
        });

        // Rust side: project + serialize back to JSON for the structural diff.
        let projection = project_workspace(&fixture_dir)
            .unwrap_or_else(|e| panic!("project_workspace failed on {fixture}: {e:#}"));
        let rust_json = serde_json::to_value(&projection)
            .unwrap_or_else(|e| panic!("serialize Rust L2 projection for {fixture}: {e}"));

        // REGEN path (mirrors the L3rt branch / temp-state epoch rebaseline). When
        // `REGEN_TEMP_GOLDENS` is set, write the ENGINE projection to the golden
        // file (matching the on-disk pretty form) instead of comparing — the
        // goldens are Rust-owned baselines (TS oracle retired).
        if regen::regen_mode() {
            let mut pretty = serde_json::to_string_pretty(&projection)
                .unwrap_or_else(|e| panic!("regen serialize L2 {fixture}: {e}"));
            pretty.push('\n');
            std::fs::write(golden_path, pretty)
                .unwrap_or_else(|e| panic!("regen write {}: {e}", golden_path.display()));
            eprintln!("REGEN l2 golden: {}", golden_path.display());
            continue;
        }

        // Forbidden-field scan on BOTH sides (belt-and-suspenders).
        scan_forbidden_keys(
            &golden_json,
            &format!("{fixture}:golden"),
            &mut forbidden_hits,
        );
        scan_forbidden_keys(&rust_json, &format!("{fixture}:rust"), &mut forbidden_hits);

        // Canonicalize top-level lists only, then positional diff.
        let g = canonicalize_l2_top(&golden_json);
        let r = canonicalize_l2_top(&rust_json);
        diff_l2_value(fixture, "", &g, &r, &mut all_divergences);
    }

    // REGEN mode wrote every golden above and asserts nothing.
    if regen::regen_mode() {
        eprintln!("REGEN l2: wrote {} golden(s)", goldens.len());
        return;
    }

    all_divergences
        .sort_by(|a, b| (a.fixture.as_str(), &a.path).cmp(&(b.fixture.as_str(), &b.path)));

    // --- Forbidden-field guard (hard fail, never allowlistable) -------------
    assert!(
        forbidden_hits.is_empty(),
        "FORBIDDEN later-gate/L3 field(s) leaked into the L2 comparison \
         (golden or rust):\n  {}",
        forbidden_hits.join("\n  ")
    );

    // --- Strict comparison ---------------------------------------------------
    let mut failure = String::new();
    if !all_divergences.is_empty() {
        failure.push_str(&format!(
            "\n{} L2 divergence(s) found (test={L2_TEST_NAME}):\n",
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
        "R1a L2-features differential FAILED (set={set:?}):{failure}"
    );

    eprintln!(
        "R1a L2 differential: set={set:?}, {} fixture(s), 0 divergences.",
        goldens.len()
    );

    // Full-corpus floor check regardless of the dev-subset gate above (Task T0.6).
    assert_meets_manifest_fixture_count_floor(&r1a_goldens_dir(), all_goldens_len);
}

// =============================================================================
// R2a L3 record-types differential pass + COVERAGE MATRIX (anti-degenerate)
// =============================================================================
//
// For each `tests/r2a-goldens/<name>.l3rt.golden.json`, build the production
// model (`assemble_and_resolve_workspace_with_program_calls`; L3's disk builder
// until engine-switch S9.5e, which moved no r2a golden) on
// `tests/r0-corpus/<name>` (the SAME source fixtures the R0/R1a passes use),
// project to the golden-shaped record-type projection, and compare structurally:
// resolved record-var/op StableTableId (OMITTED when unresolved) + per-Table
// merged fields.
//
// ## Capture point (R2a)
//
// POST-RESOLVE / PRE-SUMMARY: only the record-type surface. Every emitted field
// is built key-by-key (serde projection types), so later-gate / L4 fields cannot
// leak through a spread. A belt-and-suspenders recursive scan HARD-FAILS if any
// of `callGraph` / `eventGraph` / `coverage` / `typedEdges` / `resourceId` /
// `bindingResolution` / `argumentBindings` / `summary` / `capabilityFactsDirect`
// appears on EITHER side (matches the manifest `forbiddenKeys`).
//
// ## Comparison rules
//
//   - Both sides validated to parse as the `RecordTypeProjection` serde type
//     (shape guard — structurally omits everything but the record-type surface),
//     then compared as raw `serde_json::Value` POSITIONALLY (the projection is
//     already canonically sorted: tables by stableTableId, routines by
//     stableRoutineId, fields by (fieldNumber,name), vars by (name,tableId), ops
//     by operationId — so NO further sort/transform is applied here).
//
// ## COVERAGE MATRIX (anti-degenerate, [REV2])
//
// Across the corpus, the pass computes + ENFORCES nonzero counts of:
//   1. resolved record-var tableIds
//   2. resolved record-op tableIds
//   3. implicit-Rec resolutions (recordVariableName lc ∈ {"rec","xrec"} AND tableId set)
//   4. merged extension fields (declaringObjectId contains ":TableExtension:")
// computed from the RUST output (so it proves the Rust resolution actually FIRES,
// not "unresolved == unresolved"). If ANY of the four is zero the test FAILS —
// a degenerate (all-unresolved) port would otherwise pass a pure equality diff.
// The matrix counts are printed; an oracle cross-check asserts they equal the
// counts computed from the GOLDENS (al-sem ground truth).
//
// ## Strict comparison + scope gate
//
// No tolerance layer: any divergence fails the test (target empty).
// `R2A_L3_SET` selects the asserted fixtures:
//   - "full" (committed default since R2a Task 4 / the EXIT GATE): every
//     `tests/r2a-goldens/*.l3rt.golden.json` (the 153-fixture corpus). The
//     committed `cargo test --test differential` asserts FULL-corpus L3
//     record-type parity + the coverage matrix by default.
//   - "small": ws-d2 + ws-r2a-record-types — the proven-green dev subset, kept
//     for fast localized iteration.

/// Keys that must NEVER appear on either side of the L3 record-type comparison —
/// later-gate / L4 surfaces. Mirrors the manifest `forbiddenKeys` + the
/// projection's `r2a-l3-projection.ts` FORBIDDEN_KEYS.
const L3_FORBIDDEN_KEYS: &[&str] = &[
    "callGraph",
    "eventGraph",
    "coverage",
    "typedEdges",
    "resourceId",
    "bindingResolution",
    "argumentBindings",
    "summary",
    "capabilityFactsDirect",
];

const L3_TEST_NAME: &str = "differential_l3_record_types_match_goldens";

/// The four anti-degenerate coverage counts.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct CoverageMatrix {
    resolved_record_var_table_ids: usize,
    resolved_record_op_table_ids: usize,
    implicit_rec_resolutions: usize,
    merged_extension_fields: usize,
}

impl CoverageMatrix {
    fn add(&mut self, other: &CoverageMatrix) {
        self.resolved_record_var_table_ids += other.resolved_record_var_table_ids;
        self.resolved_record_op_table_ids += other.resolved_record_op_table_ids;
        self.implicit_rec_resolutions += other.implicit_rec_resolutions;
        self.merged_extension_fields += other.merged_extension_fields;
    }
}

/// Compute the coverage matrix for ONE projection `Value` (golden OR rust). The
/// shapes are identical, so the same walker serves both.
fn coverage_of(proj: &serde_json::Value) -> CoverageMatrix {
    let mut m = CoverageMatrix::default();
    if let Some(tables) = proj.get("tables").and_then(|t| t.as_array()) {
        for t in tables {
            if let Some(fields) = t.get("fields").and_then(|f| f.as_array()) {
                for f in fields {
                    if f.get("declaringObjectId")
                        .and_then(|d| d.as_str())
                        .map(|d| d.contains(":TableExtension:"))
                        .unwrap_or(false)
                    {
                        m.merged_extension_fields += 1;
                    }
                }
            }
        }
    }
    if let Some(routines) = proj.get("routines").and_then(|r| r.as_array()) {
        for r in routines {
            if let Some(vars) = r.get("recordVariables").and_then(|v| v.as_array()) {
                for v in vars {
                    if v.get("tableId").is_some() {
                        m.resolved_record_var_table_ids += 1;
                    }
                }
            }
            if let Some(ops) = r.get("recordOperations").and_then(|o| o.as_array()) {
                for o in ops {
                    let has_table = o.get("tableId").is_some();
                    if has_table {
                        m.resolved_record_op_table_ids += 1;
                    }
                    let name = o
                        .get("recordVariableName")
                        .and_then(|n| n.as_str())
                        .unwrap_or("");
                    if (name == "rec" || name == "xrec") && has_table {
                        m.implicit_rec_resolutions += 1;
                    }
                }
            }
        }
    }
    m
}

/// Discover every `tests/r2a-goldens/*.l3rt.golden.json`, sorted by fixture name.
fn discover_l3_goldens() -> Vec<(String, PathBuf)> {
    let dir = repo_root().join("tests").join("r2a-goldens");
    let mut out = Vec::new();
    let entries = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("failed to read L3 goldens dir {}: {e}", dir.display()));
    for entry in entries {
        let entry = entry.expect("dir entry");
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.ends_with(".l3rt.golden.json") {
            continue; // skips manifest.json, l3rt-vectors.json
        }
        let fixture = name.trim_end_matches(".l3rt.golden.json").to_string();
        out.push((fixture, entry.path()));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// The L3 record-types differential pass + coverage matrix. Gated by
/// `R2A_L3_SET` (committed default `full`: the whole 153-fixture corpus — the
/// R2a exit gate; `small` = ws-d2 + ws-r2a-record-types for dev iteration).
#[test]
fn differential_l3_record_types_match_goldens() {
    let all_goldens = discover_l3_goldens();
    assert!(
        !all_goldens.is_empty(),
        "no L3 goldens discovered under tests/r2a-goldens — corpus missing?"
    );
    let all_goldens_len = all_goldens.len();

    // Scope gate. Committed default since R2a Task 4 (the EXIT GATE) is the FULL
    // 153-fixture corpus; `small` is the proven-green dev subset.
    let set = std::env::var("R2A_L3_SET").unwrap_or_else(|_| "full".to_string());
    let small_set = ["ws-d2", "ws-r2a-record-types"];
    let goldens: Vec<(String, PathBuf)> = match set.as_str() {
        "full" | "" => all_goldens,
        "small" => all_goldens
            .into_iter()
            .filter(|(f, _)| small_set.contains(&f.as_str()))
            .collect(),
        other => panic!("R2A_L3_SET={other:?} not recognized (expected `small` or `full`)"),
    };
    assert!(
        !goldens.is_empty(),
        "R2A_L3_SET={set:?} selected zero fixtures (small set = {small_set:?})"
    );

    let mut all_divergences: Vec<Divergence> = Vec::new();
    let mut forbidden_hits: Vec<String> = Vec::new();
    let mut rust_cov = CoverageMatrix::default();
    let mut golden_cov = CoverageMatrix::default();

    for (fixture, golden_path) in &goldens {
        let fixture_dir = corpus_dir().join(fixture);
        assert!(
            fixture_dir.is_dir(),
            "L3 golden {} has no matching in-repo fixture at {} (offline corpus incomplete)",
            golden_path.display(),
            fixture_dir.display()
        );

        // Golden side: parse as JSON (for the diff) AND validate it parses as the
        // allowlisted RecordTypeProjection serde type (shape guard).
        let golden_text = std::fs::read_to_string(golden_path)
            .unwrap_or_else(|e| panic!("read L3 golden {}: {e}", golden_path.display()));
        let golden_json: serde_json::Value =
            serde_json::from_str(&golden_text).unwrap_or_else(|e| {
                panic!("L3 golden {} is not valid JSON: {e}", golden_path.display())
            });
        let _: RecordTypeProjection =
            serde_json::from_value(golden_json.clone()).unwrap_or_else(|e| {
                panic!(
                    "L3 golden {} does not parse as RecordTypeProjection: {e}",
                    golden_path.display()
                )
            });

        // Rust side: the production model (engine-switch S9.5e) → project → JSON.
        // Fail-closed (empty) layouts yield an empty projection (never throws).
        let projection = match assemble_and_resolve_workspace_with_program_calls(&fixture_dir) {
            Some(resolved) => resolved.project(),
            None => RecordTypeProjection {
                tables: vec![],
                routines: vec![],
            },
        };
        let rust_json = serde_json::to_value(&projection)
            .unwrap_or_else(|e| panic!("serialize Rust L3 projection for {fixture}: {e}"));

        // REGEN path (temp-state epoch rebaseline, Task 16). When
        // `REGEN_TEMP_GOLDENS` is set, write the ENGINE projection to the golden
        // file (matching the on-disk pretty form) instead of comparing — the
        // goldens are Rust-owned baselines (TS oracle retired).
        if regen::regen_mode() {
            let mut pretty = serde_json::to_string_pretty(&projection)
                .unwrap_or_else(|e| panic!("regen serialize L3 {fixture}: {e}"));
            pretty.push('\n');
            std::fs::write(golden_path, pretty)
                .unwrap_or_else(|e| panic!("regen write {}: {e}", golden_path.display()));
            eprintln!("REGEN l3rt golden: {}", golden_path.display());
            continue;
        }

        // Forbidden later-gate / L4 field scan on BOTH sides.
        scan_l3_forbidden(
            &golden_json,
            &format!("{fixture}:golden"),
            &mut forbidden_hits,
        );
        scan_l3_forbidden(&rust_json, &format!("{fixture}:rust"), &mut forbidden_hits);

        // Coverage matrices (Rust drives the anti-degenerate gate; golden is the
        // oracle cross-check).
        rust_cov.add(&coverage_of(&rust_json));
        golden_cov.add(&coverage_of(&golden_json));

        // Positional structural diff (both sides already canonically sorted).
        diff_l2_value(fixture, "", &golden_json, &rust_json, &mut all_divergences);
    }

    // REGEN mode wrote every golden above and asserts nothing.
    if regen::regen_mode() {
        eprintln!("REGEN l3rt: wrote {} golden(s)", goldens.len());
        return;
    }

    all_divergences
        .sort_by(|a, b| (a.fixture.as_str(), &a.path).cmp(&(b.fixture.as_str(), &b.path)));

    // --- Forbidden-field guard (hard fail, never allowlistable) -------------
    assert!(
        forbidden_hits.is_empty(),
        "FORBIDDEN later-gate/L4 field(s) leaked into the L3 comparison \
         (golden or rust):\n  {}",
        forbidden_hits.join("\n  ")
    );

    // --- COVERAGE MATRIX gate (anti-degenerate, [REV2]) ---------------------
    eprintln!(
        "R2a L3 coverage matrix (set={set:?}, {} fixture(s)): \
         resolvedRecordVarTableIds={} resolvedRecordOpTableIds={} \
         implicitRecResolutions={} mergedExtensionFields={}",
        goldens.len(),
        rust_cov.resolved_record_var_table_ids,
        rust_cov.resolved_record_op_table_ids,
        rust_cov.implicit_rec_resolutions,
        rust_cov.merged_extension_fields,
    );
    let mut zero_axes: Vec<&str> = Vec::new();
    if rust_cov.resolved_record_var_table_ids == 0 {
        zero_axes.push("resolvedRecordVarTableIds");
    }
    if rust_cov.resolved_record_op_table_ids == 0 {
        zero_axes.push("resolvedRecordOpTableIds");
    }
    if rust_cov.implicit_rec_resolutions == 0 {
        zero_axes.push("implicitRecResolutions");
    }
    if rust_cov.merged_extension_fields == 0 {
        zero_axes.push("mergedExtensionFields");
    }
    assert!(
        zero_axes.is_empty(),
        "DEGENERATE coverage matrix (set={set:?}): axis/axes {zero_axes:?} are ZERO — \
         the L3 port is not actually RESOLVING (unresolved==unresolved would pass a \
         pure equality diff). The matrix must prove resolution fires.",
    );
    // Oracle cross-check: Rust coverage must equal the GOLDEN coverage (al-sem
    // ground truth). A mismatch means resolution diverged even if the structural
    // diff somehow missed it.
    assert_eq!(
        rust_cov, golden_cov,
        "L3 coverage matrix MISMATCH vs golden oracle (set={set:?})\n  rust   = {rust_cov:?}\n  golden = {golden_cov:?}",
    );

    // --- Strict comparison ---------------------------------------------------
    let mut failure = String::new();
    if !all_divergences.is_empty() {
        failure.push_str(&format!(
            "\n{} L3 divergence(s) found (test={L3_TEST_NAME}):\n",
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
        "R2a L3 record-types differential FAILED (set={set:?}):{failure}"
    );

    eprintln!(
        "R2a L3 differential: set={set:?}, {} fixture(s), 0 divergences.",
        goldens.len()
    );

    // Full-corpus floor check regardless of the dev-subset gate above (Task T0.6).
    assert_meets_manifest_fixture_count_floor(
        &repo_root().join("tests").join("r2a-goldens"),
        all_goldens_len,
    );
}

/// Recursively collect every forbidden object-key in `value` (L3 later-gate set),
/// with its JSON pointer path.
fn scan_l3_forbidden(value: &serde_json::Value, path: &str, hits: &mut Vec<String>) {
    match value {
        serde_json::Value::Object(map) => {
            for (k, v) in map {
                let child = format!("{path}.{k}");
                if L3_FORBIDDEN_KEYS.contains(&k.as_str()) {
                    hits.push(child.clone());
                }
                scan_l3_forbidden(v, &child, hits);
            }
        }
        serde_json::Value::Array(arr) => {
            for (i, v) in arr.iter().enumerate() {
                scan_l3_forbidden(v, &format!("{path}[{i}]"), hits);
            }
        }
        _ => {}
    }
}

// ===========================================================================
// R2c — L3 EVENT-GRAPH differential pass + the anti-degenerate coverage matrix.
//
// For each `tests/r2c-goldens/*.l3eg.golden.json`, run the Rust disk-backed
// assemble→resolve→build_event_graph→project_event_graph and compare. EventSymbols
// are keyed by their stable `id`; EventEdges by `(eventId, subscriberRoutineId)` —
// both already deterministically sorted by the projection, so the compare is
// positional/structural after keying. HARD-FAILS on any forbidden later-gate / L4
// field (callGraph / typedEdges / summary / coverage / publish / capability*).
// Asserted with zero tolerance.
//
// ## The 31-event-fixtures-vs-corpus inclusion rule
//
// al-sem's dump EXCLUDES event-less fixtures: it emitted goldens ONLY for the 31
// fixtures whose RESOLVED event graph is non-empty (>=1 publisher or subscriber),
// listing the other 132 under `manifest.exclusions` ("no event graph"). The Rust
// emitter produces an EMPTY `{events:[], edges:[]}` for every event-less fixture,
// so the inclusion rule is reproduced as: compare the 31 fixtures WITH a golden
// structurally, AND additionally enforce that EVERY corpus fixture WITHOUT a golden
// projects to an empty event graph (a non-empty event graph for a non-golden
// fixture would be an inclusion divergence — the Rust port inventing events al-sem
// did not). This guards both directions of the 31-vs-163 mismatch.
//
// ## COVERAGE MATRIX (anti-degenerate, plan Task 3 / Rev 2 §6)
//
// Across the 31 goldens the pass computes + ENFORCES nonzero counts of the 8 al-sem
// `CoverageCounts` axes (`scripts/dump-l3-event-graph.ts`):
//   integrationPublishers / businessPublishers / unknownKindSymbols (synthesized
//   maybe+unknown symbols) / isolatedPublishers / symbolsWithElementName /
//   resolvedEdges / maybeEdges / unknownEdges.
// Driven by the RUST projection (proves the port actually CLASSIFIES, not
// "empty==empty"); fail-on-zero per axis; an oracle cross-check asserts the totals
// equal BOTH the per-golden recomputation AND the al-sem manifest `coverageMatrix`.

/// Forbidden later-gate / L4 keys that must NEVER appear in the L3 event-graph
/// comparison surface (golden OR rust). The event graph is R2c's surface; the call
/// graph (R2b) is a SEPARATE pass, and summaries/coverage/publish/typedEdges are
/// later gates. Mirrors the manifest `forbiddenKeys`.
const L3EG_FORBIDDEN_KEYS: &[&str] = &[
    // call-graph surface (R2b — a separate pass)
    "callsiteId",
    "dispatchKind",
    "dispatchMeta",
    "argumentBindings",
    "groups",
    "bindings",
    "callsiteResolutions",
    // later-gate / L4
    "typedEdges",
    "summary",
    "coverage",
    "publish",
    "capabilityFactsDirect",
    "rootClassifications",
];

const L3EG_TEST_NAME: &str = "differential_l3_event_graph_match_goldens";

/// The 8 al-sem event-graph coverage axes (`CoverageCounts`). Driven by Rust;
/// oracle-cross-checked against the al-sem manifest's `coverageMatrix`.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct EventGraphCoverage {
    integration_publishers: usize,
    business_publishers: usize,
    /// Synthesized maybe/unknown symbols (eventKind neither integration nor business).
    unknown_kind_symbols: usize,
    isolated_publishers: usize,
    symbols_with_element_name: usize,
    resolved_edges: usize,
    maybe_edges: usize,
    unknown_edges: usize,
}

impl EventGraphCoverage {
    fn add(&mut self, o: &EventGraphCoverage) {
        self.integration_publishers += o.integration_publishers;
        self.business_publishers += o.business_publishers;
        self.unknown_kind_symbols += o.unknown_kind_symbols;
        self.isolated_publishers += o.isolated_publishers;
        self.symbols_with_element_name += o.symbols_with_element_name;
        self.resolved_edges += o.resolved_edges;
        self.maybe_edges += o.maybe_edges;
        self.unknown_edges += o.unknown_edges;
    }
}

/// Count the 8 coverage axes from ONE projection `Value` (golden OR rust — same
/// shape). Faithful port of al-sem `countCoverage` (`scripts/dump-l3-event-graph.ts`).
fn event_graph_coverage_of(proj: &serde_json::Value) -> EventGraphCoverage {
    let mut c = EventGraphCoverage::default();
    let str_of = |v: &serde_json::Value, k: &str| -> String {
        v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string()
    };
    if let Some(events) = proj.get("events").and_then(|e| e.as_array()) {
        for s in events {
            match str_of(s, "eventKind").as_str() {
                "integration" => c.integration_publishers += 1,
                "business" => c.business_publishers += 1,
                _ => c.unknown_kind_symbols += 1,
            }
            if s.get("isolated").and_then(|v| v.as_bool()) == Some(true) {
                c.isolated_publishers += 1;
            }
            if s.get("elementName").is_some() {
                c.symbols_with_element_name += 1;
            }
        }
    }
    if let Some(edges) = proj.get("edges").and_then(|e| e.as_array()) {
        for e in edges {
            match str_of(e, "resolution").as_str() {
                "resolved" => c.resolved_edges += 1,
                "maybe" => c.maybe_edges += 1,
                _ => c.unknown_edges += 1,
            }
        }
    }
    c
}

/// Discover every `tests/r2c-goldens/*.l3eg.golden.json`, sorted by fixture name.
fn discover_l3eg_goldens() -> Vec<(String, PathBuf)> {
    let dir = repo_root().join("tests").join("r2c-goldens");
    let mut out = Vec::new();
    let entries = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("failed to read L3eg goldens dir {}: {e}", dir.display()));
    for entry in entries {
        let entry = entry.expect("dir entry");
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.ends_with(".l3eg.golden.json") {
            continue; // skips manifest.json
        }
        let fixture = name.trim_end_matches(".l3eg.golden.json").to_string();
        out.push((fixture, entry.path()));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// The event-graph projection of the production model for a fixture dir: the
/// program engine's graph (engine-switch S9.5e — until then L3's own
/// `build_event_graph`). Fail-closed → empty.
fn rust_event_graph_projection(fixture_dir: &Path) -> EventGraphProjection {
    use al_sem::program::model::events::project_event_graph;
    match assemble_and_resolve_workspace_with_program_calls(fixture_dir) {
        Some(resolved) => project_event_graph(&resolved.events.graph),
        None => EventGraphProjection {
            events: vec![],
            edges: vec![],
        },
    }
}

/// Structural diff of two L3 event-graph projection `Value`s. EventSymbols keyed by
/// `id`, EventEdges keyed by `(eventId, subscriberRoutineId)`. Each side's arrays
/// are already deterministically sorted by the projection, so keying detects
/// MISSING / EXTRA cleanly and the per-key structural compare catches field drift.
fn diff_l3eg(
    fixture: &str,
    golden: &serde_json::Value,
    rust: &serde_json::Value,
    out: &mut Vec<Divergence>,
) {
    // --- events: keyed by stable id. ---
    let emap = |v: &serde_json::Value| -> BTreeMap<String, serde_json::Value> {
        let mut m = BTreeMap::new();
        if let Some(arr) = v.get("events").and_then(|e| e.as_array()) {
            for s in arr {
                let id = s
                    .get("id")
                    .and_then(|c| c.as_str())
                    .unwrap_or("")
                    .to_string();
                m.insert(id, s.clone());
            }
        }
        m
    };
    let ge = emap(golden);
    let re = emap(rust);
    for (id, g) in &ge {
        match re.get(id) {
            None => out.push(Divergence {
                fixture: fixture.to_string(),
                path: format!("events[{id:?}]:MISSING_IN_RUST"),
                golden_value: compact(g),
                rust_value: "<absent>".to_string(),
            }),
            Some(r) => diff_l2_value(fixture, &format!("events[{id:?}]"), g, r, out),
        }
    }
    for id in re.keys() {
        if !ge.contains_key(id) {
            out.push(Divergence {
                fixture: fixture.to_string(),
                path: format!("events[{id:?}]:EXTRA_IN_RUST"),
                golden_value: "<absent>".to_string(),
                rust_value: compact(&re[id]),
            });
        }
    }

    // --- edges: keyed by (eventId, subscriberRoutineId). ---
    let dmap = |v: &serde_json::Value| -> BTreeMap<String, serde_json::Value> {
        let mut m = BTreeMap::new();
        if let Some(arr) = v.get("edges").and_then(|e| e.as_array()) {
            for e in arr {
                let ev = e
                    .get("eventId")
                    .and_then(|c| c.as_str())
                    .unwrap_or("")
                    .to_string();
                let sub = e
                    .get("subscriberRoutineId")
                    .and_then(|c| c.as_str())
                    .unwrap_or("")
                    .to_string();
                m.insert(format!("{ev}\u{1f}{sub}"), e.clone());
            }
        }
        m
    };
    let gd = dmap(golden);
    let rd = dmap(rust);
    for (key, g) in &gd {
        match rd.get(key) {
            None => out.push(Divergence {
                fixture: fixture.to_string(),
                path: format!("edges[{key:?}]:MISSING_IN_RUST"),
                golden_value: compact(g),
                rust_value: "<absent>".to_string(),
            }),
            Some(r) => diff_l2_value(fixture, &format!("edges[{key:?}]"), g, r, out),
        }
    }
    for key in rd.keys() {
        if !gd.contains_key(key) {
            out.push(Divergence {
                fixture: fixture.to_string(),
                path: format!("edges[{key:?}]:EXTRA_IN_RUST"),
                golden_value: "<absent>".to_string(),
                rust_value: compact(&rd[key]),
            });
        }
    }
}

/// Recursively collect every forbidden later-gate object-key in `value` (L3eg set).
fn scan_l3eg_forbidden(value: &serde_json::Value, path: &str, hits: &mut Vec<String>) {
    match value {
        serde_json::Value::Object(map) => {
            for (k, v) in map {
                let child = format!("{path}.{k}");
                if L3EG_FORBIDDEN_KEYS.contains(&k.as_str()) {
                    hits.push(child.clone());
                }
                scan_l3eg_forbidden(v, &child, hits);
            }
        }
        serde_json::Value::Array(arr) => {
            for (i, v) in arr.iter().enumerate() {
                scan_l3eg_forbidden(v, &format!("{path}[{i}]"), hits);
            }
        }
        _ => {}
    }
}

/// The R2c L3 event-graph differential pass + the 8-axis coverage matrix. Gated by
/// `R2C_L3EG_SET` (committed default `full`: all 31 event-bearing goldens — the R2c
/// EXIT GATE; `small` = ws-d2 + ws-r2c-mixed for dev iteration). On the FULL set it
/// ALSO enforces the inclusion rule: every corpus fixture WITHOUT a golden must
/// project to an empty event graph.
#[test]
fn differential_l3_event_graph_match_goldens() {
    let all_goldens = discover_l3eg_goldens();
    assert!(
        !all_goldens.is_empty(),
        "no L3eg goldens discovered under tests/r2c-goldens — corpus missing?"
    );

    let set = std::env::var("R2C_L3EG_SET").unwrap_or_else(|_| "full".to_string());
    let small_set = ["ws-d2", "ws-r2c-mixed"];
    let goldens: Vec<(String, PathBuf)> = match set.as_str() {
        "full" | "" => all_goldens,
        "small" => all_goldens
            .into_iter()
            .filter(|(f, _)| small_set.contains(&f.as_str()))
            .collect(),
        other => panic!("R2C_L3EG_SET={other:?} not recognized (expected `small` or `full`)"),
    };
    assert!(
        !goldens.is_empty(),
        "R2C_L3EG_SET={set:?} selected zero fixtures (small set = {small_set:?})"
    );

    let mut all_divergences: Vec<Divergence> = Vec::new();
    let mut forbidden_hits: Vec<String> = Vec::new();
    let mut rust_cov = EventGraphCoverage::default();
    let mut golden_cov = EventGraphCoverage::default();

    for (fixture, golden_path) in &goldens {
        let fixture_dir = corpus_dir().join(fixture);
        assert!(
            fixture_dir.is_dir(),
            "L3eg golden {} has no matching in-repo fixture at {} (offline corpus incomplete)",
            golden_path.display(),
            fixture_dir.display()
        );

        // Golden side: parse as JSON (for the diff) AND validate it parses as the
        // allowlisted EventGraphProjection serde type (shape guard).
        let golden_text = std::fs::read_to_string(golden_path)
            .unwrap_or_else(|e| panic!("read L3eg golden {}: {e}", golden_path.display()));
        let golden_json: serde_json::Value =
            serde_json::from_str(&golden_text).unwrap_or_else(|e| {
                panic!(
                    "L3eg golden {} is not valid JSON: {e}",
                    golden_path.display()
                )
            });
        let _: EventGraphProjection =
            serde_json::from_value(golden_json.clone()).unwrap_or_else(|e| {
                panic!(
                    "L3eg golden {} does not parse as EventGraphProjection: {e}",
                    golden_path.display()
                )
            });

        // Rust side: disk-backed assemble+resolve → project_event_graph → JSON.
        let projection = rust_event_graph_projection(&fixture_dir);
        let rust_json = serde_json::to_value(&projection)
            .unwrap_or_else(|e| panic!("serialize Rust L3eg projection for {fixture}: {e}"));

        // REGEN path (Task T0.6 — L3eg previously had none; mirrors the l2 / l3rt /
        // l3cg / l3cov branches). When `REGEN_TEMP_GOLDENS=1`, write the ENGINE
        // projection to the golden file instead of comparing — Rust-owned
        // baselines (TS oracle retired).
        if regen::regen_mode() {
            let mut pretty = serde_json::to_string_pretty(&projection)
                .unwrap_or_else(|e| panic!("regen serialize L3eg {fixture}: {e}"));
            pretty.push('\n');
            std::fs::write(golden_path, pretty)
                .unwrap_or_else(|e| panic!("regen write {}: {e}", golden_path.display()));
            eprintln!("REGEN l3eg golden: {}", golden_path.display());
            continue;
        }

        // Forbidden later-gate / L4 field scan on BOTH sides (hard fail).
        scan_l3eg_forbidden(
            &golden_json,
            &format!("{fixture}:golden"),
            &mut forbidden_hits,
        );
        scan_l3eg_forbidden(&rust_json, &format!("{fixture}:rust"), &mut forbidden_hits);

        // Coverage (Rust drives the anti-degenerate gate; golden is the oracle).
        rust_cov.add(&event_graph_coverage_of(&rust_json));
        golden_cov.add(&event_graph_coverage_of(&golden_json));

        // Keyed structural compare (events by id, edges by (eventId, subscriber)).
        diff_l3eg(fixture, &golden_json, &rust_json, &mut all_divergences);
    }

    // REGEN mode wrote every golden above and asserts nothing. It ALSO mints
    // NEW goldens for any corpus fixture that has none yet but produces a
    // non-empty event graph — the corpus grows independently of this golden
    // family (new detector fixtures land under `tests/r0-corpus/` without a
    // matching `.l3eg.golden.json`), and the loop above only ever touches
    // fixtures `discover_l3eg_goldens()` already knows about, so it silently
    // no-ops for them otherwise. This mirrors the inclusion-rule guard below
    // (same corpus scan, same "non-empty projection" test) but MINTS in regen
    // mode instead of asserting — the assert-path semantics for an
    // ungoldened event-bearing fixture are untouched (still a hard failure
    // outside regen mode).
    if regen::regen_mode() {
        let mut minted = 0usize;
        if set == "full" || set.is_empty() {
            let golden_set: std::collections::HashSet<&str> =
                goldens.iter().map(|(f, _)| f.as_str()).collect();
            let entries = std::fs::read_dir(corpus_dir()).expect("read corpus dir");
            let mut corpus: Vec<String> = entries
                .filter_map(|e| e.ok())
                .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
                .map(|e| e.file_name().to_string_lossy().to_string())
                .collect();
            corpus.sort();
            for fixture in &corpus {
                if golden_set.contains(fixture.as_str()) {
                    continue;
                }
                let proj = rust_event_graph_projection(&corpus_dir().join(fixture));
                if proj.events.is_empty() && proj.edges.is_empty() {
                    continue; // correctly excluded (empty event graph) — no golden needed
                }
                let new_golden_path = repo_root()
                    .join("tests")
                    .join("r2c-goldens")
                    .join(format!("{fixture}.l3eg.golden.json"));
                let mut pretty = serde_json::to_string_pretty(&proj)
                    .unwrap_or_else(|e| panic!("regen serialize NEW l3eg golden {fixture}: {e}"));
                pretty.push('\n');
                std::fs::write(&new_golden_path, pretty).unwrap_or_else(|e| {
                    panic!("regen write NEW {}: {e}", new_golden_path.display())
                });
                eprintln!("REGEN NEW l3eg golden: {}", new_golden_path.display());
                minted += 1;
            }
        }
        eprintln!(
            "REGEN l3eg: wrote {} golden(s), minted {minted} NEW golden(s)",
            goldens.len()
        );
        return;
    }

    // --- Inclusion-rule guard (FULL set only): every corpus fixture WITHOUT a
    //     golden must project to an EMPTY event graph (al-sem excluded the 132
    //     event-less fixtures; a non-empty graph here would be an invented event). -
    if set == "full" || set.is_empty() {
        let golden_set: std::collections::HashSet<&str> =
            goldens.iter().map(|(f, _)| f.as_str()).collect();
        let entries = std::fs::read_dir(corpus_dir()).expect("read corpus dir");
        let mut corpus: Vec<String> = entries
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        corpus.sort();
        for fixture in &corpus {
            if golden_set.contains(fixture.as_str()) {
                continue;
            }
            let proj = rust_event_graph_projection(&corpus_dir().join(fixture));
            if !proj.events.is_empty() || !proj.edges.is_empty() {
                all_divergences.push(Divergence {
                    fixture: fixture.clone(),
                    path: "NON_GOLDEN_FIXTURE_PRODUCED_EVENTS".to_string(),
                    golden_value: "<empty event graph (excluded by al-sem)>".to_string(),
                    rust_value: format!("events={} edges={}", proj.events.len(), proj.edges.len()),
                });
            }
        }
    }

    all_divergences
        .sort_by(|a, b| (a.fixture.as_str(), &a.path).cmp(&(b.fixture.as_str(), &b.path)));

    // --- Forbidden-field guard (hard fail, never allowlistable) -------------
    assert!(
        forbidden_hits.is_empty(),
        "FORBIDDEN later-gate/L4 field(s) leaked into the L3 event-graph comparison \
         (golden or rust):\n  {}",
        forbidden_hits.join("\n  ")
    );

    // --- COVERAGE MATRIX gate (anti-degenerate, fail-on-zero) ---------------
    eprintln!(
        "R2c L3eg coverage matrix (set={set:?}, {} fixture(s)):\n  \
         integrationPublishers={} businessPublishers={} unknownKindSymbols={} \
         isolatedPublishers={} symbolsWithElementName={} resolvedEdges={} \
         maybeEdges={} unknownEdges={}",
        goldens.len(),
        rust_cov.integration_publishers,
        rust_cov.business_publishers,
        rust_cov.unknown_kind_symbols,
        rust_cov.isolated_publishers,
        rust_cov.symbols_with_element_name,
        rust_cov.resolved_edges,
        rust_cov.maybe_edges,
        rust_cov.unknown_edges,
    );
    // Fail-on-zero per axis ONLY for the full corpus (the small dev set cannot
    // populate every axis — e.g. business publishers / element names are rare).
    if set == "full" || set.is_empty() {
        let axes: [(&str, usize); 8] = [
            ("integrationPublishers", rust_cov.integration_publishers),
            ("businessPublishers", rust_cov.business_publishers),
            ("unknownKindSymbols", rust_cov.unknown_kind_symbols),
            ("isolatedPublishers", rust_cov.isolated_publishers),
            ("symbolsWithElementName", rust_cov.symbols_with_element_name),
            ("resolvedEdges", rust_cov.resolved_edges),
            ("maybeEdges", rust_cov.maybe_edges),
            ("unknownEdges", rust_cov.unknown_edges),
        ];
        let zero_axes: Vec<&str> = axes
            .iter()
            .filter(|(_, n)| *n == 0)
            .map(|(name, _)| *name)
            .collect();
        assert!(
            zero_axes.is_empty(),
            "DEGENERATE L3eg coverage matrix (set={set:?}): axis/axes {zero_axes:?} are ZERO — \
             the R2c port is not actually CLASSIFYING that case (empty==empty would pass a pure \
             equality diff). The matrix must prove the event-graph build fires.",
        );
    }

    // Oracle cross-check: Rust coverage MUST equal the golden coverage (al-sem
    // ground truth) for the SAME fixture set.
    assert_eq!(
        rust_cov, golden_cov,
        "L3eg coverage matrix MISMATCH vs golden oracle (set={set:?})\n  rust   = {rust_cov:?}\n  golden = {golden_cov:?}",
    );

    // --- Strict comparison ---------------------------------------------------
    let mut failure = String::new();
    if !all_divergences.is_empty() {
        failure.push_str(&format!(
            "\n{} L3eg divergence(s) found (test={L3EG_TEST_NAME}):\n",
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
        "R2c L3 event-graph differential FAILED (set={set:?}):{failure}"
    );

    eprintln!(
        "R2c L3eg differential: set={set:?}, {} fixture(s), 0 divergences.",
        goldens.len()
    );
}

/// Oracle cross-check: the Rust event-graph coverage matrix, summed over EXACTLY the
/// al-sem manifest's OWN recorded fixture population (its `fixtures[]` array — the 33
/// fixtures al-sem actually captured at `alSemGitSha`), must equal the manifest's
/// published `coverageMatrix` (the ground-truth totals). This is independent of the
/// per-fixture golden compare — it guards the matrix counters themselves against
/// drift.
///
/// Deliberately does NOT sum over `discover_l3eg_goldens()` (the live golden-dir
/// listing): `manifest.json` is a frozen al-sem-era artifact (al-sem is retired — see
/// CLAUDE.md's Testing Philosophy — and cannot analyze fixtures added after it), so a
/// golden minted LATER by this test's own regen arm for a corpus fixture al-sem never
/// saw (e.g. the BCQuality-wave's ws-d54/55/57/59) must not be silently folded into an
/// al-sem-provenance cross-check — that would either fail spuriously (the manifest's
/// totals don't know about it) or, worse, misrepresent the manifest as having grown
/// al-sem ground truth it never actually captured.
#[test]
fn l3eg_coverage_matrix_matches_manifest_oracle() {
    let manifest_path = repo_root()
        .join("tests")
        .join("r2c-goldens")
        .join("manifest.json");
    let manifest: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&manifest_path).expect("read r2c manifest"))
            .expect("parse r2c manifest");

    let manifest_fixtures: Vec<String> = manifest
        .get("fixtures")
        .and_then(|f| f.as_array())
        .expect("manifest has fixtures[]")
        .iter()
        .map(|f| {
            f.get("fixture")
                .and_then(|v| v.as_str())
                .expect("manifest fixture entry has a `fixture` name")
                .to_string()
        })
        .collect();
    assert!(
        !manifest_fixtures.is_empty(),
        "manifest fixtures[] is empty — corpus missing?"
    );

    let mut rust_cov = EventGraphCoverage::default();
    for fixture in &manifest_fixtures {
        let proj = rust_event_graph_projection(&corpus_dir().join(fixture));
        let rust_json = serde_json::to_value(&proj).expect("serialize");
        rust_cov.add(&event_graph_coverage_of(&rust_json));
    }

    let m = manifest
        .get("coverageMatrix")
        .expect("manifest has coverageMatrix");
    let mget =
        |k: &str| -> usize { m.get(k).and_then(|v| v.as_u64()).unwrap_or(u64::MAX) as usize };
    let manifest_cov = EventGraphCoverage {
        integration_publishers: mget("integrationPublishers"),
        business_publishers: mget("businessPublishers"),
        unknown_kind_symbols: mget("unknownKindSymbols"),
        isolated_publishers: mget("isolatedPublishers"),
        symbols_with_element_name: mget("symbolsWithElementName"),
        resolved_edges: mget("resolvedEdges"),
        maybe_edges: mget("maybeEdges"),
        unknown_edges: mget("unknownEdges"),
    };
    assert_eq!(
        rust_cov, manifest_cov,
        "R2c coverage matrix MISMATCH vs al-sem manifest oracle\n  rust     = {rust_cov:?}\n  manifest = {manifest_cov:?}",
    );
    eprintln!(
        "R2c coverage matrix oracle: Rust full-corpus totals == al-sem manifest coverageMatrix."
    );
}

// ===========================================================================
// R2d — L3 COVERAGE differential pass + the anti-degenerate coverage matrix.
//
// For each `tests/r2d-goldens/*.l3cov.golden.json`, run the Rust disk-backed
// assemble→resolve→project_coverage_disk and compare the projected
// AnalysisCoverage structurally. The projection is a single flat object; its
// multisets (unresolvedCallsites / dynamicDispatchSites) + the
// routinesParseIncomplete list are sorted by the projection, so a POSITIONAL
// array compare (after sort) detects any cardinality OR id divergence (duplicates
// are preserved on BOTH sides — never deduped). HARD-FAILS on any forbidden
// later-gate / L4 field (callGraph / eventGraph / typedEdges / summary /
// analysisGaps / …). Asserted with zero tolerance.
//
// Every fixture has a golden (coverage is non-empty for every source workspace),
// so there is NO inclusion rule (unlike R2c's event graph) — all 158 compare.
//
// ## COVERAGE MATRIX (anti-degenerate, plan Task 3 / Rev 2 §6)
//
// Across the 158 goldens the pass computes + ENFORCES the al-sem manifest's
// `coverageMatrix` axes, driven by the RUST projection (proves the port actually
// CLASSIFIES, not "0==0"):
//   - sourceUnitsTotal / sourceUnitsParsed (parsed == total source-only)
//   - routinesTotal / routinesBodyAvailable
//   - routinesParseIncomplete (NONZERO — the corpus has a parse-incomplete fixture)
//   - opaqueApps (ZERO source-only — asserted ==0, NOT fail-on-zero)
//   - unresolvedCallsites (NONZERO multiset cardinality)
//   - dynamicDispatchSites (NONZERO multiset cardinality)
// An oracle cross-check asserts the Rust totals equal BOTH the per-golden
// recomputation AND the al-sem manifest `coverageMatrix`.

/// Forbidden later-gate / L4 keys that must NEVER appear in the L3 coverage
/// comparison surface (golden OR rust). Coverage is R2d's surface; the call graph
/// (R2b), event graph (R2c), and summaries/typedEdges/analysisGaps are SEPARATE
/// gates. Mirrors the manifest `forbiddenKeys`.
const L3COV_FORBIDDEN_KEYS: &[&str] = &[
    // call-graph surface (R2b — a separate pass)
    "callGraph",
    "callsiteId",
    "dispatchKind",
    "dispatchMeta",
    "argumentBindings",
    "groups",
    "bindings",
    "callsiteResolutions",
    // event-graph surface (R2c — a separate pass)
    "eventGraph",
    "events",
    "edges",
    "eventKind",
    // later-gate / L4 / R2.5
    "typedEdges",
    "summary",
    "analysisGaps",
    "capabilityFactsDirect",
    "rootClassifications",
];

const L3COV_TEST_NAME: &str = "differential_l3_coverage_match_goldens";

/// The R2d coverage matrix axes (al-sem manifest `coverageMatrix`). Driven by Rust;
/// oracle-cross-checked against the al-sem manifest's `coverageMatrix`.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct CoverageMatrix2 {
    source_units_total: usize,
    source_units_parsed: usize,
    routines_total: usize,
    routines_body_available: usize,
    routines_parse_incomplete: usize,
    opaque_apps: usize,
    unresolved_callsites: usize,
    dynamic_dispatch_sites: usize,
}

impl CoverageMatrix2 {
    fn add(&mut self, o: &CoverageMatrix2) {
        self.source_units_total += o.source_units_total;
        self.source_units_parsed += o.source_units_parsed;
        self.routines_total += o.routines_total;
        self.routines_body_available += o.routines_body_available;
        self.routines_parse_incomplete += o.routines_parse_incomplete;
        self.opaque_apps += o.opaque_apps;
        self.unresolved_callsites += o.unresolved_callsites;
        self.dynamic_dispatch_sites += o.dynamic_dispatch_sites;
    }
}

/// Count the matrix axes from ONE coverage projection `Value` (golden OR rust —
/// same shape). The counts are array LENGTHS (multisets) / scalars, so a duplicate
/// in a multiset is counted once per occurrence (cardinality), matching al-sem.
fn coverage_matrix_of(proj: &serde_json::Value) -> CoverageMatrix2 {
    let num = |k: &str| -> usize { proj.get(k).and_then(|v| v.as_u64()).unwrap_or(0) as usize };
    let len = |k: &str| -> usize {
        proj.get(k)
            .and_then(|v| v.as_array())
            .map(|a| a.len())
            .unwrap_or(0)
    };
    CoverageMatrix2 {
        source_units_total: num("sourceUnitsTotal"),
        source_units_parsed: num("sourceUnitsParsed"),
        routines_total: num("routinesTotal"),
        routines_body_available: num("routinesBodyAvailable"),
        routines_parse_incomplete: len("routinesParseIncomplete"),
        opaque_apps: len("opaqueApps"),
        unresolved_callsites: len("unresolvedCallsites"),
        dynamic_dispatch_sites: len("dynamicDispatchSites"),
    }
}

/// Discover every `tests/r2d-goldens/*.l3cov.golden.json`, sorted by fixture name.
fn discover_l3cov_goldens() -> Vec<(String, PathBuf)> {
    let dir = repo_root().join("tests").join("r2d-goldens");
    let mut out = Vec::new();
    let entries = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("failed to read L3cov goldens dir {}: {e}", dir.display()));
    for entry in entries {
        let entry = entry.expect("dir entry");
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.ends_with(".l3cov.golden.json") {
            continue; // skips manifest.json + l3cov-vectors.json
        }
        let fixture = name.trim_end_matches(".l3cov.golden.json").to_string();
        out.push((fixture, entry.path()));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// Coverage projection of the production model for a fixture dir (engine-switch
/// S9.5e: the program engine's calls; fail-closed → all-empty).
fn rust_coverage_projection(fixture_dir: &Path) -> AnalysisCoverage {
    match assemble_and_resolve_workspace_with_program_calls(fixture_dir) {
        Some(resolved) => resolved.project_coverage_disk(fixture_dir),
        None => AnalysisCoverage {
            source_units_total: 0,
            source_units_parsed: 0,
            routines_total: 0,
            routines_body_available: 0,
            routines_parse_incomplete: vec![],
            opaque_apps: vec![],
            unresolved_callsites: vec![],
            dynamic_dispatch_sites: vec![],
        },
    }
}

/// Recursively collect every forbidden later-gate object-key in `value` (L3cov set).
fn scan_l3cov_forbidden(value: &serde_json::Value, path: &str, hits: &mut Vec<String>) {
    match value {
        serde_json::Value::Object(map) => {
            for (k, v) in map {
                let child = format!("{path}.{k}");
                if L3COV_FORBIDDEN_KEYS.contains(&k.as_str()) {
                    hits.push(child.clone());
                }
                scan_l3cov_forbidden(v, &child, hits);
            }
        }
        serde_json::Value::Array(arr) => {
            for (i, v) in arr.iter().enumerate() {
                scan_l3cov_forbidden(v, &format!("{path}[{i}]"), hits);
            }
        }
        _ => {}
    }
}

/// The R2d L3 coverage differential pass + the anti-degenerate coverage matrix.
/// Gated by `R2D_L3COV_SET` (committed default `full`: all 158 goldens — the R2d /
/// R2 source-only L3 EXIT GATE; `small` = ws-d2 + ws-unresolved +
/// ws-policy-api-dynamic-dispatch for dev iteration).
#[test]
fn differential_l3_coverage_match_goldens() {
    let all_goldens = discover_l3cov_goldens();
    assert!(
        !all_goldens.is_empty(),
        "no L3cov goldens discovered under tests/r2d-goldens — corpus missing?"
    );

    let set = std::env::var("R2D_L3COV_SET").unwrap_or_else(|_| "full".to_string());
    let small_set = ["ws-d2", "ws-unresolved", "ws-policy-api-dynamic-dispatch"];
    let goldens: Vec<(String, PathBuf)> = match set.as_str() {
        "full" | "" => all_goldens,
        "small" => all_goldens
            .into_iter()
            .filter(|(f, _)| small_set.contains(&f.as_str()))
            .collect(),
        other => panic!("R2D_L3COV_SET={other:?} not recognized (expected `small` or `full`)"),
    };
    assert!(
        !goldens.is_empty(),
        "R2D_L3COV_SET={set:?} selected zero fixtures (small set = {small_set:?})"
    );

    let mut all_divergences: Vec<Divergence> = Vec::new();
    let mut forbidden_hits: Vec<String> = Vec::new();
    let mut rust_cov = CoverageMatrix2::default();
    let mut golden_cov = CoverageMatrix2::default();

    for (fixture, golden_path) in &goldens {
        let fixture_dir = corpus_dir().join(fixture);
        assert!(
            fixture_dir.is_dir(),
            "L3cov golden {} has no matching in-repo fixture at {} (offline corpus incomplete)",
            golden_path.display(),
            fixture_dir.display()
        );

        // Golden side: parse as JSON (for the diff) AND validate it parses as the
        // allowlisted AnalysisCoverage serde type (shape guard).
        let golden_text = std::fs::read_to_string(golden_path)
            .unwrap_or_else(|e| panic!("read L3cov golden {}: {e}", golden_path.display()));
        let golden_json: serde_json::Value =
            serde_json::from_str(&golden_text).unwrap_or_else(|e| {
                panic!(
                    "L3cov golden {} is not valid JSON: {e}",
                    golden_path.display()
                )
            });
        let _: AnalysisCoverage = serde_json::from_value(golden_json.clone()).unwrap_or_else(|e| {
            panic!(
                "L3cov golden {} does not parse as AnalysisCoverage: {e}",
                golden_path.display()
            )
        });

        // Rust side: disk-backed assemble+resolve → project_coverage_disk → JSON.
        let projection = rust_coverage_projection(&fixture_dir);
        let rust_json = serde_json::to_value(&projection)
            .unwrap_or_else(|e| panic!("serialize Rust L3cov projection for {fixture}: {e}"));

        // REGEN path (mirrors the l2 / l3rt / l3cg branches). When `REGEN_TEMP_GOLDENS`
        // is set, write the ENGINE projection to the golden file instead of
        // comparing — Rust-owned baselines (TS oracle retired for reclassified axes).
        if regen::regen_mode() {
            let mut pretty = serde_json::to_string_pretty(&projection)
                .unwrap_or_else(|e| panic!("regen serialize L3cov {fixture}: {e}"));
            pretty.push('\n');
            std::fs::write(golden_path, pretty)
                .unwrap_or_else(|e| panic!("regen write {}: {e}", golden_path.display()));
            eprintln!("REGEN l3cov golden: {}", golden_path.display());
            continue;
        }

        // Forbidden later-gate / L4 field scan on BOTH sides (hard fail).
        scan_l3cov_forbidden(
            &golden_json,
            &format!("{fixture}:golden"),
            &mut forbidden_hits,
        );
        scan_l3cov_forbidden(&rust_json, &format!("{fixture}:rust"), &mut forbidden_hits);

        // Coverage (Rust drives the anti-degenerate gate; golden is the oracle).
        rust_cov.add(&coverage_matrix_of(&rust_json));
        golden_cov.add(&coverage_matrix_of(&golden_json));

        // Structural compare of the whole flat projection (multisets positional
        // after the projection's sort — preserves + checks duplicates).
        diff_l2_value(
            fixture,
            "coverage",
            &golden_json,
            &rust_json,
            &mut all_divergences,
        );
    }

    // REGEN mode wrote every golden above and asserts nothing.
    if regen::regen_mode() {
        eprintln!("REGEN l3cov: wrote {} golden(s)", goldens.len());
        return;
    }

    all_divergences
        .sort_by(|a, b| (a.fixture.as_str(), &a.path).cmp(&(b.fixture.as_str(), &b.path)));

    // --- Forbidden-field guard (hard fail, never allowlistable) -------------
    assert!(
        forbidden_hits.is_empty(),
        "FORBIDDEN later-gate/L4 field(s) leaked into the L3 coverage comparison \
         (golden or rust):\n  {}",
        forbidden_hits.join("\n  ")
    );

    // --- COVERAGE MATRIX gate (anti-degenerate) -----------------------------
    eprintln!(
        "R2d L3cov coverage matrix (set={set:?}, {} fixture(s)):\n  \
         sourceUnitsTotal={} sourceUnitsParsed={} routinesTotal={} routinesBodyAvailable={} \
         routinesParseIncomplete={} opaqueApps={} unresolvedCallsites={} dynamicDispatchSites={}",
        goldens.len(),
        rust_cov.source_units_total,
        rust_cov.source_units_parsed,
        rust_cov.routines_total,
        rust_cov.routines_body_available,
        rust_cov.routines_parse_incomplete,
        rust_cov.opaque_apps,
        rust_cov.unresolved_callsites,
        rust_cov.dynamic_dispatch_sites,
    );
    // Fail-on-zero per axis ONLY for the full corpus. opaqueApps is structurally
    // EMPTY source-only (asserted ==0, NOT fail-on-zero); sourceUnitsParsed must
    // equal sourceUnitsTotal (the decrement is corpus-inert — covered by a vector).
    if set == "full" || set.is_empty() {
        let nonzero_axes: [(&str, usize); 6] = [
            ("sourceUnitsTotal", rust_cov.source_units_total),
            ("routinesTotal", rust_cov.routines_total),
            ("routinesBodyAvailable", rust_cov.routines_body_available),
            (
                "routinesParseIncomplete",
                rust_cov.routines_parse_incomplete,
            ),
            ("unresolvedCallsites", rust_cov.unresolved_callsites),
            ("dynamicDispatchSites", rust_cov.dynamic_dispatch_sites),
        ];
        let zero_axes: Vec<&str> = nonzero_axes
            .iter()
            .filter(|(_, n)| *n == 0)
            .map(|(name, _)| *name)
            .collect();
        assert!(
            zero_axes.is_empty(),
            "DEGENERATE L3cov coverage matrix (set={set:?}): axis/axes {zero_axes:?} are ZERO — \
             the R2d port is not actually CLASSIFYING that case (empty==empty would pass a pure \
             equality diff). The matrix must prove coverage accounting fires.",
        );
        assert_eq!(
            rust_cov.opaque_apps, 0,
            "opaqueApps MUST be ZERO source-only (becomes non-empty only in R2.5)",
        );
        assert_eq!(
            rust_cov.source_units_parsed, rust_cov.source_units_total,
            "sourceUnitsParsed MUST equal sourceUnitsTotal source-only (decrement is corpus-inert)",
        );
    }

    // Oracle cross-check: Rust coverage MUST equal the golden coverage (al-sem
    // ground truth) for the SAME fixture set.
    assert_eq!(
        rust_cov, golden_cov,
        "L3cov coverage matrix MISMATCH vs golden oracle (set={set:?})\n  rust   = {rust_cov:?}\n  golden = {golden_cov:?}",
    );

    // --- Strict comparison ---------------------------------------------------
    let mut failure = String::new();
    if !all_divergences.is_empty() {
        failure.push_str(&format!(
            "\n{} L3cov divergence(s) found (test={L3COV_TEST_NAME}):\n",
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
        "R2d L3 coverage differential FAILED (set={set:?}):{failure}"
    );

    eprintln!(
        "R2d L3cov differential: set={set:?}, {} fixture(s), 0 divergences.",
        goldens.len()
    );
}

/// Oracle cross-check: the FULL-corpus Rust coverage matrix must equal the al-sem
/// manifest's published `coverageMatrix` (the ground-truth totals). Independent of
/// the per-fixture golden compare — guards the matrix counters against drift. The
/// manifest carries extra axes (sourceUnitsDecremented / unresolvedMaxDup /
/// dynamicMaxDup) that this oracle ALSO checks.
#[test]
fn l3cov_coverage_matrix_matches_manifest_oracle() {
    let goldens = discover_l3cov_goldens();
    assert!(!goldens.is_empty(), "no L3cov goldens — corpus missing?");

    let mut rust_cov = CoverageMatrix2::default();
    let mut unresolved_max_dup = 0usize;
    let mut dynamic_max_dup = 0usize;
    for (fixture, _) in &goldens {
        let proj = rust_coverage_projection(&corpus_dir().join(fixture));
        let rust_json = serde_json::to_value(&proj).expect("serialize");
        rust_cov.add(&coverage_matrix_of(&rust_json));
        unresolved_max_dup = unresolved_max_dup.max(max_dup(&proj.unresolved_callsites));
        dynamic_max_dup = dynamic_max_dup.max(max_dup(&proj.dynamic_dispatch_sites));
    }

    let manifest_path = repo_root()
        .join("tests")
        .join("r2d-goldens")
        .join("manifest.json");
    let manifest: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&manifest_path).expect("read r2d manifest"))
            .expect("parse r2d manifest");
    let m = manifest
        .get("coverageMatrix")
        .expect("manifest has coverageMatrix");
    let mget =
        |k: &str| -> usize { m.get(k).and_then(|v| v.as_u64()).unwrap_or(u64::MAX) as usize };
    let manifest_cov = CoverageMatrix2 {
        source_units_total: mget("sourceUnitsTotal"),
        source_units_parsed: mget("sourceUnitsParsed"),
        routines_total: mget("routinesTotal"),
        routines_body_available: mget("routinesBodyAvailable"),
        routines_parse_incomplete: mget("routinesParseIncomplete"),
        opaque_apps: mget("opaqueApps"),
        unresolved_callsites: mget("unresolvedCallsites"),
        dynamic_dispatch_sites: mget("dynamicDispatchSites"),
    };
    assert_eq!(
        rust_cov, manifest_cov,
        "R2d coverage matrix MISMATCH vs al-sem manifest oracle\n  rust     = {rust_cov:?}\n  manifest = {manifest_cov:?}",
    );
    // The manifest's max-dup axes: source-only the corpus has NO real duplicate
    // (interface multi-edges are `maybe`, excluded), so both are 1 (or 0 if an axis
    // is empty — but unresolved/dynamic are nonzero). al-sem reports max-dup as the
    // max occurrences of any single id; with no real dup that is 1.
    assert_eq!(
        unresolved_max_dup,
        mget("unresolvedMaxDup"),
        "unresolvedMaxDup mismatch (rust={unresolved_max_dup})",
    );
    assert_eq!(
        dynamic_max_dup,
        mget("dynamicMaxDup"),
        "dynamicMaxDup mismatch (rust={dynamic_max_dup})",
    );
    assert_eq!(
        mget("sourceUnitsDecremented"),
        rust_cov.source_units_total - rust_cov.source_units_parsed,
        "sourceUnitsDecremented mismatch (source-only == 0)",
    );
    eprintln!(
        "R2d coverage matrix oracle: Rust full-corpus totals == al-sem manifest coverageMatrix \
         (incl. max-dup + decremented axes)."
    );
}

/// Max occurrences of any single id in a multiset (the manifest `*MaxDup` axis).
fn max_dup(ids: &[String]) -> usize {
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for id in ids {
        *counts.entry(id.as_str()).or_insert(0) += 1;
    }
    counts.values().copied().max().unwrap_or(0)
}

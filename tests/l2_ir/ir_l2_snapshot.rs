//! IR-OWNED L2 feature snapshot — the forward regression gate for the owned-IR L2
//! walk, replacing the migration-era `ir_dual_run` legacy-vs-IR oracle (deleted at
//! the Phase 5 seal). For every routine in the in-repo r0-corpus it digests the
//! FULL `PFeatures` (loops / operation_sites / record_operations / call_sites /
//! field_accesses / record_variables / nesting / branching / unreachable /
//! identifier_references / variables / var_assignments / condition_references / the
//! `statement_tree` CFN) produced by `project_routine_features_ir`.
//!
//! SERDE-SKIP DRIFT: the digest is over the `Debug` representation, NOT serde JSON —
//! deliberately, so it covers the `#[serde(skip)]` (and `PartialEq`-excluded) fields
//! that a serialized golden CANNOT see: `PRecordOperation.in_until_condition` /
//! `run_trigger`, `PCFNNode.source_range` / `is_case_else`, `PVarAssignment.rhs_identifier`.
//! Four such load-bearing fields silently broke during the migration precisely
//! because the dual-run byte gate (serde + PartialEq) was blind to them; this gate
//! is not.
//!
//! This is the deepest L2 contract (the raw feature output the whole engine builds
//! on), captured as a Rust-OWNED baseline: when the engine intentionally improves,
//! REGEN (`REGEN_TEMP_GOLDENS=1`) and review the diff — it is NOT pinned to the old
//! tree-sitter walk, so it does not ossify.

use std::path::{Path, PathBuf};

use al_sem::program::body::ir_walk::project_routine_features_ir;

use crate::regen;

fn collect_al_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            collect_al_files(&p, out);
        } else if p.extension().and_then(|s| s.to_str()) == Some("al") {
            out.push(p);
        }
    }
}

fn fnv1a(s: &str) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x00000100000001b3);
    }
    h
}

/// Render one source file's per-routine L2 features as digest lines, plus the
/// full Debug rendering keyed by routine for the on-failure diff.
fn features_lines(rel: &str, source: &str) -> Vec<(String, String)> {
    let file = al_syntax::parse(source);
    let mut out = Vec::new();
    for (oi, obj) in file.objects.iter().enumerate() {
        for r in &obj.routines {
            // Stable synthetic id (the golden only needs determinism, not the
            // production stableRoutineId): object index + object/routine name.
            let rid = format!("{rel}::{oi}::{}", r.name);
            let feats = project_routine_features_ir(
                &file,
                oi,
                r,
                &rid,
                source,
                "snap",
                r.dataitem_source_table.as_deref(),
            );
            // Debug, NOT serde JSON — covers `#[serde(skip)]` / PartialEq-excluded
            // fields (see the module doc: the serde-skip drift gate).
            let repr = format!("{feats:?}");
            out.push((rid, repr));
        }
    }
    out
}

#[test]
fn ir_l2_features_snapshot_over_r0_corpus() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let corpus = root.join("tests").join("r0-corpus");
    let golden = root
        .join("tests")
        .join("ir-l2-goldens")
        .join("l2_features.snapshot");

    let mut files = Vec::new();
    collect_al_files(&corpus, &mut files);
    assert!(
        files.len() > 100,
        "expected the r0-corpus to have many .al files, found {}",
        files.len()
    );
    files.sort();

    // Per-routine digest lines for the committed golden; keep the Debug rendering
    // for on-failure diffs.
    let mut lines = String::new();
    let mut repr_by_rid: std::collections::BTreeMap<String, String> =
        std::collections::BTreeMap::new();
    for path in &files {
        let Ok(source) = std::fs::read_to_string(path) else {
            continue;
        };
        let rel = path
            .strip_prefix(&corpus)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");
        for (rid, repr) in features_lines(&rel, &source) {
            lines.push_str(&format!("{rid}\t{:016x}\n", fnv1a(&repr)));
            repr_by_rid.insert(rid, repr);
        }
    }

    if regen::regen_mode() {
        std::fs::create_dir_all(golden.parent().unwrap()).unwrap();
        std::fs::write(&golden, &lines).unwrap();
        return;
    }

    let expected = std::fs::read_to_string(&golden).unwrap_or_else(|_| {
        panic!(
            "missing golden {}; regenerate with REGEN_TEMP_GOLDENS=1",
            golden.display()
        )
    });
    if expected.replace("\r\n", "\n") == lines.replace("\r\n", "\n") {
        return;
    }

    let (counts, drift) = drift_report(&expected, &lines, &repr_by_rid);
    panic!(
        "IR L2 feature snapshot drifted: {counts} (regenerate with REGEN_TEMP_GOLDENS=1 if intended):\n{}",
        drift.into_iter().take(20).collect::<Vec<_>>().join("\n")
    );
}

/// Classify snapshot drift per routine (#38). ADDED = not in the golden (a new
/// fixture licenses it); REMOVED = only in the golden; CHANGED = in both with a
/// different digest -- the only kind that is a behaviour movement to explain.
/// CHANGED lines come first, and each group is sorted, so the report is stable.
fn drift_report(
    expected: &str,
    actual: &str,
    repr_by_rid: &std::collections::BTreeMap<String, String>,
) -> (String, Vec<String>) {
    use std::collections::BTreeMap;
    let parse = |s: &'_ str| -> BTreeMap<String, String> {
        s.lines()
            .filter_map(|l| l.split_once('\t'))
            .map(|(r, d)| (r.to_string(), d.to_string()))
            .collect()
    };
    let (exp, act) = (parse(expected), parse(actual));
    let now = |rid: &str| repr_by_rid.get(rid).map_or("<none>", |j| j.as_str());
    let (mut changed, mut added, mut removed) = (Vec::new(), Vec::new(), Vec::new());
    for (rid, dig) in &act {
        match exp.get(rid) {
            None => added.push(format!("  ADDED {rid}")),
            Some(old) if old != dig => {
                changed.push(format!("  CHANGED {rid}\n    now: {}", now(rid)))
            }
            Some(_) => {}
        }
    }
    for rid in exp.keys().filter(|r| !act.contains_key(*r)) {
        removed.push(format!("  REMOVED {rid}"));
    }
    let counts = format!(
        "{} changed, {} added, {} removed",
        changed.len(),
        added.len(),
        removed.len()
    );
    (counts, [changed, removed, added].concat())
}

#[test]
fn drift_report_separates_added_from_changed() {
    let reprs = std::collections::BTreeMap::new();
    let golden = "a\t01\nb\t02\nc\t03\n";
    // A fixture-only addition: every old routine unchanged, two new ones.
    let (counts, lines) = drift_report(golden, "a\t01\nb\t02\nc\t03\nd\t04\ne\t05\n", &reprs);
    assert_eq!(counts, "0 changed, 2 added, 0 removed");
    assert_eq!(lines, ["  ADDED d", "  ADDED e"]);
    // A real movement on a pre-existing routine, plus one removal.
    let (counts, lines) = drift_report(golden, "a\t01\nb\tff\n", &reprs);
    assert_eq!(counts, "1 changed, 0 added, 1 removed");
    assert_eq!(lines, ["  CHANGED b\n    now: <none>", "  REMOVED c"]);
}

/// PROOF the Debug-based digest catches `#[serde(skip)]` drift that a serde-JSON /
/// PartialEq gate is BLIND to. `PRecordOperation.in_until_condition` is serde-skipped
/// AND excluded from the manual `PartialEq` — two ops differing ONLY in that field
/// serialize identically and compare equal, yet their `Debug` renderings (and thus
/// this gate's FNV digest) differ. This is exactly the blind spot that silently broke
/// 4 load-bearing fields during the migration; keep the snapshot digest on `Debug`.
#[test]
fn debug_digest_catches_serde_skip_drift() {
    use al_sem::program::body::features::{PAnchor, PRecordOperation, PTempState};
    let base = PRecordOperation {
        id: "r/op0".to_string(),
        op: "modify".to_string(),
        record_variable_name: "rec".to_string(),
        record_variable_id: None,
        temp_state: PTempState {
            kind: "known".to_string(),
            value: Some(false),
            parameter_index: None,
        },
        field_arguments: None,
        field_argument_infos: None,
        loop_stack: vec![],
        source_anchor: PAnchor {
            source_unit_id: "u".to_string(),
            start_line: 1,
            start_column: 0,
            end_line: 1,
            end_column: 8,
            syntax_kind: "call_expression".to_string(),
        },
        in_until_condition: false,
        run_trigger: None,
    };
    let mut flipped = base.clone();
    flipped.in_until_condition = true;

    // serde JSON is BLIND (#[serde(skip)]).
    assert_eq!(
        serde_json::to_string(&base).unwrap(),
        serde_json::to_string(&flipped).unwrap(),
        "serde must NOT see in_until_condition (it is #[serde(skip)])"
    );
    // PartialEq is BLIND (the manual impl excludes it).
    assert_eq!(
        base, flipped,
        "PartialEq must NOT see in_until_condition (excluded from the manual impl)"
    );
    // Debug SEES it → the digest differs → this gate catches the drift.
    assert_ne!(
        fnv1a(&format!("{base:?}")),
        fnv1a(&format!("{flipped:?}")),
        "the Debug digest MUST catch serde-skip drift"
    );
}

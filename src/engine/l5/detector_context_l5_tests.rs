//! Plan 2026-11-03 item L5: [`det_cone_inputs_over`] extracts every row's
//! direct facts in parallel. These tests hold the production path to a
//! serial oracle (the loop as it was before L5, written out here) on every
//! fixture app dir and on CDO, single-app and cross-app: `direct_full` and
//! `coverage_in` must be equal, compared both as maps and as a digest of
//! each map in key order.
//!
//! Discrimination (recorded in the plan's as-built note): reversing the
//! parallel collect's order in production moves both digests on CDO and on
//! the fixtures, and the test fails; restored, it passes.

use super::*;
use crate::engine::l4::capability_cone::callsite_local_scan_tests::{
    FIXTURE_ROOTS, app_dirs, workspace_model,
};
use crate::engine::l4::capability_cone::{
    DeadSites, build_cross_app_base, callsite_local_coverage_reasons, direct_facts_for_routine,
    fold_uncertainty_reasons,
};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::Path;

type Direct = HashMap<String, Vec<CapabilityFact>>;
type Coverage = HashMap<String, (String, Vec<String>)>;

/// The pre-L5 serial loop over the same inputs, then the same projection.
fn serial_oracle(m: &Model, rel: &ExecRelation) -> (Direct, Coverage) {
    let ws = &m.workspace;
    let mut pubs: HashMap<String, Vec<&EventSymbol>> = HashMap::new();
    for evt in &m.events.graph.events {
        if let Some(pr) = &evt.publisher_routine_id {
            pubs.entry(pr.clone()).or_default().push(evt);
        }
    }
    let reasons =
        callsite_local_coverage_reasons(rel.uncertainty_all(), &DeadSites::of(&ws.routines));
    let mut facts_per_row = Vec::new();
    let mut coverage_per_row = Vec::new();
    for r in &ws.routines {
        let p = pubs.get(&r.id).map_or(&[][..], Vec::as_slice);
        let (facts, status, rs) = direct_facts_for_routine(r, p);
        coverage_per_row.push(Some(fold_uncertainty_reasons(
            status,
            rs,
            reasons.get(r.id.as_str()),
        )));
        facts_per_row.push(Some(facts));
    }
    let rows = projection::RowsOfId::build(&ws.routines);
    let coverage = projection::merge_per_id(&rows, coverage_per_row, projection::least_complete);
    let direct = projection::merge_per_id(&rows, facts_per_row, |v| {
        projection::union_by_value(v, |_| ())
    });
    (direct, coverage)
}

/// A map's digest in key order (each value by its `Debug` text).
fn digest<V: std::fmt::Debug>(map: &HashMap<String, V>) -> u64 {
    let mut keys: Vec<&String> = map.keys().collect();
    keys.sort();
    let mut h = DefaultHasher::new();
    for k in keys {
        k.hash(&mut h);
        format!("{:?}", map[k]).hash(&mut h);
    }
    h.finish()
}

/// Production against the oracle over one model; returns the number of
/// routines compared.
fn check(m: &Model, rel: &ExecRelation, where_: &str, failures: &mut Vec<String>) -> usize {
    let got = det_cone_inputs(m, rel);
    let (direct, coverage) = serial_oracle(m, rel);
    let pairs = [
        ("direct_full", digest(&got.direct_full), digest(&direct)),
        ("coverage_in", digest(&got.coverage_in), digest(&coverage)),
    ];
    for (name, g, w) in pairs {
        if g != w {
            failures.push(format!("{where_}: {name} digest {g:016x}, serial {w:016x}"));
        }
    }
    if got.direct_full != direct || got.coverage_in != coverage {
        failures.push(format!("{where_}: the maps differ from the serial oracle"));
    }
    m.workspace.routines.len()
}

fn check_dir(dir: &Path, where_: &str, failures: &mut Vec<String>) -> (usize, usize) {
    let single = match workspace_model(dir) {
        Some(m) => check(&m, &ExecRelation::single_app(&m), where_, failures),
        None => {
            failures.push(format!("{where_}: no single-app model"));
            0
        }
    };
    let inputs = crate::program::inputs::WorkspaceInputs::read(dir, &Default::default());
    let cross = match build_cross_app_base(&inputs) {
        Ok(base) => check(
            &base.resolved,
            base.rel(),
            &format!("{where_} (cross-app)"),
            failures,
        ),
        Err(_) => {
            failures.push(format!("{where_}: no cross-app base"));
            0
        }
    };
    (single, cross)
}

#[test]
fn parallel_direct_facts_equal_serial_on_fixtures() {
    let tests = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let mut failures = Vec::new();
    let (mut single, mut cross) = (0, 0);
    for (root, want) in FIXTURE_ROOTS {
        let dirs = app_dirs(&tests.join(root), usize::MAX);
        assert_eq!(dirs.len(), want, "{root}: app dirs");
        for dir in &dirs {
            let (s, c) = check_dir(dir, &dir.display().to_string(), &mut failures);
            single += s;
            cross += c;
        }
    }
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
    // Precondition: the fixtures carry rows to compare in both modes.
    assert!(single > 1000 && cross > 1000, "routines {single} / {cross}");
}

/// The CDO half (skips without `CDO_WS`, panics under `ENFORCE_CDO_WS=1`).
#[test]
fn parallel_direct_facts_equal_serial_on_cdo() {
    let Some(ws) = crate::cdo::cdo_ws_or_enforce() else {
        return;
    };
    let mut failures = Vec::new();
    let (single, cross) = check_dir(&ws, "CDO", &mut failures);
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
    println!("L5 direct facts on CDO: {single} single-app rows, {cross} cross-app rows equal");
    // Precondition (2026-11-03, the pinned baseline): 5,479 / 33,287 rows.
    assert!(
        single > 5_000 && cross > 30_000,
        "routines {single} / {cross}"
    );
}

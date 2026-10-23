//! #220 (#160 step 8): roles, snapshot schema 4, preflight — the integration
//! half of the spec's test plan
//! (`docs/superpowers/specs/2026-10-22-issue-220-roles-snapshot-schema-4-design.md`
//! §5). Cross-app models come from `AnalysisTarget::build(inputs, false, ..)`
//! and the composition from `compose_full_snapshot` with
//! `SnapshotUniverse::of(&target)`.
//!
//! Fixtures: DD = `tests/common/dep_dispatch_fixture.rs` (`write_own`;
//! embedded-source dependency `XBase`).
//!
//! "Hand-stated" = a field set by assignment on the assembled model before
//! composing (CLAUDE.md).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use al_sem::engine::gate::cbor::CborValue;
use al_sem::engine::gate::diff::CoveragePolicy;
use al_sem::engine::gate::diff::cli::{DiffCliOptions, DiffRunOutcome, run_diff};
use al_sem::engine::gate::run::AnalysisTarget;
use al_sem::engine::l5::snapshot_full::{
    FullSnapshotOptions, SnapshotUniverse, compose_full_snapshot,
};
use al_sem::program::inputs::InputSet;
use serde_json::Value;

use crate::dep_dispatch_fixture as dd;

const DRIVER: &str = "cli-b-v1";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

struct Built {
    inputs: Arc<InputSet>,
    target: AnalysisTarget,
}

fn build(ws: &Path, single_app: bool) -> Built {
    let inputs = InputSet::read(ws, &crate::fingerprint_needs());
    let target = AnalysisTarget::build(&inputs, single_app, "t220").expect("analysis target");
    Built { inputs, target }
}

fn compose_with(b: &Built, universe: SnapshotUniverse) -> CborValue {
    compose_full_snapshot(
        b.target.resolved(),
        &FullSnapshotOptions {
            inputs: &b.inputs,
            driver_version: DRIVER,
            deterministic: true,
            roots_config_ignored: false,
            universe,
        },
    )
}

/// The snapshot of `b` in its own universe.
fn compose(b: &Built) -> CborValue {
    compose_with(b, SnapshotUniverse::of(&b.target))
}

fn dd_dir() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    dd::write_own(dir.path(), "");
    dir
}

// ---------------------------------------------------------------------------
// Preflight (D5): T7
// ---------------------------------------------------------------------------

fn vendored(name: &str) -> PathBuf {
    repo().join("tests/cli-b-goldens/diff/snapshots").join(name)
}

/// A temp copy of a vendored diff input, upgraded to v4 (D6's three edits),
/// then `edit` applied.
fn v4_copy(dir: &Path, name: &str, out: &str, edit: impl Fn(&mut Value)) -> PathBuf {
    let mut v: Value = serde_json::from_slice(&std::fs::read(vendored(name)).unwrap()).unwrap();
    v["schemaVersion"] = Value::from(4);
    v["analysisUniverse"] = Value::from("single-app");
    for app in v["apps"].as_array_mut().unwrap() {
        app["role"] = Value::from("primary");
    }
    edit(&mut v);
    let p = dir.join(out);
    std::fs::write(&p, serde_json::to_string_pretty(&v).unwrap()).unwrap();
    p
}

fn diff(old: &Path, new: &Path) -> DiffRunOutcome {
    let (o, n) = (
        old.to_string_lossy().to_string(),
        new.to_string_lossy().to_string(),
    );
    run_diff(&DiffCliOptions {
        old_arg: &o,
        new_arg: &n,
        format: "json",
        out: None,
        coverage_policy: CoveragePolicy::Loose,
        renames_path: None,
        fail_on: None,
        strict: false,
        deterministic: true,
        driver_version: DRIVER,
    })
}

/// T7: a temp copy of a vendored input edited back to `schemaVersion: 3`,
/// diffed against a v4 file. stderr names 3, v4 and the remedy; exit 1.
///
/// Break: accept 3 -> loads, fails.
#[test]
fn diff_refuses_schema_3() {
    let dir = tempfile::tempdir().unwrap();
    let old = v4_copy(dir.path(), "event-added-old.snap.json", "v3.json", |v| {
        v["schemaVersion"] = Value::from(3);
    });
    let new = v4_copy(dir.path(), "event-added-new.snap.json", "v4.json", |_| {});
    let v: Value = serde_json::from_slice(&std::fs::read(&old).unwrap()).unwrap();
    assert_eq!(v["schemaVersion"], 3, "precondition: the copy carries 3");
    let outcome = diff(&old, &new);
    assert_eq!(outcome.exit_code, 1);
    let msg = outcome.error_message.expect("a load error");
    assert!(msg.contains("schemaVersion 3"), "{msg}");
    assert!(msg.contains("(v4)"), "{msg}");
    assert!(
        msg.contains("re-create it with 'alsem fingerprint'"),
        "{msg}"
    );
}

// ---------------------------------------------------------------------------
// Key classes (D4): T12
// ---------------------------------------------------------------------------

/// T12 (O18): two cross-app targets built from the same workspace (equal
/// routine ids, asserted). Composing target A's model with
/// `SnapshotUniverse::of(&target_b)` panics with the model-identity message;
/// A with A composes.
///
/// Break: drop the `ptr::eq` assert -> it composes silently.
#[test]
fn cross_app_universe_binds_its_model() {
    let dir = dd_dir();
    let a = build(dir.path(), false);
    let b = build(dir.path(), false);
    let ids = |x: &Built| -> Vec<String> {
        x.target
            .resolved()
            .workspace
            .routines
            .iter()
            .map(|r| r.id.clone())
            .collect()
    };
    assert_eq!(ids(&a), ids(&b), "precondition: equal routine ids");
    let _ = compose(&a);
    let wrong = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        compose_with(&a, SnapshotUniverse::of(&b.target))
    }));
    let err = wrong.expect_err("composing A's model with B's universe must panic");
    let msg = err
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| err.downcast_ref::<&str>().map(|s| s.to_string()))
        .unwrap_or_default();
    assert!(msg.contains("model"), "the model-identity message: {msg}");
}

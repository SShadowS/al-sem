//! #160 step 9 T18 (spec §5): `compose_full_snapshot`, now the cut half plus
//! `assemble_full_snapshot`, writes the bytes the unsplit function wrote. The
//! goldens in `tests/cli-b-goldens/split/` were minted by the BASE code
//! (`70c3b743`, the unsplit `compose_full_snapshot`) on the same fixtures, in
//! both universes; the cli-b B0/snapshot/diff differentials cover single-app
//! B0 too, but never the `CrossApp` branch.

use std::path::{Path, PathBuf};

use al_sem::engine::gate::run::AnalysisTarget;
use al_sem::engine::l5::snapshot_full::{
    FullSnapshotOptions, SnapshotUniverse, compose_full_snapshot, serialize_cbor, to_sorted_json,
};

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// `compose_full_snapshot`'s (json, cbor) of `ws` in its own universe.
fn compose(ws: &Path, single_app: bool) -> (String, Vec<u8>) {
    let inputs = al_sem::program::inputs::WorkspaceInputs::read(ws, &crate::fingerprint_needs());
    let target = AnalysisTarget::build(&inputs, single_app, "t").expect("target");
    let tree = compose_full_snapshot(
        target.resolved(),
        &FullSnapshotOptions {
            inputs: &inputs,
            driver_version: "cli-split-v1",
            deterministic: true,
            roots_config_ignored: false,
            universe: SnapshotUniverse::of(&target),
        },
    );
    (to_sorted_json(&tree), serialize_cbor(&tree))
}

/// T18: DD (`dd::write_own`, a source dependency) cross-app and single-app,
/// and R5B (two symbol-only dependencies) cross-app: json and cbor equal the
/// base's, byte for byte. Precondition: the cross-app documents name the
/// cross-app universe and hold a `permissionFacts` row (so the permission
/// path is exercised). DISCRIMINATION: an empty permission array from
/// `compose_full_snapshot` makes `permissionFacts` differ.
#[test]
fn full_snapshot_split_is_byte_identical() {
    let dd_dir = tempfile::tempdir().unwrap();
    crate::dep_dispatch_fixture::write_own(dd_dir.path(), "");
    let r5b = repo().join("tests/r2-5b-fixtures/cross-app-resolution");
    let cases: [(&Path, bool, &str); 3] = [
        (dd_dir.path(), false, "full-snapshot.dd.cross-app"),
        (dd_dir.path(), true, "full-snapshot.dd.single-app"),
        (&r5b, false, "full-snapshot.r5b.cross-app"),
    ];
    for (ws, single_app, name) in cases {
        let (json, cbor) = compose(ws, single_app);
        // Minted from the base code, so never regenerated.
        let dir = repo().join("tests/cli-b-goldens/split");
        let want_json = std::fs::read_to_string(dir.join(format!("{name}.json"))).unwrap();
        let want_cbor = std::fs::read(dir.join(format!("{name}.cbor"))).unwrap();
        // Preconditions on the base's document.
        let doc: serde_json::Value = serde_json::from_str(&want_json).unwrap();
        if !single_app {
            assert_eq!(doc["analysisUniverse"], "cross-app", "precondition: {name}");
        }
        assert!(
            doc["permissionFacts"]
                .as_array()
                .is_some_and(|p| !p.is_empty()),
            "precondition: {name} has permission facts"
        );
        assert_eq!(json, want_json, "{name}: json");
        assert_eq!(cbor, want_cbor, "{name}: cbor");
    }
}

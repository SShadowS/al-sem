//! Test universes (agent-tasks C-2, C-2b, F-7) over
//! `tests/fixtures/test-universe`.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use super::*;
use crate::engine::resident::manifest::{Hashing, Manifest};
use crate::program::impact_graph::{SitedGraph, sited_from_program};
use crate::program::resolved::{ProgramScope, ResolvedProgram};

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/test-universe")
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap() {
        let e = e.unwrap();
        let t = to.join(e.file_name());
        if e.file_type().unwrap().is_dir() {
            copy_dir(&e.path(), &t);
        } else {
            std::fs::copy(e.path(), &t).unwrap();
        }
    }
}

const PROD: &str = "7d3c1a20-5b8e-4f61-9a2d-0c4e6b1f8a01";
const TEST: &str = "7d3c1a20-5b8e-4f61-9a2d-0c4e6b1f8a02";

fn graph(ws: &Path, injected: &Injected) -> (SitedGraph, std::sync::Arc<WorkspaceInputs>) {
    let inputs = WorkspaceInputs::read_scoped(ws, &Default::default(), injected);
    let p = ResolvedProgram::build(&inputs, ProgramScope::CrossApp).expect("builds");
    (
        sited_from_program(&p, &|| false).expect("not cancelled"),
        inputs,
    )
}

/// The production app's routine nodes of `g`: `(stable id, node)`.
fn production_routines(g: &SitedGraph) -> Vec<(String, u32)> {
    (0..g.node_count() as u32)
        .filter(|&n| {
            g.node_app(n)
                .is_some_and(|a| g.app_identity(a).0.eq_ignore_ascii_case(PROD))
        })
        .filter_map(|n| g.stable_id(n).map(|s| (s, n)))
        .collect()
}

/// A node's resolved out-edges, by the target's node id (equal across
/// universes for one app: ids are scoped per app), kind and `via`.
fn out_edges(g: &SitedGraph, n: u32) -> BTreeSet<(String, String, &'static str)> {
    g.sites()
        .iter()
        .filter(|s| {
            s.from == n && s.boundary != crate::program::impact_graph::sited::Boundary::Open
        })
        .map(|s| (g.node_id(s.to), format!("{:?}", s.kind), s.via()))
        .collect()
}

/// C-2 and C-2b: every production routine of the production universe is in
/// the test universe under the same StableRoutineId (the production app is
/// an injected source dependency there), and its resolved out-edges are the
/// same. The production stamp is the same function on both sides: equal.
///
/// DISCRIMINATION (2026-11-05): injecting no source (an empty
/// `Injected::sources`) leaves the test universe without the production app
/// as source: the stable-id lookup fails for every routine. Resolving the
/// injected app's calls with the approximate tier's fallback is not a knob
/// here; the edge comparison is the guard that would see it.
#[test]
fn production_routines_keep_their_ids_and_edges_in_the_test_universe() {
    let root = fixture();
    let (prod, prod_inputs) = graph(&root.join("App"), &Injected::default());
    let u = TestUniverse::new(&root.join("Test"), TEST, &root.join("App"), PROD, "auto");
    let (test, test_inputs) = graph(&root.join("Test"), &u.injected());
    let routines = production_routines(&prod);
    assert!(routines.len() >= 6, "{routines:?}");
    for (id, n) in &routines {
        let there = test.nodes_with_stable_id(id);
        assert_eq!(there.len(), 1, "{id} in the test universe");
        assert_eq!(
            out_edges(&prod, *n),
            out_edges(&test, there[0]),
            "the out-edges of {id}"
        );
        assert_ne!(
            test.flags(there[0]) & crate::program::impact_graph::sited::flag::LOCAL,
            0,
            "the production app is local source in the test universe"
        );
    }
    let a = production_source_stamp(&prod_inputs, PROD);
    let b = production_source_stamp(&test_inputs, PROD);
    assert!(a.is_some());
    assert_eq!(a, b, "one stamp function, both sides");
}

/// The key is the descriptor's (how it was found is left out), and the
/// workspace's own universe is `workspace`.
#[test]
fn universe_keys() {
    let root = fixture();
    let a = TestUniverse::new(&root.join("Test"), TEST, &root.join("App"), PROD, "auto");
    let b = TestUniverse::new(&root.join("Test"), TEST, &root.join("App"), PROD, "call");
    assert_eq!(a.key(), b.key());
    assert!(a.key().starts_with("tests:"));
    let c = TestUniverse::new(&root.join("TestFR"), TEST, &root.join("App"), PROD, "auto");
    assert_ne!(a.key(), c.key());
    assert_eq!(
        TestUniverse::workspace(&root.join("App"), PROD).key(),
        "workspace"
    );
}

/// F-7: a test universe's manifest names its injected inputs, and an edit
/// of the injected production source is reported on that engine as
/// `injected-source` with its path; the freshness check observes it from
/// the published manifest alone (`check_against` reads the `inj:` entries).
///
/// DISCRIMINATION (2026-11-05): observing only the test root
/// (`check_against` passing `Injected::default()`) reports no change at all:
/// the edit is invisible.
#[test]
fn an_injected_production_edit_is_a_change_of_the_test_universe() {
    let t = tempfile::tempdir().unwrap();
    copy_dir(&fixture(), t.path());
    let root = t.path();
    let u = TestUniverse::new(&root.join("Test"), TEST, &root.join("App"), PROD, "auto");
    let ws = root.join("Test");
    let inputs = WorkspaceInputs::read_scoped(&ws, &Default::default(), &u.injected());
    let consumed = Manifest::consumed(&inputs, false);
    let observed = Manifest::observe_scoped(&ws, &u.injected(), false, Hashing::All);
    assert_eq!(consumed, observed, "the read and the observation agree");
    assert_eq!(consumed.injected(), u.injected());
    let core = root.join("App/src/Core.al");
    let text = std::fs::read_to_string(&core).unwrap();
    std::fs::write(&core, text.replace("Item.Qty + 1", "Item.Qty + 2")).unwrap();
    let checked = Manifest::check_against(&consumed, &ws, false, true);
    let changes = consumed.changes(&checked, &ws);
    let cats: Vec<(&str, Option<String>)> = changes
        .iter()
        .map(|c| (c.category, c.path.clone()))
        .collect();
    assert_eq!(cats.len(), 1, "{cats:?}");
    assert_eq!(cats[0].0, "injected-source");
    assert!(
        cats[0]
            .1
            .as_deref()
            .is_some_and(|p| p.ends_with("App/src/Core.al")),
        "{cats:?}"
    );
}

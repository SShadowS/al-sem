//! T-manifest (design §5.2, §11): the manifest covers every input a build
//! reads, `M_consumed` (from the acquired read) equals an observation of an
//! unchanged disk, and every kind of input change moves it.

use super::*;
use crate::program::inputs::{WorkspaceInputs, WorkspaceNeeds};

const READ: WorkspaceNeeds = WorkspaceNeeds {
    build_only: false,
    fingerprint_rows: true,
};

/// A temporary copy of a workspace fixture with packages (`.alpackages`).
pub(crate) fn copy_fixture(name: &str) -> tempfile::TempDir {
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(name);
    let dir = tempfile::tempdir().unwrap();
    copy_tree(&src, dir.path());
    dir
}

fn copy_tree(from: &Path, to: &Path) {
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            std::fs::create_dir_all(&target).unwrap();
            copy_tree(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).unwrap();
        }
    }
}

fn observe_all(ws: &Path) -> Manifest {
    Manifest::observe(ws, false, Hashing::All)
}

/// An unchanged workspace: the read's manifest equals both observations,
/// hashed and stat-cached, and the stamp is the scheme's.
#[test]
fn consumed_equals_an_observation_of_an_unchanged_disk() {
    let dir = copy_fixture("tests/fixtures/lsp-diff-deps");
    let ws = dir.path();
    let before = observe_all(ws);
    let inputs = WorkspaceInputs::read(ws, &READ);
    let consumed = Manifest::consumed(&inputs, false);
    let dirty = BTreeSet::new();
    let after = Manifest::observe(
        ws,
        false,
        Hashing::Cached {
            prev: &before,
            dirty: &dirty,
        },
    );
    assert!(
        consumed.entries().keys().any(|k| k.starts_with("pkg:/")),
        "precondition: the fixture has packages: {:?}",
        consumed.entries().keys().collect::<Vec<_>>()
    );
    assert!(consumed.entries().contains_key("al:Caller.al"));
    assert_eq!(before, consumed);
    assert_eq!(consumed, after);
    assert!(check_consistent(&before, &consumed, &after).is_ok());
    assert!(consumed.stamp().starts_with(STAMP_PREFIX));
    assert_eq!(consumed.stamp(), after.stamp());
    assert_ne!(
        Manifest::consumed(&inputs, true).stamp(),
        consumed.stamp(),
        "the universe is an input"
    );
}

/// Every input kind the build reads moves the manifest and names its path:
/// a `.al` edit, add and remove; `app.json`; `roots.config.json` and the
/// coverage yaml appearing; a package's bytes; a package added; the source
/// dependency config appearing.
///
/// DISCRIMINATION (2026-11-02): leaving the package entries out of
/// `observe` (the `discover_app_files` loop skipped) fails at the first
/// case, "a .al edit: consumed equals observed" (the read's manifest names
/// the packages, the observation does not), and so does
/// `consumed_equals_an_observation_of_an_unchanged_disk`.
#[test]
fn every_input_kind_moves_the_manifest() {
    let dir = copy_fixture("tests/fixtures/lsp-diff-deps");
    let ws = dir.path();
    let pkg = ws.join(".alpackages/aaaaaaaa-0000-0000-0000-000000000001.app");
    type Edit = Box<dyn Fn(&Path)>;
    let cases: Vec<(&str, Edit, Option<std::path::PathBuf>)> = vec![
        (
            "a .al edit",
            Box::new(|ws| {
                let p = ws.join("Caller.al");
                let t = std::fs::read_to_string(&p).unwrap();
                std::fs::write(&p, t.replace("Compute(5)", "Compute(6)")).unwrap();
            }),
            Some(ws.join("Caller.al")),
        ),
        (
            "a .al added",
            Box::new(|ws| std::fs::write(ws.join("New.al"), "codeunit 50001 New {}\n").unwrap()),
            Some(ws.join("New.al")),
        ),
        (
            "a .al removed",
            Box::new(|ws| std::fs::remove_file(ws.join("New.al")).unwrap()),
            Some(ws.join("New.al")),
        ),
        (
            "app.json",
            Box::new(|ws| {
                let p = ws.join("app.json");
                let t = std::fs::read_to_string(&p).unwrap();
                assert!(t.contains("\"probe\""), "precondition: the publisher");
                std::fs::write(&p, t.replace("\"probe\"", "\"probe2\"")).unwrap();
            }),
            Some(ws.join("app.json")),
        ),
        (
            "roots.config.json appears",
            Box::new(|ws| std::fs::write(ws.join("roots.config.json"), "{}").unwrap()),
            Some(ws.join("roots.config.json")),
        ),
        (
            "the coverage yaml appears",
            Box::new(|ws| std::fs::write(ws.join("al-sem.coverage.yaml"), "x: 1\n").unwrap()),
            Some(ws.join("al-sem.coverage.yaml")),
        ),
        (
            "a package's bytes",
            Box::new(move |_| {
                let mut b = std::fs::read(&pkg).unwrap();
                b.push(0);
                std::fs::write(&pkg, b).unwrap();
            }),
            Some(ws.join(".alpackages/aaaaaaaa-0000-0000-0000-000000000001.app")),
        ),
        (
            "a package added",
            Box::new(|ws| {
                std::fs::copy(
                    ws.join(".alpackages/bbbbbbbb-0000-0000-0000-000000000002.app"),
                    ws.join(".alpackages/cccc.app"),
                )
                .unwrap();
            }),
            Some(ws.join(".alpackages/cccc.app")),
        ),
        (
            "the source dependency config appears",
            Box::new(|ws| std::fs::write(ws.join("al-sem.dependencies.json"), "{").unwrap()),
            None,
        ),
    ];
    for (what, edit, path) in cases {
        let before = observe_all(ws);
        edit(ws);
        let dirty = BTreeSet::new();
        // The stat-cached check sees it (every case changes length or mtime).
        let after = Manifest::observe(
            ws,
            false,
            Hashing::Cached {
                prev: &before,
                dirty: &dirty,
            },
        );
        assert_ne!(before, after, "{what}: no change seen");
        assert_ne!(before.stamp(), after.stamp(), "{what}: the stamp");
        if let Some(p) = path {
            let changed = before.changed_paths(&after, ws);
            assert!(
                changed.contains(&p.to_string_lossy().into_owned()),
                "{what}: {changed:?}"
            );
        }
        // The read sees the same as the observation.
        let consumed = Manifest::consumed(&WorkspaceInputs::read(ws, &READ), false);
        assert_eq!(consumed, after, "{what}: consumed equals observed");
    }
}

/// A read of B between observations of A fails the three-way check, though
/// before and after are equal (§5.1).
#[test]
fn a_consumed_b_between_two_observations_of_a_is_unstable() {
    let dir = copy_fixture("tests/fixtures/lsp-diff-deps");
    let ws = dir.path();
    let p = ws.join("Caller.al");
    let a = std::fs::read_to_string(&p).unwrap();
    let before = observe_all(ws);
    std::fs::write(&p, a.replace("Compute(5)", "Compute(6)")).unwrap();
    let consumed = Manifest::consumed(&WorkspaceInputs::read(ws, &READ), false);
    std::fs::write(&p, &a).unwrap();
    let after = observe_all(ws);
    assert_eq!(
        before, after,
        "precondition: the disk holds A before and after"
    );
    let e = check_consistent(&before, &consumed, &after).unwrap_err();
    assert_eq!(e.key, "al:Caller.al");
    assert!(e.to_string().contains("changed during the build"), "{e}");
}

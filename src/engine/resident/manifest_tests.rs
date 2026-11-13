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

/// F-6 (agent-tasks design §2.5): a source dependency's file is an input
/// with its own stat. Editing one is reported as category
/// `source-dependency` with that file's path, and the stat-cached check
/// reads and hashes only that file (the other dependency file and every
/// workspace file reuse their hash). The read's manifest equals the
/// observation before and after the edit.
///
/// DISCRIMINATION (2026-11-05): dropping the `src:` entries (no per-file
/// entry) fails at the precondition "the file is its own entry"; reading
/// every dependency file in the cached check (a key `prev` never holds)
/// fails at "only the edited file is hashed" (both files); dropping the
/// `src:` keys from `changes`, as `changed_paths` dropped
/// `ws:source-dependencies` before, fails at "the edited file's path"
/// (no change). Each restored, it passes.
#[test]
fn a_source_dependency_file_edit_is_its_own_change_and_only_it_is_hashed() {
    let t = tempfile::tempdir().unwrap();
    let ws = t.path().join("ws");
    let write = |p: &Path, text: &str| {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    };
    write(
        &ws.join("app.json"),
        r#"{"id":"aaaaaaaa-0000-4000-8000-000000000001","name":"W","publisher":"t","version":"1.0.0.0"}"#,
    );
    write(&ws.join("W.Codeunit.al"), "codeunit 50100 \"W\" { }\n");
    write(
        &ws.join(crate::program::source_deps::CONFIG_FILE),
        r#"{ "sources": ["../dep"] }"#,
    );
    write(
        &t.path().join("dep/app.json"),
        r#"{"id":"dddddddd-0000-4000-8000-000000000001","name":"D","publisher":"t","version":"1.0.0.0"}"#,
    );
    let a = t.path().join("dep/A.Codeunit.al");
    write(&a, "codeunit 50200 \"D A\" { }\n");
    write(
        &t.path().join("dep/B.Codeunit.al"),
        "codeunit 50201 \"D B\" { }\n",
    );

    let before = observe_all(&ws);
    let consumed = Manifest::consumed(&WorkspaceInputs::read(&ws, &READ), false);
    assert_eq!(
        consumed, before,
        "consumed equals observed with a source dependency"
    );
    let key = "src:../dep\u{0}A.Codeunit.al";
    assert!(
        before.entries().contains_key(key),
        "precondition: the file is its own entry: {:?}",
        before.entries().keys().collect::<Vec<_>>()
    );

    write(&a, "codeunit 50200 \"D A\" { procedure X() begin end; }\n");
    let dirty = BTreeSet::new();
    let after = Manifest::observe(
        &ws,
        false,
        Hashing::Cached {
            prev: &before,
            dirty: &dirty,
        },
    );
    assert_eq!(
        after.hashed(),
        [key.to_string()],
        "only the edited file is hashed"
    );
    let changes = before.changes(&after, &ws);
    assert_eq!(
        changes,
        vec![InputChange {
            category: "source-dependency",
            path: Some(
                ws.join("../dep")
                    .join("A.Codeunit.al")
                    .to_string_lossy()
                    .into_owned()
            ),
            detail: None,
        }],
        "the edited file's path"
    );
    let consumed = Manifest::consumed(&WorkspaceInputs::read(&ws, &READ), false);
    assert_eq!(consumed, after, "consumed equals observed after the edit");
}

/// TLS-30: two names that are not UTF-8 and read alike (`b"\xff.al"` and
/// `b"\xfe.al"`, both `\u{fffd}.al` lossily) keep two keys, in the workspace
/// and in a source dependency. `consumed` agrees with an observation, the
/// descriptor round trip keeps both, and an edit to the FIRST twin of each
/// (the one a lossy key lost: a later insert wins) moves the stamp and is
/// named by `changes`.
///
/// Discrimination (2026-11-13): the keys lossy again (`rel_key` returning
/// `rel_posix`) fails at "precondition: two keys per twin pair" (1 `al:` key
/// seen, 2 expected); `path_key` lossy again fails there too (1 package key
/// seen, 2 expected); restored, passes.
#[cfg(unix)]
#[test]
fn non_utf8_twin_names_keep_two_keys() {
    use std::os::unix::ffi::OsStrExt;
    let t = tempfile::tempdir().unwrap();
    let ws = t.path().join("ws");
    let dep = t.path().join("dep");
    let write = |p: &Path, text: &str| {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    };
    write(
        &ws.join("app.json"),
        r#"{"id":"aaaaaaaa-0000-4000-8000-000000000001","name":"W","publisher":"t","version":"1.0.0.0"}"#,
    );
    write(
        &ws.join(crate::program::source_deps::CONFIG_FILE),
        r#"{ "sources": ["../dep"] }"#,
    );
    write(
        &dep.join("app.json"),
        r#"{"id":"dddddddd-0000-4000-8000-000000000001","name":"D","publisher":"t","version":"1.0.0.0"}"#,
    );
    // Package twins (review r1, Opus): their bytes are no valid `.app`; the
    // manifest records each one's state either way.
    for (name, text) in [(&b"\xff.app"[..], "one"), (&b"\xfe.app"[..], "two")] {
        write(
            &ws.join(".alpackages")
                .join(std::ffi::OsStr::from_bytes(name)),
            text,
        );
    }
    for (folder, base) in [(&ws, 50100), (&dep, 50200)] {
        for (i, name) in [&b"\xff.al"[..], &b"\xfe.al"[..]].into_iter().enumerate() {
            let id = base + i;
            write(
                &folder.join(std::ffi::OsStr::from_bytes(name)),
                &format!("codeunit {id} \"T{id}\" {{ }}\n"),
            );
        }
    }

    let before = observe_all(&ws);
    let inputs = WorkspaceInputs::read(&ws, &READ);
    assert_eq!(
        Manifest::consumed(&inputs, false),
        before,
        "consumed equals observed"
    );
    let count = |prefix: &str| {
        before
            .entries()
            .keys()
            .filter(|k| k.starts_with(prefix))
            .count()
    };
    assert_eq!(
        (
            count("al:"),
            count("src:"),
            count("pkg:") - 2 // `pkg:list`, `pkg:unreadable-folders`
        ),
        (2, 2, 2),
        "precondition: two keys per twin pair: {:?}",
        before.entries().keys().collect::<Vec<_>>()
    );
    assert_eq!(
        Manifest::from_descriptor_entries(&before.descriptor_entries()).unwrap(),
        before,
        "the descriptor round trip keeps both"
    );

    // The first twin of each pair, by the walks' order.
    let first_ws = inputs.al_files[0].abs_path.clone();
    let first_dep =
        crate::source_text::discover_al_files(&dep, crate::source_text::NestedApps::Walk).unwrap()
            [0]
        .abs_path
        .clone();
    // A message spells the path lossily (both twins alike): the folder and
    // the U+FFFD name.
    let shown = |folder: &Path| {
        folder
            .join(std::ffi::OsStr::from_bytes(b"\xff.al"))
            .to_string_lossy()
            .into_owned()
    };
    for (first, category, folder) in [
        (&first_ws, "al-file", ws.clone()),
        (&first_dep, "source-dependency", ws.join("../dep")),
    ] {
        let was = observe_all(&ws);
        write(first, "codeunit 50999 \"Edited\" { }\n");
        let now = observe_all(&ws);
        assert_ne!(
            was.stamp(),
            now.stamp(),
            "{category}: the edit moves the stamp"
        );
        let changes = was.changes(&now, &ws);
        assert_eq!(changes.len(), 1, "{category}: one change: {changes:?}");
        assert_eq!(changes[0].category, category);
        assert_eq!(
            changes[0].path.as_deref(),
            Some(shown(&folder).as_str()),
            "{category}: the edited file's path"
        );
    }
    // Each package twin in turn: a lossy key loses one of them, whichever
    // the discovery lists first.
    for name in [&b"\xff.app"[..], &b"\xfe.app"[..]] {
        let was = observe_all(&ws);
        write(
            &ws.join(".alpackages")
                .join(std::ffi::OsStr::from_bytes(name)),
            "edited",
        );
        let now = observe_all(&ws);
        assert_ne!(
            was.stamp(),
            now.stamp(),
            "{name:?}: the edit moves the stamp"
        );
        let changes = was.changes(&now, &ws);
        assert_eq!(changes.len(), 1, "{name:?}: one change: {changes:?}");
        assert_eq!(changes[0].category, "package");
    }
    assert_eq!(
        Manifest::consumed(&WorkspaceInputs::read(&ws, &READ), false),
        observe_all(&ws),
        "consumed equals observed after the edits"
    );
}

/// TLS-30 (review r1, sol and Opus): a non-UTF-8 injected package root
/// comes back from the manifest as the same path, so a freshness check
/// observes the folder that was read. Discrimination (2026-11-13):
/// `inj:pkg-roots` written with the lossy `path_text` again fails here.
#[cfg(unix)]
#[test]
fn a_non_utf8_injected_package_root_round_trips() {
    use std::os::unix::ffi::OsStrExt;
    let t = tempfile::tempdir().unwrap();
    let root = t.path().join(std::ffi::OsStr::from_bytes(b"pkgs-\xff"));
    std::fs::create_dir_all(&root).unwrap();
    let injected = Injected {
        sources: Vec::new(),
        package_roots: vec![root.clone()],
    };
    let m = Manifest::observe_scoped(t.path(), &injected, false, Hashing::All);
    let back = Manifest::from_descriptor_entries(&m.descriptor_entries()).unwrap();
    assert_eq!(back.injected().package_roots, vec![root]);
}

/// The categories of every kind of change (agent-tasks design §2.3): a
/// file on one side only is a listing change with `added`/`removed`, an
/// edit is its file's category, and the listing entry itself is not
/// reported again beside the files that explain it.
#[test]
fn changes_name_each_inputs_category() {
    let dir = copy_fixture("tests/fixtures/lsp-diff-deps");
    let ws = dir.path();
    let before = observe_all(ws);
    let p = ws.join("Caller.al");
    let t = std::fs::read_to_string(&p).unwrap();
    std::fs::write(&p, t.replace("Compute(5)", "Compute(6)")).unwrap();
    std::fs::write(ws.join("New.al"), "codeunit 50001 New {}\n").unwrap();
    std::fs::write(ws.join("roots.config.json"), "{}").unwrap();
    std::fs::copy(
        ws.join(".alpackages/bbbbbbbb-0000-0000-0000-000000000002.app"),
        ws.join(".alpackages/cccc.app"),
    )
    .unwrap();
    let after = observe_all(ws);
    let got: Vec<(&str, Option<String>, Option<String>)> = before
        .changes(&after, ws)
        .into_iter()
        .map(|c| (c.category, c.path, c.detail))
        .collect();
    let path = |rel: &str| Some(ws.join(rel).to_string_lossy().into_owned());
    assert_eq!(
        got,
        vec![
            ("al-file", path("Caller.al"), None),
            ("al-walk", path("New.al"), Some("added".into())),
            (
                "package-list",
                path(".alpackages/cccc.app"),
                Some("added".into())
            ),
            ("roots-config", path("roots.config.json"), None),
        ]
    );
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

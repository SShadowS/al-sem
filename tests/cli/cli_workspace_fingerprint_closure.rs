//! #70 (spec `docs/superpowers/specs/2026-10-15-issue-70-cross-app-follow-ups-design.md`,
//! Acceptance rows 2a-2c, 2f-2h): `workspaceFingerprint` covers the whole
//! dependency closure the loader reads (the workspace's own `.alpackages` AND
//! every ancestor `.alpackages` up to the first directory with `.git`), so
//! swapping any loaded `.app` changes it, and nothing else does.
//!
//! Every case spawns `CARGO_BIN_EXE_alsem` (`digest` and `fingerprint`, JSON) on
//! the `tests/common/dep_dispatch_fixture.rs` workspace.

use std::path::Path;
use std::process::Command;

use serde_json::Value;

use crate::dep_dispatch_fixture::{DEP_APP_FILE, DEP_GUID, write_dependency, write_workspace};

const VERSION_OVERRIDE: &str = "cli-fp-closure-v1";

fn json(args: &[&str]) -> Value {
    let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .args(args)
        .args(["--deterministic", "--format", "json"])
        .env("ALCH_DRIVER_VERSION_OVERRIDE", VERSION_OVERRIDE)
        .output()
        .expect("spawn alsem");
    let text = String::from_utf8_lossy(&out.stdout);
    serde_json::from_str(&text).unwrap_or_else(|e| {
        panic!(
            "{args:?}: {e}: exit {:?}: stdout {text:?} stderr {}",
            out.status.code(),
            String::from_utf8_lossy(&out.stderr)
        )
    })
}

/// `alsem fingerprint <ws> --format json` (the full snapshot).
fn fingerprint(ws: &Path) -> Value {
    json(&["fingerprint", &ws.to_string_lossy()])
}

fn fp(doc: &Value) -> String {
    doc["payload"]["workspaceFingerprint"]
        .as_str()
        .unwrap_or_else(|| panic!("no workspaceFingerprint: {doc}"))
        .to_string()
}

/// `(kind, path)` of every snapshot input.
fn inputs(doc: &Value) -> Vec<(String, String)> {
    doc["payload"]["inputs"]
        .as_array()
        .unwrap_or_else(|| panic!("no inputs: {doc}"))
        .iter()
        .map(|i| {
            (
                i["kind"].as_str().unwrap_or("").to_string(),
                i["path"].as_str().unwrap_or("").to_string(),
            )
        })
        .collect()
}

/// `alsem digest <ws> --file src/Work.al --format json`.
fn digest(ws: &Path) -> Value {
    json(&["digest", &ws.to_string_lossy(), "--file", "src/Work.al"])
}

fn digest_apps(doc: &Value) -> Vec<(String, String)> {
    doc["payload"]["dependencies"]["apps"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| {
            (
                a["appId"].as_str().unwrap_or("").to_string(),
                a["source"].as_str().unwrap_or("").to_string(),
            )
        })
        .collect()
}

fn dep(source: &str) -> Vec<(String, String)> {
    vec![(DEP_GUID.to_string(), source.to_string())]
}

fn mark_repo_root(dir: &Path, git_as_file: bool) {
    std::fs::create_dir_all(dir).unwrap();
    if git_as_file {
        std::fs::write(dir.join(".git"), "gitdir: ../elsewhere/.git/worktrees/x\n").unwrap();
    } else {
        std::fs::create_dir_all(dir.join(".git")).unwrap();
    }
}

/// Row 2a. The `.app` lives in the PARENT `.alpackages` (a monorepo cache; the
/// parent is the repository root). Precondition: the cross-app digest loads it
/// (`apps == [XBase embedded]`). Rewriting the `.app`'s bytes changes both the
/// digest's and the fingerprint's `workspaceFingerprint`.
///
/// Mutation: `derive_inputs` back to `read_dir(<ws>/.alpackages)` -> unchanged
/// (today's behaviour, probe D2).
#[test]
fn fingerprint_tracks_an_ancestor_dependency_swap() {
    let dir = tempfile::tempdir().unwrap();
    mark_repo_root(dir.path(), false);
    let ws = dir.path().join("ws");
    write_workspace(&ws, "");
    let app = dir.path().join(".alpackages").join(DEP_APP_FILE);
    write_dependency(&app, 0);

    let d0 = digest(&ws);
    assert_eq!(digest_apps(&d0), dep("embedded"), "precondition: {d0}");
    let f0 = fingerprint(&ws);

    write_dependency(&app, 1);
    let d1 = digest(&ws);
    assert_eq!(digest_apps(&d1), dep("embedded"), "{d1}");
    let f1 = fingerprint(&ws);

    assert_ne!(
        fp(&d0),
        fp(&d1),
        "digest workspaceFingerprint unchanged by an ancestor dependency swap"
    );
    assert_ne!(
        fp(&f0),
        fp(&f1),
        "fingerprint workspaceFingerprint unchanged by an ancestor dependency swap"
    );
}

/// Row 2b (passes on master, probe D2; the regression pin). The `.app` in the
/// workspace's own `.alpackages`: rewriting it changes the fingerprint.
#[test]
fn fingerprint_tracks_an_own_dependency_swap() {
    let dir = tempfile::tempdir().unwrap();
    mark_repo_root(dir.path(), false);
    let ws = dir.path().join("ws");
    write_workspace(&ws, "");
    let app = ws.join(".alpackages").join(DEP_APP_FILE);
    write_dependency(&app, 0);
    let f0 = fingerprint(&ws);
    write_dependency(&app, 1);
    let f1 = fingerprint(&ws);
    assert_ne!(fp(&f0), fp(&f1));
}

/// Row 2c. A nested workspace (`outer/repo/.git`, the `.app` in
/// `outer/repo/.alpackages`, the workspace at `outer/repo/ws`). Precondition:
/// the fingerprint's inputs carry the ancestor package under its closure key
/// `../.alpackages/<name>` (so the equalities below are not vacuous). Then:
/// the same closure twice -> equal; a non-`.app` file added to the ancestor
/// cache -> equal; an `.app` added ABOVE the `.git` boundary (`outer/.alpackages`)
/// -> equal.
///
/// Mutations: hash every file in the cache folder, or ignore the `.git`
/// boundary -> differ.
#[test]
fn fingerprint_ignores_everything_but_the_closure() {
    let dir = tempfile::tempdir().unwrap();
    let outer = dir.path().join("outer");
    let repo = outer.join("repo");
    mark_repo_root(&repo, false);
    let ws = repo.join("ws");
    write_workspace(&ws, "");
    write_dependency(&repo.join(".alpackages").join(DEP_APP_FILE), 0);

    let base = fingerprint(&ws);
    let key = format!("../.alpackages/{DEP_APP_FILE}");
    assert!(
        inputs(&base).contains(&("dep-package".to_string(), key.clone())),
        "precondition: the ancestor package {key} is a fingerprint input: {:?}",
        inputs(&base)
    );
    let base = fp(&base);

    assert_eq!(fp(&fingerprint(&ws)), base, "same closure twice");

    std::fs::write(repo.join(".alpackages").join("readme.txt"), "not a package").unwrap();
    assert_eq!(fp(&fingerprint(&ws)), base, "a non-.app file in the cache");

    write_dependency(
        &outer.join(".alpackages").join("probe_Other_1.0.0.0.app"),
        7,
    );
    assert_eq!(
        fp(&fingerprint(&ws)),
        base,
        "an .app above the .git boundary"
    );
}

/// Row 2g (CLI half). The workspace is ITSELF a repository root (`.git` as a
/// directory, then as a worktree file); an `.alpackages` above it holds the
/// declared dependency. The loader must not read it (the digest lists XBase as
/// `missing`), and rewriting it must not change the fingerprint.
///
/// Mutation: delete the new `project_root/.git` check in
/// `find_all_alpackages_folders` -> the above-root cache is loaded (`embedded`)
/// and the fingerprint changes.
#[test]
fn fingerprint_ignores_a_cache_above_a_repository_root_workspace() {
    for git_as_file in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().join("ws");
        write_workspace(&ws, "");
        mark_repo_root(&ws, git_as_file);
        let app = dir.path().join(".alpackages").join(DEP_APP_FILE);
        write_dependency(&app, 0);

        let d = digest(&ws);
        assert_eq!(
            digest_apps(&d),
            dep("missing"),
            ".git as file: {git_as_file}: the loader read a cache above the workspace's own .git: {d}"
        );
        let f0 = fp(&fingerprint(&ws));
        write_dependency(&app, 1);
        let f1 = fp(&fingerprint(&ws));
        assert_eq!(f0, f1, ".git as file: {git_as_file}");
    }
}

/// The 2f/2h tree: a workspace whose own `.alpackages` holds a readable `a.app`
/// and a DIRECTORY named `broken.app` (discovered as a package; `fs::read`
/// fails, a real `read-failed`).
fn unreadable_tree(root: &Path, with_broken: bool) -> std::path::PathBuf {
    mark_repo_root(root, false);
    let ws = root.join("ws");
    write_workspace(&ws, "");
    write_dependency(&ws.join(".alpackages").join("a.app"), 0);
    if with_broken {
        std::fs::create_dir_all(ws.join(".alpackages").join("broken.app")).unwrap();
    }
    ws
}

fn has_unreadable_input(doc: &Value) -> bool {
    inputs(doc).contains(&(
        "dep-package-unreadable".to_string(),
        ".alpackages/broken.app".to_string(),
    ))
}

/// Row 2f. Precondition: the fingerprint's inputs contain a
/// `dep-package-unreadable` input keyed `.alpackages/broken.app`. Then
/// rewriting `a.app` changes the fingerprint (a readable swap is seen while
/// another package is unreadable), and the same tree without `broken.app`
/// fingerprints differently (an unreadable package never fingerprints like
/// none).
///
/// Mutation: `dependency_closure_entries` returns early on the first error
/// (today's `?`) -> `a.app`'s entry vanishes and the swap is not seen.
#[test]
fn fingerprint_tracks_a_swap_beside_an_unreadable_package() {
    let dir = tempfile::tempdir().unwrap();
    let ws = unreadable_tree(dir.path(), true);
    let f0 = fingerprint(&ws);
    assert!(
        has_unreadable_input(&f0),
        "precondition: a dep-package-unreadable input for .alpackages/broken.app: {:?}",
        inputs(&f0)
    );
    write_dependency(&ws.join(".alpackages").join("a.app"), 1);
    let f1 = fingerprint(&ws);
    assert_ne!(fp(&f0), fp(&f1), "a.app swap beside broken.app not seen");

    let clean_dir = tempfile::tempdir().unwrap();
    let clean = unreadable_tree(clean_dir.path(), false);
    write_dependency(&clean.join(".alpackages").join("a.app"), 1);
    assert_ne!(
        fp(&fingerprint(&clean)),
        fp(&f1),
        "an unreadable package fingerprints like no package"
    );
}

/// Row 2h. The 2f tree built in two different temp directories fingerprints
/// equal (the unreadable input hashes its category, not the error message,
/// which names the absolute path). Precondition as in 2f.
///
/// Mutation: hash the error message instead of the category -> differ.
#[test]
fn unreadable_package_fingerprint_is_location_independent() {
    let a_dir = tempfile::tempdir().unwrap();
    let b_dir = tempfile::tempdir().unwrap();
    let a = fingerprint(&unreadable_tree(&a_dir.path().join("one"), true));
    let b = fingerprint(&unreadable_tree(&b_dir.path().join("two/deeper"), true));
    assert!(
        has_unreadable_input(&a) && has_unreadable_input(&b),
        "precondition: a dep-package-unreadable input in both: {:?} / {:?}",
        inputs(&a),
        inputs(&b)
    );
    assert_eq!(fp(&a), fp(&b));
}

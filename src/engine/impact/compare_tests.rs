//! The comparison tuple over a scratch git repository built here: every
//! commit, branch and working-tree layer is made by the test.

use std::path::Path;

use super::*;

/// `git <args>` in the test's own repository: never a hook's `GIT_DIR`, never
/// the machine's config ([`crate::git_env::scratch_git_command`]).
fn sh(dir: &Path, args: &[&str]) -> String {
    let out = crate::git_env::scratch_git_command()
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .expect("git runs");
    assert!(out.status.success(), "git {args:?}: {out:?}");
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

fn write(dir: &Path, rel: &str, text: &str) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

/// A repository: `A` on main, `other` branches from `A` with its own commit
/// `B`, main moves on to `C`; then a staged edit, an unstaged edit and an
/// untracked `.al` file (and an untracked non-AL file) in the workspace
/// folder `ws/`.
fn repo() -> (tempfile::TempDir, String, String) {
    let t = tempfile::tempdir().unwrap();
    let d = t.path();
    sh(d, &["init", "-q", "-b", "main"]);
    write(d, "ws/src/A.Codeunit.al", "codeunit 50100 A\n{\n}\n");
    write(d, "ws/src/S.Codeunit.al", "codeunit 50101 S\n{\n}\n");
    write(d, "ws/src/U.Codeunit.al", "codeunit 50102 U\n{\n}\n");
    sh(d, &["add", "."]);
    sh(d, &["commit", "-q", "-m", "A"]);
    let a = sh(d, &["rev-parse", "HEAD"]);
    sh(d, &["checkout", "-q", "-b", "other"]);
    write(d, "ws/src/B.Codeunit.al", "codeunit 50103 B\n{\n}\n");
    sh(d, &["add", "."]);
    sh(d, &["commit", "-q", "-m", "B"]);
    let b = sh(d, &["rev-parse", "HEAD"]);
    sh(d, &["checkout", "-q", "main"]);
    write(
        d,
        "ws/src/A.Codeunit.al",
        "codeunit 50100 A\n{\n    // C\n}\n",
    );
    sh(d, &["commit", "-q", "-am", "C"]);
    write(
        d,
        "ws/src/S.Codeunit.al",
        "codeunit 50101 S\n{\n    // staged\n}\n",
    );
    sh(d, &["add", "ws/src/S.Codeunit.al"]);
    write(
        d,
        "ws/src/U.Codeunit.al",
        "codeunit 50102 U\n{\n    // unstaged\n}\n",
    );
    write(d, "ws/src/New.Codeunit.al", "codeunit 50104 New\n{\n}\n");
    write(d, "ws/notes.txt", "not AL\n");
    (t, a, b)
}

/// B-3: `--base other` alone resolves the comparison base to the MERGE BASE
/// of `other` and HEAD (`A`), not `other` itself (`B`); the change holds the
/// committed, staged and unstaged edits, each layer listed, and the
/// untracked `.al` file, listed and read as a new file.
///
/// DISCRIMINATION (2026-11-05): diffing against the requested commit itself
/// (`let base = resolved.clone()` after the merge-base call, so the tuple
/// and `git diff` both use `B`) fails at the merge-base assertion (`B`
/// found, `A` wanted); restored, passes.
#[test]
fn base_alone_compares_against_the_merge_base_with_every_layer() {
    let (t, a, b) = repo();
    let ws = t.path().join("ws");
    let (c, text) = for_base(&ws, "other").expect("resolves");
    assert_eq!(c.resolved_commit.as_deref(), Some(b.as_str()));
    assert_eq!(c.merge_base.as_deref(), Some(a.as_str()));
    assert_eq!(
        c.comparison_base.as_deref(),
        Some(a.as_str()),
        "comparison base is the merge base"
    );
    assert_eq!(c.committed, vec!["ws/src/A.Codeunit.al".to_string()]);
    assert_eq!(c.staged, vec!["ws/src/S.Codeunit.al".to_string()]);
    assert_eq!(c.unstaged, vec!["ws/src/U.Codeunit.al".to_string()]);
    assert_eq!(c.untracked, vec!["ws/src/New.Codeunit.al".to_string()]);
    // Every layer is in the diff; `B` (only on `other`) is not.
    for want in [
        "+++ b/ws/src/A.Codeunit.al",
        "+++ b/ws/src/S.Codeunit.al",
        "+++ b/ws/src/U.Codeunit.al",
        "+++ b/ws/src/New.Codeunit.al",
        "+codeunit 50104 New",
    ] {
        assert!(text.contains(want), "{want} in\n{text}");
    }
    assert!(!text.contains("B.Codeunit.al"), "{text}");
    assert!(!text.contains("notes.txt"), "{text}");
    assert_eq!(c.diff_hash.as_deref(), Some(diff_hash(&text).as_str()));
    // The untracked section parses as an added file with its lines.
    let parsed = crate::engine::l5::diff_parser::parse_unified_diff(&text);
    let new = parsed
        .files
        .iter()
        .find(|f| f.path == "ws/src/New.Codeunit.al")
        .expect("the untracked file");
    assert_eq!(new.hunks[0].changes.len(), 3);
}

/// TLS-114 (sol r1): under a top level that is not UTF-8 (`repo-\xff`), the
/// untracked `.al` file is opened at the exact top git printed and read in.
///
/// DISCRIMINATION (2026-11-25): `top` read through the lossy `git` (the old
/// code) fails ("untracked file ... could not be read": `repo-\u{FFFD}`);
/// restored, passes. Linux only: macOS (APFS) refuses a non-UTF-8 name.
#[cfg(target_os = "linux")]
#[test]
fn an_untracked_file_is_read_under_a_non_utf8_top_level() {
    use std::os::unix::ffi::OsStrExt;
    let t = tempfile::tempdir().unwrap();
    let d = t.path().join(std::ffi::OsStr::from_bytes(b"repo-\xff"));
    std::fs::create_dir_all(&d).unwrap();
    sh(&d, &["init", "-q", "-b", "main"]);
    write(&d, "ws/src/A.Codeunit.al", "codeunit 50100 A\n{\n}\n");
    sh(&d, &["add", "."]);
    sh(&d, &["commit", "-q", "-m", "A"]);
    write(&d, "ws/src/New.Codeunit.al", "codeunit 50104 New\n{\n}\n");
    let (c, text) = for_base(&d.join("ws"), "HEAD").expect("resolves");
    assert_eq!(c.untracked, vec!["ws/src/New.Codeunit.al".to_string()]);
    assert!(text.contains("+codeunit 50104 New"), "{text}");
}

/// TLS-114 (sol r1 follow-up): an untracked `.al` file whose name git would
/// quote (`é`) is listed by its name and read in; on Linux one whose name is
/// not UTF-8 (`\xff`) is read too.
///
/// DISCRIMINATION (2026-11-25): the list read without `-z`, split at LF,
/// fails (`untracked` empty: the quoted `"ws/src/N\303\251.Codeunit.al"` does
/// not end in `.al`, so the file was silently left out); opening the lossy
/// name instead of `path_from_git(raw)` fails at the `\xff` file ("could not
/// be read"); restored, passes.
#[test]
fn untracked_files_are_read_by_their_exact_names() {
    let t = tempfile::tempdir().unwrap();
    let d = t.path();
    sh(d, &["init", "-q", "-b", "main"]);
    write(d, "ws/src/A.Codeunit.al", "codeunit 50100 A\n{\n}\n");
    sh(d, &["add", "."]);
    sh(d, &["commit", "-q", "-m", "A"]);
    write(d, "ws/src/Né.Codeunit.al", "codeunit 50104 Ne\n{\n}\n");
    let mut want = vec!["ws/src/Né.Codeunit.al".to_string()];
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::ffi::OsStrExt;
        let p = d.join(std::ffi::OsStr::from_bytes(b"ws/src/X\xff.Codeunit.al"));
        std::fs::write(p, "codeunit 50105 Xff\n{\n}\n").unwrap();
        want.push("ws/src/X\u{FFFD}.Codeunit.al".to_string());
    }
    let (c, text) = for_base(&d.join("ws"), "HEAD").expect("resolves");
    assert_eq!(c.untracked, want);
    assert!(text.contains("+codeunit 50104 Ne"), "{text}");
    #[cfg(target_os = "linux")]
    assert!(text.contains("+codeunit 50105 Xff"), "{text}");
}

/// A ref that names no commit is refused with its name.
#[test]
fn an_unknown_base_is_refused() {
    let (t, _, _) = repo();
    let e = for_base(&t.path().join("ws"), "no-such-ref").unwrap_err();
    assert!(e.contains("'no-such-ref' names no commit"), "{e}");
}

/// The tuple round-trips through its JSON (a part hands it on).
#[test]
fn the_tuple_round_trips() {
    let (t, _, _) = repo();
    let (c, _) = for_base(&t.path().join("ws"), "other").unwrap();
    assert_eq!(Comparison::from_json(&c.to_json()), c);
    let d = for_input("diff", &t.path().join("ws"), Some("x"));
    assert_eq!(Comparison::from_json(&d.to_json()), d);
    assert!(d.comparison_base.is_none());
}

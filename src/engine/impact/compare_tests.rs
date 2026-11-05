//! The comparison tuple over a scratch git repository built here: every
//! commit, branch and working-tree layer is made by the test.

use std::path::Path;
use std::process::Command;

use super::*;

fn sh(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "-c",
            "commit.gpgsign=false",
        ])
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

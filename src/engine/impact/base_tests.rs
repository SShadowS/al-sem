//! `base.rs`: the reversed diff and the dependency declaration compare.

use super::*;

/// The reverse swaps paths, ranges and `+`/`-` lines; reversing twice gives
/// the text back.
#[test]
fn reverse_diff_swaps_sides_and_round_trips() {
    let d = "diff --git a/X.al b/X.al\nindex 111..222 100644\n--- a/X.al\n+++ b/X.al\n@@ -1,3 +1,4 @@ codeunit\n a\n-b\n+c\n+d\n e\n";
    let r = reverse_diff(d);
    assert_eq!(
        r,
        "diff --git a/X.al b/X.al\nindex 222..111 100644\n--- a/X.al\n+++ b/X.al\n@@ -1,4 +1,3 @@ codeunit\n a\n+b\n-c\n-d\n e\n"
    );
    assert_eq!(reverse_diff(&r), d);
}

/// A new file becomes a deleted one (and `/dev/null` moves side).
#[test]
fn reverse_diff_turns_an_added_file_into_a_deleted_one() {
    let d = "diff --git a/N.al b/N.al\nnew file mode 100644\n--- /dev/null\n+++ b/N.al\n@@ -0,0 +1,2 @@\n+x\n+y\n";
    let r = reverse_diff(d);
    assert_eq!(
        r,
        "diff --git a/N.al b/N.al\ndeleted file mode 100644\n--- a/N.al\n+++ /dev/null\n@@ -1,2 +0,0 @@\n-x\n-y\n"
    );
    let parsed = crate::engine::l5::diff_parser::parse_unified_diff(&r);
    assert_eq!(parsed.files.len(), 1);
    assert_eq!(
        parsed.files[0].kind,
        crate::engine::l5::diff_parser::DiffFileKind::Deleted
    );
}

/// A rename keeps `rename from` before `rename to`.
#[test]
fn reverse_diff_keeps_rename_order() {
    let d = "diff --git a/A.al b/B.al\nsimilarity index 100%\nrename from A.al\nrename to B.al\n";
    let r = reverse_diff(d);
    assert!(r.contains("rename from B.al\nrename to A.al\n"), "{r}");
}

/// Added, removed and re-versioned dependencies are named; equal ones are not.
#[test]
fn dependency_changes_names_each_difference() {
    let base = serde_json::json!([
        {"id": "AAA", "name": "Base", "version": "1.0.0.0"},
        {"id": "bbb", "name": "Old", "version": "2.0.0.0"},
        {"id": "ccc", "name": "Same", "version": "3.0.0.0"},
    ]);
    let new = serde_json::json!([
        {"id": "aaa", "name": "Base", "version": "1.1.0.0"},
        {"id": "ccc", "name": "Same", "version": "3.0.0.0"},
        {"id": "ddd", "name": "New", "version": "1.0.0.0"},
    ]);
    assert_eq!(
        dependency_changes(&base, &new),
        vec![
            "Base 1.0.0.0 → Base 1.1.0.0".to_string(),
            "removed Old 2.0.0.0".to_string(),
            "added New 1.0.0.0".to_string(),
        ]
    );
    assert!(dependency_changes(&base, &base).is_empty());
}

/// TLS-114: a base is materialized from a repository whose top level ends in
/// CR, ends in LF, or (Linux) is not UTF-8: `git archive` runs in the top
/// `rev-parse --show-toplevel` printed, bytes kept, less its final LF only.
///
/// DISCRIMINATION (2026-11-23): `git_bytes` trimming all trailing whitespace
/// (the old `trim_end`) fails at `repo\r` (git archive runs in `…/repo`,
/// which does not exist); popping every trailing LF fails at `repo\n`;
/// `git_path` decoding with `from_utf8_lossy` fails at `repo-\xff`
/// (`…/repo-\u{FFFD}`); restored, passes.
#[cfg(unix)]
#[test]
fn a_base_is_materialized_under_a_top_level_git_prints_exactly() {
    use std::os::unix::ffi::OsStrExt;
    let mut names: Vec<&[u8]> = vec![b"repo\r", b"repo\n"];
    if cfg!(target_os = "linux") {
        // macOS (APFS) refuses a non-UTF-8 name.
        names.push(b"repo-\xff");
    }
    for name in names {
        let t = tempfile::tempdir().unwrap();
        let r = t.path().join(std::ffi::OsStr::from_bytes(name));
        let sh = |args: &[&str]| {
            let ok = crate::git_env::scratch_git_command()
                .arg("-C")
                .arg(&r)
                .args(args)
                .status()
                .is_ok_and(|s| s.success());
            assert!(ok, "git {args:?} failed");
        };
        std::fs::create_dir_all(r.join("ws")).unwrap();
        std::fs::write(r.join("ws/app.json"), "{}").unwrap();
        sh(&["init", "-q"]);
        sh(&["add", "."]);
        sh(&["commit", "-q", "-m", "A"]);
        let m = materialize(&r.join("ws"), "HEAD", &t.path().join("out"), "ws")
            .unwrap_or_else(|e| panic!("{:?}: {e}", r));
        assert!(m.workspace.join("app.json").is_file(), "{m:?}");
    }
}

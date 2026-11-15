//! ⟨TLS-62⟩ `scripts/changelog-merge`: CHANGELOG.md's merge rule (keep both
//! sides' new entries, refuse anything else), shared by `scripts/land` and git
//! itself as the merge driver `.gitattributes` names. Every git repo here is a
//! test's own (`crate::git::run`); nothing touches the real checkout's config.

const SCRIPT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/changelog-merge");

/// The script's built-in cases: additions kept (ours first, a blank line before
/// the heading), a line both sides changed refused, CRLF refused, no conflict
/// merged as git does, and the driver's two exits.
///
/// Mutations (2026-11-15): the blank-line rule removed from the awk (`print ""`
/// dropped) -> case 1b fails, the self-test exits 1 (case 1 alone did not catch
/// it: in `--diff3` mode git keeps each side's trailing blank in the conflict);
/// the refusal of a changed line off (`state == 2 { next }`) -> case 2 fails
/// here, and `land_script::land_refuses_and_leaves_nothing_behind` fails too,
/// which shows land's CHANGELOG cases run through this script.
#[test]
fn changelog_merge_self_test_passes() {
    let out = std::process::Command::new("bash")
        .args([SCRIPT, "--self-test"])
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{text}");
    assert!(text.contains("self-test passed"), "{text}");
}

/// The repository's `.gitattributes` gives CHANGELOG.md the driver.
///
/// Mutation (2026-11-15): the `CHANGELOG.md merge=changelog` line removed ->
/// `merge: unspecified`.
#[test]
fn the_repository_names_the_changelog_driver() {
    let out = crate::git::reader(std::path::Path::new(env!("CARGO_MANIFEST_DIR")))
        .args(["check-attr", "merge", "CHANGELOG.md"])
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        "CHANGELOG.md: merge: changelog"
    );
}

const BASE: &str = "# Changelog\n\n## [Unreleased]\n\n### Fixed (A)\n\n- old\n";

fn entry(name: &str) -> String {
    format!(
        "# Changelog\n\n## [Unreleased]\n\n### Added ({name})\n\n- {name} entry\n\n### Fixed (A)\n\n- old\n"
    )
}

/// A repo whose `.gitattributes` names the driver, at `BASE`; branch `b`
/// writes `theirs`, master then writes `ours`. With `configured`, the driver
/// is set in the repo's own config, pointing at this checkout's script.
/// Returns (repo dir, `git merge b`'s success).
fn merge_case(ours: &str, theirs: &str, configured: bool) -> (tempfile::TempDir, bool) {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    let git = |args: &[&str]| crate::git::run(p, args);
    git(&["init", "-q", "-b", "master"]);
    std::fs::write(p.join(".gitattributes"), "CHANGELOG.md merge=changelog\n").unwrap();
    std::fs::write(p.join("CHANGELOG.md"), BASE).unwrap();
    git(&["add", "-A"]);
    git(&["commit", "-q", "-m", "base"]);
    git(&["checkout", "-q", "-b", "b"]);
    std::fs::write(p.join("CHANGELOG.md"), theirs).unwrap();
    git(&["commit", "-q", "-am", "theirs"]);
    git(&["checkout", "-q", "master"]);
    std::fs::write(p.join("CHANGELOG.md"), ours).unwrap();
    git(&["commit", "-q", "-am", "ours"]);
    if configured {
        git(&[
            "config",
            "merge.changelog.driver",
            &format!("bash {SCRIPT} --driver %O %A %B"),
        ]);
    }
    let ok = crate::git::scratch(p)
        .args(["merge", "-q", "--no-edit", "b"])
        .output()
        .unwrap()
        .status
        .success();
    (dir, ok)
}

/// git uses the driver for CHANGELOG.md when it is configured: two entries
/// added at one place merge (ours first, a blank line between); a line both
/// sides changed stays a conflict with git's usual markers; and without the
/// config the same additions conflict, as before this task.
///
/// Mutation (2026-11-15): the driver's refusal path without its `git
/// merge-file` fallback -> the refused case's CHANGELOG has no markers.
#[test]
fn git_merges_changelog_through_the_driver_only_when_configured() {
    let (dir, ok) = merge_case(&entry("M"), &entry("B"), true);
    assert!(ok, "additions merge through the driver");
    assert_eq!(
        std::fs::read_to_string(dir.path().join("CHANGELOG.md")).unwrap(),
        "# Changelog\n\n## [Unreleased]\n\n### Added (M)\n\n- M entry\n\n### Added (B)\n\n- B entry\n\n### Fixed (A)\n\n- old\n"
    );

    let changed = |who: &str| BASE.replace("- old", &format!("- old, {who}"));
    let (dir, ok) = merge_case(&changed("ours"), &changed("theirs"), true);
    assert!(!ok, "a line both sides changed is not merged");
    let text = std::fs::read_to_string(dir.path().join("CHANGELOG.md")).unwrap();
    assert!(
        text.contains("<<<<<<< ") && text.contains(">>>>>>> "),
        "the refused conflict keeps git's markers:\n{text}"
    );

    let (_dir, ok) = merge_case(&entry("M"), &entry("B"), false);
    assert!(
        !ok,
        "without the driver configured, git conflicts as before"
    );
}

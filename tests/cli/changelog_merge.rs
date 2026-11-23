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
    let out = crate::clean_env::command("bash")
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
    // The merge runs the driver script: a clean environment (TLS-115).
    let ok = crate::clean_env::clean_env(&mut crate::git::scratch(p))
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

/// A lane's rebase onto master goes through the driver too (review r1, Fable):
/// master's new entry and the lane's merge, master's first. But an entry master
/// already carries (the lane's earlier commit squash-merged there) next to the
/// lane's new one stops the rebase, rather than keeping the entry twice.
///
/// Mutation (2026-11-15): the awk's on-both-sides check removed -> the
/// squash-merged case rebases cleanly with `E` twice, red here (and in the
/// self-test's case 2b).
#[test]
fn a_rebase_merges_new_entries_and_stops_on_a_carried_one() {
    let rebase = |master_text: &str, lane_text: &str| {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        let git = |args: &[&str]| crate::git::run(p, args);
        git(&["init", "-q", "-b", "master"]);
        git(&[
            "config",
            "merge.changelog.driver",
            &format!("bash {SCRIPT} --driver %O %A %B"),
        ]);
        std::fs::write(p.join(".gitattributes"), "CHANGELOG.md merge=changelog\n").unwrap();
        std::fs::write(p.join("CHANGELOG.md"), BASE).unwrap();
        git(&["add", "-A"]);
        git(&["commit", "-q", "-m", "base"]);
        git(&["checkout", "-q", "-b", "lane"]);
        std::fs::write(p.join("CHANGELOG.md"), lane_text).unwrap();
        git(&["commit", "-q", "-am", "lane"]);
        git(&["checkout", "-q", "master"]);
        std::fs::write(p.join("CHANGELOG.md"), master_text).unwrap();
        git(&["commit", "-q", "-am", "master"]);
        git(&["checkout", "-q", "lane"]);
        let ok = crate::clean_env::clean_env(&mut crate::git::scratch(p))
            .args(["rebase", "-q", "master"])
            .output()
            .unwrap()
            .status
            .success();
        let text = std::fs::read_to_string(p.join("CHANGELOG.md")).unwrap();
        (dir, ok, text)
    };

    let (_d, ok, text) = rebase(&entry("M"), &entry("L"));
    assert!(ok, "two new entries rebase cleanly:\n{text}");
    assert!(
        text.find("### Added (M)").unwrap() < text.find("### Added (L)").unwrap(),
        "master's entry first:\n{text}"
    );

    // Master holds E (the lane's earlier work, squash-merged) and M; the lane
    // still has E as a commit of its own plus N.
    let with = |names: &[&str]| {
        let mut s = String::from("# Changelog\n\n## [Unreleased]\n\n");
        for n in names {
            s.push_str(&format!("### Added ({n})\n\n- {n} entry\n\n"));
        }
        s + "### Fixed (A)\n\n- old\n"
    };
    let (_d, ok, text) = rebase(&with(&["M", "E"]), &with(&["E", "N"]));
    assert!(
        !ok,
        "an entry both sides carry must stop the rebase, not be kept twice:\n{text}"
    );
    assert!(text.contains("<<<<<<< "), "git's conflict markers:\n{text}");
}

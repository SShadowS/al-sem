//! A test started from a git hook must not touch the committer's repository.
//!
//! Git exports `GIT_DIR`, `GIT_INDEX_FILE` and `GIT_WORK_TREE` to its hooks,
//! and they beat `git -C <dir>`. On 2026-11-05 the pre-commit hook ran the
//! suite with them set: the impact tests' scratch repositories became the
//! committer's own (two commits on the branch, the index replaced,
//! `core.bare=true` in the shared config). The hook now drops them, and
//! every git spawn goes through `al_sem::git_env` (`tests/common/git.rs`).
//!
//! The parent test re-runs THIS test binary on the child test with the three
//! variables pointing at a sentinel repository, exactly as a hook would hand
//! them down, and checks the sentinel afterwards: same HEAD, same index bytes,
//! same config bytes, same commit count.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Set only in the child process the parent starts.
const CHILD_FLAG: &str = "AL_SEM_GIT_ENV_ISOLATION_CHILD";
const CHILD_TEST: &str = "cli_git_env_isolation::child_builds_its_own_repository";

fn write(dir: &Path, rel: &str, text: &str) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

fn stdout(out: &std::process::Output) -> String {
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// What the parent compares before and after.
#[derive(Debug, PartialEq)]
struct SentinelState {
    head: String,
    commits: String,
    /// blake3 of `.git/index`.
    index: String,
    /// `.git/config`, verbatim.
    config: String,
}

fn sentinel_state(dir: &Path) -> SentinelState {
    SentinelState {
        head: stdout(&crate::git::run(dir, &["rev-parse", "HEAD"])),
        commits: stdout(&crate::git::run(dir, &["rev-list", "--count", "--all"])),
        index: blake3::hash(&std::fs::read(dir.join(".git/index")).unwrap())
            .to_hex()
            .to_string(),
        config: std::fs::read_to_string(dir.join(".git/config")).unwrap(),
    }
}

/// The scratch-repository steps the impact tests run (`init`, a `config`
/// write, `add`, `commit`), plus the product's own git read
/// (`workspace_git_info`), in a folder of the child's own. A no-op unless the
/// parent started it.
#[test]
fn child_builds_its_own_repository() {
    if std::env::var_os(CHILD_FLAG).is_none() {
        return;
    }
    assert!(
        std::env::var_os("GIT_DIR").is_some(),
        "the parent hands the child a hook's GIT_DIR"
    );
    let t = tempfile::tempdir().unwrap();
    let own = t.path();
    write(own, "app.json", "{}\n");
    write(own, "src/Core.al", "codeunit 50100 Core { }\n");
    crate::git::run(own, &["init", "-q"]);
    // A config write before anything can fail, so a leak always shows in the
    // sentinel's config bytes.
    crate::git::run(own, &["config", "al-sem.probe", "child"]);
    crate::git::run(own, &["add", "."]);
    crate::git::run(own, &["commit", "-qm", "fixture"]);

    let top = stdout(&crate::git::run(own, &["rev-parse", "--show-toplevel"]));
    assert_eq!(
        std::fs::canonicalize(&top).unwrap(),
        std::fs::canonicalize(own).unwrap(),
        "git answered for another repository"
    );
    let head = stdout(&crate::git::run(own, &["rev-parse", "HEAD"]));
    let (sha, _) = al_sem::program::resolve::semantic_golden::workspace_git_info(own);
    assert_eq!(
        sha.as_deref(),
        Some(head.as_str()),
        "the product read another repository"
    );
}

#[test]
fn scratch_git_ignores_a_hooks_repository_variables() {
    if std::env::var_os(CHILD_FLAG).is_some() {
        return;
    }
    let s = tempfile::tempdir().unwrap();
    let sentinel: PathBuf = s.path().to_path_buf();
    write(&sentinel, "keep.txt", "the committer's work\n");
    crate::git::run(&sentinel, &["init", "-q"]);
    crate::git::run(&sentinel, &["add", "."]);
    crate::git::run(&sentinel, &["commit", "-qm", "sentinel"]);
    let before = sentinel_state(&sentinel);

    let git_dir = sentinel.join(".git");
    let out = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", CHILD_TEST, "--nocapture", "--test-threads=1"])
        .env(CHILD_FLAG, "1")
        .env("GIT_DIR", &git_dir)
        .env("GIT_INDEX_FILE", git_dir.join("index"))
        .env("GIT_WORK_TREE", &sentinel)
        .env("GIT_PREFIX", "")
        .output()
        .expect("the test binary runs");
    let text = format!(
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let after = sentinel_state(&sentinel);
    assert_eq!(
        before, after,
        "a test's git reached the sentinel repository:\n{text}"
    );
    // The filter must have matched the child: a run of zero tests passes too.
    assert!(text.contains("1 passed"), "the child did not run:\n{text}");
    assert!(out.status.success(), "the child failed:\n{text}");
}

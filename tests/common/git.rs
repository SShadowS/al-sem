//! The integration tests' one way to run `git` (CLAUDE.md "Testing").
//!
//! Every helper starts from `al_sem::git_env`, which drops the repository
//! variables (`GIT_DIR`, `GIT_INDEX_FILE`, `GIT_WORK_TREE`, ...) a git hook
//! exports. Without that, a test started from the pre-commit hook ran
//! `git -C <tmp> commit` against the COMMITTER's repository (2026-11-05: it
//! committed onto the branch, replaced the index and set `core.bare=true`).
//! `cli_git_env_isolation` pins it.
#![allow(dead_code)]

use std::path::Path;
use std::process::{Command, Output};

/// `git -C <dir>` for a repository the test made itself: no hook variable,
/// no system or user config, a fixed identity given with `-c`.
pub fn scratch(dir: &Path) -> Command {
    let mut cmd = al_sem::git_env::scratch_git_command();
    cmd.arg("-C").arg(dir);
    cmd
}

/// `git -C <dir>` for a repository the test only READS and did not make
/// (the CDO baseline): no hook variable, the machine's config as usual.
pub fn reader(dir: &Path) -> Command {
    let mut cmd = al_sem::git_env::git_command();
    cmd.arg("-C").arg(dir);
    cmd
}

/// Run `git <args>` in the test's own repository `dir`; panic on failure.
pub fn run(dir: &Path, args: &[&str]) -> Output {
    let out = scratch(dir).args(args).output().expect("git runs");
    assert!(
        out.status.success(),
        "git {args:?} in {}: {}",
        dir.display(),
        String::from_utf8_lossy(&out.stderr)
    );
    out
}

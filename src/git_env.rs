//! **Every `git` this crate (and its tests) spawns starts here.**
//!
//! Git exports variables such as `GIT_DIR`, `GIT_INDEX_FILE` and
//! `GIT_WORK_TREE` to the hooks it runs, and they are inherited by every
//! process a hook starts. Those variables beat `git -C <dir>`: with `GIT_DIR`
//! set, `git -C <tmp> commit` writes into the repository `GIT_DIR` names, not
//! into `<tmp>`. On 2026-11-05 the pre-commit hook ran the test suite with
//! them set, and the tests that build a scratch repository committed
//! "fixture"/"base" onto the committer's branch, replaced its index and set
//! `core.bare=true` in its shared config, which broke the main checkout.
//!
//! - [`git_command`]: `git` with every repository-local variable removed, so
//!   `-C <dir>` (or the current folder) alone picks the repository. Used by
//!   the product (`alsem impact`, the golden stamps), which always names the
//!   folder it means.
//! - [`scratch_git_command`]: [`git_command`] plus isolation from the user's
//!   and the system's config and a fixed identity, for tests that build and
//!   commit to a repository of their own. `tests/common/git.rs` wraps it for
//!   the integration tests.
//!
//! The hook removes the same variables before it starts anything
//! (`scripts/git-hooks/pre-commit`); this module is the second layer, so a
//! test is safe however it is started.

use std::process::Command;

/// The repository-local variables git sets or honours: `git rev-parse
/// --local-env-vars` (git 2.56), plus `GIT_INTERNAL_SUPER_PREFIX`, which older
/// versions still list. The `local_env_vars_cover_this_git` test checks the
/// installed git lists nothing this misses.
pub const LOCAL_ENV_VARS: &[&str] = &[
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_CONFIG",
    "GIT_CONFIG_PARAMETERS",
    "GIT_CONFIG_COUNT",
    "GIT_OBJECT_DIRECTORY",
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_IMPLICIT_WORK_TREE",
    "GIT_GRAFT_FILE",
    "GIT_INDEX_FILE",
    "GIT_NO_REPLACE_OBJECTS",
    "GIT_REPLACE_REF_BASE",
    "GIT_PREFIX",
    "GIT_SHALLOW_FILE",
    "GIT_COMMON_DIR",
    "GIT_INTERNAL_SUPER_PREFIX",
];

/// `git`, with every [`LOCAL_ENV_VARS`] entry removed from its environment.
#[must_use]
pub fn git_command() -> Command {
    let mut cmd = Command::new("git");
    for var in LOCAL_ENV_VARS {
        cmd.env_remove(var);
    }
    cmd
}

/// The empty config file a scratch `git` reads as its global config.
const NO_CONFIG: &str = if cfg!(windows) { "NUL" } else { "/dev/null" };

/// The identity variables a commit hands its hook (`git commit` exports
/// `GIT_AUTHOR_*`); they beat `user.name`, so a scratch commit drops them.
const IDENTITY_VARS: &[&str] = &[
    "GIT_AUTHOR_NAME",
    "GIT_AUTHOR_EMAIL",
    "GIT_AUTHOR_DATE",
    "GIT_COMMITTER_NAME",
    "GIT_COMMITTER_EMAIL",
    "GIT_COMMITTER_DATE",
];

/// [`git_command`] for a repository the caller made itself: no system or
/// user config is read (no `core.hooksPath`, signing or default-branch
/// surprise from the machine), and the identity is fixed with `-c`, so
/// nothing is ever written into a config file.
#[must_use]
pub fn scratch_git_command() -> Command {
    let mut cmd = git_command();
    for var in IDENTITY_VARS {
        cmd.env_remove(var);
    }
    cmd.env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", NO_CONFIG)
        .args([
            "-c",
            "user.name=al-sem test",
            "-c",
            "user.email=test@al-sem.invalid",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "tag.gpgsign=false",
            "-c",
            "init.defaultBranch=master",
        ]);
    cmd
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The variables removed by `cmd`.
    fn removed(cmd: &Command) -> Vec<String> {
        cmd.get_envs()
            .filter(|(_, v)| v.is_none())
            .map(|(k, _)| k.to_string_lossy().into_owned())
            .collect()
    }

    /// The installed git names no local variable that [`LOCAL_ENV_VARS`]
    /// misses, and both commands remove every one of them.
    #[test]
    fn local_env_vars_cover_this_git() {
        let out = git_command()
            .args(["rev-parse", "--local-env-vars"])
            .output()
            .expect("git runs");
        assert!(out.status.success(), "{out:?}");
        let listed = String::from_utf8(out.stdout).unwrap();
        let listed: Vec<&str> = listed.lines().filter(|l| !l.is_empty()).collect();
        assert!(listed.contains(&"GIT_DIR"), "{listed:?}");
        for cmd in [git_command(), scratch_git_command()] {
            let gone = removed(&cmd);
            for var in &listed {
                assert!(gone.iter().any(|g| g == var), "{var} is not removed");
            }
        }
    }
}

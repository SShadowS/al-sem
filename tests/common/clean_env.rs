//! ⟨TLS-113, TLS-115⟩ The integration tests' one way to start a repo script
//! (`scripts/cdo-gate`, `cdo-speed`, `cdo-slot`, `ci-steps`, `land`, ...).
//!
//! Every subprocess of these tests that runs a script, or bash at all (a
//! stand-in, a preflight check, a git command whose merge driver is a script),
//! starts from it. A child starts from an empty environment: none of the caller's land, slot,
//! gate, speed or test-hook settings (a landing runs this suite through
//! `ci-steps all` inside its own CDO slot hold; an inherited `CDO_SLOT_OWNER`
//! renamed the slot's holder, TLS-113 review r1, sol), no `BASH_ENV`, no
//! `ALSEM_*`, no git variables a hook exported. Only what a shell, git and the
//! toolchain need, then git's test identity; each test adds its own settings.
//! `gate_scripts::scripts_see_none_of_the_callers_environment` pins it.
//!
//! Proof (TLS-113, 2026-11-21): `env_clear()` removed and the land tests run
//! with `CDO_SLOT_OWNER=leaked` exported ->
//! `land_holds_the_cdo_slot_from_start_to_end` FAILED (the holder read
//! `leaked`); restored -> passed.
#![allow(dead_code)]

use std::process::Command;

/// Clear `cmd`'s environment down to the few variables every script needs.
///
/// Host dependencies kept on purpose (TLS-115 review r1, sol), each because a
/// script cannot run without it:
/// - `PATH`: where bash, python3, git, flock, taskset, perf and cargo are. A
///   test with stub tools puts its stub folder first and keeps the rest.
/// - `TMPDIR`: where `tempfile` and the scripts' `mktemp` put their files; the
///   test process made its own fixtures there already, so a child must agree.
/// - `HOME`: tools that read it (git with no global config, python) need some
///   value; a test with stub tools gives its own (TLS-115).
/// - `RUSTUP_HOME`, `CARGO_HOME`: where the toolchain lives (server1:
///   /opt/rustup, not under HOME); a script that runs cargo needs them.
///
/// The locale is pinned, never the caller's: `LANG=C.UTF-8` (land's
/// `[[:cntrl:]]` check and every script's text handling read it).
pub fn clean_env(cmd: &mut Command) -> &mut Command {
    cmd.env_clear();
    for k in ["PATH", "HOME", "TMPDIR", "RUSTUP_HOME", "CARGO_HOME"] {
        if let Some(v) = std::env::var_os(k) {
            cmd.env(k, v);
        }
    }
    cmd.env("LANG", "C.UTF-8")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
}

/// `Command::new(program)` with [`clean_env`] applied.
pub fn command(program: impl AsRef<std::ffi::OsStr>) -> Command {
    let mut cmd = Command::new(program);
    clean_env(&mut cmd);
    cmd
}

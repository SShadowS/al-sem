//! ⟨TLS-113, TLS-115⟩ The integration tests' one way to start a repo script
//! (`scripts/cdo-gate`, `cdo-speed`, `cdo-slot`, `ci-steps`, `land`, ...).
//!
//! A child starts from an empty environment: none of the caller's land, slot,
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
pub fn clean_env(cmd: &mut Command) -> &mut Command {
    cmd.env_clear();
    // RUSTUP_HOME and CARGO_HOME only say where the toolchain lives (server1:
    // /opt/rustup, not under HOME): a script that runs cargo needs them, and
    // neither changes what a gate decides.
    for k in [
        "PATH",
        "HOME",
        "TMPDIR",
        "LANG",
        "RUSTUP_HOME",
        "CARGO_HOME",
    ] {
        if let Some(v) = std::env::var_os(k) {
            cmd.env(k, v);
        }
    }
    cmd.env("GIT_CONFIG_GLOBAL", "/dev/null")
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

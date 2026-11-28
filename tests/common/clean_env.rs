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
    // ⟨TLS-160⟩ Never the tentacle's real slot (cdo-slot's default,
    // /work/.al-sem/cdo-slot): a test that forgets its own slot folder gets
    // this process's private throwaway one instead, with no memory floor (a
    // CI runner has ~7 GB; on CI such a test once found no slot at all). A
    // test's own CDO_SLOT_DIR or CDO_SLOT_MEM_MIN_GB, set after this, wins.
    cmd.env("CDO_SLOT_DIR", throwaway_slot())
        .env("CDO_SLOT_MEM_MIN_GB", "0")
        .env("LANG", "C.UTF-8")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
}

/// This test process's throwaway slot folder (TLS-160, sol r1): made once per
/// process, atomically (`tempfile`: a fresh, unpredictable name), private
/// (mode 0700, set explicitly) and absolute, so no earlier entry or link can
/// stand in its place;
/// removed when the process exits (unix: `atexit`). Every test of one
/// `cargo test` process shares it (nextest runs each test in its own).
pub fn throwaway_slot() -> std::path::PathBuf {
    static SLOT: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();
    SLOT.get_or_init(|| {
        let mut b = tempfile::Builder::new();
        b.prefix("alsem-test-slot.");
        // Private: tempfile's default here was 0755 (measured).
        #[cfg(unix)]
        b.permissions(std::os::unix::fs::PermissionsExt::from_mode(0o700));
        let dir = b.tempdir().expect("make the throwaway slot folder").keep();
        let dir = std::fs::canonicalize(&dir).unwrap_or(dir);
        #[cfg(unix)]
        {
            extern "C" fn remove() {
                if let Some(d) = SLOT.get() {
                    let _ = std::fs::remove_dir_all(d);
                }
            }
            // SAFETY: registers a plain `extern "C"` function with no state.
            unsafe { libc::atexit(remove) };
        }
        dir
    })
    .clone()
}

/// `Command::new(program)` with [`clean_env`] applied.
pub fn command(program: impl AsRef<std::ffi::OsStr>) -> Command {
    let mut cmd = Command::new(program);
    clean_env(&mut cmd);
    cmd
}

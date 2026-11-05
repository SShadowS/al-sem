//! Shared `REGEN_TEMP_GOLDENS` value-gate helper for golden-regenerating tests.
//!
//! Doctrine (CLAUDE.md, `al-sem retired / Rust-owned`): every Rust-owned golden
//! regenerates via `REGEN_TEMP_GOLDENS=1 cargo test`. Before Task T0.6, every
//! call site checked PRESENCE (`std::env::var("REGEN_TEMP_GOLDENS").is_ok()` /
//! `.is_err()`), so `REGEN_TEMP_GOLDENS=0 cargo test` silently REWROTE every
//! golden while reporting green — `is_ok()` is true for ANY set value,
//! including `"0"`. This helper checks the VALUE: only the exact string `"1"`
//! regenerates. `"0"`, the empty string, and anything else fall through to the
//! normal assert path — fail-closed toward asserting, never toward silently
//! rewriting goldens.
//!
//! `cargo test` compiles each `tests/*.rs` file as its own separate
//! binary/crate, so a `mod` defined in one cannot be `use`d from another.
//! This file is included via `#[path = "common/regen.rs"] mod regen;` by
//! every golden-asserting test binary (Task T0.6) so there is exactly one
//! implementation. (The single `#[cfg(test)]` unit-test regen check that
//! lives inside the library itself, in `src/parser.rs`, cannot reach across
//! the `src/`/`tests/` boundary via `#[path]` without an awkward relative
//! reference into `tests/`; it mirrors this same value-semantics contract
//! with its own tiny colocated helper instead — see the comment there.)

/// Pure resolution core: given the raw env-var value (if set), decide whether
/// regen mode is active. Only the exact value `"1"` regenerates.
fn resolve_regen_mode(raw: Option<&str>) -> bool {
    raw == Some("1")
}

/// Returns `true` iff `REGEN_TEMP_GOLDENS=1` — the ONLY value that
/// regenerates goldens. Any other value (including `"0"` or an empty string)
/// returns `false`, taking the normal assert path.
///
/// Use this instead of `std::env::var("REGEN_TEMP_GOLDENS").is_ok()` /
/// `.is_err()`, which are presence-based and wrongly treat
/// `REGEN_TEMP_GOLDENS=0` as "regenerate."
#[allow(dead_code)] // not every including binary calls both helpers below
pub fn regen_mode() -> bool {
    resolve_regen_mode(std::env::var("REGEN_TEMP_GOLDENS").ok().as_deref())
}

/// Write `contents` to a committed golden or fixture file at `path` — the ONE way a
/// test may write into the repository tree.
///
/// Under `cargo-nextest` every test runs in its own process, all at once, so while
/// one test regenerates a file another may be reading it. `std::fs::write` truncates
/// the file first and fills it after, so a reader can see an EMPTY or HALF-WRITTEN
/// file. That happened: the MCP-output fixture's dependency `.app` was rewritten in
/// place during a regen run, a concurrent test loaded it as symbol-only, and a wrong
/// `explain` golden was written from that read. This helper closes both gaps:
///
/// - when the file already holds exactly `contents`, nothing is written at all (the
///   common case of a regen that changes nothing);
/// - otherwise the bytes go to a temporary file in the SAME directory, which is then
///   renamed over `path`. A rename within one file system is atomic, so a reader
///   sees either the old file whole or the new file whole, never a partial one.
///
/// The new file keeps the old file's permissions (a fresh one gets `0o644` on Unix;
/// a temporary file is created `0o600`).
#[allow(dead_code)] // not every including binary writes into the tree
pub fn write_golden(
    path: impl AsRef<std::path::Path>,
    contents: impl AsRef<[u8]>,
) -> std::io::Result<()> {
    use std::io::Write;
    let (path, contents) = (path.as_ref(), contents.as_ref());
    if std::fs::read(path).is_ok_and(|old| old == contents) {
        return Ok(());
    }
    let dir = match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => std::path::Path::new("."),
    };
    let mut tmp = tempfile::Builder::new()
        .prefix(".regen-")
        .suffix(".tmp")
        .tempfile_in(dir)?;
    tmp.write_all(contents)?;
    tmp.as_file().sync_all()?;
    let perms = match std::fs::metadata(path) {
        Ok(m) => Some(m.permissions()),
        Err(_) => default_permissions(),
    };
    if let Some(perms) = perms {
        std::fs::set_permissions(tmp.path(), perms)?;
    }
    tmp.persist(path).map_err(|e| e.error)?;
    Ok(())
}

/// After a regen wrote a directory's complete golden set with [`write_golden`]:
/// delete the files directly in `dir` whose names are not in `keep`. Use this
/// instead of clearing the directory before the writes, which would leave it
/// missing or empty for a concurrent reader. Subdirectories are left alone.
#[allow(dead_code)]
pub fn remove_stale<'a>(dir: &std::path::Path, keep: impl IntoIterator<Item = &'a str>) {
    let keep: std::collections::BTreeSet<&str> = keep.into_iter().collect();
    let entries = std::fs::read_dir(dir).unwrap_or_else(|e| panic!("read {}: {e}", dir.display()));
    for entry in entries {
        let entry = entry.unwrap_or_else(|e| panic!("read {}: {e}", dir.display()));
        let is_file = entry.file_type().is_ok_and(|t| t.is_file());
        let name = entry.file_name();
        // A `.regen-*` file is another writer's temp file, about to be renamed.
        let in_flight = name.to_str().is_some_and(|n| n.starts_with(".regen-"));
        if is_file && !in_flight && !name.to_str().is_some_and(|n| keep.contains(n)) {
            std::fs::remove_file(entry.path())
                .unwrap_or_else(|e| panic!("remove stale {}: {e}", entry.path().display()));
        }
    }
}

#[cfg(unix)]
#[allow(dead_code)]
fn default_permissions() -> Option<std::fs::Permissions> {
    use std::os::unix::fs::PermissionsExt;
    Some(std::fs::Permissions::from_mode(0o644))
}

#[cfg(not(unix))]
#[allow(dead_code)]
fn default_permissions() -> Option<std::fs::Permissions> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- pure-helper tests (no env state, never race) ---
    //
    // No test in this module mutates `REGEN_TEMP_GOLDENS`: this file is
    // `#[path]`-included into ~40 golden-asserting test binaries, and a
    // mutating test here would run concurrently with those binaries' own
    // (unlocked) `regen_mode()` reads — racing a real golden gate into its
    // regen branch and silently rewriting a committed golden. The value
    // semantics of `regen_mode()` are a trivial composition over
    // `resolve_regen_mode`, so the pure tests below fully cover it without
    // touching the process environment.

    #[test]
    fn resolve_regen_mode_true_only_for_exact_one() {
        assert!(resolve_regen_mode(Some("1")));
    }

    #[test]
    fn resolve_regen_mode_false_for_zero() {
        assert!(!resolve_regen_mode(Some("0")));
    }

    #[test]
    fn resolve_regen_mode_false_for_empty_string() {
        assert!(!resolve_regen_mode(Some("")));
    }

    #[test]
    fn resolve_regen_mode_false_for_absent() {
        assert!(!resolve_regen_mode(None));
    }

    #[test]
    fn resolve_regen_mode_false_for_other_truthy_looking_values() {
        assert!(!resolve_regen_mode(Some("true")));
        assert!(!resolve_regen_mode(Some("yes")));
        assert!(!resolve_regen_mode(Some("2")));
        assert!(!resolve_regen_mode(Some(" 1")));
        assert!(!resolve_regen_mode(Some("1 ")));
    }
}

//! TLS-109: every file a batch run writes is claimed by the file it IS, at
//! the moment it is opened, not by the path the manifest spells. The preflight
//! ([`super::manifest::check_write_sets`]) refuses most clashes before
//! anything runs; this is the guarantee for the rest (a link or parent changed
//! after the preflight, a dangling link two writes share, a shard child).
//!
//! A run makes one private folder ([`set_dir`], handed to its workers in
//! [`ENV`]). A claim is a marker file named after the file's identity
//! ([`Marker`]), created with `create_new`, holding its owner. A writer opens
//! its file WITHOUT truncating it, reads the handle's identity, claims it, and
//! only then truncates. A marker held by another owner refuses the write:
//! nothing is truncated, and the run fails ([`any_refused`]). The preflight claims
//! the files the run must not write (inputs, a refused job's named files)
//! before any job runs. Outside a batch (no folder) a write truncates as it
//! always did.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// The environment variable that hands the run's folder to its workers.
pub const ENV: &str = "ALSEM_BATCH_CLAIMS";

/// The owner of a protected input: no writer is ever its owner.
pub const READ: &str = "read";

/// A refused claim leaves this file in the run's folder, so the parent sees
/// a worker's refusal too.
const REFUSED: &str = "refused";

static DIR: OnceLock<PathBuf> = OnceLock::new();

/// This process's claims go to `dir` from now on.
pub fn set_dir(dir: PathBuf) {
    let _ = DIR.set(dir);
}

/// What a marker is named: the file's device and inode on unix. Elsewhere
/// (Windows) a hash of its canonical path: the claim is then the path
/// comparison of before TLS-109, and a hard link is not caught there.
pub type Marker = String;

#[cfg(unix)]
pub fn marker_of(id: &super::manifest::FileId) -> Marker {
    format!("{}-{}", id.0, id.1)
}

#[cfg(not(unix))]
pub fn marker_of(id: &super::manifest::FileId) -> Marker {
    use std::hash::{Hash, Hasher};
    let mut h = std::hash::DefaultHasher::new();
    id.hash(&mut h);
    format!("{:016x}", h.finish())
}

#[cfg(unix)]
fn handle_marker(f: &File, _path: &Path) -> std::io::Result<Marker> {
    use std::os::unix::fs::MetadataExt;
    let m = f.metadata()?;
    Ok(marker_of(&(m.dev(), m.ino())))
}

#[cfg(not(unix))]
fn handle_marker(_f: &File, path: &Path) -> std::io::Result<Marker> {
    Ok(marker_of(&std::fs::canonicalize(path)?))
}

/// Claim `marker` for `owner`. `Ok(false)`: another owner holds it.
fn claim(dir: &Path, marker: &str, owner: &str) -> std::io::Result<bool> {
    let path = dir.join(marker);
    match OpenOptions::new().write(true).create_new(true).open(&path) {
        Ok(mut m) => m.write_all(owner.as_bytes()).map(|()| true),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            Ok(std::fs::read_to_string(&path)? == owner)
        }
        Err(e) => Err(e),
    }
}

/// The preflight's protection of an existing file: the first owner stays
/// (two readers of one file both read). Nothing happens outside a batch.
pub fn protect(marker: &str, owner: &str) -> std::io::Result<()> {
    match DIR.get() {
        Some(dir) => claim(dir, marker, owner).map(|_| ()),
        None => Ok(()),
    }
}

/// [`protect`] the file at `path` when it exists.
pub fn protect_path(path: &Path, owner: &str) -> std::io::Result<()> {
    match File::open(path) {
        Ok(f) => protect(&handle_marker(&f, path)?, owner),
        Err(_) => Ok(()),
    }
}

/// The run's folder, when this process is in a batch run.
pub fn dir() -> Option<&'static Path> {
    DIR.get().map(PathBuf::as_path)
}

/// Open `path` for `owner`'s write: created when missing, claimed by its
/// identity, then truncated. A claim held by another owner is an error
/// naming why, and the file is left as it was.
pub fn open_for_write(path: &Path, owner: &str) -> std::io::Result<File> {
    open_claimed(path, owner, true)
}

/// [`open_for_write`] for a trace file: a refusal writes no trace but does
/// not fail the run.
pub fn open_trace(path: &Path, owner: &str) -> std::io::Result<File> {
    open_claimed(path, owner, false)
}

fn open_claimed(path: &Path, owner: &str, fails_run: bool) -> std::io::Result<File> {
    // Not truncated here: only once the claim holds.
    let f = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)?;
    if let Some(dir) = DIR.get()
        && !claim(dir, &handle_marker(&f, path)?, owner)?
    {
        if fails_run {
            let _ = File::create(dir.join(REFUSED));
        }
        return Err(std::io::Error::other(
            "refused: this file is another write or an input of this batch run (TLS-109)",
        ));
    }
    f.set_len(0)?;
    Ok(f)
}

/// Whether any claim of this run, in any of its processes, was refused.
pub fn any_refused() -> bool {
    DIR.get().is_some_and(|d| d.join(REFUSED).exists())
}

/// TLS-109 test seam, compiled into debug builds only: with
/// `ALSEM_TEST_AFTER_PREFLIGHT=<command>`, the run runs it (through `sh -c`)
/// after its preflight and before any job, so a test can change the file
/// system between the check and the opens.
#[cfg(debug_assertions)]
pub fn test_seam_after_preflight() {
    if let Ok(cmd) = std::env::var("ALSEM_TEST_AFTER_PREFLIGHT") {
        let _ = std::process::Command::new("sh").args(["-c", &cmd]).status();
    }
}

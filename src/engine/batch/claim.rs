//! TLS-109: every file a batch run writes is claimed by the file it IS, at
//! the moment it is opened, not by the path the manifest spells. The preflight
//! ([`super::manifest::check_write_sets`]) refuses most clashes before
//! anything runs; this is the guarantee for the rest (a link or parent changed
//! after the preflight, a dangling link two writes share, a shard child).
//!
//! A run makes one private folder ([`set_dir`], handed to its workers in
//! [`ENV`]; a worker refuses to run without it). A claim is a marker file
//! named after the file's identity ([`Marker`]), created with `create_new`. A
//! writer opens its file WITHOUT truncating it, reads the handle's identity,
//! claims it, and only then truncates. Every write is its own claim: an
//! existing marker refuses the open, whoever made it, with one exception:
//! the preflight's reservation of a baseline for the one `--update-baseline`
//! write that rewrites it ([`reserve`]), which that write takes once. A
//! refused open truncates nothing and fails the run ([`any_refused`]).
//!
//! The preflight claims the files the run must not write (what jobs read,
//! workspace inputs) before any job runs, and lists per refused job the
//! files it names ([`keep`]): its own streams may not write them, whoever
//! else may. Outside a batch (no folder) a write opens as it always did.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

/// The environment variable that hands the run's folder to its workers.
pub const ENV: &str = "ALSEM_BATCH_CLAIMS";

/// A refused claim leaves this file in the run's folder, so the parent sees
/// a worker's refusal too.
const REFUSED: &str = "refused";

static DIR: OnceLock<PathBuf> = OnceLock::new();

/// This process's claims go to `dir` from now on.
pub fn set_dir(dir: PathBuf) {
    let _ = DIR.set(dir);
}

/// The run's folder, when this process is in a batch run.
pub fn dir() -> Option<&'static Path> {
    DIR.get().map(PathBuf::as_path)
}

/// What a marker is named: the file's device and inode on unix. Elsewhere
/// (Windows) a hash of its canonical path, read after the open: the claim is
/// then the path comparison of before TLS-109, and neither a hard link nor a
/// path changed between the open and that read is caught there.
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

/// Create `marker` holding `text`. `Ok(false)`: it exists.
fn create(dir: &Path, marker: &str, text: &str) -> std::io::Result<bool> {
    match OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(dir.join(marker))
    {
        Ok(mut m) => m.write_all(text.as_bytes()).map(|()| true),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
        Err(e) => Err(e),
    }
}

/// The preflight's claim on an existing file no write may take. An existing
/// marker is fine (two jobs read one file). Nothing happens outside a batch.
pub fn protect(marker: &str) -> std::io::Result<()> {
    match DIR.get() {
        Some(dir) => create(dir, marker, "read").map(|_| ()),
        None => Ok(()),
    }
}

/// The preflight's claim on a baseline for the one write `key` names
/// ([`write_key`]): that write may take it once ([`open_for_write`]).
pub fn reserve(marker: &str, key: &str) -> std::io::Result<()> {
    match DIR.get() {
        Some(dir) => create(dir, marker, &format!("reserve:{key}")).map(|_| ()),
        None => Ok(()),
    }
}

/// The key of a job's file write: its name and the path as spelled.
pub fn write_key(job: &str, path: &Path) -> String {
    format!("{job}:{}", path.display())
}

/// The file listing `job`'s kept identities.
fn kept_file(dir: &Path, job: &str) -> PathBuf {
    dir.join(format!("kept-{}", job.replace(' ', "_")))
}

/// The preflight's list of the files a refused `job` names (TLS-110): its
/// own writes may not open one of them.
pub fn keep(job: &str, markers: &[Marker]) -> std::io::Result<()> {
    match DIR.get() {
        Some(dir) if !markers.is_empty() => std::fs::write(kept_file(dir, job), markers.join("\n")),
        _ => Ok(()),
    }
}

/// A unique token per claim, across the run's processes.
fn token() -> String {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    format!(
        "write:{}:{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

/// Open `path` for one write of `job`: created when missing, claimed by its
/// identity, then truncated. `key`: the write's [`write_key`] when it may
/// take a [`reserve`]d baseline (a job's file action), `None` for a stream.
/// A refused claim, or one that cannot be made, is an error naming why, the
/// file is left as it was, and the run fails.
pub fn open_for_write(path: &Path, job: &str, key: Option<&str>) -> std::io::Result<File> {
    let Some(dir) = DIR.get() else {
        // Outside a batch: exactly as before TLS-109.
        return File::create(path);
    };
    let f = open_kept(path)?;
    if f.metadata()?.is_file() {
        // The claim refused, or could not be made: the run fails.
        claim(dir, &f, path, Some(job), key).inspect_err(|_| {
            let _ = File::create(dir.join(REFUSED));
        })?;
        f.set_len(0)?;
    }
    Ok(f)
}

/// [`open_for_write`] for a trace file: a refusal writes no trace but does
/// not fail the run.
pub fn open_trace(path: &Path) -> std::io::Result<File> {
    let Some(dir) = DIR.get() else {
        return File::create(path);
    };
    let f = open_kept(path)?;
    if f.metadata()?.is_file() {
        claim(dir, &f, path, None, None)?;
        f.set_len(0)?;
    }
    Ok(f)
}

/// Open `path` to write, created when missing, NOT truncated: only once the
/// claim holds. A device, pipe or FIFO (`/dev/null`, `>(cat)`) is not a
/// regular file: it holds no content a truncation could lose and `set_len`
/// fails on it, so its writers neither claim nor truncate it, as before.
fn open_kept(path: &Path) -> std::io::Result<File> {
    OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
}

/// Claim the opened `f` for one write ([`open_for_write`]'s rules).
fn claim(
    dir: &Path,
    f: &File,
    path: &Path,
    job: Option<&str>,
    key: Option<&str>,
) -> std::io::Result<()> {
    let refused = |why: &str| Err(std::io::Error::other(format!("refused: {why} (TLS-109)")));
    let marker = handle_marker(f, path)?;
    if let Some(job) = job
        && let Ok(kept) = std::fs::read_to_string(kept_file(dir, job))
        && kept.lines().any(|m| m == marker)
    {
        return refused("this job is refused and names this file, which must stay as it is");
    }
    if !create(dir, &marker, &token())? {
        let held = std::fs::read_to_string(dir.join(&marker))?;
        let reserved = key.is_some_and(|k| held == format!("reserve:{k}"));
        if !(reserved && create(dir, &format!("{marker}.taken"), "")?) {
            return refused("this file is another write or an input of this batch run");
        }
    }
    Ok(())
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

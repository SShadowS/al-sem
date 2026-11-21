//! TLS-109: every file a batch run writes is claimed by the file it IS, at
//! the moment it is opened, not by the path the manifest spells.
//!
//! **What the claims protect.** The files a run reads (job inputs, workspace
//! inputs, its manifest), the files a refused job names, and each write's
//! own output, against every alias that exists when the run's check
//! ([`super::manifest::check_write_sets`]) runs (a link, a hard link, a
//! junction, a bind mount, a `..` spelling) and every alias the run's own
//! writes create (two dangling links that the first write resolves, a shard
//! file, a stream, a trace file). The scope is the run's own doing: a file
//! another process replaces during the run is that process's write, and
//! reads that claim their own handles throughout the engine would be a
//! separate design.
//!
//! **How.** A run makes one private folder ([`set_dir`], handed to its
//! workers in [`ENV`]; a worker refuses to run without it). A claim is a
//! marker file named after the file's identity ([`Marker`]: device and inode
//! on unix, volume serial number and file index on Windows, both read from
//! the opened handle), created with `create_new`. A writer opens its file
//! WITHOUT truncating it, reads the handle's identity, claims it, and only
//! then truncates. Every write is its own claim: an existing marker refuses
//! the open, whoever made it, with one exception: the check's reservation of
//! a baseline for the one `--update-baseline` write that rewrites it
//! ([`reserve`]), which that write takes once. A refused open, or a claim
//! that cannot be made, truncates nothing and fails the run: this process
//! records it ([`failed`]) and a worker exits [`FAILED_EXIT`].
//!
//! The check claims the files the run must not write (what jobs read,
//! workspace inputs) before any job runs, and lists per refused job the
//! files it names ([`keep`]): its own writes may not open them, whoever else
//! may. Outside a batch (no folder) a write opens as it always did.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// The environment variable that hands the run's folder to its workers.
pub const ENV: &str = "ALSEM_BATCH_CLAIMS";

/// A worker's exit when one of its claims failed, so its parent fails the
/// run without reading the claim folder (which may be what failed).
pub const FAILED_EXIT: u8 = 4;

static DIR: OnceLock<PathBuf> = OnceLock::new();

/// Set when a claim of this process was refused or could not be made.
static FAILED: AtomicBool = AtomicBool::new(false);

/// This process's claims go to `dir` from now on.
pub fn set_dir(dir: PathBuf) {
    let _ = DIR.set(dir);
}

/// The run's folder, when this process is in a batch run.
pub fn dir() -> Option<&'static Path> {
    DIR.get().map(PathBuf::as_path)
}

/// Whether a claim of this process was refused or could not be made.
pub fn failed() -> bool {
    FAILED.load(Ordering::Relaxed)
}

/// What a marker is named: a file identity as text, stable within the run.
pub type Marker = String;

pub fn marker_of(id: &super::manifest::FileId) -> Marker {
    format!("{}-{}", id.0, id.1)
}

#[cfg(unix)]
fn handle_marker(f: &File) -> std::io::Result<Marker> {
    use std::os::unix::fs::MetadataExt;
    let m = f.metadata()?;
    Ok(marker_of(&(m.dev(), m.ino())))
}

/// `GetFileInformationByHandle` on the opened handle.
#[cfg(windows)]
fn handle_marker(f: &File) -> std::io::Result<Marker> {
    let i = winapi_util::file::information(f)?;
    Ok(marker_of(&(i.volume_serial_number(), i.file_index())))
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

/// The check's claim on an existing file no write may take. An existing
/// marker is fine (two jobs read one file). Nothing happens outside a batch.
pub fn protect(marker: &str) -> std::io::Result<()> {
    match DIR.get() {
        Some(dir) => create(dir, marker, "read").map(|_| ()),
        None => Ok(()),
    }
}

/// The check's claim on a baseline for the one write `key` names
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

/// The check's list of the files a refused `job` names (TLS-110): its own
/// writes may not open one of them.
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
/// file is left as it was, and the run fails ([`failed`]).
pub fn open_for_write(path: &Path, job: &str, key: Option<&str>) -> std::io::Result<File> {
    let Some(dir) = DIR.get() else {
        // Outside a batch: exactly as before TLS-109.
        return File::create(path);
    };
    let f = open_kept(path)?;
    if f.metadata()?.is_file() {
        claim(dir, &f, Some(job), key).inspect_err(|_| FAILED.store(true, Ordering::Relaxed))?;
        f.set_len(0)?;
    }
    Ok(f)
}

/// [`open_for_write`] for a trace file, opened after every job's write: a
/// refusal writes no trace but does not fail the run.
pub fn open_trace(path: &Path) -> std::io::Result<File> {
    let Some(dir) = DIR.get() else {
        return File::create(path);
    };
    let f = open_kept(path)?;
    if f.metadata()?.is_file() {
        claim(dir, &f, None, None)?;
        f.set_len(0)?;
    }
    Ok(f)
}

/// Open `path` to write, created when missing, NOT truncated: only once the
/// claim holds. A device, pipe or FIFO (`/dev/null`, `NUL`, `>(cat)`) is not
/// a regular file: it holds no content a truncation could lose and `set_len`
/// fails on it, so its writers neither claim nor truncate it, as before.
fn open_kept(path: &Path) -> std::io::Result<File> {
    OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
}

/// Claim the opened `f` for one write ([`open_for_write`]'s rules).
fn claim(dir: &Path, f: &File, job: Option<&str>, key: Option<&str>) -> std::io::Result<()> {
    let refused = |why: &str| Err(std::io::Error::other(format!("refused: {why} (TLS-109)")));
    let marker = handle_marker(f)?;
    if let Some(job) = job {
        // A list that cannot be read is no list of nothing: refuse.
        let kept = match std::fs::read_to_string(kept_file(dir, job)) {
            Ok(kept) => kept,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(e),
        };
        if kept.lines().any(|m| m == marker) {
            return refused("this job is refused and names this file, which must stay as it is");
        }
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

/// TLS-109 test seam, compiled into debug builds only: with
/// `ALSEM_TEST_AFTER_PREFLIGHT=<command>`, the run runs it (through `sh -c`,
/// with [`ENV`] set to the claim folder) after its check and before any job,
/// so a test can change the file system between the check and the opens.
#[cfg(debug_assertions)]
pub fn test_seam_after_preflight() {
    if let Ok(cmd) = std::env::var("ALSEM_TEST_AFTER_PREFLIGHT") {
        let mut sh = std::process::Command::new("sh");
        sh.args(["-c", &cmd]);
        if let Some(dir) = dir() {
            sh.env(ENV, dir);
        }
        let _ = sh.status();
    }
}

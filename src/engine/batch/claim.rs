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
//! **What is claimed.** The files the run writes for its jobs (streams,
//! `--out`, a baseline update, shard files) and its trace files. The
//! engine's own caches (the package cache, `snapshot::cache`; the
//! impact-base cache, `engine::impact::cache`) are not claimed: each writes
//! a fresh temporary file of its own and renames it into place, and a rename
//! replaces a name in the cache folder, it never truncates a file that name
//! aliased.
//!
//! **How.** A run makes one private folder ([`set_dir`], handed to its
//! workers in [`ENV`]; a worker refuses to run without it). A claim is a
//! marker file named after the file's identity ([`Marker`]), read from the
//! opened handle ([`handle_id`]), created with `create_new`. A writer opens
//! its file WITHOUT truncating it, reads the handle's identity, claims it,
//! and only then truncates. Every write is its own claim: an existing marker
//! refuses the open, whoever made it, with one exception: the check's
//! reservation of a baseline for the one `--update-baseline` write that
//! rewrites it ([`reserve`]), which that write takes once. A refused open,
//! or a claim that cannot be made, truncates nothing and fails the run: this
//! process records it ([`failed`]) and a worker exits [`FAILED_EXIT`].
//!
//! The check claims the files the run must not write (what jobs read,
//! workspace inputs) before any job runs, and lists per refused job the
//! files it names ([`keep`]): its own writes and every trace file may not
//! open them, another job may. Outside a batch (no folder) a write opens as
//! it always did.
//!
//! **Traces.** Each process spools its trace events to a file of its own in
//! the run's folder ([`spool_path`]). After every worker has ended and every
//! job write is done, the parent publishes each spool to its trace file
//! through the claim ([`publish_spools`]): a trace that aliases an output or
//! an input costs only that trace.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use super::manifest::FileId;

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

pub fn marker_of(id: &FileId) -> Marker {
    format!("{}-{:x}", id.0, id.1)
}

/// The identity of the file `f` is: device and inode.
#[cfg(unix)]
pub(crate) fn handle_id(f: &File) -> std::io::Result<FileId> {
    use std::os::unix::fs::MetadataExt;
    let m = f.metadata()?;
    Ok((m.dev(), u128::from(m.ino())))
}

/// The identity of the file `f` is: its volume serial number and 128-bit
/// file id (`GetFileInformationByHandleEx`, `FileIdInfo`; the older 64-bit
/// index is not unique on ReFS). A file system that gives no id (or a zero
/// one, as some network providers do) is an error: no claim is made on a
/// guess.
#[cfg(windows)]
pub(crate) fn handle_id(f: &File) -> std::io::Result<FileId> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_ID_128, FILE_ID_INFO, FileIdInfo, GetFileInformationByHandleEx,
    };
    let mut info = FILE_ID_INFO {
        VolumeSerialNumber: 0,
        FileId: FILE_ID_128 {
            Identifier: [0; 16],
        },
    };
    // SAFETY: `info` is a FILE_ID_INFO the call fills, its size is given,
    // and the handle is `f`'s, open for the whole call.
    let ok = unsafe {
        GetFileInformationByHandleEx(
            f.as_raw_handle(),
            FileIdInfo,
            (&raw mut info).cast(),
            std::mem::size_of::<FILE_ID_INFO>() as u32,
        )
    };
    if ok == 0 {
        return Err(std::io::Error::last_os_error());
    }
    let id = u128::from_le_bytes(info.FileId.Identifier);
    if id == 0 {
        return Err(std::io::Error::other(
            "the file system gives this file no identity (a zero file id)",
        ));
    }
    Ok((info.VolumeSerialNumber, id))
}

/// Whether `f` is a file on a disk, which a truncation could lose content
/// of. A device, pipe or FIFO (`/dev/null`, `NUL`, `>(cat)`) is not: its
/// writers neither claim nor truncate it (`set_len` fails on it), as before.
#[cfg(unix)]
fn is_disk(f: &File) -> std::io::Result<bool> {
    Ok(f.metadata()?.is_file())
}

/// `GetFileType` is `FILE_TYPE_DISK` (`NUL` and a console are not).
#[cfg(windows)]
fn is_disk(f: &File) -> std::io::Result<bool> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{FILE_TYPE_DISK, GetFileType};
    // SAFETY: the handle is `f`'s, open for the whole call.
    Ok(unsafe { GetFileType(f.as_raw_handle()) } == FILE_TYPE_DISK)
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
    if is_disk(&f)? {
        claim(dir, &f, Some(job), key).inspect_err(|_| FAILED.store(true, Ordering::Relaxed))?;
        f.set_len(0)?;
    }
    Ok(f)
}

/// [`open_for_write`] for a trace file, published after every job write: a
/// refusal writes no trace but does not fail the run. No refused job's named
/// file is a trace's either: every kept list is checked.
pub fn open_trace(path: &Path) -> std::io::Result<File> {
    let Some(dir) = DIR.get() else {
        return File::create(path);
    };
    let f = open_kept(path)?;
    if is_disk(&f)? {
        let marker = marker_of(&handle_id(&f)?);
        for e in std::fs::read_dir(dir)? {
            let e = e?;
            if e.file_name().to_string_lossy().starts_with("kept-")
                && std::fs::read_to_string(e.path())?
                    .lines()
                    .any(|m| m == marker)
            {
                return Err(std::io::Error::other(
                    "refused: a refused job names this file, which must stay as it is (TLS-109)",
                ));
            }
        }
        claim(dir, &f, None, None)?;
        f.set_len(0)?;
    }
    Ok(f)
}

/// Open `path` to write, created when missing, NOT truncated: only once the
/// claim holds ([`is_disk`] says which files are claimed).
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
    let marker = marker_of(&handle_id(f)?);
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

/// Where this process spools its trace
/// ([`crate::engine::perf_trace::spool_file`]): a file of its own in the
/// run's folder, which aliases nothing.
pub fn spool_path() -> Option<PathBuf> {
    DIR.get()
        .map(|d| d.join(format!("spool-{}", std::process::id())))
}

/// Record beside a spool the trace file it is for, so the parent publishes
/// it ([`publish_spools`]), also when this process dies.
pub fn record_spool(spool: &Path, trace: &Path) -> std::io::Result<()> {
    let trace = std::path::absolute(trace)?;
    std::fs::write(
        spool.with_extension("to"),
        trace.to_string_lossy().as_bytes(),
    )
}

/// The parent, after every worker has ended and every job write is done:
/// publish each other process's spool to its trace file through
/// [`open_trace`] and remove it. A refused publish costs that trace only:
/// one stderr line.
pub fn publish_spools() {
    let Some(dir) = DIR.get() else { return };
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let own = format!("spool-{}.to", std::process::id());
    let mut records: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            let name = p.file_name().unwrap_or_default().to_string_lossy();
            name.starts_with("spool-") && name.ends_with(".to") && name != own
        })
        .collect();
    records.sort();
    for record in records {
        let spool = record.with_extension("");
        let published = std::fs::read_to_string(&record).and_then(|trace| {
            let bytes = std::fs::read(&spool)?;
            open_trace(Path::new(&trace))
                .and_then(|mut f| f.write_all(&bytes))
                .map_err(|e| std::io::Error::other(format!("{trace}: {e}")))
        });
        if let Err(e) = published {
            eprintln!("perf_trace: {e}; file tracing disabled");
        }
        let _ = std::fs::remove_file(&spool);
        let _ = std::fs::remove_file(&record);
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// TLS-109: a reserved baseline is taken once. The check reserves the
    /// file for one write key; the first open with that key claims it, a
    /// second open with the same key is refused (the `.taken` marker).
    ///
    /// Discrimination (2026-11-21, r5): the `.taken` create dropped (a
    /// reservation alone admitting the key): fails "a second open" (it was
    /// let through); restored, passes.
    #[test]
    fn a_reservation_is_taken_once() {
        let dir = tempfile::tempdir().unwrap();
        let file = tempfile::NamedTempFile::new().unwrap();
        let open = || OpenOptions::new().write(true).open(file.path()).unwrap();
        let marker = marker_of(&handle_id(&open()).unwrap());
        assert!(create(dir.path(), &marker, "reserve:job:b.json").unwrap());
        let first = claim(dir.path(), &open(), Some("job"), Some("job:b.json"));
        assert!(first.is_ok(), "the reserved write: {first:?}");
        let second = claim(dir.path(), &open(), Some("job"), Some("job:b.json"));
        assert!(second.is_err(), "a second open of the reservation");
    }
}

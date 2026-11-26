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
//! `--out`, a baseline update, shard files), its trace files and the IR
//! cache's report file ([`register_shared`]). The engine's own caches (the
//! package text cache, `snapshot::cache`; the dependency IR cache,
//! `program::ir_cache`; the impact-base cache, `engine::impact::cache`) are
//! not claimed: each writes a fresh temporary file of its own (created
//! exclusively, in its own folder) and renames or links it into place, and a
//! rename replaces a name in the cache folder, it never truncates a file that
//! name aliased; a prune only removes names in its folder.
//!
//! **The cache folders (TLS-116).** The other way round, a job write must not
//! land on a cache file ([`super::cache_guard`]: the run's one snapshot of the
//! text and IR cache folders). The check refuses a run with a write in a cache
//! folder, one that holds one, or one that is a cache file; and every open
//! here checks its path's folders before it creates anything and its handle's
//! identity once it is open ([`open_guarded`]), so a late alias is caught
//! too, and a refused open leaves nothing in a cache folder. The folder test
//! is path-based (links followed): a path another process changes between it
//! and the open is outside the scope above.
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
//! open them, another job may. Outside a batch (no folder) a write makes no
//! claim, but is still kept off the engine's caches (TLS-120,
//! [`open_outside`], [`make_dir`], [`open_append`]).
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

/// By `GetFileType`: a character device (`NUL`, a console) or a pipe is not
/// claimed; `FILE_TYPE_UNKNOWN` is the call's error return, which refuses
/// the write; anything else (a disk file) is claimed.
#[cfg(windows)]
fn is_disk(f: &File) -> std::io::Result<bool> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_TYPE_CHAR, FILE_TYPE_PIPE, FILE_TYPE_UNKNOWN, GetFileType,
    };
    // SAFETY: the handle is `f`'s, open for the whole call.
    match unsafe { GetFileType(f.as_raw_handle()) } {
        FILE_TYPE_CHAR | FILE_TYPE_PIPE => Ok(false),
        FILE_TYPE_UNKNOWN => {
            let e = std::io::Error::last_os_error();
            Err(match e.raw_os_error() {
                Some(0) | None => std::io::Error::other("the file system gives this file no type"),
                Some(_) => e,
            })
        }
        _ => Ok(true),
    }
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
/// file is left as it was, and the run fails ([`failed`]). Outside a batch:
/// [`open_outside`].
pub fn open_for_write(path: &Path, job: &str, key: Option<&str>) -> std::io::Result<File> {
    let Some(dir) = DIR.get() else {
        return open_outside(path);
    };
    let fail = || FAILED.store(true, Ordering::Relaxed);
    let snap = super::cache_guard::get(dir).inspect_err(|_| fail())?;
    let f = open_guarded(snap, path, Mode::Write, &fail)?;
    if is_disk(&f)? {
        claim(dir, &f, Some(job), key).inspect_err(|_| FAILED.store(true, Ordering::Relaxed))?;
        f.set_len(0)?;
    }
    Ok(f)
}

/// TLS-116: make a job's directory (a shard `--out`) and its missing parents,
/// one component at a time, each checked against the engine's cache folders
/// just before it is made: a component made earlier can make a later spelling
/// reach a cache folder (`missing/../alias/new`, `alias` a link to one, passes
/// while `missing` is absent). Refused, nothing more is made and the run fails
/// ([`failed`]). Outside a batch (TLS-120): the same steps against the
/// process's own snapshot ([`super::cache_guard::standalone`]).
pub fn make_dir(path: &Path) -> std::io::Result<()> {
    let fail = |e: std::io::Error| {
        if DIR.get().is_some() {
            FAILED.store(true, Ordering::Relaxed);
        }
        e
    };
    let held;
    let snap = match DIR.get() {
        Some(dir) => super::cache_guard::get(dir).map_err(fail)?,
        None => {
            held = super::cache_guard::standalone()?;
            &*held
        }
    };
    let mut cur = PathBuf::new();
    for c in dir_steps(std::env::current_dir()?.join(path)).components() {
        cur.push(c);
        if !matches!(c, std::path::Component::Normal(_)) {
            continue;
        }
        refuse_cache_path(snap, &cur).map_err(fail)?;
        match std::fs::create_dir(&cur) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists && cur.is_dir() => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

/// The path [`make_dir`] steps through. Windows folds a non-verbatim `..` by
/// name before it looks at any folder (`marker\..\shards` is `shards` even
/// when `marker` is a file), so it is folded here first (sol r1); unix takes a
/// `..` from where its prefix leads, so there the path stays as given. A
/// drive-relative path (`C:shards`, which joining to the current folder
/// leaves as it is) is first made absolute by Windows rules (`std::path::absolute`:
/// the drive's own current folder, sol r2).
fn dir_steps(p: PathBuf) -> PathBuf {
    if cfg!(windows) {
        let p = std::path::absolute(&p).unwrap_or(p);
        PathBuf::from(fold_windows(&p.to_string_lossy()))
    } else {
        p
    }
}

/// Windows `.` and `..` folded by name over a path's text: a verbatim path
/// (`\\?\`, `\\.\`) is taken as written; a `..` never climbs above the drive or
/// the `\\server\share`. A drive-relative path (`C:shards`) is taken as written
/// too: where it leads is the drive's current folder, which only the OS knows
/// ([`dir_steps`] makes it absolute first). Plain text, so its tests run anywhere.
fn fold_windows(s: &str) -> String {
    if s.starts_with(r"\\?\") || s.starts_with(r"\\.\") {
        return s.to_string();
    }
    let s = s.replace('/', "\\");
    if s.as_bytes().get(1) == Some(&b':') && s.as_bytes().get(2) != Some(&b'\\') {
        return s;
    }
    let (head, rest) = if let Some(r) = s.strip_prefix(r"\\") {
        let mut it = r.splitn(3, '\\');
        let (server, share) = (it.next().unwrap_or(""), it.next().unwrap_or(""));
        (
            format!(r"\\{server}\{share}"),
            it.next().unwrap_or("").to_string(),
        )
    } else if s.as_bytes().get(1) == Some(&b':') {
        (s[..2].to_string(), s[2..].to_string())
    } else {
        (String::new(), s.clone())
    };
    let mut parts: Vec<&str> = Vec::new();
    for part in rest.split('\\') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            p => parts.push(p),
        }
    }
    let mut out = head;
    for p in &parts {
        out.push('\\');
        out.push_str(p);
    }
    if parts.is_empty() {
        // `C:\`, not the drive-relative `C:`.
        out.push('\\');
    }
    out
}

/// [`open_for_write`] for a trace file, published after every job write: a
/// refusal writes no trace but does not fail the run. No refused job's named
/// file is a trace's either: every kept list is checked.
pub fn open_trace(path: &Path) -> std::io::Result<File> {
    let Some(dir) = DIR.get() else {
        return open_outside(path);
    };
    let f = open_guarded(super::cache_guard::get(dir)?, path, Mode::Write, &|| {})?;
    if is_disk(&f)? {
        refuse_kept(dir, &f)?;
        claim(dir, &f, None, None)?;
        f.set_len(0)?;
    }
    Ok(f)
}

/// Refuse `f` when any refused job names it (every kept list is checked).
fn refuse_kept(dir: &Path, f: &File) -> std::io::Result<()> {
    let marker = marker_of(&handle_id(f)?);
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
    Ok(())
}

/// The text a [`register_shared`] marker holds.
fn shared(tag: &str) -> String {
    format!("shared:{tag}")
}

/// The run's shared file, opened once per process and held: the parent's
/// registered handle ([`register_shared`]), or a worker's checked open of it.
/// One shared file exists (the IR cache report).
static SHARED: OnceLock<File> = OnceLock::new();

/// TLS-116: append `bytes` to the run's shared file `tag` at `path` (the IR
/// cache report, which every process of the run appends to), through the
/// handle this process holds: the parent's registered one, or a worker's open
/// of the file the parent registered (never created; refused when it is not
/// that file, is not a regular file, or is a cache file). A refusal appends
/// nothing; the next append tries again. Outside a batch: an append, created
/// when missing ([`open_append`]).
pub fn append_shared(path: &Path, tag: &str, bytes: &[u8]) -> std::io::Result<()> {
    let Some(dir) = DIR.get() else {
        return open_append(path)?.write_all(bytes);
    };
    let f = match SHARED.get() {
        Some(f) => f,
        None => {
            let f = open_registered(dir, path, tag)?;
            SHARED.get_or_init(|| f)
        }
    };
    (&*f).write_all(bytes)
}

/// A worker's open of the file the parent registered as `tag` at `path`.
fn open_registered(dir: &Path, path: &Path, tag: &str) -> std::io::Result<File> {
    let snap = super::cache_guard::get(dir)?;
    refuse_cache_path(snap, path)?;
    let f = options(Mode::AppendNoWait).open(path)?;
    refuse_nonregular(&f)?;
    refuse_cache_handle(snap, &f)?;
    let held = std::fs::read_to_string(dir.join(marker_of(&handle_id(&f)?)));
    if !held.is_ok_and(|h| h == shared(tag)) {
        return Err(std::io::Error::other(
            "refused: not the file this batch run registered for it (TLS-116)",
        ));
    }
    Ok(f)
}

/// A shared file must be a regular file: its identity is its claim.
fn refuse_nonregular(f: &File) -> std::io::Result<()> {
    if f.metadata()?.is_file() {
        Ok(())
    } else {
        Err(std::io::Error::other(
            "refused: not a regular file, which a batch run's shared file must be (TLS-116)",
        ))
    }
}

/// TLS-116: the parent, after its check and before any job, registers `path`
/// as the run's shared file `tag` ([`append_shared`]) and keeps it open: made
/// when missing (only once its folder passed the cache check), never
/// truncated. Refused when it is in a cache folder or a cache file, not a
/// regular file, a file a refused job names, or another claim's file (an
/// input, a job's write). Outside a batch: nothing.
pub fn register_shared(path: &Path, tag: &str) -> std::io::Result<()> {
    let Some(dir) = DIR.get() else { return Ok(()) };
    let f = register_in(super::cache_guard::get(dir)?, dir, path, tag)?;
    let _ = SHARED.set(f);
    Ok(())
}

fn register_in(
    snap: &super::cache_guard::Snapshot,
    dir: &Path,
    path: &Path,
    tag: &str,
) -> std::io::Result<File> {
    let f = open_guarded(snap, path, Mode::AppendNoWait, &|| {})?;
    refuse_nonregular(&f)?;
    refuse_kept(dir, &f)?;
    if !create(dir, &marker_of(&handle_id(&f)?), &shared(tag))? {
        return Err(std::io::Error::other(
            "refused: this file is another write or an input of this batch run (TLS-109)",
        ));
    }
    Ok(f)
}

/// TLS-116: open `path` to write (`append`: to append), never creating
/// before checking. First its folders are checked against the run's cache
/// snapshot ([`super::cache_guard`]); then an existing file is opened WITHOUT
/// creating and its handle's identity is checked (a hard link, a bind mount
/// of a file, any name of a cache file); only a missing file is created, once
/// its folder has passed, exclusively (`O_EXCL`), and a file this open made
/// is new, so its handle is not checked (an inode number a pruned cache file
/// had is no cache file). Never
/// truncates. Path-based folder test: a path another process changes during
/// the run is outside TLS-109's scope (module doc). `refused` runs when the
/// check refuses (a job write's refusal fails the run), not when the open
/// itself fails.
fn open_guarded(
    snap: &super::cache_guard::Snapshot,
    path: &Path,
    mode: Mode,
    refused: &dyn Fn(),
) -> std::io::Result<File> {
    let f = open_checked(snap, path, mode, refused)?;
    if mode == Mode::AppendNoWait {
        blocking(&f)?;
    }
    Ok(f)
}

/// How [`open_guarded`] opens.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Mode {
    /// To write (the caller truncates once the handle has passed).
    Write,
    /// To append, waiting at the open as a plain append does (a FIFO waits for
    /// its reader): a standalone run's diagnostic file (TLS-120 sol r1).
    Append,
    /// To append without waiting at the open (a batch run's files: a FIFO with
    /// no reader fails at once), then waiting on its writes ([`blocking`]).
    AppendNoWait,
}

/// TLS-120: clear the `O_NONBLOCK` an append opened with ([`options`]), so
/// its writes wait as a plain append's do and never drop a line with
/// `WouldBlock`.
#[cfg(unix)]
fn blocking(f: &File) -> std::io::Result<()> {
    use std::os::fd::AsRawFd;
    let fd = f.as_raw_fd();
    // SAFETY: `fd` is `f`'s, open for both calls; F_GETFL/F_SETFL take no
    // pointer.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags & !libc::O_NONBLOCK) } < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(not(unix))]
fn blocking(_f: &File) -> std::io::Result<()> {
    Ok(())
}

/// [`open_guarded`]'s checks and open.
fn open_checked(
    snap: &super::cache_guard::Snapshot,
    path: &Path,
    mode: Mode,
    refused: &dyn Fn(),
) -> std::io::Result<File> {
    refuse_cache_path(snap, path).inspect_err(|_| refused())?;
    // A file this open makes is new: no cache file, whatever number it got.
    let f = match options(mode).open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            match options(mode).create_new(true).open(path) {
                Ok(f) => return Ok(f),
                // A dangling symlink (`O_EXCL` does not follow it), or a
                // file made meanwhile: opened (made through the link, as
                // before TLS-116) and checked as any existing file.
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    options(mode).create(true).open(path)?
                }
                Err(e) => return Err(e),
            }
        }
        Err(e) => return Err(e),
    };
    // A device or pipe is no cache file, and may give no identity (`NUL`).
    if is_disk(&f)? {
        refuse_cache_handle(snap, &f).inspect_err(|_| refused())?;
    }
    Ok(f)
}

/// TLS-120: open `path` to write outside a batch run (a standalone `alsem`
/// command's `--out`, shard file or trace; `aldump`'s outputs), as
/// [`open_guarded`] opens a batch write against the process's own snapshot of
/// the cache folders ([`super::cache_guard::standalone`]), and truncated only
/// once its handle has passed: a name in a cache folder, or a cache file under
/// any name (a hard link, a bind mount), is refused and left as it was. No
/// claim: one process's writes are its own.
pub fn open_outside(path: &Path) -> std::io::Result<File> {
    let f = open_guarded(
        &*super::cache_guard::standalone()?,
        path,
        Mode::Write,
        &|| {},
    )?;
    if is_disk(&f)? {
        f.set_len(0)?;
    }
    Ok(f)
}

/// TLS-120: `std::fs::write` through [`open_outside`] (`aldump`'s outputs).
pub fn write_outside(path: &Path, bytes: impl AsRef<[u8]>) -> std::io::Result<()> {
    open_outside(path)?.write_all(bytes.as_ref())
}

/// TLS-120: open `path` to append, created when missing, checked as
/// [`open_outside`] checks (in a batch run against the run's snapshot, with no
/// claim): a diagnostic file an environment variable names
/// (`ALSEM_IR_CACHE_REPORT` outside a batch, `ALSEM_CONES_SCHEDULE_FILE`).
/// Outside a batch the open waits as a plain append's does (a FIFO for its
/// reader); in a batch it does not ([`Mode`]).
pub fn open_append(path: &Path) -> std::io::Result<File> {
    match DIR.get() {
        Some(dir) => open_guarded(
            super::cache_guard::get(dir)?,
            path,
            Mode::AppendNoWait,
            &|| {},
        ),
        None => open_guarded(
            &*super::cache_guard::standalone()?,
            path,
            Mode::Append,
            &|| {},
        ),
    }
}

/// How [`open_guarded`] opens ([`Mode`]). An append without waiting (a batch
/// run's shared file): `O_NONBLOCK`, so a FIFO with no reader fails the open
/// (`ENXIO`) instead of hanging it, and its type is checked before any write;
/// [`open_guarded`] then clears the flag, so its writes wait (TLS-120). A
/// job's stream may be a pipe it writes as before (TLS-109).
fn options(mode: Mode) -> OpenOptions {
    let mut o = OpenOptions::new();
    match mode {
        Mode::Write => {
            o.write(true);
        }
        Mode::Append => {
            o.append(true);
        }
        Mode::AppendNoWait => {
            o.append(true);
            #[cfg(unix)]
            std::os::unix::fs::OpenOptionsExt::custom_flags(&mut o, libc::O_NONBLOCK);
        }
    }
    o
}

/// Refuse `path` when it, or its name, lies in a cache folder or the claim
/// folder ([`super::cache_guard::refuses_path`]).
fn refuse_cache_path(snap: &super::cache_guard::Snapshot, path: &Path) -> std::io::Result<()> {
    let cwd = std::env::current_dir()?;
    match super::cache_guard::refuses_path(snap, &cwd, path)? {
        Some(why) => Err(std::io::Error::other(format!("refused: {why} (TLS-116)"))),
        None => Ok(()),
    }
}

/// Refuse the opened `f` when it is a file of the engine's caches, under any
/// name.
fn refuse_cache_handle(snap: &super::cache_guard::Snapshot, f: &File) -> std::io::Result<()> {
    if snap.holds_file(f)? {
        return Err(std::io::Error::other(
            "refused: this file is a file of the engine's caches (TLS-116)",
        ));
    }
    Ok(())
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

/// Where a process spools its trace
/// ([`crate::engine::perf_trace::spool_file`]): a file of its own in the
/// run's folder, which aliases nothing, named by `who` (`parent`, or `w<b>`
/// for worker `b`: a batch has one worker, a process id may come again).
pub fn spool_path(who: &str) -> Option<PathBuf> {
    DIR.get().map(|d| d.join(format!("spool-{who}")))
}

/// Record beside a spool the trace file it is for, so the parent publishes
/// it ([`publish_spools`]), also when this process dies. The path is kept
/// exactly ([`path_bytes`]), whatever its encoding. The record is written
/// whole to a temporary name and only then renamed into place, so a write
/// cut short never leaves a record naming a shorter path.
pub fn record_spool(spool: &Path, trace: &Path) -> std::io::Result<()> {
    let trace = std::path::absolute(trace)?;
    let tmp = spool.with_extension("to.tmp");
    write_record(&tmp, &path_bytes(&trace))?;
    std::fs::rename(&tmp, spool.with_extension("to"))
}

/// Write a spool record's bytes to `path`.
fn write_record(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    // TLS-109 test seam (debug builds only): `ALSEM_TEST_RECORD_FAILS` cuts
    // the write short after half the bytes and fails, as an I/O error would.
    #[cfg(debug_assertions)]
    if std::env::var_os("ALSEM_TEST_RECORD_FAILS").is_some() {
        std::fs::write(path, &bytes[..bytes.len() / 2])?;
        return Err(std::io::Error::other(
            "test seam: the record's write failed",
        ));
    }
    std::fs::write(path, bytes)
}

/// A path as bytes, losslessly: its raw bytes on unix.
#[cfg(unix)]
fn path_bytes(p: &Path) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;
    p.as_os_str().as_bytes().to_vec()
}

/// [`path_bytes`] read back.
#[cfg(unix)]
fn path_from(bytes: Vec<u8>) -> std::io::Result<PathBuf> {
    use std::os::unix::ffi::OsStringExt;
    Ok(PathBuf::from(std::ffi::OsString::from_vec(bytes)))
}

/// A path as bytes, losslessly: its UTF-16 units, little-endian, on Windows
/// (a path there may hold unpaired surrogates, which UTF-8 cannot).
#[cfg(windows)]
fn path_bytes(p: &Path) -> Vec<u8> {
    use std::os::windows::ffi::OsStrExt;
    p.as_os_str()
        .encode_wide()
        .flat_map(u16::to_le_bytes)
        .collect()
}

/// [`path_bytes`] read back. An odd number of bytes is no record of whole
/// UTF-16 units: an error, never a path.
#[cfg(windows)]
fn path_from(bytes: Vec<u8>) -> std::io::Result<PathBuf> {
    use std::os::windows::ffi::OsStringExt;
    if bytes.len() % 2 != 0 {
        return Err(std::io::Error::other("a spool record cut in a UTF-16 unit"));
    }
    let wide: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();
    Ok(PathBuf::from(std::ffi::OsString::from_wide(&wide)))
}

/// The parent, after every worker has ended and every job write is done:
/// publish each worker's spool (the ones with a [`record_spool`] record) to
/// its trace file through [`open_trace`] and remove it. A refused publish
/// costs that trace only: one stderr line.
pub fn publish_spools() {
    let Some(dir) = DIR.get() else { return };
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut records: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            let name = p.file_name().unwrap_or_default().to_string_lossy();
            name.starts_with("spool-") && name.ends_with(".to")
        })
        .collect();
    records.sort();
    for record in records {
        let spool = record.with_extension("");
        let published = std::fs::read(&record).and_then(|trace| {
            let trace = path_from(trace)?;
            let bytes = std::fs::read(&spool)?;
            open_trace(&trace)
                .and_then(|mut f| f.write_all(&bytes))
                .map_err(|e| std::io::Error::other(format!("{}: {e}", trace.display())))
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

    /// TLS-120 r1: an append opens without waiting (`O_NONBLOCK`), then waits on its writes
    /// as a plain append does: the open file's flags hold no `O_NONBLOCK`. A FIFO with a
    /// reader (opened on another thread, so neither open waits for good) is the case where
    /// it would matter.
    ///
    /// Discrimination (2026-11-24, r1): the `blocking(&f)?` call deleted: fails "blocking
    /// writes"; restored, passes.
    #[cfg(unix)]
    #[test]
    fn an_append_writes_blocking() {
        use std::os::fd::AsRawFd;
        let t = tempfile::tempdir().unwrap();
        let fifo = t.path().join("fifo");
        let c = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
        // SAFETY: a valid C string for the call's whole length.
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0, "mkfifo");
        let reader = {
            let fifo = fifo.clone();
            std::thread::spawn(move || {
                let mut s = String::new();
                std::io::Read::read_to_string(&mut File::open(fifo).unwrap(), &mut s).unwrap();
                s
            })
        };
        let snap = super::super::cache_guard::Snapshot::default();
        let f = loop {
            match open_guarded(&snap, &fifo, Mode::AppendNoWait, &|| {}) {
                Ok(f) => break f,
                // ENXIO: the reader has not opened yet.
                Err(_) => std::thread::sleep(std::time::Duration::from_millis(5)),
            }
        };
        // SAFETY: the fd is `f`'s, open for the call.
        let flags = unsafe { libc::fcntl(f.as_raw_fd(), libc::F_GETFL) };
        assert_eq!(flags & libc::O_NONBLOCK, 0, "blocking writes");
        (&f).write_all(b"line\n").unwrap();
        drop(f);
        assert_eq!(reader.join().unwrap(), "line\n");
    }

    /// TLS-120 sol r1: Windows folds a non-verbatim `..` by name, so [`make_dir`] steps
    /// through the folded path: `C:\tmp\marker\..\shards` is `C:\tmp\shards` (whatever
    /// `marker` is); a verbatim path stays as written; a `..` stops at the drive or the
    /// `\\server\share`; `/` is `\`. (sol r2) A drive-relative path (`C:shards`) is never
    /// rooted at the drive: the string fold leaves it as written ([`dir_steps`] makes it
    /// absolute first, `dir_steps_resolves_a_drive_relative_path` on Windows).
    ///
    /// Discrimination (2026-11-25, sol r1): the `..` arm pushing the part instead of popping:
    /// fails "(marker)"; restored, passes. (2026-11-26, sol r2) The drive-relative early
    /// return deleted: fails "(drive-relative)" (`C:\shards`); restored, passes.
    #[test]
    fn windows_dot_dot_folds_by_name() {
        for (case, given, folded) in [
            ("(drive-relative)", "C:shards", "C:shards"),
            ("(drive-relative dot dot)", r"C:a\..\b", r"C:a\..\b"),
            ("(marker)", r"C:\tmp\marker\..\shards", r"C:\tmp\shards"),
            ("(dot)", r"C:\tmp\.\shards", r"C:\tmp\shards"),
            ("(slash)", "C:/tmp/a/../b", r"C:\tmp\b"),
            ("(above the drive)", r"C:\..\..\x", r"C:\x"),
            ("(the drive)", r"C:\tmp\..", r"C:\"),
            ("(share)", r"\\host\share\a\..\..\b", r"\\host\share\b"),
            (
                "(verbatim)",
                r"\\?\C:\tmp\marker\..\shards",
                r"\\?\C:\tmp\marker\..\shards",
            ),
        ] {
            assert_eq!(fold_windows(given), folded, "{case}");
        }
    }

    /// TLS-120 sol r2 (Windows only): a drive-relative shard path on the current drive
    /// (`X:shards`) is the current folder's `shards`, as Windows resolves it, not `X:\shards`.
    ///
    /// Discrimination: not run (no Windows host here); the Linux half is
    /// `windows_dot_dot_folds_by_name`'s "(drive-relative)" case.
    #[cfg(windows)]
    #[test]
    fn dir_steps_resolves_a_drive_relative_path() {
        let cwd = std::env::current_dir().unwrap();
        let drive: String = cwd.to_string_lossy().chars().take(2).collect();
        assert_eq!(
            drive.as_bytes()[1],
            b':',
            "precondition: a drive-letter current folder"
        );
        let got = dir_steps(PathBuf::from(format!("{drive}shards")));
        assert_eq!(got, cwd.join("shards"));
    }

    /// TLS-120 sol r1: a standalone append (`Mode::Append`, what [`open_append`] opens
    /// outside a batch) waits at its open as a plain append does: with a FIFO whose reader
    /// starts 300 ms later, the open succeeds once the reader is there and the line arrives.
    ///
    /// Discrimination (2026-11-25, sol r1): `Mode::Append` opened with `O_NONBLOCK` as
    /// `AppendNoWait` is: fails "the open waits" (ENXIO); restored, passes.
    #[cfg(unix)]
    #[test]
    fn a_standalone_append_waits_for_a_late_reader() {
        let t = tempfile::tempdir().unwrap();
        let fifo = t.path().join("fifo");
        let c = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
        // SAFETY: a valid C string for the call's whole length.
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0, "mkfifo");
        let reader = {
            let fifo = fifo.clone();
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(300));
                let mut s = String::new();
                std::io::Read::read_to_string(&mut File::open(fifo).unwrap(), &mut s).unwrap();
                s
            })
        };
        let snap = super::super::cache_guard::Snapshot::default();
        let f = open_guarded(&snap, &fifo, Mode::Append, &|| {});
        let f = match f {
            Ok(f) => f,
            Err(e) => {
                // Let the reader finish its open, so the test ends.
                drop(OpenOptions::new().write(true).open(&fifo));
                panic!("the open waits: {e}");
            }
        };
        (&f).write_all(b"line\n").unwrap();
        drop(f);
        assert_eq!(reader.join().unwrap(), "line\n");
    }

    /// TLS-116: the guards of the shared file's registration ([`register_in`]),
    /// each against a hand-made snapshot of a cache folder holding one entry.
    /// Each refused case makes nothing: no file in the cache folder, no marker.
    ///
    /// Discrimination (2026-11-21, r2), each break alone, then restored (all
    /// pass): `open_guarded`'s folder check skipped (`refuse_cache_path(..)`
    /// call deleted): fails "(in the cache folder)" (registered); its handle
    /// check skipped (`false &&` on `if is_disk(&f)?`): fails "(a hard link to
    /// an entry)"; `refuse_nonregular` passing all (`true ||`): fails "(a
    /// device)"; the `refuse_kept` call deleted: fails "(a refused job's
    /// file)"; the marker test skipped (`false &&` on `if !create(..)?`): fails
    /// "(an input's marker)".
    #[cfg(unix)]
    #[test]
    fn registration_refuses_what_it_must() {
        use super::super::cache_guard::Snapshot;
        use super::super::manifest::read_file_id;
        let t = tempfile::tempdir().unwrap();
        let (cache, claims) = (t.path().join("cache"), t.path().join("claims"));
        std::fs::create_dir(&cache).unwrap();
        std::fs::create_dir(&claims).unwrap();
        let entry = cache.join("e.bin");
        std::fs::write(&entry, "ENTRY").unwrap();
        let id = |p: &Path| read_file_id(p).unwrap().unwrap();
        // Birth times unknown (0): matched by number alone.
        let snap = Snapshot {
            roots: vec![cache.clone()],
            dirs: [(id(&cache), 0)].into(),
            files: [(id(&entry), 0)].into(),
            ..Snapshot::default()
        };
        let markers = || std::fs::read_dir(&claims).unwrap().count();
        let refused = |case: &str, path: &Path| {
            let before = markers();
            let r = register_in(&snap, &claims, path, "t");
            assert!(r.is_err(), "{case}: registered");
            assert_eq!(markers(), before, "{case}: a marker made");
        };
        let inside = cache.join("report.jsonl");
        refused("(in the cache folder)", &inside);
        assert!(!inside.exists(), "nothing made in the cache folder");
        let link = t.path().join("link.jsonl");
        std::fs::hard_link(&entry, &link).unwrap();
        refused("(a hard link to an entry)", &link);
        assert_eq!(std::fs::read(&entry).unwrap(), b"ENTRY");
        refused("(a device)", Path::new("/dev/null"));
        let kept = t.path().join("kept.jsonl");
        std::fs::write(&kept, "").unwrap();
        std::fs::write(claims.join("kept-job"), marker_of(&id(&kept))).unwrap();
        refused("(a refused job's file)", &kept);
        let input = t.path().join("input.jsonl");
        std::fs::write(&input, "").unwrap();
        std::fs::write(claims.join(marker_of(&id(&input))), "read").unwrap();
        refused("(an input's marker)", &input);
        let ok = t.path().join("ok.jsonl");
        assert!(
            register_in(&snap, &claims, &ok, "t").is_ok(),
            "a plain file"
        );
        assert_eq!(
            std::fs::read_to_string(claims.join(marker_of(&id(&ok)))).unwrap(),
            "shared:t"
        );
    }

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

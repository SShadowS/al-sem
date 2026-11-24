//! TLS-116: the engine's cache folders, protected from a batch run's writes.
//!
//! A job write must not land on a cache file: a truncated text entry another
//! engine maps kills it (SIGBUS), and a job output in a cache folder is the
//! cache's to replace. The caches' own writes need no claim: every cache file
//! is made in its folder with a fresh name (an exclusive temp file, linked or
//! renamed into place through the folder's handle), lock files are opened
//! without truncation, and a prune only removes names in its folders. The
//! run's own writes make no links (no job, trace or report write creates a
//! symlink or a hard link), so every alias a check meets existed before or was
//! made by another process.
//!
//! **One snapshot per run.** The parent's check ([`take`]) first MAKES the
//! cache folders this run's caches would use, exactly as they make them:
//! ENG-113's text folder always (the text cache is on whenever a workspace has
//! dependencies, so every batch run makes it), and the IR cache's `ir/` when
//! the IR cache is on and has a key. So a spelling such as `missing/..`
//! resolves now as it will later. A folder that is no directory after that
//! (an override naming a file, a dangling symlink) turns its cache OFF for the
//! whole run, in the parent and every worker: no cache is ever used in a
//! folder the snapshot does not hold. Then the check records the identity of
//! every directory and file inside the folders ([`Snapshot`]; a name removed
//! during the walk, a concurrent prune, is skipped; any other error refuses
//! the run: the caches cannot be protected). The parent writes it, with those
//! decisions (flags, `0` or `1` only), its counts and a blake3 over flags,
//! counts and body, to the claim folder ([`publish`]); a worker reads it at
//! its start ([`load`]) and fails closed when it is missing or damaged (also
//! a cache on with no folder), so no worker walks again.
//!
//! **Identities.** Unix: device, inode and birth time (`statx`'s btime, on
//! ext4, xfs and btrfs; 0 where the file system gives none). A link keeps the
//! birth time; an inode number reused after a prune gets a new one, so it is
//! not taken for the pruned file. Windows: the volume serial and 128-bit file
//! id, which are not reused that way.
//!
//! **What it refuses** (the check, exit 3; every open again, `super::claim`):
//! a write whose resolved destination lies in a cache folder; a write whose
//! NAME, as spelled, lies in one (a cache could replace that name: a symlink
//! `ir/last-prune` pointing outside is still the prune's name), and so does
//! the name of any symlink met while resolving it ([`link_hops`]); a write that
//! is a cache file under any name (a hard link, a bind mount of a file: the
//! handle's identity); a directory target that holds a cache folder; and a
//! write into the run's claim folder. A cache file published during the run is
//! not in the snapshot, but its folder is, so no job write reaches it. The
//! folder tests are path-based: a path another process changes during the run
//! is outside TLS-109's scope.
//!
//! **Outside a batch (TLS-120).** A standalone `alsem` or `aldump` refuses
//! the same writes ([`super::claim::open_outside`]) against a snapshot of its
//! own ([`standalone`]): the same folders, made the same way, but only their
//! directories are recorded, once per process. A file is looked up at its
//! open, and only when it may have another name than the one the folder test
//! checked ([`may_alias`]: more than one link, or a mount of its own). The
//! same scope holds: an alias another process makes between the checks and
//! the open is not caught.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use super::manifest::FileId;

/// A birth time in nanoseconds since the epoch; 0: none known.
type Born = u64;

/// The identities of the cache folders and everything in them, at the check.
#[derive(Debug, Default)]
pub struct Snapshot {
    /// The cache folders, as made, for the check's messages and its "holds"
    /// test (the parent's only).
    pub(super) roots: Vec<PathBuf>,
    /// Whether this run uses the text cache and the IR cache.
    pub(super) text_on: bool,
    pub(super) ir_on: bool,
    /// Every directory: the folders themselves and every one inside them.
    pub(super) dirs: HashMap<FileId, Born>,
    /// Every other entry inside them (links are not followed or recorded).
    pub(super) files: HashMap<FileId, Born>,
    /// The run's claim folder (never written to the file: each process knows
    /// its own).
    pub(super) claims: Option<FileId>,
    /// Only the folders were walked ([`standalone`]): `files` is empty, and a
    /// file is looked up when it is opened ([`Snapshot::holds_file`]).
    pub(super) dirs_only: bool,
}

/// Whether `set` holds `id` born at `born` (an unknown birth time on either
/// side matches by number alone).
fn holds(set: &HashMap<FileId, Born>, id: &FileId, born: Born) -> bool {
    set.get(id)
        .is_some_and(|&b| b == 0 || born == 0 || b == born)
}

impl Snapshot {
    /// Whether the file `m` describes, with identity `id`, is a cache file.
    pub(super) fn is_cache_file(&self, id: &FileId, m: &std::fs::Metadata) -> bool {
        holds(&self.files, id, born(m))
    }

    /// Whether the opened `f` is a cache file under any name. A folders-only
    /// snapshot ([`standalone`]) walks the folders' files now, and only for a
    /// file that may have another name ([`may_alias`]): a file with one name,
    /// no mount of its own, is the name the folder test already checked.
    pub(super) fn holds_file(&self, f: &std::fs::File) -> std::io::Result<bool> {
        let (id, m) = (super::claim::handle_id(f)?, f.metadata()?);
        if !self.dirs_only {
            return Ok(self.is_cache_file(&id, &m));
        }
        if !may_alias(f, &m) {
            return Ok(false);
        }
        let mut now = Snapshot::default();
        walk_roots(&self.roots, &mut now, true).map_err(std::io::Error::other)?;
        Ok(now.is_cache_file(&id, &m))
    }
}

/// Whether the file `f` may have a name besides the one it was opened by: more
/// than one link, or a mount of its own (a bind mount of a file keeps its
/// device and inode). Unknown counts as may.
#[cfg(unix)]
fn may_alias(f: &std::fs::File, m: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    m.nlink() > 1 || mount_root(f)
}

/// Windows (no stable link count in std): every existing file may.
#[cfg(not(unix))]
fn may_alias(_f: &std::fs::File, _m: &std::fs::Metadata) -> bool {
    true
}

/// Whether `f` is the root of a mount (`statx`'s `STATX_ATTR_MOUNT_ROOT`); an
/// error, or a kernel that does not report the attribute, says yes.
#[cfg(target_os = "linux")]
fn mount_root(f: &std::fs::File) -> bool {
    use std::os::fd::AsRawFd;
    // SAFETY: an all-zero `statx` is a valid value of the plain C struct.
    let mut stx: libc::statx = unsafe { std::mem::zeroed() };
    // SAFETY: the empty path with `AT_EMPTY_PATH` names `f`'s descriptor, open
    // for the whole call; `stx` is the buffer the call fills.
    let r = unsafe {
        libc::statx(
            f.as_raw_fd(),
            c"".as_ptr(),
            libc::AT_EMPTY_PATH,
            0,
            &raw mut stx,
        )
    };
    let bit = libc::STATX_ATTR_MOUNT_ROOT as u64;
    r != 0 || stx.stx_attributes_mask & bit == 0 || stx.stx_attributes & bit != 0
}

/// Other unix systems have no bind mount of a file.
#[cfg(all(unix, not(target_os = "linux")))]
fn mount_root(_f: &std::fs::File) -> bool {
    false
}

/// The claim folder's file holding the parent's snapshot.
pub(super) const FILE: &str = "cache-snapshot";
const MAGIC: &str = "alsem-cache-snapshot 1";

/// `m`'s birth time ([`Born`]).
#[cfg(unix)]
fn born(m: &std::fs::Metadata) -> Born {
    m.created()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| u64::try_from(d.as_nanos()).unwrap_or(u64::MAX))
}

/// Windows file ids are 128-bit and not reused as unix inode numbers are.
#[cfg(not(unix))]
fn born(_m: &std::fs::Metadata) -> Born {
    0
}

/// The parent's snapshot: make the cache folders, decide which caches the run
/// uses, then walk the folders. `Err`: the check's one line.
pub(super) fn take() -> Result<Snapshot, String> {
    let _s = crate::engine::perf_trace::span("batch", "cache.snapshot");
    // As `snapshot::cache::cache_dir` makes it, without its warning: a folder
    // that cannot be made is the cache's to report when it is used.
    let text = crate::snapshot::cache::cache_dir_path();
    let _ = std::fs::create_dir_all(&text);
    let ir = crate::program::ir_cache::make_ir_dir();
    let mut snap = Snapshot {
        text_on: text.is_dir(),
        ir_on: ir.as_ref().is_some_and(|p| p.is_dir()),
        ..Snapshot::default()
    };
    if snap.text_on {
        snap.roots.push(text);
    }
    if snap.ir_on {
        snap.roots.extend(ir);
    }
    walk_roots(&snap.roots.clone(), &mut snap, true).map_err(|e| format!("al-sem batch: {e}"))?;
    snap.claims = claims_id()?;
    apply(&snap);
    Ok(snap)
}

/// Record `roots` and everything under them in `snap`; with `files` false,
/// the directories only (no file is looked at). `Err`: the message's tail.
fn walk_roots(roots: &[PathBuf], snap: &mut Snapshot, files: bool) -> Result<(), String> {
    for root in roots {
        let fail = |e: std::io::Error| {
            format!(
                "cannot list the cache folder {} ({e}): the engine's caches cannot be protected from the writes (TLS-116)",
                root.display()
            )
        };
        let m = std::fs::metadata(root).map_err(fail)?;
        let Some(id) = super::manifest::read_file_id(root).map_err(fail)? else {
            continue;
        };
        // The default `ir/` is inside the text folder: walked once.
        if snap.dirs.insert(id, born(&m)).is_none() {
            walk(root, snap, files).map_err(fail)?;
        }
    }
    Ok(())
}

/// TLS-120: the snapshot of a process outside a batch run (a standalone
/// `alsem` or `aldump`), taken at its first write and kept: the cache folders
/// made as [`take`] makes them (the text folder always; the IR cache's when it
/// is on, with or without a key), then their directories only.
/// A file is looked up when an open needs it ([`Snapshot::holds_file`]): a
/// whole walk costs 0.4 s on server1's 134,395 entries, the directories alone
/// do not stat a file. No cache is turned off: an outside run's caches are as
/// they were. `Err` (a folder that cannot be listed): every write is refused.
pub(super) fn standalone() -> std::io::Result<&'static Snapshot> {
    static STANDALONE: OnceLock<Result<Snapshot, String>> = OnceLock::new();
    STANDALONE
        .get_or_init(|| {
            let text = crate::snapshot::cache::cache_dir_path();
            let _ = std::fs::create_dir_all(&text);
            let mut snap = Snapshot {
                dirs_only: true,
                ..Snapshot::default()
            };
            snap.roots = [text, crate::program::ir_cache::make_ir_root()]
                .into_iter()
                .filter(|p| p.is_dir())
                .collect();
            walk_roots(&snap.roots.clone(), &mut snap, false).map(|()| snap)
        })
        .as_ref()
        .map_err(|e| std::io::Error::other(format!("refused: {e}")))
}

/// The identity of this process's claim folder, when it is in a batch run.
fn claims_id() -> Result<Option<FileId>, String> {
    match super::claim::dir() {
        Some(d) => super::manifest::read_file_id(d)
            .map_err(|e| format!("al-sem batch: the claim folder {}: {e}", d.display())),
        None => Ok(None),
    }
}

/// Turn off, for this process, each cache the snapshot says the run does not
/// use.
fn apply(snap: &Snapshot) {
    if !snap.text_on {
        crate::snapshot::cache::disable();
    }
    if !snap.ir_on {
        crate::program::ir_cache::disable();
    }
}

/// Whether `e` says the name is gone (a prune removed it during the walk).
/// Windows: also a file whose delete is pending (`ERROR_DELETE_PENDING`).
fn gone(e: &std::io::Error) -> bool {
    matches!(
        e.kind(),
        std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
    ) || (cfg!(windows) && e.raw_os_error() == Some(303))
}

/// Record every entry under `dir` (links not followed); with `files` false,
/// the directories only.
fn walk(dir: &Path, snap: &mut Snapshot, files: bool) -> std::io::Result<()> {
    let names = match std::fs::read_dir(dir) {
        Ok(rd) => rd.collect::<Result<Vec<_>, _>>(),
        Err(e) => Err(e),
    };
    let names = match names {
        Ok(names) => names,
        Err(e) if gone(&e) => return Ok(()),
        Err(e) => return Err(e),
    };
    #[cfg(debug_assertions)]
    test_seam_during_snapshot();
    for e in names {
        // The listing's own type: a file is not looked at when only the
        // directories are recorded.
        if !files && e.file_type().is_ok_and(|t| !t.is_dir()) {
            continue;
        }
        let p = e.path();
        let m = match std::fs::symlink_metadata(&p) {
            Ok(m) => m,
            Err(e) if gone(&e) => continue,
            Err(e) => return Err(e),
        };
        if m.file_type().is_symlink() {
            continue;
        }
        let Some(id) = entry_id(&p, &m)? else {
            continue;
        };
        if m.is_dir() {
            if snap.dirs.insert(id, born(&m)).is_none() {
                walk(&p, snap, files)?;
            }
        } else if files {
            snap.files.insert(id, born(&m));
        }
    }
    Ok(())
}

#[cfg(unix)]
fn entry_id(_p: &Path, m: &std::fs::Metadata) -> std::io::Result<Option<FileId>> {
    use std::os::unix::fs::MetadataExt;
    Ok(Some((m.dev(), u128::from(m.ino()))))
}

/// The volume serial number and 128-bit file id, from a handle (TLS-109's
/// [`super::manifest::read_file_id`]); `None` when the name is gone. One open
/// per entry: `GetFileInformationByHandleEx(FileIdExtdDirectoryInfo)` would
/// give the ids from the folder's listing, but no Windows machine runs this
/// code's tests, so the open TLS-109 already uses stays. An entry whose delete
/// is pending refuses the open (access denied): gone when its name is.
#[cfg(windows)]
fn entry_id(p: &Path, _m: &std::fs::Metadata) -> std::io::Result<Option<FileId>> {
    match super::manifest::read_file_id(p) {
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
            match std::fs::symlink_metadata(p) {
                Err(g) if gone(&g) => Ok(None),
                _ => Err(e),
            }
        }
        Err(e) if gone(&e) => Ok(None),
        r => r,
    }
}

/// TLS-116 test seam, compiled into debug builds only: with
/// `ALSEM_TEST_DURING_SNAPSHOT=<command>`, the walk runs it (through `sh -c`)
/// once, after it has listed the first folder and before it looks at its
/// entries, so a test can remove an entry as a concurrent prune would.
#[cfg(debug_assertions)]
fn test_seam_during_snapshot() {
    use std::sync::atomic::{AtomicBool, Ordering};
    static RAN: AtomicBool = AtomicBool::new(false);
    if let Ok(cmd) = std::env::var("ALSEM_TEST_DURING_SNAPSHOT")
        && !RAN.swap(true, Ordering::SeqCst)
    {
        let _ = std::process::Command::new("sh").args(["-c", &cmd]).status();
    }
}

static SNAPSHOT: OnceLock<Snapshot> = OnceLock::new();

/// The header's blake3: over its flags and counts (`fields`, as written) and
/// the body, so neither can change without the other.
fn hashed(fields: &str, body: &str) -> String {
    let mut h = blake3::Hasher::new();
    h.update(fields.as_bytes());
    h.update(b"\n");
    h.update(body.as_bytes());
    h.finalize().to_hex().to_string()
}

/// The snapshot's body: one line per directory and file.
fn body(snap: &Snapshot) -> String {
    use std::fmt::Write as _;
    let mut text = String::new();
    for (kind, ids) in [("d", &snap.dirs), ("f", &snap.files)] {
        for ((dev, ino), b) in ids {
            let _ = writeln!(text, "{kind} {dev} {ino} {b}");
        }
    }
    text
}

/// The parent, once its check has passed: keep `snap` for its own opens and
/// write it to the claim folder for its workers, whole (a temporary file
/// renamed into place), then claim it as a file no write may take.
pub(super) fn publish(snap: Snapshot, dir: &Path) -> std::io::Result<()> {
    let body = body(&snap);
    let fields = format!(
        "text={} ir={} dirs={} files={}",
        u8::from(snap.text_on),
        u8::from(snap.ir_on),
        snap.dirs.len(),
        snap.files.len(),
    );
    let header = format!("{MAGIC} {fields} blake3={}\n", hashed(&fields, &body));
    let tmp = dir.join(format!("{FILE}.tmp"));
    std::fs::write(&tmp, header + &body)?;
    std::fs::rename(&tmp, dir.join(FILE))?;
    if let Some(id) = super::manifest::read_file_id(&dir.join(FILE))? {
        super::claim::protect(&super::claim::marker_of(&id))?;
    }
    let _ = SNAPSHOT.set(snap);
    Ok(())
}

/// This run's snapshot: the parent's own, or (a worker) the one [`load`]
/// read. `Err` (none): every write is refused.
pub(super) fn get(dir: &Path) -> std::io::Result<&'static Snapshot> {
    match SNAPSHOT.get() {
        Some(s) => Ok(s),
        None => load(dir),
    }
}

/// A worker, at its start: read the parent's snapshot from the claim folder,
/// check its header (counts and blake3), and turn off the caches the run does
/// not use. `Err` (missing or damaged): the worker must not run.
pub fn load(dir: &Path) -> std::io::Result<&'static Snapshot> {
    let bad = |why: String| {
        std::io::Error::other(format!(
            "refused: no usable cache snapshot from the run's parent: {why} (TLS-116)"
        ))
    };
    let text = std::fs::read_to_string(dir.join(FILE)).map_err(|e| bad(e.to_string()))?;
    let (header, body) = text
        .split_once('\n')
        .ok_or_else(|| bad("no header".into()))?;
    let fields = header
        .strip_prefix(MAGIC)
        .ok_or_else(|| bad("not a snapshot".into()))?;
    let field = |k: &str| {
        fields
            .split(' ')
            .find_map(|f| f.strip_prefix(k)?.strip_prefix('='))
            .ok_or_else(|| bad(format!("no {k}")))
    };
    let (text_on, ir_on, dirs, files) = (
        field("text")?,
        field("ir")?,
        field("dirs")?,
        field("files")?,
    );
    let covered = format!("text={text_on} ir={ir_on} dirs={dirs} files={files}");
    if hashed(&covered, body) != field("blake3")? {
        return Err(bad("its blake3 does not match".into()));
    }
    let flag = |v: &str| match v {
        "0" => Ok(false),
        "1" => Ok(true),
        _ => Err(bad(format!("a flag that is neither 0 nor 1: {v}"))),
    };
    let mut snap = Snapshot {
        text_on: flag(text_on)?,
        ir_on: flag(ir_on)?,
        claims: super::manifest::read_file_id(dir).map_err(|e| bad(e.to_string()))?,
        ..Snapshot::default()
    };
    for line in body.lines() {
        let mut parts = line.split(' ');
        let (Some(kind), Some(dev), Some(ino), Some(b), None) = (
            parts.next(),
            parts.next(),
            parts.next(),
            parts.next(),
            parts.next(),
        ) else {
            return Err(bad(format!("a damaged line: {line}")));
        };
        let num = |s: &str| bad(format!("a damaged line: {line} ({s})"));
        let id = (
            dev.parse().map_err(|_| num(dev))?,
            ino.parse().map_err(|_| num(ino))?,
        );
        let b = b.parse().map_err(|_| num(b))?;
        match kind {
            "d" => snap.dirs.insert(id, b),
            "f" => snap.files.insert(id, b),
            _ => return Err(bad(format!("a damaged line: {line}"))),
        };
    }
    if field("dirs")? != snap.dirs.len().to_string()
        || field("files")? != snap.files.len().to_string()
    {
        return Err(bad("its counts do not match".into()));
    }
    // A cache on holds at least its own folder.
    if (snap.text_on || snap.ir_on) && snap.dirs.is_empty() {
        return Err(bad("a cache on with no folder".into()));
    }
    apply(&snap);
    Ok(SNAPSHOT.get_or_init(|| snap))
}

/// Why the write `spelled` (against `cwd`) must not be made, `None` when it
/// may: its resolved destination lies in a cache folder, or its name as
/// spelled does, or the name of any symlink met on the way does (a cache could
/// replace that name: `report → ir/last-prune → outside` is the prune's), or
/// any of them lies in the run's claim folder. Checked before anything is
/// created; the file itself, once open, is checked by its handle
/// (`super::claim`).
pub(super) fn refuses_path(
    snap: &Snapshot,
    cwd: &Path,
    spelled: &Path,
) -> std::io::Result<Option<&'static str>> {
    let resolved = super::manifest::resolved_path(cwd, spelled)?;
    if let Some(why) = folder_hit(snap, resolved.ancestors())? {
        return Ok(Some(why));
    }
    // Each name of the spelling: the folder that holds it.
    let folded = super::manifest::fold_path(cwd, spelled);
    if let Some(why) = folder_hit(snap, folded.ancestors().skip(1))? {
        return Ok(Some(why));
    }
    link_hops(snap, cwd.join(spelled), 0)
}

/// Follow `path` as the OS does, one component at a time; every folder it
/// enters is checked ([`folder_hit`]), also one a later `..` leaves, and at
/// each symlink met the folder holding the link's own name is checked, then
/// the walk goes on through the link's text. A missing component ends it (no
/// link lies beyond); 40 hops end it as the OS would (`ELOOP`).
fn link_hops(snap: &Snapshot, path: PathBuf, hops: u32) -> std::io::Result<Option<&'static str>> {
    use std::path::Component;
    let parts: Vec<Component> = path.components().collect();
    let mut cur = PathBuf::new();
    for (i, c) in parts.iter().enumerate() {
        match c {
            Component::CurDir => continue,
            // `cur` holds no link here (each was replaced by its text).
            Component::ParentDir => {
                cur.pop();
                continue;
            }
            c => cur.push(c),
        }
        let m = match std::fs::symlink_metadata(&cur) {
            Ok(m) => m,
            Err(e) if gone(&e) => return Ok(None),
            Err(e) => return Err(e),
        };
        if !m.file_type().is_symlink() {
            // A folder the spelling enters, even one a later `..` leaves: a
            // prune can remove it and make the path unreachable (sol r4).
            if m.is_dir()
                && let Some(why) = folder_hit(snap, std::iter::once(cur.as_path()))?
            {
                return Ok(Some(why));
            }
            continue;
        }
        let parent = cur.parent().unwrap_or(Path::new("")).to_path_buf();
        if let Some(why) = folder_hit(snap, std::iter::once(parent.as_path()))? {
            return Ok(Some(why));
        }
        if hops >= 40 {
            return Ok(None);
        }
        let mut next = parent.join(std::fs::read_link(&cur)?);
        for rest in &parts[i + 1..] {
            next.push(rest);
        }
        return link_hops(snap, next, hops + 1);
    }
    Ok(None)
}

/// The first of `dirs` (existing folders only) that is a cache folder or the
/// claim folder: why.
fn folder_hit<'a>(
    snap: &Snapshot,
    dirs: impl Iterator<Item = &'a Path>,
) -> std::io::Result<Option<&'static str>> {
    for d in dirs {
        let m = match std::fs::metadata(d) {
            Ok(m) if m.is_dir() => m,
            Ok(_) => continue,
            Err(e) if gone(&e) => continue,
            Err(e) => return Err(e),
        };
        let Some(id) = super::manifest::read_file_id(d)? else {
            continue;
        };
        if holds(&snap.dirs, &id, born(&m)) {
            return Ok(Some("this file is in a cache folder of the engine"));
        }
        if snap.claims == Some(id) {
            return Ok(Some("this file is in the batch run's claim folder"));
        }
    }
    Ok(None)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    /// TLS-116 (Fable r2 M3): an inode number the snapshot holds, reused by a
    /// file born later, is not that cache file; the same number with the same
    /// birth time is (a link keeps it).
    ///
    /// Discrimination (2026-11-22, r3): `holds` ignoring the birth time
    /// (`b == born` made `true`): fails "a reused number"; restored, passes.
    #[test]
    fn a_reused_inode_number_is_not_the_cache_file() {
        let t = tempfile::tempdir().unwrap();
        let f = t.path().join("f");
        std::fs::write(&f, "").unwrap();
        let m = std::fs::metadata(&f).unwrap();
        let id = super::super::manifest::read_file_id(&f).unwrap().unwrap();
        assert_ne!(born(&m), 0, "precondition: the file system gives a btime");
        let mut snap = Snapshot::default();
        snap.files.insert(id, born(&m));
        assert!(snap.is_cache_file(&id, &m), "the same file");
        snap.files.insert(id, born(&m) - 1);
        assert!(!snap.is_cache_file(&id, &m), "a reused number");
    }

    /// TLS-120: a file that is a mount of its own (a bind mount of a file keeps the device and
    /// inode of the file it shows) may have another name; a plain file with one link may not.
    /// The bind-mounted file is any regular file `/proc/self/mountinfo` names as a mount point
    /// (a container's `/etc/hosts`); with none the mount half is skipped, said on stderr.
    ///
    /// Discrimination (2026-11-24): `mount_root` made `false`: fails "a mounted file";
    /// restored, passes.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_mounted_file_may_have_another_name() {
        let t = tempfile::NamedTempFile::new().unwrap();
        let plain = std::fs::File::open(t.path()).unwrap();
        assert!(
            !may_alias(&plain, &plain.metadata().unwrap()),
            "a plain file"
        );
        let info = std::fs::read_to_string("/proc/self/mountinfo").unwrap();
        let mounted = info
            .lines()
            .filter_map(|l| l.split(' ').nth(4))
            .filter(|p| std::fs::metadata(p).is_ok_and(|m| m.is_file()))
            .find_map(|p| std::fs::File::open(p).ok());
        match mounted {
            Some(f) => assert!(may_alias(&f, &f.metadata().unwrap()), "a mounted file"),
            None => eprintln!("no mounted regular file here: the mount half is skipped"),
        }
    }
}

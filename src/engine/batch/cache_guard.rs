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
//! decisions, a header of counts and a blake3 of the body, to the claim folder
//! ([`publish`]); a worker reads it at its start ([`load`]) and fails closed
//! when it is missing or damaged, so no worker walks again.
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
//! `ir/last-prune` pointing outside is still the prune's name); a write that
//! is a cache file under any name (a hard link, a bind mount of a file: the
//! handle's identity); a directory target that holds a cache folder; and a
//! write into the run's claim folder. A cache file published during the run is
//! not in the snapshot, but its folder is, so no job write reaches it. The
//! folder tests are path-based: a path another process changes during the run
//! is outside TLS-109's scope.

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
    for root in snap.roots.clone() {
        let fail = |e: std::io::Error| {
            format!(
                "al-sem batch: cannot list the cache folder {} ({e}): the engine's caches cannot be protected from the run's writes (TLS-116)",
                root.display()
            )
        };
        let m = std::fs::metadata(&root).map_err(fail)?;
        let Some(id) = super::manifest::read_file_id(&root).map_err(fail)? else {
            continue;
        };
        // The default `ir/` is inside the text folder: walked once.
        if snap.dirs.insert(id, born(&m)).is_none() {
            walk(&root, &mut snap).map_err(fail)?;
        }
    }
    snap.claims = claims_id()?;
    apply(&snap);
    Ok(snap)
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

/// Record every entry under `dir` (links not followed).
fn walk(dir: &Path, snap: &mut Snapshot) -> std::io::Result<()> {
    let names = match std::fs::read_dir(dir) {
        Ok(rd) => rd
            .map(|e| e.map(|e| e.path()))
            .collect::<Result<Vec<_>, _>>(),
        Err(e) => Err(e),
    };
    let names = match names {
        Ok(names) => names,
        Err(e) if gone(&e) => return Ok(()),
        Err(e) => return Err(e),
    };
    #[cfg(debug_assertions)]
    test_seam_during_snapshot();
    for p in names {
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
                walk(&p, snap)?;
            }
        } else {
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
    let header = format!(
        "{MAGIC} text={} ir={} dirs={} files={} blake3={}\n",
        u8::from(snap.text_on),
        u8::from(snap.ir_on),
        snap.dirs.len(),
        snap.files.len(),
        blake3::hash(body.as_bytes()).to_hex()
    );
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
    if blake3::hash(body.as_bytes()).to_hex().as_str() != field("blake3")? {
        return Err(bad("its blake3 does not match".into()));
    }
    let mut snap = Snapshot {
        text_on: field("text")? == "1",
        ir_on: field("ir")? == "1",
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
    apply(&snap);
    Ok(SNAPSHOT.get_or_init(|| snap))
}

/// Why the write `spelled` (against `cwd`) must not be made, `None` when it
/// may: its resolved destination lies in a cache folder, or its name as
/// spelled does (a cache could replace that name), or either lies in the run's
/// claim folder. Checked before anything is created; the file itself, once
/// open, is checked by its handle (`super::claim`).
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
    folder_hit(snap, folded.ancestors().skip(1))
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
}

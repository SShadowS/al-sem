//! TLS-116: the engine's cache folders, protected from a batch run's writes.
//!
//! A job write must not land on a cache file: a truncated text entry another
//! engine maps kills it (SIGBUS), and a job output in a cache folder is the
//! cache's to replace. The caches' own writes need no claim: every cache file
//! is made in its folder with a fresh name (an exclusive temp file, linked or
//! renamed into place through the folder's handle), lock files are opened
//! without truncation, and a prune only removes names in its folders.
//!
//! **One snapshot per run.** The parent's check ([`take`]) first MAKES both
//! cache folders exactly as the caches make them (ENG-113's text folder, and
//! the IR cache's `ir/` when this process uses it), so a spelling such as
//! `missing/..` resolves now as it will later. Then it records the identity of
//! every directory and file inside them ([`Snapshot`]; a name removed during
//! the walk, a concurrent prune, is skipped; any other error refuses the run:
//! the caches cannot be protected). The parent hands it to its workers in the
//! claim folder ([`publish`]), so no worker walks again.
//!
//! **What it refuses** (the check, exit 3; every claim again at its open,
//! `super::claim`): a write whose folders (links followed) include a cache
//! folder, a write that is a cache file under any name (a hard link, a bind
//! mount of a file: the handle's identity), and a directory target that holds
//! a cache folder. A cache file published during the run is not in the
//! snapshot, but its folder is, so no job write reaches it.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use super::manifest::FileId;

/// The identities of the cache folders and everything in them, at the check.
#[derive(Debug, Default)]
pub struct Snapshot {
    /// The cache folders, as made, for the check's messages and its "holds"
    /// test.
    pub(super) roots: Vec<PathBuf>,
    /// Every directory: the folders themselves and every one inside them.
    pub(super) dirs: HashSet<FileId>,
    /// Every other entry inside them (links are not followed or recorded).
    pub(super) files: HashSet<FileId>,
}

/// The claim folder's file holding the parent's snapshot.
const FILE: &str = "cache-snapshot";

/// The cache folders this process writes in, made now as the caches make
/// them; a folder that is no directory after that (an `ALSEM_IR_CACHE_DIR`
/// naming a file, a folder that cannot be made) is one no cache writes in.
fn make_roots() -> Vec<PathBuf> {
    // As `snapshot::cache::cache_dir` makes it, without its warning: a folder
    // that cannot be made is the cache's to report when it is used.
    let text = crate::snapshot::cache::cache_dir_path();
    let _ = std::fs::create_dir_all(&text);
    let mut roots = vec![text];
    roots.extend(crate::program::ir_cache::make_ir_dir());
    roots.retain(|r| r.is_dir());
    roots
}

/// The parent's snapshot: make the cache folders, then walk them. `Err`: the
/// check's one line.
pub(super) fn take() -> Result<Snapshot, String> {
    let _s = crate::engine::perf_trace::span("batch", "cache.snapshot");
    let mut snap = Snapshot {
        roots: make_roots(),
        ..Snapshot::default()
    };
    for root in snap.roots.clone() {
        let fail = |e: std::io::Error| {
            format!(
                "al-sem batch: cannot list the cache folder {} ({e}): the engine's caches cannot be protected from the run's writes (TLS-116)",
                root.display()
            )
        };
        let Some(id) = super::manifest::read_file_id(&root).map_err(fail)? else {
            continue;
        };
        // The default `ir/` is inside the text folder: walked once.
        if snap.dirs.insert(id) {
            walk(&root, &mut snap).map_err(fail)?;
        }
    }
    Ok(snap)
}

/// Whether `e` says the name is gone (a prune removed it during the walk).
fn gone(e: &std::io::Error) -> bool {
    matches!(
        e.kind(),
        std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
    )
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
            if snap.dirs.insert(id) {
                walk(&p, snap)?;
            }
        } else {
            snap.files.insert(id);
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
/// [`super::manifest::read_file_id`]); `None` when the name is gone.
#[cfg(windows)]
fn entry_id(p: &Path, _m: &std::fs::Metadata) -> std::io::Result<Option<FileId>> {
    super::manifest::read_file_id(p)
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

/// The parent, once its check has passed: keep `snap` for its own opens and
/// write it to the claim folder for its workers.
pub(super) fn publish(snap: Snapshot, dir: &Path) -> std::io::Result<()> {
    use std::fmt::Write as _;
    let mut text = String::new();
    for (kind, ids) in [("d", &snap.dirs), ("f", &snap.files)] {
        for (dev, ino) in ids {
            let _ = writeln!(text, "{kind} {dev} {ino}");
        }
    }
    std::fs::write(dir.join(FILE), text)?;
    let _ = SNAPSHOT.set(snap);
    Ok(())
}

/// This run's snapshot: the parent's own, or (a worker) the parent's read
/// from the claim folder once. `Err` (no snapshot): every write is refused.
pub(super) fn get(dir: &Path) -> std::io::Result<&'static Snapshot> {
    if let Some(s) = SNAPSHOT.get() {
        return Ok(s);
    }
    let text = std::fs::read_to_string(dir.join(FILE)).map_err(|e| {
        std::io::Error::other(format!(
            "refused: no cache snapshot from the run's parent ({e}) (TLS-116)"
        ))
    })?;
    let mut snap = Snapshot::default();
    for line in text.lines() {
        let bad = || std::io::Error::other(format!("a damaged cache snapshot line: {line}"));
        let mut parts = line.split(' ');
        let (Some(kind), Some(dev), Some(ino), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return Err(bad());
        };
        let id = (
            dev.parse().map_err(|_| bad())?,
            ino.parse().map_err(|_| bad())?,
        );
        match kind {
            "d" => snap.dirs.insert(id),
            "f" => snap.files.insert(id),
            _ => return Err(bad()),
        };
    }
    Ok(SNAPSHOT.get_or_init(|| snap))
}

/// Whether `path` lies in a cache folder (or is one), by the identities of its
/// existing folders (links followed, a dangling link through its text:
/// TLS-109's `resolve`). Checked before anything is created; the file itself,
/// once open, is checked by its handle (`super::claim`). Path-based: a path
/// another process changes during the run is outside TLS-109's scope.
pub(super) fn in_cache_folder(snap: &Snapshot, path: &Path) -> std::io::Result<bool> {
    let (chain, _) = super::manifest::identity_chain(path)?;
    Ok(chain.iter().any(|id| snap.dirs.contains(id)))
}

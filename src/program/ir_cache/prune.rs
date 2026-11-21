//! Best-effort maintenance of the `ir/` tree (spec Design 9): each key folder's `last-used`,
//! and a prune at most hourly that removes old key folders, then the oldest until the tree is
//! under its soft target, and stale temp files. Only this cache's exact name shapes are
//! touched, through checked handles, under the key folder's lock.

use crate::snapshot::cache::{CacheDir, LockKind};
use std::time::{Duration, SystemTime};

use super::key::IrKey;
use super::seam::key_lock_name;

const HOUR: Duration = Duration::from_secs(3600);
const DAY: Duration = Duration::from_secs(24 * 3600);
/// Prune at most this often.
const PRUNE_EVERY: Duration = HOUR;
/// `last-used` is published again when older than this.
const LAST_USED_REFRESH: Duration = DAY;
/// Key folders unused this long are removed.
const KEY_MAX_AGE: Duration = Duration::from_secs(7 * 24 * 3600);
/// Temp files this old are a dead writer's leftovers.
const TEMP_MAX_AGE: Duration = DAY;
/// The `ir/` tree's soft target (spec Design 8).
pub(crate) const SOFT_TARGET: u64 = 4 << 30;

const LAST_USED: &str = "last-used";
const LAST_PRUNE: &str = "last-prune";
const PRUNE_LOCK: &str = "prune.lock";

fn is_hex64(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// A temp file of `CacheDir::write`'s shape for one of this cache's names:
/// `<stem>-<pid>-<n>-<nanos>.tmp`, the stem a 64-hex entry name, `last-used` or `last-prune`.
fn is_temp(name: &str) -> bool {
    let Some(rest) = name.strip_suffix(".tmp") else {
        return false;
    };
    let mut parts = rest.rsplitn(4, '-');
    let nums: Vec<_> = parts.by_ref().take(3).collect();
    let stem = parts.next().unwrap_or("");
    nums.len() == 3
        && nums
            .iter()
            .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
        && (is_hex64(stem) || stem == LAST_USED || stem == LAST_PRUNE)
}

/// A file a key folder may hold.
fn is_key_folder_file(name: &str) -> bool {
    name == LAST_USED
        || is_temp(name)
        || name
            .strip_suffix(".bin")
            .or_else(|| name.strip_suffix(".lock"))
            .is_some_and(is_hex64)
}

/// A timestamp file's time (seconds since the epoch, as text). A damaged stamp (not a number,
/// or a time `SystemTime` cannot hold) reads as absent, never a panic (sol r1).
fn read_stamp(d: &CacheDir<'_>, name: &str) -> Option<SystemTime> {
    let b = d.read_owned(name, 64).ok()??;
    let secs: u64 = std::str::from_utf8(&b).ok()?.trim().parse().ok()?;
    SystemTime::UNIX_EPOCH.checked_add(Duration::from_secs(secs))
}

/// Publish timestamp `name` as `now`, by rename (never written in place).
fn write_stamp(d: &CacheDir<'_>, name: &str, now: SystemTime) {
    let secs = now
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let _ = d.write(name, true, |o| {
        Ok(std::io::Write::write_all(o, secs.to_string().as_bytes())?)
    });
}

fn older(t: SystemTime, now: SystemTime, age: Duration) -> bool {
    now.duration_since(t).is_ok_and(|d| d > age)
}

/// Publish `key_dir`'s `last-used` when it is missing, when `force` (a process's first use),
/// or when older than a day.
pub(crate) fn touch_last_used(key_dir: &CacheDir<'_>, force: bool, now: SystemTime) {
    if force || read_stamp(key_dir, LAST_USED).is_none_or(|t| older(t, now, LAST_USED_REFRESH)) {
        write_stamp(key_dir, LAST_USED, now);
    }
}

/// The modification time of `ir_dir`'s child `name` (not followed).
fn mtime(ir_dir: &CacheDir<'_>, name: &str) -> Option<SystemTime> {
    std::fs::symlink_metadata(ir_dir.path().join(name))
        .ok()?
        .modified()
        .ok()
}

/// The names in a directory (empty on error).
fn names(d: &CacheDir<'_>) -> Vec<String> {
    std::fs::read_dir(d.path()).map_or_else(
        |_| Vec::new(),
        |rd| {
            rd.filter_map(Result::ok)
                .filter_map(|e| e.file_name().into_string().ok())
                .collect()
        },
    )
}

/// Bytes of a key folder's files.
fn folder_size(kd: &CacheDir<'_>) -> u64 {
    std::fs::read_dir(kd.path()).map_or(0, |rd| {
        rd.filter_map(Result::ok)
            .filter_map(|e| e.metadata().ok())
            .map(|m| m.len())
            .sum()
    })
}

/// Remove key folder `name` of `ir_dir`: its files of this cache's shapes, then the folder (a
/// folder that still holds a foreign name stays).
fn remove_key_folder(ir_dir: &CacheDir<'_>, kd: &CacheDir<'_>, name: &str) {
    for f in names(kd) {
        if is_key_folder_file(&f) {
            kd.unlink(&f);
        }
    }
    ir_dir.remove_dir(name);
}

/// Remove `d`'s temp files of this cache's shape older than a day.
fn remove_stale_temps(d: &CacheDir<'_>, now: SystemTime) {
    for f in names(d) {
        if is_temp(&f) && mtime(d, &f).is_some_and(|t| older(t, now, TEMP_MAX_AGE)) {
            d.unlink(&f);
        }
    }
}

/// The prune (spec Design 9), at most hourly: `last-prune` older than an hour, `prune.lock`
/// taken without waiting, `last-prune` read again under it. Then key folders unused for 7
/// days go (a folder without `last-used` aged by its own mtime, never by size); then the
/// oldest until the tree is under [`SOFT_TARGET`], never `own_key`'s; then temp files older
/// than a day. A key folder goes only under its lock (`ir/<key>.lock`, outside the folder,
/// never removed) taken exclusive without waiting, with `last-used` read under it: a
/// publisher or `last-used` writer holds it shared, so a folder in use is skipped.
pub(crate) fn maybe_prune(ir_dir: &CacheDir<'_>, own_key: &IrKey, now: SystemTime) {
    if prune_due(ir_dir, now) {
        prune_locked(ir_dir, own_key, now);
    }
}

fn prune_due(ir_dir: &CacheDir<'_>, now: SystemTime) -> bool {
    read_stamp(ir_dir, LAST_PRUNE).is_none_or(|t| older(t, now, PRUNE_EVERY))
}

/// [`maybe_prune`] past its first, unlocked look at `last-prune`: the lock, the second look,
/// the prune.
fn prune_locked(ir_dir: &CacheDir<'_>, own_key: &IrKey, now: SystemTime) {
    prune_locked_with(ir_dir, own_key, now, || {});
}

/// [`prune_locked`], running `between` after the age pass and before the size pass (a test
/// uses a folder there; production passes nothing).
fn prune_locked_with(
    ir_dir: &CacheDir<'_>,
    own_key: &IrKey,
    now: SystemTime,
    between: impl FnOnce(),
) {
    let Some(_lock) = ir_dir.lock(PRUNE_LOCK, LockKind::ExclusiveNow) else {
        return;
    };
    // Another process may have pruned between the first look and the lock.
    if !prune_due(ir_dir, now) {
        return;
    }
    write_stamp(ir_dir, LAST_PRUNE, now);
    let own = own_key.hex();
    // (name, last used, size) of the folders the size rule may remove.
    let mut sized: Vec<(String, SystemTime, u64)> = Vec::new();
    let mut total = 0u64;
    for name in names(ir_dir) {
        if !is_hex64(&name) {
            continue;
        }
        let Some(kd) = ir_dir.subdir(&name, false) else {
            continue;
        };
        let size = folder_size(&kd);
        total += size;
        remove_stale_temps(&kd, now);
        if name == own {
            continue;
        }
        let Some(_held) = ir_dir.lock(&key_lock_name(&name), LockKind::ExclusiveNow) else {
            continue; // a publisher or a `last-used` writer is in it
        };
        match read_stamp(&kd, LAST_USED) {
            // No `last-used`: a writer that died before its first publish, or a folder from
            // elsewhere: aged by the folder's own mtime, never by size.
            None => {
                if mtime(ir_dir, &name).is_some_and(|t| older(t, now, KEY_MAX_AGE)) {
                    remove_key_folder(ir_dir, &kd, &name);
                    total -= size;
                }
            }
            Some(t) if older(t, now, KEY_MAX_AGE) => {
                remove_key_folder(ir_dir, &kd, &name);
                total -= size;
            }
            Some(t) => sized.push((name, t, size)),
        }
    }
    between();
    sized.sort_by_key(|(_, t, _)| *t);
    for (name, used, size) in sized {
        if total <= SOFT_TARGET {
            break;
        }
        let Some(kd) = ir_dir.subdir(&name, false) else {
            continue;
        };
        let Some(_held) = ir_dir.lock(&key_lock_name(&name), LockKind::ExclusiveNow) else {
            continue;
        };
        // Read again under the lock: a process that used the folder since keeps it.
        if read_stamp(&kd, LAST_USED) != Some(used) {
            continue;
        }
        remove_key_folder(ir_dir, &kd, &name);
        total = total.saturating_sub(size);
    }
    remove_stale_temps(ir_dir, now);
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::DirBuilderExt;
    use std::path::Path;

    const NOW: u64 = 2_000_000_000;

    fn at(secs: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(secs)
    }

    fn mkdir(p: &Path) {
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(p)
            .unwrap();
    }

    fn key(n: u8) -> IrKey {
        IrKey([n; 32])
    }

    /// A key folder with an entry of `size` bytes, used at `used` (`None`: no markers).
    fn key_folder(ir: &Path, k: &IrKey, used: Option<u64>, size: usize) {
        let kd = ir.join(k.hex());
        mkdir(&kd);
        std::fs::write(kd.join(format!("{}.bin", "a".repeat(64))), vec![0u8; size]).unwrap();
        if let Some(u) = used {
            std::fs::write(kd.join(LAST_USED), u.to_string()).unwrap();
        }
    }

    fn ir_dir() -> (tempfile::TempDir, std::path::PathBuf) {
        let t = tempfile::tempdir().unwrap();
        let ir = t.path().join("ir");
        mkdir(&ir);
        (t, ir)
    }

    fn open(p: &Path) -> CacheDir<'_> {
        CacheDir::open(p).unwrap()
    }

    #[test]
    fn prune_runs_at_most_hourly() {
        let (_t, ir) = ir_dir();
        key_folder(&ir, &key(1), Some(NOW - 8 * 86400), 10);
        std::fs::write(ir.join(LAST_PRUNE), (NOW - 600).to_string()).unwrap();
        maybe_prune(&open(&ir), &key(9), at(NOW));
        assert!(
            ir.join(key(1).hex()).exists(),
            "pruned 10 minutes ago: not now"
        );
        maybe_prune(&open(&ir), &key(9), at(NOW + 3100));
        assert!(!ir.join(key(1).hex()).exists(), "over an hour: pruned");
    }

    /// A second caller after the first published `last-prune` does nothing: the stamp is
    /// read again under the lock.
    #[test]
    fn prune_rechecks_timestamp_under_lock() {
        let (_t, ir) = ir_dir();
        key_folder(&ir, &key(1), Some(NOW - 8 * 86400), 10);
        let d = open(&ir);
        // The first caller holds the lock and has just published `last-prune`; the second
        // read the old stamp (none) before it.
        let held = d.lock(PRUNE_LOCK, LockKind::Exclusive).unwrap();
        maybe_prune(&d, &key(9), at(NOW));
        assert!(ir.join(key(1).hex()).exists(), "busy lock: skipped");
        write_stamp(&d, LAST_PRUNE, at(NOW));
        drop(held);
        // The second caller is past its first look (it saw the stamp due) when it gets the lock.
        prune_locked(&d, &key(9), at(NOW + 60));
        assert!(
            ir.join(key(1).hex()).exists(),
            "fresh stamp under the lock: skipped"
        );
    }

    #[test]
    fn old_key_folder_removed_fresh_kept() {
        let (_t, ir) = ir_dir();
        key_folder(&ir, &key(1), Some(NOW - 8 * 86400), 10);
        key_folder(&ir, &key(2), Some(NOW - 86400), 10);
        maybe_prune(&open(&ir), &key(9), at(NOW));
        assert!(!ir.join(key(1).hex()).exists());
        assert!(ir.join(key(2).hex()).exists());
    }

    /// Over the soft target the oldest folders go first, never this process's own.
    #[test]
    fn own_key_never_removed_by_size() {
        let (_t, ir) = ir_dir();
        // Sparse files: sizes count without the bytes on disk.
        let big = |k: &IrKey, used: u64| {
            key_folder(&ir, k, Some(used), 0);
            let f = std::fs::OpenOptions::new()
                .write(true)
                .open(ir.join(k.hex()).join(format!("{}.bin", "a".repeat(64))))
                .unwrap();
            f.set_len(SOFT_TARGET / 3 + 1).unwrap();
        };
        big(&key(1), NOW - 3 * 86400); // own, oldest
        big(&key(2), NOW - 2 * 86400);
        big(&key(3), NOW - 86400);
        maybe_prune(&open(&ir), &key(1), at(NOW));
        assert!(ir.join(key(1).hex()).exists(), "own key kept");
        assert!(!ir.join(key(2).hex()).exists(), "the oldest other removed");
        assert!(ir.join(key(3).hex()).exists(), "under the target again");
    }

    #[test]
    fn folder_with_live_publisher_is_skipped() {
        let (_t, ir) = ir_dir();
        key_folder(&ir, &key(1), Some(NOW - 8 * 86400), 10);
        let kd_path = ir.join(key(1).hex());
        let shared = open(&ir)
            .lock(&key_lock_name(&key(1).hex()), LockKind::SharedNow)
            .unwrap();
        maybe_prune(&open(&ir), &key(9), at(NOW));
        assert!(kd_path.exists(), "a publisher holds it");
        drop(shared);
        maybe_prune(&open(&ir), &key(9), at(NOW + 2 * 3600));
        assert!(!kd_path.exists());
        assert!(
            ir.join(key_lock_name(&key(1).hex())).exists(),
            "the lock file stays: never unlinked while it could be held"
        );
    }

    /// Fable/sol r1: a first publisher has made its key folder and holds its lock, but has
    /// written no `last-used` yet; the folder's mtime says it is old. The prune leaves it.
    #[test]
    fn publisher_paused_before_last_used_keeps_its_folder() {
        let (_t, ir) = ir_dir();
        key_folder(&ir, &key(1), None, 10);
        let kd_path = ir.join(key(1).hex());
        let real_now = SystemTime::now();
        let shared = open(&ir)
            .lock(&key_lock_name(&key(1).hex()), LockKind::SharedNow)
            .unwrap();
        maybe_prune(&open(&ir), &key(9), real_now + KEY_MAX_AGE + 2 * HOUR);
        assert!(kd_path.exists(), "the paused publisher's folder stays");
        drop(shared);
        maybe_prune(&open(&ir), &key(9), real_now + KEY_MAX_AGE + 4 * HOUR);
        assert!(!kd_path.exists(), "released: aged by its mtime, removed");
    }

    /// Over the soft target, a folder used again between the size pass's first look and its
    /// lock is kept (its `last-used` is read again under the lock).
    #[test]
    fn size_rule_rereads_last_used_under_the_lock() {
        let (_t, ir) = ir_dir();
        let big = |k: &IrKey, used: u64| {
            key_folder(&ir, k, Some(used), 0);
            std::fs::OpenOptions::new()
                .write(true)
                .open(ir.join(k.hex()).join(format!("{}.bin", "a".repeat(64))))
                .unwrap()
                .set_len(SOFT_TARGET / 2 + 1)
                .unwrap();
        };
        big(&key(1), NOW - 3 * 86400);
        big(&key(2), NOW - 2 * 86400);
        // The age pass reads key 1 as the oldest; between the passes a process uses it.
        let used_again = ir.join(key(1).hex()).join(LAST_USED);
        prune_locked_with(&open(&ir), &key(9), at(NOW), || {
            std::fs::write(&used_again, (NOW - 60).to_string()).unwrap();
        });
        assert!(ir.join(key(1).hex()).exists(), "used again: kept");
        // Key 2 goes: once key 1 is skipped, the pass takes the next oldest.
        assert!(!ir.join(key(2).hex()).exists());
    }

    /// sol r1: a damaged stamp (a number past what `SystemTime` holds) reads as absent.
    #[test]
    fn a_damaged_stamp_reads_as_absent() {
        let (_t, ir) = ir_dir();
        std::fs::write(ir.join(LAST_PRUNE), u64::MAX.to_string()).unwrap();
        assert_eq!(read_stamp(&open(&ir), LAST_PRUNE), None);
        maybe_prune(&open(&ir), &key(9), at(NOW));
    }

    #[test]
    fn folder_without_markers_aged_by_mtime() {
        let (_t, ir) = ir_dir();
        // Fresh by its mtime (now), unmarked: kept even under an old `now`-relative stamp.
        key_folder(&ir, &key(1), None, 10);
        let real_now = SystemTime::now();
        maybe_prune(&open(&ir), &key(9), real_now);
        assert!(ir.join(key(1).hex()).exists(), "young by mtime");
        maybe_prune(&open(&ir), &key(9), real_now + KEY_MAX_AGE + 2 * HOUR);
        assert!(!ir.join(key(1).hex()).exists(), "old by mtime");
    }

    #[test]
    fn foreign_names_left_alone() {
        let (_t, ir) = ir_dir();
        std::fs::write(ir.join("notes.txt"), b"mine").unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(elsewhere.path(), ir.join(key(4).hex())).unwrap();
        std::fs::write(elsewhere.path().join("keep.bin"), b"x").unwrap();
        key_folder(&ir, &key(1), Some(NOW - 8 * 86400), 10);
        std::fs::write(ir.join(key(1).hex()).join("foreign.txt"), b"x").unwrap();
        maybe_prune(&open(&ir), &key(9), at(NOW));
        assert!(ir.join("notes.txt").exists());
        assert!(
            elsewhere.path().join("keep.bin").exists(),
            "symlink not followed"
        );
        assert!(
            ir.join(key(1).hex()).join("foreign.txt").exists(),
            "a foreign file keeps its folder"
        );
        assert!(
            !ir.join(key(1).hex()).join(LAST_USED).exists(),
            "this cache's own files went"
        );
    }

    /// Review Focus 4: a killed writer's temp file is removed after a day, a fresh one kept.
    #[test]
    fn killed_writer_temp_pruned_after_a_day() {
        let (_t, ir) = ir_dir();
        key_folder(&ir, &key(1), Some(NOW), 10);
        let kd = ir.join(key(1).hex());
        let tmp = kd.join(format!("{}-123-0-456.tmp", "b".repeat(64)));
        std::fs::write(&tmp, b"half").unwrap();
        let real_now = SystemTime::now();
        maybe_prune(&open(&ir), &key(9), real_now);
        assert!(tmp.exists(), "young");
        maybe_prune(&open(&ir), &key(9), real_now + DAY + 2 * HOUR);
        assert!(!tmp.exists(), "a day old");
    }

    #[test]
    fn last_used_refreshed_daily_or_forced() {
        let (_t, ir) = ir_dir();
        key_folder(&ir, &key(1), Some(NOW), 10);
        let kd_path = ir.join(key(1).hex());
        let kd = open(&kd_path);
        touch_last_used(&kd, false, at(NOW + 3600));
        assert_eq!(read_stamp(&kd, LAST_USED), Some(at(NOW)), "fresh: kept");
        touch_last_used(&kd, true, at(NOW + 3600));
        assert_eq!(read_stamp(&kd, LAST_USED), Some(at(NOW + 3600)), "forced");
        touch_last_used(&kd, false, at(NOW + 3600 + DAY.as_secs() + 1));
        assert_eq!(
            read_stamp(&kd, LAST_USED),
            Some(at(NOW + 3600 + DAY.as_secs() + 1)),
            "a day old: refreshed"
        );
    }

    #[test]
    fn name_shapes() {
        assert!(is_temp(&format!("{}-1-2-3.tmp", "a".repeat(64))));
        assert!(is_temp("last-used-1-2-3.tmp"));
        assert!(!is_temp("notes-1-2-3.tmp"));
        assert!(!is_temp(&format!("{}-1-2.tmp", "a".repeat(64))));
        assert!(is_key_folder_file(&format!("{}.bin", "a".repeat(64))));
        assert!(!is_key_folder_file("foreign.txt"));
    }
}

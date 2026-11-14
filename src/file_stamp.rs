//! TLS-57: the one rule for "has this file changed since I last read it?"
//! without reading it. Three readers share it: the resident engine's input
//! manifest (`engine::resident::manifest`), the LSP updater's per-file stamps
//! (`lsp::updater`) and test selection's scan cache
//! (`engine::test_select::scope`).
//!
//! A stamp is a file's length, modification time and status-change time
//! (ctime). A rewrite can keep length and mtime (`touch -r`,
//! `File::set_modified`) but not ctime, which no user call sets: a write
//! moves it to the file system's current timestamp tick. [`FileStamp::same_file`]
//! compares ctime only when the earlier stamp recorded one, so a stamp from a
//! platform with none, or one stored before ctime was kept, is checked by
//! length and mtime alone.
//!
//! **Stated residual** (what a stamp cannot see; each reader says it where it
//! answers): a same-size rewrite within one timestamp tick of the stamp
//! (ctime and mtime unmoved); a file system without a real change time (FAT,
//! some FUSE or network mounts); a platform whose standard library gives no
//! change time (Windows), where any rewrite that keeps length and mtime is
//! missed. A metadata-only change (chmod, chown) moves ctime too: it costs a
//! re-read, never a wrong answer.

use std::path::Path;
use std::time::SystemTime;

/// A file's length, modification time and change time when it was observed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FileStamp {
    len: u64,
    mtime: Option<SystemTime>,
    /// Nanoseconds since the Unix epoch; `None` where the platform's std
    /// gives none (Windows) or the stamp was stored without one.
    ctime_ns: Option<i128>,
}

impl FileStamp {
    /// The stamp of `m`.
    #[must_use]
    pub fn of(m: &std::fs::Metadata) -> FileStamp {
        FileStamp {
            len: m.len(),
            mtime: m.modified().ok(),
            ctime_ns: ctime_ns(m),
        }
    }

    /// The stamp of the file at `path`, or `None` when it cannot be stat'ed.
    #[must_use]
    pub fn at(path: &Path) -> Option<FileStamp> {
        std::fs::metadata(path).ok().map(|m| FileStamp::of(&m))
    }

    /// A stamp rebuilt from its parts (a persisted descriptor).
    #[must_use]
    pub fn from_parts(len: u64, mtime: Option<SystemTime>, ctime_ns: Option<i128>) -> FileStamp {
        FileStamp {
            len,
            mtime,
            ctime_ns,
        }
    }

    /// The file's length in bytes.
    #[must_use]
    pub fn file_len(&self) -> u64 {
        self.len
    }

    #[must_use]
    pub fn mtime(&self) -> Option<SystemTime> {
        self.mtime
    }

    #[must_use]
    pub fn ctime_ns(&self) -> Option<i128> {
        self.ctime_ns
    }

    /// The same stamp without its ctime: later comparisons against it use
    /// length and mtime alone (a test that needs a rewrite the stamp misses,
    /// as on a platform with no ctime).
    #[must_use]
    pub fn without_ctime(self) -> FileStamp {
        FileStamp {
            ctime_ns: None,
            ..self
        }
    }

    /// Whether `now` is the file this stamp saw, so what was read then may be
    /// trusted: the same length and mtime, and the same ctime when this stamp
    /// recorded one (the module doc's residual).
    #[must_use]
    pub fn same_file(&self, now: &FileStamp) -> bool {
        self.len == now.len
            && self.mtime == now.mtime
            && (self.ctime_ns.is_none() || self.ctime_ns == now.ctime_ns)
    }
}

#[cfg(unix)]
fn ctime_ns(m: &std::fs::Metadata) -> Option<i128> {
    use std::os::unix::fs::MetadataExt;
    Some(i128::from(m.ctime()) * 1_000_000_000 + i128::from(m.ctime_nsec()))
}

#[cfg(not(unix))]
fn ctime_ns(_: &std::fs::Metadata) -> Option<i128> {
    None
}

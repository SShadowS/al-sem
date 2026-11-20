//! Content-addressed source cache for embedded `.app` extraction.
//!
//! The cache key is the blake3 hex of the whole `.app` image, so a changed
//! app is always a different key — stale reads are structurally impossible.
//! The entries are the one mapping of package-derived bytes allowed: sealed
//! private files, written once and renamed into place, never modified.
//!
//! ENG-113 (design note `docs/superpowers/notes/2026-11-18-eng113-mapped-dependency-text.md`):
//! one file per package holds a header, an index and every file's decoded text, and a hit
//! hands out [`SourceText`]s that are ranges of a read-only mapping of that file, so the page
//! cache holds a package's text once for every engine process. A miss writes the file and then
//! reads it back the same way, so package text has one form when the cache is writable.

use crate::app_image::AppImage;
use crate::snapshot::embedded::{SourceFile, extract_embedded_source};
use crate::source_text::SourceText;
use anyhow::{Context, Result, bail};
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Return the stable cache directory, creating it if necessary.
///
/// Uses the OS cache directory (`dirs::cache_dir`) when available, with
/// `al-ch-snapshot-cache` appended.  Falls back to
/// `<temp_dir>/al-ch-snapshot-cache` when the OS cache dir is unavailable.
pub fn cache_dir() -> PathBuf {
    let base = dirs::cache_dir().unwrap_or_else(std::env::temp_dir);
    let dir = base.join("al-ch-snapshot-cache");
    if let Err(e) = std::fs::create_dir_all(&dir) {
        log::warn!("Failed to create snapshot cache dir {}: {e}", dir.display());
    }
    dir
}

/// The version of what extraction produces for a package, in the cache file's
/// name. Bump it when [`extract_embedded_source`]'s output for an unchanged
/// `.app` changes (2: AL synthesized from transpiled C#; its own changes too;
/// 3: synthesized routines carry their `[MethodId(n)]`; 4: the id is quoted,
/// since a negative one made the file `Recovered`; 5: generic types split no
/// local, and a call keeps its receiver's member path; 6: a property-backed
/// global `δx` is `x`; 7: an `if` on never-written Boolean parameters and
/// locals is written out, G51; 8: an interface's `extends` (ENG-142) and a
/// codeunit's `implements` (ENG-148) are written, every interface name quoted;
/// 9: an interface reference keeps its app as a `"#<guid>#".` qualifier, ENG-152).
pub const EXTRACTION_SCHEME: u32 = 9;

/// The cache file's layout, in its name and header. 1 (ENG-113): header, JSON index, text.
/// 2 (sol review): the blake3 also binds the package's content hash (an entry copied under
/// another package's name is refused), and the index must tile the text exactly. Before 1, a
/// package's cache was one JSON document (`<hash>.s<scheme>.json`), which the new name never
/// matches, so an old cache misses cleanly.
pub const CACHE_FORMAT: u32 = 2;

const MAGIC: &[u8; 8] = b"ALSEMSRC";
/// Magic, format, scheme, index length, text length, the blake3 of the package hash, the index
/// and the text.
const HEADER_LEN: usize = 8 + 4 + 4 + 8 + 8 + 32;
/// The largest index read (a package's paths; CDO's largest is well under 1 MiB): a damaged or
/// hostile length cannot make the reader parse an unbounded JSON document.
const MAX_INDEX_LEN: usize = 64 << 20;

/// One file's place in the text, and its path.
#[derive(serde::Serialize, serde::Deserialize)]
struct IndexEntry {
    path: String,
    offset: u64,
    len: u64,
}

/// Return embedded AL source for the package `image`, using the
/// content-addressed on-disk cache when available.
///
/// On a miss the source is extracted from the image, written to
/// `<cache_dir>/<blake3-hex>.s<EXTRACTION_SCHEME>.f<CACHE_FORMAT>.bin` and read back from it;
/// on a hit it is read from that file. Either way the texts are ranges of a mapping of the
/// file.
///
/// Returns `(files, content_hash)`.  `files` is empty for symbol-only apps
/// (same semantics as [`extract_embedded_source`]).  The hash is the image's
/// blake3 hex ([`AppImage::content_hash`]): the key and the extraction come
/// from the SAME bytes, so a package rewritten after its hash was taken can
/// never be extracted under the old key (context-server design r5 §5.2).
///
/// Cache I/O failures are **non-fatal** and logged at warn: a corrupt entry is extracted
/// again and rewritten; a cache that cannot be written or read back (unwritable dir, locked
/// file) leaves the extraction as owned text.
pub fn cached_source(image: &Arc<AppImage>) -> Result<(Vec<SourceFile>, String)> {
    cached_source_in(&cache_dir(), image)
}

/// [`cached_source`] with the cache directory given (tests write a temporary one).
pub fn cached_source_in(dir: &Path, image: &Arc<AppImage>) -> Result<(Vec<SourceFile>, String)> {
    let hash = image.content_hash().to_string();
    // The extraction scheme and the cache format are part of the name, not the hash (the hash
    // stays the app's identity): a package with only transpiled C# extracts to synthesized AL
    // since scheme 2, so an older entry must not be read for it.
    let name = format!("{hash}.s{EXTRACTION_SCHEME}.f{CACHE_FORMAT}.bin");

    // A cache directory another user could write is not used at all: its entries could change
    // under a map, and writing into it would hand them our source (sol review).
    let Some(d) = CacheDir::open(dir) else {
        log::warn!(
            "snapshot cache dir {} is missing or not private to this user: source of {} kept \
             as owned text",
            dir.display(),
            image.path().display()
        );
        return Ok((extract_embedded_source(image)?, hash));
    };

    // Cache hit; fall through to re-extract on a missing or refused entry.
    match d.read(&name, &hash) {
        Ok(Some(files)) => return Ok((files, hash)),
        Ok(None) => {}
        Err(e) => log::warn!(
            "snapshot cache entry {} refused ({e:#}); re-extracting",
            dir.join(&name).display()
        ),
    }

    // Cache miss (or a refused entry): extract, publish, then read the published file, so the
    // text is the mapped form.
    let files = extract_embedded_source(image)?;
    let files = publish(&d, &name, &hash, files);
    Ok((files, hash))
}

/// Publish package `hash`'s extracted `files` as entry `name` of `d` and return the entry's
/// mapped texts, or `files` (owned) when the cache cannot be used. Every publish holds the
/// package's lock and reads the entry again under it, so no two writers decide on the same
/// entry at once: an entry another process published meanwhile is used (every engine then maps
/// the one file); a missing one is published without replacing; a refused one is replaced.
/// Without the lock nothing is published (sol): an entry another process published is still
/// used, otherwise the text stays owned.
fn publish(d: &CacheDir<'_>, name: &str, hash: &str, files: Vec<SourceFile>) -> Vec<SourceFile> {
    let path = d.path.join(name);
    let lock = d.lock(hash);
    let replace = match d.read(name, hash) {
        Ok(Some(theirs)) => return theirs,
        _ if lock.is_none() => {
            log::warn!(
                "snapshot cache lock for {} not taken: nothing published, source kept as owned \
                 text",
                path.display()
            );
            return files;
        }
        Ok(None) => false,
        Err(_) => true,
    };
    if let Err(e) = d.write(name, hash, &files, replace) {
        log::warn!(
            "snapshot source cache write failed for {} ({e:#})",
            path.display()
        );
    }
    // Read back even when this write failed: another process may have just written the same
    // entry (on Windows a rename over a mapped file fails).
    match d.read(name, hash) {
        Ok(Some(mapped)) => return mapped,
        Ok(None) => {}
        Err(e) => log::warn!(
            "snapshot cache entry {} refused after writing ({e:#})",
            path.display()
        ),
    }
    log::warn!(
        "snapshot source cache entry {} unusable: source kept as owned text",
        path.display()
    );
    drop(lock);
    files
}

/// Whether `m` is this user's and writable by nobody else (Unix): a mapping is only sound while
/// nobody else can rewrite or truncate the file (sol review).
#[cfg(unix)]
fn private(m: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    // SAFETY: no arguments, cannot fail.
    m.uid() == unsafe { libc::geteuid() } && m.mode() & 0o022 == 0
}

/// An open cache directory. On Unix it holds the directory open, checked [`private`] on the
/// opened handle, and every entry is opened relative to it, so the directory checked is the
/// one read.
struct CacheDir<'a> {
    path: &'a Path,
    #[cfg(unix)]
    fd: std::fs::File,
}

impl<'a> CacheDir<'a> {
    /// `None` when `path` is not a directory, or (Unix) not [`private`].
    fn open(path: &'a Path) -> Option<CacheDir<'a>> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            let fd = std::fs::OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_DIRECTORY)
                .open(path)
                .ok()?;
            let m = fd.metadata().ok()?;
            (m.is_dir() && private(&m)).then_some(CacheDir { path, fd })
        }
        #[cfg(not(unix))]
        {
            std::fs::metadata(path)
                .ok()?
                .is_dir()
                .then_some(CacheDir { path })
        }
    }

    /// `openat` of `name` in the open directory, never following a symlink or blocking (a FIFO
    /// would block the open), with `flags` and creation `mode` added.
    #[cfg(unix)]
    fn openat(&self, name: &str, flags: libc::c_int, mode: libc::c_uint) -> std::io::Result<File> {
        use std::os::fd::{AsRawFd, FromRawFd};
        let c = std::ffi::CString::new(name)?;
        let flags = flags | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC;
        // SAFETY: a valid directory fd and a NUL-terminated name.
        let fd = unsafe { libc::openat(self.fd.as_raw_fd(), c.as_ptr(), flags, mode) };
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: `fd` was just opened and is owned by nobody else.
        Ok(unsafe { File::from_raw_fd(fd) })
    }

    /// Run `f` on the open directory's fd and two names of it, as a libc call returning 0 on
    /// success.
    #[cfg(unix)]
    fn at2(
        &self,
        a: &str,
        b: &str,
        f: impl FnOnce(libc::c_int, *const libc::c_char, *const libc::c_char) -> libc::c_int,
    ) -> std::io::Result<()> {
        use std::os::fd::AsRawFd;
        let (a, b) = (std::ffi::CString::new(a)?, std::ffi::CString::new(b)?);
        if f(self.fd.as_raw_fd(), a.as_ptr(), b.as_ptr()) == 0 {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error())
        }
    }

    /// Remove `name` from the open directory (best effort: a leftover temp file).
    #[cfg(unix)]
    fn unlink(&self, name: &str) {
        let _ = self.at2(name, name, |d, a, _| {
            // SAFETY: a valid directory fd and a NUL-terminated name.
            unsafe { libc::unlinkat(d, a, 0) }
        });
    }

    /// Open entry `name` read-only, relative to the open directory.
    #[cfg(unix)]
    fn open_entry(&self, name: &str) -> std::io::Result<File> {
        self.openat(name, libc::O_RDONLY, 0)
    }

    /// Open entry `name` read-only, not following a reparse point (a symlink), and sharing
    /// read and delete only, so an entry open for writing at that moment (a copy over it in
    /// progress) is refused, and nobody can open it for writing while the map lives (memmap2
    /// keeps a duplicate of this handle, the same file object, for the map's life). The cache
    /// directory is under this user's profile, which no other user can write.
    #[cfg(windows)]
    fn open_entry(&self, name: &str) -> std::io::Result<std::fs::File> {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_SHARE_READ: u32 = 0x1;
        const FILE_SHARE_DELETE: u32 = 0x4;
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        std::fs::OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_DELETE)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .open(self.path.join(name))
    }

    /// Read entry `name` for package `package_hash`: `None` when there is none; an error
    /// (refused) for anything but a regular file (checked on the OPENED file), on Unix one that
    /// is not [`private`], or one [`check_entry`] refuses. Else the texts are ranges of a
    /// read-only map of it.
    fn read(&self, name: &str, package_hash: &str) -> Result<Option<Vec<SourceFile>>> {
        let file = match self.open_entry(name) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e).with_context(|| format!("open cache entry {name}")),
        };
        let meta = file.metadata().context("stat cache entry")?;
        if !meta.file_type().is_file() {
            bail!("not a regular file");
        }
        #[cfg(unix)]
        if !private(&meta) {
            bail!("not private to this user");
        }
        // SAFETY: the opened entry is a regular file that no other user can write (Unix: it and
        // its directory are `private`; Windows: the per-user cache directory, and the map's
        // handle denies write sharing), and this code only ever replaces an entry whole, by a
        // rename of a unique temp file (`CacheDir::write`), never writes or truncates one in
        // place, so the mapped bytes never change. This user rewriting the cache in
        // place (cp, rsync --inplace, a restore) while an engine maps it still breaks this:
        // the process holding the map (an engine child, a one-shot CLI, the LSP server during a
        // build) can then die of SIGBUS or read changed text. The design note states the risk.
        let map = unsafe { crate::capped_io::map_read_only(&file) }
            .context("map cache entry")?
            .context("cache entry is empty")?;
        let map = Arc::new(map);
        let files = check_entry(&map, package_hash)?
            .into_iter()
            .map(|(path, r)| {
                let text =
                    SourceText::mapped(&map, r).with_context(|| format!("cache text of {path}"))?;
                Ok(SourceFile {
                    virtual_path: path,
                    text,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Some(files))
    }

    /// The publish lock of package `hash`, held while the returned value lives, so no two
    /// processes decide on one entry at once ([`publish`]). Unix: an exclusive `flock` on
    /// `<hash>.lock` in the open directory, which must be a regular, [`private`] file (`None`
    /// otherwise, or when the lock fails). Windows: no file lock (`Some` with nothing held): a
    /// rename over an entry another engine maps fails, so a replacement cannot remove an entry
    /// in use.
    fn lock(&self, hash: &str) -> Option<RepairLock> {
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            let f = self
                .openat(&format!("{hash}.lock"), libc::O_RDWR | libc::O_CREAT, 0o600)
                .ok()?;
            let m = f.metadata().ok()?;
            if !(m.is_file() && private(&m)) {
                return None;
            }
            // SAFETY: a valid fd.
            (unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX) } == 0)
                .then_some(RepairLock(Some(f)))
        }
        #[cfg(not(unix))]
        {
            let _ = hash;
            Some(RepairLock(None))
        }
    }

    /// Write `files` as entry `name`: the whole entry ([`write_entry`]) goes to a unique temp
    /// file in the directory (owner-only, created exclusively), which is then renamed into
    /// place. Two writers never share a temp file, so a rename only ever installs a complete
    /// file. Unless `replace`, an entry already there is kept (a no-clobber publish): another
    /// process published it, and every engine then maps the one file. On Unix every step goes
    /// through the open directory handle, so the entry lands in the directory that was checked.
    #[cfg(unix)]
    fn write(&self, name: &str, hash: &str, files: &[SourceFile], replace: bool) -> Result<()> {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        // A name taken (a crash's leftover, the same pid in another pid namespace) is skipped:
        // the exclusive create refuses it, and the next name differs in its counter and time.
        let (tmp, file) = (0..16)
            .find_map(|_| {
                let nanos = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.subsec_nanos());
                let tmp = format!(
                    "{hash}-{}-{}-{nanos}.tmp",
                    std::process::id(),
                    NEXT.fetch_add(1, Ordering::Relaxed)
                );
                match self.openat(&tmp, libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL, 0o600) {
                    Ok(f) => Some(Ok((tmp, f))),
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => None,
                    Err(e) => Some(Err(e).with_context(|| format!("create cache temp {tmp}"))),
                }
            })
            .context("no free cache temp name")??;
        let published =
            write_entry(&mut std::io::BufWriter::new(&file), hash, files).and_then(|()| {
                if replace {
                    self.at2(&tmp, name, |d, a, b| {
                        // SAFETY: a valid directory fd and NUL-terminated names.
                        unsafe { libc::renameat(d, a, d, b) }
                    })
                } else {
                    // A hard link fails when `name` exists: the no-clobber publish.
                    match self.at2(&tmp, name, |d, a, b| {
                        // SAFETY: a valid directory fd and NUL-terminated names.
                        unsafe { libc::linkat(d, a, d, b, 0) }
                    }) {
                        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
                        r => r,
                    }
                }
                .with_context(|| format!("publish cache temp {tmp} as {name}"))
            });
        // After a rename the temp name is gone; after a link (or a failure) it is removed.
        if !(replace && published.is_ok()) {
            self.unlink(&tmp);
        }
        published
    }

    #[cfg(windows)]
    fn write(&self, name: &str, hash: &str, files: &[SourceFile], replace: bool) -> Result<()> {
        let path = self.path.join(name);
        let mut tmp = tempfile::Builder::new()
            .prefix(&format!("{hash}-"))
            .suffix(".tmp")
            .tempfile_in(self.path)
            .with_context(|| format!("create cache temp in {}", self.path.display()))?;
        write_entry(&mut std::io::BufWriter::new(tmp.as_file_mut()), hash, files)?;
        if replace {
            tmp.persist(&path)
                .map_err(|e| e.error)
                .with_context(|| format!("rename cache temp → {}", path.display()))?;
        } else {
            match tmp.persist_noclobber(&path) {
                Ok(_) => {}
                // Another process published the entry first: keep theirs (the temp file drops).
                Err(e) if e.error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(e) => {
                    return Err(e.error)
                        .with_context(|| format!("rename cache temp → {}", path.display()));
                }
            }
        }
        Ok(())
    }
}

/// A held repair lock ([`CacheDir::lock`]); released when dropped.
struct RepairLock(#[allow(dead_code)] Option<File>);

/// A cache entry's index, checked against `bytes` (the whole entry) and the package
/// `package_hash`: the header, the format and scheme, the lengths (the index capped at
/// [`MAX_INDEX_LEN`]), the blake3 of the package hash, the index and the text, then the index
/// entries, which must tile the text exactly in order. Returns `[(path, range)]` with ranges
/// into `bytes`; each range's UTF-8 is checked when its text is made.
///
/// The blake3 check is NOT optional: nothing fsyncs, so after a crash a renamed file can have
/// the right length and zero content, which the length and UTF-8 checks both accept. It
/// covers the index and binds the package, so a damaged offset or path that is still valid
/// JSON, or a valid entry copied under another package's name, is refused.
fn check_entry(bytes: &[u8], package_hash: &str) -> Result<Vec<(String, std::ops::Range<usize>)>> {
    if bytes.len() < HEADER_LEN || &bytes[..8] != MAGIC {
        bail!("not a cache file (header)");
    }
    let u32_at = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().expect("4 bytes"));
    let u64_at = |at: usize| u64::from_le_bytes(bytes[at..at + 8].try_into().expect("8 bytes"));
    if u32_at(8) != CACHE_FORMAT || u32_at(12) != EXTRACTION_SCHEME {
        bail!("cache format or extraction scheme differs");
    }
    let index_len = usize::try_from(u64_at(16)).context("index length")?;
    let text_len = usize::try_from(u64_at(24)).context("text length")?;
    if index_len > MAX_INDEX_LEN {
        bail!("cache index longer than {MAX_INDEX_LEN} bytes");
    }
    let hash: [u8; 32] = bytes[32..64].try_into().expect("32 bytes");
    let text_start = HEADER_LEN
        .checked_add(index_len)
        .context("index length overflows")?;
    if text_start.checked_add(text_len) != Some(bytes.len()) {
        bail!("cache length is not header + index + text");
    }
    let mut hasher = blake3::Hasher::new();
    hasher.update(package_hash.as_bytes());
    hasher.update(&bytes[HEADER_LEN..]);
    if hasher.finalize().as_bytes() != &hash {
        bail!("cache hash differs (or the entry is another package's)");
    }
    let index: Vec<IndexEntry> =
        serde_json::from_slice(&bytes[HEADER_LEN..text_start]).context("cache index")?;
    let mut next = 0u64;
    let mut out = Vec::with_capacity(index.len());
    for e in index {
        if e.offset != next {
            bail!("cache index does not tile the text at {}", e.path);
        }
        // Bounded by the text before any usize sum, so none can overflow (sol review).
        next = match e.offset.checked_add(e.len) {
            Some(n) if n <= text_len as u64 => n,
            _ => bail!("cache text of {} past the end", e.path),
        };
        let (start, end) = (e.offset as usize, next as usize);
        out.push((e.path, text_start + start..text_start + end));
    }
    if next != text_len as u64 {
        bail!("cache index does not cover the text");
    }
    Ok(out)
}

/// Write package `hash`'s `files` to `out` as one whole cache entry (header, index, text) and
/// flush it.
fn write_entry(out: &mut impl Write, hash: &str, files: &[SourceFile]) -> Result<()> {
    let mut index = Vec::with_capacity(files.len());
    let mut text_len = 0u64;
    for f in files {
        let len = f.text.len() as u64;
        index.push(IndexEntry {
            path: f.virtual_path.clone(),
            offset: text_len,
            len,
        });
        text_len += len;
    }
    let index = serde_json::to_vec(&index).context("cache index")?;
    // Never publish an entry every read would refuse (sol review).
    if index.len() > MAX_INDEX_LEN {
        bail!("cache index longer than {MAX_INDEX_LEN} bytes");
    }
    let mut hasher = blake3::Hasher::new();
    hasher.update(hash.as_bytes());
    hasher.update(&index);
    for f in files {
        hasher.update(f.text.as_bytes());
    }
    out.write_all(MAGIC)?;
    out.write_all(&CACHE_FORMAT.to_le_bytes())?;
    out.write_all(&EXTRACTION_SCHEME.to_le_bytes())?;
    out.write_all(&(index.len() as u64).to_le_bytes())?;
    out.write_all(&text_len.to_le_bytes())?;
    out.write_all(hasher.finalize().as_bytes())?;
    out.write_all(&index)?;
    for f in files {
        out.write_all(f.text.as_bytes())?;
    }
    out.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::deps::app_package_zip::test_apps::build_app;

    const A: &str = "codeunit 50100 \"Ålborg\" { }\n";
    const B: &str = "table 50101 Bee { }\n";

    fn image(entries: &[(&str, &[u8])]) -> Arc<AppImage> {
        AppImage::from_bytes("pkg.app", build_app(entries))
    }

    fn two_files() -> Arc<AppImage> {
        image(&[("src/A.al", A.as_bytes()), ("src/B.al", B.as_bytes())])
    }

    fn entry(dir: &Path, img: &AppImage) -> PathBuf {
        dir.join(format!(
            "{}.s{EXTRACTION_SCHEME}.f{CACHE_FORMAT}.bin",
            img.content_hash()
        ))
    }

    fn texts(files: &[SourceFile]) -> Vec<(&str, &str)> {
        let mut v: Vec<_> = files
            .iter()
            .map(|f| (f.virtual_path.as_str(), &*f.text))
            .collect();
        v.sort_unstable();
        v
    }

    /// A miss extracts, writes one entry and hands out mapped texts read from it; a second
    /// call reads the entry (one map shared by every file), with the same texts.
    #[test]
    fn a_miss_writes_and_a_hit_maps() {
        let dir = tempfile::tempdir().unwrap();
        let img = two_files();
        let (cold, hash) = cached_source_in(dir.path(), &img).unwrap();
        assert_eq!(hash, img.content_hash());
        assert_eq!(texts(&cold), [("src/A.al", A), ("src/B.al", B)]);
        assert!(cold.iter().all(|f| f.text.is_mapped()), "a miss maps too");
        assert!(entry(dir.path(), &img).is_file());

        let (warm, _) = cached_source_in(dir.path(), &img).unwrap();
        assert_eq!(texts(&warm), texts(&cold));
        let map = warm[0].text.mapping().expect("mapped");
        assert!(
            warm.iter()
                .all(|f| f.text.mapping().is_some_and(|m| Arc::ptr_eq(m, map))),
            "one map per package"
        );
    }

    /// A valid entry is a hit: it is read, not extracted again (an entry whose text is not the
    /// package's is served as written).
    #[test]
    fn a_valid_entry_is_served_without_extracting() {
        let dir = tempfile::tempdir().unwrap();
        let img = two_files();
        let path = entry(dir.path(), &img);
        let text = "codeunit 1 Kept { }";
        let index = format!(r#"[{{"path":"src/K.al","offset":0,"len":{}}}]"#, text.len());
        write_raw(
            &path,
            img.content_hash(),
            index.as_bytes(),
            text.as_bytes(),
            None,
        );
        let (files, _) = cached_source_in(dir.path(), &img).unwrap();
        assert_eq!(texts(&files), [("src/K.al", text)]);
        assert!(files.iter().all(|f| f.text.is_mapped()));
    }

    /// The package hash `write_raw` entries are bound to in the hand-made tests.
    const PKG: &str = "pkg";

    /// Read entry `path` of private directory `dir` (missing reads as an error).
    fn read_cache(dir: &Path, path: &Path, pkg: &str) -> Result<Vec<SourceFile>> {
        let d = CacheDir::open(dir).expect("a private test dir");
        let name = path.file_name().unwrap().to_str().unwrap();
        d.read(name, pkg)?.context("no entry")
    }

    /// An entry of `index` and `text` for package `pkg` with a correct header (or the given
    /// hash).
    fn write_raw(path: &Path, pkg: &str, index: &[u8], text: &[u8], hash: Option<[u8; 32]>) {
        let mut h = blake3::Hasher::new();
        h.update(pkg.as_bytes());
        h.update(index);
        h.update(text);
        let mut out = MAGIC.to_vec();
        out.extend(CACHE_FORMAT.to_le_bytes());
        out.extend(EXTRACTION_SCHEME.to_le_bytes());
        out.extend((index.len() as u64).to_le_bytes());
        out.extend((text.len() as u64).to_le_bytes());
        out.extend(hash.unwrap_or(*h.finalize().as_bytes()));
        out.extend(index);
        out.extend(text);
        std::fs::write(path, out).unwrap();
    }

    /// Hostile or damaged entries are refused, never a panic: truncated (with the hash still
    /// matching what is left), a wrong magic, a damaged index that is still JSON (the hash
    /// covers it), another package's entry, an index that leaves a gap, overlaps, or does not
    /// cover the text, a range past the text, and an index over the cap.
    #[test]
    fn damaged_entries_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        let path = d.join("e.bin");
        let good = br#"[{"path":"a.al","offset":0,"len":3}]"#;
        write_raw(&path, PKG, good, b"abc", None);
        assert!(
            read_cache(d, &path, PKG).is_ok(),
            "precondition: the good entry reads"
        );

        let bytes = std::fs::read(&path).unwrap();
        std::fs::write(&path, &bytes[..bytes.len() - 1]).unwrap();
        assert!(read_cache(d, &path, PKG).is_err(), "truncated");
        // Truncated by one byte, with the header's text length and hash made to match what is
        // left: only the length check (the index's last entry past the text) can refuse it.
        write_raw(&path, PKG, good, b"ab", None);
        assert!(
            read_cache(d, &path, PKG).is_err(),
            "text shorter than the index"
        );
        std::fs::write(&path, &bytes[..HEADER_LEN - 1]).unwrap();
        assert!(read_cache(d, &path, PKG).is_err(), "shorter than a header");

        let mut magic = bytes.clone();
        magic[0] = b'X';
        std::fs::write(&path, &magic).unwrap();
        assert!(read_cache(d, &path, PKG).is_err(), "wrong magic");

        let mut index = bytes.clone();
        let at = HEADER_LEN + br#"[{"path":""#.len();
        assert_eq!(index[at], b'a', "precondition: the path's first byte");
        index[at] = b'z';
        std::fs::write(&path, &index).unwrap();
        assert!(
            read_cache(d, &path, PKG).is_err(),
            "a damaged index under the old hash"
        );

        write_raw(&path, PKG, good, b"abc", None);
        assert!(
            read_cache(d, &path, "other").is_err(),
            "a valid entry of another package"
        );

        for (index, why) in [
            (&br#"[]"#[..], "an empty index over text"),
            (br#"[{"path":"a.al","offset":0,"len":2}]"#, "text left over"),
            (
                br#"[{"path":"a.al","offset":1,"len":2}]"#,
                "a gap before the first entry",
            ),
            (
                br#"[{"path":"a.al","offset":0,"len":2},{"path":"b.al","offset":1,"len":2}]"#,
                "overlapping entries",
            ),
            (
                br#"[{"path":"a.al","offset":2,"len":3}]"#,
                "a range past the text",
            ),
            (
                br#"[{"path":"a.al","offset":18446744073709551615,"len":3}]"#,
                "an offset that overflows",
            ),
            // sol: tiles (offset 0), so only the bound before the usize sums refuses it (an
            // unbounded `text_start + len` overflows and panics in a debug build).
            (
                br#"[{"path":"a.al","offset":0,"len":18446744073709551615}]"#,
                "a length that overflows",
            ),
        ] {
            write_raw(&path, PKG, index, b"abc", None);
            assert!(read_cache(d, &path, PKG).is_err(), "{why}");
        }

        // A trailing byte past the header's text length, with the hash over all of it and an
        // index that tiles the stated length: only the exact-length check refuses it.
        write_raw(
            &path,
            PKG,
            br#"[{"path":"a.al","offset":0,"len":2}]"#,
            b"abc",
            None,
        );
        let mut trailing = std::fs::read(&path).unwrap();
        trailing[24..32].copy_from_slice(&2u64.to_le_bytes());
        std::fs::write(&path, &trailing).unwrap();
        let err = read_cache(d, &path, PKG).unwrap_err();
        assert!(format!("{err:#}").contains("cache length"), "{err:#}");

        // An index length over the cap is refused before the length or hash is looked at.
        let mut over = bytes.clone();
        over[16..24].copy_from_slice(&((MAX_INDEX_LEN as u64) + 1).to_le_bytes());
        std::fs::write(&path, &over).unwrap();
        let err = read_cache(d, &path, PKG).unwrap_err();
        assert!(format!("{err:#}").contains("index longer"), "{err:#}");
    }

    /// Anything but a regular file at the entry's path is refused, and the entry is then
    /// rewritten as a regular file: a symlink (to a valid entry: it is never followed) and,
    /// on Unix, a FIFO (which a plain open would block on).
    #[cfg(unix)]
    #[test]
    fn a_non_regular_entry_is_refused_and_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let img = two_files();
        let path = entry(dir.path(), &img);
        let elsewhere = dir.path().join("elsewhere.bin");
        write_raw(
            &elsewhere,
            img.content_hash(),
            br#"[{"path":"src/K.al","offset":0,"len":1}]"#,
            b"k",
            None,
        );
        std::os::unix::fs::symlink(&elsewhere, &path).unwrap();
        assert!(read_cache(dir.path(), &path, img.content_hash()).is_err());
        let (files, _) = cached_source_in(dir.path(), &img).unwrap();
        assert_eq!(texts(&files), [("src/A.al", A), ("src/B.al", B)]);
        assert!(std::fs::symlink_metadata(&path).unwrap().is_file());

        std::fs::remove_file(&path).unwrap();
        let c = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
        // SAFETY: a valid NUL-terminated path.
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
        let (files, _) = cached_source_in(dir.path(), &img).unwrap();
        assert_eq!(texts(&files), [("src/A.al", A), ("src/B.al", B)]);
        assert!(std::fs::symlink_metadata(&path).unwrap().is_file());
    }

    /// A cache directory other users can write is not used: its entry (even a valid one whose
    /// text differs) is neither read nor rewritten, and the extraction is owned text. An entry
    /// other users can write, in a private directory, is refused and replaced by a private one.
    #[cfg(unix)]
    #[test]
    fn an_untrusted_cache_is_not_used() {
        use std::os::unix::fs::PermissionsExt;
        let mode = |p: &Path, m: u32| {
            std::fs::set_permissions(p, std::fs::Permissions::from_mode(m)).unwrap();
        };
        let dir = tempfile::tempdir().unwrap();
        let img = two_files();
        let path = entry(dir.path(), &img);
        let theirs = br#"[{"path":"src/K.al","offset":0,"len":1}]"#;
        write_raw(&path, img.content_hash(), theirs, b"k", None);
        mode(&path, 0o600);
        let before = std::fs::read(&path).unwrap();
        mode(dir.path(), 0o777);
        let (files, _) = cached_source_in(dir.path(), &img).unwrap();
        assert_eq!(
            texts(&files),
            [("src/A.al", A), ("src/B.al", B)],
            "not read"
        );
        assert!(files.iter().all(|f| !f.text.is_mapped()), "owned");
        assert_eq!(std::fs::read(&path).unwrap(), before, "not rewritten");

        mode(dir.path(), 0o700);
        let (files, _) = cached_source_in(dir.path(), &img).unwrap();
        assert_eq!(
            texts(&files),
            [("src/K.al", "k")],
            "precondition: private again, the entry is read"
        );

        mode(&path, 0o666);
        let (files, _) = cached_source_in(dir.path(), &img).unwrap();
        assert_eq!(texts(&files), [("src/A.al", A), ("src/B.al", B)], "refused");
        assert!(files.iter().all(|f| f.text.is_mapped()), "the replacement");
        let m = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(m & 0o077, 0, "the replacement is owner-only: {m:o}");
    }

    /// A publish re-checks the entry under the lock: when another process has already published
    /// a valid entry (after this one was refused or missed), it is used, not replaced.
    #[test]
    fn a_publish_uses_an_entry_another_process_published() {
        let dir = tempfile::tempdir().unwrap();
        let img = two_files();
        let path = entry(dir.path(), &img);
        let hash = img.content_hash();
        let theirs = br#"[{"path":"src/K.al","offset":0,"len":1}]"#;
        write_raw(&path, hash, theirs, b"k", None);
        let before = std::fs::read(&path).unwrap();
        let d = CacheDir::open(dir.path()).unwrap();
        let name = path.file_name().unwrap().to_str().unwrap();
        let files = publish(&d, name, hash, extract_embedded_source(&img).unwrap());
        assert_eq!(texts(&files), [("src/K.al", "k")]);
        assert_eq!(std::fs::read(&path).unwrap(), before, "not replaced");
    }

    /// A publish waits for the lock: while another holder has it, the refused entry is not
    /// replaced (the worker reports it has started before the wait is timed, so a worker that
    /// was merely not scheduled yet does not pass for a blocked one); once released, the entry
    /// is replaced.
    #[cfg(unix)]
    #[test]
    fn a_publish_waits_for_the_lock() {
        let dir = tempfile::tempdir().unwrap();
        let img = two_files();
        let path = entry(dir.path(), &img);
        let hash = img.content_hash().to_string();
        std::fs::write(&path, b"refused").unwrap();
        let held = CacheDir::open(dir.path())
            .unwrap()
            .lock(&hash)
            .expect("lock");
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let (dir_path, name) = (
            dir.path().to_path_buf(),
            path.file_name().unwrap().to_str().unwrap().to_string(),
        );
        let files = extract_embedded_source(&img).unwrap();
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let repair = std::thread::spawn(move || {
            let d = CacheDir::open(&dir_path).unwrap();
            started_tx.send(()).unwrap();
            let out = texts(&publish(&d, &name, &hash, files))
                .into_iter()
                .map(|(p, t)| (p.to_string(), t.to_string()))
                .collect::<Vec<_>>();
            done_tx.send(()).unwrap();
            out
        });
        started_rx.recv().unwrap();
        assert!(
            done_rx
                .recv_timeout(std::time::Duration::from_millis(300))
                .is_err(),
            "the repair ran while the lock was held"
        );
        assert_eq!(
            std::fs::read(&path).unwrap(),
            b"refused",
            "not replaced yet"
        );
        drop(held);
        let out = repair.join().unwrap();
        assert_eq!(out.len(), 2);
        assert_ne!(
            std::fs::read(&path).unwrap(),
            b"refused",
            "replaced after release"
        );
    }

    /// A lock path that is not a regular private file (a FIFO here) is not used, never blocks,
    /// and nothing is published without it: a refused entry is left alone, and a missing one
    /// is not created (the source is owned text either way).
    #[cfg(unix)]
    #[test]
    fn a_bad_lock_replaces_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let img = two_files();
        let path = entry(dir.path(), &img);
        let hash = img.content_hash();
        std::fs::write(&path, b"refused").unwrap();
        let lock = dir.path().join(format!("{hash}.lock"));
        let c = std::ffi::CString::new(lock.as_os_str().as_encoded_bytes()).unwrap();
        // SAFETY: a valid NUL-terminated path.
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
        let (files, _) = cached_source_in(dir.path(), &img).unwrap();
        assert_eq!(texts(&files), [("src/A.al", A), ("src/B.al", B)]);
        assert!(files.iter().all(|f| !f.text.is_mapped()), "owned");
        assert_eq!(std::fs::read(&path).unwrap(), b"refused", "not replaced");

        std::fs::remove_file(&path).unwrap();
        let (files, _) = cached_source_in(dir.path(), &img).unwrap();
        assert!(files.iter().all(|f| !f.text.is_mapped()), "owned on a miss");
        assert!(
            std::fs::symlink_metadata(&path).is_err(),
            "nothing published without the lock"
        );
    }

    /// Windows: while a map of an entry lives, the entry cannot be opened for writing (the
    /// map's duplicate of the deny-write handle), though the handle `read` opened is closed.
    #[cfg(windows)]
    #[test]
    fn a_mapped_entry_cannot_be_opened_for_writing() {
        let dir = tempfile::tempdir().unwrap();
        let img = two_files();
        let (files, _) = cached_source_in(dir.path(), &img).unwrap();
        assert!(files.iter().all(|f| f.text.is_mapped()));
        let path = entry(dir.path(), &img);
        assert!(
            std::fs::OpenOptions::new().write(true).open(&path).is_err(),
            "a write open while mapped"
        );
        drop(files);
        assert!(
            std::fs::OpenOptions::new().write(true).open(&path).is_ok(),
            "precondition: a write open once the map is gone"
        );
    }

    /// A writer that finds a valid entry already published keeps it (no-clobber), so every
    /// engine maps the one file; a refused entry is replaced.
    #[test]
    fn a_published_entry_is_kept_and_a_refused_one_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let img = two_files();
        let path = entry(dir.path(), &img);
        let hash = img.content_hash();
        let files = extract_embedded_source(&img).unwrap();
        let theirs = br#"[{"path":"src/K.al","offset":0,"len":1}]"#;
        write_raw(&path, hash, theirs, b"k", None);
        let before = std::fs::read(&path).unwrap();
        let d = CacheDir::open(dir.path()).unwrap();
        let name = path.file_name().unwrap().to_str().unwrap();
        d.write(name, hash, &files, false).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), before, "kept");
        d.write(name, hash, &files, true).unwrap();
        assert_ne!(std::fs::read(&path).unwrap(), before, "replaced");
        assert_eq!(
            texts(&read_cache(dir.path(), &path, hash).unwrap()),
            [("src/A.al", A), ("src/B.al", B)]
        );
        let stray: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .filter(|n| n.to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(stray.is_empty(), "no temp file left: {stray:?}");
    }

    /// An entry whose text changed in place with its length and UTF-8 kept (one letter
    /// flipped) is refused by the hash alone, re-extracted and rewritten.
    #[test]
    fn a_broken_entry_is_refused_and_rewritten() {
        let dir = tempfile::tempdir().unwrap();
        let img = two_files();
        cached_source_in(dir.path(), &img).unwrap();
        let path = entry(dir.path(), &img);
        let mut bytes = std::fs::read(&path).unwrap();
        let at = bytes.len() - B.len() + "table 50101 ".len();
        assert_eq!(
            bytes[at], b'B',
            "precondition: the flipped byte is B's name"
        );
        bytes[at] = b'X';
        std::fs::write(&path, &bytes).unwrap();

        let (files, _) = cached_source_in(dir.path(), &img).unwrap();
        assert_eq!(texts(&files), [("src/A.al", A), ("src/B.al", B)]);
        assert!(files.iter().all(|f| f.text.is_mapped()));
        assert_ne!(
            std::fs::read(&path).unwrap(),
            bytes,
            "the entry was rewritten"
        );
    }

    /// An entry whose index changed in place and is still JSON (one path letter flipped) is
    /// refused by the hash, which covers the index, and rewritten.
    #[test]
    fn a_damaged_index_is_refused_and_rewritten() {
        let dir = tempfile::tempdir().unwrap();
        let img = two_files();
        cached_source_in(dir.path(), &img).unwrap();
        let path = entry(dir.path(), &img);
        let mut bytes = std::fs::read(&path).unwrap();
        let at = HEADER_LEN + br#"[{"path":"src/"#.len();
        assert_eq!(
            bytes[at], b'A',
            "precondition: the flipped byte is A's path"
        );
        bytes[at] = b'Z';
        std::fs::write(&path, &bytes).unwrap();

        let (files, _) = cached_source_in(dir.path(), &img).unwrap();
        assert_eq!(texts(&files), [("src/A.al", A), ("src/B.al", B)]);
        assert!(files.iter().all(|f| f.text.is_mapped()), "served mapped");
        assert_ne!(
            std::fs::read(&path).unwrap(),
            bytes,
            "the entry was rewritten"
        );
    }

    /// The pre-ENG-113 JSON entry of the same package is never read (its name differs).
    #[test]
    fn an_old_json_entry_is_not_read() {
        let dir = tempfile::tempdir().unwrap();
        let img = two_files();
        let old = dir
            .path()
            .join(format!("{}.s{EXTRACTION_SCHEME}.json", img.content_hash()));
        std::fs::write(&old, r#"[{"virtual_path":"src/Old.al","text":"old"}]"#).unwrap();
        let (files, _) = cached_source_in(dir.path(), &img).unwrap();
        assert_eq!(texts(&files), [("src/A.al", A), ("src/B.al", B)]);
    }

    /// A package with no source is a valid entry: written once, then a hit with no files that
    /// leaves the entry alone (not re-extracted and rewritten, which would give it a new
    /// inode).
    #[cfg(unix)]
    #[test]
    fn a_package_with_no_source_is_a_hit() {
        use std::os::unix::fs::MetadataExt;
        let dir = tempfile::tempdir().unwrap();
        let img = image(&[("SymbolReference.json", b"{}")]);
        let (files, _) = cached_source_in(dir.path(), &img).unwrap();
        assert!(files.is_empty());
        let path = entry(dir.path(), &img);
        assert!(
            read_cache(dir.path(), &path, img.content_hash()).is_ok_and(|f| f.is_empty()),
            "a valid empty entry"
        );
        let ino = std::fs::metadata(&path).unwrap().ino();
        let (files, _) = cached_source_in(dir.path(), &img).unwrap();
        assert!(files.is_empty());
        assert_eq!(
            std::fs::metadata(&path).unwrap().ino(),
            ino,
            "not rewritten"
        );
    }

    /// A cache that cannot be written (the directory is a file) keeps the extraction as
    /// owned text.
    #[test]
    fn an_unwritable_cache_keeps_owned_text() {
        let dir = tempfile::tempdir().unwrap();
        let not_a_dir = dir.path().join("file");
        std::fs::write(&not_a_dir, "").unwrap();
        let (files, _) = cached_source_in(&not_a_dir, &two_files()).unwrap();
        assert_eq!(texts(&files), [("src/A.al", A), ("src/B.al", B)]);
        assert!(files.iter().all(|f| !f.text.is_mapped()));
    }
}

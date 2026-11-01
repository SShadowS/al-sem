//! Owned-buffer package acquisition (context-server design r5 §5.2; review r4
//! #9): a dependency `.app` is read ONCE into an owned buffer, and every
//! product of that package (its manifest, its symbols, its extracted source,
//! the ABI the graph ingests, its fingerprint row) is built from that one
//! buffer. No loader takes a package path.
//!
//! Before, the loader opened a package once per product (the manifest probe,
//! the symbol read, the source-cache hash, the extraction, the ABI read and
//! the fingerprint hash each opened the path again), so a file rewritten in
//! place between two of those opens gave products of two different byte
//! images under one identity. A read-only mapping of the user's file is not an
//! immutable image either (the file can change under the mapping), so none is
//! made: the only mapping allowed is of a sealed private copy (the source
//! cache's own entries, `snapshot::cache`).
//!
//! The image's identity is its blake3 (computed from the buffer as it is
//! acquired) and the size and modification time taken from the same open
//! handle before the read ([`AppFileStamp`]); the stamp's `content` is that
//! blake3, so a cache keyed by the stamp (`program::dep_cache`) is a hit only
//! for the same bytes. [`PackageManifest`] is the set of identities a run
//! consumed (`M_consumed`), compared with stat-and-hash passes taken before
//! the acquisition and after the build (`M_before`, `M_after`, design r5
//! §5.1) by [`check_consistent`].
//!
//! **Where the bytes live.** The graph build and the cross-app model read a
//! package's symbols again long after the loader (`program::abi_ingest`), so
//! an image lives as long as the snapshot. Held in memory, the closure's
//! images (73 MB on CDO) sat on top of the build's peak (+48 MB measured).
//! So an image above [`PRIVATE_COPY_MIN_BYTES`] is moved, once hashed, out
//! of the heap into a **sealed private copy**: an anonymous temp file (no
//! name any other process can open; on Unix unlinked at creation), written
//! once from the buffer and never modified, read back by positional reads
//! (design r5 §5.2 allows exactly this copy). If temp storage fails, the
//! buffer stays in memory: the result never depends on the temp disk.

use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use crate::dependencies::AppFileStamp;

/// The largest package file read (a hostile or corrupt file never makes an
/// unbounded buffer). Above every real package: BaseApp is ~45 MB; a runtime
/// package's stream is itself capped at 512 MiB.
pub const APP_FILE_CAP: u64 = 1024 * 1024 * 1024;

/// An image at least this large moves to a sealed private copy; a smaller
/// one stays in memory (no temp file per small package or test fixture).
pub const PRIVATE_COPY_MIN_BYTES: u64 = 1024 * 1024;

/// The first bytes of every image, kept in memory (a header test reads 48).
const HEAD_BYTES: usize = 64;

/// Where an image's bytes are held.
enum Storage {
    Memory(Vec<u8>),
    /// The sealed private copy (see the module doc).
    Private(std::fs::File),
}

/// One `.app` file read into an owned buffer, with its identity.
pub struct AppImage {
    path: PathBuf,
    len: u64,
    head: Vec<u8>,
    storage: Storage,
    stamp: AppFileStamp,
    blake3: String,
    sha256: OnceLock<String>,
}

impl std::fmt::Debug for AppImage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppImage")
            .field("path", &self.path)
            .field("len", &self.len)
            .field("private", &self.is_private())
            .field("blake3", &self.blake3)
            .finish()
    }
}

impl AppImage {
    /// Read the package at `path`: one open, its size and modification time
    /// from that handle, then the whole file into a buffer (at most
    /// [`APP_FILE_CAP`] bytes), hashed; a large one then moves to its sealed
    /// private copy. The error keeps the OS error's kind.
    pub fn acquire(path: &Path) -> std::io::Result<Arc<AppImage>> {
        let file = std::fs::File::open(path)?;
        let meta = file.metadata()?;
        if !meta.is_file() {
            return Err(std::io::Error::other("not a file"));
        }
        if meta.len() > APP_FILE_CAP {
            return Err(std::io::Error::other(format!(
                "package file of {} bytes exceeds the {APP_FILE_CAP}-byte cap",
                meta.len()
            )));
        }
        let mut bytes = Vec::with_capacity(usize::try_from(meta.len()).unwrap_or(0));
        file.take(APP_FILE_CAP + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > APP_FILE_CAP {
            return Err(std::io::Error::other("package file exceeds the cap"));
        }
        Ok(AppImage::of(
            path.to_path_buf(),
            bytes,
            meta.modified().ok(),
        ))
    }

    /// An image of `bytes` as if read from `path` (a test's, or bytes a
    /// caller already holds), with no modification time.
    pub fn from_bytes(path: impl Into<PathBuf>, bytes: Vec<u8>) -> Arc<AppImage> {
        AppImage::of(path.into(), bytes, None)
    }

    fn of(path: PathBuf, bytes: Vec<u8>, modified: Option<std::time::SystemTime>) -> Arc<AppImage> {
        let hash = blake3::hash(&bytes);
        let len = bytes.len() as u64;
        let head = bytes[..bytes.len().min(HEAD_BYTES)].to_vec();
        let storage = if len >= PRIVATE_COPY_MIN_BYTES {
            match private_copy(&bytes) {
                Ok(file) => Storage::Private(file),
                Err(e) => {
                    log::warn!(
                        "{}: temp storage failed ({e}); keeping the package in memory",
                        path.display()
                    );
                    Storage::Memory(bytes)
                }
            }
        } else {
            Storage::Memory(bytes)
        };
        Arc::new(AppImage {
            stamp: AppFileStamp {
                len,
                modified,
                content: *hash.as_bytes(),
            },
            blake3: hash.to_hex().to_string(),
            sha256: OnceLock::new(),
            path,
            len,
            head,
            storage,
        })
    }

    /// The path it was read from (names it in messages; never reopened).
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The image's length in bytes.
    pub fn len(&self) -> u64 {
        self.len
    }

    /// Whether the image is empty.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// The first bytes (at most 64): enough for any header test.
    pub fn head(&self) -> &[u8] {
        &self.head
    }

    /// Whether the bytes live in the sealed private copy, not the heap.
    pub fn is_private(&self) -> bool {
        matches!(self.storage, Storage::Private(_))
    }

    /// A reader over the whole image, owning a share of it: what a zip
    /// archive of the package reads from. Each reader has its own position.
    pub fn reader(self: &Arc<Self>) -> ImageReader {
        ImageReader {
            image: Arc::clone(self),
            pos: 0,
        }
    }

    /// `len` bytes from `start` (a runtime package's stream); `None` when the
    /// image is shorter.
    pub fn read_range(&self, start: u64, len: u64) -> std::io::Result<Option<Vec<u8>>> {
        let Some(end) = start.checked_add(len).filter(|&e| e <= self.len) else {
            return Ok(None);
        };
        let mut out = vec![0u8; usize::try_from(end - start).map_err(std::io::Error::other)?];
        let mut done = 0;
        while done < out.len() {
            let n = self.read_at(&mut out[done..], start + done as u64)?;
            if n == 0 {
                return Err(std::io::ErrorKind::UnexpectedEof.into());
            }
            done += n;
        }
        Ok(Some(out))
    }

    /// Bytes at `offset` into `buf`; 0 at the end.
    fn read_at(&self, buf: &mut [u8], offset: u64) -> std::io::Result<usize> {
        if offset >= self.len {
            return Ok(0);
        }
        match &self.storage {
            Storage::Memory(bytes) => {
                let start = offset as usize;
                let n = buf.len().min(bytes.len() - start);
                buf[..n].copy_from_slice(&bytes[start..start + n]);
                Ok(n)
            }
            Storage::Private(file) => {
                let want = buf.len().min((self.len - offset) as usize);
                read_file_at(file, &mut buf[..want], offset)
            }
        }
    }

    /// Size, modification time and content hash: the cache key of every
    /// product built from this image.
    pub fn stamp(&self) -> AppFileStamp {
        self.stamp
    }

    /// blake3 hex of the whole file: the package's content identity (the
    /// source cache's key).
    pub fn content_hash(&self) -> &str {
        &self.blake3
    }

    /// SHA-256 hex of the whole file (a snapshot's `dep-package` row),
    /// computed once, on first ask.
    pub fn sha256(&self) -> &str {
        self.sha256.get_or_init(|| {
            use sha2::Digest;
            let mut hasher = sha2::Sha256::new();
            let mut buf = vec![0u8; 1 << 16];
            let mut offset = 0;
            loop {
                // A private copy that cannot be read back is a broken temp
                // disk under a running process: nothing sensible remains.
                let n = self
                    .read_at(&mut buf, offset)
                    .expect("reading the package's sealed private copy");
                if n == 0 {
                    break;
                }
                hasher.update(&buf[..n]);
                offset += n as u64;
            }
            crate::engine::ids::hex_lower(&hasher.finalize())
        })
    }
}

/// `bytes` written once into an anonymous temp file, rewound: the sealed
/// private copy.
fn private_copy(bytes: &[u8]) -> std::io::Result<std::fs::File> {
    let mut file = tempfile::tempfile()?;
    file.write_all(bytes)?;
    file.flush()?;
    file.seek(SeekFrom::Start(0))?;
    Ok(file)
}

#[cfg(unix)]
fn read_file_at(file: &std::fs::File, buf: &mut [u8], offset: u64) -> std::io::Result<usize> {
    std::os::unix::fs::FileExt::read_at(file, buf, offset)
}

#[cfg(windows)]
fn read_file_at(file: &std::fs::File, buf: &mut [u8], offset: u64) -> std::io::Result<usize> {
    std::os::windows::fs::FileExt::seek_read(file, buf, offset)
}

/// A reader over an image, owning a share of it and its own position:
/// several archives of one package read at once without sharing an offset.
pub struct ImageReader {
    image: Arc<AppImage>,
    pos: u64,
}

impl Read for ImageReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.image.read_at(buf, self.pos)?;
        self.pos += n as u64;
        Ok(n)
    }
}

impl Seek for ImageReader {
    fn seek(&mut self, to: SeekFrom) -> std::io::Result<u64> {
        let (base, delta) = match to {
            SeekFrom::Start(n) => {
                self.pos = n;
                return Ok(n);
            }
            SeekFrom::End(d) => (self.image.len, d),
            SeekFrom::Current(d) => (self.pos, d),
        };
        match base.checked_add_signed(delta) {
            Some(n) => {
                self.pos = n;
                Ok(n)
            }
            None => Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "seek before the start of the package image",
            )),
        }
    }
}

/// One package's identity as a manifest records it: absent (the file could
/// not be read, with why), or its length and blake3.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PackageState {
    Unreadable(String),
    Present { len: u64, blake3: String },
}

/// The identity of every package of a closure, by path, in closure order: an
/// `M_before`/`M_after` observation of the disk, or `M_consumed` (the images
/// a run used).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PackageManifest(pub Vec<(PathBuf, PackageState)>);

impl PackageManifest {
    /// Each path's bytes on disk now, streamed through blake3 (no buffer).
    pub fn observe(paths: &[PathBuf]) -> PackageManifest {
        PackageManifest(
            paths
                .iter()
                .map(|p| {
                    let state = std::fs::File::open(p)
                        .and_then(|f| {
                            let mut h = blake3::Hasher::new();
                            let len = std::io::copy(&mut &f, &mut h)?;
                            Ok(PackageState::Present {
                                len,
                                blake3: h.finalize().to_hex().to_string(),
                            })
                        })
                        .unwrap_or_else(|e| PackageState::Unreadable(e.kind().to_string()));
                    (p.clone(), state)
                })
                .collect(),
        )
    }

    /// The paths, in order.
    pub fn paths(&self) -> Vec<PathBuf> {
        self.0.iter().map(|(p, _)| p.clone()).collect()
    }
}

/// Why a run's consumed packages are not the ones on disk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InputsUnstable {
    /// The first package whose observations differ.
    pub path: PathBuf,
    pub before: Option<PackageState>,
    pub consumed: Option<PackageState>,
    pub after: Option<PackageState>,
}

impl std::fmt::Display for InputsUnstable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let show = |s: &Option<PackageState>| match s {
            None => "not listed".to_string(),
            Some(PackageState::Unreadable(why)) => format!("unreadable ({why})"),
            Some(PackageState::Present { len, blake3 }) => {
                format!("{len} bytes, blake3 {}", &blake3[..16.min(blake3.len())])
            }
        };
        write!(
            f,
            "package {} changed during the build: before {}, consumed {}, after {}",
            self.path.display(),
            show(&self.before),
            show(&self.consumed),
            show(&self.after)
        )
    }
}

/// `M_before == M_consumed == M_after` (design r5 §5.1): the run's products
/// were built from exactly the bytes on disk before and after it. A package
/// read as B while the disk held A before and after (an in-place overwrite
/// and restore during consumption) fails here, though `before == after`.
pub fn check_consistent(
    before: &PackageManifest,
    consumed: &PackageManifest,
    after: &PackageManifest,
) -> Result<(), InputsUnstable> {
    if before == consumed && consumed == after {
        return Ok(());
    }
    let state =
        |m: &PackageManifest, p: &Path| m.0.iter().find(|(q, _)| q == p).map(|(_, s)| s.clone());
    let mut paths: Vec<&PathBuf> = before.0.iter().map(|(p, _)| p).collect();
    paths.extend(consumed.0.iter().map(|(p, _)| p));
    paths.extend(after.0.iter().map(|(p, _)| p));
    let path = paths
        .into_iter()
        .find(|p| {
            let b = state(before, p);
            b != state(consumed, p) || b != state(after, p)
        })
        .cloned()
        // Same states, different order: name the first entry.
        .unwrap_or_else(|| before.0.first().map(|(p, _)| p.clone()).unwrap_or_default());
    Err(InputsUnstable {
        before: state(before, &path),
        consumed: state(consumed, &path),
        after: state(after, &path),
        path,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every byte of the image, through a reader.
    fn all(image: &Arc<AppImage>) -> Vec<u8> {
        let mut out = Vec::new();
        image.reader().read_to_end(&mut out).unwrap();
        out
    }

    /// The image is the bytes as read, hashed from the buffer; its stamp's
    /// content is that hash, so two images of different bytes never share a
    /// stamp, even at one size and modification time.
    #[test]
    fn an_image_is_its_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.app");
        std::fs::write(&p, b"AAAA").unwrap();
        let a = AppImage::acquire(&p).unwrap();
        assert!(!a.is_private(), "a small image stays in memory");
        assert_eq!(all(&a), b"AAAA");
        assert_eq!(a.content_hash(), blake3::hash(b"AAAA").to_hex().as_str());
        assert_eq!(a.sha256(), crate::engine::ids::sha256_bytes_hex(b"AAAA"));
        let b = AppImage::from_bytes(&p, b"BBBB".to_vec());
        let (mut sa, sb) = (a.stamp(), b.stamp());
        sa.modified = None;
        assert_eq!((sa.len, sb.len), (4, 4));
        assert_ne!(sa, sb, "same length, different bytes: different stamps");
        // Acquired, then the file is gone: the image still holds the bytes.
        std::fs::remove_file(&p).unwrap();
        assert_eq!(all(&a), b"AAAA");
        assert!(AppImage::acquire(&p).is_err());
        assert!(
            AppImage::acquire(dir.path()).is_err(),
            "a folder is no package"
        );
    }

    /// A large image moves to its sealed private copy: off the heap, read
    /// back exactly (whole, by range, and by two readers at once, each with
    /// its own position), hashed as the buffer was, and untouched by an
    /// in-place rewrite of the user's file.
    #[test]
    fn a_large_image_reads_back_from_its_private_copy() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("big.app");
        let bytes: Vec<u8> = (0..(PRIVATE_COPY_MIN_BYTES as usize + 4099))
            .map(|i| (i % 251) as u8)
            .collect();
        std::fs::write(&p, &bytes).unwrap();
        let image = AppImage::acquire(&p).unwrap();
        assert!(image.is_private(), "{image:?}");
        assert_eq!(image.len(), bytes.len() as u64);
        assert_eq!(image.head(), &bytes[..HEAD_BYTES]);
        std::fs::write(&p, vec![7u8; bytes.len()]).unwrap(); // in place
        assert_eq!(all(&image), bytes);
        assert_eq!(image.content_hash(), blake3::hash(&bytes).to_hex().as_str());
        assert_eq!(image.sha256(), crate::engine::ids::sha256_bytes_hex(&bytes));
        assert_eq!(
            image.read_range(1000, 50).unwrap().unwrap(),
            &bytes[1000..1050]
        );
        assert_eq!(image.read_range(image.len() - 1, 2).unwrap(), None);
        let (mut r1, mut r2) = (image.reader(), image.reader());
        r1.seek(SeekFrom::Start(10)).unwrap();
        r2.seek(SeekFrom::End(-3)).unwrap();
        let (mut a, mut b) = ([0u8; 4], [0u8; 3]);
        r1.read_exact(&mut a).unwrap();
        r2.read_exact(&mut b).unwrap();
        assert_eq!(a, bytes[10..14]);
        assert_eq!(b, bytes[bytes.len() - 3..]);
        assert!(r1.seek(SeekFrom::Current(-100)).is_err());
    }

    fn present(bytes: &[u8]) -> PackageState {
        PackageState::Present {
            len: bytes.len() as u64,
            blake3: blake3::hash(bytes).to_hex().to_string(),
        }
    }

    /// The three-way rule: equal observations pass; a run that consumed B
    /// while the disk held A before and after is refused, naming the path.
    #[test]
    fn consumed_must_equal_before_and_after() {
        let m = |s: PackageState| PackageManifest(vec![(PathBuf::from("x.app"), s)]);
        let (a, b) = (m(present(b"A")), m(present(b"B")));
        assert_eq!(check_consistent(&a, &a, &a), Ok(()));
        let err = check_consistent(&a, &b, &a).unwrap_err();
        assert_eq!(err.path, PathBuf::from("x.app"));
        assert_eq!(err.consumed, Some(present(b"B")));
        assert_eq!(err.before, err.after);
        assert!(
            check_consistent(&a, &a, &b).is_err(),
            "changed after the build"
        );
        assert!(
            check_consistent(&b, &a, &a).is_err(),
            "changed before the read"
        );
    }
}

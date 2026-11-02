//! The server-owned output folder of `alsem mcp` (context-server design r7
//! §10): every file the server writes for a client (a spilled stdout, a
//! digest, an `out` file) goes into ONE private instance folder,
//! `<output root>/<instance id>/`, and nowhere else.
//!
//! - The instance folder is created by the server (mode `0700`) and refused
//!   if it already exists; it is opened once and every later operation is
//!   relative to that handle (`openat` with `O_NOFOLLOW`, `mkdirat`,
//!   `renameat`), so a symlink planted in the folder is never followed.
//! - A client's `out` is a path relative to the instance folder, or an
//!   absolute path inside it; anything else (outside it, `..`, a root other
//!   than the folder) is refused (`refusedPath`).
//! - Every entry the server creates is recorded with its identity (device,
//!   inode). An existing entry is overwritten only when it is a recorded
//!   file whose identity still matches, through a temporary leaf and
//!   `renameat` under the sink's lock; any other existing entry is refused.
//! - At most [`QUOTA_BYTES`] are written per instance.
//! - At start, instance folders older than [`MAX_AGE`] under the root are
//!   removed (only names of the instance-id shape).
//!
//! Threat model (§10): another process of the same user can still mutate the
//! folder; the sink defends against its own races and against other users.
//!
//! Off Unix there is no handle-relative creation, so the sink fails closed:
//! it writes nothing, and a caller keeps answers inline.

use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

/// The most bytes one instance folder receives.
pub const QUOTA_BYTES: u64 = 1024 * 1024 * 1024;

/// Instance folders older than this are removed at server start.
pub const MAX_AGE: Duration = Duration::from_secs(24 * 3600);

/// Why the sink did not write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SinkError {
    /// The path is outside the instance folder, or names an entry the
    /// server did not create (`refusedPath`).
    Refused(String),
    /// The write itself failed.
    Io(String),
}

impl std::fmt::Display for SinkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SinkError::Refused(m) | SinkError::Io(m) => f.write_str(m),
        }
    }
}

/// A fresh 128-bit instance id (hex): the pid, the time and a counter,
/// hashed.
#[must_use]
pub fn new_instance_id() -> String {
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let n = N.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let seed = format!("alsem-mcp:{}:{now}:{n}", std::process::id());
    crate::engine::ids::sha256_bytes_hex(seed.as_bytes())[..32].to_string()
}

fn is_instance_name(name: &str) -> bool {
    name.len() == 32 && name.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Remove instance folders older than [`MAX_AGE`] under `root`.
fn remove_stale(root: &Path) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for e in entries.flatten() {
        let name = e.file_name();
        let Some(name) = name.to_str() else { continue };
        if !is_instance_name(name) {
            continue;
        }
        let Ok(meta) = std::fs::symlink_metadata(e.path()) else {
            continue;
        };
        let old = meta
            .modified()
            .ok()
            .and_then(|m| m.elapsed().ok())
            .is_some_and(|age| age > MAX_AGE);
        if meta.is_dir() && old {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
}

/// The relative components of `out` below the instance folder, or why not.
fn relative_components(instance: &Path, out: &str) -> Result<Vec<String>, SinkError> {
    let refused = |why: &str| SinkError::Refused(format!("out '{out}': {why}"));
    let p = Path::new(out);
    let rel: PathBuf = if p.is_absolute() {
        p.strip_prefix(instance)
            .map_err(|_| {
                refused(&format!(
                    "outside the server's output folder '{}'",
                    instance.display()
                ))
            })?
            .to_path_buf()
    } else {
        p.to_path_buf()
    };
    let mut parts = Vec::new();
    for c in rel.components() {
        match c {
            Component::Normal(s) => {
                parts.push(s.to_str().ok_or_else(|| refused("not UTF-8"))?.to_string())
            }
            Component::CurDir => {}
            Component::ParentDir => return Err(refused("'..' is not allowed")),
            Component::RootDir | Component::Prefix(_) => {
                return Err(refused("not relative to the output folder"));
            }
        }
    }
    if parts.is_empty() {
        return Err(refused("names no file"));
    }
    Ok(parts)
}

/// The server's output folder.
pub struct OutputSink {
    root: PathBuf,
    instance: PathBuf,
    inner: Mutex<imp::Inner>,
    /// Write nothing, as off Unix: the child's `no-output-dir` test hook
    /// (`ALSEM_ENGINE_TEST_HOOK`, debug and `engine-test-hooks` builds),
    /// which lets the Windows path run on any platform. Always false in a
    /// shipped build.
    inline_only: bool,
}

/// Why nothing is written off Unix (design r7 §10, Windows fail-closed).
fn inline_only_refusal(parts: &[String]) -> SinkError {
    SinkError::Refused(format!(
        "out '{}': this platform has no handle-relative file creation; outputs are inline only",
        parts.join("/")
    ))
}

/// Whether the `no-output-dir` test hook is set.
fn forced_inline_only() -> bool {
    #[cfg(any(feature = "engine-test-hooks", debug_assertions))]
    {
        matches!(
            crate::engine::host::test_hooks::from_env(),
            Some(crate::engine::host::test_hooks::Hook::NoOutputDir)
        )
    }
    #[cfg(not(any(feature = "engine-test-hooks", debug_assertions)))]
    {
        false
    }
}

impl OutputSink {
    /// Create the instance folder `<root>/<instance_id>/` (and `root` if
    /// needed), refusing an existing one; then remove stale siblings.
    pub fn create(root: &Path, instance_id: &str) -> Result<OutputSink, String> {
        std::fs::create_dir_all(root)
            .map_err(|e| format!("output root '{}': {e}", root.display()))?;
        let root = std::fs::canonicalize(root)
            .map_err(|e| format!("output root '{}': {e}", root.display()))?;
        remove_stale(&root);
        let instance = root.join(instance_id);
        let inner = imp::Inner::create(&root, instance_id)
            .map_err(|e| format!("output folder '{}': {e}", instance.display()))?;
        Ok(OutputSink {
            root,
            instance,
            inner: Mutex::new(inner),
            inline_only: forced_inline_only(),
        })
    }

    /// The output root.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The instance folder every output goes into.
    #[must_use]
    pub fn instance(&self) -> &Path {
        &self.instance
    }

    /// Whether the sink can write at all (Unix).
    #[must_use]
    pub fn writable(&self) -> bool {
        imp::WRITABLE && !self.inline_only
    }

    /// Bytes written so far.
    #[must_use]
    pub fn written(&self) -> u64 {
        self.inner.lock().expect("sink").written
    }

    /// Write `bytes` to the client's `out` path (relative to the instance
    /// folder, or absolute inside it). Returns the absolute path.
    pub fn write_out(&self, out: &str, bytes: &[u8]) -> Result<PathBuf, SinkError> {
        let parts = relative_components(&self.instance, out)?;
        if self.inline_only {
            return Err(inline_only_refusal(&parts));
        }
        self.inner
            .lock()
            .expect("sink")
            .write(&parts, bytes, true)?;
        Ok(parts.iter().fold(self.instance.clone(), |p, c| p.join(c)))
    }

    /// Write `bytes` to a new server-named file `<n>-<stem>.<ext>` in the
    /// instance folder. Returns the absolute path.
    pub fn write_new(&self, stem: &str, ext: &str, bytes: &[u8]) -> Result<PathBuf, SinkError> {
        let mut inner = self.inner.lock().expect("sink");
        inner.serial += 1;
        let name = format!("{:04}-{stem}.{ext}", inner.serial);
        if self.inline_only {
            return Err(inline_only_refusal(std::slice::from_ref(&name)));
        }
        inner.write(std::slice::from_ref(&name), bytes, false)?;
        Ok(self.instance.join(name))
    }
}

#[cfg(unix)]
mod imp {
    use super::{QUOTA_BYTES, SinkError};
    use std::collections::HashMap;
    use std::ffi::CString;
    use std::io::Write;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
    use std::path::Path;

    pub const WRITABLE: bool = true;

    /// The instance folder's handle and what the sink created in it.
    pub struct Inner {
        dir: OwnedFd,
        /// Relative path (joined with `/`) → (device, inode) of every entry
        /// the sink created.
        recorded: HashMap<String, (u64, u64)>,
        pub written: u64,
        pub serial: u64,
        temp: u64,
    }

    fn cstr(s: &str) -> std::io::Result<CString> {
        CString::new(s).map_err(|_| std::io::Error::other("a NUL byte in a path"))
    }

    fn check(rc: libc::c_int) -> std::io::Result<libc::c_int> {
        if rc < 0 {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(rc)
        }
    }

    fn open_dir_at(parent: RawFd, name: &str) -> std::io::Result<OwnedFd> {
        let c = cstr(name)?;
        // SAFETY: a plain openat on a descriptor this process owns; the
        // returned fd is owned by the `OwnedFd` alone.
        let fd = check(unsafe {
            libc::openat(
                parent,
                c.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        })?;
        // SAFETY: `fd` is a fresh descriptor nobody else owns.
        Ok(unsafe { OwnedFd::from_raw_fd(fd) })
    }

    fn mkdir_at(parent: RawFd, name: &str) -> std::io::Result<()> {
        let c = cstr(name)?;
        // SAFETY: a plain mkdirat on a descriptor this process owns.
        check(unsafe { libc::mkdirat(parent, c.as_ptr(), 0o700) }).map(|_| ())
    }

    fn create_at(parent: RawFd, name: &str) -> std::io::Result<std::fs::File> {
        let c = cstr(name)?;
        // SAFETY: as `open_dir_at`; `O_EXCL | O_NOFOLLOW` never opens an
        // existing entry or follows a link.
        let fd = check(unsafe {
            libc::openat(
                parent,
                c.as_ptr(),
                libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                0o600 as libc::c_uint,
            )
        })?;
        // SAFETY: `fd` is a fresh descriptor nobody else owns.
        Ok(unsafe { std::fs::File::from_raw_fd(fd) })
    }

    /// (device, inode, is a regular file) of `name` in `parent`, not
    /// following a link.
    fn stat_at(parent: RawFd, name: &str) -> std::io::Result<(u64, u64, bool)> {
        let c = cstr(name)?;
        // SAFETY: `st` is written by fstatat before it is read.
        let mut st: libc::stat = unsafe { std::mem::zeroed() };
        check(unsafe { libc::fstatat(parent, c.as_ptr(), &mut st, libc::AT_SYMLINK_NOFOLLOW) })?;
        #[allow(clippy::unnecessary_cast)]
        Ok((
            st.st_dev as u64,
            st.st_ino as u64,
            (st.st_mode & libc::S_IFMT) == libc::S_IFREG,
        ))
    }

    fn rename_at(parent: RawFd, from: &str, to: &str) -> std::io::Result<()> {
        let (f, t) = (cstr(from)?, cstr(to)?);
        // SAFETY: a plain renameat within one directory this process owns.
        check(unsafe { libc::renameat(parent, f.as_ptr(), parent, t.as_ptr()) }).map(|_| ())
    }

    impl Inner {
        pub fn create(root: &Path, instance_id: &str) -> std::io::Result<Inner> {
            let root_fd = std::fs::File::open(root)?;
            // Refused if it exists: mkdirat fails with EEXIST.
            mkdir_at(root_fd.as_raw_fd(), instance_id)?;
            let dir = open_dir_at(root_fd.as_raw_fd(), instance_id)?;
            Ok(Inner {
                dir,
                recorded: HashMap::new(),
                written: 0,
                serial: 0,
                temp: 0,
            })
        }

        /// Write `bytes` at `parts` below the instance folder: folders made
        /// as needed (recorded), the leaf created exclusively, or, when
        /// `overwrite` and the leaf is a recorded file of unchanged
        /// identity, replaced through a temporary leaf.
        pub fn write(
            &mut self,
            parts: &[String],
            bytes: &[u8],
            overwrite: bool,
        ) -> Result<(), SinkError> {
            let io = |e: std::io::Error| SinkError::Io(e.to_string());
            let shown = parts.join("/");
            if self.written + bytes.len() as u64 > QUOTA_BYTES {
                return Err(SinkError::Refused(format!(
                    "out '{shown}': the output folder's quota of {QUOTA_BYTES} bytes is used up"
                )));
            }
            let (leaf, dirs) = parts.split_last().expect("at least one component");
            let mut held: Option<OwnedFd> = None;
            let mut rel = String::new();
            for d in dirs {
                let parent = held
                    .as_ref()
                    .map_or(self.dir.as_raw_fd(), AsRawFd::as_raw_fd);
                if !rel.is_empty() {
                    rel.push('/');
                }
                rel.push_str(d);
                let opened = match open_dir_at(parent, d) {
                    Ok(fd) => fd,
                    Err(e) if e.raw_os_error() == Some(libc::ENOENT) => {
                        mkdir_at(parent, d).map_err(io)?;
                        let (dev, ino, _) = stat_at(parent, d).map_err(io)?;
                        self.recorded.insert(rel.clone(), (dev, ino));
                        open_dir_at(parent, d).map_err(io)?
                    }
                    Err(e) => {
                        return Err(SinkError::Refused(format!(
                            "out '{shown}': '{rel}' is not a folder the server can enter ({e})"
                        )));
                    }
                };
                held = Some(opened);
            }
            let parent = held
                .as_ref()
                .map_or(self.dir.as_raw_fd(), AsRawFd::as_raw_fd);
            match create_at(parent, leaf) {
                Ok(mut f) => {
                    f.write_all(bytes).map_err(io)?;
                    f.flush().map_err(io)?;
                    let (dev, ino, _) = stat_at(parent, leaf).map_err(io)?;
                    self.recorded.insert(shown, (dev, ino));
                }
                Err(e) if e.raw_os_error() == Some(libc::EEXIST) => {
                    let current = stat_at(parent, leaf).map_err(io)?;
                    let ours = self.recorded.get(&shown).copied();
                    if !overwrite || !current.2 || ours != Some((current.0, current.1)) {
                        return Err(SinkError::Refused(format!(
                            "out '{shown}': exists and is not a file this server wrote"
                        )));
                    }
                    self.temp += 1;
                    let tmp = format!(".{leaf}.alsem-tmp-{}", self.temp);
                    let mut f = create_at(parent, &tmp).map_err(io)?;
                    f.write_all(bytes).map_err(io)?;
                    f.flush().map_err(io)?;
                    let (dev, ino, _) = stat_at(parent, &tmp).map_err(io)?;
                    rename_at(parent, &tmp, leaf).map_err(io)?;
                    self.recorded.insert(shown, (dev, ino));
                }
                Err(e) => return Err(io(e)),
            }
            self.written += bytes.len() as u64;
            Ok(())
        }
    }
}

#[cfg(not(unix))]
mod imp {
    use super::SinkError;
    use std::path::Path;

    pub const WRITABLE: bool = false;

    /// Off Unix the sink writes nothing (design r7 §10, Windows fail-closed).
    pub struct Inner {
        pub written: u64,
        pub serial: u64,
    }

    impl Inner {
        pub fn create(_: &Path, _: &str) -> std::io::Result<Inner> {
            Ok(Inner {
                written: 0,
                serial: 0,
            })
        }

        pub fn write(&mut self, parts: &[String], _: &[u8], _: bool) -> Result<(), SinkError> {
            Err(super::inline_only_refusal(parts))
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn sink() -> (tempfile::TempDir, OutputSink) {
        let root = tempfile::tempdir().unwrap();
        let s = OutputSink::create(root.path(), &new_instance_id()).unwrap();
        (root, s)
    }

    /// Writes land inside the instance folder; a path outside it, a `..`
    /// and an absolute path elsewhere are refused.
    #[test]
    fn paths_outside_the_folder_are_refused() {
        let (_root, s) = sink();
        let p = s.write_out("a/b.json", b"x").unwrap();
        assert!(p.starts_with(s.instance()));
        assert_eq!(std::fs::read(&p).unwrap(), b"x");
        let inside = s.instance().join("c.txt");
        assert_eq!(s.write_out(inside.to_str().unwrap(), b"y").unwrap(), inside);
        for bad in ["../escape.txt", "/tmp/elsewhere.txt", "a/../../x", ""] {
            assert!(
                matches!(s.write_out(bad, b"z"), Err(SinkError::Refused(_))),
                "{bad}"
            );
        }
    }

    /// A recorded file may be overwritten; an entry the server did not
    /// create (planted in the folder), a replaced recorded file and a
    /// planted symlink folder are refused.
    #[test]
    fn overwrite_only_what_the_server_wrote() {
        let (_root, s) = sink();
        let p = s.write_out("r.json", b"one").unwrap();
        s.write_out("r.json", b"two").unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"two");
        std::fs::write(s.instance().join("planted.json"), b"theirs").unwrap();
        assert!(matches!(
            s.write_out("planted.json", b"mine"),
            Err(SinkError::Refused(_))
        ));
        assert_eq!(
            std::fs::read(s.instance().join("planted.json")).unwrap(),
            b"theirs"
        );
        // Replaced behind the server's back: a new identity.
        std::fs::remove_file(&p).unwrap();
        std::fs::write(&p, b"swapped").unwrap();
        assert!(matches!(
            s.write_out("r.json", b"three"),
            Err(SinkError::Refused(_))
        ));
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), s.instance().join("link")).unwrap();
        assert!(matches!(
            s.write_out("link/x.json", b"no"),
            Err(SinkError::Refused(_))
        ));
        assert_eq!(std::fs::read_dir(outside.path()).unwrap().count(), 0);
    }

    /// An instance folder that already exists is refused.
    #[test]
    fn an_existing_instance_folder_is_refused() {
        let root = tempfile::tempdir().unwrap();
        let id = new_instance_id();
        std::fs::create_dir(root.path().join(&id)).unwrap();
        assert!(OutputSink::create(root.path(), &id).is_err());
    }
}

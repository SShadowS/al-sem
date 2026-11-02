//! The child's private output namespace (context-server design §10).
//!
//! - The **anchor** is the output root `session/open` names: opened once as a
//!   directory handle.
//! - The **instance directory** `<root>/<engineInstanceId>/` is created by
//!   the child relative to that handle (`mkdirat`, mode `0700`) and
//!   **refused if it exists**; it is then opened `O_DIRECTORY | O_NOFOLLOW`
//!   and checked: a directory, owned by this user, no group or other bits.
//! - Every entry the child creates is created relative to the instance
//!   handle (`openat` with `O_CREAT | O_EXCL | O_NOFOLLOW`, mode `0600`) and
//!   **recorded with its identity** (device, inode).
//! - **Overwrite** is permitted only for a recorded entry whose current
//!   identity matches the record, through a temp leaf and `renameat`, under
//!   the directory's lock; anything else is refused.
//! - The child's own create/replace operations are serialized by that lock.
//!   Quota: [`QUOTA_BYTES`] per instance directory.
//!
//! Threat model (stated in §10): another process running as the same user
//! can mutate the directory; the contract defends against the child's own
//! races and other users. **Windows fails closed**: without handle-relative
//! creation no directory is made, so every spill fails; the child then
//! answers inline-only (an ask's inline cap raised to
//! [`crate::wire::MAX_INLINE_BYTES`]; a stdout larger still is answered
//! `refusedPath`, `child::ask_inline_only`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::wire::{SpillFile, SpillKind};

/// The most bytes one instance directory holds.
pub const QUOTA_BYTES: u64 = 1 << 30;

/// One child's private output directory (see the module doc).
pub struct OutputDir {
    path: PathBuf,
    #[cfg(unix)]
    dir: std::os::fd::OwnedFd,
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    /// Each created entry's identity (device, inode) and size.
    entries: BTreeMap<String, (u64, u64, u64)>,
    bytes: u64,
    next: u64,
}

#[cfg(unix)]
fn refused(why: impl Into<String>) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::PermissionDenied, why.into())
}

/// A leaf name the namespace accepts: one component, no dot names.
#[cfg(unix)]
fn leaf(name: &str) -> std::io::Result<std::ffi::CString> {
    if name.is_empty() || name == "." || name == ".." || name.contains('/') || name.contains('\0') {
        return Err(refused(format!("'{name}' is not a plain file name")));
    }
    std::ffi::CString::new(name).map_err(|e| refused(e.to_string()))
}

#[cfg(unix)]
mod sys {
    use std::ffi::CStr;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

    fn last() -> std::io::Error {
        std::io::Error::last_os_error()
    }

    pub fn open_dir(path: &std::path::Path) -> std::io::Result<OwnedFd> {
        use std::os::unix::ffi::OsStrExt;
        let c = std::ffi::CString::new(path.as_os_str().as_bytes())
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        // SAFETY: a NUL-terminated path; the returned fd is owned below.
        let fd = unsafe { libc::open(c.as_ptr(), libc::O_DIRECTORY | libc::O_CLOEXEC) };
        if fd < 0 {
            return Err(last());
        }
        // SAFETY: a fresh fd this call owns.
        Ok(unsafe { OwnedFd::from_raw_fd(fd) })
    }

    pub fn mkdirat(dir: &OwnedFd, name: &CStr) -> std::io::Result<()> {
        // SAFETY: a valid directory fd and a NUL-terminated leaf name.
        if unsafe { libc::mkdirat(dir.as_raw_fd(), name.as_ptr(), 0o700) } < 0 {
            return Err(last());
        }
        Ok(())
    }

    pub fn openat(dir: &OwnedFd, name: &CStr, flags: i32, mode: u32) -> std::io::Result<OwnedFd> {
        // SAFETY: a valid directory fd and a NUL-terminated leaf name; the
        // returned fd is owned below.
        let fd = unsafe {
            libc::openat(
                dir.as_raw_fd(),
                name.as_ptr(),
                flags | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                mode as libc::c_uint,
            )
        };
        if fd < 0 {
            return Err(last());
        }
        // SAFETY: a fresh fd this call owns.
        Ok(unsafe { OwnedFd::from_raw_fd(fd) })
    }

    /// (device, inode, mode, uid) of an open fd.
    pub fn fstat(fd: &OwnedFd) -> std::io::Result<(u64, u64, u32, u32)> {
        // SAFETY: a zeroed stat buffer filled by fstat on a valid fd.
        let mut st: libc::stat = unsafe { std::mem::zeroed() };
        if unsafe { libc::fstat(fd.as_raw_fd(), &mut st) } < 0 {
            return Err(last());
        }
        Ok((
            st.st_dev as u64,
            st.st_ino as u64,
            st.st_mode as u32,
            st.st_uid,
        ))
    }

    /// (device, inode) of `name` in `dir`, not following a link.
    pub fn fstatat(dir: &OwnedFd, name: &CStr) -> std::io::Result<(u64, u64)> {
        // SAFETY: as `fstat`, relative to a valid directory fd.
        let mut st: libc::stat = unsafe { std::mem::zeroed() };
        if unsafe {
            libc::fstatat(
                dir.as_raw_fd(),
                name.as_ptr(),
                &mut st,
                libc::AT_SYMLINK_NOFOLLOW,
            )
        } < 0
        {
            return Err(last());
        }
        Ok((st.st_dev as u64, st.st_ino as u64))
    }

    pub fn renameat(dir: &OwnedFd, from: &CStr, to: &CStr) -> std::io::Result<()> {
        // SAFETY: both names relative to one valid directory fd.
        if unsafe { libc::renameat(dir.as_raw_fd(), from.as_ptr(), dir.as_raw_fd(), to.as_ptr()) }
            < 0
        {
            return Err(last());
        }
        Ok(())
    }

    pub fn unlinkat(dir: &OwnedFd, name: &CStr) {
        // SAFETY: a name relative to a valid directory fd; failure ignored.
        unsafe { libc::unlinkat(dir.as_raw_fd(), name.as_ptr(), 0) };
    }

    pub fn euid() -> u32 {
        // SAFETY: no arguments, no failure.
        unsafe { libc::geteuid() }
    }
}

impl OutputDir {
    /// The instance directory `<root>/<instance>/`, created now (see the
    /// module doc). Refused if it exists, or if the root is not a directory.
    #[cfg(unix)]
    pub fn create(root: &Path, instance: &str) -> std::io::Result<OutputDir> {
        use std::io::ErrorKind;
        let name = leaf(instance)?;
        let anchor = sys::open_dir(root)?;
        match sys::mkdirat(&anchor, &name) {
            Ok(()) => {}
            Err(e) if e.kind() == ErrorKind::AlreadyExists => {
                return Err(refused(format!(
                    "the instance directory {} exists already: refused",
                    root.join(instance).display()
                )));
            }
            Err(e) => return Err(e),
        }
        let dir = sys::openat(&anchor, &name, libc::O_DIRECTORY | libc::O_RDONLY, 0)?;
        let (_, _, mode, uid) = sys::fstat(&dir)?;
        if mode & libc::S_IFMT != libc::S_IFDIR || uid != sys::euid() || mode & 0o077 != 0 {
            return Err(refused(format!(
                "the instance directory {} is not a private directory of this user",
                root.join(instance).display()
            )));
        }
        Ok(OutputDir {
            path: root.join(instance),
            dir,
            state: Mutex::new(State::default()),
        })
    }

    /// Windows and other platforms fail closed (no handle-relative creation).
    #[cfg(not(unix))]
    pub fn create(_root: &Path, _instance: &str) -> std::io::Result<OutputDir> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "no handle-relative creation on this platform: outputs are inline-only",
        ))
    }

    /// The instance directory.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Bytes held, and entries recorded.
    #[must_use]
    pub fn usage(&self) -> (u64, usize) {
        let s = self.state.lock().unwrap_or_else(|p| p.into_inner());
        (s.bytes, s.entries.len())
    }

    /// A new entry `name` holding `bytes` (refused if it exists).
    #[cfg(unix)]
    pub fn create_file(&self, name: &str, bytes: &[u8]) -> std::io::Result<PathBuf> {
        let mut s = self.state.lock().unwrap_or_else(|p| p.into_inner());
        self.quota(&s, bytes.len() as u64, 0)?;
        let c = leaf(name)?;
        let id = self.write_new(&c, bytes)?;
        s.bytes += bytes.len() as u64;
        s.entries
            .insert(name.to_string(), (id.0, id.1, bytes.len() as u64));
        Ok(self.path.join(name))
    }

    /// Replace the recorded entry `name` with `bytes`: permitted only while
    /// its current identity is the recorded one (a temp leaf, then
    /// `renameat` over it, under the lock); refused otherwise.
    #[cfg(unix)]
    pub fn replace_file(&self, name: &str, bytes: &[u8]) -> std::io::Result<PathBuf> {
        let mut s = self.state.lock().unwrap_or_else(|p| p.into_inner());
        let c = leaf(name)?;
        let Some(&(dev, ino, old)) = s.entries.get(name) else {
            return Err(refused(format!(
                "'{name}' is not an entry this engine created"
            )));
        };
        match sys::fstatat(&self.dir, &c) {
            Ok((d, i)) if (d, i) == (dev, ino) => {}
            _ => {
                return Err(refused(format!(
                    "'{name}' was replaced since this engine created it: refused"
                )));
            }
        }
        self.quota(&s, bytes.len() as u64, old)?;
        s.next += 1;
        let tmp = leaf(&format!(".tmp-{}-{name}", s.next))?;
        let id = self.write_new(&tmp, bytes)?;
        if let Err(e) = sys::renameat(&self.dir, &tmp, &c) {
            sys::unlinkat(&self.dir, &tmp);
            return Err(e);
        }
        s.bytes = s.bytes - old + bytes.len() as u64;
        s.entries
            .insert(name.to_string(), (id.0, id.1, bytes.len() as u64));
        Ok(self.path.join(name))
    }

    #[cfg(unix)]
    fn write_new(&self, name: &std::ffi::CStr, bytes: &[u8]) -> std::io::Result<(u64, u64)> {
        use std::io::Write;
        let fd = sys::openat(
            &self.dir,
            name,
            libc::O_CREAT | libc::O_EXCL | libc::O_WRONLY,
            0o600,
        )?;
        let (dev, ino, _, _) = sys::fstat(&fd)?;
        let mut f = std::fs::File::from(fd);
        f.write_all(bytes)?;
        f.flush()?;
        Ok((dev, ino))
    }

    #[cfg(unix)]
    fn quota(&self, s: &State, adding: u64, replacing: u64) -> std::io::Result<()> {
        if s.bytes - replacing + adding > QUOTA_BYTES {
            return Err(refused(format!(
                "the output directory's quota ({QUOTA_BYTES} bytes) is reached"
            )));
        }
        Ok(())
    }

    #[cfg(not(unix))]
    pub fn create_file(&self, _name: &str, _bytes: &[u8]) -> std::io::Result<PathBuf> {
        unreachable!("no OutputDir exists on this platform")
    }

    /// Spilled bytes as a new entry with an opaque name.
    pub fn spill(&self, kind: SpillKind, bytes: &[u8]) -> std::io::Result<SpillFile> {
        let n = {
            let mut s = self.state.lock().unwrap_or_else(|p| p.into_inner());
            s.next += 1;
            s.next
        };
        let tag = match kind {
            SpillKind::Stdout => "stdout",
            SpillKind::Stderr => "stderr",
            SpillKind::Diagnostic => "diagnostic",
        };
        let path = self.create_file(&format!("{n}.{tag}"), bytes)?;
        Ok(SpillFile::of(path.to_string_lossy().into_owned(), bytes))
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    /// T-paths (§10, §11): the instance directory is created owner-only and
    /// refused if it exists; a recorded entry is overwritten in place; an
    /// entry replaced behind the engine's back is refused; an entry the
    /// engine did not create, or a name that is not a plain leaf, is refused.
    ///
    /// DISCRIMINATION (2026-11-02): dropping the identity check in
    /// `replace_file` (`if true || (d, i) == (dev, ino)`) fails at the
    /// replaced entry: `replace_file("a.json", ..).unwrap_err()` panics (the
    /// overwrite succeeds).
    #[test]
    fn the_private_namespace_contract() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let out = OutputDir::create(root.path(), "inst").unwrap();
        let mode = std::fs::metadata(out.path()).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700, "owner-only");
        let again = OutputDir::create(root.path(), "inst");
        assert!(
            again
                .as_ref()
                .err()
                .is_some_and(|e| e.to_string().contains("exists already")),
            "a pre-existing instance directory is refused"
        );
        // Create, then overwrite in place (a recorded entry).
        let p = out.create_file("a.json", b"one").unwrap();
        assert!(
            out.create_file("a.json", b"two").is_err(),
            "create never overwrites"
        );
        out.replace_file("a.json", b"two").unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"two");
        assert_eq!(out.usage(), (3, 1));
        // An entry replaced by someone else (a new inode) is refused.
        std::fs::remove_file(&p).unwrap();
        std::fs::write(&p, b"theirs").unwrap();
        let e = out.replace_file("a.json", b"mine").unwrap_err();
        assert!(
            e.to_string().contains("was replaced"),
            "a replaced entry is refused: {e}"
        );
        assert_eq!(std::fs::read(&p).unwrap(), b"theirs");
        // A symlink planted under a recorded name is refused too.
        let q = out.create_file("b.json", b"x").unwrap();
        std::fs::remove_file(&q).unwrap();
        std::os::unix::fs::symlink(root.path().join("elsewhere"), &q).unwrap();
        assert!(out.replace_file("b.json", b"y").is_err());
        assert!(
            !root.path().join("elsewhere").exists(),
            "nothing written through the link"
        );
        // Never created by the engine; not a leaf.
        assert!(out.replace_file("c.json", b"z").is_err());
        assert!(out.create_file("../escape", b"z").is_err());
        assert!(out.create_file("..", b"z").is_err());
        // Spills land inside, opaque names, exact bytes.
        let f = out.spill(SpillKind::Stdout, b"hello\n").unwrap();
        assert!(Path::new(&f.path).starts_with(out.path()));
        assert_eq!(std::fs::read(&f.path).unwrap(), b"hello\n");
    }
}

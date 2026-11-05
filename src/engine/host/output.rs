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
//!   recorded. A named entry is also **pinned**: the child keeps the
//!   descriptor it created the file through ([`Pinned`]).
//! - **Overwrite** is permitted only for a pinned entry whose name still
//!   holds that very file ([`is_pinned_entry`]: the name is a regular file,
//!   not a link, with the pinned descriptor's device and inode, and the
//!   pinned file still has a link). It goes through a temp leaf and
//!   `renameat`, under the directory's lock; anything else is refused.
//!
//!   Why a descriptor and not the recorded (device, inode) pair: a freed
//!   inode number is handed out again, and ext4 does it at once, so a file
//!   or symlink planted after the engine's file was removed can carry the
//!   very same pair (the CI runner's failure, 2026-11-02 to 2026-11-05). An
//!   open descriptor keeps the inode allocated, so no other file on the
//!   device can have its number while it is pinned. At most [`MAX_PINNED`]
//!   entries are pinned (one descriptor each); an older entry is unpinned,
//!   and its overwrite is refused from then on (fail closed). Spills are
//!   never overwritten and are not pinned.
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

/// The most entries one namespace keeps overwritable, each pinned by one
/// open descriptor (see the module doc). Shared with the MCP server's
/// output folder (`crate::mcp::output`).
pub const MAX_PINNED: usize = 256;

/// What the identity check sees of one inode.
#[cfg(unix)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Inode {
    pub dev: u64,
    pub ino: u64,
    /// The number of names the inode has (0: it was removed).
    pub nlink: u64,
    /// A regular file (not a symlink, a folder or anything else).
    pub regular: bool,
}

/// Whether `current` (the entry's name now, not following a link) is the
/// pinned file `pinned` (its open descriptor). The device and inode alone
/// are not enough: an inode number freed by a removal is reused, so the
/// pinned file must also still have a name, and the name must be a regular
/// file.
#[cfg(unix)]
pub(crate) fn is_pinned_entry(pinned: Inode, current: Inode) -> bool {
    pinned.nlink > 0 && current.regular && (pinned.dev, pinned.ino) == (current.dev, current.ino)
}

/// The entries a namespace may overwrite, each pinned by the descriptor it
/// was created through, at most [`MAX_PINNED`] (the oldest is unpinned).
#[cfg(unix)]
#[derive(Default)]
pub(crate) struct Pinned {
    held: std::collections::HashMap<String, (std::os::fd::OwnedFd, u64)>,
    seq: u64,
}

#[cfg(unix)]
impl Pinned {
    /// Pin `key` to `fd` (replacing an earlier pin of the same key).
    pub(crate) fn pin(&mut self, key: &str, fd: std::os::fd::OwnedFd) {
        self.seq += 1;
        self.held.insert(key.to_string(), (fd, self.seq));
        if self.held.len() > MAX_PINNED {
            let oldest = self
                .held
                .iter()
                .min_by_key(|(_, (_, n))| *n)
                .map(|(k, _)| k.clone());
            if let Some(k) = oldest {
                self.held.remove(&k);
            }
        }
    }

    /// Whether `key` is pinned.
    pub(crate) fn holds(&self, key: &str) -> bool {
        self.held.contains_key(key)
    }

    /// Whether `leaf` in the directory `parent` is still the file pinned
    /// under `key` ([`is_pinned_entry`]). False when `key` is not pinned or
    /// either side cannot be read.
    pub(crate) fn is_current(
        &self,
        key: &str,
        parent: std::os::fd::RawFd,
        leaf: &std::ffi::CStr,
    ) -> bool {
        use std::os::fd::AsRawFd;
        let Some((fd, _)) = self.held.get(key) else {
            return false;
        };
        match (sys::inode_of(fd.as_raw_fd()), sys::inode_at(parent, leaf)) {
            (Ok(p), Ok(c)) => is_pinned_entry(p, c),
            _ => false,
        }
    }
}

/// One child's private output directory (see the module doc).
pub struct OutputDir {
    path: PathBuf,
    #[cfg(unix)]
    dir: std::os::fd::OwnedFd,
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    /// Each created entry's size.
    entries: BTreeMap<String, u64>,
    /// The named entries that may be overwritten.
    #[cfg(unix)]
    pinned: Pinned,
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
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};

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

    #[allow(clippy::unnecessary_cast)]
    fn inode(st: &libc::stat) -> super::Inode {
        super::Inode {
            dev: st.st_dev as u64,
            ino: st.st_ino as u64,
            nlink: st.st_nlink as u64,
            regular: st.st_mode & libc::S_IFMT == libc::S_IFREG,
        }
    }

    /// The inode an open descriptor refers to.
    pub fn inode_of(fd: RawFd) -> std::io::Result<super::Inode> {
        // SAFETY: a zeroed stat buffer filled by fstat on a valid fd.
        let mut st: libc::stat = unsafe { std::mem::zeroed() };
        if unsafe { libc::fstat(fd, &mut st) } < 0 {
            return Err(last());
        }
        Ok(inode(&st))
    }

    /// The inode `name` in the directory `dir` names, not following a link.
    pub fn inode_at(dir: RawFd, name: &CStr) -> std::io::Result<super::Inode> {
        // SAFETY: as `inode_of`, relative to a valid directory fd.
        let mut st: libc::stat = unsafe { std::mem::zeroed() };
        if unsafe { libc::fstatat(dir, name.as_ptr(), &mut st, libc::AT_SYMLINK_NOFOLLOW) } < 0 {
            return Err(last());
        }
        Ok(inode(&st))
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

    /// A new entry `name` holding `bytes` (refused if it exists), pinned so
    /// [`OutputDir::replace_file`] may overwrite it later.
    #[cfg(unix)]
    pub fn create_file(&self, name: &str, bytes: &[u8]) -> std::io::Result<PathBuf> {
        self.create_entry(name, bytes, true)
    }

    #[cfg(unix)]
    fn create_entry(&self, name: &str, bytes: &[u8], pin: bool) -> std::io::Result<PathBuf> {
        let mut s = self.state.lock().unwrap_or_else(|p| p.into_inner());
        self.quota(&s, bytes.len() as u64, 0)?;
        let c = leaf(name)?;
        let fd = self.write_new(&c, bytes)?;
        s.bytes += bytes.len() as u64;
        s.entries.insert(name.to_string(), bytes.len() as u64);
        if pin {
            s.pinned.pin(name, fd);
        }
        Ok(self.path.join(name))
    }

    /// Replace the entry `name` with `bytes`: permitted only while it is
    /// pinned and its name still holds the pinned file (a temp leaf, then
    /// `renameat` over it, under the lock); refused otherwise.
    #[cfg(unix)]
    pub fn replace_file(&self, name: &str, bytes: &[u8]) -> std::io::Result<PathBuf> {
        use std::os::fd::AsRawFd;
        let mut s = self.state.lock().unwrap_or_else(|p| p.into_inner());
        let c = leaf(name)?;
        let Some(&old) = s.entries.get(name) else {
            return Err(refused(format!(
                "'{name}' is not an entry this engine created"
            )));
        };
        if !s.pinned.holds(name) {
            return Err(refused(format!(
                "'{name}' is no longer overwritable (only the {MAX_PINNED} newest named \
                 entries are): refused"
            )));
        }
        if !s.pinned.is_current(name, self.dir.as_raw_fd(), &c) {
            return Err(refused(format!(
                "'{name}' was replaced since this engine created it: refused"
            )));
        }
        self.quota(&s, bytes.len() as u64, old)?;
        s.next += 1;
        let tmp = leaf(&format!(".tmp-{}-{name}", s.next))?;
        let fd = self.write_new(&tmp, bytes)?;
        if let Err(e) = sys::renameat(&self.dir, &tmp, &c) {
            sys::unlinkat(&self.dir, &tmp);
            return Err(e);
        }
        s.bytes = s.bytes - old + bytes.len() as u64;
        s.entries.insert(name.to_string(), bytes.len() as u64);
        s.pinned.pin(name, fd);
        Ok(self.path.join(name))
    }

    /// Create `name` exclusively (never through a link) holding `bytes`;
    /// the descriptor it was written through.
    #[cfg(unix)]
    fn write_new(
        &self,
        name: &std::ffi::CStr,
        bytes: &[u8],
    ) -> std::io::Result<std::os::fd::OwnedFd> {
        use std::io::Write;
        let fd = sys::openat(
            &self.dir,
            name,
            libc::O_CREAT | libc::O_EXCL | libc::O_WRONLY,
            0o600,
        )?;
        let mut f = std::fs::File::from(fd);
        f.write_all(bytes)?;
        f.flush()?;
        Ok(f.into())
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
    fn create_entry(&self, _name: &str, _bytes: &[u8], _pin: bool) -> std::io::Result<PathBuf> {
        unreachable!("no OutputDir exists on this platform")
    }

    /// Spilled bytes as a new entry with an opaque name (never overwritten,
    /// so not pinned).
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
        let path = self.create_entry(&format!("{n}.{tag}"), bytes, false)?;
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
    /// The replaced and symlinked cases do not depend on inode numbers: the
    /// pinned file was removed, so it has no link left (on ext4 the planted
    /// entry gets the freed inode number, which made the old (device, inode)
    /// check pass it on the CI runner; on btrfs and tmpfs it does not, so it
    /// passed locally). `identity_needs_a_live_pinned_regular_file` states
    /// that same-number case by hand.
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
        let e = out.replace_file("b.json", b"y").unwrap_err();
        assert!(e.to_string().contains("was replaced"), "{e}");
        assert!(
            !root.path().join("elsewhere").exists(),
            "nothing written through the link"
        );
        assert!(
            std::fs::symlink_metadata(&q)
                .unwrap()
                .file_type()
                .is_symlink(),
            "the planted link is left as it was"
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

    /// The identity rule with its precondition stated by hand: an entry
    /// whose name now carries the pinned file's very (device, inode) — an
    /// inode number reused after a removal, as ext4 does — is NOT the pinned
    /// file when the pinned file has no link left, nor when the name is not
    /// a regular file (a symlink). Only a linked pinned file under a regular
    /// name of the same identity is.
    ///
    /// DISCRIMINATION (2026-11-05): replacing `is_pinned_entry`'s body with
    /// the old rule `(pinned.dev, pinned.ino) == (current.dev, current.ino)`
    /// fails at "a reused inode number" (true); restored, it passes.
    #[test]
    fn identity_needs_a_live_pinned_regular_file() {
        let file = |nlink, regular| Inode {
            dev: 7,
            ino: 42,
            nlink,
            regular,
        };
        assert!(
            is_pinned_entry(file(1, true), file(1, true)),
            "the same file"
        );
        assert!(
            !is_pinned_entry(file(0, true), file(1, true)),
            "a reused inode number: the pinned file was removed"
        );
        assert!(
            !is_pinned_entry(file(0, true), file(1, false)),
            "a symlink planted at a reused inode number"
        );
        assert!(
            !is_pinned_entry(file(1, true), file(1, false)),
            "a name that is not a regular file"
        );
        let other = Inode {
            ino: 43,
            ..file(1, true)
        };
        assert!(!is_pinned_entry(file(1, true), other), "another inode");
    }

    /// At most `MAX_PINNED` named entries stay overwritable: the oldest is
    /// unpinned and its overwrite refused (fail closed), the newest still
    /// overwrites; spills take no pin.
    #[test]
    fn only_the_newest_entries_stay_overwritable() {
        let root = tempfile::tempdir().unwrap();
        let out = OutputDir::create(root.path(), "inst").unwrap();
        out.create_file("first.json", b"1").unwrap();
        for i in 0..MAX_PINNED - 1 {
            out.create_file(&format!("{i}.json"), b"n").unwrap();
        }
        // MAX_PINNED named entries are pinned; spills take no pin, so
        // "first.json" stays overwritable after them.
        for _ in 0..MAX_PINNED + 1 {
            out.spill(SpillKind::Stdout, b"s").unwrap();
        }
        out.replace_file("first.json", b"2").unwrap();
        // ... and that overwrite re-pinned it as the newest: one more named
        // entry unpins "0.json", the oldest.
        out.create_file(&format!("{}.json", MAX_PINNED - 1), b"n")
            .unwrap();
        out.replace_file("first.json", b"3").unwrap();
        let e = out.replace_file("0.json", b"2").unwrap_err();
        assert!(e.to_string().contains("no longer overwritable"), "{e}");
        let last = format!("{}.json", MAX_PINNED - 1);
        out.replace_file(&last, b"m").unwrap();
        assert_eq!(std::fs::read(out.path().join(last)).unwrap(), b"m");
    }
}

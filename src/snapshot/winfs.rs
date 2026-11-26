//! TLS-117: the Windows checks the caches need (design note
//! `docs/superpowers/notes/2026-11-21-tls117-windows-image-key.md`, sections 3 and 6).
//!
//! - **Locality** ([`local_ntfs`]): a fixed drive (`GetDriveTypeW` on the handle's
//!   `\\?\Volume{GUID}\` root) formatted NTFS. Anything else (ReFS, FAT, a removable or network
//!   drive) is refused.
//! - **The private rule** ([`check_handle`], [`walk`]): read on an opened handle, an object is
//!   private when it is not a reparse point, has a DACL, is owned by a trusted principal (this
//!   user, SYSTEM, Administrators, TrustedInstaller) and no allow ACE that applies to it gives an
//!   untrusted principal a refused right. Deny ACEs are skipped; any other ACE type refuses. Every
//!   path element from the volume root down is checked with [`CHAIN_BAD`] (what lets someone
//!   remove, rename or re-ACL it), cache folders, entries and lock files with [`LEAF_BAD`] (also
//!   adding to them or writing them). [`walk`] goes top-down and returns the rooted spelling all
//!   cache I/O uses; a missing element is made only under its just-checked parent.
//!
//! Scope (ENG-110): accidental damage and the run's own doing; another process changing files
//! on purpose mid-run is out of scope. The rule keeps other principals out of the path.

use std::ffi::{OsStr, c_void};
use std::fs::{File, OpenOptions};
use std::io::ErrorKind;
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::os::windows::io::AsRawHandle;
use std::path::{Component, Path, PathBuf, Prefix};
use std::sync::OnceLock;
use windows_sys::Win32::Foundation::{CloseHandle, GENERIC_ALL, GENERIC_WRITE, HANDLE, LocalFree};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, GetSecurityInfo, SE_FILE_OBJECT,
};
use windows_sys::Win32::Security::{
    ACCESS_ALLOWED_ACE, ACE_HEADER, ACL, DACL_SECURITY_INFORMATION, GetAce, GetTokenInformation,
    INHERIT_ONLY_ACE, OWNER_SECURITY_INFORMATION, PSID, TOKEN_QUERY, TOKEN_USER, TokenUser,
};
use windows_sys::Win32::Storage::FileSystem::{
    DELETE, FILE_ADD_FILE, FILE_ADD_SUBDIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT, FILE_DELETE_CHILD,
    FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES,
    FILE_SHARE_READ, FILE_SHARE_WRITE, GetDriveTypeW, GetFinalPathNameByHandleW,
    GetVolumeInformationByHandleW, READ_CONTROL, VOLUME_NAME_GUID, WRITE_DAC, WRITE_OWNER,
};
use windows_sys::Win32::System::SystemServices::{ACCESS_ALLOWED_ACE_TYPE, ACCESS_DENIED_ACE_TYPE};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

/// `DRIVE_FIXED` (in `Win32_System_WindowsProgramming`, a feature not otherwise needed).
const DRIVE_FIXED: u32 = 3;

/// Rights no one but the trusted may hold on any path element.
pub(crate) const CHAIN_BAD: u32 =
    DELETE | WRITE_DAC | WRITE_OWNER | FILE_DELETE_CHILD | GENERIC_WRITE | GENERIC_ALL;
/// Also refused on cache folders, entries and lock files (`FILE_WRITE_DATA` and
/// `FILE_APPEND_DATA` are the same bits as `FILE_ADD_FILE` and `FILE_ADD_SUBDIRECTORY`).
pub(crate) const LEAF_BAD: u32 = CHAIN_BAD | FILE_ADD_FILE | FILE_ADD_SUBDIRECTORY;

fn lossy(w: &[u16]) -> String {
    String::from_utf16_lossy(w)
}

/// The file system name of the volume holding `f` ("NTFS", "ReFS", ...).
fn fs_name(f: &File) -> Option<String> {
    let mut name = [0u16; 64];
    // SAFETY: a live handle; only the file system name buffer is asked for.
    let ok = unsafe {
        GetVolumeInformationByHandleW(
            f.as_raw_handle(),
            std::ptr::null_mut(),
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            name.as_mut_ptr(),
            name.len() as u32,
        )
    };
    let end = name.iter().position(|&c| c == 0).unwrap_or(name.len());
    (ok != 0).then(|| lossy(&name[..end]))
}

/// The drive type of the volume holding `f`, from its `\\?\Volume{GUID}\` root.
fn drive_type(f: &File) -> Option<u32> {
    let mut buf = vec![0u16; 32768];
    // SAFETY: a live handle; the buffer and its length match.
    let n = unsafe {
        GetFinalPathNameByHandleW(
            f.as_raw_handle(),
            buf.as_mut_ptr(),
            buf.len() as u32,
            VOLUME_NAME_GUID,
        )
    } as usize;
    if n == 0 || n >= buf.len() {
        return None;
    }
    // `\\?\Volume{...}\rest`: the root ends at the first `\` after the `}`.
    let close = buf[..n].iter().position(|&c| c == u16::from(b'}'))?;
    let mut root = buf[..=close + 1].to_vec();
    if root.last() != Some(&u16::from(b'\\')) {
        return None;
    }
    root.push(0);
    // SAFETY: a NUL-terminated root path.
    Some(unsafe { GetDriveTypeW(root.as_ptr()) })
}

/// The locality rule: a fixed drive formatted NTFS.
pub(crate) fn local_ntfs(f: &File) -> Result<(), String> {
    match (fs_name(f).as_deref(), drive_type(f)) {
        (Some("NTFS"), Some(DRIVE_FIXED)) => Ok(()),
        (fs, dt) => Err(format!(
            "not a local NTFS volume (file system {fs:?}, drive type {dt:?})"
        )),
    }
}

/// The trusted principals as SID strings: this user, SYSTEM, Administrators, TrustedInstaller
/// (the owner of system folders such as a volume root).
struct Trust(Vec<String>);

fn sid_string(sid: PSID) -> Option<String> {
    let mut p: *mut u16 = std::ptr::null_mut();
    // SAFETY: a valid SID; `p` is freed with LocalFree below.
    if unsafe { ConvertSidToStringSidW(sid, &mut p) } == 0 {
        return None;
    }
    // SAFETY: a NUL-terminated string from the call above.
    let s = unsafe {
        let len = (0..).take_while(|&i| *p.add(i) != 0).count();
        lossy(std::slice::from_raw_parts(p, len))
    };
    // SAFETY: allocated by ConvertSidToStringSidW.
    unsafe { LocalFree(p.cast()) };
    Some(s)
}

fn read_trust() -> Result<Trust, String> {
    let mut tok: HANDLE = std::ptr::null_mut();
    // SAFETY: the current process's pseudo-handle; `tok` is closed below.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut tok) } == 0 {
        return Err(format!(
            "OpenProcessToken: {}",
            std::io::Error::last_os_error()
        ));
    }
    // u64 storage: TOKEN_USER holds pointers and needs their alignment.
    let mut buf = vec![0u64; 128];
    let mut len = 0u32;
    // SAFETY: the buffer and its length in bytes match.
    let ok = unsafe {
        GetTokenInformation(
            tok,
            TokenUser,
            buf.as_mut_ptr().cast(),
            (buf.len() * 8) as u32,
            &mut len,
        )
    };
    // SAFETY: opened above.
    unsafe { CloseHandle(tok) };
    if ok == 0 {
        return Err(format!(
            "GetTokenInformation: {}",
            std::io::Error::last_os_error()
        ));
    }
    // SAFETY: the call filled an aligned TOKEN_USER.
    let user = unsafe { (*buf.as_ptr().cast::<TOKEN_USER>()).User.Sid };
    let user = sid_string(user).ok_or("the user's SID")?;
    Ok(Trust(vec![
        user,
        "S-1-5-18".into(),
        "S-1-5-32-544".into(),
        "S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464".into(),
    ]))
}

/// The process's trusted principals, read once.
fn trust() -> Result<&'static Trust, String> {
    static T: OnceLock<Result<Trust, String>> = OnceLock::new();
    T.get_or_init(read_trust).as_ref().map_err(Clone::clone)
}

impl Trust {
    fn has(&self, sid: PSID) -> bool {
        sid_string(sid).is_some_and(|s| self.0.contains(&s))
    }
}

struct Sd(*mut c_void);
impl Drop for Sd {
    fn drop(&mut self) {
        // SAFETY: allocated by GetSecurityInfo.
        unsafe { LocalFree(self.0) };
    }
}

/// The private rule on an open handle (it needs READ_CONTROL and attribute access).
pub(crate) fn check_handle(f: &File, bad: u32) -> Result<(), String> {
    let t = trust()?;
    let attrs = f
        .metadata()
        .map_err(|e| format!("stat: {e}"))?
        .file_attributes();
    if attrs & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err("a reparse point".into());
    }
    let (mut owner, mut dacl, mut sd): (PSID, *mut ACL, *mut c_void) = (
        std::ptr::null_mut(),
        std::ptr::null_mut(),
        std::ptr::null_mut(),
    );
    // SAFETY: a live handle; the out-pointers are valid; `sd` is freed by `Sd`.
    let err = unsafe {
        GetSecurityInfo(
            f.as_raw_handle(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            &mut owner,
            std::ptr::null_mut(),
            &mut dacl,
            std::ptr::null_mut(),
            &mut sd,
        )
    };
    if err != 0 {
        return Err(format!("GetSecurityInfo error {err}"));
    }
    let _sd = Sd(sd);
    if dacl.is_null() {
        return Err("an absent or null DACL".into());
    }
    if owner.is_null() || !t.has(owner) {
        return Err(format!(
            "owner {:?} is not trusted",
            (!owner.is_null()).then(|| sid_string(owner))
        ));
    }
    // SAFETY: `dacl` points into `sd`, alive until `_sd` drops.
    let count = unsafe { (*dacl).AceCount };
    for i in 0..count {
        let mut ace: *mut c_void = std::ptr::null_mut();
        // SAFETY: i < AceCount.
        if unsafe { GetAce(dacl, u32::from(i), &mut ace) } == 0 {
            return Err(format!("unreadable ACE {i}"));
        }
        // SAFETY: every ACE starts with an ACE_HEADER.
        let h = unsafe { *(ace as *const ACE_HEADER) };
        if u32::from(h.AceFlags) & INHERIT_ONLY_ACE != 0 {
            continue; // applies to children only
        }
        match u32::from(h.AceType) {
            ACCESS_DENIED_ACE_TYPE => {}
            ACCESS_ALLOWED_ACE_TYPE => {
                // SAFETY: the type says ACCESS_ALLOWED_ACE.
                let a = unsafe { &*(ace as *const ACCESS_ALLOWED_ACE) };
                let sid: PSID = (&raw const a.SidStart).cast_mut().cast();
                if a.Mask & bad != 0 && !t.has(sid) {
                    return Err(format!(
                        "ACE {i} gives {:?} rights {:#x}",
                        sid_string(sid),
                        a.Mask & bad
                    ));
                }
            }
            other => return Err(format!("ACE {i} of a type not interpreted ({other})")),
        }
    }
    Ok(())
}

/// Open one path element for checking: attributes and READ_CONTROL, not following a reparse
/// point, sharing read and write but not delete, so it cannot be renamed while the walk holds it.
fn open_element(p: &Path) -> std::io::Result<File> {
    OpenOptions::new()
        .access_mode(READ_CONTROL | FILE_READ_ATTRIBUTES)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(p)
}

/// The walk: from `\\?\X:\` down to `dir`, each element opened and checked before the next is
/// opened under it (a reparse point is refused before descending), every element with
/// [`CHAIN_BAD`], `dir` itself with [`LEAF_BAD`]; the volume must be local NTFS. With `create`, a
/// missing element is made under its checked parent, then checked. Returns the rooted spelling
/// (`\\?\X:\...`) all cache I/O must use.
pub(crate) fn walk(dir: &Path, create: bool) -> Result<PathBuf, String> {
    let mut comps = dir.components();
    let root = match comps.next() {
        Some(Component::Prefix(p)) => match p.kind() {
            Prefix::Disk(l) | Prefix::VerbatimDisk(l) => {
                PathBuf::from(format!(r"\\?\{}:\", char::from(l)))
            }
            other => return Err(format!("not a drive-letter path: {other:?}")),
        },
        _ => return Err("not an absolute path".into()),
    };
    if comps.next() != Some(Component::RootDir) {
        return Err("not an absolute path".into());
    }
    let names: Vec<&OsStr> = comps
        .map(|c| match c {
            Component::Normal(n) => Ok(n),
            other => Err(format!("a {other:?} component")),
        })
        .collect::<Result<_, _>>()?;
    let mut held = Vec::new();
    let f = open_element(&root).map_err(|e| format!("open the volume root: {e}"))?;
    local_ntfs(&f)?;
    let bad = if names.is_empty() {
        LEAF_BAD
    } else {
        CHAIN_BAD
    };
    check_handle(&f, bad).map_err(|e| format!("{}: {e}", root.display()))?;
    held.push(f);
    let mut cur = root;
    for (i, n) in names.iter().enumerate() {
        cur.push(n);
        let f = match open_element(&cur) {
            Err(e) if e.kind() == ErrorKind::NotFound && create => {
                std::fs::create_dir(&cur).map_err(|e| format!("create {}: {e}", cur.display()))?;
                open_element(&cur).map_err(|e| format!("open {}: {e}", cur.display()))?
            }
            r => r.map_err(|e| format!("open {}: {e}", cur.display()))?,
        };
        let bad = if i + 1 == names.len() {
            LEAF_BAD
        } else {
            CHAIN_BAD
        };
        check_handle(&f, bad).map_err(|e| format!("{}: {e}", cur.display()))?;
        if !f
            .metadata()
            .map_err(|e| format!("stat {}: {e}", cur.display()))?
            .is_dir()
        {
            return Err(format!("{}: not a directory", cur.display()));
        }
        held.push(f);
    }
    Ok(cur)
}

/// Native tests (the owner's Windows acceptance run: `cargo test -p al-sem --lib winfs`).
#[cfg(test)]
mod tests {
    use super::*;

    fn icacls(p: &Path, args: &[&str]) {
        let st = std::process::Command::new("icacls")
            .arg(p)
            .args(args)
            .stdout(std::process::Stdio::null())
            .status()
            .unwrap();
        assert!(st.success(), "icacls {} {args:?}", p.display());
    }

    /// The user's cache folder passes; a leaf Everyone can modify, a private leaf under a parent
    /// giving Everyone delete-child, and a junction are refused; an entry Everyone can modify
    /// fails the handle check. (Windows only; the same rule passed the probe's W6 natively.)
    #[test]
    fn windows_private_walk() {
        let lad = dirs::cache_dir().unwrap();
        walk(&lad, false).expect("the per-user cache folder passes");
        let d = tempfile::Builder::new()
            .prefix("tls117-")
            .tempdir_in(&lad)
            .unwrap();
        let open = d.path().join("open");
        std::fs::create_dir(&open).unwrap();
        icacls(&open, &["/grant", "*S-1-1-0:(M)"]);
        assert!(walk(&open, false).is_err(), "a leaf Everyone can modify");
        let parent = d.path().join("parent");
        let leaf = parent.join("leaf");
        std::fs::create_dir_all(&leaf).unwrap();
        icacls(&parent, &["/grant", "*S-1-1-0:(DC)"]);
        assert!(walk(&leaf, false).is_err(), "a replaceable parent");
        // A refused parent gets no child: the walk stops before it makes anything under it.
        let child = parent.join("new-child");
        assert!(
            walk(&child, true).is_err(),
            "a create under a refused parent"
        );
        assert!(!child.exists(), "nothing made under a refused parent");
        let target = d.path().join("target");
        std::fs::create_dir(&target).unwrap();
        let junction = d.path().join("junction");
        let st = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&junction)
            .arg(&target)
            .stdout(std::process::Stdio::null())
            .status()
            .unwrap();
        assert!(st.success());
        let err = walk(&junction.join("below"), false).unwrap_err();
        assert!(err.contains("reparse"), "{err}");
        let loose = d.path().join("loose.bin");
        std::fs::write(&loose, b"x").unwrap();
        icacls(&loose, &["/grant", "*S-1-1-0:(M)"]);
        assert!(check_handle(&File::open(&loose).unwrap(), LEAF_BAD).is_err());
        let fresh = d.path().join("fresh.bin");
        std::fs::write(&fresh, b"x").unwrap();
        check_handle(&File::open(&fresh).unwrap(), LEAF_BAD).expect("a fresh entry passes");
    }
}

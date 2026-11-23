//! The IR cache's key (spec "The key"): the blake3 of the running executable's bytes and the
//! IR format, so whatever produces or reads the IR (grammar, lowerer, codec, toolchain,
//! features) is in the key by construction.
//!
//! - **Linux:** the image is read through `/proc/self/exe`, the inode the process runs even when
//!   its path was rebuilt since.
//! - **Windows (TLS-117, design B; design note
//!   `docs/superpowers/notes/2026-11-21-tls117-windows-image-key.md`):** no path names the
//!   running image reliably, so identity comes from what runs. Part A: the canonical hash of the
//!   loaded module's read-only content ([`super::pe`]). Part B: each candidate file (the mapped
//!   name, then `GetModuleFileNameW`), opened denying write and delete, on local NTFS, must have
//!   the same headers and canonical hash, its writable sections and masked bytes must hash to the
//!   digest stamped in the running module's slot ([`IR_DATA_SLOT`]), and its own slot must equal
//!   the loaded one. The key is then [`key_of`] over the accepted file's bytes, read through that
//!   handle. An unstamped binary (every `cargo` build) gets no key; so does any other failure, with
//!   its reason in the report.

use std::io::Read;

/// Why a process has no key ([`image_key`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum NoKey {
    /// A Windows binary whose slot holds no digest: not stamped (every dev build).
    #[cfg_attr(not(windows), allow(dead_code))]
    Unstamped,
    /// A system the cache does not support (made only off Linux and Windows).
    #[cfg_attr(any(target_os = "linux", windows), allow(dead_code))]
    Unsupported,
    /// Anything else, with the reason.
    Failed(String),
}

impl std::fmt::Display for NoKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NoKey::Unstamped => f.write_str("unstamped binary: its data-digest slot is empty"),
            NoKey::Unsupported => f.write_str("not supported on this system"),
            NoKey::Failed(r) => f.write_str(r),
        }
    }
}

/// A cache key: names one key folder (`ir/<64 hex>/`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct IrKey(pub [u8; 32]);

impl IrKey {
    pub(crate) fn hex(&self) -> String {
        blake3::Hash::from_bytes(self.0).to_hex().to_string()
    }
}

/// Marks an image as an engine binary of this repository: under valgrind or a similar loader
/// `/proc/self/exe` is the tool, which lacks it, so no key is made. `#[used]` and read through
/// `black_box` (in [`image_key`]), so neither the compiler nor the linker drops it.
/// `scripts/check-ir-image` searches a built binary for these bytes.
#[used]
static IR_IMAGE_MARKER: [u8; 16] = *b"ALSEM-IR-IMAGE-1";

/// The data-digest slot (TLS-117): a 16-byte magic, then 32 digest bytes, zero until
/// `alsem-stamp` writes the digest of the built binary's writable sections and masked bytes
/// ([`super::pe::data_digest`]). Immutable, so the linker puts it in `.rdata`, which the Windows
/// key's part A hashes. `#[used]` and every read through [`slot`] (`black_box`, as for the
/// marker) keep it, and keep the optimiser from assuming its build-time zeros. The magic is
/// spelled only here; code that needs it reads it back from this static ([`slot_magic`]), so a
/// binary carries it exactly once (fixtures use a test-only magic).
#[used]
static IR_DATA_SLOT: [u8; super::pe::SLOT_LEN] = {
    let magic = *b"ALSEM-IR-SLOT-v1";
    let mut s = [0u8; super::pe::SLOT_LEN];
    let mut i = 0;
    while i < 16 {
        s[i] = magic[i];
        i += 1;
    }
    s
};

/// The slot as this process sees it.
pub(crate) fn slot() -> &'static [u8; super::pe::SLOT_LEN] {
    std::hint::black_box(&IR_DATA_SLOT)
}

/// The slot's magic, read from this binary's own slot (never a second literal).
pub fn slot_magic() -> [u8; 16] {
    slot()[..16].try_into().expect("16 bytes")
}

/// The IR marker's bytes, read through `black_box` (tests build files that carry it).
#[cfg(test)]
pub(crate) fn marker_bytes() -> [u8; 16] {
    *std::hint::black_box(&IR_IMAGE_MARKER)
}

/// The key of the running image, or why there is none (the cache is off for this process).
#[cfg(target_os = "linux")]
pub(crate) fn image_key() -> Result<IrKey, NoKey> {
    let mut f = std::fs::File::open("/proc/self/exe")
        .map_err(|e| NoKey::Failed(format!("/proc/self/exe: {e}")))?;
    key_of(&mut f).ok_or_else(|| NoKey::Failed("unreadable or unmarked image".into()))
}

/// The key of the running image (design B), or why there is none.
#[cfg(windows)]
pub(crate) fn image_key() -> Result<IrKey, NoKey> {
    win::image_key()
}

/// No key on other systems.
#[cfg(not(any(target_os = "linux", windows)))]
pub(crate) fn image_key() -> Result<IrKey, NoKey> {
    Err(NoKey::Unsupported)
}

/// The key of `image` read to its end: blake3 over a domain tag, the IR format and every byte;
/// `None` on a read error or when the bytes hold no [`IR_IMAGE_MARKER`].
#[cfg_attr(not(any(target_os = "linux", windows)), allow(dead_code))]
pub(crate) fn key_of(image: &mut impl Read) -> Option<IrKey> {
    let marker: [u8; 16] = *std::hint::black_box(&IR_IMAGE_MARKER);
    let finder = memchr::memmem::Finder::new(&marker);
    let mut h = blake3::Hasher::new();
    h.update(b"alsem-ir-key");
    h.update(&super::IR_FORMAT.to_le_bytes());
    let mut buf = vec![0u8; 1 << 20];
    // The last 15 bytes of the previous chunk, so a marker across a chunk boundary is found.
    let mut tail: Vec<u8> = Vec::with_capacity(2 * marker.len());
    let mut found = false;
    loop {
        let n = match image.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => return None,
        };
        let chunk = &buf[..n];
        h.update(chunk);
        if !found {
            let k = marker.len() - 1;
            tail.extend_from_slice(&chunk[..chunk.len().min(k)]);
            found = finder.find(&tail).is_some() || finder.find(chunk).is_some();
            if chunk.len() >= k {
                tail.clear();
                tail.extend_from_slice(&chunk[chunk.len() - k..]);
            } else {
                // A short read: `tail` holds the old tail and the whole chunk.
                let cut = tail.len().saturating_sub(k);
                tail.drain(..cut);
            }
        }
    }
    found.then(|| IrKey(*h.finalize().as_bytes()))
}

/// Design B on Windows (module docs). Every failure is a reason for no key, never a panic.
#[cfg(windows)]
mod win {
    use super::super::pe;
    use super::{IR_IMAGE_MARKER, IrKey, NoKey, key_of, slot};
    use crate::snapshot::winfs::local_ntfs;
    use std::ffi::{OsStr, OsString, c_void};
    use std::fs::{File, OpenOptions};
    use std::io::Read;
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
    use std::path::{Path, PathBuf};
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ,
    };
    use windows_sys::Win32::System::Diagnostics::Debug::ReadProcessMemory;
    use windows_sys::Win32::System::LibraryLoader::{
        GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
        GetModuleFileNameW, GetModuleHandleExW,
    };
    use windows_sys::Win32::System::ProcessStatus::K32GetMappedFileNameW;
    use windows_sys::Win32::System::Threading::GetCurrentProcess;

    /// The base of the module holding the IR marker (the code that writes and reads IR).
    fn module_base() -> Option<usize> {
        let mut h = std::ptr::null_mut();
        // SAFETY: FROM_ADDRESS takes an address inside a module as the name; UNCHANGED_REFCOUNT
        // takes no reference, so nothing is to be freed.
        let ok = unsafe {
            GetModuleHandleExW(
                GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS
                    | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
                std::hint::black_box(&IR_IMAGE_MARKER).as_ptr().cast(),
                &mut h,
            )
        };
        (ok != 0).then_some(h as usize)
    }

    /// The loaded module read with `ReadProcessMemory` on this process: an unreadable page is a
    /// failed read (no key), never a fault. Ranges are bounded by the image's own `SizeOfImage`
    /// in the canonicaliser, and by `pe::MAX_IMAGE` here.
    struct MemView {
        base: usize,
    }

    impl pe::View for MemView {
        fn read(&self, rva: u32, len: usize) -> Option<Vec<u8>> {
            if (rva as usize).checked_add(len)? > pe::MAX_IMAGE as usize {
                return None;
            }
            let mut out = vec![0u8; len];
            let mut got = 0usize;
            // SAFETY: the buffer holds `len` bytes; ReadProcessMemory checks the source range
            // and fails (rather than faulting) on a page that cannot be read.
            let ok = unsafe {
                ReadProcessMemory(
                    GetCurrentProcess(),
                    (self.base + rva as usize) as *const c_void,
                    out.as_mut_ptr().cast(),
                    len,
                    &mut got,
                )
            };
            (ok != 0 && got == len).then_some(out)
        }
    }

    /// The module's mapped file name (NT device form), opened through `\\?\GLOBALROOT`.
    fn mapped_path(base: usize) -> Option<PathBuf> {
        let mut buf = vec![0u16; 32768];
        // SAFETY: the buffer and its length in characters match.
        let n = unsafe {
            K32GetMappedFileNameW(
                GetCurrentProcess(),
                base as *const c_void,
                buf.as_mut_ptr(),
                buf.len() as u32,
            )
        } as usize;
        // A return of the buffer's size means truncated (GetMappedFileNameW docs).
        if n == 0 || n >= buf.len() {
            return None;
        }
        let mut p: Vec<u16> = OsStr::new(r"\\?\GLOBALROOT").encode_wide().collect();
        p.extend_from_slice(&buf[..n]);
        Some(PathBuf::from(OsString::from_wide(&p)))
    }

    /// The module's load-time file name.
    fn module_file_name(base: usize) -> Option<PathBuf> {
        let mut buf = vec![0u16; 32768];
        // SAFETY: `base` is this process's module handle; the buffer and its length match.
        let n =
            unsafe { GetModuleFileNameW(base as _, buf.as_mut_ptr(), buf.len() as u32) } as usize;
        (n != 0 && n < buf.len()).then(|| PathBuf::from(OsString::from_wide(&buf[..n])))
    }

    /// The accepted candidate, held open (sharing read only: no write, no rename, no delete)
    /// for the process's life, so nothing can replace or rename the keyed file after the key is
    /// made. The window that stays is from the image's load to the key, milliseconds: the key is
    /// made at the start of `main` (`install_from_env`).
    static HELD: std::sync::OnceLock<File> = std::sync::OnceLock::new();

    /// Part B on one candidate: the key and the handle it was read through, or why it was
    /// refused.
    fn check(
        p: &Path,
        a: &pe::Canon,
        slot_rva: u32,
        loaded: &[u8; pe::SLOT_LEN],
    ) -> Result<(IrKey, File), String> {
        let mut f: File = OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .open(p)
            .map_err(|e| format!("open: {e}"))?;
        let m = f.metadata().map_err(|e| format!("stat: {e}"))?;
        if !m.is_file() || m.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err("not a regular file".into());
        }
        if m.len() > 2 * u64::from(pe::MAX_IMAGE) {
            return Err("file too large".into());
        }
        local_ntfs(&f)?;
        let mut bytes = Vec::new();
        f.read_to_end(&mut bytes)
            .map_err(|e| format!("read: {e}"))?;
        match pe::check_candidate(&bytes, a)? {
            pe::Verdict::Accepted => {}
            pe::Verdict::HeadersDiffer => return Err("headers differ".into()),
            pe::Verdict::HashDiffers => return Err("canonical hash differs".into()),
        }
        match pe::check_slot(&bytes, slot_rva, loaded, &a.zeroed)? {
            pe::SlotVerdict::Matches => {}
            pe::SlotVerdict::Unstamped => return Err("unstamped".into()),
            pe::SlotVerdict::DataDiffers => {
                return Err("writable sections do not match the loaded digest".into());
            }
            pe::SlotVerdict::SlotDiffers => return Err("its slot differs".into()),
        }
        let k = key_of(&mut &bytes[..]).ok_or_else(|| "no IR marker in the file".to_string())?;
        Ok((k, f))
    }

    pub(super) fn image_key() -> Result<IrKey, NoKey> {
        let base =
            module_base().ok_or_else(|| NoKey::Failed("GetModuleHandleExW failed".into()))?;
        let mem = MemView { base };
        let a = pe::canonical(&mem, base as u64)
            .map_err(|e| NoKey::Failed(format!("the loaded image: {e}")))?;
        let slot_rva = u32::try_from((slot().as_ptr() as usize).wrapping_sub(base))
            .map_err(|_| NoKey::Failed("the data-digest slot is not in this module".into()))?;
        let loaded: [u8; pe::SLOT_LEN] = pe::View::read(&mem, slot_rva, pe::SLOT_LEN)
            .ok_or_else(|| NoKey::Failed("the data-digest slot cannot be read".into()))?
            .try_into()
            .expect("SLOT_LEN bytes");
        if loaded[16..].iter().all(|&b| b == 0) {
            return Err(NoKey::Unstamped);
        }
        let mut why = Vec::new();
        for (what, p) in [
            ("mapped name", mapped_path(base)),
            ("module file name", module_file_name(base)),
        ] {
            let Some(p) = p else {
                why.push(format!("{what}: not available"));
                continue;
            };
            match check(&p, &a, slot_rva, &loaded) {
                Ok((k, f)) => {
                    let _ = HELD.set(f);
                    return Ok(k);
                }
                Err(e) => why.push(format!("{what} {}: {e}", p.display())),
            }
        }
        Err(NoKey::Failed(format!(
            "no candidate file matches the running image ({})",
            why.join("; ")
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn marked(extra: &[u8]) -> Vec<u8> {
        [b"prefix".as_slice(), &IR_IMAGE_MARKER, extra].concat()
    }

    /// TLS-117 (the probe's W12 lesson): this binary holds the slot magic exactly once, so a
    /// stamp of it can find its slot; fixtures carry a test-only magic. Discrimination
    /// (2026-11-23): a second spelling of the magic in this test (`black_box(*b"...")`): the
    /// count is 2 and the test fails; removed, passes.
    #[test]
    fn the_slot_magic_occurs_once_in_this_binary() {
        let f = std::fs::read(std::env::current_exe().unwrap()).unwrap();
        let m = slot_magic();
        assert_eq!(f.windows(16).filter(|w| *w == m).count(), 1);
    }

    #[test]
    fn bytes_without_marker_have_no_key() {
        assert!(key_of(&mut &b"no marker in here at all"[..]).is_none());
        let mut almost = IR_IMAGE_MARKER;
        almost[15] ^= 1;
        assert!(key_of(&mut &almost[..]).is_none());
        assert!(key_of(&mut &marked(b"")[..]).is_some());
    }

    #[test]
    fn different_bytes_different_keys() {
        let (a, b) = (marked(b"x"), marked(b"y"));
        let (ka, kb) = (key_of(&mut &a[..]), key_of(&mut &b[..]));
        assert!(ka.is_some() && kb.is_some());
        assert_ne!(ka, kb);
        assert_eq!(ka, key_of(&mut &a[..]), "deterministic");
    }

    /// A marker split across two reads is still found.
    #[test]
    fn marker_across_a_read_boundary_is_found() {
        struct Chunks<'a>(Vec<&'a [u8]>);
        impl Read for Chunks<'_> {
            fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
                if self.0.is_empty() {
                    return Ok(0);
                }
                let c = self.0.remove(0);
                out[..c.len()].copy_from_slice(c);
                Ok(c.len())
            }
        }
        let bytes = marked(b"tail");
        for split in 1..bytes.len() {
            let mut r = Chunks(vec![&bytes[..split], &bytes[split..]]);
            assert_eq!(key_of(&mut r), key_of(&mut &bytes[..]), "split at {split}");
        }
        let mut bytewise = Chunks(bytes.chunks(1).collect());
        assert_eq!(
            key_of(&mut bytewise),
            key_of(&mut &bytes[..]),
            "one byte a read"
        );
        assert!(key_of(&mut &bytes[..]).is_some());
    }

    /// The child half of [`key_is_the_running_image_not_the_path`]: waits for a line on
    /// stdin, then prints the running image's key.
    #[test]
    #[ignore = "run by key_is_the_running_image_not_the_path as a child"]
    fn key_child_mode() {
        if std::env::var_os("ALSEM_IR_KEY_CHILD").is_none() {
            return;
        }
        let mut line = String::new();
        std::io::stdin().read_line(&mut line).unwrap();
        println!(
            "IRKEY={}",
            image_key().map_or_else(|e| format!("none: {e}"), |k| k.hex())
        );
        // Windows tests act on the file while this process still runs (and holds it).
        if std::env::var_os("ALSEM_IR_KEY_HOLD").is_some() {
            use std::io::Write;
            std::io::stdout().flush().unwrap();
            line.clear();
            std::io::stdin().read_line(&mut line).unwrap();
        }
    }

    /// An engine whose executable path is replaced while it runs (a rebuild) still keys on
    /// the image it runs, not on the file now at its path.
    #[cfg(target_os = "linux")]
    #[test]
    fn key_is_the_running_image_not_the_path() {
        use std::io::Write;
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("engine");
        std::fs::copy(std::env::current_exe().unwrap(), &exe).unwrap();
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o700)).unwrap();
        let original = key_of(&mut std::fs::File::open(&exe).unwrap()).expect("marked");
        let mut child = std::process::Command::new(&exe)
            .args([
                "--ignored",
                "--exact",
                "program::ir_cache::key::tests::key_child_mode",
                "--nocapture",
                "--test-threads=1",
            ])
            .env("ALSEM_IR_KEY_CHILD", "1")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        // A rebuild: the path now names another file (marked, so it would have a key).
        std::fs::remove_file(&exe).unwrap();
        std::fs::write(&exe, marked(b"another build")).unwrap();
        writeln!(child.stdin.take().unwrap()).unwrap();
        let out = child.wait_with_output().unwrap();
        assert!(out.status.success());
        let key = String::from_utf8_lossy(&out.stdout)
            .lines()
            // libtest prints the test's name on the same line first.
            .find_map(|l| l.split("IRKEY=").nth(1).map(|k| k.trim().to_string()))
            .expect("the child printed its key");
        assert_eq!(key, original.hex());
    }

    /// Run a copy of this test binary as [`key_child_mode`]: `before` runs before it makes its
    /// key, `after` after it printed the key while it still runs (it holds the accepted file).
    /// Returns what it printed after `IRKEY=` (the key's hex, or `none: <reason>`).
    #[cfg(windows)]
    fn child_key(exe: &std::path::Path, before: impl FnOnce(), after: impl FnOnce()) -> String {
        use std::io::{BufRead, Write};
        let mut child = std::process::Command::new(exe)
            .args([
                "--ignored",
                "--exact",
                "program::ir_cache::key::tests::key_child_mode",
                "--nocapture",
                "--test-threads=1",
            ])
            .env("ALSEM_IR_KEY_CHILD", "1")
            .env("ALSEM_IR_KEY_HOLD", "1")
            .current_dir(std::env::temp_dir())
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let mut stdin = child.stdin.take().unwrap();
        let mut out = std::io::BufReader::new(child.stdout.take().unwrap());
        before();
        writeln!(stdin).unwrap();
        let mut key = None;
        let mut line = String::new();
        while key.is_none() && out.read_line(&mut line).unwrap() > 0 {
            key = line.split("IRKEY=").nth(1).map(|k| k.trim().to_string());
            line.clear();
        }
        after();
        writeln!(stdin).unwrap();
        assert!(child.wait().unwrap().success());
        key.expect("the child printed its key")
    }

    /// A `.text` byte of `file` changed (the middle of the section): a rival build with the
    /// same headers and a valid slot, which design B must refuse at the canonical comparison.
    #[cfg(windows)]
    fn rival(file: &[u8]) -> Vec<u8> {
        let h = super::super::pe::Headers::parse(file).unwrap();
        let s = h.sections.iter().find(|s| s.named(".text")).unwrap();
        let mut f = file.to_vec();
        f[s.raw_ptr as usize + (s.raw_size.min(s.mapped_len()) / 2) as usize] ^= 1;
        f
    }

    /// TLS-117 (Windows; the owner's acceptance run): an unstamped copy of this test binary has
    /// no key; a stamped copy keys on its own bytes and holds the file (a rename or delete of
    /// it fails while it runs); and when its folder is renamed away and a rival build put at
    /// its old spelling before it makes its key (the mapped name keeps the old path, as the
    /// owner's probe run showed), both candidates are refused at the canonical comparison.
    #[cfg(windows)]
    #[test]
    fn windows_stamped_copy_keys_on_its_image() {
        let top = tempfile::tempdir().unwrap();
        let dir = top.path().join("dir");
        std::fs::create_dir(&dir).unwrap();
        let exe = dir.join("engine.exe");
        std::fs::copy(std::env::current_exe().unwrap(), &exe).unwrap();
        let k = child_key(&exe, || {}, || {});
        assert!(k.starts_with("none") && k.contains("unstamped"), "{k}");
        super::super::stamp::stamp_file(&exe, &exe).unwrap();
        let original = key_of(&mut std::fs::File::open(&exe).unwrap()).expect("marked");
        let held = std::cell::Cell::new(None);
        let k = child_key(
            &exe,
            || {},
            || {
                held.set(Some((
                    std::fs::rename(&exe, dir.join("moved.exe")).map_err(|e| e.raw_os_error()),
                    std::fs::remove_file(&exe).map_err(|e| e.raw_os_error()),
                )));
            },
        );
        assert_eq!(k, original.hex(), "a stamped copy keys");
        assert_eq!(
            held.take(),
            Some((Err(Some(32)), Err(Some(32)))),
            "rename and delete refused with a sharing violation while the key's handle is held"
        );
        let stamped = std::fs::read(&exe).unwrap();
        let k = child_key(
            &exe,
            || {
                std::fs::rename(&dir, top.path().join("dir-away")).unwrap();
                std::fs::create_dir(&dir).unwrap();
                std::fs::write(&exe, rival(&stamped)).unwrap();
            },
            || {},
        );
        assert!(k.starts_with("none"), "never a key here: {k}");
        assert_eq!(
            k.matches("canonical hash differs").count(),
            2,
            "both candidates refused at the canonical comparison: {k}"
        );
    }
}

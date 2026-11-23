//! TLS-117's native probe (design note
//! `docs/superpowers/notes/2026-11-21-tls117-windows-image-key.md`, section "The probe").
//!
//! Settles, on a real Windows machine, the facts the IR cache's Windows key and private-folder
//! check rest on. It prints one `PASS`, `FAIL` or `SKIP` line per item and exits 1 when any item
//! fails. Two ways to run it (both on Windows, from the repository root):
//!
//! ```text
//! cargo run --example tls117_probe
//! cargo test --example tls117_probe -- --test-threads=1 --nocapture
//! ```
//!
//! The key sequence here is a prototype of the product code (design B), not the product code: the
//! key is the whole file's blake3, without the IR marker search. The PE canonicaliser (`pe`,
//! `examples/tls117/pe.rs`) has unit tests that run on any system:
//! `cargo test --example tls117_probe pe::`.
//!
//! W10 (`examples/tls117_w10.ps1`) builds the probe several times and compares the outputs with
//! `tls117_probe w10-compare <label> <a.exe> <b.exe>`, a mode that works on any system.

#[cfg_attr(not(windows), allow(dead_code))]
#[path = "tls117/pe.rs"]
mod pe;

/// W10: a writable static (it lands in `.data`) whose initial value is the only difference of the
/// `w10-variant` build. `main` reads it, so it is kept.
static W10_VALUE: std::sync::atomic::AtomicU32 =
    std::sync::atomic::AtomicU32::new(if cfg!(feature = "w10-variant") {
        0x5157_0002
    } else {
        0x5157_0001
    });

/// `w10-compare <label> <a> <b>`: canonicalise two builds and judge them (design note section 8,
/// W10). Label `variant`: two builds differing only in [`W10_VALUE`]'s initial value must
/// canonicalise differently. Label `relink`: two links of the same source must canonicalise
/// equal with equal writable sections, or differently; never equal with different writable
/// sections. Prints the TimeDateStamp, the REPRO (/Brepro) debug entry and the PDB GUID of each.
fn w10_compare(label: &str, a: &str, b: &str) -> i32 {
    let run = || -> Result<(bool, String), String> {
        let (fa, fb) = (
            std::fs::read(a).map_err(|e| format!("{a}: {e}"))?,
            std::fs::read(b).map_err(|e| format!("{b}: {e}"))?,
        );
        let (ca, cb) = (pe::canonical_file(&fa)?, pe::canonical_file(&fb)?);
        let (da, db) = (pe::writable_bytes(&fa)?, pe::writable_bytes(&fb)?);
        let (ia, ib) = (pe::debug_info(&fa)?, pe::debug_info(&fb)?);
        let same_canon = ca.hash == cb.hash;
        let same_data = da == db;
        // Where W10_VALUE's initial value sits in each file: it must be in a writable section,
        // or the variant test would only show a constant change.
        let place = |f: &[u8], v: u32| -> String {
            let pat = v.to_le_bytes();
            let Some(off) = f.windows(4).position(|w| w == pat) else {
                return "not found".into();
            };
            let h = match pe::Headers::parse(f) {
                Ok(h) => h,
                Err(e) => return e,
            };
            h.sections
                .iter()
                .find(|s| off >= s.raw_ptr as usize && off < (s.raw_ptr + s.raw_size) as usize)
                .map_or("outside every section".into(), |s| {
                    format!(
                        "{} ({})",
                        String::from_utf8_lossy(&s.name).trim_end_matches('\0'),
                        if s.writable() {
                            "writable"
                        } else {
                            "READ-ONLY"
                        }
                    )
                })
        };
        let vb = if label == "variant" {
            0x5157_0002
        } else {
            0x5157_0001
        };
        let (pa, pb) = (place(&fa, 0x5157_0001), place(&fb, vb));
        if label == "variant" && !(pa.ends_with("(writable)") && pb.ends_with("(writable)")) {
            return Ok((
                false,
                format!("W10_VALUE is not in a writable section in both builds: a {pa}, b {pb}"),
            ));
        }
        let pass = match label {
            "variant" => !same_canon,
            "relink" => !same_canon || same_data,
            other => return Err(format!("unknown label {other}")),
        };
        Ok((
            pass,
            format!(
                "canonical {} | writable sections {} | files {} | W10_VALUE a {pa}, b {pb} | a: \
                 TimeDateStamp {:#010x}, REPRO entry {}, PDB {:?} | b: TimeDateStamp {:#010x}, \
                 REPRO entry {}, PDB {:?}",
                if same_canon { "EQUAL" } else { "different" },
                if same_data { "equal" } else { "DIFFERENT" },
                if fa == fb { "identical" } else { "differ" },
                ia.time_date_stamp,
                ia.repro,
                ia.pdb_guid,
                ib.time_date_stamp,
                ib.repro,
                ib.pdb_guid,
            ),
        ))
    };
    match run() {
        Ok((true, n)) => {
            println!("PASS W10 {label}: {n}");
            0
        }
        Ok((false, n)) => {
            println!("FAIL W10 {label}: {n}");
            1
        }
        Err(e) => {
            println!("FAIL W10 {label}: {e}");
            1
        }
    }
}

fn main() {
    // A store the optimiser cannot prove harmless, so the static stays writable (in `.data`, not
    // folded into a constant in `.rdata`); `w10-compare variant` checks where it landed.
    W10_VALUE.fetch_add(
        std::hint::black_box(0),
        std::sync::atomic::Ordering::Relaxed,
    );
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("w10-compare") && args.len() == 5 {
        std::process::exit(w10_compare(&args[2], &args[3], &args[4]));
    }
    #[cfg(windows)]
    std::process::exit(win::main());
    #[cfg(not(windows))]
    {
        eprintln!("tls117_probe: Windows only (except `w10-compare`)");
        std::process::exit(2);
    }
}

#[cfg(windows)]
mod win {
    use super::pe;
    use std::ffi::{OsStr, OsString, c_void};
    use std::fs::{File, OpenOptions, TryLockError};
    use std::io::{BufRead, BufReader, ErrorKind, Read, Write};
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
    use std::os::windows::io::AsRawHandle;
    use std::path::{Component, Path, PathBuf, Prefix};
    use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
    use std::time::{Duration, Instant};
    use windows_sys::Win32::Foundation::{
        CloseHandle, GENERIC_ALL, GENERIC_WRITE, HANDLE, LocalFree,
    };
    use windows_sys::Win32::Security::Authorization::{
        ConvertSidToStringSidW, GetSecurityInfo, SE_FILE_OBJECT,
    };
    use windows_sys::Win32::Security::{
        ACCESS_ALLOWED_ACE, ACE_HEADER, ACL, DACL_SECURITY_INFORMATION, GetAce,
        GetTokenInformation, INHERIT_ONLY_ACE, OWNER_SECURITY_INFORMATION, PSID, TOKEN_QUERY,
        TOKEN_USER, TokenUser,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        CreateHardLinkW, DELETE, FILE_ADD_FILE, FILE_ADD_SUBDIRECTORY,
        FILE_ATTRIBUTE_REPARSE_POINT, FILE_DELETE_CHILD, FILE_FLAG_BACKUP_SEMANTICS,
        FILE_FLAG_OPEN_REPARSE_POINT, FILE_ID_128, FILE_ID_INFO, FILE_READ_ATTRIBUTES,
        FILE_RENAME_INFO, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, FILE_WRITE_DATA,
        FileIdInfo, FileRenameInfoEx, GetDriveTypeW, GetFileInformationByHandleEx,
        GetFinalPathNameByHandleW, GetShortPathNameW, GetVolumeInformationByHandleW,
        MOVEFILE_REPLACE_EXISTING, MoveFileExW, READ_CONTROL, SetFileInformationByHandle,
        VOLUME_NAME_GUID, WRITE_DAC, WRITE_OWNER,
    };
    use windows_sys::Win32::System::Diagnostics::Debug::ReadProcessMemory;
    use windows_sys::Win32::System::LibraryLoader::{
        GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
        GetModuleFileNameW, GetModuleHandleExW,
    };
    use windows_sys::Win32::System::ProcessStatus::K32GetMappedFileNameW;
    use windows_sys::Win32::System::SystemServices::{
        ACCESS_ALLOWED_ACE_TYPE, ACCESS_DENIED_ACE_TYPE,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    // In `Win32_System_WindowsProgramming`, a feature not otherwise needed.
    const DRIVE_FIXED: u32 = 3;
    const FILE_RENAME_FLAG_REPLACE_IF_EXISTS: u32 = 1;
    const FILE_RENAME_FLAG_POSIX_SEMANTICS: u32 = 2;
    const ERROR_ACCESS_DENIED: i32 = 5;
    const ERROR_SHARING_VIOLATION: i32 = 32;

    /// An address inside this image (as `IR_IMAGE_MARKER` is in the product).
    #[used]
    static ANCHOR: [u8; 16] = *b"TLS117-PROBE-IMG";

    // ---------------------------------------------------------------------------------
    // Small helpers
    // ---------------------------------------------------------------------------------

    fn wide(p: &Path) -> Vec<u16> {
        p.as_os_str().encode_wide().chain([0]).collect()
    }

    fn hex(b: &[u8]) -> String {
        b.iter().map(|x| format!("{x:02x}")).collect()
    }

    fn hex16(w: &[u16]) -> String {
        w.iter().map(|x| format!("{x:04x}")).collect()
    }

    fn lossy(w: &[u16]) -> String {
        String::from_utf16_lossy(w)
    }

    fn blake3_of(r: &mut impl Read) -> std::io::Result<[u8; 32]> {
        let mut h = blake3::Hasher::new();
        let mut buf = vec![0u8; 1 << 20];
        loop {
            let n = r.read(&mut buf)?;
            if n == 0 {
                return Ok(*h.finalize().as_bytes());
            }
            h.update(&buf[..n]);
        }
    }

    /// Volume serial number and 128-bit file id of an open handle (as TLS-109's `handle_id`).
    fn file_id(f: &File) -> std::io::Result<(u64, u128)> {
        let mut info = FILE_ID_INFO {
            VolumeSerialNumber: 0,
            FileId: FILE_ID_128 {
                Identifier: [0; 16],
            },
        };
        // SAFETY: `info` is the FILE_ID_INFO the call fills; the handle is live.
        let ok = unsafe {
            GetFileInformationByHandleEx(
                f.as_raw_handle(),
                FileIdInfo,
                (&raw mut info).cast(),
                std::mem::size_of::<FILE_ID_INFO>() as u32,
            )
        };
        if ok == 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok((
            info.VolumeSerialNumber,
            u128::from_le_bytes(info.FileId.Identifier),
        ))
    }

    /// The file id of whatever `p` names now (attributes access, sharing everything, so an
    /// open image or a held file does not refuse it).
    fn file_id_at(p: &Path) -> std::io::Result<(u64, u128)> {
        let f = OpenOptions::new()
            .access_mode(FILE_READ_ATTRIBUTES)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(p)?;
        file_id(&f)
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
    fn local_ntfs(f: &File) -> Result<(), String> {
        match (fs_name(f).as_deref(), drive_type(f)) {
            (Some("NTFS"), Some(DRIVE_FIXED)) => Ok(()),
            (fs, dt) => Err(format!(
                "not a local NTFS volume (file system {fs:?}, drive type {dt:?})"
            )),
        }
    }

    // ---------------------------------------------------------------------------------
    // The key sequence (design note section 3, steps 1-6)
    // ---------------------------------------------------------------------------------

    /// Step 1: the base of the module holding [`ANCHOR`].
    fn module_base() -> Option<*const c_void> {
        let mut h = std::ptr::null_mut();
        // SAFETY: FROM_ADDRESS takes any address inside a module as the name; UNCHANGED_REFCOUNT
        // takes no reference, so nothing is to be freed.
        let ok = unsafe {
            GetModuleHandleExW(
                GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS
                    | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
                std::hint::black_box(&ANCHOR).as_ptr().cast(),
                &mut h,
            )
        };
        (ok != 0).then_some(h as *const c_void)
    }

    /// Steps 2 and 6: the mapped file name of the module at `base`, raw UTF-16 (no folding),
    /// in NT device form. `None` on failure or truncation.
    fn mapped_name(base: *const c_void) -> Option<Vec<u16>> {
        let mut buf = vec![0u16; 32768];
        // SAFETY: the buffer and its length in characters match.
        let n = unsafe {
            K32GetMappedFileNameW(
                GetCurrentProcess(),
                base,
                buf.as_mut_ptr(),
                buf.len() as u32,
            )
        } as usize;
        // A return of the buffer's size means truncated (GetMappedFileNameW docs).
        (n != 0 && n < buf.len()).then(|| buf[..n].to_vec())
    }

    /// Open a path for the key: read access, sharing read only (no write, no delete), not
    /// following a reparse point.
    fn open_candidate(p: &Path) -> std::io::Result<File> {
        OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .open(p)
    }

    /// The loaded module as a [`pe::View`], read with `ReadProcessMemory` on this process, so an
    /// unreadable page is a failed read (no key), never a fault. Every range the canonicaliser
    /// asks for is bounded by the image's own `SizeOfImage`; this caps it at `pe::MAX_IMAGE` too.
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

    /// Part A: the canonical hash of the module in memory.
    fn memory_canon(base: *const c_void) -> Result<pe::Canon, String> {
        pe::canonical(
            &MemView {
                base: base as usize,
            },
            base as u64,
        )
    }

    /// Part B on one candidate path: open it denying write and delete sharing, require a
    /// regular file on local NTFS, read it whole through that handle (bounded), and check it
    /// against A (headers first, then the canonical hash). `Ok((Some(key), verdict))` when
    /// accepted; the key is the blake3 of the bytes read through that handle.
    fn check_path(
        p: &Path,
        a: &pe::Canon,
        step: &mut dyn FnMut(&str),
    ) -> Result<(Option<[u8; 32]>, &'static str), String> {
        let f = open_candidate(p);
        step("after-open");
        let mut f = f.map_err(|e| format!("open: {e}"))?;
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
        let v = pe::check_candidate(&bytes, a);
        step("after-hash");
        Ok(match v.map_err(|e| format!("part B: {e}"))? {
            pe::Verdict::Accepted => (Some(*blake3::hash(&bytes).as_bytes()), "accepted"),
            pe::Verdict::HeadersDiffer => (None, "refused (headers differ)"),
            pe::Verdict::HashDiffers => (None, "refused (canonical hash differs)"),
        })
    }

    /// The module's load-time file name (`GetModuleFileNameW`).
    fn module_file_name(base: *const c_void) -> Option<PathBuf> {
        let mut buf = vec![0u16; 32768];
        // SAFETY: `base` is this process's module handle; the buffer and its length match.
        let n =
            unsafe { GetModuleFileNameW(base as _, buf.as_mut_ptr(), buf.len() as u32) } as usize;
        (n != 0 && n < buf.len()).then(|| PathBuf::from(OsString::from_wide(&buf[..n])))
    }

    /// Design B (design note section 3). `barrier(tag)` runs once at each of "after-query",
    /// "after-open" and "after-hash" (around the first candidate). Returns the key (or the reason
    /// for none) and a report: A's hash and each candidate's verdict.
    fn image_key(barrier: &mut dyn FnMut(&str)) -> (Result<[u8; 32], String>, String) {
        let mut report = Vec::new();
        let r = (|| {
            let base = module_base().ok_or("GetModuleHandleExW failed")?;
            let a = memory_canon(base).map_err(|e| format!("part A: {e}"))?;
            report.push(format!(
                "A {} ({} sections, {} reloc sites)",
                &hex(&a.hash)[..16],
                a.sections,
                a.reloc_sites
            ));
            let mut cands: Vec<(&str, PathBuf)> = Vec::new();
            if let Some(n) = mapped_name(base) {
                let mut p: Vec<u16> = OsStr::new(r"\\?\GLOBALROOT").encode_wide().collect();
                p.extend_from_slice(&n);
                cands.push(("mapped", PathBuf::from(OsString::from_wide(&p))));
            }
            if let Some(p) = module_file_name(base) {
                cands.push(("module-name", p));
            }
            barrier("after-query");
            let (mut opened, mut hashed) = (false, false);
            let mut key = None;
            for (what, p) in &cands {
                let mut step = |tag: &str| {
                    let seen = if tag == "after-open" {
                        &mut opened
                    } else {
                        &mut hashed
                    };
                    if !*seen {
                        *seen = true;
                        barrier(tag);
                    }
                };
                match check_path(p, &a, &mut step) {
                    Ok((Some(k), v)) => {
                        report.push(format!("{what}: {v}"));
                        key = Some(k);
                        break;
                    }
                    Ok((None, v)) => report.push(format!("{what}: {v}")),
                    Err(e) => report.push(format!("{what}: refused ({e})")),
                }
            }
            // The parent waits for every barrier: give the ones no candidate reached.
            if !opened {
                barrier("after-open");
            }
            if !hashed {
                barrier("after-hash");
            }
            key.ok_or_else(|| "no candidate file matches the running image".to_string())
        })();
        (r, report.join("; "))
    }

    // ---------------------------------------------------------------------------------
    // Child mode: a copy of this executable, driven over stdin/stdout
    // ---------------------------------------------------------------------------------

    /// Every child reply starts with this, so the parent finds it after libtest's own text.
    const TAG: &str = "@@";

    fn reply(s: &str) {
        let mut o = std::io::stdout().lock();
        let _ = writeln!(o, "{TAG}{s}");
        let _ = o.flush();
    }

    fn read_line() -> String {
        let mut l = String::new();
        let _ = std::io::stdin().read_line(&mut l);
        l.trim().to_string()
    }

    /// `KEY <hex> | <report>` or `NOKEY <reason> | <report>`.
    fn key_line((r, report): (Result<[u8; 32], String>, String)) -> String {
        match r {
            Ok(k) => format!("KEY {} | {report}", hex(&k)),
            Err(e) => format!("NOKEY {e} | {report}"),
        }
    }

    /// Commands: `name`, `key`, `key-barriers`, `lock <path>`, `exit`.
    pub(crate) fn child_loop() {
        reply("READY");
        loop {
            let cmd = read_line();
            match cmd.split_once(' ').unwrap_or((cmd.as_str(), "")) {
                ("name", _) => match module_base().and_then(mapped_name) {
                    Some(n) => reply(&format!("NAME {}", hex16(&n))),
                    None => reply("NAME -"),
                },
                ("key", _) => reply(&key_line(image_key(&mut |_| {}))),
                ("key-barriers", _) => {
                    let r = image_key(&mut |tag| {
                        reply(&format!("BARRIER {tag}"));
                        let _ = read_line();
                    });
                    reply(&key_line(r));
                }
                ("lock", p) => {
                    let f = OpenOptions::new().read(true).write(true).open(p);
                    match f.map(|f| (f.try_lock(), f)) {
                        Ok((Ok(()), f)) => {
                            reply("LOCKED");
                            let _ = read_line(); // killed while waiting here
                            drop(f);
                        }
                        other => reply(&format!("LOCKFAIL {:?}", other.map(|x| x.0))),
                    }
                }
                ("exit", _) | ("", _) => return,
                _ => reply("UNKNOWN"),
            }
        }
    }

    struct Kid {
        proc: Child,
        stdin: ChildStdin,
        out: BufReader<ChildStdout>,
    }

    impl Kid {
        fn spawn(exe: &Path) -> Result<Kid, String> {
            let mut cmd = Command::new(exe);
            if cfg!(test) {
                cmd.args([
                    "--ignored",
                    "--exact",
                    "win::tests::child_mode",
                    "--nocapture",
                    "--test-threads=1",
                ])
                .env("TLS117_CHILD", "1");
            } else {
                cmd.arg("child");
            }
            let mut proc = cmd
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .spawn()
                .map_err(|e| format!("spawn {}: {e}", exe.display()))?;
            let stdin = proc.stdin.take().unwrap();
            let out = BufReader::new(proc.stdout.take().unwrap());
            let mut k = Kid { proc, stdin, out };
            match k.recv().as_str() {
                "READY" => Ok(k),
                r => Err(format!("child said {r:?}")),
            }
        }

        fn send(&mut self, s: &str) {
            let _ = writeln!(self.stdin, "{s}");
            let _ = self.stdin.flush();
        }

        /// The next reply (the text after [`TAG`]); "" at end of output.
        fn recv(&mut self) -> String {
            let mut l = String::new();
            loop {
                l.clear();
                if self.out.read_line(&mut l).unwrap_or(0) == 0 {
                    return String::new();
                }
                if let Some(i) = l.find(TAG) {
                    return l[i + TAG.len()..].trim().to_string();
                }
            }
        }

        fn ask(&mut self, s: &str) -> String {
            self.send(s);
            self.recv()
        }

        fn name(&mut self) -> Option<Vec<u16>> {
            let r = self.ask("name");
            let h = r.strip_prefix("NAME ")?;
            (h != "-").then(|| {
                (0..h.len() / 4)
                    .map(|i| u16::from_str_radix(&h[4 * i..4 * i + 4], 16).unwrap())
                    .collect()
            })
        }
    }

    impl Drop for Kid {
        fn drop(&mut self) {
            let _ = self.proc.kill();
            let _ = self.proc.wait();
        }
    }

    // ---------------------------------------------------------------------------------
    // Outcomes and shared setup
    // ---------------------------------------------------------------------------------

    pub(crate) enum Outcome {
        Pass(String),
        Fail(String),
        Skip(String),
    }
    use Outcome::{Fail, Pass, Skip};

    /// `?` for setup steps: any error fails the item with its context.
    trait Ctx<T> {
        fn ctx(self, what: &str) -> Result<T, String>;
    }
    impl<T, E: std::fmt::Display> Ctx<T> for Result<T, E> {
        fn ctx(self, what: &str) -> Result<T, String> {
            self.map_err(|e| format!("{what}: {e}"))
        }
    }

    fn outcome(r: Result<String, String>) -> Outcome {
        match r {
            Ok(n) => Pass(n),
            Err(n) => Fail(n),
        }
    }

    fn scratch() -> Result<tempfile::TempDir, String> {
        tempfile::Builder::new()
            .prefix("tls117-")
            .tempdir()
            .ctx("scratch folder")
    }

    /// A copy of this executable at `exe`, its hash and its file id.
    fn copy_self(exe: &Path) -> Result<([u8; 32], (u64, u128)), String> {
        std::fs::copy(std::env::current_exe().ctx("current_exe")?, exe).ctx("copy self")?;
        let h = blake3_of(&mut File::open(exe).ctx("open copy")?).ctx("hash copy")?;
        Ok((h, file_id_at(exe).ctx("id of copy")?))
    }

    /// A competing valid build: this executable with one `.text` byte changed (the middle of the
    /// section; executable sections are never masked). It has the same headers, so part B
    /// reaches the canonical comparison and must refuse it there.
    fn rival_pe() -> Result<Vec<u8>, String> {
        let mut f = std::fs::read(std::env::current_exe().ctx("current_exe")?).ctx("read self")?;
        let h = pe::Headers::parse(&f)?;
        let s = h
            .sections
            .iter()
            .find(|s| s.named(".text"))
            .ok_or("no .text section")?;
        let at = s.raw_ptr as usize + (s.raw_size.min(s.mapped_len()) / 2) as usize;
        *f.get_mut(at).ok_or(".text past the file")? ^= 1;
        Ok(f)
    }

    /// Both candidates reached the canonical comparison and were refused there.
    fn both_refused_by_hash(reply: &str) -> bool {
        ["mapped", "module-name"]
            .iter()
            .all(|c| reply.contains(&format!("{c}: refused (canonical hash differs)")))
    }

    fn ends_with(name: &[u16], leaf: &str) -> bool {
        let tail: Vec<u16> = OsStr::new(&format!("\\{leaf}")).encode_wide().collect();
        name.ends_with(&tail)
    }

    /// A key reply: `Ok(true)` the running image's key, `Ok(false)` no key, `Err` anything
    /// else (a WRONG key is the failure the design must never allow).
    fn judge_key(r: &str, orig: &[u8; 32]) -> Result<bool, String> {
        judge(r, orig).map(|(k, _)| k)
    }

    /// [`judge_key`] with a printable summary: whether the key equals the running image's true
    /// key (`orig`, the copy's file hash), and the child's report (A's hash, each candidate's
    /// verdict).
    fn judge(r: &str, orig: &[u8; 32]) -> Result<(bool, String), String> {
        let (head, report) = r.split_once(" | ").unwrap_or((r, ""));
        if let Some(k) = head.strip_prefix("KEY ") {
            if k == hex(orig) {
                Ok((true, format!("key = the running image's key [{report}]")))
            } else {
                Err(format!("WRONG KEY {k} [{report}]"))
            }
        } else if let Some(why) = head.strip_prefix("NOKEY ") {
            Ok((false, format!("no key ({why}) [{report}]")))
        } else {
            Err(format!("unexpected reply {r:?}"))
        }
    }

    // ---------------------------------------------------------------------------------
    // W0: locality of the image and of the real cache roots
    // ---------------------------------------------------------------------------------

    /// ENG-113's text root (`dirs::cache_dir()/al-ch-snapshot-cache`), the IR root (`ir` in it,
    /// or `ALSEM_IR_CACHE_DIR`), and a probe override when no override is set.
    fn cache_roots() -> Result<Vec<(String, PathBuf)>, String> {
        let text = dirs::cache_dir()
            .ok_or("no per-user cache folder")?
            .join("al-ch-snapshot-cache");
        let mut v = vec![
            ("text root".to_string(), text.clone()),
            ("IR root".to_string(), text.join("ir")),
        ];
        match std::env::var_os("ALSEM_IR_CACHE_DIR").filter(|v| !v.is_empty()) {
            Some(o) => v.push(("ALSEM_IR_CACHE_DIR".to_string(), PathBuf::from(o))),
            None => v.push((
                "probe override".to_string(),
                dirs::cache_dir()
                    .ok_or("no per-user cache folder")?
                    .join(format!("tls117-override-{}", std::process::id()))
                    .join("ir"),
            )),
        }
        Ok(v)
    }

    pub(crate) fn w0_local() -> Outcome {
        outcome((|| {
            let img = File::open(std::env::current_exe().ctx("current_exe")?).ctx("open image")?;
            local_ntfs(&img).map_err(|e| format!("image: {e}"))?;
            let mut notes = vec!["image on local NTFS".to_string()];
            for (what, p) in cache_roots()? {
                let existing = p
                    .ancestors()
                    .find(|a| a.exists())
                    .ok_or(format!("{what}: no existing ancestor"))?;
                let f = OpenOptions::new()
                    .access_mode(FILE_READ_ATTRIBUTES)
                    .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
                    .open(existing)
                    .ctx(what.as_str())?;
                local_ntfs(&f).map_err(|e| format!("{what} ({}): {e}", existing.display()))?;
                notes.push(format!("{what} {} on local NTFS", p.display()));
            }
            Ok(notes.join(" | "))
        })())
    }

    // ---------------------------------------------------------------------------------
    // W1: the mapped name, and replacement inside the real sequence
    // ---------------------------------------------------------------------------------

    pub(crate) fn w1_name_follows_rename() -> Outcome {
        outcome((|| {
            let d = scratch()?;
            let exe = d.path().join("a.exe");
            copy_self(&exe)?;
            let mut k = Kid::spawn(&exe)?;
            let n1 = k.name().ok_or("no mapped name")?;
            std::fs::rename(&exe, d.path().join("b.exe"))
                .ctx("refused mutation: rename of the running image")?;
            let n2 = k.name().ok_or("no mapped name after the rename")?;
            let msg = format!("{} -> {}", lossy(&n1), lossy(&n2));
            if ends_with(&n1, "a.exe") && ends_with(&n2, "b.exe") {
                Ok(msg)
            } else {
                Err(format!("stale or odd name: {msg}"))
            }
        })())
    }

    /// What the parent does at the barrier. The image is `<scratch>/dir/a.exe`.
    #[derive(Clone, Copy, Debug)]
    pub(crate) enum Mutation {
        /// Rename the image away, then write another file at its name.
        AwayThenWrite,
        /// `MoveFileExW(other, a.exe, MOVEFILE_REPLACE_EXISTING)`.
        MoveFileExOver,
        /// `SetFileInformationByHandle(FileRenameInfoEx)`, replace-if-exists + POSIX semantics.
        PosixRenameOver,
        /// Rename the image's folder away (not back) and make a new folder with another
        /// `a.exe` at the old spelling.
        AncestorAwayThenRecreate,
    }

    fn posix_rename_over(src: &Path, leaf: &str) -> std::io::Result<()> {
        let f = OpenOptions::new()
            .access_mode(DELETE)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .open(src)?;
        let name: Vec<u16> = OsStr::new(leaf).encode_wide().collect();
        let head = std::mem::offset_of!(FILE_RENAME_INFO, FileName);
        let size = head + name.len() * 2 + 2;
        // u64 storage: FILE_RENAME_INFO holds a HANDLE, so it needs pointer alignment.
        let mut buf = vec![0u64; size.div_ceil(8)];
        let info = buf.as_mut_ptr().cast::<FILE_RENAME_INFO>();
        // SAFETY: `buf` is zeroed, aligned and large enough for the header and the name.
        unsafe {
            (*info).Anonymous.Flags =
                FILE_RENAME_FLAG_REPLACE_IF_EXISTS | FILE_RENAME_FLAG_POSIX_SEMANTICS;
            (*info).RootDirectory = std::ptr::null_mut();
            (*info).FileNameLength = (name.len() * 2) as u32;
            std::ptr::copy_nonoverlapping(
                name.as_ptr(),
                (&raw mut (*info).FileName).cast::<u16>(),
                name.len(),
            );
        }
        // SAFETY: a live handle with DELETE access and the buffer built above.
        let ok = unsafe {
            SetFileInformationByHandle(
                f.as_raw_handle(),
                FileRenameInfoEx,
                info.cast(),
                (buf.len() * 8) as u32,
            )
        };
        if ok == 0 {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(())
        }
    }

    fn mutate(
        how: Mutation,
        dir: &Path,
        exe: &Path,
        n: usize,
        other_bytes: &[u8],
    ) -> std::io::Result<()> {
        match how {
            Mutation::AwayThenWrite => {
                std::fs::rename(exe, dir.join(format!("away-{n}.exe")))?;
                std::fs::write(exe, other_bytes)
            }
            Mutation::MoveFileExOver => {
                let other = dir.join(format!("other-{n}.exe"));
                std::fs::write(&other, other_bytes)?;
                // SAFETY: NUL-terminated paths.
                let ok = unsafe {
                    MoveFileExW(
                        wide(&other).as_ptr(),
                        wide(exe).as_ptr(),
                        MOVEFILE_REPLACE_EXISTING,
                    )
                };
                if ok == 0 {
                    Err(std::io::Error::last_os_error())
                } else {
                    Ok(())
                }
            }
            Mutation::PosixRenameOver => {
                let other = dir.join(format!("other-{n}.exe"));
                std::fs::write(&other, other_bytes)?;
                posix_rename_over(&other, "a.exe")
            }
            Mutation::AncestorAwayThenRecreate => {
                std::fs::rename(dir, dir.with_file_name(format!("dir-away-{n}")))?;
                std::fs::create_dir(dir)?;
                std::fs::write(exe, other_bytes)
            }
        }
    }

    /// One run of the real sequence with `how` done at barrier `at`. `Ok(note)` or `Err`.
    fn race_once(how: Mutation, at: &str, n: usize) -> Result<String, String> {
        let s = scratch()?;
        let dir = s.path().join("dir");
        std::fs::create_dir(&dir).ctx("mkdir")?;
        let exe = dir.join("a.exe");
        let (orig, orig_id) = copy_self(&exe)?;
        let rival = rival_pe()?;
        // The POSIX control: the same rename helper must replace a target that is not running
        // (verified by file id) before an image-side "moved nothing" can count as HELD.
        let control = match how {
            Mutation::PosixRenameOver => Some(posix_control(s.path(), &rival)),
            _ => None,
        };
        let mut k = Kid::spawn(&exe)?;
        k.send("key-barriers");
        let mut did: Option<Result<String, String>> = None;
        let mut replaced = false;
        let result = loop {
            let r = k.recv();
            let Some(tag) = r.strip_prefix("BARRIER ") else {
                break r;
            };
            if tag == at {
                did = Some(match mutate(how, &dir, &exe, n, &rival) {
                    Ok(()) => match file_id_at(&exe) {
                        // Verified before the barrier is released: the old name opens
                        // another file now.
                        Ok(id) if id != orig_id => {
                            replaced = true;
                            Ok("replaced by a rival build (verified)".into())
                        }
                        // The call reported success but the image kept its name: HELD only when
                        // the control replaced a non-running target and the other file also
                        // stayed where it was (nothing moved at all).
                        Ok(_) if matches!(how, Mutation::PosixRenameOver) => {
                            let other = dir.join(format!("other-{n}.exe"));
                            match (control.clone().unwrap(), file_id_at(&other)) {
                                (Ok(c), Ok(oid)) if oid != orig_id => Ok(format!(
                                    "HELD (the OS kept the running image: the POSIX rename \
                                     reported success and moved nothing; {c})"
                                )),
                                (Err(c), _) => Err(format!(
                                    "the rename did not take, and the control failed: {c}"
                                )),
                                (_, r) => Err(format!(
                                    "mutation did not take, and the other file is not where it \
                                     was ({r:?})"
                                )),
                            }
                        }
                        Ok(_) => Err("mutation did not take: same file id at the old name".into()),
                        Err(e) => Err(format!("mutation unverifiable: {e}")),
                    },
                    // A replacement OF the running image refused by the OS: a fact about
                    // Windows, and no wrong key can follow (orchestrator decision Q8).
                    Err(e)
                        if matches!(how, Mutation::MoveFileExOver | Mutation::PosixRenameOver)
                            && matches!(
                                e.raw_os_error(),
                                Some(ERROR_SHARING_VIOLATION | ERROR_ACCESS_DENIED)
                            ) =>
                    {
                        Ok(format!("HELD (the OS protects the running image: {e})"))
                    }
                    // Past the open, the key's own handle (no delete sharing) is what refuses:
                    // the property the design relies on.
                    Err(e)
                        if at != "after-query"
                            && matches!(
                                e.raw_os_error(),
                                Some(ERROR_SHARING_VIOLATION | ERROR_ACCESS_DENIED)
                            ) =>
                    {
                        Ok(format!("blocked while the key's handle is held ({e})"))
                    }
                    Err(e) => Err(format!("refused mutation: {e}")),
                });
            }
            k.send("go");
        };
        let did = did.ok_or("the barrier never came")??;
        let (_, said) = judge(&result, &orig).map_err(|e| format!("{did}; {e}"))?;
        // A replaced name must have been refused at the canonical comparison, by both candidates,
        // when the replacement happened before either was opened.
        if replaced && at == "after-query" && !both_refused_by_hash(&result) {
            return Err(format!(
                "{did}; {said}; not both candidates were refused at the canonical comparison"
            ));
        }
        Ok(format!("{did}; {said}"))
    }

    /// The POSIX rename helper on a target nobody runs: it must replace it (the target's name
    /// then opens the source's file id).
    fn posix_control(scratch: &Path, rival: &[u8]) -> Result<String, String> {
        let target = scratch.join("ctl-target.exe");
        let src = scratch.join("ctl-src.exe");
        std::fs::write(&target, b"control target").ctx("write control target")?;
        std::fs::write(&src, rival).ctx("write control source")?;
        let src_id = file_id_at(&src).ctx("control source id")?;
        posix_rename_over(&src, "ctl-target.exe").ctx("control rename")?;
        if file_id_at(&target).ctx("control target id")? == src_id {
            Ok("control: replaced a non-running target (verified)".into())
        } else {
            Err("control: the POSIX rename did not replace a non-running target".into())
        }
    }

    fn w1_race(how: Mutation) -> Outcome {
        let mut notes = Vec::new();
        let mut failed = false;
        for (i, at) in ["after-query", "after-open", "after-hash"]
            .into_iter()
            .enumerate()
        {
            match race_once(how, at, i) {
                Ok(n) => notes.push(format!("{at}: {n}")),
                Err(e) => {
                    failed = true;
                    notes.push(format!("{at}: FAIL {e}"));
                }
            }
        }
        let note = format!("{how:?}: {}", notes.join(" | "));
        if failed { Fail(note) } else { Pass(note) }
    }

    pub(crate) fn w1_away() -> Outcome {
        w1_race(Mutation::AwayThenWrite)
    }
    pub(crate) fn w1_movefileex() -> Outcome {
        w1_race(Mutation::MoveFileExOver)
    }
    pub(crate) fn w1_posix() -> Outcome {
        w1_race(Mutation::PosixRenameOver)
    }
    pub(crate) fn w1_ancestor() -> Outcome {
        w1_race(Mutation::AncestorAwayThenRecreate)
    }

    /// Launch through a hard link, then replace that link while `a.exe` survives.
    pub(crate) fn w1_hard_link() -> Outcome {
        outcome((|| {
            let d = scratch()?;
            let exe = d.path().join("a.exe");
            let (orig, orig_id) = copy_self(&exe)?;
            let link = d.path().join("l.exe");
            // SAFETY: NUL-terminated paths.
            if unsafe {
                CreateHardLinkW(wide(&link).as_ptr(), wide(&exe).as_ptr(), std::ptr::null())
            } == 0
            {
                return Err(format!(
                    "CreateHardLinkW: {}",
                    std::io::Error::last_os_error()
                ));
            }
            let mut k = Kid::spawn(&link)?;
            let n = k.name().unwrap_or_default();
            let (_, first) = judge(&k.ask("key"), &orig)?;
            let mut notes = vec![format!("link launch: {first}, name {}", lossy(&n))];
            let how = match std::fs::remove_file(&link) {
                Ok(()) => "launch link deleted".to_string(),
                Err(e) => {
                    std::fs::rename(&link, d.path().join("l-away.exe")).map_err(|e2| {
                        format!(
                            "refused mutation: delete ({e}) and rename ({e2}) of the launch link"
                        )
                    })?;
                    format!("delete refused ({e}), renamed away")
                }
            };
            std::fs::write(&link, rival_pe()?).ctx("write a rival build at l.exe")?;
            if file_id_at(&link).ctx("id at l.exe")? == orig_id {
                return Err("mutation did not take".into());
            }
            let n = k.name().unwrap_or_default();
            let reply = k.ask("key");
            let (_, after) = judge(&reply, &orig).map_err(|e| format!("{how}: {e}"))?;
            if !both_refused_by_hash(&reply) {
                return Err(format!(
                    "{how}, replaced: {after}; not both candidates were refused at the canonical \
                     comparison"
                ));
            }
            notes.push(format!(
                "{how}, replaced by a rival build: {after}, name {}",
                lossy(&n)
            ));
            Ok(notes.join(" | "))
        })())
    }

    /// Launch through an upper-case spelling of the path.
    pub(crate) fn w1_case() -> Outcome {
        outcome((|| {
            let d = scratch()?;
            let exe = d.path().join("case-probe.exe");
            let (orig, _) = copy_self(&exe)?;
            let mut k = Kid::spawn(&exe.with_file_name("CASE-PROBE.EXE"))?;
            let n = k.name().unwrap_or_default();
            let (_, r) = judge(&k.ask("key"), &orig)?;
            Ok(format!("{r}, name {}", lossy(&n)))
        })())
    }

    /// Launch through the 8.3 short name of a long-named leaf.
    pub(crate) fn w1_short_name() -> Outcome {
        let run = || -> Result<Option<String>, String> {
            let d = scratch()?;
            let exe = d.path().join("a-long-file-name-for-the-probe.exe");
            let (orig, _) = copy_self(&exe)?;
            let mut short = vec![0u16; 32768];
            // SAFETY: NUL-terminated input; the output buffer's length is given.
            let n = unsafe {
                GetShortPathNameW(wide(&exe).as_ptr(), short.as_mut_ptr(), short.len() as u32)
            } as usize;
            if n == 0 || n >= short.len() {
                return Err(format!(
                    "GetShortPathNameW: {}",
                    std::io::Error::last_os_error()
                ));
            }
            let short = PathBuf::from(OsString::from_wide(&short[..n]));
            if short.file_name() == exe.file_name() {
                return Ok(None);
            }
            let mut k = Kid::spawn(&short)?;
            let nm = k.name().unwrap_or_default();
            let (_, r) = judge(&k.ask("key"), &orig)?;
            Ok(Some(format!(
                "launched as {}: {r}, name {}",
                short.display(),
                lossy(&nm)
            )))
        };
        match run() {
            Ok(Some(n)) => Pass(n),
            Ok(None) => Skip("8.3 disabled on this volume (no short name was made)".into()),
            Err(e) => Fail(e),
        }
    }

    // ---------------------------------------------------------------------------------
    // W2-W5
    // ---------------------------------------------------------------------------------

    /// W2: the whole sequence in this process gives this image's key.
    pub(crate) fn w2_globalroot() -> Outcome {
        outcome((|| {
            let want = blake3_of(
                &mut File::open(std::env::current_exe().ctx("current_exe")?).ctx("open self")?,
            )
            .ctx("hash self")?;
            match image_key(&mut |_| {}) {
                (Ok(k), rep) if k == want => Ok(format!("key equals the file's hash [{rep}]")),
                (Ok(_), rep) => Err(format!("key differs from the file's hash [{rep}]")),
                (Err(e), rep) => Err(format!("{e} [{rep}]")),
            }
        })())
    }

    /// W9: part B on altered copies of this executable, against part A of the running image:
    /// the unaltered file is accepted; a changed byte in a read-only section (`.text`, `.rdata`)
    /// is refused; a changed byte in `.data` is accepted, which is the stated scope (design note
    /// section 3: initial mutable data is not covered).
    pub(crate) fn w9_altered_candidates() -> Outcome {
        outcome((|| {
            let base = module_base().ok_or("GetModuleHandleExW failed")?;
            let a = memory_canon(base).map_err(|e| format!("part A: {e}"))?;
            let file = std::fs::read(std::env::current_exe().ctx("current_exe")?).ctx("read")?;
            let h = pe::Headers::parse(&file)?;
            let d = scratch()?;
            // Every case goes through the full candidate path: a file, opened denying write and
            // delete sharing, checked for local NTFS, read through that handle, checked against A.
            let run = |name: &str, bytes: &[u8]| -> Result<&'static str, String> {
                let p = d.path().join(format!("w9-{name}.exe"));
                std::fs::write(&p, bytes).ctx("write candidate")?;
                check_path(&p, &a, &mut |_| {}).map(|(_, v)| v)
            };
            // A byte inside section `name`, outside every range A zeroed (the IAT sits at the
            // start of .rdata on MSVC), searched outward from the middle.
            let flip = |name: &str| -> Result<Vec<u8>, String> {
                let s = h
                    .sections
                    .iter()
                    .find(|s| s.named(name))
                    .ok_or(format!("required section {name} is missing"))?;
                let len = s.raw_size.min(s.mapped_len());
                let masked = |o: u32| {
                    let rva = s.va + o;
                    a.zeroed.iter().any(|&(r, l)| {
                        u64::from(rva) >= u64::from(r)
                            && u64::from(rva) < u64::from(r) + u64::from(l)
                    })
                };
                let o = (0..len)
                    .map(|k| (len / 2 + k) % len)
                    .find(|&o| !masked(o))
                    .ok_or(format!("{name} is masked everywhere"))?;
                let mut f = file.clone();
                f[s.raw_ptr as usize + o as usize] ^= 1;
                Ok(f)
            };
            let mut notes = Vec::new();
            match run("unaltered", &file)? {
                "accepted" => {
                    notes.push(format!("unaltered: accepted (A {})", &hex(&a.hash)[..16]))
                }
                v => {
                    return Err(format!(
                        "the unaltered file is {v} (A zeroed {:?})",
                        a.zeroed
                    ));
                }
            }
            for name in [".text", ".rdata"] {
                match run(name, &flip(name)?)? {
                    "refused (canonical hash differs)" => notes.push(format!(
                        "{name} byte changed: refused at the canonical hash"
                    )),
                    v => return Err(format!("a changed {name} byte: {v}")),
                }
            }
            // A candidate missing a section: the section table one entry shorter.
            let mut short = file.clone();
            let nsec_at = u32::from_le_bytes(file[0x3c..0x40].try_into().unwrap()) as usize + 6;
            let nsec = u16::from_le_bytes([file[nsec_at], file[nsec_at + 1]]);
            short[nsec_at..nsec_at + 2].copy_from_slice(&(nsec - 1).to_le_bytes());
            match run("missing-section", &short) {
                Ok("refused (headers differ)") => {
                    notes.push("a section missing: refused at the headers".into())
                }
                Ok(v) => return Err(format!("a candidate missing a section: {v}")),
                Err(e) => notes.push(format!("a section missing: refused ({e})")),
            }
            // Printed, not judged: W10 decides whether a .data-only difference is caught.
            notes.push(format!(
                ".data byte changed: {}",
                run(".data", &flip(".data")?)?
            ));
            Ok(notes.join(" | "))
        })())
    }

    /// W3: write and truncate opens work on an ordinary copy (controls) and are refused on
    /// the running image; a rename of the running image works.
    pub(crate) fn w3_write_refused() -> Outcome {
        outcome((|| {
            let d = scratch()?;
            let plain = d.path().join("plain.exe");
            copy_self(&plain)?;
            OpenOptions::new()
                .write(true)
                .open(&plain)
                .ctx("control: write open of a plain file")?;
            OpenOptions::new()
                .write(true)
                .truncate(true)
                .open(&plain)
                .ctx("control: truncate of a plain file")?;
            let exe = d.path().join("a.exe");
            copy_self(&exe)?;
            let _k = Kid::spawn(&exe)?;
            let w = OpenOptions::new().write(true).open(&exe);
            let t = OpenOptions::new().write(true).truncate(true).open(&exe);
            let wd = OpenOptions::new().access_mode(FILE_WRITE_DATA).open(&exe);
            let r = std::fs::rename(&exe, d.path().join("moved.exe"));
            let del = std::fs::remove_file(d.path().join("moved.exe"));
            let note = format!(
                "running image: write {:?}, truncate {:?}, write-data {:?}, rename {:?}, delete {:?}",
                w.as_ref().err().map(std::io::Error::raw_os_error),
                t.as_ref().err().map(std::io::Error::raw_os_error),
                wd.as_ref().err().map(std::io::Error::raw_os_error),
                r.as_ref().err().map(std::io::Error::raw_os_error),
                del.as_ref().err().map(std::io::Error::raw_os_error),
            );
            if w.is_err() && t.is_err() && wd.is_err() && r.is_ok() {
                Ok(format!("controls ok; {note}"))
            } else {
                Err(note)
            }
        })())
    }

    /// W4: a key before, no key (a sharing violation) while a DELETE-access handle is held, a
    /// key after; and a write handle on a plain file refuses our open.
    pub(crate) fn w4_conflicting_access() -> Outcome {
        outcome((|| {
            let d = scratch()?;
            let exe = d.path().join("a.exe");
            let (orig, _) = copy_self(&exe)?;
            let mut k = Kid::spawn(&exe)?;
            if !judge_key(&k.ask("key"), &orig)? {
                return Err("no key before the conflicting handle".into());
            }
            let held = OpenOptions::new()
                .access_mode(DELETE)
                .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
                .open(&exe)
                .ctx("open the image with DELETE access")?;
            let during = k.ask("key");
            if !during.starts_with("NOKEY") || !during.contains("os error 32") {
                return Err(format!(
                    "while a DELETE handle is held: {during:?} (want NOKEY, sharing violation)"
                ));
            }
            drop(held);
            if !judge_key(&k.ask("key"), &orig)? {
                return Err("no key after the conflicting handle closed".into());
            }
            let plain = d.path().join("plain.bin");
            std::fs::write(&plain, b"data").ctx("write plain")?;
            let _w = OpenOptions::new()
                .access_mode(FILE_WRITE_DATA)
                .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
                .open(&plain)
                .ctx("write handle on plain")?;
            match OpenOptions::new()
                .read(true)
                .share_mode(FILE_SHARE_READ)
                .open(&plain)
            {
                Err(e) if e.raw_os_error() == Some(ERROR_SHARING_VIOLATION) => Ok(
                    "image: key, no key (sharing violation), key; plain file with a write handle: sharing violation"
                        .into(),
                ),
                other => Err(format!("plain file with a write handle: {other:?}")),
            }
        })())
    }

    fn would_block(r: Result<(), TryLockError>) -> bool {
        matches!(r, Err(TryLockError::WouldBlock))
    }

    /// W5: `std::fs::File` locks (LockFileEx) on independent handles, explicit unlock, and a
    /// killed holder.
    pub(crate) fn w5_locks() -> Outcome {
        outcome((|| {
            let d = scratch()?;
            let p = d.path().join("x.lock");
            std::fs::write(&p, b"").ctx("make lock file")?;
            let open = || OpenOptions::new().read(true).write(true).open(&p);
            let (a, b, c) = (open().ctx("a")?, open().ctx("b")?, open().ctx("c")?);
            let mut bad = Vec::new();
            if a.try_lock().is_err() {
                bad.push("a: first exclusive failed");
            }
            if !would_block(b.try_lock_shared()) {
                bad.push("b: shared beside exclusive is not WouldBlock");
            }
            if !would_block(b.try_lock()) {
                bad.push("b: exclusive beside exclusive is not WouldBlock");
            }
            a.unlock().ctx("a: unlock")?;
            if b.try_lock().is_err() {
                bad.push("b: exclusive refused after a's unlock");
            }
            b.unlock().ctx("b: unlock")?;
            if a.try_lock_shared().is_err() || b.try_lock_shared().is_err() {
                bad.push("two shared locks refused");
            }
            if !would_block(c.try_lock()) {
                bad.push("c: exclusive beside two shared is not WouldBlock");
            }
            drop((a, b));
            if c.try_lock().is_err() {
                bad.push("c: exclusive refused after the shared handles closed");
            }
            drop(c);
            let exe = d.path().join("a.exe");
            copy_self(&exe)?;
            let mut k = Kid::spawn(&exe)?;
            if k.ask(&format!("lock {}", p.display())) != "LOCKED" {
                return Err("child could not lock".into());
            }
            let e = open().ctx("e")?;
            if !would_block(e.try_lock()) {
                bad.push("e: exclusive beside a child's is not WouldBlock");
            }
            let _ = k.proc.kill();
            let _ = k.proc.wait();
            let t = Instant::now();
            let freed = loop {
                if e.try_lock().is_ok() {
                    break Some(t.elapsed());
                }
                if t.elapsed() > Duration::from_secs(10) {
                    break None;
                }
                std::thread::sleep(Duration::from_millis(20));
            };
            match (bad.is_empty(), freed) {
                (true, Some(el)) => Ok(format!("killed holder's lock freed after {el:?}")),
                (_, f) => Err(format!("{bad:?}; killed holder's lock freed: {f:?}")),
            }
        })())
    }

    // ---------------------------------------------------------------------------------
    // W6: the private rule (design note section 6), top-down from the volume root
    // ---------------------------------------------------------------------------------

    /// Rights no one but the trusted may hold on any path element.
    const CHAIN_BAD: u32 =
        DELETE | WRITE_DAC | WRITE_OWNER | FILE_DELETE_CHILD | GENERIC_WRITE | GENERIC_ALL;
    /// Also refused on cache folders, entries and lock files (`FILE_WRITE_DATA` and
    /// `FILE_APPEND_DATA` are the same bits).
    const LEAF_BAD: u32 = CHAIN_BAD | FILE_ADD_FILE | FILE_ADD_SUBDIRECTORY;

    /// The trusted principals as SID strings: the user, SYSTEM, Administrators,
    /// TrustedInstaller (the owner of system folders such as a volume root).
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

    fn trust() -> Result<Trust, String> {
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

    /// The rule on an open handle (it needs READ_CONTROL and attribute access): not a reparse
    /// point, a DACL, a trusted owner, and no allow ACE that applies to the object giving an
    /// untrusted principal a right in `bad`. Deny ACEs are skipped; other ACE types refuse.
    fn check_handle(f: &File, bad: u32, t: &Trust) -> Result<(), String> {
        let attrs = f.metadata().ctx("stat")?.file_attributes();
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

    /// Open one path element for checking: attributes and READ_CONTROL, not following a
    /// reparse point, sharing read and write but not delete, so it cannot be renamed while
    /// the walk holds it.
    fn open_element(p: &Path) -> std::io::Result<File> {
        OpenOptions::new()
            .access_mode(READ_CONTROL | FILE_READ_ATTRIBUTES)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(p)
    }

    /// The walk: from `\\?\X:\` down to `dir`, each element opened and checked before the next
    /// is opened under it (a reparse point is refused before descending), every element with
    /// [`CHAIN_BAD`], `dir` itself with [`LEAF_BAD`]. With `create`, a missing element is made
    /// under its checked parent, then checked. Returns the rooted spelling all I/O must use.
    fn walk(dir: &Path, create: bool, t: &Trust) -> Result<PathBuf, String> {
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
        let f = open_element(&root).ctx("open the volume root")?;
        local_ntfs(&f)?;
        let bad = if names.is_empty() {
            LEAF_BAD
        } else {
            CHAIN_BAD
        };
        check_handle(&f, bad, t).map_err(|e| format!("{}: {e}", root.display()))?;
        held.push(f);
        let mut cur = root;
        for (i, n) in names.iter().enumerate() {
            cur.push(n);
            let f = match open_element(&cur) {
                Err(e) if e.kind() == ErrorKind::NotFound && create => {
                    std::fs::create_dir(&cur).ctx(&format!("create {}", cur.display()))?;
                    open_element(&cur).ctx(&format!("open {}", cur.display()))?
                }
                r => r.ctx(&format!("open {}", cur.display()))?,
            };
            let bad = if i + 1 == names.len() {
                LEAF_BAD
            } else {
                CHAIN_BAD
            };
            check_handle(&f, bad, t).map_err(|e| format!("{}: {e}", cur.display()))?;
            held.push(f);
        }
        Ok(cur)
    }

    fn icacls(p: &Path, args: &[&str]) -> Result<(), String> {
        let st = Command::new("icacls")
            .arg(p)
            .args(args)
            .stdout(Stdio::null())
            .status()
            .ctx("icacls")?;
        st.success()
            .then_some(())
            .ok_or(format!("icacls {} {args:?}: {st}", p.display()))
    }

    /// W6: the real roots (text, IR, override) pass the walk, made only under checked parents;
    /// a fresh entry and lock file pass on their own handles; refused: a leaf others can
    /// modify, a private leaf under a parent others can delete children of, a junction, an
    /// entry others can modify.
    pub(crate) fn w6_private() -> Outcome {
        outcome((|| {
            let t = trust()?;
            let mut notes = Vec::new();
            let mut probe_override = None;
            for (what, p) in cache_roots()? {
                if what == "probe override" {
                    probe_override = p.parent().map(Path::to_path_buf);
                }
                let rooted = walk(&p, true, &t).map_err(|e| format!("{what} refused: {e}"))?;
                notes.push(format!("{what} {} passes", rooted.display()));
            }
            let lad = dirs::cache_dir().ok_or("no per-user cache folder")?;
            walk(&lad, false, &t)?;
            let d = tempfile::Builder::new()
                .prefix("tls117-")
                .tempdir_in(&lad)
                .ctx("scratch under the cache folder")?;
            // The entry and lock use the walk's rooted spelling; the negative cases below use
            // the plain spelling, because icacls and mklink do not take `\\?\` paths.
            let checked = walk(d.path(), false, &t).ctx("scratch folder")?;
            let plain = d.path();
            // A fresh entry and a lock file, checked on the handles that read and lock them.
            let entry = checked.join("entry.bin");
            std::fs::write(&entry, b"x").ctx("write entry")?;
            check_handle(&File::open(&entry).ctx("open entry")?, LEAF_BAD, &t)
                .map_err(|e| format!("a fresh entry refused: {e}"))?;
            let lock = OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(checked.join("x.lock"))
                .ctx("open lock")?;
            check_handle(&lock, LEAF_BAD, &t).map_err(|e| format!("a fresh lock refused: {e}"))?;
            notes.push("fresh entry and lock pass".into());
            // A leaf others can modify.
            let open = plain.join("open");
            std::fs::create_dir(&open).ctx("mkdir open")?;
            icacls(&open, &["/grant", "*S-1-1-0:(M)"])?;
            match walk(&open, false, &t) {
                Err(e) => notes.push(format!("open leaf refused ({e})")),
                Ok(_) => return Err("a leaf Everyone can modify passed".into()),
            }
            // A private leaf under a parent others can delete children of.
            let parent = plain.join("parent");
            let leaf = parent.join("leaf");
            std::fs::create_dir_all(&leaf).ctx("mkdir parent/leaf")?;
            icacls(&parent, &["/grant", "*S-1-1-0:(DC)"])?;
            let leaf_alone = check_handle(&open_element(&leaf).ctx("open leaf")?, LEAF_BAD, &t);
            match (leaf_alone, walk(&leaf, false, &t)) {
                (Ok(()), Err(e)) => notes.push(format!("replaceable parent refused ({e})")),
                other => return Err(format!("replaceable parent: {other:?}")),
            }
            // A junction.
            let target = plain.join("target");
            std::fs::create_dir(&target).ctx("mkdir target")?;
            let junction = plain.join("junction");
            let st = Command::new("cmd")
                .args(["/C", "mklink", "/J"])
                .arg(&junction)
                .arg(&target)
                .stdout(Stdio::null())
                .status()
                .ctx("mklink")?;
            if !st.success() {
                return Err(format!("mklink /J failed: {st}"));
            }
            match walk(&junction.join("below"), false, &t) {
                Err(e) if e.contains("reparse") => notes.push(format!("junction refused ({e})")),
                other => return Err(format!("junction: {other:?}")),
            }
            // An entry others can modify.
            let loose = plain.join("loose.bin");
            std::fs::write(&loose, b"x").ctx("write loose")?;
            icacls(&loose, &["/grant", "*S-1-1-0:(M)"])?;
            match check_handle(&File::open(&loose).ctx("open loose")?, LEAF_BAD, &t) {
                Err(e) => notes.push(format!("permissive entry refused ({e})")),
                Ok(()) => return Err("an entry Everyone can modify passed".into()),
            }
            if let Some(o) = probe_override {
                let _ = std::fs::remove_dir_all(o);
            }
            Ok(notes.join(" | "))
        })())
    }

    /// W7: delete a file another handle holds open (sharing delete): the delete succeeds;
    /// whether its name goes at once and whether its folder can then be removed is recorded,
    /// with NotFound kept apart from an access error.
    pub(crate) fn w7_delete_open() -> Outcome {
        outcome((|| {
            let d = scratch()?;
            let sub = d.path().join("k");
            std::fs::create_dir(&sub).ctx("mkdir")?;
            let f = sub.join("e.bin");
            std::fs::write(&f, b"x").ctx("write")?;
            let h = OpenOptions::new()
                .read(true)
                .share_mode(FILE_SHARE_READ | FILE_SHARE_DELETE)
                .open(&f)
                .ctx("open")?;
            std::fs::remove_file(&f).ctx("delete while open")?;
            let name = match std::fs::symlink_metadata(&f) {
                Err(e) if e.kind() == ErrorKind::NotFound => "name gone at once".to_string(),
                Ok(_) => "name still there (delete pending)".to_string(),
                Err(e) => format!("name state unreadable: {e}"),
            };
            let folder = match std::fs::remove_dir(&sub) {
                Ok(()) => "folder removed while the file was open".to_string(),
                Err(e) => format!("folder not removed while open ({e})"),
            };
            drop(h);
            Ok(format!("delete ok; {name}; {folder}"))
        })())
    }

    type Item = (&'static str, fn() -> Outcome);

    pub(crate) fn all() -> Vec<Item> {
        vec![
            ("W0 local NTFS: image and cache roots", w0_local),
            ("W1 mapped name follows a rename", w1_name_follows_rename),
            ("W1 rename away + write", w1_away),
            ("W1 MoveFileEx replace", w1_movefileex),
            ("W1 POSIX rename replace", w1_posix),
            ("W1 ancestor folder replaced", w1_ancestor),
            ("W1 hard link", w1_hard_link),
            ("W1 other case", w1_case),
            ("W1 8.3 short name", w1_short_name),
            ("W2 key of the unchanged image", w2_globalroot),
            ("W9 part B on altered copies", w9_altered_candidates),
            ("W3 running image refuses write", w3_write_refused),
            ("W4 conflicting access handle", w4_conflicting_access),
            ("W5 std::fs::File locks", w5_locks),
            ("W6 private folder walk", w6_private),
            ("W7 delete while open", w7_delete_open),
        ]
    }

    pub(crate) fn main() -> i32 {
        if std::env::args().nth(1).as_deref() == Some("child") {
            child_loop();
            return 0;
        }
        let mut failed = false;
        for (name, f) in all() {
            let line = match std::panic::catch_unwind(f) {
                Ok(Pass(n)) => format!("PASS {name}: {n}"),
                Ok(Skip(n)) => format!("SKIP {name}: {n}"),
                Ok(Fail(n)) => {
                    failed = true;
                    format!("FAIL {name}: {n}")
                }
                Err(_) => {
                    failed = true;
                    format!("FAIL {name}: panicked")
                }
            };
            println!("{line}");
        }
        i32::from(failed)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn check(o: Outcome) {
            match o {
                Pass(n) | Skip(n) => println!("{n}"),
                Fail(n) => panic!("{n}"),
            }
        }

        /// The child half: run by [`Kid::spawn`] in test mode.
        #[test]
        #[ignore = "run by the probe tests as a child"]
        fn child_mode() {
            if std::env::var_os("TLS117_CHILD").is_some() {
                child_loop();
            }
        }

        #[test]
        fn w0() {
            check(w0_local());
        }
        #[test]
        fn w1_name() {
            check(w1_name_follows_rename());
        }
        #[test]
        fn w1_rename_away() {
            check(w1_away());
        }
        #[test]
        fn w1_move_file_ex() {
            check(w1_movefileex());
        }
        #[test]
        fn w1_posix_rename() {
            check(w1_posix());
        }
        #[test]
        fn w1_ancestor_folder() {
            check(w1_ancestor());
        }
        #[test]
        fn w1_link() {
            check(w1_hard_link());
        }
        #[test]
        fn w1_other_case() {
            check(w1_case());
        }
        #[test]
        fn w1_short() {
            check(w1_short_name());
        }
        #[test]
        fn w2() {
            check(w2_globalroot());
        }
        #[test]
        fn w9() {
            check(w9_altered_candidates());
        }
        #[test]
        fn w3() {
            check(w3_write_refused());
        }
        #[test]
        fn w4() {
            check(w4_conflicting_access());
        }
        #[test]
        fn w5() {
            check(w5_locks());
        }
        #[test]
        fn w6() {
            check(w6_private());
        }
        #[test]
        fn w7() {
            check(w7_delete_open());
        }
    }
}

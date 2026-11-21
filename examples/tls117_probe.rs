//! TLS-117's native probe (design note
//! `docs/superpowers/notes/2026-11-21-tls117-windows-image-key.md`, section "The probe").
//!
//! Settles, on a real Windows machine, the facts the IR cache's Windows key and private-folder
//! check rest on. It prints one `PASS`, `FAIL` or `SKIP` line per W-item and exits 1 when any
//! item fails. Two ways to run it (both on Windows, from the repository root):
//!
//! ```text
//! cargo run --example tls117_probe
//! cargo test --example tls117_probe -- --test-threads=1
//! ```
//!
//! The key sequence here is a prototype of the product code, not the product code: it hashes the
//! whole file with blake3 and does not look for the IR marker.

#[cfg(not(windows))]
fn main() {
    eprintln!("tls117_probe: Windows only");
    std::process::exit(2);
}

#[cfg(windows)]
fn main() {
    std::process::exit(win::main());
}

#[cfg(windows)]
mod win {
    use std::ffi::{OsStr, OsString, c_void};
    use std::fs::{File, OpenOptions};
    use std::io::{BufRead, BufReader, Read, Write};
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
    use std::os::windows::io::AsRawHandle;
    use std::path::{Path, PathBuf};
    use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
    use std::time::{Duration, Instant};
    use windows_sys::Win32::Foundation::{
        CloseHandle, ERROR_SHARING_VIOLATION, GENERIC_ALL, GENERIC_WRITE, HANDLE, LocalFree,
    };
    use windows_sys::Win32::Security::Authorization::{GetSecurityInfo, SE_FILE_OBJECT};
    use windows_sys::Win32::Security::{
        ACCESS_ALLOWED_ACE, ACE_HEADER, ACL, DACL_SECURITY_INFORMATION, EqualSid, GetAce,
        GetTokenInformation, INHERIT_ONLY_ACE, IsWellKnownSid, OWNER_SECURITY_INFORMATION, PSID,
        TOKEN_QUERY, TOKEN_USER, TokenUser, WinBuiltinAdministratorsSid, WinLocalSystemSid,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        CreateHardLinkW, DELETE, FILE_ADD_FILE, FILE_ADD_SUBDIRECTORY,
        FILE_ATTRIBUTE_REPARSE_POINT, FILE_DELETE_CHILD, FILE_FLAG_BACKUP_SEMANTICS,
        FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ,
        FILE_SHARE_WRITE, FILE_WRITE_DATA, GetShortPathNameW, GetVolumeInformationByHandleW,
        READ_CONTROL, WRITE_DAC, WRITE_OWNER,
    };
    use windows_sys::Win32::System::LibraryLoader::{
        GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
        GetModuleHandleExW,
    };
    use windows_sys::Win32::System::ProcessStatus::K32GetMappedFileNameW;
    use windows_sys::Win32::System::SystemServices::{
        ACCESS_ALLOWED_ACE_TYPE, ACCESS_DENIED_ACE_TYPE,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    /// An address inside this image (as `IR_IMAGE_MARKER` is in the product).
    #[used]
    static ANCHOR: [u8; 16] = *b"TLS117-PROBE-IMG";

    // ---------------------------------------------------------------------------------
    // The key sequence (design note section 2, steps 1-5)
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

    /// Steps 2 and 5: the mapped file name of the module at `base`, raw UTF-16 (no folding),
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

    /// Step 3: open a device-form name through `\\?\GLOBALROOT`, read access, sharing read
    /// only, not following a reparse point.
    fn open_device_name(name: &[u16]) -> std::io::Result<File> {
        let mut p: Vec<u16> = OsStr::new(r"\\?\GLOBALROOT").encode_wide().collect();
        p.extend_from_slice(name);
        OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .open(PathBuf::from(OsString::from_wide(&p)))
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
        (ok != 0).then(|| String::from_utf16_lossy(&name[..end]))
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

    /// The whole sequence. `barrier(tag)` runs between the steps (a child waits there for its
    /// parent). `Err` is "no key", with the reason.
    fn image_key(barrier: &mut dyn FnMut(&str)) -> Result<[u8; 32], String> {
        let base = module_base().ok_or("GetModuleHandleExW failed")?;
        let n1 = mapped_name(base).ok_or("first mapped-name query failed")?;
        barrier("after-query");
        let mut f = open_device_name(&n1).map_err(|e| format!("open: {e}"))?;
        barrier("after-open");
        let m = f.metadata().map_err(|e| format!("stat: {e}"))?;
        if !m.is_file() || m.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err("not a regular file".into());
        }
        match fs_name(&f).as_deref() {
            Some("NTFS") => {}
            other => return Err(format!("volume is not NTFS: {other:?}")),
        }
        let hash = blake3_of(&mut f).map_err(|e| format!("read: {e}"))?;
        barrier("after-hash");
        let n2 = mapped_name(base).ok_or("second mapped-name query failed")?;
        if n1 != n2 {
            return Err(format!(
                "mapped name changed: {} -> {}",
                String::from_utf16_lossy(&n1),
                String::from_utf16_lossy(&n2)
            ));
        }
        drop(f);
        Ok(hash)
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

    fn hex(b: &[u8]) -> String {
        b.iter().map(|x| format!("{x:02x}")).collect()
    }

    fn hex16(w: &[u16]) -> String {
        w.iter().map(|x| format!("{x:04x}")).collect()
    }

    fn key_line(r: Result<[u8; 32], String>) -> String {
        match r {
            Ok(k) => format!("KEY {}", hex(&k)),
            Err(e) => format!("NOKEY {e}"),
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
        fn spawn(exe: &Path) -> std::io::Result<Kid> {
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
            let mut proc = cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).spawn()?;
            let stdin = proc.stdin.take().unwrap();
            let out = BufReader::new(proc.stdout.take().unwrap());
            let mut k = Kid { proc, stdin, out };
            let r = k.recv();
            if r != "READY" {
                return Err(std::io::Error::other(format!("child said {r:?}")));
            }
            Ok(k)
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
    // The W-items
    // ---------------------------------------------------------------------------------

    pub(crate) enum Outcome {
        Pass(String),
        Fail(String),
        Skip(String),
    }
    use Outcome::{Fail, Pass, Skip};

    fn wide(p: &Path) -> Vec<u16> {
        p.as_os_str().encode_wide().chain([0]).collect()
    }

    /// A scratch folder with a copy of this executable named `name`, and the copy's hash.
    fn copy_self(dir: &Path, name: &str) -> std::io::Result<(PathBuf, [u8; 32])> {
        let exe = dir.join(name);
        std::fs::copy(std::env::current_exe()?, &exe)?;
        let h = blake3_of(&mut File::open(&exe)?)?;
        Ok((exe, h))
    }

    fn scratch() -> tempfile::TempDir {
        tempfile::Builder::new()
            .prefix("tls117-")
            .tempdir()
            .expect("a scratch folder")
    }

    fn ends_with(name: &[u16], leaf: &str) -> bool {
        let tail: Vec<u16> = OsStr::new(&format!("\\{leaf}")).encode_wide().collect();
        name.ends_with(&tail)
    }

    /// The key a child reports must be the running image's, or none.
    fn judge_key(r: &str, orig: &[u8; 32]) -> Result<bool, String> {
        if let Some(k) = r.strip_prefix("KEY ") {
            if k == hex(orig) {
                Ok(true)
            } else {
                Err(format!("WRONG KEY {k}"))
            }
        } else if r.starts_with("NOKEY") {
            Ok(false)
        } else {
            Err(format!("unexpected reply {r:?}"))
        }
    }

    /// W0: the image's volume and the cache root's volume are NTFS.
    pub(crate) fn w0_ntfs() -> Outcome {
        let img = File::open(std::env::current_exe().unwrap()).map(|f| fs_name(&f));
        let root = dirs_cache().and_then(|d| {
            OpenOptions::new()
                .read(true)
                .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
                .open(d)
                .ok()
        });
        let root = root.map(|f| fs_name(&f));
        match (img, root) {
            (Ok(Some(a)), Some(Some(b))) if a == "NTFS" && b == "NTFS" => {
                Pass("image and cache root on NTFS".into())
            }
            other => Fail(format!("volumes: {other:?}")),
        }
    }

    /// `%LOCALAPPDATA%` (the parent of ENG-113's `al-ch-snapshot-cache`).
    fn dirs_cache() -> Option<PathBuf> {
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
    }

    /// W1: the mapped name follows a rename of the running image.
    pub(crate) fn w1_name_follows_rename() -> Outcome {
        let d = scratch();
        let (exe, _) = copy_self(d.path(), "a.exe").unwrap();
        let mut k = Kid::spawn(&exe).unwrap();
        let Some(n1) = k.name() else {
            return Fail("no mapped name".into());
        };
        if let Err(e) = std::fs::rename(&exe, d.path().join("b.exe")) {
            return Fail(format!("rename of the running image refused: {e}"));
        }
        let Some(n2) = k.name() else {
            return Fail("no mapped name after the rename".into());
        };
        match (ends_with(&n1, "a.exe"), ends_with(&n2, "b.exe")) {
            (true, true) => Pass(format!(
                "{} -> {}",
                String::from_utf16_lossy(&n1),
                String::from_utf16_lossy(&n2)
            )),
            _ => Fail(format!(
                "stale or odd name: {} -> {}",
                String::from_utf16_lossy(&n1),
                String::from_utf16_lossy(&n2)
            )),
        }
    }

    /// What the parent does at the barrier.
    #[derive(Clone, Copy, Debug)]
    enum Replace {
        /// Rename the image away, then write another file at its old name.
        AwayThenWrite,
        /// Rename another file over the image's name (replace-if-exists).
        RenameOver,
    }

    fn replace(exe: &Path, how: Replace, n: usize) -> String {
        let other = exe.with_file_name(format!("other-{n}.exe"));
        let _ = std::fs::write(&other, format!("another build {n}"));
        match how {
            Replace::AwayThenWrite => {
                match std::fs::rename(exe, exe.with_file_name(format!("away-{n}.exe"))) {
                    Ok(()) => match std::fs::rename(&other, exe) {
                        Ok(()) => "renamed away, replaced".into(),
                        Err(e) => format!("renamed away, replace failed: {e}"),
                    },
                    Err(e) => format!("rename away refused: {e}"),
                }
            }
            Replace::RenameOver => match std::fs::rename(&other, exe) {
                Ok(()) => "renamed over".into(),
                Err(e) => format!("rename over refused: {e}"),
            },
        }
    }

    /// W1 (race): replacement at each barrier of the real sequence gives the running image's
    /// key or none.
    pub(crate) fn w1_race() -> Outcome {
        let mut notes = Vec::new();
        for how in [Replace::AwayThenWrite, Replace::RenameOver] {
            for (i, at) in ["after-query", "after-open", "after-hash"]
                .into_iter()
                .enumerate()
            {
                let d = scratch();
                let (exe, orig) = copy_self(d.path(), "a.exe").unwrap();
                let mut k = Kid::spawn(&exe).unwrap();
                k.send("key-barriers");
                let mut did = String::from("-");
                let result = loop {
                    let r = k.recv();
                    if let Some(tag) = r.strip_prefix("BARRIER ") {
                        if tag == at {
                            did = replace(&exe, how, i);
                        }
                        k.send("go");
                    } else {
                        break r;
                    }
                };
                match judge_key(&result, &orig) {
                    Ok(got) => notes.push(format!(
                        "{how:?}@{at}: {did}; {}",
                        if got { "key" } else { "no key" }
                    )),
                    Err(e) => return Fail(format!("{how:?}@{at}: {did}; {e}")),
                }
            }
        }
        Pass(notes.join(" | "))
    }

    /// W1 (links, case, 8.3): launch through a hard link; replace the launch link while the
    /// other survives; launch through another case and through the short name.
    pub(crate) fn w1_links() -> Outcome {
        let mut notes = Vec::new();
        // Launch through a hard link.
        let d = scratch();
        let (exe, orig) = copy_self(d.path(), "a.exe").unwrap();
        let link = d.path().join("l.exe");
        // SAFETY: NUL-terminated paths.
        if unsafe { CreateHardLinkW(wide(&link).as_ptr(), wide(&exe).as_ptr(), std::ptr::null()) }
            == 0
        {
            return Fail(format!(
                "CreateHardLinkW: {}",
                std::io::Error::last_os_error()
            ));
        }
        let mut k = Kid::spawn(&link).unwrap();
        let n = k.name().unwrap_or_default();
        match judge_key(&k.ask("key"), &orig) {
            Ok(true) => notes.push(format!(
                "link launch: key, name {}",
                String::from_utf16_lossy(&n)
            )),
            Ok(false) => notes.push("link launch: no key".into()),
            Err(e) => return Fail(format!("link launch: {e}")),
        }
        // Replace the launch link; `a.exe` survives.
        let did = match std::fs::remove_file(&link) {
            Ok(()) => "launch link deleted".to_string(),
            Err(e) => match std::fs::rename(&link, d.path().join("l-away.exe")) {
                Ok(()) => format!("delete refused ({e}), renamed away"),
                Err(e2) => format!("delete refused ({e}), rename refused ({e2})"),
            },
        };
        let _ = std::fs::write(&link, "another build");
        let n = k.name().unwrap_or_default();
        match judge_key(&k.ask("key"), &orig) {
            Ok(got) => notes.push(format!(
                "{did}: {}, name {}",
                if got { "key" } else { "no key" },
                String::from_utf16_lossy(&n)
            )),
            Err(e) => return Fail(format!("{did}: {e}")),
        }
        drop(k);
        // Another case.
        let d = scratch();
        let (exe, orig) = copy_self(d.path(), "case-probe.exe").unwrap();
        let upper = exe.with_file_name("CASE-PROBE.EXE");
        let mut k = Kid::spawn(&upper).unwrap();
        let n = k.name().unwrap_or_default();
        match judge_key(&k.ask("key"), &orig) {
            Ok(got) => notes.push(format!(
                "case launch: {}, name {}",
                if got { "key" } else { "no key" },
                String::from_utf16_lossy(&n)
            )),
            Err(e) => return Fail(format!("case launch: {e}")),
        }
        drop(k);
        // The 8.3 name.
        let d = scratch();
        let (exe, orig) = copy_self(d.path(), "a-long-file-name-for-the-probe.exe").unwrap();
        let mut short = vec![0u16; 32768];
        // SAFETY: NUL-terminated input; the output buffer's length is given.
        let n = unsafe {
            GetShortPathNameW(wide(&exe).as_ptr(), short.as_mut_ptr(), short.len() as u32)
        } as usize;
        let short = PathBuf::from(OsString::from_wide(&short[..n.min(short.len())]));
        if n == 0 || short == exe {
            notes.push("8.3: no short names on this volume (skipped)".into());
        } else {
            let mut k = Kid::spawn(&short).unwrap();
            let nm = k.name().unwrap_or_default();
            match judge_key(&k.ask("key"), &orig) {
                Ok(got) => notes.push(format!(
                    "8.3 launch {}: {}, name {}",
                    short.display(),
                    if got { "key" } else { "no key" },
                    String::from_utf16_lossy(&nm)
                )),
                Err(e) => return Fail(format!("8.3 launch: {e}")),
            }
        }
        Pass(notes.join(" | "))
    }

    /// W2: the whole sequence in this process gives this image's key (so `\\?\GLOBALROOT`
    /// opens a device-form name).
    pub(crate) fn w2_globalroot() -> Outcome {
        let want = blake3_of(&mut File::open(std::env::current_exe().unwrap()).unwrap()).unwrap();
        match image_key(&mut |_| {}) {
            Ok(k) if k == want => Pass("key equals the file's hash".into()),
            Ok(_) => Fail("key differs from the file's hash".into()),
            Err(e) => Fail(e),
        }
    }

    /// W3: the running image refuses write and truncate opens, and allows a rename.
    pub(crate) fn w3_write_refused() -> Outcome {
        let d = scratch();
        let (exe, _) = copy_self(d.path(), "a.exe").unwrap();
        let _k = Kid::spawn(&exe).unwrap();
        let w = OpenOptions::new().write(true).open(&exe);
        let t = OpenOptions::new().write(true).truncate(true).open(&exe);
        let wd = OpenOptions::new().access_mode(FILE_WRITE_DATA).open(&exe);
        let r = std::fs::rename(&exe, d.path().join("moved.exe"));
        let del = std::fs::remove_file(d.path().join("moved.exe"));
        let note = format!(
            "write {:?}, truncate {:?}, write-data {:?}, rename {:?}, delete {:?}",
            w.as_ref().err().map(|e| e.raw_os_error()),
            t.as_ref().err().map(|e| e.raw_os_error()),
            wd.as_ref().err().map(|e| e.raw_os_error()),
            r.as_ref().err().map(|e| e.raw_os_error()),
            del.as_ref().err().map(|e| e.raw_os_error()),
        );
        if w.is_err() && t.is_err() && wd.is_err() && r.is_ok() {
            Pass(note)
        } else {
            Fail(note)
        }
    }

    /// W4: a conflicting access handle (delete access on the image; write access on a plain
    /// file) makes the deny-write, deny-delete open fail: no key, never a wrong one.
    pub(crate) fn w4_conflicting_access() -> Outcome {
        let d = scratch();
        let (exe, orig) = copy_self(d.path(), "a.exe").unwrap();
        let mut k = Kid::spawn(&exe).unwrap();
        let held = OpenOptions::new()
            .access_mode(DELETE)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .open(&exe);
        let Ok(held) = held else {
            return Fail(format!(
                "cannot open the image with DELETE access: {held:?}"
            ));
        };
        let image = match judge_key(&k.ask("key"), &orig) {
            Ok(false) => "image with a DELETE handle: no key".to_string(),
            Ok(true) => return Fail("a key was made while a DELETE handle was open".into()),
            Err(e) => return Fail(e),
        };
        drop(held);
        let plain = d.path().join("plain.bin");
        std::fs::write(&plain, b"data").unwrap();
        let _w = OpenOptions::new()
            .access_mode(FILE_WRITE_DATA)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .open(&plain)
            .unwrap();
        let ours = OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ)
            .open(&plain);
        match ours {
            Err(e) if e.raw_os_error() == Some(ERROR_SHARING_VIOLATION as i32) => Pass(format!(
                "{image}; plain file with a write handle: sharing violation"
            )),
            other => Fail(format!("plain file with a write handle: {other:?}")),
        }
    }

    /// W5: `std::fs::File` locks (LockFileEx): independent handles conflict, an explicit
    /// unlock frees, and a killed holder's lock is freed (time measured).
    pub(crate) fn w5_locks() -> Outcome {
        let d = scratch();
        let p = d.path().join("x.lock");
        std::fs::write(&p, b"").unwrap();
        let open = || OpenOptions::new().read(true).write(true).open(&p).unwrap();
        let (a, b) = (open(), open());
        let mut bad = Vec::new();
        if a.try_lock().is_err() {
            bad.push("first exclusive failed");
        }
        if b.try_lock_shared().is_ok() {
            bad.push("shared granted beside exclusive");
        }
        if b.try_lock().is_ok() {
            bad.push("exclusive granted twice");
        }
        a.unlock().unwrap();
        if b.try_lock_shared().is_err() {
            bad.push("shared refused after unlock");
        }
        if a.try_lock_shared().is_err() {
            bad.push("two shared refused");
        }
        if b.try_lock().is_ok() {
            bad.push("exclusive granted beside shared");
        }
        drop((a, b));
        // A killed holder.
        let (exe, _) = copy_self(d.path(), "a.exe").unwrap();
        let mut k = Kid::spawn(&exe).unwrap();
        if k.ask(&format!("lock {}", p.display())) != "LOCKED" {
            return Fail("child could not lock".into());
        }
        let c = open();
        if c.try_lock().is_ok() {
            bad.push("exclusive granted beside a child's");
        }
        let _ = k.proc.kill();
        let _ = k.proc.wait();
        let t = Instant::now();
        let freed = loop {
            if c.try_lock().is_ok() {
                break Some(t.elapsed());
            }
            if t.elapsed() > Duration::from_secs(10) {
                break None;
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        match (bad.is_empty(), freed) {
            (true, Some(e)) => Pass(format!("killed holder's lock freed after {e:?}")),
            (_, f) => Fail(format!("{bad:?}; killed holder's lock freed: {f:?}")),
        }
    }

    // --- W6: the private check (prototype of the product rule, design note section 4) ---

    const LEAF_BAD: u32 = FILE_ADD_FILE | FILE_ADD_SUBDIRECTORY;
    const CHAIN_BAD: u32 =
        DELETE | WRITE_DAC | WRITE_OWNER | FILE_DELETE_CHILD | GENERIC_WRITE | GENERIC_ALL;

    struct Sd(*mut c_void);
    impl Drop for Sd {
        fn drop(&mut self) {
            // SAFETY: allocated by GetSecurityInfo.
            unsafe { LocalFree(self.0) };
        }
    }

    fn user_sid() -> Vec<u8> {
        let mut tok: HANDLE = std::ptr::null_mut();
        // SAFETY: the current process's pseudo-handle; `tok` is closed below.
        assert!(unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut tok) } != 0);
        let mut buf = vec![0u8; 512];
        let mut len = 0u32;
        // SAFETY: the buffer and its length match.
        let ok = unsafe {
            GetTokenInformation(
                tok,
                TokenUser,
                buf.as_mut_ptr().cast(),
                buf.len() as u32,
                &mut len,
            )
        };
        // SAFETY: opened above.
        unsafe { CloseHandle(tok) };
        assert!(ok != 0);
        buf
    }

    fn trusted(sid: PSID, user: &[u8]) -> bool {
        // SAFETY: `user` holds a TOKEN_USER (from `user_sid`), live for the call; `sid` is valid.
        unsafe {
            let tu = &*(user.as_ptr() as *const TOKEN_USER);
            EqualSid(sid, tu.User.Sid) != 0
                || IsWellKnownSid(sid, WinLocalSystemSid) != 0
                || IsWellKnownSid(sid, WinBuiltinAdministratorsSid) != 0
        }
    }

    /// One object: no reparse point, a DACL, a trusted owner, and no allow ACE for anyone
    /// else carrying a right in `bad`. Unknown ACE types refuse.
    fn check_one(p: &Path, bad: u32, user: &[u8]) -> Result<(), String> {
        let f = OpenOptions::new()
            .access_mode(READ_CONTROL | FILE_READ_ATTRIBUTES)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(p)
            .map_err(|e| format!("{}: open: {e}", p.display()))?;
        let attrs = f.metadata().map_err(|e| e.to_string())?.file_attributes();
        if attrs & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(format!("{}: a reparse point", p.display()));
        }
        let (mut owner, mut dacl, mut sd): (PSID, *mut ACL, *mut c_void) = (
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        );
        // SAFETY: a live handle opened with READ_CONTROL; out-pointers are valid; `sd` is freed
        // by `Sd`.
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
            return Err(format!("{}: GetSecurityInfo {err}", p.display()));
        }
        let _sd = Sd(sd);
        if dacl.is_null() {
            return Err(format!("{}: a null DACL", p.display()));
        }
        if owner.is_null() || !trusted(owner, user) {
            return Err(format!(
                "{}: owner is not the user, SYSTEM or Administrators",
                p.display()
            ));
        }
        // SAFETY: `dacl` points into `sd`, alive until `_sd` drops.
        let count = unsafe { (*dacl).AceCount };
        for i in 0..count {
            let mut ace: *mut c_void = std::ptr::null_mut();
            // SAFETY: i < AceCount.
            if unsafe { GetAce(dacl, u32::from(i), &mut ace) } == 0 {
                return Err(format!("{}: unreadable ACE {i}", p.display()));
            }
            // SAFETY: every ACE starts with an ACE_HEADER.
            let h = unsafe { *(ace as *const ACE_HEADER) };
            if u32::from(h.AceFlags) & INHERIT_ONLY_ACE != 0 {
                continue; // applies to children only
            }
            match u32::from(h.AceType) {
                ACCESS_DENIED_ACE_TYPE => continue,
                ACCESS_ALLOWED_ACE_TYPE => {
                    // SAFETY: the type says ACCESS_ALLOWED_ACE.
                    let a = unsafe { &*(ace as *const ACCESS_ALLOWED_ACE) };
                    let sid = (&a.SidStart as *const u32).cast_mut().cast();
                    if a.Mask & bad != 0 && !trusted(sid, user) {
                        return Err(format!(
                            "{}: ACE {i} gives another principal {:#x}",
                            p.display(),
                            a.Mask & bad
                        ));
                    }
                }
                t => {
                    return Err(format!(
                        "{}: ACE {i} of a type not interpreted ({t})",
                        p.display()
                    ));
                }
            }
        }
        Ok(())
    }

    /// The rule: the leaf with the leaf rights, then the leaf and every ancestor up to the
    /// volume root with the rights that could replace, rename or re-ACL a path element.
    fn check_private_dir(dir: &Path, user: &[u8]) -> Result<(), String> {
        check_one(dir, LEAF_BAD | CHAIN_BAD, user)?;
        for a in dir.ancestors().skip(1) {
            check_one(a, CHAIN_BAD, user)?;
        }
        Ok(())
    }

    fn icacls(p: &Path, args: &[&str]) {
        let st = Command::new("icacls")
            .arg(p)
            .args(args)
            .stdout(Stdio::null())
            .status();
        assert!(
            st.is_ok_and(|s| s.success()),
            "icacls {} {args:?}",
            p.display()
        );
    }

    /// W6: the default root passes; a leaf other users can write, a private leaf under a
    /// parent they can rename, a junction, and a permissive entry are refused.
    pub(crate) fn w6_private() -> Outcome {
        let user = user_sid();
        let Some(lad) = dirs_cache() else {
            return Skip("no LOCALAPPDATA".into());
        };
        let mut notes = Vec::new();
        if let Err(e) = check_private_dir(&lad, &user) {
            return Fail(format!("the default root is refused: {e}"));
        }
        notes.push("LOCALAPPDATA chain passes".to_string());
        let d = tempfile::Builder::new()
            .prefix("tls117-")
            .tempdir_in(&lad)
            .unwrap();
        // A leaf others can write.
        let open = d.path().join("open");
        std::fs::create_dir(&open).unwrap();
        icacls(&open, &["/grant", "*S-1-1-0:(M)"]);
        match check_private_dir(&open, &user) {
            Err(e) => notes.push(format!("open leaf refused ({e})")),
            Ok(()) => return Fail("a leaf Everyone can modify passed".into()),
        }
        // A private leaf under a parent others can rename children of.
        let parent = d.path().join("parent");
        let leaf = parent.join("leaf");
        std::fs::create_dir_all(&leaf).unwrap();
        icacls(&parent, &["/grant", "*S-1-1-0:(DC)"]);
        match (
            check_one(&leaf, LEAF_BAD | CHAIN_BAD, &user),
            check_private_dir(&leaf, &user),
        ) {
            (Ok(()), Err(e)) => notes.push(format!("replaceable parent refused ({e})")),
            other => return Fail(format!("replaceable parent: {other:?}")),
        }
        // A junction.
        let target = d.path().join("target");
        std::fs::create_dir(&target).unwrap();
        let junction = d.path().join("junction");
        let st = Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&junction)
            .arg(&target)
            .stdout(Stdio::null())
            .status();
        if st.is_ok_and(|s| s.success()) {
            match check_private_dir(&junction, &user) {
                Err(e) => notes.push(format!("junction refused ({e})")),
                Ok(()) => return Fail("a junction passed".into()),
            }
        } else {
            notes.push("junction not made (skipped)".into());
        }
        // A permissive entry in a private folder.
        let entry = d.path().join("entry.bin");
        std::fs::write(&entry, b"x").unwrap();
        icacls(&entry, &["/grant", "*S-1-1-0:(M)"]);
        match check_one(&entry, LEAF_BAD | CHAIN_BAD, &user) {
            Err(e) => notes.push(format!("permissive entry refused ({e})")),
            Ok(()) => return Fail("an entry Everyone can modify passed".into()),
        }
        Pass(notes.join(" | "))
    }

    /// W7: deleting a file another handle holds open (sharing delete): does the name go at
    /// once (POSIX semantics), and can its folder then be removed?
    pub(crate) fn w7_delete_open() -> Outcome {
        let d = scratch();
        let sub = d.path().join("k");
        std::fs::create_dir(&sub).unwrap();
        let f = sub.join("e.bin");
        std::fs::write(&f, b"x").unwrap();
        let h = OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_DELETE)
            .open(&f)
            .unwrap();
        let del = std::fs::remove_file(&f);
        let gone = !f.exists();
        let rmdir = std::fs::remove_dir(&sub);
        drop(h);
        let note = format!(
            "delete {:?}, name gone at once {gone}, folder removed while open {:?}",
            del.as_ref().map_err(|e| e.raw_os_error()),
            rmdir.as_ref().map_err(|e| e.raw_os_error())
        );
        if del.is_ok() { Pass(note) } else { Fail(note) }
    }

    type Item = (&'static str, fn() -> Outcome);

    pub(crate) fn all() -> Vec<Item> {
        vec![
            ("W0 NTFS volumes", w0_ntfs),
            ("W1 mapped name follows a rename", w1_name_follows_rename),
            ("W1 race at each step", w1_race),
            ("W1 links, case, 8.3", w1_links),
            ("W2 GLOBALROOT open and key", w2_globalroot),
            ("W3 running image refuses write", w3_write_refused),
            ("W4 conflicting access handle", w4_conflicting_access),
            ("W5 std::fs::File locks", w5_locks),
            ("W6 private folder check", w6_private),
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
            check(w0_ntfs());
        }
        #[test]
        fn w1_name() {
            check(w1_name_follows_rename());
        }
        #[test]
        fn w1_race_each_step() {
            check(w1_race());
        }
        #[test]
        fn w1_links_case_short() {
            check(w1_links());
        }
        #[test]
        fn w2() {
            check(w2_globalroot());
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

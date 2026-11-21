# TLS-117: the IR cache key on Windows (design note)

Status: proposal for review. No product code yet.

ENG-110's IR cache (spec `docs/superpowers/specs/2026-11-18-eng110-ir-cache-design.md`) keys
its entries on a blake3 hash of the running executable's bytes (`src/program/ir_cache/key.rs`).
On Linux it reads `/proc/self/exe`, which is the file the process runs even when its path was
replaced. On Windows `image_key()` returns `None`, so the cache is off. This note decides how to
read exactly the loaded image on Windows, and what else must change before the cache can run
there.

**Scope** (same as ENG-110 and TLS-109's batch claims, `src/engine/batch/claim.rs`): protect
against accidental damage and against what this run itself does. Another process that changes
files on purpose while the run is going is out of scope.

## 1. What is off on Windows today (more than the key)

Reading the code at master `245e5c43` shows the key is not the only gap. Everything below is
`#[cfg(unix)]` today:

| Piece | File | On Windows today |
|-------|------|------------------|
| `image_key()` | `ir_cache/key.rs` | always `None` |
| `CacheDir::subdir` (the `ir/<key>/` folders) | `snapshot/cache.rs` | always `None` |
| `CacheDir::lock` | `snapshot/cache.rs` | holds nothing (`RepairLock(None)`) |
| `CacheDir::unlink`, `remove_dir` | `snapshot/cache.rs` | not compiled |
| the `private()` check (owner and "nobody else can write") | `snapshot/cache.rs` | not compiled; no check at all |
| `prune` module, `touch()` | `ir_cache/mod.rs`, `seam.rs` | not compiled / does nothing |
| `open_ir(create)` making `ir/` owner-only | `ir_cache/seam.rs` | makes nothing |

**Correction to the task's premise:** ENG-113's text cache has no Windows "private directory"
check to reuse. On Windows it trusts the location: `dirs::cache_dir()` is the user's
`%LOCALAPPDATA%`, and its comment says "the per-user cache directory, which no other user can
write". It also takes no lock: it relies on a rename over a mapped entry failing. The IR cache
adds `ALSEM_IR_CACHE_DIR`, which can point anywhere, so it needs a real check (section 4).

## 2. The key: which bytes to hash

### Facts

Sources were read on 2026-11-21. "Verified natively" means: not settled by any source we could
read, so an owner run on Windows must settle it (section 7).

| # | Fact | Source | Status |
|---|------|--------|--------|
| F1 | A file system must call `MmFlushImageSection(MmFlushForWrite)` when a file is opened for write access, and `MmFlushForDelete` before deleting it; the call returns FALSE when it cannot drop the image section (it is in use). The create or delete then fails. | [MmFlushImageSection](https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/ntifs/nf-ntifs-mmflushimagesection): "When opening a file for write access, the file system should call MmFlushImageSection from its IRP_MJ_CREATE dispatch routine, passing MmFlushForWrite." | Documented for write and delete. **Truncate is not mentioned.** Rename of a running exe being allowed is common knowledge, but no source we read states it. |
| F2 | Opening without `FILE_SHARE_WRITE` fails if anyone has the file open for write "or has a file mapping with write access". Opening without `FILE_SHARE_DELETE` fails if anyone has it open for delete access, and "Delete access allows both delete and rename operations." | [CreateFileW](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-createfilew), `dwShareMode` | Documented. So a handle we hold with `FILE_SHARE_READ` only blocks every later write, rename and delete of that file while we hold it. |
| F3 | `GetMappedFileNameW` "returns the name of the memory-mapped file" for an address. If the buffer is too small it truncates, returns `nSize` and sets `ERROR_INSUFFICIENT_BUFFER`. It is `K32GetMappedFileNameW` in kernel32 (psapi's `GetMappedFileName` is a wrapper). | [GetMappedFileNameW](https://learn.microsoft.com/en-us/windows/win32/api/psapi/nf-psapi-getmappedfilenamew) | Documented, but the page says **nothing about renames**, and does not state the name form. |
| F4 | The name is an NT device path (`\Device\HarddiskVolumeN\...`); Microsoft's own sample turns it into a drive-letter path with `QueryDosDevice`. | [Obtaining a File Name From a File Handle](https://learn.microsoft.com/en-us/windows/win32/memory/obtaining-a-file-name-from-a-file-handle) | Documented by example. |
| F5 | `\\?\GLOBALROOT` reaches the object manager's true root, and `\\?\` sends the rest of the string "straight to the file system". | [Naming Files, Paths, and Namespaces](https://learn.microsoft.com/en-us/windows/win32/fileio/naming-a-file) | Documented. That `CreateFileW` opens `\\?\GLOBALROOT\Device\HarddiskVolumeN\...` is **not stated**; verified natively. |
| F6 | `GetModuleFileNameW` returns the name "in the same format that was specified when the module was loaded"; `QueryFullProcessImageNameW` returns "the path to the executable image". Neither page says whether a rename after start shows. | [GetModuleFileNameW](https://learn.microsoft.com/en-us/windows/win32/api/libloaderapi/nf-libloaderapi-getmodulefilenamew), [QueryFullProcessImageNameW](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-queryfullprocessimagenamew) | Expected stale (the loader keeps the load-time string; `std::env::current_exe()` uses `GetModuleFileNameW`). Verified natively. |
| F7 | An image without base relocations must load at its preferred base; `IMAGE_REL_BASED_DIR64` "applies the difference to the 64-bit field at offset"; the import address table is overwritten at load with the real addresses of imported functions. | [PE Format](https://learn.microsoft.com/en-us/windows/win32/debug/pe-format) | Relocation rows read verbatim; the IAT sentence only from a summary. |
| F8 | windows-sys 0.61 (already a dependency) has everything needed: `K32GetMappedFileNameW` / `GetMappedFileNameW` (feature `Win32_System_ProcessStatus`), `GetModuleHandleExW` + `GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS` (`Win32_System_LibraryLoader`), `GetFileInformationByHandleEx(FileIdInfo)` (already used by `claim.rs`). `MemoryMappedFilenameInformation` (the `NtQueryVirtualMemory` class behind `GetMappedFileNameW`) is **not** in windows-sys. | `~/.cache/cargo/registry/src/.../windows-sys-0.61.2` | Checked in the crate source. |

### Why not hash the image in memory (refuted)

The Rust MSVC target links with dynamic base (ASLR), so the loader writes the load-address
difference into every field the `.reloc` table lists (F7); the base changes per boot. The import
address table holds the addresses of system DLL functions, which change with every boot and
every Windows update. `.data` and `.bss` change as the program runs. So the in-memory bytes are
not stable from one run to the next, and two runs of one binary would get different keys. A
version that skips relocated fields, the IAT and writable sections would be a PE parser written
only for this, and would still not cover bytes that are not mapped (the debug directory, any
overlay). Rejected.

### Why not `current_exe()` / `GetModuleFileNameW` (rejected)

It is the load-time path (F6). After a rename (an installer that moves the old exe aside and puts
a new one in place, a `self-replace`-style update) it names a different file, so the key would be
the new binary's and the cache would hand out IR the running code did not produce. This is exactly
the case the Linux test `key_is_the_running_image_not_the_path` pins. On Windows a plain rebuild
over a running exe usually fails (F1: write access is refused), so the path is often still right,
but "often" is not a key.

### No direct way to name the image's file

There is no documented user-mode call that gives a handle or a file id for the file behind our own
image section: `NtQuerySection` needs a section handle we do not have, and `MemoryImageInformation`
gives base and size only. So we cannot compare a handle's `FILE_ID_INFO` with "the image's
identity" directly. The check below proves identity through the name instead.

### Chosen design

`image_key()` on Windows:

1. **Find our module.** `GetModuleHandleExW(FROM_ADDRESS | UNCHANGED_REFCOUNT, &IR_IMAGE_MARKER)`:
   the module that holds the marker, so the code that writes and reads IR. (For an exe this is the
   same as `GetModuleHandleW(NULL)`; the marker's address stays right if the engine is ever a DLL.)
2. **Ask its current name.** `K32GetMappedFileNameW(GetCurrentProcess(), base, buf, 32768)`. A
   return of 0 or of `nSize` (truncated, F3) means no key.
3. **Open it.** `\\?\GLOBALROOT` + the device path (F4, F5), with `GENERIC_READ`, share mode
   `FILE_SHARE_READ` only (no write, no delete sharing), `FILE_FLAG_SEQUENTIAL_SCAN`. Check it is a
   regular disk file (`GetFileType` = disk, not a directory), as `claim.rs` already does. Any
   failure (including a sharing violation because someone has it open for write) means no key.
4. **Hash through that handle** with the existing `key_of` (marker search included).
5. **Ask the name again**, after the hash, and require it equal to step 2's. Different means no key.

Why this is sound, within scope: from step 3 until we close the handle, the file we opened (call
it F) cannot be renamed, deleted or written by anyone, because our handle shares neither write nor
delete (F2). So F's name is N for the whole window. At step 5 the image's file also has name N. One
name on one volume names one file at a time, so the image's file is F, and the bytes we hashed are
F's bytes, which nobody could change during the hash (F2, and F1 for the image itself). The
re-check in step 5 is what closes the race where the image is renamed away between step 2 and
step 3 and another file is put at N.

**This rests on one fact we could not source: that `GetMappedFileNameW` returns the file's
CURRENT name after a rename, not the name at load time (F3).** If it returns the load-time name,
step 5 cannot see the race and the design is wrong. The owner test in section 7 checks exactly
this, and the code must not ship until it passes.

Not covered, same as Linux: a file whose bytes change under the image section while it runs
without going through a write open (a raw disk write, a network share changed from another
machine). Both are another process's doing.

**Network volumes.** A device path `\Device\Mup\...` (an SMB share) works the same way, but the
image-section protection (F1) is enforced by the remote server's file system for other clients only
as far as SMB carries it. That is the out-of-scope "another machine changes the file" case. Keep the
cache on; list it under open questions in case the owner wants it off for non-local volumes.

### The marker on Windows

Unchanged: `key_of` already searches the bytes it hashes for `ALSEM-IR-IMAGE-1`, and it works on
any reader. Under a Windows instrumentation tool (DynamoRIO, AppVerifier) the module holding the
marker is still our exe, so the marker check matters less there than under valgrind, but it costs
nothing. Add to the Windows `release-ship` job in `build-and-deploy.yml` and `release.yml` a search
of `alsem.exe`, `aldump.exe` and `al-call-hierarchy.exe` for the marker (PowerShell
`Select-String -SimpleMatch` on the bytes, or `findstr /m /c:ALSEM-IR-IMAGE-1`).

### The "producer is self-contained" check on Windows

The spec requires that nothing outside the image changes how a text parses. The Windows build
links the C runtime statically (`.cargo/config.toml`: `+crt-static`), so the CRT is inside the
image and in the key. The Linux check already proves the source (grammar C, tree-sitter runtime,
lowerer) calls no locale or wide-character classification function, and that source is the same
on Windows. Proposal: no separate Windows import check; state in the spec that the Linux symbol
check covers the shared source, and the static CRT covers the rest. (Open question Q3.)

## 3. Locks: use `std::fs::File` locks on both systems

ENG-110 uses `flock` (`CacheDir::lock`) for: the per-entry lock while deciding to publish, the
key folder's lock (shared by users, exclusive by the prune), and `prune.lock`. Rust's
`File::lock`, `try_lock` and `try_lock_shared` are stable since 1.89 (the toolchain is 1.96) and are
`flock` on Unix and `LockFileEx` on Windows (`LOCKFILE_EXCLUSIVE_LOCK`, `LOCKFILE_FAIL_IMMEDIATELY`)
([std::fs::File](https://doc.rust-lang.org/std/fs/struct.File.html)). So: replace the `libc::flock`
call with those three methods and drop the Windows `RepairLock(None)` branch. One code path, no new
FFI.

Windows differences that matter ([LockFileEx](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-lockfileex)):
- Locks belong to a handle, like `flock` belongs to an open file. Good match.
- A lock is mandatory for its byte range: other handles cannot read or write it. Our lock files
  hold no data, so this never matters.
- After a process dies, "the time it takes for the operating system to unlock these locks depends
  upon available system resources". A killed engine's lock may linger briefly. Every IR lock is a
  `Now` kind except the publish lock, which waits; the cost is a missed hit or a skipped prune, never
  a wrong answer.

ENG-113's text cache would get real locks on Windows too, because it shares `CacheDir::lock`. That
is a behaviour change for ENG-113 (today it takes none on Windows). It is harmless (its publish
already works without them) but it should be said in the CHANGELOG. (Q4.)

## 4. Private directory check on Windows

On Unix `private()` requires the directory or file to be owned by this user and not writable by
group or others. Windows needs its own version because `ALSEM_IR_CACHE_DIR` can name any folder
(a shared `C:\cache`, a folder other users can write), and an entry there could be planted by
another user. The IR is trusted input: a planted entry with a valid hash would change analysis
results.

Proposal: `private()` on Windows, on the **opened** directory handle (opened with
`FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT`, and refused if it is a reparse
point):
- the owner SID is the current user's (`GetSecurityInfo(OWNER_SECURITY_INFORMATION)` vs the token
  user from `GetTokenInformation(TokenUser)`), and
- no allow entry in the DACL grants a write-type right (`FILE_ADD_FILE`, `FILE_ADD_SUBDIRECTORY`,
  `FILE_DELETE_CHILD`, `WRITE_DAC`, `WRITE_OWNER`, `DELETE`, `GENERIC_WRITE`, `GENERIC_ALL`) to a SID
  other than the user, `SYSTEM` or `Administrators`. (Same rule as OpenSSH for Windows uses for
  private key files; admins and SYSTEM can do anything on the machine anyway.)

`%LOCALAPPDATA%` has exactly that DACL by default, so the default path passes. The directories
the cache makes (`ir/`, `ir/<key>/`) inherit it. This needs the windows-sys features
`Win32_Security` and `Win32_Security_Authorization`.

Files in a private directory: on Unix each file is checked too. On Windows only this user (and
admins) can create files in a private directory, so a per-file check adds nothing within scope;
entries are still opened not following reparse points and refused if not a regular file, as
`open_entry` already does.

Relative opens: Unix opens every entry through the directory handle (`openat`). Windows can do
that only through `NtCreateFile`'s `RootDirectory`. Within scope it is not needed: nobody else can
change a private directory, so opening by path after checking the directory is enough, as ENG-113's
Windows code already does. `subdir()`, `unlink()` and `remove_dir()` on Windows become path-based
(`create_dir`, then the checked open; `remove_file`; `remove_dir`).

`ALSEM_IR_CACHE_DIR` on Windows: the same check, nothing special. A folder that fails it turns the
cache off with the existing "not private" warning, as on Unix.

## 5. Prune on Windows

The prune removes old key folders while holding that folder's lock (`ir/<key>.lock`, which lives
outside the folder and is never deleted). On Windows:
- IR entries are read whole and closed (`read_owned`), and `open_entry` shares delete, so a delete
  normally succeeds even while another process is reading.
- Whether a deleted file's name goes away at once (POSIX delete semantics) or only when its last
  handle closes depends on the Windows version and on `std::fs::remove_file`'s path (std's source
  has a POSIX-semantics delete with a fallback; we could not confirm which one `remove_file` takes).
  In the old case the folder is not empty yet, `remove_dir` fails, and the folder goes at the next
  prune. Best effort, as today.
- Virus scanners open new files briefly; a delete can fail with a sharing violation. Same answer:
  the next prune.
- A file in a pending delete cannot be created again under the same name until it closes. The
  publisher already treats any write failure as "no publish", so that is a miss, never a wrong hit.

No Windows-only prune code is needed beyond compiling `prune.rs` and its tests on Windows.

## 6. Publishing (rename) on Windows

ENG-113's Windows `write` (tempfile, `persist` / `persist_noclobber`) is reused as is. A `replace`
rename over an entry another process is reading can fail; that is a failed publish, which the seam
already handles as a miss.

## 7. What only a real Windows machine can settle (owner run)

| # | Question | How the test answers it |
|---|----------|-------------------------|
| W1 | Does `GetMappedFileNameW` return the current name after the image file is renamed? (The design rests on this.) | `key_mapped_name_follows_a_rename` |
| W2 | Does `CreateFileW` open `\\?\GLOBALROOT\Device\HarddiskVolumeN\...`? | every key test (no key otherwise) |
| W3 | Is the running exe refused for write open and for truncation, and allowed to be renamed? | `running_image_refuses_write_and_allows_rename` |
| W4 | Is a key read while another process holds the exe open with write sharing refused, not wrong? | covered by W3's precondition (a write open fails) |
| W5 | Does `try_lock_shared` fail while another handle holds `try_lock` (and the reverse)? | the existing `shared_now_lock_fails_while_exclusive_held`, un-gated from `unix` |
| W6 | Does the `private()` check pass `%LOCALAPPDATA%` and refuse a folder with an `Everyone: Modify` entry? | `windows_private_dir_check` |
| W7 | Prune and seam tests pass on NTFS (delete while a reader is open, folder removal). | `prune` and `seam` tests, un-gated from `unix` |
| W8 | The cache gives hits on CDO: a second `alsem` run reads IR, with no misses. | `cdo-gate --no-speed` output phases (Windows lines no longer say `unsupported`) |

Proposed tests, all in the lib (`cargo test -p al-sem --lib program::ir_cache` and
`cargo test -p al-sem --lib snapshot::cache`):

- `program::ir_cache::key::tests::key_is_the_running_image_not_the_path` made to run on Windows:
  copy the test exe, start it as a child waiting on stdin, **rename** the copy away (Windows refuses
  the delete Linux uses), write another marked file at the old path, release the child; it must
  print the original file's key. Proof of discrimination: key through `current_exe()` instead, red.
- `program::ir_cache::key::tests::key_mapped_name_follows_a_rename` (Windows only): the same child
  also prints its mapped name after the rename; it must end in the new name. This is W1 on its own,
  so a failure says which fact broke.
- `program::ir_cache::key::tests::running_image_refuses_write_and_allows_rename` (Windows only):
  with the child running, `OpenOptions::new().write(true)` on the copy fails, `set_len(0)` through
  any handle we can get fails, and `rename` succeeds.
- `snapshot::cache::tests::windows_private_dir_check` (Windows only): a temp dir under the user's
  temp folder passes; after adding an `Everyone` write ACE (`icacls <dir> /grant Everyone:M`) it
  fails.
- the `#[cfg(all(test, unix))]` gates on `prune.rs` and `seam.rs` tests become `#[cfg(test)]` where
  the test does not use Unix modes; the mode-based ones stay Unix-only and get the Windows ACL test
  above as their counterpart.

The owner runs, on Windows, with `TREE_SITTER_AL_PATH` set:
`cargo test -p al-sem --lib program::ir_cache` and `cargo test -p al-sem --lib snapshot::cache`,
then `bash scripts/cdo-gate --only output <CDO_WS>` for W8 (an `--only` run never runs the
Linux-only speed phase).

## 8. Open questions for the orchestrator

- **Q1.** If W1 fails (the mapped name is the load-time name), the fallback is weaker: hash the
  file at the mapped name with the same deny-write, deny-delete handle, and accept the rename race
  as "another process's doing". Is that acceptable, or should the cache then stay off on Windows?
- **Q2.** Network volumes (`\Device\Mup\...`): keep the cache on (current proposal) or require a
  local volume?
- **Q3.** Is the Linux locale-symbol check plus the static CRT enough for "self-contained" on
  Windows, or do you want an import-table allow list in the Windows CI job (`dumpbin /imports`)?
- **Q4.** Moving `CacheDir::lock` to `std::fs::File` locks gives ENG-113 real locks on Windows too.
  Fine to include in TLS-117?
- **Q5.** Should the new Windows `private()` also guard ENG-113's text cache directory? It has no
  override variable, so `%LOCALAPPDATA%` is trusted today; adding the check costs one ACL read per
  open and turns that cache off if a user's profile ACLs are unusual.

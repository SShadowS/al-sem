# TLS-117: the IR cache on Windows (design note)

**Product status (rev 10, branch `tools/tls-117-windows-cache`).** The product phase is built as
this note's sections 3-7 describe, with these as-built names: the canonicaliser and the slot code
are `src/program/ir_cache/pe.rs`; the stamp front end is `src/program/ir_cache/stamp.rs` and the
binary `alsem-stamp` (`stamp <exe> [<out>]`, `verify <exe>`), not the probe's example tool; the
slot static `IR_DATA_SLOT` is in `src/program/ir_cache/key.rs` next to the IR marker, with the
product magic `ALSEM-IR-SLOT-v1` (the fixture, `tests/fixtures/tls117/`, keeps the test-only
`TLS117-FIXT-SLOT`); the Windows key is `key::win`; the private walk, handle check and locality
rule are `src/snapshot/winfs.rs`; an unstamped binary's report line is `"ir_cache": "unstamped"`
(passed by `scripts/ir-cache-report-check`), any other failure `"no key"` with its reason;
`scripts/install-windows.ps1` and both release workflows stamp.

**Rev 11 (product review r1, sol and Fable).** On Windows the cache is **off by default**: only
an explicit `ALSEM_IR_CACHE=on` turns it on (`ir_cache::DEFAULT_ON`); one later commit flips the
default once the product acceptance (section 3) passes, and every acceptance step sets
`ALSEM_IR_CACHE=on` itself. The Windows key is computed **eagerly**, in `install_from_env`: the
first statement of `aldump`'s and `al-call-hierarchy`'s `main`, and in `alsem`'s `main` after the
perf-trace setup (skipped for commands that never use the cache: help and version, the `mcp`
front end, `engine-ask`; `batch` uses it in the parent and every worker, TLS-116, so it keys). The candidate's handle denies write and delete while its bytes are
read and is closed after the check (Fable N1: nothing reads the file again, and a handle held for
the process's life would stop an installer renaming the running exe away); section 1 states what
the few milliseconds before the key leave. On Windows the cache roots are created through the private walk
(`winfs::walk(.., true)`): the IR root, ENG-113's text root and the batch guard's root. A run
that makes no key fails `scripts/ir-cache-report-check --require-key`, which the acceptance and
both release workflows use (the release workflows run the shipped, signed `alsem.exe` once on
`tests/fixtures/tls117/ws`). The Windows-native tests are listed in section 6 and in the
CHANGELOG's TLS-117 entry: `cargo test -p al-sem --lib -- winfs key::tests::windows
snapshot::cache prune`.

Status (probe phase): **rev 6**, after sol's review r4 (`/work/.al-sem/tools-notes/tls117-design-review-sol-r4.md`):
the coordinator made the post-link **data-digest slot mandatory**; the orchestrator agreed on the
condition that the owner's local Windows install path stamps the binaries (section 3, "Building
and installing stamped binaries"). No product code yet; the native probe (`examples/tls117_probe.rs`),
the stamp tool (`examples/tls117_stamp.rs`) and the shared PE code (`examples/tls117/pe.rs`) are the
only code. **The cache stays off on Windows until the probe passes on a real machine** (section 9),
and a green probe run is diagnostics, not enablement: the product phase must also give positive
keys on the shipped, stamped MSVC binaries.

What rev 9 changes (the owner's rerun at `84e9f24a`, `/work/.al-sem/orchestrator/tls117-windows-84e9f24a/`):
W12 and W10 (now also on Windows PowerShell 5.1) PASSed; the only FAIL was W1 POSIX rename,
where the image kept its name and id and the key was the running image's, but the **control
failed too**, and the sources turned up in the checkout as `a.exe` and `ctl-target.exe`. Root
cause: the probe's rename helper passed a **bare file name** with a NULL `RootDirectory`. The
[FILE_RENAME_INFO](https://learn.microsoft.com/en-us/windows/win32/api/winbase/ns-winbase-file_rename_info)
documentation gives `RootDirectory` as NULL "if the file is not moved to a different directory,
or if the FileName member contains the full path", and as a directory handle otherwise; the runs
show the Win32 layer resolving a bare name against the **process's current directory**, so the
"same-folder" rename moved the source into the checkout (and the image's name was never the
target). The rest of the call was right (`FileRenameInfoEx` = 22; the `Flags` union with
`FILE_RENAME_FLAG_REPLACE_IF_EXISTS | FILE_RENAME_FLAG_POSIX_SEMANTICS`; `FileNameLength` in bytes
without the terminator). Rev 9 passes the **full path** of the target (and refuses a relative
one). The control must replace a non-running file, verified by file id, before the running case
counts; a failed control FAILs the item with its exact error. **The probe writes only under the
temp folder** (`%TEMP%`, through `tempfile`), sets its own current directory to the temp folder
at start, and starts every child there too; it never writes into the checkout or the directory it
was started from. Stray `a.exe` / `ctl-target.exe` in the checkout root from the earlier runs are
probe leftovers, safe to delete.

What rev 8 changed (the owner's run at `9e8bf18f`, `/work/.al-sem/orchestrator/tls117-windows-9e8bf18f/`):
- **The run.** Debug and `release-ship` probes: every item PASSed except W1 POSIX rename replace.
  Every rival was refused at the canonical comparison (rename away, ancestor folder, hard link,
  W9's `.text`/`.rdata`), every key was the running image's or absent, W9's `.data` change was
  refused by the slot, W11's unstamped copy got no key, the stamp verified. W10 PASSed (variant
  and relink, linker ran) under pwsh 7. The test build (`cargo test --example`) failed 13 items at
  "stamp the copy: the slot magic occurs more than once".
- **W1 POSIX, analysed.** At all three barriers: the `FileRenameInfoEx` call reported success, the
  running image's name `a.exe` still opened the **original file id** (verified), and the source
  `other-N.exe` was **gone from its name** (os error 2). So the replacement did not land at
  `a.exe`; the image kept its name and id. The probe's verdict code returned "did not take" before
  judging the key, so the run printed no key verdict for these three runs. A wrong key was not
  possible there: every candidate opens `a.exe`, which is the original file (by id); and a rival,
  wherever it went, is refused by content (W9 and the other W1 items show it). This was a
  classification issue in the probe: rev 8 reports it as HELD (the OS kept the running image)
  when the control replaced a non-running target, says **where the source went** (scanning the
  image's folder, the scratch folder and the current directory for its file id), always prints
  the key verdict even when classification fails, and checks every run's candidates by identity.
  One open question the next run answers: where the source went. If the control also fails and
  the source turns up in the current directory, the bare-name rename was resolved against the
  process's current directory, not the source's folder (the owner may find stray `a.exe` or
  `ctl-target.exe` files in the checkout root from the 9e8bf18f run; they are probe leftovers,
  safe to delete).
- **The test-build magic bug.** The test binary carried the probe's magic twice: the embedded
  fixture (`include_bytes!`) and the test constant both spelled `TLS117-DATA-SLOT`. Both now use a
  test-only magic (`TLS117-FIXT-SLOT`, fixture rebuilt; layout unchanged, checksum now 0x416a). New
  item **W12** checks, in every build, that the binary holds the magic exactly once;
  `examples/tls117/check-magic.sh` cross-builds the example and its test build and counts. Run
  here at rev 8 (`x86_64-pc-windows-gnu`, debug): the example **1**, its test build **1**.
- **W10** hashes through .NET (works in Windows PowerShell 5.1 and pwsh 7). **§9** says the
  grammar clone must be at the commit CLAUDE.md names.

What rev 7 changed (sol r5): the slot's digest also binds the initial file bytes of every range
canonicalisation masks (section 3, "The digest"), so nothing the canonical hash skips is unbound;
the probe refuses a key reply naming no candidate, requires an observed rival refused at the
canonical comparison in the stale-name reproductions (folder swap, deleted launch link), and marks
barriers the child only fakes as `synthetic`, never crediting one as a race; and section 3 gains
**Product acceptance before enablement**: positive keys on the shipped, stamped MSVC executables
and W8 through the installed stamped binaries, with the exact owner steps.

What rev 6 changed: the **data-digest slot** (section 3): a 48-byte marked static in `.rdata`
(16-byte magic, 32 digest bytes, zero at build), stamped after linking by `tls117_stamp` with the
blake3 of every writable section; at key time the loaded slot (covered by part A) must be
non-zero, the candidate's writable sections must hash to it, and the candidate's own slot must
equal it. Initial mutable data is no longer outside the guarantee. `in_hashed` no longer means
"hashed": metadata a mask zeroes refuses, the relocation table must lie in hashed, unmasked bytes,
and a writable and executable section refuses. The probe stamps the copies it launches; W2 and W9
run in a stamped child; W11 (new) shows an unstamped copy gets no key; every POSIX HELD path needs
the control; `after-open` protection is credited only after a successful open; each W1 candidate
is judged by identity first; W9's missing-section case must be refused at the headers; W10 stays a
diagnostic, now comparing stamped slots and checking the linker ran.

What rev 5 changed: part A reads the module through `ReadProcessMemory`; headers are found by
`e_lfanew`/`SizeOfHeaders` and compared with the module's before any section work; checked
arithmetic and size bounds throughout; imports without a lookup table refuse; masks never touch
executable sections nor come from skipped sections; the load config refuses more unmodelled forms
and masks `CastGuardOsDeterminedFailureMode`; ARM64 gets no key (section 3). The probe's W1
replacements are rival valid PEs that both candidates must refuse at the canonical comparison;
POSIX HELD needs a control that replaces a non-running target; W9 is mandatory and runs through the
full candidate path; W2 is also run on a `release-ship` build; **W10** (new) checks whether the
shipped profile makes a `.data`-only change visible, with a post-link `.data`-hash slot as the
fallback (sections 1, 8, 9). The scope lists never-mapped bytes.

What rev 4 changed: the owner run showed that `GetMappedFileNameW` keeps the old path after an
ancestor folder is renamed away and recreated, and after the launch hard link is deleted, so the
rev 1-3 key (trust the mapped name, re-check it) hashed the wrong file: two WRONG KEYs. Design B
(section 3) takes identity from what runs: the canonical hash of the loaded module's read-only
content (part A), and a candidate file is accepted only if the same canonical hash of its bytes
matches (part B); the key is then the accepted file's full blake3. The scope (section 1) now
names what B cannot see: initial mutable data. The probe reports A, B's verdict and whether the
key is the running image's for every W1 case, gains W9 (altered candidates), and tells a POSIX
rename that moved nothing (HELD) from one that did not take. The canonicaliser has unit tests that
run on Linux against a small checked-in PE (`examples/tls117/fixture.exe`, source beside it).

What rev 3 changed (sol r2): locality is a fixed drive AND NTFS, ReFS stays off (section 3); the
producer audit is at the C source level, with the reachability list (section 4); the private check
walks top-down from the volume root, creates only under a checked parent, and all cache I/O uses
the walk's rooted spelling (section 6); the probe fails a refused or unverified mutation, verifies
every replacement by file id before the barrier is released, adds explicit POSIX rename-replace,
MoveFileEx replace and an ancestor-folder replacement, and fixes the P2 gaps (section 8); W8 starts
from a fresh impact report (section 9).

What rev 2 changed: the scope is written out (section 1); the key requires local NTFS and compares
raw UTF-16 (section 3); a producer audit replaces "the static CRT is enough" (section 4; it found
locale-sensitive imports in the tree-sitter runtime); locks move to `std::fs::File` on both
systems (section 5); the private check covers the whole path, each opened entry and lock file, and
ENG-113's text folder too (section 6); the prune's temp-name pattern is fixed (section 7); the
probe is written (section 8) and the run order is fixed (section 9).

ENG-110's IR cache (spec `docs/superpowers/specs/2026-11-18-eng110-ir-cache-design.md`) keys its
entries on a blake3 hash of the running executable's bytes (`src/program/ir_cache/key.rs`). On
Linux it reads `/proc/self/exe`, the file the process runs even when its path was replaced. On
Windows `image_key()` returns `None`, so the cache is off.

## 1. Scope

The same scope as ENG-110 and TLS-109's batch claims (`src/engine/batch/claim.rs`): protect
against accidental damage and against what this run itself does, not against another process
changing files on purpose while the run is going.

**In scope — must give the running image's key or no key:**
- an installer or updater that renames the running exe away and puts a new file at its path;
- a new file renamed over the running exe's path (replace-if-exists, including POSIX rename
  semantics);
- a launch through a hard link, and the launch link deleted or replaced while another link to
  the image survives;
- a launch through a path in another letter case, or through its 8.3 short name;
- someone holding the image open with an access that conflicts (write or delete access).

- an ancestor folder of the image renamed away and a new folder made at its old spelling (the
  owner run showed the mapped name does not follow it; design B gives no key there).

**The guarantee's scope (design B with the slot, rev 6).** A key is the full blake3 of a file
whose read-only content (every section without `IMAGE_SCN_MEM_WRITE`, after relocation normalising
and with the loader-written ranges zeroed, plus the headers with `ImageBase` zeroed) equals the
running module's, **and** whose writable sections (raw bytes, with each section's name, virtual
address and sizes) hash to the digest stamped in the running module's slot. The slot lies in
`.rdata`, so part A covers the digest the running image carries; the candidate's writable sections
are recomputed, never trusted from its own slot. So initial mutable data (`.data`, `.tls`) is
covered: a rebuild that changes only a mutable static's initial value gives a different digest
and no key. An **unstamped** binary (every dev build) has an all-zero slot and gets no key.
What stays unseen:
- the bytes inside the loader-written ranges (IAT, delay-load IAT and module handle, TLS index,
  load-config pointer targets), which are zeroed on both sides of the canonical comparison (the
  slot digest covers them where they lie in writable sections, as raw file bytes);
- **bytes the loader never maps**: anything past the last section's raw data (an overlay, the
  Authenticode signature, which the security directory points to by file offset), and the file
  padding between `SizeOfHeaders` and the first section and between sections' raw data. The key
  (the full-file blake3) covers them; the identity check does not. So the key is a key **over
  the accepted file's bytes**, not an exact identity of the running file: two files that differ
  only there (an overlay, a different signature) run the same code but get different keys. That
  costs cache misses, never a wrong answer.

**The window before the key (rev 11, product review r1).** The key is computed at start (in
`install_from_env`: aldump's and al-call-hierarchy's first statement, alsem's after its
perf-trace setup), from bytes read through a handle that denies write and delete while it reads.
The key is then fixed for the process; the file is not held afterwards (an installer may rename
the running exe away, which changes nothing the process uses). What remains is the few
milliseconds between the launch and the key, and one case in it: a binary **damaged after it was stamped** in its writable
sections (so its own slot no longer matches them), launched damaged, and replaced at its path by
a pristine copy within those milliseconds. The pristine copy's read-only content equals the
running image's and its writable sections match the slot the running image carries, so it is
accepted and the key names the pristine file while the damaged initial data runs (the writable
sections are checked in the candidate file, never in memory; the reason is under "Why `.data` is
not compared in memory"). Damage in the read-only content gives no key. This case is in the
guarantee's scope as stated.

**Out of scope:** another process changing files on purpose mid-run (ENG-110's spec), raw disk
writes, a network peer; the local-NTFS rule (section 3) keeps network volumes out entirely.
Code patched in memory after load (an inline hook by security software) makes A differ from every
file: no key, fail closed.

## 2. What is off on Windows today (more than the key)

At master `245e5c43` all of these are `#[cfg(unix)]`:

| Piece | File | On Windows today |
|-------|------|------------------|
| `image_key()` | `ir_cache/key.rs` | always `None` |
| `CacheDir::subdir` (the `ir/<key>/` folders) | `snapshot/cache.rs` | always `None` |
| `CacheDir::lock` | `snapshot/cache.rs` | holds nothing (`RepairLock(None)`) |
| `CacheDir::unlink`, `remove_dir` | `snapshot/cache.rs` | not compiled |
| the `private()` check | `snapshot/cache.rs` | not compiled; no check at all |
| `prune` module, `touch()` | `ir_cache/mod.rs`, `seam.rs` | not compiled / does nothing |
| `open_ir(create)` making `ir/` owner-only | `ir_cache/seam.rs` | makes nothing |

ENG-113's text cache has no Windows private check to reuse: it trusts `%LOCALAPPDATA%` and takes
no lock (it relies on a rename over a mapped entry failing).

## 3. The key

### Facts

Sources read on 2026-11-21. "Probe" means no source settles it and the native probe does.

| # | Fact | Source | Status |
|---|------|--------|--------|
| F1 | A file system must call `MmFlushImageSection(MmFlushForWrite)` when a file is opened for write, and `MmFlushForDelete` before deleting it; it returns FALSE while the image section is in use, and the open or delete fails. | [MmFlushImageSection](https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/ntifs/nf-ntifs-mmflushimagesection) | Write and delete documented; truncate and rename not mentioned. Probe W3. |
| F2 | An open without `FILE_SHARE_WRITE` fails if the file is open for write "or has a file mapping with write access"; without `FILE_SHARE_DELETE` it fails if the file is open for delete access; "Delete access allows both delete and rename operations." | [CreateFileW](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-createfilew) | Documented. Probe W4 checks it with real conflicting handles. |
| F3 | `GetMappedFileNameW` returns the mapped file's name for an address; a too-small buffer truncates, returns `nSize` and sets `ERROR_INSUFFICIENT_BUFFER`. It is `K32GetMappedFileNameW` in kernel32. | [GetMappedFileNameW](https://learn.microsoft.com/en-us/windows/win32/api/psapi/nf-psapi-getmappedfilenamew) | Documented. **Whether a rename shows is not documented.** Probe W1. |
| F4 | The name is an NT device path (`\Device\HarddiskVolumeN\...`). | [Obtaining a File Name From a File Handle](https://learn.microsoft.com/en-us/windows/win32/memory/obtaining-a-file-name-from-a-file-handle) | Documented by example. |
| F5 | `\\?\GLOBALROOT` reaches the object manager's root, and `\\?\` sends the rest "straight to the file system". | [Naming Files, Paths, and Namespaces](https://learn.microsoft.com/en-us/windows/win32/fileio/naming-a-file) | Documented; that `CreateFileW` opens such a name is not. Probe W2. |
| F6 | `GetModuleFileNameW` (and so `std::env::current_exe()`) returns the name "in the same format that was specified when the module was loaded". | [GetModuleFileNameW](https://learn.microsoft.com/en-us/windows/win32/api/libloaderapi/nf-libloaderapi-getmodulefilenamew) | Load-time name; not used. |
| F7 | An image with relocations is moved to its load address by applying the base difference to every listed field (`IMAGE_REL_BASED_DIR64`); the import table is filled at load. | [PE Format](https://learn.microsoft.com/en-us/windows/win32/debug/pe-format) | Documented. |
| F8 | windows-sys 0.61 has `K32GetMappedFileNameW` (`Win32_System_ProcessStatus`), `GetModuleHandleExW` (`Win32_System_LibraryLoader`), `GetVolumeInformationByHandleW`, `GetFileInformationByHandleEx`, `GetSecurityInfo` and `ConvertSidToStringSidW` (`Win32_Security_Authorization`), `GetAce` (`Win32_Security`), `GetDriveTypeW`, `GetFinalPathNameByHandleW`, `MoveFileExW`, `SetFileInformationByHandle` + `FILE_RENAME_INFO` (`Win32_Storage_FileSystem`). | the crate source in the cargo registry | Checked; the probe compiles against them. |

**What the owner run at `e98e1bf0` established** (F3, F5 settled natively): the mapped name
follows a rename of the image file itself, and `\\?\GLOBALROOT` + the device name opens it; but
the mapped name **keeps the old path** after an ancestor folder is renamed away and recreated, and
after the launch hard link is deleted. Trusting a name, even re-checked, is therefore unsound
(two WRONG KEYs). MoveFileEx replace of the running image is refused by the OS (access denied);
the POSIX rename-replace reported success and the image kept its name (rev 4's probe tells whether
anything moved at all).

### Rejected alternatives

- **The rev 1-3 design** (hash the file at the mapped name, re-query the name): refuted by the
  owner run, above.
- **Hash the image in memory as the key.** Not stable: relocations (F7) change with the load
  address, the import table holds per-boot DLL addresses, and `.data` changes as the program runs.
  Design B uses a *canonical* memory hash for identity only, never as the key.
- **`current_exe()` / `GetModuleFileNameW` alone.** The load-time name (F6). Design B uses it only
  as a second candidate, accepted by content, never by name.
- **A file id for the image itself.** No documented user-mode call gives a handle or file id for
  the file behind our own image section.

### Design B

`image_key()` on Windows:

1. **The module.** `GetModuleHandleExW(FROM_ADDRESS | UNCHANGED_REFCOUNT, &IR_IMAGE_MARKER)`.
2. **Part A, the canonical memory hash.** Read the module with `ReadProcessMemory` on our own
   process (an unreadable page is a failed read, so no key, never a fault): the headers by
   `e_lfanew` and `SizeOfHeaders`, then `pe::canonical` (below) over the loaded image, relative to
   the module's load address. Any parse error, unhandled relocation type or unmodelled loader
   patching: no key.
3. **Candidates**, in order: the mapped name (`K32GetMappedFileNameW`, opened as `\\?\GLOBALROOT` +
   the device name), then `GetModuleFileNameW`. A truncated name is skipped.
4. **Part B, for each candidate:** open it with read access, sharing **read only** (no write, no
   delete), not following a reparse point; require a regular file on a **local NTFS** volume
   (`GetVolumeInformationByHandleW` says `NTFS` and `GetDriveTypeW` on the handle's
   `\\?\Volume{GUID}\` root, from `GetFinalPathNameByHandleW(.., VOLUME_NAME_GUID)`, is
   `DRIVE_FIXED`; ReFS stays off); read the whole file through that handle (at most
   2 × `MAX_IMAGE`); compare its header block, `ImageBase` zeroed, with the module's **before any
   section work**, so every size used afterwards is the running image's own; then compute
   `pe::canonical` over the file's bytes laid out by section, relative to the file's `ImageBase`.
5. **The slot.** Read the running module's slot from memory (its address is the static's; it must
   lie in a non-writable section, outside every mask, so part A covers it). An all-zero digest
   (an unstamped build) gives no key before any candidate is opened. For a candidate whose
   canonical hash equals A's: recompute the digest of **its** writable sections and require it to
   equal the loaded digest, and require its own slot bytes to equal the loaded slot.
6. **Accept** the first candidate that passes both. The key is the blake3 of the bytes read
   through that same handle (and the IR marker search over them, as on Linux). No candidate
   accepted: no key. **As built (rev 11):** the key is computed at start (`install_from_env`;
   see the rev 11 header), a candidate whose open meets a sharing violation is tried once more
   after 50 ms, and the handle is closed after the check, so the running exe can be renamed away
   after the key (`key::tests::windows_stamped_copy_keys_on_its_image` checks it).

**Why it holds.** The accepted file's read-only content equals what is running, by construction,
whatever names point where, and its writable sections hash to the digest the running image
carries in its (hashed) slot; the handle denies write and delete sharing while the bytes are read,
so the key is of exactly those bytes. A replaced name, a swapped folder or a deleted link can only
make the candidates differ from A or from the slot, which gives no key. What stays unseen is the
scope in section 1.

### The data-digest slot (rev 6, mandatory)

- **The static.** `TLS117_SLOT: [u8; 48]` (in the product: next to `IR_IMAGE_MARKER`): the 16-byte
  magic `TLS117-DATA-SLOT`, then 32 zero bytes. Immutable, so the linker puts it in `.rdata`;
  `#[used]` and every read through `std::hint::black_box` (the IR marker's pattern) keep it, and
  keep the optimiser from assuming its build-time zeros. The magic is spelled only in that static's
  initialiser (and in the stamp tool, which is never stamped); code that needs it reads it back
  from the static, so a stamped binary carries the magic exactly once.
- **The digest** (rev 7: `tls117-data-digest-2`). blake3 over the tag, then for every writable
  section in table order: its 8-byte name, virtual address, virtual size and raw size (u32 LE),
  then its raw bytes (`[PointerToRawData, +SizeOfRawData)`); then the separator `masked ranges`,
  then **every range canonicalisation masks** (the file's loader-written ranges: the IAT and every
  `FirstThunk` array, the delay-load IATs and module handles, the TLS index target, the load-config
  pointer targets), sorted by RVA then length, duplicates dropped, each as the record (holding
  section's 8-byte name, or 8 zero bytes in the headers; RVA u32 LE; length u32 LE; the range's
  initial file bytes, zero past the raw data as the loader maps them). Raw file bytes, never
  memory: by key time the process has written its writable sections and the loader its masked
  ranges. So every byte the canonical hash cannot see is bound to the slot: a delay-load IAT
  whose initial entries point at a linker thunk, an IAT's initial hint/name RVAs, a security
  cookie's initial value. At key time the same digest is recomputed from the candidate file (its
  own masks) and compared with the loaded slot. The stamp refuses a slot that overlaps a masked
  range.
- **The stamp tool** (`examples/tls117_stamp.rs`, runs on Linux and Windows on any x64 PE):
  finds the magic, refusing unless it occurs exactly once and lies in a read-only, non-executable
  section's mapped raw data; writes the digest into the 32 bytes after it; recomputes the PE
  checksum when the file carries a non-zero one (the `CheckSumMappedFile` algorithm; the loader
  checks it only for drivers and boot images, and lld/MSVC executables usually carry 0, which is
  left 0); then verifies that no byte outside the 32 digest bytes and the 4 checksum bytes changed
  and that the writable sections are unchanged. Stamping is idempotent. Stamp **before**
  Authenticode signing (signing appends the certificate and rewrites the checksum, but changes no
  section and not the slot). The stamping code is `pe::stamp` in `examples/tls117/pe.rs`, shared
  by the tool and the probe; the product phase moves that file into the crate unchanged in
  substance, so the tool the product uses is this code.
- **Why stamping keeps A and B consistent:** the stamp changes `.rdata` (and maybe the checksum in
  the headers), so a stamped file's canonical hash differs from the unstamped one's; part A of a
  process started from the stamped file sees the stamped bytes, and part B reads the same stamped
  file. Only stamped files can match a stamped process.

### Building and installing stamped binaries (product phase; plan)

Unstamped builds (every `cargo build`, `cargo run`, test or bench) get **no key**: the cache is
simply off for them on Windows, and that is fine. Only binaries that went through the install path
or CI are stamped.

- **The owner's local install path** — one script, `scripts/install-windows.ps1` (PowerShell, the
  owner's shell for native Windows work), documented next to TLS-121's Windows build notes
  (TLS-121 is adding a Windows build section to the docs and to CLAUDE.md's Prerequisites: the
  libz-ng-sys `CMAKE_GENERATOR='Visual Studio 17 2022'` workaround and a short `CARGO_TARGET_DIR`
  against MSBuild's MAX_PATH file tracking; the install script inherits both). It:
  1. builds `alsem`, `aldump` and `al-call-hierarchy` with `cargo build --profile release-ship`
     (the three binaries that install the IR cache);
  2. stamps each with the stamp tool (in the product, a small bin over the same `pe` code), and
     stops on any stamp error;
  3. checks each stamped file: the slot verifies (digest recomputed equals the slot) and the IR
     marker is present;
  4. copies them to `~/.al-sem/bin/` (`alsem.exe`, `aldump.exe`) and to each LSP package location
     given (`-LspDir <dir>`, repeatable: the `claude-code-lsps/al-language-server-go*/bin/`
     folders, or wherever the owner's installed plugin keeps `al-call-hierarchy.exe` and
     `alsem.exe`), never over a running binary (the copy fails while it runs; the script says so).
- **CI** (`build-and-deploy.yml`, `release.yml`, the Windows `release-ship` job): stamps the three
  binaries right after the build and before signing, then runs the same verification, so the
  binaries copied into the LSP packages are stamped.
- **Linux** is unchanged: its key is `/proc/self/exe`'s bytes, and no stamp is needed there (the
  static exists on every target; an unstamped slot means nothing on Linux).

### Product acceptance before enablement (rev 7, sol r5)

A green probe run is diagnostics. The cache is turned on for Windows only after, on the owner's
machine, with the product change built:

**Before you start** (PowerShell, repository root, branch `tools/tls-117-windows-cache` checked out):
```
$env:TREE_SITTER_AL_PATH = '<your tree-sitter-al clone>'
git -C $env:TREE_SITTER_AL_PATH rev-parse HEAD   # must be 88bb6871… (the commit CLAUDE.md names)
$env:CMAKE_GENERATOR     = 'Visual Studio 17 2022'
$env:CARGO_TARGET_DIR    = 'C:/atw'      # any short path; the default is too long (MAX_PATH)
# Step 4's tests first (section 6's list; they need no install):
cargo test -p al-sem --lib -- winfs key::tests::windows snapshot::cache prune
# Then build, stamp and install the three binaries (also into each LSP folder you use):
powershell -ExecutionPolicy Bypass -File scripts\install-windows.ps1 [-LspDir <folder> ...]
```
**What PASS looks like, in one place:** every `cargo test` line ends `ok` (0 failed); the install
script ends with `install-windows: done (stamped and verified)`; each
`ir-cache-report-check` in step 2 prints `PASS` and exits 0; the `Select-String` line prints
nothing; the three answers equal their `ALSEM_IR_CACHE=off` runs (`fc /b` reports no differences);
the LSP check in step 3 prints `PASS`. Send back every output, and the two medians of step 5.

1. **Positive keys on the shipped executables.** The MSVC `release-ship` `alsem.exe`, `aldump.exe`
   and `al-call-hierarchy.exe`, stamped by `scripts/install-windows.ps1` (and, separately, the ones
   the CI step stamped), each make a key: the IR cache report's lines say a key, never `no key`.
2. **W8 through the installed, stamped binaries**, not cargo-built test artifacts (`cdo-gate` and
   the cargo tests run `CARGO_BIN_EXE_*` builds, which are unstamped and so get no key): cold
   publication, warm disk hits, impact with every hit verified. Exact owner steps (PowerShell,
   after `scripts/install-windows.ps1` installed to `~/.al-sem/bin/`):
   ```
   $env:CDO_WS = 'U:\Git\DO-cdo-baseline\Cloud'
   $bin = "$HOME\.al-sem\bin\alsem.exe"
   $env:ALSEM_IR_CACHE = 'on'
   $env:ALSEM_IR_CACHE_DIR = "$env:LOCALAPPDATA\al-ch-snapshot-cache\ir-accept-$(Get-Date -Format yyyyMMddHHmmss)"
   $r = "$env:TEMP\tls117-accept"; New-Item -ItemType Directory -Force $r | Out-Null
   Remove-Item "$r\*.jsonl" -ErrorAction SilentlyContinue
   # a) cold: a fresh cache root; this process publishes
   $env:ALSEM_IR_CACHE_REPORT = "$r\cold.jsonl"; & $bin analyze $env:CDO_WS > "$r\cold.out"
   # b) warm: a second process reads what (a) published
   $env:ALSEM_IR_CACHE_REPORT = "$r\warm.jsonl"; & $bin analyze $env:CDO_WS > "$r\warm.out"
   # c) impact, every hit parsed again and compared
   $env:ALSEM_IR_CACHE_VERIFY = '1'; $env:ALSEM_IR_CACHE_REPORT = "$r\impact.jsonl"
   & $bin impact $env:CDO_WS --routine 'CDO Telemetry::UpdateLastUpload' --format json > "$r\impact.out"
   Remove-Item Env:ALSEM_IR_CACHE_VERIFY
   python scripts\ir-cache-report-check --require-key --rule output "$r\cold.jsonl"
   python scripts\ir-cache-report-check --require-key --rule warm-build "$r\warm.jsonl"
   python scripts\ir-cache-report-check --require-key --rule impact "$r\impact.jsonl"
   Select-String -Path "$r\*.jsonl" -Pattern 'no key|unsupported|unstamped'
   ```
   Pass when: `cold.jsonl` has `published_packages > 0` (and passes `--rule output`), `warm.jsonl`
   passes `--rule warm-build` (`disk_hits > 0`), `impact.jsonl` passes `--rule impact`
   (`verified_hits > 0`), every check passes `--require-key` (an `unstamped`, `unsupported` or
   `no key` line fails it; without the flag the checker passes an all-`unstamped` file, which is
   right for the cargo-built test binaries and wrong here), no report has `refused` or
   `roundtrip_failed`, and the last line finds nothing. The three outputs must equal the same commands run with
   `ALSEM_IR_CACHE=off` (the cache never changes an answer).
3. The same as step 2 with `aldump.exe` is not needed; `al-call-hierarchy.exe` gets one positive
   key check (start the LSP server once with `ALSEM_IR_CACHE=on` and `ALSEM_IR_CACHE_REPORT` set,
   open the workspace, then `ir-cache-report-check --require-key <report>`).
4. The Windows-native tests (section 6's list) pass.
5. **The eager key's start-up cost** (Fable N7): every process with the cache on now reads and
   hashes its own exe in `main`. Measure it on the installed `alsem.exe` with a command that
   starts, makes its key and does little else, cache on against off, and report both medians:
   ```
   $ws = 'tests\fixtures\tls117\ws'
   foreach ($mode in 'off', 'on') {
     $env:ALSEM_IR_CACHE = $mode
     $ms = 1..7 | ForEach-Object { (Measure-Command { & $bin analyze $ws --format json | Out-Null }).TotalMilliseconds }
     "$mode median ms: $(($ms | Sort-Object)[3])"
   }
   ```
   The difference is the key's cost per process start (expected: tens of milliseconds for a
   ~30 MB exe; a resident server pays it once).
6. **The release workflow on the branch** (Fable N6), before landing: `gh workflow run
   release.yml --ref tools/tls-117-windows-cache`. It builds, stamps, (signs when configured),
   runs the cold, warm and aldump key checks, the static-CRT check and the map listing, and
   publishes nothing (the `release` job runs only for a tag). Before running it (Fable F3):
   - `build-windows` runs in `environment: release` and, when signing is configured, **signs
     branch code with the real certificate**;
   - it fails if the `release` environment restricts branches (allow this branch once, or run
     it after landing);
   - `--ref <tag>` would publish a release: use the branch, never a tag;
   - `gh workflow run` may refuse a ref until the `workflow_dispatch` trigger is on `master`;
     then the dispatch run waits until after landing, which is safe because the Windows cache
     is still off by default there.

**Why `.data` is not compared in memory.** The engine computes the key in `main` (rev 11; it was
lazy, at the first IR cache use, before), still after the runtimes have started: the C runtime, Rust's runtime, mimalloc, `std`'s statics and the program itself
have written writable sections by then, and there is no way to know which bytes. The one earlier
point we could control is a TLS callback (`.CRT$XL*`), which runs after the loader (relocations,
`__security_cookie`, `_tls_index`) but before the C runtime's initialisers. Capturing `.data` there
would mean a copy at every process start of every binary that installs the cache, kept for the
process's life, and it would still not be reliable: a statically imported DLL's `DllMain` (or
injected security software) runs before the exe's TLS callbacks and may write the exe's memory; the
order among `.CRT$XL*` callbacks is by section name only. So the writable sections are bound
through the stamped digest instead: computed from the file at build time, carried in hashed
read-only bytes, and recomputed from the candidate file at key time.

### The canonical hash (`examples/tls117/pe.rs`)

Input: a view of the image laid out as the loader maps it (memory: the module; file: headers at
RVA 0, each section's raw data at its `VirtualAddress`, zero past `SizeOfRawData` up to
`VirtualSize`) and a `base` (memory: the load address; file: `OptionalHeader.ImageBase`).

- **Headers.** PE32+ x64 only (`Machine` 0x8664, optional-header magic 0x20b): **an ARM64 image
  (0xAA64) gets no key**, and an AMD64 header alone is not taken as proof of native x64 (see the
  CHPE rule below). The header block is found by `e_lfanew` and `SizeOfHeaders` (never a fixed
  size; `SizeOfImage` ≤ 1 GiB, `SizeOfHeaders` ≤ 1 MiB, every section inside `SizeOfImage`). The
  bytes `[0, SizeOfHeaders)` are hashed with `OptionalHeader.ImageBase` (offset 24 of the optional
  header) zeroed, because the loader may rewrite it with the load address. Every other header
  byte, the data directories and the section table included, must match. All arithmetic on
  values from the image is checked; a malformed image is a refusal, never a panic.
- **Sections.** Every section whose `Characteristics` lacks `IMAGE_SCN_MEM_WRITE` (0x80000000)
  and whose mapped length (`VirtualSize`, or `SizeOfRawData` when that is 0) is not zero, in table
  order, over its mapped length. Writable sections are skipped. `IMAGE_SCN_MEM_DISCARDABLE`
  sections (`.reloc`) are hashed: the loader maps them in a user-mode image and does not change
  them.
- **Base relocations** (data directory 5): blocks of `{u32 PageRVA, u32 BlockSize, u16 entries}`,
  `BlockSize` ≥ 8, even, inside the table; an entry is `type = e >> 12`, `offset = e & 0xfff`. On
  x64 a linker emits `IMAGE_REL_BASED_DIR64` (10, a 64-bit field) and `IMAGE_REL_BASED_ABSOLUTE`
  (0, padding). Each DIR64 site inside a hashed section is replaced by `value - base` (wrapping),
  which is the same in memory and in the file. **Any other type** (HIGHLOW 3, HIGH 1, LOW 2,
  HIGHADJ 4, the ARM/MIPS/RISC-V types 5, 7, 8, 9) refuses: no key.
- **Loader-written ranges, zeroed after normalising:**
  - the IAT (data directory 12);
  - each import descriptor's (directory 1, 20-byte entries up to an all-zero one) `FirstThunk`
    array, its length taken from `OriginalFirstThunk` (unchanged by the loader), terminator
    included. **A descriptor with `OriginalFirstThunk` 0 refuses**: without a separate lookup
    table, masking the IAT would mask the only record of which imports the slots select, and two
    files importing different functions would compare equal;
  - each delay-load descriptor's (directory 13, 32-byte entries) module-handle slot (8 bytes) and
    IAT (counted from its name table; a descriptor without one refuses); VA-based descriptors
    (`Attributes` bit 0 clear) are converted with `base`;
  - the TLS directory's (9) `AddressOfIndex` target (4 bytes: the loader writes `_tls_index`);
  - from the load config (10, read up to its `Size`, at most 0x1000): the targets of
    `SecurityCookie` (0x58), `GuardCFCheckFunctionPointer` (0x70), `GuardCFDispatchFunctionPointer`
    (0x78), `GuardRFFailureRoutineFunctionPointer` (0xD8), `GuardRFVerifyStackPointerFunctionPointer`
    (0xE8), the three `GuardXFG*FunctionPointer`s (0x118, 0x120, 0x128),
    `CastGuardOsDeterminedFailureMode` (0x130) and `GuardMemcpyFunctionPointer` (0x138), 8 bytes
    each, when non-zero. These offsets match windows-sys 0.61.2's `IMAGE_LOAD_CONFIG_DIRECTORY64`
    (checked by sol, review r3).
  - TLS callbacks are not loader-written: the callback array holds VAs, normalised as relocation
    sites like any other pointer. Ordinary x64 unwind data (`.pdata`) and the CFG target tables
    stay compared: they are not guard pointer slots.
- **Refused as loader patching this code does not model** (no key): a non-zero
  `DynamicValueRelocTable` (0xC0), `DynamicValueRelocTableOffset` (0xE0) or
  `DynamicValueRelocTableSection` (0xE4) (dynamic value relocations: import-call optimisation,
  retpoline); a non-zero `CHPEMetadataPointer` (0xC8): an ARM64EC or ARM64X image, which also has
  an AMD64 header; a load config too short (< 0xD0) to show that field; a non-zero
  `HotPatchTableOffset` (0xF0); any non-zero byte past the known fields (0x140) up to `Size`.
  (An image with no load config at all cannot be ARM64EC, which requires one.)
- **Masking rules:** a loader-written range may never overlap an executable section
  (`IMAGE_SCN_MEM_EXECUTE` or `IMAGE_SCN_CNT_CODE`): refused. The metadata each mask comes from
  (import descriptors and lookup tables, delay-load descriptors, the TLS directory, the load
  config) must itself lie in hashed bytes (the headers or a non-writable section), so mask
  metadata from a skipped (writable) section is never trusted: refused. **"In a hashed section"
  is not "hashed" (rev 6):** once every mask is known, any mask that overlaps any of that metadata
  refuses, so no mask can zero the bytes it was derived from. The relocation table (directory 5)
  must lie in hashed bytes and outside every mask. A section that is both writable and executable
  refuses (rather than being skipped as writable). The slot must lie in a non-writable section
  outside every mask.
- **Hash:** blake3 over a domain tag, the header bytes, then each hashed section's index, length
  and bytes.

If the loader writes anything else into a read-only section of our binaries, A differs from the
true file and the key is never made. The probe's W2 and W9 ("unaltered: accepted"), run on a
`--profile release-ship` build, are evidence that the model is complete **for that build on that
Windows version**, not a proof for every Windows build: a later Windows may patch more. So the
product must make "no key" visible (the IR cache report's `no key` line and the one-time warning,
as on Linux), and the shipped MSVC binaries must give positive keys on the owner's machine before
the cache is turned on.

**The marker** is unchanged: `key_of` searches the hashed bytes for it. The Windows `release-ship`
jobs (`build-and-deploy.yml`, `release.yml`) gain the marker search for `alsem.exe`, `aldump.exe`
and `al-call-hierarchy.exe`.

## 4. The producer audit on Windows

Decision Q3: the static CRT alone is not enough. ENG-110's key is only sound while nothing outside
the image changes how a text parses or decodes. Orchestrator decision (rev 3): **audit at the C
source level**, which holds whatever compiler and linker build it.

### 4.1 The source-level audit (authoritative)

The C code on the parse path is tree-sitter's runtime (crate `tree-sitter` 0.27.1, `src/lib.c`,
which includes every runtime `.c`) and the grammar (`tree-sitter-al` `88bb6871`: `src/parser.c`,
`src/scanner.c`). The lowerer and the IR codec are Rust; their text handling is `core`/`std`
(table-based `char` methods, UTF-8 decoding), which calls no OS locale API.

**Method.** GCC builds a call graph from the source: `gcc -O0 -fcallgraph-info -c` on `lib.c`,
`parser.c` and `scanner.c` (one `.ci` file each). A short script
(`/work/.al-sem/tools-notes/tls117-closure.py`, run as `python3 tls117-closure.py lib.ci
parser.ci scanner.ci`; it becomes `scripts/check-parse-path-locale` in the product phase) computes every function reachable from the roots below and lists every external callee
with its call sites.

**Roots** (what al-syntax reaches, as a superset): every `ts_parser_*`, `ts_tree_*`, `ts_node_*`,
`ts_tree_cursor_*` and `ts_language_*` function, which covers the API al-syntax calls through the
Rust binding (`Parser::new`, `set_language`, `parse` → `ts_parser_parse_with_options`,
`Tree::root_node`, the `Node` accessors `kind`, `kind_id`, `grammar_id`, `is_named`, `is_error`,
`is_missing`, `has_error`, `start_byte`/`end_byte`, `start_position`/`end_position`, `child_count`,
`child_by_field_name`/`_id`, `field_name_for_child`, `walk` and the cursor's `goto_*`, and
`Language::node_kind_for_id`/`node_kind_count`/`id_for_node_kind`); plus every function-pointer
target, because a call graph cannot follow pointers: the allocator defaults
(`ts_malloc_default`, ...), the lexer callbacks (`ts_lexer__advance`, `__mark_end`,
`__get_column`, `__eof`, `__is_at_included_range_start`, `__log`), the grammar's lex functions and
every `tree_sitter_al_external_scanner_*`. **Excluded** (al-syntax never calls them; checked by
grep over `crates/al-syntax` and `src`): `ts_parser_set_logger`, `ts_parser_logger`,
`ts_parser_print_dot_graphs`, `ts_tree_print_dot_graph`, `ts_node_string` (`Node::to_sexp`), the
wasm store, and the whole `ts_query_*` family (`Query`, `QueryCursor`).

**Result.** 212 roots reach 415 defined functions. Their external callees, all of them:

| External callee | Reached from | Locale-sensitive? |
|-----------------|--------------|-------------------|
| `malloc`, `calloc`, `realloc`, `free`, `memcpy`, `memmove`, `memset`, `memcmp`, `strcmp`, `strncmp`, `abort`, `__assert_fail` | allocator defaults, arrays, scanner state, `ts_language_symbol_for_name` / `field_id_for_name`, the scanner's keyword tables | No: byte operations. |
| `snprintf` (44 sites), `vsnprintf` (1) | `ts_parser__advance`, `__lex`, `__recover`, `__reduce`, `__select_tree`, `ts_parser_parse` and others; `ts_lexer__log` | Formatting can be (the decimal point), but every site is inside tree-sitter's `LOG`/`LOG_LOOKAHEAD` macros, which run only `if (self->lexer.logger.log || self->dot_graph_file)` (`parser.c:21-35`), or in `ts_lexer__log`'s `LOG` (`lexer.c:10`, `if (self->logger.log)`). al-syntax sets neither, so none runs; and what they format goes to the log, never into the tree. |
| `fprintf` (17), `fputc` (11), `fwrite` (33) | the allocator defaults (only on allocation failure, before `abort`), `ts_parser__log`, `ts_stack_print_dot_graph`, `ts_subtree__print_dot_graph`, `ts_language_write_symbol_as_dot_string`, and `LOG_STACK`/`LOG_TREE` in the parser | Same guards (`dot_graph_file`, logger), or the abort path. |
| an indirect call (522 sites) | the lexer callbacks, the allocator, the scanner and lex functions (all roots above), the logger (guarded), the input-read callback (Rust) | Covered by the roots. |

No `isw*`, `tow*`, `is*`/`to*` classification, `setlocale`, `localeconv`, `strtod`, `strcoll`,
`mbstowcs`/`wcstombs` or other locale-sensitive CRT function is reachable. The grammar's
`scanner.c` (including its case-insensitive word reads) calls only `calloc`, `free` and `strcmp`;
the generated `parser.c` calls nothing external. **Not reachable**: the 65 functions of the query
parser (`ts_query__parse_pattern`, `stream_*`, which call `iswspace`/`iswalnum`) and
`ts_subtree__write_to_string` (`isprint`, behind `ts_node_string`).

Windows-only source: `#ifdef _WIN32` code in tree-sitter is `_ts_dup` (for
`ts_tree_print_dot_graph`, excluded), the `Interlocked*` atomics, and `_fdopen` in
`ts_parser_print_dot_graphs` (excluded). None touches text.

### 4.2 Supporting evidence: a mingw cross build

The shipped Windows build (`x86_64-pc-windows-msvc`, `+crt-static`) puts the C runtime inside the
image, which hides CRT calls from an import list. A **mingw cross build** (`x86_64-pc-windows-gnu`,
`release-fast`, built here) imports the C runtime from `msvcrt.dll`, so every CRT call shows as an
import. It agrees with 4.1:

```
cargo build --target x86_64-pc-windows-gnu --profile release-fast --bin alsem
x86_64-w64-mingw32-objdump -p target/x86_64-pc-windows-gnu/release-fast/alsem.exe
```

187 imports in 9 DLLs: `KERNEL32`/`kernel32` (123), `msvcrt` (50), `ntdll` (5), `advapi32` (3),
`api-ms-win-core-synch-l1-2-0` (3), `bcryptprimitives`, `combase`, `shell32` (1 each).

No `setlocale`, `_create_locale`, `GetLocaleInfo*`, `LCMapString*`, `CompareString`/`Ex`,
`GetStringType*`, `FoldString`, `towlower`/`towupper` or `isw*`-by-name import. Nothing calls
`setlocale`, so the C runtime stays in the "C" locale. **Locale-sensitive or locale-adjacent imports
found**, each traced to the functions that use it in a second build with symbols kept
(`CARGO_PROFILE_RELEASE_FAST_STRIP=none`, then `objdump -d` and the callers of each import slot):

| Import | Used by | Reachable from parse, lower or codec? |
|--------|---------|----------------------------------------|
| `msvcrt!iswctype` (5 call sites) | tree-sitter's **query** parser: `stream_skip_whitespace`, `stream_is_ident_start`, `stream_scan_identifier`, `ts_query__parse_pattern` (`query.c`'s `iswspace`/`iswalnum`) | **No.** `ts_query__parse_pattern` is called only from itself and `ts_query_new`, and `ts_query_new` has no caller and no address-taken reference in the image (dead code the PE link kept; on Linux the linker drops it, which is why the Linux check never saw `iswalnum`). Nothing in `src/` or `crates/` builds a tree-sitter `Query`. |
| `msvcrt!isprint` | `ts_subtree__write_to_string` (`subtree.c`), called only from `ts_node_string` and `ts_subtree_string` | **No.** Neither has a caller in the image; nothing calls `Node::to_sexp` (two lowerer comments mention it as a debugging aid). |
| `msvcrt!localeconv`, `___lc_codepage_func`, `___mb_cur_max_func`; `kernel32!MultiByteToWideChar`/`WideCharToMultiByte` via `__mbrtowc_cp`/`__wcrtomb_cp` | mingw's own `printf` engine (`__mingw_pformat`: the decimal point, wide-character output), reached from `__mingw_snprintf`/`__mingw_vsnprintf`/`__mingw_fprintf` | **Not into the IR.** tree-sitter calls those only to format debug log lines (`LOG(...)` in `ts_parser_parse`, `ts_parser__lex` and others, which return at once when no logger is set; nothing in al-sem sets one), to print dot graphs (never enabled), and to report a failed allocation before `abort`. The text they format never reaches the tree. |
| `kernel32!CompareStringOrdinal`, `MultiByteToWideChar`, `WideCharToMultiByte` (direct) | Rust std: environment-variable key comparison (`EnvKey`, ordinal, not locale-sensitive), console reads and writes | No. |
| `msvcrt!strtol`, `fprintf`, `vfprintf` | only their import thunks (`strtol` is a mingw runtime helper); no call from tree-sitter or the grammar | No. |

The grammar's own C code (`libtree-sitter-al.a`) imports only `calloc`, `free` and `strcmp`.

The mingw link kept the unreachable query parser (it was in the image with no caller); the Linux
link dropped it, which is why the Linux check never saw `iswalnum`. That is a linker difference,
which is why 4.1, not an import list, is the proof.

### 4.3 Conclusion and pins

The Windows parse, lower and codec path calls nothing locale-sensitive, by source. The shipped MSVC
build compiles the same source, so the same functions are reachable whatever `/OPT:REF` keeps.

Pins (product phase):
- **`scripts/check-parse-path-locale`** (Linux, beside `scripts/check-ir-image`, run in CI): the
  call-graph closure of 4.1 over the tree-sitter version in `Cargo.lock` and the grammar commit in
  use. It fails if any reachable external callee is outside an allow-list (the byte functions
  above) other than the guarded logging and dot-graph sites, which it lists by function. A
  tree-sitter or grammar update that adds a locale-sensitive call on the parse path fails it.
  **As built (rev 11):** `scripts/check-parse-path-locale`, run by `scripts/ci-steps locale` (in
  `task`, `task-affected` and `all`) and by CI's `test` job; about 3 s. It compiles with
  `-fno-stack-protector` (Ubuntu's gcc otherwise adds `__stack_chk_fail`, a compiler canary) and
  `-fno-builtin` (gcc otherwise folds a builtin's call away, so a `towlower` on a constant left no
  edge; with it, the dot-graph writes show as `fputs`, which joins the guarded list). The allow
  list is the byte functions above plus the guarded `snprintf`, `vsnprintf`, `fprintf`, `fputc`
  and `fputs`, each with its **pinned caller set** (a new caller fails: it may be unguarded).
  It is a **closed world** (Fable N3): the pointer-only callbacks are roots too
  (`pop_*_callback`, `summarize_stack_callback`, `ts_decode_utf8`/`_utf16_le`/`_utf16_be`,
  `ts_string_input_read`: 221 roots reach 426 of 562 defined functions), and a defined function
  neither reached nor on its off-path list fails. That list names 136 functions exactly, no
  wildcard (Fable F2): the query engine and its helpers, the lookahead iterator, wasm, the
  logger and dot graphs, `ts_node_string` and the subtree printer, `ts_set_allocator`. gcc on Linux compiles the `#ifdef _WIN32` branches out (the
  Windows-only source listed in 4.1). Without gcc it fails on Linux and skips with one line
  elsewhere (Git Bash). Every run also checks two copies of `scanner.c`, one whose
  `tree_sitter_al_external_scanner_create` calls `towlower` and one with an added unlisted
  function, and fails unless both are refused; a grammar copy with the `towlower` call as its
  real `scanner.c` fails the real check too, and `ts_parser_parse` dropped from `snprintf`'s pin
  gives `NEW CALLER` and a failure (2026-11-23).
- **The MSVC map listing** (Windows CI job): build `alsem`, `aldump` and `al-call-hierarchy` with
  `-C link-arg=/MAP` and list every deny symbol found in each map (`setlocale`, `_create_locale`,
  `localeconv`, `isw*`, `_isw*_l`, `iswctype`, `isprint`, `tow*`, `GetLocaleInfo*`, `LCMapString*`,
  `CompareStringEx`, `CompareStringW`, `GetStringType*`, `FoldString*`) with the object that
  defines it. A map says what was linked, not what is reachable, so this list does not fail the
  job by itself: a human checks each entry against 4.1 (expected: the static UCRT's own copies, and
  tree-sitter's `lib.obj` if the link keeps the query parser). A failing pin would need
  reachability, which 4.1's check already gives.
  **As built (rev 11):** `release.yml`'s `build-windows` job sets
  `CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUSTFLAGS=-C link-arg=/MAP`, so every lld-link run writes
  a map next to its output (`deps\<name>.map`: an MSVC bin has no hash suffix there; the step
  also takes `<name>-<hash>.map`, never `alsem_stamp`'s); the step "List locale symbols in the
  link maps (for a human)" prints, for the newest map of `alsem`, `aldump` and
  `al_call_hierarchy`, each map line naming a deny symbol (up to 200; no leading word boundary,
  so `__acrt_LCMapStringW`, `_o_towlower` and `__imp_...` match), and warns when a map is
  missing; it never fails the job. A separate step fails the job when a shipped exe imports the
  dynamic C runtime (`dumpbin /dependents`), so a lost `+crt-static` cannot ship. The per-target variable is used because Cargo **merges** it with `.cargo/config.toml`'s
  `target.x86_64-pc-windows-msvc.rustflags` (`+crt-static` stays), while a plain `RUSTFLAGS`
  **replaces** them (checked 2026-11-23 with cargo 1.96 on the Linux target: the merged run kept
  `+crt-static`, the `RUSTFLAGS` run dropped it). `/MAP` only writes the map; the extra flag may
  change Cargo's hashed crate metadata (symbol name suffixes), not what the code does. Not yet
  run: the first release after this change is the check. `build-and-deploy.yml` has no map step
  (one listing is enough for a human).

## 5. Locks: `std::fs::File` on both systems (decision Q4: yes)

`CacheDir::lock` moves from `libc::flock` to `File::lock` (the waiting `Exclusive`),
`File::try_lock` (`ExclusiveNow`) and `File::try_lock_shared` (`SharedNow`), stable since Rust
1.89 (the toolchain is 1.96). The Windows `RepairLock(None)` branch goes; ENG-113 gets real locks on
Windows too (said in the CHANGELOG).

- **Linux: std uses `flock`, confirmed on 1.96.** A 1.96 binary calling the `File` lock methods
  imports `flock@GLIBC_2.2.5` (`nm -D --undefined-only`). And behaviour: with a `flock(2)`
  `LOCK_EX` held by another process (Python `fcntl.flock`), 1.96's `try_lock` and
  `try_lock_shared` both return `WouldBlock`; after `LOCK_UN` both succeed; a POSIX record lock
  (`fcntl.lockf`) does not block them. So old `libc::flock` holders and new `File::lock`
  contenders exclude each other, which matters while an old engine binary still runs beside a new
  one sharing the cache.
- **Windows: `LockFileEx`** (`LOCKFILE_EXCLUSIVE_LOCK`, `LOCKFILE_FAIL_IMMEDIATELY`)
  ([std::fs::File](https://doc.rust-lang.org/std/fs/struct.File.html),
  [LockFileEx](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-lockfileex)).
  Locks belong to a handle (as `flock` to an open file). A lock is mandatory for its byte range,
  which never matters: lock files hold no data. A dead process's locks are freed "depending upon
  available system resources", so the probe measures it (W5).
- Unchanged semantics: ENG-113's publish lock (`Exclusive`) waits; every IR lock (the per-entry
  publish lock, the key-folder lock, `prune.lock`) is a `Now` kind and never waits.

Tests for the product change:
- `snapshot::cache::tests::old_flock_holder_blocks_new_file_lock` (Unix): a child process holds
  `libc::flock(LOCK_EX)` on a lock file; `CacheDir::lock(.., ExclusiveNow)` and `SharedNow` return
  `None`; after the child releases, both succeed. Proof: take the lock with `fcntl(F_SETLK)` in the
  child instead, red.
- `snapshot::cache::tests::shared_now_lock_fails_while_exclusive_held`: un-gated from `unix`.
- Windows: independent handles, explicit release and process death are probe W5.

## 6. The private check on Windows (decisions Q2 and Q5)

**The rule.** A folder or file is private when, read on its **opened handle**:
- it is not a reparse point (any tag: junction, symlink, mount point, cloud file, ...);
- it has a DACL (an absent or null DACL refuses);
- its owner is trusted: the current user, SYSTEM (`S-1-5-18`), Administrators (`S-1-5-32-544`)
  or TrustedInstaller (`S-1-5-80-956008885-...`, the owner of system folders such as a volume
  root); SIDs are compared as strings (`ConvertSidToStringSidW`);
- no allow ACE (that applies to the object itself: inherit-only ACEs are skipped) gives an
  untrusted principal a right in the refused set; deny ACEs are skipped (they only
  remove rights); **any other ACE type refuses** (object, callback, conditional ACEs are not
  interpreted).

The refused set depends on the role:

| Role | Refused rights for others |
|------|---------------------------|
| every element of the path, from the cache folder up to the **volume root** | `DELETE`, `WRITE_DAC`, `WRITE_OWNER`, `FILE_DELETE_CHILD`, `GENERIC_WRITE`, `GENERIC_ALL` (what lets someone remove, rename or re-ACL an existing element) |
| the cache folders themselves (`al-ch-snapshot-cache`, `ir`, `ir/<key>`, or `ALSEM_IR_CACHE_DIR`) | the above plus `FILE_ADD_FILE`, `FILE_ADD_SUBDIRECTORY` |
| each entry and lock file, checked on the handle used to read or lock it | the above plus (same bits) `FILE_WRITE_DATA`, `FILE_APPEND_DATA` |

`FILE_ADD_SUBDIRECTORY` on an ancestor is allowed: it lets others add new names, never replace an
existing one, and `C:\` grants it to Authenticated Users by default.

The volume must be local NTFS (fixed drive and `NTFS`, as for the image).

**The walk (rev 3, sol r2 P1-2).** A cache folder is opened by a walk **top-down from the volume
root**: the path must be a drive-letter path with no `.` or `..`; the walk opens `\\?\X:\`, checks
locality and the chain rights on that handle, then opens each element in turn under the checked
one, refusing a reparse point before it descends further, and checks it (the last element with the
folder rights). Each element is opened sharing read and write but **not delete**, so nothing on
the path can be renamed while the walk holds it. With `create`, a missing element is made only
under its just-checked parent, then opened and checked like any other. The walk returns the rooted
spelling `\\?\X:\...\leaf`, and **all cache I/O uses that spelling** (no second resolution through
the current directory, a drive-relative path or a different case). Entries and lock files are
checked on the very handles that read, map or lock them.

**Why the whole path, and why not handle-relative opens.** sol P1-2: a private leaf under a
parent others can change is not private, because the parent's child can be swapped. Two sound
options:
- (a) handle-relative I/O, as Unix does with `openat`: `NtCreateFile` with a root directory
  handle, `SetFileInformationByHandle(FileRenameInfo)` with `RootDirectory`, and disposition-based
  deletes. windows-sys has `NtCreateFile` (under `Wdk_Storage_FileSystem`), but it needs
  `OBJECT_ATTRIBUTES`, `UNICODE_STRING`, `IO_STATUS_BLOCK` and NTSTATUS handling, and every write,
  rename and delete path of `CacheDir` changes;
- (b) **chosen:** keep path-based I/O (ENG-113's Windows code already is) and check every path
  element up to the volume root when a `CacheDir` is opened. When no element can be renamed,
  deleted or re-ACLed by anyone but the user, SYSTEM and Administrators, the path cannot be
  redirected by another principal, so path-based I/O reaches the checked folder.

(b) costs one ACL read per path element per `CacheDir::open` (about 6 for the default root) and
one per entry or lock opened. It is less code, keeps the Unix and Windows `write` paths as they
are, and is what the probe tests (W6). The cost of (b): an `ALSEM_IR_CACHE_DIR` whose ancestors
others may rename (a shared drive) turns the cache off. That is the intended answer.

**Where it applies.** `CacheDir::open`, `subdir`, `read_owned`, `read` (the mapped text entries)
and `lock` on Windows, so both caches: the IR cache, and ENG-113's text folder (Q5). A text folder
that fails falls back to owned text, as a non-private folder does on Unix today. Folders the cache
makes are created inside a checked folder and inherit its DACL; they are checked again on open.

Tests (as built, rev 11; Windows-native, run by the owner with
`cargo test -p al-sem --lib -- winfs key::tests::windows snapshot::cache prune`):
- `snapshot::winfs::tests::windows_private_walk`: the per-user cache folder passes; a leaf
  Everyone can modify, a private leaf under a parent granting Everyone `FILE_DELETE_CHILD`, and a
  path through a junction are refused; a create under that refused parent fails and makes nothing
  (`walk(.., true)`, the path every cache root is created by); an entry Everyone can modify fails
  the handle check, a fresh one passes.
- `snapshot::cache::tests::a_permissive_entry_is_refused_at_every_open`: in a private folder, a
  text entry, a stamp file and a lock file granted Everyone modify are refused at each call site
  that opens one, `read`, `read_owned` and `lock` (all through `check_private_file`); the same
  files before the grant are accepted.
- `snapshot::cache::tests::a_mapped_entry_cannot_be_opened_for_writing` and
  `shared_now_lock_fails_while_exclusive_held` (both systems).
- `program::ir_cache::key::tests::windows_stamped_copy_keys_on_its_image`: an unstamped copy
  gets no key (`unstamped`); a stamped copy keys on its image, and after its key the running exe
  can be renamed away while the process runs on (no handle held); a folder swap to a rival whose `.text`
  differs by one known byte gives no key, refused at the canonical comparison for both candidates.
- `program::ir_cache::prune::tests` run on Windows too (through the Windows `write`, `unlink` and
  `remove_dir`), except the two size-rule tests (sparse files) and the symlink part of
  `foreign_names_left_alone`; `the_writers_temp_names_are_prunable` and
  `prune_removes_what_the_writer_wrote` are new.
- The probe's W6 ran the same walk on the real roots: ENG-113's text root, the IR root, and
  `ALSEM_IR_CACHE_DIR` (or a probe-made override when it is unset).

## 7. Prune and publish on Windows

- **Temp names (sol P2).** `prune.rs`'s `is_temp` matches `<stem>-<pid>-<n>-<nanos>.tmp`, the
  Unix `write`'s shape. The Windows `write` uses `tempfile::Builder` with prefix `<stem>-` and
  suffix `.tmp`, which makes `<stem>-XXXXXX.tmp` (6 random characters, tempfile 3.27's
  `NUM_RAND_CHARS`), so the prune would never remove a killed writer's leftovers. Fix: the Windows
  `write` makes its temp file with the same name shape (`OpenOptions::create_new`, the same 16
  tries) instead of `tempfile`, so one pattern covers both systems. As built, both writers name
  their temp file with one function, `snapshot::cache::temp_name`. Tests:
  `program::ir_cache::prune::tests::the_writers_temp_names_are_prunable` (`is_temp` accepts
  `temp_name`'s output for every stem the cache writes) and `prune_removes_what_the_writer_wrote`
  (a key folder written through `CacheDir::write`, then pruned through `unlink` and `remove_dir`,
  and a stale temp of `temp_name`'s shape removed), both on Windows too.
- **Deletes.** IR entries are read whole and closed; `open_entry` shares delete, so a delete
  normally succeeds while another process reads. Whether the name goes at once (POSIX delete
  semantics) depends on the Windows build; the probe records it (W7). A folder that cannot be
  removed yet goes at the next prune; a virus scanner holding a file is the same case.
- **Publish.** ENG-113's publish into place stays, as built with `std::fs::rename` (replace) and
  `std::fs::hard_link` (no-clobber: it fails when the name exists) from the temp file, not
  `tempfile`'s `persist` / `persist_noclobber`. A rename over an entry another process is reading
  can fail: a failed publish, a miss.

## 8. The probe

`examples/tls117_probe.rs`. Windows only (on other systems it prints "Windows only" and exits 2).
It needs nothing but the repository: it copies its own executable into scratch folders and starts
the copies as children it drives over stdin. The key sequence in it is a prototype of design B
(section 3; the key is the accepted file's blake3, without the marker search). Its extra
windows-sys features are a `[target.'cfg(windows)'.dev-dependencies]` entry, so the shipped
binaries do not change.

**Rev 7 changes to the probe.** `candidates_ok` refuses a reply that names no candidate. In the
stale-name reproductions (the folder swap at "after-query", and the hard link after the launch
link is replaced) at least one candidate must be an observed rival: opened, a file id other than
the original's, refused at the canonical comparison. Barriers the child sends without having run
the step they name (no candidate opened or read, or an unstamped build) say `synthetic`
(`BARRIER after-hash synthetic`); a W1 run whose target barrier is synthetic FAILs ("not
credited as a race test") instead of counting.

**Rev 6 changes to the probe.** The probe's own binary is never stamped (it is a dev or
`release-ship` build); every W-item that needs a key copies it, **stamps the copy** (`pe::stamp`,
the stamp tool's code, with the magic read from the probe's own slot), and launches the copy; the
true key is the stamped copy's hash. Rivals are made from the stamped copy (one `.text` byte
changed, slot still valid), so they are refused at the canonical comparison alone. Every child key
report gives, per candidate, its file id and verdict (`mapped (id <vol>:<id>): ...`); the parent
judges each candidate **by identity first**: a candidate that is the original file (a surviving
alias) may be accepted; any other must have opened and been refused at the canonical comparison.
Barrier lines say whether the child's first open succeeded (`after-open opened` / `after-open
failed`); "blocked while the key's handle is held" is credited at `after-open` only after a
successful open (at `after-hash` the handle is always held). Every POSIX HELD outcome, from a
reported success that moved nothing or from an OS refusal, requires the control. W2 and W9 run
in a stamped child (W9 through the `w9` child command); W9's `.data` flip must now be refused by
the slot, and its missing-section case must be refused exactly at the headers. **W11** (new): an
unstamped copy gives no key, with "unstamped" in the reason.

**Rev 5 changes to the probe.** Every replacement file in W1 is a **rival valid build**: a copy of
the running image with one `.text` byte changed (same headers), so part B reaches the canonical
comparison. In the folder swap (when it happens at "after-query") and after the launch hard link
is replaced, both candidates must be refused there (`mapped: refused (canonical hash differs)` and
`module-name: refused (canonical hash differs)`), else the item fails. The POSIX rename-replace
runs a **control** first: the same `FileRenameInfoEx` call over a non-running target, which must
replace it (verified by file id); HELD is reported only when the control replaced and the image's
own rename moved nothing (the image kept its name and the other file stayed where it was). W9
runs every case through the full candidate path (written to a file, opened denying write and
delete, local-NTFS checked, read through the handle, headers then hash), picks flip offsets
outside every zeroed range (the IAT sits at the start of `.rdata` on MSVC), fails when a required
section (`.text`, `.rdata`, `.data`) is missing, adds a candidate with one section missing from its
table (refused at the headers), and prints the `.data` verdict without judging it (W10 judges).

**Rev 4 report.** Every key a child makes prints, after the verdict, A's hash (first 16 hex
digits, with the number of sections and relocation sites it covered) and each candidate's verdict
(`mapped: accepted`, `module-name: refused (canonical hash differs)`, or the open/read error). The
verdict says whether the key equals the running image's true key, which the parent knows: the
blake3 of the copy it launched, taken before launch. The rev 3 run's three failures must become
"key = the running image's key" or "no key": the folder swap and the deleted launch link (both
candidates then name another file, so B refuses both), and the POSIX rename-replace, which is
now `HELD (the OS kept the running image: the POSIX rename reported success and moved nothing)`
when the image kept its name **and** the other file is still where it was, and a FAIL otherwise.

| Item | What it checks | PASS when |
|------|----------------|-----------|
| W0 | the image's volume, ENG-113's text root, the IR root, and `ALSEM_IR_CACHE_DIR` (or a probe override); for a root not made yet, its deepest existing folder | each is a fixed drive (`GetDriveTypeW`) formatted `NTFS` |
| W1 name | a child renamed while running reports its mapped name | the name ends in the new file name |
| W1 rename away + write, MoveFileEx replace, POSIX rename replace, ancestor folder replaced | the child runs the real sequence with barriers after the query, after the open and after the hash; at one barrier the parent mutates; 3 runs per mutation. The image is `<scratch>\dir\a.exe`. Mutations: rename `a.exe` away and write another file at its name; `MoveFileExW(other, a.exe, MOVEFILE_REPLACE_EXISTING)`; `SetFileInformationByHandle(FileRenameInfoEx)` with `FILE_RENAME_FLAG_REPLACE_IF_EXISTS \| FILE_RENAME_FLAG_POSIX_SEMANTICS`; rename `dir` away (not back), make a new `dir` with another `a.exe` | per run: after a successful mutation, **before the barrier is released**, the old name must open a file with a different `FILE_ID_INFO`, else FAIL ("did not take"); then the key must be the original file's hash or none, never another. A refused MoveFileEx or POSIX rename-replace (a replacement OF the running image itself) with access denied or a sharing violation prints `HELD (the OS protects the running image: <error>)` and passes (decision Q8). For rename-away and the folder swap, the ordinary installer paths, a refused mutation **FAILs** ("refused mutation"), except after the open, where a sharing violation or access denied is the key's own handle refusing it and is reported as "blocked while the key's handle is held" (the property F2 claims) |
| W1 hard link | launch through a hard link; delete the launch link (or, if refused, rename it away: both refused FAILs), write another file there (checked), verify its file id differs, while `a.exe` survives | the original hash or no key, before and after |
| W1 other case | launch through an upper-case spelling | the original hash or no key |
| W1 8.3 | launch through the short name of a long-named leaf | the original hash or no key; SKIP only with "8.3 disabled on this volume (no short name was made)" |
| W2 | design B in a stamped copy, unchanged; run on the default (dev) build and on a `--profile release-ship` build | the key equals the stamped copy's hash, and every candidate is judged by identity. Evidence that the loader model is complete for that build on that Windows version, not more |
| W9 | in a stamped child, part B through the full candidate path on copies of its own (stamped) file against its part A and loaded slot: unaltered; one byte changed in `.text`; in `.rdata` (offsets outside every zeroed range); one section missing from the table; one byte changed in `.data` | unaltered accepted; `.text` and `.rdata` refused at the canonical hash; the missing section refused exactly at the headers; the `.data` change refused by the slot ("writable sections do not match the loaded digest"); a missing required section FAILs |
| W10 (diagnostic) | `examples/tls117_w10.ps1`: three `--profile release-ship` builds of the probe: base; `--features w10-variant` (only `W10_VALUE`'s initial value differs); base again after touching the probe's source. The script checks the linker ran (the output's mtime moved; its hash change is printed). `tls117_probe w10-compare` stamps both builds in memory and compares | always: the stamped slots differ exactly when the writable sections differ. **variant**: `W10_VALUE` in a writable section in both, and the writable sections differ. **relink**: equal canonical hashes only with equal writable sections. Printed: whether the canonical hashes differ, and each build's TimeDateStamp, REPRO (`/Brepro`) entry and PDB GUID |
| W11 | an unstamped copy asked for its key | no key, with "unstamped" in the reason |
| W3 | controls first: an ordinary copy opens for write and for write+truncate; then while a child runs: write, write+truncate, `FILE_WRITE_DATA` opens of the image; rename; delete (recorded) | the controls succeed, the three opens fail, the rename succeeds |
| W4 | a key; then a handle with `DELETE` access held on the running image; then closed; and a `FILE_WRITE_DATA` handle on a plain file | key before; while held, no key with a sharing violation (`os error 32`) in the reason; key after; our read-only-sharing open of the plain file fails with a sharing violation |
| W5 | `std::fs::File` locks on three handles: shared and exclusive beside an exclusive, exclusive beside two shared (third handle), each `WouldBlock`; explicit `unlock` frees; a killed child's lock is freed | all hold, and the killed holder's lock frees within 10 s (time printed) |
| W6 | the walk of section 6 on the real roots, creating missing elements under checked parents; a fresh entry and lock file checked on their own handles | the roots pass (rooted spelling printed); the entry and lock pass; refused: a leaf Everyone can modify, a private leaf (passing alone) under a parent giving Everyone `FILE_DELETE_CHILD`, a path through a junction (as a reparse point), an entry Everyone can modify |
| W7 | delete a file another handle holds open (sharing delete) | the delete succeeds; printed: whether the name went at once (NotFound), is still there (pending), or cannot be read (an access error), and whether its folder could then be removed |

W6 makes the real text root and `ir` under it when missing (as the product's first writer would),
and removes its own override folder.

Each line is `PASS`, `FAIL` or `SKIP`, with what was seen. The process exits 1 when any item
fails. The same items are `#[test]`s in the example (the child half is an ignored test the
parent starts, as `key_is_the_running_image_not_the_path` does on Linux).

**Rev 7 unit test** (17 in all): `a_masked_iat_change_after_stamping_fails_the_slot`: after
stamping, one byte of the fixture's IAT (RVA 0x6038, in the read-only `.idata`) is changed; part
B still accepts the file at the canonical comparison (the IAT is masked), and the slot check
gives `DataDiffers`. Red proof below.

**Rev 6 unit tests** (16 in all). The fixture was rebuilt with the slot (`tls117_slot` at the start
of `.rdata`, file offset 0x800, RVA 0x3000), so the DIR64 sites moved to 0x3040, 0x3048, 0x3050,
0x3058, 0x3060 (pinned literally, read off `objdump -p`). New: a stamped fixture verifies (slot at
0x800/0x3000, part B accepts it, the slot check matches, and the unstamped fixture no longer matches
part A); one `.data` byte changed after stamping passes the canonical comparison but fails the slot
(`DataDiffers`); an unstamped fixture gives `Unstamped`; stamping refuses a missing or repeated
magic; the checksum code reproduces the linker's value on the unstamped fixture (0xe3a3, from
`objdump -p`: the independent check) and the stamp rewrites it consistently; widening the IAT
directory over the import descriptors refuses ("overlaps import descriptor"). **Red proofs**
(2026-11-23): with the slot's digest comparison disabled, `a_data_change_after_stamping_fails_the_slot`
fails; with the mask-over-metadata check disabled, `a_mask_over_import_metadata_refuses` fails;
both restored, all 16 pass. The stamp tool, run here on the fixture, stamps it (checksum 0xe3a3 →
0x5ea3) and is idempotent.

**Rev 5 unit tests** (10 in all): the five DIR64 site RVAs (0x3010, 0x3018, 0x3020, 0x3028,
0x3030) and the IAT (0x6038, 0x10) are pinned literally, read off `objdump -p` once; the code and
constant flips pick offsets outside every zeroed range and go through `check_candidate` (refused
at the hash, `.idata` included); a header change (the TimeDateStamp) is refused at the headers,
while `ImageBase` alone is not a header difference; an import whose `OriginalFirstThunk` is 0
refuses; a file cut inside `.rdata` refuses; `debug_info` reads a build without a debug
directory. `w10-compare` was checked here on the fixture: identical files PASS `relink`, and the
fixture with one `.data` byte changed FAILs it (equal canonical hash, different writable
sections).

**The canonicaliser's unit tests** (`pe::tests`, any system: `cargo test --example tls117_probe
pe::`) use `examples/tls117/fixture.exe`, a 4.6 KB PE built by mingw-w64 GCC 13 from
`examples/tls117/fixture.c` (command in the source): five DIR64 sites in `.rdata`, an IAT in the
read-only `.idata`, and `counter` in `.data`. They pin: two simulated loads at different bases
(every DIR64 site relocated, `ImageBase` rewritten, the loader-written ranges filled with
per-load bytes) canonicalise to the file's hash; the wrong base changes it; a changed `.text` or
`.rdata` byte changes it; a `.data`-only change does not (the stated scope); a HIGHLOW (type 3)
entry refuses; a truncated or non-PE input refuses. Discrimination proofs (2026-11-23): with the
normalising disabled (`value - 0`), `relocated_loads_canonicalise_like_the_file` and
`the_wrong_base_changes_the_hash` fail; with no ranges zeroed,
`relocated_loads_canonicalise_like_the_file` and `fixture_has_what_the_tests_need` fail; both
restored, all 7 pass.

Checked here: `cargo check --target x86_64-pc-windows-gnu --examples --tests` is clean, and
clippy shows no warning in the examples for the Windows target or Linux (`-D warnings` on Linux,
tests included). **Rev 7, checked on a cross build** (`x86_64-pc-windows-gnu`, `release-fast`,
mingw-w64 GCC 13 + lld, at the final rev 7 code): `grep -c -a TLS117-DATA-SLOT tls117_probe.exe`
is **1**; the stamp tool stamps it (slot in `.rdata`, a non-zero checksum recomputed); and
`w10-compare relink` on that build finds `W10_VALUE` in `.data` (writable). That run also showed
the value appears as an immediate in `.text`, so `w10-compare` now reports a writable section
when any occurrence lies in one (it reported `.text` before). The shipped MSVC build was not
checked here; if it ever carried a second copy, every stamp would refuse ("occurs more than
once") and W1/W2/W9 would FAIL loudly at "stamp the copy", never pass wrongly. The Windows items
have not run on rev 7.

## 9. Order of the owner run (Windows, repository root, `TREE_SITTER_AL_PATH` set)

Environment for every step (the TLS-121 Windows build settings):
```
$env:TREE_SITTER_AL_PATH = "<your tree-sitter-al clone>"
$env:CMAKE_GENERATOR     = "Visual Studio 17 2022"
$env:CARGO_TARGET_DIR    = "C:/atw"    # any short path; the default target path is too long (MAX_PATH)
```
**The grammar clone must be at the commit CLAUDE.md names** (today `88bb6871`, the tree-sitter-al
commit whose `node-types.json` hash equals the committed `crates/al-syntax/node-types.sha256`):
`al-syntax`'s build script compares the two and stops the build on a mismatch, as it did on the
owner's first try of a feature-branch checkout before the run at 9e8bf18f. Check with
`git -C $env:TREE_SITTER_AL_PATH rev-parse --short HEAD`, and move it with
`git -C $env:TREE_SITTER_AL_PATH checkout 88bb6871` (or the commit CLAUDE.md names then).

Save each step's whole output to a file and send the files back.

1. **The probe, W0-W12** (Git Bash or PowerShell):
   ```
   cargo run --example tls117_probe
   cargo test --example tls117_probe -- --test-threads=1 --nocapture
   cargo run --profile release-ship --example tls117_probe
   ```
   The probe stamps the copies it launches itself; nothing needs stamping by hand. The third line
   runs every item on the shipped profile (`panic = "abort"`, so a panicking item ends the run
   instead of printing FAIL). Send all three outputs back. Any `FAIL` stops here: the design is
   revised. The test run includes the `pe::` unit tests.
2. **W10, a diagnostic** (PowerShell; three full-LTO `release-ship` builds, several minutes each):
   ```
   powershell -ExecutionPolicy Bypass -File examples\tls117_w10.ps1
   ```
   (Rev 8: the script hashes through .NET instead of `Get-FileHash`, which Windows PowerShell 5.1
   could not load on the owner's PC; it runs in 5.1 and in pwsh 7.)
   It prints whether the linker ran, then `PASS`/`FAIL W10 variant` and `PASS`/`FAIL W10 relink`.
   It no longer decides the design (the slot is mandatory); a FAIL is reported to the orchestrator.
3. **Optional, the stamp tool by hand** (any system; e.g. on a `release-ship` `alsem.exe`, which has
   no slot until the product adds it, so it refuses with "no slot magic"):
   ```
   cargo run --example tls117_stamp -- <file.exe> [<out.exe>]
   ```
4. **The cache tests** (after the product change, on its branch):
   ```
   cargo test -p al-sem --lib program::ir_cache
   cargo test -p al-sem --lib snapshot::cache
   cargo test --test cli cli_ir_cache::
   ```
5. **Product acceptance (W8 on the installed, stamped binaries)**: the steps in section 3,
   "Product acceptance before enablement". The `cdo-gate` steps below remain a gate run of the
   product's own tests, but they run unstamped cargo builds, which get no key on Windows, so they
   do not count as acceptance. The older gate steps, for reference:
   ```
   export CDO_WS=U:/Git/DO-cdo-baseline/Cloud
   export ALSEM_IR_CACHE_DIR="$LOCALAPPDATA/al-ch-snapshot-cache/ir-tls117-$(date +%s)"
   # a) fresh-root publish
   bash scripts/cdo-gate --only output "$CDO_WS"
   # b) a second run: other processes, hits from disk
   bash scripts/cdo-gate --only output "$CDO_WS"
   # c) impact with every hit verified, from a fresh report
   rm -f target/tls117-impact.jsonl
   ALSEM_IR_CACHE=on ALSEM_IR_CACHE_VERIFY=1 ALSEM_IR_CACHE_REPORT=target/tls117-impact.jsonl \
     ENFORCE_CDO_WS=1 cargo test --profile cdo --test cli output_cdo::cdo_impact
   python scripts/ir-cache-report-check --rule impact target/tls117-impact.jsonl
   ```
   Pass when: (a)'s `output_cdo.ir-cache.jsonl` (under `target/cdo-gate-logs/<run>/`) has
   `published_packages > 0`; (b)'s passes `ir-cache-report-check --rule warm-build`
   (`disk_hits > 0`, no `refused`, no `roundtrip_failed`); (c) passes `--rule impact`
   (`verified_hits > 0`); and **no line in any of the three says `"ir_cache": "unsupported"`**
   (the checker passes such a file today; the product change makes it fail on Windows once the
   cache is on there).

## 10. Open questions

- **Q6 (closed, rev 3):** ReFS stays off.
- **Q7 (closed, rev 3):** the audit is at the source level, pinned by
  `scripts/check-parse-path-locale`, with the MSVC map listing for a human (section 4.3).
- **Q8 (closed):** an OS refusal to replace or overwrite the running image itself (MoveFileEx
  replace, POSIX rename-replace, a write or delete of the mapped image) is a fact about Windows
  from which no wrong key can follow. The probe prints it as `HELD (the OS protects the running
  image: <error>)`, a pass. Rename-away then a new file, and the folder swap, still FAIL when
  refused or unverified: they are the ordinary installer paths and must be shown to happen.

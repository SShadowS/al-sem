# TLS-117: the IR cache on Windows (design note)

Status: **rev 3**, after sol's reviews r1 and r2 (`/work/.al-sem/tools-notes/tls117-design-review-sol-r{1,2}.md`)
and the orchestrator's decisions on them. No product code yet; the native probe
(`examples/tls117_probe.rs`) is the only code. **The cache stays off on Windows until the probe
passes on a real machine** (section 9).

What rev 3 changes (sol r2): locality is a fixed drive AND NTFS, ReFS stays off (section 3); the
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

**Out of scope:** a process that deliberately redirects an ancestor folder of the image's path
(a rename, junction or mount swap) exactly inside the window between the first name query and
the second, and restores it before the second. That is "another process changing files mid-run";
ENG-110's spec already excludes it. Raw disk writes and a network peer changing a file are out of
scope too; the NTFS rule (section 3) keeps network volumes out entirely.

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

### Rejected alternatives

- **Hash the image in memory.** Not stable: relocations (F7) change with the load address, the
  import table holds per-boot DLL addresses, and `.data` changes as the program runs.
- **`current_exe()` / `GetModuleFileNameW`.** The load-time name (F6): after an installer's
  rename it names the new file, so the key would be the new binary's.
- **A file id for the image itself.** No documented user-mode call gives a handle or file id for
  the file behind our own image section, so identity is proven through the name.

### Design

`image_key()` on Windows:

1. `GetModuleHandleExW(FROM_ADDRESS | UNCHANGED_REFCOUNT, &IR_IMAGE_MARKER)`: the module that
   holds the IR code.
2. `K32GetMappedFileNameW` on its base, buffer 32,768 characters. 0 or a full buffer (truncated,
   F3): no key. The name is kept as **raw UTF-16, never folded or converted**.
3. Open `\\?\GLOBALROOT` + that name: read access, sharing **read only** (no write, no delete),
   not following a reparse point. Any failure, including a sharing violation from a conflicting
   handle: no key.
4. On the opened handle: a regular file (not a directory, not a reparse point) on a **local
   NTFS** volume, both established positively: `GetVolumeInformationByHandleW` names the file
   system `NTFS`, and `GetDriveTypeW` on the handle's volume root (the `\\?\Volume{GUID}\` prefix
   of `GetFinalPathNameByHandleW(.., VOLUME_NAME_GUID)`) is `DRIVE_FIXED`. Anything else (ReFS,
   FAT, a removable or network drive, `\Device\Mup\...`): no key. ReFS stays off (orchestrator,
   rev 3).
5. Hash through that handle (`key_of`, marker search included).
6. Ask the mapped name again, still holding the handle; it must equal step 2's name UTF-16 unit
   for unit. Otherwise no key.

The cache root's volume gets the same local-NTFS rule (section 6).

**Why it holds, within scope.** From step 3 until the handle closes, nobody can rename, delete or
write the opened file F (our handle shares neither write nor delete, F2). At step 6 the image's
file has the name N we opened. One name names one file at a time, so the image's file is F, and
nobody could change F's bytes while we hashed them (F2; F1 for the image itself). The re-check is
what catches an image renamed away between steps 2 and 3 with a new file put at N. It does not
catch an ancestor folder swapped and swapped back inside the window: out of scope (section 1).

**The whole argument rests on W1:** the mapped name must be the file's current name after a
rename. If the probe shows the load-time name, step 6 sees nothing and the design fails; the cache
then stays off on Windows (orchestrator decision Q1).

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
- **The MSVC map listing** (Windows CI job): build `alsem`, `aldump` and `al-call-hierarchy` with
  `-C link-arg=/MAP` and list every deny symbol found in each map (`setlocale`, `_create_locale`,
  `localeconv`, `isw*`, `_isw*_l`, `iswctype`, `isprint`, `tow*`, `GetLocaleInfo*`, `LCMapString*`,
  `CompareStringEx`, `CompareStringW`, `GetStringType*`, `FoldString*`) with the object that
  defines it. A map says what was linked, not what is reachable, so this list does not fail the
  job by itself: a human checks each entry against 4.1 (expected: the static UCRT's own copies, and
  tree-sitter's `lib.obj` if the link keeps the query parser). A failing pin would need
  reachability, which 4.1's check already gives.

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

Tests (product phase):
- `snapshot::cache::tests::windows_private_leaf_under_replaceable_parent`: a leaf whose own DACL
  passes, under a parent granting Everyone `FILE_DELETE_CHILD`: refused.
- `snapshot::cache::tests::windows_permissive_existing_entry`: in a private folder, an entry with
  an Everyone modify ACE: the read refuses it (and the text cache falls back to owned text).
- `snapshot::cache::tests::windows_junction_refused`, `windows_open_leaf_refused`.
- `snapshot::cache::tests::windows_walk_creates_only_under_checked_parent`: a missing leaf under
  a parent that fails the rule is not created.
- The probe's W6 runs the same walk on the real roots: ENG-113's text root, the IR root, and
  `ALSEM_IR_CACHE_DIR` (or a probe-made override when it is unset).

## 7. Prune and publish on Windows

- **Temp names (sol P2).** `prune.rs`'s `is_temp` matches `<stem>-<pid>-<n>-<nanos>.tmp`, the
  Unix `write`'s shape. The Windows `write` uses `tempfile::Builder` with prefix `<stem>-` and
  suffix `.tmp`, which makes `<stem>-XXXXXX.tmp` (6 random characters, tempfile 3.27's
  `NUM_RAND_CHARS`), so the prune would never remove a killed writer's leftovers. Fix: the Windows
  `write` makes its temp file with the same name shape (`OpenOptions::create_new`, the same 16
  tries) instead of `tempfile`, so one pattern covers both systems. Test:
  `program::ir_cache::prune::tests::killed_writer_temp_pruned_after_a_day`, un-gated from `unix`,
  with a temp file made by the real Windows `write` failing mid-way (proof: keep `tempfile`'s
  naming, red).
- **Deletes.** IR entries are read whole and closed; `open_entry` shares delete, so a delete
  normally succeeds while another process reads. Whether the name goes at once (POSIX delete
  semantics) depends on the Windows build; the probe records it (W7). A folder that cannot be
  removed yet goes at the next prune; a virus scanner holding a file is the same case.
- **Publish.** ENG-113's rename-into-place stays (`persist`, `persist_noclobber`, with the new temp
  naming). A rename over an entry another process is reading can fail: a failed publish, a miss.

## 8. The probe

`examples/tls117_probe.rs`. Windows only (on other systems it prints "Windows only" and exits 2).
It needs nothing but the repository: it copies its own executable into scratch folders and starts
the copies as children it drives over stdin. The key sequence in it is a prototype of section 3
(blake3 of the whole file, no marker search). Its extra windows-sys features are a
`[target.'cfg(windows)'.dev-dependencies]` entry, so the shipped binaries do not change.

| Item | What it checks | PASS when |
|------|----------------|-----------|
| W0 | the image's volume, ENG-113's text root, the IR root, and `ALSEM_IR_CACHE_DIR` (or a probe override); for a root not made yet, its deepest existing folder | each is a fixed drive (`GetDriveTypeW`) formatted `NTFS` |
| W1 name | a child renamed while running reports its mapped name | the name ends in the new file name |
| W1 rename away + write, MoveFileEx replace, POSIX rename replace, ancestor folder replaced | the child runs the real sequence with barriers after the query, after the open and after the hash; at one barrier the parent mutates; 3 runs per mutation. The image is `<scratch>\dir\a.exe`. Mutations: rename `a.exe` away and write another file at its name; `MoveFileExW(other, a.exe, MOVEFILE_REPLACE_EXISTING)`; `SetFileInformationByHandle(FileRenameInfoEx)` with `FILE_RENAME_FLAG_REPLACE_IF_EXISTS \| FILE_RENAME_FLAG_POSIX_SEMANTICS`; rename `dir` away (not back), make a new `dir` with another `a.exe` | per run: after a successful mutation, **before the barrier is released**, the old name must open a file with a different `FILE_ID_INFO`, else FAIL ("did not take"); then the key must be the original file's hash or none, never another. A refused mutation **FAILs** ("refused mutation"), except after the open, where a sharing violation or access denied is the key's own handle refusing it and is reported as "blocked while the key's handle is held" (the property F2 claims) |
| W1 hard link | launch through a hard link; delete the launch link (or, if refused, rename it away: both refused FAILs), write another file there (checked), verify its file id differs, while `a.exe` survives | the original hash or no key, before and after |
| W1 other case | launch through an upper-case spelling | the original hash or no key |
| W1 8.3 | launch through the short name of a long-named leaf | the original hash or no key; SKIP only with "8.3 disabled on this volume (no short name was made)" |
| W2 | the whole sequence in the probe itself | the key equals the hash of the probe's file |
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

Checked here: `cargo check --target x86_64-pc-windows-gnu --examples --tests` is clean, and
clippy shows no warning in the probe for the Windows target or Linux. It has not run.

## 9. Order of the owner run (Windows, Git Bash, repository root, `TREE_SITTER_AL_PATH` set)

1. **The probe.**
   ```
   cargo run --example tls117_probe
   cargo test --example tls117_probe -- --test-threads=1 --nocapture
   ```
   Send both outputs back. Any `FAIL` stops here: the design is revised (a W1 failure keeps the
   cache off on Windows).
2. **The cache tests** (after the product change, on its branch):
   ```
   cargo test -p al-sem --lib program::ir_cache
   cargo test -p al-sem --lib snapshot::cache
   cargo test --test cli cli_ir_cache::
   ```
3. **The CDO steps (W8)**, with a fresh cache root that passes the private rule:
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
- **Q8.** The probe's mutations that target the running image's own name (MoveFileEx replace,
  POSIX rename replace) may be refused by Windows at "after-query" because the image section
  forbids deleting the target. Under rev 3's rule that is a FAIL ("refused mutation"). If the
  owner run shows exactly that, is a refusal of a replacement OF the running image an acceptable
  outcome (the replacement cannot happen, so no wrong key can follow), recorded as a fact rather
  than a failure?

# ENG-110: persistent cache of parsed dependency IR (design, rev. 4, 2026-11-21)

Status: a spec for review. No product code. ENG-113 (the package-text cache this one stands on)
has landed; rev. 4 is written against current master (`c80f1391`) and folds in sol's review r1
(5 P1, 2 P2), each point checked against the source first. The measurements come from a
throwaway prototype that never lands: branch `tools/eng-110-proto` (`bf12e42a`), with serde
derives on `al-syntax`'s IR and `examples/eng110_probe.rs`.

"Not verified" marks every claim this revision could not check against source or a run.

## Goal and bar

Every engine process parses and lowers every dependency file again: about 24 CPU-s per process on
CDO, and every gate runs several processes. A cache hit should replace that with a read, a check
and a decode.

**Bar** (how each is measured: Acceptance below):
- (a) **Warm cache, one-shot `alsem analyze` on CDO:** at least 30 % fewer user-mode instructions
  than master, from the one-shot protocol (not cdo-speed: cdo-speed times resident `alsem mcp`
  calls, and its instruction ratio is advisory). The resident model build (cdo-speed's `build`
  call) and a full `cdo-gate`'s wall time are reported, not judged.
- (b) **Byte-identical output**, cache off, cold and warm: goldens, the deterministic output files
  of the CDO commands compared byte for byte, the refresh verify in both modes against a cache-off
  rebuild, `cdo-gate --only harness,l4,output`. Every warm phase must show the expected hit
  coverage (a silent fall back to parsing is identical by definition).
- (c) **A cold miss** costs at most +5 % instructions over master. With the cache off, at most
  +1 %. Peak RSS of a cold miss, alone and four at once, is reported.

## Measured ceiling (server1, CDO baseline `bc3ccb18`, master `a73b4bf6`)

- **One-shot `alsem analyze`** spends 25.4-25.5 CPU-s of 57 in `al_syntax::parse` (parse plus
  lower): 44.7-45.0 % in two samply profiles. With `--single-app` it is 25.4 CPU-s of 30.4:
  every process parses every package.
- **Dependencies** are 96 % of the source bytes: 10,881 of 11,463 files, 115.5 of 120.4 MiB, in
  11 packages (`probe3.log`). So dependency parsing is ≈24 CPU-s a process.
- **In instructions:** the parser and lowerer symbols' self shares in ENG-112's `perf record` are
  ≈32 % of P-core and ≈53 % of E-core user instructions, ≈39 % combined. A lower bound: it leaves
  out the allocator under the parse, and 12,464 samples were lost.
- **The prototype**, single thread, all dependency files, in `/work/.al-sem/profiles/2026-11-18-eng110`
  (`probe.log`, `probe2.log`, `probe3.log`; a fourth run's log was overwritten):
  - parse+lower: 11.4-12.9 CPU-s;
  - encode: 0.27-0.29 CPU-s;
  - decode: 0.32-0.37 CPU-s, 2.6-3.2 % of parse+lower;
  - size: 143.9 MiB postcard (78.5 MiB deflated).

  The engine's parse is ~2× the single-thread figure: its pool runs on all 22 threads. The probe's
  own parallel parse read ~4.3× (49.64 CPU-s, `probe3.log`) on the system allocator; the engine
  uses mimalloc. These logs support a codec-cost estimate only. **The 30 % end-to-end claim is
  unproven until the one-shot protocol measures it** (sol r1).
- **Not in the probe:** the blake3 pass over each entry (~144 MiB, tens of ms at blake3 speed;
  not verified on server1), the read into an owned buffer, and the IR check pass. All three are
  inside the warm measure.
- **Heap.** The IR held after a decode is 476.5 MiB in 4.66 M allocations, against 498.3 MiB in
  4.31 M after a parse (counting allocator in the probe). Shared `Arc<str>` type texts decode
  separately (+8 % allocations), but the total is lower. The engine census confirms it per process.
  Exact-size arenas after a decode are not guaranteed (serde grows them by doubling); `exact_size`
  runs on them after decode, as after a parse.
- **The largest package's entry size is not measured** (no per-package line in the probe). It sets
  the owned-read peak (Design 4). Measured in the first implementation step.

## The key: what the IR depends on

`al_syntax::parse(source)` is a function of the text, the grammar compiled in, the tree-sitter
runtime, the lowerer's code and the toolchain. `char::to_lowercase`'s tables feed
`another_declaration_lacks`. No preprocessor symbols and no app context reach it: `parse(&str)`
takes the text alone, TLS-67 builds a fresh `tree_sitter::Parser` per file
(`crates/al-syntax/src/parse.rs`), and `#if` arms are kept in the IR. Lowerer thread-locals are
scratch only after the change in "Lowerer state" below.

An entry is valid only for one text and one `IR_KEY`. A false miss costs a parse; a false hit is a
wrong answer. The key over-approximates on purpose.

### The text: ENG-113's verified digest, carried, never re-derived

- ENG-113's entry header holds a blake3 over the package's content hash, its JSON index (every
  path, offset and length, in order) and its text (`src/snapshot/cache.rs`, `check_entry`). That
  value is the **text digest**. It names the exact ordered `SourceRoot.files`, paths included,
  under the current text format (`s9.f2`).
- **ENG-110 carries it; it never reopens the text entry by path** (sol r1 P2):
  - `CacheDir::read` returns the verified header digest with the mapped files.
    `cached_source_in` returns it as a third value: `Some` only for a mapped, verified entry (a
    hit, or the read-back after a publish); `None` for the owned fallback.
  - `SourceRoot` gains `text_digest: Option<[u8; 32]>`. `EmbeddedAppProvider` sets it.
    `DepCache`'s `WeakSource` keeps it, so `upgrade` and `deferred` carry it. Every other
    construction (workspace, source folders, tests, LSP) sets `None`.
  - `None` means no IR cache for that package: owned text, workspace files and source-folder
    dependencies are never cached.
- The IR file is named by the digest (Design 2), so builds whose extraction output differs never
  share a name and never evict each other.

### `IR_KEY`: a build-time hash closed over what produces and reads the IR

sol r1 P1-2: rev. 3 left out the tree-sitter runtime, the C compiler and its flags, features and
cfgs, the target, local overrides and the codec code outside `al-syntax/src`. `IR_KEY` is a
sha256 over these parts, in this order, each part length-prefixed:

1. **Grammar part**, computed by `crates/al-syntax/build.rs` (which compiles the grammar today):
   - every file under the grammar's `src/` (`parser.c`, `scanner.c`, `scanner.cc` if present,
     `unicode_id.h`, `tree_sitter/*.h`, `node-types.json`), in sorted relative-path order, each
     path and length hashed. A `scanner.c`-only edit that leaves `node-types.json` unchanged moves
     it. Not `node-types.sha256`, not `CACHE_VERSION_GRAMMAR`
     (`src/engine/gate/cache_prune.rs`, `"tree-sitter-al-v5.4.0-88bb6871-native"`): a grammar
     change can alter how the same text parses with no new node type (v4.0.0 moved dangling
     `else`), and the constant kept a stale commit once.
   - the C compiler's identity and flags: `cc::Build::get_compiler()`'s path, its `--version`
     output and its arguments (these include the C opt level and the `c11` standard).
   - `al-syntax` gains `links = "tree-sitter-al"` (it compiles a native library, so `links` is
     allowed) and prints `cargo:grammar_hash=<hex>`. The grammar commit (`88bb6871` today, from
     `git -C <grammar> rev-parse HEAD` when it is a checkout) is printed beside it for diagnosis
     only; it is not a key input.
2. **Lowerer and IR part:** every file under `crates/al-syntax/` (its `src/`, `build.rs` and
   `Cargo.toml`), sorted, paths and lengths hashed.
3. **Codec part:** every file of the envelope and check code in `al_sem`
   (`src/program/ir_cache/`, a new folder holding only that code).
4. **Dependency part:** the whole `Cargo.lock`. The crates whose code runs between the text and the
   decoded IR are `tree-sitter`, `serde`, `serde_derive`, `postcard`, `blake3` and `cc`. Each must
   appear in `Cargo.lock` with a registry source and a checksum (registry content for a version
   and checksum cannot change). **If any of them does not** (a `[patch]`, a path or git source),
   the build exports `IR_KEY = None` and the cache is off in that binary, logged once at
   info. It fails closed instead of guessing.
5. **Build configuration part:** `rustc -vV` (version, commit, host, LLVM version), and every
   `CARGO_CFG_*`, `CARGO_FEATURE_*` and `CARGO_PROFILE_*` variable the build script sees, plus
   `TARGET`, `PROFILE`, `OPT_LEVEL`, `DEBUG` and `CARGO_ENCODED_RUSTFLAGS`, sorted by name. The
   cfg set holds `target_pointer_width`, `target_endian`, `target_feature` and `debug_assertions`.
   Also the root `Cargo.toml` (it holds every `[profile]`, including per-package overrides) and
   every cargo config file cargo reads for this build (`.cargo/config.toml` and `.cargo/config` in
   the workspace root and its ancestors, and in `$CARGO_HOME`).

   This **partitions by configuration**; it does not prove configurations interchangeable. A
   `dev` test binary and a `release-fast` `alsem` never share entries. Cost: one cold fill per
   configuration that runs with the cache on (in practice `release-fast`, `release-ship` and the
   `cdo` profile).

The root `build.rs` combines parts 2 to 5 with the grammar hash (`DEP_TREE_SITTER_AL_GRAMMAR_HASH`)
and exports `IR_KEY` (or `None`) as an `al_sem` constant (`cargo:rustc-env` reaches only the crate
that sets it, so the key lives where it is used). Its `rerun-if-changed` covers
`crates/al-syntax/`, `src/program/ir_cache/`, `Cargo.lock`, `Cargo.toml` and the config files it
read; `rerun-if-env-changed` covers the named variables.

**Tests:**
- **Key moves:** in a temporary copy of the inputs, a byte changed in each part (a lowerer file, a
  codec file, `scanner.c` alone, `Cargo.lock`, one cfg value) moves the key. Discrimination proof:
  drop the folder or variable from the hash, the test goes red.
- **The build trigger is pinned (kept from rev. 3, Fable r2 N1):** the root `build.rs` also exports
  the hash of parts 2 and 3 (`IR_KEY_SOURCES`). A `ci-steps test` test re-hashes those folders at
  run time and asserts it equals the baked value, so a key that did not rebuild fails the task
  gate. Discrimination proof: drop a `rerun-if-changed` line, edit a lowerer file, rebuild, red.
- **Real rebuilds miss:** two binaries built from trees that differ only in a lowerer file, and two
  that differ only in profile, get different key folders; a test runs both on one fixture package
  in one cache root and sees two entries and no cross hits.
- **Fail closed:** a `Cargo.lock` copy with `tree-sitter` given a path source yields `None`.

## The codec

1. **serde derives on the IR types**, always on (Fable r1 P3: a feature would be a second build
   shape nobody tests). `al-syntax` gains a `serde` dependency (already in the workspace's lock).
2. **`Origin`** stores the grammar's symbol id (`kind_id: u16`; `crates/al-syntax/src/ir/mod.rs`),
   with `u16::MAX` for tree-sitter's builtin `ERROR`. The declaration pack's positional
   `RawKind::ALL` codec (`src/program/pack/mod.rs`) covers named kinds only and is not reused
   (sol r1 P1-2). The id is encoded as is and **validated on decode**: it must be `u16::MAX`, or
   below the grammar's `node_kind_count` with a non-empty name. That set is total: named and
   anonymous kinds and `ERROR` all pass, and any other value refuses the file. Ids are stable
   within one key because the grammar part covers `parser.c`. Decoding keeps the exact id, so the
   alias case (two ids, one name; `Origin`'s `PartialEq` compares names) round-trips exactly too.
3. **`ActionDecl.kind`** (`&'static str`, the only one in the IR) gets a hand-written pair: write
   the keyword, read it back into its static set (`action`, `systemaction`, `fileuploadaction`,
   `customaction`), refuse any other. The prototype's `#[serde(skip)]` there is NOT the design.
4. **Bounded decode** (sol r1 P1-1): each file decodes from its own index range of the verified
   buffer, never past it, with no trailing bytes (`postcard::take_from_bytes`, empty rest). serde's
   derived sequence decoding caps its preallocation (`size_hint::cautious`; not verified for the
   pinned serde/postcard versions, checked in the implementation), so a length claim cannot
   allocate more than the bytes behind it can fill. A test asserts the IR has no `Vec` of a
   zero-sized type (whose elements cost no bytes).
5. **IR check before use** (sol r1 P1-1): `AlFile::check(text_len)` in `al-syntax` (the arenas are
   private there) runs on every decoded file before it reaches a consumer:
   - every `ExprId`, `StmtId`, `BlockId` and `MemberOriginId` held anywhere (decls, nodes, side
     tables such as `tainted_calls`, `locals_incomplete`, `signature_varies`, `preproc_ranges`) is
     inside its arena;
   - every node's child ids are lower than its own id, so no walk can loop. This holds for
     expressions (children are pushed first); **for statements and blocks it is not verified**.
     If the lowerer does not push them in that order, the check walks from the roots with a depth
     bound of `MAX_LOWER_DEPTH` and a visited set instead.
   - every `Origin` has `byte_start <= byte_end <= text_len` and `start <= end`, and a valid kind id;
     every `VarWrapper` byte range and arm range is inside the text;
   - sorted side tables (`tainted_calls`, `preproc_ranges` starts) are sorted.

   A failed check refuses that file (Failure cases). The check's cost is inside the warm measure.
6. **Structural equality** (kept from rev. 3, Fable r1 P1-2): byte equality of re-encodings cannot
   see a skipped or defaulted field. `PartialEq` and `Debug` are derived on `AlFile` and every IR
   type that lacks them, and the checks assert `decoded == fresh`. A test reads
   `crates/al-syntax/src/ir/` and fails on any `serde(skip` or `serde(default`.

## Lowerer state: scratch, proven (a small product change ENG-110 makes)

sol r1 P1-3, checked in `crates/al-syntax/src/lower/mod.rs`. The lowerer keeps six thread-locals:
- `CONDITIONAL_VARS` is cleared, and `PREPROC_WALK` and `PREPROC_RANGES` are set, at the top of
  `lower_file`. `VARS_INCOMPLETE` is set false before each read. `shared_type_text`'s `TEXTS`
  (`ir/decl.rs`) decides sharing only, never a value.
- **`BUILD_CONTEXT` is the hole.** `with_build_context` extends it, runs the body, and truncates it
  only on a normal return. `lower_file` never clears it. A panic inside a body (caught by an
  engine `catch_unwind`, or a rayon worker that survives a panicking job) leaves stale symbols on
  that thread. The next file lowered there records them as its expressions' build context: a wrong
  IR. Without a cache that wrong IR lives one process. With a cache it would be written to disk
  and served to every later run.

**The change** (part of ENG-110, in `al-syntax`):
- `lower_file` clears `BUILD_CONTEXT` at its top, beside `CONDITIONAL_VARS`.
- `with_build_context` truncates through a drop guard, so an unwind restores the outer length too.
- The thread-local's doc comment says both.

**Tests** (hand-stated preconditions; one discrimination proof per guard):
- **Unwind restore:** on a one-thread pool, `catch_unwind` around `with_build_context` with a body
  that panics; then assert `BUILD_CONTEXT` is empty on that thread. Proof: remove the guard, red.
- **Entry reset:** seed `BUILD_CONTEXT` on a thread with a stale `("STALE", true)` by hand, then
  lower a fixture whose routine body has `#if` code, once clean and once `Recovered`, on that
  thread; each must equal the same file lowered on a fresh thread. Proof: remove the clear at the
  top of `lower_file`, red.
- **Purity on real data** (kept, `probe3.log`): all 10,881 dependency files lowered in parallel in
  reverse order equal the sequential lowering (0 differed). It shows no order effect on clean runs;
  it cannot show unwind safety, which the two tests above do.

## Design

1. **Built on ENG-113 as shipped** (`src/snapshot/cache.rs`; note
   `docs/superpowers/notes/2026-11-18-eng113-mapped-dependency-text.md`). ENG-110 reuses its
   `CacheDir`, made `pub(crate)` and given an owned read, not a second scheme:
   - the directory is opened once and checked on the handle (Unix: this user's, no group or other
     write bit; else the IR cache is not used, no read and no write);
   - entries are opened relative to that handle without following a symlink or blocking, and
     must be regular files private to this user;
   - writes go to a unique owner-only temp file (`O_CREAT | O_EXCL`, 0600, pid/counter/time name);
   - publication is no-clobber (`linkat`) for a missing entry and replace (`renameat`) only for an
     entry this process refused, both through the directory handle;
   - every publish holds the per-entry `flock` (`<digest>.lock`, a private regular file) and reads
     the entry again under it: an entry another process published meanwhile is used and nothing is
     written. Without the lock nothing is published. Windows: no lock; a no-clobber persist, and a
     replace only of a refused entry.

   So rev. 3's "last rename wins with identical bytes" is gone: the first valid entry is kept, as
   for the text.
2. **Layout:** `<root>/ir/<IR_KEY, 64 hex>/<text digest, 64 hex>.bin`, where `<root>` is ENG-113's
   cache directory unless `ALSEM_IR_CACHE_DIR` names another (the acceptance's fresh cold roots
   and the cache's own tests use it; the text cache stays where it is). The full key and the full
   digest are in the path (sol r1 P2), so binaries with different keys and texts with different
   extraction output never touch each other's entries.
3. **Envelope** (sol r1 P1-1; all integers little-endian):
   - header: magic `ALSEMIR\0`; format (`u32`, starts at 1); the full `IR_KEY` (32 bytes); the text
     digest (32 bytes); the file count (`u64`); the payload length (`u64`);
   - index: `count` records of `(offset u64, len u64)`, in `SourceRoot.files` order;
   - payload: every file's postcard bytes, concatenated;
   - trailer: a blake3 over every byte before it (header, index and payload).

   **A hit requires, in this order:** the entry no longer than the per-entry cap (Design 5); the
   magic and format; the key equal to this binary's; the digest equal to the `SourceRoot`'s; the
   count equal to `SourceRoot.files.len()`; the length exactly header + `16 × count` + payload +
   32; the blake3; ranges that tile the payload exactly (the first at 0, each starting where the
   previous ended, the last ending at the payload's end, every sum checked before use). Then, per
   file: decode with no trailing bytes, then `AlFile::check(text_len)` against that file's own
   text. Two index records pointing at each other's blobs are refused by the hash (it covers the
   index) and by the tiling rule, and a decoded tree is checked against the text it is attached
   to.
4. **Owned, bounded reads, not a map** (decision, sol r1 P1-4). The entry is read into one owned
   buffer (capped), hashed, decoded from that buffer, and the buffer is dropped after the package.
   - Why not mmap: ENG-113 maps its text because the page cache then holds one copy for every
     engine (the point of ENG-113). The IR is decoded into owned heap anyway; a map saves one copy
     of a buffer that lives for one package's decode. Against that, a map cannot turn an in-place
     truncate or rewrite (an outside `cp`, `rsync --inplace`, a restore) into a miss: it raises
     SIGBUS, or changes bytes between the hash and the decode, so the decoded IR is not what was
     verified. With an owned buffer the bytes decoded are the bytes hashed, and any damage is a
     refusal and a rebuild. Correctness wins (mantra 1).
   - Cost: one package's entry on the heap during its decode. Packages are taken one at a time
     (the loop over units is sequential; files inside it decode in parallel from the one buffer),
     so the peak is the largest package's entry (not measured; Ceiling). Reported in the cold and
     warm peaks.
5. **Disk bounds** (sol r1 P2):
   - **Per entry:** 1 GiB, written and read (CDO's whole set is 143.9 MiB). A package past it is
     never written and its entry never read: a parse, logged once.
   - **Total:** 4 GiB under `<root>/ir/` (about 25 CDO-sized key folders). When maintenance finds
     more, it removes whole key folders, the oldest `last-used` first, never this process's own.
     If this process's key folder alone passes the cap, it writes no new entry and logs once at
     warn.
   - Both are constants of my choosing (orchestrator to confirm). Neither has a user setting.
6. **The seam** (kept from rev. 3, Fable r1 P2-2; lines re-checked on `c80f1391`). Dependency
   packages are parsed in `src/program/dep_summary.rs`: `parse_for_build_observed` (line 62; the
   per-file call at 122) and `parse_dep_bodies` (line 175; the call at 195), each calling
   `snapshot::parse::parse_file` per file inside a `par_iter` on the big-stack pool. One function
   serves both:
   - per package, once, before the `par_iter`: the owned read and the envelope checks;
   - inside the existing `par_iter`, per file: decode and check file `i` in place of `parse_file`
     (on a refused file: `parse_file`, see Failure cases).

   A profile that summarizes and drops each tree keeps today's bound of about one tree per thread
   (`LiveTrees`). On a miss the same loop parses and also encodes each file; the encoded buffers
   of that ONE package are published when its loop ends, then freed. The extra peak on a miss is
   one package's encoded bytes.
7. **Small packages are skipped** (kept from rev. 3, Fable r1 P2-3). Below one fixed constant a
   parse beats a read, a hash and a decode; no IR file is written. It is measured once on the
   prototype (cost by package size) and written down with its table. A package with zero files
   writes nothing.
8. **Maintenance** (sol r1 P2; rev. 3 pruned only on a miss and marked a key once per process):
   - **`last-used` marker:** `<key folder>/last-used`, an empty file. It is **never written in
     place**: a process publishes a fresh temp file over it by rename. It does so at its first hit
     or write, and again at any later use when the marker is older than one day, so a resident
     engine that lives for weeks keeps its key alive.
   - **Throttle:** at most one prune per root per hour, decided at the first cache USE of a process
     (hit or miss, not only a miss): when `<root>/ir/last-prune` is older than an hour, the process
     takes a non-blocking `flock` on `<root>/ir/prune.lock` (Unix; Windows: none), publishes a new
     `last-prune` by rename, and prunes. A process that cannot take the lock skips the prune.
   - **What a prune removes:** key folders whose `last-used` is older than 7 days, as
     `scripts/sweep-builds` does for build output; then the total cap; temp files with this
     cache's name shape older than one day (a killed writer leaves up to one package's entry).
   - **Only what it can prove it made** (mantra 24): names of the exact shapes above (`ir/`, a
     64-hex folder, `<64 hex>.bin`, `<64 hex>.lock`, `last-used`, the temp name shape), opened
     through the checked directory handle, never following a symlink. Anything else under `ir/`
     is left alone.
   - A prune racing a writer of an old key is safe: the publish fails, and that process keeps its
     parsed IR.
   - `alsem cache prune` (`src/engine/gate/cache_prune.rs`) maintains a different cache (JSON
     dependency artifacts) and is not changed.
9. **`alsem batch`: the IR cache is off in the batch parent and its workers** (decision, sol r1
   P1-4). See the next section.
10. **`parse_snapshot` stays uncached on purpose** (kept from rev. 3). Its callers outside
    `dep_summary` are test modules (`lsp/custom.rs`, `lsp/handlers.rs`, `dep_summary`'s tests).

## `alsem batch` (TLS-109)

TLS-109 (`src/engine/batch/claim.rs`) claims every file a batch run writes by its identity before
truncating it, and claims the files the run reads. Its doc exempts the engine's caches because each
publishes by renaming a fresh temp file. Two gaps for ENG-110, as sol found:
- the claims do not reserve cache destination names, so a job whose `--out` lies inside the cache
  folder could have its file replaced by a publish (a rename does not truncate, but it does unlink
  the job's name);
- rev. 3's `last-used` was an in-place write (rev. 4 removed that: Design 8).

**Decision: off in batch for the first release.** When `claim::dir()` is `Some` (the batch parent,
and every worker, which refuses to run without it), the IR cache neither reads, writes nor prunes.
Weighed:
- **For off:** it is the only choice that needs no new claim design. A batch run already builds one
  model per view and shares it across its jobs, so its share of parse time is lower than a
  one-shot's. No gate and no script runs `alsem batch` today (checked: `scripts/` has no caller;
  the callers are `tests/cli` and `tests/perf_bounds.rs`, which run with the cache off anyway).
- **Lost:** a run of two or more batches starts one worker process per batch (`worker.rs`), and
  each parses every dependency; those runs keep today's cost. How often outside tools run multi-
  worker batches is not known here.
- **Later:** a follow-up task (to file) turns it on with the claim check reserving the IR folder's
  names (a job output under the cache root refused at the check) and a test for direct-path and
  alias conflicts (a job `--out` equal to, or hard-linked to, an entry path).

The ENG-113 text cache already runs in batch and has the same name gap; that is outside ENG-110 and
goes in the same follow-up.

**Test:** with `claim::set_dir` set, a build over a fixture package reads and writes nothing under a
temporary IR root (hit and miss counters zero, the root empty). Proof: drop the check, red.

## Correctness checks

- **Decoded equals parsed, by structure.** A test decodes every CDO dependency file and compares it
  with a fresh parse (`decoded == fresh`). It runs in the CDO subset (`lib_cdo`, `cdo` in its name).
- **Determinism:** a test encodes the CDO set twice, in different thread orders, and gets the same
  bytes. A refused file's fresh encoding then matches what a cold fill writes (Failure cases).
- **VERIFY mode, in production shape** (kept from rev. 3). With `ALSEM_IR_CACHE_VERIFY=1` every hit
  is parsed anyway and compared by structure with the decoded IR. A mismatch fails the process
  loudly, naming the package, the file and the key; it is never a silent fallback. `cdo-gate` sets
  it for one phase, `program_resolve_harness`, for good, on the user's real cache (a key hole
  shows only on an entry an EARLIER build wrote). Zero hits there labels the phase "VERIFY: no
  hits", a label, not a failure. **VERIFY never runs in a timed run**: cdo-speed and the one-shot
  protocol unset it and refuse to start when it is set.
- **Tests still exercise the parser** (kept from rev. 3, Fable r2 N2): `.cargo/config.toml`'s
  `[env]` sets `ALSEM_IR_CACHE=off` for every process cargo starts, which the spawned `alsem` and
  `aldump` inherit. An explicit `ALSEM_IR_CACHE=on` in the environment wins (cargo's `[env]` does
  not override a set variable unless `force = true`; from cargo's documentation, not re-tested
  here). Binaries started directly (`target/cdo/aldump`, `target/release-fast/alsem`) do not see
  `[env]`: the cache is on there by default.
- **Every identity phase sets `ALSEM_IR_CACHE=on` explicitly** (sol r1 P1-5), since the cargo
  default is off.
- **Hit coverage, per process:** with `ALSEM_IR_CACHE_REPORT=<file>` each process appends one JSON
  line at exit: argv[0] and command, key, and counts of files hit, missed, refused, skipped as
  small, and skipped in batch. The counters also go to the perf trace and the census. `cdo-gate`
  points every phase at its own report file and prints each phase's sums.
- **`ParseStatus::Recovered`:** its doc says "do not cache as authoritative". The IR cache replays
  it exactly, flag included, so consumers see what a parse gives. The doc is fixed to say so.

## What still needs the source TEXT after an IR hit

(Kept from rev. 3.) The IR stores names and literal text inline, plus byte ranges (`Origin`), but
not the source. These read the text, so ENG-113's mapped text stays the one copy and the IR cache
holds none: line and column positions (`LineTable`); source snippets in explain and digest output;
the row cache's identity fast path (`SourceText::ptr_eq`); `recovered_file_paths` and diagnostics
that quote source.

## Many processes filling the cache at once

On a cold cache each gate process misses and parses (today's cost) and encodes. At a package's end
it takes the entry's lock and reads the entry again: if another process published it meanwhile, it
writes nothing (Unix). So on Unix the write amplification is one entry per package plus temp
bytes of processes that lost the race before taking the lock (none: the lock is taken before the
temp file is created). On Windows (no lock) a loser writes a temp file and its no-clobber persist
fails; at most one temp per process per package. No lock spans the gate's start: that would
serialize it for a one-off saving. Measured: Acceptance step 5.

## Failure cases

- **The envelope is refused** (too long, magic, format, key, digest, count, length, hash, ranges):
  the whole package parses; at its end the entry is replaced under the lock (a refused entry is
  the one case that replaces). The next run hits.
- **A file's decode or check fails** under a valid hash (a codec defect, or a hand-built entry):
  that file parses; the other files keep their decoded IR. At the package's end the entry is
  replaced: the verified slices of the good files plus the fresh encodings of the refused ones
  (the codec is deterministic, so this equals a cold fill). Warned once per package, counted as
  refused.
- **Truncated or rewritten in place during a read:** the read is short or the hash fails; a refusal.
  No SIGBUS (owned reads).
- **The cache is not writable or not private:** no IR cache, logged once at warn.
- **Every refusal path is tested** with a hand-written broken entry in a temporary root: each
  envelope field, two index records swapped with the hash left as written (refused by the hash)
  and with the hash recomputed (refused by the tiling rule: the first record must start at 0 and
  each where the last ended), a gap, an overlap, a trailing byte, an over-cap length, an invalid
  kind id, an out-of-arena id, an out-of-text range, an unknown action keyword. Each test asserts
  that the model built equals the cache-off model, and that a second run hits the repaired entry.
- **Threat model** (mantra 26): the hash is an integrity check, not a signature. Someone who can
  write this user's cache can also replace this user's binaries; swapping two equal-length payload
  blobs and recomputing the hash is not defended beyond the per-file check. Other users are kept
  out by the private-directory and private-entry rules.

## Acceptance (sol r1 P1-5)

Every measurement records the master and head SHAs, the workspace SHA (`bc3ccb18`) and the `.app`
closure stamp (`dependency_closure_sha256`, `closure-v2:…`); a change in either voids the run.

1. **Identity, cache off / cold / warm** (all with `ALSEM_IR_CACHE=on` set explicitly except "off"):
   - `scripts/check-goldens` with the cache off (the default) and once with it on and warm;
   - the deterministic output files of the CDO commands that `cdo-before-after` runs (fingerprint,
     analyze primary and `--scope all`, the call-graph histogram), plus explain, query and digest
     outputs from `output_cdo`, compared **byte for byte** between head-off and head-cold, and
     head-off and head-warm. Not `cdo-before-after`'s summary. A file that differs between two
     head-off runs is nondeterministic: it is listed, filed as a bug, and left out only of this
     comparison;
   - `aldump --session-refresh <copy> --verify` in both modes, with the cache warm, its reference
     cold build made with the IR cache off (a switch in `aldump`'s verify path);
   - `cdo-gate --only harness,l4,output`, cache warm, VERIFY on in `harness`.
2. **Hit coverage:** in every warm identity phase and every warm timed run, each process's files hit
   equal its dependency files minus the small-package skip, and refused is zero. Checked per command
   and per phase from the report files, not as one total.
3. **One-shot timing** (the bar (a) and (c) numbers): `alsem analyze <CDO_WS>` (cross-app, the
   default), `release-fast`, in a timed CDO slot (`scripts/cdo-slot run … --timed`), pinned to the
   P-cores (`taskset -c 0-11`), under `perf stat -e instructions:u,cycles:u` over the whole process
   tree. Counts are kept only when the counter covered the whole run (no multiplexing: 100 %
   running); others are discarded and repeated.
   - Four arms, alternated in a rotating order, at least 5 rounds: **master**; **head-off**
     (`ALSEM_IR_CACHE=off`); **head-cold** (a FRESH, empty `ALSEM_IR_CACHE_DIR` per sample, so only
     the IR is cold); **head-warm** (one IR root filled once, untimed, before the rounds).
   - The ENG-113 text cache is the same warm one for every arm: one untimed run of each binary
     first.
   - Judged on instructions: head-warm/master ≤ 0.70; head-cold/master ≤ 1.05; head-off/master
     ≤ 1.01. CPU and wall are reported beside them; a CPU ratio is quoted only as the CLAUDE.md
     rule allows (two runs agreeing within ±12 %). A suspend or pressure label voids that round.
4. **Resident build and gate:** cdo-speed twice (its `build` call cold and warm is reported, not
   judged), and a full `cdo-gate`'s wall time, cache warm.
5. **Memory and disk:** the engine census, warm against master (the IR bucket); peak RSS of one
   cold one-shot, and of four cold one-shots started together on one fresh IR root, with the temp
   bytes seen and the bytes written against the entries' size (write amplification); the key
   folder's size for CDO; the largest entry; the small-package table.

**cdo-speed after ENG-110 lands.** Its two sides can have different keys (any `al-syntax` edit). If
one side's IR is warm and the other's cold, its build ratio reads a ~40 % swing that is not the
change. So cdo-speed fills each side's key folder in one untimed call before its warm-up, and prints
each side's hit count in the verdict; a side with zero hits on a timed call labels the verdict.

## Settled in review

1. **`IR_KEY`'s inputs:** closed over the grammar sources, the C compiler and flags, the tree-sitter
   runtime and codec crates (by lockfile checksum, else off), the lowerer, the envelope code and the
   build configuration (partitioned). Opt level and target features are in the partition, not
   argued away. `MAX_LOWER_DEPTH` is a constant 128, not read from the stack.
2. **VERIFY:** in one gate phase for good; never in a timed run.
3. **The small-package threshold:** one fixed constant, measured once.
4. **mmap or owned reads:** owned, bounded reads (Design 4).
5. **`alsem batch`:** off for the first release (section above).

## Left for the owner

- **Cargo overrides the build cannot see.** A `--config` argument on the cargo command line (for
  example a `paths` override) is not visible to a build script, and a `paths` override does not
  change `Cargo.lock`. No repo script uses either. Closing it fully would mean keying on the
  compiled crates themselves, which a build script cannot read. Ask: accept "the repo's scripts
  never pass `--config`" as the rule, or ask for another way.
- **The disk caps** (1 GiB per entry, 4 GiB total) are my choice.
- **Batch off** loses the gain for multi-worker batch runs until the follow-up lands.

## Builds on ENG-113

- ENG-110 adds the text digest to `CacheDir::read`'s and `cached_source_in`'s results and to
  `SourceRoot` and `DepCache` (the digest section). ENG-113's behaviour does not change.
- ENG-110 makes `CacheDir` and its lock reusable by the IR cache and adds an owned, capped read.

## Owner design principles applied

From `/coord/owner/design-mantra.md`:
- **1 (correct first)** and **5 (unknown is not no):** owned reads over mmap so damage is always a
  refusal; a key that cannot be closed turns the cache off rather than guessing; a per-file check
  before any consumer sees decoded IR.
- **3 (root cause):** the `BUILD_CONTEXT` leak is fixed in the lowerer, not worked around in the
  cache.
- **4 (measure, new gates advisory)** and **9 (commit to the expected result):** the one-shot bar
  is stated before the run and judged on covered instruction counts; the hit-coverage labels in
  cdo-gate start as labels.
- **7 and 8 (tests that can fail, real use):** every guard has a stated break; the coverage and
  identity checks run the real commands on CDO.
- **10 (reproducible baselines):** each measurement pins the workspace SHA and the `.app` closure.
- **13 and 14 (right-sized, reuse):** ENG-113's checked directory, lock and publish rules are reused,
  not copied; no new user setting; batch is switched off instead of growing a claim design now.
- **16 (clean cutover):** the `off` switch exists for tests and the off arm of the measure only; it
  is not a user mode.
- **24 (delete only what you made):** pruning removes only names of this cache's exact shapes.
- **26 (state the threat model):** the hash is integrity, not a signature; the spec says what it
  does not defend instead of adding a weak guard.
- **35 (file work when found):** the batch follow-up is a task to file, not a note.

## Review history

- **rev. 1:** written.
- **rev. 2:** Fable r1 folded in (3 P1, 6 P2, P3s), plus the orchestrator's VERIFY, tests and
  measures additions.
- **rev. 3:** Fable r2 folded in. N1: the build trigger is pinned by a baked-hash test. N2: one home
  for the test switch (`[env]`) and its precedence. N3: VERIFY labels zero hits. Plus its P3s.
- **rev. 4:** sol r1 (blocked rev. 3; 5 P1, 2 P2) folded in, each point checked on `c80f1391`.
  P1-1: the hash covers the whole envelope, exact count, tiled ranges, bounded decode,
  `AlFile::check`. P1-2: the key closes over grammar sources, C compiler, runtime and codec crates,
  codec code and build configuration; validated symbol ids. P1-3: the `BUILD_CONTEXT` reset and
  unwind-safe restore, with tests. P1-4: ENG-113 as shipped (lock, no-clobber), owned reads, batch
  off. P1-5: the one-shot protocol, byte comparison, per-phase coverage, closure pin. P2: the
  digest in the path, disk caps, throttled maintenance, concurrent cold-fill measure.

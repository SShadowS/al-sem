# ENG-110: persistent cache of parsed dependency IR (design, rev. 5, 2026-11-21)

Status: a spec for review. No product code. ENG-113 (the package-text cache this one stands on)
has landed. Rev. 5 is written against master `c80f1391`. It folds in sol's review r2, Fable's
review r3 and the orchestrator's decision to key the cache on the running binary. Each point was
checked against the source first. The measurements come from a throwaway prototype that never
lands: branch `tools/eng-110-proto` (`bf12e42a`), with serde derives on `al-syntax`'s IR and
`examples/eng110_probe.rs`.

"Not verified" marks every claim this revision could not check against source or a run.

## Goal and bar

Every engine process parses and lowers every dependency file again: about 24 CPU-s per process on
CDO, and every gate runs several processes. A cache hit should replace that with a read, a check
and a decode.

**Bar** (how each is measured: Acceptance below; the judgment rule is fixed there before any run):
- (a) **Warm cache, one-shot `alsem analyze` on CDO:** at least 30 % fewer user-mode instructions
  than master. The resident model build (cdo-speed) and a full `cdo-gate`'s wall time are
  reported, not judged by this bar.
- (b) **Byte-identical output:** master, head with the cache off, cold and warm all give the same
  bytes; goldens; the refresh verify in both modes against a cache-off rebuild;
  `cdo-gate --only harness,l4,output`. Every warm run must show full hit coverage over a nonzero
  count of files that reached the seam.
- (c) **A cold miss** costs at most +5 % instructions over master; the cache off at most +1 %.
  Because the key is the binary (below), every new build pays (c) once, so (c) is also the price of
  each rebuild. Peak RSS of a cold miss, alone and four at once, is reported.

## Measured ceiling (server1, CDO baseline `bc3ccb18`, master `a73b4bf6`)

- **One-shot `alsem analyze`** spends 25.4-25.5 CPU-s of 57 in `al_syntax::parse` (parse plus
  lower): 44.7-45.0 % in two samply profiles. With `--single-app` it is 25.4 CPU-s of 30.4.
- **Dependencies** are 96 % of the source bytes: 10,881 of 11,463 files, 115.5 of 120.4 MiB, in
  11 packages (`probe3.log`). So dependency parsing is ≈24 CPU-s a process.
- **In instructions:** the parser and lowerer symbols' self shares in ENG-112's `perf record` are
  ≈32 % of P-core and ≈53 % of E-core user instructions, ≈39 % combined; a lower bound.
- **The prototype**, single thread, all dependency files, in `/work/.al-sem/profiles/2026-11-18-eng110`
  (`probe.log`, `probe2.log`, `probe3.log`):
  - parse+lower: 11.4-12.9 CPU-s;
  - encode: 0.27-0.29 CPU-s;
  - decode: 0.32-0.37 CPU-s, 2.6-3.2 % of parse+lower;
  - size: 143.9 MiB postcard (78.5 MiB deflated), 1.25× the source bytes.

  The probe's parallel parse read ~4.3× (49.64 CPU-s) on the system allocator; the engine uses
  mimalloc. These logs support a codec-cost estimate only. **The 30 % end-to-end claim is unproven
  until the one-shot protocol measures it.**
- **Not in the probe:** the blake3 over each entry, the owned read, the IR check pass, and the
  binary-key hash (Key). All are inside the warm measure.
- **Heap.** The IR held after a decode is 476.5 MiB in 4.66 M allocations, against 498.3 MiB in
  4.31 M after a parse (counting allocator in the probe). `exact_size` runs after decode, as after
  a parse.
- **Not measured yet:** the largest package's entry size (it sets the owned-read peak), and the
  hash cost of the `alsem` binary (48.2-48.9 MB for the `release-fast` builds in this host's
  worktrees). Both are the first implementation step's measures.

## The key: the running binary

**Decision (orchestrator, rev. 5; sol r2 P1-1).** Rev. 4 rebuilt the key from build inputs. sol r2
showed it still open: a cargo `paths` override changes the parser or codec without touching
`Cargo.lock` or any file a build script sees; root-crate cfgs do not establish dependency cfgs;
the grammar compiler fingerprint does not cover the tree-sitter runtime's own native build. Any
list of inputs can miss one. So the key is the producer itself.

- **`IR_KEY` = blake3 of the running executable's bytes, plus the IR format number.** Whatever
  produces or reads the IR (the grammar's C code, the tree-sitter runtime, the lowerer, the codec,
  features, cfgs, profile, target, `--config` and `paths` overrides, the toolchain) is compiled
  into those bytes. A change in any of them changes the key. No wrong hit can come through the
  build.
- **Read through an opened handle, the image that is running:**
  - Linux: open `/proc/self/exe`. It is the inode the process runs, even if the path was replaced
    or unlinked after start (a rebuild while an engine runs).
  - Windows: the path of the loaded image (`GetMappedFileNameW` on the module's base address,
    which follows a rename of the running file; then opened denying write sharing). **Not
    verified on Windows.** If the implementation cannot open the running image itself, the IR
    cache is off on Windows: it fails closed.
  - Other Unix: `std::env::current_exe`, opened once. Not a target today.
- **Computed once per process, and once per binary across processes.** A sidecar entry
  `<root>/ir/exe/<stamp>` holds the hash, where the stamp is the executable's identity from the
  same open handle: device, inode, size, mtime and **ctime** in nanoseconds (Linux); volume serial,
  file id, size, last-write and change time (Windows). ctime is in the stamp because `cp -p` over
  an existing file keeps its inode, size and mtime, but no user can set ctime. The sidecar is
  published like an entry (temp file, rename) and checked like one (magic, length, blake3 of its
  own bytes). A missing or refused sidecar means: hash the binary, publish the sidecar.
- **Diagnosis only, in each entry's header:** the grammar commit (`88bb6871` today, baked by
  `al-syntax`'s build script from the grammar checkout, `unknown` if none) and the binary's
  version string. They are never compared.
- **What this removes:** rev. 4's `links` key, the hashing build scripts, the crate list and its
  lockfile rule, the build-configuration partition, the machine-path problem of hashing cargo
  configs, and the owner question on `--config`. `CACHE_VERSION_GRAMMAR` stays unused here.

**The cost: every new binary starts cold.**
- **Installed and shipped binaries** (`~/.al-sem/bin/alsem`, the LSP's engine child, a release)
  keep one key until replaced: every process after the first hits. This is where the gain lands.
- **A gate** builds once and runs many processes. Processes of ONE executable share entries:
  `output_cdo`'s several `alsem` children and the impact ratchets' `alsem impact` calls hit after
  the first fill. Each test executable (`program_resolve_harness`, `lsp`, `l4_summary_differential`,
  the lib) is its own binary and fills its own folder once, at cost (c). The next gate on an
  unchanged build reuses them only if cargo did not relink. So a gate's wall time gains less than
  the one-shot ratio; bar (a) reports it as measured.
- **Dev loops:** each relink pays (c) on its first CDO run, then hits.
- **cdo-speed:** base and head differ in bytes, so they always use separate folders. Its build rows
  change accordingly (Acceptance 4).
- **Disk:** every new binary that runs with the cache on adds a key folder (~144 MiB for CDO).
  The soft target and the hourly prune below handle the churn.

**Tests:**
- Two binaries that differ in any byte use different key folders; one fixture package in one root
  gives two entries and no cross hits. Proof: key on the format number alone, red.
- The sidecar: a stamp change (touching the binary's ctime by rewriting one byte in place in a
  copy) refuses the old sidecar and rehashes. Proof: leave ctime out of the stamp, red.
- Linux: an engine whose executable path is replaced by another file while it runs still keys on
  its own image. Proof: key through `current_exe()`'s path, red.

## The text: ENG-113's verified digest, carried

(Kept from rev. 4, sol r1 P2; checked in `src/snapshot/cache.rs`.)
- ENG-113's entry header holds a blake3 over the package's content hash, its JSON index (every
  path, offset and length, in order) and its text (`check_entry`). That value is the **text
  digest**: it names the exact ordered `SourceRoot.files` under the text format `s9.f2`.
- `CacheDir::read` returns the verified digest with the mapped files; `cached_source_in` returns
  it as a third value, `Some` only for a mapped, verified entry. `SourceRoot` gains
  `text_digest: Option<[u8; 32]>`, set by `EmbeddedAppProvider`, kept by `DepCache`'s `WeakSource`
  through `upgrade` and `deferred`, `None` in every other construction. `None` means no IR cache
  for that package.
- **Before a miss publishes, the text is re-hashed** (Fable r3 P1-1). The IR was parsed from the
  mapped text; an outside in-place rewrite of the text entry during the parse (ENG-113's stated
  risk) would give IR of other text under the old digest. After the package's parse loop, the
  writer recomputes ENG-113's digest over the package hash, the index it was given and the mapped
  texts, and publishes only if it equals the carried digest; on a mismatch it publishes nothing and
  warns. This costs one blake3 over the package's text on a miss only. The hit path does not need
  it: a decoded tree attached to a changed text is ENG-113's existing risk, unchanged.

## The codec

1. **serde derives on the IR types**, always on (Fable r1 P3). `al-syntax` adds `serde` with the
   `derive` and **`rc`** features itself (Fable r3 P3): `TypeText` is `Arc<str>`, and `al-syntax`
   must not rely on `al_sem`'s feature unification.
2. **`Origin`** stores the grammar symbol id (`kind_id: u16`; `u16::MAX` is `ERROR`). It is encoded
   as is and validated on decode: `u16::MAX`, or below `node_kind_count` with a non-empty name.
   That set is total over named and anonymous kinds and `ERROR`. The pack's `RawKind::ALL` codec
   covers named kinds only and is not reused. Ids are stable within one key (one binary).
3. **`ActionDecl.kind`** (the IR's only `&'static str`) gets a hand-written pair over its keyword
   set; any other value refuses the file.
4. **Bounded decode: real budgets** (sol r2 P1-2; serde's cautious size hint caps only the first
   reservation):
   - **Encoded bytes per file:** at most `max(64 KiB, 16 × text length)` and at most 64 MiB. CDO
     averages 1.25×; the per-file maximum is measured and written beside the constant. A file past
     it is refused before decoding.
   - **Decoded heap per file follows from it.** Every element the decoder creates consumes at least
     one encoded byte (strings consume their length; there is no zero-sized element type in the
     IR, asserted by a test). So a file creates at most `encoded length` elements, and its heap is
     at most `2 × S × encoded length`, where `S` is the size of the largest IR element type and 2
     is `Vec`'s growth. `S` is a `const` computed from `size_of` and asserted in a test.
   - **Test:** a payload whose sequence lengths claim far more elements than its bytes holds fails
     at end of input, and a counting allocator shows the peak below the bound. Proof: remove the
     encoded-length cap and feed a 64 MiB claim inside a large slice, the bound is exceeded, red.
5. **IR check before use** (`AlFile::check(text)` in `al-syntax`, where the arenas are private; sol
   r2 P1-2, Fable r3 P1-2). It runs on every decoded file before any consumer sees it:
   - every `ExprId`, `StmtId`, `BlockId` and `MemberOriginId` held anywhere (decls, nodes, side
     tables) is inside its arena;
   - **expressions:** each child id is below its own. This holds by construction: an `Expr` holds
     its children's ids, which exist only once `Ir::add_expr` has returned them (checked in
     `ir/mod.rs`). **Direct block → block** references (a `PreprocGroup` inside `Block.items`,
     if it holds blocks; not verified) follow the same rule. A flat pass, no traversal;
   - **statements and blocks:** an iterative three-colour depth-first walk over the WHOLE stmt and
     block arenas (an explicit stack, no recursion, no depth bound) finds any cycle. Each node is
     entered once, so the walk is linear in the arena sizes. It also checks that each stmt and
     block has at most one parent and is reachable from a routine body or trigger root, so no
     shared subgraph can make a consumer's walk repeat work. **Not verified that fresh IR obeys the
     one-parent and reachability rules:** the CDO round-trip test checks both rules on all 10,881
     files before they are adopted; a rule fresh IR breaks is dropped from the check and named in
     the spec;
   - every `Origin`: `byte_start <= byte_end <= text.len()`, **both on `char` boundaries of the
     text** (`str::is_char_boundary`), `start <= end`, and a valid kind id; the same for every
     `VarWrapper` byte range and arm range;
   - `tainted_calls` is strictly ascending (`call_is_tainted` binary-searches it; `mark_tainted`
     keeps it so). **`preproc_ranges` is not checked for order**: `with_build_context` records an
     inner range before its enclosing one, and `Ir::preproc_context` scans all ranges for the
     smallest that contains the id (checked in `crates/al-syntax/src/ir/mod.rs`). Rev. 4's sorted
     check would have refused fresh IR.
   - **Tests:** one hand-built malformed payload per rule (out-of-arena id, an expr child above its
     parent, a stmt cycle, a block with two parents, an unreachable stmt, a range past the text, a
     range inside a multi-byte character, an invalid kind id, an unsorted `tainted_calls`), each
     refused, each with a discrimination proof; and valid round trips of a fixture with nested
     `#if` blocks inside a routine body (inner `preproc_ranges` first), a recovered file and a file
     with non-ASCII identifiers, each equal to a fresh parse and accepted by the check.
6. **Structural equality** (kept): `PartialEq` and `Debug` derived on `AlFile` and every IR type
   that lacks them; checks assert `decoded == fresh`. A test fails on any `serde(skip` or
   `serde(default` under `crates/al-syntax/src/ir/`.

## Lowerer state: scratch, proven (a small product change ENG-110 makes)

(Kept from rev. 4; sol r2 P1-3 accepts it.) The lowerer's six thread-locals: `CONDITIONAL_VARS`,
`PREPROC_WALK` and `PREPROC_RANGES` are reset at the top of `lower_file`; `VARS_INCOMPLETE` is set
before each read; `shared_type_text`'s `TEXTS` decides sharing only. `BUILD_CONTEXT` was the hole:
`with_build_context` truncates it only on a normal return, and `lower_file` never clears it.

**The change** (in `al-syntax`): `lower_file` clears `BUILD_CONTEXT` at its top;
`with_build_context` truncates through a drop guard, so an unwind restores it too.

**Tests:** unwind restore (a panicking body under `catch_unwind` on a one-thread pool leaves the
thread-local empty; proof: remove the guard, red); entry reset (a hand-seeded stale context, then a
clean and a `Recovered` fixture with `#if` code lowered on that thread equal a fresh thread; proof:
remove the clear, red). The reverse-order purity run (`probe3.log`, 0 of 10,881 differ) stays as
evidence of no order effect; it does not prove unwind safety.

## Design

1. **Built on ENG-113's `CacheDir`, refactored, not copied** (Fable r3 P2-10). The refactor, in
   `src/snapshot/cache.rs`, keeps ENG-113's behaviour:
   - `CacheDir` becomes `pub(crate)`.
   - **`write(name, replace, writer)`**: today's unique-temp, `O_EXCL` 0600, `linkat`/`renameat`
     publish, with the entry's bytes produced by a writer closure. ENG-113's `write_entry` becomes
     one such closure; the IR envelope is another.
   - **`read_owned(name, cap) -> Result<Option<Vec<u8>>>`**: the same open (relative to the
     handle, no symlink, no blocking, regular and private), refusing a file longer than `cap`
     before reading, then reading exactly its length.
   - **`subdir(name, create)`**: opens a child directory through the handle and checks it private;
     with `create`, makes it first (`mkdirat`, 0700). **Only a writer creates** `ir/` and
     `ir/<key>/` (and `ir/exe/`), on its first publish; a reader never creates a folder.
   - `lock(name)` keeps its per-name `flock` and gains a non-blocking form for pruning.
2. **Layout:** `<root>/ir/<IR_KEY, 64 hex>/<text digest, 64 hex>.bin`, plus `<root>/ir/exe/` for
   sidecars. `<root>` is ENG-113's cache directory. `ALSEM_IR_CACHE_DIR` overrides it for the
   acceptance's fresh cold roots and the cache's own tests. **On Windows the override is refused**
   (sol r2 P1-4): ENG-113's Windows safety rests on the per-user profile location, which an
   arbitrary path does not have, and no privacy check is specified there. The process logs it once
   and runs with the cache off. Windows tests use the default location under a test-only profile
   variable instead (not verified that this is feasible; else Windows keeps only the default-path
   tests).
3. **Envelope** (kept from rev. 4): header (magic `ALSEMIR\0`, format, full key, text digest, the
   diagnosis fields, file count, payload length); index of `(offset, len)` per file in
   `SourceRoot.files` order; payload; trailing blake3 over every byte before it. A hit requires,
   in order: the entry within the per-entry cap; magic, format, key, digest; count equal to
   `SourceRoot.files.len()`; exact total length; the blake3; ranges that tile the payload exactly
   (every sum checked). Then per file: the encoded-length budget, decode with no trailing bytes,
   `AlFile::check` against that file's own text.
4. **Owned, bounded reads, not a map** (kept from rev. 4, accepted by sol r2): the bytes decoded are
   the bytes hashed, so outside damage is always a refusal, never a SIGBUS. Peak: one package's
   entry during its decode (packages are taken one at a time).
5. **Disk** (sol r2 P2):
   - **Per entry:** 1 GiB, written and read (CDO's whole set is 143.9 MiB).
   - **Total: 4 GiB is a soft target, not a cap.** A prune brings `<root>/ir/` under it. Between
     prunes writers can exceed it. **Overshoot bound:** a prune runs at most an hour after the last
     one, at the first cache use after that hour; until then the overshoot is the entries published
     in that time, each at most 1 GiB, so at most (packages published in an hour) × 1 GiB; in
     practice one CDO set (~144 MiB) per new binary run in that hour. If this process's own key
     folder alone passes 4 GiB, it publishes nothing more and warns once.
6. **The seam** (lines re-checked on `c80f1391`): `src/program/dep_summary.rs`,
   `parse_for_build_observed` (line 62, the per-file call at 122) and `parse_dep_bodies` (line 175,
   the call at 195), each calling `snapshot::parse::parse_file` per file inside a `par_iter` on the
   big-stack pool. One function serves both: per package, once, the owned read and envelope checks;
   per file inside the existing `par_iter`, decode and check in place of `parse_file`. `LiveTrees`'
   bound holds. On a miss the loop parses and encodes; the package's encoded buffers are published
   at its end (after the text re-hash), then freed.
7. **Switches are parameters in library code** (Fable r3 P3). The library takes an
   `Option<&IrCache>` (the root, the key, the report sink), carried beside `BuildProfile` to the
   seam; `None` is today's behaviour. **Only the binaries' `main`s** (`alsem`, `aldump`,
   `al-call-hierarchy`) build it, from `ALSEM_IR_CACHE`, `ALSEM_IR_CACHE_DIR`,
   `ALSEM_IR_CACHE_VERIFY` and `ALSEM_IR_CACHE_REPORT`, and from the batch state (`claim::dir()`, a
   `OnceLock` set by `alsem batch` before any job runs). The exact carrier between the `main`s and
   the seam is chosen in the plan (not verified which builder struct carries it best). Library
   tests pass their own `IrCache` with a temporary root.
8. **Small packages are skipped** (kept): below one fixed constant, measured once on the prototype
   with its table, no IR file is written.
9. **Maintenance** (sol r2 P2):
   - **`last-used`**: `<key folder>/last-used`, never written in place; published by rename at a
     process's first hit or write, and again when older than one day.
   - **Throttle:** at the first cache use of a process, if `<root>/ir/last-prune` is older than an
     hour, take the prune lock without blocking, **re-read `last-prune` under the lock** (another
     process may have just pruned), and only if still older, publish a new `last-prune` and prune.
   - **Prune lock:** Unix, `flock` on `<root>/ir/prune.lock`. Windows: `prune.lock` opened with no
     sharing (share mode 0), which no second process can open while it is held; a failed open skips
     the prune.
   - **Respects live publishes:** before removing a key folder, the prune takes every
     `<digest>.lock` in it without blocking; if any is held, the folder is skipped this round. With
     the locks held it re-reads `last-used`; a fresh one keeps the folder. Windows has no publish
     lock: a delete of a file another process has open fails and the folder is skipped; a publish
     into a folder just deleted fails, and that process keeps its parsed IR.
   - **What it removes:** key folders whose `last-used` is older than 7 days; then, oldest
     `last-used` first, key folders until under the soft target, never this process's own; sidecars
     older than 7 days; temp files of this cache's name shape older than a day. Only names of these
     exact shapes, through the checked handle, never following a symlink (mantra 24).
   - `alsem cache prune` maintains a different cache and is not changed.
10. **`parse_snapshot` stays uncached** (kept): its callers outside `dep_summary` are test modules.

## `alsem batch` (TLS-109): off, tested through the real run

(Decision kept from rev. 4; sol r2 P1-4 asks for the real path.) In a batch run (`claim::dir()` is
`Some`: the parent, and every worker, which refuses to run without it) the binary's `main` builds no
`IrCache`: no reads, no writes, no pruning, and **no report-file append** (the report file would be
a write the claims do not cover). The claims do not reserve the cache's names, so a job output
under the cache root could be unlinked by a publish; batch already shares one model build across its
jobs; no script runs `alsem batch` (its callers are `tests/cli` and `tests/perf_bounds.rs`). Lost:
multi-worker batch runs keep parsing per worker. A follow-up task (to file) turns it on with the
claim check reserving the cache folder's names, for the ENG-113 text cache too.

**Test, through the real run:** a `tests/cli` test runs `alsem batch` with two batches (so the
parent starts two workers) over a fixture workspace with an embedded-source dependency, with
`ALSEM_IR_CACHE=on`, a temporary `ALSEM_IR_CACHE_DIR` and `ALSEM_IR_CACHE_REPORT` set. After the run
the IR root holds no file and the report file does not exist. Proof: build the `IrCache` regardless
of the batch state, red.

## Correctness checks

- **Decoded equals parsed:** every CDO dependency file decoded equals a fresh parse (`lib_cdo`).
- **Determinism:** the CDO set encoded twice in different thread orders gives the same bytes.
- **VERIFY** (kept): with `ALSEM_IR_CACHE_VERIFY=1` every hit is also parsed and compared; a
  mismatch fails the process loudly. `cdo-gate` sets it for `program_resolve_harness`, for good;
  zero hits there labels the phase "VERIFY: no hits". **Never in a timed run:** cdo-speed and the
  one-shot protocol refuse to start with it set.
- **Tests still exercise the parser:** `.cargo/config.toml`'s `[env]` sets `ALSEM_IR_CACHE=off` for
  every process cargo starts; an explicit value in the environment wins (cargo's documented `[env]`
  rule without `force`; not re-tested here). Directly started binaries default to on.
- **The counters** (sol r2 P1-5, Fable r3 P1-3, P3), per process:
  - `seam_files`: files that reached the seam: the denominator, counted at the seam before any
    cache decision, independently of the cache;
  - `disk_hits` (decoded from an entry), `parsed_miss`, `refused`, `skipped_small`, `skipped_off`
    (cache off, no digest, or batch), and `published_packages`;
  - `inproc_reused_packages`: packages served from `DepCache`'s in-process tier, which never reach
    the seam. Reported apart, so in-process reuse is never mistaken for a disk hit.
  - Invariant: `seam_files = disk_hits + parsed_miss + refused + skipped_small + skipped_off`,
    asserted at exit in debug builds and checked by the acceptance.
  - With `ALSEM_IR_CACHE_REPORT=<file>`, one JSON line per process at exit (command, binary key,
    counters); also in the perf trace and census.
- **`ParseStatus::Recovered`** is replayed exactly; its doc is fixed to say the cache stores it.

## What still needs the source TEXT after an IR hit

(Kept.) Line and column positions (`LineTable`); snippets in explain and digest; the row cache's
`SourceText::ptr_eq` fast path; `recovered_file_paths` and diagnostics that quote source. ENG-113's
mapped text stays the one copy.

## Many processes filling the cache at once

Each cold process parses and encodes. At a package's end it re-hashes the text, takes the entry's
lock, reads the entry again, and publishes only if none is there (Unix). Write amplification on
Unix: one entry per package (the lock is taken before the temp file is made). Windows: a loser
writes one temp file per package and its no-clobber persist fails. No lock spans a gate's start.
Measured in Acceptance 5.

## Failure cases

- **Envelope refused** (any header field, length, hash, ranges): the package parses; at its end the
  entry is replaced under the lock. The next run hits.
- **A file's budget, decode or check fails** under a valid hash: that file parses; the others keep
  their decoded IR. At the package's end the replacement is the good files' verified slices plus
  the refused files' fresh encodings. **If that replacement equals the stored bytes** (Fable r3 P3:
  the codec itself writes what it then refuses), nothing is rewritten: a warning names the package,
  file and key, and `refused` counts it. A rewrite would loop on every run.
- **Truncated or rewritten during a read:** a refusal (owned reads).
- **The text changed during a miss's parse:** nothing is published (the text re-hash).
- **The cache is not writable or not private, or (Windows) the override is set:** no IR cache,
  logged once.
- **Tests:** each refusal with a hand-written entry in a temporary root; each asserts the built model
  equals the cache-off model and that a second run hits the repaired entry (or, for the equal-bytes
  case, that nothing was rewritten). The text re-hash: a test changes the mapped text's bytes
  through a test seam between the parse loop and the publish, and sees nothing published. Proof:
  skip the re-hash, red.
- **Threat model** (mantra 26): the hash is integrity, not a signature. Someone who can write this
  user's cache can also replace this user's binaries. Other users are kept out by the private
  directory and entry rules.

## Acceptance

The judgment rules below are fixed before any run (mantra 9). Every record carries the master and
head SHAs, the workspace SHA (`bc3ccb18`), the `.app` closure stamp (`dependency_closure_sha256`)
and the binary keys; a change in any voids the run.

1. **Identity, byte for byte.** The deterministic output files of the CDO commands (fingerprint,
   analyze primary and `--scope all`, the call-graph histogram, and the explain, query and digest
   outputs `output_cdo` produces) from four arms: **master**, **head-off**, **head-cold**,
   **head-warm**. All four must be equal byte for byte (sol r2 P1-5: master against head-off too).
   A file that differs between two runs of one arm is nondeterministic: listed, filed as a bug, and
   left out only of this comparison. Plus: goldens with the cache off and once on and warm; the
   refresh verify in both modes, its reference build made with the IR cache off;
   `cdo-gate --only harness,l4,output` warm.
2. **Coverage, never vacuous.** From the report files, per process, per command and per phase:
   - every warm run: `seam_files > 0`, `disk_hits = seam_files − skipped_small`, `refused = 0`;
   - every cold sample: `seam_files > 0`, `disk_hits = 0`, `published_packages` equal to the
     packages above the small threshold, and the entries present on disk in that sample's root;
   - every off run: `disk_hits = 0` and `published_packages = 0`.
   A run whose workload was meant to use the cache and shows `seam_files = 0` fails the acceptance.
3. **One-shot timing** (bars (a), (c)): `alsem analyze <CDO_WS>`, `release-fast`, in a timed CDO slot
   (`scripts/cdo-slot run … --timed`), counted the way cdo-speed counts since TLS-83 and TLS-111:
   user-mode `instructions` and `cycles` per PMU (`cpu_core`, `cpu_atom`), every event of one PMU
   with exactly equal running time, and the instructions' running time summed over the PMUs at
   least the call's CPU seconds less 2 % less 0.05 s (else the sample is discarded and repeated).
   No CPU pinning, as cdo-speed. User and **sys** CPU and wall are recorded beside the counts.
   - Arms per round: master, head-off (`ALSEM_IR_CACHE=off`), head-cold (a fresh, empty
     `ALSEM_IR_CACHE_DIR` per sample), head-warm (one root filled once, untimed, before the
     rounds). The ENG-113 text cache is the same warm one for every arm (one untimed run of each
     binary first). The arm order rotates each round.
   - **Judgment, fixed now:** per round, the ratio of each head arm's summed instructions (all PMUs)
     to that round's master. The verdict is the **median of the per-round ratios over 6 rounds**:
     head-warm ≤ 0.70, head-cold ≤ 1.05, head-off ≤ 1.01. If a median lies within 0.01 of its
     limit, 6 more rounds run and the median of all 12 decides; no further rounds. A suspend or
     "UNDER LOAD" label voids that round, which is re-run. CPU ratios are quoted only as CLAUDE.md
     allows.
4. **cdo-speed after ENG-110** (Fable r3 P2-5, P2-6; sol r2 P1-5). Base and head always have separate
   key folders, so a warm-cache build ratio would compare one fill against another. cdo-speed:
   - **keeps timing dependency parsing:** the judged `build` row runs with `ALSEM_IR_CACHE=off` on
     both sides, as today's build;
   - then fills each side's folder untimed, and adds a **`build (IR warm)`** row, reported with
     hit counts per side; a side with zero hits labels the verdict;
   - a `--ir-cold` mode times a build on a fresh IR root per side (the cold resident result),
     reported;
   - `child_env` (and `scripts/bench_ab.py`'s copy) **keeps `ALSEM_IR_CACHE` and
     `ALSEM_IR_CACHE_DIR` on purpose** while still stripping every other `ALSEM_*`, and refuses
     `ALSEM_IR_CACHE_VERIFY`. Checked: both strip every `ALSEM_*` today (`scripts/cdo-speed`
     `child_env`, `scripts/bench_ab.py` line 39).
5. **cdo-gate** exports `ALSEM_IR_CACHE=on` explicitly for every phase (the cargo `[env]` default is
   off), `ALSEM_IR_CACHE_VERIFY=1` for the harness phase only, and one report file per phase; its
   summary prints each phase's counters. A full gate's wall time, warm, is reported.
6. **Memory and disk:** the engine census, warm against master; peak RSS of one cold one-shot and of
   four started together on one fresh root, with temp bytes and write amplification; the key
   folder's size for CDO; the largest entry; the per-file encoded-to-text maximum; the
   small-package table; the binary hash's cost, cold and with the sidecar.

## Settled in review

1. **The key:** the running binary (orchestrator, rev. 5). Closed by construction.
2. **VERIFY:** one gate phase for good; never in a timed run.
3. **Small-package threshold:** one constant, measured once.
4. **mmap or owned:** owned, bounded reads.
5. **`alsem batch`:** off for the first release, tested through the real parent and workers.
6. **Disk:** 4 GiB is a soft target with a stated overshoot; 1 GiB per entry is a cap.

## Left for the owner

None. The binary key removed rev. 4's `--config` question. The disk numbers (1 GiB per entry, 4 GiB
soft target) and batch-off are the orchestrator's to confirm, not open design questions.

## Builds on ENG-113

- The text digest added to `CacheDir::read`'s and `cached_source_in`'s results, `SourceRoot` and
  `DepCache`. ENG-113's behaviour does not change.
- The `CacheDir` refactor (Design 1).

## Owner design principles applied

From `/coord/owner/design-mantra.md`:
- **1 (correct first)** and **5 (unknown is not no):** the binary key closes the build by
  construction instead of listing inputs; Windows without a verified running-image read, or with an
  unchecked override, turns the cache off; owned reads; a text change during a miss publishes
  nothing.
- **3 (root cause):** the `BUILD_CONTEXT` leak is fixed in the lowerer.
- **4 and 9 (measure; commit to the result first):** the one-shot judgment rule is written before
  any run; coverage can never pass on an empty denominator; gate labels start as labels.
- **7 and 8 (tests that can fail, real use):** every guard has a stated break; batch-off is tested
  through the real `alsem batch` parent and workers.
- **10 (reproducible baselines):** each record pins the workspace, the `.app` closure and the keys.
- **13 and 14 (right-sized, reuse):** ENG-113's `CacheDir` is refactored, not copied; the binary
  key deletes rev. 4's build-script machinery.
- **16 (clean cutover):** the off switch exists for tests and measurement only.
- **24 (delete only what you made):** pruning removes only this cache's exact name shapes and skips
  folders with live publish locks.
- **26 (state the threat model):** said, not guarded weakly.
- **35 (file work when found):** the batch follow-up is a task to file.

## Review history

- **rev. 1:** written.
- **rev. 2:** Fable r1 folded in (3 P1, 6 P2, P3s), plus the orchestrator's VERIFY, tests and
  measures additions.
- **rev. 3:** Fable r2 folded in (N1 baked-hash trigger test, N2 one home for the test switch, N3
  VERIFY labels zero hits).
- **rev. 4:** sol r1 (5 P1, 2 P2) folded in: whole-envelope hash, build-input key, `BUILD_CONTEXT`
  reset, owned reads, batch off, the one-shot protocol, digest in the path, disk caps.
- **rev. 5:** sol r2 (4 P1 open, 1 P2) and Fable r3 folded in, plus the orchestrator's decision to
  key on the running binary (sol P1-1; Fable P2-4, P2-7, P2-8, P2-9 dissolve with it). Decode safety
  (sol P1-2, Fable P1-2): encoded and heap budgets, a three-colour stmt/block walk, the
  `preproc_ranges` order check dropped, `char` boundaries. Miss correctness (Fable P1-1): the text
  re-hash before publish, no rewrite of equal bytes. Batch and Windows (sol P1-4): the real-run
  test, no report append in batch, the override refused on Windows. Acceptance (sol P1-5, Fable
  P1-3, P2-5, P2-6): master against head-off, the seam-file denominator, in-process reuse apart,
  cold-sample checks, the paired median rule, cdo-speed's off build row and environment pass-through,
  cdo-gate's explicit switch, TLS-111 counting, sys CPU. Disk (sol P2): soft target with overshoot,
  prune timestamp re-read under the lock, live publish locks respected, Windows coordination.
  Fable P2-10, P3: the `CacheDir` refactor named, switches as parameters, `al-syntax`'s own `rc`
  feature.

# ENG-110: persistent cache of parsed dependency IR (design, rev. 7, 2026-11-21)

Status: a spec for review. No product code. ENG-113 (the package-text cache this one stands on)
has landed. Rev. 7 folds in sol's review r4, Fable's review r5 and the orchestrator's decisions on
both. It was written on `c80f1391`; master is now `bbec5165` (TLS-113), which changes none of the
files this spec cites (checked: `git diff --stat c80f1391 bbec5165` over `dep_summary.rs`,
`snapshot/cache.rs`, `engine/batch/`, `crates/al-syntax/`, `engine/host/child.rs`, `cdo-speed` and
`bench_ab.py` is empty). The measurements come from a throwaway prototype that never lands: branch
`tools/eng-110-proto` (`bf12e42a`), with serde derives on `al-syntax`'s IR and
`examples/eng110_probe.rs`.

"Not verified" marks every claim this revision could not check against source or a run.

## Goal, gain and bar

Every engine process parses and lowers every dependency file again: about 24 CPU-s per process on
CDO. A cache hit replaces that with a read, a hash and a decode.

**Where the gain lands.** The key is the running binary (Key, below), so one binary fills its
entries once and every later process of THAT binary hits:
- **the installed `alsem`** (`~/.al-sem/bin/alsem`) and shipped Linux releases: every one-shot call
  after the first, and every restart of a resident server (`alsem mcp`, the LSP's engine child),
  which today rebuilds its model from scratch;
- nowhere else to speak of.

**A gate gets no gain.** A gate runs on a tree it has just built, so every binary in it is new and
starts cold. The in-process test binaries get no cache at all (Design 7). Only `output_cdo` and
`output_cdo_impact` run with the cache on (Acceptance 5), and their first `alsem` process fills
what the later ones hit. cdo-speed gets slower: it fills each side's folder, untimed, on every run
with a new head. The CHANGELOG says the same once this lands.

**Bar** (measured as Acceptance says; its judgment rule is fixed there before any run):
- (a) **Warm cache, one-shot `alsem analyze` on CDO:** at least 30 % fewer user-mode instructions
  than master.
- (b) **Byte-identical output:** master, head with the cache off, cold and warm all give the same
  bytes; goldens; the refresh verify in both modes against a cache-off rebuild; the CDO subset.
  Every warm run must show actual disk hits over a positive count of eligible files.
- (c) **A cold miss** costs at most +5 % instructions over master (it includes the publish round
  trip, Design 3); the cache off at most +1 %. Every new binary pays (c) once. Peak RSS of a cold
  miss, alone and four at once, is reported.

## Measured ceiling (server1, CDO baseline `bc3ccb18`, master `a73b4bf6`)

- **One-shot `alsem analyze`** spends 25.4-25.5 CPU-s of 57 in `al_syntax::parse` (parse plus
  lower): 44.7-45.0 % in two samply profiles.
- **Dependencies** are 96 % of the source bytes: 10,881 of 11,463 files, 115.5 of 120.4 MiB, in
  11 packages (`probe3.log`). So dependency parsing is ≈24 CPU-s a process.
- **In instructions:** the parser and lowerer symbols' self shares in ENG-112's `perf record` are
  ≈39 % of user instructions combined; a lower bound.
- **The prototype**, single thread, in `/work/.al-sem/profiles/2026-11-18-eng110`:
  parse+lower 11.4-12.9 CPU-s; encode 0.27-0.29; decode 0.32-0.37 (2.6-3.2 % of parse+lower);
  143.9 MiB postcard, 1.25× the source bytes. A codec-cost estimate only. **The 30 % end-to-end
  claim is unproven until the one-shot protocol measures it.**
- **Heap:** 476.5 MiB in 4.66 M allocations after a decode, against 498.3 MiB in 4.31 M after a
  parse (counting allocator in the probe).
- **The binary hash:** the `release-fast` `alsem` is 48.6 MB; sha256 over it takes about 0.06 s
  warm (Fable r4). blake3 is measured in step 1.
- **Measured in step 1** (2026-11-21, release-fast, single thread, the probe on the landed IR
  derives; note `/work/.al-sem/engine-notes/eng110-step1-measures.md`):
  - parse+lower 12.69 CPU-s; encode 0.31; decode 0.50; 144.1 MiB encoded. Every file's round
    trip held.
  - **The publish round trip** (decode + `==` + re-encode + byte compare): 0.84 CPU-s for all of
    CDO, 6.6 % of parse+lower (Fable r5 estimated ~0.35). A cold miss pays it once per package.
  - **The largest entry:** Base Application, 128.2 MB encoded (122.3 MiB), under the 1 GiB cap;
    its text 105.1 MB. The CDO key folder: ~144 MiB.
  - **The image hash:** blake3 of the 49.7 MB release-fast `alsem`, 1 MiB reads: 0.043 CPU-s
    (0.061 s wall) with the file out of the page cache, 0.027-0.028 warm.
  - **The small-package table:** a hit (read + blake3 + decode) costs 0.026-0.046 of the
    package's parse+lower at every size, down to CDO's smallest package (12 files, 25 KB): the
    cache wins by more than 20x everywhere. `SMALL_PACKAGE_MIN_FILES = 8`, below every CDO
    package, keeps only one-to-seven-file packages (not measured) from making tiny entries.
  - **nextest honours `[env]`** (cargo-nextest 0.9.152): the test and its child saw the
    `[env]` value, and an explicit environment value won. `ci-steps` and `check-goldens` need no
    export.

## The key: the running image, hashed every time

**Decision (orchestrator, revs. 5 and 6).** `IR_KEY` = blake3 of the running executable's bytes,
plus the IR format number. Whatever produces or reads the IR is compiled into those bytes: the
grammar's C code, the tree-sitter runtime, the lowerer, the codec, features, cfgs, profile, target,
`--config` and `paths` overrides, the toolchain. Any change to them changes the key. The price is
the one said above: a gate gets no gain.

- **No sidecar.** Each process hashes its full image once, lazily at its first seam use (a process
  that parses no dependency pays nothing).
- **Linux only in v1.** The image is read through `/proc/self/exe`: the inode the process runs, even
  when the path was rebuilt or unlinked since. If it cannot be opened or read, the cache is off for
  that process (fails closed).
- **Windows: the cache is off in v1.** Reading the running image there is not verified natively.
  Future names the work. Other Unix systems are not a target.
- **The producer is self-contained** (sol r4 P2). The image key covers what is in the image; it
  holds only while nothing outside it changes how a text parses or decodes. Two tests pin that on
  the `release-fast` `alsem`:
  - **shared libraries:** `readelf -d` lists exactly an allowed `NEEDED` set:
    `{libc.so.6, libgcc_s.so.1}`, or `{ld-linux-x86-64.so.2, libc.so.6, libgcc_s.so.1}`. The
    loader is allowed because the release-fast `al-call-hierarchy`, which installs the cache too,
    needs it for one symbol (`__tls_get_addr`); it is glibc's dynamic loader, mapped in every
    process, and does no text processing (orchestrator decision after review r1, 2026-11-21).
    `alsem` and `aldump` need only the first set. Any other library fails the check. It runs on
    all three binaries (`scripts/check-ir-image`). Proof: `/usr/bin/python3` (extra libraries)
    fails it;
  - **symbols:** `nm -D --undefined-only` shows no locale or wide-character classification symbol
    (`setlocale`, `newlocale`, `isw*`, `tow*` and their kin); Fable r4 found none. Proof: add a call
    to `iswalpha` through `libc`, red.
- **A tool's binary is not the key.** Under valgrind or a similar loader, `/proc/self/exe` is the
  tool. The `alsem` binary carries a 16-byte marker: a `#[used]` static read once through
  `std::hint::black_box` (Fable r5 P1-2), so neither the compiler nor the linker can drop it. The
  hashing pass also looks for it; without it the cache is off. Checked on the shipped artifact:
  - a local test searches the built `release-fast` `alsem` file for the marker bytes;
  - `build-and-deploy.yml` and `release.yml`, in their Linux `release-ship` job (`ubuntu-22.04`,
    the one that also builds the no-telemetry `alsem`), search the built `target/release-ship/alsem`
    for the marker and fail the job without it. The Windows jobs do not check it: the cache is off
    there.
  - Test: hashing a file without the marker yields no key.
- **Diagnosis only, in each entry's header:** the grammar commit (`88bb6871` today, baked by
  `al-syntax`'s build script, `unknown` if none) and the binary's version string. Never compared.

**Tests:** two binaries differing in any byte use different key folders; one fixture package in one
root gives two entries and no cross hits (proof: key on the format number alone, red). An engine
whose executable path is replaced while it runs still keys on its own image (proof: key through
`current_exe()`'s path, red).

## The text: ENG-113's verified digest, carried; fills parse an owned copy

(Carrying kept; checked in `src/snapshot/cache.rs`.) ENG-113's entry header holds a blake3 over the
package's content hash, its JSON index (paths, offsets, lengths, in order) and its text
(`check_entry`): the **text digest**, naming the exact ordered `SourceRoot.files` under text format
`s9.f2`. `CacheDir::read` returns it with the mapped files; `cached_source_in` returns it as a
third value, `Some` only for a mapped, verified entry. `SourceRoot` gains
`text_digest: Option<[u8; 32]>`, set by `EmbeddedAppProvider`, kept by `DepCache`'s `WeakSource`
through `upgrade` and `deferred`, `None` everywhere else. `None` means no IR cache for that package.

**Fills and repairs parse an owned copy of the ENG-113 entry** (Fable r5 P3; rev. 6's snapshot, made
concrete). When a package will be written (a miss or a repair):
1. the ENG-113 entry is read with `CacheDir::read_owned` (Design 1) and checked with ENG-113's own
   `check_entry`, which verifies its blake3;
2. if that check passes and its digest equals the carried digest, the package is parsed from the
   owned texts, and the IR entry is published under that digest (after the round trip, Design 3);
3. **otherwise** (the entry changed since it was mapped, or the check fails), the package is parsed
   from the map, exactly as today, and **nothing is published**; a warning names the package and
   `fill_text_changed` counts it.

So an entry always holds the IR of exactly the bytes its name digests. A change A→B→A cannot slip
through: the bytes parsed are the bytes hashed. The trees the process itself uses keep the mapped
`SourceText`, as today. Cost: one read and one blake3 of the package's text, on fills only. A hit
needs neither.

## Scope of the guarantee (orchestrator, rev. 7; sol r4 P1-1, Fable r5 P2-3)

The cache is the user's own private data, in ENG-113's private directory: on Linux the directory
and every entry must be this user's with no group or other write bit, checked on the open handle.
Deliberate tampering by someone who can write that folder is outside the guarantee: that person can
already replace the binary.

Within that scope, **an accepted entry decodes to exactly IR a fresh parse produced**, because:
- **(a) blake3:** the reader accepts only an entry whose whole-envelope blake3 matches, so its bytes
  are the bytes a writer of this key published. Accidental corruption and partial writes are
  refused (nothing fsyncs, and a crash can leave a renamed file of the right length with zero
  content);
- **(b) the round trip at publish:** a writer publishes a file's encoding only after decoding it and
  checking both that it equals the fresh IR (`decode(encode(ir)) == ir`, which sees a dropped
  field) and that re-encoding it gives the same bytes (`encode(decode(encode(ir))) ==
  encode(ir)`, which sees the raw `Origin.kind_id`: `Origin::eq` treats two ids with one name as
  equal; sol r5) (Design 3);
- **(c) a deterministic decoder:** for given bytes, the decoder of one binary (one key) always gives
  the same IR.

So consumers handle accepted IR by definition: whatever they already do with a fresh parse. That is
why rev. 7 **drops** rev. 6's safety bounds on the read side: the budget adapter, the decode charge
limit, the expression and statement depth limits, the three-colour walk, the one-parent and
reachability checks, and the per-`Origin` char-boundary and kind-id checks. sol r4 P1-1 showed the
charge adapter could not be an honest heap cap (capacity reserved before the first element, and
allocations behind `Box` and `Arc<str>`, escape it), and the walk checks only bounded IR a fresh
parse produces anyway. Under (a)-(c) none of them catches anything, and each would cost code and
time on every hit. No other per-file check was found that is cheap and catches something (a)-(c)
miss.

**Kept: integrity checks, not safety bounds.** The per-file encoded-size cap (64 MiB), the exact file
count, gap-free index ranges and no trailing bytes after each file's decode. They keep a damaged
frame from being read at all.

## The codec

1. **serde derives on the IR types**, always on. `al-syntax` adds `serde` with `derive` and `rc`
   itself (`TypeText` is `Arc<str>`).
2. **`Origin`** stores the grammar symbol id (`kind_id: u16`; `u16::MAX` is `ERROR`), encoded as is.
   Ids are stable within one key (one image).
3. **`ActionDecl.kind`** (the IR's only `&'static str`) gets a hand-written pair over its keyword
   set; any other value is a decode error.
4. **Structural equality:** `PartialEq` and `Debug` on `AlFile` and every IR type that lacks them
   (`Origin`'s exists). The publish round trip (Design 3) and the checks use `==`. A derived
   `PartialEq` compares every field, so the round trip catches a field the codec drops. A test also
   fails on any `serde(skip` or `serde(default` under `crates/al-syntax/src/ir/`, so the rule stays
   visible in review.

## Lowerer state: scratch, proven (a small product change ENG-110 makes)

(Kept; closed by sol r2-r4.) `lower_file` clears `BUILD_CONTEXT` at its top, and
`with_build_context` truncates through a drop guard, so an unwind restores it. The other five
thread-locals are reset at file entry or before each read, or decide sharing only. Tests: unwind
restore (proof: remove the guard, red) and entry reset with a hand-seeded stale context on a clean
and a `Recovered` fixture (proof: remove the clear, red).

## Design

1. **ENG-113's `CacheDir`, refactored, not copied**, in `src/snapshot/cache.rs`, ENG-113's behaviour
   unchanged: `CacheDir` becomes `pub(crate)`; `write(name, replace, writer)` takes the entry's bytes
   from a writer closure (ENG-113's `write_entry` becomes one); `read_owned(name, cap)` opens as
   today and reads at most `cap` bytes; `subdir(name, create)` opens a child directory through the
   handle, checked private, and with `create` makes it first (`mkdirat`, 0700). Only a writer creates
   `ir/` and `ir/<key>/`. `lock(name)` gains a shared and a non-blocking form.
2. **Layout:** `<root>/ir/<IR_KEY, 64 hex>/<text digest, 64 hex>.bin`. `<root>` is ENG-113's cache
   directory. `ALSEM_IR_CACHE_DIR` moves only the `ir/` tree; the text cache stays where it is. It is
   for the acceptance's fresh cold roots and the cache's tests.
3. **Envelope and publish gate.**
   - **Envelope:** header (magic `ALSEMIR\0`, format, full key, text digest, the diagnosis fields,
     file count, payload length); an index of `(offset, len, stored)` per file in
     `SourceRoot.files` order; the payload; a trailing blake3 over every byte before it.
   - **Publish gate** (orchestrator, rev. 7): before a package's entry is written, each file's fresh
     encoding is decoded once, compared with the fresh IR by `==`, and re-encoded and compared with
     the first encoding byte for byte (both halves of (b)). A file whose round trip differs, or
     whose encoding passes 64 MiB, is not stored (its record says so; a hit parses that file and
     counts it `skipped_unstored`). A round-trip failure is a codec defect: it is warned once and
     counted `roundtrip_failed`. The other files are stored. Cost: one decode per file, on a miss
     only.
   - **A hit requires**, in order: the entry within 1 GiB; magic, format, key, digest; count equal to
     `SourceRoot.files.len()`; exact total length; the blake3; stored ranges that tile the payload
     exactly (a not-stored record has length 0; every sum checked); each stored range within 64 MiB.
     Then each stored file decodes from exactly the bytes its final index record assigns it, ending
     at that range's end (no trailing bytes; sol r5).
4. **Owned, bounded reads**, not a map: the bytes decoded are the bytes hashed. Peak: one package's
   entry during its decode.
5. **The seam:** `src/program/dep_summary.rs`, `parse_for_build_observed` (line 62, the per-file call
   at 122) and `parse_dep_bodies` (line 175, the call at 195), each calling
   `snapshot::parse::parse_file` per file in a `par_iter` (unchanged on `bbec5165`). One function
   serves both: per package, once, the owned read and envelope checks; per file inside the
   `par_iter`, decode in place of `parse_file`. On a fill, the loop parses from the owned copy,
   encodes and round-trips; the package's buffers are published at its end, then freed.
6. **Small packages are skipped** below one fixed constant, measured once with its table.
7. **Who turns it on.** The seam's per-package function takes an `Option<&IrCache>` (root, key,
   verify flag, report sink); `None` is today's behaviour. The production entries reach the seam
   from `ProgramContext` through many builder paths, so threading a parameter from each `main`
   would touch every builder signature. Instead (plan decision, 2026-11-21), the carrier is a
   process-level installation, the same pattern as `claim::set_dir`: only the `main`s of `alsem`,
   `aldump` and `al-call-hierarchy` call `ir_cache::install_from_env()`, which reads
   `ALSEM_IR_CACHE`, `ALSEM_IR_CACHE_DIR`, `ALSEM_IR_CACHE_VERIFY` and `ALSEM_IR_CACHE_REPORT`. The
   two seam entries read `ir_cache::installed()` on the calling thread before entering the pool and
   pass it down; the key is resolved there, lazily. (Until TLS-116 the batch state
   `claim::dir()` was resolved there too, so a batch run never used the cache.) **Not installed
   anywhere else:** `mint-goldens` (Fable r5 P3), and the test binaries that build models in
   process (`program_resolve_harness`, `lsp`, `l4_summary_differential`, the lib's tests), whatever
   the environment says. The cache's own tests and `lib_cdo`'s round trip call the parameterized
   function with an `IrCache` on a temporary root. `aldump --session-refresh --verify` builds its
   reference model under `ir_cache::Suspend`, a guard that turns the installed cache off while it
   lives.
8. **Disk:** 1 GiB per entry is a cap. The total under `ir/` has a 4 GiB **soft target**; cleanup is
   best effort (next item). If this process's own key folder alone passes 4 GiB it publishes nothing
   more and warns once. **On a lane host the 4 GiB will sit full** (Fable r5): every lane rebuild
   that runs `output_cdo` or a CDO one-shot adds a ~144 MiB key folder, so the folder is pruned to
   the target, not below it, most of the time.
9. **Locks and cleanup, best effort:**
   - **Per-entry publish lock, non-blocking** (orchestrator, rev. 7): a writer takes the entry's lock
     without waiting; if it is busy, another process is publishing the same entry, so this one
     publishes nothing, keeps its parsed IR and counts `publish_skipped_busy`.
   - **Key-folder lock** (review r1, sol and Fable): `<root>/ir/<key>.lock`, BESIDE the key folder,
     never inside it and never removed, so removing a folder never unlinks a held lock. Every
     publisher holds it SHARED from before it creates the key folder until its publish ends, and a
     `last-used` writer while it writes. A prune removes a key folder (with or without `last-used`)
     only while holding that lock EXCLUSIVE, taken without blocking, and reads `last-used` under it;
     the size pass reads it again under the lock; if busy, it skips the folder this round. The lock
     files accumulate: one empty file per binary (key) that ever used the cache on the host
     (orchestrator decision, 2026-11-21).
   - **`last-used`:** published by rename (never written in place) at a process's first hit or
     write, and again when older than a day.
   - **A key folder without `last-used`** (a writer that died between `mkdirat` and its first
     publish, or a folder from elsewhere) is aged by the folder's own mtime instead, and removed
     only when that is older than 7 days, under its lock; it is never removed by the size rule.
   - **Throttle, checked periodically:** at a process's first cache use AND at each publish, if
     `<root>/ir/last-prune` is older than an hour, the process takes `<root>/ir/prune.lock` without
     blocking, re-reads `last-prune` under it, and only if still older publishes a new one and
     prunes.
   - **What it removes:** key folders whose `last-used` is older than 7 days; then, oldest first,
     key folders until under the soft target, never this process's own; temp files of this cache's
     name shape older than a day. Only names of these exact shapes, through the checked handle,
     never following a symlink.
   - **A pruned key folder is reopened** (Fable r5 P3): the `IrCache` keeps no handle to its key
     folder; it opens `ir/<key>` through the root handle for each package (one `openat`), and a
     writer re-creates it when it is gone. So a long-lived server whose folder was pruned misses,
     refills and publishes again.
   - **Overshoot, stated plainly:** the target can be exceeded by whatever is published between two
     prunes (at least an hour apart), and by key folders in use (skipped). It is a target, not a
     quota.
   - `alsem cache prune` maintains a different cache and is not changed.
10. **`parse_snapshot` stays uncached**: its callers outside `dep_summary` are test modules.

## `alsem batch` (TLS-116): on, the cache folders protected from job writes

Until TLS-116 a batch run (the parent and every worker) never used the cache, because TLS-109's
write claims did not cover the cache folders. ENG-113's text cache had the same gap, though it was
on in batch: a job's stream or `--out` in the text cache folder, or a hard link to an entry, would
have truncated a file other engines map (SIGBUS) or left a job output for the cache to replace.

Now the parent and every worker use the cache, and the cache folders are protected
(`engine::batch::cache_guard`; the text cache folder, and the IR cache's `ir/` folder wherever
`ALSEM_IR_CACHE_DIR` puts it, when this process uses the IR cache):
- **One snapshot per run** (`cache_guard::take`, in the parent's check). First it MAKES the folders
  the run's caches would use, exactly as the caches make them: the text folder always (every batch
  run creates it), `ir/` when the IR cache is on and has a key. So a spelling such as `missing/..`
  resolves now as it will later. A folder that is no directory afterwards (an override naming a
  file, a dangling symlink) turns its cache OFF for the whole run, parent and workers
  (`snapshot::cache::disable`, `ir_cache::disable`): no cache ever writes in a folder the snapshot
  does not hold. Then it records the identity of every directory and file inside the folders, each
  tree walked once (the default `ir/` lies in the text folder): unix device, inode and birth time
  (`statx`; a reused inode number gets a new birth time, so it is not taken for a pruned file);
  Windows volume serial and 128-bit file id. A name removed during the walk (a concurrent prune,
  or a Windows delete pending) is skipped; any other walk error refuses the run. The parent writes
  the snapshot (its on/off flags, `0` or `1` only; its counts; a blake3 over flags, counts and
  body) to the claim folder through a temporary file and a rename, and claims it so no write can
  take it; a worker reads it at its start and fails closed (exit 4, no job run) when it is missing
  or damaged, or says a cache is on with no folder.
  Workers never walk. Windows: one open per entry (TLS-109's `read_file_id`);
  `FileIdExtdDirectoryInfo` could give the ids from the listing, not done because no Windows
  machine runs these tests. Measured 2026-11-22 on server1's real `~/.cache/al-ch-snapshot-cache`
  (134,395 entries), `release-fast`, 3 runs each: the walk 381-420 ms (`ALSEM_IR_CACHE=off`); with
  the IR cache on, the span also hashes the binary for the key (once per process anyway): 514-594
  ms warm, 1,885 ms on the first run after the build.
- **The check** (`manifest::check_write_sets`, exit 3 before any job) refuses a write whose
  resolved destination lies in a cache folder; a write whose name, as spelled, lies in one (a
  symlink `ir/last-prune` pointing outside is still the prune's name, which it would replace); a
  write that passes through a symlink whose own name lies in one (every hop of the resolution is
  checked: `report → ir/last-prune → outside`, `cache_guard::link_hops`); an existing file that is a cache file under any name (a hard link, a bind mount of a file); a
  directory target that holds a cache folder; and a write into the run's claim folder.
- **Every open** (`claim::open_for_write`, `open_trace`, the report's registration and appends)
  checks the same paths first, before anything is created; then opens an existing file WITHOUT
  creating it and checks the handle's identity against the snapshot's files; only a missing file is
  created, exclusively, once its folder passed, and is not handle-checked (it is new). A dangling
  symlink outside the caches is still written through. A shard directory is made one component at
  a time, each checked just before it is made (`claim::make_dir`): a component made earlier can
  make a later spelling reach a cache folder (`missing/../alias/new`). So an alias made after the check is refused at the open
  (exit 1) and nothing is left in a cache folder. The folder tests are path-based: a path another
  process changes during the run is outside TLS-109's scope. The run's own writes make no links. A
  cache file published during the run is not in the snapshot, but its folder is.
- **Cache publishes need no claim.** Every cache file is made under its folder with a fresh name
  (`O_EXCL` temp files, then a link or rename through the folder's handle, symlinks never
  followed); a lock file is opened without truncation; a prune only removes names in its folders.
  With no job writing in or above a folder, a publish cannot land on a job's output.
- **The report** (`ALSEM_IR_CACHE_REPORT`) is one of the run's writes: the check compares it with
  every job write and input; the parent then registers it in the claim folder (through the guarded
  open with `O_NONBLOCK`, so a FIFO with no reader is refused instead of hanging the run; a regular
  file only, never truncated) and keeps that handle; a worker opens the path once, the same way and
  without creating it, and appends only when the file is the registered one and no cache file
  (`claim::append_shared`). A refused append is a warning, never a failed run. The parent writes
  its `end` line before the claim folder goes. Planning (`plan`) builds no model, so no report
  line comes before the registration; the batch test asserts a `start` line for every process.

Tests (`tests/cli/cli_ir_cache.rs`, Linux; `claim::tests::registration_refuses_what_it_must`):
two workers on a cold root publish the entry and report, and on the warm root each hits every
file from disk (one batch in the parent too); a stream in the IR folder, in the text folder and
through a link to the IR folder; a shard directory holding the IR folder; a cache folder spelled
through `missing/..`; a `--out` hard-linked to an IR entry, a text entry and a file in the default
`ir/` while `ALSEM_IR_CACHE_DIR` points elsewhere (a bind mount of a file is the same identity
test; making one needs root); a late hard link, a late symlink and a late shard directory; a trace
in a cache folder; a concurrent prune during the walk; a stream on the report; a report path
swapped after registration; each guard of the registration; a dangling-symlink cache folder
whose target a shard job makes (IR and text); a cache name linking outside; a FIFO report; a
worker with a missing or damaged snapshot; writes into the claim folder; a dangling-symlink
stream outside the caches; a reused inode number (`cache_guard::tests`); a symlink hop in a
cache folder (report and stream); and a shard directory that becomes reachable only once its
first component is made. Each guard has a recorded discrimination proof, with one exception:
skipping the handle check for a file the open itself created (`O_EXCL`). It is sound by
construction (a file that did not exist a moment ago is no cache file) and only matters on a file
system without birth times, where a new file could reuse a pruned cache file's inode number; a
test would have to predict that number. The snapshot's "(no header)" case shares its refusal with
the magic and field tests (each refuses it alone), so it has no break of its own. Every batch test
helper points its runs at a temporary `XDG_CACHE_HOME` (`cli_batch_writes::temp_cache_home`).

## Correctness checks

- **Decoded equals parsed:** every CDO dependency file decoded equals a fresh parse (`lib_cdo`, with
  its own `IrCache`).
- **Determinism:** the CDO set encoded twice in different thread orders gives the same bytes.
- **VERIFY** (`ALSEM_IR_CACHE_VERIFY=1`): every hit is also parsed and compared by structure; a
  mismatch fails the process loudly, naming package, file and key. Each compared hit counts
  `verified_hits` (sol r4 P1-2), so a phase can prove the comparison ran. In the gate it runs in
  `output_cdo_impact`, whose `alsem impact` calls are real `alsem` processes. Never in a timed run:
  cdo-speed and the one-shot protocol refuse to start with it set.
- **Tests still exercise the parser:** `.cargo/config.toml` has no `[env]` section today; adding one
  with `ALSEM_IR_CACHE=off` is new work. It covers the `alsem` and `aldump` processes cargo-run tests
  spawn; an explicit value in the environment wins (cargo's documented `[env]` rule without
  `force`). nextest honours `[env]` (measured in step 1), so `ci-steps` and `check-goldens` export
  nothing. In-process test binaries are unaffected either way (Design 7).
- **Counters** per process:
  - `seam_files`: files that reached the seam, counted before any cache decision;
  - `eligible_files`: files of packages with a digest, a cache in use and above the small threshold;
  - `disk_hits`, `verified_hits`, `parsed_miss`, `refused`, `skipped_small`, `skipped_unstored`,
    `skipped_off` (cache off, no digest, no key);
  - `published_packages`, `roundtrip_failed`, `publish_skipped_busy`, `fill_text_changed`;
  - `inproc_reused_packages`: packages served from `DepCache`'s in-process tier (they never reach
    the seam), reported apart from disk hits;
  - invariant: `seam_files = disk_hits + parsed_miss + refused + skipped_small + skipped_unstored +
    skipped_off`, asserted at exit in debug builds and checked by the acceptance.
- **The report file** (`ALSEM_IR_CACHE_REPORT=<file>`), appended with `O_APPEND`, one `write` per
  line:
  - a `start` line (pid, command, key) at the process's first seam use, and an `end` line (pid,
    counters) at its exit. A `start` without an `end` is a process that died or was killed before
    its exit path; the phase rules count it as a failure;
  - on Windows, or with no key, one line `{"pid": …, "ir_cache": "unsupported"}` (or `"no key"`)
    instead;
  - **report sites** (Fable r5 P1-1): every `main` exit of `alsem`, `aldump` and `al-call-hierarchy`
    that follows a seam use writes the `end` line. That includes the engine child: `alsem
    engine-host` ends by returning from `run_engine_host` (`src/bin/alsem.rs:4331`; `serve_with`
    returns, then `ExitCode::SUCCESS`), which is where its `end` line goes. Checked on `c80f1391`:
    the only `process::exit(0)` in `src/engine/host/child.rs` (line 425) is the `ExitAfterOpen` test
    hook, not the normal path; Fable r5 cited a site near line 924, which this tree does not have
    (line 928 is `serve`'s closing `Ok(())`). The plan lists every exit of these `main`s and writes
    the line at each one. A child killed after the front end's shutdown grace writes no `end` line,
    and that is a failure by the rule above.
- **`ParseStatus::Recovered`** is replayed exactly; its doc is fixed to say the cache stores it.

## What still needs the source TEXT after an IR hit

(Kept.) `LineTable` positions; snippets in explain and digest; the row cache's `SourceText::ptr_eq`
fast path; `recovered_file_paths` and diagnostics that quote source.

## Many processes filling the cache at once

Each cold process parses, encodes and round-trips. At a package's end it takes the key-folder lock
shared and tries the entry's lock without waiting: busy means another process is publishing it, and
this one skips. Otherwise it reads the entry again and publishes only if none is there. Write
amplification: one entry per package. No lock spans a gate's start. Measured in Acceptance 6.

## Failure cases

- **Envelope refused** (any header field, length, hash, ranges, cap): the package is parsed (a fill,
  from the owned copy) and the entry is replaced under the lock. The next run hits.
- **A stored file fails to decode, or leaves trailing bytes, under a valid hash.** By (a)-(c) this
  cannot happen; if it does, it is a defect: the file parses, a warning names package, file and key,
  and `refused` counts it. The package is refilled; if the refill's bytes equal the stored bytes,
  nothing is rewritten (a rewrite would loop on every run).
- **The ENG-113 entry changed before a fill:** parsed from the map as today; nothing published.
- **The cache is not writable or not private, or there is no key** (Windows, an unreadable or
  unmarked image): no IR cache, logged once.
- **Tests:** each envelope refusal with a hand-written entry in a temporary root, asserting the model
  equals the cache-off model and that a second run hits the repaired entry; the round-trip gate (a
  test codec seam that alters one field on decode: the file is not stored, `roundtrip_failed` is 1;
  proof: skip the comparison, red); the fill rule (a test seam changes the ENG-113 entry between the
  map and the owned read: nothing is published; proof: publish under the carried digest without
  checking, red).

## Acceptance

The judgment rules are fixed before any run. Every record carries the master and head SHAs, the
workspace SHA (`bc3ccb18`), the `.app` closure stamp (`dependency_closure_sha256`) and the keys; a
change in any voids the run. All timed and identity runs are on Linux.

**A first-ever run on a host needs three runs before a warm sample:** the first fills ENG-113's text
cache, the second fills the IR cache, the third is warm. The untimed runs below include them.

1. **Identity, byte for byte.** The deterministic output files of the CDO commands (fingerprint,
   analyze primary and `--scope all`, the call-graph histogram, and the explain, query and digest
   outputs) from **master**, **head-off**, **head-cold** and **head-warm**: all equal byte for byte.
   A file that differs between two runs of one arm is nondeterministic: listed, filed as a bug, and
   left out only of this comparison. Plus goldens (cache off), the refresh verify in both modes with
   a cache-off reference build, and `cdo-gate --only harness,l4,output`.
2. **Coverage, never vacuous**, per process, from the report lines:
   - every warm sample: `eligible_files > 0`, `disk_hits > 0`, `disk_hits = eligible_files −
     skipped_unstored`, `refused = 0`;
   - every cold sample: `eligible_files > 0`, `disk_hits = 0`, `published_packages > 0` and equal to
     the eligible packages, and the entries present on disk in that sample's root;
   - every off sample: `disk_hits = 0`, `published_packages = 0`, `skipped_off = seam_files`.

   Any `roundtrip_failed > 0` on CDO fails (sol r5): it is a codec defect, not an allowed skip.
   Report lines come only from instrumented head runs; master predates them and is judged on its
   output bytes and counts only.
   A missing report, a `start` without an `end`, or a sample meant to use the cache with no eligible
   files fails.
3. **One-shot timing** (bars (a), (c)): `alsem analyze <CDO_WS>`, `release-fast`, in a timed CDO slot
   (`scripts/cdo-slot run … --timed`), counted as cdo-speed counts since TLS-83 and TLS-111: user
   `instructions` and `cycles` per PMU (`cpu_core`, `cpu_atom`), every event of one PMU with exactly
   equal running time, and the instructions' summed running time at least the call's CPU less 2 %
   less 0.05 s (else the sample is discarded and repeated). No pinning. User and sys CPU and wall are
   recorded.
   - Arms per round, order rotating: master; head-off (`ALSEM_IR_CACHE=off`); head-cold (a fresh,
     empty `ALSEM_IR_CACHE_DIR` per sample); head-warm (one root filled untimed before the rounds).
     The text cache is equally warm for every arm.
   - **Judgment, fixed now:** per round, each head arm's summed instructions over that round's
     master. The verdict is the **median of the per-round ratios over 6 rounds**: head-warm ≤ 0.70,
     head-cold ≤ 1.05, head-off ≤ 1.01. A median within 0.01 of its limit runs 6 more rounds and the
     median of all 12 decides. A suspend or "UNDER LOAD" label voids and repeats that round.
4. **cdo-speed after ENG-110.** The judged `build` row runs with `ALSEM_IR_CACHE=off` on both sides,
   so dependency parsing stays timed. Then each side's folder is filled untimed (the slowdown said in
   Goal), and a `build (IR warm)` row is reported. Its engine children's report lines follow the
   phase rules of item 5 (per side: at least one line with hits, none with refusals, no `start`
   without an `end`); a breach labels the verdict. A `--ir-cold` mode reports a build on a fresh root
   per side. `child_env` and `scripts/bench_ab.py` (line 39) keep `ALSEM_IR_CACHE`,
   `ALSEM_IR_CACHE_DIR` and `ALSEM_IR_CACHE_REPORT` on purpose while stripping every other
   `ALSEM_*`, and refuse `ALSEM_IR_CACHE_VERIFY`.
5. **cdo-gate** (orchestrator, rev. 7; sol r4 P1-2, Fable r5 P2-1):
   - exports `ALSEM_IR_CACHE=on` and a report file **only for `output_cdo` and `output_cdo_impact`**
     (every other phase keeps the cargo default, off; the in-process binaries ignore it anyway), and
     `ALSEM_IR_CACHE_VERIFY=1` for `output_cdo_impact` only. No prefill.
   - **Phase rules over the set of a phase's report lines** (Linux):
     - `output_cdo` passes when the report exists, no `start` lacks an `end`, no line has
       `refused > 0`, the lines' `eligible_files` sum to more than 0, and at least one line has
       `disk_hits > 0` or `published_packages > 0`;
     - `output_cdo_impact` passes when the report exists, no `start` lacks an `end`, no line has
       `refused > 0`, at least one line has `disk_hits > 0`, and the lines' `verified_hits` sum to
       more than 0.
   - **Windows:** the lines say `unsupported`; the IR rules are skipped with that label, and both
     phases still run and pass or fail on their other checks, so a Windows `cdo-gate --no-speed`
     works as today.
   - These rules fail from the start rather than starting advisory (mantra 4's default), by the
     orchestrator's decision: a missing or empty report means the cache path was never exercised,
     the vacuous pass the rules exist to stop.
   - The summary prints each phase's counters. A full gate's wall time is reported (no gain
     expected).
6. **Memory and disk:** the census, warm against master; peak RSS of one cold one-shot and of four on
   one fresh root, with temp bytes, write amplification and `publish_skipped_busy`; the key folder's
   size for CDO; the largest entry; the round trip's cost; the small-package table; the blake3 cost
   of the image.

## Settled in review

1. **Key:** the running image, hashed in full every time, no sidecar; Linux only in v1; the image is
   self-contained (`NEEDED` set and symbol checks); the marker is checked on the shipped artifact.
2. **Fills:** parse the owned, checked ENG-113 entry; publish only under its digest.
3. **Guarantee:** (a) blake3, (b) the round trip at publish, (c) a deterministic decoder; the user's
   private folder is trusted. No read-side safety bounds; integrity checks only.
4. **VERIFY:** `output_cdo_impact`, for good, with `verified_hits`; never timed. In-process test
   binaries and `mint-goldens` get no cache.
5. **Gain:** the installed binary; a gate gets none.
6. **`alsem batch`:** off (then), tested through the real run with a positive control. (TLS-116 turned it
   on, with the cache folders protected from job writes: see the `alsem batch` section.)
7. **Cleanup:** best effort, periodic, coordinated per key folder; non-blocking entry locks.

## Left for the owner

None. The orchestrator confirmed the disk numbers (1 GiB per entry, 4 GiB soft target) and
batch-off (then) on 2026-11-21.

## Future (ideas only; ENG-110 does not build them)

- **Windows (TLS-117):** read the running image natively and prove the handle is the loaded file;
  then turn the cache on there.
- **Batch (TLS-116):** the claim check reserving the cache folders, for the ENG-113 text cache too.
  Done: see the `alsem batch` section.
- **A narrower key.** A later task could key the cache on a build-time hash of exactly what produces
  the cached IR: the `al-syntax` sources, the grammar's compiled inputs and the IR codec's schema,
  computed in a build script. Then a change that touches only the engine would still hit, and a gate
  could gain. It is admissible only if a review proves that nothing outside that set reaches the
  cached bytes. That review must close the holes revs. 3 and 4 already found:
  - cargo `paths` overrides and `--config` arguments, which change code without changing
    `Cargo.lock` or any file a build script sees;
  - the tree-sitter runtime's own native build (its C code and compiler flags);
  - cfgs, which a root crate's build script cannot see for its dependencies;
  - the dependency crates' code (tree-sitter, serde, postcard and the others between the text and
    the decoded IR).

## Builds on ENG-113

The text digest added to `CacheDir::read`'s and `cached_source_in`'s results, `SourceRoot` and
`DepCache`; the `CacheDir` refactor (Design 1). ENG-113's behaviour does not change.

## Owner design principles applied

From `/coord/owner/design-mantra.md`:
- **1 and 5 (correct first; unknown is not no):** the image key closes the build by construction; no
  key (Windows, an unreadable or unmarked image) turns the cache off; an entry holds the IR of the
  exact bytes its name digests, and only IR that survived its own round trip.
- **3 (root cause):** the `BUILD_CONTEXT` leak is fixed in the lowerer; a round-trip failure is a
  codec defect, warned and counted, not hidden.
- **4 and 9 (measure; commit first):** the one-shot rule is written before any run; coverage cannot
  pass on an empty or all-skipped count. The cdo-gate IR rules fail from the start, an orchestrator
  decision against 4's advisory default, stated where it applies.
- **6 (say what you don't know):** the gate's lack of gain and the full 4 GiB on lane hosts are said
  plainly.
- **7 and 8 (tests that fail; real use):** every guard has a stated break; batch-off (then) is tested through
  the real parent and workers, with a positive control.
- **10 (reproducible baselines):** each record pins the workspace, the `.app` closure and the keys.
- **13 and 14 (right-sized; reuse):** the read side's safety machinery is gone because (a)-(c) leave
  it nothing to catch; ENG-113's `CacheDir` is refactored, not copied.
- **16 (clean cutover):** the off switch exists for tests and measurement only.
- **24 (delete only what you made):** cleanup removes only this cache's exact name shapes, under the
  key-folder lock.
- **26 (state the threat model):** written as the guarantee's scope.
- **35 (file work when found):** the batch and Windows follow-ups are tasks to file.

## Review history

- **rev. 1:** written.
- **rev. 2:** Fable r1 folded in (3 P1, 6 P2, P3s), plus the orchestrator's VERIFY, tests and
  measures additions.
- **rev. 3:** Fable r2 folded in (baked-hash trigger test, one home for the test switch, VERIFY labels
  zero hits).
- **rev. 4:** sol r1 (5 P1, 2 P2) folded in: whole-envelope hash, build-input key, `BUILD_CONTEXT`
  reset, owned reads, batch off (then), the one-shot protocol, digest in the path, disk caps.
- **rev. 5:** sol r2 and Fable r3 folded in, with the orchestrator's binary key: decode budgets, the
  stmt/block walk, the text re-hash, the batch real-run test, the paired acceptance rule.
- **rev. 6:** sol r3 and Fable r4 folded in: no sidecar, Linux only, static-link and tool-binary
  guards, owned-snapshot fills, the guarantee's scope, the decode charge and depth limits, VERIFY in
  `output_cdo_impact`, the honest gain, eligible-file coverage, best-effort cleanup, the narrower-key
  idea under Future.
- **rev. 7:** sol r4 (2 P1, 1 P2) and Fable r5 (ready for a plan; P1-1, P1-2, P2s, P3s) folded in,
  with the orchestrator's decisions. The read side is simplified: the guarantee rests on blake3, a
  round trip at publish and a deterministic decoder, and rev. 6's budget adapter, charge and depth
  limits, three-colour walk, one-parent, reachability, char-boundary and kind-id checks are dropped
  (sol P1-1, Fable P2-3). Windows phases run with the IR rules labelled unsupported, hits required on
  Linux, `verified_hits` required in `output_cdo_impact` (sol P1-2). Report `start`/`end` lines and
  phase rules over them, the engine child's exit site named and the cited line corrected (Fable
  P1-1). The marker is a `#[used]` static read through `black_box` and checked in the Linux release
  jobs and a local test (Fable P1-2). The allowed `NEEDED` set is asserted (sol P2). No gate prefill,
  the switch only in the two output phases, the full 4 GiB on lane hosts said (Fable P2-1).
  Non-blocking entry locks, folders without markers aged by mtime, a pruned folder reopened (Fable
  P2-2, P3). Fills from `read_owned` plus `check_entry`, parse from the map on a mismatch,
  `mint-goldens` gets `None` (Fable P3).
- **rev. 7, amended with the plan (2026-11-21):** sol r5 (ready for a plan) and Fable r5 agreed. sol
  r5's plan notes folded in: the round trip also compares re-encoded bytes (raw `kind_id`), each
  file decodes exactly its final index range, any `roundtrip_failed` on CDO fails, reports come only
  from head runs. Design 7 states the plan's carrier (a process-level install, as `claim::set_dir`).
  Follow-ups cited as TLS-116 and TLS-117; the orchestrator confirmed the disk numbers and
  batch-off (then).

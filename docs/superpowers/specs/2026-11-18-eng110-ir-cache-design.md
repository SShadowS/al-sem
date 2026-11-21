# ENG-110: persistent cache of parsed dependency IR (design, rev. 6, 2026-11-21)

Status: a spec for review. No product code. ENG-113 (the package-text cache this one stands on)
has landed. Rev. 6 is written against master `c80f1391`. It folds in sol's review r3, Fable's
review r4 and the orchestrator's decisions on both. The measurements come from a throwaway
prototype that never lands: branch `tools/eng-110-proto` (`bf12e42a`), with serde derives on
`al-syntax`'s IR and `examples/eng110_probe.rs`.

"Not verified" marks every claim this revision could not check against source or a run.

## Goal, gain and bar

Every engine process parses and lowers every dependency file again: about 24 CPU-s per process on
CDO. A cache hit replaces that with a read, a check and a decode.

**Where the gain lands** (Fable r4 P2-1, orchestrator). The key is the running binary (Key, below),
so one binary fills its entries once and every later process of THAT binary hits:
- **the installed `alsem`** (`~/.al-sem/bin/alsem`) and shipped releases: every one-shot call after
  the first, and every restart of a resident server (`alsem mcp`, the LSP's engine child), which
  today rebuilds its model from scratch;
- nowhere else to speak of.

**A gate gets no gain.** A gate runs on a tree it has just built, so every binary in it is new and
starts cold. The in-process test binaries get no cache at all (Design 7). The `alsem` processes that
`output_cdo` and its impact phase start share one binary, but cdo-gate prefills it (Acceptance 5),
which costs about what the later hits save. cdo-speed gets slower: it fills each side's folder,
untimed, on every run with a new head. The CHANGELOG says the same once this lands.

**Bar** (measured as Acceptance says; its judgment rule is fixed there before any run):
- (a) **Warm cache, one-shot `alsem analyze` on CDO:** at least 30 % fewer user-mode instructions
  than master.
- (b) **Byte-identical output:** master, head with the cache off, cold and warm all give the same
  bytes; goldens; the refresh verify in both modes against a cache-off rebuild; the CDO subset.
  Every warm run must show actual disk hits over a positive count of eligible files.
- (c) **A cold miss** costs at most +5 % instructions over master; the cache off at most +1 %.
  Every new binary pays (c) once. Peak RSS of a cold miss, alone and four at once, is reported.

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
- **Measured in step 1:** the blake3 cost, the largest package's entry and text, each file's decode
  charge (Codec 4), the deepest expression chain in fresh IR (Codec 5), the small-package table.

## The key: the running image, hashed every time

**Decision (orchestrator, revs. 5 and 6).** `IR_KEY` = blake3 of the running executable's bytes,
plus the IR format number. Whatever produces or reads the IR is compiled into those bytes: the
grammar's C code, the tree-sitter runtime, the lowerer, the codec, features, cfgs, profile, target,
`--config` and `paths` overrides, the toolchain. Any change to them changes the key. The price is
the one said above: a gate gets no gain.

- **No sidecar** (sol r3 P1-1, Fable r4 P2-2). Rev. 5 cached the hash by file stamp; a stamp is
  metadata, not proof of the bytes, and a restored record could carry another binary's key. Each
  process hashes its full image once, **lazily at its first seam use** (a process that parses no
  dependency pays nothing).
- **Linux only in v1.** The image is read through `/proc/self/exe`: the inode the process runs,
  even when the path was rebuilt or unlinked since. If it cannot be opened or read, the cache is
  off for that process (fails closed).
- **Windows: the cache is off in v1.** Reading the running image there is not verified natively;
  opening a path that names the image does not prove the handle is the loaded file. Future names
  the work. Other Unix systems are not a target.
- **Static-link assumption** (verified by Fable r4 on the `release-fast` binary): the binary links
  only libc and libgcc_s dynamically and imports no locale or `isw*` symbols, so no shared library
  changes how a text parses or decodes. A test pins it: it lists the dynamic symbols the `alsem`
  binary imports (`nm -D --undefined-only`) and fails on any locale or wide-character
  classification symbol. Proof: add a call to `iswalpha` through `libc`, red.
- **A tool's binary is not the key.** Under valgrind or a similar loader, `/proc/self/exe` is the
  tool. The `alsem` binary carries a 16-byte marker constant; the hashing pass also looks for it, and
  without it the cache is off. Test: hashing a file without the marker yields no key.
- **Diagnosis only, in each entry's header:** the grammar commit (`88bb6871` today, baked by
  `al-syntax`'s build script, `unknown` if none) and the binary's version string. Never compared.

**Tests:** two binaries differing in any byte use different key folders; one fixture package in one
root gives two entries and no cross hits (proof: key on the format number alone, red). An engine
whose executable path is replaced while it runs still keys on its own image (proof: key through
`current_exe()`'s path, red).

## The text: ENG-113's verified digest, carried; fills parse an owned snapshot

(Carrying kept from rev. 4; checked in `src/snapshot/cache.rs`.) ENG-113's entry header holds a
blake3 over the package's content hash, its JSON index (paths, offsets, lengths, in order) and its
text (`check_entry`): the **text digest**, naming the exact ordered `SourceRoot.files` under text
format `s9.f2`. `CacheDir::read` returns it with the mapped files; `cached_source_in` returns it as a
third value, `Some` only for a mapped, verified entry. `SourceRoot` gains
`text_digest: Option<[u8; 32]>`, set by `EmbeddedAppProvider`, kept by `DepCache`'s `WeakSource`
through `upgrade` and `deferred`, `None` everywhere else. `None` means no IR cache for that package.

**Fills and repairs parse an owned snapshot** (sol r3 P1-2, orchestrator). This replaces rev. 5's
re-hash after the parse, which a change A→B→A defeats: the parse sees B, the final hash sees A, and
B's IR is published under A. When a package will be written (a miss, or a repair):
1. its texts are copied once from the map into one owned buffer;
2. ENG-113's digest is computed over the package hash, the index and that buffer;
3. the package is parsed from that buffer;
4. the entry is published under that digest, **only if it equals the carried digest**. Otherwise
   nothing is published and a warning names the package.

So an entry always holds the IR of exactly the bytes its name digests. The trees the process itself
uses keep the mapped `SourceText`, as today; if the text changed under the map, this process sees
what it read, which is ENG-113's existing risk, unchanged. Cost: one copy and one blake3 of the
package's text, on fills only; the copy lives for the package's parse. A hit needs neither.

## Scope of the guarantee (sol r3 P1-3, Fable r4 P2-3)

The cache is the user's own private data, in ENG-113's private directory: on Linux the directory
and every entry must be this user's with no group or other write bit, checked on the open handle.
The trailing blake3 guards against accidental corruption and partial writes (nothing fsyncs; a crash
can leave a renamed file of the right length with zero content). Deliberate tampering by someone who
can write the user's private folder is outside the guarantee: that person can already replace the
binary. The structural checks below are defence in depth on top of the hash: they keep a decoder or
codec defect from reaching consumers as IR a fresh parse could not produce.

## The codec

1. **serde derives on the IR types**, always on. `al-syntax` adds `serde` with `derive` and `rc`
   itself (`TypeText` is `Arc<str>`).
2. **`Origin`** stores the grammar symbol id (`kind_id: u16`; `u16::MAX` is `ERROR`), encoded as is
   and validated on decode: `u16::MAX`, or below `node_kind_count` with a non-empty name. Total
   over named and anonymous kinds and `ERROR`. Ids are stable within one key.
3. **`ActionDecl.kind`** (the IR's only `&'static str`) gets a hand-written pair over its keyword
   set; any other value refuses the file.
4. **An enforced decode limit per file** (sol r3 P1-3, orchestrator; rev. 5's `2·S·encoded` was an
   estimate, not enforced). Each file decodes through a budget adapter around postcard's
   deserializer (the pattern `serde_path_to_error` uses: it wraps every visitor and its sequence
   access; not prototyped here). It charges `2 × size_of::<T>()` for every sequence element of type
   `T` it hands out (2 for `Vec`'s doubling) and the length of every string and byte buffer, and
   fails the decode the moment the charge passes the limit.
   - **The limit is 256 MiB per file**, fixed. For scale: all 10,881 CDO files decode to 476.5 MiB
     together. Step 1 records the largest file's charge; the limit must be at least 4× it, else the
     spec is revised with the new number before the code lands.
   - **Encoded bytes per file** are capped at 64 MiB, checked from the index before decoding.
   - **Write side** (Fable r4 P3): a file whose encoding passes 64 MiB, or whose charge passes the
     limit (computed when encoding), is not stored. Its index record says "not stored"; a hit parses
     that file and counts it `skipped_budget`. The package's other files are cached.
   - **Test:** a payload whose sequence lengths claim far more than the limit fails, and a counting
     allocator shows the peak below 256 MiB plus one element. Proof: let the adapter charge nothing,
     red.
5. **IR check before use** (`AlFile::check(text)` in `al-syntax`, where the arenas are private).
   Every decoded file passes it before any consumer sees it; any failure refuses the file, which is
   then parsed (Failure cases), so consumers only ever see IR a fresh parse could produce:
   - every `ExprId`, `StmtId`, `BlockId` and `MemberOriginId` held anywhere is inside its arena;
   - **expressions:** each child id is below its own, true by construction (an `Expr` holds its
     children's ids, which exist only once `Ir::add_expr` returned them). **Block → block:** a
     `Block`'s items hold `PreprocGroup`s whose `branches: Vec<BlockId>` exist before the block is
     added, so the same rule holds (both checked in `crates/al-syntax/src/ir/`). A flat pass;
   - **expression depth** (sol r3 P1-3: `src/program/body/ir_walk.rs` follows expression children
     recursively, at lines 852, 1666 and 2240): computed iteratively in id order
     (`depth[e] = 1 + max depth of its children`, linear because children come first). Over the
     limit, the file is refused and parsed. **The limit** is the deepest chain in fresh IR on CDO
     plus a margin, measured in step 1 and written here before the code lands;
   - **one parent:** every expression, statement and block has at most one parent, so no shared
     child can make a consumer's walk repeat work;
   - **statements and blocks:** an iterative three-colour walk over the whole stmt and block arenas
     (an explicit stack, no recursion) refuses any cycle, and every stmt and block must be reachable
     from a routine body or trigger root;
   - the one-parent and reachability rules are adopted only if fresh IR obeys them: the CDO round
     trip checks all 10,881 files first (not verified yet). A rule fresh IR breaks is not dropped
     silently: the spec is revised with what fresh IR does and a bound that still protects the
     walkers;
   - every `Origin`: `byte_start <= byte_end <= text.len()`, both on `char` boundaries of the text,
     `start <= end`, a valid kind id; the same for every `VarWrapper` range and arm range;
   - `tainted_calls` is strictly ascending (it is binary-searched). `preproc_ranges` is not checked
     for order: `with_build_context` records an inner range before its enclosing one, and
     `Ir::preproc_context` scans every range, so order carries no meaning there.
   - **Tests:** one hand-built malformed payload per rule, each refused and re-parsed, each with a
     discrimination proof; valid round trips of nested `#if` in a routine body, a recovered file and
     non-ASCII identifiers, each equal to a fresh parse and accepted.
6. **Structural equality** (kept): `PartialEq` and `Debug` on `AlFile` and every IR type; checks
   assert `decoded == fresh`. A test fails on any `serde(skip` or `serde(default` under
   `crates/al-syntax/src/ir/`.

## Lowerer state: scratch, proven (a small product change ENG-110 makes)

(Kept; closed by sol r2 and r3.) `lower_file` clears `BUILD_CONTEXT` at its top, and
`with_build_context` truncates through a drop guard, so an unwind restores it. The other five
thread-locals are reset at file entry or before each read, or decide sharing only. Tests: unwind
restore (proof: remove the guard, red) and entry reset with a hand-seeded stale context on a clean
and a `Recovered` fixture (proof: remove the clear, red).

## Design

1. **ENG-113's `CacheDir`, refactored, not copied**, in `src/snapshot/cache.rs`, ENG-113's behaviour
   unchanged: `CacheDir` becomes `pub(crate)`; `write(name, replace, writer)` takes the entry's bytes
   from a writer closure (ENG-113's `write_entry` becomes one); `read_owned(name, cap)` opens as
   today and reads at most `cap` bytes; `subdir(name, create)` opens a child directory through the
   handle, checked private, and with `create` makes it first (`mkdirat`, 0700). **Only a writer
   creates** `ir/` and `ir/<key>/`. `lock(name)` gains a shared and a non-blocking form.
2. **Layout:** `<root>/ir/<IR_KEY, 64 hex>/<text digest, 64 hex>.bin`. `<root>` is ENG-113's cache
   directory. **`ALSEM_IR_CACHE_DIR` moves only the `ir/` tree**; the text cache stays where it is.
   It is for the acceptance's fresh cold roots and the cache's tests.
3. **Envelope:** header (magic `ALSEMIR\0`, format, full key, text digest, the diagnosis fields,
   file count, payload length); an index of `(offset, len, stored)` per file in `SourceRoot.files`
   order; the payload; a trailing blake3 over every byte before it. A hit requires, in order: the
   entry within 1 GiB; magic, format, key, digest; count equal to `SourceRoot.files.len()`; exact
   total length; the blake3; stored ranges that tile the payload exactly (a not-stored record has
   length 0; every sum checked). Then per stored file: the 64 MiB cap, a budgeted decode with no
   trailing bytes, `AlFile::check` against that file's own text.
4. **Owned, bounded reads**, not a map: the bytes decoded are the bytes hashed. Peak: one package's
   entry during its decode.
5. **The seam:** `src/program/dep_summary.rs`, `parse_for_build_observed` (line 62, the per-file call
   at 122) and `parse_dep_bodies` (line 175, the call at 195), each calling
   `snapshot::parse::parse_file` per file in a `par_iter`. One function serves both: per package,
   once, the owned read and envelope checks; per file inside the `par_iter`, decode and check in
   place of `parse_file`. On a fill, the loop parses from the owned snapshot and encodes; the
   package's buffers are published at its end, then freed.
6. **Small packages are skipped** below one fixed constant, measured once with its table.
7. **Switches are parameters in library code.** The library takes an `Option<&IrCache>` (root, key,
   verify flag, report sink), carried beside `BuildProfile` to the seam; `None` is today's behaviour.
   Only the binaries' `main`s (`alsem`, `aldump`, `al-call-hierarchy`) build it, from
   `ALSEM_IR_CACHE`, `ALSEM_IR_CACHE_DIR`, `ALSEM_IR_CACHE_VERIFY`, `ALSEM_IR_CACHE_REPORT` and the
   batch state (`claim::dir()`, a `OnceLock`). **Test binaries that build models in process**
   (`program_resolve_harness`, `lsp`, `l4_summary_differential`, the lib's tests) never construct
   one: they get `None` and parse as today, whatever the environment says. The cache's own tests
   and `lib_cdo`'s round trip pass an `IrCache` with a temporary root. The carrier struct between
   the `main`s and the seam is chosen in the plan (not verified which one).
8. **Disk:** 1 GiB per entry is a cap. The total under `ir/` has a 4 GiB **soft target**; cleanup is
   best effort (next item). If this process's own key folder alone passes 4 GiB it publishes nothing
   more and warns once.
9. **Cleanup, best effort** (sol r3 P2):
   - **`last-used`**: `<key folder>/last-used`, published by rename (never written in place) at a
     process's first hit or write, and again when older than a day.
   - **Throttle, checked periodically:** at a process's first cache use AND at each publish, if
     `<root>/ir/last-prune` is older than an hour, the process takes `<root>/ir/prune.lock` without
     blocking, re-reads `last-prune` under it, and only if still older publishes a new one and
     prunes. So a long-lived server that keeps publishing keeps the cleanup running.
   - **Key-folder coordination:** every publisher holds a SHARED `flock` on `<key folder>/folder.lock`
     from before it creates its temp file until its publish ends. A prune removes a key folder only
     while holding that lock EXCLUSIVE, taken without blocking, and re-reads `last-used` under it;
     if the lock is busy it skips the folder this round. A publish that starts after the prune took
     the lock waits; when the folder is gone it fails, and that process keeps its parsed IR. This
     replaces rev. 5's per-entry lock scan, which missed locks taken after enumeration.
   - **What it removes:** key folders whose `last-used` is older than 7 days; then, oldest first,
     key folders until under the soft target, never this process's own; temp files of this cache's
     name shape older than a day. Only names of these exact shapes, through the checked handle,
     never following a symlink.
   - **Overshoot, stated plainly:** the target can be exceeded by whatever is published between two
     prunes (at least an hour apart), and by key folders in use (skipped). It is a target, not a
     quota.
   - `alsem cache prune` maintains a different cache and is not changed.
10. **`parse_snapshot` stays uncached**: its callers outside `dep_summary` are test modules.

## `alsem batch` (TLS-109): off, tested through the real run

In a batch run (`claim::dir()` is `Some`: the parent and every worker) the `main` builds no
`IrCache`: no reads, no writes, no pruning, no report-file append. The claims do not reserve the
cache's names; batch already shares one model build across its jobs; no script runs `alsem batch`.
Multi-worker batch runs keep parsing per worker. Follow-up task (to file): turn it on with the claim
check reserving the cache folder's names, for the ENG-113 text cache too.

**Test, through the real run, with a positive control** (sol r3 P1-4, orchestrator): a `tests/cli`
test runs, with `ALSEM_IR_CACHE=on`, a fresh `ALSEM_IR_CACHE_DIR` and `ALSEM_IR_CACHE_REPORT` set,
over a fixture workspace with one embedded-source dependency above the small-package threshold:
- **control:** a plain `alsem analyze` on it publishes an entry under its root and writes a report
  line with `published_packages = 1` (so the environment really turns the cache on there);
- **batch:** `alsem batch` with two batches (the parent starts two workers), on a second fresh root
  and report path, leaves that root empty and writes no report file.

Proof: build the `IrCache` regardless of the batch state, red.

## Correctness checks

- **Decoded equals parsed:** every CDO dependency file decoded equals a fresh parse (`lib_cdo`, with
  its own `IrCache`).
- **Determinism:** the CDO set encoded twice in different thread orders gives the same bytes.
- **VERIFY** (`ALSEM_IR_CACHE_VERIFY=1`): every hit is also parsed and compared by structure; a
  mismatch fails the process loudly, naming package, file and key. **In the gate it runs in
  `output_cdo_impact`** (Fable r4 B1, orchestrator), whose `alsem impact` calls are real `alsem`
  processes that build the cache in their `main`. Rev. 5 put it in `program_resolve_harness`, an
  in-process test binary where no `IrCache` exists, so it could never fire. Never in a timed run:
  cdo-speed and the one-shot protocol refuse to start with it set.
- **Tests still exercise the parser:** `.cargo/config.toml` has no `[env]` section today; adding one
  with `ALSEM_IR_CACHE=off` is new work. It covers the `alsem` and `aldump` processes cargo-run tests
  spawn; an explicit value in the environment wins (cargo's documented `[env]` rule without
  `force`). **Whether nextest honours `[env]` is checked in step 1** (not verified); if it does not,
  `ci-steps` and `check-goldens` export the same value. In-process test binaries are unaffected
  either way (Design 7).
- **Counters** per process (Fable r4 P3, sol r3 P1-4):
  - `seam_files`: files that reached the seam, counted before any cache decision;
  - `eligible_files`: files of packages with a digest, a cache in use and above the small threshold;
  - `disk_hits`, `parsed_miss`, `refused`, `skipped_small`, `skipped_budget`, `skipped_off` (cache
    off, no digest, batch, no key), `published_packages`;
  - `inproc_reused_packages`: packages served from `DepCache`'s in-process tier (they never reach
    the seam), reported apart from disk hits;
  - invariant: `seam_files = disk_hits + parsed_miss + refused + skipped_small + skipped_budget +
    skipped_off`, asserted at exit in debug builds and checked by the acceptance;
  - with `ALSEM_IR_CACHE_REPORT=<file>`: one JSON line per process at exit (command, key, counters);
    also in the perf trace and census.
- **`ParseStatus::Recovered`** is replayed exactly; its doc is fixed to say the cache stores it.

## What still needs the source TEXT after an IR hit

(Kept.) `LineTable` positions; snippets in explain and digest; the row cache's `SourceText::ptr_eq`
fast path; `recovered_file_paths` and diagnostics that quote source.

## Many processes filling the cache at once

Each cold process parses and encodes. At a package's end it takes the key-folder lock shared and the
entry's lock, reads the entry again, and publishes only if none is there. Write amplification: one
entry per package. No lock spans a gate's start. Measured in Acceptance 6.

## Failure cases

- **Envelope refused:** the package is parsed (from an owned snapshot) and the entry is replaced
  under the lock. The next run hits.
- **A stored file's cap, budget, decode or check fails:** that file is parsed; the others keep their
  decoded IR. The replacement is the good files' verified slices plus the refused files' fresh
  encodings. If that replacement equals the stored bytes (the codec writes what it then refuses),
  nothing is rewritten: a warning names package, file and key, and `refused` counts it.
- **The owned snapshot's digest differs from the carried one:** nothing is published.
- **The cache is not writable or not private, or there is no key** (Windows, an unreadable or
  unmarked image): no IR cache, logged once.
- **Tests:** each refusal with a hand-written entry in a temporary root, asserting the model equals
  the cache-off model and that a second run hits the repaired entry (or, for the equal-bytes case,
  that nothing was rewritten). The snapshot rule: a test seam changes the mapped text between the
  map and the snapshot copy, and nothing is published (proof: publish under the carried digest
  without comparing, red).

## Acceptance

The judgment rules are fixed before any run. Every record carries the master and head SHAs, the
workspace SHA (`bc3ccb18`), the `.app` closure stamp (`dependency_closure_sha256`) and the keys; a
change in any voids the run.

**A first-ever run on a host needs three runs before a warm sample** (Fable r4 P3): the first fills
ENG-113's text cache, the second fills the IR cache, the third is warm. The untimed runs below
include them.

1. **Identity, byte for byte.** The deterministic output files of the CDO commands (fingerprint,
   analyze primary and `--scope all`, the call-graph histogram, and the explain, query and digest
   outputs) from **master**, **head-off**, **head-cold** and **head-warm**: all equal byte for byte.
   A file that differs between two runs of one arm is nondeterministic: listed, filed as a bug, and
   left out only of this comparison. Plus goldens (cache off), the refresh verify in both modes with
   a cache-off reference build, and `cdo-gate --only harness,l4,output`.
2. **Coverage, never vacuous**, per process, from the report files:
   - every warm sample: `eligible_files > 0`, `disk_hits > 0`, `disk_hits = eligible_files −
     skipped_budget`, `refused = 0`;
   - every cold sample: `eligible_files > 0`, `disk_hits = 0`, `published_packages > 0` and equal to
     the eligible packages, and the entries present on disk in that sample's root;
   - every off sample: `disk_hits = 0`, `published_packages = 0`, `skipped_off = seam_files`.

   A missing report, or a sample meant to use the cache with no eligible files, fails.
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
   Goal), and a `build (IR warm)` row is reported with hit counts per side; zero hits labels the
   verdict. A `--ir-cold` mode reports a build on a fresh root per side. `child_env` and
   `scripts/bench_ab.py` (line 39) keep `ALSEM_IR_CACHE` and `ALSEM_IR_CACHE_DIR` on purpose while
   stripping every other `ALSEM_*`, and refuse `ALSEM_IR_CACHE_VERIFY`.
5. **cdo-gate** exports `ALSEM_IR_CACHE=on` for every phase (the in-process binaries ignore it),
   gives each phase its own report file, prefills the exact `alsem` binary the `output_cdo` phases
   run with one untimed call before them, sets `ALSEM_IR_CACHE_VERIFY=1` for `output_cdo_impact`
   only, and **fails** a phase that expects hits (`output_cdo`, `output_cdo_impact`) when its report
   is missing or shows `disk_hits = 0`. This check fails from the start rather than starting
   advisory (mantra 4's default), by the orchestrator's decision: a missing report means the cache
   path was never exercised, the vacuous pass the rule exists to stop. The summary prints each
   phase's counters. A full gate's wall time is reported (no gain expected).
6. **Memory and disk:** the census, warm against master; peak RSS of one cold one-shot and of four on
   one fresh root, with temp bytes and write amplification; the key folder's size for CDO; the
   largest entry; the largest decode charge; the deepest expression chain; the small-package table;
   the blake3 cost of the image.

## Settled in review

1. **Key:** the running image, hashed in full every time, no sidecar; Linux only in v1.
2. **Fills:** parse and digest the same owned snapshot.
3. **Guarantee scope:** accidental damage; the user's private folder is trusted.
4. **Decode:** an enforced 256 MiB charge per file, an expression depth limit, one parent, the
   three-colour stmt/block walk; a refusal re-parses.
5. **VERIFY:** `output_cdo_impact`, for good; never timed. In-process test binaries get no cache.
6. **Gain:** the installed binary; a gate gets none.
7. **`alsem batch`:** off, tested through the real run with a positive control.
8. **Cleanup:** best effort, periodic, coordinated per key folder.

## Left for the owner

None. The disk numbers (1 GiB per entry, 4 GiB soft target), the 256 MiB decode limit and batch-off
are the orchestrator's to confirm.

## Future (ideas only; ENG-110 does not build them)

- **Windows:** read the running image natively and prove the handle is the loaded file; then turn
  the cache on there.
- **Batch:** the claim check reserving the cache folders (the follow-up task above).
- **A narrower key** (orchestrator). A later task could key the cache on a build-time hash of exactly
  what produces the cached IR: the `al-syntax` sources, the grammar's compiled inputs and the IR
  codec's schema, computed in a build script. Then a change that touches only the engine would still
  hit, and a gate could gain. It is admissible only if a review proves that nothing outside that set
  reaches the cached bytes. That review must close the holes revs. 3 and 4 already found:
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
  exact bytes its name digests; consumers never see IR a fresh parse could not produce.
- **3 (root cause):** the `BUILD_CONTEXT` leak is fixed in the lowerer.
- **4 and 9 (measure; commit first):** the one-shot rule is written before any run; coverage cannot
  pass on an empty or all-skipped count; limits are set from measured values. The cdo-gate hit check
  fails from the start, an orchestrator decision against 4's advisory default, stated where it
  applies.
- **6 (say what you don't know):** the gate's lack of gain is said plainly.
- **7 and 8 (tests that fail; real use):** every guard has a stated break; batch-off is tested through
  the real parent and workers, with a positive control.
- **10 (reproducible baselines):** each record pins the workspace, the `.app` closure and the keys.
- **13 and 14 (right-sized; reuse):** ENG-113's `CacheDir` is refactored, not copied; no sidecar.
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
  reset, owned reads, batch off, the one-shot protocol, digest in the path, disk caps.
- **rev. 5:** sol r2 and Fable r3 folded in, with the orchestrator's binary key: decode budgets, the
  stmt/block walk, the text re-hash, the batch real-run test, the paired acceptance rule.
- **rev. 6:** sol r3 (4 P1, 1 P2) and Fable r4 (B1 and its P2/P3s) folded in, with the orchestrator's
  decisions: no sidecar, the image hashed lazily each time, Linux only, static-link and tool-binary
  guards (sol P1-1, Fable P2-2); fills parse an owned snapshot (sol P1-2); the guarantee's scope, an
  enforced 256 MiB decode charge, an expression depth limit and the one-parent rule (sol P1-3,
  Fable P2-3); VERIFY moved to `output_cdo_impact`, in-process binaries get no cache, missing reports
  fail (Fable B1, sol P1-4); the honest gain, a gate gets none (Fable P2-1); eligible-file coverage,
  `skipped_off`, `skipped_budget`, the three-run warm-up, the `[env]` and nextest work, the batch
  positive control (sol P1-4, Fable P3); best-effort periodic cleanup with key-folder coordination
  (sol P2); the `PreprocGroup` hedge removed; the narrower-key idea recorded under Future.

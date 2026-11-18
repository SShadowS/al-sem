# ENG-110: persistent cache of parsed dependency IR (design, rev. 2, 2026-11-18)

Status: a spec for the two-model panel. No product code. Implementation waits for ENG-113, which
builds the package-text cache this one stands on. Rev. 2 folds in Fable's review r1 (3 P1, 6 P2)
and the orchestrator's three additions. The measurements come from a throwaway prototype that
never lands: branch `tools/eng-110-proto`, with serde derives on `al-syntax`'s IR and
`examples/eng110_probe.rs`.

## Goal and bar

Every engine process parses and lowers every dependency file again: about 24 CPU-s per process on
CDO, and every gate runs several processes. A cache hit should replace that with a read and a
decode.

**Bar:**
- (a) **Warm cache, user-mode instructions** (TLS-83's counting), confirmed by cdo-speed twice:
  - one-shot `alsem analyze` on CDO: at least 30 % fewer instructions;
  - the resident engine's model build (cdo-speed's `build` call): reported;
  - a full `cdo-gate`'s wall time: reported.
- (b) **Byte-identical output**, cold AND warm: goldens, `cdo-before-after`, fingerprint bytes,
  refresh verify in both modes, `cdo-gate --only harness,l4,output`. The warm run must report hits
  above zero (P2-6): a silent fall back to parsing is identical by definition.
- (c) **A cold miss** costs at most +5 % instructions over today. Peak memory on a cold miss is
  reported.

## Measured ceiling (server1, CDO baseline `bc3ccb18`, master `a73b4bf6`)

- **One-shot `alsem analyze`** spends 25.4-25.5 CPU-s of 57 in `al_syntax::parse` (parse plus
  lower): 44.7-45.0 % in two samply profiles. With `--single-app` it is 25.4 CPU-s of 30.4:
  every process parses every package.
- **Dependencies** are 96 % of the source bytes: 10,881 of 11,463 files, 115.5 of 120.4 MiB. So
  dependency parsing is ≈24 CPU-s a process.
- **In instructions:** the parser and lowerer symbols' self shares in ENG-112's `perf record` are
  ≈32 % of P-core and ≈53 % of E-core user instructions, ≈39 % combined. This is a lower bound: it
  leaves out the allocator under the parse, and 12,464 samples were lost.
- **The prototype**, single thread, all dependency files, in `/work/.al-sem/profiles/2026-11-18-eng110`
  (`probe.log`, `probe2.log`, `probe3.log`; a fourth run's log was overwritten):
  - parse+lower: 11.4-12.9 CPU-s;
  - encode: 0.27-0.29 CPU-s;
  - decode: 0.32-0.37 CPU-s, 2.6-3.2 % of parse+lower;
  - size: 143.9 MiB postcard (78.5 MiB deflated).

  The engine's parse is ~2× the single-thread figure: its pool runs on all 22 threads (6 P-cores
  with two threads each, plus 10 E-cores). Decode inflates the same way, to about 0.7 CPU-s.
- **The ceiling.** A hit saves about 23 CPU-s of 57 (~40 %) in a one-shot analyze. The survey's
  "400-600 CPU-s per gate" is not re-measured; bar (a) reports the gate.
- **Heap.** The IR held after a decode is 476.5 MiB in 4.66 M allocations, against 498.3 MiB in
  4.31 M after a parse, from a counting allocator in the probe. Shared `Arc<str>` type texts
  decode separately (+8 % allocations), but the total is lower, so there is no memory risk. The
  engine census confirms it per process (Measures). Exact-size arenas after a decode are not
  guaranteed (serde grows them by doubling past ~1 MiB); `exact_size` runs on them after decode,
  as after a parse.
- **Purity, measured (Fable P2-4).** The lowerer keeps thread-locals (`BUILD_CONTEXT`,
  `VARS_INCOMPLETE`, `CONDITIONAL_VARS`, `PREPROC_RANGES`, `PREPROC_WALK`). All 10,881 files
  parsed in parallel on the engine's own big-stack pool, in REVERSE order, encode identically to
  the sequential parse: 0 differ (`probe3.log`).

## The key: what the IR depends on

`al_syntax::parse(source)` is a pure function of the text, the grammar compiled in, the lowerer's
code and the Rust toolchain. `char::to_lowercase`'s tables feed `another_declaration_lacks`. No
preprocessor symbols reach it: `#if` arms are kept in the IR. An entry is valid only for one text
and one `IR_KEY`.

- **The text: bound to ENG-113's entry, not re-derived (Fable P2-1).** The IR file's header stores
  the blake3 that ENG-113's text-cache header holds, which covers its index and its text. A hit
  requires that exact value, so the IR is only served over the text it was made from. The
  alternative was trusting the image blake3 plus a hand-bumped SCHEME.
  - An `Owned` text (ENG-113's fallback when the cache is not writable) has no entry, and so no
    IR cache.
  - Workspace files and source-folder dependencies are never cached.
- **`IR_KEY`**, a sha256 computed at build time, no hand-bumped constant (Fable P1-1, P2-5):
  - **Grammar part:** `al-syntax`'s build script, which already compiles the grammar, hashes every
    file under the grammar's `src/` (`parser.c`, `scanner.c`, `unicode_id.h`, `tree_sitter/*.h`,
    `node-types.json`). Files are taken in sorted relative-path order, with each path and length
    hashed. It publishes the hash through `links = "tree-sitter-al"` /
    `cargo:grammar_hash=...`. Its `rerun-if-changed` set stays as it is: the grammar folder.
    `parser.c` (36.9 MB) does not recompile on a lowerer edit.
  - **Lowerer part:** the root `build.rs` hashes every file under `crates/al-syntax/src/`, in
    sorted relative-path order with paths and lengths, with `rerun-if-changed` on that folder. It
    adds all of `Cargo.lock`, `rustc -V`, the grammar hash (`DEP_TREE_SITTER_AL_GRAMMAR_HASH`) and
    a codec format number. It exports the result as `al_sem`'s `IR_KEY` constant.
    `cargo:rustc-env` reaches only the crate that sets it, so the key lives where it is used.
  - **A test** changes a byte under a copy of `crates/al-syntax/src/` and sees the key move, with
    a discrimination proof (break: drop the folder from the hash, so the key does not move, red).
  - **Not `node-types.sha256` alone:** a grammar change can alter how the same text parses with no
    new node type (v4.0.0 moved dangling `else` and operator precedence). Not a constant:
    `CACHE_VERSION_GRAMMAR` kept a stale commit once. A false miss costs a parse; a false hit is a
    wrong answer. The key over-approximates on purpose.

## Design

1. **Built on ENG-113** (`docs/superpowers/notes/2026-11-18-eng113-mapped-dependency-text.md`):
   - the same cache root;
   - the same write-once rule (`tempfile::NamedTempFile::new_in` + `persist`, never truncated in
     place, temp names with a known prefix);
   - its `map_read_only`;
   - a mandatory blake3 of the payload;
   - its refuse-and-rebuild rule.

   No second scheme.
2. **Layout (Fable P1-3):** `<cache>/ir/<IR_KEY:16 hex>/<package blake3>.bin`. One folder per key,
   so binaries with different keys (a lane with a lowerer edit, cdo-speed's base and head, the
   installed `alsem`) never touch each other's entries.
   - **Header:** magic; format; the FULL `IR_KEY`; the text entry's blake3; the file count; the
     payload length; the payload's blake3.
   - **Index:** `(offset, len)` per file, in the text entry's index order. That order is the same
     as the text's, bound by the text blake3.
   - **Payload:** every file's postcard bytes, concatenated. Each file must decode with no
     trailing bytes (`take_from_bytes` with an empty rest).
3. **The seam (Fable P2-2).** Dependency packages are parsed in `src/program/dep_summary.rs`:
   `parse_for_build_observed` (line 122) and `parse_dep_bodies` (line 195), each calling
   `snapshot::parse::parse_file` per file inside a `par_iter`. One function serves both:
   - it checks the entry ONCE per package (header, key, text blake3, payload blake3, index ranges);
   - it then decodes file `i` from the map inside the existing `par_iter`, in place of
     `parse_file`.

   So a profile that summarizes and drops each tree keeps today's bound of about one tree per
   thread (`LiveTrees`). A whole package is never decoded up front. On a miss, the same loop
   parses and encodes each file. The encoded buffers (≈144 MiB for CDO) are collected and written
   once at the end, and the cold-miss peak is a measure. The IR map is dropped when the package's
   decode ends: the IR is owned heap, so the SIGBUS and Windows rename windows last only for the
   decode, unlike ENG-113's text map.
4. **Small packages are skipped (Fable P2-3).** The cache holds 83,236 files (691 MiB) today,
   mostly test fixtures. Below a measured text size, a parse beats a file read plus a decode, and
   no IR file is written. The threshold is measured on the prototype: the per-package decode-plus-
   read cost against the parse cost, by size; the expected order is ~256 KiB. A package with zero
   files writes nothing either.
5. **The codec:** serde derives on the IR types, always on (Fable P3: a feature would be a second
   build shape nobody tests). `ActionDecl.kind` (`&'static str`) gets a hand-written pair: write
   the keyword, read it back into its static set, and refuse an unknown one (a miss). The
   prototype's `#[serde(skip)]` there is NOT the design.
6. **Pruning (Fable P1-3):** whole key folders unused for 7 days are removed, by the folder's mtime
   (a hit touches it), as `scripts/sweep-builds` does for build output. So are temp files with the
   known prefix older than a day; a killed writer leaves up to ~144 MiB. Nothing is ever pruned for
   being "not my key".

## Correctness checks: the key is the whole story

- **Decoded equals parsed, by STRUCTURE (Fable P1-2).** Byte equality of re-encodings cannot see a
  skipped or defaulted field: the prototype skipped `ActionDecl.kind`, and its round trip still
  read "identical". So `PartialEq` and `Debug` are derived on `AlFile` and every IR type that lacks
  them (`Origin`'s total `PartialEq` already exists). The checks assert `decoded == fresh`.
  - A test decodes every CDO dependency file and compares it with a fresh parse. It runs in the CDO
    subset (`lib_cdo`).
  - A test reads `crates/al-syntax/src/ir/` and fails on any `serde(skip` or `serde(default`.
- **VERIFY mode, in production shape (orchestrator).** With `ALSEM_IR_CACHE_VERIFY=1`, every hit is
  parsed anyway and compared by structure with the decoded IR. A mismatch fails the process loudly,
  naming the package, the file and the key. It is never a silent fallback. A hole in `IR_KEY` then
  shows up as a gate failure. `cdo-gate` sets it for one phase, `program_resolve_harness`, which
  builds the cross-app model over every package. That phase runs with a warm cache: the gate's
  first phase fills it.
- **Tests still exercise the parser (orchestrator):**
  - under `cargo test`, `ci-steps` and `check-goldens` the cache is OFF unless a test turns it on
    (`ALSEM_IR_CACHE=off` set by the test harnesses, like `CDO_WS` handling); so every golden and
    test parses as today;
  - the cache's own tests use a temporary cache directory through a `dir` seam, like ENG-113's
    `cached_source_in`;
  - the identity bar (b) runs the CDO set twice, cold (empty `XDG_CACHE_HOME`) and warm.
- **Hit and miss counters** go in the perf trace and the census, so bar (b)'s warm run proves it
  hit (Fable P2-6).
- **`ParseStatus::Recovered`:** its doc says "do not cache as authoritative". The IR cache replays
  it exactly, flag included, so consumers see what a parse would give. The doc is fixed to say so.

## What still needs the source TEXT after an IR hit

The IR stores names and literal text inline, plus byte ranges (`Origin`), but not the source.
These read the text, so ENG-113's mapped text stays the one copy and the IR cache holds none:
- line and column positions (`LineTable`): LSP results, explain, query and digest locations, SARIF;
- source snippets in explain and digest output;
- the row cache's identity fast path (`ptr_eq` on ENG-113's `SourceText`);
- `recovered_file_paths`, and diagnostics that quote source.

## Many processes filling the cache at once

On a cold cache each gate process misses, parses (today's cost), encodes and writes its own unique
temp file, then renames it into place.
- The encoding is deterministic: asserted by a test that encodes the CDO set twice, in a different
  thread order. So the last rename wins with identical bytes, and no reader sees a partial file.
- The cost is the duplicated first fill. No lock: one would serialize the gate's start for a
  one-off saving.
- On Windows a rename over a mapped file fails: the writer tries the hit path once, else keeps its
  parsed IR. The IR map is short-lived (Design 3), so this is rare.

## Failure cases

- **Any check fails** (header, key, text blake3, payload blake3, index, trailing bytes, decode
  error, unknown action keyword): refuse the entry, parse, rewrite. Each case is tested with a
  hand-written broken file in a temporary cache directory.
- **A file truncated or rewritten in place during a decode:** SIGBUS, as ENG-113 states. The window
  is one package's decode.
- **The cache is not writable:** no IR cache, logged once at warn.

## Measures (before and after, on current master after ENG-113)

- **Identity:** bar (b), cold and warm, with hits counted.
- **Instructions and CPU:** one-shot analyze, cold and warm; the resident model build
  (cdo-speed's `build` call, cold and warm cache); a full `cdo-gate`'s wall time. cdo-speed twice
  before a ratio is quoted.
- **Heap:** the engine census, warm against today (the IR bucket); the cold-miss peak RSS.
- **Disk:** the key folder's size for CDO; the small-package threshold's table.

## Open questions for the panel

1. Is the `IR_KEY` input set now complete? Anything else compiled into the parse: a target
   feature, an opt level that changes float formatting (none in the IR), the big-stack size?
2. Should VERIFY run in every gate, or only until the first ten landings pass, and then stay as an
   opt-in?
3. The small-package threshold: a fixed size, or measured per host at first use?

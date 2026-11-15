# Dependency pack cache — design

Status: DESIGN, pre-implementation.
Supersedes the follow-up sketch in `2026-08-01-preflight-verdict-cache.md` §6.
Sizing evidence: `docs/2026-08-02-dep-parse-sizing.md`.
Span census: `docs/2026-07-31-preflight-census.md`.

Reviewed adversarially in two rounds by `claude-fable-5`. Every codebase claim below was
verified against source. Two review claims failed verification and the corrections are kept
in place rather than quietly dropped — see §7 (the `CompilationContext` rationale) and §13
(the span attribution behind the format choice).

---

## §1 — The problem

`alsem analyze` fully parses and lowers every dependency source file on every run. On DO
that is 11,856 files producing 11,165 objects and 126,640 routines, costing
`preflight.parse_snapshot` 1,139.2 ms plus `preflight.dep_layer` 141.9 ms ≈ **1,280 ms**.

**Only declarations are ever consumed from dependencies.** Verified at three sites:

- `resolve_full_program_from_parts` Phase 1 filters to the primary app AND the workspace
  file set (`src/program/resolve/full.rs:805-812`)
- Phase 2 `emit_event_flow_edges` walks `graph.routines` — declarations
- `DeclSurface::build` reads only `obj.routines` → `RoutineMeta::from_decl`
  (`src/program/resolve/decl_surface.rs:75-98`)

`DeclSurface`'s own doc states the intent: `RoutineMeta` holds "EXACTLY the fields
resolution reads (audited): never the routine body — dropping the dep parse arenas is the
whole point" (`decl_surface.rs:10-11`).

So the declaration-native representation already exists. The residual cost is that
*producing* it routes through a full parse, every run.

## §2 — Workload premise

**One-off analyses of many different workspaces**, not an edit loop and not CI reruns of one
repo.

The already-shipped preflight verdict cache does not help this workload: its key covers the
workspace's own app identity and source content, so a different workspace always misses.

What repeats is the DEPENDENCY set. Base Application and System Application are static
artifacts per (BC version, localization) pair. Many customer apps target the same base
version; a user works within few localizations; Microsoft ships a bounded number of versions.

**Therefore the pack store is a small library of Microsoft base versions**, with cardinality
≈ (versions × localizations touched). This premise is the human's domain knowledge, adopted
deliberately over the review's "cross-workspace hit rate is unmeasured" objection. It is
recorded here as a premise so that a later observation of low hit rate is recognised as
falsifying it, rather than as a surprise.

## §3 — Scope

**In:** a per-dependency-app, content-keyed, fail-closed on-disk artifact ("pack") holding
the extracted declaration products, wired in as a third ingestion route.

**Out:** persisting any classified edge (unsound, §4); packs for symbol-only apps (the ABI
route is a slice of `dep_layer`'s 141.9 ms, not of the 1,139 ms parse — de-scoped in the
prior spec and still de-scoped); pack pruning (§10); the resident daemon (§11).

## §4 — The purity boundary

Dependency-derived PARSE/DECLARATION products ARE a pure function of dependency content:
`extract_nodes(app_ref, &pf.file, tier, …)` reads only the parsed file and its tier
(`src/program/build.rs:89-97`).

Dependency-derived EDGES are NOT. `ResolveIndex::build` populates `subscribers_map` by
iterating ALL of `graph.routines` with no app scoping
(`src/program/resolve/index.rs:285-300`), and `emit_event_flow_edges` emits one route per
subscriber (`resolver.rs:3006-3045`). A primary-app subscriber attaches to a dependency
publisher, so dep-internal edges keyed on dep bytes alone would be unsound.

**Packs persist nodes and `RoutineMeta`. Edges are always recomputed.**

## §5 — Architecture: the pack IS the ingestion route

Not a cache bolted beside `build_dep_layer`, but a third route inside it, symmetric with the
two that exist:

| route | population | today |
|---|---|---|
| source-parse + extract | source-bearing dep apps | Step 2 |
| `ingest_abi` | symbol-only dep apps | Step 2b |
| **pack-load** | **`EmbeddedSource`-tier dep apps with a pack hit** | **new** |

Per app: hit → nodes and `RoutineMeta` from the pack, source never read; miss → parse and
extract as today, then store a pack.

This seam is also where the daemon later plugs in: `lsp::updater`'s `Arc<DepLayer>` reuse and
a pack load become the same abstraction at different lifetimes.

**Do not** split dependency and workspace routines into different types. `ResolveIndex::build`
(`index.rs:285-300`), `emit_event_flow_edges` (`resolver.rs:3006-3045`) and
`inject_platform_event_publishers` all iterate one unified `graph.routines`; the `tier` field
already carries the distinction.

## §6 — Pack contents

Per dependency app:

- format version, key echo, self-hash — mirroring `preflight_cache::Entry`'s integrity
  discipline (`preflight_cache.rs:276-289`)
- app identity **symbolically** (guid / name / publisher / version), re-interned on load.
  `AppRef` is a per-run interning index (`build.rs:76-77`); persisting one yields
  silently-wrong graphs, not errors.
- per source file, in extraction order: virtual path, `ParseStatus`, and that file's
  extracted `ObjectNode` / `RoutineNode` contributions
- the `DeclSurface` contribution — `RoutineMeta` per routine
- recovered-file paths

Stored **pre-dedup**, per-file order preserved, so the existing Step 4 sort and
`dedup_routines_preserving_genuine_overloads` run identically over the merged population.

## §7 — The pack key

blake3, every field length-prefixed (without prefixing, `("ab","c")` and `("a","bc")` fold
identically):

domain tag ‖ pack format version ‖ `EXTRACTION_FINGERPRINT` (§8) ‖ the app's `.app` content
hash ‖ the app's `TrustTier` ‖ the app's `CompilationContext`.

`preflight_cache.rs:20-26` states the governing rule: there is no payload-level defence, a
well-formed entry under a correct key is indistinguishable from a correct one without
recomputing, so **all soundness lives in the key**.

Component notes:

- **`TrustTier` is a real input.** `extract_nodes` takes it (`build.rs:89-97`), it is stamped
  into every node, and it drives dedup behaviour.
- **`CompilationContext` is aspirational, and the spec says so.** `al_syntax::parse` takes
  only text (`crates/al-syntax/src/parse.rs:10-19`), the lowerer union-reads every `#if`
  branch, and a dependency's `preproc_symbols` is empty by construction —
  `context_from_metadata` hardcodes `BTreeSet::new()` because the manifest does not record
  them (`src/snapshot/compilation.rs:29-37`). Included because `runtime`/`platform`/
  `application` ARE populated and a future evaluating lowerer would make symbols live. It is
  a constant today; do not cite it as a live input.

**Packs are restricted to `TrustTier::EmbeddedSource` units.** Non-primary source-bearing
apps also include multi-app workspace siblings backed by *walked* source, which have no
`.app` hash — and the snapshot layer's `content_hash` for walked source is explicitly
unusable as identity: it folds only concatenated file text with no path and no length prefix,
colliding on rename and on re-splitting the same bytes across file boundaries
(`preflight_cache.rs:162-168`, which refuses to reuse it). Siblings churn constantly and are
not the base-version library this design exists for. Leaving "source-bearing" unqualified
would be a soundness hole.

## §8 — `EXTRACTION_FINGERPRINT`

### Why not the engine binary hash

The preflight cache keys on the whole binary, which "cannot rot in the first place — there is
nothing to bump" (`preflight_cache.rs:110-125`). But it invalidates every pack on every
`cargo build`, so during active engine development the cache never hits and the base-version
library premise (§2) never holds.

### Why not a hand-curated file list

Because that is a hand-bumped constant with extra steps, and this repo has a documented
corpse: `CACHE_VERSION_GRAMMAR` read `"tree-sitter-al-v2.5.2-native"` while the pinned
grammar had already moved to v3.2.0 — unbumped across two upgrades, nothing failed, until a
dedicated test was added (`preflight_cache.rs:117-125`).

The first draft of this design listed `crates/al-syntax/src/`, `node_extract.rs`, `node.rs`
and the grammar. Tracing `node_extract.rs:3-14`'s actual imports shows that closure **missing
five files**: `src/program/sig_fp.rs` (computes the `sig_fp` in every persisted
`RoutineNodeId`), `src/program/resolve/event.rs` (subscriber/publisher attribute parsing,
serialized into `RoutineNode.event_subscribers`), `src/program/resolve/edge.rs`
(`AbiEventKind`/`AbiRoutineKind`, payload fields), `src/program/resolve/receiver.rs`
(`unquote_identifier`), and `src/program/resolve/decl_surface.rs` (`RoutineMeta::from_decl`).
The next `use` added silently exits any such list.

### The decision: coarse closure plus a behavioural canary

**Closure** — hash the contents of:

- `crates/` (all of it, including the al-syntax lowerer, IR and raw vocabulary)
- `src/program/`
- `src/snapshot/` (covers `embedded.rs` / `cache.rs`, which determine which entries a `.app`
  yields and how virtual paths are named — the `.app` blake3 alone does not fix that)
- `src/engine/deps/` — **an exception to the `src/engine/` exclusion below. Conservative,
  not currently load-bearing — and the original justification for it was wrong.**

  **Corrected.** The amendment that added this directory claimed `symbol_reference.rs`
  defines two types "serialized INTO packs (`SubtypeTag`, `AbiEventKind`)". Both halves are
  false against the merged code, and the falsification was found by an independent review:
  - `symbol_reference::AbiEventKind` has no `Serialize` (`symbol_reference.rs:24`). The one
    packed is `resolve::edge::AbiEventKind` (`node_extract.rs:9`), already inside
    `src/program/`.
  - No `EmbeddedSource` pack can carry a `SubtypeTag` at all. It reaches a node only via
    `AbiParams::Complete`, whose sole production construction is `abi_ingest.rs:102`;
    `extract_nodes` always writes `AbiParams::Missing` (`node_extract.rs:681`). ABI ingestion
    is `SymbolOnly` tier, and §7 restricts packs to `EmbeddedSource`.

  So the stale-pack hole the amendment claimed to close **does not exist today**. Keep the
  directory anyway: inclusion costs over-invalidation only (one reparse per base version),
  and the invariant protecting it — "packs are `EmbeddedSource`-only, therefore `AbiParams`
  is always `Missing`" — is implicit and thin. `build.rs:1182` already notes an
  `AbiParams::Complete`-only guard "is only as sound as this invariant". If packs are ever
  extended to symbol-only apps, `SubtypeTag` becomes live payload and this directory becomes
  mandatory rather than merely prudent.

  **The lesson stands even though the example was wrong:** derive this closure from the
  reachable type set, not from directory names. The original error was reasoning about
  DIRECTORIES; the correction's error was asserting reachability without tracing it.
- the tree-sitter-al grammar's **source files** (`grammar.js`, generated `parser.c`), never a
  version string
- the tree-sitter **runtime** crate's lockfile entry — error recovery lives in the core
  library, not the grammar, so a runtime upgrade can flip `ParseStatus::Recovered`/`Clean`,
  which changes persisted `parse_incomplete` and recovered-file paths
- the pack serialization module itself

This deliberately excludes `src/engine/` **apart from `src/engine/deps/` above**, plus
`src/lsp/` and `src/bin/`, where most commits land.

**Derive this closure from the reachable type set, not from directory names.** The
mandatory check when adding any field to a packed type: locate where its type is DEFINED,
and confirm that path is inside the closure. `SubtypeTag` sits two directories away from
every other packed type and was missed exactly once by a directory-shaped reading.
Over-invalidation costs one ~1,280 ms reparse per base version; a stale pack costs
correctness.

**Canary** — a golden test that runs `extract_nodes` + `RoutineMeta::from_decl` over a pinned
fixture corpus and byte-compares the serialized pack against a committed artifact. This
guards the *behaviour*, not the file list, so a change anywhere in the true closure fails
loudly and forces a fingerprint review. Both mechanisms ship; neither alone is sufficient.

## §9 — Storage discipline

One file per key under `<os-cache>/alsem/dep-pack-v1/` — **a default, not a law.** §13's
falsified-parallelism note explains why: with one app holding 84 % of the population, the
per-app granularity this line fixes IS the critical path, and sharding that app across several
independently-hashed files is the highest-leverage change available. Treat "one file per key"
as the shape to start from and to depart from deliberately, not as a constraint.

Mirroring `preflight_cache.rs`
throughout: atomic tmp+rename, every abnormal state (missing, unreadable, schema mismatch,
key mismatch, fingerprint mismatch, self-hash mismatch) is a silent miss logged at debug, a
write failure never breaks a run, environment variables override the directory and disable
the cache entirely.

**Only fully-extracted results are stored.** A file whose parse was `Recovered` IS
deterministic content and is safe to cache; a transiently failed `.app` extraction is not.
This mirrors preflight rule 3 — `Ok` cached, `Err` never (`preflight_cache.rs:29-38`).

**Inherited trust caveat, stated not hidden:** a pack built from a stale
`al-ch-snapshot-cache` entry is poisoned even under a perfect key. That cache is the one
`preflight_cache.rs:66-70` calls out as having no override and no pruning. Packs inherit its
soundness; hardening it is out of scope here and noted as a live risk (§13).

## §10 — Light snapshot (prerequisite, not adjacent)

For dependency units, compute the pack key **without materializing source text** — the `.app`
content hash is already available via `snapshot::embedded::app_content_hash`. On a hit that
app's source is never read.

Three reasons this is a prerequisite rather than a nice-to-have:

1. **Without it the saving is not realized.** `preflight.snapshot_build` is 458.8 ms on DO,
   most of it loading 11,856 dependency source texts a hit never parses.
2. **It structurally enforces the declarations-only invariant.** "Only declarations are
   consumed" is true today and unenforced. On a hit the dependency's `ParsedFile`s simply do
   not exist (`parse_snapshot` skips units whose `source` is `None`,
   `src/snapshot/parse.rs:75-95`), so a future body-reading feature fails loudly instead of
   silently reading stale data.
3. **It has standalone value.** It cuts `snapshot_build` for the already-shipped verdict
   cache even if packs are later abandoned.

**Required before it ships:** a test asserting the verdict cache's `cache_key` is byte-equal
between a full and a light snapshot of the same workspace. `source_identity` takes the
`EmbeddedSource` branch today (`preflight_cache.rs:177`); a light unit with `source: None`
falls to the `app_path` branch (`preflight_cache.rs:186-192`). Both yield the `.app` blake3,
but any structural difference in the canonical fold would silently zero the shipped cache's
hit rate.

## §11 — Two consumers that must be rewired

These are the design's real failure points — the data is listed above, the wiring is where it
breaks.

1. **`DeclSurface` frozen-tier injection.** `DeclSurface::build` independently iterates
   parsed units (`decl_surface.rs:75-98`); on a pack hit those units do not exist. The pack
   carries `RoutineMeta`, but something must inject it into the frozen tier
   (`decl_surface.rs:66-68`). Miss this and every dependency routine metadata lookup degrades
   to a decline — a silent quality regression, not an error.
2. **`recovered_file_paths`.** It iterates parsed units (`src/snapshot/parse.rs:61-76`) and
   is the load-bearing absence-proof diagnostic: its doc states "**Any** current or future
   absence/`ProvenAbsent`-shaped claim in this engine MUST consult this diagnostic … before
   treating a file's content as complete" (`parse.rs:47-60`). On a hit, the pack's recovered
   paths must merge into that function's output or a warm run under-reports recovery.

Every other consumer of `parsed` for dependency units must be audited the same way.

### §11.3 — THE SENTINEL. Audit `unit.source`, not just `parsed` — this clause was scoped wrong

**The audit instruction above is scoped to miss the thing most likely to break.** Repo-wide,
"this dependency is symbol-only" is detected as `AppUnit.source.is_none()`
(`src/snapshot/snapshot.rs:37`). Under §10's light snapshot a pack-hit `EmbeddedSource` app
ALSO has `source: None`, so that equation silently stops holding at three dispatch sites —
none of which consume `parsed`, so none of which the clause above would have caught:

- **`src/program/build.rs:103-105`** — Step 2b skips units with `source.is_some()`, so every
  source-less unit is ABI-ingested. A pack-hit app would be fed to `ingest_abi`, producing
  SymbolOnly-tier nodes (no `RoutineMeta`, `AbiParams` from `SymbolReference.json`) alongside
  or instead of the pack's `EmbeddedSource` nodes. **Wrong tier, silently.**
- **`src/program/resolve/full.rs:1253`** — `u.source.is_none() && has_abi_objects` marks an
  opaque dependency boundary. A source-backed dep with a pack hit would be reported opaque.
- **`src/program/resolve/abi_check.rs:312-313`** — treats every source-less unit as a dep
  boundary and ingests its `SymbolReference.json` into `RawAbiIndex`.

**Fix: give `AppUnit` a tri-state** — source-bearing / symbol-only / embedded-source-not-
materialized — rather than overloading `Option`. Cheap now; otherwise it surfaces as a
mystery differential on the first light-snapshot run.

**The miss path compounds it.** Under a light snapshot even a pack MISS begins with
`source: None` and must materialize that one app's source LATE. `parse_snapshot` has no such
API — it drops `None` units outright (`src/snapshot/parse.rs:90-91`).

**Not the risk, for the record:** the two rewires §11.1/§11.2 name are both straightforward.
`PackedFile::routine_meta` is exactly the `(RoutineNodeId, RoutineMeta)` pairs
`DeclSurface::with_frozen` absorbs (`decl_surface.rs:179-182`), with last-write-wins order
reproducible from pack order per §12; and `recovered_file_paths` is a signature change on a
15-line function (`parse.rs:61-74`).

## §12 — Ordering and the equivalence gate

`dedup_routines_preserving_genuine_overloads` keeps the first occurrence per key — arbitrary
but stable, where stability comes from one deterministic extraction order. If packs are
loaded per app and concatenated in a different order than `parsed` iteration order,
first-occurrence survivors can differ between cold and warm runs.

**Pin concatenation to `snap.apps` order; preserve per-file extraction order inside each
pack.**

**The merge gate compares final analysis output (edges and findings), not just
`ProgramGraph` bytes.** A `ProgramGraph`-only comparison would pass while §11's `RoutineMeta`
degradation silently changed resolution results.

## §13 — Format and the measurement gate

**Format:** compact binary, record-per-routine (postcard-shaped), pending the gate below. The
shared-string-table design stays in reserve.

**The evidence for this choice, stated honestly.** The review first argued the format is safe
because `assemble_program_graph` already clones the whole population per run
(`build.rs:179-180`) inside a measured span. That span attribution was wrong — the clone sits
in `preflight.assemble_graph` (116.4 ms), not `preflight.dep_layer` (141.9 ms); the two are
disjoint (`full.rs:1119-1122` and `:1136-1139`). Corrected, the bound is tighter, but the
analogy still does not hold: a decode additionally does file I/O, varint parsing, UTF-8
validation on every string, enum discriminant decoding and the guid→`AppRef` re-intern, none
of which a clone pays. A serial decode plausibly lands at 100–250 ms — straddling the gate.

What was expected to carry the recommendation is **parallelism** — packs decoded per app,
concurrently.

**That wiring does not exist and step 5 must build it.** `build_dep_layer`'s per-app loop is
a plain serial `for unit in parsed` installing no pool (`src/program/build.rs:83-98`), and
`parse_snapshot`'s pool parallelism is per-FILE within one app
(`src/snapshot/parse.rs:86-95`) — there is no per-app parallel structure to inherit. The gate
bench had to build its own `par_iter` over pack paths
(`benches/dep_pack_roundtrip.rs:437-441`); the seam will have to do the same.

> **FALSIFIED BY THE MEASUREMENT — do not build on this paragraph.** The gate showed
> parallelism buys almost nothing here. Base Application is 100,944 of 119,773 routines in ONE
> serial decode unit and accounted for 89–99 % of every logged round's wall time; the ledger
> concludes outright that "more cores will not help … the critical path is effectively serial"
> (`docs/2026-08-07-dep-pack-gate-measurement.md:239-250`). The serial-decode ESTIMATE above
> was right; the MECHANISM this spec leaned on was not. The verdict survives on margin —
> ~12× against the same-corpus saving — not on the stated mechanism.
>
> **The real lever is intra-app SHARDING, and this spec structurally excluded it** by fixing
> "one file per key" (§9) with key = app (§7), so the choice was framed as
> postcard-versus-string-table when the load-bearing variable was artifact granularity. Split
> the Base Application pack into N independently-hashed shards and the 68–111 ms critical path
> falls toward the ~7–11 ms the other eight packs already demonstrate. Three measured facts say
> this is cheap: a pack is already a `Vec<PackedFile>` (`src/program/pack/mod.rs:113`), so a
> shard boundary is a file-subset boundary needing NO schema change; shape A's 7,416 per-file
> frames cost 0.42 MB and decoded FASTER than shape B in all six logged runs, so finer framing
> is free or better; and integrity is ~3 ms per round, so per-shard hashing is noise.
>
> **This changes what belongs in reserve.** The string table attacks the parse, not the shape
> of the serial unit — so it does not address the one risk the ledger names, that the number
> scales one-for-one with Base Application growth. Sharding retires that risk; re-measuring
> against a switch line does not. Hold sharding as the intended response, and record an
> archived/zero-copy format (rkyv-shaped, load ≈ read + verify ≈ 10–16 ms) as the other
> reserve: the string table attacks ~3 ms and the allocation count, an archived format attacks
> the whole ~85 ms. It was a defensible economy not to ask that at 12× margin — the owned node
> types would need borrowing twins — but it should be on the table by name, not absent.

**The gate therefore measures the hit-path shape, not a micro-benchmark:** per-app packs,
parallel decode, guid re-intern included, from a cold OS file cache at least once. Under
~200 ms proceed; approaching ~600 ms switch to the string-table format and re-measure. Note
the 600 ms line is a product-quality choice, not a soundness line — even a 300 ms load
against 1,280 ms saved is a net win.

**Unclaimed upside:** on a hit the dependency `AlFile` arenas are never allocated, so
`preflight.ctx_drop` (248.7 ms) shrinks too. The true saving exceeds the 1,280 ms headline.

**Honest ceiling:** `assemble_graph`'s 116.4 ms clone and re-sort survive untouched on every
run. Packs never make dependency cost proportional to workspace size.

## §14 — Build order

0. **Preparatory corrections** (§17). Doc and dead-code cleanup inside territory this spec
   touches; clears one open question out of step 2 before it starts.
1. **Light snapshot** + the key-equality test (§10). Standalone value; prerequisite.
2. **Serialization surface** for `ObjectNode` / `RoutineNode` / `RoutineMeta` and their
   field-type closure. `node_extract.rs` has zero serde today.
3. **Measurement gate** (§13). Placed after 2 because it cannot run without it, and before
   the seam because abandoning here costs derives and a benchmark, not ingestion surgery.
4. **`EXTRACTION_FINGERPRINT`** build script + the canary test (§8). Pure invalidation
   plumbing; not needed to measure, and the component most likely to be redesigned.
5. **Pack seam** in `build_dep_layer` (§5), including both rewires in §11.
6. **Cold-vs-warm output-equivalence gate** (§12) + discrimination proofs.

## §15 — Accepted debt

**No pruning.** This is the third unpruned cache in the repo (`preflight_cache`,
`al-ch-snapshot-cache`, now packs). Accepted deliberately here, unlike the other two, because
pack cardinality is bounded by (BC versions × localizations touched) and pack entries are
*meant* to live indefinitely — that is the base-version library premise of §2. Record the
reasoning, not merely the debt. If §2 is falsified, this decision must be revisited with it.

Note that `preflight_cache`'s unpruned state is currently *mis-documented* as avoided;
§17.1 corrects that claim as part of step 0, so this section's "third" is honest rather than
contradicted by the module it cites.

## §15a — Carried into step 5's brief (from the format-gate plan's reviews)

Raised and deliberately parked during `2026-08-07-dependency-pack-format-gate`, because
each is a CODE change outside that plan's scope. They must not be lost between plans.

- **`encode` trusts its caller to have called `compute_self_hash` first.** Nothing
  type-level enforces it. All SEVEN callers are correct today — six in
  `src/program/pack/mod.rs` plus the bench's `encode_packs`
  (`benches/dep_pack_roundtrip.rs:378`) — so
  this is not a live defect. The durable fix is to make `self_hash` unconstructible
  by hand, or have `encode` compute it itself, so a future caller cannot write a pack
  whose hash does not cover its body. Do this when the seam lands, before any pack
  reaches a real cache directory.
- **The gate bench leaves pack artifacts in a temp directory.** Arguably a feature while
  the bench is run by hand — the artifacts are inspectable — but `U:` is documented as
  disk-constrained, so revisit if it bites.
- **The bench hardcodes `parse_status_recovered = false` for every file** in both pack
  shapes. Immaterial to the gate (it is a bool), but the real seam must carry the true
  per-file value, since `snapshot::parse::recovered_file_paths` is the load-bearing
  absence-proof diagnostic (§11.2).

## §16 — Live risks

- **§2's hit-rate premise is domain knowledge, not a measurement.** Low observed hit rate
  falsifies the design's justification, not just its tuning.
- **Packs inherit `al-ch-snapshot-cache`'s soundness** (§9).
- **The coarse closure over-invalidates** during engine development; if that proves painful,
  narrow it only with the canary in place.
- **`al_syntax::parse` constructs a fresh `tree_sitter::Parser` and calls `set_language` per
  file** (`crates/al-syntax/src/parse.rs:11-14`); there are no thread-local parsers anywhere
  in the tree. Inside the same span packs target, but a genuine change to the parse path
  rather than a docs fix — see §17.5, which keeps it a separate deliverable with its own
  before/after measurement.

---

## §17 — Preparatory corrections (step 0)

Four documentation/dead-code defects found while writing this spec, all inside territory it
touches. Each was verified against source; none is speculative. §17.1–§17.4 are step 0.
§17.5 was a code change tracked separately (measured and closed by TLS-67, 2026-11-15).

### §17.1 — `preflight_cache.rs` claims a defect it half-inherited

`preflight_cache.rs:66-70` states the `snapshot::cache` defect "(no override, no pruning)" is
one "that this module deliberately does not copy". It copied half: the override exists
(`ALSEM_PREFLIGHT_CACHE_DIR`), but **pruning was never implemented** — no `prune`, no
`max_age`/`max_size`, no `remove_file` outside failed-rename cleanup — despite that module's
own spec (`2026-08-01-preflight-verdict-cache.md` §4) requiring size/age bounds.

This is the over-claim class CLAUDE.md's Testing Philosophy section already names ("docs
asserting guards are 'each pinned executably' when only some are"). **Fix:** correct the doc
to claim only the override, and state the missing pruning as known debt. Do NOT implement
pruning here — that is a separate decision, and §15 deliberately accepts unbounded growth for
packs.

### §17.2 — CLAUDE.md's thread-local parser claim is false

CLAUDE.md:210 reads "**Parallel parsing** (`rayon`): Thread-local parsers process files
concurrently". The rayon half is true (`parse_snapshot` uses `par_iter` on `big_stack_pool`,
`src/snapshot/parse.rs:86-95`). The thread-local half is false: repo-wide, `thread_local`
occurs only at `src/engine/perf_trace.rs:395` and
`src/engine/gate/policy/predicate_evaluator.rs:168`, neither a parser.

**Fix (doc only, here):** state what is actually true — parallel per-file parsing on a
dedicated rayon pool, one parser constructed per file. The code change is §17.5.

### §17.3 — `Origin.ts_id` is a dead field behind a doc naming a dead consumer

Declared at `crates/al-syntax/src/ir/mod.rs:43`, written once at `lower/mod.rs:1909`, read by
**zero** production code. Its doc says "NEVER serialize… tree-sitter recycles ids" and names
the L2 op/callsite maps as its consumer; that consumer no longer exists. The only remaining
mentions are three `ts_id: 0` test constructions and comments in
`tests/lsp/lsp_incremental_parity.rs`.

The prior spec already reached this conclusion: "The stated blocker is guarding a dead field;
delete it rather than designing around it."

**Fix:** delete the field and its doc. Doing this in step 0 removes a standing question from
step 2's serialization surface before that work begins.

### §17.4 — CLAUDE.md's `.scm` paths are wrong

CLAUDE.md:155 and :292 reference `queries/highlights.scm` / `queries/tags.scm`; both live at
`tree-sitter-al/queries/`. The substantive claim is sound — the files exist, reference the
same grammar node names, and therefore justify keeping `src/language.rs`'s dead query
constants (`DEFINITIONS`/`CALLS`/`EVENT_SUBSCRIBERS`/`VARIABLES`, confirmed at zero call
sites repo-wide). **Fix:** correct the paths only.

### §17.5 — Per-file parser construction (separate deliverable; measured and closed 2026-11-15, see the end)

`al_syntax::parse` builds a fresh `tree_sitter::Parser` and calls `set_language` on every
call (`crates/al-syntax/src/parse.rs:11-14`), so a DO run pays that ~11,856 times for
dependencies alone plus once per workspace file.

Not folded into step 0 because it is a real change to the parse path and deserves its own
before/after measurement rather than riding in a docs commit. **No saving is claimed here —
it must be measured, not asserted.**

Worth doing independently of packs: a pack hit skips this entirely, but the miss path (the
first analysis of any new base version) and every workspace file still pay it on every run.

**Measured and closed (TLS-67, 2026-11-15): no gain of 1% shown, not landed.** A throwaway probe on the CDO
baseline's 582 workspace files (`release-fast`, one thread, 3 rounds, load average ~21 from
other work): `Parser::new` + `set_language` took 86-117 µs for all 582 files (~0.15-0.2 µs
each), against 685-781 ms for parse + lower, so 0.011-0.016% of it. A reused parser's run
(663-817 ms) was within the noise (about ±7%) of the fresh one. What warm internal buffers
(the parse stack and subtree pools a fresh parser regrows) save was then measured directly: a
thread-local parser in `al_syntax::parse` against a parser per file, 12 interleaved rounds per
run, load average 9-21. Wall-time median ratio 0.94 (another session's build overlapped
that run), then 1.06 (rounds 0.85-1.57); thread CPU time median 0.987 (rounds 0.86-1.15, 7 of
12 faster). In both wall-time runs the side calling `parse()` was ~6% slower whichever parser
it used, so wall time was confounded by the code path and could not isolate reuse. The bar was 1%; no run reached it
reliably, and the change was dropped. At ~0.2 µs a file, the
~11,856 dependency files add ~2 ms. The probe was deleted after the run.

**Checked and found clean while hunting** — recorded so the sweep is not repeated: the
grammar pin is genuinely v3.2.0; the goldens table's "30 golden directories / 9 test targets"
is exact (`tests/goldens` is a parent, not a golden dir); every other `src/`, `crates/`,
`scripts/`, `tests/` path in CLAUDE.md resolves; `scripts/check-goldens` and
`scripts/git-hooks/pre-commit` exist with `core.hooksPath` configured; and
`tests/perf_support/` really does declare two publishers and two subscribers per file.

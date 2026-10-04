# Prior art: sharing library analysis across workspace roots (AL call-graph engine)

Legend: [V] = verified from a source I opened or a search snippet naming that source. [I] = my inference or recall, not verified this session.
Limits: WebFetch returned little (the rust-analyzer architecture page does NOT discuss durability/LRU). Most [V] items come from search-result snippets, not full reads. Roslyn and clangd-shard details are recall ([I]).

## 1. IDE / language-server engines

### 1.1 rust-analyzer: ItemTree vs Body, durability, LRU
- What: a per-file `ItemTree` is a "summary" of a syntax tree that is stable under function-body edits; bodies are lowered separately (`Body`). Invariant: "typing inside a function's body never invalidates global derived data". [V] https://github.com/rust-analyzer/rust-analyzer/blob/d7c99931d05e3723d878bea5dc26766791fa4e69/docs/dev/architecture.md ; https://git.bendn.org/rust-analyzer/plain/crates/hir-def/src/item_tree.rs
- Syntax trees are "semi-transient": the frontend does not keep trees for all files; it lowers them to compact forms. Only `parse`, `parse_macro_expansion`, `macro_expand` are LRU-limited by default; LRU became opt-in per query and capped at 2^16. [V, snippet] https://git.dreamy.place/mirrors/rust/plain/src/tools/rust-analyzer/docs/dev/syntax.md?h=1.65.0 ; https://github.com/rust-lang/rust-analyzer/issues/1643
- Durability: library source files get HIGH durability (they "never should change"); crate-graph changes get MEDIUM. Tagging libraries as low durability made salsa validate a bigger graph; the fix cut `CrateDefMapQuery` from ~64 ms to ~14 ms and validations from 60k to 7k. [V, snippet] https://git.joshthomas.dev/language-servers/rust-analyzer/commit/0b8fbb4fad97d2980f0070a23f5365a5ed887e2a
- Contrast: rust-analyzer does not share a library per "root". There is one global salsa DB and one CrateGraph; a library crate exists once and many workspace crates point at it. Sharing is by identity, not by cache keying. [V that CrateGraph holds inter-crate deps; "one DB" is [I]]
- Maps to us: your Arc cache keyed by file set is the cache-keyed version of "one library node". The lesson is architectural: the library tier is ONE immutable object referenced by N root graphs, never copied. You did this for decl nodes and symbol packages; do the same for source text and per-routine metadata.
- Payoff: matches your 1.2 -> 0.5 GB result. Risk: low. Hard part is invalidation (your key has path+size+mtime). "High durability" = do not revalidate the library tier on a root edit; an immutable Arc gives you that.

### 1.2 TypeScript: DocumentRegistry
- What: a store of `SourceFile` objects shared across LanguageService instances; "SourceFile objects account for most of the memory usage"; sharing one registry means all projects share at least lib.d.ts. Keyed by file name + compilation settings + version, with acquire/release (ref-counting). [V, snippet] https://github.com/microsoft/TypeScript/wiki/Using-the-Language-Service-API
- Maps: this is your Arc cache plus ref-counting. The key includes settings that change parsing; for you, that means grammar version and anything affecting parse (preprocessor symbols, runtime version). If any such setting can differ per root it must be in the key. [I]
- Payoff: same as what you got. Risk: lifetime bugs. Cheap fix: hold the cache with `Weak` so the tier frees itself when the last root drops. [I]

### 1.3 Roslyn: metadata references and skeleton references
- Metadata references are shared through a global cache of metadata readers and `AssemblySymbol`s in `ReferenceManager`; reference assemblies hold metadata only, no IL. [V, snippet] https://source.dot.net/Microsoft.CodeAnalysis.CSharp/Symbols/ReferenceManager.cs.html
- `SkeletonReferenceCache` exists in Microsoft.CodeAnalysis.Workspaces and is used by the compilation tracker. [V that it exists, snippet] https://source.dot.net/Microsoft.CodeAnalysis.Workspaces/R/c4083ab11cd9c3f9.html . What it does [I, recall, unverified]: for a project-to-project reference it emits a metadata-only skeleton assembly from the referenced project, caches it, and reuses it while the public surface is unchanged, so body edits do not rebuild dependents.
- Maps: your library tier is a skeleton. The useful trick is "skeleton identity = fingerprint of the public surface". You already have that in `src/lsp/def_surface.rs` (rung-1/rung-2 gate); reuse it as the library tier's version stamp.
- Payoff: confirmation, not a new memory win. Risk: none.

### 1.4 clangd: preamble and background index
- Preamble: parse the header prefix once, reuse until a preamble file or the compile command changes; called the most important clangd optimization. Building the preamble's dynamic index needs a full AST walk, so it runs asynchronously. [V, snippet] https://www.llvm.org/devmtg/2023-10/slides/lightning-talks/10-Improving%20clangd%20document%20open%20time%20with%20preamble%20caching.pdf ; https://llvm.googlesource.com/clang-tools-extra/+/refs/heads/master/clangd/Preamble.h
- Background index [I, recall]: persists per-file index shards on disk keyed by content digest, loaded at startup, shared by every TU that includes the header. Verify at https://clangd.llvm.org/design/indexing before relying on it.
- Maps: preamble = library tier (parse once, reuse). Persisted shards = an on-disk library summary so even the first root skips the parse on the next editor start.
- Payoff: removes the first-root transient on every start after the first. Risk: cache invalidation and format versioning.

### 1.5 IntelliJ: stub trees and shared indexes
- A stub tree is a subset of the PSI tree with only externally visible declarations, serialized compactly; stub indexes are built on serialized stubs. Touching anything outside the stub forces a switch to full AST parse. [V] https://plugins.jetbrains.com/docs/intellij/indexing-and-psi-stubs.html
- "Shared indexes" are built once (JDK, libraries, project) and reused on other machines instead of being rebuilt locally. [V] https://www.jetbrains.com/help/idea/shared-indexes.html.md
- Maps: the closest analogue of "parse library, keep declarations only, persist, fall back to full parse for bodies". Design point: the stub must hold everything resolution needs, or the slow fallback becomes common. For you that includes call obligations if library routines must have outgoing edges.
- Payoff: large (see section 3). Risk: an incomplete stub gives slow paths or wrong answers.

## 2. Summary-based / compositional interprocedural analysis

### 2.1 Averroes (placeholder library)
- Ali and Lhotak, "Averroes: Whole-Program Analysis without the Whole Program", ECOOP 2013. A placeholder library over-approximates library behaviour (~80 kB of class files vs the 25 MB Java standard library); call-graph construction 4.3x-12x faster, memory 8.4x-13x lower. [V, snippet] https://plg.uwaterloo.ca/~olhotak/pubs/ecoop13.pdf
- Basis: the "separate compilation assumption" (the library does not know the application). Cost: over-approximation, so extra imprecise edges. [V basis; precision cost [I]]
- Maps: in AL the library reaches extension code only through events, interfaces and dynamic dispatch, the same shape. A library routine's edges to library targets are root-independent; edges into app code exist only via event subscribers/interfaces. So library-internal edges can be computed once; root-specific edges are an overlay. [I]
- Payoff: large if you only need library edges, not bodies. Risk: precision loss if you pre-collapse event fan-out; keep publisher -> subscriber lists explicit so root subscribers can be added.

### 2.2 IFDS/IDE library summaries, StubDroid, Infer
- Rountev, Sharp, Xu, "IDE Dataflow Analysis in the Presence of Large Object-Oriented Libraries", CC 2008: precomputed library summaries (graph form of summary functions) over Java 1.4 (25,490 methods); about 2x faster than from scratch, no precision loss. [V, snippet] https://web.cs.ucla.edu/~harryxu/papers/rountev-cc08.pdf
- Rountev and Ryder, "Points-to and side-effect analyses for programs built with precompiled libraries", CC 2001: analyse a library separately under worst-case assumptions about clients; building and storing summaries was cheap. [V, snippet only]
- Arzt and Bodden, "StubDroid: Automatic Inference of Precise Data-flow Summaries for the Android Framework", ICSE 2016: summaries inferred once from the binary library, reused by any analysis; apps analysed in seconds where full re-analysis timed out at 30 min; less time and memory than hand-written summaries. [V, snippet] https://ris.uni-paderborn.de/record/20729
- Arzt and Bodden, "Reviser", ICSE 2014: incremental IDE/IFDS updates, clear-and-propagate, up to 80% faster than recomputation. [V, snippet] Relevant to updating summaries after edits, not to sharing.
- Infer: one summary per procedure, computed bottom-up; callers read only callee summaries (bi-abduction); runs incrementally on diffs. Distefano, Fahndrich, Logozzo, O'Hearn, "Scaling Static Analyses at Facebook", CACM 2019. [V, snippet] https://engineering.fb.com/2019/02/08/developer-tools/infer-team-award/
- What is stored per library: Averroes = synthetic stubs; IDE = flow-function graphs; StubDroid = source-to-sink flow facts per method; Infer = pre/post specs per procedure. None keeps bodies. [I, synthesis]
- Trade-off: one-time summary cost and some completeness/precision risk, in exchange for dropping bodies. Your resolver needs callee declarations and call edges, not dataflow facts, so your summary is simpler: (routine id, signature, outgoing call obligations/edges, event publisher/subscriber facts). Risk: your L4/L5 effect and db-effect detectors may read library bodies; check which consumers do before dropping them. [I]

## 3. Reducing parse / IR memory

### 3.1 Parse, summarize, drop bodies (stub / ItemTree / header-compiler style)
- rust-analyzer ItemTree and IntelliJ stubs [V above]. Google turbine is a Java "header compiler" producing signature-only output [V that it exists, snippet; details [I]] https://android.googlesource.com/platform/external/turbine/+/86c0940/ . TASTy: "method bodies can be elided" for separate compilation [V, snippet] https://github.com/lampepfl/dotty/pull/19074
- Maps: your ~800 MiB transient is library IR alive until the program graph and metadata are built. If lowering reduces each file to its summary before moving on, peak is about (threads x largest-file IR) + summaries. If the rayon `par_iter` collects every `ParsedFile` first, all IR is live at once; mapping to summaries inside the closure removes that. [I; verify in `src/snapshot/parse.rs` and `program::build`]
- Payoff: peak from ~800 MiB toward tens of MiB per thread (estimate, not measured). Cost: build must not need whole-library IR at once; any cross-file pass reading library bodies (obligation extraction in `resolve_full_program`) must become per-file. Risk: medium, a pipeline restructure.

### 3.2 Lazy body parsing on demand
- IntelliJ falls back from stub to AST only when asked [V]; rust-analyzer computes `Body` separately and on demand, with an LRU on `parse` [V].
- Maps: keep shared library text, store per-routine byte ranges in the summary, parse a body only when something needs it. Payoff: removes library body IR entirely if nothing needs it eagerly. Risk: consumers that assume eager bodies; latency on first access; needs an LRU.

### 3.3 Persisted, content-addressed summary cache
- IntelliJ shared indexes [V]; clangd shards [I]. Key = hash of file bytes (stronger than path+size+mtime) + grammar/engine version. You already have `CACHE_VERSION_GRAMMAR`-style invalidation for the dependency cache (per your CLAUDE.md), so the discipline exists.
- Format: zero-copy archives (rkyv, FlatBuffers) skip deserialization. A vendor page claims ~1.4 ms cold start for ~10 MB (Parcode) vs ~299 ms bincode [V that the claim exists; vendor benchmark, treat as marketing] https://docs.rs/crate/parcode/latest . An mmap of the archive could also take library metadata off the heap (OS-reclaimable pages). [I]
- Payoff: warm-start first root goes from "parse 8,600 files" to "map one file"; first-ever start unchanged. Risk: format versioning, hashing 8,600 files costs I/O, mmap and file locking on Windows, schema churn when `RoutineNodeId` changes. The recent "never share an unstamped dep tier" fix in git log shows stamping is the real hazard.

### 3.4 Interning, hash-consing, arenas
- rustc-style arena-backed symbol interners give O(1) compare and one copy per distinct string. [V, low-quality snippet] https://github.com/cad97/strena
- Maps: you already use `string-interner`. Question is whether the interner is per root (duplicating library strings per root) or shared with the tier. A shared tier needs either one process-global interner or tier-local symbols that never leak into root graphs. [I]
- Payoff: modest, unmeasured. Risk: a global interner is never freed; stable ids across tiers need care. Arenas free a whole file/tier at once and cut allocator overhead, but mimalloc with purge_delay=0 (your notes) already captures part of that. [I]

## 4. Call hierarchy / call graph over large dependency closures
- I found no paper or doc on LSP call-hierarchy memory over large dependency closures. Results were generic blog posts (clangd large-codebase issues; "reduce depth, use a shared index"), which is weak evidence. [V: nothing substantive]
- Likely practice [I, unverified]: clangd serves call hierarchy from its persisted symbol/ref index, not ASTs; rust-analyzer finds incoming calls by searching workspace crates only. Consequence for you: a library routine's incoming calls from other library routines are rarely wanted in a root's hierarchy, so library reverse edges could be built lazily or on demand from the shared tier.

## Event edges (per-root today)
- No prior art found as such; Averroes' separate-compilation reasoning is the nearest. [I]
- Split event edges into: (a) library publisher -> library subscriber: root-independent, belongs in the shared tier, compute once; (b) library publisher -> root subscriber: per root, small; (c) root publisher -> library/other-root subscriber: per root, small. Per-root cost then scales with the root, not Base App. If (a) dominates today, sharing it is a straightforward win.
- Caveat: wiring may depend on the root's dependency closure (subscribers in OTHER shared libs). Key the shared (a) set on exactly the library set, which your cache key already is.

## Ranked recommendations

### (a) First-root transient parse cost (~800 MiB)
1. Stream parse -> summarize -> drop IR per file (3.1). Root-cause fix for the transient, independent of caching. First run a byte census (the `byte-census` skill) to confirm what the 800 MiB is (CST, IR, or the collected Vec). Risk medium: check which later passes read library bodies.
2. Persisted content-addressed summary, mmap/zero-copy (3.3; IntelliJ shared index / clangd shard analogue). Removes the parse on warm starts. Do it AFTER (1), because (1) defines the summary format.
3. Cap concurrent big-IR files in the rayon pool (cheap stopgap): peak = threads x per-file IR. No CPU saving. [I]
4. Lazy body parse with an LRU (3.2), only if a consumer is found that truly needs library bodies; lets (1) avoid guaranteeing completeness.

### (b) Per-root event edges
1. Split into a shared library-internal set (in the library tier Arc) plus a per-root overlay (Averroes-style). Biggest and simplest win; reuse the existing library file-set key.
2. Use compact endpoints: u32 indices into the shared tier instead of full `RoutineNodeId` per edge (about 8 bytes per edge). Cheap; enables (1). [I]
3. Demand-driven overlay (compute subscribers per publisher when queried, cache), rust-analyzer style. Only worthwhile if few publishers are queried; unmeasured.

## Not verified / check before acting
- Roslyn skeleton behaviour and clangd shard format (recall only).
- rust-analyzer LRU details (snippets from older commits; may have changed after the salsa rewrite).
- That later passes do not need library IR bodies (read `program::build`, `resolve_full_program`, L4/L5 consumers).
- All payoffs above are estimates; the only measured number is your own 1.2 -> 0.5 GB.

---

## Addendum (2026-10-04): what kind of graph ours is, and what is done to that kind of graph

Written after the SerpApi search the user asked for: classify the graph first, then look at how
that class of graph is usually stored, instead of searching for "call-hierarchy memory".

**Classification.** Our program graph is a labelled, directed **property multigraph**: typed
edges (call, run, implicit trigger, event flow; several between the same pair), nodes with many
attributes (access, tier, signature fingerprint, return type, publisher kind…), and structured
node identity (`RoutineNodeId` = object + name + member + arity + signature fingerprint) rather
than an integer. It sits on an app → object → routine containment hierarchy, with app-level
dependency edges forming a DAG. Shape: a large **immutable base** (the dependency tier) plus a
small, frequently rebuilt **per-root delta** (the workspace). Closest relatives: code property
graphs (Joern's CPG / flatgraph, CodeQL databases).

**Techniques for this class, with the evidence found:**

- **Columnar storage** — node properties kept as a few large arrays per property, not one heap
  object per node. Joern moved from OverflowDB to *flatgraph* ("efficient columnar layout … in
  few, albeit very large, arrays"): on Linux 4 (48M nodes, 431M properties) heap 33 → 20 GB,
  minimum heap 80 → 30 GB, import 18 → 11 min, disk 2.6 GB → 400 MB; edges carry at most one
  property. Source: github.com/joernio/joern, `changelog/4.0.0-flatgraph.md`.
  *Fit:* our `RoutineNode`/`ObjectNode` are structs of `String`/`Vec`/`Option`, each its own
  allocation; one root held ~1.3 M allocations (census, 2026-10-03).
- **Dense integer node ids** — edges and indexes keyed by `u32` plus one id → key table; standard
  in CSR, WebGraph (Boldi & Vigna, WWW 2004) and LLAMA. *Fit:* our ids are fat structs with
  strings, deep-cloned into every map key (`dep_meta`, `decl_by_id`, `incoming`, edges). Not yet
  measured how much of `dep_meta`'s 55 MiB / 600k allocs this is — census first.
- **String interning** — one copy per distinct string. FalkorDB reports 20–60% less memory on
  graphs with repeated attributes and up to 75× faster equality checks. Source:
  falkordb.com/blog/string-interning-graph-database. *Fit:* names, lowercase names and signature
  keys repeat heavily; `string-interner` is already a dependency but unused by the program engine.
- **Projections** — a consumer loads only the labels, relationships and properties it needs into
  an in-memory catalog, reusable by several algorithms (Neo4j Graph Data Science named
  projections). *Fit:* the user's "light graph for al-call-hierarchy, full graph for al-sem" — one
  engine, one format; each consumer declares the columns it reads. Caveat: the LSP already runs
  the full resolver, so only detector-only columns (L3/L4/L5) can be dropped for it; the bigger
  win is the representation change itself, which helps both.
- **Immutable base + copy-on-write delta** — LLAMA (Macko et al., "Efficient graph analytics using
  Large Multiversioned Arrays", ICDE 2015): a mutable CSR stored as snapshots; 3–18% overhead vs an
  immutable CSR. *Fit:* what al-sem v1.3.2 (shared dependency tier + per-root overlay) and the
  embedded-mode sharing already do.

**Next step agreed with the user:** a "compact graph core" — census of string and id-clone bytes
and of which node columns each consumer reads; then a spec for dense `u32` ids, interning,
columnar node attributes and named projections (light for `al-call-hierarchy`, full for
`alsem`), gated on byte-identical goldens and the CDO metric.

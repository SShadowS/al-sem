# Preflight verdict cache — spec

**Status:** ready to implement. **Base:** master `90b5918`.

> **Removed 2026-10-05 (B3 Phase A).** The cache was implemented
> (`src/program/resolve/preflight_cache.rs`) and is now deleted. Since B3,
> `alsem analyze` needs the program context itself (the detectors read its call
> resolution), so a cached verdict could only be stored, never served. Old
> entries under `<os-cache>/alsem/preflight-v1/` are no longer read or written;
> `alsem cache prune` never managed that directory, so delete it by hand.
> The text below is kept as the record of the design.
**Measurement it is built on:** `docs/2026-07-31-preflight-census.md`.
**Design pointers:** `docs/superpowers/notes/2026-08-01-preflight-cache-pointers.md`.

`preflight.fresh_coverage` is **83.4 % of a DO run** (2,642 ms of 3,171 ms) and
10.8 % of 8020. It builds, resolves and destroys a whole-program model of ~11,600
files — ~95 % of them dependency source that did not change since the last run —
to produce four scalars.

---

## §0 — Settled questions

Both were named as blockers in the pointers doc. Both are now answered from source,
and the answers set this spec's boundaries.

### Q1 — should the preflight narrow to primary scope? **No. Closed.**

The preflight's four degradation clauses mix scopes today
(`src/engine/gate/preflight.rs:52-95`): `unknown` is **primary-scoped**
(`report.primary_histogram`), while `coverage_holds` and `recovered_files` are
**whole-program** (`ProgramReport` doc, `full.rs:182-191`; `Coverage` is distinct-id
set equality over ALL edges, `full.rs:167-178`), and `opaque_apps` is the primary's
reachable closure.

It is stricter than the analysis it gates. The detectors never see dependency source
at all: the gate's substrate is `assemble_and_resolve_workspace` →
`assemble_l3_workspace_from_disk` → `discover_al_files_app_scoped`, which excludes
`.alpackages` (`l3_workspace.rs:1474, 1528-1546`). So a dep-internal resolution hole
degrades a verdict for an analysis that never looked at dep code.

That is a real asymmetry, and narrowing it is still the wrong move — three reasons,
in order of weight:

1. **Narrowing can only ever convert `degraded` → `clean`.** A narrower scope
   observes a subset of the holes. For an instrument whose entire purpose is "never
   report a verified-clean analysis it did not verify", strictly-more-permissive is
   the unsafe direction.
2. **It buys nothing on the corpus we care about.** DO's preflight is already fully
   clean — measured: `opaqueApps: []`, empty stderr, no degraded warning, so all four
   clauses pass. Narrowing changes DO's verdict by exactly zero. The perf motive is
   better served by caching, which is verdict-preserving *by construction*.
3. It would move committed output on any workspace that currently degrades for
   dep-only reasons.

**Consequence for this spec: the cache must reproduce today's verdict exactly. Scope
semantics are out of scope.** If the asymmetry is ever revisited, it is a deliberate
contract change with its own review — never a side effect of a perf task.

### Q2 — are dep-derived products a pure function of the dep universe? **Split. Parse yes, edges no.**

`ResolveIndex::build` populates `subscribers_map` by iterating **all** of
`graph.routines` with no app scoping, keyed by the resolved *publisher*
(`src/program/resolve/index.rs:285-300`), and `emit_event_flow_edges` walks every
publisher routine emitting one Route per subscriber
(`resolver.rs:3006-3045`). A **primary-app subscriber therefore attaches to a
dependency publisher**: adding a subscriber in primary source changes the edge set
emitted for a dep publisher routine.

**So dep-derived EDGES are NOT a pure function of dep content.** Any artifact that
persists dep-internal classified edges keyed only on dep bytes is unsound.

**Dep-derived PARSE/DECLARATION products ARE pure.** `extract_nodes(app_ref, &pf.file,
tier, …)` (`build.rs:89-97`) reads only the parsed file and its tier; `parse_snapshot`
is one `al_syntax::parse` per file. Nothing primary-dependent enters.

**That boundary is fortunate: the pure side is the expensive side** (the 1,139 ms
parse) and the impure side is the cheap one. It unblocks the follow-up lever (§6)
without unblocking an unsound version of it.

One trap that rides along: `AppRef` is a per-run interning index
(`build.rs:76-77`), so any persisted node artifact must store app identity
**symbolically** (guid) and re-intern on load. Forgetting this yields silently-wrong
graphs, not errors.

---

## §1 — Scope

**In:** a content-keyed, fail-closed, on-disk cache of the `FreshCoverage` verdict.

**Out:** the dependency parse-artifact cache (§6 — now unblocked by Q2, still its own
spec), any change to preflight scope semantics (Q1), any change to what the detectors
consume.

**Explicitly de-scoped, with its reason:** persisting `AbiCache` was carried into this
work as "Lever 3". After correction it is **not worth a task**. It is a process-level
in-memory map (`abi_ingest.rs:121-135`) whose key is `(guid, name, publisher,
version)` — version-based, so persisting it as-is is unsound (rebuilding a dep `.app`
at the same version with different content is routine in dev). And on DO the
dependencies ship embedded source, so they are parsed as source rather than ingested
as ABI: the path it covers is a slice of `dep_layer`'s **141.9 ms**, not of the
1,139 ms parse. Sized honestly it is a tens-of-ms lever on DO and a **0 ms** lever on
8020. Fold it into §6 if the dep-artifact cache ever makes a content hash cheap to
reach; do not build it standalone.

### What this actually buys, stated honestly

| | DO | 8020 |
|---|---:|---:|
| preflight today | 2,642 ms (83.4 % of run) | 3,503 ms (10.8 %) |
| warm-hit floor (`snapshot_build` + key) | ~470 ms | ~455 ms |
| **saved per warm hit** | **~2.17 s** | **~3.05 s** |

**It does not help the edit loop.** The key must cover primary source content, so any
edit is a guaranteed miss and a full recompute. This is an *identical-input rerun*
optimization: CI re-runs, a second invocation for another `--format`, a re-run after a
no-op. Say so in the CHANGELOG; do not let it be read as an interactive-latency win.

The `snapshot_build` floor is **deliberate and accepted**. The sound key derives from
the snapshot the preflight already builds. A cheaper pre-snapshot key would require a
second discovery implementation that can drift from the real one — exactly the defect
`compute_gate_model_instance_id` already demonstrates (§2).

---

## §2 — The key

**Principle: derive the key FROM the snapshot the preflight already builds.** Split
`build_context_res` so `SnapshotBuilder.build()` runs first; compute the key from the
resulting `AppSetSnapshot`; look up; only on a miss continue into parse → dep_layer →
assemble → resolve.

`blake3( binary_identity ‖ canonical_fold(snapshot) )`, 64 lowercase hex, used as the
entry filename.

Fold **length-prefixed** throughout (house discipline: `sig_fp::write_len_prefixed`,
`sha256_of_strings`). Length prefixing is not stylistic here — see the collision class
in §2.2.

### §2.1 Components

1. **Binary identity** — blake3 of `std::env::current_exe()`, memoized in a
   `OnceLock`. This subsumes resolver behaviour, builtin catalogs, the grammar
   (tree-sitter-al is compiled in), the lowerer, and the semantics of every cached
   field, in one value that **cannot rot**.
   *Evidence that hand-bumped constants do rot in this repo:*
   `CACHE_VERSION_GRAMMAR` still reads `"tree-sitter-al-v2.5.2-native"`
   (`cache_prune.rs:40`) while the grammar is **v3.2.0** — never bumped across two
   grammar upgrades, and nothing failed.
   Accepted cost: an identical-source rebuild invalidates the cache (embedded
   timestamps). That fails toward recompute, which is the correct direction.
   Keep a separate hand-bumped `PREFLIGHT_CACHE_SCHEMA` **only** for the entry's
   serialization shape (§3).
2. **Workspace identity** — `snap.workspace_app` (guid, name, publisher, version) and
   `snap.world`.
3. **Per app in `snap.apps`**, canonically ordered (guid, then version, then content
   hash): guid, name, publisher, version, tier, `declared_deps` (verdict-load-bearing —
   `opaque_dependency_closure` BFSes over them, `full.rs:1215-1247`),
   `internals_visible_to` (feeds resolution visibility, `build.rs:127`), `compilation`,
   and source-content identity:
   - EmbeddedSource / SymbolOnly dep → the `.app` blake3 already computed by
     `cached_source` (`snapshot/cache.rs:38-40`). **Free — already paid for.**
   - Workspace / local-repo source → this cache's **own** per-file fold of
     `(virtual_path, len, text)`. **Do not reuse `SourceRoot.content_hash`** (§2.2).
4. **The full discovered app set, not the reachable closure.** `load_all_apps` loads
   every `.app` in ancestor `.alpackages` without app.json filtering
   (`full.rs:1181-1184`), and those become graph nodes; subscriber wiring is
   whole-snapshot-scoped (Q2). Folding all of `snap.apps` covers this. An unrelated
   package appearing or vanishing causes a spurious miss — acceptable, fails toward
   recompute.

`.dependencies/` law is satisfied **by construction**: the key sees discovered app
content, never folder names. Any design that enumerates directories by name to build
the key both violates the law and reintroduces drift.

### §2.2 Why not `SourceRoot.content_hash` — and a latent bug to fix separately

`walk_al_source` folds only `f.text.as_bytes()`, concatenated, over path-sorted files
— **no `virtual_path`, no length prefix** (`snapshot/provider.rs:59-64`). Two
workspaces collide that must not:

- a file **rename** (same texts, different names), and
- **re-splitting the same bytes across different file boundaries** — e.g. `a.al`/`b.al`
  holding `"fo"`/`"obar"` versus `"foo"`/`"bar"`. Identical concatenation, completely
  different parse. **Verdict-changing.**

This spec does **not** depend on fixing it: the cache computes its own fold. But the
collision is a latent soundness bug in the snapshot layer independent of any cache,
already recorded in `OUTSTANDING.md`, and it is the discrimination proof for §5's
re-split test row. If it is fixed instead, audit `snapshot/verify.rs` and
`snapshot.rs:154-265` first.

---

## §3 — Entry format, and the three soundness rules

`fresh_coverage(workspace_root)` is a pure function of (workspace bytes + discovered
`.app` bytes + engine binary). It reads no config, no thresholds, no verdict-affecting
env — every `AnalyzeArgs` knob acts downstream of `fresh` (`run.rs:206-209`).
Memoizing a pure function on a content-complete key is **replaying a verification that
did happen on inputs proven byte-identical**, not fabricating one. That holds iff:

- **Rule 1 — the key is content-complete.** Every stale-verdict scenario is a key gap.
  There is no payload-level defence: a well-formed entry under a correct key is
  indistinguishable from a correct one without recomputing. **All soundness lives in
  the key plus entry integrity.**
- **Rule 2 — every abnormal state means recompute, silently.** Missing, unreadable,
  JSON/schema mismatch, self-hash mismatch, version mismatch → miss, log at debug,
  never an error. `snapshot/cache.rs:44-55` is the existing model.
- **Rule 3 — cache `Ok` only, never `Err`.** `fresh` is
  `Result<FreshCoverage, String>` and `Err` is the first-class *could-not-verify*
  state (`preflight.rs:55-63`). It captures **transient environment** — a locked file,
  a dying disk. Caching it laminates a one-time I/O flake into a persistent verdict.
  Degraded `Ok`s (`unknown > 0` etc.) **are** cached: they are as deterministic as
  clean ones, and the key changes on any edit anyway.

**The payload is byte-gated OUTPUT, not just an exit code.** `run.rs:398-400` copies
`fresh.opaque_apps` into the coverage block the Json/Terminal/Html formatters render.
The entry must round-trip `FreshCoverage` exactly, storing `opaque_apps` verbatim in
its produced order (already deterministically sorted at production,
`full.rs:1248-1250`) — **store as-is, do not re-sort on load**; §5.6 proves it.
`evaluate_preflight` also warns on stderr on every degraded run independent of
`--require-dependencies` (`run.rs:467-477`), so the warm path must reproduce that
warning identically. It does, free, if the round-trip is exact.

**Entry:** self-describing JSON — `schemaVersion`, the full key material (for audit;
the filename is the lookup), the versions tuple (binary hash +
`PREFLIGHT_CACHE_SCHEMA`), the `FreshCoverage` payload, and a self-integrity hash over
the entry. Precedent for the self-hash recompute: `cache_prune.rs:139-144`.

---

## §4 — Location, concurrency, invalidation

- **Location: a NEW versioned root**, `<os-cache>/alsem/preflight-v1/`. Do **not**
  co-locate in `~/.al-sem/cache`: that directory's artifact shape and prune stdout are
  al-sem-golden-pinned, and a foreign file shape lands in `removed-unreadable`.
- **Overridable dir is mandatory** (env var or injected path). The existing
  `snapshot::cache` is NOT overridable, which makes every test touching `.app`
  extraction share global mutable state. Do not repeat that — §5's tests are
  impossible to hand-state without an isolated dir.
- **Escape hatch:** `--no-cache` (or env), and it must be **exported by
  `scripts/cdo-gate`** so the north-star CDO ratchets can never measure a warm replay
  of themselves (§5.7).
- **Concurrency:** copy `persist_cache` (`snapshot/cache.rs:82-89`) — per-process tmp
  name, atomic rename, last-wins. Concurrent writers of the same key write identical
  bytes by construction (same key ⇒ same inputs ⇒ same verdict). Two engine versions
  never collide: the binary hash is in the key. Windows rename can fail on a sharing
  violation — warn and continue uncached, as the existing pattern already does.
- **Schema evolution:** bump `PREFLIGHT_CACHE_SCHEMA` ⇒ tuple mismatch ⇒ miss ⇒
  recompute. **Never migrate in place.**
- **Pruning:** size/age-bounded. Adjacent cleanup worth folding in:
  `al-ch-snapshot-cache` has no pruning at all and grows forever (relevant on a
  disk-constrained machine).

---

## §5 — Tests

**The organizing device — a tracer payload.** Hand-write a well-formed entry whose
payload says `unknown: 999`, a value the fixture workspace can never produce, by
literal `Write` — never by asking production code to mint it. Every test below then
has a two-outcome oracle: **999 surfaced ⇒ the cache was served; the true value
surfaced ⇒ recompute ran.** Both directions observable, so each test carries its own
can-fail proof rather than needing one bolted on.

Every test drives the **production entry point** (`run_analyze_with_exit`), never the
internal lookup helper. Pinning the helper instead of the use is this repo's
five-times-caught defect.

1. **Hit is real.** Seed tracer under the key computed for fixture W → run → assert 999
   surfaces. Proves the cache is consulted *by the use*.
2. **Key completeness table** — the soundness core, one row per key component. Build
   workspace pairs differing in exactly one input; assert the keys differ. Rows: flip
   one byte in a primary body; add a file; rename a file; **re-split one file into two
   with identical concatenated bytes**; delete a dep `.app`; replace a dep `.app` with
   same (name, version) but different bytes; edit `app.json` `dependencies`; edit
   `internalsVisibleTo`; vary binary identity.
   *Discrimination proof per row:* remove that component from the fold, watch the row
   fail, restore. The re-split row's proof is pre-recorded — it fails against today's
   `SourceRoot.content_hash` (§2.2). For scripted breaks, **assert the patch applied**
   (`assert s.count(old) == 1`) — an unasserted scripted break proves nothing.
3. **Fail-closed corpus.** For each of {truncated JSON, wrong `schemaVersion`, version
   mismatch, self-hash mismatch, entry deleted}: seed tracer, corrupt, run → assert the
   TRUE verdict (not 999) **and** that a fresh valid entry replaced the bad one.
4. **Stale content never served.** Seed tracer for W, flip one byte in W, run → true
   verdict. (Row 2 exercised through the use.)
5. **Never cache `Err`.** Make the snapshot fail (id-less root `app.json` — the
   documented fail-closed layout) with an empty cache dir → assert could-not-verify
   surfaced AND the dir is still empty. Fix the workspace, run → clean, entry appears.
   *Discrimination:* temporarily cache the `Err`, watch the second half fail.
6. **Cold-vs-warm byte identity** — the new merge-gate form. Run `run_analyze_with_exit`
   twice on a fixture with a **real opaque dep** (so `opaque_apps` is non-empty and
   flows into the JSON coverage block), cold then warm → assert stdout bytes, exit code
   and stderr warning all identical. This is what pins §3's output-bearing consequence.
7. **CDO ratchets run cache-disabled.** `scripts/cdo-gate` exports the no-cache escape
   hatch. Without this the north-star measurement can silently measure a warm replay.

---

## §6 — Follow-up, now unblocked by Q2

The **dependency parse-artifact cache** — the lever that helps the *edit* loop, worth
~1.1 s + 142 ms on **every** DO run rather than only identical-input reruns. Q2 sets
its boundary: **persist the pure parse/declaration products** (per-dep extracted
`ObjectNode`/`RoutineNode`, per-dep `recovered_files` paths), **never dep-internal
edges**, which are not a pure function of dep content.

Two things it still needs, neither blocking this spec:

- App identity serialized **symbolically** (guid), re-interned on load — `AppRef` is a
  per-run index.
- A measured artifact size on DO before committing to a format. Measure the population
  before building taxonomy for it.

Do **not** persist raw `al_syntax::ir`: it has zero serde derives today (a whole-crate
serialization surface plus schema-versioning burden), parse is cheap per byte
(0.098 ms/file, parallel), and serialized IR for ~114 MB of dep text would plausibly
cost as much to deserialize as to re-parse — the "cache that costs more to load than to
recompute" failure. Persist the smaller derived products instead.

*Note for whoever writes that spec:* `Origin.ts_id` carries a doc saying "NEVER
serialize… tree-sitter recycles ids", naming L2 op/callsite maps as its consumer. That
consumer no longer exists — `ts_id` is written once (`lower/mod.rs:1909`) and read by
**zero** production code. The stated blocker is guarding a dead field; delete it rather
than designing around it. `ParseStatus::Recovered`'s own doc already carries the right
rule: *"the IR is partial; do not cache as authoritative."*

**Rejected alternative, named so it is a considered rejection rather than a blind
spot:** a resident daemon / watch mode keeps the model in memory and sidesteps
serialization entirely — and `lsp::updater` already IS that shape, reusing
`Arc<DepLayer>` across rebuilds. Plausibly the right long-run answer for the edit loop;
out of scope for a CLI-invocation cache.

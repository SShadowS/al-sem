# Compact graph core: build profiles, a lighter LSP graph, and alsem on the program engine

Status: DESIGN, reviewed section by section with the user on 2026-10-04; revised the same day
for an independent review by gpt-6.1-sol (verdict "Not ready", 2 critical + 6 important
findings, all accepted; the review is in the session scratchpad, `spec-review-gpt61sol.md`).
Evidence: the byte census (`census-report.md`, probe in the session scratchpad, phase hook on
branch `census/graph-bytes` @ `37beb3f1`) and the field-usage map (`usage-map.md`), both from
2026-10-04 on `master` @ `0b760407`. Related: `docs/2026-10-04-dependency-sharing-prior-art.md`,
`docs/superpowers/specs/2026-08-07-dependency-pack-cache-design.md` (the "pack spec").

## §1 — Goal

1. Make the LSP server light enough for the CentralGauge harness: a 3 GB Hyper-V container
   that runs `al-call-hierarchy` for 7 workspace roots next to the AL language server
   (2.2-2.4 GB).
2. Move `alsem`'s detectors off the retired L3 model onto the program engine (backlog item
   "B3"), so there is one engine and the full graph is proven in use.

## §2 — The user's rules (binding)

- **Best solution, not quickest.** Fix root causes.
- **Never delete information `alsem` or `aldump` might use later.** Every memory saving is a
  choice of VIEW (§4), never a deletion. The light view may skip a fact; the full view keeps
  it or builds it on request.
- **The full view may still peak high for now.** It only has to keep every possibility
  available. Its memory is a separate, later effort.
- **Findings may change only where the change is shown to be correct** (§7).

## §3 — What the census measured (the case for the order)

At 7 CG roots, default (`embedded`) mode:

- **The peak is the problem.** The first root holds every dependency's syntax trees at once:
  804 MiB (CDO: 915 MiB). Process peak: 1,121 MiB (CDO 1,324 MiB). That is more than all 7
  roots keep afterwards together (360 MiB). `alsem analyze` peaks at 1,335 MiB on CDO for the
  same reason (`fresh_coverage`).
- **Kept memory:** a shared tier of 234 MiB, plus 18 MiB per extra root. 86% of the per-root
  part is event links that are byte-identical in every root; 92% of those links have no
  subscriber.
- **Duplication:** 801,553 copies of 112,238 distinct `RoutineNodeId`s (96.6 MiB); strings are
  80-89% duplicates; CDO's `dep_meta` is 43% empty hash slots (40 MiB).
- **The old L3 model is small** (90 MiB on CDO, plus 55 MiB of detector context) and never
  lives at the same time as the program graph. B3 is about correct results and one parse,
  not about memory on its own.
- **Why B3 matters for results:** on CDO the L3 resolver leaves 656 calls unresolved and
  treats 654 dependency calls as dead ends; the program engine has 0 unknown. Every
  call-following detector sees less than it could.
- **Not yet measured (review finding, gpt-6.1-sol, 2026-10-04):** the census built snapshots
  but did not run the LSP updater. Each root's updater builds a `Rung1Context` (a whole-graph
  `ResolveIndex`, `DeclSurface` and object map) BEFORE waiting for its first edit, and holds
  it while idle (`src/lsp/updater.rs:1198`). The census measured comparable allocations as
  ~57 MiB of transient per extra root; in a running server they are RETAINED, per root. The
  real retained figure is therefore higher than 360 MiB. Step 0 measures it (§9).
- **Heap versus process memory:** the census counts requested heap bytes. It excludes
  allocator overhead (~24 B per allocation, reported separately) and RSS. Targets in §11 are
  heap targets; the container check is a separate RSS reading.

## §4 — Build profiles (cross-cutting)

Each tool **declares what it needs**; the build makes the union and skips the rest. This is
the same model the detectors already use (`Detector::requires`, folded by `run_detectors`).

- `BuildProfile` is a struct with one field per optional fact. Each step below adds its field.
- Two named presets: `FULL` (everything; today's behaviour exactly) and `LIGHT` (the LSP
  server's needs).
- **No default.** Every call site states its profile.
- **Light is a strict subset of full**: whatever a smaller profile keeps is identical to what
  `FULL` keeps. A test builds both on the same workspaces and compares the shared parts.
- **The profile is part of the shared-cache key** (`DepKey`) wherever it changes the shared
  dependency tier.
- **Reading a part that was not built fails loudly in debug builds and tests** (the pattern of
  the 2026-10-04 ordering-facts fix), and a test compares each tool's declared profile against
  `FULL` output. This guards against a tool that forgets a declaration.
- An internal fixed metric may declare a smaller profile, but only for the facts it provably
  does not read. `fresh_coverage` (`alsem`'s preflight count) does NOT read dependency bodies,
  so it uses `dependency_bodies: Summary`. It DOES read edge details: it runs the full report
  (`src/program/resolve/full.rs:1370`), and classification and the histogram read evidence,
  dispatch shape and conditions (`edge.rs:552-562`, `full.rs:1415-1434`). So it declares
  `edge_details`. More generally, edge classification always runs on the complete edge,
  BEFORE any profile projection drops details.

| Field (step) | `LIGHT` | `FULL` |
|---|---|---|
| `dependency_bodies` (§5) | `Summary` | `Keep` |
| (dropped 2026-10-04) `empty_event_edges` — not a profile field: the LSP snapshot, the only light view, never stores links without routes (§6 2b); the program report always keeps them | — | — |
| `edge_details` (§8) | not stored | stored |
| `dependency_source_text` (§8) | line positions only, if no consumer shows dependency source | stored |

## §5 — Step 1: dependency parsing without the spike

**Finding.** Dependency syntax trees are read in exactly two places, both one file at a time:
node extraction (`build_dep_nodes`, `src/program/build.rs`) and `RoutineMeta` for the
`DeclSurface` frozen tier (`DeclSurface::build`/`build_split`). The resolver resolves only
workspace files (`resolve_full_program_from_parts`, Phase 1). Yet `ProgramContext.parsed`
keeps every unit until the context dies.

**Design.**
- In the parallel parse, each dependency file is parsed, turned at once into its per-file
  summary, and its syntax tree dropped. The summary is the pack spec's `PackedFile`
  (`src/program/pack/mod.rs`): virtual path, `parse_status_recovered`, the file's
  `ObjectNode`s and `RoutineNode`s (pre-dedup, extraction order), and its
  `(RoutineNodeId, RoutineMeta)` pairs. No second summary format is invented.
- `dependency_bodies: Summary` keeps only the summaries. `Keep` keeps the summaries AND the
  full `ParsedUnit`s, so any future analysis can walk dependency code.
- Both profiles build the dependency layer and `DeclSurface` from the summaries, so both take
  ONE code path to the same graph.
- **Parse-recovery status gets a lasting home.** Before a summary is consumed, its
  `parse_status_recovered` flag moves into a recovered-paths list held by the dependency tier
  (shared, so a cache hit sees it too). `recovered_file_paths` (pack spec §11.2) returns that
  list combined with the workspace's own recovered files. Its absence-proof invariant is
  unchanged, and the summaries themselves are not kept.
- `build_dep_texts` (`src/lsp/snapshot.rs:1046`) also reads dependency `ParsedUnit`s, for
  paths and texts, not syntax trees. It is rewired to read the snapshot's source files
  (the same `Arc<str>`s), so it no longer depends on `parsed`.
- **Summary ownership.** Summaries are CONSUMED: their nodes move into the dependency layer
  and their `RoutineMeta` moves into `dep_meta`, then the summary is gone. They are never
  kept alongside the nodes they produced (that would erase the saving). Today's dedup clones
  survivors (`build.rs:631-669`); the new path moves them instead. An allocation test pins
  that nothing summary-shaped survives the build.
- **`Keep` on a shared-cache hit.** Today a published shared tier makes later roots parse only
  the workspace (`full.rs:1166-1177`), and the tier stores metadata and texts, not bodies. A
  `Keep` build that hits would get no dependency bodies. Rule: a tier built with
  `dependency_bodies: Keep` holds the bodies itself (shared, immutable `Arc`s), and the
  profile is part of `DepKey`, so a `Keep` request never hits a tier built without bodies.
  Tests cover LIGHT→FULL, FULL→FULL, FULL→LIGHT and concurrent builds of the same key.
- The workspace's own syntax trees are always kept.
- Tools that read dependency syntax trees today (`resolve/differential.rs`,
  `resolve/semantic_golden.rs`, `dep_cache.rs` tests) either read summaries or declare `Keep`.
- **Out of scope:** the pack spec's light snapshot (§10) and on-disk packs. Step 1 still
  loads dependency source, so the pack spec's §11.3 risk ("no source" read as "symbol-only")
  does not arise. Persisting summaries to disk stays the pack spec's own later work; step 1
  makes the in-memory summary the single route it plugs into.

**Checks.**
1. Byte-identical graph, edges, LSP snapshot and goldens under both settings, on CDO and every
   fixture. Because both settings share the new path, agreement alone cannot catch a
   regression in that path: each check also gets a mutation-based discrimination proof
   (break the summary, watch it fail), and the new path is compared against today's
   `master` output, not only against itself.
2. A test proves `Summary` frees trees as it goes: live dependency syntax trees during the
   build stay bounded by about one per worker thread.
3. Probe re-run. Target for THIS step: the first root's build peak loses the co-resident
   dependency syntax trees (804 MiB on CG, 915 MiB on CDO, minus about one file per worker
   thread); `alsem`'s `fresh_coverage` peak falls by a similar amount. The 7-root process
   peak this step reaches is recorded, not targeted: it also includes the updater indexes
   (§3), which step 2 shares. Step 0 sets the exact number.

## §6 — Step 2: lighter per-root state (revised 2026-10-04)

**Revision.** A code map made after step 1 (on master @ d7a16486) changed the order of this
step; the user approved it on 2026-10-04. Findings:
- 92% of each root's event links have no route (CG root 1: 2,033 of 24,527), and no LSP reader
  uses a link without routes. The LSP snapshot can leave them out with no sharing machinery.
  `alsem`/`aldump` read event links from the program report, not the LSP snapshot, so nothing is
  lost (spec §2): the LSP snapshot is the light view by definition.
- The ~57 MiB per idle updater is essentially `ResolveIndex` (most likely
  `routines_by_obj_name`, one entry per routine). `obj_node_map` is read only for workspace ids
  (`full.rs:660-670`), so it needs no dependency part. `DeclSurface`'s dependency part is
  already the shared `dep_meta`.
- Rung 1 never reads `ResolveIndex`'s subscriber maps (only `emit_event_flow_edges` does, and
  rung 1 forwards event links). The idle `Rung1Context` carries them for nothing.
- Hazard the original design missed: a dependency whose manifest depends on the WORKSPACE app
  gets a topology edge to `AppRef(0)` (`build.rs:410-431`), which `DepKey` does not capture, so
  its "dependency-only" event links can differ between roots. Any sharing needs a guard.

**Order (smallest, provably-identical changes first):**
- **2a.** Build `obj_node_map` from workspace objects only, at every build site. No behaviour
  change by construction.
- **2b.** The LSP snapshot stores only event links with at least one route. Every LSP answer
  must stay identical; the backstop compares ANSWERS (non-empty links, `incoming`,
  `publisher_fanout`), not raw storage.
- **2c.** Make the parts of `Rung1Context` measurable, then remove the per-routine map (direct
  lookups over the already-sorted routine list) or share it, whichever the measurement favours;
  and stop keeping the subscriber maps in the idle `Rung1Context`. Rung 1 and rung 2 answers
  must stay identical.
- **2d (conditional).** Only if 2a-2c leave enough on the table at 7 roots: share the remaining
  dependency event links and index between roots, as designed below, with the topology guard
  (a dependency that depends on the workspace app disables sharing for that tier).

The original design follows; it now applies to 2d only.

**Finding.** Dependency-to-dependency event links are byte-identical in all 7 roots; only 0-2
links per root involve the workspace. Each root rebuilds all of them (`emit_event_flow_edges`,
plus `incoming`/`publisher_fanout` keys).

**Design (2d).**
- **Shared part:** links whose publisher and subscribers are all in dependencies, computed once
  per shared dependency set and stored in the shared tier next to `dep_meta` (same
  build-on-first-use slot).
- **Per-root part:** links with a workspace publisher; **added subscribers**, where a workspace
  routine subscribes to a dependency event (kept as "publisher P also reaches these workspace
  routines", never written into the shared link); and the per-root synthetic platform
  publishers, unless they prove shareable.
- `event_edges`, `incoming` and `publisher_fanout` are read through one view ("shared plus this
  root's additions"), so handlers do not see two parts.
- Links with no route: already handled by 2b (the LSP snapshot never stores them; the
  program report, which `alsem`/`aldump` use, keeps them and counts them as `honestEmpty`).

**Composition contract** (review finding 5). Today event references index one flat vector
(`src/lsp/snapshot.rs:719-725, 848-858`), subscriber routes are globally sorted
(`index.rs:405-408`), and synthetic platform publishers are generated from ALL subscribers
after merging and appended to the root's own tier, even when the object is a dependency's
(`build.rs:446-526`). So:
- **Tier-tagged references.** An event reference names its tier (shared or root) plus a
  position within it. Adding or removing a root-owned link never shifts a shared reference.
- **Publisher lookup does not depend on stored empty links.** Per-root additions are computed
  from the subscriber index against the publisher's identity, not by finding a stored link.
  So under `LIGHT` a workspace subscription to a dependency publisher that has no stored link
  (it had no dependency subscriber) still produces its link.
- **Ordered merge.** A publisher's combined routes are the shared routes merged with the root's
  added routes in the same global order today's sort produces.
- **Synthetic platform publishers.** Publishers synthesized from dependency subscribers belong
  to the shared tier. Root-owned synthesis supplies ONLY publishers absent from the shared
  tier. When a platform event already has a shared synthetic publisher, a workspace
  subscription to it reuses that publisher (a root-owned added route), never a second
  root-owned publisher and never a relocated shared one, so subscription resolution never
  sees duplicate candidates. Tested with a platform event that has both dependency and
  workspace subscribers, and with one that has only a workspace subscriber.
- The CG measurement (identical across 7 roots) is evidence, not proof. The equality check
  is the proof, and it must include roots whose subscriptions change.

**The updater's indexes** (review finding 1). Each root's updater holds a whole-graph
`ResolveIndex`, `DeclSurface` and object map while idle. Their dependency part is the same in
every root that shares a tier. This step shares the dependency part (built once per tier, next
to `dep_meta`) and keeps only a workspace overlay per root. This was "lever 7" in the census,
priced there as transient; it is moved into this step because in a running server it is
retained.

**Checks (2a-2c: each change's LSP answers equal the previous commit's on CDO and every
fixture, rung 1/2/3 answers unchanged, probe re-run with updaters after each; 2d: as below).**
1. The combined view equals today's per-root result, link for link, on CDO, every fixture,
   and a multi-root test where roots subscribe to dependency events. Rung-2 tests add and
   remove a workspace subscription to a dependency event (including one with no dependency
   subscriber) while an OLD snapshot is still alive, and check both snapshots answer
   correctly. Rung-1 and rung-2 answers with the shared updater indexes equal today's.
2. Every LSP answer (call hierarchy, code lens, diagnostics, custom requests) is identical
   under both profiles.
3. Probe re-run. Estimates, snapshot-only (they exclude the updater indexes): each extra
   root's snapshot from about 18 MiB to about 2.5 MiB, roughly 100 MiB less at 7 roots. The
   updater-index saving is priced separately by step 0. Total per-root server memory is
   measured with the updaters running, as in step 0.

## §7 — Step 3: B3, the detectors on the program engine

**Finding.** `alsem analyze` runs `fresh_coverage` (program engine) only for the preflight
count, drops it, then builds the L3 model (`assemble_and_resolve_workspace`,
`src/engine/gate/run.rs`) that the detectors read. The L3 model is workspace-only and resolves
calls with the retired resolver. Detectors read two things from it: **body facts** (L2's walk
over the same `al-syntax` trees: statement trees, loops, record operations, variables,
conditions) and **the call graph** (L3's resolution). The call graph is the weak part.

**Design: approach C (chosen).**
- **Phase A, call graph swap.** L2 body facts are built from the program engine's
  already-parsed workspace syntax trees (no second parse). L3's call resolution is replaced by
  the program engine's edges, matched to each call site by exact source position. Detector
  code is unchanged.
- **Phase B, gradual.** The detector support layers (summaries, cones, d1's data flow) move
  onto program-graph ids one at a time, each with its own comparison.
- **Phase C, following calls into dependencies (a new capability, not a by-product).**
  Keeping dependency bodies (`FULL`) does NOT by itself let a detector follow a call inside a
  dependency: the program engine resolves only workspace files (`full.rs:805-813`), and cones
  and the combined graph take their nodes and facts from the modelled routine population
  (`detector_context.rs:913-943`, `combined_graph.rs:349-351`). So workspace `A` → dependency
  `B` → dependency `C` (which commits) still stops at `B`. Phase C adds: resolving dependency
  bodies (scope: routines reachable from the workspace), projecting their body facts for the
  detectors, handling recovered and bodyless (symbol-only) routines, and scoping findings that
  land in dependency code. It gets its own difference run. Phases A and B claim only better
  resolution of WORKSPACE calls.
- **After the switch** `alsem analyze` no longer builds the L3 model. Deleting the L3 code is
  a separate, later step, once the harness shows nothing reads it.

**The difference harness (built first).**
- Runs every detector on both paths over CDO, DO, the 8020 synthetic corpus and all fixtures,
  and compares findings by detector, location and root cause.
- Classifies each difference: **fixed** (new engine right, checked against real source with
  the `triage-findings` practice; sampling where a class is large and uniform),
  **regression**, or **unexplained**.
- **Switching bar: zero unexplained, zero regressions.** Then goldens are regenerated with the
  triage as their written evidence.

**Main risk: the adapter.** The engines identify routines and sites differently (L3 string
ids versus `RoutineNodeId`; 15,529 L3 call sites versus 20,707 program edges on CDO, which also
include event and trigger links). Position matching alone is not enough (review finding 3).
The Phase A adapter's contract:
- **Normalized site identity:** file + routine + byte span. L2 anchors use UTF-16 columns
  (`src/engine/l2/ir_walk.rs:279-294`); program sites use byte columns
  (`src/program/resolve/extract.rs:637-657`). One conversion, tested on non-ASCII source.
- **Route conversion:** one program edge can carry several routes (one-to-many); define how
  each maps to L3's call-site resolution, including ambiguous and conditional routes.
- **Operation sites:** L2 keeps record operations and `Commit` out of `PCallSite`
  (`ir_walk.rs:907-956`); program resolution emits obligations for them. Map them to the
  operation-site facts the detectors read, not to call sites.
- **Argument bindings:** the detector context rebuilds legacy calls AND events
  (`detector_context.rs:886-900`) and reads upgraded argument bindings
  (`detector_context.rs:1310-1320`), which use the callee's parameter var-ness
  (`call_resolver.rs:228-258`). The adapter must supply the callee's parameters for every
  newly resolved call (dependency callees included), or a correctly resolved call still
  leaves its record argument "unresolved-callee" (d37/d39 and parameter roles stay wrong).
- **Events:** event links translate into the detector context's event graph by the same
  rules.
- **Tests:** Unicode identifiers and text, nested calls on one line, implicit triggers,
  manual subscriptions, and a count of every unmatched site by reason (never dropped).

## §8 — Step 4: the compact graph

These lose no information, so both profiles get them, except the last two items.

1. **Numeric ids per tier.** A `u32` per routine and object instead of 96-byte
   `RoutineNodeId` copies in edges, map keys and tables. Dependency numbers are fixed per
   shared tier; workspace numbers are minted per root build (rung 2 reindexes the workspace,
   so merged `NodeSet` positions are not stable). `RoutineNodeId` stays the canonical identity
   for printing and export. Also replaces the resolver's linear
   `graph.objects.iter().find(...)` scans. Rules (review finding 6):
   - **Row numbers, not canonical numbers.** A number names a physical row. Canonical ids can
     collide on purpose (`ResolveIndex` keeps physical rows apart, `index.rs:209-225`;
     `NodeSet` allows shared/own ties, `node_set.rs:104-107`), and one number per canonical
     id would merge an aliased publisher pair and defeat the dual-publisher skip guard.
   - **The tier is part of the number** (or carried beside it), so a dependency row and a
     workspace row can never be confused.
   - **Checked limits:** building fails loudly if a tier exceeds the number range.
   - **Generation ownership:** workspace numbers belong to one root build. They never leave
     the process: LSP items keep serializing canonical identity (`handlers.rs:397-406`), so a
     call-hierarchy item from before a rung-2 rebuild still names the same routine after it.
2. **Strings stored once:** names, type texts, paths and event names in a string table
   (shared tier part plus a small per-root part). Output text unchanged.
3. **Compact node data:** `dep_meta` becomes columns indexed by routine number (removes the
   hash map, its empty slots and the repeated id and name); optional and boolean node fields
   are packed.
4. **`edge_details` (profile):** witness, evidence, conditions and the repeated caller copies,
   kept by `FULL`, not stored by `LIGHT`.
5. **`dependency_source_text` (profile):** in `LIGHT`, per-routine line positions replace the
   ~110 MiB of text, **only if** a check shows nothing in the LSP server displays dependency
   source (for example an `al-preview://` view). Otherwise the text stays. `FULL` keeps it.

Order: 1, then 2, then 3, then 4-5. Each item is priced by the probe right before it is built.
No fixed saving is promised here, because steps 1 and 2 change the base.

## §9 — How the effort runs

- **Step 0 (before step 1): measure the running server.** Extend the census probe to start
  the real updater for each root (as `src/server.rs:597-629` does) and measure the retained
  heap with every root idle, at 7 CG roots in both modes, plus one RSS reading. This fixes the
  true baseline for §11 and prices the shared updater indexes in §6.
- Each step gets its own plan, built task by task with reviews and `scripts/ci-steps task`;
  `ci-steps all` plus `scripts/cdo-gate` once per step before merging.
- The census probe and the phase hook are re-run before and after each step on CG (7 roots,
  both modes) and CDO. The phase hook moves into `master` as a no-op so the probe keeps
  working. The measurement-auditor agent checks every number before it is frozen in the
  CHANGELOG.
- Held throughout: real-unknown rate 0; `FULL` output identical to today; goldens move only in
  step 3, with written triage.
- Steps 1 and 2 can ship on their own. Whether they are ENOUGH for the 3 GB container is
  decided by the harness gate in §11, not assumed.
- The first commit fixes CLAUDE.md's wrong claim that L4/L5 consume the program engine, and
  adds the profile rule.

## §10 — Risks

1. The B3 adapter (§7): site identity, routes, operation sites, argument bindings, events.
2. A tool that forgets to declare a need (§4 guards).
3. Numeric ids going stale across a rung-2 rebuild (§8.1: workspace numbers per root build).
4. Step 1's summaries diverging from today's extraction (one code path for both settings,
   byte-identical checks).

## §11 — Success criteria

- `LIGHT`, 7 CG roots, `embedded`, counted heap: build peak about 425 MiB (from 1,121 MiB).
  This is the BUILD peak of root 1. The process peak in server order (updaters starting
  between builds) is unmeasured and is at least the idle retained figure below.
- Retained heap with the server idle before any edit (updaters included), 7 CG roots,
  `embedded`: measured at step 0 (`docs/2026-10-04-step0-server-census.md`) as **762.7 MiB**
  today (snapshots 360.5 + updaters 402.2, i.e. 57.45 MiB per root). Retained heap after
  edits is not measured. The goal is the snapshots plus ONE shared copy of the updater
  indexes (consistent with whole-graph indexes dominated by dependency entries; inferred, not
  split), not seven: about 360.5 + 57.5 = 418 MiB at best on today's snapshots (a floor; each
  extra root keeps its own workspace part, so about **422 MiB** estimated; the Light graph
  lowers the snapshot part further). `symbols`: 375.5 MiB today, at least about 181 MiB with
  one shared copy.
- **Container acceptance (the real pass criterion):** heap figures do not prove
  deployability. Steps 1 and 2 count as done for the container only when the CentralGauge
  harness's own gate passes: a signed release, a full 7-root cell at 3 GB next to the AL
  language server, zero "memory allocation ... failed" lines, over the harness's standard
  number of cold runs. RSS is reported alongside as context.
- `alsem` findings come from the program engine; every difference from today is triaged.
- Real-unknown rate stays 0; all gates green at every step; `FULL` keeps every fact.

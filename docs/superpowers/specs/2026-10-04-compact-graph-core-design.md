# Compact graph core: build profiles, a lighter LSP graph, and alsem on the program engine

Status: DESIGN, reviewed section by section with the user on 2026-10-04.
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
- An internal fixed metric that provably reads nothing optional may use a smaller profile.
  Today that is `fresh_coverage` (`alsem`'s preflight count).

| Field (step) | `LIGHT` | `FULL` |
|---|---|---|
| `dependency_bodies` (§5) | `Summary` | `Keep` |
| `empty_event_edges` (§6) | not stored | stored |
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
- `recovered_file_paths` reads the summaries' flag (pack spec §11.2). Its absence-proof
  invariant is unchanged.
- The workspace's own syntax trees are always kept.
- Tools that read dependency syntax trees today (`resolve/differential.rs`,
  `resolve/semantic_golden.rs`, `dep_cache.rs` tests) either read summaries or declare `Keep`.
- **Out of scope:** the pack spec's light snapshot (§10) and on-disk packs. Step 1 still
  loads dependency source, so the pack spec's §11.3 risk ("no source" read as "symbol-only")
  does not arise. Persisting summaries to disk stays the pack spec's own later work; step 1
  makes the in-memory summary the single route it plugs into.

**Checks.**
1. Byte-identical graph, edges, LSP snapshot and goldens under both settings, on CDO and every
   fixture.
2. A test proves `Summary` frees trees as it goes: live dependency syntax trees during the
   build stay bounded by about one per worker thread.
3. Probe re-run. Target: light peak at 7 CG roots from 1,121 MiB to about 425 MiB; `alsem`'s
   `fresh_coverage` peak falls by a similar amount.

## §6 — Step 2: share event links between roots

**Finding.** Dependency-to-dependency event links are byte-identical in all 7 roots; only 0-2
links per root involve the workspace. Each root rebuilds all of them (`emit_event_flow_edges`,
plus `incoming`/`publisher_fanout` keys).

**Design.**
- **Shared part:** links whose publisher and subscribers are all in dependencies, computed once
  per shared dependency set and stored in the shared tier next to `dep_meta` (same
  build-on-first-use slot).
- **Per-root part:** links with a workspace publisher; **added subscribers**, where a workspace
  routine subscribes to a dependency event (kept as "publisher P also reaches these workspace
  routines", never written into the shared link); and the per-root synthetic platform
  publishers, unless they prove shareable.
- `event_edges`, `incoming` and `publisher_fanout` are read through one view ("shared plus this
  root's additions"), so handlers do not see two parts.
- `empty_event_edges`: `FULL` stores links with no subscriber (the CLI counts them as
  `honestEmpty`); `LIGHT` does not. LSP answers must be identical either way.

**Checks.**
1. The combined view equals today's per-root result, link for link, on CDO, every fixture,
   and a multi-root test where roots subscribe to dependency events.
2. Every LSP answer (call hierarchy, code lens, diagnostics, custom requests) is identical
   under both profiles.
3. Probe re-run. Target: each extra root from about 18 MiB to about 2.5 MiB, roughly 100 MiB
   less at 7 roots, so 7 roots keep under about 260 MiB.

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
  onto program-graph ids one at a time, each with its own comparison. Detectors that follow
  calls into dependency code use `FULL`'s dependency bodies.
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

**Main risk: matching.** The engines identify routines and sites differently (L3 string ids
versus `RoutineNodeId`; 15,529 L3 call sites versus 20,707 program edges on CDO, which also
include event and trigger links). Matching is by source position; every unmatched site is
explained, never dropped.

## §8 — Step 4: the compact graph

These lose no information, so both profiles get them, except the last two items.

1. **Numeric ids per tier.** A `u32` per routine and object instead of 96-byte
   `RoutineNodeId` copies in edges, map keys and tables. Dependency numbers are fixed per
   shared tier; workspace numbers are minted per root build (rung 2 reindexes the workspace,
   so merged `NodeSet` positions are not stable). `RoutineNodeId` stays the canonical identity
   for printing and export. Also replaces the resolver's linear
   `graph.objects.iter().find(...)` scans.
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

- Each step gets its own plan, built task by task with reviews and `scripts/ci-steps task`;
  `ci-steps all` plus `scripts/cdo-gate` once per step before merging.
- The census probe and the phase hook are re-run before and after each step on CG (7 roots,
  both modes) and CDO. The phase hook moves into `master` as a no-op so the probe keeps
  working. The measurement-auditor agent checks every number before it is frozen in the
  CHANGELOG.
- Held throughout: real-unknown rate 0; `FULL` output identical to today; goldens move only in
  step 3, with written triage.
- Steps 1 and 2 can ship on their own (they are what the 3 GB container needs).
- The first commit fixes CLAUDE.md's wrong claim that L4/L5 consume the program engine, and
  adds the profile rule.

## §10 — Risks

1. Matching L3 call sites to program edges (§7).
2. A tool that forgets to declare a need (§4 guards).
3. Numeric ids going stale across a rung-2 rebuild (§8.1: workspace numbers per root build).
4. Step 1's summaries diverging from today's extraction (one code path for both settings,
   byte-identical checks).

## §11 — Success criteria

- `LIGHT`, 7 CG roots, `embedded`: process peak about 425 MiB (from 1,121 MiB); kept under
  about 260 MiB (from 360 MiB).
- `alsem` findings come from the program engine; every difference from today is triaged.
- Real-unknown rate stays 0; all gates green at every step; `FULL` keeps every fact.

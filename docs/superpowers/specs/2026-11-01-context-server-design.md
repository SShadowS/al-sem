# A resident context server for al-sem (design)

Status: **final design, revision 7** (2026-11-01). Branch `spec/context-server`, rebased
onto master `c5dbed28`. The design is the result of a measured requirements spec and six
review rounds; revision 7 applies the two design changes of the approving round and
states the contracts for the three landed-code fixes it asked for.

## Review history

| Round | Who | Outcome |
|---|---|---|
| Requirements spec (revision 0) | spec author | measured the cold and warm costs on CDO; recommended one shared session core with two thin front ends (option C) |
| Fable r1 | Claude (Fable) | child process per workspace; per-revision shared cells; incrementality in stages; output-size policy |
| astra round 1 | GPT (astra) | reject: view ownership, dependency roots, process globals, dependency-body validity, weak cache, stamp completeness, routine ids, wire framing, memory, protocol, tests |
| Fable r2 | Claude | phase functions; cone store; request-scoped planning; manifest; owned child protocol |
| astra round 2 | GPT | reject: core summaries, whole-program refinement, query log, attach mutations, file keys, cut-off, producers, subject-only trees, acquisition, retention, pack, protocol, outputs, tests, targets, plan order |
| Fable r3 | Claude | conservative dependency rule; raw/final rows; edge-transfer validity; Δ_pd_output; symbolic app keys; owned images |
| astra round 3 | GPT | reject: classifier completeness, anchor join, demand, attach inputs, cold order, final passes, edge-transfer, producers, store roots, acquisition, memory floor, failed-build branch, directory safety, attributions, tests |
| Fable r4 | Claude | complete declaration projection; locations split; whole demand and adapter; ABI ordinals; arena canonicalization; artifact snapshots |
| astra round 4 | GPT | reject: projection definition, location-bearing products, demand population, adapter cache, ABI identity, tree identities, arena remap, snapshot lifecycle, mapping, reservation floor, frames, bootstrap, cost gates, pack savings |
| Fable r5 | Claude | `Classify` trait; whole L2c/L7; two-way manifest check as three-way; reserve-or-fail; frame bounding at both hops |
| astra round 5 | GPT | reject, narrower; stage 4 cleared: tree-section inputs, ready snapshot, total arena ranks, location domains, private namespace |
| Fable r6 | Claude | whole tree sections; `ensure_prepared`; total internal-id ranks; two location domains; private-namespace contract |
| astra round 6 | GPT | **approve with changes**: dependency-aware `ensure_prepared` with a complete publication transaction; publisher-only zero-span fallback; three landed-code bugs (lossless stderr, frame recovery at the boundary, bounded front-end queue) |
| Fable r7 = this document | Claude | applies the two design changes and states the three contracts |

**Landed on master `66ff3d93`:** stage 0 (`PlanCx`, wire codec, explain grouping,
multi-root explain, `BoundaryMode`, prove spans), stage 1 (phase split of `View::prepare`
with owned/borrowed inputs, measurement spans), stage 1b (canonical arena order, internal-id
ranks), the routine-id migration (per-app id prefix), the `Cu.Run(x)` resolver fix, the
stage-2 plumbing (owned package images, the engine child and its front end, hidden
`alsem engine-host` / `alsem engine-ask`, `engine-test-hooks`) with the round-6 fixes
(lossless stderr, boundary-safe frame recovery, bounded front-end queue), the ABI-integrity
check as a declared need (`AbiIntegrity::{Check, Skip}`), and **stage 4** (the incremental
model, `program::session`; see the as-built notes in §6.5, §6.13, §7 and §12). **Landed
since:** the stage-2 completion (the resident child, §3's as-built note), **stage 3**
(admission, reservation, eviction, idle children, lanes, MCP wiring; §7's as-built note) and
**`--low-mem`** (§7, owner decision 2026-11-02) and the **LSP front end** (§9's as-built
note). Everything else below is the build order
from here.

## 0. Summary

AI agents (Claude Code and others) want to ask al-sem small questions about one routine
many times in a session. Today every question is a fresh process that rebuilds the whole
model: on the CDO baseline 5–9 s and 0.75–0.9 GB per question single-app, 14–29 s and
3.4 GB cross-app (the default). Most of that is the model build, which is the same for
every question about the same workspace revision.

The design is one engine-side **session core** that keeps a built **revision** of a
workspace and updates it per change, plus thin front ends: an MCP server over stdio for
agents and, later, custom methods on the existing LSP server for editors.

- The engine runs in a **child process** per workspace and universe, owned by the front
  end, because the shipped binaries abort on panic (`release-ship`); a crash kills the
  child, never the protocol endpoint. The child is the `alsem` binary (mimalloc with
  immediate purge, the CLI's clap).
- A revision is **immutable**; requests never mutate it. Its analysis artifacts live in
  immutable **artifact snapshots** that requests pin; fills and evictions publish new
  snapshots. A request obtains a **ready** snapshot holding everything it needs before it
  computes (§4.2).
- After an edit the revision is **updated, not rebuilt**: files are classified by a
  complete declaration projection; parse and resolution are per file; every whole-program
  step that would need a fragile cache key is simply rerun whole, with its measured cost
  (demand 0.15 s, adapter 0.3 s, finishing 0.3–0.4 s, final passes 1.3–1.6 s today);
  the L4 solve is seeded; the capability cone and the snapshot sections are retained
  with explicit input-validity rules (§6).
- Every answer is the CLI's **exact bytes** plus a revision id; "fresh" means the
  manifest observed before acquisition, the bytes actually consumed, and the manifest
  observed after the build name the same inputs (§5).
- Routine ids are per app and stable across file adds and removes (landed).
- Explain shares one tree across its roots and takes several roots (landed).

## 1. Measurements

Every number below was measured on the CDO baseline (`DO-cdo-baseline/Cloud`, detached
at `bc3ccb18`, 579 workspace `.al` files, 11 dependency `.app`s), release-fast builds, on
a shared Linux machine (treat ±10 % as noise). Numbers from the requirements spec are
kept where still valid; later rounds added the rest.

### 1.1 What one question costs today (one process per question)

| Command | Single-app wall | Single-app peak RSS | Cross-app wall | Cross-app peak RSS | JSON out |
|---|---:|---:|---:|---:|---:|
| `explain --routine IssueReminder` | 5.71 s | 752 MB | 27.47 s | 3,399 MB | 157 KB / 308 KB |
| `explain --diff` (2 routines) | 5.94 s | 888 MB | 26.85 s | 3,382 MB | 314 KB / 616 KB |
| `explain --table "CDO Queue Entry" --field "Language Code"` | 5.68 s | 768 MB | 23.35 s | 3,387 MB | 532 KB |
| `prove IssueReminder may-commit` | 8.95 s | 767 MB | 29.08 s | 3,350 MB | 23 KB / 2 KB |
| `digest --routine IssueReminder` | 6.24 s | 758 MB | 28.07 s | 3,368 MB | 2.8 MB / 5.6 MB |
| `digest --diff` | 6.17 s | 878 MB | 27.97 s | 3,371 MB | 5.6 MB / 11.2 MB |
| `query effects --routine IssueReminder` | 5.37 s | 765 MB | 14.62 s | 3,364 MB | 626 KB / 670 KB |
| `query touches --table "CDO Queue Entry"` | 5.15 s | 761 MB | 14.36 s | 3,350 MB | 440 KB |

A digest is 2.8–11 MB of JSON, an explain card 157–308 KB, a query 440–670 KB: none of
these fit an agent's context inline (§8.3).

### 1.2 Where the time goes, cross-app (one process)

| Stage | ms |
|---|---:|
| snapshot build (read files, identify apps) | 310–340 |
| parse | 1,230–1,375 |
| dependency layer | 980–1,170 |
| assemble graph | 310–335 |
| resolve workspace bodies | 1,565–1,715 |
| resolve dependency bodies | 4,020–4,740 |
| demand | 130–150 |
| model assembly | 1,800–2,070 (finishing 280–400 inside it) |
| attach program calls | 1,860–2,040: adapter 280–310, event graph 140–160, drops ~270, **final passes 1,140–1,630** (event-parameter proofs 1,240–1,600; dead marks 13; rules 0–10) |
| relation + base facts | 260–290 + 290–320 |
| **model build total** | **13,500–14,600** |
| L4 solve (`compute_summaries`) | 300–330 |
| transaction spans + default detector run | 330 + 150 |
| **capability-cone composition** (per view) | **5,540–7,210** with primary subjects; 9,040–9,150 in two of three runs with a Base App subject; of which the pd solve 3,700–4,200 |
| one tree (`snapshot.derive`) | 1,860–2,660 per tree |
| per-root witness (`digest.roots`) | 400–4,200 per root; 2,735–3,015 for `IssueReminder`; eight roots in one read 3,355 (+0.62 s over one); three explain roots 3,357 |
| prove's own post-root work | `prove.literal_reach` 261–272, `prove.group_paths` 25–26, `prove.reach_effects` 411–446 (0.70–0.74 s) |
| explain card render | 810–900 one card; 926 for three cards |
| `close_holder_sites` (whole-program interface refinement) | < 1 |
| `deps.layer_consume` | 210–480 |
| `deps.extract_cpu_ns` | 360–1,410 **CPU, summed over parallel files**, not wall |

### 1.3 Warm costs measured as batch increments (the lower bound a server can reach)

| Batch | Single-app increment per extra job | Cross-app increment |
|---|---:|---:|
| explain | +0.40 s | +4.2 s (each job its own tree before stage 0; shared since) |
| prove | ≈ 0 | +1.4 s |
| digest | ≈ 0 | +0.4 s |
| query effects | ≈ 0 | ≈ 0 |

Peak RSS did not grow with the number of jobs (3,352–3,402 MB cross-app for 1–8 jobs).

### 1.4 Memory

| Quantity (cross-app) | Number |
|---|---:|
| build peak | 3.36–3.49 GB |
| model + base resident after the build, mimalloc with purge 0 | 1.34–1.37 GB; 2.78 GB under the platform allocator |
| relation | 33 MB |
| composition: derived store 73 MB; raw for every primary routine (5,479) 90 MB; pd arena 441 MB (link graph 346 + working 95) | ≈ 605 MB |
| all-subject composition (33,285 subjects) | +24 s, +1.27 GB; the all-subject snapshot +1.23 GB (peak 5.30 GB) |
| the LSP server (`LspSnapshot`, LIGHT) | 0.67–0.71 GB resident; with `--l4-host` 3.1–3.5 GB (platform allocator) |
| stat pass over the workspace (583 files + 11 `.app`) | 1.2–1.7 ms |

### 1.5 What the LSP's incremental updater does today

Body edit + `didSave` visible in `incomingCalls` in 112 ms; a signature-level edit in
1.3–1.6 s — on the LIGHT `LspSnapshot`, which holds none of what explain, prove, digest
and query read (no `Model`, no dependency bodies, no relation, no composition). The
`--l4-host` re-solves only affected SCCs (83 of 33,285 on a body edit) but republishes
after 18.7 s because it rebuilds the front end. The design below reuses its generation /
cancel / publish-only-if-current pattern and its SCC invalidation rule.

## 2. Words

- **Manifest**: every file and directory the build read or probed, present or absent,
  with the identity of what it found. Its hash is the **inputs stamp**.
- **Revision**: one manifest and everything built from it; `generation` counts built
  revisions of one engine instance.
- **Engine instance**: one run of an engine child (a random 128-bit id).
- **Δ_files**, **Δ_decl**, **Δ_body**, **Δ_rows**, **Δ_resolved**, **Δ_pd_output**: the
  change sets of §6.
- **Declaration projection**: the complete hash of a file's extracted declarations (§6.1).
- **Caller-local location** / **cross-file declaration reference**: the two location
  domains of §6.5.
- **Artifact snapshot**, **ready snapshot**, **fill registry**, **artifact generation**,
  **table epoch**: §4.1–4.2.
- **Heavy request**: composes a tree or reads roots (explain, digest, prove); **light**:
  query, status.

## 3. Process architecture and the engine protocol

```
agent host ──stdio MCP──▶ alsem mcp ──┐          editor ──stdio LSP──▶ al-call-hierarchy ──┐
                                      ▼ engine protocol (NDJSON JSON-RPC)                 ▼
                              alsem engine-host --workspace <ws> [--single-app]   (one child per workspace+universe)
```

**Landed** (`src/engine/host/`, `src/wire/`): protocol version 1; the child's first line
is `engine/hello` within 10 s; frames are one JSON object per line, at most 16 MiB;
request ids at most 1 KiB; `session/open`, `session/ask`, `session/status`,
`session/shutdown`; stderr drained by the front end with a 64 KiB tail, never forwarded
to stdout; restart with backoff 1 s → 30 s that resets only after a completed ask or 60 s
of stable running; three deaths in 60 s → `failed` for 300 s; shutdown deadline 10 s;
`Answer { id, outcome: Cli | Failed, revision | null }` with the spill order of §8.3.

**Completion requirements** (the landed child still runs the one-shot path per ask):

- `session/ask` carries `requestFiles` (the bytes of every file the command reads:
  `--diff <path>`, `--file`), `diffText`, `diffRootDecided` (**mandatory** when the
  command reads paths; the resident child refuses an ask without it, it never probes the
  file system itself), `deadlineMs`, `progress`; `session/cancel {id}`,
  `session/notifyChanged {paths}`, `session/refresh {wait}` exist; the child enforces its
  workspace and universe (`notThisWorkspace`).
- **Deadlines**: expired while queued → `deadline` with no work; while waiting on a
  build or a shared fill → `deadline`, the build or fill continues for others; during
  compute → the cancel flag is set, outcome `deadline` with `workDoneMs`.
- **Queues**: heavy depth 16, light 64; beyond → `busy`. Queued requests' bytes are
  counted and capped (16 MiB) → `busy`.
- **Lossless stderr** (round-6 NEW 3): the child keeps a command's stderr as the bytes
  its `WriteAction::Stderr`s produced; the wire carries it inline as `stderr: [lines]`
  **only when** the bytes are valid UTF-8 and joining the lines with `"\n"` reproduces
  them exactly (every line newline-terminated, no other control characters changed);
  otherwise the exact bytes go to `stderrFile`, or to an explicitly lossless inline
  form (`stderrBytes`, base64). A spill is always made from the original bytes, never
  from the line form. Tests: no final newline; empty and interior blank lines; non-UTF-8
  bytes; several stderr actions — each compared with standalone replay.
- **Frame recovery at the boundary** (NEW 4): the bounded reader records whether the
  rejected line's delimiter was already consumed; the child skips the rest of the line
  only when it was not. Tests: a line of exactly the limit; the limit plus one including
  its newline; longer; at EOF — each followed by a valid request that must be answered.
- **Bounded front-end queue** (NEW 5): the front end's reader validates each frame's
  shape before queueing it, bounds the queue by aggregate bytes (64 MiB) and frames
  (1,024), coalesces progress notifications per request, and on overflow kills the child
  (`protocol`), fails the pending ask, and records the failure in `status` so it is
  observable with no ask waiting. Test: a child hook floods small frames while the
  consumer is paused; the queue stays under its bound and the child is ended.
- **Frame bounding** at both hops with one terminal constructor (§8.3).

**As built (stage 2 completion, `src/engine/host/`, `src/engine/resident/`).** The child
is resident (§4.3 order; `explain`, `prove`, `digest`, `query` off the revision's
artifacts, every other command one-shot over the pinned revision's read). Protocol 1,
fields additive: `session/open {workspace, outputRoot, universe}` → `{ok, outputDir}`;
`session/ask {argv, diffText?, requestFiles?, decisions?, freshness?, deadlineMs?,
maxInlineBytes?, progress?}`; `session/cancel {id}`, `session/notifyChanged {paths}`
(notifications, or requests answered `{cancelled}` / `{ok}`); `session/refresh {wait}` →
`{revision, waitedForBuildMs, wallMs}` or `{started: true}`; answers add `stale`,
`universe`, `timing {queuedMs, waitedForBuildMs, sharedFillMs, computeMs}`, and
`workDoneMs`/`skippedPhases` for a stopped ask. `diffRootDecided` travels as
`decisions.diffRoot` (with `decisions.changedIsFile`), mandatory only when the command
reads paths. Deviations: one FIFO with one worker (heavy 16 / light 64 depth caps), no
per-root cancellation inside `digest_effects_each` (the flag is read between phases and
tree groups), no reservation budget (stage 3), artifact snapshots without table `lens`
(no append-only tables exist before stage 5), user `--out` still `refusedPath`.

The alternative — one unwind-enabled server binary — was compared and not taken: it
cannot contain a native abort, an OOM kill or a lowerer stack overflow, memory reclaim
after eviction would depend on the allocator, and the editor's server would hold the
model.

## 4. The session core

### 4.1 Artifacts and ownership

| Owned by `WorkspaceState` (survives a revision drop) | Owned by `Revision` (dropped with it) |
|---|---|
| inputs (acquired images, manifest); parse per file; the dependency tier (strong: nodes, parsed packages, body units, extracted sources, `DepKey`, image hashes); app registry and file-key table; graph, `DeclSemantics`, `DeclLocations` per file; per-file pre-refinement resolutions; refined edges; event-flow edges; demand; per-file projections (workspace, dependency, ABI); the L4 seed of the last published revision (`SolveBaseline` + `Arc<CoreSummaries>` + `Arc<L4Analysis>` — the complete triple; dropping any part forces a full solve, 0.33 s); the cone and tree stores' **tables** (interner, arena, section tables), append-only and epoch-tagged | the finished `Model`, the cross-app base (with `rel`, `dependency_calls: Arc<Vec<_>>`), `ordering`, `rules`, this revision's `l4` + `core`, and the revision's **artifact snapshot** (§4.2) |

A request holds one `Arc<Revision>` and, from `ensure_prepared`, one `Arc<ArtifactSnapshot>`.
Nothing reachable from either is ever modified.

### 4.2 Artifact snapshots and `ensure_prepared`

```rust
struct ArtifactSnapshot {
    cone_root: PersistentMap<SccKey, Arc<SccProducts>>,       // per-SCC cones, fingerprints
    raw_root:  PersistentMap<SubjectId, Arc<RawEntry>>,       // emitted raw facts per subject
    tree_root: PersistentMap<SectionKey, Arc<Section>>,       // tree sections (§6.11)
    det: Option<Arc<DetectorArtifacts>>,
    runs: PersistentMap<RunKey, Arc<RunOutput>>,
    lens: BTreeMap<TableId, (Epoch, usize)>,                  // the length of each table this snapshot may read
    gens: PersistentMap<ArtifactKey, u64>,                     // the artifact generation each entry was filled at
}
struct Revision {
    ..., snapshot: ArcSwap<ArtifactSnapshot>,
    registry: Mutex<Registry>,   // per ArtifactKey: gen (monotonic), state, waiters, assembly pins
}
enum ArtifactState { Absent, Filling { worker, waiters }, Published, Failed(error) }
```

**Artifact keys and their prerequisites** form a fixed DAG, the one-shot order the phase
functions already follow: `Fold` (the composition's derived rows and per-SCC cones) ←
`Raw(subject)` ← `Sections(subject)`; `L4` ← `Det` ← `Run(key)`; the whole tree sections
(`identities`, `typed_edges`, `callsite_resolutions`, `coverage`, `analysis_gaps`,
`event_declarations`, `root_classifications`, `boundary`) ← `Fold`. Phases: 1 `Fold`,
`L4`; 2 `Raw(..)`, `Det`, whole sections; 3 per-subject sections, `Run(..)`.

**Artifact generation.** Every key has a monotonic `gen` in the registry, distinct from
the storage tables' epochs: a claim records the gen it saw; an eviction increments it; a
publication is valid only if the value's gen equals the key's current gen (no eviction
happened in between). `gen` answers "is this value the one we meant"; table epochs
answer "are these indices meaningful".

**`ensure_prepared(rev, needs) -> Result<Arc<ArtifactSnapshot>, Failed>`**

1. Pin the revision. Compute `plan = closure(needs)` grouped by phase.
2. Under the registry: mark every key of `plan` **pinned for assembly** by this request
   (eviction skips pinned keys; the pin is released when the request returns or fails).
3. For each phase in order:
   a. `s = rev.snapshot.load()`. For each key of the phase missing in `s`: **claim** it
      (`Absent | Failed → Filling{worker: me, gen}`) or **join** it (`Filling` by another
      worker → register as waiter). Keys already `Published` in `s` are satisfied.
   b. Wait for every joined key of this phase (condvar, bounded by the request's
      deadline; a cancelled or expired waiter unregisters and the request answers
      `cancelled`/`deadline`; the fill continues).
   c. For claimed keys: `reserve(estimate)`; on failure **release the claims** (`Filling →
      Absent`), **notify waiters** (they re-enter their own loop), answer `budget`.
      Compute the values reading the current snapshot (its prerequisites are present:
      every prerequisite phase was completed before this one began) and appending to the
      tables under each table's append lock.
   d. **Publish as one transaction** for the phase's claimed keys: a validated rebase loop
      on `rev.snapshot`: `new = cur.with(values)` where (i) for each value, `value.gen ==
      registry.gen(key)` — else that value is **discarded** (an eviction happened; the
      key's claim is released to `Absent`, waiters notified, and the loop will re-claim
      it, at most 3 times per key before `engineError { "artifact churn" }`); (ii)
      `new.lens[table] = max(cur.lens[table], mine)` for tables of the **same epoch**
      (lengths only grow); (iii) if a table's epoch in `cur` differs from the one the
      value was built against (a compaction happened), the value is **recomputed** in
      the next iteration against the new epoch — and compaction is **excluded** while any
      claim is in flight (it takes the registry lock and waits for zero `Filling` entries,
      inside the builder with the heavy lane closed), so (iii) is a defensive branch, not
      the normal path. Only after a successful swap are the keys marked `Published` and
      their waiters notified; a fill error marks `Failed(error)`, notifies waiters, and
      fails the request `engineError`.
   e. Reload `s`; if some key of the phase is still missing (discarded or failed by
      another worker), repeat the phase (bounded as above).
4. After the last phase, `s = rev.snapshot.load()` satisfies `needs` by construction;
   return it — the **ready snapshot** — and keep it pinned for the compute. Release the
   assembly pins.

Properties: a dependent is never computed before its prerequisites are published; two
requests claiming disjoint keys publish concurrently without losing each other's entries
(the rebase merges roots and lengths); an eviction between claim and publish cannot
resurrect a value (gen mismatch discards it); every failure and cancellation path releases
claims and wakes waiters; no partial snapshot is ever returned; termination is bounded
(each phase iteration publishes, joins a fill that publishes or fails, or exhausts the
retry bound).

**Eviction** publishes a snapshot without the entries (gen incremented) through the same
rebase loop, skipping assembly-pinned keys; values are freed when the last snapshot
holding them drops; table entries referenced by no live snapshot are garbage for
compaction.

### 4.3 The request path

```
decode (clap, pure) → validate (Planned::Failed | Ready, no revision) → freshness → pin (Published | Failed{inputs, failure} | None)
→ bind PlanCx from the pinned revision's inputs → plan → resolve → ensure_prepared → compute → deliver
```

- **Validate** reuses the binary's `Planned::Failed / Ready` split with a `PlanCx` that
  has not bound a workspace read: every revision-independent failure (invalid format,
  question kind, selectors, unreadable diff input, `--order`, diff-root misuse) is the
  CLI's own line and exit, exit **3** included.
- **Failed build**: the session keeps the acquired `Arc<WorkspaceInputs>` and the
  `ViewFailure`; the job is built over those inputs and answers through
  `job.view_failed` — the standalone CLI's text and exit (`kind: cli`, `revision: null`).
- One pinned revision serves plan, resolve, prepare and compute; a newer revision
  published meanwhile does not change the answer (the answer names its revision).
- `job.compute` runs over a `View` facade holding `Arc` clones of the revision's cells and
  the request's own tree and effects; `ViewCx::release` drops the request's clone only.

### 4.4 Trees and roots

A request's tree is a `CapabilitySnapshot`-shaped view over the ready snapshot for the
request's subjects: direct rows for every covered routine (as `derive_capability_facts`
emits them), inherited rows for the subjects, the whole sections by `Arc`, the pd arena
projection for the subjects, rule B (`BoundaryMode`) as a read parameter — `Off` for a
root inside a dependency, whose raw is emitted on demand from the cone store. One read
serves all roots of a request.

### 4.5 Concurrency, admission, reservation

- Lanes: one heavy, four light. Builds: one at a time; publish-only-if-current under one
  lock (the L4 host's pattern).
- **Reservation before work.** `reserve(estimate)` succeeds iff `rss + reserved +
  estimate ≤ budget`. Estimates: a per-case table (cold, B, S, P, fills, heavy request)
  from CDO measurements scaled by routine count after the first build; before the first
  build, from `.al` file count and bytes and package bytes with a nonzero floor
  (cross-app 1.5 GB, single-app 0.5 GB); the observed peak (max of the last three)
  replaces the table value.
- **Floor and reclaim.** `retained_floor` = the measured bytes of the whole
  `WorkspaceState` column (`status.retainedBytes`). A build that does not fit: close
  admission to **all** new holders (light and `latest-built` included); wait for
  in-flight requests and fill workers (the only releasers, each bounded by its deadline);
  reclaim (evict unpinned snapshot entries, compact tables, drop recomputable
  projections); retry the reservation once; else `failed { budget, estimate, budget,
  retainedFloor }` at once. Never wait with no in-flight releaser. Impossibility is
  re-evaluated on every manifest change and budget change.
- **Overrun**: work is not aborted; `status.overruns` increments; the case's estimate is
  raised; no new work until RSS is under the budget minus the largest pending reservation.
- **Incremental update** (B, S, F, A-S): built beside the old revision under reservation;
  published; the old one dies with its last holder. **Full rebuild** (cold, P): drop-first
  as above; a failure leaves `revision: null`, stated in `status`.
- **Compaction**: reserved like a fill (2 × table bytes); only inside the builder with
  the heavy lane closed and zero fills in flight; old tables stay alive for pinned
  snapshots (`status.tablesRetainedBytes`).

### 4.6 Cancellation

The flag is read before each phase, inside `digest_query_with`'s root loop (a cancelled
read returns no entries), before the render and before serialization. The answer is
`outcome: cancelled` with `workDoneMs`, `skippedPhases`, `rootsProcessed`,
`rootsTotal`. A shared fill is never cancelled by a request; a build only by
supersession.

### 4.7 Identity

`engineInstanceId` + `generation` (built revisions, from 1) + `inputsStamp`
(`al-sem-manifest-v1:` + blake3 hex). Two answers are from the same model iff all three
are equal. `revision: null` before the first publication or after a failed full rebuild.

## 5. Freshness

### 5.1 Modes and guarantee

- `fresh` (default): the manifest is checked; if it differs from the published
  revision's, the revision is updated and the answer comes from the new one.
- `latest-built`: answers now from the published revision, `stale: true` with the
  changed paths when the manifest differs; starts a background update.
- `verify`: `fresh` with every text and package hashed, not stat'ed.

> A published revision was computed from one acquired image per input; the manifest
> computed from those images (**M_consumed**) equals the manifest observed immediately
> before acquisition (**M_before**) and immediately after the build (**M_after**). The
> server does not claim that no write happened in between.

### 5.2 The manifest and acquisition

The manifest is produced by the readers themselves (`WorkspaceInputs::read`, the
snapshot and dependency loaders record each access), so it cannot omit an input the build
used: the `.al` walk's directory listings and every `.al`; root and nested `app.json`;
`roots.config.json`, `al-sem.coverage.yaml`, `al-sem.dependencies.json` (present or
absent); each source dependency's root, `app.json`, walk and files; package discovery
(each candidate `.alpackages` folder — the root's and each ancestor's up to a `.git` or
eight levels — present or absent, with its `.app` listing, and the `.git` probes); each
`.app` with its pre-read stamp and the blake3 of the bytes actually loaded; the universe
and the engine and grammar versions.

**Acquisition.** Every input is read **once** into an owned image (landed for packages:
`src/app_image.rs`; an image of 1 MiB or more moves to a sealed anonymous temp file —
unlinked at creation on Unix, written once, positional reads; smaller images stay in
memory; the memory fallback is taken if temp storage fails). Every product of a package
derives from that image; every cache entry carries the image's hash and is a hit only
for equal bytes. Stated limit: a process running as the same user can reach the sealed
file's descriptor; that is outside the guarantee. On Windows the sealed copy requires an
exclusive share mode or the image stays in memory. The three-way check (landed for
packages as `check_consistent`) is extended to **every manifest entry**; any inequality
discards the build and retries (at most three times, then `failed { inputsUnstable }`).

### 5.3 The check and the watcher

Per `fresh` request: re-walk the manifest's directories and `stat` every entry (1.2–1.7 ms
on CDO); a difference re-reads and hashes that entry. The watcher (the LSP's `watcher.rs`
mapping) marks paths dirty; dirty texts and dirty packages are re-hashed and their cache
entries confirmed. Stated residual: a rewrite preserving mtime and length with no watcher
event is missed until `verify`.

## 6. Incremental update of a revision

### 6.1 Classifying the change

`DeclProjection(file)` = blake3 over a canonical encoding, in **document order with
multiplicity**, of every extracted `ObjectNode`, every `RoutineNode` and every
`RoutineMeta` of the file, including **every field of every type and nested type**
(`ParamMeta.name`, `method_id`, `try_function`, `closed_enum`, `enum_impls`, access
flags, `lookup_page_ids`, …) except the exclusion list: the positional origins
(`RoutineMeta.origin`, `name_origin`, node spans) and `RoutineNode.source_unit`. The
mechanism is a `Classify` trait implemented by **exhaustive destructuring with no `..`**
(a new field is a compile error until the author hashes it or adds it to the exclusion
list and bumps `DECL_PROJECTION_VERSION`); nested types implement the same trait; serde
attributes play no part. `DefSurface` stays for the LSP.

| Case | Condition | Reruns |
|---|---|---|
| **B** body-only | every file in Δ_files has an equal projection; M = P = ∅ | parse Δ_body; resolve Δ_body's files; the whole-program refinements; demand; rows for changed files; finishing, attach, final passes (whole); relation, base facts; seeded L4; cone dirty set; tree sections |
| **S** declaration (incl. **F** file add/remove/rename) | some projection changed, or a file added or removed; M ∩ P-fields = ∅; P = ∅ | as B plus: graph layer, `DeclSemantics`, `ResolveIndex`; every workspace file re-resolved; **every dependency body re-resolved** (4.0–4.7 s, conservative); projections for every re-resolved file |
| **A-S** config | `app.json` fields outside the P set, `roots.config.json`, coverage yaml | root `app.json` → S plus model fields; `roots.config.json` → finishing and the root-classification section; coverage yaml → nothing served reads it |
| **P** package / identity | a `.app` or source dependency changed, or `app.json`'s `id`, `name`, `publisher`, `version`, `dependencies`, `application`, `platform`, `internalsVisibleTo`, `propagateDependencies` | full rebuild (§4.5) |

### 6.2 Retained workspace state

The left column of §4.1, keyed as §6.8 states.

### 6.3 Inputs and parse

`WorkspaceInputs::with_changes(&old, Δ_files, manifest)`; parse Δ_files on the big-stack
pool from the owned images.

### 6.4 The dependency tier and the pack

The tier is held strongly by the session; valid while `DepKey` and every package image
hash are unchanged; any P rebuilds it whole with per-package parses reused by hash. The
on-disk **pack** persists declaration products and `RoutineMeta` per app under the pack
spec's content key (format version, extraction fingerprint, `.app` content hash, tier,
compilation context, grammar sha), written atomically with an integrity trailer, remapped
symbolically on load; dependency bodies are re-parsed from the packages (their IR is
~1.5 GB) and their resolution is never persisted (it depends on the workspace). Savings
are **measured** as the fresh path minus the pack path, end to end (wall and peak), with
`deps.layer_consume` on both sides; no number is promised beforehand.

### 6.5 Semantics, locations, and the two location domains

- **`DeclSemantics`**: the graph's workspace layer, `ResolveIndex`, `RoutineMeta` minus
  origins. One `Arc`, reused under B.
- **`DeclLocations[file]`**: every declared node's complete `Origin`s (byte and
  line/column ranges, kind) and the file's physical rows (document ordinals and spans);
  rebuilt for every file in Δ_files, B included; `RowNodes::build` reads positions from
  here; `ProgramGraph.workspace_rows` is assembled from these tables. Duplicate
  occurrences keep `DeclSurface`'s **last-write-wins** rule.
- **Caller-local locations** — `ObligationId::CallSite { span }`, `Edge.site: SiteId {
  span }`, `SiteFacts` keys, the builtin-dispatch audit rows, and an event edge's
  publisher-name `SiteId` — are captured in the owning file's `FileResolution` from that
  file's retained parse and are valid exactly as long as that resolution is retained: the
  file is re-resolved whenever its parse changes.
- **Cross-file declaration references** — a route's witness to a callee or subscriber
  declaration — are stored **symbolically** (`Witness::Decl(RoutineNodeId)`) and
  materialized to `{ file, span }` through the current `DeclLocations` when a
  `ProgramReport` is assembled (what `make_routine_route` computes from
  `surface.get_with_path(rid)` today, done late).
- **Which witnesses are symbolized** (round-6 NEW 2): only a route whose cold form is
  `Witness::SourceSpan` (a source-tier callee or subscriber found in the surface). A
  symbol-only callee keeps `Witness::AbiSymbol` and its `Opaque` evidence; a source-tier
  callee missing from the surface keeps `Unknown(IndexIntegrationGap)` with
  `Witness::None`. The **synthetic zero-span site** exists only on the existing
  publisher-site branch (an event publisher lacking surface metadata gets a zero-span
  `SiteId`); it is never applied to a callee witness. A `Witness::Decl(rid)` whose
  current `DeclLocations` has no entry is an invariant failure: in debug it panics; in
  release the owning file's resolution is redone against the current semantics (which
  yields whatever cold yields, normally `Unknown(IndexIntegrationGap)`), never an invented
  zero-span source witness.
- Event-flow edges are rerun whole every revision.

**As built (stage 4, `program::session`).** Two parts of this section were deliberately
not built, and the contract above holds without them:

- **No `DeclSemantics` / `DeclLocations` split.** Graph reassembly cost 0.31–0.33 s, but
  that was a scan of every routine per platform-event subscriber in
  `inject_platform_event_publishers`. A run lookup on the cold path (`program/build.rs`,
  same answer) cut it to about 20 ms, so the graph is rebuilt every revision, which is
  exact. Witnesses keep their cold `Witness::SourceSpan` form and are recomputed from the
  current declarations on every reused route, instead of being stored as
  `Witness::Decl(rid)` and materialized late. The rules are the ones stated above: only
  `SourceSpan` routes on routines are touched, `AbiSymbol`/`None` are left alone, a
  missing routine is an invariant failure, and there is no zero-span fallback for callees.
- **Event-flow edges stay whole, and cost 64–88 ms, not milliseconds-by-design.** The
  0.73–0.94 s first measured was a scan of every routine per subscriber route in
  `emit_event_flow_edges`; a run lookup (`resolve/resolver.rs`, same routine, first in
  merged order) removed it on the cold path too. No finer rule is correct: the test
  `event_flow_edge_of_an_unchanged_publisher_moves_under_b` shows a body-only edit above a
  workspace subscriber moving the edge of an unchanged dependency publisher, because the
  edge carries the subscriber's span.
- **Any root `app.json` change is P.** A per-file resolution reads `app.json`
  (`internalsVisibleTo`, test G28), so the A-S class of §6.1 is not split out.
- **P detection uses stamps** (`DepKey`, the `app.json` bytes, source-dependency content
  hashes); the stage-2 manifest and image hashes are not wired into the session yet.
- **Per-file raw rows** (`program/model/row_cache.rs`) are reused only when every input is
  equal: the parse (by identity), the text, the demanded population, the parenless calls,
  the needs, the app and the source unit. The per-build `FileIx` is kept symbolically and
  re-stamped on reuse. The ABI block is keyed by the dependency list, the apps' `AppRef`s
  and the needs; all rows are dropped on P.

### 6.6 Resolution

Per-file **pre-refinement** `FileResolution`s are retained. Under B only Δ_body's files
are re-resolved (the LSP's rung-1 argument: a call's resolution reads other files'
declaration surfaces, never their bodies). Under S every workspace file is. The
whole-program refinement `close_holder_sites` is rerun every revision from the retained
inputs (< 1 ms); its output is diffed per obligation and changed obligations join
Δ_resolved. Dependency bodies are reused across B and re-resolved **whole** under S
(4.0–4.7 s): their resolution reads the whole-graph index (interface implementers,
extensions, name uniqueness) and a precise dependency key was judged fragile. A finer,
object-level rule is a later refinement gated by a tripwire test proving no read bypasses
its recording facade.

### 6.7 Routine ids

Landed: `"{app_scope}/{64 hex}"`, `app_scope` = 16 hex of sha256 over the lowercase app
guid; shape preserved for `l5::fingerprint`'s scanner and `stable_sub_id`; finding
fingerprints and baselines unchanged.

### 6.8 The model pipeline, layer by layer

| Layer | Computes | Inputs / cache key | Scope and cost |
|---|---|---|---|
| L0 | parse | owned image | per file |
| L1 | projection, nodes, `DeclSemantics`, `DeclLocations`, physical rows | L0 | per file; semantics whole under S |
| L2 | per-file pre-refinement resolution (symbolic cross-file witnesses) | L0 of the file, `DeclSemantics` | per file |
| L2b | `close_holder_sites`; event-flow edges | all workspace L2 results; `DeclSemantics` | whole; < 1 ms + ms |
| L2c | `cross_app_demand` | refined workspace edges ∪ **all dependency-body edges** ∪ event-flow edges; graph seeds; primary app | whole; 0.13–0.15 s |
| L3 | file projections: workspace (`whole_file_population`); dependency (`demanded_population(file, demand)`); ABI apps (`project_abi_to_index`) | per file: (parse hash, parenless set, **demanded slice of the file**); per ABI app: image hash | per file / per app |
| L4 | raw rows in the **cold order**: workspace files by name, document order; then **symbol-only** dependency apps by package path (ABI objects then routines in `SymbolReference` order); then **source-bearing** apps by package path, files by virtual path, document order | L3 | identities: source rows `(FileKey, ordinal)`; ABI rows `(AppId, ABI object ordinal, ABI routine ordinal)`; node ids beside; `AbiRowIds` as today |
| L5 | `finish_resolved` (record types, extension-field merge, root classifications, primary app) | L4, configs | whole; 0.28–0.40 s; clones raw → finished rows (the one copy per revision) |
| L6 | `RowNodes::build` | L5, `DeclLocations`, graph | whole; 67 ms |
| L7 | the adapter (every edge converted and joined: it reads `j2_row`, callee rows, graph objects, interface metadata, `SiteFacts`, the registry), two-pass order (all call-site edges in row then site order, then all implicit-trigger edges), event graph, `upgraded_bindings`, `from_ix` | materialized report, L5, `RowNodes`, graph, `AbiRowIds`, demand | **whole**; adapter 0.28–0.31 s + events 0.14–0.16 s |
| L8 | `settle_dead_mark_trust` (13 ms), `settle_rules` (≤ 10 ms), `prove_event_param_temps` (**1.24–1.60 s**; an index `object_id → rows` makes its `complete(p)` scan per object, behaviour-preserving, measured before and after) | L7 model | whole; in place on the finished rows |
| L9 | cut, relation, base facts | L8 | whole; 0.6 s |

The sum of the existing spans for L2b–L9 is a **subtotal** of 2.7–3.2 s; it omits the
cut, the raw→finished clone, witness materialization, report aggregates and drops. Stage
4 measures the whole refresh under a `revision.refresh` span and a `model.copied_bytes`
counter before any gate is set.

### 6.9 L4, seeded

The host's rule (`SolveBaseline`, R0 without the instance id, R1 row digests, R2 SCC key
and one-hop) with the complete seed triple; the core summaries are retained immutable
and borrowed by the det harvest (`CoreInput::Borrowed`) and by the next seeded solve.

### 6.10 The cone store and the arena

Retained per SCC: fact cone, coverage cone, memo row, successor set, input digest,
product fingerprint, and each member out-edge's **transfer digest**.

**Dirty** if: the SCC key is new; a member's row digest changed (facts, frames, dead
sites, guards, parameters, argument bindings); a member's typed out-edge set changed; a
member's direct coverage input or unresolved-source status changed; an out-edge's
transfer digest changed (callee parameters by name/index/type/by_ref, callee frame, caller
site requirements, publisher ↔ subscriber name mapping — what `cross_edge`,
`substitute_entry`, `GuardAnchor` and `param_guard` read on the callee); a successor is
dirty **and** its product fingerprint changed. Rules other than the last are local and
never cut off.

**Product fingerprint**: every `ConeFacts` entry's `rep_key`, the representative's full
content hash, `dist` and guard; the coverage cone; the memo row. A dirty SCC is
re-folded; its predecessors only if the fingerprint changed. A member whose direct
coverage input changed has its `CoverageRecord` recomputed regardless.

**Producers.** Affected producers = owners in Δ, owners reachable from Δ in the **old or
the new** graph, producers whose fact or frames changed; their state and link ranges are
re-solved per producer (states never interact across producers). **Δ_pd_output**,
conservative: every routine holding a state of an affected producer in the old or new
arena, plus routines whose own entries referenced such a state; their derived rows are
re-folded, their raw re-emitted (primary) or dropped (dependency), their tree sections
rebuilt — independent of cone dirtiness or cut-off. `pd.delta` gets its own span and peak
counter; the measured `pd.solve` (3.7–4.2 s) is the solve alone.

**Canonical arena (stage 1b, total ranks).** Ranks use the solve's own identities: producer
`(owner internal id, operation id)`; edge = the full `TypedOutEdge` identity in internal
ids; state `(producer rank, dist, routine internal id, temp state, reqs)` — total, the
solve's own state key; link `(edge rank, callee state rank)`. The finalization pass
permutes producers, states, edges and links, remaps every reference, rebuilds link ranges,
`by_routine` and resolved entries (first hop = lowest-ranked link whose callee has
`dist − 1`). Stable ids appear only at serialization; two internal ids projecting to one
stable id stay two states. Storage indices are epoch-local and never serialized.

**Raw emission** for primary subjects eagerly; dependency subjects on demand from the
retained cones (the all-subject emission measured 24 s, and a Base App subject's
emission can cost seconds, so it is never eager).

**Compaction** of the interner tables and the arena when garbage exceeds 25 % or every 64
revisions, under §4.5's rules.

### 6.11 The tree store

Whole = re-derived every revision by the existing `derive_*`; per key only where the key's
inputs are exactly that key's rows and cone products. The per-key input is the **full
row including anchors**: any file in Δ_files marks every id with a row in it.

| Section | Reads | Rule |
|---|---|---|
| `identities` | objects, tables (real over stub), routines | whole |
| `typed_edges` + skipped-edge keys | the ledger: call-site, record-op, publisher and subscriber anchors; `routine_to_stable`; global `sort_key` order | whole (cost measured per section in stage 5) |
| `callsite_resolutions` | ledger rows (`callee_display`, `result_consumed`, `under_asserterror`, last owning row), bodyless dependencies, typed edges, closed interface sites | whole |
| `coverage` | every routine's `CoverageRecord` | whole |
| `analysis_gaps`, `event_declarations`, `root_classifications`, `boundary` | model-level | whole |
| `capability_facts` | direct facts per routine; raw per subject | per routine / per subject: rebuilt on Δ_files membership, Δ_rows, Δ_pd_output, re-emission |
| `operation_index`, `callsite_index`, `routine_order_frames` | all rows of an id, raw of the id | per id, same rule |
| `pd_solution` | the arena at the pinned epoch, the view's subjects | per view |

**As built (stage 5, a prefix: §6.9 and the producer half of §6.10; branch
`ctxsrv/stage5`).**

- *Seeded L4 (§6.9), done.* `Revision::seeded` takes the previous revision's
  published `L4` artifact (the complete triple: `SolveBaseline`, the bundle in
  `L4Analysis`, `Arc<CoreSummaries>`) as its seed, or the seed that revision
  still held unused; the `L4` fill runs the host's seeded solve (`L4Reuse`)
  and the seed is released once the revision's own solve publishes. A seed
  is used only when `seed_key` (universe, the packages the read consumed,
  root `app.json` bytes, model-instance id, source dependencies, engine
  version — the host's `host_global`, read from the revision's own acquired
  inputs) is equal; a P change is a full solve. The seed lives on the
  revision chain rather than in a `WorkspaceState` column: the seeded
  solve compares digests, so a seed from any earlier revision with an equal
  key gives the cold answer. *Found while measuring:* the fill's 2.2-2.4 s
  on CDO was almost all the serial `Debug` digest of every routine row (the
  seeded solve itself is 0.13 s); the digests are now hashed in parallel and
  the fill is 0.57-0.70 s cold or seeded (115 of 33,285 SCCs re-solved after a
  body edit).
- *Producers (§6.10 "Producers"), done, without the arena tables.* Each
  resident fold keeps `pd_input_digests` (per routine: every model row of
  the id, its guard frame with sites sorted, its typed out-edges). The next
  revision's fold computes Δ, and `pd_solve::solve_seeded` re-solves a
  producer when its owner is reachable from Δ in the new graph, when it held
  a state at a routine of Δ in the old arena, or when its fact changed;
  every other producer's states are copied from the previous fold's arena
  (one contiguous range per producer in the canonical order) with each link's
  edge re-found in the current graph by its full identity, and everything is
  finalized by stage 1b's canonical pass, so the arena equals a full solve's.
  CDO, body edit: 2,590 of 2,634 producers copied, `pd.solve` 4.0 s -> 1.4 s
  (copy 0.37 s, finalize 0.86 s). **Deviation:** no append-only interner or
  arena tables, no epochs and no `lens`: a revision's fold is a self-contained
  immutable value that shares nothing mutable with the next revision (the
  next fold reads the previous fold through an `Arc`), so §4.2's epoch
  branch has nothing to guard and no reader can see a mix of revisions. The
  cost is memory: the previous fold stays alive until the next fold
  publishes.
- *Not built (remaining stage-5 work):* the per-SCC cone store (fact cone,
  coverage cone, memo row, input digest, product fingerprint, edge-transfer
  digests, cut-off), Δ_pd_output feeding derived/raw/tree invalidation, raw
  re-emission for Δ subjects only, the tree store (§6.11) and its per-section
  spans, T-cone / T-cone-edge / T-coverage / T-typed-edges / T-callsite-res /
  T-obj-only / T-tree, and the live T-prepared cases for retained stores.
  The composition is otherwise recomputed whole every revision.
- *Where the time goes after a body edit (CDO, release-fast, census over
  the composition):* the fold is about 15 s: producer solve 1.4 s (was 4.0),
  fact cones about 2.2 s, singleton walks about 2.6 s (memo rows, cone scans,
  raw materialization), direct facts about 0.4 s, graph and Tarjan about
  0.25 s, the recursive sets about 0.4 s, and an untimed remainder. Each
  heavy answer then spends 7-11 s of its own on top of a prepared fold (its
  tree: `snapshot.derive` 2.9-3.3 s, roots 3.2-3.7 s; prove's det and run
  about 1.5 s): the tree store is the other half of the ≤ 1 s target.

**As built (stage 5b: the cone store and the tree store; branch
`ctxsrv/stage5b`).**

- *Timers.* Every part of the fold has a span (`cone.direct`, `cone.graph`,
  `cone.tarjan`, `cone.dedup`, `cone.frames`, `pd.digests`,
  `cone.member_digests`, `pd.solve` with `pd.copy` / `pd.finalize`,
  `pd.output_delta`, `cone.walk`, `cone.finish`, `revision.fold`,
  `revision.seed_release`), and so does every tree section (`tree.base`
  and its sections; `snapshot.derive` for a view). The earlier "untimed
  remainder" was the SCC walk itself: 10.1 s of the 12.0 s fold after a body
  edit (census: singleton walks 6.7 s with their raw scans 2.5 s and raw
  emission 2.4 s, fact cones 1.9 s, recursive members 1.3 s).
- *The cone store (§6.10), done.* Kept per SCC, keyed by the member set:
  an input digest, a product fingerprint, the non-root fact and coverage
  cones (`Arc`, shared with the next fold). The input digest covers each
  member's producer-solve digest (rows, frame, typed out-edges), raw direct
  facts, direct coverage input, unresolved-source status and scope mode;
  per out-edge the transfer digest (the callee's parameters, read by the
  publisher ↔ subscriber guard mapping, and the callee's frame); the member
  order, the recursive and root flags; and each successor's key and
  fingerprint. The fingerprint hashes every cone entry's representative
  (full content hash), distance and guard in key order, and the coverage
  cone; the entry key is not hashed, being a function of the representative
  and the guard. A reused SCC's members copy their rows: derived rows by
  window (the new builder continues the previous interner, whose order no
  output reads), raw entries adopted with the table set they index
  (`InheritedFacts` segments), coverage records cloned. Δ_pd_output is
  computed exactly: per routine, the projection (producer, fact, temp
  state, requirements, seed, distance, first-hop edge) over the states of
  re-solved and vanished producers, old against new; a member in it
  re-folds its rows inside a reused SCC. Deviations: (1) memo rows are not
  kept: a memo row is F over its cone's closed entries, which the
  fingerprint covers; a re-folded predecessor rebuilds the rows it reads.
  (2) A root SCC keeps no cones, as before; one that gains a caller is
  dirty (the root flag is in its digest) and is built then. (3) No
  persistent maps: the next fold copies the reused rows into its own stores
  (`cone.walk` 0.18-0.20 s after a body edit, almost all copying). (4)
  BLAKE3 for the store's digests (SHA-256 over 4.7 M entries cost 2 s of a
  cold fold). (5) The store is kept under `--low-mem` too.
- *The tree store (§6.11), done for one revision.* `TreeBase` (identities,
  typed edges with the skipped-edge keys, call-site resolutions, gaps,
  coverage, event declarations, root classifications, routine order frames,
  boundary) is derived once per revision, never carried over; each distinct
  view is a `Tree(subjects)` artifact (evidence indexes, capability facts,
  pd arena) over it. `routine_order_frames` moved to the whole sections: its
  deriver reads rows only, never raw. Deviations: (1) no per-key sections
  across revisions (operation and call-site evidence, capability facts are
  derived per view per revision: 0.65 s; the pd projection 0.85-0.92 s);
  (2) a view's tree holds clones of the whole sections (65 ms), not shared
  `Arc`s, which would change `CapabilitySnapshot`'s field types at about 65
  construction and assignment sites; so a revision with one view holds the
  sections twice. A refresh frees the published revision's trees before it
  builds.
- *Measured on CDO* (release-fast census probe with mimalloc, quiet machine
  unless noted; `--verify` runs: every answer equal to its cold run after B,
  B, S and after B, S):

  | step | refresh | fold | L4 | tree base | explain | prove | digest | query |
  |---|---:|---:|---:|---:|---:|---:|---:|---:|
  | cold | 18.0-18.8 s | 14.3-14.9 s | 0.57-0.59 s | 1.47 s | 6.1-6.3 s | 5.7-6.1 s | 3.2-3.4 s | 40 ms |
  | B | 3.1-3.3 s | 2.16-2.25 s | 0.60-0.67 s | 1.45 s | 6.0-6.1 s | 5.8-6.3 s | 3.5 s | 37 ms |
  | B (no Δ) | 3.3 s | 1.97 s | 0.63 s | 1.43 s | 5.9 s | 5.9 s | 3.5 s | 37 ms |
  | S | 10.4-11.2 s | 2.27-2.33 s | 0.64 s | 1.46-1.55 s | 6.3 s | 5.9-6.2 s | 3.3-3.8 s | 36 ms |
  | P | 19.5 s | 15.2 s | 0.58 s | 1.50 s | 6.2 s | 5.9 s | 3.5 s | 40 ms |

  Before stage 5b (same machine): fold after B 12.0 s, explain 7.5-7.8 s,
  prove 8.7-9.1 s, digest 6.3-6.5 s. Resident memory (no budget enforced by
  the probe): default RSS 6.0 GB after B, peak 6.34 GB (6 GiB budget: 6.44
  GB; the cone store holds 4.73 M entries, +0.6 GB; a revision's trees +0.6
  GB until evicted or the next refresh); `--low-mem` 4.8 / 5.2 GB (5.9 GB
  peak across S).
- *The ≤ 1 s hypothesis is missed.* "Analysis prepared" after a non-hub
  body edit is fold 2.2 s + L4 0.6 s = 2.8 s, and 4.2-4.4 s with the tree
  base. The fold after a body edit: `pd.solve` 0.95-1.18 s (copy 0.32 s,
  canonical finalize 0.58-0.74 s), `cone.direct` 0.33 s (every routine's
  direct facts), `cone.walk` 0.19 s, `pd.digests` 0.16 s, graph, Tarjan and
  dedup 0.18 s, `pd.output_delta` 0.02-0.08 s, the rest under 0.1 s. L4:
  row digests 0.17 s of 0.6 s. The tree base is rebuilt whole by the rule
  (typed edges 0.52 s, call-site resolutions 0.48 s, order frames 0.22 s).
  A heavy answer's own work is now dominated by the root witnesses
  (`digest.roots` 2.9-3.2 s per answer, recomputed by each of explain,
  prove and digest for the same root), then `explain.indexes` 0.9 s and
  prove's det and run about 1.5 s.
- *Not built:* per-key tree sections carried across revisions; shared
  (`Arc`) whole sections inside a view's tree; a per-producer merge in place
  of the global canonical re-sort (0.58-0.74 s); parallel fresh-producer
  solving for cold and P; incremental direct facts; a per-revision memo of
  root witnesses (not in this design; the largest remaining per-answer
  cost; built since, below).

**As built (the witness memo, plan `2026-11-03-context-server-remaining.md` item W; branch
`ctxsrv/witness-memo`).** A root's `DigestEntryResult` is a function of the root and
`include_reads` over any tree holding it (the plan's Q1), so a revision keeps it as
`ArtifactKey::Witness { root, include_reads }` (kind `witness`, phase 3, value
`Arc<Option<DigestEntryResult>>`; `None` = no entry, stored so it is not asked again).
`digest_effects_each` is split into `root_entries` (the rule-B partition, the root passes) and
`finish_effects` (prove's literal reach, which mutates entries; the per-reader entries and
unresolved cones; the shared parts); the one-shot path calls both. `needs_of` adds one key per
root of every effects read; the fill reads the request's own `Tree(scope)` and fails loudly
without it. The store fills claimed keys one by one, so the fill closure carries a
request-local batch: the first witness fill computes every absent root of the request in one
pass (one index build, the roots in parallel). `trees()` hands `finish_effects` clones.
`witness` is first in `EVICTION_ORDER` and in the reclaim, is reclaimed with the trees at a
refresh, is never carried to the next revision, and `status` reports `fills.witness`
(`hit`, `miss`, `batches`). Deviations: (1) the fill does not assert the fixed root-pass
options at run time: they are not parameters of `root_entries` (it always passes no return
summaries and `ordering_witness_only` off), so no caller can alias the key with others; (2) the
plan's discrimination break "hand out the stored entry instead of a clone" cannot be written
(the value is behind an `Arc`); the proof breaks the same rule by memoizing the post-prove
entry; (3) `Case::Fill("witness")` keeps the default 60 MiB until the fill's peak is measured
apart; (4) the fill counter counts published keys, so the batch shows in `batches`, not in
`fill_counts()["witness"]`.

Measured on CDO (release-fast probe, mimalloc, `census-probe incr --witness`, load 2-4, every
answer equal to its cold run in a `--verify` run after cold, B, S and P): per command with its
view's tree warm, the root's witness evicted (miss) against present (hit): explain 4.3-4.5 s →
1.09-1.20 s, prove 3.8-4.2 s → 0.86-0.92 s, digest 3.1-3.4 s → 0.15 s (`digest.roots` 2.4-3.2
s, `digest.fingerprint_indexes` 84-111 ms per pass). A step's round of explain, then prove and
digest on the same root (cold, B, B; same machine, same hour, master `14a23675` beside it):
explain 5.8-6.1 s, prove 2.55-2.63 s (with the revision's first det and run), digest 0.15-0.16 s,
against 5.9-6.1 / 5.6-6.0 / 3.2-3.6 s on master; RSS 5.82-6.00 GB, peak 5.94-6.21 GB, against
5.82-5.99 / 6.20-6.33 GB on master (the `--witness` runs show ~7.1 GB: their extra views). Cross view: explain over three roots 6.1-6.3 s
(one pass), then prove on one of them 0.92 s (a hit). A witness holds 7.9 MiB (`IssueReminder`,
`IssueFinanceChargeMemo`) or 5.6 MiB (`RunReportID`). What a hit still pays: explain's
`explain.indexes` 0.87-1.14 s (cards 4-24 ms, reaching tests 3-4 ms), above W2's 0.4 s bar;
prove's literal reach 0.29-0.31 s and `prove.reach_effects` 0.41-0.46 s.

**As built (explain's indexes, plan item W2; branch `ctxsrv/explain-indexes`).** `Indexes` and
`CrossRules` borrowed the model, so they got an owned form: `ExplainIndexes` (in
`explain.rs`) holds `IndexData` (rows, objects and run actions as indexes into the model, ids
as owned strings; `DeclaringRows` is `DeclaringObjects` by object index, sharing its pick) and
a `CrossRulesIndex` (steps and callers as `SiteIx` indexes into their row's call sites or
record operations). No `Arc<Model>` beside borrows: the indexes are read through short-lived
views (`Indexes::over`, `CrossRules::over`) over the revision's model and the artifact, and
`CrossRules::over` asserts the routine count matches. `CrossRules` held per-run mutable state
(the memo, the stack, the 200,000-expansion budget, the stop flag, the `usable` cache); that is
now the walk's own, made fresh per run, so the artifact is immutable and a request never sees
an earlier request's stop (the memo alone keeps only complete, context-free results, so sharing
it would not change bytes; the budget and the stop would). Within one run the cards still share
one walk, as cold. `ArtifactKey::ExplainIndexes` (kind `explain_indexes`, phase 0, no
prerequisites) is demanded by `explain_plan` (a new `Plan::explain_indexes` flag, also for an
explain with no root, whose `--table/--field` answer reads the cross-rules index through
`RulesIndex::build_where_over`); `serve_plan` hands it to `View::served`, and explain's compute
asserts a served view carries it. The one-shot path builds the same owned form per run, so no
output moves. Eviction: after the det, before `tree_base`; reclaimed at a refresh; never
carried. `Case::Fill("explain_indexes")` is 30 MiB (measured peak). Deviation: the plan's
"break: share the memo" does not discriminate (see above); the proof shares the stop flag.

Measured on CDO (release-fast probe, mimalloc, `census-probe incr --witness`, cold and B,
master `0d902458` beside it, same machine and hour): explain with its witness warm (hit) 1.16-1.17
s → 0.20-0.22 s; explain over three roots 6.3-6.4 s → 5.2-5.4 s; a step's first explain 6.2-6.4 s
→ 5.9 s; prove and digest unchanged. The fill takes 0.87-0.94 s, peaks +29.0 MiB and leaves 25.7
MiB (212,165 allocations). RSS after each step 7.10 / 7.21 GB on both (the `--witness` runs'
extra views); the HWM 7.49 / 7.56 GB against 7.32 / 7.39 GB, which includes the branch probe's
extra evict-and-refill measure of the indexes. A `--verify` run after cold, B, S and P: every
answer (the four commands, explain over three roots, explain with its indexes evicted and
present) equal to its cold run; explain hit 0.22-0.24 s, fill 0.89-1.03 s.

### 6.12 Explain

Landed: grouping by equal `tests`, repeatable `--routine` (sorted distinct roots,
per-selector diagnostics, exit 0 if any card else 2, single-selector document unchanged).
Measured: three roots in one read cost +0.4–0.6 s over one; renders 0.93 s for three.

### 6.13 Cost per case (hypotheses until the stages measure them)

| Case | Today | Finished |
|---|---:|---:|
| B | ≈ 21 s (build + composition) | model ≤ 2.5 s (requires the `complete(p)` index: the whole steps are 2.7–3.2 s today); analysis ≤ 1 s for a non-hub edit |
| S / F / A-S | ≈ 21 s | ≤ 9 s (dependency re-resolution 4.0–4.7 s + workspace 1.7 s + whole steps) |
| P | ≈ 21 s | ≈ 14–15 s model + composition |
| `roots.config.json` | ≈ 21 s | < 1 s |

**As built (stage 4), model published**, measured on a temporary copy of CDO
(release-fast, `aldump --session-refresh --verify`, every step equal to the cold build):
B **2.43–2.53 s** (was 5.7–6.0 s before the per-file rows); S/F **7.0–7.35 s**; P
12.2–13.6 s (the cold model build on the same binary: 11.7–12.4 s). A B refresh still
spends: snapshot build 0.35–0.39 s, resolve 0.19 s (event flow 64–88 ms), demand 0.17 s,
dependency rows 0.25 s (cloning the kept rows), `finish_resolved` 0.27–0.30 s,
`RowNodes` 0.08 s, attach 0.90–1.0 s. S still re-resolves every dependency body
(4.1–4.7 s): the finer rule needs stage 6's recording facade and its audit.
`model.copied_bytes` is 15.5 MB per B. "Analysis prepared" is stage 5's.

## 7. Memory

Budgeted and reported by the child (`status`): retained floor, reserved, snapshots
pinned, tables retained, fill workers. ~~Targets: resident after a build and one request
of each kind ≤ 3 GB; peak ≤ one cold build (3.4–3.5 GB) — measured by T-mem, not
assumed. `--memory-budget` (default 4 GB cross-app, 1.5 GB single-app)~~ — **superseded**
by the owner's decision and stage 3's defaults below. `--idle-timeout` 600 s; the front
end's `--max-children` (default 2) and `--total-budget`. Eviction order: dependency
subjects' raw and rows, tree indexes, det and runs, cones (digests kept); never the model,
relation, tier or L4 seed.

**Measured cost of stage 4's retained rows:** about 1 GB. The peak of the whole
`aldump --session-refresh --verify` run on CDO (session plus the verifying cold builds)
rose from 5.46 GB to 6.50 GB. Stage 3's T-mem must account for it against the 3 GB
resident target; dropping the dependency rows' copy (0.25 s per B) and sharing them
instead is the first candidate if it does not fit.

**Owner decision (2026-11-02).** The default keeps everything and is the fastest; about
**4.6 GB resident on CDO** (cross-app, after a build and one ask of each served kind) is
the accepted default target, replacing the 3 GB above. An opt-in **`--low-mem`** keeps no
dependency syntax trees between revisions and parses dependency files again when a step
needs them (below). The default budgets stay as stage 3 set them: **6 GiB cross-app,
2 GiB single-app**, for both modes. *Superseded 2026-11-03 (owner decision): after stage
5b's cone and tree stores the default mode reached ~6.0 GB RSS after a B edit (peak 6.34
GB), right at 6 GiB, so the default cross-app budget is now **8 GiB**; `--low-mem` keeps
6 GiB and single-app 2 GiB.*

**As built (stage 3, `engine::resident::budget`, master `a48f8e0e`).**

- *Where the memory went* (`census-probe resident <ws> [routine]`, a counting allocator
  over the build `alsem engine-host` makes, the four asks twice, then every retained part
  dropped in order; CDO, master `e5b515be`): live heap after the build 3,522 MiB, after the
  four asks 4,441 MiB. The workspace session held 2,635 MiB of it (dependency body parse
  trees 1,121, only 115 of it source text; dependency rows 691; dependency resolution 595;
  workspace rows 70; workspace trees and resolutions 69; metadata 72), the model and
  cross-app base 888 MiB (routine rows 576), the artifacts 918 MiB (fold 602, det 191, l4
  79, run 46). The child's RSS is about 0.7 GB above its live heap (28 M live
  allocations); `mi_collect` after an ask frees none of it.
- *What was reduced and how*: no spare capacity is retained. Each parse's arenas are
  moved into exact allocations (`al_syntax::ir::exact_size`: mimalloc keeps a block a
  `shrink_to_fit` still fills by half); the dependency edges and their route vectors are
  exactly sized; the session keeps a clone of the dependency rows, not the push-built
  original; a routine's `statement_tree` is behind an `Arc`, one exact copy shared by the
  kept rows and every revision's model. Live heap after the build 2,792 MiB (was 3,522),
  after the four asks 3,711 MiB (was 4,441); child RSS after the build 3.69 GB (was
  4.26), after the four asks 4.55 GB (was 5.14), peak 5.37 GB (was 5.98). Answers
  unchanged (every golden and T-order test).
- *Admission and reservation* (§4.5): every build, fill and request's trees reserve
  their estimated peak; `rss + reserved + estimate ≤ budget`, else admission closes, the
  in-flight releasers are waited for, unheld artifacts are evicted in §7's order
  (dependency raw, the session's dependency rows, runs, det, fold, ordering, rules; never
  the model, relation, tier or L4 seed), and the reservation is tried once more, else
  `budget` with `estimateKiB`, `budgetKiB`, `retainedFloorKiB`. An estimate that cannot
  fit above the retained floor (RSS less every evictable artifact's measured bytes) fails
  at once (`impossible`). A first build is estimated from the manifest's `.al` and
  package bytes (floor 1.5 GiB cross-app, 0.5 GiB single-app) and refused before
  anything is read; observed peaks (`/proc` peak windows) replace the table. A request's
  artifacts stay pinned through its compute (found on CDO: a request's own fold was
  evicted by its trees' reservation).
- *Idle children*: `--idle-timeout` (600 s) — the child announces `engine/idleExit` and
  ends after a grace; the front end starts a new child on its next call, no death counted.
- *Lanes*: one heavy worker and four light workers (queries) in the child.
- *Panic*: a panic in a request ends the child in every profile; the front end restarts
  it (backoff, §3).
- *MCP*: `notifications/cancelled` drops a queued call or sends the running one
  `session/cancel` (not answered); `alsem_refresh` is `session/refresh {wait}`;
  `--memory-budget`, `--idle-timeout`, `--max-children` (default 2: a call for another
  workspace ends the least recently used idle child) and `--total-budget`.
- *Default budgets, and why*: **6 GiB cross-app** — the design's 4 GiB is below what CDO
  retains after one build (3.7 GB RSS), so it would refuse every first explain; 6 GiB
  holds CDO's measured peak (5.4 GB) with margin. **2 GiB single-app** (CDO single-app
  one-shot peak 0.75–0.9 GB). Measured with the default budget: no refusal, RSS after
  query + explain + prove + digest 4.59 GB, retained floor 3.90 GB; with 5 GiB the
  explain's trees reclaimed the dependency rows (10,881 files) and every answer was given
  (RSS 4.34, peak 5.10 GB); with 4.5 GiB explain is refused `budget` (floor 3.75 GB plus
  the trees' 0.9 GB).

**As built (`--low-mem`, 2026-11-02).** `alsem engine-host --low-mem`, `alsem mcp
--low-mem` (passed to each child), `aldump --session-refresh --low-mem`, `census-probe
resident … --low-mem`. The cross-app workspace session builds with
`BuildProfile::ON_DEMAND` (`DependencyBodies::OnDemand`): the tier keeps no tree (it is
the `Summary` tier and shares its `DepKey`), and a build's context holds the trees only
when it parsed them, so they die with it. The session keeps each source-bearing
dependency's extracted text instead (117 MiB on CDO): the file list the snapshot shares,
so the next snapshot reads that same text through the session's `DepCache` at the same
`.app` stamp, and nothing is extracted again. §5.2's rules hold: the text is the one the
snapshot read from the acquired package image, a tree is `al_syntax::parse` of it, and a
changed package is P, which drops all of it. What is parsed when:

| Case | Dependency files parsed |
|---|---|
| P | all (the tier is built from that parse, as in the default mode) |
| S / F | all again (`ProgramContext::ensure_dep_bodies`): every dependency body is re-resolved |
| B | none, except the files whose kept rows or routine ids miss in the model assembly (a demand expansion, changed assembly needs, rows evicted): those only, in parallel |

The row cache keys a file's rows and a dependency file's routine ids on the text, not the
tree's identity (the text stands for the parse), so it holds no tree in either mode; the
on-demand file numbering comes from the snapshot's file lists in the parse's order.
Measured on CDO (release-fast, `IssueReminder`, one child over the protocol; every
refresh step equal to cold):

| Measure | Default | `--low-mem` |
|---|---:|---:|
| RSS after query, explain, prove, digest | 4.70 GB | **3.72 GB** |
| retained floor (`status.memory.retainedBytes`) | 3.90–3.91 GB | 2.95–2.97 GB |
| peak during the asks | 5.56 GB | 4.56 GB |
| live heap after the asks (census-probe) | 3,711 MiB (trees 925 MiB, text inside) | 2,903 MiB (text 117 MiB) |
| warm query / explain / prove / digest | 0.04 / 6.3–6.4 / 6.1–6.4 / 5.5–5.7 s | 0.04 / 6.3–6.4 / 6.2–6.3 / 5.4–5.7 s |
| refresh B | 2.52–2.86 s | 2.29–2.37 s (no text extracted again) |
| refresh S / F | 6.8–7.2 s | 8.0–8.5 s (+1.2–1.4 s: 10,881 files parsed) |
| refresh P | 12.3–13.1 s | 11.9–12.5 s |
| RSS after a B, an S and an S edit, each with the four asks | 5.36 GB | 4.39 GB |

The low-mem default budget stays 6 GiB, set by measurement: at 4.5 GiB the asks after
the first S edit were refused, at 5 GiB the second S was refused as impossible (an S
parses every dependency file, its observed peak becomes its estimate, and the floor
creeps 0.2–0.3 GB per refresh in both modes, allocator retention); at 6 GiB nothing was
refused or evicted. The saving is headroom, not a smaller default. Two findings left for
later: the default mode extracts the dependency text again on every B (about 0.3 s that
`--low-mem` does not pay), and the per-refresh RSS creep. (Both fixed, next note.)

**As built (the refresh creep and the dependency text, 2026-11-02).** *The dependency
text:* the workspace session keeps each source-bearing dependency's extracted file list
in both modes, so the default mode's B and S refreshes share the text through the
session's `DepCache` at the same `.app` stamp (size, time, image blake3) instead of
extracting it again; a changed package is P. B refresh median 2.76 s -> 2.61 s on CDO.
*The creep* was measured before it was fixed (`census-probe refresh-seq` in process with a
counting allocator over mimalloc, and `front-seq` against a real child; each refresh
followed by the four asks). It is **not a retention**: the live heap stays within 2 MiB
over 30 refreshes in both modes. It is **mimalloc page fragmentation**: every refresh
builds the next revision beside the published one and then frees the old one, and a freed
block stays resident in any page a live block still holds; transparent huge pages make it
worse (`khugepaged` fills released ranges back in). It converges rather than grows without
end: over 40 B then 20 S refreshes the child went 4.81 GB cold, 5.06 GB after the first
refresh, and flattened at 5.44-5.46 GB. The fix is in the allocator, not the engine
(`al_sem::release_alloc`): `alsem engine-host` discards the interior of every freed block
of 16-512 KiB (`madvise(MADV_DONTNEED)`, Linux), and a `--low-mem` child also turns
transparent huge pages off for itself; every other command keeps plain mimalloc. After,
same refreshes (RSS cold / after the first refresh / at most):

| Mode | Before | After | Band over the first refresh |
|---|---|---|---:|
| default (40 B, 20 S) | 4.81 / 5.06 / 5.46 GB | 4.58 / 4.78 / 5.07 GB | 0.40 -> **0.29 GB** |
| default, huge pages off as well | | 4.24 / 4.44 / 4.66 GB | **0.22 GB** |
| `--low-mem` (20 B, 10 S; huge pages off) | 3.81 / 4.03 / 4.37 GB | 3.50 / 3.73 / 3.95 GB | 0.34 -> **0.22 GB** |

Every series is flat after about ten refreshes of each kind. Huge pages off cost about
4 % on the asks (the four asks 36.0 s before, 36.6 s with the release, 38.2 s with huge
pages off as well), so the default mode — kept the fastest by the owner's decision —
releases with huge pages on, and `--low-mem` turns them off. The retained floor follows
the RSS, so the budget's estimates see it. Pinned by `tests/resident_memory_bound.rs` (no
per-refresh retention, both modes) and the child's `status.memory.releasedBlocks` /
`thpDisabled` (e2e).

## 8. The MCP front end (`alsem mcp`)

### 8.1 Transport, versions, lifecycle

stdio NDJSON JSON-RPC 2.0; logs on stderr only. `SUPPORTED = ["2025-06-18", "2025-03-26",
"2024-11-05"]`: echo the client's version if supported, else `2025-06-18`. `server/discover`
is answered `-32601` in every state (Claude Code 2.1.311 probes it and falls back to
`initialize`); a 2026-07-28 form is a codec-only addition later. State machine:
`Uninitialized` (accepts `initialize`, `ping`, `server/discover`; else `-32002`) →
`Initializing` → `Ready`; `initialize` twice → `-32600`; ids number or string, `null` or
duplicate in flight → `-32600`. Progress only with `_meta.progressToken`, which every
notification carries. Negotiated 2025-03-26 or older: text block only.

### 8.2 Tools

Common inputs: `workspace`, `freshness` (`fresh` | `latest-built` | `verify`),
`singleApp`, `out`, `maxInlineBytes` (clamped to 4 MiB). `alsem_explain` (`routines[]` |
`diff` + `diffRoot` | `table` + `field`; `format` default `human`), `alsem_prove`,
`alsem_digest` (always a file plus a summary), `alsem_query_effects`,
`alsem_query_touches`, `alsem_status`, `alsem_refresh`. Every argv is parsed by the
binary's clap, so flags and defaults are the CLI's.

### 8.3 Result schema and outcomes

```
kind: "cli" | "failed";  revision: {engineInstanceId, generation, inputsStamp} | null  (always present)
cli:    exitCode (the CLI's: 0, 1, 2 or 3), stdoutText XOR stdoutFile{path,bytes,sha256},
        stderr[] XOR stderrFile XOR stderrBytes (lossless, §3), stale, changedPaths, changedPathsTruncated,
        universe, timing{queuedMs, waitedForBuildMs, sharedFillMs, computeMs}
failed: outcome ∈ {cancelled, deadline, budget, busy, engineError, refusedPath, notThisWorkspace, inputsUnstable, protocol},
        message (≤ MESSAGE_INLINE_BYTES), diagnosticFile?, workDoneMs?, skippedPhases?, rootsProcessed?, rootsTotal?,
        estimateKiB?, budgetKiB?, retainedFloorKiB?
```

`kind: cli` includes a **semantic** build failure (the command's `view_failed` text and
exit). `structuredContent.output` (the CLI JSON parsed) is a separate projection, not
byte-identical; the contract is on `stdoutText` / the file. **Frame bounding** at both
hops (landed as `bound_frame`): a failed message over the inline bound is cut and spilled;
then, while over the limit: `stdoutText` → file; `stderr` → file (exact bytes);
`changedPaths` cut to 256; then the **terminal** answer. One constructor
`Answer::terminal(id, revision | null, outcome, bounded message, Option<diagnosticFile>)`
is used for every terminal path at both hops; a spill failure yields it with the bounded
write error and no diagnostic file.

### 8.4 Presentation amendment (R30)

Explain defaults to the CLI's human mode (its own byte contract); digest always goes to a
file with a summary; query goes to a file over the cap. Full JSON is always available by
`format: "json"` / `out`.

## 9. The LSP front end

`al-call-hierarchy` gains `al-call-hierarchy/explain`, `/prove`, `/digest`, `/query`,
`/engineStatus`, `/engineRefresh` (the frozen prefix), with the MCP envelope's params and
results; one engine child per root, started on the first custom request; requests handed
to a per-root worker thread so `main_loop` never waits; `$/cancelRequest` →
`session/cancel`; the root's watcher sends `session/notifyChanged`. Configuration in
`.al-sem.json` / `~/.al-sem/config.json` through `state_paths` (legacy spellings for
reads). The editor process never holds the FULL model.

**As built (2026-11-02, `src/lsp/engine.rs`, `src/mcp/pool.rs`).**

- *Requests*: `al-call-hierarchy/explain`, `/prove`, `/digest`, `/queryEffects`,
  `/queryTouches`, `/engineStatus`, `/engineRefresh`. The design's single `/query` is two
  requests, one per MCP tool (`alsem_query_effects`, `alsem_query_touches`), so params map
  one to one. Params are the MCP tool's arguments, validated by `mcp::tools::parse_call`,
  plus `uri` (its root answers; else the first root) and `workDoneToken` (`$/progress`
  begin/end). The result is the §8.3 envelope itself (no MCP content blocks); invalid
  params `-32602`, disabled `-32803`.
- *One pool, two transports*: the MCP server's workers moved to `src/mcp/pool.rs` behind a
  `Transport` trait (wrap a response, write a frame, report progress, end a cancelled
  request). Workers are per (workspace, universe), as in MCP, not per root; the children
  bound (`maxChildren`, default 2) and queue depth are the MCP ones.
- *Cancellation*: `$/cancelRequest` drops a queued request or sends `session/cancel`; the
  request is answered `RequestCancelled` (`-32800`, LSP requires an answer), with the
  engine's `cancelled` envelope as `data` when it ran.
- *Edits*: `didSave` and every watcher event with a path (through a forwarding thread
  between the watcher and the updater) go to running children as `session/notifyChanged`
  (`EngineFront::notify_changed_if_running`, never starts a child). The watcher runs only
  for a root call hierarchy has built, so `didSave` is the edit signal otherwise.
- *Executable*: `alsem` next to `al-call-hierarchy`, or `engine.executable`. The server
  binary cannot host the engine: the child decodes each argv with the `alsem` CLI's clap
  and planning, which live in that binary. The front end's decisions for an argv that reads
  paths (`--diff`, `--file`) are made by the same binary on the server's side (`alsem
  engine-ask --params`); other argvs need none (the child's answer is the same as with the
  workspace as diff root).
- *Config*: the `engine` section of `~/.al-sem/config.json` overlaid by the first root's
  `.al-sem.json` (`enabled` default true, `executable`, `memoryBudget`, `lowMem`,
  `idleTimeoutSecs`, `maxChildren`), read at the first engine request; outputs in
  `~/.al-sem/lsp-out/<instance>/`.
- *Measured on CDO* (release-fast, `IssueReminder`, a shared machine under heavy load —
  load average 110–265 — so latencies are upper bounds): the LSP's RSS 6.0–6.1 MiB idle,
  8.2 MiB after the first engine request, 8.5 MiB after nine; the child 3.96 GB after the
  first explain (peak 4.78 GB), 4.09–4.27 GB after the warm asks. First explain (cold child
  and build) 37.0–39.7 s; warm explain 7.7–8.5 s, warm query 0.04–0.16 s, warm prove 7.8 s
  on the quieter run. Tests: `tests/lsp/lsp_engine.rs` (T-lsp).

## 10. Output policy and path safety

- **Authorized read roots**: the workspace root, each source-dependency root, each
  discovered package folder, the ancestors up to the discovery boundary for `.git` probes.
  Request files are read by the front end and shipped as bytes; `diffRoot` is a mapping
  base decided by the front end.
- **Trust anchors**: the roots named at `session/open`, canonicalized once, opened, and
  verified by identity; every descendant is reached by a component-wise walk from the
  anchor handle (`openat` with `O_DIRECTORY | O_NOFOLLOW`; `mkdirat`; reparse points
  refused; Windows `NtCreateFile` relative to the parent handle with
  `FILE_OPEN_REPARSE_POINT`, else fail closed).
- **Private namespace**: the instance directory `<output root>/<engineInstanceId>/` is
  created by the server (`0700` / owner-only DACL) and **refused if pre-existing**; every
  entry the server creates is recorded with its identity; the server's own
  validate/create/replace operations are serialized; **overwrite** is permitted only for
  a recorded entry whose current identity matches, via a temp leaf and `renameat` under
  the same lock, and refused anywhere else. Threat model: another process running as the
  same user can mutate that directory and reach sealed image descriptors; the contract
  defends against the server's own races and other users, not same-user processes (the
  CLI's boundary). The landed child's spill directory is this instance directory.
- **Windows fail-closed**: without handle-relative open, no user `out` is accepted;
  outputs go only to a verified server-owned root, else answers are inline-only and an
  oversized answer is `refusedPath`.
- Outputs are pinned until the front end has delivered the response and either the
  client's next request arrives or ten minutes pass; quota 1 GB per instance root; files
  older than 24 h removed at child start.

## 11. Equality proofs and tests

Every comparison with a cold CLI run strips only the envelope fields (`revision`,
`timing`, `stale`, `changedPaths`, `outcome`); stdout bytes, stderr bytes and exit are
compared raw. Every test states its precondition by assignment and records a break that
was seen to fail (CLAUDE.md). CDO-based tests run on a temporary copy of the baseline.

| Id | What | Break |
|---|---|---|
| T-order | query→digest, digest→query, explain→prove, prove→explain, digest→evict det→digest, digest→edit→seeded update→digest, two requests on the same subjects; each vs a cold CLI run; fill counters | drop `core` after the harvest; return coverage by `mem::take` |
| T-prepared | first request on an empty snapshot; after eviction; first dependency subject; disjoint keys published concurrently; fill and eviction racing on one base; last waiter cancelled while the fill publishes; a paused reader keeps its ready snapshot; **a joined prerequisite plus an independently claimed dependent** (B joins A's fold, claims its raw, must wait for the fold before computing); **failed reservation with a waiter** (the waiter is woken and re-enters); **tombstone-rejected publication followed by refill** (the discarded value is not published; the key is re-claimed and refilled at the new gen); **append and compaction publication races** (lengths merge monotonically; compaction waits for zero in-flight claims) | publish with a plain store; return `s` before it satisfies `needs`; compute a dependent before its prerequisite; mark `Published` before the swap |
| T-loc | unchanged workspace and dependency callers whose callee body grows; text inserted before a publisher; a body edit in the caller's own file; duplicate-occurrence last-wins; **missing publisher metadata → zero-span site**; **symbol-only callee → `Witness::AbiSymbol`**; **source-tier surface miss → `Unknown(IndexIntegrationGap)`, `Witness::None`** | apply the zero-span fallback to a callee witness → manufactured source evidence |
| T-class, T-proj-guard, T-methodid | §6.1 cases incl. `[MethodId(n)]` / `[TryFunction]` through a demonstrated `__alsem_mid_<n>` dispatch with a cold output change | classify with `DefSurface` |
| T-demand-transitive, T-convert, T-abi-rows, T-order-rows, T-rows, T-refine, T-world, T-incr | §6.6–6.8 cases (W→D1→D2; return-type change; repeated ABI entries; trigger-then-call ordering; revocation; setter closure; implementer signature change; B/S/F/A-S/P vs cold) | per-file caches for whole steps |
| T-cone, T-cone-edge, T-pd-out, T-coverage, T-typed-edges, T-callsite-res, T-obj-only, T-tree | §6.10–6.11 cases | key-only fingerprints; skip old-graph reachability; per-key invalidation of a whole section |
| T-arena-canon | shuffled inputs → identical finalized arena; two internal ids sharing one stable id both retained; incremental after insertion/deletion/compaction equals cold | rank by stable id |
| T-acquire-during / T-acquire-after | B read during acquisition while before/after name A → refuse and retry; owned A acquired then the path mutated → A consumed, may publish; a live mapping fails the latter | compare before/after only |
| T-fresh, T-manifest, T-tier, T-pin, T-plan, T-failed-build, T-exit3, T-terminal, T-frame, T-stderr (lossless cases), T-frame-boundary, T-queue-bound | §3, §4.3, §5, §8.3 | as stated in each section |
| T-race, T-cancel (one-worker pool, barrier), T-panic (`engine-test-hooks`, every profile), T-mem (a workload with a demonstrated allocation above the test budget; refusal before any cone allocation), T-budget, T-paths (inside-root overwrite permitted; recorded entry replaced → refused; pre-existing instance dir → refused), T-lsp | §4.5–4.6, §7, §10 | as stated |

## 12. Build order and targets

| Stage | Content | Exit criterion and proof |
|---|---|---|
| 0, 1, 1b, ids, 2-plumbing, 4 | landed (4 without the semantics/locations split; §6.5) | — |
| **2 (completion)** | resident revision; `ensure_prepared` (§4.2); the full-manifest three-way check; `requestFiles`, deadlines, cancel, `notifyChanged`, enforcement, mandatory `diffRootDecided`; lossless stderr, boundary-safe frame recovery, bounded front-end queue; private-namespace outputs; `Answer::terminal`; exit 3 | T-order, T-prepared (cold roots), T-acquire-*, T-exit3, T-terminal, T-frame, T-stderr, T-frame-boundary, T-queue-bound, T-paths, T-fresh/T-manifest, T-tier, T-failed-build, T-pin, T-plan; measured sequential warm numbers per command |
| 3 | admission with fill workers counted; eviction via snapshots; `latest-built`; watcher; idle; LSP | **landed** (master `a48f8e0e`; §7's as-built note; the LSP front end and T-lsp since, §9's as-built note) **except** per-root cancellation inside `digest_effects_each`, and an automated T-mem bound (measured by hand, `census-probe resident`). The 3 GB resident target is superseded (§7: ~4.6 GB default, `--low-mem` 3.7 GB) |
| 5 | seeded L4; cone store with Δ_pd_output and per-member coverage refresh; tree store with whole sections and per-section spans; retained stores integrated only after T-prepared's publication invariants pass | T-cone, T-cone-edge, T-pd-out, T-typed-edges, T-callsite-res, T-coverage, T-obj-only, T-tree, T-prepared (live), T-incr end to end |
| 6 | declaration pack; finer dependency rule behind its audit | fresh − pack path measured end to end |

### 12.1 Targets (three moments; measured unless marked)

| Measure | Target | Status |
|---|---:|---|
| model published after B | ≤ 2.5 s | measured 2.43–2.53 s (stage 4) |
| model published after S/F | ≤ 9 s | measured 7.0–7.35 s (stage 4) |
| model published after P | ≈ 14–15 s | measured 12.2–13.6 s (stage 4); analysis adds the composition |
| analysis prepared after B | ≤ 1 s non-hub | hypothesis |
| warm `query` | ≤ 0.1 s | measured |
| warm `prove`, one root | roots 2.8–2.9 s + prove's 0.70–0.74 s + ≈ 0.25 s + first-use 0.6 s | measured spans; end-to-end is stage 2's number |
| warm `explain`, one / three roots | roots 2.7–3.0 / 3.36 s + render 0.8–0.9 / 0.93 s | measured |
| dependency-root request | emission 0.1–3 s (observed delta) + roots (ms) | hypothesis range |
| cold start with pack | fresh − pack path | no number promised |
| resident / peak | ~~≤ 3 GB / ≤ one cold build~~ superseded: ~4.6 GB default (owner, 2026-11-02) | measured 4.70 / 5.56 GB default, 3.72 / 4.56 GB `--low-mem` (§7) |
| manifest check | ≤ 10 ms (1.2–1.7 ms Linux) | Windows to measure |

## 13. Decisions taken and points still open

Decided (owner): cross-app default; "fresh" default; `alsem mcp` + hidden `alsem
engine-host` in the `alsem` binary; the epic's "no daemon / no persistent cache"
non-goals lifted; explain shares one tree and takes several roots; whole-step reruns
where a precise key would be fragile.

Decided (owner, 2026-11-02): the memory targets — the default keeps everything (~4.6 GB
resident on CDO), `--low-mem` is the opt-in, budgets 6 GiB cross-app / 2 GiB single-app
(§7).

Open: an editor child beside an agent child is two children's memory; selective cone retention if the store exceeds its target;
Windows measurements (manifest check, directory walk, handle-relative creation, sealed
copy share mode) before stage-2 exit numbers are quoted there; a shared server over a
local socket for several agents on one workspace (the child protocol is socket-ready).

Windows, checked on Linux (2026-11-02, branch `ctxsrv/windows`): the lib, both binaries
and every test target cross-check clean for `x86_64-pc-windows-gnu`; fixed: the
protocol stream (`SetStdHandle`, as `dup2` on Unix), inline-only answers (the cap rises
to 4 MiB, larger is `refusedPath`, §10; before, any stdout over 64 KiB was a `protocol`
failure), the budget meter (working set; a peak window is known only when the lifetime
peak rose), `CREATE_NO_WINDOW`; the sealed copy's share mode is `tempfile`'s (0, delete
on close), so §5.2 holds. Remaining for a real Windows run, scripted in
`scripts/windows-ctxsrv-check.ps1`: the test subsets, the CDO transcript (wall times,
the child's working set, the large digest's delivery), the manifest check against its
10 ms target. Still open by design: handle-relative creation (`NtCreateFile` relative
to a parent handle) so Windows gets file outputs; no interior release of freed blocks
off Linux (the refresh RSS creep is unmeasured there); workspace keys are spelled
paths, so `U:\x` and `u:/x` would start two children.

## 14. Re-measuring

Cold per command: `/usr/bin/time -v alsem <cmd> …` with `ALSEM_TRACE=1`. Composition
and sizes: `aldump --snapshot-memory <ws>`; `ALSEM_RETAINED_CENSUS=1 alsem policy check`.
Roots by count: `alsem digest <ws> --routine …×N`. Server: a Python MCP client sending
`initialize`, `tools/list`, each tool twice sequentially, timing each call and reading
`/proc/<child>/status`; then the edit cases of §6.1 on a temp copy, timing
`alsem_refresh {wait: true}`. Spans to add: `revision.refresh`, `model.copied_bytes`,
`pd.delta`, `tree.*` per section, `ensure_prepared` fill counts and waits in `status`;
pack: fresh vs pack path end to end.

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

**Landed on master `c5dbed28`:** stage 0 (`PlanCx`, wire codec, explain grouping,
multi-root explain, `BoundaryMode`, prove spans), stage 1 (phase split of `View::prepare`
with owned/borrowed inputs, measurement spans), the routine-id migration (per-app id
prefix), the `Cu.Run(x)` resolver fix, and the stage-2 plumbing (owned package images,
the engine child and its front end, hidden `alsem engine-host` / `alsem engine-ask`,
`engine-test-hooks`). **In progress:** stage 1b (canonical arena order). Everything else
below is the build order from here.

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
- Event-flow edges are rerun whole every revision (declaration-level, milliseconds).

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

## 7. Memory

Budgeted and reported by the child (`status`): retained floor, reserved, snapshots
pinned, tables retained, fill workers. Targets: resident after a build and one request
of each kind ≤ 3 GB; peak ≤ one cold build (3.4–3.5 GB) — measured by T-mem, not assumed.
`--memory-budget` (default 4 GB cross-app, 1.5 GB single-app), `--idle-timeout` 600 s;
the front end's `--max-children` (default 2) and `--total-budget`. Eviction order:
dependency subjects' raw and rows, tree indexes, det and runs, cones (digests kept);
never the model, relation, tier or L4 seed.

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
| 0, 1, ids, 2-plumbing | landed | — |
| **1b** (in progress) | canonical arena with total internal-id ranks and the complete remapping pass | T-arena-canon; witness equality or accepted differences listed with the golden regeneration |
| **2 (completion)** | resident revision; `ensure_prepared` (§4.2); the full-manifest three-way check; `requestFiles`, deadlines, cancel, `notifyChanged`, enforcement, mandatory `diffRootDecided`; lossless stderr, boundary-safe frame recovery, bounded front-end queue; private-namespace outputs; `Answer::terminal`; exit 3 | T-order, T-prepared (cold roots), T-acquire-*, T-exit3, T-terminal, T-frame, T-stderr, T-frame-boundary, T-queue-bound, T-paths, T-fresh/T-manifest, T-tier, T-failed-build, T-pin, T-plan; measured sequential warm numbers per command |
| 3 | admission with fill workers counted; eviction via snapshots; `latest-built`; watcher; idle; LSP | T-race, T-cancel, T-panic, T-mem, T-budget, T-lsp; resident ≤ 3 GB |
| 4 | `Classify` projection; semantics/locations split with the two domains and the publisher-only fallback; L2c/L7 whole; ABI ordinals; `complete(p)` index; `revision.refresh` and copy counter | T-class, T-proj-guard, T-loc, T-methodid, T-demand-transitive, T-convert, T-abi-rows, T-rows, T-order-rows, T-refine, T-world, T-incr (model); refresh wall and copy bytes recorded |
| 5 | seeded L4; cone store with Δ_pd_output and per-member coverage refresh; tree store with whole sections and per-section spans; retained stores integrated only after T-prepared's publication invariants pass | T-cone, T-cone-edge, T-pd-out, T-typed-edges, T-callsite-res, T-coverage, T-obj-only, T-tree, T-prepared (live), T-incr end to end |
| 6 | declaration pack; finer dependency rule behind its audit | fresh − pack path measured end to end |

### 12.1 Targets (three moments; measured unless marked)

| Measure | Target | Status |
|---|---:|---|
| model published after B | ≤ 2.5 s | hypothesis; gate set from `revision.refresh` |
| model published after S/F/A-S | ≤ 9 s | hypothesis; ≥ 4.0–4.7 s measured |
| model published after P | ≈ 14–15 s | measured (model build); analysis adds the composition |
| analysis prepared after B | ≤ 1 s non-hub | hypothesis |
| warm `query` | ≤ 0.1 s | measured |
| warm `prove`, one root | roots 2.8–2.9 s + prove's 0.70–0.74 s + ≈ 0.25 s + first-use 0.6 s | measured spans; end-to-end is stage 2's number |
| warm `explain`, one / three roots | roots 2.7–3.0 / 3.36 s + render 0.8–0.9 / 0.93 s | measured |
| dependency-root request | emission 0.1–3 s (observed delta) + roots (ms) | hypothesis range |
| cold start with pack | fresh − pack path | no number promised |
| resident / peak | ≤ 3 GB / ≤ one cold build | T-mem |
| manifest check | ≤ 10 ms (1.2–1.7 ms Linux) | Windows to measure |

## 13. Decisions taken and points still open

Decided (owner): cross-app default; "fresh" default; `alsem mcp` + hidden `alsem
engine-host` in the `alsem` binary; the epic's "no daemon / no persistent cache"
non-goals lifted; explain shares one tree and takes several roots; whole-step reruns
where a precise key would be fragile.

Open: the memory targets (3 GB resident, 4 GB budget per cross-app child; an editor child
beside an agent child is two); selective cone retention if the store exceeds its target;
Windows measurements (manifest check, directory walk, handle-relative creation, sealed
copy share mode) before stage-2 exit numbers are quoted there; a shared server over a
local socket for several agents on one workspace (the child protocol is socket-ready).

## 14. Re-measuring

Cold per command: `/usr/bin/time -v alsem <cmd> …` with `ALSEM_TRACE=1`. Composition
and sizes: `aldump --snapshot-memory <ws>`; `ALSEM_RETAINED_CENSUS=1 alsem policy check`.
Roots by count: `alsem digest <ws> --routine …×N`. Server: a Python MCP client sending
`initialize`, `tools/list`, each tool twice sequentially, timing each call and reading
`/proc/<child>/status`; then the edit cases of §6.1 on a temp copy, timing
`alsem_refresh {wait: true}`. Spans to add: `revision.refresh`, `model.copied_bytes`,
`pd.delta`, `tree.*` per section, `ensure_prepared` fill counts and waits in `status`;
pack: fresh vs pack path end to end.

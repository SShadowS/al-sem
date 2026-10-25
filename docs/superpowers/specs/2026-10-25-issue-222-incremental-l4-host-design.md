# #222 (#160 step 13b): the incremental L4 host behind `--l4-host`

Status: spec for the panel, 2026-10-25, written at base `0fff74b1`. Every `file:line`
below was re-read at that base. The probe table (P1-P24) is in
`.agent/issue-222/ledger.md` under `## Probes`; this spec cites probes by number.

Parent: the epic `2026-10-18-issue-160-analysis-session-design.md`, §9 row 13 (second
half), with the operator's answers of 2026-10-22 (Q1, Q2, Q3, Q5, Q7 below). 13a (salsa
removal, watcher blindness) is merged (70fc2e9b, #232) and has no spec file in `docs/`
(P21). The pre-draft `/work/bench/160-13/draft-step13-design.md` was written before
steps 6, 9 and 12 merged; this spec replaces its host and worker sections and corrects
three of its assumptions (P5, P15, P16).

## Words

- **Revision**: one frozen set of workspace inputs (`WorkspaceInputs`,
  `src/program/inputs.rs:37`) and everything computed from it.
- **Fresh**: what today's code computes for a revision: build the target, make a `View`,
  fill `rel` and `l4` (`View::prepare`, `src/engine/session.rs:688-1041`).
- **Host**: the new `IncrementalHost`. It keeps what it needs from the last completed
  revision (the **baseline**) and builds the next revision's L4 result.
- **Incremental**: what the host computes for a revision from its baseline.
- **L4 result**: the `L4Analysis` (`src/engine/l4/analysis.rs:26-34`: the SCC, the
  `SummaryBundle`, the summarize diagnostics, the warnings) plus the core summary map
  (`CoreSummaries`, `:38`) that the same solve returns.
- **SCC**: strongly connected component of the call graph, a set of routines that can
  all reach each other. The solve visits SCCs callees first
  (`src/engine/l4/scc.rs:38-45`).
- **SCC key**: an SCC's member ids, sorted. `Scc.members` is already sorted
  (`scc.rs:31-36`), so the key is that vector.
- **Row**: everything the solve reads for one routine id (P5): every model row of the id,
  its out-edges, its uncertainty edges, the upgraded bindings of its call sites, its
  leaf summary and fixed-role flag, and one "shared event" bit per `event-dispatch`
  out-edge.
- **Row digest**: sha256 of the row's `Debug` text (P4).
- **Δ (changed set)**: the ids that were added, were removed, or whose row digest
  changed, between the baseline and the new revision.
- **Global inputs**: what the solve reads that is not per id (§4.3 R0).
- **Successor** of a routine: the target of any of its out-edges in the primary graph
  (`rel.primary().edges_by_from`). The SCC graph is built from the same edges
  (`analysis.rs:143-153`).
- **Seeded SCC**: an SCC whose results the host copies from the baseline instead of
  solving it. **Re-solved SCC**: one the host solves.
- **Early cutoff**: a re-solved SCC whose results equal the baseline's does not, by
  itself, make its callers re-solve (Q7).
- **Request** and **generation**: the LSP side bumps a counter (the generation) each time
  it asks the host for a new revision (a request).

## 1. Goal

1. **An incremental host for L4.** Given the baseline and the current disk, the host
   builds the front end fresh (inputs, program, model, relation) and re-solves only the
   SCCs that can have changed. Every other SCC is seeded from the baseline. Its L4 result
   equals a fresh one in every projection a reader sees (§4.7), after body, signature,
   edge-removal, SCC split and merge, demand and dependency edits, in both universes.
2. **Invalidation by reverse reachability, SCC identity, demand and config files**,
   stated once (§4.3) and pinned by tests that hand-state which SCCs must be re-solved.
3. **Generation-checked, cancellable L4 work off `on_swap`.** With the hidden
   `--l4-host` flag, `on_swap` only files a request. The host runs on its own thread, one
   per workspace root. A newer request cancels older work, and a stale result is never
   published.
4. **Nothing changes without the flag.** No new editor output (Q1). The default LSP, every
   CLI command and every golden stay byte-identical.

## 2. Non-goals

- **No consumer of the host's result in the editor** (Q1). The host publishes only to
  an in-process status record and a debug log line. "Analyzer diagnostics in the editor"
  is a separate issue.
- **No incremental front end.** Inputs, program, model and relation are rebuilt per
  revision. Only the L4 solve reuses work (the R3b rule:
  `docs/superpowers/notes/2026-07-24-r3b-incremental-l4-design-intent.md`).
- **No incremental `det`, cone, `caps`, witness pass or runs.** The host fills `rel` and
  `l4` only (the `query` plan's cells, `session.rs:470-478`).
- **The LSP keeps its LIGHT snapshot** (P1). The host builds its own `FULL` program and
  never reads or patches `LspSnapshot`.
- **The diagnostics recompute stays where it is**, on the updater thread inside
  `on_swap` (`server.rs:629-658`). The pre-draft moved it to a worker; the issue's scope is
  L4 work, and moving it changes editor timing for every user (OQ5).
- **No reuse across a file add, remove or rename, or an `app.json` change.** Every id
  changes then (P16), so the refresh is Full and only the oracle applies.
- **No reuse across a `.app` change** (Q2): Full.
- **No new watcher mapping.** A `roots.config.json` edit reaches the host at its next
  request; nothing reads the host's result, so a dedicated trigger buys nothing (OQ3).
- **No persistent cache and no daemon** (epic §2).
- **No CDO numbers.** Non-speed (Decision 58); P12 and P13 are dropped (OQ7).

## 3. Background (at `0fff74b1`)

### 3.1 The LSP

- `RootState::state` builds a root lazily (`server.rs:222-266`); `build_server_state`
  (`:575-668`) builds the LIGHT snapshot, publishes diagnostics and spawns the updater with
  an `on_swap` closure (`:629-658`).
- `spawn_updater` (`src/lsp/updater.rs:1465-1566`) gathers a debounced batch, picks rung 1,
  2 or 3, swaps the snapshot and calls `on_swap` on its own thread (`:1506`, `:1527`,
  `:1545`, `:1553`, `:1557`). A batch holding `ConfigChanged` calls `on_swap` with
  `SwapScope::Full` even when nothing was rebuilt, on the SAME snapshot generation
  (`:1503-1507`, `:1555-1558`; P15).
- The shipped binary aborts on panic (`Cargo.toml:247-249`; P17).

### 3.2 L4

- `build_l4` (`analysis.rs:56-124`) runs `compute_summaries_v2_bundle_with_leaves`
  (`src/engine/l4/summary_runner.rs:1358-1677`) over the relation's primary graph and SCC.
  It is called from the det fill (`src/engine/l5/detector_context.rs:1648`) and from the
  `l4`-only fill (`session.rs:982-988`), which drops the core summaries.
- The solve builds per-id maps up front (`summary_runner.rs:1380-1477`), seeds the fixed
  leaves (`:1530-1537`), then loops over the SCCs callees first (`:1567-1654`). Each SCC
  runs the roles fixpoint (`run_one_scc_roles`) and the db-effect solve
  (`solve_scc_db_effects`), both reading earlier SCCs through one settled map (`v2_map`)
  and one growing bundle builder. After the loop the builder is frozen into the
  `SummaryBundle` (`:1674`).
- The bundle's `db_effects(r)` merges the rows in `key_rank` order, so its output order
  comes from the effects' content, not from the order they were interned
  (`src/engine/l4/effect_store.rs:794-847`; P3).

### 3.3 The front end the host reuses

- `WorkspaceInputs::read(workspace, needs)` (`inputs.rs:289-374`) reads the workspace once
  (P8).
- `AnalysisTarget::build(&inputs, single_app, command)` (`src/engine/gate/run.rs:211-217`)
  builds through `build_view_model` (`run.rs:357`), the one view builder step 12 made.
- `View::new(target)` + `prepare(&query_plan())` fills `rel` then `l4`
  (`session.rs:644-659`, `:801-809`, `:982-988`).

## 4. Design

### 4.1 Where the host lives

Two new files, both in the library, so `tests/lsp` can drive them:

- `src/engine/incremental.rs`: `IncrementalHost`, the invalidation rule, the projection.
- `src/lsp/l4_host.rs`: the worker thread, the request counter and the status record.

Plus small edits: one seeded variant of the solve's SCC loop (`summary_runner.rs`), one
`L4Request` field (`analysis.rs`), two `View` methods (`session.rs`), the flag
(`src/main.rs` → `run_server` → `RootBuild` → `build_server_state`).

```rust
// src/engine/incremental.rs
pub struct IncrementalHost {
    workspace: PathBuf,
    universe: ViewUniverse,           // src/engine/batch/job.rs:29, fixed for the host's life
    last: Option<Baseline>,           // the last COMPLETED revision only
}
struct Baseline {
    global: [u8; 32],                 // §4.3 R0
    rows: HashMap<String, [u8; 32]>,  // row digest per id
    l4: L4Analysis,                   // the old SCC, bundle, diagnostics, warnings
    core: CoreSummaries,              // the old core summaries (db_effects empty)
    diag_of: HashMap<Vec<String>, Option<SummarizeDiagnostic>>, // per old SCC key
}
pub enum RefreshError { Cancelled, Build(String) }  // the baseline is untouched in both
pub enum RefreshKind { Full(FullReason), Incremental }
pub enum FullReason { NoBaseline, Global }     // Global: R0 fired
pub struct RefreshReport {
    pub kind: RefreshKind,
    pub resolved: BTreeSet<Vec<String>>,   // re-solved SCCs, as sorted "Object.Routine" labels
    pub seeded: usize,
    pub total: usize,
}
impl IncrementalHost {
    pub fn new(workspace: &Path, universe: ViewUniverse) -> Self;
    /// Build the next revision. `cancel` is polled between phases and between SCCs.
    /// On cancel, the baseline is untouched.
    pub fn refresh(&mut self, cancel: &dyn Fn() -> bool) -> Result<RefreshReport, RefreshError>;
    /// The baseline's projection (§4.7), for the oracle tests.
    pub fn projection(&self) -> Option<L4Projection>;
}
```

`refresh`, in order:

1. If a baseline exists, compute the closure digest first
   (`semantic_golden::dependency_closure_digest`, `semantic_golden.rs:351`; P24: before
   the build, so a `.app` change that slips in makes the NEXT digest differ).
2. `WorkspaceInputs::read(&workspace, &WorkspaceNeeds::default())`. Check `cancel`.
3. `AnalysisTarget::build(&inputs, universe == SingleApp, "l4-host")`. An `Err` (no app
   id, a failed build) is returned as `RefreshError::Build`; the worker logs it at debug
   and publishes nothing; the baseline stays. Check `cancel`.
4. `View::new(target)`, then `view.prepare_l4_seeded(seed)` (new, `pub(crate)`): it fills
   `rel` exactly as `prepare` does (`session.rs:801-809`), then calls `build_l4` with
   `L4Request { extra_diagnostics: vec![], seed: Some(seed) }`, and RETURNS the core
   summaries instead of dropping them (`session.rs:986`). Same order as the `query` plan,
   filled on `&mut self` from built inputs (I3). `seed` carries the baseline, `cancel`, and
   the new rows' Δ (§4.3).
5. `view.take_l4()` (new, like `take_run`, `session.rs:1183`): moves the `L4Analysis`
   out and marks `Cell::L4` released. The view (model, relation) is then dropped.
6. Only now replace `last` with the new baseline. A cancelled or failed refresh never
   reaches this line.

The host never builds `det`, the cone or a run. It reuses the step-12 builder and the
`View` cell rules; there is no second engine. Its one new engine code is the seeded loop
(§4.4).

### 4.2 The worker, the generation check and cancellation (`src/lsp/l4_host.rs`)

```rust
pub struct HostHandle { requested: Arc<AtomicU64>, wake: mpsc::Sender<()> }
impl HostHandle { pub fn request(&self); }      // O(1): bump, then send
pub struct HostStatus { pub generation: u64, pub report: RefreshReport }
pub fn spawn_l4_host(root: PathBuf, universe: ViewUniverse)
    -> (HostHandle, Arc<Mutex<Option<HostStatus>>>, JoinHandle<()>);
```

- **What bumps the generation.** `HostHandle::request` adds 1 to `requested`. It is called
  once when the worker is spawned (the first revision) and once at the start of every
  `on_swap` call, whatever the scope. It is NOT `LspSnapshot.generation`: a config-only
  batch calls `on_swap` without a new snapshot generation (P15).
- **The loop.** The worker blocks on `wake.recv()`. It exits when every sender is gone.
  It drains queued wakes (`try_recv`), reads `g = requested.load()`, and runs
  `host.refresh(&|| requested.load() != g)`.
- **How cancellation is observed.** `refresh` calls `cancel()` after reading the inputs,
  after building the target, after the relation, and inside the solve between two SCCs
  (`summary_runner.rs:1567`, one check per loop turn). A `true` stops the refresh with
  `Err(RefreshError::Cancelled)` and leaves the baseline as it was. A `Build` error is
  logged and not published. Either way the worker then loops; the wake that
  came with the newer request is still queued, so it runs at once. Inside a phase (for
  example the program build) nothing is checked, so a cancel waits for that phase to end
  (failure shape S12).
- **What a stale result does.** A refresh that runs to the end becomes the baseline even if
  a newer request arrived after the last check: it is a settled revision of real inputs, so
  the next Δ can safely start from it (OQ4). It is **published** only when
  `requested.load() == g` at that moment. Otherwise it is dropped unpublished, and the
  worker starts the newer request. So the status record never moves backwards: it holds the
  newest generation the host finished, and only a result whose generation was current.
- **Publishing** = writing `HostStatus` into the `Mutex` and one `debug!` line with the
  report. Nothing is sent to the editor (Q1). The `Mutex` is the worker's, not a `View`'s,
  so I3 does not apply.

LSP wiring, under `--l4-host` only:

- `src/main.rs`: `#[arg(long, hide = true)] l4_host: bool`, passed to `run_server`
  (`server.rs:302`) and on to `RootBuild` (`server.rs:154-176`).
- `build_server_state` (`server.rs:575`): when the flag is set, `spawn_l4_host(root,
  ViewUniverse::CrossApp)` (OQ1), and the `on_swap` closure (`:629`) calls
  `handle.request()` as its first line. Without the flag no host, no thread, no request.
- The worker thread stops when the updater thread drops its closure (the last sender).

### 4.3 The invalidation rule

Computed once per refresh, in this order. Δ and the row digests are built in sorted id
order (I7).

- **R0 Global inputs.** `global` = sha256 over: the universe; the closure digest (step 1
  of §4.1); the `app.json` bytes; the model-instance id; `FieldIndex` as sorted
  `(table, field) -> id` entries (`analysis.rs:157-167`); `graph.collided_site_callee` as
  sorted entries; `driver_version`. If it differs from the baseline's, the refresh is
  `Full(Global)`: no SCC is seeded. This covers:
  - a `.app` add, change or remove (the closure digest; Q2);
  - a `.al` file add, remove or rename, and an `app.json` version or identity change (the
    model-instance id, P16; and the `app.json` bytes);
  - a table or table-extension field edit (`FieldIndex`).
- **R1 Δ.** For each id of the new model: its row digest (Words). Δ = added ids, removed
  ids, and ids whose digest differs. **Demand** needs no rule of its own: a cross-app edit
  that makes the model demand a dependency routine adds that routine's rows, one that stops
  demanding it removes them, and the leaf and fixed-role status is in the row (P19). The
  one global input that a demand change can move, `shared_event_ids`, is moved into the row
  as one bit per `event-dispatch` out-edge (P6). `FieldIndex` does not move with demand
  (P6).
- **R2 Reuse candidates (SCC identity + one-hop rows).** An SCC of the new relation is a
  candidate for seeding only if all three hold:
  - its key equals a baseline SCC key (a split or merged SCC has a new key, so it is
    re-solved);
  - no member is in Δ;
  - no member has a successor in Δ. This is the one-hop rule from P5: the solve reads a
    callee's model row (parameter names for event binding, `body_available`, the
    subscriber row in `dispatch_bindings`), not only its summary.
- **R3 Reverse reachability with early cutoff.** Walk the new SCC list in the solve's own
  order (callees first). An SCC is re-solved if it is not a candidate, or if any successor
  SCC was re-solved AND came out different from the baseline. Otherwise it is seeded. "Came
  out different" means: a member is new, or a member's core `RoutineSummary` differs from
  the baseline's, or its db effects differ (the set of `(identity, via)` pairs read from
  the builder's row and the growing universe, against the old bundle's `db_effects`). So
  the re-solved set is the reverse cone of Δ's one-hop callers, cut where a re-solved SCC
  came out equal (Q7).
- **R4 Config files.** `roots.config.json` changes only the model's root classes (P7); it
  is not in the row or the global digest, so it dirties no SCC. `al-sem.coverage.yaml` and
  the policy file are not read by the host at all. `.al-sem.json` is the LSP's diagnostics
  config and stays the LSP's (13a). `app.json` is a global input (R0). So the config-file
  rule is: `app.json` → Full; `roots.config.json` → a refresh with no SCC re-solved; the
  others → nothing.

Over-invalidation is allowed, under-invalidation is not. The `Debug` text holds fields
the solve may not read (`file`, `site_exprs`, `source_anchor`, `declaration_start_line`,
the object's globals), so an edit can dirty a routine whose L4 input did not change (for
example a line shift in the same file). That costs a re-solve, never a wrong answer (OQ8).

### 4.4 The seeded solve (settled-summary reuse, Q3)

`L4Request` gains `seed: Option<L4Seed<'a>>` (`analysis.rs:43-45`; every production
caller passes `None`, so the default path is the code that runs today).
`build_l4` passes it to a new `compute_summaries_v2_bundle_seeded`, and the existing
`compute_summaries_v2_bundle_with_leaves` becomes that function with `None`. Everything
before the loop (`summary_runner.rs:1371-1565`) is unchanged: the same per-id maps, the
same base summaries for every routine, the same interner, the same fixed-leaf seeding. Only
the loop body at `:1567` gains one branch:

- `if cancel() { return Err(Cancelled) }`.
- Decide R2/R3 for this SCC.
- **Seeded**: for each member, insert the baseline's core summary into `v2_map` (as the
  solve would: `db_effects` empty, `summary_runner.rs:1643-1651`), and push its bundle row
  from the baseline bundle: read `old.bundle.db_effects(old_ix)`, intern each identity into
  the growing universe, split terminal and parameter-dependent effects, and `push_row`.
  This is the body of `seed_fixed_leaf_rows` (`db_effect_solver.rs:1657-1690`), factored
  into one helper both callers use. Re-push the baseline's diagnostic for this key, at the
  same place in the loop, so `summarize_diagnostics` keeps its order. `continue`.
- **Re-solved**: run the loop body as today, then record whether the SCC came out
  different (R3).

The leaf map is not touched, so the dependency-leaf contract (epic §2) holds. The host
retains, between revisions: one digest per id, the old `L4Analysis` (bundle and SCC) and
the old core summaries. It retains no model, program or relation (Q3).

### 4.5 Determinism (I7)

Δ, the SCC walk and every seeded push run in sorted id order or the solve's own order.
Digests hash `Debug` text, which has no hash-map order in it (P4). What may differ between
incremental and fresh is internal numbering only: `EffectId` and `EffectSetId` (interning
order; `SolveIx` is the same, the interner is built the same way). The oracle (§4.7)
compares only projections that do not expose those numbers, and §6 lists the leak as a
failure shape.

### 4.6 Memory

Off by default, so no default path retains anything new. With `--l4-host`, a root holds its
LIGHT snapshot, the baseline (§4.4), and during a refresh one FULL build. This is the
retention Q3 allows, for the host only. It is not measured here (non-speed; OQ7).

### 4.7 The projection the oracle compares

`L4Projection` (`pub`, `Debug + PartialEq`), built from an `L4Analysis` and its
`CoreSummaries`:

- per id (`BTreeMap`): the core `RoutineSummary` (`PartialEq` already,
  `src/engine/l4/summary.rs:294`) and its owned db effects, `db_effects(ix).map(to_owned)`;
  `None` when the id has no bundle row;
- the partition of ids by equal `terminal_base` (the groups the reverse index reads),
  as sorted groups of ids, never as `EffectSetId` numbers;
- `summarize_diagnostics` and `warnings`, in order;
- the SCC list (keys in order) and its `recursive` flags.

## 5. Tests and discrimination proofs

Every test below lists its hand-stated precondition, its break, the assertion that fails,
and the revert (I9). Breaks are made with Edit and reverted with Edit; a test does not pass
until its break was seen to fail. File set rule for the minimality cases: each routine
whose edit matters sits in its own `.al` file, and the file set never changes between
revisions (P16), so line shifts and the instance id cannot dirty an unrelated routine.

### 5.1 The oracle harness (`tests/lsp/l4_host_oracle.rs`, new member of `tests/lsp/main.rs`)

For each case: write the r0 sources into a tempdir, `host.refresh(&never)` (assert
`Full(NoBaseline)`), apply the edit with `std::fs::write` after asserting the old text
holds the replaced snippet exactly once, `host.refresh(&never)`, then build a SECOND, fresh
host on the same directory and refresh it once. Assert:

1. **oracle**: `host.projection() == fresh.projection()`;
2. **kind**: the report's kind is the hand-stated one;
3. **minimality**: `report.resolved` equals the hand-stated label set, and is a strict
   subset of all SCCs whenever the kind is `Incremental`.

The fresh side's L4 is today's code: a host with no baseline passes `seed: None`, which is
the unchanged solve. One extra test, **T0**, ties it to the command path: on each fixture,
the fresh host's bundle part, diagnostics and SCC equal those that a plain
`View::new` + `prepare(&query_plan())` leaves in `view.l4()`.

`tests/lsp/main.rs` gains the `#[path]` lines for `../common/symbol_app.rs` and
`../common/dep_dispatch_fixture.rs` (as `tests/cli/main.rs:12-22`).

### 5.2 The edit cases

Single-app cases run in both universes (a cross-app host on a workspace with no dependency
is the single-app answer, `run.rs` cross-app contract); T1i and T1j are cross-app.

| Case | Hand-stated r0 (one routine per file) | Edit | Kind; re-solved (hand-stated) | Break → failing assertion |
|---|---|---|---|---|
| T1a body | `A→B→L`, `L` inserts `Cust`; unrelated `C` | `L` also modifies `Cust` | Incremental; {L},{B},{A} | B1: drop R3's "a successor SCC came out different" clause → `A` seeded (`B` is dirty only by the one-hop rule, `A` only by R3); `A`'s db effects lack the modify; oracle fails |
| T1b body, no change at L4 | same | `L` gains `i := 1;` (an `Integer` local) | Incremental; {L},{B} (B by the one-hop rule; A and C seeded by the cutoff) | B2: drop the "came out different" test (always different) → {L},{B},{A}; minimality fails |
| T1c signature | `B` calls `L(x)` | `L(x)` → `L(x; y)`, `B`'s call updated | Incremental; {L'},{B},{A} | B3: R2 keeps only the key match (drop both Δ conditions) → `B` is seeded with its old summary, whose inherited effects carry the old `L` id in their `operation_id`; oracle fails |
| T1d edge removal | `A→L`, `A→M` | delete `A`'s call to `L` | Incremental; {A} | B3 again → `A` seeded and keeps `L`'s effect; oracle fails. LIMIT: removing `edges_by_from` from the row alone is expected to stay green here, because deleting the call also changes `A`'s model row (`call_sites`); record that run as a redundancy, not a pass |
| T1e SCC split | `A↔B`, `R→A` | delete `B→A` | Incremental; {A},{B},{R} | B5: SCC key = the SCC's index in the list → the old `{A,B}` slot seeds `{A}`; oracle fails (`in_recursive_cycle`) |
| T1f SCC merge | `A→B`, `R→A` | add `B→A` | Incremental; {A,B},{R} | B5 |
| T1g event param rename (D-1) | publisher `P` with a `var Rec` parameter raises an event; subscriber `S` (own file) binds `Rec` by name and modifies it | rename `S`'s parameter so it no longer matches by name | Incremental; {S},{P} | B6: drop the one-hop rule from R2 → `P` seeded; oracle fails. Preconditions asserted on the fresh hosts: `S`'s core summary and db effects are EQUAL at r0 and r1, and `P`'s DIFFER. Without both, the case does not test D-1 |
| T1h file add | any | add a new `.al` file | Full(Global) | B7: drop the instance id from R0 → Incremental; the kind assertion fails. LIMIT: the oracle holds either way, since every id is new and no key matches (P16) |
| T1i demand (cross-app) | `dep_dispatch_fixture::write_own` | r1: delete `Lib.Outer();` from `CallChain`; r2: put it back | Incremental at r1 with {CallChain}; Incremental at r2 with {Inner},{Outer},{CallChain} | B8: build the new core map from every baseline id (not only the seeded SCCs' members) → the r1 projection still holds `Outer` and `Inner`; oracle fails. Preconditions: the r1 model has no `Outer`/`Inner` row, r0 and r2 have both |
| T1j `.app` change (cross-app) | `write_own` | `write_dependency(.., revision 1)` | Full(Global) | B9: drop the closure digest from R0 → Incremental; the kind assertion fails. LIMIT: the revision changes bytes only, so the oracle holds either way; this case pins the Q2 rule, not a wrong answer |
| T1k table field | table `T`, `A` reads `T."F"` while `F` does not exist | add field `F` to `T` | Full(Global) | B10: drop `FieldIndex` from R0 → nothing is in Δ, `A` is seeded with the unresolved field; oracle fails. Precondition: `A`'s core summary differs between the fresh r0 and r1 hosts |
| T1l `roots.config.json` | any, with an entry point | add a `roots.config.json` | Incremental; {} (nothing re-solved) | B11: put the config bytes in R0 → Full; kind fails |
| T1m no edit | any | none | Incremental; {} | the P3 gate in its oracle form (T2) |

### 5.3 T2: the seeded solve alone (lib test, `summary_runner.rs`)

Plan task 1, before any other code (the P3 gate). Over the r0 corpus fixtures and the
cross-app fixture: solve fresh, then solve again with every SCC seeded from the first
solve. The projection equals the fresh one. Break: in the shared seeding helper, push only
terminal effects (drop the parameter-dependent half) → the projections differ on a
fixture with a parameter-dependent effect (precondition asserted: at least one routine
has one).

### 5.4 T3: cancellation keeps the baseline (`tests/lsp`)

`refresh` with a `cancel` that returns `true` on its k-th call (k chosen so the stop falls
inside the SCC loop; asserted by a call count above the number of phase checks). Assert
`Err(RefreshError::Cancelled)` and that `projection()` is unchanged. Then refresh with `never`: oracle
holds against a fresh host. Break: assign `last` before the solve (§4.1 step 6 moved up) →
the projection after the cancelled call differs; the first assertion fails.

### 5.5 T4: the worker (`src/lsp/l4_host.rs` lib tests)

- **T4a publish rule.** The publish decision is one pure function,
  `publish(requested_now, g) -> bool`. Table test. Break: `>=` instead of `==` → the
  stale row fails.
- **T4b end to end.** Spawn the worker on a fixture, call `request()` three times
  without waiting, then wait (poll with a timeout, no fixed sleep) until the status holds
  generation 4 (1 at spawn + 3). Assert its projection equals a fresh host's. Break: read
  `g` before draining the queue → the worker refreshes for generation 2 and later
  publishes 2 after 4 was requested; the generation assertion fails.

### 5.6 T5: off by default, on with the flag (`src/server.rs` tests)

- **T5a** Without the flag, after a `didSave` round trip (the existing
  `dispatch_prepare_and_outgoing_then_didsave_bumps_generation_and_republishes_diagnostics`
  shape): the `host.refresh` counter (§7) is 0. Break:
  spawn the host unconditionally → counter > 0.
- **T5b** With the flag: after the same round trip, the status reaches a generation ≥ 2
  (spawn + one `on_swap`). Break: delete the `request()` line in `on_swap` → the status
  stays at 1; the wait times out.
- Both check that the client received no message the default server would not send
  (Q1: the host publishes nothing).

### 5.7 Unchanged tests

The 8 golden targets, `cargo test -p al-sem --lib`, `tests/lsp/lsp_incremental_parity.rs`
and every server test stay green with no edit (I2). The solve's default path is the same
code with `seed: None`.

## 6. False-positive and failure shapes

- **S1 stale caller.** A routine seeded although something it reads changed. Causes: a
  solve read outside the row and the globals (S4), or a reach beyond one hop. Guards: P5's
  read list, the one-hop rule, T1a-T1g.
- **S2 half-settled baseline.** A cancelled refresh leaving a half-written baseline.
  Guard: the baseline is replaced only at §4.1 step 6; T3.
- **S3 internal numbering leak.** Incremental and fresh differ only in `EffectId` or
  `EffectSetId` numbers, and some reader exposes them. Guard: the projection hides them
  (§4.7); a reader that uses the numbers as output is a bug of its own.
- **S4 digest blind spot.** A solve input that is neither in the row nor in R0. The most
  likely silent bug. Guards: the row is defined from the solve's own reads (P5), the R0
  list is closed, and each oracle case varies one input class.
- **S5 closure A-B-A.** A `.app` swapped to new bytes and back between the digest and the
  program build: the digest matches the baseline while the build read other bytes. Not
  guarded. Improbable; stated.
- **S6 a host panic ends the LSP.** The shipped binary aborts on panic (P17). Hidden flag,
  off by default; stated, not fixed.
- **S7 stdout.** Anything the host's build path printed to stdout would corrupt the LSP's
  JSON-RPC stream. P18 found none; T5b would show a corrupted stream as a failed round trip.
- **S8 memory.** One FULL build and the baseline beside the LIGHT snapshot (§4.6). Opt-in.
- **S9 starvation.** Under edits faster than one refresh, every refresh is cancelled and
  none publishes. The updater's debounce bounds the request rate; stated.
- **S10 CPU contention.** The host's build shares the rayon pools with the updater's
  rebuilds, so rung latencies can grow while a refresh runs. Opt-in; `perf_bounds` runs
  without the flag.
- **S11 Full on file add or remove.** Not a bug: every id changes (P16).
- **S12 cancel latency.** A cancel inside the program build waits for the build to end.

## 7. Measurement plan (non-speed)

- No bench (Decision 58).
- Counters, through `pt::counter_delta`: `host.refresh` (+1 per refresh that reaches the
  solve), `host.full` (+1 per Full), `host.sccs_resolved` and `host.sccs_seeded` (per SCC).
  `build.summaries_v2` keeps counting one per solve, seeded or not. The counters are what
  T5a reads; the tests read the `RefreshReport`.
- The evidence for acceptance is the oracle tests (§5), not numbers.
- Branch gate: `scripts/ci-steps all` and `bash scripts/cdo-gate` (I10). The CDO identity
  check (SARIF at `--scope primary` and `--scope all`, the `aldump` histogram SHA) must be
  identical base vs head: the default path cannot reach the host.

## 8. Acceptance matrix

| # | Acceptance item | Proved by |
|---|---|---|
| A1 | incremental == fresh after a **body** edit | T1a (L4 change), T1b (no L4 change) |
| A2 | … after a **signature** edit | T1c |
| A3 | … after **edge removal** | T1d |
| A4 | … after an **SCC split / merge** | T1e, T1f |
| A5 | … after **dependency** edits | `.app` change: T1j, as a Full refresh (Q2); demand change: T1i, incremental |
| A6 | invalidation follows **reverse reachability** | T1a, T1b (cutoff, Q7), T1g (one-hop rows) |
| A7 | … **SCC** identity | T1e, T1f |
| A8 | … **demand** | T1i |
| A9 | … **config files** | T1l (`roots.config.json`: nothing re-solved). `app.json`: in R0 (Full); pinned through the model-instance id it feeds (T1h); its own bytes term is defence in depth with no separate test (an `app.json` edit that keeps the version changes rows anyway, e.g. `preprocessorSymbols`), stated as a LIMIT. `al-sem.coverage.yaml` and the policy file: **not deliverable as an invalidation, because the host never reads them** (P7) |
| A10 | settled summaries reused (Q3) | T2, T1m |
| A11 | generation-checked | T4a, T4b |
| A12 | cancellable, baseline kept | T3 |
| A13 | L4 work off `on_swap`, behind hidden `--l4-host`, no editor output (Q1) | T5a, T5b |
| A14 | default paths unchanged | §5.7; CDO identity check (§7) |
| A15 | non-speed: no bench | §7 |
| A16 | both universes | §5.2's universe rule |

## 9. Fallbacks (stated before the work)

| F# | If | Then |
|---|---|---|
| F1 | T2 fails: a fully seeded solve does not project equal to a fresh one | (a) seed only `v2_map` (the core summaries) and run `solve_scc_db_effects` for EVERY SCC, so the bundle is always fresh and only the roles fixpoint is reused; if T2 still fails, (b) reuse at revision level only: R0 equal and Δ empty → reuse the whole baseline `L4Analysis`; anything else → Full. A6/A7/A8 are then restated as "a no-op refresh solves nothing" and the panel is told |
| F2 | an oracle case fails although R2/R3 were applied as written (a solve read beyond one hop) | widen R2 to "no member reaches Δ through any chain of successors" (full reverse reachability, cutoff kept for summaries only); if that still fails, Full on any non-empty Δ, and A6 is restated |
| F3 | the in-loop `cancel()` needs more than the one new parameter of §4.4 | check `cancel` only between phases (before and after the solve); S12 then covers the solve too |
| F4 | an oracle case finds a global input missing from R0 | add it to R0 (a Full refresh), with its own T1 row and break |
| F5 | `take_l4` breaks a `View` cell rule the panel holds | the baseline keeps the whole `View` with every cell but `l4` released (more retention, stated in §4.6) |
| F6 | T1i shows a demand change moving `collided_site_callee` | T1i becomes Full; A5's and A8's demand half is restated as oracle-only |
| F7 | T5b or T4b is timing-flaky | the test waits on the status with a timeout and a condition, never a fixed sleep; if still flaky, T5b moves to a lib test that calls the `on_swap` closure directly |

## 10. What changes, file by file

- `src/engine/incremental.rs` (new): host, baseline, R0-R3, projection.
- `src/lsp/l4_host.rs` (new): worker, `HostHandle`, `HostStatus`, the publish rule.
- `src/engine/l4/summary_runner.rs`: the seeded entry and the one loop branch.
- `src/engine/l4/db_effect_solver.rs`: `seed_fixed_leaf_rows`'s body factored into a
  shared row seeder (byte-neutral for the leaf path).
- `src/engine/l4/analysis.rs`: `L4Request.seed`; `build_l4` passes it on.
- `src/engine/session.rs`: `prepare_l4_seeded`, `take_l4`.
- `src/main.rs`, `src/server.rs`: the hidden flag and the `request()` line.
- `tests/lsp/main.rs`, `tests/lsp/l4_host_oracle.rs`; `tests/common/dep_dispatch_fixture.rs`
  only if T1i needs a helper it does not export.
- `CHANGELOG.md`: one `Added` entry (hidden flag, no user-visible change).
- `docs/OUTSTANDING.md:705-712`: the "future incremental L4 path" item is closed.

## 11. Open questions for the panel (with recommendations)

- **OQ1 Which universe does `--l4-host` use?** Recommend: cross-app only (analyze's
  default; with no dependency it equals single-app). A value flag
  (`--l4-host=single-app`) is cheap but has no user.
- **OQ2 Is the one-hop rule (R2) right, or too coarse?** It dirties every caller of a
  routine whose row changed, so the cutoff starts one hop later (T1b re-solves `B`). A
  finer rule would digest only the part of a callee row the solve reads (parameters,
  `body_available`, presence). Recommend: the coarse rule now; the finer one has an
  under-invalidation risk (S4) and no consumer needs its speed.
- **OQ3 Should the watcher forward `roots.config.json` edits** (today dropped,
  `watcher.rs:165-171`)? Recommend: no. It reaches no L4 input (P7), and nothing reads the
  host's other cells (Q1). Revisit with the editor-diagnostics issue.
- **OQ4 Does a completed but stale refresh become the baseline?** Recommend: yes (§4.2). It
  is a settled revision of real inputs, and it shortens the next Δ.
- **OQ5 Should the diagnostics recompute also leave `on_swap`** (the pre-draft's worker)?
  Recommend: no, not in this issue. It is not L4 work, it changes editor timing for every
  user, and the issue scopes only L4 work.
- **OQ6 Accept Full on every `.al` file add, remove or rename** (P16, D-2)? Recommend: yes.
  Reuse would need ids that do not embed the file list, which is an id-schema change with
  golden churn and no consumer.
- **OQ7 Report re-solved counts on CDO** (the pre-draft's P13)? Recommend: no, not in this
  step (non-speed; no CDO consumer). File it with the editor-diagnostics issue if wanted.
- **OQ8 Over-invalidation from positional fields in the row digest** (§4.3 end). Recommend:
  accept; the fixtures put one routine per file. Projecting the fields out needs proof they
  are unread, which is S4's risk.
- **OQ9 A host panic ends the editor server** (S6). Recommend: accept for a hidden, off
  flag; a subprocess host is a daemon, which the epic rules out.

## 12. Corrections to the pre-draft and the epic

- Epic row 13 cites `updater.rs:1206-1229`; the code is now `updater.rs:1465-1566` (P20).
- Pre-draft "Generation = `LspSnapshot.generation`": wrong; the host keeps its own counter
  (P15).
- Pre-draft §4.3 rule 3 (re-solve when a successor's summaries changed): unsound for
  event binding by name; R2's one-hop row rule replaces it (P5, D-1).
- Pre-draft T1h ("file add/remove: per case"): every id changes, so it is Full (P16, D-2).
- Pre-draft §4.3 rule 0 kept `shared_event_ids` global: it moves with demand, so it moves
  into the row (P6).
- Pre-draft §4.4 moved the diagnostics recompute to a worker: dropped (OQ5).
- The pre-draft's salsa and watcher sections were 13a's and are done.

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

Revision r1 (2026-10-25): spec panel round 1 (sol S1-S13, Opus O1-O19). Every finding was
checked against the code at `0fff74b1`; §0.1 maps each change to its finding ids. The
register is `.agent/issue-222/findings.json`.

## 0.1 Changes in r1

| Change | Where | Findings |
|---|---|---|
| The host ALWAYS calls the seeded solve entry; a Full refresh passes an empty baseline. So `cancel` is polled between SCCs on every refresh, the first one included, and every solve records its per-SCC diagnostics | §4.1 step 4, §4.4 | O3, S2 |
| Seed only recomputed members (`is_recomputed`); fixed leaves and ids with no model row are left exactly as the fresh loop leaves them; corrected "core `db_effects` empty" (a fixed leaf keeps its effects) | §4.4 | O1, S5 |
| Row digests, Δ and the solve-side global inputs are computed INSIDE the seeded entry, from the maps the solve itself builds, after `rel` exists; the host adds only the host-side globals | §4.3, §4.4 | O4 |
| Closure digest on every refresh; a digest error is `RefreshError::Build`; `driver_version` dropped from R0 | §4.1 step 1, §4.3 R0 | S1, O12, O13 |
| One mutex holds the request counter and the published status, so "check current, then publish" and "bump" cannot interleave; worker loop generic over its refresh closure | §4.2 | S3, O10 |
| `ServerState` owns the host handle and status; flag plumbing pinned by an `Args` parse test and a spawned-binary test | §4.2, §5.6 | O7, O15, S10, S11 |
| R3's db-effect comparison drops `record_variable_id` | §4.3 R3 | O18 |
| Labels carry the parameter count | §4.1 | O16 |
| T2b: the mixed seeded/re-solved gate, with a forced roles cap; T2's cross-app half moves to `tests/lsp` | §5.3 | O2, O19, S5 |
| T1c break replaced; T1e reshaped so the key rule is the only guard; T1f's key break marked redundant | §5.2 | O5, O6, S6 |
| T1i rebuilt with `CallChain` in its own file; strawman break replaced | §5.2 | O8, S7 |
| T1n (shared-event bit) and T1o (collided rows) added | §5.2 | O9, S12 |
| T1j gains a semantic dependency change; T1q (`app.json`-only edit) added; false claim about `app.json` removed | §5.2, §8 A9 | S8 |
| T1d gives `L` a distinct effect; unrelated SCCs added to T1e-T1g | §5.2 | S7 |
| T1m gets a break | §5.2 | O17 |
| T3 cancels on a second refresh after an SCC completed; break is `last.take()` | §5.4 | S9, O14 |
| T4 rebuilt on a scripted fake refresh with barriers; projection check dropped from T4b | §5.5 | S4, O10, O11, S10 |
| T5 reads `ServerState`, not tracing counters; adds a config-only (same snapshot generation) case and a blocked-solve case | §5.6, §7 | O7, S11 |
| Wording: coverage yaml is read but unused; only commands pass `control: None`; rayon pools are per build; a subprocess is not inherently a daemon; stdout guard is the spawned-binary test | §4.3 R4, §4.4, §6, §11 | S13 |
| F1 re-runs T2 and T2b; F1(b) needs panel approval | §9 | S5 |
| OQ1-OQ9 recorded as decided | §11 | panel answers |

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
- **Row digest**: sha256 of the row's `Debug` text (P4), computed inside the solve from the
  solve's own maps (§4.4).
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
- **No reuse across a file add, remove or rename, or an `app.json` change.** A file
  change or a version change renames every id (P16); any other `app.json` edit is a host
  global input (R0) by policy. The refresh is Full and only the oracle applies.
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
    host_global: [u8; 32],            // §4.3 R0, host half
    solve: SolveBaseline,             // what the solve itself recorded (§4.4)
    l4: L4Analysis,                   // the old SCC, bundle, diagnostics, warnings
    core: CoreSummaries,              // the old core summaries (as the solve left them:
                                      //   db_effects empty, except a fixed leaf's own)
}
// src/engine/l4/summary_runner.rs (pub(crate)): recorded by EVERY host solve.
pub(crate) struct SolveBaseline {
    solve_global: [u8; 32],           // §4.3 R0, solve half
    rows: HashMap<String, [u8; 32]>,  // row digest per id
    diag_of: HashMap<Vec<String>, Option<SummarizeDiagnostic>>, // per SCC key
}
pub enum RefreshError { Cancelled, Build(String) }  // the baseline is untouched in both
pub enum RefreshKind { Full(FullReason), Incremental }
pub enum FullReason { NoBaseline, HostGlobal, SolveGlobal }  // which half of R0 fired
pub struct RefreshReport {
    pub kind: RefreshKind,
    pub resolved: BTreeSet<Vec<String>>,   // re-solved SCCs, as sorted labels
                                           //   "Object.Routine/<parameter count>"
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

1. Compute the closure digest, on EVERY refresh, the first one included
   (`semantic_golden::dependency_closure_digest`, `semantic_golden.rs:351`). It runs before
   the build (P24), so a `.app` change that slips in makes the NEXT digest differ. Its
   `Err` (a cache folder that cannot be listed, a file that cannot be read) is returned as
   `RefreshError::Build`; the baseline stays.
2. `WorkspaceInputs::read(&workspace, &WorkspaceNeeds::default())`. Compute the host half
   of R0 (§4.3). Check `cancel`.
3. `AnalysisTarget::build(&inputs, universe == SingleApp, "l4-host")`. An `Err` (no app
   id, a failed build) is returned as `RefreshError::Build`; the worker logs it at debug
   and publishes nothing; the baseline stays. Check `cancel`.
4. `View::new(target)`, then `view.prepare_l4_seeded(control)` (new, `pub(crate)`). It
   fills `rel` exactly as `prepare` does (`session.rs:801-809`), then calls `build_l4`
   with `L4Request { extra_diagnostics: vec![], control: Some(control) }`, and RETURNS
   the core summaries and the new `SolveBaseline` instead of dropping them
   (`session.rs:986`). Same order as the `query` plan, filled on `&mut self` from built
   inputs (I3). `control` = `cancel` plus `reuse: Option<&Baseline>`. The host ALWAYS
   passes `Some(control)`: `reuse` is `None` with no baseline or when the host half of R0
   changed, which makes the solve seed nothing (O3). The rows and Δ are not computed
   here: `rel` does not exist yet when `control` is built, so the solve computes them
   itself (§4.4; O4).
5. `view.take_l4()` (new, like `take_run`, `session.rs:1183`): moves the `L4Analysis`
   out and marks `Cell::L4` released. The view (model, relation) is then dropped.
6. Only now replace `last` with the new baseline. A cancelled or failed refresh never
   reaches this line, and nothing before it touches `last`.

The host never builds `det`, the cone or a run. It reuses the step-12 builder and the
`View` cell rules; there is no second engine. Its one new engine code is the seeded loop
(§4.4).

### 4.2 The worker, the generation check and cancellation (`src/lsp/l4_host.rs`)

```rust
struct Shared { requested: u64, status: Option<HostStatus> }  // ONE lock for both
pub struct HostHandle { shared: Arc<Mutex<Shared>>, wake: mpsc::Sender<()> }
impl HostHandle {
    pub fn request(&self);                       // lock; requested += 1; unlock; send
    pub fn status(&self) -> Option<HostStatus>;  // lock; clone
}
#[derive(Clone)] pub struct HostStatus { pub generation: u64, pub report: RefreshReport }
/// The loop, generic over its refresh so tests can script it (T4).
pub(crate) fn run_worker(
    shared: Arc<Mutex<Shared>>, wake: mpsc::Receiver<()>,
    refresh: impl FnMut(&dyn Fn() -> bool) -> Result<RefreshReport, RefreshError>);
pub fn spawn_l4_host(root: PathBuf, universe: ViewUniverse) -> (HostHandle, JoinHandle<()>);
```

- **What bumps the generation.** `HostHandle::request` adds 1 to `requested` under the
  lock, then sends one wake. It is called once when the worker is spawned (the first
  revision) and once at the start of every `on_swap` call, whatever the scope. It is NOT
  `LspSnapshot.generation`: a config-only batch calls `on_swap` without a new snapshot
  generation (P15).
- **The loop.** The worker blocks on `wake.recv()`. It exits when every sender is gone.
  It drains queued wakes (`try_recv`), THEN reads `g = requested` under the lock, and runs
  `refresh(&|| lock().requested != g)`. Draining before reading `g` means no wake is left
  for a request older than `g`; a request after the read leaves its wake queued.
- **How cancellation is observed.** On every refresh, the first one included (§4.1
  step 4), `cancel()` is called after the closure digest and the inputs, after the target,
  after the relation, and inside the solve at the top of every SCC turn
  (`summary_runner.rs:1567`). A `true` stops the refresh with
  `Err(RefreshError::Cancelled)` and leaves the baseline as it was. A `Build` error is
  logged and not published. Either way the worker then loops; the wake that came with the
  newer request is still queued, so it runs at once. Inside a phase (for example the
  program build) nothing is checked, so a cancel waits for that phase to end (S12).
- **What a stale result does.** A refresh that runs to the end becomes the baseline even if
  a newer request arrived after its last check: it is a settled revision whose recorded
  inputs (digests) were taken from the same build as its result, so the next Δ can safely
  start from it (OQ4). It is **published** only under the one lock: `lock; if requested ==
  g { status = Some(..) }; unlock`. `request` takes the same lock to bump, so a bump either
  happens before the check (the result is dropped unpublished and the queued wake starts
  the newer request) or after the write (the status then names `g`, which was current when
  it was written, and the newer request follows). So the status never names a generation
  that was already superseded when it was written, and it never moves backwards (S3).
- **Publishing** = the guarded write above and one `debug!` line
  (`l4-host: published generation <g>: <report>`). Nothing is sent to the editor (Q1). The
  lock is the worker's, not a `View`'s, so I3 does not apply. The cancel closure takes the
  same lock once per SCC turn; it is uncontended except while a request bumps it.

LSP wiring, under `--l4-host` only:

- `src/main.rs`: `#[arg(long, hide = true)] l4_host: bool`, passed to `run_server`
  (`server.rs:302`) and on to `RootBuild` (`server.rs:154-176`). Its doc comment says a
  host crash ends the server (S6).
- `build_server_state` (`server.rs:575`): when the flag is set, `spawn_l4_host(root,
  ViewUniverse::CrossApp)` (OQ1), and the `on_swap` closure (`:629`) calls
  `handle.request()` as its first line (it holds a clone of the handle).
- `ServerState` (`server.rs:117`) gains `l4_host: Option<(HostHandle, JoinHandle<()>)>`:
  `None` without the flag. Tests read the status through it (T5).
- The worker thread stops when the last `HostHandle` (the server's and the updater
  closure's) is dropped. Like the updater thread it is not joined at shutdown.

### 4.3 The invalidation rule

Computed once per refresh, in this order. Δ and the row digests are built in sorted id
order (I7). The host computes only R0's host half. Everything else is computed inside the
seeded solve entry, from the maps the solve already builds (`summary_runner.rs:1380-1477`:
`RowsOfId`, `edges_by_from`, `uncertainty_edges_by_from`, `upgraded_bindings`,
`leaf_summaries`, `fixed_roles`, `shared_events`), so the digest reads exactly what the
solve reads and there is no second reader to drift (O4).

- **R0 Global inputs**, in two halves. Either half differing from the baseline's makes the
  refresh Full: no SCC is seeded.
  - **Host half** (`Baseline.host_global`, §4.1 step 2): sha256 over the universe, the
    closure digest (§4.1 step 1), the `app.json` bytes and the model-instance id. When it
    differs the host passes `reuse: None` (`Full(HostGlobal)`). It covers a `.app` add,
    change or remove (Q2); a `.al` file add, remove or rename and an `app.json` version or
    identity change (the instance id, P16); and any other `app.json` edit, even one that
    changes no row (for example a new `description`: T1q).
  - **Solve half** (`SolveBaseline.solve_global`): sha256 over `FieldIndex` as sorted
    `(table, field) -> id` entries (`analysis.rs:157-167`) and
    `graph.collided_site_callee` as sorted entries. When it differs the solve seeds nothing
    (`Full(SolveGlobal)`). It covers a table or table-extension field edit.
  - `driver_version` is not in R0: the solve does not read it, and a test can override it
    through an environment variable (`gate/version.rs:37-39`; O13). The universe and the
    build profile are fixed for a host's life.
- **R1 Δ.** For each id of the new model: its row digest (Words). Δ = added ids, removed
  ids, and ids whose digest differs. A collided id (several model rows, one id) digests ALL
  its rows in model order, not only the last one, because the base summary is the union of
  every row (`summary_runner.rs:1422-1432`; T1o). **Demand** needs no rule of its own: a cross-app edit
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
  the baseline's, or its db effects differ. Db effects are compared as SETS of
  `(op, table_id, operation_id, temp state, via)`, read from the builder's row through the
  growing universe on one side and from the old bundle's `db_effects` on the other.
  `record_variable_id` is left out: the builder's identity has none, and the bundle fills it
  from the run's own `rvid_by_opid` map at projection time (`effect_store.rs:849-866`,
  `db_effect_solver.rs:1227-1239`; O18). A fixed-leaf SCC and an SCC of ids with no model
  row are never solved by the loop (§4.4); one "came out different" exactly when a member is
  in Δ (a leaf's summary is in its row), and its callers are then dirty by R2's one-hop
  rule anyway. So the re-solved set is the reverse cone of Δ's one-hop callers, cut where
  a re-solved SCC came out equal (Q7).
- **R4 Config files.** `roots.config.json` changes only the model's root classes (P7); it
  is not in the row or the global digest, so it dirties no SCC. `al-sem.coverage.yaml` is
  read by `WorkspaceInputs::read` with these needs (`inputs.rs:334-346`) but used only for
  fingerprint input rows, which the host never builds; the policy file is a request input
  the host never reads. `.al-sem.json` is the LSP's diagnostics config and stays the LSP's
  (13a). So the config-file rule is: `app.json` → Full (R0 host half);
  `roots.config.json` → a refresh with no SCC re-solved; the others → nothing.

Over-invalidation is allowed, under-invalidation is not. The `Debug` text holds fields
the solve may not read (`file`, `site_exprs`, `source_anchor`, `declaration_start_line`,
the object's globals), so an edit can dirty a routine whose L4 input did not change (for
example a line shift in the same file). That costs a re-solve, never a wrong answer (OQ8).

### 4.4 The seeded solve (settled-summary reuse, Q3)

`L4Request` gains `control: Option<L4Control<'a>>` (`analysis.rs:43-45`). Every command
passes `None`; only the host passes `Some`, on every refresh (§4.1 step 4). With `None`,
`build_l4` calls today's function unchanged. With `Some`, it calls a new
`compute_summaries_v2_bundle_seeded`, which returns the bundle, the core map, the
diagnostics, and the new `SolveBaseline`; `compute_summaries_v2_bundle_with_leaves`
stays a separate entry with no control, so no command can reach the new branch. T0
(§5.1) pins that the seeded entry with nothing to reuse equals the `None` path.

Inside the seeded entry, everything before the loop (`summary_runner.rs:1371-1565`) is
unchanged: the same per-id maps, the same base summaries for every routine, the same
interner, the same fixed-leaf seeding. After those maps exist, and before the loop, it
computes the solve half of R0 and every row digest (§4.3), and Δ against
`reuse.solve.rows` (empty when `reuse` is `None` or the solve half differs, so nothing is
seeded). The loop body at `:1567` gains one branch:

- `if cancel() { return Err(Cancelled) }`, at the top of every turn, on every refresh.
- Decide R2/R3 for this SCC.
- **Seeded**: only the members with `is_recomputed(id)` (`summary_runner.rs:1484-1485`)
  are touched. For each, insert the baseline's core summary into `v2_map` (the solve
  assembles them with `db_effects` empty, `:1643-1651`), and push its bundle row from the
  baseline bundle: read `old.bundle.db_effects(old_ix)`, intern each identity into the
  growing universe, split terminal and parameter-dependent effects, and `push_row`. This
  is the body of `seed_fixed_leaf_rows` (`db_effect_solver.rs:1657-1690`), factored into
  one helper both callers use. A fixed leaf is not touched: it was put into `v2_map` with
  its own effects before the loop (`:1493-1496`) and its row was seeded there
  (`:1530-1537`); the fresh loop leaves it alone too (`run_one_scc_roles` skips it,
  `:2030-2034`). An id with no model row gets no summary and no row, as in the fresh
  loop. Re-push this key's diagnostic from `reuse.solve.diag_of`, at the same place in the
  loop, so `summarize_diagnostics` keeps its order. `continue`.
- **Re-solved**: run the loop body as today, then record whether the SCC came out
  different (R3), and record its diagnostic (or `None`) in the new `diag_of`. Every SCC
  of every host solve is recorded, so a baseline made by a Full refresh can be reused
  next time (O3).

The leaf map is not touched, so the dependency-leaf contract (epic §2) holds. The host
retains, between revisions: one digest per id, one diagnostic slot per SCC key, the old
`L4Analysis` (bundle and SCC) and the old core summaries. It retains no model, program or
relation (Q3).

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

The fresh side is a host with no baseline: it runs the seeded entry with nothing to reuse.
**T0** ties that to today's command path, in two halves:

- **T0a** (lib, `summary_runner.rs`): on every r0 corpus fixture, with and without a forced
  roles cap (`with_roles_iteration_cap`, `summary_runner.rs:70`, so the diagnostic path
  runs; precondition asserted: at least one capped SCC under the cap), and with hand-stated
  `leaf_summaries`/`fixed_roles` maps (one routine with no out-edge as a leaf, one with an
  out-edge as a fixed role; asserted), the seeded entry with `reuse: None` returns the same
  bundle projection, core map and diagnostics as `compute_summaries_v2_bundle_with_leaves`.
  Break: in the seeded entry, skip the diagnostic push of a re-solved SCC → the capped
  variant differs.
- **T0b** (`tests/lsp`): on each oracle fixture, the fresh host's bundle part, diagnostics
  and SCC equal those that a plain `View::new` + `prepare(&query_plan())` leaves in
  `view.l4()`. Break: the host builds its target with the other universe → the cross-app
  fixtures differ.

`tests/lsp/main.rs` gains the `#[path]` lines for `../common/symbol_app.rs` and
`../common/dep_dispatch_fixture.rs` (as `tests/cli/main.rs:12-22`).

A break that cannot fail because another rule also guards the same case is recorded as a
**redundancy**, with the combined break that does fail, never as a pass (CLAUDE.md).

### 5.2 The edit cases

Single-app cases run in both universes (a cross-app host on a workspace with no dependency
is the single-app answer, the cross-app contract test of CLAUDE.md); T1i and T1j are
cross-app. Every fixture below is hand-written in the test, one codeunit per routine and
one `.al` file per codeunit (so "one routine per file"), each routine's effect stated, and
an unrelated routine `U` (own file, its own effect) in every case, so the strict-subset
assertion has something to keep seeded. Labels are `Codeunit.Routine/<parameter count>`.

| Case | Hand-stated r0 | Edit | Kind; re-solved (hand-stated) | Break → failing assertion |
|---|---|---|---|---|
| T1a body | `A→B→L`; `L` inserts `Cust`; `U` inserts `Item` | `L` also modifies `Cust` (same line) | Incremental; {L},{B},{A} | B1: drop R3's "a successor SCC came out different" clause → `A` seeded (`B` is dirty only by the one-hop rule, `A` only by R3); `A`'s db effects lack the modify; oracle fails |
| T1b body, no change at L4 | same | `L` gains `i := 1;` (an `Integer` local) | Incremental; {L},{B} (B by the one-hop rule; A and U seeded by the cutoff) | B2: drop the "came out different" test (always different) → {L},{B},{A}; minimality fails |
| T1c signature | `A→B`, `B` calls `L(x: Integer)`; `L` inserts `Cust` | `L(x)` → `L(x; y: Integer)`, `B`'s call updated | Incremental; {L/2},{B/0},{A/0} | **Redundancy.** `B` is dirty three ways (its own row, its successor `L/2` is added, and `L/2` is a new SCC), so no single-rule break can fail. Combined break: R2 always true AND R3's "a member is new" removed → `B` seeded with its old summary, whose inherited effects carry the old `L` id in their `operation_id`; oracle fails. Each rule alone is pinned by T1d (own row), T1g (one hop), T1a (R3) |
| T1d edge removal | `A→L`, `A→M`; `L` inserts `Cust`, `M` inserts `Item` | delete `A`'s call to `L` | Incremental; {A} | B3: drop R2's "no member in Δ" → `A` (key `{A}` unchanged, successors `L`, `M` unchanged) is seeded and keeps the `Cust` insert; oracle fails. Redundancy: removing `edges_by_from` from the row alone stays green, because deleting the call also changes `A`'s model row (`call_sites`) |
| T1e SCC split | `A→B`, `B↔C`, `C→A` (one SCC `{A,B,C}`); `A` has no effect of its own; `B` inserts `Cust` | delete `C→A` | Incremental; {B,C},{A} | B5: drop R2's key condition → `A` (not in Δ; its one successor `B` not in Δ; `{B,C}` came out equal) is seeded with `in_recursive_cycle = true`; oracle fails. Preconditions on the fresh hosts: `B` and `C` project EQUAL at r0 and r1; `A`'s `in_recursive_cycle` is true at r0, false at r1 (O5) |
| T1f SCC merge | `A→B`; `B` inserts `Cust` | add `B→A` | Incremental; {A,B} | **Redundancy:** a merged SCC always holds the source of the new edge, which is in Δ, so the key rule never decides a merge. Combined break: R2 always true → `{A,B}` seeded from the old per-id summaries (`in_recursive_cycle = false`); oracle fails |
| T1g event param rename (D-1) | publisher `P(var Rec: Record Cust)` raises event `E`; subscriber `S` (own file) binds `Rec` by name and modifies it | rename `S`'s parameter so it no longer matches by name | Incremental; {S},{P} | B6: drop the one-hop rule from R2 → `P` seeded; oracle fails. Preconditions asserted on the fresh hosts: `S`'s core summary and db effects are EQUAL at r0 and r1, and `P`'s DIFFER. Without both, the case does not test D-1 (if roles carry the name, the case is re-shaped before code, not waived) |
| T1h file add | any | add a new `.al` file | Full(HostGlobal) | B7: drop the instance id from R0 → Incremental; the kind assertion fails. Redundancy: the oracle holds either way, since every id is new and no key matches (P16) |
| T1i demand (cross-app) | `write_own`'s dependency; the workspace rewritten so `CallChain` is alone in codeunit `Ws Chain`, file `src/Chain.al`, and `src/Work.al` is `WS_AL` without it (both replacements asserted to apply once) | r1: delete `Lib.Outer();` from `CallChain`; r2: put it back | Incremental at r1 with {Ws Chain.CallChain/0}; Incremental at r2 with {Dep Lib.Inner/0},{Dep Lib.Outer/0},{Ws Chain.CallChain/0} | Combined break: R2 always true → at r1 `CallChain` is seeded with the old inherited insert; oracle fails. **Redundancy:** demand adds and removes reach the decision only through new SCC keys and the caller's own row, which T1c and T1d pin; the demand-specific input is pinned by T1n. Preconditions: the r1 model has no `Outer`/`Inner` row, r0 and r2 have both |
| T1j dependency change (cross-app) | `write_own` | (a) `write_dependency(.., revision 1)`: bytes only; (b) the `Inner` body `Insert` → `Modify` (as `write_own_modify`): a semantic change | Full(HostGlobal) for both | B9: drop the closure digest from R0 → Incremental; the kind assertion fails in (a) and (b). Precondition for (b): the fresh projections differ between r0 and r1. **Redundancy for the oracle:** in (b) `Inner`'s own row changes, so Δ alone also gives the right answer; the closure term is the Q2 rule and the guard for changes no row shows |
| T1k table field | table `T`, `A` reads `T."F"` while `F` does not exist | add field `F` to `T` | Full(SolveGlobal) | B10: drop `FieldIndex` from R0 → nothing is in Δ, `A` is seeded with the unresolved field; oracle fails. Precondition: `A`'s core summary differs between the fresh r0 and r1 hosts |
| T1l `roots.config.json` | any, with an entry point | add a `roots.config.json` | Incremental; {} (nothing re-solved) | B11: put the config bytes in R0 → Full; kind fails |
| T1m no edit | `L` hands a `var Rec` parameter to an insert (a parameter-dependent effect; asserted on the fresh projection) | none | Incremental; {} | B12: the shared seeding helper pushes only terminal effects → `L`'s seeded row loses its parameter-dependent effect; oracle fails |
| T1n shared-event bit (P6) | `P1(var Rec: Record Cust)` raises integration event `E`; subscriber `S` (own file) binds `Rec` and modifies it | add an overload `P2` of `E` AFTER `P1` in `P1`'s file, so `E` becomes shared | Incremental; {P1},{P2} | B13: leave the shared-event bit out of the row → `P1` (rows, edges and successor `S` unchanged) is seeded with its old dispatch; oracle fails. Preconditions: fresh `P1` summaries differ between r0 and r1; `P1`'s model row `Debug` text and `S`'s row are equal at r0 and r1 (O9) |
| T1o collided rows (step 11b) | routine `K` declared in an `#if A` / `#else` pair (two rows, one id, as `detector_context_11b_tests.rs:5-6`); the FIRST row inserts `Cust`, the last inserts `Item`; `A→K` | the first row's `Insert` → `Modify`, same line | Incremental; {K},{A} | B14: digest only the LAST row of a collided id → `K` not in Δ, `A` not one-hop dirty → both seeded with the old union; oracle fails. Preconditions: two rows of one id; the last row's `Debug` text equal at r0 and r1; fresh projections differ (S12) |
| T1q `app.json` only | any | add a `description` to `app.json` | Full(HostGlobal) | B15: drop the `app.json` bytes from R0 → Incremental; kind fails. Redundancy for the oracle: no row changes, so seeding is right either way (S8) |

### 5.3 T2 and T2b: the seeded solve alone (lib tests, `summary_runner.rs`)

Plan task 1, before any other code (the P3 gate). Both run over the r0 corpus fixtures,
each in four variants: plain; under a forced roles cap (`with_roles_iteration_cap`, so the
`diag_of` re-push runs); and with hand-stated `leaf_summaries` and `fixed_roles` maps (the
cross-app solve inputs, stated on a single-app model, since a lib test cannot reach
`tests/common`; O19), with and without the cap. Preconditions asserted per variant: at
least one parameter-dependent effect, one SCC with two or more members, one leaf, one fixed
role, and (capped variants) one capped SCC. The real cross-app solve is covered by T1i and
T1j in `tests/lsp`.

- **T2 (all seeded).** Solve once with `reuse: None`, keep the result as a baseline, solve
  again with it: nothing re-solved, and the projection, core map and diagnostics equal the
  first solve's. Break B12 (as T1m) → the projections differ.
- **T2b (mixed; O2).** For every SCC `k` of the fixture: take the baseline from a fresh
  solve, delete the row digests of `k`'s members from it (so they count as changed), and
  solve again. `k` and its one-hop callers are re-solved and READ seeded callees through
  the builder (`db_effect_solver.rs:286-321`, `settled_db.pd_ids`) with a universe whose
  interning order differs from a fresh one; everything else is seeded. The projection
  equals the fresh one, for every `k`. Break: push the seeded rows after the loop instead
  of in the seeded branch → while the loop runs, a re-solved caller finds no row for a
  seeded callee (`has_row` false, `effect_store.rs:577-579`) and inherits nothing; T2b
  fails for every `k` with a seeded callee, while T2 (nothing re-solved, so nothing reads
  a seeded row during the loop) stays green. That pair is the proof T2b covers what T2
  cannot.

If T2 or T2b fails, F1 applies, and F1(a) must pass both before work goes on (§9).

### 5.4 T3: cancellation keeps the baseline (`tests/lsp`)

Fixture T1a. Refresh r0 (`never`); record `projection()` and the hidden
`baseline_fingerprint()` (sha256 over the baseline's host global, solve global and row
digests). Apply T1a's edit. Refresh with a `cancel` that returns `true` on call number
`PHASE_CHECKS + report_r0.total` (the top of the LAST SCC turn; `PHASE_CHECKS` = 3, a
hidden constant), so every earlier SCC, `L`'s re-solved one included (`L` is a callee of
`B` and `A`, so its turn comes before theirs), has completed. Assert:
`Err(RefreshError::Cancelled)`; `projection()` and `baseline_fingerprint()` unchanged;
the closure was called exactly that many times. Then refresh with `never`: Incremental,
resolved {L},{B},{A} (the r0 baseline was kept, not a half state), and the oracle holds.
Break (O14): `self.last.take()` at the start of `refresh`, restored only on success →
after the cancel `projection()` is `None`; the first assertion fails.

### 5.5 T4: the worker (`src/lsp/l4_host.rs` lib tests, on `run_worker`)

`run_worker` takes the refresh as a closure, and a test-only `after_drain` hook that runs
between "drain the wakes" and "read `g`". Each test scripts the interleaving with
barriers; no test sleeps for a fixed time; every wait has a timeout.

- **T4a stale, complete, unpublished, retained (S3, S4).** The refresh wraps a real
  `IncrementalHost` on fixture T1a: it waits on barrier 1, then calls
  `host.refresh(&never)`, then records its report. Request 1 (spawn); while refresh #1
  waits, `request()` (generation 2), then release it. Refresh #1 completes. Hold
  refresh #2 at barrier 1 and assert: `status()` is `None` (generation 1 was superseded
  before its publish check) and refresh #2's report, once released, is Incremental with
  nothing re-solved (so #1 was committed as the baseline). Then `status()` reaches
  generation 2. Breaks: (i) delete the `requested == g` guard → `status()` is generation 1
  while #2 is held; (ii) make the host commit only when `cancel()` is false at the end →
  #2 is `Full(NoBaseline)`.
- **T4b no stranded request.** The `after_drain` hook calls `request()` once. Assert the
  status reaches that request's generation. Break: swap the two statements (read `g`, then
  drain): the hook's wake is drained away, the refresh for the older `g` is cancelled at
  once, and the worker blocks; the wait times out.
- **T4c `request()` never waits on a refresh.** With refresh #1 held at a barrier,
  `request()` returns (asserted on the test thread before the barrier is released). This
  is what makes `on_swap` return while L4 work runs on the worker thread. Break: make
  `request()` take a lock that the refresh holds for its whole run → the call blocks; the
  test's timeout fails.

### 5.6 T5: off by default, on with the flag

`src/server.rs` tests (they read `ServerState`, not tracing counters: `pt::counter_delta`
is a no-op unless a tracer was set up from the environment, `perf_trace.rs:522-527`,
`:748-752`; O7):

- **T5a** Without the flag: right after `build_server_state`, `state.l4_host.is_none()`.
  Break: spawn the host unconditionally → `Some`.
- **T5b** With the flag: after a `didSave` round trip (the
  `dispatch_prepare_and_outgoing_then_didsave_bumps_generation_and_republishes_diagnostics`
  shape), `l4_host.status()` reaches generation 2 (spawn + one `on_swap`). Break: delete
  the `request()` line in `on_swap` → it stays at 1; the wait times out.
- **T5c** With the flag, config only: send `ChangeEvent::ConfigChanged` on `state.tx`.
  The snapshot generation is unchanged (asserted) and the host status reaches the next
  generation. Break: call `request()` only when the snapshot generation moved → the wait
  times out (P15).

`src/main.rs` test:

- **T5d** `Args::try_parse_from(["al-call-hierarchy", "--l4-host"])` sets `l4_host`, and
  the rendered `--help` does not mention it. Break: drop `hide = true` → the help
  assertion fails.

`tests/lsp` (spawned binary, `env!("CARGO_BIN_EXE_al-call-hierarchy")`, as
`tests/cli/cli_analyze_call_hierarchy_json.rs` spawns it):

- **T5e** Run `--lsp --l4-host --no-watcher --verbose` on fixture T1a over stdio: send
  `initialize`, `initialized`, one `prepareCallHierarchy` (so the root builds), wait for
  the stderr line `l4-host: published generation 1`, then `shutdown` and `exit`. Assert
  every stdout frame is a well-formed `Content-Length` JSON-RPC message, and the set of
  server-sent methods equals that of the same session without `--l4-host` (Q1). This pins
  the `main.rs` → `run_server` → `RootBuild` path (O15) and guards S7 on the real stdout.
  Break: drop the pass-through of `l4_host` in `main.rs` → the stderr line never comes;
  the wait times out.

### 5.7 Unchanged tests

The 8 golden targets, `cargo test -p al-sem --lib`, `tests/lsp/lsp_incremental_parity.rs`
and every server test stay green with no edit (I2). Every command still calls the solve
with `control: None`, which is today's function.

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
  JSON-RPC stream. P18 found none by grep; T5e checks the real stdout of a spawned server
  with the flag on (the in-process server tests use an in-memory `Connection` and cannot).
- **S8 memory.** One FULL build and the baseline beside the LIGHT snapshot (§4.6). Opt-in.
- **S9 starvation.** Under edits faster than one refresh, every refresh is cancelled and
  none publishes. The updater's debounce bounds the request rate; stated.
- **S10 CPU contention.** Each build makes its own local rayon pool
  (`big_stack.rs:66-75`), so the host and the updater do not share a pool, but they share
  the CPU cores; rung latencies can grow while a refresh runs. Opt-in; `perf_bounds` runs
  without the flag.
- **S11 Full on file add or remove.** Not a bug: every id changes (P16).
- **S12 cancel latency.** A cancel inside the program build waits for the build to end.

## 7. Measurement plan (non-speed)

- No bench (Decision 58).
- Counters, through `pt::counter_delta`: `host.refresh` (+1 per refresh that reaches the
  solve), `host.full` (+1 per Full), `host.sccs_resolved` and `host.sccs_seeded` (per SCC).
  `build.summaries_v2` keeps counting one per solve, seeded or not. They are for traces
  only: `counter_delta` does nothing without an environment-configured tracer
  (`perf_trace.rs:522-527`, `:748-752`), so no test reads them. The tests read the
  `RefreshReport`, `HostHandle::status()` and `ServerState.l4_host` (§5.5, §5.6).
- The evidence for acceptance is the oracle tests (§5), not numbers.
- Branch gate: `scripts/ci-steps all` and `bash scripts/cdo-gate` (I10). The CDO identity
  check (SARIF at `--scope primary` and `--scope all`, the `aldump` histogram SHA) must be
  identical base vs head: the default path cannot reach the host.

## 8. Acceptance matrix

| # | Acceptance item | Proved by |
|---|---|---|
| A1 | incremental == fresh after a **body** edit | T1a (L4 change), T1b (no L4 change) |
| A2 | … after a **signature** edit | T1c (oracle; its break is a stated combined break) |
| A3 | … after **edge removal** | T1d |
| A4 | … after an **SCC split / merge** | T1e (the key rule's only guard), T1f (oracle; key rule redundant for merges) |
| A5 | … after **dependency** edits | `.app` change, bytes only and semantic: T1j, as a Full refresh (Q2); demand change: T1i, incremental; the input demand can move: T1n |
| A6 | invalidation follows **reverse reachability** | T1a (R3), T1b (cutoff, Q7), T1d (own row), T1g (one-hop rows), T1o (collided rows) |
| A7 | … **SCC** identity | T1e |
| A8 | … **demand** | T1i, T1n |
| A9 | … **config files** | T1l (`roots.config.json`: nothing re-solved); T1q (`app.json` alone: Full). `al-sem.coverage.yaml` and the policy file: **not deliverable as an invalidation, because the host never uses them** (P7; R4) |
| A10 | settled summaries reused (Q3) | T2, T2b, T1m |
| A11 | generation-checked | T4a, T4b, T5c |
| A12 | cancellable, baseline kept | T3 |
| A13 | L4 work off `on_swap`, behind hidden `--l4-host`, no editor output (Q1) | T4c, T5a, T5b, T5d, T5e |
| A14 | default paths unchanged | §5.7; CDO identity check (§7) |
| A15 | non-speed: no bench | §7 |
| A16 | both universes | §5.2's universe rule |

## 9. Fallbacks (stated before the work)

| F# | If | Then |
|---|---|---|
| F1 | T2 or T2b fails: a seeded solve does not project equal to a fresh one | (a) seed only `v2_map` (the core summaries) and run `solve_scc_db_effects` for EVERY SCC, so the bundle is always fresh and only the roles fixpoint is reused; T0a, T2 and T2b are re-run against (a) and must pass. If they still fail, (b) reuse at revision level only: R0 equal and Δ empty → reuse the whole baseline `L4Analysis`; anything else → Full. (b) drops acceptance (A6-A8 become "a no-op refresh solves nothing"), so it needs the panel's approval before any code builds on it |
| F2 | an oracle case fails although R2/R3 were applied as written (a solve read beyond one hop) | widen R2 to "no member reaches Δ through any chain of successors" (full reverse reachability, cutoff kept for summaries only); if that still fails, Full on any non-empty Δ, and A6 is restated |
| F3 | the in-loop `cancel()` needs more than the one new parameter of §4.4 | check `cancel` only between phases (before and after the solve); S12 then covers the solve too |
| F4 | an oracle case finds a global input missing from R0 | add it to R0 (a Full refresh), with its own T1 row and break |
| F5 | `take_l4` breaks a `View` cell rule the panel holds | the baseline keeps the whole `View` with every cell but `l4` released (more retention, stated in §4.6) |
| F6 | T1i shows a demand change moving `collided_site_callee` | T1i becomes Full; A5's and A8's demand half is restated as oracle-only |
| F7 | a T4 or T5 test is timing-flaky | T4 is scripted by barriers and hooks, so a flake there is a bug to fix, not to retry. T5b, T5c and T5e wait on a condition with a timeout, never a fixed sleep; if one still flakes, it moves to a lib test that calls the `on_swap` closure directly |

## 10. What changes, file by file

- `src/engine/incremental.rs` (new): host, baseline, R0-R3, projection.
- `src/lsp/l4_host.rs` (new): worker, `HostHandle`, `HostStatus`, the publish rule.
- `src/engine/l4/summary_runner.rs`: the seeded entry (row digests, Δ, the solve half of
  R0, `SolveBaseline`) and the one loop branch; T0a, T2, T2b.
- `src/engine/l4/db_effect_solver.rs`: `seed_fixed_leaf_rows`'s body factored into a
  shared row seeder (byte-neutral for the leaf path).
- `src/engine/l4/analysis.rs`: `L4Request.control`; `build_l4` passes it on and returns
  the `SolveBaseline`.
- `src/engine/session.rs`: `prepare_l4_seeded`, `take_l4`.
- `src/main.rs`: the hidden flag and T5d. `src/server.rs`: `ServerState.l4_host`, the
  `request()` line, T5a-T5c.
- `tests/lsp/main.rs`, `tests/lsp/l4_host_oracle.rs` (T0b, T1, T3, T5e);
  `tests/common/dep_dispatch_fixture.rs` only if T1i or T1j needs a helper it does not
  export (`write_dependency_lib` is private today, `:128`).
- `CHANGELOG.md`: one `Added` entry (hidden flag, no user-visible change).
- `docs/OUTSTANDING.md:705-712`: the "future incremental L4 path" item is closed.

## 11. Open questions: decided in panel round 1

Both reviewers (sol and Opus) agreed with all nine recommendations. Each is now a decision.
Where sol added a condition, it is recorded and applied.

- **OQ1 Universe: DECIDED, cross-app only** (analyze's default; with no dependency it
  equals single-app). No value flag.
- **OQ2 One-hop rule: DECIDED, the coarse rule** (every caller of a routine whose row
  changed is re-solved). Opus re-walked every solve read and found none beyond one hop;
  sol agreed "provisionally", on condition that the key inputs are tested. Applied: T1n
  (shared-event bit) and T1o (collided rows) were added, and the row is computed from the
  solve's own maps (§4.4). F2 stays as the fallback.
- **OQ3 Watcher: DECIDED, no new mapping** for `roots.config.json`. Opus notes a `.app`
  change also reaches the host only when it produces an `on_swap` (a rung-3 rebuild);
  acceptable with no consumer.
- **OQ4 Stale but complete refresh: DECIDED, it becomes the baseline.** sol's condition,
  applied: its recorded inputs come from the same build as its result (§4.2), and it is
  never published through an unsynchronized check (one lock, §4.2; T4a).
- **OQ5 Diagnostics recompute: DECIDED, it stays on the updater thread.**
- **OQ6 File add, remove or rename: DECIDED, Full.**
- **OQ7 CDO counts: DECIDED, none in this step.**
- **OQ8 Positional over-invalidation: DECIDED, accepted.** sol's and Opus's condition,
  applied: every minimality fixture really isolates it (one routine per file; T1i
  rebuilt so `CallChain` has its own file).
- **OQ9 Host panic: DECIDED, accepted** for a hidden flag that must be passed explicitly;
  the flag's doc comment says so. sol's correction, applied: a subprocess host is not in
  itself a daemon (it would end with the server); it is not taken because it adds a
  process protocol for an experiment with no consumer, not because the epic forbids it.

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

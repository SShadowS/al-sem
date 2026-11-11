# Unresolved cones: build the snapshot indexes once (TLS-17), and what a parallel reach needs (TLS-22)

Status: TLS-17's design (sections 1-6), approved in review round 3-4 (sol, Opus); section 7 records a rejected
first design and both reviewers' findings, for ENG-18 and TLS-22 to build on. Owner request
(2026-11-11, to lane tools): refactors that speed up the whole product. Reviews:
`.agent/tls17/{pi-sol,opus}-r{1,2}.md` in the main checkout.

## 1. Problem, measured

TLS-16 traced one-shot runs on the pinned CDO baseline (`docs/superpowers/notes/2026-11-11-cdo-gate-time.md`,
"Traced"; `cdo` profile, master `4a3caaaa`). `digest.unresolved_cones` took 33.2 s of a 699-root
`impact --diff` (315 s) and 66.9 s of `impact --base` (520 s), on one thread.

`unresolved_cone(snap, root)` (`src/engine/l5/unresolved_cone.rs:98`) builds four indexes over the
WHOLE snapshot before its small breadth-first search from `root`:

- `display_by_id` over `snap.identities` (`:103-120`);
- `callsite_by_id` over `snap.callsite_index` (`:123-127`);
- `outgoing` over `snap.typed_edges`, each bucket sorted (`:131-142`);
- `ledger_by_routine` over `snap.callsite_resolutions` (`:145-154`).

They depend only on the snapshot, yet its one production caller (`digest_cli.rs:2022`, inside the
`digest.unresolved_cones` span at `:1994`, in `FinishCx::finish`) calls it once per root: a 699-root
read builds them 699 times. (Raised by both reviewers of the first design; checked.)

## 2. Goal

Build the four indexes once per read and look them up for every root, with **byte-identical
output** (every golden, the CDO output tests and ratchets). No parallelism, no change to the search.

## 3. Design

- `pub(crate) struct ConeIndexes<'s>` holding the four maps, built by `ConeIndexes::new(snap)` with
  exactly today's code: the same insert order (last insert wins for a duplicate identity or call
  site id, as today), the same bucket sort, the ledger in the snapshot's row order.
- `unresolved_cone_in(snap, &ConeIndexes, root)`: today's search, reading the indexes.
- `unresolved_cone(snap, root)` stays, as `unresolved_cone_in(snap, &ConeIndexes::new(snap), root)`,
  for the unit tests that call it with one root.
- `FinishCx` (`digest_cli.rs:1880-1894`, built once per read: `:1873` for one read, `:2134` for a
  streamed read across all its batches) gets `cone_indexes: OnceLock<ConeIndexes<'t>>` next to
  `rows_by_stable`, built on the first root that needs a cone; `finish` calls `unresolved_cone_in`.
  So a read builds the indexes once, not once per root (nor once per 32-root batch).

Why the output cannot change: the maps are built from the same snapshot by the same code; the search
never iterates them, it only looks them up by key (`unresolved_cone.rs:173`, `:187`, `:233`, `:244`,
`:255`); the buckets it reads have a fixed order (`outgoing` sorted, `ledger_by_routine` in row
order); and `&ConeIndexes` has no interior mutability, so one root's search cannot change what the
next one reads (the type is the guard: a search that wrote to them would not compile).

Memory: one set of the four maps for the read, instead of one set per root at a time; it now lives
across the read's entry cloning and finishing, so the peak is not claimed equal: it is measured
(section 5).

## 4. Tests and proofs

The oracle: today's `unresolved_cone`, kept verbatim as a test-only function
(`unresolved_cone_legacy`), with its own inline index build. Two comparisons, over every r0 dir and
the dependency fixtures:

- **The raw search:** one `ConeIndexes` per tree, reused across all its roots in root order (as
  `finish` reuses it), `unresolved_cone_in` against `unresolved_cone_legacy`, every field of the raw
  result: `visited_ids` as a set (a `HashSet` prints in no fixed order), the rest field by field.
- **Through the real caller:** `FinishCx::finish`'s output (its `UnresolvedCone`s, after placement,
  literal filtering, counts and the gap and event summaries) against the same `finish` with a
  `#[cfg(test)]` switch that takes each root's raw cone from `unresolved_cone_legacy` instead: the
  finishing code is the same on both sides, so any difference is the cone source.

Preconditions (asserted): more than 100 roots, at least one cone with items, at least one tree with
two roots whose cones differ (reuse across roots is what the change is about, and the "first root's
cone for every root" break below needs it to fail).
- **Order-sensitive by hand:** a hand-built snapshot (as the module's existing tests build them)
  whose search hits its VISIT cap (the breadth-first depth cut does not depend on neighbour order,
  `unresolved_cone.rs:234-245`; only the `max_visited` break does, `:250-258`), with its
  `typed_edges` listed out of sorted order, so that the order of a routine's outgoing neighbours
  decides which routines are visited. The cap is a fixed 25,000 today (`:71`, `:100`), so the search
  takes its two limits as parameters (`unresolved_cone_in`; production passes today's defaults,
  `DEFAULT_MAX_DEPTH` and `DEFAULT_MAX_VISITED`) and the test passes a small visit cap; the legacy
  oracle gets the same two parameters. Production equals the oracle on it; preconditions asserted:
  the cap was hit, and the reverse edge order gives a different visited set.
- **Built once per read:** a thread-local `#[cfg(test)]` counter of `ConeIndexes::new` calls (like
  `STREAM_BATCHES`, `digest_cli.rs:2089`, so tests running in parallel do not share it); one
  `finish` over a read of several roots builds the indexes once, and a streamed read of several
  batches builds them once.
- **Discrimination, one per guard:**
  - Reuse (the counter): `finish` calling `unresolved_cone(tree, r)` per root again (the old path):
    the count equals the roots, the test fails; reverted, it passes.
  - Shared state (both comparisons): the shared path answering every root with the first root's
    raw cone (a break only the shared path can make): both fail; reverted, both pass.
  - Bucket order (the hand-built case): drop the sort in `ConeIndexes::new`: the production path
    differs from the legacy oracle on the order-sensitive case; reverted, passes.
- **End to end:** `scripts/ci-steps task` (every golden), and the next gate's `output_cdo`.

## 5. Measurement

Release-fast `alsem` before and after, on CDO, one job at a time: `ALSEM_TRACE=1` of the gate's
`impact --diff` (histogram) and `impact --base bc3ccb18~5 --no-tests --format json`: the
`digest.unresolved_cones` span, wall, and peak RSS. Expected: the span falls from 33 s (diff) and
67 s (base) to a few seconds, if the index builds dominate it; if most of what is left is
`count_gaps_in_cone` (`digest_cli.rs:1683`, one scan of the visited set per gap subject, per root),
that is the next step, reported, not part of this one. A smaller gain than expected is reported as
a finding.

## 6. Risks

Low: a refactor that moves code out of a loop, guarded by the type (no mutation through
`&ConeIndexes`), the oracle and the goldens. Peak memory may rise slightly (one index set held for
the read); measured.

## 7. The rejected first design, and what a parallel reach needs (for ENG-18 and TLS-22)

The first draft proposed running `literal_reach_entries` (`digest_cli.rs:2358`,
`prove.reach_effects`: 108.8 s of the diff call, 196.8 s of the base call, 5.5 s of a one-root
prove) in parallel over roots, one `SuffixCache` per worker, on the claim that every cached value is
a pure function of its key. Both reviewers (sol and Opus) said DO NOT APPROVE, and the code confirms
them:

- **History dependence (Critical, both; now ENG-18).** `LiteralReach` holds a `RecRefFlow`
  (`literal_reach.rs:1597`) whose `eval: RefCell<EvalState>` (`recref_flow.rs:201`) keeps ONE
  budget of `EVAL_BUDGET` = 200,000 evaluations for the whole read (`:221-225`); its depth cutoff is
  `EVAL_DEPTH` = 48. A cut-off key itself is never stored (it returns "anything" before the memo
  insert, `:655-661`), but every outer value computed from a cut-off is stored as final (`done`,
  `:589-592`, `:714-727`). So a search's result can depend on which searches ran before it in the
  same read: per-worker evaluator state would make it depend on the chunking, a shared one on
  thread timing, and today's serial output may already depend on what else was asked (one root
  against 699). Per-worker state alone does NOT remove the dependence. ENG-18 measures it on CDO
  and fixes it at the root: no `done` value derived from a cut-off, and a limit that is
  deterministic per evaluation. Whether the r0 fixtures reach either limit is unmeasured (a
  reviewer's reading, not a count).
- **It does not compile as designed (Critical, both).** `RefCell` makes `LiteralReach` not `Sync`;
  `SuffixCache` holds `Rc` trees and `Rc` plain sets (`literal_reach.rs:545`, `:552`), so it is not
  `Send` and cannot move between rayon threads; `LiveTree::by_routine` is a `std::cell::OnceCell`
  (`:1522`). A parallel design needs `Arc` and `OnceLock` there, and history-independent evaluator
  state (after ENG-18).
- **Other shared state (Important, sol; missed by the first draft and by Opus's round 1).** `CbGraph`
  holds `Mutex` caches too (`commit_scope.rs:656-660`), and searches build keep graphs and scratch
  per call: TLS-22's checklist must cover them.
- **Memory (Important, both).** `SuffixCache::with_limits` never lets a budget fall below one tree's
  `STATE_CAP` (400 K states, `:568-579`): 22 workers could hold 8.8 M states against 1.6 M today; an
  in-flight build can exceed the budget by one tree before eviction (`:598-618`); `plain` sets are
  unbounded; every worker adds a root tree, crossing copies and path buffers. The resident engine
  runs this pass too (`fill_witness` through `passed_root_entries`, `resident/revision.rs:927`),
  under the context server's budgets and `--low-mem` (Opus): measure there too.
- **Contention and balance (Important, Opus).** `flow_cache`'s `Mutex` is taken on every guarded
  edge of every state (`literal_reach.rs:2285-2301`, `:2631`): one cache per worker (it is a pure
  memo). Fixed chunks inside 32-root batches (`impact/jobs.rs:80`) leave threads idle and per-root
  cost is uneven: schedule per entry, with one cache per rayon thread.
- **Tests (Important, both).** Serial-against-parallel equality on the fixtures does not guarantee
  coverage of these risks (whether the fixtures reach the limits is unmeasured). A test must state
  its preconditions by hand: forced eviction, a cached cut-off, opposing
  bindings, roots sharing a suffix tree across workers.
- **Minor.** The flow cache's last writer wins (harmless: equal values; Opus and sol); the cones
  collect into a `BTreeMap`, so order comes from the key (Opus and sol); keep the
  `prove.reach_effects` span on the calling thread (Opus); a worker count from the core count makes
  peak memory machine-dependent (Opus).

## 8. TLS-22: the parallel literal pass (draft for review, 2026-11-11, master `c498fd98`)

Status: draft, not reviewed. It answers each finding of section 7 against the code after ENG-18.

### 8.1 What ENG-18 settled, and what it did not

ENG-18 (`55beb633`) gives every top-level `RecRefFlow::evaluate` a fresh `EvalState` (its own
budget, depth cut, memo and final marks, `recref_flow.rs:584-617`) and keeps the answer by its key
in `results` (`:207`). So an answer is a function of its key alone: the history finding is closed.
The STRUCTURE is not: `eval` is still a `RefCell` field that each evaluation overwrites, and
`results` is a `RefCell` map. Both keep `LiteralReach` from being `Sync`.

### 8.2 Goal

Run `literal_reach_entries`' per-root loop (`digest_cli.rs:2396`, `prove.reach_effects`: 108.8 s of
the gate's `impact --diff`, 196.8 s of `impact --base`, traced before TLS-17) over the roots of a
batch in parallel, with output **byte-identical in any thread count**, peak memory measured and
bounded independently of the machine's core count.

### 8.3 Why the loop can be split by root

Each iteration reads `reach`, `group_paths`, `unresolved_sites` (shared, read-only) and the suffix
cache, and writes only its own `entry` (`digest_cli.rs:2408-2580`; the `entries` inside the
crossing block is a local `CrossingEntries`, not the slice). So the only cross-root effects are the
caches. Every cache must be a pure memo: a value is a function of its key, so who computes it
first, and whether it was evicted and rebuilt, cannot change any answer.

| State | Where | Today | Pure function of key? | Change |
|-------|-------|-------|-----------------------|--------|
| `RecRefFlow::eval` | `recref_flow.rs:203` | `RefCell<EvalState>`, reset per evaluation | per evaluation (ENG-18) | **removed as a field**: `evaluate` owns a local `EvalState` and passes `&mut` down `eval_routine` (`:639`, recursive at `:890`). The type then forbids state outliving an evaluation, and no lock is needed. |
| `RecRefFlow::results` | `:207` | `RefCell<HashMap<EvalKey, ..>>` | yes (ENG-18) | `Mutex`, never held while evaluating (look up, unlock, evaluate, insert-if-absent). Two threads may evaluate one key; both get the same answer. |
| `LiteralReach::flow_cache` | `literal_reach.rs:1587` | `Mutex`, not held during compute (`:2301-2320`) | yes (`FlowKey` is program, entry values, keeps) | keep; measure contention (8.6); shard only if the trace shows it. |
| `CbGraph::{toward, toward_explicit}` | `commit_scope.rs:657-660` | `Mutex`, compute outside, `or_insert` (`:940-955`) | yes (by origin node) | none. |
| `LiveTree::by_routine` | `literal_reach.rs:1522` | `std::cell::OnceCell` | yes (from the tree) | `std::sync::OnceLock`. |
| `SuffixCache` | `:544-630` | `&mut`, `Rc` trees, FIFO eviction over 1.6 M states | yes: `tree_from(start, per_tree)` reads only `start`, the cap and pure memos; a capped search is the same `Stop::Capped` every time | see 8.4. |
| `judge_traps`' `asked` | `:2311` | local `RefCell` | local | none. |

Proof obligation for the table: before code, a grep for `Cell`, `RefCell`, `Rc<`, `Mutex`,
`thread_local`, `static mut` and atomics over every module the loop reaches
(`literal_reach.rs`, `recref_flow.rs`, `commit_scope.rs`, `group_path.rs`, `lflow`, `values`,
`routes`, `traps`), recorded in the plan. The compiler is the second check: `par_iter_mut` over the
loop does not compile while any of them is not `Sync`.

### 8.4 The suffix cache: one shared cache, not one per worker

Section 7 found that per-worker caches multiply memory (each budget at least one tree's 400 K
states). The suffix trees are pure (8.3), so ONE cache can serve every worker:

- `SuffixCache` becomes `Mutex<Inner>` with `Arc<LiveTree>` and `Arc<HashSet>` (`plain`); `get`
  takes `&self`. Lookup under the lock; on a miss, unlock, build, lock, insert if still absent
  (else take the one there and drop ours), evict oldest-first to the budget, unlock.
- Two workers may build the same tree once each; the trees are equal, one is kept. (An in-flight
  marker to stop the duplicate build is not built: measure the duplicate count first; it is a
  counter, 8.6.)
- Eviction order now depends on timing. That is allowed only because a tree is pure: an evicted
  tree is rebuilt with the same result. The test in 8.5 forces eviction across threads.
- Memory bound: held trees at most the budget (1.6 M states, as today) plus the trees in flight,
  at most one per worker (`workers × STATE_CAP`), plus each worker's root tree and crossing
  entries. So the worker count is part of the memory bound, and it is FIXED (8.5), not the core
  count.
- `built`, `evicted`, `peak_held` become atomics or move inside the lock. They are diagnostics
  (tests read them for the serial cases); under threads they vary and nothing prints them.

### 8.5 Scheduling and threads

- In `literal_reach_entries`, `entries.par_iter_mut().with_max_len(1)` (one root per task, so
  work stealing balances uneven roots) inside `pool.install`. The `prove.reach_effects` span stays
  on the calling thread around it.
- The pool: a dedicated rayon pool of `REACH_WORKERS` threads with `big_stack::BIG_STACK_SIZE`
  stacks (the search and `eval_routine` recurse; today they run on the caller's thread, which may
  be a big-stack thread). `REACH_WORKERS` is a constant chosen from the measurement (8.6), capped
  by the core count, never derived from it alone, so peak memory is the same on any machine with
  at least that many cores. `AL_SEM_REACH_WORKERS=N` for tests and measurement; 1 runs the loop
  serially on the calling thread (today's path, no pool).
- Batches: a streamed read passes 32 roots at a time (`impact/jobs.rs` `ROOT_BATCH`), so at most
  32 roots run at once and the batch's slowest root bounds it. Measured, not changed here; if the
  tail dominates, a follow-up overlaps batches (reported, not part of this task).
- Nested parallelism: `QueryCx::run` before the pass and the resident engine's other work keep
  their own pools; the pass runs inside its pool only.
- The resident engine (`resident/revision.rs:927`, `passed_root_entries`) runs the same pass. Under
  `--low-mem` it runs serially (`REACH_WORKERS` = 1): memory before speed there (mantra 21). The
  default resident mode uses the pool, measured against its 8 GiB budget.

### 8.6 Tests and proofs

The oracle is the serial path (`AL_SEM_REACH_WORKERS=1`, or a direct call with workers 1).

- **Thread-count equality, real output:** over every r0 dir and the dependency fixtures, the
  passed entries (serialized, every field) with workers 1, 2 and 8 are byte-equal. Preconditions
  asserted: at least one entry with a boundary group, at least one suffix-cache hit across two
  roots, more roots than workers.
- **Shared suffix tree across workers (hand-stated):** a hand-built tree where two roots cross into
  the same dependency entry state; with 2 workers and a barrier so both ask for the same key at
  once, both get equal trees and the outputs equal the serial ones. Precondition: `built` counts 2
  builds for 1 key (the race happened).
- **Forced eviction under threads:** `SuffixCache::with_limits(small, small)` so every request
  evicts; workers 4; output equals serial with no eviction. Preconditions: `evicted > 0`, some key
  built twice.
- **No evaluator state across evaluations (type guard):** `eval` is no longer a field; a test is
  not needed for what does not compile. ENG-18's order tests (`literal_flow_tests.rs:366-386`)
  keep pinning history independence, and run under workers 4 as well.
- **Results race:** two threads evaluate one `EvalKey` at once (barrier); both answers equal the
  serial answer; the map holds one entry.
- **Discrimination, one per guard:**
  - Shared cache keyed wrongly (insert under the requesting worker's FIRST key): the
    shared-tree and equality tests fail; reverted, pass.
  - A cache that keeps a cancelled or capped-by-budget tree from a different cap (insert the
    tree built with `per_tree / 2` on a race): the eviction test fails; reverted, pass.
  - Output order: collecting rows in completion order instead of entry order (a
    `par_iter().map().collect()` into a `Vec` pushed by completion): the equality test fails;
    reverted, pass.
- **End to end:** `scripts/ci-steps task`; then the branch gate's `output_cdo` (every CDO golden and
  ratchet) with the default workers, and one CDO `impact --diff` JSON compared byte for byte at
  workers 1 and the default.

### 8.7 Measurement

Release-fast `alsem` on CDO, alone on the machine, through the orchestrator:

- `ALSEM_TRACE=1` `impact --diff` (the gate's) and `impact --base bc3ccb18~5 --no-tests --format
  json`, at workers 1, 2, 4, 8, 16: `prove.reach_effects` span, wall, peak RSS (`/usr/bin/time`
  is missing here: `/proc/<pid>/status` VmHWM via the cdo-speed sampler), suffix builds,
  duplicate builds, evictions, `flow_cache` lock wait (a temporary trace counter, removed before
  the landing).
- The resident engine: `alsem mcp` warm, one 699-root impact through it, default and `--low-mem`,
  RSS against the 8 GiB / 6 GiB budgets.
- `REACH_WORKERS` = the smallest count past which the span stops falling by 10 %, within the
  budgets. A smaller gain than the span's share suggests is reported as a finding (the batch tail,
  lock contention, duplicate builds).
- `cdo-speed` must pass (its model build and test universe ratios are untouched; its impact round
  should improve).

### 8.8 Risks

- A cache that is not pure after all: the table in 8.3 is the claim, the grep is the check, the
  thread-count equality on fixtures and CDO is the net.
- Memory: bounded by workers × (one in-flight tree + root tree), measured; `--low-mem` serial.
- Stack overflow on a pool thread: the big-stack pool.
- Rayon nested inside another pool's `install` (impact's own jobs): `install` from inside a pool
  blocks that worker; measured in the impact run.

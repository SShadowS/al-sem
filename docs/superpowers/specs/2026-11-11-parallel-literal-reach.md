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

Status: approved with changes in round 3 (sol, Opus); the round-3 changes are folded in. Round 1
(sol: do not approve yet; Opus: approve with changes; no Critical from either) found three
Important gaps, both reviewers the same ones: the memory bound, how the worker count and
`--low-mem` reach the pass, and race tests that cannot force their race. Round 2: one shared pool
cannot honour a worker count (both), and concurrent requests and both admission cases (sol).
Round 3: the concurrency test's barrier could hang (both). Reviews: `.agent/tls22/{pi-sol,opus}-r{1,2,3}.md`
in the main checkout.
It answers each finding of section 7 against the code after ENG-18.

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
| `LiteralReach::flow_cache` | `literal_reach.rs:1587` | `Mutex`, not held during compute (`:2301-2320`), but the `FlowKey` is hashed inside the lock | yes (`FlowKey` is program, entry values, keeps) | keep ONE shared cache. This reverses section 7's "one per worker" (Opus): a shared pure memo saves the repeated evaluations, and the lock is short. Measured (8.7); sharded by key hash only if lock wait passes 5 % of the span. |
| `CbGraph::{toward, toward_explicit}` | `commit_scope.rs:657-660` | `Mutex`, compute outside, `or_insert` (`:940-955`) | yes (by origin node) | none. |
| `LiveTree::by_routine` | `literal_reach.rs:1522` | `std::cell::OnceCell` | yes (from the tree) | `std::sync::OnceLock`. |
| `SuffixCache` | `:544-630` | `&mut`, `Rc` trees, FIFO eviction over 1.6 M states | yes: `tree_from(start, per_tree)` reads only `start`, the cap and pure memos; a capped search is the same `Stop::Capped` every time | see 8.4. |
| `judge_traps`' `asked` | `:2311` | local `RefCell` | local | none. |
| `SuffixCache::plain` | `:552`, `:623-629` | `Rc<HashSet>`, unbounded | yes (by target routine) | `Arc`, inside the shared cache (8.4), built OUTSIDE the lock (`plain_reach` is a whole-graph search, `:3639-3650`). Unbounded as today: one set per distinct entry target. |

Proof obligation for the table: before code, a grep for `Cell`, `RefCell`, `Rc<`, `Mutex`,
`thread_local`, `static mut` and atomics over every file the loop reaches (`literal_reach.rs`,
`literal_flow.rs`, `literal_values.rs`, `literal_routes.rs`, `recref_flow.rs`, `commit_scope.rs`,
`group_path.rs`, `traps.rs`, `terminal_facts.rs`, `l4/param_guard.rs`, `digest.rs`), recorded in the
plan. In round 1 Opus ran it and sol read the files; neither found anything else. `Sync` proves thread safety, not
purity (sol): purity is the table's claim, and the thread-count tests (8.6) are its net. The
compiler is the second check: `par_iter_mut` over the loop does not compile while any of them, or
any value type in a search state, is not `Send`/`Sync`. Also check before the plan that
`LiteralPass<'t>` still compiles with `Mutex<HashMap<State<'a>, ..>>` (invariant in `'a`; the
comment at `literal_reach.rs:1622-1624` shows variance has mattered here).

### 8.4 The suffix cache: one shared cache, not one per worker

Section 7 found that per-worker caches multiply memory (each budget at least one tree's 400 K
states). The suffix trees are pure (8.3), so ONE cache can serve every worker:

- `SuffixCache` becomes `Mutex<Inner>` with `Arc<LiveTree>` and `Arc<HashSet>` (`plain`); `get`
  takes `&self`. Lookup under the lock; on a miss, unlock, build, lock, insert if still absent
  (else take the one there and drop ours, and `held` does not grow), evict oldest-first to the
  budget, unlock. A cancelled search is still never cached (`:600-603`).
- Two workers may build the same tree once each; the trees are equal, one is kept. (An in-flight
  marker to stop the duplicate build is not built: the duplicate count is measured first, 8.7.)
- Eviction order now depends on timing. That is allowed only because a tree is pure: for a fixed
  start and cap (and no cancel) the search is FIFO over snapshot-ordered edges
  (`:2811-2875`; both reviewers checked), so an evicted tree is rebuilt with the same result. The
  eviction test in 8.6 forces it across threads. `per_tree` is never changed by this work.
- `built`, `evicted`, `peak_held`, and a new `duplicate` count move inside the lock. They are
  diagnostics: no output reads them (both reviewers checked); under threads they vary.
- Pre-existing, unchanged: a tree may hold `cap + 1` states (the check runs before the insert,
  `:2854-2866`), so `held` can pass the budget by one tree; the memory accounting below uses
  `STATE_CAP + 1`.

### 8.4a Memory (round 1, both reviewers: the first bound undercounted)

A state count is not a heap bound, and a worker holds much more than one tree in flight. Per
worker at once, each search capped at `STATE_CAP + 1` states: the root's tree (`live_tree_of`), its
keep graph (`keep_graph`, `:2619`), trap-scope searches (`:1700`, `:2934`) and the other capped
searches of the loop (`:2584`, `:2702-2731`), plus a suffix tree in flight and the root's crossing
entries and witnesses. Shared, as today: the held suffix trees (budget 1.6 M states), `plain`,
`results`, `flow_cache` and `CbGraph`'s caches, which grow with what is asked, not with the worker
count, plus evicted trees still held by a reader until it drops them. So:

`peak ≈ today's serial peak + (workers − 1) × the largest per-root working set`

Roots differ, so a peak difference between two worker counts is not an upper bound (round 2,
sol). The working set is therefore measured per root, in a workers = 1 run (under threads, shared
caches and `Arc` frees blur a per-root count; round 3, Opus): a temporary trace field records each
root's own allocation peak in the pass (the largest 10 named). The parallel runs give only the
pass's whole peak, measured directly at every candidate worker count (8.7), never extrapolated. The bounds the worker count must keep, all
of them:

- the CLI: the gate's `impact --diff`/`--base` peaks against the 4 GiB host headroom rule
  (TLS-19);
- `cdo-speed`'s peak-memory limit (1.10 × the merge base). Its impact round asks one new routine,
  so its pass has few roots (impact can widen one changed routine to more: the measurement
  records how many the round passes) and the pool adds little there; `cdo-speed` must pass;
- the resident engine: admission (`resident/budget.rs`) reserves an estimate before work and only
  LEARNS from an overrun afterwards (`:27-30`), so learning is not prevention (round 2, sol). The
  pass runs under two admission cases: `Fill("witness")` (`fill_witness`, a request of at most
  `WITNESS_MEMO_MAX_ROOTS` roots, `resident/revision.rs:886-930`; starting estimate 60 MiB,
  `budget.rs:174`, "until the fill's peak is measured apart") and `Trees` (a streamed read,
  `EffectsStream::for_each`, `session.rs:1076-1088`; `Case::Trees`, 900 MiB, `budget.rs:186`,
reserved for kind `"trees"` at `bin/alsem.rs:4654-4658`; not `Case::Fill("tree")`, the tree store).
The plan raises exactly these two keys, `Case::Fill("witness")` and `Case::Trees`, if measured. Both are measured at the default
  worker count, one request and two concurrent requests, and each case's starting estimate
  (`Case::table_mib`) is set to its measured peak when the pool raises it. The change is rejected
  if a 699-root ask, or two concurrent ones, overrun the 8 GiB budget where serial does not.
  `EffectsStream` gets the worker setting as a field (it has none today, `session.rs:1039-1046`).

### 8.5 Scheduling and threads

- In `literal_reach_entries`, `entries.par_iter_mut().with_max_len(1)` (one root per task, so
  work stealing balances uneven roots) inside `pool.install`. The `prove.reach_effects` span stays
  on the calling thread around it.
- **The setting is a parameter, not an environment variable.** `ReachWorkers` (a `usize`, 1 =
  serial on the calling thread, no pool) is passed to `LiteralPass::new`, `literal_reach_pass`,
  `passed_root_entries` and `digest_effects_chunks` (every route to the loop; round 1: neither
  route carries a setting today, `digest_cli.rs:1786`, `:2144`, `resident/revision.rs:927`). The
  callers decide it:
  - the CLI (`alsem impact`, `digest`, `prove`): `REACH_WORKERS`, a constant chosen from the
    measurement (8.7), at most the core count;
  - the resident engine: from its session: `ResidentSession::low_mem()` (`resident/session.rs:400`)
    gives 1, the default gives `REACH_WORKERS`. `--low-mem` is memory before speed (mantra 21).
    `Revision` gets the setting from the session that builds it;
  - tests: pass it explicitly. No process-global setting, so tests running in one process cannot
    race on it (round 1, Opus). The binary alone reads `AL_SEM_REACH_WORKERS=N` (measurement) and
    turns it into the parameter.
- **The pools: one per worker count, per process.** `reach_pool(n)` returns a pool of exactly `n`
  threads with `big_stack::BIG_STACK_SIZE` stacks (the search and `eval_routine` recurse; not
  `big_stack_pool()`, which takes rayon's default thread count), built on first use and kept in a
  process-wide `Mutex<HashMap<usize, &'static rayon::ThreadPool>>`. So `ReachWorkers = n` really
  runs at most `n` roots at once (round 2, sol: one shared pool would ignore `n`). Production uses
  one count (`REACH_WORKERS`), so one pool; tests and measurement make a few more, which live until
  the process ends.
- **Concurrent requests.** The engine host runs several requests at once on its own std threads
  (`host/child.rs:268-272`). Their passes share the production pool, and a root runs only on a
  pool thread (the calling thread waits in `install`), so at most `REACH_WORKERS` roots are in
  flight in the whole process, however many requests run. What is per request, as today, is each
  pass's `LiteralPass` (reach, caches, `held` suffix trees, `digest_cli.rs:2235-2272`): two
  concurrent requests already hold two of those today, and parallelism does not add a third.
- **Serial inside a rayon worker.** Rayon lets a worker blocked in `install` run other jobs while
  it waits, which deadlocks when such a job takes a std `Mutex` the thread holds (`fill_witness`
  holds `scratch` across the pass, `resident/revision.rs:896`). So a pass called from inside a rayon
  worker runs serially on that thread; such a caller is parallel already. (As built, 2026-11-11:
  the draft's `debug_assert!` instead fired in the task gate, because a test,
  `ids_parent_baseline::record_fixture`, runs the digest pipeline inside its own `par_iter`. No
  production caller was found doing so; the fallback keeps a future one correct.)
- Batches: a streamed read passes 32 roots at a time (`impact/jobs.rs` `ROOT_BATCH`), so at most
  32 roots run at once and the batch's slowest root bounds it. Measured, not changed here; if the
  tail dominates, a follow-up overlaps batches (reported, not part of this task).

### 8.6 Tests and proofs

The real oracle is the goldens and the CDO output tests: workers 1 runs the NEW code too (the
`Mutex`/`Arc` cache), so a thread-count comparison cannot catch a bug that is wrong in every order
(round 1, Opus). The thread-count tests below catch what depends on timing; the goldens catch the
rest.

Races are forced, not hoped for: a `#[cfg(test)]` hook in `SuffixCache::get` (after the lookup
misses and the lock is released, before `tree_from`) and in `RecRefFlow::evaluate` (after the
`results` lookup misses and its lock is released, before the fresh `EvalState`) runs between the
miss and the build (a closure the test sets per cache,
not a global), so a test can hold the first worker there until the second has also missed. A
barrier before the call cannot force a double miss (round 1, both).

- **Thread-count equality, real output:** over every r0 dir and the dependency fixtures, the
  passed entries with workers 1, 2 and 8 are equal, compared as their `Debug` form
  (`DigestEntryResult` has no `Serialize`, `digest.rs:4203`; its `Debug` is stable, no hash
  fields). Preconditions asserted: at least one entry with a boundary group, at least one
  suffix-cache hit across two roots, more roots than workers. Threads hash with different seeds,
  so this test also exposes any HashMap-order dependence a serial run hides; a failure is a root
  cause to find, not flakiness.
- **Shared suffix tree, same target, opposing bindings (hand-stated):** a hand-built tree where two
  roots cross into the same dependency routine under DIFFERENT bindings whose suffix trees differ
  (precondition asserted: the two trees differ, so a key missing its bindings would be caught), and
  two more roots that cross under the SAME entry state. With 2 workers and the hook forcing both
  same-state requests to miss: both get equal trees, `duplicate` = 1, `held` counts the tree once,
  and every output equals the serial one.
- **Forced eviction under threads:** the same `per_tree` on both sides (round 1, sol: a different
  cap is a different question), a budget of one tree so every request evicts; workers 4; output
  equals the serial run with the default budget. Preconditions: `evicted > 0`, some key built
  twice.
- **Results race:** the hook holds two threads after both missed one `EvalKey`; both answers equal
  the serial answer, the map holds one entry.
- **Evaluator state:** `eval` is no longer a field, so state cannot outlive an evaluation (the
  type is the guard). ENG-18's order tests (`literal_flow_tests.rs:250-386`) stay as they are; they
  pin the depth cut. A budget-exhaustion case is not added: the budget is a local of each
  evaluation by construction now.
- **The setting reaches the pass, and is obeyed:** a resident session with `low_mem` runs the pass
  serially and the default session with `REACH_WORKERS`, through both routes (`fill_witness` and a
  streamed `EffectsStream`); the CLI's route likewise. Asserted on OBSERVED concurrency, not the
  parameter (round 2, sol): a test-only per-pass counter of roots in flight records its maximum,
  kept on the pass (and read through the session's `Revision` for the resident routes), never a
  process-wide counter, so tests in one process do not share it;
  with `n` workers and a test hook that holds only the FIRST wave (the first `n` roots to start
  wait, with a timeout, until `n` are in flight or the timeout passes, then all are released; later
  roots never wait, so nothing hangs on a short last group or a smaller pool), the maximum is
  exactly `n`, and 1 with `low_mem`. The test also asserts the selected pool's
  `current_num_threads()` is `n` (round 3, sol: a larger pool need not show a maximum above `n`).
- **Discrimination, one per guard:**
  - The cache key (shared-tree test): key the suffix cache by target alone, dropping the
    bindings: the shared-tree test fails; reverted, passes.
  - Insert-if-absent (shared-tree test): insert unconditionally on the second miss, adding to
    `held` twice: `held` is wrong, the test fails; reverted, passes.
  - Eviction is safe only for pure trees (eviction test): break purity on purpose, a rebuilt
    tree gets `per_tree / 2` as its cap (precondition asserted: the fixture has a tree larger than
    `per_tree / 2` whose capping changes the output): the eviction test fails; reverted, passes.
  - The setting (setting test): the resident route ignores `low_mem`: the test fails; reverted,
    passes.
  - The pool size (setting test): `reach_pool(n)` builds `REACH_WORKERS` threads whatever `n`:
    the maximum in flight differs from `n`, the test fails; reverted, passes.
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
- The resident engine: `alsem mcp` warm, default and `--low-mem`, RSS against the 8 GiB / 6 GiB
  budgets: one 699-root impact (the `Trees` case), one ask of at most `WITNESS_MEMO_MAX_ROOTS`
  roots (the `Fill("witness")` case), and two of each at once.
- The per-root working set (8.4a): each root's own allocation peak in the pass (the temporary trace
  field) from the workers = 1 run, and the pass's peak measured directly at every candidate count.
- `REACH_WORKERS` = the smallest count past which the span stops falling by 10 %, within every
  bound of 8.4a. A smaller gain than the span's share suggests is reported as a finding (the batch tail,
  lock contention, duplicate builds).
- `cdo-speed` must pass (its model build and test universe ratios are untouched; its impact round
  should improve).

### 8.8 Risks

- A cache that is not pure after all: the table in 8.3 is the claim, the grep is the check, the
  thread-count equality on fixtures and CDO is the net.
- Memory: `(workers − 1) ×` the largest measured per-root working set over serial (8.4a); at most
  `REACH_WORKERS` roots in flight per process (production uses one pool); `--low-mem` serial.
- Stack overflow on a pool thread: the big-stack pool.
- A future caller inside a rayon worker: the `debug_assert!` (8.5) and the deadlock it guards
  against are recorded there.

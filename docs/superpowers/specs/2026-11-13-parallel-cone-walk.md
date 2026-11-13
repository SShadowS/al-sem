# The capability-cone walk runs SCCs in parallel (TLS-29)

Status: r3, after three panel rounds (sol `pi-sol-r1..r3`, Opus). Schedule approved by the
orchestrator 2026-11-13 (ready queue in index order, no window, the one-shot worker count; the
measured peak decides).

## 1. The problem

`alsem analyze --scope all` on CDO spends about 5 s of 24 s in `cone.walk`
(`compose_inherited_cones`, `src/engine/l4/capability_cone.rs`), the largest single span. The
walk visits the SCCs (strongly connected components: groups of routines that call each other in a
cycle) one at a time in Tarjan order, callees first. An SCC reads only its successors' cones, so
SCCs whose successors are done could run at the same time.

Census (`ALSEM_CONES_CENSUS=1`, CDO, 2026-11-13, two runs agree):

| | |
|---|---|
| SCCs / levels / widest level | 33,227 / 81 / 16,007 |
| sum of per-SCC times | 4,951-5,292 ms |
| critical path (any scheduler's bound) | 381-406 ms |
| slowest single SCC (704 members) | 122-129 ms |
| `fact_cone_for_scc` | 2,703 ms (55%) |
| closed-set memo row builds | 1,140 ms (27,101 builds, 47,124 reads) |
| subst / derived fold / own entries | 247 / 209 / 146 ms |
| raw-half work (`scan_cone`, BFS picks) | 0: analyze composes derived rows only |
| peak live fact-cone entries: Tarjan order / level by level | 1,392,716 / 3,116,791 |

The backlog item "The cone singleton walk" (`docs/OUTSTANDING.md`) is moot for analyze: its scan
runs only for the raw half since #160 step 7b (`singleton_scans=0`). Not folded in.

## 2. The schedule (simulated, `ALSEM_CONES_SCHEDULE_FILE` dump)

A ready queue: an SCC is ready when all its successors are done; a free worker takes the ready
SCC with the LOWEST index. No window. Simulated on CDO's dump (cone counted live from its SCC's
start, freed when its last reader finishes):

| workers | walk | peak live entries |
|---:|---:|---:|
| 1 | 5,292 ms | 1,392,716 |
| 6 | 946 ms | −0.2% |
| **8** | **742 ms** | **+3.2%** |
| 12 | 539 ms | +3.2% |

A window (an SCC starts only within W of the oldest unfinished one) made both worse at 8 workers
(W = 2,048: 838 ms, +4.2%). Level by level (every cone of a level built before its frees) is
+124%. The one-shot worker count is `default_reach_workers()` (`REACH_WORKERS` = 8, capped by the
cores), as for TLS-21/22. The simulation counts cone entries only; the measured peak RSS decides.
**Kept only if the measured peak stays within a few % of master; otherwise a window or fewer
workers, and the note says which.**

## 3. Where it applies

The parallel driver runs when all hold:
- `workers > 1`;
- `keep.is_none()` (no cone store). The resident engine always asks for kept cones
  (`resident/revision.rs`, `cones: true`), so it stays serial (TLS-22 rule);
- the walk emits no raw facts: `scope.raw` is `None` or `Subjects` of an empty set (one helper,
  `WitnessScope::names_none()`, used by the driver).

Otherwise today's loop runs unchanged.

**Who benefits.** `analyze` and the other plans with an empty witness (fingerprint, events, query)
on the one-shot path. **`digest`, `impact` and `prove` do not**: their plans set
`raw = Subjects(roots)` (`session.rs` `prepare`; `digest_reads_plan`), so their walk stays
serial. A parallel raw half (a task-local raw builder per SCC, re-interned at the merge) is a
follow-up item (§7), filed with this task's note: impact's main part is cdo-gate's long pole.

**Workers reach the walk explicitly.** `compose_capabilities` / `compose_capabilities_retained`
and `compose_cone_over_graph(_retained)` take a `workers: usize`. `session::compose_cone` (the
one-shot path) passes `default_reach_workers().count()`; `compose_cone_retained` passes 1; direct
callers of `compose_cone_over_graph` (projections, tests) pass 1 unless they test the parallel
path. Tests do not set the default count, so every test reaching `session::compose_cone` runs
the parallel driver on a machine with more than one core (CI and server1) and the serial loop on a
one-core machine. That widens coverage; the guards below do not depend on it.

## 4. The parallel driver

Inputs as today: `g`, `scc`, `succ_sccs`, `direct`, `direct_raw`, `cov`, `routine_ids`,
`routines_by_id`, `frames`, `scope`, the producer solve's `resolved` (own entries).

**Pre-interned resource ids.** Every fact the walk folds has the `resource_id` of a fact in
`direct_raw`: cone entries are `Arc::clone`s of `direct` reps (`fact_cone_for_scc`,
`substitute_entry`), own entries are producers' raw facts with only `temp_state` changed
(`pd_solve` `ResolvedEntry`), and producers come from `direct_raw`; the SELF half folds
`direct_raw` itself. Before the walk the driver interns the `resource_id` of every fact of every
key of `direct_raw` (keys sorted, facts in order), without filtering by the fold's rules (a
changed `temp_state` can make a temp producer count as physical). The interner is a standalone
`ResInterner` the driver owns (not the builder's): during the walk the workers and the merge
borrow it read-only (`&ResInterner`); a miss is an invariant break: `expect` with the id. After
the walk it is moved into the derived store with the merged pools
(`ConeDerivedBuilder::from_parts(interner, pools)`). This interns ids
the fold never keeps (run commits, temp tables, subscriber events): `res_universe_len` and every
`ResBitset` grow by them, harmless. `ResInterner`'s doc ("Serial") is updated, and its "order
reaches no output" claim was checked in review (every consumer resolves and sorts, or tests
emptiness; the store has no serde or `PartialEq`).

**Per-SCC slots** (`Vec<Slot>`, index = SCC index):
- `cone: Mutex<Option<Shared>>` with `Shared { fact: Arc<ConeFacts>, cov: Arc<CoverageCone>,
  memo: Arc<OnceLock<FoldedRow>> }`: set by the SCC's own task when done (non-roots only), taken
  when its last reader finishes;
- `remaining_uses: AtomicUsize` (readers not finished), `successors_left: AtomicUsize`;
- `done: AtomicBool`, set when the task has stored its `out` (read by the start assert, which
  must not read `successors_left`, the counter a readiness bug would corrupt);
- `out: Mutex<Option<SccOut>>`: the task's results until merged.

A reader clones each successor's `Shared` (three `Arc`s) at its start under the slot's lock, and
reads lock-free after; the memo row is built by the first reader that wants it
(`OnceLock::get_or_init` on the cloned `Arc`), as today's lazy memo. A taken `Shared`'s memory goes
when the last clone drops: at most the readers running then, the simulation's rule. **A missing
successor cone is an `expect`, not today's `if let` + `debug_assert`, in the parallel path**, and
each task asserts at its start that every successor's `done` flag is set.

**The task** (`compose_scc`): fact cone (`fact_cone_for_scc`), coverage cone, the recursive SCC
set and its `fold_set` row, each member's derived row (the singleton derived half with the memo,
or the recursive member's `fold_row` + own entries, then the SELF half), each member's
`CoverageRecord`, a `ComposeStats` delta, and under the census the SCC's time and cone size. Rows
are built by a task-local `RowFolder` (two scratches, `&ResInterner`) into frozen rows (flags and
sorted id lists). The serial loop body is not duplicated: the derived half of a member moves into
functions both drivers call; the serial loop keeps its raw, seed and store parts. The fold's one
interning step (`Scratch::add`, today `&mut ResInterner`) goes behind a small trait with two
implementations: the serial loop interns (`&mut ResInterner`, unchanged behaviour), the parallel
driver looks up and `expect`s (`&ResInterner`).

With `Subjects(∅)` the parallel driver returns the same raw value the serial loop does (the
finished, empty `InheritedBuilder::default()` table), so `prepare`'s `*raw = Some(w)` sees no
difference; guard 3 asserts it.

**The queue.** `std::thread::scope` with `workers` threads (default stacks: nothing in the walk
recurses deeply; `fact_cone_for_scc` and the folds iterate), a `Mutex<BinaryHeap<Reverse<usize>>>`
of ready SCCs and a `Condvar`. A worker pops the lowest index, runs the task, stores `out`, then
(1) for each successor `y`: `remaining_uses[y] -= 1`, at 0 take `y`'s `Shared`; (2) for each
reader `p`: `successors_left[p] -= 1`, at 0 push `p`; (3) merges the finished prefix (below);
notify. A worker stops when every SCC is done. A panicking task sets an abort flag and notifies
all (a drop guard), so no worker waits forever; the scope then re-raises the panic (dev and test
builds; `release-ship` aborts). A worker that later locks a mutex the panic poisoned panics too;
that is accepted (the first panic is the one re-raised), not a bug to "fix" with
`into_inner`.

**The merge, during the walk.** A `Mutex<Merge>` holds the derived pools and rows (no interner),
`coverage_out`, the stats, under the census the `scc_nanos` / `cone_len` arrays, and `next` (the
lowest SCC index not merged). After storing its `out`, a worker takes the lock (blocking) and
merges `out[next], out[next+1], ...` while present, taking and dropping each: coverage records
into `coverage_out`, frozen rows into the pools (`push_frozen`, `finish_routine`'s layout), stats
added (sums; `max_recursive_members` and the census maxima by max; `size_hist` per element), the
SCC's time and cone size into the census arrays. Whoever stores `out[i]` then locks, so `i` is
merged by it or by an earlier holder: nothing waits for the end. Index order makes the pools'
layout independent of worker count and completion order. After the scope `next == n` is asserted
and the census arrays go to `add_schedule`.

**Census under parallelism.** The census atomics stay (thread-safe); timers become sums of the
workers' elapsed times (wall per timer, summed over threads, not measured CPU), and `COMPOSE_REST_NANOS` (wall minus summed components) is not charged by a parallel
walk (it would go negative). `CLOSED_MEMO_LIVE_MAX` counts live memo rows with an atomic. The
schedule census (`add_schedule`) reads the arrays the merge fills from each `out`.

The leak check stays: after the walk every slot's `cone` and `out` is empty.

## 5. Guards (each with a discrimination proof)

The parallel tests run the driver under a **test scheduler**: `#[cfg(test)]` the queue can run one
task at a time in a chosen order (lowest index, highest index, or a seeded shuffle of the ready
set), so a schedule-dependent break shows deterministically, not by a race.

1. **Same output at any worker count and order.** Synthetic graphs from a seeded generator in the
   test (DAG layers, recursive SCCs of 2-30 members, self-loops, shared callees, own entries, frame
   guards where the helpers allow), composed with the serial loop and with the parallel driver at
   2, 3 and 8 real threads and under the test scheduler's three orders: equal `ConeOutcome`
   (coverage, derived rows resolved to strings, stats). The real-thread runs assert that tasks
   overlapped (a test counter of the most tasks in flight at once > 1, on a graph wide enough;
   skipped with a message on a one-core machine). Break: let an SCC become ready one
   successor early; under the highest-index order the reader starts before that successor, and the
   start assert (the `done` flag) fails.
2. **A cone lives until its last reader is done.** The test scheduler orders two readers of one
   cone so the second starts after the first finishes. Break: take the `Shared` at the first
   reader's finish; the second reader's `expect` fails.
3. **The serial paths are untouched.** `workers` 8 with a raw scope (`Subjects` of a member) and
   with `keep` runs the serial loop (a test counter of parallel walks stays 0) and equals
   `workers` 1. Break: drop the raw condition from the driver choice, and the counter test fails.
   Plus `Subjects(∅)` runs the parallel driver (counter 1) and returns the same empty raw table as
   the serial loop. Break: test `is_none()` instead of `names_none()`, and it fails.
4. **No hang on a panic.** At 3 real threads, on a graph where every other SCC depends on one leaf
   (so nothing else is ready while the leaf runs), a test-only hook makes the leaf's task wait
   until the other workers are parked on the condvar (a test counter of waiting workers), then panic; the walk
   returns the panic (`catch_unwind` in the test) instead of hanging. Break: remove the abort
   notify, and the parked workers never wake: the test fails its deadline (the walk runs on a
   thread the test joins with a timeout).
5. **No leak.** The `debug_assert` over the slots runs in every guard-1 run.

## 6. Measurement

On CDO (release-fast, one window, MemAvailable ≥ 35 GiB, load < 10 before each run), master
against the branch: `analyze --scope all --format json --deterministic`: wall, CPU, peak RSS, the
`cone.walk` span; outputs byte-identical. `digest` and `impact --diff` once each as a
no-regression check (their walk stays serial): outputs byte-identical. Pass if the outputs are
equal and analyze's peak is within a few % of master; otherwise a window or fewer workers.

## 7. Not in scope

- The resident engine's walk (stays serial unless `cdo-speed` passes with memory within 1.10).
- Raw compositions (digest, impact, prove; serial). Follow-up: a task-local raw builder per SCC.
- `pd.solve` (1.2 s, before the walk) and `fact_cone_for_scc`'s own cost (7.7 M `merge_cone`
  calls); separate items.

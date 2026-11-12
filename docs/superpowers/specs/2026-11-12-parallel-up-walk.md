# TLS-21: impact's up walk over several roots at once (spec, 2026-11-12)

**Status:** reviewed 2026-11-12 by gpt-6.1-sol (5 rounds) and a local Opus agent (2 rounds).
Both READY. The main changes from review:
- the walk-window memory measurement (3.5);
- the master golden as the outside reference (test 1);
- the probe on `ReachWorkers`;
- per-task `WalkOptions`. Task: `/coord/tasks/TLS-21/task.md`. Owner approval (2026-11-11):
trade some memory for speed, with bounded parallelism, byte-identical output, measured time and
peak memory, and the resident server inside `cdo-speed`'s 1.10 memory ratio or the change opt-in.

**Who gains: the one-shot CLI only.** `alsem impact` (and its `--part up`) run from the command
line, the gate's `output_cdo` ratchets and CI. The resident engine (`alsem mcp`, `engine-host`)
keeps walking one root at a time. That is the same split TLS-22 made for the literal pass (owner
decision q-20261112T032637-1269d8fc): the resident engine answers heavy asks one at a time and
its 8 GiB budget has about 3 % headroom left after two impacts in a row. This change adds no
resident setting, flag or policy. A resident revision passes "serial" and nothing can change that
short of a new change with its own measurement.

## 1. Where the time goes (step 0, measured)

One traced one-shot `alsem impact --diff` on the pinned CDO baseline (the TLS-16 input:
`bc3ccb18~5..bc3ccb18`, 699 roots), `release-fast` of master `4f2459a6` plus trace spans in
`up::up`, 2026-11-12 13:43-13:46 UTC. The host was busy with other work (load 305 at the start,
18 at the end), so the absolute times are high; the split is the point.

| Part of `job.0.impact-up` | Time | Share |
|---|---:|---:|
| the per-root walks (`walk_one` / `walk_multi`, 699 roots) | 42.7 s | 93 % |
| the per-root summaries (`summary`, `callers_json`, ancestors) | 3.0 s | 6 % |
| the union walk (every root's nodes at once) | 0.085 s | 0.2 % |
| the union summary | 0.005 s | 0 % |
| caption parses (39 files, inside the summaries) | 0.08 s | 0.2 % |
| the whole up part | 46.0 s | |

The up part runs on the process's main thread (trace thread 1), after the main part, so a pool
of its own does not compete with other work in the same process. The walk itself uses no rayon.

So the work to spread is the per-root walk plus its summary. The union walk stays serial after
them: it is one walk whose answer (the best tier per node over all roots) cannot be split, and it
costs 0.1 s.

## 2. Today's code (master `4f2459a6`)

- `up::up(g, &UpArgs, &mut Captions) -> Value` (`src/engine/impact/up.rs`), called only from
  `UpJob::sited` (`src/engine/impact/jobs.rs`, `cancel: None`).
- `Job::sited(&self, side, graph)` (`src/engine/batch/job.rs`) is called by two routes:
  - the batch runner (`src/engine/batch/mod.rs`, the one-shot CLI);
  - a resident revision's `answer_sited` (`src/engine/resident/revision.rs`), used by `Revision`
    and `GraphRevision`.

  Three implementations: `UpJob`, `test_select::jobs` and one in `resident/session_tests.rs`.
- Per root, in order: its non-symbol nodes; a fresh visitor `Up`; `walk_one` / `walk_multi` on
  ONE shared `WalkScratch`; then `summary(&v, g, captions)` and the row. Rows are pushed in root
  order.
- `Captions`: a `&mut` cache, workspace file -> parsed page actions. A value depends only on its
  file.
- The `cost` block:
  - `perRootStates` (sum) and `unionStates`;
  - `walkMs`, the up part's wall time;
  - `walksUs`, the sum of each walk's own duration, the union walk included;
  - `scratchBytes`, the one shared scratch's capacity at the end. Today it is fixed only because
    the walks run in a fixed order: a reset keeps capacity, so the number depends on which walks
    ran before;
  - `visitorBytes`, the largest visitor of any one walk; and `inventoryBytes`.

## 3. Design

### 3.1 Who decides the worker count: the route

- `Job::sited` gains a parameter `workers: &ReachWorkers` (TLS-22's type,
  `src/engine/l5/digest_cli.rs`), the same way TLS-22 put the literal pass's count on the
  routes:
  - the batch runner passes `default_reach_workers()`: 8, capped by the cores,
    `AL_SEM_REACH_WORKERS` for measurement. This covers a standalone `--part up` and the composite
    `alsem impact`, which runs its up part in the same process (`src/bin/alsem.rs`, the composite
    route). Each child of an `alsem batch` gets its own 8, as with TLS-22;
  - `answer_sited` takes the count from its caller. `Revision` passes `self.pass_workers()`:
    its `reach_workers` field is always `SERIAL` in production, and in tests it carries the probe.
    `GraphRevision` passes `ReachWorkers::SERIAL`. The engine host never runs `impact-up` through
    the batch runner: it refuses `batch` and the composite `impact`.
- Why not decide while planning (`PlanCx::inputs` is `Some` only for a resident request): it would
  avoid the trait change, but it infers "resident" from an unrelated field. The route already
  states which engine runs the job, as it does for the literal pass.
- One knob for both passes. They run one after the other on the main thread, never at once, so
  one pool of 8 threads serves both (`reach_pool`, made `pub(crate)`). No new environment
  variable.
- `test_select::jobs`' `sited` and the test implementation take the parameter and ignore it.
- `UpArgs` gains `workers: ReachWorkers`. `UpArgs::cancel` becomes
  `Option<&'a (dyn Fn() -> bool + Sync)>`. Its only caller passes `None`. `WalkOptions` keeps its
  own type and is not `Sync` (it holds a plain `&dyn Fn`), so each task builds its own from
  `UpArgs`, and the union walk builds one too. The byte-identity promise holds with no cancel; a
  cancelled pass stops wherever its tasks were.

### 3.2 The per-root pass

1. Serially, as today: build `Shared` and, per root, its node list (`nodes_with_stable_id`
   without `SYMBOL_ONLY`). A root with no nodes gives the `not-in-graph` row and no task.
   `all_nodes` is built here, in root order.
2. Each walked root is one task: a fresh visitor, a scratch, the walk, then the row (summary,
   callers, ancestors). The task returns `(row, states, visitor bytes, walk µs)`.
3. Tasks run as `roots.par_iter().with_max_len(1).map(task).collect::<Vec<_>>()` inside
   `reach_pool(n).install`. An indexed `collect` keeps root order whatever order tasks finish
   in. `states` and `walksUs` are sums and `visitorBytes` a max, folded over that vector.
4. Serial when `workers.count() <= 1`, or when the caller is already on a rayon worker
   (`rayon::current_thread_index().is_some()`, counter `impact.up.serial_in_rayon`, probe in tests): the same
   guard and reason as TLS-22's (`install` from a worker can run other jobs while it waits).
   The serial path is the same task function in a plain loop.
5. Then the union walk and its summary, serially, as today, on a scratch taken from the stash
   (no extra scratch beside it).

**Scratches.** A `WalkScratch` holds a `seen` word per graph node, so a fresh one per root
would allocate and zero one per walk. Instead a stash, `Mutex<Vec<WalkScratch>>`: a task takes
one (or makes one when the stash is empty) and puts it back after its walk. No more scratches
exist than tasks in flight (at most `n`). The serial path uses one, as today. A scratch's
contents never reach the answer: each walk resets it (`WalkScratch::reset`), which today's code
already relies on when walks follow each other.

### 3.3 Captions shared

`Captions` becomes usable from several tasks:
- `parsed: Mutex<HashMap<String, Arc<Vec<ObjectActions>>>>`;
- `actions(&self, unit) -> Arc<Vec<ObjectActions>>`.

The parse runs outside the lock: look up, and on a miss parse, then insert if absent and return
whatever the map holds. Two tasks may parse the same file once each; both results are equal (a
value depends only on its file), and the first one inserted is kept. `texts` is read-only. The
methods that take `&mut self` (`of`, `summary`'s callers) take `&self`. Captions' parse span and
`impact.up.caption_files` counter stay (step 0's).

### 3.4 The cost block

The resident engine walks serially and the CLI in parallel, and `tests/cli/engine_host.rs`
compares their up parts byte for byte, blanking only `walkMs` and `walksUs`. So every field left
in that comparison must not depend on the worker count:

- `perRootStates`, `unionStates`: sums of each walk's own states. Same at any count.
- `visitorBytes`: the max over walks of each walk's own visitor. Same at any count.
- `inventoryBytes`: unchanged.
- `scratchBytes` **changes meaning**: the sum of every scratch's capacity at the end, the scratch
  memory the pass held. With more than one worker it depends on which task got which scratch, so
  it can differ from run to run. Like `walkMs` it measures this run and is not part of the
  answer, and the comparison test blanks it too, with the reason. The same total also goes to a
  trace counter, `impact.up.scratch_bytes`. CHANGELOG entry under Changed.
  - Reviewed alternative: keep a comparable per-walk number. Nothing cheap is well defined: a
    reset keeps capacity, so even today's value depends on walk order, and a true per-walk need
    would mean tracking high-water marks inside the walk for a field nothing reads.
  - Readers: no other code reads the up part's `scratchBytes`. `cone_probe.rs` has its own field
    of that name, over its own scratch. A search of the owner's other repositories on 2026-11-12
    found no reader.
- `walksUs`: unchanged in meaning, the sum of the walks' own durations. In parallel it exceeds
  the pass's wall time; its doc says so.

No worker count is written into the output.

### 3.5 Memory

Each extra task in flight holds:
- a `WalkScratch`;
- its visitor `Up`, which is more than `Up::bytes()` counts;
- its summary's temporaries (graph-sized vectors, sort buffers);
- possibly one caption file's parse;
- its result row until the end.

The pool's threads (32 MiB stacks) already exist in a one-shot `impact`, because the literal pass
uses them. So no bound is computed from the cost block; the extra memory is measured directly,
as the **walk window's own peak**:
- When tracing is on and on Linux, `up` resets the process's peak mark at its start (it writes
  `5` to `/proc/self/clear_refs`, which resets `VmHWM`). It reads `VmRSS` there, and reads
  `VmHWM` from `/proc/self/status` after the union summary.
- It emits trace counters `impact.up.rss_start_mb` and `impact.up.peak_mb`. Their difference is
  what the walk added on top of what was already alive. If the reset fails, it emits
  nothing, and the run reports "not measured".
- Trace only; nothing changes when tracing is off.
- Why not the alternatives:
  - `perf_trace`'s phase sampler: its `read_rss` returns `None` off Windows, and
    `peak.post_attach_mb` is the maximum over all phases.
  - A process's peak: in `--part up` it is the larger of the program build's peak and the walk's,
    so the build can hide the walk's growth.
- The whole `alsem impact` process peak is still recorded, as the number the user sees, but only
  from **untraced** runs. The reset also resets what `ru_maxrss` (`timeit.py`'s child peak)
  reports, so a traced run's process peak is not trustworthy. Each count is therefore run twice:
  traced for the span and the walk window, untraced for wall, CPU and the process peak.

The resident engine stays serial, but it runs the reorganised code (the task function, the
shared captions with their lock, the scratch stash), so its `cdo-speed` ratio is measured, not
assumed, and must stay about 1.00.

## 4. Tests (each correctness guard with a discrimination proof)

A cfg(test) probe rides on `ReachWorkers`, as TLS-22's `PassProbe` does (`with_probe`). It sits
there and not on `UpArgs` because on the resident route `UpArgs` is built inside `UpJob::sited`,
out of a test's reach, while a revision's `pass_workers()` carries its probe. The probe records:
- the most tasks in flight at once;
- the tasks run and the scratches made;
- caption parses per file.

It can also hold chosen tasks. Every hold waits for a condition with a timeout (TLS-22's bounded
`first_wave`), so a broken guard fails the test instead of hanging it.

1. **Same answer as before the change, at 1 and 8 workers.** The reference is the up JSON that
   master `4f2459a6` writes for the same fixture, committed as a golden before the refactor. It is
   minted once with master's binary, and no regen path rewrites it (`REGEN_TEMP_GOLDENS` and
   `check-goldens --regen` skip it), or a later regen would quietly rebaseline it to the new code.
   The test says where it came from. The
   new code at `SERIAL` and at `ReachWorkers::new(8)` must equal it, with `walkMs`, `walksUs` and
   `scratchBytes` blanked. The fixture has:
   - several roots;
   - a root with `#if` arms (several nodes);
   - a page-action caption;
   - a not-in-graph root placed in the middle, not at the end.

   The test also asserts that each row's `stableId` equals its root, in order. The 8-worker run
   must show more than one task in flight, or it proves nothing.
   Break: collect the rows in completion order (a `Mutex<Vec>` push) instead of the indexed
   `collect`, with root 0 held until the others finish. The test must fail.
   Second break, for a bug both paths share: push the not-in-graph rows after the walked ones.
   The golden comes from the old code, so the test must fail at both counts.
2. **Captions under a race.** Task A misses on a file and is held between its miss and its insert
   until task B has looked the file up. Both rows carry the caption, and the map ends with one
   entry.
   Break: mark the file "being parsed" with an empty entry before parsing (a plausible way to
   avoid the double parse). Task B then reads the empty entry and its row has no caption, so the
   test must fail.
3. **The resident engine walks serially.** A `Revision` built with
   `with_reach_workers(ReachWorkers::new(3))` answers `impact-up` over at least 5 roots, with the
   first 3 tasks held until 3 are in flight (bounded). Its probe must show:
   - exactly 3 in flight;
   - one task per walked root;
   - `rayon::current_num_threads() == 3` seen inside a task.

   That proves the route carries the revision's count and the pool has exactly that many
   threads. A production revision (`SERIAL`) shows at most one task in flight over the same held
   roots. `GraphRevision` passes the `SERIAL` constant, which has no probe; it gets no test and
   its one line is reviewed.
   Break, the realistic regression: build the revision's `reach_workers` from
   `default_reach_workers()` instead of `SERIAL` (`revision.rs`, where the revision is made). The
   production-revision case then shows more than one task in flight. This needs at least 2
   cores; on one core the test prints that and skips the claim.
   No test pins that the batch runner's route (the CLI) passes `default_reach_workers()`: that
   count is a process-wide `OnceLock`, which carries no probe. A regression there makes the CLI
   serial again, with the same answer. It shows only as speed, in the measurement (section 5) and
   in the gate's impact ratchet times. This is a stated limit.
4. **Serial inside a rayon worker.** With `ReachWorkers::new(4)` and a hold that would need two
   tasks in flight, `up` called from inside another rayon pool runs one task at a time (probe
   maximum 1).
   Break: drop the guard. The probe then sees 2 or more in flight, so the test fails.
5. **Scratches are reused.** At `ReachWorkers::new(2)` over 6 roots, the probe counts at most 2
   scratches made.
   Break: make a fresh scratch per task. The test fails with 6.
6. **The comparison test drops `scratchBytes`.** `tests/cli/engine_host.rs` blanks it too. This
   weakens that test by one field, and the reason is stated in the test.

## 5. Measurement (CDO, release-fast, a quiet slot from the orchestrator)

- One-shot `alsem impact --diff` (the TLS-16 input) and `--base bc3ccb18~5`:
  - with `AL_SEM_REACH_WORKERS` 1, 2, 4, 8 and 16;
  - recorded: from a traced run, the `impact.up.roots` span and `job.0.impact-up`; from an
    untraced run, the whole wall, CPU and peak RSS;
  - the stdout of every run is equal to the count-1 run's, byte for byte.

  The knob also changes the literal pass, so the whole-command numbers mix the two. The up part's
  own time comes from its span.
- `alsem impact --part up` with the 699 roots given as `--root-id` (taken once from the count-1
  composite run's output; `--part up` takes roots only from `--root-id`). Run at 1, 2, 4, 8 and
  16, traced, recording:
  - the walk window's peak over its start (`impact.up.peak_mb` - `impact.up.rss_start_mb`): the
    walk's extra memory;
  - the span and the cost block;
  - from a second, untraced run: wall, CPU and the process peak (`timeit.py`).

  The composite `--diff` runs record the same counters.

  Its stdout, with the three blanked fields, is byte-compared with master `4f2459a6`'s
  `--part up` on the same roots: the outside reference on real data.
- `cdo-speed` (base master, head this branch): the resident engine runs the reorganised code
  serially, and its ratio must show about 1.00.
- Results, scripts and raw rows go in a note,
  `docs/superpowers/notes/2026-11-12-parallel-up-walk.md`, and in CHANGELOG.

**Keep or drop.** The change stays only if:
- the up part at 8 is clearly faster than at 1;
- the walk window's extra peak at each count is stated;
- the whole command's peak at 8 grows by no more than 5 % over master's on the same input (TLS-22's bar).

If the command's peak grows by more, the orchestrator decides. If 8 is not the best count for the up part, the note
says so. The count stays shared with the literal pass unless the numbers argue for a second
setting; that would need its own decision.

## 6. Not in this change

- Any parallelism in the resident engine (see the top).
- The union walk in parallel (0.1 s).
- Splitting one root's walk.

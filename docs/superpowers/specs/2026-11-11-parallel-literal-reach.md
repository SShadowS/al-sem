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

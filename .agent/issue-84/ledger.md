# Issue #84 ledger

## Claim

```json
{"run_id":"20261016-202704-b2a733","issue":84,"attempt":1,"session":"claude-code-session:cf05ae92-26c8-4a5d-a439-ddec4928220c","branch":"issue/84-perf-lsp-presize-the-dependency-a1","worktree":"U:\\Git\\al-sem-issue-84-a1","body_hash":"fdc30c7eb2d18ab2a0e4c0c0203b83ab04f9b8130b501d4f2ef88a0979e7d01a"}
```

Issue body hash: `fdc30c7eb2d18ab2a0e4c0c0203b83ab04f9b8130b501d4f2ef88a0979e7d01a`.

Reviewer roster: the operator told us (2026-10-15) to use `gpt-6.1-sol` and `claude-opus-5.5` instead of `gpt-6-astra` and `gemini-3.8-flash`. Marks are recorded as `reviews.sol` and `reviews.opus`, and we attest with `--substitute astra=sol --substitute flash=opus`.

## Classification

**bounded.** Two allocation patterns in one function, `build_dep_nodes`, change, with no change in meaning. A probe crate measures the result.

## Design

**Defect.** The cold dependency build in `build_dep_nodes` (`src/program/build.rs`) holds about three times its live routine data at its peak:

1. *Growth by doubling (step 2b, `build.rs:229-252`).* Each SymbolOnly dependency's `ingest_abi` result is appended with `routines.extend(abi_routines)` (and `objects.extend(abi_objects)`). The list grows by doubling, so on the 263-app repro one extend reallocates 25.5 MB → 51 MB. For that moment the old and new blocks are both live, and the final capacity is about twice the content.
2. *A second copy during dedup (`dedup_routines_preserving_genuine_overloads`, `build.rs:~700-797`).* After computing one `Verdict` per sorted routine, the function builds `out = Vec::with_capacity(routines.len())`, drains `routines` into it, and replaces the list. That holds the 51 MB buffer and a 27 MB copy at once.

**Fix** (revised after spec round 1; S1-S12 in the register):
1. *Count first, reserve once (S1).* This needs exact per-app counts.
   - One shared helper in `abi_ingest.rs`, `load_abi(unit, cache) -> Option<Arc<SymbolReferenceAbi>>`, holds the cache lookup that `ingest_abi` does today (`cache.get`, then `get_or_load` on the `app_path`, then `None`). `ingest_abi` calls it.
   - A new `abi_node_counts(unit, cache) -> (usize, usize)` also calls it and returns `(abi.objects.len(), Σ abi_obj.routines.len())`. These are exactly the numbers of `ObjectNode`/`RoutineNode` that `ingest_abi` pushes: one per ABI object and one per ABI routine, with no filtering (`abi_ingest.rs:492`, `:541-643`).
   - Step 2b makes a first pass over the same SymbolOnly units (`unit.source.is_none()`) and sums the counts, then calls `objects.reserve_exact(Σo)` and `routines.reserve_exact(Σr)`. The second pass is today's loop unchanged: per app `intern`, `ingest_abi`, `abi_ingest_errors`, `share_strings` and `extend`. Each per-app vector is consumed and freed before the next app is ingested.
   - `AbiCache::get_or_load` keeps every parsed ABI (`abi_ingest.rs:~212`), so loading in the count pass holds no memory the old loop did not, and the second pass hits the cache. `parse_count` and the load errors are the same: the same units are loaded once each, only earlier.
   - Contents and order are identical. The peak during step 2b is the presized list plus ONE app's vectors, not 2x (S1).
2. *Presize the per-app vectors (S2).* `ingest_abi` allocates `objects`/`routines` with `Vec::with_capacity(count)`, using the same counts, instead of `Vec::new()`. So no single app's vector carries doubling slack, and Base Application's vector is exact.
3. *Dedup in place.*
   - Replace the drain-into-`out` loop with `routines.retain_mut(...)`, driven by an iterator over `verdicts` zipped with `contexts`. `retain_mut` visits each element exactly once, in order, and keeps the order of the elements it retains (std doc).
   - Each visit applies the same per-verdict mutation as today: set `preproc_context` when `Some`, and for `Drop` it is always `None`; `KeepCollapsed` sets `abi_overload_collapsed` and `AbiParams::CollapsedUntrusted`; `KeepAliased` sets `source_overload_aliased`. The closure returns `false` only for `Drop`.
   - Alignment (S9): `assert_eq!(verdicts.len(), routines.len())` before the loop, and `.expect(...)` on each `next()` inside it. The old `zip` silently truncated on a mismatch.
   - **No `shrink_to_fit`** (S7). The old code left capacity = the pre-dedup length (`with_capacity(routines.len())`), and an in-place `retain_mut` on the presized list leaves the same capacity, so the extra realloc would be a change, not parity.
   - The helper is also called by `assemble_program_graph` (`build.rs:324`) for workspace routines (S11), which gets the same in-place behaviour, with no change in meaning.
4. Objects take the same count-first path. Their dedup is already in place (`dedup_by`).

**Out of scope.** Other allocations in `build_dep_nodes` are not touched: ABI parsing on a cold `AbiCache`, the sort's scratch buffer, and the dependency cache. Nothing in the order or content of the graph changes.

**Measurement tool** (S3-S6). `tools/census-probe` (tracked, own workspace, counting `#[global_allocator]`) already measures the build peak in `symbols` mode. It changes as follows:
- **Large-allocation log.** Every `alloc`, `alloc_zeroed` AND `realloc` whose NEW requested size is at least 8 MiB is logged with its new size, the old size (for `realloc`), the live heap at that moment, and a backtrace (S3). A realloc whose old and new blocks are both live is reported as the transient `old + new` in the run notes; the counting allocator's net accounting cannot see it.
- **Safe backtraces** (S4):
  - a thread-local reentrancy guard declared allocation-free (N3): `thread_local!{ static IN: Cell<bool> = const { Cell::new(false) } }`, with log records pushed only while the guard is set;
  - frames captured unresolved (`Backtrace::force_capture` or `backtrace::Backtrace::new_unresolved`) and resolved only when printed;
  - `debug = "line-tables-only"` in the probe's `[profile.release]`, so frames are named.
  - Attribution uses the census-hook phase marks if a frame is inlined away.
- **Metric** (S5): "largest single allocation inside `build_dep_nodes`" means the largest `Vec<RoutineNode>`/`Vec<ObjectNode>` buffer allocated or reallocated in that phase. Its size must be a multiple of `size_of::<RoutineNode>()`/`size_of::<ObjectNode>()`, which the probe prints, and its backtrace must pass through `build_dep_nodes` or the dedup. Every other allocation of 8 MiB or more in the phase is listed too, so nothing is hidden. The "build peak" is the probe's existing per-build peak in symbols mode.
- **Corpus** (S6): the 7 refapp roots copied to a scratch dir, with `.alpackages` = `C:/ProgramData/BcContainerHelper/compiler-cache-15ff3c5d109b/symbols` (263 `.app`, the issue's BcContainerHelper compiler cache). GUID grouping keeps one version per app, so the run records the number of SymbolOnly units actually ingested and `size_of::<RoutineNode>()`.
  - **Hard gate:** before any code change, the master `f3f0b304` run must reproduce a 51,032,576-byte `Vec<RoutineNode>` realloc. If it does not, the corpus is not the issue's repro, and that is recorded and decided before going on, never measured around. The gate also records the probe's in-phase peaks (the census-hook `on_mark` phases) and checks which phase holds the build peak (N2). The pass criterion is the issue's: the OVERALL build peak drops by ≥ 40 MB (N2/N4). The phase peaks are diagnostic evidence only. If the overall drop falls short while the step 2b/dedup phase drops by ≥ 40 MB, that is recorded under `## Not delivered` as an operator decision, never reported as a pass.
  - Before and after use the same corpus, probe profile and machine.

## Assumption probes

| probe | result | evidence |
|---|---|---|
| the repro corpus exists locally | confirmed: `U:/Git/CentralGauge/harness-tasks/refapp` has the 7 roots (Core, Fleet, Integration, Leasing, Rental, Reporting, Test); `C:/ProgramData/BcContainerHelper/compiler-cache-15ff3c5d109b/symbols` holds exactly 263 `.app` (400 MB) | `ls`, `find -name '*.app'` |
| a byte-census tool for symbols-mode LSP builds exists | confirmed: `tools/census-probe` (counting allocator, `symbols <root>...`, peak per build), tracked, own `[workspace]`, built into `CARGO_TARGET_DIR=C:/lpt` | `tools/census-probe/README.md`, `src/main.rs:13-119` |
| the doubling site and the dedup copy are where the issue says | confirmed: `routines.extend(abi_routines)` at `build.rs:251` inside the step 2b loop; `Vec::with_capacity(routines.len())` + `drain(..)` at `build.rs:771-797` | source read at master `f3f0b304` |
| `retain_mut` keeps order and visits each element once, in order | std doc of `Vec::retain_mut`: "visits each element exactly once in the original order, and preserves the order of the retained elements" | Rust std docs |
| the 51 MB block reproduces on master | **REPRODUCED (T1, the hard gate).** On master `f3f0b304`: a `realloc` at `build.rs:251` grows the `Vec<RoutineNode>` from 25,516,288 B to **51,032,576 B** (capacity 227,824 × `size_of::<RoutineNode>()` 224). 263 SymbolOnly units were ingested. The pre-dedup list holds 120,336 routines (1.89× slack). The overall build peak is **266.8 MiB**, in phase `3.dep_layer`, reached when the dedup copy (26,955,264 B, `build.rs:771`) is live next to the 51 MB buffer. | `tools/census-probe/runs-issue-84/before/refapp263-symbols.txt`; `.agent/issue-84/t1.md` |
| exact per-app counts can be had without building nodes | confirmed: `ingest_abi` pushes one `ObjectNode` per `abi.objects` entry and one `RoutineNode` per `abi_obj.routines` entry, unfiltered | `abi_ingest.rs:492`, `:541-643` |
| loading the ABIs in a count pass costs no extra memory | confirmed: `AbiCache::get_or_load` inserts every parsed ABI into the cache, and `ingest_abi` loads through it | `abi_ingest.rs:183-224`, `:455-468` |
| `build_dep_nodes` is the only builder of these lists | confirmed by both reviewers: `DepCache::get_or_build` only calls the `build_dep_nodes` closure; the dedup helper's other caller is `assemble_program_graph` (`build.rs:324`) | `dep_cache.rs:126-152`, `build.rs:132-134,321-324` |
| existing dedup tests check `preproc_context` | no: the tests at `build.rs:~1386-1720` cover collapse, lockstep, singles, source duplicates and the sibling-app path (which compares the function with itself), but none checks the context intersection | opus r1 |

## Base/Head SHAs

- B = `f3f0b304b7a62780e03980b1c49fd65a2faf3f52` (origin/master)
- H = `a5ba2293aef271e6ecf011f7df0df0b041ef6c01` (approved code head; the evidence commit follows it)

## Timeline

- Worktree created at master `f3f0b304`.
- Classified as bounded; design written.
- Spec panel (sol + opus, the operator roster). Register hash = the first 16 hex of sha256(ledger.md) at the round:
  - r1 (spec_rounds 1; pi_calls 2): both REJECTED the collect-then-reserve peak, the realloc-blind logger and the `shrink_to_fit` reasoning. Opus added presizing, backtrace safety, the metric definition, the corpus gate and D2. 12 findings S1-S12, all folded into the design.
  - r2 (spec_rounds 2; pi_calls 4): both accepted S1-S9 and S11-S12, and both re-raised S10 (D2's reorder break could not fail, because the sort removes loop order). Opus raised N1 (count exactness unpinned), N2 (peak phase) and N3 (allocation-free guard). Fixed: D2 now checks the error order, `parse_count` and capacity; D3 was added; the gate checks the peak phase; the guard is a `const` `Cell`.
  - r3 (spec_rounds 3; pi_calls 6): opus accepted all 15. Sol re-raised N1 (D3's "seeded-then-loaded" path never loads) and N2 (the phase peak must not stand in for the overall peak). Opus raised the same two as N4/N6, plus N5 (capacity can equal len by chance). Fixed: D3 path 2 is an unseeded real `.app` with `parse_count` checks; the pass criterion is the OVERALL peak, with phase peaks as diagnostics and a shortfall going to Not delivered; counts are off the growth path (D2 total 5, D3 3 or 5), with reserve/`Vec::new()` breaks.
  - r4 (spec_rounds 4; pi_calls 8): both accepted all 18. Opus raised N7 (D3's break (a) needs routines on two or more objects). Fixed: D3's fixture spreads the routines over at least 2 objects (2 + 1).
  - r5 (spec_rounds 5; pi_calls 10): both accepted all 19 at hash `81494a755f31f3b2`; no new findings. CONVERGED.
- Plan: T1 = probe (an `alloc`/`realloc` log of ≥ 8 MiB) + the master baseline, the HARD GATE; T2 = the fix + D1/D2/D3 (TDD); T3 = the after-measurement + the CDO histogram compare. plan_tasks 3.
- T1 (opus; task_attempts T1 = 1): census-probe extended with an `alloc`/`alloc_zeroed`/`realloc` log of ≥ 8 MiB (const-`Cell` guard, unresolved backtraces, `line-tables-only`). It also needed a compile fix for master, a new `dependency_diagnostics` field in the `model` destructure. Gate REPRODUCED (see the probes).
- T2 (opus; task_attempts T2 = 1): fix + D1/D2/D3, TDD, PASS.
  - Red: D3 failed to compile (`abi_node_counts` missing), then, with the helpers in, failed on a real capacity check (`left: 4 right: 2`). D1 and D2 pass on master by design: they pin unchanged behaviour, and D2's capacity check matches master's `with_capacity(len)` rebuild.
  - Green: `--lib` 2040 passed; `check-goldens` 8/8 green with ZERO golden files changed; clippy clean.
  - All 9 discrimination breaks failed (see below).
- T3 (conductor): after-measurement on the same corpus, probe binary and machine (the probe was rebuilt on the branch). Raw output in `tools/census-probe/runs-issue-84/after/` (2 runs, identical peaks):
  - largest node buffer 51,032,576 B → **26,955,264 B** (the single up-front `Vec::with_capacity` reservation of `Vec<RoutineNode>`, capacity 120,336 = the exact pre-dedup count);
  - no doubling realloc and no dedup copy remain;
  - overall build peak **266.8 → 228.4 MiB** (−38.4 MiB = **−40.3 MB**; the issue writes 51,032,576 B as "51 MB", so MB = 10⁶ B). Still in `3.dep_layer`.
  - Every ≥ 8 MiB record after: 26,955,264 (the reserved list), 10,655,232 (Base Application's exact per-app `Vec<RoutineNode>`, capacity 47,568), 8,912,912 (the `StrPool` rehash, untouched), 13,477,632 (the sort scratch, untouched), plus the two `SubscriberIndex` rehashes in `7.event_edges` (unchanged).
  - CDO histogram: `aldump --program-call-graph-stats U:/Git/DO-cdo-baseline/Cloud`, built in the `cdo` profile from master `f3f0b304` (a temporary detached worktree) and from the branch. Both JSONs have sha256 `c032c8db56f1fd070fc4b7e4f1971a73019a4c8569db81a6aaeb7e0c1a1594ef`, so they are **byte-identical** (`cmp`).
- Sonnet review of T2: 7 findings R1-R7. R1 (important: nothing pinned that the dedup runs in place) was fixed with D1's `as_ptr()` check; the proof restores a with_capacity+drain rebuild and D1 fails at `build.rs:1880` "the dedup must reuse the buffer (in place), not build a second list". R2-R5 and R7 were fixed; R6 (the probe lock) is left untracked (see the register). Commit `5d16a385`.
- `code-review` skill at high (Claude Code built-in, run as given): 10 findings CR1-CR10.
  - CR1-CR3 (important) found a retained-slack regression: the in-place dedup kept the growth slack of the workspace list and of embedded dependency layers, where master's rebuild had trimmed it.
  - T4 (opus) fixed it: one reservation before step 2 (source summaries + SymbolOnly ABIs), and the dedup calls `shrink_to(pre_dedup_len)`, which is master's capacity. D2 gained a source dependency; D4 is new.
  - CR4/CR5 were deferred as discoveries; CR6/CR8 were refuted. Commits `3af3f02f` and `62afc25c` (README). The after-runs were re-taken on the final code: the same peaks.
- Final panel r1 (final_rounds 1; pi_calls 12): both re-raised N5. D2's object total of 4 lies on Vec's growth path (`ObjectNode` 320 B means a minimum capacity of 4), so a full revert to `Vec::new()` passed every test. Opus raised F1-F4 (the reservation unpinned, a stale ledger, lost probe records, the cdo-gate run not recorded); sol re-raised R6 (not an exact pin) and flagged a stale comment. T5 (opus) fixes them.
- T5 (opus; task_attempts T4 = 3): `debug_assert_eq!` on the lengths and capacities after step 2b, D2's object total moved to 5, the probe's put-back fixed, `backtrace = "=0.3.76"`, and the comment reworded. Breaks A/B/C all fail. Commit `4f1abd8b`.
- Final panel r2 (final_rounds 2; pi_calls 14): opus accepted all 41 and raised G1 (the README and ledger named the wrong code for the after-runs). Sol accepted all except F2/F4, which it re-raised as pending gate evidence, and raised F6 (the lock-reuse wording). Both were fixed in commit `a5ba2293` (README only) and in the ledger.
- Gates on `a5ba2293`: all 4 exit 0 (see Gate results). `origin/master` = `f3f0b304` = the branch base, so no rebase is needed.
- Final panel r3 (final_rounds 3; pi_calls 16): both accepted all 43; sol found nothing new. Opus raised H1 (stale record text: CR10's evidence, and this timeline). Fixed in the register and here.
- Final panel r4 (final_rounds 4; pi_calls 18): both accepted all 44; opus found nothing new. Sol raised I1 (the register summary still said 43 entries). Fixed.
- Final panel r5 (final_rounds 5; pi_calls 20): both accepted all 45; no new findings. CONVERGED.
## Acceptance matrix

| Acceptance item | Proof |
|---|---|
| No change in meaning: every golden family byte-identical; CDO histogram unchanged | `scripts/check-goldens` green with ZERO golden files moved (no regen), and `scripts/cdo-gate` green, whose ratchets pin the CDO `--program-call-graph-stats` histogram. Plus a direct before/after `target/cdo/aldump.exe --program-call-graph-stats U:/Git/DO-cdo-baseline/Cloud` JSON compare (byte-identical). |
| On the 263-app symbols-mode repro, the largest single allocation inside `build_dep_nodes` ≤ 30 MB (from 51 MB), and the build peak drops by ≥ 40 MB, both measured with an allocation logger before and after | `census-probe symbols <7 roots>` with the new ≥8 MiB `alloc`/`realloc` log, run on master `f3f0b304` (before; it must reproduce the 51,032,576-byte `Vec<RoutineNode>` realloc, see the hard gate in the Design) and on the branch (after), with the same corpus, probe profile and machine. It reports the largest `Vec<RoutineNode>`/`Vec<ObjectNode>` buffer in the `build_dep_nodes` phase (≤ 30 MB), lists every other ≥8 MiB allocation there, and gives the build peak MiB (a drop of ≥ 40 MB). Raw outputs are committed under `tools/census-probe/runs-issue-84/`, with the number of ingested SymbolOnly units and `size_of::<RoutineNode>()`. |
| A unit test pins the in-place dedup (same order, same genuine overloads kept as the old version, on a hand-built list with duplicates), with a discrimination proof | **D1** (unit, `build.rs` tests), on a hand-built, already-sorted list:<br>- exact duplicates (same id + `param_sig_key`; Drop), placed directly BEFORE a collapsed run and before an aliased run, so the verdict iterator must advance on every element;<br>- genuine SymbolOnly overloads whose fingerprints collide (KeepCollapsed);<br>- source-tier aliases sharing one id (KeepAliased), each with a distinguishing payload (e.g. its `name` or `return_type`);<br>- duplicates with DIFFERENT `preproc_context`, so the intersection is exercised;<br>- plain singles.<br>It asserts a literal ordered projection of every survivor: `id`, `param_sig_key`, the distinguishing payload, `preproc_context`, `abi_overload_collapsed`, `source_overload_aliased` and the `AbiParams` variant (S8). The expected output is stated literally, never computed by calling the old function.<br>Discrimination proofs: (a) skip the context assignment; (b) advance the iterator only on kept elements; (c) drop the `CollapsedUntrusted` demotion; (d) keep `Drop` entries. Each must fail D1. |
| (Step 2b contents, added for S10) | **D2** (unit): `build_dep_nodes` over a snapshot that interleaves at least 2 seeded SymbolOnly units (seeded through `AbiCache::seed`) with at least 2 UNSEEDED units, each with its own cache key and an `app_path` that does not exist (ingest errors).<br>FINAL FORM (after CR7 and F1/N5): the fixture also holds a SOURCE dependency (2 objects, 2 routines) that goes through step 2.<br>It asserts, stated literally: the exact `dep_objects` and `dep_routines` (ids, in order); the `abi_ingest_errors` in order; `parse_count` = the number of unseeded units (2); and `dep_objects.capacity()` = 5, `dep_routines.capacity()` = 7. The object total 5 is off Vec's growth path for `ObjectNode` (minimum capacity 4, then 8).<br>`shrink_to(pre_len)` makes the final routine capacity equal the raw total whatever was reserved. So the reservation itself is pinned by `debug_assert_eq!`s in `build_dep_nodes` (after step 2b: the lengths AND capacities of `objects`/`routines`/`dep_meta` equal the counted totals), and they run in every debug test that builds a dependency layer.<br>Node order after the step 4 sort does not depend on loop order, because the `AppRef`s come from step 1. So the order breaks target the errors. |
| (capacity parity, added for CR1) | **D4** (unit): `dedup_routines_preserving_genuine_overloads` on a `Vec::with_capacity(8)` holding 4 routines with one exact duplicate ends with `len() == 3` and `capacity() == 4`, the pre-dedup length, which is master's capacity. Break: drop the `shrink_to`, and D4 fails (8 vs 4). |
| (count exactness, added for N1) | **D3** (unit, `abi_ingest.rs` tests) covers the four load paths:<br>(1) cache hit by `AbiCache::seed`;<br>(2) a REAL load: an UNSEEDED tiny `.app` built with `crate::engine::deps::app_package_zip::test_apps::build_app` (as the existing `abi_ingest.rs` tests do), with `parse_count` 1 after counting and still 1 after ingestion (N1/N6);<br>(3) no `app_path` → `(0, 0)`;<br>(4) a failing path → `(0, 0)` with the error kept.<br>On each path it asserts `abi_node_counts(u, c) == (r.objects.len(), r.routines.len())` and `capacity == len` for both vectors, where `r = ingest_abi(u, …)`. The fixture's per-app routine count is 3 or 5, so that growth by doubling cannot land on `capacity == len` by chance (N5). The routines are spread over at least two ABI objects (e.g. 2 + 1 = 3), so break (a) gives a wrong count (N7).<br>Discrimination: (a) `abi_node_counts` counts only the first object's routines; (b) `ingest_abi` goes back to `Vec::new()`. Each must fail D3. |

## Discrimination proofs

Every break was made with Edit, run, then reverted with Edit and the file `touch`ed (T2). Logs: `t2-proof-*.log` in the session scratchpad.

| test | break | decisive failing line | after revert |
|---|---|---|---|
| D1 | (a) skip the context assignment | `build.rs:1897`: left context `[("CLEAN", true), ("X", true)]` vs right `[("CLEAN", true)]` | pass |
| D1 | (b) advance only on kept elements | `build.rs:1900`: left 1 survivor vs 6 expected | pass |
| D1 | (c) drop the `CollapsedUntrusted` demotion | `build.rs:1896`: left `"Complete"` vs right `"CollapsedUntrusted"` | pass |
| D1 | (d) keep `Drop` entries | `build.rs:1897`: left 9 entries vs 6 | pass |
| D2 | reorder the second pass | `build.rs:2063`: errors `[(AppRef(4),…u2),(AppRef(2),…u1)]` vs `[(AppRef(2),…u1),(AppRef(4),…u2)]` | pass |
| D2 | skip one unit | `build.rs:2037`: 1 object id vs 3 expected | pass |
| D2 (T2 form, superseded) | skip `routines.reserve_exact` | `build.rs:2079`: capacity `8` vs `5` | pass |
| D1 (R1) | restore a `with_capacity` + `drain` rebuild after `retain_mut` | `build.rs:1880`: "the dedup must reuse the buffer (in place), not build a second list" | pass |
| D2 (T4) | leave the step-2 term out of the object total | `build.rs:2115`: object capacity `6` vs `4` (T4 fixture) | pass |
| D4 (T4) | drop `routines.shrink_to(pre_len)` | `build.rs:2149`: capacity `8` vs `4` | pass |
| D2 + debug_assert (T5) | A: all three `Vec::with_capacity(total)` → `Vec::new()` | `build.rs:285`: capacities `(8, 8, 4)` vs totals `(5, 7, 2)`; 5 tests fail | pass |
| D2 + debug_assert (T5) | B: `routine_total` without the ABI term | `build.rs:280`: lengths `(5, 7, 2)` vs totals `(5, 2, 2)` | pass |
| D2 + debug_assert (T5) | C: `dep_meta` not reserved | `build.rs:285`: capacities `(5, 7, 4)` vs `(5, 7, 2)`; 5 tests fail | pass |
| D3 | (a) count only the first object's routines | `abi_ingest.rs:2122`: `(2, 2)` vs `(2, 3)` | pass |
| D3 | (b) `ingest_abi` back to `Vec::new()` | `abi_ingest.rs:2120`: capacity `4` vs `2` | pass |

## Findings register summary

45 entries:
- spec panel: S1-S12 and N1-N7;
- Sonnet review: R1-R7;
- code review at high: CR1-CR10;
- final panel: F1-F6, G1, H1 and I1.

Dispositions:
- **fixed:** everything else;
- **refuted** (minor, with evidence): CR6 (a double cache lookup costs nothing measurable) and CR8 (exact-capacity asserts);
- **deferred** (minor): R6 (the probe lock stays untracked; backtrace is pinned exactly) and CR4/CR5 (recorded under Discoveries with their measurements).

No blocking entry is deferred.

## Gate results

All 4 gates ran under the supervisor on head `a5ba2293aef271e6ecf011f7df0df0b041ef6c01` and returned exit 0. Logs are under `.agent/runs/20261016-202704-b2a733/logs/`.

| gate | exit | seconds | log |
|---|---|---|---|
| ci-steps-all | 0 | 574.5 | `ci-steps-all/20261016T215840-dc88814a.log` |
| check-goldens-coverage | 0 | 0.3 | `check-goldens-coverage/20261016T220815-dcf972dc.log` |
| check-goldens | 0 | 64.6 | `check-goldens/20261016T220815-4bddddf6.log` |
| cdo-gate | 0 | 234.4 | `cdo-gate/20261016T220920-e811d94f.log` |

Afterwards the worktree's `git status --porcelain -- . ':!.agent'` showed only the probe's own untracked `tools/census-probe/Cargo.lock`, which the probe builds create. It was removed, and the status is now empty. Earlier gate runs, on `5d16a385`, `62afc25c` and `4f1abd8b`, were stopped when review fixes changed the code, so they are not used.

The direct CDO compare (acceptance item 1) gave the same `aldump --program-call-graph-stats` JSON, sha256 `c032c8db…`, on master `f3f0b304`, on `3af3f02f` (T4) and on `4f1abd8b` (T5). `a5ba2293` changes only a README.

## Discoveries

1. **(from CR4) `ingest_abi` builds a per-app `Vec` that `build_dep_nodes` then copies with `extend`.**
   - Subsystem `program / dependency layer build`; locator `src/program/abi_ingest.rs` `ingest_abi`; `src/program/build.rs` step 2b.
   - Symptom: on the 263-app symbols repro (the #84 after-run), Base Application's 10,655,232 B per-app `Vec<RoutineNode>` (capacity 47,568) is live next to the 26,955,264 B reserved list during the `extend`. That is a transient second copy of every app's nodes.
   - Capability: ABI nodes are appended straight into the layer's reserved lists (e.g. `ingest_abi_into(&mut objects, &mut routines)`, with `share_strings` over the appended tail), so no per-app intermediate exists.
   - Acceptance: on the same repro, no per-app node buffer ≥ 8 MiB is logged in `build_dep_nodes`; contents are byte-identical (goldens, CDO stats).
   - Reproducer: `census-probe symbols <refapp 7 roots>` with the 263-app `.alpackages`; record #2 in `tools/census-probe/runs-issue-84/after/`.
   - Not pre-existing as a defect: master had the same copy inside its doubling. Kind: perf.
2. **(from CR5) Step 4's stable `sort_by` allocates a len/2 scratch buffer, and after #84 that is the build peak.**
   - Subsystem `program / dependency layer build`; locator `src/program/build.rs` step 4 (`routines.sort_by(|a, b| a.id.cmp(&b.id))`).
   - Symptom: on the #84 after-run, the 13,477,632 B driftsort scratch (60,168 `RoutineNode`s) is the allocation at which the build peak (228.4 MiB) is reached.
   - Capability: the sort needs no len/2 scratch: an unstable sort keyed on `(id, original index)`, or sorting an index permutation, with the same first-wins dedup order.
   - Acceptance: the scratch allocation is gone from the census log; goldens and CDO stats are byte-identical; a test pins that equal-id order (first-wins) is unchanged.
   - Reproducer: as above; record #4 in `runs-issue-84/after/`.
   - Pre-existing (master had it too). Kind: perf.

## Not delivered

Nothing. All three acceptance items are met:
- (1) goldens and the CDO JSON are unchanged;
- (2) the largest node buffer is 26,955,264 B (≤ 30 MB), and the overall build peak dropped from 266.8 to 228.4 MiB (40.3 MB ≥ 40 MB);
- (3) D1 pins the in-place dedup, with discrimination proofs.

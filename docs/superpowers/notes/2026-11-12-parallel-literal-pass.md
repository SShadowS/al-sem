# TLS-22: the literal pass runs roots in parallel (measured, 2026-11-12)

Spec: `docs/superpowers/specs/2026-11-11-parallel-literal-reach.md` section 8 (reviewed by sol and
Opus, three rounds). Plan: `docs/superpowers/plans/2026-11-11-tls22-parallel-literal-pass.md`.

**What changed.** The literal pass (`literal_reach_entries`, span `prove.reach_effects`) ran the
roots of a read one after another. In a one-shot CLI read (`alsem impact`, `digest`, `prove`) it
now runs them on a pool of a fixed number of threads (`REACH_WORKERS` = 8, capped by the core
count), one root per task. Everything the roots share is either read-only or a cache whose value
depends only on its key, so the answer is the same at any worker count. **The resident engine (the
context server) keeps the pass serial** (owner decision 2026-11-12, q-20261112T032637-1269d8fc,
below: its 8 GiB budget kept only 2 % headroom with 8 workers).

## How it was measured

- Binary: `alsem` built with `release-fast` at `d9f4135f` (TLS-22 on master `f1e9d618`).
- Host: server1, 22 cores, the orchestrator's quiet window (no build or gate beside it), one job at
  a time, 2026-11-12 00:58-02:08 UTC. `timeit.py` waited up to 3 minutes for 90 % idle before each
  one-shot run; most started at 86-89 % idle, so they are flagged `noisy` in the raw rows (load
  average 2.5-8 at the start of each run). MemAvailable never fell below 30 GB.
- One-shot: the gate's two calls on the pinned CDO baseline, `impact --diff` (699 roots) and
  `impact --base bc3ccb18~5 --no-tests`, with `ALSEM_TRACE`, at `AL_SEM_REACH_WORKERS` 1, 8, 2, 4,
  16 (diff) and 1, 8 (base). Span times are self times from `scripts/trace_summary.py`.
- Resident: one `alsem mcp` server per configuration (1 worker, 8 workers, `--low-mem`), asked in
  turn: a cold explain (the model build), a digest of 20 roots (the `Fill("witness")` case), two
  10-root digests at once, the 699-root diff impact (the streamed `Trees` case), and two of those at
  once. Peak memory is the sum of VmHWM over the server's processes after each step.
- Scripts and raw rows: `parallel-literal-pass/` next to this note (`measure.sh`, `resident.py`,
  `lowmem_x2.py`, `runs.jsonl`, `resident.jsonl`; their paths point at the session's scratchpad).

## One-shot results

| Workers | Diff: `prove.reach_effects` | Diff: wall | Diff: peak RSS | Base: `prove.reach_effects` | Base: wall | Base: peak RSS |
|--------:|----------------------------:|-----------:|---------------:|----------------------------:|-----------:|---------------:|
| 1 | 87.1 s | 218 s | 3,297 MiB | 147.4 s | 321 s | 3,611 MiB |
| 2 | 51.3 s | 185 s | 3,322 MiB | | | |
| 4 | 35.5 s | 173 s | 3,342 MiB | | | |
| 8 | **29.4 s** | **162 s** | 3,449 MiB | **53.0 s** | **253 s** | 3,760 MiB |
| 16 | 35.6 s | 196 s | 3,789 MiB | | | |

- **8 workers is the best count.** The span still falls 17 % from 4 to 8 and gets worse at 16
  (and 16 costs 15 % more memory). `REACH_WORKERS` stays 8.
- **The pass's time falls 66 % (diff) and 64 % (base); the whole call's wall falls 26 % and 21 %.**
  What is left of the wall is mostly the up walk (`job.0.impact-up`, 39-49 s, serial: TLS-21) and
  the witnesses (`digest.roots`, 32-53 s, already parallel).
- **Peak memory rises 5 % at 8 workers** (diff +153 MiB, base +149 MiB).
- **Output:** the diff call's JSON at 8 workers is byte-identical to 1 worker.
- **The shared suffix cache:** searches built (duplicates, two workers building the same tree at
  once): diff 90 at 1 worker, 103 (13) at 8, 110 (20) at 16; base 182 at 1, 220 (38) at 8. No
  evictions. The duplicates are cheap enough that the in-flight marker the spec left out is not
  needed.
- Lock contention on the shared caches was not traced separately: the span's fall to 29.4 s at
  8 workers leaves little room for it to matter.

## Resident results (`alsem mcp`, CDO)

"Two at once" below means two asks sent together. They do NOT run together: the MCP pool sends
every call for one (workspace, universe) to one engine child, and the child runs heavy asks
(impact, digest, prove) one at a time on its one heavy worker (`src/mcp/pool.rs`,
`src/engine/host/child.rs`). So the second impact runs after the first, on top of what the first
left resident; the walls show it (the pair takes about twice one ask at any worker count).

| Config | Model build | 20-root digest | Two digests | 699-root impact (its peak) | Two impacts in a row | Peak after them |
|--------|------------:|---------------:|------------:|---------------------------:|---------------------:|----------------:|
| 1 worker | 36.7 s | 3.7 s | 3.9 s | 206.3 s (6,904 MiB) | 381.6 s | **7,929 MiB** |
| 8 workers | 33.6 s | 4.3 s | 3.9 s | 140.1 s (6,890 MiB) | 263.6 s | 8,089 MiB |
| 8 workers, rerun at `e1f36353` | 43.6 s | 5.1 s | 5.7 s | 169.7 s (**6,985 MiB**) | 260.1 s | **8,030 MiB** |
| `--low-mem` | 31.5 s | 3.0 s | 3.3 s | 194.1 s (5,561 MiB) | one refused | 5,615 MiB |

- **Headroom against the 8 GiB budget (8,192 MiB)** after two 699-root impacts: 2.0 % with 8
  workers (8,030 MiB; 8,089 in the first run), 3.2 % serially (7,929 MiB). The figure is the sum of
  VmHWM over the server and its engine children, an upper bound on the engine's own RSS, which is
  what the budget meters. The gap between them, 100-160 MiB, is about the run-to-run noise (the
  model build's peak moved 5,471 -> 5,587 MiB between runs), so it is not a precise number.
- **The decision.** First (q-20261112T022816-833a1bd9, from the design mantra, principle 21): keep 8
  workers for a lone ask and lower the count while two or more run. That was built and tested, and
  measured as above: it never fires, because two passes never run at once in one engine child, and
  the second ask's peak is its own pass on top of the first's retained state. So (q-20261112T032637-1269d8fc,
  recommended by this lane): **the resident engine runs the pass serially**, which keeps the old
  ~3 % headroom; the one-shot CLI keeps 8 workers, where the gate spends its time. The inert
  concurrency policy was removed rather than kept as a guard that cannot fire, and with the resident
  engine serial the admission estimate needs no worker term, so that went too.
- **What the resident engine gives up:** a 699-root impact 206 -> 140 s (32 %) with 8 workers.
  Test: `the_resident_engine_runs_the_literal_pass_serially` (default mode and `--low-mem`).
- **What the budget does when it is passed** (`resident/budget.rs`, module doc): admission checks
  each reservation BEFORE the work, `rss + reserved + estimate <= budget`, with the estimate the
  largest of the case's last three observed peaks for that size class (by roots), or the CDO table
  before any is observed. Work is never aborted for memory. So the first ask whose real peak is
  above its estimate can overshoot the budget; it counts an overrun, its observed peak raises the
  case's estimate, and no new reservation fits until the RSS is back under the budget. After one
  such ask, the next ones are refused cleanly (as `--low-mem`'s second ask was, below), not
  overshot.
- **The one-shot CLI at 16 workers or on a bigger workspace.** Production does not run 16: the
  count is the constant 8 (capped by the cores); only `AL_SEM_REACH_WORKERS` changes it, for
  measurement. At 16 the one-shot diff's peak rose 15 % over serial, against 5 % at 8. A one-shot
  read has no memory budget; its extra over serial is about 7 times the largest root's working set,
  so it grows with the per-root search size.
- The admission estimates (`resident/budget.rs`) are unchanged: the resident engine is serial.
- **`--low-mem`: two 699-root impacts at once, the second is refused** ("memory budget: the trees
  needs about 1204 MiB; budget 6144 MiB, resident 5153 MiB ... retained floor 5153 MiB", rerun with
  `lowmem_x2.py`). That is the budget rule as it was: `--low-mem` runs the pass serially, so the
  worker count plays no part. The first ask answers.

## As built, against the spec

- **Serial inside a rayon worker** instead of the spec's debug assertion: the task gate showed a
  test (`ids_parent_baseline::record_fixture`) running the digest pipeline inside its own
  `par_iter`. No production caller was found doing so (spec 8.5 records it).
- **Not done:** the per-root allocation trace and the flow-cache lock-wait trace of spec 8.7. The
  pass's peak was measured directly at every worker count instead, which is the bound that matters,
  and the span's fall leaves no gap that contention would explain.

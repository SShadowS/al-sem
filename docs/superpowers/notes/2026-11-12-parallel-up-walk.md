# TLS-21: impact's up walk over several roots at once (measured, 2026-11-12)

**What changed.** The one-shot CLI (`alsem impact`, `--part up`) walks the up part's roots 8 at a
time, the same count and thread pool as TLS-22's literal pass (`AL_SEM_REACH_WORKERS`, 8 capped by
the cores). The resident engine (`alsem mcp`, `engine-host`) still walks one root at a time.
Output is byte-identical at any count. Spec: `docs/superpowers/specs/2026-11-12-parallel-up-walk.md`;
plan: `docs/superpowers/plans/2026-11-12-parallel-up-walk.md`.

Host: server1, 22 cores. Pinned CDO baseline (`/work/src/cdo-baseline/Cloud`), `release-fast`
binaries:
- the head: branch `tools/tls-21-parallel-up` at `b6536ae0`;
- master: `4f2459a6` plus step 0's trace-only spans, the build that minted the fixture golden.

One process at a time, in slots the orchestrator gave. Scripts and raw rows are in
`parallel-up-walk/` next to this note; their paths point at the session's scratchpad:
- `step0.sh` and `measure.sh`, the run scripts;
- `runs.jsonl`, the `timeit.py` records;
- `env.log`, the time, load and free memory before each run;
- `traces/`, every traced run (summarize with `scripts/trace_summary.py`; the counters are
  `impact.up.*`).

## 1. Step 0: where the up part's time went (master)

One traced `impact --diff` (the TLS-16 input, `bc3ccb18~5..bc3ccb18`, 699 roots), 13:43-13:46 UTC,
on a busy host (load 305 at the start), so the split is the point, not the times:

| Part of `job.0.impact-up` | Time | Share |
|---|---:|---:|
| per-root walks | 42.7 s | 93 % |
| per-root summaries | 3.0 s | 6 % |
| union walk + union summary | 0.09 s | 0.2 % |
| caption parses (39 files, inside the summaries) | 0.08 s | 0.2 % |
| the whole up part | 46.0 s | |

So the per-root walks were worth spreading. The union walk stays serial.

## 2. Composite `impact --diff` at 1, 2, 4, 8 and 16 workers (chunk a)

2026-11-12 15:28-16:10 UTC, no other CDO job, at least 35 GB free before each run. Each count was
run twice:
- **traced**, with `AL_SEM_UP_WALK_WINDOW=1`, for the spans and the walk window;
- **untraced**, with `timeit.py`, for wall, CPU and process peak.

Before every untraced run, `timeit.py` waited its full 180 s for an idle host. The load at the
start was 2.8-3.8 and the host 86-88 % idle, so each record is flagged "noisy" by its 90 % bar.
All 10 outputs are byte-identical to the 1-worker run.

| Workers | Up part (`job.0.impact-up`) | Literal pass (`prove.reach_effects`) | Wall | CPU | Process peak | Walk window: start → peak |
|---|---:|---:|---:|---:|---:|---:|
| 1 | 40.0 s | 91.4 s | 220.9 s | 601.7 s | 3,299.9 MiB | 2,506 → 2,573 MB (+67) |
| 2 | 20.5 s | 49.9 s | 176.6 s | 616.7 s | 3,338.1 MiB | 2,576 → 2,624 MB (+48) |
| 4 | 12.0 s | 36.8 s | 151.1 s | 640.3 s | 3,411.7 MiB | 2,621 → 2,663 MB (+42) |
| **8** | **8.4 s** | 32.6 s | **135.2 s** | 642.6 s | 3,484.8 MiB | 2,686 → 2,731 MB (+45) |
| 16 | 5.7 s | 32.0 s | 152.6 s | 741.2 s | 3,630.9 MiB | 2,729 → 2,775 MB (+46) |

- **The up part at 8 is 4.8 times faster: 40.0 s → 8.4 s.** The union walk is 0.08-0.13 s at
  every count. Each run parsed 9 caption files.
- **The walk's own memory is small.** The walk window, with the peak mark reset after the entry
  inventory is built, grows by 42-67 MB at any count. `scratchBytes` is 2.6 MB per scratch (one
  `seen` word per graph node), 20.7 MB at 8.
- **The whole-command numbers mix two changes.** `AL_SEM_REACH_WORKERS` also sets TLS-22's literal
  pass. The up part's own time is its span. The comparison with master at the same count is
  section 3.
- **8 stays the count.** At 16 the up part is faster, but the command is slower (152.6 s against
  135.2 s) and uses more CPU and memory.
- `timeit.py`'s peak is the largest single process's `ru_maxrss`.

## 3. Master against the head at 8 (chunk c)

The spec's memory bar compares the head at 8 with master at 8 on the same input. Master already
runs TLS-22's literal pass at 8. Master's composite `impact --diff`, untraced, ran at 16:50 UTC,
load 6.7 at the start:

| `impact --diff`, 8 workers | Wall | CPU | Process peak |
|---|---:|---:|---:|
| master `4f2459a6` | 173.5 s | 643.6 s | 3,438.0 MiB |
| head `b6536ae0` (section 2) | 135.2 s | 642.6 s | 3,484.8 MiB |

- **The whole answer is unchanged:** master's composite output is byte-identical to the head's,
  the 37 MB JSON of the full `impact --diff`.
- **The peak is +1.4 % over master**, under the spec's 5 % bar.
- The wall times come from separate runs at different loads (6.7 against 2.9), so they are not a
  clean comparison. The up part's own span is: 40.0 s → 8.4 s (section 2).

## 4. `--part up` at each count, and the byte comparison with master (chunk c)

`alsem impact --part up` over the same 699 roots, given as `--root-id`s and taken from the
1-worker composite run. The head ran at 1, 2, 4, 8 and 16 workers, traced and untraced. Master
ran once. 16:47-17:14 UTC.

The memory check before each run waits for 35 GiB. That held off the head's runs for the first
ten minutes (16:34-16:44), and they ran at 16:54.

**All 10 head outputs are byte-identical to master's** (`cost.walkMs`, `walksUs` and
`scratchBytes` blanked). This is the outside reference on real data: the parallel and the serial
path share the new code, and master's output does not.

| `--part up` | Up part (span) | Wall (untraced) | CPU | Process peak | Walk window: start → peak |
|---|---:|---:|---:|---:|---:|
| master | 43.4 s | 70.8 s | 162.0 s | 2,532.2 MiB | (no window on master) |
| head, 1 | 36.3 s | 46.7 s | 139.0 s | 2,538.3 MiB | 2,410 → 2,476 MB (+66) |
| head, 2 | 23.3 s | 47.9 s (load 15.4) | 176.6 s | 2,573.0 MiB | 2,437 → 2,526 MB (+89) |
| head, 4 | 11.9 s | 22.4 s | 145.9 s | 2,560.0 MiB | 2,432 → 2,536 MB (+104) |
| **head, 8** | **7.6 s** | **19.6 s** | 164.3 s | 2,569.9 MiB | 2,411 → 2,543 MB (+132) |
| head, 16 | 5.7 s | 17.3 s | 183.2 s | 2,604.8 MiB | 2,401 → 2,578 MB (+177) |

- **Process peak at 8: +1.5 % over master** (2,569.9 against 2,532.2 MiB). Here the walk is the
  only thing that changes with the count (`--part up` runs no literal pass). The walk window
  grows with the count, from +66 MB at 1 to +132 MB at 8: per walk in flight, a scratch (2.6 MB)
  and a visitor and summary.
- **Noisy rows:** the head's 2-worker untraced run started at load 15.4 (56 % idle after
  `timeit.py`'s wait), so its wall time is high. Master's single run started at load 5.8. The
  spans are the cleaner numbers: 43.4 / 36.3 s for master / head at 1 is the spread between two
  runs of the same serial work.

## 5. `impact --base` at 1 and 8 (chunk b)

`impact --base bc3ccb18~5 --no-tests --format json`, each run with a fresh base cache, traced and
untraced. 17:44-18:09 UTC, load 2.5-3.0 at the untraced starts. The plan was five counts; it was
cut to two for time, and the orchestrator agreed (2026-11-12).

| Workers | Up part (span) | Literal pass (span) | Wall | CPU | Process peak | Walk window: start → peak |
|---|---:|---:|---:|---:|---:|---:|
| 1 | 44.2 s | 162.3 s | 334.1 s | 792.3 s | 3,690.3 MiB | 2,781 → 2,849 MB (+68) |
| 8 | 7.4 s | 55.3 s | 204.8 s | 837.4 s | 3,963.6 MiB | 2,860 → 2,899 MB (+39) |

- **The up part at 8 is 6 times faster: 44.2 s → 7.4 s.**
- **The outputs are the same answer.** They differ only in two fields of every `--base` run: the
  base cache's folder (each run got a fresh temporary one) and the base build's millisecond
  timings. The two 1-worker runs differ from each other in exactly the same two fields.
- The process peak at 8 against 1 mixes in the literal pass (TLS-22), as in section 2. No master
  `--base` run was made. The comparison with master is the `--diff` one (section 3).

## 6. The resident engine (`cdo-speed`, chunk d)

The resident engine walks one root at a time, but it runs the reorganised code: the task
function, the shared captions with their lock, the scratch stash. Check: `scripts/cdo-speed
<CDO> --base-exe <master> --head-exe <head> --rounds 5`, 18:09-18:14 UTC, load 7.7 at the start
and 5.3 at the end. Raw: `parallel-up-walk/speed.json`, `speed.log`.

**PASS.**

| Row | CPU ratio, head/master | Wall ratio | Per-round spread |
|---|---:|---:|---:|
| round total (median) | 0.990 | 0.996 | 1.07 |
| impact (the up part runs here) | 0.932 | 0.929 | 1.56 |
| prove / explain / digest / query / tests | 0.99 / 1.20 / 1.02 / 0.94 / 1.10 | | 1.10-2.17 |
| warm-up: model build | 1.256 | 1.423 | one sample |
| warm-up: test universe | 0.962 | 1.018 | one sample |
| peak memory (VmHWM summed) | 10,186 → 10,065 MiB (×0.988) | | |

- The model build's ×1.256 CPU and ×1.42 wall are within the gate's 1.3 / 1.6 limits.
- They are noise, for two reasons. TLS-09's same-binary check on this host gave ×1.18 CPU and
  ×1.28 wall for the build (one sample each), and this change does not touch the model build.
- The per-call ratios scatter as TLS-09's same-binary check showed: about ±10 % per round, and
  more for the short calls.

## 7. Verdict

The change stays. Against the spec's keep-or-drop list (section 5):
- **Faster:** the up part at 8 is 4.8-6 times faster: 40.0 → 8.4 s (`--diff`), 44.2 → 7.4 s
  (`--base`), 36.3 → 7.6 s (`--part up`).
- **Walk window:** +39 to +132 MB at 8 workers, depending on the run.
- **Peak against master:** +1.4 % (`impact --diff`) and +1.5 % (`--part up`) at 8, under the 5 %
  bar.
- **Same answer:**
  - every head output equals master's: `--part up` at every count, and the whole `impact --diff`
    at 8;
  - `--base` gives the same answer at 1 and 8.
- **The resident engine:** `cdo-speed` passes at about 1.00, memory ×0.988.

The count stays 8, shared with the literal pass. At 16 the command is slower and heavier
(section 2), so there is no case for a second setting.

# The capability-cone walk runs SCCs in parallel: CDO measurement (TLS-29)

Spec: `docs/superpowers/specs/2026-11-13-parallel-cone-walk.md` (panel READY after three rounds;
code review READY after two). Branch `tools/tls-29-cone-walk-r2`.

## The census (before)

`ALSEM_CONES_CENSUS=1` on CDO (`analyze --scope all`, release-fast, two runs agree):
33,227 SCCs in 81 levels, the widest 16,007; the SCCs' times sum to 4,951-5,292 ms, the critical
path is 381-406 ms, the slowest SCC (704 members) 122-129 ms. `fact_cone_for_scc` is 55% of it,
the closed-set memo rows 22%. The raw half's scan is 0 in analyze, so the backlog item "The cone
singleton walk" (`docs/OUTSTANDING.md`) does not apply to it and was not folded in.

## The schedule (simulated)

From the walk's dump (`ALSEM_CONES_SCHEDULE_FILE`), a ready queue taking the lowest ready index:

| workers | walk | peak live fact-cone entries |
|---:|---:|---:|
| 1 | 5,292 ms | 1,392,716 |
| 8 | 742 ms | 1,436,755 (+3.2%) |

A window was worse at 8 workers (W = 2,048: 838 ms, +4.2%); level by level was +124% entries.
**Chosen: no window, 8 workers** (`default_reach_workers()`, `REACH_WORKERS` capped by the
cores), approved by the orchestrator on the condition that the measured peak stays within a few
percent of master.

## Measured (2026-11-13)

Setup:
- binaries: master `60981a1f` (cdo-speed's release-fast build) against head `d884b287`
  (release-fast);
- workspace: the pinned CDO baseline `/work/src/cdo-baseline/Cloud`;
- each run starts at MemAvailable ≥ 35 GiB and 1-minute load < 10, under `timeit.py`;
- the host had other projects' load (3-10 between runs).

`analyze --scope all --format json --deterministic`, three runs each, interleaved:

| | master | head |
|---|---|---|
| wall s (runs; median) | 26.49 / 25.11 / 22.72; **25.11** | 23.23 / 19.71 / 21.17; **21.17** |
| CPU s (median) | 123.5 | 129.6 |
| peak RSS MiB (median) | 3,229.6 (3,208-3,242) | 3,223.3 (3,212-3,226) |
| `cone.walk` span (one traced run) | 7,236 ms | 1,153 ms |

- **Walk:** 6.3× faster; analyze's median wall is 3.9 s shorter (−16%).
- **CPU:** +5%. That is the cost of the 8 threads (wakeups, the per-task maps, the pre-intern
  pass). It is within the run-to-run spread of 120.7-131.9 s.
- **Peak:** no rise; the head's runs fall inside master's range. So the condition holds and no
  fallback is needed.
- **Output:** all eight outputs are byte-identical (51 MB): the six untraced and the two traced.

`digest --diff` and `impact --diff` (the TLS-28 range), once each. Their walk stays serial
(`raw = Subjects(roots)`), so this is a no-regression check:

| | wall s | CPU s | peak MiB | output |
|---|---:|---:|---:|---|
| digest master / head | 70.9 / 74.6 | 538.5 / 554.8 | 6,349 / 6,317 | byte-identical (17 MB) |
| impact master / head | 131.0 / 132.9 | 554.3 / 562.2 | 3,527 / 3,546 | byte-identical (38 MB) |

Single runs; both differences are within noise.

## Follow-up (not in this task)

`digest`, `impact` and `prove` keep the serial walk because they ask for raw facts for their
roots. A parallel raw half needs a task-local raw builder per SCC, re-interned at the merge in
SCC order. impact's main part is cdo-gate's long pole, so this is the next candidate. Proposed
to the orchestrator as a task.

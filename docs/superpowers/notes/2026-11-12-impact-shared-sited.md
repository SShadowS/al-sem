# One-shot impact keeps the sited graph between its parts: CDO measurement (TLS-28)

Spec: `docs/superpowers/specs/2026-11-12-impact-shared-sited.md`. Branch
`tools/tls-28-impact-sited`.

## Setup

- Workspace: the pinned CDO baseline, `/work/src/cdo-baseline/Cloud` (`bc3ccb18`).
- Binaries (all `release-fast`):
  - **master**: TLS-27's head, no source difference from `26526e7c`;
  - **step1**: the branch with step 2 turned off by a temporary edit (reverted);
  - **head**: the branch, steps 1 and 2 (`ff733dc5`).
- Shapes are the ones `cdo-gate`'s impact phase runs:
  - `--diff` of `bc3ccb18~5..bc3ccb18` (histogram, JSON);
  - `--base bc3ccb18~5 --no-tests` (JSON, a fresh base cache each run, so a miss, as in the gate);
  - `--routine 'CDO Telemetry::UpdateLastUpload'` (JSON).
- Each run starts at MemAvailable ≥ 35 GiB and 1-minute load < 10, under
  `docs/superpowers/notes/crate-split-phase0/timeit.py` (wall, CPU, peak RSS).
- Other projects loaded the host the whole time (load 3-11 between runs, much higher in the
  first pass). **Walls are not usable; CPU is a guide; one run per cell.**

## Results (2026-11-13, second pass, binaries interleaved per shape)

| shape | binary | peak MiB | CPU s | wall s |
|---|---|---:|---:|---:|
| `--diff` | master | 3,552.8 | 678.6 | 146.2 |
| `--diff` | step1 | 3,512.5 | 664.5 | 147.2 |
| `--diff` | head | 3,554.2 | 560.6 | 135.2 |
| `--base` | master | 3,987.3 | 862.9 | 212.8 |
| `--base` | step1 | 3,952.6 | 862.0 | 215.1 |
| `--base` | head | 3,780.4 | 776.5 | 207.2 |
| `--routine` | master | 3,239.1 | 346.1 | 51.4 |
| `--routine` | head | 3,290.6 | 241.9 | 41.4 |

First pass (2026-11-12, heavier outside load, master and head only): `--diff` peak 3,320.8 →
3,533.8 MiB, CPU 767 → 715 s; `--base` peak 3,753.0 → 3,603.5 MiB, CPU 1,077 → 946 s.

**Peak memory.** master's own `--diff` peak moved 3,321 → 3,553 MiB between the two passes (7%),
so peak noise across runs is several percent. Within one pass, head against master is +0.0%
(`--diff`), −5.2% (`--base`) and +1.6% (`--routine`); step 2 against step 1 is +1.2% (`--diff`)
and −4.4% (`--base`). Step 2's cost is within the noise and at most about 1%: **step 2 stays**
(the spec's rule: drop it only above a few percent).

**CPU.** head against master: −17% (`--diff`), −10% (`--base`), −30% (`--routine`). step1 is
master's CPU within 2% on CDO; the saving there is step 2's (see below).

**Output.** Every `--diff` output (master, step1, head, traced and untraced) is byte-identical
(38 MB). `--routine` is byte-identical. `--base` differs only in the base cache folder path
(`dir`, a fresh temporary folder per run) and the base build's own timings.

## Program builds (traced `--diff`, `ALSEM_TRACE=1`)

- master: 3 `crossapp.program_build_full` spans (main, up, tests-select).
- head: 2 (main, tests-select). The up part ran on the graph the main part kept; its
  `batch.sited_cache.stamp` span took 3 ms (the other two stamps 3-4 ms).

The tests-select part does not hit on CDO, and that is correct: CDO's tests live in the sibling
test app, so the select part runs on the test universe `/work/src/cdo-baseline/Test`, a different
program (a probe printed its `ViewKey` workspace and a different manifest stamp). Step 1's up →
tests-select reuse applies only when the test universe is the workspace itself. On CDO all of the
saving comes from step 2 (main → up), which is why step1 alone measures as master.

## Not measured here

- The `cdo-gate` impact phase time (414 / 466 / 421 / 395 s on master in the 2026-11-12 gates):
  it comes from the landing gate.

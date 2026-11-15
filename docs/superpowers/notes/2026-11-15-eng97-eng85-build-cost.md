# ENG-97: ENG-85's cost on the CDO model build, measured alone

**Verdict: ENG-85 does not slow the model build. The landing's x1.232 was cdo-speed's order
effect: the side that builds second costs about x1.28 CPU, whichever side it is.** Measured with
cdo-speed itself (the `build` call: the first ask through `alsem mcp` and the engine child,
`release-fast`), six passes, three with each side first. Comparing like with like, ENG-85 is
level or faster: CPU x0.975 when built first and x0.969 when built second, wall x0.986 and
x0.881. The server's peak memory falls slightly, x0.976 to x0.998. Nothing to fix in ENG-85; the
order effect is TLS-79's.

## Why it was asked

ENG-85 (shared row strings, `SharedStr`) landed in `e708f1f6`. The landing's speed check passed,
but its `build` read CPU x1.232 and wall x1.495, the two warm-up passes disagreeing (x1.565 with
base built first, x0.970 with head first; `cdo-gate-logs/20261115-193457-444307/speed.log`). In
review, Fable had named a risk: many threads cloning the same `Arc<str>` contend on its count.

## The measurement: cdo-speed, three runs

- **Sides:** base `801cc6af` (master before ENG-85; it is ENG-85's merge base, and the landing's
  other branches changed only a test file under `src/`), head `96d49041` (ENG-85's head). Both
  `release-fast`, from cdo-speed's commit cache.
- **Command** (`tools/census-probe/runs-eng97/speed-runs.sh`): `scripts/cdo-speed
  /work/src/cdo-baseline/Cloud --base 801cc6af --head 96d49041 --rounds 1`, three times, each inside
  `scripts/cdo-slot run`, each started once the host's `some avg60` was at most 15% (2.1-4.2%).
  Each run's warm-up times `build` twice on fresh server pairs, base first, then head first
  (TLS-65), with cdo-speed's own per-pass admission (TLS-76). Tools paused its builds throughout.
- **Every pass was calm:** container stall share 3.1-6.2% per warm-up, load 9.0-19.0. All three
  runs PASS. Logs and `speed.json`: `tools/census-probe/runs-eng97/cdo-speed-run{1,2,3}.{log,json}`.

CPU seconds (user + system of the server and its engine children) and wall seconds of `build`:

| Run | Pass | Built first | Base CPU | Head CPU | Base wall | Head wall |
|---:|---:|---|---:|---:|---:|---:|
| 1 | 1 | base | 107.39 | 142.96 | 41.01 | 52.08 |
| 1 | 2 | head | 138.44 | 87.59 | 42.32 | 27.42 |
| 2 | 1 | base | 93.98 | 109.51 | 32.78 | 36.16 |
| 2 | 2 | head | 126.80 | 95.22 | 52.44 | 32.38 |
| 3 | 1 | base | 97.05 | 119.94 | 33.45 | 41.57 |
| 3 | 2 | head | 118.92 | 108.11 | 52.61 | 45.89 |

**The order effect.** The side built second is slower in all six passes: second over first,
CPU x1.100 to x1.581 (geometric mean x1.282), wall x1.103 to x1.620 (x1.307). The per-pass
head-over-base ratios therefore swing with the order: CPU x1.331, x1.165 and x1.236 when head
builds second; x0.633, x0.751 and x0.909 when it builds first. The landing's two passes were one
of each (x1.565, x0.970), and x1.232 is their geometric mean.

**ENG-85, position for position** (means over three passes each):

| Built | Base CPU | Head CPU | Head/base | Base wall | Head wall | Head/base |
|---|---:|---:|---:|---:|---:|---:|
| first | 99.5 | 97.0 | x0.975 | 35.7 | 35.2 | x0.986 |
| second | 128.1 | 124.1 | x0.969 | 49.1 | 43.3 | x0.881 |

Geometric mean of all six per-pass ratios, which cancels the order when it is the same factor
both ways: CPU x0.969, wall x0.920. cdo-speed's own per-run verdicts: x0.918, x0.935, x1.060 CPU.
Peak memory (every server process's VmHWM, summed): x0.998, x0.988, x0.976.

**What this does and does not show.** Three samples per position, with a same-position spread of
about ±10% (base built first: 94.0-107.4 s), can bound ENG-85's effect but not resolve a few
percent: the data rule out a slowdown near the landing's x1.23 and are level within about ±10%.
The reference-count risk would show as CPU; the build's CPU is level, so no cost is visible in the
build. Clones at query time (prove, explain, the detectors) are outside `build` (cdo-speed's
rounds cover them; the landing's rounds read x1.023).

## A first, narrower probe (kept for the record)

Before the cdo-speed runs, a scratch probe timed only `build_target`, the engine child's
model build, six alternating passes (`measure.sh`, `runs.tsv`, `probe-main.rs.txt`, raw logs in
`probe-time/`). Its result: CPU x1.006 (x0.972-x1.028), wall x1.011, max RSS x0.933. A Fable
measurement audit found it too narrow to carry the verdict, and the points stand:

- It covered about 55 s of CPU and 11-12 s of wall, half the CPU and a third of the wall of the
  `build` call above, and ENG-85 changed code outside it (explain, the cone, the solver).
- It was built as cargo's default release (its own workspace: no LTO, 16 codegen units) with plain
  `MiMalloc`, not the repo's `release-fast` and the server's `ReleasingMiMalloc`.
- Its first pass ran cold (the only major page faults). Without it the head was slightly
  slower in all five remaining passes: CPU x1.013, wall x1.043. Its order effect ran the other way
  from cdo-speed's: in that one process, the side run SECOND was the faster.
- Its max RSS (x0.933, about -243 MiB) is the build-only process; the server's peak barely moves.

The cdo-speed runs supersede it: they time the call the landing timed, in its binaries.

## Leads for TLS-79 (the order effect)

The second build of a pass runs while the first side's server is still alive and holds its model
(cdo-speed keeps both servers of a pair up), so the second build shares the machine's memory
bandwidth and cache with ~8 GB of resident model and that server's own threads. A one-process
probe showed no such penalty. That is a guess to test, not a finding.

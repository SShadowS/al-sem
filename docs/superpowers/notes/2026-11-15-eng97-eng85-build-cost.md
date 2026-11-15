# ENG-97: ENG-85's cost on the CDO model build, measured alone

**Verdict: no slowdown from ENG-85 was found; nothing to fix on this evidence.** Three runs of
cdo-speed itself (the `build` call: the first ask through `alsem mcp` and the engine child,
`release-fast`) read ENG-85 at CPU x0.969, with a resolution of only about ±20% (95% interval
x0.80-x1.18). A narrower but more precise probe bounds `build_target` alone at about +3% CPU. The
landing's x1.232 was **not reproduced** (the three runs: x0.918, x0.935, x1.060) and stays
unexplained: in that sample the head was slower in both positions.

Separately, cdo-speed has a strong **order effect**: the side that builds second costs about x1.28
CPU (x1.10-x1.58 between passes). It explains why the landing's two passes disagreed (x1.565
against x0.970), not its x1.232. It belongs to TLS-79.

## Why it was asked

ENG-85 (shared row strings, `SharedStr`) landed in `e708f1f6`. The landing's speed check passed,
but its `build` read CPU x1.232 and wall x1.495, its two warm-up passes disagreeing (x1.565 with
base built first, x0.970 with head first; `cdo-gate-logs/20261115-193457-444307/speed.log`). In
review, Fable had named a risk: many threads cloning the same `Arc<str>` contend on its count.

## The measurement: cdo-speed, three runs

- **Sides:** base `801cc6af` (master before ENG-85; it is ENG-85's merge base, and the landing's
  other branches changed only a test file under `src/`), head `96d49041` (ENG-85's head). Both
  `release-fast`, from cdo-speed's commit cache.
- **Command** (`tools/census-probe/runs-eng97/speed-runs.sh`; its logs are committed as
  `cdo-speed-run{1,2,3}.{log,json}`): `scripts/cdo-speed /work/src/cdo-baseline/Cloud --base
  801cc6af --head 96d49041 --rounds 1 --max-rounds 1 --no-build`, three times, each inside
  `scripts/cdo-slot run`, each started once the host's `some avg60` was at most 15% (the script's
  check; the readings were not saved). Each run's warm-up times `build` twice on fresh server
  pairs, base first, then head first (TLS-65), with cdo-speed's own per-pass admission (TLS-76).
  The tools lane said it kept its builds off during the runs; other projects still loaded the
  machine (load 9-26).
- **Every pass was calm by cdo-speed's measure:** container stall share 3.1-6.2% per warm-up, none
  admitted by timeout, `under_load` false. All three runs PASS.

CPU seconds (user + system of the server and its engine children) and wall seconds of `build`:

| Run | Pass | Built first | Base CPU | Head CPU | Base wall | Head wall |
|---:|---:|---|---:|---:|---:|---:|
| 1 | 1 | base | 107.39 | 142.96 | 41.01 | 52.08 |
| 1 | 2 | head | 138.44 | 87.59 | 42.32 | 27.42 |
| 2 | 1 | base | 93.98 | 109.51 | 32.78 | 36.16 |
| 2 | 2 | head | 126.80 | 95.22 | 52.44 | 32.38 |
| 3 | 1 | base | 97.05 | 119.94 | 33.45 | 41.57 |
| 3 | 2 | head | 118.92 | 108.11 | 52.61 | 45.89 |

### The order effect

The side built second is slower in all six passes: second over first, CPU x1.100-x1.581
(geometric mean x1.282), wall x1.103-x1.620 (x1.307). It is the same size in the landing (head
x1.277, base x1.263) and far above the "up to ~15%" in cdo-speed's own comment. It is not steady:
per run, head's factor was x1.63, x1.15, x1.11 and base's x1.29, x1.35, x1.23. So the per-pass
head-over-base ratios swing with the order (x1.331, x1.165, x1.236 with head second; x0.633,
x0.751, x0.909 with head first), and even one pair's geometric mean is noisy.

### ENG-85

The geometric mean of a pass pair cancels the order when the factor is the same both ways, which
holds on average here but not per pass. Per run: x0.918, x0.935, x1.060 CPU; over all six passes
x0.969 CPU, x0.920 wall. Split by position (each row compares different passes, so it carries any
drift between them; these are the same six samples seen another way, not more evidence):

| Built | Base CPU | Head CPU | Head/base | Base wall | Head wall | Head/base |
|---|---:|---:|---:|---:|---:|---:|
| first | 99.5 | 97.0 | x0.975 | 35.7 | 35.2 | x0.986 |
| second | 128.1 | 124.1 | x0.969 | 49.1 | 43.3 | x0.881 |

**Resolution.** Three runs give a 95% interval (t, 2 degrees of freedom, on the per-run geometric
means) of CPU x0.80-x1.18 and wall x0.69-x1.23. They cannot resolve a change under about ±20%, so
they do not show ENG-85 level or faster, only that no slowdown appeared and that the landing's
x1.232 did not recur in CPU. The tighter bound comes from the probe below: `build_target` alone
at about +3% CPU at most. The reference-count risk would show as CPU; no such cost is visible at
either precision.

**What is not covered.** Clones at query time (prove, explain, the detectors) are outside
`build`. These runs' single rounds read x0.989, x1.018 and x1.115 CPU (run 3's prove x1.114, with
the load rising to 20); one sample each, not enough to judge. The landing's three rounds read
x1.023.

**Peak memory** (every server process's VmHWM, summed): x0.998, x0.988, x0.976 (-16, -98,
-193 MiB); no noise band is known for this figure.

### The landing's x1.232

In the landing, position for position, head was slower in both: x1.225 built first (117.30 s
against 95.76 s), x1.239 built second (149.84 against 120.94). The order effect cancelled there
and head still read about 23% slower. That sample is outside all three runs here and is
unexplained; the single-pair noise above is the likeliest reading, not a proven one.

## A first, narrower probe

Before the cdo-speed runs, a scratch probe timed only `build_target`, the engine child's model
build, six alternating passes (`measure.sh`, `runs.tsv`, `probe-main.rs.txt`, raw logs in
`probe-time/`): CPU x1.006 (x0.972-x1.028), wall x1.011, max RSS x0.933. A Fable measurement audit
found it too narrow to carry the verdict:

- It covered about 55 s of CPU and 11-12 s of wall, half the CPU and a third of the wall of the
  `build` call, and ENG-85 changed code outside it (explain, the cone, the solver).
- It was built as cargo's default release (its own workspace: no LTO, 16 codegen units) with plain
  `MiMalloc`, not the repo's `release-fast` and the server's `ReleasingMiMalloc`.
- Its first pass ran cold (the only major page faults). Without it the head was slightly slower in
  all five remaining passes, CPU x1.013 and wall x1.043, which bounds `build_target`'s CPU cost at
  about +3%. Once the cold pass is dropped it showed no order effect (one process, nothing else
  resident).
- Its max RSS (x0.933, about -243 MiB) is the build-only process's; the server's peak moves less.

## A lead for TLS-79 (the order effect)

cdo-speed creates both servers of a pair before a pass and closes them after it, and makes the
`build` call on one side, then the other: the first side's server is alive, with its model, during
the second build. Its size at that moment is not measured (CLAUDE.md gives ~4.7 GB after one ask of
each kind). Idle resident memory costs no bandwidth; the thing to test is whether that server is
still doing work (background preparation, the allocator returning memory) during the second build.

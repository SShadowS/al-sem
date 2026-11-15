# ENG-97: ENG-85's cost on the CDO model build, measured alone

**Verdict: ENG-85 does not slow the model build.** Over six passes, head over base: CPU x1.006
(geometric mean; range x0.972 to x1.028), wall x1.011, peak memory x0.933 (-6.7%). Nothing to fix.

## Why it was asked

ENG-85 (shared row strings, `SharedStr`) landed in `e708f1f6`. The landing's speed check passed,
but its model-build sample read CPU x1.232 and wall x1.495, and its two warm-up passes disagreed:
x1.565 with base built first, x0.970 with head first. Fable had named a risk in review: many
threads cloning the same `Arc<str>` contend on its reference count.

## How it was measured

- **Sides:** base `801cc6af` (master before ENG-85) and head `96d49041` (ENG-85's submitted head;
  `801cc6af` is its merge base, so the diff is ENG-85 alone).
- **What runs:** one resident model build of the pinned CDO baseline (`/work/src/cdo-baseline/Cloud`),
  the engine child's path: `WorkspaceInputs::read`, `WorkspaceSession::with_mode(Default)`,
  `build_target`. The same scratch probe as ENG-84's profile, built with `cargo build --release`
  against each revision (same profile, same allocator, mimalloc), timed under `/usr/bin/time -v`.
  CPU is user + system of the process.
- **Order:** six passes, each timing both sides back to back, the order flipped every pass
  (three base-first, three head-first).
- **Admission (cdo-speed's, TLS-76):** before each pass, outside the CDO slot, the host's
  `/proc/pressure/cpu` `some avg60` had to be at most 15% (wait up to 600 s, else run labelled).
  Each run records its container stall share (`/sys/fs/cgroup/cpu.pressure` `some total` delta
  over its wall time). Every pass was admitted at once (host 5.4-10.7%), and every run was calm
  (stall share 3.2-7.7%). Each pass held the CDO slot for its two runs; tools paused its builds.
- Script, raw table and the probe's source: `tools/census-probe/runs-eng97/` (`measure.sh`,
  `runs.tsv`, `probe-main.rs.txt`; its Cargo.toml names `al-sem` by path, `mimalloc = "0.1"`,
  `[profile.release] debug = "line-tables-only"`).

## Results (head over base)

| Pass | Order | CPU | Wall | Peak RSS |
|---:|---|---:|---:|---:|
| 1 | base-first | x0.972 | x0.867 | x0.938 |
| 2 | head-first | x1.004 | x1.013 | x0.922 |
| 3 | base-first | x1.012 | x1.022 | x0.931 |
| 4 | head-first | x1.028 | x1.032 | x0.938 |
| 5 | base-first | x1.002 | x1.034 | x0.933 |
| 6 | head-first | x1.018 | x1.117 | x0.937 |

| | Geomean | Median | Base-first | Head-first |
|---|---:|---:|---:|---:|
| CPU | x1.006 | x1.008 | x0.995 | x1.017 |
| Wall | x1.011 | x1.027 | x0.971 | x1.053 |
| Peak RSS | x0.933 | x0.935 | x0.934 | x0.932 |

Medians in seconds of CPU: base 54.8, head 55.7 (one build: ~55 s of CPU in ~11-12 s of wall).
The side run second is a little slower in wall time in both orders (base-first wall x0.971,
head-first x1.053), the same order effect TLS-65 named; CPU barely moves with order.

## What this says about the landing's x1.232

It was not ENG-85. The landing's sample came from one pass per order on a loaded machine, and
the passes disagreed by x1.6; under calm admission and six passes the change sits inside
x0.97-x1.03 in CPU. The refcount-contention risk does not show: it would cost CPU (cache-line
traffic is CPU time), and CPU did not move. Peak memory falls 6.7% on this path, in line with
ENG-85's census (-199.4 MiB).

**Limits.** The probe is the engine child's build alone, not the whole `alsem mcp` warm-up
(server start-up and the first ask's other work are not in it). `release` here, where cdo-speed
builds `release-fast`; both sides share the profile, so the ratio holds, not the absolute seconds.

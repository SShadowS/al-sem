# TLS-83: what a cdo-speed build ratio resolves (from 75 past runs)

ENG-94's head against base 52d349ab gave a build CPU ratio of x0.871 in the lane's run and
x0.989 in the landing gate's run. TLS-79 had stated "±8 %". This note asks whether the spread
lies between runs (host state) or between passes, using the cdo-speed results already on disk.

## Data

- Every `speed.json` under /work written after TLS-79 landed (8ec827a0, 2026-11-16 00:58):
  **75 runs**, each with two warm-up passes. In pass 1 base builds first; in pass 2 head does.
- From each pass: the model build's CPU seconds per side. The ratio is head over base, and a
  run's reading is the geometric mean of its two pass ratios. These are the numbers the gate
  judges.
- 19 base/head pairs were measured twice, minutes apart (the lane's run and the landing's
  run, or a re-run).
- Tables: `/work/.al-sem/profiles/2026-11-18-tls83/runs.tsv`. Each row holds the time, base,
  head, the pass ratios, the reading, the base CPU per pass and the file. `order.txt` holds the
  per-side build CPU by position.

## Results

| measure | median | 90th pct | max |
|---|---:|---:|---:|
| pass 1 vs pass 2 of one run (abs. log difference) | 4.3 % | 14.6 % | — |
| run vs run, same pair (abs. log difference of readings) | 3.1 % | 9.6 % | 11.6 % (12.3 % as a plain ratio) |

- **Per pass:** sd(log ratio) over all 150 passes is 0.060. The two-pass reading then has
  sd 0.042, a 95 % band of **±8 %**. That agrees with TLS-79's figure for ONE reading.
- **Two readings of one pair:** their difference has sd 0.042 × √2 = 0.059, a 95 % band of
  **±12 %**. The 19 repeated pairs fit this: median 3.1 %, max 12.3 %; their rms gap is
  0.052, a little under the predicted 0.059, so ±12 % is on the safe side. A run's two
  passes are uncorrelated (r = -0.07), so this is pass noise, not host state.
- **ENG-94** (x0.871 against x0.989, 12.7 %) is a ~2.2σ draw of that difference. This is an
  inference from the other runs: the lane's x0.871 run is not among the 75 files (only the
  landing's x0.989 is). No host-state effect beyond the pass noise is needed to explain it.
- **The base build's CPU** moves much more between runs (70-103 s) than the ratio does. Host
  state shifts both sides of a run together, and the within-run ratio cancels most of it.
- **Pass 1 costs more on both sides.** Median CPU by position: pass 1 first 81.8 s, pass 1
  second 79.8 s, pass 2 first 78.3 s, pass 2 second 77.1 s. By means, base costs 4.0 % ± 1.0
  more in pass 1 than in pass 2, and head 2.2 % ± 0.9 (review r1, Fable; the first draft read
  the medians alone as 3.6 % on base only). A cost common to pass 1 cancels in the ratio. The
  part specific to the run's very first build is about 1.7 % ± 1.4, not significant. The mean
  reading over all 75 runs is x0.996, a bias of -0.44 % ± 0.47: no measurable bias.

## What follows

1. **The gate's verdict should state the resolution of what it measured.** One two-pass build
   reading resolves ±8 % (95 %). Comparing two readings resolves ±12 %. A reading inside ±8 %
   of 1.0 is "no measurable change", not a gain or a loss.
2. **A CHANGELOG quotes a build ratio** only when two independent runs agree within ±12 %, and
   then quotes their combined (four-pass) value with its ±6 %. (Memory rule since ENG-94;
   proposed for CLAUDE.md and the cdo-measure skill.)
3. **Ways to resolve more**, to be measured before one is built:
   - (a) More passes. Four passes give sd 0.030, ±6 %; eight give ±4 %. Each pass costs about
     +1.5 min.
   - (b) An untimed first build at the start of the run, to take the first-build cost off
     pass 1 (≈ +30 s).
   - (c) Count the server's user-mode instructions over the build call (`perf stat`, attached
     to the server's PID) next to its CPU seconds. Pinned instruction counts resolved about
     0.1 % where CPU seconds swing about ±10 % (ENG-121, 2026-11-17). Whether that holds for
     a multi-threaded, unpinned build (spin-waits, work stealing) is the open question, and
     it needs a run of master against itself.
4. **Runs this needs** (CDO slot, timed): 6 runs of master against itself, plus 4 of the
   ENG-94 pair (52d349ab against its head). They restate the resolution from fresh data and
   test (b) and (c). That is about 50 minutes of slot time. The orchestrator split it: (c)
   first, in one block.

## Block 1 (2026-11-18 05:30-05:47 UTC): instructions against CPU seconds

**Setup.** 3 cdo-speed runs on CDO of one release-fast binary (11-16) against a copy of itself,
`--rounds 2`, engine lane paused, host CPU pressure about 1 %. Every timed call also counted
user-mode instructions: `perf stat --no-scale -e cpu_core/instructions/u -e
cpu_atom/instructions/u -p <server and its live children>`, attached for the call and summed.
Attaching counts threads and child processes started later; this was checked first on a
script that starts threads and a child after the attach (it matched a direct count, minus
Python's start-up). Raw: `/work/.al-sem/profiles/2026-11-18-tls83/block1-run{1,2,3}/speed.json`
(`instr` per record).

The table gives sd(log head/base) per call over the 3 runs × passes or rounds; the true ratio
is 1:

| call | n | CPU seconds | user instructions |
|---|---:|---:|---:|
| build (warm-up) | 6 | 0.084 | 0.0031 |
| prove | 6 | 0.039 | 0.0015 |
| query | 6 | 0.252 | 0.0013 |
| explain | 6 | 0.085 | 0.0003 |
| impact | 6 | 0.120 | 0.0003 |
| warm-tests | 6 | 0.050 | 0.0001 |
| warm-prove / -query / -impact / -explain | 5-6 | 0.15-0.27 | 0.0002-0.0004 |

- **Instructions resolve far better than CPU seconds**, unpinned and multi-threaded alike.
  - For the rows the gate judges, the factor is about 27 for the build and about 390 for
    warm-tests.
  - Up to about 900 for sub-second calls. Their CPU spread is mostly the 10 ms clock tick, so
    that figure flatters.
- **The build's six instruction ratios** were 0.99862, 0.99988, 0.99969, 0.99216, 1.00013 and
  1.00011.
  - Typically within ±0.1 %. One base build read 372.98e9 against about 370.05e9, a 0.8 %
    jump: one in 12 builds.
  - Without it, sd is about 0.0006; with n = 6, sd is known only to a factor of about 2.
  - If the jump recurs, it is real extra work in the build, not counting noise.
  - Either way it is far inside the x1.02 advisory limit, against ±8 % for a two-pass CPU
    reading.
- **What instructions do not see:** memory stalls, cache misses and lost parallelism. Those
  change time without changing the count of instructions retired. A change that only makes
  memory access worse would read x1.000. CPU and wall time stay needed for that, but with
  limits that only catch large changes.

## What was built (TLS-83, decided with the orchestrator, 2026-11-18)

- cdo-speed counts instructions by default and reports the ratio on every verdict line, and
  on its PASS/FAIL line for the model build and the round total. cdo-gate's summary quotes
  that line.
- CPU and wall limits are unchanged.
- The instruction limit, x1.02, is ADVISORY: a ratio over it prints `ADVISORY (not failing)`
  and the run still passes.
- Without perf the run says `instructions: NOT counted` and judges CPU and wall, as before.
- (a) more passes and (b) an untimed first build were dropped: instructions resolve what they
  were meant to.
- A CHANGELOG quotes an instruction ratio, which resolves, as work ("instructions x0.93"),
  never as "N % faster": instructions miss stalls and parallelism, so a change that trades
  instructions for stalls reads as a gain. It quotes a CPU ratio only when two independent
  runs agree within ±12 %, and then their four-pass value with ±6 %.

## Open: when should the instruction limit fail? (for the owner, with data)

At x1.02 a branch that legitimately adds 2-3 % work (a new fact, a new check) would fail,
and that is a policy change, not a measurement fix. Questions for the owner, once about ten
real landings carry the advisory:

1. **The data.** How often a landing reads over x1.02, and in each case whether the extra
   work was intended.
2. **The limit.** Fail above a higher limit (for example x1.05), or fail only on a regression
   that is not declared in the CHANGELOG?
3. **The scope.** Judge the round total only, or each call?

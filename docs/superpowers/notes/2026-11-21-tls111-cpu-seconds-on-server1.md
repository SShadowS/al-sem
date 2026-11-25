# TLS-111: on server1, CPU seconds measure the clock and core sharing as much as the work

Status: findings and a proposal; the rule change is an owner decision.

## 1. The failure

ENG-168's first landing (2026-11-20, run `20261120-190123-140745`) failed cdo-speed on
warm-tests CPU x1.44 with instructions x0.9997 and no engine change (ENG-168 filters analyze
output only). warm-tests per pass and side (CPU s, wall s; instructions ~213.0e9 in all four):

| pass | head | base |
|---|---|---|
| 0 | 98.2, 14.9 | 102.1, 16.4 |
| 1 | 127.4, 18.4 | 59.1, 10.0 |

The re-land the same evening passed with the same shape the other way round: pass 0 head
53.7 / base 53.5, pass 1 base 104.6 / head 99.1. CPU **and wall** double on identical work,
with low stall shares (3.9-4.0 %) and low host pressure: the work ran slower, it did not wait.

## 2. The experiment (2026-11-21, under a timed slot hold)

master's `alsem fingerprint` on CDO (a parallel model build), 12 runs alternating all cores and
pinned to the P-cores (`taskset -c 0-11`), counting user-mode instructions and cycles per core
kind (`cpu_core` = P, `cpu_atom` = E and LP-E; fixed counters, no multiplexing). Script and log:
`/work/.al-sem/tools-notes/tls111/cores.sh`, `cores.log`.

| run | mode | CPU s | wall s | instr (e9) | cycles P+E (e9) | E share of instr |
|---|---|---|---|---|---|---|
| 1 | all | 150.1 | 57.6 | 519.4 | 246.1 | 24 % |
| 1 | pinned | 132.8 | 63.3 | 519.8 | 234.6 | 0 |
| 2 | all | 185.9 | 79.4 | 520.0 | 234.5 | 27 % |
| 2 | pinned | 151.5 | 66.4 | 519.9 | 232.3 | 0 |
| 3 | all | 147.7 | 57.2 | 520.0 | 246.5 | 28 % |
| 3 | pinned | 128.8 | 61.6 | 519.4 | 239.5 | 0 |
| 4 | all | 175.8 | 72.9 | 519.5 | 226.7 | 20 % |
| 4 | pinned | 154.3 | 68.9 | 519.8 | 231.2 | 0 |
| 5 | all | 158.2 | 62.5 | 519.5 | 250.9 | 34 % |
| 5 | pinned | 124.1 | 58.6 | 519.7 | 237.9 | 0 |
| 6 | all | 187.8 | 80.1 | 519.6 | 229.5 | 25 % |
| 6 | pinned | 172.7 | 79.1 | 519.8 | 235.1 | 0 |

- **Pinned to P-cores, user CPU seconds still spread 95-126 s (x1.33)** while user-mode P
  cycles stay at 231-240e9 (about ±1.7 %). The effective clock (user cycles / user seconds)
  ran from 1.86 to 2.50 GHz, against a 4.8 GHz peak. System time was a steady 29-32 s, except
  46 s in the slowest run.
- All cores: CPU 148-188 s (x1.27), cycles 227-251e9 (about ±5 %); 20-34 % of the work ran on
  E and LP-E cores. Core placement adds spread, but the larger part is the clock.
- Not on battery (the batteries read "Not charging"); `intel_pstate` active, `powersave`
  governor, `balance_performance` preference. P threads 1 and 2 reach 5.1 GHz, the others 4.8.
- Reviews (Fable, sol; `/work/.al-sem/tools-notes/tls111/review-sol-r1.md`): this shows clock
  variation on `fingerprint`, not the cause of the warm-tests failure (other call, other thread
  count), and it does not separate the clock from cores shared with other threads.

## 3. Experiment 2: the failing call itself, three causes apart (2026-11-21)

The warm-tests call exactly as cdo-speed makes it (cdo-speed's own `Server` and perf attach,
`/work/.al-sem/tools-notes/tls111/warmtests.py`, log `warmtests.log`): master against itself,
a fresh `alsem mcp` per run, the model build untimed, then the timed `alsem_tests` call. The
server is pinned to ONE thread of each P-core (0,1,3,6,8,10). Arms, interleaved, 3 rounds:
**alone**; **e-burn**, a busy loop on every E/LP-E thread (12-21: power drawn, no core shared);
**sib-burn**, a busy loop on each server thread's sibling (2,4,5,7,9,11: cores shared). Per call:
user and system seconds of the server tree; user instructions, user cycles, user ref-cycles
(cycles / ref-cycles = the clock against nominal) and kernel cycles. Same output (`ce1c54ed`)
and 212e9 user instructions in all nine.

| arm | wall s | user s | sys s | user cycles (e9) | clock / TSC (3.09 GHz) | GHz (cycles / user s) | kernel cycles (e9) |
|---|---|---|---|---|---|---|---|
| alone | 10.9 / 12.5 / 15.3 | 24.9 / 26.8 / 38.7 | 6.6 / 6.5 / 8.9 | 88.7 / 87.6 / 87.9 | 115 / 106 / 74 % | 3.57 / 3.28 / 2.27 | 25.6 / 22.4 / 22.6 |
| e-burn | 22.8 / 21.3 / 19.5 | 63.8 / 48.9 / 50.8 | 10.9 / 11.4 / 9.9 | 100.0 / 91.9 / 93.2 | 51 / 61 / 60 % | 1.57 / 1.88 / 1.84 | 17.4 / 20.8 / 18.2 |
| sib-burn | 24.5 / 21.4 / 23.1 | 60.6 / 46.8 / 48.4 | 11.7 / 10.9 / 12.3 | 144.1 / 146.1 / 145.6 | 77 / 101 / 98 % | 2.38 / 3.12 / 3.01 | 29.9 / 38.0 / 38.9 |

Each of two things can roughly double the call's CPU and wall time on identical work (against
the same round's alone run: e-burn x2.38 / x1.81 / x1.28 CPU, sib-burn x2.30 / x1.74 / x1.28):

1. **A lower clock (e-burn).** Load on cores we never touch drops our clock to 1.57-1.88 GHz,
   below the P-cores' 2.3 GHz base, with user cycles up only 4-13 % (memory waits cost fewer
   cycles at a lower clock). The package read 68-71 °C there, so this is a power or platform
   cap, not heat; but RAPL limits and throttle counters are not readable in the container, so
   which cap is not measured.
2. **Cores shared with other threads (sib-burn).** User cycles rise x1.65 (two threads split a
   core's execution units); the clock fell too in round 0 (2.38 GHz).

Not separated by this run:
- The alone runs' clock fell 3.57 → 3.28 → 2.27 GHz in run order, and after round 0 every alone
  run started 20 s after a sib-burn run: carry-over from our own burners explains that as well
  as other tenants (the arm order was fixed; no host load was recorded).
- The package read 100-101 °C in the alone and sib-burn runs: heat throttling is not ruled out
  there.
- Pressure was not recorded per row. The failing landing's container stall share (3.9-4.0 %)
  says its threads ran rather than waited, which fits both causes.
- Which cause hit the landing: cdo-speed pins nothing, so a third one from experiment 1,
  placement on E cores (20-34 % of the work, ±5 % in cycles), also applies there.

## 4. What this means for cdo-speed

- On server1, CPU seconds measure the work times the clock, the core sharing and the core
  kind, and the last three belong to whatever else runs on the host. Each can move x2 between
  the two sides of a pass. Pressure signals see none of them.
- **User cycles** remove the clock (cause 1) but not core sharing (x1.65), and a lower clock
  makes them read 3-13 % low. They are no replacement for CPU seconds.
- **Instructions** held in every run (212e9) but miss memory stalls and lost parallelism
  (TLS-83).
- History (Fable, 150 `speed.json` files): warm-tests' second/first CPU in one pass has a tight
  core (median 1.00, p10-p90 0.94-1.12) and a two-state tail: the call costs about 42-65 s or
  about 100-127 s, and 4 % of pairs pass x1.3. Pass-to-pass disagreement (max/min of the two
  ratios): median x1.06, p90 x1.23, p95 x1.80, max x2.67.
- **Disagreeing passes are not always noise.** A steady position effect k (the second side
  slower) makes the passes disagree by k² while their geometric mean stays right
  (`20261119-135052`: ratios 0.630 and 1.683, mean 1.03, a correct PASS). That is why the side
  order flips, and why a third pass (one order voting twice) would be wrong.

## 5. Proposal

1. **Record and label (no verdict changes).** Per call and core kind: user instructions and
   user cycles (both fixed counters: still no multiplexing). From them, per side, the clock
   (user cycles / user seconds) and cycles per instruction. A judged block whose sides' clocks
   differ beyond a band is labelled `CLOCK`; one whose cycles per instruction differ beyond a
   band, `CPI` (core sharing, core kind, or a real cache or memory change: the label never
   says which). The bands start from these runs and are set after ~20 landings. Label, never
   hide; the same limits judge.
2. **Re-measure a contradicted failure with a flipped pair, never one pass.** When a judged
   row FAILS and its two passes disagree beyond x1.25 (above the history's p90 of x1.23; an
   unchanged tree cannot fail the x1.30 mean below x1.69), cdo-speed runs one more flipped pair
   (two passes, fresh servers, each order once more). If the two ratios of the same order
   agree within x1.25, the geometric mean of all four judges; if they still disagree, the
   verdict is `HOST UNSTABLE`, exit 2 (cannot run), never a PASS. It costs two passes (~3 min)
   only on a contradicted failure, said in the verdict.
3. **Owner question, narrowed (TLS-83's):** may a CPU-only failure with equal instructions AND
   a `CLOCK` or `CPI` label be advisory? A clean clock and equal cycles per instruction with
   more CPU is exactly the stall regression instructions miss: that stays a failure.
4. **Owner ask, host side:** cause 1 is a host power or platform cap below base clock at 68 °C.
   Only the owner can read RAPL limits and throttle counts and try `no_turbo=1` or a
   performance profile; a steady clock at base is better for a gate than a turbo that
   switches.
   As built: the advisory takes a CLOCK label only, and only when head's lower clock explains
   the excess (the CPU ratio at equal clocks, head user × head/base clock ratio + head system
   over base CPU, within the limit; review r4), with covered counts, equal
   instructions and wall passing; CPI never waives (a CPI rise is what a real stall regression
   looks like; Fable, code review r3). So cause 2 (shared cores) stays a failure.
5. **Not pinning in TLS-111.** CPU affinity limits only where our processes run: another
   tenant's threads can still land on the sibling of a core we pin to, so pinning cannot fix
   cause 2; reserving whole cores needs host-side isolation (owner ask
   q-20261121T052155-5d983934). Pinning to P-cores would remove cause 3 but change what the
   gate measures (12 threads, not 22), so it is measured first in TLS-112.
6. **Follow-up, to test first (TLS-112):** both sides at once, each on 3 P-cores with both threads,
   swapping the sets in pass 2 (the only design where both sides see the same host state).
   Test it as one more arm of `warmtests.py` under e-burn and sib-burn before building it;
   every limit would need re-calibrating.

## 6. Owner answers (2026-11-21)

- **Power (q-20261121T025805-9ba03286):** server1 now runs the `performance` power profile
  (energy preference `performance`; about 3.25 GHz average right after the change). Cause 1 is
  to be re-measured under it (TLS-112) before any conclusion above about the clock is reused.
- **Core isolation (q-20261121T052155-5d983934):** none. cdo-speed relies on instruction
  counts and TLS-111's CLOCK and CPI labels; cause 2 (shared cores) stays a failure, labelled
  CPI. Pinning only if TLS-112's measured arms justify it.

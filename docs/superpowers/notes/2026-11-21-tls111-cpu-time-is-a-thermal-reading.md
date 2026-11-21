# TLS-111: on server1, CPU seconds measure the chip's temperature as much as the work

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
- Read right after the runs (with the other lanes' load still on): `x86_pkg_temp` 101 °C,
  not on battery (the batteries read "Not charging"), `intel_pstate` active with the
  `powersave` governor and `balance_performance` preference. The package is at its thermal
  limit, and the thermal and power budget is shared by every core: another lane's load lowers
  our clock without raising our stall share. That is why TLS-76's pressure labels did not fire.

## 3. What this means for cdo-speed

- CPU seconds on server1 depend on the chip's thermal state, which no pressure or stall signal
  sees. A two-pass CPU ratio can move x2 on identical work, which is exactly the failure.
- **User-mode cycles** are the work at the speed the cores ran, without the clock: memory
  stalls count (unlike instructions, TLS-83's reason to keep CPU), and frequency does not.
  On P-cores they held within about ±1.7 %; across mixed core kinds within about ±5 %.
- Instructions held within ±0.1 % (TLS-83), but miss memory stalls and lost parallelism.

## 4. Proposal

1. **Now, advisory (no change to any verdict):** cdo-speed records per call the cycles per core
   kind, the effective clock, the share of work on E cores and the package temperature, and
   labels a block `CLOCK` when the two sides' effective clocks differ by more than a calibrated
   band (label, never hide; the same limits judge).
2. **Owner decision:** judge cdo-speed's CPU rows on user-mode cycles instead of CPU seconds
   (wall keeps its own limits; instructions stay advisory), after a calibration of
   master-against-master passes (the cycle ratio's band), starting advisory as every new gate
   does. Optionally pin the timed servers to the P-cores, which narrows the band (±1.7 %
   against ±5 %) at the cost of the E cores' parallelism (both sides alike).
3. Out of scope here: the host's cooling or power settings (`max_perf_pct`, the governor):
   the owner's machine, not ours to change.

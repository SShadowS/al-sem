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

| arm | wall s | user s | sys s | user cycles (e9) | clock / nominal | GHz (cycles / user s) | kernel cycles (e9) |
|---|---|---|---|---|---|---|---|
| alone | 10.9 / 12.5 / 15.3 | 24.9 / 26.8 / 38.7 | 6.6 / 6.5 / 8.9 | 88.7 / 87.6 / 87.9 | 115 / 106 / 74 % | 3.57 / 3.28 / 2.27 | 25.6 / 22.4 / 22.6 |
| e-burn | 22.8 / 21.3 / 19.5 | 63.8 / 48.9 / 50.8 | 10.9 / 11.4 / 9.9 | 100.0 / 91.9 / 93.2 | 51 / 61 / 60 % | 1.57 / 1.88 / 1.84 | 17.4 / 20.8 / 18.2 |
| sib-burn | 24.5 / 21.4 / 23.1 | 60.6 / 46.8 / 48.4 | 11.7 / 10.9 / 12.3 | 144.1 / 146.1 / 145.6 | 77 / 101 / 98 % | 2.38 / 3.12 / 3.01 | 29.9 / 38.0 / 38.9 |

Two separate causes each double the call's CPU and wall time on identical work:

1. **The package's power budget (e-burn).** Load on cores we never touch drops our clock to
   51-61 % of nominal: user seconds x2.0-2.6 against the fast alone runs, wall x1.8-2.1. User
   cycles rise only 4-13 % (memory waits cost fewer cycles at a lower clock). Even alone, with
   no load of ours, the clock ran 74-115 % (other tenants on the host): user seconds x1.56 with
   user cycles flat (±0.6 %).
2. **Cores shared with other threads (sib-burn).** The clock stays near nominal (77-101 %), but
   user cycles rise x1.65: the two threads of a core split its execution units. CPU seconds
   x1.9-2.4.

Neither shows in TLS-76's pressure signals: the call's threads are running, not waiting.
The package temperature (`x86_pkg_temp`) did not track either cause (it read 61-101 °C with no
relation to the arm), so the cause is the power budget, not heat: the note's first title said
"thermal", which this corrects.

## 4. What this means for cdo-speed

- On server1, CPU seconds measure the work times the clock times the core sharing, and the
  last two belong to whatever else runs on the host. Both can move x2 within seconds, between
  the two sides of a pass. Pressure signals see neither.
- **User cycles** remove the clock (cause 1) but not core sharing (cause 2: x1.65). They are no
  replacement for CPU seconds.
- **Instructions** removed both (212e9 in every run here) but miss memory stalls and lost
  parallelism (TLS-83).
- The failing landing's two passes disagreed (x0.962 and x2.154): a disagreement no real code
  change makes, since both passes run the same two binaries.

## 5. Proposal

1. **Record and label (no verdict changes):** per call, the clock against nominal (user cycles /
   user ref-cycles) and user cycles per instruction, per side. A block whose sides' clocks
   differ beyond a calibrated band is labelled `CLOCK`; one whose cycles per instruction differ
   beyond its band while instructions agree, `SHARED` (cores shared or a real stall change:
   the label says so, never decides). Label, never hide; the same limits judge.
2. **Rerun a pass that contradicts the other:** when the two passes' CPU ratios for a judged
   row disagree beyond the calibrated band, run a third pass and judge the median of the
   three, saying so in the verdict. Real code changes make both passes agree; only noise makes
   them disagree. Costs one pass (~90 s) only on a contradiction.
3. **Owner decision (TLS-83's open question):** let equal instructions turn a CPU-only failure
   into an advisory one, given (1) labels.
4. Out of scope: the host's power settings and other tenants' load, the owner's to decide.

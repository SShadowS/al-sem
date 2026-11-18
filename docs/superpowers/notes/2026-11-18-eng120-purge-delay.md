# ENG-120: mimalloc's `purge_delay` stays 0 (measured on CDO and DO)

`alsem` sets mimalloc's `purge_delay` to 0 (`src/bin/alsem.rs`, `MI_OPTION_PURGE_DELAY`): freed
pages go back to the OS at once. In a one-shot `prove` profile mimalloc's own costs were 6.8 % of
CPU (TLS-86), so ENG-120 asked whether a small delay is worth it. The bar: at least 3 % CPU with
peak RSS within 1.10, on CDO and on DO (owner decision q-20261117T222509-b8512bbb: adopt only
if DO also clears it). **Result: it does not. The delay stays 0.**

All runs: server1, release-fast, a never-landing prototype that reads the delay from
`AL_SEM_PROTO_PURGE_DELAY` (0 is the shipped value). Outputs were identical across delays for
every command (stdout without `generatedAt`).

## CDO (baseline `bc3ccb18`), 2026-11-17

One-shot `alsem prove` (`CDO E-Mail Handler::RunReportID`, may-commit), master a3b8f86f, 3
interleaved rounds, CPU seconds (`/usr/bin/time`):

| delay | CPU s (min / median / max) | sys s (mean) | peak RSS MB (median) |
|---:|---|---:|---:|
| 0 | 72.0 / 74.1 / 78.4 | 4.93 | 2547 |
| 1 | 68.9 / 70.4 / 72.1 | 3.14 | 2601 (+2.1 %) |
| 3 | 68.9 / 72.2 / 72.6 | 2.92 | 2608 (+2.4 %) |
| 10 | 68.7 / 70.0 / 70.2 | 2.65 | 2613 (+2.6 %) |
| 100 | 67.5 / 69.2 / 74.2 | 2.20 | 2620 (+2.9 %) |

The clear signal is kernel time (`madvise`, refaults); the total CPU change sits inside the
host's ±10 % noise. Raw: `/work/.al-sem/profiles/2026-11-17-eng120/purge.tsv`.

## DO (checkout `b3b4459`), 2026-11-18

Master 05beebbc. Pinned to the P-cores (`taskset -c 0-11`, perf under it too), 3 rounds with the
delay order rotated, each run gated on the host's CPU pressure (TLS-76's rule). Counted:
`cpu_core` user- and kernel-mode instructions (`perf stat`, 100 % enabled), task-clock, peak RSS.

- **(a)** DO as checked out: no `.alpackages`, so workspace-only: the small-model case the
  original purge_delay RSS regression came from.
- **(b)** A copy of DO outside the repositories with the CDO baseline's `.alpackages` (DO's five
  dependencies at 30.0.0.0 and platform/application 28.0 are satisfied by them), deleted after.

Means over 3 runs, delay 0 / 1 / 10 ms:

| run | user instr e9 | kernel instr e9 | task-clock s | peak RSS MB |
|---|---|---|---|---|
| (a) prove | 86.98 / 86.95 / 86.98 | 0.83 / 0.64 / 0.53 | 8.1 / 8.1 / 8.0 | 407 / 436 / 456 |
| (a) analyze | 14.66 / 14.67 / 14.66 | 0.66 / 0.52 / 0.41 | 2.7 / 2.7 / 2.6 | 383 / 402 / 415 |
| (b) prove | 414.5 / 414.6 / 414.5 | 9.9 / 11.0 / 7.7 | 73.1 / 74.3 / 72.1 | 2321 / 2418 / 2427 |
| (b) analyze | 290.2 / 290.3 / 290.3 | 7.9 / 6.9 / 11.7 | 57.3 / 57.0 / 60.7 | 2316 / 2358 / 2338 |
| (b) analyze --single-app | 165.7 / 165.7 / 165.7 | 2.03 / 1.70 / 1.50 | 31.1 / 30.0 / 30.5 | 631 / 640 / 649 |

- **CPU:** user-mode instructions are flat across delays (within 0.03 %): the delay changes
  only kernel work. Kernel instructions fall on the small model, but they are under 5 % of the
  total, and in (b) they are dominated by noise. Task-clock moves nowhere near 3 %.
- **Peak RSS** grows with the delay: on the small model prove reaches +7 % at 1 ms and +12 % at
  10 ms (over the 1.10 limit), analyze +5 % and +8 %; with packages +2 % to +5 %.

Raw: `/work/.al-sem/profiles/2026-11-17-eng120/do.log` (numbers only).

## Decision

Keep `purge_delay` 0: DO clears neither half of the bar. The allocator's other costs in the
profile (free-list set-up and fresh pages) mostly came from one site's maps growing again per
producer; ENG-121 fixed that site (`pd_solve`'s per-chunk scratch).

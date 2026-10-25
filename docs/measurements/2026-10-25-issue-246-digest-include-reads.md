# #246 `alsem digest --include-reads`: the CDO cost (plan Task 2)

Measured 2026-10-25 on the pinned CDO workspace (`<CDO_WS>`, git `bc3ccb18`, clean). Head `953a8e0e`, profile
`release-fast` (`--bin alsem`, built in a clean detached worktree), mimalloc as built. Peaks are kernel `VmHWM` through
`/work/bench/160-10-11a/maxrss.py`. `Up` peak = the batch process's `VmHWM`, equal to `[batch] worker 0 peak_kib=` in
every run (one worker). Raw data and scripts: `/work/bench/246/` (`run.sh`, `runs.txt`). No source file was edited.

Method: 3 rounds, alternating flag off / on, one process at a time, order d, h, ds, Up in each round. Before each run:
no foreign cargo/rustc/alsem/aldump/nextest process and `memory.current` below 28 GiB (neither wait was ever needed;
`waits.log` is empty). Outputs have equal byte sizes across rounds per shape and flag (only sizes were compared, and the runs did not pass
`--deterministic`; sha256 kept for round 1).

**Load.** The host load comes from outside the container and was 5-24 (1-minute average at run start 5.4-21.5, at end
5.6-24.8). Peaks barely move with load. Walls do, so CPU-seconds (user+sys) are the primary cost figure. One flag-on `h`
round 1 ran at load 7.7 to 24.8 and took 105.6 s wall (528.9 CPU-s): a load artefact, kept in the range.

## Argv

`F` = `src/DocumentTypes/Finance/General/CDOIssueDocument.Codeunit.al`, `F88` =
`src/Features/HandledState/CDOUnhandledSalesPagesMgt.Codeunit.al` (88 roots). "+R" = `--include-reads` appended.

| key | argv |
|---|---|
| d | `digest <CDO_WS> --file $F --format json` [+R] |
| h | `digest <CDO_WS> --file $F88 --format json` [+R] |
| ds | `digest <CDO_WS> --single-app --file $F --format json` [+R] |
| Up | `batch <manifest>`, jobs [a, d, h, ev, qt], `ALSEM_RETAINED_CENSUS=1`; a = `analyze <CDO_WS> --format sarif`; ev = `events fanout <CDO_WS> --format json`; qt = `query touches <CDO_WS> --table 18 --format json`; flag-on adds +R to d and h only |

## Results (median [min-max] of 3)

| shape | flag | CPU-s | wall s (load) | peak KiB | output bytes | `DB_READ` | incoherent `DB_READ` diags |
|---|---|---|---|---|---|---|---|
| d | off | 125.54 [116.76-126.67] | 27.40 [26.94-27.45] | 3,163,188 [3,159,164-3,170,544] | 20,016,783 | 0 | 0 |
| d | on | 127.49 [123.81-133.25] | 27.46 [27.40-29.57] | 3,155,088 [3,149,800-3,161,852] | 24,469,396 | 1,133 | 1,059 |
| h | off | 423.41 [413.89-440.38] | 47.77 [46.70-56.27] | 3,412,092 [3,359,624-3,496,556] | 441,330,878 | 0 | 0 |
| h | on | 493.42 [483.25-528.88] | 58.37 [53.83-105.59] | 3,663,632 [3,638,772-3,664,608] | 534,104,987 | 23,248 | 21,707 |
| ds | off | 30.52 [30.28-31.76] | 4.85 [4.69-5.15] | 722,476 [710,112-726,332] | 2,951,747 | 0 | 0 |
| ds | on | 30.46 [29.66-30.81] | 4.61 [4.58-5.11] | 732,292 [726,248-743,580] | 3,676,059 | 44 | 0 |
| Up | off | 442.27 [441.78-455.80] | 56.12 [55.24-56.57] | 3,729,108 [3,719,928-3,731,688] | 461,347,661 (d+h) | 0 | 0 |
| Up | on | 486.01 [478.66-497.78] | 60.95 [60.83-64.29] | 4,062,196 [4,054,180-4,075,056] | 558,574,383 (d+h) | 24,381 | 22,766 |

Counts are for the digest outputs (for `Up`, the d and h job outputs: 1,133 + 23,248 = 24,381; 1,059 + 21,707 = 22,766).
The incoherent count is entry diagnostics with `effectType == "DB_READ"` and `kind == "temp-state-incoherent"`.

Ratios, flag on over flag off (medians):

| shape | CPU | wall | peak | bytes |
|---|---|---|---|---|
| d | 1.016 | 1.002 | 0.997 | 1.223 |
| h | 1.165 | 1.222 (load-noisy) | 1.074 | 1.210 |
| ds | 0.998 | 0.950 | 1.014 | 1.245 |
| Up | 1.099 | 1.086 | 1.089 | 1.211 |

## Admission

Rows in `src/engine/batch/admission.rs`: `("digest", X)` = 15 * STEP = 3,932,160 KiB; `("digest", S)` = 3 * STEP =
786,432 KiB.

- `d` on 3,155,088 and `h` on 3,663,632: below the X row. `ds` on 732,292: below the S row.
- `Up` on median 4,062,196 KiB (min 4,054,180, max 4,075,056) is **above** the X row. The rule fires.
  New value: 4,062,196 / 262,144 = 15.50, rounded up = **16 * STEP = 4,194,304 KiB** (4.0 GiB) for `("digest", X)`.
  `("digest", S)` stays 3 * STEP.
- Final review round 1 (G-2): the raised row belongs to `--include-reads` alone, so it is a new kind,
  `("digest reads", X)` = 16 * STEP (`("digest reads", S)` = 3 * STEP, from `ds` on). `("digest", X)` stays 15 * STEP.
- Largest flag-on peak: 4,075,056 KiB, below the stop limit of 7,340,032 KiB. W stays 4 (29,360,128 / 4,194,304 = 7).

## Notes

- The incoherent count is above 0 on `d` and `h` (and so `Up`): 1,059 of 1,133 and 21,707 of 23,248 reads carry a
  `temp-state-incoherent` diagnostic. This is a discovery for O8. A sample on `d` shows `reason: "no-l4-row"`
  (the temp-state join finds no L4 row for the read's operation). `ds` has 0 of 44.
- **Root cause of the `no-l4-row` reads** (follow-up probe, `cdo` profile build of `953a8e0e`, single runs, not timed):
  - All 1,059 are reason `no-l4-row`. 1,047 of them share ONE evidence site, `RecRef.FindFirst` at
    `CDOEMailHandler.Codeunit.al:243`, each for a different table (524 distinct tables under `IssueFinanceChargeMemo`
    alone); the other 12 have no evidence (`evidence-unavailable`).
  - It is not read-specific. The same `d` document carries 772 write effects with the same `no-l4-row` diagnostic in the
    DEFAULT output (472 `DB_INSERT` at one `RecRef.Insert`, 290 `DB_MODIFY` at one `FilterRecord.Modify`). Writes and
    reads fail the join at the same rate (95% and 93%).
  - The join key is right (the operation id) and it reads the right store (the view's `l4`). The L4 store does hold
    reads: `query effects --routine IssueFinanceChargeMemo` lists 307 operations, most of them reads (74 `Get`, 51 `Next`,
    48 `FindSet`, …). But none of the 529 operations behind the root's unjoined reads is among those 307, and the reads
    span 524 tables against L4's 52. The capability cone reaches far more code than the L4 summary for this root.
  - The witness then lands every one of those reads on the one resource-less `RecRef.FindFirst`: `fact_equivalent`
    compares `resourceId` only when both facts have one (the witness.ts parity rule), so a RecordRef fact with no
    resolved table is a carrier for a read of ANY table, and the BFS finds it first. The evidence line is therefore not
    where the table is read.
  - Both are pre-existing (#69's join, the witness carrier rule) and change no code here; recorded as discoveries.
- **`ev` and `qt` checksums in `Up` round 1:** volatile fields only, no leak. None of the bench jobs passed
  `--deterministic`, so `events fanout` writes a live `generated_at` and `query touches` a live `generatedAt` (with
  `deterministic: false`); `analyze --format sarif` writes no timestamp, which is why `a` was equal. Probe: one batch
  `[digest --file $F --include-reads, events fanout, query touches]` and the two standalone runs, all with
  `--deterministic`: `ev` and `qt` are byte-equal (`cmp`). A non-deterministic standalone `ev`/`qt` differs from the
  deterministic one only in `generated_at` (events) and `generatedAt`/`deterministic` (query) after `jq -S`.

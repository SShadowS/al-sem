# ENG-119 re-measure after ENG-110: the lowerer's extra CST scans

ENG-119 (the lowerer's scans beside its main walk) was put on hold until ENG-110 landed.
ENG-110 is the persistent cache of parsed dependency IR, landed at `245e5c43`. This note
re-measures the scans' cost with that cache warm and with it off. No product code changed.

- **Binary:** master `c3f528f3` (contains ENG-110), `profiling` build (release-fast's code
  with symbols).
- **Recorder:** samply, recorded directly on the binary (the same command `scripts/profile`
  runs).
- **Workspace:** the pinned CDO baseline `/work/src/cdo-baseline/Cloud` (`bc3ccb18`).
- **Host:** server1, 2026-11-23, every run inside the CDO slot.
- **Readers:** `tools/census-probe/runs-eng84/samply-shares.py` and `samply-chains.py`, as in
  the TLS-86 note. "CPU" means all threads' CPU, summed from samply's per-sample deltas.
- **Raw profiles and run scripts:** `/work/.al-sem/tools-notes/eng119/` (`run-profiles.sh`,
  `run-gate-phases.sh`, `shares-*.txt`, `chains-*.txt`).

**The two cache states.**
- **Warm:** `ALSEM_IR_CACHE=on`, with `ALSEM_IR_CACHE_DIR` pointing at a private folder that
  one unprofiled `analyze` run filled first. The fill's report: 10,881 files missed, 10
  packages published. Both warm runs' reports: `disk_hits` 10,881, `parsed_miss` 0,
  `refused` 0. So every dependency file came from the cache, and only the workspace's own
  files were lowered.
- **Off:** `ALSEM_IR_CACHE=off`. Every file is parsed and lowered, which is the cold path.

**The commands** (TLS-86's prove; ENG-98's analyze):
- `alsem prove <ws> "CDO E-Mail Handler::RunReportID" may-commit --format json`
- `alsem analyze --format json <ws>`

## 1. One-shot prove and analyze

Self share = CPU spent in the function itself. Inclusive share = CPU spent in the function
and everything it calls. The scan columns are inclusive shares.

| | prove warm | prove off | analyze warm | analyze off |
|---|---:|---:|---:|---:|
| CPU total | 98.3 s | 125.1 s | 53.0 s | 77.9 s |
| `ts_tree_cursor_goto_sibling_internal` self | ≤ 0.09 % | 2.40 % | ≤ 0.19 % | 3.64 % |
| `ts_tree_cursor_goto_first_child_internal` self | ≤ 0.07 % | 2.14 % | ≤ 0.18 % | 3.62 % |
| cursor stepping, sum | ≤ 0.16 % | **4.54 %** | ≤ 0.37 % | **7.26 %** |
| `RawNode::descendants_of_kinds` (= `collect_comments`) | 0.07 % | 1.76 % | 0.15 % | 2.83 % |
| `RawNode::named_ranges_where` (= `preproc_ranges`) | 0.02 % | 0.98 % | 0.04 % | 1.47 % |
| `lower::collect_run_objects` | 0.05 % | 0.96 % | 0.09 % | 1.73 % |
| **the three scans, sum** | **0.14 %** | **3.70 %** | **0.28 %** | **6.03 %** |
| the three scans, CPU-s | 0.14 s | 4.6 s | 0.15 s | 4.7 s |
| `al_syntax::parse::parse` | 1.06 % | 24.10 % | 2.17 % | 36.70 % |
| `lower_file` | 0.46 % | 10.69 % | 0.93 % | 16.57 % |
| `ts_parser_parse` | 0.55 % | 12.39 % | 1.18 % | 18.58 % |

**Notes on the table.**
- The warm self shares are upper bounds. Both cursor functions fell below the top-40 self
  list, so the table gives their inclusive maximum, which is never smaller than the self share.
- **Who calls the scans** (`samply-chains.py <needle> 6`):
  - `descendants_of_kinds` sits 100 % under `lower_file`, where `collect_comments` is inlined
    (`lower/mod.rs:68`, its one caller).
  - `named_ranges_where` sits 100 % under `preproc_ranges`. The chain shows a `memchr` /
    `contains` inline label: that is the `source.contains('#')` test next to the call.
  - `collect_run_objects` sits under `lower_object`.
- **The scans' share of cursor stepping** (`... ts_tree_cursor_goto_sibling_internal 4` and
  `... first_child_internal 4`; the frame above `goto_next_sibling` / `goto_first_child`):
  - prove off: `descendants_of_kinds` 0.63 + 0.29 %, `named_ranges_where` 0.31 + 0.16 %,
    `collect_run_objects` 0.20 + 0.12 %.
  - analyze off: 0.95 + 0.60 %, 0.51 + 0.22 %, 0.32 + 0.25 %.
  - The rest of the stepping is the lowering walk itself: `children_by_field`,
    `lower_routine`, the `from_iter` child lists, `named_children`.
- **`collect_run_objects`'s inclusive share is more than stepping.** It recurses through
  every non-code-block node of an object and calls `lower_property` on every property it
  meets. That repeats property lowering the main walk also does, so its share includes that
  repeated work.
- **Prove's total CPU is not comparable with TLS-86's.** Prove off took 125 s, against 81 s
  in TLS-86 at `9c1c099b`. Prove now does more non-build work (`eval_routine` alone is 4.3 %
  self), so the build's share of prove is smaller than it was.
- **Lost events.** samply reported "Lost 226 / 4745 events" over the four runs. Small next to
  ~350k samples.

**The ceiling.** If the scans cost nothing at all, prove and analyze would save at most
this much CPU:

| | cache warm | cache off |
|---|---:|---:|
| prove | **0.14 %** (0.14 CPU-s) | **3.7 %** (4.6 CPU-s) |
| analyze | **0.28 %** (0.15 CPU-s) | **6.0 %** (4.7 CPU-s) |

A real fix stays below this ceiling. For example, comments folded into the main walk still
have to visit every node that can hold a comment. In absolute terms, one full CDO lowering
spends ~4.6-4.7 CPU-s on the three scans in release-fast. A warm-cache run spends ~0.15
CPU-s, on the workspace's own files only.

## 2. Workloads that still lower every dependency after ENG-110

- **cdo-gate's in-process phases.** Their test binaries never install the cache. These are
  `program_resolve_harness`, `lsp` (`program_graph::`, `snapshot_robustness::`),
  `l4_summary_differential` and `lib_cdo`. All four were profiled directly, below.
- **cdo-gate's `speed` phase.** Its judged builds run with the cache off: 4 timed builds and
  2 untimed rebuilds. Its `build (IR warm)` row first fills each side's cache folder, untimed,
  and a fill also lowers everything. That is 8 full lowerings per run, more when TLS-111 adds
  an extra pair.
- **`output_cdo_impact`.** It runs with `ALSEM_IR_CACHE_VERIFY=1`, so every cache hit is
  parsed again and compared. Gate run `20261123-041410` checked 119,691 hits = 11 × 10,881:
  11 full lowerings across 5 processes.
- **`output_cdo`.** It uses the cache. When the binary is new, its first process fills the
  cache (1 lowering). In `20261123-041410` the cache was already full: 2 processes, every
  file a hit, nothing published.
- **The first run of every new binary.** The cache key is the binary's bytes, so every
  rebuild starts cold. A fill costs about ×1.035 of a cache-off run (ENG-110's numbers).
- **Windows**, until TLS-117: the cache is off there.
- **Any run with `ALSEM_IR_CACHE=off`:** cdo-speed's judged rows, cdo-before-after, and
  every measurement that follows CLAUDE.md's rule.

### cdo-gate's in-process phases, profiled

The four phases were run the way cdo-gate runs them (`cdo` profile, `CDO_WS` and
`ENFORCE_CDO_WS=1`, the same test filters and `--test-threads`), each under samply. All
passed: harness 212, lsp 2, l4 25, lib 21 tests.

| phase | CPU | cursor self | three scans | scans CPU-s | `parse` incl. | ~CDO lowerings |
|---|---:|---:|---:|---:|---:|---:|
| `program_resolve_harness` | 354 s | 10.43 % | 7.95 % | 28.2 s | 53.1 % | ~6 |
| `lsp` | 66 s | 18.63 % | 14.69 % | 9.7 s | 93.2 % | ~2 |
| `l4_summary_differential` | 105 s | 11.74 % | 9.48 % | 9.9 s | 59.9 % | ~2 |
| `lib_cdo` | 1,844 s | 8.32 % | 6.57 % | 121.2 s | 41.4 % | ~25 |
| **sum** | **2,369 s** | | | **~169 s** | | |

- **The "~CDO lowerings" column** is the phase's `al_syntax::parse` CPU divided by ~30 CPU-s
  (prove off's parse CPU, release-fast). The `cdo` build is slower than release-fast, so
  these counts read high. They are only a sense of scale.
- **The scans' cost per lowering matches the one-shots.** As a fraction of `parse` CPU the
  scans are about the same everywhere: 0.159 in `lib_cdo`, 0.153 in prove off.
- **Heavy event loss.** samply reported lost events: harness 237k, lsp 23k, l4 10k, lib 305k.
  - The CPU totals come from per-sample CPU deltas, so they should still count the CPU, but I
    did not check that.
  - The shares are trustworthy only if the lost samples were spread evenly over the code.
  - The phases' samply wall times were longer than the gate's: l4 42 s against 16 s, lib 372 s
    against 280 s. That is the recording's cost.
  - Read this table to about ±25 %.

### Scan CPU at stake in one full cdo-gate (estimate)

| part | how many full lowerings | scans CPU-s |
|---|---|---:|
| in-process phases | measured | ~169 |
| `speed` (release-fast) | 8 × ~4.6 s | ~37 |
| `output_cdo_impact` (VERIFY, `cdo` profile) | 11 × ~4.7 s | ~52 |
| `output_cdo` (new binary fills once) | 0-1 × ~4.7 s | 0-5 |
| **total** | | **~260 CPU-s** |

- Cursor stepping as a whole (self) is ~215 CPU-s in the in-process phases alone. Most of
  that is the main walk, not the scans.
- **Wall time.** The scans run inside the parallel parse, so they save much less wall time
  than CPU time. Scaling each phase's wall time by its scan share gives an estimate of
  roughly 30-40 s of a ~2,700 s gate. That figure was not measured.
- The task's own measure, the scans' share of a gate phase, is 6.6-14.7 % of CPU in each
  in-process phase. That is because those phases mostly parse.

## 3. Answer

- **Cache warm:** the scans' ceiling is **0.14 % of prove's CPU and 0.28 % of analyze's**
  (~0.15 CPU-s per run). That is far below the task's bar of ≥ 1.5 % of the model build's
  CPU. For everyday warm one-shots, the installed `alsem`, and a resident server, ENG-110 has
  removed this cost.
- **Cache off (the cold path):** the ceiling is **3.7 % of prove and 6.0 % of analyze**
  (~4.6-4.7 CPU-s per full CDO lowering). That clears the bar.
- **Is it still worth doing?** Yes, but only as a **cold-path and gate** speed-up:
  - every cdo-gate pays ~260 CPU-s of scan ceiling, ~169 of them in the in-process phases;
  - the first run of every new binary pays it, which is every run in an edit-build-measure
    loop;
  - Windows pays it until TLS-117;
  - every `ALSEM_IR_CACHE=off` measurement pays it.
- **The task should be re-scoped.** It promises a one-shot prove/analyze gain, and with a
  warm cache there is none. Its bar should be judged on the cache-off one-shot, or on an
  in-process gate phase, and its CHANGELOG should say so.
- **What a fix should target,** in order of ceiling (analyze off):
  1. `collect_comments`: 2.8 %. Comments are grammar extras, so a fold into the main walk
     must still find extras the walk never visits. The remaining stepping is the real gain.
  2. `collect_run_objects`: 1.7 %. It walks the object again and lowers every property a
     second time; the main walk's property results could be reused.
  3. `preproc_ranges`: 1.5 %. It already skips files that contain no `#`.

  A realistic fix reaches perhaps half the ceiling. That is about 2-3 % of a cold analyze
  and ~100-150 CPU-s per gate, still above the bar on the cold path.

## 4. What I did not measure

- **No instruction counts and no timed A/B** (`--timed`). The profiles give shares, not a
  calibrated speed-up. A fix needs its own `cdo-before-after` (instructions) with
  `ALSEM_IR_CACHE=off`.
- **The gate's process phases are estimates, not profiles:** `speed`, `output_cdo` and
  `output_cdo_impact` (count of lowerings × the one-shot's per-lowering cost). The impact
  phase's VERIFY lowering runs in the `cdo` profile, so its per-lowering cost may be higher
  than the release-fast figure used.
- **The in-process phases lost many samples** (see above). Their shares assume the losses
  were spread evenly over the code.
- **The gate's wall-time saving** is a scaling estimate, not a measurement.
- **Windows** (TLS-117) was not measured. Its cold path should match the "off" column, but
  that was not checked.

## 5. Implemented

ENG-119 was implemented on this branch (`55be7ee3`, `a9299095`). On a cache-off one-shot
analyze on CDO it gives instructions x0.927, with byte-identical outputs. The measurements and
discrimination proofs are in the CHANGELOG entry. The raw after-profiles are
`/work/.al-sem/tools-notes/eng119/after2-*`, and the instruction pairs are in `perf-ab/`.

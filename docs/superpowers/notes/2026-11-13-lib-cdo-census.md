# cdo-gate's lib_cdo phase and the test layout (TLS-32), plus TLS-33's follow-up

## Each lib CDO test alone (2026-11-13)

Setup:
- the gate's `cdo` profile lib test binary (`al_sem-48da630ba59f334f`, master `00f36fad`);
- one test per process, one thread, `CDO_WS` the pinned baseline, `ENFORCE_CDO_WS=1`;
- `/usr/bin/time -v`; each run started at MemAvailable ≥ 35 GiB and load < 10;
- every test passed.

| test | wall s | peak RSS MiB |
|---|---:|---:|
| `engine::resident::revision::tests::retained_emission_equals_a_composition_on_cdo` | 105.5 | **18,628** |
| `engine::l5::digest::wave2_index_tests::cdo_crossing_census` | 108.3 | 4,252 |
| `engine::l4::capability_cone::scc_set_tests::derived_only_matches_raw_fold_on_cdo` | 100.8 | 4,499 |
| `engine::l5::digest::no_path_tests::cdo_i369_no_path_rows_and_fact_ids` | 55.0 | 4,017 |
| `engine::l4::pd_solve::tests::parallel_solve_equals_the_serial_walk_on_cdo` | 41.7 | 4,552 |
| `program::dep_cache::tests::cdo_light_and_full_profiles_give_the_same_report_and_answers` | 25.2 | 3,008 |
| `engine::l4::exec_relation::tests::relation_equals_the_legacy_base_on_cdo` | 24.2 | 3,103 |
| `engine::l5::detector_context::tests_l5::parallel_direct_facts_equal_serial_on_cdo` | 23.3 | 3,440 |
| `program::resolve::reads::tests::tripwire_no_read_bypasses_the_facade_on_cdo` | 9.6 | 1,570 |
| `program::build::tests::source_overload_alias_collision_guard_group_count_pinned_on_cdo` | 4.9 | 1,527 |
| `program::build::tests::build_program_graph_over_cdo_workspace` | 4.6 | 1,550 |
| `snapshot::parse::tests::parses_all_cdo_source_units_zero_panics` | 2.9 | 1,185 |
| three `snapshot::snapshot::tests` | 0.4-0.5 each | 280 each |

Sum: 507 s. The two `cdo_gate_coverage_tests` build nothing.

What the phase's ~30 GB is: `retained_emission` plus three ~4.5 GB tests beside it, at 4
threads.
- `retained_emission` alone holds at once: a resident revision with its fold and cone store, the
  warm raw emission for over 1,200 subjects, a cold composition built while the store is alive,
  and both emissions rendered to text. Cutting it is TLS-35.
- Sharing the models (the original TLS-32 idea) would not help the wall or the peak much:
  - the duplicate builds are many (the cross-app base 7×, the single-app model 3×, the snapshot
    6×), but they cost CPU, not wall;
  - `retained_emission` (~105-185 s, unshareable: it fills its own revision) bounds the phase;
  - a shared model would stay alive for the whole run.

The census found no test that writes the disk, the env or a shared cache. Three must keep their
own builds:
- `dep_cache`: three independent profile builds;
- `tripwire_no_read`: per-thread facade hook counters;
- `retained_emission`: fills its own revision's artifacts.

## Each phase's worst peak (12 gates, 2026-11-12/13)

The container's unreclaimable MiB at each phase's worst (`memory.timeline` against
`phases.times`):

| phase | worst MiB |
|---|---:|
| `lib_cdo` (4 threads) | 30,136 |
| `output_cdo` | 27,133 |
| `output_cdo_impact` | 17,521 |
| `program_resolve_harness` | 10,004 |
| `lsp` | 5,847 |
| `l4_summary_differential` | 4,174 |

Container limit 49,152 MiB; the owner's rule keeps 4 GiB free (≤ 45,056). These are container
totals, other agents included, so adding two phases' peaks over-counts a little. That is safe.

## The layout chosen (TLS-32)

- `output_cdo` runs alone.
- Then the impact phase, with lane 2 beside it (harness, lsp, l4, then `lib_cdo` at 2 threads):
  - lib_cdo at 2 threads is at most ~23,200 (18,628 + 4,552);
  - with impact, ~40,700 (≥ 8.4 GiB free).
- Lane 2 takes ~69 + ~254 s, inside the impact phase's 348 s. The test phases should go
  818 → ~515 s.
- The memory check (the host bound from the orchestrator; the container bound from review r1,
  sol and Opus), read once after `output_cdo`, when neither part has allocated yet:
  - need = the impact phase's own worst (the gate's RSS, 16,076 over 12 gates, taken as 16,100)
    + lib_cdo's own worst at that thread count (23,300 at 2, 18,700 at 1) + 4 GiB;
  - it must fit both the host's MemAvailable (the gate starts at ≥ 35 GB, sized for a serial
    gate; the host must not swap) and the container's free memory (`memory.max` minus what
    cannot be reclaimed, every agent in it counted: the owner's rule);
  - the most threads that fit win: 2 (needs 43,496), else 1 (38,896), else lane 2 runs after
    the impact phase.
  - Today the container's other agents hold ~6-10 GB, so 1 thread is the likely pick
    (~75 s saved) and 2 threads (~300 s) only on a quiet container. TLS-35 (`retained_emission`'s
    peak) would lower both needs.
  - `CDO_GATE_SERIAL=1` keeps every phase in a row.
- Not pinned by a test: the scheduling itself (the branch on `lane2_beside`). The test checks
  `plan_layout`'s decision through `--layout`; the summary's layout line and the landing gate
  show the rest.
- Risk to watch: in TLS-23's attempt the impact tests slowed under contention (1,047 s against
  428 s alone, beside a 4-thread lib_cdo). Here lane 2 runs niced, with lib_cdo at 2 threads.
  The landing gate's impact time shows whether that holds.
- Measured, the first landing gate in the parallel layout (TLS-32's own,
  `cdo-gate-logs/20261113-091217-3487276`, 2026-11-13; host load ~195 at the speed phase's start):
  - layout: parallel, lib_cdo at 2 threads (need 43,496 MiB; host MemAvailable 49,455, container
    free 47,507);
  - memory: peak 27,275 MiB, headroom 21.9 GiB, no OOM. The memory estimate was very cautious;
  - time: no gain.
    - The impact phase took 472 s (~350 serial).
    - Lane 2 took 598 s: harness 86 (~40-45 serial), lsp 14, l4 29, lib_cdo 469 (234 at 4 threads
      serial).
    - The parallel section took ~598 s against ~650 serial, and the whole gate 1,163 s.
- Why, from the lib_cdo log (each test's time in this gate, against the census alone):

  | test | gate | alone | ratio | beside impact |
  |---|---:|---:|---:|---|
  | `relation_equals_the_legacy_base_on_cdo` | 57 s | 24 s | 2.4× | yes |
  | `derived_only_matches_raw_fold_on_cdo` | 299 s | 101 s | 3.0× | mostly |
  | `parallel_solve_equals_the_serial_walk_on_cdo` | 95 s | 42 s | 2.3× | yes |
  | `parallel_direct_facts_equal_serial_on_cdo` | 76 s | 23 s | 3.3× | yes |
  | `cdo_i369_no_path_rows_and_fact_ids` | 86 s | 55 s | 1.6× | partly |
  | `cdo_crossing_census` | 129 s | 108 s | 1.2× | barely |
  | `retained_emission_equals_a_composition_on_cdo` | 127 s | 105 s | 1.2× | no (impact had ended) |

  - The slowdown is CPU contention between the two parts, on top of the outside load. The tests
    that ran beside the impact phase took 2.3-3.3× their time alone; those after it ran near it.
  - "2 threads" is not 2 cores. Each test runs parallel code inside (rayon pools: the model
    build, TLS-29's walk, the parallel solve, the parallel direct facts), so lane 2 competes for
    every core, and the impact phase slows too (472 s against ~350).
  - The packing also hurt. libtest starts tests in name order, so the longest,
    `retained_emission` (105 s alone), started last, after the impact phase had ended.
- What could make it pay (not done; for the orchestrator):
  1. Cap lane 2's own parallelism when it runs beside the impact phase (`RAYON_NUM_THREADS` for
     its phases), so the impact phase keeps most cores. Lane 2 has ~350 s of room.
  2. Start the long lib tests first (nextest supports ordering; libtest does not).
  3. Measure again on a quieter host. Load ~195 on 22 cores makes this gate's times mostly noise
     for both layouts.
  Until then the parallel layout costs nothing in memory (21.9 GiB headroom) and gains little in
  time on a loaded host.

## TLS-36: the A/B, and serial as the default

TLS-36 did fixes 1 and 2 above: lane 2 pinned to the efficiency cores (12-19 on server1) and
`lib_cdo`'s five longest tests first. Then three gates measured the layouts:

| gate | layout | impact | lane 2 | lib_cdo | whole gate | peak |
|---|---|---:|---:|---:|---:|---:|
| TLS-34 landing (host load ~380) | parallel, 2 threads, no pin | 507 s | | 477 s | ~1,299 s | 27.1 GB |
| A on `d234c4a6` | serial | 359 s | 341 s after it (harness 42, lsp 7, l4 18, lib 274) | 274 s (long 214, rest 59) | **875 s** | 28.8 GB |
| B on `d234c4a6` | parallel, pinned, longest first | 510 s | 915 s beside it (harness 133, lsp 33, l4 52, lib 697) | 697 s | 1,131 s | 27.0 GB |

`output_cdo` took 175 s in A.

- Serial was 256 s faster on the same tree. Even pinned to the efficiency cores, lane 2 ran
  2.5-4.7× slower beside the impact phase, and the impact phase still lost ~150 s. The pin
  keeps them off each other's cores, so the contention is elsewhere (likely memory bandwidth and
  the host's other load; not measured).
- The peak barely moves (28.8 against 27.0 GB), so memory gives no reason to prefer parallel.
- **Decision (orchestrator, 2026-11-13):** serial is the default. `CDO_GATE_LAYOUT=parallel`
  keeps the parallel layout, with the pin and the memory check, for a host where it pays.
  Longest-first stays in both layouts: it costs nothing serial.

## TLS-33 follow-up: the 60 s left in speed_build

In TLS-33's landing gate (`20261113-075414-720626`), `speed_build` took 60 s, of which the
working tree's build took 0 s.
- The 60 s were `cdo-speed`'s `cached_binary` waiting on the cache lock (`~/.cache/al-sem/
  cdo-speed/lock`).
- `merge-and-push`'s post-landing warm build of the new base (`6ad5b93f`, started when ENG-25
  landed; TLS-18) was still running when TLS-33's gate began.
- A landing's merge commit has exactly the tree its gate tested, and that gate already holds the
  tree's release-fast binary. Seeding the cache from it is TLS-34.

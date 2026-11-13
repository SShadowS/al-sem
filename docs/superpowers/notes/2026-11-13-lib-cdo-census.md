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
- The host check, added by the orchestrator: a serial gate starts at host MemAvailable ≥ 35 GB.
  - The parallel layout needs MemAvailable after `output_cdo` ≥ impact's worst + lane 2's worst
    + 4 GiB (44,996 MiB), because neither has allocated yet at that point.
  - Below it, lane 2 runs after the impact phase.
  - `CDO_GATE_SERIAL=1` keeps every phase in a row.
- Risk to watch: in TLS-23's attempt the impact tests slowed under contention (1,047 s against
  428 s alone, beside a 4-thread lib_cdo). Here lane 2 runs niced, with lib_cdo at 2 threads.
  The landing gate's impact time shows whether that holds.
- Measured: (the first landing gate in the parallel layout; to fill in.)

## TLS-33 follow-up: the 60 s left in speed_build

In TLS-33's landing gate (`20261113-075414-720626`), `speed_build` took 60 s, of which the
working tree's build took 0 s.
- The 60 s were `cdo-speed`'s `cached_binary` waiting on the cache lock (`~/.cache/al-sem/
  cdo-speed/lock`).
- `merge-and-push`'s post-landing warm build of the new base (`6ad5b93f`, started when ENG-25
  landed; TLS-18) was still running when TLS-33's gate began.
- A landing's merge commit has exactly the tree its gate tested, and that gate already holds the
  tree's release-fast binary. Seeding the cache from it is TLS-34.

# TLS-09: the gate's optimized build is `release-fast` (measured, 2026-11-12)

**What changed.** `scripts/ci-steps build` and `perf-bounds` (run by `ci-steps all`, the branch
gate, and by GitHub CI's `release` job) build with `--profile release-fast` (thin LTO, 4 codegen
units) instead of `--release` (fat LTO, 1 codegen unit). The shipped profile, `release-ship`
(= `release` + `panic = "abort"`), is unchanged and is still built before any tag (below).

The owner's rule (TLS-09 brief): "the release build is what ships. Any change to its LTO or
codegen settings must be measured for runtime speed and memory as well as build time, and must
not make the shipped binary slower." This step changes no profile, only which one the gate
builds. A change to `release` itself is step (b), measured separately (section 5).

## 1. Where the release build's time goes

- Tree: master `d887bb47`; own target folder (`target3`), dependencies warm (an untimed build
  first), then `touch src/lib.rs` and a timed build, as a gate after an edit. Host: server1,
  22 cores. `timeit.py` (`docs/superpowers/notes/crate-split-phase0/`) for wall, CPU and peak
  child RSS; cargo `--timings` for each unit. 2026-11-12 09:52-10:35 UTC.
- **Noisy**: other work kept the load at 24-26 before both `release` runs (36-39 % idle) and 7
  before `release-fast`. The split below is what matters, and it barely depends on load: the
  `release` build is almost all one thread.

| Build | Wall | CPU | Peak child RSS |
|---|---:|---:|---:|
| `cargo build --release` (every bin) | 636 s | 953 s | 4,631 MiB |
| `cargo test --release --test perf_bounds --no-run` | 78 s | 77 s | 1,603 MiB |
| `cargo build --profile release-fast` | 200 s | 1,259 s | 5,238 MiB |
| `cargo test --profile release-fast --test perf_bounds --no-run` | 24 s | 105 s | 2,038 MiB |

Per unit (`--timings`):

| Unit | `release` | `release-fast` |
|---|---:|---:|
| `al-sem` lib | 392 s (one codegen unit: one thread) | 98 s |
| `alsem` bin (its own LTO link), after the lib | 242 s | 101 s |
| `aldump` bin, beside it | 164 s | 77 s |
| `al-call-hierarchy` bin, beside it | 122 s | 52 s |

So the `release` build is 62 % the lib's single codegen unit and 38 % the `alsem` fat-LTO link.

**In the gate.** ENG-22's landing `ci-steps all` (2026-11-12): clippy 46 s; the test lane
(debug build 74 s + 5,829 tests 113 s, about 3.1 min) beside the release build, which took
9 min 41 s, so the release build was the critical path by about 6.5 minutes; then the
`perf_bounds` build 55 s and its run. With `release-fast` the build (about 200 s) fits under the
test lane.

## 2. Where the shipped profile is still built before a tag

`.github/workflows/build-and-deploy.yml`, on every push to master that touches code (lines 3-14):

```yaml
on:
  push:
    branches: [master]
    paths:
      - 'src/**'
      - 'crates/**'
      - '.cargo/**'
      - 'Cargo.toml'
      - 'Cargo.lock'
      - 'rust-toolchain.toml'  # a toolchain bump changes the shipped binaries (TLS-09)
      - 'build.rs'
      - '.github/workflows/build-and-deploy.yml'
  workflow_dispatch:  # Allow manual trigger
```

and in its jobs (each `needs: test`):

- `build-windows` (`windows-latest`): `run: cargo build --profile release-ship`
- `build-linux` (`ubuntu-22.04`): `run: cargo build --profile release-ship` and
  `run: cargo build --profile release-ship --no-default-features --bin alsem`
- `build-macos` (`macos-latest`): `cargo build --profile release-ship --target x86_64-apple-darwin`
  and `--target aarch64-apple-darwin`

The last 8 master pushes before this change all ran it; 7 passed, and `f12bf250` failed in its
`test` job (the grammar hash check, `crates/al-syntax/build.rs:28`: tree-sitter-al `main` had
moved to 5.2.0 before the 3.0.0 landing re-pinned it), so its builds were skipped. A tag's
`release.yml` builds `release-ship` again.

**What this gives up.** A failure that only fat LTO or `panic = "abort"` causes now shows on the
master push, after the merge, not in the branch gate. Before this change the gate built fat LTO
but never `panic = "abort"`, so the abort case already showed only there.

**Gaps.** `rust-toolchain.toml` was not in the paths filter, so a toolchain bump never built
`release-ship` before a tag; this change adds it. A grammar change (tree-sitter-al `main`, another
repository) still triggers nothing, as before.

## 3. `perf_bounds` on `release-fast`

The bounds were set on `--release`, so the check before the switch: does every row keep at least
2x headroom when the test is built with `release-fast`? The two test binaries built in section 1
(master `d887bb47`), run directly as `cargo test` runs them (whole binary, no filter), 3 runs
each, alternating, 2026-11-12 10:47:51-10:48:22, load 4.2-4.5 at each start (other work; the
orchestrator held the lanes' builds). All 6 runs passed (14 tests). Per row: the median over the
3 runs of each run's min (the gate's own estimator), the tightest bound, and the headroom
(bound / min). Scripts and raw rows: `gate-release-build/` next to this note (`measure.sh` and
`runs.jsonl` for section 1, `perfb-runs.sh`, `perfb_table.py` and `perfb-logs/` for this one;
their paths point at the session's scratchpad).

| Row | `release` | `release-fast` | fast / release | Bound | Headroom `release` | Headroom `release-fast` |
|---|---:|---:|---:|---:|---:|---:|
| `batch_write_check` (2000 jobs) | 15.3 ms | 16.6 ms | 1.08 | none | - | - |
| `build_full_1000_files` | 77.2 ms | 81.1 ms | 1.05 | 6 s | 78x | 74x |
| `build_full_100_files` | 8.84 ms | 9.06 ms | 1.02 | 1.5 s | 170x | 166x |
| `compute_all(1000)` | 9.85 ms | 9.60 ms | 0.97 | 150 ms | 15x | 16x |
| `compute_all` scaling (ratio) | 4.645 | 4.726 | 1.02 | 10 | 2.2x | 2.1x |
| `compute_summaries_v2_bundle(1000)` | 65.3 ms | 73.2 ms | 1.12 | 230 ms | 3.5x | 3.1x |
| `incoming` | 3.36 ms | 3.59 ms | 1.07 | 75 ms | 22x | 21x |
| `prepare` | 2 µs | 4 µs | 1.74 | 3 ms | 1,470x | 847x |
| `outgoing` | 10 µs | 12 µs | 1.21 | 3 ms | 312x | 258x |
| `rung1_body_edit` | 15.3 ms | 14.0 ms | 0.91 | 100 ms | 6.5x | 7.2x |
| `recursive_scc_db_effects` | 138.9 ms | 171.1 ms | 1.23 | 650 ms | 4.7x | 3.8x |
| `rung1_body_edit_scoped` | 1.88 ms | 1.71 ms | 0.91 | 65 ms | 35x | 38x |
| `rung2_signature_edit` | 44.8 ms | 43.6 ms | 0.97 | 750 ms | 17x | 17x |

- **Every row keeps at least 2x headroom**, so the switch goes ahead. The tightest is the
  `compute_all` scaling ratio (2.2x -> 2.1x); it is a ratio of two sizes, so the profile barely
  moves it.
- `release-fast` is up to 23 % slower on a row (`recursive_scc_db_effects`) and 12 % on
  `compute_summaries_v2`; `prepare`'s 1.74 is 2 µs against 4 µs, at the timer's floor.
- These are server1 figures. GitHub's runner is slower (`tests/perf_bounds.rs`' own notes put it
  at about 2x), where the thinnest rows had less headroom already on `--release`; a flake there
  would show first on `compute_summaries_v2` and `recursive_scc_db_effects`.

## 4. As built

- `scripts/ci-steps`: `GATE_PROFILE=release-fast`, used by `run_build` and `run_perf_bounds`.
  `ci-steps all` still runs them in `<target>/gate-release` beside the tests.
- `.github/workflows/ci.yml`: comments only (the steps call `ci-steps`).
- `.github/workflows/build-and-deploy.yml`: `rust-toolchain.toml` in the paths filter.
- `CLAUDE.md` (Build Commands, Test tiers row 3, Performance Targets) and the
  `tests/perf_bounds.rs` module doc say which profile the gate builds.

## 5. Step (b): more codegen units for `release` (measured; not changed)

**Result: `release` keeps `codegen-units = 1`.** More units make the shipped build about 42 %
faster, but the runtime noise on this host is about ±10 %. That is too wide to show that the
shipped binary is not slower, which the owner's rule requires. The orchestrator decided
(2026-11-12) to keep cu=1 and record the numbers. The gain would now come only in CI's shipped
build, since the gate builds `release-fast` (section 4).

**Builds.** Master `3bcfaae7`. One target folder per value. The value is set with
`CARGO_PROFILE_RELEASE_CODEGEN_UNITS` and `CARGO_PROFILE_RELEASE_SHIP_CODEGEN_UNITS`;
`Cargo.toml` is not edited. For each: an untimed `cargo build --profile release-ship`, then
`touch src/lib.rs` and a timed build with `--timings`. 2026-11-12 12:13-13:14 UTC, quiet host:
the heavy window, no other build in the container. Script and raw rows: `gate-release-build/b/`
(`b-measure.sh`, `runs.jsonl`).

| `codegen-units` | Wall | CPU | Peak child RSS | `alsem` size | Load at start |
|---|---:|---:|---:|---:|---:|
| 1 (today) | 511 s | 768 s | 4,410 MiB | 39.9 MB | 3.9 |
| 16 | 297 s | 790 s | 5,315 MiB | 42.4 MB | 3.6 |
| 4 | 299 s | 692 s | 5,048 MiB | 42.2 MB | 8.3 |

Each value also built the `perf_bounds` test binary (`cargo test --release --no-run`): 600 / 345 /
363 s. Those times are not a cu comparison. `release` unwinds and `release-ship` aborts, so these
builds shared no artifacts with the ones above: each rebuilt all 255 dependency units. The
`perf_bounds` runs were not made, because the decision was already taken.

**Runtime noise (A/A control).** `scripts/cdo-speed /work/src/cdo-baseline/Cloud --base-exe
<cu1 alsem> --head-exe <a byte copy of it> --rounds 5`: the same binary on both sides, so any
ratio away from 1.00 is noise. 2026-11-12 13:20-13:25 UTC, load 5.9 at the start and 7.3 at the
end, 55 GB free. Raw: `gate-release-build/b/speed-aa.json` and `speed-aa.log`.

| Call | Median CPU ratio | Per-round CPU ratios |
|---|---:|---|
| prove | 1.050 | 0.905, 1.128, 1.050, 1.132, 0.966 |
| explain | 0.905 | 0.823, 1.180, 1.048, 0.905, 0.750 |
| digest | 0.938 | 0.573, 1.024, 0.938, 1.097, 0.831 |
| query | 1.038 | 1.183, 1.000, 1.062, 1.038, 0.770 |
| tests | 1.059 | 1.059, 1.083, 0.938, 1.059, 0.854 |
| impact | 0.951 | 0.951, 1.289, 0.890, 1.068, 0.874 |
| **round total** | **1.037** | 0.904, 1.132, 1.037, 1.109, 0.932 |

- Warm-up, one sample each: the model build CPU x1.18 and wall x1.28; the test universe CPU x0.973
  and wall x0.969.
- Peak memory (VmHWM summed over the servers) x0.991. The output was the same on every call.
  `cdo-speed` said PASS.
- The acceptance bar agreed for (b) needed a variant's round-total median ratio within 1.02 and
  inside the A/A band. With round-total ratios from 0.90 to 1.13 for one binary against itself, no
  variant could be told apart from cu=1. Three interleaved runs would not narrow ±10 % to 2 %.

**`cdo-speed` has at most 5 rounds.** It has 5 round routines. `--rounds 6` ran the warm-up and 5
rounds, then stopped with `IndexError` and wrote no `speed.json`. Filed as TLS-25, so the script
rejects the value up front.

**If anyone revisits this.** Measure instruction counts (`perf stat -e instructions` per call, or
over the whole round) instead of wall or CPU time. Instruction counts barely move with host load,
so a few-percent difference between cu values could be resolved on a shared host.

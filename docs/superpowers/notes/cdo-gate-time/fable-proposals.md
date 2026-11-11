# TLS-16: ways to cut `cdo-gate` wall time (independent opinion, Fable)

Read-only study, 2026-11-11. Nothing was built or run. Every number comes from the gate
logs under `/work/alsem-wt/orchestrator-gate/target/cdo-gate-logs/` (runs `003611`,
`012547`, `052939`; `065233` for the builds only) or from the code. Where I infer or
guess, I say so.

## Where the time goes (run `20261111-003611`, 1,621 s = 27 min, quiet host)

| gate clock (s) | what | seconds |
|---|---|---|
| 0-204 | `build` (31 s, warm) beside `speed_build` (204 s: the working tree's release-fast `alsem`) | 204 |
| 204-451 | `speed` | 247 |
| 451-531 | harness 51, lsp 9, l4 20 | 80 |
| 531-822 | `lib_cdo` (17 tests, 4 threads) | 291 |
| 823-1621 | `output_cdo` (24 tests) | 798 |

### `output_cdo`, per test (seconds after the phase starts)

| test | run 003611 | run 012547 | run 052939 (busy) |
|---|---|---|---|
| `cdo_run_report_id_lists_calc_sales_disc_commits` | 160 | 164 | 160 |
| `cdo_selectors_round_trip` | 284 | 308 | 285 |
| the 19 tests on the shared server, and the two with a server of their own | 307-314 | 320-328 | 318-322 |
| `cdo_test_selection_ratchets` | 347 | 364 | 357 |
| `cdo_impact_ratchets` | 657 | 662 | 687 |
| `cdo_impact_base_ratchets` | 798 | 804 | 1,032 |

So CLAUDE.md is right, with a correction. Everything except the two impact ratchets is
done about 350 s in. The last ~450 s (56% of the phase, 28% of the gate) waits only on
the two impact ratchets, and the last 141 s only on `cdo_impact_base_ratchets`. They are
not "4-7 minutes each" any more as wall time, because their one-shot calls now run at
once; they are 11 and 13 minutes when run beside everything else in the phase.

What the phase really starts, all at once (read from the test code):

- 3 resident `alsem mcp` servers on CDO, not 1: the shared `cdo_answers` server
  (`output_cdo.rs:283`), one more in `cdo_explain_digest_and_prove_give_one_table_answer`
  (`output_cdo.rs:611`), one more in `cdo_round2_size_ratchets`
  (`output_invariants_w3.rs:1689`).
- 5 one-shot `alsem impact` processes: the diff as JSON and the same diff as human text
  (the same analysis done twice), one routine, and `--base` as JSON and as human text
  (each of those two builds the base model and the head model).
- `cdo_test_selection_ratchets`: 3-4 one-shot `alsem tests` calls in a row;
  `cdo_run_report_id...`: 1 one-shot; `cdo_selectors_round_trip`; one `aldump` dump.

That is about 12-14 cross-app CDO model builds (each ~155-175 s of CPU on release-fast,
from the speed log) and several test universes (~135 s of CPU each) in one phase. This
is the TLS-12 lesson again.

Is the cost in `alsem` or in the test setup? Both. The code comment in
`cli_impact.rs:573` gives, alone on the `cdo` profile: one routine 65 s, the 699-root
diff 263-275 s, `--base` 428 s. The speed log gives the resident cost of impact on one
routine, warm: 1.0-1.5 s. So a one-shot impact has a floor of ~65 s (model + test
universe) and the diff adds ~200 s for 699 roots, about 0.3 s per root, on ~3 cores
(the gate script's own note). `src/engine/impact/` has no `rayon` use. **Inference, not
measured:** the per-root work runs one root after another. That is a product speed
issue: a 94-file diff takes 4.5 minutes for a user too.

### `lib_cdo`, per test

17 tests, 291 s on 4 threads (the script says 594 s summed when each runs alone, the
longest 137 s). The long ones: `derived_only_matches_raw_fold_on_cdo` (~190-250 s in
the phase), `retained_emission_equals_a_composition_on_cdo` (~180 s),
`cdo_crossing_census` (~135 s), `parallel_solve_equals_the_serial_walk_on_cdo` (~108 s),
then five of 45-60 s. I read eight of them: each builds its own CDO model from the
workspace (several build both the cross-app and the single-app model). None shares a
fixture.

### `speed` (247 s)

Warm-up 132 s: the two model builds 35 + 41 s, the two test universes 18 + 19 s, the
small calls ~19 s. Three rounds, 36 s each; `prove` is 28 of the 36 s. 7 s to close.
Spread of the judged calls: prove 1.01, round total 1.04.

### Builds

- `build` (the `cdo` test build): 31 s warm, 213-249 s after a source change, 419 s on
  the busy host.
- `speed_build`: the working tree's release-fast `alsem` takes 186-204 s alone (thin
  LTO, not incremental, so any source change rebuilds the whole crate), 290 s beside the
  test build. When the base commit is not in the cache, the base is built first and the
  head after it: 303 + 186 = 490 s in run `065233`, 595 s in run `052939`. Two of
  today's four runs paid for a base build.
- Nothing is shared with `ci-steps all`, which builds two other profiles (dev for the
  tests, full `release` for the build and `perf_bounds`). One landing compiles the crate
  under four profiles.

## The ranked list

Savings are against the 27-28 min quiet-host gate unless stated.

### 1. Run the impact ratchets as their own lane, also in serial mode. Saves ~6 min.

Today serial mode waits 372 s for lane 2, then 798 s for `output_cdo`, whose last
450 s is five processes on ~3 cores each while 22 cores idle. Split the phase by
filter:

- lane I (background, whole time): `cargo test ... --test cli output_cdo::cdo_impact`
  (matches both impact ratchets);
- foreground: `output_cdo:: --skip output_cdo::cdo_impact` (~350 s), then harness, lsp,
  l4, lib (372 s).

Wall after the speed phase: about max(660-800, 350 + 372) = ~800 s against 1,170 s
today. **Saving ~370 s.**

- Weakens: nothing. The same tests run with the same asserts. `on_the_cdo_gate` in
  `tests/common/cdo.rs` and `cdo_gate_run_set_matches_the_script` must change with the
  script.
- Memory: today's serial peak is lane I's processes plus the rest of `output_cdo`
  (three servers and more). The new peak is lane I plus the larger of (rest of
  `output_cdo`) and (lib on 4 threads, at most ~17 GB by the script's 4.2 GB per test).
  **Guess:** about the same peak as today. I have no memory number for serial mode.
- Verify: one run through the orchestrator, reading the gate cgroup's `memory.peak`,
  before making it the default. If it does not fit, do item 4 first.

### 2. Make `alsem impact` on many roots faster (product). Saves up to ~7 min, and users gain.

If the ~200 s per 699-root diff is per-root work on one thread (inference above), then
running roots in parallel should bring the diff from ~270 s toward the ~65 s floor plus
a small rest, and `--base` from 428 s toward ~150 s. `output_cdo` would then end when
its other tests do (~350 s): **saving up to ~450 s** in serial mode. With item 1 done
too, the saving on top of item 1 is small until item 4 is done, because lane 2 then
sets the length.

- Weakens: nothing; the ratchets (699 roots, 12,395 lines, `+681 routines`) pin the
  answer, so an ordering mistake fails them.
- Risk: output order must stay deterministic; memory per process rises with threads.
- Verify: first one traced run of `alsem impact --diff` on CDO to confirm where the
  200 s goes (through the orchestrator). Do not start the work before that. Then the
  ratchets, and `cdo-speed` (its `impact` call must not slow down).
- Smaller sibling, no product change: the diff's JSON and human answers are one
  analysis rendered twice by two processes (same for `--base`). That costs CPU and
  memory, not wall time, so it matters only as memory relief for items 1 and 6.

### 3. Never build the speed base in the gate. Saves 4-10 min on the gates that pay it (half of today's).

`cdo-speed` caches the base binary by commit. Each landing moves master, so the next
branch's first gate builds a new base: 303 s and 595 s today, before the head build can
even start. But the landing gate has just built that exact tree as its head.

- After a passing gate on a clean tree, copy the head binary into the cache under the
  key the next run will look up. Key the cache by tree hash (`HEAD^{tree}`) rather than
  commit hash, so a merge commit with the tested tree hits.
- When a base must still be built, build it at the same time as the head (it has its own
  target folder) instead of before it: run `065233` would have been ~303 s, not 490 s.
- Weakens: nothing, if the seed is written only from a clean tree (the rule
  `record_gate_pass` in `ci-steps` already uses). A binary built in place and one built
  from `git archive` come from the same source and profile.
- Risk: a seed from a tree with uncommitted edits would poison the base. Guard: refuse
  unless `git status --porcelain` is empty before and after the build. The grammar clone
  is not in the tree hash; that hole exists today too (a cached base keeps the grammar
  it was built with).
- Verify: run the gate on master after a landing; the log must say "cached binary" and
  the verdict must sit in the master-against-itself band (x0.94-1.06).

### 4. One resident server for all of `output_cdo`; shared models in `lib_cdo`. Saves 1-3 min now, and makes items 1 and 6 fit in memory.

- `cdo_explain_digest_and_prove_give_one_table_answer` and `cdo_round2_size_ratchets`
  each start their own CDO server. Put their calls on the shared `cdo_answers` server
  (or a second `OnceLock` server shared by both, if their calls must not mix with the
  recorded answers). Saves two model builds and about 10 GB (**guess**, from ~4.7 GB per
  resident server in CLAUDE.md). No wall saving alone today (the impact tail hides it);
  it matters once item 1 or 2 is in.
- `cdo_test_selection_ratchets` (3-4 one-shot `alsem tests` calls in a row, 347 s) and
  `cdo_run_report_id...` (one-shot, 160 s) can ask the same server through MCP
  (`alsem_tests`, `alsem_prove`). After item 2 these set the phase length: **saves up to
  ~150 s then** (guess).
- `lib_cdo`: a `#[cfg(test)]` `OnceLock` for the cross-app CDO model and one for the
  single-app model, used by the tests that only read a model. **Guess:** 291 s to
  ~170-200 s. I did not check every test: some must build under a hook
  (`parallel_solve_equals_the_serial_walk_on_cdo` checks the solve while it runs) or
  compare two builds (`cdo_light_and_full_profiles...`); those keep their own build.
- Weakens: a little, and it should be stated. MCP answers equal the CLI's bytes by
  contract tests on fixtures, but on CDO the one-shot path of `tests` and `prove` would
  no longer run; keep one one-shot call per command (the 65 s routine impact already is
  one). For the lib: a shared model means a test cannot notice that a second build
  differs; tests whose point is the build must not share.
- Verify: each moved test still fails when its guard is broken (one discrimination
  proof per moved guard, per CLAUDE.md), and the per-test times in the next gate log.

### 5. Skip the speed phase when the product source is the same as the base's. Saves 7.5-19 min on test-only and script-only branches.

`speed_build` + `speed` cost 451 s (run 003611), 547 s, 1,138 s (busy), 727 s. A
branch that changes only `tests/`, `scripts/`, `docs/` or `.config/` builds the same
`alsem` as its base, so the phase compares a binary with itself. Today's tools-lane
batches (TLS-01..13, TLS-14/15 are test-only commits) look like this kind. **I did not
check each batch's diff.**

- Rule: skip only when `git diff --name-only <base> HEAD` plus the working tree's
  changes touch nothing under `src/`, `crates/`, `Cargo.toml`, `Cargo.lock`,
  `build.rs`, `rust-toolchain*`, `.cargo/`. Any path the rule does not know runs the
  phase (the safe direction, like `ci-steps`'s `affected_filter`). Print a loud line
  naming the reason, and a third verdict word in the summary ("speed: same product
  source as base").
- Weakens: nothing when the rule is right: the check exists to compare two different
  programs. It does drop the incidental "the machine can still run the speed check"
  signal.
- Risk: a path that changes the binary but is missing from the list (an `include_str!`
  of a file outside `src/`, for one). Verify: `grep -rn "include_str!\|include_bytes!\|env!(" src crates`
  and add what it finds; a small test of the rule next to `gate_scripts.rs`.

### 6. Parallel lanes again, once memory allows. Saves ~1-2 min beyond item 1.

The two-lane mode peaked at 51-58 GB (CLAUDE.md), over the 48 GiB limit. Items 2 and 4
remove two servers, and maybe two of the five impact processes: **guess** 15-20 GB
less. Then the full two-lane mode fits, and the gate after the speed phase is about
max(`output_cdo`, lane 2). With items 1, 2 and 4 that is roughly 350-400 s against
1,170 s today.

- Weakens: nothing.
- Risk: an out-of-memory kill mid-gate costs more than it saves. Verify with one
  measured run (`memory.peak`) and keep `CDO_GATE_SERIAL=1` as the fallback; consider
  having the script choose by the cgroup limit.

### 7. `lib_cdo` on more threads in serial mode. Saves ~1-1.5 min. One line.

`--test-threads=4` was chosen because lib ran beside `output_cdo`. In serial mode it
runs alone. 8 threads at 4.2 GB each is ~34 GB at worst. The tests use rayon inside, so
the gain is less than 2x: **guess** 291 s to ~200 s (the longest test alone is 137 s).
Item 4's shared models make this mostly unneeded; do whichever lands first.

- Weakens: nothing. Risk: memory if the 4.2 GB figure is stale; verify with one run.

### 8. Use the `release` `alsem` that `ci-steps all` already built for the speed phase. Saves ~1-4 min per gate.

The head's release-fast build is 186-290 s on every gate with a source change, and the
speed phase cannot start before it. `ci-steps all` has, minutes earlier, built the full
`release` `alsem` in `target/gate-release/release/`. Timing `release` against `release`
needs no head build; the base comes from item 3's seed. The wait before `speed` drops
from max(test build, 186-290 s) to the test build alone: run 003611 saves 173 s, run
012547 77 s, run 065233 ~240 s.

- Weakens: nothing in kind; `release` is closer to what ships. But the limits' noise
  bands were measured on release-fast, so master against itself must be re-measured on
  `release` before trusting the 1.20/1.30/1.10 limits.
- Risk: the gate then depends on `ci-steps all` having run on the same tree (check with
  the tree stamp; fall back to building). A base that must be built costs a full-LTO
  build (~4 min or more). Without item 3 this is a loss, so it comes after item 3.

### 9. Keep other builds off the host while the gate runs. Saves up to 18 min in the bad case. Policy, not code.

Run 052939 took 45 min against 27: load average 40 at the start of `speed`, 97 at its
end. Every phase stretched (speed 541 s against 247, lib 452 against 291, the base
build 595 against ~300). The gate is already "exclusive" among gates; this run shows
other builders still ran beside it. The orchestrator can hold new builds while a gate
is in its `speed` phase at least. The speed check's wide wall limits exist for this
case, so it also lowers the risk of a false FAIL.

### 10. `ci-steps all` (8-15 min): small, mostly guesses, I had no logs for it.

- `clippy --release` re-checks the whole crate with no incremental reuse (the script
  says "minutes" against 21 s warm for `clippy-dev`, which already turns debug
  assertions off to see the same `cfg`). GitHub CI still runs the exact release lint
  before a merge. Using `clippy-dev` in the local `all` would save **a guess of 1-3
  min**. Weakens: the local run is no longer CI's exact command; CI catches the rest.
  The owner chose the exact bar on purpose after a red CI, so this needs his yes.
- Start the gate's two builds (`cdo` test build, release-fast or nothing after item 8)
  beside `ci-steps all`'s test step, so they are warm when `cdo-gate` starts: up to
  ~200-250 s. Risk: four compilers at once (the memory note says at most 2-3 builders),
  and they must be finished before `perf_bounds`, which needs a quiet machine. **Guess;
  I would not do it before the items above.**

## What I would not change

- **The speed phase's rounds and warm-up.** The warm-up is not overhead: its model
  build and test universe are judged calls, the only ones that show lost parallelism,
  and they are 113 of the 247 s. Dropping a round saves 36 s (2% of the gate) and turns
  a median of three into a mean of two, where one noisy sample decides. Not worth it.
- **Running the two servers' calls at the same time, or caching the base's timings.**
  Both put load differences into the ratio the phase exists to judge. The module doc
  rules the first out; the second breaks the "same conditions" pairing.
- **Running anything beside the speed phase.** Same reason.
- **nextest for the gate.** One process per test would rebuild the shared fixtures; the
  script measured it slower.
- **Harness, lsp, l4** (80 s together): already shared (`CDO_SHARED`) or small.

## If all of it lands (rough, quiet host)

Builds ~30-250 s (test build only) + speed 247 s + lanes ~350-400 s = about 11-15 min
against 27-28 min; about 6-8 min for a test-only branch (item 5). Items 1, 3, 5 and 7
are script or test-harness changes and give most of it: ~27 min to ~20 min for a product
branch, less than 15 min for a test-only one.

## What I did not check

- No memory numbers for serial mode; every memory statement above is a guess to be
  measured with one run.
- Where the 200 s of a diff impact goes (item 2) is inferred from three numbers, not
  profiled.
- I read 8 of the 17 lib CDO tests, not all; and no `ci-steps all` log.

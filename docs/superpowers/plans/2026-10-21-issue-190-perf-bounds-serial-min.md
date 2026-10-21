# perf_bounds: serial timing tests judged on the minimum (#190) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `tests/perf_bounds.rs`'s magnitude gates stop failing on CI from noise alone, while a real consistent slowdown still fails.

**Architecture:** A file-wide mutex makes the timing tests run one at a time (CI runs the binary's 12 tests in parallel
on 4 vCPUs, and siblings' load is sustained), and every magnitude gate compares the MINIMUM of its samples with its
bound. A source-scan test pins that every timing test takes the lock.

**Tech Stack:** Rust, libtest, `std::sync::Mutex`.

**Spec:** `.agent/issue-190/ledger.md` `## Design` (bounded issue; hash `3ff6fdd3a57c4a0e`), probes P1-P8 above it,
register `.agent/issue-190/findings.json`.

## Global Constraints

- Only `tests/perf_bounds.rs`, `docs/OUTSTANDING.md`, `CHANGELOG.md` change. No bound value, no sample count changes.
- Never edit `.github/ scripts/ .claude/ CLAUDE.md .gitignore Cargo.toml version tree-sitter-al .agent/`.
- Format with `rustfmt tests/perf_bounds.rs` only (never `cargo fmt`). Edit with the Edit tool, never a script.
- In the worktree: `export TREE_SITTER_AL_PATH=/work/alsem/tree-sitter-al`.
- Measurements in CI shape: `taskset -c 0-3 cargo test --release --test perf_bounds -- --nocapture` (default threads,
  unfiltered). Logs go to `/work/alsem/.agent/runs/20261021-143757-e143e4/`.

## Review Focus

- A test that panics while holding the lock must not turn every later test red: `serial()` recovers from poisoning.
- An attribute between `#[test]` and `fn` (e.g. `#[ignore]`) must make the scan FAIL LOUDLY naming the line, never skip.
- The scan must not pass empty (no `release_checks` found, or zero tests found → fail).
- The scan's own string literals must not match (anchor on trimmed lines equal to `#[test]` / starting `fn `).
- No dangling intra-doc link to the deleted `median` (`[`median`]` at :233, :429; mentions at :437, :753, :878).

---

### Task 1: Lock + `min_of` + scan guard + docs

**Files:**
- Modify: `tests/perf_bounds.rs` (top level after `perf_bounds_binary_is_never_empty`; `release_checks` helpers near
  `median`/`min_of` :423-449; the ten `let m = median(samples.clone());` sites 617, 639, 669, 707, 744, 947, 1007, 1071,
  1143, 1261 and their `println!`/`assert!` text; bound docs incl. `COMPUTE_SUMMARIES_V2_BOUND` :284-338)
- Modify: `docs/OUTSTANDING.md:645-649`, `CHANGELOG.md` (`## [Unreleased]` → `### Fixed`)

**Interfaces:**
- Produces (inside `release_checks`): `static TIMING_LOCK: std::sync::Mutex<()>` and
  `fn serial() -> std::sync::MutexGuard<'static, ()>` = `TIMING_LOCK.lock().unwrap_or_else(PoisonError::into_inner)`.
- Produces (top level, every profile): `#[test] fn every_timing_test_holds_the_timing_lock()`.

- [ ] **Step 1: Write the failing scan test** (top level, outside `release_checks`, no cfg): read
  `include_str!("perf_bounds.rs")`; take the lines from the one whose trim is `mod release_checks {` to the next line
  that is exactly `}`; panic if that module is not found. For each line whose trim is exactly `#[test]`: the next line's
  trim must start with `fn ` (else panic naming the line number: "attribute between #[test] and fn"); the first
  non-blank line after the signature line (the line ending in `{`) must trim to exactly `let _serial = serial();`
  (else collect the fn name). Assert the count of `#[test]` found is >= 11 and the missing list is empty, naming every
  missing fn.
- [ ] **Step 2: Run it, verify it fails** — `cargo test --test perf_bounds every_timing_test_holds_the_timing_lock`
  (debug is fine). Expected: FAIL listing all 11 `*_within_bound` fns. Record the output in the ledger.
- [ ] **Step 3: Add `TIMING_LOCK` and `serial()`** with a doc giving the rule and why (P7's 4-core numbers, the two CI
  failures 2026-10-21, #190), and put `let _serial = serial();` as the first statement of all 11 `#[test]` fns in
  `release_checks` (`compute_all_within_bound` included).
- [ ] **Step 4: Switch the ten sites to `let m = min_of(&samples);`**, print `min=`, assert text "… min {m:?} exceeds …".
  Delete `median`. Reword the `median` references (:233, :429, :437, :753, :878) as code spans / plain words.
- [ ] **Step 5: Docs.** `min_of`'s doc: drop the hyperfine/Criterion claim; say the minimum rejects transient positive
  delays and does nothing for a sustained window (the lock handles the binary's own load). One line at the bounds:
  calibrated as 3x a median, now compared with a minimum, widening per gate 3-18% (numbers in the ledger's change 3).
  `COMPUTE_SUMMARIES_V2_BOUND`: a dated note (runs 39611462366 / 39612982728, medians 231.8 / 243.0 ms, minima 147.3 /
  151.6 ms, bound 230 ms). Leave the runner trip factor as "measured in #190's D3" until D3 runs; the conductor fills
  the number afterwards. Tick `docs/OUTSTANDING.md:645` with "2026-10-21 (#190): all timing tests hold one lock and
  judge `min_of`". CHANGELOG `### Fixed` entry, plain English.
- [ ] **Step 6: Verify green** — the scan test passes in debug AND `--release`; `rustfmt tests/perf_bounds.rs`;
  `taskset -c 0-3 cargo test --release --test perf_bounds -- --nocapture` 12+1 passed.
- [ ] **Step 7: Discrimination D1a** — delete `let _serial = serial();` from ONE test (Edit; `grep -c` shows 10) → scan
  fails naming that fn; revert (`grep -c` 11) → passes. Also insert `#[ignore]` between one `#[test]` and its `fn` →
  loud failure; revert. Record both in the ledger's proof table.
- [ ] **Step 8: D1b** — 5 CI-shape runs (log `perf-d1b-lock.log`): for `build_full_1000`, `compute_summaries_v2`,
  `incoming`, `compute_all`, every run's min ≤ limit (133.44 / 51.96 / 2.868 / 7.776 ms). Break: delete every
  `let _serial = serial();` line (Edit; `grep -c` 0), 5 runs (log `perf-d1b-nolock.log`): for EACH of the four gates the
  largest per-run min exceeds its limit; revert (`grep -c` 11). Summarise with
  `/tmp/claude-1000/-work-alsem/c8325c25-f1bf-436d-a204-90152a41120c/scratchpad/agg.py`.
- [ ] **Step 9: D2** — make `compute_summaries_v2_within_bound` call `sub.solve()` 3 times inside each timed sample
  (Edit, grep-checked), run it in release → expected PASS (~135 ms); record as the dev-machine limit; revert.
- [ ] **Step 10: Local repeat** — 10 CI-shape runs, 10/10 green (log `perf-local-10x.log`).
- [ ] **Step 11: Gate and commit** — `bash scripts/ci-steps task-affected`; then from the main checkout
  `python3 scripts/agentflow --root /work/alsem --run-id <RID> check-diff --base origin/master --head HEAD --issue 190
  --cwd /work/al-sem-issue-190-a1` must return no reason. Commit `tests/perf_bounds.rs docs/OUTSTANDING.md CHANGELOG.md`
  (named, never `-A`): `perf_bounds: run timing tests one at a time and judge the minimum (#190)`.

D3 (the runner probe commit) and D4 (5/5 CI executions) are conductor steps after the draft PR, not this task.

### Task 2: Tighten `COMPUTE_SUMMARIES_V2_BOUND` after D3 (spec amendment, ledger "Amendment after D3")

D3 (run 39625785855) showed that under the lock the runner's 3x `compute_summaries_v2` min is 179.87 ms < 230 ms.

**Files:** Modify `tests/perf_bounds.rs` (`COMPUTE_SUMMARIES_V2_BOUND` and its doc; `min_of`'s doc), `CHANGELOG.md`.

- [ ] **Step 1:** `COMPUTE_SUMMARIES_V2_BOUND` = `Duration::from_millis(150)`. Its doc: calibrated to the CI runner under
  the lock (D3: 3x min 179.87 ms, samples 179.9-186.3; implied 1x ~60 ms; dev machine 47-49 ms), so 2.5x headroom on
  the runner and a 3x regression fails there by ~20%; the scope sentence: a 3x regression fails on a runner of
  D3/D3b's speed (CPU `<D3B_CPU>`, placeholder the conductor fills after D3b), 150 ms rejects 3x only while 1x > 50 ms,
  so a ~1.2x faster runner or the dev machine may let an exact 3x pass. Replace the "measured in #190's D3" note.
- [ ] **Step 2:** `min_of`'s doc: drop "maximum-likelihood"; say the minimum rejects transient positive delays.
- [ ] **Step 3:** If any doc names other bounds' calibration, cite the synthetic bounds by constant name
  (`RUNG2_SYNTHETIC_BOUND`, `RUNG1_SCOPED_SYNTHETIC_BOUND`), not line numbers.
- [ ] **Step 4:** CHANGELOG entry: add the bound change (230 → 150 ms) with the same runner scope sentence and placeholder.
- [ ] **Step 5:** Verify: `taskset -c 0-3 cargo test --release --test perf_bounds -- --nocapture` 13 passed, 5 runs
  (log `t2-5x.log`), `compute_summaries_v2` min recorded per run; `bash scripts/ci-steps task-affected` exit 0;
  `rustfmt tests/perf_bounds.rs`; check-diff `[]`. Commit `tests/perf_bounds.rs CHANGELOG.md` and this plan file:
  `perf_bounds: tighten compute_summaries_v2 to 150 ms, calibrated on the runner under the lock (#190)`.
- [ ] **Step 6 (D3b probe, separate THROWAWAY commit on top):** in `compute_summaries_v2_within_bound` only: print the
  CPU model (first `model name` line of `/proc/cpuinfo`); time 5 samples of ONE solve, then 5 samples of THREE solves,
  binding every result until `start.elapsed()` is taken and dropping it after (same placement in both loops); print
  both sample sets, the two mins and the multiplier; then assert the 3x min against the bound with the bound's own
  message; finally `panic!("D3B-PROBE: …")` if the assert passed, so the output is always shown. Local run: record the
  numbers (dev machine 3x may pass, then the D3B-PROBE panic fires). Commit
  `THROWAWAY D3b probe (#190)`; do NOT push (the conductor pushes).

### Task 3: Back to 230 ms after D3b (ledger "Amendment 2 after D3b")

D3b (run 39635284889, AMD EPYC 7763) gave runner 1x 86.52 ms, so 150 ms is withdrawn. Head is `7627e2f0` (D3b
reverted; tree = `b2a33e54`).

**Files:** Modify `tests/perf_bounds.rs` (`COMPUTE_SUMMARIES_V2_BOUND` and its doc; the stale "bound left at 230ms"
sentence; the unreflowed `min_of` doc line), `CHANGELOG.md`, this plan.

- [ ] **Step 1:** `COMPUTE_SUMMARIES_V2_BOUND` = `Duration::from_millis(230)`. Rewrite its 2026-10-21 doc: the two CI
  failures (medians 231.8 / 243.0 ms, pre-lock); under the lock the runners measured 3x min 179.87 ms (run 39625785855)
  and 1x 86.52 / 3x 248.21 ms (run 39635284889, AMD EPYC 7763), so runners differ ~1.4x; 230 ms keeps 2.7x headroom
  over the slow runner's 1x; a 3x regression fails on a runner of the EPYC 7763's speed and PASSES on the faster runner
  and on the dev machine (D2 142-153 ms; D3b local 156 ms); a machine-independent ratio gate is the follow-up. No
  `<D3B_CPU>` placeholder may remain.
- [ ] **Step 2:** The older "Bound left at 230ms (3x ~76ms) rather than tightened …" sentence becomes clearly historical
  (e.g. "Bound left at 230ms then (3x ~76ms) …; see 2026-10-21 below"). Reflow the short `min_of` doc line.
- [ ] **Step 3:** CHANGELOG: no 150 ms change; the entry says what the change does (lock + `min_of`), the runner data,
  and that the 3x catch holds only on runners of the slower speed seen, with the follow-up.
- [ ] **Step 4:** Verify: `grep -c "D3B_CPU\|from_millis(150)" tests/perf_bounds.rs CHANGELOG.md` = 0;
  `taskset -c 0-3 cargo test --release --test perf_bounds` 13 passed; `rustfmt tests/perf_bounds.rs`;
  `bash scripts/ci-steps task-affected` exit 0; check-diff `[]`. Commit `tests/perf_bounds.rs CHANGELOG.md` and this
  plan: `perf_bounds: keep compute_summaries_v2 at 230 ms; record the runner calibration (#190)`.

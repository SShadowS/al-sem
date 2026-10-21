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

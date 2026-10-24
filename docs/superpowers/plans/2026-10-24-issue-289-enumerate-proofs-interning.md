# digest enumerate_proofs: intern candidate keys — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Restore digest's pre-F1-S1 cost on CDO by interning `enumerate_proofs`' candidate keys, with byte-identical output and F1-S1's guarantee intact.

**Architecture:** In `src/engine/l5/digest.rs` `enumerate_proofs`: a `HashMap<ProofKey, usize>` id map + `found: Vec<bool>` + `found_count`; `Node.crossed` holds `(key_id, level)`; a module-level `ProofKey` newtype with a test-counting `Hash`; `ProofOutcome.popped`; one guard test. Nothing else changes.

**Tech Stack:** Rust; cargo-nextest.

**Spec:** the `## Design` section of `.agent/issue-289/ledger.md` (bounded; converged after 3 spec rounds; acceptance A1–A5 with breaks; fallbacks F1–F3). Prototype: `/work/bench/289/fix-intern.patch` (the fix) and `/work/bench/289/fix-design-prototype.patch` (fix + `ProofKey` counter + `popped` + guard test + extras that must NOT ship).

## Global Constraints

- REBASE onto current `origin/master` (it contains #160 step 9, `6172492f`) BEFORE writing code; the fix region (~:897–1151 on the base, `enumerate_proofs` ~:931 on `6172492f`) is untouched by step 9.
- Ship only: interning + `ProofKey` + `popped` + guard test + CHANGELOG `Fixed`. No `chain` / `bloom` / `bit` extras.
- Pops, push order, `MAX_STATES` points, the `truncated` test and `out.paths` order unchanged.
- No CDO runs in this task (the conductor measures after); dev-profile builds and unit tests only.
- `rustfmt <file>` only; Edit/Write only; protected paths `.github/ scripts/ .claude/ CLAUDE.md .gitignore Cargo.toml version tree-sitter-al .agent/`.

## Review Focus

1. `found_count` must rise only on a false → true change (the cut equals `found.insert(key) && found.len() >= cap`).
2. A crossed path reuses its `(id, level)` at a seed; an uncrossed seed interns at `len`.
3. Hand-written `Hash` for `ProofKey` must agree with its `Eq`.
4. The guard's `hashes <= 4` must not count HashMap growth rehashes beyond F3's bound.
5. The two F1-S1 tests and `digest_cli::tests_rule_b::b8_*` stay green.

---

### Task 1: intern candidate keys in enumerate_proofs

**Files:**
- Modify: `src/engine/l5/digest.rs` (`enumerate_proofs`, `ProofOutcome`, new `ProofKey`; tests module)
- Modify: `CHANGELOG.md`

- [ ] **Step 1: Rebase** onto `origin/master`; confirm `enumerate_proofs` is unchanged by step 9.
- [ ] **Step 2: Write the guard test** `proof_walk_hashes_keys_per_crossing_not_per_partial_path` (Design §2: `popped == 1097`, `hashes <= 4`, `paths == [(root, a, h1)]`, not truncated); run it red on master (hashes ≈ 1,094; record).
- [ ] **Step 3: Implement** per Design §1 (from `fix-intern.patch`) + `ProofKey` + `popped`.
- [ ] **Step 4: Green** — the guard, the two F1-S1 tests, `b8`, all `l5::digest` lib tests; then `scripts/ci-steps task-affected` and `scripts/check-goldens` (no regen).
- [ ] **Step 5: Discrimination proofs** — A2 (i) raw counting, (ii) crossed → `queue`, (iii) raw + both found-checks removed; A4 interned-vs-found (`k < found.len()`); A5 (a) per-pop `HashSet` restore (record the measured hash count; assert every site changed), (b) drop-at-pop deleted (popped 1,099). Each made and reverted with Edit; record test | mutation | fail | pass.
- [ ] **Step 6: Commit** after `check-diff`.

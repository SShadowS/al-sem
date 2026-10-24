# Per-detector loose subjects — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** each of the 16 listed detectors states its own routine-independent loose subject (and, for d7/d45, a loose object) when it builds its key, so `looseFingerprint` no longer parses rootCauseKey (issue #252).

**Architecture:** A typed `LooseSubject { object_id: Option<String>, key: String }` on `Finding` (`Option`, `None` = the #247 strip path, unchanged for every other detector). `loose_fingerprint_of` hashes the subject as given (stabilised, never stripped) and, when `object_id` is set, uses it in place of the anchor's object part. `fingerprint_of`, rootCauseKey, `primaryLocation` and `affected_tables` are untouched.

**Tech Stack:** Rust; the `cli` umbrella (`tests/cli/cli_loose_fingerprint.rs`), `tests/r4/r4_cross_app_program.rs`; cargo-nextest via `scripts/ci-steps`.

**Spec:** `docs/superpowers/specs/2026-10-24-issue-252-loose-subjects-design.md` (rev 2.2; converged after 3 spec rounds). It fixes every subject, test, break, precondition, golden expectation and doc change; this plan only orders the work.

## Global Constraints

- `fingerprint` bytes unchanged everywhere; only the 4 cli-a `looseFingerprint` lines the spec names may move (d45 in `ws-d8-commit-in-tx.{all,default}.json`, d48 ×2 in `ws-txn-d48-pos.all.json`); every non-cli-a golden unchanged. A different move is a stop-and-revise (spec Pre-stated fallbacks), never a regen.
- Every listed detector sets the subject next to its `fingerprint_of` line; the other detectors keep `None`.
- Stop-and-revise rules of the spec apply (a pair that does not fire, a break that stays green, an unlisted routine-bound part).
- `rustfmt <file>` only; Edit/Write only; protected paths untouched; every new test gets a discrimination proof recorded (test | mutation | fail | pass | commit).
- Out of bounds: `digest.rs` effects/witness, `ordering_engine.rs`, `fingerprint_query.rs` (#160).

## Review Focus

1. A detector NOT in the 16 → `looseFingerprint` byte-identical to today (`None` path; existing #247 tests + the cli-a goldens of other detectors).
2. A finding with `object_id` set → the loose hash uses it, the ordinary fingerprint does not (U4 + `d7_anchor_object_switch`).
3. d39–d42 with an unresolved caller table → `?` in the key, two such findings via one callee/index share a key (stated limit; a test pins the `?`).
4. d48 direct ↔ transitive inside one object → same loose key (`d48_direct_to_transitive`).
5. CDO: the multiset of `fingerprint` values identical before/after at both scopes; per-detector distinct-loose counts reported against the spec's predictions (measurement, conductor).

---

### Task 1: the typed subject, the fingerprint change, and d39–d42, d7, d45, d48

**Files:** `src/engine/l5/finding.rs` (field), `src/engine/l5/fingerprint.rs` (`loose_fingerprint_of`, `object_part`, U-tests, the reworded #247 comments), `src/engine/gate/projection.rs` if it needs the field, every `Finding { … }` literal that must now name the field (tests get `None`), detectors `d39.rs d40.rs d41.rs d42.rs` (+ the shared `binding_source_table` helper), `d7.rs`, `d45.rs`, `d48.rs` (both builders); tests in `tests/cli/cli_loose_fingerprint.rs` and `tests/r4/r4_cross_app_program.rs` for exactly these detectors as the spec's Acceptance matrix lists them.

- [ ] Step 1: write the Task-1 tests from the spec matrix (dNN move_rename ×4, dNN_same_callee_tables_distinct ×4 on the P7 table-mismatch shape (+ the P5 generic-`Record` case for d41), dNN_callees_distinct ×4, d7 move/events/anchor_object_switch, d45_publishers_distinct, d45 dependency anchor switch, d48_move_transitive, d48_loop_objects_distinct, d48_direct_to_transitive, U4); run — each fails for its stated reason on master (or, for distinct-pair tests that already pass, state why and rely on the break).
- [ ] Step 2: implement the field, `loose_fingerprint_of`, `object_part`, and the 8 detectors' subjects.
- [ ] Step 3: tests pass; `scripts/ci-steps task-affected` green; `scripts/check-goldens` — only the d45/d48 cli-a `looseFingerprint` lines the spec names may change (run it WITHOUT regen and report the exact diff; do not regen).
- [ ] Step 4: discrimination proofs per the spec's break column, made and reverted with Edit.
- [ ] Step 5: commit (`check-diff` first): `analyze: typed loose subjects for d7, d39-d42, d45, d48 (#252)`.

### Task 2: d4, d12, d16, d18, d22, d43, d57, d59, d60, docs, and the golden regen

**Files:** detectors `d4.rs d12.rs d16.rs d18.rs d22.rs d43.rs d57.rs d59.rs d60.rs`; their tests (incl. the #247 T6 rewrite) in `tests/cli/cli_loose_fingerprint.rs`; `README.md:163-180` (the spec's Docs list); `CHANGELOG.md` (`### Changed`, spec text); the 4 cli-a golden lines.

- [ ] Step 1: write the Task-2 tests from the spec matrix; run — fail for the stated reason (or state why a distinct-pair passes on master and rely on its break).
- [ ] Step 2: implement the 9 detectors' subjects.
- [ ] Step 3: tests pass; task-affected green; `scripts/check-goldens` (no regen) shows exactly the spec's 4 lines; then `scripts/check-goldens --regen`, confirm the regenerated diff is exactly those 4 `looseFingerprint` values, and `scripts/check-goldens` green.
- [ ] Step 4: discrimination proofs; README + CHANGELOG.
- [ ] Step 5: commit (`check-diff` first): `analyze: typed loose subjects for the remaining checks (#252)` — the regenerated goldens with the triage summary in the message.

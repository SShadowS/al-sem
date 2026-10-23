# d35 unknown answer at medium severity — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** d35 reports its UNKNOWN answer (no Commit found directly or proven in the cone, coverage incomplete) at `medium` instead of `high`; the direct and via-callee arms stay `high` (issue #256).

**Architecture:** The severity becomes a member of the arm tuple in d35's existing `match (direct_commit, summary_commits)`; nothing else in d35 changes. Tests live in the inline-workspace CLI file `tests/cli/cli_d35_unknown_commit.rs` and d35's lib tests; no golden moves.

**Tech Stack:** Rust; the `cli` test umbrella; cargo-nextest via `scripts/ci-steps`.

**Spec:** `.agent/issue-256/ledger.md` `## Design` (Rule, Surfaces, Goldens, Tests (1)–(7), CHANGELOG) and `## Acceptance matrix` — converged after 5 spec-panel rounds.

## Global Constraints

- Severity strings exactly `"high"` / `"medium"`. Id, rootCauseKey, title, confidence (level + `cappedBy`), root cause, note, fix hint: byte-identical to today on every arm.
- Expected golden diff: ZERO lines. `scripts/check-goldens` (no `--regen`) must stay green; a moved golden is a discovery, never regenerated.
- No new `tests/r0-corpus/` fixture. `rustfmt <file>` only. Protected paths untouched (`.github/ scripts/ .claude/ CLAUDE.md .gitignore Cargo.toml version tree-sitter-al`; `.agent/` except reading). `TREE_SITTER_AL_PATH=/work/alsem/tree-sitter-al`.
- Every new or changed test has a discrimination proof recorded (test | mutation | fail | pass | commit).

## Review Focus

1. A DIRECT Commit in a subscriber whose cone is also unknown → stays `high`/`likely` (Tests: lib `direct_commit_with_unknown_cone_stays_high`).
2. A subscriber with no effect summary at all (summary `None`) and no direct Commit → `medium` (lib, existing `d35.rs:348`/`:369` cases updated to assert severity).
3. A proven Commit in the cone plus incomplete coverage (yes arm) → stays `high`/`possible` (existing V5 + ws-d35 goldens).
4. `--fail-on high` on an unknown-only run → exit 0; `--fail-on medium` → exit 1 (CLI).
5. Default detector selection on CentralGauge's reproduction → `bySeverity == {"medium": 2}` exactly (CLI).

---

### Task 1: d35 unknown arm → `medium`, with its tests and CHANGELOG

**Files:**
- Modify: `src/engine/l5/detectors/d35.rs` (arm tuple at ~`:100-174`; `severity: "high"` at ~`:193` becomes the tuple's value; module doc `:13`, `:15-17`; lib test `:344` flips to `medium`; new lib test)
- Modify: `tests/cli/cli_d35_unknown_commit.rs` (Tests (1)–(3), (5)–(7) of the spec; `analyze(ws, Some(D35))` helper at `:139-166` returns the whole report)
- Modify: `CHANGELOG.md` (`## [Unreleased]` → `### Changed`, the spec's text)

- [ ] **Step 1: Write / change the failing tests** — exactly the spec's §Tests (1), (2), (3) (with its pre-stated fallback), (5), (6), (7) and the lib tests from Review Focus 1–2. Assertions as the spec states them (exact `bySeverity` maps, whole confidence objects, fix-hint "partly analysed" routine names). Keep the yes-arm assertion `:237` at `high`.
- [ ] **Step 2: Run** `cargo test --test cli cli_d35_unknown_commit::` and `cargo test -p al-sem --lib detectors::d35` — expected FAIL on every unknown-arm severity assertion (`left: "high" right: "medium"`), PASS on the direct/yes assertions.
- [ ] **Step 3: Implement** — add the severity to the arm tuple: direct `"high"`, yes `"high"`, unknown `"medium"`; use it in the `Finding`. Update the module doc lines.
- [ ] **Step 4: Run** the two commands above → PASS; `scripts/ci-steps task-affected` → green; `scripts/check-goldens` → green with no file changed.
- [ ] **Step 5: Discrimination proofs**: (a) unknown arm back to `"high"` → the CLI and lib unknown tests fail (incl. both `{"medium": 2}` checks); (b) severity keyed on `summary_commits == "unknown"` instead of the arm → `direct_commit_with_unknown_cone_stays_high` fails; (c) the `--fail-on` test with the arm back to `high` → `--fail-on high` exits 1. Break and revert with Edit; record each row.
- [ ] **Step 6: CHANGELOG** entry (spec §CHANGELOG text, plain English).
- [ ] **Step 7: Commit** after `check-diff` (no reasons): `d35: report an unknown commit answer at medium severity (#256)` + Co-Authored-By trailer. Files: the three above.

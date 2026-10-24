# d35: name the subscriber's own unresolved call sites — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A d35 unknown answer whose gap is in the subscriber's own body names those call sites (callee text and line) and lists only OTHER routines under "partly analysed".

**Architecture:** A message-only change in `src/engine/l5/detectors/d35.rs::coverage_reasons`, reading data already on `DetectorContext` (`uncertainty_edges_by_from`, `call_site_by_id`) with the existing `l4::db_effect_solver::is_callsite_local_kind` filter. No l4 or cone change.

**Tech Stack:** Rust; cargo-nextest; `tests/cli` umbrella.

**Spec:** the `## Design` section of `.agent/issue-265/ledger.md` (bounded issue; converged after 3 spec rounds — read it fully; it holds the acceptance matrix A1–A7 with each test's break).

## Global Constraints

- Change no l4/cone code and no shared receiver typing; `uncertainty_edges_by_from` stays primary-only.
- Early returns ("no effect summary", "coverage unknown") are byte-identical to today.
- Ids, fingerprints, confidence and severity of every finding unchanged.
- `SHOWN_TARGETS = 3` for both lists; `+N more` with the exact remainder.
- Lines print as `start_line + 1`.
- Callee text: whitespace runs collapsed to one space; F1 (60-char cut, char-boundary safe, `…`, keep ` (line n)`) only if real code needs it — not by default.
- `rustfmt <file>` only; Edit/Write only; protected paths `.github/ scripts/ .claude/ CLAUDE.md .gitignore Cargo.toml version tree-sitter-al .agent/`.

## Review Focus

1. A subscriber with BOTH a located and an unlocated own gap must keep itself under "partly analysed" (A6b).
2. A same-id opaque / parse-incomplete row must keep self (A6c); `routine_by_id` keeps only the last row, so compute the set ONCE per run in one pass over `resolved.workspace.routines` — in cross-app mode that collection already includes the dependency rows (detector_context.rs:1286-1306), no second collection exists.
3. Distinct sites with equal callee text both print; same-line sites order by column (A4).
4. A dependency subscriber at `--scope all` keeps today's text (A6d, stated limit) — its preconditions must be asserted so it cannot pass vacuously.
5. The `{reasons}` string feeds rootCause, the evidence note and fixHint — assert all three surfaces where the matrix says so.

---

### Task 1: own unresolved sites in the d35 unknown message

**Files:**
- Modify: `src/engine/l5/detectors/d35.rs` (`coverage_reasons` ~:241 and its single caller ~:151; module doc; `SHOWN_TARGETS` doc; unit tests module)
- Modify: `tests/cli/cli_d35_unknown_commit.rs` (repin A1 ~:552; new A3, A6d; a scope argument for the analyze helper ~:154)
- Modify: `CHANGELOG.md` (`## [Unreleased]` → `### Changed`)

**Interfaces:**
- Consumes: `DetectorContext::{uncertainty_edges_by_from, call_site_by_id, routine_by_id}`, `l4::db_effect_solver::is_callsite_local_kind`, `engine::l5::test_support::{minimal_ctx, call_site, ...}`.
- Produces: `fn coverage_reasons(summary: Option<&FullRoutineSummary>, routine: &ModelRoutine, incomplete_ids: &BTreeSet<..>, ctx: &DetectorContext) -> String` (exact parameter shape is the implementer's call; the per-run incomplete-id set is computed once per detector run, not per subscriber).

- [ ] **Step 1: Write the failing tests** — exactly the acceptance matrix rows A1 (repin), A3, A4, A5, A6, A6b, A6c, A6d, with the assertions and fixtures the matrix states (exact expected strings).
- [ ] **Step 2: Run them; record the red output** (`cargo test --test cli cli_d35_unknown_commit::` and `cargo test -p al-sem --lib detectors::d35`). A6, A6c(?), A6d and A2/A7 controls may already pass — say which and why.
- [ ] **Step 3: Implement** per Design §1–§5.
- [ ] **Step 4: Green** — the same commands, then `scripts/ci-steps task-affected`.
- [ ] **Step 5: Discrimination proofs** — every break the matrix names, made with Edit and reverted with Edit; record test | mutation | fail output | pass output.
- [ ] **Step 6: Commit** after `check-diff`.

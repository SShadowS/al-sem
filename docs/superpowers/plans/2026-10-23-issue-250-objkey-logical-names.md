# ObjKey::Name holds the logical name — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** every source `ObjKey::Name` is built from the folded LOGICAL name (one helper), `model_key` stops unescaping, so a dependency object whose name holds `""` gets the same id from the graph and the projection (issue #250).

**Architecture:** One helper `ObjKey::from_source(number: Option<i64>, raw: &str) -> ObjKey` in `src/program/node.rs`, used at every source construction and comparison site; `ObjKey::model_key` returns `object_key(None, n)` with no unescape; the two table-extension lookups read the node's name instead of the key text. Graph object names and the name indexes stay raw (resolution is out of scope: discovery D-A).

**Tech Stack:** Rust; cargo-nextest via `scripts/ci-steps`.

**Spec:** `docs/superpowers/specs/2026-10-23-issue-250-objkey-logical-names-design.md` (rev 3.1, converged after 4 spec rounds) — Design D1/D2, Self-consistency table, Acceptance matrix (T1, T2, T3, T3b, unit vectors), Failure shapes, Fallbacks, Measurement plan.

## Global Constraints

- Notation: AL spelling, then "(logical …)". A logical name is the AL inner text with `""` → `"`.
- Never unescape a value that is already logical (ABI names, `from_abi` keys): T1/T2 use `A""B` (logical) precisely to catch a double unescape.
- No golden and no CDO number may move (P3: population 0). A moved one is a bug, never re-minted (spec Fallbacks).
- `rustfmt <file>` only; Edit/Write only; protected paths untouched; every new test gets a discrimination proof recorded.
- Out of bounds: `digest.rs` effects/witness, `ordering_engine.rs`, `fingerprint_query.rs` (#160).

## Review Focus

1. A source numberless object whose name holds no `"` → key and every id byte-identical to today (unit vector `from_source(None, "IFoo") == Name("ifoo")`; the #83 tests).
2. A numbered source object → `Id(n)` regardless of name (unit vector).
3. An ABI name that is already logical and holds `""` → its model id keeps both quotes (`@a%22%22b`), not one (T1, D2 vector).
4. A table extension of a numberless `""` table still applies (T3, T3b).
5. `rg -n 'ObjKey::Name\(' src` outside `#[cfg(test)]` shows only the helpers in `node.rs` and read-only match arms (site check in the ledger).

---

### Task 1: the key helper, `model_key`, every site, and the tests

**Files:** `src/program/node.rs` (helper, `model_key`, docs, unit vectors); the 8 construction sites and 3 comparison sites the spec's D1 lists with file:line (`node_extract.rs`, `lsp/snapshot.rs`, `impact_graph/facts.rs`, `resolve/decl_surface.rs`, `resolve/stub.rs`, `resolve/semantic_golden.rs` (construction + `:1145-1152` lookup), `model/workspace.rs`, `resolve/full.rs`, `resolve/differential.rs:466-469`, `resolve/applicability.rs:286-288`); docs `body/scope.rs:83-106`, `model/mod.rs:34-35`, `graphify_export.rs:717-721, 1565-1567` and its restated test; `tests/cli/cli_numberless_object_ids.rs` (T1, T2); unit tests T3 (`applicability.rs`), T3b (`semantic_golden.rs`); `CHANGELOG.md` (`### Fixed`, the spec's Changelog text).

- [ ] **Step 1:** write T1, T2, T3, T3b and the `node.rs` unit vectors exactly as the spec's Acceptance matrix states them; run them — T1, T3, T3b and the vectors fail for the stated reason; T2 (the control) PASSES on master (if it does not, apply the spec's fallback and record it).
- [ ] **Step 2:** implement D1 and D2 (helper, all sites, docs, graphify test restated).
- [ ] **Step 3:** run the tests → pass; `scripts/ci-steps task-affected` green; `scripts/check-goldens` green with zero files changed; the `rg` site check output recorded.
- [ ] **Step 4:** discrimination proofs, each break/revert with Edit: T1 (unescape back in `model_key`), T2 (key from `fold(raw)` with D2 kept), T3 and T3b (return the key text in the `Name` arm), the unit vectors.
- [ ] **Step 5:** CDO measurement per the spec's Measurement plan (before = master `aldump --program-call-graph-stats` + `--dependency-bodies-stats` on the pinned CDO, after = branch; byte-identical) — through the supervisor as a `bench` run (the conductor runs it).
- [ ] **Step 6:** commit (`check-diff` first): `program graph: ObjKey::Name holds the folded logical name (#250)` + trailer.

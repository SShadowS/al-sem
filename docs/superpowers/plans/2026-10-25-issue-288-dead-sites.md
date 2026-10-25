# #288 dead sites: no coverage gap and no cone edge from code that never runs — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A call or operation site that provably never runs adds no coverage gap and no cone edge, as it already adds no direct fact. A site counts as dead only where its row's `unreachable` marks are trusted.

**Architecture:** A fail-closed per-row trust flag `ModelRoutine.dead_marks_trusted`, set at assembly (`project_ir`: no preprocessor directive in the routine; every bare `Error(...)` provably takes the Text overload, with names looked up in AL scope order and compared with AL identifier equality) and narrowed after resolution (`attach_program_calls_with`: every bare `Error(...)` binds to the builtin). One dead-site set `capability_cone::DeadSites` over a model's rows; the coverage fold (`callsite_local_coverage_reasons`) and the cone's typed-edge graph (`build_typed_edge_graph`) skip what it holds, and d35's own-site naming follows the fold.

**Tech Stack:** Rust; cargo-nextest.

**Spec:** the `## Design` section (revision 5) of `.agent/issue-288/ledger.md` (bounded; pre-ratified after 5 spec rounds). Prototype for reference only: `/work/bench/288/rev5/proto-rev5.diff`. Acceptance tests: `src/engine/l5/detector_context_288_tests.rs` and d35's `unreachable_own_site_is_not_named` / `untrusted_unreachable_own_site_is_named`, red on the base for the reasons recorded in the ledger's timeline.

## Global Constraints

- Two carried requirements on top of the prototype: (1) every identifier comparison in the gate uses AL identifier equality (`al_syntax::IdentifierFoldExt::eq_fold_identifier`), not `eq_ignore_ascii_case`; G11u/G12u pin the non-ASCII case. (2) `DeadSites::of` gets a `perf_trace` span (`cone` / `dead_sites`), and its cost is measured on CDO and recorded in the ledger.
- No schema, id or wire change: the new field is not serialized and the L2 features do not change. **No golden may move**, the l2 golden included; if one does, stop.
- `rustfmt <file>` only; Edit/Write only; protected paths `.github/ scripts/ .claude/ CLAUDE.md .gitignore Cargo.toml version tree-sitter-al .agent/` (the ledger is edited, never staged).
- One cargo process at a time, `CARGO_BUILD_JOBS=4`, `TREE_SITTER_AL_PATH=/work/alsem/tree-sitter-al`.

## Review Focus

1. The gate fails closed everywhere: the field defaults to `false`; a range that does not index the source is not clean; a bare `Error` site with no builtin edge, or with any other edge at the same `(row, site)`, untrusts the row.
2. Scope order of the Text proof: params, then the named return (`return_name` / `return_type`, which `variables` does not list), then locals, then globals; a global proof also needs the whole object directive-free (scanned once per object).
3. `DeadSites`: a pair is dead only if every row of the id that holds the site is trusted and marks it `unreachable`; rows lacking the site do not vote; `None` is never dead.
4. `build_typed_edge_graph` drops a dead edge before the `object-run-unresolved` check (so it is no unresolved source) and before Tarjan; the site is `callsite_id`, else `operation_id` (implicit triggers).
5. d35: a trusted dead edge is neither named nor counted missing; an untrusted row names its dead-marked site.

---

### Task 1: the trust gate (`ModelRoutine.dead_marks_trusted`)

**Files:**
- Modify: `src/program/model/workspace.rs` (field, `has_preproc_directive`, `source_clean`, `is_bare_error_call`, `source_dead_mark_trust`, `settle_dead_mark_trust`, the call in `project_ir`)
- Modify: `src/program/model/program_calls.rs` (`attach_program_calls_with` calls `settle_dead_mark_trust`)
- Modify: the 13 `ModelRoutine` struct literals (tests, `abi_rows.rs`) — `dead_marks_trusted: false`

**Interfaces:**
- `pub(crate) fn has_preproc_directive(text: &str) -> bool`
- `pub(crate) fn source_clean(source: &str, range: &Range<usize>) -> bool`
- `pub(crate) fn settle_dead_mark_trust(m: &mut Model)`

- [ ] **Step 1:** Implement per Design Changes 1–2, with `eq_fold_identifier` for every name and keyword comparison.
- [ ] **Step 2:** Green: G2, G10's `source_clean` half; the gate's production tests go green with Task 2.
- [ ] **Step 3:** `scripts/ci-steps task-affected`; check-diff; commit.

### Task 2: dead sites in the coverage fold, the cone and d35

**Files:**
- Modify: `src/engine/l4/capability_cone.rs` (`DeadSites`, `callsite_local_coverage_reasons`, `build_typed_edge_graph`, `compose_cone_over_graph`, `build_cross_app_base_with`, `project_r3a3`, `build_r3a3_source_only_base_over`, `compute_r3a3_real_matrix`, the two existing test callers; docs)
- Modify: `src/engine/l5/detector_context.rs` (`det_cone_inputs_over`)
- Modify: `src/engine/l5/detectors/d35.rs` (`coverage_reasons`; module doc)
- Modify: `CHANGELOG.md` (`## [Unreleased]` / `Fixed`)

**Interfaces:**
- `pub(crate) struct DeadSites<'a>`; `DeadSites::of(&'a [ModelRoutine]) -> Self`; `contains(&self, from: &str, site: Option<&str>) -> bool`
- `callsite_local_coverage_reasons(edges, dead: &DeadSites<'_>)`; `build_typed_edge_graph(typed, nodes, dead: &DeadSites<'_>)`

- [ ] **Step 1:** Implement per Design Changes 3–6; `DeadSites::of` opens a `perf_trace` span.
- [ ] **Step 2:** Green: every test in `detector_context_288_tests.rs` and both d35 tests; the whole lib suite.
- [ ] **Step 3:** Discrimination proofs, one per matrix row (P1–P4, C1–C6, M1–M7, G1–G12 incl. G11u/G12u, D1, D1'), each made and reverted with Edit; rows into the ledger's `## Discrimination proofs`; logs under `/work/bench/288/a2/proofs/`.
- [ ] **Step 4:** `scripts/check-goldens` (no regen): nothing moves. Measure the `dead_sites` span on CDO; record.
- [ ] **Step 5:** `scripts/ci-steps task-affected`; check-diff; commit.

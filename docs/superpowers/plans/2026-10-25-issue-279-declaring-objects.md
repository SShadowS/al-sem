# Declaring Objects for Every Routine-Scoped Reader (#279) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Every reader of "this routine's object", and the `[HandlerFunctions]` lookup, reads the routine's own declaring row (`DeclaringObjects::of`, #241). An `#if`-split object then no longer gives one arm's name, subtype, SourceTable, InherentCommitBehavior, Editable or SingleInstance to the other arm's routines.

**Architecture:** A mechanical swap at a closed list of sites: each last-wins id -> object map that answers a routine-scoped question becomes `DeclaringObjects`. `DetectorContext.objects_by_id` is replaced by `declaring`, so a detector cannot ask the id map for a routine's object. `FingerprintIndex::object` is renamed `object_row_by_id` (object-scoped reads only). No data-model, id, schema or fingerprint change.

**Tech Stack:** Rust; al-sem program model, policy engine, L5 detectors, HTML report.

**Spec:** `## Design` (revision 2.2) and the inventory under `## Assumption probes` in `.agent/issue-279/ledger.md`. Reviewed prototype: `/work/bench/279/rev2/proto.diff` (tree `/work/bench/279/proto`, cut from 3f5030f9).

## Global Constraints

- Edit with Edit/Write only; format with `rustfmt <file>`, never `cargo fmt`.
- Protected paths, never touched: `.github/ scripts/ .claude/ CLAUDE.md .gitignore Cargo.toml version tree-sitter-al .agent/` (the ledger is edited, never staged).
- Export `TREE_SITTER_AL_PATH=/work/alsem/tree-sitter-al`; one cargo process at a time, `CARGO_BUILD_JOBS=4`.
- No golden may move. A move is a bug in the swap: stop and triage (Design fallback (a)).
- Every new test gets a single-site discrimination proof (revert only the site it pins), recorded in the ledger.
- Per-task gate: `scripts/ci-steps task-affected`; before each commit, `agentflow check-diff`.

## Review Focus

1. `classification_per_id` must still pick the LAST classified row's copy (R13b): the per-row lookup reads the routine's own row.
2. d57 and d60 change meaning from "any row of the id" to "the routine's own row"; their fallback note says "multi-row ids whose anchors contain no routine start".
3. `test_coverage`'s `[HandlerFunctions]` key compares the declaring row by address; a routine with no row keeps `None`.
4. `pub_label` falls back to the object row only when the event has no publisher routine (table events).

---

### Task 1: The swap and its lib tests (Design changes 1-12)

**Files:**
- Modify: `src/program/model/declaring.rs` (`census_tally`; `Vec` note)
- Modify: `src/engine/l5/fingerprint.rs`, `src/engine/gate/projection.rs`, `src/engine/gate/run.rs` (rename)
- Modify: `src/engine/l5/detector_context.rs` (`declaring` field, census bucket `context.declaring`, module doc)
- Modify: `src/engine/l5/detectors/{d2,d13,d14,d17,d46,d50,d57,d60,mod}.rs`
- Modify: `src/engine/gate/policy/{predicate_fields,policy_engine}.rs`
- Modify: `src/engine/root_classification.rs`, `src/engine/l5/test_coverage.rs`, `src/engine/return_summary.rs`, `src/program/model/workspace.rs`, `src/engine/gate/format_html.rs`
- Modify: `tests/cli/cli_batch_census.rs` (R14), `tests/r4/r4_differential.rs` (comments)
- Tests: `d50::tests::cap4_reads_the_declaring_arms_icb` (R11), `return_summary::tests::bulk_icb_merge_reads_the_declaring_arm` (R12), `root_classification::tests::split_arms_classify_against_their_own_row` (R13a), `root_classification::tests::per_id_classification_is_the_last_classified_rows` (R13b)

**Interfaces:**
- `DetectorContext<'v>.declaring: DeclaringObjects<'v>` replaces `objects_by_id`.
- `FieldIndexes<'a>.declaring: DeclaringObjects<'a>` replaces `objects_by_id`.
- `FingerprintIndex::object_row_by_id(&self, id) -> Option<&ModelObject>` (object-scoped only).
- `classify_roots(&ModelEntities)` and `classification_per_id(roots, routines, objects)` keep their signatures; the private `ast_classification` takes the routine's resolved object.

- [ ] Step 1: Red on base (tests and the d50 interface skeleton only): R11, R12, R13a, R13b, R14 fail for their stated reason; logs under `/work/bench/279/impl/`.
- [ ] Step 2: Implement changes 1-11; remove the d50 skeleton helper.
- [ ] Step 3: R11-R14 green; `scripts/check-goldens` green with no golden moved.
- [ ] Step 4: Single-site proofs for R11, R12, R13a, R13b (two breaks), R14.
- [ ] Step 5: `scripts/ci-steps task-affected`; `check-diff`; commit naming #279.

### Task 2: CLI acceptance tests and CHANGELOG (R1-R10)

**Files:**
- Create: `tests/cli/cli_split_object_readers.rs` (tempdir fixture from `/work/bench/279/rev2/fixA`, both arm orders); register in `tests/cli/main.rs`
- Modify: `CHANGELOG.md` (`Fixed`)

**Interfaces:**
- Consumes: Task 1's swap through `CARGO_BIN_EXE_alsem` (`policy check`, `analyze` JSON and HTML, `digest --tests`, `query effects`).

- [ ] Step 1: Red on base: each of R1-R10 fails for its stated reason in at least one arm order (logged in Step 1 of Task 1's base run).
- [ ] Step 2: Green on Task 1's head.
- [ ] Step 3: Single-site proofs: revert only the site each test pins (policy `object_of`; `ast_classification`'s lookup in `classify_roots`; d14 test-name check and message; Test-subtype check; `workspace.rs resolve`; d60 membership; d57 membership; `page_may_modify`; `routine_label`/`pub_label`; the handler key).
- [ ] Step 4: CHANGELOG `Fixed` entry.
- [ ] Step 5: `scripts/ci-steps task-affected`; `check-diff`; commit naming #279.
- [ ] Step 6: CDO re-measure on the final head (both scopes, opt-in d17/d46/d50, fingerprint): byte-identical.

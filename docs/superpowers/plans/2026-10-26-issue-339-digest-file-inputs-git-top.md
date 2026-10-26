# Digest file inputs relative to the git top (#339) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `--file` and `--changed a.al,…` read paths the way `--diff` reads them since #77. A path is relative to the git top level, or to the workspace, or to `--diff-root`. A path that names a workspace file both ways is reported as `file-ambiguous`.

**Architecture:**
- A neutral `match_path` (returning `PathMatch`) is extracted from `diff_key`. Both the file step and the diff step use it.
- Scheme ids (`ws:`, `app:`) keep their one reading.
- The CLI decides the path root when there are file inputs too (`reads_paths()`), and the lone-`--diff-root` check moves with it.
- The new kind `file-ambiguous` joins the three projections, under the unreleased schema 1.8.0.

**Tech Stack:** Rust (`al-sem`), nextest CLI tests.

**Spec:** the `## Design` and `## Acceptance matrix` sections in `.agent/issue-339/ledger.md`. It was ratified in 3 spec rounds; the register is `.agent/issue-339/findings.json` (V1–V17).

## Global Constraints

- Diff-path behaviour does not change: the existing #77, #111 and #353 tests stay green.
- Routine selectors do not change. Exit codes do not change.
- The schema stays at `1.8.0`; its contract line gains #339.
- No golden moves (P5). If one does, stop and report it.
- `rustfmt <file>` only.
- Protected paths are never touched: `.github/ scripts/ .claude/ CLAUDE.md .gitignore Cargo.toml tree-sitter-al .agent/`.

## Review Focus

1. A `ws:` candidate copied from a `file-ambiguous` diagnostic picks exactly one file, under every root (A2, A3a).
2. The mixed run `--diff --diff-root R --file src/x.al` is now `file-unmatched` (A3a). This is the stated behaviour change.
3. A routine-only `--diff-root` still exits 3 (A3d).

---

### Task 1: file inputs read like diff paths

**Files:**
- `src/engine/l5/digest_cli.rs`:
  - `match_path` and `PathMatch`;
  - `diff_key` mapped onto them;
  - the file step;
  - `FileAmbiguous` with its `kind`, JSON and human arms;
  - the 1.8.0 contract line;
  - the docs named in the Design's Docs list;
  - the #340 unit test case.
- `src/bin/alsem.rs`:
  - `DigestInputs::reads_paths`;
  - `pin_diff_root` and `run_digest`'s root decision;
  - the lone-`--diff-root` check and its message;
  - the `--file`, `--changed` and `--diff-root` help.
- `src/engine/l5/prove.rs`: a one-line doc note at the `workspace_files: &[]` call.
- `tests/cli/cli_digest_changed_spans.rs`: A1–A3 and the `lone_diff_root_is_exit_3` update.
- Batch tests for A4, wherever the #77 batch and worker tests live.
- `CHANGELOG.md` `[Unreleased]`: one Fixed entry, one Changed entry.

**Interfaces:**
- Produces:
  - `enum PathMatch { One(String), Ambiguous(Vec<String>), None }`;
  - `fn match_path(spans: &RoutineSpanIndex, files: &FileKeys<'_>, root: &DiffRoot, path: &str) -> PathMatch`;
  - `ChangedRootsDiagnostic::FileAmbiguous { file: String, candidates: Vec<String> }`, kind `"file-ambiguous"`;
  - `DigestInputs::reads_paths(&self) -> bool`.

- [ ] **Step 1: Write the failing tests**, A1–A5, exactly as the matrix states them. Reviewer notes:
  - Write the A2 case-distinct list literally: `["ws:Cloud/src/posting.al", "ws:src/Posting.al", "ws:src/posting.al"]`.
  - Build `src/Posting.al` with `posting_copy` and a third codeunit id.
  - Skip that case with the existing `case_sensitive` helper.
  - A3(a)'s mixed regression exits 0.
- [ ] **Step 2: Run them and record the red.**
  - Cases that pin unchanged behaviour may already pass: A3(b), A3(f), and the A3(d) routine case. Record which.
  - The unit test does not compile yet, because the `FileAmbiguous` variant is missing.
- [ ] **Step 3: Implement** per the Design.
- [ ] **Step 4: Discrimination proofs.** Each one: make the break with Edit, check it applied, run, revert with Edit.

  | Break | Test that must fail |
  |---|---|
  | `reads_paths` returns `has_diff()` | A1 |
  | the file step uses `normalize_input` only | A1 |
  | `Ambiguous` → the first reading | A2 |
  | the scheme-prefixed input gets `match_path` | A2 and A3(a), `ws:` cases |
  | the lone check stays `!has_diff` | A3(a) and A3(c) |
  | the batch path keeps `has_diff()` | A4 |
  | `FileAmbiguous`'s human arm skips `escape_controls` | A5 |
- [ ] **Step 5: Gate and commit.**
  - Run `scripts/ci-steps task-affected`.
  - Run `check-diff`.
  - Commit the named files.

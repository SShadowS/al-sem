# Digest non-AL diff files (#353) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A diff path that does not look like an AL file gets its own changed-roots kind, `diff-file-not-al`. `diff-file-unmatched` and `diff-file-deleted` then mean AL-looking paths only. The digest schema version becomes 1.8.0.

**Architecture:** Two guards in `resolve_changed_roots`'s diff branch, both calling `source_text::has_al_extension`:
1. at the top of the `parsed.files` loop;
2. in the `Only in` loop, with a folder-prefix check that keeps today's handling.

The new `DiffFileNotAl` variant goes through the three exhaustive projections. The schema bump updates the goldens.

**Tech Stack:** Rust (`al-sem`), nextest CLI tests, cli-b and gate goldens.

**Spec:** `## Design` and `## Acceptance matrix` in `.agent/issue-353/ledger.md`. It was ratified in 4 spec rounds; the register is `.agent/issue-353/findings.json`, entries T1–T15.

## Global Constraints

- `diff_key` is unchanged. Roots, entries and effects of AL records are unchanged. Exit codes are unchanged.
- `--file`/`--changed-files` (`FileUnmatched`) are unchanged.
- `schemaVersion` is `1.8.0`, with a contract line at `digest_cli.rs` around `:1786`.
- `rustfmt <file>` only. Protected paths are never touched: `.github/ scripts/ .claude/ CLAUDE.md .gitignore Cargo.toml tree-sitter-al .agent/`.

## Review Focus

1. An `Only in` folder holding workspace files stays `diff-file-unmatched` (A3 c).
2. A rename from AL to non-AL reports `diff-file-deleted` on the old path (A3 d).
3. A mixed diff's entries equal those of the AL-only run (A1).

---

### Task 1: diff-file-not-al

**Files:**
- `src/engine/l5/digest_cli.rs`:
  - the variant, plus `kind()`, the JSON and human arms;
  - the two guards;
  - the schema version and contract line;
  - the #340 unit test `changed_roots_human_lines` (12 lines).
- `tests/cli/cli_digest_changed_spans.rs`: the new tests for A1–A4, plus the `:1862` assert and comment.
- `tests/cli/cli_digest_query_dependencies.rs:175` and `tests/cli/cli_digest_temp_state.rs:277-278` (schema literal).
- Regenerate the 22 goldens listed in P4.
- `CHANGELOG.md` `[Unreleased]`: one `### Changed` entry and one `### Fixed` entry.

**Interfaces:**
- Produces `ChangedRootsDiagnostic::DiffFileNotAl { file: String }`, with kind `"diff-file-not-al"`.
- Consumes `crate::source_text::has_al_extension(&Path) -> bool`.

- [ ] **Step 1: write the failing tests.** Write A1–A4 exactly as the matrix states them. Each test has an exact diagnostics list, and A1 also asserts the human output.
- [ ] **Step 2: run them and record the red.** The A1/A2/A3/A4 CLI cases fail because the old kind appears. A3 (c) and (e) may already pass, because they pin unchanged behaviour; record that. The unit test fails to compile on the missing variant.
- [ ] **Step 3: implement** the Design.
  - Guard 2 builds its readings as `diff_readings` does (`:329-334`), BEFORE that function's filter (`:335-340`), and then checks for a `<reading>/` prefix among the workspace keys.
  - A deletion's `file` is the record's path; an AL → non-AL rename uses `old_path`.
- [ ] **Step 4: goldens.**
  - Run `scripts/check-goldens` and expect only the `schemaVersion` line to move, in the 22 goldens. Report the exact list.
  - The conductor writes the triage receipt `.agent/golden-triage.md` after an independent triage. Do not regenerate until it exists. Then run `scripts/check-goldens --regen`. `git diff --stat` must show only those files, 1 line each. Run `scripts/check-goldens` again: green.
- [ ] **Step 5: discrimination proofs.** For each, make the break with Edit, check that it applied, run, revert with Edit:
  - guard 1 made always-AL → A1 fails;
  - case-sensitive extension → A2 fails;
  - skip the check on deletions → A3 (a) fails;
  - skip it in the `Only in` loop → A3 (b) fails;
  - drop the folder-prefix check → A3 (c) fails;
  - the rename uses the new path → A3 (d) fails.
- [ ] **Step 6: verify and commit.** Run `scripts/ci-steps task-affected`, then `check-diff`, then commit the named files.

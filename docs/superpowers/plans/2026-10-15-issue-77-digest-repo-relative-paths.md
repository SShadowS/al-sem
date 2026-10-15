# Issue #77 — `digest` accepts paths relative to the enclosing repository — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `alsem digest --diff`, `--file` and the `--changed` file lists also resolve a path written
relative to a directory that contains the workspace. The workspace's repository prefixes are found
automatically, or given with `--diff-prefix <path>`. A path that names a workspace file as written
always wins.

**Architecture:**
- One helper, `resolve_changed_path`, in `src/engine/l5/digest_cli.rs` gives the resolved key for
  every path input, the #72 `poisoned` set included.
- `workspace_repo_prefixes(&Path) -> Vec<String>` discovers the prefixes.
- `normalize_diff_prefix(&str) -> Result<String, String>` validates the flag.
- `ChangedInput` gains `repo_prefixes` and `workspace_files`, and derives `Default`.
- `run_digest_pipeline` gains `diff_prefix: Option<&str>`.

**Tech Stack:** Rust; `tests/cli` umbrella (`CARGO_BIN_EXE_alsem`).

**Spec:** the `## Design` in `.agent/issue-77/ledger.md` (converged in spec round 5). Its rules, the
tests T1, T1b, T2, T3, T3b, T4, T5, T6, T7, T8 and the unit tests, and its CHANGELOG text are the
spec; this plan does not restate them.

## Global Constraints

- `rustfmt <file>` only. No `#[allow(`. Edit and Write tools only.
- Never touch `.github/ scripts/ .claude/ CLAUDE.md .gitignore Cargo.toml tree-sitter-al .agent/`.
- No golden may move. A path that resolves today must resolve the same way (exact first).
- Every exit-2 CLI test parses stdout as a digest document first.

## Review Focus

1. One resolved key serves both the lookup and the `poisoned` set (T6, including the collision
   control).
2. The workspace-file set is the analysis target's discovered `ws:` list (the model's `NestedApps`
   mode) OR has spans. The unit test checks span files against the discovered list ALONE.
3. Canonicalisation, with the `absolute` fallback, and `strip_prefix` on the same path.
   Windows forms: `\\?\C:\`, `\\?\UNC\`.
4. `--diff-prefix` validation: empty, rooted, `..`.
5. All 7 `run_digest_pipeline` callers are updated, and the TS-parity digest differential stays
   green.

### Task 1: repository-relative path resolution for `digest`

**Files:**
- `src/engine/l5/digest_cli.rs`;
- `src/engine/l5/prove.rs:905` (`..Default::default()`);
- `src/bin/alsem.rs` (the `--diff-prefix` arg on `DigestCli`, passed through at `:1097`);
- the 6 test callers of `run_digest_pipeline`;
- a new `tests/cli/cli_digest_repo_prefix.rs`, plus its `mod` line in `tests/cli/main.rs`;
- `CHANGELOG.md`.

- [ ] **Red.** Write T1, T1b, T2, T3, T3b, T4, T5, T6, T7 and T8 and the unit tests. Run them and
  record each failing line. T4, T5's unmatched half and T3b's exact-match half pass on base by
  design; record that.
- [ ] **Implement** the Design: `normalize_diff_prefix`, `workspace_repo_prefixes`,
  `resolve_changed_path`, the plumbing, and the CLI flag.
- [ ] **Green:** `cargo test --test cli`, `cargo test -p al-sem --lib`, and `cargo clippy
  --all-targets -- -D warnings`.
- [ ] **Discrimination proofs.** For every test, the mutation the Design names; record each one.
- [ ] **CHANGELOG** `### Fixed` with the Design's text. Then `check-diff`, then commit.

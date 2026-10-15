# Issue #76 — digest accepts any spelling of a workspace-relative path — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `alsem digest --file` (and `--diff` paths) match a file whatever `.`, empty-segment or `\`
spelling names it, without ever matching a rooted path or cancelling `..`.

**Architecture:** One pure function, `normalize_input` in `src/engine/l5/digest_cli.rs`, is the only
place digest compares paths. It gains a narrow lexical clean-up (drop `.` segments and non-leading
empty segments; keep the raw leading empty run, at most 2), between the existing scheme strip and
lowercasing.

**Tech Stack:** Rust; `tests/cli` umbrella (`CARGO_BIN_EXE_alsem`).

**Spec:** the `## Design` and tests T1, T1n, T2, T3, T4 in `.agent/issue-76/ledger.md` (spec panel
converged in round 5).

## Global Constraints

- Only `src/engine/l5/digest_cli.rs`, `tests/cli/cli_digest_changed_spans.rs` and `CHANGELOG.md` change.
- No `#[allow(`; `rustfmt <file>` only; Edit/Write tools only.
- Never touch `.github/ scripts/ .claude/ CLAUDE.md .gitignore Cargo.toml tree-sitter-al .agent/`.
- No golden may move (span keys are already clean).

## Review Focus

1. A path that already matched (`src/posting.al`, `SRC/Posting.al`, `src\posting.al`) still matches.
2. A rooted path (`/src/…`, `//srv/…`) and `..` still do not match.
3. The diff path and the poisoned-file set use the same normalisation as `--file`.

### Task 1: lexical clean-up in `normalize_input`, with tests T1, T1n, T2, T3, T4

**Files:** Modify `src/engine/l5/digest_cli.rs:86-102` (+ a unit test in its tests module);
`tests/cli/cli_digest_changed_spans.rs` (T1, T1n, T3, T4); `CHANGELOG.md` (`### Fixed`).

- [ ] Write T1, T1n, T2, T3, T4 exactly as the ledger states; run them; confirm red for the stated
      reason (T1n and the already-working spellings may pass on base: record which).
- [ ] Implement the clean-up in `normalize_input` (order: `strip_scheme`, clean-up, lowercase).
- [ ] All green; `cargo test --test cli`, `cargo test -p al-sem --lib`, `cargo clippy --all-targets -- -D warnings`.
- [ ] Discrimination proof per test with the ledger's mutation; record each.
- [ ] CHANGELOG `### Fixed` entry; `check-diff`; commit.

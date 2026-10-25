# Digest human changed-roots diagnostics (#340) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `alsem digest --format human` prints one line per changed-roots diagnostic.

**Architecture:** `format_digest_human` takes the diagnostics too, and renders them after the entries through one exhaustive `match`. A shared `kind()` on the enum keeps the JSON and human kinds equal. Control characters are escaped.

**Tech Stack:** Rust (`al-sem`), nextest CLI tests, cli-b human goldens.

**Spec:** `## Design` and `## Acceptance matrix` in `.agent/issue-340/ledger.md` (ratified in 3 spec rounds; register `.agent/issue-340/findings.json`, S1–S10).

## Global Constraints

- The JSON bytes do not change. Exit codes do not change. The `--tests` line stays the separate last line.
- No blank line goes before the diagnostic lines, and each one ends in `\n`.
- `rustfmt <file>` only. Protected paths are never touched: `.github/ scripts/ .claude/ CLAUDE.md .gitignore Cargo.toml tree-sitter-al .agent/`.

## Review Focus

1. A path holding a control character prints on one line (A3).
2. An unmatched file next to a resolved entry still exits 0 and is now visible (A5).
3. Human goldens without diagnostics stay byte-identical (A4).

---

### Task 1: human changed-roots lines

**Files:**
- Modify `src/engine/l5/digest_cli.rs`:
  - add `ChangedRootsDiagnostic::kind`;
  - change `changed_roots_diagnostic_to_value` to use it;
  - add `changed_roots_diagnostic_human` and an escape helper;
  - change `format_digest_human`'s signature and its two callers (`digest_view`, and the unit test near `:2545`);
  - add the unit tests for A3 and A4.
- Modify `tests/cli/cli_digest_changed_spans.rs`: add `digest_raw_as` and the A1/A2 and A5 tests.
- Regenerate: the 6 human goldens named in P3 (`tests/cli-b-goldens/digest/`).
- Modify `CHANGELOG.md` (`[Unreleased]`, `### Fixed`).

**Interfaces:**
- Produces:
  - `pub fn format_digest_human(entries: &[DigestEntryFull], changed: &[ChangedRootsDiagnostic]) -> String`
  - `impl ChangedRootsDiagnostic { pub fn kind(&self) -> &'static str }`
  - `fn changed_roots_diagnostic_human(d: &ChangedRootsDiagnostic) -> String` (private)

- [ ] **Step 1: Write the failing tests.** Write A1/A2, A3, A4 and A5 exactly as the ledger's matrix states them. The A1 literal is fixed in the matrix.
- [ ] **Step 2: Run them.**
  - A1 and A5 should fail (no `changed-roots:` line).
  - A3 should fail to compile (the new signature), which is its stated red.
  - A4 must pass against today's output: write it first, before changing the signature, then adapt the call.
  - Record each outcome.
- [ ] **Step 3: Implement** per the Design (Placement, Line form, Escaping, One table).
- [ ] **Step 4: Golden triage and regen.**
  - Run `scripts/check-goldens` and confirm that only the 6 human goldens fail, each by the one added line `changed-roots: file-unmatched src/Rec.Table.al`.
  - Then run `scripts/check-goldens --regen`, check `git diff --stat` (only those 6 files, +1 line each), and run `scripts/check-goldens` again: green.
  - Any other moved line: stop and report.
- [ ] **Step 5: Discrimination proofs.** Make each mutation with Edit and revert it with Edit:
  - call site `format_digest_human(&entries, &[])` → A1 and A5 fail, A3 passes;
  - escaping removed → A3 (a) fails;
  - a blank line added before the diagnostics → A3 (b) fails.
- [ ] **Step 6: `scripts/ci-steps task-affected`, then `check-diff`, then commit** the named files.

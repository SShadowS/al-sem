# Issue #73 — `alsem graph` rejects an `app.json` without an id — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `alsem graph` applies the gate's workspace-identity rule: an `app.json` without a non-empty
string `id` exits 1 with `could not compute modelInstanceId`, while a missing or unparseable
`app.json` keeps its existing error.

**Architecture:** One check in `impact_graph::export_workspace`, immediately after
`build_cross_app_program(...)?`, using `crate::source_text::read_root_app_guid` (the rule the gate's
`compute_gate_model_instance_id` uses).

**Tech Stack:** Rust; `tests/cli` umbrella (`CARGO_BIN_EXE_alsem`).

**Spec:** the `## Design` (with tests T1, T2, T3 and the CHANGELOG text) in `.agent/issue-73/ledger.md`
(converged in spec round 3).

## Global Constraints

- Only `src/program/impact_graph/mod.rs`, `tests/cli/cli_graph_export.rs` and `CHANGELOG.md` change.
- No `#[allow(`; `rustfmt <file>` only; Edit/Write tools only.
- Never touch `.github/ scripts/ .claude/ CLAUDE.md .gitignore Cargo.toml tree-sitter-al .agent/`.
- No golden may move.

## Review Focus

1. A valid-id workspace: output byte-identical (`graph_export_golden`).
2. Missing / unparseable `app.json`: today's `snapshot build failed: read|parse` error, unchanged.
3. The check runs before any stderr output of the export.

### Task 1: the id check, with tests T1, T2, T3

**Files:** `src/program/impact_graph/mod.rs` (~:191-195), `tests/cli/cli_graph_export.rs`, `CHANGELOG.md`.

- [ ] Write T1, T2, T3 as the ledger states; run them; confirm red (T1 red; T2 red on the graph half;
      T3 passes on base, record it).
- [ ] Implement the check immediately after `build_cross_app_program(...)?`.
- [ ] All green; `cargo test --test cli`, `cargo test -p al-sem --lib`, `cargo clippy --all-targets -- -D warnings`.
- [ ] Discrimination proofs (T1: delete the check; T3: move the check before the build); record each.
- [ ] CHANGELOG `### Fixed` with the ledger's text; `check-diff`; commit.

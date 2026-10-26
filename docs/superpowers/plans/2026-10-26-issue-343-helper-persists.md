# d39 `helperPersists` (#343) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** d39 stops reporting a record that is persisted on every path after the dirtying call, by the caller or by a later helper whose own body provably persists it. A path that runs only on the `if C(..)` / `if not C(..)` arm where the callee returned the Boolean literal on which it provably leaves the record clean needs no persist. Every proof fails closed: when unsure, the finding stays. This removes the 3 CDO false positives (`CDOSetupData`) and moves no Base App row.

**Architecture:**
- L2: one in-memory field `PCFNNode.path_hint: Option<PathHint>` (`#[serde(skip)]`, outside `PartialEq`). It marks an `if` whose whole condition is a call, `exit(<bool literal>)`, `asserterror`, and ternary alternatives.
- L5 shared: `walk_paths` / `PathQuestion` in `src/engine/l5/detectors/mod.rs`, a forward walk over `statement_tree` with a small state bitset.
- d39: skip `helperPersists`, after `callerPersists` and gated by `!reset_after_call()`. Its questions are `persisted_on_every_path_after`, `helper_persists`, `helper_preserves`, `helper_clean` and `clean_return`. No L4 role is trusted by the skip; every fact about a called routine comes from its own body, to helper depth 3.

**Tech Stack:** Rust (`al-sem`), nextest CLI tests, lib unit tests, the `ir-l2` snapshot golden.

**Spec:** `## Design` in `.agent/issue-343/ledger.md` (revision 3). It was ratified in 4 spec rounds; the register is `.agent/issue-343/findings.json` (28 entries, all accepted by both reviewers).

## Global Constraints

- No other detector moves. The d39 move is exactly the 3 CDO fingerprints `5fcb917069429b4f`, `065cffc76dd0a963`, `1c237aab732f9816`; the Base App set is empty (F5: a `var` event publisher on the path fails the proof).
- `callerPersists` is unchanged.
- Only `tests/ir-l2-goldens/l2_features.snapshot` moves (Debug digest of the new field: 988 changed, 0 added, 0 removed), regenerated after a hash-only triage with a receipt.
- `rustfmt --edition 2024 <file>` only. Protected paths are never touched: `.github/ scripts/ .claude/ CLAUDE.md .gitignore Cargo.toml tree-sitter-al .agent/`.

## Review Focus

1. Fail-closed: an `exit`, `try`, `asserterror`, incomplete parse, unresolved / interface call, ternary alternative, publisher or unproven helper on the path keeps the finding.
2. The clean-return arm settles only this run's dirt (`CARRIED` for an earlier loop pass).
3. Helper facts come from bodies, never from L4 roles.

---

### Task 1: d39 `helperPersists`

**Files:**
- `src/program/body/features.rs`: `PathHint`, `PCFNNode.path_hint`, docs.
- `src/program/body/ir_walk.rs`: set the hint for `if` / `exit` / `asserterror` / ternary; `None` in `cfn_node`.
- `src/engine/l4/cfg_walker.rs`, `src/engine/l5/pending_writes.rs`, `src/engine/return_summary.rs`, `src/program/body/control_context.rs`, `src/program/body/operation_order.rs`: `path_hint: None` in struct literals.
- `src/engine/l5/detectors/mod.rs`: `walk_paths`, `PathQuestion`, `PathStates`, unit tests `walk_paths_tests` (7).
- `src/engine/l5/detectors/d39.rs`: the skip, the questions, the `helperPersists` stat, the module doc.
- `tests/cli/cli_d39_helper_persists.rs` (+ `tests/cli/main.rs`), fixture `tests/fixtures/issue-343/ws-helper/` (27 shapes, 20 expected rows).
- `tests/l2_ir/ir_l2_snapshot.rs` (doc list), `tests/ir-l2-goldens/l2_features.snapshot` (regen).
- `CHANGELOG.md`: `[Unreleased]` → `### Fixed`.

- [ ] Rebase the prototype onto `origin/master`; confirm the diff equals revision 3.
- [ ] Apply the two optional cleanups (split the `LoopGuard` preparation line, renumber the rows; the `helper_clean` copy comment) and the F5 module-doc line.
- [ ] Re-run breaks B2-B5 and B12-B14 on the final code; record RED / GREEN per break in the ledger.
- [ ] `scripts/ci-steps task-affected`.
- [ ] `scripts/check-goldens`: only `l2_features.snapshot` fails; triage it as hash-only; write `.agent/golden-triage.md`; `scripts/check-goldens --regen`; `scripts/check-goldens` green.
- [ ] CHANGELOG entry.
- [ ] Commit; `check-diff --base $(git merge-base HEAD origin/master)`.
- [ ] Final-head fingerprint gate: release-fast base = merge-base, head = this commit; Base App, CDO primary, CDO all, one at a time, with the strict `gate.sh` (expected: Base App empty, CDO 3 / 3). Record provenance.

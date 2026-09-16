# Issue #11 — derive `page-action` in the AST root classifier

Spec: `.agent/issue-11/ledger.md` (design v3.1, panel-converged — both reviewers accepted all
18 register entries). Read it first; this plan does not restate the design or the evidence.

## Why two tasks and not one

The pre-commit hook (`scripts/git-hooks/pre-commit`) blocks any commit touching `src/engine/`
unless `check-goldens` passes. This change MOVES four goldens by design, so the implementation,
its tests and the regenerated goldens must land in ONE commit — a red-then-green split across
commits is not committable here. The CHANGELOG is docs-only and is separate.

## T1 — the classifier change, its tests, and the golden regen (ONE commit)

Everything in `src/engine/root_classification.rs` plus the four regenerated goldens.

1. **`is_page_action_wrapper(syntax_kind: &str) -> bool`** — matches exactly
   `action_declaration`, `systemaction_declaration`, `fileuploadaction_declaration`,
   `customaction_declaration`. A doc comment carries the per-wrapper evidence table from the
   design: which are real AL forms, that `customaction` is DEFENSIVE with no coverage claimed,
   and why `separator_action` and `actionref_declaration` are excluded (the latter also being
   unreachable as a member).
2. **The branch change** in `kinds_for`'s `"Page" | "PageExtension"` arm — additive, exactly as
   the design shows.
3. **`ROOT_KIND_VALUES`'s comment** — `web-service-exposed` and `job-queue-entrypoint` become
   "Currently supplied only by the config overlay. Source may indicate capability; the overlay
   supplies user-asserted publication or scheduling information." No impossibility claim, and no
   claim that the overlay verifies deployment. A10.
4. **A `#[cfg(test)] mod tests`** implementing A1–A6 and A8–A11. Every case runs through
   `classify_roots`, never `is_page_action_wrapper` alone — a helper-only test does not pin the
   production use, which is the exact failure CLAUDE.md records five instances of. Hand-state
   every precondition by ASSIGNMENT. A3 must move `L3Object.object_type`, NOT `L3Routine`'s
   similarly-named field.
5. **A7's discrimination proof** — delete the `page-action` insertion, assert the deletion
   applied (`s.count(old) == 1`), record that A1/A2/A3 fail and A4/A5/A6 stay green, byte-restore
   with a hash check, re-run. Both outcomes go in the ledger's proof table. A break that comes
   back GREEN is evidence about the test, not the code.
6. **Goldens** — dispatch `golden-diff-triager` BEFORE any regen. Expected: exactly the four
   `tests/cli-b-goldens/snapshot/ws-sibling-member-triggers.*` serializations, each gaining
   `page-action` and losing nothing. `cli_b_fingerprint_oracles.rs` must NOT move. No identity or
   fingerprint may change. An unexplained line is a discovery, not a rebaseline. A12.

## T2 — CHANGELOG (separate commit)

`## [Unreleased]` → `Added`, naming the four matched wrappers, the additive behaviour, the
overlay's new third-case warning, and the action-modification gap as a stated limit.

## Out of scope, recorded

Action-extension modifications (`modify(SomeAction) { trigger OnAfterAction() }`). Not fixable
by a fifth string — `modify_action_modification` carries `target` and no `name`, so the lowerer
never captures the wrapper. Needs wrapper capture AND target-name extraction, and enclosing-member
names participate in stable routine identity, so it carries identity consequences. Filed as a
discovery.

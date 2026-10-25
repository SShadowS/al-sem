# Implicit Rec Bindings (#272) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** An implicit table/tableext/pageext/page-with-SourceTable `Rec`/`xRec` passed as a bare argument binds like a declared record, so d41/d42 report it like a local and d39 reports it except where the caller or the platform persists `Rec`.

**Architecture:** Three edits in existing code: the binding's initial state (`calls.rs`), the implicit-Rec label and synthetic-Rec shadowing (`ir_walk.rs`), and d39's single owner guard (`d39.rs` + `detectors/mod.rs`). Docs and CHANGELOG follow.

**Tech Stack:** Rust; al-sem program engine and L5 detectors.

**Spec:** `## Design` (revision 3, with the O15 amendment) in `.agent/issue-272/ledger.md`. Prototype for reference only (do not copy blindly): `/work/bench/272/rev3/proto-rev3-with-breaks.diff`.

## Global Constraints

- Edit with Edit/Write only; format with `rustfmt <file>`, never `cargo fmt`.
- Protected paths, never touched: `.github/ scripts/ .claude/ CLAUDE.md .gitignore Cargo.toml version tree-sitter-al .agent/`.
- Export `TREE_SITTER_AL_PATH=/work/alsem/tree-sitter-al` in every build.
- Every new or changed test gets a discrimination proof (break, fail, revert, pass), recorded in the ledger's proof table; every scripted break asserted to have applied.
- No golden may move except the two `ws-argument-bindings` l2 goldens (l2 snapshot + r1a), and only `CallVariations/cs3` (`implicit-rec` → `unknown`, `sourceVariableName` dropped). Anything else: stop and triage line by line (fallback (c)).
- Per-task gate: `scripts/ci-steps task-affected`.

## Review Focus

1. A declared `Rec`/`xRec` of any type (scalar included) must never bind to a phantom synthetic record — pinned by A2b L2 test.
2. A codeunit-TableNo or report-dataitem `Rec` keeps today's typed `local` binding even with a global `Rec` (O15) — the restriction has no fixture (AL0155 not checked there); covered by A7's golden staying at `cs3` only and by `gap_audit` codeunit tests already on master.
3. `xRec` always reports in d39 — A5 (`shapes_d39_owner_rule`).
4. A pageext whose base page is outside the model counts as modifiable — state it in the code comment; no fixture (stated limit).
5. d37 becomes more conservative at unresolved/ambiguous/arity-mismatched `Rec` sites — A4 (`readers_see_the_implicit_rec`).

---

### Task 1: Binding and label (changes 1 and 2)

**Files:**
- Modify: `src/program/model/calls.rs` (`initial_binding_state`, ~line 283; doc at ~280)
- Modify: `src/program/body/ir_walk.rs` (implicit-rec label; `ir_record_variables` synthetic-Rec suppression)
- Regenerate: the l2 snapshot + r1a goldens for `ws-argument-bindings` (A7)
- Tests (already written, uncommitted): `tests/cli/cli_loose_fingerprint.rs` (A1), `tests/cli/cli_implicit_rec_bindings.rs` (A2, A3, A4), `tests/l2_ir/l2_implicit_rec_source_kind.rs` (A2b), fixtures `tests/fixtures/issue-272/`

**Interfaces:**
- Produces: bindings with `source_kind == "implicit-rec"` that start `"unresolved-callee"` and upgrade like a local; label `implicit-rec` only when (a) the object is Table / TableExtension / PageExtension / Page with a resolved SourceTable (existing `has_implicit_rec` minus its codeunit-TableNo case) and (b) no parameter, local or object global of any type is named `rec`/`xrec`; else `parameter` → `local` → `unknown`.
- `ir_record_variables`: a declared `Rec` of any type suppresses the synthetic `Rec` row; the object-global part applies only to Table, TableExtension, Page, PageExtension (O15).

- [ ] Step 1: Run A1, A2, A3, A4, A2b on the unchanged base; confirm each fails for its stated reason (findings missing / wrong `sourceKind`), not a compile error; log under `/work/bench/272/tests/`.
- [ ] Step 2: Implement change 1 in `initial_binding_state`.
- [ ] Step 3: Implement change 2 (label + shadowing, with the O15 restriction).
- [ ] Step 4: A1, A2, A3, A4 and A2b pass. NOTE: the A5 test (`shapes_d39_owner_rule`) will now show extra d39 findings until Task 2; that is expected — leave it red and say so.
- [ ] Step 5: Regenerate goldens (`REGEN_TEMP_GOLDENS=1` via `scripts/check-goldens --regen` scoped by its usual targets); confirm only the two `ws-argument-bindings` l2 goldens move, only `CallVariations/cs3`.
- [ ] Step 6: Discrimination proofs: revert change 1 → A1/A2/A3/A4 fail (A2: every implicit-rec d41/d42 finding gone); drop the local shadow check → A2b `ShadowScalar` becomes `local`; drop the global shadow check → A2b gshadow sites become `local` and the model test's table is the object's own (50304/50300); ignore (b) → `implicit-rec`; restore `unwrap_or(true)` → A7 golden fails.
- [ ] Step 7: Do not commit alone if A5 is red (pre-commit hook). Tasks 1 and 2 are committed together at the end of Task 2, as two commits in sequence (`git commit --no-verify` is NOT allowed; stage Task 1's paths, and commit after Task 2 is green so the hook passes on each commit's tree — if the hook blocks Task 1's commit because A5 is red, combine both tasks into one commit).

### Task 2: d39 owner rule, docs, CHANGELOG (changes 3 and 4)

**Files:**
- Modify: `src/engine/l5/detectors/d39.rs` (guard + stat; module doc)
- Modify: `src/engine/l5/detectors/mod.rs` (delete `is_auto_persist_trigger_rec` ~line 374; add `implicit_rec_persisted_by_owner`, `PAGE_TRIGGERS_REC_PERSISTED`, `TABLEEXT_TRIGGERS_AFTER_WRITE`; fix doc at ~336 and ~366-373)
- Modify: `docs/detector-audit.md:26-27`, `src/program/model/calls.rs:280` (if not done in Task 1), `tests/cli/cli_loose_fingerprint.rs:904-907` (if not done in step 6), `CHANGELOG.md`
- Tests: `tests/cli/cli_implicit_rec_bindings.rs::shapes_d39_owner_rule` (A5), `gap_audit_b_table_triggers::table_level_trigger_rec_ops_are_suppressed`, `shapes_d40_still_skips_the_implicit_rec`, r4 `ws-d40` golden (A6), `tests/r3/r3a2_implicit_rec_pin.rs` (A8)

**Interfaces:**
- Consumes: Task 1's `implicit-rec` bindings.
- Produces: `pub(crate) fn implicit_rec_persisted_by_owner(routine, ctx) -> bool` per Design change 3; d39 skips only when binding is `implicit-rec` AND name is `rec` AND no `record_load_points` entry after the call (not `branch_exclusive`), with a comment why there is no `is_variable` check; stat `implicitRecOwnerPersists` replaces `autoPersistTriggerRec`.
- Constants (exact): `TABLEEXT_TRIGGERS_AFTER_WRITE` = OnAfterInsert, OnAfterModify, OnAfterRename; `PAGE_TRIGGERS_REC_PERSISTED` = OnAction, OnValidate, OnLookup, OnAssistEdit, OnDrillDown, OnNewRecord, OnInsertRecord, OnModifyRecord, OnBeforeAction, OnAfterAction, OnAfterValidate, OnBeforeValidate (comment: OnBeforeValidate by analogy; the rest measured on BC 28.4; OnDelete/OnAfterDelete exempt as moot). Page/PageExtension exempt only when not `Editable = false` and not `ModifyAllowed = false`; pageext reads its base page by name, a base page outside the model counts as modifiable.

- [ ] Step 1: A5 red (extra d39 rows from Task 1).
- [ ] Step 2: Implement the guard, constants, stat; delete `is_auto_persist_trigger_rec`.
- [ ] Step 3: A5, the gap test, the d40 test, A6 golden, A8 pin pass.
- [ ] Step 4: Discrimination proofs: owner rule returns false → +11 d39 on ws-shapes and the gap test fails; drop the reload condition → `ExtReloadAfter` disappears from A5; page procedure not exempt → `PageProcDirty` appears; ignore ModifyAllowed → `S NoMod Card` OnAction disappears; read the pageext's own properties → `S RO List Ext` OnAction disappears; delete C2(a) in d40 → r4 `ws-d40` golden fails.
- [ ] Step 5: Docs and CHANGELOG (Fixed: binding, label, scalar and global shadowing. Changed: d39 rule incl. the runtime-only view-mode false negative; d37 more conservative at unresolved/ambiguous/arity-mismatched `Rec` sites; d11, d21, d33 now see helper loads/filters on `Rec`).
- [ ] Step 6: `scripts/ci-steps task-affected` green; `python scripts/agentflow --root /work/alsem --run-id <rid> check-diff --base <merge-base> --head HEAD --issue 272 --cwd /work/al-sem-issue-272-a1` clean; commit (Task 1 and Task 2, see Task 1 step 7), message naming #272.

# Issue #80 — attributes survive trivia and `#if` lines — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A routine keeps every attribute written before it, whatever comment, pragma or `#if` line
sits between them, and each carried attribute's `#if` arguments are read as some real build reads them.

**Architecture:** All work is in the al-syntax lowerer, `crates/al-syntax/src/lower/mod.rs`. The
pending attributes in `collect_routines` become guarded entries `(attribute node, guard)` that flow
through comments/pragmas (the grammar's `extras`) and through single-chain `#if` wrappers. They are
admitted into arms by guard, fall through by guard, are projected onto the symbols later code reads,
and are capped loudly. `conditional_list_arm` reads a positional list with one running context.
`lower_object` filters each entry per split-header arm, then deduplicates per (node, selection).

**Tech Stack:** Rust, tree-sitter-al grammar `main` @ `8682cb5f` (via `TREE_SITTER_AL_PATH`).

**Spec:** the `## Design`, `## Acceptance matrix` and `## Not delivered` sections of
`U:/Git/al-sem-issue-80-a2/.agent/issue-80/ledger.md` (spec panel converged in round 5, ledger
sha256 `18ffd360…8b4f`). Read the whole Design before starting any task.

## Global Constraints

- Only `crates/al-syntax/src/lower/mod.rs`, `tests/cli/cli_attribute_trivia.rs`, `tests/cli/main.rs`
  and `CHANGELOG.md` change. No IR type changes (`AttributeIr` and `RoutineDecl` stay as they are).
- No committed golden file may change. A moved golden is a finding to triage, not a license to regen.
- Never touch `.github/ scripts/ .claude/ CLAUDE.md .gitignore Cargo.toml tree-sitter-al .agent/`
  (no `Cargo.toml` version edit).
- `rustfmt <file>` per file, never `cargo fmt`. Edit with Edit/Write, never sed/python.
- Every cargo command runs with `TREE_SITTER_AL_PATH=U:/Git/al-call-hierarchy/tree-sitter-al` and
  `CDO_WS` unset.
- No `#[allow(...)]` added, except the existing `clippy::type_complexity` /
  `clippy::too_many_arguments` already on `collect_routines`.
- Every new or changed test gets a discrimination proof, using the mutation named in its matrix row
  (break with Edit, run, show the failure, revert with Edit, run, show the pass). Record it in the
  ledger's `## Discrimination proofs` table.

## Review Focus

1. An attribute directly before its routine with nothing between them, the 99.9% case: the
   output must be byte-identical to today. Pinned by every golden staying unchanged and by A7's
   controls.
2. A routine inside an enclosing `#if X` whose attribute crosses no wrapper: the args must still
   be read under the routine's own context. P21's `:5507-5518` test must stay green unmodified.
3. Report/table objects, where a dangling attribute sits at the end of a nested section: it must
   never reach the next routine (A11 table case).
4. Key field lists using `#if` groups (`key_members`, `:512`) must read exactly as before when there
   is no nesting, sibling correlation or contradiction. Pinned by the existing key tests and goldens.
5. Very long `#if` runs must not hang (A17, A22).

## Shared types (introduced in Task 1/2, used by Task 3)

```rust
/// Symbol literals that a build path decides, in the form of `ctx` (`RoutineDecl::preproc_context`).
type Syms = Vec<(String, bool)>;

/// `a ∪ b`, or `None` when they give one symbol both values (no build).
fn consistent(a: &[(String, bool)], b: &[(String, bool)]) -> Option<Syms>;

/// One pending attribute: the `attribute_item` node and the projected guard it waits under.
#[derive(Clone, PartialEq, Eq)]
struct PendingAttr<'t> { node: RawNode<'t>, guard: Syms }

/// `preproc_branches` plus the implicit "no arm taken" path.
struct PreprocChain<'t> {
    branches: Vec<(Syms, bool, Vec<RawNode<'t>>)>,   // exactly preproc_branches' output
    falls_through: Option<Syms>,                       // see ledger "The chain logic"
}
fn preproc_chain<'t>(node: RawNode<'t>, source: &str, ctx: &[(String, bool)]) -> PreprocChain<'t>;

/// Reads ONE `#if` group of a positional list under the RUNNING context `run`.
fn conditional_list_arm<'t>(group: RawNode<'t>, source: &str, run: &mut Syms, out: &mut Vec<RawNode<'t>>);

const PENDING_PATH_CAP: usize = 64;
```

`collect_routines`'s output tuple changes its second element from `Vec<RawNode<'t>>` to
`Vec<PendingAttr<'t>>`, and it gains an `issues: &mut Vec<SyntaxIssue>` parameter (for the cap) and a
`cond_syms: &[(usize, String)]` parameter: every `(start_byte, symbol)` of every identifier inside the
`condition` of every `preproc_if`/`preproc_elif` node in the object, at any depth, sorted by
`start_byte`. `lower_object` builds that index once per object.

---

### Task 1: `consistent`, `preproc_chain`, running-context `conditional_list_arm`

**Files:** Modify `crates/al-syntax/src/lower/mod.rs`: `preproc_branches` (`:1156-1197`),
`conditional_list_arm` (`:1207-1226`), callers `key_members` (`:512`) and `lower_routine` (`:1441`).
Tests: A18 (`nested_attribute_arg_group_reads_under_the_chosen_arm`) and A20
(`sibling_attribute_arg_groups_share_one_context`), already written in step 6.

**Interfaces:** Produces `Syms`, `consistent`, `PreprocChain`, `preproc_chain`, and the new
`conditional_list_arm` signature.

- [x] **Step 1:** Run A18 and A20 and confirm they fail as `red.md` records:
  `cargo test -p al-syntax --lib -- lower::tests::nested_attribute_arg_group_reads_under_the_chosen_arm lower::tests::sibling_attribute_arg_groups_share_one_context`.
- [x] **Step 2:** Add `consistent`. Keep `a`'s order, append each `b` literal whose symbol is new,
  and return `None` when `b` gives a symbol in `a` the opposite value.
- [x] **Step 3 (moved to Task 2):** Rename the body of `preproc_branches` into `preproc_chain` and make it return
  `PreprocChain`. `falls_through` = `Some(syms)` iff the chain has no `PreprocElse` AND `taken` is
  false at `PreprocEndif`, where `syms` = every `#if`/`#elif` condition assumed false via
  `assume_preproc`. `preproc_branches(node, source, ctx)` becomes `preproc_chain(...).branches`, so its
  callers get the same output.
  *Done note:* nothing in Task 1 reads `falls_through`, so adding it here leaves a never-read field
  (a `dead_code` warning, red under `-D warnings`) and adding `#[allow]` is forbidden. It lands in
  Task 2 with its first reader. Task 1 adds `Syms` and uses it in `preproc_branches`'s return type,
  which removed that function's `#[allow(clippy::type_complexity)]`.
- [x] **Step 4:** Rewrite `conditional_list_arm`. Pick the first branch (skip entry 0) that is live
  under `*run` AND whose syms satisfy `consistent(run, syms).is_some()`. If none, return. Otherwise
  set `*run` to that join, then for each item recurse with the same `run` if it is a nested group
  (`item.kind() == group.kind()`), else push it when it is not `Class::Trivia`.
- [x] **Step 5:** Callers: `key_members` makes `let mut run = Vec::new();` once per key and passes
  `&mut run` to every group. `lower_routine` makes `let mut run = <the context it uses today>` once per
  argument list (Task 3 changes what it starts from) and passes `&mut run` to every group.
- [x] **Step 6:** A18 and A20 pass. Run all `lower::tests` and `cargo test --test l2_ir`. Every other
  test stays green.
- [x] **Step 7:** Discrimination proofs for A18 (recurse with an unchanged copy of the context) and
  A20 (give each group a fresh clone of the start context). Record both in the ledger.
  *Done note:* both proofs are in the task report `scratchpad/80/t1.md`; the implementer may not
  write `.agent/`, so the orchestrator copies them into the ledger.
- [x] **Step 8:** `rustfmt crates/al-syntax/src/lower/mod.rs`, `check-diff`, then commit with A18/A20
  and the step-6 tests the task turns green: `fix(al-syntax): read #if groups in a positional list with one running context (#80)`.

### Task 2: guarded pending entries in `collect_routines`

**Files:** Modify `crates/al-syntax/src/lower/mod.rs`: `collect_routines` (`:783-1010` and its doc),
`lower_object` (`:219-245`, to build `cond_syms` and pass `issues`), and the `lower_routine` call (a
temporary `attr_items.iter().map(|e| e.node)` until Task 3). Tests (step 6): A1 (incl. the renamed
`attribute_separated_by_comment_is_attached`), A2-A8, A10, A11, A13, A16, A17, A21, A22.

**Interfaces:** Consumes Task 1's `Syms`, `consistent`. First adds `PreprocChain`/`preproc_chain`
(Task 1 Step 3, moved here with its first reader). Produces `PendingAttr`,
`PENDING_PATH_CAP`, the new `collect_routines` signature and output tuple.

- [x] **Step 1:** Run the task's tests and confirm the `red.md` failures.
- [x] **Step 2:** Make `pending: Vec<PendingAttr<'t>>`. `AttributeItem` pushes
  `PendingAttr { node: child, guard: ctx.to_vec() }`. Add the keep-set arm
  `RawKind::Comment | RawKind::MultilineComment | RawKind::Pragma | RawKind::PreprocRegion | RawKind::PreprocEndregion | RawKind::PreprocDefine | RawKind::PreprocUndef => {}`,
  with a comment citing `grammar.js:539-547` (the grammar's `extras`). The routine arm pushes the
  entries sorted by `node.start_byte()` (stable).
  *Done note:* the guard is `consistent(&[], ctx)` (ctx without duplicate literals), and the
  attribute is not pushed when that is `None`. Inside an arm `ctx` is the outer ctx ∪ arm syms, so
  this is the Design's "the arm's own attributes enter with `consistent(ctx, arm_syms)`, or
  nothing". `PendingAttr` derives only `Clone`: `RawNode` has no `PartialEq`, so the dedup
  compares `node.id()`.
- [x] **Step 3:** Make `collect_routines` return the entries still pending at its end. Every `_`
  recursion and every section arm passes an EMPTY pending list and ignores the result, exactly as
  today.
- [x] **Step 4:** The wrapper arm, for `is_preproc_wrapper(child)` with exactly one `PreprocIf` and one
  `PreprocEndif` among its named children (otherwise clear and recurse as today). Walk each live arm
  of `preproc_chain(child, source, ctx)` once, with `ctx ∪ arm_syms` for routine collection, as today.
  The arm's input entries are:
  - each incoming entry `e` for which `preproc_chain(child, source, &e.guard).branches[k]` is live and
    `consistent(&e.guard, arm_syms_k)` is `Some`, with that join as its guard;
  - the arm's own attributes enter with `consistent(ctx, arm_syms_k)`, or nothing when that is
    `None`.

  Skip entry 0 and the after-`#endif` group (never walked). After the wrapper,
  `pending` = the arm leftovers, plus each incoming entry whose own chain has `falls_through:
  Some(f)` and `consistent(&e.guard, &f)` is `Some`, with that join as its guard.
- [x] **Step 5:** Projection. Right after the wrapper, with `end = child.end_byte()`, keep a literal
  `(s, v)` of an entry's guard only if `s` is in `cond_syms` at some `start_byte >= end`, or `s` is
  mentioned by a condition inside `e.node`'s own byte range (also from `cond_syms`). Then remove
  exact duplicates `(node, guard)` (compare the guard as a sorted set).
- [x] **Step 6:** Cap. If any node has more than `PENDING_PATH_CAP` entries, remove all of that node's
  entries and push one
  `SyntaxIssue { message: "attribute reaches its routine through more than 64 distinct `#if` paths; not attached".into(), origin: origin_of(node) }`
  per node, never per path.
- [x] **Step 7:** Rewrite the doc comment of `collect_routines`: the keep-set, the guarded path flow,
  projection and the cap (R1-08).
- [x] **Step 8:** The task's tests pass. All `lower::tests`, `--test l2_ir` and `--test
  program_resolve_harness` stay green.
  *Done note:* A6's `MixedBefore` needs a dedup in Task 2. `CLEAN25` is read again later in the
  same test object (`PragmaInIf`), so projection keeps both exact paths, and the routine receives
  `[Scope]` twice. The temporary node map in `lower_object` therefore keeps ONE copy per attribute
  node. That stands in for Task 3's (node, selection) dedup, which replaces it. As a side effect,
  A9 and A23 already pass in Task 2. Task 3 still owns their proofs.
- [x] **Step 9:** Discrimination proofs with each row's mutation: A1-A5, A10 (delete the keep-set
  arm), A2 (remove `MultilineComment`), A3 (remove `Pragma`; separately `PreprocRegion`), A6 (a) and
  (b), A7, A7b, A8 (a)(b)(c), A11 (a)(b), A13, A16, A17, A21, A22 (a)(b). Record each.
  *Done note:* recorded in `scratchpad/80/t2.md` (the orchestrator copies them into the ledger).
- [ ] **Step 10:** rustfmt, `check-diff`, then commit:
  `fix(al-syntax): attributes survive comments, pragmas and #if lines before their routine (#80)`.

### Task 3: per-header-arm filtering, argument context and deduplication

**Files:** Modify `lower_object` (`:229-245`) and `lower_routine` (`:1311`, attribute lowering
`:1409-1460`). Tests (step 6): A9, A14, A15, A19, A23.

**Interfaces:** Consumes `PendingAttr`, `consistent`, the running-context `conditional_list_arm`.
`lower_routine` takes `Vec<PendingAttr<'t>>` instead of `Vec<RawNode<'t>>`.

- [x] **Step 1:** Run the task's tests and confirm the `red.md` failures.
- [x] **Step 2:** In `lower_object`'s per-arm loop, compute `a_syms = arm_symbols(r, &arm, source)`.
  Keep each entry whose `consistent(&e.guard, &a_syms)` is `Some`, replacing its guard with that join.
  For a non-split routine `a_syms` is empty.
- [x] **Step 3:** In `lower_routine`, an OUTER entry's argument list starts its running context from
  the entry's guard (now `guard ∪ a_syms`). An in-node split-header attribute keeps
  `preproc_context` as today.
- [x] **Step 4:** Deduplication. For each entry, compute its selected argument NODES (the structural
  args with each `#if` group expanded by `conditional_list_arm`) BEFORE `lower_expr`. Keep the first
  entry, in start-byte order, per (node, selection). Lower only those, so there are no dead `ExprId`s
  and no duplicate "`#if` inside attribute arguments" `SyntaxIssue`s. Factor this so the selection is
  computed once and reused for lowering (DRY).
- [x] **Step 5:** The task's tests pass. All `lower::tests`, `--test l2_ir`, `--test
  program_resolve_harness` and `--test r4` stay green.
- [x] **Step 6:** Discrimination proofs: A9 (skip the dedup), A14 (start from the routine's
  `preproc_context`), A15 (skip the arm filter), A19 (merge entries by guard intersection), A23 (dedup
  in `collect_routines` before the arm filter). Record each.
  *Done note:* recorded in `scratchpad/80/t3.md`. A19's "keep only the first entry per node" break
  stays GREEN (both of `H`'s paths select `'OnB'`), so A19 is proven with the plan's intersection
  merge. The A23 break deduplicates in `lower_object` before the arm filter (same effect as in
  `collect_routines`). The review-pass fixes (text pre-check removed, hashed `settle_pending`, cap
  scope wording) and the new test `projection_sees_spaced_and_upper_case_directives` land here too.
- [ ] **Step 7:** rustfmt, `check-diff`, then commit:
  `fix(al-syntax): carried attributes are filtered per split-header arm and read under their own build path (#80)`.

### Task 4: end-to-end digest test, CHANGELOG, CDO measurement

**Files:** `tests/cli/cli_attribute_trivia.rs` and `tests/cli/main.rs` (written in step 6),
`CHANGELOG.md`.

- [ ] **Step 1:** `cargo test --test cli cli_attribute_trivia::` passes (A12, incl. the target and
  merge variants). Prove discrimination with the A12 mutations (delete the keep-set arm; start
  argument reading from the routine's context). Record both.
- [ ] **Step 2:** Population (2) scan, BEFORE comparing CDO numbers. A scratch script under `$SCRATCH`
  lists every `preproc_conditional_attribute_args` / `preproc_conditional_field_list_items` in CDO
  (`U:/Git/DO-cdo-baseline/Cloud`) and its `.alpackages` sources that is nested, shares a symbol with
  a sibling group in the same list, or has a self-contradictory arm. Record the count and the list.
- [ ] **Step 3:** Re-measure, with the same commands as `$SCRATCH/base.sh`, on the branch build:
  `aldump --program-call-graph-stats`, `--dependency-bodies-stats`, and `alsem analyze --scope primary`
  / `--scope all`. Diff against P20c. `unknown` 0, `recoveredFiles` 0, coverage holds. Trace every
  moved edge or finding to a P19 routine or a step-2 site. An untraced move blocks.
- [ ] **Step 4:** CHANGELOG `## [Unreleased]` → `### Fixed`: the bug, the fix, the stated limits (the
  64-path drop with its `SyntaxIssue`; union-read; same-signature `#if` arm duplicates still keep the
  first copy's subscriptions), and the CDO before/after numbers.
- [ ] **Step 5:** `check-diff`, then commit: `docs: CHANGELOG for #80` (include the A12 test files if
  they are not committed yet).

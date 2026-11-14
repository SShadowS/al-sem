# ENG-14: the #310 remainder: dead marks, `Error` and d20 hold in every build

Status: r4 (2026-11-14), after four review rounds (Fable "Ready on two edits" at r2, both in; sol
"Not ready" at r3). Every finding is answered in §9 to §9.3. Task: ENG-14 (coord). Issue: #310, acceptance re-scoped in
<https://github.com/SShadowS/al-sem/issues/310#issuecomment-6873897989>.
Predecessor: `refs/backup/wip/al-sem-issue-310-a1`, spec
`docs/superpowers/specs/2026-10-26-issue-310-reachability-soundness-design.md` (r3, parked). This spec
reuses its probes and reasoning and drops what wave 2 already did.

## 0. What wave 2 (B2) did, and what is left

Done on master (CHANGELOG "engine limits wave 2, B2"):
- A statement `#if` chain is an IR `PreprocGroup` (`lower_preproc_statements`); a chain with one live
  arm that every build takes is spliced. The CFN encodes a group as a **`case` with no scrutinee**, one
  `case-branch` per arm, the last arm the `else` when the group cannot fall through
  (`IrCfn::build_preproc`, ir_walk.rs:2018-2044).
- `control_flow::may_exit` is the one "may return early" rule for `control_context` and
  `operation_order`.

Left (master 6852a891, confirmed by the probes in §3):
1. **`Error` is a name test.** `IrCfn::is_error_fn` (ir_walk.rs:2053, identifier or quoted) and the
   unreachable scan (`unconditional_exit_kind`, ir_walk.rs:999, identifier only) end the path at every
   bare `Error(...)`: `Error(ErrInfo)`, an own or table-scope `procedure Error`, a call split by `#if`.
2. **Slots.** A `#if` chain in a one-statement slot is lowered inside the slot's block, so
   `if C then #if X exit; Foo(); #endif` kills `Foo`, and the next statement is not read as the slot
   body of the build that takes no arm.
3. **Split shapes keep a union reading**: split procedure bodies (prefix arms and guard), split
   closings, cond-only heads, two-head if-else, split `case` closings.
4. **The summary walkers compose sequences wrongly.** `walk_subtree`'s `block` arm
   (return_summary.rs:121-141; the same in dep_artifact_l4.rs:814-854) keeps scanning after a statement
   that may return normally, so `if C then exit; Error('x');` gives `all_paths_error: true`.
5. **#288's trust gate** (`dead_marks_trusted`, `source_dead_mark_trust`, `settle_dead_mark_trust`)
   shields only the cone's `DeadSites` and d35. d20, `return_summary`, `dep_artifact_l4`, `cfg_walker`
   and the ordering facts read the raw marks.

## 1. Goal

A dead mark, a d20 finding, an `all_paths_error: true` and a `dominatesSuccessReturn: true` are claimed
only when they hold in every build, for every binding of a bare `Error` and every error-collection
setting. **Excluded and stated as a limit:** effects of a callee that leaves the caller's trigger
without returning (`CurrReport.Skip`/`Break`/`Quit`, local or in a helper); that is ENG-74 (§8).

## 2. Non-goals

- Choosing a build configuration (L22). P14's lost precision (§3.1) is a follow-up, not this task.
- Precision inside the unordered runs (§4.2 B): real dead code there stays live (a false negative).
- The nested may-exit context lattice; `ui_events_error`, the entry temp-guard's `Error` test, the
  error-op kind; other first-name-wins type readers; collecting a split body's arm locals.

## 3. Probes

Sources `scratchpad/eng14/ws/src/*.al`; parse `scratchpad/eng14/cst/*.cst`; base readings
`scratchpad/eng14/summary-base.tsv`.

### 3.1 Results (2026-11-14)

Parse (`tree-sitter-cli` 0.26.7, grammar `f8c6d8b6`): every probe parses with no ERROR or MISSING node
and hits its intended kind, except that `OnRun` is a `preproc_split_procedure_body` (a real
`preproc_split_complete_body` probe is owed, Task 1) and P9 is `preproc_split_if_then_begin_else_shared`.
P7 carries the flat `if_keyword condition then_keyword` in its `#if` arm.

Base (`aldump --l2`, ENG-15's head binary, the tree master's b4 merge landed). Wrong today:
- dead in a build that runs it: `Call5` (OnRun), `Call7` (P7, P7Ctx), `Call8b` (P8), `Foo` (C1Then,
  C1Else, C1Loop, P1oSpliced, N2Nested), `Foo`/`Work` (M8CaseEnd), `Bar` (I3SplitIfElse, marked by the
  control context with NO d20 entry);
- `dominatesSuccessReturn: true` where a build skips it: `Bar` (P1pNoArm);
- every `Error` form ends the path (ErrorInfo forms, conditional globals, P15, P13, S1Operand(Rev),
  S2Callee, I1SplitCall, own and table-scope `Error`); `InfoThenOp`'s `Op` is `unreachable`;
- report exits (ENG-74): `AfterSkip`/`AfterBreak` `top-level`, true; `Quit` has a d20 entry but the
  code after it is `top-level`, true.

Right today: P1, P6, P6b (`Call6b` dead), P9, MayExitNested, the Text forms, `Q.Skip()` on a codeunit.
P14's `Call14` reads `conditional` (the whole-object `#if X` does not reach the routine's chain):
precision lost since B2, safe, deferred.

## 4. Design

### 4.1 One `Error` rule, decided at assembly

`ir_walk::error_call_terminates(call, ctx) -> bool` replaces `IrCfn::is_error_fn` and the `Error` arm of
`unconditional_exit_kind`. It holds only when all three hold:

1. **Untainted** (§4.2 C).
2. **Text overload proven**:
   - two or more arguments (untainted, so a `#if` cannot fake the arity);
   - one argument that is: a string literal; a name with **at least one** visible declaration, every
     visible declaration Text, Label, Code or TextConst (parameters, named return, every local and
     global of that name, all `#if` arms included), the local level only when `locals_complete`, the
     global level only when `globals_complete`, and the expression NOT in the file's parens-less call
     set (the resolver's own variable-versus-call classification, already an input of `project_ir`;
     program_calls.rs:781-787); a call to `StrSubstNo`, `Format`, `GetLastErrorText`, `CopyStr`,
     `UpperCase`, `LowerCase`, `DelChr`, `ConvertStr`, `SelectStr`, `PadStr` or `IncStr` whose verdict
     (rule 3) is Builtin (untainted); a `+` concatenation whose operands are each proven Text by these
     rules; any of these in parentheses.
   - Anything else fails: members, index expressions, other calls, a field of the implicit `Rec` or a
     `with` frame, an extension's base-object `protected var` (not visible in the file).
3. **Bound to the builtin, by the resolver's own answer.** The program is resolved before the rows
   are assembled (`ResolvedProgram::build`, program_calls.rs:781). Its per-file result already reaches
   `project_ir` as the parens-less call set, keyed by expression; a second per-file set joins it: the
   **bare-call verdicts**, one per bare call to a name the rules read (`Error` and rule 2's catalog
   names), keyed by expression, taken from the resolver's own routes for that site. So nothing
   re-derives a site's arity, argument types, `WithState`, build context or dataitem table, and there is
   no second list of scopes. `project_ir` gains this input; both its callers (the workspace path,
   workspace.rs:3784-3811, and the dependency path, :3447-3486) have the program in hand. The resolver
   can return a `Catalog` route after a scope it could not search: an extension base it could not find
   (`ExtensionBase::Miss`), an implicit-`Rec` table it could not resolve, a `with` frame
   (resolver.rs:1894-1903, 2091-2102, 2136-2157). So the verdict is recorded **during resolution**,
   with the scope facts the resolver already has: `scope_complete` is true only when every scope before
   the catalog was searched in full (own object; the extension base found and not symbol-only, or no
   base; the implicit-`Rec` table resolved with its extensions and not symbol-only, or not applicable;
   `WithState::NoWithProven`). The verdicts are disjoint:
   - **Builtin**: exactly one route, a `Catalog` route, and `scope_complete`. Rule 3 holds.
   - **User**: exactly one route, a source or ABI routine (no ambiguity), in a scope that is not
     symbol-only. An ordinary call that returns (no may-raise).
   - **Unknown**: everything else: no route or an `Unresolved` route, several routes, a `Catalog`
     route without `scope_complete` (a missing or symbol-only base or table; a symbol-only scope wins
     over User too), the resolver's `BuiltinPrecedenceCollision` for a table-scope `Error`
     (resolver.rs:2064-2077), and any bare `Error` inside `with X do`. The call may raise (below).
   A bare call with no verdict (a graph-less build, an undemanded dependency row) is Unknown. Nothing is
   rewritten after resolution, and the verdicts agree with the edges by construction.

**When the rule fails (except User), the call is still a call, and it may raise.** The leaf stays a
`call` with its condition leaves (its arguments are evaluated on every run). It is followed, in the
same block, by a synthetic one-branch `case` (no scrutinee, no `else`, the group encoding) holding a bare
`error` leaf with no call site and no condition leaves. Both synthetic nodes take a **zero-width source
range at the call's end**, so `cfg_walker`'s source-order sort (cfg_walker.rs:449-478) puts them after
the call and before the next statement, and a zero-width range attributes no field access to them
(cfg_walker.rs:1629-1676; its containment test is inclusive, so the attribution helper skips empty
ranges explicitly, and a test pins an access at the call's end). Evidence that cites where a routine
may raise points at that range, which is the `Error` call's end.

**Expression position.** The builtin `Error` returns nothing, but a User-bound `procedure Error` may
return a value (`if Error('x') then Work();`). `IrCfn::harvest` names a nested call `error` by name today
(ir_walk.rs:2097-2110), so `literal_flow` kills the state and may prune `Work`. `harvest` uses the same
rule: a nested call is `error` only when `error_call_terminates` holds (it cannot, for a value), and
otherwise a `call`.

So every walker sees: the call runs, then the routine may raise, may continue. `cfg_walker` keeps the
raising exit state; `all_paths_error` is false; the call is `onSuccessPath` and the code after it may
dominate the success return. The control-context lattice reads the code after it as `conditional`
(the code after a `case` arm that always raises, control_flow.rs:119-122), not `top-level`: safe, and
pinned in B4.

`settle_rules` uses the same rule (its two-argument shortcut, rules.rs:1199-1235, goes through the
taint check too) and keeps its own message-value logic. Its declaration lookup (rules.rs:1434-1481)
replaces `object_clean` with the completeness rule: a global's message is used only when no tier that
could shadow it is incomplete (`locals_complete` false fails it too), and every declaration of the name
agrees on the message value. `settle_dead_mark_trust` and `source_dead_mark_trust` are deleted.

**Without a program context** (`aldump --l2`, the `ir-l2` golden, `l2_workspace.rs:186`), rule 3 cannot
answer Builtin, so every bare `Error` there is call + may-raise. The `ir-l2` snapshot moves accordingly;
the Model is the truth, and §6 measures the Model.

### 4.2 Lowerer: slots, unordered runs, taint, completeness

**S. Slots.** Every caller of `lower_branch` is a one-statement slot when given a single statement:
`if` then/else, `while`/`for`/`foreach`/`repeat` bodies, `with` bodies, `asserterror`, `case` branch
bodies (lower/mod.rs:3635, 3876, 4071, 4155, 4194; Task 1 confirms the list). A `#if` chain there is
**well formed** only when every live arm holds exactly one statement and the chain cannot fall through.
Otherwise:
1. each statement of each live arm becomes its own may-run item (a one-branch group), so `[exit, Foo]`
   kills nothing;
2. the nearest enclosing statement list (climbing through enclosing slots to a `statement_block` or
   `code_block` body) becomes an unordered run (B): in the broken build its next statement is the slot
   body (P1pNoArm, N2Nested).

**B. Unordered runs.** Every item of the run is wrapped in its own one-branch group that may fall
through ("may run, ends nothing outside itself"). The runs:
- `lower_code_block` with a split closing: every item of that code block (P8);
- the cond-only head's then-block (P9);
- `lower_unmodelled_stmt`'s leftover block, a reconstructed `If` with two heads included (I3, M8);
- the tail of a split procedure body whose live `#if` arm holds the guard (P7, P7Ctx);
- any statement-position `Preproc*` shape not handled above (default; Task 1 audits every kind).

**A. Split bodies.** `lower_preproc_split_routine_body` makes the prefix arms (`#if`/`#else` parts
before `#endif`) a group, and the shared tail follows as a real sequence (OnRun's `Call5` live, P6b's
`Call6b` dead). A complete-body split's arms are the group's arms.

**C. Taint and completeness**, IR fields read only by §4.1:
- `Ir::call_is_tainted(call)`: a preprocessor node in the callee or anywhere inside an argument, or the
  call built from `preproc_split_call_statement` (P13, S1Operand(Rev), S2Callee, I1SplitCall).
- `RoutineDecl::locals_complete`, `ObjectDecl::globals_complete`: false for split bodies' arm `var`
  sections and for `var` wrappers nested deeper than one level.

**D. Fail-closed remnant: an untrusted routine has no control tree.** A structured flag, not
diagnostic wording: `RoutineDecl::control_untrusted`, set by the lowerer on the default path of B (a
statement-position `Preproc*` shape no rule above models) and on depth-budget degradation (statements,
exits included, replaced by Unknown, lower/mod.rs:3550-3578); a `parse_incomplete` routine is untrusted
too. The shapes S, A and B model on purpose do not set it, and nor does a tainted call (C handles it),
so A1-A5's breaks are not masked.

An untrusted routine's features carry **no `statement_tree`**, decided once where the features are
built, so every path sees the same: the Model, the graph-less L2 projection, and the dependency-order
reconstruction from the tree (dep_artifact_l4.rs:488-520). The existing no-tree paths are NOT all
fail-closed, so the same place also sanitizes what does not come from the tree, and the readers whose
no-tree path makes a claim change:
- `unreachable_statements` is cleared (the IR spine fills it before the tree is built,
  ir_walk.rs:953-990; d20 reads it directly);
- every call and operation site keeps NO control context (`None`, the convention a `[TryFunction]`
  routine already has: the no-tree computation returns empty maps, control_context.rs:467-501). As
  built, this is safer than `conditional`: `context_to_conditionality(None)` is `unknown`
  (conditionality.rs:47-56), no reader treats a missing context as `top-level`, and `ordering_inter`
  needs an order entry, which a treeless routine has none of;
- `cfg_walker`'s straight-line fallback (`walk_flat`, cfg_walker.rs:255-306, 1752-1856) is not used for
  an untrusted routine: its path facts are Unknown (`dirty_at_exit` Unknown, no exit-state claim);
- `param_guard` returns no frame at all for it (not a frame with no requirements, param_guard.rs:102-115);
- `return_summary`, `dep_artifact_l4`, `operation_order` and `literal_flow` claim nothing without a
  tree (to confirm in the audit).
Task 1 lists every reader of `statement_tree`, `unreachable_statements` and the control contexts, with
what it does for an untrusted routine; any that still claims something gets a change in this list.
Parse-incomplete routines take the same path (today's `walk_flat` for them is the same unsoundness).
A6 asserts each.

### 4.3 Summary composition

`return_summary::walk_subtree` and `dep_artifact_l4`'s copy become one shared function with four
outcomes per subtree: may fall through; may return normally (`exit`); may raise; may leave the
enclosing loop (`break`, `continue`). A `block` composes them: only fall-through leads to the next
statement. A loop turns its body's loop-leaving outcome into its own fall-through (`repeat break;
Error('x'); until true` returns normally; a `break` is consumed by its own innermost loop only). A
`while`, `for` or `foreach` body may not run at all (`while C do Error('x')` falls through); a `repeat`
body runs at least once (`repeat Error('x'); until true` still raises on every path). `asserterror` turns its body's raise into fall-through; `with` is transparent.
`all_paths_error` is "may raise, and neither falls through nor returns normally". `if C then exit;
Error('x')` is then false.

### 4.4 d20

d20 reads `unreachable_statements`; with §4.1-4.2 they are right, so it needs no rule of its own. The
unreachable scan uses `error_call_terminates`, so quoted `"Error"` and the CFN agree.

### 4.5 Removing the trust gate

Removed: `dead_marks_trusted` and its literal constructors, `site_is_dead_marked`'s trust check,
`has_preproc_directive`, `source_clean` (its other user, the rules' `object_clean`, moves to
`globals_complete` plus the arm-aware declaration check), `source_dead_mark_trust`,
`error_argument_proves_text`, `is_bare_error_call`, `settle_dead_mark_trust`. `DeadSites::of` keeps
"dead when every row of the id that holds the site marks it unreachable". d35 drops its
trusted/untrusted split.

| shape the gate covered | closed by |
|---|---|
| statement `#if` chains | B2 groups |
| split bodies (prefix and guard), split closings, cond-only heads, two-head if-else, split case closings | §4.2 A, B |
| broken one-statement slots (all slot kinds) | §4.2 S |
| any directive shape the lowerer does not model | §4.2 D (fail closed) |
| unproven `Error` forms, `Error(ErrInfo)` under Collect | §4.1 (call + may-raise) |
| split arguments, operands, callees, split call statements | §4.2 C taint |
| a declaration chosen across arms; an incomplete declaration list | §4.1 rule 2 + §4.2 C |
| a bare `Error` bound to an own, base or table procedure; `with`; an unloaded scope | §4.1 rule 3 (User: plain call; Unknown: may raise) |
| parse-incomplete rows | §4.2 D (marks, summaries, dominance, literal pruning) |
| rows built without assembly (test literals, ABI rows) | no body, no mark: a mark exists only where a walker computed it; test literals that set one mean it |

### 4.6 Caches and incremental refresh

- No pack change: packs hold graph nodes only (`PackedFile`, pack/mod.rs:87-102; `RoutineNode` carries
  no body fact).
- **Row cache.** A cached row's key already holds the file's parens-less call set (row_cache.rs:50-108,
  `matches`). The bare-call verdicts (§4.1 rule 3) join it the same way: a row whose verdicts changed
  (a procedure `Error` added or removed elsewhere, a base or table loaded, a source table changed) is
  re-walked. Workspace and dependency rows alike; the verdicts come from the same resolution the
  session already refreshes.
- **On-disk caches, bumped in EACH landing that changes the facts they hold** (part 1 changes dead marks
  and dominance, part 2 summaries too):
  - `CACHE_VERSION_SUMMARY_SCHEMA` (cache_prune.rs:55; summaries hold `all_paths_error` and exit
    states) and `CACHE_VERSION_DEP_CACHE` (cache_prune.rs:58);
  - `DEP_ORDER_INDEX_SCHEMA_VERSION` (dep_artifact_l4.rs:59, "1"), which the dependency artifact's
    freshness check reads (dep_artifact_l4.rs:1091-1116);
  - the impact cache's own `SUMMARY_SCHEMA` (impact/keys.rs:74-76): its keys hold direct facts with
    unreachable sites removed and literal-dead filtering (impact/keys.rs:9-30).
  A test per cache shows an old-version artifact is rejected (C3).
- **Session:** C2 adds, then removes, a table-scope `Error` and compares with a cold build, in both
  memory modes.

## 5. Acceptance

Each through production assembly, each with one discrimination proof (break, fail, revert, pass;
recorded in the ledger). Probe names refer to `scratchpad/eng14/ws/src`; each becomes an inline test
fixture.

| # | requirement | planned break |
|---|---|---|
| A1 | OnRun (and a real complete-body split): `Call5` live; P6b's `Call6b` dead | splice the prefix arms as a sequence |
| A2 | P7, P7Ctx: `Call7` live | drop the guard check |
| A3 | P8: `Call8b` live | plain sequence for the split closing |
| A4 | P9, I3, M8, each with a statement after an `exit` inside the run: none dead | one break per shape: plain sequence |
| A5 | C1Then/C1Else/C1Loop, a `with` slot, an `asserterror` slot, a case-branch slot, P1oSpliced, N2Nested: `Foo` live, `Bar` not dominating; P1pNoArm: `Bar` not dominating | skip the slot rule (per slot kind) |
| A6 | an untrusted routine (a real default-path shape from the Task 1 audit, a depth-budget degradation, a parse-incomplete routine) has no `statement_tree` in the Model, in the graph-less L2 projection and in the dependency-order reconstruction; no `unreachable_statements` and no d20; every context `None` (unknown); `all_paths_error` false; no op dominating; `cfg_walker`'s `dirty_at_exit` Unknown for `Get; Validate; Error(EI); Modify`; no `param_guard` frame; no literal-flow pruning | one break per item: keep the tree; keep the vector; leave contexts empty; use `walk_flat`; return an empty frame |
| B1 | Text forms terminate (dead + d20): literal, parenthesized, Text/Label/Code/TextConst var, Code param, `Error(Lbl, 1)`, `StrSubstNo(...)`, `Format(...)`, `'a' + T`, quoted `"Error"('x')` | drop each accepted form in turn |
| B2 | not terminating (next call live, no d20): ErrorInfo var, `(EI)`, `GetErrInfo()`, parens-less `GetErrInfo`, `Errs[i]`, named ErrorInfo return, `Rec.Field`, P13, S1Operand, S1OperandRev, S2Callee, I1SplitCall | per form: accept any one-argument call; ignore the taint |
| B3 | declarations: conditional global in both orders, P15, split-body arm local, nested conditional `var`, a name with no declaration: not terminating; plain Text global: terminating | read the first declaration only; ignore completeness; accept zero declarations |
| B4 | `Get; Validate; Error(ErrInfo); Modify` on a record parameter: `dirty_at_exit` Yes (the raise keeps its exit state), `Modify` on the success path, dominating, control context `conditional`; `all_paths_error` false; a `var` side effect in the `Error` argument reaches the continuing path in `literal_flow`. The same exit-state assertion for `with Rec do Error('x')`, `Error('x')` in a page extension whose base is not loaded (the resolver returns `Catalog` there; `scope_complete` is false), and an arity mismatch with an own `procedure Error(A; B)` | make the unproven `Error` a plain call (exit state lost) / `error` (Modify dead) / put the call inside the case (argument lost) / drop the zero-width range (the raise sorts after `Modify`) |
| B5 | `Error(StrSubstNo(Lbl, X))` terminates; the same where an extension base declares `procedure StrSubstNo(...)`: does not; `Error(GetInfo)` (parens-less, a routine returns ErrorInfo): does not | look the inner call up by own object only; treat a name in the parens-less set as a variable |
| B6 | a User-bound `procedure Error(...): Boolean` in expression position (`if Error('x') then Work();`, `B := Error('x');`): `Work` not pruned, no `error` leaf | keep `harvest`'s name test |
| C1 | own `Error` (3a) and a table extension calling its base table's `Error` (User): next call live, no d20, `all_paths_error` false, no may-raise. A page, a page extension and a report dataitem calling the table's `Error` (the resolver's `BuiltinPrecedenceCollision`: Unknown): next call live, no d20, `all_paths_error` false, may-raise kept. Two matching user routes: Unknown | skip rule 3; read a collision as User |
| C4 | rules: a Label global plus a same-named local in a split-body arm (absent from `routine.locals`): the global's message is not used | read `globals_complete` only |
| C2 | session: add then remove a table-scope `Error`; equals a cold build, both memory modes. Row cache: a cached row whose verdicts change is re-walked | leave the verdicts out of `matches` |
| C3 | each bumped on-disk cache rejects an old-version artifact | keep the old version |
| D1 | `all_paths_error`, in `return_summary` and `dep_artifact_l4` alike. False: `if C then exit; Error('x')`; `repeat break; Error('x'); until true`; the same with `continue`; `while C do Error('x')`; `asserterror Error('x')`; `with R do exit; Error('x')` (`with` is transparent; a bare `Error` inside `with` is Unknown, so may-raise). True: `Error('x')` alone; `if C then Error('a') else Error('b')`; `repeat Error('x'); until true`; `repeat repeat break; until true; Error('x'); until true` (the inner `break` is consumed by the inner loop) | one break per outcome: drop it from the composition; treat a `repeat` body as maybe-not-run; let a `break` escape its own loop |
| E1 | the gate's names are gone (a source-scan test over `src/` code, comments excluded), and the #288 tests assert the marks themselves | (the rewritten tests carry their breaks) |
| F1 | CDO before/after (§6) | (measurement) |
| G1 | `lowerer-reviewer` pass with real parse output for every §4.2 shape; two-model review of the diff | (review) |

## 6. Measurement

On CDO (`cdo-measure`, base = master at the branch point): findings per detector at both scopes,
fingerprint churn, the histogram (must not move), and a census over the assembled Model (not `--l2`,
which has no assembly step): dead marks and `all_paths_error` removed per rule (§4.1 per failing
argument shape, rule 3, §4.2 S/A/B/D, §4.3), and bare `Error` sites by argument shape, so the precision
cost of §4.1 is known. Also counted: rows made untrusted (§4.2 D) by cause, and loops that now hold a
branch because of a synthetic may-raise `case` (d60's "is there a branch in this loop" walks `if`/`case`
nodes; B2 groups already do the same). Every moved finding is triaged against source.

## 7. Golden moves expected

`tests/ir-l2-goldens/l2_features.snapshot` (trees with may-raise cases and run wrappers), d20/d35
goldens where a fixture holds a directive or an unproven `Error` (`tests/fixtures/analyze-test-quality`,
`ws-builtin-shadow`), r4/r4f where `all_paths_error` moves, and CDO output and L4 goldens if a reached
routine moves. All regenerated together (`scripts/check-goldens --regen`), every line read.

## 8. Tasks and landing

Two landings, one branch each:
- **ENG-14 part 1 (lowerer):** Task 1 (probe audit: complete-body probe, every statement-position
  `Preproc*` kind against parse output, the `lower_branch` callers, every reader of `statement_tree`
  with its no-tree path), Task 2 (§4.2 S, A, B, D with A1-A6), the cache bumps of §4.6 that its
  changes need (C3), `lowerer-reviewer`, CDO measure, CHANGELOG. Measurable alone. (As built: §4.2 C
  moved to part 2, the first change that reads it, so part 1 adds no unread IR field.)
- **ENG-14 part 2 (Error and gate):** §4.2 C, §4.1 (B1-B6, C1, C2, C4), §4.3 (D1), §4.4, §4.5 (E1),
  §4.6 (its own bumps, C3), CDO measure and triage, engine-limits.md and CHANGELOG.
- **ENG-74 (filed separately):** `CurrReport.Skip`/`Break`/`Quit` (and `currXMLport.*`), after a BC
  sidecar measurement per method, trigger and helper call: what runs after it in the trigger, in the
  caller, and of `OnPostReport`. Until then the engine keeps today's reading, and the limit is stated.

## 8.1 As built (part 1), where it differs from the text above

- **§4.2 S and B: where arm boundaries are lost, the routine is untrusted (fail closed).** The code
  review of part 1 (sol) showed that an unordered run removes false DEAD marks but keeps false GUARDS:
  in `if Flag then #if X exit; Commit(); #endif Work();` the slot's statements stay inside `if Flag`,
  though under X `Commit` runs whatever `Flag` is, so `param_guard` records `Flag = true` and a caller
  passing `false` prunes the commit. A split closing's else-arm statements (in the then-block) and a
  cond-only head's statements (under the first build's condition) have the same defect. A broken slot
  inside a statement `#if` arm was also not consumed by the arm. So a broken slot, a split closing, a
  cond-only head and a `preproc_*` leftover now push a `StmtKind::Unknown` marker next to their lowered
  items: the routine is untrusted (§4.2 D), its calls keep their sites, and no mark, guard or dominance
  comes from its tree. This replaces S's enclosing-list rewrite and B's runs on those paths. Kept precise,
  because no false guard can arise there: statement groups (B2), split-body arms (A), and a guarded
  split body's tail (an unordered top-level run).
- **A `case` with a `#if` group among its branches fails closed too** (sol, part-1 review round 2). Its
  labels union across builds and its `else` is a real else, so `0: …; #if X 1: …; #else else Commit();
  #endif` let literal reachability rule the commit out for `P(1)`, though without X it runs. An earlier
  answer (the conditional `else`'s statements as may-run items) removed the false "always" but not that
  false guard; it is withdrawn. `lower_case_body` adds a pattern-less branch holding the marker when a
  `#if` case group (`PreprocConditionalCase`, `PreprocSplitCaseExtended`, `PreprocSplitOpenStatement`)
  is among the branches, and case groups nested past the depth budget (whose deeper branches are
  dropped) add one too.
- **Every place a `#if` can sit inside a `case`** (from the grammar's `node-types.json` for
  `case_statement`, `case_body`, `case_branch`, `case_else_branch` and the case-group nodes; one test row
  each in `eng14_case_rows`, plus `literal_flow_tests::eng14_case_labels_from_a_group_prune_nothing`):

  | # | place | node kinds | reading |
  |---|---|---|---|
  | 1 | the header (`case <expr> of`) | `preproc_conditional_operand` (field `expression`), `preproc_conditional_expression_tail`, `preproc_operand_prefix` | fail closed (`RowHeader`) |
  | 1b | nested in the header expression (round 4, sol and Fable) | any `preproc_*` below the header: `parenthesized_expression`, `unary_expression`, `additive_expression`, … hold `preproc_conditional_operand` and tails | fail closed: the whole header subtree is checked (`holds_preproc`; `RowHeaderNested`) |
  | 2 | between branches | `preproc_conditional_case`, `preproc_split_case_extended`, `preproc_split_open_statement` | fail closed (`RowGroup`, `RowExtended`, `RowOpen`; literal reach `CallGroup`) |
  | 3 | a branch's labels | `preproc_split_case_branch`, `preproc_conditional_case_patterns`, `preproc_operand_prefix`, a nested `preproc_conditional_case`, any other `preproc_*` child of a `case_branch` but its body | fail closed (`RowLabelsSplit`, `RowLabelsTail`; literal reach `CallLabels`) |
  | 3b | nested in a label expression (round 4) | any `preproc_*` below a label (`(#if X 1 #else 2 #endif):`, a `range_expression`'s ends, a unary minus) | fail closed: every non-body child of a branch is checked as a subtree (`RowLabelNested`) |
  | 4 | a branch's body (a one-statement slot) | the statement-level kinds | the statement rules: one statement in every build is precise (`RowBodyWellFormed`), otherwise fail closed (`RowBodyBroken`) |
  | 5 | the `else` | none of its own (its body is statements); a second `else` only from another arm | the statement rules; a second `else` is kept as a pattern-less branch so its calls keep their sites (`RowTwoElses`), and row 2 has already failed the case closed |
  | 6 | a nested `case` | any row above, inside a branch | the same rules per case (`RowNested`) |
  | 7 | the whole case split across `#if` | `preproc_split_case_statement_end`, `preproc_split_case_end_branch` (statement level) | fail closed (`SplitCaseEnd`) |
- **`has_branching`.** An untrusted routine reads `has_branching: true`: `record_types` (RecordRef
  temp state), `event_flow` (`exact`) and d61 (`unconditional`) treat `false` as a straight-line proof
  (Fable, part-1 review).
- **The split-body guard's condition** is lowered (`If { cond, then: empty }` in its arm), so a call in it
  has a site; before, it was dropped (pre-existing).

- **§4.2 C moved to part 2.** Taint and completeness are read only by §4.1's `Error` rule; adding the IR
  fields in part 1 would land fields nothing reads (mantra 13). Part 2 adds them with their first reader.
- **§4.2 D, the trigger.** "Untrusted" is `parse_incomplete`, or a body that holds a `StmtKind::Unknown`
  at any depth (`ir_walk::block_has_unknown`, an exhaustive match over statement kinds). That covers the
  depth budget, an unmodelled wrapper that lowers to nothing, and a split call with no callee, with no new
  lowerer flag. The unordered runs of §4.2 B never produce `Unknown`, so they do not trigger it.
- **§4.2 D, contexts.** An untrusted routine's sites keep no context (`None`), not `conditional` (see
  §4.2 D's bullet).
- **§4.2 D, readers changed.** `cfg_walker::walk_param` (Unknown facts), `param_guard::frame_guards`
  (`None`), d43 `classify_subscriber` (`MaySetTrue`, before its no-branch fast path). Every other reader's
  no-tree path was checked safe: docs/superpowers/notes/2026-11-14-eng14-tree-readers.md.
- **§4.6, caches (checked in the code).** Only the impact cache is a live on-disk cache that part 1
  moves: its keys hold direct facts with unreachable sites removed, so `impact::keys::SUMMARY_SCHEMA`
  goes 13 → 14 (the key hashes it, impact/cache.rs:96). `CACHE_VERSION_SUMMARY_SCHEMA` and
  `CACHE_VERSION_DEP_CACHE` are read only by `cache_prune`'s classifier of old al-sem files; nothing
  writes an artifact stamped with them, so bumping them protects nothing. The dependency artifacts and
  their `DEP_ORDER_INDEX_SCHEMA_VERSION` stamp are built in memory per run and compared in the same
  run, never against an older binary's file. C3 needs no new test: the key mechanism is unchanged, and a
  version value needs no discrimination proof (owner decision 2026-11-04).
- **§4.2 B, two heads.** The "one head" condition was dropped: a second head's fields are never consumed,
  so a two-head shape always has leftover items and is an unordered run anyway (its discrimination proof
  came back green, which showed the check was redundant).

## 8.2 As built (part 2, ENG-80), where it differs from the text above

- **The L2 projection reads the resolver's verdicts.** §4.1 said a graph-less path reads every bare
  `Error` as may-raise. Built that way, 14 golden tests failed, 12 of them hand-written L2 vectors and
  oracles (`tests/l2_ir/l2_{cc,order,cap}_*`, `l2_vectors`) whose fixtures test the semantics of a
  terminating `Error` (the error-path context, the code after it dead, frame termination). Rebaselining
  them would have left those code paths pinned nowhere. Instead `l2_workspace::project_workspace`
  (`aldump --l2`, the r1a goldens) and the single-routine entries (`ir_features_for_named_routine`,
  `project_named_routine`) build the program for the same inputs and pass each file ONLY its bare
  verdicts (`workspace_bare_verdicts`; the parens-less set stays empty, as L2 never read it). L2 now
  reads a bare `Error` as the Model does. Only `ir_l2_snapshot`, which calls
  `project_routine_features_ir` directly with no program, keeps the graph-less reading (51 routines
  move, every one holding a bare `Error`).
- **One reading, rules included.** `ErrorReading::MayRaise` carries why (`Unproven::{Binding,
  BuildVarying, Text}`), and the rules take `stops` from it at assembly (`RuleInputs::error_readings`).
  `RuleDraft::builtin_sites` and the post-resolution half of `settle_rules` are gone; a new reason names
  an `#if`-varying call.
- **Text proof scope order.** The innermost level that declares the name decides (parameters, the named
  return value, locals, globals), and every declaration there must be Text-family; reaching a level the
  lowerer did not read in full proves nothing. (A first version required every declaration at every
  level to be Text, which an ErrorInfo global shadowed by a Text named return refuted: G11u's control.)
- **A user binding must exist in every build (review F1).** `resolve_in_object` narrows candidates by
  build only when two or more remain, so a lone `#if X local procedure Error` is bound from a call
  outside the arm. `BareVerdict::of` therefore gives `User` only when some copy of the target routine
  carries no build condition the call site's own context lacks; otherwise `Unknown` (may raise). The
  same holds for the inner Text calls (`StrSubstNo` …). The mirror (a call only in builds where the
  procedure is absent) also reads `Unknown`: safe, a precision limit (engine limits L38).
- **An undecided condition is kept whole (review r2).** A `#if` condition that decides no symbol
  (`X or Y` true, `X and Y` false) used to leave the context empty, which reads as "every build", so
  a lone `procedure Error` under it read `User` everywhere. `assume_preproc` now records it as one
  entry named by its text (`("(X or Y)", true)`), which contradicts only its own opposite.
- **Table scopes read their extensions (review F2).** The implicit-`Rec` table, and a table extension's
  base, count as searched only when neither the table nor any extension of it is symbol-only
  (`full::table_code_not_loaded`, ENG-16's rule): a symbol-only extension's symbols may not list a
  `procedure Error`.
- **Guards with no discriminating fixture (redundant for every modelled shape, kept as defence):** the
  extension-base clears of `scope_complete` (a page or report extension's implicit `Rec` is its base's
  table, and ABI pages carry no `SourceTable`, so the out-of-closure clear already fires; a table
  extension's implicit `Rec` is its base table), the inner-call taint check (an `#if` inside an inner
  call taints the outer `Error` through the subtree scan), and the quote-character check in the Text
  proof (a quoted name matches only a quoted declaration).
- **`aldump --l2` warns** on stderr when the program build fails and every bare `Error` falls back to
  "may raise" (review F4); a test pins that the program's path keys equal the discovered paths.
- **C3** holds by construction (§8.1's reasoning; the schema goes 14 → 15).
- **Parens-less guard in `proves_text`.** No fixture can name a variable and a parens-less call alike
  (a local or global sharing a procedure's name shadows it), so the guard has no discrimination proof;
  it stays as defence.
- **Found on the way:** a CR-only source file with a `#if` parses as one root ERROR (grammar), filed as
  GRM-03. The #288 CR-only test (it tested the deleted directive scan) is removed.

## 9. r0 review: how each finding is answered

| finding | answer |
|---|---|
| both: B2's group is a `case`, not an `if` | §0 corrected; §4.1 uses the `case` encoding |
| sol 2: the may-raise leaf must not make the arguments optional | §4.1: the call stays a call, the raise is a separate synthetic case after it; B4 breaks |
| Fable B2: zero declarations prove Text | §4.1 rule 2: at least one; no same-named routine; B3 row |
| Fable B3, I3; sol 3: post-resolution rewrite drifts and is not reversible | no rewrite: rule 3 is decided at assembly from the program graph; resolver contract test; C2 session test |
| Fable B4: pack schema | packs hold no body facts (checked); the dep-artifact cache key is bumped instead (§4.6) |
| both: §4.4 assumes BC behaviour; helper `Skip` makes a wrong dominance | split out to ENG-74; §1 narrowed with the limit stated |
| sol 1: `exit; Error` gives `all_paths_error: true` | confirmed (return_summary.rs:121-141); §4.3, D1 |
| sol 1: split-body prefix arms | §4.2 A, A1 |
| sol 5: keep a fail-closed fallback; `source_clean` has another user; parse-incomplete as policy | §4.2 D; §4.5 (rules move to completeness); A6 |
| Fable I1: precision cost | catalog Text builtins and concatenation proven (§4.1); census per argument shape (§6) |
| Fable I2: `with` | §4.1 rule 3 fails inside `with`; C1 |
| Fable I4, sol 6: other slots | §4.2 S lists every `lower_branch` caller; A5 per slot kind |
| Fable I5, sol 6: acceptance gaps (quoted `Error`, A4 breaks, grep is no test, C1 summary) | B1 quoted form, A4 per shape, E1 a test, C1 asserts `all_paths_error` |
| sol 6: B4 weak; D1 receiver check | B4 now a record parameter with `dirty_at_exit`; D1 moved to ENG-74 |
| sol minor: rules' 2+-argument shortcut | §4.1 last paragraph |
| both: defer P14; split §4.4; land §4.2 first | §2, §8 |

### 9.1 r1 review: how each finding is answered

| finding | answer |
|---|---|
| both (blocker): a rule-3 failure loses the raise unless a user target is proven | §4.1 rule 3: Builtin / User (matching arity: plain call) / Unknown (may raise); B4 rows for `with`, unloaded base, arity mismatch |
| both: rule 3 re-lists the resolver's scopes and misses some (report dataitems, page extensions, table ∪ extensions); symbol-only scopes | rule 3 calls `resolve_bare_with_args` itself; symbol-only counts as Unknown; C1 rows per scope |
| sol: inner catalog calls need a binding proof | rule 2's catalog calls and the parens-less name go through rule 3's lookup; B5 |
| Fable I1: contract vacuous for undemanded rows | scoped to rows the resolver resolved |
| both: §4.2 D must also stop dominance (and literal pruning); a structured flag, readable by the summary walkers | `control_untrusted` on `RoutineDecl` and `ModelRoutine`; four readers; A6 per reader |
| sol I3: D masks A3/A4's breaks | D is set only by the default path, not by modelled shapes or taint |
| sol B3, Fable: `break`/`continue`, loops, `asserterror`, `with` in the summary | §4.3 four outcomes; D1 rows |
| sol I1, Fable I5: the synthetic nodes' position and location | zero-width range at the call's end; `harvest` needs nothing (`Error` is a statement) |
| both: the graph-less L2 path | §4.1 last paragraph: may raise without a context; the snapshot moves |
| sol I2, Fable I3: caches | §4.6: row-cache lookups recorded and re-run; the three named versions bumped; C3 |
| Fable I5, sol minor: code after an unproven `Error` is `conditional` | stated in §4.1; pinned in B4 |
| Fable minor: A6 hook, E1 comments | A6 uses an audited real shape; E1 excludes comments |
| sol minor: rules' message-value agreement | §4.1: rules keep their own value logic; only `object_clean` is replaced |

### 9.2 r2 review: how each finding is answered

| finding | answer |
|---|---|
| Fable edit 1: feed rule 3 the resolver's result, not a second lookup | §4.1 rule 3: per-file bare-call verdicts, keyed by expression, passed into `project_ir` like the parens-less set |
| sol I1: "Unknown with no route" is not the resolver's absence result | rule 2 reads the parens-less set (the resolver's variable-versus-call classification); the verdicts treat `Unresolved` as Unknown |
| sol I4: cache replay inputs incomplete | gone: the verdicts are compared in the row cache's `matches` (§4.6) |
| sol B1: a User-bound `Error` in expression position | §4.1 "Expression position": `harvest` uses the same rule; B6 |
| Fable edit 2, sol B2: untrusted rows still prove things | §4.2 D: no `statement_tree` for an untrusted routine, decided where features are built; depth degradation sets the flag; A6 across Model, graph-less L2, dependency order |
| sol I2: C1 contradicts rule 3 for table-scope collisions; ambiguity | C1 split: User vs `BuiltinPrecedenceCollision` (Unknown, may-raise kept); verdicts disjoint (exactly one route for User) |
| sol I3: rules miss incomplete local shadowing | §4.1 last paragraph: every shadowing tier must be complete; C4 |
| sol I4: impact cache; ship invalidation with each landing | §4.6: impact `SUMMARY_SCHEMA` named; each landing bumps what it changes |
| sol minor: inclusive containment for zero-width ranges | §4.1: empty ranges skipped explicitly, pinned by a test |
| sol minor: D1 rows | `continue`, inner-loop `break`, `with`, `repeat Error` added |
| Fable: symbol-only vs ABI User | symbol-only wins (Unknown) |
| Fable: d60 branch side effect | §6 census |

### 9.3 r3 review (sol): how each finding is answered

| finding | answer |
|---|---|
| B1: the no-tree paths are not all fail-closed (d20's vector, `walk_flat`, empty contexts, empty `param_guard` frame) | §4.2 D lists the sanitization and the reader changes; the audit must produce changes, not only confirm; A6 one assertion and one break per item |
| B2: a sole `Catalog` route can follow a scope the resolver could not search | §4.1 rule 3: `scope_complete` recorded during resolution; Builtin needs it; B4's unloaded-base row |
| I1: two D1 expectations were wrong | D1 corrected: `with R do exit; Error('x')` false; the inner-loop `break` case true |

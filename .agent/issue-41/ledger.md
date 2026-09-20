# Issue #41 — action-extension triggers have no enclosing member

## Claim

```json
{"run_id":"20260920-013249-7d1c12","issue":41,"attempt":1,
 "session":"https://claude.ai/code/session_01Udxt1D5qya52q1HFXUzARp",
 "branch":"issue/41-agent-discovery-al-syntax-a-page-a1",
 "worktree":"U:\\Git\\al-sem-issue-41-a1",
 "body_hash":"178916f42a8cb0b8858141ebdb8cff6b896d7c576bbef0c5bf88a37ad97ee566"}
```

Worktree created from `master` @ `20a92992`. Reviewers: pi's only provider returns
`429 quota exceeded` for every model (probed `gpt-6-astra` at 2026-09-19T23:33Z), so this
issue runs with the OPERATOR-DIRECTED stand-ins — fable in astra's slot, opus in flash's —
recorded in `.agent/runs/20260920-013249-7d1c12/notify.log` and, at merge time, in the
attestation's `reviewers` field via `attest --substitute`.

## Classification

**BOUNDED.** The fix already exists in this file for the sibling node: `modify_modification`
was given exactly this treatment (an explicit `collect_routines` arm plus a `target`-field
fallback in `lower_routine`). The action-section node has the identical grammar shape. The only
open question is blast radius, which probe P4 answers.

## Assumption probes

| probe | result | evidence |
|---|---|---|
| P1 — `modify_action_modification` exists as a named node with a `target` field | **HOLDS** | `tree-sitter-al/src/node-types.json`: `modify_action_modification fields={'body': ['declaration_body'], 'target': ['identifier','quoted_identifier']}` — byte-identical field shape to `modify_modification` |
| P2 — the lowerer does not treat it as a member wrapper | **HOLDS** | `collect_routines` has an arm for `RawKind::ModifyModification` only (`lower/mod.rs:630`); the generic `_` arm requires `child.field(FieldName::Name)`, and this node has no `name` field, so `member` is inherited unchanged (and the enclosing `actions` section has no name either) → `enclosing_member: None` |
| P3 — only these two `modify_*` kinds carry the modified member's identity | **HOLDS** | All 26 `*_modification` node types enumerated from `node-types.json`: only `modify_modification` and `modify_action_modification` are `modify_*`. Every `add*`/`move*` `target` is an insertion ANCHOR naming a DIFFERENT member, exactly as `lower_routine`'s existing comment states |
| P4 — no existing fixture has `modify(` inside an `actions` block | **HOLDS** | Scanned every `tests/**/*.al` for an `actions { … }` block containing a `modify(` — zero hits. So the identity change the issue warns about moves NO existing golden; only a new fixture would, and this change needs no corpus fixture (see Design) |
| P5 — `in_dataset_modify_context` cannot be switched on by this change | **HOLDS** | `lower/mod.rs:594` gates that flag on `m.kind() == RawKind::ModifyModification` specifically. An action modification is a different kind, so the flag stays `false` — which is correct: an `actions` `modify()` is never a report-dataset modify |

## Design

Two edits, mirroring the `modify_modification` pair that already exists:

1. `collect_routines` (`crates/al-syntax/src/lower/mod.rs:630`): the arm becomes
   `RawKind::ModifyModification | RawKind::ModifyActionModification`. Both are named member
   wrappers whose name lives in `target` rather than `name`, so both must bypass the generic
   `Name`-field gate.
2. `lower_routine`'s enclosing-member fallback (`:713`): the `target` fallback matches both
   kinds. The existing comment already explains why the fallback must NOT extend to the
   `add*`/`move*` siblings; that reasoning is unchanged and P3 re-verifies it.

Nothing else changes. `in_dataset_modify_context` keeps its `ModifyModification`-only gate
(P5), so an action modify never claims dataset context.

**No corpus fixture.** Acceptance is about the lowerer, and the lowerer's own `#[cfg(test)]`
module is where the three sibling cases are already pinned. P4 shows no golden carries this
shape, so adding a corpus fixture would only manufacture golden movement without proving
anything the unit tests do not. Stated as a deliberate scope decision, not an omission.

**Tests** (in `crates/al-syntax/src/lower/mod.rs`'s test module, beside the existing three):

- a pageextension `actions { modify(MyAction) { trigger OnAction() … } }` → `enclosing_member`
  is `Some(("MyAction", origin))` and `in_dataset_modify_context` is `false`;
- a quoted target (`modify("My Action")`) → the unescaped name, matching how
  `modify_modification` handles quoting;
- a CONTROL: `addlast(actions) { action(NewAction) { trigger OnAction() … } }` → the enclosing
  member is `NewAction` (the action the trigger is really in), NOT the anchor `actions`
  target — this is the guard against extending the fallback to the `add*` family.

## Spec panel, round 1 — both stand-ins, and what they broke

Reviews: `.agent/runs/20260920-013249-7d1c12/fable-spec-41.md` (astra slot),
`opus-spec-41.md` (flash slot). Twelve register entries, two of them critical. Both reviewers
independently found the same two blocking problems, from different directions.

**Every code claim below was re-derived by the conductor before it was accepted**, and one
reviewer number was NOT adopted (see A5).

### The two blocking findings

**A1 — the control test could not fail.** `addlast(actions) { action(NewAction) { trigger … } }`
puts a NAMED `action_declaration` between the anchor and the trigger, and the generic name gate
makes that the member, so the test passes on master and passes under the very mutation it was
supposed to catch. Confirmed with a real `tree-sitter parse`: an `addlast_action_modification`
body admits a `trigger_declaration` as a DIRECT child. The control becomes that shape.

**A2 — the design shipped half the issue and falsified a comment.** The issue's Capability is
"so they classify AND identify like any other member trigger". The two lowerer edits deliver
identify only: `is_page_action_wrapper` (`src/engine/root_classification.rs:181-189`) matches
four `*_declaration` strings, so the trigger still never gains `page-action`. Worse, the KNOWN
GAP paragraph at `:176-180` explains that a fifth string "could not" work BECAUSE "the lowerer
never captures it as an enclosing member at all" — a sentence this very commit makes false.
Resolution (a), both reviewers' recommendation, is taken: add the fifth string, pin it with a
hand-stated test, and rewrite the paragraph in the same commit.

### Design v2 (supersedes the Design section above)

1. `collect_routines`: arm becomes `ModifyModification | ModifyActionModification`.
2. `lower_routine`'s fallback: matches both kinds, AND filters an empty target (A6) — a
   malformed `modify()` parses with a present, zero-width `target`, and `Some("")` takes the
   discriminated arm of `to_stable_routine_id_from_parts`, minting a different, meaningless
   stable id. One guard covers both kinds; `dataitem_table_name` already filters for exactly
   this reason.
3. `is_page_action_wrapper`: add `"modify_action_modification"`; rewrite the KNOWN GAP
   paragraph to say the gap is closed (A2).
4. Source comments corrected: which of the two gates is load-bearing (A10), and WHY the `add*`
   family stays out — not "their target is an anchor naming a different member" (false for
   `add`/`addfirst`/`addlast`, whose target is the CONTAINER, and for the two `views` forms,
   which have no required target) but "an `add*` body declares its own named members, so a
   routine inside finds its declaring member through the ordinary name gate" (A4).

**Tests** — lowerer module (`crates/al-syntax/src/lower/mod.rs`):
- action `modify(MyAction)` + `trigger OnAfterAction()` → `Some(("MyAction", origin))`;
- quoted, doubled-quote target → the outer-stripped, still-escaped IR text (A8);
- CONTROL: `addlast(Processing) { trigger OnDirect() … }` → `None` (A1);
- empty `modify()` → `None`, not `Some("")` (A6).

`src/engine/root_classification.rs`: an action-modification page trigger gains `page-action`
(hand-stated `PAnchor`, so it survives any lowerer refactor).

`tests/cli/cli_p1_enclosing_member.rs`: two action `modify()` blocks in one pageextension, each
with `OnAfterAction`, get DISTINCT stable routine ids and distinct enclosing members (A3) — the
de-collision that is this issue's real product value. Fails on master today (both collapse).

### Blast radius, measured rather than asserted (A5)

| scope | result |
|---|---|
| every `.al` and `.rs` under the worktree | no ACTION modification of any kind. (Correction, final panel C7: an earlier draft said all 52 `modify(` hits are `Rec.Modify()` calls — false. `tests/r0-corpus/ws-report-dataitem/RDExt.ReportExtension.al:16` is a real dataset `modify(Cust)` block. Its target is non-empty, so neither the new arm nor the new filter touches it, and the conclusion is unchanged.) |
| pinned CDO baseline (582 `.al`) | 246 `actions` sections, 113 action-`modify` blocks, **0** containing a trigger or procedure; 0 `add*` blocks with a trigger as a DIRECT child |
| dependency ABI | structurally out of reach: `deps/projection.rs:215` and `deps/cross_app_l3.rs:303` hardcode `enclosing_member: None` and never run the lowerer |

So no golden family moves and `cdo-gate` is dormant for this change — but the de-collision in A3
means a workspace that DOES use the shape gets new routine ids, which re-fingerprints any
committed `alsem` baseline covering it. That goes in the CHANGELOG (A12).

**One reviewer number rejected.** fable reported 128 action-`modify` blocks on CDO, opus 113.
The conductor's own brace-matched, comment- and string-stripped scan says 113; fable's figure is
not adopted. (The conductor's first scan said 114 with one trigger-bearing block — its own bug: a
`Modify();` statement matched, and the block-finder then grabbed the next unrelated brace.)

## Implementation (commit `d99d8e16`) and the gates

Implementer report with the full RED output, GREEN output and proofs D1-D7:
`.agent/runs/20260920-013249-7d1c12/impl-41.md`. Tests were written first and the red run
captured; T3 (the control) and T4 are green in the red run BY DESIGN and the report says so —
T4 only becomes load-bearing once the arm lands, which is what D4 proves.

Four gates on `d99d8e16`, all through the executor, `"supervised": true`, none timed out:
`ci-steps all` 0, `check-goldens --verify-coverage` 0, `check-goldens` 0, `cdo-gate` 0.

## Final panel — a code review plus both stand-ins

Step 10 says "run the `code-review` skill". **That skill does not exist in this checkout** —
which is issue #37, already filed. As on #23, the `code-reviewer` AGENT type was substituted;
this is recorded rather than allowed to read as the specified step having run.
Reviews: `.agent/runs/20260920-013249-7d1c12/cr-41.md`, `fable-final-41.md`, `opus-final-41.md`.

Both stand-ins accepted all seventeen A/B entries against the code as committed, and both
audited the discrimination evidence rather than the prose: opus re-hashed the three touched
files and confirmed they are byte-identical to the implementer's stated post-`rustfmt`
baseline, so no break survived into the commit.

**Three findings were worth a test, and each one is the same shape: a rule stated in a comment
that nothing executes.**

| # | finding | test added | proof |
|---|---|---|---|
| C1 | the "do not fold the `in_dataset_modify_context` gate" rule was a comment only, and the shape it warns about parses clean | `action_modify_nested_in_a_dataset_modify_is_not_dataset_context` | **D8**: folding the gate to both kinds fails it, exit 101 |
| C2 | both empty-target tests reach the guard through `target`; the `name` half B3 raised it for was unpinned | `empty_action_declaration_name_degrades_to_no_member` | **D9**: narrowing the filter fails THAT test and only that test (2 passed, 1 failed) |
| C3 | the CHANGELOG's headline claim (the `RoutineNodeId` de-collision) was pinned nowhere — the existing test covers a different function | `enclosing_member_discriminates_the_program_routine_node_id` | **D10**: dropping the member from `source_routine_node_id` fails it, exit 101 |

All three breaks asserted they applied (`count(FROM) == 1` before, absent after, bytes changed)
and each file was restored under a sha256 that matched its pre-break hash.

**One reviewer proposal was REFUTED (C9)** — extracting a shared `is_modify_member_wrapper`
helper. Both stand-ins independently said drop it: the helper must not be used at the third
look-alike site, so a named predicate whose correct use excludes one of three call sites
advertises the fold risk instead of removing it. C1's test enforces that site executably.

**One reviewer claim was CORRECTED rather than adopted (C6).** The code review said a
zero-width-name wrapper "stops being a root, and reachability-derived findings can flip".
It does not: `kinds_for` inserts `trigger-page` unconditionally. It loses `page-action`, the
evidence containment range and the inventory tie-break. opus caught this in the same round and
the register carries the corrected consequence, so a future reader does not chase a
reachability regression that cannot happen.

**A grammar defect fell out of C5** and is filed as a discovery: `tree-sitter-al` accepts a
zero-width `identifier` in a REQUIRED field (`modify()`, `action()`) with no `ERROR` and no
`MISSING`, so `has_error()` is false and the engine reports the file as cleanly parsed. We own
that grammar. The engine-side guard is needed either way and does not depend on the fix.

## Final panel rounds 2 and 3

**Round 2** ratified the fix batch: all of C1-C4 and C6-C9 accepted by both stand-ins, with C9's
refutation upheld. **C5 was re-raised, correctly** — the fix had been one file short, leaving the
`[Unreleased]` CHANGELOG bullet still saying "tree-sitter recovery can leave zero-width", the very
claim C5 had falsified. Same shape as B2. Corrected, plus three nits (D1n-D3n).

**Round 3 re-raised D1n, and both stand-ins were right.** My "fix" for the second assert message
used a `
` ESCAPE plus 13 spaces instead of a line continuation, so it still printed a hard line
break mid-sentence — and the register entry claimed both messages were fixed AND verified by D11.
That was false twice: D11 prints the D3n precondition message, not this one. Recorded in D1n's
evidence rather than quietly amended, because it is the over-claim this repo's testing rule names.

Both messages are now read back FROM THE RUNNING TEST rather than eyeballed — each break makes its
assert fire and the printed text is captured:

| assert | break that fires it | message as printed (backticks dropped for this table's markdown) |
|---|---|---|
| nested/actions | D8 (fold the gate) | `an ACTIONS-section modify is never report-dataset context, however it is nested -- if this fails, the in_dataset_modify_context gate has been widened to both modify kinds` |
| zero-width name | D9 (narrow the filter) | `a zero-width name must degrade to None, never Some("") -- the filter must stay on the whole enclosing_member, not inside the target fallback` |

Both runs exit 101 and both files byte-restore to their pre-break sha256.

| proof | break | result |
|---|---|---|
| D11 | force `in_dataset_modify_context` permanently false | the new PRECONDITION assert fails (exit 101) — the sibling trigger directly in the dataset `modify()` must BE dataset context |

**The final-panel cap (3 rounds) is now spent**, with D1n fixed but unmarked: a reviewer's mark
cannot be inferred by the conductor (the rule this session learned the hard way on #30's FF5).

## Round 4 — operator-authorized, D1n only

The cap was spent with D1n re-raised by both stand-ins. A conductor may not infer a reviewer's
mark (the #30 FF5 lesson), so the choice was `blocked final-panel-cap` or ask. **The operator
authorized one extra round, scoped to D1n alone.** Both accepted: no `\`-n escape followed by a
space run remains on any Rust line this issue adds (checked across the whole diff, not just the
hunk), the continuation keeps the space BEFORE the backslash so the words do not run together,
and the evidence records the over-claim instead of amending it away.

Two cosmetic wobbles they flagged in MY records are fixed here, neither in an assert message: the
D1n evidence sentence contained a real newline at the exact word where it meant to NAME the
escape, and the proof table's "as printed" column had markdown backticks stripped.

## Final state

B = `20a92992` (origin/master at branch creation), H = `80e89f1f`.

| gate (clean cycle, on H) | exit | seconds |
|---|---|---|
| `ci-steps all` | **0** | 654.4 |
| `check-goldens --verify-coverage` | **0** | 0.2 |
| `check-goldens` | **0** | 286.5 |
| `cdo-gate` | **0** | 360.0 |

All four `"supervised": true`, none timed out, and `git status --porcelain -- . ':!.agent'` empty
afterwards. An EARLIER cycle on this same head reported `check-goldens` exit 101 — that run was
invalid and is recorded rather than hidden: I ran discrimination proofs in the same worktree while
it was building, and two cargo builds sharing one target directory produced
`error: linking with rust-lld.exe failed`, not a test failure. The cycle above is a clean re-run
with nothing else touching the tree.

Register: 29 entries, every one accepted by both stand-ins, none open, no blocking entry deferred,
one refuted with evidence (C9). Reviewers: pi returned 429 for every model on both days, so the
attestation records `astra=fable`, `flash=opus` via `--substitute`.

## Discoveries

One, filed after merge: `tree-sitter-al` accepts a zero-width `identifier` in a REQUIRED field
(`modify()`, `action()`) with no `ERROR` and no `MISSING`, so `has_error()` is false and the engine
reports the file as cleanly parsed. We own that grammar. The engine-side guard added here does not
depend on the fix.

## Not delivered

- Nothing from the issue's Acceptance. Both halves — capture as a member wrapper, and extract the
  name from `target` — are delivered, and the classify half (`page-action`) that the issue's
  Capability line also asks for was added in the same commit.
- No `tests/r0-corpus/` fixture: measured as movement without evidence (no golden serializes the
  field, no corpus or CDO source carries the shape). Stated as a scope decision, ratified twice.

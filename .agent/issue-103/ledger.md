# Issue #103 ledger

## Claim

```json
{"run_id":"20261017-144254-74d8cf","issue":103,"attempt":1,"session":"claude-code-session:cf05ae92-26c8-4a5d-a439-ddec4928220c","branch":"issue/103-x-a1","worktree":"U:\\Git\\al-sem-issue-103-a1","body_hash":"dc5f862b9e4b12475869c8d5b2823ba035c9d6e5efa8fd87c61f5189126a8062"}
```

Issue body hash: `dc5f862b9e4b12475869c8d5b2823ba035c9d6e5efa8fd87c61f5189126a8062`.

Reviewer roster: on 2026-10-15 the operator replaced `gpt-6-astra` and `gemini-3.8-flash` with `gpt-6.1-sol` and `claude-opus-5.5`. Marks are recorded as `reviews.sol` and `reviews.opus`, and we attest with `--substitute astra=sol --substitute flash=opus`.

## Classification

**Bounded.** There is one root cause in one shared function: `dedup_routines_preserving_genuine_overloads` (`src/program/build.rs:746-851`), which both the workspace and the dependency builds route through. A second, small fix goes in the model's event-edge loop, because the first fix would multiply an existing duplication. The design below is the spec.

## Assumption probes

All probes ran on master `a744cd61`. Details are in `.agent/issue-103/investigation.md`.

| probe | result | evidence |
|---|---|---|
| the issue reproduces | confirmed. Digest FireB = `['EVENT_PUBLISH']`, with COMMIT missing. `--program-call-graph-stats` classes the OnB event-flow edge as **`honestEmpty`**, a false "provably no callee", so `unknown` cannot see the bug. | investigation §1 |
| the dedup drops the later arms' facts | confirmed. The survivor is the first copy of each `param_sig_key`, and only `preproc_context` is merged (`build.rs:781-796`, `826-828`). It drops `event_subscribers`, `publisher_kind`, `include_sender`, `access` and `return_type`. | investigation §2a/2b |
| publisher-ness through `#if` is the same bug | confirmed (`ws3`). When arm 1 is plain and arm 2 is `[IntegrationEvent]`, the survivor has `publisher_kind: None`, so there is no event-flow edge. The model degrades it to a `maybe` edge. | investigation §2c |
| the body is also lost | **falsified.** `resolve/full.rs:948-958` walks every decl under the shared id, so both arms' calls are kept (measured with `ws2`). | investigation §2b |
| the model unions subscriptions itself | **falsified.** `program_event_graph` maps each model row to the program node and emits the NODE's subscriptions (`model/events.rs:280-289`, `374-378`). It also emits one edge PER ROW, so an `#if` duplicate already gets OnA→H TWICE on master (`r3a1`). | investigation §2d |
| fixtures or goldens exercise this | no. There are no `#if` files in `tests/r0-corpus`/`tests/fixtures`, and no inline fixture has a same-signature `#if` duplicate with an event attribute. | investigation §4 |
| CDO has this shape | a heuristic text probe finds 13 same-signature `#if` duplicates, NONE with event attributes. The histogram is expected to be byte-identical; the exact in-code probe (D4) confirms it. | investigation §4 |

## Design

**D1. Union the event facts into the dedup survivor.** This goes in `dedup_routines_preserving_genuine_overloads`, mirroring the existing `preproc_context` pattern: pass 1 computes over the same-`param_sig_key` copies of the run (`routines[i..j]`, which are in document order), and pass 2 assigns to the survivor.
- **Scope: SOURCE-tier routines only** (`tier != SymbolOnly`). An ABI `KeepCollapsed` run groups genuinely different overloads under one empty key (`build.rs:806-807`, `abi_ingest.rs:572-573, 599-600`). Merging there would mix their event kinds and could set `include_sender` to `None`. ABI routines never carry subscriptions anyway (`abi_ingest.rs:597`). A test pins that a collapsed ABI survivor is unchanged.
- **Same `param_sig_key` only.** Copies with a different key are genuine overloads aliased to one id. They are never merged; the existing `source_overload_aliased` marking is unchanged. A test pins this with distinct subscriptions per key. **Superseded by the CR1 provenance fix (see Timeline):** the union covers the same-key copies from the survivor's own source file (`RoutineNode.source_unit`) ONLY; copies from another file keep first-wins.
- **`event_subscribers`.** The survivor's OWN list is kept exactly as it is, including any duplicates it already has. Its ordinals are therefore unchanged (`index.rs:261`). Each later copy's entries are appended in document order, skipping an entry equal to one already in the merged list. Equality is the full derived `PartialEq` of `ParsedSubscriberArgs` (`resolve/event.rs:22-39`): publisher type, name or id, event name, element, and both skip flags. Two arms that subscribe the same event with different skip flags therefore keep two entries; a test pins this. The doc of `Subscription.ordinal` (`index.rs:103-105`) is updated to "position in the survivor's merged subscriber list". The implementer greps every `ordinal` reader first and records them.
  - A routine with no duplicate (a single-copy run) is unchanged byte for byte.
- **`publisher_kind`.** The first `Some` among the copies in document order: if ANY arm is a publisher, the survivor is one. Integration vs Business only changes the event-kind label (`model/events.rs:449-455`).
- **`include_sender`.** Taken from the copies whose `publisher_kind` is `Some`. If they all agree, use that value. If they disagree, use `None`. This fails closed: `subscriber_arity_bound` then refuses the `+1` Sender tolerance, exactly as for any unknown (`index.rs:297-316`). `ponytail:` if the D4 probe finds disagreeing arms on real code, revisit. **Superseded by CR2 (see Timeline):** the rule is now `Some(true)` if any publisher arm is true, `Some(false)` if all are false, otherwise `None`.
- **Left alone:** `access` and `return_type` need their own policy and are filed as a discovery. ABI-tier routines never carry subscriptions (`abi_ingest.rs:597`). `preproc_context` is already right.
- **Build conditions.** The merged subscriptions carry no per-arm `#if` condition. This matches the existing doctrine at `build.rs:781-784`: "the survivor exists under either". The result is the possible-build over-approximation the engine uses everywhere for `#if`.
- **Downstream.** Every consumer that reads the survivor (`SubscriberIndex::build`, resolver event-flow edges, the model event graph, LSP `ws_event_edges`, the impact graph, platform-publisher injection at `build.rs:571-576`, which runs after the dedup) now sees the union with no further change. The LSP def-surface fingerprint is built per extracted node before the dedup, so it is unaffected.

**D1b. The differential route verifier.** `resolve/differential.rs:465-492` returns at the FIRST declaration matching name and arity. So with the reproducer's arm order it checks only the OnA arm and REJECTS the new, correct OnB route. The fix: a declaration MATCHES the target only when its full source routine id equals the route's subscriber id, `source_routine_node_id(obj_id, r) == *sub_rid`. Name and arity are not enough, because a same-arity overload `H(X: Text)` has a different `sig_fp`.
- Loop over ALL matching declarations.
- Return `true` as soon as one carries a matching subscription attribute.
- Return `false` when at least one matching declaration was found but none subscribes.
- Return `true` when NO matching declaration was found at all. This keeps today's fail-open (`differential.rs:496-497`) for a routine the verifier cannot locate.

A direct test asserts that:
- OnA→H and OnB→H are accepted;
- a subscription to OnC, which no arm declares, is rejected;
- OnB→H(Integer) is rejected when only a sibling overload `H(X: Text)` subscribes to OnB.

**An existing test that D1b would silently disarm:** `semantic_golden.rs:2360-2420`, `event_route_passes_with_include_sender_true_and_arity_one_subscriber`. It builds `sub_rid` by hand with `sig_fp: 0` for a 1-parameter routine, but that routine's real `sig_fp` is non-zero (`sig_fp.rs:119-131`). Under full-id matching it would match nothing and pass through fail-open without checking the attribute. The fix:
- take its `sub_rid` from `source_routine_node_id` (the graph's real id);
- add a twin with the real id and a MISMATCHING attribute, which must be rejected; this proves the declaration was examined.

The implementer greps every other test that builds a `sub_rid` by hand with a literal `sig_fp`, treats each one the same way, and lists them in the ledger.

**D2. Stop emitting duplicate model event edges for one routine id.** `program_event_graph` (`model/events.rs:365-540`) emits the edges of a subscriber model ROW from the program node's subscriptions. An `#if` duplicate is two rows with one id, so every edge is emitted twice; after D1 that becomes 2 rows × 2 subscriptions. The fix: process the FIRST SUCCESSFULLY MAPPED subscriber row of each routine id, and skip later rows of that id.
- **When an id counts as processed.** It is inserted into the processed set only AFTER its row maps to a program node through `node_by_span`. The maps are built at `model/events.rs:284-289` (workspace) and `:302-315` (dependency); the lookup is at `:370-373`. An unmapped first row therefore does not block a later row that maps. This matters for dependency rows, where `DeclSurface` keeps only the LAST same-id declaration's span (`decl_surface.rs:174-177`).
- **The census (decided).**
  - `model_subscribers` and `unmapped_subscribers` are counted before the mapping step (`:369`, `:371`). They stay PER ROW and are unchanged. **Superseded by CR5 (see Timeline) for `unmapped_subscribers`:** it now counts only the rows whose routine id NO row mapped.
  - `subscribers_without_subscription` (`:375`) and every per-subscription counter now count ONCE PER ROUTINE ID, because a skipped row never reaches them: `bound_model`, `bound_model_missing`, `bound_dependency`, `bound_platform`, `ambiguous`, `orphaned`, `object_unresolved` (inside `for s in subs`). This is intended: a duplicate row is not a second subscription. For a workspace `#if` duplicate whose arms subscribe the same event, `bound_model` goes from 2 (master) to 1.
  - No fixture has such a duplicate, so no golden that prints the census moves. A unit test pins the per-id count on the reproducer.
- **What does not change.** This is not an edge-key dedup. A single-row routine's edges, including any multiplicity they have today, are unchanged. `EventEdge` (`model/events.rs:83-91`) gains no field.

A cross-app test with a source-bearing dependency (`symbol_app::write_source_app`) has an `#if`-duplicated subscriber in the DEPENDENCY. It asserts that the subscriber's edges appear exactly once.

**D3. Not in scope (discoveries):**
- `access`/`return_type` keep only the first arm. Arms that differ only in return type are two MODEL ids (`model/workspace.rs:1336-1342`) on one program node, so both rows receive the node's unioned edges. Each is a distinct model id, so D2 does not skip either.
- `RoutineNode` keeps the first same-id decl while `DeclSurface` keeps the last: workspace `DeclSurface::build` (`src/program/resolve/decl_surface.rs:288-299`, the `local.insert` at `:299`), dependency `DepMeta` (`:140-145`).

**D4. Temporary CDO probe** (not committed): in pass 1, for each same-key group with 2 or more copies, count the groups where the copies differ in `event_subscribers`, `publisher_kind.is_some()`, `include_sender`, `access` or `return_type`. Run it for the workspace and dependency dedup calls (`build.rs:302`, `361`) via `target/cdo/aldump.exe --program-call-graph-stats` on CDO. Record the counts in the ledger, then remove the probe.

## Acceptance matrix

| Acceptance item | Proof |
|---|---|
| The collapsed survivor's `event_subscribers` is the union of every copy's, deduplicated | (A1) A `build.rs` unit test in the style of `preproc_same_signature_arms_collapse_to_one_unmarked_survivor` (`build.rs:1735`): parse the reproducer, `extract_nodes`, sort, dedup, then assert the survivor's subscriptions are EXACTLY [OnA, OnB] in that order. (A1b) Arms that subscribe the SAME event give ONE entry. (A1c) Three arms (`#if A … #elif B … #else …`) give the union of all three, in order. |
| A program-build test with the reproducer shows FireB reaches COMMIT | (A2) A CLI test in `tests/cli/cli_attribute_trivia.rs`, using its `commit_by_routine` helper (`:30-67`), on the reproducer: FireA and FireB both reach COMMIT. (A2b) A program-engine test: the OnB event-flow edge resolves to H (`resolvedSource`), and the stats show no `honestEmpty` for it. |
| Publisher facts | (A3) The `ws3` shape (arm 1 plain `OnA`, arm 2 `[IntegrationEvent] OnA`, unconditional subscriber H) gives an event-flow edge OnA→H that is resolved, not `maybe`. (A3b) Publisher arms that disagree on `include_sender` give `None`. Arms that agree keep their value. |
| Same-signature collapse otherwise unchanged | (A4) The existing dedup unit tests (`build.rs:1442-1999`, `2159`) stay green unchanged. A single-copy run's routine is equal (`==`) before and after. `scripts/check-goldens` is green with no golden moved. The CDO `--program-call-graph-stats` JSON is byte-identical between master and the branch. |
| No duplicate model edges | (A5) The model event graph of the reproducer has EXACTLY one OnA→H edge and one OnB→H edge, checked through the r3a1 combined graph or `program_event_graph` directly. |
| No leakage | (A6) An aliased pair: a unit test in the style of the existing collision test (`build.rs:1635-1642`). It HAND-STATES two source routines with the SAME `RoutineNodeId` (forced equal `sig_fp`) and asserted-unequal `param_sig_key`s, the first subscribing OnA and the second OnB. After the dedup, each key's survivor holds its own subscription only. The collision is stated by hand for determinism. A real AL pair CAN reach it, because `sig_fp` collapses internal whitespace while `param_sig_key` only trims and folds (`build.rs:1623-1632`, `sig_fp.rs:66-73`, `node_extract.rs:475-480`). An example is `Record Customer` vs `Record  Customer`. (A6b) Same event, different skip flags in the two arms: two entries. (A6c) An ABI `KeepCollapsed` survivor with differing `publisher_kind`/`include_sender` among the collapsed rows is unchanged. |
| Verifier (D1b) | (A7) A direct differential-verifier test: OnA→H and OnB→H are accepted. A subscription to OnC, which no arm declares, is rejected. OnB→H(Integer) is rejected when only a sibling overload `H(X: Text)` subscribes to OnB. A routine not found at all stays accepted (fail-open). |
| Dependency rows (D2) | (A8) A cross-app test: an `#if`-duplicated subscriber in a source-bearing dependency gets its edges exactly once, including when its first row does not map to a node. |
| Discrimination | Each break is made with Edit, run, and reverted. (1) Drop the pass-2 `event_subscribers` assignment: A1/A2/A2b fail. (2) Union without dedup: A1b fails. (3) Keep the first `publisher_kind`: A3 fails. (4) First-wins `include_sender`: A3b fails. (5) Drop the D2 row skip: A5 fails. (6) Merge across different `param_sig_key`s: A6 fails. (6b) Compare subscriptions by event name only: A6b fails. (6c) Drop the source-tier guard: A6c fails. (7) Return at the first matching decl in the verifier: A7 fails. (7b) Match verifier decls by name and arity only: A7's overload case fails. (7c) Return `false` when no decl is found: A7's fail-open case fails. (8) Mark an id processed before it maps: A8 fails. |

## Measurement plan

- Goldens: none are expected to move, because no fixture has the shape. `scripts/check-goldens` must be green.
- CDO: run the D4 probe counts. The histogram must be byte-identical between master and the branch. `cdo-gate` runs once at the branch gate.

## Base/Head SHAs

- B = `a744cd6162b178024b8895d94676a80209e49f36` (origin/master at claim and at freeze; no rebase was needed)
- H = `771e071e` + full SHA in the attestation (the approved code head; the evidence commit follows it)

## Discoveries

1. **program build: `access` and `return_type` keep only the first `#if` arm (bug).**
   - The dedup survivor keeps the first arm's `access` and `return_type` (`build.rs`). Arms that differ only in return type are two model ids on one program node.
   - CDO has 2 such groups: codeunit `6192852`, `getaccesstoken` and `getfreshaccesstoken`.
2. **decl surface: dependency `DepMeta` keeps only the LAST same-id declaration span (enhancement).**
   - So earlier `#if` rows of a dependency routine never map in the model event graph.
   - The CR5 counting fix hides that, but the span is still lost.
   - The workspace `DeclSurface` and `RoutineNode` also disagree: first-wins vs last-wins (`decl_surface.rs:140-145`, `:288-299`).
3. **program build: a whole-object `#if` duplicate keeps the first object's `EventSubscriberInstance` (bug).**
   - `subscriber_instance_manual` is per object (`node_extract.rs:790` at 771e071e; `:769` on master), and `EventFacts::union` does not merge it.
   - So when two object declarations under `#if`/`#else` differ in `EventSubscriberInstance`, the survivor keeps the first object's manual-binding flag, and the second object's merged subscriptions inherit it.
   - Possible fixes: an explicit any-arm rule, or keeping the flag per subscription.

## Timeline

- The worktree was created at master `a744cd61`.
- Investigation (opus, read-only; subagents 1): `.agent/issue-103/investigation.md`.
- Classified as bounded. The design above is the spec.
- Spec panel (sol + opus; hash = sha256(ledger.md)[:16]):
  - r1 (spec_rounds 1; pi_calls 2; hash b9322e8088f64c23): both confirmed the root cause and the fix location. Merged findings S1-S8. The important ones: the verifier claim was false; the ABI `KeepCollapsed` merge; the D2 identity named fields that do not exist and changed single-row multiplicity; break 6 could not fail. Revision 2 adds the source-tier guard, D1b (the verifier loops over all decls), the D2 row skip, the ordinal rule, A6/A6b/A6c/A7, and fixed citations.
  - r2 (spec_rounds 2; pi_calls 4; hash d7b7c559f4d1b7d0): S1-S3 and S5-S7 accepted by both. S4 re-raised by both: A6 did not build an id collision. S8 re-raised by opus: the workspace citation. New findings:
    - sol, important: the verifier must match by full routine id; D2 must mark an id only after a successful span map;
    - opus, minor: the verifier's fail-open rule; where the census counts skipped rows.

  - r3 (spec_rounds 3; pi_calls 6; hash 1e9474fa5c4d2176): opus accepted all 11 entries, and sol accepted 10 but re-raised N2, because the census claim was wrong. New findings: F1 (important, D1b disarms the `semantic_golden` test that uses a literal `sig_fp: 0`), F2 (A6's claim that a collision is impossible is false), F3 (the census counters, merged into N2), F4 (citation). Revision 4 specifies the census per counter, fixes and twins the `sig_fp: 0` test and greps the others, corrects A6's rationale, and fixes the citation.

  - r4 (spec_rounds 4; pi_calls 8; hash a6d45f4b61136fcc): both accepted all 14 entries. Sol had no new findings. Opus raised two minors: R1, `bound_model_missing` was missing from the census list; R2, a stale census clause in this timeline. Revision 5 fixes both.
  - r5 (spec_rounds 5; pi_calls 10; hash 28f851ffcdd2b41e): both accepted all 16 entries. Sol found nothing new. Opus found no ledger defect and two register-text nits (R1's line 399→400; N2's evidence list lacked `bound_model_missing`), both fixed in the register's text. CONVERGED.
- Plan: `docs/superpowers/plans/2026-10-17-issue-103-if-dedup-event-union.md` (plan_tasks 3).
- T0+T1 (opus; subagents 2; task_attempts T0T1 = 1): PASS. Report: `.agent/issue-103/t1.md`.
  - **Red tests:** A1, A1c, A3, A3b, A4, A6b, A2, A2b, A3-CLI, A5, A7 and A8 were red for the stated reasons. A1b, A6, A6c and A7's overload and fail-open cases are guards, so they pass on master by design.
  - **Results:** lib 2080 green, clippy clean, check-goldens green with no golden moved.
  - **Proofs:** breaks 1-8, plus 6b, 6c, 7b, 7c and F1, each fail and pass after revert. Break 5 leaves A8 green, which is expected: A5 guards break 5, and A8 guards break 8.
  - **D4 CDO probe:** dependency layer, 28 source groups and 0 ABI groups; no difference in subscriptions, publisher-ness or `include_sender`; 2 groups differ in `return_type` (a discovery). Workspace: 0 groups. The `--program-call-graph-stats` output is byte-identical between master, the branch with the probe, and the clean branch (sha256 `c032c8db…`). The probe was removed.
- Commit `74a503b6` (code, tests, CHANGELOG, plan). Pre-commit goldens OK; `check-diff` `reasons: []`.
- Gates and the `code-review` skill (high) started. The gates were stopped once the review findings meant the code would change.
- **`code-review` (high), 8 findings, CR1-CR8:**
  - CR1 (important): the union also merged same-object copies from two FILES.
  - CR2 (important): the `None` fallback for `include_sender` dropped Sender subscribers.
  - CR3 (important): the verifier's fail-open was widened to accept a wrong `sig_fp`.
  - CR4: duplicate edges per model id; REFUTED, because the ids are distinct declarations (spec D3).
  - CR5 (important): dependency `#if` rows inflated `unmapped_subscribers`.
  - CR6: `publisher_kind` takes the first arm; REFUTED, because it is a label only (spec D1).
  - CR7 (minor): needless clone.
  - CR8 (minor): repeated derivation.
- **CR fix** (opus; subagents 3; task_attempts CR = 1). Report: `.agent/issue-103/cr-fix.md`.
  - **CR1:** only the `#if` arms of one site are merged: the leading copies whose build conditions pairwise contradict (`preproc_contradicts`). Later copies stay first-wins. Two limits are stated with a `ponytail:` comment: an undecidable arm is not grouped, and two files that each hold one opposite arm are grouped. `RoutineNode` has no source file, which becomes a discovery.
  - **CR2 (deviation from D1):** `include_sender` is `Some(true)` if any publisher arm is true; `Some(false)` if all are false; otherwise `None`. A3b now binds `H(Sender)` and asserts `count_unknown_include_sender_plus1_subscribers == 0`.
  - **CR3:** fail-open only when the object or name is absent; a wrong `sig_fp` on an existing name is rejected (new A7 case).
  - **CR5 (deviation from D2's census text):** `unmapped_subscribers` counts only rows whose id NO row mapped. A8 asserts 0.
  - **CR7/CR8:** same-key copies are computed once, and identical copies are skipped.
  - **Results:** lib 2081, harness 201, r4 58 and cli 5 green; clippy clean; check-goldens green with no golden moved. The CDO stats are byte-identical to master (sha `c032c8db…`). Breaks CR1, CR2, CR2b, CR3 and CR5 each fail and pass after revert.
- Commit `18aa8830` (CR fixes). The gates started at it were STOPPED, because final-panel findings followed.
- **Final panel r1** (final_rounds 1; pi_calls 12; hash 7504ead09ec3739d):
  - **opus** accepted all 24 entries and raised 3 minors (F1-F3): A8 does not assert its own precondition; the D1/D2 text was superseded but not marked; the Discoveries section was unfilled.
  - **sol** re-raised CR1: contradicting conditions are not provenance, and its cross-file counterexample still merges. It raised F4 (important): a whole-object `#if` duplicate in one file loses the second arm. It raised F5 (minor): discrimination on the final paths, i.e. the A6c guard and break 2 through A4.
- **Provenance fix** (opus; subagents 4; task_attempts CR = 2). Report: `.agent/issue-103/prov-fix.md`.
  - `RoutineNode.source_unit: Option<NonZeroU32>` is the file index within one dedup population. It is serde-skipped, has no size cost (224 B), and is stamped by the workspace and dependency builds.
  - The union merges exactly the same-key copies of the survivor's unit, and the heuristic is removed. Tests (a)-(d) cover: a whole-object `#if` in one file, plus an end-to-end digest; the cross-file counterexample; an undecidable arm; opposite arms in two files.
  - Every break (1-8, 6b, 6c, 7b, 7c, CR1-prov ×3, CR2, CR3, CR5) was re-run on the FINAL tree.
  - lib 2084, cli 6, harness 201, r4 58 green; clippy clean; no golden moved. The CDO stats are byte-identical.
  - The A8 doc now states its limit (F1). The D1/D2 bullets are marked superseded (F2). Discoveries are filled (F3); the "no source file" discovery is dropped, because this fix resolves it.
- Commit `771e071e`. `check-diff` `reasons: []`.
- **Final panel r2** (final_rounds 2; pi_calls 14; hash ca788e618c6ea555): both reviewers accepted all 29 entries. Sol found nothing new. Opus found F6 (the D1 same-key bullet was stale) and F7 (`EventSubscriberInstance` on whole-object duplicates). Both were fixed in LEDGER TEXT ONLY: F6 is marked superseded, and F7 is Discovery 3 (deferred). The code and the hash are unchanged.
- **Final panel r3** (final_rounds 3; pi_calls 16; hash ca788e618c6ea555): both accepted all 31 entries. CONVERGED. Sol accepts F7 as a documented, deferred bug, so its disposition is `deferred` (minor). Opus left three ledger-text nits (a stale `node_extract` line, the Discovery numbering, this r2 entry). They were fixed here, in bookkeeping text only, with no code change.
- **Gates at `771e071e`:** all supervised, all exit 0. Logs are under `.agent/runs/20261017-144254-74d8cf/logs/`:
  - ci-steps-all: 604.9 s (`ci-steps-all/20261017T165511-93ed7bd0.log`);
  - check-goldens-coverage: 2.1 s (`check-goldens-coverage/20261017T170516-7f27f801.log`);
  - check-goldens: 65.3 s (`check-goldens/20261017T170519-4f4b42ac.log`);
  - cdo-gate: 219.2 s (`cdo-gate/20261017T170624-38eb7564.log`).

  The porcelain check is empty.
- **Rebase:** `origin/master` is `a744cd61`, an ancestor of HEAD. No rebase.
- **Freeze:** the ledger and register are committed as the evidence commit.

    History (r3; its census clause is superseded by r4's per-counter decision in D2). Revision 3 made these changes:
    - the verifier matches on the full id and keeps fail-open when nothing is found;
    - D2 processes the first successfully mapped row (r3 claimed the census was unchanged; r4 replaced that with the per-counter decision);
    - A6 is hand-stated;
    - A7 gains negative and fail-open cases;
    - A8 covers a dependency duplicate;
    - the citations are fixed.

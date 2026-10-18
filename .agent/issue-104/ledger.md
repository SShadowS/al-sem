# Issue #104 ledger

## Claim

```json
{"run_id":"20261018-151058-7d94d0","issue":104,"attempt":2,"session":"claude-code-session:c26c6ca6-002c-49f4-bd91-33f31cbdf028","branch":"issue/104-agent-discovery-program-engine-a2","worktree":"U:\\Git\\al-sem-issue-104-a2","body_hash":"8cb85c6636b3c473b9b2c7415ec8a202a192c1ed2becda8de2c198fe8c187851"}
```

Issue body hash (sha256): `8cb85c6636b3c473b9b2c7415ec8a202a192c1ed2becda8de2c198fe8c187851` (unchanged since attempt 1).
Worktree base (`make-worktree`): `60522971afd71f6853b7120bfef5f9a3d7460991` (the same base as attempt 1, so every code reference below still holds).

**Attempt 2 starts from attempt 1's design.** Attempt 1 (run `20261018-115641-25d4e8`, worktree `U:\Git\al-sem-issue-104-a1`) ended `blocked spec-panel-cap` with both reviewers (sol, opus) accepting 17 of 18 register entries; S5-1 (a test-fixture detail, fixed in the matrix) had no marks when the cap was reached. This attempt is NOT a pre-ratified handoff (the claim carries no `handoff`): its spec panel runs again from round 1. Every carried register entry's marks are reset to `unreviewed` (attempt 1's are kept as `a1_reviews`), and round 1 asks both reviewers to mark each one against the current text, then review for anything new. The ledger below is attempt 1's, copied unchanged except for this header and the timeline; attempt 1's timeline is kept as history.

Reviewer roster (operator instruction for this loop, 2026-10-18: "Dont use gemini. USe gpt 6.1 sol and local opus 5.5"): `sol` = gpt-6.1-sol via pi (astra slot), `opus` = a local Opus 5.5 subagent (flash slot). Attest with `--substitute astra=sol --substitute flash=opus`.

## Classification

**bounded.** Two consumers of a routine's parsed attributes read only one of
several per-build readings of the same attribute: `publisher_include_sender`
(`src/program/resolve/event.rs:301`, `.find()` = first reading) and the
`event_subscribers` list in `src/program/node_extract.rs:792-799` (one entry per
reading, no dedup). The fix is local to those two places plus one shared helper
that #103's `EventFacts::union` (`src/program/build.rs`) already needs. No new
subsystem, no schema change.

## Design

Since #80 the lowerer can give ONE routine several `AttributeIr`s for one
attribute: one per build path that reaches the routine with a different reading
of an `#if` group inside the attribute's arguments. These are possible builds of
the same declaration, exactly like #103's `#if` arms of one routine. They must be
merged with the SAME rule #103 uses across arms, so one helper serves both.

1. **IncludeSender.** New `event::union_include_sender(impl Iterator<Item =
   Option<bool>>) -> Option<bool>`: `Some(true)` if any reading is `Some(true)`
   (some build passes the Sender, so a Sender-capturing subscriber is valid AL in
   that build); `Some(false)` if there is at least one reading and every one is
   `Some(false)`; otherwise `None`. `publisher_include_sender` reads arg 0 of
   EVERY publisher attribute (`integrationevent`/`businessevent`/`internalevent`)
   and returns the union. `EventFacts::union` replaces its inline copy of the same
   rule with a call to the helper (DRY; behaviour unchanged).
2. **Subscriptions: one per distinct event, merged where they are wired**
   (revised after spec round 1, S1-1/S1-3). `RoutineNode::event_subscribers`
   stays one entry per reading (and per #103 arm), unchanged. The one place
   every EDGE and subscription-INVENTORY consumer reads from is
   `SubscriberIndex::build`
   (`src/program/resolve/index.rs`): `subscribers_of` feeds the event-flow edges
   (`resolver.rs emit_event_flow_edges`) and `subscriptions()` feeds the model
   event graph (`model/events.rs:287`) and the census (`full.rs:2307`). There,
   before wiring, a subscriber routine's entries are merged by EVENT KEY:
   `(publisher, event_name folded, element)`. Mechanism (revised round 4,
   S3-5): the routine's entries are first each taken through the existing
   object resolution and candidate selection WITHOUT recording; then grouped by
   key; then each group is recorded once. The element is part of the key only
   when the entry's outcome is `Bound(rid)` and that routine's `publisher_kind ==
   Some(PublisherKind::Platform)` — the exact test the model applies
   (`model/events.rs:402-411`), so a table with a REAL source publisher of a
   platform event name (the injector then adds no synthetic, `build.rs:621-628`)
   is keyed like the model keys it. For every other outcome the model never uses
   the element, so two readings differing only in element are one subscription
   and the merged entry keeps the first reading's element. Two entries with the
   same key always have the same outcome: the key fixes the publisher object and
   event, and the candidates depend only on those and the routine's own arity
   (`index.rs:286-325`). `publisher` is the RESOLVED
   publisher object id (`resolve_subscription_publisher`) when it resolves, else
   the textual `(publisher_object_type, publisher_name, publisher_id)`. So
   `Codeunit::"Pub"` in one reading and `50100` in another are one key. Each
   merged entry keeps its FIRST entry's ordinal (no ordinal moves; the only
   reader of `Subscription::ordinal` is a test, `index.rs:2772`). The skip flags
   of entries that share a key are AND-ed: a skip condition is kept only when
   every reading carries it. Reason: the flags become `Condition`s on the one
   subscriber edge (`index.rs build_entry`); if any build does NOT skip, the
   edge may run without the license/permission, so the condition does not hold
   for the edge — the same may-execute over-approximation as IncludeSender.
   Merged groups are recorded in first-ordinal order, so `subscriptions()`
   keeps its documented order ("`graph.routines` order then attribute order",
   `index.rs:393-394`). Each recorded `Subscription` carries the merged
   (AND-ed) conditions and the first reading's textual publisher fields and
   element, so `Subscription.conditions` equals the conditions on the edge's
   `SubscriberEntry` (T2-1).
   Because the merge sits at wiring, it also collapses #103's arm case (two arms
   subscribing one event with different skip flags): #103's RoutineNode-level
   tests A4/A6b stay true as written (they assert the node's list, which is not
   changed), and the index gives one edge for them. A6b's doc comment ("two
   different subscriptions") is reworded: the node keeps both entries, and
   `SubscriberIndex` wires them as one subscription (S3-3).
3. **Isolated** (S1-2, S3-4). `model/events.rs parse_isolated` reads `Isolated`
   from the FIRST `IntegrationEvent` (arg 2), else the first `BusinessEvent`
   (arg 1). It now reads EVERY `IntegrationEvent` and `BusinessEvent` reading,
   each at its own index, and returns `Some(true)` when ANY reading is isolated
   (or carries the arg unparseable, its existing conservative rule): an
   isolated publisher in some build is a barrier (Rule 5, prefer exclusion over
   a false weave). `InternalEvent` is NOT read, as today: its `Isolated` (arg 1,
   `event.rs:285`) was never read by this function. That is a separate
   pre-existing gap, filed as a discovery, not widened here.

Two raw readers of `event_subscribers` bypass the index and stay as they are:
`build.rs inject_platform_event_publishers` (its `seen` set already makes one
synthetic publisher per `(table, event)`, so a duplicate reading adds nothing),
and the diagnostic below.

Key normalisation: every string field of `ParsedSubscriberArgs` is already
folded at parse time (`event.rs:60` lowercases the object type; `:129-141`
`resolve_publisher_name` folds the name; `:112-119` `name_arg_text` folds event
name and element), so the textual key needs no further folding.

Not changed, deliberately: `count_unknown_include_sender_plus1_subscribers`
(`index.rs:1125`), a diagnostic that by its doc shares no code path with the
wiring; it may count a duplicated reading twice. The IncludeSender union can
move it either way (S3-2): `[None, Some(true)]` was `None` and is now
`Some(true)` (lowers it), and `[Some(false), unreadable]` was `Some(false)`
and is now `None` (raises it). Wiring is unchanged in the second case, because
`subscriber_arity_bound` treats `None` like `Some(false)` (`event.rs:336`); only
the diagnostic can move, and only for an unreadable reading, which the ABI
probe found 0 times in 13,581 (`event.rs:296-300`).

Non-goals: `publisher_kind`/`event_kind` when readings carry DIFFERENT publisher
attribute kinds (still the first, `is_event_publisher`/`publisher_event_kind`);
the ABI tier (`abi_publisher_include_sender`, one attribute per routine there);
detector readers of `attributes_parsed` outside `src/program` (d29/d38/d59 etc.)
— checked separately, a discovery if wrong.

## Assumption probes

| probe | result | evidence |
|-------|--------|----------|
| The lowerer gives one routine two `IntegrationEvent` readings for the issue's reproducer | TRUE | throwaway `probe_104` (tests/cli, not committed): `OnFoo pub=Some(Integration) is=Some(false)` with `#if X false #else true #endif`; the second reading's `true` is lost |
| A Sender subscriber is orphaned by it | TRUE | same probe: model edge `("onfoo","H","maybe")`, census `orphaned: 1` |
| Two readings of one `[EventSubscriber]` (skip flag under `#if`) give two subscriptions | TRUE | same probe: `G subs=[…license:true…, …license:false…]`, event flow `[("g",true),("g",true)]`, model has `("onfoo","G","resolved")` twice, `bound_model: 2` |
| The differential verifier is unaffected | TRUE | `differential.rs:490-503` accepts a route when ANY reading matches (`.any`); its `publisher_include_sender` input comes from the graph |

## Base/Head SHAs

- Attempt 2, after the final panel: B = `origin/master` = `60522971afd71f6853b7120bfef5f9a3d7460991` (unchanged since make-worktree; `git fetch origin` then `merge-base --is-ancestor origin/master HEAD` true, so no rebase); H = `c8a7d3f8bc503ea752ea18a94dc213ad3518439b`.

## Timeline

Attempt 1 (history):

- make-worktree: `{worktree: U:\Git\al-sem-issue-104-a1, base: 60522971}`; caps: none charged.
- classify: bounded; probes: 4 rows, all true.
- spec round 1 (astra + flash): 7 findings (S1-1..S1-7); design moved the event merge into `SubscriberIndex::build`, added Isolated. Reviews: `.agent/runs/20261018-115641-25d4e8/spec-r1-{astra,flash}.md`.
- spec round 2 (astra + flash): astra re-raised S1-6; 4 new (S2-1..S2-4, two refuted with parse-time folding evidence). flash failed twice on a pi MCP startup error ("Project servers blocked"); a fresh thread succeeded.
- OPERATOR: "Dont use gemini. USe gpt 6.1 sol and local opus 5.5". Roster from round 3: `sol` (gpt-6.1-sol via pi) in the astra slot, `opus` (local Opus 5.5 subagent) in the flash slot (`--substitute astra=sol --substitute flash=opus`). Earlier astra/flash marks kept as `astra_flash_marks_r2`, not counted.
- spec round 3 (sol + opus): all 11 accepted by both; 5 new minor (S3-1..S3-5).
- spec round 4 (sol + opus): sol re-raised S3-2 (wrong example) and S3-5 (textual platform test); opus raised S4-1 (same as S3-5). Fixed: bind first, key element only on `Bound` + `PublisherKind::Platform`.
- spec round 5 (sol + opus): sol accepted all 17, no new findings; opus accepted all 17, one new minor S5-1 (A3e real-publisher fixture must admit the subscriber's arity, or it tests `Orphaned`). Fixed in the matrix, but sol has not marked S5-1.
- `charge spec_rounds` remaining 0 after round 5. Not converged (S5-1 unreviewed by both). Handoff rule NOT met (a missing mark). Outcome: `blocked spec-panel-cap`. Caps used: spec_rounds 5/5, pi_calls 7, subagents 3 (+ ranking agent).

Attempt 2:

- operator `unblock 104`; claimed attempt 2 (`claim` reply above); make-worktree `{worktree: U:\Git\al-sem-issue-104-a2, base: 60522971}`; ledger and register carried from attempt 1.
- spec round 1 (sol + opus; marks reset): both accepted all 18 carried entries; opus raised T2-1 (minor: record order and recorded conditions after a merge), fixed in Design item 2. Reviews: `.agent/runs/20261018-151058-7d94d0/spec-r1-{sol,opus}.md`.
- spec round 2: both accepted all 19 entries, no new findings. **Converged** (no open entry, every entry accepted by both, no blocking entry deferred). Caps: spec_rounds 2/5, pi_calls 2, subagents 2 (+ ranking agent).
- acceptance tests (opus subagent; report `.agent/runs/20261018-151058-7d94d0/acceptance-tests.md`): 13 tests in 4 files, uncommitted. Independently re-run by the conductor (`cargo test -p al-sem --lib -- include_sender_is_the_union subscriber_readings_keep_only two_entries_of_one_event isolated_in_the_second_reading isolated_is_read_from_every` → 5 FAILED; `cargo test --test cli cli_attribute_trivia::` → 5 FAILED, 9 ok). RED as stated: A1 (`include_sender` Some(false)), A3 (two routes to `g`, one with SkipOnMissingLicense), A3b, A3e codeunit (two routes), A3e real-table (two Bound subscriptions), A3d and A5 (`subscribers_of` len 2), A4 (5 of 8 truth-table rows wrong via `publisher_include_sender`), A6 and A6c (not isolated). Declared controls passing today: A2 (Sender reading first), A3c (two different events), A3e platform half (`/a`, `/b`). The 6 existing #80/#103 tests in the file still pass. A4 tests the union rule through `publisher_include_sender` because `union_include_sender` does not exist yet.
- plan `docs/superpowers/plans/2026-10-18-issue-104-attribute-readings.md` (4 tasks; `charge plan_tasks` ×4, remaining 8). Single commit for Tasks 1-3 by design (shared test file, red tests in tree).
- implement (subagent-driven): Task 1 opus → sonnet review (approved, 2 minor); Task 2 opus → sonnet review (approved, 6 minor); Task 3 opus → sonnet review (approved, 2 minor test gaps); review fixes: sonnet for Tasks 1+2 (all applied, +2 tests), sonnet for Task 3 (+3 cases, +1 table test). Task 4 (CHANGELOG) written by the conductor (a documentation paragraph; substitution recorded).
- commits: `2d38d011` (fix + tests), `194afc43` (CHANGELOG), `d2679b55` (plan); `check-diff` clean. Gates round 1 green on `d2679b55`.
- final panel round 1: `code-review` skill (high) 7 findings; sol APPROVED; opus APPROVED + 2 minor docs. Fixes (opus subagent) committed `4efda61b`: single platform decision (`Subscription.platform`), docs, CHANGELOG. Reviews: `.agent/runs/20261018-151058-7d94d0/final-r1-{sol,opus}.md`.
- final panel round 2: both accepted all 28 entries; opus one minor (F2-N1), fixed with a comment (conductor edit, no behaviour change), committed `c8a7d3f8`.
- final panel round 3: both accepted all 29 entries, no new findings. Converged. Caps: final_rounds 3/5.
- gates round 2 killed by memory pressure (the heartbeat loop too); operator "restart now"; gates round 3 green on `c8a7d3f8`. Final verification by the fix agent: `cargo test -p al-sem --lib program::` 957 passed, 0 failed; `cargo test --test cli -- cli_attribute_trivia::` 14 passed; `cargo build --all-targets` 0 warnings. task_attempts: 1 per task.

## Acceptance matrix

| # | Acceptance item | Proof |
|---|-----------------|-------|
| A1 | A subscriber with the extra Sender parameter binds when the Sender reading is the SECOND | program-build test (`tests/cli/cli_attribute_trivia.rs`): event flow of `OnFoo` resolves to `h` from source; model edge `resolved`; publisher `include_sender == Some(true)` |
| A2 | … and when it is the FIRST | same test, other orientation |
| A3 | Two readings of one `[EventSubscriber]` naming the same event give ONE subscription edge | program-build test: event flow lists `g` once, model has one edge for `G`, `bound_model` counts it once; the emitted event-flow `Route` to `g` carries no `SkipOnMissingLicense` condition (flags AND-ed) |
| A3e | Two readings of a PLATFORM table event with DIFFERENT elements keep both subscriptions; on a codeunit event they are one | program-build test: a table's `OnAfterValidateEvent` subscribed with elements `a`/`b` → two model edges (ids `/a`, `/b`); a codeunit event with elements `a`/`b` → one route, one model edge; a table that declares a REAL `[IntegrationEvent] OnAfterValidateEvent` publisher whose arity ADMITS the subscriber (S5-1), subscribed with elements `a`/`b` → outcome `Bound` to the non-Platform routine, one `resolved` model edge |
| A3b | … also when one reading names the publisher by name and the other by number | program-build test, same assertions |
| A3c | Two readings naming two DIFFERENT events keep both subscriptions | program-build test: both events' flows reach the subscriber |
| A3d | A skip flag set in EVERY reading is kept; a flag set in only some readings is dropped — for BOTH flags | `SubscriberIndex` unit test (hand-stated `event_subscribers`): license set in both + permission set in one → conditions are exactly `[SkipOnMissingLicense]`; and the mirror (permission in both, license in one) → exactly `[SkipOnMissingPermission]` |
| A4 | IncludeSender rule, hand-stated | unit tests of `union_include_sender` (truth table) and `publisher_include_sender` over two `BusinessEvent` readings and two `InternalEvent` readings |
| A5 | #103's arm case gives one edge too | `SubscriberIndex` unit test: two hand-stated entries of one event with different flags → one entry in `subscribers_of`, one `Subscription`, first ordinal |
| A6 | Isolated in any reading makes the event isolated | unit test of `build_event_symbol`/`isolated_event_ids` over two readings (second isolated) |
| A6c | Mixed-kind and every-reading Isolated | `build_event_symbol` unit test: readings `[IntegrationEvent(false,false,false)]`, `[BusinessEvent(false,true)]` → isolated |

## Discrimination proofs

Every break was made with Edit, its marker grep-counted to 1 before the run, and reverted with Edit (count 0 after). Full reports: `.agent/runs/20261018-151058-7d94d0/{task1-report,task2-report,task3-report,review-fix-1-2,review-fix-3}.md`.

| Test | Break | Fails with | After revert |
|------|-------|-----------|--------------|
| `union_include_sender_truth_table`, `include_sender_is_the_union_of_every_reading`, `a1_sender_in_the_second_reading_binds_the_sender_subscriber`, #103 `if_duplicate_publisher_include_sender_is_true_when_any_arm_is` | B1: helper returns the first reading only | truth table `[Some(false), Some(true)]` → `Some(false)`; A4 5 rows wrong; A1 `left: Some(false) right: Some(true)`; build.rs:2553 same | pass |
| `union_include_sender_truth_table`, `include_sender_is_the_union_of_every_reading` | B2: an unknown reading no longer forces `None` | `[None]` → `Some(false)`; row `[IntegrationEvent(false,…)] [IntegrationEvent(Foo,…)]` → `Some(false)` | pass |
| #103 `if_duplicate_publisher_include_sender_is_true_when_any_arm_is` (the `EventFacts::union` call site) | B3: `.take(1)` before the helper | build.rs:2554 `left: Some(false) right: Some(true)` | pass |
| `a3e_codeunit_event_readings_differing_only_in_element_are_one_subscription`, `a3e_real_table_publisher_…` | always key on element | two routes to `g`; two `Bound` to the Integration publisher | pass |
| `a3e_control_platform_table_event_readings_keep_one_subscription_per_element` | never key on element | only `/a`, `/b` missing | pass |
| `a3b_readings_naming_the_publisher_by_name_and_by_number_are_one_subscription` | key on textual publisher only | two routes to `g` | pass |
| `subscriber_readings_keep_only_skip_flags_set_in_every_reading`, `two_entries_of_one_event_are_one_subscription_at_the_first_ordinal`, `a3_two_readings_of_one_subscriber_are_one_subscription` | OR the skip flags instead of AND | `[SkipOnMissingLicense, SkipOnMissingPermission]` vs `[SkipOnMissingLicense]`; `[SkipOnMissingLicense]` vs `[]`; a3 route keeps the flag | pass |
| `a3_…`, `two_entries_…` (and a3b, a3e codeunit/real-table, skip-flag test) | no merge at all | a3 two routes; two_entries `left: 2 right: 1` | pass |
| `merged_subscription_sits_at_its_first_members_ordinal_with_its_fields`, `two_entries_…` | record the LAST member's ordinal | `[(0, onaftery), (2, onafterx)]` vs `(1, onafterx)`; two_entries `[(1, [])]` vs `[(0, [])]` | pass |
| `unresolved_publisher_readings_of_one_event_are_one_subscription` | textual (unresolved) keys never merge | `left: 2 right: 1` | pass |
| `isolated_in_the_second_reading_makes_the_event_isolated`, `isolated_is_read_from_every_reading_of_every_kind` | read only the first reading of each kind | events.rs:779 `(None,false)` vs `(Some(true),true)`; :809 "second BusinessEvent" | pass |
| `isolated_is_read_from_every_reading_of_every_kind` | BusinessEvent read at index 2 instead of 1 | mixed-kind case fails | pass |
| `isolated_is_read_from_every_reading_of_every_kind` | BusinessEvent ALSO read at index 2 | :827 "BusinessEvent is not read at index 2" | pass |
| `isolated_single_attribute_table` | non-literal arg → not isolated | :842 "non-literal" | pass |
| `isolated_in_the_second_reading_…` (old shape) | first publisher reading only, any kind | fails; `…every_kind` also fails on the mixed case | pass |

## Findings register summary

29 entries, every one `accepted` by both `sol` (gpt-6.1-sol, astra slot) and `opus` (local Opus 5.5, flash slot). Spec panel (attempt 1 rounds 1-5, attempt 2 rounds 1-2): S1-1..S5-1 and T2-1, converged. Final panel: the built-in `code-review` skill at high (7 findings, F1-CR1..CR7) plus both reviewers over 3 rounds (F1-O1, F1-O2, F2-N1). Dispositions: fixed 21, refuted 7, deferred 1 (F1-CR7, minor). The three refuted `important` entries: S1-4 and S2-3 (spec claims disproved by source: the publisher-attribute filter and parse-time folding) and F1-CR1 (pre-existing, intended; its limitation filed as a discovery); both reviewers accepted each refutation. No entry open; no blocking entry deferred.

## Gate results

Round 1 (head `d2679b55`, base `60522971`), each through `agentflow run`, all `supervised: true`, `queued_s: 0`:

| Gate | Exit | Tests | Log |
|------|-----:|-------|-----|
| `ci-steps-all` | 0 | 3400 passed, 0 failed | `.agent/runs/20261018-151058-7d94d0/logs/ci-steps-all/20261018T155659-6076bf9f.log` |
| `check-goldens-coverage` | 0 | — | `…/logs/check-goldens-coverage/20261018T160930-4d9cc048.log` |
| `check-goldens` | 0 | 843 passed, 0 failed; no golden moved | `…/logs/check-goldens/20261018T160931-77f3c569.log` |
| `cdo-gate` | 0 | 2318 passed, 0 failed; `cdo-gate: PASS` (CDO ratchets incl. the real-unknown and frozen-digest checks) | `…/logs/cdo-gate/20261018T161048-bc61d8f0.log` |

Round 2 (head `4efda61b`, after the final-panel fixes): STOPPED by Claude Code under system memory pressure (2.1 GB of 93.7 GB free) partway through `ci-steps-all`; no verdict. Operator: "restart now".

Round 3 (head `c8a7d3f8`, the final head H), all `supervised: true`, `queued_s: 0`:

| Gate | Exit | Tests | Log |
|------|-----:|-------|-----|
| `ci-steps-all` | 0 | 3400 passed, 0 failed | `…/logs/ci-steps-all/20261018T164558-633dc6f5.log` |
| `check-goldens-coverage` | 0 | — | `…/logs/check-goldens-coverage/20261018T165544-eefeb6fe.log` |
| `check-goldens` | 0 | 843 passed, 0 failed; no golden moved | `…/logs/check-goldens/20261018T165545-86b21947.log` |
| `cdo-gate` | 0 | 2318 passed, 0 failed; `cdo-gate: PASS` | `…/logs/cdo-gate/20261018T165706-a53d2dd8.log` |

`git status --porcelain -- . ':!.agent'` after each run: empty. Toolchain `rustc 1.96.0`; grammar `tree-sitter-al` at `8682cb5f`; `CDO_WS` = the pinned `DO-cdo-baseline/Cloud` worktree at `bc3ccb18` (identity hash: the commit SHA).

## Discoveries

```json
[{"subsystem": "program model / event symbols (src/program/model/events.rs)", "locator": "src/program/model/events.rs:116 parse_isolated; :102 publisher_event_kind", "symptom": "parse_isolated never reads [InternalEvent(IncludeSender, Isolated)]'s Isolated (arg 1, event.rs:285 doc), and publisher_event_kind maps InternalEvent to 'unknown', so an isolated InternalEvent publisher is never an isolation barrier.", "kind": "bug", "origin_issue": 104, "reproducer": "a codeunit with [InternalEvent(false, true)] procedure OnFoo() and a subscriber: isolated_event_ids does not contain OnFoo's event id.", "pre_existing": true, "capability": "an isolated InternalEvent publisher is an isolation barrier like an isolated IntegrationEvent/BusinessEvent", "acceptance": "isolated_event_ids contains an [InternalEvent(false, true)] publisher's event id; a unit test pins it; r4f goldens triaged if they move"},
 {"subsystem": "program engine / event-flow edges (src/program/resolve/resolver.rs, src/program/resolve/edge.rs)", "locator": "src/program/resolve/edge.rs:438 Route (no element field); src/program/resolve/resolver.rs emit_event_flow_edges (~3300-3340)", "symptom": "A routine subscribing a platform field/action event for two elements (two fields' OnAfterValidateEvent) gets two SubscriberEntry rows, correctly, but the EventFlow edge from the synthetic platform publisher carries two Routine routes to the same subscriber that differ only in their conditions: Route has no element, so a consumer of the edge cannot tell which field each route belongs to and sees what looks like a duplicate.", "kind": "enhancement", "origin_issue": 104, "reproducer": "table T with fields A, B; a codeunit routine H with [EventSubscriber(ObjectType::Table, Database::T, 'OnAfterValidateEvent', 'A', false, false)] and the same for 'B': the program engine's OnAfterValidateEvent EventFlow edge has two routes to H with identical targets.", "pre_existing": true, "capability": "event-flow routes of a platform per-element event say which element they belong to (or one route per subscriber with the element set)", "acceptance": "the reproducer's EventFlow edge distinguishes the two routes by element (or carries one route per element-specific subscription with its element), pinned by a program-build test"}]
```

Filed at step 10 if this attempt merges.

## Not delivered

Every acceptance-matrix row (A1, A2, A3, A3b, A3c, A3d, A3e, A4, A5, A6, A6c) is met by a test that pins the use and has a discrimination proof. Not delivered, as stated non-goals: `publisher_kind` / `event_kind` for readings of DIFFERENT publisher kinds (first reading wins), the ABI tier, `InternalEvent`'s `Isolated` (a discovery), and per-element routes on event-flow edges (a discovery).

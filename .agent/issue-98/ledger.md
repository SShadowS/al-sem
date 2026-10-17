# Issue #98 ledger

## Claim

```json
{"run_id":"20261017-094507-e0b327","issue":98,"attempt":1,"session":"claude-code-session:cf05ae92-26c8-4a5d-a439-ddec4928220c","branch":"issue/98-agent-discovery-l5-digest-witne-a1","worktree":"U:\\Git\\al-sem-issue-98-a1","body_hash":"ce7943fca8eadc03d6c72092ea4fe9c1bba91b634aa365d314ce14fa3898d504"}
```

Issue body hash: `ce7943fca8eadc03d6c72092ea4fe9c1bba91b634aa365d314ce14fa3898d504`.

Reviewer roster: the operator told us (2026-10-15) to use `gpt-6.1-sol` and `claude-opus-5.5` instead of `gpt-6-astra` and `gemini-3.8-flash`. Marks are recorded as `reviews.sol` and `reviews.opus`, and we attest with `--substitute astra=sol --substitute flash=opus`.

## Classification

**bounded.** This is the same shape as #96's D5 (`witness_first_hop_operation_id`), applied to a second edge kind that has no call site:
- one new optional fact field, carried through the existing fact plumbing;
- one new arm in the shared first-hop matcher.

No new edge kind, no new graph layer and no detector logic change. The design below is the spec.

## Assumption probes

All probes ran on master `35d6d2d7`. Details, commands and workspaces are in `.agent/issue-98/investigation.md`.

| probe | result | evidence |
|---|---|---|
| the issue reproduces | confirmed. `digest --routine OnX` (OnX is the publisher): `DB_INSERT` has `viaPaths: []`, `sourceKind: unavailable` and an `evidence-unavailable` entry diagnostic. The fingerprint witness shows `first-hop-not-found`, detail `event-dispatch` | investigation §1 |
| the cone, prove and fingerprint already see the write | confirmed. Only the witness is missing. Prove answers `yes`, but with `viaPaths: []` | investigation §1 table |
| the BFS already walks event-dispatch hops | confirmed. Digest on the RAISER (`Raise` calls `OnX()`) gives the path `call Raise->OnX`, then `event-dispatch OnX->HandleOnX`, with source evidence at `Subscriber.al:7:8`. The bug exists only when event-dispatch is the FIRST hop | investigation §1 control |
| the event edge's `from` is the publisher procedure | confirmed. `combined_graph.rs:754-797` uses `from: publisher_routine_id`, `event_id: Some(sym.id)`, no call site, no op. Every other typed-edge kind sets `event_id: None` | investigation §2 |
| one root has at most one dispatched event id | confirmed. A routine publishes at most one `EventSymbol` (`events.rs:324-331`) | investigation §2 |
| `(event_id, to)` is unique | **falsified for a corner case**. Two `[EventSubscriber]` attributes on one procedure for the same event give two byte-identical `EventEdge`s, with no dedup. This was not confirmed end to end. It is pre-existing and recorded as a discovery candidate | investigation §2 |
| evidence needs the event hop's anchor | **falsified**. Evidence comes from the path's TERMINAL hop (`evidence_from_terminal`, `digest.rs:2587-2614`). Once a path exists, the evidence is the subscriber's write | investigation §3 |

## Design

**Root cause.** `retag` (`src/engine/l4/capability_cone.rs` ~:1648) gives an inherited fact a first-hop identity: the edge's call site (`witness_callsite_id`), or since #96 its trigger op (`witness_first_hop_operation_id`). An `event-dispatch` edge has neither. A fact inherited across one therefore carries no first-hop id. In `src/engine/l5/digest.rs`:
- Case C of `reconstruct_witness_paths` (~:1162-1178) returns `first-hop-not-found` before the BFS;
- `enumerate_proofs` returns early (~:856);
- `is_first_hop` (~:345) has no arm that could match.

**D1. A first-hop event id on the fact.** Add `witness_first_hop_event_id: Option<String>` to `CapabilityFact`.
- **Where it is set.** `retag` sets it for EVERY edge from `edge.event_id`, and never inherits it from `rep`. It is `Some` exactly when the first hop is an event-dispatch edge; every other kind has `event_id: None`. This is #96's N4 rule: a stale id from a deeper hop must never survive a retag.
- **Which id.** The value is the INTERNAL event id, the same string that `SnapshotGraphEdge::EventDispatch.event_id` keeps, so the matcher compares like with like.
- **Plumbing.** Carry it through the same plumbing as `witness_first_hop_operation_id`:
  - `snapshot_fact`, `SnapshotCapabilityFact` and its `Serialize` (key `witnessFirstHopEventId`, written last and only when present). The id stays INTERNAL, as every other snapshot fact id does;
  - `PCapabilityFact` (`#[serde(default, skip_serializing_if = "Option::is_none")]`) and its projection. The projection writes it with `stable_event_id`, the same stable form the fact's own `resourceId` uses for event facts (`capability_cone.rs:3023-3033`). The op field uses `stable_sub_id`, but that form is for routine-rooted sub-ids and passes an event id through unchanged;
  - the capability-fact CBOR writer in `snapshot_full.rs` (~:223);
  - the required-permission `derivedFromCapability` writer (`snapshot_full.rs:769-783`, `make_required_perm`), which already writes `witnessFirstHopOperationId` and gains `witnessFirstHopEventId`;
  - the heap-byte census in `cone_census.rs:165-178` (byte accounting, not a writer);
  - every constructor that now sets the op field to `None`.
- **Memo identity.** `effect_fact_loop_identity` includes it, as D5 did for the op. This is DEFENSIVE only: the subject is already part of the identity, and a subject has one fact per key, so no test can tell the difference. That is stated here instead of being claimed as pinned.

**D2. Match by it, through ONE helper.** Today four sites read the first-hop ids. Two of them call `is_first_hop`: the `first_edges` filter (`digest.rs:1189-1193`) and the proof filter (`:921`). The other two test the ids directly: the Case C let-else (`:1164-1168`) and the `enumerate_proofs` early return (`:856`). A new first-hop kind could miss one of the direct sites. So:
- Add `enum FirstHop<'a> { Op(&'a str), Event(&'a str), Callsite(&'a str) }` and `fn fact_first_hop(f: &Fact) -> Option<FirstHop<'_>>`. It applies the precedence op, then event, then call site. The three are mutually exclusive by construction, because `retag` sets each one from the same edge.
- `is_first_hop` matches on `fact_first_hop(f)`. Its `Event(ev)` arm: an edge is a first hop iff it is `EventDispatch` and its event id equals `ev`.
- The Case C let-else and the `enumerate_proofs` early return both use `fact_first_hop(f)`, so all four sites share one definition.
- The snapshot edge's event id is read through ONE public accessor, `edge_event_id()`, beside `edge_operation_id()`/`edge_callsite_id()`. Per CR6, the former private `event_id()` was renamed to it; there is no wrapper.
- **Deviation from revision 2, from code review CR1.** Seeds whose first hop is an EVENT dispatch must pass the `valid_nodes` reachability prune. Call-site and op seeds keep their pre-#98 behaviour by scope choice, and pruning them too is a discovery. When every subscriber is pruned, the result is `terminal-not-found`, not `first-hop-not-found`. Pinned by three unit tests: `event_first_hop_does_not_seed_an_opaque_subscriber`, `…_a_subscriber_that_cannot_reach_the_write`, and `…_with_every_subscriber_pruned_is_terminal_not_found`.
- The `first-hop-not-found` detail for an unmatched first hop is the id `fact_first_hop` returned: the op, the event id or the call site.

**Why the event id alone, without `to`.** A first-hop key names the dispatch SITE, not one target. An interface-dispatch call site already fans out to several candidate edges, and a #96 op can fire several triggers. All subscriber edges of one dispatch run in the root's frame. The BFS carrier test and the proof filter keep only the paths that reach an equivalent fact. So the witness may list every writing subscriber, up to the path cap, and not only the subscriber the cone's tie-break picked. Adding `to` would pin the witness to one subscriber, and the key would still not be unique (the duplicate-subscription probe).

**Rejected: the zero-churn alternative.** We could match `fact.via == "event-dispatch"` against the edge kind, with no new field. It is rejected because it leans, through a lossy display string, on the invariant that a root dispatches at most one event, and it would over-match silently if that invariant changed. The explicit id is the honest identity and follows D5.

**Not changed.**
- **The event hop's anchor.** It stays `None`; the raise site belongs to the raiser's frame.
- **Conditionality.** An event hop's conditionality stays `UNKNOWN`. Today the `_` arm of `compute_path_conditionality` treats the hop as UNKNOWN, and that already makes every event-crossing path `unknown` for Raise-rooted digests too. Changing it moves every event path, so it is its own decision and becomes a discovery.
- **The `EVENT_PUBLISH` anchor.** The direct `EVENT_PUBLISH` fact on a publisher has no witness anchor. It keeps its own `evidence-unavailable` entry diagnostic, so prove's `witness_truncated` (`prove.rs:556-560`) stays true for every publisher root. This is a discovery and outside this issue.

**False-positive / failure shapes.**
- **(a) Over-matching.** With a first-hop event id, a non-event edge never matches, because the arm checks the edge kind. A call-site or op fact never matches an event edge, because those edges carry no call site and no op.
- **(b) A fact whose representative came through subscriber S1 now gets a witness through S2.** This is allowed. Both reach an equivalent fact; this is the carrier contract and the same as interface fan-out.
- **(c) Cost.** No new traversal MECHANISM, but new work. Publisher-first facts used to return before the BFS. Now they traverse, bounded by the existing path and state caps from #97. That work is measured (see the measurement plan). `CapabilityFact` also grows by one `Option<String>` (24 B), and that is measured too.

## Acceptance matrix

| Acceptance item | Proof |
|---|---|
| Event-dispatch first hops are identified by the dispatch edge | (A1) a `retag` unit test: an event edge sets `witness_first_hop_event_id = edge.event_id`, and a later non-event retag clears a stale id. (A1b) `is_first_hop` unit tests: an event fact matches the event edge with the same id, does not match an event edge with a different id, and does not match a call or trigger edge. A call-site or op fact does not match an event edge |
| The witness BFS and the proofs follow them | (A2) CLI test: a digest of the publisher (`--routine OnX`) gives `DB_INSERT` `viaPaths[0] == [event-dispatch OnX->HandleOnX]`. It has no `evidence-unavailable` entry diagnostic whose `effectType == "DB_INSERT"`. The direct `EVENT_PUBLISH` keeps its own diagnostic, which is a non-goal. The fingerprint witness for the OnX `insert` fact (OnX is rooted by `roots.config.json`) has `paths` non-empty and no `first-hop-not-found`. (A2b) proofs: a parameter-dependent write through the event first hop. The publisher is `OnX(var L: Record "I98 Log")` and the subscriber is `HandleOnX(var L: Record "I98 Log")`, which does `L.Insert()`. The parameter NAME must match, because `event_pair` binds by name (`pd_solve.rs:111-118`). `cross_edge` maps the parameter-dependent state across the event edge, so the fact on OnX is a proof root reached through `enumerate_proofs`, and raw PD producers are not BFS carriers. The test asserts a viaPath through the event hop for that fact. (A2c) fan-out: a publisher with TWO subscribers that both insert into the table. `viaPaths` holds both event-dispatch first hops, which pins "event id alone, without `to`" |
| A test shows source evidence and a viaPath for a write reached through a subscriber | (A3) the A2 test also asserts `evidence.sourceKind == "source"` at the subscriber's `Log.Insert` anchor |
| Serialized shape | (A4) extend the existing snapshot_full CBOR/permission test pattern (`snapshot_full.rs:1970-2029`): an event-first-hop fact writes `witnessFirstHopEventId`, both on the capability fact and in `derivedFromCapability`. A non-event fact writes neither key. The `PCapabilityFact` projection gives the stable event form |
| Discrimination | Delete the `Event` arm of `is_first_hop`: A2, A3 and A2c fail. Make `retag` inherit the event id from `rep`: A1 fails. Make Case C ignore `Event`: A2 fails. Make the `enumerate_proofs` early return ignore `Event`: A2b fails. Pin the representative subscriber: A2c fails. The fact carries no subscriber id, so a literal "add `to`" cannot be written. The proof truncates the event first edges to one in Case C, and A1b's `same_other_sub` assertion pins the `to`-free matcher at unit level. Drop the `derivedFromCapability` key: A4 fails. CR1: turn the event-seed prune off, and the three CR1 unit tests fail (0 pass, 3 fail) |

## Measurement plan

- **Goldens, predicted (investigation §5).**
  - Shape: the r3a3 goldens with `via: event-dispatch` facts gain `witnessFirstHopEventId` (26 files, `--test r3`). So do the cli-b snapshot/fingerprint shards and the CBOR for `ws-d8-commit-in-tx`, and the r4f `ws-txn-d47-event-pos.snapshot`.
  - Permission provenance: the `derivedFromCapability` rows in the snapshot/fingerprint required-permission output gain `witnessFirstHopEventId` where the permission comes from an event-first-hop fact.
  - Witness content: `cli-b-goldens/digest/ws-d8-commit-in-tx.json` (entry `OnAfterPostSalesDoc`: COMMIT and DB_INSERT gain a path and source evidence) and `r4f-goldens/ws-txn-d47-event-pos.digest` (entry `OnAfterProcess`: HTTP). A new path also feeds `factId` (it is built from `viaPaths[0]`'s link signature, `digest.rs:11-21`), canonical occurrence identity and ordering inputs (`digest.rs:2725-2765`, `:3595-3625`). So occurrence ids and possibly the r4f orderingfacts/scoped goldens move too.
  - Every moved line is triaged by `golden-diff-triager` before regen.
- **CDO.** `cdo-gate` must pass. If the frozen CDO L4 digests move, triage them, as #96 did.
- **Cost (publisher-first work is new, so it is measured), master vs branch, `release-fast` on CDO:**
  - time a `fingerprint --witness 1` over the whole workspace, which includes every rooted publisher-first fact;
  - time a digest of a publisher routine that has many subscribers;
  - compare peak RSS and the census capability-fact bytes, for the 24 B field. As run, the census was `C1_CONE_CENSUS`, which counts direct facts only. The analyze-path `ALSEM_CONES_CENSUS` was not run, so the cost of inherited facts is covered by peak RSS.

## Base/Head SHAs

- B = `35d6d2d7f0862399aeacb44326ce7f5e19f7d824` (origin/master at claim and at freeze; no rebase was needed)
- H = `35473af7dcfd3d3b30a0d1cc4dc8c1072912b35c` (the approved code head; the evidence commit follows it)

## Discoveries

1. **l5 / digest witness seeding (enhancement).** Call-site and op seeds of `reconstruct_witness_paths` still skip the `valid_nodes` prune. So an opaque candidate of an interface-dispatch call site, which cannot reach the carrier, is still seeded. It can be reported as a 1-hop `opaque-or-unresolved-boundary` path that sorts before the real path. #98 pruned only event seeds, as a scope choice. Pruning all seeds by the same rule would make the witness consistent.
2. **l4 / capability-fact first-hop identity (enhancement).** A fact names its first hop through three optional fields: `witness_callsite_id`, `witness_first_hop_operation_id` and `witness_first_hop_event_id`. Each edge kind that can be a first hop needs a new field threaded through about 12 sites. One `FirstHopSite` enum (call site | op | event) would replace them. The witness wire keys and goldens move with it.
3. **l5 / digest path conditionality (enhancement).** `compute_path_conditionality` has no `EventDispatch` arm. An event hop falls to `_` and pushes `UNKNOWN`, which makes every event-crossing path `conditionality: unknown`, even when the call and the write are unconditional. The ordering engine models the same hop as May (`event-subscriber-may`). A `CONDITIONAL` arm, like #96's trigger arm, would be more precise.
4. **l5 / digest `EVENT_PUBLISH` evidence (bug).** The direct `EVENT_PUBLISH` fact on a publisher procedure has no witness anchor. It always yields `evidence-unavailable`, and the inherited `EVENT_PUBLISH` on a raiser is `unavailable` too, because its terminal is synthetic. So prove's `witness_truncated` (`prove.rs:556-560`) is true for every publisher root, and a must-question or a `no` answer there gets a `witness-truncated` blocker.
5. **program/model events (bug, unconfirmed end to end).** One procedure with two `[EventSubscriber]` attributes for the same event yields two byte-identical `EventEdge`s (`events.rs:377-541`, no dedup). That would make two identical typed edges and could fire the `derive_pd_arena` `debug_assert!(prev.is_none())` (`snapshot.rs:1533-1537`) in a debug build. The release build digests normally, and whether the subscriber index really emits two subscriptions was not confirmed.

## Timeline

- Worktree created at master `35d6d2d7`.
- Investigation (opus, read-only; subagents 1): `.agent/issue-98/investigation.md`.
- Classified as bounded; the design above is the spec.
- Spec panel (sol + opus, the operator roster). Register hash = sha256(ledger.md)[:16] at the round:
  - r1 (spec_rounds 1; pi_calls 2; hash 6ac71c9c9eb00d59): both confirmed the root cause, the event-id-only match and the encodings. Sol raised 1 important finding and 3 minor ones, opus 1 important and 8 minor (13 merged entries S1-S13; S1 = A2 diagnostic scope, important). Revision 2 fixed all of them.
  - r2 (spec_rounds 2; pi_calls 4; hash dce3fe8d863a79cb): both accepted S1-S13; no new findings. CONVERGED.
- Plan: `docs/superpowers/plans/2026-10-17-issue-98-event-dispatch-first-hop.md`, tasks T0-T2 (plan_tasks 3).
- T0 (opus; subagents 2; task_attempts T0 = 1): `tests/cli/cli_event_dispatch_first_hop.rs`, 9 tests.
  - 4 are RED for the stated reasons: A2/A3 (no path, `unavailable`), A2 fingerprint (`first-hop-not-found` detail `event-dispatch`), A2b (no path), A2c (neither subscriber path).
  - 5 are green by design: the fixture self-check, the Raise controls (EVT, FAN), the OnX fact-exists control, and the A2b precondition (OnX insert is `parameter-dependent`; HandleOnX's direct write is the only producer, so any path must come from proofs).

  Report: `.agent/issue-98/t0.md`.
- T1 (opus; subagents 3; task_attempts T1 = 1): PASS.
  - **Code:** D1 + D2 as specified.
  - **Results:** lib 2067 green; the 9 CLI tests green (A2b turned green without a temp-class fix); clippy clean.
  - **Proofs (a)-(f)** each fail and pass after revert. (e) is a `truncate(1)` single-subscriber pin, because the fact carries no subscriber id to add `to` with.
  - **Golden inventory:** cli 10, r3 1 (r3a3 54 key additions in 26 fixtures), r4 2. Witness content moved in ws-d8 and the d47 digests. Ordering guarantees knock on in the ws-d8 `OnAfterPostSalesDoc` entry (needs triage).

  Report: `.agent/issue-98/t1.md`.
- T1 review (sonnet; subagents 4): no critical or important findings. One stale doc (`cone_derived.rs:36`, "those four") fixed. The CHANGELOG entry is added. The proof (e) wording in the ledger is updated. Report: `.agent/issue-98/t1-review.md`.
- Golden triage (golden-diff-triager; subagents 5): 39 MOVED, UNEXPLAINED 0.
  - (1) Key shape: r3a3 54 lines in 26 files; ws-d8 snapshot/fingerprint shards and CBOR; the d47 snapshot.
  - (2) Witness content: the ws-d8 and d47 publisher digests.
  - (3) Ordering knock-on: the ws-d8 `OnAfterPostSalesDoc` COMMIT/DB_INSERT guarantees, verified against `subscriber.al` (Insert then Commit, no branch), and identical to the raiser's committed labels.
  - The r3a3 regen also rewrites the key order inside `extra` objects (−450/+450) in those 26 old-format files. It is format-only, because the comparison is structural.

  Report: `.agent/issue-98/golden-triage-report.md`; receipt: `.agent/golden-triage.md`.
- T2 regen (`agentflow run --name check-goldens-regen`, exit 0, 164.9 s, supervised; log `.agent/runs/20261017-094507-e0b327/logs/check-goldens-regen/20261017T110415-ba0635c7.log`): moved exactly the 39 triaged files, no manifest matrix.
- Commit `afb4b231`: code, tests, goldens, CHANGELOG, plan. The pre-commit hook goldens are OK, and `check-diff` gives `reasons: []`.
- Gates (step 9) at `afb4b231`, all supervised, all exit 0. Logs are under `.agent/runs/20261017-094507-e0b327/logs/`.
  - ci-steps-all: 743.5 s (`ci-steps-all/20261017T110909-c7dc6239.log`).
  - check-goldens-coverage: 0.5 s (`check-goldens-coverage/20261017T112132-02435a25.log`).
  - check-goldens: 74.5 s (`check-goldens/20261017T112133-9da4a33a.log`).
  - cdo-gate: 225.2 s (`cdo-gate/20261017T112248-e4495961.log`).

  `git status --porcelain -- . ':!.agent'` is empty.
- Final panel: the `code-review` skill at high, plus the CDO cost measurement (opus; subagents 6), started.
- CDO cost (opus, release-fast, master `35d6d2d7` vs `afb4b231`, before the CR1 prune, which only removes work; alternating runs; exact OS peaks). Report: `.agent/issue-98/cdo-cost.md`.
  - **Whole-workspace `fingerprint --witness 1`:** wall +47 s against a 198 s spread within one binary; peak RSS +0.5 MB. The output is byte-identical: publishers are not default roots.
  - **Digest of `CDOEvents.Codeunit.al`** (156 roots, all of CDO's own publishers): wall −3 s and RSS +1 MB, both inside noise.
  - **Census:** +24 B per `CapabilityFact`, +0.79 MB (+0.79%).
  - **Output:** 14 publisher-rooted effects gain an event-dispatch-first path and source evidence, their `factId` changes, and 14 `evidence-unavailable` diagnostics go. The ordering knock-on: 13 effects go from `ORDERING_UNKNOWN` to `[]`, and 1 COMMIT gains `COMMIT_REACHABLE` and `COMMIT_DOMINATES_RETURN`. Nothing else moves.
  - **Gap:** CDO's many-subscriber publishers are all in dependencies, and the digest selector cannot root them. The largest case measured has 1 subscriber.
- `code-review` (high), 6 findings, recorded as CR1-CR6:
  - CR1 (important): event seeds skipped the `valid_nodes` prune, so an opaque subscriber could win with cap 1.
  - CR2 (important): no test covered that shape.
  - CR3 (minor, deferred): one first-hop identity; filed as a discovery.
  - CR4 (minor): stale docs.
  - CR5 (minor): refuted; the ledger is committed at the freeze.
  - CR6 (minor): duplicate accessor.
- CR fix (opus; subagents 7; task_attempts CR = 1), commit `d9827e66`:
  - Event seeds obey the prune; call-site and op seeds are unchanged; the all-pruned case gives `terminal-not-found`.
  - 3 new unit tests. With the prune removed, 0 pass and 3 fail; restored, all pass.
  - A2/A2c assert the exact set of `viaPaths`.
  - One accessor, `edge_event_id`; docs updated.
  - lib 2070 green, cli 9 green, clippy clean, check-goldens green, no golden moved. Report: `.agent/issue-98/cr-fix.md`.

  The step-9 gates must run again at `d9827e66`.
- Gates rerun at `d9827e66`: all supervised, all exit 0.
  - ci-steps-all: 616.4 s (`ci-steps-all/20261017T114236-a6ad1c2c.log`).
  - check-goldens-coverage: 0.8 s (`check-goldens-coverage/20261017T115252-d7aba22c.log`).
  - check-goldens: 58.4 s (`check-goldens/20261017T115253-695f7955.log`).
  - cdo-gate: 166.1 s (`cdo-gate/20261017T115352-eea5686c.log`).

  The porcelain check is empty.
- Final panel (sol + opus). Register hash = the first 16 hex of sha256 of `git diff master..HEAD -- . ':!.agent'`:
  - r1 (final_rounds 1; pi_calls 6; hash 09e252bf2b8946cf): both accepted all 19 entries, with no critical or important new finding. The new minors are F1-F3:
    - F1: the seed-prune scope wording and the may-execute overstatement;
    - F2: stale ledger D2, and the CR1 deviation not recorded;
    - F3: the `fact_first_hop` doc.

    All three were fixed in `35473af7` (comment, doc, ledger and triage text only).
- Gates rerun at `35473af7`: all supervised, all exit 0.
  - ci-steps-all: 632.4 s (`ci-steps-all/20261017T120527-c8a52450.log`).
  - check-goldens-coverage: 0.5 s (`check-goldens-coverage/20261017T121600-868edb05.log`).
  - check-goldens: 65.3 s (`check-goldens/20261017T121600-54dfb7e1.log`).
  - cdo-gate: 164.3 s (`cdo-gate/20261017T121706-c3391a62.log`).

  The porcelain check is empty.
  - r2 (final_rounds 2; pi_calls 8; hash 417b12eabac21904): both accepted all 22 entries, and both judged the `ORDERING_UNKNOWN` → `[]` knock-on correct. One new minor, G1: the census naming in the ledger and in `cdo-cost.md`. Fixed in the evidence text only; the code is unchanged.
  - r3 (final_rounds 3; pi_calls 10; hash 417b12eabac21904): both accepted all 23 entries; no new findings. CONVERGED.
- Rebase (step 11): `origin/master` is `35d6d2d7`, an ancestor of HEAD. No rebase was needed, so no gate rerun and no extra panel round.
- Freeze (step 12): `check-diff` over B..H gives `reasons: []`; `sanitize` gives `violations: {}`. The ledger and `findings.json` are committed as the evidence commit.

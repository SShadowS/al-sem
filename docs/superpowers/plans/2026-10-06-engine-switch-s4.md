# Engine switch S4 — event inventory (G7, #57)

Spec: `docs/superpowers/specs/2026-10-06-engine-switch-design.md` (rev 3), G7 and S4.
Branch: `engine-switch/s4-event-inventory`, from master `496b7424` (S3 merged).

## Problem

`alsem analyze` builds the detector event graph with L3's `build_event_graph`. It looks
publishers up in the workspace symbol table only. On CDO, 54 of 56 subscriber edges are
`unknown` (the publisher is in a dependency), and `build_event_flow_indexes` keeps only
`resolved` edges, so d43/d44/d45 never see them. The program engine already binds those
subscriptions (`SubscriberIndex`, including platform table/page events through synthetic
publishers), but it silently drops a subscription whose publisher object it cannot find.

## Steps (one commit each)

**S4.1 — Program subscription inventory.** `SubscriberIndex` keeps EVERY parsed
subscription, in graph routine order then attribute order, with its outcome: bound
(publisher routine, conditions, element), ambiguous (publisher object, candidate count),
orphaned (publisher object found, no eligible publisher routine), object unresolved
(type and name as written; also an unknown object type). `ambiguous`/`orphaned` become
views over this one list. `emit_event_flow_edges` is unchanged, so the call graph and
`--program-call-graph-stats` must stay byte-identical. Additive; no output change.

**S4.2 — Detector event graph from the inventory.** New adapter
`engine/l3/program_events.rs` (sibling of `program_calls.rs`; renamed/moved in S9)
builds the detector `EventGraph` from the inventory. `attach_program_calls` sets
`L3Resolved::precomputed_events`; `build_detector_context` uses it when present (the
`alsem analyze` path and the r4/r4f goldens). Every other consumer keeps L3's builder
until S6.
- Population: the model's subscriber routines, in model order (the routines the detectors
  see), each joined to its program node through its declaration anchor. Every
  subscription of that node becomes one edge (L3 read only the FIRST `[EventSubscriber]`
  of a routine).
- Workspace publisher symbols: as today, from the model's publisher routines.
- Edge resolution: bound -> `resolved`; ambiguous -> `ambiguous` (new, not proven);
  orphaned -> `maybe`; object unresolved -> `unknown`. Every consumer tests
  `!= "resolved"`, so the new string is excluded from proven indexes.
- A publisher that is not a model routine (dependency publisher, platform-synthetic
  publisher) gets an `EventSymbol` with `publisher_routine_id: None` and a new
  `publisher_ref` (its program identity, in `ExternalTargetRef.target`'s format, and its
  dependency body state). The combined graph, cones, fan-out and d43/d45 key on
  `publisher_routine_id`, so they do not change; `subscribers_by_event` gains the
  workspace subscribers of such events, which is what d44 reads.
- Edges also carry the subscription's element filter and conditions (manual binding,
  skip-on-missing-license/permission), per G7.
- No fallback: a model subscriber with no program node is counted and emitted as
  `unknown` with a note.
Tests (hand-stated preconditions, each with a discrimination proof):
  (a) end-to-end through the analyze path: a workspace with a symbol-only dependency
      whose codeunit publishes an integration event, and two workspace subscribers that
      write the same table -> one d44 finding; break: map a bound dependency publisher
      to `unknown` -> the finding disappears.
  (b) G7: an unresolved subscription stays in the graph and drives fan-out coverage
      (`dispatch_edges` = `partial`); break: drop non-bound subscriptions -> `complete`.
  (c) the second `[EventSubscriber]` of one routine produces its own edge.

**S4.3 — Measure, triage, document.** Harness `s3-6` vs `s4`, `--b3`,
`--program-call-graph-stats` (must be byte-identical), full findings both scopes. Every
new finding triaged against CDO source (`triage-findings`), golden triage receipt,
CHANGELOG, spec "as built" note, OUTSTANDING.

## Decisions recorded here (spec: "decided here, implemented in S8")

- **d43** needs the RAISER of a dependency event (a dependency call site) and its guard
  analysis: dependency bodies, S8. Unchanged in S4.
- **d45** (owner decision requested): proposed — a dependency publisher is a d45 root
  when at least one primary routine is in its subscriber chain; its own writes come from
  its summary once S7/S8 provide dependency summaries, and until then the finding states
  the publisher's coverage as `unknown`. Not implemented in S4.
- **d38** (obsolete subscription) could flag subscriptions to obsolete dependency events
  once dependency publisher attributes (`Obsolete`) are kept; follow-up, not S4.

## Known limits

- A `[EventSubscriber]` whose arguments do not parse is dropped by both engines before
  the inventory (`node_extract`); not counted. Follow-up if a population appears.

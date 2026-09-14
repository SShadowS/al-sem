# Issue 25 — d19 must not flag event-publisher parameters

Spec: `.agent/issue-25/ledger.md` (`## Design`, `## Acceptance matrix`), hash `2176f1a1973906af`.
Spec panel converged round 2, 12/12 accepted by both reviewers.

## Task 1 (only task) — skip `event-publisher` in d19, with the counter and the tests

**Change.** In `src/engine/l5/detectors/d19.rs`, after the existing `"event-subscriber"`
skip, add a `"event-publisher"` skip plus a `skipped_event_publisher` counter wired into the
detector's stats exactly like `skipped_trigger` / `skipped_event_subscriber`.

Ordering is load-bearing: `ir_routine_kind` gives `eventsubscriber` precedence, so the
publisher check MUST come after the subscriber check or a dual-attribute routine is booked
against the wrong counter.

**Tests.** TDD — red first, in the same commit as the fix (a test committed alone would fail
the pre-commit golden gate). The test drives REAL workspace assembly and the REGISTERED
detector, never a helper, and must assert assembly actually produced the routines rather than
degrading to an empty result (see `tests/r4/r4_differential.rs:1346-1355` for the shape to
avoid — it lets a negative test pass without analysing anything).

Cases:
1. `[IntegrationEvent]` publisher with unused parameters → 0 findings
2. `[BusinessEvent]` publisher with unused parameters → 0 findings
3. mixed ASCII casing (e.g. `[integrationEVENT]`) → 0 findings
4. ordinary procedure with a genuinely unused parameter → STILL 1 finding (positive control)
5. `[EventSubscriber]` books a SUBSCRIBER skip, not a publisher skip (pins the ordering)
6. the publisher skip statistic counts ROUTINES, not parameters

**Corpus regression** (assert or measure and record): d19 findings `ws-d59` 6→0,
`ws-d38` 3→0, `ws-d12-dead-event` 1→0, `ws-d19` 2→2.

**Discrimination proof.** Remove ONLY the new skip, assert the patch applied (`count == 1`),
record the REAL failing output, restore, record the REAL passing output. A break that comes
back green is evidence about the test, not the code.

**Prediction to verify, not assume:** no committed golden moves. r4 goldens are per-detector
and none of the publisher fixtures lists d19; zero-valued skips are omitted from stats
serialization (`registry.rs:169-175`). `scripts/check-goldens` is the verification.

**Out of scope:** `[InternalEvent]` — filed as a discovery against `ir_routine_kind`.

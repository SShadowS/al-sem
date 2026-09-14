# Issue 25 — d19-unused-parameter fires on `[IntegrationEvent]` publisher parameters

## Claim

```json
{
  "run_id": "20260914-000607-73686e",
  "issue": 25,
  "attempt": 1,
  "branch": "issue/25-agent-discovery-l5-d19-unused-p-a1",
  "worktree": "U:\Git\al-sem-issue-25-a1",
  "body_hash": "ddff390785f89338af0272d639e3db1c29100e70f53df46c8dcd304d4dde5562"
}
```

## Classification

**bounded** — one detector, one skip, mirroring two skips the same function already has.
The issue's own suggested mechanism is not the one the codebase already provides; see P2.

## Assumption probes

| # | Assumption | Result | Evidence |
|---|---|---|---|
| P1 | d19 fires on an `[IntegrationEvent]` publisher's parameters | **HOLDS — measured** | `alsem analyze --detector d19-unused-parameter` on the r0 corpus: `ws-d59` **6** findings (every routine there is an `[IntegrationEvent]` publisher with an empty body, incl. the textbook `OnBeforePost(var IsHandled: Boolean)`), `ws-d38` **3**, `ws-d12-dead-event` **1**. Against `ws-d19` (ordinary procedures) **2** genuine. So 10 FP vs 2 TP in the corpus alone |
| P2 | The fix is a `has_attribute` gate mirroring d53/d61 (the issue's proposal) | **FALSIFIED — a better mechanism already exists** | `ir_walk.rs:2517` `ir_routine_kind` ALREADY returns `"event-publisher"` for `integrationevent`/`businessevent`. d19 already skips by `routine.kind` for `"trigger"` and `"event-subscriber"` (`d19.rs:44-52`). The consistent fix is a THIRD kind skip, not a new attribute call. Also the issue's "d53 and d61" is half wrong: only **d53** uses `has_attribute`; d61 has no such call |
| P3 | IntegrationEvent + BusinessEvent is the whole publisher population | **FALSIFIED — `InternalEvent` is a third** | `app_package.rs:76` `is_publisher()` = `IntegrationEvent \| BusinessEvent \| InternalEvent`. `ir_routine_kind` covers only the first two, so an `[InternalEvent]` publisher classifies `"procedure"` and will STILL false-positive after this fix. **Overstated by me, corrected by the panel:** the corpus's only `InternalEvent` routine, `OnInternalSignal()`, takes NO parameters, so this is a classification gap and NOT an active d19 false positive today. Filed as a discovery rather than folded in: closing it changes L2 classification, which moves `tests/r1a-goldens/ws-r0-canon-stress.l2.golden.json` AND shifts routine IDs (`ir_routine_kind` also feeds `kind_for_id` at `l3_workspace.rs:903`) |
| P4 | The issue's reproducer command runs as written | **FALSIFIED (minor)** | `alsem analyze <ws> --format json --min-severity info` cannot produce a d19 finding: `analyze` takes the workspace as a POSITIONAL, and d19 is in neither preset (`transaction-integrity`, `bcquality` = d52–d64). Reproducing needs `--detector d19-unused-parameter`. Recorded so the next reader does not repeat the dead end below |

### Probe-method note (my own error, recorded because it nearly inverted the result)

My first three probe runs reported **0** findings everywhere, including on `ws-d19`, whose
committed r4 golden has 2. I had read `d['findings']`; the analyzer's JSON nests them at
`payload.findings`. The zero was my parser, not the detector. The control run (`ws-d19`,
known-nonzero) is what caught it — without it I would have recorded "premise falsified,
d19 does not fire" and closed the issue wrongly. Same key-path mistake occurred earlier in
this session; it is a recurring trap with this CLI's JSON envelope.

## Design

Add a third `routine.kind` skip to `detect_d19`, **placed after** the two it already has:

```rust
if routine.kind == "event-publisher" {
    skipped_event_publisher += 1;
    continue;
}
```

plus the matching `skipped_event_publisher` stat, mirroring `skipped_trigger` /
`skipped_event_subscriber`. The statistic counts **routines**, not parameters.

**Why by kind, not by attribute.** d19 expresses its existing exclusions as `routine.kind`
comparisons, and `ir_routine_kind` is the single place mapping attributes to kinds. L3 does
not copy an L2 projection — it CALLS that classifier, at `l3_workspace.rs:1257` for
`L3Routine.kind` and again at `:903` for `kind_for_id`. Re-deriving "is this a publisher?"
from `attributes_parsed` inside d19 would stand up a second, independently-maintained answer
beside the first — the duplication shape issue #10 is open about for `ROOT_KIND_VALUES`.

**Ordering is load-bearing.** `ir_routine_kind` gives `eventsubscriber` precedence
(`ir_walk.rs:2519-2522`), so a routine carrying both a subscriber and a publisher attribute
classifies `"event-subscriber"`. Putting the publisher check after the subscriber check keeps
that routine booked against `skipped_event_subscriber`, preserving the existing accounting.

**Scope of the equivalence claim.** For `IntegrationEvent` and `BusinessEvent` the kind check
and an attribute gate are equivalent; this is NOT claimed universally. `InternalEvent` matches
neither. A Unicode divergence also exists between the two paths (`fold_identifier` folds
Turkish dotted-I to `i`, `has_attribute` uses `to_lowercase` — `casing.rs:12-35` vs
`al_attributes.rs:62-64`); it cannot arise here because the kind path never calls
`has_attribute`, but it is recorded rather than claimed away.

**What is being asserted, and what is not.** A publisher body is empty by language
definition — the compiler generates the dispatch — so d19's body-local predicate is 100%
false-positive on that population by construction. Its parameters exist for subscribers to
read as well as write (`ws-d59` carries a non-var informational parameter alongside the
`var IsHandled` handshake). The claim is that **d19 cannot evaluate this property**, NOT that
a publisher parameter can never deserve removal — that question needs subscriber-use evidence
and belongs to the event-graph detectors (`d12` already covers the dead-event case).

**Deliberately not done:** `InternalEvent` (P3). Out of scope for this Acceptance, not an
active false positive, and closing it carries L2-classification plus routine-ID blast radius.
Filed as a discovery against `ir_routine_kind` — the single definition — never as a
d19-local attribute special case, which would create exactly the second definition this
design avoids.

## Acceptance matrix

| item | proven by |
|---|---|
| An `[IntegrationEvent]` publisher's parameters produce no d19 finding | integration test over a real assembled workspace + the registered detector; asserts assembly produced the routines (never an empty-result fallback — see `r4_differential.rs:1346-1355` for the shape to avoid) |
| A `[BusinessEvent]` publisher's parameters likewise | same test; the kind covers both, which the issue's IntegrationEvent-only proposal would not have |
| Mixed ASCII casing (`[integrationEVENT]`) still suppressed | same test — attributes are lowercased at `ir/decl.rs:152` |
| An ordinary procedure's genuinely unused parameter STILL fires | the positive control, without which "skip everything" would pass |
| An `[EventSubscriber]` stays booked as a subscriber skip, not a publisher skip | assert the two counters separately; pins the ordering above |
| The skip statistic counts routines | `ws-d59` 5, `ws-d38` 3, `ws-d12-dead-event` 1 (routines), against 6/3/1 findings suppressed |
| Corpus regression | d19 findings: `ws-d59` 6->0, `ws-d38` 3->0, `ws-d12-dead-event` 1->0, `ws-d19` 2->2 |
| ~~**No committed golden moves**~~ **FALSIFIED — see Gate results** | `scripts/check-goldens` byte-clean. Predicted zero: r4 goldens are per-detector and none of the publisher fixtures lists d19; zero-valued skips are omitted from stats serialization (`registry.rs:169-175`). This is a prediction the gate VERIFIES, not an assumption |

Discrimination proof: remove ONLY the new skip, assert the patch applied, record the real
failing output, restore, record the real pass.

## Implementation notes

### Test location, and why

`tests/gap/gap_d19_event_publisher_skip.rs`, registered in the `gap` umbrella's
`main.rs`.

The `gap` umbrella is the repo's existing home for detector-behaviour audits driven
by REAL in-memory workspace assembly — `gap_audit_d20_break.rs` is the template this
file follows exactly (`assemble_and_resolve_default` → `registered_detectors()` →
`run_detectors`). That shape satisfies the Acceptance requirement to drive assembly
and the registered detector rather than a helper.

Rejected alternatives:

- **A `#[cfg(test)]` module inside `d19.rs`.** The five detectors that have one
  (`d1`, `d17`, `d50`, `d59`, `d63`) all test *helper* functions — `d63`'s tests call
  `looks_like_html_concat` directly. That is precisely the "test pins the function,
  not the use" shape CLAUDE.md forbids: the production call site could be deleted and
  those tests stay green.
- **`tests/r4/`.** That umbrella is golden-differential territory (`r4_differential`
  plus the `r4f_*` projections); a hand-written behaviour test is not a golden
  projection, and adding a member there invites golden-gate coupling for no gain.

The anti-degenerate guard is `D19Run::assert_assembled`, which asserts the named
routine exists AND carries the expected `kind`, parameter count, `body_available`
and `!parse_incomplete`. Without it, a fixture that failed to parse would analyse
zero routines and every "expects 0 findings" case would pass while proving nothing —
the failure shape called out at `tests/r4/r4_differential.rs:1346-1355`.

### The "ordering is load-bearing" claim is FALSE — measured

The `## Design` section above, and the plan, both state that d19's check order is
load-bearing: put the publisher check before the subscriber check and a
dual-attribute routine is booked against the wrong counter.

**Falsified.** Swapping the two blocks in `detect_d19` left all 7 tests green. The
reason is structural: d19's three checks are mutually-exclusive equality tests
against ONE scalar, `routine.kind`, which by then already holds a single decided
value. Reordering equality tests on an immutable scalar cannot be observed.

The precedence is real, but it is resolved UPSTREAM, in `ir_walk::ir_routine_kind`'s
branch order — which is the correct place for it and exactly why d19 does not need to
know. Inverting that branch order fails
`dual_attribute_routine_books_the_subscriber_counter` at its `assert_assembled` line
with `left: "event-publisher", right: "event-subscriber"`. So the test does pin the
precedence; it just pins it where the precedence actually lives.

The misleading comment that this task first wrote into `d19.rs` was corrected to
record the measurement rather than repeat the claim.

### The "no committed golden moves" prediction is FALSE — measured

`scripts/check-goldens` (run with `CDO_WS` unset) is green on 8 of its 9 targets.
`--test cli` fails with 6 genuine byte-mismatches (+3 `ENV_LOCK` poison cascades
downstream of them), all on ONE fixture:

`tests/r0-corpus/ws-d8-commit-in-tx/src/posting.al` declares
`[IntegrationEvent(false, false)] procedure OnAfterPostSalesDoc(Header: Record "Sales Header")`
with an empty body. d19 was flagging `Header` — a textbook instance of the false
positive this issue is about. Suppressing it moves:

| golden family | delta |
|---|---|
| `tests/cli-a-goldens/stats/ws-d8-commit-in-tx.{default,all}.json` | d19 `candidatesConsidered` 1→0, `findingsEmitted` 1→0, `skipped` gains `"eventPublisher": 1` |
| `tests/cli-a-goldens/json/ws-d8-commit-in-tx.{default,all}.json` | findings 6→5, `byDetector.d19-unused-parameter` removed, `bySeverity.info` 3→2, `totalFindings` 6→5 |
| `tests/cli-a-goldens/terminal/ws-d8-commit-in-tx.*` | `"INFO (3):"` → `"INFO (2):"` |
| `tests/cli-a-goldens/html/ws-d8-commit-in-tx.{default,all}.html` | tally info 3→2 |
| `tests/gate-goldens/` pr-summary `ws-d8-commit-in-tx.default` | `"3 info"` → `"2 info"` |
| `tests/gate-goldens/` SARIF `ws-d8-commit-in-tx.default` | the d19 rule/result removed |

Every moved line is the intended suppression. Nothing else moves: r4/r4f, l2_ir,
differential (r0/r1a/r2\*), r3, r25_abi, l4-summary-baseline and semantic-edges are
all byte-clean. The plan's prediction was reasoned about r4 goldens only, where it
was correct; it did not account for the cli-a/gate families, which run the FULL
detector set over a corpus workspace that happens to contain a publisher.

**The regen was NOT run** — a `PreToolUse` hook blocks any command matching
`REGEN_TEMP_GOLDENS=1` / `check-goldens --regen` / `mint-goldens` unless
`GOLDEN_TRIAGED=1` is set in the HOOK's own environment, which a tool call cannot
set (a command-line prefix does not reach it). The triage the hook asks for is the
table above. Hand-off command:

```
GOLDEN_TRIAGED=1 TREE_SITTER_AL_PATH=… scripts/check-goldens --regen   # CDO_WS unset
```

`CDO_WS` must be unset for that run: the workspace it points at has drifted from its
mint-time SHA (`64643a2f…` → `bc3ccb18…`), so a regen with it set would rewrite
`tests/l4-summary-baseline/` from drifted data.

### Pre-existing failures, proven not caused by this change

With `CDO_WS` set, 6 CDO-gated tests fail (`cdo_whole_program_v2_matches_frozen_digest`,
`cdo_reverse_index_matches_slow_oracle`, `cdo_full_program_coverage_and_self_reported_metric`,
`cdo_genuine_wrong_is_precedence_adjudicated`, `cdo_l3_semantic_audit_no_fresh_wrong`,
`cdo_trigger_audit_frozen_load`). Proven pre-existing rather than assumed: reverting
`d19.rs` to its `HEAD` content and re-running `--test l4_summary_differential --test
program_resolve_harness` reproduces the identical 6 failures. They are CDO workspace
drift (the harness prints the SHA-drift warning itself, and one reports a missing
source path), and they are in resolution-engine targets that this L5-detector change
does not touch.

## Timeline

| when | phase | outcome |
|------|-------|---------|
| 2026-09-14T00:06Z | claim | ok, attempt 1 |
| 2026-09-14T00:1xZ | worktree + classify | bounded |
| 2026-09-14T00:2xZ | probes | 4 probes: P1 holds (measured 10 FP vs 2 TP), P2/P3/P4 falsified |

## Gate results

| gate | exit | notes |
|---|---:|---|
| `cargo test --test gap gap_d19_event_publisher_skip` | 0 | 7/7 pass |
| `cargo clippy --all-targets --all-features` | 0 | clean |
| `rustfmt --check` (touched files) | 0 | clean |
| `scripts/check-goldens` (CDO_WS unset) | **101** | 8 of 9 targets byte-clean; `--test cli` moves 14 `cli_a_*` tests. **Triaged and explained below.** Regen BLOCKED, see Not delivered |

### Golden triage — every moved line explained

Cause: `tests/r0-corpus/ws-d8-commit-in-tx/src/posting.al:18-21` carries

```al
[IntegrationEvent(false, false)]
procedure OnAfterPostSalesDoc(Header: Record "Sales Header")
begin
end;
```

and the committed stats golden encodes the false positive verbatim:

```json
{"detector": "d19-unused-parameter", "findingsEmitted": 1, "skipped": {"eventSubscriber": 1}}
```

After the fix that entry becomes `findingsEmitted: 0` with `skipped` gaining
`eventPublisher: 1`, and the fixture's totals go 6 findings -> 5 (info 3 -> 2). Families
touched: cli-a stats/json/terminal/html plus gate pr-summary and SARIF — all the SAME one
fixture. **No unexplained line.** This is precisely the false positive the issue removes.

**Both spec reviewers predicted zero golden movement and both were wrong** (ASTRA-06,
FLASH-06): they reasoned only about the r4 families, where they were correct, and nobody
checked cli-a/gate. Recorded as CONDUCTOR-01. The spec had framed zero-movement as "a
prediction the gate VERIFIES, not an assumption", so the process caught it — but the
prediction itself was wrong and is corrected here rather than quietly dropped.

### Pre-existing CDO failures (proven, not assumed)

The 6 CDO-gated failures reproduce identically with `d19.rs` reverted to its HEAD content,
so they are not this change's. They are CDO workspace drift (mint `64643a2f` vs current
`bc3ccb18`) plus a separate pre-existing `ambiguousResolved=67` ratchet breach, in
resolution-engine targets this change does not touch.

## Not delivered

**The golden regeneration.** The regen is blocked by a `PreToolUse` hook requiring
`GOLDEN_TRIAGED=1` in the HOOK's own environment. A command-line prefix cannot set that —
verified twice, by the implementer and again by me — despite the hook's message saying to
put it "in the command", which is misleading. The triage the gate asks for is complete and
written above; only the mechanical regen is outstanding. Same gate that blocked #12 and #16.

Run from the MAIN checkout with `CDO_WS` unset — the CDO workspace has drifted, so a regen
with it set would rewrite `tests/l4-summary-baseline/` from drifted data:

```
GOLDEN_TRIAGED=1 CDO_WS= TREE_SITTER_AL_PATH=U:/Git/al-call-hierarchy/tree-sitter-al   bash scripts/check-goldens --regen
```


## Gate results — supervised (step 9), commit `2b6ddb26`

| gate | exit | supervised | notes |
|---|---:|---|---|
| `check-diff --base master --head HEAD` | 0 | — | `reasons: []`, no protected path touched |
| `check-goldens --verify-coverage` | 0 | true | 44 golden dirs declared |
| `check-goldens` | **101** | true | 6 CDO-gated failures, ALL pre-existing (below). Every non-CDO target green |
| `cdo-gate` | **1** | true | same population |
| pre-commit hook (goldens) | 0 | — | passed on its own, no bypass |

### The 6 failures are pre-existing — proven twice, independently

1. I measured this exact set on `master` earlier tonight, BEFORE this branch existed.
2. The implementer reverted `d19.rs` to its HEAD content and reproduced the identical 6.

They are two unrelated problems, neither touched by this change:

- **CDO workspace drift** — goldens minted against DO `64643a2f`, checkout now at
  `bc3ccb18`; the engine reports the divergence itself and asks for a re-mint.
- **`ambiguousResolved = 67` against a pin of 0** — a separate ratchet breach, measured at
  67 both with and without tonight's Query-catalog fix, so also not new. It had been MASKED
  until tonight because the `real_unknown_rate` assertion fires first and aborted the test
  before reaching it.

### Consequence for the flow

`attest` (step 12) refuses on `gates-not-green`, so this issue cannot be attested or merged
while the CDO baseline is red — **and neither can any other non-docs issue**. The blocker is
the shared CDO baseline, not this change. Nothing here is blessed or worked around.


## Outcome: blocked

**Reason code: `wall-clock-cap`.**

The executor's 4-hour wall-clock cap for this run was reached at the post-rebase
re-gate (`charge rebase_regate` -> `{"exhausted": "wall-clock"}`), so no further
supervised gate could run and the issue could not be attested or merged.

Not a cap on the WORK — on the RUN. Most of those four hours went to the CDO
baseline arc that this issue turned out to be blocked behind: the committed CDO
goldens were minted from a tree whose SHA exists in no checkout, which had silently
disabled the semantic audit (it paired ZERO sites and still reported a pass). That
had to be fixed before any non-docs issue could pass `cdo-gate`. It is fixed and
merged (`3ad5b4bd`).

### What IS done, and verified

- The fix itself: `detect_d19` skips `routine.kind == "event-publisher"`, with the
  `eventPublisher` skip statistic counting routines.
- 7 tests in `tests/gap/gap_d19_event_publisher_skip.rs`, driving real assembly and
  the registered detector.
- Discrimination proof recorded both ways (remove the skip -> 4 publisher cases
  fail with real findings; restore -> 7/7 pass).
- Measured corpus deltas: ws-d59 6->0, ws-d38 3->0, ws-d12-dead-event 1->0,
  ws-d19 2->2. 10 false positives removed, 0 true positives lost.
- Goldens triaged and regenerated (one fixture, `ws-d8-commit-in-tx`, whose
  committed golden encoded the false positive verbatim).
- Spec panel converged round 2, 12/12 accepted by both reviewers.
- Gates green pre-rebase: check-diff clean, coverage 0, goldens 0, pre-commit hook
  passed unaided.
- Rebased cleanly onto the new master as `7a822d84`; working tree clean.

### What is NOT done

The post-rebase re-gate, the final panel, the attestation, the PR and the merge.
The branch `issue/25-agent-discovery-l5-d19-unused-p-a1` is ready for a fresh run to
pick up from step 9 — nothing needs redoing, only re-running.

### Carried forward

While resolving the rebase I found 2 literal NUL bytes in CHANGELOG.md, introduced by
my own earlier edit on the baseline branch (a `\\0` in a shell heredoc became a real NUL),
which made git treat the file as binary. Fixed in this rebase; master carries the
fixed copy once this branch merges.


## Final panel — pre-merge review of the actual diff

| reviewer | evidence | verdict |
|---|---|---|
| `gpt-6-astra` | 26 files | **sound for its stated scope; no critical or important defect** |
| `gemini-3.8-flash` | 7 files | **sound, safe, adheres to CLAUDE.md** |

Both were asked specifically whether any test in `gap_d19_event_publisher_skip.rs` can
pass WITHOUT the detector running — the failure mode CLAUDE.md warns about. Both traced
the path independently and concluded it cannot: `run_d19` requires exactly one registered
d19 descriptor, drives `run_detectors`, and requires d19's returned statistics; the
registry only emits statistics from a SUCCESSFUL detector result (`registry.rs:562-600`),
so an error or caught panic yields diagnostics with no stats and fails the lookup; and
`add_skip` only creates a key when the count is non-zero (`registry.rs:169-175`), so a
no-op run scores 0 against assertions demanding 1 or 2.

### Findings, and what was done

Both raised the SAME minor finding, independently: the committed plan still stated the
falsified "ordering is load-bearing" claim as a live requirement, even though `d19.rs` and
this ledger's implementation notes retract it. astra added two more.

1. **minor (both) — plan states a falsified claim as a requirement.** FIXED: the paragraph
   in `docs/superpowers/plans/2026-09-14-issue-25-d19-publisher-skip.md` is struck through
   and annotated SUPERSEDED, pointing at the measurement that falsified it.
2. **minor (astra) — the acceptance matrix retains the falsified zero-golden-movement
   prediction.** FIXED: that row is struck through and marked FALSIFIED, pointing at the
   Gate results section which records what actually moved and why.
3. **minor (astra) — the discrimination proof is SUMMARISED, not evidenced.** Fair, and
   the stricter reading of CLAUDE.md's own rule ("record both outcomes"). The verbatim
   outcomes are therefore recorded below rather than described.

### Discrimination proof — verbatim outcomes

Mutation: delete ONLY the four-line `"event-publisher"` skip in `detect_d19`, after
asserting the text occurred exactly once (`patch applied: removed 1 occurrence`).

**BREAK APPLIED — `cargo test --test gap gap_d19_event_publisher_skip`:**

```
test result: FAILED. 3 passed; 4 failed
```

with the four publisher cases failing on real findings, e.g.

```
an IntegrationEvent publisher's parameters exist for SUBSCRIBERS to use; d19 cannot
see that and must not flag them. findings: [ Finding { ... root_cause:
"OnBeforePost declares parameter 'DocumentNo' (Code[20]) at position 0 but the body
never references it." ... } ]
```

**RESTORED — same command:**

```
test result: ok. 7 passed; 0 failed
```

Both directions observed, not predicted. Note the separate ordering break, run in the same
session, came back GREEN — which is what falsified finding 1's claim, and is recorded as
evidence about the CODE (d19's checks are order-independent) rather than about the test.

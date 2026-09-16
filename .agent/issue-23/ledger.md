# Issue 23 — d50's transaction-managing gate counts temp-inclusive table writes, unlike d8

## Claim

```json
{"run_id":"20260915-202949-a1a165","issue":23,"attempt":1,
 "session":"https://claude.ai/code/session_01Udxt1D5qya52q1HFXUzARp",
 "branch":"issue/23-agent-discovery-l5-d50-s-trans-a1",
 "worktree":"U:\\Git\\al-sem-issue-23-a1",
 "body_hash":"c34e58abb26b7a227fdf9fac3e5fdab33021b0c24d0b849de3ea7e32bac428c0"}
```

Worktree created from `master` @ `715cde09`.

## Classification

**BOUNDED.** The change is understood, local, and its acceptance criteria are already
stated in the issue. The production edit is a single method swap between two accessors
that already exist side by side with identical signatures. What makes it non-trivial is
the FIXTURE, not the code — see Design.

## Assumption probes

All read-only, against the worktree at `715cde09`.

| probe | result | evidence |
|---|---|---|
| P1 d50's gate counts temp-inclusive writes | **holds** | `d50.rs:74` — `ctx.cone_derived.writes_tables_count_of(&summary.routine_id) >= TRANSACTION_THRESHOLD_TABLES` |
| P2 that accessor is temp-INCLUSIVE | **holds** | `cone_derived.rs:424-427` reads `row.table_writes_all`; the module doc at `:23-24` states it is "insert\|modify\|delete on `resource_kind == \"table\"` with a `resource_id`, INCLUDING known-temp" |
| P3 d8 counts PHYSICAL writes | **holds** | `d8.rs:40-42` — `writes_physical_tables_count_of(&summary.routine_id) >= TRANSACTION_THRESHOLD_TABLES` |
| P4 the physical accessor already exists with the same shape | **holds** | `cone_derived.rs:449-452` — same `(&self, routine_id: &str) -> usize` signature, reads `row.physical_table_writes`, documented at `:25-27` as "the same, EXCLUDING `fact_is_known_temp`". The fix is a drop-in |
| P5 `temporary` is modelled and already used in the corpus | **holds** | `Record Customer temporary` in `ws-d3-temp`, `ws-d33`, `ws-d40`, `ws-d52`, `ws-d56` |
| P6 **no committed fixture exercises the count path to a POSITIVE result** | **holds, with round-1 wording correction** | `is_transaction_managing` short-circuits on `posting_name_matches(&r.name)` (`d50.rs:65-67`) BEFORE reaching the count. `ws-d50-pos`'s routine is `PostDocument`, which matches `^(Post\|Apply\|Release)[A-Z]` (`d50.rs:47-57`). So the committed positive fixture is transaction-managing BY NAME. Round 1 (astra) corrected my original wording: d50 evaluates EVERY routine in the span (`d50.rs:199-205`), including the non-posting `RunWorkerChecked`, so the count path DOES execute today — at a zero-table count. What is absent is a threshold-POSITIVE count case. The consequence for the fixture is unchanged: a new fixture reusing a posting-style name would pass before AND after and prove nothing |
| P7 a new `r0-corpus` fixture needs a seed r4 golden AND explicit registration | **holds, and round 1 found the half I had missed** | CLAUDE.md's rule that the regen REWRITES but cannot MINT an r4 golden holds, so a seed (projection shape, `findingCount` 0, empty `findings`) must be committed first. But both reviewers found independently that R4 drives an EXPLICIT named Smoke list (`tests/r4/r4_differential.rs:111-128`, iterated at `:1670-1682`) — verified directly: `fixture: "ws-d50-pos"` at `:115`. A fixture that exists on disk with a seed but no entry is never run, and R4 stays green while testing nothing |
| P7b an EVENT-FREE fixture does NOT need an r2c/l3eg golden | **holds — my original claim was over-broad** | `differential.rs:2135-2142` skips a new fixture whose event graph is empty rather than demanding a golden, and `event_graph.rs:258-283` shows plain writer procedures and an ordinary `OnRun` are neither publishers nor subscribers. The `ir-l2` snapshot still grows |

### A dependency worth recording, not blocking

This change points d50 at `writes_physical_tables_count_of`. Issue **#33** (filed by this
session's #20 run) will CHANGE what that accessor answers: cone representative selection
currently discards a physical fact when a temp fact wins the same `inherited_fact_key`, so
`physical_table_writes` today under-reports for mixed groups. Both changes are
improvements and they do not conflict — #23 aligns d50 to the physical semantics, #33
makes those semantics correct — but d50's finding population will move a SECOND time when
#33 lands, and that movement must not be mistaken then for a regression from this change.
Recorded here so the next triage has it.

## Design

Design v3, after spec-panel round 2. Round 2 accepted 11 of 12 entries and RE-RAISED S-03:
own checked-run callsites remove the shared-HELPER problem but do not exclude a shared
CALLER. Disposition table at the end.

**Production change — one line.** `d50.rs:74`: `writes_tables_count_of` →
`writes_physical_tables_count_of`, with a comment naming d8 as the detector this aligns
with and stating why (a temporary record never dirties the transaction). Both reviewers
confirmed twice that no other production change is needed: manager selection already
filters through this gate (`d50.rs:199-215`) and severity depends on an effective explicit
commit, not the count (`d50.rs:293-304`).

**Two doc corrections ship with it.** `d50.rs:59-60` still documents the gate as
`writesTablesOf(...)`, false after the swap. `affected_tables` REMAINS the temp-inclusive
span footprint (`d50.rs:342`) — not a second gate, deliberately unchanged, and not to be
described as "proven dirty physical tables".

**Scope, precisely.** "Physical" means EXCLUDING known/true temp facts, not "proven to
reach SQL": unknown, parameter-dependent and absent temp states still count. The narrowing
is not limited to temp-only routines — two physical plus one temporary also drops 3 → 2.
The NAME branch is untouched, so a posting-named temp-only routine still qualifies; A6
pins that exception executably.

### The fixture, and the contract that makes it valid

`tests/r0-corpus/ws-d50-temp-gate/` with FOUR writers over three shared table objects
(`T1`, `T2`, `T3`). Per-routine `temporary` declarations decide temp-ness; counts are
per-routine, so sharing table objects across routines cannot bleed
(`cone_derived.rs:413-452`).

| routine | writes | inclusive / physical | d50 before | d50 after |
|---|---|---|---|---|
| `BufferRows` | T1,T2,T3 all `temporary` | 3 / 0 | fires | **no finding** |
| `StageRows` | T1,T2,T3 all physical | 3 / 3 | fires | fires |
| `StageMixedRows` | T1,T2 physical, T3 `temporary` | 3 / 2 | fires | **no finding** |
| `PostBuffers` | T1,T2,T3 all `temporary`, POSTING name | 3 / 0 | fires | fires (name branch) |

Expected population: **four findings before, two after** — retained subjects exactly
`StageRows` and `PostBuffers`.

**THE ISOLATION CONTRACT (S-03, re-raised in round 2).** Each writer holds its OWN
`if Codeunit.Run(Codeunit::"…") then;`. That alone is NOT sufficient. The four writers must
also be mutually independent: **no writer calls another, and no common AL driver calls more
than one.** Round 2's counterexample:

```
RunCases → BufferRows → checked Run
         → StageRows  → checked Run
```

`BufferRows`' backward span contains `RunCases` (`transaction_spans.rs:92-114`) -- that
much is guaranteed by construction, and it is exactly what the membership assertions reject.

CORRECTED in attempt 2 (astra, the NINTH over-claim): an earlier draft of this paragraph
continued that `RunCases` would inherit `StageRows`' three PHYSICAL writes and therefore be
accepted as a manager. That does not follow. `BufferRows` and `StageRows` write the SAME
three tables; the inherited dedup key excludes temp state (`capability_cone.rs:1202-1212`);
and the equal-distance tie-break is the first-hop edge sort key, not a preference for
physical writes (`:1537-1557`). If the `BufferRows` edge wins those ties, the driver inherits
the TEMPORARY representatives and its physical count is 0 -- so it FAILS the manager gate
rather than passing it. That is issue #33 once more.

The isolation contract does not rest on any of that. It rests on the membership assertions,
which reject a contaminated span whatever the driver's own cone turns out to contain. The
manager-qualification story was speculation about engine behaviour three subsystems away, and
it is removed rather than restated -- which is the lesson the ninth over-claim taught, after
the seventh's fix contained the eighth.

So: the four writers are UNCALLED public procedures. Seed discovery scans routines without
requiring root reachability (`transaction_spans.rs:411-424`), so no driver is needed. The
shared empty worker target must not call back into them. **The test asserts each checked
span's routine membership is exactly its own writer** — that is what makes the contract
executable rather than a comment.

**Harness registration.** The fixture is added to R4's named Smoke list
(`tests/r4/r4_differential.rs:111-128`) alongside its seed golden, as ONE combined fixture:
it produces ≥1 finding, so it satisfies the anti-degenerate check at `:1713-1723` without
needing the negatives list.

## Acceptance matrix

| # | Acceptance item | What proves it |
|---|---|---|
| A1 | d50's COUNT branch counts physical writes | A unit test beside d50's native tests. **Not vacuous**: `minimal_ctx` has empty summaries and a default cone store, so an unmodified one returns false at the missing-summary branch (`d50.rs:68-70`) even BEFORE the fix. The test populates a real three-temp-table summary and store and asserts the `3 / 0` precondition first |
| A2 | `BufferRows` (non-posting, 3 temp) produces NO d50 finding after | Assert by the PRIMARY CHECKED-CALLSITE location (`d50.rs:217-235`), never by absence from manager evidence — otherwise a witness change reads as suppression |
| A3 | `StageRows` (non-posting, 3 physical) fires before AND after | Same subject-assertion rule |
| A4 | Discrimination, covering BOTH suppressed routines | Break `d50.rs:74` back to the inclusive accessor, ASSERT the break applied, and require `BufferRows` AND `StageMixedRows` to report again while `StageRows`/`PostBuffers` stay green; restore; re-run. The recorded failure must be the CORPUS assertion, not merely the unit test |
| A5 | The count path reaches a POSITIVE result | Neither non-posting writer's name matches `POSTING_NAME_RE`; each writes three DISTINCT resolved ids |
| A6 | The NAME branch is unchanged | `PostBuffers`, temp-only with its own checked run, fires before AND after |
| A7 | The mixed case is a same-shape control | `StageMixedRows` differs from `StageRows` only in the third declaration being `temporary`. Assert its counts are exactly `3 / 2`, and that it REPORTS before the fix (A4) — an after-only absence would pass even if its Run were unchecked |
| A8 | The fixture is genuinely executed by R4 | An unconditional end-of-test assertion that `ws-d50-temp-gate` appears EXACTLY ONCE in completed ported results. Proof, per round 2: removing the Smoke entry must fail THAT assertion (removing entry + checks together is why "remove it and watch silence" proves nothing), and with registration restored, altering an expected payload in its golden must fail the byte comparison at `r4_differential.rs:1499-1521` |
| A9 | Span isolation holds | Assert each checked span's routine membership is exactly its own writer — the executable form of the isolation contract |

## Measurement plan

- Full `scripts/check-goldens`, `--verify-coverage` first. Expected: the new fixture's r4
  golden and the `ir-l2` snapshot. `ws-d50-pos` and `ws-d50-neg` must be BYTE-UNCHANGED
  (pos is transaction-managing by NAME, neg's run is unchecked); movement in either is an
  unexplained line, not a rebaseline.
- `scripts/ci-steps all`; `scripts/cdo-gate` (not docs-only).
- Real-code delta: d50 is OPT-IN (`presets.rs:70`), so a default run measures nothing. Any
  comparison must EXPLICITLY SELECT d50 and compare full payloads, not counts or
  fingerprints — a narrowing gate can change a retained finding's manager and explanation
  while id, count and fingerprint are identical (`d50.rs:212`, `fingerprint.rs:133-175`).
  Report "unmeasured" honestly if no real workspace is available; both reviewers agree that
  is acceptable for an opt-in detector.
- CHANGELOG under `## [Unreleased]`, scoped: physical-count alignment of the COUNT branch,
  name heuristic unchanged.

## Round 2 disposition

| id | round-2 decision | answered by |
|---|---|---|
| S-03 | **re-raised** | The isolation contract above: writers mutually independent, no common driver, uncalled public procedures, plus A9 asserting span membership |
| R2-A8 | new, important | A8 rewritten: an unconditional "appears exactly once in ported results" assertion, whose removal is what must fail |
| R2-A7 | new, minor | A7 now requires the same-shape control to REPORT before the fix (via A4) and pins `3 / 2` |
| R2-A1 | new, important | A1's vacuity trap named: `minimal_ctx` returns false at the missing-summary branch before the fix |
| R2-SUBJ | new, important | A2/A3 assert by primary checked-callsite location, not manager evidence |
| S-01,02,04..12 | accepted by both | unchanged from design v2 |

## Timeline

| when | phase | outcome |
|---|---|---|
| 2026-09-15T20:29Z | claim | ok, attempt 1, from `master` @ `715cde09` |
| | classify | BOUNDED |
| | probes | 7; P6 and P7 corrected by round 1 |
| | spec panel r1 | remaining 2. Both APPROVE the swap, no critical defect. 12 findings |
| | design v2 | revised against all 12 |
| | spec panel r2 | remaining 1. flash 12/12 accepted; astra 11 accepted, **S-03 re-raised** + 4 new |
| | design v3 | isolation contract, A8 rewritten, A1 vacuity guard, subject-assertion rule |

## Acceptance tests — the RED state (step 6)

Written by an `opus` subagent; every claim below RE-VERIFIED by the conductor running the
tests directly, not taken from the agent's report.

`d50.rs:74` is byte-unchanged — confirmed against the diff: the only `writes_*_count_of`
lines the diff adds are inside the test module (the `assert_counts` helper and A1's own
precondition). The production gate still reads `writes_tables_count_of`.

**`cargo test --test gap gap_issue23 -- --test-threads=1` -> 4 passed, 3 FAILED (exit 101)**

| test | result | why |
|---|---|---|
| `a2_temp_only_writer_produces_no_finding` | **FAILED** | `BufferRows` (3 temp / 0 physical) still reports. All four subjects present: BufferRows, StageRows, StageMixedRows, PostBuffers |
| `a7_mixed_writer_below_physical_threshold_produces_no_finding` | **FAILED** | `StageMixedRows` (3 inclusive / 2 physical) still reports |
| `fixture_population_is_exactly_stage_rows_and_post_buffers` | **FAILED** | left `[BufferRows, PostBuffers, StageMixedRows, StageRows]`, right `[PostBuffers, StageRows]` |
| `a3_physical_writer_still_reports` | ok | control — must stay green through the fix |
| `a5_count_path_is_reached_and_counts_three_distinct_tables` | ok | counts measured 3/0, 3/3, 3/2, 3/0 — exactly the fixture table |
| `a6_posting_named_temp_only_writer_still_reports_via_the_name_branch` | ok | the stated exception, now executable |
| `a9_each_checked_span_holds_exactly_its_own_writer` | ok | **the isolation contract holds** — four `CheckedRunImplicit` seeds, each span's membership exactly its own writer |

**`cargo test -p al-sem --lib d50::tests::a1` -> 1 passed, 2 FAILED (exit 101)**

- `a1_temp_only_writes_are_not_transaction_managing` — FAILED
- `a1_mixed_writes_below_the_physical_threshold_are_not_transaction_managing` — FAILED
- `a1_control_three_physical_writes_are_transaction_managing` — **ok**, which is what proves
  the COUNT branch is live and reachable rather than inert

**The vacuity trap (R2-A1) is closed.** `assert_counts(&ctx, ID, 3, 0)` runs at `d50.rs:1050`
BEFORE the gate assertion, so the test proves the summary exists (past the missing-summary
early return at `:68-70`) and the derived row really carries 3 inclusive / 0 physical. A test
built on an unmodified `minimal_ctx` would have returned false for the wrong reason.

**No test that was predicted to fail passed.** That asymmetry — A2/A7/population/A1 red while
A3/A5/A6/A9 green — is the evidence the tests discriminate rather than merely assert.

### Two states deliberately left red, with reasons

- **R4** fails its byte compare (`findingCount` 4 vs the seed's 0). That is the specified
  pre-regen state: the regeneration REWRITES but cannot MINT, so the seed had to be committed
  as `0`/empty. The fixture IS registered and IS executed — the harness produced real output —
  and `run_smoke_entry` panics on the compare before reaching A8's assertion, so A8 becomes
  observable only once the fix lands and the golden is rebuilt to its 2-finding content.
- **`--test l2_ir`** is red: the new fixture adds 5 routines (4 writers + the worker's
  `OnRun`) to `l2_features.snapshot`. The subagent attempted the rebuild, `golden-regen-guard.sh`
  refused it for want of a triage receipt, and it correctly stopped rather than working around
  the hook. Rebuilding one family alone is the trap CLAUDE.md names, and rebuilding r4 NOW
  would bless the 4-finding bug. Both families are rebuilt together, after the fix.

`--test differential` is GREEN, which confirms probe P7b directly: the event-free fixture
needs no r2c/l3eg golden.

Other gates at this point: `cargo clippy --all-targets --all-features -- -D warnings` exit 0;
`--test gap` whole umbrella 169 passed with only the 3 intended failures; `-p al-sem --lib`
1770 passed with only the 2 intended A1 failures.

### A false positive in this session's own hook, recorded

Appending this very section by heredoc was BLOCKED by `.claude/hooks/golden-regen-guard.sh`,
because the prose above contains the literal regeneration command. The hook scans raw command
TEXT rather than the command actually being run, so quoting the invocation inside a document
trips it. The guard's purpose is sound and it fired correctly for the subagent; this
particular match is a defect in the matcher, filed as a discovery rather than worked around
with the escape hatch.

## Task 1 — the gate swap (GREEN)

`d50.rs:74` now calls `writes_physical_tables_count_of`; the helper doc at `:59-60` corrected;
a CHANGELOG entry under `Fixed`, scoped to the COUNT branch with the NAME heuristic stated as
unchanged. `affected_tables` and `transaction_spans.rs` untouched.

One extra doc correction the conductor made beyond the implementer's brief: `d50.rs:962`, in
the acceptance-test module, asserted in the PRESENT tense that "d50 gates on
`writes_tables_count_of`". True when the tests were written, false the moment the fix landed.
Corrected to "USED TO gate on ... that is the defect these tests pin". The implementer flagged
it and deliberately did not touch it, being outside its stated scope — the right call; leaving
a false statement in the tree was not an option either.

Verified by the conductor, not taken from the report: `--test gap` 172 passed / 0 failed;
`-p al-sem --lib` 1772 passed / 0 failed; clippy `-D warnings` exit 0; `check-diff` reasons `[]`.

### A4 — discrimination proof (verbatim outcome)

Break: `d50.rs:74` reverted to `writes_tables_count_of`, occurrence asserted `== 1` before
writing, broken-file sha256 recorded. Under the break:

- `a2_temp_only_writer_produces_no_finding` FAILED
- `a7_mixed_writer_below_physical_threshold_produces_no_finding` FAILED
- `fixture_population_is_exactly_stage_rows_and_post_buffers` FAILED
  (left `[BufferRows, PostBuffers, StageMixedRows, StageRows]`, right `[PostBuffers, StageRows]`)
- `a1_temp_only_writes_are_not_transaction_managing` FAILED
- `a1_mixed_writes_below_the_physical_threshold_are_not_transaction_managing` FAILED
- `a3`, `a5`, `a6`, `a9` and `a1_control_three_physical_writes_are_transaction_managing` GREEN

Restored by byte-copy (never `git checkout --`); restored sha256 matched the pre-break hash;
re-run green. No test predicted to fail came back green.

## Task 2 — goldens regenerated under a triage receipt (GREEN)

### Sequencing correction to the plan, found by the pre-commit hook

The plan said "commit T1, then regenerate in T2". That is impossible here: the pre-commit hook
blocks any commit touching `src/engine/` unless the golden gate passes, and the goldens are
deliberately red until T2. T1 and T2 therefore land in ONE commit. The plan was wrong about the
order and the hook was right.

### Triage

Dispatched `golden-diff-triager` over the full nine-target gate run. Seven targets green;
exactly two moved, both FIXTURE-SHAPED, **zero unexplained lines**. Receipt written to
`<main checkout>/.agent/golden-triage.md` before regenerating — no `GOLDEN_TRIAGED` escape
hatch used.

Two results from the triage worth more than its verdict:

1. **Monotonicity at the fold.** `cone_derived.rs:626-631` pushes to `s_phys_writes` only
   inside `if !is_temp`, always alongside `s_writes_all`. So physical ⊆ inclusive for every
   routine, the new gate IMPLIES the old one, and its single call site (`d50.rs:212`) is a
   positive filter whose empty result means `continue`. **d50's finding set can only shrink**;
   a `findingCount: 0` golden cannot grow. That bounds the blast radius by argument.
2. **Cross-transport re-derivation.** `ws-d50-pos` was not merely unmodified in git — it was
   re-derived under the new code through `alsem analyze` and reproduced the committed
   fingerprint `2923ea56173f66ed` byte-for-byte. `ws-d50-neg`, which the fail-fast run never
   reached, was closed separately: 0 findings through the same transport, and its
   `Codeunit.Run` is unchecked so it never produces a seed.

A reporter quirk recorded for the next reader: `tests/l2_ir/ir_l2_snapshot.rs:145` prints
`CHANGED` for any rid whose golden entry does not match, and that predicate is also true when
the rid is ABSENT. "5 routine(s) drifted" therefore meant 5 ADDED, not 5 altered.

### Regeneration, verified against the receipt

Ran through the supervisor (`exit_code` 0, `supervised` true), all families together.

| receipt said | measured |
|---|---|
| l2 snapshot +5, 0 changed, 0 removed | 5 added, 0 removed; all 5 lines name the new fixture |
| r4 golden 0 -> 2 findings | 2 |
| subjects `StageRows`, `PostBuffers` | exactly those |
| both `info` | both `info` |
| `affectedTables` keeps all 3 incl. the temporary | 3 each |
| `ws-d50-pos` / `ws-d50-neg` byte-unchanged | unchanged |

Nothing appeared that the triage had not predicted, and nothing predicted failed to appear.
Gate re-run clean afterwards: `exit_code` 0, `supervised` true; `--test r4` 28 passed / 0 failed
(was 27 / 1).

### A8 — discrimination proof, and a first attempt that proved nothing

A8 only became reachable once the golden was correct: `run_smoke_entry` previously panicked on
the byte compare before control reached the assertion.

**First break, DISCARDED.** Renaming the Smoke entry's `fixture` field to
`ws-d50-temp-gate-DISABLED-FOR-PROOF` made the harness fail at `r4_differential.rs:1497` with
`missing R4 golden: ...-DISABLED-FOR-PROOF.r4.golden.json`. That is an artifact of the break
itself — a golden lookup keyed on the renamed fixture — not evidence about A8. A failure for
the wrong reason proves exactly as little as a green break.

**Second break, the real one.** Deleted the whole `Smoke { fixture: "ws-d50-temp-gate", ... }`
entry, occurrence asserted `== 1`. A8's own assertion fired at `r4_differential.rs:1803`:

```
ws-d50-temp-gate must appear EXACTLY ONCE in the completed ported results; found 0
```

Restored by byte-copy; restored sha256 `b46fad1b27ce10b5576f` matched the pre-break hash;
`--test r4` green at 28 passed.

## Gate results (step 9)

All four through the supervisor from the worktree, each reporting its own
`exit_code`/`supervised` from the supervised process rather than a shell status.

| gate | exit_code | supervised | log |
|---|---|---|---|
| `ci-steps-all` | 0 | true | `.agent/runs/20260915-202949-a1a165/logs/ci-steps-all.log` |
| `check-goldens-coverage` | 0 | true | `...\/check-goldens-coverage.log` (44 golden dirs declared) |
| `check-goldens` | 0 | true | `...\/check-goldens.log` (all nine targets) |
| `cdo-gate` | 0 | true | `...\/cdo-gate.log` — `cdo-gate: PASS` |

`cdo-gate` ran `cargo test -p al-sem --lib` (the log carries "running the library test suite
(--lib)"). That matters here specifically: the `--lib` step was added to `cdo-gate` earlier on
2026-09-15 after it was found the gate had silently admitted two regressions in one day
because it never ran the tests they lived in. d50's three A1 tests are `--lib` tests, so this
is the first issue whose unit tests the gate actually executed rather than appeared to.

Toolchain: cargo 1.96.0. Grammar: tree-sitter-al v4.4.0 (submodule pin at the repo tip).
`CDO_WS` identity (sha256 of the path, first 16 -- the flow records an identity, never the
path itself, because this file is committed and posted verbatim as a PR body): `1bcef7e9970b254c`.
It is the pinned detached DO worktree at `bc3ccb18`.

`git status --porcelain -- . ':!.agent'` empty after the gates — they changed nothing.

## Real-code d50 delta (closing register entry S-09)

The measurement plan permitted reporting this "unmeasured" IF no real workspace was
available. One was, so it was measured rather than exempted.

Method: `alsem analyze --detector d50-checked-run-implicit-commit --with-evidence
--deterministic --format json <CDO_WS>`, run from two `release-fast` binaries built from the
SAME source tree at two commits — baseline from `master` @ `715cde09`, patched from the
worktree @ `b3d28784` — against the same pinned workspace.

`--detector` is load-bearing: d50 is OPT-IN (`presets.rs:70`), so a default run executes it in
NEITHER binary and the comparison would be two empty sets agreeing. `--with-evidence` was used to
widen the payload, but the round-1 final panel (astra) corrected the reason given here:
`evidencePath` is NOT the only place a manager change surfaces. `d50.rs:320` embeds
`manager.name` in `rootCause` and `d50.rs:333` embeds `manager.object_id` in
`affectedObjects`, so a manager swap moves those fields too. FULL-PAYLOAD comparison is
therefore the right method — which is what was done — but the justification originally
written here was wrong.

RESULT — the two outputs are BYTE-IDENTICAL (sha256 `41c159fb4c425f87…`, 13,325 bytes each).

| | |
|---|---|
| baseline findings | 3 |
| patched findings | 3 |
| removed | 0 |
| added | 0 |
| retained with changed payload (the S-09 case) | 0 |

The three findings: `RunExportEDocCodeunit` (medium), `SendQueue` (medium), `RunFromWizard`
(info) — all retained, all byte-identical.

WHAT THIS DOES AND DOES NOT SHOW. The precise claim it supports is narrow: **no emitted d50
finding or payload changed on this pinned workspace.**

It does NOT show the shape is absent from CDO, and an earlier draft of this ledger said it
did. The round-1 final panel (astra) supplied the counterexample, and it follows directly
from `d50.rs:208-221`: a mixed writer can own the checked Run while a POSTING-named ancestor
sorts ahead of it as `managers[0]`. The ancestor qualifies by NAME before and after, so it
stays the selected manager and the emitted payload is byte-identical — even though the mixed
writer stopped qualifying. Unchanged output therefore cannot distinguish "the shape is
absent" from "the shape is present but not the thing being emitted". (The round-1 panel
split here: flash explicitly endorsed the absence claim as honest; astra refuted it with the
mechanism above. The mechanism wins.)

It also does NOT show the change is unnecessary — the fixture proves the defect is real and
reachable — and it does NOT show the three survivors are true positives, which would require
a triage wave the doctrine reserves for a new detector shipping DEFAULT. d50 is opt-in.

An ADDED finding would have falsified the triage's monotonicity argument outright; none
appeared, which is consistent with it.

### Three measurement errors on the way, recorded because they are the instructive part

1. **Wrong CLI shape.** The first attempt passed `--project <ws>`; `analyze` takes the
   workspace POSITIONALLY. Both binaries died on a clap error and wrote zero bytes.
2. **A wrapper's exit code read as the command's.** That failed run was reported by the
   background task as "exit code 0" — the status of the whole shell line, which ended in
   `echo` and therefore always succeeded. The conductor stated "baseline captured" on that
   basis and had to correct it. The inverse then happened: the corrected run reported "failed,
   exit 1" because the line ended in `[ $rc -ne 0 ] && …`, which returns false when there is
   nothing to report — while `alsem` itself had exited 0. This is the same defect class as the
   `| tail` rule CLAUDE.md already names; only the explicitly captured `rc=$?` value was
   trustworthy. The GATE results above are unaffected: those come from `agentflow run`'s own
   JSON, not a shell status.
3. **`d['findings']` instead of `d['payload']['findings']`.** The comparison script read the
   wrong nesting level and reported 0 findings for both binaries. This is a REPEAT of a
   mistake already made earlier in this same session on a different probe. It was caught only
   because the script carried an explicit vacuity guard — "BOTH ZERO -- the comparison is
   VACUOUS ... cannot distinguish 'no movement' from 'detector never fired'" — instead of
   printing a clean, entirely fictional "no movement". A lenient parser would have written a
   false zero into this ledger.

### A8, second half — the payload-mutation proof (was missing; raised as F1 by the final panel)

The acceptance matrix required TWO experiments for A8 and only the first was recorded. The
second, run now:

Break: `used in StageRows performs an implicit commit` -> `used in StageRowsMUTATED ...` in
`tests/r4-goldens/ws-d50-temp-gate.r4.golden.json`, occurrence asserted `== 1` before writing,
no regeneration. Result — `--test r4` exit 101:

```
panicked at tests\r4\r4_differential.rs:1524:9:
  R4 ACCEPTANCE GATE: ws-d50-temp-gate (R4-H) did NOT byte-match its golden
test result: FAILED. 27 passed; 1 failed
```

Restored by byte-copy; restored sha256 `532d4fac204a8cd404dc` matched the pre-break hash;
`--test r4` green at 28 passed, tree back to the committed state.

Why both halves were needed: the first proof (removing the Smoke entry) shows the fixture is
EXECUTED; this one shows its payload is CHECKED. Either alone would admit the other failure —
a registered fixture whose comparator had stopped comparing, or a live comparator on a fixture
nothing runs.

## Final panel (step 10)

| round | astra | flash | charged |
|---|---|---|---|
| 1 | approve the production change; 4 MINOR findings, all in the RECORD | 0 findings, "sound, ready to attest and merge" | `final_rounds` 1 |
| 2 | accept all 25; ONE new finding (FP-05, the sixth over-claim) | accept all 25; **concedes FP-02** | `final_rounds` 2 |
| 3 | accept FP-05 and the other 25; ONE new finding (FP-06, the seventh) | accept all 26; found no seventh | `final_rounds` 3 |

The round-1 split on FP-02 is worth recording as a disagreement resolved on evidence rather
than by seniority. Flash called the claim "CDO's posting code does not contain the shape"
honest and scoped. Astra refuted it with a mechanism (`d50.rs:208-221`): a mixed writer can
stop qualifying while a POSTING-named ancestor remains `managers[0]`, so the payload is
byte-identical either way. The mechanism was taken over the assertion, the ledger claim was
narrowed, and in round 2 flash traced the mechanism itself and accepted the correction.

## Outcome: BLOCKED — `final-rounds`

`attest` refused, and its verdict is the record:

```json
{"error": "register-not-converged", "entries": ["FP-06"]}
```

`charge final_rounds` returns `{"exhausted": "final_rounds"}`. FP-06 is `fixed` in
`769a9b64` — using astra's own suggested wording — but cannot be marked `accepted` by both
reviewers without a fourth panel round, and there is no round left to run.

**What is NOT wrong.** Both reviewers approved the production change at every round. All four
gates are green and supervised. 26 of 27 register entries are accepted by both. The single
unconverged entry is a MINOR documentation finding whose fix is already committed and whose
correcting text the raising reviewer supplied. The code has not changed since `b3d28784`.

**Why blocking is still right.** The rule is that a change ships on a converged register, and
this register is not converged. Merging anyway would mean overriding a reviewer's open finding
because it seemed small — which is the judgement the cap exists to take out of the
conductor's hands. An arc that found seven false statements in its own prose is the worst
possible candidate for "this last one is probably fine".

**What a human or attempt 2 needs to do:** adjudicate FP-06. One entry, already fixed. The
worktree is preserved at `769a9b64`.

## Discoveries

(filed by `/orchestrate` step 10 in the shape below)

## Not delivered

- The register did not converge, so no PR was created and nothing was merged.
- `/issue` step 10 specifies "run the `code-review` skill at high". No such skill exists in
  this checkout (`.claude/skills/` holds arc-capstone, byte-census, discrimination-proof,
  panel-review, perf-probe, rust-coding-skill, scripted-edit, skill-creator, triage-findings),
  and `/code-review` proper is a user-triggered billed built-in the flow cannot launch. The
  `code-reviewer` AGENT was substituted and produced CR-01..CR-05. Recorded as a substitution,
  not as the specified step.
- The real-code delta measured d50 only. It does not establish that the three surviving CDO
  findings are true positives; that needs a triage wave, which doctrine reserves for a new
  detector shipping DEFAULT, and d50 is opt-in.

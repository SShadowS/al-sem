# Issue #18 — d50's medium→info demotion path is never exercised by the corpus

## Claim

```json
{"run_id":"20260920-161401-ed7a5a","issue":18,"attempt":1,
 "session":"https://claude.ai/code/session_01Udxt1D5qya52q1HFXUzARp",
 "branch":"issue/18-agent-discovery-l5-d50-s-medium-a1",
 "worktree":"U:\\Git\\al-sem-issue-18-a1",
 "body_hash":"dc0941ecdac43e4d7bcd0f7080cfee500383ed19addb6f7fbd1352b76bf87254"}
```

Worktree from `master` @ `73623c10`. Reviewers: pi's only provider has returned `429 quota
exceeded` for every model on both days of this session, so the OPERATOR-DIRECTED stand-ins run
both panels — fable in astra's slot, opus in flash's — recorded in the attestation via
`attest --substitute`.

## Classification

**BOUNDED.** One new fixture, one seeded golden, one registration line, and a discrimination
proof. No production behaviour changes — this issue exists to make an existing behaviour
FALSIFIABLE, which is the repo's own testing rule applied to a guard that currently has none
at the corpus level.

## Assumption probes

Full report: `.agent/runs/20260920-161401-ed7a5a/probe-18.md`.

| probe | result | evidence |
|---|---|---|
| P1 — the premise still holds after #12 merged `onrun-codeunit` into the list | **HOLDS** | The break was re-run in the opposite direction (the kind is present now, so the break is a REMOVAL): delete `"onrun-codeunit"` from `D50_UNTRUSTED_ROOT_KINDS` (`d50.rs:80`), assert it landed (`grep -c` 1 → 0), run `--test r4`: **30/30 green, zero goldens moved**. The corpus cannot tell the two versions apart. The same break DOES fail two hand-built `--lib` oracles — so the guard is pinned only by synthetic `DetectorContext` unit tests, which is exactly the "test pins the function, not the use" shape |
| P2 — what produces a d50 finding at MEDIUM | **four things must line up** | (a) a `CheckedRunImplicit` seed — literally `if Codeunit.Run(Codeunit::"X") then;`; (b) a span built by walking CALLERS backward, where a caller that itself commits is visited but NOT expanded (the seed is exempt); (c) a transaction-managing routine in the span — either the `^(Post\|Apply\|Release)[A-Z]` name rule or ≥ `TRANSACTION_THRESHOLD_TABLES` (3) distinct NON-temporary tables in its forward cone; (d) a committer in the span with a literal `Commit()` in its OWN body that passes all four caps |
| P3 — which cap bites, and how to flip it | **cap 2, and `local` is the lever** | `public-procedure` is untrusted and `root_classification.rs:281` gives it to ANY `procedure` with no access modifier — so a plain `procedure` committer is always capped. A `local procedure` gets NO root classification and passes. Confirmed against `tests/r4f-goldens/ws-d50-pos.rootclass.golden.json`: 2 classifications for a 3-routine file, the local one absent |
| P4 — is the finding id stable across the flip | **YES, and so is the fingerprint** | `id = format!("d50/{}", span.commit_operation_id)` (`d50.rs:373`), and `commit_operation_id` is the checked call site's id. `fingerprint_of` hashes detector + object + routine + tables + root-cause key — **severity is not a part**. So a test can assert same id, same fingerprint, unchanged count, `medium` → `info` |
| P5 — which golden families a new corpus fixture moves | **r4 (seeded) + `l2_features.snapshot`; and r2c must stay empty-event** | Direct precedent measured, not assumed: `ws-d50-temp-gate` (`df42d1e4`) moved exactly `tests/r4-goldens/<fix>.r4.golden.json`, `tests/ir-l2-goldens/l2_features.snapshot`, plus its registration. Every r0/r1a/r2a/r2b/r2d/r3a1/r3a2/r3a3/r4f family is driven by an explicit `manifest.json` fixture list and `ws-d50-temp-gate` appears in none. The r4 golden **must be SEEDED first** — `run_smoke_entry` asserts the file exists BEFORE the regen, and the regen rewrites but cannot mint |

## Design

One new fixture directory, `tests/r0-corpus/ws-d50-medium`, carrying **three mutually
independent codeunits** (the isolation contract `ws-d50-temp-gate` already established: no writer
calls another, no shared driver), so one fixture yields all three cases and they cannot interfere:

| codeunit | committer | committer's root kind | expected |
|---|---|---|---|
| A | `local procedure` — the seed itself | *(none)* | **medium** |
| B | a Codeunit `OnRun` trigger | `onrun-codeunit` (untrusted) | **info** <- the demotion, exercised at last |
| C | B's shape PLUS a second, uncapped committer | mixed | **medium** <- the control |

C is what makes B's `info` mean something: the cap is about whether ANY qualifying committer in
the span is uncapped, so a second uncapped one must keep the finding at `medium`. Without C, a
bug that capped everything unconditionally would still produce B's expected `info`.

**No production code changes.** `D50_UNTRUSTED_ROOT_KINDS` stays exactly as it is; this issue
adds the coverage that makes editing it falsifiable.

### Acceptance matrix

| # | item | proof |
|---|---|---|
| A1 | a fixture produces a d50 finding at `medium` | codeunit A's finding in the r4 golden, severity `medium` |
| A2 | the SAME finding moves to `info` when its root kind is untrusted | codeunit B: same shape, committer is an `OnRun`; severity `info`, `findingCount` unchanged |
| A3 | a control with a second qualifying committer stays `medium` | codeunit C |
| A4 | **the demotion is now falsifiable** — the issue's actual point | remove `"onrun-codeunit"` from `D50_UNTRUSTED_ROOT_KINDS` and B's severity flips `info` → `medium`, so `--test r4` FAILS. That break is a no-op on master today (P1), which is the whole defect |
| A5 | no EXISTING golden line moves | `check-goldens` per-line diff; new lines belong to the new fixture only |
| A6 | the second half of the issue — "revisit whether `onrun-codeunit` belongs in the list" | decided ON the new evidence, in the ledger, rather than left open |

## Spec panel round 1 — two criticals, both MEASURED, and the fix was reproducing the defect

Reviews: `.agent/runs/20260920-161401-ed7a5a/fable-spec-18.md` (astra slot),
`opus-spec-18.md` (flash slot). Both reviewers BUILT the candidate fixtures and ran them rather
than reasoning about them, and each found a critical the design would have shipped.

**K1 (opus) — the proposed fixture reproduced the very defect this issue exists to fix.** Delete
the `Commit()` from B's `OnRun` and B *still* emits `info`, same finding, count unchanged. So B's
golden line would have been byte-indistinguishable from "there is no escalation witness here at
all" — which is exactly what makes `ws-d50-pos` worthless as a guard today. The control did not
cover it (with C's commit deleted, C stays `medium` on its helper alone), and A was no
differential either: A and B differed in four ways at once.

**K2 (fable) — the control had a vacuity mode that yields ZERO findings.** `backward_cone` visits
a committing caller but never expands it, so a CHAIN (`OnRun` commits → `Helper` commits → seed)
truncates at `Helper`, the `OnRun` never enters the span, and the seed is skipped silently.
Measured: count 0. The r4 anti-degenerate check is ≥1 per FIXTURE, so A alone would have
satisfied it while B or C vanished.

**K3 (fable) — a process finding worth more than the fixture.** The worktree's BUILD ARTIFACTS
were poisoned by the probe's break: `d50.rs` was restored at 16:22:39, but `aldump.exe` had been
built at 16:21:11, inside the break window. `git status` said clean; every manual `alsem`/`aldump`
run measured the broken list. fable lost a loop chasing a defect that did not exist and caught it
only by rebuilding. Protocol for this issue, and worth generalising: **a break experiment ends
with a rebuild, so the artifacts are restored too, not only the source** — and any manual
measurement records the binary mtime against the source mtime.

### Design v2

- **A and B are one-token twins.** Identical chain, identical bodies; the ONLY delta is the
  committer's declaration — `local procedure RunAll()` in A versus `trigger OnRun()` in B.
  Measured by opus: A = `medium`, B = `info`. A cap break flips B to `medium`; a span/commit break
  flips A to `info`. Neither can rot behind the other.
- **C's two committers are INDEPENDENT callers of the seed**, never a chain — the chain shape
  measures zero findings.
- **`findingCount` is pinned explicitly at 3**, and each case is asserted by id + severity +
  fingerprint, not by presence.
- **A gap test pins the precondition directly** (opus's belt, precedent
  `tests/gap/gap_issue23_d50_physical_write_gate.rs`): B's `OnRun` id IS in the seed span's
  `routines_in_span` and IS the `SeedKind::ExplicitCommit` `commit_routine_id`. A severity value
  alone can never distinguish "capped" from "nothing happened"; this does.
- **A6 is now a decision, not a promise.** With the twins in place the question is settled by
  d50's own argument: an `OnRun` is reachable from arbitrary code via `Codeunit.Run`, AL has no
  nested transactions, so its `Commit()` may be committing a caller's uncommitted work and is not
  provably at the top of its own transaction. `onrun-codeunit` STAYS in the list; what changes is
  that the answer is now falsifiable.

### Design v3 (round 2 corrected two of my own v2 statements)

Both re-raises were measured, and both were right:

- **The gap test is REQUIRED, not a belt (K1, opus).** With B's `Commit()` removed
  line-count-preservingly, B's finding is BYTE-IDENTICAL: same id, same fingerprint, still `info`.
  So the twins do NOT close K1 on their own — a severity value cannot distinguish "capped" from
  "nothing happened", and calling the gap test a belt licensed dropping the only thing that can.
  The twins stay for the OTHER direction: a span/commit break flips A from `medium` to `info`,
  which only the twin makes observable.
- **C's constraint as I wrote it WAS the vacuity mode (K2, opus).** "Independent callers of the
  SEED" measures `count = 0`. The correct constraint: C's two committers are independent callers
  of a shared TRANSACTION-MANAGING routine that calls the seed (or the seed is itself
  transaction-managing).
- **A6's reason was the wrong one (K5, fable).** I argued the `OnRun`'s `Commit()` is MORE
  consequential; cap 2 does not test consequence. It asks whether the committer is provably at the
  top of its OWN transaction, and an entry point reachable from arbitrary callers is not — the
  same reason `public-procedure` is listed. `onrun-codeunit` STAYS, on that ground.

## Implementation, gates, and the final panel

Implemented against design v3 (`impl-18.md`): the `ws-d50-medium` fixture (A/B one-line twins,
C the mixed control), its seeded r4 golden, the `Smoke` registration, and the REQUIRED gap test.
Proofs D1-D4 all discriminate, each asserted-applied, byte-restored under sha256 and followed by
a rebuild. **D2 is the one that matters**: commenting out B's `Commit()` leaves `--test r4` at
EXIT=0 with the golden byte-identical while `--test gap` goes red — the r4 golden genuinely
cannot tell "capped" from "no witness at all", which is why the gap test is required rather than
belt-and-braces.

The implementer also hit K3's hazard in a NEW form and caught it by hash: its restore helper used
`git checkout --`, which **cannot restore an untracked file**, so two fixture breaks were silently
stacked and the "final green" read red. The sha256 comparison is the only reason it surfaced.
Generalisable, and now recorded twice over: **for a new file, `git checkout --` is not a restore —
verify by hash, never by `git status`.**

### Final panel

An independent code review plus both stand-ins. Between them they found the same defect TWICE
MORE, each time one level up from the last:

| # | finding | closed by |
|---|---|---|
| G1 (code review) | C's role as the mixed-committer control was ITSELF unfalsifiable — drop C's `OnRun` `Commit()` and C stays `medium` on its helper, golden byte-identical, both gap tests green | C's span asserted as an exact set, and BOTH of C's committers asserted to seed an `ExplicitCommit` span |
| G4 (BOTH stand-ins, independently) | nothing pinned that `onrun-codeunit` is the kind that caps B — `trigger OnRun()` → `procedure OnRun()` re-caps it under `public-procedure`, which is also on the list, and the issue's own break becomes a no-op AGAIN | the severity test asserts B's classification is EXACTLY `["onrun-codeunit"]`; conductor-measured: that edit fails `--test gap` while `--test r4` stays 30/30 byte-identical |
| G2/G5 | doc claims broader than measured, plus citations — one of which was REJECTED after checking, because the proposed range would have swallowed a comment line and dropped a brace | corrected; the rejection is named with its reason in the CHANGELOG |
| G3 | the tests pin "capped", not "cap 2 capped" | stated limit with its wake condition in the gap test header; both stand-ins checked the exclusion argument and agreed `minor`/`deferred` is honest |

**Register: 10 entries, every one accepted by both stand-ins, none open, no blocking entry
deferred.** Both reviewers then tried to break the fixture again and could not: every link in the
chain the demotion depends on is pinned by object-qualified id, so the only remaining way B can
be `info` is cap 2 itself — which is G3's stated limit, a code regression rather than a fixture
edit.

### Gate results (supervised, on the final tree)

| gate | exit |
|---|---|
| `ci-steps all` | **0** |
| `check-goldens --verify-coverage` | **0** |
| `check-goldens` | **0** |
| `cdo-gate` | **0** |

Two earlier cycles were lost to the `program_resolve_harness.exe` link race (a build in the same
worktree while a gate ran) and are recorded rather than hidden; the cycle above is clean, and the
only delta between it and the merged tree is one CHANGELOG paragraph, which no gate reads.

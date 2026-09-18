# Issue 21 — the allowed-companion op list is argument-blind

## Claim

```json
{"run_id":"20260918-182957-88c4f2","issue":21,"attempt":1,
 "branch":"issue/21-l5-allowed-companion-op-list-d5-d60-a1",
 "worktree":"U:\\Git\\al-sem-issue-21-a1",
 "body_hash":"3d3912ec2188399a621ac8047bd030f768a9de974db0855a5cc7bafbf6cdfd34"}
```

Worktree from `master` @ `51fa4f6e`.

## Classification

**BOUNDED, but CROSS-LAYER — and the issue text does not say so.** One half is an L5 detector
change; the other cannot be done at L5 at all, because the data it needs is not captured at L2.
Closing that needs an L2 change — by CHOICE the serialized `FIELD_ARGS_OPS` route rather than a
serde-skipped internal channel, for the reason design v2 §4 gives. P5/P6 establish the gap; the
route is a decision, not a necessity.

## Assumption probes

Everything below was MEASURED — a purpose-built fixture run through the real `alsem` binary, and
source reads for the mechanism. Nothing here is inferred from the issue text.

### The fixture and its controls

Four routines in a plain codeunit and the same four in an `Upgrade` codeunit: a CONTROL (the shape
both detectors are SUPPOSED to flag) plus the issue's three unsound shapes. The controls exist
because a zero-finding result from a broken harness reads exactly like a clean one — the trap that
nearly produced a false negative on issue #32 earlier in this session.

| probe | result | evidence |
|---|---|---|
| P1 d5 reports all three unsound shapes | **holds** | `alsem analyze --detector d5-set-based-opportunity,d60-…`: d5 fires on `MultiStepAdvance`, `InLoopFilterChange`, `InLoopKeyChange` AND on the control. 4/4 |
| P2 d60 reports them too | **holds, but only in an Upgrade codeunit** | d60's join requires `object_subtype ∈ {Upgrade, Install}` (`d60.rs:6-7`). My first fixture was a plain codeunit, so d60 reported NOTHING — including on the control. Adding an `Upgrade` codeunit made d60 fire on all four. **The first run looked like "d60 is unaffected" and was a fixture defect** |
| P3 the two detectors share the list the issue describes | **FALSE on master** | d5 has `ALLOWED_OTHER_OPS` (`d5.rs:23-30`). d60 has no such list — only `CURSOR_OPS` (`d60.rs:30`), used for a different purpose (identifying vars with a live cursor, `:103`). The list the issue calls "shared" is the one #16 would ADD, and **#16 is not merged** (`git log master` finds no `(#16)`) |
| P4 the defect is nonetheless real in both | **holds** | Same measurement as P1/P2. d60 reaches the same wrong answer by a different route: its body check rejects per-row calls, other-var ops and if/case, and an in-loop `SetRange`/`SetCurrentKey` on the SAME var passes all three |
| P5 **`Next`'s argument is not available at L5** | **holds — and it resizes the issue** | `FIELD_ARGS_OPS` (`record_op.rs:39-54`) lists SetRange, SetFilter, SetLoadFields, AddLoadFields, SetCurrentKey, Validate, Get, Find, FindFirst, FindLast, FindSet, CalcFields, CalcSums, TestField. **`Next` is not in it**, so `record_op_field_args` (`ir_walk.rs:369-371`) returns `(None, None)` for every `Next`. Confirmed by dumping `aldump --l2` over the fixture: querying `fieldArgumentInfos` on all 8 `Next` ops returns nothing, including the two written `Next(2)`. The field is OMITTED when `None` (`features.rs:237` `skip_serializing_if`), so this is an absent key rather than a serialized null — the source fact above is what the conclusion rests on |
| P6 the OTHER two shapes ARE decidable at L5 today | **holds** | The same dump shows `SetRange` and `SetCurrentKey` carrying populated `fieldArgumentInfos` — e.g. `SetRange(Amt, R.Amt)` yields `[{"kind":"identifier","text":"Amt",…}, …]`, so a literal second argument is distinguishable from a row-dependent one. `loop_stack` (`l3_workspace.rs:262`) separates in-loop from pre-loop |
| P7 **a FOURTH unsound shape exists that the issue does not list** | **holds** | Raised by flash in round 1 and then MEASURED: an extra `R.Next()` in the loop BODY advances twice per iteration, so the loop skips every other row -- and d5 reports it. `d5.rs:108-114` allows ANY op named `Next` among `ops_in_loop`, making no distinction between the loop terminator and a body advance. Added to the fixture as `BodyNextSkips`; d5 fires on it |

### What P5 means

The issue's Acceptance opens with *"Next is treated as an ordinary advance only when its argument is
absent or the literal 1"*. **That is not implementable in d5 or d60 as they stand**, because the
argument never reaches them. Closing it means capturing the argument at L2. The route CHOSEN is
adding `"Next"` to `FIELD_ARGS_OPS`, which moves the byte-stable, golden-backed L2 contract
(`tests/ir-l2-goldens/`) that CLAUDE.md warns can move several families at once. A serde-skipped
internal channel (precedent: `run_trigger`) would avoid that movement; design v2 §4 says why it is
the worse trade here. Either way the L5-only reading of this issue is not available.

So the work splits:

- **Half A — in-loop set/order changes (shapes 2 and 3).** Pure L5. The data is already there.
- **Half B — `Next(n)` (shape 1).** L2 contract change first, then L5 consumes it.

Both are required by the Acceptance, so neither can be dropped without returning
`blocked acceptance-unmet`. Recording the split up front because it determines the diff's shape,
the golden blast radius, and the review the panel should give it — not as an excuse to narrow scope.

### One thing the dump cannot tell me, stated rather than assumed

`in_until_condition` (`l3_workspace.rs:267-271`) is serde-skipped entirely (`features.rs:271`), so a
query for it in the `--l2` dump returns nothing. That is an absent key, not a serialized null, and
either way it is NOT evidence the flag is unavailable to detectors — it is
in-memory data the L3 projection deliberately drops from serialized output. (This is exactly the
lossy-dump hazard issue #15 is about.) Whether d5/d60 can use it to tell the loop's terminating
`Next` from a body `Next` must be checked in code, not in the dump.

## Design (v2 — after round 1)

Both reviewers independently rejected the same two things, and astra found a third that v1 would
have caused. Every claim verified against the code before rewriting.

| round-1 claim | verified at | v1 was |
|---|---|---|
| textual equality of `field_arguments` does not prove filter redundancy | both reviewers, with counterexamples (`SetRange(Code, FilterVal)` where `FilterVal` is reassigned in-loop; `SetRange(Amt,0); SetRange(Amt,1); FindSet()`) | **unsound** — v1's A6 compared argument TEXT and called it redundancy |
| **deleting `ALLOWED_OTHER_OPS` removes a safety boundary** | `d5.rs:108-114` — `all_allowed` requires EVERY non-Modify op to be on the list, so `Delete`, `Insert`, `Validate`, `Get` disqualify TODAY | **a regression v1 would have introduced** — a `whole_set_break` returning `None` for anything unmentioned would newly ACCEPT them, adding false positives while removing others |
| terminator vs body `Next` must be distinguished, and the helper already exists | `detectors/mod.rs:827-829` `is_terminator_next`, used by `d1.rs:839/1024/1304`, `d2.rs:109`, `d1_graph.rs:150` | **incomplete** — v1 had no such distinction and asked the panel whether one was needed |
| d60 does not inspect ops on the DRIVER var at all | `d60.rs:145-151` — the guard is `record_variable_name.to_lowercase() != var_lc`, i.e. only OTHER vars | **wrong** — v1 said "replace the list in both"; d60 has nothing to replace and needs a new pass |
| my shape-2 fixture does not semantically narrow the set | `Probe.al` — `SetRange(Amt, R.Amt)` runs BEFORE `R.Amt := 1`, so `R.Amt` is still 0 and the setter REAPPLIES the existing filter | **weak evidence** — it covers the row-dependent SYNTAX but is not a witness that traversal changed |
| the L2 contract change is a choice, not a necessity | `features.rs` — `run_trigger` is serde-skipped and still forwarded to L3, a direct precedent for an internal channel | **overstated** — v1 said it "must" move the serialized contract |

### v2

**1. The allow-list STAYS; a traversal veto is added on top.** This is astra's option 1 and it is
the smaller, safer diff. Eligibility is unchanged — an op not on `ALLOWED_OTHER_OPS` still
disqualifies, so `Delete`/`Insert`/`Validate`/`Get` keep being rejected exactly as today. The new
predicate only ever REMOVES findings:

```rust
/// Why an in-loop companion op defeats a whole-set claim. `None` = no veto.
/// NOT an eligibility test -- the caller's existing allow-list still applies.
pub enum WholeSetBreak { SkipsRows, ChangesSet, ChangesOrder }

pub(crate) fn whole_set_break(op: &L3RecordOperation, candidate_loop: &str)
    -> Option<WholeSetBreak>;
```

**2. In-loop `SetRange`/`SetFilter` → `ChangesSet`; `SetCurrentKey` → `ChangesOrder`.
Unconditionally.** A6's carve-out is DROPPED. Both reviewers showed textual equality proves
neither equal evaluated values nor the effective filter at loop entry, and astra additionally
showed d5's "associated retrieval" lookup (`d5.rs:117-125`) has no before-the-loop check, so it
cannot serve as the safety evidence a carve-out would need. Dropping it costs only a false
negative and is stated as a deliberate conservative choice — not as proof that every repeated
filter changes the set.

**3. Advance discipline — one eligible advance, in the candidate loop's own terminator.**
`is_terminator_next` (`detectors/mod.rs:827-829`) is REUSED UNCHANGED. It is not strengthened,
because d1/d2 ask a different question of it — whether an op is loop control — and widening it
would move their behaviour. The rule:

- exactly one eligible advance on the driver, and `is_terminator_next(op)` is true for it;
- its `loop_stack.last()` equals the CANDIDATE loop's id (a nested loop's terminator is not its
  parent's);
- any other `Next` on the driver inside the loop → `SkipsRows`.

This is what closes P7's body-`Next` shape, which no argument predicate can catch because both
calls are unit-step.

**4. `Next(n)` — the L2 half.** Add `"Next"` to `FIELD_ARGS_OPS` (`record_op.rs:39-54`). Three
distinct states, which the acceptance must separate:

- `Some([])` — captured, genuinely no arguments → eligible;
- `Some([{kind:"integer", value:"1"}])` → eligible;
- anything else, INCLUDING `None` → not eligible. `None` means the information is unavailable and
  must never be read as "no arguments".

`kind` is `"integer"` and `value` is the RAW text (`ir_walk.rs:399-402`), so `Next(+1)` and
`Next(-1)` lower to a unary expression rather than `Literal::Int` and are rejected, as are
`Next(0)`, `Next(Step)` and any expression.

The serialized contract moves rather than using a serde-skip channel, deliberately: an
arity-only internal channel cannot tell `Next(1)` from `Next(2)` from `Next(N)`, so it would need
literal classification anyway — at which point it is a second, private copy of a representation
L2 already has. CLAUDE.md permits intentional Rust-owned contract changes; the cost is golden
movement, which is measured and triaged rather than blessed.

**5. d60 gets a NEW body pass, not a substitution.** It currently inspects calls, other-record ops
and if/case, and nothing at all about ops on the driver var (`d60.rs:145-151`). It gains a pass
over driver-var ops in the loop applying the same veto, with its own skip counter in
`DetectorStats` so the suppression is countable rather than invisible.

**6. The fixture is fixed.** Shape 2 becomes `SetRange("Entry No.", R."Entry No.")` — no
entry-number filter exists before retrieval, so this genuinely narrows traversal to the current
row. A separate `SetFilter` case is added. The original `SetRange(Amt, R.Amt)` is KEPT as a
distinct case, because it is the row-dependent syntax shape and should also be vetoed.

### Scope, stated so it is not over-claimed

This closes the traversal holes named in the issue plus P7's. It does NOT make every remaining d5
or d60 recommendation sound: d5 does not establish absence of conditional branches or validate
assignment values, and d60 does not classify assignment right-hand sides as constant or
same-record. Those are separate gaps, out of scope, and recorded rather than implied fixed.

## Acceptance matrix

| # | item | proof |
|---|---|---|
| A1 | `Next()` with no args in the terminator stays an ordinary advance | control fixture still reported by both detectors |
| A2 | `Next(1)` in the terminator stays eligible | fixture; still reported |
| A3 | `Next(2)` disqualifies | shape 1: no finding from d5, none from d60 |
| A4 | `Next(0)`, `Next(-1)`, `Next(Step)` disqualify | one fixture case each |
| A5 | a BODY `Next()` disqualifies even at unit step | P7's shape: no finding from either |
| A6 | in-loop `SetRange` narrowing to the current row disqualifies | `SetRange("Entry No.", R."Entry No.")`: no finding |
| A7 | in-loop `SetFilter` disqualifies | own fixture case |
| A8 | in-loop `SetCurrentKey` disqualifies | shape 3: no finding |
| A9 | **previously-rejected companions stay rejected** | `Delete`, `Insert`, `Validate`, `Get` in-loop still produce NO finding — pinned as regression tests, because v1 would have started accepting them |
| A10 | `SetLoadFields`/`AddLoadFields` stay harmless | fixture; still reported |
| A11 | `None` argument info is not read as "no arguments" | direct unit assertion on the predicate, not only end-to-end |
| A12 | `is_terminator_next` is unchanged | d1/d2/d1_graph behaviour identical; their goldens do not move |
| A13 | d60's suppression is countable | new `DetectorStats` skip counter, asserted non-zero on the fixtures |
| A14 | discrimination, per arm | break each, watch the matching fixture regain its finding, byte-restore under sha256 |
| A15 | the L2 golden movement is explained | every moved line accounted for as a `Next` gaining its argument list, in the triage receipt |
| A16 | nothing else moves | `check-goldens` explained-and-green, `cdo-gate` green |

## Questions for round 2

1. Does keeping the allow-list AND adding the veto fully close A9's regression risk, or does the
   two-gate shape leave an op class in neither?
2. Is "exactly one eligible advance, in the candidate loop's own terminator" the right rule, or
   does it reject a legitimate shape (e.g. a guarded early `Next` that cannot execute)?
3. astra noted `in_until_condition` also covers compound conditions and early-stopping terminators,
   so being in the `until` is not itself proof of exhaustion. Is recording that as a stated limit
   enough, or does it need a canonical exhaustion check?
4. Mark every entry `accepted` or `re-raised`.

## Round 2, and the one place the reviewers disagree

flash accepted all eight entries. astra accepted B1-B6, re-raised B7 and B8 as **wording only**
(both fixed above), and raised Q3 as a NEW substantive requirement. They disagree on Q3, so it is
mine to settle with evidence rather than by preference.

### Q3: astra's counterexamples are REAL — I measured them

```al
until (R.Next() = 0) or StopNow;   // modifies only the first row when StopNow
until R.Next() <> 0;               // wrong polarity
```

Both have exactly one unit-step advance in the candidate loop's own terminator, no filter or key
mutation, no in-loop call. Both pass every rule in design v2. **And both are reported by d5
today** — added to the fixture as `CompoundTerminator` and `WrongPolarityTerminator`, and measured:
REPORTED, alongside the control. This is a live hole, not a theoretical one.

### Why it is nonetheless scoped OUT of #21, with the feasibility measured

astra's bounded fix is to recognise a terminator structurally equivalent to `R.Next(args) = 0`.
**That data does not reach a detector.** `PLoop` (`features.rs:292-298`) carries exactly three
fields — `id`, `loop_type`, `source_anchor`. No terminator expression is retained in the
detector-facing projection; the only terminator signal a detector sees is the per-op boolean
`in_until_condition`, which by construction cannot separate `until R.Next() = 0` from
`until (R.Next() = 0) or StopNow`, because the operator was never kept.

So the exhaustion check is not a refinement of this issue's L5 work. It is an additional L2
FEATURE — carrying the terminator's expression shape — on top of the argument capture this issue
already needs. (Not another layer, and not necessarily another serialized-contract move: astra's
round-3 precision note, accepted.) And it answers a different question — what does this loop's
termination condition mean — from the one the issue names, which is what these companion ops'
arguments mean.

**What changes in the design instead:** the traversal rules are stated as NECESSARY, not
sufficient. v2's wording implied the terminator rule established whole-set traversal; it does not,
and it no longer claims to. The fix still only ever REMOVES findings, so shipping it with
exhaustion unchecked leaves the detectors strictly more precise than today — just not sound.

Filed as a discovery with both measured counterexamples as its reproducer.

### astra's round-2 refinements, accepted

- **A9's baseline is d5 ONLY.** d60 has no allow-list, so same-driver `Delete`/`Insert`/`Validate`/
  `Get` are rejected by neither its existing gates nor the new veto. That is a PRE-EXISTING gap,
  not a regression this change introduces, and A9 must not promise the baseline for both.
- **Cardinality belongs to the caller, not the per-op predicate.** `whole_set_break(op, loop)`
  cannot count advances. A loop-level helper collects all driver `Next` ops whose `loop_stack`
  contains the candidate loop and requires exactly one — including requiring at least one in d60.
- **A4 gains `Next(+1)` and an expression case**, which it omitted.

## Questions for round 3 (last round)

1. Is scoping exhaustion OUT justified by the feasibility measurement above — `PLoop` carries no
   terminator expression, so the check is a second L2 feature — or does it still belong in #21?
2. Do the B7/B8 wording corrections actually land, or is there still a passage calling the
   contract move necessary or a dump absence a null?
3. Mark every entry `accepted` or `re-raised`. `spec_rounds` is 3/3 after this.

## Spec panel: CONVERGED

All ten entries accepted by BOTH reviewers at round 3. Three rounds, three criticals, and the
design that emerged is materially different from the one I brought:

- v1 would have DELETED d5 allow-list and, in doing so, started accepting `Delete`/`Insert`/
  `Validate`/`Get` -- removing three false positives while adding a new class of them. astra
  caught it; the allow-list stays and the new predicate is a veto layered on top.
- v1 compared filter argument TEXT and called that redundancy. Both reviewers killed it with
  counterexamples. Dropped.
- v1 had no terminator/body distinction at all. flash predicted the shape, I measured it, and d5
  reports it today (P7).

B9 is the one entry accepted as an explicit scope EXCLUSION rather than a fix: both reviewers
ruled the exhaustion check needs the terminator expression, which no detector-facing projection
carries. Its two counterexamples are measured, live false positives and become the discovery reproducer.

Round 3 also corrected two over-claims in my own scope rationale ("a third layer, a second
contract movement" and "nowhere in the pipeline"), both fixed above. That is the failure mode
this session keeps surfacing in my writing: the argument was right and I stated it larger than it was.

## Implementation and discrimination proofs

Commit `3b12dc0a`. Baseline on the committed fixture: `BadLoop`, `UnitStepAdvance` and
`InLoopLoadFields` reported; the other eleven routines not. 14 of 14 acceptance rows match.

Every break below was ASSERTED to have applied before measuring — a scripted break that matched
nothing returns green and reads exactly like a passing test — and every file was byte-restored
under a sha256 check.

| # | break | result |
|---|---|---|
| D1' | `is_unit_advance` accepts ANY integer step, not only `1` | **DISCRIMINATES** — `MultiStepAdvance` and `ZeroStepAdvance` regain their findings |
| D2 | `SetRange`/`SetFilter` stop vetoing | **DISCRIMINATES** — `InLoopSetRange`, `InLoopSetFilter` regain |
| D3 | `SetCurrentKey` stops vetoing | **DISCRIMINATES** — `InLoopSetCurrentKey` regains |
| D5 | the veto becomes d5's SOLE gate, replacing `ALLOWED_OTHER_OPS` | **DISCRIMINATES** — `InLoopDelete` and `InLoopValidate` start being reported |
| D4 + D4' | placement and cardinality broken TOGETHER | **DISCRIMINATES** — `BodyAdvanceSkipsRows` regains |
| D4 alone | cardinality relaxed to "at least one advance" | **no flip** — see below |
| D4' alone | terminator ownership not required | **no flip** — see below |

**D5 is the one worth reading twice.** It reproduces exactly the regression astra predicted at
spec round 1: make the new predicate the only gate and d5 starts reporting `InLoopDelete` and
`InLoopValidate`, which it rejects today. That is the design v1 I brought to the panel. The proof
is that keeping `ALLOWED_OTHER_OPS` is load-bearing and not just conservatism.

### Two proofs that did NOT flip, and what each actually means

Neither is a pass. CLAUDE.md is explicit that a green break is evidence about the TEST, so both
were diagnosed rather than accepted.

**My first D1 was an invalid break.** I removed `"Next"` from `FIELD_ARGS_OPS`, expecting the bug
back. It did not come back — because `field_argument_infos` is then `None`, and `is_unit_advance`
treats `None` as NOT eligible (the tri-state B6 required). So that break makes the detector MORE
conservative, silencing even `BadLoop`; it does not reintroduce the defect. The break was wrong,
not the code. D1' reintroduces the actual defect — accepting any integer step — and discriminates.

**`BodyAdvanceSkipsRows` is defended REDUNDANTLY, and that is a real property of the code.**
Breaking cardinality alone leaves the per-op placement check (the body `Next` is not the
terminator, so it still vetoes). Breaking placement alone leaves cardinality (there are two
advances). Only breaking BOTH brings the shape back, which it does. So the pair is load-bearing
and neither half is individually provable against this shape — recorded as a stated LIMIT of these
two proofs rather than papered over by pretending a single-arm break worked.

## Final (implementation) panel — a SUBSTITUTE reviewer, and what that costs

The rostered reviewers were unreachable: `gpt-6-astra`, `gemini-3.8-flash` and `claude-fable-5`
all returned `429 quota exceeded` through pi, which fronts a single `github-copilot` provider —
so the quota is provider-wide and no pi substitute existed. At the operator's instruction the
review was instead run through the Agent tool on `fable`, a different backend entirely.

**It found ten things and was right about every one that was checked.** Two mattered:

- **C1 (critical): half the fix had NO committed coverage.** The d5 acceptance fixture is a plain
  codeunit, and d60 returns before its loop unless the workspace holds an Upgrade/Install object
  (`d60.rs:88-91`). So every "no finding from d60" acceptance row was pinned by nothing, and my
  report of "14 of 14 rows" was d5 only — the d60 numbers came from a scratchpad fixture that was
  never committed. Fixed: seven routines in `ws-d60`, and with the d60 pass removed **6 of 6**
  unsound shapes return.
- **I1 (important): the `None` arm became unpinnable end-to-end.** Adding `"Next"` to
  `FIELD_ARGS_OPS` means L2 now always captures, so `None` occurs in no fixture and a future edit
  making it eligible — the actual bug direction — would move no golden at all. Only a unit test
  can catch it; three now exist in `detectors/mod.rs`.

Also: **I2** (`Reset`/`Copy`/`Find*`/`Get` on the driver are traversal breaks in this issue's own
category that d60 had no gate for), **I3** (this ledger's triage receipt under-itemised ~2,000
moved lines and mislabelled r2a/r2d as r0-goldens), **M5** (dead fixture text), **M6** (the stale
"never fix the vector" doctrine — fixed in place rather than filed, because it is a one-line edit
in a file this change already touches), **M7** (A16's proof column was written before any gate log
existed). **M4** independently reproduced this ledger's manifest arithmetic, including the
off-by-one: r3a2 emits no effect for `Validate` at all, so 12 Modify + 13 Next + 12 FindSet +
1 Delete = 38 where r3a3 counts 39.

### Two errors of mine that the review exposed indirectly

**My first fix for C1 was itself hollow.** `ws-d60/src/Codeunit.al` holds TWO objects; appending
before the file's last brace put all seven routines in codeunit 50935 "D60 Normal", outside d60's
scope, where they were silently unreported — a fixture that passes while pinning nothing, which is
the exact defect C1 raised.

**I then diagnosed that with a probe placed by the same wrong assumption** and concluded d60
reports only triggers, not procedures. That is false, and I had written it into a source comment
before checking. Moved into codeunit 50934, the unit-step procedure reports correctly. When a
probe and the thing it probes share a bug, the probe confirms the bug.

Twice in this issue a green result meant a broken test rather than working code — once in the
discrimination pass, once here.

## Register status: NOT converged, and the merge is an operator override

| entries | state |
|---|---|
| B1-B10 (spec) | accepted by BOTH rostered reviewers at spec round 3 |
| F1-F7 (implementation, from fable) | raised and **fixed**, but reviewed by NOBODY — fable has not seen the fixes, and the rostered reviewers never saw the implementation at all |

`attest` would refuse this register, and correctly: it reads `reviews.get("astra")` and
`reviews.get("flash")` by name (`scripts/agentflow/cli.py:513`) and requires both `accepted`.
Recording fable's verdict under either name would forge a record that is posted verbatim as the
public PR comment, so it was not done.

**This merge therefore happens on the operator's explicit instruction, outside the gated path,
with no attestation.** That is stated here rather than implied, because the whole point of the
attestation is that nobody has to take the merge on trust — and for this one, they do.

## Gate results

All four supervised by the executor (`"supervised": true`), none timed out, on a QUIESCED
worktree with no reviewer or other agent touching it.

| gate | exit | log |
|---|---|---|
| `scripts/ci-steps all` | **0** | `logs/ci-steps-all-21d.log` |
| `scripts/check-goldens --verify-coverage` | **0** | `logs/check-goldens-coverage-21d.log` |
| `scripts/check-goldens` | **0** | `logs/check-goldens-21d.log` |
| `scripts/cdo-gate` | **0** | `logs/cdo-gate-21d.log` |

**Only the `-21d` run is cited.** Two earlier runs are discarded: `-21` predates the review
fixes, and `-21b` overlapped the substitute reviewer's discrimination patches on this same
worktree. `ci-steps-all-21b` finished before the first patch and was sound, but citing a run
whose siblings are not is worse than re-running, so all of `-21b` is dropped.

A16 is claimed on this run and not before — the earlier wording asserted gate results that did
not yet exist, which was the same over-claim shape as the review finding that caught it.

## Not delivered

- **An attestation.** The register is honestly NOT converged: F1-F7 are fixed and reviewed by
  nobody. `attest` would refuse it, and recording the substitute's verdict under a rostered
  reviewer's name would forge the public PR comment. The merge is an operator override.
- **An implementation review by the rostered reviewers.** They were unreachable for the whole
  final panel; `fable` stood in on a different backend.
- **A re-review of the twelve fixes.** Pass 2 verified the first seven; the five it raised in
  that pass (N1-N6) are fixed and unreviewed.
- **Exhaustion checking** (B9) and the `until Done` false negative — both stated limits, to be
  filed as follow-ups.

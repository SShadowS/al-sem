# Issue #33 — cone representative selection can discard the strongest obligation

## Claim

```json
{"run_id":"20260920-110144-f4ce12","issue":33,"attempt":1,
 "session":"https://claude.ai/code/session_01Udxt1D5qya52q1HFXUzARp",
 "branch":"issue/33-agent-discovery-l4-cone-represen-a1",
 "worktree":"U:\\Git\\al-sem-issue-33-a1",
 "body_hash":"3707d320f97101c8f2fd191c416682ef354977a85998e89cfdfb47cc903bdd2f"}
```

Worktree from `master` @ `731ce416`. Reviewers: pi's only provider still returns `429 quota
exceeded` for every model (probed `gpt-6-astra` this session), so the OPERATOR-DIRECTED stand-ins
run both panels — fable in astra's slot, opus in flash's — recorded in the attestation via
`attest --substitute`.

## Classification

**ARCHITECTURAL.** Not because any single edit is large — each is a comparator — but because the
change alters what the whole db-effect substrate considers the representative obligation for a
table, and that value feeds the L5 detectors and the digest's witness reconstruction. It also
turns out to REQUIRE a second, cross-layer change to stay sound (probe P4).

## Assumption probes

Full report: `.agent/runs/20260920-110144-f4ce12/probe-33.md` (read-only investigation).

| probe | result | evidence |
|---|---|---|
| P1 — there are four reduction sites | **FALSIFIED, there are FIVE** | `capability_cone.rs:2242` and `:3344` (byte-identical copies), `:1414`, `:1542`, plus a fifth at `:1687`/`:1725` that is a BFS first-wins with NO key — it needs an upgrade-on-better, not a reorder. The issue's "four" undercounts, and site 2 being a copy of site 1 is how it came to be missed in the first place |
| P2 — the key omits temp state | **HOLDS** | `inherited_fact_key` (`:1204-1212`) is `op\|resource_kind\|resource_id\|confidence`; `extra`, and so `temp_state`, is absent. That is the whole bug surface |
| P3 — the two named tests enshrine the wrong answer | **HOLDS** | `temp_physical_dedup_trap_follows_the_winning_representative` and `bfs_sibling_facts_use_the_key_deduped_direct_map`: four assertions invert, six are structural and survive, and one (the raw/deduped asymmetry) LOSES discriminating power and needs a redesigned fixture rather than an inverted expectation |
| P4 — "depends on the witness/ordering fix landing first" | **CONFIRMED, and the dependency is NOT met** | The witness/ordering fix is #32, which was ANSWERED AS A SPIKE, not implemented. Its own answer says the false positive is unreachable today because "the upstream cone dedup MASKS it by discarding the physical representative" — the exact masking #33 removes. See below |
| P5 — no existing golden covers a mixed temp/physical cone | **HOLDS, measured** | Zero of the 93 `tempState`-bearing `tests/r3a3-goldens/` files carry two facts sharing `(op, resourceKind, resourceId, confidence)` with different `tempState`, within a routine or across routines in one fixture. So sites 1-5 have no existing coverage and no golden should move — to be VERIFIED by `check-goldens`, not assumed |

### P4 in full: landing #33 alone creates a new false-positive class

Ordering decides *whether* an occurrence is a physical write from the FACT's `temp_state`
(`ordering_engine.rs:226`, the cone representative's), but decides *where in the source* it
happens from the WITNESS TERMINAL's operation id (`digest.rs:2445` → `ordering_engine.rs:167`).
Those are two different objects, and `fact_equivalent` — the terminal matcher (`digest.rs:683-713`)
— does not compare `temp_state` at all.

Today the cone hands ordering the TEMP representative, whose `Known{true}` makes
`is_physical_write` false and skips the whole `WRITE_PENDING_AT_EXTERNAL_IO` arm. Flip the
representative to physical and the terminal matcher still lands on the nearest equivalent direct
fact — the temp one. The result is an occurrence graded PHYSICAL and anchored at a TEMP
operation: d47 fires CRITICAL on `Temp.Insert(); Http.Get(); PhysicalWriter()`.

So this change carries #32's guard with it: at `digest.rs:997` the terminal `find` must also
require the temp class to match the representative's. The narrow placement (guard the `find`, not
`fact_equivalent` itself) is deliberate — `fact_equivalent` is also the reverse-BFS prune seed
(`digest.rs:947`) and is intentionally tolerant of absent fields, so widening it there would
degrade witnesses to `terminal-not-found` and move `via_paths` bytes on many goldens.

## Design

1. **One comparator, five call sites.** Extract `better_representative(a, b)` (or a
   `(is_known_temp, …)` sort key) in `capability_cone.rs` and route all five sites through it.
   Non-known-temp is ordered LEXICOGRAPHICALLY FIRST, ahead of distance — a temp fact at distance
   1 must not beat a physical fact at distance 2. Site 5 is a BFS first-wins and needs an
   upgrade-on-better rather than a reordered comparison.
2. **Cache the predicate, do not compute it in the comparator.** `ConeFactEntry` gains
   `is_known_temp` alongside the cached `rep_key`, set at mint time: the entry is `Arc`-shared
   across millions of merges per Base-App run.
3. **#32's guard, narrow.** `digest.rs:997`'s terminal `find` additionally requires
   `fact_is_known_temp(direct) == fact_is_known_temp(fact)`.
4. **Honest framing, in the code.** Non-known-temp is NOT "proven physical": an unknown candidate
   outranks a known-false one within the preferred class. That is conservative for permissions and
   must be written as such, not as "selects the strongest physical evidence".

**Acceptance matrix** (each row → what proves it):

| # | acceptance item | proof |
|---|---|---|
| A1 | each of the five sites prefers non-known-temp | one fixture per site where the temp fact currently wins; after the change the representative is non-known-temp. A per-site discrimination proof reverts that site alone and watches its fixture fail |
| A2 | the preference is lexicographically first, not an equal-distance tie-break | a fixture with temp at distance 1 and physical at distance 2; the physical wins |
| A3 | derived physical writes contain the table | `writes_physical_tables_of(root)` is non-empty for the trap fixture |
| A4 | the two named tests are re-triaged, not preserved | their structural assertions stay; only the unsafe temp-winner expectation inverts; the one that loses discriminating power gets a redesigned fixture |
| A5 | no NEW false positive from the unmasking | a d47 fixture in #32's shape (temp write, external IO, then a physical write of the same table) produces NO `WRITE_PENDING_AT_EXTERNAL_IO`, while a control whose physical write genuinely precedes the IO still does. Discrimination: remove the terminal guard and the first fixture gains the finding |
| A6 | no golden moves, or every moved line is explained | `check-goldens` before and after; P5 predicts zero movement and that is a prediction to falsify, not an assumption |
| A7 | the detector-triage obligation | d8/d44/d47 findings on a real workspace, before and after, with every moved finding triaged against source |

## Spec panel round 1, and the design fork it forced

Reviews: `.agent/runs/20260920-110144-f4ce12/fable-spec-33.md` (astra slot),
`opus-spec-33.md` (flash slot). Ten register entries, four critical.

The panel did not merely poke holes in the comparator design; opus (C3) asked whether the
comparator is the right shape AT ALL, and named the alternative: **widen `inherited_fact_key` by
temp class** so both facts survive, and let the conservative merges L5 already has
(`digest.rs:merge_temp_state`, `ordering_engine.rs:known_temp_only`) do the work they were
written for. Its objection to the comparator is that it is lossy in a place where the
downstream layers are built to consume both facts.

opus asked for that fork to be decided by measurement rather than argument. It was.

### The measurement (`.agent/runs/20260920-110144-f4ce12/measure-33.md`)

On the pinned CDO baseline, engine at `731ce416`, with the widening switched by env var in an
out-of-tree build so both sides run the same binary:

| | today | widened | delta |
|---|---|---|---|
| cone entries (cross-app, per-routine inherited) | 166,503 | 169,846 | **+2.0%** |
| cone entries (source-only) | 35,502 | 37,335 | +5.2% |
| wall, median of 3 paired runs | 40.02 s | 40.04 s | inside noise |
| peak RSS, median of 3 | 1993 MB | 1959 MB | inside noise (the ±1.4% spread WITHIN one config is larger than the difference between them) |

And the bug is not a curiosity: **3,343 mixed keys on 1,779 routines; in 1,755 of them the
known-temp fact wins today and hides a physical one — 292 of those are WRITES, on 222 routines.**
Source-verified examples are in the report (a `TempDOFileEDoc` local hiding a physical `Modify`
of `"CDO File"` in `CDOEMailTemplateManagement.AppendXMLToPDF`; the classic `TempErrorMessage`
pair in Base Application `Codeunit 1380.BatchProcess`).

**Decision: design v2 takes the widened key.** The memory objection — the one real reason to
prefer the lossy collapse in a repo that fought a 5.1 GB peak — does not survive measurement.
And the comparator is worse than "adequate but lossy": at the 1,588 keys where the non-temp fact
already wins, it would permanently discard the temp fact that the L5 merges are written to
consume.

### Design v2

1. **`inherited_fact_key` gains a temp-class component** (`capability_cone.rs:1204`). Binary
   (`fact_is_known_temp`), so at most one extra entry per key.
2. **No comparator, no tie-break, no ordering change anywhere.** Every one of the five reduction
   sites keeps its existing comparison byte-identical. This is what makes S2 and S3 moot rather
   than fixed: the two classes stop colliding, so nothing has to arbitrate between them, and the
   same-class populations opus warned about are untouched by construction.
3. **`tests/r3/r3a3_oracles.rs:73` reimplements the key** and widens with it.
4. **#32's guard, narrow** (`digest.rs:999`): the terminal `find` requires the temp class to
   match the representative's, via a new `is_known_temp_snap(&Fact)` that folds the two existing
   `matches!` copies in that file. The same guard goes on the reverse-BFS prune seed (`:947`).
   Without it a physical representative can still terminate on a temp operation, because
   `fact_equivalent` does not compare temp state.
5. **Framing, stated correctly**: conservative for the physical-write SET, never "conservative
   for permissions" — d44 subtracts writers from readers and can DELETE a finding; d43 can
   DOWNGRADE one. Both directions are hazards and both get triaged.

### Acceptance matrix v2

| # | item | proof |
|---|---|---|
| A1 | a mixed key yields TWO facts, not one | a fixture with a temp and a physical write of one table; both survive in `capability_facts_inherited`. Discrimination: revert the key widening and the pair collapses |
| A2 | the physical obligation is no longer hidden by distance | temp at distance 1, physical at distance 2: `writes_physical_tables_of(root)` contains the table. Fails on master |
| A3 | `oracle_r3a3_inherited_factkey_dedup` still holds | the widened key is still unique per surviving fact |
| A4 | the two named tests are re-triaged, not preserved | structural assertions stay; the temp-winner expectation goes. Executable proof for the sibling test: fold the RAW Vec instead of the deduped map and it must FAIL |
| A5 | no NEW false positive from the unmasking | #32's shape (temp write, external IO, physical write of the same table) produces NO `WRITE_PENDING_AT_EXTERNAL_IO`, AND on that same fixture the representative is physical and the witness is `incomplete: false` — so a pass cannot mean "the fix is absent". The control (physical write genuinely before the IO) is verified to fire ON MASTER before it counts |
| A6 | no EXISTING golden line moves; every new line belongs to a new fixture | `check-goldens` before and after, diffed per line |
| A7 | the detector-triage obligation, BOTH directions | d8, d43, d44, d45, d47, d50 on a real workspace before and after; every appeared finding and every vanished/downgraded finding triaged against source; above 30% false positives the affected detector ships opt-in |

## Why this tick does not merge #33, and what it does instead

The operator's constraint is TICKS, not wall-clock time, so the work continues rather than
stopping at the design. What cannot continue is the ATTESTATION: the executor's 4-hour
wall-clock cap on this run expires at 15:05, and after that a supervised gate cannot even be
STARTED (`budget.check_deadline` guards `run` and `charge`; `merge`, `post-merge` and the rest
are unaffected). A gate run outside the supervisor is fine as evidence for me, but passing its
exit code to `attest` would claim a supervised run that did not happen, so this tick does not
attest and does not merge.

Two things therefore land on the branch instead of in `master`: the implementation of design v2,
verified locally and labelled as UNSUPERVISED, and the CDO measurement of what actually moves.
The next run claims attempt 2 with a fresh budget and needs only: supervised gates, the triage
wave over the six detectors, the final panel, and the merge.

## Spec panel round 2: design v2 ratified, two acceptance rows still could not fail

Both stand-ins RATIFIED design v2 (`fable-spec33-r2.md`, `opus-spec33-r2.md`) and accepted S1-S7,
S9 and S10. Both independently RE-RAISED S8, and both were right:

- **A4's replacement proof comes back GREEN under v2.** The sibling fixture's two facts are of
  DIFFERENT temp classes, so under the widened key both survive the deduped map and folding the
  raw Vec gives the same answer. The break applies cleanly and the test still passes — CLAUDE.md's
  "a discrimination proof that PASSES is evidence about the TEST", for the third time in this arc.
- **A3 could not fail at all.** `discover_fixtures` enumerates the r3a3 corpus, which has zero
  mixed keys (P5, plus opus's own 834-file scan), so the row re-asserted an invariant over a
  population that cannot exercise the change.

Two evidence corrections they made to my register, both adopted: the oracle half of S2 is FIXED
(by widening the oracle's mirror), not moot — under v2 two facts really do share one BASE key in
`out`; and S4's count/type was wrong — `digest.rs` holds ONE `matches!` over
`Option<SnapTempState>`, the other lives in `ordering_engine.rs` over a different type, so one
`is_known_temp_snap(&Fact)` folds NEITHER without an inner over the Option.

Both also objected to S10's label: `refuted` reads as "the reviewer was wrong", and they were not
— the finding is true of design A and inapplicable to v2. The register vocabulary is closed and
has no `moot`, so the distinction is written into the evidence instead.

### Acceptance matrix v3 (supersedes v2)

| # | item | proof |
|---|---|---|
| A1 | a mixed key yields TWO surviving facts | a fixture with a known-temp and a physical write of one table. Discrimination: un-widen the key and the pair collapses |
| A2 | the physical obligation is no longer hidden by distance | temp at distance 1, physical at distance 2: `writes_physical_tables_of(root)` contains the table. Fails on master |
| A3 | the r3a3 dedup oracle still holds ON A POPULATION THAT CAN EXERCISE IT | the A1 fixture is added to the r3a3 corpus WITH a golden; discrimination is to widen the production key but NOT the oracle's mirror (`r3a3_oracles.rs:73`) and watch `oracle_r3a3_inherited_factkey_dedup` fail. `oracle_r3a3_inherited_keys_trace_to_a_direct_producer` stays true either way (`retag` preserves `extra`) — the widening is load-bearing for the first oracle and inert for the second, and the ledger says which |
| A4 | the two named tests are RETIRED, not rescued | their subject (a mixed key collapsing to one representative) no longer exists. The lost coverage is replaced by the oracle below |
| A4b | **new oracle**: `writes_physical_tables_of(r)` equals the naive union over `r`'s reachable direct facts, for every routine in every fixture | fails on master — the trap test's own construction is the counterexample — and passes under v2 |
| A5 | no NEW false positive from the unmasking | #32's shape produces NO `WRITE_PENDING_AT_EXTERNAL_IO`, AND on that same fixture the representative is physical and the witness is `incomplete: false`, so a pass cannot mean "the fix is absent". The control is verified to fire ON MASTER first |
| A6 | no EXISTING golden line moves; every new line belongs to a new fixture | `check-goldens` before and after, diffed per line |
| A7 | the detector-triage obligation, BOTH directions | d8, d43, d44, d45, d47, d50 on a real workspace before and after; every appeared, vanished or downgraded finding triaged against source |
| A8 | **additivity** — no existing fact changes or disappears, only new ones appear | provable from the shape (the widened key's candidate group is a SUBSET of today's, and each site's rule applied to a subset returns the same winner for the winner's own class), asserted at fixture level, and corroborated on CDO by a FOLD-COUNT comparison per table (no count decreased anywhere) — not, as an earlier draft of this row said, by a byte-identical join over the whole before/after fact sets, which was never run |

## Implementation, gates, and the final panel

Implemented by an `opus` subagent against design v2 (`impl-33.md`): the widened key, #32's guard
in both places, the two named tests RETIRED (not rescued), the naive-union oracle that replaces
them, a new `ws-d33-mixed-temp-key` fixture with its r3a3 golden, and the A5 d47 pair. The
implementer reported one of its own discrimination proofs coming back GREEN and root-caused it
rather than quietly re-running — the projected anchor comes from the fact's own
`witness_operation_id`, so the fingerprint test was pinning something other than its docstring
claimed; the claim was replaced with the measured one and the test renamed.

### Gates (supervised, on the final tree)

| gate | exit | note |
|---|---|---|
| `ci-steps all` | **0** | two earlier attempts failed with `rust-lld: permission denied` on `program_resolve_harness.exe` — a build race with the implementer's own still-finishing test run, not a test failure. Recorded rather than hidden; the stale binary was removed and the gate re-run clean |
| `check-goldens --verify-coverage` | **0** | |
| `check-goldens` | **0** | no EXISTING golden line moved; the new lines belong to the new fixture (A6) |
| `cdo-gate` | **0** | |

**Which gate ran on which tree — stated precisely, because they are not all the same tree.**
All four were re-run after the doc-fix batch and all four passed (14:58). Two further DOC-ONLY
edits followed, both forced by the panel: the T1 residual, and then opus's own correction of the
replacement (a `None`-rid fact sorts LAST, not first — the sort key joins its parts with `|` =
0x7C, above every byte a real rid starts with, so the ordering leg of the argument was false in
three places including the shipped CHANGELOG; the wildcard leg carries the conclusion alone).

The executor's 4-hour cap then closed. What that leaves:

| gate | tree it ran on |
|---|---|
| `ci-steps all` (build + clippy + full suite) | the tree WITH the first doc fix — exit 0, supervised (`ci-steps-all-e`, 572s) |
| `check-goldens`, `--verify-coverage`, `cdo-gate` | the tree BEFORE both doc fixes — all exit 0, supervised |
| final tree (both doc fixes) | verified UNSUPERVISED: `cargo clippy --all-targets --all-features` exit 0 with zero warning/error lines, `cargo test -p al-sem --lib` 1804 passed, `cargo test --test r4` 30 passed |

The delta across all three trees is four doc comments and one CHANGELOG paragraph — no Rust
expression, no test logic, no fixture, no golden. Clippy is the gate a doc comment can actually
break (a malformed doc attribute or a broken intra-doc link), and clippy ran clean on the final
tree. `check-goldens` reads goldens, the coverage check reads declared directories and `cdo-gate`
runs CDO ratchets; none of them reads a test module's doc comment. Both stand-ins were asked
whether to disclose this or revert the doc fix to make the trees byte-identical, and both said
disclose: reverting would ship a sentence the panel has proven false in order to make a
bookkeeping artifact tidier.

### A7, discharged — and its honest limit

`triage-33.md`: across the FULL 41-detector default set on the pinned CDO baseline, **2069
findings before and after, 0 appeared, 0 vanished, 0 severity changes**, and exactly 2
content-only changes, both in d8, both triaged against source at **0% false positives**. No
detector approaches the 30% opt-in threshold.

The mechanism matters more than the zero: the implementer instrumented `fold_fact` on both
binaries and, FOR THE THREE TABLES IT MEASURED, the count of PROVEN-physical (`known(false)`)
write folds is identical before and after. That is a three-table result, not a global one, and
the earlier draft of this paragraph overstated it as global. What the
widened key propagates is `parameter-dependent` facts that a known-temp sibling used to displace.
No fold count anywhere decreased — additivity (A8) confirmed on real data. This is exactly why
the framing S5 forced matters: non-known-temp is NOT proven physical.

**Stated limit (T7, opus N2).** Four of A7's six detectors — d43, d44, d45 — emit zero findings
on this workspace before AND after, so the obligation was really exercised against d8 and d50.
d43's DOWNGRADE and d44's DELETE paths, the two directions S5 and S7 exist to police, have no
population here, and `alsem analyze`'s substrate is source-only by design so the cross-app
population they need is unreachable through the shipped CLI. Filed as issue **#57**.

## Final panel rounds 2-4, and how T1 closed

Round 2 ratified the fix batch (T1-T7), with fable re-raising T1 and both re-raising T5. Round 3
fixed both and produced the sharpest correction of the arc: **opus caught its OWN earlier claim**
— it had argued (in the code review) that a `None`-rid fact "sorts FIRST" and that the
first-match-wins terminal therefore reaches it first. It sorts LAST: `capability_fact_sort_key`
joins its parts with `|` = 0x7C, which outranks every byte a real rid begins with. The false leg
had by then reached three places including the shipped CHANGELOG. The conclusion survives on the
wildcard leg alone (`fact_equivalent` compares rids only when both are `Some`), and the worked
example is now a three-routine shape where the mis-terminal needs no ordering argument at all.

The final-panel cap was spent at that point, so closing T1 needed the OPERATOR's authorization
for one more single-entry round — the same exception as #41's. Granted; opus verified the remedy
against the tree and accepted, noting the false claim now survives only inside an explicit
retraction, which is what it asked for.

**Register: 20 entries, every one accepted by both stand-ins, none open, no blocking entry
deferred, one refuted with its reason stated in words because the vocabulary has no `moot`.**

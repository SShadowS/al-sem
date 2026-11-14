# Output validation, round 2: what the reviews changed (design spec)

**Status:** design only, revision 3 (after two review rounds: Fable and astra accepted
revisions 1 and 2 "with changes"). No code lands with this file.
**Date:** 2026-11-07. **Based on:** master `20ce85c4`, binary al-sem 2.1.1.
**Extends:** `2026-11-06-output-validation-r1.md` (here "r1"). Its rules stay: the per-finding
"Current / New / Files / Tests" shape, the word table r1 §8.1 (owner-approved; reused here, and
every new word is marked **new word**), the JSON migration rule r1 §8.3, and the test rules of
r1 §12.1 (one discrimination proof per correctness guard; none for wording or size).
**Input:** the 40 round-2 calls (c01–c30 repeated, n01–n10 new) on the CDO baseline
(`bc3ccb18`); the fact checks; two independent reviews against one rubric (Fable, astra). All
under `target/tmp/output-validation/round2/` (local, not committed): `review/review-bundle.md`,
`bundle.md`, `factcheck/*.md`, `review/review-{fable,astra}.md`, `review/rubric.md`.
**Reviews of this spec:** revision 1, `review/spec-r2-review-{fable,astra}.md`; revision 2,
`review/spec-r2-rev2-review-{fable,astra}.md`. What each item changed: "Revision 3" and
"Revision 2" below.
**Register:** every finding of both output reviews, merged, is in Appendix A (G1–G62; G59–G62
added in revision 2). Round-1 findings a reviewer marked not resolved or partly resolved are
reopened in Appendix B.

Words used here are r1's (hub, tier, relation, boilerplate, envelope). Also:
- **population**: one group of ancestors that share a relation and a tier (for example
  "reached by UI navigation, via-dependency").
- **program entries**: every entry in the loaded program, counted by the canonical entry
  identity of §2.1 (one per entry, in one kind).

**The contract that does not move:** the MCP answer stays the CLI's stdout byte for byte.

---

## Revision 3: what each review item changed

Both reviewers accepted revision 2 with changes. Every item is applied; none is declined. The
`declaredRunnable` rules were checked in `project.rs` (rule 2 at a call site, the `TestPage`
declaration rule) and `impact_graph/mod.rs` before writing.

| Item | Change made |
|---|---|
| 1. Cost matches three bits (both) | §2.1 Cost: `NAV`, `RULE`, `CONFIGURED` give 8 `ext` values and 48 slots per node; `seen` becomes a dense `u64` per node (48 of 64 bits), with the explicit hash fallback kept for `ext` ≥ 8 (other consumers); memory restated (8 bytes per node per scratch instead of 1, one class byte per kept node in `Up`). §2.1 step 2: the tier is read from R0's `Tier`; impact sets no `DEP` bit; a test pins `flag::LOCAL == !is_dependency_node` per app-owned node, and hubs are tested apart as `via-hub` (astra rev-3 review) |
| 2. Configuration witnesses are candidates (astra); rule edges (astra, Fable) | §2.1 step 2: evidence is established only for `local` / `via-dependency` with a known relation; a `via-configuration` state is a candidate that keeps its relation and says it depends on configuration data; it is in candidate counts and ranks first among candidates. Step 1: `declaredRunnable` split into the call-site builtin rule (code) and the `TestPage` declaration rule (`RULE`); the known-false plain declarations make no edge and stay excluded, with a test. Sketch, JSON (`candidates.overConfiguration`), tests and §6.2 updated |
| 3. W1 share (astra; Fable) | §2.1 Interim: one eligibility predicate (the root-kind mask test `programEntries` uses) on both sides, `<n>` taken from reached nodes rather than `Up.entries`, changed routines excluded from both; the line names what neither side counts; a test pins `<n> ≤ <N>` |
| 4. G17 TryFunction (astra) | §3.10 G17: `declared TryFunction; interception depends on how the caller invokes it`, printed only for a TryFunction; the caller-interception follow-up stays |
| Fable: `EventDispatch` superset | §2.1 step 1 table: the cell says it is a deliberate superset of `is_code_call_site` |
| Fable: n09 direct callers and publisher | §2.1 Output: a second n09 sketch for a subscriber with direct callers: the code population counts both together, and the line says so |
| astra/Fable: rule edges must not restore declaredRunnable | covered by item 2 |

---

## Revision 2: what each review item changed

Both reviewers accepted revision 1 with changes. Every must-change is applied. Every
should-change is applied except where a row says "declined" and why. The code claims both
reviewers corrected were checked in the source before writing (§2.1 "What the walk records
today" names what was checked).

### R2.1 Must-changes

| Item | Change made |
|---|---|
| Fable 1 (G2 "shows the field") | §2.2: `each opens a page on the field's table; whether the page shows or edits the field is not established`; the same words in §2.4 |
| Fable 2, astra 2 (NAV predicate) | §2.1 step 1 is now a per-boundary classification table (code / UI navigation / candidate / no bit), checked against `project.rs`'s via-to-boundary map and `is_code_call_site`. A background start, a test runner and a test handler are code; `declaredRunnable` is a candidate (relation not established); a known navigation step is kept on a path that later crosses a hub |
| Fable 3 (total order) | §2.1 step 2: established `code` > established `UI navigation` > candidate (over a hub or a rule edge); within a class the better tier, then fewer hops; the witness comes from the state that sets the class |
| Fable 4, astra should 1 (`Up` has `step`) | §2.1: `Up` implements `step` to record action-run sites (`action_site`, the captions use it) and sets no bit; W2 extends it |
| astra 1 (tier and relation independent; W1 truthful) | §2.1 restates the defect: the tier words are right as tiers; the relation axis is missing and configuration is filed under `via-dependency`, so an agent who has only seen code-only populations reads the tier as code. W1: `no direct code caller found` (computed, never inferred from routine kind); the subscriber line reports direct callers and publishers apart; `external:` suppressed in human text until the code population exists; JSON and the envelope summary carry `relationRecorded: false` and the population's name |
| astra 2 (R0 tier vs app identity; tests promise) | §2.1 step 2: impact's tier uses the app identity `dep_callers.rs` uses (`node_app != primary_app`), not R0's `flag::LOCAL`; in impact's production universe the two agree, and a test pins it. The promise that tests read the walk's bit is removed (the test walk is a different universe and visitor) |
| astra 3 (entry inventory; performance contract) | §2.1 step 4: a canonical entry identity shared by numerator and denominator (overlapping root kinds, `#if` arms, action-run page entries, background targets, roots.config overlays; public procedures stay in `external:`). W1 counts distinct entries. A limited walk makes no program-wide or absence claim. Cost: measurement targets on the walk phase only, with walk time, `Up.states` and peak scratch and visitor memory |
| astra 4 (no stronger claims) | §2.2: `no modeled code-call witness in the selected scope`. §2.4: triggers use the existing dispatch rules (the sited graph's `TriggerOf` and lifecycle sites), pages are `potential UI surfaces`, negatives use the scoped words |
| astra 5 (validation is not a write) | §3.2: `record writes: 16 sites` and `validations: 5 sites` apart; `opFamily: "write"` membership and temporary records stated. §3.1: `may write CDO Setup` |
| astra 6 (schema migrations) | §4.6: every JSON change is a new field or part of a named major (query 5.0.0, digest 5.0.0) with the previous shape kept one release and a reconstruction test; `origin.file` keeps the GUID form and `origin.fileDisplay` is added; `programEntries` stays a number beside a new `programEntriesByKind`; the requested selector stays apart from the canonical display; `dependencies` keeps its list until the major |
| astra 7 (bounded summary) | §3.4: rows use compact references; a test encodes c25's 20 rows and checks the 8 KiB bound; `summaryTruncated` and the index stay; each row keeps conditional persistence, uncertainty, table verdicts and test status (also the owner's c25 condition, §5) |
| astra 8 (appendices) | App. B: F26 is resolved per astra (G9 carries F77 only); F38 Fable resolved / astra partly but acceptable; F41's c15 part points to G53. G53's reason corrected. New rows G59 (c21 JSON path bulk), G60 (n07 tests follow-up), G61 (n07 doc comment), G62 (n05 scope). G48 moved to handled elsewhere as a deferred engine requirement |

### R2.2 Should-changes

| Item | Change made |
|---|---|
| Fable (cost fallback (a) first) | §2.1 Cost: widening `seen` is part of W2's design, not a fallback; an R0 commit that must leave the r0 goldens unmoved (width corrected to `u64` in revision 3) |
| Fable (budget the walk), astra 3 | §2.1 Cost: targets on the up-walk phase (time, `Up.states`, peak memory), not the whole call |
| Fable (interim `<n>` distinct) | §2.1 Interim: `<n>` is the union of entry nodes; the existing fold line's share (`entryBuckets / programEntries`, a bucket sum over distinct nodes) is fixed; W1's fold line says `broad reach`, not `broad navigation reach` |
| Fable (interim words) | §2.1 Interim: `through a dependency`, `within the workspace <k>`; subscriber line as astra 1 |
| Fable (union program-wide line) | §2.1 Rules: a union line when the populations together cross the threshold |
| Fable, astra (bracket form) | §5 new words: `[<tier>, <relation>]`. The rule is astra's, not Fable's: a bare `[<tier>]` is not redefined to mean code; a tool whose populations are code-only by construction says so once in its scope, and tools that mix relations print both axes (§6) |
| Fable (G4 scope split) | §2.4: field triggers, actions and subscribers in W1; table and page triggers in W3. Subscriber row: the publisher plus any direct code caller |
| Fable (§3.6 citation) | fixed: §6 |
| Fable (G17 TryFunction) | §3.10 G17: the declaration fact (wording corrected in revision 3, R3 item 4) |
| Fable (wording test) | §2.1 Tests: the interim bracket check is a golden, not a proof |
| Fable (W2 before W5) | §0.1: W5 depends on W2 |
| Fable (why ≥90%) | §2.1 Rules: one line |
| astra (identical counts do not prove one set) | §0, §2.1: the counts motivate measuring the share; they do not prove set identity (they also differ slightly: 16,749 / 16,754 / 16,760) |
| astra (G11 order and dedup) | §3.5: deterministic order, one test serving several routines counted once, totals and fallback about the full selection |
| astra (G13/G42 suppression and sites) | §3.7: `coverage incomplete` is dropped only when each of its reasons is a listed blocker; §4.5 G42: a blocker without a site keeps its kind with `site: null`; nothing tells a script to skip it |
| astra (G24 computed population) | §4.1: the count is computed from G3's direct callers that the callers' context analyzed |
| astra (G33 estimate) | §4.4, §6: one candidate overall by default (the measured ~3.4 KB option), counts by context kept |
| astra (§1: c08 gate before W1; `usage-error`) | §1: both added |
| astra (register the new words) | §5: `relation not recorded`, `relation not established` |

### R2.3 Scope creep (Fable) and owner decisions

- **G4's table- and page-trigger rows:** moved to W3 (§2.4), designed with the existing
  dispatch rules (astra 4).
- **Cost fallback (b), the dominance prune:** cut from W2. If the measured cost misses the
  targets after (a), the prune becomes its own design item, with the dominance proof astra asks
  for, reviewed before use.
- **`opFamily` values `lock`, `validate`, `filter`:** cut. Only `write` ships (§3.2).
- **Owner decisions (2026-11-07):** recorded in §5 (c08, c25, c02, c20, new words).

### R2.4 W2 as built: cost result and departures (for review before landing)

**Cost, measured** (release-fast, the up walk alone on CDO, median of 3; before is the commit
that only adds the cost fields, after is the W2 binary):

| Call | States | Walk time | Scratch | Visitor | Peak RSS |
|---|---|---|---|---|---|
| c12 | 70,466 → 137,937 (1.96×) | 52.0 → 134.4 ms (2.59×) | 1.13 → 2.12 MB | 1.33 → 3.17 MB | 2.48 → 2.48 GB |
| c20 | 38,772 → 100,992 (2.60×) | 33.2 → 108.7 ms (3.27×) | 0.87 → 2.07 MB | 1.33 → 3.17 MB | 2.44 → 2.48 GB |
| n09 | 38,757 → 100,977 (2.61×) | 30.8 → 107.9 ms (3.51×) | 0.87 → 2.07 MB | 1.33 → 3.17 MB | 2.46 → 2.46 GB |

The states target (≤ 2×) holds on c12 only; the time target (≤ 1.5×) is missed on all three.
Two per-step costs were already removed (an allocation per step and a name scan). By §2.1, W2
stops here.

**Proposal (to decide):**
- **(A, recommended) Accept the measured cost; pruning stays a later item.** The up walk is
  under 0.14 s on calls of about 10 s (≈1.3%); peak RSS does not move; scratch plus visitor
  stay under 6 MB. The relative targets were a guard against state explosion. The states grow
  2–2.6×, which is bounded by the 8 `ext` values and is far from explosion. The pruning design,
  with its dominance proof, becomes a register item. It is built only if a later measurement
  shows the walk matters (for example, a multi-routine impact over a hub).
- **(B) Write and review the pruning design now**, and land W2 only with it.

**Departures from §2.1 / §2.2 as written:**
1. **Hubs in the `LOCAL` test.** A callbacks hub (`h:callbacks:<app>`) does own an app, so the test
   skips every `HUB` node, not only nodes with no app. When a dependency is loaded as a source
   folder, R0 treats it as local, so `LOCAL == !is_dependency_node` holds for the package form
   only. The test pins that form and checks that the source-folder form differs.
2. **Page-action identity.** The facts now record each `RunObject` action's name, and the sited
   graph keeps the names per action-run site. That is what lets "a page action with both a
   trigger and a `RunObject` counts once" hold. An action-run site whose actions are not named
   counts as one entry for its page.
3. **No `roots.config` overlay on either side.** Impact walks without extra entries today, so
   overlay entries are in neither the numerator nor the denominator.
4. **"(every candidate decided)"** prints only when there are no candidates. Otherwise the line
   says "(the candidates below are not decided)". The UI-navigation line is left out when its
   population is empty.
5. **n09's "raised by … Customer deletes at <k> sites" clause is not built.** The existing
   `raised by:` line stays, now tagged `[tier, code]`. The §2.2 expectation was wrong on the
   source: the subscriber exits when `RunTrigger` is false. Within 2 hops of the publisher there
   is no routine, only the Customer pages, which delete by navigation. Code: 1 ancestor (the
   publisher). UI navigation: 14,528 (25%).
6. **c12 is not program-wide.** UI navigation reaches 25% of the program's entries (11,231 of
   43,793). That is below the 50% threshold, so neither `program-wide` nor `broad reach` fires
   on these calls. The thresholds stay as written.

A check on real output: in every walk the population totals add up to the old ancestor total
(c12: 0 + 14,527 + 2,016 + 5,654 = 22,197).

#### R2.4 review result (2026-11-07): both reviewers "accept with changes", option A

**Cost: A.** Pruning is not a landing condition. Corrections to the reasoning above:
- The walks alone take 108–134 ms. The whole up part, with summaries and captions (`walkMs`),
  rises from 49–70 ms to 199–210 ms. Both numbers stay visible.
- Time grows faster than states (c20 3.27× against 2.60×; n09 3.51× against 2.61×): the work per
  state rose 25–35% (`site_bits`, `Population::of`, `canon.add`). Noted; no fix.
- Peak RSS is roughly stable, not unchanged (c20 +40 MiB).
- **The relative targets are replaced by an absolute bar:** the up part (`walkMs`) at most
  250 ms per changed routine on CDO c12, release-fast.
- **Register item, pruning (with its dominance proof):** triggered by a measured impact on many
  changed routines (or over a hub) whose up part exceeds 5% of the call. A multi-root
  performance check is part of that item. The state budget and partial-result guards stay.

**Departure 5, reworded reason:** the RunTrigger guard is real (`CDODataDeleteHandler.Codeunit.al:137`),
but it is not why there are no delete sites. Fable scanned the Base Application (8,073 files)
and Business Foundation: no typed `Customer` `Delete`/`DeleteAll` site and no `Rec.Delete` in a
Customer-sourced object. In the analyzed production program the only Customer deletes are page
deletions (UI navigation). The CDO test app does delete Customers (`Delete(true)`), which is
landing change 3.

**Landing changes (both reviewers' required items):**
1. **A background start counts once.** A background-target object and the `OnRun` it starts
   are one entry, not two. Merge the identities when the target routine is known; otherwise
   show an unresolved target separately. Update the inventory and share tests.
2. **The unnamed page-action fallback is explicit.** Count the CDO action-run sites that fall
   back to "one entry per page". When an answer's totals include such groups, the JSON carries
   their count and the human line says so. They are not silently folded into canonical totals.
3. **The scope line says what is not counted.** In human and JSON scope, once per answer:
   test code is not in these callers (it is reported under `tests:`); the `roots.config` entry
   overlay is not applied; `local` includes dependencies loaded from source folders when there
   are any. `direct code callers: none found` carries the test-code qualifier.
4. **The "event publishers among its ancestors" human block** is either rendered from
   classified states (with `[tier, relation]`) or dropped. The direct `raised by:` line stays.
5. **One program total in JSON.** `programEntries` becomes the canonical count (the same one as
   `programShare.program`); the legacy mask count is renamed or dropped. Consumers are ours.
6. **c12 prints `the changed routine is a page action (an entry itself)`** as §2.1's final form
   says (today only the JSON `rootEntryKinds` carries it).
7. **The stale `population` label** (W1's interim "all ancestors (…)") is dropped or kept only on
   the legacy `ancestors` object, with that meaning stated.
8. **Wording:** `reached by code: none — no established code-path witness in the analyzed graph`.
9. **Later (W4/W5, not W2):** n09 states its publisher three times; the publisher line uses
   `(through a dependency, 1 hop)` where other lines use `[via-dependency, code]`.

---

## 0. Summary

Both reviewers said **not yet** on the round-2 outputs. They agree the round is a large step
(60-plus round-1 findings resolved) and agree on one misleading defect: **impact prints a broad
reach under tier words with no relation and with configuration filed as dependency** (G1). The
near-identical per-kind counts in c12, c20 and n09 suggest the reverse walk reaches about the
whole program from three unrelated routines; §2.1 measures that share rather than assuming it.

The other misleading findings:
- Fable: n02's tests headline presents 37 UI-navigation-only tests as "run first" (G2); explain's
  `callers:` leaves out direct page-action callers (G3); `callers: none` on a field trigger (G4).
  astra saw G2 and G4 as missing information, not misleading; the design is the same either way.
- astra: c08's `analysis: complete` against its own family blockers (G5) and `outcome:
  not-found` for an ambiguous selector (G6). Both are on `fix/r2-counts`.

**Counts** (Appendix A): 62 findings; 45 design changes; 5 handled elsewhere; 12 rejected.
Of the design changes, 4 are misleading, 19 missing, 8 noise, 7 size, 7 consistency.

### 0.1 Milestones

Each lands on its own. Later goldens are rebased on earlier ones (regeneration, not rework).

| # | Milestone | Findings | Size | Depends on |
|---|---|---|---|---|
| W1 | **What misleads, output only**: impact's interim labels and program share, tests relation headline, explain caller counts, `run by:` for field triggers, actions and subscribers | G1 (interim), G2, G3, G4 (W1 rows), G15 | M | `fix/r2-counts` (with the c08 gate, §1) and `fix/r2-temp-events` merged |
| W2 | **Impact relation in the walk** (engine): code vs UI navigation, the via-configuration tier, canonical entries | G1 (engine and final output) | L | W1 |
| W3 | **Missing evidence** | G4 (W3 rows), G7–G14, G16–G23, G60, G62 | L | W1 (G7 shares impact's formatter) |
| W4 | **Consistency** | G39–G45 | M | — |
| W5 | **Noise and size** | G24–G38 | M | W2 (G37's whole-change block uses the final populations), W3, W4 |

W1 first: it is the only milestone that removes a misleading output. W2 before W5. W5 last:
nothing in it misleads.

---

## 1. Handled elsewhere

These are correctness bugs, not design. Each is fixed on a branch, with its own tests. Round 3
checks each on the call that raised it.

| Branch | What it fixes | Calls | Register |
|---|---|---|---|
| `fix/r2-temp-events` | paths through a temporary record's table event into a subscriber that exits on `IsTemporary`; c02's RecordRef "hub" that is a fixed table (`UpgradeBaseApp.Codeunit.al:2100`) | c12, c20, n09, n10, c02 | fact checks |
| `fix/r2-counts` | first-codeunit-OnRun witness from the merged bucket; c20's "run first … codeunits 69000 — a superset … for the 678 tests"; c11's "1 dependency routine entered" beside three; n09's "raised by 368" (publishers among ancestors, not raisers); n09's `analysis complete` against explain's `partial`; n07's "dependency code: none reached" beside dependency calls; n08's `rootsRequested: 0`; the ambiguous `outcome` (c26, n08); c06's caller-rule check depth; c08's `analysis: complete` against family blockers | c06, c08, c11, c12, c20, c26, n07, n08, n09 | G5, G6 |
| `fix/r2-inventory` | named return values not inventoried (c30); Temp Blob streams counted as FILE (c25, n10) | c25, c30, n10 | fact checks |
| r1 §10 (engine, deferred) | deep reads and locks in query effects; declaration-level field consumers for a rename; callers' held locks and lock order | c08, c11, c22, n10 | G46, G47, G48 |

What this spec needs from them:
- **G5** makes `analysis` reflect the families' statuses. astra's point stands: an analysis gap
  (a call not resolved) makes it partial; the advertised inventory depth (reads one dependency
  routine deep, by design) is a limit, not a gap. **The c08 regression gate** (c08's JSON says
  `analysis: partial` with its reasons) must be green before W1 merges. G14 builds on it.
- **G6** derives `outcome` from the resolution diagnostics. Its test covers each value
  (`answered`, `not-found`, `ambiguous`, `no-routine-changed`, `input-not-mapped`,
  `usage-error`): Fable's condition for accepting the departure, kept as the test.

---

## 2. Misleading findings (W1, W2)

### 2.1 G1: impact's reach by relation and tier, and its share of the program

**Raised by both** (Fable B1, cross-tool 1–2; astra M2, cross-tool 1). Calls c12, c20, n09.

**Current** (c12, a page action with no code caller):
```
reached by code callers: no — no code caller found in the analyzed graph (every candidate decided)
reached beyond it: 22,378 ancestors — by tier: local 0, via-dependency 16,749, via-hub 5,629
  entries among them, by kind (counted per legacy bucket: …): page actions 5,777 (via-hub 996), codeunit OnRun 447 (via-hub 132), …
  first page action: [local] PageExtension CDOSalesInvoiceExt.action(CDOOpenEMail)::OnAction (0 hops)
external: apps outside the program may call routines of its cone (by reason, routines): friend 98, public 506
```
The defect (astra's statement): **the tier words are right as tiers.** What is missing is the
relation axis, and configured dispatch is filed under `via-dependency`. Prove, touches and tests
populations are code paths, so an agent who has only seen those reads 16,749 code callers.
Also:
- The `entries among them` lines of c12, c20 and n09 are nearly identical (page actions 5,777 in
  all three; the ancestor tiers differ slightly). Equal counts do not prove one set; they are why
  the share of program entries is measured and printed.
- The changed routine is listed as its own first entry (`[local] … (0 hops)`) while the tier
  line says `local 0` (fact check c12 #6, n09 #8). `external: friend 98, public 506` repeats in
  all three calls.

#### What the walk records today (checked in the source)

- R0's reverse walk (`src/program/impact_graph/reverse.rs`) keeps one state per
  `(node, tier, crossedEvent, ext)`. `Tier` is `Local` / `ThroughDependency` / `OnlyOpen`;
  `Local` means `flag::LOCAL` (the workspace app, or an app loaded from local source in a test
  universe). `ext` is a byte of consumer bits set by `ConeVisitor::step`. Ext-0 states live in a
  dense per-node byte (`WalkScratch::seen`); any other state goes to the `seen_ext` hash set.
- `project.rs` maps each edge's `via` to a `Boundary`: `call`/`interface` → `Sync`; `run` of a
  trigger → `TriggerOf`; `run` of an event and `eventFlow` → `EventDispatch`; `run` and
  `declaredRunnable` → `Run`; `implicitTrigger`/`tableEvent` → `TriggerOf`; `backgroundStart` →
  `Background`; `lifecycle`/`clientEvent` → `Lifecycle`; `testRunner`, `testHandler`; anything
  else → `Open`. A site also carries a conditions mask; `configuredDispatch` labels Base
  Application's workflow-response arms (`configured_dispatch.rs`). R0 ignores conditions.
- `is_code_call_site` (`reverse.rs`) is true for `Sync`/`Run`/`TriggerOf` with via `call`,
  `interface`, `run`, `implicitTrigger` or `tableEvent`; for `EventDispatch` with via
  `eventFlow`; and for a `Lifecycle` site from an object node into a trigger the run runs
  (`object_run_reaches`: `OnOpenPage`, `OnRun`, …). It is **false** for `declaredRunnable`,
  every other lifecycle site (actions, field triggers, parts, opened pages, the source table's
  triggers), `Background`, `TestHandler`, `TestRunner` and `Open`. So its complement is not "UI
  navigation" (Fable 2, astra 2).
- `dep_callers.rs` uses two ext bits (`DEP`, `CONFIGURED`); its `DEP` is **app identity**
  (`node_app != primary_app`), not R0's `flag::LOCAL`.
- `Up` (`src/engine/impact/up.rs`) **implements `step`**: it records the action-run site per
  page (`action_site`, used by the captions) and returns `from.ext` unchanged. It keeps per node
  the best `(Tier, hops)` and per legacy entry bucket a `Firsts`.
- `programEntries` already exists in impact-up JSON: the count of nodes whose root-kind mask
  has any entry kind (`EntryKind::of_root_kinds`, public procedures excluded). The render's fold
  share divides `entryBuckets` (a sum over overlapping buckets) by it: a bucket sum over a
  distinct count.
- `root_kind_mask` gives overlapping kinds: an action trigger is `trigger-page` **and**
  `page-action`. Action-run page entries (a page made an entry by another page's action
  `RunObject`), static background targets (`Boundary::Background`) and roots.config overlays
  (`WalkOptions::extra_entries`) are reported by R0 outside that mask.

#### Engine change (W2)

1. **Site classes.** Impact's `step` keeps recording `action_site` and also sets bits on
   `ext`, from this table:

   | Site (boundary, via) | Class | Bit |
   |---|---|---|
   | `Sync` (`call`, `interface`); `Run` (`run`); `TriggerOf` (`run`, `implicitTrigger`, `tableEvent`) | code | none |
   | `EventDispatch` (`eventFlow`, and `run` of an event) | code (a deliberate superset of `is_code_call_site`, which accepts `eventFlow` only: running a publisher is code; `caller_agreement_tests` pins the solve-graph relation, not this class) | none |
   | `Lifecycle` from an object node into a trigger the run runs (`object_run_reaches`) | code | none |
   | `Background` (a static start) | code (the context axis labels it) | none |
   | `TestRunner`, `TestHandler` (test universes only; a handler runs because the test's code opened a page) | code | none |
   | any other `Lifecycle` (`lifecycle`, `clientEvent`): actions, field triggers, parts, opened pages, the source table's triggers; and an action-run site | UI navigation | `NAV` |
   | `Run` (`declaredRunnable`) at a call site: a dispatching builtin on a receiver typed as one object (`ReportVar.SaveAsPdf()`, `project.rs` rule 2; the site has the call's anchor) | code (the builtin runs the object) | none |
   | `Run` (`declaredRunnable`) from a declaration: a routine or object declaring a `TestPage`/`TestRequestPage` variable (test universes only; no call-site anchor) | candidate: relation not established | `RULE` |
   | `Open` (into or out of a hub) | candidate (tier `via-hub`) | none; earlier bits are kept |
   | any site with the `configuredDispatch` condition | as its row, plus | `CONFIGURED` |

   Bits are only set, never cleared, so the weakest edge decides (r1 §8.1), and a known
   navigation step stays known when the path later crosses a hub (astra 2). R0 does not change.
   **The known-false edges stay gone** (astra): a plain declaration of a `Page`, `Report`,
   `XmlPort` or `Query` variable makes no edge since `fix/reach-and-test-selection`
   (`impact_graph/mod.rs`: "a plain declaration of those four kinds runs nothing"), and r1 §8.1
   says no output names one as a route. Keeping rule edges as candidates applies only to the two
   `declaredRunnable` rules above. The two rows are told apart by the call-site anchor; a test
   pins both (a `SaveAsPdf` fixture is code, a `TestPage` declaration fixture is `RULE`) and a
   third pins that a plain `Page` variable declaration yields no site.
   **Changed 2026-11-14 (TLS-51):** the call-site row is gone. Since ENG-19 the resolver makes
   every dispatching builtin on a typed receiver a `run` route (an `EngineRun` site, code), so
   `project.rs` rule 2 found nothing and was removed. Every `declaredRunnable` site is now a
   declaration and classifies as `RULE`; the `SaveAsPdf` fixture checks a `run` site instead.
2. **One class per node, with a total order** (Fable 3, astra). The class of a state is
   *(evidence, relation, tier)*:
   - **tier** comes from R0's `Tier`, read as it is: `OnlyOpen` → `via-hub`; else `CONFIGURED`
     → `via-configuration`; else `ThroughDependency` → `via-dependency`; else `local`. Impact
     sets **no `DEP` bit** and tracks no app identity of its own. A test pins, per **app-owned**
     node of the production universe (`node_app(n).is_some()`), `flag::LOCAL ==
     !is_dependency_node` (`dep_callers.rs`), so R0's tier and app identity agree there. Hubs own
     no app (`sited.rs` gives them no `LOCAL` flag, and `is_dependency_node` is false for them),
     so they are outside that equality and are tested apart: a path over a hub is `via-hub`;
   - **relation**: `code` (no `NAV`, no `RULE`, not over a hub), `UI navigation` (`NAV`, no
     `RULE`, not over a hub), or **not established** (`RULE`, or over a hub; a path that also
     needs navigation says so);
   - **evidence**: **established** only when the tier is `local` or `via-dependency` and the
     relation is known. Every other state is a **candidate**: over configuration (tier
     `via-configuration`; r1 §8.1: "a `via-configuration` witness is always a candidate"), over
     a rule edge, or over a hub. A candidate over configuration keeps its known relation
     (`candidate over configuration, code`) and says the evidence depends on configuration
     data the analysis has not read;
   - **order**: established `code` > established `UI navigation` > candidate over configuration
     (`code` before `UI navigation`) > candidate over a rule edge > candidate over a hub
     (configuration can be decided by reading data; a hub cannot, r1 §8.1). Within a step the
     better tier, then fewer hops. A node takes the best state that reached it, and **its witness
     is that state's** (astra: the shown witness and the class come from one state). Candidate
     counts include every candidate class, configuration included.
3. **Canonical entries** (astra 3). One entry identity, used by numerator and denominator:
   - a routine node, with the `#if` arms of one stable id as one entry;
   - one kind per entry, by a fixed priority over its overlapping root kinds (`page action`
     before `page trigger`; r1 §8.1's disjoint kinds);
   - a page action is one entry whether it runs through its `OnAction` trigger or through
     `RunObject` (the action-run site); an action with both counts once;
   - static background targets and roots.config overlay kinds are entries in both counts (the
     same `extra_entries` map and `Background` sites);
   - public procedures are not entries here: impact reports them under `external:` (code
     population only), so r1 §8.1's `public procedure` kind appears there, not in the shares.
   The changed routine is not counted as an entry it reaches.
4. **Program entry inventory**: the canonical count per kind over the whole sited graph, once per
   graph, kept with it. One pass over the nodes and the action-run and background sites.
5. **JSON** (`al-sem.impact-up`, additive, r1 §8.3): `byRelation: {code: {local, viaDependency},
   uiNavigation: {local, viaDependency}, candidates: {overConfiguration: {code, uiNavigation},
   overRule, overHub}}` for ancestors and for
   entries; `entriesByKind` (canonical, disjoint); `programEntriesByKind` (new, beside
   `programEntries`, which stays a number); `programShare` per population; `relationRecorded:
   true`. The legacy `ancestors` tiers and bucket counts stay until the next impact version. The
   impact base cache key (`cache.rs`, `ENTRY_VERSION`) is bumped.
6. **Limited walks** (a `Limit` was reported): no `program-wide` line, no absence verdict; the
   population lines say `analysis partial (limit: …)`.

**Cost** (measurement targets, not estimates; astra 3). The table sets **three** bits (`NAV`,
`RULE`, `CONFIGURED`), so `ext` takes 8 values: with 3 tiers and `crossedEvent` that is
**48 state slots per node** (today 6). Most nodes should get one or two classes, which is what is
measured.
- **(a) Dense storage, part of the design** (Fable): `WalkScratch::seen` becomes a `u64` per node
  (today `u8`); a state with `ext` 0–7 takes bit `(ext * 3 + tier) * 2 + crossedEvent`, 48 of
  64 used. On c12 nearly every ancestor carries `NAV`; without this every one would go to the
  `seen_ext` hash set. **The explicit hash fallback stays** for `ext` values of 8 and above,
  which other consumers' bits can reach (callers' context sets its own); impact and
  `dep_callers` (bits 1 and 2) stay in the dense array. This is an R0 commit of its own; the r0
  goldens must not move.
- **Memory:** the dense array is 8 bytes per graph node per scratch instead of 1 (8×; one scratch
  per walking thread), whatever the states; `Up` adds one class byte per node it already keeps
  (`best`), and keeps per class at most the witnesses it keeps today. States themselves (the
  level queues) grow with the classes actually reached, bounded by the state-count target.
- **Targets**, on `release-fast`, before and after, on c12, c20 and n09: up-walk states
  (`Up.states`) at most 2×; up-walk phase time at most 1.5× (the phase alone: c12's 10.2 s call
  is mostly effects); peak scratch plus visitor memory measured and reported against the
  figures above.
- If the targets are missed after (a), W2 stops and a pruning design (with its dominance proof
  over every state dimension and witness tie-break) goes to review first. Pruning is not in W2.

#### Output (W2, the final form)

c12 (`<…>` are computed; the witness line is illustrative):
```
calls: Codeunit "CDO Document E-Mail Management"::SalesInvHeaderOpenEMail (:51)
the changed routine is a page action (an entry itself)
reached by code: none — no code path reaches it in the analyzed graph (every candidate decided)
reached by UI navigation: <a> ancestors — via-dependency <a1>, local <a2>; program-wide: <p>% of the program's entries (<n> of <N>); counts by kind folded (detail: true prints them)
  nearest: [via-dependency, UI navigation] Page "Sales Invoice" (43) opens it (<h> hops)
candidates over configuration (depends on configuration data not read): <g> ancestors (code <g1>, UI navigation <g2>)
candidates (relation not established): over a rule edge <r>, over a hub <b> ancestors — hubs: [hub Page.Run of a variable at dep:Base Application:src/AdvancedSettingsExtImpl.Codeunit.al:22], … (format: "json" lists them)
```
n09 (a table-event subscriber):
```
direct code callers: none found
reached by code: <c> ancestors — raised by Table Customer (18)::OnAfterDeleteEvent: Customer deletes at <k> sites, nearest [via-dependency, code] …
reached by UI navigation: … program-wide: …
```
When a subscriber also has direct code callers, the first line lists them and the code
population counts both ways in once:
```
direct code callers: 2 (Codeunit "…"::…, Codeunit "…"::…)
reached by code: <c> ancestors, through its direct callers and its publisher together — raised by Table Customer (18)::OnAfterDeleteEvent; nearest [local, code] Codeunit "…"::… (1 hop)
```

Rules:
- **Every population line names its relation and its tier.** A population whose relation is not
  recorded never carries a bracketed tier.
- **Code first**, with its nearest witness, whatever its size (r1 §3.2).
- **Program-wide.** A population whose canonical entries are at least 90% of the program's prints
  `program-wide: <p>% of the program's entries (<n> of <N>)` (**new word**) and folds its
  per-kind counts behind `detail: true`. When no single population reaches 90% but their union
  does, one line says so: `together these populations reach <p>% of the program's entries`.
  From 50% up to 90%, r1 §3.2's fold line prints. **Why 90% and not equality:** the changed
  routine is excluded and the counts move with every correctness fix, so equality would never
  fire. Both thresholds are measured on CDO (c12, c20 both routines, n09, two narrow routines)
  before they are fixed. The line states the share and nothing else, never "not specific"
  (r1 §11.2 row 6).
- **`external:`** prints only for the code population.
- **The hubs line prints once per answer.**
- **Canonical kinds** replace the legacy buckets; the `(counted per legacy bucket …)` note goes.

#### Interim output (W1: truthful without the engine change)

Human:
```
direct code callers: none found
reached beyond it: 22,378 ancestors by code calls, UI navigation, configuration or a hub (relation not recorded by this walk): within the workspace 0, through a dependency 16,749, over a hub 5,629
  these ancestors hold <p>% of the program's declared entries (<n> of <N>; pages run by an action's RunObject, background targets and roots.config entries are counted on neither side); counts by kind folded (detail: true prints them)
  hubs on these paths: [hub …], … +7 (format: "json" lists them)
```
Rules:
- The population names all four relations: a subscriber's ancestors include code paths through
  its publisher (n09), so "UI navigation or a hub" would be false.
- `direct code callers: none found` is the computed result (`select::direct_callers`), never a
  claim from the routine kind (astra 1: a subscriber can also be called directly). A subscriber
  prints its publishers on the `raised by:` line as today.
- No bracketed tier on this population; plain words say where paths go (`within the workspace`,
  `through a dependency`, `over a hub`). `first …` items move behind `detail: true`, each tagged
  `[relation not recorded]`.
- **One eligibility predicate for both sides** (astra 3, Fable): a node counts when its root-kind
  mask has an entry kind, `!EntryKind::of_root_kinds(g.root_kinds(n)).is_empty()`, the test
  today's `programEntries` already uses. `<N>` is the program's nodes passing it, minus the
  changed routines that pass it; `<n>` is the reached nodes passing it (from `Up::reach`, not
  from `Up.entries`, which also holds R0's out-of-mask entries: action-run pages,
  `extra_entries`, background targets), the changed routines excluded. So `<n> ≤ <N>` by
  construction, and the line names what neither side counts. Never the bucket sum. A test pins
  `<n> ≤ <N>` on a fixture with an action-run page and a background target (break: take `<n>`
  from `Up.entries`). The fold line between 50% and 90% says `broad reach (<p>% of the program's
  declared entries; list folded)`, not "broad navigation reach": W1 records no relation. The
  ≥90% line says `these ancestors hold …` (above), not `program-wide`; W2's canonical identity
  replaces this predicate and adds the out-of-mask entries to both sides.
- `external:` is not printed in human text (astra 1); JSON keeps it with the population field.
- **JSON and the envelope summary say the same** (astra 1): impact-up gains
  `relationRecorded: false` and `population: "all ancestors (code, UI navigation, configuration,
  hub)"`; the envelope summary's routine rows carry `relationRecorded: false`.
- The changed routine is not listed as its own entry; the hubs line prints once.

**Files:** `src/engine/impact/up.rs` (`step` bits, class per node, canonical entries, shares),
`src/program/impact_graph/reverse.rs` (`seen` widened), `src/program/impact_graph/sited.rs`
(program inventory), `src/engine/impact/render.rs` (`reached_clause`, fold share, whole-change
block), `src/engine/impact/cache.rs`.
**Tests** (one discrimination proof each, except the last):
- a fixture page action reached only by a page that opens it: `reached by code: none`; it is in
  the navigation population (break: do not set `NAV`);
- a fixture subscriber whose publisher is raised by a code call: code population (break: classify
  `EventDispatch` as navigation);
- a mixed path: code call, then a page opened by navigation, then a hub: candidate that needs
  navigation; and a node reached both by `(code, via-hub)` and `(UI navigation, local)` takes
  `UI navigation, local`, with that state's witness (break: rank by tier before relation);
- a background start on the path stays code (break: set `NAV` on `Background`);
- the configured-dispatch fixture (`configured_dispatch_tests.rs`): a candidate over
  configuration with relation `code`, counted among candidates and not in established code; and
  a node reached by `(code, via-configuration)` and `(UI navigation, local)` takes the
  established `UI navigation, local` (break: treat a configured state as established);
- the two `declaredRunnable` rules: a `SaveAsPdf` call site is code, a `TestPage` declaration is
  `RULE`, and a plain `Page` variable declaration yields no site (break: classify every
  `declaredRunnable` site as `RULE`; since TLS-51 that is the production rule, and the
  break is to classify a `declaredRunnable` site as code);
- per app-owned node of the production fixture, `flag::LOCAL == !is_dependency_node` (break:
  mark a dependency node `LOCAL`); and every hub node classifies a path through it as
  `via-hub` (break: let a hub take the `local` tier);
- canonical entries: an action with an `OnAction` trigger and a `RunObject` counts once, in
  numerator and denominator (break: count both);
- program-wide: a root reaching every entry prints the share and folds the counts; a narrow root
  does not; a limited walk prints neither (break: compare against the reached set);
- the root is not its own entry (break: count it);
- interim (W1): no bracketed tier on the unclassified population, `relationRecorded: false` in
  JSON: a golden only (wording; r1 §12.1).

### 2.2 G2: tests on a trigger say when every witness is UI navigation

**Raised by** Fable (B2, misleading) and astra (n02, missing). Call n02.

**Current:**
```
answer: run first (app "Continia Document Output 365 - Test"): codeunits 68984|68850|68989|68911 — a superset: whole codeunits, for the 37 selected test procedures on a local path; then the whole suite (…)
  Codeunit 68984 CDO Variant Setup UI Tests: 30 tests; first [local] 3 hops (…; via Page "CDO E-Mail Template Card" → Page "CDO E-MailTemplateMergeFields" → Table "CDO E-Mail Template MergeField"."Get Field From"::OnValidate); this witness requires UI navigation; invocation of the action is not established
```

**New:**
```
answer: no modeled code-call witness in the selected scope; 37 tests reach this trigger by UI navigation only (each opens a page on the field's table; whether the page shows or edits the field is not established) — candidates: codeunits 68984|68850|68989|68911 (a superset: whole codeunits); then the whole suite (selection incomplete: unresolved-test-calls)
tests on a local path: 37 tests in 4 codeunits — by relation: code 0, UI navigation 37
  Codeunit 68984 CDO Variant Setup UI Tests: 30 tests; first [local, UI navigation] 3 hops (…)
```
Rules:
- The headline states the split by relation whenever any selected test is navigation-only. When
  some reach by code: `run first: <n> tests by code calls (codeunits …); <m> more by UI
  navigation only (codeunits …)`, code first (tests already rank so).
- "In the selected scope" (astra 4): the selection is incomplete and the tier asked may exclude
  dependency paths, so the negative is scoped.
- Tests mix relations, so every item prints both axes: `[local, UI navigation]`,
  `[via-dependency, code]` (§5, §6).
- The caveat prints once, in the headline, naming the user step the target needs, from its
  routine kind: a field trigger "edits the field" (`OnValidate`), "looks up the field"
  (`OnLookup`), "assist-edits the field" (`OnAssistEdit`); an action "invokes the action". The
  page is described as on the field's table, never as showing the field (Fable 1).
- JSON (`al-sem.tests/2`, additive): `selection.byRelation: {"code": 0, "UI navigation": 37}`.
  `runFirst[].relation` exists. `runFirstLine` uses the same words.
- Tests keep their own relation rule (`render.rs` `relation()`); W2 does not change the tests
  walk (astra 2).

**Files:** `src/engine/test_select/render.rs`.
**Tests:** a fixture field trigger reached only through a page on its table: the headline says
`no modeled code-call witness in the selected scope` and the item tag is `[local, UI
navigation]` (break: drop the relation from the headline). One proof.

### 2.3 G3: `callers:` counts direct page-action callers

**Raised by** Fable (B3, misleading, low); fact check c01-c10 #1, n01-n10 #9. Calls c06, n10.

**Current** c06: `callers: 13, reached from 118 page actions`, while the same card's
caller-rule line counts 43 (`checked by 2 callers … 41 analyzed callers`). Explain moves a direct
`OnAction` caller out of the callers (`is_action_trigger`, `explain.rs`) into `reached_from`,
which holds every page action whose trigger reaches the routine over resolved calls, directly or
not. n10: `callers: 30` where the graph has 71 direct callers.

**New:**
```
callers and tests: callers: 43 direct (13 routines and page triggers, 30 page actions); 118 page actions reach it in all; tests: …
callers: 43 direct — 13 routines and page triggers: Page "CDO Vendor Remittance Advice"::HasRecipients, …; 30 page actions in 30 objects (detail: true lists them); 118 page actions reach it in all (directly or through callers)
```
n10: `callers: 71 direct (30 table-extension procedures, 41 page actions); 115 page actions reach it in all`.
Rules: "callers" is one set on every line of a card: the direct callers, the caller-rule line's
set. A direct page action is a caller and one of the actions that reach it. JSON (additive):
`callers.direct`, `callers.directPageActions`; `reachedFromSummary.actions` stays the "in all"
count.
**Files:** `src/engine/l5/explain.rs`, `routine_rollup.rs`. **Tests:** a fixture routine called
directly by two procedures and one action trigger, and by one action indirectly: `callers: 3
direct (…, 1 page action); 2 page actions reach it in all`; the caller-rule count is 3 (break:
exclude action triggers again).

### 2.4 G4: a trigger says what runs it, not `callers: none`

**Raised by** Fable (B4, misleading, low) and astra (n07, missing). Calls n07, c10, c05.

**Current** n07: `callers: none` on `Table "CDO Template Line Report"."Background FP PDF File
Name"::OnValidate`.

**New** (on a trigger, `callers:` becomes `run by:`, **new word**):
```
run by: Validate calls on the field: none observed (searched: 5,479 workspace bodies, 28,639 dependency bodies; not searched: dynamic field numbers); potential UI surfaces: 2 pages on table "CDO Template Line Report" (Page "…", Page "…"; whether a page shows or edits this field is not established); an assignment to the field does not run it
```

| Trigger | Code that runs it | What a user may do | Milestone |
|---|---|---|---|
| field `OnValidate` | `Validate(<field>)` sites (field-touches index, use kind `validate`) | potential UI surfaces: pages on the table | W1 |
| field `OnLookup`, `OnAssistEdit` | none in code | the same pages | W1 |
| page action `OnAction` | none in code | `entry point: this action (no code caller)` (also G26) | W1 |
| event subscriber | the publisher (`raised by:`), plus any direct code caller found | — | W1 |
| table `OnInsert` / `OnModify` / `OnDelete` / `OnRename` | the record operations whose sited `TriggerOf` site reaches this trigger (the existing dispatch rule, `RunTrigger` included; Rename by its own rule) | potential UI surfaces: pages on the table | W3 |
| page trigger | the runs whose object-run reaches this trigger (`object_run_reaches`: only the triggers a run runs); the other page triggers by their lifecycle sites | the pages and actions that open the page | W3 |

Rules: the pages are the sited graph's lifecycle sites from page objects into the table's
triggers, labelled **potential UI surfaces** (astra 4); page columns are not indexed (r1 §10).
Negatives use the §8.1 scoped words (`none observed (searched: …; not searched: …)`). In W1 a
table or page trigger prints `direct code callers: none found (what runs a trigger is not listed
for this kind yet)` instead of `callers: none`.
**Files:** `src/engine/l5/explain.rs`, `routine_rollup.rs`, `field_touches.rs`.
**Tests:** a fixture field with one `Validate` call and one page on its table: `run by: Validate
calls … 1 site; potential UI surfaces: 1 page` (break: print `callers: none` for a trigger
again). One proof; W3 adds one for a table trigger reached by a record operation.

---

## 3. Missing information (W3)

### 3.1 G7: impact names the tables, fields and company context (c20)

**Both.** Reopens r1 F56, F75. **Current:** `effects: may write 1 table (1 owned by this app, 0 by
dependencies)`. **New** (explain's formatter, r1 §9.1, reused; conditionality kept):
```
effects: may write CDO Setup (Modify at CDOSubscribers.Codeunit.al:<l>; the routine assigns "Azure Blob Folder", "Azure Blob Container Name", "Azure Blob Private Endpoint URL"); errors 1
  target company: not established by this analysis (ChangeCompany at CDOSubscribers.Codeunit.al:55)
```
n09 names its three tables the same way. **Files:** `impact/render.rs` reads explain's
tables-when-few, assignments and `change_company_sites` (`l5/explain.rs`), moved to one shared
function. **Tests:** goldens only.

### 3.2 G8: touches on a bare table represents each operation family, writes first

**Both** (n03; astra c18; Fable c30). **Current** n03 ("which routines write Customer"): twelve
samples, all `[Get]`. **New:** samples follow the families, each with its own count:
```
  record writes: 16 sites in <r> routines — Modify 13, Insert 1, ModifyAll 1, DeleteAll 1 (temporary records: <t>)
    [Modify] at … in …; [Insert] at … in …; [ModifyAll] …; [DeleteAll] …
  validations: 5 sites (a validation may lead to a write; it is not one)
  locks: none observed
  key-relevant (not exhaustive): Get 35 (primary key), SetCurrentKey 0, SetRange/SetFilter on key fields 0
  reads: …
```
Up to 12 samples, shared by family (record writes at most 6). Key-field SetRange/SetFilter sites
are sampled before Gets. c18 gets the same order. A new input `opFamily: "write"` (**new word**)
lists every record-write site; its members are Insert, Modify, ModifyAll, Delete, DeleteAll and
Rename; temporary-record sites are included and tagged with their temp state, counted apart in the
line. No other value ships (§R2.3). **Files:** `l4/effect_query_cli.rs`, `mcp/tools.rs`.
**Tests:** a fixture table with a Get, a Validate and a Modify: the Modify is sampled under
record writes and the Validate is not counted there (break: sort key-relevant first again).

### 3.3 G9: digest hands tests over to the tests tool (c25)

**Both.** Reopens r1 F77. Digest's engine does not load the sibling test app (c24 shows two
engines), so `testStatus: "not loaded"` is true but leaves the agent stuck. **New:** the
summary's `testScope` and the result index carry `next: {"tool": "alsem_tests", "arguments":
{"files": ["src/Features/Email/Sending/CDOSendMailManagement.Codeunit.al"]}}`; human digest
prints `tests: not analyzed here (test app "Continia Document Output 365 - Test" is not loaded in
this engine) — alsem_tests {"files": […]} selects them`. The selection stays the tests tool's
(r1 §9.5); astra accepts the handoff. **Files:** `l5/digest_cli.rs`, `result_index.rs`.

### 3.4 G10: digest summary rows fit, and keep what a decision needs (c25)

**Both.** Reopens r1 F42. **Current:** 6 of 20 rows fit (`summaryTruncated: rowsListed 6`):
five `SendMail` answers are ~400 characters each; `"shared": {}` is empty although six overloads
share most facts. **New:**
- A row carries `selector`, `analysis`, `testStatus`, `tableAnswers` and a short `answer`;
  `signature` and `stableId` move to the index row the summary row names (compact reference).
- The short `answer` is one clause per effect family, **with its qualifiers kept** (astra 7):
  `may write 9 tables (9 by dependencies; 2 only when the caller's record is not temporary);
  commits: may; UI 2; errors 5; HTTP 1; analysis partial (calls not resolved 3)`. The full
  sentence is in the index.
- Overload rows use the `shared` block c15 uses.
- The overflow contract of r1 §5.2 stays: `summaryTruncated` and the complete index whenever the
  rows do not fit.
- The index row keeps conditional persistence, uncertainty, table verdicts and test status (the
  owner's c25 condition, §5).
**Tests:** the r1 §12.1 overflow gate gains c25's shape: the encoded summary of 20 rows is
≤ 8 KiB, and with a wider fixture the summary truncates with `summaryTruncated` and the index
holds every row (break: drop `summaryTruncated`).

### 3.5 G11: a capped test selection spreads over the changed routines (c21)

Fable. **Current:** `maxTests: 10` returns ten tests that all reach one of 20 changed routines;
`runFirstLine` names 15 codeunits "for the 144 selected test procedures". **New:** under a cap,
`runFirst` takes, in changed-routine order (the `changed[]` order), each routine's nearest test
(code relation before navigation, then tier, hops, test display); a test already taken for an
earlier routine is skipped, so it counts once; then the remaining slots fill by the global rank.
`runFirstLine` describes the returned tests: `codeunits 68874|68914|68929 — for the 10 returned
tests (144 selected in all)`; totals, `selectionComplete` and the fallback stay about the full
selection. **Files:** `test_select/render.rs`. **Tests:** two changed routines, a cap of 2: one
test per routine; one test nearest to both counts once (break: rank globally). One proof.

### 3.6 G12: large inline JSON gets the summary too (c21)

astra (Fable accepted the departure; §6). **New:** an inline JSON answer over 8 KiB gets the same
envelope `summary` a file answer gets (for tests: `selectedTests`, totals, `selectionComplete`,
run target, fallback), with the same overflow accounting if the summary itself exceeds its
budget. No index file: the full result is inline. **Files:** `src/mcp/mod.rs`. **Tests:** MCP
surface contract test (c21's shape carries `summary`).

### 3.7 G13: a blocker is named by its site; `coverage incomplete` is not double-counted (c15, c19)

**Both.** Reopens r1 F42 (c15 part). **Current** c15: `"firstBlocker": {"detail": "coverage
status: partial", "kind": "coverage-incomplete"}` while `blockedBy[1]` is the real site
(`Provider.GetCache().GetSubscription … LicenseManagement.Codeunit.al:456`). c19: `blocked by:
interface dispatch (open world) 4, coverage incomplete 1`.
**New:**
- `coverage incomplete` is dropped from the blocker list and count **only when each of its
  reasons is a listed blocker** (astra); it then shows as the analysis line's reason. Otherwise it
  stays a blocker.
- `firstBlocker` is the first blocker with a site; when no blocker has one, the first blocker as
  it is (`site: null`).
- When 3 tables or fewer are observed written, the row names them: `"observedWrites": ["CDO E-Mail
  Log", "CDO Log Attachment"]`.
**Files:** `l5/digest_cli.rs`, `l5/prove.rs`. **Tests:** one open-world dispatch: `firstBlocker`
has its site and the count is 1; a coverage gap with an unlisted reason stays a blocker (break:
drop the meta-entry always). One proof.

### 3.8 G14: the limits line agrees with the body (c01, c11, c23)

Fable (c01, cross-tool 6); fact check c11 #1. Reopens r1 F58. **Current:** c11 and c23 print
`limits: none` and, in the body, `deeper dependency locks are not inventoried`. c01 prints
`limits: none` for an added `Modify()` and nothing says whether the table's `OnModify` trigger
and `OnAfterModifyEvent` subscribers were followed. **New:** the header's `limits:` collects every
limit the body states (`limits: locks one dependency routine deep`). A write line says what its
table events ran: `Modify at :24 runs: no OnModify trigger; table-event subscribers: none in the
loaded program (followed)`. Fable's "only when non-empty" is declined (G56). **Files:**
`engine/result_header.rs`, `l4/effect_query_cli.rs`, `l5/digest_cli.rs`. **Tests:** header
invariant: a body limit clause implies a non-empty `limits:` (break: hard-code `none`).

### 3.9 G15: impact prints the routine's direct callees (c12)

Fable. Reopens r1 F55 for impact. **New:** `calls: Codeunit "CDO Document E-Mail
Management"::SalesInvHeaderOpenEMail (:51)` from explain's `calls_line`. Lands in W1 with G1.

### 3.10 Smaller missing items

| id | Call | Current | New |
|---|---|---|---|
| G16 | n01 | `answer: yes`; the one pathed witness is a dependency activation-wizard commit | `answer: yes — COMMIT at CDOLegacyeDocDispatcher.Codeunit.al:229 (condition not resolved), and 10 more`; workspace sites listed before dependency ones; a conditional first witness prints its condition (both) |
| G17 | n04 | `[ERROR_IMPLICIT] at …:50` | `Get (implicit error) at …:50 (in a loop)`, c27's form; the routine's own declaration, when it is one: `declared TryFunction; interception depends on how the caller invokes it` (an error still propagates when the caller ignores the Boolean result; nothing is printed when it is not a TryFunction); `whether a caller traps it: alsem_prove {"routine": …, "question": "callers-intercept-errors"}` (both). The rollback claim is declined (G57) |
| G18 | c07 | `then the whole suite` | `then the whole suite: app "Continia Document Output 365 - Test", 1,844 tests`; `tier: "dependency" gives the 445 tests' codeunit filter` (both) |
| G19 | c04, c23 | c04 (`showSource: true`) ends after the header; c23 lacks the trigger's location and route | c04: `defined at …` and the excerpt of the root's first statements; c23: `defined at …:<l>` and `route: OnValidate → SetMergePDF → Modify (:467)` (astra; reopens r1 F48) |
| G20 | c02 | blockers by count; the unrecognized guard is mid-text | the blockers that carry an action print first (r1 §7.5): `to narrow it: the guard at CDOEMailTemplateManagement.Codeunit.al:1433 is not recognized; a roots.config.json would name the background targets` (both) |
| G21 | c18 | `table: not resolved (RecordRef)` | `table: not resolved (RecordRef RecRef: opened from a variable)`, r1 §8.1's target-resolution words (astra) |
| G22 | c30 | key-relevant uses without the key | `keys: PK "Code"; …` before the key-relevant line; `not searched: key and ordering uses in declarations (page SourceTableView, report DataItemTableView)` (astra) |
| G23 | c10 | the field answer, then a whole OnValidate card; `at …:97: no rule`; `(kinds: RecordRef.Field by number, TransferFields copies): 2`; `alsem_query_touches with table and field` | the field card only, with `field 60 declared at CDOSetup.Table.al:<l>`; the trigger as a follow-up call; per-kind counts (`RecordRef.Field by number 2, TransferFields copies 0`); `no rule` replaced by what was checked, or dropped (the implementer finds what it reports); complete calls for both follow-ups (both) |
| G60 | n07 | `tests: … not analyzed in this call — alsem_tests selects them` | the complete call: `alsem_tests {"routines": ["Table \"CDO Template Line Report\".\"Background FP PDF File Name\"::OnValidate"]}` (astra) |
| G62 | n05 | the zero-operation answer does not say its scope | the scope clause says `record operations only; not page-context restrictions or non-record effects` (astra) |

---

## 4. Noise, size and consistency (W4, W5)

### 4.1 G24: explain with `callersContext` leads with the callers (c06, n10)

**Both.** Reopens r1 F29, F47 (partly). **New** order: header; `answer:` summarizing the callers'
context (`pending writes: yes (8 witnesses); intercepted errors: unknown; non-interactive:
unknown`), not `writes: no`; own state changes; the three callers' answers, each saying what was
found among the analyzed callers, a computed population: the direct callers of G3 that the
callers' context analyzed (`pending writes: none found among <k> analyzed direct callers; unknown
because <m> routines other apps may call`); `locks held by callers: not analyzed (no lock
facts)`; the `callers:` line (G3); then the effects as summary lines only (`detail: true` for the
writes). Caption and "most actions" lines move behind `detail: true`. The pending-write witness
names its operation and table: `Modify on "CDO E-Mail Template Header" at <file:line> in
DocOutput.GetRecipientsDescription, before the call at …:264`. Target: n10 from 40 KB to ~5 KB.

### 4.2 G25: one copy of each line

**Both.** Remove the second copy of `stopped:` (c02, c29); `tests:` (c05, c06, c10, n07); c22's
`consumers:` (same as `answer:`); c22's `filters:` search clause (`filters: none observed (same
search)`); c14's `not checked: no engine is running`. c22's legend prints once before the
per-object list. `more: detail: true lists every list complete` becomes `more: detail: true lists
every list in full`.

### 4.3 Other noise items

| id | Calls | Change |
|---|---|---|
| G26 | c05 | the action's snippet prints once; `reached from page action <itself>` becomes `entry point: this action (no code caller)`; an excerpt says which line it shows (`first statement at :183`), since `defined at :178` was followed by lines 183–187 (fact check c05 #5) (both) |
| G27 | all human | the header's `scope:` keeps the clause that differs per tool; the dependency-write clause prints only when the cone enters dependency code (c04) (both) |
| G28 | c07, c21, c28, n02 | `test scope:` and the symbols paragraph become one clause: `selection incomplete: 4,702 unresolved test calls (7 test-library apps missing; format: "json" lists them and the fix)` (both). Once per session is declined (G58) |
| G29 | c11, c23 | the `answer:` line splits into `writes:`, `reads:`, `locks:` lines; c11's operation rows print before the deep-write summary (both) |
| G30 | c13 | `not computed here: events, capabilities and permissions` stays; `(the CLI's diff command gives them; it is not available over MCP)` goes; the hunk list folds to `25 files (detail: true)` (both) |
| G31 | c19, n01 | `top callees not resolved` prints only when it adds to the blocker list; c19 lists all four blocking calls instead of `+1`; interface calls count as `interface dispatch (open world)`, not "not resolved" (fact check n01 #3) (both) |

### 4.4 Size (decisions in §5)

| id | Call | Change |
|---|---|---|
| G32 | c28, n02 | tests human paths use prove's shared segments (`path_segments.rs`): c28's five paths share their last five hops; n02's share the navigation tail; a `by tier:` totals line first (both) |
| G33 | c02 | **one candidate overall** by default (counts by context stay; `detail: true` gives one per context); repeated member and file elided on consecutive hops; one `stopped:` line (~3.4 KB, the implementer's measured option) |
| G34 | c08 | tables for table id/name and `origin.file`; `qualifiedDisplay` derived; per-table summary rows folded (~480 KB); `summary.writes.byTable` complete when 20 tables or fewer (c08 has 13); `selector` stays on every row. Pretty JSON stays the default. Schema: §4.6 |
| G35 | c25 | hops as arrays; compact form as the digest file default (~290 KB), with `rowDetail: compact` in the envelope and `compact: false` still available; path prefixes shared only if the segment work gives them. Schema: §4.6 |
| G36 | c17 | each object's callers grouped by hop distance (~58 KB); the 3 callers whose first touch is not the shared one say so in one line; "the three are disjoint" names the three populations |
| G37 | c20 | the whole-change block prints counts only; the hubs line once; nearest entry per population per routine (~4.3 KB, before G7 adds the table names) |
| G38 | n05 (c08, n08) | JSON gains `dependencyCount` and `dependencyClosure` (additive); the full `dependencies.apps` list leaves only in each schema's next major (§4.6); `families` keeps its structured fields and drops the four constant sentences into the schema doc (both) |

### 4.5 Consistency

**G39: "writes" means a record write** (astra cross-tool 3; fact check c10 #10). Field-level
facts use **new words**: `field assignment` and `validation`. c10: `answer: the field is assigned
or validated by 1 routine` and `confirmed field writers (assignment or validation): 1`; the
record-level summary keeps `writes:`. An operation prints its own verb: c25's `may modify …
(Delete at :141)` becomes `may delete …`; mixed kinds `may change`. G8 applies the same split.

**G40: not-found and ambiguous answers** (both; reopens r1 F8). One failure sentence (not three:
c03, c26, n06); no assumptions or scope clause on a lookup miss; nearest names ranked by edit
distance (n06's `SendEMailAction` first); a complete call (`alsem_explain {"routines": [<both
selectors>]}`) with the distinguishing parameter named (c26: `differs in: a trailing Boolean`);
n08 prints one candidate array (selector, signature, stable id, `defined at`), ordered by line,
and no effect metadata (11 KB to ~2 KB). Ambiguity never picks a routine (G52).

**G41: names residue** (both; fact checks). Every object name in renderer-owned text and JSON
display fields goes through r1 §2.1's formatter. Instances: by-object lists (c18, c30, n03, n10
`most actions`, `by publisher`); detail strings (c15 and c25 `detail.boundary`, c25
`uncovered[].objectDisplay`, n01 `beyond Codeunit CSC Core::…`); JSON `objectDisplay` (c16, c18,
n05); c15's `changed.routines` echo and n08's `candidates`; c22's header (`of CDO Queue Entry`);
c27's prove header; c05's `calls:` line (`Object::Routine`); c06's `Table 6175283 (app
f4b69b55-…)` becomes `Table "CDO E-Mail Template Header"`; n09's `first page action: Page "CDO
Unhandled"` names the action that opens it; `selector` on every routine row (c08's `routines`
table, c21's rows). What the caller asked for stays as given in its own field; the canonical
display is a separate field (astra 6). JSON locations: §4.6. Single-word names stay unquoted
(G55).

**G42: one JSON shape per field** (Fable cross-tool 8). New fields beside the old (§4.6): digest
`effectCounts` (always a map) beside `effectsByType` (a map or `{"shared": "s1"}`), with
`sharedWith: "s1"`; digest `tableAnswers[].tableRef` with prove's `{id, name, number, app}`
beside `table`. A blocker with no source site keeps its kind and has `site: null`; nothing marks
it as skippable (astra).

**G43: legacy words in JSON** (astra; fact check c08 #7). The envelope summary moves now:
`byTempState` keys `temporary` / `not temporary` / `caller-dependent` / `not resolved`. Payload
`entryPoints` / `beyondEntryPoints` get `dependencyCalls` / `beyondDependencyCalls` beside them.

**G44: `deterministic: false`** (astra; reopens r1 F66). Prove and digest JSON add `volatile:
["generatedAt"]` beside it (additive).

**G45: a location says its role** (astra; fact check c21-c30 #5; reopens r1 F78). c29's
`Page "CDO Customer Card"::OnOpenPage (…:345)` cites the call line, c30's transitive list the
definition line. Each prints its role: `(called at :345)`, `(defined at :243)`.

### 4.6 Schema migrations (astra 6)

r1 §8.3 holds: a new field beside the old one is a minor bump; a changed shape or meaning is a
major, the previous shape kept behind `schema: <previous>` for one release, with a test that
reconstructs the old document from the new one.

| Change | Kind | Schema | Reconstruction test |
|---|---|---|---|
| G34 table and file tables in query effects rows | major | query 5.0.0 (`schema: 4` one release) | every 4.x row's `tableId`, `tableName`, `origin.file` rebuilt from the tables |
| G35 hops as arrays in digest | major | digest 5.0.0 (`schema: 4` one release) | every 4.x hop string rebuilt |
| G42 `effectCounts`, `sharedWith`, `tableRef` | minor now; `effectsByType`'s shared form and the old `table` leave in digest 5.0.0 | digest 4.1.0 → 5.0.0 | — |
| G43 `dependencyCalls`, `beyondDependencyCalls` | minor now; old names leave in the next majors | query 4.x, digest 4.x | — |
| G38 `dependencyCount`, `dependencyClosure` | minor now; the full list leaves in each next major (the app identities stay reachable through the closure entry in the index) | query, digest, prove | — |
| G41 `origin.fileDisplay` (`dep:<app name>:…`) | minor; `origin.file` keeps its GUID form and `origin.appId` stays | query 4.x, digest 4.x | — |
| G1 `byRelation`, `programEntriesByKind`, `relationRecorded` | minor; `programEntries` stays a number | impact-up | — |
| G2 `selection.byRelation` | minor | `al-sem.tests/2` | — |
| G44 `volatile` | minor | prove, digest | — |

Query 5.0.0 and digest 5.0.0 are the only breaks; each ships with its consumer notice (r1 §7.2,
§7.3: ConTinAI reads query rows; bc-dev-mcp is pinned and not exposed).

---

## 5. Decisions on the known open items

**Owner decisions, 2026-11-07:**

| Item | Fable | astra | Decision |
|---|---|---|---|
| c08 729 KB (target 200 KB) | take the four options (~480 KB); no compact default; accept missing 200 KB | ~480 KB acceptable as an explicit revised budget after normalization, not as meeting 200 KB | **Owner:** the 200 KB target is dropped; the size check is pinned at about 480 KB after G34's cuts. Pretty JSON stays the default |
| c25 643 KB (target 300 KB) | arrays plus compact file default (~290 KB), if `rowDetail: compact` is stated and `compact: false` stays | compact acceptable only with an adequate readable index | **Owner:** compact is allowed as the default for digest files (~290 KB); a readable index and a pretty option remain. This changes r1 §7.3. astra's must-change 7 applies: the index keeps conditional persistence, uncertainty, table verdicts and test status (G10) |
| c02 6.7 KB (target 3 KB) | take all three options | the same; one candidate overall | **Owner:** the target moves to the measured size after the cuts: 3.5 KB (G33) |
| c20 7.0 KB (target 4 KB) | take all three; add table names | counts only, hubs once; ~4.3 KB acceptable with the table evidence | **Owner:** the target moves to the measured value after the cuts, about 4.6 KB (G37 + G7) |
| new words | — | — | **Owner:** approved once both spec reviewers accept them; no second owner sign-off |

The new words: `program-wide` (with its percentage), `run by:`, `potential UI surfaces`, `field
assignment` / `validation`, `opFamily` (value `write`), `relation not recorded` / `relation not
established`, and the printed form `[<tier>, <relation>]`. Rule for the bracket form (astra): a
bare `[<tier>]` keeps r1's meaning (a tier only); a tool whose populations are code-only by
construction (prove's callers, touches up) says `code paths only` once in its scope clause, and a
tool that mixes relations (tests, impact) prints both axes on every item.

**Other open items:**

| Item | Fable | astra | Decision |
|---|---|---|---|
| c17 73 KB (target 60 KB) | take hop grouping (~58 KB) | acceptable as is; grouping optional | **G36**: meets the target, loses no identity |
| impact: no relation split, no configuration tier | not acceptable | not acceptable | **G1**: interim in W1, engine in W2 |
| entry counts per legacy bucket | acceptable; moot after G1 | acceptable as interim; prefer distinct | W1 counts distinct nodes; **canonical kinds in W2** |
| `outcome` from the answer text | acceptable if pinned per value | not acceptable | **structured, on `fix/r2-counts`**, a test per value |
| summaries only for file answers | acceptable | not for inline JSON | **G12** |
| no generation on explain's header | acceptable | acceptable | **no change** (G54) |

---

## 6. Where the reviewers disagree

### 6.1 Output reviews

| Topic | Fable | astra | Resolution |
|---|---|---|---|
| n02 (G2) | misleading | missing | **misleading, W1**: agents act on the headline; the fix is the same |
| c06/n10 callers, n07 trigger (G3, G4) | misleading, low | c06 not raised; n07 missing | **W1**: small, and both ask for the n07 content |
| c08 `analysis: complete` (G5) | not raised | misleading | **`fix/r2-counts`**; Fable's c11/c23 `limits: none` is G14 |
| `outcome` derivation (G6) | acceptable if pinned | not acceptable | **structured** (astra), with a test per value (Fable) |
| inline JSON summary (G12) | file-only is fine | c21 needs one | **astra**: sorted JSON buries the selection |
| c17 size (G36) | take grouping | acceptable as is | **grouping**: astra calls it acceptable polish |
| c25 file default (G35) | compact with conditions | prefers prefixes; compact with a readable index | **compact plus G10's index** (owner decision) |
| "program-wide" wording (G1) | say "program totals, not specific" | never "not specific" (r1 §11.2 row 6) | **state the share and fold the counts** |
| c05 event names (G49) | (r1 F80: print them) | noise | **keep the names** (r1 F80) |
| n08 ambiguity (G40, G52) | expand every overload | a ready call | **astra**: ambiguity never picks (r1 §11.1) |
| `limits:` (G14, G56) | only when non-empty | (wave-2 §3.7 keeps it) | **visible and truthful** |

### 6.2 Spec reviews (of revision 1)

| Topic | Fable | astra | Resolution |
|---|---|---|---|
| Bracket form | `[<tier>, <relation>]`, with "a bare `[<tier>]` means a code path" | do not redefine `[local]` to mean code | **astra's rule, Fable's form** (§5): redefining a tier word would make every existing `[local]` claim a relation it never measured. Tools that mix relations print both axes; code-only tools say so once in their scope. Fable's need (an agent can tell code from navigation on any line) is met |
| `external:` in W1 | restrict to the code population | suppress until the code population exists | **astra**: in W1 there is no code population, so "restricted to it" means not printed. JSON keeps it with its population named |
| c02 default (G33) | one candidate per context | one candidate overall | **astra**: it is the implementer's measured ~3.4 KB option behind the owner's 3.5 KB target; per context is behind `detail: true`, and the counts by context stay on the default line |
| `TestHandler` class | decide; would say code | not code-call, so classify explicitly | **code** (Fable's reason: the test's code opened the page); the table says so, and it only occurs in test universes |
| Rule edges (`declaredRunnable`) | not followed is defensible | an "excluded rule" class | **followed, as candidates** (relation not established): not following would drop reach with no evidence of absence; labelling them keeps the population honest. Revision 3: only the `TestPage` declaration rule is a candidate; the typed-receiver builtin rule is code; the known-false plain declarations stay excluded (§2.1 step 1) |
| Cost fallback (b) | conditional fallback | needs a dominance proof first | **cut from W2** (§R2.3): a separate reviewed design if (a) misses the targets |

---

## 7. Milestones in detail

Each milestone: the task gate per commit; `scripts/check-goldens --regen` with every moved family
read and named in the commit; `check-goldens --verify-coverage`; `scripts/cdo-gate` at the branch
gate; a CHANGELOG entry. Round 3 rebuilds `~/.al-sem/bin/alsem`, re-runs the 40 calls with the
round-2 recorder, and checks the calls named below.

| # | Golden families it moves | Round-3 verification calls |
|---|---|---|
| W1 | `cli-output-goldens/impact`, `cli-output-goldens/tests`, `cli-output-goldens/mcp-surface` (impact, explain), `cli-explain-goldens`, `cli-output-goldens/cdo` (explain renderings) | c12, c20, n09: no bracketed tier on the unclassified population, `relationRecorded: false` in JSON and summary, the share line with distinct `<n>`, no root-as-entry, hubs once, no `external:` in text, `calls:`; n02: scoped headline, `[local, UI navigation]`; c06, n10: `callers: 43 direct …`, `71 direct …`; n07, c10: `run by:`; c05: `entry point: this action` |
| W2 | `cli-output-goldens/impact`, impact JSON goldens, `cli-output-goldens/mcp-surface` (impact); `r0-goldens` must **not** move (also after the `seen` widening) | c12 (code none; navigation program-wide), c20 (Copy Company report by code, its page by navigation), n09 (code through Customer deletes), the second c20 routine (narrow); §2.1's cost targets on c12, c20, n09 |
| W3 | `cli-query-goldens`, `cli-explain-goldens`, `cli-output-goldens` (digest, tests, cdo), `cli-output-goldens/mcp-surface`, the overflow and header invariants | c20 (table, fields, company), n03, c18, c30, c25 (20 rows ≤ 8 KiB, `next`), c21 (spread, `runFirstLine`, inline summary), c15, c19, c01, c11, c23, n01, n04, c07, c04, c02, c10, n05, n07; a table trigger's `run by:` |
| W4 | every human family (names), `cli-query-goldens` and digest JSON (new fields, query 5.0.0, digest 5.0.0 with their `schema: 4` documents), prove JSON goldens | c05, c06, c08, c15, c16, c18, c22, c25, c27, c29, c30, n01, n03, n05, n08, n09, n10; c03, c26, n06 (G40) |
| W5 | `cli-output-goldens` (digest, tests, prove, cdo), `cli-query-goldens`, the size ratchets in `output_invariants*.rs` set to §5's values in the same commits | c02 (≤ 3.5 KB), c05, c06, c08 (~480 KB), c13, c14, c17 (< 60 KB), c19, c20 (~4.6 KB), c22, c25 (~290 KB compact), c28, c29, n02, n05, n10 (~5 KB) |

---

## Appendix A. Findings register (round 2)

Columns: raised by (fable, astra, both); category; disposition (design change with its section;
handled elsewhere; rejected with the reason). Duplicates across the two reviews are merged into
one row and say so.

### A.1 Counts

| Disposition | Misleading | Missing | Noise | Size | Consistency | Total |
|---|---|---|---|---|---|---|
| design change | 4 | 19 | 8 | 7 | 7 | 45 |
| handled elsewhere | 2 | 3 | — | — | — | 5 |
| rejected | — | 4 | 3 | 2 | 3 | 12 |
| **all** | 6 | 26 | 11 | 9 | 10 | **62** |

### A.2 Register

| id | Finding | Calls | Raised by | Category | Disposition |
|---|---|---|---|---|---|
| G1 | impact's reach without relation, with configuration under `via-dependency`; per-kind lines near-constant (merged: Fable B1 + cross-tool 1, 2, c12 `external:`; astra M2 + cross-tool 1, legacy bucket counts) | c12, c20, n09 | both | misleading | design §2.1 (W1 interim, W2 engine) |
| G2 | tests headline presents UI-navigation-only tests as "run first … on a local path"; bare `[local]`; "action" for a field trigger | n02 | both (Fable misleading, astra missing) | misleading | design §2.2 |
| G3 | `callers:` excludes direct page-action callers that the caller-rule line counts | c06, n10 | fable | misleading | design §2.3 |
| G4 | `callers: none` on a field trigger | n07, c10 | both (Fable misleading, astra missing) | misleading | design §2.4 (W1 and W3 rows) |
| G5 | `analysis: complete` beside family blockers | c08 (c11) | astra | misleading | handled elsewhere: `fix/r2-counts` (c08 gate before W1) |
| G6 | `outcome: not-found` for an ambiguous selector | c26, n08 | astra | misleading | handled elsewhere: `fix/r2-counts` |
| G7 | c20 does not name the table, fields, operation or company context; n09 its three tables | c20, n09 | both | missing | design §3.1 |
| G8 | touches on a bare table samples only Gets; writes not listed (merged: Fable n03, c30; astra n03, c18, cross-tool 9) | n03, c18, c30 | both | missing | design §3.2 |
| G9 | digest `tests: true` gives `not loaded` with no handoff | c25 | both | missing | design §3.3 |
| G10 | digest summary lists the wrong 6 of 20 rows; answers cover writes only; `shared` unused | c25 | both | missing | design §3.4 |
| G11 | capped tests pick 10 tests of one routine; `runFirstLine` describes the uncapped set | c21 | fable | missing | design §3.5 |
| G12 | large inline JSON has no compact selection summary | c21 | astra | missing | design §3.6 |
| G13 | `firstBlocker` is the meta-entry; `coverage incomplete` counted as a blocker; observed tables not named | c15, c19 | both | missing | design §3.7 |
| G14 | `limits: none` beside a body-stated limit; table-event subscribers of a write not stated | c01, c11, c23 | fable | missing | design §3.8 |
| G15 | impact has no `calls:` line | c12 | fable | missing | design §3.9 |
| G16 | may-commit leads with a dependency witness; first line names no site | n01 | both | missing | design §3.10 |
| G17 | implicit error without its operation; TryFunction not said; no interception follow-up | n04 | both | missing | design §3.10 |
| G18 | "then the whole suite" has no run target; no dependency filter line | c07 | both | missing | design §3.10 |
| G19 | no root location or excerpt on a negative with `showSource`; trigger location and route missing | c04, c23 | astra | missing | design §3.10 |
| G20 | the actionable blockers are not first | c02 | both | missing | design §3.10 |
| G21 | unresolved-table sites without receiver and reason | c18 | astra | missing | design §3.10 |
| G22 | key-relevant uses without the key fields or the declaration exclusions | c30 | astra | missing | design §3.10 |
| G23 | field answer followed by a trigger card; `no rule`; kinds with no entries; follow-ups not complete calls | c10 | both | missing | design §3.10 |
| G24 | callers'-context cards lead with effects; captions and 35 KB of effects bury the callers' answers; no "lock state not analyzed" | c06, n10 | both | noise | design §4.1 |
| G25 | repeated lines in one answer | c02, c05, c06, c10, c14, c22, c29, n07 | both | noise | design §4.2 |
| G26 | action snippet twice; the action "reached from" itself; excerpt not at the cited line | c05 | both | noise | design §4.3 |
| G27 | header line 3 carries per-tool scope prose on every answer | all | both | noise | design §4.3 |
| G28 | tests boilerplate ~600 bytes per answer | c07, c21, c28, n02 | both | noise | design §4.3 |
| G29 | query-effects answer is one 560–600 character sentence | c11, c23 | both | noise | design §4.3 |
| G30 | dead CLI pointer; hunk list and method paragraph for a BOM-only change | c13 | both | noise | design §4.3 |
| G31 | `top callees not resolved` repeats the blockers, misses one, mixes interface calls | c19, n01 | both | noise | design §4.3 |
| G32 | tests paths repeat long shared tails | c28, n02 | both | size | design §4.4 |
| G33 | c02 6.9 KB (target 3 KB) | c02 | both | size | design §4.4, owner decision §5 |
| G34 | c08 729 KB (target 200 KB); `byTable` cut at 10 of 13 | c08 | both | size | design §4.4, owner decision §5 |
| G35 | c25 643 KB (target 300 KB) | c25 | both | size | design §4.4, owner decision §5 |
| G36 | c17 73 KB (target 60 KB); 3 callers with their own touch unnamed; "the three" unnamed | c17 | both (astra: acceptable) | size | design §4.4, §5 |
| G37 | c20 7.0 KB (target 4 KB); whole-change block repeats; hubs ×3 | c20 | both | size | design §4.4, owner decision §5 |
| G38 | 7 KB for zero rows: dependency list and family prose per call | n05, c08, n08 | both | size | design §4.4, §4.6 |
| G39 | "writes" means field assignment and record write; Delete printed as "modify" | c10, c25 | astra | consistency | design §4.5 |
| G40 | not-found sentence ×3; nearest names unranked; no complete call; 11 KB for an ambiguity | c03, c26, n06, n08 | both | consistency | design §4.5 |
| G41 | unquoted multi-word names, `Object: Routine`, table by number, GUID in text-like JSON, page for an action, rows without `selector` | c05, c06, c08, c15, c16, c18, c21, c22, c25, c27, c30, n01, n03, n05, n08, n09, n10 | both | consistency | design §4.5, §4.6 |
| G42 | polymorphic `effectsByType`; two `table` shapes; blocker without a site | c15, c16 | fable | consistency | design §4.5, §4.6 |
| G43 | `known(false)` in the envelope; `entryPoints` in JSON | c08, c15 | astra | consistency | design §4.5, §4.6 |
| G44 | `deterministic: false` unexplained in the document | c16 | astra | consistency | design §4.5 |
| G45 | `file:line` after a routine means call line or definition line | c29, c30 | astra | consistency | design §4.5 |
| G46 | locks deeper than one dependency routine [engine] | c11, n10 | both | missing | handled elsewhere: r1 §10 (deferred engine capability); the output states it (r1 F57) |
| G47 | declaration-level rename consumers [engine] | c22 | astra | missing | handled elsewhere: r1 §10 (F60); the output states it |
| G48 | callers' held locks and lock order [engine] | n10 | astra | missing | handled elsewhere: a deferred engine requirement (§1); its output limitation ("locks held by callers: not analyzed") is G24 |
| G49 | c05's 17 event names dominate a small card | c05 | astra | noise | rejected: r1 F80 (owner-approved) asked for the names; they are one line |
| G50 | routes to c05's two error sites | c05 | astra | missing | rejected: optional per the reviewer; the locations are usable and prove gives routes |
| G51 | the via-dependency count moved 369 → 445 with no note | c07 | fable | missing | rejected: not agent-facing (the reviewer says so); recorded for the owner |
| G52 | expand an ambiguous digest root to every overload | n08 | fable | missing | rejected: an ambiguous selector never picks (r1 §11.1, c26 row); G40's complete call gives all five in one paste |
| G53 | c15 118 KB: semantic bulk (repeated signatures, overlapping shared-effect detail) | c15 | astra | size | rejected for this round (deferred): the shared blocks and the normalized artifact already serve the comparison, and G13 makes the summary carry the verdicts; astra accepts deferral on that basis |
| G54 | explain header shows no revision generation | all explain | both (both accept) | consistency | rejected (no change): both accept; the envelope has it |
| G55 | single-word object names unquoted (`Table Customer (18)`) | c20, n09 | fable | consistency | rejected: r1 §2.1 (owner-approved) quotes only when AL needs quotes |
| G56 | print `limits:` only when non-empty | all | fable | noise | rejected: wave-2 §3.7 keeps limits visible; G14 makes it truthful |
| G57 | n04: say "the publisher's transaction rolls back" | n04 | fable | missing | rejected: the analysis does not establish rollback (c29: "nothing here says which writes roll back"); G17 adds the TryFunction fact and the interception follow-up |
| G58 | tests boilerplate once per session | c07, c21, c28, n02 | fable | noise | rejected: the answer is the CLI's stdout byte for byte, which has no session; G28 shortens it |
| G59 | c21 JSON repeats long test paths (normalized shared paths) | c21 | astra | size | rejected for this round (deferred to the next tests schema major): c21 is 31 KB, inside the inline cap; G11 and G12 put the selection first; a paths table is a tests major with no reader asking yet |
| G60 | n07 test status gives no complete `alsem_tests` call | n07 | astra | missing | design §3.10 |
| G61 | n07 `doc:` text should stay visibly a source comment | n07 | astra | consistency | rejected (no change): it already prints under the `doc:` label, which marks it as the source's doc comment; the reviewer asked that it remain so |
| G62 | n05 does not say its answer is about record operations, not page safety | n05 | astra | missing | design §3.10 |

---

## Appendix B. Round-1 findings reopened

A finding a reviewer marked **not resolved** or **partly resolved** in round 2, with the
reviewer's evidence. "→" names the round-2 row that carries it.

| r1 | Status (reviewer) | Evidence quoted by the reviewer | → |
|---|---|---|---|
| F12 | not resolved (both) | Fable: c12/c20/n09 `page actions 5,777 (via-hub 996), codeunit OnRun 447 …`; astra: `nearest entry per tier (relation not recorded by this walk)` | G1 |
| F11 | partly (Fable) | `first:` items carry `[via-dependency]` but not that the walk is UI navigation | G1 |
| F53 | partly (astra) | impact keeps overlapping `legacy bucket` counts | G1 |
| F56, F75 | not resolved (both) | c20: `may write 1 table (1 owned by this app, 0 by dependencies)` | G7 |
| F58 | not resolved (Fable) | c01: `limits: none`; nothing on OnModify / OnAfterModifyEvent subscribers | G14 |
| F55 | not resolved for impact (Fable) | c12 has no `calls:` line | G15 |
| F48 | not resolved (astra: c04); partly (c10, c23) | c04 ends after the header; c10 lacks the field declaration; c23 the trigger location | G19, G23 |
| r1 c05 item | not resolved (Fable) | c05 lines 183–187 printed twice | G26 |
| F66 | not resolved (astra) | c16 `deterministic: false` unexplained | G44 |
| F61 (engine part) | not resolved (astra) | `deeper dependency locks are not inventoried` | G46 |
| F60 | not resolved (astra) | c22 still excludes columns, CalcFormula, TableRelation | G47 |
| F77 | not resolved (astra); partly (Fable) | c25 `"testStatus": "not loaded"`, no follow-up. (F26, the false coverage presentation, astra marks **resolved**) | G9 |
| F42 | partly (both) | c25 `summaryTruncated: rowsListed 6, rowsTotal 20`; c15 `firstBlocker` is `coverage status: partial` | G10, G13 |
| F45 | partly (both: c02); not resolved (astra: c28) | c02 6,863 bytes with `stopped:` twice; c28 five full paths, 7,729 bytes | G33, G32 |
| F40 | partly (both) | c08 1,006 KB → 729 KB | G34 |
| F41 | partly (both) | c25 1.18 MB → 643 KB; c15 118 KB | G35 (c25), G53 (c15) |
| F38 | resolved (Fable: repeated witness); partly but acceptable (astra: size) | c17 320 KB → 73 KB | G36 |
| F46 | partly (both) | c20 whole-change block repeats; hubs line ×3 | G37 |
| F1–F4, r1 §2.1 selector | partly (both) | `Codeunit CSC Core::LogUsageExtended`; `Table 6175283 (app …)`; c08 `r:262` without `selector` | G41 |
| F8 | partly (astra) | c26 gives a parameter fragment, not a complete call | G40 |
| F36 | partly (astra) | c26/n08 `outcome: not-found` | G6 |
| F50, F61 | partly (astra) | c08 `analysis: complete` beside `notExhaustive` families | G5 |
| F78 | partly (astra) | some locations unlabelled or inside strings | G45 |
| F29, F47 | partly (astra) | c06 headline `writes: no`; caption and caller lists dominate | G24 |
| F28, F30, F44 | partly (astra) | c21's selection follows large arrays; no inline summary; repeated paths | G11, G12, G59 |
| F63 | partly (both) | c07 `then the whole suite` without a run target; no dependency filter | G18 |
| F64, F31, F32 | partly (both) | tests paragraphs per answer; header line 3 ~250–450 bytes | G28, G27 |
| F34, F35 | partly (both) | c13's CLI pointer; c10 `alsem_query_touches with table and field`; n07's tests line | G30, G23, G60 |
| F49, F51 | partly (astra) | envelope `known(false)`; JSON `entryPoints` | G43 |

# Output validation, round 3 revision (r3): what round 4 builds

Status: **revision 3**. Revision 2 folded in both reviews of the draft; revision 3 adds the
second-pass changes (astra: `var`-formal aliasing, the case no-match edge, the rebuild dropping
selectors, measuring with selectors; Fable: selectors without duplicating `display`, and
selectors win over the inline target).
Date: 2026-11-08. Base: master `275bdb2d`.
Earlier specs: `2026-11-06-output-validation-r1.md`, `2026-11-07-output-validation-r2.md`.
Inputs: the round-3 reviews and rechecks
(`target/tmp/output-validation/round3/review/{review,recheck}-{astra,fable}.md`), the round-3
fact checks (`round3/factcheck/*.md`), the fix agents' reports, and the reviews of this spec
(`round3/review/spec-r3-review-{astra,fable}.md`).

Both reviewers accepted round 3. The owner asked for a fourth round to close what is still
open: the design points both reviewers share, the two "path cannot run" answers (n01, n10),
and the compact tests shape. Owner decisions (2026-11-08): **build both engine items (E1, E2)**
and **bump the tests schema in this round (T1)**.

Item ids: A = astra, F = Fable; P/top-10/cross numbers are theirs. To avoid a clash with this
spec's E1/E2, astra's round-3 deferred engine items are called **D1–D3** here.

---

## 0. Changes from the reviews (revision 2)

| Item | Change | From |
|---|---|---|
| E1 | Literal list includes `Var::Member` on an Option/Enum-typed variable; Integer/Option compare with labels by ordinal after AL's conversion; Enum compares by (enum id, ordinal); value sets with an explicit unknown; unresolved labels and `else`; full "never assigned" rule; E2 values bind callees | A 1–5, F 1–5 |
| E2 | Statement order, joins, loop widening before any read in the loop; tracked variables listed; publisher rule = every possibly-invoked subscriber proven not to modify, per universe; guards kept when not evaluated; conservative cost fallback | A, F 1–4 |
| T1 | Problem statement corrected; exact 3→2 rebuild; one-line `reaches` objects; size target 38.8 KB measured; `relation` semantics; L3's candidate field in the shape | A, F |
| H2, H3, H4, H5, H7 | Narrowed as each reviewer asked | A, F |
| L3, L4 | Machine representation; measurement and boundary tests | A |
| §5 | A P6, P10, P12, F top-9, F cross-8 rationales corrected; new §5.2 for open points astra listed | A |
| §6 | Regression gates for the closed c21/c20 fixes; what the unknown-count gate means | A |

---

## 1. Engine: literal values beyond Booleans (n10, n01)

### E1 — Integer, Enum and Option values (fixes n10)

**Problem.** The literal reach (`src/engine/l5/literal_reach.rs`) binds Boolean parameters and
RecordRef tables, not Integer, Enum or Option values. n10: `DocSave` passes
`OutputAction::SaveAsPDF` into `RunReportID`'s **Integer** parameter `OutputActionInt`; the
labels are members of a **global Option variable** (`OutputAction: Option Print,EMailOpen,…,,,SaveAsPDF`,
`CDOEMailHandler.Codeunit.al:26`), used as `case OutputActionInt of OutputAction::Print …`
(:426) and `if OutputActionInt = OutputAction::SaveAsPDF` (:461). So n10 counts 282 tables and
5 commit sites that only other branches reach, while its own detail says "no listed workspace
route runs with these literal arguments".

**Value domain.** A bound value is a finite set of concrete values, or **unknown**. A join
with unknown is unknown. Each route keeps its own binding: an unknown argument on one route
never inherits another route's literal. An effect is ruled out only when **every** route to it
is ruled out.

**What binds a parameter.** A parameter is eligible when it is passed by value, has no
assignment in the routine (compound assignment included), is never passed `var`, never passed
to an unknown callee, and never written by `Evaluate` or `Clear` (the existing
`param_guard` usable rule). It binds when every caller on the route passes one of:
- an integer literal;
- an option or enum label: `Enum::"X"::Y`, `X::Y`, `Rec.Field::Value`, **and `Var::Member`
  where `Var` is an Option- or Enum-typed local, global or parameter** (n10's form), resolved
  to its ordinal, counting blank option members;
- a parameter of the caller that is itself bound (the existing chain);
- after E2: a caller's tracked local whose value set is known.

**Comparison.** Values compare **after the applicable AL conversion**: an Integer or Option
parameter compares with an option or enum label by ordinal; an Enum-typed parameter compares
by (enum type identity, ordinal). A conversion the engine does not model makes the comparison
unknown (both arms live).

**Where it prunes.** `case P of A, B: … else …`, `if P = A`, `if P <> A`, `P in [A, B]`, with
the parameter on one side and a label or literal on the other:
- an arm is dead only when **every** label in it resolved and none matches the bound set; a
  label the engine cannot resolve (an extension value from an unloaded app, `FromInteger`, a
  range, an expression) makes its arm live **and** keeps `else` live;
- `else` (or, without one, the implicit no-match edge: never the shared code after the `case`)
  is dead only when every bound value matches a resolved label of some arm;
- an unknown enum value is never narrowed to the members loaded today (an enum extension can
  add values); a resolved extension member passed explicitly may be a one-value binding.

**Shows on.** n10 (headline and detail agree). c25, c15, n01 may move; attribute every row.

**Tests and proofs.** Fixtures: an Integer parameter against Option labels (the n10 shape),
an Enum parameter, an Option parameter; controls: a `var` parameter, an assigned parameter,
a parameter passed `var` to a helper, a non-literal argument, an unknown join of two routes,
an unresolved label (its arm and `else` stay live), an unknown extensible enum value, a known
extension member. One discrimination proof per rule.

### E2 — Within-routine values and publisher `var` parameters (fixes n01)

**Problem.** After E1, n01's commit at `CDOMailManagement.Codeunit.al:123` still looks
reachable: `UseSMTP` is reassigned (`if UseHandleDoc then UseSMTP := true`), passed `var` to
the publisher `OnBeforeCreateAndSendMailV2`, and `IsEmailPreview := not UseSMTP` is derived.
The witness condition "guarded-by-IsHandled" leaves out the real guards (`if IsEmailPreview`,
`if Modal`).

**Flow rules.**
- **Tracked:** locals; by-value parameters; `var` parameters from their first in-routine
  assignment, **with alias safety**: two `var` formals can name the same actual (`P(X, X)`),
  and a `var` formal can name a global the routine also writes. So a tracked `var` formal
  becomes unknown at any write to another `var` formal or to a global of a compatible type, and
  at any call (a callee may write the aliased actual). **Globals never.** Any expression the
  pass cannot evaluate gives unknown.
- **Order:** each statement is evaluated with its incoming state; then its writes and call side
  effects apply.
- **Values:** literals, other tracked variables, `not`/`and`/`or` over them, and E1's value
  sets. By-value passing keeps the value; `Evaluate`, `Clear` and a `var` pass to anything other
  than a publisher (below) write it (unknown).
- **Joins:** all feasible incoming branches join; unsupported assignments and uncertain
  mutations give unknown.
- **Loops:** a variable assigned anywhere in a loop body (implicit loop counters included, and
  writes through calls) is **unknown at every read inside the loop, including reads before the
  assignment, and after the loop**. (`X := false; repeat if X then Write(); X := true; until …`
  keeps the write.)

**Publisher `var` parameters.** Passing a tracked variable `var` to an event publisher keeps
its value only when **every possibly-invoked loaded subscriber is proven unable to modify the
corresponding parameter**, directly or transitively: a subscriber that assigns it, forwards it
`var` (to a helper or a nested publisher), passes it to an unknown callee, or writes it with
`Evaluate`/`Clear` counts as modifying; an unresolved callee, an unavailable subscriber body or
an uncertain parameter mapping prevents keeping the value. With several subscribers, all must
pass; order does not matter. The subscriber inventory is **per universe** (the Test app
subscribes to `OnBeforeCreateAndSendMailV2` with `var UseSMTP`, so the test universe keeps
nothing there). The loaded-subscribers assumption is stated in the human text and in
structured output wherever it decides a route, naming the universe's subscribers considered.
It never strengthens a tool's default negative proof beyond that tool's existing policy.

**Witness conditions.** A condition lists the guards on the route with their polarity:
evaluated ones by value, the others as `not evaluated` (never a class label presented as a
guard). Before X2 lands, H2(d) prints `condition: not evaluated (IsHandled pattern)`.

**Cost.** Measure before/after on CDO (n01, n10, c25: time and peak RSS). If the pass costs
more than 10% on c25, skipping it is allowed only by falling back conservatively (as without
E2) for the whole answer; no stale values, no mixed modes. For a multi-root digest "the routes
a question asks about" means the routes from the named roots.

**Tests and proofs.** Literal flow, derived `not`, branch join, a loop-carried value (the
example above), a `var` parameter's first assignment, two `var` formals called with the same
actual (`A := true; B := false; if not A then Write();` called as `P(X, X)` keeps the write), a global (never tracked), a publisher with
no subscriber, one that assigns, one that forwards to a helper that assigns, an unavailable
subscriber body, two subscribers, an unresolved guard kept in the condition. One proof per
rule.

---

## 2. Tests schema 3 (T1, both reviewers)

**Problem.** c21 is 52 KB. Each `runFirst` row carries `reaches` (every reached changed
routine's stable id), `reachesHops` (a positionally parallel array of hop counts) and
`reachesByNavigationOnly` (the subset of those ids reached only by UI navigation).

**Shape (schema 3; schema 2 behind `schema: 2` for one release, r1 §8.3).**
- `changed[]` entries gain `"key": "c:1"`, `"c:2"`, … in `changed[]` order. Keys are unique
  within one document, not stable across calls; every reference resolves to one entry.
- A `runFirst` row's `reaches` is one object per reached changed routine, in `changed[]` order:
  `{"changed": "c:3", "hops": 8, "relation": "code"}`, plus `"candidate": "runtime-editable"`
  when the only path is a candidate page path (L3). `relation` is `code` when any code path
  reaches that routine, `UI navigation` only when every path is navigation (schema 2's
  `reachesByNavigationOnly` meaning).
- **Each `reaches` object is printed on one line** in the pretty output (measured on the
  round-3-final c21: one key per line 44,028 B, over the cap; one line per object 38,828 B).
- `reachesHops` and `reachesByNavigationOnly` leave. `reachesComplete`, the returned-tests
  `runTarget` (B1) and every other field stay as they are.

**3 → 2 rebuild (exact).** Replace each key with its routine's stable id; rebuild `reachesHops`
in the same order; rebuild `reachesByNavigationOnly` as the ids whose relation is
`UI navigation` (absent when empty, as schema 2 does); drop every field schema 3 adds
(`key`, `candidate`, and §5.2's `selector` fields wherever schema 3 adds them: overload rows
of `changed[]`, `runFirst` rows, summary rows); restore the
schema-2 version metadata; keep schema-2 ordering and the absent-versus-empty distinction.
Tests: mixed code/navigation targets, an empty result, a partial inventory
(`reachesComplete: false`), a non-trivial changed set, and a binary-level rebuild test like
`every_v4_document_is_rebuilt_from_v5`.

**Target.** c21 ≤ 38.8 KB, inline, measured on the complete schema 3 (§5.2's selectors
included; the 38,828 B figure was measured without them). If the complete answer exceeds the
inline cap, it stays a file: reach information is never cut to meet the target. CHANGELOG
consumer notice (bc-dev-mcp reads the tests output).

---

## 3. Output design both reviewers share

| id | Change | Shows on |
|---|---|---|
| H1 | **Impact folds the UI-navigation census by default.** `entries by kind` and `by context` under `reached by UI navigation` print only with `detail: true`; the population line keeps count, share, nearest entry, relation and candidate qualifications. One rule in routines mode and diff mode. | c12, c20, n09, m02 |
| H2 | **Prove "yes" leads with what decides it.** (a) On a `yes`, drop the detailed `top callees not resolved` / `top interface dispatch` lists; keep the partial-analysis qualification. (b) Rank only witnesses still eligible after literal analysis; unconditional workspace witnesses before conditional ones. (c) Every headline witness names a real terminal operation and its site (`Insert (implicit error) at …`, `COMMIT at …`), groups included; never the dependency-entry location in its place. (d) Conditions follow E2's rule. | n01, n04, m04, c27 |
| H3 | **One negative form for effect questions:** `no — no <effect> site in the analyzed cone (every candidate decided; …)`, as c27; m06 takes it. Callers'-context questions keep their own subject and scope. An answer with blockers stays `unknown`; "none observed" never becomes "every candidate decided". | m06, c27 (c19 stays unknown) |
| H4 | **Names residue.** One same-object form in explain **and** impact (both print `::Name` today): `this object: Name`, in the word table. c08 `routines[]` rows carry a readable selector that round-trips to the same stable id; where names collide, an app-qualified selector, else the stable id stays the selector with a readable `display`. c06's witness names the call it is inside, both call locations and the write's own location. | n07, n09, n10, m10, c08, c06 |
| H5 | **Small lookup failures come back inline.** An automatic transport rule: a digest whose outcome is `ambiguous` or `not-found` and whose complete answer fits the inline cap is returned inline (byte for byte the CLI answer, in the requested format); an explicit file-output request still writes a file. Human output's header says `answer: ambiguous — names 2 routines` / `not found` and drops `more: --format json for every effect row`; JSON keeps its structured diagnostics, `outcome` and `notAnalyzed`. Tested at the cap boundary. | n08, m01 |
| H6 | **`dependencyClosure` → `dependencyFingerprint`** in query 5.0.0 and digest 5.0.0 (unreleased, so no new major); the schema-4 adapter keeps the 4.0.0 contract. The schema doc states the hash inputs and canonical form (sorted `appId@version`, blake3, `apps-v1:` prefix) and that it identifies an app set: not a retrievable manifest, not proof of unchanged package contents. | c08, c15, n05 |
| H7 | **Lock-edit answers lead with the limit.** With `callersContext`, the `answer:` summary includes `caller-held locks and their order: not analyzed`, and the callers' block's first line says `locks held by callers: not analyzed (no lock facts)`. | n10, m10, c06 |

---

## 4. Leftovers from the round-3 fix work

| id | Change | Shows on |
|---|---|---|
| L1 | A table extension's keys attach to the table it extends in the model, with the extension named; added secondary keys stay distinct from the base primary key. | fixture (the CDO Customer extension declares no keys, so n03 does not change) |
| L2 | `runs:` prints one set of parentheses: `Codeunit "Base Poster" (70100), Run at :15`. | fixture |
| L3 | A page step that is only a candidate (a runtime `Editable(...)` setter can make the page editable) keeps relation `UI navigation` and carries `candidate: "runtime-editable"` on reach and nearest records (T1's shape; schema 2 drops it). Human text: `UI navigation (candidate)`; explain marks such pages `(may be read-only at runtime)`. It applies to mutation-dependent edges only, not to opening the page or its lifecycle. Impact never promotes the same edge to established, and prefers an established route when one exists. | fixture; m03 |
| L4 | Measure the per-routine reach walk (warm, release-fast, median of 3, the test-selection phase alone) at 32, 64 and 128 changed routines on CDO; raise the limit to the largest size under 2 s. When capped, the output names the limit and the routines not expanded, says `reachesComplete: false`, and never turns "not expanded" into "no modeled path"; the spread over changed routines does not treat incomplete reach sets as complete. Boundary tests at the limit and limit+1. | synthetic multi-root fixture |

---

## 5. Single-reviewer items and open points

### 5.1 Decisions on items one reviewer raised

| From | Item | Decision |
|---|---|---|
| A P1 | c08's envelope **and** index carry the analysis status, its reasons and the separate shallow-operation / deep-write scopes | **Do.** |
| A P3 | Touches answers offer a complete write-only follow-up (current table, `opFamily: "write"`) | **Do** (n03, m09, c18). |
| A P5 | Shorter callers' context cards | **Do**, after H7: keep direct-caller counts, one witness, blockers and qualifications that change assumptions; fold names behind detail. |
| A P6 | c06's write location; m07's own behavior | **Do**: c06 with H4 (the write's own location too); m07 prints `starts by: …` from its first statements (read from the routine's statement tree, the same source as explain's excerpt), labelled as a start, not a summary. |
| A P10 | Compact zero-operation answers; duplicate digest-index maps | **Do** zero-operation (keeps blockers, scope, and the page-context distinction). Dedup index maps only where they are aliases by construction, not equal by chance; the index keeps its own compatibility contract. |
| A P12 | Optional extra routes | **Defer**: they are optional evidence available through further queries. |
| F top-9 | c28 headline count; c25 `testStatus`; c15 `tableAnswers` | **Do**: c28 `337 more by UI navigation only in 36 codeunits` (the full list stays in the JSON); c25 a shared default `testStatus` field at document level with per-row values only where they differ, the index keeping per-row status; c15 shared answers only if table-specific verdicts and blockers and the schema-4 rebuild survive. |
| F cross-8 | Impact header line 3 | **Deliberate deferral** (round-2 G56 kept visible limits; G27 shortened only some scope prose; further shortening is not planned this round). |
| A D1–D3 | Deep read/lock inventory; declaration rename consumers; caller-held locks | **Defer** (engine; answers state the limits). |

### 5.2 Open points astra listed that the draft left out

| Point | Decision |
|---|---|
| Selectors in tests (`changed[]`, returned tests, summary and index rows) | **Do** with T1, without duplicating text: `changed[].display` is already the selector form, so it is **documented as the selector** (a row whose display is not a valid selector, such as an overload needing its parameter list, also carries `selector`); `runFirst` rows (which have `codeunit` and `procedure` but no routine string) and summary/index rows get a `selector` (test procedures use their own selector form). If selectors and the c21 inline target still conflict after the re-measure, **selectors win** and c21 stays a file. |
| Remaining vocabulary: c22's field-level `write`, touches' `reads` without Gets, legacy `entryPoints` in summaries and indexes, old temporary-state strings | **Do**: the word table's terms in every place listed; `entryPoints` in digest-index summaries becomes `dependencyCalls` (digest 5.0.0 unreleased). |
| Typo recovery: n06 and m08 rank the right name but give no ready call | **Do**: a complete corrected call for the first nearest name. |
| Digest-index presentation: overall-effect sentences; repeated tests handoff | **Do** the handoff once per index; the per-answer sentences describe the overall effect, not the first row. |
| c06's unqualified `analysis complete` above callers'-context unknowns | **Do**: the header names the component it qualifies (`analysis complete (the routine's own cone)`) when a callers' block follows. |

---

## 6. Milestones and gates

| # | Contents | Depends on |
|---|---|---|
| X1 | E1 | — |
| X2 | E2 | X1 |
| X3 | H1–H7, §5.1 and §5.2 "Do" items except the tests ones | — (H2(d) prints `not evaluated` until X2) |
| X4 | T1 with §5.2's tests selectors, L1–L4 | — |

**Gates** (owner decisions 2026-11-06/07): each branch runs `scripts/ci-steps task` and
`check-goldens --verify-coverage`; CDO goldens are re-minted with plain `cargo test` only when a
change can move CDO output; the landing tree runs `ci-steps all` then `cdo-gate`; the MCP binary
is reinstalled once after the batch. X1 and X2 also run the CDO library pins and frozen digests
and attribute every moved one.

**Regression gates added:** c21's returned-tests run target (B1) and c20's whole-codeunit
overlap wording stay pinned (existing tests; any change to them is a reviewed change).

**The unknown-count gate** is the resolver's `unknown` edge count from
`aldump --program-call-graph-stats` (call resolution). It must stay 0. E1/E2's own "unknown"
values are analysis states, not resolver edges; they are never suppressed or folded to make
any count look better.

Round 4 then re-runs the 50 calls with the same arguments plus 10 new ones (new seed, excluding
every earlier target), fact-checks every claim, and sends the bundle to both reviewers.

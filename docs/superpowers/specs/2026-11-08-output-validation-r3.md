# Output validation, round 3 revision (r3): what round 4 builds

Status: **draft for review** (Fable and astra see it before implementation, goal step 5).
Date: 2026-11-08. Base: master `275bdb2d`.
Earlier specs: `2026-11-06-output-validation-r1.md`, `2026-11-07-output-validation-r2.md`.
Inputs: the round-3 reviews and rechecks
(`target/tmp/output-validation/round3/review/{review,recheck}-{astra,fable}.md`), the round-3
fact checks (`round3/factcheck/*.md`) and the fix agents' reports.

Both reviewers accepted round 3. The owner asked for a fourth round to close what is still
open: the design points both reviewers share, the two "path cannot run" answers (n01, n10),
and the compact tests shape. Owner decisions (2026-11-08): **build both engine items (E1, E2)**
and **bump the tests schema in this round (T1)**.

Every item says where it came from (A = astra, F = Fable; P/top-10 numbers are theirs), what
changes, and the call that shows it. Items only one reviewer raised are listed in §5 with a
decision each.

---

## 1. Engine: literal values beyond Booleans (n10, n01)

### E1 — Integer, Enum and Option parameter values (fixes n10)

**Problem.** The literal reach (`src/engine/l5/literal_reach.rs`) binds Boolean parameters and
RecordRef tables. It does not bind Integer, Enum or Option values. `DocSave` passes
`OutputAction::SaveAsPDF` into `RunReportID`'s Integer parameter `OutputActionInt`, and
`RunReportID` branches with `case OutputActionInt of OutputAction::…`. So n10 counts 282 tables
and 5 commit sites that only the other branches reach, while its own detail says "no listed
workspace route runs with these literal arguments".

**Change.** Add a value-set binding beside `Bindings.tables`:
- Bound for a by-value Integer / Enum / Option parameter that the routine never assigns, when
  every caller on the route passes a literal: an integer literal, an enum value
  (`Enum::"X"::Y`, `X::Y`), an option value (`Rec.Field::Value`), or a parameter of the caller
  that is itself bound (the existing chain).
- Read at `case P of A, B: …` arms (including `else`) and at `if P = A` / `if P <> A`, `in [A, B]`
  where one side is the parameter and the other a literal of the same type.
- An arm whose labels cannot match the bound set is not entered on that route; the effects
  only it reaches are ruled out exactly as Boolean-guarded ones are today ("literal arguments
  rule out … N effect rows").
- Enum values compare by their declared ordinal **and** name (an enum extension can add
  values; a value the engine cannot resolve makes the arm live). Option values compare by
  ordinal. Integer literals by value.
- `var` parameters, assigned parameters, and arguments that are not literals leave the
  parameter unbound (every arm live), as today.

**Shows on.** n10: the tables and commit sites reached only through `SendElectronicDocument` /
`Print` leave its headline; the detail and the headline agree. c25/c15/n01 may move too
(attribute every moved row).

**Tests and proofs.** A fixture with an Integer, an Enum and an Option parameter, each
switching to different writes; controls: a `var` parameter, an assigned parameter, a
non-literal argument, an enum value from an extension. One discrimination proof per rule
(arm pruning, `if P = A`, the enum-extension rule, the `var` rule).

### E2 — Local derived values and publisher `var` parameters (fixes n01)

**Problem.** After E1, n01's first witness moves off the `SendElectronicDocument` branch, but
`CDOMailManagement.Codeunit.al:123`'s commit still looks reachable: `UseSMTP` is reassigned
(`if UseHandleDoc then UseSMTP := true`), passed `var` to the publisher
`OnBeforeCreateAndSendMailV2`, and `IsEmailPreview := not UseSMTP` is derived from it. The
witness's condition "guarded-by-IsHandled" also leaves out the real guards (`if IsEmailPreview`,
`if Modal`).

**Change.**
- **Within-routine constant flow** for Boolean (and E1's value-set) locals and parameters:
  a forward pass over the statement tree that tracks a variable's value set through
  assignments of literals, of other tracked variables, and of `not`, `and`, `or` over them;
  joins at branch merges; widens to "unknown" at loops that assign it, at `var` passes to any
  routine other than an event publisher, and at any assignment the pass cannot evaluate.
- **Publisher `var` parameters:** passing a tracked variable `var` to an event publisher keeps
  its value only when **no loaded subscriber** of that event assigns the parameter. This is
  the engine's existing loaded-subscribers assumption, stated in the answer's assumptions when
  it decides a route.
- **Conditions name the real guard:** a witness's condition lists the guards on its route that
  the pass evaluated (for n01: `if IsEmailPreview`, `if Modal`), not only the IsHandled pattern.

**Shows on.** n01: the first-ranked commit is one that can run with the action's arguments
(today `CoreMgt:118`, entered at `CDOEMailHandler:240`), and its condition names its guards.

**Tests and proofs.** Fixtures for each flow rule (literal, derived `not`, branch join, loop
widening, publisher with and without an assigning subscriber). One proof per rule.

**Cost and limit.** Measured before/after on CDO (`alsem prove` and `digest` on n01, n10, c25:
time and peak RSS). If E2's pass costs more than 10% on c25, it is gated to the routes a
question asks about and the cost is reported.

---

## 2. Tests schema 3 (T1, both reviewers)

**Problem.** c21 is 52 KB because each `runFirst` row carries `reaches` (every reached changed
routine's stable id), `reachesHops` and `reachesByNavigationOnly`: three parallel arrays a
script must zip by position (F recheck 3, A recheck 3).

**Change (tests schema 3; schema 2 stays behind `schema: 2` for one release, as r1 §8.3).**
- `changed[]` gains a short key per routine: `"key": "c:1"`, `"c:2"`, … in `changed[]` order.
- Each `runFirst` row's `reaches` becomes one object per reached changed routine:
  `{"changed": "c:3", "hops": 8, "relation": "code"}` (relation `code` or `UI navigation`),
  ordered by `changed[]` order. `reachesHops` and `reachesByNavigationOnly` leave.
- `reachesComplete` stays. Every other field is unchanged.
- The rebuild 3 → 2 is exact (a reconstruction test over the c21 fixture and a binary-level
  test like `every_v4_document_is_rebuilt_from_v5`).
- CHANGELOG consumer notice (bc-dev-mcp reads the tests output).

**Shows on.** c21 under the 40 KiB inline cap if the measure allows (target: ≤ 36 KB; report
the measured size), c28, n02, m03.

---

## 3. Output design both reviewers share

| id | From | Change | Shows on |
|---|---|---|---|
| H1 | F top-3/cross-3, A P11 | **Impact folds the UI-navigation census by default.** The `entries by kind` and `by context` lines under `reached by UI navigation` print only with `detail: true`; the default keeps the population line (count, share, nearest). One folding rule in both modes (F cross-10: routines mode and diff mode differ today). | c12, c20, n09, m02 |
| H2 | F top-4/cross-6, A P6 | **Prove "yes" leads with what decides it.** (a) Drop `top callees not resolved` and `top interface dispatch` on a `yes` (they only qualify a `no`/`unknown`). (b) Unconditional workspace witnesses before conditional ones. (c) Every headline witness names its operation and site (`Insert (implicit error) at …`, `COMMIT at …`), groups included (today only error groups do). (d) A condition names its guard (E2 supplies the guards it evaluates). | n01, n04, m04, c27 |
| H3 | F cross-5 | **One negative form in prove:** `no — no <effect> site in the analyzed cone (every candidate decided; …)`, as c27 does; m06's bare `answer: no` takes it. | m06, c27, c19 |
| H4 | F top-6/cross-4, A P7 | **Names residue:** no leading `::` in `calls:` for same-object routines (print `this object: Name`, as impact does); every `routines[]` row in c08 carries a name selector (dependency rows too; the stable id stays in its own field); c06's witness names the call it is inside (`inside GetRecipientsDescription (called at :264), before this routine's call at :276`). | n07, n09, n10, m10, c08, c06 |
| H5 | F top-5/cross-7, A P8 | **Small lookup failures come back inline.** A digest whose outcome is `ambiguous` or `not-found` and whose answer fits the inline cap returns inline, like explain's (c26). Its header says `answer: ambiguous — names 2 routines` (or `not found`), not `no answer`; no `more: --format json for every effect row` line. | n08, m01 |
| H6 | A P9 (F did not object) | **`dependencyClosure` is called what it is.** Rename the field `dependencyFingerprint` (a hash of the document's own app list, `apps-v1:<hash>`), in query 5.0.0 and digest 5.0.0 before they are released (no new major: 5.0.0 is unreleased), with a schema-doc sentence saying it identifies the app set and is not a retrievable manifest. | c08, c15, n05 |
| H7 | A P4 (F: c06/n10 lock line noted) | **Lock-edit answers lead with the limit.** When explain runs with `callersContext`, the callers' answer block's first line is `locks held by callers: not analyzed (no lock facts)`, before pending writes. | n10, m10, c06 |

---

## 4. Leftovers from the round-3 fix work (correctness or consistency)

| id | Change | Shows on |
|---|---|---|
| L1 | A table extension's keys attach to the table it extends in the model (`ModelTable` keys), so the keys line lists them with the extension named. | n03 (if the Customer table has extension keys), fixture |
| L2 | `runs:` prints one set of parentheses for a dependency object: `Codeunit "Base Poster" (70100), Run at :15`. | fixture |
| L3 | Page permissions: a page path that is only a candidate (runtime `Editable(...)` setter) is labelled `UI navigation (candidate)` in test selection, and explain marks such pages under "potential UI surfaces" as `(may be read-only at runtime)`. | fixture; m03 |
| L4 | `reaches` completeness beyond 32 changed routines: measure the cost of the per-routine walk at 64 and 128 changed routines on CDO; raise the limit if the cost stays under 2 s, else keep 32 and say so. | c25-sized fixture |

---

## 5. Items one reviewer raised: decisions

| From | Item | Decision |
|---|---|---|
| A P1 | c08's envelope and index carry the analysis status, its reasons and the operation-depth scopes | **Do** (small, additive envelope fields). |
| A P3 | Touches answers offer a complete write-only follow-up (`opFamily: "write"`) | **Do** (one follow-up line; n03, m09, c18). |
| A P5 | Shorter callers' context cards (counts, one witness, blockers; names behind detail) | **Do**, after H7, keeping the lock line first. |
| A P6 rest | c06's pending-write operation location; m07 a one-line own-behavior description | **Do** (c06 with H4; m07: `does: <first statements summary>` from the excerpt already shown). |
| A P10 | Compact zero-operation answers (n05, m05); no duplicate digest-index maps | **Do** zero-operation; **do** the index dedup if the index has two maps with the same content (measure). |
| A P12 | Optional extra routes (c05 two error routes; impact nearest-entry routes) | **Reject**: a complete follow-up call already gives them; adding routes grows every answer. |
| F top-9 | c28 headline: `337 more by UI navigation only in 36 codeunits` instead of the id list; c25 drop per-row `testStatus` when equal on all rows; c15 share identical `tableAnswers` | **Do** c28 and c25; **do** c15 if the shared form keeps the 4.0.0 rebuild exact. |
| F cross-8 | Impact header line 3 (~600 bytes, same on every call) | **Reject** again: declined in round 2 (G56/G27, both reviewers accepted); not reopened. |
| A E1–E3 | Deep read/lock inventory; declaration rename consumers; caller-held locks | **Defer** (engine; both reviewers deferred; answers state the limits). |

---

## 6. Milestones and gates

| # | Contents | Depends on |
|---|---|---|
| X1 | E1 | — |
| X2 | E2 | X1 |
| X3 | H1–H7, §5 "Do" items | — (H2(d) takes E2's guards when X2 lands; until then it names what the engine has) |
| X4 | T1, L1–L4 | — |

Gates (owner decisions 2026-11-06/07): each branch runs `scripts/ci-steps task` and
`check-goldens --verify-coverage`; CDO goldens re-minted with plain `cargo test` only when a
change can move CDO output; the landing tree runs `ci-steps all` then `cdo-gate`; the MCP binary
is reinstalled once after the batch. Engine changes (X1, X2) also run the CDO library pins and
frozen digests and attribute every moved one. The CDO unknown count stays 0.

Round 4 then re-runs the 50 calls with the same arguments plus 10 new ones (new seed, excluding
every earlier target), fact-checks every claim, and sends the bundle to both reviewers.

# Output validation, round 1: what the MCP answers should look like (design spec)

**Status:** design only, draft for the owner. No code lands with this file.
**Date:** 2026-11-06. **Based on:** master `6e577c24`, binary al-sem 2.1.1.
**Input:** 30 MCP calls recorded on the CDO baseline (`bc3ccb18`), each with the task an agent
had; a fact check of every claim against the AL source; two independent reviews of the outputs
against one rubric (Fable and astra). All of it is under
`target/tmp/output-validation/round1/` (local, not committed): `review/review-bundle.md` (the
calls), `bundle.json` (full answers), `factcheck/`, `review/review-{fable,astra}.md`.
**Register:** every finding, merged across reviewers and calls, is in Appendix A (ids F1–F72).
This spec covers the design changes and the new correctness bugs. Rejected findings are in the
register with their reason.
**Out of scope:** the fact check's 13 wrong claims. They are being fixed on
`fix/field-writers-implicit`, `fix/explain-callers-counts`, `fix/db-event-subscribers` and
`fix/reach-and-test-selection`. Where a change here builds on one of those fixes, it says so.

**Stays consistent with:** the MCP output plan (`docs/superpowers/plans/2026-11-04-mcp-output-fixes.md`,
owner decisions 1–5, consumer notes §4.1), the wave-2 design (`…/specs/2026-11-04-mcp-output-wave2-design.md`,
rules §3.6–§3.8), the agent-tasks design (`…/specs/2026-11-05-agent-tasks-design.md`, §5 "existing
effects", §12 surface and versions), and the call-graph export contract
(`…/specs/2026-10-13-callgraph-export-design.md`). Nothing here changes `alsem graph` JSONL.

Words used here:
- **hub** (or **open site**): a call the engine cannot resolve to one target, such as
  `Codeunit.Run(<variable>)`. The graph stands for it with one node that may lead anywhere. A
  path over a hub is a possibility, not a path the code is known to take.
- **tier**: how a path reached its end. `local` (every hop in the workspace or a loaded test
  app), `via-dependency` (it passes dependency code), `via-hub` (it passes a hub).
- **boilerplate**: the coverage / assumptions / limits / scope lines every card repeats.
- **envelope**: the JSON the MCP tool returns around the answer text (`exitCode`, `summary`, …).

**The contract that does not move:** the MCP answer stays the CLI's stdout byte for byte. Every
change below is a change to the CLI output, or to the envelope, made on purpose with its goldens
regenerated and read.

---

## 0. Summary

The engine's facts in round 1 were mostly right (13 wrong claims in 30 calls, all on fix
branches). The outputs failed agents in four ways, and both reviewers found the same four:

1. **The same thing has several names.** Overloads print as one name (four identical
   `CreateSMTPLogEntries` rows); a table's field triggers and a page's actions print as bare
   `OnValidate` / `OnAction`; dependency routines print as `Codeunit 448::handlerequest`.
2. **Possibilities look like facts.** A path over a hub is printed as evidence (c02); a reach set
   the size of the whole program is listed as if it were specific (c12, c20); a conditional write
   is led by "no write" (c01, c23); a filtered "no" hides the sites that do match (c27).
3. **The answer is not first, and not in the summary.** c06's COMMIT hazard is the third-to-last
   line; c15's asked-for CDO File verdict is missing from the envelope summary; c17's new
   information is on line 957 of 961.
4. **Size comes from repetition, not content.** query effects JSON 1 MB for 671 rows; touches
   `callers` 320 KB with one witness repeated 952 times; digest 1.18 MB for 20 overlapping cones.

Plus seven new correctness bugs (§1), each verified in the code.

### 0.1 Milestones

Each milestone can land on its own. The goldens of later ones are rebased on earlier ones; that
is regeneration, not rework.

| # | Milestone | Findings | Size | Depends on |
|---|---|---|---|---|
| V0 | **Word table** (decision only): the tier, entry-kind, "unknown" and "complete" words of §8.1, approved before any renderer uses them | F49–F52 | — | — |
| V1 | **Correctness bugs** | F5, F9, F15, F16, F24, F25, F39 | S, 7 small commits | — |
| V2 | **Verdict and tier wording** (what misleads) | F10, F12, F14, F17, F18, F19, F20, F22, F23, F27, F29 | M-L | V0; F12 after `fix/reach-and-test-selection`, F17 after `fix/field-writers-implicit` |
| V3 | **Identities** | F1–F4, F6–F8 | M | — |
| V4 | **Result header, summaries, follow-ups** | F28, F30–F37, F42, F43 | M | V0 |
| V5 | **Size and dedup** (two schema majors) | F38, F40, F41, F44–F47 | L | V3 (rows refer to routine ids) |
| V6 | **Counting units and per-tool items** | F53–F57, F59, F60, F62–F67, F48, F72 | M | V0 |

Order: V1 first (small, and two of its bugs feed V2 text). V0 is a one-page owner decision.
V2 and V3 next, in either order. V4 and V6 after V0. V5 last: it bumps two schema majors and
needs consumer notices.

---

## 1. Correctness bugs (V1)

Each is its own commit with a test that pins the use (the scenario, not the helper) and one
discrimination proof (break the fix, watch the test fail, revert).

| id | Bug | Fix | Test (discrimination break) |
|---|---|---|---|
| F15 | query effects files a `Get` under "caller-dependent writes": c11 prints `caller-dependent writes 2 on 1 table` then `Purchase Header: Get 1, Modify 1`. `effect_query_cli.rs:2991` `bucket()` returns `callerDependent` before it reads `op_family` | Split the caller-dependent class by op family as the physical class is split: `callerDependent` (writes), and caller-dependent reads join `reads` with a `(caller-dependent)` note. JSON `writeTables` loses the read rows; a minor query bump (3.6.0), since a row moves bucket | fixture routine with a caller-dependent `Get` and `Modify` on one parameter: the writes count is 1, the read is under reads (break: return `callerDependent` for every op again) |
| F16 | One answer, two bases: `(caller-dependent: parameter 3)` (1-based, `temp_note`) and `temp parameter-dependent(2)` (raw 0-based, `temp_state_str` in the `--all` rows) | Human rows print the same words as `temp_note`: `caller-dependent: parameter 3 (ToPurchaseHeader)` with the parameter name. JSON keeps `parameterIndex` (0-based) and adds `parameterPosition` (1-based) and `parameterName` | the c11 shape in the fixture: both lines name position 3 (break: print the raw index) |
| F24 | impact exits 2 when the change has no routine (c13), although the accounting is complete. `impact/render.rs:715` | Exit 0. The summary gains `outcome: "no-routine-changed"` (see F36). Exit 2 stays for "no answer" (unresolvable input) | `cli_impact.rs`: a header-only diff exits 0 with the outcome (break: restore the `roots.is_empty()` rule) |
| F25 | impact without a base says `— signature changed` of a routine the diff adds (c20). `impact/jobs.rs:268-274` sets the status from "a changed line falls on the signature" | Without a base, the status says what the diff shows: `new in this diff (every line added)` when all of the routine's lines are `+` lines, else `signature lines changed in the diff`. "Signature changed" is kept for a base comparison (`delta.pairing == signature`) | the c20 patch shape: an added routine prints `new in this diff` (break: drop the all-added check) |
| F5 | prove's callers' context answers print `dep:<guid>:path` in human text (c02); the rule is `dep:<app name>:path` (plan 2g, wave-2 §3.8 rule 5). `callers_document` (`prove.rs` ~L2981) never calls `dependencies.readable_locations`, which the effects path calls (`prove.rs:2711`) | Call it in `callers_document`. JSON keeps the guid (the ConTinAI citation reader needs it) | the hygiene test `human_outputs_are_clean_and_within_budget` gains a callers' context answer and asserts no `dep:<36-char guid>:` in human text (break: remove the call) |
| F9 | tests paths print two overloads alike: `Codeunit CDO E-Mail::Send → Codeunit CDO E-Mail::Send` (c28). Agent-tasks §12.1 requires an overload handle in every path; `test_select/render.rs` has none | Print the handle (`Send/1` → `Send/2`) through the same `RollupCx::handle` rule impact uses; after V3, the signature form of §2.1 | a fixture test whose path passes two same-name overloads: the two hops differ (break: print the bare display) |
| F39 | The touches `callers` input says "without their witnesses" (`tools.rs:485`, `:460`); every caller line carries one (c17) | The description is made true by F38 (V5): caller rows lose their witness. Until V5, correct the description ("each with its first touch") in V1 | `descriptions_name_exactly_the_inputs_the_schemas_take` gains a check that the `callers` text matches the output's row shape on the fixture (break: revert the text) |

---

## 2. Identities and selectors (V3)

### 2.1 One display form for a routine (F1, F2, F3)

**Current** (quotes):
- c15 summary: four rows, each `"routine": "Codeunit CDO Log Management::CreateSMTPLogEntries"`.
- c01: `--- Table CDO E-Mail Job::OnValidate ---` (the table has 12 `OnValidate` triggers).
- c05: `Page CDO E-Mail Templates::OnAction`; c12: `PageExtension CDOSalesInvoiceExt::OnAction/0#1`;
  c05 again: `page action ExportTemplateAction on Page CDO E-Mail Templates (caption "Export Template")`.

**New:** one canonical display, used in every header, list row, path hop and JSON `display`:

```
Table "CDO E-Mail Job"."Table No."::OnValidate
Page "CDO E-Mail Templates".action(ExportTemplateAction "Export Template")::OnAction
PageExtension CDOSalesInvoiceExt.action(CDOOpenEMail "Open Email (Draft)")::OnAction
Codeunit "CDO Log Management"::CreateSMTPLogEntries(Record "CDO E-Mail Template Line", RecordRef, Variant, Text, Text, Text, Text, Boolean, Boolean)
```

Rules: the parameter list prints only when the name is overloaded in its object (the rest stay
short); in a path, a hop prints its parameters when the next or previous hop has the same name
(wave-2 §3.8 rule 6, now applied by every renderer, tests and impact included); the member
(field, action, data item) prints for every trigger that has one. JSON gains `signature` (the
parameter types) and `member` (`{kind, name, caption}`) beside `display` and `stableId`. The
`/0#1` handle stays in JSON as `handle` only.

Where the data is: workspace routines have the member in the model; graph nodes carry `member`,
`objectName`, `memberName` and `arity` (call-graph spec, node table). No engine change.

**Files:** one formatter in `src/engine/l5/routine_rollup.rs` (next to `handle`), read by
`explain.rs`, `digest_cli.rs`/`digest_v3.rs` (summary rows), `effect_query_cli.rs`,
`field_touches.rs`, `prove.rs` and `callers_context.rs`, `test_select/render.rs`, `impact/render.rs`.

### 2.2 Dependency routines by name (F4)

**Current** (c02, c12): `Codeunit 448::handlerequest`, `Table 110::applies-to doc. no.::onlookup`,
`Codeunit 6225383::executerunner`. **New:** `Codeunit "Job Queue Dispatcher" (448)::HandleRequest`,
`Table "Sales Invoice Header" (110)."Applies-to Doc. No."::OnLookup` — the declared case, the name
first, the number in brackets. Read from the graph node's `objectName`/`memberName`/`member`, not
from its id. Same files as §2.1.

### 2.3 The resolved target in prove (F6)

**New:** prove JSON `payload.result.table: {id, name, app}` for `writes-table`, and
`payload.result.event` for `publishes-event`; the human answer line names the table as resolved:
`writes-table:CDO E-Mail Job (table 6175283, this app)`. Prove 2.5.0 (additive). Note: a
misspelled table already fails (`unknown table '…'`, `prove.rs:2574`); the echo is for a number
or a name that two apps share.

### 2.4 Selector failures that help the next call (F7, F8)

**Current** c03: `explain: no routine of the program has the id or name 'SendMailTwice'` (exit 2).
c26: one 1,000-character line with both overloads.

**New** c03:
```
explain: no routine in the loaded program is named 'SendMailTwice' (exit 2: no answer)
  nearest names: Codeunit "CDO Send Mail Management"::SendMail (6 overloads); Codeunit "CDO E-Mail"::Send (…)
  selector forms: Object::Routine, Object::Routine(<parameter types>), or a stable id
```
**New** c26:
```
explain: 'Codeunit "CDO E-Mail Template Management"::PrintDocuments' names 2 routines (exit 2: no answer)
  1. PrintDocuments(Record "CDO E-Mail Template Header", …, var Record "CDO File" temporary)           :1117
  2. PrintDocuments(Record "CDO E-Mail Template Header", …, var Record "CDO File" temporary, Boolean)  :1122  (+ Boolean)
  both: routines: ["…::PrintDocuments(<list 1>)", "…::PrintDocuments(<list 2>)"]
```
Nearest names come from the selector index (edit distance on the routine name, then same object);
at most 5. The ambiguity stays an error (astra's recommendation; see §11). Files:
`selector_index.rs`, the selector error paths of explain, prove, digest, query.

**Schema:** explain `al-sem.explain/4` gets additive `signature`/`member`; digest 3.1.0 (summary
rows gain `signature`, `stableId`); query 3.6.0; tests and impact keep version 1 (additive).
**Goldens:** every `--test cli` family with a routine name: `cli-explain-goldens`, `cli-query-goldens`,
`cli-b-goldens` (prove), `cli-output-goldens` (fixture, `impact/`, `cdo/`), the tests and impact
inline goldens. **Consumers:** ConTinAI and bc-dev-mcp key on `stableId`, not on `display` (plan
§4.1); no change. Tell al-lsp-for-agents' owner that display strings change (nothing parses them
today). **Verify (round 2):** c15 and c25 summaries show distinct signatures; c01 and c23 headers
name the field; c05, c06, c12, c17 name actions; c02, c12 paths name dependency objects; c03
lists `SendMail`; c26 offers the two-selector value.

---

## 3. Tiers on witnesses (V2)

### 3.1 prove: possibilities are labelled where they are printed (F10)

**Current** c02:
```
  evidence: 47 witnesses (nearest first)
    [non-interactive-entry] Codeunit 448 is started in a background session or task (at dep:…:282) at dep:…:282 (14 hops, onlyOpen)
  non-interactive entries: background-start 9, install-codeunit 3, upgrade-codeunit 35
```
**New:**
```
  answer:   unknown — no caller reaches it except over a hub (an unresolved Codeunit.Run, Page.Run or record trigger)
  established witnesses: none
  candidates over a hub: 47 (background start 9, install 3, upgrade 35); nearest:
    [via-hub] job queue: Codeunit "Job Queue Dispatcher" (448) → … → [hub Codeunit.Run at JobQueueStartCodeunit.Codeunit.al:32] → Codeunit "CDO Auto-Send Runner"::OnRun → …
```
Rules: an answer with only hub-mediated witnesses prints `established witnesses: none` before
any candidate; every witness line starts with its tier; entry-kind counts of candidates are
labelled as candidates. The JSON already carries the tier per witness; it gains
`establishedWitnesses` and `candidateWitnesses` counts (prove 2.5.0). **Files:** `callers_context.rs`
`format_human`, `prove.rs` evidence rendering.

### 3.2 A reach that is the whole program says so (F12)

**Current** c12 (and c20, per routine and again for the whole change):
`reached from: 6,043 page actions (first: Page 21 …), 10,094 page triggers …, 27,718 ancestors`.
**New:**
```
  reached from: this action only by code (0 code callers)
  beyond it, by UI navigation and hubs: 27,718 ancestors (94% of the program's entries) — not specific to this change
    by tier: local 0, via-dependency 4,631 page actions, via-hub 1,412; detail: true lists the first 10 of each
```
(The share and the tier split are illustrative; the fact check's own counts are in
`factcheck/c11-c20.md`, c12.)
Rule: when the ancestors over navigation or hubs exceed a set share of the program's entries
(start at 50%, measured on CDO), the per-kind lists are replaced by this one line; lists come
back with `detail: true`. Code callers (direct and transitive, by code calls) always print first.
The fix branch already prints the tier on `first:` items (F11, rejected as done); this builds on
it. **Files:** `impact/render.rs` (`reached_clause`, the whole-change block), `impact/up.rs` for
the share. [engine] note: `fix/reach-and-test-selection` narrows `declaredRunnable`; the share is
measured after it lands.

### 3.3 Tests: tier per group, local first (F13)

**Current** c28: groups list `68960: 18 tests, 7 hops via Page CDO Email Editor → OnAction → …`
with no tier; c07: `run first — local path: none; nearest test: … (4 hops, through dependency code)`.
**New:** every group and the nearest line carry the tier; ranking is tier first (local, then
via-dependency, then via-hub), then hops; a group whose path enters through UI navigation (a test
page opened, no action invoked) says `opens the page; no action of this path is invoked` when the
test's own body shows that. c07's line becomes `no local test; nearest via dependency code: …
(not a focused test)`. JSON: `runFirst[].tier` exists; add `group.tier`. **Files:**
`test_select/render.rs`, `test_select/select.rs` (rank key).

**Verify (round 2):** c02 prints `established witnesses: none`; c12 and c20 print one
"not specific" line, code callers first; c28 groups show tiers and local groups lead; c07 says
"not a focused test".

---

## 4. Negative and conditional verdict wording (V2)

### 4.1 A conditional write is a write risk, not a "no" (F14)

**Current** c01:
```
  summary — writes: no observed write that may persist; caller-dependent: 1 table
```
**New** (explain, digest human, query effects):
```
  summary — writes: CDO E-Mail Job (Modify at :24) unless the caller's record is temporary; no other write observed
```
Rule: when a card has caller-dependent writes and no may-persist writes, the summary leads with
the caller-dependent ones and says the condition. When the routine is a trigger of the table it
writes (`Rec` is the record being validated, inserted or modified), the condition reads
`unless Rec is temporary (only a temporary caller record makes it one)`. This changes wave-2
§3.8 rule 2 (order), which put may-persist writes first unconditionally. The JSON is unchanged.
**Files:** `routine_rollup.rs` summary line, `effect_query_cli.rs` summary parts.

### 4.2 Field writers: a count with its search (F17)

**Current** c10: `field: CDO Setup.Document Folder (caption "Document Folder"): 0 writer(s)`.
**New:**
```
field: CDO Setup."Document Folder" (caption "Document Folder"): 1 writer observed
  Table "CDO Setup"."Document Folder"::OnValidate — assigns at CDOSetup.Table.al:97
  searched: 5,479 workspace routine bodies and the dependency bodies loaded; not searched: page columns, CalcFormula, TableRelation
  reads, filters and validations: alsem_query_touches with table and field
```
With none found: `no writer observed (searched: …; not searched: …)`, never a bare `0`. This uses
the same searched/not-searched sentence as touches field mode (`field_touches.rs`), from one
helper. The count is fixed on `fix/field-writers-implicit`; this lands after it. **Files:**
`explain.rs` field mode, `field_writers.rs`.

### 4.3 A filtered negative says what it filtered, and what else matched (F18)

**Current** c27: `answer: no — no reachable site matches … other sites: 3 reachable`.
**New:**
```
  answer:   no FieldError site is reachable (every candidate decided)
  other reachable error sites: 3 — TestField at CDOMergeTableField.Table.al:64; TestField at :65; Get (implicit error) at :526
  to see their routes: op: "TestField", or no op
```
Up to 5 other sites with op kind and location; the rest as a count. The JSON already has
`otherSites` counts; it gains the list (prove 2.5.0). **Files:** `prove_slice.rs`, `prove.rs`.

### 4.4 An unknown names what would decide it (F19, F20)

**Current** c29: `unknown — no witness found, but the cone is not complete … assumptions: none
(apps that depend on this app are not loaded; their calls block a no)`.
c19: `… --closed-world assumes the loaded interface implementations`.
**New** (one line after the answer, only on `unknown`):
```
  plain reading: no loaded caller intercepts the error; unknown only because 2 routines are public
  would decide it: assumeNoDependents: true (assumes no other app calls these 2 routines)
  note: this is about interception; it says nothing about which writes roll back
```
For c19: `would decide it: closedWorld: true (assumes only the loaded interface implementations
exist; it changes the assumption, not what runs)`. The knob is named only when it is the sole
blocker kind. The note on rollback prints for `callers-intercept-errors` only. Owner rule kept:
no "safe" in any output (agent-tasks §5). **Files:** `prove.rs` human, `callers_context.rs`.

### 4.5 Status says how hard it checked (F22)

**Current** c09 and c24 print the same headline: `FRESH: no input changed since generation 1
(checked just now: 582 files, 11 packages).` — c09 hashed every file, c24 compared size and time.
**New:** `… FRESH: no input changed since generation 1 (every input hashed just now: 594 inputs)`
vs `… FRESH: no change seen since generation 1 (size and modification time, just now; verify: true
hashes every input)`. This edits the agent-tasks §2.3 headline table; `check.strength` is added
(`hashed` / `size-and-time`). `statusVersion` stays 2 (additive). **Files:** `src/mcp/freshness.rs`.

### 4.6 Touches table mode says "transitively" and shows sites (F23)

**Current** c30: `1802 routines touch it` and 12 routine headers from one codeunit.
**New:** `1,802 workspace routines touch it transitively; 214 sites in 96 routines touch it
directly` (numbers illustrative), then the direct sites (§9.3). A listed line is a site, never a
routine header. **Files:** `effect_query_cli.rs` touches table mode.

### 4.7 A change mode says what it computed (F27)

**New** first line of a digest or impact answer driven by a `diff` without a base:
`current effects of the 2 routines this diff touches (the diff selects roots; no before/after is
computed — pass base for that)`. With a base, impact already prints its comparison tuple.
**Files:** `digest_cli.rs` human header, `impact/render.rs`.

**Schema:** prove 2.5.0 (additive); explain, digest, query text only. **Goldens:**
`cli-b-goldens` (prove), `cli-explain-goldens`, `cli-query-goldens`, `cli-output-goldens` incl.
`cdo/`, impact and tests goldens. **Consumers:** ConTinAI `verify.py` reads `result.answer`,
`evidence`, `blockedBy`, `assumptions` — unchanged. **Verify (round 2):** c01 and c23 lead with
the conditional write; c10 prints "1 writer observed" with its search; c27 lists the 2 TestField
sites and the Get; c19 and c29 name their knob; c24's headline says "size and modification time";
c30 says "transitively" and shows sites; c20's first line says no before/after.

---

## 5. Result first, and one result header (V4)

### 5.1 The answer leads (F28, F29)

**c06** (explain `callersContext`) **new** first section after the header:
```
  for adding a COMMIT, an error or UI here (callers' context):
    pending writes: YES — 8 callers may hold an uncommitted write when this runs; first:
      Page "CDO Sales Invoice Actions"::OnAfterGetCurrRecord → Rec.Modify at …:NN, then SetReportSelectionUsage at …:276
    intercepted errors: unknown (2 routines are public; assumeNoDependents: true decides)
    non-interactive: unknown (job-queue entries live in data: no roots.config.json)
```
The witness comes from the pending-writes walk the section already runs; explain prints its
first witness instead of only the count. **c17**: the "through dependency code / configured
dispatch" block moves above the caller list. **c21/c24**: covered by §5.2 (summary first).

### 5.2 One result header on every tool, and a summary always (F30, F42, F43)

Every answer starts with the same three lines, and every envelope carries a `summary`, inline or
spilled:
```
<tool> <resolved inputs, canonical names>   [universe, revision generation]
answer: <the one-line result>
complete: <analysis: complete | partial (reasons)>; scope: <one clause>; more: <MCP inputs>
```
Envelope `summary` (≤ 8 KiB, as today) gains, per tool: explain — each card's header and
summary line; prove — answer, established/candidate counts, the deciding knob; impact —
accounting, outcome, first five routines; tests — `selectedTests` (≤ `maxTests`), totals,
truncation, `selectionComplete`, fallback. Digest summary rows (F42): one row per **distinct**
routine (overloads folded with their signatures listed, counts that differ shown), and
`tableAnswers` for every `writesTables` entry (`{answer, blockers, firstBlocker}`); a cut list
says `listed 9 of 20 (distinct routines 6)` instead of an unexplained `countsOnly`. query effects
summary (F43) gains `writes.byTable` (top 10 with op counts). The JSON writer sorts keys, so
"first" in a document can only mean "in the summary"; that is why the summary carries the
selection, not a reordered document.

### 5.3 Boilerplate: keep the facts, drop the prose (F31, F32)

Wave-2 §3.7 says coverage, assumptions and limits are always visible, never folded. That stays.
What changes: when all three are trivial they print as one line, `complete · assumptions: loaded
program · limits: none`; the static scope prose (how dependency code is followed, temp records
are not writes, line base, which flag lists everything) moves to the tool description, where it
already is, and leaves a short clause in the header (`scope: whole cone with literal arguments`).
Empty sections (`extension points: none`, `events: 0 … 0 more`, `rules: none of its own;
inherited: none`, `commits: none` repeated after the summary) are not printed when the summary
line already says it. The dependencies line is one form everywhere: `dependencies: 11 apps`
in the header clause; the full list only with `all`/`detail`; the envelope's `universe` block has
them (finishing wave-2 Q6, which `finish_human` applied only where a `scope:` line exists).

### 5.4 Status answers in one copy (F33)

The text block becomes the headline(s), `check` (fresh, strength, complete, inputs, residual)
and `refresh` per engine, about 400 bytes; `structuredContent` keeps the full JSON (operator
data under `details`, as agent-tasks §2.3 set). This follows owner decision 3 (one copy of the
answer). **Files:** `src/mcp/server.rs` status, `freshness.rs`.

**Schema:** digest 3.1.0 (summary only, additive); envelope `summary` for explain/prove/impact/tests
is new (MCP side, pinned by `mcp_server.rs`). **Goldens:** every human family (the header); the
envelope tests in `tests/cli/mcp_*.rs`; `cli-output-goldens/cdo/*.digest-summary.json`.
**Consumers:** bc-dev-mcp reads digest documents, not the MCP summary; no change. **Verify:**
c06 leads with pending writes and names a caller; c15's summary carries the CDO File answer;
c25's summary has one row per distinct routine; c21's envelope lists the selected tests; c09's
text is under 500 bytes; c01's and c11's dependency list is a count.

---

## 6. MCP-shaped follow-ups (V4)

### 6.1 Name MCP inputs, not CLI flags (F34, F37)

**Current:** `--all lists them` (c04), `--closed-world assumes …` (c19), `alsem prove <routine>
writes-table:<table>` (c11), `alsem diff <base> <new> gives them` (c13), `every one with detail or
format json` (c13), `--tier dependency lists them` (c07), `The next command builds it` (c14).
**New:** the MCP front end appends a hidden `--surface=mcp` to the argv it builds; with it, the
renderers print `all: true`, `closedWorld: true`, `alsem_prove with question writes-table:<T>`,
`tier: "dependency"`, `singleApp: true on any tool`. A command with no MCP tool (`alsem diff`)
is named as "not available over MCP" or dropped. The byte contract holds: the MCP answer equals
the CLI's stdout for the same argv, and the contract tests pass the flag. One helper owns the
mapping (`src/engine/follow_up.rs`, a table flag → MCP input), so no renderer spells a flag.

### 6.2 `next:` only when it applies (F35)

`next:` lines are built from the card: suggest `writes-table` only when the card has a write,
`may-commit` only with a commit, the callers' context questions only when the card has callers.
c01's "alsem prove names the caller" becomes `alsem_prove with question
callers-have-pending-writes shows which callers pass a non-temporary record` only if that
question can answer it; otherwise it is dropped.

### 6.3 What a non-zero exit means (F36)

The envelope gains `outcome`: `answered`, `no-routine-changed`, `not-found`, `ambiguous`,
`usage-error`. Exit 0 for every answered outcome (F24), 2 for no answer. The descriptions say so
once in the server instructions.

**Goldens:** none move without the flag (the CLI default stays CLI-worded); the MCP contract
tests and `output_invariants_w3::descriptions_*` gain the `--surface=mcp` cases; a new invariant
"no `--` flag text in any MCP answer" (break: drop the flag from the front end). **Verify:** c04,
c13, c19, c11, c14 name MCP inputs; c06 has no writes-table suggestion; c13's envelope says
`outcome: no-routine-changed`.

---

## 7. Size and dedup of large answers (V5)

### 7.1 Touches `callers: true` (F38)

**Current** c17: 961 lines, 320 KB; every caller line repeats `12 touches (12 on persistent
records, 0 …); first [CalcFields] read at …:740 in Table CDO E-Mail Log::GetEMailFromStorage, through …`.
**New:**
```
  up: all 952 transitive callers touch CDO E-Mail Log; shared first touch: [CalcFields] at CDOEMailLog.Table.al:740 (GetEMailFromStorage)
  through dependency code: 2 more workspace callers; 81 only via configured dispatch (Workflow Response Handling::ExecuteResponse)
  callers (952 routines in 176 objects), by object, nearest hops first:
    Codeunit "CDO E-Mail Template Management" (9): CreateDocumentsForRecord (1 hop), CreateDocuments(… "CDO E-Mail Template Line" …) (2), …
```
Every caller stays (the task asked for all of them; the 952 rows are 952 distinct routines). The
witness prints once when shared, and per row only when it differs. Target: under 60 KB on c17.
JSON: `up.callers[]` rows refer to a `witnesses` table by id. Query 4.0.0 together with §7.2.

### 7.2 query effects JSON (F40, F43)

**Current** c08: 1,005,860 bytes for 671 rows; `firstHop` and `origin.routine` (display, object,
stable id, anchor) inlined per row; `via: "inherited"` in all 671. **New:** digest 3.0's shape:
`payload.routines` (`r:<n>` → display, signature, object, anchor, stableId) once; rows carry
`firstHop: "r:12"`, `origin: {routine: "r:40", line, column}`; `via` printed only when rows
differ. Target: under 200 KB on c08. **Schema:** query 4.0.0 (fields change shape). **Consumer:**
ConTinAI `effects.py` reads each row's `origin.anchor.line` (plan §4.1): it must read the routine
table. Tell its maintainers before merge; keep `--schema 3` for one release, as digest did
(owner decision 5).

### 7.3 Digest with many roots (F41)

**Current** c25: 1,182,595 bytes for 20 roots of one file, six `SendMail` overloads with ~190
effects each. **New:** effects are shared across roots: `payload.effects` holds each effect once
with `roots: ["r:1","r:4",…]`; per root only its own counts and the effects only it reaches.
`compact: true` stays one line, but the default (indented) form is what the tool documents for
file readers. Target: under 300 KB on c25. **Schema:** digest 4.0.0, `schema: 3` kept one release.
**Consumer:** bc-dev-mcp reads digest 2.x and is pinned to 1.7.1 (`satisfiesPin` refuses another
major), so it is not exposed until its pin moves; its readers (`risk.ts`, `alsem-writes.ts`) then
read `payload.effects` with `roots`. ConTinAI does not read digest.

### 7.4 Tests JSON (F44)

`hidden` becomes `{"<class>": {"reason": "…", "nodes": [...]}}` (reason once per class);
`nearestTestedCaller` omitted when null. `al-sem.tests` version 2 (a field changes shape). No
outside reader yet (bc-dev-mcp adopts it later, agent-tasks Q10).

### 7.5 prove witness paths (F45)

Witnesses that share a tail print it once: `… → [shared 7 workspace hops, see first witness] →
SetFiles`; a location prints once per line (today `(at X) at X`); blocker kinds print as counts
on one line, with the ones that carry an action (an unrecognized guard, a missing
`roots.config.json`) listed first. Target: c02 under 3 KB.

### 7.6 impact detail (F46)

With `detail: true`, each list prints one item per line, ten per list, indented under its kind;
the whole-change block is omitted when the change has one routine; "no base given" prints once
in the header (§4.7). Target: c20 under 4 KB.

### 7.7 explain caller rules (F47)

`caller rule (applied by 2: …; skipped by 41: 5 pages, 14 page extensions — detail: true lists
them)`; ties in "most actions" print as `6 objects with 5 each`.

**Goldens:** `cli-query-goldens` (JSON), `cli-output-goldens` digest JSON and `cdo/` summaries,
the digest size ratchets in `output_invariants*.rs`, tests JSON goldens. The ratchets are lowered
to the new targets in the same commits. **Verify:** c17 < 60 KB with 952 caller names; c08 < 200
KB; c25 < 300 KB; c21 `hidden` once per class; c02 < 3 KB; c20 < 4 KB.

---

## 8. Vocabulary and counting units (V0 decides, V6 applies)

### 8.1 The word table (F49–F52)

One spelling per idea, used in human text and JSON values alike:

| Idea | Word | Replaces |
|---|---|---|
| path tiers | `local`, `via-dependency`, `via-hub` | `onlyOpen`, `only-open`, `only over open sites`, `through an open site (a hub)`, `throughDependency`, `through-dependency`, `localApproximate` (JSON keeps the old values one release beside the new ones) |
| a hub in a path | `[hub Codeunit.Run at <file>:<line>]` | `h:run:codeunit`, `h:record:modify` |
| execution entries | `page action`, `page trigger`, `API page`, `job queue`, `background start`, `install`, `upgrade`, `test` | `trigger-page`, `page-event`, `page-action`, `page triggers` |
| dependency boundary | `dependency call` (the crossing), `dependency routine entered` (the target) | "entry point" in explain/digest/query |
| unknowns | `answer: unknown` (prove only); `table: not resolved` / `table: decided at run time (RecordRef)`; `temp: caller-dependent` / `temp: not known`; `condition: not known` | `isUnknownBucket`, `unknown` as a table, `temp state unknown`, `(unknown)` after an error |
| completeness | always with a subject: `analysis complete`, `selection complete`, `freshness check complete`, `tests reaching it` (never "coverage" for static reach) | bare `complete`, `coverage: complete`, `testCoverage` status words |

### 8.2 Count units (F53, F54)

Every count names its unit and, where two counts could be compared, how they relate: c05
`writes beyond dependency calls: none (1 dependency call writes only temporary records)`; c15
`commits: 1 site (2 more under literal arguments that cannot run)`; c08 summary `671 operations on
81 tables one routine deep; whole cone beyond 14 dependency calls: 397 tables (writes only)`;
c29 `uncaught at 72 entries (30 page actions, 42 page triggers, overlapping callers counted once
per entry)`; c17 `+3 hops`; c22 `consumers: 3 sites, 1 candidate`. Touches tags get a one-line
legend under the first tagged row (`implicit-access: a bare field name inside the table's own
code`). Wave-2 §3.6 is the rule set for the units; this adds "print the unit".

**Schema:** JSON value renames are a minor bump with the old value kept one release (prove
2.6.0, query, impact, tests). **Goldens:** every human family. **Verify:** round 2's bundle has
none of the replaced words (a grep over the 30 answers, also kept as an invariant on the fixture:
break = put one old word back).

---

## 9. Per-tool items (V6)

### 9.1 explain (F55, F56, F67, F48, F72)

- **Calls first** (F55): `calls: Codeunit "CDO E-Mail Template Import/Export"::ExportTemplates
  (:181)`, the direct callees in source order, at most 5 (impact's routine block too).
- **Tables named when few** (F56): `writes: CDO Setup (Modify at :62; assigns <its 3 fields>)`
  when 3 tables or fewer; the field list follows wave-2
  2d (the routine's assignments, on INSERT/MODIFY/VALIDATE only). Impact's `may write 1 table`
  becomes the same.
- **Caller rules** (F67): `caller rule: rejects when not VariantRec.IsRecord; checked by 2
  callers, not checked by 41` ("skipped" reads like a bypass).
- **showSource** (F48): the excerpt shows the executable line (the call or the operation), not
  the declaration; the root's own excerpt starts at its first statement.
- **Own events vs reached events** (F72): `events raised (17, downstream)` and `publishes: none
  of its own` on one line.

### 9.2 query effects (F57)

The summary always prints `locks:` and `reads:` (with `none observed` when zero), since a lock
question (c11) must not depend on a missing line meaning zero.

### 9.3 query touches, table mode (F59, F60)

**New** (c30):
```
--- touches of CDO E-Mail Template Header (table 6175283) ---
  direct: 214 sites in 96 routines — Get 120, FindSet 31, SetRange 28, Modify 12, Insert 9, SetCurrentKey 3 (key-relevant), …
  by object: Codeunit "CDO E-Mail Template Management" 41 sites, Table "CDO Document Template Line" 18, …
  transitive: 1,802 workspace routines reach one of these sites
  key-relevant: SetCurrentKey at …:NN, …; SetRange on key fields at …
    [Get] at CDODocumentTemplateLine.Table.al:1162 in Table "CDO Document Template Line"::…
```
(numbers illustrative). c18's `unknown` table gets the same shape, with each RecordRef site's
receiver and why the table is not resolved, and the warning that an unresolved write can be any
table. Key-relevant uses: SetCurrentKey sites come from the operation index; "SetRange on a key
field" needs the field-use index joined with the table's keys, which field mode already has per
field (`field_touches.rs`, `key-use`). If the join costs more than a second on CDO, ship
SetCurrentKey only and mark the rest [engine]. **Rename completeness** (F60): field mode's verdict
says `not a complete rename inventory: page columns, keys, CalcFormula and TableRelation are not
searched (the compiler finds them)`; searching declaration-level consumers is [engine] (§10).

### 9.4 impact (F62)

`object headers: 25 files, line 1 only — a byte-order mark removed (no declaration changed)` when
the old and new header lines differ only by a BOM or whitespace; otherwise the changed property
names. The accounting has both line texts.

### 9.5 tests (F63, F64)

The selection ends with what to run: `run: codeunits 68960|68934|… (a superset: whole codeunits,
N tests, for 369 selected); then the whole suite (selection incomplete)`. `test scope:` prints
one clause (`1 test app, 1,844 tests; 4 apps skipped: alsem_status lists why`); the
symbol-download paragraph prints once as `missing test-library symbols: 7 apps (Library Assert,
…) — add them to Test/.alpackages` (agent-tasks §13.3 Q3's specific next action, shortened).

### 9.6 status (F65)

`inputs` gains `appJson: 1` (so 582 + 11 + 1 = 594 hashed adds up).

### 9.7 prove JSON (F66)

The prove description and schema note say `deterministic` concerns `generatedAt` only, never the
answer.

**Goldens:** `cli-explain-goldens`, `cli-query-goldens`, impact and tests goldens,
`cli-output-goldens/cdo/`. **Verify:** c05 names the callee; c20 names CDO Setup and its fields;
c11 prints `locks: none observed`; c30 and c18 show ops by kind and direct sites; c13 says "byte-
order mark"; c07 and c28 end with what to run; c09's inputs add up.

---

## 10. Engine notes

Kept apart, per the rubric. None blocks a milestone above.

- **Declaration-level field consumers** (F60, c22): page columns, keys, CalcFormula,
  TableRelation are not indexed. A rename inventory needs them. Output-only stopgap: §9.3.
- **Key-relevant uses in table mode** (F59): possible from existing indexes; measure the join (§9.3).
- **DB-event subscribers** (c01, c11): on `fix/db-event-subscribers`.
- **`declaredRunnable` and page navigation** (c12, c20): the first on `fix/reach-and-test-selection`;
  navigation stays by design and is labelled (§3.2).
- **Deep reads and locks in query effects** (c08, astra): rejected; plan §5 Q4 keeps per-operation
  detail one routine deep.

---

## 11. Where the reviewers disagree

| Topic | Fable | astra | Recommendation |
|---|---|---|---|
| c17 caller list | dedupe by routine, ~40 lines | keep every caller, share the witness | **astra**: the task asked for every caller, and Fable's duplicates are distinct overloads and actions (952 rows = 952 callers, checked). Share the witness, group by object (§7.1) |
| Boilerplate | one short `scope:` line or the envelope | scope and completeness beside the answer, standardized | **both, within wave-2 §3.7**: the three facts stay visible as one line; static prose goes to the description (§5.3) |
| c13 outcome | exit 0 | a distinct outcome code | **exit 0 plus `outcome`** (§1 F24, §6.3): agents treat non-zero as failure; the outcome tells cases apart |
| c19 closed world | name `closedWorld: true` | warn it can hide the problem | **both**: name it and say it changes the assumption, not what runs (§4.4) |
| c12 whole-program reach | omit, "not specific" | a separate optional section | **one line with counts by tier and "not specific"**; lists only with `detail` (§3.2) |
| c26 ambiguity | one card per overload when ≤3 | a copyable request for both | **astra**: a card the agent did not ask for may be the wrong one; the error stays exit 2 with a ready value (§2.4) |
| c19 wording | — | "does not establish safety" | **rejected** (owner rule: no "safe" in output); the knob line covers it |
| c02, c07 | "partly" (layout makes candidates look like evidence) | "yes" | no output decision depends on the verdict; §3.1 and §3.3 apply either way |
| c18 | "partly" | "no" (no operation is identified) | astra's reading: §9.3 makes the answer name operations |

---

## 12. How it is verified

- **Per change:** the test and discrimination proof named in its section, run with the narrowest
  command (`cargo test --test cli <member>::`). Correctness guards (§1, the tier labels of §3, the
  outcome of §6.3) each get one proof; wording, display and size do not (owner decision
  2026-11-04).
- **Per milestone:** the task gate; `scripts/check-goldens --regen` with every moved family read
  and named in the commit; `check-goldens --verify-coverage`; `scripts/cdo-gate` at the branch
  gate (the CDO rendering goldens in `cli-output-goldens/cdo/` move in every milestone); a
  CHANGELOG entry.
- **Round 2:** rebuild and copy `~/.al-sem/bin/alsem` (it does not update itself), re-run the
  same 30 calls with `target/tmp/output-validation/round1/record.py` and `plan.json`, and check
  each call against the "Verify" lines above. The same two reviewers read round 2 against the
  same rubric; a finding that comes back is reopened in the register.

---

## Appendix A. Findings register (round 1)

Columns: class = correctness bug / design change / rejected; priority: high = can mislead an
agent, medium = missing or inconsistent, low = noise or size. "Raised by": fable, astra or both.

### A.1 Counts

| Class | High | Medium | Low | Total |
|---|---|---|---|---|
| correctness bug | 2 | 3 | 2 | 7 |
| design change | 10 | 28 | 18 | 56 |
| rejected | — | — | — | 9 |
| **all** | | | | **72** |

### A.2 Correctness bugs

| id | Summary | Calls | Raised by | Priority |
|---|---|---|---|---|
| F5 | prove callers' context human text prints `dep:<guid>:`; `callers_document` skips `readable_locations` | c02 | both | medium |
| F9 | tests paths print two overloads alike; agent-tasks §12.1 requires handles | c28 | fable | low |
| F15 | query effects counts a `Get` under "caller-dependent writes" (`bucket()`, `effect_query_cli.rs:2991`) | c11 | both | high |
| F16 | parameter 3 (1-based) and `parameter-dependent(2)` (0-based) in one answer | c11 | both | medium |
| F24 | impact exits 2 on a complete "0 changed routines" answer (`impact/render.rs:715`) | c13 | both | high |
| F25 | impact without a base says "signature changed" of a routine the diff adds (`impact/jobs.rs:268-274`) | c20 | both | medium |
| F39 | touches `callers` described as "without witnesses"; each row has one (`tools.rs:485`) | c17 | fable | low |

### A.3 Design changes

| id | Summary | Calls | Raised by | Priority | Spec |
|---|---|---|---|---|---|
| F1 | Overloads by bare name in summaries, JSON display, caller lists, paths | c15, c17, c21, c25, c28 | both | high | §2.1 |
| F2 | Table triggers without the field | c01, c23 | both | medium | §2.1 |
| F3 | Page action triggers: three forms, no action name | c05, c06, c12, c17 | both | medium | §2.1 |
| F4 | Dependency routines by number, lower-cased member | c02, c12, c20 | fable | medium | §2.2 |
| F6 | prove does not echo the resolved table (a misspelling already errors) | c16 | both | low | §2.3 |
| F7 | Not-found selector: no near matches or selector forms | c03 | both | medium | §2.4 |
| F8 | Ambiguous selector: one long line; no ready value for both | c26 | both | low | §2.4 |
| F10 | prove prints hub-mediated witnesses as evidence; tier at line end only | c02 | both | high | §3.1 |
| F12 | Whole-program reach listed as if specific, twice | c12, c20 | both | high | §3.2 |
| F13 | Tests: no tier per group; nearest reads as focused; ranking ignores tier | c07, c21, c28 | both | medium | §3.3 |
| F14 | Conditional write led by "no observed write that may persist" | c01, c23 | both | high | §4.1 |
| F17 | Field writers: bare `0 writer(s)` with no search statement | c10 | both | high | §4.2 |
| F18 | Filtered negative hides the other reachable sites | c27 | both | high | §4.3 |
| F19 | unknown does not name the deciding input or the plain reading | c02, c19, c29 | both | medium | §4.4 |
| F20 | Interception answer may read as a rollback guarantee | c29 | astra | low | §4.4 |
| F22 | Status headline same for hashed and size/time checks | c09, c24 | astra | medium | §4.5 |
| F23 | Touches table mode: no "transitively"; routine headers shown as sites | c18, c30 | both | high | §4.6 |
| F27 | Change modes do not say "current source, no before/after" | c01, c20 | astra | medium | §4.7 |
| F28 | Answer buried (c17 line 957; status; tests selection) | c17, c21, c24 | both | medium | §5.1 |
| F29 | explain callers' context: COMMIT hazard last, no witness | c06 | both | high | §5.1 |
| F30 | No shared result header; no summary for inline explain/prove/impact/tests | all | both | medium | §5.2 |
| F31 | Four boilerplate texts per card; empty sections | c01, c04, c05, c11, c22, c23 | both | medium | §5.3 |
| F32 | Dependencies line in three forms | c01, c04, c17, c18, c22, c30 | fable | low | §5.3 |
| F33 | Status text duplicates the envelope; operator data by default | c09, c14, c24 | both | low | §5.4 |
| F34 | CLI flags and non-MCP commands in MCP text | c04, c11, c13, c17, c19, c28 | both | medium | §6.1 |
| F35 | Static `next:` lines | c01, c05, c06 | fable | medium | §6.2 |
| F36 | Exit-code meaning not in the envelope | c03, c13, c26 | fable | low | §6.3 |
| F37 | No-engine status does not name `singleApp: true` | c14 | both | low | §6.1 |
| F38 | Touches callers: 320 KB of repeated witnesses (reviewers disagree, §11) | c17 | both | medium | §7.1 |
| F40 | query effects JSON 1 MB: routine identities per row, constant `via` | c08 | both | medium | §7.2 |
| F41 | Digest repeats overlapping cones per root | c15, c25 | both | medium | §7.3 |
| F42 | Digest summary: no `tableAnswers`; 9 of 20 routines with unexplained `countsOnly` | c15, c25 | both | high | §5.2, §7.3 |
| F43 | query effects summary: no writes by table | c08 | fable | medium | §5.2 |
| F44 | Tests JSON: `hidden` reason ×47; null fields; selection not in summary | c21 | both | medium | §7.4 |
| F45 | prove paths: shared tails repeated; blocker counts; `(at X) at X` | c02 | both | medium | §7.5 |
| F46 | impact detail: 3,000-character lines; whole-change census repeated | c12, c20 | both | medium | §7.6 |
| F47 | explain caller-rule line 1,900 characters | c06 | both | low | §7.7 |
| F49 | "unknown" with five meanings | c05, c08, c11, c18 | both | medium | §8.1 |
| F50 | "complete"/"coverage"/"reached" without a subject | c09, c21, c25, c28 | astra | medium | §8.1 |
| F51 | "entry point" with two meanings | c05, c06, c11, c12 | astra | medium | §8.1 |
| F52 | Tier and entry-kind words spelled many ways | c02, c06, c07, c12, c20, c29 | fable | medium | §8.1 |
| F53 | Count units not stated or reconciled | c05, c08, c15, c17, c22, c29 | both | medium | §8.2 |
| F54 | Touches tags without a legend | c22 | both | low | §8.2 |
| F55 | No direct callees in explain and impact | c05, c12 | both | medium | §9.1 |
| F56 | Tables (and fields) not named when few | c20 | both | medium | §9.1 |
| F57 | query effects: no `locks:`/`reads:` line when zero | c11, c23 | both | medium | §9.2 |
| F59 | Touches table mode: no ops, no direct sites, no key-relevant uses | c18, c30 | both | high | §9.3 |
| F60 | Rename: declaration-level consumers not searched; say so [engine] | c22 | both | low | §9.3, §10 |
| F62 | impact: header change kind not said (a BOM) | c13 | both | low | §9.4 |
| F63 | tests: no runnable target; filter superset not said | c07, c28 | both | low | §9.5 |
| F64 | tests: scope and symbol paragraphs repeat | c07, c28 | both | low | §9.5 |
| F65 | status inputs omit app.json (594 = 582 + 11 + 1) | c09 | astra | low | §9.6 |
| F66 | prove `deterministic: false` unexplained | c16 | astra | low | §9.7 |
| F67 | "skipped by" reads as a guard bypass | c06 | astra | low | §9.1 |
| F48 | showSource excerpts show declarations, not the call | c04, c05 | both | low | §9.1 |
| F72 | Own events vs reached events not told apart | c05 | astra | low | §9.1 |

### A.4 Rejected

| id | Summary | Calls | Raised by | Reason |
|---|---|---|---|---|
| F11 | impact `first:` items without tier | c12, c20 | both | already fixed on `fix/reach-and-test-selection` (`node_short`, `reached_clause`) |
| F21 | prove should say "does not establish safety" | c19 | astra | owner rule (agent-tasks §5): "safe" appears in no output; F19 carries the useful part |
| F26 | digest `testCoverage` "complete" with 0 tests | c25 | both | already fixed on `fix/reach-and-test-selection` (`NoTestCode`, `test-apps-not-loaded`) |
| F58 | limit line "DB-event subscribers not followed" | c01, c11 | fable | already fixed on `fix/db-event-subscribers` (the cone follows them); a stopgap only if that branch slips |
| F61 | deep dependency reads and locks in query effects | c08 | astra | plan §5 Q4 / wave 3a decision: per-operation detail one routine deep |
| F68 | no bucket for page triggers among callers | c06 | fable | already fixed on `fix/explain-callers-counts` |
| F69 | upgrade helpers counted as entries; guard line | c02 | fable | already fixed on `fix/explain-callers-counts` |
| F70 | runFirst prefers add-in-event paths | c21 | both | already fixed on `fix/reach-and-test-selection`; tier ranking stays in F13 |
| F71 | "UI and errors: none" for an added `Modify()` | c01 | fable | already fixed on `fix/field-writers-implicit` |

Ids are not in theme order: they were numbered as findings were merged.

# Output validation, round 1: what the MCP answers should look like (design spec)

**Status:** design only, revision 2 (after the review of revision 1 by Fable and astra, both
"accept with the listed changes"). No code lands with this file.
**Date:** 2026-11-06. **Based on:** master `6e577c24`, binary al-sem 2.1.1.
**Reviews of revision 1:** `target/tmp/output-validation/round1/review/spec-review-fable.md`
and `…/spec-review-astra.md`. What each item changed: "Revision 2" below.
**Input:** 30 MCP calls recorded on the CDO baseline (`bc3ccb18`), each with the task an agent
had; a fact check of every claim against the AL source; two independent reviews of the outputs
against one rubric (Fable and astra). All of it is under
`target/tmp/output-validation/round1/` (local, not committed): `review/review-bundle.md` (the
calls), `bundle.json` (full answers), `factcheck/`, `review/review-{fable,astra}.md`.
**Register:** every finding, merged across reviewers and calls, is in Appendix A (ids F1–F85;
F73–F85 added in revision 2). This spec covers the design changes and the new correctness bugs.
Findings another branch fixes are in the register as "handled elsewhere".
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

## Revision 2: what each review item changed

"Fable n" and "astra n" point at the must-change lists (§6 of each review); "should" items and
section comments are named by their topic. Every must-change is applied. One should-change is
declined in part (marked). Where the two reviewers conflict, the decision and its reason are in
§11.2.

### R2.1 Must-change items

| Item | Change made |
|---|---|
| Fable 1 (display is not a selector) | §2.1: the selector grammar gains the member forms `Object."Field"::Trigger` and `Object.action(Name)::OnAction` (caption optional on input, ignored); JSON gains `selector` beside `display`; a round-trip test (every emitted selector resolves to its own stable id). Merged with astra's "display ≠ selector, round-trip tests" (§11.2 row 1) |
| Fable 2 (knob only when sole blocker deletes c02's line) | §4.4 rewritten: every input that removes a blocker kind is named, with what it removes and what stays. Merged with astra's objection to "would decide it" (§11.2 row 2) |
| Fable 3 (reach sketch mixes units; single-hub fan-out; name the hub) | §3.2: one unit per line; folding also fires on one hub's fan-out; the hub is named |
| Fable 4, astra (bare `complete` in the header) | §5.2, §5.3: `analysis: complete` / `analysis: partial (…)`; never bare |
| Fable 5 (word table gaps) | §8.1 rewritten: tier precedence, configured dispatch, a closed hub-kind list, resolution words, negative-certainty row, witness row, entry kinds, "entry" banned alone |
| Fable 6 (F42 `tableAnswers`, F57 into V2) | §0.1: both moved to V2 |
| Fable 7, astra (F15 also the temporary arm, all op families) | §1 F15: both non-physical classes split by op family (reads, locks, validations, writes); read and lock totals count every class. Already passed to the V1 fix branch |
| astra: Appendix A omissions, priorities, dispositions; restore F21 | Appendix A: new rows F73–F79 (astra's missing items) and F80–F85 (Fable's six low items); F20, F63 raised to medium, F22, F50 to high; the seven "already fixed" rows get their own class **handled elsewhere** (an integration and round-2 obligation, §12.2), not "rejected"; F21 reversed (§4.4); F61 split into an accepted disclosure (design) and a deferred engine capability (§10) |
| astra: tier, relationship, configured dispatch, projection defined apart | §8.1: `tier` (with precedence), `relation` (code / UI navigation), configured dispatch as its own tier, `projection: approximate` kept as its own flag |
| astra: c02 all-callers claim; "would decide it" | §3.1 answer line scoped to non-interactive routes and "established by the static model"; §4.4 "re-analysis under an assumption" |
| astra: one identity and verdict per overload; per-routine test status | §5.2: digest summary keeps one row per stable id (shared counts factored, never folded); each row has its `tableAnswers` and test status (F77); §2.1 signatures on every occurrence of an overloaded name |
| astra: standalone machine-readable scope per operation family, deep writes, test selection | §5.2 JSON `result.scope` block in every document (F76); query's per-family scope (F61) |
| astra: access kind, temp state, target resolution, reachability independent; lock/read totals | §1 F15; §8.1 resolution and temp rows; §9.2 lock scope |
| astra: lossless dedup | §7.2, §7.3, §7.4, §7.5: shared intrinsic facts plus per-root/per-row annotations; a reconstruction test per shape (§12.1) |
| astra: field references vs numeric candidates; filter result; declared keys | §9.3: separate inventories, `filters:` line always; the "keys are not searched" claim was wrong (`field_touches.rs` reads `declared_keys`) and is corrected in §9.3, §10, F60 |
| astra: c17 other-branches relation, identities, population counts | §7.1 |
| astra: run target first; superset label; scope, cap, fallback first | §9.5 (tests), §5.2 (impact and digest summaries); F63 moved to V2 |
| astra: complete MCP follow-ups; no false gates; empty analysis vs unresolved input | §6.1 `{tool, arguments}`; §6.2 gates; §1 F24 and §6.3 `input-not-mapped` |
| astra: summary truncation, JSON compatibility, three schema majors, semantic gates | §5.2 cap rules; §8.3 migration rule; §0.1 and §7: three majors (query 4, digest 4, tests 2); §12.1 semantic tests |

### R2.2 Should-change items and section comments

| Item | Disposition |
|---|---|
| Fable §6.1: one MCP-surface fixture golden per tool | applied, §6.1 |
| Fable §3.1: answer line names the actual hub kind | applied, §3.1 |
| Fable §4.1: order when both classes exist | applied, §4.1 |
| Fable §2.4: name found only in a dependency | applied, §2.4 |
| Fable: six low items | applied, F80–F85 in §5.3, §7.5, §9.1, §9.5 |
| Fable §12: dated fix-branch gating table | applied, §12.2 |
| Fable §2.1: quoting rule; overload rule on list rows too | applied, §2.1 (the rule is now "every occurrence of an overloaded name") |
| Fable §5.2: explain's `answer:` line | applied, §5.2 (the card's summary line) |
| Fable §8.1 #4: one pair `not resolved` / `run-time` on every axis | **declined in part**: `not resolved` is adopted on every axis; `run-time` is not, because the engine cannot show a target is a run-time value (astra: some RecordRefs resolve statically). §11.2 row 5 |
| astra: captions in headers and legends, not on every hop | applied, §2.1 |
| astra: near matches thresholded; complete selectors in c26 | applied, §2.4 |
| astra: c06 "8 witnesses", own state changes, every blocker kind | applied, §5.1 (F75) |
| astra: c20 company context | applied as a design row (F75) with an [engine] check: whether the model records `ChangeCompany` on a record variable is not verified (§10) |
| astra: tests group tier distribution, navigation wording, execution before navigation | applied, §3.3; F13's "ranking ignores tier" narrowed: `select.rs:457` already sorts by (tier, hops); the defect is the rendering and the missing relation axis |
| astra: c02 editing implication; c29 uncaught path under `detail` | applied, F79 in §4.4 |
| astra: field-writer definition location, categories, assignment vs persistence | applied, §4.2 |
| astra: filter carried into summary and JSON | applied, §4.3 |
| astra: §4.1 wording ("may modify persistent … when `Rec` is non-temporary") | applied, §4.1 |
| astra: "no interception was found in analyzed callers" | applied, §4.4 |
| astra: freshness compact answer keeps strength, revision, limits, `not checked`; derive input counts | applied, §4.5, §5.4, §9.6 |
| astra: diff mode says whether the diff matched and what was actually computed | applied, §4.7 |
| astra: three-line header for human output only; JSON structured | applied, §5.2 |
| astra: keep the full assumption sentence and answer-specific limits beside the answer | applied, §5.3 |
| astra: `singleApp` on an analysis request; may-commit offered when unknown; drop the pending-writes claim; `--` invariant scope | applied, §6.1, §6.2 |
| astra: query operation location kept apart from routine anchor; `via` default | applied, §7.2 |
| astra: impact detail as an evidence budget | applied, §7.6 |
| astra: caller rules "no equivalent check recognized in 41 analyzed callers" | applied, §7.7, §9.1 |
| astra: c29 overlap rule before "72 entries" | applied, §8.2 |
| astra: F55 short route, F56 qualifications, F57 lock scope, `Get` in key evidence, BOM only | applied, §9.1–§9.4 |
| astra: move completeness subject, candidates, lock qualification, exact selection, F55/F56/F48 earlier | applied, §0.1 (V2 gains F13, F22, F42-part, F48, F50, F55, F56, F57, F63) |
| astra: defer the percentage heuristic | applied as astra asked: the share only folds lists, it never sets a label (§3.2) |
| astra: `unknown` allowed wherever a verdict exists | applied, §8.1 |

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
| V0 | **Word table** (decision only): §8.1 and the migration rule §8.3, approved and dated before any renderer uses them | F49, F51, F52 | — | — |
| V1 | **Correctness bugs** | F5, F9, F15, F16, F24, F25, F39 | S, 7 small commits | — |
| V2 | **What misleads**: verdict and tier wording, silent lines, the evidence a hazard needs | F10, F12, F13, F14, F17, F18, F19, F20, F21, F22, F23, F27, F29, F42 (`tableAnswers` only), F48, F50, F55, F56, F57, F63, F75, F79 | L | V0; the fix-branch gates of §12.2 |
| V3 | **Identities** and the selector grammar | F1–F4, F6–F8 | M | — |
| V4 | **Result header, summaries, follow-ups** | F28, F30–F37, F42 (the rest), F43, F61, F76, F77, F78, F81, F85 | M | V0, V2, V3 |
| V5 | **Size and dedup** (three schema majors: query 4.0.0, digest 4.0.0, tests 2) | F38, F40, F41, F44–F47, F83, F84 | L | V3 (rows refer to routine ids), V4 (the `result` contract) |
| V6 | **Counting units and per-tool items** | F53, F54, F59, F60, F62, F64–F67, F72, F73, F74, F80, F82 | M | V0 |

Order: V1 first (small, and two of its bugs feed V2 text). V0 is a one-page owner decision,
signed off here before V2 starts: **V0 approved: (date, owner) — open.** V2 and V3 next, in
either order. V4 after V2 and V3; V6 after V0. V5 last: three schema majors with consumer
notices, and nothing in it misleads.

---

## 1. Correctness bugs (V1)

Each is its own commit with a test that pins the use (the scenario, not the helper) and one
discrimination proof (break the fix, watch the test fail, revert).

| id | Bug | Fix | Test (discrimination break) |
|---|---|---|---|
| F15 | query effects files a `Get` under "caller-dependent writes": c11 prints `caller-dependent writes 2 on 1 table` then `Purchase Header: Get 1, Modify 1`. `effect_query_cli.rs:2991` `bucket()` returns `callerDependent` before it reads `op_family`; the `Temporary` arm returns before it too (Fable), so a temporary `Get` reads as "temporary records" work | Split **both** non-physical classes by op family as the physical class is split: writes, reads, locks, validations and other families each keep their class as a note (`caller-dependent`, `temporary`). The read, lock and validate totals count every class (a temporary or caller-dependent lock is a lock). JSON `writeTables` loses the read rows; a minor query bump (3.6.0), since a row moves bucket. Passed to the V1 fix branch | fixture routine with, on one caller-dependent parameter, a `Get`, a `LockTable` and a `Modify`, and on a temporary record a `Get`: writes count 1, reads 2 (one caller-dependent, one temporary), locks 1 (break: return the class for every op again) |
| F16 | One answer, two bases: `(caller-dependent: parameter 3)` (1-based, `temp_note`) and `temp parameter-dependent(2)` (raw 0-based, `temp_state_str` in the `--all` rows) | Human rows print the same words as `temp_note`: `caller-dependent: parameter 3 (ToPurchaseHeader)` with the parameter name. JSON keeps `parameterIndex` (0-based) and adds `parameterPosition` (1-based) and `parameterName` | the c11 shape in the fixture: both lines name position 3 (break: print the raw index) |
| F24 | impact exits 2 when the change has no routine (c13), although the accounting is complete. `impact/render.rs:715` | Exit 0 with `outcome: "no-routine-changed"` (see F36) **only when the accounting is complete** (no unmapped line, no file not mapped, no diff line that fails to match the working tree). When no routine resolved because input did not map, the outcome is `input-not-mapped` and the exit stays 2 (no answer) | `cli_impact.rs`: a header-only diff exits 0 with `no-routine-changed`; a diff whose lines do not match the tree exits 2 with `input-not-mapped` (break: restore the `roots.is_empty()` rule; break 2: drop the completeness condition) |
| F25 | impact without a base says `— signature changed` of a routine the diff adds (c20). `impact/jobs.rs:268-274` sets the status from "a changed line falls on the signature" | Without a base, the status says what the diff shows: `new in this diff (every line added)` when all of the routine's lines are `+` lines, else `signature lines changed in the diff`. "Signature changed" is kept for a base comparison (`delta.pairing == signature`) | the c20 patch shape: an added routine prints `new in this diff` (break: drop the all-added check) |
| F5 | prove's callers' context answers print `dep:<guid>:path` in human text (c02); the rule is `dep:<app name>:path` (plan 2g, wave-2 §3.8 rule 5). `callers_document` (`prove.rs` ~L2981) never calls `dependencies.readable_locations`, which the effects path calls (`prove.rs:2711`) | Call it in `callers_document`. JSON keeps the guid (the ConTinAI citation reader needs it) | the hygiene test `human_outputs_are_clean_and_within_budget` gains a callers' context answer and asserts no `dep:<36-char guid>:` in human text (break: remove the call) |
| F9 | tests paths print two overloads alike: `Codeunit CDO E-Mail::Send → Codeunit CDO E-Mail::Send` (c28). Agent-tasks §12.1 requires an overload handle in every path; `test_select/render.rs` has none | Print the handle (`Send/1` → `Send/2`) through the same `RollupCx::handle` rule impact uses; after V3, the signature form of §2.1 | a fixture test whose path passes two same-name overloads: the two hops differ (break: print the bare display) |
| F39 | The touches `callers` input says "without their witnesses" (`tools.rs:485`, `:460`); every caller line carries one (c17) | V1 corrects the description to what is printed ("each with its first touch"). V5 (§7.1) changes both together to "each caller once; its first touch only when it differs from the shared one" | `descriptions_name_exactly_the_inputs_the_schemas_take` gains a check that the `callers` text matches the output's row shape on the fixture (break: revert the text) |

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

Rules:
- **Overloads:** the parameter list prints on **every occurrence** of a name that is overloaded in
  its object — header, list row, path hop, summary row — and never on a name that is not (the rest
  stay short). This replaces wave-2 §3.8 rule 6 (hop neighbours only), which leaves a lone
  `SendMail` hop and c17's list rows ambiguous. Parameter modifiers (`var`, `temporary`) and full
  types are kept.
- **Members:** the member (field, action, data item) prints for every trigger that has one. The
  caption prints in headers and legends only, not on every path hop (astra: noise).
- **Quoting:** an object, field or action name is quoted when AL would need quotes (a space or a
  reserved character), and only then: `Codeunit "CDO Log Management"`, `PageExtension CDOSalesInvoiceExt`.
- **Apps:** two loaded objects with one name keep today's app disambiguation (`routine_rollup.rs`).
- **JSON:** gains `signature` (the parameter types), `member` (`{kind, name, caption}`) and
  `selector` (below) beside `display` and `stableId`. The `/0#1` handle stays in JSON as `handle` only.

**Display and selector** (Fable 1, astra). Today `selector_index.rs` accepts `Owner::Routine` and
`Owner::Routine(<types>)` only, so a trigger display could not be pasted into the next call. V3
extends the grammar with the member forms `Object."Field"::Trigger` and
`Object.action(Name)::OnAction` (a caption after the name is accepted and ignored on input), and
every JSON row carries `selector`: the shortest form that resolves to that one stable id. The
human display is a valid selector except for the caption. Test: every `selector` emitted in the
fixture's and CDO's goldens resolves to its own stable id (break: drop the member forms from the
grammar).

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
  1. PrintDocuments(<9 parameter types>)           :1117
  2. PrintDocuments(<the same 9>, Boolean)          :1122  (+ Boolean)
  both: routines: ["<complete selector 1>", "<complete selector 2>"]
```
The sketch abbreviates; the real output prints both parameter lists in full and the `both:`
value holds two complete selectors (or the stable ids), ready to send. Nearest names come from
the selector index (edit distance on the routine name, then same object), at most 5, and only
names within a distance threshold (set on the fixture and CDO; "no close match" when none
passes): five distant names are worse than none. When the name exists only in a dependency app,
the answer says so (`found only in Base Application: Codeunit "X"::Y; selectors are scoped to the
workspace app`) instead of listing unrelated workspace names. The ambiguity stays an error
(astra's recommendation; see §11). Files: `selector_index.rs`, the selector error paths of
explain, prove, digest, query.

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
  answer:   unknown — no non-interactive route without a hub was established in the analyzed graph;
            47 candidates pass a hub (Codeunit.Run of a variable at OnpremSessionHandler.Codeunit.al:43,
            at JobQueueStartCodeunit.Codeunit.al:32; record trigger (Modify) at CDODataUpgrade.Codeunit.al:128)
  established witnesses: none
  candidates over a hub: 47 (background start 9, install 3, upgrade 35); nearest:
    [via-hub] job queue: Codeunit "Job Queue Dispatcher" (448) → … → [hub Codeunit.Run at JobQueueStartCodeunit.Codeunit.al:32] → Codeunit "CDO Auto-Send Runner"::OnRun → …
```
Rules: the answer line is about the question's own population (here non-interactive routes, not
all callers: c17 shows SetFiles has many code callers) and names the hub kinds actually on the
candidate paths, at most three sites, never a generic list (Fable). "Established" means
established by the static model, not observed. An answer with only hub-mediated witnesses
prints `established witnesses: none` before any candidate; every witness line starts with its
tier; entry-kind counts of candidates are labelled as candidates. The JSON already carries the tier per witness; it gains
`establishedWitnesses` and `candidateWitnesses` counts (prove 2.5.0). **Files:** `callers_context.rs`
`format_human`, `prove.rs` evidence rendering.

### 3.2 A reach that is the whole program says so (F12)

**Current** c12 (and c20, per routine and again for the whole change):
`reached from: 6,043 page actions (first: Page 21 …), 10,094 page triggers …, 27,718 ancestors`.
**New:**
```
  reached by code: this action only (0 code callers; nearest publisher: none)
  reached by UI navigation: 4,820 of the program's 16,137 entries (page actions 5,393, page triggers …) — folded: not specific to this change
  reached over hubs: 1,412 entries, after [hub page variable "Sales Invoice" in Codeunit "Document Errors Mgt." (9070), declared, never run]
    entries by tier: local 0, via-dependency 4,631, via-hub 1,412; detail: true lists one representative per kind
```
(Numbers illustrative; the fact check's own counts are in `factcheck/c11-c20.md`, c12.)

Rules:
- **Labels never depend on size** (astra). Every reach is split by relation (`code`, `UI
  navigation`) and by tier, whatever its population. Code reach (direct and transitive callers,
  publishers) always prints first, with its nearest witness, even when the other lines fold.
- **One unit per line** (Fable). A line counts entries (page actions, page triggers, job-queue
  and background starts, …) or ancestors, never both; a share has numerator and denominator of
  the same population (entries reached / entries in the program).
- **Folding** (lists replaced by the count line, back with `detail: true`) fires when the entries
  reached over navigation or hubs exceed a share of the program's entries (start at 50%,
  measured on CDO), **or** when one hub alone reaches more than that share (Fable: a single
  `Codeunit.Run` hub reaching 90% should fold even if the total is 45%). The share only folds; it
  never makes a reach "not real" (astra: a widely used routine can truly reach most of the program).
- **The hub is named** on the line, with its kind from the closed list of §8.1: it is what an
  agent would fix or ignore.

The fix branch already prints the tier on `first:` items (F11, handled elsewhere); this builds
on it. **Files:** `impact/render.rs` (`reached_clause`, the whole-change block), `impact/up.rs`
for the counts per relation and the share. [engine] note: `fix/reach-and-test-selection`
narrows `declaredRunnable`; the share is measured after it lands.

### 3.3 Tests: tier per group, local first (F13)

**Current** c28: groups list `68960: 18 tests, 7 hops via Page CDO Email Editor → OnAction → …`
with no tier; c07: `run first — local path: none; nearest test: … (4 hops, through dependency code)`.
**New:** every group and the nearest line carry the tier and the relation. A codeunit group whose
tests differ in tier shows the distribution (`18 tests: local 0, via-dependency 18`) and labels
its representative path's tier. A witness that needs UI navigation says `this witness requires UI
navigation; invocation of the action is not established` (astra: the test's own body is not
enough to claim "no action is invoked"; helpers and handlers matter). c07's line becomes `no
local test; nearest via dependency code: … (not a focused test)`. JSON: `runFirst[].tier`
exists; add `relation` and `group.tiers`.

Ranking: `select.rs:457` already sorts tests by (tier, hops, codeunit, name), so revision 1's
"ranking ignores tier" was wrong (astra). What is missing is the relation: a `local` path that
needs UI navigation ranks ahead of a `via-dependency` code path today. The rank key becomes
(relation: code before navigation, tier, hops, …). **Files:** `test_select/render.rs`,
`test_select/select.rs` (rank key).

**Verify (round 2):** c02 prints `established witnesses: none` and names the hub sites on its
candidate paths; c12 and c20 split reach by code / UI navigation / hub, code first with its
nearest witness, navigation folded with the hub named; c28 groups show tier distributions and
code paths lead navigation-only ones; c07 says "not a focused test".

---

## 4. Negative and conditional verdict wording (V2)

### 4.1 A conditional write is a write risk, not a "no" (F14)

**Current** c01:
```
  summary — writes: no observed write that may persist; caller-dependent: 1 table
```
**New** (explain, digest human, query effects):
```
  summary — writes: may modify persistent CDO E-Mail Job records (Modify at :24) when Rec is not temporary;
            otherwise it changes a temporary record; no other write observed
```
Rules:
- When a card has caller-dependent writes and no may-persist writes, the summary leads with the
  caller-dependent ones and states the condition in astra's words above ("may modify persistent …
  when … is not temporary; otherwise it changes a temporary record"): "writes … unless temporary"
  reads as unconditional, and a database write is not yet a committed change.
- When the routine is a trigger of the table it writes (`Rec` is the record being validated,
  inserted or modified), the condition names `Rec` and adds `(Rec is temporary only when its
  caller's record is)`.
- **Both classes** (Fable): may-persist writes first, then the conditional ones **on the same
  summary line** (`writes: CDO Log (Insert); may modify persistent CDO E-Mail Job when Rec is not
  temporary`), never only under a later heading.
- Path conditions and an incomplete analysis qualify the line as they do today (`no observed …`).

This changes wave-2 §3.8 rule 2 (order), which put may-persist writes first unconditionally. The
JSON is unchanged.
**Files:** `routine_rollup.rs` summary line, `effect_query_cli.rs` summary parts.

### 4.2 Field writers: a count with its search (F17)

**Current** c10: `field: CDO Setup.Document Folder (caption "Document Folder"): 0 writer(s)`.
**New:**
```
field: CDO Setup."Document Folder" (field 60, caption "Document Folder", defined at CDOSetup.Table.al:89): 1 writer observed
  Table "CDO Setup"."Document Folder"::OnValidate — assignment at CDOSetup.Table.al:97 (changes the record in memory; persisted by whoever modifies Rec)
  writer kinds searched: assignment, Validate, ModifyAll, TransferFields into the table, by-number RecordRef.Field (candidates)
  searched: 5,479 workspace routine bodies and the dependency bodies loaded; not searched: page columns, CalcFormula, TableRelation; dynamic field numbers: 183 sites not matched
  reads, filters and validations: alsem_query_touches with table and field
```
With none found: `no writer observed (searched: …; not searched: …)`, never a bare `0`. A
writer row says whether it is an assignment, a `Validate` or a record write (astra): an
assignment changes the field in memory, and only a later `Modify`/`Insert` persists it. The
writer kinds listed are those `field_touches.rs` classifies as `write`, `validate` and
`copy-candidate`; the implementing commit takes the exact list from that code. This uses
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
Up to 5 other sites with op kind and location; the rest as a count. The other sites are
labelled as outside the filter. The filter is part of the verdict in every form: the human
answer line, the envelope summary (`filter: {op: "FieldError"}`) and the JSON result
(`result.filter`, `result.otherSites[]`), so a script that reads only `answer: "no"` also reads
what it is "no" about (astra). Prove 2.5.0. **Files:** `prove_slice.rs`, `prove.rs`.

### 4.4 An unknown names what it does not establish, and which assumption changes it (F19, F20, F21, F79)

**Current** c29: `unknown — no witness found, but the cone is not complete … assumptions: none
(apps that depend on this app are not loaded; their calls block a no)`.
c19: `… --closed-world assumes the loaded interface implementations`.
**New** (lines after the answer, only on `unknown`):
```
  plain reading: no interception was found in the analyzed callers; unknown because 2 routines are public
  not established: that no caller turns this error into false; and nothing here says which writes roll back
  re-analysis under an assumption: assumeNoDependents: true — assumes no other app calls these 2 routines;
    removes every blocker of this answer (2 external-caller blockers)
```
c02 (nine blocker kinds):
```
  re-analysis under an assumption: assumeNoDependents: true — assumes no other app calls the public routines;
    removes 438 external-caller blockers; 9,156 hub blockers and 6,710 order-unknown blockers stay
  not established: that a user is present; do not assume an interactive session in code added here
```
c19: `re-analysis under an assumption: closedWorld: true — only if the deployment has no other
implementations of these interfaces; it changes the assumption, not what runs; removes every
blocker of this answer (3 open-world sites)`, and `not established: that it never commits`.

Rules:
- **Every input that removes a blocker kind is named**, with what it removes and what stays
  (Fable 2: the revision-1 rule "only when it is the sole blocker kind" printed nothing for c02,
  the call that motivated it). `removes every blocker of this answer` is printed only when it
  does; no line says the input "decides" the answer (astra: an assumption is not evidence, and the
  re-run's answer is not computed here).
- **`not established:`** states the conclusion the answer does not support, in the question's
  terms: `that it never commits` (may-commit), `that no caller turns this error into false`
  (callers-intercept-errors, with the rollback clause), `that a user is present`
  (callers-may-run-noninteractive, with the editing implication astra asked for, F79). This is
  astra's F21 point without the banned word: the owner rule (agent-tasks §5) bans "safe" and
  "unsafe", not a statement of what is not established. §11.2 row 3.
- The plain reading says "in the analyzed callers", never an unrestricted "no caller".
- With `detail: true`, `callers-intercept-errors` prints one representative uncaught route
  (F79, c29).

**Files:** `prove.rs` human, `callers_context.rs`.

### 4.5 Status says how hard it checked (F22)

**Current** c09 and c24 print the same headline: `FRESH: no input changed since generation 1
(checked just now: 582 files, 11 packages).` — c09 hashed every file, c24 compared size and time.
**New:** `… FRESH: no input changed since generation 1 (every input hashed just now: 594 inputs)`
vs `… FRESH: no change seen since generation 1 (size and modification time, just now; verify: true
hashes every input)`. This edits the agent-tasks §2.3 headline table; `check.strength` is added
(`hashed` / `size-and-time`). The "594 inputs" count is the sum of the categories actually
checked (§9.6), not a fixed formula. With no engine for the universe asked: `not checked`, as
today. `statusVersion` stays 2 (additive). **Files:** `src/mcp/freshness.rs`.

### 4.6 Touches table mode says "transitively" and shows sites (F23)

**Current** c30: `1802 routines touch it` and 12 routine headers from one codeunit.
**New:** `1,802 workspace routines touch it transitively; 214 sites in 96 routines touch it
directly` (numbers illustrative), then the direct sites (§9.3). A listed line is a site, never a
routine header. **Files:** `effect_query_cli.rs` touches table mode.

### 4.7 A change mode says what it computed (F27)

**New** first line of a digest or impact answer driven by a `diff`: it states the computation
actually performed, not the inputs given (astra), and whether the diff matched the analyzed
source:
- no base: `current effects of the 2 routines this diff touches, in the working tree (the diff's
  lines match it: yes); no before/after computed — pass base for that`;
- diff lines that do not match: `… (the diff's lines match the working tree: no, 3 hunks at
  uncertain positions; routines placed by name)`;
- base given, delta skipped (`delta: false`) or the base build failed: `base <ref> given; before/after
  not computed (delta: false | base build failed: <reason>)`.

With a computed base, impact already prints its comparison tuple. **Files:** `digest_cli.rs`
human header, `impact/render.rs`.

**Schema:** prove 2.5.0 (additive); explain, digest, query text only. **Goldens:**
`cli-b-goldens` (prove), `cli-explain-goldens`, `cli-query-goldens`, `cli-output-goldens` incl.
`cdo/`, impact and tests goldens. **Consumers:** ConTinAI `verify.py` reads `result.answer`,
`evidence`, `blockedBy`, `assumptions` — unchanged. **Verify (round 2):** c01 and c23 lead with
the conditional write; c10 prints "1 writer observed" with its search; c27 lists the 2 TestField
sites and the Get, and its summary carries the filter; c02, c19 and c29 name their assumption
inputs with what each removes and print `not established:`; c24's headline says "size and
modification time"; c30 says "transitively" and shows sites; c20's first line says no
before/after and that its diff matched.

---

## 5. Result first, and one result header (V4)

### 5.1 The answer leads (F28, F29)

**c06** (explain `callersContext`) **new** first section after the header:
```
  own state changes: sets 2 codeunit globals (ReportSelectionUsage, …); no record operation
  for adding a COMMIT, an error or UI here (callers' context):
    pending writes: YES — 8 witnesses of an uncommitted write when this runs; first ([local]):
      Page "CDO Sales Invoice Actions"::OnAfterGetCurrRecord → Rec.Modify at …:NN, then SetReportSelectionUsage at …:276
    intercepted errors: unknown (2 routines are public; re-analysis under assumeNoDependents: true removes every blocker)
    non-interactive: unknown (blockers: external callers 438, hubs 9,156, publication data not loaded, …)
```
The witness comes from the pending-writes walk the section already runs; explain prints its
first witness (tier, source of the write, order of write and call) instead of only the count.
The count says "witnesses" unless distinct callers were counted (astra). The non-interactive
line names every blocker kind by count, not the one that is easiest to explain. `own state
changes` (F75) lists what the routine itself changes: globals, record operations, or "none". **c17**: the "through dependency code / configured
dispatch" block moves above the caller list. **c21/c24**: covered by §5.2 (summary first).

### 5.2 One result header on every tool, and a summary always (F30, F42, F43, F61, F76, F77)

**Human output** starts with the same three lines (astra: human only; a JSON document never gains
a text preamble):
```
<tool> <resolved inputs, canonical names>   [universe, revision generation]
answer: <the one-line result>
analysis: <complete | partial (reasons)>; scope: <one clause>; more: <MCP inputs>
```
`answer:` is the verdict for prove, the selection line for tests, the accounting outcome for
impact, the headline for status, and the card's summary line for explain and digest (Fable:
explain has no yes/no). Variants: an unresolved input prints `answer: no answer — <why>` and the
input as given, not "resolved inputs"; status prints its headline(s) only.

**JSON documents** (F76) carry the same facts as structured fields in every standalone
document, readable without the MCP envelope: `payload.result.scope` with `analysis`
(`complete`/`partial` and reasons), `assumptions` (the full labels), `limits`, and, per operation
family where the tool reports families (F61: query effects `reads`, `writes`, `locks`,
`validate`; and the deep-write aggregate beyond dependency calls), the depth and whether the
family's inventory is exhaustive (`perOperation: "one dependency routine deep"`,
`beyondDependencyCalls: "writes only, whole cone"`, `locks: "one dependency routine deep; not
exhaustive beyond"`). Tests and impact add `selection` (`complete`, the cap asked, returned,
total, fallback).

**Envelope `summary`** (≤ 8 KiB, as today) gains, per tool: explain — each card's header and
summary line; prove — answer, filter, established/candidate counts, the assumption inputs and
what each removes; impact — accounting, outcome, first five routines, the tests line with run
target, scope and fallback; tests — `selectedTests` (≤ `maxTests`), totals, truncation,
`selectionComplete`, run target, fallback. Digest summary rows (F42, F77): **one row per stable
id** — four c15 rows, twenty c25 rows, each with its signature (astra: folding overloads into one
row would erase the differences the agent asked about). Facts shared by several rows are
factored into one `shared` block that the rows reference, so the rows stay small. Each row has
`tableAnswers` for every `writesTables` entry (`{table: {id, name}, answer, blockers,
firstBlocker}`) and its test status (`reached` / `no-known-path` / `not-loaded`, nearest test);
the test scope and fallback print once. query effects summary (F43) gains `writes.byTable` (top
10 with op counts, the total table count, a truncation flag, and caller-dependent and
unresolved-table counts apart).

**Cap rules** (astra): the 8 KiB cap never drops a requested verdict (`tableAnswers`, the prove
answer and filter), the selection up to `maxTests`, the fallback, or a requested root's row.
When those alone exceed it, the summary keeps them and drops per-row counts first; it always
says what it cut (`summaryTruncated: {rowsListed, rowsTotal, dropped: ["effectsByType", …]}`)
and points at the full document. `countsOnly` is replaced by this field.

The JSON writer sorts keys, so "first" in a document can only mean "in the summary"; that is why
the summary carries the selection, not a reordered document.

### 5.3 Boilerplate: keep the facts, drop the prose (F31, F32)

Wave-2 §3.7 says coverage, assumptions and limits are always visible, never folded. That stays,
and the assumption keeps its full sentence (astra: "loaded program" alone hides the exclusion).
What changes: when all three are trivial they print as one line, `analysis complete ·
assumptions: loaded program; unloaded implementations and subscribers are not enumerated ·
limits: none`. The static scope prose that is the same for every answer (temp records are not
writes, line base, which input lists everything) moves to the tool description, where it already
is. A scope fact that limits **this** answer stays beside it (astra): dependency depth when the
answer reached dependency code, an effect family not reported (`no lock facts`), literal-route
treatment when rows were ruled out. Empty sections (`extension points: none`, `events: 0 … 0
more`, `rules: none of its own; inherited: none`, `commits: none` repeated after the summary)
are not printed when the summary line already says it. A section about code the agent did not
ask about prints as a count, not a line (F81: c05's `telemetry at 1 place: TELEMETRY beyond
Codeunit Base64 Convert::ToBase64(InStream)` becomes part of the summary's kind counts). Touches
field mode drops its `note:` that repeats the `searched:` clause (F85). The dependencies line is one form everywhere: `dependencies: 11 apps`
in the header clause; the full list only with `all`/`detail`; the envelope's `universe` block has
them (finishing wave-2 Q6, which `finish_human` applied only where a `scope:` line exists).

### 5.4 Status answers in one copy (F33)

The text block becomes the headline(s), `check` (fresh, strength, complete, checked revision
and time, inputs by category including a test engine's production-source inputs, residual:
unsaved buffers and the same-size/same-time limit) and `refresh` per engine, about 500 bytes;
`not checked` with its reason when no engine matches; `structuredContent` keeps the full JSON (operator
data under `details`, as agent-tasks §2.3 set). This follows owner decision 3 (one copy of the
answer). **Files:** `src/mcp/server.rs` status, `freshness.rs`.

**Schema:** digest 3.1.0 (summary and `result.scope`, additive); query 3.6.0, prove 2.5.0,
explain `/4`, tests and impact version 1 gain `result.scope` additively; envelope `summary` for
explain/prove/impact/tests is new (MCP side, pinned by `mcp_server.rs`). **Goldens:** every human family (the header); the
envelope tests in `tests/cli/mcp_*.rs`; `cli-output-goldens/cdo/*.digest-summary.json`.
**Consumers:** bc-dev-mcp reads digest documents, not the MCP summary; no change. **Verify:**
c06 leads with pending writes and names a witness; c15's summary carries the CDO File answer on
each of its four rows; c25's summary has 20 rows, each with a signature and a test status, or
says what it cut; c21's envelope lists the selected tests and the run target; c16's JSON has
`result.scope`; c09's text is under 600 bytes; c01's and c11's dependency list is a count.

---

## 6. MCP-shaped follow-ups (V4)

### 6.1 Name MCP inputs, not CLI flags (F34, F37)

**Current:** `--all lists them` (c04), `--closed-world assumes …` (c19), `alsem prove <routine>
writes-table:<table>` (c11), `alsem diff <base> <new> gives them` (c13), `every one with detail or
format json` (c13), `--tier dependency lists them` (c07), `The next command builds it` (c14).
**New:** the MCP front end appends a hidden `--surface=mcp` to the argv it builds. With it, a
follow-up is a complete call, not a flag fragment (astra): `{tool, arguments}` with the resolved
selector, the question or filter and the scope inputs already filled, printed as
`alsem_prove {"routine": "<selector>", "question": "writes-table:CDO File", "closedWorld": true}`
and carried in JSON as `followUps: [{tool, arguments, why}]`. An in-place input change prints as
the input (`all: true`, `tier: "dependency"`). The no-engine status (c14) names an analysis call
that builds the engine (`alsem_explain {"routines": […], "singleApp": true}`), not "any tool"
(a second status call builds nothing). A command with no MCP tool (`alsem diff`) is named as
"not available over MCP" or dropped. The byte contract holds: the MCP answer equals the CLI's
stdout for the same argv, and the contract tests pass the flag. One helper owns the mapping
(`src/engine/follow_up.rs`, a table flag → MCP input), so no renderer spells a flag.

Tests: one fixture golden per tool rendered with `--surface=mcp` (Fable: a byte compare catches a
misspelled input, which a regex does not); every `followUps[].arguments` validates against the
tool's input schema in `tools.rs`; the invariant "no CLI flag in renderer-owned follow-up text"
covers `next:`/`more:` lines and fold notes only (astra: source excerpts, diffs and `---`
headings legitimately contain `--`). Break: drop the flag from the front end.

### 6.2 `next:` only when it applies (F35)

`next:` lines are built from the card, and a suggestion is dropped only when its answer is
already on the card (astra: an unknown or incomplete cone is exactly when a question helps):
`writes-table` for a write the card shows without a witness, or when writes are `no observed`
under partial analysis; `may-commit` when commits are `may` or `no observed` under partial
analysis, not when the card proves `none`; the callers' context questions when the card has
callers. c06 (complete analysis, no writes, no commits) gets neither. c01's "alsem prove names
the caller" is dropped: no prove question names which callers pass a non-temporary record
(revision 1 suggested `callers-have-pending-writes` for it; it does not answer that).

### 6.3 What a non-zero exit means (F36)

The envelope gains `outcome`: `answered`, `no-routine-changed` (complete accounting only, §1
F24), `input-not-mapped`, `not-found`, `ambiguous`, `usage-error`. Exit 0 for `answered` and
`no-routine-changed`, 2 for the rest (no answer). The descriptions say so once in the server
instructions.

**Goldens:** the CLI goldens do not move (the default stays CLI-worded); new `--surface=mcp`
fixture goldens, one per tool, in the `cli-output-goldens` family; the MCP contract tests and
`output_invariants_w3::descriptions_*` gain the flag cases. **Verify:** c04, c13, c19, c11, c14
name MCP calls with complete arguments; c06 has no writes-table or may-commit suggestion; c13's
envelope says `outcome: no-routine-changed`.

---

## 7. Size and dedup of large answers (V5)

### 7.1 Touches `callers: true` (F38)

**Current** c17: 961 lines, 320 KB; every caller line repeats `12 touches (12 on persistent
records, 0 …); first [CalcFields] read at …:740 in Table CDO E-Mail Log::GetEMailFromStorage, through …`.
**New:**
```
  up: all 952 transitive callers touch CDO E-Mail Log through other branches (not through SetFiles itself);
      shared first touch: [CalcFields] at CDOEMailLog.Table.al:740 (GetEMailFromStorage)
  also, not in the 952: 2 workspace callers only through dependency code; 81 only via configured dispatch
      (Workflow Response Handling::ExecuteResponse); the three populations are disjoint
  callers (952 routines in N objects), by object, nearest hops first:
    Codeunit "CDO E-Mail Template Management" (9): CreateDocumentsForRecord :… (+1 hop), CreateDocuments(<list A>) :… (+2 hops), CreateDocuments(<list B>) :… (+3 hops), …
```
Every caller stays, each with its identity (§2.1), definition location, hop distance and touch
classification (the task asked for all of them; the 952 rows are 952 distinct routines, and Fable
concedes the point). The "through other branches" relation stays. The three populations' counts
say whether they overlap (here: disjoint). The witness prints once when shared, and per row only
when it differs; the `callers` description says exactly that (F39). Target: under 60 KB on c17.
JSON: `up.callers[]` rows refer to a `witnesses` table by id. Query 4.0.0 together with §7.2.

### 7.2 query effects JSON (F40, F43)

**Current** c08: 1,005,860 bytes for 671 rows; `firstHop` and `origin.routine` (display, object,
stable id, anchor) inlined per row; `via: "inherited"` in all 671. **New:** digest 3.0's shape:
`payload.routines` (`r:<n>` → display, signature, object, definition anchor, stableId) once;
rows carry `firstHop: "r:12"` and `origin: {routine: "r:40", file, line, column}` — the
operation's own location stays on the row, apart from the routine's definition anchor (astra).
`via` moves to a document default (`payload.defaults.via: "inherited"`) and a row carries it
only when it differs. Target: under 200 KB on c08. **Schema:** query 4.0.0 (fields change shape). **Consumer:**
ConTinAI `effects.py` reads each row's `origin.anchor.line` (plan §4.1): it must read the routine
table. Tell its maintainers before merge; keep `--schema 3` for one release, as digest did
(owner decision 5).

### 7.3 Digest with many roots (F41)

**Current** c25: 1,182,595 bytes for 20 roots of one file, six `SendMail` overloads with ~190
effects each. **New:** the intrinsic facts of an effect (site, operation, table, effect type)
are shared: `payload.effects` holds each once. What can differ by root stays per root (astra):
each root lists its effects as references with annotations — literal liveness (live / ruled
out), temp state in that root's frame, path or crossing, conditionality, and ordering or
refutation evidence (`validForRefutation`). A root's view is the shared facts plus its own
annotations, and is the same as today's per-root entry. Test: a fixture site live from one root
and ruled out from another keeps both answers after sharing; and every root's reconstructed
entry equals the 3.x entry for the same input (break: drop the per-root liveness annotation).
`compact: true` stays one line, but the default (indented) form is what the tool documents for
file readers. Target: under 300 KB on c25. **Schema:** digest 4.0.0, `schema: 3` kept one release.
**Consumer:** bc-dev-mcp reads digest 2.x and is pinned to 1.7.1 (`satisfiesPin` refuses another
major), so it is not exposed until its pin moves; its readers (`risk.ts`, `alsem-writes.ts`) then
read `payload.effects` with `roots`. ConTinAI does not read digest.

### 7.4 Tests JSON (F44)

`hidden` becomes `{"<class>": {"reason": "…", "nodes": [...]}}` (reason once per class); a node
whose reason, channel or `selectionRelevant` differs from its class's keeps them on the node
(astra: grouping must lose nothing). `nearestTestedCaller` omitted when null. `al-sem.tests` version 2 (a field changes shape). No
outside reader yet (bc-dev-mcp adopts it later, agent-tasks Q10).

### 7.5 prove witness paths (F45)

Witnesses that share a tail print it once under a stable segment id: the first witness prints
`… → [segment S1: 7 workspace hops] → SetFiles` with the hops listed once under `segments:`, and
later witnesses print `… → [S1] → SetFiles` (astra: "see first witness" breaks when the order
changes). Hub sites, guard annotations and tiers stay on the hop that has them. A location
prints once per line (today `(at X) at X`); a `through` clause prints only when it names a
routine other than the `in` one (F84, c23's `in …::SetMergePDF, through …::SetMergePDF`). Blocker
kinds print as counts on one line, with the ones that carry an action (an unrecognized guard, a
missing `roots.config.json`) listed first. The traversal statistic `coverage: the reverse cone,
42651 states over 10091 nodes` (c02, c29) leaves the human text and stays in JSON (F83). Target:
c02 under 3 KB.

### 7.6 impact detail (F46)

With `detail: true`, a routine gets an evidence budget instead of ten items per list (astra: ten
per category is still a census): its direct callers and publishers with call sites, then one
representative per relation and tier (code local, code via-dependency, UI navigation, over a
hub), each on its own line, about 15 lines per routine; the counts per kind stay on one line. The
whole-change block is omitted when the change has one routine; "no base given" prints once in
the header (§4.7). Target: c20 under 4 KB.

### 7.7 explain caller rules (F47)

`caller rule (checked by 2: …; no equivalent check recognized in 41 analyzed callers: 5 pages,
14 page extensions — detail: true lists them)`; ties in "most actions" print as `6 objects with
5 each`.

**Schema majors in V5: three** (astra): query 4.0.0 (§7.1, §7.2), digest 4.0.0 (§7.3) and
`al-sem.tests` version 2 (§7.4). Each keeps the previous shape behind `schema: <previous>` for
one release (owner decision 5's pattern), and the word renames of §8 that touch these documents
ship in the same major (§8.3), so no reader sees two breaking changes.

**Goldens:** `cli-query-goldens` (JSON), `cli-output-goldens` digest JSON and `cdo/` summaries,
the digest size ratchets in `output_invariants*.rs`, tests JSON goldens. The ratchets are lowered
to the new targets in the same commits. The byte targets are ratchets, never a reason to drop a
requested identity or an annotation a decision needs (astra). **Verify:** c17 < 60 KB with 952
caller identities; c08 < 200 KB; c25 < 300 KB with every root reconstructable; c21 `hidden` once
per class; c02 < 3 KB; c20 < 4 KB.

---

## 8. Vocabulary and counting units (V0 decides; every later milestone uses it)

### 8.1 The word table (F49–F52)

One spelling per idea, used in human text and JSON values alike. Each row is a separate axis; a
path or a fact carries one value per axis (astra: tier alone cannot say "code or navigation").

| Axis | Words | Replaces / notes |
|---|---|---|
| **tier** of a path (where it goes) | `local` (no dependency hop and no hub), `via-dependency` (a dependency hop, no hub), `via-configuration` (a dispatch whose targets are decided by data the engine can name, such as workflow responses or report selections), `via-hub` (an unresolved dispatch hop) | replaces `onlyOpen`, `only-open`, `only over open sites`, `through an open site (a hub)`, `throughDependency`, `through-dependency`, `tier configured`. **Precedence** (Fable): a path's tier is its weakest hop, `via-hub` > `via-configuration` > `via-dependency` > `local`. Configured dispatch is its own tier, not a hub (both reviewers: a configured target can be decided by reading data; a hub cannot) |
| **result tier** (what an answer found) | `established witnesses: N` / `candidates over a hub: N` (and `over configuration: N`) | keeps the fact that **only** hub-mediated witnesses were found, which a per-witness tier loses (astra). JSON `establishedWitnesses`, `candidateWitnesses`. "Established" = by the static model, never observed |
| **relation** of a path (how it starts) | `code` (calls, runs, triggers, events) / `UI navigation` (a page the user opens from another page) | new axis; impact and tests rank code before navigation (§3.3) |
| **projection** | `approximate` flag beside the tier | replaces `localApproximate`, which must not silently become `local` (astra): the test universe's source projection qualifies the path |
| **hub kinds** (closed list, printed `[hub <kind> at <file>:<line>]`) | `Codeunit.Run of a variable`, `Page.Run of a variable`, `Report.Run of a variable`, `record trigger (<operation>)`, `interface dispatch`, `page variable (declared, never run)`, `background start of a variable` | replaces `h:run:codeunit`, `h:record:modify`, `HUB:run:report`. A hub is an **unresolved** target; when some targets are known (a closed enum, an interface with loaded implementations) the known ones print as a list, not as a hub (astra). Configured dispatch is a tier, not a hub kind. The implementing commit checks the list against `impact_graph` hub classes (`any`/`name`/`run`/`record`/`callbacks`) and adds any it misses |
| **execution entries** | `page action`, `page trigger`, `API page`, `web service` (published from `roots.config.json`), `job queue`, `background start`, `install`, `upgrade`, `report` (request page, data-item triggers), `codeunit OnRun` (a literal `Codeunit.Run`), `table trigger`, `event subscriber`, `public procedure` (callable by other apps), `test` | replaces `trigger-page`, `page-event`, `page-action`, `page triggers`. Only the entry routines of an install or upgrade codeunit are `install`/`upgrade` entries, not its helpers (fact check c02 #7, on `fix/explain-callers-counts`). **Execution context** (API, job queue, background) is a separate axis from the entry kind, since they overlap (astra): a job-queue run of a codeunit's `OnRun` is `codeunit OnRun` in context `job queue` |
| **dependency boundary** | `dependency call` (the crossing), `dependency routine entered` (the target) | replaces "entry point" in explain/digest/query. "entry" alone always means an execution entry (Fable) |
| **verdict** | `yes` / `no` / `unknown`, wherever a verdict exists: prove, digest `tableAnswers`, touches `down`, explain's `writesTables` | astra: "prove only" contradicted the spec's own `tableAnswers` |
| **negative certainty** (Fable) | `no (every candidate decided)` — proven absent under the stated assumptions; `none observed (searched: …; not searched: …)` — an index found nothing, with its search; `unknown (blocked by: …)` | `0 writer(s)`, `no observed write`, `no indexed consumer observed` all become the middle level |
| **target resolution** | `table: <name>` / `table: not resolved (<receiver kind>: <reason>)` | replaces `isUnknownBucket`, `unknown` as a table, "decided at run time". A RecordRef does not prove the table varies at run time (astra: some RecordRefs resolve statically), so the reason says what failed, e.g. `RecordRef opened from a variable` |
| **temp state** | `temporary` / `not temporary` / `caller-dependent (parameter 3, <name>)` / `not resolved` | replaces `known(true)`, `known(false)`, `parameter-dependent(i)`, `temp state unknown`; independent of access kind and reachability |
| **condition** of an effect | `unconditional on success`, `in a loop`, `conditional`, `condition not resolved` | replaces the bare `(unknown)` after an error site (c05) |
| **completeness** | always with a subject: `analysis complete|partial`, `selection complete|incomplete`, `freshness check complete`, `tests with a modeled path` (never "coverage" for static reach; changed-line execution is not established); and the non-results `not analyzed`, `not requested`, `not loaded`, `not applicable`, which are not synonyms for partial or empty (astra) | replaces bare `complete`, `coverage: complete`, `testCoverage` status words |

Fable's proposal to pair `not resolved` with `run-time` on every axis is adopted for `not
resolved` and declined for `run-time`: the engine has no fact that proves a target is a run-time
value (§11.2 row 5).

### 8.2 Count units (F53, F54)

Every count names its unit and, where two counts could be compared, how they relate: c05
`writes beyond dependency calls: none (1 dependency call writes only temporary records)`; c15
`commits: 1 site (2 more under literal arguments that cannot run)`; c08 summary `671 operations on
81 tables one routine deep; whole cone beyond 14 dependency calls: 397 tables (writes only)`;
c29 `uncaught at: page actions 30, page triggers 42 (distinct entries; an entry is counted
once, in its own kind)` — a total prints only when the kinds are disjoint by construction, and
the unit (entries, paths, states) is named (astra); c17 `+3 hops`; c22 `consumers: 3 sites, 1
candidate`. Operations, sites, tables, routines and entries are counted apart, and a category
sum is never printed as a total of distinct things. Touches tags get a one-line
legend under the first tagged row (`implicit-access: a bare field name inside the table's own
code`). Wave-2 §3.6 is the rule set for the units; this adds "print the unit".

### 8.3 JSON migration rule (astra)

Replacing a value inside an existing field is a breaking change, not an additive one. So: a
renamed value goes into a **new field** beside the old one (`tier: "onlyOpen"` stays, `pathTier:
"via-hub"` is added); the old field stays for one release, then leaves in the next major. For
query, digest and tests the old fields leave in their V5 majors (§7), so readers see one break;
for prove, explain and impact the new fields are a minor bump (prove 2.6.0) and the old ones
leave in their next major. The vocabulary invariant (no replaced word) covers renderer-owned
human text and the new fields only, not legacy fields during the window or quoted source.

**Goldens:** every human family. **Verify:** round 2's bundle has none of the replaced words in
renderer-owned text (a grep over the 30 answers, also kept as an invariant on the fixture: break
= put one old word back).

---

## 9. Per-tool items (V6)

### 9.1 explain (F55, F56, F67, F48, F72, F80) — F48, F55, F56 land in V2

- **Calls first** (F55): `calls: Codeunit "CDO E-Mail Template Import/Export"::ExportTemplates
  (:181)`, the direct callees in source order, at most 5 (impact's routine block too), and, for
  each effect the card leads with, its short route from the routine (`UI: DownloadFromStream at
  CDOEMailTemplateImportExport.Codeunit.al:24, via ExportTemplates → ExportAsXMLDialog`): a
  callee list alone does not explain how the action reaches its effect (astra).
- **Tables named when few** (F56): `writes: CDO Setup (Modify at :62; the routine assigns <its 3
  fields>)` when 3 tables or fewer; the field list follows wave-2 2d (the routine's assignments,
  on INSERT/MODIFY/VALIDATE only) and is labelled as the routine's assignments, not as what each
  `Modify` writes; receiver and control-flow qualifications stay (astra). Impact's `may write 1
  table` becomes the same.
- **Caller rules** (F67): `caller rule: rejects when not VariantRec.IsRecord; checked by 2
  callers; no equivalent check recognized in 41 analyzed callers` ("skipped" reads like a bypass,
  "not checked" like a proof of absence).
- **showSource** (F48): the excerpt shows the executable line (the call or the operation), not
  the declaration; the root's own excerpt starts at its first statement.
- **Own events vs reached events** (F72): `events raised (17, downstream)` and `publishes: none
  of its own` on one line. **Event names** (F80): the distinct event names print when they come
  from 5 publishers or fewer (`Codeunit "CDO Events": OnBeforeExportTemplate, OnAfter…`), else
  `detail: true lists them`.

### 9.2 query effects (F57) — V2

The summary always prints `locks:` and `reads:` (with `none observed` when zero), since a lock
question (c11) must not depend on a missing line meaning zero. Each carries its scope (astra):
`locks: none observed (one dependency routine deep; deeper dependency locks are not
inventoried)`. The counts include every temp class and unresolved-table operations (§1 F15).

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
(numbers illustrative). "Direct" means the operation's own routine is a workspace routine whose
body holds the operation on this table; the full routine list stays available with `all: true`.
c18's `unknown` table gets the same shape, with each site's receiver and why the table is not
resolved, and the warning that an unresolved write can be any table. Key-relevant uses (not
exhaustive, and labelled so): `Get` sites (they use the primary key), SetCurrentKey sites (from
the operation index), and SetRange/SetFilter on key fields, which needs the field-use index
joined with the table's declared keys — field mode already reads them (`field_touches.rs`
`declared_keys`, advertised in `tools.rs`). If the join costs more than a second on CDO, ship
`Get` and SetCurrentKey only and mark the rest [engine].

**Field mode** (F60, F74, astra): confirmed named references (`read`, `write`, `validate`,
`filter`, …) and numeric candidates (`RecordRef.Field(8)` on an unresolved table) print as two
inventories, with the note that a numeric reference does not break on a rename (it may on a
renumbering). `filters:` always prints, with `none observed within the searched bodies` when
empty. The verdict says `not a complete rename inventory: page and report columns, CalcFormula
and TableRelation are not searched (the compiler finds them); keys holding the field are listed
above`. Revision 1 wrongly said keys are not searched: the table's declared keys are read.
Searching the other declaration-level consumers is [engine] (§10).

### 9.4 impact (F62)

`object headers: 25 files, line 1 only — a byte-order mark removed (no declaration changed)` when
the old and new lines differ only by a leading BOM, or by whitespace outside quoted names and
values (astra: whitespace inside a quoted identifier or literal is a real change); otherwise the
changed property names. The accounting has both line texts.

### 9.5 tests (F63, F64, F82) — F63 lands in V2

The answer **starts** with what to run (astra), in the test app's terms:
`run first (app "Continia Document Output 365 - Test"): codeunits 68960|68934|… — a superset:
whole codeunits, N tests, for the 369 selected procedures; then the whole suite (selection
incomplete: <why>)`. When the selection is exact, the procedures are listed instead and the line
says `exact`. The same run line, scope and fallback appear in impact's tests line and digest's
test status (§5.2). `test scope:` prints one clause (`1 test app, 1,844 tests; 4 apps skipped:
alsem_status lists why`); the symbol-download paragraph prints once as `missing test-library
symbols: 7 apps (Library Assert, …) — add them to Test/.alpackages` (agent-tasks §13.3 Q3's
specific next action, shortened). `production bytes: the same in the production and test
universes` (c07, c28) becomes `test universe: the same production source as the main engine`
or is dropped when nothing differs (F82).

### 9.6 status (F65)

`inputs` lists every category the check hashed or compared, from the categories themselves
(astra: not a fixed `appJson: 1`): on c09 `alFiles 582, packages 11, appJson 1`, which adds up to
the 594 hashed.

### 9.7 prove JSON (F66)

The prove description and schema note say `deterministic` concerns `generatedAt` only, never the
answer.

**Goldens:** `cli-explain-goldens`, `cli-query-goldens`, impact and tests goldens,
`cli-output-goldens/cdo/`. **Verify:** c05 names the callee, the route to its UI effect and the
17 event names; c20 names CDO Setup and the assigned fields; c11 prints `locks: none observed`
with its scope; c30 and c18 show ops by kind and direct sites; c22 separates the numeric
candidate and prints `filters:`; c13 says "byte-order mark"; c07 and c28 start with what to run;
c09's inputs add up.

### 9.8 Other per-answer items (F73, F78)

- **Guarantees and refutation** (F73, c15, astra): a digest guarantee whose evidence is
  `validForRefutation: false` prints as a description (`ordering: commit after insert,
  descriptive only`), never beside refutable facts without that label; the JSON keeps the flag
  next to each guarantee and the summary counts the two apart.
- **Location roles** (F78, astra): every printed location says what it is — `defined at` (a
  routine's declaration), `called at` (a call site), `at` (an operation) — and JSON location
  objects carry `role: definition | callSite | operation`. c18 and c30 printed definition lines
  where operations were asked for; this makes the difference visible everywhere.

---

## 10. Engine notes

Kept apart, per the rubric. None blocks a milestone above.

- **Declaration-level field consumers** (F60, c22): page and report columns, CalcFormula,
  TableRelation are not indexed (declared keys are). A rename inventory needs them. Output-only
  stopgap: §9.3.
- **Key-relevant uses in table mode** (F59): possible from existing indexes; measure the join (§9.3).
- **Company context of a write** (F75, c20): whether the model records `ChangeCompany` on a record
  variable, so a write can be labelled with the company it targets, is not verified. Check before
  V2 implements F75's company part; without the fact, F75 prints the routine's own state changes
  only.
- **Deep reads and locks in query effects** (F61, c08): deferred engine capability (plan §5 Q4
  keeps per-operation detail one dependency routine deep). The output part, saying so per
  operation family, is accepted (§5.2).
- **DB-event subscribers** (c01, c11): on `fix/db-event-subscribers`.
- **`declaredRunnable` and page navigation** (c12, c20): the first on `fix/reach-and-test-selection`;
  navigation stays by design and is labelled (§3.2).
---

## 11. Where the reviewers disagree

### 11.1 Round-1 reviews (of the outputs)

| Topic | Fable | astra | Recommendation |
|---|---|---|---|
| c17 caller list | dedupe by routine, ~40 lines | keep every caller, share the witness | **astra**: the task asked for every caller, and Fable's duplicates are distinct overloads and actions (952 rows = 952 callers, checked). Share the witness, group by object (§7.1). **Settled**: Fable conceded in the spec review |
| Boilerplate | one short `scope:` line or the envelope | scope and completeness beside the answer, standardized | **both, within wave-2 §3.7**: the three facts stay visible as one line; static prose goes to the description (§5.3) |
| c13 outcome | exit 0 | a distinct outcome code | **exit 0 plus `outcome`** (§1 F24, §6.3): agents treat non-zero as failure; the outcome tells cases apart |
| c19 closed world | name `closedWorld: true` | warn it can hide the problem | **both**: name it as a re-analysis under an assumption and say it changes the assumption, not what runs (§4.4; wording settled in §11.2 row 2) |
| c12 whole-program reach | omit, "not specific" | a separate optional section | **one line with counts by tier and "not specific"**; lists only with `detail` (§3.2) |
| c26 ambiguity | one card per overload when ≤3 | a copyable request for both | **astra**: a card the agent did not ask for may be the wrong one; the error stays exit 2 with a ready value (§2.4) |
| c19 wording | — | "does not establish safety" | revision 1 rejected it; **revision 2 accepts the substance** without the banned word: `not established: that it never commits` (§4.4, §11.2 row 3) |
| c02, c07 | "partly" (layout makes candidates look like evidence) | "yes" | no output decision depends on the verdict; §3.1 and §3.3 apply either way |
| c18 | "partly" | "no" (no operation is identified) | astra's reading: §9.3 makes the answer name operations. Fable agrees in the spec review |

### 11.2 Spec reviews (of revision 1): conflicts and decisions

| # | Topic | Fable | astra | Decision and reason |
|---|---|---|---|---|
| 1 | Display vs selector | make the display a selector (extend the grammar with member forms), or print the stable id | keep `display` and a guaranteed `selector` apart; round-trip tests; captions off the hops | **Both, combined** (§2.1): extend the grammar (Fable's preferred option) so the human display is pasteable except for its caption, and emit `selector` in JSON with a round-trip test (astra). The two are compatible; neither reviewer's reason argues against the other's mechanism |
| 2 | Naming an assumption input on `unknown` | `would narrow it:` (removes some blockers) and `would decide it:` (removes all) | no "would decide it": an assumption is not evidence; "conditional re-analysis" | **astra's label, Fable's content** (§4.4): `re-analysis under an assumption:` with what it removes and what stays, and `removes every blocker of this answer` when true. "Decide" is dropped because the re-run's answer is not computed here and could still be unknown or yes; Fable's need — tell narrowing from full removal — is met by the removes/stays clause |
| 3 | F21 ("does not establish …") | agrees with revision 1's rejection (the owner rule) | reverse it: "transaction preservation is not established" uses no banned word | **astra** (§4.4): the owner rule bans the words "safe"/"unsafe", not a statement of what an answer does not show, and Fable's agreement was with keeping the banned word out, which still holds. `not established:` lines in the question's terms. Owner may overrule if the rule was meant more widely; flagged in the report |
| 4 | Overload signatures | also on list rows, not only path hops | on every occurrence of an overloaded name | **astra's rule** (§2.1), which includes Fable's: every occurrence, short names when not overloaded |
| 5 | Unknown words | one pair `not resolved` / `run-time` on table, temp and condition | a RecordRef does not prove a run-time table; use `not resolved` with receiver and reason | **astra** (§8.1): `not resolved (<receiver>: <reason>)` on every axis; `run-time` is not used, because the engine has no fact that proves a target varies at run time. Fable's goal (the same words on every axis, saying why) is kept through the reason |
| 6 | Folding a whole-program reach | fire on the total share or on one hub's fan-out | the share may fold, never label; split by relation regardless of size | **Both** (§3.2): Fable's triggers decide folding only; astra's rule that labels and the code/navigation/hub split never depend on size |
| 7 | Digest summary rows | (accepted revision 1's folded "distinct routine" rows) | one row per stable id, never folded | **astra** (§5.2): the c15 task compares overloads, so folding loses the asked-for difference; sharing common facts keeps the size down. Fable did not argue for folding |
| 8 | Impact `detail` | (accepted ten items per list) | an evidence budget | **astra** (§7.6): a census with shorter lines is still a census |
| 9 | Moving work earlier | F42 `tableAnswers`, F57 into V2 | also F13, completeness subject, candidates, lock scope, exact selection, F55/F56/F48 | **both** (§0.1): V2 grows to everything that misleads or leaves a hazard without its evidence. V2 is now the largest milestone; it can split into V2a (wording, F10–F29) and V2b (evidence lines, F48/F55/F56/F57/F63) if the implementer wants two branches |

No conflict is left open. Row 3 depends on how the owner reads the agent-tasks §5 rule; the
decision follows its words.

---

## 12. How it is verified

### 12.1 Tests

- **Semantic gates** (astra), beyond goldens and byte counts, each with a discrimination proof:
  - every emitted `selector` resolves to its own stable id (§2.1);
  - shared evidence reconstructs each root's and each row's original meaning: a digest root's
    entry, a query row's operation location, a prove witness's hops through its segment (§7);
  - read, lock and validate totals include every temp class and unresolved-table operations (§1 F15);
  - the summary cap keeps requested verdicts, the selection up to `maxTests`, the fallback and the
    truncation metadata (§5.2);
  - `unknown` stays `unknown` unless new evidence or an explicitly changed assumption is given
    (no renderer turns it into a softer "no");
  - zero resolved roots from input that did not map is `input-not-mapped`, never
    `no-routine-changed` (§1 F24).
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

### 12.2 Fix-branch gates (check and date before the dependent commit)

| Branch | Findings it handles (handled elsewhere) | Spec work that waits for it | Merged (date, commit) |
|---|---|---|---|
| `fix/field-writers-implicit` | c10, c01 (F71) | §4.2 F17 | open |
| `fix/explain-callers-counts` | c06 ×2, c02 ×2 (F68, F69) | §5.1 F29 counts, §8.1 install/upgrade entries | open |
| `fix/db-event-subscribers` | c11 (F58) | the F58 stopgap limit line is dropped only after this merges; if V2 lands first, V2 adds the limit `table-event subscribers are not followed` and removes it when the branch lands | open |
| `fix/reach-and-test-selection` | c12 ×3, c20, c21, c25 (F11, F26, F70) | §3.2 F12 (the share is measured after it), §3.3 F13, §5.2 digest test status | open |

Each handled-elsewhere finding is verified in round 2 on the call that raised it, like every
other finding.

---

## Appendix A. Findings register (round 1)

Columns: class = correctness bug / design change / handled elsewhere (another branch fixes it;
an integration and round-2 obligation, §12.2); priority: high = can mislead an agent, medium =
missing or inconsistent, low = noise or size. "Raised by": fable, astra or both (in revision 2,
"spec review" marks an item a reviewer added when reviewing revision 1).

Revision 2 changes: F20 and F63 raised to medium, F22 and F50 to high (astra: each can mislead a
decision); F21 reversed to a design change; F61 split (its output part is a design change, its
engine part deferred, §10); the seven "already fixed" rows moved from "rejected" to "handled
elsewhere"; F73–F79 (astra's missing items) and F80–F85 (Fable's six low items) added. No
finding is rejected any more.

### A.1 Counts

| Class | High | Medium | Low | Total |
|---|---|---|---|---|
| correctness bug | 2 | 3 | 2 | 7 |
| design change | 12 | 35 | 24 | 71 |
| handled elsewhere | — | — | — | 7 |
| **all** | | | | **85** |

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
| F13 | Tests: no tier per group; nearest reads as focused; no code-vs-navigation relation in the rank (tests already sort by tier and hops, `select.rs:457`) | c07, c21, c28 | both | medium | §3.3 |
| F14 | Conditional write led by "no observed write that may persist" | c01, c23 | both | high | §4.1 |
| F17 | Field writers: bare `0 writer(s)` with no search statement | c10 | both | high | §4.2 |
| F18 | Filtered negative hides the other reachable sites | c27 | both | high | §4.3 |
| F19 | unknown does not name the deciding input or the plain reading | c02, c19, c29 | both | medium | §4.4 |
| F20 | Interception answer may read as a rollback guarantee | c29 | astra | medium | §4.4 |
| F21 | An unknown does not say which conclusion it does not establish ("transaction preservation is not established"), without the banned word (reversed in revision 2) | c19 | astra | medium | §4.4 |
| F22 | Status headline same for hashed and size/time checks | c09, c24 | astra | high | §4.5 |
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
| F50 | "complete"/"coverage"/"reached" without a subject | c09, c21, c25, c28 | astra | high | §8.1 |
| F51 | "entry point" with two meanings | c05, c06, c11, c12 | astra | medium | §8.1 |
| F52 | Tier and entry-kind words spelled many ways | c02, c06, c07, c12, c20, c29 | fable | medium | §8.1 |
| F53 | Count units not stated or reconciled | c05, c08, c15, c17, c22, c29 | both | medium | §8.2 |
| F54 | Touches tags without a legend | c22 | both | low | §8.2 |
| F55 | No direct callees in explain and impact | c05, c12 | both | medium | §9.1 |
| F56 | Tables (and fields) not named when few | c20 | both | medium | §9.1 |
| F57 | query effects: no `locks:`/`reads:` line when zero | c11, c23 | both | medium | §9.2 |
| F59 | Touches table mode: no ops, no direct sites, no key-relevant uses | c18, c30 | both | high | §9.3 |
| F60 | Rename: declaration-level consumers (columns, CalcFormula, TableRelation; keys are read) not searched; say so [engine] | c22 | both | low | §9.3, §10 |
| F61 | query effects: no machine-readable scope per operation family (reads, writes, locks; deep writes); the deep read/lock inventory itself is deferred [engine] | c08, c11 | astra | medium | §5.2, §10 |
| F62 | impact: header change kind not said (a BOM) | c13 | both | low | §9.4 |
| F63 | tests: no runnable target first; exact selection vs codeunit superset not said | c07, c28 | both | medium | §9.5 |
| F64 | tests: scope and symbol paragraphs repeat | c07, c28 | both | low | §9.5 |
| F65 | status inputs omit app.json (594 = 582 + 11 + 1) | c09 | astra | low | §9.6 |
| F66 | prove `deterministic: false` unexplained | c16 | astra | low | §9.7 |
| F67 | "skipped by" reads as a guard bypass | c06 | astra | low | §9.1 |
| F48 | showSource excerpts show declarations, not the call | c04, c05 | both | low | §9.1 |
| F72 | Own events vs reached events not told apart | c05 | astra | low | §9.1 |
| F73 | Digest guarantees not told apart from evidence marked `validForRefutation: false` | c15 | astra (spec review) | medium | §9.8 |
| F74 | Touches field mode: confirmed references and numeric candidates mixed; no `filters:` result; rename sensitivity of a numeric reference not said | c22 | astra (spec review) | medium | §9.3 |
| F75 | explain/impact: the routine's own state changes (c06: two globals) and the company a write targets (c20) not stated; the company part needs an [engine] check | c06, c20 | astra (spec review) | low | §5.1, §10 |
| F76 | Standalone JSON documents lack scope, assumptions and completeness without the MCP envelope | c16 (all JSON) | astra (spec review) | medium | §5.2 |
| F77 | Digest summary rows lack per-routine test status | c25 | astra (spec review) | medium | §5.2 |
| F78 | Locations do not say their role (definition, call site, operation) | c18, c30 (all tools) | astra (spec review) | medium | §9.8 |
| F79 | c02: no editing implication ("do not assume a user is present"); c29: no representative uncaught route under `detail` | c02, c29 | astra (spec review) | low | §4.4 |
| F80 | The 17 raised events folded as `Codeunit CDO Events 17`, no names | c05 | fable (spec review) | low | §9.1 |
| F81 | `telemetry at 1 place … Base64 Convert` is noise for an export action | c05 | fable (spec review) | low | §5.3 |
| F82 | `production bytes: the same in the production and test universes` unexplained | c07, c28 | fable (spec review) | low | §9.5 |
| F83 | `coverage: the reverse cone, 42651 states over 10091 nodes` is a statistic in human text | c02, c29 | fable (spec review) | low | §7.5 |
| F84 | `in …::SetMergePDF, through …::SetMergePDF` names one routine twice | c23 | fable (spec review) | low | §7.5 |
| F85 | Touches field `note:` repeats the `searched:` clause (also in F31's text) | c22 | fable (spec review) | low | §5.3 |

### A.4 Handled elsewhere

Not design work here; each is an integration obligation (the branch merges) and a round-2 check
on the call that raised it (§12.2). Where a reviewer's presentation point goes beyond the fix,
it is kept as its own design row (named in the last column).

| id | Summary | Calls | Raised by | Branch / note |
|---|---|---|---|---|
| F11 | impact `first:` items without tier | c12, c20 | both | `fix/reach-and-test-selection` (`node_short`, `reached_clause`); the size-independent split is F12 |
| F26 | digest `testCoverage` "complete" with 0 tests | c25 | both | `fix/reach-and-test-selection` (`NoTestCode`, `test-apps-not-loaded`); astra's presentation points (test scope and per-routine status in the summary) are F50 and F77 |
| F58 | limit line "DB-event subscribers not followed" | c01, c11 | fable | `fix/db-event-subscribers` (the cone follows them); a dated stopgap in §12.2 if that branch lands after V2 |
| F68 | no bucket for page triggers among callers | c06 | fable | `fix/explain-callers-counts` |
| F69 | upgrade helpers counted as entries; guard line | c02 | fable | `fix/explain-callers-counts` |
| F70 | runFirst prefers add-in-event paths | c21 | both | `fix/reach-and-test-selection`; astra's presentation point (relation in the rank) is F13 |
| F71 | "UI and errors: none" for an added `Modify()` | c01 | fable | `fix/field-writers-implicit` |

Ids are not in theme order: they were numbered as findings were merged.
